# flicker-componentcatalog

The **UI test scene**: a Developer-realm *bench* (a launchable workbench) that shows one
live demo copy of every engine widget kind, each in its own card, flowing top-to-bottom in a
scrollable tray. A left nav rail of bookmarks scrolls the tray to a card and highlights the
card at the top of the view. This is what an author launches to *see* what a kind can do —
every kind present, every feature on, resolved live from the real scene file and theme, never
baked into the bench. It is a *scene crate* — a library that supplies one `Scene` behaviour
paired with an authored scene file and a Lua pair script, launched by name from the
`prism-alpha` roster — of the same shape as
[`../flicker-clicktrainer/README.md`](../flicker-clicktrainer/README.md), which is the
smallest complete example if you are meeting the pattern for the first time.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

**Vocabulary used below** (each is a flicker word, not a general one): a **bench** is a
launchable workbench scene; a **scene file** is the authored `*.scene.json` that declares the
component **tree**; its **pair script** is the same-named `.lua` that turns raw numbers into
display values; the **Model** is the per-frame key→value table the engine hands to Lua and to
the tree's binds; the **walker** is the Rust pass (`run_ui`) that lays out, hit-tests and
draws the tree in one call; a **signal** is a device-independent input verb (`Menu`,
`PageNext`, `Confirm`) — never a key or a button; an **intent** is a signal bound to a result
name as data on the scene file's root (`"on_menu": "pause_open"`); a **result** is a name
fired into the frame's results map; a **bind** is a Model key a control reads and (two-way)
writes back; a **token** is a `$name` display-string reference; a **card** is one demo box in
the tray; the **PTT** (paged menu) is the two-rail page→tab composite; a **surface** is a
drawing ground the scene's own 2D/3D element occupies under the UI.

## Where it sits

- **Builds on:** [`flicker`](../../core/flicker/README.md) (the umbrella:
  `Scene`/`Transition`, `Renderer`/`FrameGraph`, `ScriptHost`/`ValueMap`, the walker entry
  points `run_ui`/`render_hud`, the live-surface `ViewportFiller`/`ViewportLayout` and
  `grid_segments_xy`, and `input_device::last_input_context`) ·
  [`flicker-input-core`](../../input/flicker-input-core/README.md) (the `ActionSignal`
  vocabulary) · [`flicker-input-router`](../../input/flicker-input-router/README.md)
  (`InputHandler`/`Router`) · [`flicker-shell`](../../frontend/flicker-shell/README.md)
  (`PauseScene`, `Theme`, `input_profile`) · `serde_json` · `tracing`. The component draw/
  hit/bind logic it demos lives in [`flicker-widgets`](../../frontend/flicker-widgets/README.md);
  the tree/frame types (`UiNode`, `UiFrame`, `UiIntents`) are documented in
  [`flicker-scene`](../../frontend/flicker-scene/README.md).
- **Used by:** [`prism-alpha`](../../../prism-alpha/src/main.rs) only, and only through
  [`scene`](#public-api). Its roster entry is in `roster()` — id `"componentcatalog"`, title
  `"Component Catalog"`, realm `REALM_DEVELOPER` (third Developer bench, after `assetpipeline`
  and `quartermaster`).
- **Reads from the content tree:**

| Path | When | If missing |
|---|---|---|
| [`content/sensorium/scenes/componentcatalog.scene.json`](../../../content/sensorium/scenes/componentcatalog.scene.json) | at launch, by the kernel — the parsed `SceneDef` is handed to `scene()` | no `tree` ⇒ `tracing::error!` and the bench draws nothing (`update` early-returns) |
| [`content/sensorium/scripts/componentcatalog.lua`](../../../content/sensorium/scripts/componentcatalog.lua) | compiled in via `include_str!` (`src/lib.rs:37`), loaded in `ComponentCatalog::new` | load error ⇒ `tracing::error!` and every demo shows raw/empty, no nav highlight, the paged menu does not gate (see Sharp edges) |
| [`content/sensorium/resources/ui_theme.json`](../../../content/sensorium/resources/ui_theme.json) | per frame via `load_shared_styles` — the one palette (`$token` colours) | a `$token` colour falls back to a compiled default, silently |
| [`content/data/stringtable.json`](../../../content/data/stringtable.json) | per draw — the `$cat_*` display tokens the tree names | the raw token text draws |

To change what a card shows or says, edit the scene file — see
[`../../../content/sensorium/README.md`](../../../content/sensorium/README.md) for the
authoring format. This file does not re-teach it.

## Public API

Three items, all reachable from `lib.rs`. `params` in the scene file is `{}` and is never
read; `behaviour: "componentcatalog"` is the roster key that binds the file to `scene`.

| Item | For | The one thing to know |
|---|---|---|
| `pub fn scene(def: &SceneDef) -> Box<dyn Scene>` | the roster factory — the only intended entry point | The `SceneDef` is the *parsed scene file*; the kernel resolves it from the manifest by id and hands it here. |
| `pub struct ComponentCatalog` | the `Scene` implementation | All frame state lives here. The card list, the two-way bind set, and the section tracker are **derived from the authored tree at construction**, so adding a card is one authored box + its bookmark and zero Rust bookkeeping. |
| `pub fn ComponentCatalog::new(def: &SceneDef) -> Self` | the unboxed constructor | Loads the pair script and clones the authored tree; builds the theme and viewport lazily (`enter`/`render` need the `Renderer`). |

`ComponentCatalog` and `ComponentCatalog::new` have no caller outside this crate today —
`prism-alpha` registers `scene` only, and there is no binary target. They are the seam a
second host would use, kept deliberately — not dead code. The `Scene` trait methods
it implements are `enter`, `update`, `render`; it leaves the rest at their defaults. There is
no compiled tuning: the only `const`s are the viewport demo's `DEMO_RADIUS`/`DEMO_GROUND`/
`DEMO_CUBE` (the wireframe cube drawn into the `surface` card) and `CONTENT_SCROLL_BIND`.

## The one thing to internalise — scroll-to is read live from layout

The walker reports every id'd node's resolved rect (`UiFrame::rects`). The list lays out
**all** cards into the full content height (the viewport is only a draw-time clip), so an
off-screen card is still placed and reported. The offset that brings card `i` to the top is
therefore `card_i.y - card_0.y` — the height stacked above it, read live per frame — so there
are no hardcoded card heights and no scene↔Rust height fork. A bookmark fire sets that
offset; the wheel writes it (the list echoes it back clamped) and the bench adopts the clamped
truth; the card nearest the current offset is the highlighted bookmark. And the required card
set is **derived from `flicker::ui::rust_component_kinds()`** by a gate, so the catalog can
never silently lag the engine — a newly promoted kind fails the build until its card exists.

## Interactions

### Signals it answers

Signals only — never keys or buttons; what *produces* a signal is profile data, out of scope
here. `update` builds one walker layer with `WalkerHandler::hud(…).with_nav(tree, model)
.with_intents(ui_intents)` and dispatches the pump's events through it (`Router::dispatch`);
the crate owns no resolver. Because the tree carries `tab_group`s, the nav suite is
**live** here (unlike Click Trainer).

| Signal | Channel | Effect |
|---|---|---|
| `Menu` | **declared intent** — `"on_menu": "pause_open"` on the root | fires `pause_open` ⇒ `src/lib.rs:352` returns `Transition::Push(PauseScene)` |
| `PageNext` / `PagePrev` | **declared intents** — `"on_page_next"`/`"on_page_prev"` on the root | fire `cat_pm_page_next`/`cat_pm_page_prev`, which step the Paged Menu card's page rail (see Results) |
| `NavUp`·`NavDown`·`NavLeft`·`NavRight` | `with_nav` | move the single focus among the members of the focused pane (`cat_nav`, `cat_content`, the footers) |
| `Confirm` | `with_nav` | activate the focused control, or enter the focused container pane |
| `Cancel` | `with_nav` | back out of an entered pane |
| `PanelNext` / `PanelPrev` | `with_nav` (the left stick) | cycle between pane stops by geometry |
| `PrimaryAction` (pointer) | walker hit pass | click a bookmark / control; the wheel scrolls the pane under the cursor |

The intent layer is loud on typos: an `on_<signal>` whose suffix names no `ActionSignal` is
`tracing::warn!`-ed and skipped (`flicker-widgets/src/intents.rs:64`), never silently dropped.
The Paged Menu card's **tab** rails additionally step on a controller's shoulders — that
mapping is owned by the `paged_menu` kind in `flicker-widgets`, not declared here.

### Results and exits it fires

| Result | Produced by | Consumed by |
|---|---|---|
| `nav_0` … `nav_26` | each bookmark button's `"action"` | `src/lib.rs:337-346` — scrolls the tray so that card sits at the top |
| `cat_content_scroll` | the tray `list` echoing its wheel-clamped offset | `src/lib.rs:329` — adopted as the bench's scroll truth; a bookmark jump overrides the resting echo |
| `cat_pm_page_next` / `cat_pm_page_prev` | the `PageNext`/`PagePrev` intents **and** the two Nav Footer `BACK`/`NEXT` buttons | the `tabs`/`pill_toggle` kind self-steps its own `bind` (`cat_pm_page`) by ±1, clamped, on the **next** `run_ui` pass (`flicker-widgets/src/component.rs:1256`). One name, every trigger. |
| `cat_pm_tab_next` / `cat_pm_tab_prev` | the Paged Menu's shoulder mapping (controller) or a direct pill click | same self-step channel, on `cat_pm_tab` |
| `pause_open` | the `Menu` intent **and** both footers' `MENU` buttons | `src/lib.rs:352` → `Transition::Push` |
| `cat_button_click` · `cat_menu_cut/copy/paste` · `cat_slot_cast` · `cat_popup_ok/cancel` | the demo controls on cards 0, 15, 19, 22 | **nothing — by design.** A catalog demo fires its action to prove the control is live; the result is intentionally inert. These are not silent-failure seams; the names resolve and fire. |

**Exits: none.** `"exits": {}` is empty and the crate does not implement `Scene::route`, so an
authored exit would be inert. The only stack move is the pause `Push`.

### The card catalog — 27 cards

The tree ships 27 cards (`card_0`…`card_26`), one bookmark (`nav_<i>`) each. They cover all
**25** `RUST_COMPONENT_KINDS` (the standalone `tabs` kind shares card 23) plus three
feature/structural demos: the `runes` decoration flag (card 3), the sprite *presenting* mode
a.k.a. "splash" (card 24), and the structural `surface` kind (card 26). Every display string
is a `$token`; every interactive value rides a `bind` the bench seeds and folds.

| Card | Bookmark token | Kind / feature demoed | Exercised via |
|---|---|---|---|
| 0 | `$cat_nav_button` | `button` (primary, `lg`) | action `cat_button_click` (inert) |
| 1 | `$cat_nav_panel` | `panel` (nested styled box) | static |
| 2 | `$cat_nav_sprite` | `sprite` (raw quad, `tex 1`) | static |
| 3 | `$cat_nav_rune_corners` | the `runes: true` decoration flag on a `cell` (the retired Rune Corners kind) | static |
| 4 | `$cat_nav_tooltip` | `tooltip` (rune + name/meta) | static |
| 5 | `$cat_nav_checkbox` | `checkbox` | bind `cat_check_val` |
| 6 | `$cat_nav_toggle` | `toggle` | bind `cat_toggle_val` |
| 7 | `$cat_nav_radio` | `radio` (two, shared value) | bind `cat_radio_val` |
| 8 | `$cat_nav_tile` | `tile` | bind `cat_tile_on` + `enabled_bind cat_tile_loaded` |
| 9 | `$cat_nav_pill_toggle` | `pill_toggle` (3 options) | bind `cat_pill_val` |
| 10 | `$cat_nav_select` | `select` (3 options + placeholder) | bind `cat_select_val` |
| 11 | `$cat_nav_slider` | `slider` (0–100, `%`) | bind `cat_slider_val` + `focus_group cat_content` |
| 12 | `$cat_nav_stepper` | `stepper` (0–10) | bind `cat_stepper_val` |
| 13 | `$cat_nav_text_field` | `text_field` | bind `cat_field_val` |
| 14 | `$cat_nav_list` | `list` (4 rows, own scrollbar) | bind `cat_list_demo_off` |
| 15 | `$cat_nav_context_menu` | `context_menu` (cut/copy/divider/paste-disabled) | actions `cat_menu_*` (inert) |
| 16 | `$cat_nav_gauge` | `gauge` (lo/hi bands) | bind `cat_gauge_val` |
| 17 | `$cat_nav_resource_gauge` | `resource_gauge` (tone `health`) | bind `cat_rgauge_val` |
| 18 | `$cat_nav_stat_dot` | `stat_dot` (green, glow) | static |
| 19 | `$cat_nav_action_slot` | `action_slot` (rune, key, charges, cd) | action `cat_slot_cast` (inert) |
| 20 | `$cat_nav_medallion` | `medallion` (sapphire ring) | static |
| 21 | `$cat_nav_badge` | `badge` (bronze) | static |
| 22 | `$cat_nav_popup_panel` | `popup_panel` (title/subtitle/footer + 2 buttons) | actions `cat_popup_ok/cancel` (inert) |
| 23 | `$cat_nav_paged_menu` | `paged_menu` (PTT) — page `tabs` + two page-gated `pill_toggle` tab rails + 4 content cells; **also covers the `tabs` kind** | binds `cat_pm_page`/`cat_pm_tab`; `tabs_shown cat_pm_tabs_shown`; visibility derived in Lua |
| 24 | `$cat_nav_splash` | the `sprite` *presenting* mode (`fade_in`/`hold`/`fade_out`/`fit`/`backdrop`) | **reads `Model.elapsed`** (seeded `0.9`) — see Finding 1 |
| 25 | `$cat_nav_nav_footer` | `nav_footer` (glyph legend + `MENU`/`BACK`/`NEXT` cluster) | buttons fire `pause_open` / `cat_pm_page_prev` / `cat_pm_page_next` |
| 26 | `$cat_nav_surface` | the structural `surface` kind — a nested live surface (`layout: quad`) | filled by `ViewportFiller` (wireframe cube on a grid), orbit on left-drag |

A second `nav_footer` (`cat_footer`) sits as a persistent band below the tray — it carries the
always-present `MENU` button, independent of scrolling; card 25 is the *demo* of the same kind.

### Model keys

Two hops. Rust publishes **raw** values into the script; the pair script seeds initial demo
values and derives the display/visibility keys; the merged map is what the tree's binds read.

| Key(s) | Role · type | Publisher | Partner (reader / bind) |
|---|---|---|---|
| `cat_content_scroll` | tray scroll offset · Number px | `src/lib.rs:205` (raw) **and** the tray `list` (two-way) | the tray `list` `bind` |
| `section` | index of the top card · Number | `src/lib.rs:206` | `componentcatalog.lua:52` (lights the active bookmark) |
| `card_count` | card total · Number | `src/lib.rs:207` | `componentcatalog.lua:53` (loops `nav_sty_i`) |
| `input_device` | live display device token · Text | `src/lib.rs:210` | the `paged_menu` kind — drops pad-glyph hints on kbm, shows them on a pad |
| `nav_sty_0` … `nav_sty_26` | bookmark style path · Text | `componentcatalog.lua:54` (derive) | each bookmark's `style_bind` (primary = active, secondary = idle) |
| `cat_pm_on_p0`·`cat_pm_on_p1` | page gates · Bool | `componentcatalog.lua:61` | the two tab rails' `visible_bind` |
| `cat_pm_p0_t0`·`cat_pm_p0_t1`·`cat_pm_p1_t0`·`cat_pm_p1_t1` | page×tab content gates · Bool | `componentcatalog.lua:63-66` | the 4 content cells' `visible_bind` |
| the demo binds: `cat_check_val` `cat_toggle_val` `cat_radio_val` `cat_tile_on` `cat_tile_loaded` `cat_pill_val` `cat_select_val` `cat_slider_val` `cat_stepper_val` `cat_field_val` `cat_gauge_val` `cat_rgauge_val` `cat_pm_page` `cat_pm_tab` `cat_pm_tabs_shown` `elapsed` | seeded initial value | `componentcatalog.lua:18-36` (only while the engine has echoed no committed value) | each card's control; a committed value folds back via `apply_results` and wins thereafter |
| `cat_nav_scroll`·`cat_list_demo_off` | scroll offsets · Number px | neither seeded nor published — self-initialise to 0 | the nav rail / card-14 lists (two-way) |

Every two-way `bind` the tree carries is discovered from the tree at load (`tree_binds`) and
folded back generically, so a new bound demo control round-trips without
touching Rust. This scene does **not** publish the `sig_<name>` intent mirror (it has no
script that observes it).

### What it hands the shell

- A `FrameGraph` per frame: the `surface` card's `ViewportFiller` declares its offscreen
  passes and composites the wireframe cube at `base+2` (inside the card's well), and the
  walker's `HudCommand` list is blitted by `render_hud` as one overlay at `base+1`.
- `Transition::Push(PauseScene)` on `pause_open`, built from a `Theme` created in `enter` and
  the `"World"` context map from `flicker_shell::input_profile()` (fallback
  `InputMap::wasd_and_mouse`).
- The clear colour, set once in `enter`.
- No threads, no workers, no async.

## Gates

`cargo test -p flicker-componentcatalog` — 8 tests, all green.

| Test | What it holds |
|---|---|
| `the_pair_script_seeds_and_derives` | The pair script loads; `derive()` seeds the demo values (`cat_check_val` ON), lights bookmark 0 active and rests the rest, and gates the Paged Menu on page 1. |
| `one_bookmark_and_card_box_per_component` | Every card has its `nav_<i>` bookmark, and **every** `rust_component_kinds()` kind appears somewhere in the tray — a new engine kind fails here, by name, until its card is authored. |
| `the_surface_kind_keeps_its_live_catalog_card` | The structural `surface` kind (invisible to the roster-coverage gate) keeps its live card 26 with a `layout`, by name. |
| `every_authored_style_path_resolves_to_a_block` | Every style path any shipped scene tree names resolves to a real style block — a dead path would draw compiled defaults silently. |
| `every_ladder_button_in_a_row_sits_in_an_aligned_flow` | A `size_class` button in a `stretch` row would be a silent no-op; this forbids it. |
| `every_pane_group_has_a_clear_container` | Every `tab_group` has exactly one actionless container node with a non-zero authored ordinal, so pad navigation never strands members on a bare leaf. |
| `the_nav_rail_draws_rust_owned_modal_chrome_not_hardcoded_bytes` | End to end: the theme is colours only, the scene file owns its layout blocks, the loader resolves `modal.buttons.variants.primary` to the palette's `$sap_base`, and the walked tree draws exactly that fill on the active bookmark. |
| `the_scene_ships_clean_kinds_and_no_raw_literals` | Every kind is known; every display literal is a `$token`, in the tree and in what Rust publishes. |

## Sharp edges

- **The splash card's fade needs a ticking clock that nothing in the card names.** The
  `sprite` presenting mode reads `Model.elapsed` with no `*_bind` prop
  (`flicker-widgets/src/component.rs:3047`); the pair script seeds `elapsed = 0.9` to freeze
  card 24 at full alpha. An author copying the card cannot discover the dependency from the
  card. See Finding 1.
- **Losing the pair script is a soft, wide failure.** If `componentcatalog.lua` fails to load,
  the bench still draws, but every demo shows its raw/empty default, no bookmark highlights,
  and the Paged Menu does not gate — because the seeds, `nav_sty_*`, and `cat_pm_*` gates all
  come from `derive()`. The `the_pair_script_seeds_and_derives` gate keeps that off the screen.
- **A typo'd bind / style_bind / visible_bind key resolves to nothing, silently.** A missing
  Model key reads nil, and the control falls back to its default (a bookmark with no
  `nav_sty_i` uses its plain style; a cell with no visibility gate stays shown). Only the
  intent layer (`on_<signal>`) and the build gates warn; the per-frame bind lookup does not.
- **Strip stepping lands one frame late.** A `PageNext` / footer `NEXT` records the step name
  this frame; the `tabs`/`pill_toggle` advances its bind on the **next** `run_ui` pass
  (`flicker-widgets/src/component.rs:1256`). The page visibly turns on the following frame.
- **`enter` must run before `update`/`render`.** The pause `Push` is guarded by `if let
  Some(theme)` — before `enter` builds the theme, `Menu` would silently do nothing. The kernel
  always calls `enter` first.
- **Card 11's slider carries both `tab_group` and `focus_group`.** `tab_group` is nav
  membership; `focus_group` echoes the slider's `bind` into a shared group key so a focused
  slider row shows focus (`flicker-widgets/src/component.rs:2446`). Both are real and read —
  the pair is not a duplicate.
