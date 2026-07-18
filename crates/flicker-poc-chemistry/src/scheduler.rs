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

/// The formation scheduler: an ordered stage list, a master seed (for the
/// per-stage RNG streams), and a worker pool for the cell sweep.
pub struct Scheduler {
    stages: Vec<Box<dyn Stage>>,
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
            if !stage.is_live(&state) {
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
