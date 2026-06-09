# Handoff — Material Model: Implementation Progress

> Companion to `docs/material-model-handoff.md` (the design). This captures what
> has been **built** so far and the decisions made along the way, so a fresh
> context can resume without re-deriving them. Re-verify code anchors — names
> drift.

---

## 1. What's built — the crate stack

Four crates now realize data-model tiers ①–② plus world-gen (design handoff §1):

- **`flicker-materials`** — tier ① **vocabulary**. Loads `data/materials/*.json`
  behind a `TableSource` seam (`JsonTableSource` today; flicker-net → web → DB
  later, same trait). `Tables` indexes elements (by symbol / atomic number) and
  materials (by id / name); `blend_traits` and `blend_traits_by_number` are the
  composition-weighted element-trait blend. `ElementId` / `MaterialId` aliases
  keep the two `u8` namespaces apart. The classifier (composition → one of 256
  materials) is **deliberately deferred**.
- **`flicker-worldstate`** — tier ② **ledger**. `Composition` = element →
  absolute mass, **conservation-safe by construction** (only `add` / clamped
  `remove` / `add_composition`; no setter that could create or destroy mass).
  `Cell { composition, bulk_composition, surface_material: Option<MaterialId>,
  effects }`. `Ledger` = sparse `HashMap<CellCoord, Cell>`, materialize-on-touch.
  Persistence format left open.
- **`flicker-worldgen`** — the **epoch pipeline**. `Epoch1` seeds a per-hex
  composition = abundance × density-driven latitude bias × correlated 3D
  value-noise, normalized to a target mass. `pipeline`: an `EpochTransform` trait
  and `six_epoch_stack`, which runs Epoch 1 then Epochs 2-6 as `PassThrough`
  copies (real transforms slot in later). Pure / decoupled from topology — takes
  a unit-sphere direction per hex.
- **`hex-world`** (example) — now the **stack visualization** (§3 below).

All four are tested + clippy-clean.

---

## 2. Decisions made this thread (the non-obvious ones)

- **Real scale.** 1 world unit = 1 cluster = 128 ft → a hex is 2048 units
  (≈49.6 mi) across, the column 256 units (≈6.2 mi) tall. `layers::HEX_SIZE`
  is now `1024`. (f32 stays comfortable at this world size.)
- **Colored materials, NOT textures / UVs.** The art team isn't onboarded for
  textures yet, so we stay on the existing material-palette mesh path — per-draw
  `tint` over a near-white base gives arbitrary per-hex colour with no new
  textured-3D pipeline.
- **This view is a DATA VISUALIZATION, not a gameplay view.** No LOD, no sparse
  materialization, no generate-on-approach — those are runtime/gameplay concerns.
  We draw the world data at its native resolution.
- **Two resolutions, kept apart.** The **epochs operate at hex level** — one
  composition per hex (the macro "what is this region mostly made of"). The
  **per-cell detail** (2048² ≈ 4.2M cells per hex) is a *separate* cluster-column
  **materialization** pass, derived from the hex-level epochs + neighbours +
  noise. **Not built yet** — today's tiles are one colour per hex.
- **Epochs 2-6 are copies for now.** Each `EpochTransform` reads the layer below;
  until each epoch's real geology is written it passes the layer through
  unchanged. The planes diverge the moment a `PassThrough` is replaced.
  **Ground = Epoch 6.** Epoch-layers 7 (underground) / 8 (GM) are skipped.
- **Water cycle: kept, undrawn.** The `LayerStack` sim mechanics (heat,
  convection, evaporation, precipitation) are correct and **still tick**; only
  the viscous-effect *rendering* (water / lava / ice as heightmap meshes) was
  removed. Per the design, water / ice / lava are **effects** (Rivulets,
  deferred), not stacked heightmap layers.

---

## 3. `hex-world` — the stack viz (current state)

Each hex is a vertical stack at real scale, bottom → top:

- **6 epoch planes** — flat hexes from `six_epoch_stack`, each tinted by that
  epoch layer's **dominant element** (golden-ratio hue by atomic number; the
  table has no colours). Identical today (copies).
- **9 surface-sim bands** — the existing band model as **colored empty
  translucent shells** (`BAND_MAT` zone colours) at their real altitudes
  `0..256`. The sim ticks underneath; nothing of it is drawn.

Fly camera (WASD / R-F / RMB-look, Esc). Tunable consts: `VEXAG` (vertical
exaggeration — `1.0` = true scale, where the 256-tall column reads as a thin slab
under the 2048-wide hex; bump it to read the bands), `CAM_HOME` / `MOVE_SPEED`,
`BAND_ALPHA`, `SLAB` (epoch spacing). Gutted: the flat biome view, per-hex
inspector, Lua HUD, Local view, graticule, billboards. **Orphaned files**:
`scripts/hex_ui.lua` and `ui_elements.json` (no longer referenced — left in place
pending a cleanup decision).

---

## 4. Next

1. **Viz tuning** — pending a flythrough: `VEXAG` (the real column is thin),
   camera framing, band/epoch spacing.
2. **Cell materialization** — the per-hex → per-cell step: interpolate a hex's
   composition with its neighbours + per-cell noise → a composition/material per
   cell, so tiles resolve from one flat colour toward their 2048-cell detail.
   This is where the ~4M-cells-per-hex data volume first appears.
3. **Real epoch transforms** — replace the `PassThrough`s one at a time (Epoch 2
   differentiation by density, 3 tectonics, 4 hydrosphere, 5 mineralization /
   ore veins, 6 erosion / biomes — world-gen spec). Each grows the per-hex layer
   type beyond a bare composition (density profile, plate id, …).
4. **Design handoff §8.6** — re-home the water cycle onto the ledger + pass-based
   stepping, then build **Rivulets**. The sim is kept and ready; the tier-②
   substrate it should run on now exists.

---

## 5. Status (design handoff §8)

- 3. `TableSource` loader — **done** (`flicker-materials`).
- 4. Aggregate ledger schema — **done** (`flicker-worldstate`).
- 5. Epoch 1 — **done** (`flicker-worldgen`; generated *and* rendered in the
  `hex-world` stack viz).
- 6. Re-home water cycle + Rivulets — **pending** (the big remaining one).
