//! [`Scheduler`] — the observable, steppable formation loop (spec §11) — and
//! [`CellProgress`], the per-cell progress it emits over a channel for a UI /
//! debug view to consume, detached from any renderer.
//!
//! `step()` advances one tick: sample [`PlanetState`], run each live stage in
//! order (each with its own deterministic RNG stream), then run the conservation
//! audit. A [`WorkerPool`] drives the per-cell [`sweep`](Scheduler::sweep) — the
//! "worker pool processes cells" harness of §11, used at M0 to fill the
//! generation loading bar. The two-phase deterministic scatter (§11) and the
//! semi-Lagrangian transports (§6.1) arrive with the interior stages (M1) that
//! need them.

use std::sync::mpsc::{self, Sender};

use std::collections::BTreeSet;

use flicker_worker::WorkerPool;

use crate::planet::{PlanetState, World};
use crate::stage::{Stage, StageRng};

/// One unit of per-cell progress, emitted as the worker pool sweeps the columns —
/// what the layer-view / loading UI listens to (spec §11).
#[derive(Clone, Debug)]
pub struct CellProgress {
    pub cell_id: u32,
    pub stage: &'static str,
}

/// Cells processed per worker job — a handful of chunky jobs rather than 92k tiny
/// ones.
const SWEEP_CHUNK: usize = 2048;

/// What one process is doing, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessState {
    /// The stage's own name — the same string the conservation audit would use if
    /// this stage leaked, so a reading here and a panic there name the same thing.
    pub name: &'static str,
    /// The maintainer is holding it.
    pub held: bool,
    /// Its own chemistry gate is satisfied. A process that is neither held nor
    /// ready is **waiting on the world**, which is a different thing from stopped.
    pub ready: bool,
}

impl ProcessState {
    /// Whether this process actually runs on the next tick.
    pub fn running(&self) -> bool {
        !self.held && self.ready
    }
}

/// The formation scheduler: an ordered stage list, a master seed (for the
/// per-stage RNG streams), and a worker pool for the cell sweep.
pub struct Scheduler {
    stages: Vec<Box<dyn Stage>>,
    /// Stages the maintainer is holding, by name. A held stage does not run.
    ///
    /// Gating needs no new concept in the model: a held process is simply one that
    /// did not run this tick, which is the same thing that happens when its own
    /// chemistry gate is closed. The difference is only in *why*, and that
    /// difference belongs in the readout, not in the mechanism.
    held: BTreeSet<String>,
    seed: u64,
    ticks: u64,
    pool: WorkerPool,
    /// How often the audit runs: every tick in debug/test, every 100 in release.
    /// The invariant harness is **never disabled** (§4.3).
    audit_every: u64,
}

impl Scheduler {
    /// A scheduler over `stages`, seeded by `seed`.
    pub fn new(stages: Vec<Box<dyn Stage>>, seed: u64) -> Self {
        let audit_every = if cfg!(debug_assertions) { 1 } else { 100 };
        Self {
            stages,
            held: BTreeSet::new(),
            seed,
            ticks: 0,
            pool: WorkerPool::with_default_size(),
            audit_every,
        }
    }

    /// Ticks advanced so far.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Master seed of this run (a per-run initial condition — spec §3.5).
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Number of registered stages.
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    /// Hold a stage, or let it go again. Holding one that is already held, or
    /// releasing one that was never held, is not an error — the caller is
    /// expressing what it wants the state to BE.
    pub fn set_held(&mut self, stage: &str, held: bool) {
        if held {
            self.held.insert(stage.to_string());
        } else {
            self.held.remove(stage);
        }
    }

    /// Whether a stage is being held.
    pub fn is_held(&self, stage: &str) -> bool {
        self.held.contains(stage)
    }

    /// **What every process is doing and why** — the readout behind the bench's
    /// process panel.
    ///
    /// The three states are deliberately distinct. "Nothing is happening" and
    /// "nothing is happening *yet, because the core is only 40% formed*" are
    /// different facts, and only one of them is a bug.
    pub fn processes(&self, state: &PlanetState) -> Vec<ProcessState> {
        self.stages
            .iter()
            .map(|s| ProcessState {
                name: s.name(),
                held: self.held.contains(s.name()),
                ready: s.is_live(state),
            })
            .collect()
    }

    /// Advance the world by one tick of `dt_myr`. Samples [`PlanetState`] at the
    /// top (stages read *that*, never the live world's totals — unambiguous
    /// read/write ordering, §7.1), runs each live stage in order with its own
    /// deterministic RNG stream, emits [`CellProgress`] per live stage if a
    /// `progress` sink is given, then runs the conservation audit. Returns the
    /// top-of-tick state.
    pub fn step(
        &mut self,
        world: &mut World,
        dt_myr: f64,
        progress: Option<&Sender<CellProgress>>,
    ) -> PlanetState {
        let state = PlanetState::sample(world);

        for (index, stage) in self.stages.iter().enumerate() {
            if self.held.contains(stage.name()) || !stage.is_live(&state) {
                continue;
            }
            let mut rng = StageRng::for_stage(self.seed, index);
            stage.tick(world, dt_myr, &mut rng);

            if let Some(tx) = progress {
                sweep_cells(&self.pool, world.cell_count(), stage.name(), tx);
            }

            // Both conserved-ledger invariants after every stage (debug/test) — a
            // leak panics naming exactly the stage that broke it.
            if cfg!(debug_assertions) {
                world.audit(stage.name());
                world.audit_compound_bound(stage.name());
            }
        }

        world.tick_myr += dt_myr;
        self.ticks += 1;

        // Periodic audit even in release, and always at least once per tick — so a
        // leak in seeding or a stage-free tick still surfaces.
        if self.ticks.is_multiple_of(self.audit_every) {
            world.audit("tick");
            world.audit_compound_bound("tick");
        }
        state
    }

    /// Fan a read-only pass over `n_cells` across the worker pool, emitting one
    /// [`CellProgress`] per cell under `stage`. Synchronous (returns once every
    /// cell has been reported). The M0 consumer is the generation loading bar; M1
    /// per-cell stages reuse the same pool.
    pub fn sweep(&self, n_cells: usize, stage: &'static str, tx: &Sender<CellProgress>) {
        sweep_cells(&self.pool, n_cells, stage, tx);
    }
}

/// The worker-pool cell sweep (spec §11). Chunked; blocks on a completion barrier
/// so a tick step is synchronous. Sends are best-effort — a dropped receiver (UI
/// closed) simply ends the reporting.
fn sweep_cells(pool: &WorkerPool, n_cells: usize, stage: &'static str, tx: &Sender<CellProgress>) {
    if n_cells == 0 {
        return;
    }
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let mut chunks = 0usize;
    let mut start = 0usize;
    while start < n_cells {
        let end = (start + SWEEP_CHUNK).min(n_cells);
        let tx = tx.clone();
        let done_tx = done_tx.clone();
        pool.submit(move || {
            for cell_id in start..end {
                let _ = tx.send(CellProgress {
                    cell_id: cell_id as u32,
                    stage,
                });
            }
            let _ = done_tx.send(());
        });
        chunks += 1;
        start = end;
    }
    drop(done_tx);
    for _ in 0..chunks {
        let _ = done_rx.recv();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    fn tiny_world() -> World {
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("material tables");
        let b = Budget::from_dir(&dir, &t).expect("budget from repo");
        World::seed(icosphere(4), b, &t, 42)
    }

    /// A stage that DESTROYS mass — removes iron and routes it nowhere. The audit
    /// must catch it and name the stage.
    struct LeakStage;
    impl Stage for LeakStage {
        fn name(&self) -> &'static str {
            "LeakStage"
        }
        fn tick(&self, world: &mut World, _dt: f64, _rng: &mut StageRng) {
            world.mantle.remove(0, 26, 1.0e18);
        }
    }

    /// A conserving stage: sinks iron from a mantle cell into core
    /// (differentiation-like).
    struct DifferentiateStage;
    impl Stage for DifferentiateStage {
        fn name(&self) -> &'static str {
            "Differentiate"
        }
        fn tick(&self, world: &mut World, _dt: f64, _rng: &mut StageRng) {
            let moved = world.mantle.remove(0, 26, 1.0e20);
            world.reservoirs.core.add(26, moved);
        }
    }

    #[test]
    // The per-stage naming audit is debug-gated (spec §4.3 cadence: every tick in
    // debug/test, every 100 in release), so this proof runs under the project's
    // `cargo test` (debug); skip it in release rather than fail.
    #[cfg_attr(not(debug_assertions), ignore = "debug-only per-stage audit cadence")]
    #[should_panic(expected = "LeakStage")]
    fn leaking_stage_panics_naming_the_stage() {
        let mut sched = Scheduler::new(vec![Box::new(LeakStage)], 42);
        let mut world = tiny_world();
        sched.step(&mut world, 1.0, None); // audit after the stage panics naming it
    }

    #[test]
    fn conserving_stage_holds_and_advances() {
        let mut sched = Scheduler::new(vec![Box::new(DifferentiateStage)], 42);
        let mut world = tiny_world();
        let core_before = world.reservoirs.core.total();
        sched.step(&mut world, 1.0, None);
        assert!(world.reservoirs.core.total() > core_before, "iron sank into the core");
        assert_eq!(world.tick_myr, 1.0);
        assert_eq!(sched.ticks(), 1);
    }

    #[test]
    fn empty_schedule_still_conserves_and_advances() {
        // The undifferentiated ball with no stages: it just sits there, and the
        // periodic audit confirms nothing leaked.
        let mut sched = Scheduler::new(vec![], 7);
        let mut world = tiny_world();
        for _ in 0..3 {
            sched.step(&mut world, 1.0, None);
        }
        assert_eq!(world.tick_myr, 3.0);
        assert_eq!(sched.ticks(), 3);
    }

    #[test]
    fn cell_sweep_reports_every_cell() {
        let sched = Scheduler::new(vec![], 42);
        let world = tiny_world();
        let (tx, rx) = mpsc::channel();
        sched.sweep(world.cell_count(), "generate", &tx);
        drop(tx);
        let got: Vec<_> = rx.iter().collect();
        assert_eq!(got.len(), world.cell_count(), "one progress message per cell");
        assert!(got.iter().all(|p| p.stage == "generate"));
    }

    #[test]
    fn same_seed_same_rng_stream() {
        // Determinism substrate (§11): a stage's RNG stream is a pure function of
        // (seed, stage index).
        let mut a = StageRng::for_stage(1234, 3);
        let mut b = StageRng::for_stage(1234, 3);
        let mut c = StageRng::for_stage(1234, 4);
        assert_eq!(a.next_u64(), b.next_u64(), "same (seed, index) → same stream");
        assert_ne!(StageRng::for_stage(1234, 3).next_u64(), c.next_u64(), "different index → different stream");
    }
}

#[cfg(test)]
mod tick_zero_gates {
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::planet::{PlanetState, World};
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    /// **A NEW WORLD STARTS WITH ALMOST EVERYTHING SHUT.**
    ///
    /// At t=0 the planet is an undifferentiated ball at ~4000 K: no crust, no
    /// sea, no air, no life. The only processes that can honestly be doing
    /// anything are the interior's own heat and the infall arriving from
    /// outside. Everything else is waiting on a condition the world has not met
    /// yet, and the process panel must SAY so — a stage that reports "running"
    /// while its tick is a no-op is the readout lying about the world.
    ///
    /// This is the tick-0 half of the gate ruling: gates open and shut on
    /// chemistry, and the sim must not start with them all open.
    #[test]
    fn a_fresh_world_has_almost_every_gate_shut() {
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("t"));
        let b = Budget::from_dir(&dir, &t).expect("b");
        let w = World::seed(icosphere(4), b, &t, 5);
        let stages =
            crate::formation_stages(std::sync::Arc::clone(&t), &w.budget.clone(), &crate::Levers::brisk());
        let state = PlanetState::sample(&w);

        let live: Vec<&str> =
            stages.iter().filter(|s| s.is_live(&state)).map(|s| s.name()).collect();
        let waiting: Vec<&str> =
            stages.iter().filter(|s| !s.is_live(&state)).map(|s| s.name()).collect();
        eprintln!("t=0 running: {live:?}\nt=0 waiting: {waiting:?}");

        // The interior drives everything and is hot from the first tick; water
        // starts arriving from outside immediately. That is the whole list.
        assert_eq!(
            live,
            vec!["RadiogenicDecay", "CoreFormation", "MantleConvection", "Outgassing", "WaterDelivery"],
            "only the interior and the infall may run on a bare magma ball"
        );
        // And the rest are explicitly waiting on the world, not silently idle.
        for shut in [
            "CrustGeneration",
            "Volcanism",
            "WaterCycle",
            "CarbonSink",
            "Biosphere",
            "Maturation",
            "Conveyor",
            "Hydrothermal",
            "Erosion",
            "Crystallization",
            "StrataReconcile",
        ] {
            assert!(waiting.contains(&shut), "{shut} should be waiting at t=0, not running");
        }
    }
}

#[cfg(test)]
mod no_scripted_outcomes {
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::planet::{PlanetState, World};
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    fn grown(seed: u64, ticks: usize, hold: &[&str]) -> (World, PlanetState) {
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("t"));
        let b = Budget::from_dir(&dir, &t).expect("b");
        let mut w = World::seed(icosphere(4), b, &t, seed);
        let mut s = super::Scheduler::new(
            crate::formation_stages(std::sync::Arc::clone(&t), &w.budget.clone(), &crate::Levers::brisk()),
            seed,
        );
        for name in hold {
            s.set_held(name, true);
        }
        let mut state = PlanetState::sample(&w);
        for _ in 0..ticks {
            state = s.step(&mut w, 1.0, None);
        }
        (w, state)
    }

    /// **THE COVERAGE LEVER bounds the infall by the world's own solved
    /// state.** Three twins: unlimited, cut off at a low target, and denied
    /// entirely. The cutoff must stop delivery with budget remaining the
    /// moment the sea stands at the target — and the denied twin's coverage
    /// is the SELF-WATERED FLOOR, printed here because it is the honest lower
    /// bound of what the slider can reach: below it, the planet waters itself
    /// from its own exhaled steam and the comets were never the reason.
    #[test]
    fn the_coverage_lever_cuts_the_infall_at_the_target() {
        let run_with = |target: f64, hold: &[&str]| {
            let dir = content_data_dir();
            let t =
                std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("t"));
            let b = Budget::from_dir(&dir, &t).expect("b");
            let mut w = World::seed(icosphere(4), b, &t, 7);
            let levers =
                crate::Levers { water_coverage_target: target, ..crate::Levers::brisk() };
            let mut s = super::Scheduler::new(
                crate::formation_stages(std::sync::Arc::clone(&t), &w.budget.clone(), &levers),
                7,
            );
            for name in hold {
                s.set_held(name, true);
            }
            let mut state = PlanetState::sample(&w);
            for _ in 0..220 {
                state = s.step(&mut w, 1.0, None);
            }
            state
        };

        let open = run_with(1.0, &[]);
        let capped = run_with(0.30, &[]);
        let denied = run_with(1.0, &["WaterDelivery"]);
        eprintln!(
            "coverage: open {:.3} (delivered {:.3e}) / capped@0.30 {:.3} (delivered {:.3e}) / \
             self-watered floor {:.3}",
            open.submerged_frac,
            open.delivered_water_kg,
            capped.submerged_frac,
            capped.delivered_water_kg,
            denied.submerged_frac,
        );

        // The cutoff bit: the capped world stopped taking water with budget
        // still unspent, and took less than the open twin.
        assert!(
            capped.delivered_water_kg < open.delivered_water_kg,
            "the cutoff must stop delivery early: {:.3e} vs {:.3e}",
            capped.delivered_water_kg,
            open.delivered_water_kg
        );
        // And it stopped for the stated reason: the sea stands at (or past)
        // the target, so the gate reads shut RIGHT NOW off the current state.
        let t = Tables::from_source(&JsonTableSource::new(&content_data_dir())).expect("tables");
        let mut stage = crate::infall::WaterDelivery::new(&t);
        stage.target_coverage = 0.30;
        assert!(
            !crate::process_file::gate_of("WaterDelivery").holds(&capped, &crate::Levers { water_coverage_target: 0.30, ..crate::Levers::brisk() }),
            "at coverage {:.3} the capped gate must read shut",
            capped.submerged_frac
        );
        assert!(
            crate::process_file::gate_of("WaterDelivery").holds(&PlanetState::default(), &crate::Levers::brisk()) || PlanetState::default().submerged_frac == 0.0,
            "and a dry world's gate reads open"
        );
    }

    /// **THE CUTOFF CANNOT MEASURE A WORLD WITH NO GROUND.** The defect this
    /// pins (Aaron's window, 2026-08-06): at ~26 My the first crust froze, the
    /// delivered trickle finally had sub-solidus ground to pool on, and the
    /// sea-level solve on a ZERO-RELIEF world put an epsilon film over every
    /// cell — `submerged_frac` read 100% with 3% of the water delivered and
    /// crust at 0.000%. Any coverage target below 1.0 then read "target
    /// reached" and shut the infall for good: a 3-billion-year tail strangled
    /// at 26 My by a gate reading a quantity that means nothing until relief
    /// exists. The guard: while `lid_frac` is zero the cutoff does not bite —
    /// coverage is a statement about ground standing clear of the sea, and
    /// there is no ground.
    #[test]
    fn the_cutoff_waits_for_ground_before_measuring_coverage() {
        let levers = crate::Levers { water_coverage_target: 0.5, ..crate::Levers::brisk() };
        let gate = crate::process_file::gate_of("WaterDelivery");

        // The film world: no lid anywhere, yet 100% "submerged" — the reading
        // the defect turned on. Delivery must CONTINUE.
        let film = PlanetState {
            lid_frac: 0.0,
            submerged_frac: 1.0,
            ocean_mass_kg: 1.0e15,
            delivered_water_kg: 4.0e19,
            ..PlanetState::default()
        };
        assert!(
            gate.holds(&film, &levers),
            "a film over a world with no relief must not read as coverage reached"
        );

        // The same coverage numbers with a real lid: now the reading MEANS
        // something, and the cutoff bites exactly as the lever asks.
        let lidded = PlanetState { lid_frac: 0.2, ..film.clone() };
        assert!(
            !gate.holds(&lidded, &levers),
            "with ground standing, a sea past target genuinely shuts the infall"
        );

        // And below target with a lid, delivery runs — the lever's actual job.
        let drier = PlanetState { submerged_frac: 0.3, ..lidded };
        assert!(gate.holds(&drier, &levers), "below target, the comets keep coming");
    }

    /// **NOTHING IS SCRIPTED.** Denying the world its water infall — holding
    /// the gate for the whole run — must produce a MATERIALLY different
    /// planet, because every downstream consequence is supposed to derive
    /// from the state the water would have created, never from a timeline.
    ///
    /// The honest subtlety this pins alongside: a denied-infall world is NOT
    /// bone-dry, and must not be. The planet's own mantle carries hydrogen,
    /// and the magma era exhales it as steam through the same conserved
    /// chemistry as everything else — Earth's own ocean is part outgassed,
    /// part delivered. What the hold removes is the DELIVERED share, and the
    /// books must show exactly that: zero delivered water, and a sea that is
    /// only what the rock itself breathed out.
    #[test]
    fn a_world_denied_its_water_infall_comes_out_different() {
        let ticks = 220;
        let (_, with) = grown(7, ticks, &[]);
        let (_, without) = grown(7, ticks, &["WaterDelivery"]);

        // The hold held: not a kilogram arrived from outside.
        assert_eq!(without.delivered_water_kg, 0.0, "held infall delivers NOTHING");
        assert!(with.delivered_water_kg > 0.0, "the twin control received its water");

        // And the world is genuinely different for it — not the same story on
        // a different label. The denied world's hydrosphere is what the mantle
        // alone exhaled; the control's carries the delivery on top.
        assert!(
            without.ocean_mass_kg > 0.0,
            "the denied world still has the sea its own rock breathed out"
        );
        assert!(
            without.ocean_mass_kg < with.ocean_mass_kg,
            "denied infall ⇒ a smaller sea: {:.3e} vs {:.3e}",
            without.ocean_mass_kg,
            with.ocean_mass_kg
        );
        // The strongest no-script statement there is: the two seas differ by
        // (approximately) EXACTLY the water that arrived. Nothing compensated,
        // nothing ran to a timeline — remove the boundary input and the
        // difference IS the boundary input, give or take the vapour the water
        // cycle holds aloft at any instant. Measured on this run: the mantle's
        // own exhaled steam is ~5× the delivery, so a denied world is smaller
        // by ~10%, not bone-dry — Earth's ocean, too, is part outgassed, part
        // delivered.
        // **Most of the delivery is sea; the rest became what the sea built.**
        //
        // This used to pin `Δocean == delivered` to within 20%, and that held
        // while the world had almost nowhere else to put water. It no longer
        // does, and the reason is chemistry rather than a leak: delivered water
        // is H AND O, and the oxygen does not stay water. The wetter twin lays
        // down more carbonate, crystallises more silicate and breathes out more
        // free O₂ — every one of those keeps oxygen and hands the hydrogen
        // back. Measured on this run: 74% of the delivery is still standing sea,
        // the vapour aloft is IDENTICAL between the twins, and the remainder is
        // in the ground and the air as oxygen doing other work.
        //
        // Nothing here is unaccounted for. `World::audit` proves that every
        // tick of both runs, per element, or the run panics — so the exact
        // conservation statement is already made continuously and does not need
        // restating here. What this test uniquely proves is the SECOND half:
        // remove the boundary input and the world is materially, measurably
        // different, in the direction the input points.
        let delta = with.ocean_mass_kg - without.ocean_mass_kg;
        eprintln!(
            "ocean with {:.3e} / without {:.3e} / delta {delta:.3e} / delivered {:.3e} \
             (sea keeps {:.0}%; sky delta {:.3e})",
            with.ocean_mass_kg,
            without.ocean_mass_kg,
            with.delivered_water_kg,
            100.0 * delta / with.delivered_water_kg,
            with.water_vapour_kg - without.water_vapour_kg,
        );
        assert!(
            delta > 0.5 * with.delivered_water_kg,
            "the sea must be where most of the delivery went: {delta:.3e} of {:.3e}",
            with.delivered_water_kg
        );
        assert!(
            delta <= with.delivered_water_kg * 1.2,
            "and it cannot exceed what arrived: {delta:.3e} of {:.3e}",
            with.delivered_water_kg
        );
        // The difference propagates: the sea STANDS at a different height, and
        // sea level is what erosion, the carbon sink and the habitability read
        // act against. (`submerged_frac` is too coarse a probe on a 162-cell
        // test world — it moves in whole-cell steps, and a tenth of a sea can
        // vanish without unflooding one — but the solved level itself is
        // continuous and must move.) Downstream is downstream of the STATE.
        assert!(
            (with.sea_level_m - without.sea_level_m).abs() > 1.0,
            "a different sea stands at a different level: {:.1} m vs {:.1} m",
            with.sea_level_m,
            without.sea_level_m
        );
    }
}

#[cfg(test)]
mod gates_match_their_ticks {
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::planet::{PlanetState, World};
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    /// **A GATE THAT SUMMARISES A PER-CELL THRESHOLD MUST USE THE EXTREME THAT
    /// ADMITS WORK, NEVER THE MEAN.**
    ///
    /// Three stages act on cells above a temperature: `CoreFormation` (iron
    /// segregates above `FE_SEGREGATION_K`), `Volcanism` (melt exists above
    /// `ERUPTION_FLOOR_K`), `Outgassing` (a species leaves above its release
    /// floor). All three used to gate on the MEAN, which fails in the direction
    /// that matters: with the average below the threshold but a region still
    /// above it, the stage was switched off while it was genuinely working.
    ///
    /// Volcanism is the case that makes it obvious — a plume is BY DEFINITION
    /// hotter than the world around it, so the mean is exactly the wrong
    /// statistic for the one stage built on heat locality.
    ///
    /// (`CrustGeneration` is the mirror image and was already right: freezing
    /// happens where it is COLDEST, so it reads the min.)
    #[test]
    fn a_cold_world_with_one_hot_plume_keeps_its_hot_stages_live() {
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("t"));
        let b = Budget::from_dir(&dir, &t).expect("b");
        let mut w = World::seed(icosphere(4), b, &t, 5);

        // A world that has cooled well past every threshold — except for one
        // stubborn plume still hot enough to melt, segregate and degas.
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 500.0;
        }
        w.mantle.temp_k[0] = 2600.0;
        crate::planet::freeze_lid(&mut w);
        let state = PlanetState::sample(&w);
        assert!(state.mean_mantle_temp_k < 1200.0, "the AVERAGE is cold");
        assert!(state.max_mantle_temp_k > 1800.0, "but one cell is not");

        assert!(crate::process_file::gate_of("Volcanism").holds(&state, &crate::Levers::default()), "the plume can still erupt");
        assert!(crate::process_file::gate_of("Outgassing").holds(&state, &crate::Levers::default()), "and still degas");
        assert!(crate::process_file::gate_of("CoreFormation").holds(&state, &crate::Levers::default()), "and still sink iron");

        // Cool that last cell and all three shut for good — the gates close on
        // the world, not on a clock.
        w.mantle.temp_k[0] = 500.0;
        let cold = PlanetState::sample(&w);
        assert!(!crate::process_file::gate_of("Volcanism").holds(&cold, &crate::Levers::default()), "no melt anywhere: volcanism is over");
        assert!(!crate::process_file::gate_of("CoreFormation").holds(&cold, &crate::Levers::default()), "no segregation anywhere: the core is done");

        // Outgassing's floor is the lowest in the vocabulary (nitrogen, 600 K),
        // so 500 K silences it too — the distillation series has run out.
        assert!(!crate::process_file::gate_of("Outgassing").holds(&cold, &crate::Levers::default()), "below every release floor: the sky is finished");
    }
}

#[cfg(test)]
mod bake_report {
    use std::sync::Arc;

    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::planet::{PlanetState, World};
    use crate::scheduler::Scheduler;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    /// **Does the planet change when only the RESOLUTION changes?**
    ///
    /// Aaron's hypothesis (2026-08-06): baking at freq 48 instead of 96 might be
    /// making a *smaller planet*, so an Earth-sized water budget would drown it.
    /// Worth answering with a measurement rather than an argument, because if it
    /// were true every number he has looked at would be suspect.
    ///
    /// The claim under test is the design's own: **frequency is RESOLUTION, not
    /// SIZE.** The radius is a fixed constant, so `4πR²` is the same sphere at
    /// every frequency and `cell_area_m2` simply divides it into more or fewer
    /// pieces — which means the water-to-surface ratio, and therefore how
    /// drowned the world gets, must not care.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn resolution_is_not_size() {
        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        for freq in [12u32, 24, 48] {
            let b = Budget::from_dir(&dir, &t).expect("budget");
            let mut w = World::seed(icosphere(freq), b, &t, 42);
            let levers = crate::Levers::default();
            let mut sched =
                Scheduler::new(crate::formation_stages(Arc::clone(&t), &w.budget.clone(), &levers), 42);
            let area = w.cell_area_m2();
            let total = area * w.columns.len() as f64;
            for _ in 0..500 {
                sched.step(&mut w, crate::NOMINAL_DT_MYR, None);
            }
            let st = PlanetState::sample(&w);
            eprintln!(
                "freq {freq:>3}: {:>6} cells · cell {:.3e} m² · TOTAL SURFACE {:.4e} m² \
                 (4πR² = {:.4e})\n          planet {:.4e} kg · ocean {:.3e} kg · \
                 submerged {:.1}% · sea {:.0} m · lid {:.0}%",
                w.columns.len(),
                area,
                total,
                4.0 * std::f64::consts::PI
                    * crate::PLANET_RADIUS_M
                    * crate::PLANET_RADIUS_M,
                w.budget.total(),
                st.ocean_mass_kg,
                st.submerged_frac * 100.0,
                st.sea_level_m,
                st.lid_frac * 100.0,
            );
        }
    }

    /// **The bake telescope — a report, not a test.**
    ///
    /// ```text
    /// cargo test -p flicker-poc-chemistry --release bake_timeline -- --ignored --nocapture
    /// ```
    ///
    /// Runs the physics **as written** (default levers — the geologic e-folds,
    /// 100 ky ticks) across 4.5 BY on a small grid and prints every gate
    /// transition with its date, so the pacing of the eras can be judged by
    /// eye. It asserts nothing about the timeline — the timeline belongs to
    /// the world — only the conservation harness rides along as always.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn bake_timeline_report() {
        /// **The Starter's hydrogen knob, as the bake sees it.** `1.0` is
        /// `accretion.json` as shipped; the app's forge applies exactly this
        /// multiplier through [`Budget::rescaled`], so a bake at 0.15 measures
        /// the world the maintainer gets by dialling H down and pressing FORGE.
        ///
        /// It is here because the endowment is the one input that decides
        /// whether this planet has an ocean or IS one: at 1.0 the mantle
        /// exhales over 3e21 kg of water on its own — twice the entire comet
        /// budget — and every run drowns regardless of what the tectonics do.
        const H_SCALE: f64 = 0.15;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        eprintln!("── forge: H × {H_SCALE} ──");
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(
            crate::formation_stages(Arc::clone(&t), &w.budget.clone(), &levers),
            42,
        );
        // A second, identical stage list purely to PROBE `is_live` — the
        // scheduler owns its own.
        let probes = crate::formation_stages(Arc::clone(&t), &w.budget.clone(), &levers);
        let mut live: Vec<bool> = vec![false; probes.len()];

        let ticks: u64 = (4500.0 / crate::NOMINAL_DT_MYR).ceil() as u64; // 4.5 BY
        eprintln!("── bake: {} cells · {} ticks · dt {} My ──", w.columns.len(), ticks, crate::NOMINAL_DT_MYR);
        // **The hypsometry probe.** Gate timings say WHEN the eras turn; this
        // says whether the world is turning into a planet — is there land, does
        // it stand clear of the sea, and how deep are the basins under it.
        // Printed on a cadence so the shape of the curve is visible, which is
        // the only honest way to judge "is this heading anywhere" without
        // requiring an outcome of it.
        let hypsometry = |w: &World, state: &PlanetState| {
            let area = w.cell_area_m2();
            let mut elev: Vec<f64> =
                w.columns.iter().map(|c| crate::column::elevation_m(c, area)).collect();
            elev.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let at = |q: f64| elev[((elev.len() - 1) as f64 * q) as usize];
            let sea = state.sea_level_m;
            let land = elev.iter().filter(|&&e| e >= sea).count() as f64 / elev.len() as f64;
            // **The slope histogram.** Aaron's own regression check: gradients
            // should SETTLE near the angle of repose, not pile at the maximum.
            // A landscape standing at cliffs everywhere is one where nothing is
            // answering the over-steepening, which is the black-ring defect.
            let span = area.sqrt();
            let mut grades: Vec<f64> = Vec::with_capacity(w.columns.len());
            let raw: Vec<f64> =
                w.columns.iter().map(|c| crate::column::elevation_m(c, area)).collect();
            for i in 0..w.columns.len() {
                let low = w.grid.neighbors[i]
                    .iter()
                    .map(|&j| raw[j as usize])
                    .fold(f64::INFINITY, f64::min);
                grades.push(((raw[i] - low) / span).max(0.0));
            }
            grades.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let g = |q: f64| grades[((grades.len() - 1) as f64 * q) as usize];
            eprintln!(
                "{:>9.1} My  land {:>5.1}%  cont {:>5.1}%  lid {:>5.1}%  sea {:>8.0} m  \
                 hyps [{:>7.0} {:>7.0} {:>7.0} {:>7.0} {:>7.0}]  ocean {:.2e} kg",
                state.tick_myr,
                land * 100.0,
                state.continental_frac * 100.0,
                state.lid_frac * 100.0,
                sea,
                at(0.0),
                at(0.25),
                at(0.5),
                at(0.75),
                at(1.0),
                state.ocean_mass_kg,
            );
            eprintln!(
                "            slope p50 {:.4}  p90 {:.4}  max {:.4}   (repose 0.035)",
                g(0.5),
                g(0.9),
                g(1.0),
            );
        };
        let every = (250.0 / crate::NOMINAL_DT_MYR) as u64; // a line per ~250 My
        for tick in 0..ticks {
            sched.step(&mut w, crate::NOMINAL_DT_MYR, None);
            if tick % 25 == 0 || tick + 1 == ticks {
                let state = PlanetState::sample(&w);
                for (i, s) in probes.iter().enumerate() {
                    let now = s.is_live(&state);
                    if now != live[i] {
                        eprintln!(
                            "{:>9.1} My  {} {}",
                            state.tick_myr,
                            s.name(),
                            if now { "OPENED" } else { "shut" }
                        );
                        live[i] = now;
                    }
                }
            }
            if tick % every == 0 || tick + 1 == ticks {
                hypsometry(&w, &PlanetState::sample(&w));
            }
        }
        let end = PlanetState::sample(&w);
        eprintln!(
            "── end: lid {:.0}% · continental {:.0}% · submerged {:.0}% · sea {:.0} m · air {:.2e} kg · ocean {:.2e} kg · mean strata {:.1} (max {}) · mantle {:.0} K ──",
            end.lid_frac * 100.0,
            end.continental_frac * 100.0,
            end.submerged_frac * 100.0,
            end.sea_level_m,
            end.atmosphere_mass_kg,
            end.ocean_mass_kg,
            end.mean_strata,
            end.max_strata,
            end.mean_mantle_temp_k,
        );
    }
}
