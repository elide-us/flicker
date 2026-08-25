# flicker-worldstate

The retained **material truth** of the world: a conserved, per-column record of
*how much of each element and compound is where*, plus the one pure read that turns
that record into the single material a surface is drawn as. This is a headless data
crate — no rendering, no ticking, no I/O. Every other world crate deposits into,
takes from, and classifies against these types; nothing in here decides *when* the
world changes, only *what it is made of* at any instant.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate. Start
> points: the materials-unification plan (MCP `30FE7F58`) and the classifier's
> landing (MCP `A33BD44B`).

## Vocabulary (flicker words used below)

- **Composition** — an element→mass table (e.g. Si: 8000, O: 2000). The conserved
  quantity of the whole model: shape can be re-derived every bake, but mass cannot
  be created or destroyed.
- **Compound ledger** — the same idea one level up: compound→mass (Water, Quartz,
  Hematite, …), formed from elements by the (external) epoch chemistry.
- **Cell** — everything known about one *surface column*: its surface composition,
  the solid column beneath it, its classified drawn material, and its surface
  effects. Height/shape is **not** stored — it is a bake-time function of the mass.
- **Ledger** — the sparse map of coordinate→Cell. "Sparse" = only columns that have
  been *materialized* (touched) exist; an absent coordinate is 100%-solid bulk that
  has not been revealed yet.
- **Material identity / drawn face** — the one entry from the ≤255-slot draw catalog
  (`flicker-materials`) that a column renders as. It is a *derived narrowing* of the
  ledger, never a stored truth — extract the quartz from sand and the same column
  re-classifies to what remains.
- **Classify** — compute that drawn face from a ledger. `classify_material` is the
  one canonical read.
- **Materialize** — reveal/insert a column into the ledger on first touch.

## Where it sits

- **Builds on:** [`flicker-materials`](../../content/flicker-materials/README.md) —
  the world's element/compound/material **vocabulary**. This crate stores mass keyed
  by that vocabulary's ids and asks a `Tables` (the loaded vocabulary) to name the
  drawn material. It holds no catalog of its own.
- **Used by:** [`flicker-worldgen`](../flicker-worldgen/README.md) and
  [`flicker-poc-chemistry`](../flicker-poc-chemistry/README.md) build columns,
  reservoirs, and crust on `Composition` / `CompoundLedger` today;
  [`flicker-worldengine`](../flicker-worldengine/README.md) sits above them.
- **Reads from the content tree:** nothing. The crate receives a `&Tables` the
  caller already built (from `Alpha/content/data`); it never opens a file. (The
  crate's *tests* load `Alpha/content/data` to classify against real materials.)

### Maturity — what is wired vs. built-and-waiting

This is an engine toolbox; some of its surface is built toward the ratified spec
ahead of its first caller (MCP `F42DA5E0`). Two layers:

- **In active use:** `Composition`, `CompoundLedger`, `CompoundId` — the mass
  vectors that worldgen and poc-chemistry already build on.
- **Built, green, awaiting its consumer:** the retained surface **`Ledger`** (`Cell`,
  `Effects`, `CellCoord`) and **`classify_material`**. The classifier is complete and
  tested; the live caller that feeds real columns through it into voxel draw is the
  next consumer to build (MCP `A33BD44B`: "ready and waiting on that consumer"). The
  **erosion sweep / write-back** that will mutate `Ledger` over geological time is
  deferred. None of this is dead — it is substrate with its driver not yet attached.

## Public API

### `Composition` — element→mass vector (the conserved quantity)

| Item | For | The one thing to know |
|---|---|---|
| `Composition::new()` / `default()` | empty vector | no mass |
| `from_iter([(ElementId, f64), …])` | build from pairs | `ElementId` is the atomic number (`u8`); duplicate keys accumulate |
| `add(element, amount)` | deposit mass | **non-positive amount is a no-op** and never creates a key — taking mass out goes through `remove` |
| `remove(element, amount) -> f64` | take mass | clamps to what is present and **returns what it actually took**; drops the key at zero |
| `add_composition(&other)` | merge/confluence | sums each element; total becomes the sum of both totals |
| `amount(element) -> f64` | one element's mass | `0.0` if absent |
| `total()` / `len()` / `is_empty()` | aggregates | `total` = Σ mass; `len` = distinct elements present |
| `dominant() -> Option<ElementId>` | "what is this mostly" | heaviest element; **on a tie the higher atomic number wins** |
| `iter()` | walk contents | `(ElementId, f64)` in ascending atomic-number order (deterministic) |

Amounts are `f64` on purpose: many small per-pass transfers accumulate over
geological time and the conservation invariant is unforgiving of rounding.

### `CompoundLedger` + `CompoundId` — compound→mass vector

Same shape and same conservation-safe rules as `Composition`, keyed by compound.

| Item | For | The one thing to know |
|---|---|---|
| `CompoundId` (`= u16`) | a compound's catalog id | matches `flicker_materials::CompoundDef::id`; one id space across `compounds.json` + `crust_compounds.json` |
| `new` · `add` · `remove` · `amount` · `total` · `len` · `is_empty` · `iter` | as `Composition` | `add` non-positive = no-op; `remove` clamps + reports; `iter` ascending id order |
| `dominant() -> Option<CompoundId>` | heaviest compound | **on a tie the higher id wins**; this is the classifier's primary key |

### `Cell` + `Effects` — one surface column

| Item | For | The one thing to know |
|---|---|---|
| `Cell.composition` | surface element mass | the conserved per-column truth |
| `Cell.bulk_composition` | solid column beneath | sparse; empty until a dig/reveal materializes it |
| `Cell.surface_material: Option<MaterialId>` | **cached** drawn face | a cache of `classify_material`, **not a second truth**; `None` until classified — see Sharp edges |
| `Cell.effects: Effects` | water / ice / lava state | tracked state, not stacked layers; motion belongs to the deferred sweep |
| `Cell::from_surface(comp)` | seed a fresh column | surface set; bulk empty, dry, unclassified |
| `Cell::surface_traits(&Tables) -> ElementTraits` | fallback hardness/brittleness | **always** the element blend `Σ fractionᵢ·traitᵢ`; it never reads `surface_material`. Empty → `ElementTraits::ZERO` (not NaN) |
| `Cell::dominant_element()` | coarse tint / query | delegates to `composition.dominant()` |
| `Cell::reclassify(&CompoundLedger, &Tables)` | refresh the cached face | re-runs `classify_material` and stores it; **call after any change** to the composition or compounds |
| `Effects.water_saturation` / `.ice` / `.lava` | surface effect amounts | `water_saturation` will be bounded by the material's `water_capacity` by the deferred sweep, not here |
| `Effects::DRY` | the zero effect | all three at `0.0` |

### `Ledger` + `CellCoord` — the sparse surface map

| Item | For | The one thing to know |
|---|---|---|
| `CellCoord { x: i32, z: i32 }` · `CellCoord::new(x, z)` | address a column | surface `(x, z)` only — no `y`; the voxel layer maps its `ClusterId` onto this, so the dependency runs voxel → ledger, never the reverse |
| `Ledger::new()` | empty store | nothing materialized |
| `get(coord)` / `get_mut(coord)` | read/edit a column | `None` if not materialized |
| `contains(coord)` | presence test | — |
| `materialize(coord) -> &mut Cell` | reveal-on-touch | inserts a default (empty) cell if absent and returns it; the single write door seeding and write-back share |
| `insert(coord, cell) -> Option<Cell>` | place/replace | returns the previous cell if any |
| `remove(coord) -> Option<Cell>` | reclaim a column | — |
| `len()` / `is_empty()` / `iter()` | walk the store | `iter` yields `(CellCoord, &Cell)` **unordered** |
| `total_mass() -> f64` | conservation accounting | Σ over all cells of surface + bulk mass — the handle the (deferred) sweep must keep from drifting |

### `classify_material` — the canonical composition → material-identity read

```rust
pub fn classify_material(
    comp: &Composition,
    compounds: &CompoundLedger,
    tables: &Tables,
) -> Option<MaterialId>
```

The one place a ledger becomes a drawn face. **Pure** — it encodes no causal rule
and no phase change; you (or the sim) mutate the ledger and re-run this to see what
the column now looks like. `MaterialId` is the `u8` index into the draw catalog.

Two keys, tried in order:

1. **Dominant compound.** The heaviest compound (`compounds.dominant()`) whose name
   a catalog material claims via its `represents` column
   (`Tables::material_representing`) → that material's id. If the dominant compound
   has no representing material (e.g. a mantle mineral like Olivine), fall through.
2. **Element-signature fallback** over `comp`: every element symbol in a material's
   `signature` must be present (mass > 0) in the composition; the **most specific
   (longest) signature wins**; a **lower `MaterialId` breaks ties** deterministically.
3. **Nothing matches, or the ledger is empty → `None`.** Unclassified ground is
   drawn loud-magenta by the mesh palette rather than guessed — a name that fails to
   resolve fails loud, never to a plausible-but-wrong material (MCP `4BB12A75`).

Note which argument each key reads: key 1 reads the **compound** ledger, key 2 reads
the **element** composition. Pass both. Temperature/pressure conditions (ice vs.
water, molten vs. solid) are deliberately **not** consulted here — that is a later
registry hook, and Ice/Lava carry no `represents` on purpose.

`classify_material` is the **single** canonical composition→material read.
`flicker-worldgen`'s `classify.rs` still exposes a material-pick path, but its
material-matching half is **superseded** by this function — do not call it for
material identity (see that crate's README).

## Interactions

- **Signals / Model keys / results:** none. This is a headless data crate with no UI
  surface, no input, and no runtime Model participation.
- **What it hands other crates:** the `Composition` / `CompoundLedger` mass vectors,
  the `Ledger` of `Cell`s, and the `MaterialId` from `classify_material`.
- **Serialization:** `Cell`, `Composition`, `CompoundLedger`, `Effects`, `CellCoord`
  derive `serde`. `Ledger` derives it too but does not round-trip through plain JSON
  — see Sharp edges.
- **Threads / async:** none. Nothing here ticks; the ledger is a snapshot read and
  rewritten by an (external, deferred) pass.

## Gates

`source ~/.cargo/env && cargo test -p flicker-worldstate` — 23 tests, all green.
Each pins one contract:

- **Composition** — `empty_is_zero`; `add_accumulates` (incl. non-positive no-op);
  `remove_is_clamped_and_reports_what_it_took`;
  `moving_mass_between_compositions_conserves_total`; `merge_conserves_and_sums`;
  `dominant_picks_the_heaviest`; `iter_is_atomic_number_ordered`;
  `serde_json_round_trip`.
- **CompoundLedger** — `add_remove_conserve_and_stay_sparse`; `serde_round_trip`.
- **Cell** — `from_surface_leaves_everything_else_default`;
  `surface_traits_blend_through_the_vocabulary` (incl. empty → `ZERO`);
  `dominant_element_reads_the_composition`; `serde_json_round_trip`.
- **Ledger** — `starts_empty`; `materialize_on_touch_inserts_and_persists`;
  `insert_remove_and_distinct_coords`;
  `total_mass_sums_surface_and_bulk_over_all_cells`.
- **classify_material** — `dominant_compound_picks_the_representing_material` (incl.
  the Iron Oxide→Hematite synonym); `reclassifying_after_extraction_follows_the_ledger`
  (extract the quartz, the same column re-classifies); `unrepresented_compound_falls_back_to_signature`;
  `signature_prefers_specific_then_lower_id`; `empty_ledger_is_unclassified`.

## Sharp edges

- **`Cell::surface_material` can silently go stale.** It is a cache of
  `classify_material`. Nothing marks it dirty and no gate enforces refresh: mutate a
  cell's composition or compounds and forget `reclassify(...)`, and the column keeps
  rendering its *previous* face with no error. Treat the ledger as truth and call
  `reclassify` after every change, or read `classify_material` directly.
- **`Ledger` does not round-trip through `serde_json`.** Its map is keyed by the
  `CellCoord` struct, which JSON cannot use as an object key; `serde_json::to_string`
  on a `Ledger` errors. The `serde` derives exist for a future bake format (an
  entries-list, or the repo's compact-then-gzip convention); the persistence format
  is deliberately unfixed. Serialize individual `Cell`s, not the whole `Ledger`,
  through JSON.
- **Two tie-break directions in one classify path.** `dominant()` (both ledgers)
  breaks a mass tie toward the **higher** id; the signature fallback breaks a
  same-length tie toward the **lower** `MaterialId`. Both are deterministic; they
  simply resolve different ties.
- **`classify_material` needs both arguments populated.** A cell with compounds but
  an empty element composition classifies only if the dominant compound is
  represented; otherwise the signature fallback has no elements to match and returns
  `None`.
- **`surface_traits` is the fallback basis, not the classified material's traits.**
  It always returns the element blend. Any "the formed material's authoritative
  traits override the blend" policy lives in a (not-yet-built) consumer, not here.
- **`add` / `remove` are the only mutators of mass — by design.** There is no setter
  that could create or destroy matter; a negative "add" does nothing rather than
  silently subtracting.
