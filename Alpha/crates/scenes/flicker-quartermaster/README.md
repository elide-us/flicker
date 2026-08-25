# flicker-quartermaster

The **Quartermaster Bench** — the content air-traffic controller. Ingest benches
(Clayworks, Loomforge, the retargeter) drop their processed output into `staging/`;
nothing the game reads changes until someone looks at it and **promotes** it into
`package/` (the tree the runtime loads). This crate is the scene where that happens: a
two-tab file manager over the two trees that also owns the package **manifest** (the
append-only ledger recording what was promoted). It is a *scene crate* — one screen the
`prism-alpha` launcher registers and runs — built on the canonical five-line UI
architecture, drivable from a controller on a single focus.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

A few flicker words this README leans on:
- **staging / package** — the two content tiers. `staging/` holds an ingest bench's raw
  output; `package/` is the only tree the engine reads at runtime. **Promote** = move an
  asset from the first to the second and record it.
- **Model** — the per-frame `key → value` table the behaviour publishes and the pair
  script + drawing layer read.
- **pair script** — `quartermaster.lua`, the scene's LOGIC half; it *derives* gates and
  style paths from the raw Model the Rust behaviour publishes.
- **signal / intent / result** — an `ActionSignal` (e.g. `Confirm`, `Undo`) is the
  trigger-agnostic name for an input; a component *captures* the signals it cares about
  and the capture IS the intent (there is no separate router). Inside this bench every
  captured signal, pointer click and key edge is folded into one flat **result** name
  (`"undo"`, `"confirm"`, …) that a single dispatcher consumes.
- **walker** — the shared focus/pointer handler (`flicker-widgets`) that owns
  `Nav*`/`Confirm`/`Cancel`/`Panel*` and turns a pointer click into a spatially-targeted
  `Confirm`.
- **slot bank** — a fixed run of authored rows (`row_slot_0..11`, …); the behaviour
  *windows* an unbounded filesystem listing into them, since the walker has no repeater.
- **token** — a `$name` the stringtable resolves to localized display text; the Model
  carries `$tokens` and style *paths*, never English or rgba.
- **realm** — a launcher grouping; this bench is registered under the Developer realm.

## Where it sits

- **Builds on:**
  - `flicker` (umbrella) — `ui` (walker, `SceneDef`, `run_ui`, `render_hud`, the drift
    gates), `render`, `scene` (`Scene`/`Transition`), `script` (`ScriptHost`,
    `ValueMap`), `ui::{Command, CommandHistory}` (the undo spine).
  - `flicker-content` — the content domain: `roots()` (package/staging resolution),
    `classify_package`/`PackageClass`, the reversible file ops (`BatchFileOp`, `FileOp`,
    `probe_conflicts`, `keep_both_name`, `occupied`), the `manifest` module, and
    `baseline::CANON_BONES`. See `../../content/flicker-content/README.md` — it documents
    the promotion primitives and the manifest format this bench writes.
  - `flicker-input-core` (`ActionSignal`, `InputState`), `flicker-input-router`
    (`Router`, `InputHandler`) — the pump this scene consumes signals from.
  - `flicker-shell` (`PauseScene`, `Theme`) — the pause overlay pushed on `Menu`.
- **Used by:** `prism-alpha` — `main.rs` registers
  `SceneEntry::new("quartermaster", …, flicker_quartermaster::scene).with_realm(REALM_DEVELOPER)`.
  This crate exposes nothing else to the workspace.
- **Reads from the content tree:**
  | Path | When | If missing |
  |---|---|---|
  | `Alpha/content/sensorium/scenes/quartermaster.scene.json` | compiled in via `include_str!` (the test/`with_roots` copy); the runtime receives the same file through the manifest `SceneDef` | `with_roots` panics ("the shipped scene file parses"); at runtime `new` logs an error and draws nothing |
  | `Alpha/content/sensorium/scripts/quartermaster.lua` | compiled in via `include_str!` | logs an error and degrades to raw Model values (no derived style paths / page gates) |
  | `<content_root>/package/` and `<content_root>/staging/` | every `refresh` (listing + tree + queue) | an unreadable folder lists EMPTY rather than failing |
  | `<content_root>/package/manifest.json` (gz-at-rest) | appended on promote, rewound on undo-of-promote | `append` failure rolls the moved files back so ledger and tree never disagree |

The two roots come from `flicker_content::roots()`, which resolves the executable's
declared `content_root` (`content.json`); `package`/`staging` are derived, not separately
configured.

## Public API

### Scene entry point

| Item | What it is for | The one thing to know |
|---|---|---|
| `fn scene(def: &SceneDef) -> Box<dyn Scene>` | The factory the launcher roster registers | Wraps `Quartermaster::new(def)` |
| `struct Quartermaster` | The bench itself (implements `Scene`) | Holds all state; the runtime drives it through the `Scene` trait |
| `Quartermaster::new(def: &SceneDef)` | Runtime constructor — the manifest hands in the authored `SceneDef` | Uses `Roots::from_config()` |
| `Quartermaster::with_roots(roots: Roots)` | Test seam: browses explicit roots on the SHIPPED scene file | Parses the compiled-in `quartermaster.scene.json`, so tests exercise the same authored tree the runtime gets |
| `Quartermaster::default()` | `with_roots(Roots::from_config())` | — |

### The promote / review surface (the mission)

| Item | What it is for | The one thing to know |
|---|---|---|
| `promote_selected(&mut self)` | Promote the selected staged asset into `package/` | Moves the asset's WHOLE folder to the mirrored path AND appends the manifest row as **one** history entry; a target collision raises the shared prompt. **ALL-OR-NOTHING** — see Sharp edges. `&mut self` + queue/undo-entangled, so **not** callable headlessly (use `flicker-content`'s manifest primitives for scripted promotion) |
| `refresh_queue(&mut self)` | Re-scan the staging tiers for promotable assets | Selection survives by asset dir where it still exists |
| `selected_queue_item(&self) -> Option<&QueueItem>` | The asset every Review verb acts on | The single Review focus |
| `facts_warning_tokens(&self) -> Vec<&'static str>` | The selected asset's warning `$tokens` | Real facts, worst first: `$qm_warn_off_canon` (bone count ≠ `baseline::CANON_BONES`), `$qm_warn_missing_textures` (a listed beside-rig file absent), `$qm_warn_target_occupied` (promote will collide) |
| `last_error_token(&self) -> Option<&str>` / `last_error(&self)` | The last mutation's outcome for the status line | A `$token` or measured text, never composed copy |

### Navigation (single-focus controller baseline)

| Item | What it is for | The one thing to know |
|---|---|---|
| `confirm(&mut self)` | Open the focused folder / activate | On the tree, both opens the branch and navigates into it |
| `cancel(&mut self)` | Collapse a branch, else climb one level | Stops at the roots |
| `up(&mut self)` | Climb to the parent folder | Never above a root; lands the cursor on the folder you left |
| `nav(&mut self, delta: isize)` | D-pad within the focused pane (±1) | Clamps; scrolls the focused row into view |
| `focus_pane(&mut self, forward: bool)` | Left stick: move between panes (Tree ↔ List) | Two panes, so forward and back are the same hop |
| `cycle_tab(&mut self)` | L1/R1: Review ↔ Files | — |
| `open_dir(&mut self, dir: PathBuf)` | Move the listing into `dir` | No-op if `dir` is not a directory; resets the cursor |
| `sort_by(&mut self, key: SortKey)` | Re-sort the listing | Asking for the current key flips direction (column-header semantics) |
| `refresh(&mut self)` | Re-read listing + tree + queue from disk | Keeps focus on the same path where it still exists |
| `tab` · `pane` · `cwd` · `rows` · `selected` · `tree_view` · `sort` | Read accessors for tests and the Model | — |

### Mutation (Files tab — move-only)

| Item | What it is for | The one thing to know |
|---|---|---|
| `cut(&mut self)` | Pick the focused item up | There is no copy — this manager only relocates; single-item clipboard |
| `paste(&mut self)` | Put the clipboard down (into the focused folder, else `cwd`) | A collision opens the prompt; pasting where it already lives is a no-op (no history entry); a folder cannot be pasted into itself |
| `create_folder(&mut self)` | New folder in `cwd` | Picks the next free "New Folder" name; lands focused |
| `clipboard(&self)` · `can_undo` · `can_redo` | Clipboard / history state | — |
| `undo(&mut self)` / `redo(&mut self)` | Step the command history | Every mutation is exactly one entry — a 40-item batch is one undo |
| `begin_rename` · `is_renaming` · `rename_draft` · `type_into_rename(typed, backspace)` · `commit_rename` · `cancel_rename` | The inline rename | First typed char REPLACES the name (pristine-replace); an unusable name blocks the commit and keeps the draft |
| `open_menu` · `is_menu_open` · `close_menu` | The context menu (state = whether it is up + the pad cursor) | The authored tree centres and gates it; **opening it from input is currently unwired** — see Interactions |
| `is_prompting` · `prompt_conflict` · `prompt_remaining` · `toggle_apply_rest` · `resolve_conflict(how)` · `cancel_conflict` | The collision prompt | Resolutions accumulate into ONE batch; `cancel_conflict` keeps the clipboard loaded |
| `toast_count(&self) -> usize` | Live confirmations | Capped at 3 (`TOAST_SLOTS`), oldest dropped, ~8 s dwell |

### The frame seam

| Item | What it is for | The one thing to know |
|---|---|---|
| `apply_results(&mut self, results: &ValueMap)` | **THE ONE dispatcher** — folds a frame's result names into state | A pad chord, a key and a click all arrive here as the same result name; the precedence ladder (prompt → menu → rename → tabs → nav → verbs) lives here |

`Quartermaster` also implements `Scene` (`enter`/`update`/`render`); the runtime calls
those — you do not.

### `fs_model` — the headless navigation + queue model

Split out so the whole browse/queue model is testable without a window. All `pub`:

| Item | What it is for |
|---|---|
| `struct Row` · `fn list_dir(dir, sort, descending) -> Vec<Row>` | One classified listing row (`path` is LOGICAL); a directory listing, folders first |
| `enum SortKey { Name, Type, Size }` (+ `id()`) | The sort keys; `Name` is default |
| `struct TreeRow` · `fn tree_rows(&Roots, &[PathBuf]) -> Vec<TreeRow>` | The folder tree flattened to display rows; descends only into opened branches |
| `struct Roots { package, staging }` (+ `from_config`, `all`) | The two browsed roots |
| `fn breadcrumb(&Roots, dir) -> Vec<String>` | The crumb trail relative to the containing root |
| `fn parent_within_roots(&Roots, dir) -> Option<PathBuf>` | The "up" target; `None` at a root |
| `const ITEM_ROOTS: [&str; 4]` | The four promotable tiers — `["characters", "props", "materials", "retarget/clips"]` — mirroring the ingest benches' commit roots |
| `struct QueueItem` · `fn staging_queue(&Roots) -> Vec<QueueItem>` | One promotable asset FOLDER per entry, tier-ordered; classified off its primary json |
| `fn files_under(dir) -> Vec<PathBuf>` | Every file under a folder (the dependencies that travel on promote), sorted |
| `fn logical(physical) -> PathBuf` | Drop the at-rest `.gz` so callers speak the loaders' names |

## Interactions

### Signals it captures

Per the ratified signal contract (MCP `C2C98408`), all input is expressed as
`ActionSignal`s — never keys or buttons (DFE3E44E). Two channels reach the one
dispatcher:

**Declared intents on the screen root** (`on_<signal>` in `quartermaster.scene.json`, an
intent capture — there is no separate intent router):

| Signal | Fires the result | Effect |
|---|---|---|
| `Menu` | `pause_open` | Push the pause overlay |
| `TabNext` / `TabPrev` | `tab_next` / `tab_prev` | Review ↔ Files |
| `Undo` / `Redo` | `undo` / `redo` | Step the history |
| `Cut` / `Paste` | `cut` / `paste` | Move-clipboard |
| `Rename` | `rename` | Open the inline rename on the focused row |
| `CreateFolder` | `create_folder` | New folder in `cwd` |
| `ContextMenu` | `menu_open` | Open the context menu on the focused item |

**Walker-owned signals** — NOT declared anywhere (the drift gate asserts their absence).
The bench keeps its single-focus cursor scene-side (no `tab_group`s), so these pass the
walker to `QuartermasterBase`, which converts them to result names:
`NavUp/Down/Left/Right → nav_up/down/left/right`, `Confirm → confirm`, `Cancel → cancel`,
`PanelNext/PanelPrev → panel_next/panel_prev`. Because a pointer click is a
spatially-targeted `Confirm` (37722F91), every on-screen control — the tab buttons, sort
headers, the PROMOTE control and queue rows, the conflict-dialog and context-menu buttons
— activates through this channel too.

**The one ruled raw exception:** while a rename is open, `Enter`/`Esc` are read directly
(→ `rename_commit`/`rename_cancel`), because the `TextEntry` context ships an empty
binding map by design, so the intent channel is deliberately unavailable there.

> ### ⚠ Caveat — the seven editor-verb captures fire for nobody today
>
> `on_undo`, `on_redo`, `on_cut`, `on_paste`, `on_rename`, `on_create_folder` and
> `on_context_menu` are declared and resolve as valid signal names (so no gate warns),
> **but no shipped input profile can currently produce any of these seven signals.** Six
> of them (`Undo`…`CreateFolder`) live only inside `chord::editor_chords()`, and no
> profile installs `InputContext::Chord` nor drives `ChordLayer::update`; `ContextMenu` is
> bound nowhere at all. So from real hardware these captures never fire — and because the
> context menu is the *only other* door to cut / paste / rename / new-folder, that whole
> editor-verb surface is gated behind the same unwired layer. See MCP incidents
> `A50A2ABA` and `01BB4228`, and `../../input/flicker-input-core/README.md`.
>
> This is **unfinished wiring, not dead code** (F42DA5E0): every method behind these verbs
> is fully implemented and covered by passing tests, and is reachable in tests via
> `apply_results` with the result name or a direct call. The fix direction is to install
> and drive the chord layer and bind `ContextMenu` (+ the text pair) at the *profile*
> layer — bindings are out of scope of the bench itself (DFE3E44E) — which lights all
> seven up. What DOES work from a shipped profile today: tabs, pane focus, selection,
> Confirm/Cancel, pause, the wheel, and every on-screen control (PROMOTE included) via the
> pointer.

### Results it fires / routes

`apply_results` consumes a flat result vocabulary folded from both channels above plus the
walker's click results: navigation (`nav_*`, `panel_*`, `confirm`, `cancel`, tab ids),
the editor verbs (`undo`/`redo`/`cut`/`paste`/`rename`/`create_folder`), the menu
(`menu_open`/`menu_dismiss`), the rename edges (`rename_commit`/`rename_cancel`), the
conflict answers (`conflict_replace`/`conflict_keep_both`/`conflict_skip`,
`conflict_apply_rest`), and the Review actions (`promote`, `rv_pick_slot_<s>`) plus the
per-slot pick/open names for the windowed banks (`row_pick_slot_<s>`,
`tree_open_slot_<s>`). Externally the only transition it returns is `Transition::Push` of
a `PauseScene` on `pause_open`.

### Model keys

The behaviour PUBLISHES raw state each frame (`hud_model`); the pair script
(`quartermaster.lua`) DERIVES presentation from it. Owner of the raw keys = this crate;
owner of the derived keys = the pair script.

- **Published (raw):** `tab`, `pane`, `menu_sel`, `queue_len`, `has_rename`/`has_menu`/
  `has_prompt`/`has_clip`/`has_error`, `crumbs`, `count`, `clip_name`, `can_undo`,
  `rename_draft`, `error`; the windowed banks `row_slot_<s>_{on,name,type,size,color,sel,
  dir,file}` (12), `tree_slot_<s>_{on,text,sel}` + `tree_line_<s>_off_x` (12),
  `rv_slot_<s>_{on,name,meta,color,sel}` (8); the selected asset `rv_{class,files,size,
  target}` + `rv_warn_<i>{,_on}`; the collision prompt `conflict_{name,where,existing,
  incoming,multi,rest}` + `conflict_apply_rest`; the toast bank `toast_<i>_{on,label,
  name}` (3).
- **Derived (by `quartermaster.lua`):** the page gates `files_on`/`review_on`/`rv_empty`,
  the pane/tab style paths `*_pane_sty`/`tab_*_sty`, the per-slot washes
  `{tree_slot,row_slot,rv_slot}_<s>_sty`, and the menu highlight `menu_sty_<i>`.

Colours cross the Model as dotted STYLE PATHS (`quartermaster.class.<id>`), never rgba, so
`ui_theme.json` stays the one palette (8D8A4215).

### Threads / workers

None — everything is synchronous. Asset facts are parsed once per selection (a per-click
cost, never per-frame); the queue and listing are re-scanned on `refresh`.

## Gates

`cargo test -p flicker-quartermaster` (45 tests; the shared-tree rule forbids a
workspace-wide run here). The load-bearing drift gates:

| Test | What breaks it |
|---|---|
| `the_tree_passes_the_drift_gates` | An unknown component kind, a raw display literal in the tree, raw copy published into the Model, the declared intents drifting from the ratified contract, a walker-owned signal declared on any node, or the pair script failing to load/derive |
| `every_token_the_screen_ships_resolves` | A `$token` with no stringtable entry survives to the draw boundary (a typo'd token would ship as raw `$qm_…`) — the fail-loud gate (4BB12A75) |
| `hud_routes_clicks_on_the_slot_banks_and_tabs` | A slot target renders text but takes no click (zero-extent overlay child — the twice-burned regression) |
| `tree_carets_and_indent_follow_the_ruling` | The caret glyph set (`^` collapsed · `>` expanded · `·` leaf) or the 14 px/level indent changes |
| `promote_moves_the_asset_appends_the_manifest_and_one_undo_returns_both` | Promote fails to move the whole folder + append the row as one entry, or undo fails to return both |
| `a_promote_onto_an_occupant_prompts_and_replace_ships_the_whole_asset` / `a_partial_conflict_answer_refuses_the_promote` | The all-or-nothing promote contract |
| `the_queue_reads_facts_warns_honestly_and_selection_survives_refresh` | Warnings become placeholders, or a refresh moves the cursor |
| `a_full_move_and_undo_runs_on_a_single_focus` · `a_colliding_paste_raises_the_prompt_rather_than_replacing_silently` · `replace_displaces_the_occupant_and_undoes_cleanly` · `keep_both_lands_beside_the_occupant` · `skip_moves_nothing_and_records_nothing` | The move + conflict-resolution spine |
| `the_first_typed_character_replaces_the_name` · `an_unusable_name_blocks_the_commit_and_keeps_the_draft` · `a_rename_swallows_every_other_intent` | Inline-rename behaviour |
| `the_prompt_swallows_every_other_intent` · `a_prompt_raised_from_the_menu_outranks_it` · `the_menu_survives_idle_frames_and_closes_on_a_real_pick` | The modal precedence ladder |
| `the_toast_bank_never_outgrows_the_protos_slots` · `a_toast_expires_after_its_dwell` | Toast capacity + dwell |
| fs_model: `a_listing_shows_logical_names_not_the_at_rest_gz` · `folders_sort_first_under_every_key_and_direction` · `the_trash_is_never_listed_or_walked` · `the_tree_shows_both_roots_and_descends_only_where_opened` · `up_stops_at_the_roots` · `an_unreadable_folder_lists_empty_rather_than_failing` | The headless browse model |

## Sharp edges

- **The seven editor-verb captures are unwired from input today** — see the Interactions
  caveat above. Their behaviour is reachable only via `apply_results`/direct calls until a
  profile binds them.
- **Promote is ALL-OR-NOTHING.** An asset's files are dependencies that travel together;
  on a promote collision, only a full Replace ships. Any Skip or Keep-both makes the
  promote partial and it refuses out loud (`$qm_err_promote_partial`) — nothing moves.
- **`promote_selected` is not headless.** It is `&mut self` and entangled with the queue,
  toasts and undo history. For scripted / CLI promotion use the primitives in
  `flicker-content` (`manifest::{append, read, remove, ManifestEntry}` + a folder move);
  see `../../content/flicker-content/README.md`.
- **Mutation verbs act on the LIST pane's cursor only.** `cut`/`paste`/`begin_rename` read
  the listing selection and do nothing while the TREE holds focus (acting on a tree folder
  is a deliberate later capability, never a silent fallback).
- **`Row.path` is LOGICAL** — the at-rest `.gz` is dropped. Carrying the physical `.gz`
  makes `dst == src` comparisons silently fail (a rename to the same name reads as "name
  taken"). The manifest's `path`/`promoted_from` are likewise forward-slash logical paths.
- **`.trash` is never listed or walked** — it is the undo machinery's parking space; a
  Replace parks the occupant there (recoverable), which is what keeps Replace revertible.
- **Rename pristine-replace:** the text field has no selection model, so the first typed
  character clears the draft (what select-all-then-type would do); backspace edits the
  existing name instead.
- **`refresh` survives a folder vanishing under it** — an unreadable directory lists empty
  rather than erroring; the cursor stays on the same path where it still exists.
- **Slot-bank capacities are stated in Rust AND the scene** — `LIST_SLOTS`/`TREE_SLOTS`
  = 12, `QUEUE_SLOTS` = 8, `TOAST_SLOTS` = 3 must match the `*_slot_<s>` banks the
  authored `quartermaster.scene.json` spells out.
- **A missing/throwing pair script degrades loudly** — the bench falls back to raw Model
  values (no derived style paths or page gates) and logs the error via `tracing`.

## The scene pair

The tree and logic are authored, not hand-composed here (five-line architecture,
491BD9BB):

- `Alpha/content/sensorium/scenes/quartermaster.scene.json` — the tree, anchors, slot
  banks and declared intents.
- `Alpha/content/sensorium/scripts/quartermaster.lua` — the derive logic (page gates +
  style paths).

For how those files are authored — the scene format, `$token` stringtable, `ui_theme.json`
palette and `ui_style.json` weights — see `../../../content/sensorium/README.md`. This
bench is the reference implementation of the raw-Model + `derive()`-style-paths +
windowed-slot-bank pattern; `flicker-clicktrainer` (`../flicker-clicktrainer/README.md`)
is the sibling reference for a pure-2D pump chain.
