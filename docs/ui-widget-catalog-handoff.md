# flicker UI — Widget Catalog & Consolidation (formalization, 2026-07-08)

The formal, consolidated UI control set for flicker clients — scoped to **ship the world-gen
timeline viewer**, not to build a full MUI. It unifies the four divergent `ui_elements.json`
copies and names the widget vocabulary. This is the **spec**; the build follows (see §6).

## 0. The architecture it formalizes (unchanged, load-bearing)

- **Draw protocol = three primitives.** The engine↔Lua HUD boundary carries only
  `HudCommand::{Rect, Sprite, Text}` (+ layer/align). Every widget is *composed* from those in
  Lua. State crosses the boundary as **plain `Bool | Number | Text` only** (no handles, no nested
  tables) via a two-way `Model` — the load-bearing invariant (CLAUDE.md §2).
- **Three homes:** widget **toolkit** = `crates/flicker-ui` (`widgets.lua` + `render_hud` +
  `load_ui_json`) — the real shared seam both `flicker-shell` and every client consume. UI
  **content** (chrome `ui_elements.json` + shared scripts + theme tokens) = `Alpha/content`.
  Chrome **host** = `flicker-shell` (Alpha).
- **`id` is the one contract:** a widget's `id` is the single key that flows
  content → `WorldConfig` lever → `Model` → widget → results (`e3_mountain_uplift`, `ab_O`,
  `e4_water_delivery`). List/grid rows key by an id-prefix convention (`ab_<sym>`, `iso_<compound>`)
  since the boundary carries no nested tables.

## 1. The widget catalog (17 types)

**Exist today** (in `widgets.lua`, keep/promote): `label`, `panel` (flat *or* 9-slice), `button`,
`slider` (± tick ruler), `toggle`/`checkbox`, `dropdown`, `stepper`, `section_header`.

**Partial** (extend): `timeline` (bespoke, 6 epochs, flat → promote to `W.timeline_*`, 9 epochs +
3 group bands + markers + transport); `tabs`/`nav` (dialect-split Family A tabs vs Family B nav →
one `W.tabs`, horizontal + vertical).

**Missing** (build only what the viewer needs): `layout container` (row/column/stack — a
rect-yielding helper; the single highest-leverage gap, positioning is all absolute pixels today);
`grid` (N-column cell layout — the element-mix is hand-rolled modulo math); `list`/`scroll-list`
(the 28-element / 89-compound / 19-action selectors — **blocked** on scroll input + scissor, see
§5); `radio`/`segmented` (generalize sol2's tri-state phase-nav — needs a disabled state `button`
lacks); `legend`/`colorbar` (maps the active field's gradient to values — none exists for the ~14
field views).

**Missing + BLOCKED, out of scope:** `text`/`numeric input` — `update()` is mouse-only; no
keyboard/text crosses the boundary. Note it; do not smuggle it in.

## 2. The timeline-viewer control set

1. **Grouped 9-epoch timeline scrubber** — duration-weighted segment per epoch, 3 group bands
   (I Molten 1-3 · II Water/Life/Crust 4-6 · III Strata 7-9), active highlight, draggable 0..1 playhead.
2. **Transport** — play/pause + speed, decoupled from the SKY/celestial slider it piggybacks on today.
3. **Per-epoch param panel** for the selected epoch — label/slider/value rows **generated from
   `engine.params()` / `epoch_defaults.json`** (`LeverDef {id,default,min,max}`, never hardcoded),
   covering all 9 epochs incl. `e4_water_delivery` and the `e7/e8/e9` duration levers.
4. **Epoch selector** — stepper, or tabs grouped by the 3 bands.
5. **Epoch-1 element-mix slider grid** — the `ab_<sym>` compact sliders, driven from `abundance.json`.
6. **View-mode selector across three axes** — the 14 field ViewModes + a per-element axis + a
   per-compound axis (the engine now forms a `CompoundLedger`). Dropdown or radio/segmented.
7. **Element & compound toggle-lists** — scroll-list to pick which element (27) / compound (78) to
   colour by / isolate (needs the missing scroll-list).
8. **Legend/colorbar** for the active field.
9. **Grid-frequency stepper** (`hud.controls.freq`), unchanged.
10. **Two seed buttons** — per-epoch **reseed** (`engine.reseed(e)`, invalidate-forward) *and* a
    separate **new-world** base-seed (`engine.set_seed`).
11. **Stats readout** — `EpochSnapshot.provenance` (per-epoch seed, steps, `conserved_mass`) +
    compound & harvestable-node counts.

## 3. Unified `ui_elements.json` schema (flat, data-only — stays)

```jsonc
{
  "_comment": "...",
  "theme": {
    "tokens":  { "accent":[r,g,b,a], "track":[..], "fill":[..], "handle":[..], "cell":[..],
                 "hot":[..], "panel_bg":[..], "border":[..] },
    "styles":  { "slider":{track,fill,handle,handle_w,tick?}, "checkbox":{...}, "dropdown":{...},
                 "stepper":{...}, "button":{...}, "panel":{bg,border?} }   // values may be "$token" or literal
  },
  // ONE widget object shape, discriminated by `type`:
  "<name>": { "type": "label|panel|button|slider|toggle|dropdown|stepper|section_header|
                        list|grid|row|column|timeline|tabs|radio|legend",
              "id": "<stable Model key; required for interactive types>",
              "label": "...", "min","max","step","default","fmt",       // numeric
              "options": ["..."] | [{id,label,disabled?}],               // dropdown/radio/tabs
              "items":   [{id,label,...}],                               // list/grid/tabs children
              "style":   "$slider" | { ...inline },
              "layout":  {x,y,w,h} | {gap,pad,cols,col_x[],row_h,anchor:"tl|tr|.."} }
}
```
The **flat vs textured** dialect fork collapses to *data*: `panel` with a `texture` ⇒ 9-slice, absent
⇒ flat tinted rect; palette differences become **theme tokens** referenced by name.

## 4. Consolidation plan (unify the four copies)

1. **Split chrome vs scene.** Chrome (`logo/modal/screens/settings/loading`) is shared; each app's
   `hud`/scene block stays its own but in the one vocabulary.
2. **`Alpha/content/resources/ui_elements.json` = canonical chrome.** Each app's file shrinks to
   `{ "$extends": chrome, <scene block> }`. Reconcile the two byte-divergences (modal/loading
   `panel.h` 420 vs 384; voxel's 3 extra celestial checkboxes).
3. **Collapse the two dialects** into one schema (difference = data + theme tokens, not structure).
4. **One copy of `logo.lua`/`modal.lua`** under `Alpha/content/scripts`; reconcile the 3 divergent
   `settings.lua` into one nav+tabs-capable script; **route its private slider/checkbox/dropdown
   through the shared `Widgets` toolkit** (delete the re-impls).
5. **Collapse the lever-table triplication** (`world.rs PARAM_DEFS` 6-epoch / `epoch_defaults.json`
   9-epoch / `ui_elements.json hud.epochs`) → **one source: `epoch_defaults.json`**. The viewer
   builds param panels from `engine.params()`; `ui_elements.json` carries only labels/layout;
   `world.rs PARAM_DEFS` is retired. Same for abundance (`abundance.json`).
6. **Standardize the id field** to `id` everywhere (fix `key`/`sym` → `id` + a separate `sym` label).
7. **Retire the flicker-world shell fork** once theme-as-data + client-contributed settings land —
   flicker-world drops its `shell.rs`/chrome scripts and depends on `flicker-shell`, keeping only its scene.

## 5. Boundary work (blocked — flag, don't smuggle in)

- **Scroll / scissor** for scroll-lists: needs a `Model.scroll` convention or a `clip` HudCommand.
- **Text/numeric input:** needs keyboard/text across the boundary (`update()` is mouse-only).

These are separately-decided boundary changes, not part of formalizing-for-the-timeline. The
element/compound lists can ship as fixed-height (no scroll) until then.

## 6. First steps — status

- [x] **1. This doc** — the catalog + control set + schema.
- [x] **Toolkit promoted to `Alpha/crates/frontend/flicker-widgets`** (per the user) — moved
  `widgets.lua` + the render bridge + loader there; `crates/flicker-ui` is now a thin
  `pub use flicker_widgets::*;` re-export so no consumer changes. Workspace builds green.
- [x] **2. Missing widgets added** to `widgets.lua` as `*_update`/`*_draw` pairs on the Model+state
  contract: `layout_rows`/`layout_grid`, `radio_*` (segmented, with disabled state),
  `legend_draw` (colorbar), `list_*` (fixed-height; scroll deferred as boundary work), and
  `timeline_*` (9-epoch duration-weighted segments + 3 group bands + draggable playhead). Validated
  by a cargo test that eval-loads the toolkit through `ScriptHost` (`widgets_lua_parses_and_evaluates`).
- [~] **3. Wire the timeline** into the viewer scene — **DONE for the scrubber** (2026-07-08).
  The `flicker-pocepochs` `WorldScene` now hosts a `ScriptHost` + `Widgets` and drives the new
  `Widgets.timeline_*` widget via a self-contained `src/world_hud.lua` (no `ui_elements.json` yet —
  layout/colours are inline consts, a deliberate thin slice that avoids the content-unification in
  steps 4-5). The bottom bar shows 9 duration-weighted segments (weighted by each epoch's
  `e*_duration` lever) + the 3 group bands (I/II/III); **dragging the playhead scrubs the epoch and
  forward-regenerates** whatever epoch it lands in; `ui_capture` suppresses the orbit camera while
  the pointer is on the bar; ↑/↓ still nudge the epoch (snapping the playhead to the segment centre).
  Verified headless (`scene::tests::timeline_hud_loads_and_drives_the_widget` runs update+draw
  against the real `widgets.lua`); the user verifies the visual drag. **Still deferred:** transport
  (play/pause + speed), and the rest of the timeline-viewer control set (§2 items 3-11 — per-epoch
  param panels, element-mix grid, view/legend, seed buttons).
- [ ] **4. Collapse the lever/abundance triplication** → viewer builds panels from `engine.params()`;
  retire `world.rs PARAM_DEFS`. *(Pending — touches the live flicker-world viewer.)*
- [ ] **5. Unify the chrome** — canonical `Alpha/content/resources/ui_elements.json` + theme tokens +
  `$extends`; dedupe `logo/modal/settings.lua`; route `settings.lua` through the toolkit.
  *(Pending — content refactor across 4 files + the live viewers; do with the viewer build.)*
- [ ] **6. Bind the world-gen scene** to `WorldEngine` via the `id` contract (the viewer build).

**Landed this session:** the spec (§1-5), the crate promotion, and the widget toolkit additions
(all cargo-verified). **Remaining:** the content unification (steps 4-5) + wiring the viewer
(steps 3, 6) — best done together as the flicker-shell viewer is built, since they touch the live
`ui_elements.json` / `world_ui.lua` / `world.rs` and need the running window to verify.

## 7. Decisions (see conversation)

- **Toolkit home:** grow `crates/flicker-ui` in place (recommended) vs promote to
  `Alpha/crates/frontend/flicker-widgets`. Content consolidates to `Alpha/content` regardless.
- **Build scope:** build only the timeline-viewer widget set + spec the rest (recommended) vs the
  full MUI catalog.
- **Schema:** keep flat scalar `Model` round-trip + id-prefix rows (recommended, preserves the
  data-only invariant) vs widen the boundary to nested tables (a larger, separate change).
