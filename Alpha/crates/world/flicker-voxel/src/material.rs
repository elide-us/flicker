//! Packed material identity.
//!
//! A [`Material`] is a 32-bit value with three subfields:
//!
//! | Bits     | Field          | Range            |
//! | -------- | -------------- | ---------------- |
//! | `0..8`   | `primary`      | `0..256` (`u8`)  |
//! | `8..16`  | `secondary`    | `0..256` (`u8`)  |
//! | `16..24` | `blend`        | `0..256` (`u8`)  |
//! | `24..32` | reserved       | `0` (bit 31 = direct-RGB escape) |
//!
//! The primary and secondary indices are **`MaterialId`s into the 256-slot
//! material catalog** (`Alpha/content/data/materials.json`) — the u8 wire
//! value is the ratified ceiling (one id space, id 0 = Air/EMPTY), narrowed
//! from a 12-bit demo layout on 2026-08-19. 8-bit blend covers gradient
//! transitions between the two; only `blendable`-class materials carry a
//! nonzero blend (hard-edge classes force it to `0` at mesh build).
//!
//! The top byte is reserved and zero for every catalog material. Bit 31 is
//! the **direct-RGB escape flag** of the mesh shader contract
//! (`flicker-render` `mesh.wgsl`): when set, bits `0..24` are an RGB888
//! colour instead of packed indices — for continuous data maps the catalog
//! can't express. This constructor never sets it; escape words are packed by
//! the visualisation producers against the shader contract.
//!
//! The all-zero raw value is conventionally [`Material::EMPTY`].

/// Bit width of each subfield.
const FIELD_BITS: u32 = 8;
/// Bit offset of the `secondary` subfield.
const SECONDARY_SHIFT: u32 = FIELD_BITS;
/// Bit offset of the `blend` subfield.
const BLEND_SHIFT: u32 = FIELD_BITS * 2;

/// Packed material identity. See module docs for the bit layout.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Material {
    raw: u32,
}

impl Material {
    /// The "empty" / "no material" sentinel. Raw value `0` (= catalog id 0,
    /// Air).
    pub const EMPTY: Self = Self { raw: 0 };

    /// Construct from component values. Infallible: every subfield is a `u8`,
    /// so no input can overflow its bits.
    #[inline]
    #[must_use]
    pub const fn new(primary: u8, secondary: u8, blend: u8) -> Self {
        Self {
            raw: (primary as u32)
                | ((secondary as u32) << SECONDARY_SHIFT)
                | ((blend as u32) << BLEND_SHIFT),
        }
    }

    /// Construct from a raw packed `u32`. Any value is accepted — use this
    /// when you've stored a material elsewhere and need to round-trip it
    /// (including direct-RGB escape words, which set bit 31).
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

    /// The primary material id (`0..256`) — a catalog `MaterialId`.
    #[inline]
    #[must_use]
    pub const fn primary(&self) -> u8 {
        (self.raw & 0xFF) as u8
    }

    /// The secondary material id (`0..256`) — a catalog `MaterialId`.
    #[inline]
    #[must_use]
    pub const fn secondary(&self) -> u8 {
        ((self.raw >> SECONDARY_SHIFT) & 0xFF) as u8
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
        let cases: [(u8, u8, u8); 8] = [
            (0, 0, 0),
            (1, 0, 0),
            (0, 1, 0),
            (0, 0, 1),
            (1, 2, 3),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 255),
        ];
        for (p, s, b) in cases {
            let m = Material::new(p, s, b);
            assert_eq!(m.primary(), p, "primary round-trip");
            assert_eq!(m.secondary(), s, "secondary round-trip");
            assert_eq!(m.blend(), b, "blend round-trip");
        }
    }

    #[test]
    fn bit_layout_matches_spec() {
        // primary in low 8 bits
        assert_eq!(Material::new(1, 0, 0).raw(), 0x0000_0001);
        assert_eq!(Material::new(0xFF, 0, 0).raw(), 0x0000_00FF);

        // secondary in next 8 bits
        assert_eq!(Material::new(0, 1, 0).raw(), 0x0000_0100);
        assert_eq!(Material::new(0, 0xFF, 0).raw(), 0x0000_FF00);

        // blend in bits 16..24
        assert_eq!(Material::new(0, 0, 1).raw(), 0x0001_0000);
        assert_eq!(Material::new(0, 0, 0xFF).raw(), 0x00FF_0000);

        // all fields packed; the top byte stays clear for catalog materials
        assert_eq!(Material::new(0xFF, 0xFF, 0xFF).raw(), 0x00FF_FFFF);
    }

    #[test]
    fn raw_constructor_accepts_anything() {
        // Even values `new` never produces are accepted verbatim — e.g. a
        // direct-RGB escape word (bit 31 set) from a visualisation producer.
        const RAW: u32 = 0xDEAD_BEEF;
        let m = Material::from_raw(RAW);
        assert_eq!(m.raw(), RAW);
        // Subfield accessors still return the masked bits.
        assert_eq!(m.primary(), (RAW & 0xFF) as u8);
        assert_eq!(m.secondary(), ((RAW >> 8) & 0xFF) as u8);
        assert_eq!(m.blend(), ((RAW >> 16) & 0xFF) as u8);
    }

    #[test]
    fn equality_and_hash() {
        use std::collections::HashSet;
        let a = Material::new(1, 2, 3);
        let b = Material::new(1, 2, 3);
        let c = Material::new(1, 2, 4);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }
}
