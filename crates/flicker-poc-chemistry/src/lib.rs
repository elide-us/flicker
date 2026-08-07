//! flicker-poc-chemistry — the chemistry-first world simulation (**M0 + M1**).
//!
//! > Simulate the chemistry. Everything else is derived.
//!
//! The planet begins as a **bulk accretion budget** (`accretion.json`, spec §3) —
//! an undifferentiated hot ball — and every feature (core, mantle, crust, ocean,
//! ore) is an *output* of processes acting on a conserved mass ledger, never a
//! seed. Earth-likeness is an outcome, never a target; a run that ends as a Venus
//! or a Mars is a correct run.
//!
//! This crate is **GPU-free**: the binary (`main.rs`) wraps it in a flicker-shell
//! app, but the simulation here needs no renderer. M0 lands:
//!
//! - the type layer ([`Budget`], [`Reservoirs`], [`Column`]/[`Layer`],
//!   [`World`], [`PlanetState`]);
//! - the two conserved ledgers (`Composition` / `CompoundLedger` from
//!   `flicker-worldstate`) wired into reservoirs and column stacks;
//! - the steppable [`Scheduler`] + worker-pool cell sweep + [`CellProgress`];
//! - and — first, per the spec — the **conservation harness** ([`World::audit`]),
//!   proven by a deliberately-leaking stage in the tests.
//!
//! Derived properties (`elevation`, `crust_kind`, `thickness`, `density`,
//! `hardness`) are **functions, never fields** — see [`column`].
//!
//! **M1 (the interior)** adds the per-cell [`MantleField`], radiogenic heat,
//! **core differentiation** (iron sinks inward and the planet separates a metallic
//! core from a silicate mantle), mantle convection (the semi-Lagrangian resample of
//! §6.1). The crust / volatile / thermostat / life / rock
//! stages (M2–M6) and the surface phase (M7) are not here yet; see the build spec.

pub mod atmosphere;
pub mod biosphere;
pub mod budget;
pub mod column;
pub mod config;
pub mod crust;
pub mod habitability;
pub mod hydrothermal;
pub mod infall;
pub mod interior;
pub mod mantle;
pub mod observer;
pub mod planet;
pub mod process_file;
pub mod reservoir;
pub mod scheduler;
pub mod stage;
pub mod surface;
pub mod tectonics;

pub use atmosphere::{
    air_shells, AirShell, CarbonSink, GasVocabulary, Outgassing, WaterCycle, DEFAULT_OUTGAS_RATE,
};
pub use biosphere::{
    Biosphere, LifeStage, Maturation, DEFAULT_DECOMPOSER_NICHE_KG, DEFAULT_PRODUCTION_RATE,
};
pub use budget::{Budget, BudgetError};
pub use column::{
    basal_pressure_pa, crust_kind, crust_thickness_m, density_kg_m3, dissimilarity, elevation_m,
    overburden_pa, thickness_m, Column, CrustKind, FormationProcess, Layer, GRAVITY_M_S2,
    MANTLE_DENSITY, SUBDUCTABLE_DENSITY,
};
pub use crust::{
    CrustDensification, CrustGeneration, Crystallization, Delamination, StrataReconcile,
    Volcanism,
    DEFAULT_ERUPTION_RATE, STRATA_SOFT_CAP,
};
pub use tectonics::{audit_occupancy, cell_spacing, Conveyor};
pub use surface::{bed_resistance, greenhouse_k, Erosion, Weather, WeatherField};
pub use infall::{LateVeneer, WaterDelivery};
pub use hydrothermal::{enrichment, is_playable, prospect, Hydrothermal, Prospect};
pub use config::{
    cell_area_m2, content_data_dir, NOMINAL_DT_MYR, PLANET_CELLS, PLANET_FREQ, PLANET_MASS_KG,
    PLANET_RADIUS_M,
};
pub use interior::{radiogenic_power_w, CoreFormation, MantleConvection, RadiogenicDecay};
pub use mantle::{MantleField, MAGMA_OCEAN_K};
pub use observer::{PlateEvent, PlateId, PlateObservation, PlateObserver, PlateRecord, Seam};
pub use habitability::{observe as observe_habitability, Habitability};
pub use planet::{p_co2_pa, sea_level_m, PlanetState, World};
pub use process_file::{load_processes, Gate, Gated, ProcessDef};
pub use scheduler::ProcessState;
pub use reservoir::{Ocean, Reservoirs};
pub use scheduler::{CellProgress, Scheduler};
pub use stage::{Stage, StageRng};

/// The M1 interior formation stages, in order (spec §7.5): radiogenic heat → core
/// differentiation (iron sinks to the core) → mantle convection.
pub fn interior_stages() -> Vec<Box<dyn Stage>> {
    vec![
        Box::new(RadiogenicDecay { heat: 1.0 }),
        Box::new(CoreFormation),
        Box::new(MantleConvection),
    ]
}

/// Mass of the late veneer, kg — a few parts in ten thousand of the planet, which
/// is the scale of Earth's own. Tiny, and the difference between a world with
/// accessible gold and one without.
pub const DEFAULT_VENEER_KG: f64 = 2.0e21;

/// **What the maintainer may set about a forming world.**
///
/// Two kinds of thing, and the distinction is the whole discipline: the three
/// **inputs at the system boundary** — how much water arrives, how hot the inside
/// is, how hard the star shines — and the **rates** at which processes run. Set a
/// condition or set a pace; then wait and see what the world does with it.
///
/// There is deliberately nothing here that writes a result. No lever raises a
/// mountain, floods a basin, or puts an ore body anywhere, because a control that
/// could paint a continent would make every observation of the world afterwards
/// unfalsifiable. Every field below is a parameter of a transformation.
///
/// The multipliers are `1.0` for "as the physics has it"; the budgets are absolute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levers {
    // ── The three inputs at the boundary ──
    /// Total water delivered over the run, kg. More floods more of the world —
    /// though how much more is the sea-level solve's answer, not this dial's.
    pub water_budget_kg: f64,
    /// Coverage at which the infall CUTS OFF, `0..1`: delivery runs only while
    /// the solved submerged fraction stands below this. A condition on the
    /// boundary input, not an outcome written into the world — how much water
    /// any coverage takes is the hypsometry's answer, hitting the target is a
    /// real gate transition, and coverage remains free to drift afterwards as
    /// mountains rise and seas boil. `1.0` disables the cutoff (the budget
    /// alone governs). **The dial only bites above the planet's self-watered
    /// floor**: the mantle's own exhaled steam makes a sea with no infall at
    /// all, so a true desert world needs the accreted-H endowment lowered, not
    /// just the comets turned away.
    pub water_coverage_target: f64,
    /// How much of the late metal veneer arrives, kg.
    pub veneer_budget_kg: f64,
    /// Multiplier on radiogenic heating — the heat from inside.
    pub core_heat: f64,
    /// Multiplier on insolation — the heat from the star.
    pub stellar_heat: f64,

    // ── The rates ──
    /// How fast bare cooled mantle freezes into sea floor.
    pub crust_gen_rate: f64,
    /// How much of a sunken slab returns as arc melt.
    pub arc_return: f64,
    /// Fraction of a hot cell's volatile inventory that degasses per Myr at full
    /// heat — how hard the young planet exhales its air.
    pub outgas_rate: f64,
    /// Fraction of a plume cell's mantle erupted per Myr at full vigor — how
    /// violent the world's volcanism is.
    pub eruption_rate: f64,
    /// Fraction of the air's carbon that life fixes per Myr — how vigorous the
    /// biosphere is.
    pub production_rate: f64,
    /// Buried lignin, kg, at which the decomposer guild evolves to eat wood.
    /// **This is the planet's coal endowment dial**: raise it and the burial
    /// window stays open longer. It sets a CONDITION for the guild's arrival,
    /// never an amount of coal — how much actually gets buried before the
    /// window shuts is the world's business.
    pub decomposer_niche_kg: f64,
    /// Fraction of the remaining water budget delivered per Myr — how hard the
    /// outer system rains comets, distinct from how MUCH ([`water_budget_kg`](Self::water_budget_kg)).
    pub water_delivery_rate: f64,
    /// Strain, as a multiple of the planet's mean, at which lithosphere yields.
    /// Higher gives fewer, larger plates.
    pub yield_strain: f32,
    /// Erosive power per unit of gathered flow and slope.
    pub erosion_rate: f64,
    /// How much of a hot bed's metal the circulating fluid takes per Myr.
    pub leach_rate: f64,
}

impl Default for Levers {
    /// The physics as written — every multiplier at one, every rate at the value
    /// the process chose for itself.
    fn default() -> Self {
        Self {
            water_budget_kg: surface::DEFAULT_WATER_KG,
            water_coverage_target: 1.0,
            veneer_budget_kg: DEFAULT_VENEER_KG,
            core_heat: 1.0,
            stellar_heat: 1.0,
            crust_gen_rate: crust::DEFAULT_CRUST_GEN_RATE,
            outgas_rate: atmosphere::DEFAULT_OUTGAS_RATE,
            eruption_rate: crust::DEFAULT_ERUPTION_RATE,
            production_rate: biosphere::DEFAULT_PRODUCTION_RATE,
            decomposer_niche_kg: biosphere::DEFAULT_DECOMPOSER_NICHE_KG,
            arc_return: tectonics::DEFAULT_ARC_RETURN,
            water_delivery_rate: infall::DEFAULT_WATER_DELIVERY_RATE,
            yield_strain: tectonics::DEFAULT_YIELD_STRAIN,
            erosion_rate: surface::DEFAULT_EROSION_RATE,
            leach_rate: hydrothermal::DEFAULT_LEACH_RATE,
        }
    }
}

impl Levers {
    /// The **mechanism-test speeds** — the pre-recalibration rate constants
    /// (retired 2026-08-06 when the defaults moved to geologic e-folds), kept so
    /// a test that probes a *mechanism* finishes in tens of ticks instead of
    /// thousands. The app never uses these: [`Default`](Self::default) is the
    /// physics as written; `brisk` is a test fixture, not a preset.
    pub fn brisk() -> Self {
        Self {
            crust_gen_rate: 0.03,
            outgas_rate: 0.01,
            production_rate: 0.004,
            leach_rate: 0.02,
            water_delivery_rate: 0.004,
            ..Self::default()
        }
    }
}

/// **The world, in the order it works.** One pipeline — there is no version of
/// this planet without weather, so there is no second list to drift from this one.
///
/// The interior drives everything, so it goes first: heat, differentiation, and the
/// convection whose flow the plates ride. The hot mantle **exhales** — outgassing
/// fills the air with real gas compounds, and from that moment the greenhouse read
/// is warming the weather. Bare mantle that has cooled enough freezes
/// into sea floor — and where hot mantle sits under that fresh lid, it **erupts
/// through it**, piling lava into volcanoes and venting the gas that was
/// dissolved in the melt. Water arrives from outside, and the **sky decides where it
/// stands**: a sea boils off molten ground, a cooling air rains its excess into
/// the ocean, and the vapour that stays aloft is itself a greenhouse gas — the
/// feedback is in the loop, not in a rule. A standing sea then **drinks the
/// air's carbon** and lays it down as calcite on the floor, which is where
/// limestone comes from and why an old wet world trends toward a nitrogen sky.
/// If the world is temperate and wet, **life** takes hold in it: it pulls carbon
/// out of the sky into tissue and gives back the oxygen, what is exposed rots
/// (as methane while there is nothing to breathe with, as carbon dioxide once
/// there is), and what gets buried before it can rot **cooks into coal and oil**.
/// Then the **conveyor** — the plates
/// take their shape from the flow, each turns about its own axis, and everything
/// that happens where two stacks meet happens there. Hydrothermal circulation works
/// on the hot rock the conveyor has just arranged. Then the weather works on the
/// ground: a range has to be raised before rain can take it down. Free elements
/// organise into minerals, which is what lets the rock tier recognise what the
/// surface is made of. And last, each column reconciles the stack it now carries,
/// including the sediment that landed on it this tick.
///
/// Isostasy, sea level and hypsometry are **not** in this list — they are derived
/// reads ([`elevation_m`], [`sea_level_m`]), computed on demand from the ledger
/// rather than stepped. Neither is subduction: it is not a stage, it is what
/// happens to the denser of two stacks that want the same ground.
pub fn formation_stages(
    tables: std::sync::Arc<flicker_materials::Tables>,
    budget: &Budget,
    levers: &Levers,
) -> Vec<Box<dyn Stage>> {
    let defs = process_file::load_processes(&config::content_data_dir());
    defs.into_iter()
        .map(|def| {
            let stage = build_stage(&def.runs, &tables, budget, levers).unwrap_or_else(|| {
                panic!(
                    "processes.json runs '{}', but no such transformation is registered — \
                     new physics is written in Rust first, then named in the file",
                    def.runs
                )
            });
            Box::new(process_file::Gated::new(stage, def.gate, *levers)) as Box<dyn Stage>
        })
        .collect()
}

/// The transformation registry — every stage the pipeline file may name. The
/// FILE decides which of these run, in what order, behind what gate; this match
/// only knows how to build each one from the live levers.
fn build_stage(
    name: &str,
    tables: &std::sync::Arc<flicker_materials::Tables>,
    budget: &Budget,
    levers: &Levers,
) -> Option<Box<dyn Stage>> {
    Some(match name {
        "RadiogenicDecay" => Box::new(RadiogenicDecay { heat: levers.core_heat }),
        "CoreFormation" => Box::new(CoreFormation),
        "MantleConvection" => Box::new(MantleConvection),
        "Outgassing" => Box::new(Outgassing::new(tables, levers.outgas_rate)),
        "CrustGeneration" => Box::new(CrustGeneration { rate: levers.crust_gen_rate }),
        "CrustDensification" => Box::new(CrustDensification),
        "Delamination" => Box::new(Delamination),
        "Volcanism" => Box::new(Volcanism::new(tables, levers.eruption_rate)),
        "WaterDelivery" => Box::new({
            let mut delivery = WaterDelivery::new(tables);
            delivery.budget_kg = levers.water_budget_kg;
            delivery.rate = levers.water_delivery_rate;
            delivery.target_coverage = levers.water_coverage_target;
            delivery
        }),
        "WaterCycle" => Box::new(WaterCycle::new(tables, levers.stellar_heat)),
        "CarbonSink" => Box::new(CarbonSink::new(tables)),
        "Biosphere" => {
            Box::new(Biosphere::new(tables, levers.production_rate, levers.decomposer_niche_kg))
        }
        "Maturation" => Box::new(Maturation::new(tables)),
        "LateVeneer" => Box::new(LateVeneer::new(tables, budget, levers.veneer_budget_kg)),
        "Conveyor" => {
            Box::new(Conveyor { yield_strain: levers.yield_strain, arc_return: levers.arc_return })
        }
        "Hydrothermal" => Box::new(Hydrothermal::new(tables, levers.leach_rate)),
        "Erosion" => Box::new(Erosion::new(
            std::sync::Arc::clone(tables),
            levers.erosion_rate,
            levers.stellar_heat,
        )),
        "Crystallization" => Box::new(Crystallization::new(std::sync::Arc::clone(tables))),
        "StrataReconcile" => Box::new(StrataReconcile),
        _ => return None,
    })
}
