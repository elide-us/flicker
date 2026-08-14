//! The **Rust component walker** — the engine half of the component-UI model.
//!
//! A screen declares a tree of [`UiNode`]s (via its Lua `tree()` builder, parsed
//! by `flicker-script`); every control's BEHAVIOUR — draw, hit, geometry — lives
//! HERE, in the `draw_<kind>` arms and [`rust_hit_shape`] / [`rust_owns_hit`].
//! This module also owns the generic machinery around them: it lays the tree out
//! into rects, keeps the RETAINED draw cache (structural fingerprints — an
//! unchanged node replays its cached commands), applies each [`HitVerdict`]
//! generically (claims / captures / focus / popups), routes results, and draws
//! the STRUCTURAL primitives — `text`, styled-container backgrounds, `list`
//! layout + clip. A component's draw emits plain [`HudCommand`]s; the walker
//! collects them into [`UiFrame::commands`] for the existing
//! [`render_hud`](crate::render_hud). Interaction rides two-way name channels: a
//! node's `bind` ↔ a `Model` key (values), its `action` → an event name, both
//! returned in the [`UiFrame::results`] `ValueMap`.
//!
//! For a stretch of 2026-07/08 each control's draw + hit lived in a `ui/<kind>.lua`
//! module the walker dispatched to across a `ComponentLibrary` seam, and the draw
//! cache existed to BOUND those crossings. Every control is back in the engine, the
//! module tier is deleted, and the seam with it (2026-08-10) — the cache stays,
//! because bounding redraws is a win of its own.
//!
//! Components read their colours/sizes from the resolved `ui_theme.json` by a
//! dotted `style` path (`"paperdoll.fit.slider"`) — so the palette stays in one
//! place (Prism `theme.tokens`) and a node carries only its truly-local data.
//!
//! A COMPONENT's draw/layout/hit belongs HERE, in the engine — Aaron's ratified
//! taxonomy (9C141E1C) says *"the walker's per-control draw + hit + bind code IS
//! that Component's logic"*, and the 2026-08-09 ruling (BF0AF0C9) restored that
//! after the 2026-07-30 inversion moved the controls into `ui/<kind>.lua`. Lua
//! ORCHESTRATES: it positions nodes, names binds and actions, carries config —
//! it does not own semantics. A new control joins `RUST_COMPONENT_KINDS`, gains a
//! `draw_<kind>` arm here, and answers its hit either with a trivial
//! [`rust_hit_shape`] or a bespoke arm ([`rust_owns_hit`]). There is no other
//! tier to put one in.

use std::collections::{HashMap, HashSet};

use flicker_render::Vec2;
use flicker_script::{FontRole, HudCommand, TextAlign, UiAnchor, UiNode, Value, ValueMap};
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
    /// The plain-data props the last real draw marshalled for a COMPONENT (`None`
    /// for the structural kinds, which build none). The hit arm reuses these —
    /// patching only the live fields (`bind_value`/`open`/`captured`) — instead of
    /// rebuilding the whole map per hit call on an unchanged node.
    props: Option<Json>,
    /// Last frame this entry was used, for eviction.
    touched: u64,
}

/// The retained per-node draw cache — **draw-on-change**: a node re-runs its draw
/// only when one of its inputs actually changed, so a still frame emits nothing and
/// replays instead of rebuilding every node's commands.
///
/// It was built to bound crossings into the `ui/<kind>.lua` tier (deleted
/// 2026-08-10) and outlived it, because the saving was never really about Lua: a
/// draw marshals a props map and formats strings, and `text` — the most numerous
/// kind in a real tree — did that every frame for a label that almost never changes.
#[derive(Default)]
struct DrawCache {
    entries: HashMap<u64, CacheEntry>,
    /// Monotonic frame counter driving eviction.
    frame: u64,
}

/// How much work one [`run_ui`] pass actually did — the observable that keeps the
/// draw-on-change guarantee honest (a test asserts `redraw_nodes == 0` on a still
/// frame). Counters, not timings, so they cost nothing in a release build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiStats {
    /// Nodes redrawn this frame — the rest replayed from the cache.
    pub redraw_nodes: u32,
    /// Nodes laid out this frame (the denominator for the above).
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
    /// A captured drag's value-in-flight — the **commit-on-release** contract
    /// (Aaron, 2026-08-06): while a slider drag holds the pointer, the live value
    /// feeds only the DRAW (the knob tracks the hand); `frame.results` keeps
    /// reporting the resting model value, and the single results write happens on
    /// the release frame. `(bind, live value, the control's min — the echo
    /// default the resting report falls back to)`.
    drag_value: Option<(String, Value, f64)>,
    /// **Local display ownership** (Aaron, 2026-08-07) — what a control SHOWS after the
    /// user has committed to it, held here rather than re-derived from the scene's
    /// model every frame. `bind -> (the committed value, the model's value when it was
    /// committed)`.
    ///
    /// A control is not a view onto a Model key: it emits ONE event when a human moves
    /// it, and it accepts a change when the underlying value itself moves. Without
    /// this, the displayed position round-trips through the scene — the control shows
    /// the stale model value on every frame the scene has not folded the edit back, and
    /// a scene that publishes the key conditionally drags the control backwards.
    ///
    /// Both exits are self-clearing, which is what keeps the map the size of "controls
    /// the user has touched recently" rather than the size of the tree:
    /// * the model arrives at the committed value — the scene agreed, nothing left to
    ///   hold;
    /// * the model moves to something ELSE — an external change outranks the local
    ///   edit, and the control follows the authority (this is also what makes a scene's
    ///   clamp or snap visible instead of fighting the control).
    local: HashMap<String, (Value, Option<Value>)>,
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
    /// text_field re-establishes it through that component's `focus` verdict
    /// ([`hit_text_field`] → [`apply_hit_verdict`]) — `Default` starts `None`.
    focus: Option<String>,
    /// The pointer has taken over the input modality: REAL mouse activity
    /// (movement / click / wheel — not the first frame, not a resize) sets it;
    /// any routed nav-family signal (d-pad, arrows, tab, confirm — see the
    /// walker's `note_nav_input`) clears it. While `false` (nav mode — the
    /// entry state, so a scene's seeded default focus lights immediately) the
    /// focused node draws hot; while `true` only the pointer does. `pub(crate)`
    /// so the walker layer flips it without a second state home.
    pub(crate) pointer_mode: bool,
    /// Previous frame's `(mouse, screen)` — the "did the pointer actually move" test
    /// the modality takeover reads. Screen rides along so a resize is not mistaken for
    /// a move. `None` (first frame) counts as unmoved.
    last_mouse: Option<(Vec2, Vec2)>,
    /// Pad nudges recorded by the walker layer for the FOCUSED slider —
    /// `(node id, direction ±1, coarse)` — applied by the next `run_ui` pass,
    /// which is where the node's `step`/`min`/`max` live. THE component-level
    /// controller channel every slider gets for free: d-pad on the slider's
    /// own axis steps it, no scene wiring.
    nudges: Vec<(String, i32, bool)>,
    /// Result NAMES the walker layer drained last frame — applied by the next
    /// `run_ui` pass, where an option strip (`tabs` / `pill_toggle`) naming one
    /// as its `next_action` / `prev_action` advances its OWN bind by ±1, CLAMPED
    /// at the ends (a linear rail never wraps). Same channel shape as [`Self::nudges`], the same reason: the
    /// NODE owns the range (here, its own children count), so the step is
    /// computed where that lives. THE component-level stepping every option
    /// strip gets for free — a click, a shoulder press and a pad Confirm on the
    /// rail's hint button all converge on one numeric index write.
    steps: Vec<String>,
    /// The chord modifier is currently held (`ChordBegin` press…release) —
    /// OBSERVED by the walker layer, never consumed, so chord verbs elsewhere
    /// keep working. A held chord scales a nudge to the coarse step.
    pub(crate) chord: bool,
    /// Pane LOCK — the four-context pane model (MCP 0EFF5464). While `false` the LEFT
    /// STICK cycles panes (`PanelNext`/`PanelPrev`) and a pane feeds its interior
    /// nothing; a `Confirm` on a pane (an actionless focused container) ENTERS it, and
    /// the left stick then belongs to that pane's interior — its viewport camera, which
    /// the scene gates on [`entered`](Self::entered). `Cancel` exits. The d-pad (`Nav*`)
    /// is the in-pane cursor and stays live in BOTH states (menus navigate with it).
    pub(crate) entered: bool,
    /// Live press-feedback flashes, `action/result name → intensity 0..1`
    /// (Aaron, 2026-08-08: *"the icons should briefly glow … to indicate the
    /// click … Even if it does nothing, the visual cue is important UX"* — and
    /// his follow-up ruling: this is a BUTTON behaviour, not a special kind).
    /// Lit wherever an ACTION fires — a click in the hit pass, an activate
    /// verdict, a pad `Confirm` on the focused node, or a declared signal
    /// firing the same result name in the walker layer — and every control
    /// whose `action` matches reads the intensity back as a `flash` draw prop.
    /// A `Vec` because the live set is at most a few entries — a map's hashing
    /// would cost more than the scan.
    flashes: Vec<(String, f32)>,
    /// Action NAMES a POINTER click activated this frame (a button/toggle/context
    /// row the hit pass fired). Recorded by [`run_ui`]'s hit pass, drained by the
    /// walker's [`take_fired`](crate::WalkerHandler::take_fired) — the ONE
    /// activation channel — so a click and a pad `Confirm` converge on the same
    /// `sig_<name>` mirror (rule 37722F91 "all input events are signals"; pump P2,
    /// MCP `0569DA9B`). Cleared at the top of every `run_ui` pass, so a scene that
    /// never runs a walker (never drains) can accumulate at most one frame of
    /// clicks rather than leaking.
    fired_pointer: Vec<String>,
}

/// How much a press flash fades per frame (per [`UiState::flash_tick`]): full
/// glow to gone in 15 ticks ≈ a quarter second at 60 Hz — long enough to read,
/// short enough that mashing a rail reads as pulses rather than a held light.
const FLASH_DECAY: f32 = 1.0 / 15.0;

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

    /// The pane LOCK state (MCP 0EFF5464): `true` once a `Confirm` has ENTERED the
    /// focused pane, so the left stick belongs to that pane's interior. A multi-pane
    /// scene gates its viewport camera on `entered() && focused() == <pane id>`;
    /// `Cancel` clears it. Navigating between panes is only possible while `false`.
    pub fn entered(&self) -> bool {
        self.entered
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

    /// Light the press flash for `key` (an action/result name, e.g. `page_next`)
    /// at full intensity. Idempotent while lit: a repeat press restarts the fade
    /// rather than stacking a second entry.
    pub fn flash(&mut self, key: &str) {
        match self.flashes.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = 1.0,
            None => self.flashes.push((key.to_string(), 1.0)),
        }
    }

    /// One tick of the fade: every live flash loses [`FLASH_DECAY`]; spent ones
    /// leave the set. Called once per frame by the walker's `take_fired` — the
    /// same once-per-dispatch cadence that records them.
    pub fn flash_tick(&mut self) {
        for (_, v) in &mut self.flashes {
            *v -= FLASH_DECAY;
        }
        self.flashes.retain(|(_, v)| *v > 0.0);
    }

    /// The current intensity of `key`'s flash — `0.0` when not lit.
    pub fn flash_intensity(&self, key: &str) -> f32 {
        self.flashes.iter().find(|(k, _)| k == key).map_or(0.0, |(_, v)| *v)
    }

    /// Record a pad nudge for the focused slider `id` (`dir` +1 toward max,
    /// −1 toward min; `coarse` = the chord modifier was held). Applied — and
    /// stepped, clamped, written to the bind — by the next `run_ui` pass.
    pub(crate) fn push_nudge(&mut self, id: &str, dir: i32, coarse: bool) {
        self.nudges.push((id.to_string(), dir, coarse));
    }

    /// Record a result NAME the walker layer fired, for the next `run_ui` pass to
    /// offer to every option strip as a possible `next_action` / `prev_action`
    /// (see [`Self::steps`]). A name no strip claims steps nothing — drained
    /// either way, so stale names never accumulate.
    pub(crate) fn push_step(&mut self, name: &str) {
        self.steps.push(name.to_string());
    }

    /// Record that a POINTER click activated action `name` this frame — the hit
    /// pass calls this wherever it fires an `action` (the `Rect` arm, an `activate`
    /// verdict, a context-menu child). The walker's `take_fired` drains it, so the
    /// click rides the ONE activation channel to the `sig_<name>` mirror exactly
    /// like a pad `Confirm` (rule 37722F91; pump P2).
    pub(crate) fn record_pointer_fire(&mut self, name: &str) {
        self.fired_pointer.push(name.to_string());
    }

    /// Drain this frame's pointer activations (see [`Self::record_pointer_fire`]).
    /// Called by the walker's `take_fired` — the pointer names it appends to the
    /// one drain the scene reads.
    pub(crate) fn take_pointer_fired(&mut self) -> Vec<String> {
        std::mem::take(&mut self.fired_pointer)
    }

    /// Whether the LAST input was nav-family (d-pad / arrows / tab / confirm)
    /// rather than the pointer. This is the modality that decides whether the
    /// focused node lights as hot — and the value a scene publishes into its
    /// Model (e.g. `pad_mode`) so screens can swap key-hint labels per device.
    pub fn nav_mode(&self) -> bool {
        !self.pointer_mode
    }

    /// Record a routed nav-family signal: nav modality resumes, so the focused
    /// node lights again. Called by the walker's nav handler on every signal it
    /// routes.
    pub(crate) fn note_nav_input(&mut self) {
        self.pointer_mode = false;
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
    /// Every ID'd node's RESOLVED rect (`[x, y, w, h]`), in placement order —
    /// what the layout actually gave each named control this frame. The
    /// twice-burned lesson behind it: a control can be perfectly formed in the
    /// TREE and still resolve to zero pixels, invisible and unclickable, with
    /// every tree-shape gate green. A scene gate walks its real surface through
    /// `run_ui` and asserts [`rect`](Self::rect) — presence AND extent — for
    /// each control a user must be able to see and hit.
    pub rects: Vec<(String, [f32; 4])>,
    /// How much drawing this pass actually did — see [`UiStats`].
    pub stats: UiStats,
}

impl UiFrame {
    /// The rect the walker reserved for the `rtt` node with this `id`, as the
    /// render-crate rect an offscreen pass composites into. `None` while the
    /// viewport is off screen — which is also what lets a scene skip the pass
    /// entirely that frame.
    ///
    /// This is THE hand-off for an RTT viewport (the walker reserves, the scene
    /// fills), so it lives here rather than as a find/map dance every scene
    /// repeats and one of them eventually gets subtly wrong.
    pub fn rtt_rect(&self, id: &str) -> Option<flicker_render::Rect> {
        self.rtts.iter().find(|s| s.id == id).map(|s| flicker_render::Rect {
            pos: Vec2::new(s.x, s.y),
            size: Vec2::new(s.w, s.h),
        })
    }

    /// The rect the layout RESOLVED for the id'd node this frame — any node,
    /// not only `rtt` (see [`rects`](Self::rects)). `None` when the node is
    /// absent or hidden; a `Some` of zero extent is a control that exists and
    /// cannot be seen or clicked, which is exactly what a surface gate asserts
    /// against.
    pub fn rect(&self, id: &str) -> Option<flicker_render::Rect> {
        self.rects.iter().find(|(n, _)| n == id).map(|(_, r)| flicker_render::Rect {
            pos: Vec2::new(r[0], r[1]),
            size: Vec2::new(r[2], r[3]),
        })
    }
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
    /// Accumulated ink fade inherited down the tree (1.0 = none) — a container's
    /// `faded` toggle multiplies every DESCENDANT command's alpha at assembly time
    /// (the cut-clipboard marking, a dimmed pane). Applied OUTSIDE the DrawCache, so
    /// cached commands stay unfaded and a fade flip never invalidates an entry.
    fade: f32,
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
/// `ui_theme.json` (colours/sizes by dotted `style` path), `input` the
/// pointer snapshot, `state` the retained drag capture. Returns the draw
/// commands + the results `ValueMap`. Every component draws AND hit-tests in this
/// module — there is no other tier to dispatch to.
pub fn run_ui(
    tree: &UiNode,
    model: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &mut UiState,
) -> UiFrame {
    let screen = Rect { x: 0.0, y: 0.0, w: input.screen.x, h: input.screen.y };
    let mut placed = Vec::new();
    resolve(tree, screen, model, 0.0, 1.0, None, child_key(0, tree, 0), &mut placed);

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
    // Commit-on-release rides the same edge: the value the captured drag was holding
    // back lands in `results` exactly once, here — before the hit pass, so
    // `echo_binds` sees the key as written and the scene folds the one real change
    // this frame.
    // Local display ownership (see `UiState::local`): a control the user has committed
    // to keeps SHOWING what they set, so the display never waits on the scene to fold
    // the edit back. Seeded into `results` before anything else reads it — the same
    // seam commit-on-release uses — so `eff_value`, `echo_binds` and the draw all
    // follow it with no further plumbing. Both retain arms drop the entry the moment
    // it stops carrying information, so the map self-empties.
    state.local.retain(|bind, (val, seen)| {
        let now = model.get(bind);
        // The scene arrived at the committed value: it agrees, so there is nothing
        // left to hold and the ordinary model path takes over again.
        // The model moved somewhere ELSE: an external change outranks the local edit.
        now != Some(&*val) && now == seen.as_ref()
    });
    for (bind, (val, _)) in &state.local {
        results.set(bind.clone(), val.clone());
    }
    if !input.down {
        if let Some((bind, val, _)) = state.drag_value.take() {
            results.set(bind, val);
        }
        state.dragging.clear();
    }
    // Pointer takeover: REAL pointer activity — movement, a click, a wheel tick,
    // but not the first frame and not a resize — flips the modality so the
    // nav-focus highlight yields to hover. A nav signal flips it back
    // (`note_nav_input`), which is what makes the seeded first button light on
    // entry, stop hijacking hover the moment the mouse moves, and relight the
    // instant the d-pad speaks again.
    if input.clicked
        || input.wheel != 0.0
        || matches!(state.last_mouse, Some((m, _)) if m != input.mouse)
    {
        state.pointer_mode = true;
    }
    state.last_mouse = Some((input.mouse, input.screen));
    // This frame's pointer activations start empty — the hit pass below records
    // each fired `action` (the walker drains them into the one `sig_<name>` mirror,
    // rule 37722F91 / pump P2). Cleared here, not in `take_fired`, so a scene that
    // runs no walker cannot accumulate stale clicks across frames.
    state.fired_pointer.clear();
    for p in &placed {
        hit_node(p, model, input, state, styles, &mut results, &mut hud_hit);
    }
    // The generic every-frame TYPED-FOLD: this frame's keyboard input flows into the
    // FOCUSED node's bound string, in Rust, whatever the pointer is doing — see
    // [`fold_typed`]. After the hit pass (a click that just focused a field folds the
    // same frame), before the echo (an edit must never be shadowed).
    fold_typed(&placed, model, input, state, &mut results);
    // Take local ownership of anything a human just committed — after every write
    // channel (hit verdicts, the release commit, typed text) and BEFORE the echo, so
    // only real edits are seen here and an echo default can never be mistaken for one.
    record_local(&placed, model, &results, state);
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
        // Does this node's draw read the pointer/focus at all? Every interactive
        // COMPONENT does — it receives `hot`/`pressed`/`focused`/`mx`/`my` (which keeps
        // a pointer-live kind like context_menu hover-fresh, a text_field's ring/caret
        // focus-fresh, and a button's hover/press/flash states live). Structural boxes
        // never consult the cursor.
        let hot_matters = crate::is_rust_component(&p.node.component);
        // Fast path — an entry whose every input is unchanged replays verbatim. The
        // fingerprint is folded against the entry's OWN read-key list, borrowed in
        // place, so a still frame allocates nothing and redraws nothing.
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
            let start = commands.len();
            commands.extend_from_slice(&e.commands);
            e.touched = frame;
            // The inherited `faded` dim is applied on the way OUT of the cache, so a
            // fade flip re-tints a replayed entry without ever invalidating it.
            if p.fade < 1.0 {
                fade_commands(&mut commands[start..], p.fade);
            }
            continue;
        }
        // Miss: draw for real and rebuild the entry. The read-key list is recomputed
        // here rather than reused because a tree rebuilt from scratch each frame (the
        // Loomforge bench, the chat panel) may hand this identity different props.
        let read_keys = read_keys_of(p.node);
        let fp =
            node_fingerprint(p, st, styles, &read_keys, model, &results, input, state, hot_matters);
        let start = commands.len();
        let props = draw_node(p, model, &results, styles, input, state, &mut commands);
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
        // Cache first, fade second: the entry holds the UNFADED run (fade is not a
        // fingerprint input), and this frame's emission dims in place after.
        if p.fade < 1.0 {
            fade_commands(&mut commands[start..], p.fade);
        }
    }
    // Evict what this frame did not touch, but only once the map has grown well past
    // the live tree — a screen that toggles between two panels should keep both cached,
    // while a tree that structurally churns must not leak.
    if cache.entries.len() > 2 * placed.len().max(16) {
        cache.entries.retain(|_, e| frame.wrapping_sub(e.touched) < 120);
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

    // Pad nudges — the walker layer's slider channel: step the FOCUSED
    // slider's bind by the NODE's own `step` (`step_coarse` under the chord),
    // clamped to its range, written as a committed result exactly like a
    // stepper click (discrete gesture → immediate write; commit-on-release is
    // for drags). Applied HERE because the node owns step/min/max, and LAST
    // among the writes so the every-frame echo cannot put the resting value
    // back over the step. A nudge for a hidden or vanished slider steps
    // nothing — drained either way, so stale presses never accumulate.
    for (id, dir, coarse) in std::mem::take(&mut state.nudges) {
        let Some(p) =
            placed.iter().find(|p| p.node.component == "slider" && p.node.id == id)
        else {
            continue;
        };
        let Some(bind) = p.node.bind.as_deref() else { continue };
        let min = pnum(p.node, "min").unwrap_or(0.0);
        let max = pnum(p.node, "max").unwrap_or(1.0);
        let fine = pnum(p.node, "step").unwrap_or(1.0);
        let step = if coarse { pnum(p.node, "step_coarse").unwrap_or(fine * 10.0) } else { fine };
        let cur = results.number(bind).or_else(|| model.number(bind)).unwrap_or(min);
        results.set(bind.to_string(), (cur + f64::from(dir) * step).clamp(min, max));
    }

    // Strip stepping — THE OPTION STRIP OWNS ITS OWN STEPPING. A `tabs` /
    // `pill_toggle` authoring `next_action` / `prev_action` advances its own bind
    // by ±1 whenever the walker layer fired that result name (a shoulder signal,
    // a pad Confirm on the rail's hint button, a bound key — all one channel).
    // The step CLAMPS at the ends — next stops on the last entry, prev on the first;
    // a linear rail must NEVER wrap (right→leftmost / left→rightmost is an
    // unexpected-UX anti-pattern, Aaron 2026-08-12). The bound is the strip's OWN
    // children count, so a rail never learns how many entries it has and no scene owns
    // a stepper. Applied HERE, beside the slider nudges and after the echo, for the
    // same reason: the NODE owns the range, and the every-frame echo must not put the
    // resting index back over the step.
    for name in std::mem::take(&mut state.steps) {
        for p in &placed {
            if !matches!(p.node.component.as_str(), "tabs" | "pill_toggle") {
                continue;
            }
            let dir = match (ptext(p.node, "next_action"), ptext(p.node, "prev_action")) {
                (Some(n), _) if n == name => 1.0,
                (_, Some(n)) if n == name => -1.0,
                _ => continue,
            };
            let Some(bind) = p.node.bind.as_deref() else { continue };
            let len = p.node.children.len() as f64;
            if len <= 0.0 {
                continue;
            }
            let cur = results.number(bind).or_else(|| model.number(bind)).unwrap_or(0.0);
            // CLAMP at the ends — next stops on the last entry, prev on the first; a
            // linear rail must NOT wrap (right→leftmost / left→rightmost is an
            // unexpected-UX anti-pattern, Aaron 2026-08-12).
            results.set(bind.to_string(), (cur + dir).clamp(0.0, len - 1.0));
        }
    }

    // Every id'd node's resolved rect, for `UiFrame::rect` — the layout's own
    // answer to "did this control get pixels", harvestable by scene gates.
    let rects: Vec<(String, [f32; 4])> = placed
        .iter()
        .filter(|p| !p.node.id.is_empty())
        .map(|p| (p.node.id.clone(), [p.rect.x, p.rect.y, p.rect.w, p.rect.h]))
        .collect();

    // Commit-on-release, the held half: the draw above consumed the live in-flight
    // value (the knob follows the hand), but the SCENE must keep seeing the resting
    // value until the button releases — so the live write is replaced by the model
    // echo (the same default `echo_binds` uses) before `results` leave the walker.
    if input.down {
        if let Some((bind, _, min)) = state.drag_value.as_ref() {
            results.set(bind.clone(), model.number(bind).unwrap_or(*min));
        }
    }

    UiFrame { commands, results, rtts, rects, stats }
}

// ── Layout ───────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn resolve<'a>(
    node: &'a UiNode,
    rect: Rect,
    model: &ValueMap,
    layer: f32,
    fade: f32,
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
    // `faded` accumulates the same way: a faded container dims its whole subtree.
    let fade = fade * node_fade(node, model);
    out.push(Placed { node, rect, enabled: enabled(node, model), layer, fade, clip, key });
    if node.children.is_empty() || no_descend(&node.component) {
        return;
    }
    let inner = rect.inset_xy(pad_x(node), pad_y(node));
    match node.component.as_str() {
        // A `list` (scrolling region): children flow as a column shifted up by the
        // bound offset, and the whole subtree is clipped to the viewport (`inner`).
        // Content taller than the viewport scrolls. This LAYOUT is a structural
        // primitive and lives here; the region's own draw (backdrop + scrollbar) and
        // hit (claim + wheel→offset) are the COMPONENT's, in `draw_list`/`hit_list`.
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
            flow(node, content, model, layer, fade, view, key, out, false);
        }
        "row" => flow(node, inner, model, layer, fade, clip, key, out, true),
        // `cell` is the generic layout BOX (a "div") — same vertical-flow engine as
        // `cell` is THE box — one vertical-flow engine, one name. (It absorbed `column`
        // and `panel`: a vertical list is a `cell`, and a carved-stone panel is a `cell`
        // carrying a `style`. `row` remains only because its axis genuinely differs.)
        // `panel` lays out exactly like `cell`: it IS a vertical box that happens to
        // own a backdrop + a focus rim. Without this arm the walker would anchor-
        // overlay its children (the generic fall-through), and a pane's contents
        // would stack on top of one another instead of flowing down it.
        "cell" | "panel" => flow(node, inner, model, layer, fade, clip, key, out, false),
        // A 2-D track grid — the CSS-Grid generalisation of `flow` (see the Grid
        // section). Must sit before the `_` catch-all so its children are placed
        // into cells rather than anchor-overlaid.
        "grid" => grid_arrange(node, inner, model, layer, fade, clip, key, out),
        // The carved modal slab: its authored `children` are the ITEMS (buttons/rows the
        // scene supplies), flowed vertically below the drawn title block. The title /
        // subtitle / divider / footer are CHROME the component draws (not placed nodes),
        // so only the items are placed here — starting at `items_top`, spaced by
        // `items_gap`, keyed by each item's original sibling index.
        "popup_panel" => {
            let c = popup_chrome(node, rect);
            let mut y = c.items_top;
            for (i, child) in node.children.iter().enumerate().filter(|(_, c)| visible(c, model)) {
                let mh = child_main(child, model, false);
                let r = Rect { x: c.inner_x, y, w: c.inner_w, h: mh };
                resolve(child, r, model, layer, fade, clip, child_key(key, child, i), out);
                y += mh + c.items_gap;
            }
        }
        // The two-rail page/tab control (PTT): a vertical column of [page rail][rule]
        // [tab rail][content]. The rails are AUTHORED child components (the first `tabs`
        // = page rail, the first `pill_toggle` = tab rail) that the walker PLACES at the
        // band rects; every other child flows in the content region. The gutter hints and
        // the rule are chrome the component draws, not placed nodes.
        "paged_menu" => {
            let lay = paged_layout(node, rect, model);
            let mut content: Vec<(usize, &UiNode)> = Vec::new();
            let mut tabs_done = false;
            let mut pills_done = false;
            for (i, child) in node.children.iter().enumerate() {
                if !visible(child, model) {
                    continue;
                }
                match child.component.as_str() {
                    "tabs" if !tabs_done => {
                        tabs_done = true;
                        if let Some(rail) = lay.page_rail {
                            resolve(child, rail, model, layer, fade, clip, child_key(key, child, i), out);
                        }
                    }
                    "pill_toggle" if !pills_done => {
                        pills_done = true;
                        if let Some(pill) = lay.tab_pill {
                            resolve(child, pill, model, layer, fade, clip, child_key(key, child, i), out);
                        }
                    }
                    _ => content.push((i, child)),
                }
            }
            flow_kids(node, &content, lay.content, model, layer, fade, clip, key, out, false);
        }
        // page / stack / anything else: overlay children, each placed by its own anchor.
        _ => {
            // Index over ALL children (not just the visible ones) so a sibling toggling
            // its visibility never renumbers — and therefore never re-keys — the rest.
            for (i, c) in node.children.iter().enumerate() {
                if !visible(c, model) {
                    continue;
                }
                let r = anchored(c, inner, model);
                resolve(c, r, model, layer, fade, clip, child_key(key, c, i), out);
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
    fade: f32,
    clip: Option<[f32; 4]>,
    key: u64,
    out: &mut Vec<Placed<'a>>,
    horizontal: bool,
) {
    // Carry each visible child's index among ALL siblings, so its cache key is stable
    // when a sibling above it hides.
    let kids: Vec<(usize, &UiNode)> =
        node.children.iter().enumerate().filter(|(_, c)| visible(c, model)).collect();
    flow_kids(node, &kids, area, model, layer, fade, clip, key, out, horizontal);
}

/// The flow engine over an EXPLICIT (index, child) list — so a composite that owns a
/// SUBSET of its children (a `paged_menu`'s content region, once its rails are placed
/// separately) can share the same grow/align/gap distribution, with each child keeping
/// its own `child_key` from its ORIGINAL sibling index. [`flow`] is the whole-`children`
/// caller. Gap and align read from `node`, exactly as before.
#[allow(clippy::too_many_arguments)]
fn flow_kids<'a>(
    node: &'a UiNode,
    kids: &[(usize, &'a UiNode)],
    area: Rect,
    model: &ValueMap,
    layer: f32,
    fade: f32,
    clip: Option<[f32; 4]>,
    key: u64,
    out: &mut Vec<Placed<'a>>,
    horizontal: bool,
) {
    let n = kids.len();
    let main = if horizontal { area.w } else { area.h };
    let cross = if horizontal { area.h } else { area.w };

    // A no-grow child's main extent: an `aspect` (w÷h) prop derives it from the
    // CROSS extent — a square viewport in a row is exactly as wide as the row
    // is tall, with no wrapper geometry — otherwise its measured size.
    let main_of = |c: &UiNode| match pnum(c, "aspect") {
        Some(a) if horizontal => cross * a as f32,
        Some(a) => cross / (a as f32).max(1e-6),
        None => child_main(c, model, horizontal),
    };

    let mut fixed = 0.0;
    let mut grow_total = 0.0;
    for (_, c) in kids {
        match c.grow {
            Some(g) => grow_total += g,
            None => fixed += main_of(c),
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
    for (i, c) in kids {
        let len = match c.grow {
            Some(g) if grow_total > 0.0 => free * g / grow_total,
            Some(_) => 0.0,
            None => main_of(c),
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
        resolve(c, r, model, layer, fade, clip, child_key(key, c, *i), out);
        pos += len + node.gap;
    }
}

/// A `list` region's intrinsic content height — its visible children stacked as a
/// column (pad + inter-child gaps + each child's main size). The basis for the max
/// scroll offset (`content_h - viewport_h`): `resolve` lays out and clamps with it,
/// the fingerprint folds it (a row appearing must invalidate the bar), and
/// `component_props` hands it to the component as `content_h` so [`draw_list`]'s
/// scrollbar and [`hit_list`]'s wheel clamp can never disagree with the placement.
fn scroll_content_h(node: &UiNode, model: &ValueMap) -> f32 {
    let kids: Vec<&UiNode> = node.children.iter().filter(|c| visible(c, model)).collect();
    let gaps = node.gap * kids.len().saturating_sub(1) as f32;
    pad_y(node) * 2.0 + gaps + kids.iter().map(|c| child_main(c, model, false)).sum::<f32>()
}

/// Place an absolutely-anchored node's box within `parent` (corner/edge + offset).
/// A `width_frac`/`height_frac` prop sizes the box as a fraction of the parent
/// rect — the flex-style constraint a full-screen backdrop or a viewport-tall Muse
/// needs, so the tree stays built-once and adapts to any window size at layout time.
/// An `aspect` (width÷height) prop LOCKS the ratio and derives the MISSING side
/// from the given one: a height-sized node keeps its width in step (the historical
/// contract), while a width-sized node derives its height — so a full-bleed plate
/// (`width_frac: 1.0`, the Muse) keeps its proportions and spills past the top and
/// bottom instead of stretching with the window. Height wins when both are given.
fn anchored(node: &UiNode, parent: Rect, model: &ValueMap) -> Rect {
    let m = measure(node, model);
    let h_given = node
        .height
        .or_else(|| pnum(node, "height_frac").map(|f| parent.h * f as f32));
    let w_given = node
        .width
        .or_else(|| pnum(node, "width_frac").map(|f| parent.w * f as f32));
    let (w, h) = match pnum(node, "aspect") {
        Some(aspect) => match (h_given, w_given) {
            (Some(h), _) => (h * aspect as f32, h),
            (None, Some(w)) => (w, w / (aspect as f32).max(1e-6)),
            (None, None) => (m.y * aspect as f32, m.y),
        },
        None => (w_given.unwrap_or(m.x), h_given.unwrap_or(m.y)),
    };
    let (a, off) = placement(node, model);
    let x = match a {
        UiAnchor::TopLeft | UiAnchor::Left | UiAnchor::BottomLeft => parent.x,
        UiAnchor::Top | UiAnchor::Center | UiAnchor::Bottom => parent.x + (parent.w - w) * 0.5,
        UiAnchor::TopRight | UiAnchor::Right | UiAnchor::BottomRight => parent.x + parent.w - w,
    } + off[0];
    let y = match a {
        UiAnchor::TopLeft | UiAnchor::Top | UiAnchor::TopRight => parent.y,
        UiAnchor::Left | UiAnchor::Center | UiAnchor::Right => parent.y + (parent.h - h) * 0.5,
        UiAnchor::BottomLeft | UiAnchor::Bottom | UiAnchor::BottomRight => parent.y + parent.h - h,
    } + off[1];
    Rect { x, y, w, h }
}

/// A node's placement — anchor + pixel offset — where a per-id ARRANGE BIND overrides the
/// value the JSON authored. A scene's Lua `arrange()` (and any per-user layout override)
/// publishes `<id>_anchor` / `<id>_off_x` / `<id>_off_y` into the model; when present they
/// win, so a player can move a component the scene file centred. An id-less node, or one
/// with no such bind, keeps its authored placement — unbound trees are unchanged. Only
/// [`anchored`] (absolutely-placed nodes) consults this, so the per-frame lookup touches a
/// handful of overlay nodes, not the whole tree.
fn placement(node: &UiNode, model: &ValueMap) -> (UiAnchor, [f32; 2]) {
    if node.id.is_empty() {
        return (node.anchor.unwrap_or(UiAnchor::TopLeft), node.offset);
    }
    let anchor = model
        .text(&format!("{}_anchor", node.id))
        .and_then(UiAnchor::from_name)
        .or(node.anchor)
        .unwrap_or(UiAnchor::TopLeft);
    let ox = model
        .number(&format!("{}_off_x", node.id))
        .map_or(node.offset[0], |n| n as f32);
    let oy = model
        .number(&format!("{}_off_y", node.id))
        .map_or(node.offset[1], |n| n as f32);
    (anchor, [ox, oy])
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
        "cell" | "panel" => {
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
        // The modal slab is an anchored overlay sized to its content: a fixed `panel_w`
        // wide and tall enough for the drawn title block + the stacked items + the footer.
        // Shares [`popup_chrome`] with resolve/draw so the reserved space can never drift.
        "popup_panel" => {
            let w = pnum(node, "panel_w").unwrap_or(404.0) as f32;
            let c = popup_chrome(node, Rect { x: 0.0, y: 0.0, w, h: 0.0 });
            let items: f32 = kids.iter().map(|k| child_main(k, model, false)).sum::<f32>()
                + c.items_gap * kids.len().saturating_sub(1) as f32;
            let footer = if c.has_footer { c.gap + text_line_h(c.footer_size) } else { 0.0 };
            // `items_top` (relative to y=0) already carries the top pad + title block +
            // the gap before the items; the footer block and bottom pad close it out.
            let h = node.height.unwrap_or(c.items_top + items + footer + c.pad);
            Vec2::new(node.width.unwrap_or(w), h)
        }
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
    fade: f32,
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
        resolve(k, r, model, layer, fade, clip, child_key(key, k, *i), out); // the child fills its cell
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
/// changed segment label). Rows such a control lays PAST its own rect (a context menu's
/// overflow) stay clickable for free: its arm in [`hit_node`] runs for every placed
/// node, so the component's own row math always gets the click.
fn no_descend(kind: &str) -> bool {
    matches!(kind, "tabs" | "pill_toggle" | "select" | "context_menu")
}

pub(crate) fn visible(node: &UiNode, model: &ValueMap) -> bool {
    match &node.visible_bind {
        Some(k) => model.is_on(k),
        None => true,
    }
}

/// A node's OWN fade contribution — the `faded` behavior toggle (any container or
/// component may author it; state usually rides `faded_bind`). Faded ⇒ the `fade`
/// factor (0.45, the pre-stage map's cut-clipboard marking); resting ⇒ 1.0. Visual
/// only: hit-testing, focus and nav are untouched — a cut row stays interactive.
fn node_fade(node: &UiNode, model: &ValueMap) -> f32 {
    let faded = match ptext(node, "faded_bind") {
        Some(k) => model.is_on(k),
        None => pbool(node, "faded"),
    };
    if faded {
        pnum(node, "fade").map(|n| n as f32).unwrap_or(0.45).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn enabled(node: &UiNode, model: &ValueMap) -> bool {
    match &node.enabled_bind {
        Some(k) => model.is_on(k),
        None => true,
    }
}

// ── Hit-test ─────────────────────────────────────────────────────────────────

/// The trivial hit geometries a component can declare in [`rust_hit_shape`], so the
/// walker answers its hover/claim generically instead of running a bespoke arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitShape {
    /// The whole node rect is the interactive region: the walker claims the pointer
    /// on rect-contains and, on a click inside, fires the node's `action` / toggles
    /// its bool `bind` generically (button, panel, tile, action_slot).
    Rect,
    /// Presentational: never claims the pointer, never interacts (sprite, tooltip,
    /// rune_corners, the read-out gauges).
    None,
}

/// What one component's hit arm decided for this frame's pointer — the plain-data
/// verdict [`apply_hit_verdict`] applies generically (each field maps onto
/// walker-owned state or the results map; the arm never touches either directly).
/// Every field is optional; `Default` is "nothing happened".
#[derive(Clone, Debug, Default, PartialEq)]
struct HitVerdict {
    /// The pointer is over this component's TIGHT region (claims `hud_hit`).
    hit: bool,
    /// New value for the node's `bind` (a click toggled/picked/dragged it).
    value: Option<Value>,
    /// Fire the node's `action` (a click landed on its activating region).
    activate: bool,
    /// `Some(true)` grabs pointer capture for the node (a slider drag starts);
    /// release-on-button-up stays the walker's generic rule.
    capture: Option<bool>,
    /// `Some(true)` opens this node's popup (`state.open`), `Some(false)` closes it.
    open: Option<bool>,
    /// `Some(true)` gives this node KEYBOARD focus (`state.focus` ← the node's `id`;
    /// a node with no id cannot hold focus, so it is a no-op there). `Some(false)` /
    /// `None` leave focus unchanged — CLEARING stays the walker's generic rule: any
    /// fresh click drops focus up front, and the clicked field re-claims it here.
    focus: Option<bool>,
    /// Write the node's `bind` into its `focus_group` key (slider focus).
    group_focus: bool,
    /// Fire the `action` of the node's CHILD at this index — **1-based**, matching
    /// the `props.children[i]` the component indexed to find the row (a context
    /// menu's items are data children of the menu node, so the MENU receives the
    /// dispatch and names the picked row here). `0`, out-of-range, or a child with
    /// no/empty `action` is a silent no-op in the walker.
    activate_child: Option<usize>,
    /// A LOUD complaint about the node's authored data, raised by the component's own
    /// arm and surfaced by the walker as a `tracing::warn!`. This is how authored data
    /// the component cannot act on — an option whose `value` is the wrong TYPE, say —
    /// says so instead of doing nothing (the fail-loud law for authored names). At
    /// most one per verdict.
    warn: Option<String>,
    /// Fire an ARBITRARY result name through the full activation channel — the twin of
    /// the generic full-rect arm's fire (`results.set` + flash + strip-step +
    /// pointer-mirror), for a component whose click activates a name that is NOT its own
    /// `action`. A `paged_menu`'s hint gutter fires the neighbouring rail's
    /// `prev_action`/`next_action`, so the rail steps itself on the very name a shoulder
    /// signal or a pad Confirm on that rail would carry — one channel.
    fire: Option<String>,
}

/// A node's retained-interaction identity — its `id`, else its `bind`, else `""`.
/// One rule for pointer capture (`state.dragging`) and the open popup
/// (`state.open`).
fn node_ident(node: &UiNode) -> &str {
    if !node.id.is_empty() {
        &node.id
    } else {
        node.bind.as_deref().unwrap_or("")
    }
}

fn hit_node(
    p: &Placed,
    model: &ValueMap,
    input: &UiInput,
    state: &mut UiState,
    styles: &Json,
    results: &mut ValueMap,
    hud_hit: &mut bool,
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
        // ── Rust components that answer their own HIT ────────────────────────
        // A checkbox's box, a toggle's pill and a radio's circle are each a SUB-RECT
        // of the node, computed from the component's own props — geometry
        // `rust_hit_shape` cannot express, so these kinds answer with a real verdict
        // here instead (see [`rust_owns_hit`], which is also what the roster gate
        // accepts in place of a declared shape). Their idle echo is `echo_binds`.
        // An OPTION STRIP answers here for the other reason: a `pill_toggle`'s well is
        // such a sub-rect too, and it, `tabs` and `select` all PICK the entry under the
        // pointer rather than firing an action — a decision no trivial shape can carry.
        // `select` also reaches PAST its rect (its popup lies below the field) and toggles
        // `state.open`, so it is answered here every frame rather than only when the rect
        // pre-filter would have let it through.
        // The VALUE CONTROLS are here for the third reason — a click means something
        // other than claim-and-fire. A `slider` claims its whole row and grabs group
        // focus, but only its grab band CAPTURES, and a captured one keeps mapping the
        // pointer into the bind between click edges (release and the echoes stay the
        // walker's generic rules). A `stepper` steps only on its two end cells. A
        // `text_field`'s region IS its rect, but its click takes KEYBOARD FOCUS where the
        // trivial `Rect` arm would fire an action and toggle a string bind to a bool —
        // the keyboard itself stays walker-generic (`fold_typed`, then `echo_binds`).
        // A `list` is the fourth reason: its claim IS the whole rect, but the gesture it
        // answers is the WHEEL, folded into the bound offset and clamped to the
        // walker-measured `content_h`. Its idle echo is `echo_binds` (number, default 0).
        // A `context_menu` is the fifth: only its authored rect CLAIMS, but its rows are
        // child data it stacks itself and may lay PAST that rect, so the click has to be
        // resolved against the component's own row math. Answering here — this arm runs
        // for every placed node — is what delivers those clicks.
        k if rust_owns_hit(k) => {
            // ONE prop surface with the draw: `component_hit_props` hands back the
            // draw cache's own map with `bind_value` patched to this instant, so the
            // hit measures its box from exactly the props the box was drawn with —
            // including an authored `box_bind`, which reading `node.props` would drop.
            let props = component_hit_props(p, model, results, input, state, styles);
            // The click edge is pre-gated on `enabled`: a disabled control still hovers
            // (claims) but never writes.
            let click = input.clicked && p.enabled;
            // One arm, one dispatch table — the props assembly above is identical for
            // every kind, exactly as it is in the draw dispatch.
            let verdict = match k {
                "toggle" => hit_toggle(input.mouse, r, &props, click),
                "radio" => hit_radio(input.mouse, r, &props, click),
                "pill_toggle" => hit_pill_toggle(input.mouse, r, &props, click),
                "tabs" => hit_tabs(input.mouse, r, &props, click),
                "select" => hit_select(input.mouse, r, &props, click),
                // The one arm that also needs the RAW HELD state: a captured slider
                // keeps mapping the pointer into its bind between click edges, which
                // is the `down` half of the click/held pair.
                "slider" => hit_slider(input.mouse, r, &props, click, input.down),
                "stepper" => hit_stepper(input.mouse, r, &props, click),
                "text_field" => hit_text_field(input.mouse, r, click),
                // The one arm that never reads the click edge at all: a `list` folds
                // this frame's WHEEL tick, which rides the props (patched live) rather
                // than the button state.
                "list" => hit_list(input.mouse, r, &props),
                "context_menu" => hit_context_menu(input.mouse, r, &props, click),
                // The other arm that ignores the click edge: a `badge` only CLAIMS its
                // pill — it has no bind to write and no action to fire.
                "badge" => hit_badge(input.mouse, r, &props),
                // The PTT: claims its whole frame, and a click in a hint gutter FIRES the
                // neighbouring rail's step name (read live off the rail child) — so the
                // rail steps itself, exactly as the old hint button did.
                "paged_menu" => hit_paged_menu(input.mouse, r, node, model, click),
                _ => hit_checkbox(input.mouse, r, &props, click),
            };
            // ONE seam: a verdict touches state/results in exactly one place, so every
            // control's interaction flows through identical plumbing.
            apply_hit_verdict(verdict, p, state, results, hud_hit);
        }
        // A `badge` is above for the FIRST reason and nothing else: its claim region is
        // the PILL a style may inset inside the node rect, and the verdict stops there —
        // a chip claims so the scene cannot pick through it, and never binds or fires.
        // `button`, `panel`, `tile` and `action_slot` declare a trivial geometry in
        // [`rust_hit_shape`] instead, and this arm answers them generically (hover
        // claims; a click fires the action / toggles the bind). Besides the bespoke arm
        // above, only the generic plumbing remains — the drag-source, this arm, and the
        // styled-container claim below.
        // Presentational (sprite / tooltip / rune_corners and the read-out gauges):
        // never claims, never interacts. Said out loud rather than left to the
        // catch-all, because the [`HitShape::None`] declaration is what the roster
        // gate accepts as "this control HAS answered its hit".
        k if rust_hit_shape(k) == Some(HitShape::None) => {}
        // Full-rect control (button / panel / tile / action_slot): hover claims; a
        // click inside fires the node's `action` and/or toggles its bool `bind`.
        k if rust_hit_shape(k) == Some(HitShape::Rect) => {
            if r.contains(input.mouse) {
                *hud_hit = true;
                if input.clicked && p.enabled {
                    if let Some(action) = &node.action {
                        results.set(action.clone(), true);
                        // Press feedback is a BUTTON behaviour: firing the action
                        // lights its flash — the same flash a declared signal firing
                        // this result name lights, so every activation path
                        // acknowledges alike.
                        state.flash(action);
                        // …and feeds the strip-step channel, exactly as the signal
                        // path does (`walker.rs` flash+push_step together). Without
                        // this a MOUSE click on a rail hint flashed but never stepped
                        // the strip — a click is an activation like any other.
                        state.push_step(action);
                        // …and rides the ONE activation channel to the `sig_<name>`
                        // mirror: the walker's `take_fired` drains this so a click on
                        // `mode_<realm>` mirrors `sig_mode_<realm>` for menu.lua just
                        // like a pad Confirm would (rule 37722F91 / pump P2).
                        state.record_pointer_fire(action);
                    }
                    if let Some(bind) = &node.bind {
                        let val = !eff_bool(results, model, bind);
                        results.set(bind.clone(), val);
                    }
                }
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
/// through identical plumbing: `hit`→`hud_hit`,
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
    let ident = node_ident(node);
    if verdict.hit {
        *hud_hit = true;
    }
    // The component's own complaint about this node's authored data. Surfacing it
    // here is what keeps an unusable authored value LOUD instead of a control that
    // quietly does nothing.
    if let Some(msg) = &verdict.warn {
        tracing::warn!("ui: node {:?} ({}): {msg}", node.id, node.component);
    }
    if let (Some(val), Some(bind)) = (verdict.value, node.bind.as_deref()) {
        // Commit-on-release: a CAPTURED write (a slider drag, including the press
        // frame that takes the capture) is a value-in-flight, not an emission.
        // The live value still lands in `results` so this frame's draw follows
        // the hand; the frame tail puts the resting model value back before the
        // scene reads `results`, and the release edge at the top of
        // [`run_ui`] performs the one real write. Uncaptured value writes
        // (a stepper click, a tab pick, a list wheel tick) stay immediate —
        // each is already a discrete, committed gesture.
        if state.dragging.contains(ident) || verdict.capture == Some(true) {
            state.drag_value = Some((bind.to_string(), val.clone(), slider_range(node).0 as f64));
        }
        results.set(bind.to_string(), val);
    }
    if verdict.activate {
        if let Some(action) = &node.action {
            results.set(action.clone(), true);
            // Every activation path lights the action's flash (see the Rect arm).
            state.flash(action);
            // …and rides the one activation channel to the mirror (rule 37722F91).
            state.record_pointer_fire(action);
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
            state.flash(action);
            // …and rides the one activation channel to the mirror (rule 37722F91).
            state.record_pointer_fire(action);
        }
    }
    // An arbitrary named fire (a `paged_menu` hint gutter → the rail's step name): the
    // SAME full channel the generic Rect arm runs for a button's `action`, `push_step`
    // included, so the neighbouring rail advances this frame. Without the strip-step a
    // click would flash but never step (the bug rule 37722F91 / the Rect arm both fixed).
    if let Some(name) = &verdict.fire {
        results.set(name.clone(), true);
        state.flash(name);
        state.push_step(name);
        state.record_pointer_fire(name);
    }
    if verdict.group_focus {
        if let (Some(fg), Some(bind)) = (focus_group(node), node.bind.as_deref()) {
            results.set(fg.to_string(), bind.to_string());
        }
    }
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
/// Keyboard is NOT pointer: this runs unconditionally every frame — a typing frame
/// with a parked pointer is not input-active, so the hit pass skips every node, yet
/// it must still fold; the changed value then re-fingerprints the focused field, so it
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

/// Claim **local display ownership** for every bind a human just moved — the write
/// half of [`UiState::local`].
///
/// A key sitting in `results` at this point can only have come from an interaction
/// (a hit verdict, the release commit, a typed fold); the echo has not run yet, so
/// there are no defaults to confuse with edits. A value equal to the model's is an
/// echo of what the scene already believes and claims nothing.
///
/// A key we are already holding replays its own seed every frame — re-recording that
/// would keep refreshing `seen` to track the model and quietly destroy the
/// external-change exit. So only a value that differs from what we hold — a genuinely
/// NEW edit — re-stamps the entry.
fn record_local(placed: &[Placed], model: &ValueMap, results: &ValueMap, state: &mut UiState) {
    for p in placed {
        let Some(bind) = p.node.bind.as_deref() else { continue };
        let Some(val) = results.get(bind) else { continue };
        let seen = model.get(bind);
        if seen == Some(val) {
            continue;
        }
        if matches!(state.local.get(bind), Some((held, _)) if held == val) {
            continue;
        }
        state.local.insert(bind.to_string(), (val.clone(), seen.cloned()));
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
/// * `tabs` defaults to its first child's numeric `value` (a strip always has one
///   active); the index pickers (`pill_toggle`/`select`) echo the model's NUMBER,
/// * the name picker (`radio`) echoes only the text the model holds,
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
                // Which segment is selected is an INDEX — a number — so the echo
                // reports a number, defaulting to the first child's numeric `value`
                // (a strip always has one active tab).
                let first = node.children.first().and_then(|c| match c.props.get("value") {
                    Some(Value::Number(n)) => Some(*n),
                    _ => None,
                });
                if let Some(n) = model.number(bind).or(first) {
                    results.set(bind.to_string(), n);
                }
            }
            "pill_toggle" | "select" => {
                if let Some(n) = model.number(bind) {
                    results.set(bind.to_string(), n);
                }
            }
            "radio" => {
                // The one NAME-keyed picker: a row's literal id, echoed as text.
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
/// would emit byte-identical commands, so the cached ones are replayed and the draw
/// arm never runs.
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

    // A splash's alpha ramp is driven by the scene clock (`Model.elapsed`) — a
    // model read with no `*_bind` prop naming it, so it is folded by KIND, the
    // same way `list` folds its content height. The CURRENT alpha rather than the
    // raw clock, so the hold plateau (and the fully-faded tails) still replay
    // while the ramps redraw every frame they actually change.
    if node.component == "splash" {
        h.f32(splash_alpha_of(node, model));
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
    for key in ["style_off", "tab_active", "tab_idle", "glyph_style", "color", "tint", "rune_color"] {
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
    let hot = hot_matters && (r.contains(input.mouse) || (state.nav_mode() && focused));
    h.bool(hot);
    // Pressed (hot + primary held) as its own bit: the press nudge/stops must
    // redraw on BOTH edges of the click, and the release frame has the same
    // pointer position as the held frame — `hot` alone would replay it.
    h.bool(hot && input.down);
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

    // A live press flash is a draw input: the glow fades a step per frame, so the
    // node must re-fingerprint (and redraw) each tick or the first frame's glow
    // would freeze on screen. Folded only for nodes that fire an ACTION (the
    // flash is the button's activate acknowledgement), and as `0.0` when unlit —
    // so the resting fingerprint is stable.
    if let Some(action) = node.action.as_deref() {
        h.f32(state.flash_intensity(action));
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

/// Assemble the plain-data props a component's draw receives for one node: its
/// resolved style block, its label, and its hover/focus state. The walker owns style
/// resolution and retained interaction state; the component owns how it DRAWS them —
/// the one place the walker↔component prop contract lives.
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
        || (state.nav_mode() && !node.id.is_empty() && state.focused() == Some(node.id.as_str()));
    // Start from the node's own scalar props (box / label_x / label_size / value / … —
    // each component reads whichever it needs), then overlay the walker-resolved fields.
    // Display-text props resolve through the stringtable here, so a component's draw
    // only ever sees FINAL text; value/bind channels never resolve (user text is data).
    let mut props = serde_json::Map::new();
    for (k, v) in &node.props {
        props.insert(k.clone(), display_prop_json(k, v));
    }
    // Generic bound-prop channel: any authored `<name>_bind` delivers the effective
    // Model/results value as `<name>` (a bound value overrides a literal `<name>`
    // prop, matching text_bind→label). The cache needs no new wiring — every
    // `*_bind` key is already in the fingerprint's read set. Binds the walker
    // consumes itself stay out so their stems keep walker semantics.
    const WALKER_BINDS: [&str; 10] = [
        "text_bind", "label_bind", "visible_bind", "enabled_bind", "style_bind",
        "color_bind", "live_bind", "rune_bind", "name_bind", "meta_bind",
    ];
    // Stems the walker itself inserts below — an authored `<stem>_bind` here
    // would be silently overwritten, so it is a warned authoring error, not a
    // quiet no-op (the fail-loud law for authored names).
    const WALKER_STEMS: [&str; 11] = [
        "hot", "pressed", "enabled", "focused", "open", "captured", "wheel", "label", "style",
        "layer", "content_h",
    ];
    for (k, v) in &node.props {
        let Some(stem) = k.strip_suffix("_bind") else { continue };
        if WALKER_BINDS.contains(&k.as_str()) {
            continue;
        }
        if WALKER_STEMS.contains(&stem) {
            tracing::warn!(
                "ui: `{k}` names a walker-owned prop — the bound value would be \
                 overwritten; rename the prop"
            );
            continue;
        }
        if let Value::Text(key) = v {
            if let Some(val) = eff_value(results, model, key) {
                props.insert(stem.to_string(), value_to_json(val));
            }
        }
    }
    props.insert("label".to_string(), Json::String(node_text(node, model, results)));
    props.insert("hot".to_string(), Json::Bool(hovered));
    // Pressed = hot + primary held; the fingerprint folds the same bit, so a
    // component drawing its press state (nudge + press_* stops) invalidates on
    // both click edges.
    props.insert("pressed".to_string(), Json::Bool(hovered && input.down));
    props.insert("enabled".to_string(), Json::Bool(enabled(node, model)));
    // A component emits at layer 0; the walker offsets the whole node's commands by its
    // accumulated sub-layer afterwards (see run_ui's draw loop), so 0 always overrides
    // the node's own `layer` sub-layer prop here.
    props.insert("layer".to_string(), serde_json::json!(0.0));
    props.insert("style".to_string(), st.clone());
    // Resolve the named alternate-style paths a control may carry (a tile's loaded-vs-
    // empty `style_off`; a tab strip's `tab_active`/`tab_idle`; a glyph-faced button's
    // `glyph_style` — the atlas description) into their blocks, like `style`, so the
    // component reads a resolved block rather than a path.
    for key in ["style_off", "tab_active", "tab_idle", "glyph_style"] {
        if let Some(path) = ptext(node, key) {
            props.insert(key.to_string(), jpath(styles, path).clone());
        }
    }
    // The composites carry chrome styles WITH BUILDER DEFAULTS, so a bare
    // `{component: "popup_panel", title, children}` still draws its slab and a bare
    // `paged_menu` its rule + glyph hints. Resolved here (with the default path when the
    // node authors none) rather than in the loop above, which only resolves what is
    // present and so could not carry a default.
    match node.component.as_str() {
        "popup_panel" => {
            for (key, dflt) in [("panel_style", "modal.panel"), ("divider_style", "modal.divider")] {
                props.insert(key.to_string(), jpath(styles, ptext(node, key).unwrap_or(dflt)).clone());
            }
            // The title/subtitle/footer colours are dotted paths (a colour cannot ride as
            // a scalar prop) — resolved to rgba here, like a `text` node's `color`, since
            // the draw fn has no `styles` handle.
            for (key, dflt) in [
                ("title_color", "modal.title.color"),
                ("subtitle_color", "modal.subtitle.color"),
                ("footer_color", "modal.footer.color"),
            ] {
                let c = json_color(jpath(styles, ptext(node, key).unwrap_or(dflt)), INK);
                props.insert(format!("{key}_rgba"), serde_json::json!([c[0], c[1], c[2], c[3]]));
            }
            // A live subtitle: `subtitle_bind` names the Model key whose CURRENT text
            // the chrome draws (the display-confirm countdown). Resolved here because
            // the draw fn has no model handle — the same reason as the colours above.
            if let Some(bind) = ptext(node, "subtitle_bind") {
                if let Some(t) = model.text(bind) {
                    props.insert("subtitle_live".to_string(), serde_json::json!(t));
                }
            }
        }
        "paged_menu" => {
            props.insert("rule_style".to_string(), jpath(styles, ptext(node, "rule_style").unwrap_or("paged_menu.rule")).clone());
            props.insert("glyph_style".to_string(), jpath(styles, ptext(node, "glyph_style").unwrap_or("pad_glyphs")).clone());
        }
        "splash" => {
            // `backdrop` is a dotted style path (a colour cannot ride as a scalar
            // prop); default = the scene styles' `logo.backdrop`, falling back to
            // opaque black in `draw_splash` when unauthored.
            let c = json_color(jpath(styles, ptext(node, "backdrop").unwrap_or("logo.backdrop")), [0.0, 0.0, 0.0, 1.0]);
            props.insert("backdrop_rgba".to_string(), serde_json::json!([c[0], c[1], c[2], c[3]]));
        }
        _ => {}
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
    // "entered": the focused pane is LOCKED (0EFF5464) — the panel draws its distinct
    // entered rim (the mode's required render affordance), the scene feeds its camera.
    props.insert(
        "entered".to_string(),
        Json::Bool(
            state.entered() && !node.id.is_empty() && state.focused() == Some(node.id.as_str()),
        ),
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
    // Press feedback: an action-firing control reads its action's live flash
    // intensity back as `flash` (0..1) — the button's activate acknowledgement,
    // lit by a click, a Confirm, or a declared signal firing the same result
    // name, even when the action wrapped to nowhere. Injected only while lit,
    // so a resting button's props are byte-identical to before this mechanism
    // existed.
    if let Some(action) = node.action.as_deref() {
        let intensity = state.flash_intensity(action);
        if intensity > 0.0 {
            props.insert("flash".to_string(), Json::from(f64::from(intensity)));
        }
    }
    Json::Object(props)
}

/// Draw one laid-out node. Returns the component props it marshalled, which the
/// walker caches so the HIT arms can reuse them instead of rebuilding the map
/// (see [`component_hit_props`]).
fn draw_node(
    p: &Placed,
    model: &ValueMap,
    results: &ValueMap,
    styles: &Json,
    input: &UiInput,
    state: &UiState,
    out: &mut Vec<HudCommand>,
) -> Option<Json> {
    let node = p.node;
    let r = p.rect;
    // `style_bind` (a Model key holding a dotted style path) wins over a literal `style`, so a
    // node's fill/border can follow its state — the non-interactive pipeline tabs pick active vs
    // idle this way, one node per tab instead of a stack of visibility-toggled panels.
    let st = resolve_style(node, styles, model, results);
    match node.component.as_str() {
        // ── Rust components ──────────────────────────────────────────────
        // Every control draws HERE, in the engine. There is no other tier.
        k if crate::is_rust_component(k) => {
            let props = component_props(node, st, styles, model, results, input, state, r);
            // One arm, one dispatch table — the props assembly above is identical for
            // every component, so listing kinds separately would only repeat it.
            match k {
                "panel" => draw_panel(r, &props, out),
                "sprite" => draw_sprite(r, &props, out),
                "splash" => draw_splash(r, node, model, &props, out),
                "rune_corners" => draw_rune_corners(r, &props, out),
                "tooltip" => draw_tooltip(r, &props, out),
                "checkbox" => draw_checkbox(r, &props, out),
                "toggle" => draw_toggle(r, &props, out),
                "radio" => draw_radio(r, &props, out),
                "tile" => draw_tile(r, &props, out),
                "pill_toggle" => draw_pill_toggle(r, &props, out),
                "tabs" => draw_tabs(r, &props, out),
                "select" => draw_select(r, &props, out),
                "slider" => draw_slider(r, &props, out),
                "stepper" => draw_stepper(r, &props, out),
                "text_field" => draw_text_field(r, &props, out),
                "list" => draw_list(r, &props, out),
                "context_menu" => draw_context_menu(r, &props, out),
                "gauge" => draw_gauge(r, &props, out),
                "resource_gauge" => draw_resource_gauge(r, &props, out),
                "stat_dot" => draw_stat_dot(r, &props, out),
                "action_slot" => draw_action_slot(r, &props, out),
                "medallion" => draw_medallion(r, &props, out),
                "badge" => draw_badge(r, &props, out),
                "popup_panel" => draw_popup_panel(r, node, &props, out),
                "paged_menu" => draw_paged_menu(r, node, model, &props, out),
                _ => draw_button(r, &props, out),
            }
            return Some(props);
        }
        // Styled boxes — including `cell` (the generic layout box) and an `rtt`, whose
        // panel IS its PiP backdrop; the scene's frame graph blits the render target over
        // this (see `RttSlot`). All draw a bg ONLY when they carry a style — an unstyled
        // box (a plain unstyled `cell`) is transparent structure.
        "cell" | "row" | "stack" | "screen" | "rtt" | "grid" => {
            if !st.is_null() {
                draw_panel_bg(r, st, out);
            }
        }
        // (`list` — the scrolling region's backdrop + scrollbar — draws in the
        // engine arm above, like every other component; only its column LAYOUT +
        // viewport clip stay here in `resolve`. `list_draw_is_byte_pinned` holds
        // the bytes.)
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
        // Anything else is not a component kind at all (the roster gate in this
        // module's tests holds every interactive kind to an arm above), so there is
        // nothing to draw — only the structural primitives live down here.
        _ => {}
    }
    None
}

/// Scale the alpha of every colour in `cmds` by `f` — the assembly half of the
/// `faded` container toggle (see [`node_fade`] and the draw loop in [`run_ui`]).
/// Runs on the way out of the [`DrawCache`], never into it. A clip carries no
/// colour and passes through.
fn fade_commands(cmds: &mut [HudCommand], f: f32) {
    for c in cmds {
        match c {
            HudCommand::Rect { color, .. }
            | HudCommand::Sprite { color, .. }
            | HudCommand::Text { color, .. }
            | HudCommand::TextCaret { color, .. } => color[3] *= f,
            HudCommand::Panel { color, color2, border_color, .. } => {
                color[3] *= f;
                color2[3] *= f;
                border_color[3] *= f;
            }
            HudCommand::Clip { .. } => {}
        }
    }
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

// ── Components (the engine tier) ─────────────────────────────────────────────
//
// A COMPONENT owns its whole draw definition and lives HERE, in the engine.
// Aaron's ratified taxonomy says so in as many words (9C141E1C: *"the walker's
// per-control draw + hit + bind code IS that Component's logic"*), and the
// 2026-08-09 ruling (BF0AF0C9) restored it after the 2026-07-30 inversion moved
// these into `ui/<kind>.lua`. Lua ORCHESTRATES — position, callback hooks,
// config metadata — it does not own semantics.
//
// A component draws into the `rect` the layout engine gives it and owns no
// layout. Draw and hit read ONE plain-data `props` map (built by
// `component_props`), so there is exactly one prop surface, not two.

/// The trivial hit geometry a component declares, read by the one generic claim in
/// [`hit_node`].
///
/// * [`HitShape::Rect`] — a full-rect control: its whole box is the interactive
///   region and its only interaction is hover-claim + click-fires-`action`.
/// * [`HitShape::None`] — presentational: never claims, never interacts.
/// * `None` — not a Rust component, or one owning bespoke geometry: those answer
///   through [`rust_owns_hit`] and their own arm in [`hit_node`] instead, and MUST NOT
///   appear here (see that fn — the two answers are mutually exclusive).
///
/// An engine control MUST answer here or in [`rust_owns_hit`], or the walker stops
/// treating it as interactive at all — the hover, the click and the focus ring go
/// quiet at once, which is precisely what the button slice caught. The roster gate
/// in this module's tests holds every kind to it.
fn rust_hit_shape(kind: &str) -> Option<HitShape> {
    match kind {
        // A panel's whole box is its surface: the claim alone is the point, so a click
        // on a pane's background does not pick through to the scene behind it. A tile
        // is the other full-rect control: the whole slot cell is the target and a click
        // inside toggles its bool bind. An action slot is the same shape of thing as a
        // button — its rim, keybind tag and charge count are read-out, so a click
        // anywhere on the recess casts.
        // A `popup_panel` is a full-rect claim like a `panel`: its slab must not pick
        // through to the scene behind the modal, and its items are their own child nodes
        // that answer their own clicks. It writes no bind and fires no action of its own.
        "button" | "panel" | "tile" | "action_slot" | "popup_panel" | "splash" => Some(HitShape::Rect),
        // Pure decoration — never claims, never interacts. A tooltip that claimed would
        // eat every click beneath the cursor it follows; a sprite is an image, not a
        // surface, so clicks pass through to the scene behind it. The gauges, the stat
        // dot and the portrait medallion are the READ-OUT half of that rule: they report
        // state and are never targets, so a bar, a legend or a party portrait laid over
        // the world does not eat the click that steers it.
        "sprite" | "rune_corners" | "tooltip" | "gauge" | "resource_gauge" | "stat_dot"
        | "medallion" => Some(HitShape::None),
        _ => None,
    }
}

/// The engine-tier components whose hit is a REAL verdict rather than a trivial shape —
/// because their tight region is a sub-rect the component computes from its own props (a
/// checkbox's box, a toggle's pill, a radio's circle, a pill toggle's well, a badge's
/// chip — which a style may inset inside the node rect, and which CLAIMS and nothing
/// else), or because a click does more than claim-and-fire (an option strip picks the
/// cell under the pointer;
/// a select opens, picks and closes; a slider claims the whole row but only its grab band
/// starts a drag, and then maps the pointer into the bind for as long as it is held; a
/// text field takes KEYBOARD FOCUS where the generic full-rect arm would have fired an
/// action and toggled its string bind to a bool; a `list` claims its whole rect but folds
/// a WHEEL tick into its bound offset, which no click-driven shape can express). Each
/// owns an arm in [`hit_node`] — the Rust twin of a module's `M.hit` — returning a full
/// [`HitVerdict`].
///
/// Such a kind must NEVER appear in [`rust_hit_shape`]: [`HitShape::Rect`] there would
/// silently widen the tight region to the whole node rect, so a click on the caption
/// row beside a 14px box would toggle the bind. The two answers are mutually
/// exclusive, and the roster gate accepts a migrated control that gives EITHER.
fn rust_owns_hit(kind: &str) -> bool {
    matches!(
        kind,
        "checkbox"
            | "toggle"
            | "radio"
            | "pill_toggle"
            | "tabs"
            | "select"
            | "slider"
            | "stepper"
            | "text_field"
            | "list"
            | "context_menu"
            | "badge"
            // A `paged_menu` owns its hit for the fifth reason and one of its own: the
            // four hint GUTTERS are sub-rects it lays out itself, and a click in one fires
            // the neighbouring rail's step name (`prev_action`/`next_action`) rather than
            // an action or bind of the menu node — geometry and a dispatch no trivial
            // shape can carry.
            | "paged_menu"
    )
}

/// The **panel** — the backdrop a pane IS, and the focus rim it wears.
///
/// A panel is the one component every multi-panel view is built from, and it owns
/// exactly two things: its BACKDROP (from its own style block, so a pane's fill,
/// radius and resting edge are a style token — never a caller's string) and its
/// FOCUS RIM (from the walker-injected `focused` prop).
///
/// **The walker decides WHICH panel holds the cursor** (the left stick cycles
/// `tab_group`); the panel decides what that LOOKS like. A scene never passes a rim
/// style and never learns a rim exists — that split is what violation F2 was about
/// (`2FE653F9`: *"THE SCENE SHOULD KNOW NOTHING ABOUT HOW A FUCKING COMPONENT
/// WORKS"*), so keep it here.
///
/// It writes NO bind and NO action: a panel is a focus target and a backdrop.
/// Anything interactive inside it is its own component, in its own node.
fn draw_panel(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let focused = props.get("focused").and_then(|v| v.as_bool()).unwrap_or(false);
    let entered = props.get("entered").and_then(|v| v.as_bool()).unwrap_or(false);
    // `{ resting, focused, entered }` sub-blocks, each an ordinary container block. The
    // ENTERED block (the pane is LOCKED — 0EFF5464) draws a rim distinct from FOCUSED
    // (navigating-to), so the pad player can SEE the mode. A style that carries the
    // container keys DIRECTLY (no split) is used as-is; each state falls down the chain
    // (entered → focused → resting → the block), so a plain panel style still draws.
    let block = if entered {
        s.get("entered").or_else(|| s.get("focused")).or_else(|| s.get("resting")).unwrap_or(s)
    } else if focused {
        s.get("focused").or_else(|| s.get("resting")).unwrap_or(s)
    } else {
        s.get("resting").unwrap_or(s)
    };
    draw_panel_bg(r, block, out);
}

/// The **sprite** — an image node: blit the engine texture named by `tex` into the
/// node's whole rect, tinted white × `alpha`. The menu's Muse plate is one of these.
///
/// It owns the BLIT and nothing else: the aspect lock, the anchor and the deliberate
/// spill past the viewport (cover, never letterbox) are the walker's layout, and the
/// node's `layer` prop is the walker's sub-layer — which is how one component serves a
/// full-bleed backdrop and a 32px icon alike.
///
/// No `tex` draws NOTHING rather than texture 0 — the same rule the glyph face
/// follows: a missing image must be visibly missing, never silently the wrong picture.
fn draw_sprite(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let Some(tex) = props.get("tex").and_then(|v| v.as_f64()) else {
        return;
    };
    out.push(HudCommand::Sprite {
        tex: tex.floor() as u32,
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: [1.0, 1.0, 1.0, jnum(props, "alpha", 1.0)],
        layer: 0.0,
        // The WHOLE image: an atlas sub-rect is the glyph face's business, not this one's.
        uv: [0.0, 0.0, 1.0, 1.0],
    });
}

/// **Splash** — ONE full-bleed image that fades in, holds, then fades out: the
/// intro-logo component. The DRAWING logic lives here (the fifth-plus component,
/// same as the other four dozen); the scene's pair script CONFIGURES it from its
/// `arrange()` — the entry's `image` / `fade_in` / `hold` / `fade_out` prop
/// overrides, which the engine applies onto this node's props.
///
/// Props: `tex` (texture index), `fade_in` / `hold` / `fade_out` (seconds),
/// `fit` (0..1 of the rect the image may fill), `backdrop_rgba` (resolved by
/// `component_props`). Model: `elapsed` (seconds), `img_w` / `img_h` (the
/// image's native pixels, published by the scene at load).
///
/// The alpha ramp is the exact timeline the two splash scripts used to carry in
/// duplicate: linear rise over `fade_in`, flat 1.0 through `hold`, linear fall
/// over `fade_out`.
pub(crate) fn splash_alpha(elapsed: f32, fade_in: f32, hold: f32, fade_out: f32) -> f32 {
    let alpha = if elapsed < fade_in {
        if fade_in <= 0.0 { 1.0 } else { elapsed / fade_in }
    } else if elapsed > fade_in + hold {
        if fade_out <= 0.0 { 0.0 } else { 1.0 - (elapsed - fade_in - hold) / fade_out }
    } else {
        1.0
    };
    alpha.clamp(0.0, 1.0)
}

/// The splash node's CURRENT ramp alpha: its timeline props (with `fade_out`
/// falling back to `fade_in`, exactly as the draw defaults) against the scene
/// clock (`Model.elapsed`). THE one reader for both the draw and the cache
/// fingerprint, so the default chain can never fork between them.
fn splash_alpha_of(node: &UiNode, model: &ValueMap) -> f32 {
    let fade_in = pnum(node, "fade_in").unwrap_or(0.6) as f32;
    let hold = pnum(node, "hold").unwrap_or(1.2) as f32;
    let fade_out = pnum(node, "fade_out").unwrap_or(f64::from(fade_in)) as f32;
    let elapsed = model.number("elapsed").unwrap_or(0.0) as f32;
    splash_alpha(elapsed, fade_in, hold, fade_out)
}

fn draw_splash(r: Rect, node: &UiNode, model: &ValueMap, props: &Json, out: &mut Vec<HudCommand>) {
    let bg = first_color(props, &["backdrop_rgba"], [0.0, 0.0, 0.0, 1.0]);
    out.push(HudCommand::Rect { x: r.x, y: r.y, w: r.w, h: r.h, color: bg, layer: 0.0 });

    let Some(tex) = pnum(node, "tex") else { return };
    let fit = pnum(node, "fit").unwrap_or(0.9) as f32;
    let alpha = splash_alpha_of(node, model);

    // Contain-fit the image's native size inside `fit` of the rect, centred.
    let iw = model.number("img_w").unwrap_or(1.0).max(1.0) as f32;
    let ih = model.number("img_h").unwrap_or(1.0).max(1.0) as f32;
    let scale = (r.w * fit / iw).min(r.h * fit / ih);
    let (w, h) = (iw * scale, ih * scale);
    out.push(HudCommand::Sprite {
        tex: tex.floor() as u32,
        x: r.x + (r.w - w) * 0.5,
        y: r.y + (r.h - h) * 0.5,
        w,
        h,
        color: [1.0, 1.0, 1.0, alpha],
        layer: 0.0,
        uv: [0.0, 0.0, 1.0, 1.0],
    });
}

/// **Rune corners** — four Elder-Futhark glyphs inset from the rect's corners, the TOP
/// pair in rune-light and the BOTTOM pair in dim bronze. The carved inlay a Prism frame
/// wears: pure decoration, no bind, no action, no claim.
///
/// Each corner is its OWN prop with its own default, so a caller overrides exactly one
/// — the `frame` template blanks `tr` with an EMPTY STRING on a closable frame so the
/// glyph does not paint beneath the ✕ — without restating the set. An empty string is
/// therefore a meaningful value, not a missing one, and must not fall back to the
/// default (Lua's `or` agrees: `""` is truthy there).
///
/// The bottom pair is inset UP by a further glyph height so its box mirrors the top
/// pair's — text is placed by its TOP edge, so subtracting `size` is what stops the
/// bottom two hanging off the edge.
fn draw_rune_corners(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    // Defaults ARE the house look (five-line item 3): the retired `settings.runes`
    // block's values — inset 14, size 16, rune-glow top, bronze-dim bottom — are the
    // compiled fallback now, so a bare `rune_corners` draws the carved-corner chrome
    // and a scene that wants a different look overrides with a `style` block (the
    // Component Catalog does). Mirrors of theme tokens; the drift gate
    // `rune_corners_default_matches_theme_tokens` fails loud if they diverge.
    let inset = jnum(s, "inset", 14.0);
    // The node may pin a glyph size; otherwise the style's, else the house default.
    let size = jnum(props, "glyph_size", jnum(s, "size", 16.0));
    let glow = first_color(s, &["top"], RUNE);
    let bronze = first_color(s, &["bot"], BRONZE_DIM);
    let by = r.y + r.h - inset - size;
    // The right pair anchors at `w - inset` and draws right-aligned, so both edges hold
    // the same visual margin without measuring a glyph.
    for (key, dflt, x, y, color, align) in [
        ("tl", "ᛞ", r.x + inset, r.y + inset, glow, TextAlign::Left),
        ("tr", "ᛝ", r.x + r.w - inset, r.y + inset, glow, TextAlign::Right),
        ("bl", "ᚨ", r.x + inset, by, bronze, TextAlign::Left),
        ("br", "ᛟ", r.x + r.w - inset, by, bronze, TextAlign::Right),
    ] {
        let glyph = props.get(key).and_then(|v| v.as_str()).unwrap_or(dflt);
        push_text(out, x, y, glyph, size, color, align, FontRole::Rune, false, false, -1.0, None);
    }
}

/// The **tooltip** — a small info card: an optional styled backdrop, then an optional
/// element RUNE at the top-left with the name headline beside it and a dim meta line
/// below, in the text column right of the rune.
///
/// The SCENE positions it (the node's rect) and gates it (`visible_bind`); the card
/// owns only what it looks like. Its three fields arrive ALREADY ABSENT when empty, so
/// each row draws only when it has something to say — a runeless tip is a name and a
/// meta line, never three rows with a hole in them.
///
/// It never claims the pointer: a cursor-following tip that claimed would eat every
/// click underneath it.
fn draw_tooltip(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    // Backdrop through the one styled-box path, and ONLY when the node carries a style
    // — an unstyled tip is bare text floating over whatever it sits on.
    if !s.is_null() {
        draw_panel_bg(r, s, out);
    }
    let inner = r.inset(jnum(s, "pad", 12.0));
    let name_sz = jnum(s, "name_size", 16.0);
    let meta_sz = jnum(s, "meta_size", 12.0);
    let gap = jnum(s, "gap", 4.0);

    // Optional element rune, top-left — the text column then indents past it so the
    // glyph and the headline share one baseline.
    let mut indent = 0.0;
    if let Some(rune) = props.get("rune").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) {
        // `rune_color` arrives as a resolved rgba (the walker walks the dotted path),
        // because a colour cannot ride as a scalar prop.
        let color = first_color(props, &["rune_color"], RUNE);
        push_text(out, inner.x, inner.y, rune, name_sz, color, TextAlign::Left, FontRole::Rune, false, false, -1.0, None);
        indent = name_sz * 1.3 + 4.0;
    }
    if let Some(name) = props.get("name").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) {
        let color = first_color(s, &["name_color"], INK);
        push_text(out, inner.x + indent, inner.y, name, name_sz, color, TextAlign::Left, FontRole::Display, false, false, -1.0, None);
    }
    if let Some(meta) = props.get("meta").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) {
        let color = first_color(s, &["meta_color"], DIM);
        push_text(out, inner.x + indent, inner.y + name_sz + gap, meta, meta_sz, color, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
    }
}

/// `a` moved toward `b` by `t` (0..1), per channel — the brighten-on-activate ramp.
fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// One atlas cell's uv sub-rect. Row-major over a `cols` x `rows` grid — the SAME
/// arithmetic the sheet generator (`tools/gen_prism_pad_glyphs.py`) lays out with.
fn cell_uv(idx: f32, cols: f32, rows: f32) -> [f32; 4] {
    let (cx, cy) = (idx % cols, (idx / cols).floor());
    [cx / cols, cy / rows, (cx + 1.0) / cols, (cy + 1.0) / rows]
}

/// The glyph face: one cell of the controller-icon atlas, square and centred.
/// EVERYTHING about the atlas — texture, grid, name → cell map, colours — is the
/// resolved `glyph_style` block (`ui_theme.json` → `pad_glyphs`); the node
/// carries only WHICH glyph. An unknown name draws NOTHING rather than cell 0: a
/// hint silently showing the wrong button is worse than one visibly missing.
fn draw_glyph_face(r: Rect, props: &Json, flash: f32, out: &mut Vec<HudCommand>) {
    let g = props.get("glyph_style").unwrap_or(&Json::Null);
    let name = props.get("glyph").and_then(|v| v.as_str()).unwrap_or_default();
    let idx = g.get("cells").and_then(|c| c.get(name)).and_then(|v| v.as_f64());
    let (Some(tex), Some(idx)) = (g.get("tex").and_then(|v| v.as_f64()), idx) else {
        return;
    };
    let size = jnum(props, "glyph_size", r.w.min(r.h)).min(r.w).min(r.h);
    let box_ = Rect {
        x: r.x + (r.w - size) * 0.5,
        y: r.y + (r.h - size) * 0.5,
        w: size,
        h: size,
    };
    let mut color = first_color(g, &["color"], BRONZE);
    if flash > 0.0 {
        let lit = first_color(g, &["flash"], FLASH_LIT);
        color = lerp_color(color, lit, flash);
        // A soft glow swell behind the glyph, growing with the flash.
        let grow = size * 0.3 * flash;
        out.push(HudCommand::Panel {
            x: box_.x - grow,
            y: box_.y - grow,
            w: box_.w + grow * 2.0,
            h: box_.h + grow * 2.0,
            color: [lit[0], lit[1], lit[2], 0.28 * flash],
            color2: [lit[0], lit[1], lit[2], 0.28 * flash],
            grad: 0.0,
            radius: (size + grow * 2.0) * 0.28,
            border: 0.0,
            border_color: CLEAR,
            feather: grow,
            layer: 0.0,
        });
    }
    out.push(HudCommand::Sprite {
        tex: tex as u32,
        x: box_.x,
        y: box_.y,
        w: box_.w,
        h: box_.h,
        color,
        layer: 0.0,
        uv: cell_uv(idx.floor() as f32, jnum(g, "cols", 4.0), jnum(g, "rows", 4.0)),
    });
}

/// One interaction state's face for a compiled button [`BtnVariant`].
#[derive(Clone, Copy)]
struct BtnFace {
    top: [f32; 4],
    bot: [f32; 4],
    border: [f32; 4],
    label: [f32; 4],
}

/// The compiled default palette for a named button `variant` — the house looks
/// the `modal.buttons.variants.*` scene blocks used to carry, now DRAWING-CODE
/// defaults (five-line architecture item 3, rule 491BD9BB): a scene names
/// `variant: "primary"` and carries no style block, so the look is single-sourced
/// here instead of copied per scene. An explicit `style` block still overrides any
/// stop key-by-key. Every colour MIRRORS the theme token named in its comment; the
/// gate `button_variant_defaults_match_theme_tokens` reads ui_theme.json and fails
/// loud if a value drifts (so the mirror can never silently fork — rule AEEF2A68).
#[derive(Clone, Copy)]
struct BtnVariant {
    idle: BtnFace,
    hover: BtnFace,
    press: BtnFace,
    glow: [f32; 4],
}

const BTN_PRIMARY: BtnVariant = BtnVariant {
    idle: BtnFace {
        top: [0.141, 0.247, 0.471, 1.0],  // sap_base
        bot: [0.082, 0.153, 0.267, 1.0],  // sap_base_lo
        border: [0.227, 0.353, 0.627, 1.0], // sap_border
        label: [0.933, 0.949, 1.0, 1.0],  // ink_sapphire
    },
    hover: BtnFace {
        top: [0.173, 0.298, 0.557, 1.0],  // sap_hover
        bot: [0.102, 0.188, 0.341, 1.0],  // sap_hover_lo
        border: [0.286, 0.416, 0.722, 1.0], // sap_hover_border
        label: [0.933, 0.949, 1.0, 1.0],  // ink_sapphire
    },
    press: BtnFace {
        top: [0.09, 0.173, 0.329, 1.0],   // sap_press
        bot: [0.063, 0.122, 0.235, 1.0],  // sap_press_lo
        border: [0.165, 0.267, 0.471, 1.0], // sap_press_border
        label: [0.933, 0.949, 1.0, 1.0],  // ink_sapphire (press falls to label)
    },
    glow: [0.094, 0.188, 0.384, 0.45],    // sap_glow
};

const BTN_SECONDARY: BtnVariant = BtnVariant {
    idle: BtnFace {
        top: [0.125, 0.141, 0.18, 1.0],   // stone_btn
        bot: [0.078, 0.09, 0.122, 1.0],   // stone2
        border: [0.227, 0.255, 0.314, 1.0], // edge4
        label: [0.839, 0.816, 0.761, 1.0], // ink_button
    },
    hover: BtnFace {
        top: [0.157, 0.176, 0.22, 1.0],   // stone_btn_hi
        bot: [0.098, 0.114, 0.149, 1.0],  // surface_top
        border: [0.431, 0.353, 0.204, 1.0], // bronze_dim
        label: [0.906, 0.882, 0.824, 1.0], // ink_bright
    },
    press: BtnFace {
        top: [0.078, 0.09, 0.122, 1.0],   // stone2
        bot: [0.055, 0.063, 0.086, 1.0],  // stone1
        border: [0.169, 0.188, 0.235, 1.0], // edge2
        label: [0.839, 0.816, 0.761, 1.0], // ink_button (press falls to label)
    },
    glow: [0.0, 0.0, 0.0, 0.0],           // none
};

const BTN_DANGER: BtnVariant = BtnVariant {
    idle: BtnFace {
        top: [0.659, 0.216, 0.255, 1.0],  // danger_base
        bot: [0.439, 0.102, 0.133, 1.0],  // danger_base_lo
        border: [0.784, 0.439, 0.478, 1.0], // danger_hi
        label: [0.949, 0.827, 0.839, 1.0], // danger_text_hi
    },
    hover: BtnFace {
        top: [0.776, 0.255, 0.302, 1.0],  // danger_hover
        bot: [0.518, 0.122, 0.157, 1.0],  // danger_hover_lo
        border: [0.784, 0.439, 0.478, 1.0], // danger_hi
        label: [0.949, 0.827, 0.839, 1.0], // danger_text_hi
    },
    press: BtnFace {
        top: [0.439, 0.102, 0.133, 1.0],  // danger_base_lo
        bot: [0.29, 0.067, 0.086, 1.0],   // danger_press_lo
        border: [0.353, 0.165, 0.188, 1.0], // danger_border
        label: [0.949, 0.827, 0.839, 1.0], // danger_text_hi (press falls to label)
    },
    glow: [0.659, 0.216, 0.255, 0.4],     // danger_glow
};

const BTN_GHOST: BtnVariant = BtnVariant {
    idle: BtnFace {
        top: [0.0, 0.0, 0.0, 0.0],        // stage_void
        bot: [0.0, 0.0, 0.0, 0.0],        // stage_void
        border: [0.0, 0.0, 0.0, 0.0],     // none
        label: [0.561, 0.541, 0.49, 1.0], // dim
    },
    hover: BtnFace {
        top: [0.0, 0.0, 0.0, 0.0],        // stage_void
        bot: [0.0, 0.0, 0.0, 0.0],        // stage_void
        border: [0.169, 0.188, 0.235, 1.0], // edge2
        label: [0.871, 0.847, 0.788, 1.0], // ink
    },
    press: BtnFace {
        top: [0.0, 0.0, 0.0, 0.0],        // stage_void (press falls to idle)
        bot: [0.0, 0.0, 0.0, 0.0],        // stage_void
        border: [0.0, 0.0, 0.0, 0.0],     // none
        label: [0.561, 0.541, 0.49, 1.0], // dim
    },
    glow: [0.0, 0.0, 0.0, 0.0],           // none
};

/// A garish MAGENTA palette drawn for an UNKNOWN `variant` name — fail-loud, so a
/// typo'd variant is a visible defect on screen, never a silent neutral button
/// (rule 4BB12A75: an authored name that resolves to nothing is the difference
/// between authorable and not).
const BTN_UNKNOWN: BtnVariant = BtnVariant {
    idle: BtnFace {
        top: [1.0, 0.0, 1.0, 1.0],
        bot: [1.0, 0.0, 1.0, 1.0],
        border: [1.0, 1.0, 0.0, 1.0],
        label: [1.0, 1.0, 0.0, 1.0],
    },
    hover: BtnFace {
        top: [1.0, 0.0, 1.0, 1.0],
        bot: [1.0, 0.0, 1.0, 1.0],
        border: [1.0, 1.0, 0.0, 1.0],
        label: [1.0, 1.0, 0.0, 1.0],
    },
    press: BtnFace {
        top: [1.0, 0.0, 1.0, 1.0],
        bot: [1.0, 0.0, 1.0, 1.0],
        border: [1.0, 1.0, 0.0, 1.0],
        label: [1.0, 1.0, 0.0, 1.0],
    },
    glow: [0.0, 0.0, 0.0, 0.0],
};

/// Resolve a `variant` prop name to its compiled default palette. A NAME that is
/// present but unrecognised returns [`BTN_UNKNOWN`] (magenta) rather than falling
/// through to a neutral look — the authored-name-fails-loud contract (4BB12A75).
fn button_variant(name: &str) -> BtnVariant {
    match name {
        "primary" => BTN_PRIMARY,
        "secondary" => BTN_SECONDARY,
        "danger" => BTN_DANGER,
        "ghost" => BTN_GHOST,
        _ => BTN_UNKNOWN,
    }
}

/// The **button** — an SDF panel slab + a centred FACE, with hover + pressed
/// states (press = 1px nudge + `press_*` stops), an optional sapphire glow halo,
/// and per-variant fill/border/label (primary / secondary / danger / ghost). The
/// look comes from the `variant` prop's compiled default ([`BtnVariant`]); an
/// explicit `style` block overrides any stop, and a variant-less button keeps the
/// neutral sapphire fallback.
///
/// THE FACE is a label OR a glyph — a button is a button either way (Aaron,
/// 2026-08-08: a rail hint *"IS a BUTTON, it just happens to be one that sends a
/// prev/next signal … it has a different graphic"*). A node carrying `glyph`
/// draws one cell of the controller-icon atlas instead of text; everything else
/// about it — hit, action, hover, press, flash — is the same button.
///
/// PRESS FEEDBACK (`flash`) is the button's ACTIVATE acknowledgement: whenever
/// this button's `action` fires — a click, a pad Confirm on focus, or a screen-
/// declared signal firing the same result name — the walker injects a fading
/// intensity (0..1, ~250 ms) and the face brightens toward the style's `flash`
/// colour with a soft glow swell. The cue matters most exactly when the action
/// changed nothing on screen (one page, wrapped).
///
/// The HIT is answered generically in Rust — a button's whole box is its
/// interactive region and its only interaction is hover-claim + click-fires-
/// `action`, so it needs no per-kind hit arm at all.
fn draw_button(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let hot = props.get("hot").and_then(|v| v.as_bool()).unwrap_or(false);
    let pressed = props.get("pressed").and_then(|v| v.as_bool()).unwrap_or(false);
    // Activate feedback, injected by the walker only while a flash is live.
    let flash = jnum(props, "flash", 0.0);

    // DS press contract: the whole slab (glow included) nudges down 1px while
    // held; `press_*` stops pick the darker fill, falling to the idle fill for
    // variants that define none.
    let r = if pressed { Rect { y: r.y + 1.0, ..r } } else { r };

    // A `variant` names one of the compiled house palettes (primary / secondary /
    // danger / ghost); its stops become the per-state DEFAULTS below. A variant-less
    // button keeps the neutral sapphire fallback, and an explicit `style` block still
    // overrides any stop key-by-key.
    let v = props.get("variant").and_then(|x| x.as_str()).map(button_variant);

    // Optional sapphire glow halo behind the button, only on hover.
    let glow = first_color(s, &["glow"], v.map(|v| v.glow).unwrap_or(CLEAR));
    if glow[3] > 0.0 && hot {
        out.push(HudCommand::Panel {
            x: r.x - 3.0,
            y: r.y - 3.0,
            w: r.w + 6.0,
            h: r.h + 6.0,
            color: glow,
            color2: glow,
            grad: 0.0,
            radius: jnum(s, "radius", 3.0) + 3.0,
            border: 0.0,
            border_color: CLEAR,
            feather: 4.0,
            layer: 0.0,
        });
    }

    // Fill / border / label pick their hover vs idle stops down the alias chain —
    // the button OWNS this state→style logic.
    let (top, bot, mut border, mut label_color) = if pressed {
        let f = v.map(|v| v.press);
        let top = first_color(s, &["press_top", "fill_top", "cell", "fill"], f.map(|f| f.top).unwrap_or(SAP));
        (
            top,
            first_color(s, &["press_bot", "fill_bot", "cell", "fill"], f.map(|f| f.bot).unwrap_or(top)),
            first_color(s, &["press_border", "border"], f.map(|f| f.border).unwrap_or(CLEAR)),
            first_color(s, &["press_label", "label"], f.map(|f| f.label).unwrap_or(INK)),
        )
    } else if hot {
        let f = v.map(|v| v.hover);
        let top = first_color(s, &["hover_top", "hot", "fill_top", "cell", "fill"], f.map(|f| f.top).unwrap_or(SAP));
        (
            top,
            first_color(s, &["hover_bot", "hot", "fill_bot", "cell", "fill"], f.map(|f| f.bot).unwrap_or(top)),
            first_color(s, &["hover_border", "border"], f.map(|f| f.border).unwrap_or(CLEAR)),
            first_color(s, &["hover_label", "label"], f.map(|f| f.label).unwrap_or(INK)),
        )
    } else {
        let f = v.map(|v| v.idle);
        let top = first_color(s, &["fill_top", "cell", "fill"], f.map(|f| f.top).unwrap_or(SAP));
        (
            top,
            first_color(s, &["fill_bot", "cell", "fill"], f.map(|f| f.bot).unwrap_or(top)),
            first_color(s, &["border"], f.map(|f| f.border).unwrap_or(CLEAR)),
            first_color(s, &["label"], f.map(|f| f.label).unwrap_or(INK)),
        )
    };

    // Activate flash on the SLAB: the border brightens toward the lit colour, so a
    // label button acknowledges exactly like a glyph button (one behaviour, every
    // face). The glyph face adds its own glow swell in `draw_glyph_face`.
    if flash > 0.0 {
        let lit = first_color(s, &["flash"], FLASH_LIT);
        border = lerp_color(border, lit, flash);
        label_color = lerp_color(label_color, lit, flash);
    }

    out.push(HudCommand::Panel {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: top,
        color2: bot,
        grad: if top == bot { 0.0 } else { 1.0 },
        radius: jnum(s, "radius", 3.0),
        border: if border[3] > 0.0 { jnum(s, "border_w", 1.0) } else { 0.0 },
        border_color: border,
        feather: 0.0,
        layer: 0.0,
    });

    // The face: a glyph when the node names one, the centred label otherwise.
    if props.get("glyph").is_some_and(|v| !v.is_null()) {
        draw_glyph_face(r, props, flash, out);
        return;
    }
    let lsz = jnum(s, "label_size", jnum(props, "label_size", 14.0));
    let label = props.get("label").and_then(|v| v.as_str()).unwrap_or_default();
    push_text(
        out,
        r.x + r.w * 0.5,
        r.y + (r.h - lsz) * 0.5,
        label,
        lsz,
        label_color,
        TextAlign::Center,
        FontRole::Label,
        false,
        false,
        -1.0,
        None,
    );
}






/// A boxed picker's **box**: a `box`-sized square at the node rect's top-left, shared
/// by `checkbox` and `radio`.
///
/// THE geometry — the draw and the hit both read it, so the mark can never sit where
/// the click does not reach, and a caption laid beside it in the SAME node stays inert
/// rather than becoming a second, invisible target.
///
/// `box` (default 14) is a NODE prop, not a style key: a row's tick size is part of the
/// layout the row height was authored around, not part of its palette.
fn box_rect(r: Rect, props: &Json) -> Rect {
    let size = jnum(props, "box", 14.0);
    Rect { x: r.x, y: r.y, w: size, h: size }
}

/// The row caption a boxed picker (`checkbox` / `radio`) wears to the RIGHT of its
/// box, vertically centred on it. Empty copy draws nothing at all.
///
/// Decoration, never a target: the tight region is the box, so this text takes no
/// hover state and carries no geometry the hit has to know about. The CALLER passes
/// the colour, because that is the one thing the two pickers disagree on by default.
///
/// **props**: `label` · `label_x` (default `box` + `label_gap`, measured from the node
/// rect's left edge) · `label_gap` (8) · `label_size` (13). Node props rather than
/// style keys, so a dense list can tighten its gutter without a palette of its own.
fn draw_box_caption(r: Rect, bx: Rect, props: &Json, color: [f32; 4], out: &mut Vec<HudCommand>) {
    let Some(label) = props.get("label").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) else {
        return;
    };
    let size = jnum(props, "label_size", 13.0);
    let x = r.x + jnum(props, "label_x", bx.w + jnum(props, "label_gap", 8.0));
    push_text(out, x, bx.y + (bx.h - size) * 0.5, label, size, color, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
}

/// The **checkbox** — a square box, an inset tick while its bound bool is true, and an
/// inert caption beside it.
///
/// The BOX is the control: [`box_rect`] is the whole of its geometry and
/// [`hit_checkbox`] claims exactly that square, so a checkbox laid into a wide settings
/// row leaves the rest of that row's clicks alone. The caption draws here only because
/// it belongs to this node.
///
/// The tick is a filled inset square, not a glyph: one SDF panel, no font dependency,
/// and it still reads at the 14px default where a vector check would turn to mud.
///
/// **props**: `box` (14) · `label` · `label_x` · `label_gap` (8) · `label_size` (13) ·
/// `bind_value` — the bound bool, absent reading as unchecked.
/// **style**: `box` (box fill) · `radius` (0) · `border` + `border_w` (1, the edge
/// drawn only when `border` carries alpha) · `pad` (3, the tick's inset) · `check`
/// (tick fill) · `check_radius` (0) · `label` (caption colour).
fn draw_checkbox(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let bx = box_rect(r, props);
    // The box, with a hairline edge only when the style authorises one.
    let border = first_color(s, &["border"], CLEAR);
    let radius = jnum(s, "radius", 0.0);
    push_panel(out, bx, first_color(s, &["box"], PANEL), None, radius, border, jnum(s, "border_w", 1.0));

    // The tick draws only for a TRUE bind: a box bound to a key the model has not set
    // yet reads as OFF, never blank.
    if props.get("bind_value").and_then(|v| v.as_bool()) == Some(true) {
        // `Rect::inset` clamps a `pad` wider than half the box to a zero extent where
        // the raw arithmetic would go negative — unreachable from any authored style,
        // and the SDF prefers the clamp.
        let tick = bx.inset(jnum(s, "pad", 3.0));
        push_panel(out, tick, first_color(s, &["check"], INK), None, jnum(s, "check_radius", 0.0), CLEAR, 0.0);
    }
    draw_box_caption(r, bx, props, first_color(s, &["label"], INK), out);
}

/// The verdict a BOOL picker returns — a checkbox and a toggle differ only in WHERE
/// their tight region is, so the decision itself is written once.
///
/// Hovering the region claims the pointer; a click inside writes the NEGATION of the
/// bound value. An absent bind reads as false, so the first click on a key the model
/// never set turns it ON — what an author expects of a box that drew unchecked.
/// Reporting the value on idle frames is `echo_binds`'s job, not this one's.
fn bool_pick(over: bool, click: bool, props: &Json) -> HitVerdict {
    let mut v = HitVerdict { hit: over, ..Default::default() };
    if over && click {
        let on = props.get("bind_value").and_then(|b| b.as_bool()) == Some(true);
        v.value = Some(Value::Bool(!on));
    }
    v
}

/// The checkbox's tight region is its BOX — a click on the caption row beside it is
/// inert, and does not even claim.
fn hit_checkbox(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    bool_pick(box_rect(r, props).contains(m), click, props)
}

/// A toggle's **pill**: the style-sized `w`×`h` switch at the node rect's left edge,
/// vertically centred.
///
/// STYLE-owned rather than rect-owned, so a switch dropped into a full-width settings
/// row stays a switch with room for a caption instead of becoming a 400px lozenge —
/// and shared with the hit, which is what leaves the rest of that row inert rather
/// than a second, invisible click target.
fn toggle_pill(r: Rect, s: &Json) -> Rect {
    let (w, h) = (jnum(s, "w", 50.0), jnum(s, "h", 25.0));
    Rect { x: r.x, y: r.y + (r.h - h) * 0.5, w, h }
}

/// The **toggle** — a rounded pill switch: ON draws an `on_top`→`on_bot` gradient
/// track with the knob at the RIGHT, OFF a flat `off_bg` track with the knob at the
/// LEFT. The knob's SIDE is the state and the colour swap only reinforces it, which is
/// why the switch still reads at a glance with the palette taken away.
///
/// Track stops, edge, knob fill and knob X all swing together on the one bit — the
/// toggle OWNS that state→style mapping, exactly as the button owns its hover/press
/// chain. `on_bot` falls back to the sapphire floor rather than to `on_top`, so a style
/// naming only `on_top` still gets the ramp it was drawn for (the chain is preserved
/// verbatim from the module it replaces).
///
/// The knob is the track inset by `knob_pad` all round, so its radius is half its side
/// inside a track whose radius is half ITS height — concentric at any style `h`.
///
/// **props**: `bind_value` — the bound bool, absent reading as off.
/// **style**: `w` (50) · `h` (25) · `radius` (half the pill height) · `border_w` (1) ·
/// ON `on_top` / `on_bot` / `on_border` / `knob_on` · OFF `off_bg` / `off_bot`
/// (defaults to `off_bg`, i.e. flat) / `off_border` / `knob_off` · `knob_pad` (3) ·
/// `knob_radius` (half the knob's side).
fn draw_toggle(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let pill = toggle_pill(r, s);
    let pad = jnum(s, "knob_pad", 3.0);
    let knob = (pill.h - 2.0 * pad).max(0.0);
    let on = props.get("bind_value").and_then(|v| v.as_bool()) == Some(true);

    let (track, track2, border, knob_fill, knob_x) = if on {
        (
            first_color(s, &["on_top"], SAP),
            first_color(s, &["on_bot"], SAP),
            first_color(s, &["on_border"], CLEAR),
            first_color(s, &["knob_on"], RUNE),
            // Right-aligned: the far edge, backed off by the knob and its gutter.
            pill.x + pill.w - pad - knob,
        )
    } else {
        let bg = first_color(s, &["off_bg"], STONE);
        (
            bg,
            first_color(s, &["off_bot", "off_bg"], bg),
            first_color(s, &["off_border"], CLEAR),
            first_color(s, &["knob_off"], DIM),
            pill.x + pad,
        )
    };
    let border_w = jnum(s, "border_w", 1.0);
    push_panel(out, pill, track, Some(track2), jnum(s, "radius", pill.h * 0.5), border, border_w);
    let knob_r = Rect { x: knob_x, y: pill.y + pad, w: knob, h: knob };
    push_panel(out, knob_r, knob_fill, None, jnum(s, "knob_radius", knob * 0.5), CLEAR, 0.0);
}

/// The toggle's tight region is its PILL — a click in the rest of the row it sits in
/// is inert, and does not even claim.
fn hit_toggle(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    let pill = toggle_pill(r, props.get("style").unwrap_or(&Json::Null));
    bool_pick(pill.contains(m), click, props)
}

/// The **radio** — one row of an exclusive group: a circle, a filled dot when this row
/// IS the group's selection, and a caption naming the option.
///
/// It is a checkbox with the two differences that make it a different control. The
/// geometry is ROUND — a radius of half the side, which the panel shader clamps to a
/// circle — so the affordance says *one of these* where a square says *each of these*.
/// And the mark is an EQUALITY, not a bool: this row's literal `value` against
/// `bind_value`, the selection every row of the group shares through one `bind`. A row
/// carrying no `value` can never be the selection, and drawing it permanently unlit is
/// what makes that authoring hole visible.
///
/// Both sides of the comparison reach `props` through `value_to_json`, so it is
/// type-exact — a numeric option never matches a text selection, the same no-coercion
/// rule the module's `==` gave.
///
/// **props**: `box` (14) · `value` (this row's literal id) · `bind_value` (the group's
/// selection) · `label` · `label_x` · `label_gap` (8) · `label_size` (13).
/// **style**: `box` (circle fill) · `radius` (half the box side) · `border` +
/// `border_w` (1) · `pad` (3, the dot's inset) · `check` (dot fill) · `check_radius`
/// (half the dot's side) · `label` (caption colour).
fn draw_radio(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let bx = box_rect(r, props);
    let border = first_color(s, &["border"], CLEAR);
    let fill = first_color(s, &["box"], PANEL);
    push_panel(out, bx, fill, None, jnum(s, "radius", bx.w * 0.5), border, jnum(s, "border_w", 1.0));

    let selected = props
        .get("value")
        .filter(|v| !v.is_null())
        .is_some_and(|v| Some(v) == props.get("bind_value"));
    if selected {
        let dot = bx.inset(jnum(s, "pad", 3.0));
        let dot_fill = first_color(s, &["check"], INK);
        push_panel(out, dot, dot_fill, None, jnum(s, "check_radius", dot.w * 0.5), CLEAR, 0.0);
    }
    // The caption NAMES the option here, so — unlike a checkbox's, which is body copy
    // in someone else's row — it is part of the choice and styles with it.
    draw_box_caption(r, bx, props, first_color(s, &["label"], INK), out);
}

/// The radio's tight region is its CIRCLE (the caption row is inert), and a click
/// inside selects THIS row: the verdict's value — the row's literal string id — sets
/// the group key unconditionally, so the pick wins over every sibling's echo whatever
/// the placement order (an echo only fills a key nobody wrote, and echoes run after the
/// whole hit pass).
///
/// A non-string `value` selects nothing: the group key is the one NAME-keyed picker
/// (`echo_binds` reports it as text), so writing a number there would set a selection
/// no row could ever match.
fn hit_radio(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    let over = box_rect(r, props).contains(m);
    let mut v = HitVerdict { hit: over, ..Default::default() };
    if over && click {
        if let Some(id) = props.get("value").and_then(|v| v.as_str()) {
            v.value = Some(Value::Text(id.to_string()));
        }
    }
    v
}

/// The **tile** — a slot cell: LIT from `style` when its enabling bind says the slot is
/// loaded, EMPTY from `style_off` when it is not.
///
/// TWO style blocks, not one block styled two ways. A loaded equipment slot and an
/// empty socket are different objects on the paperdoll, and giving each its own block
/// lets a scene author them independently instead of smuggling a state flag through a
/// colour. Inside the chosen block the fill is `hot` when the tile is loaded AND its
/// bind is true, else `cell` — an empty tile never reads as selected however its bind
/// happens to sit, because an unloaded slot cannot be the thing you picked. A node
/// naming no `style_off` therefore goes to the const floor when unloaded: the one place
/// a missing authored prop changes the picture rather than being invisible.
///
/// The caption ALWAYS draws — an empty one is an empty text command — because a slot
/// whose art carries the meaning is the normal case, not the exception.
///
/// The HIT is the generic full-rect claim ([`rust_hit_shape`]): the whole cell is the
/// target and a click toggles its bool bind, so it needs no per-kind hit arm.
///
/// **props**: `enabled` (walker-injected from `enabled_bind` — the slot is loaded) ·
/// `bind_value` · `label` · `style_off` (the empty block, resolved like `style`).
/// **style** / **style_off**: `cell` (resting fill) · `hot` (fill while loaded AND on) ·
/// `radius` (0) · `border` + `border_w` (1, the edge drawn only when `border` carries
/// alpha) · `label` (caption colour) · `label_size` (12).
fn draw_tile(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let loaded = props.get("enabled").and_then(|v| v.as_bool()) == Some(true);
    let s = props.get(if loaded { "style" } else { "style_off" }).unwrap_or(&Json::Null);
    let on = loaded && props.get("bind_value").and_then(|v| v.as_bool()) == Some(true);
    let fill = if on { first_color(s, &["hot"], SAP) } else { first_color(s, &["cell"], PANEL) };
    let border = first_color(s, &["border"], CLEAR);
    push_panel(out, r, fill, None, jnum(s, "radius", 0.0), border, jnum(s, "border_w", 1.0));

    let size = jnum(s, "label_size", 12.0);
    let label = props.get("label").and_then(|v| v.as_str()).unwrap_or_default();
    push_text(
        out,
        r.x + r.w * 0.5,
        r.y + (r.h - size) * 0.5,
        label,
        size,
        first_color(s, &["label"], INK),
        TextAlign::Center,
        FontRole::Label,
        false,
        false,
        -1.0,
        None,
    );
}

// ── Option strips (the children-as-data controls) ────────────────────────────
//
// ── THE STRIP-SELECTION BOUNDARY (stated here once, for every option strip) ──
// "Which segment of an option strip is selected" is a 0-based NUMBER, end to end:
// the child `value`, the bind, the model, the echo and the hit verdict all carry
// the same index. An INDEX IS A NUMBER; a NAME (a radio row's literal id, e.g.
// `sec_audio`) is TEXT, and `radio` is the only name-keyed picker. `tabs`,
// `pill_toggle` and `select` are index-keyed and accept NOTHING else — a strip
// whose option carries a non-number `value` warns ([`warn_once`] →
// [`HitVerdict::warn`]) rather than clicking to nothing, because a component that
// took both representations would make the fork the contract.
//
// Their options are the node's CHILDREN read as DATA ([`no_descend`]), never placed
// nodes, so each strip lays its own cells out — which is why draw and hit share one
// geometry fn per kind. A second copy of that arithmetic would drift, and the drift
// reads as "the click landed one segment over".

/// An option child's bound value: its numeric `value`, else `None`.
///
/// There is NO label fallback — a label is display text, an index is a number, and
/// conflating them is the fork the strip boundary above exists to close.
fn option_index(child: &Json) -> Option<f64> {
    child.get("value").and_then(|v| v.as_f64())
}

/// The complaint a strip raises about an option whose `value` is not its index —
/// 1-based like the `props.children[i]` the component walked, and naming the type it
/// actually got, so an author can find the row without counting from zero.
fn strip_value_warn(kind: &str, i: usize, child: &Json) -> String {
    format!(
        "{kind}: option {} `value` must be its numeric index, got {}",
        i + 1,
        lua_type(child.get("value"))
    )
}

/// A pill toggle's **well**: a style-`h`-tall track, vertically centred in the node
/// rect (the rect's own height when the style names none, and never taller than the
/// rect it sits in). Shared by draw and hit, so the track that lights and the track
/// that claims are one track.
fn pill_well(r: Rect, s: &Json) -> Rect {
    let sh = jnum(s, "h", r.h);
    let h = if r.h > 0.0 { sh.min(r.h) } else { sh };
    Rect { x: r.x, y: r.y + ((r.h - h) * 0.5).max(0.0), w: r.w, h }
}

/// The `i`-th of `n` segment **cells**: the well inset by `pad` all round, split into
/// equal segments along x. Shared by draw and hit — the pad rim left over at the well's
/// edges is deliberately part of neither cell, which is what makes a rim click claim
/// the pointer while selecting nothing.
fn pill_cell(well: Rect, pad: f32, n: usize, i: usize) -> Rect {
    let cw = (well.w - pad * 2.0) / n as f32;
    Rect { x: well.x + pad + i as f32 * cw, y: well.y + pad, w: cw, h: (well.h - pad * 2.0).max(0.0) }
}

/// The **pill toggle** — a segmented control: a rounded WELL (fill + hairline edge)
/// split into one cell per option child, the selected cell wearing a floating gradient
/// pill and every cell drawing its child's `label`.
///
/// Selection is `value == bind_value` compared AS AUTHORED — an index in practice (the
/// strip boundary above), but the draw never narrows to a number, so a strip whose
/// options and bind agree on some other representation still lights. The HIT does
/// narrow, because what it writes leaves the control.
///
/// The WELL is the interactive region, not the cells: a click on the `pad` rim between
/// them claims the pointer and selects nothing (see [`hit_pill_toggle`]), so a near-miss
/// inside the control never picks through to the scene behind it.
///
/// **props**: `bind_value` (the selected index) · `children` (`{ value, label }` per
/// segment).
/// **style**: `h` (the node height — the well's track) · `pad` (3) · `radius` (15) ·
/// `bg` (well fill) · `border` + `border_w` (1, the edge drawn only when `border`
/// carries alpha) · `active_top` / `active_bot` (the selected pill's stops — `active_bot`
/// falls back to the sapphire floor, NOT to `active_top`, so a style naming only the top
/// stop still gets the ramp it was drawn for) · `active_inset` (1, the pill's horizontal
/// inset within its cell) · `active_radius` (one under the well's) · `active_label` /
/// `label` (segment label colours) · `label_size` (11).
fn draw_pill_toggle(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let pad = jnum(s, "pad", 3.0);
    let radius = jnum(s, "radius", 15.0);
    let well = pill_well(r, s);

    // The well: a `bg` fill and a hairline `border`.
    let border = first_color(s, &["border"], CLEAR);
    push_panel(out, well, first_color(s, &["bg"], PANEL), None, radius, border, jnum(s, "border_w", 1.0));

    let kids = jkids(props);
    if kids.is_empty() {
        return;
    }
    let want = jopt(props, "bind_value");
    let lsz = jnum(s, "label_size", 11.0);
    for (i, child) in kids.iter().enumerate() {
        let cell = pill_cell(well, pad, kids.len(), i);
        // An absent `value` never wins — a segment nobody authored a value for must not
        // look chosen just because the bind is unset too.
        let active = matches!(jopt(child, "value"), Some(v) if Some(v) == want);
        if active {
            // The floating highlight: an `active_top`→`active_bot` gradient inset within
            // the cell, its radius one under the well's so the two curves nest.
            let inset = jnum(s, "active_inset", 1.0);
            let pill = Rect {
                x: cell.x + inset,
                y: cell.y,
                w: (cell.w - 2.0 * inset).max(0.0),
                h: cell.h,
            };
            push_panel(
                out,
                pill,
                first_color(s, &["active_top"], SAP),
                Some(first_color(s, &["active_bot"], SAP)),
                jnum(s, "active_radius", (radius - 1.0).max(0.0)),
                CLEAR,
                0.0,
            );
        }
        let lc = if active {
            first_color(s, &["active_label"], INK)
        } else {
            first_color(s, &["label"], DIM)
        };
        push_text(
            out,
            cell.x + cell.w * 0.5,
            cell.y + (cell.h - lsz) * 0.5,
            jstr(child, "label"),
            lsz,
            lc,
            TextAlign::Center,
            FontRole::Label,
            false,
            false,
            -1.0,
            None,
        );
    }
}

/// The pill toggle's tight region is its WELL: hovering it claims the pointer, and a
/// click inside a segment CELL selects that child's index. The pad rim claims without
/// selecting, and the idle echo is the walker's generic pass ([`echo_binds`]).
///
/// The last matching cell wins, mirroring the scan order the strip has always had —
/// cells never overlap, so it is only ever one.
fn hit_pill_toggle(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    let s = props.get("style").unwrap_or(&Json::Null);
    let well = pill_well(r, s);
    let mut v = HitVerdict { hit: well.contains(m), ..HitVerdict::default() };
    if !click {
        return v;
    }
    let kids = jkids(props);
    let pad = jnum(s, "pad", 3.0);
    for (i, child) in kids.iter().enumerate() {
        if !pill_cell(well, pad, kids.len(), i).contains(m) {
            continue;
        }
        // A segment's value is its INDEX — a number. Anything else is an authoring
        // error that must SAY so, not click to nothing.
        match option_index(child) {
            Some(val) => v.value = Some(Value::Number(val)),
            None => warn_once(&mut v, strip_value_warn("pill_toggle", i, child)),
        }
    }
    v
}

/// Whether a `tabs` rail stacks its cells TOP-TO-BOTTOM (`vertical`) instead of laying
/// them left-to-right. ONE reader for the axis, so the drawn rows and the clickable rows
/// (both go through [`tab_cell`]) can never disagree. A strict bool, exactly like
/// [`slider_vertical`] — a real `true` stands the rail up, anything else is the default
/// horizontal strip. `pill_toggle` never reads it (it stays horizontal-only).
fn tabs_vertical(props: &Json) -> bool {
    props.get("vertical").and_then(|v| v.as_bool()) == Some(true)
}

/// The `i`-th of `n` tab **cells**: the strip inset by the node's `pad_x`/`pad_y`, split
/// evenly along x with `gap` between cells. Shared by draw and hit — a click in the gap
/// claims the strip and selects nothing precisely because both sides compute the same
/// cells.
///
/// The inset is the strip's LITERAL arithmetic (not [`Rect::inset_xy`], which clamps):
/// a strip padded past its own height must place its labels and its clicks at the same
/// wrong y, not at two different ones.
///
/// A `vertical` rail (see [`tabs_vertical`]) instead stacks the cells DOWN the strip: each
/// spans the full inner WIDTH and takes an equal share of the inner HEIGHT, `gap` between
/// rows — the exact y-axis mirror of the horizontal split. One helper, so draw and hit
/// follow the same axis.
fn tab_cell(r: Rect, props: &Json, n: usize, i: usize) -> Rect {
    let (px, py) = (jnum(props, "pad_x", 0.0), jnum(props, "pad_y", 0.0));
    let gap = jnum(props, "gap", 0.0);
    let inner =
        Rect { x: r.x + px, y: r.y + py, w: r.w - 2.0 * px, h: r.h - 2.0 * py };
    if tabs_vertical(props) {
        let th = ((inner.h - gap * (n as f32 - 1.0)) / n as f32).max(0.0);
        return Rect { x: inner.x, y: inner.y + i as f32 * (th + gap), w: inner.w, h: th };
    }
    let tw = ((inner.w - gap * (n as f32 - 1.0)) / n as f32).max(0.0);
    Rect { x: inner.x + i as f32 * (tw + gap), y: inner.y, w: tw, h: inner.h }
}

/// One tab **cell**: fill / border / label picked down the hover-vs-resting alias
/// chains, then the optional `underline` rule along the bottom edge, then the centred
/// label. The chains are why ONE cell renderer serves an active tab, an idle tab and a
/// hovered one of either — the strip passes the block, the cell reads its state out of it.
///
/// **style**: `fill_top` / `fill_bot` (falling to `active_top` / `active_bot`, then
/// `fill`) · `hover_top` / `hover_bot` (falling to `hot`, then the resting chain) ·
/// `border` / `hover_border` + `border_w` (1) · `radius` (3) · `label` (falling from
/// `hover_label` / `active_label`) · `label_size` (13) · `underline` + `underline_w` (2)
/// + `underline_inset` (0, a rule shorter than its cell).
fn draw_tab_cell(
    r: Rect,
    s: &Json,
    variant: Option<BtnVariant>,
    label: &str,
    hovered: bool,
    out: &mut Vec<HudCommand>,
) {
    let (top, bot, border, lc) = if let Some(v) = variant {
        // A `tab_*_variant` cell draws the compiled button palette (fill/border/label
        // per state) instead of a resolved style block — a rail's active/idle cells ARE
        // buttons, so they wear the one house look ([`BtnVariant`]) as drawing code, and
        // the `modal.buttons.variants.*` blocks they used to name are retired.
        let f = if hovered { v.hover } else { v.idle };
        (f.top, f.bot, f.border, f.label)
    } else if hovered {
        let top = first_color(s, &["hover_top", "hot", "fill_top", "active_top", "fill"], PANEL);
        (
            top,
            first_color(s, &["hover_bot", "hot", "fill_bot", "active_bot", "fill"], top),
            first_color(s, &["hover_border", "border"], CLEAR),
            first_color(s, &["hover_label", "active_label", "label"], INK),
        )
    } else {
        let top = first_color(s, &["fill_top", "active_top", "fill"], PANEL);
        (
            top,
            first_color(s, &["fill_bot", "active_bot", "fill"], top),
            first_color(s, &["border"], CLEAR),
            first_color(s, &["active_label", "label"], INK),
        )
    };
    push_panel(out, r, top, Some(bot), jnum(s, "radius", 3.0), border, jnum(s, "border_w", 1.0));
    // UNDERLINE variant: a style carrying `underline` marks its state with a rule along
    // the cell's bottom edge instead of (or over) the filled pill. This is what a PAGE
    // rail wants — the pill reads as a control, the underline as "where you are" — and
    // it stays one component because the difference is entirely presentational.
    let ul = first_color(s, &["underline"], CLEAR);
    if ul[3] > 0.0 {
        let uw = jnum(s, "underline_w", 2.0);
        let inset = jnum(s, "underline_inset", 0.0);
        push_rect(
            out,
            Rect { x: r.x + inset, y: r.y + r.h - uw, w: (r.w - 2.0 * inset).max(0.0), h: uw },
            ul,
        );
    }
    let lsz = jnum(s, "label_size", 13.0);
    push_text(
        out,
        r.x + r.w * 0.5,
        r.y + (r.h - lsz) * 0.5,
        label,
        lsz,
        lc,
        TextAlign::Center,
        FontRole::Label,
        false,
        false,
        -1.0,
        None,
    );
}

/// The **tab strip** — an optional background bar (the node's own `style`), then one
/// cell per tab child laid across the inner width: the cell whose `value` is the bound
/// selection styles from `tab_active`, the rest from `tab_idle`, and every cell lights
/// on hover.
///
/// Selection is `value == bind_value` compared AS AUTHORED (see [`draw_pill_toggle`]),
/// with one extra rule: an EMPTY `value` never selects. A strip always shows one active
/// tab, so a blank would make the first unauthored one look chosen.
///
/// The strip claims the pointer even BETWEEN cells (see [`hit_tabs`]) — a click in the
/// gutter belongs to the rail, not to whatever lies behind it.
///
/// **props**: `bind_value` (the selected index) · `children` (`{ value, label }` — or
/// `text` — per tab) · `tab_active` / `tab_idle` (the resolved per-state blocks) ·
/// `gap` / `pad_x` / `pad_y` (the node's own layout metrics) · `mx` / `my` (the pointer,
/// for per-cell hover).
/// **style**: the strip's own background bar, drawn through [`draw_panel_bg`] and ONLY
/// when the node carries one. Each cell's palette is [`draw_tab_cell`]'s.
fn draw_tabs(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    // The strip's background bar, only when the node carries a style at all — an
    // unstyled rail is its cells and nothing else.
    if let Some(st) = jopt(props, "style") {
        draw_panel_bg(r, st, out);
    }
    let kids = jkids(props);
    if kids.is_empty() {
        return;
    }
    let active_st = props.get("tab_active").unwrap_or(&Json::Null);
    let idle_st = props.get("tab_idle").unwrap_or(&Json::Null);
    // A rail may name a compiled button VARIANT per state instead of a style block —
    // the retired-`modal.buttons.variants` path. Additive: a strip that names neither
    // draws exactly as before (from its `tab_active`/`tab_idle` blocks).
    let active_var = props.get("tab_active_variant").and_then(|v| v.as_str()).map(button_variant);
    let idle_var = props.get("tab_idle_variant").and_then(|v| v.as_str()).map(button_variant);
    let want = jopt(props, "bind_value");
    let m = pointer(props);
    for (i, child) in kids.iter().enumerate() {
        let cell = tab_cell(r, props, kids.len(), i);
        let active = matches!(jopt(child, "value"), Some(v) if *v != "" && Some(v) == want);
        // `label` is the tab's copy; `text` is the older spelling the chat panel's strip
        // still authors. A PRESENT-but-empty `label` wins, exactly as Lua's `or` chain had it.
        let label = match jopt(child, "label") {
            Some(l) => l.as_str().unwrap_or_default(),
            None => jstr(child, "text"),
        };
        draw_tab_cell(
            cell,
            if active { active_st } else { idle_st },
            if active { active_var } else { idle_var },
            label,
            cell.contains(m),
            out,
        );
    }
}

/// The tab strip's tight region is the WHOLE strip — it claims even between cells, so a
/// click in the gutter does not pick through — and a click inside a tab CELL selects
/// that child's index. The default-to-first echo on idle frames is the walker's generic
/// pass ([`echo_binds`]).
fn hit_tabs(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    let mut v = HitVerdict { hit: r.contains(m), ..HitVerdict::default() };
    let kids = jkids(props);
    for (i, child) in kids.iter().enumerate() {
        if !tab_cell(r, props, kids.len(), i).contains(m) {
            continue;
        }
        // A cell laid PAST the authored strip (a padded rail) still claims — the cell is
        // the tab, and the strip rect is only its usual bound.
        v.hit = true;
        if !click {
            continue;
        }
        // A tab's value is its INDEX — a number. Saying so is the whole point: a
        // silently-ignored value is a tab that clicks to nothing.
        match option_index(child) {
            Some(val) => v.value = Some(Value::Number(val)),
            None => warn_once(&mut v, strip_value_warn("tabs", i, child)),
        }
    }
    v
}

/// A select's popup **outer rect**: flush under the field by the menu block's `gap`,
/// `row_h` per option. Shared by draw and hit so they can never disagree.
///
/// It lies BELOW the node's own rect — the one thing about this control the walker has
/// to know, which is why `state.open` keeps its owner interactive whatever the rect
/// pre-filters say.
fn select_menu(r: Rect, menu: &Json, n: usize) -> (Rect, f32) {
    let row_h = jnum(menu, "row_h", 30.0);
    let rect =
        Rect { x: r.x, y: r.y + r.h + jnum(menu, "gap", 6.0), w: r.w, h: row_h * n as f32 };
    (rect, row_h)
}

/// The `i`-th option **row**: full field width, stacked from the popup's top edge. The
/// ROW is the region — both the hover test and the click test read it — while the band
/// actually drawn is inset within it.
fn select_row(r: Rect, menu: Rect, row_h: f32, i: usize) -> Rect {
    Rect { x: r.x, y: menu.y + i as f32 * row_h, w: r.w, h: row_h }
}

/// Whether an option row is the SELECTED one: its INDEX equals the bound one. Both sides
/// must be numbers ([`option_index`] refuses anything else), so a row carrying a
/// non-index `value` matches only an ABSENT bind — preserved from the module it replaces,
/// because a tier move must change nothing, not even a latent oddity.
fn option_selected(child: &Json, bind: Option<&Json>) -> bool {
    match (option_index(child), bind) {
        (Some(v), Some(b)) => b.as_f64() == Some(v),
        (None, None) => true,
        _ => false,
    }
}

/// The field's display text, and whether it is the (dim) PLACEHOLDER: the label of the
/// option whose index is bound, else the node's `placeholder`.
///
/// The label is looked UP from the index and never stored as the selection — which is
/// what keeps the two directions honest: the row SHOWS a label, the bind CARRIES a number.
fn select_display<'a>(props: &'a Json, kids: &'a [Json]) -> (&'a str, bool) {
    let picked = props
        .get("bind_value")
        .and_then(|v| v.as_f64())
        .and_then(|cur| kids.iter().find(|k| option_index(k) == Some(cur)));
    match picked {
        Some(k) => (jstr(k, "label"), false),
        None => (jstr(props, "placeholder"), true),
    }
}

/// The field's downward **caret**: `steps` stacked 1px rules, each narrower than the
/// last by `size / steps` — a triangle with no glyph-font dependency, so a dropdown
/// needs no icon sheet. `steps` is both the row count and the taper, so the wedge keeps
/// its proportions at any count.
fn draw_select_caret(cx: f32, cy: f32, size: f32, steps: usize, color: [f32; 4], out: &mut Vec<HudCommand>) {
    for i in 0..steps {
        let w = size * (1.0 - i as f32 / steps as f32);
        push_rect(out, Rect { x: cx - w * 0.5, y: cy - 1.0 + i as f32, w, h: 1.0 }, color);
    }
}

/// The **select** — a dropdown: a FIELD (always drawn: panel, the selected option's
/// label or a dim placeholder, and a downward caret) plus, while THIS select is the
/// walker's open popup, a MENU of option rows lifted one sub-layer above the field.
///
/// The popup's commands are emitted at layer 0 like everything else and then lifted as
/// a RUN through [`offset_layer`] — the walker's own mechanism, applied here to the part
/// of one node that must cover the rest of it. Lifting the whole run (rather than each
/// command as it is pushed) is what stops a later row label from being forgotten and
/// painting underneath the popup it belongs to.
///
/// An option's `value` is its 0-based INDEX — a number; the `label` is what the row
/// SHOWS and is never the value (see the strip boundary above).
///
/// **props**: `open` (walker-injected — is THIS select the open popup) · `bind_value`
/// (the selected index) · `children` (`{ value, label }` options) · `placeholder` (the
/// field's text when nothing is selected) · `mx` / `my` (the pointer, for row hover).
/// **style**: two sub-blocks.
/// `field`: `top` / `bot` (stops) · `border` + `border_w` (1) · `radius` (3) ·
/// `pad_x` (14, the label's inset) · `label` / `placeholder` (falling through `caret`,
/// then `label`) · `label_size` (15) · `caret` (falling to `label`) · `caret_size` (9) ·
/// `caret_steps` (4) · `caret_inset` (16, from the field's right edge).
/// `menu`: `top` / `bot` · `border` + `border_w` (1) · `radius` (3) · `gap` (6, under
/// the field) · `row_h` (30) · `row_pad` (4, the band's inset each side) · `sel_bg` /
/// `hover_bg` (row bands) · `sel_label` (falling to `label`) / `label` · `label_size`
/// (15) · `pad_x` (14) · `lift` (1, the popup's sub-layer above the field).
fn draw_select(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let base = props.get("style").unwrap_or(&Json::Null);
    let field = base.get("field").unwrap_or(&Json::Null);
    let menu = base.get("menu").unwrap_or(&Json::Null);
    let kids = jkids(props);

    // ── the field (always drawn) ──
    let top = first_color(field, &["top"], PANEL);
    let border = first_color(field, &["border"], CLEAR);
    push_panel(
        out,
        r,
        top,
        Some(first_color(field, &["bot"], top)),
        jnum(field, "radius", 3.0),
        border,
        jnum(field, "border_w", 1.0),
    );
    let lsz = jnum(field, "label_size", 15.0);
    let (label, placeholder) = select_display(props, kids);
    // A placeholder is not a label with a different colour — it falls down its OWN chain
    // (`placeholder` → `caret` → `label`) so a block that never named one still dims.
    let lc = if placeholder {
        first_color(field, &["placeholder", "caret", "label"], DIM)
    } else {
        first_color(field, &["label"], INK)
    };
    let pad_x = jnum(field, "pad_x", 14.0);
    push_text(out, r.x + pad_x, r.y + (r.h - lsz) * 0.5, label, lsz, lc, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
    draw_select_caret(
        r.x + r.w - jnum(field, "caret_inset", 16.0),
        r.y + r.h * 0.5,
        jnum(field, "caret_size", 9.0),
        jnum(field, "caret_steps", 4.0).max(0.0) as usize,
        first_color(field, &["caret", "label"], DIM),
        out,
    );

    // ── the popup, only while THIS select is open ──
    if props.get("open").and_then(|v| v.as_bool()) != Some(true) {
        return;
    }
    let lifted = out.len();
    let (m_rect, row_h) = select_menu(r, menu, kids.len());
    let mtop = first_color(menu, &["top"], STONE);
    let mborder = first_color(menu, &["border"], CLEAR);
    push_panel(
        out,
        m_rect,
        mtop,
        Some(first_color(menu, &["bot"], mtop)),
        jnum(menu, "radius", 3.0),
        mborder,
        jnum(menu, "border_w", 1.0),
    );
    let msz = jnum(menu, "label_size", 15.0);
    let row_pad = jnum(menu, "row_pad", 4.0);
    let row_x = jnum(menu, "pad_x", 14.0);
    let cur = jopt(props, "bind_value");
    let m = pointer(props);
    for (i, child) in kids.iter().enumerate() {
        let row = select_row(r, m_rect, row_h, i);
        let selected = option_selected(child, cur);
        // The selected row's band, else the hovered one's — inset each side so the
        // popup's own edge stays visible around it. Selection wins over hover.
        let band = if selected {
            Some(first_color(menu, &["sel_bg"], SAP))
        } else if row.contains(m) {
            Some(first_color(menu, &["hover_bg"], STONE))
        } else {
            None
        };
        if let Some(color) = band {
            push_rect(
                out,
                Rect { x: row.x + row_pad, y: row.y, w: row.w - 2.0 * row_pad, h: row.h },
                color,
            );
        }
        let rc = if selected {
            first_color(menu, &["sel_label", "label"], INK)
        } else {
            first_color(menu, &["label"], INK)
        };
        push_text(out, r.x + row_x, row.y + (row_h - msz) * 0.5, jstr(child, "label"), msz, rc, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
    }
    // Lift the whole popup run above the field. The walker adds the NODE's own sub-layer
    // on top of this afterwards, so the +1 stays relative to the field wherever the
    // select sits in a stacked screen.
    let lift = jnum(menu, "lift", 1.0);
    for c in &mut out[lifted..] {
        offset_layer(c, lift);
    }
}

/// The select's hit: the FIELD claims; while open the popup below it claims too. A click
/// on the closed field OPENS it; while open, ANY click closes — matching the settings
/// dropdown — and one inside the popup also picks the row under the pointer. The
/// selected-value echo on idle frames is the walker's generic pass ([`echo_binds`]).
fn hit_select(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    let menu = props.get("style").and_then(|s| s.get("menu")).unwrap_or(&Json::Null);
    let kids = jkids(props);
    let open = props.get("open").and_then(|v| v.as_bool()) == Some(true);
    let (m_rect, row_h) = select_menu(r, menu, kids.len());
    let over_menu = open && m_rect.contains(m);
    let mut v = HitVerdict { hit: r.contains(m) || over_menu, ..HitVerdict::default() };
    if !click {
        return v;
    }
    if !open {
        if r.contains(m) {
            v.open = Some(true);
        }
        return v;
    }
    // Open: every click closes. `apply_hit_verdict` only clears `state.open` when THIS
    // node still owns it, so a stray close is inert rather than shutting someone else's.
    v.open = Some(false);
    if !over_menu {
        return v;
    }
    for (i, child) in kids.iter().enumerate() {
        if !select_row(r, m_rect, row_h, i).contains(m) {
            continue;
        }
        // An option's value is its INDEX — a number. Anything else is an authoring
        // error that must SAY so, not pick to nothing.
        match option_index(child) {
            Some(val) => v.value = Some(Value::Number(val)),
            None => warn_once(&mut v, strip_value_warn("select", i, child)),
        }
    }
    v
}

// ── Value controls (a number you drag, step, or type) ────────────────────────
//
// The three controls whose bind holds a VALUE rather than a choice. Two of them
// (`slider` / `stepper`) render that value through [`fmt_val`], and all three own a
// tight region their draw and hit share one geometry fn for — which is why neither
// can declare a trivial [`rust_hit_shape`].

/// Format a bound number the way a value readout shows it — shared by the slider's
/// readout and the stepper's field.
///
/// `decimals` places (default 2), a leading `+` on a non-negative value when `plus` is
/// set, then `suffix`. All three ride the NODE's own props, so a readout's format is
/// authored WITH the control instead of guessed from the magnitude — which is what lets
/// one row read `0.75` and the next `+3 dB`.
fn fmt_val(v: f32, props: &Json) -> String {
    // A negative `decimals` is nonsense authoring; the cast saturates it to 0 places
    // rather than panicking (Lua's `string.format` errored on the built `"%.-1f"`).
    let dec = jnum(props, "decimals", 2.0).floor().max(0.0) as usize;
    let sign = if props.get("plus").and_then(|b| b.as_bool()) == Some(true) && v >= 0.0 {
        "+"
    } else {
        ""
    };
    format!("{sign}{v:.dec$}{}", jstr(props, "suffix"))
}

/// A normalised position clamped into `0..=1` — how far along a rail the fill reaches,
/// and how far along it a drag has landed.
///
/// Deliberately NOT `f32::clamp`, which PROPAGATES NaN — and both callers can be handed
/// one by authored data: a degenerate `max == min` range makes the value fraction `0/0`,
/// and a rail squeezed to zero width (caption + readout columns wider than the node)
/// makes the pointer fraction the same. This order SWALLOWS it, because `f32::min` hands
/// back the non-NaN operand, so the worst an impossible layout does is peg the control at
/// its top end — never write a NaN into the Model, which nothing downstream would survive.
#[allow(clippy::manual_clamp)]
fn saturate(t: f32) -> f32 {
    t.min(1.0).max(0.0)
}

/// Whether the slider stands UPRIGHT (`vertical`) rather than lying along the row.
///
/// One reader for the three places that fork on it — [`slider_track`], the draw and the
/// hit — so the rail, the fill, the handle and the drag axis can never disagree about
/// which way the control runs.
///
/// A strict bool, matching `config::flag`: the Lua tier tested this for TRUTHINESS, so
/// an authored `"vertical": 1` used to stand the rail up and now does not. Every
/// authored dial writes a real bool, so the tightening is invisible — but it is a
/// tightening, and this is where it is written down.
fn slider_vertical(props: &Json) -> bool {
    props.get("vertical").and_then(|v| v.as_bool()) == Some(true)
}

/// The slider's **track** — the rail the fill and the handle sit on, and the rail a drag
/// maps the pointer over. THE geometry: the draw and the hit both read it, so the value
/// can never live somewhere the click cannot reach.
///
/// Horizontal: inset past the reserved `label_w` / `value_w` columns, `slider_h` tall
/// (default: the whole row), vertically centred. Upright: the caption owns a band across
/// the TOP and the rail takes the rest of the height, `slider_h` WIDE — that prop is the
/// rail's CROSS size either way — horizontally centred. An EMPTY caption reserves no
/// band, so a bare dial rails the full height.
///
/// **props**: `vertical` · `label` · `label_w` (0) · `value_w` (0) · `slider_h` (the row
/// height lying down, 10 upright).
/// **style**: `label_size` (13) · `label_gap` (8 — the upright caption's clearance).
fn slider_track(r: Rect, props: &Json) -> Rect {
    if slider_vertical(props) {
        let s = props.get("style").unwrap_or(&Json::Null);
        let top = if jstr(props, "label").is_empty() {
            0.0
        } else {
            jnum(s, "label_size", 13.0) + jnum(s, "label_gap", 8.0)
        };
        let w = jnum(props, "slider_h", 10.0);
        return Rect { x: r.x + (r.w - w) * 0.5, y: r.y + top, w, h: (r.h - top).max(0.0) };
    }
    let label_w = jnum(props, "label_w", 0.0);
    let h = jnum(props, "slider_h", r.h);
    Rect {
        x: r.x + label_w,
        y: r.y + (r.h - h) * 0.5,
        w: (r.w - label_w - jnum(props, "value_w", 0.0)).max(0.0),
        h,
    }
}

/// The **slider** — a labelled value track: an optional caption column, a rail carrying
/// the fill up to the value with a handle riding on it, and an optional value readout.
///
/// While FOCUSED (its `focus_group` currently holds this node's bind) the caption, rail
/// and fill all recolour together. With no pointer on screen that recolour IS the cursor,
/// which is why it is a full stop swap rather than a tint — the slider OWNS this
/// state→style mapping exactly as the button owns its hover/press chain.
///
/// `vertical` stands the rail up: BOTTOM is min, TOP is max (a planet grows upward), the
/// fill rises from the floor, the handle becomes a bar across the rail, and the caption
/// moves to a band across the top. With `value_w` on, the RANGE marks sit beside the
/// rail's ends and the LIVE value rides beside the handle — "what am I setting" answered
/// at the point of motion instead of in a resting caption that only moves on commit.
/// (That readout brightens while `captured`, which no DRAW props carry today: only
/// [`component_hit_props`] patches that field in. The branch is ported faithfully and is
/// inert until `component_props` injects it — the recorded follow-up, and a change to
/// both tiers rather than part of this move.)
///
/// THE PAD CHANNEL is the walker's and every slider gets it for free: while this node
/// holds focus, d-pad along its own axis steps the bind by `step` (`step_coarse` under
/// the chord), clamped and committed like a stepper click. The cross axis still moves
/// focus, so a slider is never a trap. It keys off the KIND, so this tier move is
/// invisible to it.
///
/// **props**: `bind_value` (the bound number, absent reading as `min`) · `min` (0) ·
/// `max` (1) · `label` · `label_w` (0) · `value_w` (0 — the readout is opt-in) ·
/// `slider_h` · `vertical` · `captured` · `focused` · `decimals` / `plus` / `suffix`
/// (the readout's format, see [`fmt_val`]).
/// **style**: `track` / `focus_track` (rail fill) · `fill` / `focus_fill` (the filled
/// run) · `fill_hi` (a highlight line along the fill's leading edge, drawn only when it
/// carries alpha) + `fill_hi_w` (1) · `handle` + `handle_w` (9) + `handle_over` (4, the
/// handle's overhang past the rail on each side) · `focus_label` (caption + live
/// readout while lit) · `label_size` (13) · `label_gap` (8) · `value_color` (readout +
/// range marks) · `value_size` (12) · `value_gap` (10, upright readout clearance) ·
/// `range_size` (`value_size` − 2, floored at 8).
fn draw_slider(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let vertical = slider_vertical(props);
    let focused = props.get("focused").and_then(|v| v.as_bool()) == Some(true);
    let min = jnum(props, "min", 0.0);
    let max = jnum(props, "max", 1.0);
    let value = jnum(props, "bind_value", min);
    let track = slider_track(r, props);

    // The caption: the left column lying down, the band across the TOP upright — one
    // prop, both orientations, the same focus recolour. Its resting colour is the ink
    // const rather than a style key: no alias ever existed here, and inventing one now
    // would repaint every block that already carries an unrelated `label`.
    let lsz = jnum(s, "label_size", 13.0);
    let label = jstr(props, "label");
    if !label.is_empty() {
        let lc = if focused { first_color(s, &["focus_label"], RUNE) } else { INK };
        let ly = if vertical { r.y } else { r.y + (r.h - lsz) * 0.5 };
        push_text(out, r.x, ly, label, lsz, lc, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
    }

    // Rail, the fill up to the value, then the handle riding on it.
    let track_col = if focused {
        first_color(s, &["focus_track"], STONE)
    } else {
        first_color(s, &["track"], STONE)
    };
    let fill_col =
        if focused { first_color(s, &["focus_fill"], RUNE) } else { first_color(s, &["fill"], SAP) };
    push_rect(out, track, track_col);
    let t = saturate((value - min) / (max - min));
    let hw = jnum(s, "handle_w", 9.0);
    let over = jnum(s, "handle_over", 4.0);
    let fill_hi = first_color(s, &["fill_hi"], CLEAR);
    let hi_w = jnum(s, "fill_hi_w", 1.0);
    let handle = first_color(s, &["handle"], SAP);
    if vertical {
        // The fill RISES from the floor of the rail; the handle is a bar across it.
        let fh = track.h * t;
        let fy = track.y + track.h - fh;
        push_rect(out, Rect { y: fy, h: fh, ..track }, fill_col);
        if fill_hi[3] > 0.0 && fh > 0.0 {
            push_rect(out, Rect { y: fy, h: hi_w, ..track }, fill_hi);
        }
        let hy = track.y + track.h * (1.0 - t);
        push_rect(
            out,
            Rect { x: track.x - over, y: hy - hw * 0.5, w: track.w + 2.0 * over, h: hw },
            handle,
        );
        // The live readout rides BESIDE the handle, on the same `value_w` opt-in the
        // horizontal form spends on a right column; the RANGE marks read off the rail's
        // own ends on the FAR side, so the live number never collides with them.
        if jnum(props, "value_w", 0.0) > 0.0 {
            let vsz = jnum(s, "value_size", 12.0);
            let gap = jnum(s, "value_gap", 10.0);
            let live = props.get("captured").and_then(|v| v.as_bool()) == Some(true);
            let vc = if live {
                first_color(s, &["focus_label"], RUNE)
            } else {
                first_color(s, &["value_color"], DIM)
            };
            push_text(out, track.x + track.w + gap, hy - vsz * 0.5, &fmt_val(value, props), vsz, vc, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
            let rsz = jnum(s, "range_size", (vsz - 2.0).max(8.0));
            let rc = first_color(s, &["value_color"], DIM);
            push_text(out, track.x - gap, track.y - rsz * 0.5, &fmt_val(max, props), rsz, rc, TextAlign::Right, FontRole::Body, false, false, -1.0, None);
            push_text(out, track.x - gap, track.y + track.h - rsz * 0.5, &fmt_val(min, props), rsz, rc, TextAlign::Right, FontRole::Body, false, false, -1.0, None);
        }
        return;
    }
    let fw = track.w * t;
    push_rect(out, Rect { w: fw, ..track }, fill_col);
    if fill_hi[3] > 0.0 && fw > 0.0 {
        push_rect(out, Rect { w: fw, h: hi_w, ..track }, fill_hi);
    }
    push_rect(
        out,
        Rect {
            x: track.x + track.w * t - hw * 0.5,
            y: track.y - over,
            w: hw,
            h: track.h + 2.0 * over,
        },
        handle,
    );
    // The readout, lying down: the right column the rail was already inset for (the
    // upright form drew beside its handle above).
    if jnum(props, "value_w", 0.0) > 0.0 {
        let vsz = jnum(s, "value_size", 12.0);
        let vc = first_color(s, &["value_color"], DIM);
        push_text(out, r.x + r.w, r.y + (r.h - vsz) * 0.5, &fmt_val(value, props), vsz, vc, TextAlign::Right, FontRole::Body, false, false, -1.0, None);
    }
}

/// The slider's hit: the WHOLE row claims and, on a click, grabs group focus — but only
/// the padded GRAB band (the rail ± `grab_pad`, so a press just off a thin rail still
/// takes it) captures the drag. While captured and held, the pointer maps over the rail
/// into the bound value even after it leaves the row, because the walker keeps a
/// captured node dispatching.
///
/// Claiming the whole row while capturing only the band is the point: the caption and
/// the readout are part of the control (clicking them focuses it) without being places
/// a drag can start from and jump the value.
///
/// Everything the verdict names is walker-generic — `group_focus` writes the bind into
/// the node's `focus_group`, `capture` holds `state.dragging` until the button-up edge
/// releases it, and a captured `value` is commit-on-release, landing in `results` only
/// for this frame's draw (see [`apply_hit_verdict`]).
///
/// **props**: `min` (0) · `max` (1) · `captured` · `grab_pad` (6) · plus everything
/// [`slider_track`] reads.
fn hit_slider(m: Vec2, r: Rect, props: &Json, click: bool, down: bool) -> HitVerdict {
    let track = slider_track(r, props);
    let vertical = slider_vertical(props);
    let mut v = HitVerdict { hit: r.contains(m), ..HitVerdict::default() };
    if v.hit && click {
        v.group_focus = true;
        let pad = jnum(props, "grab_pad", 6.0);
        let grab = if vertical {
            Rect { x: track.x - pad, w: track.w + 2.0 * pad, ..track }
        } else {
            Rect { y: track.y - pad, h: track.h + 2.0 * pad, ..track }
        };
        // Only ever `Some(true)`: `Some(false)` would RELEASE a capture, and letting go
        // is the walker's generic button-up rule, never a component's decision.
        v.capture = grab.contains(m).then_some(true);
    }
    // The HELD state, not the click edge — the press frame takes the capture and this
    // same frame maps it, then every held frame after keeps following the hand.
    if down && (props.get("captured").and_then(|c| c.as_bool()) == Some(true) || v.capture == Some(true)) {
        let (min, max) = (jnum(props, "min", 0.0), jnum(props, "max", 1.0));
        let t = if vertical {
            // Bottom is min, top is max: the upright rail maps its pointer INVERTED.
            1.0 - saturate((m.y - track.y) / track.h)
        } else {
            saturate((m.x - track.x) / track.w)
        };
        v.value = Some(Value::Number(f64::from(min + t * (max - min))));
        // A drag that has left the row still claims: the pointer belongs to this control
        // until it is let go.
        v.hit = true;
    }
    v
}

/// The stepper's **field** box and its two square END cells — THE geometry, shared by
/// the draw and the hit so they can never disagree about which pixels step the value.
///
/// The field is inset past the optional caption column, `field_h` tall (default: the
/// whole row) and vertically centred; each end button is as wide as the field is TALL,
/// which is what keeps the two squares square at any authored row height.
///
/// **props**: `label_w` (0) · `field_h` (the row height) · `btn_w` (the field height —
/// override it and the end cells stop being squares, which a dense row may want).
fn stepper_cells(r: Rect, props: &Json) -> (Rect, Rect, Rect) {
    let label_w = jnum(props, "label_w", 0.0);
    let h = jnum(props, "field_h", r.h);
    let field = Rect { x: r.x + label_w, y: r.y + (r.h - h) * 0.5, w: (r.w - label_w).max(0.0), h };
    let bw = jnum(props, "btn_w", field.h);
    (field, Rect { w: bw, ..field }, Rect { x: field.x + field.w - bw, w: bw, ..field })
}

/// The **stepper** — a −[value]+ numeric box: an optional caption column, then a field
/// with two square end buttons painted over it and the formatted value centred between
/// them.
///
/// It is the slider's discrete twin: the same bound number, the same `min`/`max`/format
/// props, but EXACT — a click is worth precisely one `step`, where a drag is worth
/// wherever the hand stopped. That is why the two end cells are the whole interactive
/// region ([`hit_stepper`]) and the value between them is inert: a stepper that also
/// jumped on a field click would be a slider with a bad rail.
///
/// The end buttons paint OVER the field rather than beside it, so the field's own fill
/// shows only in the middle and the three boxes need no gutter arithmetic to line up.
///
/// **props**: `bind_value` (the bound number, absent reading as `min`) · `min` (0) ·
/// `label` · `label_w` (0) · `field_h` · `btn_w` · `dec_glyph` (`-`) / `inc_glyph` (`+`,
/// the end-button faces) · `decimals` / `plus` / `suffix` (see [`fmt_val`]).
/// **style**: `field` / `box` (field fill — BOTH spellings, every authored stepper in
/// the app uses `box`) · `btn` (end-cell fill) · `label` (caption + glyph ink) ·
/// `value_color` (falls back to `label`) · `label_size` (13) · `glyph_size`
/// (`label_size`) · `value_size` (`label_size`).
fn draw_stepper(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let value = jnum(props, "bind_value", jnum(props, "min", 0.0));
    let (field, minus, plus) = stepper_cells(r, props);
    let lsz = jnum(s, "label_size", 13.0);
    let label_col = first_color(s, &["label"], INK);

    // The optional caption in the reserved left column — the slider's, to the pixel, so
    // a stack of the two reads as one column of rows.
    let label = jstr(props, "label");
    if !label.is_empty() {
        push_text(out, r.x, r.y + (r.h - lsz) * 0.5, label, lsz, label_col, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
    }

    // The field, then the two end buttons over it.
    push_rect(out, field, first_color(s, &["field", "box"], PANEL));
    let btn_col = first_color(s, &["btn"], STONE);
    push_rect(out, minus, btn_col);
    push_rect(out, plus, btn_col);

    // The faces, then the value centred in the field. The glyphs are display FONT, not
    // display copy: they are the control's own affordance rather than authored text, so
    // they never pass through the stringtable.
    let gsz = jnum(s, "glyph_size", lsz);
    for (cell, key, dflt) in [(minus, "dec_glyph", "-"), (plus, "inc_glyph", "+")] {
        let glyph = props.get(key).and_then(|v| v.as_str()).unwrap_or(dflt);
        push_text(out, cell.x + cell.w * 0.5, cell.y + (cell.h - gsz) * 0.5, glyph, gsz, label_col, TextAlign::Center, FontRole::Label, false, false, -1.0, None);
    }
    let vsz = jnum(s, "value_size", lsz);
    let vc = first_color(s, &["value_color", "label"], label_col);
    push_text(out, field.x + field.w * 0.5, field.y + (field.h - vsz) * 0.5, &fmt_val(value, props), vsz, vc, TextAlign::Center, FontRole::Body, false, false, -1.0, None);
}

/// The stepper's hit: the whole ROW claims, but only the two square end cells STEP — `-`
/// down, `+` up, by `step` (default 1), clamped to `[min, max]` from the CURRENT bound
/// value. A click on the value between them changes nothing (the every-frame echo still
/// reports it), which is what makes the number safe to click on.
///
/// A discrete, committed gesture: [`apply_hit_verdict`] writes it straight through,
/// where a captured slider's value would be held for the release edge.
///
/// The arithmetic runs in f64 — the width the bind actually is, and the width the
/// walker's pad-nudge channel steps this same bind in — so a click and a d-pad tick
/// land on exactly the same value instead of drifting apart in the last digit.
///
/// **props**: `bind_value` · `min` (0) · `max` (1) · `step` (1) · plus everything
/// [`stepper_cells`] reads.
fn hit_stepper(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    let mut v = HitVerdict { hit: r.contains(m), ..HitVerdict::default() };
    if !click {
        return v;
    }
    let (_, minus, plus) = stepper_cells(r, props);
    let dir = if minus.contains(m) {
        -1.0
    } else if plus.contains(m) {
        1.0
    } else {
        return v;
    };
    let num = |key: &str, dflt: f64| props.get(key).and_then(|n| n.as_f64()).unwrap_or(dflt);
    let (min, max) = (num("min", 0.0), num("max", 1.0));
    // Clamped in the module's order — NOT `f64::clamp`, which PANICS on an inverted
    // authored range where this quietly lets `min` win.
    v.value = Some(Value::Number((num("bind_value", min) + dir * num("step", 1.0)).min(max).max(min)));
    v
}

/// The **text field** — a single-line input in a sunk-black well: the well panel, then
/// the bound string (or the dim `placeholder` while it is empty) left-aligned and
/// vertically centred, plus a block caret at the end of the text while focused.
///
/// The BORDER is a state channel and the field owns the whole mapping: the rune-light
/// focus ring while this field holds keyboard focus, the bronze hover edge under the
/// pointer, else the resting edge. Focus outranks hover — a field you are typing in
/// must not change appearance because the pointer drifted across it.
///
/// The caret is placed by REAL glyph measurement, never `chars × advance` (text ruling
/// 2026-07-31): the whole buffer rides as the command's `prefix` and the render bridge
/// measures the shaped Unicode string, so the bar lands after the last glyph in any
/// script. Its right clamp keeps it inside the well when the text overruns.
///
/// The KEYBOARD is deliberately NOT here. Focus is held by the walker (`state.focus`,
/// claimed through [`hit_text_field`]'s verdict and cleared by the generic clicked-frame
/// rule), typed characters and backspace fold into the bound string in [`fold_typed`],
/// and the every-frame report is [`echo_binds`] — so a typing frame with a parked
/// pointer never needs an interaction pass at all.
///
/// The value is USER DATA: it is never resolved through the stringtable, while the
/// placeholder — display copy — arrives already resolved.
///
/// **props**: `bind_value` (the bound string; a non-string bind reads as empty) ·
/// `placeholder` · `focused` · `mx` / `my` (the pointer, for the hover edge) ·
/// `text_pad` (8) · `caret_w` (2) · `label_size` (14, when the style names none).
/// **style**: `top` / `fill_top` / `bg` · `bot` / `fill_bot` / `bg` (well fill stops) ·
/// `radius` (3) · `border` (resting edge) + `border_w` (1) · `hover_border` ·
/// `caret` / `focus_border` (the ring, and the caret's own colour) + `focus_border_w`
/// (2) · `label` / `color` (value ink) · `placeholder` (empty-state ink) ·
/// `label_size`.
fn draw_text_field(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let focused = props.get("focused").and_then(|v| v.as_bool()) == Some(true);
    // The pointer itself, NOT the walker's `hot` prop: `hot` folds keyboard focus in
    // while the pad drives, so a pad-focused field would light the bronze hover edge
    // underneath its own focus ring.
    let hovered = r.contains(pointer(props));

    let top = first_color(s, &["top", "fill_top", "bg"], STONE);
    let bot = first_color(s, &["bot", "fill_bot", "bg"], top);
    let (border, border_w) = if focused {
        (first_color(s, &["caret", "focus_border", "border"], RUNE), jnum(s, "focus_border_w", 2.0))
    } else if hovered {
        (first_color(s, &["hover_border", "border"], DIM), jnum(s, "border_w", 1.0))
    } else {
        (first_color(s, &["border"], STONE), jnum(s, "border_w", 1.0))
    };
    push_panel(out, r, top, Some(bot), jnum(s, "radius", 3.0), border, border_w);

    // The value, or the placeholder while it is empty. `label_size` reads the STYLE
    // first and the node second: a field's copy sizes with its palette, but one dense
    // row can still shrink its own.
    let lsz = jnum(s, "label_size", jnum(props, "label_size", 14.0));
    let pad = jnum(props, "text_pad", 8.0);
    let value = jstr(props, "bind_value");
    let (shown, color) = if value.is_empty() {
        (jstr(props, "placeholder"), first_color(s, &["placeholder"], DIM))
    } else {
        (value, first_color(s, &["label", "color"], INK))
    };
    let (tx, ty) = (r.x + pad, r.y + (r.h - lsz) * 0.5);
    push_text(out, tx, ty, shown, lsz, color, TextAlign::Left, FontRole::Body, false, false, -1.0, None);

    if focused {
        let cw = jnum(props, "caret_w", 2.0);
        out.push(HudCommand::TextCaret {
            x: tx,
            y: ty,
            w: cw,
            h: lsz,
            // The WHOLE buffer is the prefix — the bridge measures it and offsets the
            // bar by the result, which is the entire reason this is its own command.
            prefix: value.to_string(),
            size: lsz,
            color: first_color(s, &["caret"], RUNE),
            layer: 0.0,
            font: FontRole::Body,
            max_x: r.x + r.w - pad - cw,
        });
    }
}

/// The text field's hit: the well claims on hover, and a click inside claims KEYBOARD
/// focus through the verdict. The walker holds focus by node id, so its generic
/// clicked-frame clear plus this claim is the whole focus life cycle — and a field
/// authored without an `id` can never hold it (and so never shows a caret).
///
/// Deliberately NOT a [`HitShape::Rect`], even though its region IS the whole rect: that
/// generic arm fires the node's `action` and TOGGLES its bind, and this control's bind
/// holds the edited STRING — a rect claim would overwrite the buffer with a bool on
/// every click, and would never claim focus at all.
///
/// Typed characters never arrive here: [`fold_typed`] owns the keyboard, in Rust, every
/// frame — which is what keeps a typing frame with a parked pointer off the interaction
/// path entirely. It reads no props, so it takes none.
fn hit_text_field(m: Vec2, r: Rect, click: bool) -> HitVerdict {
    let over = r.contains(m);
    HitVerdict { hit: over, focus: (over && click).then_some(true), ..HitVerdict::default() }
}

// ── Containers (a surface that holds OTHER content) ──────────────────────────
//
// The two controls whose subject is a region rather than a value: a `list`
// (a scrolling viewport with a bar) and a `context_menu` (a floating stack of
// rows). Both share the container discipline — the component owns the SURFACE
// (backdrop, bar, row washes) and the walker owns the placement — and both own a
// bespoke hit for the same reason the value controls do: a wheel notch and a row
// pick are not claim-and-fire.

/// A `list`'s VIEWPORT: the node rect inset by its own pads (extents floored at 0,
/// which [`Rect::inset_xy`] already guarantees). THE geometry — [`draw_list`]'s bar and
/// [`hit_list`]'s clamp both measure from it, so the thumb can never disagree with the
/// offset it maps.
fn list_viewport(r: Rect, props: &Json) -> Rect {
    r.inset_xy(jnum(props, "pad_x", 0.0), jnum(props, "pad_y", 0.0))
}

/// The **list** — a scrolling column region: an optional styled backdrop plus, when the
/// content overflows the viewport, a right-edge scrollbar (track + proportional thumb
/// with a grab floor).
///
/// The region owns only its SURFACE. The LAYOUT stays the walker's: [`resolve`] flows
/// the children shifted by the bound offset and clips them to the viewport (a structural
/// primitive), then hands this component the resulting [`scroll_content_h`] as
/// `content_h` — the SAME number the placement used, so the bar here and the wheel clamp
/// in [`hit_list`] can never disagree with where the rows actually landed.
///
/// The backdrop draws UNCLIPPED (the children carry the viewport clip, and the bar must
/// stay visible at the edge) and ONLY when the node carries a style: an unstyled region
/// is transparent structure. The bar reads its stops off that same block, falling to the
/// neutral defaults when there is none — [`first_color`] / [`jnum`] on a `Null` behave
/// exactly as a missing block does, so an unstyled region still gets a usable bar.
///
/// **props**: `bind_value` (the scroll offset, px) · `content_h` (walker-measured) ·
/// `pad_x` / `pad_y` (the node's insets — the viewport is the padded rect).
/// **style**: `bar_w` (4) · `bar_inset` (0, from the viewport's right edge) ·
/// `thumb_min` (28, the grab floor) · `track` / `thumb` (the two bar colours) · plus the
/// shared container backdrop keys ([`draw_panel_bg`]: `panel_bg` / `fill` / `border` /
/// `radius` / …).
fn draw_list(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    if !s.is_null() {
        draw_panel_bg(r, s, out);
    }
    let inner = list_viewport(r, props);
    let content_h = jnum(props, "content_h", 0.0);
    let max = content_h - inner.h;
    // Content that FITS gets no bar at all: there is nothing to scroll, and a permanent
    // full-height thumb would only lie about that. Kept in this positive form (rather
    // than an inverted early return) so a NaN `content_h` falls out here too.
    if max > 0.0 {
        let offset = jnum(props, "bind_value", 0.0).max(0.0).min(max);
        let bw = jnum(s, "bar_w", 4.0);
        let track =
            Rect { x: inner.x + inner.w - bw - jnum(s, "bar_inset", 0.0), w: bw, ..inner };
        push_rect(out, track, first_color(s, &["track"], STONE));
        // Proportional thumb (viewport ÷ content, of the viewport) with a floor so a mile
        // of content still leaves something to grab. The free travel shrinks by the same
        // amount, which is what keeps a floored thumb parking exactly at the end.
        let floor = jnum(s, "thumb_min", 28.0).min(inner.h);
        let thumb_h = (inner.h * (inner.h / content_h)).max(floor).min(inner.h);
        let ty = inner.y + (offset / max) * (inner.h - thumb_h);
        push_rect(out, Rect { y: ty, h: thumb_h, ..track }, first_color(s, &["thumb"], SAP));
    }
}

/// The list's hit. The WHOLE rect claims — a click on a scrolling region must not pick
/// through to the scene behind it — and a wheel tick over it writes the offset: the
/// current value minus wheel·speed, clamped to `[0, content − viewport]`.
///
/// Wheel-less frames return the bare CLAIM, deliberately: writing the offset every frame
/// would have this control fight the scene for ownership of its own bind, so the value
/// moves only when the wheel does. The write is uncaptured, so [`apply_hit_verdict`]
/// commits it immediately — a notch is a discrete gesture, not a value in flight.
///
/// The value math runs in f64 (the stepper's rule, not the slider's): a notch ACCUMULATES
/// into the model across ticks, and rounding the running offset through f32 every frame
/// would drift a number the scene also reads. The geometry stays f32 — it is pixels, and
/// it is shared with the draw.
///
/// The one control the walker's `click && enabled` pre-gate does not reach, because it
/// answers no click: a wheel tick rides the PROPS, so a disabled region still scrolls.
/// That is the behaviour as authored — `enabled` gates what a control WRITES on a press,
/// and a scroll position is a view, not an edit.
///
/// **props**: `wheel` (this frame's tick — patched live by [`component_hit_props`], never
/// cached) · `scroll_speed` (46 px per notch, off the NODE: how fast a region scrolls is
/// authored behaviour, not skin) · `bind_value` · `content_h` · the viewport pads.
fn hit_list(m: Vec2, r: Rect, props: &Json) -> HitVerdict {
    if !r.contains(m) {
        return HitVerdict::default();
    }
    let mut v = HitVerdict { hit: true, ..HitVerdict::default() };
    let wheel = jnum(props, "wheel", 0.0);
    if wheel != 0.0 {
        let max = (f64::from(jnum(props, "content_h", 0.0)) - f64::from(list_viewport(r, props).h))
            .max(0.0);
        let cur = props.get("bind_value").and_then(|n| n.as_f64()).unwrap_or(0.0);
        let speed = f64::from(jnum(props, "scroll_speed", 46.0));
        // Clamped in the module's own order — NOT `f64::clamp`, whose `min > max`
        // contract would panic if a future edit let the ceiling go negative.
        v.value = Some(Value::Number((cur - f64::from(wheel) * speed).max(0.0).min(max)));
    }
    v
}

/// The `i`-th (0-based) menu **row**: a full-width, `row_h`-tall band stacked from the
/// TOP of the menu rect — THE geometry, shared by [`draw_context_menu`] and
/// [`hit_context_menu`] so a row's wash and its click region can never disagree. The same
/// stacking [`select_row`] lays a popup's options with, which is why the two read as one
/// control.
///
/// Deliberately UNBOUNDED by the rect: a menu with more items than its authored height
/// covers lays rows past the bottom, and those rows are real (see [`hit_context_menu`]).
fn menu_row(r: Rect, row_h: f32, i: usize) -> Rect {
    Rect { y: r.y + i as f32 * row_h, h: row_h, ..r }
}

/// The **context menu** — a standalone floating menu (DS `ContextMenu`), the reusable
/// form of the popup a `select` draws inline. THE NODE RECT IS THE MENU; its items ride
/// as CHILD DATA, each carrying a `label` (or `text`), an optional right-aligned `hint`
/// keybind, `active` / `disabled` bools, and a `divider` flag that spends the whole band
/// on a hairline instead of a row.
///
/// It reuses the settings dropdown's menu block verbatim, so a menu and a `select` popup
/// are the same object to the eye — that shared block is why the row washes inset by
/// `row_pad` and the labels sit `pad_x` in, rather than at numbers of this component's
/// own choosing.
///
/// Rows stack from the TOP at `row_h` and may run PAST the authored bottom: such a row
/// still hover-washes here and still fires in [`hit_context_menu`], though only the
/// authored rect ever CLAIMS the pointer.
///
/// FLOATING is the author's job, not the component's: a `layer` prop on the node lifts
/// the whole subtree in [`run_ui`]. A `select` has to lift its popup internally because
/// field and popup share one node; a context_menu owns its node, so it needs no internal
/// lift and deliberately has none.
///
/// **props**: `children` (the rows) · `mx` / `my` (the pointer, for the hover wash).
/// **style** (the shared menu block): `top` / `bot` (backdrop stops) · `border` +
/// `border_w` (1) · `radius` (3) · `row_h` (30) · `row_pad` (4, the wash's inset each
/// side) · `pad_x` (14, the label's inset) · `hint_pad` (falls to `pad_x`) ·
/// `label_size` (15) · `label` / `sel_label` (falls to `label`) / `disabled` (falls to
/// `label`) / `hint` · `sel_bg` / `hover_bg` (row washes) · `divider` (falls to `border`)
/// + `divider_inset` (8) / `divider_h` (1, the hairline).
fn draw_context_menu(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let st = props.get("style").unwrap_or(&Json::Null);

    // Menu backdrop: top→bot fill, rounded, and a hairline edge only when the block gives
    // one an alpha. Its stops are the popup's OWN `top`/`bot` keys, not the container
    // alias chain — routing this through `draw_panel_bg` would silently honour
    // `overlay`/`panel_bg`/`bg`/`fill` fallbacks a menu block never carried. Unlike a
    // `list`, it draws unconditionally: a menu with no surface is not a menu.
    let top = first_color(st, &["top"], STONE);
    push_panel(
        out,
        r,
        top,
        Some(first_color(st, &["bot"], top)),
        jnum(st, "radius", 3.0),
        first_color(st, &["border"], CLEAR),
        jnum(st, "border_w", 1.0),
    );

    let row_h = jnum(st, "row_h", 30.0);
    let msz = jnum(st, "label_size", 15.0);
    let row_pad = jnum(st, "row_pad", 4.0);
    let pad_x = jnum(st, "pad_x", 14.0);
    let m = pointer(props);
    for (i, c) in jkids(props).iter().enumerate() {
        let row = menu_row(r, row_h, i);
        if jbool(c, "divider") {
            // A divider spends its whole band on one centred hairline — no wash, no
            // label, and (in the hit) no click.
            let inset = jnum(st, "divider_inset", 8.0);
            push_rect(
                out,
                Rect {
                    x: r.x + inset,
                    y: row.y + (row_h * 0.5).floor(),
                    w: (r.w - 2.0 * inset).max(0.0),
                    h: jnum(st, "divider_h", 1.0),
                },
                first_color(st, &["divider", "border"], DIM),
            );
            continue;
        }
        let (disabled, active) = (jbool(c, "disabled"), jbool(c, "active"));
        // Row wash, inset each side so the menu's own edge stays visible around it: an
        // active row takes the selection fill and KEEPS it under the pointer; only a live
        // row hovers, so a disabled one never promises a click it will not honour.
        let wash = if active {
            Some(first_color(st, &["sel_bg"], SAP))
        } else if !disabled && row.contains(m) {
            Some(first_color(st, &["hover_bg"], STONE))
        } else {
            None
        };
        if let Some(color) = wash {
            push_rect(
                out,
                Rect { x: r.x + row_pad, w: (r.w - 2.0 * row_pad).max(0.0), ..row },
                color,
            );
        }
        // Label (left): disabled dims, active takes the selected-label colour.
        let lc = match (disabled, active) {
            (true, _) => first_color(st, &["disabled", "label"], DIM),
            (_, true) => first_color(st, &["sel_label", "label"], INK),
            _ => first_color(st, &["label"], INK),
        };
        // `label` else `text`, by PRESENCE — an authored `label` shadows `text` even when
        // it is not display copy, which is the module's own selection rule and `jstr`'s
        // ruling about what counts as text.
        let label = if jopt(c, "label").is_some() { jstr(c, "label") } else { jstr(c, "text") };
        let ty = row.y + (row_h - msz) * 0.5;
        push_text(out, r.x + pad_x, ty, label, msz, lc, TextAlign::Left, FontRole::Body, false, false, -1.0, None);
        // Optional right-aligned keybind hint, always dim — a shortcut is a reminder,
        // never competition for the label. Only a STRING is a hint (a stray number would
        // be an authoring error, and drawing nothing is how it shows).
        if let Some(hint) = c.get("hint").and_then(|h| h.as_str()) {
            let hc = first_color(st, &["hint"], DIM);
            let hx = r.x + r.w - jnum(st, "hint_pad", pad_x);
            push_text(out, hx, ty, hint, msz, hc, TextAlign::Right, FontRole::Body, false, false, -1.0, None);
        }
    }
}

/// The context menu's hit. The AUTHORED rect claims the pointer — a gap, a divider or a
/// disabled row must not pick through to the scene behind an open menu — while the ROW
/// loop is deliberately ungated by that rect, so a menu taller than its authored box
/// still fires the rows below it.
///
/// The verdict NAMES the picked row 1-based (`activate_child`) and the walker fires that
/// CHILD's `action`: the items are data of the menu node, so the menu is the dispatch
/// surface and the row is only an index. Dividers and disabled rows are inert.
///
/// No early exit — the LAST match wins, so a click exactly on a shared row edge (both
/// bands contain it, [`Rect::contains`] being inclusive) resolves to the lower row rather
/// than firing two.
fn hit_context_menu(m: Vec2, r: Rect, props: &Json, click: bool) -> HitVerdict {
    let mut v = HitVerdict { hit: r.contains(m), ..HitVerdict::default() };
    if !click {
        return v;
    }
    let row_h = jnum(props.get("style").unwrap_or(&Json::Null), "row_h", 30.0);
    for (i, c) in jkids(props).iter().enumerate() {
        if !jbool(c, "divider") && !jbool(c, "disabled") && menu_row(r, row_h, i).contains(m) {
            v.activate_child = Some(i + 1);
        }
    }
    v
}

/// The **gauge** — a read-only condition read-out: a stone track with a highlighted
/// *habitable band* between `lo` and `hi`, and a marker at the bound value — green
/// (`marker_in`) while the reading sits INSIDE the band, caution-coloured (`marker`)
/// outside it.
///
/// A missing or NEGATIVE reading means *no signal yet*, not zero: the bar is washed
/// with `no_signal` and NO marker is drawn. That distinction is the whole point of the
/// control — an unobserved axis must never read as one pinned at its floor, which is
/// exactly what a marker parked at the left edge would claim.
///
/// The band is STATIC observer data authored on the NODE (`lo` / `hi` props — not a
/// style, not a bind); only the value moves, which is why one proto serves all five
/// habitability axes.
///
/// Presentational: the reading and the band live in the Model (the habitability
/// observer publishes them), the gauge owns only how they DRAW, and it never claims
/// the pointer.
///
/// **props**: `lo` (0.3) · `hi` (0.7) — the band, in bind units · `bind_value` — the
/// reading (absent, non-numeric or negative ⇒ no signal).
/// **style**: `track` (the bar) · `band` (the habitable zone) · `marker` +
/// `marker_in` (the in-band colour, falling back to `marker`) · `marker_w` (4) ·
/// `marker_over` (3 — how far the marker overhangs the track at each end) · `sheen` +
/// `sheen_h` (1, the top hairline) · `no_signal` (the no-reading wash).
fn draw_gauge(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let lo = jnum(props, "lo", 0.3);
    let hi = jnum(props, "hi", 0.7);

    push_rect(out, r, first_color(s, &["track"], WELL));
    // The habitable band — the green zone in the middle of the bar.
    push_rect(
        out,
        Rect { x: r.x + r.w * lo, y: r.y, w: r.w * (hi - lo), h: r.h },
        first_color(s, &["band"], BAND),
    );
    // The top hairline. This and the `no_signal` wash below are gated on ALPHA where
    // the module tested PRESENCE: a stop authored fully transparent painted an
    // invisible rule there and emits nothing here — identical pixels, one command
    // fewer, and the guard `resource_gauge` already used for its own sheen.
    let sheen = first_color(s, &["sheen"], CLEAR);
    if sheen[3] > 0.0 {
        push_rect(out, Rect { h: jnum(s, "sheen_h", 1.0), ..r }, sheen);
    }

    // No signal yet: wash the whole bar out and stop. A NON-NUMERIC reading takes this
    // branch too (the module's `type(value) ~= "number"` guard) — "the observer
    // published nothing usable" and "the observer published nothing" are the same fact
    // to a read-out, and so is a NaN.
    let Some(value) =
        props.get("bind_value").and_then(|v| v.as_f64()).map(|v| v as f32).filter(|v| *v >= 0.0)
    else {
        let wash = first_color(s, &["no_signal"], CLEAR);
        if wash[3] > 0.0 {
            push_rect(out, r, wash);
        }
        return;
    };

    // The marker sits at the CLAMPED position but takes its colour from the RAW
    // reading, so a value past the end of the track still reports out-of-band instead
    // of being dragged into the green by the clamp.
    let keys: &[&str] =
        if value >= lo && value <= hi { &["marker_in", "marker"] } else { &["marker"] };
    let mw = jnum(s, "marker_w", 4.0);
    // A caliper ACROSS the bar rather than a fill inside it: the marker overhangs the
    // track by `marker_over` at each end, which is what keeps it readable over the band.
    let over = jnum(s, "marker_over", 3.0);
    push_rect(
        out,
        Rect {
            x: r.x + r.w * saturate(value) - mw * 0.5,
            y: r.y - over,
            w: mw,
            h: r.h + over * 2.0,
        },
        first_color(s, keys, MARKER),
    );
}

/// The **resource gauge** — the DS resource bar: a sunk stone track with a lit gradient
/// fill, an optional caps label row above it (label left, readout right) and a `low`
/// warning state that swaps the rim and the label to blood.
///
/// Distinct from [`draw_gauge`], which is a band-and-marker READ-OUT: this is a FILLED
/// FRACTION. ONE style block serves every bar — `track` / `border` / `radius` / `pad` /
/// `sheen` are shared and the TONE picks its own `<tone>_top` / `_bot` / `_label` /
/// `_border` stops out of that same block (the badge precedent), so a new resource is a
/// token triple, not a new component. Only `cast` authors a `<tone>_border` today (its
/// bronze rim); every other tone falls through to the shared `border`, and dropping
/// that head from the chain would silently strip the cast bar's rim.
///
/// The LABEL row is optional and reserves height off the top: a bar with no label IS
/// the whole rect (the compact hotbar variant), and a label row that ate the node draws
/// no track at all rather than a smeared one.
///
/// Presentational: a bar reports state and never claims the pointer.
///
/// **props**: `bind_value` — the fill fraction, clamped to 0..=1 · `tone` (`health`) —
/// `health` / `mana` / `stamina` / `cast`, or any tone the block carries stops for; a
/// non-string falls back to `health` · `label` — the caps row's copy, empty ⇒ no row ·
/// `readout` — pre-formatted TEXT for the right of that row (a bound NUMBER reads as
/// empty, the tightening `label` took when the button moved) · `low` — the warning state.
/// **style**: `track` · `border` + `<tone>_border` + `low_border` · `border_w` (1, the
/// rim drawn only when its colour carries alpha) · `radius` (10, capped at half the
/// track height so a short bar reads as a capsule) · `pad` (2, the fill's inset) ·
/// `<tone>_top` / `<tone>_bot` (the fill's two stops) · `sheen` + `sheen_inset` (1) +
/// `sheen_h` (1) · `label_size` (10) · `label_gap` (6, the row's clearance over the
/// track) · `<tone>_label` / `label` / `low_label` · `readout_size` (12) ·
/// `readout_color`.
fn draw_resource_gauge(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let tone = props.get("tone").and_then(|v| v.as_str()).unwrap_or("health");
    let low = jbool(props, "low");

    // The optional caps label row; the track takes whatever height is left.
    let label_sz = jnum(s, "label_size", 10.0);
    let (mut track_y, mut track_h) = (r.y, r.h);
    let label = jstr(props, "label");
    if !label.is_empty() {
        let tone_label = format!("{tone}_label");
        // The warning label is its OWN stop with no tone fallback — a blood label must
        // never quietly come back as the resource's colour.
        let label_color = if low {
            first_color(s, &["low_label"], INK)
        } else {
            first_color(s, &[tone_label.as_str(), "label"], INK)
        };
        push_text(out, r.x, r.y, label, label_sz, label_color, TextAlign::Left, FontRole::Label, false, false, -1.0, None);
        // The readout keeps the SHARED colour in every state: the label carries the tone
        // and the warning, the number stays legible.
        let readout = jstr(props, "readout");
        if !readout.is_empty() {
            let color = first_color(s, &["readout_color"], INK);
            push_text(out, r.x + r.w, r.y, readout, jnum(s, "readout_size", 12.0), color, TextAlign::Right, FontRole::Label, false, false, -1.0, None);
        }
        let row = label_sz + jnum(s, "label_gap", 6.0);
        track_y = r.y + row;
        track_h = r.h - row;
    }
    // A label row that ate the whole node leaves no bar worth drawing.
    if track_h <= 2.0 {
        return;
    }

    // The sunk track: a flat well under a hairline rim, its radius capped at half the
    // height so a short bar reads as a capsule rather than a clipped box.
    let radius = jnum(s, "radius", 10.0).min(track_h * 0.5);
    let tone_border = format!("{tone}_border");
    let border = if low {
        first_color(s, &["low_border", "border"], CLEAR)
    } else {
        first_color(s, &[tone_border.as_str(), "border"], CLEAR)
    };
    let track = Rect { x: r.x, y: track_y, w: r.w, h: track_h };
    push_panel(out, track, first_color(s, &["track"], WELL), None, radius, border, jnum(s, "border_w", 1.0));

    // The lit fill: the tone's two-stop gradient inset by `pad`, plus a sheen along its
    // top edge. A sub-pixel fill is dropped — a bar at half a percent would otherwise
    // draw a sliver that reads as a rounding artefact rather than as a value.
    let pad = jnum(s, "pad", 2.0);
    let frac = saturate(props.get("bind_value").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32);
    let fw = (r.w - 2.0 * pad) * frac;
    if fw > 1.0 {
        let (tone_top, tone_bot) = (format!("{tone}_top"), format!("{tone}_bot"));
        let top = first_color(s, &[tone_top.as_str()], INK);
        let bot = first_color(s, &[tone_bot.as_str()], top);
        let fr = Rect { x: r.x + pad, y: track_y + pad, w: fw, h: track_h - 2.0 * pad };
        push_panel(out, fr, top, Some(bot), (radius - pad).max(0.0), CLEAR, 0.0);
        let sheen = first_color(s, &["sheen"], CLEAR);
        if sheen[3] > 0.0 {
            let inset = jnum(s, "sheen_inset", 1.0);
            push_rect(
                out,
                Rect { x: fr.x + inset, w: (fr.w - 2.0 * inset).max(0.0), h: jnum(s, "sheen_h", 1.0), ..fr },
                sheen,
            );
        }
    }
}

/// The **stat dot** — the DS Septisigil gem-dot, keyed by prism hue (white / yellow /
/// red / orange / black / blue / green). One colour means one stat AND its two schools,
/// so the same dot serves a stat list, a school tag and a legend without any of them
/// agreeing on a second vocabulary.
///
/// Hues are a NESTED block (`hues.<name>` — a `fill` + `glow` pair off the `sig_*`
/// ramp), read straight off that block, which is why the module's local `col()` needed
/// no Rust twin: [`first_color`] already is that reader. The name INDEXES the block, so
/// the `blue` default fires only for an ABSENT `hue` — a mistyped bind delivering a
/// number misses (a key `hues` cannot hold) and the flat floors show through, exactly
/// as the module's `(s.hues or {})[props.hue or "blue"] or {}` did.
///
/// The glow ring is ON by default and `glow = false` flattens it — a legend row wants
/// the colour, not the light. Only a real `false` opts out, so a bind delivering
/// something else still glows.
///
/// Presentational: a dot is a legend, and never claims the pointer.
///
/// **props**: `hue` (`blue`) — names a `hues.<name>` block · `glow` — `false` flattens
/// the ring.
/// **style**: `hues.<name>.fill` / `.glow` · `border` (the gem's rim colour) ·
/// `border_w` (1) · `radius` (half the dot — a rounded sigil is a smaller radius) ·
/// `glow_pad` (4, how far the ring stands off the gem) · `glow_feather` (5).
fn draw_stat_dot(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    // The largest inscribed square, centred — a true circle in any slot.
    let d = r.w.min(r.h);
    let (cx, cy) = (r.x + (r.w - d) * 0.5, r.y + (r.h - d) * 0.5);
    let name = match props.get("hue") {
        // Absent / null / `false` are the Lua-falsy cases the `or "blue"` default was
        // written for; any other non-name is a lookup that misses on purpose.
        None | Some(Json::Null) | Some(Json::Bool(false)) => Some("blue"),
        Some(Json::String(n)) => Some(n.as_str()),
        _ => None,
    };
    let hue = name.and_then(|n| s.get("hues").and_then(|h| h.get(n))).unwrap_or(&Json::Null);
    let radius = jnum(s, "radius", d * 0.5);

    // The glow is on unless the node says EXACTLY `false`: opting out is a deliberate
    // authoring act, so anything else (absent, or a non-bool) still lights.
    if props.get("glow").and_then(|v| v.as_bool()) != Some(false) {
        let glow = first_color(hue, &["glow"], CLEAR);
        if glow[3] > 0.0 {
            let pad = jnum(s, "glow_pad", 4.0);
            // A feathered disc standing `pad` off the gem on every side. Its radius
            // tracks the gem's, so a rounded-square sigil keeps a matching halo.
            out.push(HudCommand::Panel {
                x: cx - pad,
                y: cy - pad,
                w: d + pad * 2.0,
                h: d + pad * 2.0,
                color: glow,
                color2: glow,
                grad: 0.0,
                radius: radius + pad,
                border: 0.0,
                border_color: CLEAR,
                feather: jnum(s, "glow_feather", 5.0),
                layer: 0.0,
            });
        }
    }

    // The gem, rimmed at `border_w` UNCONDITIONALLY — not through [`push_panel`]'s
    // alpha gate, because the two differ here: the shader's band replaces the outer
    // ring with the border colour, so a transparent rim cuts a 1px notch out of the
    // gem's edge. That is how a dot with no authored `border` has always drawn, and
    // `border_w: 0` is the way to ask for a flat disc instead.
    let fill = first_color(hue, &["fill"], SIG_BLUE);
    out.push(HudCommand::Panel {
        x: cx,
        y: cy,
        w: d,
        h: d,
        color: fill,
        color2: fill,
        grad: 0.0,
        radius,
        border: jnum(s, "border_w", 1.0),
        border_color: first_color(s, &["border"], CLEAR),
        feather: 0.0,
        layer: 0.0,
    });
}

/// The **action slot** — the DS ActionSlot: a sunk hotbar recess with a faint bronze
/// rim one step inside it, a centred rune glyph over a rune-light halo, a keybind tag
/// top-left, a charge count bottom-right, and a cooldown veil that recedes as `cd`
/// falls 1→0. `active` lights the sapphire edge and its glow.
///
/// The DS cooldown is a CONIC wipe and the engine has no arc primitive, so the veil is
/// a top-down vertical wipe (the interior's height × `cd`) — the same information in
/// one rect. That substitution is deliberate, and is why the veil is one panel rather
/// than a fan of them.
///
/// The HIT is the generic full-rect claim ([`rust_hit_shape`]): the whole recess is the
/// target and a click fires the node's `action`. A slot has no sub-region — the rim,
/// the tag and the count are all read-out, so a click anywhere on it casts.
///
/// **props** (each also arrives through the generic `<name>_bind` channel): `rune` —
/// the ability glyph · `key` — the keybind tag · `charges` — the count; a NUMBER
/// truncates toward zero (the `%d` it was formatted with) and a STRING passes through
/// (a `"3/5"`, an `"∞"`) · `cd` — the cooldown fraction, clamped to 0..=1 · `active` —
/// the lit-ring state.
/// **style**: `radius` (4) · `bg_top` / `bg_bot` (the recess gradient) · `border` /
/// `active_border` (falling back to `border`, so a block naming one edge still rims a
/// lit slot) / `border_w` (1, the edge drawn only when its colour carries alpha) ·
/// `active_glow` + `glow_pad` (3) + `glow_feather` (4) · `rim` + `rim_w` (1) · `inset`
/// (1 — how far inside the slab the rim and the veil sit, the inner radius following) ·
/// `rune_color` · `rune_scale` (0.42 of the slot height) · `rune_halo` + `halo_scale`
/// (1.2 of the glyph) + `halo_feather` (6) · `key_color` / `key_size` (10) / `key_x` (4)
/// / `key_y` (2) · `charge_color` / `charge_size` (11) / `charge_x` (5, from the right
/// edge) / `charge_y` (2, from the bottom) · `cd_veil`.
fn draw_action_slot(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let active = jbool(props, "active");
    let radius = jnum(s, "radius", 4.0);

    // An ACTIVE slot glows BEFORE the slab, exactly like a hovered button: a feathered
    // disc standing `glow_pad` off every edge, its radius tracking the slab's.
    if active {
        let glow = first_color(s, &["active_glow"], CLEAR);
        if glow[3] > 0.0 {
            let pad = jnum(s, "glow_pad", 3.0);
            out.push(HudCommand::Panel {
                x: r.x - pad,
                y: r.y - pad,
                w: r.w + pad * 2.0,
                h: r.h + pad * 2.0,
                color: glow,
                color2: glow,
                grad: 0.0,
                radius: radius + pad,
                border: 0.0,
                border_color: CLEAR,
                feather: jnum(s, "glow_feather", 4.0),
                layer: 0.0,
            });
        }
    }

    // The recess itself: a dark vertical slab whose edge swaps to sapphire while active.
    let border = if active {
        first_color(s, &["active_border", "border"], CLEAR)
    } else {
        first_color(s, &["border"], CLEAR)
    };
    let top = first_color(s, &["bg_top"], STONE_BTN);
    let bot = first_color(s, &["bg_bot"], WELL);
    push_panel(out, r, top, Some(bot), radius, border, jnum(s, "border_w", 1.0));

    // The recess INTERIOR — one step in from the slab, with the radius stepped down to
    // match. The bronze rim and the cooldown veil are the two things that live in it, so
    // they share the one `inset` rather than agreeing on a literal twice.
    let inset = jnum(s, "inset", 1.0);
    let inner = r.inset(inset);
    let inner_radius = (radius - inset).max(0.0);
    let rim = first_color(s, &["rim"], CLEAR);
    if rim[3] > 0.0 {
        push_panel(out, inner, CLEAR, None, inner_radius, rim, jnum(s, "rim_w", 1.0));
    }

    // The rune glyph, centred, over its halo. The size is a FRACTION of the slot height
    // rounded to whole pixels — a glyph on a half pixel reads as a smudge.
    let rune = jstr(props, "rune");
    if !rune.is_empty() {
        let gsz = (r.h * jnum(s, "rune_scale", 0.42) + 0.5).floor();
        let halo = first_color(s, &["rune_halo"], CLEAR);
        if halo[3] > 0.0 {
            let hw = gsz * jnum(s, "halo_scale", 1.2);
            out.push(HudCommand::Panel {
                x: r.x + (r.w - hw) * 0.5,
                y: r.y + (r.h - hw) * 0.5,
                w: hw,
                h: hw,
                color: halo,
                color2: halo,
                grad: 0.0,
                radius: hw * 0.5,
                border: 0.0,
                border_color: CLEAR,
                feather: jnum(s, "halo_feather", 6.0),
                layer: 0.0,
            });
        }
        push_text(out, r.x + r.w * 0.5, r.y + (r.h - gsz) * 0.5, rune, gsz, first_color(s, &["rune_color"], BRONZE), TextAlign::Center, FontRole::Rune, false, false, -1.0, None);
    }

    // The keybind tag, top-left.
    let key = jstr(props, "key");
    if !key.is_empty() {
        let ksz = jnum(s, "key_size", 10.0);
        push_text(out, r.x + jnum(s, "key_x", 4.0), r.y + jnum(s, "key_y", 2.0), key, ksz, first_color(s, &["key_color"], BRONZE), TextAlign::Left, FontRole::Label, false, false, -1.0, None);
    }

    // The charge count, bottom-right. A NUMBER is truncated toward zero — the `%d` the
    // module formatted it with — and a STRING is passed through, so a slot can read
    // "3/5" or "∞" as well as "3". Anything else has nothing to say and draws no row.
    let charges = match jopt(props, "charges") {
        Some(Json::Number(n)) => n.as_f64().map(|v| (v as i64).to_string()).unwrap_or_default(),
        Some(Json::String(t)) => t.clone(),
        _ => String::new(),
    };
    if !charges.is_empty() {
        let csz = jnum(s, "charge_size", 11.0);
        let y = r.y + r.h - csz - jnum(s, "charge_y", 2.0);
        push_text(out, r.x + r.w - jnum(s, "charge_x", 5.0), y, &charges, csz, first_color(s, &["charge_color"], BRONZE), TextAlign::Right, FontRole::Label, false, false, -1.0, None);
    }

    // The cooldown veil: a top-down wipe over the `cd` fraction of the interior.
    // Unguarded by the veil colour's alpha, exactly as the module drew it — a block
    // naming no `cd_veil` still spends its one transparent command while cooling.
    let cd = saturate(jnum(props, "cd", 0.0));
    if cd > 0.0 {
        let veil = Rect { h: inner.h * cd, ..inner };
        push_panel(out, veil, first_color(s, &["cd_veil"], CLEAR), None, inner_radius, CLEAR, 0.0);
    }
}

/// The **medallion** — the DS PortraitMedallion: a circular gem well set in a metal
/// ring. The ring is a named VARIANT (`bronze` / `sapphire` / `danger` / `ghost` /
/// `gold` — a metal gradient pair under `style.rings`), and the sapphire one, the
/// active-player ring, adds an outer glow disc and a rune-light outline.
///
/// Variants are a NESTED block read straight off `rings.<name>`, which is why the
/// module's local `col()` needed no Rust twin — [`first_color`] already is that reader
/// (the `stat_dot` hue precedent). The name INDEXES the block, so the `bronze` default
/// fires only for an ABSENT `ring`: a mistyped bind delivering a number misses (a key
/// `rings` cannot hold) and the flat bronze floors show through, exactly as the
/// module's `(s.rings or {})[props.ring or "bronze"] or {}` did.
///
/// The well shows a rune glyph. A texture PORTRAIT waits on a circular sprite mask
/// (a primitive gap, logged in the Engine catalog) — the tier move is not the place to
/// close it.
///
/// Presentational: a medallion is a read-out and never claims the pointer.
///
/// **props**: `ring` (`bronze`) — names a `rings.<name>` block, also via `ring_bind` ·
/// `rune` — the glyph in the well · `rune_color` — a resolved rgba override (a dotted
/// path prop), winning over the style's.
/// **style**: `rings.<name>.top` / `.bot` (the metal pair) / `.glow` / `.halo` ·
/// `glow_pad` (5, how far the glow disc and the halo outline stand off the ring) ·
/// `glow_feather` (8) · `halo_w` (1) · `ring_w` (3, the metal band's width) · `bg_top`
/// / `bg_bot` (the well's gradient) · `rim` + `rim_w` (2) · `rune_color` ·
/// `rune_scale` (0.5 of the medallion's diameter).
fn draw_medallion(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    // The largest inscribed square, centred — a true circle in any slot.
    let d = r.w.min(r.h);
    let disc = Rect { x: r.x + (r.w - d) * 0.5, y: r.y + (r.h - d) * 0.5, w: d, h: d };
    let name = match props.get("ring") {
        // Absent / null / `false` are the Lua-falsy cases the `or "bronze"` default was
        // written for; any other non-name is a lookup that misses on purpose.
        None | Some(Json::Null) | Some(Json::Bool(false)) => Some("bronze"),
        Some(Json::String(n)) => Some(n.as_str()),
        _ => None,
    };
    let ring = name.and_then(|n| s.get("rings").and_then(|r| r.get(n))).unwrap_or(&Json::Null);

    // The ACTIVE ring's light, outside the metal: a feathered glow disc under a thin
    // rune-light outline, both standing `glow_pad` off the ring on every side.
    let pad = jnum(s, "glow_pad", 5.0);
    let halo_box = disc.inset(-pad);
    let halo_radius = halo_box.w * 0.5;
    let glow = first_color(ring, &["glow"], CLEAR);
    if glow[3] > 0.0 {
        out.push(HudCommand::Panel {
            x: halo_box.x,
            y: halo_box.y,
            w: halo_box.w,
            h: halo_box.h,
            color: glow,
            color2: glow,
            grad: 0.0,
            radius: halo_radius,
            border: 0.0,
            border_color: CLEAR,
            feather: jnum(s, "glow_feather", 8.0),
            layer: 0.0,
        });
    }
    let halo = first_color(ring, &["halo"], CLEAR);
    if halo[3] > 0.0 {
        push_panel(out, halo_box, CLEAR, None, halo_radius, halo, jnum(s, "halo_w", 1.0));
    }

    // The metal ring: a full-round slab in the variant's gradient pair.
    let ring_top = first_color(ring, &["top"], BRONZE);
    push_panel(out, disc, ring_top, Some(first_color(ring, &["bot"], BRONZE_DIM)), d * 0.5, CLEAR, 0.0);

    // The gem well inside the band: a dark radial look drawn as a vertical pair. A band
    // wider than the medallion leaves no well worth drawing.
    let well = disc.inset(jnum(s, "ring_w", 3.0));
    if well.w > 2.0 {
        let well_top = first_color(s, &["bg_top"], CLEAR);
        let well_bot = first_color(s, &["bg_bot"], well_top);
        // Rimmed UNCONDITIONALLY, not through [`push_panel`]'s alpha gate: the shader's
        // band replaces the well's outer ring with the border colour, so a transparent
        // `rim` deliberately cuts the well's edge back to the metal behind it. That is
        // how a medallion with no authored rim has always drawn (the `stat_dot` gem
        // reads the same way); `rim_w: 0` is the way to ask for a flat well instead.
        out.push(HudCommand::Panel {
            x: well.x,
            y: well.y,
            w: well.w,
            h: well.h,
            color: well_top,
            color2: well_bot,
            grad: if well_bot == well_top { 0.0 } else { 1.0 },
            radius: well.w * 0.5,
            border: jnum(s, "rim_w", 2.0),
            border_color: first_color(s, &["rim"], CLEAR),
            feather: 0.0,
            layer: 0.0,
        });
    }

    // The rune glyph, centred in the well. Its size is a FRACTION of the medallion,
    // rounded to whole pixels — a glyph on a half pixel reads as a smudge.
    let rune = jstr(props, "rune");
    if !rune.is_empty() {
        let gsz = (d * jnum(s, "rune_scale", 0.5) + 0.5).floor();
        // A dotted `rune_color` prop (already resolved to rgba by the walker) overrides
        // the style block's — one unit's affinity colour without a style block per unit.
        let color = first_color(props, &["rune_color"], first_color(s, &["rune_color"], RUNE));
        push_text(out, disc.x + d * 0.5, disc.y + (d - gsz) * 0.5, rune, gsz, color, TextAlign::Center, FontRole::Rune, false, false, -1.0, None);
    }
}

/// A badge's **pill**: `pad`-inset horizontally, the style's `h` tall (never taller
/// than the node), vertically centred in the node rect.
///
/// THE geometry — [`draw_badge`] and [`hit_badge`] both read it, so what the eye sees
/// and what the pointer claims can never drift apart, and the rim a style insets stays
/// inert instead of becoming an invisible target. STYLE-owned rather than rect-owned so
/// a chip dropped into a tall row stays a chip; an `h`-less style fills the node.
fn badge_pill(r: Rect, s: &Json) -> Rect {
    let pad = jnum(s, "pad", 0.0);
    let sh = jnum(s, "h", r.h);
    // A node with no height yet (measured before layout) leaves the style's own height
    // standing alone rather than clamping the chip to nothing.
    let h = if r.h > 0.0 { sh.min(r.h) } else { sh };
    Rect { x: r.x + pad, y: r.y + ((r.h - h) * 0.5).max(0.0), w: (r.w - pad * 2.0).max(0.0), h }
}

/// The **badge** — a small rounded chip with a centred label: a `solid` badge fills
/// bronze, otherwise its `tone` picks a background/label pair out of the one block.
///
/// TONE IS A PREFIX, not a fixed enum: any `tone` reads its own `<tone>_bg` /
/// `<tone>_label` stops and falls through to the `neutral` pair when the block carries
/// none — so a new chip colour is a token pair in `ui_theme.json`, not a new arm
/// here (the pattern `resource_gauge` names as "the badge precedent"). The two DS tones
/// that carry their own missing-key floor, `accent` (sapphire) and `bronze` (stone),
/// keep it: their chains are exactly the module's, so a block naming only one of them
/// still gets the colour it was drawn for.
///
/// `solid` WINS over any tone — the loudest state a badge has, and a row that sets both
/// means the loud one.
///
/// **props**: `label` — the chip's copy · `solid` — the filled-bronze state (a STRICT
/// bool) · `tone` (`neutral`) — the stop prefix · `label_size` (11, the style's own
/// `label_size` overriding it).
/// **style**: `pad` (0) · `h` (the node's height) · `radius` (half the pill — a full
/// capsule) · `border` + `border_w` (1, the edge drawn only when `border` carries
/// alpha) · `<tone>_bg` / `<tone>_label` · `solid_bg` / `solid_label` · `label_size`.
fn draw_badge(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = props.get("style").unwrap_or(&Json::Null);
    let pill = badge_pill(r, s);

    let (bg, label_color) = if jbool(props, "solid") {
        (first_color(s, &["solid_bg"], INK), first_color(s, &["solid_label"], STONE))
    } else {
        // A non-string `tone` (a mistyped bind) reads as `neutral`, exactly as the
        // module's two string compares both failing did.
        match props.get("tone").and_then(|v| v.as_str()).unwrap_or("neutral") {
            "accent" => {
                (first_color(s, &["accent_bg"], SAP), first_color(s, &["accent_label"], INK))
            }
            "bronze" => {
                (first_color(s, &["bronze_bg"], STONE), first_color(s, &["bronze_label"], INK))
            }
            other => {
                let (bg, label) = (format!("{other}_bg"), format!("{other}_label"));
                (
                    first_color(s, &[bg.as_str(), "neutral_bg"], PANEL),
                    first_color(s, &[label.as_str(), "neutral_label"], DIM),
                )
            }
        }
    };

    let border = first_color(s, &["border"], CLEAR);
    push_panel(out, pill, bg, None, jnum(s, "radius", pill.h * 0.5), border, jnum(s, "border_w", 1.0));
    // The style's size wins over the node's, which is the one prop a dense strip tunes.
    let lsz = jnum(s, "label_size", jnum(props, "label_size", 11.0));
    push_text(out, pill.x + pill.w * 0.5, pill.y + (pill.h - lsz) * 0.5, jstr(props, "label"), lsz, label_color, TextAlign::Center, FontRole::Label, false, false, -1.0, None);
}

/// The badge's tight region is its PILL — a style that insets the chip (`pad` / `h`)
/// leaves the rim around it inert rather than claiming it.
///
/// A badge only CLAIMS: it is a status chip over UI surface, so the scene must not pick
/// through it, but it writes no bind and fires no action. That is the whole verdict —
/// which is why it needs a bespoke arm rather than [`HitShape::Rect`], whose claim
/// would be the node rect and whose click would fire an action a chip does not have.
fn hit_badge(m: Vec2, r: Rect, props: &Json) -> HitVerdict {
    let pill = badge_pill(r, props.get("style").unwrap_or(&Json::Null));
    HitVerdict { hit: pill.contains(m), ..Default::default() }
}


// ── Composites (engine-drawn assemblies) ─────────────────────────────────────
//
// A `popup_panel` (the carved modal slab) and a `paged_menu` (the two-rail page/tab
// control, "PTT"). Each was a template BUILDER that `expand()` spliced into a `cell`
// tree; now the engine lays it out, draws it and hit-tests it at walk time from the one
// resolved-block prop surface every component obeys (201F4F51 P1). Layout / draw / hit
// share a per-kind geometry helper, so the reserved chrome space can never drift.

/// A single drawn text line's reserved height — glyph size + the walker's default
/// `leading` (10), the SAME basis [`measure`]'s `text` arm uses, so the chrome a
/// composite DRAWS lines up with where an equivalent `text` node would have flowed.
fn text_line_h(size: f32) -> f32 {
    size + 10.0
}

/// The vertical geometry a [`popup_panel`] reserves for its drawn chrome, computed ONCE
/// for resolve (which places the items below it), measure (which sizes the slab) and
/// draw (which paints it). `title` is always reserved — empty copy still holds its line,
/// faithful to the builder; `subtitle`/`divider`/`footer` reserve space only when
/// authored. Positions are absolute (from `rect`), so measure passes a `y = 0` rect and
/// reads `items_top` as the top block's height.
struct PopupChrome {
    inner_x: f32,
    inner_w: f32,
    pad: f32,
    gap: f32,
    items_gap: f32,
    title_y: f32,
    title_size: f32,
    subtitle_y: Option<f32>,
    subtitle_size: f32,
    divider_y: Option<f32>,
    items_top: f32,
    has_footer: bool,
    footer_size: f32,
}

fn popup_chrome(node: &UiNode, rect: Rect) -> PopupChrome {
    let pad = pnum(node, "panel_pad").unwrap_or(38.0) as f32;
    let gap = pnum(node, "panel_gap").unwrap_or(16.0) as f32;
    let items_gap = pnum(node, "items_gap").unwrap_or(12.0) as f32;
    let title_size = pnum(node, "title_size").unwrap_or(52.0) as f32;
    let subtitle_size = pnum(node, "subtitle_size").unwrap_or(11.0) as f32;
    let footer_size = pnum(node, "footer_size").unwrap_or(10.0) as f32;
    // A subtitle reserves its line when authored statically OR bound live
    // (`subtitle_bind` — the display-confirm countdown). A bound line always shows,
    // so LAYOUT needs no model handle to know it is there.
    let has_subtitle = ptext(node, "subtitle").is_some_and(|s| !s.is_empty())
        || ptext(node, "subtitle_bind").is_some();
    let has_footer = ptext(node, "footer").is_some_and(|s| !s.is_empty());

    let mut y = rect.y + pad;
    let title_y = y;
    y += text_line_h(title_size);
    let subtitle_y = if has_subtitle {
        y += gap;
        let sy = y;
        y += text_line_h(subtitle_size);
        Some(sy)
    } else {
        None
    };
    let divider_y = if pbool(node, "divider") {
        y += gap;
        let dy = y;
        y += 1.0;
        Some(dy)
    } else {
        None
    };
    y += gap; // the gap between the title block and the first item
    PopupChrome {
        inner_x: rect.x + pad,
        inner_w: (rect.w - pad * 2.0).max(0.0),
        pad,
        gap,
        items_gap,
        title_y,
        title_size,
        subtitle_y,
        subtitle_size,
        divider_y,
        items_top: y,
        has_footer,
        footer_size,
    }
}

/// The **popup panel** — the carved modal slab the pause / confirm / menu popups build
/// on. It DRAWS its chrome — the styled backdrop, an always-present centred title, an
/// optional subtitle, an optional 1px divider and an optional footer — while its ITEMS
/// are the authored child nodes the walker placed (and draws) as ordinary controls. The
/// panel writes no bind and fires no action; it claims its whole rect ([`rust_hit_shape`]
/// = `Rect`) so a click on the slab does not pick through to the scene behind it.
fn draw_popup_panel(r: Rect, node: &UiNode, props: &Json, out: &mut Vec<HudCommand>) {
    if let Some(st) = jopt(props, "panel_style") {
        draw_panel_bg(r, st, out);
    }
    let c = popup_chrome(node, r);
    let cx = c.inner_x + c.inner_w * 0.5;
    // Title — always drawn (empty copy centres nothing, faithful to the proto's `@title=`).
    let title = crate::strings::resolve(ptext(node, "title").unwrap_or_default());
    push_text(out, cx, c.title_y, &title, c.title_size, first_color(props, &["title_color_rgba"], INK), TextAlign::Center, FontRole::Display, false, false, -1.0, None);
    if let Some(sy) = c.subtitle_y {
        // A bound subtitle's LIVE text (injected by `component_props`) wins over the
        // authored static copy; both resolve $tokens the same way.
        let sub = match jopt(props, "subtitle_live").and_then(|v| v.as_str()) {
            Some(live) => crate::strings::resolve(live),
            None => crate::strings::resolve(ptext(node, "subtitle").unwrap_or_default()),
        };
        push_text(out, cx, sy, &sub, c.subtitle_size, first_color(props, &["subtitle_color_rgba"], DIM), TextAlign::Center, FontRole::Label, false, false, -1.0, None);
    }
    if let Some(dy) = c.divider_y {
        if let Some(ds) = jopt(props, "divider_style") {
            draw_panel_bg(Rect { x: c.inner_x, y: dy, w: c.inner_w, h: 1.0 }, ds, out);
        }
    }
    if c.has_footer {
        let foot = crate::strings::resolve(ptext(node, "footer").unwrap_or_default());
        // Pinned to the slab's bottom edge (its measured height reserved this line + the
        // bottom pad), so it sits below the last item wherever the item count lands.
        let fy = r.y + r.h - c.pad - text_line_h(c.footer_size);
        push_text(out, cx, fy, &foot, c.footer_size, first_color(props, &["footer_color_rgba"], DIM), TextAlign::Center, FontRole::Label, false, false, -1.0, None);
    }
}

/// The sub-rects a [`paged_menu`] lays out inside its padded frame — computed ONCE so
/// resolve (places the authored rails + content), draw (backdrop + rule + hints) and hit
/// (the four hint gutters) can never disagree. Each rail rect is `Some` only when that
/// rail is actually shown: the PAGE rail needs a `tabs` child and `!hide_pages`; the RULE
/// needs a `pill_toggle` child and `!hide_tabs`; the TAB rail needs those AND `tabs_shown`
/// (its collapse rides that gate, exactly as the builder's `visible_bind` did).
struct PagedLayout {
    page_rail: Option<Rect>, // where the `tabs` child is placed (middle of the page band)
    lt: Option<Rect>,
    rt: Option<Rect>,
    rule: Option<Rect>,
    tab_pill: Option<Rect>, // where the `pill_toggle` child is placed
    lb: Option<Rect>,
    rb: Option<Rect>,
    content: Rect,
}

fn paged_layout(node: &UiNode, outer: Rect, model: &ValueMap) -> PagedLayout {
    // LEFT page-rail mode (`page_side: "left"`): the PAGE rail stands up as a fixed-width
    // LEFT COLUMN (the authored `vertical` `tabs` child), a 1px vertical rule divides it
    // from the RIGHT area, and that right area carries the ORDINARY horizontal band layout —
    // the tab rail (still horizontal, with its LB/RB hints) over the content. There are NO
    // LT/RT page-hint gutters here (a vertical category rail is clicked directly, and still
    // page-cycles via signals). Branched at the very top so the default top-rail path below
    // stays byte-for-byte identical.
    if ptext(node, "page_side") == Some("left") {
        return paged_layout_left(node, outer, model);
    }
    // The frame inset is the node's own `pad` structural field (scene-authored
    // arrangement), read via the standard `inner` insets like every other container.
    let inner = outer.inset_xy(pad_x(node), pad_y(node));
    let child = |kind: &'static str| node.children.iter().find(move |c| visible(c, model) && c.component == kind);
    let has_page = child("tabs").is_some() && !pbool(node, "hide_pages");
    let has_tabsec = child("pill_toggle").is_some() && !pbool(node, "hide_tabs");
    let tabs_open = has_tabsec && model.is_on(ptext(node, "tabs_shown").unwrap_or("paged_tabs_shown"));

    let mut y = inner.y;
    let (mut page_rail, mut lt, mut rt) = (None, None, None);
    if has_page {
        let rail_h = pnum(node, "rail_h").unwrap_or(42.0) as f32;
        let hw = pnum(node, "hint_w").unwrap_or(54.0) as f32;
        let rgap = pnum(node, "rail_gap").unwrap_or(30.0) as f32;
        lt = Some(Rect { x: inner.x, y, w: hw, h: rail_h });
        rt = Some(Rect { x: inner.x + inner.w - hw, y, w: hw, h: rail_h });
        page_rail = Some(Rect { x: inner.x + hw + rgap, y, w: (inner.w - 2.0 * (hw + rgap)).max(0.0), h: rail_h });
        y += rail_h;
    }
    let mut rule = None;
    if has_tabsec {
        rule = Some(Rect { x: inner.x, y, w: inner.w, h: 1.0 });
        y += 1.0;
    }
    let (mut tab_pill, mut lb, mut rb) = (None, None, None);
    if tabs_open {
        let tab_h = pnum(node, "tab_h").unwrap_or(44.0) as f32;
        let hw2 = pnum(node, "hint_w2").unwrap_or(46.0) as f32;
        let tgap = pnum(node, "tab_gap").unwrap_or(20.0) as f32;
        // The pill carries its own width (its `size` in the horizontal band); the
        // [LB · pill · RB] cluster is centred, exactly as the builder's grow spacers did.
        let pill_w = child("pill_toggle").map(|c| child_main(c, model, true)).unwrap_or(0.0);
        let cluster = 2.0 * hw2 + 2.0 * tgap + pill_w;
        let cx = inner.x + ((inner.w - cluster) * 0.5).max(0.0);
        lb = Some(Rect { x: cx, y, w: hw2, h: tab_h });
        tab_pill = Some(Rect { x: cx + hw2 + tgap, y, w: pill_w, h: tab_h });
        rb = Some(Rect { x: cx + hw2 + tgap + pill_w + tgap, y, w: hw2, h: tab_h });
        y += tab_h;
    }
    let content = Rect { x: inner.x, y, w: inner.w, h: (inner.y + inner.h - y).max(0.0) };
    PagedLayout { page_rail, lt, rt, rule, tab_pill, lb, rb, content }
}

/// [`paged_layout`] for `page_side: "left"` — the vertical-page-rail variant. Splits the
/// padded frame into a fixed `page_w`-wide LEFT COLUMN (where the `vertical` `tabs` page
/// rail is placed) and a RIGHT area, with a 1px vertical `rule` + a small `page_gap`
/// between them; the right area then gets the SAME `[LB · pill · RB]` tab band over grow-
/// content the top-rail path lays inside `inner`, only WITHOUT a page band or its LT/RT
/// gutters (`lt`/`rt` = `None`). No page rail (no `tabs` child, or `hide_pages`) collapses
/// the column so the right area is the whole inner — the axis twin of the top path dropping
/// its page band. Duplicates the tab-band math on purpose: the top path stays untouched.
fn paged_layout_left(node: &UiNode, outer: Rect, model: &ValueMap) -> PagedLayout {
    let inner = outer.inset_xy(pad_x(node), pad_y(node));
    let child = |kind: &'static str| node.children.iter().find(move |c| visible(c, model) && c.component == kind);
    let has_page = child("tabs").is_some() && !pbool(node, "hide_pages");
    let has_tabsec = child("pill_toggle").is_some() && !pbool(node, "hide_tabs");
    let tabs_open = has_tabsec && model.is_on(ptext(node, "tabs_shown").unwrap_or("paged_tabs_shown"));

    // The left column + the vertical rule, and the right area that remains.
    let (page_rail, rule, right) = if has_page {
        let page_w = pnum(node, "page_w").unwrap_or(200.0) as f32;
        let gap = pnum(node, "page_gap").unwrap_or(12.0) as f32;
        let page_rail = Rect { x: inner.x, y: inner.y, w: page_w, h: inner.h };
        let rule = Rect { x: inner.x + page_w, y: inner.y, w: 1.0, h: inner.h };
        let rx = inner.x + page_w + gap + 1.0;
        let right = Rect { x: rx, y: inner.y, w: (inner.x + inner.w - rx).max(0.0), h: inner.h };
        (Some(page_rail), Some(rule), right)
    } else {
        (None, None, inner)
    };

    // The tab rail + content, laid horizontally inside the RIGHT area — the identical
    // centred cluster + grow-content the top path lays inside `inner`, minus the page band.
    let mut y = right.y;
    let (mut tab_pill, mut lb, mut rb) = (None, None, None);
    if tabs_open {
        let tab_h = pnum(node, "tab_h").unwrap_or(44.0) as f32;
        let hw2 = pnum(node, "hint_w2").unwrap_or(46.0) as f32;
        let tgap = pnum(node, "tab_gap").unwrap_or(20.0) as f32;
        let pill_w = child("pill_toggle").map(|c| child_main(c, model, true)).unwrap_or(0.0);
        let cluster = 2.0 * hw2 + 2.0 * tgap + pill_w;
        let cx = right.x + ((right.w - cluster) * 0.5).max(0.0);
        lb = Some(Rect { x: cx, y, w: hw2, h: tab_h });
        tab_pill = Some(Rect { x: cx + hw2 + tgap, y, w: pill_w, h: tab_h });
        rb = Some(Rect { x: cx + hw2 + tgap + pill_w + tgap, y, w: hw2, h: tab_h });
        y += tab_h;
    }
    let content = Rect { x: right.x, y, w: right.w, h: (right.y + right.h - y).max(0.0) };
    PagedLayout { page_rail, lt: None, rt: None, rule, tab_pill, lb, rb, content }
}

/// One rail hint — a single atlas cell (from the resolved `glyph_style` block) centred
/// in its gutter rect, the SAME emit convention [`draw_button`] uses for a glyph face.
fn draw_paged_hint(rect: Option<Rect>, name: &str, size: f32, glyph_style: &Json, flash: f32, out: &mut Vec<HudCommand>) {
    let Some(rect) = rect else { return };
    let p = serde_json::json!({ "glyph": name, "glyph_size": size, "glyph_style": glyph_style.clone() });
    draw_glyph_face(rect, &p, flash, out);
}

/// The **paged menu** (PTT) — the two-rail page/tab control. It DRAWS its frame backdrop
/// (the node's own `style`), the 1px rule between the rails, and the four controller-glyph
/// hints (`lt`/`rt` flanking the page rail, `lb`/`rb` the tab rail). The rails themselves
/// are AUTHORED child components (a `tabs` page rail, a `pill_toggle` tab rail) the walker
/// placed at the band rects; the content is every other child, flowed below. The rails own
/// their own stepping — the hints only FIRE their step names (see [`hit_paged_menu`]).
fn draw_paged_menu(r: Rect, node: &UiNode, model: &ValueMap, props: &Json, out: &mut Vec<HudCommand>) {
    if let Some(st) = jopt(props, "style") {
        draw_panel_bg(r, st, out);
    }
    let lay = paged_layout(node, r, model);
    if let (Some(rule), Some(rs)) = (lay.rule, jopt(props, "rule_style")) {
        draw_panel_bg(rule, rs, out);
    }
    let glyph_style = props.get("glyph_style").unwrap_or(&Json::Null);
    let hg = pnum(node, "hint_glyph").unwrap_or(26.0) as f32;
    let hg2 = pnum(node, "hint_glyph2").unwrap_or(22.0) as f32;
    // The gutter under a PRESSED pointer lights its glyph — the click highlight the hints
    // carry, restored on the native PTT kind. Press-only (`pressed` = mouse-down over the
    // frame, `mx`/`my` = the injected pointer): it clears on release, so no highlight
    // lingers on the last-clicked hint.
    let pressed = props.get("pressed").and_then(|v| v.as_bool()).unwrap_or(false);
    let mouse = Vec2::new(
        props.get("mx").and_then(|v| v.as_f64()).unwrap_or(f64::from(f32::MIN)) as f32,
        props.get("my").and_then(|v| v.as_f64()).unwrap_or(f64::from(f32::MIN)) as f32,
    );
    let hot = |rect: Option<Rect>| {
        if pressed && rect.is_some_and(|rc| rc.contains(mouse)) {
            1.0
        } else {
            0.0
        }
    };
    draw_paged_hint(lay.lt, "lt", hg, glyph_style, hot(lay.lt), out);
    draw_paged_hint(lay.rt, "rt", hg, glyph_style, hot(lay.rt), out);
    draw_paged_hint(lay.lb, "lb", hg2, glyph_style, hot(lay.lb), out);
    draw_paged_hint(lay.rb, "rb", hg2, glyph_style, hot(lay.rb), out);
}

/// The PTT's hit: the frame CLAIMS (a click on the page background does not pick through),
/// and a click in a hint GUTTER fires the neighbouring rail's step name — LT/RT read the
/// PAGE rail's `prev_action`/`next_action`, LB/RB the TAB rail's. The name rides the full
/// activation channel ([`HitVerdict::fire`]), so the rail steps ITSELF on the very name a
/// shoulder signal or a pad Confirm on that rail carries — one channel, no scene stepper.
fn hit_paged_menu(m: Vec2, r: Rect, node: &UiNode, model: &ValueMap, click: bool) -> HitVerdict {
    let mut v = HitVerdict { hit: r.contains(m), ..HitVerdict::default() };
    if !click {
        return v;
    }
    let lay = paged_layout(node, r, model);
    let rail = |kind: &'static str| node.children.iter().find(move |c| visible(c, model) && c.component == kind);
    let act = |n: Option<&UiNode>, key: &str| n.and_then(|n| ptext(n, key)).filter(|s| !s.is_empty()).map(str::to_string);
    let over = |rect: Option<Rect>| rect.is_some_and(|rc| rc.contains(m));
    if over(lay.lt) {
        v.fire = act(rail("tabs"), "prev_action");
    } else if over(lay.rt) {
        v.fire = act(rail("tabs"), "next_action");
    } else if over(lay.lb) {
        v.fire = act(rail("pill_toggle"), "prev_action");
    } else if over(lay.rb) {
        v.fire = act(rail("pill_toggle"), "next_action");
    }
    v
}

// ── Geometry helpers ─────────────────────────────────────────────────────────

/// A numeric control's authored range — `min` (0) and `max` (1) off the node's own
/// props. Read by the pad-nudge channel, the stepper/slider echo default (`min`) and
/// commit-on-release, so all three agree on what "resting" means.
///
/// (The pill-toggle geometry doc that sat here belongs to [`pill_well`] /
/// [`pill_cell`], where that arithmetic now lives again.)
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
    // `label_bind` is the bound twin of `label`, exactly as `text_bind` is of
    // `text` — the pairs fall back in the same order on both sides, so a node
    // whose display copy is a LABEL can bind it without renaming the prop.
    //
    // It was listed in `WALKER_BINDS` (so the generic bind loop skips it, leaving
    // it to this function) but never read here, which made it a bind that resolved
    // to NOTHING: an authored `label_bind` drew an empty box, silently. Every
    // existing use funnelled through a template param into `text_bind` instead,
    // which is why nothing had caught it.
    let body = match ptext(node, "text_bind").or_else(|| ptext(node, "label_bind")) {
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

/// A prop that is PRESENT — the `props.k ~= nil` test, in the engine.
///
/// A tree authored in Lua cannot hold a `nil` field at all, so an author who writes
/// one means "authored nothing"; a null that survives the JSON marshal must read as
/// ABSENT here too, or a control would treat that nothing as a value.
fn jopt<'a>(v: &'a Json, key: &str) -> Option<&'a Json> {
    v.get(key).filter(|x| !x.is_null())
}

/// A prop's display TEXT, else `""` — the engine twin of the `child.label or ""` idiom.
///
/// Only a JSON *string* is text: a numeric `label` reads as EMPTY rather than being
/// coerced, exactly as [`draw_button`]'s label has since it moved to the engine tier.
/// Display copy is a string — and a number showing up where copy belongs is a bug in
/// the authored tree, not something to paper over.
fn jstr<'a>(v: &'a Json, key: &str) -> &'a str {
    v.get(key).and_then(|s| s.as_str()).unwrap_or_default()
}

/// A prop that is a JSON `true`, else false — the engine twin of a component's
/// `props.k == true` test.
///
/// STRICT on purpose, exactly as those `== true` comparisons were: only a real boolean
/// counts, so a stray number or string in a row's authored data never quietly turns it
/// inert (which, for a menu row's `disabled`, would be a click that goes nowhere with
/// nothing on screen to explain it).
fn jbool(v: &Json, key: &str) -> bool {
    v.get(key).and_then(|b| b.as_bool()).unwrap_or(false)
}

/// A control's option CHILDREN — the plain `{ value, label }` maps [`component_props`]
/// passes down for the children-as-data kinds ([`no_descend`]). Empty when it has none.
fn jkids(props: &Json) -> &[Json] {
    props.get("children").and_then(|k| k.as_array()).map_or(&[][..], |k| k.as_slice())
}

/// The pointer, for a component that lights a sub-region it laid out itself (a tab cell,
/// an option row). The walker injects `mx`/`my` every frame; the unreachable fallback
/// keeps a props map built without one (a unit test) from hovering the origin.
fn pointer(props: &Json) -> Vec2 {
    Vec2::new(jnum(props, "mx", f32::NEG_INFINITY), jnum(props, "my", f32::NEG_INFINITY))
}

/// The LUA type name of an authored prop — for a component's complaint about it. The
/// tree an author wrote is a Lua table however the engine happens to marshal it, so
/// the complaint names the type they can see in their own file, not the JSON it
/// arrived as.
fn lua_type(v: Option<&Json>) -> &'static str {
    match v {
        None | Some(Json::Null) => "nil",
        Some(Json::Bool(_)) => "boolean",
        Some(Json::Number(_)) => "number",
        Some(Json::String(_)) => "string",
        Some(Json::Array(_) | Json::Object(_)) => "table",
    }
}

/// Record a LOUD complaint about a node's authored DATA on the verdict, which
/// [`apply_hit_verdict`] surfaces as a `tracing::warn!`. The FIRST wins, so a
/// component says it once per node per hit.
///
/// This is the fail-loud path for data a component cannot act on (an option whose `value`
/// is the wrong TYPE). A control that instead shrugged would leave the author staring at
/// a strip that clicks to nothing, which is the difference between authorable and not.
fn warn_once(v: &mut HitVerdict, msg: String) {
    if v.warn.is_none() {
        v.warn = Some(msg);
    }
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
/// plain [`Value`], for [`component_props`] to hand a component (checkbox → bool,
/// slider → number, select → text).
fn eff_value<'a>(results: &'a ValueMap, model: &'a ValueMap, key: &str) -> Option<&'a Value> {
    results.get(key).or_else(|| model.get(key))
}

/// A [`Value`] as its natural JSON scalar, for marshalling a prop into a props map.
fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(n) => serde_json::json!(n),
        Value::Text(t) => Json::String(t.clone()),
    }
}

/// One flat tinted **rect** — the command-level sibling of [`push_panel`] /
/// [`push_text`].
///
/// The legacy primitive, and still the right one for a 1px rule or a flat band: a
/// [`HudCommand::Rect`] is the white texture tinted, with no radius, border or gradient
/// to pay for. A component reaching for a corner radius wants [`push_panel`] instead.
///
/// Emitted at layer 0 like every other component command — the walker lifts a node's
/// whole run onto its sub-layer afterwards, and a component that stacks WITHIN itself
/// (a select's popup over its field) lifts its own run with [`offset_layer`].
fn push_rect(out: &mut Vec<HudCommand>, r: Rect, color: [f32; 4]) {
    out.push(HudCommand::Rect { x: r.x, y: r.y, w: r.w, h: r.h, color, layer: 0.0 });
}

/// One rounded-rect SDF **panel** with explicit colours — the command-level sibling
/// of [`push_text`].
///
/// [`draw_panel_bg`] is the STYLE-BLOCK path: it reads a container's fill/border down
/// the key-alias chains and draws its backdrop. This is the path a component takes
/// once it has already PICKED its colours and is drawing a sub-box of its own rect —
/// a checkbox's box, a toggle's knob — where the alias chain belongs to the component,
/// not to the block.
///
/// `fill2` names the second gradient stop (`None` = solid) and `grad` follows the
/// emitter's own default: 0 when the two stops match, 1 (vertical) when they differ.
/// The edge draws at `border_w` px ONLY when `border` carries alpha — a transparent
/// border colour means NO edge, never an invisible hairline eating a pixel of fill.
fn push_panel(
    out: &mut Vec<HudCommand>,
    r: Rect,
    fill: [f32; 4],
    fill2: Option<[f32; 4]>,
    radius: f32,
    border: [f32; 4],
    border_w: f32,
) {
    let fill2 = fill2.unwrap_or(fill);
    out.push(HudCommand::Panel {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        color: fill,
        color2: fill2,
        grad: if fill == fill2 { 0.0 } else { 1.0 },
        radius,
        border: if border[3] > 0.0 { border_w } else { 0.0 },
        border_color: border,
        feather: 0.0,
        layer: 0.0,
    });
}

#[allow(clippy::too_many_arguments)]
fn push_text(out: &mut Vec<HudCommand>, x: f32, y: f32, text: &str, size: f32, color: [f32; 4], align: TextAlign, font: FontRole, italic: bool, bold: bool, tracking: f32, wrap: Option<f32>) {
    out.push(HudCommand::Text { x, y, text: text.to_string(), size, color, layer: 0.0, align, font, italic, bold, tracking, wrap });
}

// Neutral fallbacks (only used when a style path is missing — real colour comes
// from the resolved Prism tokens in `ui_theme.json`).
const INK: [f32; 4] = [0.871, 0.847, 0.788, 1.0];
const PANEL: [f32; 4] = [0.078, 0.09, 0.122, 1.0];
const RUNE: [f32; 4] = [0.435, 0.592, 1.0, 1.0];
const SAP: [f32; 4] = [0.141, 0.247, 0.471, 1.0];
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
/// Glyph-face resting tint — mirrors `$bronze`. The authored source is
/// `pad_glyphs.color` in `ui_theme.json`; this is only the missing-key floor.
const BRONZE: [f32; 4] = [0.722, 0.592, 0.353, 1.0];
/// The dim half of the bronze pair — mirrors `$bronze_dim`; a medallion ring's bottom
/// gradient stop when its variant block names no `bot`.
const BRONZE_DIM: [f32; 4] = [0.431, 0.353, 0.204, 1.0];
/// Activate-flash lit colour — mirrors `$rune_glow_hi` (`pad_glyphs.flash`).
const FLASH_LIT: [f32; 4] = [0.616, 0.722, 1.0, 1.0];
/// Dimmed ink — secondary/meta copy and the bottom rune pair. Mirrors `$ink_dim`.
const DIM: [f32; 4] = [0.561, 0.541, 0.49, 1.0];
/// The sunken stone a switch rests in — a toggle's OFF track. Mirrors `$stone1`
/// (`settings.controls.toggle.off_bg` is the authored source; this is the floor).
const STONE: [f32; 4] = [0.055, 0.063, 0.086, 1.0];
/// The sunk-well floor — mirrors `$well`. A condition gauge's track, a resource bar's
/// channel and an action slot's recess floor all bottom out here when their block names
/// no `track` / `bg_bot`.
const WELL: [f32; 4] = [0.039, 0.047, 0.063, 1.0];
/// Raised button stone — mirrors `$stone_btn`; an action slot's lit top stop when its
/// block names no `bg_top`. (Distinct from [`STONE`], the darker `$stone1` a switch
/// rests in — the two are different tokens, not a rounding of each other.)
const STONE_BTN: [f32; 4] = [0.125, 0.141, 0.18, 1.0];
/// A condition gauge's habitable band — the translucent green zone. No `$token`
/// mirrors it; the authored `pocepochs.hab.gauge` block carries the literal.
const BAND: [f32; 4] = [0.184, 0.616, 0.357, 0.55];
/// A condition gauge's out-of-band marker — mirrors `$stam_hi`.
const MARKER: [f32; 4] = [0.941, 0.804, 0.416, 1.0];
/// The septisigil blue gem — mirrors `$sig_blue`; a stat dot's hue when its block
/// names none.
const SIG_BLUE: [f32; 4] = [0.176, 0.373, 0.69, 1.0];

#[cfg(test)]
mod tests {
    use super::*;

    /// DRIFT GATE (rule AEEF2A68): the button `variant` compiled defaults are a
    /// MIRROR of the theme tokens they were promoted from (the deleted
    /// `modal.buttons.variants.*` blocks). Read ui_theme.json and assert every stop
    /// still equals its token, so the mirror can never silently fork — move a token
    /// in the theme and this fails until the compiled default follows.
    #[test]
    fn button_variant_defaults_match_theme_tokens() {
        let theme: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../content/sensorium/resources/ui_theme.json"),
            )
            .expect("theme reads"),
        )
        .expect("theme parses");
        let tokens = theme.get("theme").and_then(|t| t.get("tokens")).expect("theme.tokens");
        let tok = |name: &str| -> [f32; 4] {
            let a = tokens
                .get(name)
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("token `{name}` missing"));
            std::array::from_fn(|i| a[i].as_f64().unwrap() as f32)
        };
        // (compiled default stop, the token it mirrors)
        let checks: &[([f32; 4], &str)] = &[
            (BTN_PRIMARY.idle.top, "sap_base"),
            (BTN_PRIMARY.idle.bot, "sap_base_lo"),
            (BTN_PRIMARY.idle.border, "sap_border"),
            (BTN_PRIMARY.idle.label, "ink_sapphire"),
            (BTN_PRIMARY.hover.top, "sap_hover"),
            (BTN_PRIMARY.hover.bot, "sap_hover_lo"),
            (BTN_PRIMARY.hover.border, "sap_hover_border"),
            (BTN_PRIMARY.press.top, "sap_press"),
            (BTN_PRIMARY.press.bot, "sap_press_lo"),
            (BTN_PRIMARY.press.border, "sap_press_border"),
            (BTN_PRIMARY.glow, "sap_glow"),
            (BTN_SECONDARY.idle.top, "stone_btn"),
            (BTN_SECONDARY.idle.bot, "stone2"),
            (BTN_SECONDARY.idle.border, "edge4"),
            (BTN_SECONDARY.idle.label, "ink_button"),
            (BTN_SECONDARY.hover.top, "stone_btn_hi"),
            (BTN_SECONDARY.hover.bot, "surface_top"),
            (BTN_SECONDARY.hover.border, "bronze_dim"),
            (BTN_SECONDARY.hover.label, "ink_bright"),
            (BTN_SECONDARY.press.top, "stone2"),
            (BTN_SECONDARY.press.bot, "stone1"),
            (BTN_SECONDARY.press.border, "edge2"),
            (BTN_DANGER.idle.top, "danger_base"),
            (BTN_DANGER.idle.bot, "danger_base_lo"),
            (BTN_DANGER.idle.border, "danger_hi"),
            (BTN_DANGER.idle.label, "danger_text_hi"),
            (BTN_DANGER.hover.top, "danger_hover"),
            (BTN_DANGER.hover.bot, "danger_hover_lo"),
            (BTN_DANGER.press.top, "danger_base_lo"),
            (BTN_DANGER.press.bot, "danger_press_lo"),
            (BTN_DANGER.press.border, "danger_border"),
            (BTN_DANGER.glow, "danger_glow"),
            (BTN_GHOST.idle.top, "stage_void"),
            (BTN_GHOST.idle.label, "dim"),
            (BTN_GHOST.hover.border, "edge2"),
            (BTN_GHOST.hover.label, "ink"),
        ];
        for (got, name) in checks {
            assert_eq!(*got, tok(name), "button variant default drifted from token `{name}`");
        }
    }

    /// DRIFT GATE (rule AEEF2A68): `rune_corners`' compiled house defaults mirror the
    /// theme tokens the retired `settings.runes` block named. Read ui_theme.json and
    /// assert the corner colours still equal their tokens, so promoting the block into
    /// drawing code can't silently fork from the one palette.
    #[test]
    fn rune_corners_default_matches_theme_tokens() {
        let theme: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../content/sensorium/resources/ui_theme.json"),
            )
            .expect("theme reads"),
        )
        .expect("theme parses");
        let tok = |name: &str| -> [f32; 4] {
            let a = theme["theme"]["tokens"][name]
                .as_array()
                .unwrap_or_else(|| panic!("token `{name}` missing"));
            std::array::from_fn(|i| a[i].as_f64().unwrap() as f32)
        };
        assert_eq!(RUNE, tok("rune_glow"), "rune_corners top default mirrors rune_glow");
        assert_eq!(BRONZE_DIM, tok("bronze_dim"), "rune_corners bottom default mirrors bronze_dim");
    }

    /// The splash timeline, ported verbatim from the retired per-scene scripts:
    /// linear rise over fade_in, flat 1.0 through hold, linear fall over fade_out.
    #[test]
    fn splash_alpha_ramps_holds_and_falls() {
        let a = |t: f32| splash_alpha(t, 0.6, 1.2, 0.6);
        assert_eq!(a(0.0), 0.0);
        assert!((a(0.3) - 0.5).abs() < 1e-6, "halfway up the fade-in");
        assert_eq!(a(0.6), 1.0);
        assert_eq!(a(1.2), 1.0, "flat through the hold");
        assert!((a(2.1) - 0.5).abs() < 1e-6, "halfway down the fade-out");
        assert_eq!(a(2.4), 0.0);
        assert_eq!(a(9.0), 0.0, "clamped after the end");
        assert_eq!(splash_alpha(0.0, 0.0, 1.0, 0.5), 1.0, "zero fade-in shows instantly");
    }

    /// THE SPLASH ANIMATES THROUGH THE CACHE: its alpha is driven by
    /// `Model.elapsed` — a model read no `*_bind` prop declares — so the
    /// fingerprint must fold it by kind. Without that fold the first frame
    /// (alpha ≈ 0) replays forever: in-window, a splash that never fades in.
    /// The plateau still replays — equal alpha ⇒ byte-identical commands.
    #[test]
    fn a_splash_redraws_as_its_clock_advances() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard (see `hovering_redraws_only_the_hovered_node`).
        let _g = crate::strings::test_guard();
        let mut sp = node("splash");
        sp.id = "sp".into();
        sp.width = Some(200.0);
        sp.height = Some(100.0);
        sp.anchor = Some(UiAnchor::TopLeft);
        sp = prop(sp, "tex", Value::Number(1.0));
        sp = prop(sp, "fade_in", Value::Number(0.6));
        sp = prop(sp, "hold", Value::Number(1.2));
        sp = prop(sp, "fade_out", Value::Number(0.6));
        let input = input_at(-9.0, -9.0, false);
        let mut state = UiState::new();
        let mut at = |t: f64| {
            let model =
                ValueMap::new().with("elapsed", t).with("img_w", 100.0).with("img_h", 50.0);
            run_ui(&sp, &model, &styles(), &input, &mut state)
        };
        let alpha_of = |f: &UiFrame| {
            f.commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Sprite { color, .. } => Some(color[3]),
                    _ => None,
                })
                .expect("the splash drew its image")
        };

        let rise = at(0.3);
        assert!((alpha_of(&rise) - 0.5).abs() < 0.01, "mid-rise draws at half alpha");
        let top = at(0.6);
        assert_eq!(top.stats.redraw_nodes, 1, "the clock advancing invalidates the splash");
        assert!(alpha_of(&top) > 0.99, "the ramp completed");
        at(1.0);
        let plateau = at(1.5);
        assert_eq!(plateau.stats.redraw_nodes, 0, "the hold plateau replays from cache");
        assert_eq!(alpha_of(&plateau), 1.0);
        let faded = at(2.1);
        assert_eq!(faded.stats.redraw_nodes, 1, "the fade-out invalidates again");
        assert!((alpha_of(&faded) - 0.5).abs() < 0.01, "halfway down the fade-out");
    }
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

    /// A per-id ARRANGE bind (`<id>_off_x`/`_off_y`/`_anchor`) overrides a node's authored
    /// placement, so a scene's Lua `arrange()` — and a per-user layout override — can move a
    /// component the scene file centred. With no bind, the authored placement stands.
    #[test]
    fn an_arrange_bind_overrides_a_nodes_static_placement() {
        use super::{anchored, Rect};
        let mut hud = node("panel");
        hud.id = "hud".into();
        hud.width = Some(100.0);
        hud.height = Some(50.0);
        hud.anchor = Some(UiAnchor::TopLeft);
        hud.offset = [0.0, 0.0];
        let parent = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };

        // No bind → the authored top-left placement stands.
        let base = anchored(&hud, parent, &ValueMap::new());
        assert_eq!((base.x, base.y), (0.0, 0.0), "authored placement stands when no arrange bind");

        // arrange() moved it: a centre anchor + a (40, 25) offset override wins.
        let mut model = ValueMap::new();
        model.set("hud_anchor", "center");
        model.set("hud_off_x", 40.0);
        model.set("hud_off_y", 25.0);
        let moved = anchored(&hud, parent, &model);
        // center: x = (800-100)/2 + 40 = 390 ; y = (600-50)/2 + 25 = 300.
        assert_eq!(
            (moved.x, moved.y),
            (390.0, 300.0),
            "arrange anchor+offset binds override the authored placement"
        );
    }

    // ── Draw cache (draw on change) ──────────────────────────────────────────
    //
    // The ratified rule is that a node is redrawn only when one of its inputs changed;
    // every other node replays its cached commands. These tests hold the walker to it
    // through `UiStats::redraw_nodes`, and pin the replay byte-for-byte where the
    // commands themselves are the point.

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
        // `$token` display props resolve at the draw boundary — for the `text`
        // primitive AND for a component's own label prop — while a bound VALUE
        // (user data) passes through untouched.
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

        let f = run_ui(
            &page,
            &ValueMap::new(),
            &styles(),
            &input_at(-9.0, -9.0, false),
            &mut UiState::new(),
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
            "the button's label prop resolved too: {texts:?}"
        );
        assert!(!texts.iter().any(|t| t.contains('$')), "no raw sigil reached a command: {texts:?}");
    }

    #[test]
    fn a_still_frame_redraws_nothing() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // The blocking defect this cache exists to fix: the second frame of an
        // unchanged screen used to re-marshal and re-run every component's draw.
        let page = tree();
        let model = ValueMap::new().with("flag", true);
        let mut state = UiState::new();
        let go = |state: &mut UiState| {
            run_ui(&page, &model, &styles(), &input_at(-9.0, -9.0, false), state)
        };

        let first = go(&mut state);
        assert_eq!(first.stats.redraw_nodes, first.stats.nodes, "cold frame draws every node");

        let second = go(&mut state);
        assert_eq!(second.stats.redraw_nodes, 0, "an unchanged frame redraws nothing");
        assert_eq!(second.commands, first.commands, "the replay is byte-identical");
    }

    #[test]
    fn only_the_nodes_whose_inputs_changed_redraw() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // Flipping one bound value must not disturb its neighbours' cached commands.
        let page = tree();
        let mut state = UiState::new();
        let off = ValueMap::new().with("flag", false);
        let on = ValueMap::new().with("flag", true);
        let input = input_at(-9.0, -9.0, false);

        run_ui(&page, &off, &styles(), &input, &mut state);
        let flipped = run_ui(&page, &on, &styles(), &input, &mut state);
        assert_eq!(flipped.stats.redraw_nodes, 1, "only the checkbox reads `flag`");
    }

    // ── Hit + the draw cache together ────────────────────────────────────────
    //
    // The hit pass runs every frame for every placed node and the draw pass replays
    // whatever did not change, so a pointer parked on a control keeps its claim while
    // costing zero redraws. These hold that pair against real engine controls.

    #[test]
    fn an_idle_frame_redraws_nothing_and_keeps_hud_hit() {
        let _g = crate::strings::test_guard();
        // Pointer resting on the checkbox: the second, unchanged frame redraws nothing,
        // yet `hud_hit` stays claimed — the claim's continuity comes from RECOMPUTING
        // the verdict each frame, which an engine-tier arm does for free.
        let page = tree();
        let model = ValueMap::new().with("flag", true);
        let mut state = UiState::new();
        // (22,22) is inside the checkbox's 14×14 box at (16,16).
        let over = input_at(22.0, 22.0, false);

        let first = run_ui(&page, &model, &styles(), &over, &mut state);
        assert!(first.results.is_on("hud_hit"), "pointer over the box claims");

        let second = run_ui(&page, &model, &styles(), &over, &mut state);
        assert_eq!(second.stats.redraw_nodes, 0, "still frame: nothing redraws");
        assert!(second.results.is_on("hud_hit"), "the claim survives the idle frame");
    }

    #[test]
    fn a_verdict_routes_its_value_for_the_pointed_at_node_only() {
        // The generic verdict plumbing (`apply_hit_verdict`), held over two stacked
        // checkboxes: the pointer sits on the FIRST. Hovering its BOX claims, a click
        // routes the verdict's `value` into that node's bind — and the sibling under
        // neither the pointer nor the click says nothing but its idle echo.
        let mk = |id: &str, bind: &str| {
            let mut n = node("checkbox");
            n.id = id.into();
            n.size = Some(20.0);
            n.bind = Some(bind.into());
            prop(n, "box", Value::Number(14.0))
        };
        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.width = Some(120.0);
        col.children = vec![mk("p1", "b1"), mk("p2", "b2")];
        let mut page = node("screen");
        page.children = vec![col];
        let model = ValueMap::new();
        let mut state = UiState::new();
        let styles = serde_json::json!({});

        // Move frame: the pointer lands inside p1's 14×14 box (rows are y 0..20 and
        // 20..40, each box pinned to its row's top-left).
        let f = run_ui(&page, &model, &styles, &input_at(6.0, 6.0, false), &mut state);
        assert!(f.results.is_on("hud_hit"), "the box claimed the pointer");

        // Rest frame: the claim is recomputed, not remembered, so it survives.
        let f = run_ui(&page, &model, &styles, &input_at(6.0, 6.0, false), &mut state);
        assert!(f.results.is_on("hud_hit"));

        // Click frame: the verdict's value routes into the bind — for that node alone.
        let f = run_ui(&page, &model, &styles, &input_at(6.0, 6.0, true), &mut state);
        assert!(f.results.is_on("b1"), "verdict value wrote the clicked node's bind");
        assert!(!f.results.is_on("b2"), "the un-clicked sibling only echoes its rest value");

        // A click on the caption row BESIDE the box is inert — the tight region is the
        // box, not the row, so nothing claims and nothing writes.
        let mut fresh = UiState::new();
        let f = run_ui(&page, &model, &styles, &input_at(90.0, 6.0, true), &mut fresh);
        assert!(!f.results.is_on("hud_hit"), "the caption row is not a target");
        assert!(!f.results.is_on("b1"), "…and it writes nothing");
    }

    /// `label_bind` resolves like `text_bind` — the bound twin of `label`.
    ///
    /// It was declared in `WALKER_BINDS` (so the generic bind loop skips it) but
    /// never read by `node_text`, which made it a bind that resolved to NOTHING:
    /// an authored `label_bind` drew an empty box, silently, and no gate saw it
    /// because an empty string trips neither the token check nor the raw-literal
    /// check. Every prior use funnelled through a template param into `text_bind`,
    /// which is why it survived unnoticed.
    #[test]
    fn label_bind_resolves_like_text_bind() {
        let model = ValueMap::new().with("pill", "Worley").with("cap", "Granite");

        let bound = prop(node("button"), "label_bind", Value::Text("pill".into()));
        assert_eq!(node_text(&bound, &model, &ValueMap::new()), "Worley");

        // A literal `label` still wins when there is no bind, and the bind wins
        // when both are present — the same precedence `text`/`text_bind` has.
        let literal = prop(node("button"), "label", Value::Text("Fallback".into()));
        assert_eq!(node_text(&literal, &model, &ValueMap::new()), "Fallback");
        let both = prop(
            prop(node("button"), "label", Value::Text("Fallback".into())),
            "label_bind",
            Value::Text("cap".into()),
        );
        assert_eq!(node_text(&both, &model, &ValueMap::new()), "Granite");

        // `text_bind` still takes precedence, so nothing that worked before moves.
        let pair = prop(
            prop(node("text"), "text_bind", Value::Text("cap".into())),
            "label_bind",
            Value::Text("pill".into()),
        );
        assert_eq!(node_text(&pair, &model, &ValueMap::new()), "Granite");

        // A bind naming a key the Model does not publish is empty, not a panic.
        let missing = prop(node("button"), "label_bind", Value::Text("absent".into()));
        assert_eq!(node_text(&missing, &model, &ValueMap::new()), "");
    }

    #[test]
    fn a_focus_verdict_claims_state_focus_and_needs_an_id() {
        // Two stacked `text_field`s — the engine control whose verdict claims KEYBOARD
        // focus: one carries an id, the other only a bind. Focus is held BY id, so the
        // id-less one cannot take it.
        let mut named = node("text_field");
        named.id = "f1".into();
        named.size = Some(20.0);
        let mut anon = node("text_field");
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
        let f = run_ui(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, true), &mut state);
        assert!(f.results.is_on("hud_hit"));
        assert_eq!(state.focused(), Some("f1"), "focus=true set state.focus to the node id");

        // A non-click frame leaves focus alone.
        run_ui(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, false), &mut state);
        assert_eq!(state.focused(), Some("f1"), "focus persists across idle frames");

        // Clicking the ID-LESS node: the clicked frame clears focus up front, and an
        // empty-id claim is a no-op — nothing re-establishes it.
        run_ui(&page, &model, &serde_json::json!({}), &input_at(60.0, 26.0, true), &mut state);
        assert_eq!(state.focused(), None, "an id-less node cannot hold focus");

        // Re-claim, then click empty space: the generic click-away rule clears.
        run_ui(&page, &model, &serde_json::json!({}), &input_at(60.0, 6.0, true), &mut state);
        assert_eq!(state.focused(), Some("f1"));
        run_ui(&page, &model, &serde_json::json!({}), &input_at(400.0, 300.0, true), &mut state);
        assert_eq!(state.focused(), None, "clicking away clears focus generically");
    }

    /// The **generic full-rect claim** ([`HitShape::Rect`]) — held over a `tile`, the
    /// engine control that declares it and carries both channels: hovering claims, a
    /// click inside fires the node's `action` AND toggles its bool `bind`. (Capture and
    /// commit-on-release, the other generic channels, are held over a real `slider` in
    /// `slider_drag_captures_and_commits_on_release`.)
    #[test]
    fn a_rect_hit_shape_claims_fires_and_toggles() {
        let mut c = node("tile");
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

        let f = run_ui(&page, &model, &serde_json::json!({}), &input_at(25.0, 10.0, true), &mut state);
        assert!(f.results.is_on("hud_hit"), "the rect claims the pointer");
        assert!(f.results.is_on("poke"), "click inside fires the action");
        assert!(f.results.is_on("lit"), "click inside toggles the bool bind");

        // Outside: no claim, no fire.
        let f = run_ui(&page, &model, &serde_json::json!({}), &input_at(200.0, 200.0, true), &mut state);
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
        let page = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();

        run_ui(&page, &model, &styles(), &input_at(-9.0, -9.0, false), &mut state);
        // (20, 45) is inside the button: the column sits at 16,16 with a 20px checkbox
        // above a 24px button.
        let hover = run_ui(&page, &model, &styles(), &input_at(20.0, 45.0, false), &mut state);
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
        let model = ValueMap::new().with("flag", true);
        let input = input_at(-9.0, -9.0, false);
        let mut state = UiState::new();

        let first = run_ui(&tree(), &model, &styles(), &input, &mut state);
        let rebuilt = run_ui(&tree(), &model, &styles(), &input, &mut state);
        assert_eq!(rebuilt.stats.redraw_nodes, 0, "an equal tree rebuilt from scratch replays");
        assert_eq!(rebuilt.commands, first.commands, "…and the replay is byte-identical");
    }

    #[test]
    fn restyling_redraws_the_nodes_that_use_the_changed_block() {
        // Redraw counts fold `strings::generation()` into every fingerprint, so hold
        // the stringtable guard: a concurrent test's `load_str` mid-test would bump the
        // generation and force spurious redraws (an order-dependent flake).
        let _g = crate::strings::test_guard();
        // Cached commands carry RESOLVED colours, so a hot-reloaded `ui_theme.json`
        // must invalidate them — while an equal tree rebuilt at a new address must not
        // (the fingerprint folds block CONTENT, never its address).
        let page = tree();
        let model = ValueMap::new().with("flag", true);
        let input = input_at(-9.0, -9.0, false);
        let mut state = UiState::new();

        run_ui(&page, &model, &styles(), &input, &mut state);
        let same = run_ui(&page, &model, &styles(), &input, &mut state);
        assert_eq!(same.stats.redraw_nodes, 0, "an equal styles tree still replays");

        let mut restyled = styles();
        restyled["btn"]["fill_top"] = serde_json::json!([1.0, 0.0, 0.0, 1.0]);
        let reloaded = run_ui(&page, &model, &restyled, &input, &mut state);
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

        run_ui(&seg("ONE"), &model, &styles, &input, &mut state);
        let same = run_ui(&seg("ONE"), &model, &styles, &input, &mut state);
        assert_eq!(same.stats.redraw_nodes, 0, "an unchanged strip replays");
        let renamed = run_ui(&seg("TWO"), &model, &styles, &input, &mut state);
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

    // The bar's neutral fallbacks (track / thumb when the style block carries neither)
    // are `super::{STONE, SAP}`, in scope through the `use super::*` above. The
    // test-local COPIES that stood here while the bar lived in `ui/list.lua` were
    // deleted with the port: shadowing the real consts would have made these pins
    // agree with themselves rather than with the component.

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
    fn list_bar_width_inset_and_grab_floor_are_authorable() {
        // The bar's three metrics are style keys whose DEFAULTS are the values the
        // control shipped with (4 / 0 / 28) — so authoring them changes the bar and
        // authoring nothing changes nothing (the sibling tests pin the defaults).
        // 256×128 viewport, 64 rows of 64 → content 4096: the proportional thumb is 4px,
        // well under any floor, so `thumb_min` is what sizes it.
        let styles = serde_json::json!({
            "well": { "bar_w": 10, "bar_inset": 6, "thumb_min": 40,
                      "track": [0.25, 0.5, 0.75, 1.0], "thumb": [1.0, 0.5, 0.25, 1.0] }
        });
        let page = scroll_fixture(256.0, 128.0, 64, 64.0, Some("well"));
        let f = run_ui(
            &page,
            &ValueMap::new().with("sy", 0.0),
            &styles,
            &input_at(-9.0, -9.0, false),
            &mut UiState::new(),
        );
        let bars = bar_rects(&f.commands);
        // x = inner right (256) − bar_w (10) − bar_inset (6) = 240.
        assert_eq!(bars[0], (240.0, 0.0, 10.0, 128.0, [0.25, 0.5, 0.75, 1.0]), "authored track");
        assert_eq!(bars[1], (240.0, 0.0, 10.0, 40.0, [1.0, 0.5, 0.25, 1.0]), "authored grab floor");
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

    // (This pin is the SAME command list the Lua module emitted before the draw came
    // back to the engine in the 2026-08-09 restoration — it passed unchanged across the
    // move, which is the proof that a control's tier is invisible to what it draws.)
    #[test]
    fn list_draw_is_byte_pinned() {
        // Both draw branches at the byte level: a
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
    fn a_wheel_tick_scrolls_under_a_parked_pointer_without_a_crossing() {
        let _g = crate::strings::test_guard();
        // A wheel tick with a motionless pointer must still scroll the list under the
        // cursor, while a wheel-less still frame redraws nothing at all.
        let page = scroll_fixture(256.0, 128.0, 4, 64.0, None);
        let m = ValueMap::new().with("sy", 0.0);
        let styles = serde_json::json!({});
        let mut state = UiState::new();
        let over = input_at(100.0, 60.0, false);

        run_ui(&page, &m, &styles, &over, &mut state);
        let still = run_ui(&page, &m, &styles, &over, &mut state);
        assert_eq!(still.stats.redraw_nodes, 0, "a wheel-less still frame redraws nothing");
        assert!(still.results.is_on("hud_hit"), "the claim survives the still frame");

        // Wheel tick, pointer unmoved: one notch scrolls the bind by the default 46px
        // speed, and the moved thumb redraws the region.
        let f = run_ui(&page, &m, &styles, &input_wheel(100.0, 60.0, -1.0), &mut state);
        assert_eq!(f.results.number("sy"), Some(46.0), "one notch × the default speed");
        assert!(f.results.is_on("hud_hit"));
        assert!(f.stats.redraw_nodes >= 1, "the scrolled region redrew: {:?}", f.stats);

        // A wheel tick with the pointer OFF the region scrolls nothing.
        let mut fresh = UiState::new();
        let f = run_ui(&page, &m, &styles, &input_wheel(400.0, 300.0, -1.0), &mut fresh);
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
        let t = tree();
        let model = ValueMap::new().with("flag", false);
        let mut state = UiState::new();
        // Column at (16,16) width 120: checkbox rows y 16..36, button y 36..60.
        let frame =
            run_ui(&t, &model, &styles(), &input_at(50.0, 48.0, true), &mut state);
        assert!(frame.results.is_on("go"), "button action fired");
        assert!(frame.results.is_on("hud_hit"), "pointer over UI claims the mouse");
        assert!(!frame.commands.is_empty(), "something was drawn");
        assert!(!frame.results.is_on("flag"), "checkbox untouched by a button click");
    }

    /// THE ENGINE ROSTER GATE — held against EVERY control the engine claims, so a new
    /// one cannot land half-wired.
    ///
    /// The button slice discovered that a control's kind gated three separate walker
    /// decisions, and satisfying only the draw broke the others silently. One of the
    /// three — "it has LEFT `UI_COMPONENT_MODULES`" — retired with that list on
    /// 2026-08-10: there is no second tier to leave, so it is now true by construction
    /// rather than by assertion. The two that still bite:
    /// 1. it is a LEGAL kind, so every authored tree and proto can name it;
    /// 2. it answers the HIT in Rust — either by declaring a trivial geometry in
    ///    [`rust_hit_shape`] or by owning a bespoke arm ([`rust_owns_hit`]) — without
    ///    which the walker stops treating it as interactive at all (no hover, no
    ///    click, no action). The two are mutually exclusive: a control with a tight
    ///    sub-rect region must NOT declare `Rect`, which would widen it to the node.
    #[test]
    fn every_engine_component_is_legal_and_answers_its_hit_in_rust() {
        for kind in crate::rust_component_kinds() {
            assert!(crate::is_known_kind(kind), "{kind} must be a legal authored kind");
            assert!(
                rust_hit_shape(kind).is_some() || rust_owns_hit(kind),
                "{kind} must answer its hit in Rust — a declared shape or a bespoke \
                 arm — or it stops being interactive"
            );
            assert!(
                !(rust_hit_shape(kind).is_some() && rust_owns_hit(kind)),
                "{kind} answers its hit twice; a bespoke region must not also declare \
                 a trivial shape, which would widen it to the whole node rect"
            );
        }
    }

    /// A `panel` wears the walker's focus as a STYLE, and nothing else: the engine decides
    /// which pane holds the cursor, the panel decides what that looks like. A scene never
    /// passes a rim (violation F2) — so the two states must differ HERE, in the component,
    /// or that rim has nowhere legitimate to come from.
    #[test]
    fn panel_draws_its_own_focus_rim_from_the_walker_prop() {
        let style = serde_json::json!({
            "resting": { "fill": [0.0, 0.0, 0.0, 1.0], "border": [0.0, 0.0, 0.0, 0.0] },
            "focused": { "fill": [0.0, 0.0, 0.0, 1.0], "border": [1.0, 1.0, 1.0, 1.0] },
        });
        let border_of = |focused: bool| {
            let props = serde_json::json!({ "style": style, "focused": focused });
            let mut out = Vec::new();
            draw_panel(Rect { x: 0.0, y: 0.0, w: 40.0, h: 20.0 }, &props, &mut out);
            match out.first().expect("a panel drew its backdrop") {
                HudCommand::Panel { border, border_color, .. } => (*border, *border_color),
                other => panic!("expected a Panel command, got {other:?}"),
            }
        };
        let (resting_w, _) = border_of(false);
        let (focused_w, focused_color) = border_of(true);
        assert_eq!(resting_w, 0.0, "a resting pane wears no rim");
        assert!(focused_w > 0.0, "a focused pane wears one");
        assert_eq!(focused_color, [1.0, 1.0, 1.0, 1.0], "and it is the focused block's edge");

        // A style with NO resting/focused split is used as-is for both states, so a plain
        // container style still draws rather than silently rendering nothing.
        let plain = serde_json::json!({ "style": { "fill": [0.2, 0.2, 0.2, 1.0] } });
        let mut out = Vec::new();
        draw_panel(Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }, &plain, &mut out);
        assert_eq!(out.len(), 1, "an unsplit panel style still draws its backdrop");
    }

    /// `button` keeps its BEHAVIOUR now that the engine owns its draw and hit — the
    /// action fires, the pointer is claimed, and the slab is really drawn.
    #[test]
    fn button_draws_in_rust() {
        // A button-ONLY tree, so nothing else's commands can stand in for the
        // button's own draw.
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
        col.children = vec![btn];
        let mut t = node("screen");
        t.children = vec![col];

        let model = ValueMap::new();
        let mut state = UiState::new();
        // Column at (16,16) width 120 → the button occupies y 16..40.
        let frame =
            run_ui(&t, &model, &styles(), &input_at(50.0, 28.0, true), &mut state);

        assert!(frame.results.is_on("go"), "the Rust button still fires its action");
        assert!(frame.results.is_on("hud_hit"), "and still claims the pointer");
        assert!(!frame.commands.is_empty(), "and drew something");
        assert_eq!(frame.stats.redraw_nodes, frame.stats.nodes, "and the cold frame drew it all");
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

        // Click outside: no claim, no toggle — but the echo still reports. The scene
        // has folded the toggle above by now, so `on` is what BOTH the model and the
        // control hold; an outside click must leave it there rather than toggle again.
        let folded = ValueMap::new().with("sel", true);
        let f = run_ui(&page, &folded, &st, &input_at(300.0, 300.0, true), &mut state);
        assert!(!f.results.is_on("hud_hit"));
        assert!(f.results.is_on("sel"), "outside click leaves the bind alone");
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

        // Column at (16,16): row A circle 16..30 × 16..30, row B circle 16..30 ×
        // 36..50. Click inside row B's circle → the group selects "b".
        let frame = run_ui(
            &page,
            &model,
            &styles(),
            &input_at(22.0, 42.0, true),
            &mut state,
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
        // row echoes the group's current value, none overwrites it with its own. The
        // scene has folded the pick above, so that value is now "b".
        let picked = ValueMap::new().with("choice", "b");
        let frame = run_ui(
            &page,
            &picked,
            &styles(),
            &input_at(300.0, 300.0, false),
            &mut state,
        );
        assert_eq!(frame.results.text("choice"), Some("b"), "no-click frame echoes current selection");

        // The radio's TIGHT region is its circle: a click on row B's LABEL area
        // (inside the node rect, right of the 14×14 circle) neither selects nor
        // claims — while the group key still echoes.
        let frame = run_ui(
            &page,
            &model,
            &styles(),
            &input_at(70.0, 42.0, true),
            &mut state,
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

        // + button (right square): 5 → 6. Its own `UiState`: this is the other button
        // acting on the same resting 5, not a second press after the one above (which
        // the control still holds at 4 until the scene folds it).
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &st, &input_at(108.0, 12.0, true), &mut state);
        assert_eq!(frame.results.number("qty"), Some(6.0), "+ steps up by step");

        // No click (pointer between the buttons) → echoes the bound value.
        let mut state = UiState::new();
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
        let opt = |value: f64, label: &str| {
            let n = prop(node("option"), "value", Value::Number(value));
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
        pill.children = vec![opt(0.0, "Low"), opt(1.0, "Med"), opt(2.0, "High")];

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
        let model = ValueMap::new().with("mode", 0.0);
        let mut state = UiState::new();

        // Pill at (0,0) 180×30. Inner strip x 3..177 (174 wide) → 3 cells of 58:
        // low 3..61, med 61..119, high 119..177. Click the middle cell → index 1.
        let frame = run_ui(&page, &model, &styles, &input_at(90.0, 15.0, true), &mut state);
        assert_eq!(frame.results.number("mode"), Some(1.0), "middle segment selects its index");
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
        // The scene has folded the pick above, so the current selection is "med".
        let picked = ValueMap::new().with("mode", 1.0);
        let frame = run_ui(&page, &picked, &styles, &input_at(90.0, 15.0, false), &mut state);
        assert_eq!(frame.results.number("mode"), Some(1.0), "non-click frame reports current value");

        // A click OUTSIDE every cell leaves the selection untouched.
        let frame = run_ui(&page, &picked, &styles, &input_at(300.0, 15.0, true), &mut state);
        assert_eq!(frame.results.number("mode"), Some(1.0), "a miss doesn't change the value");

        // A click on the well's PAD RIM (x=1 < the 3px pad, inside the well) claims
        // the pointer but lands in no segment — the selection stays put.
        let frame = run_ui(&page, &model, &styles, &input_at(1.0, 15.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"), "the well rim still claims");
        assert_eq!(frame.results.number("mode"), Some(0.0), "rim click selects nothing");
    }

    #[test]
    fn tabs_click_selects_value_and_defaults_to_first() {
        // Three tabs (a|b|c) bound to "tab", a 300×30 strip at the origin: three
        // even 100px cells. Children are pure data carriers (value + label).
        let mk = |value: f64, label: &str| {
            let mut t = node("tab");
            t = prop(t, "value", Value::Number(value));
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
        tabs.children = vec![mk(0.0, "A"), mk(1.0, "B"), mk(2.0, "C")];
        let mut page = node("tabs_page");
        page.children = vec![tabs];

        let st = serde_json::json!({
            "ta": { "fill_top": [0.2,0.3,0.5,1.0], "label": [1.0,1.0,1.0,1.0] },
            "ti": { "fill_top": [0.09,0.10,0.13,1.0], "label": [0.56,0.54,0.49,1.0] }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Click the middle cell (x 100..200) → selects its index 1.
        let frame = run_ui(&page, &model, &st, &input_at(150.0, 15.0, true), &mut state);
        assert_eq!(frame.results.number("tab"), Some(1.0), "clicking tab 2 writes its index");
        assert!(frame.results.is_on("hud_hit"), "pointer over the strip claims the mouse");

        // No prior value + pointer off the strip → reports the first tab (a strip
        // always has one active tab), and claims nothing. Its own `UiState`: "no prior
        // value" means no prior PICK either, and the click above is one.
        let mut state = UiState::new();
        let frame = run_ui(&page, &model, &st, &input_at(400.0, 400.0, false), &mut state);
        assert_eq!(frame.results.number("tab"), Some(0.0), "unset bind defaults to the first tab");
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
        let mk = |value: f64, label: &str| {
            let t = node("tab");
            let t = prop(t, "value", Value::Number(value));
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
        tabs.children = vec![mk(0.0, "A"), mk(1.0, "B"), mk(2.0, "C")];
        let mut page = node("tabs_page");
        page.children = vec![tabs];
        let st = serde_json::json!({
            "ta": { "fill_top": [0.2,0.3,0.5,1.0] },
            "ti": { "fill_top": [0.09,0.10,0.13,1.0] }
        });
        let model = ValueMap::new().with("tab", 2.0);
        let mut state = UiState::new();

        // Click in the gap between cell 1 and cell 2 (x ≈ 106).
        let frame = run_ui(&page, &model, &st, &input_at(106.0, 15.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"), "the strip claims between cells");
        assert_eq!(frame.results.number("tab"), Some(2.0), "a gap click selects nothing");

        // Click inside cell 2 (x ≈ 160) → selects index 1.
        let frame = run_ui(&page, &model, &st, &input_at(160.0, 15.0, true), &mut state);
        assert_eq!(frame.results.number("tab"), Some(1.0), "cell click selects its index");
    }

    #[test]
    fn tabs_vertical_stacks_cells_top_to_bottom_and_click_picks_the_row() {
        // A `vertical` strip, 200 wide × 300 tall, three cells (a|b|c) bound to "cat":
        // stacked TOP-TO-BOTTOM, each spanning the full 200 width, 100px tall.
        let mk = |value: f64, label: &str| {
            let t = node("tab");
            let t = prop(t, "value", Value::Number(value));
            prop(t, "label", Value::Text(label.into()))
        };
        let mut tabs = node("tabs");
        tabs.id = "tabs".into();
        tabs.bind = Some("cat".into());
        tabs.width = Some(200.0);
        tabs.height = Some(300.0);
        tabs.anchor = Some(UiAnchor::TopLeft);
        tabs = prop(tabs, "vertical", Value::Bool(true));
        tabs = prop(tabs, "tab_active", Value::Text("ta".into()));
        tabs = prop(tabs, "tab_idle", Value::Text("ti".into()));
        tabs.children = vec![mk(0.0, "A"), mk(1.0, "B"), mk(2.0, "C")];
        let mut page = node("tabs_page");
        page.children = vec![tabs];
        let st = serde_json::json!({
            "ta": { "fill_top": [0.2,0.3,0.5,1.0] },
            "ti": { "fill_top": [0.09,0.10,0.13,1.0] }
        });
        let model = ValueMap::new();

        // The 2nd row (y 100..200) picks index 1 — the click the task calls out.
        let f = run_ui(&page, &model, &st, &input_at(100.0, 150.0, true), &mut UiState::new());
        assert_eq!(f.results.number("cat"), Some(1.0), "a click on the 2nd row picks index 1");
        assert!(f.results.is_on("hud_hit"), "the vertical strip claims the pointer");

        // Discriminators vs a HORIZONTAL strip: a 200-wide horizontal strip would split on
        // X (cells 0..66.7, 66.7..133.3, 133.3..200), so BOTH of these clicks would pick the
        // opposite index there — proving the cell axis really flipped to Y.
        let f = run_ui(&page, &model, &st, &input_at(180.0, 50.0, true), &mut UiState::new());
        assert_eq!(f.results.number("cat"), Some(0.0), "top row picks 0 (a horizontal strip picks 2 at x=180)");
        let f = run_ui(&page, &model, &st, &input_at(20.0, 250.0, true), &mut UiState::new());
        assert_eq!(f.results.number("cat"), Some(2.0), "bottom row picks 2 (a horizontal strip picks 0 at x=20)");

        // A drawn cell spans the FULL width (a horizontal cell would be ~66.7 wide).
        let full_w = f.commands.iter().any(|c| matches!(c, HudCommand::Panel { w, .. } if (*w - 200.0).abs() < 0.5));
        assert!(full_w, "vertical cells span the full strip width");
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
        let opt = |i: f64, label: &str| {
            let mut n = node("option");
            n = prop(n, "value", Value::Number(i));
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
        sel.children = vec![opt(0.0, "Alpha"), opt(1.0, "Beta")];
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

        // Closed: idle pointer far away. The field panel fills the node rect exactly
        // and is the ONLY panel (no popup rows drawn while closed).
        let f0 = run_ui(&t, &model, &styles, &input_at(400.0, 400.0, false), &mut state);
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
        let f1 = run_ui(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state);
        assert_eq!(state.open.as_deref(), Some("sel"), "clicking the field opens the menu");
        assert!(f1.results.is_on("hud_hit"), "the field claims the pointer");

        // Menu open (state persists). Rows start at y = 40 + 6 = 46, row_h 30:
        // row 0 = 46..76 (Alpha), row 1 = 76..106 (Beta). Click Beta.
        let f2 = run_ui(&t, &model, &styles, &input_at(100.0, 90.0, true), &mut state);
        assert_eq!(f2.results.number("mode"), Some(1.0), "clicking Beta writes its index");
        assert!(state.open.is_none(), "picking an option closes the menu");

        // A click outside a re-opened menu just closes it (writes nothing new).
        run_ui(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state); // re-open
        assert_eq!(state.open.as_deref(), Some("sel"));
        run_ui(&t, &model, &styles, &input_at(600.0, 500.0, true), &mut state); // click far outside
        assert!(state.open.is_none(), "a click outside closes the menu");
    }

    #[test]
    fn open_select_popup_outside_the_node_rect_still_claims_and_picks() {
        // The popup lies BELOW the select's own 200×40 rect (rows at y 46..106): a
        // naive rect pre-filter would drop it. Hovering a row must claim the pointer
        // (and keep claiming on an idle frame); clicking it must pick.
        //
        // `select`'s hit arm runs in `hit_node` for every placed node, with no
        // candidate pre-filter to escape — which is what lets the off-rect popup
        // rows answer at all.
        let t = select_tree();
        let styles = select_styles_json();
        let model = ValueMap::new();
        let mut state = UiState::new();

        // Open via a field click.
        run_ui(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state);
        assert_eq!(state.open.as_deref(), Some("sel"));

        // Hover row 1 (y=90 — outside the node rect): the popup claims. `hit_select`
        // reaches PAST the node rect, which is why the select is answered every frame
        // rather than only when a rect test would have let it through.
        let f = run_ui(&t, &model, &styles, &input_at(100.0, 90.0, false), &mut state);
        assert!(f.results.is_on("hud_hit"), "the open popup claims outside the node rect");

        // Idle frame (nothing moved): the claim persists.
        let f = run_ui(&t, &model, &styles, &input_at(100.0, 90.0, false), &mut state);
        assert!(f.results.is_on("hud_hit"), "the claim survives an idle frame");

        // Click the hovered row: Beta is picked and the menu closes.
        let f = run_ui(&t, &model, &styles, &input_at(100.0, 90.0, true), &mut state);
        assert_eq!(f.results.number("mode"), Some(1.0), "the row outside the rect picks");
        assert!(state.open.is_none(), "picking closes the menu");
    }

    #[test]
    fn select_open_menu_rows_are_lifted_above_the_field() {
        let t = select_tree();
        let styles = select_styles_json();
        let model = ValueMap::new().with("mode", 0.0);
        let mut state = UiState::new();
        // Force it open, then draw: the field is layer 0, the popup panel + rows layer 1.
        run_ui(&t, &model, &styles, &input_at(100.0, 20.0, true), &mut state);
        assert_eq!(state.open.as_deref(), Some("sel"));
        let frame = run_ui(&t, &model, &styles, &input_at(0.0, 0.0, false), &mut state);
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

        // The popup's TEXT is lifted with it. The render pass is ascending-layer
        // painter's order ACROSS pipelines, so a row label left behind on layer 0 would
        // be painted over by its own backdrop — an open dropdown showing selection bands
        // and no option text. A panel-only assertion would sail straight through that.
        let row_texts: Vec<(f32, &str)> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { layer, text, .. } => Some((*layer, text.as_str())),
                _ => None,
            })
            .collect();
        assert!(
            row_texts.iter().any(|(l, t)| *t == "Alpha" && *l == 1.0)
                && row_texts.iter().any(|(l, t)| *t == "Beta" && *l == 1.0),
            "every option label rides the popup's sub-layer: {row_texts:?}"
        );
        // …and the field's own label stays on the base layer beneath it.
        assert_eq!(row_texts.first().map(|(l, _)| *l), Some(0.0), "the field label is not lifted");
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

    /// **A rail click selects a NUMERIC value.** A roster-driven strip numbers
    /// its entries (value = index); highlighting already compared numbers, so a
    /// string-only click gate was a rail that showed the selection but could
    /// never change it — the silent dead channel the populous page/tab rails
    /// shipped with. Both strip kinds carry the same contract.
    #[test]
    fn a_click_on_a_strip_selects_a_numeric_value() {
        for kind in ["tabs", "pill_toggle"] {
            let mut strip = node(kind);
            strip.id = "rail".into();
            strip.bind = Some("pick".into());
            strip.width = Some(300.0);
            strip.height = Some(40.0);
            strip.anchor = Some(UiAnchor::TopLeft);
            let mut first = node("option");
            first = prop(first, "value", Value::Number(0.0));
            first = prop(first, "label", Value::Text("A".into()));
            let mut second = node("option");
            second = prop(second, "value", Value::Number(1.0));
            second = prop(second, "label", Value::Text("B".into()));
            strip.children = vec![first, second];
            let mut page = node("screen");
            page.children = vec![strip];

            let model = ValueMap::new().with("pick", 0.0);
            let styles = serde_json::json!({});
            // Idle: the echo reports the resting selection — that is the
            // contract the dispatcher's changed-value test leans on.
            let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
            assert_eq!(f.results.number("pick"), Some(0.0), "{kind}: idle echoes the resting value");
            // A click in the SECOND cell writes that entry's numeric value.
            let f = run_ui(&page, &model, &styles, &input_at(225.0, 20.0, true), &mut UiState::new());
            assert_eq!(f.results.number("pick"), Some(1.0), "{kind}: the click selects index 1");
        }
    }

    /// A 300×40 option strip of `kind` at the origin, bound to `pick`, whose two
    /// options carry the `value`s given — the fixture the strip-representation gates
    /// share.
    fn strip_page(kind: &str, values: [Value; 2]) -> UiNode {
        let [a, b] = values;
        let mut strip = node(kind);
        strip.id = "rail".into();
        strip.bind = Some("pick".into());
        strip.width = Some(300.0);
        strip.height = Some(40.0);
        strip.anchor = Some(UiAnchor::TopLeft);
        let mut first = prop(node("option"), "value", a);
        first = prop(first, "label", Value::Text("A".into()));
        let mut second = prop(node("option"), "value", b);
        second = prop(second, "label", Value::Text("B".into()));
        strip.children = vec![first, second];
        let mut page = node("screen");
        page.children = vec![strip];
        page
    }

    /// **The abandoned representation fails LOUD.** Which segment of an option strip
    /// is selected is an INDEX, and an index is a NUMBER — in the option's `value`, on
    /// the bind, in the model and in the verdict. A strip that also accepted a text
    /// `value` would make the fork its contract, so each of the three index strips
    /// (`tabs` / `pill_toggle` / `select`) refuses a non-number and says so through
    /// `warn_once` → `HitVerdict.warn`, instead of clicking to nothing.
    ///
    /// All three answer their hit in the engine, so the refusal is asserted against
    /// the Rust arm itself.
    ///
    /// (`radio` is the one NAME-keyed picker and is deliberately not in this list.)
    #[test]
    fn a_strip_option_with_a_non_numeric_value_warns_and_never_writes() {
        let styles = serde_json::json!({});
        type StripHit = fn(Vec2, Rect, &Json, bool) -> HitVerdict;
        // `select` picks from a POPUP, so it needs the extra frame that opens it; the
        // two strips pick straight from the cell under the pointer.
        for (kind, opens, hit) in [
            ("tabs", false, hit_tabs as StripHit),
            ("pill_toggle", false, hit_pill_toggle as StripHit),
            ("select", true, hit_select as StripHit),
        ] {
            let page = strip_page(kind, [Value::Number(0.0), Value::Text("b".into())]);
            let model = ValueMap::new().with("pick", 0.0);
            let mut state = UiState::new();
            // The pointer sits on the SECOND option: cell 2 of the strips (x 225),
            // popup row 2 of the select (y 40 + 6 + 45).
            let at = if opens { input_at(150.0, 91.0, true) } else { input_at(225.0, 20.0, true) };
            if opens {
                run_ui(&page, &model, &styles, &input_at(150.0, 20.0, true), &mut state);
            }
            let f = run_ui(&page, &model, &styles, &at, &mut state);
            assert_eq!(
                f.results.text("pick"),
                None,
                "{kind}: a text `value` is never written to the bind"
            );
            assert_eq!(
                f.results.number("pick"),
                Some(0.0),
                "{kind}: the bind still reports the resting index"
            );

            // …and the refusal is LOUD: the component's own complaint rides the verdict
            // (the walker turns it into a `tracing::warn!`).
            let props = serde_json::json!({
                "children": [
                    { "value": 0.0, "label": "A" },
                    { "value": "b", "label": "B" },
                ],
                "open": true,
                "bind_value": 0.0,
            });
            let v = hit(at.mouse, Rect { x: 0.0, y: 0.0, w: 300.0, h: 40.0 }, &props, true);
            assert_eq!(v.value, None, "{kind}: no value is written for a text option");
            assert!(v.warn.is_some(), "{kind}: the refusal warns — it never fails to nothing");
        }
    }

    /// **The knobs the strips gained on their way into the engine are REAL.** Each was
    /// a literal buried in the module the control replaced; each is now a style key
    /// whose DEFAULT is that same literal — so nothing moved on screen, and a caller who
    /// wants it moved now has somewhere to say so. Both halves are asserted: the default
    /// still draws the old picture, and the key actually moves it.
    #[test]
    fn promoted_strip_style_keys_are_real_knobs() {
        let opts = serde_json::json!([{ "value": 0.0, "label": "A" }, { "value": 1.0, "label": "B" }]);
        let panels = |cmds: &[HudCommand]| -> Vec<(f32, f32, f32, f32, f32, f32)> {
            cmds.iter()
                .filter_map(|c| match c {
                    HudCommand::Panel { x, y, w, h, radius, layer, .. } => {
                        Some((*x, *y, *w, *h, *radius, *layer))
                    }
                    _ => None,
                })
                .collect()
        };
        let rects = |cmds: &[HudCommand]| -> Vec<(f32, f32, f32, f32, f32)> {
            cmds.iter()
                .filter_map(|c| match c {
                    HudCommand::Rect { x, y, w, h, layer, .. } => Some((*x, *y, *w, *h, *layer)),
                    _ => None,
                })
                .collect()
        };

        // ── pill_toggle: where the floating highlight sits inside its cell ──
        // A 180×30 well, pad 3 → two 87px cells from x 3.
        let pill_rect = Rect { x: 0.0, y: 0.0, w: 180.0, h: 30.0 };
        let pill = |style: Json| {
            let mut out = Vec::new();
            let props =
                serde_json::json!({ "style": style, "children": opts.clone(), "bind_value": 0.0 });
            draw_pill_toggle(pill_rect, &props, &mut out);
            out
        };
        let base = serde_json::json!({ "pad": 3, "h": 30, "radius": 15, "active_top": [0.1,0.2,0.3,1.0] });
        assert_eq!(
            panels(&pill(base))[1],
            (4.0, 3.0, 85.0, 24.0, 14.0, 0.0),
            "default highlight: 1px in, radius one under the well's"
        );
        let tuned = serde_json::json!({
            "pad": 3, "h": 30, "radius": 15, "active_top": [0.1,0.2,0.3,1.0],
            "active_inset": 5, "active_radius": 2
        });
        assert_eq!(
            panels(&pill(tuned))[1],
            (8.0, 3.0, 77.0, 24.0, 2.0, 0.0),
            "`active_inset` / `active_radius` move it"
        );

        // ── tabs: how far the underline rule runs inside its cell ──
        let strip = Rect { x: 0.0, y: 0.0, w: 200.0, h: 30.0 };
        let underline = |inset: Json| {
            let mut out = Vec::new();
            let props = serde_json::json!({
                "style": null,
                "tab_active": { "underline": [0.6,0.7,1.0,1.0], "underline_w": 3, "underline_inset": inset },
                "tab_idle": {},
                "children": [{ "value": 0.0, "label": "A" }],
                "bind_value": 0.0, "gap": 0.0, "pad_x": 0.0, "pad_y": 0.0
            });
            draw_tabs(strip, &props, &mut out);
            rects(&out)[0]
        };
        assert_eq!(underline(Json::Null), (0.0, 27.0, 200.0, 3.0, 0.0), "default: the full cell width");
        assert_eq!(underline(serde_json::json!(6)), (6.0, 27.0, 188.0, 3.0, 0.0), "`underline_inset` shortens it");

        // ── select: the popup's drop + sub-layer, the field's and rows' insets ──
        let mut sel = Vec::new();
        let props = serde_json::json!({
            "style": {
                "field": { "pad_x": 20, "caret_inset": 30, "caret_size": 12 },
                "menu": { "gap": 12, "lift": 3, "row_h": 30, "row_pad": 10, "pad_x": 25,
                          "sel_bg": [0.2,0.3,0.5,1.0] }
            },
            "children": opts.clone(), "bind_value": 0.0, "open": true, "placeholder": "…"
        });
        draw_select(Rect { x: 0.0, y: 0.0, w: 200.0, h: 40.0 }, &props, &mut sel);
        assert_eq!(
            panels(&sel)[1],
            (0.0, 52.0, 200.0, 60.0, 3.0, 3.0),
            "`menu.gap` drops the popup under the field and `menu.lift` raises it"
        );
        let all = rects(&sel);
        assert_eq!(all[0], (164.0, 19.0, 12.0, 1.0, 0.0), "`caret_inset` / `caret_size` place the caret");
        assert_eq!(
            all.last().copied(),
            Some((10.0, 52.0, 180.0, 30.0, 3.0)),
            "`menu.row_pad` insets the selected row's band, lifted with the popup"
        );
        let texts: Vec<(f32, f32)> = sel
            .iter()
            .filter_map(|c| match c {
                HudCommand::Text { x, layer, .. } => Some((*x, *layer)),
                _ => None,
            })
            .collect();
        assert_eq!(texts[0], (20.0, 0.0), "`field.pad_x` insets the field's own label");
        assert!(
            texts[1..].iter().all(|t| *t == (25.0, 3.0)),
            "`menu.pad_x` insets every row label, and each rides the popup's layer: {texts:?}"
        );
    }

    /// **A focused panel draws its OWN rim.** The real [`draw_panel`], the real
    /// `panel` style block: with the walker's focus on the pane the component
    /// reaches for the `focused` sub-block (the 2px sapphire border), and without
    /// it the `resting` one (the 1px edge). The CALLER passes no style string, no
    /// rim, no focus flag — a scene that computed a pane's rim is exactly the
    /// second focus system this component exists to end.
    #[test]
    fn a_focused_panel_draws_its_own_rim() {
        // A COMPONENT-BEHAVIOUR test owns its style fixture — the `panel` block is
        // scene content now (five-line split), not something shipped shared.
        let styles = serde_json::json!({
            "panel": {
                "resting": { "border_w": 1.0, "border": [0.3, 0.3, 0.32, 1.0] },
                "focused": { "border_w": 2.0, "border": [0.25, 0.45, 0.85, 1.0] },
                "entered": { "border_w": 2.0, "border": [0.55, 0.45, 0.85, 1.0] }
            }
        });
        let resting = styles["panel"]["resting"]["border_w"].as_f64().expect("panel.resting");
        let focus_w = styles["panel"]["focused"]["border_w"].as_f64().expect("panel.focused");
        assert_ne!(resting, focus_w, "the two states must be distinguishable at all");

        let mut pane = node("panel");
        pane.id = "pop_left".into();
        pane.tab_group = "pop_left".into();
        pane = prop(pane, "style", Value::Text("panel".into()));
        pane.width = Some(200.0);
        pane.height = Some(120.0);
        pane.anchor = Some(UiAnchor::TopLeft);
        assert!(
            !pane.props.keys().any(|k| k == "focused" || k.ends_with("_style")),
            "the caller passes no focus flag and no rim style — only the block name"
        );
        let mut page = node("screen");
        page.children = vec![pane];

        let border_of = |state: &mut UiState| {
            let f = run_ui(
                &page,
                &ValueMap::new(),
                &styles,
                &input_at(-9.0, -9.0, false),
                state,
            );
            f.commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Panel { border, .. } => Some(f64::from(*border)),
                    _ => None,
                })
                .expect("the panel drew its backdrop")
        };

        let mut idle = UiState::new();
        assert_eq!(border_of(&mut idle), resting, "an unfocused pane rests");
        let mut focused = UiState::new();
        focused.request_focus("pop_left");
        assert_eq!(border_of(&mut focused), focus_w, "the focused pane draws its own rim");
    }

    /// **A rail step advances the strip by one, and CLAMPS at the ends** — over the REAL
    /// (native) `paged_menu`. The control owns its stepping: the walker records the fired
    /// result name (`tab_next` — the same name the RB hint gutter fires and the shoulder
    /// signal carries), and the NEXT pass advances that strip's own bind by +1, clamped to
    /// its OWN children (next stops at the last, prev at the first — a linear rail never
    /// wraps; wrapping is an unexpected-UX anti-pattern, Aaron 2026-08-12). No scene owns a
    /// stepper, and no count is written down anywhere.
    #[test]
    fn a_rail_hint_press_steps_the_strip_by_one_and_clamps() {
        let styles = serde_json::json!({});
        // The native tree: page rail (`tabs`, id `pg`, 2 options) + tab rail
        // (`pill_toggle`, id `tb`, 3 options), each authored with its own step names.
        let screen = paged_menu_tree(false);

        // The authored tab rail carries its own step names — the SAME the hint gutter fires.
        let tabs = screen.children[0].children.iter().find(|n| n.id == "tb").expect("the tab rail");
        assert_eq!(tabs.props.get("next_action"), Some(&Value::Text("tab_next".into())));
        assert_eq!(tabs.props.get("prev_action"), Some(&Value::Text("tab_prev".into())));

        // Three tabs, resting on 0: `tab_next` walks 0 → 1 → 2 → 2 (CLAMPS at the last,
        // never wraps back to 0).
        let mut state = UiState::new();
        let mut model = ValueMap::new().with("page", 0.0).with("tab", 0.0).with("paged_tabs_shown", true);
        for want in [1.0, 2.0, 2.0] {
            state.push_step("tab_next");
            let f = run_ui(&screen, &model, &styles, &input_at(-9.0, -9.0, false), &mut state);
            assert_eq!(f.results.number("tab"), Some(want), "the strip steps itself, clamping at the last");
            model = model.with("tab", want); // the scene folded it back
        }
        // ...and `tab_prev` walks 2 → 1 → 0 → 0 (CLAMPS at the first, never wraps to the last).
        for want in [1.0, 0.0, 0.0] {
            state.push_step("tab_prev");
            let f = run_ui(&screen, &model, &styles, &input_at(-9.0, -9.0, false), &mut state);
            assert_eq!(f.results.number("tab"), Some(want), "prev clamps at the first");
            model = model.with("tab", want);
        }
        // The step is over each rail's OWN length: the 2-page rail's prev at 0 stays 0.
        state.push_step("page_prev");
        let f = run_ui(&screen, &model, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(f.results.number("page"), Some(0.0), "two pages: prev at 0 clamps to 0, never wraps to 1");
        // A name no strip claims steps nothing, and never accumulates.
        state.push_step("something_else");
        let f = run_ui(&screen, &model, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(f.results.number("tab"), Some(0.0), "an unclaimed name is inert");
        assert_eq!(f.results.number("page"), Some(0.0));
    }

    /// A MOUSE CLICK on a rail hint steps the strip — the click path feeds the
    /// strip-step channel exactly as the signal path does. Regression for the bug
    /// Aaron found in-window (2026-08-10): a hint FLASHED on click but never advanced
    /// the page, because only the signal path called `push_step` and a click set the
    /// result alone. A click is an activation like any other.
    #[test]
    fn a_mouse_click_on_a_rail_hint_steps_the_strip() {
        let styles = serde_json::json!({});
        let option = |i: usize| prop(node("option"), "value", Value::Number(i as f64));

        let mut hint = node("button");
        hint.action = Some("idx_next".into());
        hint.size = Some(24.0);

        let mut strip = node("tabs");
        strip.id = "strip".into();
        strip.bind = Some("idx".into());
        strip = prop(strip, "next_action", Value::Text("idx_next".into()));
        strip.children = vec![option(0), option(1), option(2)];
        strip.size = Some(24.0);

        let mut col = node("cell");
        col.anchor = Some(UiAnchor::TopLeft);
        col.offset = [16.0, 16.0];
        col.width = Some(200.0);
        col.children = vec![hint, strip];
        let mut screen = node("screen");
        screen.children = vec![col];

        let mut state = UiState::new();
        let model = ValueMap::new().with("idx", 0.0);
        // The hint is the first child: y 16..40. Click it.
        let f1 = run_ui(&screen, &model, &styles, &input_at(50.0, 28.0, true), &mut state);
        // The step channel is one-frame (like the signal path): fold f1's value and run
        // once more without a click so a same-frame OR next-frame step both land.
        let model = model.with("idx", f1.results.number("idx").unwrap_or(0.0));
        let f2 = run_ui(&screen, &model, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert_eq!(
            f2.results.number("idx"),
            Some(1.0),
            "a mouse click on the hint stepped the strip — a click is an activation"
        );
    }

    // ── Composites: popup_panel + paged_menu ─────────────────────────────────

    /// A `popup_panel` reserves its drawn title block at the top, then flows its
    /// authored ITEMS below — the items are ordinary child nodes, the title is chrome.
    /// The slab measures to `panel_w` × (title block + items + pad), so an anchored
    /// modal sizes to its content.
    #[test]
    fn popup_panel_reserves_the_title_block_then_flows_its_items() {
        let item = |id: &str| {
            let mut n = node("button");
            n.id = id.into();
            n.size = Some(40.0);
            n.action = Some(id.into());
            n
        };
        let mut pop = node("popup_panel");
        pop.id = "pop".into();
        pop.anchor = Some(UiAnchor::TopLeft);
        pop = prop(pop, "title", Value::Text("POP".into()));
        pop.children = vec![item("it0"), item("it1")];
        let mut screen = node("screen");
        screen.children = vec![pop];

        let f = run_ui(&screen, &ValueMap::new(), &serde_json::json!({}), &input_at(-9.0, -9.0, false), &mut UiState::new());
        let r = |id: &str| f.rect(id).unwrap_or_else(|| panic!("{id} placed"));
        // Defaults: panel_pad 38, panel_gap 16, title_size 52 → items_top = 38 + (52+10) + 16 = 116.
        let (pop_r, it0, it1) = (r("pop"), r("it0"), r("it1"));
        assert_eq!((pop_r.size.x, pop_r.size.y), (404.0, 246.0), "slab is panel_w × content height");
        assert_eq!((it0.pos.x, it0.pos.y), (38.0, 116.0), "first item flows under the title block");
        assert_eq!((it0.size.x, it1.pos.y), (328.0, 168.0), "items span the inner width, spaced by items_gap");
    }

    /// A `popup_panel` DRAWS its own chrome: the styled backdrop and the centred title.
    #[test]
    fn popup_panel_draws_its_backdrop_and_title() {
        let mut pop = node("popup_panel");
        pop.id = "pop".into();
        pop.anchor = Some(UiAnchor::TopLeft);
        pop = prop(pop, "title", Value::Text("PAUSED".into()));
        let mut screen = node("screen");
        screen.children = vec![pop];
        let styles = serde_json::json!({ "modal": { "panel": { "fill": [0.1, 0.1, 0.1, 1.0] } } });

        let f = run_ui(&screen, &ValueMap::new(), &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        let titled = f.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "PAUSED"));
        let backed = f.commands.iter().any(|c| matches!(c, HudCommand::Panel { .. }));
        assert!(titled, "the panel drew its title");
        assert!(backed, "the panel drew its backdrop");
    }



    /// A `paged_menu` whose two authored rails and a content child are present: the page
    /// rail (`tabs`) sits between the LT/RT hint gutters, the tab rail (`pill_toggle`) is
    /// centred below the 1px rule, and the content child GROWS to fill the rest.
    fn paged_menu_tree(hide_tabs: bool) -> UiNode {
        let opts = |n: usize| (0..n).map(|i| prop(node("option"), "value", Value::Number(i as f64))).collect::<Vec<_>>();
        let mut pages = node("tabs");
        pages.id = "pg".into();
        pages.bind = Some("page".into());
        pages = prop(pages, "next_action", Value::Text("page_next".into()));
        pages = prop(pages, "prev_action", Value::Text("page_prev".into()));
        pages.children = opts(2);
        let mut tabs = node("pill_toggle");
        tabs.id = "tb".into();
        tabs.bind = Some("tab".into());
        tabs.size = Some(520.0);
        tabs = prop(tabs, "next_action", Value::Text("tab_next".into()));
        tabs = prop(tabs, "prev_action", Value::Text("tab_prev".into()));
        tabs.children = opts(3);
        let mut content = node("cell");
        content.id = "ct".into();
        content.grow = Some(1.0);
        let mut pm = node("paged_menu");
        pm.id = "pm".into();
        pm.anchor = Some(UiAnchor::TopLeft);
        pm.width = Some(800.0);
        pm.height = Some(600.0);
        pm.pad = 40.0;
        pm = prop(pm, "style", Value::Text("paged_menu.frame".into()));
        if hide_tabs {
            pm = prop(pm, "hide_tabs", Value::Bool(true));
        }
        pm.children = vec![pages, tabs, content];
        let mut screen = node("screen");
        screen.children = vec![pm];
        screen
    }

    #[test]
    fn paged_menu_places_both_rails_and_grows_content() {
        let screen = paged_menu_tree(false);
        let model = ValueMap::new().with("page", 0.0).with("tab", 0.0).with("paged_tabs_shown", true);
        let f = run_ui(&screen, &model, &serde_json::json!({}), &input_at(-9.0, -9.0, false), &mut UiState::new());
        let r = |id: &str| f.rect(id).unwrap_or_else(|| panic!("{id} placed"));
        // inner = (40,40,720,520). Page band: hint_w 54, rail_gap 30, rail_h 42.
        let pg = r("pg");
        assert_eq!((pg.pos.x, pg.pos.y, pg.size.x, pg.size.y), (124.0, 40.0, 552.0, 42.0), "page rail between the LT/RT gutters");
        // Rule 1px at y 82; tab band centred: cluster = 2*46 + 2*20 + 520 = 652, cx = 40 + 34 = 74.
        let tb = r("tb");
        assert_eq!((tb.pos.x, tb.pos.y, tb.size.x, tb.size.y), (140.0, 83.0, 520.0, 44.0), "tab pill centred below the rule");
        // Content grows to fill below the tab band (y = 40+42+1+44 = 127).
        let ct = r("ct");
        assert_eq!((ct.pos.y, ct.size.y), (127.0, 433.0), "content grows to fill the rest");
    }

    #[test]
    fn paged_menu_hidden_tab_rail_collapses_and_content_reclaims_it() {
        let screen = paged_menu_tree(true);
        let model = ValueMap::new().with("page", 0.0).with("tab", 0.0).with("paged_tabs_shown", true);
        let f = run_ui(&screen, &model, &serde_json::json!({}), &input_at(-9.0, -9.0, false), &mut UiState::new());
        assert!(f.rect("tb").is_none(), "hide_tabs drops the pill rail entirely");
        // No rule, no tab band: content starts right under the page rail (y = 82).
        let ct = f.rect("ct").expect("content placed");
        assert_eq!((ct.pos.y, ct.size.y), (82.0, 478.0), "content reclaims the collapsed rail space");
    }

    #[test]
    fn paged_menu_draws_the_rule_and_four_glyph_hints() {
        let screen = paged_menu_tree(false);
        let model = ValueMap::new().with("page", 0.0).with("tab", 0.0).with("paged_tabs_shown", true);
        let styles = serde_json::json!({
            "paged_menu": { "frame": { "fill": [0.05, 0.05, 0.07, 1.0] }, "rule": { "color": [0.4, 0.3, 0.2, 1.0] } },
            "pad_glyphs": { "tex": 7, "cols": 4, "rows": 4, "cells": { "lt": 0, "rt": 1, "lb": 2, "rb": 3 } },
        });
        let f = run_ui(&screen, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        let sprites = f.commands.iter().filter(|c| matches!(c, HudCommand::Sprite { tex: 7, .. })).count();
        assert_eq!(sprites, 4, "all four rail-hint glyphs drew from the atlas");
        let rule = f.commands.iter().any(|c| matches!(c, HudCommand::Panel { h, y, .. } if (*h - 1.0).abs() < 0.01 && (*y - 82.0).abs() < 0.01));
        assert!(rule, "the 1px rule drew between the rails");
    }

    #[test]
    fn paged_menu_hint_gutter_click_fires_the_rails_step() {
        // Start on the LAST page (1 of 2) so a prev actually MOVES — a clamp at the
        // first would hide whether the gutter even fired.
        let model = ValueMap::new().with("page", 1.0).with("tab", 0.0).with("paged_tabs_shown", true);
        // LT gutter (x 40..94, y 40..82): fires the PAGE rail's prev_action → page 1 → 0.
        let f = run_ui(&paged_menu_tree(false), &model, &serde_json::json!({}), &input_at(60.0, 60.0, true), &mut UiState::new());
        assert!(f.results.is_on("hud_hit"), "the frame claims the click");
        assert_eq!(f.results.number("page"), Some(0.0), "LT stepped the page rail back");
        // RB gutter (x 680..726, y 83..127): fires the TAB rail's next_action → tab 0 → 1 (3 tabs).
        let f = run_ui(&paged_menu_tree(false), &model, &serde_json::json!({}), &input_at(700.0, 100.0, true), &mut UiState::new());
        assert_eq!(f.results.number("tab"), Some(1.0), "RB stepped the tab rail forward");
    }

    /// `page_side: "left"` stands the PAGE rail up as a fixed-width LEFT COLUMN (the authored
    /// `vertical` `tabs`), a 1px vertical rule divides it from the RIGHT area, and the tab
    /// rail + content take that right area — while the default (top) geometry is unchanged.
    #[test]
    fn paged_menu_left_side_stands_the_page_rail_in_a_left_column() {
        let opts = |n: usize| (0..n).map(|i| prop(node("option"), "value", Value::Number(i as f64))).collect::<Vec<_>>();
        let mut pages = node("tabs");
        pages.id = "pg".into();
        pages.bind = Some("page".into());
        pages = prop(pages, "vertical", Value::Bool(true));
        pages.children = opts(3);
        let mut tabs = node("pill_toggle");
        tabs.id = "tb".into();
        tabs.bind = Some("tab".into());
        tabs.size = Some(520.0);
        tabs.children = opts(3);
        let mut content = node("cell");
        content.id = "ct".into();
        content.grow = Some(1.0);
        let mut pm = node("paged_menu");
        pm.id = "pm".into();
        pm.anchor = Some(UiAnchor::TopLeft);
        pm.width = Some(800.0);
        pm.height = Some(600.0);
        pm.pad = 40.0;
        pm = prop(pm, "style", Value::Text("paged_menu.frame".into()));
        pm = prop(pm, "page_side", Value::Text("left".into()));
        pm.children = vec![pages, tabs, content];
        let mut screen = node("screen");
        screen.children = vec![pm];

        let styles = serde_json::json!({
            "paged_menu": { "frame": { "fill": [0.05,0.05,0.07,1.0] }, "rule": { "color": [0.4,0.3,0.2,1.0] } },
        });
        let model = ValueMap::new().with("page", 0.0).with("tab", 0.0).with("paged_tabs_shown", true);
        let f = run_ui(&screen, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        let r = |id: &str| f.rect(id).unwrap_or_else(|| panic!("{id} placed"));

        // inner = (40,40,720,520). page_w 200, page_gap 12 → left column, then a 1px rule at
        // x 240, then the right area at x = 40+200+12+1 = 253 (w 507).
        let pg = r("pg");
        assert_eq!((pg.pos.x, pg.pos.y, pg.size.x, pg.size.y), (40.0, 40.0, 200.0, 520.0), "page rail is the fixed-width left column");
        // Tab rail rides the top of the right area (cluster centred; pill 520 overflows the
        // 507 area so cx clamps to 253 → pill at 253+46+20 = 319), fully RIGHT of the rule.
        let tb = r("tb");
        assert_eq!((tb.pos.x, tb.pos.y, tb.size.x, tb.size.y), (319.0, 40.0, 520.0, 44.0), "tab rail rides the top of the right area");
        assert!(tb.pos.x > 240.0, "the tab rail sits right of the vertical rule");
        // Content fills the right area BELOW the tab band (y = 40+44 = 84).
        let ct = r("ct");
        assert_eq!((ct.pos.x, ct.pos.y, ct.size.x, ct.size.y), (253.0, 84.0, 507.0, 476.0), "content fills the right area below the tab band");
        // The divider is a 1px-WIDE, full-height vertical rule (top mode's is 1px-TALL).
        let vrule = f.commands.iter().any(|c| matches!(c, HudCommand::Panel { x, w, h, .. }
            if (*w - 1.0).abs() < 0.01 && (*h - 520.0).abs() < 0.01 && (*x - 240.0).abs() < 0.01));
        assert!(vrule, "a 1px vertical rule divides the left column from the right area");

        // The DEFAULT (top) path is byte-for-byte unchanged: the same kind with no
        // `page_side` lays the page rail horizontally between the LT/RT gutters, as before.
        let top = paged_menu_tree(false);
        let f2 = run_ui(&top, &model, &serde_json::json!({}), &input_at(-9.0, -9.0, false), &mut UiState::new());
        let pg2 = f2.rect("pg").expect("pg placed");
        assert_eq!((pg2.pos.x, pg2.pos.y, pg2.size.x, pg2.size.y), (124.0, 40.0, 552.0, 42.0), "page_side:top (default) geometry is unchanged");
    }

    /// **A strip selection is reported as a NUMBER.** The every-frame echo is the
    /// channel the selection travels on an idle frame, so it carries the same one
    /// representation the click does — never a stringified index.
    #[test]
    fn echo_binds_reports_a_strip_selection_as_a_number() {
        let styles = serde_json::json!({});
        for kind in ["tabs", "pill_toggle", "select"] {
            let page = strip_page(kind, [Value::Number(0.0), Value::Number(1.0)]);
            let model = ValueMap::new().with("pick", 1.0);
            let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
            assert_eq!(f.results.number("pick"), Some(1.0), "{kind}: the echo is a number");
            assert_eq!(f.results.text("pick"), None, "{kind}: the echo is never text");
        }
        // `radio` is the exception that proves the boundary: a row's literal NAME is
        // text, and its echo stays text.
        let mut radio = node("radio");
        radio.id = "r".into();
        radio.bind = Some("sec".into());
        radio.width = Some(200.0);
        radio.height = Some(30.0);
        radio.anchor = Some(UiAnchor::TopLeft);
        let mut page = node("screen");
        page.children = vec![radio];
        let model = ValueMap::new().with("sec", "sec_audio");
        let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        assert_eq!(f.results.text("sec"), Some("sec_audio"), "radio echoes the row NAME as text");
    }

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
        // input-active, so the hit pass runs for zero nodes — the fold is
        // unconditional — yet the value updates and the field (alone) redraws.
        //
        // `redraw_nodes` is the assertion: it proves the field re-rendered AND that
        // nothing else did, so neither a field that silently stopped drawing nor a
        // tree that redrew wholesale would pass.
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
        assert_eq!(f.stats.redraw_nodes, 1, "exactly the field redraws for the new value");
        assert!(f.results.is_on("hud_hit"), "the resting claim survives the typing frame");
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

    #[test]
    fn text_field_draw_is_byte_pinned() {
        // Both draw branches at the byte level, held across the field's two tier
        // moves (Rust → `ui/text_field.lua` in S6, back to `draw_text_field` under
        // ruling BF0AF0C9): a focused, valued field (well + label-coloured value +
        // measured caret) and an empty resting field (well + dim resolved
        // placeholder, no caret). It runs through `run_ui`, so it is the ENGINE arm
        // under test now, and the numbers below did not move.
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
        let f = run_ui(&b, &model, &styles(), &input_at(-9.0, -9.0, false), &mut state);
        let drew = f.commands.iter().any(
            |c| matches!(c, HudCommand::Text { text, .. } if text.contains("Skin")),
        );
        assert!(drew, "the bound caption reached the draw commands: {:?}", f.commands);

        // With no bind, the literal label still wins — existing buttons are unaffected.
        let plain = prop(node("button"), "label", Value::Text("GO".into()));
        let f = run_ui(
            &plain,
            &ValueMap::new(),
            &styles(),
            &input_at(-9.0, -9.0, false),
            &mut state,
        );
        assert!(f
            .commands
            .iter()
            .any(|c| matches!(c, HudCommand::Text { text, .. } if text == "GO")));
    }

    /// **The promoted knobs are live.** Restoring these four controls to the engine also
    /// gave each the parameters its module had hardcoded — a corner radius, an edge
    /// width, a caption gutter, a knob inset — every one defaulting to the literal it
    /// replaced, so nothing moved. A promoted key that no arm actually reads would be
    /// API surface that silently does nothing (the fail-quiet hole the authored-name law
    /// exists to close), so each is asserted here to change the picture.
    #[test]
    fn boolean_controls_honour_their_promoted_keys() {
        let r = Rect { x: 0.0, y: 0.0, w: 120.0, h: 20.0 };

        // checkbox — box radius + authored edge width, the tick's own radius, and the
        // caption's gutter and colour.
        let props = serde_json::json!({
            "style": { "box": [0.1,0.1,0.1,1.0], "radius": 4, "border": [1.0,0.0,0.0,1.0],
                       "border_w": 3, "check": [1.0,1.0,1.0,1.0], "check_radius": 2,
                       "label": [0.0,1.0,0.0,1.0], "pad": 3 },
            "box": 14, "label": "L", "label_gap": 20, "bind_value": true
        });
        let mut out = Vec::new();
        draw_checkbox(r, &props, &mut out);
        match &out[..] {
            [HudCommand::Panel { radius: box_r, border, border_color, .. }, HudCommand::Panel { radius: tick_r, .. }, HudCommand::Text { x, color, .. }] =>
            {
                assert_eq!(*box_r, 4.0, "the box takes the style radius");
                assert_eq!((*border, *border_color), (3.0, [1.0, 0.0, 0.0, 1.0]), "authored edge width");
                assert_eq!(*tick_r, 2.0, "the tick takes its own radius");
                assert_eq!(*x, 34.0, "the caption sits `box` + `label_gap` from the rect edge");
                assert_eq!(*color, [0.0, 1.0, 0.0, 1.0], "…in the style's caption colour");
            }
            other => panic!("checkbox drew {other:?}"),
        }

        // toggle — knob inset (which sizes AND places the knob), both radii, the edge
        // width, and the OFF track's second gradient stop.
        let props = serde_json::json!({
            "style": { "w": 50, "h": 25, "knob_pad": 5, "radius": 2, "knob_radius": 1,
                       "off_bg": [0.1,0.1,0.1,1.0], "off_bot": [0.0,0.0,1.0,1.0],
                       "off_border": [1.0,1.0,1.0,1.0], "border_w": 4 },
            "bind_value": false
        });
        let mut out = Vec::new();
        draw_toggle(r, &props, &mut out);
        match &out[..] {
            [HudCommand::Panel { radius: track_r, border, color2, grad, .. }, HudCommand::Panel { x, y, w, h, radius: knob_r, .. }] =>
            {
                assert_eq!((*track_r, *border), (2.0, 4.0), "track radius + authored edge width");
                assert_eq!((*color2, *grad), ([0.0, 0.0, 1.0, 1.0], 1.0), "`off_bot` ramps the OFF track");
                assert_eq!((*x, *w, *h), (5.0, 15.0, 15.0), "`knob_pad` insets and sizes the knob");
                assert_eq!(*y, r.y + (r.h - 25.0) * 0.5 + 5.0, "…on both axes");
                assert_eq!(*knob_r, 1.0, "the knob takes its own radius");
            }
            other => panic!("toggle drew {other:?}"),
        }

        // radio — a SQUARE radio: both round defaults are style-owned, not baked in.
        let props = serde_json::json!({
            "style": { "box": [0.1,0.1,0.1,1.0], "check": [1.0,1.0,1.0,1.0],
                       "radius": 0, "check_radius": 0 },
            "box": 14, "label": "", "value": "a", "bind_value": "a"
        });
        let mut out = Vec::new();
        draw_radio(r, &props, &mut out);
        match &out[..] {
            [HudCommand::Panel { radius: box_r, .. }, HudCommand::Panel { radius: dot_r, .. }] => {
                assert_eq!((*box_r, *dot_r), (0.0, 0.0), "a radio can be authored square");
            }
            other => panic!("radio drew {other:?}"),
        }

        // tile — a rounded, edged slot cell.
        let props = serde_json::json!({
            "style": { "cell": [0.2,0.2,0.2,1.0], "radius": 6,
                       "border": [1.0,1.0,0.0,1.0], "border_w": 2 },
            "enabled": true, "label": ""
        });
        let mut out = Vec::new();
        draw_tile(r, &props, &mut out);
        match &out[..] {
            [HudCommand::Panel { radius, border, border_color, .. }, HudCommand::Text { .. }] => {
                assert_eq!(*radius, 6.0, "the cell takes the style radius");
                assert_eq!((*border, *border_color), (2.0, [1.0, 1.0, 0.0, 1.0]), "and an authored edge");
            }
            other => panic!("tile drew {other:?}"),
        }
    }

    /// A checkbox always draws its box; the inset `check` tick appears ONLY when its
    /// bound value is true — drawn by the ENGINE arm (box = 1 panel, box + tick = 2).
    /// Also proves `bind_value` + the merged node props (`box`/`label`) still reach the
    /// component: the walker assembles ONE prop surface, and the restoration must not
    /// change what a control receives.
    #[test]
    fn checkbox_ticks_when_bound_true() {
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
            run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new())
                .commands
                .iter()
                .filter(|c| matches!(c, HudCommand::Panel { .. }))
                .count()
        };
        assert_eq!(panels(false), 1, "unchecked: just the box");
        assert_eq!(panels(true), 2, "checked: box + tick");

        // The row label (a merged node prop) reaches the component's draw.
        let model = ValueMap::new().with("flag", false);
        let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        assert!(f.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "Enable")));
    }

    /// A toggle's knob sits at the RIGHT of the pill when its bound value is true, at the
    /// LEFT when false — drawn by the ENGINE arm.
    #[test]
    fn toggle_knob_shifts_with_bound_value() {
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
            run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new())
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
    /// when it is not — drawn by the ENGINE arm (proves `enabled` + the resolved
    /// `style_off` block still reach the component).
    #[test]
    fn tile_swaps_style_when_not_loaded() {
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
            run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new())
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
    /// `value` equals the bound selection — proving the children-as-data list reaches the
    /// engine-tier draw, and that selection compares the value AS AUTHORED (this strip
    /// is driven with TEXT values, which the DRAW must still match even though the HIT
    /// narrows a written index to a number).
    #[test]
    fn pill_toggle_lights_the_selected_segment() {
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
            let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
            assert!(f.stats.redraw_nodes >= 1, "the pill really drew this frame");
            f.commands.iter().filter(|c| matches!(c, HudCommand::Panel { .. })).count()
        };
        assert_eq!(panels("none"), 1, "no active segment: just the well");
        assert_eq!(panels("run"), 2, "the selected segment adds a highlight panel");
    }

    /// A tab strip styles the cell whose child `value` == the bound selection from
    /// `tab_active`, the rest from `tab_idle` — proving the resolved per-state blocks
    /// and the children-as-data list both reach the engine-tier draw, and (like the pill
    /// above) that the draw's selection compare stays value-as-authored.
    #[test]
    fn tabs_style_the_selected_tab_active() {
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
        let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        assert!(f.stats.redraw_nodes >= 1, "the strip really drew this frame");
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
    /// through min/max) — drawn by the engine-tier arm.
    #[test]
    fn slider_fills_to_the_bound_value_in_rust() {
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
            let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
            assert!(f.stats.redraw_nodes >= 1, "the slider really drew this frame");
            let widths: Vec<f32> = f.commands.iter().filter_map(|c| match c {
                HudCommand::Rect { w, .. } => Some(*w),
                _ => None,
            }).collect();
            widths[1]
        };
        assert_eq!(fill_w(0.0), 0.0, "value 0 → empty fill");
        assert_eq!(fill_w(50.0), 50.0, "value 50 of 100 → half of the 100px track");
    }

    /// A stepper draws its field + two end buttons (3 rects) and the value formatted with
    /// `decimals`/`suffix` — in the engine tier.
    #[test]
    fn stepper_draws_field_buttons_and_formatted_value() {
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
        let f = run_ui(&page, &model, &styles, &input_at(-9.0, -9.0, false), &mut UiState::new());
        assert!(f.stats.redraw_nodes >= 1, "the stepper really drew this frame");
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

    /// The **commit-on-release** contract (Aaron, 2026-08-06): a captured drag
    /// tracks the hand visually, but `frame.results` keeps reporting the RESTING
    /// model value the whole drag — the one real write happens on the release
    /// frame. A scene folding results therefore sees exactly one update per drag.
    #[test]
    fn slider_drag_captures_and_commits_on_release() {
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
        // Track spans the full 200px width from x=0; press at the midpoint. The
        // press CAPTURES, but the emission is held back: results still echo the
        // resting model value.
        let frame = run_ui(&page, &model, &st, &input_at(100.0, 10.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"));
        assert!(state.dragging.contains("s"), "grab-band press captures");
        assert_eq!(frame.results.number("v"), Some(0.0), "mid-drag results stay at rest");

        // Still held, cursor moved right → the drag keeps TRACKING (even
        // off-track), but still emits nothing.
        let held = UiInput { mouse: Vec2::new(180.0, 10.0), clicked: false, down: true, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let frame = run_ui(&page, &model, &st, &held, &mut state);
        assert_eq!(frame.results.number("v"), Some(0.0), "still no emission while held");
        let live = state.drag_value.as_ref().expect("the drag holds its value in flight");
        let Value::Number(n) = live.1 else { panic!("a slider drags a number") };
        assert!(n > 80.0, "the in-flight value follows the hand: {n}");

        // Release: the ONE real write, at the last dragged position.
        let up = UiInput { mouse: Vec2::new(180.0, 10.0), clicked: false, down: false, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let frame = run_ui(&page, &model, &st, &up, &mut state);
        let v = frame.results.number("v").expect("release commits the bind");
        assert!(v > 80.0, "release commits the dragged value: {v}");
        assert!(state.drag_value.is_none(), "the in-flight value is spent");
        assert!(state.dragging.is_empty(), "capture released");
    }

    /// **A captured node keeps answering once the pointer LEAVES its rect**, until
    /// the button-up edge releases it. Dragging a slider and sliding off the row is
    /// the ordinary way to use one, and the hit pass is otherwise rect-gated — so
    /// without the capture short-circuit the value would freeze the instant the hand
    /// wandered, which is the bug this pins.
    ///
    /// The gate that used to hold this asserted the retired Lua crossing counter
    /// (`lua_hits == 1` for the off-rect node). It re-pins the same invariant through
    /// the BEHAVIOUR instead: the in-flight value keeps tracking an off-rect pointer,
    /// and the release commits from out there.
    #[test]
    fn a_captured_slider_keeps_tracking_off_rect_until_release() {
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

        // Press on the rail (row is x 0..200, y 0..20) — captures.
        run_ui(&page, &model, &st, &input_at(100.0, 10.0, true), &mut state);
        assert!(state.dragging.contains("s"), "the grab-band press captures");

        // Still held, but the pointer has left the row entirely (y = 400, far below
        // it). A rect test would drop this node; the capture keeps it dispatching.
        let off = |x: f32, down: bool| UiInput {
            mouse: Vec2::new(x, 400.0),
            clicked: false,
            down,
            screen: Vec2::new(800.0, 600.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let f = run_ui(&page, &model, &st, &off(180.0, true), &mut state);
        assert_eq!(f.results.number("v"), Some(0.0), "commit-on-release still holds off-rect");
        let live = state.drag_value.as_ref().expect("the off-rect drag still holds a value");
        let Value::Number(n) = live.1 else { panic!("a slider drags a number") };
        assert!(n > 80.0, "the value tracks the pointer's x even off the row: {n}");

        // Release from out there: the one real write lands, and the capture ends.
        let f = run_ui(&page, &model, &st, &off(180.0, false), &mut state);
        let v = f.results.number("v").expect("an off-rect release still commits");
        assert!(v > 80.0, "release commits the off-rect dragged value: {v}");
        assert!(state.dragging.is_empty(), "capture released");
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

        // Click in the ±6px slop ABOVE the track (y=10, inside 8..14): captures,
        // maps the pointer over the track ((170-80)/180 = 50) into the in-flight
        // value — and commits it on release (commit-on-release: mid-drag results
        // keep echoing the resting 25).
        let mut state = UiState::new();
        let f = run_ui(&page, &model, &st, &input_at(170.0, 10.0, true), &mut state);
        assert_eq!(f.results.number("v"), Some(25.0), "the press frame emits nothing yet");
        let live = state.drag_value.as_ref().expect("grab-band press captures the value");
        let Value::Number(n) = live.1 else { panic!("a slider drags a number") };
        assert!((n - 50.0).abs() < 2.0, "press maps the pointer over the track: {n}");
        let up = UiInput { mouse: Vec2::new(170.0, 10.0), clicked: false, down: false, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui(&page, &model, &st, &up, &mut state);
        let v = f.results.number("v").expect("release commits");
        assert!((v - 50.0).abs() < 2.0, "release commits the mapped value: {v}");

        // Idle frame with the pointer off the row: the group-focus key echoes the
        // model's persisted focus.
        let mut state = UiState::new();
        let focused = ValueMap::new().with("v", 25.0).with("fit_focus", "v");
        let f = run_ui(&page, &focused, &st, &input_at(700.0, 500.0, false), &mut state);
        assert_eq!(f.results.text("fit_focus"), Some("v"), "focus echoes off-pointer");
    }

    /// Local display ownership: a committed control does NOT wait on the scene. The
    /// model here NEVER moves — the scene has not folded the edit back (or publishes
    /// the key conditionally, which is the same thing from the control's side) — and
    /// the slider still holds what the user set instead of snapping to the stale 25.
    #[test]
    fn a_committed_value_survives_a_model_that_never_catches_up() {
        let (page, st) = grab_slider();
        let stale = ValueMap::new().with("v", 25.0);
        let mut state = UiState::new();

        run_ui(&page, &stale, &st, &input_at(170.0, 10.0, true), &mut state);
        let up = UiInput { mouse: Vec2::new(170.0, 10.0), clicked: false, down: false, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        let f = run_ui(&page, &stale, &st, &up, &mut state);
        let committed = f.results.number("v").expect("release commits");
        assert!((committed - 50.0).abs() < 2.0, "release commits: {committed}");

        // The frames that used to snap back. Nothing has changed the model, so nothing
        // has the standing to move the control.
        for frame in 0..3 {
            let f = run_ui(&page, &stale, &st, &input_at(700.0, 500.0, false), &mut state);
            let v = f.results.number("v").expect("a bound control always reports");
            assert!(
                (v - committed).abs() < f32::EPSILON as f64,
                "frame {frame} after release fell back to the stale model: {v}",
            );
        }
    }

    /// The other half of the contract: local ownership yields to the AUTHORITY. A
    /// model that moves on its own — an external change, or the scene clamping the
    /// commit to something it will actually honour — takes the control with it.
    #[test]
    fn an_external_model_change_outranks_the_local_edit() {
        let (page, st) = grab_slider();
        let stale = ValueMap::new().with("v", 25.0);
        let mut state = UiState::new();

        run_ui(&page, &stale, &st, &input_at(170.0, 10.0, true), &mut state);
        let up = UiInput { mouse: Vec2::new(170.0, 10.0), clicked: false, down: false, screen: Vec2::new(800.0, 600.0), typed: String::new(), backspace: false, wheel: 0.0 };
        run_ui(&page, &stale, &st, &up, &mut state);

        // The scene honours the edit as 40 (a clamp, a snap-to-step, a sim talking
        // back) rather than the 50 the pointer asked for.
        let clamped = ValueMap::new().with("v", 40.0);
        let f = run_ui(&page, &clamped, &st, &input_at(700.0, 500.0, false), &mut state);
        assert_eq!(f.results.number("v"), Some(40.0), "the control follows the authority");
        assert!(state.local.is_empty(), "an overruled entry stops being held");

        // And the ordinary agreement case: the scene arrives at exactly what was
        // committed, so the entry has nothing left to carry.
        let mut state = UiState::new();
        run_ui(&page, &stale, &st, &input_at(170.0, 10.0, true), &mut state);
        run_ui(&page, &stale, &st, &up, &mut state);
        let agreed = ValueMap::new().with("v", 50.0);
        let f = run_ui(&page, &agreed, &st, &input_at(700.0, 500.0, false), &mut state);
        assert_eq!(f.results.number("v"), Some(50.0));
        assert!(state.local.is_empty(), "the scene agreed — nothing left to hold");
    }

    /// The grab-band slider both ownership tests drive: track x 80..260, y 14..26,
    /// grab band y 8..32; a press at (170, 10) maps to 50 of 0..100.
    fn grab_slider() -> (UiNode, Json) {
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
        let mut page = node("screen");
        page.children = vec![sl];
        (page, serde_json::json!({}))
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

    /// A width-sized node with an `aspect` derives its HEIGHT (the cover-fit Muse:
    /// full viewport width, square, right-anchored) — so on a 800×600 screen the
    /// plate is 800×800, flush right, vertically centred, spilling 100px past the
    /// top AND the bottom instead of letterboxing.
    #[test]
    fn aspect_derives_height_from_a_given_width_and_right_anchor_centres_it() {
        let mut muse = node("sprite");
        muse.anchor = Some(UiAnchor::Right);
        muse = prop(muse, "tex", Value::Number(4.0));
        muse = prop(muse, "width_frac", Value::Number(1.0));
        muse = prop(muse, "aspect", Value::Number(1.0)); // square → height follows width

        let mut page = node("screen");
        page.children = vec![muse];

        let model = ValueMap::new();
        let mut state = UiState::new();
        let frame =
            run_ui(&page, &model, &serde_json::json!({}), &input_at(0.0, 0.0, false), &mut state);
        let (x, y, w, h) = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Sprite { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .expect("sprite drawn");
        assert!((w - 800.0).abs() < 0.5, "full viewport width, got {w}");
        assert!((h - 800.0).abs() < 0.5, "aspect=1 derives height from width, got {h}");
        assert!((x - 0.0).abs() < 0.5, "full-width plate reaches the right edge, got x={x}");
        assert!((y - (-100.0)).abs() < 0.5, "vertically centred → equal 100px spill, got y={y}");
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
        let frame = run_ui(&page, &model, &styles(), &input_at(0.0, 0.0, false), &mut state);

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

        let frame = run_ui(&page, &model, &styles, &input_at(400.0, 400.0, false), &mut state);

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

        let mut page = node("screen");
        page.children = vec![tip.clone()];

        // Click squarely over the card (rect 20,20 .. 240,84; centre ≈ 130,52) — a
        // presentational tip claims nothing.
        let frame = run_ui(&page, &model, &styles, &input_at(130.0, 52.0, true), &mut state);
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
        let frame = run_ui(&page2, &model, &styles, &input_at(-9.0, -9.0, false), &mut state);
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "Emberlash")),
            "name still drawn without a rune"
        );
        assert!(
            !frame.commands.iter().any(|c| matches!(c, HudCommand::Text { font, .. } if *font == FontRole::Rune)),
            "no rune glyph when the prop is absent"
        );
    }

    // Uses inline style json rather than the shared `styles()` fixture.
    #[test]
    fn nav_focus_lights_until_the_pointer_takes_over() {
        // The launcher bug (Aaron 2026-08-01): the seeded nav focus drew `hot`
        // permanently, so pointer hover on the first button changed nothing.
        // Modality arbitration: nav mode (the entry state) lights the focused
        // node; REAL pointer movement takes the highlight over; hover then works
        // on the same node. A nav signal hands it back (walker test).
        let mut b = node("button");
        b.id = "go".into();
        b.width = Some(120.0);
        b.height = Some(40.0);
        b.anchor = Some(UiAnchor::TopLeft);
        b = prop(b, "style", Value::Text("btn".into()));
        let mut page = node("screen");
        page.children = vec![b];
        let st = serde_json::json!({ "btn": {
            "fill_top": [0.1, 0.1, 0.1, 1.0], "hover_top": [0.9, 0.9, 0.9, 1.0] } });
        let model = ValueMap::new();
        let mut state = UiState::new();
        state.request_focus("go");
        let first_fill = |frame: &UiFrame| {
            frame
                .commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Panel { color, .. } => Some(*color),
                    _ => None,
                })
                .expect("button drew")
        };

        // Frame 1: pointer parked far away, never moved — nav mode, the seed lights.
        let f1 =
            run_ui(&page, &model, &st, &input_at(500.0, 400.0, false), &mut state);
        assert!(state.nav_mode(), "entry state is nav modality");
        assert_eq!(first_fill(&f1)[0], 0.9, "seeded nav focus draws the hover state");

        // Frame 2: the pointer MOVES (still outside the rect) — takeover, highlight yields.
        let f2 =
            run_ui(&page, &model, &st, &input_at(510.0, 400.0, false), &mut state);
        assert!(!state.nav_mode(), "real pointer movement takes the modality over");
        assert_eq!(first_fill(&f2)[0], 0.1, "the focused button unlights under pointer modality");

        // Frame 3: hover the button — pointer hover works on the very same node.
        let f3 =
            run_ui(&page, &model, &st, &input_at(60.0, 20.0, false), &mut state);
        assert_eq!(first_fill(&f3)[0], 0.9, "pointer hover lights it in pointer modality");
    }

    #[test]
    fn resource_gauge_fills_by_fraction_and_stays_passive() {
        // A half-full mana bar, 200×20 at the origin: a track panel spanning the
        // node and a fill spanning half the padded width, claiming nothing.
        let mut g = node("resource_gauge");
        g.id = "g".into();
        g.width = Some(200.0);
        g.height = Some(20.0);
        g.anchor = Some(UiAnchor::TopLeft);
        g.bind = Some("mp".into());
        g = prop(g, "tone", Value::Text("mana".into()));
        g = prop(g, "style", Value::Text("resource_gauge".into()));
        let mut page = node("screen");
        page.children = vec![g];

        let st = serde_json::json!({
            "resource_gauge": {
                "track": [0.04, 0.05, 0.06, 1.0], "border": [0.17, 0.19, 0.24, 1.0],
                "radius": 10, "pad": 2, "sheen": [0.9, 0.94, 1.0, 0.16],
                "mana_top": [0.35, 0.53, 0.95, 1.0], "mana_bot": [0.12, 0.25, 0.61, 1.0]
            }
        });
        let model = ValueMap::new().with("mp", 0.5);
        let mut state = UiState::new();

        let frame =
            run_ui(&page, &model, &st, &input_at(100.0, 10.0, true), &mut state);
        assert!(!frame.results.is_on("hud_hit"), "a read-only gauge never claims the pointer");
        assert!(frame.stats.redraw_nodes >= 1, "the bar really drew this frame");
        let panels: Vec<_> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { x, w, .. } => Some((*x, *w)),
                _ => None,
            })
            .collect();
        assert_eq!(panels.len(), 2, "track + fill: {panels:?}");
        assert!((panels[0].1 - 200.0).abs() < 0.01, "track spans the node");
        assert!(
            (panels[1].1 - (200.0 - 4.0) * 0.5).abs() < 0.6,
            "fill = the bind fraction of the padded track: {}",
            panels[1].1
        );
    }

    /// The `gauge` was the one restored control with NO coverage at all — a pure
    /// read-out that nothing else in the suite would notice going quiet. It draws in the
    /// engine, it marks the reading in the band's colour or the caution one, and a
    /// NEGATIVE reading is *no signal yet* rather than a value pinned at the floor.
    #[test]
    fn gauge_marks_its_reading_and_washes_out_with_no_signal() {
        // One habitability axis: a 200×12 bar with a 0.3..0.7 band, pointer parked on it
        // with the button down (a read-out must ignore both).
        let mut g = node("gauge");
        g.id = "g".into();
        g.width = Some(200.0);
        g.height = Some(12.0);
        g.anchor = Some(UiAnchor::TopLeft);
        g.bind = Some("temp".into());
        g = prop(g, "lo", Value::Number(0.3));
        g = prop(g, "hi", Value::Number(0.7));
        g = prop(g, "style", Value::Text("gauge".into()));
        let mut page = node("screen");
        page.children = vec![g];

        let st = serde_json::json!({
            "gauge": {
                "track": [0.08, 0.09, 0.12, 0.95], "band": [0.18, 0.62, 0.36, 0.55],
                "marker": [0.94, 0.8, 0.42, 1.0], "marker_in": [0.18, 0.62, 0.36, 1.0],
                "no_signal": [0.06, 0.06, 0.09, 0.72], "sheen": [0.9, 0.94, 1.0, 0.1]
            }
        });
        let mut state = UiState::new();
        let read = |v: f64, state: &mut UiState| {
            let model = ValueMap::new().with("temp", v);
            let frame =
                run_ui(&page, &model, &st, &input_at(100.0, 6.0, true), state);
            assert!(!frame.results.is_on("hud_hit"), "a read-out never claims the pointer");
            assert!(frame.stats.redraw_nodes >= 1, "the gauge really drew this frame");
            frame
                .commands
                .iter()
                .filter_map(|c| match c {
                    HudCommand::Rect { x, w, color, .. } => Some((*x, *w, *color)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // Inside the band: track + band + sheen, then a marker in the in-band colour.
        let inside = read(0.5, &mut state);
        assert_eq!(inside.len(), 4, "track + band + sheen + marker: {inside:?}");
        assert!((inside[1].0 - 60.0).abs() < 0.01, "the band starts at `lo` along the track");
        assert!((inside[1].1 - 80.0).abs() < 0.01, "…and spans `hi - lo`");
        assert!((inside[3].0 - 98.0).abs() < 0.01, "the marker is centred on the reading");
        assert_eq!(inside[3].2, [0.18, 0.62, 0.36, 1.0], "an in-band reading marks green");

        // Outside it, the caution stop — the `marker_in` alias must not leak out here.
        let outside = read(0.9, &mut state);
        assert_eq!(outside[3].2, [0.94, 0.8, 0.42, 1.0], "an out-of-band reading marks caution");

        // Negative = NO SIGNAL: the wash covers the bar and no marker is drawn at all.
        let quiet = read(-1.0, &mut state);
        assert_eq!(quiet.len(), 4, "track + band + sheen + wash: {quiet:?}");
        assert_eq!(quiet[3].2, [0.06, 0.06, 0.09, 0.72], "the last rect is the wash…");
        assert!((quiet[3].1 - 200.0).abs() < 0.01, "…covering the whole bar, marker-free");
    }

    /// **The promoted knobs are live.** Restoring the two gauges and the stat dot also
    /// gave each the parameters its module had hardcoded — the marker's overhang, the
    /// caps row's clearance, the sheen's inset and height, the glow's stand-off and
    /// feather — every one defaulting to the literal it replaced, so nothing moved on
    /// screen. A promoted key no arm actually reads would be API surface that silently
    /// does nothing, so each is asserted here to change the picture.
    #[test]
    fn gauge_group_honours_its_promoted_keys() {
        // gauge — `marker_w` + `marker_over` size the caliper, `sheen_h` the top rule.
        let props = serde_json::json!({
            "style": { "track": [0.1,0.1,0.1,1.0], "band": [0.0,1.0,0.0,0.5],
                       "marker": [1.0,1.0,0.0,1.0], "marker_w": 6, "marker_over": 5,
                       "sheen": [1.0,1.0,1.0,0.2], "sheen_h": 3 },
            "lo": 0.25, "hi": 0.75, "bind_value": 1.0
        });
        let mut out = Vec::new();
        draw_gauge(Rect { x: 0.0, y: 10.0, w: 100.0, h: 20.0 }, &props, &mut out);
        match &out[..] {
            [_, _, HudCommand::Rect { h: sheen_h, .. }, HudCommand::Rect { x, y, w, h, .. }] => {
                assert_eq!(*sheen_h, 3.0, "`sheen_h` thickens the top rule");
                assert_eq!((*x, *w), (97.0, 6.0), "`marker_w` sizes the caliper, centred on the value");
                assert_eq!((*y, *h), (5.0, 30.0), "`marker_over` overhangs the track at both ends");
            }
            other => panic!("gauge drew {other:?}"),
        }

        // resource_gauge — `label_gap` clears the caps row, `border_w` sets the rim, and
        // `sheen_inset` / `sheen_h` size the highlight over the fill.
        let props = serde_json::json!({
            "style": { "track": [0.1,0.1,0.1,1.0], "border": [1.0,0.0,0.0,1.0], "border_w": 3,
                       "radius": 4, "pad": 2, "label_size": 10, "label_gap": 20,
                       "mana_top": [0.0,0.0,1.0,1.0], "mana_bot": [0.0,0.0,0.5,1.0],
                       "sheen": [1.0,1.0,1.0,0.2], "sheen_inset": 4, "sheen_h": 2 },
            "tone": "mana", "label": "MP", "bind_value": 1.0
        });
        let mut out = Vec::new();
        draw_resource_gauge(Rect { x: 0.0, y: 0.0, w: 100.0, h: 60.0 }, &props, &mut out);
        match &out[..] {
            [HudCommand::Text { .. }, HudCommand::Panel { y: track_y, h: track_h, border, .. }, HudCommand::Panel { color2, grad, .. }, HudCommand::Rect { x, w, h, .. }] =>
            {
                assert_eq!((*track_y, *track_h), (30.0, 30.0), "`label_gap` clears the caps row");
                assert_eq!(*border, 3.0, "`border_w` sets the track's rim");
                assert_eq!((*color2, *grad), ([0.0, 0.0, 0.5, 1.0], 1.0), "the tone's second stop ramps the fill");
                assert_eq!((*x, *w, *h), (6.0, 88.0, 2.0), "`sheen_inset` + `sheen_h` size the highlight");
            }
            other => panic!("resource_gauge drew {other:?}"),
        }

        // stat_dot — `glow_pad` stands the ring off the gem, `glow_feather` softens it,
        // and `radius` + `border_w` make a rounded sigil out of the same component.
        let props = serde_json::json!({
            "style": { "hues": { "red": { "fill": [1.0,0.0,0.0,1.0], "glow": [1.0,0.0,0.0,0.5] } },
                       "border": [0.0,0.0,0.0,0.5], "border_w": 2, "radius": 3,
                       "glow_pad": 6, "glow_feather": 9 },
            "hue": "red"
        });
        let mut out = Vec::new();
        draw_stat_dot(Rect { x: 0.0, y: 0.0, w: 20.0, h: 16.0 }, &props, &mut out);
        match &out[..] {
            [HudCommand::Panel { x, w, radius: glow_r, feather, .. }, HudCommand::Panel { radius: gem_r, border, .. }] =>
            {
                assert_eq!((*x, *w), (-4.0, 28.0), "`glow_pad` stands the ring off the gem");
                assert_eq!((*glow_r, *feather), (9.0, 9.0), "…and `glow_feather` softens it");
                assert_eq!((*gem_r, *border), (3.0, 2.0), "a rounded sigil with an authored rim");
            }
            other => panic!("stat_dot drew {other:?}"),
        }

        // A mistyped bind (a NUMBER hue) indexes `hues` with a key it cannot hold: the
        // lookup MISSES and the flat floor shows through — it does not quietly resolve
        // the default block, which would draw a glow the author never asked for.
        let props = serde_json::json!({
            "style": { "hues": { "blue": { "fill": [0.0,0.0,1.0,1.0], "glow": [0.0,0.0,1.0,0.5] } } },
            "hue": 3
        });
        let mut out = Vec::new();
        draw_stat_dot(Rect { x: 0.0, y: 0.0, w: 16.0, h: 16.0 }, &props, &mut out);
        match &out[..] {
            [HudCommand::Panel { color, .. }] => {
                assert_eq!(*color, SIG_BLUE, "no block, no glow — just the const floor");
            }
            other => panic!("a missed hue drew {other:?}"),
        }
    }

    #[test]
    fn action_slot_receives_generic_binds_and_claims_clicks() {
        // The generic `<name>_bind` channel: cd_bind/charges_bind values arrive as
        // `cd`/`charges`, sizing the cooldown veil and the charge count; the slot
        // itself is a rect control that fires its action on click — drawn and hit in
        // the ENGINE since the slots slice.
        let mut a = node("action_slot");
        a.id = "slot1".into();
        a.width = Some(58.0);
        a.height = Some(58.0);
        a.anchor = Some(UiAnchor::TopLeft);
        a.action = Some("cast_1".into());
        a = prop(a, "rune", Value::Text("ᚱ".into()));
        a = prop(a, "cd_bind", Value::Text("cd1".into()));
        a = prop(a, "charges_bind", Value::Text("ch1".into()));
        a = prop(a, "style", Value::Text("action_slot".into()));
        let mut page = node("screen");
        page.children = vec![a];

        let st = serde_json::json!({
            "action_slot": {
                "bg_top": [0.13, 0.14, 0.18, 1.0], "bg_bot": [0.04, 0.05, 0.06, 1.0],
                "border": [0.23, 0.25, 0.31, 1.0], "rim": [0.72, 0.59, 0.35, 0.14],
                "radius": 4, "rune_color": [0.72, 0.59, 0.35, 1.0],
                "rune_halo": [0.44, 0.59, 1.0, 0.38],
                "charge_color": [0.62, 0.72, 1.0, 1.0], "charge_size": 11,
                "cd_veil": [0.02, 0.03, 0.05, 0.72]
            }
        });
        let model = ValueMap::new().with("cd1", 0.25).with("ch1", 3.0);
        let mut state = UiState::new();

        let frame =
            run_ui(&page, &model, &st, &input_at(30.0, 30.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"), "the slot claims the pointer");
        assert!(frame.results.is_on("cast_1"), "click fires the slot's action");
        let veil = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { h, color, .. } if color[3] > 0.7 && color[3] < 0.74 => Some(*h),
                _ => None,
            })
            .next_back()
            .expect("cooldown veil drawn");
        assert!((veil - 56.0 * 0.25).abs() < 0.6, "veil covers the cd fraction: {veil}");
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "3")),
            "bound charge count renders as an integer"
        );
        assert!(frame.stats.redraw_nodes >= 1, "the slot really drew this frame");
    }

    #[test]
    fn medallion_and_stat_dot_draw_passively() {
        // Presentational pair: a sapphire-ring medallion (halo + ring + well) and a
        // green stat dot (glow + gem). Neither may claim the pointer.
        let mut m = node("medallion");
        m.width = Some(58.0);
        m.height = Some(58.0);
        m.anchor = Some(UiAnchor::TopLeft);
        m = prop(m, "ring", Value::Text("sapphire".into()));
        m = prop(m, "rune", Value::Text("ᛞ".into()));
        m = prop(m, "style", Value::Text("medallion".into()));
        let mut d = node("stat_dot");
        d.width = Some(16.0);
        d.height = Some(16.0);
        d.anchor = Some(UiAnchor::TopRight);
        d = prop(d, "hue", Value::Text("green".into()));
        d = prop(d, "style", Value::Text("stat_dot".into()));
        let mut page = node("screen");
        page.children = vec![m, d];

        let st = serde_json::json!({
            "medallion": {
                "rim": [0.06, 0.02, 0.03, 1.0], "bg_top": [0.14, 0.08, 0.09, 1.0],
                "bg_bot": [0.06, 0.02, 0.03, 1.0], "rune_color": [0.44, 0.59, 1.0, 1.0],
                "ring_w": 3,
                "rings": {
                    "sapphire": { "top": [0.23, 0.35, 0.63, 1.0], "bot": [0.1, 0.19, 0.34, 1.0],
                                   "glow": [0.09, 0.19, 0.38, 0.45], "halo": [0.44, 0.59, 1.0, 0.38] }
                }
            },
            "stat_dot": {
                "border": [0.0, 0.0, 0.0, 0.55],
                "hues": { "green": { "fill": [0.18, 0.62, 0.36, 1.0], "glow": [0.18, 0.62, 0.36, 0.55] } }
            }
        });
        let model = ValueMap::new();
        let mut state = UiState::new();

        let frame =
            run_ui(&page, &model, &st, &input_at(29.0, 29.0, true), &mut state);
        assert!(!frame.results.is_on("hud_hit"), "presentational kinds never claim");
        let panels =
            frame.commands.iter().filter(|c| matches!(c, HudCommand::Panel { .. })).count();
        assert!(panels >= 5, "halo + ring + well + glow + gem drew: {panels}");
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { font, .. } if *font == FontRole::Rune)),
            "the medallion rune renders in the Rune face"
        );
        assert!(frame.stats.redraw_nodes >= 1, "both really drew this frame");
    }

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

        // Pointer over the pill → the badge claims the mouse (scene can't pick through).
        let frame = run_ui(&page, &model, &st, &input_at(30.0, 10.0, true), &mut state);
        assert!(frame.results.is_on("hud_hit"), "pointer over the badge claims the mouse");
        assert!(frame.stats.redraw_nodes >= 1, "the chip really drew this frame");

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
        let frame = run_ui(&page2, &model, &st, &input_at(500.0, 500.0, false), &mut state);
        let fill = frame
            .commands
            .iter()
            .find_map(|c| match c {
                HudCommand::Panel { color, .. } => Some(*color),
                _ => None,
            })
            .expect("solid badge drew its pill");
        assert_eq!(fill, [0.72, 0.59, 0.35, 1.0], "solid overrides tone → solid_bg (bronze)");

        // TONE IS A PREFIX: a tone the arm never names reads its own `<tone>_bg` when the
        // block carries one, and falls through to the neutral pair when it does not — so
        // a new chip colour is a token pair, not a new arm. The fall-through half is what
        // the module did for every unknown tone, and must not change.
        let toned = |style: &Json, tone: &str| {
            let mut b = node("badge");
            b.width = Some(60.0);
            b.height = Some(20.0);
            b.anchor = Some(UiAnchor::TopLeft);
            b = prop(b, "label", Value::Text("!".into()));
            b = prop(b, "tone", Value::Text(tone.into()));
            b = prop(b, "style", Value::Text("badge".into()));
            let mut page = node("screen");
            page.children = vec![b];
            let mut state = UiState::new();
            let f = run_ui(&page, &ValueMap::new(), style, &input_at(500.0, 500.0, false), &mut state);
            f.commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Panel { color, .. } => Some(*color),
                    _ => None,
                })
                .expect("badge drew its pill")
        };
        let mut with_danger = st.clone();
        with_danger["badge"]["danger_bg"] = serde_json::json!([0.55, 0.11, 0.11, 1.0]);
        assert_eq!(
            toned(&with_danger, "danger"),
            [0.55, 0.11, 0.11, 1.0],
            "a tone reads its own `<tone>_bg` stop"
        );
        assert_eq!(
            toned(&st, "danger"),
            [0.08, 0.09, 0.12, 1.0],
            "and falls through to the neutral pair when the block names none"
        );
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

    // (This pin is the SAME command list the Lua module emitted before the draw came
    // back to the engine in the 2026-08-09 restoration — it passed unchanged across the
    // move, which is the proof that a control's tier is invisible to what it draws.)
    #[test]
    fn context_menu_draw_is_byte_pinned() {
        // The full command list for a 3-row menu (plain+hint · active · divider),
        // pointer on row 0.
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
    fn a_warm_context_menu_moves_its_hover_wash_with_the_pointer() {
        // The hover wash is drawn from `mx`/`my`, so a menu that is CACHED from a
        // previous frame must still re-fingerprint when the pointer moves within it —
        // `hot_matters` (component.rs's draw loop) is what folds the pointer in, and it
        // asks `is_rust_component`. Two frames sharing one
        // `UiState` is the only way to see that: every other menu test builds a fresh
        // state, so the cache is always cold and a frozen wash would go unnoticed.
        let _g = crate::strings::test_guard();
        let page = context_menu_tree();
        let styles = context_menu_styles();
        let model = ValueMap::new();
        let mut state = UiState::new();
        let wash_ys = |f: &UiFrame| -> Vec<f32> {
            f.commands
                .iter()
                .filter_map(|c| match c {
                    HudCommand::Rect { y, color, .. } if *color == [0.1, 0.15, 0.25, 1.0] => {
                        Some(*y)
                    }
                    _ => None,
                })
                .collect()
        };

        // Row 0 (y 0..30), then row 4 (y 120..150) — both live rows, so both wash.
        let first = run_ui(&page, &model, &styles, &input_at(100.0, 15.0, false), &mut state);
        assert_eq!(wash_ys(&first), vec![0.0], "the cold frame washes the hovered row");
        let moved = run_ui(&page, &model, &styles, &input_at(100.0, 135.0, false), &mut state);
        assert_eq!(wash_ys(&moved), vec![120.0], "a warm menu's wash follows the pointer");
        assert!(moved.stats.redraw_nodes >= 1, "the moved pointer redrew it: {:?}", moved.stats);
    }

    #[test]
    fn context_menu_row_metrics_are_authorable() {
        // The row insets the module hardcoded are style keys whose DEFAULTS are the
        // shipped values (row_pad 4 · pad_x 14 · hint falling to pad_x · divider inset 8,
        // hairline 1) — `context_menu_draw_is_byte_pinned` holds those defaults, so this
        // only has to show the knobs reach the geometry. 200-wide menu, row_h 30.
        let mut styles = context_menu_styles();
        for (k, v) in
            [("row_pad", 10.0), ("pad_x", 20.0), ("hint_pad", 6.0), ("divider_inset", 24.0), ("divider_h", 3.0)]
        {
            styles["menu"][k] = serde_json::json!(v);
        }
        let f = run_ui(
            &context_menu_tree(),
            &ValueMap::new(),
            &styles,
            &input_at(100.0, 15.0, false),
            &mut UiState::new(),
        );
        let rect_at = |color: [f32; 4]| {
            f.commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Rect { x, y, w, h, color: c2, .. } if *c2 == color => {
                        Some((*x, *y, *w, *h))
                    }
                    _ => None,
                })
                .expect("that wash / hairline drew")
        };
        assert_eq!(rect_at([0.1, 0.15, 0.25, 1.0]), (10.0, 0.0, 180.0, 30.0), "row_pad insets the wash");
        assert_eq!(rect_at([0.3, 0.3, 0.3, 1.0]), (24.0, 75.0, 152.0, 3.0), "the hairline's inset + weight");
        let text_x = |s: &str| {
            f.commands
                .iter()
                .find_map(|c| match c {
                    HudCommand::Text { text, x, .. } if text == s => Some(*x),
                    _ => None,
                })
                .expect("that label drew")
        };
        assert_eq!(text_x("Cut"), 20.0, "pad_x insets the label");
        assert_eq!(text_x("X"), 194.0, "hint_pad insets the keybind from the right edge");
    }

    #[test]
    fn a_visible_context_menu_idles_without_redrawing() {
        // An on-screen context menu with the pointer resting on it: the cold frame
        // draws it, and the second, unchanged frame replays from the draw cache
        // without redrawing while the surface keeps its claim.
        let _g = crate::strings::test_guard();
        let page = context_menu_tree();
        let styles = context_menu_styles();
        let model = ValueMap::new();
        let mut state = UiState::new();
        let over = input_at(100.0, 15.0, false);

        let first = run_ui(&page, &model, &styles, &over, &mut state);
        assert!(first.results.is_on("hud_hit"), "pointer over the menu claims");
        assert!(first.stats.redraw_nodes >= 1, "the cold frame really drew it: {:?}", first.stats);

        let second = run_ui(&page, &model, &styles, &over, &mut state);
        assert_eq!(second.stats.redraw_nodes, 0, "idle frame: nothing redraws");
        assert_eq!(second.commands, first.commands, "…and the replay is byte-identical");
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

    // ── The values group's byte pins ─────────────────────────────────────────
    //
    // `slider` / `stepper` / `text_field` came back to the engine tier under ruling
    // BF0AF0C9. A transient harness gated that move by diffing each new arm against
    // the `ui/<kind>.lua` module it replaced, command for command, across every
    // branch — and was deleted with the port (as its S6 predecessor was), because a
    // permanent test may not depend on modules the tier sweep is about to remove.
    // These pins inherit its duty for the two kinds that had none: the numbers below
    // ARE the module's output, transcribed after that diff came back clean.

    fn pin_rect(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> HudCommand {
        HudCommand::Rect { x, y, w, h, color, layer: 0.0 }
    }

    fn pin_text(
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: [f32; 4],
        align: TextAlign,
        font: FontRole,
    ) -> HudCommand {
        HudCommand::Text {
            x,
            y,
            text: text.to_string(),
            size,
            color,
            layer: 0.0,
            align,
            font,
            italic: false,
            bold: false,
            tracking: -1.0,
            wrap: None,
        }
    }

    /// The full slider palette, every stop distinguishable so a pin can tell which
    /// alias the arm actually picked.
    fn pinned_slider_style() -> Json {
        serde_json::json!({
            "track": [0.1, 0.11, 0.12, 1.0], "fill": [0.2, 0.4, 0.7, 1.0],
            "fill_hi": [0.6, 0.8, 1.0, 0.5], "handle": [0.9, 0.9, 1.0, 1.0],
            "focus_track": [0.2, 0.2, 0.3, 1.0], "focus_fill": [0.4, 0.6, 1.0, 1.0],
            "focus_label": [0.5, 0.7, 1.0, 1.0], "value_color": [0.4, 0.4, 0.35, 1.0],
            "label_size": 15, "value_size": 11, "handle_w": 7
        })
    }

    #[test]
    fn slider_draw_is_byte_pinned() {
        const INK_C: [f32; 4] = [0.871, 0.847, 0.788, 1.0];
        const TRACK: [f32; 4] = [0.1, 0.11, 0.12, 1.0];
        const FILL: [f32; 4] = [0.2, 0.4, 0.7, 1.0];
        const HI: [f32; 4] = [0.6, 0.8, 1.0, 0.5];
        const HANDLE: [f32; 4] = [0.9, 0.9, 1.0, 1.0];
        const VALUE: [f32; 4] = [0.4, 0.4, 0.35, 1.0];
        let draw = |r: Rect, props: &Json| {
            let mut out = Vec::new();
            draw_slider(r, props, &mut out);
            out
        };

        // LYING DOWN, a full row at (12,20) 300×40: an 80px caption column, a 40px
        // readout column, a 12px rail centred between them, the value 2.5 of −10..10
        // (t = 0.625) and a `+`-signed one-decimal readout with a suffix.
        let row = Rect { x: 12.0, y: 20.0, w: 300.0, h: 40.0 };
        let props = serde_json::json!({
            "style": pinned_slider_style(), "label": "SIZE", "layer": 0.0,
            "label_w": 80.0, "value_w": 40.0, "slider_h": 12.0,
            "min": -10.0, "max": 10.0, "bind_value": 2.5,
            "decimals": 1.0, "suffix": " kg", "plus": true
        });
        assert_eq!(
            draw(row, &props),
            vec![
                // Caption: the rect's left edge, centred on the 15px line.
                pin_text(12.0, 32.5, "SIZE", 15.0, INK_C, TextAlign::Left, FontRole::Body),
                // Rail: x 12+80, w 300−80−40, y centred for a 12px height.
                pin_rect(92.0, 34.0, 180.0, 12.0, TRACK),
                // Fill to 0.625 of 180, then the 1px highlight along its top edge.
                pin_rect(92.0, 34.0, 112.5, 12.0, FILL),
                pin_rect(92.0, 34.0, 112.5, 1.0, HI),
                // Handle: 7 wide, centred on the fill's end, overhanging 4px each side.
                pin_rect(201.0, 30.0, 7.0, 20.0, HANDLE),
                // Readout: right-aligned on the row's own right edge.
                pin_text(312.0, 34.5, "+2.5 kg", 11.0, VALUE, TextAlign::Right, FontRole::Body),
            ],
            "the horizontal slider draw is byte-stable"
        );

        // UPRIGHT at (40,10) 60×200: the caption owns a band across the top (15px line
        // + an 8px gap), the rail takes the remaining 177px, BOTTOM is min — value 4 of
        // 1..9 fills 0.375 of the rail FROM THE FLOOR — the handle is a bar, the live
        // readout rides beside it and the range marks sit off the rail's far side.
        let dial = Rect { x: 40.0, y: 10.0, w: 60.0, h: 200.0 };
        let props = serde_json::json!({
            "style": pinned_slider_style(), "label": "POP", "layer": 0.0, "vertical": true,
            "slider_h": 10.0, "value_w": 44.0, "min": 1.0, "max": 9.0,
            "bind_value": 4.0, "decimals": 0.0
        });
        assert_eq!(
            draw(dial, &props),
            vec![
                pin_text(40.0, 10.0, "POP", 15.0, INK_C, TextAlign::Left, FontRole::Body),
                pin_rect(65.0, 33.0, 10.0, 177.0, TRACK),
                pin_rect(65.0, 143.625, 10.0, 66.375, FILL),
                pin_rect(65.0, 143.625, 10.0, 1.0, HI),
                pin_rect(61.0, 140.125, 18.0, 7.0, HANDLE),
                pin_text(85.0, 138.125, "4", 11.0, VALUE, TextAlign::Left, FontRole::Body),
                // Range marks: `value_size` − 2, the MAX at the top (a planet grows up).
                pin_text(55.0, 28.5, "9", 9.0, VALUE, TextAlign::Right, FontRole::Body),
                pin_text(55.0, 205.5, "1", 9.0, VALUE, TextAlign::Right, FontRole::Body),
            ],
            "the upright slider draw is byte-stable"
        );
    }

    #[test]
    fn stepper_draw_is_byte_pinned() {
        const INK_C: [f32; 4] = [0.871, 0.847, 0.788, 1.0];
        let draw = |r: Rect, props: &Json| {
            let mut out = Vec::new();
            draw_stepper(r, props, &mut out);
            out
        };
        let row = Rect { x: 12.0, y: 20.0, w: 160.0, h: 28.0 };

        // Bare style, no caption: every fallback const and default fires — the panel
        // floor for the field, the stone floor for the end cells, ink for all three
        // faces, a 13px line and `value_size` falling back to it.
        assert_eq!(
            draw(row, &serde_json::json!({ "style": {}, "label": "", "layer": 0.0, "bind_value": 0.5 })),
            vec![
                pin_rect(12.0, 20.0, 160.0, 28.0, PANEL),
                pin_rect(12.0, 20.0, 28.0, 28.0, STONE),
                pin_rect(144.0, 20.0, 28.0, 28.0, STONE),
                pin_text(26.0, 27.5, "-", 13.0, INK_C, TextAlign::Center, FontRole::Label),
                pin_text(158.0, 27.5, "+", 13.0, INK_C, TextAlign::Center, FontRole::Label),
                pin_text(92.0, 27.5, "0.50", 13.0, INK_C, TextAlign::Center, FontRole::Body),
            ],
            "the unstyled stepper draw is byte-stable"
        );

        // Styled with a caption column and a shorter field. `box` is the spelling every
        // authored stepper in the app uses — dropping that alias would black out all of
        // them, so it is pinned here rather than left to the `field` name alone.
        let props = serde_json::json!({
            "style": {
                "box": [0.1, 0.11, 0.12, 1.0], "btn": [0.2, 0.2, 0.24, 1.0],
                "label": [0.9, 0.88, 0.8, 1.0], "value_color": [0.5, 0.5, 0.45, 1.0],
                "label_size": 15, "value_size": 11
            },
            "label": "FPS", "layer": 0.0, "label_w": 60.0, "field_h": 20.0,
            "min": 0.0, "max": 240.0, "bind_value": 60.0, "decimals": 0.0, "suffix": " fps"
        });
        assert_eq!(
            draw(row, &props),
            vec![
                pin_text(12.0, 26.5, "FPS", 15.0, [0.9, 0.88, 0.8, 1.0], TextAlign::Left, FontRole::Body),
                // Field: past the 60px caption column, 20 tall and centred in the row.
                pin_rect(72.0, 24.0, 100.0, 20.0, [0.1, 0.11, 0.12, 1.0]),
                // Each end cell is as wide as the field is TALL — square at any height.
                pin_rect(72.0, 24.0, 20.0, 20.0, [0.2, 0.2, 0.24, 1.0]),
                pin_rect(152.0, 24.0, 20.0, 20.0, [0.2, 0.2, 0.24, 1.0]),
                pin_text(82.0, 26.5, "-", 15.0, [0.9, 0.88, 0.8, 1.0], TextAlign::Center, FontRole::Label),
                pin_text(162.0, 26.5, "+", 15.0, [0.9, 0.88, 0.8, 1.0], TextAlign::Center, FontRole::Label),
                pin_text(122.0, 28.5, "60 fps", 11.0, [0.5, 0.5, 0.45, 1.0], TextAlign::Center, FontRole::Body),
            ],
            "the styled stepper draw is byte-stable"
        );
    }

    /// **The knobs these three gained on their way into the engine are REAL** — the
    /// same gate the option strips hold ([`promoted_strip_style_keys_are_real_knobs`]).
    /// Each was a literal buried in the module the control replaced; each is now a key
    /// whose DEFAULT is that same literal, so nothing moved on screen and a caller who
    /// wants it moved has somewhere to say so. Both halves are asserted: the pins above
    /// prove the defaults still draw the old picture, and this proves the keys move it.
    #[test]
    fn promoted_value_control_keys_are_real_knobs() {
        let rects = |cmds: &[HudCommand]| -> Vec<(f32, f32, f32, f32)> {
            cmds.iter()
                .filter_map(|c| match c {
                    HudCommand::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                    _ => None,
                })
                .collect()
        };
        let texts = |cmds: &[HudCommand]| -> Vec<(f32, f32, String, f32)> {
            cmds.iter()
                .filter_map(|c| match c {
                    HudCommand::Text { x, y, text, size, .. } => {
                        Some((*x, *y, text.clone(), *size))
                    }
                    _ => None,
                })
                .collect()
        };
        let row = Rect { x: 0.0, y: 0.0, w: 200.0, h: 20.0 };

        // `handle_over` — the handle's overhang past the rail, 4px on each side.
        let over = |style: Json| {
            let mut out = Vec::new();
            draw_slider(row, &serde_json::json!({ "style": style, "label": "", "bind_value": 0.0 }), &mut out);
            rects(&out)[2]
        };
        assert_eq!(over(serde_json::json!({})).3, 28.0, "the default still overhangs 4px each side");
        assert_eq!(over(serde_json::json!({ "handle_over": 0.0 })).3, 20.0, "0 flushes it to the rail");

        // `fill_hi_w` — the highlight line's thickness along the fill's leading edge.
        let hi = |style: Json| {
            let mut out = Vec::new();
            draw_slider(row, &serde_json::json!({ "style": style, "label": "", "bind_value": 1.0 }), &mut out);
            rects(&out)[2].3
        };
        let lit = serde_json::json!({ "fill_hi": [1.0, 1.0, 1.0, 1.0] });
        assert_eq!(hi(lit.clone()), 1.0, "the default highlight is a 1px rule");
        let mut thick = lit.clone();
        thick["fill_hi_w"] = serde_json::json!(3.0);
        assert_eq!(hi(thick), 3.0, "…and `fill_hi_w` thickens it");

        // `grab_pad` — how far off a thin rail a press still takes the drag.
        let grabbed = |pad: Json, my: f32| {
            let props = serde_json::json!({ "style": {}, "label": "", "slider_h": 4.0, "grab_pad": pad });
            hit_slider(Vec2::new(100.0, my), row, &props, true, true).capture == Some(true)
        };
        assert!(grabbed(Json::Null, 4.0), "the default band reaches 6px above the rail");
        assert!(!grabbed(serde_json::json!(0.0), 4.0), "…and `grab_pad` 0 tightens it to the rail");

        // `btn_w` — the stepper's end cells stop being squares when a row says so.
        let cells = |props: Json| {
            let mut out = Vec::new();
            draw_stepper(row, &props, &mut out);
            rects(&out)
        };
        assert_eq!(cells(serde_json::json!({ "style": {}, "label": "" }))[1].2, 20.0, "square by default");
        let wide = cells(serde_json::json!({ "style": {}, "label": "", "btn_w": 40.0 }));
        assert_eq!((wide[1].2, wide[2].0), (40.0, 160.0), "`btn_w` widens both ends");

        // `dec_glyph` / `inc_glyph` — the two faces, ASCII by default.
        let faces = |props: Json| {
            let mut out = Vec::new();
            draw_stepper(row, &props, &mut out);
            let t = texts(&out);
            (t[0].2.clone(), t[1].2.clone())
        };
        assert_eq!(faces(serde_json::json!({ "style": {}, "label": "" })), ("-".into(), "+".into()));
        let arrows = serde_json::json!({ "style": {}, "label": "", "dec_glyph": "◀", "inc_glyph": "▶" });
        assert_eq!(faces(arrows), ("◀".to_string(), "▶".to_string()), "the faces are authorable");

        // `focus_border_w` — the text field's focus ring, 2px against a 1px rest.
        let ring = |props: Json| {
            let mut out = Vec::new();
            draw_text_field(row, &props, &mut out);
            match out[0] {
                HudCommand::Panel { border, .. } => border,
                _ => panic!("the well draws first"),
            }
        };
        let lit_border = serde_json::json!({ "border": [1.0, 1.0, 1.0, 1.0] });
        assert_eq!(ring(serde_json::json!({ "style": lit_border, "mx": -9.0, "my": -9.0 })), 1.0);
        let mut focused = serde_json::json!({ "style": lit_border, "mx": -9.0, "my": -9.0 });
        focused["focused"] = Json::Bool(true);
        assert_eq!(ring(focused.clone()), 2.0, "the focus ring is 2px by default");
        focused["style"]["focus_border_w"] = serde_json::json!(4.0);
        assert_eq!(ring(focused), 4.0, "…and the style can thicken it");
    }
}
