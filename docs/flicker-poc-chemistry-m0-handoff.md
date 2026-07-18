# flicker-poc-chemistry — M0 foundation handoff

**Landed 2026-07-12.** First slice of the chemistry-first world-gen rewrite
(build spec: *flicker-poc-chemistry — Build Specification*, reconciled 2026-07-11).
This crate is a **rewrite, not a refactor**, and it **supersedes the
`flicker-world-epoch-redesign` manifesto** (confirmed by the user, 2026-07-12).

> **Thesis:** *Simulate the chemistry; everything else is derived.* The planet
> starts as a **bulk accretion budget** — an undifferentiated hot ball — and every
> feature (core, mantle, crust, ocean, ore) is an **output**, never a seed.
> Earth-likeness is an outcome, never a target.

## Why the rewrite (the one-paragraph version)
The old `abundance.json` seeded the planet with **Earth's crustal assay** (46% O,
5% Fe, Mg only as a floor). That is *the answer fed in as the input*: you cannot
differentiate a metallic core out of 5% iron, nor convect a mantle out of trace
magnesium. The new `accretion.json` seeds a **bulk planet** (32% Fe, 30% O, 15%
Si, 14% Mg) and makes crust an emergent output + a success criterion.

## What M0 delivered (spec §10 "M0 — Ledger + harness")
- **Data (shared `Alpha/content/data/`):**
  - `accretion.json` — the bulk-planet seed (28 elements, mass-% summing to 100,
    `planet_mass_kg = 5.972e24`). Replaces `abundance.json` as the seed (the old
    crustal numbers survive only as an *acceptance test* for the grown crust).
  - `rocks.json` — the missing **Element → Mineral → Rock** tier (12 sim-required
    minerals: olivine/pyroxene/plagioclase/micas/serpentine/pyrite/…; 19 rocks with
    an *erosional-resistance* ladder). **Authored but not yet consumed** — the rock
    classifier + `CompoundDef` hardness/density fields are **M6**.
  - `periodic_table.json` — **Li (Z=3) synced** → 28 elements (Book III ruling
    F3450870). Count-assertion consumers updated: `flicker-materials`,
    `flicker-system` (`loads_all_28_elements`).
- **New crate `crates/flicker-poc-chemistry`** — GPU-free sim library + a thin
  flicker-shell app (structured like `flicker-pocepochs`, per the user's ask):
  loading screen → globe → on-screen conservation ledger, steppable a tick at a
  time.

### Crate layout
| File | Role |
|---|---|
| `src/config.rs` | Invariants: `PLANET_FREQ = 96` (92,162 cells — **no size slider**), `PLANET_MASS_KG`, `CELL_AREA_M2`, content-dir seam. |
| `src/budget.rs` | Immutable `Budget` (bulk seed → absolute kg; asserts Σ≈100 and every symbol resolves — a missing Li errors loudly). |
| `src/reservoir.rs` | `Reservoirs` (core/mantle/atmosphere/ocean/delivered/escaped) — all `Composition`s. Ocean = its element content (no geometry, §4.5). |
| `src/column.rs` | `Column`/`Layer` (composition + order only) + **derived classifiers as free functions** (`crust_kind`, `elevation_m`, `thickness_m`, `density_kg_m3`). |
| `src/planet.rs` | `World` (budget + reservoirs + columns + grid) + `PlanetState` (top-of-tick aggregate) + **the conservation harness** (`audit` + `audit_compound_bound`). |
| `src/stage.rs` | `Stage` trait (gates on chemistry, never the tick number) + deterministic `StageRng` (SplitMix64, per-stage streams). |
| `src/scheduler.rs` | `Scheduler::step()` (sample → run live stages → audit both ledgers) + worker-pool cell sweep + `CellProgress`. |
| `src/{main,scene,camera,globe}.rs` | The flicker-shell app (bin target). |

### Invariants proven (13 lib tests, `cargo test -p flicker-poc-chemistry`)
- **Element conservation (§4.3):** `Σ(reservoirs + columns) + Escaped == accreted + Delivered`, to 1e-9 relative, every tick. A deliberately-**leaking stage panics naming the stage**; a raw leak is caught; **element creation** (an unbudgeted species) is caught (the tracker scans reservoirs + columns, not just the budget); delivery adds to both sides.
- **Compound-ledger bound (§4.1):** `Σ mineral-element-mass ≤ free element mass`, run every tick alongside the element audit; a `should_panic` test forms an over-budget mineral to prove it fires. Vacuous at M0 (no minerals) but *enforced*, not dead code.
- **Derived-never-stored (§9):** `elevation`/`crust_kind`/`thickness`/`density`/`hardness` are functions, never fields (`relief_m` is the only stored sub-hex scalar). No `NodeSpawner`/`place_ore`/`ensure_ore_veins` anywhere. Exactly **two** conserved ledgers.
- **Determinism (§11):** same `(seed, stage index)` → identical RNG stream; the worker sweep never mutates world state; no wall-clock/RNG entropy.

### Verification
`cargo build --workspace`, `cargo clippy` clean; full workspace test suite green.
An adversarial multi-lens review (5 spec-dimension finders → refute-or-confirm per
finding) ran against the M0 hard gates: **4/5 dimensions clean, 0 false positives.**
Its 2 confirmed findings were fixed here (the compound-bound audit was dead code →
now wired every tick + tested; the debug-only leak proof → annotated to skip under
`--release`).

## Deliberately NOT done (scope leashes honoured)
- **`flicker-worldgen` / `flicker-worldengine` NOT deleted.** The spec says delete
  them, but `flicker-world` (the app) and `Alpha/flicker-pocepochs` still depend on
  `flicker-worldgen`; deletion has to wait until the new crate can stand in. They
  remain, untouched.
- **`flicker-system` / `flicker-sol2` not built on** (only a broken count-assertion
  test updated for the Li sync).
- No M1+ physics (transport, isostasy, plates, real stages) — the spec forbids
  faking it.

## Next steps (spec milestone ladder — the user prioritises)
- **M-1 — ISEA equal-area (worldgrid Slice 3b).** *Still pending, still blocking
  for meaningful areal results.* `worldgrid/sphere.rs` uses the cheap projection
  (hex area spread ~1.75×); the spec wants `<1.05`. Mass conservation (M0) is
  areal-independent, so M0 stands, but `thickness`/weathering/submerged-fraction
  inherit the error until this lands.
- **M1 — Interior:** radiogenic heat → mantle convection → plate kinematics →
  **the planet differentiates** (core + mantle separate from the bulk seed).
  Semi-Lagrangian transport (§6.1 — *resample, never scatter*) from the first line;
  the `relief_stays_smooth_through_plate_drift` roughness regression test lands here.
- **M6** consumes `rocks.json` (mineral former, rock classifier, `CompoundDef`
  hardness/density/brittleness).

Run it: `cargo run -p flicker-poc-chemistry`. Controls: **Space** step/play ·
**Down** reset · **V** view (hot ball ↔ shards) · drag rotate · wheel zoom · **Esc** menu.
