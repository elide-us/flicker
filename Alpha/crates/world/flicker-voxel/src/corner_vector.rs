//! Byte-encoded per-axis corner offset.
//!
//! Each axis of a [`CornerVector`] is stored as a single byte mapping the
//! range `[-0.5, 2.5]` to `[0, 255]`. Encoding and decoding are lossy round
//! trips (max axis error ≈ `1.5/255` ≈ `0.0059`, half the `3/255` step), so
//! equality is defined on the stored byte representation rather than on
//! decoded floats — two `CornerVector`s are equal iff their three bytes
//! match.
//!
//! # Why `[-0.5, 2.5]` (ratified 2026-08-27)
//!
//! The wider-than-`[0, 1]` range lets a voxel's owned corner extend into
//! neighbor space, which is what enables overlapping geometry and inverted
//! voxels in later phases. The reach is deliberately **asymmetric about the
//! rest position `0.5`**: for an owned vertex to land on the `-0.5` extreme
//! position *of the adjacent cell's frame*, the owner has to stretch to
//! `+2.5` from its own min corner — at the old `+1.5` cap it only reached
//! the neighbor's rest corner, never its extreme, so a fully-welded or
//! fully-inverted configuration was unrepresentable.
//!
//! The span `3.0` also makes one cell-unit exactly `85` byte steps
//! (`255 / 3`), so the *same world point* re-encoded relative to the
//! adjacent cell lands on the byte grid exactly (`byte ± 85`). The old span
//! `2.0` put one cell-unit at `127.5` steps — off the grid by half a step,
//! which made byte-exact cross-frame agreement impossible.
//!
//! Mirror hazard: because the range is asymmetric, a vertex past `+1.5` has
//! no legal reflection — editor transforms must constrain or refuse, never
//! clamp-and-continue (see the paired mirror invariant in MCP memory).

/// Byte value that encodes the axis default `0.5` (the rest position —
/// the voxel's own cell center).
///
/// `((0.5 + 0.5) / 3.0 * 255.0).round() == 85`.
const DEFAULT_AXIS_BYTE: u8 = 85;

/// Inclusive lower bound of the per-axis floating-point range.
const AXIS_MIN: f32 = -0.5;

/// Inclusive upper bound of the per-axis floating-point range.
const AXIS_MAX: f32 = 2.5;

/// A per-voxel corner offset stored as three bytes — one per axis.
///
/// Constructible from `f32` components (with clamping to `[-0.5, 2.5]` and
/// safe handling of non-finite inputs) or from raw bytes. Equality and
/// hashing operate on the byte representation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CornerVector {
    bytes: [u8; 3],
}

impl CornerVector {
    /// The default corner vector `(0.5, 0.5, 0.5)`, encoded as `[85, 85, 85]`.
    /// Points at the voxel's own center.
    pub const DEFAULT: Self = Self {
        bytes: [DEFAULT_AXIS_BYTE; 3],
    };

    /// Construct from three axis components. Out-of-range inputs are
    /// clamped to `[-0.5, 2.5]`; non-finite inputs (NaN, ±Inf) fall back
    /// to the default axis value (`0.5`).
    #[inline]
    #[must_use]
    pub fn from_components(x: f32, y: f32, z: f32) -> Self {
        Self {
            bytes: [encode_axis(x), encode_axis(y), encode_axis(z)],
        }
    }

    /// Construct from three raw byte values. Any byte value is valid; no
    /// validation is performed.
    #[inline]
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 3]) -> Self {
        Self { bytes }
    }

    /// The raw byte representation.
    #[inline]
    #[must_use]
    pub const fn bytes(&self) -> [u8; 3] {
        self.bytes
    }

    /// Decode the three axis components as `f32`s in `[-0.5, 2.5]`.
    #[inline]
    #[must_use]
    pub fn to_components(&self) -> [f32; 3] {
        [
            decode_axis(self.bytes[0]),
            decode_axis(self.bytes[1]),
            decode_axis(self.bytes[2]),
        ]
    }
}

impl Default for CornerVector {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Encode one axis value to its byte representation.
///
/// Out-of-range inputs are clamped; non-finite inputs fall back to the
/// default-axis byte rather than silently producing `0` via the `f32 as u8`
/// saturating cast.
fn encode_axis(value: f32) -> u8 {
    if !value.is_finite() {
        return DEFAULT_AXIS_BYTE;
    }
    let clamped = value.clamp(AXIS_MIN, AXIS_MAX);
    ((clamped + 0.5) / 3.0 * 255.0).round() as u8
}

/// Decode one axis byte back to its `f32` value in `[-0.5, 2.5]`.
#[inline]
fn decode_axis(byte: u8) -> f32 {
    (byte as f32 / 255.0) * 3.0 - 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_centered() {
        let v = CornerVector::default();
        assert_eq!(v.bytes(), [85, 85, 85]);
        let [x, y, z] = v.to_components();
        // Default encoding 85 decodes to 0.5 within a small epsilon
        // (85/255 is exactly one third up the 3.0 span).
        assert!((x - 0.5).abs() < 0.005);
        assert!((y - 0.5).abs() < 0.005);
        assert!((z - 0.5).abs() < 0.005);
    }

    #[test]
    fn extremes_encode_to_byte_endpoints() {
        assert_eq!(
            CornerVector::from_components(-0.5, -0.5, -0.5).bytes(),
            [0, 0, 0]
        );
        assert_eq!(
            CornerVector::from_components(2.5, 2.5, 2.5).bytes(),
            [255, 255, 255]
        );
    }

    #[test]
    fn extremes_decode_to_endpoints() {
        let lo = CornerVector::from_bytes([0, 0, 0]).to_components();
        let hi = CornerVector::from_bytes([255, 255, 255]).to_components();
        assert_eq!(lo, [-0.5, -0.5, -0.5]);
        assert_eq!(hi, [2.5, 2.5, 2.5]);
    }

    #[test]
    fn out_of_range_inputs_clamp() {
        assert_eq!(
            CornerVector::from_components(-100.0, -1.0, -0.500001).bytes()[0],
            0
        );
        assert_eq!(
            CornerVector::from_components(-100.0, -1.0, -0.500001).bytes()[1],
            0
        );
        assert_eq!(
            CornerVector::from_components(-100.0, -1.0, -0.500001).bytes()[2],
            0
        );

        assert_eq!(
            CornerVector::from_components(3.0, 100.0, 2.500001).bytes()[0],
            255
        );
        assert_eq!(
            CornerVector::from_components(3.0, 100.0, 2.500001).bytes()[1],
            255
        );
        assert_eq!(
            CornerVector::from_components(3.0, 100.0, 2.500001).bytes()[2],
            255
        );
    }

    #[test]
    fn non_finite_inputs_fall_back_to_default() {
        let nan = CornerVector::from_components(f32::NAN, f32::NAN, f32::NAN);
        let pinf = CornerVector::from_components(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let ninf =
            CornerVector::from_components(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        // NaN: defensive default (not the silent `0` from `NaN as u8`).
        assert_eq!(nan.bytes(), [DEFAULT_AXIS_BYTE; 3]);
        // ±Inf: also defensive default, since clamp(±Inf) returns the bound
        // but is_finite is the actual gate we use.
        assert_eq!(pinf.bytes(), [DEFAULT_AXIS_BYTE; 3]);
        assert_eq!(ninf.bytes(), [DEFAULT_AXIS_BYTE; 3]);
    }

    #[test]
    fn round_trip_precision_across_full_range() {
        // Sweep every byte; decode → encode must reproduce the byte exactly.
        for byte in 0u8..=255u8 {
            let cv = CornerVector::from_bytes([byte, byte, byte]);
            let [x, _, _] = cv.to_components();
            let re_encoded = CornerVector::from_components(x, x, x);
            assert_eq!(
                re_encoded.bytes(),
                [byte; 3],
                "round-trip failed for byte {byte}"
            );
        }
    }

    #[test]
    fn round_trip_precision_within_one_step() {
        // Sweep f32 values across the legal range and confirm decode is
        // within half the 3/255 step of the original. This is the
        // documented precision bound and the practical accuracy a caller
        // can rely on.
        let step = 0.01_f32;
        let mut v = -0.5_f32;
        while v <= 2.5 {
            let cv = CornerVector::from_components(v, v, v);
            let [decoded, _, _] = cv.to_components();
            assert!(
                (decoded - v).abs() <= 1.5 / 255.0 + 1e-6,
                "round-trip error too large for {v}: got {decoded}"
            );
            v += step;
        }
    }

    #[test]
    fn one_cell_unit_is_exactly_85_byte_steps() {
        // The span-3.0 alignment property the range was ratified for: the
        // same world point re-encoded relative to the adjacent cell (one
        // cell-unit away) lands exactly 85 byte steps over, so cross-frame
        // agreement is representable byte-exactly everywhere both frames
        // can express the point.
        for byte in 0u8..=170u8 {
            let v = decode_axis(byte);
            assert_eq!(
                encode_axis(v + 1.0),
                byte + 85,
                "byte {byte} + one cell-unit missed the grid"
            );
        }
    }

    #[test]
    fn equality_is_on_bytes_not_decoded_floats() {
        // Two inputs that differ in f32 but encode to the same byte must
        // compare equal. Pick values both safely inside the byte-100 range:
        // 0.675 → 99.9→100, 0.678 → 100.1→100.
        let a = CornerVector::from_components(0.675, 0.675, 0.675);
        let b = CornerVector::from_components(0.678, 0.678, 0.678);
        assert_eq!(a.bytes(), [100, 100, 100]);
        assert_eq!(b.bytes(), [100, 100, 100]);
        assert_eq!(a, b);
    }

    #[test]
    fn bytes_constructor_round_trips() {
        for &b in &[0u8, 1, 42, 85, 127, 128, 200, 254, 255] {
            let cv = CornerVector::from_bytes([b, b, b]);
            assert_eq!(cv.bytes(), [b, b, b]);
        }
    }
}
