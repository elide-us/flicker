# flicker-world — Epoch Redesign Manifesto ("the last time")

**Status:** design locked (2026-07-07). **Slice-1 headless core LANDED & verified (2026-07-08)** —
the forward-regenerative engine (`crates/flicker-worldengine`), the `.epoch` file format, content-data
levers, E1-E6 iterative steppers (E4 = the water cycle + gaseous erosion), E7-9 stubs, the
all-elements-form-a-node guarantee, and conservation. See
`docs/flicker-world-epoch-redesign-slice1-handoff.md`. Next: the flicker-shell viewer + the Prism
compounds import (blocked on repo access).
This is the north-star for rebuilding `flicker-world` as the definitive planet-evolution
simulation. It supersedes the *execution model* of the current one-shot epoch pipeline
(`flicker-worldgen`) while **keeping its real physics**. It does NOT relitigate the
locked spatial/data invariants (§8).

> **One line:** a **forward-regenerative, time-stepped 9-epoch simulation** on the
> icosahedron hex-world that plays *Earth backwards to design its levers, then runs
> forward with creative variance* — a callable library that grows Earth-*like* worlds
> with interesting geometry.

---

## 1. Mission

Build `flicker-world` into a **headless world-generator library** (plus a thin viewer
client) that takes a planet's **bulk composition + a seed + per-epoch controls** and
evolves it across **~4.5 billion years in 9 epochs** into a materialized, layered,
Earth-like world. The generator is **callable** — a game (or a caller generating the
"seven Home worlds") invokes it with different inputs; the crate itself knows nothing
about how many worlds or which celestial system produced the composition.

## 2. Philosophy — the load-bearing "why"

- **Play Earth backwards to design the levers; run forward with variance.** We
  reverse-engineer Earth's *end state* to choose each epoch's controls and their sane
  default ranges. We do **not** literally reverse-simulate or hard-clamp to targets. The
  sim runs **forward and emergent**; creative variance on the reverse-engineered levers
  wiggles it into Earth-*like* worlds with interesting zones. **Reality-LIKE, not
  reality-accurate.**
- **The real physics are the game.** We salvage the correct, expensive physics we already
  built (isostasy, plates, hydrosphere from the H/O budget, ore veins, biomes) — but we
  **were not using the tools correctly before.** This time we **refactor them to iterate**:
  run them as many-step accumulating simulations, not one-shot transforms.
- **Simulate, don't paint.** Every epoch **moves conserved material** (transport /
  advection / deposition) — never crossfades cosmetic textures. Swirl, strata, and
  continents *emerge from the transport*. Conserve to the gram.

## 3. Engine architecture — the forward-regenerative pipeline

Each epoch is a **pure, deterministic function** that runs a many-step sim internally and
emits an **immutable snapshot**:

```
snapshot[n] = epoch[n].run( snapshot[n-1], vars[n], seed[n] )
              └─ "run" = iterate many steps (count ∝ the epoch's duration) to accumulate traits
```

- The pipeline is a **build-graph of immutable per-epoch snapshots** (the cache substrate).
- **Replay-forward mechanic (locked):** editing `vars[n]` (or `seed[n]`) **invalidates
  `snapshot[n..9]` and recomputes forward**; `snapshot[0..n-1]` stay frozen. Tweak Epoch 1
  → reseed & regenerate the whole chain; tweak a late epoch → only re-run from there. The
  epoch you edit *does* re-run (its own output changes); everything upstream is untouched.
- **The viewer** is a thin client: a **timeline slider** scrubs the 9 epochs; each phase
  exposes its own controls (seed, inputs, meta-vars, physical drivers). Regeneration is
  live — dynamic as you drag.
- **Determinism is mandatory** (it's what makes caching + replay work): an epoch's output
  is a pure function of `(prev snapshot, vars, seed)`. Snapshots are serializable so a
  finished world can be captured/handed off.

## 4. The nine epochs — three groups, ~1.5 BY each

Salvaged-from → the current `flicker-worldgen` epoch whose *physics* we refactor into the
iterative stepper. New capability called out where the old code doesn't have it.

**Group I — Liquid / Molten (Epochs 1–3, ~1.5 BY) · hex-scale**
1. **Composition scatter** — bulk composition scattered spatially over the icosphere
   (heavy→equator, volatile→pole, correlated fBm), per-world seed. *(salvage E1)*
2. **Molten liquid dynamics** — silicates *flowing / convecting / cycling like hurricanes*;
   density differentiation; first crust firming. **New: genuine convective material
   transport on the sphere** (not a one-shot density sort). *(salvage E2 + real advection)*
3. **Plates, cracks, veins** — plates firm and move; faults/cracks open; elements sort into
   **large veins**. **New: iterated plate motion over time.** *(salvage E3, made iterative)*

**Group II — Water / Life / Crust (Epochs 4–6, ~1.5 BY) · hex-scale**
4. **Outgassing → atmosphere + oceans** — the **water cycle joins here** (the
   `flicker-pocepochs` `layers.rs` nucleus, re-homed to the sphere); **early-atmosphere
   erosion** begins. *(salvage E4 + integrate the conserved water cycle)*
5. **Mineralization** — hydrothermal signature, ore-vein tracing along faults, microbial
   cradles at vents. *(salvage E5)*
6. **Life / carbonation / erosion / biomes** — **simulated significantly longer than
   before** (was seconds): hydraulic + thermal erosion, carbonate deposition (Dover
   precursor), organics (coal/oil precursor), Whittaker biomes → **rich sub-layers of shape
   and detail on the crust from below.** *(salvage E6, greatly expanded duration/iteration)*

**Group III — Strata / Heightmap (Epochs 7–9, ~1.5 BY) · heightmap-scale · DEFERRED**
7–9. **The IVoxelLayer material-band stack.** *Literal* erosion simulations on **actual 2K
   per-hex heightmaps** (accepting the data bloom — hundreds of 2K textures). Continental
   plate layers under the **deterministic-material rules**; **epeirogenic uplift + incision**
   expose strata (Grand-Canyon layer cake); thick carbonate bands (White Cliffs of Dover).
   **Aim ~5–6 layers**; their thickness and where bands form are **procedurally grown from
   the cycles** of this final 1.5 BY → roughly modern Earth. Feeds the per-cluster **2048²
   materialization bridge** (`material-model-impl-handoff.md`) → voxel clusters.
   *(New first-class epochs; the deferred "stratigraphy / column strata-stack" of
   `worldgen-timeline-system` item 5.)*

## 5. Two scales (resolves the output boundary)

- **Epochs 1–6 = hex-scale** — per-cell icosphere state (`HexState`-like), time-stepped.
  **This is the entire first implementation iteration.**
- **Epochs 7–9 = heightmap-scale** — 2K per-hex textures, literal erosion, the strata stack
  + the data-volume bloom. **Deferred to a later iteration.**

## 6. Inputs, drivers, seven worlds

- **Input:** bulk composition (which Prism elements, how much) + a **per-world seed** for
  the spatial scatter and per-epoch randomness. The bulk composition comes from an
  **upstream celestial sim that is OUT OF SCOPE here** — this crate is the callee.
- **Physical drivers, realistic per epoch:** material motion, heat (insolation), rotation,
  axial tilt, oceans, atmosphere — each surfaced as a lever where it matters.
- **The "seven Home worlds"** are simply seven **calls** to the library with different
  inputs — a *caller* concern, not the crate's. (Not modeled inside `flicker-world`.)

## 7. Salvage / refactor / out

- **Salvage (physics kept, execution refactored to iterate):** `flicker-worldgen`'s epoch
  physics (E1–E6); `flicker-pocepochs/layers.rs` conserved water cycle; `flicker-worldgrid`
  icosphere topology; `flicker-materials` tables; `flicker-worldstate` `Composition`/`Ledger`;
  `flicker-world`'s icosphere globe build + timeline UI (the viewer base).
- **Refactor:** one-shot transforms → **iterative, forward-regenerative steppers** with
  per-epoch immutable snapshots. Split `flicker-world` into **lib (generator) + thin viewer**.
- **Out / abandoned:** random solar-system generation (celestial is specific & upstream);
  the one-shot execution model; `flicker-celestial` (abandoned — its per-body-evolution
  *concept* is revived here as new code, not resurrected).

## 8. Invariants (do NOT relitigate)

Data is truth, **shape is disposable**; **conserve mass to the gram**; **absolute element
amounts, not densities**; **equal-area icosahedral hex cells** (ISEA — pending); **one
continuous planet** (blend per-cell state across neighbours, not per-hex noise); every epoch
**visually continues** the prior; **simulate-not-paint**; **emergent within an epoch**, only
the *lever defaults* are reverse-engineered from Earth; **reality-LIKE, not accurate**.

## 9. First slice (next session) + roadmap

**First slice — the skeleton that proves the model:**
1. The **forward-regenerative engine** + per-epoch **immutable snapshot cache** + the
   replay-forward invalidation.
2. The **timeline UI** wired to it (scrub epochs; per-epoch control panels; live regen).
3. **Epochs 1–6 as iterative hex-scale steppers** — physics salvaged/refactored from
   `flicker-worldgen`, run as accumulating multi-step sims; water cycle joined at E4.
4. Epochs **7–9 stubbed** (heightmap strata = the next iteration).

**Later:** Group III heightmap strata + the 2048² materialization bridge; ISEA equal-area
projection; ledger `CellId↔CellCoord`; cross-hex vein paths; onset-event layer
(great-oxygenation, mass extinctions), moon-forming impact.

## 10. Source notes this builds on

- **Docs:** `clayengine_world_generation_spec_v2.md` (epoch spec), `flicker-world-system-spec.md`,
  `flicker-sol2-epoch3-pipeline-roadmap.md` (Phase-3 / IVoxelLayer / epoch renumbering),
  `material-model-impl-handoff.md` (the 2048² materialization bridge — the overarching goal),
  `biosphere-epoch-handoff.md` (Dover, within-epoch cycles), `water-cycle-handoff.md`
  (`layers.rs` nucleus), `epoch-pipeline-review.md` / `epoch-data-audit-handoff.md`
  (the "one-shot, not time-stepped" reality we're now changing).
- **Memory:** `epochs-are-per-body-evolution` (epochs = evolution stages), `system-sim-epoch3-cutoff`
  (formation ends at E3), `worldgen-timeline-system` (duration/onset clock + stratigraphy item 5),
  `celestial-evolution-simulate-not-paint`, `less-code-every-calculation-counts`.
