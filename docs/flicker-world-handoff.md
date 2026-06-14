# Handoff — `flicker-world` (the world viewer app)

**Status:** **W1–W5 + U1–U2b + E1–E3 built** — the icosahedral-planet viewer +
motion/rotation sim, the full app shell + settings workbench, and the **epoch-viz
pass** (E1 diffuse material map, E2 differentiation/Crust, E3 hypsometric relief),
each epoch auto-showing its own meaningful field. Next epoch passes (Epoch 5 has no
dedicated view yet); plus the deferred settings behaviours.

> **Continuing in a new session?** The epoch-viz work (§3b) is ongoing. The
> **canonical definition of what each epoch is *supposed* to do** lives in
> **`docs/clayengine_world_generation_spec_v2.md`** (§"Epoch specifications",
> Epoch 1-6, each with a Phase-1-simple / Phase-2-sophisticated plan). Read it
> before the next epoch pass; this handoff only tracks the *visualization*.
> Implementation of the epochs is in `crates/flicker-worldgen/src/epoch{1..6}.rs`.
**Name:** `flicker-world` is provisional (not final).
**Audience:** Claude Code (implementation), Elideus (review).
**Builds on:** `docs/hex-sphere-handoff.md` (the grid it renders),
`examples/hex-world` + `examples/hex-map` (the app architecture it consolidates).

---

## 1. What it is

A new **crate** `crates/flicker-world` (binary) — the application that
consolidates the app architecture from `hex-world` and `hex-map` onto the
icosahedral grid (`flicker-worldgrid`). It opens a window, generates a planet,
and renders the whole sphere as one mesh with each cell coloured by a world-gen
epoch field, under an orbit camera.

Run it:

```
cargo run -p flicker-world
```

Controls: **drag** rotate · **wheel** zoom · **V** cycle field (elevation / biome
/ plates / temperature / precipitation / prebiotic / composition / crust) · **↑/↓**
epoch · **R** reseed · **[** / **]** grid · **Esc** quit. The HUD panel has the
same controls plus a **per-epoch knob panel** (sliders that re-tune that phase and
regenerate on release), and on the **Composition epoch** an **element-mix grid** —
one slider per element of the starting balance, best viewed in the **composition**
field. **Reseed (R / button) is per-layer:** seeds are a per-epoch chain
(`world::seed_chain`); reseeding while viewing epoch *e* advances `seeds[e]`,
re-derives the seeds of the layers built on it, and re-rolls **only that epoch's**
knobs (`world::mutate_epoch_params`, central-biased, deterministic) — so the
upstream layers stay byte-identical (a locked-in Epoch 1 keeps its look) while that
phase gets a fresh variation. Build a "nice" world phase by phase. Grid frequency
preserved. Each epoch runs under its own seed via `generate_with_seeds`.

---

## 2. Architecture (the pieces brought in)

Scene-driven via `flicker::scene::SceneManager` (the engine's intended app shell):

| Module | Owns |
|---|---|
| `main.rs` | `run(SceneManager::new(Loading))` — the entry point. |
| `world.rs` | `WorldData` (grid + **all six epoch layers**), `WorldParams` + `PARAM_DEFS` (the tunable per-epoch knobs + defaults), and `generate(tables, params, freq, seed)` — runs the epoch chain with the tuned knobs through `EpochCtx`, **keeping every layer**. Pure given `(params, freq, seed)`. |
| `color.rs` | `ViewMode` + `cell_material()` — colours each cell of the *selected* epoch layer by field (reuses hex-world's `pack_ramp`; palette mirrors `flicker-render`'s `mesh.wgsl`). |
| `globe.rs` | `build(data, epoch, mode, radius)` — fan-triangulates the selected layer's cells to flat-shaded polygons; winds triangles outward (the mesh pipeline back-face culls). |
| `camera.rs` | `OrbitCam` — drag-rotate + wheel-zoom (rotation suppressed while the pointer is over the HUD). |
| `scene.rs` | `Loading` (splash → generate → replace) and `World` (input, epoch/field selection, knob harvest → regen-on-release, mesh rebuild, **Lua HUD**; Esc pushes Pause). |
| `shell.rs` | The app shell: the **`Logo`** splash (plays `assets/*.png` via `scripts/logo.lua`), the reusable flat **`Modal`** (`Menu` / `Pause`), and the **`Settings`** workbench scene (`scripts/settings.lua`) incl. the Key Mappings rebind (`RebindCapture`). Stack-flow ported from voxel-cluster, skinned flat. |
| `settings.rs` | Persisted app settings: `GameSettings` (flat value map → `settings.json`), the `SETTINGS` control defs (page/id/kind/default; mirrors `ui_elements.json` `settings.pages`), and the process-global `GAME_SETTINGS`. |
| `scripts/world_ui.lua` + `ui_elements.json` | The in-world control panel (`hud.*`) **and** the shell `modal`/`screens` sections. `world_ui.lua` draws the HUD; `modal.lua` draws the menu/pause/settings panels. |

**Reuse, not copy:** the heavy lifting (grid, epochs, materials, render/scene/app
stack) stays in its crates; `flicker-world` is the thin orchestration on top —
exactly the consolidation goal. The colouring convention (`pack_ramp`) and the
scene/camera/HUD patterns are the distilled lessons from hex-world/hex-map.

---

## 3. What's deliberately deferred (the "needs love" parts)

- **Lua UI (W2). ✅** A control panel built on the engine's UI pattern
  (`flicker-script` `ScriptHost` + `flicker-ui` `render_hud`/`load_widgets`/
  `load_ui_json` + the shared `Widgets` toolkit): field dropdown, grid-frequency
  stepper, reseed button, live stats. Authored for this app's controls rather
  than copying hex-world's mismatched panel; same system, same widgets. A
  headless test (`scene.rs`) loads the script against the real JSON + widgets and
  runs an update/draw cycle, so Lua breakage is caught without a window.

- **Epoch control panel (W3). ✅** All six epoch layers are kept; an **epoch
  stepper** picks which to render, and a **knob panel** exposes each phase's real
  `flicker-worldgen` parameters (Epoch 1 composition bias/contrast, Epoch 3
  plates/uplift/rift, Epoch 4 ocean/temps, Epoch 6 rain/erosion, …) as sliders
  that mutate `WorldParams` and **regenerate on release**. Knob ids live once in
  `world.rs` `PARAM_DEFS` (read by `generate`, published to the HUD) and mirror
  `ui_elements.json` for labels/ranges. A unit test proves a knob change changes
  the world; the headless HUD test renders the tectonics knobs.

- **Element-mix grid (W4). ✅** Epoch 1's *element abundances*
  (`Epoch1Params.abundance`) are tunable as a 2-column slider grid (`ABUNDANCE_DEFS`
  → `ab_<sym>` params), shown on the Composition epoch, with a **Composition view
  mode** (dominant-element tint via `element_index` in `color.rs`) so the balance
  is visible. A unit test proves boosting `ab_Fe` shifts the composition.

- **Hotspot volcanism (W5a). ✅ — the first new simulation.** Added to
  `flicker-worldgen` **Epoch 3** (`crates/flicker-worldgen/src/epoch3.rs`): fixed
  mantle hotspots; using the plate **motion vectors Epoch 3 already computes**,
  each plate drifting over a plume gets an uplift comet — a blob at the plume, a
  long tail downstream (`hotspot_trail`) → island/seamount chains. Three new
  params (`hotspots`, `hotspot_uplift`, `hotspot_trail`); **`hotspots` defaults to
  0 in `flicker-worldgen`** so base tectonics + its tests are unchanged, while
  **flicker-world ships `e3_hotspots = 6`** so it's visible. Surfaced in the
  Tectonics knob panel. Unit test: turning hotspots on lifts the surface above the
  no-hotspot peak.

- **Axial tilt / insolation (W5b). ✅** Epoch 4 (`epoch4.rs`) gains `axial_tilt`:
  its annual-mean effect flattens the equator→pole temperature band (quadratic, so
  Earth-like tilt barely shifts it but extreme tilt evens it out / warms the
  poles). Default `0` in worldgen (band unchanged + tests pass); flicker-world
  ships `e4_axial_tilt = 23.5`, knob in the Hydrosphere panel, visible in the
  temperature field. Unit test: tilt 90 raises the coldest (polar) temperature.

- **App shell (U1). ✅** Stack-of-scenes flow: `Menu` (Start/Settings/Quit) →
  `Loading` → `World` → `Pause` overlay (Resume/Settings/Quit) → `Settings`
  overlay, with a reusable flat `Modal` (`shell.rs` + `modal.lua` + `screens`
  JSON). Esc in-world pauses (world freezes beneath the overlay). Ported from
  voxel-cluster's pattern, flat-skinned rather than dragging in its gothic
  `Theme`. Headless test renders each screen's buttons.

- **Settings workbench (U2a). ✅** A framed left-nav panel (`shell.rs` `Settings`
  + `settings.lua` + `ui_elements.json` `settings`): nav (Camera / Audio / Video /
  Key Mappings) on the left, data-driven control rows (sliders / toggles /
  dropdowns from `settings.pages[<page>].controls`) on the right. Values live
  under `<page>_<id>` keys in `GameSettings`, persisted to `settings.json` on Back.
  Camera page carries invert Y/X, sensitivities, deadzones, trigger, and
  **vibration on/off + intensity (prefs only — no rumble backend exists yet)**.

- **Logo splash + Key Mappings rebind (U2b). ✅** `Logo` scene plays the Elideus
  Productions + Clay Engine PNGs (`assets/`, `scripts/logo.lua`, `UI.logo`) →
  Menu, skippable. The Key Mappings page lists the standard `Action`s with their
  binding; clicking a row starts a live `RebindCapture` (overlay: "press a
  key…"); Esc/click cancels. `parse_action`/`binding_label` in `settings.rs`.

Still deferred:

- **Keybinding disk persistence — BLOCKED.** The engine `InputMap` can't
  serialise to JSON (`input_to_action` keys on `InputBinding`, a non-string key →
  "key must be a string"). So `GameSettings.input_map` is `#[serde(skip)]` —
  rebinds persist for the *session* (via the `GAME_SETTINGS` global) but reset to
  default on restart. Fix is in `flicker-core` (custom `InputMap` serde that
  stores only `action_to_bindings`); spawned as a separate task.
- **Display-apply + gameplay routing.** Apply video display-mode via the renderer
  (`set_windowed`/`set_*_fullscreen`); route camera settings into the orbit camera;
  route the world's controls through `Action`/`InputMap` (today they're hardcoded). The splash is a centred text title; real logo
  art lives in `voxel-cluster`'s heavy gothic theme — promote a reusable one.
- **Pick / cell inspection + abstraction pass.** Click a cell to read its
  `HexState` into the panel (hex-map's pick-ray); lift text/camera/pick into
  shared modules.

---

## 3b. Epoch visualization (the "epoch passes")

> **Canonical epoch design:** `docs/clayengine_world_generation_spec_v2.md`
> (§"Epoch specifications"). It defines what each epoch *means* and its
> Phase-1/Phase-2 sophistication. The sim is in `flicker-worldgen/src/epoch*.rs`;
> the per-epoch fields it writes are on `HexState`
> (`flicker-worldgen/src/state.rs`). This section is only the **viewing** layer.

Each epoch must read as its own meaningful phase. Two mechanisms support that:

- **Direct-RGB material escape (in `flicker-render` `mesh.wgsl`).** Added a second
  material encoding: `primary == 0xFFF` ⇒ the upper bits are a packed **RGB666**
  colour, so a view can express *any* colour, not just a 2-stop palette ramp.
  Additive + conflict-free (no real palette index is 0xFFF; existing ramps use
  indices ≤ 30). `color.rs` `direct([r,g,b])` packs it. This is the enabler for
  continuous data maps.
- **`ViewMode::Composition` = the diffuse material map.** Each cell's colour is the
  **amount-weighted blend** of its surface elements' muted primordial tints
  (`element_rgb`: Fe→molten rust, C→near-black, silicates→blue-grey rock) over a
  dark base — so the *mix* (and the element sliders) shift the colour **smoothly**,
  not in discrete dominant-element jumps. (Replaces the old hard
  dominant-element classification, which didn't visibly respond to slider edits.)
- **`ViewMode::Crust` = differentiation (Epoch 2).** Gravitational differentiation
  sinks heavy metals to the core and floats a light silicate crust. Two reads: the
  Composition view shows the **iron draining** from the surface (surface() → crust,
  iron-depleted), and the Crust view ramps **molten** (thin crust, heavy near the
  surface) → **solid** silicate (thick crust), normalised per layer. Responds to
  the mix: more iron → thinner crust (a test guards this). At Epoch 1 crust_fraction
  is 0 everywhere → all-molten, which reads correctly as "undifferentiated".
- **Per-epoch "natural" view** (`scene.rs` `natural_view` + `set_epoch`): stepping
  to an epoch auto-selects the field it's best read in — Composition for Epoch 1,
  **Crust for Epoch 2**, Elevation for Tectonics, Temperature for Hydrosphere,
  Biome for Erosion. (Epoch 5 / Mineralization has no dedicated view yet — a future
  pass; it falls back to Elevation.)

---

## 4. Notes / decisions

- **Colour is palette-packed *or* direct RGB.** The mesh shader resolves
  `material = primary | secondary<<12 | blend<<24` against a fixed palette, **or**
  `primary == 0xFFF` ⇒ packed RGB666 (§3b). Palette ramp for discrete/banded data;
  direct RGB for continuous maps.
  (`material_index_color` in `mesh.wgsl`). Continuous fields ramp between two
  palette stops. To add a colour, extend the shader palette and reference its
  index in `color.rs`.
- **Two cost tiers.** Changing the **field or epoch** only rebuilds + re-uploads
  the mesh from the already-computed layers (cheap). Changing a **knob, seed, or
  grid frequency** re-runs the whole epoch chain (`generate`) then rebuilds the
  mesh; knob drags defer this to release so dragging stays smooth. Fine at these
  cell counts on the A18 (see memory `dev-box-profile`). Per-cell flat shading
  keeps cells legible.
- **Equal-area still pending.** Positions are the grid's cheap projection
  (Slice 3b ISEA is not done), so cell sizes vary ~1.75×; visible but not wrong.
- **Verification is visual** — `cargo build/clippy -p flicker-world` is green; the
  user runs the window and confirms the planet renders/colours/orbits (per memory
  `user-verifies-app-themselves`).

---

## 5. Slice ladder

- **W1 — runnable viewer. ✅** Crate scaffold, `Loading → World` flow, icosphere
  mesh coloured by epoch field, orbit camera, text HUD. Builds, clippy-clean.
- **W2 — Lua UI. ✅** `world_ui.lua` + `ui_elements.json` driving a field
  dropdown / grid stepper / reseed button / stats via `flicker-script` +
  `flicker-ui`; orbit suppressed over the HUD; headless HUD test. Text HUD kept
  as fallback.
- **W3 — epoch control panel. ✅** Keep all six epoch layers; epoch stepper +
  per-epoch knob sliders (`WorldParams`/`PARAM_DEFS`) that re-tune each phase and
  regenerate on release. Unit + headless HUD tests.
- **W4 — element-mix grid + composition view. ✅** Per-element abundance sliders
  (Epoch 1) via the JSON-row grid; composition field tints by dominant element.
- **W5a — hotspot volcanism. ✅** Plate-motion-driven uplift chains over fixed
  mantle plumes (Epoch 3, `flicker-worldgen`); knobs in the Tectonics panel.
- **W5b — axial tilt. ✅** Obliquity flattens the temperature band (Epoch 4); knob
  in the Hydrosphere panel.
- **U1 — app shell. ✅** Menu/Loading/World/Pause/Settings scene flow + reusable
  flat Modal.
- **U2a — settings workbench. ✅** Framed left-nav panel; Camera/Audio/Video pages
  persisted to `settings.json`.
- **U2b — Logo splash + Key Mappings rebind. ✅** Elideus/Clay splash; live
  rebind via `RebindCapture` (session-persistent; disk persistence blocked on the
  `InputMap`-serde task).
- **E1 — Epoch-1 visualization. ✅** Direct-RGB shader escape + continuous
  diffuse material map (`ViewMode::Composition`) + per-epoch natural views.
- **E2 — Epoch-2 differentiation. ✅** `ViewMode::Crust` (molten-thin → solid),
  iron drains in the Composition view; both respond to the element mix.
- **Epoch passes (ongoing).** Epoch 5 needs an ore/vein view; Epoch 3 (tectonics)
  and 4 (hydrosphere) could get richer reads; etc. **The recorded-data audit
  (what each epoch records vs the spec, and the 6↔9 water-cycle reconciliation)
  now lives in `docs/epoch-data-audit-handoff.md`** — its slice 1 added Epoch 3's
  cross-hex `Plate` records + per-hex `plate_age` (`WorldData.plates`, HUD plate
  count). A plate-motion / age overlay is the natural viewer follow-on.
- **Later — keybinding disk persistence, display-apply + gameplay routing,
  pick/inspect, abstraction pass.**
- **Later — logos/loading art, pick/inspect, abstraction pass.**
