//! Packed material identity.
//!
//! A [`Material`] is a 32-bit value with three subfields:
//!
//! | Bits     | Field          | Range            |
//! | -------- | -------------- | ---------------- |
//! | `0..12`  | `primary`      | `0..4096`        |
//! | `12..24` | `secondary`    | `0..4096`        |
//! | `24..32` | `blend`        | `0..256` (`u8`)  |
//!
//! 12-bit primary and secondary indices give 4096 entries each — generous
//! for a texture-backed material set. 8-bit blend covers gradient
//! transitions between the two at the resolution of a single byte.
//!
//! Index components are surfaced as `u16` because 16 bits is the smallest
//! type that holds a 12-bit value; bounds checks still apply at the call
//! site (values `> 4095` return `None`).
//!
//! The all-zero raw value is conventionally [`Material::EMPTY`].

/// Bit width of the `primary` and `secondary` subfields.
const INDEX_BITS: u32 = 12;
/// Bit mask covering `INDEX_BITS` bits, used for raw-value masking.
const INDEX_MASK: u32 = (1u32 << INDEX_BITS) - 1;
/// Bit offset of the `secondary` subfield.
const SECONDARY_SHIFT: u32 = INDEX_BITS;
/// Bit offset of the `blend` subfield.
const BLEND_SHIFT: u32 = INDEX_BITS * 2;
/// Maximum value (inclusive) that fits in an index subfield. Typed as
/// `u16` because that's the input type to [`Material::new`].
const INDEX_MAX: u16 = 0xFFF;

/// Packed material identity. See module docs for the bit layout.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Material {
    raw: u32,
}

impl Material {
    /// The "empty" / "no material" sentinel. Raw value `0`.
    pub const EMPTY: Self = Self { raw: 0 };

    /// Construct from component values. Returns `None` if either the
    /// `primary` or `secondary` index exceeds 12 bits (`> 4095`); `blend`
    /// is a `u8` so it cannot overflow.
    #[inline]
    #[must_use]
    pub const fn new(primary: u16, secondary: u16, blend: u8) -> Option<Self> {
        if primary > INDEX_MAX || secondary > INDEX_MAX {
            return None;
        }
        Some(Self {
            raw: (primary as u32)
                | ((secondary as u32) << SECONDARY_SHIFT)
                | ((blend as u32) << BLEND_SHIFT),
        })
    }

    /// Construct from a raw packed `u32`. Any value is accepted — use this
    /// when you've stored a material elsewhere and need to round-trip it.
    #[inline]
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    /// The raw packed `u32` representation.
    #[inline]
    #[must_use]
    pub const fn raw(&self) -> u32 {
        self.raw
    }

    /// The primary material index (`0..4096`).
    #[inline]
    #[must_use]
    pub const fn primary(&self) -> u16 {
        (self.raw & INDEX_MASK) as u16
    }

    /// The secondary material index (`0..4096`).
    #[inline]
    #[must_use]
    pub const fn secondary(&self) -> u16 {
        ((self.raw >> SECONDARY_SHIFT) & INDEX_MASK) as u16
    }

    /// The blend factor between primary and secondary (`0..256`).
    #[inline]
    #[must_use]
    pub const fn blend(&self) -> u8 {
        ((self.raw >> BLEND_SHIFT) & 0xFF) as u8
    }
}

impl Default for Material {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_has_raw_zero() {
        assert_eq!(Material::EMPTY.raw(), 0);
        assert_eq!(Material::EMPTY.primary(), 0);
        assert_eq!(Material::EMPTY.secondary(), 0);
        assert_eq!(Material::EMPTY.blend(), 0);
    }

    #[test]
    fn default_equals_empty() {
        assert_eq!(Material::default(), Material::EMPTY);
    }

    #[test]
    fn component_round_trip() {
        let cases: [(u16, u16, u8); 10] = [
            (0, 0, 0),
            (1, 0, 0),
            (0, 1, 0),
            (0, 0, 1),
            (1, 2, 3),
            (4095, 0, 0),
            (0, 4095, 0),
            (0, 0, 255),
            (4095, 4095, 255),
            (1234, 2345, 67),
        ];
        for (p, s, b) in cases {
            let m = Material::new(p, s, b).expect("in-range");
            assert_eq!(m.primary(), p, "primary round-trip");
            assert_eq!(m.secondary(), s, "secondary round-trip");
            assert_eq!(m.blend(), b, "blend round-trip");
        }
    }

    #[test]
    fn bit_layout_matches_spec() {
        // primary in low 12 bits
        assert_eq!(Material::new(1, 0, 0).unwrap().raw(), 0x0000_0001);
        assert_eq!(Material::new(0xFFF, 0, 0).unwrap().raw(), 0x0000_0FFF);

        // secondary in next 12 bits
        assert_eq!(Material::new(0, 1, 0).unwrap().raw(), 0x0000_1000);
        assert_eq!(Material::new(0, 0xFFF, 0).unwrap().raw(), 0x00FF_F000);

        // blend in top 8 bits
        assert_eq!(Material::new(0, 0, 1).unwrap().raw(), 0x0100_0000);
        assert_eq!(Material::new(0, 0, 0xFF).unwrap().raw(), 0xFF00_0000);

        // all fields packed
        assert_eq!(
            Material::new(0xFFF, 0xFFF, 0xFF).unwrap().raw(),
            0xFFFF_FFFF
        );
    }

    #[test]
    fn overflow_returns_none() {
        assert!(Material::new(4096, 0, 0).is_none());
        assert!(Material::new(0, 4096, 0).is_none());
        // u16::MAX (= 65535) is the largest input still > 4095.
        assert!(Material::new(u16::MAX, 0, 0).is_none());
        assert!(Material::new(0, u16::MAX, 0).is_none());
        // blend is u8, can't overflow at the call site.
    }

    #[test]
    fn boundary_4095_is_valid() {
        // 12-bit max is inclusive.
        assert!(Material::new(4095, 4095, 255).is_some());
    }

    #[test]
    fn raw_constructor_accepts_anything() {
        // Even values that wouldn't be reachable via `new` are accepted
        // verbatim (e.g. arbitrary u32 from an external store).
        const RAW: u32 = 0xDEAD_BEEF;
        let m = Material::from_raw(RAW);
        assert_eq!(m.raw(), RAW);
        // Subfield accessors still return the masked bits, narrowed to u16.
        assert_eq!(m.primary(), (RAW & INDEX_MASK) as u16);
        assert_eq!(m.secondary(), ((RAW >> 12) & INDEX_MASK) as u16);
        assert_eq!(m.blend(), ((RAW >> 24) & 0xFF) as u8);
    }

    #[test]
    fn equality_and_hash() {
        use std::collections::HashSet;
        let a = Material::new(1, 2, 3).unwrap();
        let b = Material::new(1, 2, 3).unwrap();
        let c = Material::new(1, 2, 4).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }
}
