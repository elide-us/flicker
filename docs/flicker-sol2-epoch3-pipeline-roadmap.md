# flicker — Phase 1→3 / Epoch-3 pipeline roadmap

Planning doc (2026-06-24). Captures the multi-session task inventory the user laid out to take
`examples/flicker-sol2` from "working POC" to a **cleanly boxed simulation with an API + data
contracts** that feeds the **single-world planet simulation** (Phase 3). This is a roadmap, not a
commitment of order — prioritisation is the user's. Honours all locked sol2 decisions (cast-by-
atomic-weight, star-mass extraction, emergent/no-clamp, no scripted celestial events).

## The phase model (user's framing)

- **Phase 1 — initial system material distribution.** The supernova ejecta cloud → the overdensity
  "hot spot" locations. (Today: the sol2 distribution view + `detect.rs` dots.)
- **Phase 2 — collapse / gravity-well sim, capped at ~1.5 BY** (systems stabilise by then).
  Hot spots → bodies → accretion/moons/rings. **This END point = END OF EPOCH 3** of the original
  Nine-Epoch planet model — sol2 has *subsumed* the formation epochs. Output: the selected HZ
  body's **accurate material accounting** (the Phase-3 seed). (Today: `collapse.rs`.)
- **Phase 3 — the single-world planet sim** on the HZ body, built as the 49.6-mi-hex ISEA
  hex-sphere (12 pentagons buried). **Epochs 4–6: ~1.5 BY of plate tectonics** — multiple complete
  crustal submersions, continents joining / splitting / fragmenting / subducting — driving an
  **expanded layered material model + vast resource veins**. (Today: `flicker-worldgrid` /
  `-worldgen` / `-world`; epochs 1–6 exist but fixed-iteration, materialization not built.)

---

## Workstream A — Box up flicker-sol2 into a clean simulation crate + API

The foundation; everything else hangs off it. (Extract the *working sol2*, **not** the abandoned
`flicker-celestial`.)

- [ ] Extract the sim into a library crate (name TBD — fresh name, e.g. `flicker-system`; do NOT
      reuse `flicker-celestial`). The `examples/flicker-sol2` viewer becomes a thin shell over it.
- [ ] Separate the three concerns cleanly: **sim logic** (lib) ↔ **render** (`scene.rs`/`draw.rs`)
      ↔ **input/UI** (data-only). Today render + input are fused in `scene.rs`.
- [ ] **Input contract — `SystemConfig`**: every lever as one struct (Mass, Metallicity, explosion/
      reach, falloff, clump, seed; `STAR_GAS_FRAC`, `DISK_SPIN`, `DRAG`/`GAS_TAU`/`DRAG_FLOOR`,
      `DRAG_TARGET_FRAC`, `RADIUS_K`, `SOFTENING`, `HILL_FRAC`, `TIDAL_FRAC`, `RHO_*`,
      `PLAYABLE_MASS_*`/`HZ_*`, `SIM_YEARS_PER_SEC`, `MOTES_PER_EL`). The constants become fields.
- [ ] **Output contract — `SystemState` / `BodySnapshot`**: per-body conserved `comp`, mass, vel,
      type, ring tonnage, host, playable flag — a stable read model for UI + handoff.
- [ ] **Handoff contract — `Epoch3Handoff`** (the Phase-2→3 seam): the selected HZ body's exact
      per-element composition + total mass + moon(s) + orbital/insolation context (for temperature).
- [ ] Decision: crate placement (umbrella re-export? workspace-only like `flicker-celestial` was?).

### Session-A outcome — DONE 2026-06-24 (handoff to Session B)

**Workstream A is complete.** The sim is boxed into a new GPU-free crate and the viewer is a thin
shell over it. 21 tests pass (moved with the sim), clippy clean, `cargo build --workspace` green.

**New crate `crates/flicker-system`** (workspace-only dep, like `flicker-celestial` was; not in the
umbrella). Deps: `glam` (Vec2) + `flicker-materials`. Modules:
- `model` (cast + Prism table + colours), `mass` (two-dial tonnage), `cloud` (clumpy sheared field),
  `detect` (**now world-space**: `detect() -> Vec<HotSpot{au,theta,strength}>`, no screen/`View`),
  `collapse` (the sim — now reads all levers off `Sim::tuning`).
- `config` — **`SystemConfig`** (the input contract): `{ cast, mass, clump, seed, motes_per_el,
  tuning }`. **`Tuning`** hoists every collapse physics/typing/playability lever (softening,
  radius_k, star_gas_frac, disk_spin, drag/drag_target_frac/gas_tau/drag_floor, rho_*, hill_frac,
  tidal_frac, star/giant/planet_mass, playable_mass_*, hz_*) — defaults reproduce the confirmed
  regime. Only `G`/unit-conversion/`MAX_DT` stay as constants.
- `system` — **`System`** facade + the output contracts. Verbs: `new(config)`, `config()` /
  `config_mut()`, `sync_distribution()` (re-derive Phase-1 after dial changes), `reseed(seed)`,
  `ignite()`, `clear()`, `step(dt_years)`. Reads: `ejecta()`/`cloud()`/`cloud_mass()`/`sim()`,
  `anchor_au()`, `hot_spots(time)`, **`state() -> Option<SystemState>`**, **`epoch3_handoff() ->
  Option<Epoch3Handoff>`** (picks the most-massive playable world).
- **`SystemState`** `{ time, star_mass, total_mass, init_total, bodies: Vec<BodySnapshot> }`;
  **`BodySnapshot`** `{ index, pos, vel, mass, ring_mass, radius_au, kind, host, playable }` +
  `is_star()`/`is_moon()`. **`Epoch3Handoff`** `{ composition: Vec<ElementMass>, total_mass_msun,
  orbit_radius_au, star_mass_msun, moons, has_ring }` — the Phase-2→3 payload.

**`examples/flicker-sol2`** is now a thin viewer: `CloudView { system: System, <view state> }`. It
feeds dials through `config_mut()` + `sync_distribution()`, and renders from `system.sim()` /
`system.hot_spots()`. (`scene.rs`/`draw.rs`/`well.rs` only; the five sim modules moved out.)

**For Session B (UI seam + phase nav):** route `SystemConfig` through the **Lua HUD**
(`flicker-script` data-only boundary; the `flicker-world` per-epoch-knob panel is the live pattern;
the voxel-cluster celestial panel is a *purpose-only* reference — speed control + eclipse alignment).
The contracts are ready to drive a UI: feed `SystemConfig`, read `SystemState` for the panel.
**Minor A-tail (optional):** the renderer still reads `system.sim()` fields directly (a hybrid) —
it can migrate to `SystemState`/`BodySnapshot` for full purity; and `serde` on the contracts is
deferred (add when the handoff needs to persist to disk / cross a process boundary).

## Workstream B — UI seam (Lua) + phase/stage navigation

The existing UI is the **Lua HUD** (`flicker-script` strict data-only boundary; `flicker-world`'s
per-epoch knob panels are the live pattern). The **voxel-cluster celestial panel is a PURPOSE
reference only** — its code is fabricated; harvest the *intent* (speed control, eclipse alignment),
nothing else.

- [x] Route `SystemConfig` levers through the Lua HUD (replaced the hardcoded key handling in
      `scene.rs`); swapping the panel = swapping a `.lua` file.
- [~] **Time-scale / speed dial** (yr/s) + pause done; **alignment scrubbing** (seek the sim clock
      to line up eclipses / transits) deferred — the collapse isn't reversible, so a real scrub
      needs a re-run-from-t0 or a recorded-state buffer. Flagged, not built.
- [x] **Phase/stage navigation UI**: Phase 1 (distribution) ↔ Phase 2 (collapse) buttons (clear /
      ignite); Phase 3 (planet) is a disabled signpost (it lives in flicker-world).

### Session-B outcome — DONE 2026-06-24

**The Lua UI seam + app shell landed.** `examples/flicker-sol2` is now a scene-driven app mirroring
the flicker-world pattern, not a single hardcoded-key viewer.

- **App shell** (`src/shell.rs`, ported from flicker-world): `Logo` (two **full-frame** splash PNGs,
  `mode:"fill"` in `ui_elements.json` — switch to `cover`/`fit` without recompiling) → `Menu`
  (Start / Settings / Quit) → `Sim`; `Sim` pushes `Pause` (overlay) on Esc; Menu/Pause push
  `Settings`. **Start drops straight into the Stage-1 sim.** `Settings` is a lean placeholder modal
  (sol2 has no GameSettings/InputMap infra — the dials live on the sim panel); promote to a tabbed
  workbench when sol2 grows real display/input settings.
- **Sim scene** (`src/scene.rs`, `CloudView` → `Sim`): all world rendering preserved (rings, focus
  band, hot-spot dots, collapse bodies, motion arcs, gravity well, axes). Every readout + control
  moved to Lua. `hud_model()` publishes the dials + conserved-mass + collapse status; `apply_ui()`
  harvests the panel back into `SystemConfig` + view state. `ui_capture` gates world hover/zoom so a
  slider drag doesn't also re-focus a ring. Esc → Pause overlay.
- **Lua HUD** (`scripts/sim_ui.lua` + `ui_elements.json`): a **bottom-right-quarter control panel**
  (phase nav · explosion/falloff/clump/mass/metallicity/speed/view-edge sliders · pause/dots/well
  checkboxes · new-seed/new-system/reset buttons) and a **top-right stats overlay** (cloud mass,
  metals %, conserved sum, focus, tonnage line; collapse t/bodies/star/conservation/type breakdown).
  `scripts/logo.lua` + `scripts/modal.lua` round out the screens.
- **UI backend improvement** (the user's invite): added reusable `checkbox_update`/`checkbox_draw`
  + `panel_draw` to `crates/flicker-ui/src/widgets.lua` (additive — flicker-world/voxel-cluster
  unaffected; their tests still pass).
- **Verified:** `cargo build --workspace` green; `flicker-sol2` (2 HUD-load tests) + `flicker-ui` +
  `flicker-world` tests pass; clippy clean on the touched crates.
- **Known tradeoff:** the bottom-right panel (semi-opaque) overlaps the lower-right of the ring view
  (the view center is left-of-centre but the rings are large/symmetric). Left as-is — the sol2 view
  is user-confirmed; reposition the view if the occlusion bothers. **A-tail still open:** the
  renderer reads `system.sim()` directly (hybrid) rather than `SystemState`; serde on the contracts
  still deferred.

## Workstream C — Phase-2 rigor: 1.5 BY cap, material accounting, handoff emission

- [ ] **Cap Phase 2 at ~1.5 BY** (configurable) — the stabilisation point = end of Epoch 3.
- [ ] **Audit + test the per-body material accounting**: the HZ body's final `comp` must be the
      exact integral of everything it accreted over the full run (global conservation already
      holds at 1.0000; this verifies the *per-body* ledger, incl. how ring/moon mass is attributed).
- [ ] **Emit `Epoch3Handoff`** for the selected HZ world — the readable material payload for Phase 3.

## Workstream D — Phase-1/2 outcome constraints (acceptance + regeneration)

- [ ] Constraint/acceptance layer: generate → test against required features → **discard + reseed**
      until satisfied. Minimum constraint: **a playable world that HAS a moon.** Configurable (the
      user's art levers; could grow to "≥N rocky", "a gas giant", "rings present", etc.).
- [ ] Surface it in UI (auto-reseed until constraints met / show why a system was rejected).
- [ ] **Seed Stage 2 from the Phase-1 *potentials* (a subset, not all).** Design gap flagged by the
      user (2026-06-24): potentials are meant to be *candidates* — Stage 2 should select a subset.
      But today `collapse::from_cloud` samples `motes_per_el` parcels **per element from the clump
      CDF** — it does **not** seed from `detect`'s hot-spot dots at all (the dots are a separate
      Phase-1 viz). The intended design (Phase 2 = "transition hot spots into bodies") is to seed
      from the detected potentials and **select a subset**. **Interim done (2026-06-24):** default
      `motes_per_el` halved 24 → 12 to start Stage 2 with ~half the bodies (conservation-safe —
      heavier parcels). **Full alignment (future):** make `from_cloud` seed from the `detect` hot
      spots + a real selection step (top-N by strength / probabilistic), so "potentials → selected
      bodies" is literal.

## Workstream E — Comets (missed body type)

- [ ] Add **comets** as an emergent type: small **icy** body on a **high-eccentricity / outer**
      orbit (use `orbit_peri_apo` eccentricity + a `COMET_MIN_ECCENTRICITY`-style gate; spec
      AC039447 had 0.4). Distinguish from `IcyBody`/`Asteroid`.
- [ ] Render the comet path (the eccentric orbit) / a tail cue. Fits the Body/Ring/Belt/**Comet**
      satellite-kind vocabulary from the celestial spec.

## Workstream F — Phase 3: the single-world planet sim (its own multi-session arc)

Extends the **existing, live** hex-sphere stack (NOT abandoned). Large; partially built.

- [ ] **Epoch reconciliation**: sol2 Phase-2 subsumes the *formation* epochs (≈1–3); the planet sim
      runs the *crustal-evolution* epochs (≈4–6). Define the boundary and renumber per the vision
      note (43DB8577 — possible fresh 7–9 "life era", runtime layers → 10–12).
- [ ] **Seed the planet from `Epoch3Handoff`**: HZ body composition → per-cell initial `Composition`
      on the hex-sphere. Pick `freq` from planet radius (tile fixed at 49.6 mi; Mercury≈48,
      Earth≈100). Wire through `flicker-worldstate::Ledger` (`CellId↔CellCoord`, Slice 4 — pending).
- [ ] **Iterative plate tectonics over ~1.5 BY** (≥6 full crust recycles): make the current
      *fixed-iteration* Epoch-3/tectonics sim run long-form so continents emerge, collide, split,
      fragment, subduct. (Re-home the conserved water cycle from `examples/hex-world/layers.rs`.)
- [ ] **Expanded layered material model** — the per-cluster 2048² **materialization bridge** (the
      stated overarching goal; `docs/material-model-impl-handoff.md`). Produce a **heightmap STACK
      of materials** per cluster (the VoxelFarm IVoxelLayer idea, replicated in flicker-voxel):
      e.g. dirt 40 voxels deep over stone 80 deep — dig down and pass through the strata. Realistic
      target ~5–6 stacked layers (Grand-Canyon-wall feel within technical limits).
- [ ] **Material veins**: vast condensed resource veins spread across the world (cross-hex vein
      paths — currently per-hex only; the Rivulet/vein work).

---

## Cross-cutting decisions to settle as we go (the user's calls)

- New sol2 lib crate **name + placement**.
- Exact **`Epoch3Handoff`** shape (what Phase 3 actually needs: composition only? + moon(s)? +
  insolation/temperature context?).
- How Phase 3 ingests it — directly vs through the `flicker-worldstate` ledger.
- IVoxelLayer = **replicated technique**, not a VoxelFarm dependency (flicker has its own voxel path).
- Epoch renumbering (depends on the "life era" trilogy decision).

## Invariants to honour throughout (do not relitigate)

Emergent / no-clamp (27AB9E65); simulate-not-paint; **celestial events emerge from deterministic
state, never scripted** (9D9001ED — the predict-the-heavens payoff); conserve to the gram; the
locked cast/star-extraction model (C79F2F14) — refinements (e.g. ice-giant ordering, note
401A2B86) sit in the star-mass-extraction layer, not a rewrite. Reality-LIKE, not reality-accurate.
