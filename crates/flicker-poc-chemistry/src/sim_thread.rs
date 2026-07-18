//! The simulation runs on its **own thread**, detached from the renderer (spec
//! §11: "a worker pool processes cells detached from any renderer"). The scene
//! sends it commands and reads the latest published [`Snapshot`]; it never steps
//! the sim on the render thread — doing so froze the whole app at 92k cells (the
//! per-cell stages plus the every-tick conservation audit are far too heavy to run
//! inside a frame).
//!
//! Ownership is clean: the sim thread owns the `World`, `Scheduler`, and the grid;
//! the render thread owns a clone of the topology (for the mesh) and whatever
//! snapshot it last read. They talk over a command channel and a single
//! mutex-guarded slot that the sim overwrites and the renderer reads.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use glam::Vec3;

use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgrid::icosphere_with_outlines;

use flicker_poc_chemistry::{
    content_data_dir, formation_stages, Budget, FormationProcess, PlanetState, PlateEvent,
    PlateObservation, PlateObserver, Scheduler, World, NOMINAL_DT_MYR, PLANET_FREQ,
};

/// Target sim rate while playing (ticks/sec) — a watchable pace that also leaves
/// the CPU free for a smooth 60 fps render. The sim thread never blocks the frame,
/// so a slow tick just lowers this, it never hitches the UI.
const PLAY_TICKS_PER_SEC: f32 = 30.0;

/// Commands from the render thread to the sim thread.
pub enum SimCommand {
    /// Space — start/stop advancing.
    TogglePlay,
    /// Down — back to t=0 with the same seed (the same planet, restarted).
    Reset,
    /// R — a new seed → a different planet.
    Reseed(u64),
    /// The window is closing.
    Shutdown,
}

/// Static, render-only topology + the immutable bulk-seed readout. Sent once, when
/// the sim thread has finished building the planet.
pub struct StaticData {
    pub dirs: Vec<Vec3>,
    pub outlines: Vec<Vec<Vec3>>,
    /// The bulk seed's element distribution `(number, symbol, percent)`, ≥0.1%,
    /// descending — computed once on the sim thread (it owns the budget + tables).
    pub budget_dist: Vec<(u8, String, f64)>,
}

/// Compact per-cell render data — what the layer-shell meshes read. `beds` is a
/// bitmask of the layer types stacked in this cell (it grows as later stages add
/// layer types), so each shell mesh can draw only the cells that carry its layer.
#[derive(Clone)]
pub struct CellView {
    pub temp_k: f32,
    pub differentiation: f32,
    /// bit 0 = oceanic crust bed, bit 1 = continental crust bed (M2).
    pub beds: u8,
    /// Persistent plate id from the observer (`0` = diffuse).
    pub plate: u32,
    /// Seam class code (0 interior · 1 divergent · 2 convergent · 3 transform).
    pub seam: u8,
}

/// Layer-bed bitmask flags (kept next to [`CellView`] so the renderer and the
/// publisher agree on the bit layout as the stack grows).
pub const BED_OCEANIC: u8 = 0b01;
pub const BED_CONTINENTAL: u8 = 0b10;

/// One published frame of sim state for the renderer.
#[derive(Clone)]
pub struct Snapshot {
    /// Monotonic publish counter — the render thread rebuilds its mesh only when
    /// this changes (survives a reset-to-tick-0, unlike the tick number).
    pub gen: u64,
    pub tick: u64,
    pub tick_myr: f64,
    pub playing: bool,
    pub swept_cells: usize,
    pub state: PlanetState,
    pub cells: Vec<CellView>,
    /// Number of tracked plates this tick (observer, not sim state).
    pub plate_count: usize,
    /// The most recent plate life-events `(tick_myr, event)`, oldest → newest.
    pub recent_events: Vec<(f64, PlateEvent)>,
}

/// The render thread's handle to the sim thread.
pub struct SimHandle {
    cmd_tx: Sender<SimCommand>,
    static_rx: Receiver<StaticData>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    join: Option<JoinHandle<()>>,
}

impl SimHandle {
    /// Spawn the sim thread; it builds the planet, then loops. Returns immediately —
    /// the render thread shows a loading screen until [`take_static`](Self::take_static)
    /// yields the topology.
    pub fn spawn(seed: u64) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (static_tx, static_rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let latest_for_thread = Arc::clone(&latest);
        let join = thread::Builder::new()
            .name("flicker-sim".into())
            .spawn(move || sim_main(seed, cmd_rx, static_tx, latest_for_thread))
            .expect("spawn sim thread");
        Self { cmd_tx, static_rx, latest, join: Some(join) }
    }

    /// Send a command (best-effort — a closed channel means the thread is gone).
    pub fn send(&self, cmd: SimCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// The one-time topology, once the sim thread has built it.
    pub fn take_static(&self) -> Option<StaticData> {
        self.static_rx.try_recv().ok()
    }

    /// The latest snapshot **iff** it is newer than `since_gen` — avoids cloning
    /// 92k cells every frame when nothing advanced.
    pub fn latest_if_newer(&self, since_gen: u64) -> Option<Snapshot> {
        let guard = self.latest.lock().ok()?;
        match &*guard {
            Some(s) if s.gen != since_gen => Some(s.clone()),
            _ => None,
        }
    }
}

impl Drop for SimHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(SimCommand::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// The sim thread body: build the planet, publish it, then service commands and
/// advance while playing.
fn sim_main(
    seed0: u64,
    cmd_rx: Receiver<SimCommand>,
    static_tx: Sender<StaticData>,
    latest: Arc<Mutex<Option<Snapshot>>>,
) {
    let (grid, outlines) = icosphere_with_outlines(PLANET_FREQ);
    let dir = content_data_dir();
    let tables = Tables::from_source(&JsonTableSource::new(&dir)).expect("material tables");
    let budget = Budget::from_dir(&dir, &tables).expect("accretion.json");

    let _ = static_tx.send(StaticData {
        dirs: grid.dirs.clone(),
        outlines,
        budget_dist: budget_distribution(&budget, &tables),
    });

    let mut seed = seed0;
    let mut world = World::seed(grid.clone(), budget.clone(), &tables, seed);
    let mut sched = Scheduler::new(formation_stages(), seed);
    let mut playing = false;
    let mut gen = 0u64;
    // The plate observer is read-only annotation on the sim thread — never a stage.
    let mut observer = PlateObserver::new();
    let mut event_log: Vec<(f64, PlateEvent)> = Vec::new();

    let play_period = Duration::from_secs_f32(1.0 / PLAY_TICKS_PER_SEC);
    observe_and_publish(&latest, &world, &sched, &mut observer, &mut event_log, playing, &mut gen);

    loop {
        loop {
            match cmd_rx.try_recv() {
                Ok(SimCommand::TogglePlay) => playing = !playing,
                Ok(SimCommand::Reset) => {
                    world = World::seed(grid.clone(), budget.clone(), &tables, seed);
                    sched = Scheduler::new(formation_stages(), seed);
                    observer.reset();
                    event_log.clear();
                    playing = false;
                    observe_and_publish(&latest, &world, &sched, &mut observer, &mut event_log, playing, &mut gen);
                }
                Ok(SimCommand::Reseed(s)) => {
                    seed = s;
                    world = World::seed(grid.clone(), budget.clone(), &tables, seed);
                    sched = Scheduler::new(formation_stages(), seed);
                    observer.reset();
                    event_log.clear();
                    playing = false;
                    observe_and_publish(&latest, &world, &sched, &mut observer, &mut event_log, playing, &mut gen);
                }
                Ok(SimCommand::Shutdown) => return,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        if playing {
            sched.step(&mut world, NOMINAL_DT_MYR, None);
            observe_and_publish(&latest, &world, &sched, &mut observer, &mut event_log, playing, &mut gen);
            thread::sleep(play_period);
        } else {
            thread::sleep(Duration::from_millis(16));
        }
    }
}

/// Run the read-only plate observer on the (already-stepped) world, fold its events
/// into the rolling log, and publish a fresh snapshot. The observer is **not** a sim
/// stage — it annotates the stepped world; causes-only stays intact.
fn observe_and_publish(
    latest: &Arc<Mutex<Option<Snapshot>>>,
    world: &World,
    sched: &Scheduler,
    observer: &mut PlateObserver,
    event_log: &mut Vec<(f64, PlateEvent)>,
    playing: bool,
    gen: &mut u64,
) {
    let obs = observer.observe(world);
    for e in &obs.events {
        event_log.push((world.tick_myr, e.clone()));
    }
    const KEEP: usize = 8;
    if event_log.len() > KEEP {
        event_log.drain(0..event_log.len() - KEEP);
    }
    publish(latest, world, sched, &obs, event_log, playing, gen);
}

/// Build and publish a render snapshot into the shared slot.
fn publish(
    latest: &Arc<Mutex<Option<Snapshot>>>,
    world: &World,
    sched: &Scheduler,
    obs: &PlateObservation,
    event_log: &[(f64, PlateEvent)],
    playing: bool,
    gen: &mut u64,
) {
    let state = PlanetState::sample(world);
    let cells = world
        .columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            let mut beds = 0u8;
            for layer in &col.layers {
                match layer.formed_by {
                    FormationProcess::OceanicCrust => beds |= BED_OCEANIC,
                    FormationProcess::ContinentalArc => beds |= BED_CONTINENTAL,
                    FormationProcess::Primordial => {}
                }
            }
            CellView {
                temp_k: world.mantle.temp_k[i] as f32,
                differentiation: world.mantle.differentiation[i] as f32,
                beds,
                plate: obs.labels[i],
                seam: obs.seams[i].code(),
            }
        })
        .collect();

    *gen += 1;
    let snap = Snapshot {
        gen: *gen,
        tick: sched.ticks(),
        tick_myr: world.tick_myr,
        playing,
        swept_cells: world.cell_count(),
        state,
        cells,
        plate_count: obs.plates.len(),
        recent_events: event_log.to_vec(),
    };
    if let Ok(mut g) = latest.lock() {
        *g = Some(snap);
    }
}

/// The bulk seed's element distribution `(number, symbol, percent)` for elements
/// ≥ 0.1%, descending — what a *planet* is made of (Fe-dominated, not O).
fn budget_distribution(budget: &Budget, tables: &Tables) -> Vec<(u8, String, f64)> {
    let total = budget.total();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut list: Vec<(u8, String, f64)> = budget
        .iter()
        .map(|(num, mass)| {
            let sym = tables
                .element_by_number(num)
                .map(|e| e.symbol.clone())
                .unwrap_or_else(|| num.to_string());
            (num, sym, mass / total * 100.0)
        })
        .filter(|(_, _, pct)| *pct >= 0.1)
        .collect();
    list.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    list
}
