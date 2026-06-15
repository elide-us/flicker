//! **Epoch 4 — hydrosphere formation** (world-gen spec).
//!
//! Water condenses and fills the low ground. Both the *amount* and the *shape* of
//! the ocean are now composition-driven: the planet's **water endowment** is the
//! H₂O its element budget can form ([`water_endowment`] — hydrogen plus the oxygen
//! left free after the rock takes its share), and that volume is **poured onto the
//! Epoch-3 terrain** ([`fill_to_volume`]) until it's held, setting a global sea
//! level and each hex's water depth. So dialling Epoch 1's H or O up/down floods or
//! drains the world. Surface **temperature** follows latitude (warm equator, cold
//! poles) minus an elevation lapse. This is the stage that hands the planet to the
//! water cycle — oceans now sit on real terrain with a real hardness field to erode.
//!
//! The `hydration` knob is the only free dial left — an outgassing/delivery
//! efficiency that scales how much of the endowment reaches the surface. Feeding
//! atmosphere CO₂ back into temperature (greenhouse) is a further refinement.

use std::cmp::Ordering;

use flicker_materials::{Element, ElementId, PhysicalState, Tables};
use flicker_worldstate::Composition;

use crate::pipeline::{EpochCtx, EpochTransform, NOMINAL_DURATION};
use crate::state::{HexState, LifeStage};

/// The volcanic volatile-formers that outgas into the atmosphere: H (→ water
/// vapor), C (→ CO₂), N (→ N₂), S (→ SO₂), Cl (→ HCl). Deliberately **excludes**
/// the abundant lattice-bound oxygen and the silicate rock-formers — they are
/// locked in the crust, not free to outgas — so the air reads as a real young
/// volcanic atmosphere instead of being swamped by crustal oxygen.
const VOLATILES: [ElementId; 5] = [1, 6, 7, 16, 17];

/// Hydrogen — the water-vapor tracer loaded into the air by evaporation.
const VAPOR_ELEMENT: ElementId = 1;

/// Water chemistry: H and O combine as H₂O (2 H : 1 O).
const HYDROGEN: ElementId = 1;
const OXYGEN: ElementId = 8;
/// Molar mass of H₂O, for turning formable water moles back into a mass.
const WATER_MOLAR_MASS: f64 = 18.015;
/// Calibration: turns the planet's mean per-hex water-formation potential
/// ([`water_endowment`], a mass-action product so it carries large molar units)
/// into a basin-fill volume (elevation-units summed over hexes) at `hydration == 1`.
/// Set so the default Earth-like mix floods ~62% of the Epoch-3 terrain; the
/// `hydration` knob and the H/O mix scale it from there.
const WATER_FILL_GAIN: f32 = 0.1;

/// The organic-chemistry builders that gate prebiotic precursor formation: C, N,
/// P, S (H and O ride in the water itself). A cell rich in these, with liquid
/// water and energy, brews life-precursor compounds.
const ORGANIC: [ElementId; 4] = [6, 7, 15, 16];

/// Composition share of [`ORGANIC`] that reads as a full prebiotic signal (a few
/// percent — these elements are trace).
const ORGANIC_REF: f64 = 0.02;

/// Water depth (normalized elevation) beyond which a submerged cell is too deep /
/// dark / cold to be a prebiotic cradle — only the sunlit shallows cook.
const SHALLOW_DEPTH: f32 = 0.5;

/// Prebiotic level above which a hex reads as the [`LifeStage::Prebiotic`] stage.
const PREBIOTIC_STAGE: f32 = 0.02;

/// Epoch 4 parameters.
pub struct Epoch4 {
    /// Wetness multiplier on the **composition-derived** water endowment — the
    /// outgassing/delivery efficiency that turns the formable H₂O into surface
    /// ocean. `1` is the baseline; `0` is a dry planet; higher drowns more. The
    /// *amount* of ocean is driven by the H/O budget (see [`water_endowment`]); this
    /// knob just scales how much of it reaches the surface, so the submerged
    /// fraction is an emergent output, not a dial.
    pub hydration: f32,
    /// Surface temperature at the equator / poles (°C-ish).
    pub equator_temp: f32,
    pub pole_temp: f32,
    /// Temperature drop per unit of land elevation above sea level.
    pub lapse: f32,
    /// Axial tilt (obliquity, degrees). Its annual-mean effect is to **flatten**
    /// the equator→pole gradient (gentle at low tilt, the poles warming toward the
    /// equator as it climbs). `0` leaves the untilted latitude band unchanged.
    pub axial_tilt: f32,
    /// Overall wetness: scales how strongly warm water loads humidity into the
    /// air and rains out. `1` is the baseline; `0` makes a dry planet.
    pub vapor_scale: f32,
    /// Diffusion passes carrying ocean moisture inland (coast wet → interior dry)
    /// for the precipitation / humidity field.
    pub moisture_spread: u32,
    /// Reference total the well-mixed outgassed atmosphere normalizes to — the
    /// planet's air-inventory scale (so the mix is stable regardless of how much
    /// volatile mass the volcanic hexes happened to carry).
    pub atmosphere_mass: f64,
    /// Within-epoch **chemistry time** on the shared clock — how long prebiotic
    /// precursors brew. Normalised to [`NOMINAL_DURATION`], so the nominal value is
    /// the baseline; a longer hydrosphere era accumulates more precursors.
    pub duration: u32,
    /// How fast prebiotic precursor compounds accumulate per cycle in a perfect
    /// cradle (warm shallow organic water). `0` disables prebiotic chemistry.
    pub prebiotic_rate: f32,
}

impl Default for Epoch4 {
    fn default() -> Self {
        Self {
            hydration: 1.0,
            equator_temp: 28.0,
            pole_temp: -25.0,
            lapse: 40.0,
            axial_tilt: 0.0,
            vapor_scale: 1.0,
            moisture_spread: 3,
            atmosphere_mass: 1000.0,
            duration: NOMINAL_DURATION,
            prebiotic_rate: 0.3,
        }
    }
}

/// Fraction of the air-inventory mass that water vapor adds at full humidity —
/// humid air carries visibly more water than dry air.
const VAPOR_FRACTION: f32 = 0.5;

/// Floor on the warmth modulation of precipitation: even a cold coast keeps this
/// fraction of its proximity-driven moisture, so cold-**wet** regions (taiga) stay
/// moist rather than collapsing to dry (the Whittaker moisture axis needs to stay
/// usefully independent of temperature).
const WARMTH_FLOOR: f32 = 0.4;

impl EpochTransform for Epoch4 {
    fn epoch(&self) -> u8 {
        4
    }

    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        if prev.is_empty() {
            return Vec::new();
        }
        let n = prev.len();
        // Oceans fill from the planet's water endowment: how much H₂O the element
        // budget can form (hydrogen + free oxygen left after the rock locks its
        // share), poured onto the Epoch-3 terrain until that volume is held. A
        // hydrogen- or oxygen-poor mix floods less — the coastline traces the
        // composition, not a free dial.
        let endowment = water_endowment(prev, ctx.tables);
        let mut elevs: Vec<f32> = prev.iter().map(|s| s.elevation).collect();
        elevs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        let volume = endowment as f32 * self.hydration.max(0.0) * WATER_FILL_GAIN * n as f32;
        let sea_level = fill_to_volume(&elevs, volume);

        // Obliquity flattening: quadratic so a moderate (Earth-like) tilt barely
        // shifts the annual mean, while extreme tilt evens the band out.
        let tilt_flatten = (self.axial_tilt.max(0.0) / 90.0).clamp(0.0, 1.0).powi(2);

        let mut out: Vec<HexState> = prev
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut s = s.clone();
                s.sea_level = sea_level;
                s.water_depth = (sea_level - s.elevation).max(0.0);
                // Temperature: latitude band (flattened by axial tilt), then land
                // cools with height; ocean stays near the sea-level base.
                let poleness = ctx.dirs[i].y.abs() * (1.0 - tilt_flatten);
                let base = self.equator_temp + (self.pole_temp - self.equator_temp) * poleness;
                let land_height = (s.elevation - sea_level).max(0.0);
                s.temperature = base - self.lapse * land_height;
                s
            })
            .collect();

        // The well-mixed atmosphere: integrate volcanic outgassing of the volatile
        // formers across every volcanic hex, then normalize to the air-mass scale.
        // This is the planet's bulk air, the same everywhere; local water vapor is
        // added per hex below.
        let mut bulk = Composition::new();
        for s in prev {
            if s.volcanic <= 0.0 {
                continue;
            }
            for &el in &VOLATILES {
                let a = s.composition.amount(el);
                if a > 0.0 {
                    bulk.add(el, a * s.volcanic as f64);
                }
            }
        }
        let bulk_total = bulk.total();
        if bulk_total > 0.0 {
            let scale = self.atmosphere_mass / bulk_total;
            bulk = bulk.iter().map(|(el, a)| (el, a * scale)).collect();
        }

        // Ocean proximity diffused inland: 1 over water (submerged or at the
        // waterline), fading to dry interior. Coastline-inclusive (`elevation <=
        // sea_level`) so the shore itself seeds moisture.
        let mut prox = vec![0.0f32; n];
        for (i, s) in out.iter().enumerate() {
            if s.elevation <= sea_level {
                prox[i] = 1.0;
            }
        }
        for _ in 0..self.moisture_spread {
            let snap = prox.clone();
            for i in 0..n {
                let (mut sum, mut cnt) = (snap[i], 1.0f32);
                for &nb in &ctx.neighbors[i] {
                    sum += snap[nb as usize];
                    cnt += 1.0;
                }
                prox[i] = sum / cnt;
            }
        }

        // Per hex: warm air over water is humid → precipitation + water vapor in
        // the local atmosphere. Cold poles and dry interiors stay arid.
        let warm_ref = self.equator_temp.max(1.0);
        let chem = (self.duration as f32 / NOMINAL_DURATION as f32).max(1e-3);
        for (i, s) in out.iter_mut().enumerate() {
            let warmth = (s.temperature / warm_ref).clamp(0.0, 1.0);
            // Atmospheric humidity: warm air holds more water vapor (clouds / the
            // air's own moisture). Precipitation / effective moisture: ocean-sourced
            // (proximity), warmth-modulated but floored so cold-wet coasts stay
            // moist — the single moisture field biomes and the life thread read.
            let humidity = (warmth * prox[i] * self.vapor_scale).max(0.0);
            let warmth_mod = WARMTH_FLOOR + (1.0 - WARMTH_FLOOR) * warmth;
            s.precipitation = (prox[i] * warmth_mod * self.vapor_scale).clamp(0.0, 1.0);

            let mut atm = bulk.clone();
            let vapor = humidity * VAPOR_FRACTION * self.atmosphere_mass as f32;
            if vapor > 0.0 {
                atm.add(VAPOR_ELEMENT, vapor as f64);
            }

            // Prebiotic chemistry: liquid water × organic elements × energy
            // (warmth boosted by volcanism — incl. the hotspot island shores),
            // accumulated over the epoch's cycles. Sunlit shallows and wet land
            // ponds are cradles; the deep cold abyss and dry interiors brew none.
            let wet = if s.water_depth > 0.0 {
                (1.0 - s.water_depth / SHALLOW_DEPTH).clamp(0.0, 1.0)
            } else {
                s.precipitation
            };
            let organic = organic_share(s.surface());
            let energy = warmth * (1.0 + s.volcanic);
            s.prebiotic = (self.prebiotic_rate * chem * energy * wet * organic).clamp(0.0, 1.0);
            if s.prebiotic >= PREBIOTIC_STAGE {
                s.life_stage = LifeStage::Prebiotic;
            }

            s.atmosphere = atm;
        }

        out
    }
}

/// Oxygen (atoms) one atom of this element locks into oxides: a cation of book
/// valence `v` balances `v/2` of the O²⁻ it bonds with. Oxygen itself and the
/// gas-state volatiles (H, N, Cl, …) don't lock *crustal* oxygen, so they read 0 —
/// the hydrogen is free to make water, not rock.
fn oxide_oxygen_per_atom(e: &Element) -> f64 {
    if e.number == OXYGEN || e.state == PhysicalState::Gas {
        0.0
    } else {
        e.valence_electrons as f64 / 2.0
    }
}

/// The planet's **surface-water endowment**: the mean per-hex H₂O mass fraction the
/// element budget can form. Water is H₂O, so it takes hydrogen **and** *free*
/// oxygen — the oxygen left once the rock-forming cations have taken theirs
/// ([`oxide_oxygen_per_atom`]); a silica- or iron-rich crust locks most of it, an
/// oxygen-rich one leaves a surplus. The formable water is the limiting reagent of
/// the two, so a hydrogen-poor *or* an oxygen-poor world makes less ocean. This is
/// the lever Epoch 1's H/O sliders pull on the hydrosphere.
fn water_endowment(prev: &[HexState], tables: &Tables) -> f64 {
    if prev.is_empty() {
        return 0.0;
    }
    let mut sum_frac = 0.0;
    for s in prev {
        let total = s.composition.total();
        if total <= 0.0 {
            continue;
        }
        let (mut h_mol, mut o_mol, mut o_locked) = (0.0f64, 0.0f64, 0.0f64);
        for (el, mass) in s.composition.iter() {
            let Some(e) = tables.element_by_number(el) else { continue };
            let moles = mass / e.atomic_mass.max(1e-6) as f64;
            match el {
                HYDROGEN => h_mol += moles,
                OXYGEN => o_mol += moles,
                _ => {}
            }
            o_locked += moles * oxide_oxygen_per_atom(e);
        }
        let free_o = (o_mol - o_locked).max(0.0);
        // H₂O needs both reactants: water scales with the *availability of each*
        // (law-of-mass-action style), so a hydrogen-poor OR an oxygen-poor cell
        // makes little — and dialling either down at Epoch 1 visibly drains the
        // ocean (a pure limiting-reagent min lets the renormalized H-share mask O).
        let water_mol = (h_mol / 2.0) * free_o;
        sum_frac += water_mol * WATER_MOLAR_MASS / total;
    }
    sum_frac / prev.len() as f64
}

/// Sea level that floods the (ascending) elevations to a total `volume` — the sum
/// over hexes of `max(0, level − elevation)`. Monotone in the level, so a short
/// binary search converges; `volume <= 0` leaves the world dry.
fn fill_to_volume(elevs: &[f32], volume: f32) -> f32 {
    let (lo, hi) = (elevs[0], elevs[elevs.len() - 1]);
    if volume <= 0.0 || hi <= lo {
        return lo;
    }
    let held = |level: f32| -> f32 { elevs.iter().map(|&e| (level - e).max(0.0)).sum() };
    let (mut a, mut b) = (lo, hi);
    for _ in 0..40 {
        let mid = 0.5 * (a + b);
        if held(mid) < volume {
            a = mid;
        } else {
            b = mid;
        }
    }
    0.5 * (a + b)
}

/// Composition share of the [`ORGANIC`] builders (C, N, P, S), normalized by
/// [`ORGANIC_REF`] so a few-percent share reads as a full prebiotic signal.
fn organic_share(comp: &Composition) -> f32 {
    let total = comp.total();
    if total <= 0.0 {
        return 0.0;
    }
    let org: f64 = ORGANIC.iter().map(|&e| comp.amount(e)).sum();
    ((org / total) / ORGANIC_REF).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldstate::Composition;
    use glam::Vec3;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
    }

    /// A composition that can actually form water (hydrogen + a free-oxygen surplus
    /// over the silicate demand), for tests that need oceans on a synthetic terrain.
    fn wet_rock() -> Composition {
        Composition::from_iter([(8, 6000.0), (14, 2500.0), (1, 40.0)]) // O, Si, H
    }

    /// An `Epoch4` whose endowment × hydration fills `prev` to (about) `target_sea`,
    /// so a test can place the coastline independent of the calibration gain.
    fn epoch4_at_sea(prev: &[HexState], t: &Tables, target_sea: f32, base: Epoch4) -> Epoch4 {
        let endow = water_endowment(prev, t) as f32;
        let volume: f32 = prev.iter().map(|s| (target_sea - s.elevation).max(0.0)).sum();
        let hydration =
            if endow > 0.0 { volume / (endow * WATER_FILL_GAIN * prev.len() as f32) } else { 0.0 };
        Epoch4 { hydration, ..base }
    }

    #[test]
    fn ocean_fills_the_low_basins_first() {
        let n = 10;
        let t = tables();
        let dirs: Vec<Vec3> = (0..n)
            .map(|i| Vec3::new(0.0, i as f32 / n as f32, 1.0).normalize())
            .collect();
        let neighbors: Vec<Vec<u32>> = (0..n).map(|_| vec![]).collect();
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        // Elevations spread evenly from -1 (deep) to +1 (peak); every hex can form
        // water, so the endowment is uniform and only the terrain shapes the fill.
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(wet_rock());
                s.elevation = -1.0 + 2.0 * i as f32 / (n - 1) as f32;
                s
            })
            .collect();
        // Fill to sea level 0: the lower half floods, the upper half stays land.
        let out = epoch4_at_sea(&prev, &t, 0.0, Epoch4::default()).apply(&ctx, &prev);
        let submerged = out.iter().filter(|s| s.water_depth > 0.0).count();
        assert!((3..=7).contains(&submerged), "submerged {submerged}, expected ~half");
        assert!(out[0].water_depth > 0.0, "deepest hex should be ocean");
        assert_eq!(out[n - 1].water_depth, 0.0, "highest hex should be dry");
        // Water pools low-first: depth never increases as the ground rises.
        for w in out.windows(2) {
            assert!(w[1].water_depth <= w[0].water_depth + 1e-6, "deeper ground should hold deeper water");
        }
        assert!(out.iter().all(|s| s.temperature.is_finite()));
    }

    #[test]
    fn equator_is_warmer_than_the_poles() {
        let t = tables();
        let dirs = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)]; // equator, pole
        let neighbors = [vec![], vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let flat = HexState::new(Composition::new()); // elevation 0 both
        let out = Epoch4::default().apply(&ctx, &[flat.clone(), flat]);
        assert!(out[0].temperature > out[1].temperature, "equator should beat pole");
    }

    /// A ring of `n` hexes around the equator — a minimal connected world.
    fn ring(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let dirs = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors = (0..n)
            .map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32])
            .collect();
        (dirs, neighbors)
    }

    /// Run Epochs 1→2→3 so Epoch 4 has real volcanic activity + composition to
    /// outgas from.
    fn through_epoch3<'a>(
        t: &'a Tables,
        dirs: &'a [Vec3],
        neighbors: &'a [Vec<u32>],
    ) -> (EpochCtx<'a>, Vec<HexState>) {
        use crate::epoch1::{Epoch1, Epoch1Params};
        use crate::epoch2::Epoch2;
        use crate::epoch3::Epoch3;
        let e1 = Epoch1::new(t, Epoch1Params::default(), 7);
        let ctx = EpochCtx { tables: t, dirs, neighbors, seed: 7 };
        let seed: Vec<HexState> = dirs.iter().map(|&d| HexState::new(e1.seed_hex(d))).collect();
        let e2 = Epoch2::default().apply(&ctx, &seed);
        let e3 = Epoch3::default().apply(&ctx, &e2);
        (ctx, e3)
    }

    #[test]
    fn outgassing_builds_a_volatile_atmosphere_not_bound_oxygen() {
        let t = tables();
        let (dirs, neighbors) = ring(30);
        let (ctx, e3) = through_epoch3(&t, &dirs, &neighbors);
        let out = Epoch4::default().apply(&ctx, &e3);

        let atm_total: f64 = out.iter().map(|s| s.atmosphere.total()).sum();
        assert!(atm_total > 0.0, "no atmosphere outgassed");
        for s in &out {
            // Lattice oxygen and the silicate rock-formers stay in the crust.
            assert_eq!(s.atmosphere.amount(8), 0.0, "free oxygen leaked into the air");
            assert_eq!(s.atmosphere.amount(14), 0.0, "silicon leaked into the air");
            assert!(s.precipitation.is_finite() && (0.0..=1.0).contains(&s.precipitation));
            assert!(s.atmosphere.total().is_finite());
        }
        // Water vapor (H) is present — the seed of the water cycle.
        assert!(out.iter().any(|s| s.atmosphere.amount(1) > 0.0), "no water vapor in the air");
        // Precipitation forms somewhere warm and wet.
        assert!(out.iter().any(|s| s.precipitation > 0.0), "no precipitation anywhere");
    }

    #[test]
    fn precipitation_is_wetter_in_the_warm_wet_tropics() {
        let t = tables();
        // Equatorial ocean (warm), polar ocean (cold), and a high land hex to lift
        // the sea level above both ocean floors so both are submerged.
        let dirs = [Vec3::X, Vec3::Y, Vec3::Z];
        let neighbors = [vec![], vec![], vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let mk = |elev: f32| {
            let mut s = HexState::new(wet_rock());
            s.elevation = elev;
            s
        };
        let prev = [mk(-0.9), mk(-0.9), mk(0.5)];
        let out = epoch4_at_sea(&prev, &t, 0.0, Epoch4::default()).apply(&ctx, &prev);
        assert!(
            out[0].water_depth > 0.0 && out[1].water_depth > 0.0,
            "both oceans should be submerged"
        );
        // Warmth modulates rainfall: the warm equatorial ocean out-rains the cold
        // polar ocean by a clear margin (both are moisture-available, being sea).
        assert!(
            out[0].precipitation > out[1].precipitation + 0.2,
            "warm equatorial ocean should clearly out-rain the cold pole ({} vs {})",
            out[0].precipitation,
            out[1].precipitation
        );
    }

    /// Silica rock with a free-oxygen surplus + hydrogen (so the seas can form) and
    /// organic-element traces (C, N, P, S) — the prebiotic cradle test bed.
    fn organic_wet_rock() -> Composition {
        Composition::from_iter([
            (8, 6000.0),  // O — enough to leave a free surplus over the silica
            (14, 3000.0), // Si
            (1, 40.0),    // H
            (6, 150.0),   // C
            (7, 20.0),    // N
            (15, 10.0),   // P
            (16, 30.0),   // S
        ])
    }

    #[test]
    fn prebiotic_chemistry_favors_warm_shallow_organic_water() {
        let t = tables();
        // All warm-equator hexes; fill to sea level 0 so the -0.2/-0.3 hexes are
        // sunlit coastal cradles and the -1.0 hex is the deep cold abyss.
        let dirs = [Vec3::X, Vec3::X, Vec3::X, Vec3::X, Vec3::X];
        let neighbors: [Vec<u32>; 5] = [vec![], vec![], vec![], vec![], vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let mk = |elev: f32| {
            let mut s = HexState::new(organic_wet_rock());
            s.elevation = elev;
            s
        };
        let prev = [mk(-1.0), mk(-0.3), mk(-0.2), mk(0.4), mk(0.5)];
        let out = epoch4_at_sea(&prev, &t, 0.0, Epoch4::default()).apply(&ctx, &prev);

        assert!(out.iter().all(|s| s.prebiotic.is_finite() && (0.0..=1.0).contains(&s.prebiotic)));
        assert!(
            out[1].prebiotic > out[0].prebiotic,
            "warm shallow organic water should out-cook the deep abyss"
        );
        assert!(out[0].prebiotic < 1e-6, "the deep cold abyss should brew ~no precursors");
    }

    #[test]
    fn more_chem_time_brews_more_precursors() {
        let t = tables();
        let dirs = [Vec3::X, Vec3::X, Vec3::X, Vec3::X, Vec3::X];
        let neighbors: [Vec<u32>; 5] = [vec![], vec![], vec![], vec![], vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let mk = |elev: f32| {
            let mut s = HexState::new(organic_wet_rock());
            s.elevation = elev;
            s
        };
        let prev = [mk(-1.0), mk(-0.3), mk(-0.2), mk(0.4), mk(0.5)];
        let young = epoch4_at_sea(&prev, &t, 0.0, Epoch4 { duration: 1, ..Epoch4::default() }).apply(&ctx, &prev);
        let old = epoch4_at_sea(&prev, &t, 0.0, Epoch4 { duration: 10, ..Epoch4::default() }).apply(&ctx, &prev);
        assert!(
            old[1].prebiotic > young[1].prebiotic,
            "more chemistry time should brew more precursors in the cradle"
        );
    }

    // --- The hydrosphere is composition-driven: H/O endowment → ocean volume. ---

    /// Ocean coverage on a ring world after E1→E2→E3→E4, with an Epoch-1 tweak
    /// applied (e.g. dial an element's abundance). The submerged fraction is now an
    /// *output* of the H/O budget, not a dial.
    fn ocean_coverage(t: &Tables, tweak: impl Fn(&mut crate::epoch1::Epoch1Params)) -> f32 {
        use crate::epoch1::{Epoch1, Epoch1Params};
        use crate::epoch2::Epoch2;
        use crate::epoch3::Epoch3;
        let (dirs, neighbors) = ring(160);
        let mut p = Epoch1Params::default();
        tweak(&mut p);
        let e1 = Epoch1::new(t, p, 7);
        let ctx = EpochCtx { tables: t, dirs: &dirs, neighbors: &neighbors, seed: 7 };
        let seed: Vec<HexState> = dirs.iter().map(|&d| HexState::new(e1.seed_hex(d))).collect();
        let l2 = Epoch2::default().apply(&ctx, &seed);
        let l3 = Epoch3::default().apply(&ctx, &l2);
        let l4 = Epoch4::default().apply(&ctx, &l3);
        l4.iter().filter(|s| s.water_depth > 0.0).count() as f32 / l4.len() as f32
    }

    #[test]
    fn more_hydrogen_makes_more_ocean() {
        let t = tables();
        let dry = ocean_coverage(&t, |p| {
            p.abundance.insert("H".into(), 0.02);
        });
        let wet = ocean_coverage(&t, |p| {
            p.abundance.insert("H".into(), 0.6);
        });
        assert!(wet > dry, "more hydrogen should flood more of the world ({wet} vs {dry})");
    }

    #[test]
    fn less_oxygen_dries_the_world() {
        let t = tables();
        // The default mix sits near the rock/oxygen balance, so the free oxygen left
        // for water is thin: starving oxygen (rock soaks it all) drowns less.
        let earthlike = ocean_coverage(&t, |_| {});
        let oxygen_poor = ocean_coverage(&t, |p| {
            p.abundance.insert("O".into(), 20.0);
        });
        assert!(earthlike > 0.0, "the default mix should make some ocean");
        assert!(
            oxygen_poor < earthlike,
            "an oxygen-poor world should be drier ({oxygen_poor} vs {earthlike})"
        );
    }

    #[test]
    fn free_oxygen_gates_the_water_budget() {
        let t = tables();
        let st = |c: Composition| HexState::new(c);
        // Same hydrogen; surplus oxygen vs. oxygen all locked up by silicon.
        let surplus = [st(Composition::from_iter([(1, 100.0), (8, 5000.0), (14, 100.0)]))];
        let locked = [st(Composition::from_iter([(1, 100.0), (8, 1000.0), (14, 9000.0)]))];
        assert!(
            water_endowment(&surplus, &t) > water_endowment(&locked, &t),
            "free oxygen should make more water than oxygen bound in silica"
        );
        assert_eq!(water_endowment(&locked, &t), 0.0, "silica should soak every oxygen → no water");
    }

    #[test]
    fn fill_to_volume_holds_the_requested_water() {
        let elevs = [-1.0, -0.5, 0.0, 0.5, 1.0];
        assert_eq!(fill_to_volume(&elevs, 0.0), -1.0, "no water → dry world");
        let lvl = fill_to_volume(&elevs, 1.5);
        let held: f32 = elevs.iter().map(|&e| (lvl - e).max(0.0)).sum();
        assert!((held - 1.5).abs() < 1e-2, "fill should hold the requested volume (held {held})");
    }
}
