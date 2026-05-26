#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CornerVector {
    x: u8,
    y: u8,
    z: u8,
}

impl Default for CornerVector {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl CornerVector {
    pub const DEFAULT: Self = Self {
        x: 128,
        y: 128,
        z: 128,
    };

    #[must_use]
    pub fn from_components(x: f32, y: f32, z: f32) -> Self {
        Self {
            x: encode_axis(x),
            y: encode_axis(y),
            z: encode_axis(z),
        }
    }

    #[must_use]
    pub const fn from_bytes(x: u8, y: u8, z: u8) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn to_bytes(self) -> (u8, u8, u8) {
        (self.x, self.y, self.z)
    }

    #[must_use]
    pub fn to_components(self) -> (f32, f32, f32) {
        (
            decode_axis(self.x),
            decode_axis(self.y),
            decode_axis(self.z),
        )
    }
}

fn encode_axis(value: f32) -> u8 {
    let clamped = value.clamp(-0.5, 1.5);
    (((clamped + 0.5) / 2.0) * 255.0).round() as u8
}

fn decode_axis(value: u8) -> f32 {
    (f32::from(value) / 255.0) * 2.0 - 0.5
}

#[cfg(test)]
mod tests {
    use super::{decode_axis, encode_axis, CornerVector};

    #[test]
    fn round_trip_decoding_stays_within_quantization_error() {
        let mut v = -0.5_f32;
        while v <= 1.5 {
            let encoded = encode_axis(v);
            let decoded = decode_axis(encoded);
            assert!((decoded - v).abs() <= (1.0 / 255.0) + f32::EPSILON);
            v += 0.001;
        }
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let cv = CornerVector::from_components(-2.0, 5.0, 0.5);
        assert_eq!(cv.to_bytes(), (0, 255, 128));
    }

    #[test]
    fn extremes_match_spec() {
        assert_eq!(encode_axis(-0.5), 0);
        assert_eq!(encode_axis(1.5), 255);
        assert!((decode_axis(0) + 0.5).abs() <= f32::EPSILON);
        assert!((decode_axis(255) - 1.5).abs() <= f32::EPSILON);
    }

    #[test]
    fn equality_is_based_on_encoded_bytes() {
        let a = CornerVector::from_components(0.5001, 0.5001, 0.5001);
        let b = CornerVector::from_components(0.5002, 0.5002, 0.5002);
        assert_eq!(a, b);
    }

    #[test]
    fn default_points_to_center() {
        assert_eq!(CornerVector::default(), CornerVector::DEFAULT);
        assert_eq!(CornerVector::DEFAULT.to_bytes(), (128, 128, 128));
    }
}
