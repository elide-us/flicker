# Handoff — Epoch data audit: what world-gen records vs the spec

> A deep read of `crates/flicker-worldgen` answering "what data does the epoch
> sim actually record, and does it align with the design intent?" — plus the
> refinement slices that close the gaps. Companion to
> `docs/clayengine_world_generation_spec_v2.md` (the canonical epoch design) and
> `docs/flicker-world-handoff.md` (the viewer that renders these layers).
> Re-verify code anchors by name (line numbers drift).

---

## 1. Procedural or simulated? — both, in distinct layers

The honest answer splits cleanly, and the split matters for where this goes:

- **World-gen (the six epochs) is procedural generation, not a time-stepped
  simulation.** `six_epoch_stack` (`pipeline.rs`) runs Epoch 1 (seed) → 2 → … → 6
  as **one-shot pure transforms** of `(seed, params, dirs, neighbors)`. The only
  iteration is *local* (Epoch 6's erosion loop, Epoch 5's vein tracing). There is
  no planet-level tick with memory.
- **The water cycle (`examples/hex-world/src/layers.rs`) is a genuine stateful,
  conserved simulation** — ticked, `Σ(water+ice+band_moisture)` invariant to
  <0.1% over 300 ticks. That is the runtime "layer 9" prototype, a different beast
  in a different crate. See `docs/water-cycle-handoff.md`.

So "purely procedural?" → the **formation** epochs are; the **runtime** water/
atmosphere cycle is a real sim. **They are not yet connected** (§4).

## 2. The three data tiers

| Tier | What | Where | Persisted? |
|---|---|---|---|
| **1. Macro state (authoritative)** | per-hex `HexState` + per-epoch snapshots; cross-hex `Plate` records | `state.rs`; held in `flicker-world` `WorldData.layers` / `.plates` | **In RAM only** — regenerated on reseed/knob. No disk blob yet (the spec's Oracle artifact / `world-state` crate is deferred). |
| **2. Sub-hex spatial fields (re-derived)** | hardness / relief / vein filaments per cluster-cell (`CellSample`) | `field.rs` `FieldSampler` — recomputed from a `HexState` + world position at render/materialize time | Never stored. Deterministic. |
| **3. Cluster-column textures (micro)** | the 2048² heightmap stacks | spec §"Cluster-column materialization" | **Not built.** Tier 2 is its precursor. |

This matches the `voxel-data-layering` memory: throwaway input → source-of-truth
state → ephemeral recycled cache.

## 3. What each epoch records — and the gap vs spec

`HexState` (`state.rs`) is the recorded macro vector; each epoch accumulates onto
it. **✅ recorded · ⚠ Phase-1 simplification · ❌ spec asks, not recorded.**

| Epoch | Records (`HexState` / cross-hex) | Spec gaps |
|---|---|---|
| 1 Composition | ✅ `composition` | per-hex `seed` ❌ (one world seed today) |
| 2 Differentiation | ✅ `crust`, `crust_fraction`, `volcanic` | ⚠ `density_profile` is a 2-bucket crust/bulk split, not depth-keyed |
| 3 Tectonics | ✅ `plate`, `continental`, `boundary`, `elevation`, `orogeny`, **`plate_age`**, cross-hex **`Plate{id,continental,motion,members}`** | (closed by this audit's slice 1 — see §5) |
| 4 Hydrosphere | ✅ `sea_level`, `water_depth`, `temperature`, **`atmosphere`** (outgassed volatiles + local vapor), **`precipitation`** | (closed by slice 2 — §5) |
| 5 Mineralization | ✅ `hydrothermal`, `vein_element`, `vein_strength` (+deposits metal into crust) | cross-hex `Veins{path,depth_profile,concentration_profile}` ❌ (per-hex membership only — deliberate) |
| 6 Erosion | ✅ `flow`, `sediment`, `watershed`, `biome`, cross-hex **`Watershed` basins** (+refines `elevation`/`water_depth`) | `surface_material_signature` ⚠ (implied by crust/composition) |

## 4. The big alignment finding — the 6↔9 reconciliation

The nine-layer gameplay world (atmospheric layers driving the water-cycle erosion
sim **and** clouds / hydrosphere / energy / lightning) is the **runtime** model —
the hex-world 9-band stack (3 zones × 3 sub-bands: below/terrain/atmosphere),
prototype of runtime layers 7/8/9 (GM/underground/surface; memory
`epoch-layer-reconciliation`). The six epochs are **formation**; they should hand
that sim its initial condition.

**The runtime sim is currently stranded:**
- It lives in throwaway `examples/hex-world` on the **abandoned flat two-disc
  topology** (`topology.rs`) that the hex-sphere pivot superseded.
- It is **not seeded from `HexState`** and does **not** run on the icosphere
  (`flicker-worldgrid`).
- The seam where they meet is **Epoch 4**, which produces no `atmosphere_composition`
  and no precipitation — exactly the inputs the atmosphere/water bands need.
  Lightning/energy isn't built anywhere yet.

So "align with the epoch evolution intent" has a concrete reading: the formation
epochs must hand a complete atmosphere/hydrosphere initial state to a water-cycle
sim on the **same grid**, and that handoff does not exist yet.

## 5. Slice ladder

- **Slice 1 — Epoch 3 to spec: cross-hex `Plate`s + `plate_age`. ✅** Epoch 3 already
  computed per-plate drift vectors, used them for boundary classification, then
  **threw them away**. Now `Epoch3::partition(ctx) -> Partition { plate, plates }`
  is a public method (the deterministic Voronoi partition `apply` runs internally),
  returning the spec's `plates` structure: `Plate { id, continental, motion,
  members }`. Per-hex **`plate_age`** (Myr) is written from a multi-source BFS
  (`ridge_distance`): ~0 at oceanic divergent ridges, ageing toward the subducting
  margins (`max_age`, default 200), continental crust floored old (cratons,
  ≥0.6·`max_age`). `flicker-world` `WorldData.plates` records it; the HUD shows the
  plate count. Tests: `partition_records_drifting_plates_covering_every_hex`,
  `plate_age_is_young_at_ridges_and_old_on_continents` (worldgen 38 unit + 1
  integration green; flicker-world 13 green). **Deferred (adjacent):** a viewer
  plate-motion / age overlay, exposing `max_age` as a HUD knob, and Epoch 5 tracing
  veins along `Plate.motion` faults rather than the hydro field alone.

- **Slice 2 — Epoch 4 → water-cycle seam. ✅** Epoch 4 now records the formation
  initial-condition the runtime water/atmosphere bands need: per-hex
  **`atmosphere: Composition`** = volcanic outgassing of the **scarce volatiles**
  (`VOLATILES` = H, C, N, S, Cl — deliberately *not* the lattice-bound O/Si, so the
  air reads as a young CO₂/water-vapor atmosphere, not crustal oxygen), integrated
  globally + normalized to `atmosphere_mass`, plus a local water-vapor (H) loading;
  and **`precipitation: f32`** (0..1) = warmth × ocean-proximity (diffused inland
  `moisture_spread` passes). New knob `e4_vapor_scale` ("Rain / humidity") and a
  **`ViewMode::Precipitation`** heatmap (arid tan → humid blue). Tests:
  `outgassing_builds_a_volatile_atmosphere_not_bound_oxygen` (O/Si stay out of the
  air, H vapor present), `precipitation_is_wetter_in_the_warm_wet_tropics`; pentagon
  integration test extended to assert `precipitation`/`atmosphere` finite at the
  defect. worldgen 40 unit + 1 integration green; flicker-world 13 green; clippy
  clean. **Done since:** Epoch 6 now reads this `precipitation` for biomes/flora
  instead of recomputing moisture (E4 split into warm-coupled `humidity` for
  atmosphere vapor vs proximity-driven `precipitation`; proximity made
  coastline-inclusive). **Still deferred:** an atmosphere gas-mix view; CO₂/SO₂
  (C/S) outgassing needs compound awareness to be more than an element proxy.

- **Later — re-home the water cycle.** Promote `layers.rs` to a `world-state`
  crate, seed it from `HexState` on the icosphere (the Epoch 4 atmosphere +
  precipitation are now its initial condition), retire the flat topology. Then the
  nine-layer / clouds / lightning vision is wired to the planet. Multi-slice.

- **Other recorded-data gaps (pick as needed):** cross-hex `Veins` (Epoch 5 still
  per-hex only); per-hex materialization `seed`; depth-keyed `density_profile`;
  `surface_material_signature` as a distinct field; persisting the macro state to a
  disk blob (the deferred `world-state` storage). (Cross-hex `Watersheds` ✅ done.)

## 6. Verify

`cargo test -p flicker-worldgen` (38 unit + 1 integration), `cargo test -p
flicker-world` (13), `cargo clippy -p flicker-worldgen -p flicker-world` clean.
Visual confirmation (the planet renders, the HUD reads the plate count) is the
user's per `user-verifies-app-themselves`.
