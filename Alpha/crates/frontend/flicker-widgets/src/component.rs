//! The **Rust component walker** — the engine half of the component-UI model.
//!
//! A screen declares a tree of [`UiNode`]s (via its Lua `tree()` builder, parsed
//! by `flicker-script`); this module OWNS the rest: it lays the tree out into
//! rects, hit-tests the pointer against it, and draws each node with its Rust
//! **template**. `HudCommand` is the templates' internal output (fed to the
//! existing [`render_hud`](crate::render_hud)); it no longer crosses the Lua
//! boundary. Interaction rides the same two-way name channels the immediate HUD
//! used: a node's `bind` ↔ a `Model` key (values), its `action` → an event name,
//! both returned in the [`UiFrame::results`] `ValueMap`. So an app swaps
//! `script.update`+`script.draw`+`render_hud` for [`run_ui`] + `render_hud` and
//! keeps applying the very same result keys.
//!
//! Templates read their colours/sizes from the resolved `ui_elements.json` by a
//! dotted `style` path (`"paperdoll.fit.slider"`) — so the palette stays in one
//! place (Prism `theme.tokens`) and a node carries only its truly-local data.
//!
//! This is a match-based template registry today (one arm per component kind);
//! the arms are the "component definitions" (ContentForge `ComponentEntry`s) and
//! new kinds are added here in one place.

use std::collections::{HashMap, HashSet};

use flicker_render::Vec2;
use flicker_script::{
    ComponentLibrary, FontRole, HitShape, HitVerdict, HudCommand, TextAlign, UiAnchor, UiNode,
    Value, ValueMap,
};
use serde_json::Value as Json;

/// The per-frame interaction snapshot handed to [`run_ui`] — the same data the
/// legacy `ScriptHost::update` received, in one struct.
pub struct UiInput {
    /// Cursor position (pixels, top-left origin).
    pub mouse: Vec2,
    /// Left-button press *edge* this frame (a fresh click).
    pub clicked: bool,
    /// Left-button *held* state (for slider drags).
    pub down: bool,
    /// Screen size (the root layout rect).
    pub screen: Vec2,
    /// Text committed by the keyboard this frame — appended to a focused
    /// `text_field`'s bound string. Empty on non-typing frames and for scenes with
    /// no keyboard wiring yet.
    pub typed: String,
    /// Backspace *edge* this frame — pops one char from a focused `text_field`.
    pub backspace: bool,
    /// This frame's mouse-wheel delta (positive = scroll up), consumed by the
    /// `list` region under the pointer. Scenes wire their engine snapshot's
    /// `mouse_wheel_delta` straight in; `0.0` on wheel-less frames.
    pub wheel: f32,
}

// ── Draw cache ───────────────────────────────────────────────────────────────

/// FNV-1a, 64-bit — the fingerprint fold. Chosen over `DefaultHasher` because it is
/// seedable (so a child's hash can continue its parent's), allocation-free, and
/// stable within a run; the fingerprint never leaves the process, so cross-version
/// stability is not a requirement.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    /// Continue from an existing hash — how a child folds its parent's key in.
    fn seed(v: u64) -> Self {
        Fnv(v)
    }
    fn bytes(&mut self, b: &[u8]) {
        for byte in b {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    /// Bit pattern, not value — NaN and ±0 hash distinctly, which is what a
    /// "did this change" test wants (and no rect ever holds a NaN in practice).
    fn f32(&mut self, v: f32) {
        self.u64(v.to_bits() as u64);
    }
    fn bool(&mut self, v: bool) {
        self.u64(v as u64);
    }
    /// Length-terminated so `"ab" + "c"` and `"a" + "bc"` differ.
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
        self.u64(s.len() as u64);
    }
    fn value(&mut self, v: &Value) {
        match v {
            Value::Bool(b) => {
                self.u64(1);
                self.bool(*b)
            }
            Value::Number(n) => {
                self.u64(2);
                self.u64(n.to_bits())
            }
            Value::Text(t) => {
                self.u64(3);
                self.str(t)
            }
        }
    }
    /// Fold a resolved STYLE BLOCK. Content, never address: a scene is free to rebuild
    /// an equal styles tree each frame, and a hot-reloaded one must invalidate. Blocks
    /// are a handful of keys, and `component_props` already deep-CLONES the same block
    /// every time it builds props — so hashing it is cheaper than the work it saves.
    fn json(&mut self, v: &Json) {
        match v {
            Json::Null => self.u64(0),
            Json::Bool(b) => {
                self.u64(1);
                self.bool(*b)
            }
            Json::Number(n) => {
                self.u64(2);
                self.u64(n.as_f64().unwrap_or(f64::NAN).to_bits())
            }
            Json::String(s) => {
                self.u64(3);
                self.str(s)
            }
            Json::Array(a) => {
                self.u64(4);
                self.u64(a.len() as u64);
                for e in a {
                    self.json(e);
                }
            }
            Json::Object(o) => {
                self.u64(5);
                let mut fold = 0u64;
                for (k, val) in o {
                    let mut e = Fnv::new();
                    e.str(k);
                    e.json(val);
                    fold ^= e.finish();
                }
                self.u64(fold);
            }
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// One cached node draw: the commands it emitted last time, and the fingerprint of
/// every input that produced them.
struct CacheEntry {
    /// Fold of everything the node's draw reads — see [`node_fingerprint`].
    fingerprint: u64,
    /// The emitted commands, already lifted onto the node's sub-layer, so a replay is
    /// a straight copy with no post-processing.
    commands: Vec<HudCommand>,
    /// The `Model`/results keys this node's draw reads (its `bind`, every `*_bind`
    /// prop's target, its `focus_group`). Built once when the entry is created —
    /// which keys a node reads is a property of the node, not of the frame.
    read_keys: Vec<String>,
    /// The plain-data props the last real draw marshalled for a LUA component
    /// (`None` for Rust-drawn kinds). The hit dispatch reuses these — patching only
    /// the live fields (`bind_value`/`open`/`captured`) — instead of rebuilding the
    /// whole map per hit call on an unchanged node.
    props: Option<Json>,
    /// Last frame this entry was used, for eviction.
    touched: u64,
}

/// The retained per-node draw cache — the mechanism that makes the Lua component
/// dispatch **bounded** (draw-on-change), as ratified: a node re-enters Lua only when
/// one of its inputs actually changed, so a still frame crosses the boundary zero
/// times instead of once per node.
///
/// It caches EVERY node's draw, not just the Lua-dispatched ones: `text` is the most
/// numerous kind in a real tree and its Rust arm formats a string and pushes a command
/// every frame for a label that almost never changes.
#[derive(Default)]
struct DrawCache {
    entries: HashMap<u64, CacheEntry>,
    /// Monotonic frame counter driving eviction.
    frame: u64,
}

/// How much work one [`run_ui_with`] pass actually did — the observable that keeps
/// the bounded-dispatch guarantee honest (a test asserts `lua_draws == 0` on a still
/// frame). Counters, not timings, so they cost nothing in a release build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiStats {
    /// Nodes that crossed into the Lua component library to draw this frame.
    pub lua_draws: u32,
    /// Nodes that crossed into the Lua component library to HIT-TEST this frame.
    /// Bounded like the draws: dispatch happens only on input-active frames, and only
    /// for candidate nodes (pointer near the rect / the open popup / a captured drag)
    /// — an idle frame is 0.
    pub lua_hits: u32,
    /// Nodes redrawn this frame (Lua or Rust) — the rest replayed from the cache.
    pub redraw_nodes: u32,
    /// Nodes laid out this frame (the denominator for the two above).
    pub nodes: u32,
}

/// What a drag-source node picked up — the payload a **scene-owned** canvas resolves
/// on release (e.g. `kind: "clip", id: "walk_forward"` dropped onto a state node).
/// The walker only carries the payload; it never decides what a drop means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragPayload {
    /// Category of the dragged thing, from the source node's `drag_kind` prop.
    pub kind: String,
    /// Identity of the dragged thing — the source node's `drag_id` prop, else its `id`.
    pub id: String,
}

/// Retained interaction state the caller holds across frames: the sliders capturing
/// the mouse mid-drag (keyed by node id/bind), plus the in-flight drag payload. A
/// slider drag keeps updating — and keeps claiming the mouse — until the button
/// releases, even if the cursor leaves the track.
#[derive(Default)]
pub struct UiState {
    dragging: HashSet<String>,
    drag: Option<DragPayload>,
    /// Retained draw cache — the bounded-dispatch mechanism. Lives here because this
    /// is the walker's one across-frames object, and because a scene running two
    /// walker passes (a HUD and a floating chat panel) holds two `UiState`s and so
    /// gets two independent caches for free.
    cache: DrawCache,
    /// The id of the single currently-open `select` popup, or `None`. Set/cleared
    /// entirely within the select hit arm (a closed field's click opens it; while
    /// open, any click closes it) — `derive(Default)` starts it `None`.
    open: Option<String>,
    /// The id of the `text_field` that currently owns keyboard focus, or `None`.
    /// run_ui clears it at the top of any clicked frame; a click landing in a
    /// text_field re-establishes it through the module's `focus` verdict
    /// (`ui/text_field.lua` → [`apply_hit_verdict`]) — `Default` starts `None`.
    focus: Option<String>,
    /// Previous frame's `(mouse, screen)` — the "did input actually change" test the
    /// bounded HIT dispatch gates on. Screen rides along so a resize re-tests hover
    /// under a stationary cursor. `None` (first frame) counts as changed.
    last_mouse: Option<(Vec2, Vec2)>,
    /// Previous frame's button-held state: the RELEASE edge is input activity too (a
    /// capture dying changes verdicts), so the frame after a drag re-evaluates instead
    /// of replaying the mid-drag memo.
    last_down: bool,
    /// Per-node (by [`Placed::key`]) result of the LAST component hit dispatch —
    /// whether that node claimed the pointer. Idle frames replay this instead of
    /// crossing into Lua, so `hud_hit` stays continuous at zero cost; an input-active
    /// frame refreshes (candidate → dispatch) or clears (pointer left) each entry.
    hit_memo: HashMap<u64, bool>,
}

impl UiState {
    /// A fresh, empty interaction state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The payload currently in flight, if any — so a scene-owned canvas can
    /// highlight valid drop targets mid-drag.
    pub fn drag(&self) -> Option<&DragPayload> {
        self.drag.as_ref()
    }

    /// Abandon an in-flight drag (e.g. the scene rejected the drop).
    pub fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// The id of the `text_field` that currently owns keyboard focus, if any.
    pub fn focused(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// Programmatically give keyboard focus to a `text_field` by its node `id` —
    /// the scene's hook for focus-by-keypress (e.g. pressing **T** to enter chat),
    /// since focus is otherwise established only by a click landing in the field.
    /// `run_ui` clears focus at the top of any *clicked* frame, so a scene that
    /// wants a field to stay focused across clicks elsewhere re-asserts this each
    /// frame BEFORE `run_ui`.
    pub fn request_focus(&mut self, id: impl Into<String>) {
        self.focus = Some(id.into());
    }

    /// Drop keyboard focus (e.g. Escape leaves the chat input).
    pub fn clear_focus(&mut self) {
        self.focus = None;
    }
}

/// One `rtt` node's reserved picture-in-picture slot — a rect the walker laid
/// out but deliberately does not fill.
///
/// The walker runs late (its commands are main-frame draws), while
/// `FrameGraph::execute` must run FIRST in a scene's `render()` — the offscreen
/// passes reset the shared per-frame draw queues. A `rtt` node therefore
/// *reserves* its rect here, and the scene feeds the slot to its frame graph:
///
/// ```text
/// fg.target(handle, clear, |r| draw_source(r, source));
/// fg.composite_panel(handle, CompositeTarget::Screen, rect, layer, tint, None, None);
/// ```
///
/// Passing `frame: None` to that composite is deliberate: the walker has already
/// drawn the node's `panel` style as the backdrop through the normal 2D path, so
/// the graph only blits the image. That keeps every panel in the codebase drawn
/// by exactly one code path.
#[derive(Clone, Debug, PartialEq)]
pub struct RttSlot {
    /// The node's `id` — a scene keys its per-slot render target off this.
    pub id: String,
    /// Which `stages.<source>` sub-scene to render (the node's `source` prop).
    pub source: String,
    /// The IMAGE rect in screen pixels — already inset inside the node's frame.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Sub-layer, matching the node's own draw commands.
    pub layer: f32,
    /// Whether this slot should render a FRESH target this frame; `false` means the
    /// scene should blit its cached poster instead. N live targets cost N GPU
    /// submits per frame and a pack-browser screen carries ~14 stages, so liveness
    /// is authored data (`live` / `live_bind`), not a renderer detail.
    pub live: bool,
    /// Composite tint (default opaque white), from the node's `tint` dotted colour
    /// path or its style block.
    pub tint: [f32; 4],
}

/// The output of one [`run_ui`] pass: the draw commands (for
/// [`render_hud`](crate::render_hud)) and the result values / fired events (for
/// the engine to apply — identical in shape to the old `update` return).
pub struct UiFrame {
    /// Draw commands, in painter's order — feed straight to `render_hud`.
    pub commands: Vec<HudCommand>,
    /// Toggles / slider values / fired actions, plus `hud_hit` (pointer over any
    /// UI region — so the scene behind must not pick through).
    pub results: ValueMap,
    /// PiP slots reserved by `rtt` nodes this frame — see [`RttSlot`]. Empty
    /// for a tree with no stages, so existing callers are unaffected.
    pub rtts: Vec<RttSlot>,
    /// How much drawing this pass actually did — see [`UiStats`].
    pub stats: UiStats,
}

/// A geometry rect (pixels).
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }
    fn inset(&self, p: f32) -> Rect {
        self.inset_xy(p, p)
    }
    /// Inset left/right by `px`, top/bottom by `py` — per-axis padding. Clamped so a
    /// pad wider than the rect yields a zero (never negative) inner extent, keeping
    /// layout stable when a bar's horizontal inset would otherwise exceed its width.
    fn inset_xy(&self, px: f32, py: f32) -> Rect {
        Rect {
            x: self.x + px,
            y: self.y + py,
            w: (self.w - 2.0 * px).max(0.0),
            h: (self.h - 2.0 * py).max(0.0),
        }
    }
}

/// Effective horizontal inset for a node: `pad_x` when set, else the uniform `pad`.
fn pad_x(n: &UiNode) -> f32 {
    n.pad_x.unwrap_or(n.pad)
}
/// Effective vertical inset for a node: `pad_y` when set, else the uniform `pad`.
fn pad_y(n: &UiNode) -> f32 {
    n.pad_y.unwrap_or(n.pad)
}

/// A laid-out node: its resolved rect, whether it is interactive this frame, and
/// its sub-layer (accumulated down the tree from each node's optional `layer`
/// prop), so a node's draw commands can be lifted above / dropped below its
/// siblings' — e.g. the menu's Muse sprite sitting BELOW the popup panel.
struct Placed<'a> {
    node: &'a UiNode,
    rect: Rect,
    enabled: bool,
    layer: f32,
    /// Scissor clip inherited from a `list` ancestor (px x,y,w,h), or `None`.
    /// Propagated in `resolve`; the draw pass emits a `HudCommand::Clip` when it
    /// changes between placed nodes (tree order keeps a list subtree contiguous).
    clip: Option<[f32; 4]>,
    /// Stable identity for the retained [`DrawCache`] — this node's `id` when it has
    /// one, else its structural path (parent key ⊕ kind ⊕ sibling index). Structural
    /// rather than positional so it survives a tree REBUILT from scratch each frame
    /// (flicker-loomforge and the chat panel do exactly that).
    key: u64,
}

// ── Run ────────────────────────────────────────────────────────────────────

/// Lay out, hit-test, and draw a component `tree` for one frame. `model` is the
/// engine's published values (read by `bind`), `styles` the resolved
/// `ui_elements.json` (colours/sizes by dotted `style` path), `input` the
/// pointer snapshot, `state` the retained drag capture. Returns the draw
/// commands + the results `ValueMap`. Components draw AND hit-test through the
/// built-in embedded library ([`UI_COMPONENT_MODULES`](crate::UI_COMPONENT_MODULES))
/// — see [`run_ui_with`] to substitute one.
pub fn run_ui(
    tree: &UiNode,
    model: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &mut UiState,
) -> UiFrame {
    run_ui_with(tree, model, styles, input, state, None)
}

// The built-in component library a `None` lib falls back to, so `run_ui` (and any
// caller whose own host failed to load) still has working component draw + hit
// logic — since S4 the per-control behaviour lives ONLY in `ui/<kind>.lua`, there
// is no Rust twin left to fall back on. One per thread (the Luau VM is
// single-threaded state), built lazily on first use.
thread_local! {
    static DEFAULT_LIB: Option<flicker_script::ScriptHost> =
        match flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES) {
            Ok(host) => Some(host),
            Err(e) => {
                tracing::error!("built-in ui component library failed to load: {e}");
                None
            }
        };
}

/// Like [`run_ui`], but with an optional Lua [`ComponentLibrary`]: any node whose
/// `component` kind the library `handles` has its DRAW and HIT dispatched to
/// `ui/<kind>.lua` (component logic in Lua, rendering in Rust). Everything else —
/// layout, retained state, generic hit plumbing, result routing — is identical.
/// `None` falls back to the built-in embedded library (exactly what [`run_ui`]
/// passes); a screen whose own `ScriptHost` carries the modules passes `Some(host)`
/// so components and scene script share one VM.
pub fn run_ui_with(
    tree: &UiNode,
    model: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &mut UiState,
    lib: Option<&dyn ComponentLibrary>,
) -> UiFrame {
    match lib {
        Some(lib) => run_ui_impl(tree, model, styles, input, state, Some(lib)),
        None => DEFAULT_LIB.with(|dl| {
            run_ui_impl(
                tree,
                model,
                styles,
                input,
                state,
                dl.as_ref().map(|h| h as &dyn ComponentLibrary),
            )
        }),
    }
}

fn run_ui_impl(
    tree: &UiNode,
    model: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &mut UiState,
    lib: Option<&dyn ComponentLibrary>,
) -> UiFrame {
    let screen = Rect { x: 0.0, y: 0.0, w: input.screen.x, h: input.screen.y };
    let mut placed = Vec::new();
    resolve(tree, screen, model, 0.0, None, child_key(0, tree, 0), &mut placed);

    // Hit-test pass: fold events + value edits into `results`, drag into `state`.
    let mut results = ValueMap::new();
    let mut hud_hit = false;
    // Click-away de-focus: a fresh click clears text_field focus up front; a click
    // that lands in a text_field re-establishes it in that field's hit arm below.
    if input.clicked {
        state.focus = None;
    }
    // Capture release is a GENERIC rule: everything captured (slider drags) lets go
    // the frame the button is up, before any hit runs — so the release frame already
    // reports the echo value, exactly as the old per-arm `dragging.remove` did.
    if !input.down {
        state.dragging.clear();
    }
    // The bounded-dispatch gate for component HIT calls: Lua is entered only when the
    // input could change a verdict — a click edge, a RELEASE edge (a capture just
    // died), an actual pointer/screen change, a wheel tick (a `list` under a parked
    // pointer must still scroll), or a held drag (which keeps writing while the
    // button is down). A still frame replays each node's memoized `hit` instead
    // (see `dispatch_component_hit`).
    let pointer_moved = state.last_mouse != Some((input.mouse, input.screen));
    state.last_mouse = Some((input.mouse, input.screen));
    let released = state.last_down && !input.down;
    state.last_down = input.down;
    let input_active = input.clicked
        || released
        || pointer_moved
        || input.wheel != 0.0
        || (input.down && !state.dragging.is_empty());
    let mut lua_hits: u32 = 0;
    for p in &placed {
        hit_node(p, model, input, state, styles, &mut results, &mut hud_hit, lib, input_active, &mut lua_hits);
    }
    // The generic every-frame TYPED-FOLD: this frame's keyboard input flows into the
    // FOCUSED node's bound string, in Rust, whatever the pointer is doing — see
    // [`fold_typed`]. After the hit pass (a click that just focused a field folds the
    // same frame), before the echo (an edit must never be shadowed).
    fold_typed(&placed, model, input, state, &mut results);
    // The generic every-frame BIND ECHO: every placed control with a `bind` whose key
    // no interaction wrote this frame reports its effective value, so an engine that
    // reads the keys unconditionally stays in sync on idle frames too (the paperdoll
    // HUD asserts this contract). Runs AFTER the hit pass, so a click always wins
    // over an echo whatever the placement order.
    echo_binds(&placed, model, &mut results);
    if !state.dragging.is_empty() {
        hud_hit = true;
    }

    // Drag channel: publish the in-flight payload and the release edge so a scene-owned
    // canvas can resolve the drop against its own geometry. Deliberately does NOT force
    // `hud_hit` — the drop usually lands on the scene (a graph node), not on the UI, so
    // the scene must still be allowed to pick.
    if let Some(d) = state.drag.clone() {
        results.set("drag_kind", d.kind.as_str());
        results.set("drag_id", d.id.as_str());
        if input.down {
            results.set("drag_active", true);
        } else {
            results.set("drag_dropped", true);
            state.drag = None;
        }
    }

    results.set("hud_hit", hud_hit);

    // Draw pass: values reflect this frame's edits (results override model).
    //
    // Every node's commands are RETAINED and replayed unless one of its inputs changed
    // (see `DrawCache`) — the ratified bounded-dispatch rule. The cache is moved out of
    // `state` for the pass so the per-node draw can keep borrowing `state` immutably.
    let mut commands = Vec::new();
    let mut stats = UiStats { nodes: placed.len() as u32, ..UiStats::default() };
    let mut cache = std::mem::take(&mut state.cache);
    cache.frame = cache.frame.wrapping_add(1);
    let frame = cache.frame;
    let mut cur_clip: Option<[f32; 4]> = None;
    for p in &placed {
        // Toggle the scissor clip at each list boundary: a list node's
        // descendants are contiguous in tree order, so one `Clip` opens the run and
        // the next node with a different clip closes it. Emitted outside the node's
        // command range so it is never cached with the node and the layer-offset
        // below never touches it.
        if p.clip != cur_clip {
            commands.push(HudCommand::Clip { rect: p.clip });
            cur_clip = p.clip;
        }
        let st = resolve_style(p.node, styles, model, &results);
        // Does this node's draw read the pointer/focus at all? Only a Lua component
        // does (it receives `hot`/`focused`/`mx`/`my` — which keeps a ported
        // pointer-live kind like context_menu hover-fresh and a text_field's
        // ring/caret focus-fresh); no remaining Rust draw arm consults the cursor.
        let hot_matters = lib.is_some_and(|l| l.handles(&p.node.component));
        // Fast path — an entry whose every input is unchanged replays verbatim. The
        // fingerprint is folded against the entry's OWN read-key list, borrowed in
        // place, so a still frame allocates nothing and enters Lua zero times.
        let replay = match cache.entries.get(&p.key) {
            Some(e) => {
                node_fingerprint(
                    p, st, styles, &e.read_keys, model, &results, input, state, hot_matters,
                ) == e.fingerprint
            }
            None => false,
        };
        if replay {
            let e = cache.entries.get_mut(&p.key).expect("probed above");
            commands.extend_from_slice(&e.commands);
            e.touched = frame;
            continue;
        }
        // Miss: draw for real and rebuild the entry. The read-key list is recomputed
        // here rather than reused because a tree rebuilt from scratch each frame (the
        // Loomforge bench, the chat panel) may hand this identity different props.
        let read_keys = read_keys_of(p.node);
        let fp =
            node_fingerprint(p, st, styles, &read_keys, model, &results, input, state, hot_matters);
        let start = commands.len();
        let (crossed, props) = draw_node(p, model, &results, styles, input, state, lib, &mut commands);
        // Lift this node's commands onto its sub-layer. Within one layer the 2D
        // pipelines draw ui-panels before sprites before text, so without this a
        // sprite (the Muse) would cover a same-layer panel (the popup); a higher
        // `layer` on the popup subtree keeps it on top. Done BEFORE caching, so a
        // replay needs no post-processing.
        if p.layer != 0.0 {
            for c in &mut commands[start..] {
                offset_layer(c, p.layer);
            }
        }
        stats.redraw_nodes += 1;
        stats.lua_draws += crossed as u32;
        cache.entries.insert(
            p.key,
            CacheEntry {
                fingerprint: fp,
                commands: commands[start..].to_vec(),
                read_keys,
                props,
                touched: frame,
            },
        );
    }
    stats.lua_hits = lua_hits;
    // Evict what this frame did not touch, but only once the map has grown well past
    // the live tree — a screen that toggles between two panels should keep both cached,
    // while a tree that structurally churns must not leak.
    if cache.entries.len() > 2 * placed.len().max(16) {
        cache.entries.retain(|_, e| frame.wrapping_sub(e.touched) < 120);
        // The hit memo keys the same node identities; prune it alongside so a
        // structurally churning tree can't leak stale claims either.
        state.hit_memo.retain(|k, _| cache.entries.contains_key(k));
    }
    state.cache = cache;
    // Restore the full frame after a trailing clipped run so nothing downstream inherits it.
    if cur_clip.is_some() {
        commands.push(HudCommand::Clip { rect: None });
    }

    // Stage pass: `rtt` nodes reserve a PiP slot for the scene's frame graph to
    // fill (the walker cannot — see `RttSlot`). Their backdrop panel was already
    // drawn above by the normal styled-box path, so only the INSET image rect
    // travels here.
    let mut rtts = Vec::new();
    for p in &placed {
        if p.node.component != "rtt" {
            continue;
        }
        let Some(source) = ptext(p.node, "source") else {
            tracing::warn!("rtt node {:?} has no `source` prop — slot skipped", p.node.id);
            continue;
        };
        let st = style_of(p.node, styles);
        // `inset` may ride as a node prop or sit in the shared panel style, so a
        // whole family of stages can share one inset without repeating it.
        let inset = pnum(p.node, "inset")
            .map(|n| n as f32)
            .unwrap_or_else(|| jnum(st, "inset", 0.0));
        let img = p.rect.inset(inset);
        // Liveness: an explicit Model bind wins, then a literal `live` prop, else
        // live (the single-stage case should just work).
        let live = match ptext(p.node, "live_bind") {
            Some(key) => eff_bool(&results, model, key),
            None if p.node.props.contains_key("live") => pbool(p.node, "live"),
            None => true,
        };
        let tint = match ptext(p.node, "tint") {
            Some(path) => json_color(jpath(styles, path), [1.0; 4]),
            None => first_color(st, &["tint"], [1.0; 4]),
        };
        rtts.push(RttSlot {
            id: p.node.id.clone(),
            source: source.to_string(),
            x: img.x,
            y: img.y,
            w: img.w,
            h: img.h,
            layer: p.layer,
            live,
            tint,
        });
    }

    UiFrame { commands, results, rtts, stats }
}

// ── Layout ───────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn resolve<'a>(
    node: &'a UiNode,
    rect: Rect,
    model: &ValueMap,
    layer: f32,
    clip: Option<[f32; 4]>,
    key: u64,
    out: &mut Vec<Placed<'a>>,
) {
    if !visible(node, model) {
        return;
    }
    // A node's optional `layer` prop accumulates down the tree, so a whole
    // subtree (a styled popup + its buttons + labels) can sit above a backdrop.
    let layer = layer + pnum(node, "layer").map(|n| n as f32).unwrap_or(0.0);
    out.push(Placed { node, rect, enabled: enabled(node, model), layer, clip, key });
    if node.children.is_empty() || no_descend(&node.component) {
        return;
    }
    let inner = rect.inset_xy(pad_x(node), pad_y(node));
    match node.component.as_str() {
        // A `list` (scrolling region): children flow as a column shifted up by the
        // bound offset, and the whole subtree is clipped to the viewport (`inner`).
        // Content taller than the viewport scrolls. This LAYOUT is a structural
        // primitive and stays Rust; the region's draw (backdrop + scrollbar) and hit
        // (claim + wheel→offset) live in `ui/list.lua` like any other component.
        "list" => {
            let content_h = scroll_content_h(node, model);
            let max = (content_h - inner.h).max(0.0);
            let offset = node
                .bind
                .as_deref()
                .and_then(|b| model.number(b))
                .unwrap_or(0.0)
                .clamp(0.0, max as f64) as f32;
            // Reserve a right gutter for the scrollbar so content lays out (and clips)
            // to the LEFT of it — otherwise a right-aligned control underlaps the bar and
            // its edge gets shaved by the viewport clip.
            let gutter = pnum(node, "gutter").map(|n| n as f32).unwrap_or(16.0);
            let view_w = (inner.w - gutter).max(0.0);
            let content = Rect { x: inner.x, y: inner.y - offset, w: view_w, h: content_h };
            let view = Some([inner.x, inner.y, view_w, inner.h]);
            flow(node, content, model, layer, view, key, out, false);
        }
        "row" => flow(node, inner, model, layer, clip, key, out, true),
        // `cell` is the generic layout BOX (a "div") — same vertical-flow engine as
        // `cell` is THE box — one vertical-flow engine, one name. (It absorbed `column`
        // and `panel`: a vertical list is a `cell`, and a carved-stone panel is a `cell`
        // carrying a `style`. `row` remains only because its axis genuinely differs.)
        "cell" => flow(node, inner, model, layer, clip, key, out, false),
        // A 2-D track grid — the CSS-Grid generalisation of `flow` (see the Grid
        // section). Must sit before the `_` catch-all so its children are placed
        // into cells rather than anchor-overlaid.
        "grid" => grid_arrange(node, inner, model, layer, clip, key, out),
        // page / stack / anything else: overlay children, each placed by its own anchor.
        _ => {
            // Index over ALL children (not just the visible ones) so a sibling toggling
            // its visibility never renumbers — and therefore never re-keys — the rest.
            for (i, c) in node.children.iter().enumerate() {
                if !visible(c, model) {
                    continue;
                }
                let r = anchored(c, inner, model);
                resolve(c, r, model, layer, clip, child_key(key, c, i), out);
            }
        }
    }
}

/// A child's cache identity: its own `id` when it has one (stable wherever the node
/// moves), else its structural path — the parent's key folded with the child's kind
/// and its index among ALL siblings.
fn child_key(parent: u64, child: &UiNode, index: usize) -> u64 {
    let mut h = Fnv::seed(parent);
    if child.id.is_empty() {
        h.str(&child.component);
        h.u64(index as u64);
    } else {
        // An id is unique within the tree by contract, so it alone identifies the node —
        // seeded from a constant, not the parent, so re-parenting keeps the entry.
        h = Fnv::new();
        h.str(&child.id);
    }
    h.finish()
}

/// Flow children along the main axis (row = x, column = y), filling the cross
/// axis. Fixed `size` children take their length; `grow` children share the rest
/// by weight. Ported from the exploratory `ui/layout.lua` resolver.
#[allow(clippy::too_many_arguments)]
fn flow<'a>(
    node: &'a UiNode,
    area: Rect,
    model: &ValueMap,
    layer: f32,
    clip: Option<[f32; 4]>,
    key: u64,
    out: &mut Vec<Placed<'a>>,
    horizontal: bool,
) {
    // Carry each visible child's index among ALL siblings, so its cache key is stable
    // when a sibling above it hides.
    let kids: Vec<(usize, &UiNode)> =
        node.children.iter().enumerate().filter(|(_, c)| visible(c, model)).collect();
    let n = kids.len();
    let main = if horizontal { area.w } else { area.h };

    let mut fixed = 0.0;
    let mut grow_total = 0.0;
    for (_, c) in &kids {
        match c.grow {
            Some(g) => grow_total += g,
            None => fixed += child_main(c, model, horizontal),
        }
    }
    let free = main - fixed - node.gap * n.saturating_sub(1) as f32;

    // Cross-axis alignment of children (a CONTAINER prop): `stretch` (default — fill the
    // cross extent, the historical behaviour) or `start`/`center`/`end` (size the child to
    // its intrinsic cross extent and pin it). Main-axis distribution stays the job of a
    // `grow` spacer — one tool per axis. This never collides with a `text` leaf's own
    // `align` (left/center/right), which is read at DRAW; `flow` only reads it on the parent.
    let align = ptext(node, "align");
    let mut pos = if horizontal { area.x } else { area.y };
    for (i, c) in &kids {
        let len = match c.grow {
            Some(g) if grow_total > 0.0 => free * g / grow_total,
            Some(_) => 0.0,
            None => child_main(c, model, horizontal),
        };
        // Cross extent: full (stretch) or the child's measured cross size, pinned by `align`.
        let cross_full = if horizontal { area.h } else { area.w };
        let (cross_len, cross_off) = match align {
            Some(a) if a != "stretch" => {
                let m = measure(c, model);
                let cl = (if horizontal { m.y } else { m.x }).min(cross_full);
                let off = match a {
                    "center" => (cross_full - cl) * 0.5,
                    "end" => cross_full - cl,
                    _ => 0.0, // "start" (and any unknown token → start, never a panic)
                };
                (cl, off)
            }
            _ => (cross_full, 0.0),
        };
        let r = if horizontal {
            Rect { x: pos, y: area.y + cross_off, w: len, h: cross_len }
        } else {
            Rect { x: area.x + cross_off, y: pos, w: cross_len, h: len }
        };
        resolve(c, r, model, layer, clip, child_key(key, c, *i), out);
        pos += len + node.gap;
    }
}

/// A `list` region's intrinsic content height — its visible children stacked as a
/// column (pad + inter-child gaps + each child's main size). The basis for the max
/// scroll offset (`content_h - viewport_h`): `resolve` lays out and clamps with it,
/// the fingerprint folds it (a row appearing must invalidate the bar), and
/// `component_props` hands it to `ui/list.lua` as `content_h` so the module's
/// scrollbar and wheel clamp can never disagree with the placement.
fn scroll_content_h(node: &UiNode, model: &ValueMap) -> f32 {
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let gaps = node.gap * kids.len().saturating_sub(1) as f32;
    pad_y(node) * 2.0 + gaps + kids.iter().map(|c| child_main(c, model, false)).sum::<f32>()
}

/// Place an absolutely-anchored node's box within `parent` (corner/edge + offset).
/// A `width_frac`/`height_frac` prop sizes the box as a fraction of the parent
/// rect — the flex-style constraint a full-screen backdrop or a viewport-tall Muse
/// needs, so the tree stays built-once and adapts to any window size at layout time.
/// An `aspect` (width÷height) prop instead LOCKS width to the resolved height, so an
/// image keeps its proportions (the square Muse) instead of stretching with the window.
fn anchored(node: &UiNode, parent: Rect, model: &ValueMap) -> Rect {
    let m = measure(node, model);
    let h = node
        .height
        .or_else(|| pnum(node, "height_frac").map(|f| parent.h * f as f32))
        .unwrap_or(m.y);
    let w = match pnum(node, "aspect") {
        Some(aspect) => h * aspect as f32,
        None => node
            .width
            .or_else(|| pnum(node, "width_frac").map(|f| parent.w * f as f32))
            .unwrap_or(m.x),
    };
    let a = node.anchor.unwrap_or(UiAnchor::TopLeft);
    let x = match a {
        UiAnchor::TopLeft | UiAnchor::Left | UiAnchor::BottomLeft => parent.x,
        UiAnchor::Top | UiAnchor::Center | UiAnchor::Bottom => parent.x + (parent.w - w) * 0.5,
        UiAnchor::TopRight | UiAnchor::Right | UiAnchor::BottomRight => parent.x + parent.w - w,
    } + node.offset[0];
    let y = match a {
        UiAnchor::TopLeft | UiAnchor::Top | UiAnchor::TopRight => parent.y,
        UiAnchor::Left | UiAnchor::Center | UiAnchor::Right => parent.y + (parent.h - h) * 0.5,
        UiAnchor::BottomLeft | UiAnchor::Bottom | UiAnchor::BottomRight => parent.y + parent.h - h,
    } + node.offset[1];
    Rect { x, y, w, h }
}

/// A node's intrinsic box — explicit `width`/`height` win; a container measures
/// from its (visible) children; a leaf falls back to `size` (its main-axis len).
fn measure(node: &UiNode, model: &ValueMap) -> Vec2 {
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let gaps = node.gap * kids.len().saturating_sub(1) as f32;
    match node.component.as_str() {
        "row" => {
            let w = node.width.unwrap_or_else(|| {
                pad_x(node) * 2.0 + gaps + kids.iter().map(|c| child_main(c, model, true)).sum::<f32>()
            });
            let h = node.height.unwrap_or_else(|| {
                pad_y(node) * 2.0
                    + kids.iter().map(|c| child_cross(c, model, true)).fold(0.0, f32::max)
            });
            Vec2::new(w, h)
        }
        "cell" => {
            let h = node.height.unwrap_or_else(|| {
                pad_y(node) * 2.0
                    + gaps
                    + kids.iter().map(|c| child_main(c, model, false)).sum::<f32>()
            });
            let w = node.width.unwrap_or_else(|| {
                pad_x(node) * 2.0
                    + kids.iter().map(|c| child_cross(c, model, false)).fold(0.0, f32::max)
            });
            Vec2::new(w, h)
        }
        "stack" => {
            // Overlay container: hug the largest child (so a styled panel sizes to
            // its content column while corner decorations anchor to its edges),
            // unless an explicit width/height/size overrides.
            let cw = kids.iter().map(|c| measure(c, model).x).fold(0.0_f32, f32::max);
            let ch = kids.iter().map(|c| measure(c, model).y).fold(0.0_f32, f32::max);
            Vec2::new(node.width.or(node.size).unwrap_or(cw), node.height.or(node.size).unwrap_or(ch))
        }
        // A content-sized / nested grid must report a real intrinsic box (auto
        // tracks sum from their cells); without this arm it would collapse to the
        // leaf `width.or(size)` and every auto track would break.
        "grid" => grid_measure(node, model),
        // A size-less text row reserves glyph size + leading (text ruling 2026-07-31):
        // the `size = text_size + 10` arithmetic templates used to carry is the ENGINE's
        // default, so template data holds no math. An explicit size/height wins (wrapped
        // multi-line text still authors its own height); width stays explicit/grow —
        // the walker is glyph-free, so it cannot measure a line's width.
        "text" => {
            let ts = pnum(node, "text_size").map(|n| n as f32).unwrap_or(14.0);
            let lead = pnum(node, "leading").map(|n| n as f32).unwrap_or(10.0);
            Vec2::new(
                node.width.or(node.size).unwrap_or(0.0),
                node.height.or(node.size).unwrap_or(ts + lead),
            )
        }
        _ => Vec2::new(
            node.width.or(node.size).unwrap_or(0.0),
            node.height.or(node.size).unwrap_or(0.0),
        ),
    }
}

fn child_main(c: &UiNode, model: &ValueMap, horizontal: bool) -> f32 {
    if let Some(s) = c.size {
        return s;
    }
    let m = measure(c, model);
    if horizontal {
        m.x
    } else {
        m.y
    }
}

fn child_cross(c: &UiNode, model: &ValueMap, horizontal: bool) -> f32 {
    let m = measure(c, model);
    if horizontal {
        m.y
    } else {
        m.x
    }
}

// ── Grid ───────────────────────────────────────────────────────────────────
//
// A 2-D track grid, the CSS-Grid generalisation of `flow`. `cols`/`rows` are each
// a whitespace-separated track-spec string (`"auto 1fr 52"`) parsed here; children
// place into cells by their own `col`/`row`/`col_span`/`row_span` props, else they
// auto-flow row-major into the first free cell. Two passes, exactly like flow: a
// bottom-up `measure` arm sizes auto tracks from content, and a top-down
// `grid_arrange` resolves fixed → auto → fr and hands each child its cell rect.
// Nothing here reads a grid CHILD's `size`/`grow` — in a grid the TRACK owns the
// cell's extent and the child fills it, the same "cell fills, child stretches"
// contract flow uses on its cross axis. This is purely additive; row/column/panel/
// stack/page are untouched.

/// One grid track's sizing function (the CSS-Grid track kinds the walker needs):
/// an exact pixel length, a content-sized `auto`, or an `fr` weight that shares
/// leftover space. Parsed from a `cols`/`rows` track-spec string.
#[derive(Clone, Copy, Debug)]
enum Track {
    Fixed(f32), // "52"   -> exactly 52px
    Auto,       // "auto" -> the max intrinsic size of its single-span cells
    Fr(f32),    // "1fr"  -> a weight in the free-space distribution
}

/// A child's placement: the top-left cell it occupies and how many tracks it spans.
/// Auto-flow assigns the first free cell (row-major) when `col`/`row` are absent;
/// spans are clamped to >= 1 (and columns to the track count — columns never grow).
#[derive(Clone, Copy, Debug)]
struct Placement {
    col: usize,
    row: usize,
    col_span: usize,
    row_span: usize,
}

/// Which grid axis a helper is operating on (keeps the col/row code paths shared).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis {
    Col,
    Row,
}

/// Parse a `cols`/`rows` track-spec (`"auto 1fr 52"`) into a track list. An empty
/// or absent spec degrades to a single `1fr` track so a bare grid is one fill cell
/// (the graceful, panic-free default).
fn parse_tracks(spec: Option<&str>) -> Vec<Track> {
    let tracks: Vec<Track> = spec
        .unwrap_or("")
        .split_whitespace()
        .map(parse_track)
        .collect();
    if tracks.is_empty() {
        vec![Track::Fr(1.0)]
    } else {
        tracks
    }
}

/// Parse one track token. An unparseable token falls back to `Auto` so a typo
/// never drops a track and shifts every later child's column index.
fn parse_track(tok: &str) -> Track {
    if tok.eq_ignore_ascii_case("auto") {
        return Track::Auto;
    }
    if tok.len() >= 2 && tok[tok.len() - 2..].eq_ignore_ascii_case("fr") {
        return match tok[..tok.len() - 2].parse::<f32>() {
            Ok(w) => Track::Fr(w),
            Err(_) => Track::Auto,
        };
    }
    match tok.parse::<f32>() {
        Ok(px) => Track::Fixed(px),
        Err(_) => Track::Auto,
    }
}

/// Round-and-clamp a `col`/`row`/`*_span` prop to a track index/count: `f64::round`
/// then `as usize` (a saturating cast — a negative value becomes 0).
fn place_index(node: &UiNode, key: &str, dflt: usize) -> usize {
    match pnum(node, key) {
        Some(n) => n.round() as usize,
        None => dflt,
    }
}

/// A child's placement from its `col`/`row`/`col_span`/`row_span` props, plus
/// whether it is EXPLICIT (carries a `col` or `row`) so auto-flow can resolve the
/// rest afterwards. Spans clamp to >= 1; `col_span` also clamps to the track count
/// (columns never grow implicitly).
fn child_placement(c: &UiNode, n_cols: usize) -> (Placement, bool) {
    let explicit = c.props.contains_key("col") || c.props.contains_key("row");
    let col = place_index(c, "col", 0);
    let row = place_index(c, "row", 0);
    let col_span = place_index(c, "col_span", 1).max(1).min(n_cols.max(1));
    let row_span = place_index(c, "row_span", 1).max(1);
    (Placement { col, row, col_span, row_span }, explicit)
}

/// Grow a `Vec<bool>` occupancy grid so it covers at least `rows` rows of `n_cols`.
fn ensure_rows(occ: &mut Vec<bool>, n_cols: usize, rows: usize) {
    let want = rows * n_cols;
    if occ.len() < want {
        occ.resize(want, false);
    }
}

/// Resolve every visible child's final [`Placement`]: EXPLICIT children first (they
/// may overlap — CSS allows it, and it is what makes the stack case expressible),
/// then row-major auto-flow into the first free cell, extending rows implicitly.
/// The returned row count may exceed `rows.len()`. Occupancy is a growable
/// `Vec<bool>` indexed `row * n_cols + col`.
fn place_children(kids: &[&UiNode], n_cols: usize) -> Vec<Placement> {
    let n_cols = n_cols.max(1);
    let mut places: Vec<Option<Placement>> = vec![None; kids.len()];
    let mut occ: Vec<bool> = Vec::new();

    // Pass A — explicit placement. Clamp `col` so `col + col_span <= n_cols`
    // (columns never grow); rows are left free to grow. Mark the spanned block
    // occupied so auto-flow steps around it. Explicit children MAY overlap each
    // other (occupancy is only consulted by auto-flow), which is the stack case.
    for (i, c) in kids.iter().enumerate() {
        let (mut p, explicit) = child_placement(c, n_cols);
        if !explicit {
            continue;
        }
        p.col = p.col.min(n_cols - p.col_span);
        ensure_rows(&mut occ, n_cols, p.row + p.row_span);
        for r in p.row..p.row + p.row_span {
            for cc in p.col..p.col + p.col_span {
                occ[r * n_cols + cc] = true;
            }
        }
        places[i] = Some(p);
    }

    // Pass B — sparse auto-flow. A row-major cursor advances to the first cell
    // whose `col_span × row_span` block is entirely free, growing implicit rows as
    // needed. `col_span` is already clamped to `n_cols`, so a block always fits some
    // column.
    let mut cursor = 0usize; // linear index: row * n_cols + col
    for (i, c) in kids.iter().enumerate() {
        if places[i].is_some() {
            continue;
        }
        let (p0, _) = child_placement(c, n_cols);
        let (col_span, row_span) = (p0.col_span, p0.row_span);
        loop {
            let col = cursor % n_cols;
            let row = cursor / n_cols;
            if col + col_span > n_cols {
                // The span would overhang the right edge — skip to the next row.
                cursor = (row + 1) * n_cols;
                continue;
            }
            ensure_rows(&mut occ, n_cols, row + row_span);
            let mut fits = true;
            'block: for r in row..row + row_span {
                for cc in col..col + col_span {
                    if occ[r * n_cols + cc] {
                        fits = false;
                        break 'block;
                    }
                }
            }
            if fits {
                for r in row..row + row_span {
                    for cc in col..col + col_span {
                        occ[r * n_cols + cc] = true;
                    }
                }
                places[i] = Some(Placement { col, row, col_span, row_span });
                cursor = row * n_cols + col + col_span;
                break;
            }
            cursor += 1;
        }
    }

    // Every slot was filled (Pass A explicit or Pass B auto-flow).
    places.into_iter().map(|p| p.expect("every child placed")).collect()
}

/// Clone `rows_spec` and pad it to `n_rows` with the node's `auto_rows` sizing
/// (default `auto`), so implicit rows the auto-flow appended have a track.
fn extend_rows(rows_spec: &[Track], n_rows: usize, node: &UiNode) -> Vec<Track> {
    let mut rows = rows_spec.to_vec();
    let fill = parse_track(ptext(node, "auto_rows").unwrap_or("auto"));
    while rows.len() < n_rows {
        rows.push(fill);
    }
    rows
}

/// The intrinsic (content) size of one track on `axis`: `Fixed` short-circuits to
/// its px; `Auto`/`Fr` return the max over their SINGLE-SPAN cells of the child's
/// `measure()` on that axis (`.x` for a column, `.y` for a row). A track no cell
/// lands in measures 0. Multi-span children are excluded (the CSS "distribute extra
/// to spanned tracks" step is a deliberate Phase-2 extension).
fn track_intrinsic(
    tracks: &[Track],
    i: usize,
    kids: &[&UiNode],
    places: &[Placement],
    axis: Axis,
    model: &ValueMap,
) -> f32 {
    if let Track::Fixed(px) = tracks[i] {
        return px;
    }
    let mut m = 0.0_f32;
    for (k, p) in kids.iter().zip(places.iter()) {
        let (start, span) = match axis {
            Axis::Col => (p.col, p.col_span),
            Axis::Row => (p.row, p.row_span),
        };
        if span == 1 && start == i {
            let val = match axis {
                Axis::Col => measure(k, model).x,
                Axis::Row => measure(k, model).y,
            };
            m = m.max(val);
        }
    }
    m
}

/// One axis's USED track sizes for the arrange pass — a direct 2-D lift of flow's
/// fixed-vs-grow split: `Fixed` → px, `Auto` → its content intrinsic, `Fr` →
/// `free * w / fr_total`. `free` is deliberately NOT clamped to `>= 0`, so an
/// over-constrained grid yields negative fr cells exactly as flow yields negative
/// grow lengths — the identity that makes row/column parity byte-exact. When no fr
/// track exists the leftover `free` is simply trailing slack (content top/left-
/// packed), matching a flow of all-fixed children.
fn size_tracks(
    tracks: &[Track],
    kids: &[&UiNode],
    places: &[Placement],
    axis: Axis,
    extent: f32,
    gap: f32,
    model: &ValueMap,
) -> Vec<f32> {
    let n = tracks.len();
    let mut sizes = vec![0.0_f32; n];
    let mut fr_total = 0.0_f32;
    let mut base_sum = 0.0_f32;
    for (i, t) in tracks.iter().enumerate() {
        match t {
            Track::Fixed(px) => {
                sizes[i] = *px;
                base_sum += *px;
            }
            Track::Auto => {
                let s = track_intrinsic(tracks, i, kids, places, axis, model);
                sizes[i] = s;
                base_sum += s;
            }
            Track::Fr(w) => fr_total += *w, // sizes[i] stays 0 until the fr split
        }
    }
    let used = base_sum + gap * n.saturating_sub(1) as f32;
    let free = extent - used; // NOT clamped — mirrors flow's unclamped grow share
    if fr_total > 0.0 {
        for (i, t) in tracks.iter().enumerate() {
            if let Track::Fr(w) = t {
                sizes[i] = free * w / fr_total;
            }
        }
    }
    sizes
}

/// Prefix offsets of a sized track list from `origin`, inserting `gap` between
/// tracks: `o[0] = origin`, `o[i] = o[i-1] + sizes[i-1] + gap`.
fn track_offsets(origin: f32, sizes: &[f32], gap: f32) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut cur = origin;
    for s in sizes {
        offsets.push(cur);
        cur += s + gap;
    }
    offsets
}

/// The pixel extent a span covers: the sum of its tracks plus the interior gaps.
fn span_extent(sizes: &[f32], start: usize, span: usize, gap: f32) -> f32 {
    let end = (start + span).min(sizes.len());
    let sum: f32 = sizes[start..end].iter().sum();
    sum + gap * span.saturating_sub(1) as f32
}

/// Place children into a 2-D track grid: size each axis's tracks (fixed, then
/// content-sized auto, then fr sharing the remainder), then give each child its
/// span-aware cell rect and recurse into `resolve`. The 2-D generalisation of
/// `flow` — it threads `layer`/`clip` through unchanged (preserving painter order
/// and scroll-clip contiguity), and the child FILLS its cell (so a grid child's own
/// size/grow do not re-enter, exactly as flow fills the cross axis).
#[allow(clippy::too_many_arguments)]
fn grid_arrange<'a>(
    node: &'a UiNode,
    area: Rect,
    model: &ValueMap,
    layer: f32,
    clip: Option<[f32; 4]>,
    key: u64,
    out: &mut Vec<Placed<'a>>,
) {
    let cols = parse_tracks(ptext(node, "cols"));
    let rows_spec = parse_tracks(ptext(node, "rows"));
    // Sibling indices ride along for cache keying (see `child_key`).
    let idx: Vec<usize> =
        node.children.iter().enumerate().filter(|(_, c)| visible(c, model)).map(|(i, _)| i).collect();
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let places = place_children(&kids, cols.len());
    // Implicit rows: auto-flow may reference rows past the spec — extend with
    // `auto_rows` so every placed row has a track to size and offset.
    let n_rows = places.iter().map(|p| p.row + p.row_span).max().unwrap_or(0).max(rows_spec.len());
    let rows = extend_rows(&rows_spec, n_rows, node);
    let col_gap = pnum(node, "col_gap").map(|n| n as f32).unwrap_or(node.gap);
    let row_gap = pnum(node, "row_gap").map(|n| n as f32).unwrap_or(node.gap);

    let cw = size_tracks(&cols, &kids, &places, Axis::Col, area.w, col_gap, model);
    let rh = size_tracks(&rows, &kids, &places, Axis::Row, area.h, row_gap, model);
    let col_x = track_offsets(area.x, &cw, col_gap);
    let row_y = track_offsets(area.y, &rh, row_gap);

    for ((k, p), i) in kids.iter().zip(places.iter()).zip(idx.iter()) {
        let r = Rect {
            x: col_x[p.col],
            y: row_y[p.row],
            w: span_extent(&cw, p.col, p.col_span, col_gap),
            h: span_extent(&rh, p.row, p.row_span, row_gap),
        };
        resolve(k, r, model, layer, clip, child_key(key, k, *i), out); // the child fills its cell
    }
}

/// A grid's intrinsic box — the `measure` counterpart of [`grid_arrange`]. Fixed
/// tracks contribute their px; auto AND fr tracks contribute their content
/// intrinsic (fr's weight is an arrange-time distribution rule, not an intrinsic
/// size — so for measuring the outer box an fr track reports its content, exactly
/// as `measure("row")` sums `child_main` over its grow children too). Explicit
/// width/height still win.
fn grid_measure(node: &UiNode, model: &ValueMap) -> Vec2 {
    let cols = parse_tracks(ptext(node, "cols"));
    let rows_spec = parse_tracks(ptext(node, "rows"));
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let places = place_children(&kids, cols.len());
    let n_rows = places.iter().map(|p| p.row + p.row_span).max().unwrap_or(0).max(rows_spec.len());
    let rows = extend_rows(&rows_spec, n_rows, node);
    let col_gap = pnum(node, "col_gap").map(|n| n as f32).unwrap_or(node.gap);
    let row_gap = pnum(node, "row_gap").map(|n| n as f32).unwrap_or(node.gap);

    let cols_w: f32 = (0..cols.len())
        .map(|i| track_intrinsic(&cols, i, &kids, &places, Axis::Col, model))
        .sum();
    let rows_h: f32 = (0..rows.len())
        .map(|i| track_intrinsic(&rows, i, &kids, &places, Axis::Row, model))
        .sum();

    let w = node
        .width
        .unwrap_or_else(|| pad_x(node) * 2.0 + col_gap * cols.len().saturating_sub(1) as f32 + cols_w);
    let h = node
        .height
        .unwrap_or_else(|| pad_y(node) * 2.0 + row_gap * rows.len().saturating_sub(1) as f32 + rows_h);
    Vec2::new(w, h)
}

/// The kinds whose children are DATA, not placed nodes: a segmented control lays its
/// own segments out inside its single rect. Their children therefore never appear in
/// `placed`, so their props ride along in the parent's `component_props` — and the
/// parent's [`DrawCache`] fingerprint must fold them in (nothing else would notice a
/// changed segment label). The same class gets the click-edge candidate escape in
/// [`dispatch_component_hit`]: rows a module lays past the node rect (a context
/// menu's overflow) stay clickable.
fn no_descend(kind: &str) -> bool {
    matches!(kind, "tabs" | "pill_toggle" | "select" | "context_menu")
}

fn visible(node: &UiNode, model: &ValueMap) -> bool {
    match &node.visible_bind {
        Some(k) => model.is_on(k),
        None => true,
    }
}

fn enabled(node: &UiNode, model: &ValueMap) -> bool {
    match &node.enabled_bind {
        Some(k) => model.is_on(k),
        None => true,
    }
}

// ── Hit-test ─────────────────────────────────────────────────────────────────

/// How far outside a node's rect the pointer still makes it a hit-dispatch
/// CANDIDATE. The candidate set errs generous (a few pixels of slop cost one cheap
/// Lua call on an input frame); the component's `M.hit` owns the tight verdict.
const HIT_SLOP: f32 = 8.0;

/// A node's retained-interaction identity — its `id`, else its `bind`, else `""`.
/// One rule for pointer capture (`state.dragging`), the open popup (`state.open`),
/// and the hit dispatch's candidate test.
fn node_ident(node: &UiNode) -> &str {
    if !node.id.is_empty() {
        &node.id
    } else {
        node.bind.as_deref().unwrap_or("")
    }
}

#[allow(clippy::too_many_arguments)]
fn hit_node(
    p: &Placed,
    model: &ValueMap,
    input: &UiInput,
    state: &mut UiState,
    styles: &Json,
    results: &mut ValueMap,
    hud_hit: &mut bool,
    lib: Option<&dyn ComponentLibrary>,
    input_active: bool,
    lua_hits: &mut u32,
) {
    let node = p.node;
    let r = p.rect;

    // Drag source — prop-driven so ANY row/cell/panel can be one (no new component
    // kind). Pressing inside a node carrying `drag_kind` picks up a payload; `run_ui`
    // reports it, and the scene-owned canvas decides what the drop means.
    if input.clicked && p.enabled && state.drag.is_none() && r.contains(input.mouse) {
        if let Some(kind) = ptext(node, "drag_kind") {
            let id = ptext(node, "drag_id").unwrap_or(node.id.as_str()).to_string();
            state.drag = Some(DragPayload { kind: kind.to_string(), id });
            *hud_hit = true;
        }
    }

    match node.component.as_str() {
        // `checkbox` / `toggle` / `radio` hit logic lives in `ui/<kind>.lua` (the
        // dispatch arm below): tight box/pill/circle regions; idle echo is `echo_binds`.
        // `button` / `tile` declare `hit_shape = "rect"` there instead — the dispatch
        // arm answers them in Rust (hover claims; click fires action / toggles bind).
        // `list` hit logic (rect claim + wheel→offset, clamped to the walker-measured
        // `content_h` prop) lives in `ui/list.lua` via the dispatch arm below; the
        // wheel tick crosses as a live patched prop, and a wheel frame counts as
        // input-active so a parked pointer still scrolls. Its idle echo is
        // `echo_binds` (number, default 0).
        // `slider` hit logic (row claim + group focus, grab-band capture, drag-maps-
        // value) lives in `ui/slider.lua` via the dispatch arm below; capture release
        // and the focus/value echoes are the walker's generic rules. `stepper` (row
        // claim + −/+ end-cell stepping) and `pill_toggle` (well claim + segment-cell
        // pick) likewise live in their `ui/<kind>.lua` modules.
        // `tabs` hit logic (strip claim + tab-cell pick) lives in `ui/tabs.lua` via
        // the dispatch arm below, and `select`'s (field claim + open/close + option
        // pick, with the popup BELOW the node rect) in `ui/select.lua` — the dispatch
        // candidate set includes the open popup's owner, so those off-rect rows still
        // receive their hits. Both idle echoes are `echo_binds`.
        // `text_field` hit logic (well claim + a click inside taking keyboard focus
        // through the verdict's `focus` field) lives in `ui/text_field.lua` via the
        // dispatch arm below; the KEYBOARD stays walker-generic — `fold_typed` folds
        // typed/backspace into the focused node's bind (never a Lua crossing) and
        // `echo_binds` reports the value.
        // `badge` hit logic (claim = its pill, possibly style-inset) lives in
        // `ui/badge.lua`, and `context_menu`'s (menu-rect claim + child-action row
        // pick via the verdict's `activate_child`; rows laid past the authored rect
        // stay clickable through the click-edge candidate escape) in
        // `ui/context_menu.lua` — both via the dispatch arm below.
        // The ONE dispatch arm for every Lua-owned component kind — S7 (`scroll` →
        // `list`) retired the last per-control Rust arm, so only the generic
        // plumbing remains: drag-source above, this dispatch, and the styled-
        // container claim below. A module-declared trivial shape is answered here in
        // Rust — zero Lua crossings; everything else asks the module's `M.hit` for a
        // verdict, bounded to input-active frames × candidate nodes.
        k if lib.is_some_and(|l| l.handles(k)) => {
            let lib = lib.expect("guarded by is_some_and");
            match lib.hit_shape(k) {
                // Presentational (sprite / tooltip / rune_corners): never claims,
                // never interacts — exactly the old fall-through-to-nothing.
                Some(HitShape::None) => {}
                // Full-rect control (button / tile): hover claims; a click inside
                // fires the node's `action` and/or toggles its bool `bind`.
                Some(HitShape::Rect) => {
                    if r.contains(input.mouse) {
                        *hud_hit = true;
                        if input.clicked && p.enabled {
                            if let Some(action) = &node.action {
                                results.set(action.clone(), true);
                            }
                            if let Some(bind) = &node.bind {
                                let val = !eff_bool(results, model, bind);
                                results.set(bind.clone(), val);
                            }
                        }
                    }
                }
                None => dispatch_component_hit(
                    lib, p, model, input, state, styles, results, hud_hit, input_active, lua_hits,
                ),
            }
        }
        // A styled container (a panel) claims the pointer, so a click on the
        // panel background doesn't pick through to the scene. An `rtt` claims it
        // too: the PiP image is UI surface, not a hole through to the world.
        "row" | "cell" | "stack" | "screen" | "rtt" | "grid"
            if has_style(node, styles) && r.contains(input.mouse) =>
        {
            *hud_hit = true;
        }
        _ => {}
    }
}

/// Dispatch one component's `M.hit` — the bounded way.
///
/// * **Idle frame** (`!input_active`): no Lua; replay the node's memoized `hit` so
///   `hud_hit` stays continuous while the pointer rests on a control.
/// * **Active frame, non-candidate**: the pointer is beyond the rect + slop and the
///   node neither owns the open popup nor holds a capture — its claim is cleared
///   without a crossing (every tight region except the open popup lies within the
///   rect, and the popup's owner IS a candidate).
/// * **Active frame, candidate**: cross into Lua with the cached draw props (live
///   fields patched) and apply the verdict generically.
///
/// The candidate set deliberately includes the `state.open` owner (a `select`'s
/// popup lies BELOW its node rect), every captured node (a slider drag keeps
/// reporting while the pointer is off the track), and — on the click edge only —
/// every children-as-data ([`no_descend`]) control: such a control lays its own
/// rows out and may lay them PAST its authored rect (a context menu with more
/// items than its height covers), geometry the walker cannot see, so its module's
/// row math gets the final say on clicks. Click edges are rare, so move/idle
/// frames stay rect-bounded and the still-frame zero-crossing guarantee holds.
#[allow(clippy::too_many_arguments)]
fn dispatch_component_hit(
    lib: &dyn ComponentLibrary,
    p: &Placed,
    model: &ValueMap,
    input: &UiInput,
    state: &mut UiState,
    styles: &Json,
    results: &mut ValueMap,
    hud_hit: &mut bool,
    input_active: bool,
    lua_hits: &mut u32,
) {
    let node = p.node;
    let r = p.rect;
    if !input_active {
        if state.hit_memo.get(&p.key).copied().unwrap_or(false) {
            *hud_hit = true;
        }
        return;
    }
    let m = input.mouse;
    let near = m.x >= r.x - HIT_SLOP
        && m.x <= r.x + r.w + HIT_SLOP
        && m.y >= r.y - HIT_SLOP
        && m.y <= r.y + r.h + HIT_SLOP;
    let ident = node_ident(node);
    let candidate = near
        || state.open.as_deref() == Some(ident)
        || state.dragging.contains(ident)
        || (input.clicked && no_descend(&node.component));
    if !candidate {
        state.hit_memo.remove(&p.key);
        return;
    }
    let props = component_hit_props(p, model, results, input, state, styles);
    // The click edge crosses pre-gated on the node's enabled state, so a disabled
    // control hovers (claims) but never acts — the rule every Rust arm applied.
    let click = input.clicked && p.enabled;
    *lua_hits += 1;
    match lib.hit_component(
        &node.component,
        m.x,
        m.y,
        [r.x, r.y, r.w, r.h],
        &props,
        click,
        input.down,
    ) {
        Ok(verdict) => apply_hit_verdict(verdict, p, state, results, hud_hit),
        Err(e) => {
            tracing::warn!("lua component '{}' hit failed ({e})", node.component);
        }
    }
}

/// The props for a component HIT call: the draw cache's stored map when the node has
/// drawn before (the common case — geometry/style props only change when the node
/// does, and the draw pass re-caches them the same frame), else a fresh
/// `component_props` build. Either way the LIVE fields — `bind_value`, `open`,
/// `captured`, `wheel` — are patched to this instant's state, because the verdict
/// computes from them (a checkbox toggles its CURRENT value; a captured slider maps
/// the drag; a `list` folds the wheel tick into its offset). `wheel` is patched here
/// and NEVER enters the cached draw props or the fingerprint — a tick is transient;
/// the bind change it produces is what invalidates the draw.
fn component_hit_props(
    p: &Placed,
    model: &ValueMap,
    results: &ValueMap,
    input: &UiInput,
    state: &UiState,
    styles: &Json,
) -> Json {
    let node = p.node;
    let mut props = match state.cache.entries.get(&p.key).and_then(|e| e.props.clone()) {
        Some(props) => props,
        None => {
            let st = resolve_style(node, styles, model, results);
            component_props(node, st, styles, model, results, input, state, p.rect)
        }
    };
    if let Json::Object(map) = &mut props {
        match node.bind.as_deref().and_then(|b| eff_value(results, model, b)) {
            Some(v) => {
                map.insert("bind_value".to_string(), value_to_json(v));
            }
            None => {
                map.remove("bind_value");
            }
        }
        let ident = node_ident(node);
        map.insert(
            "open".to_string(),
            Json::Bool(!ident.is_empty() && state.open.as_deref() == Some(ident)),
        );
        map.insert("captured".to_string(), Json::Bool(state.dragging.contains(ident)));
        map.insert("wheel".to_string(), serde_json::json!(input.wheel));
    }
    props
}

/// Apply one component's [`HitVerdict`] to the walker-owned channels — the ONLY
/// place a verdict touches state/results, so every control's interaction flows
/// through identical plumbing: `hit`→`hud_hit` (+ the idle-frame memo),
/// `value`→`bind`, `activate`→`action`, `group_focus`→`focus_group`,
/// `capture`→`state.dragging` (release stays the generic button-up rule),
/// `open`→`state.open` (closing only if this node still owns it),
/// `focus`→`state.focus` (claim only — clearing stays `run_ui`'s generic
/// clicked-frame rule — and only for a node with an id, since focus is held BY id).
fn apply_hit_verdict(
    verdict: HitVerdict,
    p: &Placed,
    state: &mut UiState,
    results: &mut ValueMap,
    hud_hit: &mut bool,
) {
    let node = p.node;
    state.hit_memo.insert(p.key, verdict.hit);
    if verdict.hit {
        *hud_hit = true;
    }
    if let (Some(val), Some(bind)) = (verdict.value, node.bind.as_deref()) {
        results.set(bind.to_string(), val);
    }
    if verdict.activate {
        if let Some(action) = &node.action {
            results.set(action.clone(), true);
        }
    }
    // A child-row activation (`activate_child`, 1-based): a children-as-data control
    // (a context menu) receives the dispatch itself and names the picked row; the
    // walker fires that CHILD's `action` — read live off the node, exactly like the
    // node-level `action` above. Absent/empty action, 0, or out-of-range: no-op.
    if let Some(i) = verdict.activate_child {
        if let Some(action) = i
            .checked_sub(1)
            .and_then(|i| node.children.get(i))
            .and_then(|c| c.action.as_deref())
            .filter(|a| !a.is_empty())
        {
            results.set(action.to_string(), true);
        }
    }
    if verdict.group_focus {
        if let (Some(fg), Some(bind)) = (focus_group(node), node.bind.as_deref()) {
            results.set(fg.to_string(), bind.to_string());
        }
    }
    let ident = node_ident(node);
    match verdict.capture {
        Some(true) => {
            state.dragging.insert(ident.to_string());
        }
        Some(false) => {
            state.dragging.remove(ident);
        }
        None => {}
    }
    match verdict.open {
        Some(true) => state.open = Some(ident.to_string()),
        Some(false) if state.open.as_deref() == Some(ident) => state.open = None,
        _ => {}
    }
    if verdict.focus == Some(true) && !node.id.is_empty() {
        state.focus = Some(node.id.clone());
    }
}

/// The generic every-frame **typed-fold** — the KEYBOARD twin of [`echo_binds`]:
/// when a placed, enabled node holds keyboard focus (`state.focus`, matched by id)
/// and carries a `bind`, this frame's committed text appends to the bound string and
/// a backspace edge pops one char (`String::pop` — one CHARACTER, multibyte-safe),
/// the result written into `results` for the engine to apply.
///
/// Keyboard is NOT pointer: this runs unconditionally every frame, in Rust — a
/// typing frame with a parked pointer is not input-active and never enters Lua, yet
/// must still fold; the changed value then re-fingerprints the focused field, so it
/// (alone) redraws. Runs AFTER the hit pass, so a click that just focused the field
/// folds the same frame, and BEFORE `echo_binds`, so the echo never shadows an edit.
fn fold_typed(
    placed: &[Placed],
    model: &ValueMap,
    input: &UiInput,
    state: &UiState,
    results: &mut ValueMap,
) {
    if input.typed.is_empty() && !input.backspace {
        return;
    }
    let Some(focus) = state.focus.as_deref().filter(|id| !id.is_empty()) else { return };
    for p in placed {
        if p.node.id != focus || !p.enabled {
            continue;
        }
        let Some(bind) = p.node.bind.as_deref() else { continue };
        let mut text = eff_text(results, model, bind).unwrap_or("").to_string();
        text.push_str(&input.typed);
        if input.backspace {
            text.pop();
        }
        results.set(bind.to_string(), text);
    }
}

/// The generic every-frame **bind echo** — the load-bearing contract that every
/// placed control with a `bind` reports its effective value each frame (the
/// paperdoll HUD reads the keys unconditionally and a test asserts it). Fills only
/// keys no interaction wrote this frame, with each kind's own absent-value default
/// — exactly the defaults the old per-control Rust arms applied:
///
/// * bool controls (`checkbox`/`toggle`/`tile`) echo `false` when unset,
/// * numeric controls (`slider`/`stepper`) echo their `min`,
/// * `list` echoes its offset with a `0` default (top of the content),
/// * `tabs` defaults to its first child's `value` (a strip always has one active),
/// * text pickers (`radio`/`pill_toggle`/`select`) echo only what the model holds,
/// * `text_field` echoes the model's text with an empty-string default (a field
///   always reports — the focused frame's EDIT comes from [`fold_typed`], which
///   runs first and wins here by having already set the key).
///
/// `context_menu` carries no bind at all (its rows fire child actions through the
/// verdict's `activate_child`). A `focus_group` key echoes alongside, so slider
/// focus survives the pointer leaving the row.
fn echo_binds(placed: &[Placed], model: &ValueMap, results: &mut ValueMap) {
    for p in placed {
        let node = p.node;
        if let (Some(fg), Some(_)) = (focus_group(node), node.bind.as_deref()) {
            if results.get(fg).is_none() {
                if let Some(cur) = model.text(fg) {
                    results.set(fg.to_string(), cur.to_string());
                }
            }
        }
        let Some(bind) = node.bind.as_deref() else { continue };
        if results.get(bind).is_some() {
            continue;
        }
        match node.component.as_str() {
            "checkbox" | "toggle" | "tile" => {
                results.set(bind.to_string(), model.is_on(bind));
            }
            "slider" | "stepper" => {
                let min = slider_range(node).0 as f64;
                results.set(bind.to_string(), model.number(bind).unwrap_or(min));
            }
            "list" => {
                results.set(bind.to_string(), model.number(bind).unwrap_or(0.0));
            }
            "tabs" => {
                let cur = model
                    .text(bind)
                    .map(str::to_string)
                    .or_else(|| {
                        node.children.first().and_then(|c| ptext(c, "value")).map(str::to_string)
                    });
                if let Some(v) = cur {
                    results.set(bind.to_string(), v);
                }
            }
            "radio" | "pill_toggle" | "select" => {
                if let Some(cur) = model.text(bind) {
                    results.set(bind.to_string(), cur.to_string());
                }
            }
            "text_field" => {
                results.set(bind.to_string(), model.text(bind).unwrap_or("").to_string());
            }
            _ => {}
        }
    }
}

// ── Draw cache: what a node reads, and whether it changed ────────────────────

/// The `Model`/results keys one node's draw reads. Two families, both derived from the
/// node itself rather than from a per-kind table: its `bind` (the value it renders),
/// and every prop whose NAME ends in `_bind` — the repo-wide convention for "this prop
/// holds a Model key" (`text_bind`, `color_bind`, `style_bind`, `live_bind`,
/// `rune_bind`/`name_bind`/`meta_bind`) — plus `focus_group`, which names a key the
/// same way without the suffix. A new `*_bind` prop is therefore covered the day it is
/// authored, with no change here.
///
/// `visible_bind`/`enabled_bind` are deliberately absent: visibility decides whether the
/// node is placed at all, and `enabled` is folded into the fingerprint directly.
fn read_keys_of(node: &UiNode) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    if let Some(b) = node.bind.as_deref() {
        keys.push(b.to_string());
    }
    for (k, v) in &node.props {
        let names_a_key = k.ends_with("_bind") || k == "focus_group";
        if let (true, Value::Text(target)) = (names_a_key, v) {
            keys.push(target.clone());
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

/// Fold every input of a node's draw into one number. Equal fingerprint ⇒ the node
/// would emit byte-identical commands, so the cached ones are replayed and the Lua
/// boundary is not crossed.
///
/// Allocation-free by construction: it reads `&str`s, `f32`s and `Value`s in place and
/// never builds the props map — that map (and the marshalling behind it) is the cost
/// this exists to avoid.
#[allow(clippy::too_many_arguments)]
fn node_fingerprint(
    p: &Placed,
    st: &Json,
    styles: &Json,
    read_keys: &[String],
    model: &ValueMap,
    results: &ValueMap,
    input: &UiInput,
    state: &UiState,
    hot_matters: bool,
) -> u64 {
    let node = p.node;
    let r = p.rect;
    let mut h = Fnv::new();

    // Identity + geometry. Commands are cached in absolute coordinates and already
    // lifted onto the node's sub-layer, so any of these moving invalidates them.
    h.str(&node.component);
    h.f32(r.x);
    h.f32(r.y);
    h.f32(r.w);
    h.f32(r.h);
    h.f32(p.layer);
    match p.clip {
        Some(c) => {
            h.u64(1);
            for v in c {
                h.f32(v);
            }
        }
        None => h.u64(0),
    }
    h.bool(p.enabled);

    // The stringtable generation: a (re)load — a language switch — changes what a
    // display string renders as without any prop changing, so every node folds it in
    // (one relaxed atomic read; bumps are rare).
    h.u64(crate::strings::generation() as u64);

    // Layout metrics that cross as props for a component that sub-lays-out its children,
    // plus `size` — a `text` node falls back to it for its font size, so it is a draw
    // input in its own right and not only a layout one.
    h.f32(node.gap);
    h.f32(pad_x(node));
    h.f32(pad_y(node));
    h.f32(node.size.unwrap_or(f32::NAN));

    // A `list` region draws a bar sized from its CONTENT, which lives in children that
    // are placed (and fingerprinted) separately — so a row appearing or resizing inside
    // it would otherwise leave a stale thumb.
    if node.component == "list" {
        h.f32(scroll_content_h(node, model));
    }

    // The node's own scalar props, order-independently (a HashMap has no stable order,
    // so each entry is hashed alone and the results XOR-folded).
    let mut props_fold = 0u64;
    for (k, v) in &node.props {
        let mut e = Fnv::new();
        e.str(k);
        e.value(v);
        props_fold ^= e.finish();
    }
    h.u64(props_fold);

    // Every style block this node draws from. `st` covers `style` and `style_bind`;
    // the rest are the props that name a further block by dotted path (a tile's
    // `style_off`, a tab strip's active/idle pair, a text's `color`, a stage's `tint`,
    // a tooltip's `rune_color`) — plus `color_bind`, where the Model holds the path.
    h.json(st);
    for key in ["style_off", "tab_active", "tab_idle", "color", "tint", "rune_color"] {
        if let Some(path) = ptext(node, key) {
            h.json(jpath(styles, path));
        }
    }
    if let Some(k) = ptext(node, "color_bind") {
        if let Some(path) = eff_text(results, model, k) {
            h.json(jpath(styles, path));
        }
    }

    // Interaction state the draw reads: hover (or keyboard focus, which `component_props`
    // folds into the same `hot` flag) and whether this node is the open popup.
    //
    // Only for a node whose draw actually READS hover, though — a component (which gets
    // it as `hot`) or the pointer/focus-aware arms. A plain styled box draws the
    // same whether or not the cursor is over it, and every such box on the way down to a
    // hovered control contains the cursor too; folding hover in unconditionally would
    // redraw that whole ancestor chain on every mouse move.
    let focused = !node.id.is_empty() && state.focused() == Some(node.id.as_str());
    let hot = hot_matters && (r.contains(input.mouse) || focused);
    h.bool(hot);
    // Keyboard focus as its OWN bit, not only OR-ed into `hot`: with the pointer
    // parked INSIDE the rect `hot` is true either way, so a focus change there
    // (request_focus / clear_focus / Escape) would otherwise replay a stale
    // caret/ring instead of redrawing the field.
    h.bool(hot_matters && focused);
    let ident = if node.id.is_empty() { node.bind.as_deref() } else { Some(node.id.as_str()) };
    let open = ident.is_some() && state.open.as_deref() == ident;
    h.bool(open);

    // Bound values — this frame's edits override the model, exactly as the draw reads them.
    for k in read_keys {
        match eff_value(results, model, k) {
            Some(v) => h.value(v),
            None => h.u64(u64::MAX),
        }
    }

    // A segmented control lays its own segments out, so its children are DATA it draws
    // from rather than nodes placed (and fingerprinted) in their own right.
    if no_descend(&node.component) {
        let mut kids_fold = 0u64;
        for (i, c) in node.children.iter().enumerate() {
            // Each child's props XOR-folded (a HashMap's iteration order differs per
            // INSTANCE, so a rebuilt tree would otherwise fingerprint differently every
            // frame), then folded into the position-sensitive per-child hash.
            let mut props_fold = 0u64;
            for (k, v) in &c.props {
                let mut kv = Fnv::new();
                kv.str(k);
                kv.value(v);
                props_fold ^= kv.finish();
            }
            let mut e = Fnv::new();
            e.u64(i as u64);
            e.u64(props_fold);
            kids_fold ^= e.finish();
        }
        h.u64(kids_fold);
        // Those same controls light a segment under the cursor, so the pointer itself is
        // an input — but only while it can reach them. A `select`'s popup extends BELOW
        // its rect, so `hot` alone would miss it; whole-pixel precision is enough to pick
        // a row or a tab.
        if hot || open {
            h.f32(input.mouse.x.round());
            h.f32(input.mouse.y.round());
        }
    }

    h.finish()
}

// ── Draw ─────────────────────────────────────────────────────────────────────

/// Props that hold DISPLAY text and therefore resolve through the stringtable on the
/// way to a component. Value/bind channels are deliberately absent — bound data and
/// user text (a chat buffer) are never substituted. `pub(crate)` so the
/// [`raw_display_literals`](crate::raw_display_literals) audit walks the SAME list
/// (one vocabulary, never a drifting twin).
pub(crate) const DISPLAY_STR_PROPS: [&str; 10] =
    ["label", "text", "title", "subtitle", "footer", "placeholder", "hint", "name", "meta", "prefix"];

fn display_prop_json(key: &str, v: &Value) -> Json {
    match v {
        Value::Text(s) if DISPLAY_STR_PROPS.contains(&key) => {
            Json::String(crate::strings::resolve(s).into_owned())
        }
        _ => value_to_json(v),
    }
}

/// Assemble the plain-data props a Lua component receives for one node: its resolved
/// style block, its label, and its hover/focus state. The walker owns style resolution
/// and retained interaction state; the component owns how it DRAWS them. Grows per
/// control as more kinds are ported (S2) — the one place the walker↔component prop
/// contract lives.
#[allow(clippy::too_many_arguments)]
fn component_props(
    node: &UiNode,
    st: &Json,
    styles: &Json,
    model: &ValueMap,
    results: &ValueMap,
    input: &UiInput,
    state: &UiState,
    r: Rect,
) -> Json {
    let hovered = r.contains(input.mouse)
        || (!node.id.is_empty() && state.focused() == Some(node.id.as_str()));
    // Start from the node's own scalar props (box / label_x / label_size / value / … —
    // each component reads whichever it needs), then overlay the walker-resolved fields.
    // Display-text props resolve through the stringtable here, so a Lua component only
    // ever sees FINAL text; value/bind channels never resolve (user text is data).
    let mut props = serde_json::Map::new();
    for (k, v) in &node.props {
        props.insert(k.clone(), display_prop_json(k, v));
    }
    props.insert("label".to_string(), Json::String(node_text(node, model, results)));
    props.insert("hot".to_string(), Json::Bool(hovered));
    props.insert("enabled".to_string(), Json::Bool(enabled(node, model)));
    // A component emits at layer 0; the walker offsets the whole node's commands by its
    // accumulated sub-layer afterwards (see run_ui's draw loop), so 0 always overrides
    // the node's own `layer` sub-layer prop here.
    props.insert("layer".to_string(), serde_json::json!(0.0));
    props.insert("style".to_string(), st.clone());
    // Resolve the named alternate-style paths a control may carry (a tile's loaded-vs-
    // empty `style_off`; a tab strip's `tab_active`/`tab_idle`) into their blocks, like
    // `style`, so the component reads a resolved block rather than a path.
    for key in ["style_off", "tab_active", "tab_idle"] {
        if let Some(path) = ptext(node, key) {
            props.insert(key.to_string(), jpath(styles, path).clone());
        }
    }
    // The pointer + the node's layout metrics, for a component that sub-lays-out its
    // children or hovers a sub-region (tabs' per-tab hover, a select's option rows).
    props.insert("mx".to_string(), serde_json::json!(input.mouse.x));
    props.insert("my".to_string(), serde_json::json!(input.mouse.y));
    props.insert("gap".to_string(), serde_json::json!(node.gap));
    props.insert("pad_x".to_string(), serde_json::json!(pad_x(node)));
    props.insert("pad_y".to_string(), serde_json::json!(pad_y(node)));
    // A `list`'s scrollbar and wheel clamp are sized from the walker-measured
    // content height — the same number `resolve` laid the children out with (and the
    // fingerprint already folds), so the module's bar can never disagree with the
    // placement. List-only: no other kind reads it, and it walks the children.
    if node.component == "list" {
        props.insert("content_h".to_string(), serde_json::json!(scroll_content_h(node, model)));
    }
    // Whether THIS node is the currently-open one (a select's popup): its identity is its
    // `id`, else its `bind` (the `node_ident` rule), matched against the retained open id.
    let ident = if node.id.is_empty() { node.bind.as_deref() } else { Some(node.id.as_str()) };
    props.insert("open".to_string(), Json::Bool(ident.is_some() && state.open.as_deref() == ident));
    // "focused": the walker's retained focus, as one prop covering both models — a
    // module never reads walker state directly. Generically it is KEYBOARD focus
    // (this node's id owns `state.focus` — a text_field's ring + caret); a
    // `focus_group` member (a slider row) instead reads the shared group key
    // currently holding this node's `bind`, exactly as before.
    props.insert(
        "focused".to_string(),
        Json::Bool(!node.id.is_empty() && state.focused() == Some(node.id.as_str())),
    );
    if let (Some(fg), Some(bind)) = (ptext(node, "focus_group"), node.bind.as_deref()) {
        props.insert("focused".to_string(), Json::Bool(eff_text(results, model, fg) == Some(bind)));
    }
    // A tooltip's rune/name/meta content: the Model text under `<field>_bind` (this
    // frame's edit, else the model), else the literal `<field>` prop; absent when empty.
    for (field, bind) in [("rune", "rune_bind"), ("name", "name_bind"), ("meta", "meta_bind")] {
        let v = match ptext(node, bind) {
            Some(key) => eff_text(results, model, key),
            None => ptext(node, field),
        };
        match v.filter(|s| !s.is_empty()) {
            Some(s) => {
                props.insert(field.to_string(), Json::String(crate::strings::resolve(s).into_owned()));
            }
            None => {
                props.remove(field);
            }
        }
    }
    // A tooltip's rune colour: a dotted `rune_color` path resolved to rgba.
    if let Some(path) = ptext(node, "rune_color") {
        let c = json_color(jpath(styles, path), RUNE);
        props.insert("rune_color".to_string(), serde_json::json!([c[0], c[1], c[2], c[3]]));
    }
    // Segmented controls (pill_toggle / tabs / select / context_menu) iterate their
    // children's props (each carries a `value` / `label`); pass them as a plain list so
    // the component can lay out and light up its segments.
    if !node.children.is_empty() {
        let kids = node
            .children
            .iter()
            .map(|c| {
                let mut m = serde_json::Map::new();
                for (k, v) in &c.props {
                    m.insert(k.clone(), display_prop_json(k, v));
                }
                // A child's `action` is a STRUCT field (never in `props`), so cross it
                // explicitly: a control that owns its rows (context_menu) sees which are
                // actionable. Firing stays the walker's job — the verdict names the row
                // (`activate_child`) and `apply_hit_verdict` reads the LIVE node.
                if let Some(a) = c.action.as_deref() {
                    m.insert("action".to_string(), Json::String(a.to_string()));
                }
                Json::Object(m)
            })
            .collect();
        props.insert("children".to_string(), Json::Array(kids));
    }
    // The effective bound value (results override model): a checkbox's checked bool, a
    // slider's number, a select/pill's selected text. Named distinctly from a node's own
    // `value` prop (a radio's literal option). Absent → the component defaults it (nil).
    if let Some(bind) = node.bind.as_deref() {
        if let Some(v) = eff_value(results, model, bind) {
            props.insert("bind_value".to_string(), value_to_json(v));
        }
    }
    Json::Object(props)
}

/// Draw one laid-out node. Returns whether the draw CROSSED into the Lua component
/// library — the walker counts those into [`UiStats::lua_draws`], which is how the
/// bounded-dispatch guarantee is asserted in tests — plus the component props it
/// marshalled for a Lua kind, which the walker caches so the HIT dispatch can reuse
/// them instead of rebuilding the map (see [`component_hit_props`]).
#[allow(clippy::too_many_arguments)]
fn draw_node(
    p: &Placed,
    model: &ValueMap,
    results: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &UiState,
    lib: Option<&dyn ComponentLibrary>,
    out: &mut Vec<HudCommand>,
) -> (bool, Option<Json>) {
    let node = p.node;
    let r = p.rect;
    // `style_bind` (a Model key holding a dotted style path) wins over a literal `style`, so a
    // node's fill/border can follow its state — the non-interactive pipeline tabs pick active vs
    // idle this way, one node per tab instead of a stack of visibility-toggled panels.
    let st = resolve_style(node, styles, model, results);
    // Lua component dispatch: a control OWNS its draw in `ui/<kind>.lua` (no Rust
    // twins remain — S4). The walker hands it the resolved rect + plain-data props
    // and renders what it emits. An absent library or a Lua error falls through to
    // the STRUCTURAL arms below, so an interactive kind then draws only its styled
    // box (or nothing) — a visible failure, never a silent second implementation.
    let mut built_props: Option<Json> = None;
    if let Some(lib) = lib {
        if lib.handles(&node.component) {
            let props = component_props(node, st, styles, model, results, input, state, r);
            match lib.draw_component(&node.component, [r.x, r.y, r.w, r.h], &props) {
                Ok(cmds) => {
                    out.extend(cmds);
                    return (true, Some(props));
                }
                Err(e) => tracing::warn!(
                    "lua component '{}' draw failed ({e}); structural draw only",
                    node.component
                ),
            }
            built_props = Some(props);
        }
    }
    match node.component.as_str() {
        // Styled boxes — including `cell` (the generic layout box) and an `rtt`, whose
        // panel IS its PiP backdrop; the scene's frame graph blits the render target over
        // this (see `RttSlot`). All draw a bg ONLY when they carry a style — an unstyled
        // box (a plain unstyled `cell`) is transparent structure.
        "cell" | "row" | "stack" | "screen" | "rtt" | "grid" => {
            if !st.is_null() {
                draw_panel_bg(r, st, out);
            }
        }
        // (`list` — the scrolling region's backdrop + scrollbar — draws via
        // `ui/list.lua` through the dispatch at the top of this fn, like every
        // other component; only its column LAYOUT + viewport clip stay above in
        // `resolve`. `list_lua_draw_is_byte_pinned` holds the bytes.)
        "text" => {
            let text = node_text(node, model, results);
            // Font size: an explicit `text_size` prop, else the node's layout height
            // (a single line is usually its own height), else a default.
            let size = pnum(node, "text_size").map(|n| n as f32).or(node.size).unwrap_or(14.0);
            // Colour: a dotted `color` path into a token-resolved rgba (text's escape
            // hatch, since colours can't ride as scalar props), else the style block.
            // `color_bind` names a Model key holding that same dotted path, so a row whose
            // STATE decides its colour (a conform provenance, a pass/fail check) rides the
            // one two-way name channel instead of needing a node per possible colour.
            let path = match ptext(node, "color_bind") {
                Some(key) => eff_text(results, model, key),
                None => ptext(node, "color"),
            };
            let color = match path {
                Some(p) => json_color(jpath(styles, p), INK),
                None => first_color(st, &["color", "label_color"], INK),
            };
            // Align WITHIN the node's box: centre/right resolve against the rect
            // width (the menu's title centres over the popup), left keeps the edge.
            let align = node_align(node);
            let x = match align {
                TextAlign::Center => r.x + r.w * 0.5,
                TextAlign::Right => r.x + r.w,
                TextAlign::Left => r.x,
            };
            // A node opts into WRAPPING with `wrap = true`: the line then breaks to the node's own
            // laid-out width (`r.w`) instead of running off on one line. The node must reserve enough
            // HEIGHT for the wrapped lines (the geometry `measure` can't know the line count without
            // font metrics), so a wrapped text is authored with an explicit multi-line height.
            let wrap = if pbool(node, "wrap") { Some(r.w) } else { None };
            push_text(out, x, r.y, &text, size, color, align, node_font(node), pbool(node, "italic"), pbool(node, "bold"), pnum(node, "tracking").map(|n| n as f32).unwrap_or(-1.0), wrap);
        }
        // Every interactive control (`button` / `text_field` / `select` / …) DRAWS
        // via the Lua component library (`ui/<kind>.lua`), dispatched at the top of
        // this fn — no per-control Rust draw arms remain, only the structural
        // primitives above.
        _ => {}
    }
    (false, built_props)
}


/// Add a node's accumulated sub-layer onto one of its emitted commands (see the
/// draw loop in [`run_ui`]). Every `HudCommand` carries a `layer`.
fn offset_layer(c: &mut HudCommand, dl: f32) {
    match c {
        HudCommand::Rect { layer, .. }
        | HudCommand::Sprite { layer, .. }
        | HudCommand::Text { layer, .. }
        | HudCommand::TextCaret { layer, .. }
        | HudCommand::Panel { layer, .. } => *layer += dl,
        // A clip toggle carries no layer — it rides submission order, not the sort.
        HudCommand::Clip { .. } => {}
    }
}

// ── Templates ────────────────────────────────────────────────────────────────

fn draw_panel_bg(r: Rect, st: &Json, out: &mut Vec<HudCommand>) {
    // Key-aliasing (same spirit as the button variants): a styled container reads
    // its fill from whichever of these its block carries — `fill_top/bot` (panels),
    // `bg_top/bot` (the menu's gradient backdrop), `overlay` (the pause/confirm dim),
    // or a single `color` (the bronze divider rule).
    let top = first_color(st, &["fill_top", "bg_top", "overlay", "panel_bg", "bg", "fill", "color"], PANEL);
    let bot = first_color(st, &["fill_bot", "bg_bot", "overlay", "panel_bg", "bg", "fill", "color"], top);
    let border = first_color(st, &["panel_border", "border"], [0.0; 4]);
    out.push(HudCommand::Panel {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: top,
        color2: bot,
        // `grad` direction from the style (0 flat · 1 vertical · 2 horizontal),
        // defaulting to vertical when the two stops differ — the horizontal scrim
        // fade over the Muse needs `grad: 2`.
        grad: jnum(st, "grad", if top == bot { 0.0 } else { 1.0 }),
        radius: jnum(st, "radius", 0.0),
        border: if border[3] > 0.0 { jnum(st, "border_w", 1.0) } else { 0.0 },
        border_color: border,
        // `feather` (default 0) lets a styled panel be a soft drop shadow — the
        // menu's popup shadow is just a feathered, offset panel behind the popup.
        feather: jnum(st, "feather", 0.0),
        layer: 0.0,
    });
}






// (`draw_text_field` — the single-line input's Rust draw arm — was ported to
// `ui/text_field.lua` in S6 and deleted; `text_field_lua_draw_is_byte_pinned`
// carries its byte-level regression duty. Mid-string carets (grapheme-cluster
// stepping) remain the recorded follow-up, now the module's.)


// ── Geometry helpers ─────────────────────────────────────────────────────────

/// A pill-toggle's geometry: the rounded **well** (a style-`h`-tall track centred
/// in the node rect) plus one **cell** rect per option child — the inner strip
/// (well inset by the style `pad`) split into equal segments. Draw & hit share it
/// so they agree exactly, mirroring settings.lua's pill/segment draw.
fn slider_range(node: &UiNode) -> (f32, f32) {
    (
        pnum(node, "min").map(|n| n as f32).unwrap_or(0.0),
        pnum(node, "max").map(|n| n as f32).unwrap_or(1.0),
    )
}

fn focus_group(node: &UiNode) -> Option<&str> {
    ptext(node, "focus_group")
}

// ── Value / style / command helpers ──────────────────────────────────────────

fn node_text(node: &UiNode, model: &ValueMap, results: &ValueMap) -> String {
    let prefix = ptext(node, "prefix").unwrap_or("");
    let body = match ptext(node, "text_bind") {
        Some(key) => eff_text(results, model, key).unwrap_or_default(),
        None => ptext(node, "text").or(ptext(node, "label")).unwrap_or_default(),
    };
    // Display strings resolve through the stringtable (`$token` → active locale;
    // sigil-gated, so Model-driven data text passes through untouched).
    format!("{}{}", crate::strings::resolve(prefix), crate::strings::resolve(body))
}


fn node_align(node: &UiNode) -> TextAlign {
    match ptext(node, "align") {
        Some("center") => TextAlign::Center,
        Some("right") => TextAlign::Right,
        _ => TextAlign::Left,
    }
}

fn node_font(node: &UiNode) -> FontRole {
    match ptext(node, "font") {
        Some("display") => FontRole::Display,
        Some("label") => FontRole::Label,
        Some("rune") => FontRole::Rune,
        _ => FontRole::Body,
    }
}

fn has_style(node: &UiNode, styles: &Json) -> bool {
    !style_of(node, styles).is_null()
}

fn style_of<'a>(node: &UiNode, styles: &'a Json) -> &'a Json {
    match ptext(node, "style") {
        Some(path) => jpath(styles, path),
        None => &Json::Null,
    }
}

/// Like [`style_of`], but a node may name a Model key in `style_bind` that HOLDS the dotted
/// style path — so a node's whole styling can follow its STATE (an active vs idle tab) through
/// the one two-way name channel, exactly as a text node's `color_bind` does for its colour. A
/// literal `style` is the fallback when no bind is set, or the bound key is absent this frame.
fn resolve_style<'a>(node: &UiNode, styles: &'a Json, model: &ValueMap, results: &ValueMap) -> &'a Json {
    if let Some(key) = ptext(node, "style_bind") {
        if let Some(path) = eff_text(results, model, key) {
            return jpath(styles, path);
        }
    }
    match ptext(node, "style") {
        Some(path) => jpath(styles, path),
        None => &Json::Null,
    }
}

/// Walk a dotted path (`"paperdoll.fit.slider"`) into the styles tree; missing
/// segment → `Null`.
fn jpath<'a>(root: &'a Json, path: &str) -> &'a Json {
    let mut cur = root;
    for seg in path.split('.') {
        cur = cur.get(seg).unwrap_or(&Json::Null);
    }
    cur
}

fn jnum(v: &Json, key: &str, dflt: f32) -> f32 {
    v.get(key).and_then(|n| n.as_f64()).map(|n| n as f32).unwrap_or(dflt)
}

/// First present rgba among `keys`, else `dflt`.
fn first_color(v: &Json, keys: &[&str], dflt: [f32; 4]) -> [f32; 4] {
    for key in keys {
        if let Some(a) = v.get(key).and_then(|c| c.as_array()) {
            if a.len() >= 4 {
                return std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32);
            }
        }
    }
    dflt
}

// Node-prop readers — thin wrappers over the shared `config` surface (read `node.props`).
fn ptext<'a>(node: &'a UiNode, key: &str) -> Option<&'a str> {
    crate::config::text(&node.props, key)
}

fn pnum(node: &UiNode, key: &str) -> Option<f64> {
    crate::config::num(&node.props, key)
}

fn pbool(node: &UiNode, key: &str) -> bool {
    crate::config::flag(&node.props, key)
}

/// Read a colour that IS a 4-array `Value` (a token-resolved rgba), else `dflt`.
/// Used for a text node's dotted `color` path (`"paperdoll.stats.color"`).
fn json_color(v: &Json, dflt: [f32; 4]) -> [f32; 4] {
    match v.as_array() {
        Some(a) if a.len() >= 4 => std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32),
        _ => dflt,
    }
}

fn eff_bool(results: &ValueMap, model: &ValueMap, key: &str) -> bool {
    match results.get(key) {
        Some(Value::Bool(b)) => *b,
        _ => model.is_on(key),
    }
}

fn eff_text<'a>(results: &'a ValueMap, model: &'a ValueMap, key: &str) -> Option<&'a str> {
    results.text(key).or_else(|| model.text(key))
}

/// The effective value of a bound key — this frame's result edit, else the model — as a
/// plain [`Value`], for [`component_props`] to hand a Lua component (checkbox → bool,
/// slider → number, select → text).
fn eff_value<'a>(results: &'a ValueMap, model: &'a ValueMap, key: &str) -> Option<&'a Value> {
    results.get(key).or_else(|| model.get(key))
}

/// A [`Value`] as its natural JSON scalar, for marshalling a prop to a Lua component.
fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(n) => serde_json::json!(n),
        Value::Text(t) => Json::String(t.clone()),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_text(out: &mut Vec<HudCommand>, x: f32, y: f32, text: &str, size: f32, color: [f32; 4], align: TextAlign, font: FontRole, italic: bool, bold: bool, tracking: f32, wrap: Option<f32>) {
    out.push(HudCommand::Text { x, y, text: text.to_string(), size, color, layer: 0.0, align, font, italic, bold, tracking, wrap });
}

// Neutral fallbacks (only used when a style path is missing — real colour comes
// from the resolved Prism tokens in `ui_elements.json`).
const INK: [f32; 4] = [0.871, 0.847, 0.788, 1.0];
const PANEL: [f32; 4] = [0.078, 0.09, 0.122, 1.0];
const RUNE: [f32; 4] = [0.435, 0.592, 1.0, 1.0];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn node(component: &str) -> UiNode {
        UiNode { component: component.to_string(), ..Default::default() }
    }

    fn prop(mut n: UiNode, k: &str, v: Value) -> UiNode {
        n.props.insert(k.to_string(), v);
        n
    }

    fn styles() -> Json {
        serde_json::json!({
            "cb": { "box": [0.1,0.1,0.1,1.0], "check": [1.0,1.0,1.0,1.0], "border": [0.2,0.2,0.2,1.0], "pad": 3 },
            "btn": { "fill_top": [0.2,0.3,0.5,1.0], "hover_top": [0.3,0.4,0.6,1.0], "label": [1.0,1.0,1.0,1.0], "border": [0.3,0.4,0.6,1.0] }
        })
    }

    // A page with one anchored column: a checkbox (bind "flag") over a button
    // (action "go"). Exercises layout (anchor + flow), hit-test (both kinds),
    // and same-frame value reflection.
    fn tree() -> UiNode {
        let cb = {
            let mut n = node("checkbox");
            n.id = "cb".into();
            n.size = Some(20.0);
            n.bind = Some("flag".into());
            n = prop(n, "box", Value::Number(14.0));
            n = prop(n, "label", Value::Text("F".into()));
            prop(n, "style", Value::Text("cb".into()))
        };
        let btn = {
            let mut n = node("button");
            n.id = "btn".into();
            n.size = Some(24.0);
            n.action = Some("go".into());
            n = prop(n, "label", Value::Text("GO".into()));
            prop(n, "style", Value::Text("btn".into()))
        };
        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [16.0, 16.0];
        col.width = Some(120.0);
        col.children = vec![cb, btn];

        let mut page = node("screen");
        page.children = vec![col];
        page
    }

    fn input_at(x: f32, y: f32, clicked: bool) -> UiInput {
        UiInput { mouse: Vec2::new(x, y), clicked, down: clicked, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 }
    }

    /// A wheel tick at a parked pointer — the input a `list` scrolls on.
    fn input_wheel(x: f32, y: f32, wheel: f32) -> UiInput {
        UiInput { wheel, ..input_at(x, y, false) }
    }

    // ── Draw cache (bounded dispatch) ────────────────────────────────────────
    //
    // The ratified rule is that a node crosses into Lua only when one of its inputs
    // changed. These tests hold the walker to it: `UiStats` reports the crossings, and
    // a counting library double proves the counter and the gate agree rather than both
    // being wrong in the same direction.

    /// A `ComponentLibrary` that tallies real dispatches and forwards to a live one.
    struct CountingLib {
        inner: flicker_script::ScriptHost,
        draws: std::cell::Cell<u32>,
        hits: std::cell::Cell<u32>,
    }

    impl ComponentLibrary for CountingLib {
        fn handles(&self, kind: &str) -> bool {
            self.inner.handles(kind)
        }
        fn hit_shape(&self, kind: &str) -> Option<HitShape> {
            self.inner.hit_shape(kind)
        }
        fn draw_component(
            &self,
            kind: &str,
            rect: [f32; 4],
            props: &Json,
        ) -> Result<Vec<HudCommand>, flicker_script::ScriptError> {
            self.draws.set(self.draws.get() + 1);
            self.inner.draw_component(kind, rect, props)
        }
        fn hit_component(
            &self,
            kind: &str,
            mx: f32,
            my: f32,
            rect: [f32; 4],
            props: &Json,
            click: bool,
            down: bool,
        ) -> Result<HitVerdict, flicker_script::ScriptError> {
            self.hits.set(self.hits.get() + 1);
            self.inner.hit_component(kind, mx, my, rect, props, click, down)
        }
    }

    fn counting_lib() -> CountingLib {
        CountingLib {
            inner: flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
                .expect("component library"),
            draws: std::cell::Cell::new(0),
            hits: std::cell::Cell::new(0),
        }
    }

    #[test]
    fn a_sizeless_text_row_reserves_glyph_size_plus_leading() {
        // Text ruling 2026-07-31: the row-height arithmetic templates used to carry
        // (`size = text_size + 10`) is the ENGINE's default for a size-less text node —
        // 22px glyphs reserve a 32px row, the 14px default reserves 24. An explicit
        // `size` still wins (every pre-existing node sets one, so nothing regresses).
        let t1 = {
            let n = node("text");
            let n = prop(n, "text", Value::Text("A".into()));
            prop(n, "text_size", Value::Number(22.0))
        };
        let t2 = {
            let n = node("text");
            prop(n, "text", Value::Text("B".into()))
        };
        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.width = Some(200.0);
        col.children = vec![t1, t2];
        let mut page = node("screen");
        page.children = vec![col];

        let f = run_ui(&page, &ValueMap::new(), &styles(), &input_at(-9.0, -9.0, false), &mut UiState::new());
        let ys: Vec<f32> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { y, .. } => Some(*y),
                _ => None,
            })
            .collect();
        assert_eq!(ys.len(), 2, "both rows drew");
        assert_eq!(ys[1] - ys[0], 32.0, "the 22px row reserved 22 + leading 10");
    }

    #[test]
    fn display_strings_resolve_through_the_stringtable() {
        // `$token` display props resolve at the draw boundary — for the Rust text
        // primitive AND for props crossing into a Lua component — while a bound
        // VALUE (user data) passes through untouched.
        let _g = crate::strings::test_guard();
        crate::strings::load_str(r#"{ "walker_go": { "en-us": "GO!" } }"#, "en-us");

        let t = {
            let mut n = node("text");
            n.size = Some(20.0);
            n = prop(n, "label", Value::Text("$walker_go".into()));
            n
        };
        let btn = {
            let mut n = node("button");
            n.id = "b".into();
            n.size = Some(24.0);
            n = prop(n, "label", Value::Text("$walker_go".into()));
            prop(n, "style", Value::Text("btn".into()))
        };
        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.width = Some(160.0);
        col.children = vec![t, btn];
        let mut page = node("screen");
        page.children = vec![col];

        let lib = counting_lib();
        let f = run_ui_with(
            &page,
            &ValueMap::new(),
            &styles(),
            &input_at(-9.0, -9.0, false),
            &mut UiState::new(),
            Some(&lib),
        );
        let texts: Vec<&str> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"GO!"), "the text primitive resolved its token: {texts:?}");
        assert_eq!(
            texts.iter().filter(|t| **t == "GO!").count(),
            2,
            "the Lua button's label crossed already-resolved: {texts:?}"
        );
        assert!(!texts.iter().any(|t| t.contains('$')), "no raw sigil reached a command: {texts:?}");
    }

    #[test]
    fn a_still_frame_redraws_nothing_and_never_enters_lua() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // The blocking defect this cache exists to fix: the second frame of an
        // unchanged screen used to re-marshal and re-run every component's Lua draw.
        let lib = counting_lib();
        let page = tree();
        let model = ValueMap::new().with("flag", true);
        let mut state = UiState::new();
        let go = |state: &mut UiState| {
            run_ui_with(&page, &model, &styles(), &input_at(-9.0, -9.0, false), state, Some(&lib))
        };

        let first = go(&mut state);
        assert_eq!(first.stats.redraw_nodes, first.stats.nodes, "cold frame draws every node");
        assert!(first.stats.lua_draws >= 2, "checkbox + button dispatch to Lua: {:?}", first.stats);
        let drawn_after_first = lib.draws.get();

        let second = go(&mut state);
        assert_eq!(second.stats.redraw_nodes, 0, "an unchanged frame redraws nothing");
        assert_eq!(second.stats.lua_draws, 0, "…and crosses into Lua zero times");
        assert_eq!(lib.draws.get(), drawn_after_first, "the library really was not called");
        assert_eq!(second.commands, first.commands, "the replay is byte-identical");
    }

    #[test]
    fn only_the_nodes_whose_inputs_changed_redraw() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // Flipping one bound value must not disturb its neighbours' cached commands.
        let lib = counting_lib();
        let page = tree();
        let mut state = UiState::new();
        let off = ValueMap::new().with("flag", false);
        let on = ValueMap::new().with("flag", true);
        let input = input_at(-9.0, -9.0, false);

        run_ui_with(&page, &off, &styles(), &input, &mut state, Some(&lib));
        let flipped = run_ui_with(&page, &on, &styles(), &input, &mut state, Some(&lib));
        assert_eq!(flipped.stats.redraw_nodes, 1, "only the checkbox reads `flag`");
        assert_eq!(flipped.stats.lua_draws, 1, "and only it re-enters Lua");
    }

    // ── Hit dispatch (bounded) ───────────────────────────────────────────────
    //
    // S4's twin of the draw-cache tests: component HIT logic lives in Lua, but the
    // walker enters it only on input-active frames, and only for candidate nodes.
    // These use inline probe components so the guarantees hold independent of which
    // real controls have migrated.

    /// A minimal verdict component: claims on rect-hover; a click toggles its bound
    /// bool and fires its action.
    const PROBE_COMPONENT: &str = r#"
        local M = {}
        function M.draw(cmds, r, props) end
        function M.hit(mx, my, r, props, click, down)
          local over = mx >= r.x and mx <= r.x + r.w and my >= r.y and my <= r.y + r.h
          local v = { hit = over }
          if over and click then
            v.value = not (props.bind_value == true)
            v.activate = true
          end
          return v
        end
        return M
    "#;

    #[test]
    fn an_idle_frame_dispatches_zero_hits_and_keeps_hud_hit() {
        let _g = crate::strings::test_guard();
        // Pointer resting on the checkbox: the second, unchanged frame must not cross
        // into Lua for hits OR draws, yet `hud_hit` stays claimed (memo continuity).
        let lib = counting_lib();
        let page = tree();
        let model = ValueMap::new().with("flag", true);
        let mut state = UiState::new();
        // (22,22) is inside the checkbox's 14×14 box at (16,16).
        let over = input_at(22.0, 22.0, false);

        let first = run_ui_with(&page, &model, &styles(), &over, &mut state, Some(&lib));
        assert!(first.results.is_on("hud_hit"), "pointer over the box claims");
        let hits_after_first = lib.hits.get();

        let second = run_ui_with(&page, &model, &styles(), &over, &mut state, Some(&lib));
        assert_eq!(second.stats.lua_hits, 0, "still frame: zero hit crossings");
        assert_eq!(second.stats.lua_draws, 0, "still frame: zero draw crossings");
        assert_eq!(lib.hits.get(), hits_after_first, "the library really was not called");
        assert!(second.results.is_on("hud_hit"), "the claim survives the idle frame");
    }

    #[test]
    fn verdict_dispatch_is_bounded_and_applies_generically() {
        // Two probe nodes stacked in a column; the pointer sits on the FIRST. Only
        // that node is a candidate (the second is beyond the slop), so exactly one
        // Lua hit call happens on the move frame — and zero once the pointer rests.
        let lib = flicker_script::ScriptHost::library(&[("ui.probe", PROBE_COMPONENT)])
            .expect("probe library");
        let mk = |id: &str, bind: &str, action: &str| {
            let mut n = node("probe");
            n.id = id.into();
            n.size = Some(20.0);
            n.bind = Some(bind.into());
            n.action = Some(action.into());
            n
        };
        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.width = Some(120.0);
        col.children = vec![mk("p1", "b1", "a1"), mk("p2", "b2", "a2")];
        let mut page = node("screen");
        page.children = vec![col];
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Move frame: pointer lands on p1 (rows are y 0..20 and 20..40).
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, false), &mut state, Some(&lib));
        assert_eq!(f.stats.lua_hits, 1, "only the hovered node dispatches");
        assert!(f.results.is_on("hud_hit"), "the probe claimed the pointer");

        // Rest frame: zero crossings, claim continuity.
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, false), &mut state, Some(&lib));
        assert_eq!(f.stats.lua_hits, 0, "idle frame never enters Lua");
        assert!(f.results.is_on("hud_hit"));

        // Click frame: the verdict's value/activate route into bind/action — and only
        // for the clicked node.
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, true), &mut state, Some(&lib));
        assert!(f.results.is_on("b1"), "verdict value wrote the bind");
        assert!(f.results.is_on("a1"), "verdict activate fired the action");
        assert!(f.results.get("b2").is_none(), "the un-clicked sibling stays silent");
        assert!(!f.results.is_on("a2"));
    }

    /// A focus-claiming component: a click inside asks for keyboard focus — the
    /// verdict channel a Lua `text_field` claims focus through.
    const FOCUS_COMPONENT: &str = r#"
        local M = {}
        function M.draw(cmds, r, props) end
        function M.hit(mx, my, r, props, click, down)
          local over = mx >= r.x and mx <= r.x + r.w and my >= r.y and my <= r.y + r.h
          local v = { hit = over }
          if over and click then v.focus = true end
          return v
        end
        return M
    "#;

    #[test]
    fn a_focus_verdict_claims_state_focus_and_needs_an_id() {
        let lib = flicker_script::ScriptHost::library(&[("ui.focal", FOCUS_COMPONENT)])
            .expect("focal library");
        // Two stacked focal nodes: one with an id, one with only a bind — focus is
        // held BY id, so the id-less one cannot take it.
        let mut named = node("focal");
        named.id = "f1".into();
        named.size = Some(20.0);
        let mut anon = node("focal");
        anon.bind = Some("nb".into());
        anon.size = Some(20.0);
        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.width = Some(120.0);
        col.children = vec![named, anon];
        let mut page = node("screen");
        page.children = vec![col];
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Click the named node → its verdict claims focus.
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, true), &mut state, Some(&lib));
        assert!(f.results.is_on("hud_hit"));
        assert_eq!(state.focused(), Some("f1"), "focus=true set state.focus to the node id");

        // A non-click frame leaves focus alone.
        run_ui_with(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, false), &mut state, Some(&lib));
        assert_eq!(state.focused(), Some("f1"), "focus persists across idle frames");

        // Clicking the ID-LESS node: the clicked frame clears focus up front, and an
        // empty-id claim is a no-op — nothing re-establishes it.
        run_ui_with(&page, &model, &serde_json::json!({}), &input_at(60.0, 26.0, true), &mut state, Some(&lib));
        assert_eq!(state.focused(), None, "an id-less node cannot hold focus");

        // Re-claim, then click empty space: the generic click-away rule clears.
        run_ui_with(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, true), &mut state, Some(&lib));
        assert_eq!(state.focused(), Some("f1"));
        run_ui_with(&page, &model, &serde_json::json!({}), &input_at(400.0, 300.0, true), &mut state, Some(&lib));
        assert_eq!(state.focused(), None, "clicking away clears focus generically");
    }

    /// A capture component: a click grabs the pointer; while held it reports the
    /// pointer x as its value — even after the pointer leaves the rect.
    const GRAB_COMPONENT: &str = r#"
        local M = {}
        function M.draw(cmds, r, props) end
        function M.hit(mx, my, r, props, click, down)
          local over = mx >= r.x and mx <= r.x + r.w and my >= r.y and my <= r.y + r.h
          local v = { hit = over }
          if click and over then v.capture = true end
          if down and (props.captured == true or v.capture == true) then v.value = mx end
          return v
        end
        return M
    "#;

    #[test]
    fn captured_node_keeps_dispatching_off_rect_until_release() {
        let lib = flicker_script::ScriptHost::library(&[("ui.grab", GRAB_COMPONENT)])
            .expect("grab library");
        let mut g = node("grab");
        g.id = "g".into();
        g.bind = Some("gx".into());
        g.width = Some(100.0);
        g.height = Some(20.0);
        g.anchor = Some(UiAnchor::TopLeft);
        let mut page = node("screen");
        page.children = vec![g];
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Press inside → capture + same-frame value.
        let press = UiInput { mouse: Vec2::new(30.0, 10.0), clicked: true, down: true, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &press, &mut state, Some(&lib));
        assert_eq!(f.results.number("gx"), Some(30.0), "press writes the value");
        assert!(f.results.is_on("hud_hit"));

        // Held, pointer far off the rect: the capture keeps it a candidate, so the
        // value keeps flowing (the select-popup/slider-drag escape from the rect
        // pre-filter).
        let held = UiInput { mouse: Vec2::new(500.0, 300.0), clicked: false, down: true, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &held, &mut state, Some(&lib));
        assert_eq!(f.stats.lua_hits, 1, "the captured node still dispatches");
        assert_eq!(f.results.number("gx"), Some(500.0), "drag keeps writing off-rect");
        assert!(f.results.is_on("hud_hit"), "a live capture claims the pointer");

        // Release (pointer unmoved): capture clears generically; no dispatch, no write.
        let release = UiInput { mouse: Vec2::new(500.0, 300.0), clicked: false, down: false, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &release, &mut state, Some(&lib));
        assert_eq!(f.stats.lua_hits, 0, "release frame is idle — no crossing");
        assert!(f.results.get("gx").is_none(), "no interaction, no echo for a probe kind");
        assert!(!f.results.is_on("hud_hit"), "claim dropped with the capture");
    }

    /// A rect-shaped chip: the walker answers its hit in Rust — zero crossings —
    /// firing `action` and toggling a bool `bind` on a click inside.
    const CHIP_COMPONENT: &str = r#"
        local M = {}
        M.hit_shape = "rect"
        function M.draw(cmds, r, props) end
        function M.hit(mx, my, r) return true end
        return M
    "#;

    #[test]
    fn a_rect_hit_shape_answers_in_rust_with_zero_crossings() {
        let lib = flicker_script::ScriptHost::library(&[("ui.chip", CHIP_COMPONENT)])
            .expect("chip library");
        let mut c = node("chip");
        c.id = "c".into();
        c.bind = Some("lit".into());
        c.action = Some("poke".into());
        c.width = Some(50.0);
        c.height = Some(20.0);
        c.anchor = Some(UiAnchor::TopLeft);
        let mut page = node("screen");
        page.children = vec![c];
        let model = ValueMap::new();
        let mut state = UiState::new();

        let f = run_ui_with(&page, &model, &serde_json::json!({}), &input_at(25.0, 10.0, true), &mut state, Some(&lib));
        assert_eq!(f.stats.lua_hits, 0, "a trivial shape never enters Lua");
        assert!(f.results.is_on("hud_hit"), "the rect claims the pointer");
        assert!(f.results.is_on("poke"), "click inside fires the action");
        assert!(f.results.is_on("lit"), "click inside toggles the bool bind");

        // Outside: no claim, no fire.
        let f = run_ui_with(&page, &model, &serde_json::json!({}), &input_at(200.0, 200.0, true), &mut state, Some(&lib));
        assert!(!f.results.is_on("hud_hit"));
        assert!(!f.results.is_on("poke"));
    }

    #[test]
    fn hovering_redraws_only_the_hovered_node() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // Hover is a draw input (`hot`), so moving onto a control must invalidate that
        // control — and nothing else.
        let lib = counting_lib();
        let page = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();

        run_ui_with(&page, &model, &styles(), &input_at(-9.0, -9.0, false), &mut state, Some(&lib));
        // (20, 45) is inside the button: the column sits at 16,16 with a 20px checkbox
        // above a 24px button.
        let hover = run_ui_with(&page, &model, &styles(), &input_at(20.0, 45.0, false), &mut state, Some(&lib));
        assert_eq!(hover.stats.redraw_nodes, 1, "only the button's hover state changed: {:?}", hover.stats);
    }

    #[test]
    fn a_rebuilt_tree_still_hits_the_cache() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // Loomforge and the chat panel rebuild their whole `UiNode` tree every frame, so
        // cache identity has to be structural, not the address of a retained node.
        let lib = counting_lib();
        let model = ValueMap::new().with("flag", true);
        let input = input_at(-9.0, -9.0, false);
        let mut state = UiState::new();

        run_ui_with(&tree(), &model, &styles(), &input, &mut state, Some(&lib));
        let rebuilt = run_ui_with(&tree(), &model, &styles(), &input, &mut state, Some(&lib));
        assert_eq!(rebuilt.stats.redraw_nodes, 0, "an equal tree rebuilt from scratch replays");
        assert_eq!(rebuilt.stats.lua_draws, 0, "…without re-entering Lua");
    }

    #[test]
    fn restyling_redraws_the_nodes_that_use_the_changed_block() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // Cached commands carry RESOLVED colours, so a hot-reloaded `ui_elements.json`
        // must invalidate them — while an equal tree rebuilt at a new address must not
        // (the fingerprint folds block CONTENT, never its address).
        let lib = counting_lib();
        let page = tree();
        let model = ValueMap::new().with("flag", true);
        let input = input_at(-9.0, -9.0, false);
        let mut state = UiState::new();

        run_ui_with(&page, &model, &styles(), &input, &mut state, Some(&lib));
        let same = run_ui_with(&page, &model, &styles(), &input, &mut state, Some(&lib));
        assert_eq!(same.stats.redraw_nodes, 0, "an equal styles tree still replays");

        let mut restyled = styles();
        restyled["btn"]["fill_top"] = serde_json::json!([1.0, 0.0, 0.0, 1.0]);
        let reloaded = run_ui_with(&page, &model, &restyled, &input, &mut state, Some(&lib));
        assert_eq!(reloaded.stats.redraw_nodes, 1, "only the button reads `btn`: {:?}", reloaded.stats);
    }

    #[test]
    fn a_list_bar_redraws_when_its_content_changes_height() {
        // The bar's thumb is sized from the CONTENT, which lives in separately
        // fingerprinted children — so hiding a row has to invalidate the region itself.
        let mut sc = node("list");
        sc.id = "sc".into();
        sc.bind = Some("sy".into());
        sc.width = Some(200.0);
        sc.height = Some(100.0);
        sc.anchor = Some(UiAnchor::TopLeft);
        sc = prop(sc, "gutter", Value::Number(0.0));
        for i in 0..4 {
            let mut row = node("cell");
            row.id = format!("row{i}");
            row.size = Some(50.0);
            row.visible_bind = Some(format!("show{i}"));
            sc.children.push(row);
        }
        let mut page = node("screen");
        page.children = vec![sc];
        let styles = serde_json::json!({});
        let input = input_at(-9.0, -9.0, false);
        let all = |n: usize| {
            let mut m = ValueMap::new();
            for i in 0..4 {
                m = m.with(format!("show{i}").as_str(), i < n);
            }
            m
        };
        let mut state = UiState::new();

        run_ui(&page, &all(4), &styles, &input, &mut state);
        let hidden = run_ui(&page, &all(3), &styles, &input, &mut state);
        let bars = hidden
            .commands
            .iter()
            .filter(|c| matches!(c, HudCommand::Rect { .. }))
            .count();
        assert!(bars > 0, "200 content over a 100 viewport still overflows, so a bar draws");
        assert!(
            hidden.stats.redraw_nodes >= 1,
            "the list region redraws when a row leaves: {:?}",
            hidden.stats
        );
    }

    #[test]
    fn a_segments_label_change_redraws_its_parent() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // A segmented control's children are DATA it draws itself, never placed nodes —
        // so nothing but the parent's own fingerprint can notice they changed.
        let lib = counting_lib();
        let seg = |label: &str| {
            let mut opt = node("option");
            opt = prop(opt, "value", Value::Text("a".into()));
            opt = prop(opt, "label", Value::Text(label.into()));
            let mut t = node("tabs");
            t.id = "tabs".into();
            t.bind = Some("tab".into());
            t.anchor = Some(UiAnchor::TopLeft);
            t.width = Some(200.0);
            t.height = Some(30.0);
            t.children = vec![opt];
            let mut page = node("screen");
            page.children = vec![t];
            page
        };
        let model = ValueMap::new().with("tab", "a");
        let input = input_at(-9.0, -9.0, false);
        let styles = serde_json::json!({});
        let mut state = UiState::new();

        run_ui_with(&seg("ONE"), &model, &styles, &input, &mut state, Some(&lib));
        let same = run_ui_with(&seg("ONE"), &model, &styles, &input, &mut state, Some(&lib));
        assert_eq!(same.stats.redraw_nodes, 0, "an unchanged strip replays");
        let renamed = run_ui_with(&seg("TWO"), &model, &styles, &input, &mut state, Some(&lib));
        assert_eq!(renamed.stats.redraw_nodes, 1, "a renamed segment redraws the strip");
    }

    #[test]
    fn list_emits_a_viewport_clip_and_wheel_moves_the_bound_offset() {
        // A 200×100 list viewport holding 3 rows of 50 = 150 content → 50px max.
        let mut sc = node("list");
        sc.id = "sc".into();
        sc.bind = Some("sy".into());
        sc.width = Some(200.0);
        sc.height = Some(100.0);
        sc.anchor = Some(UiAnchor::TopLeft);
        sc = prop(sc, "gutter", Value::Number(0.0)); // full-width viewport for the assertions
        for i in 0..3 {
            let mut row = node("cell");
            row.id = format!("row{i}");
            row.size = Some(50.0);
            sc.children.push(row);
        }
        let mut page = node("screen");
        page.children = vec![sc];
        let styles = serde_json::json!({});

        // At rest: the content subtree is clipped to the 200×100 viewport, then reset.
        let model = ValueMap::new().with("sy", 0.0);
        let frame = run_ui(&page, &model, &styles, &input_at(-1.0, -1.0, false), &mut UiState::new());
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Clip { rect: Some(r) }
                if (r[2] - 200.0).abs() < 0.5 && (r[3] - 100.0).abs() < 0.5)),
            "list subtree is clipped to its viewport"
        );
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Clip { rect: None })),
            "the clip is reset after the list region"
        );

        // Wheel down over the region moves the bound offset, within [0, 50] — the
        // wheel rides `UiInput.wheel` now, not a Model-key convention.
        let model = ValueMap::new().with("sy", 0.0);
        let frame = run_ui(&page, &model, &styles, &input_wheel(100.0, 50.0, -1.0), &mut UiState::new());
        let sy = frame.results.number("sy").expect("scroll offset reported");
        assert!(sy > 0.0 && sy <= 50.0, "wheel scrolled within bounds: {sy}");

        // A large delta clamps at the content max.
        let frame = run_ui(&page, &model, &styles, &input_wheel(100.0, 50.0, -10.0), &mut UiState::new());
        assert_eq!(frame.results.number("sy"), Some(50.0), "clamped to the content max");
    }

    /// Build the canonical list-region fixture the S7 behaviour tests share: a
    /// `rows × row_h` column in a `w × h` viewport at the top-left origin, pad 0,
    /// gutter 0 — every derived quantity (content_h, max, thumb) is exact in both
    /// f32 and f64, so byte-level assertions are stable.
    fn scroll_fixture(w: f32, h: f32, rows: usize, row_h: f32, style: Option<&str>) -> UiNode {
        let mut sc = node("list");
        sc.id = "sc".into();
        sc.bind = Some("sy".into());
        sc.width = Some(w);
        sc.height = Some(h);
        sc.anchor = Some(UiAnchor::TopLeft);
        sc = prop(sc, "gutter", Value::Number(0.0));
        if let Some(s) = style {
            sc = prop(sc, "style", Value::Text(s.into()));
        }
        for i in 0..rows {
            let mut row = node("cell");
            row.id = format!("row{i}");
            row.size = Some(row_h);
            sc.children.push(row);
        }
        let mut page = node("screen");
        page.children = vec![sc];
        page
    }

    /// `ui/list.lua`'s neutral bar fallbacks (track / thumb when the style block
    /// carries neither) — pinned here so a module palette drift fails a test.
    const STONE: [f32; 4] = [0.055, 0.063, 0.086, 1.0];
    const SAP: [f32; 4] = [0.141, 0.247, 0.471, 1.0];

    /// The two bar rects (track, thumb) a frame drew, in emit order.
    fn bar_rects(cmds: &[HudCommand]) -> Vec<(f32, f32, f32, f32, [f32; 4])> {
        cmds.iter()
            .filter_map(|c| match c {
                HudCommand::Rect { x, y, w, h, color, .. } => Some((*x, *y, *w, *h, *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn list_bar_geometry_tracks_content_and_clamps_thumb() {
        // 256×128 viewport, 4 rows of 64 → content 256, max 128, thumb 128·(128/256)=64
        // — every quantity exact, so the rect assertions are byte-level.
        let page = scroll_fixture(256.0, 128.0, 4, 64.0, None);
        let styles = serde_json::json!({});
        let run = |sy: f64| {
            let m = ValueMap::new().with("sy", sy);
            run_ui(&page, &m, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new())
        };

        // At rest: track spans the viewport at the inner right edge (bar_w default 4);
        // the thumb is proportional and top-aligned.
        let bars = bar_rects(&run(0.0).commands);
        assert_eq!(
            bars,
            vec![(252.0, 0.0, 4.0, 128.0, STONE), (252.0, 0.0, 4.0, 64.0, SAP)],
            "track + thumb at offset 0"
        );

        // Mid offset maps linearly over the free travel (128−64): 32/128 · 64 = 16.
        let bars = bar_rects(&run(32.0).commands);
        assert_eq!(bars[1], (252.0, 16.0, 4.0, 64.0, SAP), "thumb rides the offset");

        // An over-max bound value draws clamped: the thumb parks at the bottom.
        let bars = bar_rects(&run(999.0).commands);
        assert_eq!(bars[1], (252.0, 64.0, 4.0, 64.0, SAP), "thumb clamps to the end of travel");

        // Very tall content: the proportional thumb (128·128/4096 = 4) clamps to the
        // 28px minimum, and the travel shrinks to match (max offset → y = 128−28).
        let tall = scroll_fixture(256.0, 128.0, 64, 64.0, None);
        let m = ValueMap::new().with("sy", 4096.0);
        let f = run_ui(&tall, &m, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        let bars = bar_rects(&f.commands);
        assert_eq!(bars[1], (252.0, 100.0, 4.0, 28.0, SAP), "thumb has a 28px floor");
    }

    #[test]
    fn list_backdrop_only_when_styled_and_no_bar_when_content_fits() {
        // Content (1 row of 64) fits the 128 viewport: no track/thumb either way; the
        // backdrop panel appears ONLY when the node carries a style.
        let styles = serde_json::json!({ "well": { "panel_bg": [0.125, 0.25, 0.5, 1.0], "radius": 2 } });
        let go = |style: Option<&str>| {
            let page = scroll_fixture(256.0, 128.0, 1, 64.0, style);
            run_ui(&page, &ValueMap::new(), &styles, &input_at(-9.0, -9.0, false), &mut UiState::new())
        };

        let unstyled = go(None);
        assert_eq!(bar_rects(&unstyled.commands), vec![], "fitting content draws no bar");
        assert!(
            !unstyled.commands.iter().any(|c| matches!(c, HudCommand::Panel { .. })),
            "an unstyled region is transparent structure"
        );

        let styled = go(Some("well"));
        assert_eq!(bar_rects(&styled.commands), vec![], "styling adds no bar");
        let panels: Vec<_> =
            styled.commands.iter().filter(|c| matches!(c, HudCommand::Panel { .. })).collect();
        assert_eq!(panels.len(), 1, "the styled region draws exactly its backdrop");
        assert!(
            matches!(panels[0], HudCommand::Panel { x: 0.0, y: 0.0, w: 256.0, h: 128.0, .. }),
            "the backdrop fills the node rect (unclipped): {:?}",
            panels[0]
        );
    }

    #[test]
    fn list_claims_hud_hit_and_wheel_clamps_at_zero() {
        let page = scroll_fixture(256.0, 128.0, 4, 64.0, None);
        let styles = serde_json::json!({});

        // Pointer over the region claims; parked outside it does not.
        let m = ValueMap::new().with("sy", 0.0);
        let f = run_ui(&page, &m, &styles, &input_at(100.0, 60.0, false), &mut UiState::new());
        assert!(f.results.is_on("hud_hit"), "the region claims the pointer");
        let f = run_ui(&page, &m, &styles, &input_at(400.0, 300.0, false), &mut UiState::new());
        assert!(!f.results.is_on("hud_hit"), "outside the region nothing claims");

        // Wheel UP at the top edge stays clamped at 0 (the lower bound's twin of the
        // content-max clamp the sibling test pins).
        let m = ValueMap::new().with("sy", 0.0);
        let f = run_ui(&page, &m, &styles, &input_wheel(100.0, 60.0, 1.0), &mut UiState::new());
        assert_eq!(f.results.number("sy"), Some(0.0), "wheel up from the top clamps at 0");
    }

    #[test]
    fn a_scrolled_row_redraws_at_its_shifted_position() {
        let _g = crate::strings::test_guard();
        // Rows are text leaves so their draw position is observable; scrolling the
        // region must redraw them at the shifted y (rect is a fingerprint input),
        // including the row the offset reveals from below the fold.
        let mut sc = node("list");
        sc.id = "sc".into();
        sc.bind = Some("sy".into());
        sc.width = Some(256.0);
        sc.height = Some(128.0);
        sc.anchor = Some(UiAnchor::TopLeft);
        sc = prop(sc, "gutter", Value::Number(0.0));
        for i in 0..4 {
            let mut row = node("text");
            row.id = format!("row{i}");
            row.size = Some(64.0);
            row = prop(row, "text", Value::Text(format!("R{i}")));
            sc.children.push(row);
        }
        let mut page = node("screen");
        page.children = vec![sc];
        let styles = serde_json::json!({});
        let input = input_at(-9.0, -9.0, false);
        let mut state = UiState::new();

        let ys = |cmds: &[HudCommand]| -> Vec<f32> {
            cmds.iter()
                .filter_map(|c| match c {
                    HudCommand::Text { y, .. } => Some(*y),
                    _ => None,
                })
                .collect()
        };
        let at0 = run_ui(&page, &ValueMap::new().with("sy", 0.0), &styles, &input, &mut state);
        assert_eq!(ys(&at0.commands), vec![0.0, 64.0, 128.0, 192.0]);
        let at40 = run_ui(&page, &ValueMap::new().with("sy", 40.0), &styles, &input, &mut state);
        assert_eq!(
            ys(&at40.commands),
            vec![-40.0, 24.0, 88.0, 152.0],
            "every row redrew at its scrolled position"
        );
        assert!(at40.stats.redraw_nodes >= 4, "the moved rows really redrew: {:?}", at40.stats);
    }

    // (The transient S7 parity test — the Rust `scroll` draw arm vs `ui/list.lua`,
    // byte for byte across styled/unstyled × short/long content × offsets
    // 0/mid/max/over-max — gated the port and was deleted with the Rust arm; the
    // byte-pinned draw below and the behaviour tests around it carry the duty.)
    #[test]
    fn list_lua_draw_is_byte_pinned() {
        // Both draw branches at the byte level, outliving the deleted Rust arm: a
        // styled overflowing region (backdrop + track + thumb + viewport clip) and
        // an unstyled fitting one (nothing but the clip toggles). pad 4 in a
        // 256×128 box → inner 248×120; 2×116 rows → content 8+232 = 240 (twice the
        // viewport: thumb 60, free travel 60, max offset 120 — every quantity
        // exact, so the pins are stable).
        let styles = serde_json::json!({
            "well": { "panel_bg": [0.125, 0.25, 0.5, 1.0], "radius": 2,
                      "bar_w": 8, "track": [0.25, 0.5, 0.75, 1.0], "thumb": [1.0, 0.5, 0.25, 1.0] }
        });
        let region = |style: Option<&str>, rows: usize, row_h: f32| {
            let mut sc = node("list");
            sc.id = "sc".into();
            sc.bind = Some("sy".into());
            sc.width = Some(256.0);
            sc.height = Some(128.0);
            sc.pad = 4.0;
            sc.anchor = Some(UiAnchor::TopLeft);
            if let Some(s) = style {
                sc = prop(sc, "style", Value::Text(s.into()));
            }
            for i in 0..rows {
                let mut row = node("cell");
                row.id = format!("row{i}");
                row.size = Some(row_h);
                sc.children.push(row);
            }
            let mut page = node("screen");
            page.children = vec![sc];
            page
        };
        let bar = |y: f32, h: f32, color: [f32; 4]| HudCommand::Rect {
            x: 244.0,
            y,
            w: 8.0,
            h,
            color,
            layer: 0.0,
        };
        // The children's viewport clip: inner (248 wide) minus the default 16px bar
        // gutter the layout reserves on the right.
        let clip = HudCommand::Clip { rect: Some([4.0, 4.0, 232.0, 120.0]) };
        let unclip = HudCommand::Clip { rect: None };

        // Styled + overflowing at a mid offset: backdrop, then track + thumb
        // (30/120 of the 60px free travel → the thumb rides 15px below the top).
        let f = run_ui(
            &region(Some("well"), 2, 116.0),
            &ValueMap::new().with("sy", 30.0),
            &styles,
            &input_at(-9.0, -9.0, false),
            &mut UiState::new(),
        );
        let expected = vec![
            HudCommand::Panel {
                x: 0.0,
                y: 0.0,
                w: 256.0,
                h: 128.0,
                color: [0.125, 0.25, 0.5, 1.0],
                color2: [0.125, 0.25, 0.5, 1.0],
                grad: 0.0,
                radius: 2.0,
                border: 0.0,
                border_color: [0.0, 0.0, 0.0, 0.0],
                feather: 0.0,
                layer: 0.0,
            },
            bar(4.0, 120.0, [0.25, 0.5, 0.75, 1.0]),
            bar(19.0, 60.0, [1.0, 0.5, 0.25, 1.0]),
            clip.clone(),
            unclip.clone(),
        ];
        assert_eq!(f.commands, expected, "the styled overflowing draw is byte-stable");

        // An over-max offset draws clamped: the thumb parks at the end of travel.
        let f = run_ui(
            &region(Some("well"), 2, 116.0),
            &ValueMap::new().with("sy", 999.0),
            &styles,
            &input_at(-9.0, -9.0, false),
            &mut UiState::new(),
        );
        assert_eq!(f.commands[2], bar(64.0, 60.0, [1.0, 0.5, 0.25, 1.0]), "clamped thumb pins");

        // Unstyled + fitting content: transparent structure — only the clip toggles.
        let f = run_ui(
            &region(None, 1, 100.0),
            &ValueMap::new().with("sy", 0.0),
            &styles,
            &input_at(-9.0, -9.0, false),
            &mut UiState::new(),
        );
        assert_eq!(f.commands, vec![clip, unclip], "the empty draw is byte-stable");
    }

    #[test]
    fn a_wheel_tick_is_input_active_and_scrolls_under_a_parked_pointer() {
        let _g = crate::strings::test_guard();
        // The S7 gate extension: a wheel tick with a motionless pointer must count
        // as input activity (the list under the cursor scrolls), while wheel-less
        // still frames keep the S4 zero-crossing guarantee.
        let lib = counting_lib();
        let page = scroll_fixture(256.0, 128.0, 4, 64.0, None);
        let m = ValueMap::new().with("sy", 0.0);
        let styles = serde_json::json!({});
        let mut state = UiState::new();
        let over = input_at(100.0, 60.0, false);

        run_ui_with(&page, &m, &styles, &over, &mut state, Some(&lib));
        let still = run_ui_with(&page, &m, &styles, &over, &mut state, Some(&lib));
        assert_eq!(still.stats.lua_hits, 0, "a wheel-less still frame never enters Lua");
        assert_eq!(still.stats.lua_draws, 0);
        assert!(still.results.is_on("hud_hit"), "the claim survives via the memo");

        // Wheel tick, pointer unmoved: dispatch happens and one notch scrolls the
        // bind by the default 46px speed, and the moved thumb redraws the region.
        let f = run_ui_with(&page, &m, &styles, &input_wheel(100.0, 60.0, -1.0), &mut state, Some(&lib));
        assert!(f.stats.lua_hits >= 1, "the wheel tick alone is input-active: {:?}", f.stats);
        assert_eq!(f.results.number("sy"), Some(46.0), "one notch × the default speed");
        assert!(f.results.is_on("hud_hit"));
        assert!(f.stats.redraw_nodes >= 1, "the scrolled region redrew: {:?}", f.stats);

        // A wheel tick with the pointer OFF the region scrolls nothing.
        let mut fresh = UiState::new();
        let f = run_ui_with(&page, &m, &styles, &input_wheel(400.0, 300.0, -1.0), &mut fresh, Some(&lib));
        assert_eq!(f.results.number("sy"), Some(0.0), "off-region wheel only echoes");
    }

    #[test]
    fn a_list_echoes_its_bound_offset_like_any_control() {
        // The generic idle contract (replacing the old arm's hover-only reporting):
        // an untouched list reports its effective offset every frame — the model's
        // number, else 0 (the top).
        let page = scroll_fixture(256.0, 128.0, 4, 64.0, None);
        let styles = serde_json::json!({});
        let idle = input_at(-9.0, -9.0, false);
        let f = run_ui(&page, &ValueMap::new().with("sy", 12.0), &styles, &idle, &mut UiState::new());
        assert_eq!(f.results.number("sy"), Some(12.0), "idle echo reports the model value");
        let f = run_ui(&page, &ValueMap::new(), &styles, &idle, &mut UiState::new());
        assert_eq!(f.results.number("sy"), Some(0.0), "an absent offset defaults to 0");
    }

    #[test]
    fn button_click_fires_action_and_claims_mouse() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let t = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();
        // Column at (16,16) width 120: checkbox rows y 16..36, button y 36..60.
        let frame =
            run_ui_with(&t, &model, &styles(), &input_at(50.0, 48.0, true), &mut state, Some(&lib));
        assert!(frame.results.is_on("go"), "button action fired");
        assert!(frame.results.is_on("hud_hit"), "pointer over UI claims the mouse");
        assert!(!frame.commands.is_empty(), "something was drawn");
        assert!(!frame.results.is_on("flag"), "checkbox untouched by a button click");
    }

    #[test]
    fn checkbox_click_toggles_bound_value() {
        let t = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();
        // Checkbox box is the 14×14 at the column's top-left (16..30, 16..30).
        let frame = run_ui(&t, &model, &styles(), &input_at(22.0, 22.0, true), &mut state);
        assert!(frame.results.is_on("flag"), "checkbox toggled its bind on");
    }

    #[test]
    fn tile_hover_claims_click_toggles_and_always_echoes() {
        // A tile is a full-rect control: hovering anywhere on it claims the pointer,
        // a click toggles its bound bool, and the bind echoes every frame regardless.
        let mut tl = node("tile");
        tl.id = "t1".into();
        tl.bind = Some("sel".into());
        tl.width = Some(60.0);
        tl.height = Some(60.0);
        tl.anchor = Some(UiAnchor::TopLeft);
        let mut page = node("screen");
        page.children = vec![tl];
        let st = serde_json::json!({});
        let model = ValueMap::new().with("sel", false);
        let mut state = UiState::new();

        // Hover (no click): claims, does not toggle, still echoes.
        let f = run_ui(&page, &model, &st, &input_at(30.0, 30.0, false), &mut state);
        assert!(f.results.is_on("hud_hit"), "tile hover claims the pointer");
        assert!(!f.results.is_on("sel"), "hover alone does not toggle");
        assert!(f.results.get("sel").is_some(), "bind echoes on a hover frame");

        // Click inside: toggles on.
        let f = run_ui(&page, &model, &st, &input_at(30.0, 30.0, true), &mut state);
        assert!(f.results.is_on("sel"), "tile click toggles its bind on");

        // Click outside: no claim, no toggle — but the echo still reports.
        let f = run_ui(&page, &model, &st, &input_at(300.0, 300.0, true), &mut state);
        assert!(!f.results.is_on("hud_hit"));
        assert!(!f.results.is_on("sel"), "outside click leaves the bind alone");
        assert!(f.results.get("sel").is_some(), "bind echoes on an off-pointer frame");
    }

    #[test]
    fn badge_rim_outside_its_inset_pill_does_not_claim() {
        // A badge whose style insets the pill (`pad` 6, `h` 16 in a 60×24 node):
        // the claim region is the PILL, not the node rect — the rim is inert.
        let mut b = node("badge");
        b.id = "b".into();
        b.width = Some(60.0);
        b.height = Some(24.0);
        b.anchor = Some(UiAnchor::TopLeft);
        b = prop(b, "label", Value::Text("HOT".into()));
        b = prop(b, "style", Value::Text("badge".into()));
        let mut page = node("screen");
        page.children = vec![b];
        let st = serde_json::json!({ "badge": { "pad": 6, "h": 16 } });
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Pill spans x 6..54, y 4..20. The left rim (x=2) is inside the node rect
        // but outside the pill.
        let f = run_ui(&page, &model, &st, &input_at(2.0, 12.0, false), &mut state);
        assert!(!f.results.is_on("hud_hit"), "the rim outside the pill claims nothing");
        // Above the pill (y=2) likewise.
        let f = run_ui(&page, &model, &st, &input_at(30.0, 2.0, false), &mut state);
        assert!(!f.results.is_on("hud_hit"), "above the centred pill claims nothing");
        // Inside the pill: claims.
        let f = run_ui(&page, &model, &st, &input_at(30.0, 12.0, false), &mut state);
        assert!(f.results.is_on("hud_hit"), "the pill itself claims the pointer");
    }

    #[test]
    fn checkbox_label_row_outside_box_neither_toggles_nor_claims() {
        // The checkbox's TIGHT region is its 14×14 box, not the label row: a click on
        // the caption (inside the node rect, right of the box) must neither flip the
        // bind nor claim the pointer — while the two-way echo still reports the value.
        let t = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();
        // Node rect spans (16,16)..(136,36); the box only (16,16)..(30,30). Click the
        // label area at (60,22).
        let frame = run_ui(&t, &model, &styles(), &input_at(60.0, 22.0, true), &mut state);
        assert!(!frame.results.is_on("flag"), "label-row click does not toggle");
        assert!(!frame.results.is_on("hud_hit"), "label-row click does not claim");
        assert!(frame.results.get("flag").is_some(), "the bind still echoes every frame");

        // And hovering the box WITHOUT clicking claims but does not toggle.
        let frame = run_ui(&t, &model, &styles(), &input_at(22.0, 22.0, false), &mut state);
        assert!(frame.results.is_on("hud_hit"), "box hover claims");
        assert!(!frame.results.is_on("flag"), "hover alone does not toggle");
    }

    #[test]
    fn toggle_click_flips_bound_bool_and_stays_in_rect() {
        // A 50×25 toggle pill (dims from its style block) as the only child of a
        // page, anchored top-left so its node rect is exactly the pill's box.
        let mut tg = node("toggle");
        tg.id = "tg".into();
        tg.bind = Some("sw".into());
        tg.width = Some(50.0);
        tg.height = Some(25.0);
        tg.anchor = Some(UiAnchor::TopLeft);
        tg = prop(tg, "style", Value::Text("tg".into()));
        let mut page = node("screen");
        page.children = vec![tg];

        let st = serde_json::json!({
            "tg": {
                "w": 50, "h": 25,
                "on_top": [0.14, 0.25, 0.47, 1.0], "on_bot": [0.10, 0.18, 0.36, 1.0],
                "on_border": [0.20, 0.30, 0.60, 1.0],
                "off_bg": [0.08, 0.09, 0.12, 1.0], "off_border": [0.20, 0.23, 0.28, 1.0],
                "knob_on": [0.93, 0.95, 1.0, 1.0], "knob_off": [0.56, 0.54, 0.49, 1.0]
            }
        });
        let model = ValueMap::new().with("sw", false);
        let mut state = UiState::new();

        // Off → a click anywhere on the pill (spans 0..50 × 0..25) flips it on.
        let frame = run_ui(&page, &model, &st, &input_at(25.0, 12.0, true), &mut state);
        assert!(frame.results.is_on("sw"), "toggle flipped its bind on");
        assert!(frame.results.is_on("hud_hit"), "pointer over the pill claims the mouse");

        // Every emitted panel (pill + knob) stays inside the 50×25 node rect.
        for c in &frame.commands {
            if let HudCommand::Panel { x, y, w, h, .. } = c {
                assert!(
                    *x >= -0.01 && *y >= -0.01 && x + w <= 50.01 && y + h <= 25.01,
                    "toggle geometry within node rect: {x},{y} {w}×{h}"
                );
            }
        }

        // Two-way echo: a non-click frame reports the model's current value unchanged.
        let on = ValueMap::new().with("sw", true);
        let idle = UiInput { mouse: Vec2::new(999.0, 999.0), clicked: false, down: false, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let frame = run_ui(&page, &on, &st, &idle, &mut state);
        assert!(frame.results.is_on("sw"), "off-pointer frame still echoes the bound bool");
    }

    #[test]
    fn toggle_row_outside_pill_neither_flips_nor_claims() {
        // In a row WIDER than its pill, the toggle's tight region is the 50×25 pill at
        // the left edge (vertically centred) — a click in the rest of the row is inert.
        let mut tg = node("toggle");
        tg.id = "tg".into();
        tg.bind = Some("sw".into());
        tg.width = Some(200.0);
        tg.height = Some(41.0);
        tg.anchor = Some(UiAnchor::TopLeft);
        tg = prop(tg, "style", Value::Text("tg".into()));
        let mut page = node("screen");
        page.children = vec![tg];
        let st = serde_json::json!({ "tg": { "w": 50, "h": 25 } });
        let model = ValueMap::new().with("sw", false);
        let mut state = UiState::new();

        // Pill spans x 0..50, y 8..33 ((41-25)/2 = 8). Click at x=120 — inside the
        // node rect, right of the pill.
        let frame = run_ui(&page, &model, &st, &input_at(120.0, 20.0, true), &mut state);
        assert!(!frame.results.is_on("sw"), "row click outside the pill does not flip");
        assert!(!frame.results.is_on("hud_hit"), "…and does not claim the pointer");
        assert!(frame.results.get("sw").is_some(), "the bind still echoes every frame");

        // Above the pill (y=3 < 8) inside the row: still inert.
        let frame = run_ui(&page, &model, &st, &input_at(25.0, 3.0, true), &mut state);
        assert!(!frame.results.is_on("sw"), "click above the centred pill does not flip");

        // Inside the pill: flips and claims.
        let frame = run_ui(&page, &model, &st, &input_at(25.0, 20.0, true), &mut state);
        assert!(frame.results.is_on("sw"), "pill click flips");
        assert!(frame.results.is_on("hud_hit"), "pill click claims");
    }

    #[test]
    fn radio_click_selects_its_value_and_echoes_otherwise() {
        // Two radios sharing the exclusive group key "choice"; model starts on "a".
        let radio = |id: &str, value: &str| {
            let mut n = node("radio");
            n.id = id.into();
            n.size = Some(20.0);
            n.bind = Some("choice".into());
            n = prop(n, "box", Value::Number(14.0));
            n = prop(n, "value", Value::Text(value.into()));
            n = prop(n, "label", Value::Text(value.into()));
            prop(n, "style", Value::Text("cb".into()))
        };
        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [16.0, 16.0];
        col.width = Some(120.0);
        col.children = vec![radio("r_a", "a"), radio("r_b", "b")];
        let mut page = node("screen");
        page.children = vec![col];

        let model = ValueMap::new().with("choice", "a");
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");

        // Column at (16,16): row A circle 16..30 × 16..30, row B circle 16..30 ×
        // 36..50. Click inside row B's circle → the group selects "b".
        let frame = run_ui_with(
            &page,
            &model,
            &styles(),
            &input_at(22.0, 42.0, true),
            &mut state,
            Some(&lib),
        );
        assert_eq!(frame.results.text("choice"), Some("b"), "clicking row B selects b");
        assert!(frame.results.is_on("hud_hit"), "the radio circle claims the pointer");

        // The selected row draws a filled inner dot INSIDE row B's 14×14 box.
        let dot = frame.commands.iter().find_map(|c| match c {
            HudCommand::Panel { x, y, w, h, .. } if *w < 14.0 && *h < 14.0 => Some((*x, *y, *w, *h)),
            _ => None,
        });
        let (dx, dy, dw, dh) = dot.expect("selected radio draws an inner dot");
        assert!(
            dx >= 16.0 && dy >= 36.0 && dx + dw <= 30.0 && dy + dh <= 50.0,
            "dot stays within row B's box, got ({dx},{dy},{dw},{dh})"
        );

        // A frame with the pointer off every row leaves the selection intact: each
        // row echoes the model's current value, none overwrites it with its own.
        let frame = run_ui_with(
            &page,
            &model,
            &styles(),
            &input_at(300.0, 300.0, false),
            &mut state,
            Some(&lib),
        );
        assert_eq!(frame.results.text("choice"), Some("a"), "no-click frame echoes current selection");

        // The radio's TIGHT region is its circle: a click on row B's LABEL area
        // (inside the node rect, right of the 14×14 circle) neither selects nor
        // claims — while the group key still echoes.
        let frame = run_ui_with(
            &page,
            &model,
            &styles(),
            &input_at(70.0, 42.0, true),
            &mut state,
            Some(&lib),
        );
        assert_eq!(frame.results.text("choice"), Some("a"), "label-row click does not select");
        assert!(!frame.results.is_on("hud_hit"), "label-row click does not claim");
    }

    #[test]
    fn stepper_buttons_step_and_clamp_bound_value() {
        // A 120×24 stepper at the top-left: field spans the row, so the − end
        // button is x 0..24 and the + end button is x 96..120 (each square).
        let mut sp = node("stepper");
        sp.id = "sp".into();
        sp.bind = Some("qty".into());
        sp.width = Some(120.0);
        sp.height = Some(24.0);
        sp.anchor = Some(UiAnchor::TopLeft);
        sp = prop(sp, "min", Value::Number(0.0));
        sp = prop(sp, "max", Value::Number(10.0));
        sp = prop(sp, "step", Value::Number(1.0));
        sp = prop(sp, "decimals", Value::Number(0.0));
        let mut page = node("stepper_page");
        page.children = vec![sp];

        let st = serde_json::json!({});

        // − button (left square): 5 → 4, and the pointer claims the mouse.
        let model = ValueMap::new().with("qty", 5.0);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &st, &input_at(12.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(4.0), "− steps down by step");
        assert!(frame.results.is_on("hud_hit"), "pointer over the stepper claims the mouse");

        // + button (right square): 5 → 6.
        let frame = run_ui(&page, &model, &st, &input_at(108.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(6.0), "+ steps up by step");

        // No click (pointer between the buttons) → echoes the bound value.
        let frame = run_ui(&page, &model, &st, &input_at(60.0, 12.0, false), &mut state);
        assert_eq!(frame.results.number("qty"), Some(5.0), "reports current value each frame");

        // Clamp at the floor: 0 − step stays at min.
        let lo = ValueMap::new().with("qty", 0.0);
        let frame = run_ui(&page, &lo, &st, &input_at(12.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(0.0), "clamped at min");

        // Clamp at the ceiling: 10 + step stays at max.
        let hi = ValueMap::new().with("qty", 10.0);
        let frame = run_ui(&page, &hi, &st, &input_at(108.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(10.0), "clamped at max");

        // A CLICK between the end buttons (the value field, x 24..96) steps nothing —
        // the tight regions are the two squares — but still claims and echoes.
        let frame = run_ui(&page, &model, &st, &input_at(60.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(5.0), "field click steps nothing");
        assert!(frame.results.is_on("hud_hit"), "the stepper row claims the pointer");
    }

    #[test]
    fn stepper_geometry_stays_within_node_rect() {
        let mut sp = node("stepper");
        sp.id = "sp".into();
        sp.bind = Some("qty".into());
        sp.width = Some(120.0);
        sp.height = Some(24.0);
        sp.anchor = Some(UiAnchor::TopLeft);
        sp = prop(sp, "min", Value::Number(0.0));
        sp = prop(sp, "max", Value::Number(10.0));
        let mut page = node("stepper_page");
        page.children = vec![sp];

        let model = ValueMap::new().with("qty", 5.0);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &serde_json::json!({}), &input_at(0.0, 0.0, false), &mut state);
        // Every drawn box (field + both end buttons) lands inside the 120×24 rect.
        for c in &frame.commands {
            if let HudCommand::Rect { x, y, w, h, .. } = c {
                assert!(
                    *x >= -0.01 && *y >= -0.01 && x + w <= 120.01 && y + h <= 24.01,
                    "stepper rect within node: x={x} y={y} w={w} h={h}"
                );
            }
        }
    }

    #[test]
    fn pill_toggle_click_selects_segment_value() {
        let opt = |value: &str, label: &str| {
            let n = prop(node("option"), "value", Value::Text(value.into()));
            prop(n, "label", Value::Text(label.into()))
        };
        let mut pill = node("pill_toggle");
        pill.id = "pt".into();
        pill.bind = Some("mode".into());
        pill.width = Some(180.0);
        pill.height = Some(30.0);
        pill.anchor = Some(UiAnchor::TopLeft);
        pill = prop(pill, "style", Value::Text("pill".into()));
        // Options are CHILD data nodes (value+label), not placed sub-widgets.
        pill.children = vec![opt("low", "Low"), opt("med", "Med"), opt("high", "High")];

        let mut page = node("screen");
        page.children = vec![pill];

        let styles = serde_json::json!({
            "pill": {
                "bg": [0.05, 0.06, 0.08, 1.0], "border": [0.2, 0.2, 0.2, 1.0],
                "radius": 15, "pad": 3, "h": 30,
                "active_top": [0.14, 0.25, 0.47, 1.0], "active_bot": [0.10, 0.18, 0.34, 1.0],
                "active_label": [0.9, 0.9, 0.95, 1.0], "label": [0.5, 0.5, 0.5, 1.0], "label_size": 11
            }
        });
        let model = ValueMap::new().with("mode", "low");
        let mut state = UiState::new();

        // Pill at (0,0) 180×30. Inner strip x 3..177 (174 wide) → 3 cells of 58:
        // low 3..61, med 61..119, high 119..177. Click the middle cell → "med".
        let frame = run_ui(&page, &model, &styles, &input_at(90.0, 15.0, true), &mut state);
        assert_eq!(frame.results.text("mode"), Some("med"), "middle segment selects its value");
        assert!(frame.results.is_on("hud_hit"), "pointer over the pill claims the mouse");

        // Every drawn panel stays within the 180×30 node rect (well + highlight).
        for c in &frame.commands {
            if let HudCommand::Panel { x, y, w, h, .. } = c {
                assert!(
                    *x >= 0.0 && *y >= 0.0 && x + w <= 180.5 && y + h <= 30.5,
                    "pill panel within node rect, got {x},{y},{w},{h}"
                );
            }
        }

        // No click → echoes the current selection unchanged (two-way sync each frame).
        let frame = run_ui(&page, &model, &styles, &input_at(90.0, 15.0, false), &mut state);
        assert_eq!(frame.results.text("mode"), Some("low"), "non-click frame reports current value");

        // A click OUTSIDE every cell leaves the selection untouched.
        let frame = run_ui(&page, &model, &styles, &input_at(300.0, 15.0, true), &mut state);
        assert_eq!(frame.results.text("mode"), Some("low"), "a miss doesn't change the value");

        // A click on the well's PAD RIM (x=1 < the 3px pad, inside the well) claims
        // the pointer but lands in no segment — the selection stays put.
        let frame = run_ui(&page, &model, &styles, &input_at(1.0, 15.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"), "the well rim still claims");
        assert_eq!(frame.results.text("mode"), Some("low"), "rim click selects nothing");
    }

    #[test]
    fn tabs_click_selects_value_and_defaults_to_first() {
        // Three tabs (a|b|c) bound to "tab", a 300×30 strip at the origin: three
        // even 100px cells. Children are pure data carriers (value + label).
        let mk = |value: &str, label: &str| {
            let mut t = node("tab");
            t = prop(t, "value", Value::Text(value.into()));
            prop(t, "label", Value::Text(label.into()))
        };
        let mut tabs = node("tabs");
        tabs.id = "tabs".into();
        tabs.bind = Some("tab".into());
        tabs.width = Some(300.0);
        tabs.height = Some(30.0);
        tabs.anchor = Some(UiAnchor::TopLeft);
        tabs = prop(tabs, "tab_active", Value::Text("ta".into()));
        tabs = prop(tabs, "tab_idle", Value::Text("ti".into()));
        tabs.children = vec![mk("a", "A"), mk("b", "B"), mk("c", "C")];
        let mut page = node("tabs_page");
        page.children = vec![tabs];

        let st = serde_json::json!({
            "ta": { "fill_top": [0.2,0.3,0.5,1.0], "label": [1.0,1.0,1.0,1.0] },
            "ti": { "fill_top": [0.09,0.10,0.13,1.0], "label": [0.56,0.54,0.49,1.0] }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Click the middle cell (x 100..200) → selects its value "b".
        let frame = run_ui(&page, &model, &st, &input_at(150.0, 15.0, true), &mut state);
        assert_eq!(frame.results.text("tab"), Some("b"), "clicking tab 2 writes its value");
        assert!(frame.results.is_on("hud_hit"), "pointer over the strip claims the mouse");

        // No prior value + pointer off the strip → reports the first tab (a strip
        // always has one active tab), and claims nothing.
        let frame = run_ui(&page, &model, &st, &input_at(400.0, 400.0, false), &mut state);
        assert_eq!(frame.results.text("tab"), Some("a"), "unset bind defaults to the first tab");
        assert!(!frame.results.is_on("hud_hit"), "pointer off the strip doesn't claim the mouse");

        // Every drawn cell / label stays within the 300×30 strip rect.
        for c in &frame.commands {
            match c {
                HudCommand::Panel { x, y, w, h, .. } => assert!(
                    *x >= -0.01 && *y >= -0.01 && x + w <= 300.01 && y + h <= 30.01,
                    "tab cell within strip: x={x} y={y} w={w} h={h}"
                ),
                HudCommand::Text { x, y, .. } => assert!(
                    *x >= -0.01 && *x <= 300.01 && *y >= -0.01 && *y <= 30.01,
                    "tab label within strip: x={x} y={y}"
                ),
                _ => {}
            }
        }
    }

    #[test]
    fn tabs_gap_between_cells_claims_but_selects_nothing() {
        // A padded strip with a gap: pad_x 10, gap 20 over 320px → cells of
        // (300 − 40) / 3 = 86.67: a 20..40, b 40+66.67.. etc. Precisely: inner x
        // 10..310, cells at 10..96.67, 116.67..203.33, 223.33..310. A click in the
        // 96.67..116.67 gap (or the 0..10 pad) claims the strip but selects nothing.
        let mk = |value: &str, label: &str| {
            let t = node("tab");
            let t = prop(t, "value", Value::Text(value.into()));
            prop(t, "label", Value::Text(label.into()))
        };
        let mut tabs = node("tabs");
        tabs.id = "tabs".into();
        tabs.bind = Some("tab".into());
        tabs.width = Some(320.0);
        tabs.height = Some(30.0);
        tabs.anchor = Some(UiAnchor::TopLeft);
        tabs.gap = 20.0;
        tabs.pad_x = Some(10.0);
        tabs = prop(tabs, "tab_active", Value::Text("ta".into()));
        tabs = prop(tabs, "tab_idle", Value::Text("ti".into()));
        tabs.children = vec![mk("a", "A"), mk("b", "B"), mk("c", "C")];
        let mut page = node("tabs_page");
        page.children = vec![tabs];
        let st = serde_json::json!({
            "ta": { "fill_top": [0.2,0.3,0.5,1.0] },
            "ti": { "fill_top": [0.09,0.10,0.13,1.0] }
        });
        let model = ValueMap::new().with("tab", "c");
        let mut state = UiState::new();

        // Click in the gap between cell 1 and cell 2 (x ≈ 106).
        let frame = run_ui(&page, &model, &st, &input_at(106.0, 15.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"), "the strip claims between cells");
        assert_eq!(frame.results.text("tab"), Some("c"), "a gap click selects nothing");

        // Click inside cell 2 (x ≈ 160) → selects "b".
        let frame = run_ui(&page, &model, &st, &input_at(160.0, 15.0, true), &mut state);
        assert_eq!(frame.results.text("tab"), Some("b"), "cell click selects its value");
    }

    fn select_styles_json() -> Json {
        serde_json::json!({
            "controls": {
                "field": { "top": [0.0,0.0,0.0,1.0], "bot": [0.0,0.0,0.0,1.0], "border": [0.2,0.2,0.2,1.0], "radius": 3, "h": 40, "label": [1.0,1.0,1.0,1.0], "label_size": 15, "caret": [0.5,0.5,1.0,1.0] },
                "menu": { "top": [0.1,0.1,0.1,1.0], "bot": [0.0,0.0,0.0,1.0], "border": [0.2,0.2,0.2,1.0], "radius": 3, "row_h": 30, "label": [1.0,1.0,1.0,1.0], "label_size": 15, "sel_bg": [0.2,0.3,0.5,1.0], "sel_label": [1.0,1.0,1.0,1.0], "hover_bg": [0.1,0.15,0.25,1.0] }
            }
        })
    }

    // A select (bind "mode") over two option children. A field click opens the
    // popup (into `state.open`); an option click writes that option's `value` and
    // closes; the closed field's panel stays within the node rect.
    fn select_tree() -> UiNode {
        let opt = |val: &str, label: &str| {
            let mut n = node("option");
            n = prop(n, "value", Value::Text(val.into()));
            prop(n, "label", Value::Text(label.into()))
        };
        let mut sel = node("select");
        sel.id = "sel".into();
        sel.bind = Some("mode".into());
        sel.width = Some(200.0);
        sel.height = Some(40.0);
        sel.anchor = Some(UiAnchor::TopLeft);
        sel = prop(sel, "placeholder", Value::Text("Choose…".into()));
        sel = prop(sel, "style", Value::Text("controls".into()));
        sel.children = vec![opt("a", "Alpha"), opt("b", "Beta")];
        let mut page = node("screen");
        page.children = vec![sel];
        page
    }

    #[test]
    fn select_click_opens_then_option_click_writes_bind_and_closes() {
        let t = select_tree();
        let styles = select_styles_json();
        let model = ValueMap::new(); // no initial selection
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");

        // Closed: idle pointer far away. The field panel fills the node rect exactly
        // and is the ONLY panel (no popup rows drawn while closed).
        let f0 = run_ui_with(&t, &model, &styles, &input_at(400.0, 400.0, false), &mut state, Some(&lib));
        assert!(state.open.is_none(), "starts closed");
        let panels: Vec<_> = f0
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect();
        assert_eq!(panels, vec![(0.0, 0.0, 200.0, 40.0)], "closed = just the field panel, within the node rect");

        // Click the field (0..200 × 0..40) → opens into state.open, writes nothing yet.
        let f1 = run_ui_with(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state, Some(&lib));
        assert_eq!(state.open.as_deref(), Some("sel"), "clicking the field opens the menu");
        assert!(f1.results.is_on("hud_hit"), "the field claims the pointer");

        // Menu open (state persists). Rows start at y = 40 + 6 = 46, row_h 30:
        // row 0 = 46..76 (Alpha), row 1 = 76..106 (Beta). Click Beta.
        let f2 = run_ui_with(&t, &model, &styles, &input_at(100.0, 90.0, true), &mut state, Some(&lib));
        assert_eq!(f2.results.text("mode"), Some("b"), "clicking Beta writes its value");
        assert!(state.open.is_none(), "picking an option closes the menu");

        // A click outside a re-opened menu just closes it (writes nothing new).
        run_ui_with(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state, Some(&lib)); // re-open
        assert_eq!(state.open.as_deref(), Some("sel"));
        run_ui_with(&t, &model, &styles, &input_at(600.0, 500.0, true), &mut state, Some(&lib)); // click far outside
        assert!(state.open.is_none(), "a click outside closes the menu");
    }

    #[test]
    fn open_select_popup_outside_the_node_rect_still_claims_and_picks() {
        // The popup lies BELOW the select's own 200×40 rect (rows at y 46..106): a
        // naive rect pre-filter would drop it. Hovering a row must claim the pointer
        // (and keep claiming on an idle frame); clicking it must pick.
        let t = select_tree();
        let styles = select_styles_json();
        let model = ValueMap::new();
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");

        // Open via a field click.
        run_ui_with(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state, Some(&lib));
        assert_eq!(state.open.as_deref(), Some("sel"));

        // Hover row 1 (y=90 — outside the node rect): the popup claims. The open
        // owner stays a CANDIDATE despite the rect pre-filter, so the select still
        // receives the dispatch.
        let f = run_ui_with(&t, &model, &styles, &input_at(100.0, 90.0, false), &mut state, Some(&lib));
        assert!(f.results.is_on("hud_hit"), "the open popup claims outside the node rect");
        assert_eq!(f.stats.lua_hits, 1, "the open select received the hit dispatch");

        // Idle frame (nothing moved): the claim persists without re-dispatch.
        let f = run_ui_with(&t, &model, &styles, &input_at(100.0, 90.0, false), &mut state, Some(&lib));
        assert!(f.results.is_on("hud_hit"), "the claim survives an idle frame");
        assert_eq!(f.stats.lua_hits, 0, "…at zero Lua crossings");

        // Click the hovered row: Beta is picked and the menu closes.
        let f = run_ui_with(&t, &model, &styles, &input_at(100.0, 90.0, true), &mut state, Some(&lib));
        assert_eq!(f.stats.lua_hits, 1, "the click reached the select's M.hit");
        assert_eq!(f.results.text("mode"), Some("b"), "the row outside the rect picks");
        assert!(state.open.is_none(), "picking closes the menu");
    }

    #[test]
    fn select_open_menu_rows_are_lifted_above_the_field() {
        let t = select_tree();
        let styles = select_styles_json();
        let model = ValueMap::new().with("mode", "a");
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        // Force it open, then draw: the field is layer 0, the popup panel + rows layer 1.
        run_ui_with(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state, Some(&lib));
        assert_eq!(state.open.as_deref(), Some("sel"));
        let frame = run_ui_with(&t, &model, &styles, &input_at(0.0, 0.0, false), &mut state, Some(&lib));
        // First panel = field (layer 0); a later panel = the popup (layer 1).
        let panel_layers: Vec<f32> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { layer, .. } => Some(*layer),
                _ => None,
            })
            .collect();
        assert_eq!(panel_layers.first(), Some(&0.0), "field sits on the base layer");
        assert!(panel_layers.contains(&1.0), "popup panel is lifted a sub-layer");
    }

    #[test]
    fn text_field_focus_type_backspace_and_click_away() {
        // A single text_field (bind "name") anchored at (10,10), 200×40.
        let mut tf = node("text_field");
        tf.id = "name_field".into();
        tf.width = Some(200.0);
        tf.height = Some(40.0);
        tf.anchor = Some(UiAnchor::TopLeft);
        tf.offset = [10.0, 10.0];
        tf.bind = Some("name".into());
        tf = prop(tf, "placeholder", Value::Text("enter name".into()));
        tf = prop(tf, "style", Value::Text("field".into()));
        let mut page = node("screen");
        page.children = vec![tf];

        let styles = serde_json::json!({
            "field": {
                "top": [0.02, 0.02, 0.03, 1.0], "bot": [0.04, 0.04, 0.05, 1.0],
                "border": [0.2, 0.2, 0.2, 1.0], "hover_border": [0.5, 0.4, 0.2, 1.0],
                "caret": [0.43, 0.59, 1.0, 1.0], "radius": 3, "h": 40,
                "label": [0.9, 0.9, 0.85, 1.0], "label_size": 15
            }
        });
        let mut state = UiState::new();

        // Click inside the well → focuses the field and claims the mouse.
        let model = ValueMap::new().with("name", "");
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 30.0, true), &mut state);
        assert!(f.results.is_on("hud_hit"), "a click in the well claims the mouse");
        assert_eq!(state.focus.as_deref(), Some("name_field"), "click focuses the field");

        // Type two chars on a non-click frame → appended to the bound string. Feed
        // the prior frame's result back as the model, as the engine would.
        let model = ValueMap::new().with("name", f.results.text("name").unwrap_or("").to_string());
        let mut typing = input_at(100.0, 30.0, false);
        typing.typed = "Hi".into();
        let f = run_ui(&page, &model, &styles, &typing, &mut state);
        assert_eq!(f.results.text("name"), Some("Hi"), "typed chars append to the value");

        // Backspace → pops the last char.
        let model = ValueMap::new().with("name", f.results.text("name").unwrap().to_string());
        let mut bs = input_at(100.0, 30.0, false);
        bs.backspace = true;
        let f = run_ui(&page, &model, &styles, &bs, &mut state);
        assert_eq!(f.results.text("name"), Some("H"), "backspace pops the last char");

        // Well geometry stays within the node rect (10,10 .. 210,50), and while
        // focused a caret rect is emitted inside it.
        let well = f.commands.iter().find_map(|c| match c {
            HudCommand::Panel { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
            _ => None,
        }).expect("well drawn");
        assert!(well.0 >= 10.0 && well.1 >= 10.0 && well.0 + well.2 <= 210.0 && well.1 + well.3 <= 50.0, "well within node rect: {well:?}");
        // The caret is a MEASURED command: the walker emits the buffer as `prefix`
        // and the render bridge shapes it — never a char-count estimate.
        let caret = f.commands.iter().find_map(|c| match c {
            HudCommand::TextCaret { x, prefix, max_x, .. } => Some((*x, prefix.clone(), *max_x)),
            _ => None,
        }).expect("a measured caret is emitted while focused");
        assert!(caret.0 >= 10.0 && caret.2 <= 210.0, "caret anchors + clamps inside the well: {caret:?}");
        assert_eq!(caret.1, "H", "the caret prefix is the buffer to measure");

        // Click OUTSIDE the well → de-focuses; the value survives.
        let model = ValueMap::new().with("name", f.results.text("name").unwrap().to_string());
        let f = run_ui(&page, &model, &styles, &input_at(400.0, 300.0, true), &mut state);
        assert!(state.focus.is_none(), "clicking away clears focus");
        assert_eq!(f.results.text("name"), Some("H"), "value preserved on click-away");

        // A keystroke while unfocused is ignored.
        let model = ValueMap::new().with("name", "H");
        let mut typing = input_at(400.0, 300.0, false);
        typing.typed = "X".into();
        let f = run_ui(&page, &model, &styles, &typing, &mut state);
        assert_eq!(f.results.text("name"), Some("H"), "typing is ignored when unfocused");
    }

    // ── text_field behaviour (S6) ────────────────────────────────────────────
    //
    // The per-facet pins around the oracle test above: placeholder / borders /
    // caret / the typed-fold's frame discipline. Written against the Rust arms
    // and held green through the Lua port — they are the port's contract.

    /// One text_field (bind "name", id "name_field") at (10,10) 200×40 under a screen.
    fn text_field_tree() -> UiNode {
        let mut tf = node("text_field");
        tf.id = "name_field".into();
        tf.width = Some(200.0);
        tf.height = Some(40.0);
        tf.anchor = Some(UiAnchor::TopLeft);
        tf.offset = [10.0, 10.0];
        tf.bind = Some("name".into());
        tf = prop(tf, "placeholder", Value::Text("enter name".into()));
        tf = prop(tf, "style", Value::Text("field".into()));
        let mut page = node("screen");
        page.children = vec![tf];
        page
    }

    /// The full field style block — every colour the arm reads, so each state's
    /// pick is distinguishable (placeholder ≠ label, hover_border ≠ border ≠ caret).
    fn text_field_styles() -> Json {
        serde_json::json!({
            "field": {
                "top": [0.02, 0.02, 0.03, 1.0], "bot": [0.04, 0.04, 0.05, 1.0],
                "border": [0.2, 0.2, 0.2, 1.0], "hover_border": [0.5, 0.4, 0.2, 1.0],
                "caret": [0.43, 0.59, 1.0, 1.0], "radius": 3,
                "label": [0.9, 0.9, 0.85, 1.0], "label_size": 15,
                "placeholder": [0.35, 0.35, 0.3, 1.0]
            }
        })
    }

    #[test]
    fn text_field_placeholder_when_empty_is_dim_and_resolved() {
        // An empty value shows the placeholder: stringtable-RESOLVED (it is display
        // text, not user data) and in the dim placeholder colour; a non-empty value
        // shows the value in the label colour and no placeholder.
        let _g = crate::strings::test_guard();
        crate::strings::load_str(r#"{ "tf_hint": { "en-us": "enter name" } }"#, "en-us");
        let mut page = text_field_tree();
        page.children[0].props.insert("placeholder".into(), Value::Text("$tf_hint".into()));
        let styles = text_field_styles();

        let f = run_ui(&page, &ValueMap::new().with("name", ""), &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        let (text, color) = f
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Text { text, color, .. } => Some((text.clone(), *color)),
                _ => None,
            })
            .expect("the empty field drew its placeholder");
        assert_eq!(text, "enter name", "the $token placeholder resolved");
        assert_eq!(color, [0.35, 0.35, 0.3, 1.0], "placeholder uses the dim placeholder colour");

        let f = run_ui(&page, &ValueMap::new().with("name", "Ada"), &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        let texts: Vec<(String, [f32; 4])> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { text, color, .. } => Some((text.clone(), *color)),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 1, "a valued field draws the value only: {texts:?}");
        assert_eq!(texts[0].0, "Ada", "the bound value is shown");
        assert_eq!(texts[0].1, [0.9, 0.9, 0.85, 1.0], "the value uses the label colour");
    }

    #[test]
    fn text_field_border_tracks_rest_hover_and_focus() {
        // The well's border is a state channel: resting edge (1px `border`), bronze
        // hover edge (1px `hover_border`), rune-light focus ring (2px `caret`).
        let page = text_field_tree();
        let styles = text_field_styles();
        let model = ValueMap::new().with("name", "");
        let well = |f: &UiFrame| {
            f.commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Panel { border, border_color, .. } => Some((*border, *border_color)),
                    _ => None,
                })
                .expect("the well drew")
        };

        // Resting: pointer far away, no focus.
        let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        assert_eq!(well(&f), (1.0, [0.2, 0.2, 0.2, 1.0]), "resting edge");

        // Hovered (no click): pointer in the well.
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 30.0, false), &mut UiState::new());
        assert_eq!(well(&f), (1.0, [0.5, 0.4, 0.2, 1.0]), "bronze hover edge");

        // Focused (click in the well): the 2px rune-light ring wins over hover.
        let mut state = UiState::new();
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 30.0, true), &mut state);
        assert_eq!(state.focused(), Some("name_field"));
        assert_eq!(well(&f), (2.0, [0.43, 0.59, 1.0, 1.0]), "focus ring");
    }

    #[test]
    fn text_field_caret_only_while_focused_with_measured_prefix() {
        // Unfocused — even valued and hovered — no caret; focused: ONE caret whose
        // `prefix` is the whole buffer (the render bridge measures the shaped string)
        // and whose right clamp keeps it inside the well.
        let page = text_field_tree();
        let styles = text_field_styles();
        let model = ValueMap::new().with("name", "Ada");

        let f = run_ui(&page, &model, &styles, &input_at(100.0, 30.0, false), &mut UiState::new());
        assert!(
            !f.commands.iter().any(|c| matches!(c, HudCommand::TextCaret { .. })),
            "no caret while unfocused"
        );

        let mut state = UiState::new();
        state.request_focus("name_field");
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 30.0, false), &mut state);
        let carets: Vec<_> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::TextCaret { x, y, w, h, prefix, size, color, max_x, .. } => {
                    Some((*x, *y, *w, *h, prefix.clone(), *size, *color, *max_x))
                }
                _ => None,
            })
            .collect();
        assert_eq!(carets.len(), 1, "focused field emits exactly one caret");
        let (x, y, w, h, prefix, size, color, max_x) = carets[0].clone();
        assert_eq!(x, 18.0, "caret anchors at the text origin (r.x + text pad)");
        assert_eq!(y, 22.5, "vertically centred with the 15px line");
        assert_eq!((w, h, size), (2.0, 15.0, 15.0));
        assert_eq!(prefix, "Ada", "the prefix is the WHOLE buffer to measure");
        assert_eq!(color, [0.43, 0.59, 1.0, 1.0], "caret colour from the style");
        assert_eq!(max_x, 200.0, "clamped to the well's right text edge (210 - pad 8 - w 2)");
    }

    #[test]
    fn typing_with_the_pointer_parked_folds_in_rust_and_redraws_the_field() {
        let _g = crate::strings::test_guard();
        // Keyboard ≠ pointer: a typing frame with a parked pointer is NOT
        // input-active, so it crosses into Lua for zero HIT calls — the fold is
        // Rust — yet the value updates and the field (alone) redraws.
        let page = text_field_tree();
        let styles = text_field_styles();
        let mut state = UiState::new();

        // Click in the well (focus), then a neutral frame to absorb the release edge.
        let f = run_ui(&page, &ValueMap::new().with("name", ""), &styles, &input_at(100.0, 30.0, true), &mut state);
        assert_eq!(state.focused(), Some("name_field"));
        let model = ValueMap::new().with("name", f.results.text("name").unwrap_or("").to_string());
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 30.0, false), &mut state);
        let model = ValueMap::new().with("name", f.results.text("name").unwrap_or("").to_string());

        // Typing frame: pointer/buttons identical to the previous frame.
        let mut typing = input_at(100.0, 30.0, false);
        typing.typed = "Q".into();
        let f = run_ui(&page, &model, &styles, &typing, &mut state);
        assert_eq!(f.results.text("name"), Some("Q"), "the parked-pointer frame still folds");
        assert_eq!(f.stats.lua_hits, 0, "the fold is Rust — no hit crossing");
        assert_eq!(f.stats.redraw_nodes, 1, "exactly the field redraws for the new value");
        assert_eq!(f.stats.lua_draws, 1, "…one Lua draw crossing: the field's own");
        assert!(f.results.is_on("hud_hit"), "the resting claim survives via the memo");
    }

    #[test]
    fn text_field_backspace_pops_a_whole_char() {
        // The buffer is Unicode: one backspace removes one CHARACTER (however many
        // bytes), never a stray byte of a multibyte glyph.
        let page = text_field_tree();
        let styles = text_field_styles();
        let mut state = UiState::new();

        run_ui(&page, &ValueMap::new().with("name", ""), &styles, &input_at(100.0, 30.0, true), &mut state);
        let mut typing = input_at(100.0, 30.0, false);
        typing.typed = "é⬥".into();
        let f = run_ui(&page, &ValueMap::new().with("name", ""), &styles, &typing, &mut state);
        assert_eq!(f.results.text("name"), Some("é⬥"));

        let model = ValueMap::new().with("name", f.results.text("name").unwrap().to_string());
        let mut bs = input_at(100.0, 30.0, false);
        bs.backspace = true;
        let f = run_ui(&page, &model, &styles, &bs, &mut state);
        assert_eq!(f.results.text("name"), Some("é"), "the whole ⬥ popped");
    }

    #[test]
    fn focus_change_under_a_stationary_pointer_redraws_the_field() {
        let _g = crate::strings::test_guard();
        // With the pointer parked INSIDE the well, `hot` (hover ∪ focus) never
        // changes — the fingerprint's dedicated focus bit is what makes the
        // ring/caret appear and disappear instead of replaying stale commands.
        let page = text_field_tree();
        let styles = text_field_styles();
        let model = ValueMap::new().with("name", "Ada");
        let mut state = UiState::new();
        let over = input_at(100.0, 30.0, false);
        let has_caret =
            |f: &UiFrame| f.commands.iter().any(|c| matches!(c, HudCommand::TextCaret { .. }));

        let f = run_ui(&page, &model, &styles, &over, &mut state);
        assert!(!has_caret(&f), "unfocused: no caret");

        state.request_focus("name_field");
        let f = run_ui(&page, &model, &styles, &over, &mut state);
        assert!(has_caret(&f), "gaining focus under a parked pointer draws the caret");
        assert_eq!(f.stats.redraw_nodes, 1, "…by redrawing the field, not replaying");

        state.clear_focus();
        let f = run_ui(&page, &model, &styles, &over, &mut state);
        assert!(!has_caret(&f), "losing focus under a parked pointer removes the caret");
        assert_eq!(f.stats.redraw_nodes, 1);
    }

    // (The transient S6 parity test — `draw_text_field` vs the Lua module, byte for
    // byte across placeholder/valued/hovered/focused/focused-away states — gated the
    // port and was deleted with the Rust arm; the byte-pinned draw below and the
    // behaviour tests above carry the regression duty.)
    #[test]
    fn text_field_lua_draw_is_byte_pinned() {
        // Both draw branches at the byte level, outliving the deleted Rust arm:
        // a focused, valued field (well + label-coloured value + measured caret)
        // and an empty resting field (well + dim resolved placeholder, no caret).
        let page = text_field_tree();
        let styles = text_field_styles();
        let text = |s: &str, color: [f32; 4]| HudCommand::Text {
            x: 18.0,
            y: 22.5,
            text: s.to_string(),
            size: 15.0,
            color,
            layer: 0.0,
            align: TextAlign::Left,
            font: FontRole::Body,
            italic: false,
            bold: false,
            tracking: -1.0,
            wrap: None,
        };
        let well = |border: f32, border_color: [f32; 4]| HudCommand::Panel {
            x: 10.0,
            y: 10.0,
            w: 200.0,
            h: 40.0,
            color: [0.02, 0.02, 0.03, 1.0],
            color2: [0.04, 0.04, 0.05, 1.0],
            grad: 1.0,
            radius: 3.0,
            border,
            border_color,
            feather: 0.0,
            layer: 0.0,
        };

        // Focused + valued: click in the well — the same frame draws the ring + caret.
        let f = run_ui(&page, &ValueMap::new().with("name", "Ada"), &styles, &input_at(100.0, 30.0, true), &mut UiState::new());
        let expected = vec![
            well(2.0, [0.43, 0.59, 1.0, 1.0]),
            text("Ada", [0.9, 0.9, 0.85, 1.0]),
            HudCommand::TextCaret {
                x: 18.0,
                y: 22.5,
                w: 2.0,
                h: 15.0,
                prefix: "Ada".to_string(),
                size: 15.0,
                color: [0.43, 0.59, 1.0, 1.0],
                layer: 0.0,
                font: FontRole::Body,
                max_x: 200.0,
            },
        ];
        assert_eq!(f.commands, expected, "the focused draw is byte-stable");

        // Empty + resting: the placeholder branch, no caret.
        let f = run_ui(&page, &ValueMap::new().with("name", ""), &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        let expected = vec![
            well(1.0, [0.2, 0.2, 0.2, 1.0]),
            text("enter name", [0.35, 0.35, 0.3, 1.0]),
        ];
        assert_eq!(f.commands, expected, "the placeholder draw is byte-stable");
    }

    /// A button's caption resolves through the same `text_bind` channel every other text-bearing
    /// node uses, so an exclusive choice or a state-dependent action can label itself from the
    /// Model instead of needing one node per possible caption.
    #[test]
    fn button_label_can_come_from_the_model() {
        let mut b = node("button");
        b.id = "b".into();
        b.size = Some(24.0);
        b = prop(b, "text_bind", Value::Text("caption".into()));
        b = prop(b, "label", Value::Text("FALLBACK".into()));
        b = prop(b, "style", Value::Text("btn".into()));

        let model = ValueMap::new().with("caption", "\u{25c9}  Skin");
        let mut state = UiState::new();
        // Buttons DRAW via the Lua component now, so give the walker the component library.
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("button component library");
        let f =
            run_ui_with(&b, &model, &styles(), &input_at(-9.0, -9.0, false), &mut state, Some(&lib));
        let drew = f.commands.iter().any(
            |c| matches!(c, HudCommand::Text { text, .. } if text.contains("Skin")),
        );
        assert!(drew, "the bound caption reached the draw commands: {:?}", f.commands);

        // With no bind, the literal label still wins — existing buttons are unaffected.
        let plain = prop(node("button"), "label", Value::Text("GO".into()));
        let f = run_ui_with(
            &plain,
            &ValueMap::new(),
            &styles(),
            &input_at(-9.0, -9.0, false),
            &mut state,
            Some(&lib),
        );
        assert!(f
            .commands
            .iter()
            .any(|c| matches!(c, HudCommand::Text { text, .. } if text == "GO")));
    }

    /// A checkbox always draws its box; the inset `check` tick appears ONLY when its
    /// bound value is true — dispatched to `ui/checkbox.lua` (box = 1 panel, box + tick =
    /// 2). Also proves `bind_value` + the merged node props (`box`/`label`) cross.
    #[test]
    fn checkbox_lua_ticks_when_bound_true() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let styles = serde_json::json!({
            "cb": { "box": [0.1, 0.1, 0.1, 1.0], "check": [0.2, 0.9, 0.3, 1.0],
                    "border": [0.3, 0.3, 0.3, 1.0], "pad": 3 }
        });
        let mut cb = node("checkbox");
        cb.bind = Some("flag".into());
        cb.anchor = Some(UiAnchor::TopLeft);
        cb.offset = [10.0, 10.0];
        cb.width = Some(120.0);
        cb.height = Some(20.0);
        cb = prop(cb, "box", Value::Number(14.0));
        cb = prop(cb, "label", Value::Text("Enable".into()));
        cb = prop(cb, "style", Value::Text("cb".into()));
        let mut page = node("screen");
        page.children = vec![cb];

        let panels = |checked: bool| {
            let model = ValueMap::new().with("flag", checked);
            run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib))
                .commands
                .iter()
                .filter(|c| matches!(c, HudCommand::Panel { .. }))
                .count()
        };
        assert_eq!(panels(false), 1, "unchecked: just the box");
        assert_eq!(panels(true), 2, "checked: box + tick");

        // The row label (a merged node prop) reaches the Lua component.
        let model = ValueMap::new().with("flag", false);
        let f = run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib));
        assert!(f.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "Enable")));
    }

    /// A toggle's knob sits at the RIGHT of the pill when its bound value is true, at the
    /// LEFT when false — dispatched to `ui/toggle.lua`.
    #[test]
    fn toggle_lua_knob_shifts_with_bound_value() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let styles = serde_json::json!({
            "tg": { "w": 50, "h": 24, "on_top": [0.1, 0.3, 0.6, 1.0], "off_bg": [0.1, 0.1, 0.1, 1.0],
                    "knob_on": [0.9, 0.9, 1.0, 1.0], "knob_off": [0.5, 0.5, 0.5, 1.0] }
        });
        let mut tg = node("toggle");
        tg.bind = Some("on".into());
        tg.anchor = Some(UiAnchor::TopLeft);
        tg.width = Some(60.0);
        tg.height = Some(24.0);
        tg = prop(tg, "style", Value::Text("tg".into()));
        let mut page = node("screen");
        page.children = vec![tg];
        // The knob is the small SQUARE panel (w == h, smaller than the 24px pill height).
        let knob_x = |on: bool| {
            let model = ValueMap::new().with("on", on);
            run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib))
                .commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Panel { x, w, h, .. } if (w - h).abs() < 0.5 && *w < 24.0 => Some(*x),
                    _ => None,
                })
                .expect("toggle draws a knob")
        };
        assert!(knob_x(true) > knob_x(false), "the knob shifts right when the toggle is on");
    }

    /// A tile fills from `style` when its enabled binding is loaded, and from `style_off`
    /// when it is not — dispatched to `ui/tile.lua` (proves `enabled` + `style_off` cross).
    #[test]
    fn tile_lua_swaps_style_when_not_loaded() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let styles = serde_json::json!({
            "on":  { "cell": [0.2, 0.2, 0.2, 1.0] },
            "off": { "cell": [0.9, 0.0, 0.0, 1.0] }
        });
        let mut tile = node("tile");
        tile.bind = Some("sel".into());
        tile.enabled_bind = Some("loaded".into());
        tile.anchor = Some(UiAnchor::TopLeft);
        tile.width = Some(40.0);
        tile.height = Some(40.0);
        tile = prop(tile, "style", Value::Text("on".into()));
        tile = prop(tile, "style_off", Value::Text("off".into()));
        let mut page = node("screen");
        page.children = vec![tile];
        let fill = |loaded: bool| {
            let model = ValueMap::new().with("loaded", loaded).with("sel", false);
            run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib))
                .commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Panel { color, .. } => Some(*color),
                    _ => None,
                })
                .expect("tile draws a fill panel")
        };
        assert_eq!(fill(true), [0.2, 0.2, 0.2, 1.0], "loaded uses `style`");
        assert_eq!(fill(false), [0.9, 0.0, 0.0, 1.0], "unloaded uses `style_off`");
    }

    /// A pill_toggle draws its well, plus a highlight panel on the segment whose child
    /// `value` equals the bound selection — dispatched to `ui/pill_toggle.lua` (proves the
    /// `children` list crosses).
    #[test]
    fn pill_toggle_lua_lights_the_selected_segment() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let styles = serde_json::json!({
            "pt": { "bg": [0.1, 0.1, 0.1, 1.0], "active_top": [0.2, 0.4, 0.7, 1.0],
                    "label": [0.6, 0.6, 0.6, 1.0], "active_label": [1.0, 1.0, 1.0, 1.0] }
        });
        let seg = |value: &str, label: &str| {
            let n = prop(node("cell"), "value", Value::Text(value.into()));
            prop(n, "label", Value::Text(label.into()))
        };
        let mut pt = node("pill_toggle");
        pt.bind = Some("mode".into());
        pt.anchor = Some(UiAnchor::TopLeft);
        pt.width = Some(160.0);
        pt.height = Some(28.0);
        pt = prop(pt, "style", Value::Text("pt".into()));
        pt.children = vec![seg("walk", "Walk"), seg("run", "Run")];
        let mut page = node("screen");
        page.children = vec![pt];

        let panels = |selected: &str| {
            let model = ValueMap::new().with("mode", selected);
            run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib))
                .commands
                .iter()
                .filter(|c| matches!(c, HudCommand::Panel { .. }))
                .count()
        };
        assert_eq!(panels("none"), 1, "no active segment: just the well");
        assert_eq!(panels("run"), 2, "the selected segment adds a highlight panel");
    }

    /// A tab strip styles the cell whose child `value` == the bound selection from
    /// `tab_active`, the rest from `tab_idle` — dispatched to `ui/tabs.lua` (proves the
    /// resolved `tab_active`/`tab_idle` blocks + the `children` list cross).
    #[test]
    fn tabs_lua_styles_the_selected_tab_active() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let styles = serde_json::json!({
            "active": { "fill_top": [0.2, 0.4, 0.7, 1.0], "label": [1.0, 1.0, 1.0, 1.0] },
            "idle":   { "fill_top": [0.1, 0.1, 0.1, 1.0], "label": [0.5, 0.5, 0.5, 1.0] }
        });
        let tab = |value: &str, label: &str| {
            let n = prop(node("cell"), "value", Value::Text(value.into()));
            prop(n, "label", Value::Text(label.into()))
        };
        let mut strip = node("tabs");
        strip.bind = Some("sel".into());
        strip.anchor = Some(UiAnchor::TopLeft);
        strip.width = Some(200.0);
        strip.height = Some(30.0);
        strip = prop(strip, "tab_active", Value::Text("active".into()));
        strip = prop(strip, "tab_idle", Value::Text("idle".into()));
        strip.children = vec![tab("a", "A"), tab("b", "B")];
        let mut page = node("screen");
        page.children = vec![strip];

        let model = ValueMap::new().with("sel", "a");
        let f = run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib));
        let fills: Vec<[f32; 4]> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(fills.len(), 2, "one cell panel per tab (no strip style)");
        assert_eq!(fills[0], [0.2, 0.4, 0.7, 1.0], "the selected tab 'a' uses the active fill");
        assert_eq!(fills[1], [0.1, 0.1, 0.1, 1.0], "the unselected tab 'b' uses the idle fill");
    }

    /// A slider's fill rect spans the track proportionally to its bound value (mapped
    /// through min/max) — dispatched to `ui/slider.lua`.
    #[test]
    fn slider_lua_fills_to_the_bound_value() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let styles = serde_json::json!({
            "sl": { "track": [0.1, 0.1, 0.1, 1.0], "fill": [0.2, 0.4, 0.7, 1.0], "handle": [0.9, 0.9, 1.0, 1.0] }
        });
        let mut sl = node("slider");
        sl.bind = Some("v".into());
        sl.anchor = Some(UiAnchor::TopLeft);
        sl.width = Some(100.0);
        sl.height = Some(20.0);
        sl = prop(sl, "min", Value::Number(0.0));
        sl = prop(sl, "max", Value::Number(100.0));
        sl = prop(sl, "style", Value::Text("sl".into()));
        let mut page = node("screen");
        page.children = vec![sl];
        // Rects, in order: track (full width 100), fill (value-scaled), handle.
        let fill_w = |v: f64| {
            let model = ValueMap::new().with("v", v);
            let cmds = run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib)).commands;
            let widths: Vec<f32> = cmds.iter().filter_map(|c| match c {
                HudCommand::Rect { w, .. } => Some(*w),
                _ => None,
            }).collect();
            widths[1]
        };
        assert_eq!(fill_w(0.0), 0.0, "value 0 → empty fill");
        assert_eq!(fill_w(50.0), 50.0, "value 50 of 100 → half of the 100px track");
    }

    /// A stepper draws its field + two end buttons (3 rects) and the value formatted with
    /// `decimals`/`suffix` — dispatched to `ui/stepper.lua`.
    #[test]
    fn stepper_lua_draws_field_buttons_and_formatted_value() {
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let styles = serde_json::json!({
            "st": { "field": [0.1, 0.1, 0.1, 1.0], "btn": [0.2, 0.2, 0.2, 1.0], "label": [1.0, 1.0, 1.0, 1.0] }
        });
        let mut sp = node("stepper");
        sp.bind = Some("n".into());
        sp.anchor = Some(UiAnchor::TopLeft);
        sp.width = Some(120.0);
        sp.height = Some(24.0);
        sp = prop(sp, "decimals", Value::Number(0.0));
        sp = prop(sp, "suffix", Value::Text(" fps".into()));
        sp = prop(sp, "style", Value::Text("st".into()));
        let mut page = node("screen");
        page.children = vec![sp];
        let model = ValueMap::new().with("n", 60.0);
        let f = run_ui_with(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new(), Some(&lib));
        let rects = f.commands.iter().filter(|c| matches!(c, HudCommand::Rect { .. })).count();
        assert_eq!(rects, 3, "field background + two end buttons");
        assert!(
            f.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "60 fps")),
            "the value renders formatted with decimals + suffix"
        );
    }

    /// `color_bind` names a Model key holding a dotted style path, so a row whose STATE decides
    /// its colour resolves through one node rather than one node per possible colour.
    #[test]
    fn text_colour_can_follow_a_bound_style_path() {
        let styles = serde_json::json!({
            "map": { "ok": [0.0, 1.0, 0.0, 1.0], "review": [1.0, 1.0, 0.0, 1.0] }
        });
        let mut t = node("text");
        t.size = Some(20.0);
        t = prop(t, "text", Value::Text("thigh_l".into()));
        t = prop(t, "color_bind", Value::Text("row_color".into()));

        let mut state = UiState::new();
        let green = ValueMap::new().with("row_color", "map.ok");
        let f = run_ui(&t, &green, &styles, &input_at(-9.0, -9.0, false), &mut state);
        let color = f.commands.iter().find_map(|c| match c {
            HudCommand::Text { color, .. } => Some(*color),
            _ => None,
        });
        assert_eq!(color, Some([0.0, 1.0, 0.0, 1.0]), "resolved through the bound path");

        // The SAME node, a different bound path → a different colour. One node, N states.
        let amber = ValueMap::new().with("row_color", "map.review");
        let f = run_ui(&t, &amber, &styles, &input_at(-9.0, -9.0, false), &mut state);
        let color = f.commands.iter().find_map(|c| match c {
            HudCommand::Text { color, .. } => Some(*color),
            _ => None,
        });
        assert_eq!(color, Some([1.0, 1.0, 0.0, 1.0]));
    }

    /// A node's whole `style` can ride a bound Model path (`style_bind`), so ONE panel switches
    /// between an active and an idle look from state — how the non-interactive pipeline tabs light
    /// the current step without a stack of visibility-toggled panels.
    #[test]
    fn panel_style_can_follow_a_bound_path() {
        let styles = serde_json::json!({
            "tab_active": { "fill_top": [0.1, 0.2, 0.4, 1.0], "fill_bot": [0.1, 0.2, 0.4, 1.0] },
            "tab_idle":   { "fill_top": [0.2, 0.2, 0.2, 1.0], "fill_bot": [0.2, 0.2, 0.2, 1.0] }
        });
        let mut t = node("cell");
        t.width = Some(80.0);
        t.height = Some(24.0);
        t.anchor = Some(UiAnchor::TopLeft);
        t = prop(t, "style_bind", Value::Text("tab_style".into()));
        // A literal fallback the bind overrides — proves the bind wins when the key is present.
        t = prop(t, "style", Value::Text("tab_idle".into()));

        let mut state = UiState::new();
        let panel_fill = |f: &UiFrame| {
            f.commands.iter().find_map(|c| match c {
                HudCommand::Panel { color, .. } => Some(*color),
                _ => None,
            })
        };

        let active = ValueMap::new().with("tab_style", "tab_active");
        let f = run_ui(&t, &active, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(panel_fill(&f), Some([0.1, 0.2, 0.4, 1.0]), "bound path selects the active style");

        // The SAME node, a different bound value → the idle style. One node, N states.
        let idle = ValueMap::new().with("tab_style", "tab_idle");
        let f = run_ui(&t, &idle, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(panel_fill(&f), Some([0.2, 0.2, 0.2, 1.0]), "a different bound value → idle");

        // With the bound key absent, the literal `style` fallback still draws — existing nodes are
        // unaffected by the new channel.
        let f = run_ui(&t, &ValueMap::new(), &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(panel_fill(&f), Some([0.2, 0.2, 0.2, 1.0]), "no bound value → literal style");
    }

    #[test]
    fn drag_source_picks_up_payload_and_reports_drop() {
        // Any component kind can be a drag source — it is prop-driven, not a new kind.
        let mut row = node("cell");
        row.id = "row".into();
        row = prop(row, "drag_kind", Value::Text("clip".into()));
        row = prop(row, "drag_id", Value::Text("walk_forward".into()));

        let model = ValueMap::new();
        let screen = Vec2::new(200.0, 100.0);
        let mut state = UiState::new();

        // Press inside the source → payload picked up, active while held.
        let press = UiInput { mouse: Vec2::new(50.0, 50.0), clicked: true, down: true, screen, typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui(&row, &model, &styles(), &press, &mut state);
        assert_eq!(f.results.text("drag_kind"), Some("clip"));
        assert_eq!(f.results.text("drag_id"), Some("walk_forward"));
        assert!(f.results.is_on("drag_active"), "held drag is active");
        assert!(!f.results.is_on("drag_dropped"), "not dropped while still held");
        assert_eq!(state.drag().map(|d| d.id.as_str()), Some("walk_forward"));

        // Still held, cursor moved — the payload is retained across frames.
        let hold = UiInput { mouse: Vec2::new(180.0, 90.0), clicked: false, down: true, screen, typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui(&row, &model, &styles(), &hold, &mut state);
        assert!(f.results.is_on("drag_active"));
        assert_eq!(f.results.text("drag_id"), Some("walk_forward"));

        // Release → exactly one drop edge carrying the payload, then the drag clears.
        let release = UiInput { mouse: Vec2::new(180.0, 90.0), clicked: false, down: false, screen, typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui(&row, &model, &styles(), &release, &mut state);
        assert!(f.results.is_on("drag_dropped"), "release reports the drop");
        assert_eq!(f.results.text("drag_id"), Some("walk_forward"), "drop carries the payload");
        assert!(state.drag().is_none(), "drag clears after the drop");

        // A node without `drag_kind` never picks anything up.
        let mut plain = UiState::new();
        let f = run_ui(&node("cell"), &model, &styles(), &press, &mut plain);
        assert!(f.results.text("drag_kind").is_none());
        assert!(plain.drag().is_none());
    }

    #[test]
    fn slider_drag_writes_bound_value_and_captures() {
        let mut sl = node("slider");
        sl.id = "s".into();
        sl.bind = Some("v".into());
        sl.width = Some(200.0);
        sl.height = Some(20.0);
        sl.anchor = Some(UiAnchor::TopLeft);
        sl = prop(sl, "slider_h", Value::Number(12.0));
        sl = prop(sl, "min", Value::Number(0.0));
        sl = prop(sl, "max", Value::Number(100.0));
        let mut page = node("screen");
        page.children = vec![sl];

        let st = serde_json::json!({});
        let model = ValueMap::new().with("v", 0.0);
        let mut state = UiState::new();
        // Track spans the full 200px width from x=0; press at the midpoint.
        let frame = run_ui(&page, &model, &st, &input_at(100.0, 10.0, true), &mut state);
        let v = frame.results.number("v").expect("slider wrote its bind");
        assert!((v - 50.0).abs() < 2.0, "midpoint press ≈ 50, got {v}");
        assert!(frame.results.is_on("hud_hit"));

        // Still held, cursor moved right → keeps updating even off-track.
        let held = UiInput { mouse: Vec2::new(180.0, 10.0), clicked: false, down: true, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let frame = run_ui(&page, &model, &st, &held, &mut state);
        assert!(frame.results.number("v").unwrap() > 80.0, "drag keeps writing");
    }

    /// A labelled slider row (label column + track + value column, like the fit
    /// gadget's): the whole ROW claims and focuses, but only the padded GRAB band
    /// (track ±6px) captures a drag.
    #[test]
    fn slider_row_focuses_but_only_the_grab_band_captures() {
        let mut sl = node("slider");
        sl.id = "s".into();
        sl.bind = Some("v".into());
        sl.width = Some(300.0);
        sl.height = Some(40.0);
        sl.anchor = Some(UiAnchor::TopLeft);
        sl = prop(sl, "slider_h", Value::Number(12.0));
        sl = prop(sl, "label_w", Value::Number(80.0));
        sl = prop(sl, "value_w", Value::Number(40.0));
        sl = prop(sl, "min", Value::Number(0.0));
        sl = prop(sl, "max", Value::Number(100.0));
        sl = prop(sl, "focus_group", Value::Text("fit_focus".into()));
        let mut page = node("screen");
        page.children = vec![sl];
        let st = serde_json::json!({});
        let model = ValueMap::new().with("v", 25.0);

        // Track spans x 80..260, y 14..26; the grab band pads it to y 8..32.

        // Click in the LABEL column: claims the pointer + grabs group focus, but
        // captures nothing and leaves the value at its echo.
        let mut state = UiState::new();
        let f = run_ui(&page, &model, &st, &input_at(30.0, 20.0, true), &mut state);
        assert!(f.results.is_on("hud_hit"), "the whole row claims");
        assert_eq!(f.results.text("fit_focus"), Some("v"), "a row click grabs group focus");
        assert_eq!(f.results.number("v"), Some(25.0), "no drag: the value just echoes");
        // Held next frame with the pointer over the track x: still no capture.
        let held = UiInput { mouse: Vec2::new(170.0, 20.0), clicked: false, down: true, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui(&page, &model, &st, &held, &mut state);
        assert_eq!(f.results.number("v"), Some(25.0), "label-press never became a drag");

        // Click INSIDE the row but ABOVE the grab band (y=4 < 8): claims + focuses,
        // no capture.
        let mut state = UiState::new();
        let f = run_ui(&page, &model, &st, &input_at(170.0, 4.0, true), &mut state);
        assert!(f.results.is_on("hud_hit"));
        assert_eq!(f.results.text("fit_focus"), Some("v"));
        assert_eq!(f.results.number("v"), Some(25.0), "outside the grab band nothing captures");

        // Click in the ±6px slop ABOVE the track (y=10, inside 8..14): captures and
        // writes the mapped value ((170-80)/180 = 50).
        let mut state = UiState::new();
        let f = run_ui(&page, &model, &st, &input_at(170.0, 10.0, true), &mut state);
        let v = f.results.number("v").expect("grab-band press writes");
        assert!((v - 50.0).abs() < 2.0, "press maps the pointer over the track: {v}");

        // Idle frame with the pointer off the row: the group-focus key echoes the
        // model's persisted focus.
        let mut state = UiState::new();
        let focused = ValueMap::new().with("v", 25.0).with("fit_focus", "v");
        let f = run_ui(&page, &focused, &st, &input_at(700.0, 500.0, false), &mut state);
        assert_eq!(f.results.text("fit_focus"), Some("v"), "focus echoes off-pointer");
    }

    #[test]
    fn hidden_subtree_is_not_placed() {
        let mut hidden = node("button");
        hidden.action = Some("nope".into());
        hidden.visible_bind = Some("shown".into());
        hidden.size = Some(24.0);
        hidden.anchor = Some(UiAnchor::TopLeft);
        let mut page = node("screen");
        page.children = vec![hidden];

        let model = ValueMap::new().with("shown", false);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &serde_json::json!({}), &input_at(5.0, 5.0, true), &mut state);
        assert!(!frame.results.is_on("nope"), "a hidden button can't be clicked");

        let model = ValueMap::new().with("shown", true);
        let frame = run_ui(&page, &model, &serde_json::json!({}), &input_at(5.0, 5.0, true), &mut state);
        assert!(frame.results.is_on("nope"), "shown → clickable");
    }

    #[test]
    fn sprite_aspect_locks_square_and_sits_below_a_layered_panel() {
        // A viewport-tall Muse sprite (tex 4, height = 114% of the 600px screen,
        // width aspect-locked square) on the base layer, and a popup panel lifted
        // to layer 1 above it.
        let mut muse = node("sprite");
        muse.anchor = Some(UiAnchor::BottomRight);
        muse = prop(muse, "tex", Value::Number(4.0));
        muse = prop(muse, "height_frac", Value::Number(1.14));
        muse = prop(muse, "aspect", Value::Number(1.0)); // square → width follows height

        let mut popup = node("cell");
        popup.anchor = Some(UiAnchor::Center);
        popup.width = Some(200.0);
        popup.height = Some(100.0);
        popup = prop(popup, "layer", Value::Number(1.0));
        popup = prop(popup, "style", Value::Text("btn".into())); // any style → a panel bg

        let mut page = node("screen");
        page.children = vec![muse, popup];

        let model = ValueMap::new();
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");
        let frame = run_ui_with(&page, &model, &styles(), &input_at(0.0, 0.0, false), &mut state, Some(&lib));

        // The sprite blits tex 4 at 1.14 × the 600px screen height = 684px, square, layer 0.
        let (tex, w, h, slayer) = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Sprite { tex, w, h, layer, .. } => Some((*tex, *w, *h, *layer)),
                _ => None,
            })
            .expect("sprite drawn");
        assert_eq!(tex, 4);
        assert!((h - 684.0).abs() < 0.5, "height = 1.14×600, got {h}");
        assert!((w - h).abs() < 0.5, "aspect=1 keeps the Muse square, got w={w} h={h}");
        assert_eq!(slayer, 0.0, "the Muse stays on the base layer");

        // The popup panel is lifted a whole layer above the backdrop sprite.
        let panel_layer = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Panel { layer, .. } => Some(*layer),
                _ => None,
            })
            .expect("popup panel drawn");
        assert_eq!(panel_layer, 1.0, "popup subtree lifts above the backdrop");
    }

    #[test]
    fn stage_node_reserves_an_inset_slot_and_draws_its_own_backdrop() {
        let st = serde_json::json!({
            "thumb": { "fill_top": [0.1,0.1,0.1,1.0], "fill_bot": [0.0,0.0,0.0,1.0],
                       "border": [0.2,0.2,0.2,1.0], "border_w": 1, "radius": 6, "inset": 2 },
            "warm": [1.0, 0.9, 0.8, 1.0]
        });
        let mut s = node("rtt");
        s.id = "pack_thumb".into();
        s.anchor = Some(UiAnchor::TopLeft);
        s.offset = [10.0, 20.0];
        s.width = Some(92.0);
        s.height = Some(92.0);
        s = prop(s, "style", Value::Text("thumb".into()));
        s = prop(s, "source", Value::Text("portrait".into()));
        s = prop(s, "tint", Value::Text("warm".into()));
        let mut page = node("screen");
        page.children = vec![s];

        let mut state = UiState::new();
        let frame = run_ui(&page, &ValueMap::new(), &st, &input_at(50.0, 60.0, false), &mut state);

        assert_eq!(frame.rtts.len(), 1, "one PiP slot reserved");
        let slot = &frame.rtts[0];
        assert_eq!(slot.id, "pack_thumb");
        assert_eq!(slot.source, "portrait");
        // The image rect is the node rect inset by the STYLE's `inset` — so a whole
        // family of stages shares one inset without repeating it per node.
        assert_eq!((slot.x, slot.y, slot.w, slot.h), (12.0, 22.0, 88.0, 88.0));
        assert_eq!(slot.tint, [1.0, 0.9, 0.8, 1.0], "tint resolved from its dotted path");
        assert!(slot.live, "a stage with no liveness policy renders");
        // The walker draws the backdrop itself, which is why the scene passes
        // `frame: None` to composite_panel — one panel, one code path.
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Panel { .. })),
            "stage drew its panel backdrop"
        );
        assert!(frame.results.is_on("hud_hit"), "a stage claims the pointer");
    }

    #[test]
    fn stage_liveness_follows_its_bind_and_a_sourceless_stage_reserves_nothing() {
        let st = serde_json::json!({ "thumb": { "fill_top": [0.1,0.1,0.1,1.0] } });
        let staged = |id: &str, live_key: &str| {
            let mut s = node("rtt");
            s.id = id.into();
            s.anchor = Some(UiAnchor::TopLeft);
            s.width = Some(40.0);
            s.height = Some(40.0);
            s = prop(s, "style", Value::Text("thumb".into()));
            s = prop(s, "source", Value::Text("portrait".into()));
            prop(s, "live_bind", Value::Text(live_key.into()))
        };
        // A stage with no `source` is dropped rather than reserving a broken slot.
        let mut orphan = node("rtt");
        orphan.id = "orphan".into();
        orphan.width = Some(10.0);
        orphan.height = Some(10.0);

        let mut page = node("screen");
        page.children = vec![staged("hot", "sel"), staged("cold", "unsel"), orphan];

        let model = ValueMap::new().with("sel", true).with("unsel", false);
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &st, &input_at(700.0, 500.0, false), &mut state);

        assert_eq!(frame.rtts.len(), 2, "the source-less stage reserved nothing");
        let live_of = |id: &str| frame.rtts.iter().find(|s| s.id == id).unwrap().live;
        assert!(live_of("hot"), "bound true → renders a fresh target");
        assert!(!live_of("cold"), "bound false → scene reuses its cached poster");
    }

    #[test]
    fn rune_corners_draws_four_glyphs_glow_top_bronze_bottom() {
        // A rune_corners overlay filling a 200×120 rect at the origin, carrying the four
        // corner glyphs + the reused `runes` style block (mirrors `settings.runes`).
        let mut rc = node("rune_corners");
        rc.id = "rc".into();
        rc.width = Some(200.0);
        rc.height = Some(120.0);
        rc.anchor = Some(UiAnchor::TopLeft);
        rc = prop(rc, "tl", Value::Text("ᛞ".into()));
        rc = prop(rc, "tr", Value::Text("ᛝ".into()));
        rc = prop(rc, "bl", Value::Text("ᚨ".into()));
        rc = prop(rc, "br", Value::Text("ᛟ".into()));
        rc = prop(rc, "style", Value::Text("runes".into()));
        let mut page = node("screen");
        page.children = vec![rc];

        let glow: [f32; 4] = [0.43, 0.59, 1.0, 1.0];
        let bronze: [f32; 4] = [0.5, 0.38, 0.2, 1.0];
        let styles = serde_json::json!({
            "runes": { "size": 16, "inset": 12, "top": glow, "bot": bronze }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");

        let frame = run_ui_with(&page, &model, &styles, &input_at(400.0, 400.0, false), &mut state, Some(&lib));

        // Exactly four Rune-font glyphs, emitted in TL, TR, BL, BR order.
        let runes: Vec<(&str, f32, f32, [f32; 4])> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { text, x, y, color, font: FontRole::Rune, .. } => {
                    Some((text.as_str(), *x, *y, *color))
                }
                _ => None,
            })
            .collect();
        assert_eq!(runes.len(), 4, "four corner glyphs drawn");
        assert_eq!(runes[0].0, "ᛞ", "top-left glyph");
        assert_eq!(runes[1].0, "ᛝ", "top-right glyph");
        assert_eq!(runes[2].0, "ᚨ", "bottom-left glyph");
        assert_eq!(runes[3].0, "ᛟ", "bottom-right glyph");

        // Top pair glows (rune-light); bottom pair is dim bronze.
        assert_eq!(runes[0].3, glow, "top-left uses the glow colour");
        assert_eq!(runes[1].3, glow, "top-right uses the glow colour");
        assert_eq!(runes[2].3, bronze, "bottom-left uses the bronze colour");
        assert_eq!(runes[3].3, bronze, "bottom-right uses the bronze colour");

        // Left pair anchors at the left inset; right pair anchors at the right inset
        // (right-aligned). Every glyph anchor sits inside the 200×120 node rect.
        assert!((runes[0].1 - 12.0).abs() < 0.01 && (runes[2].1 - 12.0).abs() < 0.01, "left glyphs at inset 12");
        assert!((runes[1].1 - 188.0).abs() < 0.01 && (runes[3].1 - 188.0).abs() < 0.01, "right glyphs at w - inset");
        for &(g, x, y, _) in &runes {
            assert!((0.0..=200.0).contains(&x) && (0.0..=120.0).contains(&y), "glyph {g} anchor within rect: ({x},{y})");
        }

        // Top pair sits above the bottom pair (a corner decoration, not a pile).
        assert!(runes[0].2 < runes[2].2 && runes[1].2 < runes[3].2, "top pair above bottom pair");

        // No interaction: a bare decoration doesn't claim the pointer on its own.
        assert!(!frame.results.is_on("hud_hit"), "a bare rune_corners overlay claims nothing");
    }

    #[test]
    fn tooltip_paints_name_meta_and_a_coloured_rune_without_claiming_the_pointer() {
        // A floating info card the SCENE positions (rect) and gates (visible_bind):
        // an element rune badge, a name headline, and a dim meta line. Presentational
        // — it must never claim the pointer, or a cursor-following tip eats every click.
        let mut tip = node("tooltip");
        tip.id = "tip".into();
        tip.width = Some(220.0);
        tip.height = Some(64.0);
        tip.anchor = Some(UiAnchor::TopLeft);
        tip.offset = [20.0, 20.0];
        tip = prop(tip, "style", Value::Text("tip".into()));
        tip = prop(tip, "name", Value::Text("Emberlash".into()));
        tip = prop(tip, "rune", Value::Text("\u{16A0}".into())); // ᚠ Elder Futhark 'fehu'
        tip = prop(tip, "rune_color", Value::Text("elem.fire".into()));
        tip = prop(tip, "meta", Value::Text("evocation · 1 action · 3 mana".into()));

        let styles = serde_json::json!({
            "elem": { "fire": [1.0, 0.4, 0.1, 1.0] },
            "tip": {
                "bg": [0.05, 0.06, 0.09, 0.94], "border": [0.17, 0.19, 0.24, 1.0],
                "radius": 5, "pad": 10,
                "name_color": [0.9, 0.9, 0.85, 1.0], "name_size": 16,
                "meta_color": [0.5, 0.5, 0.5, 1.0], "meta_size": 12
            }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");

        let mut page = node("screen");
        page.children = vec![tip.clone()];

        // Click squarely over the card (rect 20,20 .. 240,84; centre ≈ 130,52) — a
        // presentational tip claims nothing.
        let frame = run_ui_with(&page, &model, &styles, &input_at(130.0, 52.0, true), &mut state, Some(&lib));
        assert!(!frame.results.is_on("hud_hit"), "a presentational tooltip never steals the pointer");

        // The backdrop panel fills the node rect (the single Panel command).
        let panel = frame.commands.iter().find_map(|c| match c {
            HudCommand::Panel { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
            _ => None,
        });
        assert_eq!(panel, Some((20.0, 20.0, 220.0, 64.0)), "card backdrop fills the node rect");

        // Name headline drawn in the Display face.
        let name = frame.commands.iter().find_map(|c| match c {
            HudCommand::Text { text, font, y, .. } if text == "Emberlash" => Some((*font, *y)),
            _ => None,
        }).expect("name headline drawn");
        assert_eq!(name.0, FontRole::Display, "the name uses the display face");

        // Meta line drawn in the dim meta colour, on the row below the name.
        let meta = frame.commands.iter().find_map(|c| match c {
            HudCommand::Text { text, color, y, .. } if text.contains("evocation") => Some((*color, *y)),
            _ => None,
        }).expect("meta line drawn");
        assert_eq!(meta.0, [0.5, 0.5, 0.5, 1.0], "meta uses the dim meta_color");
        assert!(meta.1 > name.1, "meta sits on the line below the name");

        // Element rune drawn in the Rune face, coloured by its dotted rune_color path.
        let rune = frame.commands.iter().find_map(|c| match c {
            HudCommand::Text { text, font, color, .. } if *font == FontRole::Rune => Some((text.clone(), *color)),
            _ => None,
        }).expect("rune glyph drawn in the rune face");
        assert_eq!(rune.0.as_str(), "\u{16A0}", "the glyph is the node's rune");
        assert_eq!(rune.1, [1.0, 0.4, 0.1, 1.0], "rune colour resolved from the dotted rune_color path");

        // Every glyph origin sits inside the node rect (20,20 .. 240,84).
        for c in &frame.commands {
            if let HudCommand::Text { x, y, .. } = c {
                assert!(*x >= 20.0 && *y >= 20.0 && *x <= 240.0 && *y <= 84.0, "text within card: {x},{y}");
            }
        }

        // The rune is OPTIONAL — the same card with no `rune` still draws name + meta,
        // and emits no Rune-face glyph.
        let mut plain = tip;
        plain.props.remove("rune");
        let mut page2 = node("screen");
        page2.children = vec![plain];
        let frame = run_ui_with(&page2, &model, &styles, &input_at(-9.0, -9.0, false), &mut state, Some(&lib));
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "Emberlash")),
            "name still drawn without a rune"
        );
        assert!(
            !frame.commands.iter().any(|c| matches!(c, HudCommand::Text { font, .. } if *font == FontRole::Rune)),
            "no rune glyph when the prop is absent"
        );
    }

    // Add inside `mod tests`, alongside the other piece tests. Uses inline style json.
    #[test]
    fn badge_draws_toned_pill_and_claims_pointer() {
        // A presentational badge: an accent-tone chip labelled "NEW", 60×20 at the origin.
        let mut b = node("badge");
        b.id = "b".into();
        b.width = Some(60.0);
        b.height = Some(20.0);
        b.anchor = Some(UiAnchor::TopLeft);
        b = prop(b, "label", Value::Text("NEW".into()));
        b = prop(b, "tone", Value::Text("accent".into()));
        b = prop(b, "style", Value::Text("badge".into()));
        let mut page = node("screen");
        page.children = vec![b];

        let st = serde_json::json!({
            "badge": {
                "pad": 0, "h": 20, "radius": 10, "label_size": 11,
                "accent_bg": [0.14, 0.25, 0.47, 1.0], "accent_label": [0.93, 0.95, 1.0, 1.0],
                "neutral_bg": [0.08, 0.09, 0.12, 1.0], "neutral_label": [0.56, 0.54, 0.49, 1.0],
                "bronze_bg": [0.43, 0.35, 0.20, 1.0], "bronze_label": [0.87, 0.85, 0.79, 1.0],
                "solid_bg": [0.72, 0.59, 0.35, 1.0], "solid_label": [0.03, 0.04, 0.05, 1.0]
            }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");

        // Pointer over the pill → the badge claims the mouse (scene can't pick through).
        let frame = run_ui_with(&page, &model, &st, &input_at(30.0, 10.0, true), &mut state, Some(&lib));
        assert!(frame.results.is_on("hud_hit"), "pointer over the badge claims the mouse");

        // The pill uses the accent tone's bg, a pill radius (≈ h/2), and stays inside the
        // 60×20 node rect.
        let pill = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Panel { x, y, w, h, color, radius, .. } => Some((*x, *y, *w, *h, *color, *radius)),
                _ => None,
            })
            .expect("badge drew its pill");
        assert_eq!(pill.4, [0.14, 0.25, 0.47, 1.0], "accent tone fills with accent_bg");
        assert!((pill.5 - 10.0).abs() < 0.01, "radius ≈ h/2 (a full capsule)");
        assert!(
            pill.0 >= -0.01 && pill.1 >= -0.01 && pill.0 + pill.2 <= 60.01 && pill.1 + pill.3 <= 20.01,
            "pill within the node rect: {pill:?}"
        );

        // The centred label reached the draw commands in the accent label colour.
        let label = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Text { text, color, align, .. } => Some((text.clone(), *color, *align)),
                _ => None,
            })
            .expect("badge drew its label");
        assert_eq!(label.0, "NEW");
        assert_eq!(label.1, [0.93, 0.95, 1.0, 1.0], "label uses accent_label");
        assert!(matches!(label.2, TextAlign::Center), "label is centred");

        // `solid` OVERRIDES the tone → a filled bronze chip (solid_bg / solid_label),
        // even though an `accent` tone is also present.
        let mut b2 = node("badge");
        b2.width = Some(60.0);
        b2.height = Some(20.0);
        b2.anchor = Some(UiAnchor::TopLeft);
        b2 = prop(b2, "label", Value::Text("LIVE".into()));
        b2 = prop(b2, "tone", Value::Text("accent".into())); // tone present…
        b2 = prop(b2, "solid", Value::Bool(true)); // …but solid wins
        b2 = prop(b2, "style", Value::Text("badge".into()));
        let mut page2 = node("screen");
        page2.children = vec![b2];
        let frame = run_ui_with(&page2, &model, &st, &input_at(500.0, 500.0, false), &mut state, Some(&lib));
        let fill = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Panel { color, .. } => Some(*color),
                _ => None,
            })
            .expect("solid badge drew its pill");
        assert_eq!(fill, [0.72, 0.59, 0.35, 1.0], "solid overrides tone → solid_bg (bronze)");
    }

    // ── Add inside `mod tests`, alongside the other per-kind tests. Uses the same
    // `node` / `prop` / `input_at` / `run_ui` harness the existing tests use.
        #[test]
        fn context_menu_row_click_fires_action_skips_divider_and_disabled() {
            // Items are CHILD data nodes: a plain row (+keybind hint), an active row, a
            // divider, a disabled row, and a final plain row. row_h 30 → five 30px slots
            // stacked from the top of the menu rect.
            let mut cut = prop(node("item"), "label", Value::Text("Cut".into()));
            cut.action = Some("cut".into());
            cut = prop(cut, "hint", Value::Text("X".into()));

            let mut copy = prop(node("item"), "label", Value::Text("Copy".into()));
            copy.action = Some("copy".into());
            copy = prop(copy, "active", Value::Bool(true));

            let divider = prop(node("item"), "divider", Value::Bool(true));

            let mut paste = prop(node("item"), "label", Value::Text("Paste".into()));
            paste.action = Some("paste".into());
            paste = prop(paste, "disabled", Value::Bool(true));

            let mut del = prop(node("item"), "label", Value::Text("Delete".into()));
            del.action = Some("del".into());

            let mut menu = node("context_menu");
            menu.id = "ctx".into();
            menu.width = Some(200.0);
            menu.height = Some(150.0);
            menu.anchor = Some(UiAnchor::TopLeft);
            menu = prop(menu, "style", Value::Text("menu".into()));
            menu.children = vec![cut, copy, divider, paste, del];

            let mut page = node("screen");
            page.children = vec![menu];

            // The reused settings.controls.menu block shape (values inline, not a live path).
            let styles = serde_json::json!({
                "menu": {
                    "top": [0.1,0.1,0.1,1.0], "bot": [0.0,0.0,0.0,1.0],
                    "border": [0.2,0.2,0.2,1.0], "radius": 3, "row_h": 30, "label_size": 15,
                    "label": [1.0,1.0,1.0,1.0], "sel_bg": [0.2,0.3,0.5,1.0],
                    "sel_label": [1.0,1.0,1.0,1.0], "hover_bg": [0.1,0.15,0.25,1.0]
                }
            });
            let model = ValueMap::new();
            let mut state = UiState::new();

            // Row 0 (y 0..30) is live → fires "cut" and the menu claims the pointer.
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 15.0, true), &mut state);
            assert!(f.results.is_on("cut"), "clicking a live row fires its action");
            assert!(f.results.is_on("hud_hit"), "the menu surface claims the pointer");

            // Row 3 (y 90..120) is disabled → its action never fires, but the surface
            // still claims the pointer (no pick-through to the scene behind).
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 105.0, true), &mut state);
            assert!(!f.results.is_on("paste"), "a disabled row is not clickable");
            assert!(f.results.is_on("hud_hit"), "still claims the pointer over a disabled row");

            // Row 2 (y 60..90) is a divider → inert; nothing fires.
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 75.0, true), &mut state);
            assert!(
                !f.results.is_on("cut") && !f.results.is_on("copy") && !f.results.is_on("paste") && !f.results.is_on("del"),
                "a divider row fires no action"
            );

            // Row 4 (y 120..150) is live → fires "del".
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 135.0, true), &mut state);
            assert!(f.results.is_on("del"), "the last row fires its action");

            // Idle draw frame: pointer over row 0. Every drawn panel / row band / hairline /
            // label stays within the 200×150 node rect, and the active row draws its wash.
            let f = run_ui(&page, &model, &styles, &input_at(100.0, 15.0, false), &mut state);
            for c in &f.commands {
                match c {
                    HudCommand::Panel { x, y, w, h, .. } => assert!(
                        *x >= -0.01 && *y >= -0.01 && x + w <= 200.01 && y + h <= 150.01,
                        "menu panel within node rect: {x},{y} {w}×{h}"
                    ),
                    HudCommand::Rect { x, y, w, h, .. } => assert!(
                        *x >= -0.01 && *y >= -0.01 && x + w <= 200.01 && y + h <= 150.01,
                        "row wash / hairline within node rect: {x},{y} {w}×{h}"
                    ),
                    HudCommand::Text { x, y, .. } => assert!(
                        *x >= -0.01 && *x <= 200.01 && *y >= -0.01 && *y <= 150.01,
                        "menu text within node rect: {x},{y}"
                    ),
                    _ => {}
                }
            }
            let has_sel = f
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Rect { color, .. } if *color == [0.2, 0.3, 0.5, 1.0]));
            assert!(has_sel, "the active row draws a selection wash (sel_bg)");

            // A click fully OUTSIDE the menu fires nothing and claims nothing.
            let f = run_ui(&page, &model, &styles, &input_at(400.0, 400.0, true), &mut state);
            assert!(!f.results.is_on("cut") && !f.results.is_on("del"), "a click off the menu fires nothing");
            assert!(!f.results.is_on("hud_hit"), "a click off the menu doesn't claim the pointer");
        }

    /// The S5 context-menu behaviour fixture: a 200×150 menu at the origin whose
    /// style block carries the FULL alias set (divider/disabled/hint included), so
    /// the per-state colours are distinguishable in the emitted commands.
    fn context_menu_styles() -> Json {
        serde_json::json!({
            "menu": {
                "top": [0.1,0.1,0.1,1.0], "bot": [0.0,0.0,0.0,1.0],
                "border": [0.2,0.2,0.2,1.0], "radius": 3, "row_h": 30, "label_size": 15,
                "label": [1.0,1.0,1.0,1.0], "sel_bg": [0.2,0.3,0.5,1.0],
                "sel_label": [0.9,0.95,1.0,1.0], "hover_bg": [0.1,0.15,0.25,1.0],
                "divider": [0.3,0.3,0.3,1.0], "disabled": [0.4,0.4,0.4,1.0],
                "hint": [0.5,0.5,0.5,1.0]
            }
        })
    }

    /// The matching 5-row menu: Cut (hint "X") · Copy (active) · divider ·
    /// Paste (disabled) · Delete. row_h 30 → rows at y 0/30/60/90/120.
    fn context_menu_tree() -> UiNode {
        let mut cut = prop(node("item"), "label", Value::Text("Cut".into()));
        cut.action = Some("cut".into());
        cut = prop(cut, "hint", Value::Text("X".into()));
        let mut copy = prop(node("item"), "label", Value::Text("Copy".into()));
        copy.action = Some("copy".into());
        copy = prop(copy, "active", Value::Bool(true));
        let divider = prop(node("item"), "divider", Value::Bool(true));
        let mut paste = prop(node("item"), "label", Value::Text("Paste".into()));
        paste.action = Some("paste".into());
        paste = prop(paste, "disabled", Value::Bool(true));
        let mut del = prop(node("item"), "label", Value::Text("Delete".into()));
        del.action = Some("del".into());

        let mut menu = node("context_menu");
        menu.id = "ctx".into();
        menu.width = Some(200.0);
        menu.height = Some(150.0);
        menu.anchor = Some(UiAnchor::TopLeft);
        menu = prop(menu, "style", Value::Text("menu".into()));
        menu.children = vec![cut, copy, divider, paste, del];
        let mut page = node("screen");
        page.children = vec![menu];
        page
    }

    #[test]
    fn context_menu_hover_washes_live_rows_only_and_right_aligns_hints() {
        let page = context_menu_tree();
        let styles = context_menu_styles();
        let model = ValueMap::new();

        // Hovering row 0 (a live row): exactly one hover wash, at the select-popup
        // inset geometry (x+4, row top, w−8, row_h); the active row keeps its
        // selection wash; the divider hairline is centred in its band; the hint is
        // right-aligned 14px in from the menu's right edge; the disabled label dims.
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 15.0, false), &mut UiState::new());
        assert!(f.results.is_on("hud_hit"), "the menu surface claims the hover");
        let rects: Vec<(f32, f32, f32, f32, [f32; 4])> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Rect { x, y, w, h, color, .. } => Some((*x, *y, *w, *h, *color)),
                _ => None,
            })
            .collect();
        let hover: Vec<_> =
            rects.iter().filter(|r| r.4 == [0.1, 0.15, 0.25, 1.0]).collect();
        assert_eq!(hover.len(), 1, "exactly one hovered row washes: {rects:?}");
        assert_eq!((hover[0].0, hover[0].1, hover[0].2, hover[0].3), (4.0, 0.0, 192.0, 30.0));
        let sel: Vec<_> = rects.iter().filter(|r| r.4 == [0.2, 0.3, 0.5, 1.0]).collect();
        assert_eq!(sel.len(), 1, "the active row washes sel_bg");
        assert_eq!((sel[0].0, sel[0].1, sel[0].2, sel[0].3), (4.0, 30.0, 192.0, 30.0));
        let hairline: Vec<_> = rects.iter().filter(|r| r.4 == [0.3, 0.3, 0.3, 1.0]).collect();
        assert_eq!(hairline.len(), 1, "the divider draws one hairline");
        assert_eq!((hairline[0].0, hairline[0].1, hairline[0].2, hairline[0].3), (8.0, 75.0, 184.0, 1.0));
        let texts: Vec<(&str, f32, f32, [f32; 4], TextAlign)> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { text, x, y, color, align, .. } => {
                    Some((text.as_str(), *x, *y, *color, *align))
                }
                _ => None,
            })
            .collect();
        let hint = texts.iter().find(|t| t.0 == "X").expect("hint drew");
        assert!(matches!(hint.4, TextAlign::Right), "hint is right-aligned");
        assert_eq!((hint.1, hint.2), (186.0, 7.5), "hint anchors 14px in from the right edge");
        assert_eq!(hint.3, [0.5, 0.5, 0.5, 1.0], "hint uses the hint colour");
        let paste = texts.iter().find(|t| t.0 == "Paste").expect("disabled row drew");
        assert_eq!(paste.3, [0.4, 0.4, 0.4, 1.0], "a disabled label dims");
        let copy = texts.iter().find(|t| t.0 == "Copy").expect("active row drew");
        assert_eq!(copy.3, [0.9, 0.95, 1.0, 1.0], "the active label uses sel_label");

        // Hovering the DISABLED row (y 90..120): no hover wash anywhere — an inert
        // row never highlights — yet the surface still claims the pointer.
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 105.0, false), &mut UiState::new());
        assert!(
            !f.commands.iter().any(
                |c| matches!(c, HudCommand::Rect { color, .. } if *color == [0.1, 0.15, 0.25, 1.0])
            ),
            "a disabled row neither fires nor highlights"
        );
        assert!(f.results.is_on("hud_hit"));

        // Hovering the divider band (y 60..90): inert too — no hover wash.
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 75.0, false), &mut UiState::new());
        assert!(
            !f.commands.iter().any(
                |c| matches!(c, HudCommand::Rect { color, .. } if *color == [0.1, 0.15, 0.25, 1.0])
            ),
            "a divider band never highlights"
        );
        assert!(f.results.is_on("hud_hit"));
    }

    #[test]
    fn context_menu_rows_laid_past_the_rect_still_fire() {
        // Six live rows in an authored 200×150 rect: row_h 30 lays row 5 at y
        // 150..180 — PAST the rect bottom. That row stays clickable (the load-
        // bearing semantic: the row loop is not gated on the authored rect), while
        // the hud_hit claim remains the authored rect only.
        let mut menu = node("context_menu");
        menu.id = "ctx6".into();
        menu.width = Some(200.0);
        menu.height = Some(150.0);
        menu.anchor = Some(UiAnchor::TopLeft);
        menu = prop(menu, "style", Value::Text("menu".into()));
        menu.children = (1..=6)
            .map(|i| {
                let mut it = prop(node("item"), "label", Value::Text(format!("R{i}")));
                it.action = Some(format!("a{i}"));
                it
            })
            .collect();
        let mut page = node("screen");
        page.children = vec![menu];
        let styles = context_menu_styles();
        let model = ValueMap::new();

        // Click mid-row-5 (y=165, beyond the 150px rect): its action fires; the
        // claim does not extend past the authored rect.
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 165.0, true), &mut UiState::new());
        assert!(f.results.is_on("a6"), "a row laid past the rect bottom still fires");
        assert!(!f.results.is_on("hud_hit"), "only the authored rect claims the pointer");

        // The same row hover-washes when the frame draws (draw shares the row math).
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 165.0, false), &mut UiState::new());
        assert!(
            f.commands.iter().any(|c| matches!(
                c,
                HudCommand::Rect { y, color, .. }
                    if *y == 150.0 && *color == [0.1, 0.15, 0.25, 1.0]
            )),
            "the overflow row hover-washes at its own band"
        );

        // A row inside the rect keeps firing as before.
        let f = run_ui(&page, &model, &styles, &input_at(100.0, 15.0, true), &mut UiState::new());
        assert!(f.results.is_on("a1"));
        assert!(f.results.is_on("hud_hit"));
    }

    // (The transient S5 parity test — `draw_context_menu` vs the Lua module, byte
    // for byte across hover/active/divider/disabled/plain/off-menu pointer states —
    // gated the port and was deleted with the Rust arm; the byte-pinned draw below
    // and the behaviour tests above carry the regression duty.)
    #[test]
    fn context_menu_lua_draw_is_byte_pinned() {
        // The full command list for a 3-row menu (plain+hint · active · divider),
        // pointer on row 0 — the byte-level pin that outlives the deleted Rust arm.
        let mut cut = prop(node("item"), "label", Value::Text("Cut".into()));
        cut.action = Some("cut".into());
        cut = prop(cut, "hint", Value::Text("X".into()));
        let mut copy = prop(node("item"), "label", Value::Text("Copy".into()));
        copy.action = Some("copy".into());
        copy = prop(copy, "active", Value::Bool(true));
        let divider = prop(node("item"), "divider", Value::Bool(true));
        let mut menu = node("context_menu");
        menu.id = "ctx3".into();
        menu.width = Some(200.0);
        menu.height = Some(90.0);
        menu.anchor = Some(UiAnchor::TopLeft);
        menu = prop(menu, "style", Value::Text("menu".into()));
        menu.children = vec![cut, copy, divider];
        let mut page = node("screen");
        page.children = vec![menu];

        let f = run_ui(
            &page,
            &ValueMap::new(),
            &context_menu_styles(),
            &input_at(100.0, 15.0, false),
            &mut UiState::new(),
        );
        let text = |x: f32, y: f32, s: &str, color: [f32; 4], align: TextAlign| HudCommand::Text {
            x,
            y,
            text: s.to_string(),
            size: 15.0,
            color,
            layer: 0.0,
            align,
            font: FontRole::Body,
            italic: false,
            bold: false,
            tracking: -1.0,
            wrap: None,
        };
        let expected = vec![
            HudCommand::Panel {
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h: 90.0,
                color: [0.1, 0.1, 0.1, 1.0],
                color2: [0.0, 0.0, 0.0, 1.0],
                grad: 1.0,
                radius: 3.0,
                border: 1.0,
                border_color: [0.2, 0.2, 0.2, 1.0],
                feather: 0.0,
                layer: 0.0,
            },
            // Row 0: hover wash (4px inset), left label, right-aligned hint.
            HudCommand::Rect { x: 4.0, y: 0.0, w: 192.0, h: 30.0, color: [0.1, 0.15, 0.25, 1.0], layer: 0.0 },
            text(14.0, 7.5, "Cut", [1.0, 1.0, 1.0, 1.0], TextAlign::Left),
            text(186.0, 7.5, "X", [0.5, 0.5, 0.5, 1.0], TextAlign::Right),
            // Row 1: selection wash + sel_label.
            HudCommand::Rect { x: 4.0, y: 30.0, w: 192.0, h: 30.0, color: [0.2, 0.3, 0.5, 1.0], layer: 0.0 },
            text(14.0, 37.5, "Copy", [0.9, 0.95, 1.0, 1.0], TextAlign::Left),
            // Row 2: the divider hairline, centred in its band.
            HudCommand::Rect { x: 8.0, y: 75.0, w: 184.0, h: 1.0, color: [0.3, 0.3, 0.3, 1.0], layer: 0.0 },
        ];
        assert_eq!(f.commands, expected, "the context menu draw is byte-stable");
    }

    #[test]
    fn a_visible_context_menu_idles_at_zero_crossings() {
        let _g = crate::strings::test_guard();
        // An on-screen (open) context menu with the pointer resting on it: the
        // second, unchanged frame crosses into Lua zero times for BOTH draw and hit,
        // yet the surface keeps its claim (memo continuity).
        let lib = counting_lib();
        let page = context_menu_tree();
        let styles = context_menu_styles();
        let model = ValueMap::new();
        let mut state = UiState::new();
        let over = input_at(100.0, 15.0, false);

        let first = run_ui_with(&page, &model, &styles, &over, &mut state, Some(&lib));
        assert!(first.results.is_on("hud_hit"), "pointer over the menu claims");
        assert!(first.stats.lua_draws >= 1, "cold frame draws the menu via Lua");
        let hits_after = lib.hits.get();
        let draws_after = lib.draws.get();

        let second = run_ui_with(&page, &model, &styles, &over, &mut state, Some(&lib));
        assert_eq!(second.stats.lua_hits, 0, "idle frame: zero hit crossings");
        assert_eq!(second.stats.lua_draws, 0, "idle frame: zero draw crossings");
        assert_eq!(second.stats.redraw_nodes, 0, "idle frame: nothing redraws");
        assert_eq!(lib.hits.get(), hits_after, "the library really was not hit");
        assert_eq!(lib.draws.get(), draws_after, "the library really was not drawn");
        assert!(second.results.is_on("hud_hit"), "the claim survives the idle frame");
    }

    // ── Grid layout ──────────────────────────────────────────────────────────
    //
    // The grid arm is exercised through `run_ui` (like every other kind): each
    // CHILD is a styled `panel` leaf, so it emits exactly one `HudCommand::Panel`
    // at its resolved cell rect, in child order; the GRID container is left
    // unstyled so only the child panels appear. `panels()` collects those rects,
    // and the parity tests build a row/column/stack tree AND the equivalent grid
    // tree and assert the child-rect vectors coincide within eps.

    /// Every emitted panel's rect, in draw order (== child order for these trees).
    fn panels(f: &UiFrame) -> Vec<(f32, f32, f32, f32)> {
        f.commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect()
    }

    /// A styled `panel` leaf — one draw command at its cell rect. Optional explicit
    /// `col`/`row` placement and an intrinsic `width` (for auto-track content sizing).
    fn cell(col: Option<i32>, row: Option<i32>, width: Option<f32>) -> UiNode {
        let mut n = prop(node("cell"), "style", Value::Text("box".into()));
        if let Some(c) = col {
            n = prop(n, "col", Value::Number(c as f64));
        }
        if let Some(r) = row {
            n = prop(n, "row", Value::Number(r as f64));
        }
        n.width = width;
        n
    }

    /// A top-left-anchored grid with an explicit box, so its inner rect is exactly
    /// `(0, 0, w, h)` — deterministic cell math with zero container padding.
    fn grid(w: f32, h: f32, cols: &str, rows: &str, gap: f32, kids: Vec<UiNode>) -> UiNode {
        let mut g = node("grid");
        g.id = "g".into();
        g.anchor = Some(UiAnchor::TopLeft);
        g.width = Some(w);
        g.height = Some(h);
        g.gap = gap;
        g = prop(g, "cols", Value::Text(cols.into()));
        g = prop(g, "rows", Value::Text(rows.into()));
        g.children = kids;
        g
    }

    fn page_of(child: UiNode) -> UiNode {
        let mut p = node("screen");
        p.children = vec![child];
        p
    }

    fn boxes() -> Json {
        serde_json::json!({ "box": { "fill_top": [0.1, 0.1, 0.1, 1.0] } })
    }

    fn assert_rects_eq(a: &[(f32, f32, f32, f32)], b: &[(f32, f32, f32, f32)], what: &str) {
        assert_eq!(a.len(), b.len(), "{what}: child count differs ({a:?} vs {b:?})");
        for (i, (ra, rb)) in a.iter().zip(b.iter()).enumerate() {
            let d = (ra.0 - rb.0).abs() + (ra.1 - rb.1).abs() + (ra.2 - rb.2).abs() + (ra.3 - rb.3).abs();
            assert!(d < 1e-3, "{what}: child {i} rect {ra:?} != {rb:?}");
        }
    }

    fn run(page: &UiNode) -> UiFrame {
        run_ui(page, &ValueMap::new(), &boxes(), &input_at(-1.0, -1.0, false), &mut UiState::new())
    }

    #[test]
    fn grid_fixed_fr_fixed_track_sizing() {
        // cols "40 1fr 60" in an 800-wide grid, no gap: the middle fr track eats the
        // 700px remainder; x offsets fall at 0 / 40 / 740.
        let g = grid(800.0, 100.0, "40 1fr 60", "1fr", 0.0, vec![cell(None, None, None), cell(None, None, None), cell(None, None, None)]);
        let ps = panels(&run(&page_of(g)));
        assert_rects_eq(&ps, &[(0.0, 0.0, 40.0, 100.0), (40.0, 0.0, 700.0, 100.0), (740.0, 0.0, 60.0, 100.0)], "fixed/fr/fixed");
    }

    #[test]
    fn grid_fr_ratio_distribution() {
        // "1fr 2fr" splits a 600px extent 1:2 → 200 / 400, summing to the full width.
        let g = grid(600.0, 50.0, "1fr 2fr", "1fr", 0.0, vec![cell(None, None, None), cell(None, None, None)]);
        let ps = panels(&run(&page_of(g)));
        assert_rects_eq(&ps, &[(0.0, 0.0, 200.0, 50.0), (200.0, 0.0, 400.0, 50.0)], "fr ratio");
    }

    #[test]
    fn grid_auto_track_from_content() {
        // "auto auto" sizes each column to its cell's intrinsic width (30, 70); no
        // free space is distributed (there is no fr track), so the columns pack left.
        let g = grid(800.0, 40.0, "auto auto", "1fr", 0.0, vec![cell(None, None, Some(30.0)), cell(None, None, Some(70.0))]);
        let ps = panels(&run(&page_of(g)));
        assert!((ps[0].2 - 30.0).abs() < 1e-3 && (ps[1].2 - 70.0).abs() < 1e-3, "auto tracks size to content: {ps:?}");
        assert!((ps[0].0 - 0.0).abs() < 1e-3 && (ps[1].0 - 30.0).abs() < 1e-3, "auto columns pack left: {ps:?}");
    }

    #[test]
    fn grid_auto_then_fr() {
        // "auto 1fr": the auto column resolves to its 52px content FIRST, then the fr
        // column takes the remaining 748 — proving auto is sized before fr.
        let g = grid(800.0, 40.0, "auto 1fr", "1fr", 0.0, vec![cell(None, None, Some(52.0)), cell(None, None, None)]);
        let ps = panels(&run(&page_of(g)));
        assert!((ps[0].2 - 52.0).abs() < 1e-3, "auto col = 52: {ps:?}");
        assert!((ps[1].2 - 748.0).abs() < 1e-3, "fr col = extent - 52: {ps:?}");
    }

    #[test]
    fn grid_row_gap_and_col_gap_default_to_node_gap() {
        // A 2×2 grid with node.gap = 10 and no explicit col_gap/row_gap: both axes
        // inherit 10, so cell 1 starts at 215 on each axis.
        let kids = || vec![cell(None, None, None), cell(None, None, None), cell(None, None, None), cell(None, None, None)];
        let g = grid(420.0, 420.0, "1fr 1fr", "1fr 1fr", 10.0, kids());
        let ps = panels(&run(&page_of(g)));
        // cols: (420 - 10) / 2 = 205; second column x = 205 + 10 = 215. Same on rows.
        assert!((ps[1].0 - 215.0).abs() < 1e-3, "col_gap defaults to node.gap: {ps:?}");
        assert!((ps[2].1 - 215.0).abs() < 1e-3, "row_gap defaults to node.gap: {ps:?}");

        // Now override row_gap = 4 while leaving col_gap on the default: rows use 4
        // (row 1 y = 208 + 4 = 212), columns still use 10 (col 1 x = 215).
        let mut g = grid(420.0, 420.0, "1fr 1fr", "1fr 1fr", 10.0, kids());
        g = prop(g, "row_gap", Value::Number(4.0));
        let ps = panels(&run(&page_of(g)));
        assert!((ps[1].0 - 215.0).abs() < 1e-3, "cols keep the node.gap of 10: {ps:?}");
        assert!((ps[2].1 - 212.0).abs() < 1e-3, "rows use the overridden gap of 4: {ps:?}");
    }

    #[test]
    fn grid_overflow_negative_fr_matches_flow() {
        // "500 1fr" in a 400px extent: the fixed track already overflows, so the fr
        // track's free space is -100 → a negative-width cell, exactly as flow yields
        // a negative grow length. This intentional no-clamp is what makes parity exact.
        let g = grid(400.0, 50.0, "500 1fr", "1fr", 0.0, vec![cell(None, None, None), cell(None, None, None)]);
        let ps = panels(&run(&page_of(g)));
        assert!((ps[0].2 - 500.0).abs() < 1e-3, "fixed track keeps its 500: {ps:?}");
        assert!((ps[1].2 + 100.0).abs() < 1e-3, "fr track is negative (extent - 500): {ps:?}");
    }

    #[test]
    fn grid_explicit_placement() {
        // A child explicitly at col=1,row=1 lands in the bottom-right cell of a 2×2.
        let g = grid(200.0, 200.0, "1fr 1fr", "1fr 1fr", 0.0, vec![cell(Some(1), Some(1), None)]);
        let ps = panels(&run(&page_of(g)));
        assert_rects_eq(&ps, &[(100.0, 100.0, 100.0, 100.0)], "explicit bottom-right cell");
    }

    #[test]
    fn grid_col_span() {
        // cols "50 50 50" col_gap 8; a child col=0 col_span=2 covers two tracks plus
        // the interior gap → 50 + 50 + 8 = 108.
        let mut c = cell(Some(0), None, None);
        c = prop(c, "col_span", Value::Number(2.0));
        let g = grid(200.0, 50.0, "50 50 50", "1fr", 8.0, vec![c]);
        let ps = panels(&run(&page_of(g)));
        assert!((ps[0].2 - 108.0).abs() < 1e-3, "col_span covers tracks + interior gap: {ps:?}");
    }

    #[test]
    fn grid_row_span() {
        // Symmetric on rows: rows "50 50 50" row_gap 8; row=0 row_span=2 → 108 tall.
        let mut c = cell(None, Some(0), None);
        c = prop(c, "row_span", Value::Number(2.0));
        let g = grid(50.0, 200.0, "1fr", "50 50 50", 8.0, vec![c]);
        let ps = panels(&run(&page_of(g)));
        assert!((ps[0].3 - 108.0).abs() < 1e-3, "row_span covers tracks + interior gap: {ps:?}");
    }

    #[test]
    fn grid_auto_flow_row_major() {
        // Three auto children in a 2-column grid wrap row-major into (0,0),(1,0),(0,1)
        // — the third generating an implicit second row.
        let g = grid(200.0, 200.0, "1fr 1fr", "1fr", 0.0, vec![cell(None, None, None), cell(None, None, None), cell(None, None, None)]);
        let ps = panels(&run(&page_of(g)));
        assert!((ps[0].0 - 0.0).abs() < 1e-3 && (ps[0].1 - 0.0).abs() < 1e-3, "child 0 at (0,0): {ps:?}");
        assert!((ps[1].0 - 100.0).abs() < 1e-3 && (ps[1].1 - 0.0).abs() < 1e-3, "child 1 at (1,0): {ps:?}");
        assert!((ps[2].0 - 0.0).abs() < 1e-3 && (ps[2].1 - 200.0).abs() < 1e-3, "child 2 wraps to the implicit row (0,1): {ps:?}");
    }

    #[test]
    fn grid_auto_flow_skips_explicit() {
        // An explicit child at (0,0) plus two auto children: the autos step around the
        // occupied cell, landing at (1,0) then (0,1).
        let g = grid(200.0, 200.0, "1fr 1fr", "1fr 1fr", 0.0, vec![cell(Some(0), Some(0), None), cell(None, None, None), cell(None, None, None)]);
        let ps = panels(&run(&page_of(g)));
        assert!((ps[1].0 - 100.0).abs() < 1e-3 && (ps[1].1 - 0.0).abs() < 1e-3, "auto skips to (1,0): {ps:?}");
        assert!((ps[2].0 - 0.0).abs() < 1e-3 && (ps[2].1 - 100.0).abs() < 1e-3, "auto continues at (0,1): {ps:?}");
    }

    #[test]
    fn grid_empty_spec_is_single_fill_cell() {
        // An empty cols/rows spec degrades to one 1fr track each — a single fill cell.
        let g = grid(300.0, 200.0, "", "", 0.0, vec![cell(None, None, None)]);
        let ps = panels(&run(&page_of(g)));
        assert_rects_eq(&ps, &[(0.0, 0.0, 300.0, 200.0)], "empty spec fills the inner rect");

        // A garbage spec never panics: unparseable tokens fall back to auto tracks and
        // the child still gets placed (a panel is emitted).
        let g = grid(300.0, 200.0, "?? %%", "", 0.0, vec![cell(None, None, Some(40.0))]);
        let ps = panels(&run(&page_of(g)));
        assert_eq!(ps.len(), 1, "garbage spec still places the child without panicking: {ps:?}");
    }

    #[test]
    fn grid_ignores_child_size_and_grow() {
        // A grid child fills its cell regardless of its own size/grow — the TRACK owns
        // the extent (unlike flow, where size/grow drive the main-axis length).
        let mut c = prop(node("cell"), "style", Value::Text("box".into()));
        c.size = Some(999.0);
        c.grow = Some(5.0);
        let g = grid(300.0, 100.0, "1fr", "1fr", 0.0, vec![c]);
        let ps = panels(&run(&page_of(g)));
        assert_rects_eq(&ps, &[(0.0, 0.0, 300.0, 100.0)], "child fills its cell, ignoring size/grow");
    }

    #[test]
    fn grid_threads_clip_and_layer() {
        // A grid inside a list region: its children inherit the viewport clip (a
        // Clip command precedes them), and a child carrying a `layer` prop has its
        // panel lifted onto that sub-layer above a plain sibling.
        let mut lifted = prop(node("cell"), "style", Value::Text("box".into()));
        lifted = prop(lifted, "layer", Value::Number(5.0));
        let g = grid(200.0, 100.0, "1fr 1fr", "1fr", 0.0, vec![cell(None, None, None), lifted]);

        let mut sc = node("list");
        sc.id = "sc".into();
        sc.bind = Some("sy".into());
        sc.width = Some(200.0);
        sc.height = Some(100.0);
        sc.anchor = Some(UiAnchor::TopLeft);
        sc = prop(sc, "gutter", Value::Number(0.0));
        sc.children = vec![g];

        let f = run(&page_of(sc));
        // A viewport clip opens the list subtree before the grid's child panels.
        let clip_idx = f.commands.iter().position(|c| matches!(c, HudCommand::Clip { rect: Some(_) }));
        let panel_idx = f.commands.iter().position(|c| matches!(c, HudCommand::Panel { .. }));
        assert!(clip_idx.is_some() && panel_idx.is_some() && clip_idx < panel_idx, "grid children carry the list viewport clip");
        // The lifted child's panel sits on layer 5; a plain sibling stays at 0.
        let layers: Vec<f32> = f.commands.iter().filter_map(|c| match c {
            HudCommand::Panel { layer, .. } => Some(*layer),
            _ => None,
        }).collect();
        assert!(layers.iter().any(|l| (l - 5.0).abs() < 1e-3), "a layer-tagged grid child is lifted: {layers:?}");
        assert!(layers.iter().any(|l| l.abs() < 1e-3), "a plain grid child stays on the base layer: {layers:?}");
    }

    #[test]
    fn grid_nested_measure_sizes_parent() {
        // A grid with fixed tracks (cols "30 40", rows "20") nested in a width/height-
        // less column reports a real intrinsic box, so the column sizes to it:
        // width = 30 + 40 = 70, height = 20 (grid pad/gap are 0).
        let mut g = node("grid");
        g = prop(g, "cols", Value::Text("30 40".into()));
        g = prop(g, "rows", Value::Text("20".into()));
        g = prop(g, "style", Value::Text("box".into()));

        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col = prop(col, "style", Value::Text("box".into()));
        col.children = vec![g];

        let ps = panels(&run(&page_of(col)));
        // The column's own panel (drawn first) reflects the measured grid box.
        assert!((ps[0].2 - 70.0).abs() < 1e-3, "column width == 30 + 40: {ps:?}");
        assert!((ps[0].3 - 20.0).abs() < 1e-3, "column height == the 20px row: {ps:?}");
    }

    #[test]
    fn grid_reproduces_row() {
        // PARITY: a horizontal flow (A size=40, B grow=1, C size=60, gap=8) and the
        // grid cols="40 1fr 60" rows="1fr" produce byte-identical child rects.
        let styled = |id: &str| prop({ let mut n = node("cell"); n.id = id.into(); n }, "style", Value::Text("box".into()));
        let mut a = styled("a");
        a.size = Some(40.0);
        let mut b = styled("b");
        b.grow = Some(1.0);
        let mut c = styled("c");
        c.size = Some(60.0);

        let mut row = node("row");
        row.anchor = Some(UiAnchor::TopLeft);
        row.width = Some(800.0);
        row.height = Some(600.0);
        row.gap = 8.0;
        row.children = vec![a, b, c];

        let g = grid(800.0, 600.0, "40 1fr 60", "1fr", 8.0, vec![cell(None, None, None), cell(None, None, None), cell(None, None, None)]);

        assert_rects_eq(&panels(&run(&page_of(g))), &panels(&run(&page_of(row))), "grid reproduces row");
    }

    #[test]
    fn grid_reproduces_column() {
        // PARITY: a vertical flow (A size=30, B grow=1, C size=50, gap=8) and the grid
        // rows="30 1fr 50" cols="1fr" produce byte-identical child rects.
        let styled = |id: &str| prop({ let mut n = node("cell"); n.id = id.into(); n }, "style", Value::Text("box".into()));
        let mut a = styled("a");
        a.size = Some(30.0);
        let mut b = styled("b");
        b.grow = Some(1.0);
        let mut c = styled("c");
        c.size = Some(50.0);

        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.width = Some(400.0);
        col.height = Some(600.0);
        col.gap = 8.0;
        col.children = vec![a, b, c];

        let g = grid(400.0, 600.0, "1fr", "30 1fr 50", 8.0, vec![cell(None, None, None), cell(None, None, None), cell(None, None, None)]);

        assert_rects_eq(&panels(&run(&page_of(g))), &panels(&run(&page_of(col))), "grid reproduces column");
    }

    #[test]
    fn grid_reproduces_stack() {
        // PARITY: a stack of two full-bleed overlay children (width_frac=1,
        // height_frac=1, anchor TopLeft) and the grid cols="1fr" rows="1fr" with both
        // children in cell (0,0) both fill the container — and coincide across trees.
        let full = || {
            let mut n = prop(node("cell"), "style", Value::Text("box".into()));
            n.anchor = Some(UiAnchor::TopLeft);
            n = prop(n, "width_frac", Value::Number(1.0));
            n = prop(n, "height_frac", Value::Number(1.0));
            n
        };
        let mut stack = node("stack");
        stack.anchor = Some(UiAnchor::TopLeft);
        stack.width = Some(300.0);
        stack.height = Some(200.0);
        stack.children = vec![full(), full()];

        let g = grid(300.0, 200.0, "1fr", "1fr", 0.0, vec![cell(Some(0), Some(0), None), cell(Some(0), Some(0), None)]);

        let gp = panels(&run(&page_of(g)));
        let sp = panels(&run(&page_of(stack)));
        assert_rects_eq(&gp, &sp, "grid reproduces stack");
        assert_rects_eq(&gp, &[(0.0, 0.0, 300.0, 200.0), (0.0, 0.0, 300.0, 200.0)], "both children fill the container");
    }

    /// The `frame` TEMPLATE's centre content is inset past the corner-rune zone
    /// purely by the border-grid track structure — WITH NO `title_pad`-style prop
    /// anywhere. This is the Phase-2 payoff: what `window` achieved with a patch-prop
    /// (`title_pad = 30` to shove the title clear of the top-left rune), `frame`
    /// achieves structurally, because the corner cell IS the intersection of the two
    /// edge tracks that also inset the centre.
    #[test]
    fn frame_center_clears_corner_runes_intrinsically() {
        use crate::template::{builtin_templates, expand};

        // A `box`-styled panel that fills whatever cell it lands in (no own size).
        let box_panel = || prop(node("cell"), "style", Value::Text("box".into()));

        // Build the frame TEMPLATE node by hand: center + nw regions, a 400×300 frame,
        // every edge track 30. Crucially there is NO title_pad / body_pad prop.
        let mut frame = node("");
        frame.component = String::new();
        frame.template = Some("frame".into());
        frame.props.insert("w".into(), Value::Number(400.0));
        frame.props.insert("h".into(), Value::Number(300.0));
        frame.props.insert("edge".into(), Value::Number(30.0));
        frame.slots.insert("center".into(), vec![box_panel()]);
        frame.slots.insert("nw".into(), vec![box_panel()]);

        // run_ui does NOT expand templates — it resolves directly — so expand FIRST,
        // else we'd lay out a bare, unexpanded template node and prove nothing.
        let built = expand(frame, &builtin_templates());
        let ps = panels(&run(&page_of(built)));

        // Emission order → ps[0] = nw corner rect, ps[1] = center rect. (The unstyled
        // stack + grid emit no Panel; rune_corners emits TEXT, not Panel — so only the
        // two `box` regions appear.) Assertions are anchor-independent (relative).
        assert_eq!(ps.len(), 2, "only the two box regions emit panels: {ps:?}");
        let (nw, center) = (ps[0], ps[1]);
        // The corner cell is exactly the w_size × n_size intersection: 30 × 30.
        assert!((nw.2 - 30.0).abs() < 1e-3 && (nw.3 - 30.0).abs() < 1e-3, "nw cell is 30×30: {nw:?}");
        // The centre is inset from the frame by exactly the edge tracks — its origin
        // sits (w_size, n_size) = (30, 30) past the corner cell's origin. 30 >= the
        // rune box (inset 14 + size 16), so the content clears the corner rune BY
        // CONSTRUCTION, with no title_pad prop present anywhere.
        assert!((center.0 - nw.0 - 30.0).abs() < 1e-3, "center inset by w_size past nw: {center:?} vs {nw:?}");
        assert!((center.1 - nw.1 - 30.0).abs() < 1e-3, "center inset by n_size past nw: {center:?} vs {nw:?}");
        // Centre extent = frame minus the two edges on each axis: 400-60, 300-60.
        assert!((center.2 - 340.0).abs() < 1e-3, "center width = 400 - 2*30: {center:?}");
        assert!((center.3 - 240.0).abs() < 1e-3, "center height = 300 - 2*30: {center:?}");
    }

    /// END-TO-END clearance: a `frame` filling the 800×600 screen with only a `center`
    /// region and DEFAULT edges (30 = the corner-rune box extent, inset 14 + size 16)
    /// must lay its centre content inset 30px on every side — clearing the rune zone —
    /// WITH NO title_pad-style prop. Proves the whole frame→grid→arrange chain through
    /// `run_ui`, not just template expansion: the clearance is structural, not authored.
    #[test]
    fn frame_center_clears_the_corner_rune_zone_end_to_end() {
        let mut fr = node(""); // component is ignored when `template` is set
        fr.template = Some("frame".into());
        fr = prop(fr, "w_frac", Value::Number(1.0));
        fr = prop(fr, "h_frac", Value::Number(1.0));
        let mut center = prop(node("cell"), "style", Value::Text("box".into()));
        center.id = "center".into();
        fr.slots.insert("center".into(), vec![center]);

        let tree = crate::template::expand(page_of(fr), &crate::template::builtin_templates());
        let ps = panels(&run(&tree));
        // The inset centre cell: (w_edge, n_edge, W - w - e, H - n - s) = (30, 30, 740, 540).
        assert!(
            ps.iter().any(|&(x, y, w, h)| {
                (x - 30.0).abs() < 1e-3 && (y - 30.0).abs() < 1e-3
                    && (w - 740.0).abs() < 1e-3 && (h - 540.0).abs() < 1e-3
            }),
            "centre content must be inset 30px on every side (clearing the rune zone), got {ps:?}"
        );
    }

    /// END-TO-END: the `n` TITLE bar occupies its OWN top-CENTRE cell — it does NOT span
    /// the corner cells. Regression guard for the "title bar overwrites the corner runes"
    /// defect (a full-bleed `n` / `s` span, col 0 · span 3). Builds a real `frame` with
    /// `nw` + `n` + `center` regions through `run_ui` and asserts the laid-out `n` rect
    /// begins EXACTLY at the `w` edge track (where the `nw` corner cell ends), so the title
    /// zone and the corner-rune zone are distinct, non-overlapping rectangles.
    #[test]
    fn frame_title_bar_sits_in_its_own_cell_clear_of_the_corners() {
        use crate::template::{builtin_templates, expand};
        // A `box`-styled panel that fills whatever cell it lands in (no own size).
        let box_panel = || prop(node("cell"), "style", Value::Text("box".into()));

        let mut frame = node("");
        frame.component = String::new();
        frame.template = Some("frame".into());
        frame.props.insert("w".into(), Value::Number(400.0));
        frame.props.insert("h".into(), Value::Number(300.0));
        frame.props.insert("edge".into(), Value::Number(30.0));
        frame.props.insert("n_size".into(), Value::Number(52.0));
        frame.slots.insert("nw".into(), vec![box_panel()]);
        frame.slots.insert("n".into(), vec![box_panel()]);
        frame.slots.insert("center".into(), vec![box_panel()]);

        let built = expand(frame, &builtin_templates());
        let ps = panels(&run(&page_of(built)));

        // Emission order (nw, n, center) → ps[0] = nw corner, ps[1] = n title, ps[2] = center.
        // The frame self-anchors CENTRE, so its corner is offset — assertions are RELATIVE to nw.
        assert_eq!(ps.len(), 3, "the three box regions emit panels: {ps:?}");
        let (nw, n, center) = (ps[0], ps[1], ps[2]);
        // The nw corner cell is the w_size × n_size intersection: 30 × 52.
        assert!((nw.2 - 30.0).abs() < 1e-3 && (nw.3 - 52.0).abs() < 1e-3, "nw cell is 30×52: {nw:?}");
        // The title bar begins EXACTLY where the corner column ends (nw.x + w_size = nw.x + 30),
        // in the same top row (nw.y), and spans ONLY the centre column (width = 400 - 30 - 30).
        assert!((n.0 - nw.0 - 30.0).abs() < 1e-3, "n title starts at the w edge, clear of the nw corner: {n:?} vs {nw:?}");
        assert!((n.1 - nw.1).abs() < 1e-3, "n title shares the top row with nw: {n:?} vs {nw:?}");
        assert!((n.2 - 340.0).abs() < 1e-3, "n title spans only the centre column: {n:?}");
        assert!((n.3 - 52.0).abs() < 1e-3, "n title fills the top-row band (n_size): {n:?}");
        // Disjoint zones: the title's left edge meets the corner's right edge — no intrusion.
        assert!(n.0 >= nw.0 + nw.2 - 1e-3, "title zone and corner zone are disjoint: {n:?} vs {nw:?}");
        // Sanity: the centre is inset one edge track below + right of nw (the rune-cleared cell).
        assert!((center.0 - nw.0 - 30.0).abs() < 1e-3 && (center.1 - nw.1 - 52.0).abs() < 1e-3, "center inset past the edges: {center:?} vs {nw:?}");
    }

    /// Cross-axis `align` on a container sizes each child to its intrinsic cross extent
    /// and pins it (start/center/end) instead of stretching it to fill. Here a 200-wide
    /// column with a 60-wide child + `align=center` lands the child at x-offset 70.
    #[test]
    fn flow_align_center_uses_intrinsic_cross_size_and_centers() {
        let mut child = prop(node("cell"), "style", Value::Text("box".into()));
        child.width = Some(60.0); // intrinsic cross extent (column → cross is x)
        child.size = Some(20.0); // main-axis (height)
        let mut col = node("cell");
        col.id = "c".into();
        col.anchor = Some(UiAnchor::TopLeft);
        col.width = Some(200.0);
        col.height = Some(100.0);
        col = prop(col, "align", Value::Text("center".into()));
        col.children = vec![child];
        let ps = panels(&run(&page_of(col)));
        assert_eq!(ps.len(), 1, "one child box: {ps:?}");
        let (x, _, w, _) = ps[0];
        assert!((w - 60.0).abs() < 1e-3, "child keeps its intrinsic 60 width (not stretched): {ps:?}");
        assert!((x - 70.0).abs() < 1e-3, "centered in 200 → x offset (200-60)/2 = 70: {ps:?}");
        // Default (no align) STRETCHES to the full 200, proving align is opt-in / non-breaking.
        let mut stretch = node("cell");
        stretch.id = "s".into();
        stretch.anchor = Some(UiAnchor::TopLeft);
        stretch.width = Some(200.0);
        stretch.height = Some(100.0);
        let mut k = prop(node("cell"), "style", Value::Text("box".into()));
        k.size = Some(20.0);
        stretch.children = vec![k];
        let sp = panels(&run(&page_of(stretch)));
        assert!((sp[0].2 - 200.0).abs() < 1e-3, "no align → child fills cross (200): {sp:?}");
    }

    /// `cell` is THE layout box — the one vertical-flow engine, and
    /// transparent until it carries a style. Two children flow as a column; the unstyled
    /// cell emits no bg of its own, a styled cell draws one.
    #[test]
    fn cell_is_a_transparent_flow_box_until_styled() {
        let kid = || {
            let mut k = prop(node("cell"), "style", Value::Text("box".into()));
            k.size = Some(30.0);
            k
        };
        let mut c = node("cell");
        c.id = "cell".into();
        c.anchor = Some(UiAnchor::TopLeft);
        c.width = Some(100.0);
        c.height = Some(100.0);
        c.children = vec![kid(), kid()];
        let ps = panels(&run(&page_of(c)));
        assert_eq!(ps.len(), 2, "unstyled cell draws no bg — only its two children: {ps:?}");
        assert!((ps[0].1).abs() < 1e-3 && (ps[1].1 - 30.0).abs() < 1e-3, "children flow as a column (y 0, 30): {ps:?}");

        let mut styled = node("cell");
        styled.id = "s".into();
        styled.anchor = Some(UiAnchor::TopLeft);
        styled.width = Some(100.0);
        styled.height = Some(40.0);
        styled = prop(styled, "style", Value::Text("box".into()));
        let ps2 = panels(&run(&page_of(styled)));
        assert_eq!(ps2.len(), 1, "a styled cell draws its carved-stone bg: {ps2:?}");
    }

    // Keep HashMap import used even if the struct-literal path changes.
    #[allow(dead_code)]
    fn _uses_hashmap() -> HashMap<String, Value> {
        HashMap::new()
    }
}
