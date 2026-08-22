//! The **Screen declaration** — S8 of the Prism UI convergence: a scene declares,
//! as data, WHICH sections (panels, dialogs, nested sections) its screen is
//! showing, and drives them through one small helper instead of scattering
//! `m.set("x_visible", …)` choreography through its update loop.
//!
//! A **section** is any subtree of the screen gated by a `visible_bind` — a
//! settings section, a modal `choice_dialog`, an inspector panel, a nested
//! `surface`. The declaration is the [`Section`] list a scene constructs its
//! [`Sections`] with, once; each frame the helper publishes every section's
//! state into the Model ([`Sections::publish`]), and the walker's existing
//! `visible_bind` machinery does the rest. (This helper was named `Sections`
//! until 2026-08-21, when `surface` became the drawing-surface KIND.) **There is deliberately no second
//! visibility path**: the helper writes the same Model keys the tree already
//! reads, so a section toggled by the helper is indistinguishable from one
//! toggled by hand — and the draw cache sees an unchanged key as an unchanged
//! fingerprint (a still frame stays at zero crossings).
//!
//! What this replaces is the WITHIN-scene show/hide chain only. Scene-STACK
//! overlays (a pushed pause/settings/confirm scene freezing the game below —
//! `flicker-scene`'s update-top-only rule) are NOT sections and stay stack
//! scenes; a dialog popping up INSIDE a screen is.
//!
//! `context` is live S9 wiring now: a converted scene calls
//! [`Sections::apply_section_contexts`] once per frame after its section
//! updates, and each context-carrying flip becomes a `PushContext` (on-edge) /
//! `PopContext` (off-edge) queued into the frame's `RouteCtx` — reconciled by
//! the router's ordinary `apply_context_requests`, so the Router stays the ONE
//! routing authority. [`Sections::visibility_diff`] remains the raw diff for
//! tests and tooling; the two share one baseline, so a screen uses ONE of them
//! per frame.

use flicker_input_core::InputContext;
use flicker_input_router::RouteCtx;
use flicker_script::ValueMap;

/// One declared section of a screen: its stable `id`, the Model key its
/// `visible_bind` reads (defaults to the id), its initial state, an optional
/// radio `group` for [`Sections::set_exclusive`], and the optional
/// `InputContext` name to hold while it is shown (S9 data — inert today).
#[derive(Clone, Debug)]
pub struct Section {
    /// Stable identity the scene drives the section by (`show("inspector")`).
    pub id: String,
    /// The Model key the subtree's `visible_bind` names; `None` = the id itself.
    pub key: Option<String>,
    /// State the section starts in (`false` unless [`Section::on`] is called).
    pub initial: bool,
    /// Radio group: [`Sections::set_exclusive`] turns every OTHER member of the
    /// same group off. Ungrouped sections are never touched by exclusivity.
    pub group: Option<String>,
    /// `InputContext` NAME to hold while shown (resolved through the context
    /// name registry — built-ins today, extensible contexts register by name):
    /// [`Sections::apply_section_contexts`] queues its `PushContext` on the
    /// on-edge and `PopContext` on the off-edge.
    pub context: Option<String>,
}

impl Section {
    /// A section starting hidden, publishing under its own id.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            key: None,
            initial: false,
            group: None,
            context: None,
        }
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

    /// Join a radio group (see [`Sections::set_exclusive`]).
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Declare the `InputContext` this section holds while shown (S9 data).
    pub fn context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// The Model key this section publishes under.
    fn model_key(&self) -> &str {
        self.key.as_deref().unwrap_or(&self.id)
    }
}

/// One section state flip reported by [`Sections::visibility_diff`] — the S9
/// hook: a change carrying a `context` becomes a `PushContext` (`on`) or
/// `PopContext` (`!on`) through the Router once S9 wires it.
#[derive(Debug, PartialEq)]
pub struct SectionChange<'a> {
    pub id: &'a str,
    pub on: bool,
    pub context: Option<&'a str>,
}

/// A screen's declared section set + their current states — the minimal Scene
/// Manager. Construct once with the declaration, mutate through
/// [`show`](Self::show) / [`hide`](Self::hide) / [`toggle`](Self::toggle) /
/// [`set`](Self::set) / [`set_exclusive`](Self::set_exclusive), publish into
/// the frame's Model with one [`publish`](Self::publish) call.
///
/// Costs: state ops are a linear probe over the handful of declared sections
/// (no hashing, no allocation); `publish` performs exactly the Model writes the
/// scattered `m.set` lines it replaces did; `visibility_diff` allocates only on
/// a frame where something actually flipped.
pub struct Sections {
    decls: Vec<Section>,
    state: Vec<bool>,
    /// Baseline for [`visibility_diff`](Self::visibility_diff) /
    /// [`apply_section_contexts`](Self::apply_section_contexts) — starts at the
    /// initial states, advances each time either consumes the diff.
    prev: Vec<bool>,
    /// S9 LIFO ledger: decl indices of context-carrying sections whose context
    /// is currently pushed, in push order — how
    /// [`apply_section_contexts`](Self::apply_section_contexts) knows an
    /// off-edge's stack position (and detects an out-of-order hide).
    held: Vec<usize>,
}

impl Sections {
    /// Build from a screen's declaration. A duplicate id is a declaration bug:
    /// the later entry is dropped with a warning (best-effort, like every
    /// loader in this crate), so the first declaration stays authoritative.
    pub fn new(decls: Vec<Section>) -> Self {
        let mut kept: Vec<Section> = Vec::with_capacity(decls.len());
        for d in decls {
            if kept.iter().any(|k| k.id == d.id) {
                tracing::warn!("sections: duplicate declaration `{}` dropped", d.id);
            } else {
                kept.push(d);
            }
        }
        let state: Vec<bool> = kept.iter().map(|d| d.initial).collect();
        let prev = state.clone();
        // A section declared initially-on with a context never saw an on-EDGE,
        // so nothing is held at construction (contexts ride edges, and the
        // stack's World base is the router's floor anyway).
        Self {
            decls: kept,
            state,
            prev,
            held: Vec::new(),
        }
    }

    fn idx(&self, id: &str) -> Option<usize> {
        let i = self.decls.iter().position(|d| d.id == id);
        if i.is_none() {
            tracing::warn!("sections: unknown section `{id}`");
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

    /// Show `id` and hide every other section in its radio group — the
    /// one-of-N pattern (a settings section rail, an input sub-tab strip).
    /// Sections outside the group (other groups, ungrouped dialogs) are never
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
                tracing::warn!("sections: `{id}` has no group — set_exclusive is just show");
                self.state[i] = true;
            }
        }
    }

    /// Publish every section's state into the frame's Model under its declared
    /// key — the ONE visibility write a scene makes per frame. Values ride the
    /// same keys the tree's `visible_bind`s already name, so unchanged states
    /// leave every fingerprint (and therefore the draw cache) untouched.
    pub fn publish(&self, m: &mut ValueMap) {
        for (d, on) in self.decls.iter().zip(&self.state) {
            m.set(d.model_key(), *on);
        }
    }

    /// S9 context wiring — the LIVE consumer of the visibility diff: fold this
    /// frame's flips into router context requests. An on-edge whose section
    /// declares a `context` queues `PushContext` (name resolved through
    /// [`InputContext`]'s registry; an unknown name is warned and skipped — the
    /// vocabulary gate); its off-edge queues `PopContext`. The requests land in
    /// the frame's [`RouteCtx`] and are reconciled by the router's ordinary
    /// `apply_context_requests` — the Router stays the one routing authority.
    ///
    /// **LIFO discipline**: contexts un-push in reverse show order. An
    /// out-of-order hide (a section hidden while a LATER-shown context section
    /// is still up) cannot remove from the middle of the router's stack, so it
    /// **pops-to** its own context — the newer contexts above it are popped with
    /// it, with a warning; the router never pops below the `World` base, so the
    /// worst degradation is the base map. Declare-and-hide LIFO and this path
    /// never fires.
    ///
    /// Shares the diff baseline with [`visibility_diff`](Self::visibility_diff)
    /// (each advances it), so a screen uses exactly ONE of the two per frame —
    /// converted scenes call this; tests/tooling read the raw diff.
    pub fn apply_section_contexts(&mut self, route: &mut RouteCtx) {
        for i in 0..self.decls.len() {
            if self.state[i] == self.prev[i] {
                continue;
            }
            self.prev[i] = self.state[i];
            let d = &self.decls[i];
            let Some(name) = d.context.as_deref() else {
                continue;
            };
            let Some(context) = InputContext::from_name(name) else {
                tracing::warn!(
                    "sections: `{}` names unknown InputContext `{name}` — not routed",
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
                                "sections: `{}` hidden out of LIFO order — popping {above} newer context(s) with it",
                                d.id
                            );
                        }
                        for _ in pos..self.held.len() {
                            route.pop_context();
                        }
                        self.held.truncate(pos);
                    }
                    None => tracing::warn!(
                        "sections: `{}` off-edge holds no context (never pushed, or popped by an out-of-order hide) — not routed",
                        d.id
                    ),
                }
            }
        }
    }

    /// Every section whose state changed since the last diff (baseline = the
    /// declared initial states), with its S9 `context` attached — then advance
    /// the baseline. The raw-data twin of
    /// [`apply_section_contexts`](Self::apply_section_contexts) (they share the
    /// baseline), kept for tests and tooling. A quiet frame returns an empty
    /// `Vec` without allocating.
    pub fn visibility_diff(&mut self) -> Vec<SectionChange<'_>> {
        let Self {
            decls, state, prev, ..
        } = self;
        decls
            .iter()
            .zip(state.iter().zip(prev.iter_mut()))
            .filter_map(|(d, (now, before))| {
                if *now == *before {
                    return None;
                }
                *before = *now;
                Some(SectionChange {
                    id: &d.id,
                    on: *now,
                    context: d.context.as_deref(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{run_ui, UiInput, UiState};
    use flicker_render::Vec2;
    use flicker_script::{UiNode, Value};

    fn decls() -> Vec<Section> {
        vec![
            Section::new("sec_video").group("section").on(),
            Section::new("sec_audio").group("section"),
            Section::new("sec_input").group("section"),
            Section::new("sub_keyboard").group("subtab").on(),
            Section::new("sub_mouse").group("subtab"),
            Section::new("inspector").key("has_pick"),
            Section::new("confirm_close").context("Menu"),
        ]
    }

    #[test]
    fn declaration_initial_states_publish_under_their_keys() {
        let s = Sections::new(decls());
        assert!(
            s.is_on("sec_video") && s.is_on("sub_keyboard"),
            "declared-on sections start on"
        );
        assert!(
            !s.is_on("sec_audio") && !s.is_on("inspector"),
            "the rest start hidden"
        );
        assert!(!s.is_on("nonsense"), "an unknown id reads as hidden");

        let mut m = ValueMap::new();
        s.publish(&mut m);
        assert!(
            m.is_on("sec_video"),
            "a section publishes under its id by default"
        );
        assert!(!m.is_on("sec_audio"));
        assert!(!m.is_on("has_pick"), "a declared `key` overrides the id");
        assert!(
            m.get("inspector").is_none(),
            "…and the id itself is NOT published"
        );
        assert!(!m.is_on("confirm_close"));
    }

    #[test]
    fn show_hide_toggle_set_drive_state() {
        let mut s = Sections::new(decls());
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
        let mut s = Sections::new(decls());
        s.show("confirm_close"); // an ungrouped dialog, on
        s.set_exclusive("sec_input");
        assert!(s.is_on("sec_input"), "the chosen member is on");
        assert!(
            !s.is_on("sec_video") && !s.is_on("sec_audio"),
            "its group siblings are off"
        );
        assert!(
            s.is_on("sub_keyboard"),
            "the OTHER radio group is untouched"
        );
        assert!(s.is_on("confirm_close"), "ungrouped sections are untouched");
        // Ungrouped exclusivity degrades to show (with a warning).
        s.set_exclusive("inspector");
        assert!(s.is_on("inspector"));
        assert!(s.is_on("sec_input"), "…and flips nothing else");
    }

    #[test]
    fn visibility_diff_reports_flips_once_with_their_context() {
        let mut s = Sections::new(decls());
        assert!(
            s.visibility_diff().is_empty(),
            "the initial states are the baseline"
        );

        s.set_exclusive("sec_audio");
        s.show("confirm_close");
        let diff = s.visibility_diff();
        assert_eq!(
            diff,
            vec![
                SectionChange {
                    id: "sec_video",
                    on: false,
                    context: None
                },
                SectionChange {
                    id: "sec_audio",
                    on: true,
                    context: None
                },
                SectionChange {
                    id: "confirm_close",
                    on: true,
                    context: Some("Menu")
                },
            ],
            "each flip is reported once, carrying its declared S9 context"
        );
        assert!(
            s.visibility_diff().is_empty(),
            "taking the diff advances the baseline"
        );

        s.hide("confirm_close");
        assert_eq!(
            s.visibility_diff(),
            vec![SectionChange {
                id: "confirm_close",
                on: false,
                context: Some("Menu")
            }],
            "the off edge carries the context too — S9's PopContext"
        );

        // A flip that flips BACK before the diff is taken is not a change at all.
        s.show("inspector");
        s.hide("inspector");
        assert!(s.visibility_diff().is_empty());
    }

    #[test]
    fn a_duplicate_declaration_keeps_the_first_entry() {
        let s = Sections::new(vec![Section::new("a").on(), Section::new("a")]);
        assert!(s.is_on("a"), "the first declaration stays authoritative");
        let mut m = ValueMap::new();
        s.publish(&mut m);
        assert!(m.is_on("a"));
    }

    // ── S9 context wiring (apply_section_contexts) ───────────────────────────

    use flicker_input_core::{ContextualBindings, InputContext, InputMap};
    use flicker_input_router::{apply_context_requests, RouteCtx};

    /// Two context-carrying dialogs plus a context-less panel.
    fn ctx_decls() -> Vec<Section> {
        vec![
            Section::new("plain"),
            Section::new("dialog_a").context("Menu"),
            Section::new("dialog_b").context("Radial"),
            Section::new("broken").context("Nonsense"),
        ]
    }

    /// Drain `s`'s flips into `cb` through the full seam: queue requests, then
    /// reconcile them exactly as a scene does after dispatch.
    fn reconcile(s: &mut Sections, cb: &mut ContextualBindings) {
        let mut route = RouteCtx::new();
        s.apply_section_contexts(&mut route);
        apply_context_requests(cb, &route.requests);
    }

    #[test]
    fn on_and_off_edges_push_and_pop_their_declared_context() {
        let mut s = Sections::new(ctx_decls());
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
        let mut s = Sections::new(ctx_decls());
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
        let mut s = Sections::new(ctx_decls());
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
        let mut s = Sections::new(ctx_decls());
        let mut route = RouteCtx::new();
        s.show("broken");
        s.apply_section_contexts(&mut route);
        assert!(route.requests.is_empty(), "the vocabulary gate skips it");
        s.hide("broken");
        s.apply_section_contexts(&mut route);
        assert!(route.requests.is_empty(), "…on both edges");
    }

    // ── Screen ▸ RTT composition ─────────────────────────────────────────────

    fn input_still() -> UiInput {
        UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(800.0, 600.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        }
    }

    /// A Screen declaration naming an rtt-view section round-trips visibility like
    /// any other section: hidden, the walker never places the subtree, so its
    /// `SurfaceSlot`s are not reserved; shown, both stages come back. This is the S8
    /// Screen▸RTTs story document-by-test.
    ///
    /// The vehicle is a `row` of two `rtt` wells authored straight from component
    /// KINDS — the template tier that once wrapped this (a `multi_view` of two
    /// `rtt_panel`s) is gone (201F4F51). The subject of the gate is the SURFACE
    /// round trip, not the arrangement: any declaration reserving two stages would
    /// do, and this one is the shape the benches ship.
    #[test]
    fn a_nested_surface_section_round_trips_visibility() {
        let json = r#"{ "component": "surface", "children": [
            { "component": "row", "id": "pair", "visible_bind": "workbench_on", "gap": 4,
              "children": [
                { "component": "surface", "id": "pair_left_rtt",  "tab_group": "l", "source": "cam_a" },
                { "component": "surface", "id": "pair_right_rtt", "tab_group": "r", "source": "cam_b" }
              ] }
        ] }"#;
        // The ONE arrangement reader — no expansion pass; the tree is already kinds.
        let value: serde_json::Value = serde_json::from_str(json).expect("json parses");
        let tree = flicker_script::parse_ui_json(&value).expect("arrangement parses");
        assert!(
            crate::unknown_kinds(&tree).is_empty(),
            "the declared screen names known kinds"
        );

        let mut sections = Sections::new(vec![Section::new("workbench").key("workbench_on").on()]);
        let mut state = UiState::new();
        let run = |sections: &Sections, state: &mut UiState| {
            let mut m = ValueMap::new();
            sections.publish(&mut m);
            run_ui(&tree, &m, &serde_json::json!({}), &input_still(), state)
        };

        let shown = run(&sections, &mut state);
        let ids: Vec<&str> = shown.surfaces.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["pair_left_rtt", "pair_right_rtt"],
            "both stages reserve slots"
        );
        assert_eq!(shown.surfaces[0].source, "cam_a");
        assert_eq!(shown.surfaces[1].source, "cam_b");

        sections.hide("workbench");
        assert_eq!(
            sections.visibility_diff(),
            vec![SectionChange {
                id: "workbench",
                on: false,
                context: None
            }]
        );
        let hidden = run(&sections, &mut state);
        assert!(
            hidden.surfaces.is_empty(),
            "a hidden rtt section reserves nothing"
        );

        sections.show("workbench");
        let back = run(&sections, &mut state);
        assert_eq!(
            back.surfaces.len(),
            2,
            "showing it again brings both slots back"
        );
    }

    // ── Cache discipline ─────────────────────────────────────────────────────

    /// Publishing the section keys every frame must not defeat the draw cache:
    /// unchanged states are unchanged Model values, so a still frame replays
    /// byte-identical commands and redraws nothing; flipping one section
    /// redraws only what appeared.
    #[test]
    fn republishing_unchanged_sections_keeps_the_draw_cache_still() {
        // Fingerprints fold `strings::generation()`; hold the guard so a
        // concurrent test's stringtable load can't force spurious redraws.
        let _g = crate::strings::test_guard();

        // A screen with an always-on button and a section-gated panel.
        let btn = {
            let mut n = UiNode {
                component: "button".into(),
                ..Default::default()
            };
            n.id = "ok".into();
            n.size = Some(24.0);
            n.props.insert("label".into(), Value::Text("OK".into()));
            n.props.insert("style".into(), Value::Text("btn".into()));
            n
        };
        let panel = {
            let mut n = UiNode {
                component: "cell".into(),
                ..Default::default()
            };
            n.id = "tools".into();
            n.size = Some(40.0);
            n.visible_bind = Some("tools".into());
            n.props.insert("style".into(), Value::Text("box".into()));
            n
        };
        let mut col = UiNode {
            component: "cell".into(),
            ..Default::default()
        };
        col.width = Some(120.0);
        col.anchor = Some(flicker_script::UiAnchor::TopLeft);
        col.children = vec![btn, panel];
        let mut page = UiNode {
            component: "surface".into(),
            ..Default::default()
        };
        page.children = vec![col];

        let styles = serde_json::json!({
            "btn": { "fill_top": [0.2,0.3,0.5,1.0], "label": [1.0,1.0,1.0,1.0] },
            "box": { "fill": [0.1,0.1,0.1,1.0] }
        });
        let mut sections = Sections::new(vec![Section::new("tools")]);
        let mut state = UiState::new();
        let run = |sections: &Sections, state: &mut UiState| {
            let mut m = ValueMap::new();
            sections.publish(&mut m);
            run_ui(&page, &m, &styles, &input_still(), state)
        };

        let first = run(&sections, &mut state);
        // What this test is about is the CACHE: a cold frame draws, a
        // republished-but-unchanged frame does not.
        assert!(first.stats.redraw_nodes > 0, "cold frame draws the button");

        let second = run(&sections, &mut state);
        assert_eq!(
            second.stats.redraw_nodes, 0,
            "republished, unchanged: nothing redraws"
        );
        assert_eq!(
            second.commands, first.commands,
            "the replay is byte-identical"
        );

        sections.show("tools");
        let third = run(&sections, &mut state);
        // The appearing panel is a cache miss, and its auto-height parent re-measures
        // (its rect grew) — but the button REPLAYS: a section flip that leaves its
        // inputs alone must not redraw it.
        assert_eq!(
            third.stats.redraw_nodes, 2,
            "the appearing panel + its resized parent"
        );
    }
}
