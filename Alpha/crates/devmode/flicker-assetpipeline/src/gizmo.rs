//! **The bench's half of the gadget** — which joint is selected, what a `GadgetDelta` means to
//! the [`Document`], and the perspective deform test's spring-back. Everything else — the handles,
//! the press table, the pick tolerance, the drag math, the snap accumulator and the mirror guard —
//! belongs to [`flicker_rigview::Gadget`] (contract 7811D68B), which this module drives from the
//! panels' own [`PanelFacts`] — a projection, a ray and a pointer sample, never a device read
//! (rule 985A1F73) and never a panel object.
//!
//! Since H3 each panel is a `model_view` SUB SCENE (ruling EBDB3518), so the facts arrive
//! from the child through its typed channel and are already CLEAN: a press the panel's own
//! chrome claimed — its corner label / flip button, its isolate row — never reaches the child's
//! root surface, so it is never in the sample this module sees. There is no label case to skip
//! here any more; the panel answers its own chrome inside its own tree.
//!
//! ## What a press means (Aaron's table, 2026-09-07)
//! The handles are the manipulator (they used to be decorative — review 402A4B93 finding 5), so
//! ONE gadget shared by the four panels answers every press — but the two projections read a
//! press differently, because the PERSPECTIVE panel is where joints are CHOSEN and the three
//! ORTHOGRAPHIC panels are where the chosen joint is PLACED:
//! 1. **PERSPECTIVE — a handle**, out along its shaft — the gadget's own [`Press::Axis`]: begin
//!    its drag, or, in Flip mode, mirror about that axis and stay put. The one exclusion is the
//!    PIVOT's own ball, where all three shafts meet and no axis is meant: a press there is a pick.
//! 2. **PERSPECTIVE — a joint** within tolerance: select it, and if the pointer goes on moving,
//!    that is the joint's REACH test (below). Empty space is the camera's (orbit).
//! 3. **ORTHOGRAPHIC — anywhere**: never a pick, never a jump, never a handle. The press begins
//!    the gadget's FREE view-plane drag of the SELECTED joint (`Gadget::begin_free`): it moves
//!    from where it is by however far the pointer travels (Aaron: "click and drag the joint from
//!    where it is currently to a new position"), and a click that travels nowhere moves nothing.
//!    No handles are drawn in an ortho panel — a press beside the joint used to land on a shaft
//!    and lock the drag to that axis ("the side view does not allow for up down manipulation"),
//!    and the `off X/Y/Z` dials remain the exact-axis tool. The ortho camera pans on the RIGHT
//!    button only (the filler's rule), so a left press can never wander the view. Before this
//!    table an ortho press re-picked whatever joint stacked under the pointer along the depth
//!    axis and then handed the held button to the camera — "dropping the selection of the
//!    joint and moving the camera".
//!
//! ## What a drag means
//! The panel decides (ruling 985A6850): the PERSPECTIVE panel runs a DEFORM TEST and springs
//! back on release; an ORTHOGRAPHIC panel REPOSITIONS the rest skeleton for good. The perspective
//! test is IK-STYLE for Translate ([`DragMode::Reach`], `Document::pose_reach`): the joint reaches
//! for the pointer and the bones above it TURN — the way an animation moves a joint — so the
//! skinning is judged under a bend, never a stretch; a joint with nothing above it to bend (a
//! hip, the first spine link) keeps the plain offset deform ([`DragMode::Deform`]), as Rotate and
//! Scale do. `Gadget::cancel` deliberately un-applies nothing, because restoring a document is
//! document business; the restore value (or the live pose) rides the drag here.

use flicker::ui::SurfacePointer;
use flicker_globe::Arrows;
use flicker_mechanics::{closest_point_ray_segment, Axis, GadgetModes, GizmoMode};
use flicker_rigview::{Gadget, GadgetDelta, GadgetStyle, Press, Projection};
use glam::{Mat3, Mat4, Vec3};

use crate::services::{BoneOffset, Document};

/// WHAT ONE PANEL IS THIS FRAME, as far as the manipulator is concerned — the whole of what
/// the gizmo needs and the whole of what a panel owes it.
///
/// A panel is a sub scene now, so these three come off its typed channel
/// (`ModelView::projection` / `ray` / `pointer`) rather than off a `RigView` the bench holds:
/// the gizmo can no longer reach into a panel, and does not need to.
#[derive(Clone, Debug)]
pub(crate) struct PanelFacts {
    /// Which view this is — the one thing that decides what a press and a drag MEAN.
    pub(crate) projection: Projection,
    /// The world ray through this frame's pointer, `None` while the cursor is elsewhere or
    /// the panel's own chrome claimed it.
    pub(crate) ray: Option<(Vec3, Vec3)>,
    /// The panel's root-surface sample — press edge, capture and buttons.
    pub(crate) pointer: Option<SurfacePointer>,
}

/// A JOINT pick lands within this fraction of the gadget's handle length. (The gadget owns the
/// tolerance for its own HANDLES; this is the bench's, for the thing only the bench can pick.)
const JOINT_TOL_FRAC: f32 = 0.35;

/// Snap steps, each in its mode's own currency — what the `gizmo_snap` checkbox turns on.
/// A centimetre of travel, the CAD-standard 15° of turn, and a tenth of a scale ratio.
const SNAP_TRANSLATE: f32 = 1.0;
const SNAP_ROTATE: f32 = 15.0;
const SNAP_SCALE: f32 = 0.1;

/// The four modes the bench's radios offer.
///
/// The first three ARE [`GizmoMode`]'s continuous drags. The fourth is the discrete mirror, which
/// deliberately is NOT a `GizmoMode` variant: adding one is a breaking change to an enum that
/// `flicker-mechanics` and the gadget both match on (deadend 7F44380D, still standing), and the
/// shipped design puts the fourth mode where it costs nothing — `GadgetModes::FLIP` plus
/// [`Gadget::flip`]. So Flip is a mode of the BENCH, and it borrows Translate's arrows as the
/// axis a mirror is taken about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GizmoUi {
    #[default]
    Translate,
    Rotate,
    Scale,
    Flip,
}

impl GizmoUi {
    /// The radio value for this mode, and back — `crate::ui::GIZMO_VALUES` is the authored list.
    pub(crate) fn value(self) -> &'static str {
        crate::ui::GIZMO_VALUES[self as usize]
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        [Self::Translate, Self::Rotate, Self::Scale, Self::Flip]
            .into_iter()
            .find(|m| m.value() == s)
    }

    /// The gadget drag mode this UI mode puts the gadget in. Flip has no drag of its own, so it
    /// shows Translate's arrows and presses them to pick the mirror axis.
    fn drag_mode(self) -> GizmoMode {
        match self {
            Self::Translate | Self::Flip => GizmoMode::Translate,
            Self::Rotate => GizmoMode::Rotate,
            Self::Scale => GizmoMode::Scale,
        }
    }

    /// This mode's snap step in its own currency (distance / degrees / ratio).
    fn snap(self) -> f32 {
        match self {
            Self::Translate | Self::Flip => SNAP_TRANSLATE,
            Self::Rotate => SNAP_ROTATE,
            Self::Scale => SNAP_SCALE,
        }
    }
}

/// What a live drag means to the document — the one thing the panel's projection decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DragMode {
    /// The perspective TEST in a handle's own currency (Rotate, Scale — and Translate on a joint
    /// with no chain to bend): the authored offset moves, and `restore` puts it back on release.
    Deform { restore: BoneOffset },
    /// The perspective TEST, IK-style: the joint reaches for `target` from `origin` (its rest
    /// position at the press) and its chain bends to follow. `free` is the view-plane drag a press
    /// ON the joint began — the anchor hit and the plane normal — and `None` when a Translate
    /// handle's axis feeds the target instead. Release clears the pose.
    Reach {
        origin: Vec3,
        target: Vec3,
        free: Option<(Vec3, Vec3)>,
    },
    /// The orthographic EDIT: the rest skeleton moves, permanently.
    Reposition,
}

/// Where `ray` meets the plane through `on` with normal `n` (`None` when it runs along it).
fn plane_hit((o, d): (Vec3, Vec3), on: Vec3, n: Vec3) -> Option<Vec3> {
    let denom = d.dot(n);
    if denom.abs() < 1e-6 {
        return None;
    }
    Some(o + d * ((on - o).dot(n) / denom))
}

/// The joint nearest the ray within `tol` — the pick the gadget cannot make.
pub(crate) fn nearest_joint(
    ray: (Vec3, Vec3),
    joints: impl Iterator<Item = Vec3>,
    tol: f32,
) -> Option<usize> {
    let (o, d) = ray;
    let mut best: Option<(usize, f32)> = None;
    for (i, c) in joints.enumerate() {
        let (pr, ps) = closest_point_ray_segment(o, d, c, c);
        let miss = (pr - ps).length();
        if best.is_none_or(|(_, bd)| miss < bd) {
            best = Some((i, miss));
        }
    }
    best.filter(|&(_, miss)| miss <= tol).map(|(i, _)| i)
}

/// The bench's gadget seat: ONE [`Gadget`] for the four panels, the radios' mode, the snap and
/// auto-depth toggles, and which panel owns the live drag.
pub(crate) struct Gizmo {
    gadget: Gadget,
    ui: GizmoUi,
    snap: bool,
    auto_depth: bool,
    drag: Option<(usize, DragMode)>,
}

impl Default for Gizmo {
    fn default() -> Self {
        Self {
            gadget: Gadget::default(),
            ui: GizmoUi::default(),
            snap: false,
            // AUTO DEPTH ships ON (ruling F9F728CA): the guided rig's promise is that an ortho
            // drag places the joint in three axes, not the two the picture shows.
            auto_depth: true,
            drag: None,
        }
    }
}

impl Gizmo {
    /// The modes this step allows — the Lua gate, mapped by `flicker_rigview::modes_from_names`.
    /// A gate that no longer allows the radios' mode moves the bench to whatever the gadget fell
    /// back to, so the two can never disagree about which mode is live.
    pub(crate) fn set_modes(&mut self, modes: GadgetModes) {
        self.gadget.set_modes(modes);
        if !self.allows(self.ui) {
            self.ui = match self.gadget.mode() {
                GizmoMode::Translate => GizmoUi::Translate,
                GizmoMode::Rotate => GizmoUi::Rotate,
                GizmoMode::Scale => GizmoUi::Scale,
            };
        }
        self.gadget.set_mode(self.ui.drag_mode());
    }

    /// The radios' mode. Refused (and left alone) when the step's gate forbids it.
    pub(crate) fn set_ui_mode(&mut self, ui: GizmoUi) {
        if ui == self.ui || !self.allows(ui) {
            return;
        }
        if self.gadget.set_mode(ui.drag_mode()) {
            self.ui = ui;
        }
    }

    /// Does the step's gate allow this mode? Flip needs BOTH its own bit and Translate's, because
    /// it has no handles of its own — it picks its mirror axis off the translate arrows.
    fn allows(&self, ui: GizmoUi) -> bool {
        let modes = self.gadget.modes();
        match ui {
            GizmoUi::Flip => modes.allows_flip() && modes.allows(GizmoMode::Translate),
            other => modes.allows(other.drag_mode()),
        }
    }

    /// The mode the radios show — after any refusal, so a gated-off radio cannot lie.
    pub(crate) fn ui_mode(&self) -> GizmoUi {
        self.ui
    }

    /// The `gizmo_snap` checkbox, both ways.
    pub(crate) fn set_snap(&mut self, on: bool) {
        self.snap = on;
    }

    pub(crate) fn snapping(&self) -> bool {
        self.snap
    }

    /// The `auto_depth` checkbox, both ways (ruling F9F728CA).
    pub(crate) fn set_auto_depth(&mut self, on: bool) {
        self.auto_depth = on;
    }

    pub(crate) fn auto_depth(&self) -> bool {
        self.auto_depth
    }

    /// This panel's handle overlay, for its `ViewContext::overlay` — the PERSPECTIVE panel's
    /// only: an orthographic panel's every press is the free drag, so it draws no handle it
    /// would not honour.
    pub(crate) fn handle_lines(&self, projection: Projection, style: &GadgetStyle) -> Arrows {
        if projection.is_ortho() {
            return Arrows::new();
        }
        self.gadget.handle_lines(projection, style)
    }

    /// Run one frame against the panels' [`PanelFacts`]. `active` is the Rig step with a rig
    /// loaded; `radius` the subject's framing radius (handle length and both tolerances follow).
    /// Returns the panel whose pointer the gadget consumed — the bench holds that panel's
    /// camera still (`ViewContext::camera_held`) while the drag is the manipulator's.
    pub(crate) fn interact(
        &mut self,
        doc: &mut Document,
        panels: &[PanelFacts],
        active: bool,
        radius: f32,
    ) -> Option<usize> {
        let (sel, globals) = (doc.bone_sel(), doc.parsed().map(|p| p.globals.clone()));
        let (Some(sel), Some(globals), true) = (sel, globals, active) else {
            self.gadget.cancel();
            if matches!(self.drag, Some((_, DragMode::Reach { .. }))) {
                doc.clear_pose();
            }
            self.drag = None;
            return None;
        };
        let pivot = globals
            .get(sel)
            .map(|g| g.w_axis.truncate())
            .unwrap_or(Vec3::ZERO);
        self.gadget.set_frame(pivot, Mat3::IDENTITY, radius);

        // Continue (or release) the live drag.
        if let Some((panel, mode)) = self.drag {
            let ptr = panels.get(panel).and_then(|f| f.pointer.as_ref());
            if !ptr.is_some_and(|p| p.captured && p.left) {
                match mode {
                    DragMode::Deform { restore } => doc.restore_offset(sel, restore),
                    DragMode::Reach { .. } => doc.clear_pose(),
                    // THE ORTHO DRAG'S SECOND HALF (spec 76EB9552): the picture placed the joint
                    // in the two axes it can show; THE RELEASE resolves the third — the one it
                    // cannot — to the middle of the body mass, the joint and its mirrored twin
                    // each in their own column. On the release edge only (B694F6B1), and only
                    // when the drag actually moved the joint: a click that travelled nowhere
                    // places nothing, here as everywhere (incident E4C6CED5). The `auto_depth`
                    // checkbox switches the whole resolve off (ruling F9F728CA) — the hand keeps
                    // the depth it dragged with.
                    DragMode::Reposition => {
                        let moved = self.gadget.moved();
                        let depth = panels
                            .get(panel)
                            .and_then(|f| f.projection.depth_axis())
                            .filter(|_| self.auto_depth && moved);
                        if let Some(depth) = depth {
                            doc.resolve_drag_depth(sel, depth);
                        }
                        // THE MARKERS RAIL IS NOT MOVED HERE (incident 9715303C): placing a joint
                        // and accepting it are two acts, so a mis-drag or a stray press can never
                        // walk the rail on. The drag has already marked what it moved; ACCEPT is
                        // the only way forward.
                    }
                }
                self.gadget.end();
                self.drag = None;
                return None;
            }
            if let Some(ray) = panels.get(panel).and_then(|f| f.ray) {
                match mode {
                    // The joint's own drag: the pointer's spot on the press plane IS the target.
                    DragMode::Reach {
                        origin,
                        free: Some((anchor, normal)),
                        ..
                    } => {
                        if let Some(hit) = plane_hit(ray, anchor, normal) {
                            let target = origin + (hit - anchor);
                            doc.pose_reach(sel, target);
                            self.drag = Some((
                                panel,
                                DragMode::Reach {
                                    origin,
                                    target,
                                    free: Some((anchor, normal)),
                                },
                            ));
                        }
                    }
                    // A Translate handle feeds the target along its axis.
                    DragMode::Reach {
                        origin,
                        target,
                        free: None,
                    } => {
                        if let Some(GadgetDelta::Translate(v)) = self.gadget.update(ray) {
                            let target = target + v;
                            doc.pose_reach(sel, target);
                            self.drag = Some((
                                panel,
                                DragMode::Reach {
                                    origin,
                                    target,
                                    free: None,
                                },
                            ));
                        }
                    }
                    other => {
                        if let Some(delta) = self.gadget.update(ray) {
                            apply(doc, sel, &globals, other, delta);
                        }
                    }
                }
            }
            return Some(panel);
        }

        // AIM: the panel the pointer is over pre-highlights the handle under it.
        match panels.iter().find(|f| f.pointer.is_some()) {
            Some(f) => self.gadget.pick(f.projection, f.ray),
            None => self.gadget.pick(Projection::Perspective, None),
        };

        // A fresh press, decided by the one rule the module docs describe.
        let joint_tol = self.gadget.size() * JOINT_TOL_FRAC;
        let snap = self.snap.then(|| self.ui.snap());
        for (panel, facts) in panels.iter().enumerate() {
            let Some(_) = facts.pointer.as_ref().filter(|p| p.pressed && p.left) else {
                continue;
            };
            // No chrome case to skip: a press the panel's own tree claimed never reaches its
            // root surface, so a sample that got this far is the picture's.
            let Some(ray) = facts.ray else {
                continue;
            };
            let projection = facts.projection;
            // ORTHOGRAPHIC: the press IS the free drag of the selected joint — no pick, no jump,
            // no handle — from where the joint is, by the pointer's travel.
            if projection.is_ortho() {
                if self.ui != GizmoUi::Flip && self.gadget.begin_free(ray, snap) {
                    self.drag = Some((panel, DragMode::Reposition));
                    return Some(panel);
                }
                continue;
            }
            // Inside the pivot's own ball every handle shaft is equidistant, so no axis is
            // meant there: that press is a selection, and only OUTSIDE it can a handle win.
            let on_pivot = nearest_joint(ray, std::iter::once(pivot), joint_tol).is_some();
            let press = self.gadget.decide(projection, ray);
            let grabbed = match press {
                Press::Axis(axis) if !on_pivot => Some(axis),
                _ => None,
            };
            if let Some(axis) = grabbed {
                // A mirror needs an axis, so Flip mode takes a handle and nothing else.
                if self.ui == GizmoUi::Flip {
                    self.mirror(doc, sel, &globals, axis);
                    return Some(panel);
                }
                if self.gadget.begin(projection, ray, snap) {
                    self.drag = Some((panel, drag_mode(doc, sel, projection, self.ui, pivot)));
                    return Some(panel);
                }
            }
            // The joint pick — the perspective panel's alone — and the reach test a held press
            // on that joint becomes: its plane faces the press ray through the joint.
            let joints = globals.iter().map(|g| g.w_axis.truncate());
            if let Some(i) = nearest_joint(ray, joints, joint_tol) {
                doc.select_bone(i);
                if self.ui == GizmoUi::Translate && doc.reach_chain(i).is_some() {
                    let origin = globals[i].w_axis.truncate();
                    let normal = ray.1.normalize_or_zero();
                    if let Some(anchor) = plane_hit(ray, origin, normal) {
                        self.drag = Some((
                            panel,
                            DragMode::Reach {
                                origin,
                                target: origin,
                                free: Some((anchor, normal)),
                            },
                        ));
                    }
                }
                return Some(panel);
            }
            // Empty space in the perspective panel: the camera's.
        }
        None
    }

    /// The discrete mirror about `axis`, through the gadget's guard. The validator is the bench's
    /// domain answer: a joint with no `_l`/`_r` twin has nowhere to reflect to, so the op is
    /// REFUSED — no partial write, and the handle draws dead (invariant C670523A's per-axis tier).
    ///
    /// About X — the body's median plane (the gadget's basis is the world's) — the mirror is
    /// MIRROR → (`Document::mirror_to_twin`, ruling 380BDCC8): the twin subtree is put at the
    /// reflection of the selected one's WORLD positions, so an asymmetry the two sides had is
    /// gone rather than carried. About Y or Z there is no twin plane, so the gadget's own
    /// reflection of the authored offset stands.
    fn mirror(&mut self, doc: &mut Document, sel: usize, globals: &[Mat4], axis: Axis) {
        let has_twin = doc.mirror_of(sel).is_some();
        if let Some(GadgetDelta::Flip(m)) = self.gadget.flip(axis, |_| has_twin) {
            if axis == Axis::X {
                doc.mirror_to_twin(sel);
            } else {
                doc.mirror_offset(sel, globals, m);
            }
        }
    }
}

/// What a handle drag started in this panel means to the document: an ortho panel repositions;
/// the perspective panel reaches (Translate, on a joint with a chain to bend) or deforms.
fn drag_mode(
    doc: &Document,
    sel: usize,
    projection: Projection,
    ui: GizmoUi,
    pivot: Vec3,
) -> DragMode {
    if projection != Projection::Perspective {
        return DragMode::Reposition;
    }
    if ui == GizmoUi::Translate && doc.reach_chain(sel).is_some() {
        return DragMode::Reach {
            origin: pivot,
            target: pivot,
            free: None,
        };
    }
    DragMode::Deform {
        restore: doc.selected_offset().unwrap_or_default(),
    }
}

/// One frame's [`GadgetDelta`] onto the document.
///
/// Translate is the one currency the panel splits: the perspective test DEFORMS (the authored
/// offset moves, and springs back) where an orthographic drag REPOSITIONS the rest skeleton.
/// Rotate and Scale write the authored offset in both panels — the rest pose has no rotation or
/// scale editor to reposition, so the offset is the document's one consumer for each.
fn apply(doc: &mut Document, sel: usize, globals: &[Mat4], mode: DragMode, delta: GadgetDelta) {
    match delta {
        GadgetDelta::Translate(v) => match mode {
            DragMode::Deform { .. } => doc.apply_gizmo_delta(sel, globals, v),
            DragMode::Reposition => doc.reposition_bone(sel, globals, v),
            DragMode::Reach { .. } => {} // fed by `interact` itself, never through here
        },
        GadgetDelta::Rotate(q) => doc.apply_gizmo_rotate(sel, globals, q),
        GadgetDelta::Scale(s) => doc.apply_gizmo_scale(sel, s),
        GadgetDelta::Flip(m) => {
            doc.mirror_offset(sel, globals, m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::render::{Rate, Rect, ViewportLayout};
    use flicker::ui::SurfaceSlot;
    use flicker_rigview::gadget::modes_from_names;
    // The FIXTURE only: a real camera in a real seat is what turns a pointer into a ray, and
    // that is exactly what a `model_view` sub scene hands the bench through `PanelFacts`. The
    // bench itself owns no panel any more.
    use flicker_rigview::RigView;
    use glam::{Quat, Vec2};
    // The guided rig's own field, to assert the bench's depth read against (spec 76EB9552).
    use flicker_content::Flesh;

    /// The Rig step's gate, as `assetpipeline.lua` publishes it.
    fn rig_modes() -> GadgetModes {
        modes_from_names(crate::ui::GIZMO_VALUES)
    }

    /// A seated panel of `projection`, framed on the document's subject.
    fn panel(doc: &Document, projection: Projection) -> RigView {
        let styles = serde_json::json!({ "stages": { "rig_test": { "lighting": "studio" } } });
        let mut v = RigView::new("rig_test", &styles, projection);
        let f = crate::compose::framing(doc);
        v.set_frame(f.centre, f.radius);
        v.seat(Some(&SurfaceSlot {
            id: "p".into(),
            source: "rig_test".into(),
            scene: String::new(),
            params: Default::default(),
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 400.0,
            layer: 0.0,
            rate: Rate::Live,
            tint: [1.0; 4],
            layout: ViewportLayout::Single,
        }));
        v.set_lines(Arrows::new());
        v
    }

    /// The panel-local pixel a world point projects to.
    fn local_of(v: &RigView, world: Vec3) -> Vec2 {
        let cam = v.camera();
        let ndc = cam.view_projection(1.0).project_point3(world);
        Vec2::new((ndc.x * 0.5 + 0.5) * 400.0, (0.5 - ndc.y * 0.5) * 400.0)
    }

    fn pointer(local: Vec2, pressed: bool, held: bool, delta: Vec2) -> SurfacePointer {
        SurfacePointer {
            id: "p".into(),
            root: false,
            cursor: local,
            local,
            delta,
            left: pressed || held,
            right: false,
            pressed,
            wheel: 0.0,
            captured: held,
            rect: Rect {
                pos: Vec2::ZERO,
                size: Vec2::new(400.0, 400.0),
            },
        }
    }

    /// The panels' [`PanelFacts`] for one frame — what a `model_view` sub scene hands the bench
    /// through its typed channel (`projection()` / `ray()` / `pointer()`), built here from the
    /// `RigView` fixtures because a ray needs a real camera in a real seat.
    fn facts(panels: &[RigView], ptrs: &[Option<SurfacePointer>]) -> Vec<PanelFacts> {
        panels
            .iter()
            .zip(ptrs)
            .map(|(v, p)| PanelFacts {
                projection: v.projection(),
                ray: v.ray_at(p.as_ref()),
                pointer: p.clone(),
            })
            .collect()
    }

    /// A gizmo on the Rig step's gate, already in `ui` mode.
    fn seated(ui: GizmoUi) -> Gizmo {
        let mut g = Gizmo::default();
        g.set_modes(rig_modes());
        g.set_ui_mode(ui);
        assert_eq!(g.ui_mode(), ui, "the Rig gate allows every authored mode");
        g
    }

    /// The panel-local pixel of the framed pivot's `axis` handle, out along its shaft past the
    /// joint balls — where the gadget's pick wins and [`JOINT_TOL_FRAC`] no longer reaches.
    fn handle_at(g: &Gizmo, v: &RigView, axis: Axis) -> Vec2 {
        local_of(v, g.gadget.pivot() + axis.unit() * g.gadget.size() * 0.7)
    }

    /// A document at the Rig step with `target` selected, and its framing radius.
    fn rigged(tag: &str) -> (Document, usize, f32) {
        let mut doc = crate::tests::synthetic_rigged_doc(tag);
        // The HEAD: on the plumb line, clear of every other joint ball in any projection. The
        // old pick — the middle of the bone list, a finger joint — coincides with its
        // neighbours now that the hand chain lies down the mesh's fingers (2026-09-07), and a
        // press between coincident balls is a coin toss.
        let target = doc
            .parsed()
            .unwrap()
            .model
            .bones
            .iter()
            .position(|b| b.name == "head")
            .expect("the canon has a head");
        assert!(doc.select_bone(target));
        let radius = crate::compose::framing(&doc).radius;
        (doc, target, radius)
    }

    /// PERSPECTIVE: a press on a joint selects it; a press on a HANDLE then reaches (the head's
    /// chain bends to follow the axis), and the release springs the pose back with the authored
    /// offset untouched — the ephemeral deform test (ruling 985A6850, IK-style since 2026-09-07).
    #[test]
    fn a_perspective_drag_deforms_and_springs_back() {
        let (mut doc, target, radius) = rigged("gizmo_persp");
        let panels = [panel(&doc, Projection::Perspective)];
        let joint = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let mut g = seated(GizmoUi::Translate);

        // The press on the joint selects it (and nothing drags yet).
        doc.select_bone(0);
        let at = local_of(&panels[0], joint);
        let owned = g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
            true,
            radius,
        );
        assert_eq!(owned, Some(0), "the press is the gizmo's");
        assert_eq!(
            doc.bone_sel(),
            Some(target),
            "the pressed joint is selected"
        );

        // An idle frame re-frames the gadget on the joint just selected — that is when its
        // handles appear, so the press below can land on one.
        g.interact(&mut doc, &facts(&panels, &[None]), true, radius);
        assert!(
            (g.gadget.pivot() - joint).length() < 1e-3,
            "the gadget followed the selection"
        );

        // The press on its X handle begins the deform test.
        let before = doc.selected_offset().unwrap();
        let rest = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let grab = handle_at(&g, &panels[0], Axis::X);
        let owned = g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(grab, true, false, Vec2::ZERO))]),
            true,
            radius,
        );
        assert_eq!(owned, Some(0), "the handle press is the gadget's");
        assert!(
            g.drag.is_some(),
            "and it began a drag rather than re-selecting"
        );

        let dragged = grab + Vec2::new(40.0, 0.0);
        let owned = g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(dragged, false, true, Vec2::new(40.0, 0.0)))],
            ),
            true,
            radius,
        );
        assert_eq!(owned, Some(0), "the drag holds the pointer");
        let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (moved - rest).length() > 0.5,
            "the joint followed the pointer: {rest} → {moved}"
        );

        let owned = g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))]),
            true,
            radius,
        );
        assert_eq!(owned, None, "released");
        assert_eq!(
            doc.selected_offset().unwrap(),
            before,
            "the deform test springs back"
        );
        let back = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (back - rest).length() < 1e-3,
            "the joint is home again: {back} vs {rest}"
        );
    }

    /// PERSPECTIVE, THE REACH TEST (Aaron 2026-09-07): a press ON the selected joint that goes on
    /// moving drags the joint after the pointer by BENDING its chain — the grandparent stays
    /// put (it turns about itself), the joint closes on the pointer's spot, and the release
    /// springs the whole chain home without touching any authored offset.
    #[test]
    fn a_perspective_joint_drag_reaches_with_its_chain() {
        let (mut doc, target, radius) = rigged("gizmo_reach");
        let panels = [panel(&doc, Projection::Perspective)];
        let chain = doc
            .reach_chain(target)
            .expect("the head has a neck and a spine to bend");
        let root_of_chain = *chain.last().unwrap();
        let rest = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let anchor_rest = doc.parsed().unwrap().globals[root_of_chain]
            .w_axis
            .truncate();
        let offset_before = doc.selected_offset().unwrap();
        let mut g = seated(GizmoUi::Translate);
        let at = local_of(&panels[0], rest);
        let owned = g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
            true,
            radius,
        );
        assert_eq!(owned, Some(0), "the press on the joint is the gizmo's");
        assert!(
            matches!(g.drag, Some((0, DragMode::Reach { free: Some(_), .. }))),
            "and it armed the reach test on the joint itself: {:?}",
            g.drag
        );

        let dragged = at + Vec2::new(30.0, 0.0);
        let owned = g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(dragged, false, true, Vec2::new(30.0, 0.0)))],
            ),
            true,
            radius,
        );
        assert_eq!(owned, Some(0), "the drag holds the pointer");
        let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (moved - rest).length() > 0.5,
            "the joint followed the pointer: {rest} → {moved}"
        );
        let anchor_now = doc.parsed().unwrap().globals[root_of_chain]
            .w_axis
            .truncate();
        assert!(
            (anchor_now - anchor_rest).length() < 1e-3,
            "the chain's top turned about itself: {anchor_rest} vs {anchor_now}"
        );
        let Some((_, DragMode::Reach { target: want, .. })) = g.drag else {
            panic!("still reaching");
        };
        assert!(
            (moved - want).length() < (rest - want).length(),
            "the joint closed on the pointer's spot: {want} (was {rest}, now {moved})"
        );
        assert_eq!(
            doc.selected_offset().unwrap(),
            offset_before,
            "a reach authors nothing"
        );

        let owned = g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))]),
            true,
            radius,
        );
        assert_eq!(owned, None, "released");
        let back = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (back - rest).length() < 1e-3,
            "the chain sprang home: {back} vs {rest}"
        );
    }

    /// ORTHOGRAPHIC NEVER PICKS AND NEVER JUMPS (Aaron 2026-09-07: "click and drag the joint from
    /// where it is currently to a new position"): a press on ANOTHER joint's pixel keeps the
    /// selection and moves nothing; the drag that follows carries the SELECTED joint from where
    /// it is by the pointer's travel — at its own depth along the view axis — permanently.
    #[test]
    fn an_orthographic_press_drags_the_selected_joint_instead_of_picking() {
        let (mut doc, target, radius) = rigged("gizmo_place");
        let panels = [panel(&doc, Projection::Front)];
        let globals = doc.parsed().unwrap().globals.clone();
        let rest = globals[target].w_axis.truncate();
        // Some other joint, well away from the head in the front view.
        let other = globals
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != target)
            .map(|(i, g)| (i, g.w_axis.truncate()))
            .max_by(|a, b| a.1.distance(rest).total_cmp(&b.1.distance(rest)))
            .map(|(i, _)| i)
            .unwrap();
        let other_pos = globals[other].w_axis.truncate();
        let mut g = seated(GizmoUi::Translate);
        g.interact(&mut doc, &facts(&panels, &[None]), true, radius); // frame the gadget on the head
        let at = local_of(&panels[0], other_pos);
        let owned = g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
            true,
            radius,
        );
        assert_eq!(owned, Some(0), "the press is the gizmo's");
        assert_eq!(doc.bone_sel(), Some(target), "the selection never moved");
        let pressed = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (pressed - rest).length() < 1e-3,
            "a press moves nothing: {rest} vs {pressed}"
        );
        assert!(
            matches!(g.drag, Some((0, DragMode::Reposition))),
            "and the free drag is armed on the selected joint"
        );
        // Drag up the panel: the head rises from where it was, and only in the view plane.
        let dragged = at + Vec2::new(0.0, -40.0);
        g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(dragged, false, true, Vec2::new(0.0, -40.0)))],
            ),
            true,
            radius,
        );
        let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            moved.z > rest.z + 0.5 && (moved.x - rest.x).abs() < 1e-2,
            "the head rose from where it was: {rest} → {moved}"
        );
        assert!(
            (moved.y - rest.y).abs() < 1e-3,
            "at its own depth: {} vs {}",
            moved.y,
            rest.y
        );
        g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))]),
            true,
            radius,
        );
        let kept = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (kept - moved).length() < 1e-3,
            "the move is permanent: {moved} vs {kept}"
        );
    }

    /// THE SIDE VIEW MOVES A JOINT UP AND DOWN (Aaron 2026-09-07: "the side view does not allow
    /// for up down manipulation"): in the LEFT panel a drag up the panel raises the selected
    /// joint — from a spot clear of the joint AND from a press right beside it, where a handle
    /// shaft used to catch the press and lock the drag to its own axis. The view's depth axis (X)
    /// is the one direction the DRAG never moves: while the button is down the joint rides at its
    /// own depth. (The RELEASE does move along it, by design — the guided rig's depth read, spec
    /// 76EB9552; `an_orthographic_release_resolves_the_hidden_axis_to_the_middle_of_the_mass` is
    /// that gate. So the assertion below is a MID-DRAG one, taken before the button comes up.)
    #[test]
    fn the_side_view_moves_the_joint_up_and_down() {
        let (mut doc, target, radius) = rigged("gizmo_side");
        let panels = [panel(&doc, Projection::Left)];
        let mut g = seated(GizmoUi::Translate);
        g.interact(&mut doc, &facts(&panels, &[None]), true, radius);
        let joint = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let beside = local_of(&panels[0], joint + Vec3::Y * g.gadget.size() * 0.5);
        for (name, at) in [
            ("clear of the joint", Vec2::new(4.0, 4.0)),
            ("beside the joint", beside),
        ] {
            g.interact(&mut doc, &facts(&panels, &[None]), true, radius);
            let rest = doc.parsed().unwrap().globals[target].w_axis.truncate();
            assert_eq!(
                g.interact(
                    &mut doc,
                    &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
                    true,
                    radius
                ),
                Some(0),
                "{name}: the press begins the free drag"
            );
            assert!(
                matches!(g.drag, Some((0, DragMode::Reposition))),
                "{name}: a free reposition, never an axis: {:?}",
                g.drag
            );
            let up = at + Vec2::new(0.0, -40.0);
            g.interact(
                &mut doc,
                &facts(
                    &panels,
                    &[Some(pointer(up, false, true, Vec2::new(0.0, -40.0)))],
                ),
                true,
                radius,
            );
            let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
            assert!(
                moved.z > rest.z + 0.5,
                "{name}: the drag raised the joint: {rest} → {moved}"
            );
            assert!(
                (moved.x - rest.x).abs() < 1e-3,
                "{name}: MID-DRAG, never along the view's depth axis"
            );
            g.interact(
                &mut doc,
                &facts(&panels, &[Some(pointer(up, false, false, Vec2::ZERO))]),
                true,
                radius,
            );
        }
    }

    /// ORTHOGRAPHIC: a press away from any joint and any handle, with a selection, repositions it
    /// for good — the REST skeleton moves (the authored offset is untouched) and stays moved.
    #[test]
    fn an_orthographic_drag_repositions_the_selected_joint() {
        let (mut doc, target, radius) = rigged("gizmo_ortho");
        let panels = [panel(&doc, Projection::Front)];
        let offset_before = doc.selected_offset().unwrap();
        let rest = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let gen_before = doc.pose_gen;
        let mut g = seated(GizmoUi::Translate);
        // A corner of the panel: far from every joint and every handle.
        let at = Vec2::new(4.0, 4.0);
        assert_eq!(
            g.interact(
                &mut doc,
                &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
                true,
                radius
            ),
            Some(0)
        );
        let dragged = at + Vec2::new(0.0, 30.0);
        assert_eq!(
            g.interact(
                &mut doc,
                &facts(
                    &panels,
                    &[Some(pointer(dragged, false, true, Vec2::new(0.0, 30.0)))]
                ),
                true,
                radius
            ),
            Some(0)
        );
        assert_eq!(
            g.interact(
                &mut doc,
                &facts(&panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))]),
                true,
                radius
            ),
            None
        );
        let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (moved - rest).length() > 0.5,
            "the rest joint moved: {rest} → {moved}"
        );
        assert_ne!(
            doc.pose_gen, gen_before,
            "a permanent conform edit bumps the pose"
        );
        assert_eq!(
            doc.selected_offset().unwrap(),
            offset_before,
            "the authored offset is untouched"
        );
        assert_eq!(doc.bone_sel(), Some(target));
    }

    /// The guided rig's own reading of the fixture for a MIDLINE joint: where ruling F9F728CA's
    /// rule puts it on the panel's hidden `axis` — for the fixture's ball, the symmetry plane.
    fn middle_of_the_mass(doc: &Document, joint: Vec3, axis: usize) -> Option<f32> {
        let p = doc.parsed().expect("a rigged fixture");
        Flesh::build(&p.model).limb_depth(joint, None, axis, true)
    }

    /// THE GUIDED RIG'S DEPTH READ (spec 76EB9552 slice 3, re-cut by ruling F9F728CA) — the ortho
    /// drag's second half. A LEFT-panel drag places the joint in the two axes the picture SHOWS;
    /// the RELEASE resolves the third, the one it cannot. The target is the HEAD: a MIDLINE joint
    /// standing in a body run, whose answer is the SYMMETRY PLANE (Aaron: "the app determines the
    /// correct depth in the mesh to be at the middle of the body mass" — for a midline joint on
    /// the hidden X axis that middle is x = 0). Mid-drag nothing of the kind happens: the joint
    /// rides at its own depth until the button comes up (commit-on-release B694F6B1).
    ///
    /// The SAME joint released in the FRONT panel keeps its depth instead — the hidden axis there
    /// is Y, the spine sits in the body's back third, and a body run's midpoint would haul it to
    /// mid-torso. Nothing moves that the rule cannot justify.
    #[test]
    fn an_orthographic_release_resolves_the_hidden_axis_to_the_middle_of_the_mass() {
        let (mut doc, target, radius) = rigged("gizmo_depth");
        // The twin is its own gate, below.
        doc.mirror_joints = false;
        // Push the joint OFF the middle first: the canon rig already stands on the median plane,
        // so a resolve there would move nothing and prove nothing.
        let globals = doc.parsed().unwrap().globals.clone();
        doc.reposition_bone(target, &globals, Vec3::X * 25.0);
        let panels = [panel(&doc, Projection::Left)];
        let mut g = seated(GizmoUi::Translate);
        g.interact(&mut doc, &facts(&panels, &[None]), true, radius);
        let start = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(start.x > 20.0, "the joint starts off the middle: {start}");

        // A corner of the panel: clear of every joint and every handle.
        let at = Vec2::new(4.0, 4.0);
        g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
            true,
            radius,
        );
        let dragged = at + Vec2::new(0.0, -14.0);
        g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(dragged, false, true, Vec2::new(0.0, -14.0)))],
            ),
            true,
            radius,
        );
        let mid = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (mid.z - start.z).abs() > 0.5,
            "the drag carried the joint across the picture: {start} → {mid}"
        );
        assert!(
            (mid.x - start.x).abs() < 1e-3,
            "MID-DRAG the depth is untouched: {} vs {}",
            mid.x,
            start.x
        );

        // The release edge: the depth — and only the depth — resolves.
        let want = middle_of_the_mass(&doc, mid, 0).expect("a midline joint on the fixture's body");
        g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))]),
            true,
            radius,
        );
        let after = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (after.y - mid.y).abs() < 1e-3 && (after.z - mid.z).abs() < 1e-3,
            "the two axes the picture shows stay the hand's: {mid} → {after}"
        );
        assert!(
            (after.x - want).abs() < 1e-2,
            "the release put the depth at `Flesh::limb_depth` = {want}, not {}",
            after.x
        );
        // And the fixture's own geometry says where that is: its body is a ball planted on the
        // plumb line, so the symmetry plane of any column of it IS the median plane.
        let cell = Flesh::build(&doc.parsed().unwrap().model).cell();
        assert!(
            after.x.abs() < 2.0 * cell,
            "a midline joint lands on the median plane: {} (cell {cell})",
            after.x
        );

        // FRONT: the hidden axis is Y, and a midline joint in a body run keeps what the hand gave
        // it — the horse-spine case the ruling protects.
        let before = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert_eq!(
            doc.resolve_drag_depth(target, Projection::Front.depth_axis().unwrap()),
            0,
            "a midline joint has no Y a body run can justify"
        );
        assert!(
            (doc.parsed().unwrap().globals[target].w_axis.truncate() - before).length() < 1e-4,
            "so nothing moves"
        );
    }

    /// AUTO DEPTH OFF (ruling F9F728CA): the checkbox switches the release resolve off whole, and
    /// the drag leaves the hidden axis exactly where the hand had it — the escape hatch for the
    /// placements only a human eye can make. The two axes the picture shows still land.
    #[test]
    fn auto_depth_off_leaves_the_hidden_axis_to_the_hand() {
        let (mut doc, target, radius) = rigged("gizmo_auto_off");
        doc.mirror_joints = false;
        let globals = doc.parsed().unwrap().globals.clone();
        doc.reposition_bone(target, &globals, Vec3::X * 25.0);
        let panels = [panel(&doc, Projection::Left)];
        let mut g = seated(GizmoUi::Translate);
        assert!(g.auto_depth(), "the toggle ships ON");
        g.set_auto_depth(false);
        g.interact(&mut doc, &facts(&panels, &[None]), true, radius);
        let start = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            middle_of_the_mass(&doc, start, 0).is_some_and(|x| (x - start.x).abs() > 5.0),
            "with the toggle ON this joint WOULD move: {start}"
        );

        let at = Vec2::new(4.0, 4.0);
        g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
            true,
            radius,
        );
        let dragged = at + Vec2::new(0.0, -14.0);
        g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(dragged, false, true, Vec2::new(0.0, -14.0)))],
            ),
            true,
            radius,
        );
        g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))]),
            true,
            radius,
        );
        let after = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            after.z > start.z + 0.5,
            "the drag itself still landed: {start} → {after}"
        );
        assert!(
            (after.x - start.x).abs() < 1e-4,
            "the release resolved nothing: {} vs {}",
            after.x,
            start.x
        );
    }

    /// OFF THE SILHOUETTE: a joint the hand dragged clear of the body has no column to centre in,
    /// so the release leaves its depth exactly where the hand put it — it is never pulled to some
    /// far run, and never quietly re-placed.
    #[test]
    fn a_release_off_the_body_leaves_the_depth_alone() {
        let (mut doc, target, radius) = rigged("gizmo_off_body");
        doc.mirror_joints = false;
        // Out past the body's widest point: the column through the joint is empty flesh.
        let globals = doc.parsed().unwrap().globals.clone();
        let reach = 3.0 * doc.parsed().unwrap().half_extent.x;
        doc.reposition_bone(target, &globals, Vec3::new(reach, 12.0, 0.0));
        let panels = [panel(&doc, Projection::Front)];
        let mut g = seated(GizmoUi::Translate);
        g.interact(&mut doc, &facts(&panels, &[None]), true, radius);
        let start = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            middle_of_the_mass(&doc, start, 1).is_none(),
            "the fixture's body does not reach {start}"
        );

        let at = Vec2::new(4.0, 4.0);
        g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
            true,
            radius,
        );
        let dragged = at + Vec2::new(0.0, -10.0);
        g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(dragged, false, true, Vec2::new(0.0, -10.0)))],
            ),
            true,
            radius,
        );
        g.interact(
            &mut doc,
            &facts(&panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))]),
            true,
            radius,
        );
        let after = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            after.z > start.z + 0.5,
            "the drag itself still landed: {start} → {after}"
        );
        assert!(
            (after.y - start.y).abs() < 1e-4,
            "an empty column leaves the depth alone: {} vs {}",
            after.y,
            start.y
        );
    }

    /// THE RELEASE NEVER MOVES THE MARKERS RAIL (incident 9715303C, Aaron on the Elk: *"if you
    /// click and your drag isn't right, it advances anyway"*). Placing a joint and accepting it
    /// are two acts: an ortho drag of the very joint the rail is asking for PLACES it — the joint
    /// and its mirrored twin are marked — and the release leaves the rail where it stood, so a
    /// mis-drag stays in front of the human to be fixed. A press that travels nowhere (the stray
    /// click) marks nothing. Only ACCEPT walks on, to the next joint still unplaced.
    #[test]
    fn a_release_places_the_prompted_joint_but_never_moves_the_rail() {
        let (mut doc, _, radius) = rigged("gizmo_rail");
        let asked = |doc: &Document| -> usize {
            let name = doc
                .marker_name()
                .expect("the fixture leaves joints to place");
            let bones = &doc.parsed().unwrap().model.bones;
            bones.iter().position(|b| b.name == name).unwrap()
        };
        // A SIDED prompt, so the twin half is exercised: SKIP along the rail to the first one.
        for _ in 0..doc.markers().len() {
            if doc.mirror_of(asked(&doc)).is_some() {
                break;
            }
            doc.step_marker(crate::services::MarkerStep::Skip);
        }
        let (stood, sel) = (doc.marker(), asked(&doc));
        let twin = doc.mirror_of(sel).expect("the rail prompts a sided joint");
        assert!(doc.select_bone(sel));
        let panels = [panel(&doc, Projection::Front)];
        let mut g = seated(GizmoUi::Translate);
        g.interact(&mut doc, &facts(&panels, &[None]), true, radius);
        let rest = doc.parsed().unwrap().globals[sel].w_axis.truncate();

        // THE STRAY CLICK: pressed and released on the spot.
        let at = Vec2::new(4.0, 4.0);
        for (pressed, held) in [(true, false), (false, false)] {
            g.interact(
                &mut doc,
                &facts(&panels, &[Some(pointer(at, pressed, held, Vec2::ZERO))]),
                true,
                radius,
            );
        }
        assert!(
            !doc.placed()[sel] && !doc.placed()[twin],
            "a click places nothing"
        );
        assert_eq!(doc.marker(), stood, "and moves no rail");

        // THE DRAG: pressed, carried, released.
        let dragged = at + Vec2::new(0.0, -30.0);
        for (ptr, pressed, held, delta) in [
            (at, true, false, Vec2::ZERO),
            (dragged, false, true, Vec2::new(0.0, -30.0)),
            (dragged, false, false, Vec2::ZERO),
        ] {
            g.interact(
                &mut doc,
                &facts(&panels, &[Some(pointer(ptr, pressed, held, delta))]),
                true,
                radius,
            );
        }
        // A drag that really moved the prompted joint — exactly the case the release walked on.
        let after = doc.parsed().unwrap().globals[sel].w_axis.truncate();
        assert!(
            (after - rest).length() > 0.5,
            "the drag moved the prompted joint: {rest} → {after}"
        );
        assert!(
            doc.placed()[sel] && doc.placed()[twin],
            "the drag marks the joint and its mirrored twin"
        );
        assert_eq!(
            doc.marker(),
            stood,
            "the RELEASE leaves the rail on the joint it placed"
        );

        // ACCEPT walks on — to the next joint still unplaced, past the twin the drag placed.
        assert!(doc.step_marker(crate::services::MarkerStep::NextUnplaced));
        assert_ne!(doc.marker(), stood, "ACCEPT moves the rail");
        assert!(
            !doc.placed()[asked(&doc)],
            "onto a joint that still wants a human"
        );
    }

    /// THE MIRRORED TWIN RESOLVES IN ITS OWN COLUMN: a column through one knee crosses BOTH legs,
    /// so a twin that read the selected joint's column would be placed in the wrong leg. The
    /// fixture's body is SHEARED in depth (its middle runs y = 0.4·x), so the two sides' middles
    /// are different numbers and only an own-column read finds both.
    #[test]
    fn the_mirrored_twin_resolves_its_depth_in_its_own_column() {
        const SHEAR: f32 = 0.4;
        let (mut doc, _, _) = rigged("gizmo_twin");
        {
            let p = doc
                .source
                .as_mut()
                .and_then(|s| s.parsed.as_mut())
                .expect("the fixture is parsed");
            for v in &mut p.model.vertices {
                v.p[1] += SHEAR * v.p[0];
            }
            p.geometry_changed();
            p.rebuild(&[]);
        }
        // A sided joint well off the plumb line (so the two columns differ) and deep in the flesh.
        let (sel, twin) = {
            let p = doc.parsed().unwrap();
            let flesh = Flesh::build(&p.model);
            p.globals
                .iter()
                .enumerate()
                .filter(|(_, g)| g.w_axis.truncate().x.abs() > 5.0)
                .filter_map(|(i, g)| {
                    let w = g.w_axis.truncate();
                    Some((i, doc.mirror_of(i)?, flesh.radius_at(w)))
                })
                .max_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(i, t, _)| (i, t))
                .expect("the canon rig has sided joints inside the body")
        };
        assert!(doc.mirror_joints, "symmetry is on by default");
        assert_eq!(
            doc.resolve_drag_depth(sel, Projection::Front.depth_axis().unwrap()),
            2,
            "the joint AND the twin the drag mirrored onto"
        );
        let globals = &doc.parsed().unwrap().globals;
        let (l, r) = (
            globals[sel].w_axis.truncate(),
            globals[twin].w_axis.truncate(),
        );
        let cell = Flesh::build(&doc.parsed().unwrap().model).cell();
        for (name, p) in [("the joint", l), ("its twin", r)] {
            assert!(
                (p.y - SHEAR * p.x).abs() < 2.0 * cell,
                "{name} sits in the middle of ITS OWN column (y = {SHEAR}·{}) : {p}",
                p.x
            );
        }
        assert!(
            (l.y - r.y).abs() > 2.0,
            "two columns, two different depths: {l} vs {r}"
        );
    }

    /// ROTATE reaches the document: the delta's twist about the bone's own X axis lands on the
    /// offset's roll — the value the `off_roll` dial writes, so the two never disagree.
    #[test]
    fn a_rotate_delta_turns_the_offsets_roll() {
        let (mut doc, target, _) = rigged("gizmo_rotate");
        let globals = doc.parsed().unwrap().globals.clone();
        let before = doc.selected_offset().unwrap().roll;
        let axis = globals[target].x_axis.truncate().normalize();
        let q = Quat::from_axis_angle(axis, 30f32.to_radians());
        apply(
            &mut doc,
            target,
            &globals,
            DragMode::Reposition,
            GadgetDelta::Rotate(q),
        );
        let after = doc.selected_offset().unwrap().roll;
        assert!(
            (after - before - 30.0).abs() < 0.5,
            "roll {before} → {after}, wanted +30°"
        );

        // A turn about an axis the offset cannot express contributes nothing rather than
        // inventing a second rotation channel.
        let perp = axis.any_orthonormal_vector();
        apply(
            &mut doc,
            target,
            &globals,
            DragMode::Reposition,
            GadgetDelta::Rotate(Quat::from_axis_angle(perp, 30f32.to_radians())),
        );
        assert!(
            (doc.selected_offset().unwrap().roll - after).abs() < 1e-3,
            "no second channel"
        );
    }

    /// SCALE reaches the document: the per-axis factors multiply the offset's scale (which is what
    /// `rest_globals` folds onto the bone), and the floor stops a drag through the pivot inverting
    /// the bone — mirroring is `flip`'s guarded job, not scale's.
    #[test]
    fn a_scale_delta_scales_the_bone_offset() {
        let (mut doc, target, _) = rigged("gizmo_scale");
        let globals = doc.parsed().unwrap().globals.clone();
        assert_eq!(
            doc.selected_offset().unwrap().scale,
            [1.0; 3],
            "identity is one"
        );
        apply(
            &mut doc,
            target,
            &globals,
            DragMode::Reposition,
            GadgetDelta::Scale(Vec3::new(2.0, 1.0, 1.0)),
        );
        assert_eq!(doc.selected_offset().unwrap().scale, [2.0, 1.0, 1.0]);
        apply(
            &mut doc,
            target,
            &globals,
            DragMode::Reposition,
            GadgetDelta::Scale(Vec3::new(-4.0, 1.0, 1.0)),
        );
        assert!(
            doc.selected_offset().unwrap().scale[0] > 0.0,
            "never through zero into a reflection"
        );
    }

    /// FLIP is REFUSED for a joint with no `_l`/`_r` twin: no delta, nothing written, and the
    /// gadget raises the refusal so the handle draws dead (invariant C670523A).
    #[test]
    fn a_flip_without_a_mirror_partner_is_refused_and_writes_nothing() {
        let mut doc = crate::tests::synthetic_rigged_doc("gizmo_flip");
        let globals = doc.parsed().unwrap().globals.clone();
        let bones = doc.bone_rows();
        let lone = bones
            .iter()
            .position(|(n, _)| crate::services::mirror_name(n).is_none())
            .expect("the canon rig has centre bones");
        let paired = bones
            .iter()
            .enumerate()
            .find(|(_, (n, _))| {
                crate::services::mirror_name(n).is_some_and(|m| bones.iter().any(|(o, _)| *o == m))
            })
            .map(|(i, _)| i)
            .expect("the canon rig has left/right pairs");

        let mut g = seated(GizmoUi::Flip);
        g.gadget
            .set_frame(globals[lone].w_axis.truncate(), Mat3::IDENTITY, 100.0);
        assert!(doc.select_bone(lone));
        let before = doc.selected_offset().unwrap();
        let gen_before = doc.pose_gen;
        g.mirror(&mut doc, lone, &globals, Axis::X);
        assert_eq!(
            g.gadget.refused(),
            Some(Axis::X),
            "the refusal is raised for the handle"
        );
        assert_eq!(
            doc.selected_offset().unwrap(),
            before,
            "and nothing was written"
        );
        assert_eq!(doc.pose_gen, gen_before, "not even a pose rebuild");

        // A joint that HAS a twin mirrors, and the refusal clears.
        assert!(doc.select_bone(paired));
        doc.set_selected_offset(BoneOffset {
            t: [3.0, 0.0, 0.0],
            roll: 12.0,
            scale: [1.0; 3],
        });
        let twin = doc.mirror_of(paired).expect("its twin");
        g.gadget
            .set_frame(globals[paired].w_axis.truncate(), Mat3::IDENTITY, 100.0);
        g.mirror(&mut doc, paired, &globals, Axis::X);
        assert_eq!(
            g.gadget.refused(),
            None,
            "a legal mirror clears the refusal"
        );
        // About X the mirror is MIRROR → (ruling 380BDCC8): the twin sits at the reflection of
        // the paired joint's WORLD position across the median plane, offset and all.
        let p = doc.parsed().unwrap();
        let src = p.globals[paired].w_axis.truncate();
        let dst = p.globals[twin].w_axis.truncate();
        assert!(
            (dst - Vec3::new(-src.x, src.y, src.z)).length() < 1e-2,
            "the twin at {dst} mirrors {src} across X = 0"
        );
        // About Y there is no twin plane, so the gadget's own reflection of the authored offset
        // stands: the roll reverses.
        let globals = p.globals.clone();
        g.mirror(&mut doc, paired, &globals, Axis::Y);
        assert!(doc.select_bone(twin));
        assert_eq!(
            doc.selected_offset().unwrap().roll,
            -12.0,
            "the Y reflection reverses the roll"
        );
    }

    /// SNAP quantizes the drag: with the checkbox on, the joint lands on the step grid instead of
    /// following the pointer continuously.
    #[test]
    fn snapping_quantizes_the_drag() {
        let (mut doc, target, radius) = rigged("gizmo_snap");
        let panels = [panel(&doc, Projection::Front)];
        let joint = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let mut g = seated(GizmoUi::Translate);
        g.set_snap(true);
        g.gadget.set_frame(joint, Mat3::IDENTITY, radius);
        let grab = handle_at(&g, &panels[0], Axis::Z);
        assert_eq!(
            g.interact(
                &mut doc,
                &facts(&panels, &[Some(pointer(grab, true, false, Vec2::ZERO))]),
                true,
                radius
            ),
            Some(0)
        );
        // A hair of travel is inside one snap step, so nothing moves at all.
        let nudge = grab + Vec2::new(0.0, 0.4);
        g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(nudge, false, true, Vec2::new(0.0, 0.4)))],
            ),
            true,
            radius,
        );
        let after = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!(
            (after - joint).length() < 1e-4,
            "a sub-step nudge emits nothing: {joint} → {after}"
        );
        // A long drag lands on the grid.
        let far = grab + Vec2::new(0.0, 60.0);
        g.interact(
            &mut doc,
            &facts(
                &panels,
                &[Some(pointer(far, false, true, Vec2::new(0.0, 60.0)))],
            ),
            true,
            radius,
        );
        let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let step = (moved.z - joint.z) / SNAP_TRANSLATE;
        assert!(
            (step - step.round()).abs() < 1e-2,
            "landed off the grid: {} steps",
            step
        );
    }

    /// Off the Rig step nothing is picked, and the pointer stays the camera's.
    #[test]
    fn an_inactive_gizmo_leaves_the_pointer_to_the_camera() {
        let mut doc = crate::tests::synthetic_rigged_doc("gizmo_off");
        let panels = [panel(&doc, Projection::Perspective)];
        let joint = doc.parsed().unwrap().globals[3].w_axis.truncate();
        let at = local_of(&panels[0], joint);
        let mut g = seated(GizmoUi::Translate);
        assert_eq!(
            g.interact(
                &mut doc,
                &facts(&panels, &[Some(pointer(at, true, false, Vec2::ZERO))]),
                false,
                100.0
            ),
            None
        );
    }

    /// The radios ARE the mode switch: each authored value parses to a mode and reaches the
    /// gadget, and a mode the step's gate forbids is refused rather than silently taken.
    #[test]
    fn the_mode_radios_switch_the_gadget() {
        let mut g = Gizmo::default();
        g.set_modes(rig_modes());
        for (i, value) in crate::ui::GIZMO_VALUES.iter().enumerate() {
            let ui = GizmoUi::parse(value).unwrap_or_else(|| panic!("{value} is an authored mode"));
            g.set_ui_mode(ui);
            assert_eq!(g.ui_mode(), ui, "the radio for {value} switched the gadget");
            assert_eq!(ui.value(), *value, "and maps back to slot {i}");
        }
        assert_eq!(
            GizmoUi::parse("orbit"),
            None,
            "an unauthored value is not a mode"
        );

        // A translate-only surface refuses the other three and stays where it is.
        let mut g = Gizmo::default();
        g.set_modes(modes_from_names(["translate"]));
        for ui in [GizmoUi::Rotate, GizmoUi::Scale, GizmoUi::Flip] {
            g.set_ui_mode(ui);
            assert_eq!(g.ui_mode(), GizmoUi::Translate, "{ui:?} is gated off");
        }
    }

    /// THE HANDLES COME FROM THE GADGET, for the PERSPECTIVE panel: the overlay is
    /// `Gadget::handle_lines` in the bench's theme colours. An ORTHOGRAPHIC panel draws NO handle
    /// at all — its every press is the free drag (Aaron 2026-09-07). A gated-off surface draws
    /// nothing either.
    #[test]
    fn the_handles_are_the_gadgets_and_an_ortho_panel_draws_none() {
        let (doc, target, radius) = rigged("gizmo_handles");
        let style = crate::compose::gadget_style(&crate::compose::theme());
        let mut g = seated(GizmoUi::Translate);
        g.gadget.set_frame(
            doc.parsed().unwrap().globals[target].w_axis.truncate(),
            Mat3::IDENTITY,
            radius,
        );
        let segments = |a: &Arrows| a.iter().map(|(_, v)| v.len()).sum::<usize>();
        let persp = segments(&g.handle_lines(Projection::Perspective, &style));
        let front = segments(&g.handle_lines(Projection::Front, &style));
        assert!(persp > 0, "the perspective panel draws every handle");
        assert_eq!(front, 0, "an orthographic panel draws no handle");

        // Every colour drawn is one the bench named — no handle escapes the palette.
        let named = [
            style.idle[0],
            style.idle[1],
            style.idle[2],
            style.aimed,
            style.locked,
            style.modifying,
            style.refused,
        ];
        for (c, _) in g.handle_lines(Projection::Perspective, &style) {
            assert!(
                named.contains(&c),
                "an unnamed handle colour {c:?} reached the overlay"
            );
        }

        // A step that gates the gadget off draws nothing (the Prep / Preview / Review panels).
        g.set_modes(modes_from_names::<&str>([]));
        assert_eq!(
            segments(&g.handle_lines(Projection::Perspective, &style)),
            0
        );
    }

    /// The JOINT pick — the one the gadget cannot make — still takes the nearest within tolerance
    /// and nothing beyond it.
    #[test]
    fn the_pick_takes_the_nearest_joint_within_tolerance() {
        let joints = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(5.0, 0.0, 10.0),
        ];
        let ray = (Vec3::new(0.2, -50.0, 10.0), Vec3::Y);
        assert_eq!(nearest_joint(ray, joints.iter().copied(), 1.0), Some(1));
        assert_eq!(nearest_joint(ray, joints.iter().copied(), 0.1), None);
    }

    /// DEVELOPMENT-TIER GATES (Aaron 2026-09-05, ruling 977B4D38): the hard-coded handoff
    /// conditions of a refactor — tests that read this crate's own source and assert a
    /// transition holds. `cargo test -- --skip gates::` is the production tier (every OS);
    /// `cargo test -- gates::` runs only these (one OS in CI). A gate names the transition
    /// it enforces and is deleted when that transition closes.
    mod gates {
        /// THE ABSORBED CODE IS GONE: the private press table, the pick tolerance, the arrow length
        /// and the handle composer moved into `flicker_rigview::Gadget` (7811D68B), and a copy left
        /// behind here would be a second source of truth for what a press means and how big a handle
        /// is. The needles are assembled rather than written, so the gate does not trip over itself.
        #[test]
        fn the_old_decide_table_and_handle_composer_are_deleted() {
            let needles = [
                ["GIZMO", "ARROW", "FRAC"].join("_"),
                ["PICK", "TOL", "FRAC"].join("_"),
                ["gizmo", "segments"].join("_"),
                ["fn ", "decide("].concat(),
            ];
            for (what, src) in [
                ("gizmo.rs", include_str!("gizmo.rs")),
                ("compose.rs", include_str!("compose.rs")),
                ("scene.rs", include_str!("scene.rs")),
            ] {
                for needle in &needles {
                    assert!(
                        !src.contains(needle),
                        "{what} still carries `{needle}` — the gadget absorbed it"
                    );
                }
            }
        }
    }
}
