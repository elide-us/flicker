# Handoff — the layered field model & water cycle → going vertical

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

## ⇒ Actionable handoff — the water cycle goes vertical

Since the model below (§0–6) was written, the example grew the **full flat hex
graph**, **reliable neighbour-finding**, a **fly-camera world**, a **per-hex
inspector**, and a corrected **vertical-scale model**. **The vertical arc is now
complete: Step 1 (inspector), Step 2 (9-band vertical model + terrain
normalization), and Step 3 (the conserved vertical water cycle / conveyor) are
all done** — see the per-step blocks below for what landed and what was deferred.
The remaining arc is **horizontal**: cross-hex halo + sediment (see *NEXT*). §0–6
remain the load-bearing foundation; §7–9's *viewer* specifics are superseded —
it's one fly-camera world (WASD/RF, RMB look) with **left-click to split a hex
out**.

### Current state (locked foundation, on `main`)
- **Flat hex graph** (`topology.rs`, no longer parked): two **flat-top** discs
  (north/south poles at their centres), joined at the equator with a half-hex
  interlock. Flat-top = triangle points face E/W (the 2048-cluster wide axis).
- **Reliable edge-neighbours** (`HexMap::neighbours`): cube-coordinate adjacency
  (exact for the hexagon-of-hexagons) + a symmetric equator fold — **0 asymmetry
  / 0 dups / 0 self-refs at any size** (`neighbours_are_symmetric_clean_at_every_size`).
  Replaced the old proportional `edge_refs`, which had dozens of asymmetric edges.
  This is the backbone a global sweep iterates over.
- **Inspector (Step 1, done):** left-click a hex → a translucent **hexagonal
  prism** splits out *in place* over the tile — per-band coloured glass walls +
  edge lines, every layer sheet floating inside (`build_inspect`, `hex_walls`,
  `column_lines`, `band_boundaries`, `PY_*`). The selected hex ticks live; click
  again to close. **Step 2 renders its new bands inside this column.**
- Plus the water cycle (§3), radiative cascade (§4), climate→biomes (§5), and the
  render-class taxonomy (§1) — all still as described below.
- **Scale:** `VSCALE = 0.15` is display-only and deliberately flat (planet
  scale); the sim runs on **normalized** altitude, so a flat display changes no
  physics. Hexes are full size; only the index billboards were shrunk.

### The vertical-scale model (the corrected, load-bearing context)
The atmosphere is **real stacked cluster layers**, not theoretical space. With
`CLUSTER_DIM = 256` and a voxel = **0.5 ft**:
- A **cluster = a 128 ft cube** (256 × 0.5 ft).
- Hex = **2048 cluster-columns E–W** (point-to-point) = 2048 × 128 ft =
  262,144 ft ≈ **49.6 mi** wide (N/S flat-to-flat ≈ 43 mi; area ≈ 1,600 sq mi —
  a Rhode-Island-sized drainage basin). *A cluster-column is 128 ft; an earlier
  "1024 ft / 8 clusters wide" here conflated voxels with cluster-columns.*
- The world is **256 stacked cluster layers = 32,768 ft ≈ 6.2 miles** vertical
  (256 clusters × 256 voxels × 0.5 ft).
- **The heightmap's 256 grey levels index the surface's cluster layer** — one
  grey step = one cluster layer = **128 ft** (not 256 voxels inside *one* cluster,
  which is the simplification the demo currently renders).

A column is **ground = layers `0..H`**, **air = layers `H..255`** (H = the
heightmap value); the air column has `256 − H` layers and **varies per column**.
That makes the physics literal:
- **Pressure** integrates down the real 128-ft layer count; a mountain (higher H)
  has a shorter, lower-pressure column — walking up is one 128-ft layer at a time.
- **Orographic rain**: higher H → fewer air layers → condensation reached lower →
  windward wet. **Rain-shadow deserts**: the ozone-warmed band aloft caps
  convection, so inland cloud never rains. A few layers of relief tip thresholds.

### The conveyor belt (what Steps 2–3 build)
A hex is a **vertical slice**; the sim's job is the whole slice:
1. Solar heat enters per-layer, top-down (the radiative cascade = the inter-band
   *filters*; ozone is a real higher layer range).
2. Surface evaporates moisture into the lowest air layer.
3. Convection lifts it band→band (buoyancy; rate from the temp/pressure gradient).
4. It condenses where the column crosses threshold → rain/snow back down.
5. Column-integrated pressure ties it together, driving convection **and** wind.

### STEP 2 — Vertical band stack (✅ DONE, on `macbook`)
**Defined and aligned the vertical data only — no new water movement** (the
deliberate leash; physics is Step 3). What landed in `layers.rs` + the inspector:
- **The 256-layer y-spine is real, not assumed.** The `ClusterId` y-field is
  **8 bits** (`crates/flicker-voxel/src/cluster_id.rs`, layout LOCKED
  `[LOD:4][x:10][y:8][z:10]`), so the local box is 1024×**256**×1024 clusters =
  24.8×**6.2**×24.8 mi. Below layer 0 = the molten floor; above 256 = the
  celestial engine's domain.
- **9 fixed bands = 3 zones × 3 sub-bands** (`BANDS`, `BAND_BOUNDS` tiling
  `0..Y_LAYERS` in equal thirds): **below** 0..3 (molten GM floor / veins / caves),
  **terrain** 3..6 (lowland / hill / alpine), **atmosphere** 6..9 (lower trop /
  cloud deck / thin air). Each band carries `{ band_temp, band_moisture,
  band_pressure }` (SoA, `G*G*BANDS`).
- **Bands are global ranges; a band's *role* is per-column** (`band_role`):
  above the surface layer = air, the one containing it = surface, below =
  underground. Same band is air for a lowland column, underground for a mountain
  one — the orographic mechanism is now structural, not tuned.
- **Terrain normalized into the strict middle third** (`surface_layer`): the
  global `ground` window `[64,192]` (= `world_height` 128±32 × `RELIEF_GAIN`)
  maps into layers `[85,170)`, guaranteeing underground room below + atmosphere
  above *every* column.
- **The profile is a pure derive** (`refresh_bands`, called from `generate`/`tick`):
  geothermal warming below, lapse-cool + ozone inversion + thermosphere swing
  above; baseline exponential pressure; `band_moisture` = `humidity` re-expressed
  by altitude (so it **never enters `total_water`** — conservation untouched,
  the test still passes at <0.1%).
- **Inspector rebuilt** (`build_inspect`): the split-out column is now the real
  256-layer y-axis — 9 zone-coloured translucent bands, surface relief seated at
  its true normalized layer, cloud deck in the air bands, the per-band temp
  profile + air-moisture as heatmaps at altitude. (`PY_*`/`EXPLODE_BASE` exploded
  stack retired; `layer_py` + `BAND_BOUNDS` drive it now.) +4 tests (26 total
  was 18→22 in `-p hex-world`): band tiling, terrain normalization, role
  partition, profile finiteness/pressure-falls/moisture-balances.
- *Decisions locked this round:* N=9 (3×3); terrain hard-confined to the middle
  third; profiles **derived** this round (authoritative + transporting in Step 3).

### STEP 3 — Vertical water cycle / the conveyor (✅ DONE, on `macbook`)
**The actual movement of water** — `band_moisture` is now the conserved air pool
and the cycle runs vertically as the sediment conveyor (lift → drop → feed
runoff). What landed in `layers.rs`:
- **`humidity` retired; `band_moisture` is authoritative + conserved.**
  `total_water` = `Σ(water + ice + Σ_b band_moisture)`; the conservation test
  passes end-to-end at <0.1% drift over 300 ticks.
- **The tick pipeline is the conveyor:** `update_thermal → refresh_band_temp →
  update_pressure → update_wind → evaporate → advect_bands → convect →
  condense_precipitate → runoff → melt_ice → update_climate`.
- **`evaporate`** moves surface water into the lowest air band above the surface.
- **`advect_bands`** is per-band upwind advection, **closed against any face
  where the band isn't air for the neighbour** (moisture never drifts into rock —
  the rough orographic concentrator against terrain rises).
- **`convect`** is the lift: buoyant (warm-below) air carries moisture band→band,
  rate ∝ the temp drop to the band above (`CONVECT_RATE`/`CONVECT_T`).
- **`condense_precipitate`** is the drop: per-band capacity falls with band temp
  (cold/high → low capacity), excess rains out to the surface (water if warm, ice
  if freezing); `cloud` = peak air-band saturation.
- **`update_pressure`** is real column-integrated pressure: each band's pressure
  = air mass (baseline density thinning with altitude + moisture) **strictly
  above** it; surface pressure = the whole air column above the surface, so a
  mountain reads lower pressure. Drives `update_wind` (the old `−temperature`
  placeholder is gone).
- **Tests:** +1 (`the_conveyor_runs_and_conserves`: air moisture is perturbed by
  the cycle *and* total water is conserved); `band_profile` updated for the real
  pressure curve; `the_sim_stays_finite` checks per-band moisture. 23 total.
- The Step-2 inspector now visualizes the **live** cycle for free (it already
  read `band_moisture`/`band_temp`/`band_pressure`).

*Deferred from Step 3 (didn't need them to make the conveyor run):* true
orographic/rain-shadow precision (current version concentrates moisture against
terrain via the advect guard, doesn't rain it on the windward slope per se);
folding the `A_THERMO`/`A_UV` cascade absorption *into* the bands (still computed
in `update_thermal` and blended into `band_temp`); an in-flight cloud pool
(precip lands same tick).

### NEXT — the horizontal conveyor + sediment (where water earns its purpose)
Water exists to move **sediment** to the ocean (memory: *water = sediment
conveyor*). The vertical lift/drop now works per-hex; the remaining arc:
- **Cross-hex halo exchange (§8.1)** — the rolling array sweep with async edge
  reads (see *execution model* above) so moisture/water/clouds cross hex seams
  and rivers run between hexes. This is the horizontal half of the conveyor.
- **Sediment/composition carried by the flows (§8.7)** — fields gain a
  composition vector; `runoff` (and convection/precip) move it conserved. This is
  the actual payload the whole cycle was built to transport.

### Keep in line with these three (the reminder)
One coherent arc — build each so the others still hold:
1. **The inspector (Step 1) keeps working.** Step 2's bands render *inside* the
   split-out translucent column; don't bypass it. It is the instrument that makes
   the vertical model verifiable — you can't see a conveyor belt from the
   top-down map.
2. **The vertical-scale model is the substrate.** 256 cluster layers × 128 ft;
   air = the real layers above `H`; pressure counted per 128-ft step. Steps 2–3
   build *on* this, not on abstract band counts.
3. **Conservation never drifts** (§3; spec §4). Every transfer — evaporation,
   convection, condensation, precipitation, melt — moves mass between pools and
   never creates/destroys it. Keep the render-class split (§1): substance = mesh,
   influence/bands = heatmap.

### Parallel track (not in these two steps, don't forget it)
Steps 2–3 are **per-hex (vertical)**. The **horizontal** coupling — clouds/water
flowing *across* hexes — is **halo exchange** over the now-reliable
`HexMap::neighbours` (§8.1). Tiles still sim in isolation; wire it once the
vertical model is in, to make the cycle global (rivers cross hexes, weather
drifts). Independent of the vertical work, but needed for a coherent world.

### The execution model — the rolling batch sweep (the frame Step 3 lives in)
The per-hex `tick` is **one step of a serial sweep over the planetary array**:
start at index 0 (north pole), process that hex's layers + update its sim, walk
down the array to the last hex (south pole), then start again — a continuous
rolling pass. This is the **erosion pass** (world-system spec §5): the *same*
sweep that evolves the field sim is the one that will **read in player-changed
voxel data and aggregate it back into this simulation** (the degeneration /
write-back). The field/water tick is the *easy* ride-along; the expensive batch
work is the per-cluster bake generation + change aggregation, which is **why**
the load is spread one-hex-at-a-time rather than ticking the whole planet at once.
- **Neighbour reads are asynchronous** — heat/pressure (and later halo moisture)
  read whatever value the neighbour is *currently* at; no barrier, no
  double-buffer. The sweep tolerates a one-pass-stale edge (it catches up next
  lap). This is consistent with the "three decoupled clocks" (spec §7) — render
  LOD, cache warmth, and the erosion sweep never have to agree.
- **Scale reminder:** a hex is ~49.6 mi across (≈1,600 sq mi), so each sweep step
  advances a Rhode-Island-sized drainage basin; an Earth-sized planet is ~95k
  hexes (R≈125), so a full lap is ~95k tick-steps spread over wall-clock time.

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
- **The hex graph's job is addressing + adjacency, not stitching the flat
  terrain.** `topology.rs` is now **revived and load-bearing** — it lays out the
  full two-disc flat graph and provides reliable `neighbours` (the backbone for
  halo exchange, §8.1). Terrain continuity itself still comes free from the
  heightmap; the graph carries adjacency (incl. the equator fold) and
  `celestial_dir`. Wrap-around/streaming and the still-open "per-hex data layout
  (square 2048² vs 6-strips)" decision remain future work.
- **The sun sweeps in world coordinates** so the temperature field joins
  seamlessly across tiles (globally normalized heatmap, no seam).
- **Not yet joined: the dynamics.** Tiles currently sim **independently** —
  terrain and the world-positioned heat field join, but clouds/humidity/water do
  **not** flow across hex seams. That is the next structural step (§8.1).

---

## 8. Where to expand (prioritized)

### 8.1 Cross-hex halo exchange — *the horizontal parallel track*
Make weather flow across the joined map: each tile exchanges its edge cells with
graph neighbours each tick (domain-decomposition halo), routing through the now
**reliable `HexMap::neighbours`** (the old proportional `edge_refs` is gone).
Until this lands, "joined" is true for terrain but not for clouds/water.
**Sequencing revised:** the *vertical* atmosphere arc (Steps 2–3 in the handoff
block at the top) was prioritized first; halo exchange is the **horizontal**
parallel track — wire it once the vertical model is in.

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
- **`examples/hex-world/src/main.rs`** — viewer only. Now the **full flat hex
  graph** as one fly-camera world (WASD/RF, RMB look, Esc) with a graticule
  overlay + index billboards, and **left-click → split a hex out** in place (the
  inspector). *(The old "E toggle Exploded↔World" is gone.)* Helpers: `hex_flat_pos`/
  `ring_offset` (layout), `build_inspect`/`hex_walls`/`column_lines`/`pick_hex`
  (inspector), `build_graticule`.
- **`examples/hex-world/src/topology.rs`** — **revived and the load-bearing graph**:
  `HexMap::neighbours` (reliable cube-coordinate adjacency + symmetric equator
  fold), `cube`, `spiral_to_cube`. Its `#[cfg(test)]` proves symmetry at any size.
- **`crates/flicker-render/src/shaders/mesh.wgsl`** — demo palette extended to
  index 23 (the only change outside the example).
- **Tests:** `cargo test -p hex-world` → **18** (sim conservation/biome/atmosphere
  in `layers.rs`; neighbour symmetry + layout tessellation in `topology.rs`/`main.rs`).

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
- **Flat-top** hex orientation (points E/W); two-disc spiral layout, equator interlock.
- **Reliable** cube-coordinate `neighbours` (symmetric at any size); proportional
  `edge_refs` is gone.
- **Vertical-scale model:** 256 stacked cluster layers × 128 ft (a cluster is a
  128 ft cube); air = the real layers above the surface (`H`). The atmosphere
  sim is built on this, not abstract band counts.
- The **inspector** (in-place split-out translucent hex prism) is the verification
  instrument for the vertical model; new layers/bands render inside it.

**Deferred (do not invent; see §8):**
- Cross-hex dynamics (halo exchange) — terrain joins, weather doesn't yet.
- Biome feedback (albedo/evapotranspiration); independent pressure/wind viz.
- The `Primitive` bake bridge and per-cluster octave refinement.
- Sediment/composition + the Rivulet DAG optimization.
- GM-lever forcing layer; real celestial/eclipse wiring.
- Air-as-material run-length columns (caves); the spherical-graph data layout.
- Crate promotion to `world-state`.
