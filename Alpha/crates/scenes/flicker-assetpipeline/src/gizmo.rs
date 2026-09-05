//! **The bench's half of the gadget** — which joint is selected, what a `GadgetDelta` means to
//! the [`Document`], and the perspective deform test's spring-back. Everything else — the handles,
//! the press table, the pick tolerance, the drag math, the snap accumulator and the mirror guard —
//! belongs to [`flicker_rigview::Gadget`] (contract 7811D68B), which this module drives from the
//! walker's `SurfacePointer` samples and the panels' rays (never a device read, rule 985A1F73).
//!
//! ## What a press means
//! The handles are the manipulator now (they used to be decorative — review 402A4B93 finding 5), so
//! ONE gadget shared by the four panels answers every press, in three tiers:
//! 1. **A handle**, out along its shaft — the gadget's own [`Press::Axis`]: begin its drag, or, in
//!    Flip mode, mirror about that axis and stay put. The one exclusion is the PIVOT's own ball,
//!    where all three shafts meet and no axis is meant: a press there falls through to the pick.
//! 2. **A joint** within tolerance — select it, in either projection. This is the one pick that
//!    stays the BENCH's; the gadget knows nothing about skeletons.
//! 3. **Empty space in an ORTHOGRAPHIC panel** — the gadget's [`Press::Free`], the view-plane drag
//!    the old `Reposition` row began. A miss in the perspective panel is the camera's.
//!
//! ## What a drag means
//! The panel decides, exactly as it always did (ruling 985A6850): the PERSPECTIVE panel runs a
//! DEFORM TEST — the joint moves so the skinning can be judged and springs back to its authored
//! offset on release ([`DragMode::Deform`]) — and an ORTHOGRAPHIC panel REPOSITIONS the rest
//! skeleton for good. `Gadget::cancel` deliberately un-applies nothing, because restoring a
//! document is document business; the restore value rides the drag here.

use flicker::ui::SurfacePointer;
use flicker_globe::Arrows;
use flicker_mechanics::{closest_point_ray_segment, Axis, GadgetModes, GizmoMode};
use flicker_rigview::{Gadget, GadgetDelta, GadgetStyle, Press, Projection, RigView};
use glam::{Mat3, Mat4, Vec3};

use crate::services::{BoneOffset, Document};

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
    /// The perspective TEST: the joint moves, and `restore` puts it back on release.
    Deform { restore: BoneOffset },
    /// The orthographic EDIT: the rest skeleton moves, permanently.
    Reposition,
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

/// The bench's gadget seat: ONE [`Gadget`] for the four panels, the radios' mode, the snap toggle,
/// and which panel owns the live drag.
#[derive(Default)]
pub(crate) struct Gizmo {
    gadget: Gadget,
    ui: GizmoUi,
    snap: bool,
    drag: Option<(usize, DragMode)>,
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

    /// This panel's handle overlay, ready for `RigView::set_overlay`.
    pub(crate) fn handle_lines(&self, projection: Projection, style: &GadgetStyle) -> Arrows {
        self.gadget.handle_lines(projection, style)
    }

    /// Run one frame against the panels' pointer samples. `active` is the Rig step with a rig
    /// loaded; `radius` the subject's framing radius (handle length and both tolerances follow).
    /// Returns the panel whose pointer the gadget consumed — the scene withholds that pointer
    /// from the panel's camera.
    pub(crate) fn interact(
        &mut self,
        doc: &mut Document,
        panels: &[RigView],
        pointers: &[Option<SurfacePointer>],
        active: bool,
        radius: f32,
    ) -> Option<usize> {
        let (sel, globals) = (doc.bone_sel(), doc.parsed().map(|p| p.globals.clone()));
        let (Some(sel), Some(globals), true) = (sel, globals, active) else {
            self.gadget.cancel();
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
            let ptr = pointers.get(panel).and_then(|p| p.as_ref());
            if !ptr.is_some_and(|p| p.captured && p.left) {
                if let DragMode::Deform { restore } = mode {
                    doc.restore_offset(sel, restore);
                }
                self.gadget.end();
                self.drag = None;
                return None;
            }
            if let Some(ray) = panels.get(panel).and_then(|v| v.ray_at(ptr)) {
                if let Some(delta) = self.gadget.update(ray) {
                    apply(doc, sel, &globals, mode, delta);
                }
            }
            return Some(panel);
        }

        // AIM: the panel the pointer is over pre-highlights the handle under it.
        match panels.iter().zip(pointers).find(|(_, p)| p.is_some()) {
            Some((v, p)) => self.gadget.pick(v.projection(), v.ray_at(p.as_ref())),
            None => self.gadget.pick(Projection::Perspective, None),
        };

        // A fresh press, decided by the one rule the module docs describe.
        let joint_tol = self.gadget.size() * JOINT_TOL_FRAC;
        let snap = self.snap.then(|| self.ui.snap());
        for (panel, (view, ptr)) in panels.iter().zip(pointers).enumerate() {
            let Some(p) = ptr.as_ref().filter(|p| p.pressed && p.left) else {
                continue;
            };
            let Some(ray) = view.ray_at(Some(p)) else {
                continue;
            };
            let projection = view.projection();
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
                    self.drag = Some((panel, drag_mode(doc, projection)));
                    return Some(panel);
                }
            }
            let joints = globals.iter().map(|g| g.w_axis.truncate());
            if let Some(i) = nearest_joint(ray, joints, joint_tol) {
                doc.select_bone(i);
                return Some(panel);
            }
            // Empty space in an ortho panel: the free view-plane reposition.
            if press == Press::Free
                && self.ui != GizmoUi::Flip
                && self.gadget.begin(projection, ray, snap)
            {
                self.drag = Some((panel, drag_mode(doc, projection)));
                return Some(panel);
            }
        }
        None
    }

    /// The discrete mirror about `axis`, through the gadget's guard. The validator is the bench's
    /// domain answer: a joint with no `_l`/`_r` twin has nowhere to reflect to, so the op is
    /// REFUSED — no partial write, and the handle draws dead (invariant C670523A's per-axis tier).
    fn mirror(&mut self, doc: &mut Document, sel: usize, globals: &[Mat4], axis: Axis) {
        let has_twin = doc.mirror_of(sel).is_some();
        if let Some(GadgetDelta::Flip(m)) = self.gadget.flip(axis, |_| has_twin) {
            doc.mirror_offset(sel, globals, m);
        }
    }
}

/// What a drag started in this panel means to the document.
fn drag_mode(doc: &Document, projection: Projection) -> DragMode {
    if projection == Projection::Perspective {
        DragMode::Deform {
            restore: doc.selected_offset().unwrap_or_default(),
        }
    } else {
        DragMode::Reposition
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
    use glam::{Quat, Vec2};

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
        let target = doc.bone_count().unwrap() / 2;
        assert!(doc.select_bone(target));
        let radius = crate::compose::framing(&doc).radius;
        (doc, target, radius)
    }

    /// PERSPECTIVE: a press on a joint selects it; a press on a HANDLE then deforms, and the
    /// release springs the authored offset back — the ephemeral deform test (ruling 985A6850).
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
            &panels,
            &[Some(pointer(at, true, false, Vec2::ZERO))],
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
        g.interact(&mut doc, &panels, &[None], true, radius);
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
            &panels,
            &[Some(pointer(grab, true, false, Vec2::ZERO))],
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
            &panels,
            &[Some(pointer(dragged, false, true, Vec2::new(40.0, 0.0)))],
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
            &panels,
            &[Some(pointer(dragged, false, false, Vec2::ZERO))],
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
                &panels,
                &[Some(pointer(at, true, false, Vec2::ZERO))],
                true,
                radius
            ),
            Some(0)
        );
        let dragged = at + Vec2::new(0.0, 30.0);
        assert_eq!(
            g.interact(
                &mut doc,
                &panels,
                &[Some(pointer(dragged, false, true, Vec2::new(0.0, 30.0)))],
                true,
                radius
            ),
            Some(0)
        );
        assert_eq!(
            g.interact(
                &mut doc,
                &panels,
                &[Some(pointer(dragged, false, false, Vec2::ZERO))],
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
        assert!(doc.select_bone(twin));
        assert_eq!(
            doc.selected_offset().unwrap().roll,
            -12.0,
            "the reflection reverses the roll"
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
                &panels,
                &[Some(pointer(grab, true, false, Vec2::ZERO))],
                true,
                radius
            ),
            Some(0)
        );
        // A hair of travel is inside one snap step, so nothing moves at all.
        let nudge = grab + Vec2::new(0.0, 0.4);
        g.interact(
            &mut doc,
            &panels,
            &[Some(pointer(nudge, false, true, Vec2::new(0.0, 0.4)))],
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
            &panels,
            &[Some(pointer(far, false, true, Vec2::new(0.0, 60.0)))],
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
                &panels,
                &[Some(pointer(at, true, false, Vec2::ZERO))],
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

    /// THE HANDLES COME FROM THE GADGET, per panel: the overlay is `Gadget::handle_lines` in the
    /// bench's theme colours, and an ORTHOGRAPHIC panel drops the axis it looks along (which would
    /// project to a point). A gated-off surface draws nothing at all.
    #[test]
    fn the_handles_are_the_gadgets_and_an_ortho_panel_hides_its_depth_axis() {
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
        assert!(
            front > 0 && front < persp,
            "Front drops the axis it looks along: {front}/{persp}"
        );

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
