//! The ejecta-cast **model**: how far the supernova flings each element.
//!
//! The driver is intentionally simple (the design's call): a single characteristic
//! *cast distance* per element, a decreasing function of **atomic weight** — heavy
//! elements fall short, light ones reach far. Mass / abundance is not modelled here;
//! only the distance each element settles at.
//!
//! Physical intuition behind the default: in an explosion that partitions kinetic
//! energy roughly evenly across particles, ejection speed goes as `v ∝ m^(-1/2)`
//! (energy equipartition), so distance does too. That is the `falloff = 0.5`
//! default; the knob lets you push toward equal-momentum (`falloff → 1`, heavies
//! pulled hard inward) or a flatter spread (`falloff → 0`) to find a "semi-natural"
//! distribution. The explosion-size dial scales the whole reach.
//!
//! Element identity (symbol, atomic mass, colour) comes from the **Prism** periodic
//! table via [`flicker_materials::Tables`] — the same 28-element vocabulary the rest
//! of the project stays within.

use flicker_materials::{JsonTableSource, Tables};

/// The repo material data directory (the canonical Prism source), relative to this
/// example's manifest — the same files world-gen and the other POCs load.
const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");

/// Load the Prism vocabulary (periodic table + materials) from `Alpha/content/data`.
pub fn load_tables() -> Tables {
    Tables::from_source(&JsonTableSource::new(MATERIALS_DIR)).expect("repo Alpha/content/data loads")
}

/// Reference mass that anchors the **outer** edge of the cloud: the lightest element
/// (hydrogen) is cast to the full reach, everything heavier falls short of it.
const M_REF: f32 = 1.008;
/// Cast reach (AU) of the lightest element at explosion `0.0` (a small, sparse event).
const REACH_MIN_AU: f32 = 8.0;
/// Cast reach (AU) of the lightest element at explosion `1.0` (a large, energetic event).
const REACH_MAX_AU: f32 = 80.0;

/// The two tunable dials of the cast model.
#[derive(Clone, Copy, Debug)]
pub struct CastParams {
    /// Size of the initial explosion, `0..1`. Scales how far *everything* is cast
    /// (it sets the lightest element's reach between [`REACH_MIN_AU`] and
    /// [`REACH_MAX_AU`]); the per-element ratios are unchanged.
    pub explosion: f32,
    /// Atomic-weight falloff exponent: `distance ∝ (M_REF / m)^falloff`. `0.5` is
    /// energy equipartition (`v ∝ 1/√m`); higher pulls heavy elements harder toward
    /// the star, lower flattens the spread.
    pub falloff: f32,
}

impl Default for CastParams {
    fn default() -> Self {
        Self {
            // Tuned to the Sol regime: a disk that forms ~2 gas giants + ice giants +
            // a few terrestrials (the rest of the parameters — star mass, snow line, MMSN
            // — are already Sun-calibrated). Exact body counts vary by seed.
            explosion: 0.55,
            falloff: 0.5,
        }
    }
}

impl CastParams {
    /// Reach (AU) of the lightest element for the current explosion size.
    pub fn reach_au(&self) -> f32 {
        REACH_MIN_AU + self.explosion.clamp(0.0, 1.0) * (REACH_MAX_AU - REACH_MIN_AU)
    }

    /// Characteristic cast distance (AU) for an element of `atomic_mass`.
    pub fn distance_au(&self, atomic_mass: f32) -> f32 {
        let m = atomic_mass.max(M_REF);
        self.reach_au() * (M_REF / m).powf(self.falloff)
    }
}

/// One element's identity + display colour. Its cast distance is *not* stored — it is
/// a pure function of the live [`CastParams`], recomputed each frame.
#[derive(Clone, Debug)]
pub struct ElementCast {
    pub symbol: String,
    pub name: String,
    pub number: u8,
    pub atomic_mass: f32,
    pub color: [f32; 3],
}

/// The cloud's element set, sorted by atomic mass **ascending** (so index `0` is the
/// lightest = outermost ring; the legend reads outer → inner top → bottom).
#[derive(Clone, Debug)]
pub struct Ejecta {
    pub elements: Vec<ElementCast>,
}

impl Ejecta {
    /// Build the element set from the loaded Prism table.
    pub fn from_tables(tables: &Tables) -> Self {
        let mut elements: Vec<ElementCast> = tables
            .elements()
            .iter()
            .map(|e| ElementCast {
                symbol: e.symbol.clone(),
                name: e.name.clone(),
                number: e.number,
                atomic_mass: e.atomic_mass,
                color: element_rgb(e.number),
            })
            .collect();
        elements.sort_by(|a, b| a.atomic_mass.total_cmp(&b.atomic_mass));
        Self { elements }
    }
}

/// Per-element tint (atomic number → RGB): grounded mineral / oxide / flame-test
/// associations (olivine-green Mg, cobalt/titanium blues, sulfur/sodium yellows,
/// iron rust, gold). Lifted from the solar-system POC so the two viewers read the
/// same element colours. Any unmapped number falls back to neutral rock.
fn element_rgb(el: u8) -> [f32; 3] {
    match el {
        1 => [0.42, 0.52, 0.62],  // H   pale ice-blue
        2 => [0.66, 0.74, 0.86],  // He  pale sky
        6 => [0.12, 0.12, 0.13],  // C   graphite near-black
        7 => [0.46, 0.52, 0.56],  // N   pale grey-blue
        8 => [0.34, 0.42, 0.52],  // O   blue-grey (bound oxygen)
        11 => [0.74, 0.56, 0.30], // Na  sodium yellow
        12 => [0.44, 0.60, 0.46], // Mg  olivine green
        13 => [0.56, 0.58, 0.62], // Al  pale aluminium grey
        14 => [0.64, 0.56, 0.42], // Si  quartz sand
        15 => [0.58, 0.34, 0.26], // P   phosphor brick
        16 => [0.80, 0.70, 0.22], // S   sulfur yellow
        17 => [0.56, 0.68, 0.44], // Cl  pale green
        19 => [0.56, 0.40, 0.62], // K   potassium lilac
        20 => [0.74, 0.70, 0.60], // Ca  pale limestone
        22 => [0.56, 0.64, 0.74], // Ti  titanium blue-steel
        24 => [0.30, 0.56, 0.38], // Cr  chrome green
        26 => [0.56, 0.22, 0.12], // Fe  rust-red
        27 => [0.24, 0.36, 0.70], // Co  cobalt blue
        28 => [0.50, 0.58, 0.50], // Ni  nickel green-grey
        29 => [0.25, 0.66, 0.58], // Cu  verdigris teal-green
        30 => [0.62, 0.66, 0.70], // Zn  pale blue-white
        47 => [0.80, 0.82, 0.84], // Ag  bright silver
        50 => [0.62, 0.62, 0.66], // Sn  pewter grey
        78 => [0.72, 0.74, 0.76], // Pt  pale platinum
        79 => [0.86, 0.68, 0.20], // Au  gold
        82 => [0.42, 0.44, 0.50], // Pb  galena blue-grey
        92 => [0.56, 0.72, 0.24], // U   uranyl yellow-green
        _ => [0.55, 0.52, 0.50],  // unmapped → neutral
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_all_28_elements() {
        let ej = Ejecta::from_tables(&load_tables());
        // 28 after Li was synced into periodic_table.json (2026-07-11, Book III
        // ruling F3450870) — see flicker-materials.
        assert_eq!(ej.elements.len(), 28);
        // Sorted ascending by mass: hydrogen first, uranium last.
        assert_eq!(ej.elements.first().unwrap().symbol, "H");
        assert_eq!(ej.elements.last().unwrap().symbol, "U");
    }

    #[test]
    fn heavier_elements_cast_shorter() {
        let p = CastParams::default();
        // Hydrogen reaches the full explosion reach; iron and uranium fall short.
        let h = p.distance_au(1.008);
        let fe = p.distance_au(55.85);
        let u = p.distance_au(238.0);
        assert!((h - p.reach_au()).abs() < 1e-3);
        assert!(fe < h, "iron casts shorter than hydrogen");
        assert!(u < fe, "uranium casts shorter than iron");
        assert!(u > 0.0);
    }

    #[test]
    fn explosion_size_scales_every_distance() {
        let small = CastParams { explosion: 0.0, ..Default::default() };
        let big = CastParams { explosion: 1.0, ..Default::default() };
        // Same falloff → the ratio between two elements is invariant; only the scale grows.
        let r_small = small.distance_au(55.85) / small.distance_au(1.008);
        let r_big = big.distance_au(55.85) / big.distance_au(1.008);
        assert!((r_small - r_big).abs() < 1e-4);
        assert!(big.distance_au(55.85) > small.distance_au(55.85));
    }
}
