# flicker-world — Continuous Sim, Slice 2 (the heat-loss scrubber UI)

State-of-play for **Slice 2** of the continuous, cooling-driven planet sim
(`docs/flicker-world-continuous-sim.md` = design of record; read
`docs/flicker-world-continuous-sim-slice1-handoff.md` first — it built the cooling
clock this slice makes visible). Slice 2 = **make the cooling clock observable** and
**merge the two per-epoch iteration scrubbers into one heat-loss scrubber**, per the
doc §6 ("the per-epoch iteration scrubbers → one logarithmic heat-loss scrubber with
generated onset markers"). Pure viewer work — no physics change.

> **Landed 2026-07-09.** Tests: worldgen 87, worldengine 16, pocepochs **3** (+1) —
> all green; clippy clean on the three crates (only the pre-existing
> `flicker-core/src/input/*` warnings). Build clean. User picked the fuller
> **merge-into-one-scrubber** option (over visible-first/relabel).

---

## 1. What the slice delivers

- **One unified heat-loss scrubber** replaces the two per-epoch step sliders (the old
  Epoch-2 "molten convection" + Epoch-3 "tectonic iteration" scrubbers). It spans the
  whole molten→tectonic cooling as a single position `cool_step ∈ [0, cool_total]`:
  - `[0, molten_steps]` → **Epoch 2** convection (`cool_step` = convection steps);
  - `(molten_steps, cool_total]` → **Epoch 3** drift (`cool_step − molten_steps` steps).
  - Dragging **across the boundary switches the shown epoch** (and moves the bottom
    timeline playhead to match); entering the tectonic era forces the molten era to
    full first, so E3 always runs off the complete molten era.
- **The cooling clock is now visible:**
  - a persistent **"planetary heat" readout** in the stats line (`T` + "% cooled"),
    on every epoch, from the authoritative `snapshot.temperature`;
  - a **logarithmic T-decay curve** (`T = (1−k)^step`) drawn under the scrubber, hot at
    the molten left, decaying to cool at the right, with the current step lit;
  - the **molten|tectonic boundary** tick and the **tectonics-onset marker** (gold) on
    both the track and the curve — so a *delayed* onset (radiogenic-rich world) shows
    up as the marker sliding into the tectonic region, and a **stagnant** world shows
    "plate tectonics never begin" instead of a marker.

## 2. Where it lives

- **`crates/flicker-worldengine/src/engine.rs`** — two small read-accessors:
  `cooling_k()` (the decay coefficient, off the always-present E1 seed layer) and
  `tectonics_onset_step()` (absolute onset step, `None` = stagnant). The viewer plots
  the curve from `cooling_k` and marks `tectonics_onset_step`.
- **`Alpha/flicker-pocepochs/src/scene.rs`** — replaced the `e2_*`/`e3_*` scrubber
  state with `cool_step` / `cool_total` / `molten_steps` / `cool_onset` / `cool_k` /
  `cool_my_per_step` / `temperature`. `refresh()` syncs the span + onset + `T`;
  `update()` drives the one scrubber (with the boundary→epoch switch);
  `settle_cooling_for(ep)` parks the scrubber + clears the step override on coarse
  (timeline / ↑↓) navigation onto E2/E3. Stats line gains the `T` readout.
- **`Alpha/flicker-pocepochs/src/world_hud.lua`** — the two per-epoch scrubber
  branches (update + draw) collapsed into one `on_epoch2() or on_epoch3()` branch:
  the slider, the boundary + onset ticks, the T/era/step (+My) label, and the bar
  sparkline of the cooling curve. New model keys `cool_step/cool_total/cool_molten/
  cool_onset/cool_k/cool_temp/cool_my`; emits `cool_step`. The curve is computed in
  Lua from `cool_k` (`(1−k)^s`) — plain-number display math, the authoritative `T`
  still comes from the engine (T_SPACE=0/T_MOLTEN=1 today, so the display matches).

## 3. Behaviour notes / decisions

- **Boundary parking:** landing on E2 parks the scrubber at `molten_steps` (full
  convection, handle on the divider); landing on E3 parks at `cool_total` (full
  tectonics). So coarse epoch nav shows the settled sim, and the scrubber then rewinds
  within/across the cooling span.
- **The onset marker is meaningful now:** at the default it sits at the molten/tectonic
  boundary (onset delay 0); for a radiogenic-rich world it sits partway into the
  tectonic region, and the E3 positions *before* it show the rigid-but-not-yet-drifting
  lid (drift begins at the marker).
- **The bottom 9-epoch timeline is unchanged** — it stays the coarse epoch selector.
  Reparameterizing *it* into the full heat-loss timeline (epochs as markers on one
  cooling axis, doc §6) waits until the clock spans more than E2–E3.

## 4. Verify (user runs the visuals)

`cargo run -p flicker-pocepochs` → scrub to Epoch 2 or 3: one "Heat loss" scrubber now
spans the molten→tectonic cooling. Drag it left↔right to cool the planet from molten
convection into plate tectonics; watch the **T readout**, the **log curve**, and the
**gold onset marker**. Reseed (R) or crank a radiogenic recipe to see the onset move.
Keep `cargo test -p flicker-worldgen -p flicker-worldengine -p flicker-pocepochs`
green (87 + 16 + 3) + clippy.

## 5. NOT in this slice (later)

- **Full-timeline reparameterization** (the bottom 9-epoch bar → one logarithmic
  heat-loss axis with epochs as onset markers) — needs the clock to span the whole
  history (E4–E9 still freeze `T`), which the water-delivery / outgassing slices bring.
- The remaining physics onsets: **water-delivery**, **outgassing**, **moon→tilt→
  seasons**, **chemistry** (each will drop another marker on this scrubber).
