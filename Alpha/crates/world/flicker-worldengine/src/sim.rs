//! **The tick simulation** — stripped to Epoch 1 + Epoch 2.
//!
//! One evolving [`World`] is ticked forward. **Epoch 1** is the molten seed (the composition
//! scatter, tick 0). Each tick then runs **one Epoch-2 molten-convection pass** over it. That
//! is the whole simulation for now — deliberately minimal.
//!
//! This is the foundation for a *condition-derived* event simulation: later, environment
//! events (outgassing, water delivery, crust solidification, tectonics, …) get added as
//! checks that fire when the planet's **state** crosses a threshold — never scheduled to a
//! tick number. The tick is only the clock. One tick = the time for the crust to move one hex
//! (~49.65 mi) at plate speed — see [`MY_PER_TICK`].
//!
//! History is deterministic re-sim: the sim stores no per-tick history; [`Simulation::state_at`]
//! re-runs from the nearest cached checkpoint, so a tick is a pure function of the prior tick.

use std::collections::BTreeMap;

use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::cooling::{T_MOLTEN, T_WATER_DELIVERY};
use flicker_worldgen::{
    processes, run_tick, Epoch1, EpochCtx, HexState, Process, MY_PER_TECTONIC_STEP,
};
use flicker_worldgrid::{icosphere_with_outlines, Sphere};
use glam::Vec3;

use crate::config::{build_epoch1, WorldConfig};
use crate::levers::{GeneratorError, GeneratorParams};

/// Millions of years one tick represents: **the time for the crust to move one hex**
/// (~49.65 mi) at plate speed. Derived from the same cm/yr constant as the tectonic timeline —
/// [`MY_PER_TECTONIC_STEP`] is that time per 0.2-hex advance, so one whole hex is ×5.
pub const MY_PER_TICK: f32 = MY_PER_TECTONIC_STEP * 5.0;

/// The planet's water endowment as a fraction of its seed element mass — the finite
/// outer-system reservoir (the R8a water control input). Legibility-exaggerated for the POC
/// viewer (real oceans are a tiny mass fraction) so the hydrosphere shell reads; it is an
/// INPUT knob, never a target — the ocean that results is emergent from the cooling path.
const WATER_BUDGET_FRAC: f64 = 0.05;
/// Fraction of the *remaining* water budget delivered per tick while the delivery window is
/// open — a flux, so water accumulates gradually (geological) and total delivery is bounded
/// by the depleting budget, not by a designed ocean size.
const WATER_DELIVERY_RATE: f64 = 0.06;

/// Checkpoint every N ticks so a reset / scrub re-sims only a short span.
const CHECKPOINT_INTERVAL: u64 = 16;

/// The single evolving planet: the per-hex state + the tick count. Cloneable, so a checkpoint
/// is a snapshot and the re-sim forks from it.
#[derive(Clone)]
pub struct World {
    /// Per-hex state, ticked forward by the process pipeline. Each hex owns its own
    /// temperature (K) — the truth.
    pub cells: Vec<HexState>,
    /// Ticks elapsed since the molten seed.
    pub tick: u64,
    /// Planet **mean temperature** (K) — derived from the per-hex temperatures each tick, for
    /// the HUD and planetary gauges. The per-hex `HexState::temperature` is the real state.
    pub temp: f32,
    /// The planet's remaining finite outer-system **water budget** (element mass) — the R8a
    /// control input, rationed out over ticks. Not a target; how much becomes ocean vs
    /// atmosphere is emergent.
    water_budget: f64,
    /// Cumulative water mass **delivered** from that budget — external matter, so the column
    /// conservation invariant is `seed + delivered`.
    pub delivered: f64,
}

impl World {
    /// The outer-system water still to be delivered (element mass). With `delivered` it recovers
    /// the planet's original water endowment (the reference the habitability axes read against).
    pub fn water_budget(&self) -> f64 {
        self.water_budget
    }
}

/// One tick: ration this tick's water delivery from the finite budget, then run the **process
/// pipeline** over every hex (compute all effects, apply all effects — see
/// [`flicker_worldgen::run_tick`]). The per-hex temperatures the pipeline sets are then meaned
/// for the HUD. Nothing here schedules a layer — the processes fire on each hex's material state.
fn tick_world(world: &mut World, ctx: &EpochCtx, procs: &[Box<dyn Process>]) {
    // Water delivery is open while the planet is still warm enough to be receiving/holding a
    // veneer — a flux from the depleting budget, not a designed amount.
    let deliver_total = if world.temp <= T_WATER_DELIVERY {
        (world.water_budget * WATER_DELIVERY_RATE).min(world.water_budget)
    } else {
        0.0
    };
    world.water_budget -= deliver_total;

    let delivered = run_tick(
        &mut world.cells,
        ctx.neighbors,
        ctx.tables,
        procs,
        deliver_total,
    );
    world.delivered += delivered;

    if !world.cells.is_empty() {
        world.temp =
            world.cells.iter().map(|c| c.temperature).sum::<f32>() / world.cells.len() as f32;
    }
    world.tick += 1;
}

/// The **tick simulation**: the Epoch-1 seed ticked forward under Epoch-2 convection, with
/// deterministic re-sim + interval checkpoints so the viewer can play / reset.
pub struct Simulation {
    tables: Tables,
    sphere: Sphere,
    /// Per-cell boundary polygons (built with the sphere) — the viewer's mesh needs them.
    outlines: Vec<Vec<Vec3>>,
    base_seed: u64,
    /// Cached worlds by tick (the molten seed at 0, plus interval + requested ticks).
    checkpoints: BTreeMap<u64, World>,
}

impl Simulation {
    /// Build a sim from an explicit vocabulary + levers + recipe: the topology and the molten
    /// seed (tick 0, the Epoch-1 composition scatter) are built now.
    pub fn new(tables: Tables, params: GeneratorParams, mut config: WorldConfig) -> Self {
        config.freq = config.clamped_freq();
        let (sphere, outlines) = icosphere_with_outlines(config.freq);
        let e1 = Epoch1::new(&tables, build_epoch1(&config, &params), config.seed);
        // Seed every hex molten (the magma-ocean temperature) — the process pipeline evolves it.
        let cells: Vec<HexState> = sphere
            .dirs
            .iter()
            .map(|&d| {
                let mut h = HexState::new(e1.seed_hex(d));
                h.temperature = T_MOLTEN;
                h
            })
            .collect();
        let seed_mass: f64 = cells.iter().map(|c| c.column.total_mass()).sum();
        let water_budget = WATER_BUDGET_FRAC * seed_mass;
        let mut checkpoints = BTreeMap::new();
        checkpoints.insert(
            0,
            World {
                cells,
                tick: 0,
                temp: T_MOLTEN,
                water_budget,
                delivered: 0.0,
            },
        );
        Self {
            tables,
            sphere,
            outlines,
            base_seed: config.seed,
            checkpoints,
        }
    }

    /// Build from the repo's `Alpha/content/data` at the Earth-like defaults.
    pub fn from_repo() -> Result<Self, GeneratorError> {
        Self::from_repo_seeded(crate::config::DEFAULT_FREQ, crate::config::DEFAULT_SEED)
    }

    /// Build from the repo content at a given grid `freq` + world `seed`.
    pub fn from_repo_seeded(freq: u32, seed: u64) -> Result<Self, GeneratorError> {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        let tables = Tables::from_source(&JsonTableSource::new(dir))
            .expect("repo Alpha/content/data material tables load");
        let params = GeneratorParams::from_dir(dir)?;
        let mut config = WorldConfig::from_params(&params);
        config.freq = freq;
        config.seed = seed;
        Ok(Self::new(tables, params, config))
    }

    /// The world at `tick`, deterministically re-simmed from the nearest cached checkpoint
    /// ≤ `tick`. Caches the requested tick plus every [`CHECKPOINT_INTERVAL`]-th tick along
    /// the way, so a later reset / scrub re-sims only a short span.
    pub fn state_at(&mut self, tick: u64) -> &World {
        if !self.checkpoints.contains_key(&tick) {
            let start = *self
                .checkpoints
                .range(..=tick)
                .next_back()
                .expect("the tick-0 checkpoint is always present")
                .0;
            let mut world = self.checkpoints[&start].clone();
            let mut fresh: Vec<(u64, World)> = Vec::new();
            {
                let ctx = EpochCtx {
                    tables: &self.tables,
                    dirs: &self.sphere.dirs,
                    neighbors: &self.sphere.neighbors,
                    seed: self.base_seed,
                };
                let procs = processes(); // the process collection, rebuilt per re-sim span
                while world.tick < tick {
                    tick_world(&mut world, &ctx, &procs);
                    if world.tick.is_multiple_of(CHECKPOINT_INTERVAL) {
                        fresh.push((world.tick, world.clone()));
                    }
                }
            }
            for (t, w) in fresh {
                self.checkpoints.entry(t).or_insert(w);
            }
            self.checkpoints.insert(tick, world);
        }
        &self.checkpoints[&tick]
    }

    /// The already-computed world at `tick` (call after [`state_at`](Self::state_at) /
    /// [`ensure`](Self::ensure) has cached it).
    pub fn world(&self, tick: u64) -> Option<&World> {
        self.checkpoints.get(&tick)
    }

    /// Compute + cache `tick` without holding the returned borrow.
    pub fn ensure(&mut self, tick: u64) {
        self.state_at(tick);
    }

    pub fn sphere(&self) -> &Sphere {
        &self.sphere
    }
    pub fn outlines(&self) -> &[Vec<Vec3>] {
        &self.outlines
    }
    pub fn tables(&self) -> &Tables {
        &self.tables
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim() -> Simulation {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        let tables = Tables::from_source(&JsonTableSource::new(dir)).expect("tables load");
        let params = GeneratorParams::from_dir(dir).expect("params load");
        let mut config = WorldConfig::from_params(&params);
        config.freq = 6; // a cheap topology (~362 cells)
        Simulation::new(tables, params, config)
    }

    /// INVARIANT (not an outcome): the process pipeline conserves element mass across many
    /// ticks — the whole column mass only ever grows by the water delivered from outside.
    #[test]
    fn the_pipeline_conserves_mass_modulo_delivery() {
        let mut s = sim();
        let col_mass = |w: &World| -> f64 { w.cells.iter().map(|c| c.column.total_mass()).sum() };
        let seed = col_mass(s.state_at(0));
        let w = s.state_at(50).clone();
        let total = col_mass(&w);
        assert!(
            (total - (seed + w.delivered)).abs() <= 1e-6 * (seed + w.delivered),
            "column mass = seed + delivered ({seed} + {} vs {total})",
            w.delivered
        );
    }

    /// INVARIANT: history is a pure function of the seed — re-simming lands on the same world.
    #[test]
    fn re_sim_is_deterministic_and_resets_cleanly() {
        // A fresh run to tick 32 vs. a run that ran ahead to 64 then re-simmed 32 (a reset
        // back through 0 also lands on the identical world).
        let mut a = sim();
        let wa = a.state_at(32).cells.clone();
        let mut b = sim();
        b.state_at(64);
        assert_eq!(b.state_at(32).cells, wa, "re-sim to the same tick diverged");
        assert_eq!(b.state_at(0).tick, 0, "reset to the Epoch-1 seed");
        assert_eq!(b.state_at(32).cells, wa, "re-run after reset diverged");
    }
}
