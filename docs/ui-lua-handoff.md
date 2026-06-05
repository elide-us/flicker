# Handoff — Lua-driven UI + display/resolution rework

> Standalone handoff for a fresh Claude Code session. Re-verify anchors (symbols
> drift). Builds on `docs/ui.md` (current UI architecture), `docs/architecture.md`
> (engine/crate layering), and the scene system in `flicker-scene`. **Commit the
> current working tree first** (see "Verified state").

## Destination

Two things, in priority order:

1. **Move the front-end UI into the Lua layer.** Today the menus/pause/loading/
   settings are **hardcoded in Rust** (`examples/voxel-cluster/src/ui.rs` +
   `main.rs`). The Lua layer (`flicker-script` / `scripts/hud.lua`) only drives
   the in-game debug checkboxes. The goal: let Lua define the front-end UI
   **data-drivenly** through a generic Rust 2D interface — Lua owns layout /
   labels / interaction; Rust provides the gothic textures, `measure_text`, and
   draw primitives.
2. **Rework the display/resolution model** to fix a DPI/aspect wrinkle (below).

The whole front-end (logo → menu → game[boot→active] → pause, + a settings
panel and a confirm-or-revert overlay) is **built and working** in Rust; this is
a refactor of *where the UI is defined*, not new UI capability.

## Verified state (this session, checked against the repo)

- **Branch `main`.** Last commit `6b52978 Game State Manager, pre Lua UI refactor`
  has the scene system. **Uncommitted (commit before starting):** the UI/settings
  work — `crates/flicker-render/src/{renderer.rs,pipeline_text.rs}`,
  `examples/voxel-cluster/{Cargo.toml,.gitignore,src/{main.rs,ui.rs,display.rs}}`,
  `docs/ui.md`, `Cargo.lock`.
- **Builds clean.** `cargo build --workspace` OK. **fmt clean** on touched crates.
  **clippy:** only two *pre-existing* warnings in `main.rs` (`:422` `map_or` in
  `try_pick`, `:524` loop-index in `VirtualVoxel::build`) — both in untouched
  inspector code; everything new is clean.
- **Tests:** `flicker-scene` visibility test (1), `display` ladder tests (3),
  `flicker-voxel` ~119, etc. The `ui_preview` test is `#[ignore]` (writes PNGs to
  `target/`; run `cargo test -p voxel-cluster ui_preview -- --ignored`).
- **Functionally tested** by the user up to the menu: launch → logo → menu →
  resolution change → confirm overlay. **In-game (pause) settings + confirm
  over pause are unverified** (superseded by this refactor).

## Codebase map (where the UI lives)

- **`flicker-scene`** (engine) — `Scene` trait (`enter`/`update`→`Transition`/
  `render`/`exit`/`is_overlay`), `Transition` (`Replace`/`Push`/`Pop`/`Quit`/
  `None`), `SceneManager` (stack; updates only the top scene; renders the visible
  slice bottom-up; applies structural changes in `render`).
- **`flicker-render::Renderer`** (engine) — 2D: `draw_sprite`, `draw_text`,
  `draw_triangle`, **`measure_text(text,size)->Vec2`** (real glyphon metrics).
  Window: `set_windowed` / `set_borderless_fullscreen` /
  `set_exclusive_fullscreen` / `monitor_size` / `is_fullscreen` (winit stays
  behind the renderer). `measure_text` and the text pipeline `measure` are in
  `pipeline_text.rs`.
- **`examples/voxel-cluster/src/ui.rs`** — the gothic toolkit (example-local,
  promotable). Procedural raster art via a `Canvas` (stone fill, bevel, cracks,
  gold filigree scrollwork) baking the `Theme` textures (panel, button, white).
  Widgets: `modal_layout`/`ModalLayout`/`ModalButton`, `Theme::draw_panel`
  (title + optional subtitle + two buttons), `draw_loading`, `wordmark`,
  `scrim`/`backdrop`/`dim`, and `Dropdown` (primitive-drawn, dynamic height).
  Palette + sizing are consts at the top. **`build_button` is user-locked
  ("GREAT, don't touch").**
- **`examples/voxel-cluster/src/main.rs`** — the scenes (`LogoScene`,
  `MenuScene`, `GameScene`, `PauseScene`, `ConfirmDisplayScene`), the
  `SettingsPanel` (two dropdowns), `apply_display_change`, and the voxel demo.
- **`examples/voxel-cluster/src/display.rs`** — `DisplayMode`, `Resolution`,
  `resolution_options` (ladder), `DisplaySetting` (`apply`/`DEFAULT` = windowed
  1080), a process-wide `CURRENT` (`Mutex`), and **`settings.json` persistence**
  (`load_from_disk` at startup, `save_to_disk` on every `set_current`).
- **`flicker-script` + `scripts/hud.lua`** — the Lua seam. `ScriptHost::update`
  returns `Toggles` (named bools); `ScriptHost::draw` returns `Vec<HudCommand>`
  (`Rect{x,y,w,h,color}` / `Text{x,y,text,size,color}`). One-way today.
- **`examples/square-chase/src/main.rs`** — the minimal 2D interactivity
  reference: `draw_sprite` + `input.mouse_*` hit-testing + `renderer.window()`.

## Task 1 — Lua-driven front-end UI

> **✅ In progress — protocol foundation + main menu ported.** The seam already
> had the right shape (Lua owns layout/state/hit-testing; the engine renders
> plain-data commands and reads back named values — the XNA model in miniature),
> so we took the **Option-B-flavoured** path: *enrich the framework surface Lua
> calls* rather than impose a Rust widget tree. Landed in `flicker-script`:
> per-command `layer`, a `Sprite` command (engine textures referenced by id via
> the host-set `Textures` global / `ScriptHost::set_texture_ids`), `Text` with
> `align="center"` (the consumer measures + centres at draw time — keeps
> `measure_text` in Rust where `&mut Renderer` lives), and screen size passed
> into `update`/`draw` for responsive layout. The consumer has one shared
> `render_hud` path (used by every Lua screen) that maps commands → renderer
> calls and applies each command's `layer` relative to the scene's base.
> **Main menu ported:** `examples/voxel-cluster/scripts/menu.lua` owns the
> layout/labels/hit-testing (mirroring the old `modal_layout`/`draw_panel`
> pixel-for-pixel) and emits sprite/text commands using the gothic
> `Theme::lua_textures`; `MenuScene` is now a thin shell that loads the script,
> renders its commands, and routes the `start`/`quit` momentary actions to
> `Transition`s. Verified: builds clean, `flicker-script` tests green, and the
> menu renders + QUIT works in the running app. **Still Rust (next):** the
> settings dropdowns (still a Rust `SettingsPanel`), and the pause/logo/loading
> screens — port them next using the same surface. Toggles double as momentary
> actions, so no separate events channel was needed. The original design
> discussion below is kept for context.

**The seam problem.** Today Lua is one-way (emit `Rect`/`Text`, read `Toggles`).
Interactive widgets (buttons, dropdowns) need a **two-way** protocol: Lua
declares widgets (with stable ids + state like "open"); Rust hit-tests against
the real cursor + font metrics and reports back *which widget was
clicked/hovered/changed*. Decide the ownership split:

- **Option A — Rust owns widget state, Lua declares layout + reacts.** Lua emits
  a widget tree (panel/button/dropdown nodes with ids); Rust lays out (using
  `measure_text`), draws (using `Theme`), hit-tests, and returns events
  (`{id, kind: clicked|changed, value}`) for Lua's next `update`. This keeps the
  tricky interaction/measurement in Rust and makes Lua declarative — likely the
  cleanest. Extend `HudCommand` into a richer node vocabulary and add an
  `events` return channel alongside `Toggles`.
- **Option B — Lua owns everything, Rust is a thin draw/measure/​hit-test RPC.**
  More flexible, more Lua complexity, more round-trips per frame.

Recommendation: **Option A.** The `ui.rs` widgets (`Theme::draw_panel`,
`Dropdown`, `modal_layout`, hit-test rects) already encapsulate the
Rust-side machinery — they should **promote to a reusable crate (e.g.
`flicker-ui`)** with the generic widgets/layout/hit-test, leaving the
game-specific gothic *theme* (palette, procedural textures) in the example.
The Lua scripts then describe screens (logo/menu/pause/settings) as widget
trees; the scenes (`MenuScene` etc.) become thin shells that load a Lua screen
and route its events to `Transition`s.

**Concrete steps (suggested):**
1. Promote the generic widget/layout/hit-test machinery out of `ui.rs` into
   `flicker-ui` (keep the gothic theme example-local). `measure_text` lives in
   `flicker-render`; widgets take a `&mut Renderer`.
2. Extend the Lua protocol: a node-tree `HudCommand` vocabulary (panel, button,
   dropdown, label, spacer) with ids, and an `events`/`results` return so Lua
   gets clicks/selections. Mind that `measure_text` needs `&mut Renderer`
   (glyphon shaping) → it runs in `render`/`enter`; geometry needed in `update`
   is measured once and cached (the current `SettingsPanel` does this).
3. Port one screen first (the **main menu**) end-to-end as the vertical slice,
   then pause/settings/logo.

### Task 1b — 2D layer ordering (the modal-text bleed-through)

> **✅ Implemented (renderer-side).** Landed as the canonical DXTK-style model:
> an ambient `layer: f32` on the `Renderer` (`set_layer`/`layer`, reset each
> `begin_frame`) threaded into `draw_sprite`/`draw_text`/`draw_triangle`; each 2D
> pipeline tags items with their layer, stable-sorts by `(layer, submission)` in
> `prepare`, and exposes `layers()` + `render_layer()`; `end_frame` walks the
> union of layers ascending, drawing triangle → sprite → text per layer (pure
> painter's order, no depth buffer — `DepthNone`). Text uses a per-layer
> `TextRenderer` pool (glyphon renders all of a renderer's areas at once). The
> `SceneManager` sets `layer = absolute stack index` before each scene's
> `render`, so overlays sort above with **zero changes to `ui.rs`/scenes** — the
> modal bleed-through is fixed structurally. Default layer `0` reproduces the
> old triangle→sprite→text order exactly, so all existing screens are
> pixel-identical. Verified by a headless pixel-readback test
> (`crates/flicker-render/src/layering_test.rs`) proving a layer-1 sprite covers
> layer-0 text and that layer beats submission order. **Still open:** threading a
> per-node `layer` through the *Lua* protocol (Task 1) and the richer sprite
> surface (source-rect/rotation/scale/mirror — Task 1c below / the `Sprite`
> port). The diagnosis below is kept for context.

**Symptom.** The confirm overlay (`ConfirmDisplayScene`) draws its panel + KEEP/
REVERT buttons over the menu, but the menu's START/QUIT (and the settings-panel
labels) **bleed through** the dialog — you see both sets of text at once. The
25%-dim and the modal panel don't hide them.

**Root cause — it's cross-pipeline order, not the sprite batch.** `end_frame`
(`renderer.rs`) renders the 2D pipelines in a *fixed* order — `triangle → sprite
→ text` — and within each pipeline in submission order. There is **no per-draw
layer/z** on `draw_sprite`/`draw_text`/`draw_triangle` today (contrary to the
"sprite already takes a layer" hunch — it takes only `position`/`size`/`color`).
Consequence: **all text renders after all sprites, regardless of which scene
queued it.** So the modal's *sprites* (dim, panel, buttons) correctly cover the
menu's *sprites*, but the menu's *text* — queued by the scene below — still
paints on top of the modal's panel, because text is a strictly-later pipeline.
Sprite-vs-sprite ordering is already correct; sprite-vs-text is the break.

**What a fix needs.** A 2D *layer* key honoured **across** triangle/sprite/text,
not just within the sprite batch. An overlay scene draws at a higher base layer
than the scene beneath; within a scene the panel-fill sits below its label.

**Reference — XNA/DirectXTK `SpriteBatch` (`Repos/Toybox/FloodControl`).** The
classic `SpriteBatch`/`SpriteFont` pair (DirectXTK `Src/SpriteBatch.cpp`,
`SpriteFont.cpp`) is the pattern to derive from. The load-bearing ideas:
- **One unified 2D queue — text *is* sprites.** There is a single `mSpriteQueue`.
  `SpriteFont::DrawString` doesn't use a separate path; it emits **each glyph as
  `spriteBatch->Draw(atlasTexture, …, layerDepth)`** (`SpriteFont.cpp:343`). So
  glyph quads and ordinary quads share one queue, one sort, one batch loop. This
  is exactly the seam flicker lacks: our glyphon text is a wholly separate
  pipeline that always renders last. **The bleed-through is a direct symptom of
  *not* having a unified queue.**
- **Per-draw `layerDepth` (a float, 0..1)** carried on every sprite (DirectXTK
  packs it in `originRotationDepth.w`). This is the "layer/z" the sprite call
  should take.
- **`SpriteSortMode`** picks the ordering policy at `Begin`: `Deferred` (keep
  submission order, batch by texture — stable), `Texture` (sort by texture, max
  batching), `BackToFront` / `FrontToBack` (sort by `layerDepth`), `Immediate`
  (no batching). `FlushBatch` *sorts*, then walks the sorted list coalescing
  adjacent same-texture runs into one draw call (`SpriteBatch.cpp` `FlushBatch`/
  `SortSprites`). Our sprite pipeline already does the coalesce step (`runs`) —
  it just never sorts and never sees text.
- **The unstable-sort gotcha.** DirectXTK uses `std::sort` (not `stable_sort`) for
  the depth modes, so equal-`layerDepth` sprites can reorder unpredictably. For
  us: sort by the key **`(layer, submission_index)`** (or use a stable sort) so
  ties break by submission order. *That tie-break is "the sprite batching order"*
  — make it explicit, don't inherit an unstable one.
- **Painter's order, not GPU depth, for 2D.** Alpha-blended UI must draw
  back-to-front on the CPU; the depth buffer can't reject correctly because
  blended fragments don't carry meaningful depth. This **rules out** the
  "depth-test+write the 2D pipelines" idea floated earlier — confirmed fragile,
  drop it.
- **Atlas source-rects + `SpriteEffects` (mirroring) + scale/rotation/origin.**
  `Draw` takes a `sourceRectangle` (a sub-read of a sheet — FloodControl's
  `GamePiece::GetSourceRect` carves 40×40 cells out of one texture) and
  `SpriteEffects_Flip{Horizontally,Vertically}`. Our `draw_sprite` is full-UV
  (0..1) only; adding `source: Option<Rect>` lets the gothic theme bake into one
  atlas → fewer texture swaps → longer coalesced runs. Not required for the bug
  fix, but it's the same lever that makes batching pay off.

**Design for flicker (derived).** Give `draw_sprite`/`draw_text`/`draw_triangle`
a `layer: f32` (or `u16`). Don't unify into glyphon's internals yet — instead
generalize DirectXTK's `FlushBatch` *across pipelines*: collect all 2D draws into
one list keyed by `(layer, submission_index)`, sort stably, then walk it issuing
the owning pipeline's draws and **flushing on pipeline change** (the same loop
DirectXTK runs, flushing on *texture* change). Text runs do a glyphon
prepare+render scoped to that run — several small glyphon prepares/renders per
frame instead of one, which is nothing at UI volumes. The eventual "north star"
(only if text/sprite interleaving gets heavy) is full DirectXTK-style
unification: rasterize glyphs into an atlas and emit them as ordinary sprites, so
there's literally one queue — but that's a later phase, not needed for the bug.

**Confirmed by the XNA games (`Repos/xna/…-ArchiveProjects-XNA`: FloodControl,
RobotRampage, AsteroidBeltAssault — the Kurt Jaegers book projects).** In
practice these ship the *simplest* form of the above: `spriteBatch.Begin()` with
**no sort mode** (= `Deferred`, submission order), and **all** layering comes from
*draw-call order in one batch*. RobotRampage's `Game1.Draw` is literally `TileMap
→ WeaponManager → Player → EnemyManager → EffectsManager → GoalManager →
DrawString(HUD)` — the HUD text sits on top purely because it's the last
submission into the **same** batch as the sprites; every draw passes
`layerDepth = 0`. That is the invariant flicker breaks: text in a separate
always-last pipeline. So the minimum correct fix is "one batch, submission order,
text interleaved"; `layer` is the escape hatch for when draw order must differ
from sort order. The reusable API surface to standardize toward (all three games
share it, near-verbatim):
- **`Sprite`** (`RobotRampage/Sprite.cs`, `AsteroidBeltAssault/Sprite.cs`):
  `Texture` + `List<Rectangle> frames` (atlas source-rects) + `currentFrame`/
  `frameTime` animation, `TintColor` (alpha), `Rotation`, origin
  (`RelativeCenter`), `Draw` → `spriteBatch.Draw(tex, screenPos, source, tint,
  rotation, origin, scale, SpriteEffects, layerDepth)`. Plus collision helpers
  (`BoundingBoxRect`, `IsBoxColliding`/`IsCircleColliding`) — out of scope for
  render but note they hang off the same sprite.
- **`Camera`** (`RobotRampage/Camera.cs`): a 2D scroll camera — `Position`
  (clamped to a world rect), `ViewPort`, `Transform(point|rect)` world→screen,
  `ObjectIsVisible(rect)` cull. Sprites cull on `Camera.ObjectIsVisible` before
  `Draw`. (XNA also accepts a transform matrix in `Begin` for the same effect.)
- **`TileMap`** (`RobotRampage/TileMap.cs`): sheet texture + `List<Rectangle>
  tiles` + `int[,]` grid; `Draw` iterates only the **visible window** (camera
  cull) emitting `Draw(tex, screenRect, sourceRect, …)` — the 32×32-cell atlas
  read, the grown-up "16×16 subreads of a 256×256 sheet."

**Tie-in with Task 1.** This is the renderer half of "Lua handles batching
order": the Lua node vocabulary carries a `layer`/`z` per node (or derives it from
tree depth + an explicit `layer` on overlay roots — the "UI tree owns ordering"
pattern), with the scene base layer coming from the scene-stack position. But
*honouring* the layer is a `flicker-render` change — Lua can declare layers, yet
nothing moves until the renderer interleaves sprite+text by layer. **The renderer
ordering can and should land first:** it fixes the current bug on its own (even
pre-Lua, by giving the scenes a per-draw layer), then the layer field threads
through the Lua protocol unchanged.

## Task 2 — display/resolution rework

**The wrinkle.** `display::resolution_options` scales the ladder rungs to the
*monitor's* aspect (`round(height * aspect)`), so on a **non-16:9** display the
"1080" rung isn't `1920×1080` and the **1080 default's dropdown highlight can
fall back to 540**. Also windowed sizing is physical pixels while the runner
opens a `960×540` *logical* window (so the effective default is 1080 on a 2×
display, 540 on 1×).

**Suggested fix:** make **windowed** rungs a fixed 16:9 ladder
(`960×540` debug, `1280×720`, `1920×1080`, `2560×1440`) filtered to `≤` native
height, plus **Native** (the monitor's true size) for fullscreen. That keeps the
1080 default always present + highlighted, honours "filter 540p up", and gives a
correct in-ratio option via Native. Decide windowed sizing in **logical vs
physical** pixels for consistent cross-DPI behaviour. Update the `display` unit
tests accordingly. (User OK'd revisiting the "in-ratio per rung" idea here.)

## Invariants — do not break

1. **Scene model.** Only the top scene updates; overlays (`is_overlay`) render
   over the scene below; structural transitions apply in `render` (they need
   `&mut Renderer` for `enter`/`exit`). A pushed overlay (pause, confirm) freezes
   everything beneath — that's what makes modals block interaction.
2. **`measure_text` needs `&mut Renderer`** (glyphon shaping mutates the font
   system) → call it from `render`/`enter`; cache any widths needed in `update`.
3. **Window is the display source of truth;** `display::CURRENT` mirrors it and
   `settings.json` persists it (`set_current` writes; `load_from_disk` +
   `LogoScene::enter`-applies at launch). Default = windowed 1080; `DEBUG_RES`
   (540) stays the ladder's bottom rung.
4. **No binary UI assets** — `ui.rs` art is procedural/deterministic.
5. **`build_button` is locked** (user loved it); don't restyle it.
6. **Confirm-or-revert UX:** resolution / exclusive-fullscreen changes apply
   instantly, push `ConfirmDisplayScene` (25% dim so the new res shows through,
   blocks interaction, 15s auto-revert with Keep/Revert). Windowed/borderless
   apply outright.

## Confirm first (open decisions)

- **Lua ownership split** (Option A vs B above) — recommend A.
- **Promote widgets to `flicker-ui`?** vs keep in the example for now. (The
  theme is game-specific regardless.)
- **Resolution rework now or alongside the Lua port?** They're independent;
  the Lua port is the bigger lift.
- **Generalize `settings.json`** beyond display (audio/controls) when a second
  setting appears — currently display-only, serde-derived, next to the crate
  (`/settings.json`, git-ignored).

## Pinned — parked

- A `flicker-ui` crate is the natural home for the generic widgets, but only
  worth extracting once the Lua protocol shape is settled (don't pre-factor).
- The voxel "world generator / bounded horizon / LOD8 backdrop" work
  (`docs/voxel-terrain-walking-handoff.md`, Phase B) is **unrelated and still
  open** — different track from the UI work here.
