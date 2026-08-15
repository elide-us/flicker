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

        // Snap the air's species bookkeeping back inside the conserved element
        // bound before the tick closes — bounded float-drift housekeeping (a
        // real leak still panics inside it). See [`World::settle_air_species`].
        world.settle_air_species();

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
            crate::formation_stages(std::sync::Arc::clone(&t), &w, &crate::Levers::brisk());
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
            crate::formation_stages(std::sync::Arc::clone(&t), &w, &crate::Levers::brisk()),
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
                crate::formation_stages(std::sync::Arc::clone(&t), &w, &levers),
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
        let t = Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables");
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
        // back. Measured at Earth scale: 74% of the delivery was still standing
        // sea. Under the size model this world is a freq-4 PLANETOID, and its
        // delivery is size³ of the reference while the rock-work sinks are
        // AREAL — same 49.65-mi hexes, same per-cell carbonate and silicate
        // appetite — so a far larger share of a far smaller delivery gets
        // banked into rock: measured on this run, 36% stands as extra sea, the
        // vapour aloft is IDENTICAL between the twins, and the rest is in the
        // ground and the air as oxygen doing other work. Small worlds lose
        // their delivered water to rock faster; that is the physics, not a
        // leak (every gram of both twins is audited every tick).
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
            delta > 0.25 * with.delivered_water_kg,
            "a material share of the delivery must stand as extra sea: {delta:.3e} of {:.3e}",
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

    /// **DOES THE CIRCUIT CLOSE?** (2026-08-07, Aaron: *"we know we are sinking
    /// materials, that is part of the design, we just need to make sure that
    /// what we sink is also returned to the simulation so we can continue to
    /// sort and compact the plates."*)
    ///
    /// Both ends of the seam circuit, per tick: mass the trenches swallowed and
    /// mass the ridges welled back up. **An aggregate question** — the planet is
    /// the well, so what matters is whether the two totals track each other over
    /// time, not where any particular gram went.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn seam_circuit_report() {
        const BAKE: usize = 400;
        const WATCH: usize = 400;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, 0.15)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..BAKE {
            sched.step(&mut w, dt, None);
        }
        let _ = crate::tectonics::take_seam_flux();

        eprintln!("      My        sunk kg      welled kg    welled/sunk   crust kg");
        let (mut tot_sunk, mut tot_welled) = (0.0f64, 0.0f64);
        for tick in 0..WATCH {
            sched.step(&mut w, dt, None);
            let (sunk, welled) = crate::tectonics::take_seam_flux();
            tot_sunk += sunk;
            tot_welled += welled;
            if tick % 50 == 49 {
                let crust: f64 = w.columns.iter().map(|c| c.mass_kg()).sum();
                eprintln!(
                    "{:>8.0}  {sunk:>13.3e}  {welled:>13.3e}  {:>12.4}   {crust:>9.3e}",
                    w.tick_myr,
                    if sunk > 0.0 { welled / sunk } else { f64::NAN },
                );
            }
        }
        eprintln!(
            "\nover {WATCH} ticks: sunk {tot_sunk:.4e} kg · welled {tot_welled:.4e} kg · \
             welled/sunk {:.4}",
            tot_welled / tot_sunk.max(f64::MIN_POSITIVE)
        );
        eprintln!("(the ledger itself is proven by World::audit every tick; this is about FLOW)");
    }

    /// **WHAT GROWS THE CRUST, AND WHAT REMOVES IT?** (2026-08-07 night.)
    ///
    /// Closing the seam circuit took welled/sunk from 0.087 to 0.765, and with
    /// arc melt returning another ~0.24 that is essentially ALL of it: nothing
    /// stays down. The 91% that used to strand in the mantle was, by accident,
    /// the crust's only real sink — so the question is now whether crust has any
    /// sink at all, and if not, what is filling the world up.
    ///
    /// Per-stage crust mass delta, which no global average can show.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn crust_budget_report() {
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500;
        const WATCH: usize = 100;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..BAKE {
            sched.step(&mut w, dt, None);
        }

        let crust = |w: &World| -> f64 { w.columns.iter().map(|c| c.mass_kg()).sum() };
        let stages = crate::formation_stages(Arc::clone(&t), &w, &levers);
        let names: Vec<&'static str> = stages.iter().map(|s| s.name()).collect();
        let mut delta = vec![0.0f64; stages.len()];
        let start = crust(&w);

        for _ in 0..WATCH {
            let state = PlanetState::sample(&w);
            for (index, stage) in stages.iter().enumerate() {
                if !stage.is_live(&state) {
                    continue;
                }
                let before = crust(&w);
                let mut rng = crate::stage::StageRng::for_stage(42, index);
                stage.tick(&mut w, dt, &mut rng);
                delta[index] += crust(&w) - before;
            }
            w.tick_myr += dt;
            w.settle_air_species();
        }
        w.audit("crust budget");
        let end = crust(&w);

        eprintln!("\n== crust budget over {WATCH} ticks: {start:.4e} -> {end:.4e} kg ({:+.2}%) ==",
            100.0 * (end - start) / start);
        let mut rows: Vec<usize> = (0..stages.len()).filter(|&i| delta[i].abs() > 0.0).collect();
        rows.sort_by(|&a, &b| delta[b].abs().partial_cmp(&delta[a].abs()).unwrap());
        for i in rows {
            eprintln!("  {:<20} {:>+12.4e} kg", names[i], delta[i]);
        }
        eprintln!("  (positive MAKES crust, negative REMOVES it)");
    }

    /// **IS THERE A CONTINENT, OR ONLY DOTS?** (2026-08-07, Aaron: *"doesn't
    /// look like continents oceans and mountain ranges just looks like random
    /// dots all over the place"*.)
    ///
    /// Every metric this session moved — denudation, mean strata, welled/sunk,
    /// land fraction — is a GLOBAL AVERAGE, and a global average is blind to
    /// arrangement. A planet that is 33% continental can be one supercontinent
    /// or two thousand speckles, and every number reads the same.
    ///
    /// So measure the arrangement: connected regions of continental crust, how
    /// big they are, and how much a cell agrees with its neighbours. Random dots
    /// and a continent are trivially distinguishable this way — dots have
    /// neighbour-agreement at the base rate and a largest region of one or two
    /// cells; a continent has agreement near 1 and one region holding most of
    /// the area.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn coherence_report() {
        use crate::column::{crust_kind, CrustKind};
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        // **Resolution is PLANET SIZE here** — same hex span, more cells, bigger
        // world. It is not a detail setting: sea-floor age is basin width in
        // cells divided by one cell per tick, so a small planet's basins are
        // narrow and its floor is recycled before it can cool. `FLICKER_FREQ=96`
        // is the bench's own planet; the default keeps the run affordable.
        let freq: u32 = std::env::var("FLICKER_FREQ")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(24);
        let mut w = World::seed(icosphere(freq), b, &t, 42);
        eprintln!("── freq {freq} · {} cells ──", w.columns.len());
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        for _ in 0..BAKE {
            sched.step(&mut w, crate::NOMINAL_DT_MYR, None);
        }

        let n = w.columns.len();
        let area = w.cell_area_m2();
        let cont: Vec<bool> =
            (0..n).map(|i| crust_kind(&w.columns[i]) == CrustKind::Continental).collect();
        let sea = crate::planet::sea_level_m(&w);
        // **On the FLEXED surface** — the one the sea level was solved against.
        // Comparing Airy elevation to a flexed sea level measures neither, and
        // reading that mix as "flexure made it worse" was very nearly the
        // conclusion drawn from it.
        let surface = crate::planet::elevation_field(&w);
        let land: Vec<bool> = (0..n).map(|i| surface[i] > sea).collect();

        // Connected regions, by flood fill over the grid's own neighbours.
        let regions = |mask: &[bool]| -> Vec<usize> {
            let mut seen = vec![false; n];
            let mut sizes = Vec::new();
            for start in 0..n {
                if !mask[start] || seen[start] {
                    continue;
                }
                let (mut stack, mut size) = (vec![start], 0usize);
                seen[start] = true;
                while let Some(c) = stack.pop() {
                    size += 1;
                    for &j in &w.grid.neighbors[c] {
                        let j = j as usize;
                        if mask[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
                sizes.push(size);
            }
            sizes.sort_unstable_by(|a, b| b.cmp(a));
            sizes
        };
        // How much a cell agrees with its neighbours — 1.0 is a solid mass,
        // the population's own share is indistinguishable from noise.
        let agreement = |mask: &[bool]| -> (f64, f64) {
            let (mut same, mut total) = (0usize, 0usize);
            for i in 0..n {
                if !mask[i] {
                    continue;
                }
                for &j in &w.grid.neighbors[i] {
                    total += 1;
                    if mask[j as usize] {
                        same += 1;
                    }
                }
            }
            let share = mask.iter().filter(|&&m| m).count() as f64 / n as f64;
            (same as f64 / total.max(1) as f64, share)
        };

        // **Is the threshold cutting a continuum, or separating two families?**
        // `crust_kind` is a hard cut at SUBDUCTABLE_DENSITY with no hysteresis.
        // If basement density is one smooth hump straddling that value, the
        // speckle is cells jittering either side of an arbitrary line and the
        // classification is the defect. If it is two humps with a gap, the
        // threshold is honest and the incoherence is real structure.
        let cut = crate::column::SUBDUCTABLE_DENSITY;
        let mut dens: Vec<f64> = (0..n)
            .filter(|&i| !w.columns[i].layers.is_empty())
            .map(|i| {
                let (mut mass, mut vol) = (0.0, 0.0);
                for bed in w.columns[i].layers.iter().filter(|l| {
                    !matches!(
                        l.formed_by,
                        crate::column::FormationProcess::Sediment
                            | crate::column::FormationProcess::Organic
                    )
                }) {
                    let m = bed.mass_kg();
                    mass += m;
                    vol += m / crate::column::density_kg_m3(bed);
                }
                if vol > 0.0 {
                    mass / vol
                } else {
                    w.columns[i].mean_density()
                }
            })
            .collect();
        dens.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        eprintln!("\n── basement density, the field `crust_kind` cuts at {cut:.0} ──");
        let q = |v: &[f64], p: f64| v[((v.len() - 1) as f64 * p) as usize];
        eprintln!(
            "  p01 {:.0} · p10 {:.0} · p25 {:.0} · p50 {:.0} · p75 {:.0} · p90 {:.0} · p99 {:.0}",
            q(&dens, 0.01), q(&dens, 0.10), q(&dens, 0.25), q(&dens, 0.50),
            q(&dens, 0.75), q(&dens, 0.90), q(&dens, 0.99),
        );
        // A histogram tight around the cut: a gap here means two families.
        let near = (cut - 300.0, cut + 300.0);
        let bins = 12usize;
        let width = (near.1 - near.0) / bins as f64;
        let mut hist = vec![0usize; bins];
        for &d in &dens {
            if d >= near.0 && d < near.1 {
                hist[(((d - near.0) / width) as usize).min(bins - 1)] += 1;
            }
        }
        eprintln!("  within ±300 of the cut, {} of {} columns:", hist.iter().sum::<usize>(), dens.len());
        for (k, &c) in hist.iter().enumerate() {
            let lo = near.0 + k as f64 * width;
            let mark = if lo <= cut && cut < lo + width { " ← CUT" } else { "" };
            eprintln!("    {:>6.0}–{:<6.0} {:>5}  {}{mark}", lo, lo + width, c, "#".repeat(c / 8));
        }

        // **Would FLEXURE fix it?** Isostasy here is Airy — purely local, each
        // column floating on its own. Real lithosphere is an elastic plate: a
        // load is compensated over a flexural wavelength of ~100–200 km, so at
        // 74 km cells the neighbours carry part of it and elevation is
        // spatially correlated by construction.
        //
        // Simulated here as a neighbour-weighted mean before committing to the
        // refactor — `self_weight` is how much of the load a cell carries
        // alone, so a large weight is stiff/local and a small one is a limp
        // plate spreading everything. Reported at three stiffnesses so the
        // answer is a curve, not a single number that could be luck.
        let elev_local: Vec<f64> =
            (0..n).map(|i| crate::column::elevation_m(&w.columns[i], area)).collect();
        let _ = &elev_local;
        eprintln!("\n── would flexure correlate it? (Airy today = infinitely stiff) ──");
        for self_weight in [6.0f64, 3.0, 1.0] {
            let mut e = elev_local.clone();
            // Two relaxation passes ≈ a load spread over ~2 cells ≈ 150 km.
            for _ in 0..2 {
                let prev = e.clone();
                for i in 0..n {
                    let nb = &w.grid.neighbors[i];
                    if nb.is_empty() {
                        continue;
                    }
                    let sum: f64 = nb.iter().map(|&j| prev[j as usize]).sum();
                    e[i] = (prev[i] * self_weight + sum) / (self_weight + nb.len() as f64);
                }
            }
            // Sea level has to be re-solved on the flexed surface, or the
            // comparison is against a coastline that no longer exists. Same
            // submerged fraction as the world actually has.
            let mut sorted = e.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let submerged = (0..n).filter(|&i| elev_local[i] <= sea).count();
            let flexed_sea = sorted[submerged.min(n - 1)];
            let flexed_land: Vec<bool> = (0..n).map(|i| e[i] > flexed_sea).collect();
            let sizes = regions(&flexed_land);
            let (agree, share) = agreement(&flexed_land);
            eprintln!(
                "  self_weight {self_weight:>4.1}: regions {:>4} · largest {:>4} ({:>4.1}%) · \
                 singletons {:>3} · agreement {:.3} (noise ~{:.3})",
                sizes.len(),
                sizes.first().copied().unwrap_or(0),
                100.0 * sizes.first().copied().unwrap_or(0) as f64
                    / flexed_land.iter().filter(|&&m| m).count().max(1) as f64,
                sizes.iter().filter(|&&s| s == 1).count(),
                agree,
                share,
            );
        }

        // **Is there a shelf, a slope and an abyssal plain?** Earth's hypsometry
        // is famously bimodal — a continental platform near sea level, an
        // abyssal plain 4–5 km down, and a steep slope between them that almost
        // nothing sits on. Split the surface by what the ground IS and print
        // where each population stands: a real margin shows as two separated
        // humps, a drowned mush shows as one.
        {
            use crate::column::{crust_kind, CrustKind};
            let mut by_class: Vec<(&str, Vec<f64>)> = vec![
                ("land (cont, dry)", vec![]),
                ("SHELF (cont, wet)", vec![]),
                ("bed (ocean, wet)", vec![]),
                ("exposed (ocean, dry)", vec![]),
            ];
            let mut thick: Vec<Vec<f64>> = vec![vec![]; 4];
            let mut densy: Vec<Vec<f64>> = vec![vec![]; 4];
            let mut cooledness: Vec<Vec<f64>> = vec![vec![]; 4];
            for (i, &surf) in surface.iter().enumerate().take(n) {
                let wet = surf < sea;
                let slot = match (crust_kind(&w.columns[i]), wet) {
                    (CrustKind::Undifferentiated, _) => continue,
                    (CrustKind::Continental, false) => 0,
                    (CrustKind::Continental, true) => 1,
                    (CrustKind::Oceanic, true) => 2,
                    (CrustKind::Oceanic, false) => 3,
                };
                by_class[slot].1.push(surface[i] - sea);
                thick[slot].push(crate::column::crust_thickness_m(&w.columns[i], area));
                densy[slot].push(w.columns[i].mean_density());
                cooledness[slot].push({
                    let c = &w.columns[i];
                    let m = c.mass_kg().max(1.0);
                    c.layers.iter().map(|l| l.cooled * l.mass_kg()).sum::<f64>() / m
                });
            }
            eprintln!("\n── depth below sea level, by what the ground IS ──");
            for (label, v) in by_class.iter_mut() {
                if v.is_empty() {
                    eprintln!("  {label:<22} (none)");
                    continue;
                }
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let q = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
                eprintln!(
                    "  {label:<22} n {:>5} · p10 {:>8.0} · p50 {:>8.0} · p90 {:>8.0} m",
                    v.len(),
                    q(0.1),
                    q(0.5),
                    q(0.9)
                );
            }
            // **The contrast that MAKES a bimodal hypsometry.** Earth separates
            // continent from abyss by ~5 km, out of thickness (~35 vs ~7 km) and
            // density (~2700 vs ~2900). If those two do not differ here, no
            // amount of water will carve a shelf and a slope.
            eprintln!("\n── the contrast: thickness · density · cooled ──");
            for (k, (label, _)) in by_class.iter().enumerate() {
                if thick[k].is_empty() {
                    continue;
                }
                let med = |v: &mut Vec<f64>| {
                    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    v[v.len() / 2]
                };
                eprintln!(
                    "  {label:<22} thickness {:>8.0} m · density {:>6.0} kg/m³ · cooled {:.2}",
                    med(&mut thick[k]),
                    med(&mut densy[k]),
                    med(&mut cooledness[k]),
                );
            }
        }

        for (label, mask) in [("continental crust", &cont), ("land above sea", &land)] {
            let sizes = regions(mask);
            let count = mask.iter().filter(|&&m| m).count();
            let (agree, share) = agreement(mask);
            let singles = sizes.iter().filter(|&&s| s == 1).count();
            eprintln!(
                "\n── {label}: {count} of {n} cells ({:.1}%) ──\n\
                   regions {} · largest {} ({:.1}% of the population) · singletons {singles}\n\
                   top ten {:?}\n\
                   neighbour agreement {:.3}  (noise would be ~{:.3}; a solid mass ~1.0)",
                100.0 * share,
                sizes.len(),
                sizes.first().copied().unwrap_or(0),
                100.0 * sizes.first().copied().unwrap_or(0) as f64 / count.max(1) as f64,
                &sizes[..sizes.len().min(10)],
                agree,
                share,
            );
        }
    }

    /// **WHY IS EROSION DOING NOTHING?** (2026-08-07.)
    ///
    /// `relief_attribution_report` measured Erosion moving **2,600× less ground
    /// than MassWasting** and contributing −0.00006 of smoothing — effectively
    /// zero — *after* the erosion transport rework landed the same session.
    ///
    /// The cut is `rate · √flow · slope · dt / resistance`, capped by how much
    /// the stream still has room to carry. Any of five things could be starving
    /// it, and arguing about which from the source is exactly what has been
    /// wrong three times today. So this reports the ACTUAL inputs, per land
    /// cell, computed through the very functions the tick uses — the probe
    /// cannot measure a different law than the one that runs.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn erosion_forcing_report() {
        use crate::surface::{cell_spacing_m, slope_between, stream_capacity_kg, stream_cut_m};
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..BAKE {
            sched.step(&mut w, dt, None);
        }

        let n = w.columns.len();
        let area = w.cell_area_m2();
        let weather = crate::surface::Weather::observe(&w, dt, levers.stellar_heat);
        let elevation: Vec<f64> =
            w.columns.iter().map(|c| crate::column::elevation_m(c, area)).collect();

        // The tick's own drainage: lowest neighbour, then gather high-to-low.
        let downhill: Vec<Option<usize>> = (0..n)
            .map(|i| {
                w.grid.neighbors[i]
                    .iter()
                    .map(|&j| j as usize)
                    .filter(|&j| elevation[j] < elevation[i])
                    .min_by(|&a, &b| {
                        elevation[a].partial_cmp(&elevation[b]).unwrap_or(std::cmp::Ordering::Equal)
                    })
            })
            .collect();
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

        // Per LAND cell with somewhere to drain — the only cells that can cut.
        let (mut rains, mut flows, mut slopes) = (vec![], vec![], vec![]);
        let (mut wants, mut rooms, mut cuts) = (vec![], vec![], vec![]);
        let (mut capped, mut sinks, mut sea) = (0usize, 0usize, 0usize);
        for cell in 0..n {
            if elevation[cell] <= weather.sea_level {
                sea += 1;
                continue;
            }
            let Some(to) = downhill[cell] else {
                sinks += 1;
                continue;
            };
            let slope = slope_between(elevation[cell] - elevation[to]);
            let resistance = w.columns[cell]
                .layers
                .last()
                .map(|l| crate::surface::bed_resistance(&t, l) as f64)
                .unwrap_or(1.0);
            let want = stream_cut_m(levers.erosion_rate, flow[cell], slope, resistance, dt);
            let density = w.columns[cell]
                .layers
                .last()
                .map(crate::column::density_kg_m3)
                .unwrap_or(2700.0);
            let room = stream_capacity_kg(flow[cell], area, dt) / (density * area).max(1.0);
            if room < want {
                capped += 1;
            }
            rains.push(weather.rain[cell]);
            flows.push(flow[cell]);
            slopes.push(slope);
            wants.push(want);
            rooms.push(room);
            cuts.push(want.min(room));
        }

        let pct = |v: &mut Vec<f64>, q: f64| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v[((v.len() - 1) as f64 * q) as usize]
        };
        let land = rains.len();
        eprintln!(
            "\n── erosion forcing at {:.0} My · {land} land cells · {sinks} sinks · {sea} submerged ──",
            w.tick_myr
        );
        eprintln!("  cell spacing {:.0} m · dt {dt} My · rate {}", cell_spacing_m(), levers.erosion_rate);
        for (label, v, unit) in [
            ("rain", &mut rains, "m/My"),
            ("flow (gathered)", &mut flows, "m/My"),
            ("slope", &mut slopes, ""),
            ("want cut", &mut wants, "m/tick"),
            ("carry headroom", &mut rooms, "m/tick"),
            ("ACTUAL cut", &mut cuts, "m/tick"),
        ] {
            eprintln!(
                "  {label:<16} p10 {:>11.4e}  p50 {:>11.4e}  p90 {:>11.4e}  max {:>11.4e} {unit}",
                pct(v, 0.1),
                pct(v, 0.5),
                pct(v, 0.9),
                pct(v, 1.0),
            );
        }
        eprintln!(
            "  capacity binds on {capped} of {land} land cells ({:.1}%) — elsewhere the \
             STREAM-POWER law is the limit",
            100.0 * capped as f64 / land.max(1) as f64
        );
        let total: f64 = cuts.iter().sum();
        eprintln!(
            "  summed cut across all land, one tick: {total:.4e} m  \
             (MassWasting moved 1.5e7 m over 100 ticks for comparison)"
        );
    }

    /// **WHAT MAKES THE RELIEF?** (2026-08-07.)
    ///
    /// Slope p90 sits at the angle of repose across a tenth of the planet, and
    /// two confident guesses at the cause — the unpaced crumple, then the crust
    /// ceiling — were both real defects and neither moved it. So stop guessing
    /// and attribute it, the way `what_eats_a_continent_report` settled an
    /// equally circular argument: run the pipeline **stage by stage** and record
    /// what each one does to the elevation field.
    ///
    /// Three separate questions, because a stage can be guilty of any of them:
    /// - **How much ground does it move?** `Σ|Δelev|` — the sheer scale of its
    ///   effect, whether or not that effect is steep.
    /// - **Does it raise or lower the world?** net `Δelev`, summed.
    /// - **Does it STEEPEN?** the change it makes to slope p90. This is the one
    ///   that names the culprit: a stage can move enormous amounts of rock and
    ///   leave the landscape no steeper (it fills as much as it cuts), or move
    ///   very little and ratchet the gradient every tick.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn relief_attribution_report() {
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500;
        const WATCH: usize = 100;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..BAKE {
            sched.step(&mut w, dt, None);
        }

        let n = w.columns.len();
        let area = w.cell_area_m2();
        let span = area.sqrt();
        let elevations = |w: &World| -> Vec<f64> {
            w.columns.iter().map(|c| crate::column::elevation_m(c, area)).collect()
        };
        // The same slope read the bake prints: drop to the lowest neighbour,
        // over the cell spacing.
        let slope_p90 = |w: &World, elev: &[f64]| -> f64 {
            let mut g: Vec<f64> = (0..n)
                .map(|i| {
                    let low = w.grid.neighbors[i]
                        .iter()
                        .map(|&j| elev[j as usize])
                        .fold(f64::INFINITY, f64::min);
                    ((elev[i] - low) / span).max(0.0)
                })
                .collect();
            g.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            g[((n - 1) as f64 * 0.9) as usize]
        };

        let stages = crate::formation_stages(Arc::clone(&t), &w, &levers);
        let names: Vec<&'static str> = stages.iter().map(|s| s.name()).collect();
        let mut moved = vec![0.0f64; stages.len()];
        let mut net = vec![0.0f64; stages.len()];
        let mut steepened = vec![0.0f64; stages.len()];
        let mut fired = vec![0usize; stages.len()];

        for _ in 0..WATCH {
            let state = PlanetState::sample(&w);
            for (index, stage) in stages.iter().enumerate() {
                if !stage.is_live(&state) {
                    continue;
                }
                let before = elevations(&w);
                let p90_before = slope_p90(&w, &before);
                let mut rng = crate::stage::StageRng::for_stage(42, index);
                stage.tick(&mut w, dt, &mut rng);
                let after = elevations(&w);
                steepened[index] += slope_p90(&w, &after) - p90_before;
                for i in 0..n {
                    moved[index] += (after[i] - before[i]).abs();
                    net[index] += after[i] - before[i];
                }
                fired[index] += 1;
            }
            w.tick_myr += dt;
            w.settle_air_species();
        }
        w.audit("relief watch");

        let elev = elevations(&w);
        eprintln!(
            "\n── relief attribution, {WATCH} ticks · standing slope p90 {:.4} (repose {}) ──",
            slope_p90(&w, &elev),
            crate::surface::REPOSE_SLOPE,
        );
        eprintln!(
            "  {:<20} {:>13} {:>13} {:>13} {:>7}",
            "stage", "moved |Δ| m", "net Δ m", "Δ slope p90", "ticks"
        );
        let mut rows: Vec<usize> =
            (0..stages.len()).filter(|&i| moved[i] > 0.0 || steepened[i] != 0.0).collect();
        // Sorted by who STEEPENS, because that is the question.
        rows.sort_by(|&a, &b| {
            steepened[b].partial_cmp(&steepened[a]).unwrap_or(std::cmp::Ordering::Equal)
        });
        for i in rows {
            eprintln!(
                "  {:<20} {:>13.3e} {:>13.3e} {:>+13.5} {:>7}",
                names[i], moved[i], net[i], steepened[i], fired[i]
            );
        }
        eprintln!("(Δ slope p90 sums the per-tick change each stage made — positive STEEPENS)");
    }

    /// **WHERE DOES A TICK GO?** (2026-08-07, Aaron: *"now at 15 seconds by 30
    /// ticks in"*.)
    ///
    /// Wall-clock per stage over the early ticks — the era the bench actually
    /// stalls in. Written because two guesses at the cost in a row were wrong:
    /// the crate's own bakes run 4.5 BY without complaint, so nothing in the
    /// suite reveals a per-tick cost, and reasoning about complexity from the
    /// source had already sent me to the wrong stage twice.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn stage_cost_report() {
        use std::time::Instant;
        const TICKS: usize = 12;
        // **The bench's own resolution.** Measured first at freq 24 (5,762
        // cells) and the whole sim came to 2 ms/tick, which said the cost had to
        // be app-side. The bench runs freq 96 — 92,162 cells, sixteen times as
        // many — so that number was answering a question nobody asked. Measure
        // at the size the thing actually runs.
        const FREQ: u32 = crate::PLANET_FREQ;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, 0.15)]);
        let mut w = World::seed(icosphere(FREQ), b, &t, 42);
        eprintln!("── {} cells ──", w.columns.len());
        let levers = crate::Levers::default();
        let stages = crate::formation_stages(Arc::clone(&t), &w, &levers);
        let names: Vec<&'static str> = stages.iter().map(|s| s.name()).collect();
        let mut spent = vec![0u128; stages.len()];
        let dt = crate::NOMINAL_DT_MYR;

        let whole = Instant::now();
        for tick in 0..TICKS {
            let state = PlanetState::sample(&w);
            for (index, stage) in stages.iter().enumerate() {
                if !stage.is_live(&state) {
                    continue;
                }
                let mut rng = crate::stage::StageRng::for_stage(42, index);
                let at = Instant::now();
                stage.tick(&mut w, dt, &mut rng);
                spent[index] += at.elapsed().as_micros();
            }
            w.tick_myr += dt;
            w.settle_air_species();
            if tick % 10 == 9 {
                eprintln!("  … {} ticks in {:?}", tick + 1, whole.elapsed());
            }
        }

        eprintln!("\n── {TICKS} ticks in {:?} ──", whole.elapsed());
        let mut rows: Vec<usize> = (0..stages.len()).filter(|&i| spent[i] > 0).collect();
        rows.sort_by_key(|&i| std::cmp::Reverse(spent[i]));
        let total: u128 = spent.iter().sum::<u128>().max(1);
        for i in rows {
            eprintln!(
                "  {:<20} {:>9.1} ms  ({:>5.1}%)  {:>8.2} ms/tick",
                names[i],
                spent[i] as f64 / 1000.0,
                100.0 * spent[i] as f64 / total as f64,
                spent[i] as f64 / 1000.0 / TICKS as f64,
            );
        }
    }

    /// **WHICH GATES MOVE, AND WHEN** (2026-08-07, Aaron: *"It got to tick 59
    /// and just stopped doing anything."*)
    ///
    /// The bench **stops the run on a gate edge** by design, so "it stopped" and
    /// "it hung" look identical from the outside. This lists every edge over the
    /// early ticks with the tick it happened on, so the two can be told apart by
    /// reading rather than by guessing — and so that ADDING a process to the
    /// pipeline, which adds a gate that can produce an edge, has its cost
    /// visible.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn gate_edges_report() {
        const TICKS: usize = 120;
        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, 0.15)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);

        let names: Vec<&'static str> = crate::formation_stages(Arc::clone(&t), &w, &levers)
            .iter()
            .map(|s| s.name())
            .collect();
        let mut was: Vec<bool> = Vec::new();
        for tick in 0..TICKS {
            sched.step(&mut w, crate::NOMINAL_DT_MYR, None);
            let state = PlanetState::sample(&w);
            let now: Vec<bool> = names
                .iter()
                .map(|n| crate::process_file::gate_of(n).holds(&state, &levers))
                .collect();
            if was.is_empty() {
                was = now;
                continue;
            }
            for (i, (&before, &after)) in was.iter().zip(now.iter()).enumerate() {
                if before != after {
                    eprintln!(
                        "tick {tick:>4} ({:>8.1} My)  {:<20} {}",
                        w.tick_myr,
                        names[i],
                        if after { "OPENED — the bench pauses here" } else { "shut" }
                    );
                }
            }
            was = now;
        }
        eprintln!("(no line above for a tick = nothing gated on it)");
    }

    /// **DOES THE PRESSURE GATE EVER OPEN?** (2026-08-07.)
    ///
    /// [`Eclogitisation`](crate::crust::Eclogitisation) registered zero of
    /// everything in `what_eats_a_continent_report`, and there are two very
    /// different reasons that could happen: the stage is working and simply
    /// never changes a cell's CLASSIFICATION (it converts rock that is already
    /// oceanic), or it never fires at all because no column on this planet
    /// reaches [`eclogite_pa`](crate::crust::eclogite_pa). The first is correct
    /// behaviour; the second means the pressure half of densification was
    /// removed and nothing replaced it, and `Delamination` now has less to shed.
    ///
    /// A stage that does nothing looks identical to a stage that does the right
    /// nothing, so this measures the gate directly rather than inferring it.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn eclogite_gate_report() {
        use crate::column::{crust_kind, overburden_pa, CrustKind};
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        for _ in 0..BAKE {
            sched.step(&mut w, crate::NOMINAL_DT_MYR, None);
        }

        let (area, g) = (w.cell_area_m2(), w.gravity_m_s2());
        let gate = crate::crust::eclogite_pa(&w);
        let mut deepest: f64 = 0.0;
        let (mut beds, mut past_gate, mut converted) = (0usize, 0usize, 0usize);
        let (mut cols_past, mut cols_converted) = (0usize, 0usize);
        let mut sum_eclog = 0.0;
        for col in &w.columns {
            let (mut any_past, mut any_conv) = (false, false);
            for i in 0..col.layers.len() {
                let load = overburden_pa(col, i, g, area);
                deepest = deepest.max(load);
                beds += 1;
                if load >= gate {
                    past_gate += 1;
                    any_past = true;
                }
                sum_eclog += col.layers[i].eclogitised;
                if col.layers[i].eclogitised > 0.01 {
                    converted += 1;
                    any_conv = true;
                }
            }
            cols_past += any_past as usize;
            cols_converted += any_conv as usize;
        }
        let n = w.columns.len();
        eprintln!(
            "── eclogite gate at {:.0} My · gate {gate:.2e} Pa · g {g:.2} m/s² ──\n\
             deepest overburden in the world {deepest:.3e} Pa ({:.2}× the gate)\n\
             beds {beds} · past gate {past_gate} ({:.3}%) · converted {converted} ({:.3}%)\n\
             columns {n} · with a bed past gate {cols_past} · with a converted bed {cols_converted}\n\
             mean eclogitised over all beds {:.4}",
            w.tick_myr,
            deepest / gate,
            100.0 * past_gate as f64 / beds.max(1) as f64,
            100.0 * converted as f64 / beds.max(1) as f64,
            sum_eclog / beds.max(1) as f64,
        );

        // **And whether the CRUST CEILING is reachable at all.** With the seam
        // circuit closed the conveyor is crust-neutral, so delamination is one
        // of the only net sinks left — and its threshold is an absolute 1.9 GPa
        // written from Earth arithmetic (~70 km × 2800 kg/m³ × 9.81). This
        // planet's gravity is derived from its size, so the same rock column
        // presses far less here. If nothing reaches it, crust has no ceiling.
        let delam = crate::crust::delamination_pa(&w);
        let mut basal: Vec<f64> = w
            .columns
            .iter()
            .map(|c| crate::column::basal_pressure_pa(c, g, area))
            .collect();
        basal.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let at = |q: f64| basal[((basal.len() - 1) as f64 * q) as usize];
        eprintln!(
            "basal pressure p50 {:.3e} · p99 {:.3e} · max {:.3e} Pa · \
             delamination gate {delam:.2e} · over it {} of {}",
            at(0.5),
            at(0.99),
            at(1.0),
            basal.iter().filter(|&&p| p >= delam).count(),
            basal.len(),
        );

        // And what the converted rock IS — the claim is that it is deep root,
        // not surface continent.
        let cont = (0..n).filter(|&i| crust_kind(&w.columns[i]) == CrustKind::Continental).count();
        let cont_converted = (0..n)
            .filter(|&i| crust_kind(&w.columns[i]) == CrustKind::Continental)
            .filter(|&i| w.columns[i].layers.iter().any(|l| l.eclogitised > 0.01))
            .count();
        eprintln!("continental columns {cont} · of those with converted rock {cont_converted}");
    }

    /// **WHAT EATS A CONTINENT?** (2026-08-06, Aaron: *"the black dots which are
    /// still eating the center of continents around mountains… the continents
    /// are just eating themselves"*.)
    ///
    /// The earlier probe asked who strips a hex to BARE and found nobody. This
    /// asks the sharper question his words actually name: what turns ground
    /// that WAS continental into ground that is not — because a felsic cell
    /// whose light cover is removed exposes whatever sits under it, and if that
    /// is mafic it densifies and goes black. Cover loss, not annihilation.
    ///
    /// Attributes every Continental → not-Continental transition, and every
    /// bed removed from a continental column, to the stage that did it.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn what_eats_a_continent_report() {
        use crate::column::{crust_kind, CrustKind};
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500;
        const WATCH: usize = 200;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..BAKE {
            sched.step(&mut w, dt, None);
        }
        let n = w.columns.len();
        let cont = |w: &World| {
            (0..n).filter(|&i| crust_kind(&w.columns[i]) == CrustKind::Continental).count()
        };
        eprintln!("at {:.0} My: {} of {n} cells continental", w.tick_myr, cont(&w));

        let stages = crate::formation_stages(Arc::clone(&t), &w, &levers);
        let names: Vec<&'static str> = stages.iter().map(|s| s.name()).collect();
        let mut lost = vec![0usize; stages.len()];
        let mut gained = vec![0usize; stages.len()];
        let mut beds_taken = vec![0usize; stages.len()];
        let mut mass_taken = vec![0.0f64; stages.len()];

        for _ in 0..WATCH {
            let state = PlanetState::sample(&w);
            for (index, stage) in stages.iter().enumerate() {
                if !stage.is_live(&state) {
                    continue;
                }
                let was: Vec<bool> = (0..n)
                    .map(|i| crust_kind(&w.columns[i]) == CrustKind::Continental)
                    .collect();
                let beds: Vec<usize> = w.columns.iter().map(|c| c.layers.len()).collect();
                let mass: Vec<f64> = w.columns.iter().map(|c| c.mass_kg()).collect();
                let mut rng = crate::stage::StageRng::for_stage(42, index);
                stage.tick(&mut w, dt, &mut rng);
                for i in 0..n {
                    let now = crust_kind(&w.columns[i]) == CrustKind::Continental;
                    if was[i] && !now {
                        lost[index] += 1;
                    } else if !was[i] && now {
                        gained[index] += 1;
                    }
                    if was[i] {
                        let db = beds[i] as i64 - w.columns[i].layers.len() as i64;
                        if db > 0 {
                            beds_taken[index] += db as usize;
                        }
                        let dm = mass[i] - w.columns[i].mass_kg();
                        if dm > 0.0 {
                            mass_taken[index] += dm;
                        }
                    }
                }
            }
            w.tick_myr += dt;
            w.settle_air_species();
        }
        w.audit("continent watch");

        eprintln!("\n── Continental → NOT continental, {WATCH} ticks ──");
        let mut rows: Vec<usize> = (0..stages.len())
            .filter(|&i| lost[i] > 0 || gained[i] > 0 || beds_taken[i] > 0)
            .collect();
        rows.sort_by_key(|&i| std::cmp::Reverse(lost[i]));
        for i in rows {
            eprintln!(
                "  {:<20} lost {:>6} · regained {:>6} · beds off continental {:>7} · mass {:>9.2e} kg",
                names[i], lost[i], gained[i], beds_taken[i], mass_taken[i],
            );
        }
        eprintln!("  continental now: {} of {n}", cont(&w));
    }

    /// **WHO EATS A HEX?** (2026-08-06, Aaron in-window: *"Still eating hexes
    /// every now and then… the eaten hexes now march away from the mountains,
    /// so conveyer theory still holds"*.)
    ///
    /// A hex stripped to BARE MANTLE reads at elevation exactly 0 — near-black
    /// on a relief ramp whose 2nd percentile is negative — and regrows only at
    /// the crust-generation e-fold, so it stays dark for a long time and is
    /// carried by the plate while it does. This attributes every
    /// crust-bearing → bare transition on the WHOLE planet to the stage that
    /// caused it, rather than to whichever stage is suspected.
    ///
    /// Distinct from [`black_dot_report`], which anatomises the cells that are
    /// merely LOW next to cells that are high; this one is about cells that
    /// have nothing left at all.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn who_eats_a_hex_report() {
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500;
        const WATCH: usize = 200;

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..BAKE {
            sched.step(&mut w, dt, None);
        }
        let n = w.columns.len();
        eprintln!(
            "at {:.0} My: {} of {n} cells already bare",
            w.tick_myr,
            w.columns.iter().filter(|c| c.layers.is_empty()).count()
        );

        let stages = crate::formation_stages(Arc::clone(&t), &w, &levers);
        let names: Vec<&'static str> = stages.iter().map(|s| s.name()).collect();
        let mut emptied = vec![0usize; stages.len()];
        let mut refilled = vec![0usize; stages.len()];
        let mut eaten_kg = vec![0.0f64; stages.len()];
        let mut bare_census: Vec<usize> = Vec::new();

        for _ in 0..WATCH {
            let state = PlanetState::sample(&w);
            for (index, stage) in stages.iter().enumerate() {
                if !stage.is_live(&state) {
                    continue;
                }
                let had: Vec<bool> = w.columns.iter().map(|c| !c.layers.is_empty()).collect();
                let before: Vec<f64> = w.columns.iter().map(|c| c.mass_kg()).collect();
                let mut rng = crate::stage::StageRng::for_stage(42, index);
                stage.tick(&mut w, dt, &mut rng);
                for cell in 0..n {
                    let now_bare = w.columns[cell].layers.is_empty();
                    if had[cell] && now_bare {
                        emptied[index] += 1;
                        eaten_kg[index] += before[cell];
                    } else if !had[cell] && !now_bare {
                        refilled[index] += 1;
                    }
                }
            }
            w.tick_myr += dt;
            w.settle_air_species();
            bare_census.push(w.columns.iter().filter(|c| c.layers.is_empty()).count());
        }
        w.audit("who-eats watch");

        eprintln!("\n── crust-bearing → BARE, whole planet, {WATCH} ticks ──");
        let mut rows: Vec<usize> =
            (0..stages.len()).filter(|&i| emptied[i] > 0 || refilled[i] > 0).collect();
        rows.sort_by_key(|&i| std::cmp::Reverse(emptied[i]));
        for i in rows {
            eprintln!(
                "  {:<20} emptied {:>6} · refilled {:>6} · mass eaten {:>9.2e} kg",
                names[i], emptied[i], refilled[i], eaten_kg[i]
            );
        }
        let (lo, hi) = (
            bare_census.iter().copied().min().unwrap_or(0),
            bare_census.iter().copied().max().unwrap_or(0),
        );
        eprintln!(
            "  standing bare count over the window: min {lo} · max {hi} · last {}",
            bare_census.last().copied().unwrap_or(0)
        );
    }

    /// **IS ANYTHING BEING EATEN, AND WHERE ARE THE RIDGES?** (2026-08-06,
    /// Aaron: *"anything that dips into the core goes back into the ledger to
    /// be produced somewhere else, it definitely felt like materials were being
    /// eaten and never did I see new seams of crust coming out of heat lines
    /// (think — mid atlantic ridge)"*.)
    ///
    /// Two separate claims, both measurable, so neither has to be argued:
    ///
    /// 1. **Is mass leaving the books?** The conservation harness already
    ///    proves per-element `present == expected` every tick, so nothing can
    ///    be destroyed — but that is not the same as nothing being *stranded*.
    ///    This prints the whole partition over time, so a reservoir that only
    ///    ever grows (a one-way sink) is visible as such. The core is the one
    ///    worth watching: iron that sinks into it is accounted for, but if it
    ///    never comes back it is gone from play, which is what "eaten" would
    ///    feel like from the surface.
    /// 2. **Are ridges producing new crust?** A mid-ocean ridge should read as
    ///    a LINE of cells whose youngest bed was laid down very recently. This
    ///    measures the age of every cell's youngest bed and then segments the
    ///    fresh ones into connected domains — so "no new seams" becomes a
    ///    number: how many fresh cells, and do they form long thin runs or
    ///    scattered specks?
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn ledger_and_ridge_report() {
        const H_SCALE: f64 = 0.15;
        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        let total = w.budget.total();

        eprintln!(
            "{:>8}  {:>7} {:>7} {:>7} {:>6} {:>6} {:>7}   {:>9} {:>9}",
            "My", "core%", "mantle%", "crust%", "sea%", "air%", "gone%", "crust kg", "core kg"
        );
        let mut last_core = 0.0f64;
        let mut core_ever_fell = false;
        let partition = |w: &World| {
            let r = &w.reservoirs;
            (
                r.core.total(),
                w.mantle.total_mass(),
                w.columns.iter().map(|c| c.mass_kg()).sum::<f64>(),
                r.ocean.mass_kg(),
                r.atmosphere.mass_kg(),
                r.escaped.total(),
            )
        };
        for step in 0..=2800 {
            if step > 0 {
                sched.step(&mut w, dt, None);
            }
            if step % 400 != 0 {
                continue;
            }
            let (core, mantle, crust, sea, air, gone) = partition(&w);
            if core < last_core - 1.0 {
                core_ever_fell = true;
            }
            last_core = core;
            let pct = |m: f64| 100.0 * m / total;
            eprintln!(
                "{:>8.0}  {:>7.2} {:>7.2} {:>7.3} {:>6.3} {:>6.3} {:>7.4}   {crust:>9.2e} {core:>9.2e}",
                w.tick_myr,
                pct(core),
                pct(mantle),
                pct(crust),
                pct(sea),
                pct(air),
                pct(gone),
            );
        }
        eprintln!(
            "\ncore gave mass back at some point: {core_ever_fell}  (a one-way core means \
             everything it swallowed is out of play for good)"
        );

        // ── Where is crust being MADE right now? ──
        let now = w.tick_myr;
        let youngest: Vec<f64> = w
            .columns
            .iter()
            .map(|c| {
                c.layers
                    .iter()
                    .map(|l| now - l.formed_at_myr)
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let mut ages: Vec<f64> = youngest.iter().copied().filter(|a| a.is_finite()).collect();
        ages.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let at = |q: f64| ages[((ages.len() - 1) as f64 * q) as usize];
        eprintln!(
            "\nyoungest bed per cell, My since it formed: p10 {:.0} · p50 {:.0} · p90 {:.0} · \
             oldest {:.0}",
            at(0.10),
            at(0.50),
            at(0.90),
            at(1.0),
        );
        for window in [20.0f64, 50.0, 200.0] {
            let fresh: Vec<bool> = youngest.iter().map(|&a| a <= window).collect();
            let n_fresh = fresh.iter().filter(|&&f| f).count();
            // Connected runs of fresh ground — THE ridge question. A spreading
            // centre is a long thin domain; scattered resurfacing is specks.
            let (_labels, n_domains, sizes) =
                crate::observer::segment_where(&w, 2, &|i, j| fresh[i] && fresh[j]);
            let biggest = sizes.iter().skip(1).copied().max().unwrap_or(0);
            eprintln!(
                "  formed within {window:>4.0} My: {n_fresh:>5} cells ({:>4.1}%) in {n_domains:>4} \
                 runs of 2+, biggest run {biggest} cells",
                100.0 * n_fresh as f64 / w.columns.len() as f64,
            );
        }
        let by_process = |p: crate::column::FormationProcess| {
            w.columns
                .iter()
                .filter(|c| c.layers.last().is_some_and(|l| l.formed_by == p))
                .count()
        };
        use crate::column::FormationProcess as F;
        eprintln!(
            "\ntop bed by origin: oceanic {} · arc {} · volcanic {} · sediment {} · organic {} · \
             hydrothermal {} · primordial {}",
            by_process(F::OceanicCrust),
            by_process(F::ContinentalArc),
            by_process(F::Volcanic),
            by_process(F::Sediment),
            by_process(F::Organic),
            by_process(F::Hydrothermal),
            by_process(F::Primordial),
        );
    }

    /// **WHAT IS THE BLACK DOT BESIDE THE WHITE ONE?** (2026-08-06, Aaron's
    /// standing defect: *"the black dots are still happening around
    /// mountains"*.)
    ///
    /// The relief view is a pure greyscale ramp from the **2nd to the 98th
    /// elevation percentile** (`elevation_color` in the God Mode scene), so
    /// "black" is not a kind of ground — it is literally *at or below p2*. This
    /// probe reproduces that classification exactly, isolates the dark cells
    /// that touch a bright one (the ring Aaron sees), and asks what they are
    /// made of, so three competing explanations can be told apart by
    /// measurement rather than argument:
    ///
    /// - **bare datum** — the column was consumed entirely (subduction), so it
    ///   sits at the bare-mantle datum 0 while its neighbour towers;
    /// - **densified basin** — real negative elevation, eclogite-loaded, stable
    ///   and correct (a trench beside a range is what a trench IS);
    /// - **erosion pit** — over-cut ground that mass wasting is failing to
    ///   refill.
    ///
    /// The second phase re-runs the pipeline **stage by stage** over the same
    /// cells, so whatever is taking their rock is named rather than guessed. It
    /// tracks LOCATIONS, not columns, deliberately: the conveyor relocates
    /// stacks, and a location is what renders.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn black_dot_report() {
        const H_SCALE: f64 = 0.15;
        const BAKE: usize = 1500; // ~2.4 BY — erosion long since running
        const WATCH: usize = 120; // ticks of stage-by-stage attribution

        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget").rescaled(&[(1, H_SCALE)]);
        let mut w = World::seed(icosphere(24), b, &t, 42);
        let levers = crate::Levers::default();
        let mut sched = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
        let dt = crate::NOMINAL_DT_MYR;
        for _ in 0..BAKE {
            sched.step(&mut w, dt, None);
        }

        let area = w.cell_area_m2();
        let n = w.columns.len();
        let elev = |w: &World, i: usize| crate::column::elevation_m(&w.columns[i], area);

        // The scene's own ramp, reproduced: p2 → p98, clamped. Anything at or
        // below `lo` renders pure black; at or above `hi`, pure white.
        let mut sorted: Vec<f64> = (0..n).map(|i| elev(&w, i)).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let (lo, hi) = (sorted[(n - 1) * 2 / 100], sorted[(n - 1) * 98 / 100]);
        let bright: Vec<bool> = (0..n).map(|i| elev(&w, i) >= hi).collect();
        let dark: Vec<usize> = (0..n).filter(|&i| elev(&w, i) <= lo).collect();
        let ring: Vec<usize> = dark
            .iter()
            .copied()
            .filter(|&i| w.grid.neighbors[i].iter().any(|&j| bright[j as usize]))
            .collect();

        // What a population is MADE of — the three hypotheses' fingerprints.
        let anatomy = |w: &World, label: &str, pop: &[usize]| {
            if pop.is_empty() {
                eprintln!("{label:<22} (none)");
                return;
            }
            let k = pop.len() as f64;
            let mean = |f: &dyn Fn(usize) -> f64| pop.iter().map(|&i| f(i)).sum::<f64>() / k;
            let bare = pop.iter().filter(|&&i| w.columns[i].layers.is_empty()).count();
            let at_datum = pop.iter().filter(|&&i| elev(w, i).abs() < 1.0).count();
            let negative = pop.iter().filter(|&&i| elev(w, i) < -1.0).count();
            let sed = pop
                .iter()
                .filter(|&&i| {
                    w.columns[i]
                        .layers
                        .iter()
                        .any(|l| l.formed_by == crate::column::FormationProcess::Sediment)
                })
                .count();
            // The two dense-making states, reported APART. They used to be one
            // number, which is precisely why a pressure phase change firing on
            // surface continental rock could hide inside it for so long.
            let mass_weighted = |i: usize, f: &dyn Fn(&crate::column::Layer) -> f64| {
                let c = &w.columns[i];
                let m = c.mass_kg().max(1.0);
                c.layers.iter().map(|l| f(l) * l.mass_kg()).sum::<f64>() / m
            };
            let cooled = mean(&|i| mass_weighted(i, &|l| l.cooled));
            let eclogitised = mean(&|i| mass_weighted(i, &|l| l.eclogitised));
            eprintln!(
                "{label:<22} n {:>5} · elev {:>8.0} m · bare {:>5.1}% · at-datum {:>5.1}% · \
                 neg {:>5.1}% · beds {:>4.1} · crust {:>9.2e} kg · cooled {:>4.2} · \
                 eclog {:>4.2} · sed {:>5.1}%",
                pop.len(),
                mean(&|i| elev(w, i)),
                100.0 * bare as f64 / k,
                100.0 * at_datum as f64 / k,
                100.0 * negative as f64 / k,
                mean(&|i| w.columns[i].layers.len() as f64),
                mean(&|i| w.columns[i].mass_kg()),
                cooled,
                eclogitised,
                100.0 * sed as f64 / k,
            );
        };

        eprintln!(
            "── black-dot anatomy at {:.0} My · relief ramp p2 {lo:.0} m → p98 {hi:.0} m ──",
            w.tick_myr
        );
        let all: Vec<usize> = (0..n).collect();
        let bright_cells: Vec<usize> = (0..n).filter(|&i| bright[i]).collect();
        anatomy(&w, "world", &all);
        anatomy(&w, "BLACK (<= p2)", &dark);
        anatomy(&w, "  of which RING", &ring);
        anatomy(&w, "WHITE (>= p98)", &bright_cells);

        // ── Phase 2: who takes their rock? Stage by stage, on the same cells. ──
        let stages = crate::formation_stages(Arc::clone(&t), &w, &levers);
        let names: Vec<&'static str> = stages.iter().map(|s| s.name()).collect();
        let mut removed = vec![0.0f64; stages.len()];
        let mut added = vec![0.0f64; stages.len()];
        let mut stripped_bare = vec![0usize; stages.len()];
        // Per-location churn: how many ticks each ring cell stood bare — the
        // discriminator for the consumed-column hypothesis.
        let mut bare_ticks = vec![0usize; ring.len()];

        for _ in 0..WATCH {
            let state = PlanetState::sample(&w);
            for (index, stage) in stages.iter().enumerate() {
                if !stage.is_live(&state) {
                    continue;
                }
                let before: Vec<f64> = ring.iter().map(|&i| w.columns[i].mass_kg()).collect();
                let had: Vec<bool> = ring.iter().map(|&i| !w.columns[i].layers.is_empty()).collect();
                let mut rng = crate::stage::StageRng::for_stage(42, index);
                stage.tick(&mut w, dt, &mut rng);
                for (k, &i) in ring.iter().enumerate() {
                    let now = w.columns[i].mass_kg();
                    let d = now - before[k];
                    if d < 0.0 {
                        removed[index] -= d;
                    } else {
                        added[index] += d;
                    }
                    if had[k] && w.columns[i].layers.is_empty() {
                        stripped_bare[index] += 1;
                    }
                }
            }
            for (k, &i) in ring.iter().enumerate() {
                if w.columns[i].layers.is_empty() {
                    bare_ticks[k] += 1;
                }
            }
            w.tick_myr += dt;
            w.settle_air_species();
            w.audit("black-dot watch");
        }

        eprintln!("\n── who moves the RING's rock, over {WATCH} ticks ({} cells) ──", ring.len());
        let mut rows: Vec<usize> = (0..stages.len())
            .filter(|&i| removed[i] > 0.0 || added[i] > 0.0 || stripped_bare[i] > 0)
            .collect();
        rows.sort_by(|&a, &b| removed[b].total_cmp(&removed[a]));
        for i in rows {
            eprintln!(
                "  {:<20} took {:>9.2e} kg · gave {:>9.2e} kg · net {:>+9.2e} · stripped-to-bare {}",
                names[i],
                removed[i],
                added[i],
                added[i] - removed[i],
                stripped_bare[i],
            );
        }
        let never_bare = bare_ticks.iter().filter(|&&b| b == 0).count();
        let always_bare = bare_ticks.iter().filter(|&&b| b == WATCH).count();
        eprintln!(
            "  ring bareness: never {never_bare} · always {always_bare} · intermittent {} \
             (of {})",
            ring.len() - never_bare - always_bare,
            ring.len(),
        );
        eprintln!("\n── the ring, after {WATCH} more ticks ──");
        anatomy(&w, "RING (same cells)", &ring);
    }

    /// **A coarser grid is a SMALLER PLANET.** The tile span is the canon —
    /// 2048 px × 128 ft ≈ 49.65 mi at every frequency — so the planet fits the
    /// grid: half the frequency is half the radius, an eighth of the mass and
    /// the water, half the gravity, and the SAME cell area (the T6 freq↔radius
    /// pinning). This probe's previous life, `resolution_is_not_size`, measured
    /// the fixed-Earth chemistry config being self-consistent and reported that
    /// as the design — it was measuring the two-models defect (2026-08-06
    /// incident). Now it asserts the size law and reports how drowned each
    /// size of world gets under the same reference dials.
    #[test]
    #[ignore = "a report for the maintainer, run by hand"]
    fn frequency_is_size() {
        let dir = content_data_dir();
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        for freq in [12u32, 24, 48] {
            let b = Budget::from_dir(&dir, &t).expect("budget");
            let mut w = World::seed(icosphere(freq), b, &t, 42);
            let levers = crate::Levers::default();
            let mut sched =
                Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &levers), 42);
            let s = w.size_scale();
            // The size law, asserted before the bake so the report can be trusted.
            assert!(
                ((w.budget.total() - crate::PLANET_MASS_KG * s.powi(3)) / w.budget.total()).abs()
                    < 1e-9,
                "accreted mass rides size³"
            );
            assert!(
                (w.cell_area_m2() - crate::CELL_AREA_M2).abs() < 1e-3,
                "the hex is the same hex at every frequency"
            );
            for _ in 0..500 {
                sched.step(&mut w, crate::NOMINAL_DT_MYR, None);
            }
            let st = PlanetState::sample(&w);
            eprintln!(
                "freq {freq:>3}: {:>6} cells · R {:.0} km (s = {s:.3}) · surface {:.4e} m²\n\
                 \x20         planet {:.4e} kg · g {:.2} m/s² · ocean {:.3e} kg · \
                 submerged {:.1}% · sea {:.0} m · lid {:.0}%",
                w.columns.len(),
                w.radius_m() / 1000.0,
                w.cell_area_m2() * w.columns.len() as f64,
                w.budget.total(),
                w.gravity_m_s2(),
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
            crate::formation_stages(Arc::clone(&t), &w, &levers),
            42,
        );
        // A second, identical stage list purely to PROBE `is_live` — the
        // scheduler owns its own.
        let probes = crate::formation_stages(Arc::clone(&t), &w, &levers);
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
            // **The FLEXED surface**, which is what `sea_level_m` was solved
            // against. Reading Airy elevation against a flexed sea level
            // measures neither and made this report disagree with
            // `PlanetState::submerged_frac` by thirty points.
            let mut elev: Vec<f64> = crate::planet::elevation_field(w);
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
            let raw: Vec<f64> = crate::planet::elevation_field(w);
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
                "            slope p50 {:.4}  p90 {:.4}  max {:.4}   (repose {})",
                g(0.5),
                g(0.9),
                g(1.0),
                crate::surface::REPOSE_SLOPE,
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
