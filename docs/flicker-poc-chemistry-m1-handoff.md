# flicker-poc-chemistry — M1 interior handoff

**Landed 2026-07-12.** The second slice of the chemistry-first rewrite (build spec
*flicker-poc-chemistry — Build Specification* §10 "M1 — Interior"; M0 handoff:
`docs/flicker-poc-chemistry-m0-handoff.md`). Direction of record:
[[chemistry-first-rewrite]] in memory.

> **M1's required output (spec §10):** *the planet differentiates* — a metallic
> core and a silicate mantle separate from the undifferentiated bulk seed. It does,
> and the ~31%-of-planet, iron-dominated core is **emergent** from partition
> coefficients — nothing hardcodes "a third."

## What landed
The M0 global mantle `Composition` became a **per-cell field**, and four interior
stages run on it.

| Piece | File | What it does |
|---|---|---|
| `MantleField` | `mantle.rs` | Dense SoA over the 92,162 cells (per-cell element mass + `temp_k` + `velocity` + `differentiation`). ~20 MB, cache-friendly — the correct technique for a field ticked repeatedly, not 92k `BTreeMap`s. Cached per-element `totals` → O(1) audit. Seeded homogeneously (budget/N, remainder in the last cell; cached totals = exact budget). |
| `RadiogenicDecay` | `interior.rs` | Heat from ²³⁸U/²³⁵U/⁴⁰K decay curves (real half-lives + formation-time isotope fractions). The young planet runs hotter because the isotopes hadn't decayed — it falls out of the curve, not a constant. **Th is absent from the 28-element Prism table** (noted; would need a Book ruling), so early heat is a bit below Earth — a correct consequence of the element set. Changes temperature only. |
| `CoreFormation` | `interior.rs` | The iron catastrophe: while a cell is molten (T ≥ 1800 K — a **chemistry** gate, not a clock), siderophiles (Fe/Ni/Co/S/Cu/P/Cr/Pt/Au/Ag) drain from the mantle cell into the global core by **partition coefficient**, toward target `m0·(1−d·φ)`. Rate scales with temperature, so it **sweeps out from the hot upwellings**. Mass moves mantle→core, conserved. |
| `MantleConvection` | `interior.rs` | Surface velocity ∝ −∇T (diverges over hot upwellings, converges over cold downwellings); advects temperature by the **semi-Lagrangian resample** (§6.1) — a convex upstream average, drift = `velocity·dt` clamped to CFL. Changes temperature/velocity only. |
| `PlateKinematics` | `plate.rs` | Plates **emerge** as coherent-velocity domains (union-find over adjacency); the count falls out of the convection pattern (no hard-coded 8). Assigns `column.plate_id` + records each plate's drift. Moves no mass. |

The viewer (`scene.rs`): **Space** plays, **V** cycles temperature / core-formation
progress / plates / shards, **R** reseeds a new planet, **Down** restarts this one.
HUD shows core %, differentiated %, mantle temp, plate count, radiogenic TW, and the
live conservation ledger (core growing out of the mantle). `cargo run -p flicker-poc-chemistry`.

## Invariants proven (28 lib tests)
- **Conservation every tick** — the audit runs after every stage; all four M1 stages
  conserve (radiogenic/convection/plates move no mass; core formation debits==credits).
  The seed holds the whole budget exactly.
- **The planet differentiates** — after ~120 ticks the core is 25–40% of the planet
  and >80% iron; the mantle is iron-depleted vs the bulk seed. Emergent, not targeted.
- **Semi-Lagrangian smoothness (§6.1)** — `temperature_stays_smooth_through_convection`:
  the advected field stays bounded (a scatter would blow it up 30–50×). The M1 analogue
  of `relief_stays_smooth_through_plate_drift`.
- **Determinism (§11)** — `the_full_interior_run_is_deterministic` runs the FULL
  pipeline (incl. the neighbour-summing convection + plate stages) twice and hashes the
  entire world (temperature field, every core element, differentiation, plate ids);
  same seed → identical hash, different seed → different world.
- **Radiogenic decline** — younger = hotter, from the decay curve.
- **Plates partition every cell** into an emergent (>1, <N) number of domains.

## Verification
Full workspace green; clippy clean. An adversarial multi-lens review (6 spec-dimension
finders → independent refute-or-confirm, 21 agents) found **no major bugs** — the
verifiers confirmed conservation, determinism, and emergent differentiation hold in
production. Its survivors were all minor/nit and are addressed:
- Determinism test strengthened to the full-pipeline world-hash (was 2 stages + a scalar).
- HUD ledger `expected` now includes `delivered` (was a latent false-BROKEN at M3).
- Convection drift now scales with `dt` (consistent under the future adaptive tick);
  the resample regulariser scales with cell spacing².
- Seed doc precision (cached total exact; array sum exact-to-rounding).
- Differentiation rate made temperature-dependent (also a review nit — was a fixed rate).

## Deliberately NOT done (still honoured)
- No lateral **composition** advection yet — M1 advects only the intensive temperature
  (keeps conservation exact); the conservative mass-transport scheme lands at M2 (crust
  drift), where the `budget/N` differentiation reference is also revisited.
- The 3 radial mantle shells (§5.1) are collapsed to one well-mixed cell per column —
  M1's output doesn't need radial resolution.
- `flicker-worldgen`/`flicker-worldengine` still un-deleted (the app + pocepochs depend
  on worldgen). M-1 (ISEA equal-area) still pending — areal-independent, so M0/M1 stand.

## Next (spec ladder)
**M2 — Crust:** MOR crust generation, subduction, **Airy isostasy** (absolute, never
rank — §6.2), relief. Output: a bimodal hypsometric curve and an Earth-like crustal
assay, *grown* not seeded. The conservative semi-Lagrangian mass transport (crust drift)
and the `relief_stays_smooth_through_plate_drift` regression land here.
