# flicker-worldgen

The offline **world-generation kernels**: the physics passes that turn a seed + a
composition recipe into the foundational per-hex maps a planet is built from —
element distribution, a differentiated crust, drifting plates and elevation, oceans
and climate, ore veins, and eroded terrain with biomes. It is a **toolbox of passes,
not a single "make a planet" call**: each pass reads the array of per-hex state and
adds or transforms fields. `flicker-worldengine` is the facade that sequences these
passes into a scrubbable epoch timeline; the runtime links none of this (it is
offline-heavy and produces the tier-② composition maps `flicker-worldstate` then
runs on).

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## Vocabulary (flicker terms used below)

- **Hex** — one cell of the planet's hex-sphere. This crate never builds the sphere;
  it operates on an array of per-hex state indexed `0..n`, given each hex's unit-sphere
  direction and its neighbour indices from outside (see *Where it sits*).
- **`HexState`** — the per-hex working record that every pass reads and writes (one per
  hex). It accumulates fields as passes run. **It carries two representations of a
  cell's material side by side** — see *The two shapes of this crate*.
- **Composition** — a conserved element-mass bag (element id → mass), from
  `flicker-worldstate`. The element ledger; never invented or destroyed by the
  conserving passes.
- **CompoundLedger / compounds** — the second ledger: how much of each *named compound*
  (H₂O, SiO₂, ores, …) has formed, bounded so the element mass locked in compounds never
  exceeds the element ledger.
- **Layer / column** — a `HexState`'s vertical stack of strata (core → mantle → crust →
  ocean → atmosphere), each owning its own composition + heat + motion. The "physical
  truth" the tick engine operates on.
- **Epoch** — one named stage of formation (Epoch 1 seed … Epoch 6 erosion). An
  `EpochTransform` reads the previous stage's per-hex layer and produces this stage's.
- **Plate / boundary** — Epoch 3 partitions the hexes into drifting plates; a hex's
  *boundary* is its relationship to its neighbours' plates (convergent / divergent /
  transform / interior).
- **Vein** — a chain/field of hexes carrying one concentrated ore metal (Epoch 5).

## Where it sits

- **Builds on:**
  - `flicker-materials` — the element/compound/material vocabulary (`Tables`), loaded
    from the content tree. Every pass takes `&Tables` for element densities, hardness,
    compound stoichiometry, etc.
  - `flicker-worldstate` — the conserved ledgers (`Composition`, `CompoundLedger`) each
    `HexState` is built from.
  - `flicker-poc-chemistry` — the **world-constant canon**: `TILE_SPAN_M` (the hex span),
    restated once here as `HEX_CM` to derive `MY_PER_TECTONIC_STEP`. This crate must not
    carry its own typing of the span (canon-unanimity).
  - `flicker-primitive` — the shared lattice noise (`noise.rs` is only its `Vec3` face).
- **Used by:**
  - `flicker-worldengine` — **the facade** that drives these kernels. It sequences the
    Epoch transforms, the tick engine, the conveyor, the water cycle and the compound
    former into a forward-regenerative snapshot timeline. It is the crate that turns
    "worldgen kernels" into "a planet".
  - `flicker-pocepochs` — the epoch-viewer scene crate.
- **Reads from the content tree:** nothing directly. It reads the vocabulary through the
  `Tables` its caller loads from `Alpha/content/data` (`periodic_table.json`,
  `compounds.json`, `materials.json`, `rocks.json`). Element *abundance* is a generation
  parameter (`Epoch1Params::abundance`), **not** a content file. If a symbol a pass names
  (e.g. the `"Water"` compound) is missing from the tables, the pass no-ops for that step.

## The two shapes of this crate (read this first)

`HexState` is written by **two parallel pipelines**, and which fields are meaningful
depends on which one produced them. A caller must know which it is driving.

| | Epoch chain | Tick engine |
|---|---|---|
| Entry point | `six_epoch_stack` / the `Epoch1..Epoch6` transforms | `run_tick` + `processes()` (+ `FormerPlan`) |
| Writes | the **flat** `HexState` fields (`composition`, `crust`, `elevation`, `heat`, `plate`, `boundary`, `water_depth`, `temperature`, `precipitation`, `vein_*`, `biome`, …) | the vertical **`column: LayerLedger`** + per-layer `compounds` |
| Model | per-hex aggregates, one snapshot per epoch, kept side by side | a growable stratum column evolved one tick at a time |
| Conserves | element mass where documented (see *Gates*); relief passes move only derived elevation | element mass across the whole column every tick |
| `temperature` unit | **°C-ish** (Epoch 4 latitude/lapse) | **Kelvin** (`T_SPACE`=2.7 … `T_SOLIDUS`=1400) |

`flicker-worldengine` drives **both**. The flat `composition` and the column mantle are
bridged for convection (`molten.rs` swaps them so one algorithm serves both); most other
fields are **not** bound across the two — the epoch chain does not read the column, and
the tick engine does not update the flat fields it doesn't own (under the tick engine the
column is authoritative and the flat `composition` is left untouched). So when you read a
`HexState`, know which pipeline filled it, and which fields that pipeline actually writes
(the table above). This split is the single biggest thing to hold in your head when using
the crate — see *Sharp edges*.

## Public API

### Seeding & the epoch chain (`state`, `epoch1`..`epoch6`, `pipeline`)

The classic path: seed a per-hex composition, then thread it through six transforms,
each keeping its own layer so the stack can be inspected epoch by epoch.

| Item | What it is | The one thing to know |
|---|---|---|
| `Epoch1` / `Epoch1Params` | The seed kernel: per-hex bulk composition from a `dir` on the unit sphere. | Pure & seeded — same `(seed, dir)` ⇒ same composition. `Params::default()` approximates **Earth's crustal** mass-% (O 46, Si 28, …), tunable; `abundance` is keyed by chemical symbol, not in `periodic_table.json`. |
| `Epoch1::seed_hex(dir)` / `seed_world(dirs)` | Seed one hex / a whole array. | Result normalized to `Epoch1Params::target_mass`. |
| `HexState` | The per-hex working record. | Cheap to clone; `#[serde(default)]` so old `.epoch` bakes gain new fields as `Default`. `surface()` returns the crust once differentiated, else the bulk. |
| `Biome` / `Boundary` / `LifeStage` | Enums on `HexState` (biome; plate-boundary kind; life-thread stage). | `LifeStage` is ordered (`stage >= Floral`); advanced, never regressed. |
| `EpochCtx<'a>` | Shared read-only context for every pass: `tables`, `dirs` (unit-sphere direction per hex), `neighbors` (neighbour indices per hex), `seed`. | This is the topology seam — the crate is degree-agnostic, so a 5-neighbour pentagon "just works" (see the integration test). |
| `EpochTransform` | Trait: `epoch()` + `apply(ctx, prev) -> Vec<HexState>`. | One transform = read the previous layer, produce this one. |
| `Epoch2`..`Epoch6` | The transforms: differentiation, tectonics, hydrosphere, mineralization, erosion/biomes. | Each has a `Default` and a set of **public tunable knobs** (fields) — see below. |
| `epoch_stack(seed, ctx, &[&dyn EpochTransform])` | Run an arbitrary chain, keeping every layer. | Result `[0]` = seed, then one layer per transform. |
| `six_epoch_stack(&Epoch1, ctx)` | The default six-layer chain. | Returns 6 layers, **Epoch 1 at `[0]`, Epoch 6 (the ground) at `[5]`**. |
| `EPOCHS` | `= 6`, the default stack length. | The ground is `EPOCHS - 1`. |
| `pipeline::NOMINAL_DURATION` | `= 5`; the nominal value of each post-seed epoch's `duration` knob on the shared cycle clock. | Not re-exported at the crate root; reach it via the module. At this value each epoch reproduces its baseline output; lower = shorter phase. |
| `PassThrough(u8)` | A transform that copies the previous layer verbatim. | A placeholder for an unwritten epoch. **Not used by the default stack** (see findings — its doc claims E4-6 are unwritten; they are not). |
| `watersheds(layer) -> Vec<Watershed>` / `Watershed` | Group a finished Epoch-6 layer into drainage basins by `HexState::watershed`. | Reconstruction only; the per-hex sink id is written by Epoch 6. |
| `dominant_histogram(&[Composition])` | Count how many compositions have each element dominant. | A headless "see and verify" summary of a distribution. |

**Epoch knob sets** (public fields on each struct; all have documented `Default`s):
`Epoch2` (`crust_density_max`, `polar_thickening`, `duration`, `settle_override`),
`Epoch3` (`plates`, `continental_fraction`, `continental_base`/`oceanic_base`,
`mountain_uplift`, `rift_drop`, `boundary_threshold`, `hotspots`/`hotspot_uplift`/
`hotspot_trail`, `max_age`, `duration`, `isostasy_abs`, `crust_pile`),
`Epoch4` (`hydration`, `equator_temp`/`pole_temp`, `lapse`, `axial_tilt`, `vapor_scale`,
`moisture_spread`, `atmosphere_mass`, `duration`, `prebiotic_rate`),
`Epoch5` (`boundary_drive`, `volcanic_drive`, `water_drive`, `vein_threshold`,
`max_vein_len`, `max_veins`, `metal_density_min`, `deposit_fraction`,
`microbial_threshold`, `vent_life_boost`, `duration`),
`Epoch6` (`duration`, `rain`, `erosion_rate`, `flow_exp`, `talus`/`talus_rate`,
`alpine_height`, `floral_threshold`/`fungal_threshold`, `organics_rate`,
`decomposer_onset`, `carbonate_onset`/`carbonate_rate`).

### The vertical column model (`layer`)

The tick engine's material truth: a growable stack of strata per hex.

| Item | What it is | The one thing to know |
|---|---|---|
| `LayerKind` | `Core, Mantle, Crust, Ocean, Atmosphere` (`#[non_exhaustive]`). | `rank()` gives the deep→high stacking order `ensure` inserts by. |
| `Layer` | One stratum: `kind`, `composition`, `compounds`, `heat`, `motion`, `thickness`. | Owns material *and* dynamics together; `mass()` = its element total. |
| `LayerLedger` | A hex's column, deepest→highest. | `from_primordial(bulk, heat)` = one Mantle layer holding the whole budget; core/crust/ocean/air are grown, never seeded. `total_composition()` merges every stratum (the single "how much of each element this hex holds" read); `transfer` conserves; `ensure(kind, heat)` grows a layer at its correct rank; `set_thickness_by_mass_share()` keeps drawn heights consistent with mass. |

### The tick engine (`process`)

The per-hex, two-pass process loop that evolves the column.

| Item | What it is | The one thing to know |
|---|---|---|
| `Process` | Trait: `name()`, `applies(ctx)` (gate on material state), `compute(ctx, out)` (pure — emits `Effect`s). | Effects, not outcomes: a process applies a physical change; history emerges. |
| `Effect` | `Transfer` (conserved layer→layer, same or neighbour hex), `Deliver` (external mass — the water veneer), `Compound` (record/release a compound), `SetTemperature` (K). | Only `Deliver` adds mass; the rest conserve. |
| `Ctx<'a>` | The read-only per-hex view: `hex`, `cells`, `neighbors`, `tables`, `delivery` (this hex's water ration). | Frozen state — pass-1 reads never see pass-2 writes. |
| `processes()` | The canonical ordered collection: Temperature, Convection, CoreDifferentiation, CrustFreezing, Outgassing, Hydrosphere. | Order-independent by construction (compute-all then apply-all). |
| `run_tick(cells, neighbors, tables, procs, delivery_total) -> f64` | One tick: compute every applicable process over the frozen state, then apply. | Returns externally-delivered mass (for the ledger). Splits `delivery_total` evenly across hexes; rebuilds thicknesses after. |

### The thermal clock (`cooling` — reach via the module)

The single global cooling state the continuous-sim keys every onset off. `temperature`
here is **Kelvin**.

- Constants: `T_MOLTEN` (1900), `T_SPACE` (2.7), `T_SOLIDUS`/`T_DIFF_FREEZE` (1400),
  `T_CONDENSE` (373), `T_WATER_DELIVERY` (500), `COOLING_TAU_MY` (1000),
  `BASE_COOLING_K`, `K_HEAT`/`U_HEAT` (radiogenic weights), `MAX_RADIOGENIC_SLOWDOWN`,
  `RADIOGENIC_HALF`, `DIFF_RATE`, `CRUST_FIRM`, `LID_SHARE`.
- Functions: `normalized(temp)` (K → 0..1), `element_heat_weight`, `radiogenic_index`,
  `cell_radiogenic_heat`, `cooling_k(cells)`, `temperature_at(k, step)` (closed form),
  `cooling_step(temp, k)` (one tick), `convection_vigor(k, full_steps)` (front-loaded,
  mean-preserving), `differentiation_settle(k, full_steps)` (feeds `Epoch2::settle_override`),
  `coherent_lid(cells)`, `tectonics_onset_delay(...)`.

### Tectonic conveyor & deformation (`epoch3` extras, `tectonics`)

The time-stepped tectonics the one-shot `Epoch3::apply` can't express.

| Item | What it is |
|---|---|
| `Plate` / `Partition` | Cross-hex plate records: a plate's id/kind/drift/members; the per-hex `plate` index + the `Plate` list. |
| `Epoch3::partition` / `partition_heat` | Build the partition — from crust buoyancy, or (redesign path) from the Epoch-2 convection `heat` field with an **emergent plate count**. |
| `Epoch3::apply_with(ctx, prev, &Partition, &flow)` | `apply` against a caller-supplied partition + per-cell flow — curved seams when fed the real convection flow. |
| `Epoch3::drift_plates(...)` | Batch conveyor: advance plates whole-cell steps, tracking the partition through the moves. |
| `advance_conveyor(cells, &mut Partition, ctx, flow)` | One whole-cell conveyor advance (the tick-sim's single-step form of `drift_plates`). |
| `buoyancy_ranks(prev)` | Per-hex crust-buoyancy percentile (0..1) — the isostatic base-elevation signal. |
| `convection_flow(ctx, heat)` | Per-cell surface-flow tangent down the heat gradient — the spatially-varying plate motion. |
| `smooth_crust_thickness(cells, ctx, passes)` | De-scatter drifted `crust_fraction` before isostasy (drift path only). |
| `MY_PER_TECTONIC_STEP` | Million-years one drift iteration represents (derived from the span canon). |
| `run_orogeny(...)` | Deep-time subduction/collision deformation: trenches, arcs, rifts (moves derived relief only). |
| `run_tectonic_hotspots(...)` | Fixed mantle plumes + drift → trailing volcanic island chains (relief only). |

### Molten convection (`molten`)

| Item | What it is |
|---|---|
| `run_molten_convection(cells, ctx, steps)` | Self-organize buoyancy into convection cells and write the `heat` field (Epoch-2 phase; one-shot path, operates on flat `composition`). |
| `run_molten_convection_cooling(cells, ctx, steps, vigor)` | As above with a per-step vigor schedule (from `cooling::convection_vigor`). |
| `seed_convection_heat(cells, ctx)` / `convection_step(cells, ctx, vigor)` | The **column** path: seed then step convection on the mantle layer (swaps the mantle comp into the shared algorithm), mirroring heat + motion into the layer. Leaves the flat `composition` frozen. |

### Water cycle (`water`)

- `run_water_cycle(cells, ctx, sweeps)` — the conserved evaporate → advect → condense →
  runoff → freeze/melt sweep; writes the emergent (rain-shadowed) `precipitation`.
  Moves only the three water phases (`surface_water`/`humidity`/`ice`); never touches the
  element ledger.
- `total_water(cells)` — `Σ(surface_water + humidity + ice)`, the conservation witness.

### Erosion (`erosion`)

- `run_protoatmospheric_erosion(cells, ctx, talus, rate, steps)` — gentle hardness-aware
  creep run *before* Epoch 6, so relief doesn't run away over deep time. Moves derived
  elevation only (symmetric ⇒ the field sum is conserved).

### Mineralization (`epoch5`, `hydrothermal`)

- `Epoch5` — the one-shot mineralizer: per-hex `hydrothermal` signature + greedy vein
  traces along the fault network, depositing metal into the crust.
- `run_hydrothermal_veins(cells, ctx, &HydrothermalParams, steps)` / `HydrothermalParams`
  — the iterative reaction-transport that grows veins by leach→diffuse→precipitate,
  writing the `vein_element`/`vein_strength` **concentration field** only (fabricates no
  mass; the compound former turns concentration into ore compounds downstream).
  **See *Sharp edges* — the ore-vein approach is under review.**

### Compound former (`chemistry`)

Turns a cell's *elements* into *compounds* (the second ledger), epoch-aligned.

| Item | What it is | The one thing to know |
|---|---|---|
| `FormerPlan` | Compiled once from `Tables`; `form_for_epoch(cells, epoch, params)` forms the compounds that belong to that epoch. | Caps every compound at the *free* element mass, so locked mass never exceeds the ledger. Water delivery (epoch 4) is the one additive input (added to both ledgers). |
| `ChemistryParams` | `water_delivery`, `formation_rate`. | Tunable knobs. |
| `formable_mass(avail, fractions)` | The shared stoichiometric primitive: max mass formable from available elements. | One source for former + tick-sim outgassing (no drift). |
| `water_element_split(tables)` | `(Water compound id, H frac, O frac)` from the table's real stoichiometry. | The single source both paths split delivered water by. `None` if no Water compound. |
| `locked_element_mass(cell, tables)` | Element mass bound in a cell's compounds — the accounting check. | |

### Per-cell spatial fields (`field`)

- `FieldSampler<'a>` — turns a hex's aggregate `HexState` into continuous sub-hex
  fields (hardness, ridged/foliated relief, ore-vein filaments) for rendering/erosion.
  Many display-tuned public knobs; `sample` / `sample_blended` / `sample_blended_at`
  return a `CellSample { hardness, elevation, dominant, vein }`. Deterministic per
  position; neighbouring hexes that share an edge position agree (seam-free terrain).

### Derived classification (`classify`)

- `classify(comp, compounds, temp, pressure, tables) -> LayerClass` / `Phase` — the
  derived "what is this volume right now" read for a viewer (phase + label + colour +
  density). For composition→**material identity** call
  `flicker_worldstate::classify_material` instead; this module is the phase/label/colour
  read only, and its module doc says not to extend the material matching here (see
  findings).

### Noise (`noise` — reach via the module)

- `value_noise(p, salt, seed)`, `fbm(p, octaves, salt, seed)`, and re-exports
  `billow`/`contrast`/`ridged` — the `Vec3` face of `flicker_primitive`'s single lattice
  noise. No arithmetic of its own.

## Interactions

**None of the UI kind** — this crate is offline and headless. It reads no input signals,
publishes no Model keys, renders nothing, and spawns no threads or async. Its entire
contract is functional: given an `EpochCtx` (topology + vocabulary + seed) and a slice of
`HexState`, each pass returns/mutates `HexState`. It reads the material vocabulary through
the `Tables` its caller supplies (loaded from `Alpha/content/data`).

## Gates

The drift gates a change must keep green (`cargo test -p flicker-worldgen` — 99 unit +
1 integration):

- **Seeding** (`epoch1`): `every_hex_normalizes_to_target_mass`,
  `deterministic_and_seed_sensitive`, `varies_regionally`,
  `heavy_elements_enrich_toward_the_equator`, `volatile_elements_enrich_toward_the_poles`,
  `distribution_has_several_dominant_elements`.
- **The chain** (`pipeline`): `six_layers_threaded_through_the_chain` (each epoch changes
  the state as documented). Integration: `epoch_stack_runs_on_a_pentagon_patch` — the
  chain runs unmodified on the real icosahedral topology incl. the 5-neighbour pentagon,
  nothing goes non-finite, and Epoch-1 fields stay continuous across neighbours.
- **Differentiation** (`epoch2`): `heavy_elements_sink_out_of_the_crust`,
  `a_shorter_molten_era_leaves_more_iron_in_the_crust`, `volcanic_is_in_range_and_finite`.
- **Tectonics** (`epoch3`, `tectonics`, `molten`): plate/boundary/elevation coverage;
  `a_drifting_plate_grows_a_chain_downstream_of_the_plume`,
  `a_subduction_arc_grows_over_deep_time`, `subduction_digs_a_trench_and_lifts_an_arc`,
  `convection_conserves_mass_and_moves_material`,
  `heat_is_cold_over_dense_material_and_hot_over_light`,
  `convection_stirs_the_column_mantle_and_leaves_the_flat_field_frozen`.
- **Hydrosphere** (`epoch4`, `water`): `ocean_fills_the_low_basins_first`,
  `more_hydrogen_makes_more_ocean`, `less_oxygen_dries_the_world`,
  `free_oxygen_gates_the_water_budget`, `outgassing_builds_a_volatile_atmosphere_not_bound_oxygen`,
  `prebiotic_chemistry_favors_warm_shallow_organic_water`,
  `water_is_conserved_across_the_sweep`, `rain_reaches_land_and_varies`.
- **Mineralization** (`epoch5`, `hydrothermal`): `veins_form_and_carry_metal_into_the_crust`,
  `percolation_grows_a_selective_vein_field`, `the_bulk_element_ledger_is_untouched`,
  `deterministic_for_a_seed`.
- **Erosion/biomes** (`epoch6`, `erosion`): `erosion_grades_the_terrain_and_conserves_mass`,
  `erosion_conserves_crust_material_globally`, `drainage_partitions_into_watersheds`,
  `biomes_are_assigned_and_varied`, `time_gates_preserve_coal_and_chalk`,
  `weathering_softens_a_spike_and_conserves_the_field`.
- **The tick engine** (`process`): `the_tick_conserves_element_mass`,
  `cooling_reads_the_live_column_not_the_frozen_flat_composition`,
  `convection_moves_mass_between_hexes`.
- **The former** (`chemistry`): `forms_compounds_without_exceeding_the_element_ledger`,
  `water_delivery_is_additive_and_conserved_into_the_compound`.
- **Fields** (`field`): hardness range/variation, convection warp, crust ridging, orogeny
  lift, vein banding, foliation, determinism.
- **Cooling** (`cooling`): radiogenic slowdown, differentiation vs cooling rate,
  front-loaded-but-mean-preserving vigor, coherent-lid majority.

## Sharp edges

- **`HexState` is written by two pipelines** (see *The two shapes of this crate*). The
  flat fields (epoch chain) and the `column`/`compounds` (tick engine) are largely **not**
  bound to each other; only `composition`↔mantle is bridged (for convection). Know which
  pipeline filled a `HexState`, and which fields that pipeline writes, before you trust a
  field.
- **`HexState::temperature` has two unit conventions.** Epoch 4 writes it in **°C-ish**
  (equator 28, pole −25); the tick engine and `cooling` treat it as **Kelvin** (space 2.7,
  solidus 1400). The same field name means different units depending on the producer —
  a real trap.
- **The ore-vein mechanism is under review.** Epoch 5 + `hydrothermal.rs` use *placed*
  hydrothermal sources (`HydrothermalParams::max_sources` / `Epoch5::max_veins`, default
  64), nearest-source metal **provinces** (`hydrothermal::province_carriers`), and a
  ridged-fbm vein filament (`FieldSampler::filament`). This ore path is flagged for change
  in the crate's MCP notes — treat it as unstable. The concentration field itself is
  conserving (fabricates no mass; `composition` untouched).
- **Determinism is a contract.** Every pass is deterministic from `(seed, inputs)` — no
  wall-clock, no thread entropy. Tests pin it; keep it.
- **Conservation is per-pass, not universal.** Element mass is conserved by the tick
  engine, molten convection, the water cycle (its three phases), the compound former, and
  Epoch-6 crust transport; the **relief** passes (`run_orogeny`, hotspots,
  `run_protoatmospheric_erosion`) move only *derived* elevation and hold no element mass.
- **`Epoch2` differentiation has two dials.** `settle_override` (cooling-derived) wins when
  set; otherwise it falls back to the `duration`-fraction. The one-shot pipeline uses
  `duration`; the cooling engine supplies `settle_override`.
- **`classify` is not the material-identity read.** Use
  `flicker_worldstate::classify_material` for composition→material identity; this module's
  `classify` is the viewer's phase/label/colour read only (and its own doc says not to
  extend the material matching here).
- **`PassThrough` is a placeholder wired into nothing** in the default stack; its doc
  claims Epochs 4-6 are unwritten, which is stale (they are real).
- **`Epoch1Params::default()` seeds Earth's *crustal* assay**, not a bulk-planet budget —
  a birth-certificate default meant to be tuned, not a physical accretion recipe.
