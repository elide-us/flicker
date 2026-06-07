# Handoff — the layered field model & water cycle (POC proven, ready to expand)

> Captures the model proven out in `examples/hex-world` over a design+build run:
> a stack of heightmap-style **field layers** per hex, a conserved **water
> cycle**, a star-driven **radiative atmosphere**, **climate→biome** emergence,
> and **multi-hex joining**. The hex-world example is throwaway POC; the *model*
> and the reusable nucleus (`layers.rs`) are the load-bearing part. Re-verify
> code anchors (names over line numbers — they drift). This doc both records the
> decisions and frames where expansion makes sense.
>
> Relationship to `docs/flicker-world-system-spec.md`: this is the **concrete,
> running** realization of that spec's §3.1 (three-layer column), §4 (mass
> conserved / shape disposable), §6 (water cycle), and §12 (renderer as
> downstream reduction). Where the spec leans on the optimized "Rivulet DAG",
> this POC uses the simpler cellular-automata flow the spec calls the naive
> version — see *Expansion*.

---

## 0. The one boundary everything hangs on — substance vs meaning

The most important line we drew. It is **not** depth (surface vs underground);
it is:

- **Geo-sim — one conserved model, top to bottom.** Material/air/composition
  truth, flow, void formation, erosion. Surface water and an underground lava
  tube obey the *same* conservation laws.
- **Gameplay — a separate discipline.** *Where* the ten resource veins sit,
  *what* a cave means for play, harvest economy, caves-as-authored-content
  (the §10 dungeon "sampler"). Layered **on top of** the geo-sim truth.

A resource vein is therefore **not a structure** — it is a *property of the
composition field* (copper-fraction over threshold across a vast region). That
is why it can be planetary-scale and effectively infinite: nothing enumerates
it, it is queried, and harvest decrements a huge ledger quantity. Walling off
the *meaning* costs nothing in the *substance*; they share one truth.

---

## 1. Render classes — the layer taxonomy (load-bearing)

Every layer is one of three classes. Mixing them up (drawing an influence field
as relief) was a real early mistake; keep them distinct.

| Class | What | How it renders | Conserved? |
|---|---|---|---|
| **Substance** | ground, water, ice, lava, cloud | **mesh** (relief) | water/ice yes; lava=rock mass; cloud=derived |
| **Influence field** | temperature, pressure, wind, humidity, stratosphere, thermosphere | **flat heatmap** (colour-ramped, no relief) | no — they *drive* other layers |
| **Control / forcing** | GM-lever layer (heat zones, lava/material injectors) | (a field; setpoints) | no — metered external source |

Heatmaps are drawn flat **on purpose** — their job is to influence, not to be
geometry. The viewer renders a heatmap by packing a two-stop colour ramp into
the mesh shader's material word (`pack_ramp(cold, hot, t)` → shader does
`mix(cold,hot,t)`), so a flat sheet becomes a continuous heatmap **with no
shader change**. The demo palette (`crates/flicker-render/src/shaders/mesh.wgsl`)
was extended with lava/ice/land, aurora/UV/void, and 7 biome colours (indices
1–23).

---

## 2. The vertical column (full stack, bottom → top)

Causation is **bottom-up** (spec §3.1): the GM-lever layer drives everything
above it.

```
  atmosphere      thermosphere · stratosphere(ozone) · [cloud decks]   ← influence + substance
                  humidity · pressure · wind · temperature             ← influence fields
  surface         water · ice · lava · ground                          ← substance (water cycle here)
  resource/middle veins, gameplay materials, (caves*)                  ← harvest & build (gameplay)
  GM levers       flat forcing heightmap (heat zones, injectors)       ← the engine room
        └──────────────────────────────► drives all layers above
```

The water cycle runs in the **top three sub-layers** (surface + atmosphere).
The two heat sources for the temperature field are the **sun from above** and
**geothermal/GM from below**.

`*` Caves are deferred (see §6).

---

## 3. The water cycle — conserved, ticking (`layers.rs`)

`LayerStack::tick(dt)` runs an ordered pass pipeline. Each pass is a **transfer**
(never create/destroy); closed boundaries make total water **conserved**.

0. *(future)* apply GM levers — inject heat/lava/material from below.
1. **`update_thermal`** — radiative cascade (see §4) → temperature (+ upper atm).
2. **`update_wind`** — pressure `= −temperature`; wind `= −∇pressure +` prevailing.
3. **`evaporate`** — `water → humidity`, rate ∝ temperature over exposed water.
4. **`advect_humidity`** — conservative upwind flux along wind (closed boundary).
5. **`condense_precipitate`** — air capacity falls with cold/altitude (orographic
   lift); supersaturation rains out as **water if warm, ice if freezing**; `cloud`
   records saturation.
6. **`runoff`** — water flows downhill over `ground+ice+lava`, pooling in basins.
   **Water's level is `ground + water` per cell** — contour-local, not a global
   plane (each basin finds its own surface).
7. **`melt_ice`** — ice → water above freezing.
8. **`update_climate`** — integrate weather into climate (see §5).

**The invariant:** `Σ(water + ice + humidity)` is constant per tick (lava is a
separate rock-mass pool; the only cross-pool exchange would be water→steam over
lava). This is spec §4's "mass cannot drift," and it is the one property guarded
by test (`water_mass_is_conserved_across_many_ticks`: <0.1% drift / 300 ticks).
The GM-lever layer, when wired, is the **one legitimate, metered** source/sink —
a logged boundary input, not a leak.

---

## 4. The atmosphere — a radiative cascade from the star

The upper atmosphere is pure influence-field heatmap, driven entirely by stellar
flux absorbed **band-by-band on the way down**:

```
star flux ─► thermosphere (eats hardest radiation; extreme day/night swing)
                 │ residual
            ─► ozone/stratosphere (eats UV → warms with altitude: the inversion)
                 │ residual
            ─► troposphere/surface (only the residual flux heats the ground)
```

Because only the **residual** reaches the surface, the upper atmosphere
genuinely modulates surface temperature — proven by `ozone_shields_the_surface`
(punch an ozone hole → more UV through → hotter ground). The sun is a **planar
day/night wave in world coordinates** (`insolation`), so a joined map shares one
continuous terminator. This is the natural home for celestial events: wiring the
repo's existing `celestial_dir`/eclipse cycle into `insolation` makes an eclipse
a cold shadow sweeping these heatmaps, for free.

---

## 5. Climate → biome (emergence)

**Biomes reflect climate, not weather.** `climate_temp`/`climate_moisture` are a
slow exponential average (`CLIMATE_RATE`) of the flickering weather. Biomes
classify off the average, so the map is stable while day/night cycles.

`biome_material` is a Whittaker temperature×moisture grid (cold/temperate/hot ×
dry/mid/wet → tundra/taiga/grassland/forest/savanna/desert/rainforest) with
**freezing → tundra/taiga** and **alpine → bare rock** overrides. Critical
tuning lesson: **classify by terciles, not min/max** — min/max spreads the range
but clusters everything mid (first pass came out 90% grassland). Terciles split
land into actual thirds → a real 8-biome spread. Biome **colours the realized
surface** (the game-world ground) in both views; it is derived (read-only), no
new conserved state. Guarded by `biomes_differentiate_from_climate` (≥5 kinds).

---

## 6. Scale, the realized surface, and caves

- **Scale: one field cell ≈ one cluster.** This layer is the **macro / LOD8
  world-data** resolution. `realized()` (substance composite: `ground + topmost
  thickness`, painted by the top layer) is the LOD8 resting surface.
- **The per-cluster gameplay bake is the layer *below this*, out of scope here.**
  One pixel expands into a full 256³ voxel cluster (the "Russian doll"), where
  `flicker-voxel`'s contour runs for real with octave-stacked sub-detail
  generated on approach (spec §7). The realized composite is the *input* to that,
  not that.
- **The renderer already handles full 3D.** Contour samples
  `Primitive::is_solid(x,y,z)` — a heightmap is just the *monotonic* case
  (`y < height`). A cave is any non-monotonic column (air below solid). So caves
  are a **data-representation** question, not a rendering one: generalize a column
  from fixed slots to a **run-length stack of `(material, thickness)` including
  air**; a void is an `air` run, governed by the existing hardness/brittleness
  traits. This is spec §14's deferred true-3D step — continuous with this model,
  not a new paradigm.

---

## 7. Multi-hex joining

- **Terrain continuity is free.** The world heightmap is a continuous function
  (its own module guarantees adjacent samplers agree), so joining hexes is "more
  tiles sampling the shared function." The viewer's **World view** lays out
  concentric hex rings (`RINGS`, axial coords on the proven bubble spacing) and
  draws each tile's realized surface into one continuous map.
- **The hex graph's job is addressing + wrap, not stitching.** A flat interior
  patch needs no graph. `topology.rs` (the spherical flat-neighbour graph, with
  `celestial_dir`) is **parked**; it plugs in for **wrap-around and streaming**,
  where the still-open "per-hex data layout (square 2048² vs 6-strips)" decision
  lives.
- **The sun sweeps in world coordinates** so the temperature field joins
  seamlessly across tiles (globally normalized heatmap, no seam).
- **Not yet joined: the dynamics.** Tiles currently sim **independently** —
  terrain and the world-positioned heat field join, but clouds/humidity/water do
  **not** flow across hex seams. That is the next structural step (§8.1).

---

## 8. Where to expand (prioritized)

### 8.1 Cross-hex halo exchange — *highest structural value*
Make weather flow across the joined map: each tile exchanges its edge cells with
graph neighbours each tick (domain-decomposition halo). In the flat patch the
neighbour cell is the spatially-coincident one; with the real graph it routes
through `topology.rs` edge-refs. Until this lands, "joined" is true for terrain
but not for clouds/water. **Do this before adding more layers.**

### 8.2 Biome feedback loops — *closes the climate loop*
Biome is a terminal read-out today. Feed it back: biome → **albedo** → into the
temperature cascade (forests warm, deserts/ice cool); vegetation →
**evapotranspiration** → humidity. Turns a one-way classification into a real
climate system.

### 8.3 Surface more of what's computed
- **Wind** is already computed (`wind_x/wind_z`), just not drawn — surface it as
  a speed heatmap (trivial) or arrows (lines pipeline).
- **Pressure** becomes its own layer once it has independent dynamics (today it's
  just `−temperature`); pair with wind.
- **Soil moisture / saturation** — the first real *trait field* (a ground
  property feeding runoff + biome); the bridge toward the spec's material ledger.

### 8.4 Richer terrain (POC-local, throwaway)
`world_height` is one ridged Navier-Stokes field. Compose landforms locally
(continent mask × ridged mountains × FBM), feature stamps (volcano cones, rifts,
plateaus), and droplet pre-erosion — kept in the demo, not the shared primitive.

### 8.5 The bake bridge — `impl Primitive for LayerStack`
The thin adapter that connects this data to the real per-cluster pipeline:
`is_solid(x,y,z) = y < realized_height(x,z)` (monotonic today; run-length when
caves arrive), material from `realized()`. Hand to existing `contour()`; point
`examples/voxel-cluster`'s `world_at` at it. `realized()` already does the hard
part. (Still **macro→cluster**; per-cluster octave refinement is separate.)

### 8.6 Graduate the nucleus to a crate
`layers.rs` is pure data + functions (no scene deps). It is the seed of the
spec's §13.4 **`world-state`** crate (`FieldStack` + passes + conservation +
`materialize`). Promote when the model settles; keep the demo thin.

### 8.7 The deferred-but-coming systems
- **Sediment / salinity / composition** carried by the flows — the *next big
  topic* (content movement). Fields gain a composition vector; passes move it
  conserved. This is also where the spec's **Rivulet DAG** optimization replaces
  the naive runoff CA (O(1) transport along routed chains).
- **GM-lever layer** as the bottom forcing field (geothermal heat, lava/material
  injection) — the metered source of §3.
- **Real celestial/eclipse** wired into `insolation` (§4).
- **Air-as-material run-length columns** for caves/voids (§6).

---

## 9. What's built (the POC)

- **`examples/hex-world/src/layers.rs`** — the reusable nucleus. `LayerStack`
  (substance + influence + climate fields), `generate`, `tick` (the pass
  pipeline), `realized`, `biome_material`, `build_sheet` (one mesh builder for
  both relief and heatmaps), `pack_ramp`, `minmax`, `terciles`. **8 tests**:
  conservation, finite/bounded, sun-moves, thermosphere swing, ozone↓surface,
  biome variety, realized validity, thresholds.
- **`examples/hex-world/src/main.rs`** — viewer only. **E** toggles
  Exploded (one hex, full stack) ↔ World (joined rings); **Space** pauses the
  sim; camera auto-orbits (A/D, W/S, Esc).
- **`crates/flicker-render/src/shaders/mesh.wgsl`** — demo palette extended to
  index 23 (the only change outside the example).
- **Parked, on disk, un-`mod`-ed:** `topology.rs`, `planet_mesh.rs` (the prior
  flat-graph traversal POC) — revive for the wrap/streaming step (§8.1).

---

## 10. Decided vs deferred

**Decided (load-bearing):**
- Three render classes (substance/mesh, influence/heatmap, control/forcing).
- Vertical column with bottom-up causation; water cycle in the top sub-layers.
- Water cycle is conserved transfers; `Σ(water+ice+humidity)` invariant; closed
  boundaries; GM lever is the one metered source.
- Atmosphere = top-down radiative cascade; residual flux couples to surface.
- Biomes classify off **climate** (integrated weather), by **terciles**.
- Water level is **contour-local** (`ground+water`), not a global sea.
- One field cell ≈ one cluster (macro/LOD8); per-cluster bake is the layer below.
- Substance-vs-meaning is the scope seam; veins are field thresholds, not objects.
- Terrain continuity is free from the heightmap; the graph is for wrap/addressing.

**Deferred (do not invent; see §8):**
- Cross-hex dynamics (halo exchange) — terrain joins, weather doesn't yet.
- Biome feedback (albedo/evapotranspiration); independent pressure/wind viz.
- The `Primitive` bake bridge and per-cluster octave refinement.
- Sediment/composition + the Rivulet DAG optimization.
- GM-lever forcing layer; real celestial/eclipse wiring.
- Air-as-material run-length columns (caves); the spherical-graph data layout.
- Crate promotion to `world-state`.
