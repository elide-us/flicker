//! ⛔ RETIRED FROM THE ENGINE (Aaron, 2026-08-12) — this runtime was VENDORED into
//! its ONLY consumer (this dormant bench) when flicker-widgets dropped the
//! Workflow/Orchestration construct; it dies entirely when this bench migrates to
//! the scene-def + Lua system. Do not grow it; do not re-export it.
//!
//! The **Workflow** — an Orchestration with an ordinal (Aaron, ratified
//! 2026-08-01): a LINEAR sequence of step surfaces + gates + the document
//! contract, wrapping ONE [`Surfaces`] exclusive group. Branching is deliberately
//! NOT here: a required branch is a *different workflow definition*, chosen up
//! front by the dispatch cards (the launcher-field pattern) — "linear, multiple
//! workflow definitions in data."
//!
//! **The document is the pipe.** Steps never talk to each other: each reads and
//! writes the bench's one document, and declares its contract as `needs` (Model
//! keys that must be present to enter) and `yields` (keys it produces). Gating
//! is fail-loud: `Next` into a step whose needs are unmet warns and refuses —
//! never a blank page (the authored-names law).
//!
//! **Back is destructive-guarded** (Aaron's R3): `Back` with unsaved step
//! changes arms the discard confirmation (`wf_discard` surface — the `workflow`
//! proto wires a `choice_dialog` to it); `wf_discard_yes` steps back and clears
//! the dirty flag, `wf_discard_no` keeps editing. Reverting the *document's*
//! stage edits on discard is stage logic — the scene reacts to the same
//! `wf_discard_yes` result; the runtime owns progression only.
//!
//! The runtime does NO IO. File dialogs, content/processed/baked writes live in
//! stage logic — that split is what keeps this thin. Workflow *definitions* are
//! durable content (`ui_workflows.json`, loaded by [`workflows_from_json`]);
//! step documents are ephemeral scene state.
//!
//! Result vocabulary (fixed, one way): `wf_next` · `wf_back` ·
//! `wf_discard_yes` · `wf_discard_no`. Published Model keys: each step's
//! surface key (via [`Surfaces`]) · `wf_discard` · `wf_step` · `wf_step_i` /
//! `wf_step_n` (1-based) · `wf_can_next` · per-step `wf_<id>_title` (resolved
//! through the stringtable, so rail chips ride pre-localized binds) and
//! `wf_<id>_style` (a dotted style path for `style_bind` rail chips:
//! `workflow.chip.active` / `.visited` / `.todo`).

use flicker::script::ValueMap;
use flicker::ui::{Surface, Surfaces};
use std::collections::HashMap;

/// One declared step of a workflow: its stable `id`, its rail title (a
/// `$token`), the surface its subtree is gated by (defaults to the id), the
/// document keys it `needs` on entry and `yields` for downstream steps, and the
/// optional `InputContext` its surface holds while shown.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Step {
    pub id: String,
    /// Rail label as a stringtable `$token` (resolved at publish).
    pub title: String,
    /// The surface / `visible_bind` key of the step's subtree; `None` = the id.
    #[serde(default)]
    pub surface: Option<String>,
    /// Document keys that must be PRESENT to enter this step (fail-loud gate).
    #[serde(default)]
    pub needs: Vec<String>,
    /// Document keys this step produces — the declared forward contract.
    #[serde(default)]
    pub yields: Vec<String>,
    /// `InputContext` name the step's surface holds while shown (S9 data).
    #[serde(default)]
    pub context: Option<String>,
}

impl Step {
    // Steps only ever DESERIALIZE from `ui_workflows.json` — there is no
    // hand-construction path.

    /// The Model key the step's subtree gates on: an explicit `surface`, else
    /// the NAMESPACED default `wf_step_<id>` — bare ids collided with sibling
    /// Model namespaces (and with document keys like `attach`).
    fn surface_key(&self) -> String {
        self.surface
            .clone()
            .unwrap_or_else(|| format!("wf_step_{}", self.id))
    }
}

/// One workflow definition as it ships in `ui_workflows.json`: the linear step
/// list. (The file also carries a `title` per workflow — display copy the bench
/// reads from its own tree, so the parse ignores it here.)
#[derive(Clone, Debug, serde::Deserialize)]
pub struct WorkflowDef {
    pub steps: Vec<Step>,
}

#[derive(serde::Deserialize)]
struct WorkflowsFile {
    workflows: HashMap<String, WorkflowDef>,
}

/// Load the workflow definitions from `ui_workflows.json` content (durable
/// content under `content/sensorium/resources/`). Best-effort like every
/// loader in this crate: a parse error warns and yields an empty map rather
/// than taking the bench down.
pub fn workflows_from_json(json: &str) -> HashMap<String, WorkflowDef> {
    match serde_json::from_str::<WorkflowsFile>(json) {
        Ok(f) => f.workflows,
        Err(e) => {
            tracing::warn!("workflow: ui_workflows.json failed to parse — {e}");
            HashMap::new()
        }
    }
}

/// The dedicated discard-confirmation surface every workflow carries.
const DISCARD: &str = "wf_discard";

/// A running workflow: the ordinal over one [`Surfaces`] exclusive group.
/// Construct from a [`WorkflowDef`]'s step list, feed it the frame's results +
/// document with [`handle`](Self::handle), and publish with
/// [`publish`](Self::publish).
pub struct Workflow {
    steps: Vec<Step>,
    current: usize,
    visited: Vec<bool>,
    /// The current step has unsaved changes — armed by stage logic via
    /// [`set_dirty`](Self::set_dirty); makes `Back` destructive-guarded.
    dirty: bool,
    surfaces: Surfaces,
}

impl Workflow {
    /// Build from the linear step list. Step surfaces form one exclusive group;
    /// the first step starts shown; the discard dialog surface rides along.
    ///
    /// Construction validates the declared contract fail-loud: a `needs` key no
    /// earlier step `yields` is warned at BUILD time (it is either scene-provided
    /// — legitimate — or unsatisfiable, and the author finds out now, not at the
    /// hundredth click of a Next that refuses).
    pub fn new(steps: Vec<Step>) -> Self {
        for (i, s) in steps.iter().enumerate() {
            for need in &s.needs {
                let upstream = steps[..i]
                    .iter()
                    .any(|p| p.yields.iter().any(|y| y == need));
                if !upstream {
                    tracing::warn!(
                        "workflow: step `{}` needs `{need}`, which no earlier step yields — \
                         scene-provided, or unsatisfiable",
                        s.id
                    );
                }
            }
        }
        let mut decls: Vec<Surface> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut d = Surface::new(s.surface_key().as_str()).group("wf_steps");
                if i == 0 {
                    d = d.on();
                }
                if let Some(c) = &s.context {
                    d = d.context(c.clone());
                }
                d
            })
            .collect();
        decls.push(Surface::new(DISCARD));
        let visited = vec![false; steps.len()];
        Self {
            steps,
            current: 0,
            visited,
            dirty: false,
            surfaces: Surfaces::new(decls),
        }
    }

    /// Build from a loaded definition.
    pub fn from_def(def: &WorkflowDef) -> Self {
        Self::new(def.steps.clone())
    }

    /// The current step's id.
    pub fn step(&self) -> &str {
        &self.steps[self.current].id
    }

    /// Mark the current step as carrying unsaved changes (stage logic calls
    /// this on edits, clears it on commit) — what arms the Back discard guard.
    pub fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
    }

    /// Whether `Next` may advance: a next step exists and every document key it
    /// `needs` is present. (The last step's `Next` is the bench's own finish
    /// action — the runtime ignores it.)
    pub fn can_next(&self, doc: &ValueMap) -> bool {
        match self.steps.get(self.current + 1) {
            Some(next) => next.needs.iter().all(|k| doc.get(k).is_some()),
            None => false,
        }
    }

    /// Consume this frame's workflow results against the document: `wf_next`
    /// advances through the gate (unmet needs warn and refuse — fail loud,
    /// never a blank page); `wf_back` steps back, or arms the discard dialog
    /// when the step is dirty; `wf_discard_yes` / `wf_discard_no` resolve it.
    pub fn handle(&mut self, results: &ValueMap, doc: &ValueMap) {
        if results.is_on("wf_next") {
            match self.steps.get(self.current + 1) {
                Some(next) => {
                    let unmet: Vec<&str> = next
                        .needs
                        .iter()
                        .filter(|k| doc.get(k).is_none())
                        .map(String::as_str)
                        .collect();
                    if unmet.is_empty() {
                        // Leaving a step that declared yields it never produced is
                        // caught HERE, at the source — not one step later when a
                        // downstream `needs` fails against it.
                        let lied: Vec<&str> = self.steps[self.current]
                            .yields
                            .iter()
                            .filter(|k| doc.get(k).is_none())
                            .map(String::as_str)
                            .collect();
                        if !lied.is_empty() {
                            tracing::warn!(
                                "workflow: leaving `{}` without its declared yields {lied:?}",
                                self.step()
                            );
                        }
                        self.visited[self.current] = true;
                        self.current += 1;
                        self.dirty = false;
                    } else {
                        tracing::warn!(
                            "workflow: `{}` cannot enter — needs {unmet:?} not in the document",
                            next.id
                        );
                    }
                }
                None => tracing::warn!(
                    "workflow: `wf_next` on the last step `{}` — finishing is the bench's own action",
                    self.step()
                ),
            }
        }
        if results.is_on("wf_back") && self.current > 0 {
            if self.dirty {
                self.surfaces.show(DISCARD);
            } else {
                self.current -= 1;
            }
        }
        if results.is_on("wf_discard_yes") && self.surfaces.is_on(DISCARD) {
            self.surfaces.hide(DISCARD);
            self.dirty = false;
            self.current = self.current.saturating_sub(1);
        }
        if results.is_on("wf_discard_no") {
            self.surfaces.hide(DISCARD);
        }
    }

    /// Publish the workflow's whole UI state into the frame's Model: the step
    /// surfaces (exclusive on the current step), the discard dialog, the rail
    /// chips (`wf_<id>_title` pre-resolved through the stringtable so labels
    /// ride localized binds; `wf_<id>_style` as a dotted `style_bind` path),
    /// and the footer keys (`wf_step`, `wf_step_i`/`_n`, `wf_can_next`).
    pub fn publish(&mut self, doc: &ValueMap, m: &mut ValueMap) {
        let key = self.steps[self.current].surface_key();
        self.surfaces.set_exclusive(&key);
        self.surfaces.publish(m);
        m.set("wf_step", self.step().to_string());
        m.set("wf_step_i", (self.current + 1) as f64);
        m.set("wf_step_n", self.steps.len() as f64);
        m.set("wf_can_next", self.can_next(doc));
        for (i, s) in self.steps.iter().enumerate() {
            let state = if i == self.current {
                "active"
            } else if self.visited[i] {
                "visited"
            } else {
                "todo"
            };
            m.set(
                format!("wf_{}_title", s.id),
                flicker::ui::strings::resolve(&s.title).into_owned(),
            );
            m.set(
                format!("wf_{}_style", s.id),
                format!("workflow.chip.{state}"),
            );
            // Rail MEMBERSHIP: true for every step of the running definition, so
            // the rail derives from the data — a chip's visible_bind names this,
            // and adding a step to ui_workflows.json grows the rail with no
            // hand-kept id list anywhere.
            m.set(format!("wf_{}_show", s.id), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps() -> Vec<Step> {
        // Through the same serde path the shipped `ui_workflows.json` takes.
        serde_json::from_value(serde_json::json!([
            { "id": "task", "title": "$wf_step_task", "yields": ["source"] },
            { "id": "conform", "title": "$wf_step_conform", "needs": ["source"], "yields": ["rig"] },
            { "id": "review", "title": "$wf_step_review", "needs": ["rig"] }
        ]))
        .expect("test steps deserialize")
    }

    fn fired(name: &str) -> ValueMap {
        ValueMap::new().with(name, true)
    }

    #[test]
    fn linear_walk_is_gated_by_declared_needs() {
        let mut w = Workflow::new(steps());
        assert_eq!(w.step(), "task");

        // The gate refuses while `source` is absent from the document…
        let empty = ValueMap::new();
        assert!(!w.can_next(&empty));
        w.handle(&fired("wf_next"), &empty);
        assert_eq!(w.step(), "task", "unmet needs refuse the advance (warned)");

        // …and admits once the step yielded it.
        let doc = ValueMap::new().with("source", "clay/thing");
        assert!(w.can_next(&doc));
        w.handle(&fired("wf_next"), &doc);
        assert_eq!(w.step(), "conform");

        // The last step has no next: can_next is false there by definition.
        let doc = doc.with("rig", "humanoid");
        w.handle(&fired("wf_next"), &doc);
        assert_eq!(w.step(), "review");
        assert!(
            !w.can_next(&doc),
            "the last step's Next is the bench's finish"
        );
        w.handle(&fired("wf_next"), &doc);
        assert_eq!(w.step(), "review", "…and the runtime ignores it");
    }

    #[test]
    fn back_is_clean_when_not_dirty_and_guarded_when_dirty() {
        let mut w = Workflow::new(steps());
        let doc = ValueMap::new().with("source", "s").with("rig", "r");
        w.handle(&fired("wf_next"), &doc);
        assert_eq!(w.step(), "conform");

        // Clean back: straight step-back, no dialog.
        w.handle(&fired("wf_back"), &doc);
        assert_eq!(w.step(), "task");
        let mut m = ValueMap::new();
        w.publish(&doc, &mut m);
        assert!(!m.is_on("wf_discard"));

        // Dirty back: arms the discard dialog and HOLDS the step.
        w.handle(&fired("wf_next"), &doc);
        w.set_dirty(true);
        w.handle(&fired("wf_back"), &doc);
        assert_eq!(w.step(), "conform", "held until the dialog resolves");
        let mut m = ValueMap::new();
        w.publish(&doc, &mut m);
        assert!(m.is_on("wf_discard"), "the confirm dialog is up");

        // Keep editing: dialog closes, step and dirty flag stay.
        w.handle(&fired("wf_discard_no"), &doc);
        let mut m = ValueMap::new();
        w.publish(&doc, &mut m);
        assert!(!m.is_on("wf_discard"));
        assert_eq!(w.step(), "conform");

        // Discard: dialog closes, the step goes back, dirty clears.
        w.handle(&fired("wf_back"), &doc);
        w.handle(&fired("wf_discard_yes"), &doc);
        assert_eq!(w.step(), "task");
        w.handle(&fired("wf_back"), &doc);
        assert_eq!(w.step(), "task", "Back on the first step is inert");
    }

    #[test]
    fn publish_reports_rail_footer_and_exclusive_surfaces() {
        let mut w = Workflow::new(steps());
        let doc = ValueMap::new().with("source", "s");
        let mut m = ValueMap::new();
        w.publish(&doc, &mut m);

        assert!(m.is_on("wf_step_task"), "the current step's surface is on");
        assert!(
            !m.is_on("wf_step_conform") && !m.is_on("wf_step_review"),
            "the rest are off (exclusive)"
        );
        assert!(
            m.is_on("wf_task_show") && m.is_on("wf_conform_show") && m.is_on("wf_review_show"),
            "every defined step publishes rail membership"
        );
        assert_eq!(m.text("wf_step"), Some("task"));
        assert_eq!(m.number("wf_step_i"), Some(1.0));
        assert_eq!(m.number("wf_step_n"), Some(3.0));
        assert!(m.is_on("wf_can_next"));
        assert_eq!(m.text("wf_task_style"), Some("workflow.chip.active"));
        assert_eq!(m.text("wf_conform_style"), Some("workflow.chip.todo"));
        // (Raw-token fallback for an UNLOADED table is a strings-module property,
        // tested there — not asserted here, where sibling tests load the real table
        // process-wide.)
        assert!(m.text("wf_task_title").is_some());

        w.handle(&fired("wf_next"), &doc);
        let mut m = ValueMap::new();
        w.publish(&doc, &mut m);
        assert!(
            m.is_on("wf_step_conform") && !m.is_on("wf_step_task"),
            "the exclusive group advanced"
        );
        assert_eq!(m.text("wf_task_style"), Some("workflow.chip.visited"));
        assert_eq!(m.text("wf_conform_style"), Some("workflow.chip.active"));
    }

    #[test]
    fn definitions_load_from_json_and_build() {
        let json = r#"{ "workflows": {
            "asset_import": { "title": "$wf_asset_import", "steps": [
                { "id": "task", "title": "$wf_step_task", "yields": ["source"] },
                { "id": "review", "title": "$wf_step_review", "needs": ["source"], "context": "Menu" }
            ] }
        } }"#;
        let defs = workflows_from_json(json);
        let def = defs.get("asset_import").expect("definition loads");
        assert_eq!(def.steps.len(), 2);
        let mut w = Workflow::from_def(def);
        assert_eq!(w.step(), "task");
        let doc = ValueMap::new().with("source", "s");
        w.handle(&fired("wf_next"), &doc);
        assert_eq!(w.step(), "review");

        assert!(
            workflows_from_json("not json").is_empty(),
            "a bad file warns, never panics"
        );
    }
}
