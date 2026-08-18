//! stage — geometry for a `stage` sub-scene's content layers.
//!
//! A `stage` UI node composites an offscreen sub-scene (a character portrait, a
//! turntable) into its rect; the `stages.<source>.layers` list in
//! `ui_theme.json` names what that sub-scene draws. The layer kinds that are
//! plain line geometry live here as PURE FUNCTIONS — no GPU, no renderer state —
//! so a scene builds them once and can unit-test the result without a device.
//!
//! Feed the output straight to [`Renderer::draw_lines`](crate::Renderer::draw_lines)
//! (depth-tested, under the character) or `draw_lines_overlay` (always visible).
//!
//! Salvaged from the retired `flicker-packeditor`, which drew the same gold ground
//! ring under every character on its field.

use glam::Vec3;

/// A usable authored dimension: finite and greater than zero. Authored JSON reaches
/// these helpers unvalidated, so every entry point guards on it.
fn positive(v: f32) -> bool {
    v.is_finite() && v > 0.0
}

/// Line-loop segments approximating a circle on the horizontal plane through
/// `center` — the ground ring under a staged character. `segments` sides; 24 is
/// what the pack editor used and it reads smooth down to portrait sizes.
///
/// A degenerate ring (fewer than 3 sides, or a non-finite / non-positive radius)
/// returns empty rather than panicking, so authored JSON can be passed straight
/// through without validation.
pub fn ring_segments(center: Vec3, radius: f32, segments: usize) -> Vec<(Vec3, Vec3)> {
    use std::f32::consts::TAU;
    if segments < 3 || !positive(radius) {
        return Vec::new();
    }
    let at = |k: usize| {
        let a = k as f32 / segments as f32 * TAU;
        Vec3::new(
            center.x + radius * a.cos(),
            center.y,
            center.z + radius * a.sin(),
        )
    };
    (0..segments).map(|i| (at(i), at(i + 1))).collect()
}

/// A square ground grid centred on the origin at height `y`: lines every `spacing`
/// metres out to `extent` metres each way. The `grid` stage layer — the faint floor
/// under a turntable shot.
///
/// Returns empty on non-finite or non-positive inputs, for the same reason as
/// [`ring_segments`].
pub fn grid_segments(spacing: f32, extent: f32, y: f32) -> Vec<(Vec3, Vec3)> {
    if !(positive(spacing) && positive(extent)) {
        return Vec::new();
    }
    let n = (extent / spacing).floor() as i32;
    let mut segs = Vec::with_capacity((2 * n as usize + 1) * 2);
    for i in -n..=n {
        let t = i as f32 * spacing;
        segs.push((Vec3::new(t, y, -extent), Vec3::new(t, y, extent)));
        segs.push((Vec3::new(-extent, y, t), Vec3::new(extent, y, t)));
    }
    segs
}

/// A square ground grid centred on the origin in the **XY plane** at height `z`: lines
/// every `spacing` units out to `extent` each way.
///
/// The Z-up sibling of [`grid_segments`]. Content is authored Z-up (ground = XY, +Z up)
/// and the asset-pipeline editor draws the RAW source at that reckoning, so its stage
/// floor lies in XY; the Y-up [`grid_segments`] is for runtime-space scenes, which see
/// content only after the loader's Z-up→Y-up conversion. Same guards as its sibling.
pub fn grid_segments_xy(spacing: f32, extent: f32, z: f32) -> Vec<(Vec3, Vec3)> {
    if !(positive(spacing) && positive(extent)) {
        return Vec::new();
    }
    let n = (extent / spacing).floor() as i32;
    let mut segs = Vec::with_capacity((2 * n as usize + 1) * 2);
    for i in -n..=n {
        let t = i as f32 * spacing;
        segs.push((Vec3::new(t, -extent, z), Vec3::new(t, extent, z)));
        segs.push((Vec3::new(-extent, t, z), Vec3::new(extent, t, z)));
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Z-up grid is the same lattice in the other plane: flat in Z, spanning X and Y.
    /// A scene that mixed the two conventions would draw its floor as a wall, so the plane
    /// each one lies in is the thing worth pinning.
    #[test]
    fn xy_grid_is_flat_in_z_and_spans_both_ground_axes() {
        let segs = grid_segments_xy(0.5, 3.0, -1.25);
        assert_eq!(segs.len(), 26, "13 lines each ground axis, interleaved");
        assert!(
            segs.iter().all(|(a, b)| a.z == -1.25 && b.z == -1.25),
            "every vertex sits on the given ground height"
        );
        for axis in [0usize, 1] {
            let c = |v: &Vec3| if axis == 0 { v.x } else { v.y };
            let min = segs.iter().map(|(a, _)| c(a)).fold(f32::MAX, f32::min);
            let max = segs.iter().map(|(a, _)| c(a)).fold(f32::MIN, f32::max);
            assert!(
                (min + 3.0).abs() < 1e-5 && (max - 3.0).abs() < 1e-5,
                "axis {axis} spans the extent"
            );
        }
        // The Y-up sibling is untouched by this: it is still flat in Y.
        assert!(grid_segments(0.5, 3.0, 0.0).iter().all(|(a, _)| a.y == 0.0));
        // Same degenerate guards.
        assert!(grid_segments_xy(0.0, 3.0, 0.0).is_empty());
        assert!(grid_segments_xy(0.5, f32::NAN, 0.0).is_empty());
    }

    #[test]
    fn ring_closes_on_itself_and_sits_at_one_height() {
        let segs = ring_segments(Vec3::new(1.0, 0.5, -2.0), 0.45, 24);
        assert_eq!(segs.len(), 24);
        // Every vertex is on the ring's plane and at the ring's radius.
        for (a, b) in &segs {
            for v in [a, b] {
                assert!((v.y - 0.5).abs() < 1e-5, "ring left its plane");
                let r = ((v.x - 1.0).powi(2) + (v.z + 2.0).powi(2)).sqrt();
                assert!((r - 0.45).abs() < 1e-4, "vertex off the radius: {r}");
            }
        }
        // The loop is continuous: each segment starts where the last ended, and the
        // last closes back onto the first.
        for w in segs.windows(2) {
            assert!((w[0].1 - w[1].0).length() < 1e-5, "ring has a gap");
        }
        assert!(
            (segs[23].1 - segs[0].0).length() < 1e-4,
            "ring never closed"
        );
    }

    #[test]
    fn degenerate_ring_and_grid_are_empty_not_panics() {
        assert!(ring_segments(Vec3::ZERO, 0.45, 2).is_empty());
        assert!(ring_segments(Vec3::ZERO, 0.0, 24).is_empty());
        assert!(ring_segments(Vec3::ZERO, f32::NAN, 24).is_empty());
        assert!(grid_segments(0.0, 6.0, 0.0).is_empty());
        assert!(grid_segments(0.5, -1.0, 0.0).is_empty());
        assert!(grid_segments(f32::INFINITY, 6.0, 0.0).is_empty());
    }

    #[test]
    fn grid_spans_the_extent_both_ways() {
        let segs = grid_segments(0.5, 3.0, 0.0);
        // 13 lines each axis (-3.0 .. 3.0 step 0.5), interleaved.
        assert_eq!(segs.len(), 26);
        assert!(segs.iter().all(|(a, b)| a.y == 0.0 && b.y == 0.0));
        let min_x = segs.iter().map(|(a, _)| a.x).fold(f32::MAX, f32::min);
        let max_x = segs.iter().map(|(a, _)| a.x).fold(f32::MIN, f32::max);
        assert!((min_x + 3.0).abs() < 1e-5 && (max_x - 3.0).abs() < 1e-5);
    }
}
