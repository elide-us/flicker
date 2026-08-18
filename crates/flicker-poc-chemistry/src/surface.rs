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

/// Water delivered over a run by default, kg, **at reference (freq-96) scale**
/// — the scale of Earth's ocean. Like every mass budget it rides `size_scale³`
/// ([`Levers::sized`](crate::Levers::sized)): the same fraction of a smaller
/// planet is less water, and the sea it makes is proportionally shallower.
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
    let sum: f64 = (0..n)
        .map(|i| cell_surface_temp_k(world, i, insol, greenhouse))
        .sum();
    sum / n as f64
}

/// **Evaporation off a water surface, metres per Myr at full warmth** — a FLUX,
/// not a fraction of the reservoir. Earth's ocean gives up ~1.2 m/yr, and this
/// is that number in the sim's own currency.
///
/// It used to be `0.35`, read as *"the fraction of a submerged cell's water that
/// evaporates per Myr"* — depth times a rate. That form **structurally capped
/// ocean turnover at 0.35 per Myr**, because you cannot evaporate more than all
/// of a reservoir; Earth turns its ocean over ~270 times per Myr. No value of a
/// fraction can fix that, and anything above 1.0 is nonsense.
///
/// Evaporation is a **surface** process: a 4 km ocean and a 4 m pond give up the
/// same depth per square metre. Depth belongs in the ledger as a limit on what
/// is available, never as a multiplier on the rate — and a shallow sea therefore
/// turns over FASTER, which is the physically right answer this world was
/// getting backwards.
///
/// Measured consequence of the old form: rain 17 m/Myr, rivers ~3,000× under-fed,
/// and denudation at 21 mm/Myr against a real 10–100 m/Myr (E782666A).
const EVAPORATION_FLUX_M_PER_MY: f64 = 1.2e6;
/// Fraction of the vapour over a cell that drifts to its neighbours per Myr.
const ADVECTION_RATE: f64 = 0.55;
/// Fraction of vapour that rains out per Myr at full forcing.
const CONDENSATION_RATE: f64 = 0.5;
/// How strongly rising ground forces air up and wrings it out — what makes a
/// windward slope wet and the ground behind it dry.
///
/// **1.0, against a base of [`BASE_WRING`].** At 4.0 a mountain flank wrung its
/// air completely dry (the term saturated) while flat warm ground gave up an
/// eighth of its vapour — an 8× concentration that put essentially all the
/// world's rain on the slopes beside mountains and left the interiors arid
/// (Aaron, 2026-08-06: *"rain storms aren't just concentrated on the 50m along
/// the side of mountains, they happen everywhere"*). Real windward
/// enhancement is more like 2–5× lowland rainfall, which is what this gives.
const OROGRAPHIC_GAIN: f64 = 1.0;

/// What air gives up wherever it is, before latitude or terrain say anything —
/// **it rains everywhere**. Orography and cool air modulate this; they no
/// longer dwarf it.
const BASE_WRING: f64 = 0.5;

/// Erosive power, m of rock per Myr at unit √flow, unit slope, unit resistance.
/// The overall strength of the carving; the *contrast* between rocks is what
/// shapes it. **Recalibrated 2026-08-06** with the moisture fix and the move to
/// physical slope (rise over the actual cell spacing — the old currency divided
/// the drop by a bare 1000 m): the two changes rescale the same term, so the
/// rate was re-derived from the measured forcing (`erosion_forcing_report`)
/// against the sim's own uplift pace — the conveyor builds relief at roughly a
/// tenth of a metre per tick on active ground, so the MEDIAN land cut must sit
/// well under that or the world levels to a grey mean (the attractor Aaron
/// kept seeing), while the steep-and-wet tail is what carves. Measured at the
/// 2.4-BY mid-bake (freq 24, H 0.15, seed 42): rate 1.0 gave land cut
/// p50 0.101 / p99 1.11 m per tick — so 0.25 puts the median at ~0.025 m/tick,
/// a quarter of the uplift pace, with the tail near 0.3 m/tick where it is
/// steep, wet and soft.
pub const DEFAULT_EROSION_RATE: f64 = 0.25;

/// **How much sediment moving water can hold — a SATURATION**, as a mass
/// fraction of the water carrying it.
///
/// Real rivers run ~0.4 kg/m³ suspended load on the global average and ~0.5 for
/// the Mississippi; the Yellow River, the muddiest large river on Earth, reaches
/// ~35. `0.01` is 10 kg/m³ — a heavily laden river, which is the right end of
/// the range for a tectonically active world shedding fresh orogens.
///
/// **This replaced a capacity proportional to `√flow · slope`** — stream POWER,
/// with no water in it at all. That form tracked the slope, so it stayed high
/// down a steep reach and collapsed at the slope break, dumping an entire
/// catchment's load in one place at the basin edge (Aaron, 2026-08-06: *"the
/// entire volume being moved directly to the end of the drainage basin in one
/// move"*). Saturation is a property of the water, not of the ground it is
/// running over.
const MAX_SEDIMENT_FRAC: f64 = 0.01;

/// How far sediment travels before it settles, m — the characteristic transport
/// length. A fraction `spacing / this` of whatever a stream is carrying lands in
/// every cell it crosses, so a river **exchanges all the way down** instead of
/// running full and dumping at one break. Ten cell spacings: a grain entrained
/// in the headwaters typically reaches the sea, but the floodplain, the shelf
/// and the delta each take their share on the way.
const TRANSPORT_LENGTH_M: f64 = 10.0 * 74_357.0;

/// What freshly-cut sediment weighs on the way down, kg/m³ — converts the
/// capacity's metres-of-cut currency into carried mass.
const SEDIMENT_DENSITY: f64 = 2200.0;

/// Rise over run, per cell spacing, above which rock will not stand — the
/// effective strength of crust at hex scale. Ground steeper than this sheds
/// ([`MassWasting`]) until it can stand; the bake report prints slopes against
/// this same number, in this same currency. 0.08 over the 74-km spacing is a
/// ~6-km step between neighbouring cells — Andes-class: the tallest fronts
/// real crust holds over that distance — so a genuine young collision belt
/// STANDS (measured: tighter thresholds shaved the belts onto adjacent
/// oceanic slabs, whose subduction then drained the continental inventory),
/// while the single-hex runaways (tens of kilometres, 10–15× over threshold)
/// still shed into ranges.
pub const REPOSE_SLOPE: f64 = 0.08;

/// Fraction of the over-repose EXCESS that slumps per Myr. Proportional to the
/// overshoot — the same self-limiting posture as the delamination ceiling — and
/// split across the receiving neighbours, so the relaxation can never invert a
/// slope or overshoot a pit into a peak, which is exactly the flicker this
/// mechanism exists to end. At the ~6-km threshold this touches essentially
/// nothing but the runaway convergence cells (the bake's p90 slope is 0.007
/// against the 0.08 repose), so it is set aggressive: the one white-dot spike's
/// standing height is the balance of the conveyor's pinned firehose against
/// this rate — measured, 0.25 held it near 17 km; doubling halves it.
const TALUS_RATE: f64 = 0.5;

/// No single tick may take more than this fraction of the bed it is cutting.
/// Erosion is a process, not an event, and a tick that strips a whole bed is
/// how a cell beside a steep neighbour reaches bare rock. **Kept from the
/// reverted rework** — it is a pure bound, independent of the calibration that
/// went wrong.
const MAX_STRIP_FRAC: f64 = 0.25;
/// Resistance assumed for rock the catalog does not recognise. Deliberately middling
/// and deliberately not zero — unknown rock must not erode like salt.
const UNKNOWN_RESISTANCE: f32 = 0.5;

/// Metres between neighbouring cell centres — `√(cell area)`, the run every
/// aggregate slope is measured over. The size model fixed the span, so this is
/// one number (≈74 km) on every world at every frequency — which is what lets
/// the repose threshold and the erosion rate calibrate once and hold
/// everywhere.
pub fn cell_spacing_m() -> f64 {
    crate::config::CELL_AREA_M2.sqrt()
}

/// Rise over run between two cells, in the one slope currency: metres of drop
/// per metre of [`cell_spacing_m`]. Physical, so the repose constant, the bake
/// report's histogram and the cut law all speak the same number.
pub(crate) fn slope_between(drop_m: f64) -> f64 {
    (drop_m / cell_spacing_m()).max(0.0)
}

/// **The stream-power law** — metres of rock one tick's gathered flow can cut at
/// this slope through this rock. One definition, shared by the tick and the
/// forcing probe, so the report can never measure a different law than the one
/// that runs.
pub(crate) fn stream_cut_m(rate: f64, flow: f64, slope: f64, resistance: f64, dt_myr: f64) -> f64 {
    rate * flow.max(0.0).sqrt() * slope * dt_myr / resistance.clamp(0.02, 1.0)
}

/// **What the water crossing a cell can HOLD, kg** — its own mass times
/// [`MAX_SEDIMENT_FRAC`]. `flow` is metres of water depth per Myr gathered from
/// the whole catchment, so the water mass through this cell in one tick is
/// `flow · dt · area · ρ_water`.
///
/// Capacity now depends on **how much water there is**, not on how steep the
/// ground is. A big river on a flat delta still carries its load — which is why
/// deltas are built out of what the river brought, not out of what fell off the
/// mountain beside them.
pub(crate) fn stream_capacity_kg(flow: f64, area: f64, dt_myr: f64) -> f64 {
    let water_kg = flow.max(0.0) * dt_myr * area * WATER_DENSITY;
    MAX_SEDIMENT_FRAC * water_kg
}

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
    ///
    /// **THROUGHPUT, not a withdrawal.** This is the total depth of water that
    /// falls on the cell over the interval — the same water falling over and
    /// over. Earth's hydrological cycle turns in ~3,700 years, so at a 1.6 Myr
    /// tick it completes some four hundred times *within one step*: this number
    /// exceeding the standing ocean depth is expected and is not a conservation
    /// problem.
    ///
    /// Nothing here moves mass — [`Weather::observe`] takes `&World`. The
    /// LEDGER is [`WaterCycle`](crate::atmosphere::WaterCycle)'s business, and
    /// it is an equilibrium: over an interval this long, evaporation and
    /// rainfall balance and the *net* transfer is near zero. Two different
    /// quantities, and conflating them is what starved erosion — the forcing
    /// field was being computed with reservoir-withdrawal semantics when
    /// nothing was being withdrawn.
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
        let sea_level = sea_level_m(world);
        let elevation: Vec<f64> = crate::planet::elevation_field(world);

        // What the air keeps — a read of which gases are actually in it.
        let greenhouse = greenhouse_k(world);

        // Surface temperature: how steeply the star strikes — radiative, so a
        // brighter star lifts temperature by the fourth root of its flux — plus
        // what the air keeps.
        let insol = stellar.max(0.0).powf(0.25);
        // Per cell, and the distinction matters: bare ground is molten rock at
        // the mantle's own temperature, which evaporates everything and rains
        // nothing. Weather happens on the parts that have frozen.
        let temp_k: Vec<f64> = (0..n)
            .map(|i| cell_surface_temp_k(world, i, insol, greenhouse))
            .collect();

        // Evaporation: only off water, and only where it is warm.
        let ocean_volume = world.reservoirs.ocean.mass_kg() / WATER_DENSITY;
        let submerged: Vec<f64> = (0..n)
            .map(|i| (sea_level - elevation[i]).max(0.0))
            .collect();
        // Depth decides WHETHER this cell is a water surface; it does not scale
        // how hard that surface evaporates. See [`EVAPORATION_FLUX_M_PER_MY`].
        let mut vapour: Vec<f64> = (0..n)
            .map(|i| {
                if submerged[i] <= 0.0 || ocean_volume <= 0.0 {
                    return 0.0;
                }
                let warmth = ((temp_k[i] - 273.0) / 40.0).clamp(0.0, 1.0);
                EVAPORATION_FLUX_M_PER_MY * warmth * dt_myr
            })
            .collect();

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
        // rises under it. The share is a FRACTION of the parcel — never more
        // water than the air above the cell holds. (The over-unity forcing this
        // used to carry was the moisture-budget defect Aaron named; it landed
        // together with the erosion-rate re-derivation, as one measured change,
        // because the old rate was calibrated against the phantom rain.)
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

        WeatherField {
            temp_k,
            rain,
            sea_level,
        }
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
        Self {
            tables,
            rate,
            stellar,
        }
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
        tables
            .compounds()
            .iter()
            .find(|c| c.id == id)
            .map(|c| (c.name.clone(), mass))
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
        // The FLEXED surface: drainage, slopes and coastlines are questions
        // about where the ground actually sits, and a plate holds its
        // neighbours up. Airy elevation would route rivers around speckle.
        let elevation: Vec<f64> = crate::planet::elevation_field(world);

        // Where each cell drains: the neighbour that lies lowest, if any does.
        // A cell with no lower neighbour is a sink — a lake, or the sea.
        let downhill: Vec<Option<usize>> = (0..n)
            .map(|i| {
                world.grid.neighbors[i]
                    .iter()
                    .map(|&j| j as usize)
                    .filter(|&j| elevation[j] < elevation[i])
                    .min_by(|&a, &b| {
                        elevation[a]
                            .partial_cmp(&elevation[b])
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            })
            .collect();

        // Gather the rain downhill, headwaters first, so a trunk carries what its
        // whole catchment fed it. Visiting cells high-to-low is the topological
        // order of the drainage graph — flow only ever moves down.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            elevation[b]
                .partial_cmp(&elevation[a])
                .unwrap_or(std::cmp::Ordering::Equal)
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
        //
        // TRANSPORT-LIMITED: the flow leaving a cell can hold only
        // [`stream_capacity_kg`] — so much of its own cutting — and both sides
        // of the ledger answer to it. Cutting stops when the stream is full
        // (a loaded river polishes rather than digs, which is what ends the
        // endless trunk incision that used to ship whole mountain ranges into
        // the abyssal sinks), and the load above capacity settles where the
        // slope slackens — floodplains at the range front, shelves at the
        // coast, filled basins — instead of riding to the deepest cell.
        let mut load: Vec<Composition> = vec![Composition::new(); n];
        for &cell in &order {
            let mut carrying = std::mem::take(&mut load[cell]);

            match downhill[cell] {
                // A sink — a basin, or the sea. The water stops, so everything it
                // was carrying settles here.
                None => land(world, cell, carrying),
                Some(to) => {
                    let slope = slope_between(elevation[cell] - elevation[to]);
                    let capacity = stream_capacity_kg(flow[cell], area, dt_myr);

                    // Ground under the sea is not being rained on and is not
                    // being cut; above it, the stream cuts only while it has
                    // room to carry what it cuts.
                    if elevation[cell] > weather.sea_level {
                        let resistance = self.surface_resistance(&world.columns[cell]) as f64;
                        let want_m = stream_cut_m(self.rate, flow[cell], slope, resistance, dt_myr);
                        let headroom_kg = (capacity - carrying.total()).max(0.0);
                        let density = world.columns[cell]
                            .layers
                            .last()
                            .map(crate::column::density_kg_m3)
                            .unwrap_or(SEDIMENT_DENSITY);
                        let cut_m = want_m.min(headroom_kg / (density * area).max(1.0));
                        for (e, m) in strip(&mut world.columns[cell], cut_m, area) {
                            carrying.add(e, m);
                        }
                    }

                    // **The exchange, at EVERY cell.** A river is not a bucket
                    // that only spills when full: part of what it carries
                    // settles wherever it goes, and it picks more up where it
                    // has room. So the load lands ALL THE WAY DOWN — the
                    // valley floor, the floodplain, the shelf, the delta — and
                    // a catchment's sediment stops arriving as one lump at the
                    // first slope break (Aaron, 2026-08-06).
                    let settle = (cell_spacing_m() / TRANSPORT_LENGTH_M).clamp(0.0, 1.0);
                    let over = (carrying.total() - capacity).max(0.0);
                    let drop_kg = over + (carrying.total() - over).max(0.0) * settle;
                    let frac = (drop_kg / carrying.total().max(1.0)).clamp(0.0, 1.0);
                    if frac > 0.0 {
                        let mut settling = Composition::new();
                        for (e, m) in carrying
                            .iter()
                            .map(|(e, m)| (e, m * frac))
                            .collect::<Vec<_>>()
                        {
                            let got = carrying.remove(e, m);
                            settling.add(e, got);
                        }
                        land(world, cell, settling);
                    }
                    load[to].add_composition(&carrying);
                }
            }
        }
    }
}

/// **Mass wasting** — ground steeper than rock can stand collapses downhill.
///
/// The aggregate's answer to over-steepening, and the mechanism the black-ring
/// defect was missing: fluvial incision digs a pit beside a peak, and nothing
/// used to answer the cliff between them, so the pair just sharpened until the
/// elevation view flickered white against black. Here any drop steeper than the
/// [`REPOSE_SLOPE`] over the cell spacing sheds its EXCESS — proportional to
/// the overshoot, like the delamination ceiling, so the relaxation is
/// self-limiting and can never invert a slope — split across the receiving
/// neighbours, drawing from the WHOLE column ([`strip_deep`]), because a
/// collapsing mountainside does not stop at the top bed.
///
/// This is also the missing lateral spread: a single-hex spike a collision
/// piled up sheds into its ring of neighbours tick after tick and becomes a
/// massif several cells wide — a mountain RANGE, which no placement rule ever
/// draws.
pub struct MassWasting;

impl Stage for MassWasting {
    fn name(&self) -> &'static str {
        "MassWasting"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let n = world.columns.len();
        if n == 0 {
            return;
        }
        let area = world.cell_area_m2();
        let threshold_m = REPOSE_SLOPE * cell_spacing_m();
        // Everything is measured against the tick-start snapshot, and each
        // donor sheds at most half its excess split by the max neighbour count
        // — so no pair can invert and no pit can be piled above its walls,
        // whatever order the cells are visited in. Geologically slow is
        // correct here; oscillation is the one thing this must never do.
        let step = (TALUS_RATE * dt_myr).min(0.5) / 6.0;
        // **LOCAL elevation here, deliberately — not the flexed field.** Mass
        // wasting answers the over-steepening of an actual pile of rock, so it
        // has to SEE the pile. Flexure describes where the plate sits, and
        // reading it here let a 16 km spike smooth to a 3.5 km apparent drop,
        // fall under the repose threshold, and stand forever with its mass
        // still heaped on one cell (caught by
        // `a_spike_sheds_into_a_range_and_never_inverts`).
        //
        // The split is principled: hydrology and coastlines are questions about
        // the SURFACE and read `elevation_field`; mass stability is a question
        // about the COLUMN and reads its own buoyancy.
        let elevation: Vec<f64> = world.columns.iter().map(|c| elevation_m(c, area)).collect();
        for cell in 0..n {
            for j in 0..world.grid.neighbors[cell].len() {
                let to = world.grid.neighbors[cell][j] as usize;
                let drop = elevation[cell] - elevation[to];
                if drop <= threshold_m {
                    continue;
                }
                let shed_m = (drop - threshold_m) * step;
                let taken = strip_deep(&mut world.columns[cell], shed_m, area);
                if !taken.is_empty() {
                    let at = world.tick_myr;
                    world.columns[to].deposit(FormationProcess::Sediment, at, &taken);
                }
            }
        }
    }
}

/// **What share of the air over a cell wrings out**, `0..1` — cool air gives up
/// more, and ground that RISES under the air forces it up and wrings it harder.
///
/// **Bounded by 1**: a cell cannot rain more water than the air above it
/// holds. This is the moisture-budget fix (the unbounded forcing reached 5.25
/// and was the phantom water the old erosion rate was calibrated against); it
/// landed together with that rate's re-derivation, per the
/// correction-and-recalibration-are-one-change lesson.
fn wrung_fraction(temp_k: f64, rise_m: f64) -> f64 {
    let cool = ((300.0 - temp_k) / 60.0).clamp(0.0, 1.0);
    let lift = (rise_m / 2000.0).clamp(0.0, 1.0) * OROGRAPHIC_GAIN;
    (CONDENSATION_RATE * (BASE_WRING + cool + lift)).clamp(0.0, 1.0)
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
    let taken = top.release(if whole {
        1.0
    } else {
        (want / have).min(MAX_STRIP_FRAC)
    });
    if col.layers.last().is_some_and(|l| l.mass_kg() <= 0.0) {
        col.layers.pop();
    }
    taken
}

/// Take up to `depth_m` off a column **through as many beds as it takes** — the
/// mass-wasting draw. A collapsing slope does not stop at the top bed the way a
/// river's polish does, so this has no per-bed strip cap; it is bounded instead
/// by the caller's overshoot-proportional `depth_m`. Same film rule as
/// everything else: a bed worn to less than `MIN_BED_MASS_KG` goes whole, and
/// "gone" is a mass test, never `is_empty` on a ledger that can carry
/// zero-valued keys.
fn strip_deep(col: &mut Column, depth_m: f64, area: f64) -> Vec<(ElementId, f64)> {
    let mut left = depth_m;
    let mut taken: Vec<(ElementId, f64)> = Vec::new();
    while left > 0.0 {
        let Some(top) = col.layers.last_mut() else {
            break;
        };
        let have = top.mass_kg();
        if have <= 0.0 {
            col.layers.pop();
            continue;
        }
        let density = crate::column::density_kg_m3(top);
        let want_kg = left * density * area;
        let whole = have - want_kg < crate::column::MIN_BED_MASS_KG;
        let got = top.release(if whole {
            1.0
        } else {
            (want_kg / have).min(1.0)
        });
        let got_kg: f64 = got.iter().map(|&(_, m)| m).sum();
        left -= got_kg / (density * area).max(1.0);
        taken.extend(got);
        if col.layers.last().is_some_and(|l| l.mass_kg() <= 0.0) {
            col.layers.pop();
        }
        if !whole || got_kg <= 0.0 {
            break;
        }
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
        let mut s = Scheduler::new(
            crate::formation_stages(Arc::clone(&t), &w, &crate::Levers::brisk()),
            seed,
        );
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
            t.erosional_resistance(
                std::iter::once((mineral.to_string(), 1.0)),
                UNKNOWN_RESISTANCE,
            )
        };
        let quartz = of("Quartz");
        let halite = of("Halite");
        assert!(
            quartz > halite,
            "quartz {quartz} should outlast halite {halite}"
        );
        let unknown = t.erosional_resistance(
            std::iter::once(("Nothing The Catalog Knows".to_string(), 1.0)),
            UNKNOWN_RESISTANCE,
        );
        assert_eq!(
            unknown, UNKNOWN_RESISTANCE,
            "unknown rock must not erode like salt"
        );
    }

    /// Soft ground goes first. Two columns standing at the same height above the
    /// same drop, differing ONLY in what they are made of, do not lose the same
    /// amount — which is the difference between a landscape developing shape and
    /// merely getting lower.
    #[test]
    fn soft_ground_goes_first() {
        let t = tables();
        let area = 5.0e9;
        let quartz = t
            .compounds()
            .iter()
            .find(|c| c.name == "Quartz")
            .expect("quartz");
        let halite = t
            .compounds()
            .iter()
            .find(|c| c.name == "Halite")
            .expect("halite");

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
                cooled: 0.0,
                eclogitised: 0.0,
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
        assert_eq!(
            w.columns[cell].layers.len(),
            before + 1,
            "the sediment became a bed"
        );
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
        assert!(
            slope > flat,
            "rising ground wrings harder: {slope} vs {flat}"
        );
        assert!(wrung_fraction(250.0, 0.0) > flat, "and so does cold air");
        // The moisture budget: however hard the forcing, a cell cannot rain
        // more than the air above it holds. (The over-unity defect that used
        // to live here was the phantom water the old erosion rate was
        // calibrated against; it was removed together with that rate's
        // re-derivation.)
        assert!(
            wrung_fraction(250.0, 50_000.0) <= 1.0,
            "a share of a parcel is never more than the parcel"
        );
    }

    /// **Saturation is a property of the WATER.** What a stream can hold rides
    /// how much water crosses the cell and nothing else — a big river on a flat
    /// delta still carries its load, which is how a delta gets built out of
    /// what the river brought. The old law multiplied by slope, so capacity
    /// collapsed at every slope break and a whole catchment's sediment landed
    /// in one place.
    #[test]
    fn capacity_is_a_saturation_of_the_water_not_of_the_slope() {
        let area = crate::config::CELL_AREA_M2;
        let small = stream_capacity_kg(50.0, area, 1.6);
        let trunk = stream_capacity_kg(500.0, area, 1.6);
        assert!(trunk > small, "more water carries more sediment");
        assert!(
            (trunk / small - 10.0).abs() < 1e-9,
            "and it is LINEAR in the water — ten times the flow, ten times the load"
        );
        assert_eq!(
            stream_capacity_kg(0.0, area, 1.6),
            0.0,
            "no water carries nothing"
        );
        // It is exactly the saturation fraction of the water's own mass.
        let water = 50.0 * 1.6 * area * WATER_DENSITY;
        assert!((small - MAX_SEDIMENT_FRAC * water).abs() < 1.0);
    }

    /// **A river exchanges all the way down.** Even a stream well under its
    /// saturation drops part of its load in every cell it crosses, so sediment
    /// lands along the whole path instead of arriving as one lump wherever the
    /// gradient first slackens.
    #[test]
    fn a_stream_drops_something_in_every_cell_it_crosses() {
        let settle = (cell_spacing_m() / TRANSPORT_LENGTH_M).clamp(0.0, 1.0);
        assert!(
            settle > 0.0 && settle < 1.0,
            "a share settles per cell: {settle}"
        );
        // Under capacity, the drop is that share — not zero, which is what the
        // overflow-only rule gave.
        let carrying = 1.0e15f64;
        let capacity = 1.0e18f64;
        let over = (carrying - capacity).max(0.0);
        let drop = over + (carrying - over).max(0.0) * settle;
        assert!(
            over == 0.0 && drop > 0.0,
            "well under capacity, and still deposits {drop:.3e} kg"
        );
    }

    /// **It rains everywhere.** Rising ground still wrings harder — that is what
    /// makes a wet windward slope and a dry shadow — but it no longer takes
    /// essentially all of the world's rain: the flank-to-flat ratio is the 2–5×
    /// real orography gives, not the 8× that left every interior arid.
    #[test]
    fn orography_modulates_the_rain_rather_than_monopolising_it() {
        let flat = wrung_fraction(300.0, 0.0);
        let flank = wrung_fraction(300.0, 3000.0);
        assert!(flat > 0.2, "flat warm ground still rains: {flat}");
        assert!(
            flank > flat,
            "and rising ground rains harder: {flank} vs {flat}"
        );
        let ratio = flank / flat;
        assert!(
            (2.0..=5.0).contains(&ratio),
            "orographic enhancement {ratio:.1}x is realistic"
        );
    }

    /// **Over-steep ground sheds until it can stand.** A single-hex spike — the
    /// white-dot defect — spreads into its ring of neighbours instead of
    /// flickering against them: every pair's slope relaxes toward the repose
    /// threshold, monotonically, conserving every gram, and no neighbour is
    /// ever piled above the donor (the inversion that made the old picture
    /// wiggle).
    #[test]
    fn a_spike_sheds_into_a_range_and_never_inverts() {
        let (mut w, _t) = world(4, 11, 0);
        let area = w.cell_area_m2();
        // Build a genuinely absurd spike from the whole planetoid's mantle —
        // tens of kilometres over its neighbours, like the runaways the bake
        // used to keep.
        let spike = 0usize;
        let mut melt: Vec<(ElementId, f64)> = Vec::new();
        for (e, want) in [(14u8, 6.0e17), (8u8, 4.0e17), (13u8, 2.0e17)] {
            let mut want = want;
            for src in 0..w.mantle.n_cells() {
                if want <= 0.0 {
                    break;
                }
                let took = w.mantle.remove(src, e, want);
                if took > 0.0 {
                    melt.push((e, took));
                }
                want -= took;
            }
        }
        w.columns[spike].deposit(FormationProcess::ContinentalArc, 0.0, &melt);
        w.audit("spike fixture");

        let elev = |w: &World, i: usize| elevation_m(&w.columns[i], area);
        let worst = |w: &World| {
            (0..w.columns.len())
                .flat_map(|i| w.grid.neighbors[i].iter().map(move |&j| (i, j as usize)))
                .map(|(i, j)| elev(w, i) - elev(w, j))
                .fold(f64::MIN, f64::max)
        };
        let before = worst(&w);
        assert!(
            before > 10_000.0,
            "the fixture is a real runaway: {before:.0} m"
        );

        let stage = MassWasting;
        let mut rng = crate::stage::StageRng::new(4);
        let mut last = before;
        for _ in 0..200 {
            stage.tick(&mut w, crate::config::NOMINAL_DT_MYR, &mut rng);
            w.audit("MassWasting");
            let now = worst(&w);
            assert!(
                now <= last + 1e-6,
                "relaxation is monotone: {last:.1} → {now:.1}"
            );
            // The spike stays the local high: shedding may never invert the pair.
            for &j in &w.grid.neighbors[spike] {
                assert!(
                    elev(&w, spike) >= elev(&w, j as usize) - 1e-6,
                    "a neighbour was piled above the donor"
                );
            }
            last = now;
        }
        assert!(
            last < before * 0.5,
            "the cliff relaxed: {before:.0} → {last:.0} m"
        );
        let grew = w.grid.neighbors[spike]
            .iter()
            .filter(|&&j| {
                w.columns[j as usize]
                    .layers
                    .iter()
                    .any(|l| l.formed_by == FormationProcess::Sediment)
            })
            .count();
        assert!(
            grew >= 3,
            "the spike became a massif: {grew} neighbours carry its talus"
        );
    }

    /// **EROSION MUST WEAR THE WORLD DOWN AT A GEOLOGICAL RATE.**
    ///
    /// Not an outcome assertion — it names no landscape and requires no shape.
    /// It pins the *order of magnitude* of a physical quantity that is measured
    /// on the real Earth, which is the one kind of number this simulation may be
    /// held to (the same standing correction that says a real-world quantity is
    /// supplied and cited, never guessed).
    ///
    /// **It exists because a five-order-of-magnitude error passed the whole
    /// suite, twice, in both directions.** Evaporation was written as a fraction
    /// of the ocean's DEPTH per Myr, which structurally capped the hydrological
    /// cycle at 0.35 turnovers/Myr against Earth's ~270; denudation ran at
    /// **21 mm/Myr** for weeks with every test green. Fixing it multiplied rain
    /// by ~17,000× and every test stayed green again. The suite checked
    /// mechanism — does capacity bind, does sediment conserve — and nothing
    /// checked SCALE.
    ///
    /// The band is deliberately wide. Real continental denudation spans cratons
    /// at ~5 m/Myr to active orogens at 10–100; anywhere inside two orders of
    /// that is a world doing recognisable geology, and outside it is a world
    /// where rivers are either ornamental or catastrophic.
    #[test]
    fn erosion_wears_the_world_down_at_a_geological_rate() {
        let t = tables();
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let mut w = World::seed(icosphere(12), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..400 {
            sched.step(&mut w, dt, None);
        }

        let area = w.cell_area_m2();
        let weather = Weather::observe(&w, dt, levers.stellar_heat);
        let elevation: Vec<f64> = w.columns.iter().map(|c| elevation_m(c, area)).collect();
        let n = w.columns.len();
        let mut cuts: Vec<f64> = Vec::new();
        for i in 0..n {
            if elevation[i] <= weather.sea_level {
                continue;
            }
            let low = w.grid.neighbors[i]
                .iter()
                .map(|&j| elevation[j as usize])
                .fold(f64::INFINITY, f64::min);
            if !low.is_finite() || low >= elevation[i] {
                continue;
            }
            let resistance = w.columns[i]
                .layers
                .last()
                .map(|l| bed_resistance(&t, l) as f64)
                .unwrap_or(1.0);
            cuts.push(stream_cut_m(
                levers.erosion_rate,
                weather.rain[i],
                slope_between(elevation[i] - low),
                resistance,
                dt,
            ));
        }
        assert!(
            !cuts.is_empty(),
            "the fixture has land with somewhere to drain"
        );
        cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median_m_per_my = cuts[cuts.len() / 2] / dt;

        assert!(
            (0.05..=200.0).contains(&median_m_per_my),
            "median denudation {median_m_per_my:.4} m/Myr is outside anything geology does \
             (cratons ~5, active orogens 10–100). Under it, rivers are ornamental and the \
             world keeps whatever tectonics builds; over it, the continents wash away."
        );
    }

    /// **The forcing, measured on a real world** — the instrument the reverted
    /// rework was missing (measurement-first: constants are chosen against
    /// these numbers, not guessed). Bakes the H=0.15 world to a mid-life state
    /// and prints the distributions of rain, flow, slope, wanted cut and
    /// capacity over land.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn erosion_forcing_report() {
        let t = tables();
        let b = Budget::from_dir(&content_data_dir(), &t)
            .expect("budget")
            .rescaled(&[(1, 0.15)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let mut s = Scheduler::new(
            crate::formation_stages(Arc::clone(&t), &w, &crate::Levers::default()),
            42,
        );
        for _ in 0..1500 {
            s.step(&mut w, crate::config::NOMINAL_DT_MYR, None);
        }

        let area = w.cell_area_m2();
        let dt = crate::config::NOMINAL_DT_MYR;
        let weather = Weather::observe(&w, dt, 1.0);
        let elevation: Vec<f64> = w.columns.iter().map(|c| elevation_m(c, area)).collect();
        let n = w.columns.len();
        let downhill: Vec<Option<usize>> = (0..n)
            .map(|i| {
                w.grid.neighbors[i]
                    .iter()
                    .map(|&j| j as usize)
                    .filter(|&j| elevation[j] < elevation[i])
                    .min_by(|&a, &b| {
                        elevation[a]
                            .partial_cmp(&elevation[b])
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            })
            .collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            elevation[b]
                .partial_cmp(&elevation[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut flow: Vec<f64> = weather.rain.clone();
        for &cell in &order {
            if let Some(to) = downhill[cell] {
                flow[to] += flow[cell];
            }
        }

        let land: Vec<usize> = (0..n)
            .filter(|&i| elevation[i] > weather.sea_level && downhill[i].is_some())
            .collect();
        let pct = |mut v: Vec<f64>| -> (f64, f64, f64, f64) {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let at = |q: f64| v[(((v.len() - 1) as f64) * q) as usize];
            (at(0.5), at(0.9), at(0.99), *v.last().unwrap_or(&0.0))
        };
        let erosion = Erosion::new(Arc::clone(&t), DEFAULT_EROSION_RATE, 1.0);
        let slope_of = |i: usize| slope_between(elevation[i] - elevation[downhill[i].unwrap()]);
        let (r50, r90, r99, rmax) = pct(land.iter().map(|&i| weather.rain[i]).collect());
        let (f50, f90, f99, fmax) = pct(land.iter().map(|&i| flow[i]).collect());
        let (s50, s90, s99, smax) = pct(land.iter().map(|&i| slope_of(i)).collect());
        let (c50, c90, c99, cmax) = pct(land
            .iter()
            .map(|&i| {
                let res = erosion.surface_resistance(&w.columns[i]) as f64;
                stream_cut_m(DEFAULT_EROSION_RATE, flow[i], slope_of(i), res, dt)
            })
            .collect());
        let (k50, k90, k99, kmax) = pct(land
            .iter()
            .map(|&i| stream_capacity_kg(flow[i], area, dt))
            .collect());
        eprintln!(
            "land {} of {n} · sea {:.0} m · spacing {:.0} m · repose {}\n\
             rain  p50 {r50:.3}  p90 {r90:.3}  p99 {r99:.3}  max {rmax:.3}  (m/tick)\n\
             flow  p50 {f50:.2}  p90 {f90:.2}  p99 {f99:.2}  max {fmax:.2}\n\
             slope p50 {s50:.5}  p90 {s90:.5}  p99 {s99:.5}  max {smax:.5}\n\
             cut   p50 {c50:.3}  p90 {c90:.3}  p99 {c99:.3}  max {cmax:.3}  (m/tick at rate {})\n\
             cap   p50 {k50:.2e}  p90 {k90:.2e}  p99 {k99:.2e}  max {kmax:.2e}  (kg)",
            land.len(),
            weather.sea_level,
            cell_spacing_m(),
            REPOSE_SLOPE,
            DEFAULT_EROSION_RATE,
        );
    }
}
