# flicker-world — Continuous Sim, Slice 3 (one heat-loss timeline)

State-of-play for **Slice 3** of the continuous, cooling-driven planet sim
(`docs/flicker-world-continuous-sim.md` = design of record). Slices 1–2 built the
cooling clock (`flicker_worldgen::cooling`) across E2→E3 and a scrubber to see it; the
clock still **froze at E3** and the viewer had two disconnected time controls, so the
world still read as discrete bolted-on epochs. Slice 3 addresses that head-on: **the
whole history becomes one continuous heat-loss timeline** (doc §6, "the 9 fixed epochs →
onset markers on one cooling timeline; the per-epoch iteration scrubbers → one
logarithmic heat-loss scrubber").

> **Landed 2026-07-09.** Tests: worldgen 87, worldengine **17** (+1), pocepochs **4**
> (+1 net) — all green; clippy clean on the three crates; full `cargo build --workspace`
> clean. No physics change (zero E4–E9 regression risk); the change is the *thermal
> accounting* + the *viewer*.

> **Update (2026-07-09b) — Space play/pause + the batch-cycle foundation.** The timeline
> now *plays*: **Space** toggles playback, the world **starts paused at Epoch 1**, and
> pressing Space auto-advances the cooling cursor — running E2 convection then (as the
> tectonics-onset conditions are met) E3 drift — and **pauses at Epoch 4's start** (the
> frontier of what's wired to iterate). Resuming from a finished run replays from the
> molten seed; a manual scrub / ↑↓ / reseed stops playback. This is deliberately built as
> the **planet batch cycle**: `advance_play` is a cursor stepping passes over the state
> (`apply_cool_step` = one pass), auto-pausing at a stop condition — the same shape the
> runtime will use to batch-process hexes. Wiring a later epoch into the run is a one-line
> bump of `PLAY_STOP_EPOCH` (+ that epoch's onset/sub-step arm in `cool_target_at_step`);
> the loop extends into it automatically. Consts: `PLAY_STEPS_PER_SEC = 6`, `PLAY_STOP_EPOCH
> = 4`. A `PLAYING`/`PAUSED` indicator + "Space play/pause" are in the stats HUD. Tests:
> pocepochs **5** (+1, `playback_runs_the_cooling_cycle_and_pauses_at_the_frontier`);
> worldengine 17, worldgen 87; clippy clean.

---

## 1. What the slice delivers

**The cooling clock now runs the whole history.** Before, E4–E9 did
`temperature = prev.temperature` — `T` froze after tectonics. Now they continue the
same Newtonian decay, so `T` falls monotonically molten (1.0 at the E1 seed) → the
geological crawl (<0.1 by E9) across *every* epoch. The later epochs' physics is
untouched (only the recorded `T` advances); each becomes a real `T`-gated onset in a
later slice.

**The viewer's two time controls are now one heat-loss timeline.** The bottom bar
*is* the cooling clock: its nine segments are weighted by each epoch's cooling-step
share, a logarithmic `T = (1−k)^step` decay curve + the tectonics-onset marker are drawn
above it, and the single playhead scrubs a global cooling step over the whole history.
The separate E2/E3 heat-loss scrubber (slice 2) is gone — absorbed into the timeline.

## 2. Where it lives

- **`crates/flicker-worldengine/src/engine.rs`**
  - E4–E9 continuation: after the epoch `match`, `if epoch >= 4 { temperature =
    cooling::temperature_at(k, self.cool_step_end(epoch)) }`.
  - `COOL_STEPS_PER_DURATION = 4` (the post-tectonic cooling rate; matches the
    molten/tectonic rates so the clock is uniform per duration unit).
  - Accessors: `epoch_cool_steps(e)`, `cool_step_before(e)`, `cool_step_end(e)`,
    `cooling_total_steps()` — the one continuous cooling axis the viewer reads.
- **`Alpha/flicker-pocepochs/src/scene.rs`**
  - `cool_step` is now a global position `0..cool_total` over all epochs; `cool_starts`
    caches the per-epoch boundaries; `applied_e2/applied_e3` track the sub-step overrides
    so a drag regenerates only when the sim state actually changes.
  - `segment_edges` is cooling-weighted (E1 = a fixed `E1_FRAC = 0.05` leading slice).
  - `cool_step_at`/`playhead_at`/`epoch_at_cool`/`cool_target` map the playhead ↔ the
    cooling clock ↔ (epoch, E2/E3 sub-step); `park_epoch` is the ↑↓ / initial park.
  - `update()`: the timeline playhead drives everything (no separate scrubber);
    `settle_cooling_for` / `epoch_at` / `segment_center` retired.
- **`Alpha/flicker-pocepochs/src/world_hud.lua`** — one control: the timeline draws the
  cooling-weighted segments + group bands + playhead (shared `Widgets.timeline_*`), the
  log T-decay curve strip above the bands, the gold onset tick, and a `HEAT-LOSS CLOCK ·
  T=… (…% cooled)` caption (+ a "stagnant lid" note when tectonics never begin). The
  Epoch-1 planet-size slider stays. New model keys `cool_e1frac` (+ the slice-2 cool_*);
  the scrubber's `cool_step` *output* is gone (the scene derives it from the playhead).

## 3. Behaviour / decisions

- **No recalibration.** E2/E3's `T` values (and thus slices 1–2's verified output) are
  byte-identical; only the previously-frozen E4–E9 now record a falling `T`. So the
  default planet is unchanged — this slice is safe by construction.
- **`T`→low tail is honest.** With the slice-1 `k`, `T` reaches the geological crawl
  (~0.02–0.08) by E5–E6. That's the model saying the *formation/interior* heat is spent;
  the later epochs (oceans, life, erosion) are surface/sun-driven, and become their own
  onsets later. `T_SPACE = 0` today; a small radiogenic floor is a later refinement.
- **E4–E9 don't sub-step yet.** Within their segments the playhead moves (cooling
  position), but the epoch shows its full settled sim; only E2/E3 sub-scrub. Per-epoch
  sub-stepping for the later epochs is deferred.
- **Linear-in-steps axis.** The bar is linear in cooling steps (the T curve carries the
  "fast early" log shape). A true log-*time* axis is a possible refinement.

## 4. Verify (user runs the visuals) — worth an eyeball

`cargo run -p flicker-pocepochs`. Drag the **one bottom timeline** left→right: the planet
should cool continuously from the molten seed through convection → tectonics → oceans →
life → erosion, with the **T readout falling the whole way** (top-left stats) and the
**log curve + gold onset tick** above the bar. Things to check specifically (this was a
large viewer rewrite done without a live window):
- dragging within the molten (E2) / tectonic (E3) segments still sub-steps the
  convection / drift as before;
- crossing segment boundaries switches the shown epoch cleanly (no flicker/jumping);
- ↑/↓ still parks on whole epochs; the size slider still works on Epoch 1;
- reseed (R) moves the onset tick with the recipe's cooling pace; a stagnant world shows
  the "no plate tectonics" note.

Keep `cargo test -p flicker-worldgen -p flicker-worldengine -p flicker-pocepochs` green
(87 + 17 + 4) + clippy.

## 5. NOT in this slice (the honest gaps → next)

- **The later epochs still "pop" at their boundaries** — the *frame* is now one
  continuous cooling timeline, but E4–E9 processes still turn on at their epoch slots,
  not at `T`-thresholds. Making them real `T`-gated onsets is the remaining physics
  unification: **water-delivery** (oceans condense when `T` < boiling — the first and
  most visible one), then **outgassing**, **moon→tilt→seasons**, **chemistry**.
- Per-epoch sub-stepping for E4–E9 on the timeline; a log-*time* axis; a radiogenic
  `T` floor for the tail.
