//! [`WalkerHandler`] — the adapter that makes the component walker a **layer
//! of the input event bus** (spec §4.1 / task A).
//!
//! The walker already computes the one fact the router needs: `hud_hit`
//! ("UI consumed the pointer this frame", `component.rs`'s [`run_ui`](crate::run_ui)
//! result). This adapter wraps that fact as a
//! [`flicker_input_router::InputHandler`]: its [`handle`](InputHandler::handle)
//! returns [`Flow::Consumed`] for pointer-driven signals when the walker owns the
//! pointer, so the gameplay base handler below it never world-picks *through* a
//! panel — exactly the old `!hud_hit && !chat_hit` gate, now structural.
//!
//! Focus is the walker's other job on the bus (spec §4.3): a single
//! [`UiState::focus`](crate::UiState) id is the source of truth for **both** mouse
//! and gamepad, so the router writes focus *through* the walker via
//! [`apply_focus`](WalkerHandler::apply_focus) rather than owning a second store.
//!
//! # Directional nav (spec §8, flattened per plan 1A292918 T2/T3)
//!
//! When the walker layer is given the current UI tree ([`WalkerHandler::with_nav`])
//! — and, for spatial moves, this frame's resolved rects
//! ([`with_rects`](WalkerHandler::with_rects)) — it **consumes** the directional-nav
//! signals while it owns a focusable tree. **One meaning per signal, and the walker
//! owns all of them:**
//!
//! **The IMPLIED PANEL CONTEXT (Aaron 2026-09-02).** There is no enter step and no
//! lock: the panel the cursor sits in — the container the focused control belongs to,
//! or the focused container itself when it has no interior (a viewport pane) — IS the
//! context. It wears the sapphire rim ([`UiState::focused_pane`]), the left stick
//! switches it, and ABXY/d-pad route to it.
//!
//! * `NavUp/Down/Left/Right` (the D-PAD) move focus WITHIN the focused pane — its
//!   interior ring ([`nav`], wrapping, with the slider nudge folded in). On an
//!   interior-less container (a viewport pane) they move BETWEEN panel stops instead,
//!   so the d-pad is never dead: GEOMETRICALLY when rects are present
//!   ([`nav_geometric`] — the stop that sits that way on screen, no wrap at the edge),
//!   else by the ordinal stop ring. In-control components answer the d-pad ALONE. Flat
//!   groups (menus, settings rows — no container) ring whole and are unchanged.
//! * `Confirm` fires the focused node's `action` the SAME way a click does
//!   (`results.set(action, true)`), delivered on the ONE drain,
//!   [`take_fired`](WalkerHandler::take_fired), or FLIPS a focused toggle. On an
//!   actionless container it does nothing — a pane is never "entered".
//! * `Cancel` is always scene-level: it requests a context pop / back-out
//!   ([`cancelled`](WalkerHandler::cancelled)); what B means inside a pane is that
//!   pane's own semantics, declared by the scene.
//! * `PanelNext`/`PanelPrev` (the LEFT STICK) are the stick's OWN pane intent — SEPARATE
//!   from the d-pad's `Nav*`, never tangled (Aaron 2026-08-18). They move between panel
//!   stops ([`pane_move`](WalkerHandler::pane_move): geometric with rects, ordinal ring
//!   without) and DESCEND into the landing pane's lowest-ordinal control, so the pane is
//!   operable the moment it is focused; on a menu they hop flat groups
//!   ([`cycle_group`]). They NEVER nudge a control.
//!
//! `TabNext`/`TabPrev` (the bumpers) are deliberately NOT here: they belong to the
//! page/tab control's tab rail, which steps itself.
//!
//! Every focus write lands in the one `UiState.focus` id, so d-pad and pointer
//! share a single highlight. All are consumed so they never leak to gameplay.
//!
//! # No cycle
//!
//! `flicker-widgets` depends on `flicker-input-router` (and `-core`); the router
//! depends on neither. The edge is one-way — frontend → router → core (spec §2 /
//! Risk RT-16) — so hosting the adapter here keeps the router frontend-free.

use flicker_input_core::{ActionSignal, EventKind, InputContext};
use flicker_input_router::{
    nav, nav_geometric, Flow, FocusChange, Focusable, InputEvent, InputHandler, NavDir, RouteCtx,
};
use flicker_script::{UiNode, ValueMap};

use crate::component::{visible, UiState};
use crate::intents::UiIntents;

/// How a focusable value control STEPS under the pad — the axis its own d-pad presses run
/// along, and (for a [`Toggle`](StepKind::Toggle)) that a `Confirm` flips it. The walker
/// classifies each steppable here; `run_ui` applies the step by reading the placed node's
/// kind, so this only drives the walker's nudge-vs-move + Confirm-flip decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StepKind {
    /// A horizontal slider — Left/Right nudge, Up/Down move focus.
    SliderH,
    /// A vertical slider — Up/Down nudge, Left/Right move focus.
    SliderV,
    /// A `select` / `pill_toggle` — Left/Right cycle the index (clamped), Up/Down move.
    Options,
    /// A `toggle` — Left/Right set off/on, Up/Down move, and `Confirm` flips it.
    Toggle,
}

/// The walker as one layer of the event bus. Borrows the retained
/// [`UiState`] (so it can write focus) and carries this frame's `hud_hit`
/// verdict (the canonical `run_ui` "UI consumed pointer").
pub struct WalkerHandler<'a> {
    ui: &'a mut UiState,
    /// This frame's `hud_hit` — the walker consumed the pointer over some UI
    /// region (a panel, a slider drag, …). OR-folded across a scene's walker
    /// passes (e.g. a HUD pass and a floating-chat pass) before construction.
    consumed_pointer: bool,
    /// The focusable nodes of the current UI tree, flattened for directional nav
    /// (spec §8). Empty = this walker layer is **not** navigable, so nav signals
    /// pass through instead of being consumed. Filled by [`Self::with_nav`].
    focusables: Vec<Focusable>,
    /// `(node id → action name)` for the focusables that carry an `action`, so
    /// `Confirm` can fire the focused node's action. Parallel to `focusables`.
    actions: Vec<(String, String)>,
    /// `(node id, StepKind)` for every focusable STEPPABLE value control — a slider,
    /// select, pill_toggle or toggle. Nav on the control's own axis STEPS it (the
    /// component-level pad contract every value control gets, `1B5F6BB8`) instead of moving
    /// focus; the cross axis still navigates away, so a control is never a focus trap.
    /// Filled by [`Self::with_nav`].
    steppables: Vec<(String, StepKind)>,
    /// The `action` of the node `Confirm` activated this frame, if any. Drained
    /// into `fired` by [`Self::take_fired`] — the ONE channel a scene reads.
    activated: Option<String>,
    /// `Cancel` was consumed this frame (the scene pops its modal / backs out).
    cancelled: bool,
    /// The screen's declarative signal bindings (S9): a mapped signal's Press
    /// records its result name in `fired` and the event is consumed. `None` =
    /// the screen declares nothing on this layer. Filled by [`Self::with_intents`].
    intents: Option<&'a UiIntents>,
    /// Result names fired by declared intents this frame, in firing order.
    /// Drained by the scene via [`take_fired`](Self::take_fired).
    fired: Vec<String>,
    /// Every `tab_group` some focusable CLAIMS — the OWNERSHIP half of the nested
    /// pane model (Aaron 2026-08-15): a node whose id is claimed here is a
    /// CONTAINER, whatever tier it sits on. Filled by [`Self::with_nav`].
    claimed: std::collections::HashSet<String>,
    /// Ids of nodes authored `pane: true` — the explicit container marker for a
    /// pane ownership cannot derive (a viewport-only pane has no focusable
    /// members to claim it). Filled by [`Self::with_nav`].
    pane_flagged: std::collections::HashSet<String>,
    /// Every visible `text_field` with an id — the nodes a text-entry session can
    /// open on, with their authored exits. Filled by [`Self::with_nav`].
    text_fields: Vec<TextFieldNav>,
    /// Whether this screen's TOPMOST visible modal slab currently lets Cancel leave —
    /// [`popup_dismissable`], read once per frame from the same tree + model the walk
    /// used. `true` (the default, and the answer for every screen with no slab) leaves
    /// Cancel exactly as it was; `false` makes this layer SWALLOW it. Filled by
    /// [`Self::with_nav`].
    dismissable: bool,
}

/// A `text_field` the walker can enter (see [`WalkerHandler::frame`]).
#[derive(Clone, Debug)]
struct TextFieldNav {
    id: String,
    /// Result name fired when `SubmitText` closes the session (`submit_action`).
    on_submit: Option<String>,
    /// Result name fired when `CancelText` closes it (`cancel_action`).
    on_cancel: Option<String>,
    /// The first typed character replaces the whole value (`select_all_on_enter`).
    select_all: bool,
    /// The field `EnterText` targets when no text field is focused (`default_text`) —
    /// the chat line's bound key / chord.
    default_text: bool,
}

/// How a text-entry session closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextExit {
    /// `SubmitText`: the value stands, `submit_action` fires.
    Submit,
    /// `CancelText`: the pre-edit value is restored, `cancel_action` fires.
    Cancel,
    /// The focus left the field (a click elsewhere, a nav move): the value stands,
    /// nothing fires.
    Blur,
}

impl<'a> WalkerHandler<'a> {
    /// Build the handler from the retained walker `ui` and this frame's
    /// `consumed_pointer` (the canonical `run_ui` `hud_hit`, `component.rs`; the
    /// scene OR-folds its walker passes into this bool).
    ///
    /// The bool — not a re-walk of the tree — is the input, because `run_ui`
    /// already ran the authoritative layout + hit-test this frame; re-testing the
    /// tree here would duplicate it and could disagree.
    ///
    /// Not navigable by default — call [`with_nav`](Self::with_nav) to enable
    /// directional nav for a screen whose nodes carry `tab_group`/`nav_ordinal`.
    pub fn hud(ui: &'a mut UiState, consumed_pointer: bool) -> Self {
        Self {
            ui,
            consumed_pointer,
            focusables: Vec::new(),
            actions: Vec::new(),
            steppables: Vec::new(),
            activated: None,
            cancelled: false,
            intents: None,
            fired: Vec::new(),
            claimed: std::collections::HashSet::new(),
            pane_flagged: std::collections::HashSet::new(),
            text_fields: Vec::new(),
            // No tree yet ⇒ no slab ⇒ nothing holds Cancel. `with_nav` reads the real
            // answer off the screen.
            dismissable: true,
        }
    }

    /// Make this walker layer **navigable**: flatten `tree`'s focusable nodes
    /// (those carrying a non-empty `tab_group`) so it consumes and routes the
    /// directional-nav signals (spec §8). Pass the SAME tree AND model this
    /// frame's `run_ui` walked, so nav focus and pointer focus address the same
    /// nodes — and so a hidden overlay contributes nothing (see
    /// [`focusables_of`]).
    pub fn with_nav(mut self, tree: &UiNode, model: &ValueMap) -> Self {
        collect_nav(
            tree,
            model,
            &mut self.focusables,
            &mut self.actions,
            &mut self.steppables,
        );
        self = self.with_text_fields(tree, model);
        // THE DISMISSABLE TOGGLE (ruling DA0E1B57): whether the screen's topmost modal
        // slab lets Cancel out is a property of the COMPONENT, read from the same tree
        // and model this pass flattened — riding here, rather than on a builder of its
        // own, because a second builder is a second thing to remember and every screen
        // that can host a modal already calls this one.
        self.dismissable = crate::popup_dismissable(tree, model);
        // OWNERSHIP vs MEMBERSHIP (nested panes, Aaron 2026-08-15): a node's
        // `tab_group` names the ring it BELONGS to; a node is a CONTAINER because
        // other nodes claim its id as their group — or because it authors the
        // explicit `pane: true` marker (a viewport-only pane has no members to
        // claim it). Top-tier containers carry no `tab_group`, so the member pass
        // above skipped them — collect them here into the empty-group ring the
        // left stick cycles, ordered by their AUTHORED `nav_ordinal`.
        self.claimed = self.focusables.iter().map(|f| f.group.clone()).collect();
        collect_containers(
            tree,
            model,
            &self.claimed,
            &mut self.focusables,
            &mut self.pane_flagged,
        );
        // The implied pane follows the retained focus from the first frame — a scene
        // reading `focused_pane` before any signal arrives sees the cursor's pane.
        self.sync_pane();
        self
    }

    /// Make this walker layer the TEXT-ENTRY owner over `tree` WITHOUT making it
    /// navigable: collect the visible `text_field`s (and their authored exits) so a
    /// session can open on one — a click into it, a scene's `open_text_entry`, the
    /// `EnterText` signal — and close through `SubmitText` / `CancelText`. A bench
    /// that routes nav to its own base layer (Quartermaster) uses this alone;
    /// [`with_nav`](Self::with_nav) includes it.
    pub fn with_text_fields(mut self, tree: &UiNode, model: &ValueMap) -> Self {
        collect_text_fields(tree, model, &mut self.text_fields);
        self
    }

    /// Patch this frame's RESOLVED screen rects (`UiFrame.rects`) onto the collected
    /// focusables, so the flattened panel tier can move GEOMETRICALLY
    /// ([`nav_geometric`]) — "Left from the centre lands on the channel beside it."
    /// Call AFTER [`with_nav`](Self::with_nav). Ids absent from `rects` keep the zero
    /// rect and fall back to the ordinal stop ring, so a scene (or a headless test)
    /// that passes nothing still navigates — just not spatially.
    pub fn with_rects(mut self, rects: &[(String, [f32; 4])]) -> Self {
        for f in &mut self.focusables {
            if let Some((_, r)) = rects.iter().find(|(id, _)| id == &f.id) {
                f.rect = *r;
            }
        }
        self
    }

    /// Whether a Cancel would be ANSWERED here rather than passed down: the screen
    /// declared what Cancel means (`on_cancel`), or this layer owns a focusable tree and
    /// so runs the scene-level back-out. The gate the dismissable swallow rides, so a
    /// non-navigable HUD pass never eats a Cancel it was not going to answer anyway.
    fn owns_cancel(&self) -> bool {
        !self.focusables.is_empty()
            || self
                .intents
                .is_some_and(|i| i.result_for(ActionSignal::Cancel).is_some())
    }

    /// Whether `id` is a pane container — claimed by members, or explicitly
    /// authored `pane: true`. The one test `try_enter`, the ring and the flat-group
    /// rule all consult, so the tiers can never disagree about what a container is.
    fn is_container(&self, id: &str) -> bool {
        self.claimed.contains(id) || self.pane_flagged.contains(id)
    }

    /// Bind the screen's declarative intents (S9): a Press of a signal `intents`
    /// maps records its declared result name (drained by
    /// [`take_fired`](Self::take_fired)) and BOTH edges of the signal are
    /// consumed, so a declared signal never leaks below this layer.
    ///
    /// **A declared binding is not a licence to name a WALKER-OWNED signal.** The
    /// declaration channel exists for the signals a screen genuinely owns — Menu,
    /// the page/tab rails, the chord verbs. Confirm, Cancel, `Nav*` and `Panel*`
    /// mean one thing on every screen in Prism (activate / back out / move the
    /// cursor / pick the panel), and this layer answers all of them; a screen that
    /// named one would statically kill it on itself, because the intent branch in
    /// [`handle`](InputHandler::handle) consumes and returns BEFORE [`Self::act`]
    /// (violation F1, 2026-08-09 — every button on a bench went dead exactly this
    /// way). The pointer-consume gate still runs first: a click the HUD owns is
    /// swallowed, never re-fired as an intent.
    pub fn with_intents(mut self, intents: &'a UiIntents) -> Self {
        self.intents = Some(intents);
        self
    }

    /// **The ONE activation drain.** Every result name this walker layer produced
    /// this frame, in firing order: the names of declared intents that fired (S9)
    /// **and** the `action` of the node a pad `Confirm` activated. Both are the
    /// same thing to the scene — a name to fold into its results exactly like a
    /// click (`results.set(name, true)`) and republish once as the transient
    /// `sig_<name>` Model mirror ([`UiIntents::mirror_into`]).
    ///
    /// Confirm rides HERE rather than on a second accessor because a second
    /// accessor is a second thing to remember: nine benches drained the intents
    /// and never read the activation, so a focused button could be reached with
    /// the d-pad and never pressed. One channel, and every scene that already
    /// drains gets pad activation with no new line.
    ///
    /// Also the press-flash clock: every call decays the retained flashes one
    /// step, then lights this dispatch's fired result names — so any BUTTON
    /// whose `action` is that name glows exactly as if it had been clicked
    /// (the flash is the button's activate acknowledgement, one mechanism for
    /// every input route). Riding here — the one call every intent-using scene
    /// already makes exactly once per frame — means no scene adds a tick line,
    /// and a scene without intents (nothing to flash) never pays for one.
    pub fn take_fired(&mut self) -> Vec<String> {
        // A pad Confirm's action joins the drain — `act` already lit its flash, so
        // pushing it here (before the tick's relight loop skips it) keeps ONE list.
        if let Some(action) = self.activated.take() {
            self.fired.push(action);
        }
        self.ui.flash_tick();
        for name in &self.fired {
            self.ui.flash(name);
        }
        // The strip-stepping channel (the same shape as `push_nudge`): the names
        // that fired this frame are handed to the next `run_ui` pass, where a
        // `tabs` / `pill_toggle` naming one as its `next_action`/`prev_action`
        // advances its OWN bind. The control owns its stepping; the scene sees
        // only the resulting index.
        for name in &self.fired {
            self.ui.push_step(name);
        }
        // Fold in this frame's POINTER activations (a click on a button / toggle /
        // context row, recorded by `run_ui`'s hit pass). The hit pass already lit
        // their flash + strip-step, so they join the drain here — AFTER the relight
        // loop, not through it — purely to reach the one `sig_<name>` mirror the
        // scene republishes. Pointer and pad now converge on this single channel: a
        // mouse click on `mode_<realm>` mirrors `sig_mode_<realm>` exactly like a pad
        // Confirm (rule 37722F91 "all input events are signals"; the additive step of
        // pump P2, MCP `0569DA9B` — `hit_node`'s direct fire is removed once every
        // scene drains here).
        let mut fired = std::mem::take(&mut self.fired);
        fired.extend(self.ui.take_pointer_fired());
        fired
    }

    /// Whether `Cancel` was consumed this frame — the scene pops its modal / backs
    /// out (the router-owned `PopContext` is also queued during dispatch).
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// Apply the focus decision reconciled by
    /// [`apply_context_requests`](flicker_input_router::apply_context_requests) to
    /// the walker's retained focus — one id for mouse **and** gamepad (spec §4.3).
    /// `None` leaves focus untouched.
    pub fn apply_focus(&mut self, change: Option<FocusChange>) {
        match change {
            Some(FocusChange::Set(id)) => self.ui.request_focus(id),
            Some(FocusChange::Clear) => self.ui.clear_focus(),
            None => {}
        }
    }

    /// Route one directional-nav signal (on its press edge) into a focus write /
    /// activation / cancel. `current` is the retained focus id.
    fn act(&mut self, signal: ActionSignal, rc: &mut RouteCtx) {
        // Every routed nav-family signal restores nav modality: the focused node
        // lights again after the pointer had taken the highlight over.
        self.ui.note_nav_input();
        let current = self.ui.focused().map(str::to_string);
        match signal {
            ActionSignal::NavUp => self.nudge_or_move(current.as_deref(), NavDir::Up),
            ActionSignal::NavDown => self.nudge_or_move(current.as_deref(), NavDir::Down),
            ActionSignal::NavLeft => self.nudge_or_move(current.as_deref(), NavDir::Left),
            ActionSignal::NavRight => self.nudge_or_move(current.as_deref(), NavDir::Right),
            // The LEFT STICK drives the pane tier through `Panel*` — the stick's OWN
            // intent, SEPARATE from the d-pad's `Nav*` (Aaron 2026-08-18: the two are
            // never tangled, and in-control components answer only the d-pad). It moves
            // between panel stops (geometric with rects, ordinal ring without) and
            // descends into the landing pane; on a menu it hops flat groups. Never
            // gated: the focused pane is the context, so there is nothing to be
            // "inside" of (Aaron 2026-09-02). (The bumpers are NOT here — they belong
            // to the page/tab control's own rail.)
            ActionSignal::PanelNext => self.pane_move(current.as_deref(), NavDir::Right, true),
            ActionSignal::PanelPrev => self.pane_move(current.as_deref(), NavDir::Left, false),
            ActionSignal::Confirm => {
                // THE DRAG CHANNEL rides Confirm exactly as it rides the pointer
                // button (controller is the floor, BA4487BD): pressing a `drag_kind`
                // source picks the payload up, and the next press over a matching
                // `drop_accept` target drops it. WHICH of the two this is, the node's
                // own props decide, so the focused id is recorded here and resolved by
                // the next `run_ui` pass — the same shape as `push_nudge`.
                //
                // While a payload is in flight Confirm IS the drop and nothing else: a
                // pointer release over a target likewise drops rather than clicking,
                // because the click edge was spent on the press that picked it up.
                let carrying = self.ui.drag().is_some();
                if let Some(id) = current.as_deref() {
                    self.ui.push_drag_confirm(id);
                }
                if carrying {
                    self.sync_pane();
                    return;
                }
                // Controller selection of a text field: Confirm on it IS the way in
                // (Aaron 2026-09-03) — the same switch a click into it and `EnterText`
                // reach. A pad press commits no character, so no trigger guard.
                if let Some(id) = current
                    .as_deref()
                    .filter(|id| self.text_field(id).is_some())
                    .map(str::to_string)
                {
                    self.enter_text(rc, &id, 0);
                    self.sync_pane();
                    return;
                }
                // CONFIRM = APPLY (Aaron 2026-09-04): the stages pending in the FOCUSED
                // pane commit first — the pad's release edge — whatever else this press
                // does. The next `run_ui` pass writes them.
                let pane = self.ui.focused_pane().map(str::to_string);
                let committing = self.ui.commit_stages(pane.as_deref());
                // A focused node WITH an action activates like a click (menu buttons,
                // pane controls, settings rows) — same path, same `sig_<name>` mirror.
                let action = current
                    .as_deref()
                    .and_then(|id| self.action_for(id))
                    .map(str::to_string);
                match action {
                    Some(a) if committing => {
                        // "Commit, then fire" (Aaron 2026-09-04): the activation waits
                        // for the pass that lands the commit, so the scene folds the
                        // values before the action — one press, the right order.
                        self.ui.defer_fire(&a);
                    }
                    Some(a) => {
                        // A pad Confirm is an activation like any click — the focused
                        // button lights the same flash (one acknowledgement, every route).
                        self.ui.flash(&a);
                        self.activated = Some(a);
                    }
                    // A focused TOGGLE has no action — Confirm FLIPS it (a dir-0 nudge the
                    // application loop reads as "invert"), so Confirm operates a checkbox
                    // the way it activates a button. Any other actionless node (a pane
                    // container) is a no-op: a pane is never entered (Aaron 2026-09-02).
                    // A Confirm that just committed a stage is SPENT: it must not also
                    // flip the staged toggle straight back.
                    None => {
                        if let Some((id, StepKind::Toggle)) = current
                            .as_deref()
                            .and_then(|id| self.step_kind(id).map(|k| (id, k)))
                            .filter(|_| !committing)
                        {
                            self.ui.push_nudge(id, 0, false);
                        }
                    }
                }
            }
            // Cancel is SCENE-LEVEL, always (Aaron 2026-09-02): with no enter tier there
            // is no level to exit, so B pops the scene's modal/context and the scene
            // decides what backing out means where the cursor stands.
            ActionSignal::Cancel => {
                // Backing out ABANDONS whatever the cursor was carrying — the pad's
                // twin of releasing the button over nothing. Additive: Cancel is still
                // SCENE-LEVEL as always, never consumed by the drag.
                self.ui.cancel_drag();
                self.cancelled = true;
                rc.pop_context();
            }
            _ => {}
        }
        self.sync_pane();
    }

    /// Directional nav with the slider contract folded in: when the FOCUSED
    /// node is a slider and `dir` runs along its own axis, the press NUDGES it
    /// (recorded in [`UiState`], stepped/clamped/written by the next `run_ui`
    /// pass — chord held scales to the coarse step); every other case moves
    /// focus exactly as before. The cross axis still navigates away, so a
    /// slider is never a focus trap.
    fn nudge_or_move(&mut self, current: Option<&str>, dir: NavDir) {
        if let Some(id) = current {
            if let Some(kind) = self.step_kind(id) {
                let vertical = kind == StepKind::SliderV;
                let step = match (vertical, dir) {
                    (true, NavDir::Up) => 1, // up grows — top is max
                    (true, NavDir::Down) => -1,
                    (false, NavDir::Right) => 1,
                    (false, NavDir::Left) => -1,
                    _ => 0, // the cross axis — fall through to focus movement
                };
                if step != 0 {
                    let coarse = self.ui.chord;
                    self.ui.push_nudge(id, step, coarse);
                    return;
                }
            }
        }
        self.move_focus(current, dir);
    }

    /// The [`StepKind`] of a focusable value control, if `id` is one.
    fn step_kind(&self, id: &str) -> Option<StepKind> {
        self.steppables
            .iter()
            .find(|(sid, _)| sid == id)
            .map(|(_, k)| *k)
    }

    fn move_focus(&mut self, current: Option<&str>, dir: NavDir) {
        // The ring of the node being MOVED — the focused control's own pane for the
        // d-pad, or a container's peer tier when the stick (or an interior-less pane)
        // moves the pane itself.
        let ring = self.ring_for(current);
        // A direction on a CONTAINER (the cursor rests on one only when it has no
        // interior — a viewport pane) moves BETWEEN its peer stops. With this frame's
        // resolved rects it moves GEOMETRICALLY (the stop that sits that way on screen)
        // and does NOT wrap at the surface edge; with no rects (headless tests, a scene
        // that passes none) it falls back to the ordinal stop ring, which wraps. The
        // landing pane is DESCENDED into (its lowest-ordinal control takes the cursor).
        // Interior controls and flat-group members fall through to the ordinal ring
        // below — they walk WITHIN their pane; menus and settings rows are unchanged.
        if let Some(cur) = current {
            if self.is_container(cur) {
                let has_rect = ring
                    .iter()
                    .any(|f| f.id == cur && f.rect[2] > 0.0 && f.rect[3] > 0.0);
                let next = if has_rect {
                    nav_geometric(&ring, cur, dir)
                } else {
                    nav(&ring, Some(cur), dir)
                };
                if let Some(id) = next {
                    self.ui.request_focus(id);
                    self.descend();
                }
                return;
            }
        }
        if let Some(id) = nav(&ring, current, dir) {
            self.ui.request_focus(id);
            // Acquiring the tree (no focus yet) or landing on a subpanel container
            // among a pane's members descends the same way — the cursor never rests
            // on a container that has an interior.
            self.descend();
        }
    }

    /// The STICK's pane-tier move (`Panel*` — the stick's OWN intent, SEPARATE from the
    /// d-pad's `Nav*`; Aaron 2026-08-18). On a bench (the focused stop is a container) it
    /// moves between panel stops exactly as the d-pad does at the top tier — geometric
    /// with rects, the ordinal stop ring without — via [`move_focus`]. On a menu (flat
    /// groups, no container) it HOPS groups via [`cycle_group`] (the main-menu pin,
    /// A61E7175). It NEVER nudges a control: the stick is the pane cursor, and in-control
    /// components answer only the d-pad.
    fn pane_move(&mut self, current: Option<&str>, dir: NavDir, forward: bool) {
        // The stick moves the PANE the cursor is in, wherever inside it the cursor
        // stands: resolve the focused control to its container first, then step that
        // container among its peers (geometric with rects) and descend into the landing
        // pane. A flat-group member has no container → hop groups.
        let pane = current
            .and_then(|c| self.pane_of(c))
            .map(str::to_string);
        match pane {
            Some(p) => {
                // The stick steps the pane among its peers in AUTHORED `nav_ordinal`
                // order, wrapping — never geometrically: `Panel*` is Left/Right only,
                // and with the d-pad living inside the pane a geometric stick could not
                // reach a pane stacked above or below another. Authored order is the
                // stick-stop order (the surviving half of 8FBF77AB), and it is never
                // heuristic (BA4487BD).
                let ring = self.ring_for(Some(&p));
                if let Some(id) = nav(&ring, Some(&p), dir) {
                    // PENDING-STAGES GUARD (Aaron 2026-09-04): a pane holding pad-staged
                    // values is never left silently — the move is PARKED and the scene
                    // raises the Apply / Revert prompt; its answer resumes the move
                    // (`frame`) or keeps the cursor here.
                    if self.ui.stages_in(Some(&p)) {
                        self.ui.park_pane_move(id);
                        return;
                    }
                    self.ui.request_focus(id);
                    self.descend();
                }
            }
            None => {
                self.cycle_group(current, forward);
                self.descend();
            }
        }
    }

    /// The pane `id` belongs to: `id` itself when it is a container, else the
    /// container of its `tab_group` when one exists in this frame's tree. `None` for a
    /// flat-group member (menus, settings rows) and for unknown ids.
    fn pane_of<'s>(&'s self, id: &'s str) -> Option<&'s str> {
        if self.is_container(id) {
            return Some(id);
        }
        let group = self.focusables.iter().find(|f| f.id == id)?.group.as_str();
        (!group.is_empty() && self.container_exists(group)).then_some(group)
    }

    /// If the cursor rests on a container that HAS interior controls, drop it onto the
    /// lowest-`(ordinal, id)` one — the pane is operable the moment it is focused, with
    /// no enter step (Aaron 2026-09-02). Nested: a landing member that is itself a
    /// container descends again. An interior-less container (a viewport pane) keeps
    /// the cursor: its rim shows and its camera consumes the sticks.
    fn descend(&mut self) {
        loop {
            let Some(cur) = self.ui.focused().map(str::to_string) else { return };
            if !self.is_container(&cur) {
                return;
            }
            let interior = self
                .focusables
                .iter()
                .filter(|f| f.group == cur)
                .min_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)))
                .map(|f| f.id.clone());
            match interior {
                Some(id) => self.ui.request_focus(id),
                None => return,
            }
        }
    }

    /// Re-derive the IMPLIED PANE from the current focus (`UiState::focused_pane`) —
    /// called after every focus write and once at construction, so a scene reading the
    /// pane at the top of the next frame sees where the cursor went.
    fn sync_pane(&mut self) {
        let pane = self
            .ui
            .focused()
            .and_then(|id| self.pane_of(id))
            .map(str::to_string);
        self.ui.pane = pane;
    }

    fn cycle_group(&mut self, current: Option<&str>, forward: bool) {
        // The stick's ring is ONE STOP PER PANE-OR-GROUP: a container where one
        // exists (paned scenes — authored `nav_ordinal` order), else the group's
        // lowest-(ordinal, id) member — so the flat groups a menu is made of
        // (the mode rail, the scene list) keep their stick hop with no pane
        // ceremony. Wraps both ways; standing anywhere INSIDE a flat group
        // counts as standing on its stop.
        let ring = self.ring();
        let mut stops: Vec<Focusable> = ring
            .iter()
            .filter(|f| f.group.is_empty() && self.is_container(&f.id))
            .cloned()
            .collect();
        let mut flat_groups: Vec<&str> = ring
            .iter()
            .filter(|f| !f.group.is_empty())
            .map(|f| f.group.as_str())
            .collect();
        flat_groups.sort_unstable();
        flat_groups.dedup();
        for g in flat_groups {
            if let Some(first) = ring
                .iter()
                .filter(|f| f.group == g)
                .min_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)))
            {
                stops.push(first.clone());
            }
        }
        stops.sort_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)));
        if stops.is_empty() {
            return;
        }
        let at = current.and_then(|c| {
            stops.iter().position(|f| f.id == c).or_else(|| {
                // Inside a flat group: its stop stands for the whole group.
                let cur_group = ring.iter().find(|f| f.id == c).map(|f| f.group.clone());
                cur_group
                    .filter(|g| !g.is_empty())
                    .and_then(|g| stops.iter().position(|f| f.group == g))
            })
        });
        let next = match at {
            Some(i) if forward => (i + 1) % stops.len(),
            Some(i) => (i + stops.len() - 1) % stops.len(),
            None => 0,
        };
        self.ui.request_focus(stops[next].id.clone());
    }

    /// The focusables the d-pad may land on RIGHT NOW — the ring of the node the cursor
    /// stands on ([`ring_for`](Self::ring_for)), so the d-pad walks WITHIN the focused
    /// pane and the stick moves between panes (Aaron 2026-09-02).
    fn ring(&self) -> Vec<Focusable> {
        let cur = self.ui.focused().map(str::to_string);
        self.ring_for(cur.as_deref())
    }

    /// The PEERS of `id` — the ring it moves in:
    /// - an INTERIOR control (its `tab_group` has a container): the members of that
    ///   group — the pane's own controls, and any subpanel containers among them;
    /// - a CONTAINER, a flat-group member, or nothing focused: the tier it sits on —
    ///   for a nested subpanel the members of ITS container's group, else the top tier:
    ///   the top-level containers (collected under the empty group, ordered by their
    ///   authored ordinals) plus every member of a container-less "flat" group (menus,
    ///   the settings rows) — so single-context surfaces navigate with no pane
    ///   ceremony.
    ///
    /// Kept off the hot path only in that it allocates a small Vec per nav press; the
    /// navigability gate + action/slider lookups deliberately stay on the FULL
    /// collections, so an empty-interior pane never de-navigates the layer.
    fn ring_for(&self, id: Option<&str>) -> Vec<Focusable> {
        if let Some(cur) = id {
            let group = self
                .focusables
                .iter()
                .find(|f| f.id == cur)
                .map(|f| f.group.clone())
                .unwrap_or_default();
            if !group.is_empty() && self.container_exists(&group) {
                return self
                    .focusables
                    .iter()
                    .filter(|f| f.group == group)
                    .cloned()
                    .collect();
            }
        }
        self.focusables
            .iter()
            .filter(|f| {
                (f.group.is_empty() && self.is_container(&f.id))
                    || (!f.group.is_empty() && !self.container_exists(&f.group))
            })
            .cloned()
            .collect()
    }

    /// Whether a container NODE for `group` is present in this frame's focusables
    /// — the flat-group rule's other half: members of a group whose container is
    /// hidden (or was never authored) navigate flat rather than being stranded.
    fn container_exists(&self, group: &str) -> bool {
        self.focusables.iter().any(|f| f.id == group)
    }

    fn action_for(&self, id: &str) -> Option<&str> {
        self.actions
            .iter()
            .find(|(fid, _)| fid == id)
            .map(|(_, a)| a.as_str())
    }

    fn text_field(&self, id: &str) -> Option<&TextFieldNav> {
        self.text_fields.iter().find(|t| t.id == id)
    }

    /// Open the text-entry session on `id`: the `TextEntry` context goes on the stack
    /// (the pump then resolves only `SubmitText`/`CancelText`, the runner allows IME,
    /// and every other key reaches the field as text) and the field takes focus.
    /// `guard`: folds that drop committed text first — the trigger key's own character
    /// when a KEY binding fired `EnterText`.
    fn enter_text(&mut self, rc: &mut RouteCtx, id: &str, guard: u8) {
        if self.ui.edit_id() == Some(id) {
            return;
        }
        let select_all = self.text_field(id).is_some_and(|t| t.select_all);
        rc.push_context(InputContext::TextEntry);
        rc.set_focus(id);
        self.ui.begin_edit(id, select_all, guard);
    }

    /// Close the open session (see [`TextExit`]). The context pops; the field's
    /// authored exit result fires into the one drain a scene reads.
    fn leave_text(&mut self, rc: &mut RouteCtx, exit: TextExit) {
        let Some(id) = self.ui.edit_id().map(str::to_string) else {
            return;
        };
        let field = self.text_field(&id).cloned();
        rc.pop_context();
        self.ui.end_edit(exit == TextExit::Cancel);
        match exit {
            TextExit::Submit => {
                if let Some(name) = field.and_then(|f| f.on_submit) {
                    self.fired.push(name);
                }
                self.ui.clear_focus();
                rc.clear_focus();
            }
            TextExit::Cancel => {
                if let Some(name) = field.and_then(|f| f.on_cancel) {
                    self.fired.push(name);
                }
                self.ui.clear_focus();
                rc.clear_focus();
            }
            TextExit::Blur => {}
        }
    }

    /// The field `EnterText` opens: the focused text field, else the screen's
    /// `default_text` field (the chat line).
    fn enter_target(&self) -> Option<String> {
        if let Some(id) = self.ui.focused().filter(|id| self.text_field(id).is_some()) {
            return Some(id.to_string());
        }
        self.text_fields
            .iter()
            .find(|t| t.default_text)
            .map(|t| t.id.clone())
    }
}

/// Collect every visible `text_field` with an id and its authored exits — walking
/// `children` like [`collect_nav`]; a hidden subtree contributes nothing.
fn collect_text_fields(node: &UiNode, model: &ValueMap, out: &mut Vec<TextFieldNav>) {
    if !visible(node, model) {
        return;
    }
    if node.component == "text_field" && !node.id.is_empty() {
        let text = |k: &str| match node.props.get(k) {
            Some(flicker_script::Value::Text(t)) if !t.is_empty() => Some(t.clone()),
            _ => None,
        };
        out.push(TextFieldNav {
            id: node.id.clone(),
            on_submit: text("submit_action"),
            on_cancel: text("cancel_action"),
            select_all: crate::config::flag(&node.props, "select_all_on_enter"),
            default_text: crate::config::flag(&node.props, "default_text"),
        });
    }
    for child in &node.children {
        collect_text_fields(child, model, out);
    }
}

/// Flatten a UI `tree` into the router's [`Focusable`] list (spec §8): every node
/// with a non-empty `tab_group` **and** a non-empty `id` becomes one focusable,
/// carrying its Lua-authored `nav_ordinal` + `tab_group`. `rect` is a placeholder —
/// this slice's nav is ordinal-primary and does not consult it (see [`nav`]).
///
/// **A HIDDEN SUBTREE IS NOT NAVIGABLE.** `model` is consulted for every node's
/// `visible_bind`, and an invisible node prunes itself AND its descendants: a
/// closed modal, a gated overlay or an off-tab page must not put anything in the
/// nav ring, or the pad walks into controls nobody can see. The model must be the
/// SAME one this frame's `run_ui` walked, so nav and draw agree on what is on
/// screen.
pub fn focusables_of(tree: &UiNode, model: &ValueMap) -> Vec<Focusable> {
    let mut focusables = Vec::new();
    let mut actions = Vec::new();
    let mut steppables = Vec::new();
    collect_nav(tree, model, &mut focusables, &mut actions, &mut steppables);
    focusables
}

/// Recursively collect the focusables (+ their `action`s, + which of them are
/// SLIDERS and their orientation) of a tree — walking `children` and template
/// `slots` so a slot-authored button is navigable too. Invisible nodes prune
/// their whole subtree (see [`focusables_of`]).
fn collect_nav(
    node: &UiNode,
    model: &ValueMap,
    focusables: &mut Vec<Focusable>,
    actions: &mut Vec<(String, String)>,
    steppables: &mut Vec<(String, StepKind)>,
) {
    if !visible(node, model) {
        return;
    }
    if !node.tab_group.is_empty() && !node.id.is_empty() {
        focusables.push(Focusable {
            id: node.id.clone(),
            group: node.tab_group.clone(),
            ordinal: node.nav_ordinal,
            rect: [0.0; 4],
        });
        if let Some(action) = &node.action {
            actions.push((node.id.clone(), action.clone()));
        }
        // Every focusable value control is STEPPABLE by the pad on its own axis (the d-pad
        // operates it; a rail/tab strip carries no bind here and is driven by the shoulders).
        let kind = match node.component.as_str() {
            "slider" if crate::config::flag(&node.props, "vertical") => Some(StepKind::SliderV),
            "slider" => Some(StepKind::SliderH),
            "select" | "pill_toggle" => Some(StepKind::Options),
            "toggle" => Some(StepKind::Toggle),
            _ => None,
        };
        if let Some(kind) = kind {
            steppables.push((node.id.clone(), kind));
        }
    }
    for child in &node.children {
        collect_nav(child, model, focusables, actions, steppables);
    }
}

/// The OWNERSHIP pass (nested panes): collect the container nodes the member pass
/// could not — visible nodes with an id but NO `tab_group` of their own that are
/// either claimed by members or authored `pane: true`. They join the focusable
/// list under the EMPTY group (the top-tier ring the left stick cycles), carrying
/// their authored `nav_ordinal` as the explicit stick-stop order. Also records
/// every `pane: true` id at ANY depth, so `is_container` covers viewport-only
/// subpanels the same test covers claimed ones.
fn collect_containers(
    node: &UiNode,
    model: &ValueMap,
    claimed: &std::collections::HashSet<String>,
    focusables: &mut Vec<Focusable>,
    pane_flagged: &mut std::collections::HashSet<String>,
) {
    if !visible(node, model) {
        return;
    }
    if !node.id.is_empty() {
        let flagged = crate::config::flag(&node.props, "pane");
        if flagged {
            pane_flagged.insert(node.id.clone());
        }
        if node.tab_group.is_empty() && (flagged || claimed.contains(&node.id)) {
            focusables.push(Focusable {
                id: node.id.clone(),
                group: String::new(),
                ordinal: node.nav_ordinal,
                rect: [0.0; 4],
            });
        }
    }
    for child in &node.children {
        collect_containers(child, model, claimed, focusables, pane_flagged);
    }
}

/// The signals THIS LAYER owns while it holds a focusable tree — the ones whose
/// meaning is the same on every screen in Prism, so no screen may name them
/// (see [`WalkerHandler::with_intents`]). The bumpers (`TabNext`/`TabPrev`) are
/// deliberately absent: they belong to the page/tab control's own rail.
pub fn walker_owned(signal: ActionSignal) -> bool {
    is_nav_signal(signal) || signal == ActionSignal::ChordBegin || is_text_signal(signal)
}

/// The text-entry family — the ONE switch in (`EnterText`) and the two exits the
/// `TextEntry` map binds. Owned by the walker: it holds the session on the field.
fn is_text_signal(signal: ActionSignal) -> bool {
    matches!(
        signal,
        ActionSignal::EnterText | ActionSignal::SubmitText | ActionSignal::CancelText
    )
}

/// The directional-nav signals the walker consumes while it owns a focusable tree.
fn is_nav_signal(signal: ActionSignal) -> bool {
    matches!(
        signal,
        ActionSignal::NavUp
            | ActionSignal::NavDown
            | ActionSignal::NavLeft
            | ActionSignal::NavRight
            | ActionSignal::PanelNext
            | ActionSignal::PanelPrev
            | ActionSignal::Confirm
            | ActionSignal::Cancel
    )
}

/// Signals driven by the pointer — the ones a `hud_hit` should swallow so the
/// scene behind a panel does not also act on the click.
fn is_pointer_signal(signal: ActionSignal) -> bool {
    matches!(
        signal,
        ActionSignal::PrimaryAction | ActionSignal::SecondaryAction
    )
}

impl InputHandler for WalkerHandler<'_> {
    /// The walker's SUBSCRIPTION (MCP `67DEE93A` / `2A221E4A`): it owns the nav
    /// FAMILY (move focus / activate / cancel / panel + the chord modifier it
    /// observes), the POINTER signals (its `hud_hit` gate over UI), and any signal
    /// the screen DECLARED an intent for. Everything else — a scene's orchestration
    /// signals, gameplay actions — passes straight through to the layers below. The
    /// walker is the SECONDARY (focus) context, NOT an eat-everything layer; the
    /// dispatcher now enforces that by never even offering it a signal it does not
    /// own here.
    fn subscribes(&self, signal: ActionSignal) -> bool {
        walker_owned(signal)
            || is_pointer_signal(signal)
            || self.intents.is_some_and(|i| i.result_for(signal).is_some())
    }

    /// The per-frame reconcile (before any event): the route's text reaches the open
    /// session, and a text-entry session follows the FOCUS. A click that just landed in a text field (`run_ui` claimed focus and
    /// noted `click_focus`) ENTERS — a mouse click into the field is one of the three
    /// ways in (Aaron 2026-09-03); a focus that has left the field (a click elsewhere,
    /// a nav move) LEAVES with the value standing.
    fn frame(&mut self, rc: &mut RouteCtx) {
        // A pane move the Apply / Revert prompt released: complete it now — focus the
        // target and descend into it, exactly as the parked stick press would have.
        if let Some(target) = self.ui.take_resume_pane_move() {
            self.ui.note_nav_input();
            self.ui.request_focus(target);
            self.descend();
            self.sync_pane();
        }
        if let Some(id) = self.ui.edit_id().map(str::to_string) {
            if self.ui.focused() != Some(id.as_str()) {
                self.leave_text(rc, TextExit::Blur);
            }
        }
        // The route's TEXT: the pump read the keyboard because the context is TextEntry,
        // and this layer holds the session it was read for — queue it for the next fold.
        // Nothing else ever carries it (no scene, no snapshot field).
        if !rc.text.is_empty() && self.ui.text_entry() {
            self.ui.push_text(std::mem::take(&mut rc.text));
        }
        if std::mem::take(&mut self.ui.enter_pending) {
            if let Some(id) = self
                .ui
                .focused()
                .filter(|id| self.text_field(id).is_some())
                .map(str::to_string)
            {
                self.enter_text(rc, &id, 0);
            }
        }
    }

    fn handle(&mut self, ev: &InputEvent, rc: &mut RouteCtx) -> Flow {
        // The chord modifier is OBSERVED, never consumed: the walker tracks
        // held-ness (it scales a slider nudge to the coarse step) and passes
        // the event on, so the chord layer below keeps every verb it owns.
        if ev.signal == ActionSignal::ChordBegin {
            match ev.kind {
                EventKind::Press => self.ui.chord = true,
                EventKind::Release => self.ui.chord = false,
                _ => {}
            }
            return Flow::Pass;
        }
        // The typed form of the old `hud_hit` / `chat_hit` gate: when the pointer
        // is over UI, the walker consumes the click so the gameplay base handler
        // (last in the chain) never world-picks through the panel.
        if self.consumed_pointer && is_pointer_signal(ev.signal) {
            return Flow::Consumed;
        }
        // THE DISMISSABLE TOGGLE (ruling DA0E1B57), on the ONE Cancel path: while the
        // screen's topmost modal slab reads NOT dismissable, this layer eats Cancel on
        // both edges and produces NOTHING — the declared `on_cancel` below does not
        // fire, the nav back-out in `act` does not run, and nothing leaks to the layers
        // beneath. Only a layer that WOULD have answered Cancel swallows it, so a
        // pass-through HUD stays a pass-through. The exit still exists (the host injects
        // it); the component decides when it is allowed — and it says yes by default, so
        // no modal traps unless something is actively holding the player (B89FAC21).
        if ev.signal == ActionSignal::Cancel && !self.dismissable && self.owns_cancel() {
            return Flow::Consumed;
        }
        // Declarative intents (S9): a signal the screen ROOT bound (`on_<signal>`)
        // fires its declared result name on the Press and is consumed on BOTH
        // edges (like nav — neither edge leaks below). Declared bindings beat the
        // nav defaults below: the screen owns what its bound signal means. The
        // fire is also the press-feedback cue: `take_fired` lights the result
        // name's flash, and any button whose `action` is that name glows — even
        // when the action itself wrapped to nowhere (Aaron, 2026-08-08: the
        // visible response is the point).
        if let Some(name) = self.intents.and_then(|i| i.result_for(ev.signal)) {
            if ev.kind == EventKind::Press {
                self.fired.push(name.to_string());
            }
            return Flow::Consumed;
        }
        // Text entry (Aaron 2026-09-03): `EnterText` is THE switch into the session — a
        // bound key/chord targets the focused text field, else the screen's default one;
        // `SubmitText` / `CancelText` are the only signals the TextEntry map resolves,
        // and they close it. A key binding commits its own character too, so the entry
        // guard drops the next two folds' text (macOS lands it a frame late).
        if is_text_signal(ev.signal) {
            match ev.signal {
                ActionSignal::EnterText => {
                    let Some(id) = self.enter_target() else {
                        return Flow::Pass;
                    };
                    if ev.kind == EventKind::Press {
                        self.ui.note_nav_input();
                        self.enter_text(rc, &id, 2);
                        self.sync_pane();
                    }
                    return Flow::Consumed;
                }
                ActionSignal::SubmitText | ActionSignal::CancelText => {
                    if !self.ui.text_entry() {
                        return Flow::Pass;
                    }
                    if ev.kind == EventKind::Press {
                        let exit = if ev.signal == ActionSignal::SubmitText {
                            TextExit::Submit
                        } else {
                            TextExit::Cancel
                        };
                        self.leave_text(rc, exit);
                        self.sync_pane();
                    }
                    return Flow::Consumed;
                }
                _ => {}
            }
        }
        // Directional nav — only while this walker layer is navigable (owns a
        // focusable tree). When it is not, these signals pass through so the
        // gameplay base handler still sees them.
        if is_nav_signal(ev.signal) && !self.focusables.is_empty() {
            // The resolver emits BOTH a press and a release for every edge; act on
            // the press so one d-pad tap is one step, but consume both so neither
            // edge leaks to gameplay (spec §8).
            if ev.kind == EventKind::Press {
                self.act(ev.signal, rc);
            }
            return Flow::Consumed;
        }
        Flow::Pass
    }
}

#[cfg(test)]
mod tests {
    use flicker_input_core::{EventKind, InputContext, InputState};
    use flicker_input_router::RouteCtx;

    use super::*;

    /// An empty model: nothing is gated, so every node is visible. Nav filters on
    /// visibility now, so a test tree must be collected against some model.
    fn shown() -> ValueMap {
        ValueMap::new()
    }

    fn press<'a>(signal: ActionSignal, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, EventKind::Press, InputContext::World, raw)
    }

    fn release<'a>(signal: ActionSignal, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, EventKind::Release, InputContext::World, raw)
    }

    fn button(id: &str, group: &str, ordinal: u32, action: &str) -> UiNode {
        UiNode {
            id: id.into(),
            component: "button".into(),
            action: Some(action.into()),
            tab_group: group.into(),
            nav_ordinal: ordinal,
            ..Default::default()
        }
    }

    /// A menu tree: a column of buttons, some in group "menu", some in "scenes".
    fn menu_tree() -> UiNode {
        UiNode {
            id: "root".into(),
            component: "column".into(),
            children: vec![
                button("start", "menu", 0, "start"),
                button("settings", "menu", 1, "settings"),
                button("quit", "menu", 2, "quit"),
                button("load_a", "scenes", 0, "scene_a"),
                button("load_b", "scenes", 1, "scene_b"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn consumes_pointer_only_when_hud_hit() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();

        // hud_hit → the click is consumed; a non-pointer signal still passes.
        let mut h = WalkerHandler::hud(&mut ui, true);
        assert_eq!(
            h.handle(&press(ActionSignal::PrimaryAction, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(
            h.handle(&press(ActionSignal::Jump, &raw), &mut rc),
            Flow::Pass
        );
    }

    #[test]
    fn passes_pointer_when_no_hud_hit() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false);
        assert_eq!(
            h.handle(&press(ActionSignal::PrimaryAction, &raw), &mut rc),
            Flow::Pass
        );
    }

    /// **A fired intent lights its RESULT name's flash; the fade runs on the
    /// take_fired clock.** The press fires the result and `take_fired` lights
    /// that name at full intensity — so a BUTTON whose `action` is the same
    /// name glows exactly as if it had been clicked; the release edge is merely
    /// consumed. Each subsequent `take_fired` decays the flash until it
    /// expires. This is the whole press-feedback contract, keyed by the ONE
    /// name every activation route shares.
    #[test]
    fn a_fired_intent_lights_its_results_flash_and_fades() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut intents_tree = UiNode {
            component: "surface".into(),
            id: "root".into(),
            ..Default::default()
        };
        intents_tree.props.insert(
            "on_page_next".into(),
            flicker_script::Value::Text("page_flip".into()),
        );
        let intents = UiIntents::of(&intents_tree);

        let mut h = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        assert_eq!(
            h.handle(&press(ActionSignal::PageNext, &raw), &mut rc),
            Flow::Consumed
        );
        let fired = h.take_fired();
        assert_eq!(
            fired,
            vec!["page_flip".to_string()],
            "the press fires the result"
        );
        assert_eq!(
            h.ui.flash_intensity("page_flip"),
            1.0,
            "the fire lights the RESULT name's flash — the button's action key"
        );

        assert_eq!(
            h.handle(&release(ActionSignal::PageNext, &raw), &mut rc),
            Flow::Consumed,
            "the release edge is consumed so it never leaks below"
        );

        let mut last = 1.0;
        let mut frames = 0;
        while h.ui.flash_intensity("page_flip") > 0.0 {
            h.take_fired();
            let now = h.ui.flash_intensity("page_flip");
            assert!(now < last, "the flash fades monotonically");
            last = now;
            frames += 1;
            assert!(frames < 120, "the flash must expire, not linger forever");
        }
        assert!(
            frames >= 5,
            "\"briefly\" is a fade, not a single-frame blink"
        );
        // An undeclared signal fires nothing and lights nothing.
        assert_eq!(
            h.handle(&press(ActionSignal::TabNext, &raw), &mut rc),
            Flow::Pass
        );
        assert!(h.take_fired().is_empty());
        assert_eq!(h.ui.flash_intensity("tab_next"), 0.0);
    }

    /// Two panes for the staging contract (Aaron 2026-09-04): `a` holds a dial that
    /// stages on Confirm plus a button, `b` holds a button; stick order a → b.
    fn staged_panes() -> UiNode {
        use flicker_script::Value;
        let mut dial = UiNode {
            component: "slider".into(),
            id: "dial".into(),
            ..Default::default()
        };
        dial.bind = Some("v".into());
        dial.tab_group = "a".into();
        dial.nav_ordinal = 0;
        dial.size = Some(24.0);
        dial.props.insert("min".into(), Value::Number(0.0));
        dial.props.insert("max".into(), Value::Number(100.0));
        dial.props
            .insert("apply".into(), Value::Text("confirm".into()));
        let mut go = button("go", "a", 1, "go");
        go.size = Some(24.0);
        let mut a = UiNode {
            component: "cell".into(),
            id: "a".into(),
            nav_ordinal: 1,
            ..Default::default()
        };
        a.children = vec![dial, go];
        let mut bgo = button("bgo", "b", 0, "bgo");
        bgo.size = Some(24.0);
        let mut b = UiNode {
            component: "cell".into(),
            id: "b".into(),
            nav_ordinal: 2,
            ..Default::default()
        };
        b.children = vec![bgo];
        let mut col = UiNode {
            component: "cell".into(),
            ..Default::default()
        };
        col.anchor = Some(flicker_script::UiAnchor::TopLeft);
        col.width = Some(200.0);
        col.children = vec![a, b];
        let mut tree = UiNode {
            component: "surface".into(),
            id: "root".into(),
            ..Default::default()
        };
        tree.children.push(col);
        tree
    }

    fn idle_input() -> crate::UiInput {
        use flicker_render::Vec2;
        crate::UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(800.0, 600.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        }
    }

    /// **CONFIRM = APPLY, then FIRE** (Aaron 2026-09-04): a Confirm anywhere in a pane
    /// commits the values its dials STAGED; on a focused BUTTON the activation waits
    /// for the pass that lands the commit, so the scene folds the values before the
    /// action — one press, the right order.
    #[test]
    fn confirm_commits_the_panes_stages_then_fires_the_button() {
        let raw = InputState::new();
        let tree = staged_panes();
        let mut model = ValueMap::new();
        model.set("v", 50.0);
        let styles = serde_json::json!({});
        let idle = idle_input();
        let mut ui = UiState::new();
        ui.request_focus("dial");
        let mut rc = RouteCtx::new();

        // A step on the dial is staged, not written.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(
                h.handle(&press(ActionSignal::NavRight, &raw), &mut rc),
                Flow::Consumed
            );
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(
            frame.results.number("v"),
            Some(50.0),
            "the scene keeps seeing the resting value"
        );
        assert!(ui.staged_any(), "the stage is held");

        // Down walks to the pane's button; Confirm there commits, THEN fires.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
            assert_eq!(h.ui.focused(), Some("go"));
            assert_eq!(
                h.handle(&press(ActionSignal::Confirm, &raw), &mut rc),
                Flow::Consumed
            );
            assert!(
                h.take_fired().is_empty(),
                "the activation waits behind the commit"
            );
            assert!(!h.ui.staged_any(), "the stage is queued for commit");
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(frame.results.number("v"), Some(51.0), "the commit lands");
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(
                h.take_fired(),
                vec!["go".to_string()],
                "…and the button fires in that same pass"
            );
        }
    }

    /// **A pane holding stages is never left silently** (Aaron 2026-09-04): the stick's
    /// move is PARKED, the scene is handed exactly one prompt, and its answer completes
    /// the move on the walker's next frame — Apply commits first, Revert drops the
    /// stage — while Keep-editing leaves the cursor and the stage where they were.
    #[test]
    fn the_stick_asks_before_leaving_a_pane_with_stages() {
        let raw = InputState::new();
        let tree = staged_panes();
        let mut model = ValueMap::new();
        model.set("v", 50.0);
        let styles = serde_json::json!({});
        let idle = idle_input();
        let mut rc = RouteCtx::new();

        let stage_then_stick = |ui: &mut UiState| {
            ui.request_focus("dial");
            {
                let mut h = WalkerHandler::hud(ui, false).with_nav(&tree, &model);
                h.handle(&press(ActionSignal::NavRight, &raw), &mut RouteCtx::new());
            }
            crate::run_ui(&tree, &model, &styles, &idle, ui);
            assert!(ui.staged_any());
            let mut h = WalkerHandler::hud(ui, false).with_nav(&tree, &model);
            assert_eq!(
                h.handle(&press(ActionSignal::PanelNext, &raw), &mut RouteCtx::new()),
                Flow::Consumed
            );
            assert_eq!(
                h.ui.focused(),
                Some("dial"),
                "the move is parked, not made"
            );
        };

        // Keep editing: the cursor stays, the stage stands, the move is forgotten.
        let mut ui = UiState::new();
        stage_then_stick(&mut ui);
        assert!(ui.take_stage_prompt(), "one prompt for the scene");
        assert!(!ui.take_stage_prompt(), "…exactly one");
        ui.keep_stages();
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.frame(&mut rc);
            assert_eq!(h.ui.focused(), Some("dial"));
            assert!(h.ui.staged_any(), "the stage still stands");
        }

        // Apply: the commit lands and the cursor lands in pane b.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        }
        assert!(ui.take_stage_prompt());
        ui.apply_stages();
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.frame(&mut rc);
            assert_eq!(h.ui.focused(), Some("bgo"), "the parked move completes");
            assert_eq!(h.ui.focused_pane(), Some("b"));
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(frame.results.number("v"), Some(51.0), "applied");

        // Revert: the stage is dropped, the move completes, the model stands.
        let mut ui = UiState::new();
        stage_then_stick(&mut ui);
        assert!(ui.take_stage_prompt());
        ui.revert_stages();
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.frame(&mut rc);
            assert_eq!(h.ui.focused(), Some("bgo"));
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert!(!ui.staged_any());
        assert_eq!(frame.results.number("v"), Some(50.0), "reverted to the model");
    }

    /// **Every slider owns the pad channel** — the component-level contract,
    /// asserted generically: nav along the FOCUSED slider's own axis nudges
    /// its bound value by the NODE's `step` (`step_coarse` while the chord
    /// modifier is held — observed, never consumed), the write clamps to the
    /// node's range, and the cross axis still moves focus so a slider is
    /// never a trap. No scene declares anything for any of this.
    #[test]
    fn a_focused_slider_steps_on_its_axis_and_chord_scales_the_step() {
        use flicker_render::Vec2;
        use flicker_script::Value;

        let raw = InputState::new();
        let mut vdial = UiNode {
            component: "slider".into(),
            id: "vdial".into(),
            ..Default::default()
        };
        vdial.bind = Some("v".into());
        vdial.tab_group = "g".into();
        vdial.nav_ordinal = 0;
        vdial.size = Some(120.0);
        vdial.props.insert("vertical".into(), Value::Bool(true));
        vdial.props.insert("min".into(), Value::Number(0.0));
        vdial.props.insert("max".into(), Value::Number(100.0));
        let mut hdial = UiNode {
            component: "slider".into(),
            id: "hdial".into(),
            ..Default::default()
        };
        hdial.bind = Some("h".into());
        hdial.tab_group = "g".into();
        hdial.nav_ordinal = 1;
        hdial.size = Some(24.0);
        hdial.props.insert("min".into(), Value::Number(0.0));
        hdial.props.insert("max".into(), Value::Number(10.0));
        let mut col = UiNode {
            component: "cell".into(),
            ..Default::default()
        };
        col.anchor = Some(flicker_script::UiAnchor::TopLeft);
        col.width = Some(200.0);
        col.children = vec![vdial, hdial];
        let mut tree = UiNode {
            component: "surface".into(),
            id: "root".into(),
            ..Default::default()
        };
        tree.children.push(col);

        let mut model = ValueMap::new();
        model.set("v", 50.0);
        model.set("h", 5.0);
        let styles = serde_json::json!({});
        let idle = crate::UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(800.0, 600.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let mut ui = UiState::new();
        ui.request_focus("vdial");
        let mut rc = RouteCtx::new();

        // Vertical: Up steps toward max by the default step of 1.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(
                h.handle(&press(ActionSignal::NavUp, &raw), &mut rc),
                Flow::Consumed
            );
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(
            frame.results.number("v"),
            Some(51.0),
            "NavUp steps the vertical dial"
        );

        // Chord held scales to the coarse step — and the modifier itself is
        // OBSERVED (Flow::Pass), so the chord layer below still sees it.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(
                h.handle(&press(ActionSignal::ChordBegin, &raw), &mut rc),
                Flow::Pass
            );
            assert_eq!(
                h.handle(&press(ActionSignal::NavDown, &raw), &mut rc),
                Flow::Consumed
            );
            assert_eq!(
                h.handle(&release(ActionSignal::ChordBegin, &raw), &mut rc),
                Flow::Pass
            );
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(
            frame.results.number("v"),
            Some(40.0),
            "chord + NavDown steps by the coarse default (step × 10)"
        );

        // The cross axis is never captured: Left moves focus off the dial.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(
                h.handle(&press(ActionSignal::NavLeft, &raw), &mut rc),
                Flow::Consumed
            );
        }
        assert_eq!(ui.focused(), Some("hdial"), "the cross axis navigates away");

        // Horizontal: Right steps toward max; the write clamps to the range.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            for _ in 0..8 {
                assert_eq!(
                    h.handle(&press(ActionSignal::NavRight, &raw), &mut rc),
                    Flow::Consumed
                );
            }
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(
            frame.results.number("h"),
            Some(10.0),
            "eight steps from 5 clamp at the horizontal dial's max"
        );
    }

    #[test]
    fn apply_focus_writes_through_the_walker() {
        let mut ui = UiState::new();
        {
            let mut h = WalkerHandler::hud(&mut ui, false);
            h.apply_focus(Some(FocusChange::Set("chat_input".into())));
        }
        assert_eq!(ui.focused(), Some("chat_input"));
        {
            let mut h = WalkerHandler::hud(&mut ui, false);
            h.apply_focus(Some(FocusChange::Clear));
        }
        assert_eq!(ui.focused(), None);
    }

    /// A HIDDEN SUBTREE IS NOT NAVIGABLE. Before this, focusables were collected
    /// with no model at all, so a closed modal's buttons sat in the nav ring and
    /// the pad could walk into controls nobody could see — the reason a bench
    /// could not put `tab_group` on an overlay at all.
    #[test]
    fn a_hidden_subtree_contributes_no_focusables() {
        // A visible screen, plus an overlay gated by `dialog_open`.
        let mut screen = UiNode {
            component: "surface".into(),
            ..Default::default()
        };
        let mut bench = UiNode {
            component: "button".into(),
            ..Default::default()
        };
        bench.id = "bench_btn".into();
        bench.tab_group = "bench".into();

        let mut overlay = UiNode {
            component: "cell".into(),
            ..Default::default()
        };
        overlay.visible_bind = Some("dialog_open".into());
        let mut confirm = UiNode {
            component: "button".into(),
            ..Default::default()
        };
        confirm.id = "dialog_confirm".into();
        confirm.tab_group = "dialog".into();
        overlay.children = vec![confirm];
        screen.children = vec![bench, overlay];

        // Closed: the overlay's button is NOT navigable.
        let closed = focusables_of(&screen, &shown());
        let ids: Vec<_> = closed.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["bench_btn"],
            "a closed overlay is out of the ring: {ids:?}"
        );

        // Open: it joins the ring, so a modal CAN carry focusable controls.
        let mut open = ValueMap::new();
        open.set("dialog_open", true);
        let shown_ids: Vec<_> = focusables_of(&screen, &open)
            .iter()
            .map(|f| f.id.clone())
            .collect();
        assert_eq!(
            shown_ids,
            vec!["bench_btn", "dialog_confirm"],
            "{shown_ids:?}"
        );
    }

    #[test]
    fn focusables_of_flattens_only_tab_group_nodes() {
        let items = focusables_of(&menu_tree(), &shown());
        // 5 buttons carry a tab_group; the root column (no tab_group) is skipped.
        assert_eq!(items.len(), 5);
        assert!(items
            .iter()
            .all(|f| f.group == "menu" || f.group == "scenes"));
        assert!(items.iter().any(|f| f.id == "start" && f.ordinal == 0));
    }

    #[test]
    fn nav_signal_restores_nav_modality() {
        // The pointer had taken the modality over; the first routed nav signal
        // hands it back, so the focused node lights again.
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.pointer_mode = true;
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert!(h.ui.nav_mode(), "a routed nav press restores nav modality");
    }

    #[test]
    fn nav_moves_focus_within_group_and_is_consumed() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        // No current focus: NavDown enters the list at the lowest ordinal.
        assert_eq!(
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(h.ui.focused(), Some("start"));
        // NavDown again steps by ordinal within the group.
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("settings"));
        // The matching RELEASE is consumed too, but does NOT move focus again.
        assert_eq!(
            h.handle(&release(ActionSignal::NavDown, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(h.ui.focused(), Some("settings"));
        // NavUp steps back.
        h.handle(&press(ActionSignal::NavUp, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("start"));
    }

    /// **The LEFT STICK cycles PANELS, and it wraps.** Three panes, and `PanelNext`
    /// four times returns to where it started — each landing DESCENDS into the pane's
    /// lowest-ordinal control, and the pane it belongs to is the IMPLIED context
    /// (`focused_pane`). This is the whole of "which pane has the cursor": no scene
    /// enum, no rim style, no enter/exit mode (Aaron 2026-09-02).
    #[test]
    fn the_left_stick_cycles_panels_and_wraps() {
        let raw = InputState::new();
        // Three PANELS, each with its own interior control. A container carries NO
        // `tab_group` of its own — its members CLAIM it — and its `nav_ordinal` is
        // the AUTHORED stick-stop order (Aaron 2026-08-15).
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        for (i, group) in ["pop_left", "pop_view", "pop_right"].iter().enumerate() {
            let mut pane = UiNode {
                id: (*group).into(),
                component: "panel".into(),
                nav_ordinal: i as u32 + 1,
                ..Default::default()
            };
            pane.children
                .push(button(&format!("ctl_{i}"), group, 1, "act"));
            tree.children.push(pane);
        }
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        for (want, ctl) in [
            ("pop_left", "ctl_0"),
            ("pop_view", "ctl_1"),
            ("pop_right", "ctl_2"),
            ("pop_left", "ctl_0"),
        ] {
            assert_eq!(
                h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc),
                Flow::Consumed,
                "the panel tier is the walker's"
            );
            assert_eq!(
                h.ui.focused_pane(),
                Some(want),
                "the landing pane is the implied context"
            );
            assert_eq!(
                h.ui.focused(),
                Some(ctl),
                "…and the cursor descends onto its lowest-ordinal control"
            );
        }
        // …and backwards, wrapping the other way.
        h.handle(&press(ActionSignal::PanelPrev, &raw), &mut rc);
        assert_eq!(h.ui.focused_pane(), Some("pop_right"));
    }

    /// **The IMPLIED PANEL CONTEXT (Aaron 2026-09-02).** Navigate to an interior-less
    /// pane (a viewport): it is the focused pane the moment the stick lands on it — no
    /// `Confirm` to enter, no lock, and `PanelNext` is never gated. `Confirm` on the
    /// actionless pane does nothing; `Cancel` is ALWAYS scene-level.
    #[test]
    fn the_focused_pane_is_implied_confirm_never_enters_and_cancel_is_scene_level() {
        let raw = InputState::new();
        // Two actionless, interior-less PANE containers — nothing claims them, so
        // each authors the explicit `pane: true` marker (the viewport-pane form),
        // with the stick-stop order authored on their ordinals.
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        for (i, group) in ["pop_left", "pop_view"].iter().enumerate() {
            let mut pane = UiNode {
                id: (*group).into(),
                component: "panel".into(),
                nav_ordinal: i as u32 + 1,
                ..Default::default()
            };
            pane.props
                .insert("pane".into(), flicker_script::Value::Bool(true));
            tree.children.push(pane);
        }
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("pop_left"));
        assert_eq!(
            h.ui.focused_pane(),
            Some("pop_left"),
            "an interior-less pane IS the focused pane the moment the stick lands on it"
        );

        // Confirm on the actionless pane is a no-op — there is nothing to enter.
        h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("pop_left"));
        assert_eq!(h.ui.focused_pane(), Some("pop_left"));
        assert!(h.take_fired().is_empty(), "an actionless pane fires nothing");

        // PanelNext is never gated — the stick always switches panes.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(
            h.ui.focused_pane(),
            Some("pop_view"),
            "the stick switches the implied pane freely"
        );

        // Cancel is SCENE-LEVEL, always: it pops the scene's context, never a pane.
        assert_eq!(
            h.handle(&press(ActionSignal::Cancel, &raw), &mut rc),
            Flow::Consumed
        );
        assert!(h.cancelled(), "Cancel is scene-level under the implied context");
        assert_eq!(
            h.ui.focused_pane(),
            Some("pop_view"),
            "…and the focused pane is untouched by it"
        );
    }

    /// **THE STICK HOPS FLAT GROUPS TOO** — the main-menu regression pin
    /// (2026-08-15): a menu is flat groups with NO containers (the mode rail, the
    /// scene list), and the stick must hop between them exactly as it cycles a
    /// bench's panes; standing anywhere inside a group counts as standing on its
    /// stop. Without this the pad could never reach the scene list to launch.
    #[test]
    fn the_stick_hops_between_flat_groups_with_no_containers() {
        let raw = InputState::new();
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        // Two flat groups, menu-shaped: a mode rail and a scene list. No node has
        // a claimed id, so there are no containers anywhere.
        for (i, id) in ["explore", "build", "developer"].iter().enumerate() {
            tree.children.push(button(id, "menu", i as u32, id));
        }
        for (i, id) in ["populous", "sablework"].iter().enumerate() {
            tree.children.push(button(id, "scenes", i as u32, id));
        }
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        // First press lands on the first stop; d-pad walks WITHIN the group.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("explore"));
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("build"),
            "d-pad stays inside the flat group"
        );

        // The stick hops to the OTHER group — even from mid-group.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("populous"),
            "stick hops to the scene list"
        );
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("sablework"),
            "…whose rows the d-pad walks"
        );

        // And back, wrapping.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("explore"), "the stop ring wraps");
        h.handle(&press(ActionSignal::PanelPrev, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("populous"), "…both ways");
    }

    /// **THE D-PAD MOVES INTERIOR-LESS PANES GEOMETRICALLY** (plan 1A292918 T2/T3,
    /// kept under the implied context for panes the cursor can REST on — viewports).
    /// With this frame's rects, a direction lands on the stop that actually sits that
    /// way on screen — Left from the centre view → the card beside it; Down walks the
    /// stacked cards; Right returns to the view — and there is NO WRAP at the surface
    /// edge.
    #[test]
    fn the_flattened_top_tier_moves_geometrically_with_rects() {
        let raw = InputState::new();
        // Three stacked "cards" on the left + a tall "view" on the right, each an
        // interior-less `pane: true` container (a viewport-style pane the cursor rests
        // on). Geometry (not ordinal) decides adjacency.
        let mk = |id: &str, ord: u32| {
            let mut c = UiNode {
                id: id.into(),
                component: "panel".into(),
                nav_ordinal: ord,
                ..Default::default()
            };
            c.props
                .insert("pane".into(), flicker_script::Value::Bool(true));
            c
        };
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        tree.children.push(mk("voice_1", 2));
        tree.children.push(mk("voice_2", 3));
        tree.children.push(mk("voice_3", 4));
        tree.children.push(mk("view", 8));

        let rects = vec![
            ("voice_1".to_string(), [0.0, 0.0, 100.0, 40.0]),
            ("voice_2".to_string(), [0.0, 50.0, 100.0, 40.0]),
            ("voice_3".to_string(), [0.0, 100.0, 100.0, 40.0]),
            ("view".to_string(), [150.0, 0.0, 120.0, 140.0]),
        ];
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &shown())
            .with_rects(&rects);

        h.ui.request_focus("view".to_string());
        // Left from the centre lands on the top-left card (banded; ties → lowest ordinal).
        h.handle(&press(ActionSignal::NavLeft, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("voice_1"),
            "Left from the view → the adjacent card"
        );
        // Down walks the stacked cards.
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("voice_2"));
        // Right returns to the view beside them.
        h.handle(&press(ActionSignal::NavRight, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("view"), "Right from a card → the view");
        // No wrap: nothing above voice_1.
        h.ui.request_focus("voice_1".to_string());
        h.handle(&press(ActionSignal::NavUp, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("voice_1"),
            "nothing above voice_1 → no move (the tier does not wrap)"
        );
    }

    /// The STICK's `Panel*` is a SEPARATE intent from the d-pad's `Nav*` (Aaron
    /// 2026-08-18). It steps the cursor's PANE among its peers in AUTHORED ordinal
    /// order (wrapping) — never geometrically, since Left/Right alone could not reach a
    /// stacked pane — and descends into the landing pane (Aaron 2026-09-02).
    #[test]
    fn the_stick_pane_intent_moves_the_tier_in_authored_order() {
        let raw = InputState::new();
        let mk = |id: &str, ord: u32| {
            let mut c = UiNode {
                id: id.into(),
                component: "panel".into(),
                nav_ordinal: ord,
                ..Default::default()
            };
            c.children
                .push(button(&format!("{id}_c"), id, 1, &format!("{id}_c")));
            c
        };
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        tree.children.push(mk("voice_1", 2));
        tree.children.push(mk("voice_2", 3));
        tree.children.push(mk("view", 8));
        let rects = vec![
            ("voice_1".to_string(), [0.0, 0.0, 100.0, 40.0]),
            ("voice_2".to_string(), [0.0, 50.0, 100.0, 40.0]),
            ("view".to_string(), [150.0, 0.0, 120.0, 140.0]),
        ];
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &shown())
            .with_rects(&rects);

        // The cursor stands INSIDE the view pane (on its control) — the stick resolves
        // the control to its pane and moves the PANE. Authored order: voice_1 (2),
        // voice_2 (3), view (8) — PanelPrev from the view lands on voice_2, the
        // geometrically-adjacent voice_1 notwithstanding.
        h.ui.request_focus("view_c".to_string());
        h.handle(&press(ActionSignal::PanelPrev, &raw), &mut rc);
        assert_eq!(
            h.ui.focused_pane(),
            Some("voice_2"),
            "the stick steps panes in authored order, not by geometry"
        );
        assert_eq!(h.ui.focused(), Some("voice_2_c"), "…descending into the pane");
        // Stick-right (PanelNext) → back to the view.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused_pane(), Some("view"), "…and back, both ways");
        assert_eq!(h.ui.focused(), Some("view_c"));
        // …and PanelNext past the last pane WRAPS to the first — every pane is
        // reachable by the stick alone.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused_pane(), Some("voice_1"), "wraps in authored order");
    }

    /// **Authored subpanels under the implied context.** A rack of subpanels: the stick
    /// lands on the rack and DESCENDS all the way to the first subpanel's first control;
    /// the d-pad walks that subpanel's controls; the stick then moves between the
    /// SUBPANELS (the pane the cursor is in steps among its peers); Cancel never pops
    /// a level — it is scene-level (Aaron 2026-09-02).
    #[test]
    fn authored_subpanels_still_nest_one_level() {
        let raw = InputState::new();
        let mut rack = UiNode {
            id: "rack".into(),
            component: "panel".into(),
            nav_ordinal: 1,
            ..Default::default()
        };
        for n in 1..=2u32 {
            let mut row = UiNode {
                id: format!("voice_{n}"),
                component: "stack".into(),
                tab_group: "rack".into(),
                nav_ordinal: n,
                ..Default::default()
            };
            row.children.push(button(
                &format!("v{n}_a"),
                &format!("voice_{n}"),
                1,
                "act_a",
            ));
            row.children.push(button(
                &format!("v{n}_b"),
                &format!("voice_{n}"),
                2,
                "act_b",
            ));
            rack.children.push(row);
        }
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        tree.children.push(rack);

        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        // The stick acquires the rack and descends: rack → voice_1 → v1_a.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("v1_a"), "descends to the first control");
        assert_eq!(
            h.ui.focused_pane(),
            Some("voice_1"),
            "the implied pane is the innermost container the cursor is in"
        );

        // The d-pad walks the subpanel's controls (and wraps within it).
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("v1_b"));
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("v1_a"), "the d-pad stays in the pane");

        // The stick moves the SUBPANEL among its peers, descending again.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused_pane(), Some("voice_2"), "the stick switches subpanels");
        assert_eq!(h.ui.focused(), Some("v2_a"));

        // Cancel never pops a level — it is the scene's.
        h.handle(&press(ActionSignal::Cancel, &raw), &mut rc);
        assert!(h.cancelled(), "Cancel is scene-level");
        assert_eq!(h.ui.focused(), Some("v2_a"), "…and moves nothing");
    }

    /// **The d-pad walks WITHIN the focused pane; the stick switches panes** (Aaron
    /// 2026-09-02). Acquiring the tree lands on the first pane's first control; the
    /// d-pad wraps inside that pane and never leaves it; `PanelNext` moves to the next
    /// pane and descends; `Confirm` on a control with an action fires it. This fixture
    /// passes no rects, exercising the ordinal ring fallback.
    #[test]
    fn the_dpad_walks_within_the_pane_and_the_stick_switches_panes() {
        let raw = InputState::new();
        // Two pane CONTAINERS, each claimed by its interior controls.
        let mut pane_a = UiNode {
            id: "pane_a".into(),
            component: "panel".into(),
            nav_ordinal: 1,
            ..Default::default()
        };
        pane_a.children.push(button("a_one", "pane_a", 1, "a_one"));
        pane_a.children.push(button("a_two", "pane_a", 2, "a_two"));
        let mut pane_b = UiNode {
            id: "pane_b".into(),
            component: "panel".into(),
            nav_ordinal: 2,
            ..Default::default()
        };
        pane_b.children.push(button("b_one", "pane_b", 1, "b_one"));
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        tree.children.push(pane_a);
        tree.children.push(pane_b);

        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        // The d-pad acquires the tree: the first pane's first control, pane implied.
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("a_one"),
            "acquiring the tree descends into the first pane"
        );
        assert_eq!(h.ui.focused_pane(), Some("pane_a"));

        // The d-pad walks the pane's controls and WRAPS inside it — never leaves.
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("a_two"), "the d-pad walks the pane");
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("a_one"), "…and wraps within it");
        assert_eq!(h.ui.focused_pane(), Some("pane_a"), "the pane never changed");

        // The STICK switches panes, descending into the next one.
        h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc);
        assert_eq!(h.ui.focused_pane(), Some("pane_b"), "the stick switches panes");
        assert_eq!(h.ui.focused(), Some("b_one"));

        // Confirm on the focused control fires its action on the one drain.
        h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
        assert_eq!(h.take_fired(), vec!["b_one".to_string()]);
    }

    /// **The pad carries and drops a payload end to end** (controller is the floor,
    /// BA4487BD). Confirm on the focused `drag_kind` source picks it up (and still
    /// fires the source's own action, exactly as a pointer PRESS on a drag source
    /// does); Confirm on the focused `drop_accept` target IS the drop and nothing
    /// else — no plain activation while a payload is in flight, the same way a
    /// pointer release over a target drops rather than clicking.
    #[test]
    fn the_pad_picks_up_a_payload_and_drops_it_on_an_accepting_target() {
        use crate::component::{run_ui, UiInput};
        use flicker_script::{UiAnchor, Value};
        use flicker_render::Vec2;

        let raw = InputState::new();
        let styles = serde_json::json!({});
        let model = shown();
        // The pointer sits nowhere near either box for the whole test — every edge
        // below comes from the pad.
        let idle = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(200.0, 100.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };

        let mut src = button("src", "g", 1, "open_clip");
        src.anchor = Some(UiAnchor::TopLeft);
        src.width = Some(60.0);
        src.height = Some(40.0);
        src.props
            .insert("drag_kind".into(), Value::Text("clip".into()));
        src.props
            .insert("drag_id".into(), Value::Text("walk_forward".into()));
        let mut bin = button("bin", "g", 2, "bind_clip");
        bin.anchor = Some(UiAnchor::TopLeft);
        bin.offset = [100.0, 0.0];
        bin.width = Some(60.0);
        bin.height = Some(40.0);
        bin.props
            .insert("drop_accept".into(), Value::Text("clip".into()));
        let mut tree = UiNode {
            id: "root".into(),
            component: "screen".into(),
            ..Default::default()
        };
        tree.children = vec![src, bin];

        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc); // acquire → src
            assert_eq!(h.ui.focused(), Some("src"));
            // Confirm on the SOURCE: a pickup is recorded AND the button still fires,
            // the same pair a pointer press on a drag source produces.
            h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
            assert_eq!(h.take_fired(), vec!["open_clip".to_string()]);
        }
        let f = run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert!(f.results.is_on("drag_active"), "the pad is carrying it");
        assert_eq!(f.results.text("drag_id"), Some("walk_forward"));

        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc); // → bin
            assert_eq!(h.ui.focused(), Some("bin"));
            // Confirm while CARRYING is the drop alone — `bind_clip` must NOT arrive
            // here as a plain activation, or the scene would see it twice.
            h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
            assert!(
                h.take_fired().is_empty(),
                "Confirm while carrying activates nothing directly"
            );
        }
        let f = run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert!(f.results.is_on("bind_clip"), "…it lands as the DROP instead");
        assert_eq!(f.results.text("drop_id"), Some("walk_forward"));
        assert_eq!(f.results.text("drop_target"), Some("bin"));
        assert!(ui.drag().is_none(), "the payload is delivered");
    }

    /// **Cancel abandons the carry** — the pad's twin of releasing over nothing — and
    /// is STILL scene-level (Aaron 2026-09-02): backing out is never consumed by the
    /// drag, so the scene pops its context exactly as it always did.
    #[test]
    fn cancel_abandons_an_in_flight_drag_and_stays_scene_level() {
        use crate::component::{run_ui, UiInput};
        use flicker_script::Value;
        use flicker_render::Vec2;

        let raw = InputState::new();
        let styles = serde_json::json!({});
        let model = shown();
        let idle = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(200.0, 100.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let mut src = button("a", "g", 1, "a");
        src.props
            .insert("drag_kind".into(), Value::Text("clip".into()));
        let tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            children: vec![src],
            ..Default::default()
        };
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
            h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
        }
        run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert!(ui.drag().is_some(), "the pad is carrying a payload");

        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
        h.handle(&press(ActionSignal::Cancel, &raw), &mut rc);
        assert!(h.ui.drag().is_none(), "backing out drops what was carried");
        assert!(h.cancelled(), "…and Cancel is still the scene's");
    }

    /// **A value control STEPS on its own axis and MOVES FOCUS on the cross axis** — never a
    /// focus trap (nav-tier contract 1B5F6BB8). A focused `select`: Left/Right nudge it
    /// (focus stays), Up/Down move to the next control.
    #[test]
    fn a_focused_value_control_steps_on_its_axis_and_moves_focus_across() {
        let raw = InputState::new();
        let mut sel = UiNode {
            id: "sel".into(),
            component: "select".into(),
            tab_group: "g".into(),
            nav_ordinal: 0,
            bind: Some("sel".into()),
            ..Default::default()
        };
        sel.children = vec![
            UiNode {
                component: "option".into(),
                ..Default::default()
            },
            UiNode {
                component: "option".into(),
                ..Default::default()
            },
        ];
        let btn = button("b", "g", 1, "b");
        let mut tree = UiNode {
            id: "root".into(),
            component: "cell".into(),
            ..Default::default()
        };
        tree.children = vec![sel, btn];
        let mut ui = UiState::new();
        ui.request_focus("sel");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        // Own axis (Right): steps the control — focus stays put (a nudge, not a move).
        h.handle(&press(ActionSignal::NavRight, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("sel"),
            "own-axis nav steps the control, focus stays"
        );
        // Cross axis (Down): moves focus to the next control — the control is not a trap.
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(
            h.ui.focused(),
            Some("b"),
            "cross-axis nav moves focus off the control"
        );
    }

    /// **The bumpers no longer move focus.** `TabNext`/`TabPrev` belong to the
    /// page/tab control's own rail (which steps itself now), so the walker passes
    /// them straight through — one meaning per signal.
    #[test]
    fn the_bumpers_no_longer_move_focus() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.request_focus("settings"); // in group "menu"
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        assert_eq!(
            h.handle(&press(ActionSignal::TabNext, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(
            h.handle(&press(ActionSignal::TabPrev, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(
            h.ui.focused(),
            Some("settings"),
            "the bumpers moved nothing"
        );
    }

    /// **Confirm on a focused button arrives through `take_fired`** — the ONE
    /// activation drain. A scene that already drains its declared intents gets
    /// pad activation with no extra line, and the name it folds is byte-identical
    /// to the one a click writes.
    #[test]
    fn confirm_on_a_focused_button_arrives_through_take_fired() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.request_focus("quit");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        assert_eq!(
            h.handle(&press(ActionSignal::Confirm, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(
            h.take_fired(),
            vec!["quit".to_string()],
            "the drain carries the activation"
        );
        assert!(h.take_fired().is_empty(), "and it drains");

        // The POINTER twin: a click on the same button writes the same key.
        let mut pad = ValueMap::new();
        {
            let mut ui2 = UiState::new();
            ui2.request_focus("quit");
            let mut rc2 = RouteCtx::new();
            let mut h2 = WalkerHandler::hud(&mut ui2, false).with_nav(&tree, &shown());
            h2.handle(&press(ActionSignal::Confirm, &raw), &mut rc2);
            for name in h2.take_fired() {
                pad.set(name, true);
            }
        }
        let mut click = ValueMap::new();
        click.set("quit", true); // what the hit pass writes for a clicked button
        assert!(pad.is_on("quit") && click.is_on("quit"));
        assert_eq!(
            pad.get("quit"),
            click.get("quit"),
            "same key, same value, same channel"
        );
    }

    /// **A mouse CLICK converges with a pad Confirm on the ONE drain (pump P2 /
    /// rule 37722F91 "all input events are signals").** `run_ui`'s hit pass records
    /// the clicked button's `action` into the shared [`UiState`]; the walker's
    /// [`take_fired`](WalkerHandler::take_fired) drains it — so a click reaches the
    /// scene's `sig_<name>` mirror byte-identically to a pad Confirm on the focused
    /// button. Before P2 the click fired only into `results` and never rode the
    /// drain, so a mouse-clicked `mode_<realm>` never mirrored `sig_mode_<realm>`
    /// and the menu's Lua never latched the page (MCP `4180A432`).
    #[test]
    fn a_mouse_click_arrives_through_take_fired_like_a_pad_confirm() {
        use flicker_render::Vec2;

        // One actionable button, top-left, filling a 200-wide cell at height 40 —
        // a click at its centre lands inside its resolved rect.
        let mut btn = button("go", "menu", 0, "go");
        btn.size = Some(40.0);
        let mut col = UiNode {
            component: "cell".into(),
            ..Default::default()
        };
        col.anchor = Some(flicker_script::UiAnchor::TopLeft);
        col.width = Some(200.0);
        col.children = vec![btn];
        let mut tree = UiNode {
            component: "surface".into(),
            id: "root".into(),
            ..Default::default()
        };
        tree.children.push(col);

        let model = ValueMap::new();
        let styles = serde_json::json!({});
        let click = crate::UiInput {
            mouse: Vec2::new(100.0, 20.0),
            clicked: true,
            down: true,
            right_down: false,
            screen: Vec2::new(800.0, 600.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let mut ui = UiState::new();

        // Hit pass: the click writes the action into `results` (unchanged) AND records
        // it on the one activation channel for the walker to drain.
        let frame = crate::run_ui(&tree, &model, &styles, &click, &mut ui);
        assert!(
            frame.results.is_on("go"),
            "the click still writes the action into results"
        );
        let hud_hit = frame.results.is_on("hud_hit");

        // The walker drains the SAME name a pad Confirm would — the convergence that
        // reaches the `sig_<name>` mirror.
        let mut h = WalkerHandler::hud(&mut ui, hud_hit).with_nav(&tree, &model);
        assert_eq!(
            h.take_fired(),
            vec!["go".to_string()],
            "the click rides the one drain to the mirror, like a pad Confirm"
        );
        assert!(h.take_fired().is_empty(), "and it drains exactly once");
    }

    #[test]
    fn cancel_backs_out_and_queues_pop_context() {
        use flicker_input_router::RouterRequest;
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        assert_eq!(
            h.handle(&press(ActionSignal::Cancel, &raw), &mut rc),
            Flow::Consumed
        );
        assert!(h.cancelled());
        assert!(rc.requests.contains(&RouterRequest::PopContext));
    }

    // ── Text entry (Aaron 2026-09-03: one signal switches the context) ───────

    fn field(id: &str, group: &str, ordinal: u32) -> UiNode {
        let mut n = UiNode {
            id: id.into(),
            component: "text_field".into(),
            bind: Some(format!("{id}_val")),
            tab_group: group.into(),
            nav_ordinal: ordinal,
            ..Default::default()
        };
        n.props.insert(
            "submit_action".into(),
            flicker_script::Value::Text(format!("{id}_go")),
        );
        n.props.insert(
            "cancel_action".into(),
            flicker_script::Value::Text(format!("{id}_drop")),
        );
        n
    }

    /// A bench form: a button, a digits field, and a default (chat-like) field.
    fn form_tree() -> UiNode {
        let mut chat = field("chat", "", 0);
        chat.props
            .insert("default_text".into(), flicker_script::Value::Bool(true));
        UiNode {
            id: "root".into(),
            component: "column".into(),
            children: vec![button("apply", "form", 0, "apply"), field("count", "form", 1), chat],
            ..Default::default()
        }
    }

    #[test]
    fn confirm_on_a_focused_text_field_enters_text_entry() {
        use flicker_input_router::RouterRequest;
        let raw = InputState::new();
        let tree = form_tree();
        let mut ui = UiState::new();
        ui.request_focus("count");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        assert_eq!(
            h.handle(&press(ActionSignal::Confirm, &raw), &mut rc),
            Flow::Consumed
        );
        assert!(rc
            .requests
            .contains(&RouterRequest::PushContext(InputContext::TextEntry)));
        assert!(rc
            .requests
            .contains(&RouterRequest::SetFocus("count".into())));
        assert!(h.ui.text_entry(), "the session is open on the field");
        assert_eq!(h.ui.edit_id(), Some("count"));
        assert!(h.take_fired().is_empty(), "entering fires no result");
    }

    #[test]
    fn enter_text_targets_the_focused_field_else_the_default_one() {
        use flicker_input_router::RouterRequest;
        let raw = InputState::new();
        let tree = form_tree();
        // Nothing focused: the screen's `default_text` field (the chat line) opens.
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        assert_eq!(
            h.handle(&press(ActionSignal::EnterText, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(h.ui.edit_id(), Some("chat"));
        assert!(rc
            .requests
            .contains(&RouterRequest::SetFocus("chat".into())));
        // A focused text field wins over the default.
        let mut ui = UiState::new();
        ui.request_focus("count");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.handle(&press(ActionSignal::EnterText, &raw), &mut rc);
        assert_eq!(h.ui.edit_id(), Some("count"));
        // A screen with no text field passes the signal on.
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&menu_tree(), &shown());
        assert_eq!(
            h.handle(&press(ActionSignal::EnterText, &raw), &mut rc),
            Flow::Pass
        );
    }

    #[test]
    fn submit_and_cancel_close_the_session_and_fire_the_field_exits() {
        use flicker_input_router::RouterRequest;
        let raw = InputState::new();
        let tree = form_tree();
        for (signal, name, restores) in [
            (ActionSignal::SubmitText, "count_go", false),
            (ActionSignal::CancelText, "count_drop", true),
        ] {
            let mut ui = UiState::new();
            ui.begin_edit("count", false, 0);
            // A fold captured the origin and the user typed since.
            ui.edit.as_mut().unwrap().origin = Some("12".into());
            let mut rc = RouteCtx::new();
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
            // The exits are meaningless outside a session.
            assert_eq!(
                h.handle(&press(signal, &raw), &mut rc),
                Flow::Consumed,
                "{signal:?}"
            );
            assert!(rc.requests.contains(&RouterRequest::PopContext), "{signal:?}");
            assert!(!h.ui.text_entry(), "{signal:?} closed the session");
            assert_eq!(h.ui.focused(), None, "{signal:?} released the focus");
            assert_eq!(h.take_fired(), vec![name.to_string()], "{signal:?}");
            assert_eq!(
                ui.revert.is_some(),
                restores,
                "only Cancel restores the pre-edit value"
            );
        }
        // Outside a session the exits pass through.
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        assert_eq!(
            h.handle(&press(ActionSignal::SubmitText, &raw), &mut rc),
            Flow::Pass
        );
    }

    /// The route's text reaches the open session and nothing else: a walker with a
    /// session queues it (the next fold applies it), a walker without one drops it.
    #[test]
    fn the_frame_hook_delivers_the_routes_text_to_the_open_session_only() {
        use flicker_input_core::TextStream;
        let tree = form_tree();
        let text = || TextStream {
            typed: "8".into(),
            ..Default::default()
        };
        let mut ui = UiState::new();
        ui.begin_edit("count", false, 0);
        let mut rc = RouteCtx::new();
        rc.text = text();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.frame(&mut rc);
        assert_eq!(ui.pending_text.typed, "8", "queued for the next fold");
        // No session: the text is dropped, never queued.
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        rc.text = text();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.frame(&mut rc);
        assert!(ui.pending_text.is_empty());
    }

    #[test]
    fn the_frame_hook_enters_on_a_click_focus_and_leaves_when_focus_moves_away() {
        use flicker_input_router::RouterRequest;
        let tree = form_tree();
        // A click landed in the field this frame (run_ui's verdict claimed focus).
        let mut ui = UiState::new();
        ui.open_text_entry("count");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.frame(&mut rc);
        assert!(h.ui.text_entry());
        assert!(rc
            .requests
            .contains(&RouterRequest::PushContext(InputContext::TextEntry)));
        // A pad nav landing on the field does NOT enter — Confirm does.
        let mut ui = UiState::new();
        ui.request_focus("count");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.frame(&mut rc);
        assert!(!h.ui.text_entry());
        // Focus that left the field (a click elsewhere) blurs: the context pops, the
        // value stands, nothing fires.
        let mut ui = UiState::new();
        ui.begin_edit("count", false, 0);
        ui.request_focus("apply");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.frame(&mut rc);
        assert!(!h.ui.text_entry());
        assert!(rc.requests.contains(&RouterRequest::PopContext));
        assert!(h.take_fired().is_empty());
        assert!(ui.revert.is_none());
    }

    // ── Declarative intents (S9) ─────────────────────────────────────────────

    /// A tree whose ROOT binds signals declaratively (`on_<signal>` props).
    fn declaring_tree() -> UiNode {
        let mut tree = menu_tree();
        tree.props.insert(
            "on_menu".into(),
            flicker_script::Value::Text("pause_open".into()),
        );
        tree.props.insert(
            "on_cancel".into(),
            flicker_script::Value::Text("settings_close".into()),
        );
        tree
    }

    #[test]
    fn declared_intent_fires_on_press_and_consumes_both_edges() {
        let raw = InputState::new();
        let tree = declaring_tree();
        let intents = UiIntents::of(&tree);
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_intents(&intents);

        // Press records the declared result name and consumes.
        assert_eq!(
            h.handle(&press(ActionSignal::Menu, &raw), &mut rc),
            Flow::Consumed
        );
        // The matching Release is consumed too but fires nothing again.
        assert_eq!(
            h.handle(&release(ActionSignal::Menu, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(h.take_fired(), vec!["pause_open".to_string()]);
        assert!(h.take_fired().is_empty(), "take_fired drains");
        // An undeclared signal still passes (the layer is not a black hole).
        assert_eq!(
            h.handle(&press(ActionSignal::Jump, &raw), &mut rc),
            Flow::Pass
        );
    }

    #[test]
    fn declared_binding_beats_the_nav_default() {
        // The tree is navigable AND declares `cancel_action`: the declaration owns
        // Cancel — the intent fires and the built-in back-out (cancelled() +
        // PopContext) does NOT also run. The screen said what Cancel means.
        let raw = InputState::new();
        let tree = declaring_tree();
        let intents = UiIntents::of(&tree);
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &shown())
            .with_intents(&intents);

        assert_eq!(
            h.handle(&press(ActionSignal::Cancel, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(h.take_fired(), vec!["settings_close".to_string()]);
        assert!(
            !h.cancelled(),
            "declared Cancel does not also run the nav back-out"
        );
        assert!(rc.requests.is_empty(), "…and queues no PopContext");
        // Undeclared nav signals keep their defaults on the same handler.
        assert_eq!(
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(
            h.ui.focused(),
            Some("start"),
            "nav still walks the focusables"
        );
    }

    // ── The DISMISSABLE toggle (ruling DA0E1B57) ─────────────────────────────

    /// A modal screen: the declaring root with a `popup_panel` slab under it, carrying
    /// whatever `dismissable` / `dismissable_bind` props the case is about.
    fn modal_tree(props: &[(&str, flicker_script::Value)]) -> UiNode {
        let mut slab = UiNode {
            id: "popup".into(),
            component: "popup_panel".into(),
            children: vec![button("ok", "modal", 0, "ok")],
            ..Default::default()
        };
        for (k, v) in props {
            slab.props.insert((*k).to_string(), v.clone());
        }
        let mut tree = UiNode {
            id: "root".into(),
            component: "surface".into(),
            children: vec![slab],
            ..Default::default()
        };
        tree.props.insert(
            "on_cancel".into(),
            flicker_script::Value::Text("modal_cancel".into()),
        );
        tree
    }

    /// Drive ONE Cancel press at a screen and report what the walker produced.
    fn cancel_on(tree: &UiNode, model: &ValueMap) -> (Flow, Vec<String>, bool) {
        let raw = InputState::new();
        let intents = UiIntents::of(tree);
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false)
            .with_nav(tree, model)
            .with_intents(&intents);
        let flow = h.handle(&press(ActionSignal::Cancel, &raw), &mut rc);
        (flow, h.take_fired(), h.cancelled())
    }

    /// **A NON-DISMISSABLE SLAB SWALLOWS CANCEL.** The screen still DECLARES its
    /// `on_cancel` (the host always injects one — the exit exists), but while the
    /// component says no, the walker eats the signal and fires nothing: no result name,
    /// no scene-level back-out, and nothing leaks below.
    #[test]
    fn a_non_dismissable_popup_panel_swallows_cancel() {
        let tree = modal_tree(&[("dismissable", flicker_script::Value::Bool(false))]);
        let (flow, fired, cancelled) = cancel_on(&tree, &shown());
        assert_eq!(
            flow,
            Flow::Consumed,
            "the signal never leaks below the slab"
        );
        assert!(
            fired.is_empty(),
            "the declared `on_cancel` must NOT fire while the slab holds the player"
        );
        assert!(!cancelled, "…and the nav back-out does not run either");

        // The same through the BIND: a pair script publishing `false` holds it shut.
        let bound = modal_tree(&[(
            "dismissable_bind",
            flicker_script::Value::Text("modal_dismissable".into()),
        )]);
        let mut model = ValueMap::new();
        model.set("modal_dismissable", false);
        let (_, fired, cancelled) = cancel_on(&bound, &model);
        assert!(fired.is_empty(), "a bound `false` swallows Cancel too");
        assert!(!cancelled);
    }

    /// **A DISMISSABLE SLAB IS UNCHANGED.** Default (nothing authored), an explicit
    /// `true`, and a bind published `true` all fire the screen's declared exit exactly
    /// as they did before the toggle existed.
    #[test]
    fn a_dismissable_popup_panel_fires_the_declared_on_cancel() {
        let mut lit = ValueMap::new();
        lit.set("modal_dismissable", true);
        for (case, tree, model) in [
            ("nothing authored (the default)", modal_tree(&[]), shown()),
            (
                "an explicit `dismissable: true`",
                modal_tree(&[("dismissable", flicker_script::Value::Bool(true))]),
                shown(),
            ),
            (
                "a bind published true",
                modal_tree(&[(
                    "dismissable_bind",
                    flicker_script::Value::Text("modal_dismissable".into()),
                )]),
                lit,
            ),
            // No slab at all: an ordinary screen is untouched by any of this.
            (
                "no popup_panel on the screen",
                {
                    let mut t = menu_tree();
                    t.props.insert(
                        "on_cancel".into(),
                        flicker_script::Value::Text("modal_cancel".into()),
                    );
                    t
                },
                shown(),
            ),
        ] {
            let (flow, fired, _) = cancel_on(&tree, &model);
            assert_eq!(flow, Flow::Consumed, "{case}: Cancel is still this layer's");
            assert_eq!(
                fired,
                vec!["modal_cancel".to_string()],
                "{case}: the declared exit fires"
            );
        }
    }

    /// **A BIND NOBODY PUBLISHES READS AS DISMISSABLE.** The fail-loud direction of the
    /// toggle: a typo'd `dismissable_bind` (or a pair script that failed to load) must
    /// cost you the FEATURE, never the way out — the alternative is a modal with no
    /// exit, which is the one thing a modal may never be (B89FAC21).
    #[test]
    fn an_unpublished_dismissable_bind_reads_as_dismissable() {
        let tree = modal_tree(&[(
            "dismissable_bind",
            flicker_script::Value::Text("nobody_publishes_this".into()),
        )]);
        let (_, fired, _) = cancel_on(&tree, &shown());
        assert_eq!(
            fired,
            vec!["modal_cancel".to_string()],
            "an unpublished bind is never a trap"
        );
        // …and a slab HIDDEN this frame holds nothing either, whatever it authored.
        let mut hidden = modal_tree(&[("dismissable", flicker_script::Value::Bool(false))]);
        hidden.children[0].visible_bind = Some("slab_shown".into());
        let (_, fired, _) = cancel_on(&hidden, &shown());
        assert_eq!(
            fired,
            vec!["modal_cancel".to_string()],
            "a slab that is not on screen does not hold Cancel"
        );
    }

    /// The toggle is scoped to the CANCEL routing: every other signal the screen owns
    /// keeps working while the slab is held shut (a busy modal still walks its buttons).
    #[test]
    fn a_held_slab_still_answers_every_other_signal() {
        let raw = InputState::new();
        let tree = modal_tree(&[("dismissable", flicker_script::Value::Bool(false))]);
        let intents = UiIntents::of(&tree);
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &shown())
            .with_intents(&intents);
        assert_eq!(
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc),
            Flow::Consumed
        );
        assert_eq!(h.ui.focused(), Some("ok"), "nav still walks the slab");
        h.handle(&press(ActionSignal::Confirm, &raw), &mut rc);
        assert_eq!(
            h.take_fired(),
            vec!["ok".to_string()],
            "Confirm still activates the focused control"
        );
    }

    #[test]
    fn empty_intents_change_nothing() {
        let raw = InputState::new();
        let intents = UiIntents::default();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        assert_eq!(
            h.handle(&press(ActionSignal::Menu, &raw), &mut rc),
            Flow::Pass
        );
        assert!(h.take_fired().is_empty());
    }

    #[test]
    fn nav_passes_through_when_not_navigable() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        // Plain hud() (no with_nav) → nav signals fall through to gameplay.
        let mut h = WalkerHandler::hud(&mut ui, false);
        assert_eq!(
            h.handle(&press(ActionSignal::NavDown, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(
            h.handle(&press(ActionSignal::Confirm, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(h.ui.focused(), None);
    }
}
