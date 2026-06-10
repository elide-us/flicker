# Handoff — Material Model: Implementation Progress

> Companion to `docs/material-model-handoff.md` (the design) and
> `docs/clayengine_world_generation_spec_v2.md` (the epoch spec). Captures what is
> **built**, the decisions behind it, and where the next context picks up. Re-verify
> code anchors — names drift.
>
> **State at this boundary:** the world-gen pipeline runs **Epochs 1–4 with real
> formation physics** — the planet emerges from molten composition through a
> crystallized crust, plate tectonics + mountains, to oceans. Next context's focus:
> **Epochs 5–6, then re-home the water cycle onto this output** (the erosion sim's
> starting point).

---

## 1. The crate stack (all tested + clippy-clean)

- **`flicker-materials`** — tier ① **vocabulary**. `data/materials/*.json` behind a
  `TableSource` seam (`JsonTableSource` now; net/DB later). `Tables` indexes
  elements (symbol / atomic number) and materials (id / name);
  `blend_traits[_by_number]` = composition-weighted element-trait blend.
  `ElementId`/`MaterialId` aliases. **Classifier (composition → one of 256
  materials) deferred** — this is why hardness is element-blend-based, not
  rock-type-based (see §5).
- **`flicker-worldstate`** — tier ② **ledger**. `Composition` = element → absolute
  mass, conservation-safe (`add` / clamped `remove` / `add_composition`; no
  matter-creating setter). `Cell { composition, bulk_composition, surface_material,
  effects }`, `Ledger` = sparse `HashMap<CellCoord, Cell>`. *This is the substrate
  the re-homed water cycle should run on — but the epoch pipeline currently outputs
  `HexState`, not `Ledger` (see §4).*
- **`flicker-worldgen`** — the **epoch pipeline + fields** (the bulk of recent work):
  - `HexState` (`state.rs`) — the per-hex state threaded through the chain:
    `composition, crust, crust_fraction, volcanic, plate, continental, boundary,
    elevation, orogeny, sea_level, water_depth, temperature`. Each epoch adds/edits
    fields. `surface()` = crust once differentiated, else bulk composition.
  - `EpochTransform` + `EpochCtx` (`pipeline.rs`) — `apply(ctx, prev) -> Vec<HexState>`.
    `EpochCtx` carries `{ tables, dirs (unit-sphere per hex), neighbors, seed }`.
    `six_epoch_stack` seeds Epoch 1 then runs Epochs 2–4 (real) + 5–6 (`PassThrough`
    copies), keeping **every** layer for the stacked viz.
  - **Epoch 1** (`epoch1.rs`) — composition seed: `abundance × density latitude-bias
    (heavy→equator, volatile→pole) × correlated 3D noise`, normalized to a target
    mass. Abundance is an epoch *parameter* (Earth-crust default), not table data.
  - **Epoch 2** (`epoch2.rs`) — **differentiation**: elements ≤ `crust_density_max`
    rise to `crust`, heavier sink (bulk conserved). Thin/equatorial crust → volcanic.
  - **Epoch 3** (`epoch3.rs`) — **plate tectonics**: seed N plates, grow by
    multi-source BFS over `neighbors` (Voronoi), continental/oceanic type + drift,
    boundary classification from relative motion, `elevation` (continents high, ocean
    low, **mountains at convergent**, rifts at divergent). Sets `orogeny` = convergence
    strength at convergent hexes.
  - **Epoch 4** (`epoch4.rs`) — **hydrosphere**: bathtub-fill `sea_level` so
    `ocean_fraction` of the surface floods; per-hex `water_depth` + latitude/elevation
    `temperature`.
  - **`FieldSampler`** (`field.rs`) — **the per-cell fields** (the "hardness +
    composition matrix"). Turns a hex's aggregate into continuous sub-cell terrain:
    per-element NS sub-fields → per-cell composition → **hardness blend over the
    *solid* rock-formers** (gases excluded — they'd wash it to 0). Drives **relief**:
    hard rock ridges up, soft planes low. Layered formation effects: **convection**
    (iterated domain-warp — "swirling iron", kinematic), **crystallization** (ridged
    relief once `crust` is set, vs smooth molten swell), **orogeny** (folds + lifts at
    `orogeny > 0`). Sampled in continuous world coords → seamless across hexes.
- **`hex-world`** (example) — the **stack visualization** (§3).

---

## 2. The model (how to think about it)

- **Continuous spatial *fields*, not per-hex scalars.** The per-hex aggregate is the
  field's local average; its sub-cell NS structure **is** the terrain. "1 cell = 1
  cluster = 128 ft" is the field's *sampling* resolution.
- **Hardness is the spine.** Hardness = erosion resistance, derived from *which
  elements sit where*. Its spatial distribution is exactly what the **water cycle
  will erode** — soft carves into valleys/seabeds, hard resists into ridges. This is
  the bridge from world-gen to the erosion sim.
- **The formation arc, visible end to end:** molten swirls (convection) → crystallized
  crust (ridged) → plates & lifted mountains (tectonics + orogeny) → oceans
  (hydrosphere).
- Reference: design handoff §0 (material → moves sediment → *is* the water cycle's
  point); spec §3.2 ("Navier-Stokes plus many erosion cycles"). The fields lean on the
  `flicker-primitive/heightmap.rs` NS-wave idiom.

---

## 3. `hex-world` — the stack viz

Per hex, vertical stack at real scale (`EPOCH_GAP = 640` between the exploded epoch
planes; 9 sim bands at true `0..256` altitude on top):

- **6 epoch planes** — per-cell **relief meshes** sampled from `FieldSampler` (so you
  see swirls/ridges/coasts, not flat hexes), tinted by each epoch's dominant element.
  Epoch 1 molten · Epoch 2 crystalline · Epoch 3 mountains · **Epoch 4+ flood a flat
  sea** (`M_WATER_MID`) over submerged cells. Epochs 5–6 reuse Epoch 3 relief + Epoch
  4 water.
- **9 sim-band shells** — empty colored translucent prisms (`BAND_MAT`).

The kept `LayerStack` water-cycle sim **still ticks but is undrawn** (its viscous
heightmaps were removed; mechanics retained). Fly: WASD / R-F / RMB-look / Esc.
Knobs: `EPOCH_GAP`, `VEXAG`, `CAM_HOME`/`MOVE_SPEED`, and `FieldSampler::new`
(`composition_freq`, `relief_freq`, `relief_amp`, `tectonic_scale`, `convection_iters`,
`flow_*`, `orogeny_lift`, `fold_freq_mult`). `Epoch{2,3,4}::default` hold the geology
params. **Orphaned, unused:** `scripts/hex_ui.lua`, `ui_elements.json`.

---

## 4. Next context — the focus

Goal: reach the **water-cycle simulation starting point**.

1. **Epoch 5 — mineralization / ore veins.** Concentrate the trace metals (already
   present everywhere from Epoch 1, sunk by Epoch 2) into **veins/bands** along
   hydrothermal + fault paths (convergent boundaries, `volcanic`). Spec §Epoch 5.
   The user specifically wants "large veins of ore concentrated into bands."
2. **Epoch 6 — erosion / sedimentation / biomes.** Refine elevation by erosion across
   hex adjacency, set surface-material signature + biomes. (Note: this is *macro*
   erosion at hex scale — distinct from the runtime per-pass water cycle.)
3. **Re-home the water cycle** (design handoff §8.6) — the real target. The kept
   `LayerStack` physics (heat/convection/evaporate/condense/runoff — right physics,
   wrong execution model) gets re-homed onto **this pipeline's output + the hardness
   field**, pass-based, with **Rivulets** moving sediment (design handoff §5). The
   hardness field is what it erodes.

**Key seam to resolve early:** the epoch pipeline outputs `Vec<HexState>`; the runtime
ledger is `flicker-worldstate::Ledger` (`Cell`/`Composition`). Decide how `HexState` +
the per-cell fields land into the ledger the water cycle runs on (the design's
"materialization" / aggregate). This is the hinge between world-gen and the sim.

---

## 5. Open refinements (deferred, flagged — don't silently assume)

- **Formed-material classifier** (composition → granite/basalt/… → real rock hardness).
  Today hardness is an element blend (solids only). This is the biggest realism lever.
- **Dynamic convection** — current convection is kinematic (prescribed flow warp), not
  buoyancy-driven NS.
- **Linear fold belts** — orogeny is per-hex intensity; sub-hex suture-line ranges are
  finer (matters at higher ring counts).
- **Per-cell materialization** — the viz samples the field at `G=64` per hex; the full
  2048²-cluster materialization for the ledger is not built.
- **Water budget from composition** — `ocean_fraction` is a knob; tie it to H/O.
  Atmosphere-from-outgassing + precipitation are unbuilt Epoch-4 pieces.
- **World is R=3 (74 hexes)** test scale; features are chunky but emergent. Larger R
  resolves continents/ranges/veins finer.

---

## 6. Status (design handoff §8)

- 3 `TableSource` ✅ · 4 ledger schema ✅ · 5 Epoch 1 ✅ — **and well beyond**:
  Epochs 1–4 now run real formation physics with per-cell fields.
- 6 re-home water cycle + Rivulets — **pending; the next context's destination.**
