# flicker-world Epoch Redesign — Slice 1 Handoff (the forward-regenerative core)

**Status:** Slice-1 **headless generator core landed & verified** (2026-07-08). Built to
`docs/flicker-world-epoch-redesign.md`. The thin **viewer** (flicker-shell client) and the
**compounds import** are the next pieces (see §5). This doc is the state-of-play; read the
manifesto for the "why".

---

## 1. What landed

A GPU-free, forward-regenerative world generator, mirroring the `flicker-system` / `flicker-sol2`
split. All headless, all verified by `cargo test` (the user verifies visuals when the viewer lands).

### New crate: `crates/flicker-worldengine`
The facade over the `flicker-worldgen` physics kernels. Modules:
- **`config.rs`** — `WorldConfig` (string-keyed lever `values` + `freq` + `seed`), the salvaged
  `build_epoch1`/`build_transforms`/`seed_chain`/`next_seed`/`mutate_epoch` (moved out of the old
  `flicker-world/src/world.rs`). `WORLD_EPOCHS = 9`.
- **`levers.rs`** — the content-data seam: `GeneratorParamsSource` trait (a `TableSource` mirror) +
  `JsonGeneratorSource` loading `abundance.json` + `epoch_defaults.json`; `GeneratorParams` (indexed).
- **`snapshot.rs`** — `EpochSnapshot { epoch, cells: Vec<HexState>, plates, watersheds, provenance }`
  (a named, `serde(default)`-expandable struct, NOT a bare Vec) + `Provenance` + `conserved_mass()`.
- **`engine.rs`** — `WorldEngine`: owns the topology, config, seed chain, and a lazy
  `cache: Vec<Option<EpochSnapshot>>`. Verbs: `snapshot(epoch)` (lazy forward-fill),
  `set_lever`/`reseed`/`set_seed`/`set_freq` (each does the **locked forward-only invalidation** —
  editing epoch *n* drops `n..=9`, freezes `..n`), `capture()` → `.epoch`, `restore()`.
- **`epochfile.rs`** — the **`.epoch` file format** (JSON, modelled on `.flight`): header
  (`format`/`version`/`_comment`) + `config` + `snapshots`. `from_json`/`to_json`/`load`/`save`/
  `validate`; a `.gz` path is transparently gzipped (the `bakes/*.json.gz` convention). Bit-exact
  round-trip (see §3).
- **`nodes.rs`** — the **all-elements-are-harvestable** gameplay guarantee (§2).
- **`src/bin/bake.rs`** — headless bake CLI: `cargo run -p flicker-worldengine --bin bake -- [freq] [seed] [out]`.

### `flicker-worldgen` changes (physics kernels — serde + the water cycle)
- **serde derives** (with `serde(default)`) on `HexState` + `Boundary`/`Biome`/`LifeStage` + `Plate` +
  `Watershed`, so an `EpochSnapshot` serialises. `HexState::new` is now `{ composition, ..Default() }`
  so the data model can grow without touching it. (`glam` gained the `serde` feature for `Plate.motion`.)
- **`water.rs`** — the **Epoch-4 water cycle** re-homed from `flicker-pocepochs/layers.rs` onto the
  icosphere neighbour graph (§2). New `HexState` fields: `surface_water`, `humidity`, `ice`.

### Content data (`Alpha/content/data/`)
- **`abundance.json`** — the Epoch-1 element-scatter lever set (the old `ABUNDANCE_DEFS`; origin = the
  solarbirth element sliders). The 14 crust-forming generative levers; every other element is still
  represented via `Epoch1Params.abundance_floor`.
- **`epoch_defaults.json`** — every tunable lever `{epoch, id, default, min, max}` (the old
  hardcoded `PARAM_DEFS`), epochs 1-9 (7-9 carry a `duration` for the 9-epoch clock).
- **`Alpha/content/epochs/earthlike.epoch.gz`** — a committed sample world (freq 6, 1.2 MB).

### Workspace
`Cargo.toml`: registered `flicker-worldengine`; `glam` += `serde`; `serde_json` += `float_roundtrip`
(bit-exact `.epoch` reload). `cargo check --workspace` is clean.

## 1b. Material pipeline stage 2 — the compound layer (LANDED 2026-07-08)

The core material flow is **Elements → Compounds → Materials** (memory spec "Materials pipeline").
Stage 2 (compounds) now runs in the epoch sim — we stopped faking it:
- **`flicker-worldstate::CompoundLedger`** — the second conserved ledger (`CompoundId → mass`),
  mirroring `Composition`. Lives on `HexState::compounds`.
- **`flicker-materials`** — `compounds.json` (78 Prism compounds) loads into `Tables`;
  `compound_mass_fractions(compound)` gives the per-element stoichiometry; `compound_by_id`,
  `ores_of(symbol)` queries.
- **`flicker-worldgen::chemistry`** — the **epoch-aligned compound former** (`FormerPlan`):
  category-general rules (derived from each formula, not hand-coded) form the natural compounds at
  the epoch whose conditions suit them — oxides/silicates in the molten bulk (E2), delivered water +
  salts at the hydrosphere (E4), sulfide ores + natives at the veins (E5), carbonates/nitrates/
  organics with warm shallow life (E6). The engine runs it after each epoch.
- **Water is additive** — most H₂O is *delivered from the outer system*, so at E4 it is added to
  **both** ledgers (its H/O to `composition`, H₂O to `compounds`). Lever: `e4_water_delivery`.
- **Conservation:** the element ledger stays the truth; formation caps each compound at the *free*
  element mass so the element mass locked in compounds never exceeds `composition` (tested
  `compounds_form_and_stay_bounded_by_the_elements`); element mass is conserved exactly except the
  tracked E4 delivery (tested `bulk_mass_is_conserved_modulo_water_delivery`). Sample world: 54
  compounds form; mass = 3.62M seed + 362×1200 delivered = 4.054M, to the gram.
- **Levers:** `ChemistryParams { water_delivery, formation_rate }` — formation params are tunable
  (only `water_delivery` is content-data so far; per-class rates/conditions are `chemistry.rs` consts
  and rough v1, meant to be tuned against the viewer / promoted to levers).

**Stage 3 (Materials)** — the in-game surface appearance (a procedural texture/PBR expression system:
noise masking + blending → albedo/color/UV/metallic/roughness maps, a future poc-editor) — is the end
goal and **deferred**.

## 2. The nine epochs today

| # | Group | Engine behaviour |
|---|---|---|
| 1 | molten | E1 seed layer (t=0 composition scatter), always cached. |
| 2 | molten | **Accelerated** — wraps `Epoch2::apply` (its `duration` knob scales differentiation). |
| 3 | molten | **Real, material-derived plate tectonics with actual crust motion.** The material **drifts**: `Epoch3::drift_material` advects each plate's crust (composition + thickness) along its motion for the iteration count, **conserved**, re-deriving the partition each step so **seams migrate with the cratons** and provinces **collide + merge** (mountains emerge from crust piling, rifts from thinning). Calibrated: one iteration ≈ `MY_PER_TECTONIC_STEP` (~0.32 My — a ~50-mi hex at ~5 cm/yr → ~1.6 My/hex), so the scrubber reads geological time. The partition is a **pure function of the (frozen) Epoch-2 crust + the fixed iteration count**, so it is **stable across E3 reseeds** (`e3_plates` AND `e3_duration` excluded from reseed jitter — reseed varies only deformation magnitudes): plate cores = **craton centres** (local maxima of the smoothed `crust_fraction` = `craton_field`, `CRATON_SMOOTH_PASSES` 3); grow = a **watershed flood over the crust field** (boundaries settle in the thin-crust valleys → they **track the material**, not straight Voronoi midlines); drift = **material** (each plate leans toward its thinnest-crust edge). The tectonic **iteration count is viewer-scrubbable** (`engine.set_epoch3_steps` / `epoch3_full_steps`) — step the deformation 0→full to watch belts/trenches/rifts build. No per-epoch seed noise ⇒ reseeding E3 only jitters deformation magnitudes, never the layout (`e3_plates` count excluded from reseed jitter; tests `partition_is_material_derived…`, `reseeding_epoch3_keeps…`). Then the **iterative deep-time orogeny + subduction** (`tectonics::run_orogeny`, `4 × duration` steps): each convergent margin resolves **asymmetrically from the plate buoyancy contrast** — a heavy plate meeting a light one **subducts into a trench** (deep valley) while the overrider lifts a mountain **arc**; two similar-buoyancy plates **collide** (both pile the full belt). Arc/collision uplift **accumulates + diffuses inland** (tall AND wide belts); **trenches + rift valleys** (divergent margins) accumulate as sharp depressions. One-shot orogeny + analytic hotspots OFF in `apply`. **Fixed a sign inversion** — mountains build at genuine convergence, not rifts (pinned by `converging_plates_build_a_belt…`, `subduction_digs_a_trench…`). Records `plates` (cross-hex). |
| 4 | water | `Epoch4::apply` (static endowment) **then the iterative water cycle** — evaporate → advect vapour (edge flux) → orographic condense/precipitate → runoff → freeze/melt, `8 × duration` sweeps. Rewrites `precipitation` as the emergent rain-shadow field. |
| 5 | water | `Epoch5::apply` (hydrothermal field + microbial life; **greedy path-trace OFF**) **then the iterative hydrothermal percolation** (`hydrothermal::run_hydrothermal_veins`, `4 × duration` steps): fluid leaches at hot sources, diffuses along the fault network, precipitates → **branching veins emerge** (metal per emergent nearest-source *province*). Percolation writes only the `vein_strength`/`vein_element` **concentration field** (fabricates no mass); the **compound former** turns it into conserved ore compounds. |
| 6 | water | `Epoch6::apply` — erosion now **precipitation-driven** (rainfall weighted by E4's emergent rain-shadowed `precipitation`, not uniform) and a **conserved crust-material conveyor** (erosion sheds the soft/low-hardness fraction of a hex's `crust` downstream and deposits it in the basins — Σ crust invariant; the hex-scale precursor to the E7-9 heightmap Rivulet erosion) + `watersheds`; then the **harvestable-ore guarantee** (`nodes::ensure_ore_veins`, after all formation): every curated mineable ore/gem (`harvestable` in `compounds.json`) must reach a concentrated vein — force a small conserved seam for any the physics missed. **Replaces the old every-element node guarantee** (not every element needs a vein; most are refined diffusely from bulk voxels). |
| 7-9 | strata | **Pass-through stubs** (clone prev, bump epoch) — the heightmap-strata layers of the next iteration; present so the 9-slot cache + timeline are exercised end to end. |

Per your direction: **early epochs accelerated** (analytic, no new convection/advection physics);
**atmosphere + gaseous erosion brought in fully at E4**.

**The node guarantee (`nodes.rs`).** Every element must be harvestable from ≥1 node. Presence is
already universal (E1 floor), but a dilute trace element can lack a *node*. After E5, any element
that is neither a dominant surface element, nor a vein, nor a deposit anywhere gets a vein node
stamped at its richest cell — deterministic, mass-neutral (only markers change). Tested:
`every_element_forms_at_least_one_node`.

**The water cycle (`water.rs`).** Three conserved phases on `HexState` (`surface_water`, `humidity`,
`ice`); `Σ` is invariant across the sweep (tested `water_is_conserved_across_the_sweep`). Never
touches the bulk element ledger, so composition conservation is untouched. A prevailing zonal wind
advects vapour as a conservative **edge flux**; rising terrain wrings it out → windward rain, leeward
rain-shadow (tested `rain_reaches_land_and_varies`). Rates are `const`s in `water.rs`, **tunable** —
the user verifies the look and adjusts. Multi-band vertical atmosphere (the full `layers.rs` 9-band
stack) is a later refinement; v1 uses the 3-phase conveyor + orographic condensation.

## 3. Conservation & determinism (the load-bearing invariants, all tested)

- **Bulk element mass** `Σ cell.composition.total()` is invariant across every epoch
  (`bulk_mass_is_conserved_across_every_epoch`). The epochs derive crust/atmosphere/deposits/water
  from the bulk without depleting it.
- **Total water** is invariant across the E4 sweep.
- **Forward-regen replay** — `editing_a_late_lever_freezes_earlier_epochs`,
  `editing_an_early_lever_invalidates_forward`, `reseeding_an_epoch_leaves_upstream_identical`.
- **`.epoch` round-trip is bit-exact** (JSON + disk + gzip), thanks to `serde_json/float_roundtrip`.

## 4. Perf note

Full-res (freq 48, ~23 042 cells) all-9-epochs generation ≈ **4.6 s** (incl. the 40-sweep water
cycle). Fine for offline gen. But **live-drag on an Epoch-1 edit re-runs the whole chain (~4.6 s)** —
in the viewer, either drop the interactive `freq` or offload to `flicker-worker` (flagged, out of
slice 1). Editing a *late* epoch is cheap (only it + downstream re-run). A full-res `.epoch.gz` is
~71 MB — do **not** commit full worlds; the committed sample is freq 6.

## 5. Next (in priority order)

1. **The viewer — a `flicker-shell` client** (per your direction: the Alpha shell skeleton, NOT the
   old hand-rolled/examples shell). Thin, like `flicker-solarbirth`: `flicker_shell::run(ShellConfig
   { game_scene })`, a `Sim` scene that holds a `WorldEngine` + orbit cam, a **timeline slider**
   scrubbing the 9 epochs (`engine.snapshot(e)`), per-epoch control panels driven by
   `engine.params()` levers, live regen via `set_lever`/`reseed`/`set_freq`. Salvage the old
   `flicker-world` `globe.rs`/`color.rs`/`camera.rs` (ranges + 14 view modes + globe build), generalise
   6→9 epochs. The engine is ready to drive it.
2. **Compounds import from the Prism books — DONE (vocabulary); classifier next.** Source is
   `~/Repos/elide-us/Prism/BookIII.md` (on the filesystem, not GitHub). The full catalog (79
   compounds: Common 9, Mineral 22, Useful 12, Gemstone 10, Biological 9, Alloy 17) is transcribed
   into **`Alpha/content/data/compounds.json`** and loaded into `flicker_materials::Tables` via the
   `TableSource` seam (`load_compounds`, tolerant of a missing file). Each row: `id, name, formula`
   (verbatim), `category, elements[{symbol,count}]` (parsed, restricted to Prism elements — F in
   Apatite's formula is dropped from the parsed list but kept in `formula`), `extracted_element`
   (ore target), `natural` (forms in-world vs crafted alloy), `uses`. Queries: `compounds()`,
   `compound(name)`, `ores_of(symbol)`. **Remaining:** the composition→compound **classifier** (which
   compound a cell's composition forms — the deferred "formed-material classifier", BookIII §"Elements,
   Compounds, and Classified Materials") and wiring the mineral ores to the node/vein layer (an
   element's node could be its actual ore mineral, e.g. Fe→Hematite).
3. **Water cycle depth** — tune rates against the viewer; optionally a small multi-band vertical
   atmosphere for stronger orographic blocking; feed emergent `precipitation` fully into E6 erosion.
4. **Group III (E7-9)** — the real heightmap-strata layers + the 2048² materialization bridge.
5. **Worker offload** for live-drag at high freq; ISEA equal-area projection; ledger `CellId↔CellCoord`.

## 6. Where things live
- Engine: `crates/flicker-worldengine/` · Physics + water cycle: `crates/flicker-worldgen/`
- Content: `Alpha/content/data/{abundance,epoch_defaults}.json` · Samples: `Alpha/content/epochs/`
- Old viewer (to be superseded by the shell client): `crates/flicker-world/`
- Cross-machine memory: decision "flicker-world epoch-redesign: crate layout + slice-1 build decisions".
