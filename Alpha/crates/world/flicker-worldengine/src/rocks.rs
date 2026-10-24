//! **THE ROCKS OF THE LEDGER.** The planet names what a hex is made of from
//! the engine's rock catalog (`content/data/rocks.json`, read through
//! [`flicker_materials::Tables`]) and reads how it resists erosion THERE —
//! `RockDef::erosional_resistance`, the same number the chemistry crate's
//! `bed_resistance` weighs a bed's minerals to. Aaron 2026-10-19: *"there
//! should already be a literal record of materials and their hardness in
//! the game engine that you're using in the ledger for what composes a
//! particular hex."* Until that day the V5 planet carried a scale of its
//! own (`HARD_SEDIMENT` 0.3 … `HARD_ROOT` 0.9) beside the catalog; that
//! scale is gone. One read at the hex and at the pixel.
//!
//! The ledger's channel (`Composed::hardness`, the epochs' parcels, the
//! era's strata) still carries the NUMBER — the rock's resistance, blended
//! where the processes blend — and names the rock back by [`Rock::nearest`].
//! Carrying the rock itself (a mix per tile, as a chemistry bed carries its
//! minerals) is the next step of the migration spec.

use flicker_materials::{JsonTableSource, Tables};
use std::sync::OnceLock;

/// The rocks the planet's processes name, each an id in the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rock {
    /// THE SOFT BAND — what the era's water lays down and strips out.
    Shale,
    /// Coarse sediment, consolidated.
    Sandstone,
    /// The basement: the continental swells and coarse layers.
    Granite,
    /// The floor: young crust born at a ridge, the ocean layers' texture,
    /// a vent's pour.
    Basalt,
    /// The arc: bubbles.
    Andesite,
    /// The ranges along the plate boundaries.
    Gneiss,
    /// The orogenic root: crust thickened to the ceiling.
    Quartzite,
    /// A strike's bowl and rim: shattered rock, the softest thing short of
    /// loose sediment.
    ImpactBreccia,
}

impl Rock {
    pub const ALL: [Rock; 8] = [
        Rock::Shale,
        Rock::Sandstone,
        Rock::Granite,
        Rock::Basalt,
        Rock::Andesite,
        Rock::Gneiss,
        Rock::Quartzite,
        Rock::ImpactBreccia,
    ];

    /// The catalog id.
    pub const fn id(self) -> &'static str {
        match self {
            Rock::Shale => "shale",
            Rock::Sandstone => "sandstone",
            Rock::Granite => "granite",
            Rock::Basalt => "basalt",
            Rock::Andesite => "andesite",
            Rock::Gneiss => "gneiss",
            Rock::Quartzite => "quartzite",
            Rock::ImpactBreccia => "impact_breccia",
        }
    }

    /// How the rock resists erosion, `0..1` — the catalog's number.
    pub fn resistance(self) -> f32 {
        catalog()[self as usize]
    }

    /// The rock whose resistance lies nearest `resistance` — what the ledger's
    /// blended number reads back as.
    pub fn nearest(resistance: f32) -> Rock {
        let c = catalog();
        Rock::ALL
            .iter()
            .copied()
            .min_by(|a, b| {
                (c[*a as usize] - resistance)
                    .abs()
                    .total_cmp(&(c[*b as usize] - resistance).abs())
            })
            .unwrap_or(Rock::Granite)
    }
}

/// The catalog's resistances in [`Rock::ALL`] order, read once from the
/// repo's `Alpha/content/data` (the engine's `from_repo` seam). A missing
/// catalog or an unnamed rock is LOUD: the ledger has no number to invent.
fn catalog() -> &'static [f32; 8] {
    static CATALOG: OnceLock<[f32; 8]> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        let tables = Tables::from_source(&JsonTableSource::new(dir))
            .expect("the rock catalog (Alpha/content/data/rocks.json) loads");
        let mut out = [0.0f32; 8];
        for (k, rock) in Rock::ALL.iter().enumerate() {
            out[k] = tables
                .rock(rock.id())
                .unwrap_or_else(|| panic!("rocks.json names no rock `{}`", rock.id()))
                .erosional_resistance;
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every rock the planet names is in the catalog, and the catalog
    /// orders them as the ground does**: the soft band under sandstone
    /// under the floor under the basement under the root; breccia stands
    /// with the soft; the nearest read returns each rock from its own
    /// number.
    #[test]
    fn the_planet_names_rocks_the_catalog_knows() {
        for r in Rock::ALL {
            let v = r.resistance();
            assert!((0.0..=1.0).contains(&v), "{r:?} resists {v}");
            assert_eq!(Rock::nearest(v), r);
        }
        assert!(Rock::Shale.resistance() < Rock::Sandstone.resistance());
        assert!(Rock::Sandstone.resistance() < Rock::Basalt.resistance());
        assert!(Rock::Basalt.resistance() <= Rock::Granite.resistance());
        assert!(Rock::Granite.resistance() <= Rock::Gneiss.resistance());
        assert!(Rock::Gneiss.resistance() < Rock::Quartzite.resistance());
        assert!(Rock::ImpactBreccia.resistance() < Rock::Sandstone.resistance());
    }
}
