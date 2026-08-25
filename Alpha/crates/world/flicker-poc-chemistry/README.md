# flicker-poc-chemistry

The engine's **planet-formation simulation**: a bulk ball of elements in, a
finished world — core, mantle, crust, oceans, an atmosphere, ore, life's
leavings — out, grown one geological tick at a time. Its founding rule is
*simulate the chemistry; everything else is derived*: the planet begins as a
bulk **accretion budget** (an undifferentiated hot ball) and every feature is an
*output* of mass-conserving processes acting on it, never a seed. Earth-likeness
is an outcome, never a target — a run that ends a Venus or a Mars is a correct
run.

It is a **headless, GPU-free library**. It owns no window, no mesh, no input; it
runs a `World` forward and answers questions about it. Its consumers put a face
on it: `flicker-godmode` (the God Mode viewer), `flicker-worldtile` (the
per-cell pixel tier).

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

**flicker words used throughout, defined once:**

- **budget** — the immutable starting inventory: absolute kg of each element the
  planet accreted ([`Budget`], from `accretion.json`). The right-hand side of
  the conservation law.
- **reservoir** — a global store conserved matter lives in: core, atmosphere,
  ocean, plus the boundary terms `delivered` (arrived from space) and `escaped`
  (lost to space) ([`Reservoirs`]).
- **column** — one hex cell's vertical rock stack ([`Column`]); a **layer** /
  **bed** is one deposited slab within it ([`Layer`]). One column per cell,
  92,162 cells at the reference size.
- **the two ledgers** — the only two conserved quantities: elements
  (`Composition`) and the minerals those elements are bound into
  (`CompoundLedger`), both from [`flicker-worldstate`](../flicker-worldstate).
  Rock is *derived* from them, never a third store.
- **stage / transformation** — one mass-conserving step of the sim ([`Stage`]):
  it moves mass between reservoirs/columns (or debits `escaped` / credits
  `delivered`) and never creates or destroys it.
- **gate** — a condition over the planet's own solved state, measured every tick,
  that decides whether a stage runs. Gates read *chemistry, never the clock*.
- **lever** — a maintainer-set input or rate ([`Levers`]): set a *condition* or a
  *pace*, never a result.
- **observer / classifier** — a **read-only** pass that *annotates* the world
  (plates, ore prospects, weather, habitability) without moving any mass.
- **size_scale** — how big this grid's planet is against the reference (freq-96)
  planet; every absolute in the crate is a reference value carried to other grid
  sizes by this factor.

---

## Where it sits

**Cluster:** `Alpha/crates/world` (the world-sim crates).

- **Builds on:**
  - [`flicker-worldgrid`](../flicker-worldgrid) — the icosphere topology
    (`Sphere`, `icosphere`): the cell lattice and per-cell neighbours every
    per-cell read walks.
  - [`flicker-worldstate`](../flicker-worldstate) — the two conserved ledgers
    (`Composition`, `CompoundLedger`) reservoirs, columns and the mantle field
    are built from.
  - [`flicker-materials`](../../content/flicker-materials) — `Tables`: the
    periodic table, compound catalog and rock catalog (loaded from the content
    tree), used to resolve element symbols, mineral stoichiometry, gas species
    and rock resistances.
  - [`flicker-worker`](../../core/flicker-worker) — the `WorkerPool` behind the
    per-cell sweep.
- **Used by:**
  - [`flicker-godmode`](../../scenes/flicker-godmode) — the God Mode bench. Runs
    this crate on its **own thread** (`sim_thread.rs`), sends it commands, reads
    the latest `Snapshot`, and draws the result. **The reference consumer** — read
    it to see the whole API driven in anger.
  - [`flicker-worldtile`](../flicker-worldtile) — materializes one cell of a
    finished world into a 2048² heightmap tile (the "pixel tier"); re-exports
    [`TILE_SPAN_M`] rather than deriving its own span.
  - `flicker-worldgen` — the **legacy** 9-epoch pipeline (`epoch3.rs`) still
    links this crate; it is superseded by the chemistry-first rewrite and being
    retired. Do not build new work on it.
- **Reads from the content tree** (`Alpha/content/data/`, resolved by
  [`content_data_dir`]):
  | File | When it is read | If it is missing / malformed |
  |---|---|---|
  | `accretion.json` | [`Budget::from_repo`]/[`from_dir`](Budget::from_dir) at world setup | loud `BudgetError` — a symbol absent from the periodic table, or weights not summing to 100, aborts; there is no silent floor |
  | `processes.json` | [`formation_stages`] → [`load_processes`] at pipeline build | **panics** — missing file, parse error, empty roster, an unknown gate `read`/`op`, or a `runs` naming no registered stage all abort loudly |
  | `periodic_table.json`, `rocks.json` | via `flicker-materials` `Tables` | `Tables` load error (the crate never reaches a world) |

  This crate does **not** read `abundance.json` — that is the legacy
  worldgen/worldengine crustal seed, read only by those crates.

---

## Rendering the output — reuse `flicker-globe`, never a bespoke mesh

This crate draws nothing. **The one canonical layer/planet/interior renderer is
[`flicker-globe`](../../frontend/flicker-globe/README.md)** — its `GlobeWorld`
plus `ShellSpec`. Any view of a planet or its interior **must** reuse it; writing
a new mesh for a layer view is a ratified violation (MCP rule **55644181**).

The seam is a **shell list**. A `ShellSpec` is one sparse shell — a tiling, a
radius, a per-cell colour where `color(i) -> None` leaves a hole (so a crust
shell exists only where there is crust), and an optional per-column `cell_radius`
so a shell can follow each stack's own height. You build one `ShellSpec` per
layer from *this crate's* reads and hand the list to `GlobeWorld::set_shells`;
`flicker-globe` owns the mesh, the camera and the offscreen plumbing. This crate
supplies **what the planet is made of**, never how it is drawn:

- [`elevation_field`] / [`sea_level_m`] — surface height and where the sea stands;
- [`crust_kind`], [`density_kg_m3`], [`enrichment`] — per-column classifiers a
  colourer reads;
- [`air_shells`] — the atmosphere as a stack of gas veils ([`AirShell`]);
- [`PlateObserver::observe`] — per-cell plate labels and seam classes for overlays.

`flicker-godmode` is the worked example: it publishes N sparse shells (core,
mantle, two crust beds, a veil per gas) rebuilt each time the sim advances.

> **Heads-up (see Findings #1):** rule 55644181's own text still cites the
> renderer as `flicker-poc-chemistry/src/globe.rs::build_shell`. That file no
> longer exists — the renderer consolidated into `flicker-globe`. The *principle*
> stands; only the citation drifted.

---

## How a world is built (the 60-second model)

```text
Budget (accretion.json)                     ← the immutable element inventory
   │  World::seed(grid, budget, tables, seed)
   ▼
World  = reservoirs + per-cell MantleField + one Column per cell + grid
   │  Scheduler::new(formation_stages(tables, &world, &levers), seed)
   ▼
loop:  Scheduler::step(&mut world, dt_myr, progress)
        1. sample PlanetState  (cheap top-of-tick aggregate; stages read THIS)
        2. for each stage in processes.json order:
              run it  IF  not held  AND  its gate holds(state, levers)
              → audit + audit_compound_bound   (panic naming the stage on a leak)
        3. settle air species; tick_myr += dt_myr
```

One tick is **~1.6 Myr** — the time for the ground to move one hex at plate speed
([`NOMINAL_DT_MYR`], derived, never typed in). A planet bakes over ~2,800 ticks.
Derived properties (elevation, crust kind, thickness, density, sea level) are
**functions of the ledger, never stored fields** — recomputed on demand.

Minimal real driver (from `flicker-worldtile`'s tests):

```rust
let tables = Tables::from_source(&JsonTableSource::new(content_data_dir()))?;
let budget = Budget::from_dir(&content_data_dir(), &tables)?;
let mut world = World::seed(icosphere(freq), budget, &tables, seed);
let mut sched = Scheduler::new(
    formation_stages(Arc::clone(&tables), &world, &Levers::default()),
    seed,
);
for _ in 0..n_ticks {
    sched.step(&mut world, NOMINAL_DT_MYR, None);
}
let sea = sea_level_m(&world);        // a derived read of the finished world
```

---

## Public API

### Setup & the loop

| Item | What it is for | The one thing to know |
|---|---|---|
| [`Budget`] · [`Budget::from_repo`] / [`from_dir`](Budget::from_dir) | Load the immutable accretion seed | Absolute kg per element; **immutable once built** — re-endow with [`rescaled`](Budget::rescaled) *before* seeding, never after |
| [`Budget::rescaled`] | A re-endowed copy (the Starter's per-element knobs) | The planet mass *follows* the elements — triple the iron and the world is heavier |
| [`World`] · [`World::seed`] | The whole mutable sim state; seed an undifferentiated hot ball | `seed` sizes the budget `× size_scale³` to the grid at the one seam — callers pass the reference budget |
| [`PlanetState`] · [`PlanetState::sample`] | The cheap global aggregate stages read | **Lags one tick by construction** (sampled top-of-tick) so read/write ordering is unambiguous; stages never write it |
| [`Scheduler`] · [`Scheduler::new`] · [`step`](Scheduler::step) | The observable, steppable formation loop | `step` runs the audit after every stage in debug/test, every 100 ticks in release, and never fewer than once per tick |
| [`Scheduler::set_held`] / [`is_held`](Scheduler::is_held) / [`processes`](Scheduler::processes) | Hold a stage; read what every process is doing | A *held* stage and a *gate-closed* stage both simply "did not run" — the difference is only in the readout ([`ProcessState`]) |
| [`Scheduler::sweep`] · [`CellProgress`] | Fan a per-cell progress pass across the worker pool | Synchronous; sends are best-effort (a dropped receiver just ends reporting) |
| [`formation_stages`] | **Build the production pipeline** from `processes.json` | The one seam where levers meet a world — sizes the kg levers here so gates and stages measure in one frame |
| [`interior_stages`] | The fixed M1 interior triplet (radiogenic → core → convection) | **Reference/test only** — used by the determinism gate; production uses [`formation_stages`], not this |
| [`Stage`] · [`StageRng`] | The transformation trait; a deterministic per-stage RNG stream | `is_live` gates on chemistry, never the tick; one master seed splits into an independent stream per stage → same seed, identical world |
| [`Levers`] · [`sized`](Levers::sized) · [`brisk`](Levers::brisk) | The maintainer's dials | See **The levers** below; `brisk` is a *test fixture* (fast rates), not a preset the app uses |

### The conserved ledgers & the harness

| Item | What it is for | The one thing to know |
|---|---|---|
| [`World::audit`] | The conservation harness: `present == accreted + delivered` for every element | Panics naming the offending **stage** and element; tolerance 1e-9 relative; never disabled |
| [`World::audit_compound_bound`] | The second-ledger bound: minerals/gas species ≤ the free elements backing them | Panics naming the stage; compounds are an *accounting of* the element budget, not new matter |
| [`World::present_mass`] / [`expected_mass`](World::expected_mass) | The two sides of the invariant, per element | `present` = reservoirs + mantle + columns + escaped; `expected` = accreted + delivered |
| [`Reservoirs`] · [`Ocean`] · [`Air`] | The global stores (core/atmosphere/ocean/delivered/escaped) | `Ocean`/`Air` mass is **derived** from element content, never stored separately; the mantle is *not* here — it is per-cell |
| [`MantleField`] | The per-cell interior (element mass, temperature, velocity, differentiation) | Dense struct-of-arrays over 92k cells with a cached per-element total → O(1) audit; mass moves only via `add`/`remove` |
| [`Column`] · [`Layer`] · [`FormationProcess`] | One cell's rock stack and its beds | A bed records what process made it and the worst pressure/temperature it has seen |

### The pipeline as content (`processes.json`)

| Item | What it is for | The one thing to know |
|---|---|---|
| [`load_processes`] | Read the pipeline roster from the content dir | Loud on every failure (see the content table above) |
| [`ProcessDef`] | One pipeline entry: `runs` · `summary` · `watch` · `view` · `gate` | `runs` must name a registered stage; `view` names a bench view and is validated by the **consumer**, not here (Findings #3) |
| [`Gate`] · [`Gate::holds`] | The gate grammar: `true`/`false`, `all`/`any`, or one `read op value` comparison | Reads a planet-state field or `lever:<name>`; ops are `< <= > >=`; **any unknown read/op panics** — a gate cannot fail to nothing |
| [`Gated`] | A registered stage wrapped in its authored gate | The stage supplies the physics, the file supplies the condition; the pipeline holds only these |

The gate `read` vocabulary (kept in sync with `processes.json`'s `_meta.reads`):
`mean_mantle_temp_k · min_mantle_temp_k · max_mantle_temp_k · differentiation_frac
· crust_frac · continental_frac · lid_frac · mean_elevation_m · sea_level_m ·
submerged_frac · ocean_mass_kg · atmosphere_mass_kg · water_vapour_kg ·
delivered_water_kg · p_co2 · greenhouse_k · mean_strata · compounds_kg`, plus
`lever:<any Levers field>`. **Discipline (extreme-not-mean):** a gate over a
per-cell threshold reads the *extreme* that admits work (`max_…` for "anywhere
still hot enough", `min_…` for "anywhere cold enough") — the mean silently stops
a stage while its anomalies still work.

### The transformations (stages `processes.json` may name)

Registered in `build_stage` (lib.rs); the file decides which run, in what order,
behind what gate. Grouped by the pipeline's own order:

| Stage | Transforms | Gated on (per shipped file) |
|---|---|---|
| `RadiogenicDecay` | U/K decay warms the mantle on real half-lives | always |
| `CoreFormation` | The iron catastrophe — siderophiles drain mantle → core | anywhere still `> 1800 K` and not yet differentiated |
| `MantleConvection` | Overturn down the temperature gradient (the flow plates ride) | always |
| `Outgassing` | Hot rock exhales volatiles as real gas compounds | anywhere hotter than the most willing gas (`> 600 K`) |
| `CrustGeneration` | Cooled mantle freezes into mafic sea floor | the **coldest** cell below the solidus |
| `ThermalSubsidence` · `Eclogitisation` · `Delamination` | Crust densifies with age / overburden; over-dense root sheds | chemistry gates in the file |
| `Volcanism` | Plume cells erupt through the lid, venting dissolved gas | a lid exists and a cell is still above the melt floor |
| `WaterDelivery` | Comets rain the water budget in; molten ground flashes it to steam | budget remaining, below the coverage cutoff |
| `WaterCycle` | The sky decides where water stands (evaporate / rain / vapour greenhouse) | chemistry gates in the file |
| `CarbonSink` | A standing sea drinks CO₂ and lays down calcite | chemistry gates in the file |
| `Biosphere` · `Maturation` | Life fixes carbon → tissue → coal/oil; buried organics mature | a temperate, wet, lidded world |
| `LateVeneer` | The late metal veneer arrives (why a planet has minable gold) | after a core exists |
| `Conveyor` | Plates relocate stacks a hex step; collision subducts/thickens | chemistry gates in the file |
| `Hydrothermal` | Circulating fluid leaches metal into veins | chemistry gates in the file |
| `Erosion` · `MassWasting` | Rain wears ranges down; slopes past repose collapse | chemistry gates in the file |
| `Crystallization` · `Metamorphism` · `StrataReconcile` | Free elements organise into minerals; beds reconcile the stack | chemistry gates in the file |

### Derived reads (functions, never fields)

| Item | Answers | The one thing to know |
|---|---|---|
| [`elevation_field`] | The surface height of every column, with lithospheric flexure | Runs 2 flexure relaxation passes **per call** — not cached; the field everything that cares about *shape* should read |
| [`sea_level_m`] | Where the sea stands | **Solved** (bisection on the hypsometry vs the conserved ocean volume), never set; an empty ocean rests on the lowest ground |
| [`p_co2_pa`] | Atmospheric CO₂ partial pressure | The one pressure read, shared so `PlanetState` and habitability can't disagree |
| [`elevation_m`] · [`crust_kind`] · [`crust_thickness_m`] · [`thickness_m`] | Per-column Airy isostasy / classification / thickness | `elevation_m` is one column floating alone; `elevation_field` adds neighbour coupling |
| [`density_kg_m3`] · [`overburden_pa`] · [`basal_pressure_pa`] · [`dissimilarity`] · [`geotherm_k`] | Per-bed/column physical reads | Free functions over a `Layer`/`Column` — take `gravity_m_s2` and `cell_area_m2` explicitly |
| [`greenhouse_k`] · [`bed_resistance`] · [`cell_spacing`] | Sky warming / erosional resistance / hex spacing | `greenhouse_k` reads the *species* ledger, so a thick transparent air warms nothing |

### Observers & classifiers (read-only — causes only, never outcomes)

| Item | What it reads out | The one thing to know |
|---|---|---|
| [`PlateObserver`] · [`PlateObservation`] · [`Seam`] · [`PlateEvent`] · [`PlateRecord`] · [`PlateId`] | Plates as a derived read of the velocity field: labels, seams, birth/death/merge/split | Not a `Stage`; moves no mass, never in the harness. Hysteresis stops the plate count flickering; construct one per planet, `reset` on reseed |
| [`observe_habitability`] · [`Habitability`] · [`Axis`] · [`BANDS`] | Earth-likeness on five axes (interior/surface/atmosphere/hydrosphere/pH) | A gauge, never a gate on the sim — it *observes*, it does not steer |
| [`prospect`] · [`Prospect`] · [`enrichment`] · [`is_playable`] · [`ore_metals`] | Where ore is workable, and whether the world is worth mining | An external validity classifier — reads the finished world, places nothing |
| [`Weather`] · [`WeatherField`] | Per-cell temperature / precipitation derived from the global air | `WeatherField::observe` is a snapshot the erosion stage and views consume |
| [`air_shells`] · [`AirShell`] · [`GasVocabulary`] · [`MAX_AIR_SHELLS`] | The atmosphere as a stack of gas veils, heaviest lowest | Feeds the shell renderer directly |

### Constants a caller reads or tunes

- **The size model** ([`config`]): [`TILE_SPAN_M`] (the canon — one hex is a
  2048² map at 128 ft/px ≈ 49.65 mi), [`CELL_AREA_M2`], [`PLANET_FREQ`] (96),
  [`PLANET_CELLS`] (92,162), [`PLANET_MASS_KG`], [`GRAVITY_M_S2`],
  [`NOMINAL_DT_MYR`], and the derivations [`radius_for_cells`],
  [`radius_for_freq`], [`size_scale`]. Every absolute is a **reference value at
  freq 96**; a world on any other grid is a *smaller/larger planet* (never bigger
  hexes) and derives its own radius/mass/gravity/budget through `size_scale`.
- **Default rates** (each the physics as the process chose it, all `Levers`
  defaults): [`DEFAULT_OUTGAS_RATE`], [`DEFAULT_CRUST_GEN_RATE`],
  [`DEFAULT_ERUPTION_RATE`], [`DEFAULT_PRODUCTION_RATE`],
  [`DEFAULT_DECOMPOSER_NICHE_KG`], [`DEFAULT_WATER_DELIVERY_RATE`],
  [`DEFAULT_LEACH_RATE`], [`DEFAULT_YIELD_STRAIN`], [`DEFAULT_ARC_RETURN`],
  [`DEFAULT_EROSION_RATE`], [`DEFAULT_VENEER_KG`], `DEFAULT_WATER_KG`.
- **Physical thresholds** exposed for gate-coupling tests: `MAGMA_OCEAN_K`,
  `ECLOGITE_DEPTH_M`, `STRATA_SOFT_CAP`, `SUBDUCTABLE_DENSITY`, `MANTLE_DENSITY`,
  and the `CompoundId` gas/organic constants in `atmosphere`/`biosphere`.

### The levers (set a condition or a pace — never a result)

[`Levers`] is the whole maintainer surface. It holds exactly two kinds of thing,
and the distinction is the discipline: the **three boundary inputs** (how much
water arrives, how hot the inside is, how hard the star shines) and the **rates**
processes run at. **Nothing here writes an outcome** — no lever raises a mountain,
floods a basin, or places ore, because a control that could paint a continent
would make every later observation of the world unfalsifiable (rule **935269B7**).
The three kg levers (`water_budget_kg`, `veneer_budget_kg`, `decomposer_niche_kg`)
are composition statements at reference scale; [`formation_stages`] sizes them
`× size_scale³` so a gate and a ledger always compare in one frame.

---

## Interactions

- **Input signals / results / Model keys — none.** This is a headless simulation
  library: it captures no `ActionSignal`, fires no results, and touches no UI
  Model. Its consumer (`flicker-godmode`) owns all input and rendering. (When you
  see "Look/Zoom signals" or a camera, that is `flicker-globe`, not here.)
- **What it hands consumers:** [`PlanetState`] (the cheap aggregate), the derived
  reads and observer outputs above, and [`CellProgress`] over an `mpsc` channel
  during a sweep. `flicker-godmode` packs these into a `Snapshot` for its render
  thread.
- **Threads / workers:** the per-cell sweep runs on `flicker-worker`'s
  `WorkerPool` (chunked, blocks on a completion barrier — a `step` stays
  synchronous). The crate itself does no threading; `flicker-godmode` runs the
  whole sim on its own thread because a 92k-cell tick plus the every-tick audit
  is far too heavy for a frame.

---

## Gates

142 tests (`cargo test -p flicker-poc-chemistry`; 127 fast + 15 `#[ignore]`d
full-planet runs). The contract families a change must keep green:

- **Conservation harness** (`planet.rs`): `raw_leak_is_caught`,
  `creation_of_an_unbudgeted_element_is_caught`,
  `compound_bound_catches_over_budget_minerals`, `compound_bound_is_vacuous_at_seed`,
  `conserving_transfer_holds`, `delivery_adds_to_both_sides`,
  `seed_is_undifferentiated_and_balanced` — the invariant, and proof it fires.
- **Determinism** (`interior.rs`): `the_full_interior_run_is_deterministic` — same
  seed, bit-identical world hash across the full pipeline.
- **The size model** (`config.rs`, `planet.rs`): `the_planet_fits_the_grid`,
  `a_world_is_the_size_its_grid_implies`, `one_tick_is_one_hex_of_plate_motion`,
  `the_reference_planet_is_earth_sized`.
- **Pipeline-as-content** (`process_file.rs`): `the_shipped_roster_parses_and_measures`,
  `coupled_gate_numbers_match_the_physics` — the file parses, every read resolves,
  and gate numbers coupled to a stage's own constants are pinned equal (so the
  file can't gate a stage open while its tick no-ops).
- **The sea-level solve** (`planet.rs`): `an_empty_ocean_floods_nothing`,
  `sea_level_ponds_exactly_the_water_present`, `more_water_stands_higher`.
- **No scripted outcomes** (`scheduler.rs`, module literally named
  `no_scripted_outcomes`): `the_coverage_lever_cuts_the_infall_at_the_target`,
  `a_world_denied_its_water_infall_comes_out_different` — a lever sets a condition;
  the world's response is emergent.
- **Per-transformation behaviour**: each stage module carries its own gates —
  e.g. `crust.rs::the_volcanism_gate_opens_on_a_lid_and_shuts_on_the_cold`,
  `tectonics.rs::a_moving_plate_carries_its_whole_stack` /
  `the_world_stays_full_through_every_step` (occupancy),
  `hydrothermal.rs::prospecting_only_reads`,
  `atmosphere.rs::the_ocean_condenses_out_of_the_steam_as_the_world_cools`.

---

## Sharp edges

- **Broken content aborts the sim — it does not degrade.** A missing/malformed
  `processes.json`, an unknown gate `read`/`op`, a `runs` naming no stage, or an
  `accretion.json` element absent from the periodic table all **panic / error
  loudly** (rule 4BB12A75). Good discipline, but a caller must treat a content
  edit as something that can hard-stop the run.
- **Gates cannot read the clock.** There is no tick or date field — a process that
  should start "later" must name the *chemistry* that makes it later.
- **`PlanetState` lags one tick.** Stages read the top-of-tick snapshot, never the
  live world's running totals.
- **Derived reads recompute on every call.** `elevation_field` runs its flexure
  passes each time, and `sea_level_m` calls `elevation_field` — a hot loop calling
  them repeatedly pays for the recompute each time. Sample once per tick.
- **Everything absolute is reference-scale (freq 96).** A sub-freq world is a
  *smaller planet*; radius, mass, gravity and the kg levers all ride `size_scale`.
  The sizing happens at exactly two seams (`World::seed`, `formation_stages`) — do
  not size a budget or lever a second time.
- **`interior_stages()` is not the way to build a sim.** It is the fixed M1
  triplet a determinism test uses; production pipelines come from
  `formation_stages` + `processes.json`.
- **The crate's own `lib.rs` header is stale** (see Findings #2): it describes an
  "M0 + M1" crate wrapped by a `main.rs`, with crust/volatiles/life "not here
  yet". None of that is true anymore — the full pipeline is present and there is no
  binary. Trust this README and the code, not that header, until it is refreshed.
