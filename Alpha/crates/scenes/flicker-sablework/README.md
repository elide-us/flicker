# flicker-sablework

The **Sablework Bench** — the texture-synthesizer *console*. It is the interactive
face on [`flicker-texture`](../../content/flicker-texture/README.md): a six-voice
noise rack on the left, a tiled swatch (or a lit turntable) in the middle, and an
output stage on the right. Every control is a two-way `bind` into one
`TextureRecipe`; **nothing here computes an image** — the bench turns knobs, the
instrument bakes, and **Commit** writes the finished material into `staging/` for
the Content Manager to promote. It runs as a canonical *scene pair* (the five-line
architecture) and is registered under the **Developer** realm in `prism-alpha`.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## Vocabulary (the flicker words used below)

- **Scene pair** — a scene is authored as two files plus Rust: `sablework.scene.json`
  (the component tree + anchors + this scene's style blocks) and `sablework.lua`
  (presentation logic), with the Rust component kinds doing the drawing. See
  [`Alpha/content/sensorium/README.md`](../../../content/sensorium/README.md) for how
  to author them; this README does not re-teach the format.
- **Model** — the per-frame `key → value` table the behaviour hands the tree and the
  Lua. The tree `bind`s a key to show its value; the Lua's `derive()` reads Model keys
  and returns more of them (presentation), folded on top.
- **Signal** — an abstract input event (Confirm, Menu, NavUp, TabNext…). Nothing in
  this bench is wired to a key; a mouse click is a Confirm at whatever it hits. The
  signal catalog is [`flicker-input-core`](../../input/flicker-input-core/README.md).
- **Result / intent** — the *name* a captured signal (or a pointer click) fires. All
  of them land in one place, [`route::apply`], as entries in a `ValueMap`.
- **Bake / recipe / rack / output stage / rung / patch / roll** — the instrument's own
  words, all defined in
  [`flicker-texture`](../../content/flicker-texture/README.md). Briefly: a **recipe**
  is the whole saved instrument; a **bake** turns `(recipe, size)` into a set of PBR
  maps; a **rung** is one resolution on the bake ladder; a **patch** is a factory
  recipe; **Roll** rolls a whole new instrument from a seed.
- **Staging** — the review tier a bench writes to
  ([`Alpha/content/staging/`](../../../content/staging/README.md)); nothing it produces
  reaches the running game until the Content Manager promotes it to `package/`.

## Where it sits

Scenes cluster (`Alpha/crates/scenes/`). It is a *thin face* on headless services —
the instrument is [`flicker-texture`], the threading is [`flicker-worker`], the file
tree is [`flicker-content`]. Its sibling scene pairs are the other migrated benches
(the reference for this shape is Quartermaster; the pump/2D chain is
[`flicker-clicktrainer`](../flicker-clicktrainer/README.md)).

- **Builds on:**
  - [`flicker-texture`](../../content/flicker-texture/README.md) — THE instrument:
    `TextureRecipe`, the six-voice rack, `bake`, the resolution ladder, `presets`,
    `random`. Every control edits a recipe; the bench never synthesizes pixels itself.
  - [`flicker-worker`](../../core/flicker-worker/README.md) — the `WorkerPool` every
    bake runs on. A 2K bake is ~360 ms and even the 256² preview ~10 ms, so neither
    belongs on the frame thread.
  - [`flicker-content`](../../content/flicker-content/README.md) — the content **roots**
    service (`staging()` + `data()`) and the gz write seam (`package::write_text`). The
    bench asks *where* content lives; it never spells out a path.
  - [`flicker-materials`](../../content/flicker-materials/README.md) — the 256-row
    material index (`MaterialId = u8`) a recipe optionally binds to, read through
    `Tables` / `JsonTableSource`.
  - [`flicker-shell`](../../frontend/flicker-shell/README.md) — the `PauseScene` overlay
    and the shared `Theme`; the pump-owned input profile for the pause map.
  - `flicker` (umbrella) — `Scene`/`Renderer`/`FrameGraph`, `ScriptHost`, the walker
    (`WalkerHandler`) and the drift gates; `flicker-input-core` + `flicker-input-router`
    — the input model and the router (the "pump") that resolves this frame's signals.
  - `image` — PNG encoding for the committed maps (`flicker-texture` stays format-free
    and hands back raw pixels; turning those into a file is the bench's job).
- **Used by:** `prism-alpha` — registers [`scene`] in the launcher roster
  (`SceneEntry::new("sablework", …, flicker_sablework::scene).with_realm(REALM_DEVELOPER)`).
- **Reads from the content tree** (each: path · when · what happens if missing):
  - `content/sensorium/scenes/sablework.scene.json` — the authored tree, this scene's
    `styles`, **and** the `stages.sablework_lit` block the lit view compiles. `include_str!`d
    for the tests; the runtime gets the same file through the manifest `SceneDef`. No tree
    ⇒ an error is logged and nothing draws.
  - `content/sensorium/scripts/sablework.lua` — the pair script. `include_str!`d. Fails to
    load ⇒ error logged, the bench runs on **raw** Model values only (no row washes, only
    the first view cell shows).
  - `content/data/materials.json` — the material index (via `roots().data()`); also the
    write target for an in-place **rename** (strictly by byte id). Unreadable ⇒ warn, an
    unbound-only picker (visibly wrong, not silently wrong).
  - `content/data/palettes.json` (`palettes.emissive`) — the emissive glow palette.
    Unreadable ⇒ warn, an empty swatch strip (visibly wrong).
  - `content/data/stringtable.json` — every display string the bench publishes is a
    `$token` resolved here at the draw boundary; a token with no entry is gated loud.
- **Writes to the content tree:** `staging/materials/<Asset>/` on Commit — one PNG per
  map plus `<Asset>.texture.json.gz` (the recipe, written **last**). Never `package/`.

## Public API

Everything is reachable from the crate root (`flicker_sablework::…`); the four
sub-modules (`commit`, `lit`, `palette`, `route`) are `pub`.

### The scene

| Item | What it is | The one thing to know |
|---|---|---|
| `fn scene(def: &SceneDef) -> Box<dyn Scene>` | The roster factory `prism-alpha` registers | The manifest resolves `sablework.scene.json` and hands its def here. |
| `struct Sablework` | The `Scene`: recipe + preview + worker + lit view + the Model | One `TextureRecipe` is the whole editable state; everything else is a view of it. |
| `Sablework::new(def)` | Runtime constructor | Clones the def's tree + styles; fills the Rust-filled containers; kicks the first preview bake. |
| `Sablework::shipped()` | A bench on the `include_str!`d scene file | The seam a test drives without an app — the same authored tree the runtime gets. |
| `Sablework::bake_rung()` | `-> &BakeSize` | The rung a Commit will use; only ever an **offered** rung. |
| accessors (tests) | `recipe`, `selected_map`, `showing_lit`, `selected_voice`, `shown_generation`, `authored_tree` | Read-only proofs for the gates (e.g. `shown_generation` proves an edit actually re-baked). |

### View vocabulary (one set of ids, shared three ways)

| Item | What it is | The one thing to know |
|---|---|---|
| `const MAP_IDS: [&str; 7]` | The flat view-cell ids in `MapKind::ALL` order (`map_base`…`map_emit`) | The SAME vocabulary the tree's `visible_bind` cells and `sablework.lua`'s `MAPS` use; the drift gates pin all three to each other. |
| `const LIT_ID: &str` | `"map_lit"` — the lit view's cell | Not an eighth `MAP_IDS` entry: the seven are flat maps the swatch blits, this one is a rendered sub-scene of all of them. |
| `const VIEW_COUNT: usize` | `= 8` | The seven flat maps + the lit view; the number the view tabs bind ranges `0..VIEW_COUNT`. |
| `enum CommitState` | `Idle · Working · Done(path) · Failed(why)` | What the status line reads while a commit runs on the worker. `Done` carries the folder so the reviewer can go look. |

### `commit` — write the artifact into `staging/`

| Item | What it is | The one thing to know |
|---|---|---|
| `fn commit(recipe, size, staging_root) -> io::Result<Committed>` | Bake at `size`, write the folder | `staging_root` is passed in (a test drives a temp tree); the scene passes `roots().staging()`. |
| `struct Committed { dir, files }` | What a commit wrote | `files` is in write order, the recipe **last** — a reader that finds the recipe can trust the maps beside it are complete. |
| `fn asset_name(name, id) -> String` | Fold an authored name into the asset standard | PascalCase-Hyphenated, no separators, no `..`; empty ⇒ the id, then `"Untitled"`. A name reaches here from a text field, so it is forced, never trusted. |
| `const MATERIALS_DIR` | `"materials"` | One name, so the staging and package sides cannot drift. |

Maps land **raw** PNG in the *narrowest* form that carries their meaning (scalar maps
as L8, colour maps + `Normal` as RGB8); the recipe lands **gz**. That split is the
at-rest rule (`GZIFY_EXTENSIONS` skips images) — gzipping a PNG would make a promotion
a transcode. See the module docs for the full rationale.

### `lit` — the lit turntable preview

| Item | What it is | The one thing to know |
|---|---|---|
| `struct LitPreview` | The material on a spinning body under a fixed light | Roughness/metalness/normal barely read in a flat swatch; this is the view that makes the output stage dialable. |
| `enum Body { Sphere, Plane }` | Which body the sample wears; `toggled()`, `id()` | Sphere sweeps every surface angle past the light; plane shows the pattern + the seam undistorted. |
| `LitPreview::render(...)` | Declare the offscreen pass + composite it into the walker-reserved seat | **Borrows** the swatch's texture handles — uploads nothing. `Height` (terrain data) is deliberately not bound; `Emit` is. |
| `LitPreview::tick(dt)` / `free(r)` / `built()` | Advance the turntable · release the target · is-it-built | See **Known gaps** — `free` exists but nothing calls it. |
| `const STAGE_SOURCE` | `"sablework_lit"` | The stage block name; the look (lighting, framing) is DATA there, not constants in Rust. |

### `palette` — the emissive glow palette

| Item | What it is | The one thing to know |
|---|---|---|
| `struct GlowPalette` / `GlowColor { id, rgb }` | The ordered prefab glow colours from `palettes.json` | The array **index** is the identity — swatch order, nav order, and the pick index are all a position in this list. |
| `GlowPalette::load(data_dir)` | Load `palettes.emissive` | Unreadable/malformed ⇒ empty (warn, never panic). **No** Rust fallback table — that would be a second source of truth that drifts (AEEF2A68). |
| `GlowPalette::nearest(rgb) -> Option<usize>` | Nearest entry by squared distance | Ties resolve to the lower index; `None` for an empty palette. This is how Roll's random glow snaps back onto the palette. |
| `len` / `is_empty` / `get` / `iter` | Read the palette | — |
| `fn inject_glow_styles(ui_styles, palette)` | Write one literal-rgba style block per entry | Runs at `enter()` **after** token resolution, under `sablework.glowsw.<id>` (+`_sel`). The swatch colours are data, not authored theme. |

### `route` — the dispatcher

| Item | What it is | The one thing to know |
|---|---|---|
| `fn apply(bench, results) -> bool` | THE one place a control changes the recipe | Returns **true only when the recipe actually changed** — that boolean is what gates the re-bake. View changes, selection, binding, rung, rename and page switches all return false. |

## Interactions

### Signals it captures (by name — never keys, DFE3E44E / 37722F91)

- **Walker-owned** (the pump/walker feeds these; the scene never declares them):
  `NavUp/Down/Left/Right` across the flattened stop roster, `Confirm`/`Activate` on the
  focused control, `PanelNext`/`PanelPrev` between the top-tier panel stops, and slider
  value write-backs. A **pointer** click is a `Confirm` at the hit node.
- **Screen-declared intents** (data on the root — the scene captures the signal and
  fires a result *name*):

  | Signal captured | Result name fired | What it does |
  |---|---|---|
  | `Menu` | `pause_open` | Push the shell pause overlay |
  | `TabNext` / `TabPrev` | `map_next` / `map_prev` | Step the view ring (see the subtlety below) |
  | `PageNext` / `PagePrev` | `patch_next` / `patch_prev` | Step the **factory-patch** ring |

- **The ruled text-entry exception:** while a material **rename** is open, the bench reads
  raw `Enter` (commit), `Esc` (cancel), and typed text / backspace directly — the one place
  it touches hardware, because the signal bus carries no text-entry vocabulary (the
  Quartermaster precedent). Edge-detected so a hold does not re-fire. Everything else is
  signals.

### The one subtle concept — a rail owns its own step name

`map_next` / `map_prev` are **not** handled by [`route::apply`]. The authored view rail
(`sw_views`, with `next_action`/`prev_action`) steps its **own bound number** `sel_map`
*inside the walker*; the scene only reads the resulting echoed index. So a bumper
(`TabNext`), a gutter click, and a picked tab all move the one number, and the dispatcher
sees only `sel_map`. A dispatcher arm for `map_next` would be a **second** consumer —
+2 per press, the skipped-tab bug (MCP `801B1B09`), which a gate now forbids. If you are
looking for where `map_next` is handled, it is the rail, not `route.rs`.

### Results the dispatcher owns (all arrive in one `ValueMap`)

An **image edit** (re-bakes; `apply` returns true):

- **Rack**, per voice `n = 1..6`: `ch{n}_on` (toggle), `ch{n}_source` / `ch{n}_blend`
  (step the enum), `ch{n}_scale` / `ch{n}_octaves` / `ch{n}_warp` / `ch{n}_amount`
  (sliders). *Touching any control of a voice also selects it* (a view change, not a bake).
- **Output stage**: `relief`, `roughness`, `roughness_mod`, `metalness`, `metalness_mod`,
  `ao`, `emissive_strength`, `emissive_band`.
- **Tint knobs**: `tint_hue`, `tint_sat` — recolour the *whole* ramp (the ramp is the
  recipe's one colour representation; the knobs read and write it).
- **Glow pick**: `glow_pick_<i>` — write the i-th palette colour verbatim into the glow.
- **Selected-voice fine knobs**: `sel_lacunarity`, `sel_gain`, `sel_contrast`, `sel_invert`.
- **Whole-instrument**: `roll` (new instrument, seeded off the current seed), `reseed`
  (same rack, new performance), `patch_next` / `patch_prev` (factory ring).

**Not** an image edit (no re-bake — same pixels or view-only):

- `sel_map` (the bound view number; clamped into `0..VIEW_COUNT`), `lit_body`, `lit_spin`.
- `sel_material` (dropdown option index) and `mat_pick_<i>` (materials-list row) — both
  bind `recipe.material`; a bind changes identity, not the image.
- `rename` / `rename_commit` / `rename_cancel`, `bake_size` (steps offered rungs only),
  `commit` (starts one worker job; a second click while `Working` is ignored),
  `page_bench` / `page_materials` (the bench↔materials page switch; also closes any rename).
- `pause_open` — pushes `PauseScene`.

### Model keys

- **Published raw** by the behaviour (`hud_model`): the two cursors as numbers
  (`sel_ch`, `sel_map`, `sel_page`); per voice `ch{n}_on`, `ch{n}_source_label`,
  `ch{n}_fx` / `ch{n}_fx2` (the live effect blurb), `ch{n}_blend_label`, `ch{n}_scale`
  / `octaves` / `warp` / `amount`; the output stage + `tint_hue` / `tint_sat`; the glow
  (`glow_count`, `glow_sel`, `glow_sw{n}_id`); the selected voice
  (`sel_voice`, `sel_lacunarity`, `sel_gain`, `sel_contrast`, `sel_invert`); the readouts
  (`recipe_line`, `preview_info`, `bake_info`, `size_label`, `commit_status`); the
  material dropdown (`mat_opt_{i}`, `mat_row_{i}`, `sel_material`) and rename state
  (`renaming`, `material_bound`, `material_id_label`, `rename_draft`),
  `lit_body_label`, `lit_spin`. Numbers are pre-formatted in Rust; all copy is `$tokens`.
- **Derived** by `sablework.lua` `derive()`: `ch{n}_sty` (row wash off `sel_ch`),
  `{map}_shown` / `lit_shown` (exactly one view cell, off `sel_map`), `glow_sw{n}_sty`
  (swatch wash off `glow_sel`), `page_bench_shown` / `page_materials_shown` +
  `page_*_sty` (off `sel_page`), `not_renaming` / `can_rename`.
- **Rust-filled containers** (the ratified "Rust fills the static scene's container"
  pattern, at construction): `material_select` (one `option` per material, label-bound to
  `mat_opt_<i>`), `sw_glow_swatches` (one `button` per palette entry, firing `glow_pick_<i>`),
  `sw_mat_list` (one row per material, firing `mat_pick_<i>`).

### Style keys the scene names

`sablework.{bar, button, button_on, checkbox, col, frame, panel, select, slider, well,
row, row_sel}` and the injected `sablework.glowsw.<id>` / `<id>_sel`. Colours and
weights live in `ui_theme.json` / `ui_style.json` and the scene file's own `styles`
block (the five-line split); the `glowsw` blocks are injected from `palettes.json` at
`enter()`. The `stages.sablework_lit` block (lighting + camera for the lit view) is
authored in **`sablework.scene.json`** (a top-level `stages` block), merged over the
theme and compiled once at `enter()`.

### Threads / workers

All baking runs on a `flicker_worker::WorkerPool`. **Preview** bakes are
generation-counted — an edit bumps the counter, submits a job with a clone of the recipe,
and the newest result wins (stale ones are dropped, so a fast drag never queues a
backlog). **Commit** is one worker job at a time; its `CommitState` arrives on a channel
drained in `update`. The **lit** view declares an offscreen `FrameGraph` surface each
frame it shows, composited into the seat the walker reserved for the `sw_lit` node.

## Gates

`cargo test -p flicker-sablework` — the drift gates pin the three authored artifacts
(scene tree · pair script · Rust vocabulary) to each other, so a rename in one is loud in
the others. None needs a GPU. The load-bearing ones:

- **Authoring / fail-loud:** `the_shipped_scene_file_parses_with_a_tree_and_styles`,
  `the_hud_names_only_known_kinds`, `the_hud_ships_no_raw_display_literals`,
  `no_raw_display_copy_is_published_into_the_model`, `every_token_the_bench_draws_resolves`,
  `every_declared_intent_reaches_a_handler`, `every_swatch_style_path_resolves`.
- **The rail / anti-double-step:** `the_scene_never_hand_steps_a_rail_owned_name` (the
  skipped-tab fix, pinned at behaviour *and* source level).
- **Tree↔Lua↔Rust lockstep:** `the_view_tabs_cover_every_map_and_the_lit_view`,
  `every_view_cell_exists_and_is_visibility_gated`, `exactly_one_view_cell_is_shown`,
  `the_selected_voice_row_wears_the_wash`.
- **The dispatcher's contract:** `only_recipe_edits_ask_for_a_bake`,
  `rewriting_the_same_value_does_not_rebake`, `the_view_number_clamps_into_the_ring`,
  `touching_a_voice_selects_it`, `scale_is_always_a_whole_number_of_cells`,
  `every_knob_clamps_to_a_valid_recipe`, `the_pills_step_through_every_option_and_wrap`,
  `a_checkbox_can_turn_a_voice_off`, `the_patch_buttons_walk_the_library_both_ways`.
- **Material binding + rename:** `the_material_dropdown_binds_by_option_index`,
  `the_dropdown_reads_the_binding_as_a_specific_name`,
  `the_material_rename_edits_a_bound_name_and_persists`, `rename_needs_a_bound_material`,
  `the_byte_id_is_explicit_in_the_model`.
- **Commit:** `a_commit_starts_once_and_does_not_block`, `the_commit_status_is_never_blank`,
  `the_size_control_never_reaches_a_gated_rung`, `binding_and_size_are_not_image_edits`,
  and in `commit.rs`: `a_commit_writes_the_maps_and_the_recipe_in_package_layout`,
  `maps_stay_raw_and_the_recipe_lands_gz`, `the_committed_recipe_round_trips_and_rebuilds`,
  `everything_committed_classifies`, `a_commit_touches_only_the_staging_root_it_was_given`,
  `names_are_folded_into_the_asset_standard`,
  `maps_encode_narrow_and_decode_back_to_the_baked_values`.
- **Tint / glow palette:** `the_tint_knobs_recolour_the_base_map_gold`,
  `re_tinting_the_same_colour_does_not_rebake`, `the_glow_palette_loads_the_shipped_set`
  (8 colours, black excluded), `glow_nearest_picks_the_closest_entry`,
  `a_glow_pick_writes_the_exact_colour_and_re_bakes_once`, `roll_snaps_the_glow_onto_the_palette`,
  `stepping_patches_keeps_authored_glow_no_snap_on_load`.
- **Nav / layout:** `the_flattened_nav_topology_is_the_authored_surface` (six channels as
  direct stops, enter-depth ≤ 1, geometric adjacency over the resolved layout),
  `the_six_channel_cards_fit_the_rack`, `the_card_contents_flow_inside_the_tile`.
- **Materials page:** `the_page_switch_shows_exactly_one_page`,
  `the_materials_list_is_filled_from_the_index`, `the_materials_list_binds_by_row`,
  `the_materials_page_tokens_resolve`.
- **Lit view** (in `lit.rs`): `the_authored_stage_is_read_and_lights_the_sample`,
  `both_bodies_are_whole_triangle_soups`, `the_sphere_is_unit_with_outward_normals`,
  `the_turntable_wraps_and_ignores_frame_rate`, `the_body_toggle_round_trips`.

Run a real commit headless (no GPU):
`cargo run -p flicker-sablework --example commit_patches -- <staging_root> [size]` —
commits every factory patch and prints what each file classified as (what the Content
Manager's Type column will show). Defaults to the 2K baseline.

## Sharp edges

- **`sel_map` is the VIEW index, not a map index.** `0..6` are the flat maps in
  `MapKind::ALL` order; `7` is the LIT view. `selected_map()` returns a `MapKind` even on
  the LIT tab — it reports `BaseColor` there (the lit view's albedo), because the lit tab
  has no single map.
- **`PageNext`/`PagePrev` step *patches*, not the page.** The `sel_page` bench↔materials
  switch is driven by the `page_bench` / `page_materials` buttons (a `Confirm` on them),
  **not** by the `PageNext`/`PagePrev` signals — those step the factory-patch ring. The
  word "page" means two different things here; mind the gap.
- **Two ways to bind a material, on purpose.** The header **dropdown** binds by option
  index (0 = Unbound, i+1 = the i-th material; a value past the list clamps to Unbound);
  the materials-**page list** binds the same `recipe.material` by row. Quick pick vs.
  roomy browse — not a fork (the concept, a `u8` id, has one representation).
- **Unbound is a real state**, not an absence: a scratch surface whose identity is
  undecided. A rename needs a *bound* material (there is no byte to relabel otherwise).
- **The preview trails the slider by a frame or two** (worker bake) and is baked at
  `PREVIEW_SIZE` (256²) — the *same image* as the commit, sampled coarsely. A stale
  preview upload (size mismatch) is rejected with a warn rather than shown torn.
- **Committed maps use `flicker-texture`'s map roles**, including `<Asset>_Height.png` —
  a role the content map vocabulary does not list (see **Known gaps**).
- **4K/8K bake correctly but are off the picker** (a `flicker-texture` gate pending an
  engine memory budget); the size stepper only reaches *offered* rungs.

## Known gaps

- **The lit render target is not released on scene exit.** `LitPreview::free()` (which
  calls `free_render_target`) exists but nothing calls it, and `Sablework` implements no
  `Scene::exit`, so leaving the bench leaks the offscreen target — the exact case ruled
  by MCP `728E682F` (sibling scenes free theirs in `exit()`). The fix is to wire
  `exit()`; this README notes it so an operator debugging VRAM is not misled.
- **`<Asset>_Height.png` overreaches the content map vocabulary.** `commit` writes a file
  per `MapKind`, and `MapKind::role()` yields `Height`, but the fixed vocabulary in
  [`Alpha/content/README.md`](../../../content/README.md) has no `Height`. The role name
  is owned upstream by [`flicker-texture`](../../content/flicker-texture/README.md) (where
  it is already tracked); it is called out here because `staging/` is where a human first
  sees the file.
