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
    biosphere, content_data_dir, crust_kind, elevation_m, enrichment, formation_stages,
    observe_habitability,
    Budget, CrustKind, Habitability, Levers, PlanetState, PlateEvent, PlateObservation,
    PlateObserver, ProcessState, Scheduler, World, NOMINAL_DT_MYR, PLANET_FREQ,
};

/// Target sim rate while playing (ticks/sec) — a watchable pace that also leaves
/// the CPU free for a smooth 60 fps render. The sim thread never blocks the frame,
/// so a slow tick just lowers this, it never hitches the UI.
const PLAY_TICKS_PER_SEC: f32 = 30.0;

/// Commands from the render thread to the sim thread.
#[derive(Debug)]
pub enum SimCommand {
    /// Space — start/stop advancing.
    TogglePlay,
    /// Down — back to t=0 with the same seed (the same planet, restarted).
    Reset,
    /// R / FORGE — a new world, born from the Starter's pending knobs.
    Reseed(SeedSpec),
    /// **Run rain over the inspected tile** — start/stop the per-pixel erosion
    /// batch on its own background thread. The tile keeps evolving and the preview
    /// keeps updating while the aggregate sim does whatever it is doing.
    ErodeToggle,
    /// **Look inside one cell** — materialise its tile and publish a preview.
    ///
    /// The truth migration for a single cell, on demand. It is not cheap (a few
    /// hundred milliseconds and ~100 MiB for the maps) and it is not something the
    /// sim does on its own — so it happens when the maintainer asks, on the sim
    /// thread, where a slow call costs a tick rather than a frame.
    Inspect(u32),
    /// **Hold a process, or let it go.** A held stage does not run; the world
    /// simply carries on without it, which is how the maintainer guides formation
    /// without ever writing a result into it.
    Hold { stage: String, held: bool },
    /// **Set the conditions.** Replaces the levers and rebuilds the pipeline
    /// around them. Cheap — a pipeline is a handful of boxed stages — and it keeps
    /// every rate a construction-time fact rather than something read through a
    /// lock on the hot path.
    SetLevers(Levers),
    /// Ticks per second while playing.
    SetRate(f32),
    /// The window is closing.
    Shutdown,
}

/// **The forge order** — everything a NEW world is born from. These are t=0
/// boundary conditions, not live levers: dialing them moves nothing until the
/// forge births a fresh planet. This is where Mars and Mercury come from — a
/// smaller ball, a different endowment, the same laws.
#[derive(Clone, Debug)]
pub struct SeedSpec {
    /// The run's random seed.
    pub seed: u64,
    /// Icosphere frequency — the planet's size in cells (`10·f² + 2`).
    pub freq: u32,
    /// Per-element multipliers applied to the base accretion budget
    /// ([`Budget::rescaled`]): `(atomic number, factor)`.
    pub scales: Vec<(u8, f64)>,
}

/// The Starter's knob roster: the volatiles that make sky and sea, and the
/// rock-formers that make everything else. Symbols resolved through the
/// periodic table at spawn; the UI shows them as data.
const SEED_KNOBS: [&str; 12] =
    ["H", "C", "N", "S", "O", "Si", "Fe", "Mg", "Ca", "Al", "Na", "K"];

/// A materialised tile, reduced to something a panel can show.
#[derive(Clone)]
pub struct TilePreview {
    /// RGBA image of the tile's composite surface, `dim` square.
    pub rgba: Vec<u8>,
    pub dim: u32,
    /// What it is: cell, beds, clusters, height range.
    pub caption: String,
}

/// Static, render-only topology + the immutable bulk-seed readout. Sent once, when
/// the sim thread has finished building the planet.
pub struct StaticData {
    pub dirs: Vec<Vec3>,
    pub outlines: Vec<Vec<Vec3>>,
    /// The bulk seed's element distribution `(number, symbol, percent)`, ≥0.1%,
    /// descending — computed once on the sim thread (it owns the budget + tables).
    pub budget_dist: Vec<(u8, String, f64)>,
    /// The compound catalog's `(id, formula)` vocabulary (name where the catalog
    /// has no formula) — how the air readout says WHAT the dominant gas is. The
    /// whole catalog rides along so every species a later stage books is already
    /// nameable.
    pub gas_names: Vec<(u16, String)>,
    /// The pipeline **as the maintainer authored it** — `processes.json`'s own
    /// entries, whole. The gate cards read their prose and the tab strip reads
    /// their `view`, so the file the maintainer edits IS what the bench shows.
    ///
    /// Shipped as the loaded type rather than a tuple of the fields anyone
    /// currently wants: a tuple has to be widened, at three sites, every time
    /// the file grows a column — which is exactly the drift the file exists to
    /// avoid.
    pub processes: Vec<flicker_poc_chemistry::ProcessDef>,
    /// The Starter's knobs: `(atomic number, symbol)` for each endowment slider,
    /// in display order — resolved once from the periodic table.
    pub seed_elements: Vec<(u8, String)>,
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
    /// How many beds this column has stacked — the history it is carrying.
    pub strata: u8,
    /// Isostatic elevation, m on the mantle datum. Read against
    /// `state.sea_level_m` to see what is land and what is under water — or
    /// read [`CellView::coast`], which has already done exactly that.
    pub elevation_m: f32,
    /// The richest metal seam in this column, as a multiple of the planet's own
    /// share of that metal. `1` is ordinary rock; a vein is orders above.
    pub ore: f32,
    /// What the ground here IS against the sea — [`SHELF_LAND`] … plus
    /// [`SHELF_EDGE`] where a neighbour is something else. Buoyancy against
    /// water level, which is the distinction "shelf vs deep floor" actually
    /// turns on and the one no rock colour can show.
    pub coast: u8,
    /// Rain reaching this ground, m of water per tick — the weather field
    /// erosion actually cuts with, published instead of discarded.
    pub rain: f32,
    /// Unit heading this column is being carried along (tangent, world space);
    /// zero while it is not moving.
    pub motion_dir: Vec3,
    /// How far along its next one-hex step this column has been carried,
    /// `0..1` — the honest read, because a plate STEPS discretely: a raw
    /// velocity would look frozen between steps while this fills toward one.
    pub motion_step: f32,
}

/// Water-equivalent mass per kilogram of bound hydrogen — water is ~11.2% H by
/// mass, so hydrogen locked in tissue or rock stands for this much sea that is
/// no longer sea.
const WATER_PER_H: f64 = 1.0 / 0.1119;

/// The metals the ore view looks for — the elements the catalog says harvestable
/// minerals are extracted for, kept to the ones a hydrothermal fluid can move.
/// Filled once when the sim thread loads the tables.
static ORE_METALS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

/// Layer-bed bitmask flags (kept next to [`CellView`] so the renderer and the
/// publisher agree on the bit layout as the stack grows).
pub const BED_OCEANIC: u8 = 0b01;
pub const BED_CONTINENTAL: u8 = 0b10;

// ── Coast classes ([`CellView::coast`]) ──
//
// The two facts that decide what a piece of ground IS to anyone standing on
// it: what the mantle makes of it (buoyant enough to keep, or swallowable) and
// whether the sea is over it. Crossed, they are the four grounds — and the
// pair that no other view separates is CONTINENTAL-UNDER-WATER (a shelf) from
// OCEANIC-UNDER-WATER (deep floor), which in the rock view are both just blue.
/// No crust at all — bare mantle.
pub const SHELF_NONE: u8 = 0;
/// Buoyant crust standing above the sea — land.
pub const SHELF_LAND: u8 = 1;
/// Buoyant crust standing under the sea — the continental shelf.
pub const SHELF_SHELF: u8 = 2;
/// Dense crust under the sea — ocean bed.
pub const SHELF_BED: u8 = 3;
/// Dense crust standing above the sea — exposed floor (a rarity worth seeing).
pub const SHELF_EXPOSED: u8 = 4;
/// Set when at least one neighbour is a DIFFERENT class: the coastline itself,
/// computed where the adjacency lives so the renderer needs no topology.
pub const SHELF_EDGE: u8 = 0b1000;
/// Mask of the class, without the edge flag.
pub const SHELF_CLASS: u8 = 0b0111;

/// **What this ground is**, from the two facts that decide it: whether the
/// mantle could swallow this column ([`crust_kind`]) and whether the sea stands
/// over it. Pure, so the table can be read — and tested — without a planet.
pub fn coast_class(kind: CrustKind, elevation_m: f64, sea_level_m: f64) -> u8 {
    let drowned = elevation_m < sea_level_m;
    match kind {
        CrustKind::Undifferentiated => SHELF_NONE,
        CrustKind::Continental if drowned => SHELF_SHELF,
        CrustKind::Continental => SHELF_LAND,
        CrustKind::Oceanic if drowned => SHELF_BED,
        CrustKind::Oceanic => SHELF_EXPOSED,
    }
}

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
    /// What every process is doing and why — running, held, or waiting on the
    /// world. The bench's process panel is a direct read of this.
    pub processes: Vec<ProcessState>,
    /// The conditions and rates currently in force.
    pub levers: Levers,
    /// How far life has got, and what it has put in the ground: the stage's
    /// token plus buried tissue / coal / oils, kg. The fuel a world has is the
    /// carbon its biosphere buried before anything evolved to eat it.
    pub life: flicker_poc_chemistry::LifeStage,
    pub tissue_kg: f64,
    pub coal_kg: f64,
    pub oils_kg: f64,
    /// **Where the planet's water actually is**, kg — standing sea, vapour
    /// aloft, and the hydrogen life and the rock have taken out of circulation
    /// (as water-equivalent). Mass is audited every tick and never leaks, so
    /// "the ocean is shrinking" is always one of these four growing instead;
    /// without the other three on screen the sea's number looks like a loss.
    pub water_sea_kg: f64,
    pub water_sky_kg: f64,
    pub water_life_kg: f64,
    pub water_stone_kg: f64,
    /// The classified air, heaviest gas first: `(catalog id, column kg/m²)` —
    /// the derived shell stack ([`flicker_poc_chemistry::air_shells`]), read on
    /// the sim thread because it owns the tables.
    pub air_shells: Vec<(u16, f32)>,
    /// The five-axis life-supporting reading — the earth-likeness gauges and
    /// the gate indication for the truth migration. An observer's verdict: it
    /// tells the maintainer the planet is ready; arming stays a human act.
    pub habitability: Habitability,
    /// The most recent gate transitions, oldest → newest. The sim pauses itself
    /// on these, and the readout says which one stopped it.
    pub gate_events: Vec<GateEvent>,
    /// Ticks per second while playing — the transport dial's ECHO. The dial
    /// publishes what the sim is actually pacing at rather than what the bench
    /// last asked for, so a clamped or ignored request shows up as a control
    /// that springs back instead of a lie that sticks.
    pub rate_hz: f32,
}

/// **A gate moved** — a process's own chemistry either let it start or stopped
/// it. `opened` says which way.
#[derive(Clone)]
pub struct GateEvent {
    pub at_myr: f64,
    pub stage: &'static str,
    pub opened: bool,
}

/// Watches the pipeline's gates for **edges**.
///
/// The gate it watches is the EFFECTIVE one — `ready && !held`: the world's
/// chemistry satisfied AND the maintainer's hand off the lever. A change in it
/// is the world crossing a threshold: the first crust freezing over, the
/// interior cooling past the melt floor, a sea standing long enough to start
/// eating the sky. Those are the moments the planet changes character — an
/// epoch boundary in embryo — and they are the natural places for the
/// simulation to stop and let the maintainer look.
///
/// Two silences, both deliberate:
/// - **A held stage says nothing.** Its chemistry may keep crossing thresholds
///   as the world evolves, but a gate the maintainer has pinned shut is shut —
///   announcing its private chemistry read as "events" is how a run full of
///   holds still popped summaries on what looked like a schedule.
/// - **The maintainer's own hand is not a world event.** An edge counts only
///   when the hold flag was still across the pair of frames, so clicking
///   HOLD/RELEASE never pauses the run on itself.
///
/// Detection is a pure consumer of what the process panel already publishes: no
/// stage knows this exists, and nothing here can influence the world.
#[derive(Default)]
struct GateWatch {
    /// Last tick's `(name, effective, held)` (names stable across a rebuild).
    prev: Vec<(&'static str, bool, bool)>,
    log: Vec<GateEvent>,
}

impl GateWatch {
    /// The most recent transitions kept for the readout.
    const KEEP: usize = 6;

    /// Compare this tick's gates with last tick's; log every edge and report
    /// whether any moved. The **first** call only establishes the baseline —
    /// a world's opening conditions are not a transition.
    fn observe(&mut self, at_myr: f64, procs: &[ProcessState]) -> bool {
        let now: Vec<(&'static str, bool, bool)> =
            procs.iter().map(|p| (p.name, p.ready && !p.held, p.held)).collect();
        let mut moved = false;
        if !self.prev.is_empty() {
            for &(stage, opened, held) in &now {
                let was = self.prev.iter().find(|&&(n, ..)| n == stage);
                if let Some(&(_, was_open, was_held)) = was {
                    if held == was_held && was_open != opened {
                        self.log.push(GateEvent { at_myr, stage, opened });
                        moved = true;
                    }
                }
            }
            if self.log.len() > Self::KEEP {
                self.log.drain(0..self.log.len() - Self::KEEP);
            }
        }
        self.prev = now;
        moved
    }

    fn reset(&mut self) {
        self.prev.clear();
        self.log.clear();
    }
}

/// What the sim thread tells the erosion worker.
enum EroCmd {
    /// A freshly-materialised tile to work on, with each migrated bed's density
    /// and resistance (the rock tier's read, taken once — the worker itself never
    /// touches the world).
    Tile(Box<flicker_worldtile::Tile>, Vec<flicker_worldtile::BedProps>),
    /// Start/stop the rain.
    Toggle,
}

/// The per-pixel erosion batch — **its own background thread**, per the restraint
/// ruling: hundreds of milliseconds per pass over ~3.5M pixel-clusters is work
/// that belongs to neither the render thread nor the sim thread. It owns the
/// inspected tile outright (pixels are truth; the aggregate keeps its own course),
/// paces itself at a watchable cadence, and publishes a preview into the same slot
/// the inspector already reads.
fn erosion_main(
    cmd_rx: Receiver<EroCmd>,
    tile_slot: Arc<Mutex<Option<TilePreview>>>,
) {
    use flicker_worldtile::{Eroder, ErosionParams};

    /// Between passes, so the batch stays a background hum rather than a core pinned
    /// flat — the restrained prototype posture.
    const PASS_PAUSE: Duration = Duration::from_millis(350);

    let mut eroder = Eroder::new();
    let params = ErosionParams::default();
    let mut current: Option<(Box<flicker_worldtile::Tile>, Vec<flicker_worldtile::BedProps>)> = None;
    let mut raining = false;
    let mut passes = 0u64;
    let mut books = flicker_worldtile::PassReport::default();

    loop {
        // Idle: block until told otherwise. Raining: pace the passes.
        let cmd = if raining && current.is_some() {
            match cmd_rx.recv_timeout(PASS_PAUSE) {
                Ok(c) => Some(c),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match cmd_rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            }
        };
        match cmd {
            Some(EroCmd::Tile(tile, props)) => {
                // A new inspection replaces the ground being rained on; the books
                // start over with it.
                current = Some((tile, props));
                passes = 0;
                books = flicker_worldtile::PassReport::default();
            }
            Some(EroCmd::Toggle) => raining = !raining,
            None => {}
        }
        if raining {
            if let Some((tile, props)) = current.as_mut() {
                let r = eroder.pass(tile, props, &params);
                passes += 1;
                books.eroded_kg += r.eroded_kg;
                books.deposited_kg += r.deposited_kg;
                books.exported_kg += r.exported_kg;
                let (rgba, dim) = tile.preview_rgba(4);
                let caption = format!(
                    "{} · pass {} · −{:.1} Mt · +{:.1} Mt · out {:.1} Mt",
                    tile.summary(),
                    passes,
                    books.eroded_kg / 1.0e9,
                    books.deposited_kg / 1.0e9,
                    books.exported_kg / 1.0e9,
                );
                if let Ok(mut slot) = tile_slot.lock() {
                    *slot = Some(TilePreview { rgba, dim, caption });
                }
            }
        }
    }
}

/// The render thread's handle to the sim thread.
pub struct SimHandle {
    cmd_tx: Sender<SimCommand>,
    static_rx: Receiver<StaticData>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    /// The most recent tile preview, in its own slot so a 100 MiB materialisation
    /// is never cloned along with the per-frame snapshot.
    tile: Arc<Mutex<Option<TilePreview>>>,
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
        let tile: Arc<Mutex<Option<TilePreview>>> = Arc::new(Mutex::new(None));
        let tile_for_thread = Arc::clone(&tile);
        let join = thread::Builder::new()
            .name("flicker-sim".into())
            .spawn(move || sim_main(seed, cmd_rx, static_tx, latest_for_thread, tile_for_thread))
            .expect("spawn sim thread");
        Self { cmd_tx, static_rx, latest, tile, join: Some(join) }
    }

    /// Send a command (best-effort — a closed channel means the thread is gone).
    pub fn send(&self, cmd: SimCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// The one-time topology, once the sim thread has built it.
    pub fn take_static(&self) -> Option<StaticData> {
        self.static_rx.try_recv().ok()
    }

    /// Take the tile preview if one has been made since the last look.
    pub fn take_tile(&self) -> Option<TilePreview> {
        self.tile.lock().ok()?.take()
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
    tile_slot: Arc<Mutex<Option<TilePreview>>>,
) {
    let (mut grid, outlines) = icosphere_with_outlines(PLANET_FREQ);
    let dir = content_data_dir();
    // Shared with the stages that read the catalog (crystallization asks what a
    // mineral is made of; erosion asks how well the resulting rock lasts).
    let tables = Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("material tables"));
    let _ = ORE_METALS.set(
        tables
            .compounds()
            .iter()
            .filter(|c| c.harvestable)
            .filter_map(|c| c.extracted_element.as_deref())
            .filter_map(|sym| tables.element(sym))
            .filter(|e| e.category == "transition_metal")
            .map(|e| e.number)
            .collect(),
    );
    // The BASE seed: what accretion.json says. Every forged world scales THIS,
    // so knob settings never compound across reseeds.
    let base_budget = Budget::from_dir(&dir, &tables).expect("accretion.json");
    let mut budget = base_budget.clone();

    let seed_elements: Vec<(u8, String)> = SEED_KNOBS
        .iter()
        .filter_map(|sym| tables.element(sym).map(|e| (e.number, sym.to_string())))
        .collect();

    // Static topology + seed readout: sent at spawn and RE-SENT whenever a forge
    // changes them (a new size is a new mesh; a new endowment is a new seed
    // panel). The scene replaces its copy whenever one arrives.
    let send_static = |grid: &flicker_worldgrid::Sphere,
                       outlines: Vec<Vec<Vec3>>,
                       budget: &Budget,
                       seed_elements: &[(u8, String)]| {
        let _ = static_tx.send(StaticData {
            dirs: grid.dirs.clone(),
            outlines,
            budget_dist: budget_distribution(budget, &tables),
            gas_names: tables
                .compounds()
                .iter()
                .map(|c| {
                    let label =
                        if c.formula.is_empty() { c.name.clone() } else { c.formula.clone() };
                    (c.id, label)
                })
                .collect(),
            seed_elements: seed_elements.to_vec(),
            processes: flicker_poc_chemistry::load_processes(&content_data_dir()),
        });
    };

    // The sim thread keeps the outlines: materialising a tile needs the cell's own
    // boundary, and the render thread has its own copy for the mesh.
    let mut tile_outlines = outlines.clone();
    send_static(&grid, outlines, &budget, &seed_elements);

    let mut levers = Levers::default();
    let mut rate_hz = PLAY_TICKS_PER_SEC;
    let mut seed = seed0;
    let mut world = World::seed(grid.clone(), budget.clone(), &tables, seed);
    let mut sched = Scheduler::new(formation_stages(Arc::clone(&tables), &budget, &levers), seed);
    let mut playing = false;
    let mut gen = 0u64;
    // The plate observer is read-only annotation on the sim thread — never a stage.
    let mut observer = PlateObserver::new();
    let mut event_log: Vec<(f64, PlateEvent)> = Vec::new();
    // Watches the pipeline's gates for edges and stops the run on one.
    let mut gates = GateWatch::default();

    // The erosion worker: spawned with the sim, fed on Inspect, toggled on demand.
    // Dropping `ero_tx` at the end of sim_main is its shutdown.
    let (ero_tx, ero_rx) = mpsc::channel::<EroCmd>();
    let ero_slot = Arc::clone(&tile_slot);
    let _erosion_thread = thread::Builder::new()
        .name("flicker-erosion".into())
        .spawn(move || erosion_main(ero_rx, ero_slot))
        .expect("spawn erosion thread");


    observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen);

    loop {
        loop {
            match cmd_rx.try_recv() {
                Ok(SimCommand::Inspect(cell)) => {
                    let cell = cell as usize;
                    if cell < world.columns.len() {
                        // The tile span is fixed at a hex, so the planet's radius is
                        // whatever this grid frequency implies.
                        let r = flicker_worldtile::radius_for_freq(grid.freq);
                        let tile = flicker_worldtile::materialize(&world, cell, r, &tile_outlines[cell]);
                        let (rgba, dim) = tile.preview_rgba(4);
                        if let Ok(mut slot) = tile_slot.lock() {
                            *slot = Some(TilePreview { rgba, dim, caption: tile.summary() });
                        }
                        // Hand the ground to the rain. Each migrated bed carries the
                        // rock tier's own read of what it is — the same resistance
                        // the aggregate erosion uses, so the two scales cannot
                        // disagree about how hard a rock is.
                        let props: Vec<flicker_worldtile::BedProps> = world.columns[cell]
                            .layers
                            .iter()
                            .map(|bed| flicker_worldtile::BedProps {
                                density_kg_m3: flicker_poc_chemistry::density_kg_m3(bed),
                                resistance: flicker_poc_chemistry::bed_resistance(&tables, bed),
                            })
                            .collect();
                        let _ = ero_tx.send(EroCmd::Tile(Box::new(tile), props));
                    }
                }
                Ok(SimCommand::ErodeToggle) => {
                    let _ = ero_tx.send(EroCmd::Toggle);
                }
                Ok(SimCommand::Hold { stage, held }) => {
                    sched.set_held(&stage, held);
                    // Republish NOW: the console's rows and its HOLD/RELEASE
                    // labels are reads of the snapshot, and while the run is
                    // paused no tick will publish one — without this the click
                    // works but the panel keeps showing the old state until
                    // something else happens to publish. The watch never fires
                    // on the hold itself (the maintainer's hand is not a world
                    // event), so this cannot pause the run on its own click —
                    // but a REAL edge that lands the same instant still counts.
                    if observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen) {
                        playing = false;
                    }
                }
                Ok(SimCommand::SetRate(hz)) => {
                    rate_hz = hz.clamp(1.0, 240.0);
                    // Republish, for the same reason Hold does: while paused no
                    // tick publishes, so the dial's ECHO stays at the old value
                    // and the control springs back the moment it is released
                    // (Aaron, 2026-08-06 — "slider at the start for ticks... it
                    // snaps back to where it was").
                    observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen);
                }
                Ok(SimCommand::SetLevers(next)) => {
                    // Rates live in the stages, so changing one rebuilds them. The
                    // world is untouched: this changes how it will evolve from here,
                    // never what it already is. Held stages keep their state, because
                    // holding is the scheduler's business, not the pipeline's.
                    levers = next;
                    let held: Vec<String> = sched
                        .processes(&PlanetState::sample(&world))
                        .into_iter()
                        .filter(|p| p.held)
                        .map(|p| p.name.to_string())
                        .collect();
                    sched = Scheduler::new(
                        formation_stages(Arc::clone(&tables), &budget, &levers),
                        seed,
                    );
                    for name in held {
                        sched.set_held(&name, true);
                    }
                    if observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen) {
                        playing = false;
                    }
                }
                Ok(SimCommand::TogglePlay) => {
                    playing = !playing;
                    // Same republish, same reason — and this one is the button's
                    // own LABEL. Pausing stops the ticks that would have
                    // published the new state, so the transport went on saying
                    // PAUSE after it had already paused.
                    observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen);
                }
                Ok(SimCommand::Reset) => {
                    world = World::seed(grid.clone(), budget.clone(), &tables, seed);
                    sched = Scheduler::new(formation_stages(Arc::clone(&tables), &budget, &levers), seed);
                    observer.reset();
                    event_log.clear();
                    gates.reset();
                    playing = false;
                    observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen);
                }
                Ok(SimCommand::Reseed(spec)) => {
                    // A forge is a NEW PLANET: size, endowment and seed all take
                    // effect here and nowhere else. The budget is rescaled from
                    // BASE (knobs never compound), the grid rebuilds only when
                    // the size actually changed, and the fresh topology + seed
                    // readout are re-sent so the scene rebuilds its meshes.
                    seed = spec.seed;
                    budget = base_budget.rescaled(&spec.scales);
                    if spec.freq != grid.freq {
                        let (g, o) = icosphere_with_outlines(spec.freq);
                        grid = g;
                        tile_outlines = o.clone();
                        send_static(&grid, o, &budget, &seed_elements);
                    } else {
                        send_static(&grid, tile_outlines.clone(), &budget, &seed_elements);
                    }
                    world = World::seed(grid.clone(), budget.clone(), &tables, seed);
                    sched = Scheduler::new(formation_stages(Arc::clone(&tables), &budget, &levers), seed);
                    observer.reset();
                    event_log.clear();
                    gates.reset();
                    playing = false;
                    observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen);
                }
                Ok(SimCommand::Shutdown) => return,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        if playing {
            sched.step(&mut world, NOMINAL_DT_MYR, None);
            // A gate moving is the world crossing one of its own condition
            // thresholds — the moment it changed character. Stop there and let
            // the maintainer look; Space picks the run back up.
            if observe_and_publish(&latest, &world, &sched, &tables, &mut observer, &mut event_log, &mut gates, playing, rate_hz, &levers, &mut gen) {
                playing = false;
            }
            thread::sleep(Duration::from_secs_f32(1.0 / rate_hz));
        } else {
            thread::sleep(Duration::from_millis(16));
        }
    }
}

/// Run the read-only plate observer on the (already-stepped) world, fold its events
/// into the rolling log, and publish a fresh snapshot. The observer is **not** a sim
/// stage — it annotates the stepped world; causes-only stays intact.
///
/// Returns **true if a gate moved this tick** — the caller's cue to stop the run
/// so the maintainer can see the world at the moment it changed character.
#[allow(clippy::too_many_arguments)]
fn observe_and_publish(
    latest: &Arc<Mutex<Option<Snapshot>>>,
    world: &World,
    sched: &Scheduler,
    tables: &Tables,
    observer: &mut PlateObserver,
    event_log: &mut Vec<(f64, PlateEvent)>,
    gates: &mut GateWatch,
    playing: bool,
    rate_hz: f32,
    levers: &Levers,
    gen: &mut u64,
) -> bool {
    let obs = observer.observe(world);
    for e in &obs.events {
        event_log.push((world.tick_myr, e.clone()));
    }
    const KEEP: usize = 8;
    if event_log.len() > KEEP {
        event_log.drain(0..event_log.len() - KEEP);
    }
    publish(latest, world, sched, tables, &obs, event_log, gates, playing, rate_hz, levers, gen)
}

/// Total booked mass of the named compounds across every bed on the planet —
/// how the readout says what life has put in the ground.
fn organic_total(world: &World, of: &[u16]) -> f64 {
    world
        .columns
        .iter()
        .flat_map(|c| c.layers.iter())
        .map(|l| of.iter().map(|&id| l.minerals.amount(id)).sum::<f64>())
        .sum()
}

/// Build and publish a render snapshot into the shared slot. Returns whether a
/// gate moved — read off the same process vector the panel shows, so the pause
/// and the readout can never disagree about what happened.
#[allow(clippy::too_many_arguments)]
fn publish(
    latest: &Arc<Mutex<Option<Snapshot>>>,
    world: &World,
    sched: &Scheduler,
    tables: &Tables,
    obs: &PlateObservation,
    event_log: &[(f64, PlateEvent)],
    gates: &mut GateWatch,
    playing: bool,
    rate_hz: f32,
    levers: &Levers,
    gen: &mut u64,
) -> bool {
    let state = PlanetState::sample(world);
    let cell_area = world.cell_area_m2();
    // The metals worth looking for, named by the catalog rather than here.
    let ore_metals: &[u8] = ORE_METALS.get().map(|v| v.as_slice()).unwrap_or(&[]);
    // **This tick's weather, published instead of thrown away.** `Weather` holds
    // no state and takes `&World` — it is a read, and running it here neither
    // touches the world nor double-counts the erosion stage's own call. Same dt
    // the pipeline steps at, so the rain shown is the rain that cut.
    let weather = flicker_poc_chemistry::Weather::observe(world, NOMINAL_DT_MYR, levers.stellar_heat);
    // What each ground IS against the sea. Computed in two passes because the
    // EDGE is a fact about neighbours, and the adjacency lives here.
    let sea = state.sea_level_m;
    let coast: Vec<u8> = world
        .columns
        .iter()
        .map(|col| coast_class(crust_kind(col), elevation_m(col, cell_area), sea))
        .collect();
    let cells = world
        .columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            // Which shell this cell belongs to is its BUOYANCY, not the provenance
            // of the beds it happens to contain. Once the conveyor started thrusting
            // stacks onto one another most columns carry beds of both kinds, so
            // reading `formed_by` lit up both shells nearly everywhere and the
            // picture went flat. `crust_kind` is the read that decides what the
            // column does — whether the mantle can swallow it — so it is the read
            // that decides what it looks like.
            let beds = match crust_kind(col) {
                CrustKind::Oceanic => BED_OCEANIC,
                CrustKind::Continental => BED_CONTINENTAL,
                CrustKind::Undifferentiated => 0,
            };
            // The step the conveyor is ACTUALLY about to take: its own carried
            // displacement against its own spacing law (`cell_spacing`), so the
            // arrow cannot point at a different step from the one that lands.
            let carried = col.accum_disp;
            let spacing = flicker_poc_chemistry::cell_spacing(world, i).max(1e-9);
            CellView {
                temp_k: world.mantle.temp_k[i] as f32,
                differentiation: world.mantle.differentiation[i] as f32,
                beds,
                plate: obs.labels[i],
                seam: obs.seams[i].code(),
                strata: col.layers.len().min(u8::MAX as usize) as u8,
                elevation_m: elevation_m(col, cell_area) as f32,
                ore: ore_metals
                    .iter()
                    .map(|&e| enrichment(world, i, e).0)
                    .fold(0.0f64, f64::max) as f32,
                coast: coast[i]
                    | if world.grid.neighbors[i]
                        .iter()
                        .any(|&j| coast[j as usize] != coast[i])
                    {
                        SHELF_EDGE
                    } else {
                        0
                    },
                rain: weather.rain[i] as f32,
                motion_dir: carried.normalize_or_zero(),
                motion_step: (carried.length() / spacing).clamp(0.0, 1.0),
            }
        })
        .collect();

    // The gates, read once: the panel shows them and the watch reads the same
    // vector for edges, so the pause and the readout cannot disagree.
    let processes = sched.processes(&state);
    let moved = gates.observe(world.tick_myr, &processes);

    *gen += 1;
    let snap = Snapshot {
        gen: *gen,
        tick: sched.ticks(),
        tick_myr: world.tick_myr,
        playing: playing && !moved,
        rate_hz,
        swept_cells: world.cell_count(),
        processes,
        gate_events: gates.log.clone(),
        levers: *levers,
        air_shells: flicker_poc_chemistry::air_shells(world, tables)
            .iter()
            .map(|s| (s.compound, s.column_kg_m2 as f32))
            .collect(),
        habitability: observe_habitability(world, levers),
        life: world.life,
        tissue_kg: organic_total(world, &[biosphere::CELLULOSE, biosphere::LIGNIN]),
        coal_kg: organic_total(world, &[biosphere::COAL]),
        oils_kg: organic_total(world, &[biosphere::OILS]),
        // The hydrosphere's four addresses. Life and rock are counted by the
        // HYDROGEN they hold, scaled to the water that hydrogen came out of —
        // the honest question is "how much sea did this take", and a tonne of
        // cellulose does not lock a tonne of water.
        water_sea_kg: world.reservoirs.ocean.mass_kg(),
        water_sky_kg: world.reservoirs.atmosphere.species.amount(flicker_poc_chemistry::atmosphere::WATER_VAPOUR),
        water_life_kg: world
            .columns
            .iter()
            .flat_map(|c| c.layers.iter())
            .filter(|l| l.formed_by == flicker_poc_chemistry::FormationProcess::Organic)
            .map(|l| l.elements.amount(1) * WATER_PER_H)
            .sum(),
        water_stone_kg: world
            .columns
            .iter()
            .flat_map(|c| c.layers.iter())
            .filter(|l| l.formed_by != flicker_poc_chemistry::FormationProcess::Organic)
            .map(|l| l.elements.amount(1) * WATER_PER_H)
            .sum(),
        state,
        cells,
        plate_count: obs.plates.len(),
        recent_events: event_log.to_vec(),
    };
    if let Ok(mut g) = latest.lock() {
        *g = Some(snap);
    }
    moved
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

#[cfg(test)]
mod tests {
    use super::*;

    fn procs(gates: &[(&'static str, bool)]) -> Vec<ProcessState> {
        gates
            .iter()
            .map(|&(name, ready)| ProcessState { name, held: false, ready })
            .collect()
    }

    fn procs_held(gates: &[(&'static str, bool, bool)]) -> Vec<ProcessState> {
        gates
            .iter()
            .map(|&(name, ready, held)| ProcessState { name, held, ready })
            .collect()
    }

    /// **The pause hook.** A gate's `ready` bit is a read of the world's own
    /// chemistry, so a CHANGE in it is the world crossing a threshold — and that
    /// is where the run stops. The opening conditions are not a transition: the
    /// first observation only establishes the baseline, or every world would
    /// pause on its own first tick.
    #[test]
    fn the_watch_fires_on_edges_and_never_on_the_baseline() {
        let mut w = GateWatch::default();

        // First look: whatever the world starts as is simply what it is.
        assert!(!w.observe(0.0, &procs(&[("Outgassing", true), ("Volcanism", false)])));
        assert!(w.log.is_empty(), "an opening condition is not an event");

        // Nothing moved.
        assert!(!w.observe(1.0, &procs(&[("Outgassing", true), ("Volcanism", false)])));

        // The lid froze over: Volcanism woke up. That is a pause.
        assert!(w.observe(2.0, &procs(&[("Outgassing", true), ("Volcanism", true)])));
        let e = w.log.last().expect("an event");
        assert_eq!((e.stage, e.opened, e.at_myr), ("Volcanism", true, 2.0));

        // Steady again — a gate that is merely still open is not news.
        assert!(!w.observe(3.0, &procs(&[("Outgassing", true), ("Volcanism", true)])));

        // The interior cooled past the melt floor: it shuts, and that pauses too.
        assert!(w.observe(9.0, &procs(&[("Outgassing", false), ("Volcanism", false)])));
        assert_eq!(w.log.len(), 3, "both of this tick's edges were logged");
        assert!(w.log.iter().rev().take(2).all(|e| !e.opened), "and both were closures");
    }

    /// **The two silences.** A HELD stage says nothing — its chemistry may keep
    /// crossing thresholds as the world evolves, but a gate the maintainer
    /// pinned shut is shut, and announcing its private read is how a run full
    /// of holds still popped summaries on what looked like a schedule. And the
    /// maintainer's own hand is not a world event: hold/release never fires the
    /// watch, so clicking a lever cannot pause the run on itself.
    #[test]
    fn a_held_stage_is_silent_and_the_lever_is_not_an_event() {
        let mut w = GateWatch::default();

        // Baseline: infall running, unheld.
        assert!(!w.observe(0.0, &procs_held(&[("WaterDelivery", true, false)])));

        // The maintainer holds it. The effective gate shuts — but that was the
        // LEVER, not the world. Silence.
        assert!(
            !w.observe(1.0, &procs_held(&[("WaterDelivery", true, true)])),
            "holding a stage must not fire the watch"
        );
        assert!(w.log.is_empty());

        // While held, its chemistry flips back and forth as the world evolves.
        // Still silence — a pinned gate has nothing to announce.
        assert!(!w.observe(2.0, &procs_held(&[("WaterDelivery", false, true)])));
        assert!(!w.observe(3.0, &procs_held(&[("WaterDelivery", true, true)])));
        assert!(w.log.is_empty(), "a held stage's private chemistry is not an event");

        // Released while its chemistry is satisfied: the release itself is the
        // maintainer's hand — silent — and only a LATER, world-caused change
        // speaks again.
        assert!(!w.observe(4.0, &procs_held(&[("WaterDelivery", true, false)])));
        assert!(
            w.observe(5.0, &procs_held(&[("WaterDelivery", false, false)])),
            "after release, the world's own transitions fire again"
        );
        let e = w.log.last().expect("an event");
        assert_eq!((e.stage, e.opened), ("WaterDelivery", false));
    }

    /// A reseed is a different planet: its opening conditions are a baseline, not
    /// a transition from the last world's.
    #[test]
    fn a_reset_forgets_the_previous_world() {
        let mut w = GateWatch::default();
        w.observe(0.0, &procs(&[("Volcanism", true)]));
        w.reset();
        assert!(w.log.is_empty(), "the log is cleared");
        assert!(
            !w.observe(0.0, &procs(&[("Volcanism", false)])),
            "the new world's opening state does not fire against the old one's"
        );
    }
}
