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

pub mod budget;
pub mod column;
pub mod config;
pub mod crust;
pub mod interior;
pub mod mantle;
pub mod observer;
pub mod planet;
pub mod reservoir;
pub mod scheduler;
pub mod stage;

pub use budget::{Budget, BudgetError};
pub use column::{
    crust_kind, crust_thickness_m, density_kg_m3, elevation_m, thickness_m, Column, CrustKind,
    FormationProcess, Layer, MANTLE_DENSITY,
};
pub use crust::{CrustGeneration, Subduction};
pub use config::{
    content_data_dir, CELL_AREA_M2, NOMINAL_DT_MYR, PLANET_CELLS, PLANET_FREQ, PLANET_MASS_KG,
    PLANET_RADIUS_M,
};
pub use interior::{radiogenic_power_w, CoreFormation, MantleConvection, RadiogenicDecay};
pub use mantle::{MantleField, MAGMA_OCEAN_K};
pub use observer::{PlateEvent, PlateId, PlateObservation, PlateObserver, PlateRecord, Seam};
pub use planet::{PlanetState, World};
pub use reservoir::{Ocean, Reservoirs};
pub use scheduler::{CellProgress, Scheduler};
pub use stage::{Stage, StageRng};

/// The M1 interior formation stages, in order (spec §7.5): radiogenic heat → core
/// differentiation (iron sinks to the core) → mantle convection.
pub fn interior_stages() -> Vec<Box<dyn Stage>> {
    vec![
        Box::new(RadiogenicDecay),
        Box::new(CoreFormation),
        Box::new(MantleConvection),
    ]
}

/// The full formation pipeline through M2: the interior stages, then crust
/// generation and subduction (Airy isostasy is the derived `elevation_m` read, not
/// a stage). The app and M2 tests register these to run the planet forward.
pub fn formation_stages() -> Vec<Box<dyn Stage>> {
    let mut stages = interior_stages();
    stages.push(Box::new(CrustGeneration));
    stages.push(Box::new(Subduction));
    stages
}
