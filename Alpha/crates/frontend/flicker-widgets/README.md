# flicker-widgets

The engine's **UI toolkit**: the Rust **component walker** that lays out, draws and
hit-tests every control itself, the kind vocabulary it enforces, the scene-file loader,
the one stage compiler, the input-bus adapter, the theme/stringtable loaders, and the
Lua HUD draw bridge. It is the frontend seam every client and scene crate consumes; the
`flicker` umbrella re-exports it as `flicker::ui`. If a screen is on screen in Prism, this
crate drew it.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## The words this README uses

These are flicker terms, not general programming ones — each is defined once here:

- **Model** — the per-frame `key → Value` table (`flicker_script::ValueMap`) the scene
  hands the walker; a node's `bind` reads a value out of it, the walker writes results back
  into a fresh one.
- **bind** — a node prop naming a Model key: `bind` for a control's value, the `*_bind`
  family (`text_bind`, `visible_bind`, `enabled_bind`, `color_bind`, …) for one display
  facet. A bind names a key; the key's value rides the Model.
- **signal** — a resolved, device-agnostic input event (`flicker_input_core::ActionSignal`:
  `Confirm`, `Cancel`, `NavUp`, `Menu`, …). Nothing in Prism wires to a key or a button;
  everything wires to a signal. This crate names signals only, never the keys behind them.
- **intent / result** — a fired **result name** (a string like `pause_open` or `start`).
  A screen root declares `on_<signal>: "<result>"`; when that signal fires, the result name
  joins the drain. A click on a `button` fires its `action` name the same way. **Signals ARE
  intents** — there is no separate intent router.
- **exit** — a scene-file line mapping a fired result name to another scene (`"done" →
  CeLogo`). Routing is data; behaviour is Rust.
- **surface** — a drawing region. The root surface is the screen; a nested `surface` node
  reserves a rect the scene fills with an offscreen 3D pass (a globe, a bench viewport).
- **stage** — the authored recipe (lighting · clear · camera · layers · passes) a nested
  surface renders, named by the node's `source` prop and compiled by this crate.
- **walker** — the layout/draw/hit engine (`run_ui`) that consumes a node tree each frame.
- **section** — a screen subtree gated by a `visible_bind` (a dialog, a settings pane); a
  scene drives them as data through `Sections` instead of hand-rolled show/hide.
- **pair script / token** — a scene's Lua half (`tree()` + logic); a `$token` is a
  stringtable ref (`$menu_quit`) or a palette ref (`$sap_base`), resolved before draw.

For how a human **authors** scenes, trees and stages, see
[`../../../content/sensorium/README.md`](../../../content/sensorium/README.md) (the scene
format) and [`../../../content/sensorium/STAGES.md`](../../../content/sensorium/STAGES.md)
(stage recipes). This README documents the Rust API; it does not re-teach authoring.

## Where it sits

- **Builds on:** `flicker-render` (the draw calls + the typed `StageDef`/`Rate`/pass
  vocabulary the stage compiler produces), `flicker-script` (the Lua↔Rust seam: `UiNode`,
  `ValueMap`, `HudCommand`), `flicker-scene` (the stack machine — the scene loader speaks its
  `GotoMode`/`Transition` so the authored vocabulary can't fork), `flicker-input-router` +
  `flicker-input-core` (the event bus the walker is one layer of), `flicker-core::roots`
  (the one content-root knob).
- **Used by:** every scene crate (`flicker-populous`, `flicker-godmode`, `flicker-loomforge`,
  `flicker-pocclusters`, …), `flicker-shell`, and `prism-alpha`. They call `run_ui`, build
  on the component kinds, and drain the walker's results.
- **Reads from the content tree** (paths resolve through `flicker-core::roots`, not a
  manifest climb):
  | Path | When | If missing |
  |---|---|---|
  | `sensorium/resources/ui_theme.json` | `load_styles*` / `load_ui_json*` | empty object → walker falls to neutral fallback colours |
  | `sensorium/resources/ui_stages.json` (satellite) | merged during load | skipped (no stage library / lighting presets) |
  | `sensorium/resources/ui_style.json` (satellite) | merged during load | skipped (no global weight/effect defaults) |
  | `sensorium/scenes/<Name>.scene.json` | `SceneManifest::load_dir` | the folder listing IS the roster; a broken member fails the whole load |
  | `data/stringtable.json` | `strings::load_str` (called by the client) | `$token`s render RAW and warn once |

## Public API

### The walker — `run_ui` and its frame

The one entry point. `run_ui(tree, model, styles, input, state) -> UiFrame` lays the tree
out, draws + hit-tests every control, and returns the draw commands and results. Every
control draws AND hit-tests inside this call — there is no other tier.

| Item | What it is | The one thing to know |
|---|---|---|
| `run_ui` | one frame of layout + draw + hit | pure over `(tree, model, styles, input)` except the retained `state`; call once per walker pass |
| `UiInput` | the per-frame pointer/keyboard snapshot | `mouse`, `clicked` (edge), `down`/`right_down` (held), `screen`, `typed`, `backspace`, `wheel`, `exclusive`, `motion` — scenes wire their engine snapshot straight in |
| `UiState` | retained interaction state, held across frames | one per walker pass (a scene running a HUD + a chat panel holds two); owns focus, drag capture, the draw cache, flashes, the pane enter-stack |
| `UiFrame` | the output | `commands` (→ `render_hud`) · `results` (Model out) · `surfaces` · `pointer` · `rects` · `stats` |
| `UiFrame::rect(id)` | the resolved rect of any id'd node | `Some` of zero extent = a control that exists but can't be seen or clicked — what a scene gate asserts against |
| `UiFrame::surface_rect/surface_slot/surface(id)` | a nested surface's reserved rect/layout/slot | the walker RESERVES, the scene FILLS; `None` = off screen (skip the pass) |
| `UiFrame::surface_pointer(id)` / `root_pointer()` | this frame's pointer sample for a surface | the live-scene barrier — a behaviour reads this, never the device |
| `SurfaceSlot` | one nested surface's reserved slot | `id` · `source` (which `stages.<source>`, empty = behaviour fills its own) · rect · `rate` · `tint` · `layout` |
| `SurfacePointer` | the pointer sample a surface element receives | UI captures first, then pass-through; `captured` holds until both buttons release |
| `UiStats` | redraw accounting | `redraw_nodes` / `nodes` — a still frame is zero |
| `DragPayload` | what a drag-source picked up | `kind` + `id`; the walker only carries it, the scene decides what a drop means |
| `UiState::request_focus/clear_focus/focused` | keyboard focus by node id | `run_ui` clears focus at the top of any *clicked* frame; re-assert before `run_ui` to keep it |
| `UiState::flash/flash_intensity` | press-feedback glow | lit wherever an action fires (click, pad Confirm, declared signal); a matching `button` reads the intensity back |
| `UiState::nav_mode` | last input was pad/keys, not the pointer | the value a scene publishes (e.g. `pad_mode`) to swap device hint labels |
| `BtnSize`, `SIZE_SM/MD/LG` | the button size ladder | a node's `size_class` picks a rung; an explicit `height`/`label_size` still wins |

### The kind vocabulary

The complete set of `component` kinds a tree may name, and the drift gates that keep an
authored name from failing to a blank panel.

| Item | What it is | The one thing to know |
|---|---|---|
| `rust_component_kinds()` | the roster of interactive Component kinds the engine draws | THE single source of truth; the Component Catalog derives its required demo cards from here |
| `is_known_kind(kind)` | is `kind` any legal kind (structural + component) | the union is the whole vocabulary; anything else is a typo |
| `unknown_kinds(tree)` | every kind in a tree the engine does not know, deduped | a screen's test asserts this is empty, so a stale/typo'd kind fails the build, not the frame |
| `raw_display_literals(tree)` | every raw (non-`$token`) display string in a tree | a shipped screen asserts this empty, so hardcoded English fails the build; new copy goes to the stringtable |

The structural kinds (`surface`, `cell`, `row`, `stack`, `grid`, `text`, `option`) are laid
out by the walker itself. The interactive Component kinds — `button`, `panel`, `sprite`,
`tooltip`, `checkbox`, `toggle`, `radio`, `tile`, `pill_toggle`, `tabs`, `select`, `slider`,
`stepper`, `text_field`, `list`, `context_menu`, `gauge`, `resource_gauge`, `stat_dot`,
`action_slot`, `medallion`, `badge`, `popup_panel`, `paged_menu`, `nav_footer` — each own a
`draw_<kind>` arm and hit logic in `component.rs`. A new control is a new arm plus a new
entry in the roster; there is no other tier to put one in.

### The input-bus adapter — `WalkerHandler`

Makes the walker one layer of the `flicker-input-router` event bus. Constructed each frame
from the retained `UiState` and this frame's `hud_hit`.

| Item | What it is | The one thing to know |
|---|---|---|
| `WalkerHandler::hud(ui, consumed_pointer)` | the base adapter | not navigable until `.with_nav(...)`; `consumed_pointer` is `run_ui`'s `hud_hit` (OR-folded across passes) |
| `.with_nav(tree, model)` | make it navigable | pass the SAME tree + model `run_ui` walked, so nav and draw agree on what is on screen (a hidden subtree contributes no focusables) |
| `.with_rects(rects)` | patch this frame's resolved rects on | enables GEOMETRIC directional nav; absent rects fall back to the ordinal ring |
| `.with_intents(intents)` | bind the screen's declared `on_<signal>` intents | a declared binding may NOT name a walker-owned signal (see Interactions) |
| `take_fired()` | **the ONE activation drain** | drains declared-intent fires + pad `Confirm` activations + pointer activations, decays flashes, steps option strips — a navigable screen MUST call this once per frame or pad activation silently dies |
| `cancelled()` | `Cancel` was consumed this frame | the scene pops its modal / backs out |
| `apply_focus(change)` | write the router's focus decision through to `UiState` | one focus id for pointer AND pad |
| `focusables_of(tree, model)` | flatten a tree's focusable nodes | an invisible node prunes itself and its subtree |
| `walker_owned(signal)` | is `signal` one the walker owns on every screen | the nav family + `ChordBegin`; the bumpers (`TabNext`/`TabPrev`) are deliberately NOT owned |

### Declarative intents — `UiIntents`

| Item | What it is | The one thing to know |
|---|---|---|
| `UiIntents::of(tree)` | collect the screen ROOT's `on_<signal>` props | scans the ROOT ONLY; build once per tree build and retain — never per frame |
| `result_for(signal)` | the declared result name for a signal | `None` if the screen didn't bind it |
| `mirror_into(m, fired)` | republish each fired name as `sig_<name> = true` | TRANSIENT — rides exactly one Model publish, then gone; latch it yourself if you need it longer |

### Scene files + manifest — `SceneDef` / `SceneManifest`

The `<Name>.scene.json` reader and the folder index. The folder listing IS the roster;
nothing in Rust names a scene.

| Item | What it is | The one thing to know |
|---|---|---|
| `SceneDef::parse(id, text)` / `from_json` | read one scene file | fails LOUD on every authored name — bad kind, stale `template`/`slots` key, unspellable `mode`, restated `id`, unknown top-level key |
| `SceneDef` fields | `id` · `behaviour` · `params` · `boot` · `tree` · `exits` · `styles` | `id` is the FILE NAME, never restated in the document; `tree` is `None` for a Rust/immediate-mode scene |
| `SceneDef::exit(result)` | a fired result → a `Transition::Goto` | `None` is ordinary — most results the scene handles itself |
| `SceneDef::targets()` | every `(result, target id)` | the client's hook to gate that each target resolves in its roster |
| `SceneDef::stages()` | the scene's own top-level `stages` section | folded under `styles`; a scene NAMES a lighting preset, never authors one |
| `SceneDef::param_str(key)` | one string param | the shape a behaviour reads its config through |
| `SceneManifest::load_dir(dir)` / `from_files` | index the scenes folder | exactly one scene must claim `"boot": true`; zero or two is loud |
| `SceneManifest::get/boot/ids/scenes/len` | roster access | iteration is sorted (stable across machines) |
| `scene_id_from_file_name`, `SCENE_FILE_SUFFIX` | the naming convention | `TegLogo.scene.json` → `TegLogo` |

### The stage compiler — `stages`

The ONE compiler from `stages.<source>` JSON to the typed `flicker_render::StageDef` every
surface filler consumes. It replaced three private parsers. Every authoring problem is
returned as data and gated on shipped content.

| Item | What it is | The one thing to know |
|---|---|---|
| `stage_def(styles, source)` | compile one source, warning each problem | `None` = `stages.<source>` unauthored (also warned) |
| `compile_stage(styles, source)` | compile + return every PROBLEM as `Vec<String>` | the gate's entry point; values still degrade to defaults — a bad file costs the look, never the picture |
| `stage_defs(styles)` | every authored source, keyed by name | skips `lighting` and `_`-comments (see `is_source_key`) |
| `lighting_preset(styles, name)` | one `stages.lighting.<name>` `LightRig` | the shared library presets |
| `compile_rate`, `is_source_key` | rate sugar; source-key filter | `rate` / `live` / `live_bind` control how often a surface re-renders |

### Theme, styles and the draw bridge

Two exposure paths over one pipeline: `load_styles*` returns the resolved tree the Rust
walker resolves `style` paths against; `load_ui_json*` hands the same tree to Lua as the
`UI` global. Both merge satellites, then scene blocks, then resolve `$token`s against the
one palette (`theme.tokens`). Pick the cell that matches your consumer and input:

| | returns `Value` (for the Rust walker) | sets Lua `UI` global |
|---|---|---|
| **from a disk path** | `load_styles` / `load_styles_for` | `load_ui_json` / `load_ui_json_for` |
| **from embedded `&str`s** | `load_styles_str` / `load_styles_strs` / `load_styles_strs_for` | `load_ui_json_str` / `load_ui_json_strs` / `load_ui_json_strs_for` |
| **through the shared theme path** | `load_shared_styles` | `load_shared_ui_json` |

The `_for` variants take the scene's own `styles` blocks (the five-line split); `shared_theme_path()`
resolves `<content_root>/sensorium/resources/ui_theme.json`. A satellite or scene block
carrying a `theme` key is refused loudly (the one-palette guard, rule 8D8A4215).

| Item | What it is | The one thing to know |
|---|---|---|
| `render_hud(renderer, commands, white, textures)` | draw a `HudCommand` list | each command's `layer` is RELATIVE to the renderer's base layer, so a script stacks sub-layers without knowing its scene depth |
| `shared_theme_path()` | the one theme path | replaces the ~11 per-crate manifest climbs |

### The stringtable — `strings`

| Item | What it is | The one thing to know |
|---|---|---|
| `resolve(s)` | `$token` → active-locale text; `$$` → literal `$` | a missing token renders RAW and warns once — the visible failure gate |
| `load_str(json, locale)` | (re)load the active table | replaces the ONE process-wide table; bad JSON keeps the previous one |
| `flatten(json, locale)` | `{token:{locale:text}}` → `token→text` | per-token fallback to `en-us` |
| `generation()` | monotonic load counter | folded into every node fingerprint, so a language switch redraws exactly the text it changes |
| `raw_model_publish_literals(src)` | find display copy pushed from Rust into the Model | the Model-channel strings gate a scene self-runs on its own `lib.rs` |

### Sections, chat panel, undo/redo

| Item | What it is | The one thing to know |
|---|---|---|
| `Section` / `Sections` | declare a screen's `visible_bind`-gated subtrees as data | `publish(m)` writes the same Model keys the tree already reads — no second visibility path |
| `Sections::set_exclusive` | one-of-N (a settings rail) | radio via the optional `group` |
| `Sections::apply_section_contexts` | fold visibility edges into router `PushContext`/`PopContext` | LIFO discipline; the Router stays the one routing authority |
| `SectionChange` | one raw visibility diff entry | for tests/tooling; a scene uses this OR `apply_section_contexts`, not both |
| `chat_panel(x,y,w,h, view)` | the floating comms window builder | a bare `UiNode` builder the scene rebuilds each frame (so it can move/resize); run through a second `run_ui` pass |
| `ChatView` / `ChatLineView` / `ChatLineKind` / `RosterEntry` / `CORNER` | the chat builder's inputs | the `chat_tab` bind carries the active channel's INDEX, not its name |
| `Command` / `CommandHistory` / `DEFAULT_DEPTH` | the editor undo/redo spine | domain-free; a 40-item batch is ONE command (one Ctrl+Z); a failed revert stays on the undo stack |

## Interactions

- **Signals it captures.** The `WalkerHandler` layer subscribes to (and consumes) three
  groups, all by `ActionSignal` NAME — never a key or button (rule DFE3E44E):
  1. **The nav family the walker owns on every screen** (`walker_owned`): `NavUp` / `NavDown`
     / `NavLeft` / `NavRight` move focus (geometric with rects, ordinal ring without);
     `Confirm` activates the focused node's `action` or enters a pane; `Cancel` exits a pane
     one level or pops the context; `PanelNext` / `PanelPrev` move the pane cursor. `ChordBegin`
     is OBSERVED (scales a slider nudge) and passed through, never consumed. The bumpers
     (`TabNext` / `TabPrev`) are deliberately NOT owned — they belong to a `paged_menu`'s own
     tab rail.
  2. **The pointer signals** (`PrimaryAction` / `SecondaryAction`) — consumed only when
     `hud_hit` is set (the pointer is over UI), so the scene behind a panel never picks
     through it. A mouse click IS a `Confirm` targeted at whatever it hits (rule 37722F91).
  3. **Whatever the screen ROOT declared.** A screen root node carries `on_<signal>:
     "<result>"` props (`"on_menu": "pause_open"`), collected by `UiIntents::of` and bound
     with `.with_intents(...)`. **Signals ARE intents** (37722F91): an `on_<signal>` is the
     screen SUBSCRIBING to that signal (subscription model 67DEE93A) — it is a capture
     declaration, not a mapping into a second vocabulary. **There is NO intent router.** A
     declared binding may NOT name a walker-owned signal (`Confirm`/`Cancel`/`Nav*`/`Panel*`):
     those mean one thing on every screen, and naming one would statically kill it.
- **Results / intents it fires.** `take_fired()` returns the result names produced this
  frame — declared-intent fires, the `action` of a pad-`Confirm`-activated node, and pointer
  activations — as ONE list. The scene folds each name into its results (`results.set(name,
  true)`) exactly like a click and republishes it once as `sig_<name>` via
  `UiIntents::mirror_into`. A scene file's `exits` then maps a fired result name to a
  `Transition::Goto` (`SceneDef::exit`).
- **Model keys.**
  - *Read (bound):* `bind` (a control's value) and the `*_bind` family — `text_bind`,
    `label_bind`, `visible_bind`, `enabled_bind`, `color_bind`, `style_bind`, `live_bind`,
    `faded_bind`, `subtitle_bind`, `name_bind`, `meta_bind`, `rune_bind`, `cd_bind`,
    `charges_bind`. Each names a Model key; the walker reads it each frame.
  - *Written (published):* `run_ui` sets `hud_hit` into `results`; `UiIntents::mirror_into`
    writes transient `sig_<name>` keys; `Sections::publish` writes each section's
    `visible_bind` key. Everything else in the Model is the scene's to publish.
- **What it hands other crates.** `UiFrame` (draw commands + results + reserved surface
  slots + pointer sample + resolved rects); `SurfaceSlot` / `SurfacePointer` (the live-scene
  barrier — the walker reserves and samples, the scene's behaviour fills and reads);
  `HudCommand` lists for `render_hud`; `SceneManifest` (the roster) for the client's kernel.
- **Threads / workers.** None. The one piece of process-wide state is the stringtable's
  global table (`strings`), guarded by a `RwLock` — see Sharp edges.

## Gates

The drift gates a change must keep green (`cargo test -p flicker-widgets`):

- `unknown_kinds_catches_a_typo` — the kind vocabulary gate can fail, and `core` is rejected.
- `raw_display_literals_finds_copy_and_honours_exemptions` — the raw-copy gate and its exemptions.
- `rust_fallback_consts_mirror_theme_tokens_exactly` + `component_consts_mirror_their_named_theme_tokens`
  — every neutral fallback colour in `component.rs` is byte-for-byte its named `theme.tokens` entry.
- `resolve_tokens_expands_refs_and_leaves_unknowns` — `$token` expansion is order-independent.
- `load_styles_merges_satellites_and_refuses_a_palette_fork` — the satellite merge + one-palette guard.
- `ui_theme_json_is_the_theme_and_nothing_else` + `no_component_block_lives_in_a_shared_file`
  — the five-line split: `ui_theme.json` is the palette alone, no component blocks in shared files.
- `no_scene_reads_a_device_or_names_a_pane_style` — a scene-crate source sweep: no scene reads
  a device, names the retired pane palette, grows its own globe, or declares a walker-owned signal.
- `no_shipped_scene_names_the_template_tier` + `no_shipped_scene_authors_a_retired_surface_kind`
  — absence gates over shipped scene JSON (no `template`/`slots`; no `screen`/`rtt`/`viewport`).
- `every_shipped_stage_compiles_clean` + `every_surface_source_in_a_shipped_scene_resolves`
  (in `stages.rs`) — every shipped stage compiles with zero problems and every authored surface
  source resolves.
- Scene loader gates (`scene_def.rs`): `authored_names_that_do_not_resolve_are_load_errors`,
  `a_manifest_indexes_the_files_and_finds_the_one_boot`, and the goto-mode / restated-id gates.
- Intent + walker gates (`intents.rs`, `walker.rs`): `vocabulary_gate_skips_unknown_and_malformed_declarations`,
  `only_the_root_declares`, `a_focused_slider_steps_on_its_axis_and_chord_scales_the_step`,
  `the_left_stick_cycles_panels_and_wraps`, `a_hidden_subtree_contributes_no_focusables`.

## Sharp edges

- **A bound key the scene never publishes reads as nothing, silently.** The Model is
  nil-sparse: `text_bind: "hp"` with no `hp` published renders blank, no error. Publish every
  key your tree binds.
- **`take_fired()` is mandatory for a navigable screen.** A screen that runs a `.with_nav(...)`
  handler but never drains `take_fired()` lets the pad reach a focused button and never
  activate it — the drain is also the flash/step clock. One call per frame, always.
- **`on_<signal>` is honoured on the ROOT node only.** `UiIntents::of` scans the root's
  props; the same prop on a child is silently ignored. (Banked: MCP `78EF71AC`.)
- **A node's `signal:` prop is never validated.** A `tooltip` or a `nav_footer` option names
  an `ActionSignal` to draw its device-adaptive glyph; a typo'd or unpublished name reserves
  the space and draws an empty affordance, silently. (Banked: MCP `17CFFD85`.)
- **The walker-owned declaration gate scans the wrong corpus.** `no_scene_reads_a_device_or_names_a_pane_style`
  sweeps Rust source under `Alpha/crates/scenes`, but declarations now live in scene JSON
  under `content/sensorium/scenes` — so a scene file that declares a walker-owned signal
  (e.g. `on_cancel`) passes the gate green. (Banked: MCP `49633838`.)
- **Sliders commit on release.** While a drag holds the pointer, the live value feeds only
  the draw; `results` keeps reporting the resting value until the release frame.
- **`run_ui` clears keyboard focus at the top of any clicked frame.** To keep a `text_field`
  focused across clicks elsewhere, call `request_focus` before `run_ui` each frame.
- **The stringtable is one process-wide table.** `load_str` replaces it for the whole
  process; there is no per-window locale.
- **One `UiState` per walker pass.** A scene running a HUD pass and a floating chat pass holds
  two states, and so two independent draw caches.
