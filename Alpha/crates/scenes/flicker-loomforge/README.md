# flicker-loomforge

The **Loomforge Bench** — the animation-authoring editor. A four-page *bench* (a live,
in-game authoring tool: State Machine · Pack Browser · Creature Composer · TAE Editor) that
reads **and writes** a `flicker.pack` on disk: the character state machine, its clip
bindings, its transitions, and the per-frame combat *timeline* events (TAE = Timeline
Authoring Editor). It supersedes the retired read-only `flicker-packeditor`. This crate is
the *editor*; the pack format and its runtime (pose, skin, state machine) live in
[`flicker-skeletal`](../../animation/flicker-skeletal/README.md). It is a **scene** — a
self-contained screen the `prism-alpha` launcher can push — packaged as a library and
registered in that launcher's roster.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:**
  - [`flicker-skeletal`](../../animation/flicker-skeletal/README.md) — the pack types this
    crate authors (`PackFile`, `StateMachineDef`, `EventDef`, `Trigger`, `Response`) and the
    seam-aware `read_pack`/`write_pack` it round-trips.
  - `flicker` (umbrella) — re-exports the app/render/scene/script layers, and
    [`flicker-widgets`](../../frontend/flicker-widgets/README.md), the **walker** (the
    component engine that lays out and draws the authored chrome tree and answers the
    pointer).
  - [`flicker-scene`](../../frontend/flicker-scene/README.md) — the `Scene`/`Transition`
    contract this crate implements.
  - [`flicker-input-core`](../../input/flicker-input-core/README.md) +
    [`flicker-input-router`](../../input/flicker-input-router/README.md) — the *pump* seam:
    the launcher resolves this frame's **signals** and hands them in; this scene owns no
    key-bindings and no resolver.
  - `flicker-shell` — the pause overlay (`PauseScene`) and theme.
  - `flicker-core` — content roots and the gz-at-rest content seam.
- **Used by:** `prism-alpha` — its roster registers this bench via [`scene`](#the-external-api)
  (`SceneEntry::new("loomforge", "Loomforge Bench", …, flicker_loomforge::scene)`).
- **Reads from the content tree** (all under `Alpha/content/`):

  | Path | When | If missing |
  |---|---|---|
  | `sensorium/scenes/loomforge.scene.json` | handed in by the manifest as a `SceneDef` (the tests embed a copy) | no tree → a blank screen, logged `error!` |
  | `sensorium/scripts/loomforge.lua` | embedded at compile time (`include_str!`) | `error!`, pages all gate off — chrome only |
  | `packages/characters/GolemBase_Low/…` | opened on `enter` (the baseline pack + base rig) | `load_error`, shown loudly in the status line |
  | `packages/retarget/clips/locomotion/` | the shared clip library the pack resolves against | clips fail to resolve → validation warnings |
  | `packages/characters/**/*.pack.json` | scanned once on `enter` for the Pack Browser | an empty browser (real files only; nothing invented) |
  | `data/stringtable.json` (`lf_*` tokens) | every display string resolves through it | see *Sharp edges* (loud in tests, silent in-window) |

## The scene is a PAIR (the five-line UI architecture)

This is a *scene pair* (rule 491BD9BB): the look is data, the logic is a small script, and
Rust only draws and holds state. Nothing here re-teaches how to author that pair —
see [`Alpha/content/sensorium/README.md`](../../../content/sensorium/README.md).

- **`loomforge.scene.json`** — the *chrome* tree (top bar, tab bar, rails, lists,
  inspectors) and this bench's style blocks. Its root declares the signal intents (below).
- **`loomforge.lua`** — the *behaviour* `loomforge`'s pair script. Its `derive()` turns the
  raw **Model** (the per-frame `key → value` table this scene publishes) into *presentation*
  only: the four page gates (`page_sm`/`page_pack`/`page_creature`/`page_tae`) and every
  lit/idle style wash. It reads Model; it never edits the pack.
- **Rust** (this crate) — draws the two things the closed component set has no template for:
  the node-graph **canvas** and the TAE **timeline**. Both are free-2D, edge/curve, drag-drop
  surfaces, so they are scene-drawn inside walker-reserved rects. Rust also holds all editor
  state and owns the one dispatch that turns fired names into pack edits.

A **surface** is a rect the walker lays out but does not fill — the scene renders into it
(the reference pattern is nested surfaces; here `lf_canvas`, `lf_tae_strip`,
`lf_tae_page_strip`, and the doll thumbnails). A **doll** is a live skinned preview of a clip
rendered to its own offscreen target; a **poster** is a doll that has stopped re-rendering
and is showing its last frame (to spend no GPU submit).

## The external API

The crate's *external* surface is deliberately tiny — the launcher only calls `scene`; the
rest is the editor's own machinery, re-exported for tests and for a debugger.

| Item | What it is | The one thing to know |
|---|---|---|
| `scene(def: &SceneDef) -> Box<dyn Scene>` | The roster factory — the client behaviour `prism-alpha` registers. | The manifest resolves `loomforge.scene.json` and hands its `def` here; this is the whole entry point. |
| `struct LoomforgeBench` | The `Scene` itself. | Built only from a `SceneDef` (`new`); a def-less bench is a blank screen. |
| `LoomforgeBench::new(&SceneDef)` | Runtime constructor. | Clones the def's tree + styles into the bench. |
| `LoomforgeBench::shipped()` | A bench on the embedded scene file, for tests. | **Test builds only** — outside `#[cfg(test)]` it is `unreachable!`, so `Default` (which routes here) must never be used at runtime. |
| `LoomforgeBench::doc() -> Option<&EditorDoc>` | The loaded document, if any. | `None` until a pack loads (or if the open failed). |
| `LoomforgeBench::tab() -> Tab` | The visible page. | Opens on `Tab::StateMachine`. |

**Re-exported document types** (`pub use doc::{EditorDoc, Tab, Tool}`):

`EditorDoc` is the loaded pack plus the clip library it resolves against — **the authored
`PackFile` IS the document** (lossless; exactly what `save` writes back, including the
hand-authored `_note` header). The runtime `StateMachine` is only a *derived preview*,
rebuilt from the def whenever the graph changes and never read back (the `def → machine`
direction is lossy).

| Group | Methods |
|---|---|
| Lifecycle | `load(pack, &[clip_dirs])` · `from_parts(path, pack, model)` · `save()` (writes the gz-at-rest form at the logical path's `.gz` twin) |
| Read | `path` · `pack` · `def` · `states` · `model` · `dirty` · `warnings` · `preview` / `preview_mut` · `selected` · `clip_names` · `state_index` · `clip_index` · `clip_axis` |
| Graph edits | `select` · `bind_clip` · `add_state` · `remove_state` · `add_transition` · `remove_transition` · `nudge_priority` · `nudge_blend` · `cycle_trigger` |
| Event (TAE) edits | `nudge_event_tick` · `nudge_event_end` · `cycle_event_hit_type` · `nudge_event_capsule` · `toggle_event_response` · `nudge_event_parry_scale` |
| Rebuild | `rebuild_preview` |

Every edit returns a `bool` (or `Option`) and only dirties the document on a change the pack
accepted — a rejected edit (a stale reference, a clamped dial) leaves `dirty` untouched, so
the status line can report "could not …" instead of looking like it worked. The graph-edit
methods take `EdgeRef` / `EventRef`, which are **crate-internal** (defined in the private
`doc` module and not re-exported): the full edit API is driven by the bench, not by external
callers.

`Tab` (`StateMachine`/`PackBrowser`/`CreatureComposer`/`TaeEditor`) and `Tool`
(`Select`/`AddState`/`Link`/`Delete`) each expose `ALL`, `id()` (the node id **and** fired
action name — `tab_sm`, `tool_link`, …) and `label()`. **`label()` is retained and
test-pinned but is not the display source** — the tab/tool buttons show the `$lf_tab_*` /
`$lf_tool_*` tokens (see *Sharp edges*).

**Internal modules** (private `mod`s; their `pub` items are crate-internal, not part of the
API above): `canvas` (node-graph layout, hit-testing, zoom/pan `View`), `doc` (the document +
pure graph-edit functions over a `StateMachineDef`), `packs` (the library scan + derived
`PackKind` classification + filtering), `stage` (`StageRig` — the GPU doll rig and its
per-slot render targets), `tae` (the timeline lane model, frame↔pixel mapping, and the two
combat *budget* gauges).

## Interactions

### Signals it captures

Input reaches the bench as **signals** — never keys or buttons (rules 37722F91, DFE3E44E).
The launcher's pump resolves the frame's signals and the walker consumes them; there is **no
intent router** — the signal *is* the intent, and an `on_<signal>` on the scene root is a
capture declaration that fires a **result name** the one dispatch reads.

| Signal | Declared as | Fires result | Effect |
|---|---|---|---|
| `Menu` | `on_menu` | `pause_open` | pushes the shell `PauseScene` (with the profile's `World` map) |
| `TabNext` / `TabPrev` | `on_tab_next` / `on_tab_prev` | `tab_next` / `tab_prev` | cycles the four pages, with wrap |
| `ModeNext` / `ModePrev` | `on_mode_next` / `on_mode_prev` | `tool_next` / `tool_prev` | cycles the four canvas tools, with wrap |

The **pointer is also a signal** (37722F91): a click is a `Confirm` targeted at whatever it
hits. The walker answers it for every button/list in the chrome and reports `hud_hit` when it
consumed the pointer over UI. The scene reads the *raw* pointer sample only inside the
walker-reserved surfaces (`lf_canvas`, the two TAE strips), gated on `!hud_hit`, for the
bespoke canvas/timeline tools the walker has no components for — this is the sanctioned
scene-owned raw-pointer path, not a signal bypass.

### Results / actions it fires

Chrome buttons fire `action` names; the one dispatch (`apply_actions`) routes them. The
captured-signal results above land in the *same* map, so both channels are handled
identically.

- **Pages / tools:** `tab_sm` · `tab_pack` · `tab_creature` · `tab_tae` (from `Tab::id`);
  `tool_select` · `tool_add` · `tool_link` · `tool_delete` (from `Tool::id`); plus the
  `tab_next/prev` · `tool_next/prev` cycles.
- **Top bar:** `save` · `validate`.
- **State-machine rail:** `cycle_trigger` (advances the Link tool's trigger) ·
  `clip_prev` / `clip_next` (page the clip library); transition inspector: `edge_trigger` ·
  `prio_dec`/`prio_inc` · `blend_dec`/`blend_inc` · `edge_delete`.
- **Pack Browser:** `packcard_<i>` (select) · `packkind_<i>` (kind filter, 0–3) ·
  `packskel_<i>` (skeleton filter) · `pack_load`.
- **TAE Editor:** `tae_play` · `tae_prev` · `tae_next` (transport) · `tae_start_dec`/`_inc` ·
  `tae_end_dec`/`_inc` · `tae_cap_dec`/`_inc` · `tae_hit` · `tae_resp_<i>` (0–4) ·
  `tae_pscale_dec`/`_inc`.
- **Drag channel** (walker-published, read by the scene): `drag_kind` (`"clip"`) · `drag_id`
  (the clip name) · `drag_active` · `drag_dropped`. A clip row dragged onto a state card fires
  the drop, which the canvas resolves against its own geometry to `bind_clip`.
- **Scene transition it returns:** `Transition::Push(PauseScene)` on `pause_open`; otherwise
  `Transition::None`.

### Model keys

The bench publishes a **raw** Model each frame (`hud_model`), the pair script folds its
**derived** presentation over it (`derive`), and last frame's fired signal names ride in as a
transient `sig_<name>` mirror. Every authored bind in the tree resolves to one of these keys
(verified — see *Gates*). Grouped:

| Owner | Keys (representative) | Consumed by |
|---|---|---|
| Rust raw — readouts | `rig_badge` · `lf_status` · `save_label` · `pack_summary` · `clip_scroll_line` · `clipname_<i>` · every `edge_*` / `pack_*` / `tae_*` / `budget_*` line | tree `text_bind` / `label_bind` |
| Rust raw — cursors & masks | `sel_tab` (a **number**, 0–3) · `tool` (an id) · `has_edge` · `pack_visible` · `pack_sel` · `pack_open` · `packkind_<i>_on` · `skel_count` / `skel_<i>_on` · `tae_has_event` · `tae_resp_<i>_on` · `clip_prev_enabled` / `clip_next_enabled` | the pair script's `derive()`, and `enabled_bind`s |
| Rust raw — liveness | `live_<clipname>` (true only for the row under the cursor) | the clip-row doll's `live_bind` |
| Rust raw — colours | `packmeta_<i>_color` · `pack_kind_color` · `tae_ev_head_color` · `tae_resp_head_color` · `budget_*_color` | `color_bind` (dotted paths into `ui_theme.json`) |
| Lua derived — gates | `page_sm` · `page_pack` · `page_creature` · `page_tae` · `rail_clips` · `rail_edge` · `tae_no_event` | `visible_bind` |
| Lua derived — washes | `tab_<id>_sty` · `tool_<id>_sty` · `packkind_<i>_sty` · `skel_<i>_sty` · `packcard_<i>_sty` · `tae_resp_<i>_sty` · `pack_load_sty` | `style_bind` |

Wire names — triggers (`clip_done`, `attack`, …), response names (`block`, `parry`, …), hit
types (`slash`, …) — are published as **data**, not localised copy: what the inspector shows
is exactly what lands in the JSON.

### The refill mechanism (the one subtle move)

Three named container cells are **refilled by Rust at event time** (not per frame) — the
"Rust fills the container" pattern: `lf_clip_rows` (on pack load / scroll),
`lf_pack_cards` (on scan / filter change), `lf_skel_rows` (on scan). Each refilled row keeps a
**content-keyed id** — a clip row's doll is `clipdoll_<name>`, a card's is `packdoll_<i>` —
because that id *is* the doll's poster-cache key, so re-binding a clip yields a new key and a
fresh render, and a stale poster is impossible. Row labels ride per-row binds
(`clipname_<i>`), so no literal string ever enters the refilled tree.

### What it hands the renderer

- **Reserved surfaces** the scene draws into: `lf_canvas` (node graph, unstyled),
  `lf_tae_strip` (SM-page timeline preview, styled), `lf_tae_page_strip` (TAE-page timeline,
  styled). Their rects come back from the walker (`surface_rect`), so what is drawn and what
  the pointer picks share one layout.
- **Dolls** — one shared skinned rig uploaded once (`StageRig`), GPU-skinned per instance:
  the clip-row dolls, the pack-card thumbnails, the `taedoll` preview, and one per state card.
  Each declares an offscreen pass into the frame's `FrameGraph`, run before the overlay chrome
  so composites land under the panels. `Rate::Live` for the one selected/hovered doll,
  `Rate::Poster` for the rest.
- **Threads / workers / async:** none. All work is on the frame; the only off-thread cost is
  the GPU offscreen passes.

## Gates

The crate's tests are the drift gates. Domain suites (`canvas` · `doc` · `packs` · `stage` ·
`tae`) cover the pure logic; the `lib` suite covers the scene wiring:

- `the_shipped_scene_file_authors_the_bench` — the scene file parses, names behaviour
  `loomforge`, declares the pause intent, carries the three refill containers, and authors
  every tab/tool id and must-hit control (incl. the three surfaces).
- `the_pair_script_derives_the_page_gates_and_washes` — builds the real bench with the real
  `loomforge.lua` and asserts the raw cursors come back as the derived gates/washes.
- `hud_tree_walks_with_model` — the real tree walks the real model: no unknown kinds, no raw
  display literals, the chrome draws, and the must-hit controls resolve to **non-zero
  extent** (a control can be well-formed and still lay out to zero pixels).
- `the_declared_pause_intent_fires_through_the_authored_tree` — a `Menu` press through the
  real router is consumed by the walker and fires `pause_open`.
- `tab_actions_switch_the_page` / `tool_and_trigger_actions_route` — the action ids and the
  cycle intents route (with wrap; a tool switch abandons a half-drawn edge).
- `load_bind_save_round_trips_to_disk` — load → drag-bind a clip → save → reload, with the
  `_note` header and every state surviving (skips cleanly if the content tree is absent).
- `clip_row_carries_a_bound_doll_and_is_a_drag_source` — a refilled row is a drag source,
  its doll names a real stage source and binds liveness to the published key, its id is the
  cache key, and its label rides a bind.
- `pack_browser_refill_is_well_formed_over_the_real_library` /
  `the_clip_rail_refills_and_scrolls_over_the_real_library` — the refills produce unique ids,
  one card+doll per visible pack, one row per skeleton, and re-window on scroll.
- `no_raw_display_copy_published_into_the_model` — the Model-channel strings gate: display
  copy published from Rust must be a resolved `$token` or carry a `strings-gate-exempt`
  reason.
- `every_pack_kind_colour_resolves_in_the_scene_styles` · `every_tae_lane_resolves_all_four_colours`
  · `every_tae_lane_swatch_resolves` · `card_stage_colours_resolve_in_the_scene_styles` — pin
  the scene-drawn dotted colour paths, which otherwise fall back silently (see *Sharp edges*).
- `filtering_reclamps_the_selection` · `actions_are_safe_without_a_document` — the detail pane
  never describes a hidden pack; save/validate with no document report instead of panicking.

## Sharp edges

- **The TAE transport does not park the playhead.** `time` advances every frame
  unconditionally and the playhead + `Frame N / total` readout both read it, so `tae_play`
  (pause) only freezes the doll *image* (it becomes a poster) while the bar keeps sweeping,
  and `tae_prev`/`tae_next` (step) are immediately swamped by the per-frame advance. The
  `tae_playing` field's own doc-comment describes park-and-step behaviour the code does not
  implement. A true parked-frame transport needs its own paused clock. (Flagged in MCP.)
- **Loading a different pack over unsaved edits discards them silently** — no undo, no
  unsaved-work prompt (loading the *same* pack is a no-op that keeps edits). (Flagged in MCP.)
- **The Creature Composer page is a pending stub** — the tab is reachable but shows only a
  placeholder; the composer is not built yet.
- **Telegraph budget always assumes the entry tier** — `authoring_tier()` returns `None`
  because the creature/encounter model that carries `tier` lands with the combat system; the
  gauge says "no tier" rather than implying one was read.
- **Scene-drawn colours fall back silently.** The canvas and timeline read `ui_theme.json`
  by dotted path with a hardcoded fallback, so a renamed key shows a wrong-but-plausible
  colour, not a blank. The gates above pin the paths they cover, but **not every** scene-drawn
  path is pinned (e.g. `loomforge.canvas.card_fill_top`, `loomforge.tae.fill_bot`,
  `loomforge.rail_title.color`).
- **`Tab::label` / `Tool::label` are dead display copy** — retained and test-pinned, but the
  buttons render the `$lf_tab_*` / `$lf_tool_*` tokens; the English in `label()` is a second
  copy that no longer feeds the UI.
- **One-frame doll-liveness lag** — the hovered row is resolved from *this* frame's rects and
  animates *next* frame.
- **Offscreen doll targets are not freed on scene exit** — the bench implements no `exit()`
  and `StageRig` has no teardown; the per-slot targets are pruned only amortised, during
  render, down to the live set plus a slack of 8. Leaving the bench strands them (see the
  render-target-lifecycle rule). (Reported as an implementation gap.)
- Runtime construction is **manifest-only**: `Default`/`shipped()` panic outside tests, so
  the bench can never come up def-less and blank without a loud failure.
