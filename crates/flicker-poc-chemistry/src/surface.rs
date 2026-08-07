//! **The surface cycle** — water arriving, weather happening, and rock being taken
//! from where it stands to somewhere else.
//!
//! The conveyor ([`crate::tectonics`]) decides what rock is where and how high it
//! rides. This decides what the weather does to it. Together they are the two
//! halves of a landscape: tectonics builds relief, water takes it apart, and the
//! shapes that survive are the ones the water could not carry off.
//!
//! # The chain, and why each link is a consequence
//!
//! **Water arrives** from outside ([`crate::infall`]) — delivered rather than
//! conjured, so the conservation ledger still balances. Everything downstream is
//! that water moving.
//!
//! **Where it is warm, water leaves the sea.** Surface temperature falls out of how
//! steeply the star strikes (latitude) and how much the air holds in; evaporation
//! follows the temperature and the amount of sea actually under it. Vapour drifts,
//! and rains out where the air cools or is forced up over rising ground — so a
//! range catches the rain on the side it faces and leaves a **dry shadow** behind
//! it. Nobody places a desert.
//!
//! **Rain runs downhill**, gathering as it goes, and what gathers has the power to
//! cut. How much it cuts depends on what it is cutting: the resistance of the rock
//! at the surface, read from the rock tier. **This is the whole mechanism.** A
//! landscape of one uniform material erodes evenly and, once normalised, has not
//! changed shape at all — the *contrast* is what makes shapes. Soft ground goes
//! first and a resistant intrusion is left standing, which is a ridge appearing
//! because the ground around it left, not because anything pushed it up.
//!
//! **What is carried has to land.** Sediment travels downstream while the flow can
//! hold it and settles where the flow slackens — in basins, and along coasts. It
//! settles as material on top of a column, which is to say it enters the stratum
//! lifecycle exactly like any other deposit, and burial does the rest.

use flicker_materials::{ElementId, Tables};
use flicker_worldstate::Composition;

use crate::column::{elevation_m, Column, FormationProcess};
use crate::planet::{sea_level_m, World};
use crate::stage::{Stage, StageRng};

/// Liquid water density, kg/m³.
pub const WATER_DENSITY: f64 = 1000.0;

/// Water delivered over a run by default, kg — the scale of Earth's ocean.
pub const DEFAULT_WATER_KG: f64 = 1.4e21;

/// Solar constant proxy: surface temperature, K, where the star is straight
/// overhead and the air holds nothing in. Latitude takes it down from here.
pub const INSOLATION_K: f64 = 300.0;
/// Floor on surface temperature at the poles, K — the night side of the same sum.
const POLAR_K: f64 = 230.0;
/// Greenhouse warmth cap, K — what a fully opaque air adds over the bare sum.
/// (A Venus-class CO₂ ocean saturates here; the cap is this world's scale, not
/// Venus's 500 K.)
const GREENHOUSE_K: f64 = 60.0;
/// Potency-weighted gas column, kg/m², at which the air holds in ~63% of the cap
/// (Earth's CO₂ + vapour columns land near the knee of this curve).
const GREENHOUSE_REF_KG_M2: f64 = 45.0;

/// Greenhouse potency per kilogram, relative to CO₂ = 1 — which gases hold the
/// star's warmth in. N₂/SO₂/HCl are transparent here (SO₂'s aerosol *cooling* is
/// a refinement this deliberately floors at zero). Candidates for a compounds.json
/// field once the vocabulary settles — behaviour belongs in the table.
fn greenhouse_potency(compound: flicker_worldstate::CompoundId) -> f64 {
    match compound {
        crate::atmosphere::WATER_VAPOUR => 1.6,
        crate::atmosphere::CARBON_DIOXIDE => 1.0,
        94 => 12.0, // methane (reducing branch — formed by nothing yet)
        95 => 4.0,  // ammonia (likewise)
        _ => 0.0,
    }
}

/// How much warmth the air holds in, K — a read of the **species** ledger, not the
/// bulk: a tonne of nitrogen does nothing, a tonne of CO₂ warms. Saturating in the
/// column mass, so the first potent air matters most and a runaway thickens toward
/// the cap instead of past it.
pub fn greenhouse_k(world: &World) -> f64 {
    let area = world.cell_area_m2() * world.columns.len().max(1) as f64;
    let column: f64 = world
        .reservoirs
        .atmosphere
        .species
        .iter()
        .map(|(id, kg)| kg * greenhouse_potency(id))
        .sum::<f64>()
        / area;
    GREENHOUSE_K * (1.0 - (-(column / GREENHOUSE_REF_KG_M2)).exp())
}

/// The radiative surface law: how warm the star and the air make a point on the
/// ground — steepness of strike (latitude), how hard the star shines, and what
/// the air holds in. **The one temperature read**, shared by [`Weather`] and the
/// bulk water cycle ([`crate::atmosphere::WaterCycle`]) so the rain map and the
/// reservoir exchange can never disagree about how warm the world is.
pub fn radiative_temp_k(lat_cos: f64, insol: f64, greenhouse: f64) -> f64 {
    (POLAR_K + (INSOLATION_K - POLAR_K) * lat_cos) * insol + greenhouse
}

/// **Bare ground is the interior.** A cell with no crust has no surface
/// distinct from the magma beneath it — it radiates the planet's own heat, and
/// starlight is a rounding error next to lava. Only once a lid has frozen does
/// the radiative balance decide anything, because only then is there something
/// for the star to warm and the air to hold warmth in.
///
/// This is the difference between a world that *is* hot and a world that merely
/// sits in a warm orbit, and leaving it out let a magma ocean report a
/// temperate surface on its first tick.
pub fn cell_surface_temp_k(world: &World, cell: usize, insol: f64, greenhouse: f64) -> f64 {
    if world.columns[cell].layers.is_empty() {
        world.mantle.temp_k[cell]
    } else {
        let lat_cos = (1.0 - world.grid.dirs[cell].y.abs() as f64).clamp(0.0, 1.0);
        radiative_temp_k(lat_cos, insol, greenhouse)
    }
}

/// Mean surface temperature over the globe, K — the bulk read the water cycle
/// equilibrates against and the habitability observer classifies. Area-weighted
/// over [`cell_surface_temp_k`], so a half-frozen world reads as the mean of its
/// lava and its rock rather than pretending either one is the whole story.
/// `stellar` is the boundary input: how hard the star shines, as a multiple of
/// nominal.
pub fn mean_surface_temp_k(world: &World, stellar: f64) -> f64 {
    let n = world.columns.len();
    if n == 0 {
        return 0.0;
    }
    let insol = stellar.max(0.0).powf(0.25);
    let greenhouse = greenhouse_k(world);
    let sum: f64 = (0..n).map(|i| cell_surface_temp_k(world, i, insol, greenhouse)).sum();
    sum / n as f64
}

/// Fraction of a submerged cell's water that evaporates per Myr at full warmth.
const EVAPORATION_RATE: f64 = 0.35;
/// Fraction of the vapour over a cell that drifts to its neighbours per Myr.
const ADVECTION_RATE: f64 = 0.55;
/// Fraction of vapour that rains out per Myr at full forcing.
const CONDENSATION_RATE: f64 = 0.5;
/// How strongly rising ground forces air up and wrings it out. This is what makes
/// a windward slope wet and the ground behind it dry.
const OROGRAPHIC_GAIN: f64 = 4.0;

/// Erosive power per unit of gathered flow and slope, m of rock per Myr. The
/// overall strength of the carving; the *contrast* between rocks is what shapes it.
pub const DEFAULT_EROSION_RATE: f64 = 6.0e-4;
/// How much of the gathered flow's carrying capacity is used up per step, so a
/// slackening river drops its load rather than carrying it to the sea entire.
const DEPOSITION_RATE: f64 = 0.35;

/// No single tick may take more than this fraction of the bed it is cutting.
/// Erosion is a process, not an event, and a tick that strips a whole bed is
/// how a cell beside a steep neighbour reaches bare rock. **Kept from the
/// reverted rework** — it is a pure bound, independent of the calibration that
/// went wrong.
const MAX_STRIP_FRAC: f64 = 0.25;
/// Resistance assumed for rock the catalog does not recognise. Deliberately middling
/// and deliberately not zero — unknown rock must not erode like salt.
const UNKNOWN_RESISTANCE: f32 = 0.5;

/// **Weather** — surface temperature, evaporation, drift, and rain.
///
/// Holds no state of its own between ticks: the fields it produces are read by
/// [`Erosion`] in the same tick and then forgotten, because they are weather, not
/// geology. What survives a tick is what the weather *did* to the rock.
pub struct Weather;

/// One tick of weather, per cell — the fields erosion reads.
pub struct WeatherField {
    /// Surface temperature, K.
    pub temp_k: Vec<f64>,
    /// Rain reaching the ground, in metres of water depth per Myr.
    pub rain: Vec<f64>,
    /// Where the sea stands this tick, m.
    pub sea_level: f64,
}

impl Weather {
    /// Work out this tick's weather. Read-only in the world — nothing here moves
    /// mass, it only says what the air is doing. `stellar` is the boundary input:
    /// how hard the star shines, as a multiple of nominal (the celestial host
    /// supplies it; the GM lever stands in until then).
    pub fn observe(world: &World, dt_myr: f64, stellar: f64) -> WeatherField {
        let n = world.columns.len();
        let area = world.cell_area_m2();
        let sea_level = sea_level_m(world);
        let elevation: Vec<f64> = world.columns.iter().map(|c| elevation_m(c, area)).collect();

        // What the air keeps — a read of which gases are actually in it.
        let greenhouse = greenhouse_k(world);

        // Surface temperature: how steeply the star strikes — radiative, so a
        // brighter star lifts temperature by the fourth root of its flux — plus
        // what the air keeps.
        let insol = stellar.max(0.0).powf(0.25);
        // Per cell, and the distinction matters: bare ground is molten rock at
        // the mantle's own temperature, which evaporates everything and rains
        // nothing. Weather happens on the parts that have frozen.
        let temp_k: Vec<f64> =
            (0..n).map(|i| cell_surface_temp_k(world, i, insol, greenhouse)).collect();

        // Evaporation: only off water, and only where it is warm.
        let ocean_volume = world.reservoirs.ocean.mass_kg() / WATER_DENSITY;
        let submerged: Vec<f64> = (0..n)
            .map(|i| (sea_level - elevation[i]).max(0.0))
            .collect();
        let submerged_volume: f64 = submerged.iter().sum::<f64>() * area;
        let mut vapour: Vec<f64> = (0..n)
            .map(|i| {
                if submerged[i] <= 0.0 || ocean_volume <= 0.0 {
                    return 0.0;
                }
                let warmth = ((temp_k[i] - 273.0) / 40.0).clamp(0.0, 1.0);
                submerged[i] * EVAPORATION_RATE * warmth * dt_myr
            })
            .collect();
        let _ = submerged_volume;

        // Drift: vapour spreads to its neighbours, so air off a sea reaches inland.
        for _ in 0..2 {
            let before = vapour.clone();
            for i in 0..n {
                let neighbours = &world.grid.neighbors[i];
                if neighbours.is_empty() {
                    continue;
                }
                let share = before[i] * ADVECTION_RATE / neighbours.len() as f64;
                vapour[i] -= before[i] * ADVECTION_RATE;
                for &j in neighbours {
                    vapour[j as usize] += share;
                }
            }
        }

        // Rain: air wrings out where it is cool, and much harder where the ground
        // rises under it.
        //
        // ⚠️ **KNOWN DEFECT, deliberately left standing for now** (2026-08-06).
        // `wrung_fraction` can exceed 1 and the parcel is never depleted, so a
        // cell rains more than the vapour above it and its neighbours still find
        // that vapour untouched. Aaron identified it exactly. Fixing it in
        // isolation made the whole world STATIC, because `DEFAULT_EROSION_RATE`
        // and the carry capacity were both calibrated against this inflated
        // rain — correcting the moisture budget without re-deriving those
        // constants together turned erosion off. The fix and the recalibration
        // have to land as one measured change, not as a lone correction.
        let rain: Vec<f64> = (0..n)
            .map(|i| {
                let rise = world.grid.neighbors[i]
                    .iter()
                    .map(|&j| elevation[i] - elevation[j as usize])
                    .fold(0.0f64, f64::max)
                    .max(0.0);
                (vapour[i] * wrung_fraction(temp_k[i], rise)).max(0.0)
            })
            .collect();

        WeatherField { temp_k, rain, sea_level }
    }
}

/// **Erosion** — rain gathers downhill, cuts what it can, and puts down what it
/// can no longer carry.
///
/// The cut is scaled by the **resistance of the rock at the surface**, read from
/// the rock tier through the mineral assemblage of the top bed. That read is the
/// entire reason a landscape develops shape: erode everything equally and, after
/// normalising, nothing has happened.
///
/// Every gram taken off one column lands on another or in the sea. Nothing
/// evaporates out of the ledger.
pub struct Erosion {
    tables: std::sync::Arc<Tables>,
    /// Erosive power per unit of gathered flow and slope.
    pub rate: f64,
    /// Multiplier on how hard the star shines.
    pub stellar: f64,
}

impl Erosion {
    pub fn new(tables: std::sync::Arc<Tables>, rate: f64, stellar: f64) -> Self {
        Self { tables, rate, stellar }
    }

    /// How well this column's surface stands up to weather, `0`..`1` — the top
    /// bed's [`bed_resistance`].
    fn surface_resistance(&self, col: &Column) -> f32 {
        match col.layers.last() {
            Some(top) => bed_resistance(&self.tables, top),
            None => UNKNOWN_RESISTANCE,
        }
    }
}

/// How well one bed stands up to weather, `0`..`1`. Reads its minerals against the
/// rock catalog; a bed whose minerals the catalog does not know gets the middling
/// default rather than eroding like salt.
///
/// **The one resistance read**, shared by the aggregate stage here and the
/// per-pixel tier (`flicker-worldtile`) — the two scales must answer "how hard is
/// this rock" identically or the world changes character at the migration.
pub fn bed_resistance(tables: &Tables, bed: &crate::column::Layer) -> f32 {
    if bed.minerals.is_empty() {
        return UNKNOWN_RESISTANCE;
    }
    let named = bed.minerals.iter().filter_map(|(id, mass)| {
        tables.compounds().iter().find(|c| c.id == id).map(|c| (c.name.clone(), mass))
    });
    tables.erosional_resistance(named, UNKNOWN_RESISTANCE)
}

impl Stage for Erosion {
    fn name(&self) -> &'static str {
        "Erosion"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let n = world.columns.len();
        if n == 0 {
            return;
        }
        let area = world.cell_area_m2();
        let weather = Weather::observe(world, dt_myr, self.stellar);
        let elevation: Vec<f64> = world.columns.iter().map(|c| elevation_m(c, area)).collect();

        // Where each cell drains: the neighbour that lies lowest, if any does.
        // A cell with no lower neighbour is a sink — a lake, or the sea.
        let downhill: Vec<Option<usize>> = (0..n)
            .map(|i| {
                world.grid.neighbors[i]
                    .iter()
                    .map(|&j| j as usize)
                    .filter(|&j| elevation[j] < elevation[i])
                    .min_by(|&a, &b| {
                        elevation[a].partial_cmp(&elevation[b]).unwrap_or(std::cmp::Ordering::Equal)
                    })
            })
            .collect();

        // Gather the rain downhill, headwaters first, so a trunk carries what its
        // whole catchment fed it. Visiting cells high-to-low is the topological
        // order of the drainage graph — flow only ever moves down.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            elevation[b].partial_cmp(&elevation[a]).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut flow: Vec<f64> = weather.rain.clone();
        for &cell in &order {
            if let Some(to) = downhill[cell] {
                flow[to] += flow[cell];
            }
        }

        // Cut, carry, and drop. What the water carries is a composition, handed
        // from cell to cell — so every gram that leaves a column is in the water or
        // on the ground, never in limbo. That is why this needs no scratch state on
        // the world and cannot leak.
        let mut load: Vec<Composition> = vec![Composition::new(); n];
        for &cell in &order {
            let mut carrying = std::mem::take(&mut load[cell]);

            // Ground under the sea is not being rained on and is not being cut.
            if elevation[cell] > weather.sea_level {
                if let Some(to) = downhill[cell] {
                    let slope = ((elevation[cell] - elevation[to]) / 1000.0).max(0.0);
                    let resistance =
                        self.surface_resistance(&world.columns[cell]).clamp(0.02, 1.0) as f64;
                    // Stream power: gathered flow and slope do the cutting, and the
                    // rock decides how much of it lands. The DIVISION by resistance
                    // is the whole outcrop model — soft ground goes first.
                    let cut_m = self.rate * flow[cell].sqrt() * slope * dt_myr / resistance;
                    for (e, m) in strip(&mut world.columns[cell], cut_m, area) {
                        carrying.add(e, m);
                    }
                }
            }

            match downhill[cell] {
                // A sink — a basin, or the sea. The water stops, so everything it
                // was carrying settles here.
                None => land(world, cell, carrying),
                Some(to) => {
                    // The flow slackens where the catchment below is not much
                    // bigger than this one; what it can no longer hold settles.
                    let ease = 1.0 - (flow[cell] / (flow[to] + 1e-9)).min(1.0);
                    let drop = (DEPOSITION_RATE * ease).clamp(0.0, 1.0);
                    let mut settling = Composition::new();
                    for (e, m) in carrying.iter().map(|(e, m)| (e, m * drop)).collect::<Vec<_>>() {
                        let got = carrying.remove(e, m);
                        settling.add(e, got);
                    }
                    land(world, cell, settling);
                    load[to].add_composition(&carrying);
                }
            }
        }
    }
}

/// **What share of the air over a cell wrings out**, `0..1` — cool air gives up
/// more, and ground that RISES under the air forces it up and wrings it harder.
///
/// ⚠️ **NOT bounded by 1, and that is the standing defect** — see the use site.
/// Forced hard enough (the term reaches 5.25) this claims more water than the
/// air holds, and the parcel is never debited either, so neighbours find the
/// same vapour untouched. Clamping it ALONE is not the fix: it silently
/// rescales every downstream flow, and `DEFAULT_EROSION_RATE` is calibrated
/// against the inflated numbers, so the world goes static. The correction and
/// the recalibration are one change.
fn wrung_fraction(temp_k: f64, rise_m: f64) -> f64 {
    let cool = ((300.0 - temp_k) / 60.0).clamp(0.0, 1.0);
    let lift = (rise_m / 2000.0).clamp(0.0, 1.0) * OROGRAPHIC_GAIN;
    (CONDENSATION_RATE * (0.25 + cool + lift)).max(0.0)
}

/// Take up to `depth_m` off the top of a column and hand the mass back to the
/// caller. Bounded by what is actually there, so a river cannot cut a hole through
/// the planet, and it reports what it ACTUALLY took rather than what it wanted.
fn strip(col: &mut Column, depth_m: f64, area: f64) -> Vec<(ElementId, f64)> {
    if depth_m <= 0.0 {
        return Vec::new();
    }
    let Some(top) = col.layers.last_mut() else {
        return Vec::new();
    };
    let have = top.mass_kg();
    if have <= 0.0 {
        return Vec::new();
    }
    let want = depth_m * crate::column::density_kg_m3(top) * area;
    // Same rule as the hillslope pass: a bed worn to a film goes whole, and
    // "gone" is a mass test — an emptied ledger can still be carrying keys.
    let whole = have - want < crate::column::MIN_BED_MASS_KG;
    let taken = top.release(if whole { 1.0 } else { (want / have).min(MAX_STRIP_FRAC) });
    if col.layers.last().is_some_and(|l| l.mass_kg() <= 0.0) {
        col.layers.pop();
    }
    taken
}

/// Put carried sediment down on this column. It arrives as material on top, which
/// means it enters the stratum lifecycle like any other deposit: sediment like what
/// is already there thickens that bed, and a change of source starts a new one — so
/// a canyon wall reads as a layer cake without anybody drawing bands.
fn land(world: &mut World, cell: usize, sediment: Composition) {
    let add: Vec<(ElementId, f64)> = sediment.iter().collect();
    if add.is_empty() {
        return;
    }
    let at = world.tick_myr;
    world.columns[cell].deposit(FormationProcess::Sediment, at, &add);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::scheduler::Scheduler;
    use flicker_materials::JsonTableSource;
    use flicker_worldgrid::icosphere;
    use std::sync::Arc;

    fn tables() -> Arc<Tables> {
        Arc::new(Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables"))
    }

    fn world(freq: u32, seed: u64, ticks: usize) -> (World, Arc<Tables>) {
        let t = tables();
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, seed);
        let mut s = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w.budget.clone(), &crate::Levers::brisk()), seed);
        for _ in 0..ticks {
            s.step(&mut w, 1.0, None);
        }
        (w, t)
    }

    /// **The whole mechanism.** Rock the catalog knows to be resistant scores higher
    /// than rock it knows to be soft, and an assemblage it does not recognise gets
    /// the middling default rather than eroding like salt. Without this read every
    /// surface erodes alike, and a landscape that erodes evenly has — once
    /// normalised — no shape at all.
    #[test]
    fn the_rock_tier_tells_hard_ground_from_soft() {
        let t = tables();
        let of = |mineral: &str| {
            t.erosional_resistance(std::iter::once((mineral.to_string(), 1.0)), UNKNOWN_RESISTANCE)
        };
        let quartz = of("Quartz");
        let halite = of("Halite");
        assert!(quartz > halite, "quartz {quartz} should outlast halite {halite}");
        let unknown = t.erosional_resistance(
            std::iter::once(("Nothing The Catalog Knows".to_string(), 1.0)),
            UNKNOWN_RESISTANCE,
        );
        assert_eq!(unknown, UNKNOWN_RESISTANCE, "unknown rock must not erode like salt");
    }

    /// Soft ground goes first. Two columns standing at the same height above the
    /// same drop, differing ONLY in what they are made of, do not lose the same
    /// amount — which is the difference between a landscape developing shape and
    /// merely getting lower.
    #[test]
    fn soft_ground_goes_first() {
        let t = tables();
        let area = 5.0e9;
        let quartz = t.compounds().iter().find(|c| c.name == "Quartz").expect("quartz");
        let halite = t.compounds().iter().find(|c| c.name == "Halite").expect("halite");

        let make = |mineral_id: u16| {
            let mut col = Column::empty(0);
            let mut elements = Composition::new();
            elements.add(14, 6.0e18);
            elements.add(8, 4.0e18);
            let mut minerals = flicker_worldstate::CompoundLedger::new();
            minerals.add(mineral_id, 1.0e18);
            col.layers.push(crate::column::Layer {
                elements,
                minerals,
                formed_at_myr: 0.0,
                formed_by: FormationProcess::OceanicCrust,
                peak_pt: (0.0, 0.0),
            densified: 0.0,
            });
            col
        };
        let erosion = Erosion::new(Arc::clone(&t), DEFAULT_EROSION_RATE, 1.0);
        let hard = erosion.surface_resistance(&make(quartz.id));
        let soft = erosion.surface_resistance(&make(halite.id));
        assert!(hard > soft, "resistance {hard} (quartz) vs {soft} (halite)");

        // Same cut, same everything else — the softer column sheds more rock.
        let cut = 30.0;
        let shed = |mineral_id: u16, resistance: f32| {
            let mut col = make(mineral_id);
            let taken = strip(&mut col, cut / resistance as f64, area);
            taken.iter().map(|&(_, m)| m).sum::<f64>()
        };
        assert!(
            shed(halite.id, soft) > shed(quartz.id, hard),
            "the soft column has to lose more to the same weather"
        );
    }

    /// Every gram the water lifts is carried and put down somewhere. Run against
    /// the standing conservation harness, which is what would catch rock quietly
    /// evaporating out of the ledger on its way downstream.
    #[test]
    fn what_erosion_takes_lands_somewhere() {
        let (mut w, t) = world(6, 5, 120);
        let stage = Erosion::new(Arc::clone(&t), DEFAULT_EROSION_RATE, 1.0);
        let mut rng = crate::stage::StageRng::new(2);
        for _ in 0..20 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("Erosion");
            w.audit_compound_bound("Erosion");
        }
    }

    /// Sediment is not a special case: it lands as material on top of a column and
    /// goes through the same stratum lifecycle as anything else, so a stack records
    /// what the weather did to it the same way it records what the mantle did.
    #[test]
    fn sediment_enters_the_stratum_lifecycle() {
        let (mut w, _t) = world(6, 9, 0);
        let cell = 0usize;
        let mut carried = Composition::new();
        carried.add(14, 4.0e18);
        carried.add(8, 3.0e18);
        let before = w.columns[cell].layers.len();
        land(&mut w, cell, carried);
        assert_eq!(w.columns[cell].layers.len(), before + 1, "the sediment became a bed");
        assert_eq!(
            w.columns[cell].layers.last().expect("a bed").formed_by,
            FormationProcess::Sediment,
            "and it remembers that the water put it there"
        );
    }

    /// **Orography raises the SHARE of the air that falls, never the amount of
    /// air.** The claim lives in the fraction: ground that rises wrings harder
    /// than flat ground at the same temperature, and cool air harder than warm.
    #[test]
    fn rising_ground_wrings_a_larger_share() {
        let flat = wrung_fraction(290.0, 0.0);
        let slope = wrung_fraction(290.0, 3000.0);
        assert!(slope > flat, "rising ground wrings harder: {slope} vs {flat}");
        assert!(
            wrung_fraction(250.0, 0.0) > flat,
            "and so does cold air"
        );
        // ⚠️ This term is NOT bounded by 1 today, and that is the standing
        // moisture-budget defect documented at its use site: forced hard
        // enough it claims more water than the air holds. Recorded here so the
        // number is visible rather than assumed.
        assert!(
            wrung_fraction(250.0, 50_000.0) > 1.0,
            "the known over-unity forcing is still present — remove this line with the fix"
        );
    }

}
