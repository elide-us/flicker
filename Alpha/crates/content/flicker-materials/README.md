# flicker-materials

The world-material **vocabulary**: the typed, in-memory form of the periodic
table, the compound catalog, the rock catalog, and the 256-slot material index.
It answers "what element/compound/rock/material is this?" and computes a few
composition-weighted blends on top — and it does so behind a **swappable
`TableSource`**, so the rows come from JSON files today and can come from a web
service or a database later with **no change to any caller**. It is standalone:
no graphics, no voxel storage, no simulation — just the vocabulary the rest of
the world model classifies and draws against.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.
> (The `docs/material-model-handoff.md` that the source doc-comments still point
> at does not exist — see *Sharp edges*.)

## Where it sits

- **Builds on:** `serde` / `serde_json` (rows deserialize straight from JSON),
  `thiserror` (the one error type). Nothing else — this is a leaf crate.
- **Used by:** the world-sim crates `flicker-worldstate` (the classifier and the
  per-cell composition **ledger** — the store the simulation keeps of how much of
  each element sits in a cell), `flicker-worldgen`, `flicker-worldengine`,
  `flicker-worldtile`, `flicker-poc-chemistry`; the scene crates
  `flicker-sablework` (the material-rename tool — the one writer),
  `flicker-godmode`, `flicker-pocclusters` (feeds `render_palette` to the mesh
  renderer); `flicker-texture`; and the excluded `crates/flicker-system`,
  `crates/flicker-celestial`.
- **Reads from the content tree:** a directory of JSON tables (default
  `Alpha/content/data/`). Each file is named by a `pub const` (below). What
  happens when one is missing depends on the file:

  | File (`const`) | Holds | Missing → |
  |---|---|---|
  | `periodic_table.json` (`PERIODIC_TABLE_FILE`) | element rows | **error** (`Io`) — required |
  | `materials.json` (`MATERIALS_FILE`) | the 256-slot material index | **error** (`Io`) — required |
  | `compounds.json` (`COMPOUNDS_FILE`) | gameplay compounds | skipped (empty) |
  | `crust_compounds.json` (`CRUST_COMPOUNDS_FILE`) | mantle / world-sim compounds | skipped (empty) |
  | `rocks.json` (`ROCKS_FILE`) | modal rock recipes | skipped (empty) |

  The two compound files share **one id space** and are merged into a single
  catalog at load, so a consumer resolves any compound by name or id regardless
  of which file it came from.

## The four tiers (the vocabulary)

The types name four stacked tiers. You need the distinctions to use the API:

- **`Element`** — one row of the periodic table, keyed by atomic number
  (`ElementId`) or chemical symbol (`"Fe"`). Carries real-world physicals and
  three blendable base *traits* (hardness, brittleness, water_capacity).
- **`CompoundDef`** — a named element combination (`Fe₂O₃`, `CaCO₃`, an alloy),
  keyed by catalog id (`u16`) or name. Minerals **are** compounds (they live in
  this one catalog); alloys/steels are also compounds, flagged `natural = false`.
- **`RockDef`** — a **modal mixture of minerals** (not a formula): granite is
  "coarse-grained igneous rock", named by the minerals it contains and their
  proportions. Its point is `erosional_resistance` — how well it resists being
  worn away, the number differential erosion runs on.
- **`MaterialDef`** — one of the 256 *drawn* slots (`MaterialId` = `u8`, the wire
  value a voxel — the engine's unit cell of world volume — carries). A material is
  what an aggregate element *composition* **classifies to** from a voxel's point
  of view (granite, dirt, water…). The classifier that decides *which* material a
  composition forms is **deliberately not in this crate** — it lives in
  `flicker-worldstate`. This crate supplies the vocabulary that classifier reads
  (`represents`, `signature`) and the primary-key lookup (`material_representing`).

## The `TableSource` seam (the DB-candidate pattern)

`TableSource` is a trait with one method per row list (`load_elements`,
`load_materials`, and defaulted `load_compounds` / `load_rocks`). The simulation
**asks a source** for rows and never hardcodes a path. `Tables::from_source`
indexes whatever the source returns into the queryable vocabulary.

```rust
use flicker_materials::{JsonTableSource, Tables};

// Today: read the JSON tables from a content directory.
let source = JsonTableSource::new("Alpha/content/data");
let tables = Tables::from_source(&source)?;          // loads + gates + indexes

let iron   = tables.element("Fe").unwrap();          // by symbol
let granite = tables.material_by_name("Granite").unwrap();
let hematite = tables.material_representing("Hematite"); // compound → drawn material
```

Later, a `flicker-net` → web-service → DB source implements the same trait; the
call sites above are unchanged. This is the reference case of the **data-placement
law** (MCP rule D5ED9ACF): `Alpha/content/data/` is shaped as a database-backend
candidate — flat, id-keyed rows — precisely so this swap is mechanical.

**Reads go through the trait; the one write does not.** `save_material_name`
(rename a material label) lives on the concrete `JsonTableSource`, not on
`TableSource`. A caller holding `&impl TableSource` can read but not rename; the
network/DB source will implement its own write behind its own type. See *Sharp
edges*.

## Public API

### Loading, the source seam, and errors

| Item | What it is for | The one thing to know |
|---|---|---|
| `trait TableSource` | The load seam: `load_elements`, `load_materials`, `load_compounds`, `load_rocks` | Compounds/rocks default to empty so an older/leaner source still loads |
| `struct JsonTableSource` | A `TableSource` over a directory of JSON files | Holds only the dir; `Clone` |
| `JsonTableSource::new(dir)` | Construct one | `dir` is the content-data folder |
| `JsonTableSource::save_material_name(id, name)` | Relabel one material in `materials.json`, keyed **strictly by byte id** | Preserves `_meta` + every other row; a miss is a **loud error**, never a silent no-op. Concrete-only (not on the trait) |
| `enum MaterialError` | Load/write failure: `Io`, `Parse`, `Schema` | Every variant names the offending file |
| `PERIODIC_TABLE_FILE` … `ROCKS_FILE` (5 `const`s) | The filenames a `JsonTableSource` reads | See the content-tree table above |

### Building the vocabulary

| Item | What it is for | The one thing to know |
|---|---|---|
| `Tables::from_source(&src)` | **The construction path.** Load, **gate** (fail loud), index | The gates run **only here** — see *Sharp edges* |
| `Tables::from_rows(elements, materials)` | Index already-loaded element + material rows (no compounds) | **No gates run.** For callers that got rows elsewhere / tests |
| `Tables::from_rows_full(elements, materials, compounds)` | As above, with compounds | **No gates run** |

### The vocabulary types

| Type | Key(s) | Notes |
|---|---|---|
| `Element` | `symbol: String`, `number: u8` (`ElementId`) | `hardness` / `brittleness` / `water_capacity` are the blendable base traits |
| `ElementId` = `u8` | atomic number | The stable element key; compositions key by this, not symbol |
| `PhysicalState` | — | Closed enum `Solid` / `Liquid` / `Gas`; an unknown value fails deserialization |
| `MaterialDef` | `id: u8` (`MaterialId`), `name` | `render_class`, `represents` (compound names, classifier primary key), `signature` (elements, fallback key), authoritative traits, `color` |
| `MaterialId` = `u8` | 256-slot index (`0..=255`) | Distinct alias from `ElementId` — a material id and an atomic number are different namespaces, do not mix |
| `RenderClass` | — | Closed 4-way axis: `Blendable` / `HardEdge` / `Translucent` / `Emissive`. Exactly one per material; no fallback |
| `RESERVED_EXOTIC_FIRST` = `248` | — | First id of the reserved exotic-emissive block `248..=255`; no material may be defined there until released by ruling (the loader gates it) |
| `CompoundDef` | `id: u16`, `name` | `formula`, `category`, parsed `elements`, `natural`, `harvestable`, physical fields, `crystallizes` / `metamorphic` (chemistry-sim inputs) |
| `CompoundElement` | — | One `{ symbol, count }` term of a formula |
| `MetamorphicRule` | — | A phase's stability limit: `to` phase above `pressure_pa` + `temp_k` |
| `RockDef` | `id: String` (slug), `name` | `modal: name → mass fraction` (keys are exact compound names), `erosional_resistance` |
| `ElementTraits` | — | The `Σ fractionᵢ·traitᵢ` blend result (hardness/brittleness/water_capacity); `ElementTraits::ZERO` is the all-zero blend |

### Querying `Tables`

All lookups return `Option` / a slice — a miss is `None` / empty, never a panic.

| Method | Returns |
|---|---|
| `elements()` / `materials()` / `compounds()` / `rocks()` | the full row slice, in load order |
| `element(symbol)` / `element_by_number(n)` | one `Element` |
| `material(id)` / `material_by_name(name)` | one `MaterialDef` |
| `material_representing(compound_name)` | the material whose `represents` claims that compound (classifier primary key); `None` → fall back to `signature` |
| `compound(name)` / `compound_by_id(id)` | one `CompoundDef` |
| `harvestable_compounds()` | iterator of the curated mineable ores/gems (`harvestable = true`) |
| `ores_of(symbol)` | iterator of compounds whose extracted element is `symbol` (e.g. `"Fe"` → Hematite) |
| `rock(slug)` | one `RockDef` |

### Derived computations

| Method | Computes |
|---|---|
| `blend_traits(comp)` | `Σ fractionᵢ·traitᵢ` over `(symbol, amount)` — the *fallback* effective traits of a raw composition. Unknown symbols / non-positive amounts skipped; empty or all-unknown → `ElementTraits::ZERO` |
| `blend_traits_by_number(comp)` | Same, keyed by atomic number — the form the sim's ledger stores; agrees exactly with `blend_traits` |
| `compound_mass_fractions(&c)` | per-element mass fractions of a compound (Σ = 1), keyed by atomic number |
| `compound_molar_mass(&c)` | formula-unit mass (u); `0.0` for an unknown formula |
| `erosional_resistance(minerals, default)` | modal-weighted resistance of a mineral assemblage; an assemblage matching nothing gets `default` (loud, not silently `0`) |
| `render_palette()` | `[[f32; 4]; 256]`, index = material id — the buffer shape `Renderer::set_material_palette` takes. Undefined slots **and Air (id 0)** stay loud-wrong **magenta**, matching the renderer's boot palette, so a bad id renders visibly missing |

### Small helpers

| Method | Does |
|---|---|
| `CompoundDef::extracted()` | the extracted element symbol, empty-string filtered to `None` |
| `CompoundDef::contains(symbol)` | is `symbol` a constituent |
| `RockDef::modal_sorted()` | the modal minerals, heaviest fraction first |

## Interactions

- **Input signals:** none. This is a data-model crate — it answers no
  `ActionSignal`s and wires to no input.
- **Model keys:** none published or bound. It hands typed data to callers
  directly, not through the per-frame Model.
- **What it hands other crates:** the `Tables` handle (read-only after
  construction) that the world-sim crates query; the `render_palette()` colour
  buffer that `flicker-pocclusters` uploads to the mesh renderer
  (`flicker-render`); the raw `signature` / `represents` data and
  `material_representing()` that `flicker-worldstate`'s classifier reads.
- **Threads / async:** none. Build once, query freely; `Tables` is immutable
  after construction.

## Gates

`cargo test -p flicker-materials` — 14 tests, all green. They load the **real**
`Alpha/content/data` tables, so they double as content-conformance gates.

| Test | Enforces |
|---|---|
| `loads_the_full_vocabulary` | element/material counts, unique symbols + numbers (a duplicate row can't hide) |
| `element_lookups_resolve` | by-symbol and by-number agree; a gas state parses |
| `material_lookups_resolve` | id / name lookups; Air's empty signature parses; ore extraction field |
| `render_classes_cover_the_index` | every non-Air material has exactly one render class; reserved block empty; class assignments match the ruling |
| `render_palette_is_catalog_colors_over_loud_magenta` | defined slot = catalog colour; every undefined slot + Air = magenta, no invented fallback |
| `compounds_load_from_the_catalog` | the two split files merge to one id space; unique ids/names across the split; retired ids stay retired; formulas/extraction resolve |
| `merged_minerals_are_first_class_compounds` | the mantle minerals load from the second file with their physical fields and `sim_required` flags |
| `physical_fields_cover_the_whole_registry` | every compound row carries hardness/density/brittleness in range (one-table directive) |
| `rock_modal_references_resolve_via_the_compound_catalog` | every `rocks.json` modal key is a real compound name (typo/rename guard); no minerals defined in `rocks.json` |
| `blend_of_a_single_element_is_that_element` | a one-element blend equals that element's traits |
| `blend_is_amount_weighted_and_normalized` | the blend is amount-weighted and lies between the endpoints |
| `blend_skips_unknowns_and_nonpositive` | unknown symbols / zero amounts contribute nothing; nothing usable → `ZERO`, never NaN |
| `blend_by_number_matches_by_symbol` | the symbol- and number-keyed blends agree exactly |
| `save_material_name_relabels_by_id_and_preserves_the_rest` | rename is strict-by-id, preserves `_meta` + other rows, and a missing id is a loud error |

## Sharp edges

- **The content gates run only in `from_source`.** `from_rows` and
  `from_rows_full` index the same data with **no validation** — a material with a
  missing `render_class`, an id inside the reserved `248..=255` block, a
  `represents` naming an unknown compound, or **two materials claiming the same
  compound** all sail through silently (the last duplicate simply wins in the
  index). Prefer `from_source`; if you must use the `from_rows*` path, you own the
  validation.
- **A typo'd element symbol in a blend fails to `ZERO`, silently.** `blend_traits`
  skips unknown symbols by design, so a fully-unknown composition — e.g. passing
  the full name `"Iron"` instead of the symbol `"Fe"` — returns
  `ElementTraits::ZERO`, indistinguishable from a genuine all-zero blend. Authored
  content that must fail loud goes through the `from_source` gates instead; blend
  is a best-effort numeric aggregate.
- **The rename write is not behind the `TableSource` trait.** `save_material_name`
  is concrete to `JsonTableSource`. Code that holds a `&impl TableSource` cannot
  rename; only code holding the JSON source can. When the DB backend lands its
  write path is a separate, not-yet-written seam.
- **`MaterialDef.extracted_element` is the raw field** (an `Option<String>` that
  can hold an empty string); only `CompoundDef` has the empty-filtering
  `.extracted()` helper. Read the material field directly and filter yourself.
- **Every lookup miss is `None` / empty**, and `compounds()` / `rocks()` are empty
  when their files are absent — a query against a source that never loaded them
  returns nothing rather than erroring.
- **Stale design pointer in the source.** The module doc-comments and `Cargo.toml`
  reference `docs/material-model-handoff.md` (§1, §2, §6, §8) — a file that does
  not exist and a `docs/` location the project retired. The design of record is in
  MCP. This is a reported implementation gap, not a doc you can follow.
