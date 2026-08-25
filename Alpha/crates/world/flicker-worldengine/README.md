# flicker-worldengine

The GPU-free, headless **planet-generation engine**. It turns a *recipe* (a bulk-element
mix + a seed + per-epoch knobs) into an evolving planet, and it hands the resulting per-cell
state to a viewer to render — it never touches a GPU or a window itself. It is the
planet-scale sibling of `flicker-system` (the star-system sim): a caller that wants the
"seven Home worlds" is just seven of these with different recipes.

Two vocabulary words used throughout: an **epoch** is one of nine named formation stages
(molten seed → convection → tectonics → hydrosphere → mineralization → erosion → three
strata stubs); a **snapshot** is the whole planet's state frozen at the end of one epoch.

> Design of record — why it is shaped this way, decisions, history, and the world-gen
> unification rulings — lives in the project's MCP memory, not here. This file documents how
> to use the crate.

---

## Two ways to run a planet — read this first

The crate ships **two independent drivers of the same planet**, for two different jobs. They
do **not** share a run and nothing bridges them; picking the right one is the first decision.

| | `WorldEngine` (`engine.rs`) | `Simulation` (`sim.rs`) |
|---|---|---|
| **Model** | discrete: nine epochs, each a batch of many sub-steps, cached as one immutable `EpochSnapshot` | continuous: one `World` ticked forward, one convection pass per tick |
| **Coverage** | the full pipeline, epochs 1–6 real + 7–9 stubs | **epochs 1–2 only** (molten seed + convection) — deliberately minimal, the foundation the tick sim grows on |
| **Editing** | "play god" — edit a lever/reseed an epoch → that epoch and everything after it recompute, earlier epochs stay frozen | re-sim from the nearest checkpoint; deterministic in the seed |
| **Produces a `.epoch`** | **yes** — `capture()` → `EpochFile` → save | no |
| **Habitability `observe()`** | no (observe reads a tick `World`, not a snapshot) | **yes** |
| **Who drives it today** | the `bake` bin + the crate's tests | the live `flicker-pocepochs` scene (the interactive planet-simulation viewer) |

Rule of thumb: **want a captured, finished, fully-evolved world on disk → `WorldEngine`
+ `bake`. Want an interactive, ticking, observable planet in the app → `Simulation`.**

> The overlap between the two (both build the Epoch-1 seed and run Epoch-2 convection, by
> different code paths) is a known fork — see [Sharp edges](#sharp-edges).

---

## Where it sits

- **Builds on:**
  - `flicker-worldgen` — the physics kernels (epoch transforms `Epoch1..Epoch6`, the
    `cooling` clock, `run_tick`/`processes`, orogeny/erosion/water-cycle/hydrothermal passes).
    This crate is the **facade** that sequences those kernels; it holds no physics of its own.
  - `flicker-materials` — the material vocabulary (`Tables`: elements, compounds, harvestable ores).
  - `flicker-worldgrid` — the icosphere topology (`Sphere`, `icosphere_with_outlines`).
  - `flicker-core` — the one shared gzip / gz-at-rest routine the `.epoch` load/save uses.
- **Used by:**
  - `flicker-pocepochs` (`Alpha/crates/scenes/flicker-pocepochs`) — the scene that renders a
    `Simulation` and shows the `observe()` panel. It authors the scene pair
    `Alpha/content/sensorium/scenes/pocepochs.scene.json` + `.../scripts/pocepochs.lua`
    (see `Alpha/content/sensorium/README.md` for how scene pairs are authored).
  - the `bake` bin in this crate (the headless `.epoch` writer).
- **Reads from the content tree** (`Alpha/content/data/`, via `from_repo*` or an explicit dir):
  - `epoch_defaults.json` — the 38 lever definitions (`LeverDef`); **missing ⇒ `GeneratorError`**.
  - `abundance.json` — the Epoch-1 element mix (`AbundanceDef`); **missing ⇒ `GeneratorError`**.
  - `periodic_table.json` / `compounds.json` / `crust_compounds.json` — the material vocabulary
    (`Tables`); **missing ⇒ panic** in `from_repo` (it `expect`s the load).
- **Writes:** `.epoch` captures (default `Alpha/content/package/epochs/earthlike.epoch.gz`).

---

## Public API

Everything below is re-exported from `lib.rs` unless a module path is given.

### `WorldEngine` — the nine-epoch capture engine

**Construction**

| item | what it is for | the one thing to know |
|---|---|---|
| `WorldEngine::new(tables, params, config)` | build from an explicit vocabulary + levers + recipe | builds the sphere + Epoch-1 seed now; epochs 2–9 fill lazily |
| `WorldEngine::from_repo()` | Earth-like defaults from `Alpha/content/data` | **dev-only path** (compile-time relative dir); real callers pass a source — see [Sharp edges](#sharp-edges) |
| `WorldEngine::restore(tables, params, config, snapshots)` | rebuild from a captured `.epoch`'s recipe + snapshots | snapshots with the wrong cell count are dropped and recomputed on demand |

**The forward-regenerative core** — *forward-regenerative* = each epoch is a pure function of
`(previous snapshot, config, that epoch's seed)`, so editing a knob invalidates that epoch
onward and leaves everything before it byte-identical.

| item | what it is for | the one thing to know |
|---|---|---|
| `snapshot(epoch) -> &EpochSnapshot` | epoch `e`'s state, computing any missing epochs `..=e` forward | the single call the timeline scrubs; `epoch` is 1-indexed and clamped to `1..=9` |
| `peek(epoch) -> Option<&EpochSnapshot>` | an already-computed epoch, or `None` if invalidated | pure read, no work; `peek(1)` is always `Some` |
| `realize_all()` | force-compute all nine epochs | just `snapshot(WORLD_EPOCHS)` |

**"Play god" edit verbs** — each performs the locked forward-only invalidation.

| item | what it is for | the one thing to know |
|---|---|---|
| `set_lever(id, value)` | set a knob (clamped to its band) and invalidate its epoch forward | editing an Epoch-1 lever (or an `ab_<symbol>` mix entry) reseeds the whole chain. **A typo'd `id` silently no-ops and nukes the whole cache — see [Sharp edges](#sharp-edges)** |
| `reseed(epoch)` | fresh variation of one epoch, upstream left identical | jitters that epoch's knobs within their bands; `e3_plates`/`e3_duration` are held fixed so the plate layout never reshuffles |
| `set_seed(base)` | a whole new world | invalidates everything |
| `set_freq(freq)` | change grid resolution | rebuilds the topology; invalidates everything; clamped `MIN_FREQ..=MAX_FREQ` |
| `set_epoch2_steps(Option<u32>)` | scrub Epoch-2 convection (`0` = raw buoyancy, `None` = full) | invalidates from epoch 2 |
| `set_epoch3_steps(Option<u32>)` | scrub Epoch-3 tectonics (`0` = undeformed partition, `None` = full) | invalidates from epoch 3 |

**The cooling clock** — one continuous heat-loss axis spanning all nine epochs (`T=1` molten
at birth → cooling toward space). Every method is a read for a viewer to plot/label the
timeline; none mutates except `tectonics_onset_step` (it computes epoch 2 to read the lid).

| item | returns |
|---|---|
| `cooling_k()` | the per-step Newtonian decay coefficient (set by radiogenic content) |
| `epoch_cool_steps(e)` | cooling steps epoch `e` advances the clock (E1 = 0) |
| `cool_step_before(e)` / `cool_step_end(e)` | cumulative clock position at the start / end of epoch `e` |
| `cooling_total_steps()` | the whole span (`0..cooling_total_steps()`) |
| `epoch2_full_steps()` / `epoch3_full_steps()` | the top of the convection / tectonic scrubber at the current duration |
| `epoch3_my_per_step()` | real time (million years) one tectonic iteration represents — for labelling |
| `tectonics_onset_step() -> Option<u32>` | absolute clock step plates begin, or `None` = a stagnant still-molten world |

**Capture & read access**

| item | what it is for |
|---|---|
| `capture(comment) -> EpochFile` | realise all nine epochs and bundle recipe + snapshots into a savable `.epoch` |
| `config()` / `params()` / `tables()` / `sphere()` / `outlines()` / `seeds()` | borrow the recipe, lever tables, vocabulary, topology, per-cell boundary polygons, per-epoch seed chain |

### `Simulation` — the interactive tick sim

*Tick* = the time for crust to move one hex (~49.65 mi) at plate speed; see `MY_PER_TICK`.
History is deterministic re-sim: no per-tick log is stored — `state_at` re-runs from the
nearest cached checkpoint, so a tick is a pure function of the prior tick.

| item | what it is for | the one thing to know |
|---|---|---|
| `Simulation::new(tables, params, config)` | build from explicit inputs | builds the sphere + molten tick-0 seed now |
| `Simulation::from_repo()` / `from_repo_seeded(freq, seed)` | Earth-like / seeded defaults from `Alpha/content/data` | **dev-only path** — see [Sharp edges](#sharp-edges) |
| `state_at(tick) -> &World` | the world at `tick`, re-simmed from the nearest checkpoint ≤ `tick` | caches `tick` + every 16th tick along the way |
| `world(tick) -> Option<&World>` | an already-cached world (no work) | call after `state_at`/`ensure` |
| `ensure(tick)` | compute + cache `tick` without holding the borrow | |
| `sphere()` / `outlines()` / `tables()` | topology + boundary polygons + vocabulary for the viewer mesh | |

**`World`** — the single evolving planet the tick sim advances (`Clone`, so a checkpoint is a
snapshot):

| field | meaning |
|---|---|
| `cells: Vec<HexState>` | per-hex state, ticked by the process pipeline |
| `tick: u64` | ticks elapsed since the molten seed |
| `temp: f32` | planet **mean** temperature in **Kelvin** (a HUD/gauge derived value; the per-hex `HexState::temperature` is the truth) |
| `delivered: f64` | cumulative water mass delivered from outside (column conservation = `seed + delivered`) |
| `water_budget() -> f64` | the finite outer-system water still to be delivered |

### `observe()` + `Habitability` + `Axis` — the condition observer (`habitability.rs`)

A **pure classifier** over a tick-sim `World`. It reads what the sim already produced and
reports where each condition axis sits against its habitable "green band". It **adds no
causal rules and never steers the sim** — every world is always *somewhere*, and
"life-supporting" is the one coincidence where every axis is simultaneously in band,
*detected, never scripted* (the causes-only law; MCP rule `DDAC1B1C`).

| item | what it is for | the one thing to know |
|---|---|---|
| `observe(world: &World) -> Habitability` | read all five condition axes | pure — mutates nothing; re-observing yields the identical reading |
| `Habitability.axes: Vec<Axis>` | the five axes in display order | interior · surface-temp · atmosphere · hydrosphere · ocean-pH |
| `Habitability.life_supporting: bool` | the full conjunction (every axis live **and** in band) | **always `false` today** — two axes are unsimulated stubs; this is the honest "cannot yet be judged" state, not an outcome |
| `Habitability.axes_live` / `axes_in_band` | how many axes have a live signal / are in band | |
| `Habitability.atmosphere_kind: &'static str` | dominant air species as a `$hab_air_*` token, or `"—"` | |
| `Axis { name, signal: Option<f32>, lo, hi, low_label, high_label }` | one axis: position `0..1` vs its band `[lo,hi]` | `signal == None` ⇒ that axis's causal stage isn't simulated yet (shown greyed, excluded from the verdict) |
| `Axis::in_band()` | live signal inside `[lo, hi]`? | a `None` signal is never in band |

`name` / `low_label` / `high_label` / `atmosphere_kind` are emitted as `$…` **stringtable
tokens** (a `$name` the consumer resolves to localized text). The observer never hands out raw
English; all 22 `hab_*` tokens live in `Alpha/content/data/stringtable.json`.

### The recipe & the levers (`config.rs`, `levers.rs`)

A **lever** is one authored, data-driven generation knob, keyed by a stable string **id**
(`e3_mountain_uplift`) that is the same in `epoch_defaults.json`, in the recipe, and on the
HUD slider. The **`ab_<symbol>`** ids (e.g. `ab_O`, `ab_Fe`) are the Epoch-1 element mix.

| item | what it is for | the one thing to know |
|---|---|---|
| `WorldConfig { values, freq, seed }` | the durable, serialisable **input recipe** (captured in a `.epoch`) | `values` is a string-keyed lever map; cheap to clone for the edit loop |
| `WorldConfig::from_params(params)` | seed every lever at its default, every element at its weight | at `DEFAULT_FREQ`/`DEFAULT_SEED` |
| `WorldConfig::get` / `set` / `set_clamped` | read / write / band-clamped write of a lever | `get` returns `0.0` for an **unknown id** (silent — see [Sharp edges](#sharp-edges)) |
| `GeneratorParams` | the loaded, indexed lever + abundance tables | `levers()` / `abundance()` / `lever(id)` |
| `GeneratorParams::from_source` / `from_dir` / `from_repo` / `from_rows` | build from any source / a dir / repo content / raw rows | `from_source` is the one real construction path; the rest wrap it |
| `GeneratorParamsSource` (trait) + `JsonGeneratorSource` | the swappable data-source seam (JSON now, DB/web later) | implement the trait for a new backend; no caller changes |
| `LeverDef { epoch, id, default, min, max, note }` | one knob's definition | `[min,max]` is the slider span **and** the reseed wander band |
| `AbundanceDef { symbol, weight }` | one Epoch-1 element's starting relative abundance | |
| `build_epoch1` / `build_transforms` | turn flat lever values into the typed `flicker_worldgen` epoch kernels | mapping is verbatim — the physics is `flicker-worldgen`'s |
| `next_seed` / `seed_chain` / `mutate_epoch` | splitmix64 next-seed · per-epoch seed chain · one-epoch reseed jitter | a per-epoch reseed leaves upstream seeds untouched |
| `repo_content_dir()` | the shared `Alpha/content/data` path | so consumers needn't re-spell the relative prefix |

The **full lever catalog** (all 38 ids, their epochs, defaults, and bands) is not repeated
here — it lives in `Alpha/content/data/epoch_defaults.json`, the single source. That is where
you discover the next knob to set. (Note: `config::ABUNDANCE_PREFIX` — the `"ab_"` string — is
`pub` in the module but **not** re-exported from `lib.rs`; reach it as
`flicker_worldengine::config::ABUNDANCE_PREFIX`.)

### The `.epoch` capture format (`epochfile.rs`)

A `.epoch` is a generated world captured as authored data — JSON, the same shape as a
`.flight`/`.pack`: a `format`/`version` header, an optional `_comment`, the input `config`,
then the per-epoch `snapshots`. Because the recipe rides along, a capture can be reloaded
*or* regenerated bit-for-bit.

| item | what it is for | the one thing to know |
|---|---|---|
| `EpochFile { format, version, comment, config, snapshots }` | a parsed capture | `#[serde(default)]` throughout ⇒ forward-tolerant (older bakes still load) |
| `EpochFile::new` | wrap a recipe + snapshots with the current header | |
| `EpochFile::load(path)` / `save(path)` | read/write through the shared gz seam | a `.gz` path (or a `.epoch` with a `.gz` twin) is transparently gzipped |
| `EpochFile::from_json` / `to_json` / `to_json_pretty` | parse / compact / indented text | `save` writes compact (snapshots are large); `to_json_pretty` for humans |
| `EPOCH_FORMAT` (`"flicker.epoch"`) / `EPOCH_VERSION` (`1`) | the header constants | |

`load`/`from_json` validate: non-empty, every snapshot the same cell count, epochs in range
and ascending — a bad file returns `EpochFileError`, never a half-world.

### `EpochSnapshot` + `Provenance` (`snapshot.rs`)

| item | what it is for | the one thing to know |
|---|---|---|
| `EpochSnapshot { epoch, cells, plates, watersheds, temperature, provenance }` | one epoch's immutable output — a whole planet | a named struct (not a bare `Vec`) so the model can grow via `#[serde(default)]` fields |
| `.temperature: f32` | the planet's **global thermal state, normalized `0..1`** (the cooling clock; `1`=molten) | **distinct from `HexState::temperature`** (per-hex surface °C) and from `World.temp` (mean Kelvin) — see [Sharp edges](#sharp-edges) |
| `len()` / `is_empty()` / `conserved_mass()` | cell count / emptiness / `Σ cell.composition.total()` | `conserved_mass` is the invariant witness (see [Gates](#gates)) |
| `Provenance { epoch, seed, steps, conserved_mass }` | the reproducibility stamp in a `.epoch` | |
| `masses_agree(a, b, rel_eps)` | relative-tolerance mass comparison | the conservation check the engine runs from the seed layer forward |

### `nodes::ensure_ore_veins(cells, tables) -> usize`

The **harvestable-ore vein guarantee** (a gameplay backstop, run once at Epoch 6). Not every
element gets a vein — only the curated `harvestable` ores (Hematite, Native Gold, …). Any that
the physics never concentrated to a mineable seam is force-formed a small **conserved** seam at
the cell best able to make it. Returns how many veins had to be forced (`0` once the physics
already covers the catalog). Never exceeds a cell's conserved element ledger.

### Re-exports (so a viewer needn't depend on `flicker-worldgen`/`-materials` directly)

`classify`, `HexState`, `Layer`, `LayerClass`, `LayerKind`, `LayerLedger`, `Phase`,
`cooling` (module) from `flicker-worldgen`; `Tables` from `flicker-materials`.

### Constants

`WORLD_EPOCHS` (9) · `DEFAULT_FREQ` (48, ≈23042 cells) · `DEFAULT_SEED` · `MIN_FREQ` (6) ·
`MAX_FREQ` (96, ≈92k cells = full Earth) · `MY_PER_TICK` (million years per tick).

---

## Interactions

- **Signals / intents:** **none.** This is a headless engine — it reads no input and captures
  no signals. All input, Model publishing/binding, and scene wiring belong to the consumer
  (`flicker-pocepochs`) and its scene pair; see `Alpha/content/sensorium/README.md`.
- **Model keys:** none published or bound here. The crate's *output contract* is data
  (`EpochSnapshot`/`World`/`Habitability`) plus the `$hab_*` and `$hab_air_*` stringtable
  tokens the observer emits for the consumer to resolve.
- **What it hands other crates:** the per-cell `cells` arrays + `sphere()`/`outlines()` for a
  viewer's mesh; an `EpochFile` for disk.
- **Threads / workers / async:** none — synchronous; the `WorldEngine` cache and the
  `Simulation` checkpoints fill lazily on the calling thread.

---

## Gates

The 16 tests that pin the contracts (run `cargo test -p flicker-worldengine`):

| test | what breaks it |
|---|---|
| `bulk_mass_is_conserved_modulo_water_delivery` | any epoch adds/loses element mass except the Epoch-4 water delivery |
| `compounds_form_and_stay_bounded_by_the_elements` | a cell locks more of an element into compounds than its ledger holds |
| `editing_a_late_lever_freezes_earlier_epochs` | an Epoch-6 edit disturbs an earlier epoch, or fails to drop epoch 6 |
| `editing_an_early_lever_invalidates_forward` | an Epoch-3 edit leaves a later epoch stale (or drops an earlier one) |
| `reseeding_an_epoch_leaves_upstream_identical` | a reseed changes an upstream epoch |
| `reseeding_epoch3_keeps_the_material_derived_plate_layout` | reseeding E3 reshuffles the plate partition (it must only jitter magnitudes) |
| `every_harvestable_ore_forms_somewhere` | a harvestable ore reaches no mineable vein; or water is wrongly mineable |
| `the_cooling_clock_spans_every_epoch_as_one_axis` | the per-epoch cooling boundaries aren't contiguous/increasing across E2–E9 |
| `epochs_seven_to_nine_are_present_stubs` | a stub epoch doesn't span the whole planet |
| `masses_agree_within_tolerance` | the relative-tolerance mass comparison misbehaves |
| `the_pipeline_conserves_mass_modulo_delivery` | the tick pipeline's column mass ≠ `seed + delivered` |
| `re_sim_is_deterministic_and_resets_cleanly` | re-simming/​resetting the tick sim diverges |
| `observer_reads_five_axes_and_is_pure` | the observer stops reading five axes, or mutates state |
| `captures_round_trip_through_json_and_disk` | a `.epoch` doesn't round-trip through JSON + plain/gz disk |
| `rejects_empty_and_out_of_range` | an invalid `.epoch` is accepted |
| `repo_tables_load_and_index` | the content lever/abundance tables fail to load or index |

---

## Sharp edges

- **Two drivers, one crate.** `WorldEngine` and `Simulation` both evolve "this planet" but
  share no run and nothing bridges them: `Simulation` can't produce a `.epoch`; `WorldEngine`
  can't be tick-scrubbed; `observe()` reads only a tick `World`, never a snapshot. They also
  each build the Epoch-1 seed and run Epoch-2 convection by *different* code. Use the
  [table above](#two-ways-to-run-a-planet--read-this-first) to pick one; don't expect a value
  to cross between them.
- **A typo'd lever id fails silently *and* over-reacts.** `set_lever("e3_montain_uplift", …)`
  (misspelled) does not warn: `WorldConfig::set_clamped` inserts a brand-new key generation
  never reads, and `set_lever` can't resolve the id so it treats it as an **Epoch-1** edit and
  invalidates the whole nine-epoch cache. `WorldConfig::get` likewise returns `0.0` for any
  unknown id. There is no "unknown lever" gate — a mistyped knob is indistinguishable from a
  no-op. Prefer checking `params().lever(id).is_some()` before `set_lever`.
- **Three different "temperature"s on the public surface, different units:**
  `EpochSnapshot.temperature` (normalized `0..1` cooling clock, whole planet) ·
  `World.temp` (mean **Kelvin**, whole planet) · `HexState::temperature` (per-hex surface
  **°C**). The observer's interior axis bridges two of them via `cooling::normalized(world.temp)`.
  The field docs explain each in place, but the names collide — read the unit, not the word.
- **`from_repo` / `from_repo_seeded` are dev-only.** They bake a compile-time relative path
  (`CARGO_MANIFEST_DIR/../../../../Alpha/content/data`) valid only in the dev workspace layout;
  an installed/relocated build won't find it. Real callers pass a `GeneratorParamsSource` (or
  an explicit dir) so the data home stays swappable. (Also true of `bake`'s default output path.)
- **`.epoch` capture is write-only in the shipped app so far.** `bake` writes
  `earthlike.epoch.gz`; `EpochFile::load` + `WorldEngine::restore` exist and round-trip in the
  tests, but no shipped scene loads a captured world yet. The read path is a finished tool
  awaiting a consumer, not a broken one.
- **`life_supporting` is always `false` today.** Two of the five observer axes (surface
  temperature, ocean pH) have no causal stage in the tick sim yet, so their signal is `None` and
  the full-conjunction verdict cannot be true. This is the intended honest state; each axis goes
  live as its procedure lands.
- **Tick checkpoints grow unbounded.** A long `Simulation` play accumulates one checkpoint per
  16 ticks (plus each requested tick) with no eviction.
- **Epochs 7–9 are pass-through stubs.** `snapshot(7..=9)` returns the Epoch-6 cells unchanged
  (only the cooling clock advances) so the nine-slot timeline is exercised end to end.
