//! **The gadget** — the ONE reusable 3D-manipulation overlay (gadget direction F28531B5).
//!
//! The [`Gadget`] is the FILLER over the gizmo MATH in [`flicker_mechanics::gizmo`]: it holds the
//! manipulated feature's frame, the mode a surface is in and the modes its scene ALLOWS, turns a
//! pointer ray into a picked handle and a drag, and hands back one [`GadgetDelta`] per frame for
//! the consumer to apply to its own document. It owns no document, no selection, no pointer device
//! and no colour: rays come from [`RigView::ray_at`](crate::RigView::ray_at) fed by the walker's
//! `SurfacePointer` (contract 985A1F73 — a filler takes the sample, never the device), and colours
//! arrive as resolved tokens in a [`GadgetStyle`] the CONSUMER fills (rule 790872EE: no rgba
//! literals in engine code).
//!
//! ONE gadget serves a scene's whole panel set — Clayworks' quad view is four `surface` panels and
//! at most one of them may be dragging. The panel's [`Projection`] is therefore a per-CALL argument,
//! not construction state: the same gadget draws different handle sets into different panels.
//!
//! ## The four modes, and where the fourth one lives
//! Translate / Rotate / Scale are the three continuous drags of [`GizmoMode`]; FLIP is discrete and
//! is NOT a drag mode — it is gated by [`GadgetModes::FLIP`] and applied by [`Gadget::flip`], which
//! goes through the guarded [`flicker_mechanics::flip`] with the consumer's own validator. (A
//! `GizmoMode::Flip` variant cannot land until Clayworks' exhaustive match moves with it — deadend
//! 7F44380D; do not re-attempt it from here.)
//!
//! ## Mode availability is AUTHORED, and the filler never reads the Model
//! Per the direction, mode availability is a declarative Component prop: a scene publishes a Model
//! value — `gadget_modes`, a list of mode names (`"translate"`, `"rotate"`, `"scale"`, `"flip"`) —
//! and the SCENE'S RUST reads that value each frame, maps it with [`modes_from_names`] and calls
//! [`Gadget::set_modes`]. The gadget never touches the Model, the scene tree or Lua: that is the
//! five-line split (491BD9BB), and it is why a surface that only places things can ship
//! `["translate"]` from its scene file with no Rust change anywhere.
//!
//! ## State colours are a RENDER CONTRACT
//! FB4283D1 makes "ghost preview + colour-coded state" a contract every manipulable surface must
//! meet, not polish. [`handle_lines`](Gadget::handle_lines) colours each handle by its
//! [`HandleState`] — Idle, Aimed (the pointer's pre-highlight), Locked (picked, nothing moved yet),
//! Modifying (the drag is producing deltas), Refused (the domain said no) — the Aim → Locked →
//! Modify machine, drawn.
//!
//! ## The consumer sweep this absorbs
//! Clayworks' `Alpha/crates/scenes/flicker-assetpipeline/src/gizmo.rs` (its `decide` / `nearest_joint`
//! / `Gizmo::interact` deform+reposition loop) becomes a `Gadget` plus a few lines applying each
//! [`GadgetDelta`] to its `Document`, and `compose.rs`'s private `handles()` (compose.rs:186-195)
//! plus its `GIZMO_ARROW_FRAC` (compose.rs:42) and the gizmo module's `PICK_TOL_FRAC` (gizmo.rs:22)
//! are absorbed here — that whole module is then deletable. csgtest is the second consumer.

use flicker_globe::Arrows;
use flicker_mechanics::{
    flip as flip_guarded, gizmo_segments, pick_handle_among, world_axis, Axis, DragDelta,
    DragState, GadgetModes, GizmoMode,
};
use glam::{Mat3, Mat4, Quat, Vec3};

use crate::Projection;

/// Handle length as a fraction of the framed subject's radius — Clayworks' `GIZMO_ARROW_FRAC`
/// (`flicker-assetpipeline/src/compose.rs:42`), absorbed so the bench keeps no copy.
const HANDLE_FRAC: f32 = 0.18;
/// Pick tolerance as a fraction of the handle length — Clayworks' `PICK_TOL_FRAC`
/// (`flicker-assetpipeline/src/gizmo.rs:22`), absorbed with it.
const PICK_TOL_FRAC: f32 = 0.35;
/// A handle within this cosine of the panel's depth axis is edge-on: it projects to a point, so it
/// is neither drawn nor pickable there (~8°, which tolerates a rotated feature basis).
const DEPTH_COS: f32 = 0.99;

/// One frame of manipulation for the consumer to apply to its own document.
///
/// The three drag currencies COMPOSE (translations add, quaternions multiply, factors multiply), so
/// a consumer applies each frame's delta as it lands. `Flip` is the one-shot mirror.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GadgetDelta {
    /// WORLD translation to add (the consumer converts world → its own parent space).
    Translate(Vec3),
    /// Rotation to compose about the gadget's pivot.
    Rotate(Quat),
    /// Per-axis multiplicative factors in the feature's LOCAL frame.
    Scale(Vec3),
    /// The mirror transform, already carried to the pivot — apply it to the geometry.
    Flip(Mat4),
}

impl From<DragDelta> for GadgetDelta {
    fn from(d: DragDelta) -> Self {
        match d {
            DragDelta::Translate(v) => Self::Translate(v),
            DragDelta::Rotate(q) => Self::Rotate(q),
            DragDelta::Scale(s) => Self::Scale(s),
        }
    }
}

/// What a press decides. Absorbs the shape of Clayworks' `decide` table
/// (`flicker-assetpipeline/src/gizmo.rs:46-54`) with the gadget's HANDLES where that table had the
/// bench's joints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Press {
    /// The press landed on a handle: drag that axis (that table's rows 1 and 3).
    Axis(Axis),
    /// The press landed on nothing in an ORTHOGRAPHIC panel: drag the mode's FREE, view-relative
    /// form — the plane drag its `Reposition` row began, and the reason an ortho panel's depth axis
    /// is not a handle at all (row 4).
    Free,
    /// Nothing here is the gadget's: the pointer belongs to the panel's camera. A miss in the
    /// PERSPECTIVE panel (row 2, no unambiguous drag plane), or a gadget with no allowed mode
    /// (row 5's "no selection").
    Camera,
}

/// A handle's drawn state — the Aim → Locked → Modify machine FB4283D1 makes a render contract.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HandleState {
    /// At rest.
    Idle,
    /// Under the pointer: the candidate pre-highlighted before commit (AIM).
    Aimed,
    /// Picked and held, nothing moved yet (LOCKED).
    Locked,
    /// Producing deltas (MODIFY).
    Modifying,
    /// The domain refused this axis' op — drawn dead, the invariant's per-axis "disable the invalid
    /// action" tier (C670523A).
    Refused,
}

/// The gadget's colours, all of them the CONSUMER's: resolved theme tokens, never literals in this
/// crate (rule 790872EE). There is deliberately no `Default` — a surface names every colour it
/// draws, so a missing token fails at the call site instead of painting the gadget invisible.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GadgetStyle {
    /// The three handles at rest, in X, Y, Z order (the R/G/B axis tokens).
    pub idle: [[f32; 4]; 3],
    /// The handle under the pointer.
    pub aimed: [f32; 4],
    /// The picked handle, held but not yet moving.
    pub locked: [f32; 4],
    /// The handle being dragged.
    pub modifying: [f32; 4],
    /// A handle whose op the domain refused.
    pub refused: [f32; 4],
}

impl GadgetStyle {
    /// The colour a handle draws in, given its axis and state.
    pub fn color(&self, axis: Axis, state: HandleState) -> [f32; 4] {
        match state {
            HandleState::Idle => self.idle[axis as usize],
            HandleState::Aimed => self.aimed,
            HandleState::Locked => self.locked,
            HandleState::Modifying => self.modifying,
            HandleState::Refused => self.refused,
        }
    }
}

/// The authored mode names → the gate. The ONE place the `gadget_modes` spelling lives, so two
/// consumers cannot drift into two vocabularies; unknown names are ignored (an empty or all-unknown
/// list yields [`GadgetModes::NONE`], an inert gadget, which is visible immediately).
pub fn modes_from_names<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> GadgetModes {
    names
        .into_iter()
        .fold(GadgetModes::NONE, |acc, n| match n.as_ref() {
            "translate" => acc.with(GadgetModes::TRANSLATE),
            "rotate" => acc.with(GadgetModes::ROTATE),
            "scale" => acc.with(GadgetModes::SCALE),
            "flip" => acc.with(GadgetModes::FLIP),
            other => {
                tracing::warn!("gadget_modes: unknown mode {other:?} — ignored");
                acc
            }
        })
}

/// The reusable 3D-manipulation gadget: the feature's frame, the mode, the gate, and at most one
/// live drag. See the module docs for the whole contract.
#[derive(Clone, Debug)]
pub struct Gadget {
    modes: GadgetModes,
    mode: GizmoMode,
    pivot: Vec3,
    basis: Mat3,
    /// Handle length in world units, derived from the framed subject's radius.
    size: f32,
    hover: Option<Axis>,
    drag: Option<DragState>,
    /// Has the live drag actually produced a delta yet? Locked → Modifying.
    moved: bool,
    refused: Option<Axis>,
}

impl Default for Gadget {
    fn default() -> Self {
        Self {
            modes: GadgetModes::default(),
            mode: GizmoMode::default(),
            pivot: Vec3::ZERO,
            basis: Mat3::IDENTITY,
            size: 1.0,
            hover: None,
            drag: None,
            moved: false,
            refused: None,
        }
    }
}

impl Gadget {
    /// Frame the gadget on the manipulated feature: its `pivot`, its `basis` (columns are the
    /// feature's world axes; `Mat3::IDENTITY` for a world-aligned gadget) and the framed subject's
    /// bounding `radius`, from which the handle length and the pick tolerance follow. Called every
    /// frame the selection or the framing can move; it never disturbs a live drag's own frame,
    /// which [`DragState`] captured at the press.
    pub fn set_frame(&mut self, pivot: Vec3, basis: Mat3, radius: f32) {
        self.pivot = pivot;
        self.basis = basis;
        self.size = (radius * HANDLE_FRAC).max(1.0);
    }

    /// The modes this surface allows — the scene's authored gate (see the module docs). An active
    /// mode the new gate forbids falls back to the first allowed one, so a gadget is never sitting
    /// in a mode its surface disowns.
    pub fn set_modes(&mut self, modes: GadgetModes) {
        self.modes = modes;
        if !modes.allows(self.mode) {
            if let Some(m) = modes.modes().next() {
                self.mode = m;
            }
        }
    }

    pub fn modes(&self) -> GadgetModes {
        self.modes
    }

    /// Switch the drag mode. Refused (`false`, mode unchanged) when the surface's gate forbids it —
    /// the gate is authoritative, not advisory.
    pub fn set_mode(&mut self, mode: GizmoMode) -> bool {
        if !self.modes.allows(mode) {
            return false;
        }
        self.mode = mode;
        true
    }

    pub fn mode(&self) -> GizmoMode {
        self.mode
    }

    pub fn pivot(&self) -> Vec3 {
        self.pivot
    }

    /// The handle length in world units — the scale a consumer's own hit tests should match.
    pub fn size(&self) -> f32 {
        self.size
    }

    /// The handle the pointer is on, if any.
    pub fn hover(&self) -> Option<Axis> {
        self.hover
    }

    /// Is a drag live?
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The drag's snapped total in its mode's display currency (distance / degrees / ratio) — the
    /// number a readout or a ghost preview shows.
    pub fn total(&self) -> Option<f32> {
        self.drag.as_ref().map(DragState::total)
    }

    /// The axis whose op the domain last refused, for the surface to show. Cleared by the next
    /// accepted [`flip`](Self::flip) or by [`cancel`](Self::cancel).
    pub fn refused(&self) -> Option<Axis> {
        self.refused
    }

    /// The handles this panel may draw and pick: the mode's axes from
    /// [`GadgetModes::handles_for`] (EMPTY for a gated-off mode — a disabled mode has nothing to
    /// grab) less an orthographic panel's depth axis, which projects to a point.
    pub fn handles(&self, projection: Projection) -> impl Iterator<Item = Axis> + '_ {
        let depth = projection.depth_axis();
        self.modes
            .handles_for(self.mode)
            .iter()
            .copied()
            .filter(move |&a| !hidden_by(depth, self.basis, a))
    }

    /// AIM. The handle nearest the pointer's ray within tolerance, recorded as the hover so
    /// [`handle_lines`](Self::handle_lines) pre-highlights it before any commit. `None` for the ray
    /// (the pointer is not over this panel) clears the hover. A live drag keeps its held handle as
    /// the aim and this is inert.
    pub fn pick(&mut self, projection: Projection, ray: Option<(Vec3, Vec3)>) -> Option<Axis> {
        if self.drag.is_some() {
            return self.hover;
        }
        self.hover = ray.and_then(|r| self.pick_at(projection, r));
        self.hover
    }

    /// What a press with this ray would decide, without acting — the table this gadget absorbed
    /// (see [`Press`]).
    pub fn decide(&self, projection: Projection, ray: (Vec3, Vec3)) -> Press {
        if !self.modes.allows(self.mode) {
            return Press::Camera;
        }
        match self.pick_at(projection, ray) {
            Some(a) => Press::Axis(a),
            None if projection.is_ortho() => Press::Free,
            None => Press::Camera,
        }
    }

    /// LOCK. Begin a drag from the press ray, snapping in the mode's own currency (distance for
    /// Translate, DEGREES for Rotate, ratio for Scale; `None` is continuous). Returns whether the
    /// press was the gadget's — a consumer withholds that frame's pointer from the panel's camera
    /// when it was, exactly as Clayworks' `interact` returns the panel it consumed. A press while a
    /// drag is already live is refused: one gadget, one drag.
    pub fn begin(&mut self, projection: Projection, ray: (Vec3, Vec3), snap: Option<f32>) -> bool {
        if self.drag.is_some() {
            return false;
        }
        let axis = match self.decide(projection, ray) {
            Press::Axis(a) => Some(a),
            Press::Free => None,
            Press::Camera => return false,
        };
        self.hover = axis;
        self.moved = false;
        self.drag = Some(DragState::begin(
            self.mode, axis, self.basis, self.pivot, ray, snap,
        ));
        true
    }

    /// MODIFY. Advance the live drag with this frame's ray. `None` when no drag is live or the
    /// frame moved nothing — a snapped drag emits on step boundaries only, and a ray the mode
    /// cannot resolve holds, both by design.
    pub fn update(&mut self, ray: (Vec3, Vec3)) -> Option<GadgetDelta> {
        let delta = self.drag.as_mut()?.update(ray);
        if delta.is_identity() {
            return None;
        }
        self.moved = true;
        Some(delta.into())
    }

    /// Release the drag, keeping everything it applied (the consumer has been applying each frame's
    /// delta as it landed). The hover stays: the pointer is still on the handle.
    pub fn end(&mut self) {
        self.drag = None;
        self.moved = false;
    }

    /// Abandon the drag and fall back to a clean AIM state (FB4283D1's "B returns to Aim"): the
    /// hover and any refusal clear too.
    ///
    /// Nothing is un-applied here. A surface that must revert holds its own restore value across
    /// the drag and writes it back — Clayworks' deform test does exactly that (`DragMode::Deform {
    /// restore }`, `flicker-assetpipeline/src/gizmo.rs:117-119`), and that is document business,
    /// which an engine crate has no business knowing.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.moved = false;
        self.hover = None;
        self.refused = None;
    }

    /// The discrete MIRROR about `axis`, guarded by the domain's own validator.
    ///
    /// `valid` is handed the candidate transform and answers whether the result is representable —
    /// a voxel consumer checks each `m.transform_point3(corner)` against the CornerVector range.
    /// A refusal produces NO delta and raises [`refused`](Self::refused) for the surface to show;
    /// no clamp, no partial application (invariant C670523A). `None` also when the surface's gate
    /// omits `FLIP` — at surface scope the op does not exist, so there is nothing to gray out and
    /// no refusal is raised.
    pub fn flip(&mut self, axis: Axis, valid: impl FnOnce(&Mat4) -> bool) -> Option<GadgetDelta> {
        if !self.modes.flip_handles().contains(&axis) {
            return None;
        }
        match flip_guarded(axis, self.basis, self.pivot, valid) {
            Ok(m) => {
                self.refused = None;
                Some(GadgetDelta::Flip(m))
            }
            Err(r) => {
                self.refused = Some(r.axis);
                None
            }
        }
    }

    /// This frame's OVERLAY line batches for one panel, ready for
    /// [`RigView::set_overlay`](crate::RigView::set_overlay): the active mode's handles (translate
    /// arrows, rotate rings, scale boxes) from the mechanics geometry, coloured by each handle's
    /// [`HandleState`] out of `style`, one batch per resolved colour.
    ///
    /// When the surface allows FLIP, every mirrorable axis also gets its opposite-pointing arrow —
    /// the double-ended arrow that reads as "and back the other way". It is the same
    /// [`gizmo_segments`] geometry taken through a negated basis, not a second geometry path (the
    /// mirror deliberately has no handles of its own — deadend 7F44380D).
    ///
    /// A gated-off mode draws NOTHING, and neither does an orthographic panel's depth axis.
    pub fn handle_lines(&self, projection: Projection, style: &GadgetStyle) -> Arrows {
        let mut out = Arrows::new();
        let depth = projection.depth_axis();
        let mut emit = |segs: Vec<(Vec3, Vec3, [f32; 4])>| {
            for (a, b, tag) in segs {
                let Some(axis) = axis_of(tag) else { continue };
                if hidden_by(depth, self.basis, axis) {
                    continue;
                }
                push(&mut out, style.color(axis, self.state(axis)), (a, b));
            }
        };
        if self.modes.allows(self.mode) {
            emit(gizmo_segments(
                self.pivot, self.basis, self.mode, self.size, None,
            ));
        }
        if self.modes.allows_flip() {
            emit(gizmo_segments(
                self.pivot,
                self.basis * -1.0,
                GizmoMode::Translate,
                self.size,
                None,
            ));
        }
        out
    }

    /// A handle's drawn state, by the Aim → Locked → Modify machine. A FREE drag has no handle, so
    /// it highlights none: the whole panel is the manipulator in that form.
    fn state(&self, axis: Axis) -> HandleState {
        if self.refused == Some(axis) {
            return HandleState::Refused;
        }
        if self.drag.and_then(|d| d.axis()) == Some(axis) {
            return if self.moved {
                HandleState::Modifying
            } else {
                HandleState::Locked
            };
        }
        if self.hover == Some(axis) {
            HandleState::Aimed
        } else {
            HandleState::Idle
        }
    }

    /// The handle this ray lands on, over the handles the panel is actually showing — the same
    /// [`handles`](Self::handles) set that is drawn, on the stack (a pick runs per panel per frame).
    fn pick_at(&self, projection: Projection, ray: (Vec3, Vec3)) -> Option<Axis> {
        let mut shown = [Axis::X; 3];
        let mut n = 0;
        for a in self.handles(projection) {
            shown[n] = a;
            n += 1;
        }
        pick_handle_among(
            ray,
            self.pivot,
            self.basis,
            self.mode,
            self.size,
            self.size * PICK_TOL_FRAC,
            &shown[..n],
        )
    }
}

/// Does this panel's depth axis swallow the handle? A handle within [`DEPTH_COS`] of the direction
/// the panel looks along projects to a point: it cannot be aimed at and cannot be dragged.
fn hidden_by(depth: Option<Vec3>, basis: Mat3, axis: Axis) -> bool {
    depth.is_some_and(|d| world_axis(basis, axis).dot(d).abs() > DEPTH_COS)
}

/// The axis a mechanics segment belongs to. [`gizmo_segments`] tags every segment with its own
/// axis' colour, which is how the gadget (and Clayworks' `compose::handles`, compose.rs:189-193)
/// groups them; asking with `hover: None` means every tag is a base colour, so the match is total.
/// The test `every_segment_of_every_mode_carries_its_axis` is the gate over that channel.
fn axis_of(tag: [f32; 4]) -> Option<Axis> {
    Axis::ALL.into_iter().find(|a| a.color() == tag)
}

/// Append a segment to its colour's batch, making the batch if this is its first.
fn push(out: &mut Arrows, color: [f32; 4], seg: (Vec3, Vec3)) {
    match out.iter_mut().find(|(k, _)| *k == color) {
        Some((_, v)) => v.push(seg),
        None => out.push((color, vec![seg])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test tokens: a consumer's job in the app, literals here so the assertions can name them.
    const STYLE: GadgetStyle = GadgetStyle {
        idle: [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        ],
        aimed: [1.0, 1.0, 0.0, 1.0],
        locked: [1.0, 0.5, 0.0, 1.0],
        modifying: [1.0, 0.0, 1.0, 1.0],
        refused: [0.2, 0.2, 0.2, 1.0],
    };

    /// A gadget framed at the origin with a handle length of 18 (radius 100 × 0.18) and a pick
    /// tolerance of 6.3. The three DRAG modes only: the tests that want the mirror's extra arrows
    /// opt FLIP in themselves, so a handle count here is the mode's own geometry.
    fn gadget(mode: GizmoMode) -> Gadget {
        let mut g = Gadget::default();
        g.set_modes(GadgetModes::ALL.without(GadgetModes::FLIP));
        g.set_frame(Vec3::ZERO, Mat3::IDENTITY, 100.0);
        assert!(g.set_mode(mode));
        g
    }

    /// Segments of one colour in the batches.
    fn count(lines: &Arrows, color: [f32; 4]) -> usize {
        lines
            .iter()
            .filter(|(c, _)| *c == color)
            .map(|(_, v)| v.len())
            .sum()
    }

    fn total(lines: &Arrows) -> usize {
        lines.iter().map(|(_, v)| v.len()).sum()
    }

    /// A ray parallel to `dir` that passes through `through`.
    fn ray(through: Vec3, dir: Vec3) -> (Vec3, Vec3) {
        (through - dir * 50.0, dir)
    }

    #[test]
    fn every_segment_of_every_mode_carries_its_axis() {
        for mode in GizmoMode::ALL {
            for (_, _, tag) in gizmo_segments(Vec3::ZERO, Mat3::IDENTITY, mode, 10.0, None) {
                assert!(axis_of(tag).is_some(), "{mode:?}: untagged segment {tag:?}");
            }
        }
    }

    #[test]
    fn each_mode_draws_its_own_geometry_and_a_gated_mode_draws_nothing() {
        // Translate: 3 axes × (shaft + 2 head lines). Rotate: 3 rings of 24. Scale: shaft + 12 box
        // edges per axis.
        for (mode, segs) in [
            (GizmoMode::Translate, 9),
            (GizmoMode::Rotate, 72),
            (GizmoMode::Scale, 39),
        ] {
            let lines = gadget(mode).handle_lines(Projection::Perspective, &STYLE);
            assert_eq!(total(&lines), segs, "{mode:?}");
        }

        // A surface that only translates cannot be put into Rotate, and draws no rings.
        let mut g = gadget(GizmoMode::Translate);
        g.set_modes(GadgetModes::TRANSLATE);
        assert!(!g.set_mode(GizmoMode::Rotate), "the gate refuses the mode");
        assert_eq!(g.mode(), GizmoMode::Translate);
        assert_eq!(
            total(&g.handle_lines(Projection::Perspective, &STYLE)),
            9,
            "arrows only — no rings"
        );

        // The gate is retroactive: dropping Rotate off a gadget sitting in it moves it out.
        let mut g = gadget(GizmoMode::Rotate);
        g.set_modes(GadgetModes::ALL.without(GadgetModes::ROTATE));
        assert_ne!(g.mode(), GizmoMode::Rotate);
        assert!(g.handles(Projection::Perspective).count() > 0);

        // And an inert gadget draws nothing at all.
        let mut g = gadget(GizmoMode::Translate);
        g.set_modes(GadgetModes::NONE);
        assert_eq!(total(&g.handle_lines(Projection::Perspective, &STYLE)), 0);
        assert_eq!(g.handles(Projection::Perspective).count(), 0);
    }

    #[test]
    fn a_pick_takes_the_nearest_handle_and_nothing_beyond_the_tolerance() {
        let mut g = gadget(GizmoMode::Translate);
        // Down the +X shaft (0..18), 9 out: X misses by 0, Y and Z by 9 > 6.3.
        assert_eq!(
            g.pick(
                Projection::Perspective,
                Some(ray(Vec3::new(9.0, 0.0, 0.0), Vec3::Y))
            ),
            Some(Axis::X)
        );
        assert_eq!(g.hover(), Some(Axis::X));
        // Far off every handle.
        assert_eq!(
            g.pick(
                Projection::Perspective,
                Some(ray(Vec3::new(40.0, 0.0, 40.0), Vec3::Y))
            ),
            None
        );
        // No pointer over the panel clears the aim.
        g.pick(
            Projection::Perspective,
            Some(ray(Vec3::new(0.0, 9.0, 0.0), Vec3::Z)),
        );
        assert_eq!(g.hover(), Some(Axis::Y));
        assert_eq!(g.pick(Projection::Perspective, None), None);
        assert_eq!(g.hover(), None);
    }

    #[test]
    fn a_perspective_drag_on_the_x_arrow_translates_along_x_only() {
        let mut g = gadget(GizmoMode::Translate);
        let press = ray(Vec3::new(9.0, 0.0, 0.0), Vec3::Y);
        assert_eq!(
            g.decide(Projection::Perspective, press),
            Press::Axis(Axis::X)
        );
        assert!(g.begin(Projection::Perspective, press, None));
        let moved = g.update(ray(Vec3::new(14.0, 0.0, 0.0), Vec3::Y));
        let Some(GadgetDelta::Translate(v)) = moved else {
            panic!("expected a translation, got {moved:?}");
        };
        assert!(
            (v - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-3,
            "along X only: {v}"
        );
        g.end();
        assert!(!g.dragging());
    }

    #[test]
    fn a_quarter_sweep_on_the_y_ring_rotates_ninety_degrees_about_y() {
        let mut g = gadget(GizmoMode::Rotate);
        // The rings sit at 0.85 × the handle length.
        let r = g.size() * 0.85;
        let press = ray(Vec3::new(r, 0.0, 0.0), Vec3::NEG_Y);
        assert_eq!(
            g.decide(Projection::Perspective, press),
            Press::Axis(Axis::Y)
        );
        assert!(g.begin(Projection::Perspective, press, None));
        // A right-handed quarter turn about +Y carries +X to −Z.
        let swept = g.update(ray(Vec3::new(0.0, 0.0, -r), Vec3::NEG_Y));
        let Some(GadgetDelta::Rotate(q)) = swept else {
            panic!("expected a rotation, got {swept:?}");
        };
        let (axis, angle) = q.to_axis_angle();
        assert!((axis.dot(Vec3::Y) - 1.0).abs() < 1e-3, "about +Y: {axis}");
        assert!(
            (angle.to_degrees() - 90.0).abs() < 1e-2,
            "90°: {}",
            angle.to_degrees()
        );
        assert!((q * Vec3::Z - Vec3::X).length() < 1e-3, "Z turns to X");
        assert!(
            (g.total().unwrap() - 90.0).abs() < 1e-2,
            "the readout agrees"
        );
    }

    #[test]
    fn a_scale_drag_on_the_z_handle_scales_z_alone() {
        let mut g = gadget(GizmoMode::Scale);
        assert!(g.begin(
            Projection::Perspective,
            ray(Vec3::new(0.0, 0.0, 9.0), Vec3::Y),
            None
        ));
        let scaled = g.update(ray(Vec3::new(0.0, 0.0, 18.0), Vec3::Y));
        let Some(GadgetDelta::Scale(s)) = scaled else {
            panic!("expected a scale, got {scaled:?}");
        };
        assert!(
            (s - Vec3::new(1.0, 1.0, 2.0)).length() < 1e-3,
            "(1,1,k): {s}"
        );
    }

    #[test]
    fn a_refused_flip_yields_no_delta_and_raises_the_flag() {
        let mut g = gadget(GizmoMode::Translate);
        g.set_modes(GadgetModes::ALL);
        assert_eq!(g.flip(Axis::X, |_| false), None, "the validator refused");
        assert_eq!(g.refused(), Some(Axis::X));
        assert_eq!(
            count(
                &g.handle_lines(Projection::Perspective, &STYLE),
                STYLE.refused
            ),
            6,
            "the refused axis draws dead, both its arrow and its mirror"
        );
        // The same axis on a permissive domain mirrors, and clears the flag.
        let Some(GadgetDelta::Flip(m)) = g.flip(Axis::X, |_| true) else {
            panic!("expected a mirror");
        };
        assert!(
            (m.transform_point3(Vec3::new(2.0, 3.0, 4.0)) - Vec3::new(-2.0, 3.0, 4.0)).length()
                < 1e-4
        );
        assert_eq!(g.refused(), None);
        // A surface whose gate omits FLIP has no mirror at all — and no refusal to show.
        g.set_modes(GadgetModes::TRANSLATE);
        assert_eq!(g.flip(Axis::X, |_| true), None);
        assert_eq!(g.refused(), None);
    }

    #[test]
    fn an_orthographic_panel_hides_and_refuses_its_depth_axis() {
        let mut g = gadget(GizmoMode::Translate);
        // FRONT looks along −Y, so with a world-aligned basis the Y handle is edge-on.
        assert_eq!(Projection::Front.depth_axis(), Some(Vec3::NEG_Y));
        let shown: Vec<Axis> = g.handles(Projection::Front).collect();
        assert_eq!(shown, vec![Axis::X, Axis::Z], "no Y handle in FRONT");
        let lines = g.handle_lines(Projection::Front, &STYLE);
        assert_eq!(count(&lines, STYLE.idle[1]), 0, "no Y segments drawn");
        assert_eq!(total(&lines), 6, "two axes' arrows");
        // A ray straight down the Y shaft picks it in PERSPECTIVE and nothing in FRONT.
        let down_y = ray(Vec3::new(0.0, 9.0, 0.0), Vec3::Z);
        assert_eq!(g.pick(Projection::Perspective, Some(down_y)), Some(Axis::Y));
        assert_eq!(g.pick(Projection::Front, Some(down_y)), None);
        // And that miss is the ortho panel's FREE plane drag, where perspective yields the camera.
        assert_eq!(g.decide(Projection::Front, down_y), Press::Free);
        assert_eq!(
            g.decide(
                Projection::Perspective,
                ray(Vec3::new(40.0, 0.0, 40.0), Vec3::Y)
            ),
            Press::Camera
        );
        assert!(g.begin(Projection::Front, down_y, None));
        assert!(g.dragging(), "the free form began");
    }

    #[test]
    fn a_handle_colours_by_state_from_idle_through_aim_and_lock_to_modify() {
        let mut g = gadget(GizmoMode::Translate);
        let x_ray = ray(Vec3::new(9.0, 0.0, 0.0), Vec3::Y);
        let lines = g.handle_lines(Projection::Perspective, &STYLE);
        assert_eq!(count(&lines, STYLE.idle[0]), 3, "idle");

        g.pick(Projection::Perspective, Some(x_ray));
        let lines = g.handle_lines(Projection::Perspective, &STYLE);
        assert_eq!(count(&lines, STYLE.aimed), 3, "aimed");
        assert_eq!(count(&lines, STYLE.idle[0]), 0);

        assert!(g.begin(Projection::Perspective, x_ray, None));
        let lines = g.handle_lines(Projection::Perspective, &STYLE);
        assert_eq!(count(&lines, STYLE.locked), 3, "locked: held, not moved");

        assert!(g.update(ray(Vec3::new(14.0, 0.0, 0.0), Vec3::Y)).is_some());
        let lines = g.handle_lines(Projection::Perspective, &STYLE);
        assert_eq!(count(&lines, STYLE.modifying), 3, "modifying");
        assert_eq!(count(&lines, STYLE.idle[1]), 3, "the other axes stay idle");

        g.cancel();
        let lines = g.handle_lines(Projection::Perspective, &STYLE);
        assert_eq!(count(&lines, STYLE.idle[0]), 3, "cancel returns to aim");
    }

    #[test]
    fn the_flip_gate_adds_the_mirrored_arrow_and_the_names_map_once() {
        let mut g = gadget(GizmoMode::Translate);
        g.set_modes(GadgetModes::TRANSLATE);
        assert_eq!(total(&g.handle_lines(Projection::Perspective, &STYLE)), 9);
        g.set_modes(GadgetModes::TRANSLATE.with(GadgetModes::FLIP));
        assert_eq!(
            total(&g.handle_lines(Projection::Perspective, &STYLE)),
            18,
            "double-ended: the mirrored arrows join the batches"
        );

        assert_eq!(
            modes_from_names(["translate", "flip"]),
            GadgetModes::TRANSLATE.with(GadgetModes::FLIP)
        );
        assert_eq!(
            modes_from_names(["rotate", "nonsense"]),
            GadgetModes::ROTATE
        );
        assert_eq!(modes_from_names::<&str>([]), GadgetModes::NONE);
    }

    #[test]
    fn a_snapped_drag_emits_only_on_the_step_and_one_gadget_holds_one_drag() {
        let mut g = gadget(GizmoMode::Translate);
        let press = ray(Vec3::new(9.0, 0.0, 0.0), Vec3::Y);
        assert!(g.begin(Projection::Perspective, press, Some(5.0)));
        assert!(!g.begin(Projection::Perspective, press, None), "one drag");
        // Two units along a five-unit grid is still the same grid point.
        assert_eq!(g.update(ray(Vec3::new(11.0, 0.0, 0.0), Vec3::Y)), None);
        let stepped = g.update(ray(Vec3::new(12.5, 0.0, 0.0), Vec3::Y));
        assert_eq!(
            stepped,
            Some(GadgetDelta::Translate(Vec3::new(5.0, 0.0, 0.0)))
        );
    }
}
