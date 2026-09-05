//! Renderer-agnostic TRS transform-gizmo geometry + picking (the Blender/Maya-style manipulator).
//!
//! Pure glam — every function returns `(Vec3, Vec3, [f32;4])` coloured line segments (or takes/returns
//! rays as `(origin, dir)` tuples), so any editor can draw it through its own line pipeline and reuse
//! the picking math. NO renderer / assetpipeline / source-model types enter here: the gizmo operates on
//! a plain `origin` + `basis` (world axes as `Mat3` columns), so it works on ANY conformed rig.
//!
//! All four gadget manipulations are here (gadget direction F28531B5 — ONE reusable 3D gadget):
//! three CONTINUOUS drags — TRANSLATE (arrows), ROTATE (rings), SCALE (axis boxes + a uniform
//! centre) — and the discrete FLIP. Each drag has the SAME two forms: an axis-locked one derived
//! from the picked handle ([`drag_translate`] / [`drag_rotate`] / [`drag_scale`]) and a
//! view-relative free one ([`drag_plane`] / [`drag_angle`] / [`drag_scale_uniform`]).
//! [`DragState`] wraps either with the axis lock and the snap step.
//!
//! [`GizmoMode`] enumerates the three DRAGS — it is what geometry and picking branch on.
//! [`GadgetModes`] is the four-mode surface gate, Flip included, that a scene's Lua sets.
//!
//! Snapping quantizes the ACCUMULATED value, never the per-frame step: the raw sweep runs
//! continuously underneath so a long snapped drag lands on exact multiples instead of drifting by
//! every frame's rounding.
//!
//! FLIP is guarded. A reflection is not always representable — the CornerVector range is asymmetric,
//! so a corner past +1.5 mirrors outside the legal range (invariant C670523A, the Landmark bug). So
//! [`flip`] takes the domain's validator and REFUSES ([`FlipRefused`]) rather than handing back a
//! clamped, silently-wrong mirror.

use glam::{Mat3, Mat4, Quat, Vec3};

use crate::collision::closest_point_ray_segment;

/// Segments of a ring / round arrowhead sweep — enough to read as a circle at gizmo scale.
const RING_SEGMENTS: usize = 24;
/// Arrowhead length as a fraction of the handle length.
const HEAD_LEN_FRAC: f32 = 0.22;
/// Arrowhead half-width as a fraction of the handle length.
const HEAD_WIDTH_FRAC: f32 = 0.08;
/// Rotate-ring radius as a fraction of the handle length.
const RING_RADIUS_FRAC: f32 = 0.85;
/// Scale-handle end-box half-extent as a fraction of the handle length.
const BOX_HALF_FRAC: f32 = 0.08;
/// The smallest scale ratio a drag will ever produce. A gadget never scales a thing to zero, and
/// never THROUGH zero into a reflection — mirroring is [`flip`]'s job, and it is guarded.
pub const MIN_SCALE: f32 = 1e-3;

/// Which continuous manipulation the gizmo performs. Geometry, picking and drag math branch on this.
///
/// The gadget's FOURTH mode, Flip, is not here: a flip is a discrete op ([`flip`]), not a sweep, and
/// it is gated by [`GadgetModes::FLIP`] rather than selected as a drag. (It also cannot be added to
/// this enum today without breaking Clayworks, which matches it exhaustively — see the deadend
/// banked with this slice.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GizmoMode {
    /// Move along an axis (or, unlocked, in the view plane).
    #[default]
    Translate,
    /// Turn about an axis (or, unlocked, about the view direction).
    Rotate,
    /// Scale along an axis (or, unlocked, uniformly from the centre handle).
    Scale,
}

impl GizmoMode {
    /// All three drag modes, in the gadget's cycle order.
    pub const ALL: [GizmoMode; 3] = [GizmoMode::Translate, GizmoMode::Rotate, GizmoMode::Scale];

    /// This mode's bit in a [`GadgetModes`] set.
    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// Which of the gadget's four manipulations a surface allows — pure DATA, no rendering and no input
/// reads. This is the per-surface gate the gadget direction (F28531B5) puts in the scene's Lua: a
/// surface that only places things ships `TRANSLATE`, one whose geometry has no legal reflection
/// drops `FLIP`. `Default` is [`GadgetModes::ALL`] — a surface that says nothing gets the whole
/// gadget.
///
/// The three drag modes are asked about with [`GadgetModes::allows`], the discrete fourth with
/// [`GadgetModes::allows_flip`], mirroring the split in [`GizmoMode`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GadgetModes(u8);

impl GadgetModes {
    /// Nothing enabled — the gadget is inert.
    pub const NONE: Self = Self(0);
    /// Only [`GizmoMode::Translate`].
    pub const TRANSLATE: Self = Self(GizmoMode::Translate.bit());
    /// Only [`GizmoMode::Rotate`].
    pub const ROTATE: Self = Self(GizmoMode::Rotate.bit());
    /// Only [`GizmoMode::Scale`].
    pub const SCALE: Self = Self(GizmoMode::Scale.bit());
    /// Only the discrete mirror op, [`flip`] — the bit after the three drag modes'.
    pub const FLIP: Self = Self(1 << 3);
    /// Every mode.
    pub const ALL: Self = Self(0b1111);

    /// The union — `GadgetModes::TRANSLATE.with(GadgetModes::FLIP)`.
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The difference — `GadgetModes::ALL.without(GadgetModes::FLIP)`.
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Is this drag `mode` enabled on the surface?
    pub const fn allows(self, mode: GizmoMode) -> bool {
        self.0 & mode.bit() != 0
    }

    /// May this surface mirror at all? A surface whose domain has no legal reflection answers `false`
    /// and the op never appears — the invariant's "disable the invalid op" tier at SURFACE scope,
    /// where [`FlipRefused`] is the same answer at per-axis scope.
    pub const fn allows_flip(self) -> bool {
        self.0 & Self::FLIP.0 != 0
    }

    /// The handles a pick may land on for `mode`: its three axes, or NOTHING when the surface gates
    /// that mode off. A disabled mode has nothing to grab — that is the whole point of the gate.
    pub fn handles_for(self, mode: GizmoMode) -> &'static [Axis] {
        if self.allows(mode) {
            &Axis::ALL
        } else {
            &[]
        }
    }

    /// The axes a mirror may be taken about — all three, or NOTHING when `FLIP` is gated off.
    /// [`handles_for`](Self::handles_for)'s counterpart for the discrete mode.
    pub fn flip_handles(self) -> &'static [Axis] {
        if self.allows_flip() {
            &Axis::ALL
        } else {
            &[]
        }
    }

    /// The enabled DRAG modes in cycle order — what a mode-next / mode-prev walks.
    pub fn modes(self) -> impl Iterator<Item = GizmoMode> {
        GizmoMode::ALL.into_iter().filter(move |&m| self.allows(m))
    }
}

impl Default for GadgetModes {
    fn default() -> Self {
        Self::ALL
    }
}

/// One of the three gizmo axes. `X`→red, `Y`→green, `Z`→blue (the standard R=X/G=Y/B=Z convention).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// All three axes, in X, Y, Z order.
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    /// Unit direction in the gizmo's LOCAL frame (before `basis` is applied).
    pub fn unit(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }

    /// Base RGBA for this axis (full alpha): R=X, G=Y, B=Z.
    pub fn color(self) -> [f32; 4] {
        match self {
            Axis::X => [0.90, 0.25, 0.25, 1.0],
            Axis::Y => [0.35, 0.90, 0.35, 1.0],
            Axis::Z => [0.35, 0.55, 1.00, 1.0],
        }
    }

    /// Brightened RGBA for the hovered/active axis (pushed toward white, alpha kept).
    pub fn hover_color(self) -> [f32; 4] {
        let [r, g, b, a] = self.color();
        [(r + 1.0) * 0.5, (g + 1.0) * 0.5, (b + 1.0) * 0.5, a]
    }
}

/// The world-space unit direction of `axis` under `basis` (basis columns = the gizmo's world axes).
/// Falls back to the raw axis if `basis` collapses the column to zero.
///
/// Public because it is the module's ONE convention for "where does this handle point in the
/// world", and a surface that draws or hides handles must ask the same question the geometry, the
/// picking and the drag math ask — a second copy of these three lines would be a second source of
/// truth for the gadget's frame.
pub fn world_axis(basis: Mat3, axis: Axis) -> Vec3 {
    let n = (basis * axis.unit()).normalize_or_zero();
    if n == Vec3::ZERO {
        axis.unit()
    } else {
        n
    }
}

/// Two unit vectors spanning the plane perpendicular to `dir` (assumed roughly unit).
fn perp_basis(dir: Vec3) -> (Vec3, Vec3) {
    let seed = if dir.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = dir.cross(seed).normalize_or_zero();
    let u = if u == Vec3::ZERO { Vec3::Y } else { u };
    let v = dir.cross(u).normalize_or_zero();
    let v = if v == Vec3::ZERO { Vec3::Z } else { v };
    (u, v)
}

/// Gizmo handle line segments, each carrying its own RGBA (the hovered axis is brightened).
///
/// - `origin` = gizmo centre in the caller's draw space.
/// - `basis`  = columns are the gizmo's world axes (`Mat3::IDENTITY` = world-aligned; a bone rotation
///   for a local gizmo). Axis directions are re-normalised, so a non-orthonormal basis is tolerated.
/// - `size`   = handle length in that space.
/// - `hover`  = axis to highlight, if any.
///
/// Translate: per axis a shaft + a two-line V arrowhead (3 segments). Rotate: a ring per axis.
/// Scale: per axis a shaft + a small end box. A surface in the discrete Flip mode picks its axis on
/// whichever handles it is already showing — the mirror has no handles of its own.
pub fn gizmo_segments(
    origin: Vec3,
    basis: Mat3,
    mode: GizmoMode,
    size: f32,
    hover: Option<Axis>,
) -> Vec<(Vec3, Vec3, [f32; 4])> {
    let mut segs = Vec::new();
    for axis in Axis::ALL {
        let color = if hover == Some(axis) {
            axis.hover_color()
        } else {
            axis.color()
        };
        let dir = world_axis(basis, axis);
        match mode {
            GizmoMode::Translate => push_arrow(&mut segs, origin, dir, size, color),
            GizmoMode::Rotate => push_ring(&mut segs, origin, dir, size * RING_RADIUS_FRAC, color),
            GizmoMode::Scale => push_scale_handle(&mut segs, origin, dir, size, color),
        }
    }
    segs
}

/// Shaft `origin → tip` plus a two-line V arrowhead at the tip.
fn push_arrow(
    segs: &mut Vec<(Vec3, Vec3, [f32; 4])>,
    origin: Vec3,
    dir: Vec3,
    size: f32,
    color: [f32; 4],
) {
    let tip = origin + dir * size;
    segs.push((origin, tip, color));
    let (u, _) = perp_basis(dir);
    let back = tip - dir * (size * HEAD_LEN_FRAC);
    let w = u * (size * HEAD_WIDTH_FRAC);
    segs.push((tip, back + w, color));
    segs.push((tip, back - w, color));
}

/// A segmented ring of `radius` about `origin` in the plane perpendicular to `dir`.
fn push_ring(
    segs: &mut Vec<(Vec3, Vec3, [f32; 4])>,
    origin: Vec3,
    dir: Vec3,
    radius: f32,
    color: [f32; 4],
) {
    let (u, v) = perp_basis(dir);
    let point = |t: f32| origin + (u * t.cos() + v * t.sin()) * radius;
    let mut prev = point(0.0);
    for i in 1..=RING_SEGMENTS {
        let t = (i as f32) / (RING_SEGMENTS as f32) * std::f32::consts::TAU;
        let p = point(t);
        segs.push((prev, p, color));
        prev = p;
    }
}

/// Shaft `origin → tip` plus a small axis-aligned-to-`dir` box at the tip (12 edges).
fn push_scale_handle(
    segs: &mut Vec<(Vec3, Vec3, [f32; 4])>,
    origin: Vec3,
    dir: Vec3,
    size: f32,
    color: [f32; 4],
) {
    let tip = origin + dir * size;
    segs.push((origin, tip, color));
    let (u, v) = perp_basis(dir);
    let h = size * BOX_HALF_FRAC;
    let corner = |sx: f32, sy: f32, sz: f32| tip + (dir * sx + u * sy + v * sz) * h;
    let c = [
        corner(-1.0, -1.0, -1.0),
        corner(1.0, -1.0, -1.0),
        corner(1.0, 1.0, -1.0),
        corner(-1.0, 1.0, -1.0),
        corner(-1.0, -1.0, 1.0),
        corner(1.0, -1.0, 1.0),
        corner(1.0, 1.0, 1.0),
        corner(-1.0, 1.0, 1.0),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for &(i, j) in &EDGES {
        segs.push((c[i], c[j], color));
    }
}

/// Nearest axis handle the ray `(ray_origin, ray_dir)` hits within `max_dist` (draw-space units),
/// else `None`. Rays are `(origin, dir)` to match `Camera::pick_ray`. Translate/Scale test the shaft
/// (a finite segment); Rotate tests the ring. Returns the closest-miss axis under the threshold.
///
/// The MODE gate is the caller's: a surface picks in the mode it is in, and [`GadgetModes`] decides
/// which modes it may be in at all ([`GadgetModes::handles_for`] is empty for a gated-off mode).
pub fn pick_handle(
    ray_origin: Vec3,
    ray_dir: Vec3,
    origin: Vec3,
    basis: Mat3,
    mode: GizmoMode,
    size: f32,
    max_dist: f32,
) -> Option<Axis> {
    pick_handle_among(
        (ray_origin, ray_dir),
        origin,
        basis,
        mode,
        size,
        max_dist,
        &Axis::ALL,
    )
}

/// [`pick_handle`] restricted to the handles a surface is actually SHOWING.
///
/// `handles` is [`GadgetModes::handles_for`] (empty for a gated-off mode, so nothing is grabbable)
/// less whatever the view degenerates: an ORTHOGRAPHIC panel's depth axis projects to a point, and
/// a ray aimed near the pivot runs down its whole shaft, so leaving it in would let it win every
/// pick. A handle that is not drawn must not be pickable — that is the whole of this parameter.
///
/// Takes the ray as `(origin, dir)`, the convention every drag function here already uses.
pub fn pick_handle_among(
    ray: (Vec3, Vec3),
    origin: Vec3,
    basis: Mat3,
    mode: GizmoMode,
    size: f32,
    max_dist: f32,
    handles: &[Axis],
) -> Option<Axis> {
    let (ray_origin, ray_dir) = ray;
    let mut best: Option<(Axis, f32)> = None;
    for &axis in handles {
        let dir = world_axis(basis, axis);
        let miss = match mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                let tip = origin + dir * size;
                let (pr, ps) = closest_point_ray_segment(ray_origin, ray_dir, origin, tip);
                (pr - ps).length()
            }
            GizmoMode::Rotate => {
                ring_miss(ray_origin, ray_dir, origin, dir, size * RING_RADIUS_FRAC)
            }
        };
        if miss <= max_dist && best.map(|(_, m)| miss < m).unwrap_or(true) {
            best = Some((axis, miss));
        }
    }
    best.map(|(a, _)| a)
}

/// Miss distance from ray `(o, d)` to a ring of `radius` about `center` with plane normal `n`.
/// Intersect the ray with the ring plane, then take |radial distance − radius|; `f32::INFINITY` if
/// the ray is parallel to the plane or hits behind the origin.
fn ring_miss(o: Vec3, d: Vec3, center: Vec3, n: Vec3, radius: f32) -> f32 {
    let dn = d.dot(n);
    if dn.abs() < 1e-6 {
        return f32::INFINITY;
    }
    let t = (center - o).dot(n) / dn;
    if t <= 0.0 {
        return f32::INFINITY;
    }
    let p = o + d * t;
    ((p - center).length() - radius).abs()
}

/// Axis-constrained WORLD translation delta between two cursor rays. Projects each ray's closest
/// point onto the INFINITE axis line through `origin` (direction `basis * axis`), and returns
/// `axis_dir * (param_now − param_prev)`. `Vec3::ZERO` when a ray runs (near-)parallel to the axis
/// (the projection is ill-conditioned) — the caller holds the previous value that frame.
///
/// The result is a WORLD delta; the caller converts world → parent-local before writing
/// `BoneOffset.t` (that conversion, and the units, are the assetpipeline seam's job, not the core's).
pub fn drag_translate(
    axis: Axis,
    basis: Mat3,
    origin: Vec3,
    ray_prev: (Vec3, Vec3),
    ray_now: (Vec3, Vec3),
) -> Vec3 {
    let u = world_axis(basis, axis);
    let (Some(s_prev), Some(s_now)) = (
        axis_param(ray_prev.0, ray_prev.1, origin, u),
        axis_param(ray_now.0, ray_now.1, origin, u),
    ) else {
        return Vec3::ZERO;
    };
    u * (s_now - s_prev)
}

/// Free PLANAR translation delta between two cursor rays, in the plane through `origin` with unit
/// `normal` — for dragging in an ORTHOGRAPHIC view, where the view direction is the locked axis and
/// the joint moves in the two axes you SEE. Intersects each ray with the plane and returns the
/// in-plane displacement (`Vec3::ZERO` when a ray runs near-parallel to the plane). Like
/// [`drag_translate`], the result is a WORLD delta the caller converts to parent-local.
pub fn drag_plane(
    normal: Vec3,
    origin: Vec3,
    ray_prev: (Vec3, Vec3),
    ray_now: (Vec3, Vec3),
) -> Vec3 {
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return Vec3::ZERO;
    }
    match (
        plane_hit(n, origin, ray_prev),
        plane_hit(n, origin, ray_now),
    ) {
        (Some(a), Some(b)) => b - a,
        _ => Vec3::ZERO,
    }
}

/// Where the ray meets the plane through `origin` with unit `normal`; `None` when the ray runs
/// (near-)parallel to it. Shared by [`drag_plane`] and [`drag_angle`].
fn plane_hit(normal: Vec3, origin: Vec3, (o, d): (Vec3, Vec3)) -> Option<Vec3> {
    let denom = d.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    Some(o + d * ((origin - o).dot(normal) / denom))
}

/// Signed angle (RADIANS) the cursor sweeps about `origin` between two rays, measured in the plane
/// whose unit `normal` is the rotation axis — right-handed about `normal`, and unbounded (a sweep
/// past half a turn keeps counting, because the caller accumulates frame by frame rather than
/// re-measuring from the press).
///
/// `0.0` when a ray runs (near-)parallel to the plane or lands on the pivot (the sweep is
/// ill-conditioned) — the caller holds the previous value that frame, exactly as [`drag_plane`] does.
/// This is ROTATE's free form: pass the view direction for the screen-space ring.
pub fn drag_angle(
    normal: Vec3,
    origin: Vec3,
    ray_prev: (Vec3, Vec3),
    ray_now: (Vec3, Vec3),
) -> f32 {
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return 0.0;
    }
    let (Some(a), Some(b)) = (
        plane_hit(n, origin, ray_prev),
        plane_hit(n, origin, ray_now),
    ) else {
        return 0.0;
    };
    let (ra, rb) = (a - origin, b - origin);
    if ra.length() < 1e-6 || rb.length() < 1e-6 {
        return 0.0; // on the pivot: no radius, no angle
    }
    ra.cross(rb).dot(n).atan2(ra.dot(rb))
}

/// Axis-constrained delta ROTATION between two cursor rays: the sweep about the picked ring's world
/// axis (`basis * axis`) through `origin`, as a quaternion. `Quat::IDENTITY` when the sweep is
/// ill-conditioned. Rotate's answer to [`drag_translate`], with the same shape; deltas COMPOSE, so a
/// caller can multiply each frame's result onto the pose as it arrives.
pub fn drag_rotate(
    axis: Axis,
    basis: Mat3,
    origin: Vec3,
    ray_prev: (Vec3, Vec3),
    ray_now: (Vec3, Vec3),
) -> Quat {
    let n = world_axis(basis, axis);
    Quat::from_axis_angle(n, drag_angle(n, origin, ray_prev, ray_now))
}

/// Axis-constrained SCALE ratio between two cursor rays: how much further along the handle's axis
/// line the cursor now projects than it did (`s_now / s_prev`). `1.0` when either projection is
/// ill-conditioned or the previous one sat on the pivot; clamped to [`MIN_SCALE`], so a drag through
/// the pivot pins instead of flipping the sign. Scale's answer to [`drag_translate`]; ratios
/// COMPOSE multiplicatively across frames.
pub fn drag_scale(
    axis: Axis,
    basis: Mat3,
    origin: Vec3,
    ray_prev: (Vec3, Vec3),
    ray_now: (Vec3, Vec3),
) -> f32 {
    let u = world_axis(basis, axis);
    let (Some(s_prev), Some(s_now)) = (
        axis_param(ray_prev.0, ray_prev.1, origin, u),
        axis_param(ray_now.0, ray_now.1, origin, u),
    ) else {
        return 1.0;
    };
    if s_prev.abs() < 1e-6 {
        return 1.0;
    }
    (s_now / s_prev).max(MIN_SCALE)
}

/// UNIFORM scale ratio between two cursor rays — SCALE's free form, for the centre handle: the ratio
/// of the cursor's distance from `origin`, taken as each ray's closest approach to the pivot (which
/// is the on-screen radial distance in an orthographic view, with no view matrix needed here).
/// `1.0` when the previous ray passed through the pivot; clamped to [`MIN_SCALE`].
pub fn drag_scale_uniform(origin: Vec3, ray_prev: (Vec3, Vec3), ray_now: (Vec3, Vec3)) -> f32 {
    let reach = |(o, d): (Vec3, Vec3)| {
        let (pr, _) = closest_point_ray_segment(o, d, origin, origin);
        (pr - origin).length()
    };
    let prev = reach(ray_prev);
    if prev < 1e-6 {
        return 1.0;
    }
    (reach(ray_now) / prev).max(MIN_SCALE)
}

/// Per-axis scale factors for a handle ratio, in the gizmo's LOCAL frame (the caller composes with
/// `basis`): the locked axis takes `ratio` and the others stay `1.0`; `None` — the centre/uniform
/// handle — scales all three. The ratio is clamped to [`MIN_SCALE`].
pub fn scale_factors(axis: Option<Axis>, ratio: f32) -> Vec3 {
    let r = ratio.max(MIN_SCALE);
    match axis {
        None => Vec3::splat(r),
        Some(Axis::X) => Vec3::new(r, 1.0, 1.0),
        Some(Axis::Y) => Vec3::new(1.0, r, 1.0),
        Some(Axis::Z) => Vec3::new(1.0, 1.0, r),
    }
}

/// The MIRROR transform for `axis`: the reflection in the plane through `pivot` whose normal is that
/// handle's world direction (`basis * axis`) — a Householder reflection carried to the pivot.
///
/// UNGUARDED: this is the permissive form, for domains where every reflection is representable. Use
/// [`flip`] wherever it is not (invariant C670523A).
pub fn flip_matrix(axis: Axis, basis: Mat3, pivot: Vec3) -> Mat4 {
    let n = world_axis(basis, axis);
    let r = Mat3::from_cols(
        Vec3::X - 2.0 * n.x * n,
        Vec3::Y - 2.0 * n.y * n,
        Vec3::Z - 2.0 * n.z * n,
    );
    Mat4::from_translation(pivot) * Mat4::from_mat3(r) * Mat4::from_translation(-pivot)
}

/// A flip the domain refused: mirroring about `axis` would put the result outside the legal range,
/// so NOTHING was produced and the caller's geometry is untouched.
///
/// This is the typed refusal invariant C670523A demands. A clamped mirror is not a fallback — it is
/// a silently wrong shape, which is the bug the invariant exists to prevent. The `axis` is here so a
/// surface can gray the handle out (the invariant's "disable the invalid op" tier).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FlipRefused {
    /// The axis whose reflection has no legal result.
    pub axis: Axis,
}

/// Mirror about `axis`, GUARDED by the domain's own validator.
///
/// `valid` is handed the candidate [`flip_matrix`] and answers whether the result it would produce
/// is representable — a voxel caller closes over its corners and checks each
/// `m.transform_point3(corner)` against the CornerVector range. The mechanics layer cannot know that
/// range (it is renderer- and domain-agnostic), so the range check is the CALLER's and this is the
/// seam it plugs into. A `false` answer is `Err(FlipRefused)`: no transform, no clamp, no partial
/// application. For the permissive case, call [`flip_matrix`] directly.
pub fn flip(
    axis: Axis,
    basis: Mat3,
    pivot: Vec3,
    valid: impl FnOnce(&Mat4) -> bool,
) -> Result<Mat4, FlipRefused> {
    let m = flip_matrix(axis, basis, pivot);
    if valid(&m) {
        Ok(m)
    } else {
        Err(FlipRefused { axis })
    }
}

/// One frame of a drag, in the picked mode's own currency. All three COMPOSE — translations add,
/// rotations multiply, scale factors multiply — so a caller applies each frame's delta as it lands.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DragDelta {
    /// WORLD translation to add (the caller converts world → parent-local).
    Translate(Vec3),
    /// Rotation to compose about the drag's pivot, about the WORLD handle axis.
    Rotate(Quat),
    /// Per-axis multiplicative factors in the gizmo's LOCAL frame; `Vec3::ONE` is no change.
    Scale(Vec3),
}

impl DragDelta {
    /// Did this frame move nothing? The check every caller needs before it touches the document —
    /// a snapped drag emits identity on most frames by design, and an ill-conditioned ray emits it
    /// too.
    pub fn is_identity(self) -> bool {
        match self {
            DragDelta::Translate(v) => v.abs_diff_eq(Vec3::ZERO, 1e-6),
            DragDelta::Rotate(q) => 1.0 - q.w.abs() < 1e-6,
            DragDelta::Scale(s) => s.abs_diff_eq(Vec3::ONE, 1e-6),
        }
    }
}

/// A live drag: the picked handle, its constraints, and the accumulator that keeps SNAPPING
/// drift-free.
///
/// The axis lock comes from the handle that was picked. `None` is each mode's view-relative FREE
/// form — one rule across all three: Translate drags in the press ray's plane, Rotate turns about the
/// press ray's direction (the screen-space ring), Scale scales uniformly from the centre.
///
/// The snap step quantizes the ACCUMULATED value, never the frame's step: `raw` keeps running
/// continuously underneath and only the difference between successive QUANTIZED totals is ever
/// emitted. That is why a long snapped drag lands on exact multiples — quantizing each frame's step
/// instead would round away a little of every frame and drift off the grid.
///
/// The gadget's fourth mode, the mirror, is discrete and has no drag at all: the surface calls
/// [`flip`] on the press instead of beginning a state.
#[derive(Clone, Copy, Debug)]
pub struct DragState {
    mode: GizmoMode,
    axis: Option<Axis>,
    basis: Mat3,
    origin: Vec3,
    /// The PRESS ray's direction — the plane normal / rotation axis of the free forms.
    view: Vec3,
    /// Snap step in the mode's own currency, already in working units (radians for Rotate);
    /// `0.0` is continuous.
    snap: f32,
    /// The previous frame's ray.
    ray: (Vec3, Vec3),
    /// Raw continuous accumulation. A free Translate accumulates the whole in-plane vector; every
    /// other form is a scalar in `.x` — locked distance, radians turned, or the scale ratio.
    raw: Vec3,
    /// The last quantized value handed to the caller, in the same shape as `raw`.
    emitted: Vec3,
}

impl DragState {
    /// Begin a drag on a picked handle.
    ///
    /// - `axis` — the handle's axis, or `None` for the mode's free form (see the type docs).
    /// - `basis` — columns are the gizmo's world axes, as everywhere else in this module.
    /// - `origin` — the pivot the drag turns/scales about and measures against.
    /// - `ray` — the ray at the PRESS; its direction is the free forms' view axis.
    /// - `snap` — the step, in DISTANCE for Translate, DEGREES for Rotate, RATIO for Scale.
    ///   `None` (or a non-positive step) drags continuously.
    pub fn begin(
        mode: GizmoMode,
        axis: Option<Axis>,
        basis: Mat3,
        origin: Vec3,
        ray: (Vec3, Vec3),
        snap: Option<f32>,
    ) -> Self {
        let view = ray.1.normalize_or_zero();
        let step = snap.unwrap_or(0.0).max(0.0);
        // Scale accumulates a RATIO, whose neutral value is 1; the additive modes start at 0.
        let zero = match mode {
            GizmoMode::Scale => Vec3::new(1.0, 0.0, 0.0),
            _ => Vec3::ZERO,
        };
        Self {
            mode,
            axis,
            basis,
            origin,
            view: if view == Vec3::ZERO { Vec3::Z } else { view },
            // Rotate's step arrives in degrees but accumulates in radians; convert once, here.
            snap: if mode == GizmoMode::Rotate {
                step.to_radians()
            } else {
                step
            },
            ray,
            raw: zero,
            emitted: zero,
        }
    }

    /// The mode this drag was begun in.
    pub fn mode(&self) -> GizmoMode {
        self.mode
    }

    /// The locked axis, or `None` for the free form — what the surface highlights while dragging.
    pub fn axis(&self) -> Option<Axis> {
        self.axis
    }

    /// The drag's SNAPPED total so far, in the mode's display currency: world distance along the
    /// locked axis (its LENGTH for a free planar drag), DEGREES turned, or the scale RATIO. This is
    /// the number a readout or a ghost preview shows.
    pub fn total(&self) -> f32 {
        match (self.mode, self.axis) {
            (GizmoMode::Translate, None) => self.emitted.length(),
            (GizmoMode::Rotate, _) => self.emitted.x.to_degrees(),
            _ => self.emitted.x,
        }
    }

    /// Advance the drag with this frame's cursor ray and return what to apply THIS FRAME.
    ///
    /// A ray the mode cannot resolve (parallel to the axis or the plane, or through the pivot)
    /// contributes nothing and the drag holds — the same contract [`drag_translate`] has.
    pub fn update(&mut self, ray_now: (Vec3, Vec3)) -> DragDelta {
        let prev = self.ray;
        self.ray = ray_now;
        match self.mode {
            GizmoMode::Translate => match self.axis {
                Some(a) => {
                    let u = world_axis(self.basis, a);
                    self.raw.x += drag_translate(a, self.basis, self.origin, prev, ray_now).dot(u);
                    DragDelta::Translate(u * self.step_scalar())
                }
                None => {
                    self.raw += drag_plane(self.view, self.origin, prev, ray_now);
                    let q = Vec3::new(
                        quantize(self.raw.x, self.snap),
                        quantize(self.raw.y, self.snap),
                        quantize(self.raw.z, self.snap),
                    );
                    let step = q - self.emitted;
                    self.emitted = q;
                    DragDelta::Translate(step)
                }
            },
            GizmoMode::Rotate => {
                let n = self.rotation_axis();
                self.raw.x += drag_angle(n, self.origin, prev, ray_now);
                DragDelta::Rotate(Quat::from_axis_angle(n, self.step_scalar()))
            }
            GizmoMode::Scale => {
                let r = match self.axis {
                    Some(a) => drag_scale(a, self.basis, self.origin, prev, ray_now),
                    None => drag_scale_uniform(self.origin, prev, ray_now),
                };
                self.raw.x = (self.raw.x * r).max(MIN_SCALE);
                let q = quantize(self.raw.x, self.snap).max(MIN_SCALE);
                let step = q / self.emitted.x;
                self.emitted.x = q;
                DragDelta::Scale(scale_factors(self.axis, step))
            }
        }
    }

    /// The rotation axis: the locked handle's world direction, or the press ray's for the free ring.
    fn rotation_axis(&self) -> Vec3 {
        self.axis
            .map(|a| world_axis(self.basis, a))
            .unwrap_or(self.view)
    }

    /// Quantize the accumulated scalar and hand back only what has not been emitted yet — the whole
    /// of the no-drift discipline, in three lines.
    fn step_scalar(&mut self) -> f32 {
        let q = quantize(self.raw.x, self.snap);
        let step = q - self.emitted.x;
        self.emitted.x = q;
        step
    }
}

/// Round `v` to the nearest multiple of `step`; a non-positive `step` is continuous (`v` unchanged).
fn quantize(v: f32, step: f32) -> f32 {
    if step > 0.0 {
        (v / step).round() * step
    } else {
        v
    }
}

/// Closest parameter `s` on the infinite line `origin + s*u` (u unit) to the ray `(o, d)`.
/// `None` when the ray is near-parallel to the line (denominator → 0).
fn axis_param(o: Vec3, d: Vec3, origin: Vec3, u: Vec3) -> Option<f32> {
    let w0 = o - origin;
    let a = d.dot(d);
    let b = d.dot(u); // u is unit, so u·u = 1
    let denom = a - b * b; // = a*c − b² with c = 1
    if denom < 1e-6 {
        return None;
    }
    let e = u.dot(w0);
    let dd = d.dot(w0);
    Some((a * e - b * dd) / denom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_segments_count_and_axis_colours() {
        let segs = gizmo_segments(Vec3::ZERO, Mat3::IDENTITY, GizmoMode::Translate, 10.0, None);
        // 3 axes × (shaft + 2 arrowhead lines) = 9.
        assert_eq!(segs.len(), 9);
        // The first segment of each axis is its shaft, carrying that axis' base colour.
        assert_eq!(segs[0].2, Axis::X.color());
        assert_eq!(segs[3].2, Axis::Y.color());
        assert_eq!(segs[6].2, Axis::Z.color());
        // X shaft runs origin → +X*size.
        assert!((segs[0].0 - Vec3::ZERO).length() < 1e-5);
        assert!((segs[0].1 - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn hover_brightens_only_the_hovered_axis() {
        let segs = gizmo_segments(
            Vec3::ZERO,
            Mat3::IDENTITY,
            GizmoMode::Translate,
            10.0,
            Some(Axis::Y),
        );
        assert_eq!(segs[0].2, Axis::X.color(), "X unchanged");
        assert_eq!(segs[3].2, Axis::Y.hover_color(), "Y brightened");
        assert_ne!(Axis::Y.color(), Axis::Y.hover_color());
    }

    #[test]
    fn rotate_and_scale_geometry_present() {
        let rings = gizmo_segments(Vec3::ZERO, Mat3::IDENTITY, GizmoMode::Rotate, 10.0, None);
        assert_eq!(rings.len(), 3 * RING_SEGMENTS);
        let boxes = gizmo_segments(Vec3::ZERO, Mat3::IDENTITY, GizmoMode::Scale, 10.0, None);
        // 3 axes × (shaft + 12 box edges) = 39.
        assert_eq!(boxes.len(), 3 * 13);
    }

    #[test]
    fn pick_hits_the_intended_axis() {
        // A ray straight down −Z through the middle of the +X shaft → picks X.
        let hit = pick_handle(
            Vec3::new(5.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::ZERO,
            Mat3::IDENTITY,
            GizmoMode::Translate,
            10.0,
            1.0,
        );
        assert_eq!(hit, Some(Axis::X));
    }

    #[test]
    fn pick_misses_past_the_threshold() {
        // Same ray offset +5 in Y → 5 units off every shaft, beyond max_dist 1 → None.
        let hit = pick_handle(
            Vec3::new(5.0, 5.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::ZERO,
            Mat3::IDENTITY,
            GizmoMode::Translate,
            10.0,
            1.0,
        );
        assert_eq!(hit, None);
    }

    #[test]
    fn pick_chooses_the_nearest_of_two_candidate_axes() {
        // Ray down −Z passing nearer the Y shaft than the X shaft.
        let hit = pick_handle(
            Vec3::new(0.3, 5.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::ZERO,
            Mat3::IDENTITY,
            GizmoMode::Translate,
            10.0,
            2.0,
        );
        assert_eq!(hit, Some(Axis::Y));
    }

    #[test]
    fn drag_delta_is_axis_aligned_and_correct_magnitude() {
        // Two −Z rays whose lines sit at x=0 then x=3 → +X delta of 3.
        let delta = drag_translate(
            Axis::X,
            Mat3::IDENTITY,
            Vec3::ZERO,
            (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -1.0)),
            (Vec3::new(3.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -1.0)),
        );
        assert!(
            (delta - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4,
            "delta {delta:?}"
        );
    }

    #[test]
    fn drag_parallel_ray_is_a_no_op() {
        // Ray running ALONG the X axis → projection ill-conditioned → zero delta (hold previous).
        let delta = drag_translate(
            Axis::X,
            Mat3::IDENTITY,
            Vec3::ZERO,
            (Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            (Vec3::new(5.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
        );
        assert_eq!(delta, Vec3::ZERO);
    }

    #[test]
    fn drag_respects_a_rotated_basis() {
        // Basis rotates local X onto world +Y; a drag along Axis::X should move in world +Y.
        let basis = Mat3::from_cols(Vec3::Y, Vec3::X, Vec3::Z);
        let delta = drag_translate(
            Axis::X,
            basis,
            Vec3::ZERO,
            (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -1.0)),
            (Vec3::new(0.0, 4.0, 10.0), Vec3::new(0.0, 0.0, -1.0)),
        );
        assert!(
            (delta - Vec3::new(0.0, 4.0, 0.0)).length() < 1e-4,
            "delta {delta:?}"
        );
    }

    #[test]
    fn drag_plane_moves_in_the_view_plane_and_locks_the_view_axis() {
        // View looking along -Z (a Top view): the plane is XY through the origin.
        let n = Vec3::Z;
        let prev = (Vec3::new(1.0, 2.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let now = (Vec3::new(4.0, 6.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let d = drag_plane(n, Vec3::ZERO, prev, now);
        assert!(
            (d - Vec3::new(3.0, 4.0, 0.0)).length() < 1e-4,
            "in-plane XY move, Z locked: {d:?}"
        );
        // A ray parallel to the plane cannot resolve a point → no move.
        let par = (Vec3::ZERO, Vec3::X);
        assert_eq!(drag_plane(n, Vec3::ZERO, par, par), Vec3::ZERO);
    }

    /// A −Z ray whose line passes through `(x, y)` — a Top-view cursor over the XY plane.
    fn down(x: f32, y: f32) -> (Vec3, Vec3) {
        (Vec3::new(x, y, 10.0), Vec3::new(0.0, 0.0, -1.0))
    }

    // ── Rotate ────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_quarter_sweep_turns_ninety_degrees_about_the_handle_axis() {
        // Cursor swings +X → +Y about the Z ring: a right-handed quarter turn about +Z.
        let q = drag_rotate(
            Axis::Z,
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(10.0, 0.0),
            down(0.0, 10.0),
        );
        let (n, a) = q.to_axis_angle();
        assert!((n - Vec3::Z).length() < 1e-4, "about +Z: {n:?}");
        assert!((a.to_degrees() - 90.0).abs() < 1e-3, "{}°", a.to_degrees());
        // The other way round is the same turn negated.
        let back = drag_rotate(
            Axis::Z,
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 10.0),
            down(10.0, 0.0),
        );
        assert!(
            (drag_angle(Vec3::Z, Vec3::ZERO, down(0.0, 10.0), down(10.0, 0.0)).to_degrees() + 90.0)
                .abs()
                < 1e-3
        );
        assert!(
            (q * back).w.abs() > 1.0 - 1e-5,
            "the two cancel: {:?}",
            q * back
        );
    }

    #[test]
    fn a_rotate_sweep_is_ill_conditioned_on_the_pivot_and_edge_on() {
        // Cursor ON the pivot: no radius, no angle.
        assert_eq!(
            drag_angle(Vec3::Z, Vec3::ZERO, down(0.0, 0.0), down(0.0, 5.0)),
            0.0
        );
        // Ray running IN the ring's plane never meets it.
        let edge = (Vec3::new(0.0, 0.0, 0.0), Vec3::X);
        assert_eq!(drag_angle(Vec3::Z, Vec3::ZERO, edge, edge), 0.0);
        assert_eq!(
            drag_angle(Vec3::ZERO, Vec3::ZERO, down(1.0, 0.0), down(0.0, 1.0)),
            0.0
        );
    }

    #[test]
    fn a_rotate_drag_reports_degrees_turned() {
        let mut d = DragState::begin(
            GizmoMode::Rotate,
            Some(Axis::Z),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(10.0, 0.0),
            None,
        );
        let step = d.update(down(0.0, 10.0));
        let DragDelta::Rotate(q) = step else {
            panic!("rotate drag yields a rotation, got {step:?}");
        };
        assert!((q.to_axis_angle().1.to_degrees() - 90.0).abs() < 1e-3);
        assert!((d.total() - 90.0).abs() < 1e-3, "{}°", d.total());
        // Past a half turn keeps counting — the accumulator does not wrap.
        d.update(down(-10.0, 0.0));
        d.update(down(0.0, -10.0));
        assert!((d.total() - 270.0).abs() < 1e-2, "{}°", d.total());
    }

    // ── Scale ─────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn dragging_a_scale_handle_outward_doubles_only_its_axis() {
        let mut d = DragState::begin(
            GizmoMode::Scale,
            Some(Axis::X),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(5.0, 0.0),
            None,
        );
        let DragDelta::Scale(f) = d.update(down(10.0, 0.0)) else {
            panic!("scale drag yields factors");
        };
        assert!((f - Vec3::new(2.0, 1.0, 1.0)).length() < 1e-4, "{f:?}");
        assert!((d.total() - 2.0).abs() < 1e-4);
    }

    #[test]
    fn the_uniform_handle_scales_all_three_axes() {
        let mut d = DragState::begin(
            GizmoMode::Scale,
            None,
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(5.0, 0.0),
            None,
        );
        let DragDelta::Scale(f) = d.update(down(10.0, 0.0)) else {
            panic!("scale drag yields factors");
        };
        assert!((f - Vec3::splat(2.0)).length() < 1e-4, "{f:?}");
    }

    #[test]
    fn a_scale_drag_never_falls_below_the_minimum() {
        let mut d = DragState::begin(
            GizmoMode::Scale,
            Some(Axis::Y),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 10.0),
            None,
        );
        for _ in 0..8 {
            d.update(down(0.0, 1e-4)); // collapse hard onto the pivot, over and over
            assert!(
                d.total() >= MIN_SCALE,
                "pinned at the floor, got {}",
                d.total()
            );
        }
        // And the ratio never goes negative through the pivot — it pins, it does not mirror.
        assert!(
            drag_scale(
                Axis::Y,
                Mat3::IDENTITY,
                Vec3::ZERO,
                down(0.0, 5.0),
                down(0.0, -5.0)
            ) >= MIN_SCALE
        );
        assert_eq!(
            scale_factors(Some(Axis::Z), -4.0),
            Vec3::new(1.0, 1.0, MIN_SCALE)
        );
    }

    // ── Flip ──────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn flip_mirrors_the_picked_axis_and_leaves_the_others() {
        for (axis, mirrored) in [
            (Axis::X, Vec3::new(-3.0, 4.0, 5.0)),
            (Axis::Y, Vec3::new(3.0, -4.0, 5.0)),
            (Axis::Z, Vec3::new(3.0, 4.0, -5.0)),
        ] {
            let m = flip_matrix(axis, Mat3::IDENTITY, Vec3::ZERO);
            let p = m.transform_point3(Vec3::new(3.0, 4.0, 5.0));
            assert!((p - mirrored).length() < 1e-5, "{axis:?} → {p:?}");
            // A reflection is its own inverse.
            assert!((m.transform_point3(p) - Vec3::new(3.0, 4.0, 5.0)).length() < 1e-5);
        }
        // About a pivot: the plane moves with it.
        let m = flip_matrix(Axis::X, Mat3::IDENTITY, Vec3::new(2.0, 0.0, 0.0));
        assert!(
            (m.transform_point3(Vec3::new(5.0, 1.0, 0.0)) - Vec3::new(-1.0, 1.0, 0.0)).length()
                < 1e-5
        );
    }

    /// The guard seam of invariant C670523A: the CornerVector range `[-0.5, +2.5]` is asymmetric
    /// about the rest position, so a corner past +1.5 has NO legal reflection. The validator says
    /// no and the flip is refused outright — never clamped into a silently wrong shape.
    #[test]
    fn a_flip_the_validator_rejects_is_refused_and_changes_nothing() {
        const LO: f32 = -0.5;
        const HI: f32 = 2.5;
        let legal = |corners: &[Vec3], m: &Mat4| {
            corners.iter().all(|c| {
                let p = m.transform_point3(*c);
                [p.x, p.y, p.z].iter().all(|v| (LO..=HI).contains(v))
            })
        };
        let pivot = Vec3::splat(0.5); // the rest position the range is asymmetric about

        // A corner at +1.4 reflects to -0.4: inside the range, allowed.
        let ok = [Vec3::new(1.4, 0.5, 0.5)];
        let m = flip(Axis::X, Mat3::IDENTITY, pivot, |m| legal(&ok, m)).expect("representable");
        assert!((m.transform_point3(ok[0]).x - (-0.4)).abs() < 1e-5);

        // A corner at +2.5 would reflect to -1.5 — the Landmark bug. Refused.
        let bad = [Vec3::new(2.5, 0.5, 0.5)];
        let refused = flip(Axis::X, Mat3::IDENTITY, pivot, |m| legal(&bad, m));
        assert_eq!(refused, Err(FlipRefused { axis: Axis::X }));
        // Nothing was produced, so the caller's geometry is untouched by construction.
        assert_eq!(bad[0], Vec3::new(2.5, 0.5, 0.5));
        // The very same geometry may still be legal on another axis — the refusal is per-axis.
        assert!(flip(Axis::Y, Mat3::IDENTITY, pivot, |m| legal(&bad, m)).is_ok());
    }

    // ── Axis lock + snap ──────────────────────────────────────────────────────────────────────

    #[test]
    fn an_axis_lock_keeps_an_off_axis_drag_on_its_axis() {
        // The cursor moves diagonally; the X-locked drag takes only the X component.
        let mut d = DragState::begin(
            GizmoMode::Translate,
            Some(Axis::X),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 0.0),
            None,
        );
        let DragDelta::Translate(v) = d.update(down(3.0, 7.0)) else {
            panic!("translate drag yields a translation");
        };
        assert!(
            (v - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4,
            "locked to X: {v:?}"
        );
        // Unlocked, the same motion moves in the whole view plane.
        let mut free = DragState::begin(
            GizmoMode::Translate,
            None,
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 0.0),
            None,
        );
        let DragDelta::Translate(v) = free.update(down(3.0, 7.0)) else {
            panic!("translate drag yields a translation");
        };
        assert!(
            (v - Vec3::new(3.0, 7.0, 0.0)).length() < 1e-4,
            "free in the view plane: {v:?}"
        );
    }

    #[test]
    fn a_snapped_sweep_steps_at_the_halfway_point() {
        // 37° with a 15° step reads 30°; passing 37.5° it reads 45°.
        let mut d = DragState::begin(
            GizmoMode::Rotate,
            Some(Axis::Z),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(10.0, 0.0),
            Some(15.0),
        );
        let at = |deg: f32| down(10.0 * deg.to_radians().cos(), 10.0 * deg.to_radians().sin());
        // A 37° sweep has not reached the 37.5° halfway point, so it reads 30° — two whole steps
        // crossed in this frame, and the 7° remainder stays in the raw accumulator.
        let DragDelta::Rotate(q) = d.update(at(37.0)) else {
            panic!("rotate drag yields a rotation");
        };
        assert!(
            (q.to_axis_angle().1.to_degrees() - 30.0).abs() < 1e-3,
            "{q:?}"
        );
        assert!((d.total() - 30.0).abs() < 1e-3, "{}°", d.total());
        // Nudging within the same step emits nothing at all.
        assert!(d.update(at(37.4)).is_identity(), "still short of 37.5°");
        let step = d.update(at(38.0));
        assert!(!step.is_identity(), "past 37.5° the drag steps");
        assert!((d.total() - 45.0).abs() < 1e-3, "{}°", d.total());
        let DragDelta::Rotate(q) = step else {
            panic!("rotate drag yields a rotation");
        };
        assert!(
            (q.to_axis_angle().1.to_degrees() - 15.0).abs() < 1e-3,
            "one whole step, not a fraction"
        );
    }

    #[test]
    fn a_long_snapped_drag_does_not_drift_off_the_step() {
        // 90° swept in 90 one-degree frames, snapped to 15°: exactly 90°, and exactly 6 steps.
        let mut d = DragState::begin(
            GizmoMode::Rotate,
            Some(Axis::Z),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(10.0, 0.0),
            Some(15.0),
        );
        let at = |deg: f32| down(10.0 * deg.to_radians().cos(), 10.0 * deg.to_radians().sin());
        let mut steps = 0;
        for i in 1..=90 {
            if !d.update(at(i as f32)).is_identity() {
                steps += 1;
            }
        }
        assert!(
            (d.total() - 90.0).abs() < 1e-2,
            "no drift over 90 frames: {}°",
            d.total()
        );
        assert_eq!(
            steps, 6,
            "one emission per crossed step, never a fraction of one"
        );
    }

    #[test]
    fn a_snapped_translate_lands_on_multiples_of_the_step() {
        let mut d = DragState::begin(
            GizmoMode::Translate,
            Some(Axis::X),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 0.0),
            Some(0.25),
        );
        let mut total = Vec3::ZERO;
        for i in 1..=40 {
            let DragDelta::Translate(v) = d.update(down(i as f32 * 0.1, 0.0)) else {
                panic!("translate drag yields a translation");
            };
            total += v;
        }
        // 40 frames of 0.1 = 4.0 raw, quantized to the 0.25 grid = 4.0 exactly.
        assert!(
            (total - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-4,
            "{total:?}"
        );
        assert!((d.total() - 4.0).abs() < 1e-4);
        // Every emitted total sits on the grid.
        assert!(((d.total() / 0.25).round() * 0.25 - d.total()).abs() < 1e-5);
    }

    #[test]
    fn a_snapped_scale_steps_in_ratio() {
        let mut d = DragState::begin(
            GizmoMode::Scale,
            Some(Axis::X),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(4.0, 0.0),
            Some(0.5),
        );
        assert!(
            d.update(down(4.8, 0.0)).is_identity(),
            "ratio 1.2 rounds back to 1.0"
        );
        let DragDelta::Scale(f) = d.update(down(6.0, 0.0)) else {
            panic!("scale drag yields factors");
        };
        assert!((f.x - 1.5).abs() < 1e-4, "ratio 1.5 is a whole step: {f:?}");
        assert!((d.total() - 1.5).abs() < 1e-4);
    }

    #[test]
    fn a_drag_holds_its_value_through_an_unresolvable_ray() {
        // A ray running ALONG the locked axis cannot be projected: the frame contributes nothing.
        let along = (Vec3::ZERO, Vec3::X);
        let mut d = DragState::begin(
            GizmoMode::Translate,
            Some(Axis::X),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 0.0),
            None,
        );
        d.update(down(2.0, 0.0));
        assert!(d.update(along).is_identity(), "the drag holds");
        assert!(
            (d.total() - 2.0).abs() < 1e-4,
            "and keeps what it had: {}",
            d.total()
        );
    }

    #[test]
    fn a_drag_reports_the_handle_it_was_begun_on() {
        let d = DragState::begin(
            GizmoMode::Rotate,
            Some(Axis::X),
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 0.0),
            None,
        );
        assert_eq!(d.mode(), GizmoMode::Rotate);
        assert_eq!(d.axis(), Some(Axis::X));
        let free = DragState::begin(
            GizmoMode::Scale,
            None,
            Mat3::IDENTITY,
            Vec3::ZERO,
            down(0.0, 0.0),
            None,
        );
        assert_eq!(free.axis(), None, "the uniform centre handle");
    }

    // ── Mode gating ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn gadget_modes_hide_the_handles_of_a_disabled_mode() {
        let all = GadgetModes::default();
        assert_eq!(all, GadgetModes::ALL);
        for m in GizmoMode::ALL {
            assert!(all.allows(m));
            assert_eq!(all.handles_for(m), &Axis::ALL);
        }
        assert!(all.allows_flip());
        assert_eq!(all.flip_handles(), &Axis::ALL);
        // A surface whose geometry has no legal reflection drops Flip; its axes go with it, and
        // nothing else moves — the four bits are independent.
        let no_flip = GadgetModes::ALL.without(GadgetModes::FLIP);
        assert!(!no_flip.allows_flip());
        assert!(no_flip.flip_handles().is_empty());
        assert_eq!(no_flip.handles_for(GizmoMode::Scale), &Axis::ALL);
        assert_eq!(no_flip.modes().collect::<Vec<_>>(), GizmoMode::ALL.to_vec());
        // A placement-only surface: the disabled modes have no handles and leave the cycle.
        let place = GadgetModes::TRANSLATE.with(GadgetModes::ROTATE);
        assert_eq!(
            place.modes().collect::<Vec<_>>(),
            vec![GizmoMode::Translate, GizmoMode::Rotate]
        );
        assert!(place.handles_for(GizmoMode::Scale).is_empty());
        assert!(!place.allows_flip());
        assert_eq!(GadgetModes::NONE.modes().count(), 0);
        assert_eq!(
            GadgetModes::ALL.modes().collect::<Vec<_>>(),
            GizmoMode::ALL.to_vec()
        );
        // Every mode owns a distinct bit — no two modes gate each other.
        for m in GizmoMode::ALL {
            let only = GadgetModes::NONE.with(GadgetModes(1 << (m as u8)));
            assert_eq!(only.modes().collect::<Vec<_>>(), vec![m]);
            assert!(!only.allows_flip());
        }
        assert!(GadgetModes::FLIP.allows_flip());
        assert_eq!(
            GadgetModes::FLIP.modes().count(),
            0,
            "flip is not a drag mode"
        );
    }
}
