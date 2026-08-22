//! The plate + seam **observer** (spec §5.2 — the emergent tectonic read).
//!
//! A plate is not state the sim owns: it is a *derived read of the convection
//! velocity field*, the same way `rock_kind` is a read of composition. This module
//!
//! 1. **segments** the surface into coherent-velocity domains (a plate = a maximal
//!    block of cells whose velocities agree — a rigid raft),
//! 2. tracks their **identity across ticks** — so a plate *growing*, *shrinking*,
//!    *merging*, or *splitting* is legible as one continuous plate with events,
//!    rather than a fresh random relabelling every tick, and
//! 3. classifies the **seams** between plates (ridge / trench / transform) from the
//!    two plates' relative motion — the read that decides what happens to material
//!    where plates meet.
//!
//! It is **read-only**: it moves no mass, is not a [`Stage`](crate::stage::Stage),
//! never runs in the conservation harness, and never feeds back into the causes
//! (causes-only — the observer only annotates). Its output drives the viewer and,
//! later, the acceptance harness ("N major plates, M continental collisions").

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glam::Vec3;

use crate::interior::tangent_toward;
use crate::planet::World;

/// Two neighbours share a plate when their velocities differ by less than this
/// fraction of the mean flow speed — a rigid block moves together. Higher → fewer,
/// larger plates. The resulting count is emergent, never fixed.
const COHERENCE: f32 = 0.6;
/// **Hysteresis on the coupling, as two multipliers on the base threshold.**
///
/// The threshold is mean-relative on purpose (it keeps working as the flow
/// decays), but that puts the segmentation permanently at its own percolation
/// point: a whole population of contacts rides within a hair of `1.0×`, and
/// each tick's numerical drift flips a few — one flipped edge at a bottleneck
/// reconnects or severs entire domains, and the plate count jumps 3→8→5→4
/// while nothing on the surface visibly changes (Aaron's churn report,
/// 2026-08-06). So a contact INSIDE a plate stays coupled until it genuinely
/// tears ([`STICK`]×), and a contact that was not inside one couples only when
/// it genuinely calms ([`JOIN`]×). The dead band between them is where the
/// flicker used to live; a REAL reorganisation crosses the whole band and
/// still reads exactly as before.
const STICK: f32 = 1.3;
const JOIN: f32 = 0.8;
/// A domain smaller than this many cells is not tracked as a plate — it is diffuse
/// lithosphere (label `0`). Suppresses per-tick speckle without a temporal filter.
const MIN_PLATE_CELLS: usize = 8;
/// A previous↔current domain pair is a "same plate" edge only when their overlap is
/// at least this fraction of the *smaller* domain — the substantial-fraction guard
/// that keeps ordinary boundary drift from reading as a split or merge.
const SIGNIFICANT_FRAC: f64 = 0.30;
/// |convergence| below this multiple of the shear speed is a transform (strike-slip)
/// boundary rather than a ridge or trench.
const TRANSFORM_RATIO: f32 = 0.6;

/// A persistent plate id. `0` is reserved for *diffuse* (no tracked plate).
pub type PlateId = u32;

/// Seam class of a cell that borders a different plate (read from the two plates'
/// relative motion). A cell whose neighbours are all the same plate is `Interior`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Seam {
    Interior,
    /// Plates pulling apart — a spreading ridge.
    Divergent,
    /// Plates closing — a trench / collision.
    Convergent,
    /// Plates shearing past each other — strike-slip.
    Transform,
}

impl Seam {
    /// Compact code for the render snapshot: 0 interior, 1 divergent, 2 convergent,
    /// 3 transform.
    pub fn code(self) -> u8 {
        match self {
            Seam::Interior => 0,
            Seam::Divergent => 1,
            Seam::Convergent => 2,
            Seam::Transform => 3,
        }
    }
}

/// One tracked plate this tick.
#[derive(Clone, Debug)]
pub struct PlateRecord {
    pub id: PlateId,
    pub cell_count: usize,
    /// Mean surface velocity of the plate (its rigid drift).
    pub drift: Vec3,
}

/// A tectonic life-event flagged between the previous observation and this one.
#[derive(Clone, Debug, PartialEq)]
pub enum PlateEvent {
    /// A new coherent domain with no significant ancestor.
    Born(PlateId),
    /// A plate that lost its cells to no single successor.
    Died(PlateId),
    /// Several plates fused into one (`into` survives; `from` retire).
    Merged { from: Vec<PlateId>, into: PlateId },
    /// One plate rifted into several (the largest keeps the id).
    Split { from: PlateId, into: Vec<PlateId> },
}

/// One observation: per-cell plate label + seam class, the plate roster, and the
/// life-events since the previous observation.
#[derive(Clone, Debug, Default)]
pub struct PlateObservation {
    /// Persistent plate id per cell (`0` = diffuse).
    pub labels: Vec<PlateId>,
    /// Seam class per cell.
    pub seams: Vec<Seam>,
    /// The plates present this tick, ascending by id.
    pub plates: Vec<PlateRecord>,
    /// What changed since the previous observation.
    pub events: Vec<PlateEvent>,
}

/// The stateful observer — holds the previous labelling and the monotonic id
/// allocator so plate identity survives across ticks. Construct one per planet;
/// [`reset`](Self::reset) it on reseed/restart.
pub struct PlateObserver {
    prev: Vec<PlateId>,
    next_id: PlateId,
}

impl Default for PlateObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl PlateObserver {
    pub fn new() -> Self {
        Self {
            prev: Vec::new(),
            next_id: 1,
        }
    }

    /// Forget all identity — the next observation starts fresh (every plate `Born`).
    pub fn reset(&mut self) {
        self.prev.clear();
        self.next_id = 1;
    }

    fn alloc(&mut self) -> PlateId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Observe the world: segment the velocity field, match identity against the
    /// previous observation, classify seams. **Read-only** in `world`.
    pub fn observe(&mut self, world: &World) -> PlateObservation {
        let n = world.mantle.n_cells();
        let (raw, n_raw, new_size) = segment(world, &self.prev);
        let (persistent, events) = self.resolve(&raw, n_raw, &new_size);

        // Relabel cells to persistent ids and accumulate each plate's drift.
        let mut labels = vec![0u32; n];
        let mut acc: BTreeMap<PlateId, (usize, Vec3)> = BTreeMap::new();
        for cell in 0..n {
            let r = raw[cell];
            if r == 0 {
                continue;
            }
            let id = persistent[r as usize];
            labels[cell] = id;
            let e = acc.entry(id).or_insert((0, Vec3::ZERO));
            e.0 += 1;
            e.1 += world.mantle.velocity[cell];
        }
        let plates: Vec<PlateRecord> = acc
            .iter()
            .map(|(&id, &(cells, dsum))| PlateRecord {
                id,
                cell_count: cells,
                drift: dsum / cells.max(1) as f32,
            })
            .collect();

        let drift_of: BTreeMap<PlateId, Vec3> = plates.iter().map(|p| (p.id, p.drift)).collect();
        let seams = classify_seams(world, &labels, &drift_of);

        self.prev = labels.clone();
        PlateObservation {
            labels,
            seams,
            plates,
            events,
        }
    }

    /// Map each new raw component (`1..=n_raw`) to a persistent id, and emit the
    /// life-events implied by how the previous plates map onto the new ones. Returns
    /// `persistent[raw]` (index `0` unused) and the events. Deterministic: every
    /// order-dependent choice breaks ties by size then smallest id.
    fn resolve(
        &mut self,
        raw: &[u32],
        n_raw: usize,
        new_size: &[usize],
    ) -> (Vec<PlateId>, Vec<PlateEvent>) {
        let mut persistent = vec![0u32; n_raw + 1];
        let mut events = Vec::new();

        // Previous plates present, with their sizes (deterministic order).
        let mut prev_size: BTreeMap<PlateId, usize> = BTreeMap::new();
        for &p in &self.prev {
            if p != 0 {
                *prev_size.entry(p).or_insert(0) += 1;
            }
        }

        // No prior plates (first observation, or a still world last tick): all Born.
        if prev_size.is_empty() {
            for slot in persistent.iter_mut().skip(1) {
                let id = self.alloc();
                *slot = id;
                events.push(PlateEvent::Born(id));
            }
            return (persistent, events);
        }

        let prev_ids: Vec<PlateId> = prev_size.keys().copied().collect();
        let prev_index: BTreeMap<PlateId, usize> =
            prev_ids.iter().enumerate().map(|(i, &p)| (p, i)).collect();

        // Overlap counts between new raw comps and previous plates.
        let mut overlap: HashMap<(u32, PlateId), usize> = HashMap::new();
        for (cell, &r) in raw.iter().enumerate() {
            let p = self.prev.get(cell).copied().unwrap_or(0);
            if r != 0 && p != 0 {
                *overlap.entry((r, p)).or_insert(0) += 1;
            }
        }
        let ov = |r: u32, p: PlateId| overlap.get(&(r, p)).copied().unwrap_or(0);

        // Bipartite union-find: new comps are nodes `r-1`, previous plates are nodes
        // `n_raw + prev_index[p]`. Union the significant edges; each connected
        // component is one continuation / merge / split / reorganisation.
        let n_prev = prev_ids.len();
        let mut uf = UnionFind::new(n_raw + n_prev);
        for (&(r, p), &count) in &overlap {
            let smaller = new_size[r as usize].min(prev_size[&p]) as f64;
            if count as f64 >= SIGNIFICANT_FRAC * smaller {
                uf.union((r - 1) as usize, n_raw + prev_index[&p]);
            }
        }

        // Group nodes by component root (BTreeMap → deterministic iteration).
        let mut groups: BTreeMap<usize, (Vec<u32>, Vec<PlateId>)> = BTreeMap::new();
        for r in 1..=n_raw {
            groups.entry(uf.find(r - 1)).or_default().0.push(r as u32);
        }
        for (i, &p) in prev_ids.iter().enumerate() {
            groups.entry(uf.find(n_raw + i)).or_default().1.push(p);
        }

        for (_root, (mut news, mut prevs)) in groups {
            news.sort_unstable();
            prevs.sort_unstable();
            match (news.len(), prevs.len()) {
                (0, _) => {
                    for p in prevs {
                        events.push(PlateEvent::Died(p));
                    }
                }
                (_, 0) => {
                    for r in news {
                        let id = self.alloc();
                        persistent[r as usize] = id;
                        events.push(PlateEvent::Born(id));
                    }
                }
                (1, 1) => {
                    // Continuation — one plate, grown / shrunk / reshaped. No event.
                    persistent[news[0] as usize] = prevs[0];
                }
                (1, _) => {
                    // Merge — the survivor is the largest previous plate.
                    let into = *prevs
                        .iter()
                        .max_by_key(|&&p| (prev_size[&p], std::cmp::Reverse(p)))
                        .expect("non-empty");
                    persistent[news[0] as usize] = into;
                    let from: Vec<PlateId> = prevs.into_iter().filter(|&p| p != into).collect();
                    events.push(PlateEvent::Merged { from, into });
                }
                (_, 1) => {
                    // Split — the largest child inherits the id, the rest are fresh.
                    let from = prevs[0];
                    let mut order = news.clone();
                    order.sort_by_key(|&r| (std::cmp::Reverse(new_size[r as usize]), r));
                    persistent[order[0] as usize] = from;
                    for &r in &order[1..] {
                        persistent[r as usize] = self.alloc();
                    }
                    let into: Vec<PlateId> =
                        order.iter().map(|&r| persistent[r as usize]).collect();
                    events.push(PlateEvent::Split { from, into });
                }
                _ => {
                    // Many-to-many reshuffle: give each child its dominant ancestor
                    // if still unclaimed, else a fresh id. No single clean event.
                    let mut taken: BTreeSet<PlateId> = BTreeSet::new();
                    let mut order = news.clone();
                    order.sort_by_key(|&r| (std::cmp::Reverse(new_size[r as usize]), r));
                    for &r in &order {
                        let best = prevs
                            .iter()
                            .copied()
                            .filter(|p| !taken.contains(p) && ov(r, *p) > 0)
                            .max_by_key(|&p| (ov(r, p), std::cmp::Reverse(p)));
                        let id = match best {
                            Some(p) => {
                                taken.insert(p);
                                p
                            }
                            None => self.alloc(),
                        };
                        persistent[r as usize] = id;
                    }
                }
            }
        }

        (persistent, events)
    }
}

/// Segment the velocity field into coherent-velocity domains. Returns the per-cell
/// raw label (`0` = diffuse, below the size floor), the number of major domains, and
/// each domain's cell count (`new_size[raw]`, index 0 unused). Deterministic: raw
/// ids are handed out in cell order.
///
/// `prev` is the previous observation's labelling, and it carries the
/// hysteresis: a contact whose two cells rode the SAME plate last tick holds to
/// [`STICK`]× the base threshold, everything else must come down to [`JOIN`]×.
/// An empty / stale `prev` (first look, or a fresh topology) reads at exactly
/// `1.0×` — the first observation is unchanged. A steady field is a fixed
/// point: an edge inside a plate was under `1.0× ≤ STICK×` so it stays, an edge
/// that failed `1.0×` is above `JOIN×` so it stays out — identical labels, no
/// events, which is what `a_steady_world_is_all_continuation` pins.
fn segment(world: &World, prev: &[PlateId]) -> (Vec<u32>, usize, Vec<usize>) {
    let n = world.mantle.n_cells();
    if n == 0 {
        return (Vec::new(), 0, vec![0]);
    }
    let vel = &world.mantle.velocity;
    let mean_speed: f32 = vel.iter().map(|v| v.length()).sum::<f32>() / n as f32;
    // +ε so an all-still field is ONE plate, not n singletons.
    let thresh = COHERENCE * mean_speed + 1e-6;
    let sticky = prev.len() == n;
    segment_where(world, MIN_PLATE_CELLS, &|i, j| {
        let k = if !sticky {
            1.0
        } else if prev[i] != 0 && prev[i] == prev[j] {
            STICK
        } else {
            JOIN
        };
        (vel[i] - vel[j]).length() < k * thresh
    })
}

/// Grow domains out of whatever `couple` says holds two neighbours together, then
/// keep the ones big enough to be worth a name.
///
/// The **policy** is the caller's and the **mechanism** is here, because there are
/// two callers with genuinely different questions. This observer asks only whether
/// the flow is coherent — it is reading the velocity field. The conveyor
/// ([`crate::tectonics`]) also asks whether there is lithosphere to transmit the
/// stress, because it is about to move rock. One union-find, two policies; a second
/// copy of this would be two answers to "what is a plate".
pub(crate) fn segment_where(
    world: &World,
    min_cells: usize,
    couple: &dyn Fn(usize, usize) -> bool,
) -> (Vec<u32>, usize, Vec<usize>) {
    let n = world.mantle.n_cells();
    if n == 0 {
        return (Vec::new(), 0, vec![0]);
    }

    let mut uf = UnionFind::new(n);
    for i in 0..n {
        for &j in &world.grid.neighbors[i] {
            let j = j as usize;
            if j > i && couple(i, j) {
                uf.union(i, j);
            }
        }
    }

    let mut root_size = vec![0usize; n];
    for i in 0..n {
        root_size[uf.find(i)] += 1;
    }
    // Roots that clear the floor get a dense raw id (in cell order); the rest are
    // diffuse (0).
    let mut raw_of_root = vec![0u32; n];
    let mut new_size = vec![0usize]; // index 0 = diffuse
    for i in 0..n {
        let root = uf.find(i);
        if raw_of_root[root] == 0 && root_size[root] >= min_cells {
            new_size.push(root_size[root]);
            raw_of_root[root] = (new_size.len() - 1) as u32;
        }
    }
    let n_raw = new_size.len() - 1;
    let raw: Vec<u32> = (0..n).map(|i| raw_of_root[uf.find(i)]).collect();
    (raw, n_raw, new_size)
}

/// Classify each cell that borders a different plate by the two plates' relative
/// motion; interior cells (all neighbours same plate) stay `Interior`.
fn classify_seams(world: &World, labels: &[u32], drift_of: &BTreeMap<PlateId, Vec3>) -> Vec<Seam> {
    let n = labels.len();
    let dirs = &world.grid.dirs;
    let neighbors = &world.grid.neighbors;
    let mut seams = vec![Seam::Interior; n];
    for i in 0..n {
        let li = labels[i];
        if li == 0 {
            continue;
        }
        let di = drift_of.get(&li).copied().unwrap_or(Vec3::ZERO);
        let mut best = Seam::Interior;
        let mut best_mag = 0.0f32;
        for &j in &neighbors[i] {
            let j = j as usize;
            let lj = labels[j];
            if lj == 0 || lj == li {
                continue;
            }
            let dj = drift_of.get(&lj).copied().unwrap_or(Vec3::ZERO);
            let nrm = tangent_toward(dirs[i], dirs[j]); // unit i→j in the tangent plane
            let rel = di - dj; // plate i relative to plate j
            let conv = rel.dot(nrm); // >0: i closing on j; <0: pulling apart
            let shear = (rel - conv * nrm).length();
            let mag = rel.length();
            if mag <= best_mag {
                continue;
            }
            best_mag = mag;
            best = if conv.abs() < TRANSFORM_RATIO * shear {
                Seam::Transform
            } else if conv > 0.0 {
                Seam::Convergent
            } else {
                Seam::Divergent
            };
        }
        seams[i] = best;
    }
    seams
}

/// Weighted-union / path-compression disjoint sets over cells or bipartite nodes.
struct UnionFind {
    parent: Vec<u32>,
    size: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n as u32).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.parent[r] as usize != r {
            r = self.parent[r] as usize;
        }
        let mut c = x;
        while c != r {
            let next = self.parent[c] as usize;
            self.parent[c] = r as u32;
            c = next;
        }
        r
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big as u32;
        self.size[big] += self.size[small];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::interior::{MantleConvection, RadiogenicDecay};
    use crate::scheduler::Scheduler;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    fn world(freq: u32, seed: u64) -> World {
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        World::seed(icosphere(freq), b, &t, seed)
    }

    /// Drive convection so the velocity field has real structure.
    fn convected(freq: u32, seed: u64, ticks: usize) -> World {
        let mut w = world(freq, seed);
        let mut s = Scheduler::new(
            vec![
                Box::new(RadiogenicDecay::default()),
                Box::new(MantleConvection),
            ],
            seed,
        );
        for _ in 0..ticks {
            s.step(&mut w, 1.0, None);
        }
        w
    }

    /// Feed the observer a velocity field built to have TWO coherent domains, and
    /// check the mechanism finds them.
    ///
    /// Deliberately synthetic. This used to run the real sim at freq 6 and assert
    /// "more than one plate", which was an assertion about an emergent OUTCOME —
    /// and it was quietly leaning on the old projection's grid ghost: once the
    /// ISEA slice made cells equal-area, a barely-convecting young planet at 362
    /// cells honestly reads as ONE coherent domain and the assertion failed. The
    /// mechanism is what belongs in a test; how many plates a given world grows is
    /// Aaron's to watch, never a test's to require.
    #[test]
    fn segmentation_separates_domains_that_move_differently() {
        let mut w = world(6, 4);
        // Two hemispheres sliding opposite ways along their shared tangent frame:
        // coherent within, sharply discordant across the equator.
        for cell in 0..w.mantle.n_cells() {
            let dir = w.grid.dirs[cell];
            let east = Vec3::Y.cross(dir).normalize_or_zero();
            let sign = if dir.y >= 0.0 { 1.0 } else { -1.0 };
            w.mantle.velocity[cell] = east * sign;
        }
        let obs = PlateObserver::new().observe(&w);
        assert!(
            obs.plates.len() >= 2,
            "two discordant domains must segment apart, got {}",
            obs.plates.len()
        );
        assert!(obs.plates.len() < w.mantle.n_cells());
        // The discordant boundary is seen as a seam, not read as interior.
        assert!(
            obs.seams.iter().any(|s| *s != Seam::Interior),
            "the boundary between the domains produced no seam"
        );
    }

    #[test]
    fn plates_partition_the_surface_consistently() {
        let w = convected(6, 4, 20);
        let obs = PlateObserver::new().observe(&w);
        // Whatever it segments is the world's business; that the segmentation is
        // SELF-CONSISTENT is the observer's.
        assert!(obs.plates.len() < w.mantle.n_cells());
        // Labels and the roster agree: every labelled cell rides a listed plate.
        let ids: BTreeSet<PlateId> = obs.plates.iter().map(|p| p.id).collect();
        for &l in &obs.labels {
            if l != 0 {
                assert!(ids.contains(&l), "cell on an unlisted plate {l}");
            }
        }
        // First observation → every plate was Born.
        assert_eq!(
            obs.events
                .iter()
                .filter(|e| matches!(e, PlateEvent::Born(_)))
                .count(),
            obs.plates.len()
        );
    }

    #[test]
    fn observation_is_deterministic() {
        let run = || {
            let w = convected(6, 11, 20);
            PlateObserver::new().observe(&w)
        };
        let a = run();
        let b = run();
        assert_eq!(a.labels, b.labels, "same seed → identical labels");
        assert_eq!(a.events, b.events, "same seed → identical events");
    }

    #[test]
    fn a_steady_world_is_all_continuation() {
        // Observe the same unchanged world twice with ONE observer: the second pass
        // must reproduce the ids and flag no events (nothing moved).
        let w = convected(6, 3, 15);
        let mut obs = PlateObserver::new();
        let first = obs.observe(&w);
        let second = obs.observe(&w);
        assert_eq!(
            first.labels, second.labels,
            "identity is stable across a still tick"
        );
        assert!(
            second.events.is_empty(),
            "a steady world flags no events, got {:?}",
            second.events
        );
    }

    /// **The count does not flicker while the world does not change.** The
    /// mean-relative threshold parks the segmentation at its own percolation
    /// point, where per-tick numerical drift used to flip a handful of edges
    /// and jump the plate count 3→8→5→4 with nothing visibly moving. With the
    /// hysteresis band, a field jittered by far less than the band's width
    /// must keep the same plates — same count, same labels, no events.
    #[test]
    fn a_jittering_field_does_not_churn_the_plates() {
        let mut w = world(6, 9);
        let n = w.mantle.n_cells();
        // Two clean hemispheres — a genuinely two-plate world.
        let base = |w: &World, cell: usize| {
            let dir = w.grid.dirs[cell];
            let east = Vec3::Y.cross(dir).normalize_or_zero();
            east * if dir.y >= 0.0 { 1.0 } else { -1.0 }
        };
        for cell in 0..n {
            w.mantle.velocity[cell] = base(&w, cell);
        }
        let mut obs = PlateObserver::new();
        let first = obs.observe(&w);
        assert!(first.plates.len() >= 2, "the fixture segments at all");

        // Ten ticks of deterministic per-cell jitter, a couple of percent of
        // the flow — far inside the STICK/JOIN dead band, the scale of the
        // numerical drift the real run carries.
        for tick in 0..10u32 {
            for cell in 0..n {
                let h = (cell as u32)
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(tick * 97);
                let eps = ((h >> 8) % 1000) as f32 / 1000.0 - 0.5; // −0.5..0.5
                w.mantle.velocity[cell] = base(&w, cell) * (1.0 + 0.04 * eps);
            }
            let next = obs.observe(&w);
            assert_eq!(
                next.plates.len(),
                first.plates.len(),
                "tick {tick}: the count churned under jitter"
            );
            assert!(
                next.events.is_empty(),
                "tick {tick}: jitter is not a tectonic event: {:?}",
                next.events
            );
        }
    }

    #[test]
    fn birth_then_split_then_merge_are_detected() {
        // Craft the velocity field directly to force each transition on one observer.
        let mut w = world(4, 42);
        let n = w.mantle.n_cells();
        let mut obs = PlateObserver::new();

        // (1) Uniform flow → one plate, born.
        for i in 0..n {
            w.mantle.velocity[i] = Vec3::X;
        }
        let a = obs.observe(&w);
        assert_eq!(a.plates.len(), 1, "uniform flow is one plate");
        assert!(
            a.events.iter().any(|e| matches!(e, PlateEvent::Born(_))),
            "the plate was born"
        );

        // (2) Two opposed hemispheres → the plate splits in two.
        for i in 0..n {
            w.mantle.velocity[i] = if w.grid.dirs[i].x >= 0.0 {
                Vec3::X
            } else {
                -Vec3::X
            };
        }
        let b = obs.observe(&w);
        assert_eq!(b.plates.len(), 2, "opposed hemispheres are two plates");
        assert!(
            b.events
                .iter()
                .any(|e| matches!(e, PlateEvent::Split { .. })),
            "a split fired, got {:?}",
            b.events
        );
        // The seam between them is flagged (not all Interior).
        assert!(
            b.seams.iter().any(|&s| s != Seam::Interior),
            "the plate boundary is a seam"
        );

        // (3) Back to uniform → the two plates merge.
        for i in 0..n {
            w.mantle.velocity[i] = Vec3::X;
        }
        let c = obs.observe(&w);
        assert_eq!(c.plates.len(), 1, "the field is coherent again");
        assert!(
            c.events
                .iter()
                .any(|e| matches!(e, PlateEvent::Merged { .. })),
            "a merge fired, got {:?}",
            c.events
        );
    }
}
