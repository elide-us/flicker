//! The **Screen declaration** — S8 of the Prism UI convergence: a scene declares,
//! as data, WHICH surfaces (templates, panels, dialogs, rtt views) its screen is
//! showing, and drives them through one small helper instead of scattering
//! `m.set("x_visible", …)` choreography through its update loop.
//!
//! A **surface** is any subtree of the screen gated by a `visible_bind` — a
//! settings section, a modal `choice_dialog`, an inspector panel, an rtt-view
//! template. The declaration is the [`Surface`] list a scene constructs its
//! [`Surfaces`] with, once; each frame the helper publishes every surface's
//! state into the Model ([`Surfaces::publish`]), and the walker's existing
//! `visible_bind` machinery does the rest. **There is deliberately no second
//! visibility path**: the helper writes the same Model keys the tree already
//! reads, so a surface toggled by the helper is indistinguishable from one
//! toggled by hand — and the draw cache sees an unchanged key as an unchanged
//! fingerprint (a still frame stays at zero crossings).
//!
//! What this replaces is the WITHIN-scene show/hide chain only. Scene-STACK
//! overlays (a pushed pause/settings/confirm scene freezing the game below —
//! `flicker-scene`'s update-top-only rule) are NOT surfaces and stay stack
//! scenes; a dialog popping up INSIDE a screen is.
//!
//! `context` is live S9 wiring now: a converted scene calls
//! [`Surfaces::apply_surface_contexts`] once per frame after its surface
//! updates, and each context-carrying flip becomes a `PushContext` (on-edge) /
//! `PopContext` (off-edge) queued into the frame's `RouteCtx` — reconciled by
//! the router's ordinary `apply_context_requests`, so the Router stays the ONE
//! routing authority. [`Surfaces::visibility_diff`] remains the raw diff for
//! tests and tooling; the two share one baseline, so a screen uses ONE of them
//! per frame.

use flicker_input_core::InputContext;
use flicker_input_router::RouteCtx;
use flicker_script::ValueMap;

/// One declared surface of a screen: its stable `id`, the Model key its
/// `visible_bind` reads (defaults to the id), its initial state, an optional
/// radio `group` for [`Surfaces::set_exclusive`], and the optional
/// `InputContext` name to hold while it is shown (S9 data — inert today).
#[derive(Clone, Debug)]
pub struct Surface {
    /// Stable identity the scene drives the surface by (`show("inspector")`).
    pub id: String,
    /// The Model key the subtree's `visible_bind` names; `None` = the id itself.
    pub key: Option<String>,
    /// State the surface starts in (`false` unless [`Surface::on`] is called).
    pub initial: bool,
    /// Radio group: [`Surfaces::set_exclusive`] turns every OTHER member of the
    /// same group off. Ungrouped surfaces are never touched by exclusivity.
    pub group: Option<String>,
    /// `InputContext` NAME to hold while shown (resolved through the context
    /// name registry — built-ins today, extensible contexts register by name):
    /// [`Surfaces::apply_surface_contexts`] queues its `PushContext` on the
    /// on-edge and `PopContext` on the off-edge.
    pub context: Option<String>,
}

impl Surface {
    /// A surface starting hidden, publishing under its own id.
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), key: None, initial: false, group: None, context: None }
    }

    /// Publish under `key` instead of the id (the tree's `visible_bind` key when
    /// it predates the declaration — e.g. the pocclusters inspector reads
    /// `has_pick`).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Start shown.
    pub fn on(mut self) -> Self {
        self.initial = true;
        self
    }

    /// Join a radio group (see [`Surfaces::set_exclusive`]).
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Declare the `InputContext` this surface holds while shown (S9 data).
    pub fn context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// The Model key this surface publishes under.
    fn model_key(&self) -> &str {
        self.key.as_deref().unwrap_or(&self.id)
    }
}

/// One surface state flip reported by [`Surfaces::visibility_diff`] — the S9
/// hook: a change carrying a `context` becomes a `PushContext` (`on`) or
/// `PopContext` (`!on`) through the Router once S9 wires it.
#[derive(Debug, PartialEq)]
pub struct SurfaceChange<'a> {
    pub id: &'a str,
    pub on: bool,
    pub context: Option<&'a str>,
}

/// The operation an [`OrchestrationRule`] applies to its target surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceOp {
    Show,
    Hide,
    Toggle,
    /// [`Surfaces::set_exclusive`] — show the target, hide its radio group.
    Exclusive,
}

/// One declarative rule of an **Orchestration** — Aaron's vocabulary
/// (ratified 2026-08-01): *"an Orchestration is a layout that changes based on
/// signals."* When the named result/signal fires this frame, the op is applied
/// to the target surface: `OnSignal(OpenInventory) → Show(inventory)` as data,
/// replacing the hand-written `if results.is_on(..) { surfaces.show(..) }`
/// chains. A [`crate::workflow::Workflow`] is an Orchestration with an ordinal.
#[derive(Clone, Debug)]
pub struct OrchestrationRule {
    /// The fired result name to react to — a click `action`, a declared-intent
    /// result, or a `sig_*` mirror key; anything truthy in this frame's results.
    pub on: String,
    /// What happens to the target.
    pub op: SurfaceOp,
    /// The surface id the op drives (unknown ids warn, never panic).
    pub target: String,
}

impl OrchestrationRule {
    fn rule(on: impl Into<String>, op: SurfaceOp, target: impl Into<String>) -> Self {
        Self { on: on.into(), op, target: target.into() }
    }

    /// `on` fired → show `target`.
    pub fn show(on: impl Into<String>, target: impl Into<String>) -> Self {
        Self::rule(on, SurfaceOp::Show, target)
    }

    /// `on` fired → hide `target`.
    pub fn hide(on: impl Into<String>, target: impl Into<String>) -> Self {
        Self::rule(on, SurfaceOp::Hide, target)
    }

    /// `on` fired → flip `target`.
    pub fn toggle(on: impl Into<String>, target: impl Into<String>) -> Self {
        Self::rule(on, SurfaceOp::Toggle, target)
    }

    /// `on` fired → show `target` exclusively within its radio group.
    pub fn exclusive(on: impl Into<String>, target: impl Into<String>) -> Self {
        Self::rule(on, SurfaceOp::Exclusive, target)
    }
}

/// A screen's declared surface set + their current states — the minimal Scene
/// Manager. Construct once with the declaration, mutate through
/// [`show`](Self::show) / [`hide`](Self::hide) / [`toggle`](Self::toggle) /
/// [`set`](Self::set) / [`set_exclusive`](Self::set_exclusive), publish into
/// the frame's Model with one [`publish`](Self::publish) call.
///
/// Costs: state ops are a linear probe over the handful of declared surfaces
/// (no hashing, no allocation); `publish` performs exactly the Model writes the
/// scattered `m.set` lines it replaces did; `visibility_diff` allocates only on
/// a frame where something actually flipped.
pub struct Surfaces {
    decls: Vec<Surface>,
    state: Vec<bool>,
    /// Baseline for [`visibility_diff`](Self::visibility_diff) /
    /// [`apply_surface_contexts`](Self::apply_surface_contexts) — starts at the
    /// initial states, advances each time either consumes the diff.
    prev: Vec<bool>,
    /// S9 LIFO ledger: decl indices of context-carrying surfaces whose context
    /// is currently pushed, in push order — how
    /// [`apply_surface_contexts`](Self::apply_surface_contexts) knows an
    /// off-edge's stack position (and detects an out-of-order hide).
    held: Vec<usize>,
}

impl Surfaces {
    /// Build from a screen's declaration. A duplicate id is a declaration bug:
    /// the later entry is dropped with a warning (best-effort, like every
    /// loader in this crate), so the first declaration stays authoritative.
    pub fn new(decls: Vec<Surface>) -> Self {
        let mut kept: Vec<Surface> = Vec::with_capacity(decls.len());
        for d in decls {
            if kept.iter().any(|k| k.id == d.id) {
                tracing::warn!("surfaces: duplicate declaration `{}` dropped", d.id);
            } else {
                kept.push(d);
            }
        }
        let state: Vec<bool> = kept.iter().map(|d| d.initial).collect();
        let prev = state.clone();
        // A surface declared initially-on with a context never saw an on-EDGE,
        // so nothing is held at construction (contexts ride edges, and the
        // stack's World base is the router's floor anyway).
        Self { decls: kept, state, prev, held: Vec::new() }
    }

    fn idx(&self, id: &str) -> Option<usize> {
        let i = self.decls.iter().position(|d| d.id == id);
        if i.is_none() {
            tracing::warn!("surfaces: unknown surface `{id}`");
        }
        i
    }

    /// Current state of `id` (`false` for an unknown id, with a warning).
    pub fn is_on(&self, id: &str) -> bool {
        self.idx(id).is_some_and(|i| self.state[i])
    }

    /// Set `id` to `on`. The workhorse for gates DERIVED from scene state each
    /// frame (`set("inspector", selection.is_some())`) — an unchanged value is
    /// a no-op all the way down (same Model value ⇒ same fingerprints ⇒ no
    /// redraw, and no entry in the next diff).
    pub fn set(&mut self, id: &str, on: bool) {
        if let Some(i) = self.idx(id) {
            self.state[i] = on;
        }
    }

    /// Show `id`.
    pub fn show(&mut self, id: &str) {
        self.set(id, true);
    }

    /// Hide `id`.
    pub fn hide(&mut self, id: &str) {
        self.set(id, false);
    }

    /// Flip `id`.
    pub fn toggle(&mut self, id: &str) {
        if let Some(i) = self.idx(id) {
            self.state[i] = !self.state[i];
        }
    }

    /// Show `id` and hide every other surface in its radio group — the
    /// one-of-N pattern (a settings section rail, an input sub-tab strip).
    /// Surfaces outside the group (other groups, ungrouped dialogs) are never
    /// touched. On an ungrouped id this is just [`show`](Self::show), with a
    /// warning — exclusivity over nothing is a declaration bug.
    pub fn set_exclusive(&mut self, id: &str) {
        let Some(i) = self.idx(id) else { return };
        match self.decls[i].group.clone() {
            Some(group) => {
                for (j, d) in self.decls.iter().enumerate() {
                    if d.group.as_deref() == Some(group.as_str()) {
                        self.state[j] = j == i;
                    }
                }
            }
            None => {
                tracing::warn!("surfaces: `{id}` has no group — set_exclusive is just show");
                self.state[i] = true;
            }
        }
    }

    /// Apply an Orchestration's rules over this frame's fired results — call
    /// after the scene folds its results (clicks, intents, `take_fired` names)
    /// and before [`publish`](Self::publish). Rules whose `on` did not fire are
    /// no-ops; a fired rule is exactly one of the state ops above, so unchanged
    /// outcomes still cost nothing downstream.
    pub fn orchestrate(&mut self, rules: &[OrchestrationRule], results: &ValueMap) {
        for r in rules {
            if !results.is_on(&r.on) {
                continue;
            }
            match r.op {
                SurfaceOp::Show => self.show(&r.target),
                SurfaceOp::Hide => self.hide(&r.target),
                SurfaceOp::Toggle => self.toggle(&r.target),
                SurfaceOp::Exclusive => self.set_exclusive(&r.target),
            }
        }
    }

    /// Publish every surface's state into the frame's Model under its declared
    /// key — the ONE visibility write a scene makes per frame. Values ride the
    /// same keys the tree's `visible_bind`s already name, so unchanged states
    /// leave every fingerprint (and therefore the draw cache) untouched.
    pub fn publish(&self, m: &mut ValueMap) {
        for (d, on) in self.decls.iter().zip(&self.state) {
            m.set(d.model_key(), *on);
        }
    }

    /// S9 context wiring — the LIVE consumer of the visibility diff: fold this
    /// frame's flips into router context requests. An on-edge whose surface
    /// declares a `context` queues `PushContext` (name resolved through
    /// [`InputContext`]'s registry; an unknown name is warned and skipped — the
    /// vocabulary gate); its off-edge queues `PopContext`. The requests land in
    /// the frame's [`RouteCtx`] and are reconciled by the router's ordinary
    /// `apply_context_requests` — the Router stays the one routing authority.
    ///
    /// **LIFO discipline**: contexts un-push in reverse show order. An
    /// out-of-order hide (a surface hidden while a LATER-shown context surface
    /// is still up) cannot remove from the middle of the router's stack, so it
    /// **pops-to** its own context — the newer contexts above it are popped with
    /// it, with a warning; the router never pops below the `World` base, so the
    /// worst degradation is the base map. Declare-and-hide LIFO and this path
    /// never fires.
    ///
    /// Shares the diff baseline with [`visibility_diff`](Self::visibility_diff)
    /// (each advances it), so a screen uses exactly ONE of the two per frame —
    /// converted scenes call this; tests/tooling read the raw diff.
    pub fn apply_surface_contexts(&mut self, route: &mut RouteCtx) {
        for i in 0..self.decls.len() {
            if self.state[i] == self.prev[i] {
                continue;
            }
            self.prev[i] = self.state[i];
            let d = &self.decls[i];
            let Some(name) = d.context.as_deref() else { continue };
            let Some(context) = InputContext::from_name(name) else {
                tracing::warn!(
                    "surfaces: `{}` names unknown InputContext `{name}` — not routed",
                    d.id
                );
                continue;
            };
            if self.state[i] {
                self.held.push(i);
                route.push_context(context);
            } else {
                match self.held.iter().rposition(|&h| h == i) {
                    Some(pos) => {
                        let above = self.held.len() - 1 - pos;
                        if above > 0 {
                            tracing::warn!(
                                "surfaces: `{}` hidden out of LIFO order — popping {above} newer context(s) with it",
                                d.id
                            );
                        }
                        for _ in pos..self.held.len() {
                            route.pop_context();
                        }
                        self.held.truncate(pos);
                    }
                    None => tracing::warn!(
                        "surfaces: `{}` off-edge holds no context (never pushed, or popped by an out-of-order hide) — not routed",
                        d.id
                    ),
                }
            }
        }
    }

    /// Every surface whose state changed since the last diff (baseline = the
    /// declared initial states), with its S9 `context` attached — then advance
    /// the baseline. The raw-data twin of
    /// [`apply_surface_contexts`](Self::apply_surface_contexts) (they share the
    /// baseline), kept for tests and tooling. A quiet frame returns an empty
    /// `Vec` without allocating.
    pub fn visibility_diff(&mut self) -> Vec<SurfaceChange<'_>> {
        let Self { decls, state, prev, .. } = self;
        decls
            .iter()
            .zip(state.iter().zip(prev.iter_mut()))
            .filter_map(|(d, (now, before))| {
                if *now == *before {
                    return None;
                }
                *before = *now;
                Some(SurfaceChange { id: &d.id, on: *now, context: d.context.as_deref() })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{run_ui, run_ui_with, UiInput, UiState};
    use crate::template::builtin_templates;
    use flicker_render::Vec2;
    use flicker_script::{UiNode, Value};

    fn decls() -> Vec<Surface> {
        vec![
            Surface::new("sec_video").group("section").on(),
            Surface::new("sec_audio").group("section"),
            Surface::new("sec_input").group("section"),
            Surface::new("sub_keyboard").group("subtab").on(),
            Surface::new("sub_mouse").group("subtab"),
            Surface::new("inspector").key("has_pick"),
            Surface::new("confirm_close").context("Menu"),
        ]
    }

    #[test]
    fn declaration_initial_states_publish_under_their_keys() {
        let s = Surfaces::new(decls());
        assert!(s.is_on("sec_video") && s.is_on("sub_keyboard"), "declared-on surfaces start on");
        assert!(!s.is_on("sec_audio") && !s.is_on("inspector"), "the rest start hidden");
        assert!(!s.is_on("nonsense"), "an unknown id reads as hidden");

        let mut m = ValueMap::new();
        s.publish(&mut m);
        assert!(m.is_on("sec_video"), "a surface publishes under its id by default");
        assert!(!m.is_on("sec_audio"));
        assert!(!m.is_on("has_pick"), "a declared `key` overrides the id");
        assert!(m.get("inspector").is_none(), "…and the id itself is NOT published");
        assert!(!m.is_on("confirm_close"));
    }

    #[test]
    fn show_hide_toggle_set_drive_state() {
        let mut s = Surfaces::new(decls());
        s.show("inspector");
        assert!(s.is_on("inspector"));
        s.hide("inspector");
        assert!(!s.is_on("inspector"));
        s.toggle("confirm_close");
        assert!(s.is_on("confirm_close"));
        s.set("confirm_close", false);
        assert!(!s.is_on("confirm_close"));
        // Unknown ids are inert (warned), never a panic mid-frame.
        s.show("nonsense");
        s.toggle("nonsense");
    }

    #[test]
    fn set_exclusive_flips_only_its_own_group() {
        let mut s = Surfaces::new(decls());
        s.show("confirm_close"); // an ungrouped dialog, on
        s.set_exclusive("sec_input");
        assert!(s.is_on("sec_input"), "the chosen member is on");
        assert!(!s.is_on("sec_video") && !s.is_on("sec_audio"), "its group siblings are off");
        assert!(s.is_on("sub_keyboard"), "the OTHER radio group is untouched");
        assert!(s.is_on("confirm_close"), "ungrouped surfaces are untouched");
        // Ungrouped exclusivity degrades to show (with a warning).
        s.set_exclusive("inspector");
        assert!(s.is_on("inspector"));
        assert!(s.is_on("sec_input"), "…and flips nothing else");
    }

    #[test]
    fn visibility_diff_reports_flips_once_with_their_context() {
        let mut s = Surfaces::new(decls());
        assert!(s.visibility_diff().is_empty(), "the initial states are the baseline");

        s.set_exclusive("sec_audio");
        s.show("confirm_close");
        let diff = s.visibility_diff();
        assert_eq!(
            diff,
            vec![
                SurfaceChange { id: "sec_video", on: false, context: None },
                SurfaceChange { id: "sec_audio", on: true, context: None },
                SurfaceChange { id: "confirm_close", on: true, context: Some("Menu") },
            ],
            "each flip is reported once, carrying its declared S9 context"
        );
        assert!(s.visibility_diff().is_empty(), "taking the diff advances the baseline");

        s.hide("confirm_close");
        assert_eq!(
            s.visibility_diff(),
            vec![SurfaceChange { id: "confirm_close", on: false, context: Some("Menu") }],
            "the off edge carries the context too — S9's PopContext"
        );

        // A flip that flips BACK before the diff is taken is not a change at all.
        s.show("inspector");
        s.hide("inspector");
        assert!(s.visibility_diff().is_empty());
    }

    #[test]
    fn orchestration_rules_apply_fired_results_to_surfaces() {
        // The HUD story as data: OnSignal(inventory_open) → Show(inventory);
        // rules whose result did not fire are no-ops; unknown targets warn.
        let mut s = Surfaces::new(vec![
            Surface::new("inventory"),
            Surface::new("map"),
            Surface::new("sec_video").group("section").on(),
            Surface::new("sec_audio").group("section"),
        ]);
        let rules = vec![
            OrchestrationRule::show("inventory_open", "inventory"),
            OrchestrationRule::hide("inventory_close", "inventory"),
            OrchestrationRule::toggle("map_toggle", "map"),
            OrchestrationRule::exclusive("pick_audio", "sec_audio"),
            OrchestrationRule::show("broken", "nonsense"),
        ];

        let mut results = ValueMap::new().with("inventory_open", true).with("map_toggle", true);
        s.orchestrate(&rules, &results);
        assert!(s.is_on("inventory") && s.is_on("map"));
        assert!(s.is_on("sec_video"), "unfired rules touch nothing");

        results = ValueMap::new()
            .with("inventory_close", true)
            .with("map_toggle", true)
            .with("pick_audio", true)
            .with("broken", true);
        s.orchestrate(&rules, &results);
        assert!(!s.is_on("inventory"), "hide rule fired");
        assert!(!s.is_on("map"), "toggle rule flipped back");
        assert!(s.is_on("sec_audio") && !s.is_on("sec_video"), "exclusive drove the group");
    }

    #[test]
    fn a_duplicate_declaration_keeps_the_first_entry() {
        let s = Surfaces::new(vec![Surface::new("a").on(), Surface::new("a")]);
        assert!(s.is_on("a"), "the first declaration stays authoritative");
        let mut m = ValueMap::new();
        s.publish(&mut m);
        assert!(m.is_on("a"));
    }

    // ── S9 context wiring (apply_surface_contexts) ───────────────────────────

    use flicker_input_core::{ContextualBindings, InputContext, InputMap};
    use flicker_input_router::{apply_context_requests, RouteCtx};

    /// Two context-carrying dialogs plus a context-less panel.
    fn ctx_decls() -> Vec<Surface> {
        vec![
            Surface::new("plain"),
            Surface::new("dialog_a").context("Menu"),
            Surface::new("dialog_b").context("Radial"),
            Surface::new("broken").context("Nonsense"),
        ]
    }

    /// Drain `s`'s flips into `cb` through the full seam: queue requests, then
    /// reconcile them exactly as a scene does after dispatch.
    fn reconcile(s: &mut Surfaces, cb: &mut ContextualBindings) {
        let mut route = RouteCtx::new();
        s.apply_surface_contexts(&mut route);
        apply_context_requests(cb, &route.requests);
    }

    #[test]
    fn on_and_off_edges_push_and_pop_their_declared_context() {
        let mut s = Surfaces::new(ctx_decls());
        let mut cb = ContextualBindings::new(InputMap::empty());

        // Quiet frame: nothing queued, baseline untouched.
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::World);

        // On-edge → the declared context is pushed; a context-less flip routes nothing.
        s.show("plain");
        s.show("dialog_a");
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::Menu);

        // Off-edge → popped back to the base.
        s.hide("dialog_a");
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::World);

        // The baseline advanced with every consume: the raw diff is empty now.
        assert!(s.visibility_diff().is_empty(), "shared baseline advanced");
    }

    #[test]
    fn lifo_hides_pop_in_reverse_show_order() {
        let mut s = Surfaces::new(ctx_decls());
        let mut cb = ContextualBindings::new(InputMap::empty());

        s.show("dialog_a"); // Menu
        reconcile(&mut s, &mut cb);
        s.show("dialog_b"); // Radial over Menu
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::Radial);

        s.hide("dialog_b"); // LIFO: the newest holder pops first
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::Menu);
        s.hide("dialog_a");
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::World);
    }

    #[test]
    fn out_of_order_hide_pops_to_its_context_and_never_below_world() {
        let mut s = Surfaces::new(ctx_decls());
        let mut cb = ContextualBindings::new(InputMap::empty());

        s.show("dialog_a"); // Menu
        s.show("dialog_b"); // Radial (same frame — declaration order = push order)
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::Radial);

        // Hiding A (below B) can't remove from the middle of the router stack:
        // it pops-TO its own context (warned), taking B's newer context with it.
        s.hide("dialog_a");
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::World, "pop-to reached the base");

        // B's later off-edge finds no held context: warned, nothing popped —
        // and the stack can never go below World regardless.
        s.hide("dialog_b");
        reconcile(&mut s, &mut cb);
        assert_eq!(cb.active(), InputContext::World);
    }

    #[test]
    fn unknown_context_name_is_warned_and_never_routed() {
        let mut s = Surfaces::new(ctx_decls());
        let mut route = RouteCtx::new();
        s.show("broken");
        s.apply_surface_contexts(&mut route);
        assert!(route.requests.is_empty(), "the vocabulary gate skips it");
        s.hide("broken");
        s.apply_surface_contexts(&mut route);
        assert!(route.requests.is_empty(), "…on both edges");
    }

    // ── Screen ▸ RTT composition ─────────────────────────────────────────────

    fn input_still() -> UiInput {
        UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            screen: Vec2::new(800.0, 600.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        }
    }

    /// A Screen declaration naming an rtt-view TEMPLATE surface round-trips
    /// visibility like any other surface: hidden, the walker never places the
    /// subtree, so its `RttSlot`s are not reserved; shown, both stages come
    /// back. This is the S8 Screen▸RTTs story document-by-test — the kept
    /// `side_by_side_rtt` data template, gated by a declared surface, driven
    /// through the helper (no current scene ships one, per the S3 ruling).
    #[test]
    fn an_rtt_view_template_surface_round_trips_visibility() {
        let json = r#"{ "component": "screen", "children": [
            { "template": "side_by_side_rtt", "id": "pair", "visible_bind": "workbench_on",
              "left_source": "cam_a", "right_source": "cam_b", "gap": 4 }
        ] }"#;
        // The surviving data path: the ONE arrangement reader + the registry.
        let value: serde_json::Value = serde_json::from_str(json).expect("json parses");
        let tree = crate::expand(
            flicker_script::parse_ui_json(&value).expect("arrangement parses"),
            &builtin_templates(),
        );
        assert!(crate::unknown_kinds(&tree).is_empty(), "the declared screen expands clean");

        let mut surfaces =
            Surfaces::new(vec![Surface::new("workbench").key("workbench_on").on()]);
        let mut state = UiState::new();
        let run = |surfaces: &Surfaces, state: &mut UiState| {
            let mut m = ValueMap::new();
            surfaces.publish(&mut m);
            run_ui(&tree, &m, &serde_json::json!({}), &input_still(), state)
        };

        let shown = run(&surfaces, &mut state);
        let ids: Vec<&str> = shown.rtts.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["pair_left", "pair_right"], "both stages reserve slots");
        assert_eq!(shown.rtts[0].source, "cam_a");
        assert_eq!(shown.rtts[1].source, "cam_b");

        surfaces.hide("workbench");
        assert_eq!(
            surfaces.visibility_diff(),
            vec![SurfaceChange { id: "workbench", on: false, context: None }]
        );
        let hidden = run(&surfaces, &mut state);
        assert!(hidden.rtts.is_empty(), "a hidden rtt surface reserves nothing");

        surfaces.show("workbench");
        let back = run(&surfaces, &mut state);
        assert_eq!(back.rtts.len(), 2, "showing it again brings both slots back");
    }

    // ── Cache discipline ─────────────────────────────────────────────────────

    /// Publishing the surface keys every frame must not defeat the draw cache:
    /// unchanged states are unchanged Model values, so a still frame replays
    /// byte-identical commands with zero Lua crossings; flipping one surface
    /// redraws only what appeared.
    #[test]
    fn republishing_unchanged_surfaces_keeps_the_draw_cache_still() {
        // Fingerprints fold `strings::generation()`; hold the guard so a
        // concurrent test's stringtable load can't force spurious redraws.
        let _g = crate::strings::test_guard();
        let lib = flicker_script::ScriptHost::library(crate::UI_COMPONENT_MODULES)
            .expect("component library");

        // A screen with an always-on button and a surface-gated panel.
        let btn = {
            let mut n = UiNode { component: "button".into(), ..Default::default() };
            n.id = "ok".into();
            n.size = Some(24.0);
            n.props.insert("label".into(), Value::Text("OK".into()));
            n.props.insert("style".into(), Value::Text("btn".into()));
            n
        };
        let panel = {
            let mut n = UiNode { component: "cell".into(), ..Default::default() };
            n.id = "tools".into();
            n.size = Some(40.0);
            n.visible_bind = Some("tools".into());
            n.props.insert("style".into(), Value::Text("box".into()));
            n
        };
        let mut col = UiNode { component: "cell".into(), ..Default::default() };
        col.width = Some(120.0);
        col.anchor = Some(flicker_script::UiAnchor::TopLeft);
        col.children = vec![btn, panel];
        let mut page = UiNode { component: "screen".into(), ..Default::default() };
        page.children = vec![col];

        let styles = serde_json::json!({
            "btn": { "fill_top": [0.2,0.3,0.5,1.0], "label": [1.0,1.0,1.0,1.0] },
            "box": { "fill": [0.1,0.1,0.1,1.0] }
        });
        let mut surfaces = Surfaces::new(vec![Surface::new("tools")]);
        let mut state = UiState::new();
        let run = |surfaces: &Surfaces, state: &mut UiState| {
            let mut m = ValueMap::new();
            surfaces.publish(&mut m);
            run_ui_with(&page, &m, &styles, &input_still(), state, Some(&lib))
        };

        let first = run(&surfaces, &mut state);
        assert!(first.stats.lua_draws >= 1, "cold frame draws the button via Lua");

        let second = run(&surfaces, &mut state);
        assert_eq!(second.stats.redraw_nodes, 0, "republished, unchanged: nothing redraws");
        assert_eq!(second.stats.lua_draws, 0, "…and Lua is never entered");
        assert_eq!(second.commands, first.commands, "the replay is byte-identical");

        surfaces.show("tools");
        let third = run(&surfaces, &mut state);
        // The appearing panel is a cache miss, and its auto-height parent re-measures
        // (its rect grew) — but the button REPLAYS: no Lua crossing on a surface flip
        // that leaves its inputs alone.
        assert_eq!(third.stats.redraw_nodes, 2, "the appearing panel + its resized parent");
        assert_eq!(third.stats.lua_draws, 0, "the button replays — Lua is never re-entered");
    }
}
