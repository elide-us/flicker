//! **Gizmo interaction** — the pointer's picks and drags on the rig view's joints, driven
//! ONLY by the walker's `SurfacePointer` samples and the panels' rays (never a device
//! read, rule 985A1F73).
//!
//! Perspective panel: a press near a joint selects it and starts a DEFORM TEST drag — the
//! joint moves under the pointer so the skinning can be judged, and springs back to its
//! authored offset on release. Orthographic panels: a press near a joint selects it; a
//! press elsewhere with a selection starts a REPOSITION drag — the authored offset itself
//! moves, permanently. Both drag in the panel's view plane (the ray direction at the
//! press is the plane normal). The old Ctrl modifier is gone with the raw key read: an
//! ortho panel selects by proximity instead.

use flicker::ui::SurfacePointer;
use flicker_mechanics::{closest_point_ray_segment, drag_plane};
use flicker_rigview::{Projection, RigView};
use glam::Vec3;

use crate::compose::GIZMO_ARROW_FRAC;
use crate::services::{BoneOffset, Document};

/// A pick lands on a joint within this fraction of the gizmo's arrow length.
const PICK_TOL_FRAC: f32 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DragMode {
    /// The perspective test: moves the joint, restores `restore` on release.
    Deform { normal: Vec3, restore: BoneOffset },
    /// The orthographic edit: moves the authored offset for good.
    Reposition { normal: Vec3 },
}

/// What a press decides, given the panel's projection, the nearest joint (if any within
/// tolerance) and whether a joint is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    /// Select `joint` and start a deform test on it.
    Deform(usize),
    /// Select `joint` only.
    Select(usize),
    /// Start repositioning the selected joint.
    Reposition,
    /// Nothing: the press belongs to the camera.
    Camera,
}

pub(crate) fn decide(projection: Projection, hit: Option<usize>, has_selection: bool) -> Press {
    match (projection, hit, has_selection) {
        (Projection::Perspective, Some(i), _) => Press::Deform(i),
        (Projection::Perspective, None, _) => Press::Camera,
        (_, Some(i), _) => Press::Select(i),
        (_, None, true) => Press::Reposition,
        (_, None, false) => Press::Camera,
    }
}

/// The joint nearest the ray within `tol` — the pick.
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

/// The interaction state: at most one drag, owned by one panel.
#[derive(Default)]
pub(crate) struct Gizmo {
    drag: Option<(usize, DragMode, (Vec3, Vec3))>,
}

impl Gizmo {
    /// Run one frame against the panels' pointer samples. `active` is the Rig step with a
    /// rig loaded; `radius` the subject's framing radius (pick tolerance scales with it).
    /// Returns the panel whose pointer the gizmo consumed this frame — the scene withholds
    /// that pointer from the panel's camera.
    pub(crate) fn interact(
        &mut self,
        doc: &mut Document,
        panels: &[RigView],
        pointers: &[Option<SurfacePointer>],
        active: bool,
        radius: f32,
    ) -> Option<usize> {
        if !active {
            self.drag = None;
            return None;
        }
        let Some(globals) = doc.parsed().map(|p| p.globals.clone()) else {
            self.drag = None;
            return None;
        };
        let origin_of = |sel: usize| {
            globals
                .get(sel)
                .map(|g| g.w_axis.truncate())
                .unwrap_or(Vec3::ZERO)
        };

        // Continue (or release) an active drag.
        if let Some((panel, mode, ray_prev)) = self.drag {
            let ptr = pointers.get(panel).and_then(|p| p.as_ref());
            let held = ptr.is_some_and(|p| p.captured && p.left);
            let Some(sel) = doc.bone_sel() else {
                self.drag = None;
                return None;
            };
            if !held {
                if let DragMode::Deform { restore, .. } = mode {
                    doc.restore_offset(sel, restore);
                }
                self.drag = None;
                return None;
            }
            let ray = panels
                .get(panel)
                .and_then(|v| v.ray_at(ptr))
                .unwrap_or(ray_prev);
            let normal = match mode {
                DragMode::Deform { normal, .. } | DragMode::Reposition { normal } => normal,
            };
            let world_delta = drag_plane(normal, origin_of(sel), ray_prev, ray);
            self.drag = Some((panel, mode, ray));
            if world_delta != Vec3::ZERO {
                match mode {
                    DragMode::Deform { .. } => doc.apply_gizmo_delta(sel, &globals, world_delta),
                    DragMode::Reposition { .. } => doc.reposition_bone(sel, &globals, world_delta),
                }
            }
            return Some(panel);
        }

        // A fresh press on some panel.
        let tol = (radius * GIZMO_ARROW_FRAC).max(1.0) * PICK_TOL_FRAC;
        for (panel, (view, ptr)) in panels.iter().zip(pointers).enumerate() {
            let Some(p) = ptr.as_ref().filter(|p| p.pressed && p.left) else {
                continue;
            };
            let Some(ray) = view.ray_at(Some(p)) else {
                continue;
            };
            let hit = nearest_joint(ray, globals.iter().map(|g| g.w_axis.truncate()), tol);
            let normal = ray.1.normalize_or_zero();
            match decide(view.projection(), hit, doc.bone_sel().is_some()) {
                Press::Deform(i) => {
                    doc.select_bone(i);
                    let restore = doc.selected_offset().unwrap_or_default();
                    self.drag = Some((panel, DragMode::Deform { normal, restore }, ray));
                    return Some(panel);
                }
                Press::Select(i) => {
                    doc.select_bone(i);
                    return Some(panel);
                }
                Press::Reposition => {
                    self.drag = Some((panel, DragMode::Reposition { normal }, ray));
                    return Some(panel);
                }
                Press::Camera => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::render::{Rate, Rect, ViewportLayout};
    use flicker::ui::SurfaceSlot;
    use flicker_globe::Arrows;
    use glam::Vec2;

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

    /// Perspective: a press on a joint selects it and starts the deform test; the drag
    /// moves the joint; the release springs it back to its authored offset.
    #[test]
    fn a_perspective_drag_deforms_and_springs_back() {
        let mut doc = crate::tests::synthetic_rigged_doc("gizmo_persp");
        let panels = [panel(&doc, Projection::Perspective)];
        let radius = crate::compose::framing(&doc).radius;
        let target = doc.bone_count().unwrap() / 2;
        let joint = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let at = local_of(&panels[0], joint);
        let mut g = Gizmo::default();

        let owned = g.interact(&mut doc, &panels, &[Some(pointer(at, true, false, Vec2::ZERO))], true, radius);
        assert_eq!(owned, Some(0), "the press is the gizmo's");
        assert_eq!(doc.bone_sel(), Some(target), "the pressed joint is selected");
        let before = doc.selected_offset().unwrap();
        let rest = doc.parsed().unwrap().globals[target].w_axis.truncate();

        let dragged = at + Vec2::new(40.0, 0.0);
        let owned = g.interact(&mut doc, &panels, &[Some(pointer(dragged, false, true, Vec2::new(40.0, 0.0)))], true, radius);
        assert_eq!(owned, Some(0), "the drag holds the pointer");
        let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!((moved - rest).length() > 0.5, "the joint followed the pointer: {rest} → {moved}");

        let owned = g.interact(&mut doc, &panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))], true, radius);
        assert_eq!(owned, None, "released");
        assert_eq!(doc.selected_offset().unwrap(), before, "the deform test springs back");
        let back = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!((back - rest).length() < 1e-3, "the joint is home again: {back} vs {rest}");
    }

    /// Orthographic: a press away from any joint with a selection repositions it for good —
    /// the REST skeleton moves (the authored offset is untouched) and stays moved on release.
    #[test]
    fn an_orthographic_drag_repositions_the_selected_joint() {
        let mut doc = crate::tests::synthetic_rigged_doc("gizmo_ortho");
        let panels = [panel(&doc, Projection::Front)];
        let radius = crate::compose::framing(&doc).radius;
        let target = doc.bone_count().unwrap() / 2;
        assert!(doc.select_bone(target));
        let offset_before = doc.selected_offset().unwrap();
        let rest = doc.parsed().unwrap().globals[target].w_axis.truncate();
        let gen_before = doc.pose_gen;
        // A corner of the panel: far from every joint.
        let at = Vec2::new(4.0, 4.0);
        let mut g = Gizmo::default();
        assert_eq!(g.interact(&mut doc, &panels, &[Some(pointer(at, true, false, Vec2::ZERO))], true, radius), Some(0));
        let dragged = at + Vec2::new(0.0, 30.0);
        assert_eq!(g.interact(&mut doc, &panels, &[Some(pointer(dragged, false, true, Vec2::new(0.0, 30.0)))], true, radius), Some(0));
        assert_eq!(g.interact(&mut doc, &panels, &[Some(pointer(dragged, false, false, Vec2::ZERO))], true, radius), None);
        let moved = doc.parsed().unwrap().globals[target].w_axis.truncate();
        assert!((moved - rest).length() > 0.5, "the rest joint moved: {rest} → {moved}");
        assert_ne!(doc.pose_gen, gen_before, "a permanent conform edit bumps the pose");
        assert_eq!(doc.selected_offset().unwrap(), offset_before, "the authored offset is untouched");
        assert_eq!(doc.bone_sel(), Some(target));
    }

    /// Off the Rig step nothing is picked, and the pointer stays the camera's.
    #[test]
    fn an_inactive_gizmo_leaves_the_pointer_to_the_camera() {
        let mut doc = crate::tests::synthetic_rigged_doc("gizmo_off");
        let panels = [panel(&doc, Projection::Perspective)];
        let joint = doc.parsed().unwrap().globals[3].w_axis.truncate();
        let at = local_of(&panels[0], joint);
        let mut g = Gizmo::default();
        assert_eq!(g.interact(&mut doc, &panels, &[Some(pointer(at, true, false, Vec2::ZERO))], false, 100.0), None);
    }

    #[test]
    fn a_press_decides_by_projection_hit_and_selection() {
        assert_eq!(decide(Projection::Perspective, Some(3), false), Press::Deform(3));
        assert_eq!(decide(Projection::Perspective, None, true), Press::Camera);
        assert_eq!(decide(Projection::Top, Some(2), true), Press::Select(2));
        assert_eq!(decide(Projection::Front, None, true), Press::Reposition);
        assert_eq!(decide(Projection::Left, None, false), Press::Camera);
    }

    #[test]
    fn the_pick_takes_the_nearest_joint_within_tolerance() {
        let joints = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 10.0), Vec3::new(5.0, 0.0, 10.0)];
        let ray = (Vec3::new(0.2, -50.0, 10.0), Vec3::Y);
        assert_eq!(nearest_joint(ray, joints.iter().copied(), 1.0), Some(1));
        assert_eq!(nearest_joint(ray, joints.iter().copied(), 0.1), None);
    }
}
