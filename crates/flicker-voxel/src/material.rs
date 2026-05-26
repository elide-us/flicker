#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Material {
    raw: u32,
}

impl Material {
    pub const EMPTY: Self = Self { raw: 0 };

    const INDEX_MASK: u32 = 0x0FFF;

    #[must_use]
    pub const fn new(primary: u16, secondary: u16, blend: u8) -> Option<Self> {
        if primary > Self::INDEX_MASK as u16 || secondary > Self::INDEX_MASK as u16 {
            return None;
        }

        Some(Self {
            raw: (primary as u32) | ((secondary as u32) << 12) | ((blend as u32) << 24),
        })
    }

    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.raw
    }

    #[must_use]
    pub const fn primary(self) -> u16 {
        (self.raw & Self::INDEX_MASK) as u16
    }

    #[must_use]
    pub const fn secondary(self) -> u16 {
        ((self.raw >> 12) & Self::INDEX_MASK) as u16
    }

    #[must_use]
    pub const fn blend(self) -> u8 {
        (self.raw >> 24) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::Material;

    #[test]
    fn round_trip_components() {
        let material = Material::new(100, 2500, 200).expect("material should fit");
        assert_eq!(material.primary(), 100);
        assert_eq!(material.secondary(), 2500);
        assert_eq!(material.blend(), 200);
    }

    #[test]
    fn bit_layout_matches_spec() {
        let material = Material::new(0x123, 0xABC, 0xEF).expect("material should fit");
        assert_eq!(material.raw(), 0xEFABC123);
    }

    #[test]
    fn index_overflow_returns_none() {
        assert!(Material::new(4096, 0, 0).is_none());
        assert!(Material::new(0, 4096, 0).is_none());
    }

    #[test]
    fn raw_constructor_accepts_any_u32() {
        let raw = 0xDEADBEEF;
        let material = Material::from_raw(raw);
        assert_eq!(material.raw(), raw);
    }

    #[test]
    fn empty_material_raw_is_zero() {
        assert_eq!(Material::EMPTY.raw(), 0);
        assert_eq!(Material::default(), Material::EMPTY);
    }
}
