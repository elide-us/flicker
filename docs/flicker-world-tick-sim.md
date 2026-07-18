# flicker-world — The Tick Simulation (execution architecture)

The **concrete execution model** that realizes `docs/flicker-world-continuous-sim.md`
(the philosophy: heat-loss master clock, processes accumulate, onsets are conditions).
Locked with the user 2026-07-09. It **supersedes the discrete execution model** — the
9-epoch `EpochSnapshot` cache + `run_epoch` + fixed `cool_step_before(e)` boundaries —
which was a *timeline wearing a simulation's clothes*: it scheduled boundaries ("at
cool_step 40, Epoch 4 starts") instead of producing them.

> **One line:** one evolving world is ticked forward forever; each tick runs every
> **active process** over all hexes, then every **evaluation** checks the world for
> **tipping points** that **activate new processes** — so phases (the old "epochs") are
> *emergent* (which processes are on), not scheduled.

---

## 1. The loop

```
each tick (the sim's unit of time; the viewer plays ~1/sec):
  1. run every ACTIVE process over all hexes          — the pipeline
  2. run every EVALUATION against the resulting world — end-of-tick conditions
  3. each evaluation whose condition is now met is a TIPPING POINT:
       activate its process(es)  (they tick forever from now on)
  tick += 1
```

Nothing is handed off; processes **accumulate**. A phase = the current active set.
The sim is **infinite** — it never "finishes at Epoch 9"; it keeps evolving.

## 2. The three pieces

- **World** — the single evolving truth: the per-hex `HexState` array + global state
  (thermal `T`, tick count, the plate partition/phase once tectonics is on, the active
  set + which tippings have fired, the composition-derived cooling `k`). Everything the
  re-sim needs to reproduce a tick lives here.
- **Process** — one **per-tick** update over all hexes. A trait; our existing physics
  kernels become processes (each does *one step*, persisting its evolving state in the
  World, not a batch of N steps):
  - `RadiativeCooling` — `T -= k·(T − space)` each tick (**T is now a state variable,
    not a `(1-k)^step` formula**). `k` is composition-modulated (radiogenic K/U).
  - `MoltenConvection` — one convection pass on the persisted `heat` field; vigor ∝ live `T`.
  - `Differentiation` — incremental crust hardening; rate ∝ live `T` (freezes as it cools).
  - `PlateDrift` (+ orogeny, weathering) — one drift step on the convection flow.
  - later: `Outgassing`, `WaterDelivery`, `WaterCycle`, `Mineralization`, `Erosion`, …
- **Evaluation** — a condition checked at the end of a tick; when it fires it activates
  process(es). Adding a process to a phase = adding its evaluation to the pipeline tail.
  We expect **many** — every tipping point is one:
  - *molten motion has ordered materials enough* → `Differentiation`/crust-hardening
  - *crust below solidus **and** a coherent lid* → `PlateDrift` (tectonics begins)
  - *surface cooled below boiling* → `WaterDelivery` + `WaterCycle`
  - *interior degassed past a volatile point* → `Outgassing`
  - … reverse-engineered so outcomes land Earth-like (the guardrails), emergent within.

## 3. History = deterministic re-sim (the locked scrubbing model)

The sim runs **purely forward**; it stores **no per-tick history**. To show tick `N`
(scrub or play), `state_at(N)` re-runs the tick loop from the seed — or from the
**nearest cached checkpoint** ≤ `N` — forward to `N`. Checkpoints are cached at intervals,
so scrubbing back only re-sims a short span. This is the existing forward-regenerative
cache, moved from epoch granularity to tick granularity: same determinism guarantee
(a tick is a pure function of the prior tick + the fixed process/evaluation registry),
low memory, no stored history. The runtime batch server just ticks forward and never
scrubs; re-sim is a viewer affordance.

## 4. What migrates

- **Reused as-is (become processes/evaluations):** every physics kernel (convection,
  drift/orogeny, water cycle, veins, erosion), the cooling math, and the onset
  conditions (`coherent_lid`, below-solidus). Kernels written as `for _ in 0..steps`
  loops get a **single-step form** exposed; the World holds the per-step state (the
  `heat` field, the drift phase, the partition) so ticking is stateful and stateless
  re-seeding disappears.
- **Replaced:** the `WorldEngine` epoch-snapshot cache + `run_epoch` dispatch + the
  `cool_step_before(e)` fixed boundaries → the `Simulation` tick loop + evaluations.
  The viewer's `advance_play` cursor (right shape) drives `Simulation::state_at` instead
  of scrubbing precomputed epochs.

## 5. Build order

1. **This slice — the core + the molten→tectonics phase**, headless + tested:
   `World` / `Process` / `Evaluation` / `Simulation` (tick + `state_at` + checkpoints);
   `RadiativeCooling` + `MoltenConvection` + `Differentiation` active from tick 0; the
   below-solidus + firm-lid **tipping** activating `PlateDrift`. Proves tick → evaluate →
   activate → accumulate + deterministic re-sim on the part already condition-driven.
2. **[DONE] Wire the viewer** to `Simulation::state_at` (`Alpha/flicker-pocepochs`): the
   timeline playhead → tick, deterministic-re-sim play/scrub, the globe drawn from
   `World.cells`, the phase + `T` readout, and **emergent** tipping markers (drawn where
   the evaluations actually fired — `Simulation::tippings`). The timeline is a fixed tick
   window (the sim is infinite). Hydro/Veins views are gone until their processes migrate.
3. **Migrate the later processes** — water-delivery, outgassing, moon/tilt/seasons,
   chemistry, mineralization, erosion — each a process + its evaluation, one at a time
   (each re-adds its view). Every new evaluation drops another emergent marker on the
   timeline; nothing else in the loop changes.

## 6. Invariants carried over
- Conservation: processes move the conserved element ledger only by add/remove/transfer;
  derived fields (crust thickness, elevation, heat) are re-derived, never a mass sink.
- Determinism: no wall-clock / RNG inside a tick beyond the seed; `state_at(N)` is stable.
- Earth-like guardrails constrain the tipping thresholds; the run is emergent within them.
