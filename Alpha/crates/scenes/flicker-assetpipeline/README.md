# flicker-assetpipeline — the Clayworks Bench

The in-app **content-import bench**: open a folder of raw source files, walk a step
wizard that conforms them to the engine's canonical skeleton / mount sockets, bake the
one self-describing rig (or clip variants, or a static prop), and **Commit** the result
into `content/staging/`. It is a *scene crate* (a screen the app hosts) in the `scenes`
cluster, and it **leads the Developer realm** (the app's authoring section) — the largest
bench in the tree.

The crate **hosts; it does not process.** Every stage — scan, parse, rename-to-canonical,
conform, bake, commit — belongs to [`flicker-content`](../../content/flicker-content/README.md);
the viewport is `flicker-render`'s shared quad grid; the rig-overlay geometry is
`flicker-mechanics`; the on-screen controls are the `flicker-widgets` component walker.
This crate wires those together into a wizard and shows their reports. Adding processing
logic *here* would fork a pipeline that already exists.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

---

## Where it sits

- **Builds on:**
  - `flicker` (umbrella) — the `Scene` trait, `run_ui`/`render_hud` walker, the `Renderer`
    and shared `QuadGrid` viewport, the Lua `ScriptHost`, `ValueMap` (the Model).
  - [`flicker-content`](../../content/flicker-content/README.md) — **THE pipeline stages**
    this bench drives: `scan_folder`, `parse_fbx`, `rename_to_canonical`,
    `conform_to_canonical`, `bake_rig`, `write_rig` / `write_garment` / `write_prop`,
    the retarget `build_variants` / `write_variants`, and `roots()` (the staging/package
    path service). The bench owns no copy of any of these.
  - `flicker-skeletal` — the `flicker.rig` format it reads and writes, plus pose/skin for
    the viewport.
  - `flicker-mechanics` — rig-view overlay geometry (joint balls, bone diamonds, collision
    capsules) and ray-vs-segment joint picking.
  - `flicker-input-core` + `flicker-input-router` — the shared input **signal** bus and the
    resolver, consumed read-only. *Signal* = an abstract input event (Confirm, Menu, LookUp…),
    never a raw key; the catalog is `flicker-input-core`.
  - `flicker-shell` — the pause overlay (`PauseScene`) and `Theme` this bench runs inside.
  - `rfd` (native open-folder dialog), `image` (source-texture decode for the fit preview).
- **Used by:** `prism-alpha` — the roster registers this crate's `scene` factory:
  `SceneEntry::new("assetpipeline", "Clayworks Bench", "primary", flicker_assetpipeline::scene)`
  `.with_realm(REALM_DEVELOPER)` (`Alpha/prism-alpha/src/main.rs:65`). It is first in the
  Developer realm's menu order (pinned by the roster gate at `main.rs:225`).
- **Reads from the content tree** (each: path · when · what happens if missing):
  - [`assetpipeline.scene.json`](../../../content/sensorium/scenes/assetpipeline.scene.json)
    — the screen tree, this bench's style blocks, AND the three workflow definitions in
    `params.workflows`. Resolved by the manifest and handed to `scene()` at construction. A
    missing `tree` logs an error and draws nothing; a missing `params.workflows.import_character`
    **panics at build** (fail-loud — the wizard *is* this bench).
  - [`assetpipeline.lua`](../../../content/sensorium/scripts/assetpipeline.lua) — the *pair
    script* (compiled into the crate via `include_str!`). Loaded at construction; on a load
    error the bench logs and falls back to base styles.
  - `ui_theme.json` — the one palette; the scene's node `style` paths resolve against it.
  - the stringtable — every `$ap_*` / `$wf_*` token the tree and this behaviour name.
  - the fitting base body (`GolemBase`, skeleton-only load) and the bake-preview idle clip
    `retarget/clips/locomotion/In-Place/idle_neutral.json`, both from the **package** tree.
- **Writes to the content tree:** `content/staging/{characters,retarget/clips,props}/` via
  `flicker_content::roots().staging()`. Never the package the game reads — see
  [The Commit → staging step](#the-commit--staging-step).

For how to author the scene file and pair script this crate hosts, see the
[Sensorium authoring guide](../../../content/sensorium/README.md). The sibling scene crate
[`flicker-clicktrainer`](../flicker-clicktrainer/README.md) is a smaller example of the same
scene-pair shape.

---

## Public API

The external surface is **one function plus its constructor** — this crate is driven by the
authored scene pair, not by Rust callers.

| Item | What it is for | The one thing to know |
|---|---|---|
| `scene(def: &SceneDef) -> Box<dyn Scene>` | The factory the roster registers. The manifest resolves `assetpipeline.scene.json` and hands its `SceneDef` here. | The only entry point the app uses. |
| `AssetPipeline::new(def: &SceneDef) -> Self` | The same bench, unboxed. | Reads the tree, styles, and `params.workflows` off the def. |
| `AssetPipeline::shipped() -> Self` | Test seam: parse the *shipped* scene file and build the bench with no app. | Outside `cfg(test)` this is `unreachable!` — the manifest's `SceneDef` is the only construction path. |

Everything else is private. In particular, the wizard runtime lives in a private
`mod workflow` and is **not** callable from outside — but it defines the schema a human
authors in the scene file, documented next.

### The `params.workflows` schema (authoring contract)

`params.workflows` is a map of *workflow* name → definition. A **workflow** is one linear
wizard; branching lives *between* definitions (the Task cards choose which one runs). The
three shipped names are `import_character`, `import_prop`, `import_animation`.

| Field (per `step`) | Type | Meaning |
|---|---|---|
| `id` | string | Stable step id. Its subtree in the tree gates on `visible_bind: "wf_step_<id>"` (or an explicit `surface`). |
| `title` | `$token` | The rail-chip label, resolved through the stringtable at publish. |
| `needs` | `[string]` | Document keys that must be PRESENT to enter this step. `Next` into a step with an unmet need **refuses and warns** — never a blank page. |
| `yields` | `[string]` | Document keys this step produces — the declared forward contract. Leaving a step without its declared yields warns at the source. |

The *document* is the per-frame presence set the gates read; a key is present exactly when
the state it names exists: `source` (an open scanned folder), `class` (the declared asset
class), `rig`, `attach`, `fit`, `clip`, `committed`. A `needs` key that no earlier step
`yields` is warned at **build** time (it is either scene-provided or unsatisfiable — the
author finds out at load, not at the hundredth click of a `Next` that refuses).

---

## Interactions

### Signals it captures

All input arrives as *signals* through the ONE dispatch the pump hands `update()`; this
bench matches **signal names, never keys**. Capture is declared two ways: `on_<signal>`
intents on the authored scene root (consumed by the walker), and the camera signals claimed
by a handler below the walker.

| Declared intent (scene root) | Fires result | Reaches | Status |
|---|---|---|---|
| `on_menu` | `pause_open` | pushes `PauseScene` (Resume / Settings / Main Menu / Quit) | live |
| `on_tab_next` / `on_tab_prev` | `wf_next` / `wf_back` | the wizard forward / back (the pad bumpers walk the rail) | live |
| `on_mode_next` / `on_mode_prev` | `gizmo_next` / `gizmo_prev` | the gizmo-mode cycle (Translate → Rotate → Scale) | **not wired — see Sharp edges** |

- **Camera signals** — `LookUp/Down/Left/Right`, `ZoomIn/ZoomOut` — are captured by
  `EditorLayer`, the handler seated **below** the walker, and ONLY while the viewport pane
  `ap_view` is *entered* (`ui_state.entered_group()`), giving stick-look / stick-zoom on the
  2×2 perspective panel. Everywhere else they pass through untouched (the "world-below-walker"
  pattern). Rates: `STICK_LOOK_RATE` 2.4 rad/s, `STICK_ZOOM_RATE` 4/s.
- **Bespoke pointer tier** (the one ruled raw-input exception, not signals): per-panel
  orbit / pan / zoom and gizmo pick / drag are polled from raw pointer edges inside the
  reserved viewport rects (`ap_quad`, `ap_clip_pair`, `ap_bake_view`). `Ctrl`+ortho-click
  changes the focused joint. This tier is deliberate and documented as the exception.

### Results it fires

All of these are result names produced in the single dispatch (a button `action`, a bank
select, or a declared intent) and consumed the same frame:

- **Task cards:** `import_character`, `import_accessory`, `import_prop`, `import_animation`
  — each opens the native folder dialog and dispatches into the matching workflow.
- **Wizard:** `wf_next`, `wf_back`, `wf_discard_yes`, `wf_discard_no`.
- **Stage edits:** bank selects `pick_sel_N` / `bone_sel_N` / `sock_sel_N` / `att_sel_N`
  (N = 0..5), pagers `pick_prev|next` / `bone_prev|next` / `sock_prev|next`, plus
  `bake_skin`, `bone_reset`, `gizmo_next`, `gizmo_prev`.
- **Finish:** `commit`, `next_piece`.
- **Terminal transition:** `pause_open` → `Transition::Push(PauseScene)` (the only way out).

### Model keys

*Model* = the per-frame key→value table this behaviour publishes and the scene file binds.
The bench publishes a raw model each frame; the scene file binds it; the pair script derives
only presentation. Keys, by group (all published by this crate unless noted):

| Group | Keys (published) | Bound in the scene as |
|---|---|---|
| Header / inspector | `asset_name`, `asset_file`, `insp_title`, `insp_badge`, `insp_0`..`insp_7` | `text_bind` |
| Footer | `step_title`, `step_hint`, `next_label`, `next_enabled`, `back_enabled` | `text_bind` / `enabled_bind` |
| Step rail (per `<id>`) | `wf_<id>_title`, `wf_<id>_state`, `wf_<id>_show` | `label_bind` / (→ style) / `visible_bind` |
| Step surfaces | `wf_step_task`, `wf_step_prep`, `wf_step_preview`, `wf_step_attach`, `wf_step_review` | `visible_bind` |
| Conform roles | `on_conform_skeleton`, `on_conform_mount`, `on_conform_clip`, `on_pick` | `visible_bind` |
| Viewport gate | `view_quad`, `view_clip`, `view_bake` (exactly one true) | `visible_bind` on the three surfaces |
| Bone / socket / pick / attach banks | `bone_<i>`, `bone_<i>_color`, `sock_<i>`, `pick_<i>`, `att_<i>` + `*_page`, `*_prev_enabled`, `*_next_enabled`; cursors `bone_sel`/`bone_window`, `sock_sel`/`sock_window`, `pick_sel`/`pick_window`, `att_sel_idx` | `label_bind` / `color_bind` / (cursors → pair-script washes) |
| Skeleton page | `rig_headline`, `rig_legend`, `rig_progress` (gauge), `rig_sel` | `text_bind` / gauge `bind` |
| Fit / attach readouts | `fit_socket`, `att_sel`, `prep_height`, `prep_status`, `preview_status`, `prep_active` | `text_bind` / `visible_bind` |
| Review | `req_0`..`req_3`, `req_<i>_color`, `commit_label`, `commit_enabled`, `has_committed` | `text_bind` / `color_bind` / `enabled_bind` / `visible_bind` |
| Two-way controls (`bind`) | `show_skeleton`, `show_base`, `show_collision`, `show_wireframe`, `prefer_staged`, `as_provided`, `stature_cm`, `keep_pct`, `off_x/y/z/roll`, `gizmo_mode`, `mirror`, `variant_rm`, `variant_ip`, `fit_ox/oy/oz`, `fit_rx/ry/rz`, `fit_sx/sy/sz`, `fit_scale`, `att_x/y/z` | `bind` (published each frame AND written back on edit) |

The pair script [`assetpipeline.lua`](../../../content/sensorium/scripts/assetpipeline.lua)
derives **presentation only**: each rail chip's `wf_<id>_style` from the published
`wf_<id>_state` (`workflow.chip.active|visited|todo`), and each bank row's `*_sty` wash from
the published `*_sel` / `*_window` **numbers** (an index is a number). See the
[Sensorium guide](../../../content/sensorium/README.md) for how binds and style paths work.

### Viewport rects it reserves

The bench reserves three RTT (render-to-texture) rects by node id and the render pass fills
them with the shared `ViewportFiller` / `QuadGrid`: `ap_quad` (2×2: perspective orbit + top /
side / front ortho), `ap_clip_pair` (RootMotion | In-Place side-by-side), `ap_bake_view`
(single, the Preview page). Exactly one is visible per frame, page-gated by
`view_quad`/`view_clip`/`view_bake`. **No threads, workers, or async** — bake and retarget
run inline on Commit.

---

## The Commit → staging step

Commit is the bench's reason to exist. The `commit` result routes by class to a **staging**
tier, then bakes:

| Asset | Staging tier (`roots().staging()/…`) | Bake call (`flicker-content`) |
|---|---|---|
| Character / Skin (default) | `characters/` | `write_rig` (carries authored offsets + the attach `MountPoint` list) |
| Clothing (garment) | `characters/` | `write_garment` |
| Environment prop | `props/` | `write_prop` |
| Animation | `retarget/clips/` | `write_variants` (honours the RootMotion / In-Place picks) |

Output is **staged, not shipped.** *Staging* is the review tier; the *package* tree is what
the game reads. "I imported an asset" and "the asset ships" are two events: the **Quartermaster**
bench promotes staging → package, and only the package is on the engine's read path. See
[`flicker-content`'s README](../../content/flicker-content/README.md) for the bake / `roots`
API and the staging↔package split, and [`staging/`'s README](../../../content/staging/README.md)
for what the staging tier is.

Two knobs on the Task page change where a load starts, both bound two-way:
- `prefer_staged` — re-open this asset's already-staged rig instead of re-ingesting the FBX.
- `as_provided` — import the vendor rig faithfully as the editing view; the Commit gate still
  translates joint frames to the canon.

---

## Gates

44 tests (`cargo test -p flicker-assetpipeline`). The load-bearing ones, by name:

**Workflow runtime (`mod workflow`):**
- `linear_walk_is_gated_by_declared_needs` — `Next` refuses a step whose `needs` are unmet.
- `back_is_clean_when_not_dirty_and_guarded_when_dirty` — `Back` arms the discard dialog only on a dirty step.
- `publish_reports_rail_footer_and_exclusive_surfaces` — the published rail/footer/surface keys.
- `definitions_load_from_scene_params_and_build` — `params.workflows` parses into runnable definitions.

**Bench + services:**
- `every_declared_intent_reaches_the_dispatcher` — the `on_*` intents' result names drive the dispatcher (see the Sharp-edges note on what this does *not* cover).
- `rail_chips_ride_the_workflow_runtime_binds` / `a_non_character_workflow_has_no_attach_step` / `restart_on_review_returns_to_task_with_the_default_definition` — rail composition per definition.
- `the_gate_refuses_without_a_source` / `stage_gates_require_their_input` — fail-loud stage entry.
- `commit_roots_route_by_class_and_all_land_in_staging` — the routing table above, and that every tier lands under `staging/`, never the package.
- `commit_writes_a_loadable_rig_carrying_the_authored_offsets` / `commit_routes_a_prop_to_the_static_bake` / `committing_an_as_provided_rig_translates_frames_to_canon` — the Commit routing + bake contract.
- `an_animation_walks_retarget_preview_and_commits_the_picked_variants` — the clip path end to end.
- `adopt_staged_reopens_the_committed_rig` / `adopting_the_promoted_golem_retains_the_fitted_joints` — the `prefer_staged` reload.
- `the_preview_page_plays_the_commit_bake_under_the_shared_idle` — Preview plays exactly the commit bake.
- `authored_offsets_move_the_skeleton_and_reset_restores_it` / `the_bone_map_pages_over_the_whole_skeleton` / `attach_points_track_their_parent_bone_and_offset` / `fit_stage_authors_the_prop_mount` — the conform / attach / mount editors.
- `right_drag_pan_tracks_the_cursor_without_rotating_the_view` / `wheel_zoom_is_proportional_and_moves_all_four_views` / `the_fitting_body_loads_its_mesh_for_the_reference_view` / `collision_overlay_has_capsules_and_joint_balls` — the viewport tier.
- `bone_map_colours_resolve_against_the_scene_styles` — the map colour key resolves against `ui_theme.json`.

---

## Sharp edges

- **The gizmo-cycle intents fire from nothing in-app.** `on_mode_next` / `on_mode_prev`
  declare the results `gizmo_next` / `gizmo_prev`, but the signals behind them (`ModeNext` /
  `ModePrev`) are `Reserved` in `flicker-input-core` and **no shipped profile binds them**
  (MCP incident A50A2ABA). So the L2/R2 gizmo cycle never fires; the gizmo mode is reachable
  only via the on-screen radio (`gizmo_mode` bind). The vocabulary gate does not warn because
  the signal *name* resolves. This is unfinished wiring, not dead surface — the fix is to bind
  (or re-scope) the two signals.
- **`every_declared_intent_reaches_the_dispatcher` does not cover the binding channel.** It
  fires `gizmo_next`/`gizmo_prev` by *result name* and asserts the dispatcher cycles the mode.
  It is green while the *signal→result* path above is dead — the gate certifies the dispatcher,
  not that any input can reach it.
- **The viewport node ids are load-bearing magic strings.** `ap_quad` / `ap_clip_pair` /
  `ap_bake_view` must match between the scene file and the Rust `surface_slot("ap_quad")`
  reads. Rename one side and the view silently fails to seat (the node gets no rect; nothing
  draws) — there is no error. (Older MCP notes call the 2×2 node `editor_quad`; the shipped
  code and scene file use `ap_quad` — the code is truth.)
- **No default construction.** `AssetPipeline::shipped()` is `unreachable!` outside tests, and
  a scene file missing the `import_character` definition **panics at build**. Both are
  fail-loud by design: a def-less bench is a blank screen, so the crate refuses to be one.
- **Slider ranges are authored data.** Fit offsets ±50 cm, rotations ±180°, scales 0.01–4,
  bone offsets ±20 / roll ±90, attach offsets ±30, stature 40–200 cm, keep 50–100 % — all live
  in the scene file, not in Rust. Re-tune them there.
- **Sparse-nil Model.** Every `bind` reads through `is_on` / typed getters that default when a
  key is absent, so a display toggle or gate over an unpublished key reads as `false`/empty
  rather than erroring — safe, but a mistyped bind name shows nothing rather than warning.
