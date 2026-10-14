//! **The panel's roster, as data.**
//!
//! The stable id, params, binds, actions and gate names shared by the authored tree
//! (`scenes/model_view.scene.json`), the Model this scene publishes, its pair script
//! (`scripts/model_view.lua`) and the ONE dispatcher — so the four cannot drift apart.
//! This module BUILDS nothing: the panel's chrome is authored as data.

use flicker_rigview::Projection;

/// The scene's id — ONE name in four places: the file name under `scenes/`, the `scene`
/// a `surface` node names, the file's `behaviour`, and the prism-alpha roster entry id.
pub const ID: &str = "model_view";

/// The authored scene, shipped with the crate. Embedded for the DRIFT GATES only: nothing
/// in this crate reads the file at runtime — the manifest indexes it with every other
/// scene and hands its parsed def to [`scene`](crate::scene) at root depth, or, through
/// `flicker_shell::scene_def`, to the scene that seats it in one of its `surface` nodes.
#[cfg(test)]
pub const SCENE: &str = include_str!("../../../../content/sensorium/scenes/model_view.scene.json");
/// The pair script, compiled in and loaded at runtime exactly as every bench loads its own
/// (`ScriptHost::new(SCRIPT, SCRIPT_NAME)` in [`ModelView::new`](crate::ModelView::new)).
pub const SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/model_view.lua");
/// The chunk name the pair script loads under (`SceneName.lua`, the 1:1 pair).
pub const SCRIPT_NAME: &str = "model_view.lua";

/// The ONE stage the panel's picture draws under — the studio-lit rig recipe the scene
/// file authors (`stages.rig`). Seven near-identical `rig_*` blocks collapse into it.
pub const STAGE: &str = "rig";

/// The panel's own focus group: its chrome is navigable inside the panel, which is the
/// whole of this scene's screen.
pub const PANE: &str = "model_view";

// ── Params (the host's per-instance knobs) ──────────────────────────────────

/// Which projection this instance shows — one of [`PROJECTIONS`]' names.
pub const P_PROJECTION: &str = "projection";
/// Whether this instance may show the isolate row. Default: whether it is orthographic.
pub const P_CHROME: &str = "chrome";
/// Whether this instance may show its corner label (and so its flip control).
pub const P_LABEL: &str = "label";

/// The projection a `projection` param names, and the panel it builds. `side` is the
/// editor's LEFT quad (its flip shows RIGHT) — the authored name is the picture's, the
/// enum's is the camera's.
pub const PROJECTIONS: [(&str, Projection); 4] = [
    ("persp", Projection::Perspective),
    ("top", Projection::Top),
    ("side", Projection::Left),
    ("front", Projection::Front),
];

/// The projection an authored name selects, or `None` when the name is not one of
/// [`PROJECTIONS`] — the caller warns and falls back.
#[must_use]
pub fn projection_of(name: &str) -> Option<Projection> {
    PROJECTIONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, p)| *p)
}

/// The authored name of a projection — what this scene publishes as `Model.projection`
/// for the script to read. The inverse of [`projection_of`], from the one table.
#[must_use]
pub fn projection_name(projection: Projection) -> &'static str {
    PROJECTIONS
        .iter()
        .find(|(_, p)| *p == projection)
        .map_or("persp", |(n, _)| *n)
}

// ── The Model this scene publishes (what the script reads) ──────────────────

/// The projection's authored NAME (`Model.projection`).
pub const M_PROJECTION: &str = "projection";
/// Whether an orthographic panel views from the opposite side (`Model.flipped`).
pub const M_FLIPPED: &str = "flipped";
/// The `chrome` / `label` params, echoed so the script's gates read one source.
pub const M_CHROME: &str = "chrome";
pub const M_LABEL: &str = "label";

// ── Two-way binds (published each frame, echoed back from the results) ──────

/// Isolate this panel's near-side limbs.
pub const LIMB: &str = "limb";
/// Cut everything behind the cull plane.
pub const CULL: &str = "cull";
/// Where that cut sits, 0..1 of the subject's depth from the panel's near face.
pub const CULL_AT: &str = "cull_at";

/// The corner label's caption, derived by the script from the projection + the flip.
pub const VIEW_LABEL: &str = "view_label";

// ── Gates `arrange()` lights ────────────────────────────────────────────────

/// The isolate row is shown (an orthographic panel whose host asked for chrome).
pub const CHROME_ON: &str = "chrome_on";
/// The corner label is shown (an orthographic panel whose host asked for a label).
pub const LABEL_ON: &str = "label_on";

// ── Actions ─────────────────────────────────────────────────────────────────

/// View from the opposite side — the corner label IS the flip control.
pub const FLIP: &str = "flip";

// ── Node ids (what the containment gate measures) ───────────────────────────

pub const NODE_FLIP: &str = "mv_flip";
pub const NODE_ISOLATE: &str = "mv_isolate";
pub const NODE_LIMB: &str = "mv_limb";
pub const NODE_CULL: &str = "mv_cull";
pub const NODE_CULL_AT: &str = "mv_cull_at";
