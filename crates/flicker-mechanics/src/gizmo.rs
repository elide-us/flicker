//! Renderer-agnostic TRS transform-gizmo geometry + picking (the Blender/Maya-style manipulator).
//!
//! Pure glam — every function returns `(Vec3, Vec3, [f32;4])` coloured line segments (or takes/returns
//! rays as `(origin, dir)` tuples), so any editor can draw it through its own line pipeline and reuse
//! the picking math. NO renderer / assetpipeline / source-model types enter here: the gizmo operates on
//! a plain `origin` + `basis` (world axes as `Mat3` columns), so it works on ANY conformed rig.
//!
//! Slice 1 implements TRANSLATE fully (arrow handles, shaft picking, axis-constrained drag). Rotate
//! (rings) and Scale (axis boxes) geometry + picking are present as clean seams for slice 2.

use glam::{Mat3, Vec3};

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

/// Which manipulation the gizmo performs. Geometry + picking branch on this.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GizmoMode {
    /// Move along an axis (slice 1).
    #[default]
    Translate,
    /// Turn about an axis (slice 2).
    Rotate,
    /// Scale along an axis (slice 2).
    Scale,
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
        [
            (r + 1.0) * 0.5,
            (g + 1.0) * 0.5,
            (b + 1.0) * 0.5,
            a,
        ]
    }
}

/// The world-space unit direction of `axis` under `basis` (basis columns = the gizmo's world axes).
/// Falls back to the raw axis if `basis` collapses the column to zero.
fn world_axis(basis: Mat3, axis: Axis) -> Vec3 {
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
/// Scale: per axis a shaft + a small end box.
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
fn push_arrow(segs: &mut Vec<(Vec3, Vec3, [f32; 4])>, origin: Vec3, dir: Vec3, size: f32, color: [f32; 4]) {
    let tip = origin + dir * size;
    segs.push((origin, tip, color));
    let (u, _) = perp_basis(dir);
    let back = tip - dir * (size * HEAD_LEN_FRAC);
    let w = u * (size * HEAD_WIDTH_FRAC);
    segs.push((tip, back + w, color));
    segs.push((tip, back - w, color));
}

/// A segmented ring of `radius` about `origin` in the plane perpendicular to `dir`.
fn push_ring(segs: &mut Vec<(Vec3, Vec3, [f32; 4])>, origin: Vec3, dir: Vec3, radius: f32, color: [f32; 4]) {
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
fn push_scale_handle(segs: &mut Vec<(Vec3, Vec3, [f32; 4])>, origin: Vec3, dir: Vec3, size: f32, color: [f32; 4]) {
    let tip = origin + dir * size;
    segs.push((origin, tip, color));
    let (u, v) = perp_basis(dir);
    let h = size * BOX_HALF_FRAC;
    let corner = |sx: f32, sy: f32, sz: f32| tip + (dir * sx + u * sy + v * sz) * h;
    let c = [
        corner(-1.0, -1.0, -1.0), corner(1.0, -1.0, -1.0),
        corner(1.0, 1.0, -1.0), corner(-1.0, 1.0, -1.0),
        corner(-1.0, -1.0, 1.0), corner(1.0, -1.0, 1.0),
        corner(1.0, 1.0, 1.0), corner(-1.0, 1.0, 1.0),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1), (1, 2), (2, 3), (3, 0),
        (4, 5), (5, 6), (6, 7), (7, 4),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ];
    for &(i, j) in &EDGES {
        segs.push((c[i], c[j], color));
    }
}

/// Nearest axis handle the ray `(ray_origin, ray_dir)` hits within `max_dist` (draw-space units),
/// else `None`. Rays are `(origin, dir)` to match `Camera::pick_ray`. Translate/Scale test the shaft
/// (a finite segment); Rotate tests the ring. Returns the closest-miss axis under the threshold.
pub fn pick_handle(
    ray_origin: Vec3,
    ray_dir: Vec3,
    origin: Vec3,
    basis: Mat3,
    mode: GizmoMode,
    size: f32,
    max_dist: f32,
) -> Option<Axis> {
    let mut best: Option<(Axis, f32)> = None;
    for axis in Axis::ALL {
        let dir = world_axis(basis, axis);
        let miss = match mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                let tip = origin + dir * size;
                let (pr, ps) = closest_point_ray_segment(ray_origin, ray_dir, origin, tip);
                (pr - ps).length()
            }
            GizmoMode::Rotate => ring_miss(ray_origin, ray_dir, origin, dir, size * RING_RADIUS_FRAC),
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
    let hit = |(o, d): (Vec3, Vec3)| -> Option<Vec3> {
        let denom = d.dot(n);
        if denom.abs() < 1e-6 {
            return None; // ray parallel to the plane
        }
        Some(o + d * ((origin - o).dot(n) / denom))
    };
    match (hit(ray_prev), hit(ray_now)) {
        (Some(a), Some(b)) => b - a,
        _ => Vec3::ZERO,
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
        let segs = gizmo_segments(Vec3::ZERO, Mat3::IDENTITY, GizmoMode::Translate, 10.0, Some(Axis::Y));
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
        assert!((delta - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4, "delta {delta:?}");
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
        assert!((delta - Vec3::new(0.0, 4.0, 0.0)).length() < 1e-4, "delta {delta:?}");
    }

    #[test]
    fn drag_plane_moves_in_the_view_plane_and_locks_the_view_axis() {
        // View looking along -Z (a Top view): the plane is XY through the origin.
        let n = Vec3::Z;
        let prev = (Vec3::new(1.0, 2.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let now = (Vec3::new(4.0, 6.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let d = drag_plane(n, Vec3::ZERO, prev, now);
        assert!((d - Vec3::new(3.0, 4.0, 0.0)).length() < 1e-4, "in-plane XY move, Z locked: {d:?}");
        // A ray parallel to the plane cannot resolve a point → no move.
        let par = (Vec3::ZERO, Vec3::X);
        assert_eq!(drag_plane(n, Vec3::ZERO, par, par), Vec3::ZERO);
    }
}
