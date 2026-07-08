//! **Epoch 3 — plate tectonic structuring** (world-gen spec).
//!
//! Partition the hex array into drifting **plates**, classify each hex's boundary
//! from its neighbours' plates and motion, and write a mean surface
//! **elevation**. The base elevation comes from **crust buoyancy** (isostasy):
//! the thick, light crust Epoch 2 floated up rides high (continents), the thin
//! dense crust sits low (ocean basins) — so continents appear *where Epoch 2's
//! light crust is* and Epoch 3 visibly continues Epoch 2. Plate boundaries then
//! carve on top: **convergent boundaries throw up mountain belts**, divergent
//! boundaries open rifts. The elevation it writes is the proto-heightmap the
//! erosion sim later refines. Needs the hex neighbour graph (from [`EpochCtx`]).

use std::cmp::Ordering;

use glam::Vec3;

use crate::pipeline::{EpochCtx, EpochTransform, NOMINAL_DURATION};
use crate::state::{Boundary, HexState};

/// Epoch 3 parameters.
pub struct Epoch3 {
    /// Number of plates to seed.
    pub plates: usize,
    /// Land/ocean balance: the fraction of the surface that reads continental. The
    /// most-buoyant `continental_fraction` of hexes (top crust-buoyancy percentile)
    /// are flagged continental; the rest oceanic. Same dial purpose as before (more
    /// → more land), but the *shapes* now follow the material (Epoch 2 crust), not a
    /// random per-plate roll.
    pub continental_fraction: f32,
    /// High end of the isostatic elevation ramp — the base elevation of the most
    /// buoyant (thickest, lightest) crust.
    pub continental_base: f32,
    /// Low end of the isostatic elevation ramp — the base elevation of the least
    /// buoyant (thinnest, densest) crust.
    pub oceanic_base: f32,
    /// Max uplift added at a fully-convergent boundary (mountains).
    pub mountain_uplift: f32,
    /// Max drop at a fully-divergent boundary (rift).
    pub rift_drop: f32,
    /// Closing-speed magnitude above which a boundary counts as convergent /
    /// divergent (below it is a transform fault).
    pub boundary_threshold: f32,
    /// Number of fixed mantle hotspots (plumes). `0` disables hotspot volcanism
    /// entirely — the default, so the base tectonics output is unchanged.
    pub hotspots: usize,
    /// Peak uplift a hotspot adds where a plate sits over it.
    pub hotspot_uplift: f32,
    /// Trail length of the volcanic chain the drifting plate leaves downstream of
    /// a hotspot (in unit-sphere chord units) — larger = longer island chains.
    pub hotspot_trail: f32,
    /// Oldest crust age in millions of years — the age a stable plate interior /
    /// continental craton reaches. Sea floor ages from ~0 at a divergent ridge up
    /// toward this at the far (subducting) margin.
    pub max_age: f32,
    /// Within-epoch **drift time** on the shared clock — how long the plates move.
    /// Displacement = rate × time, so more time accumulates taller convergent belts,
    /// deeper rifts, and **longer hotspot island chains** (the plate has drifted
    /// further over the plume). Normalised to [`NOMINAL_DURATION`], so the nominal
    /// value reproduces today's output.
    pub duration: u32,
}

impl Default for Epoch3 {
    fn default() -> Self {
        Self {
            plates: 8,
            continental_fraction: 0.4,
            // The isostatic ramp dominates the elevation: continents sit well above
            // sea level (`0` until the hydrosphere), ocean basins well below, so the
            // gross land/ocean pattern reads as the (inherited) crust and the
            // coastline falls at the continental threshold (see `apply`). Tectonic
            // deformation then rides on top.
            continental_base: 0.4,
            oceanic_base: -0.6,
            mountain_uplift: 0.6,
            rift_drop: 0.25,
            boundary_threshold: 0.15,
            hotspots: 0,
            hotspot_uplift: 0.6,
            hotspot_trail: 0.4,
            max_age: 200.0,
            duration: NOMINAL_DURATION,
        }
    }
}

/// A tectonic plate — the spec's cross-hex `plates` record: a stable id, its kind
/// (continental vs oceanic), its rigid drift over the sphere, and the hexes that
/// belong to it. Output of [`Epoch3::partition`], recorded alongside the per-hex
/// layer (the per-hex [`HexState::plate`] is the reverse mapping).
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    /// Stable plate id — also this plate's index in [`Partition::plates`].
    pub id: u16,
    /// Continental (rides high) vs oceanic (sits low) — a plate-level summary, set
    /// from the plate's **mean crust buoyancy** (the per-hex flag, used for crust
    /// age, is set from each hex's own buoyancy).
    pub continental: bool,
    /// Rigid-body drift direction over the unit sphere; the relative motion across
    /// a shared edge is what classifies a boundary convergent / divergent.
    pub motion: Vec3,
    /// Hex indices belonging to this plate.
    pub members: Vec<u32>,
}

/// The plate partition: each hex's plate index plus the cross-hex [`Plate`]
/// records. Produced by [`Epoch3::partition`]; [`Epoch3::apply`] runs the same
/// partition internally, so the per-hex layer and the cross-hex records agree.
#[derive(Clone, Debug, PartialEq)]
pub struct Partition {
    /// Per-hex plate index (`0..plates.len()`), indexed by hex.
    pub plate: Vec<usize>,
    /// One record per plate, indexed by plate id.
    pub plates: Vec<Plate>,
}

/// Cross-trail half-width of a hotspot chain (unit-sphere chord units).
const HOTSPOT_WIDTH: f32 = 0.07;

/// Fraction of convergent uplift a *fully oceanic* hex gets — a modest island arc.
/// Buoyant continental crust scales up to the full belt, so mountains build the
/// inherited continents rather than spawning random mid-ocean land (the thing that
/// made Epoch 3 read as disconnected from Epoch 2).
const OCEANIC_OROGENY: f32 = 0.25;

/// splitmix64 → `[0, 1)` for deterministic per-plate / per-seed randomness.
fn rand01(z: u64) -> f64 {
    let mut z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

impl Epoch3 {
    /// Partition the hexes into drifting plates: grow a Voronoi partition over the
    /// neighbour graph from deterministic seed hexes, then give each plate a rigid
    /// 3D drift and a continental/oceanic kind from its **mean crust buoyancy**
    /// ([`buoyancy_ranks`] of `prev`). Returns the per-hex assignment and the
    /// cross-hex [`Plate`] records (the spec's `plates` structure). Deterministic
    /// from the seed + upstream crust, so calling it to **record** the cross-hex
    /// output yields the same partition [`apply`](Self::apply) uses internally.
    pub fn partition(&self, ctx: &EpochCtx, prev: &[HexState]) -> Partition {
        let n = ctx.dirs.len();
        if n == 0 {
            return Partition { plate: Vec::new(), plates: Vec::new() };
        }
        // Seed plates at distinct hexes, then grow them over the neighbour graph
        // (multi-source BFS → each hex joins its nearest seed = a Voronoi
        // partition over hex adjacency).
        let k = self.plates.max(1).min(n);
        let seeds = pick_seeds(n, k, ctx.seed);
        let plate = grow_plates(n, &seeds, ctx.neighbors);

        // Crust buoyancy ranked across the planet, and the per-plate mean — a plate
        // reads continental when its average crust is in the buoyant band.
        let ranks = buoyancy_ranks(prev);
        let cont_threshold = 1.0 - self.continental_fraction.clamp(0.0, 1.0);
        let mut rank_sum = vec![0.0f32; k];
        let mut count = vec![0u32; k];
        for (i, &p) in plate.iter().enumerate() {
            rank_sum[p] += ranks.get(i).copied().unwrap_or(0.0);
            count[p] += 1;
        }

        // Per-plate kind (from mean buoyancy) + a 3D drift direction (deterministic
        // from the seed).
        let mut plates: Vec<Plate> = (0..k)
            .map(|p| {
                let mean_rank = if count[p] > 0 { rank_sum[p] / count[p] as f32 } else { 0.0 };
                let continental = mean_rank >= cont_threshold;
                let h = ctx.seed ^ (p as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
                let theta = rand01(h ^ 0xABCD) as f32 * std::f32::consts::TAU;
                let drift_y = rand01(h ^ 0x1234) as f32 * 2.0 - 1.0;
                let motion = Vec3::new(theta.cos(), drift_y, theta.sin()).normalize_or_zero();
                Plate { id: p as u16, continental, motion, members: Vec::new() }
            })
            .collect();
        for (i, &p) in plate.iter().enumerate() {
            plates[p].members.push(i as u32);
        }
        Partition { plate, plates }
    }
}

/// Per-hex **crust buoyancy** ranked in `0..1` across the planet: each hex's
/// `crust_fraction` (Epoch 2 — how much light crust floated up, a crust-thickness
/// proxy) turned into its percentile rank. Thick light crust → rank near 1 (rides
/// high, continents); thin dense crust → rank near 0 (sits low, ocean basins).
/// Ranking rather than an absolute min-max guarantees a full continent↔ocean
/// spread even when the absolute crust-fraction spread is thin — the same
/// percentile trick Epoch 4 uses to set sea level. This is the signal Epoch 3
/// turns into base elevation, so continents land where the light crust is.
fn buoyancy_ranks(prev: &[HexState]) -> Vec<f32> {
    let n = prev.len();
    if n == 0 {
        return Vec::new();
    }
    // Sort hex indices by crust buoyancy ascending; ties broken by index so the
    // ranking (and thus the whole epoch) stays deterministic.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        prev[a]
            .crust_fraction
            .partial_cmp(&prev[b].crust_fraction)
            .unwrap_or(Ordering::Equal)
            .then(a.cmp(&b))
    });
    let denom = (n - 1).max(1) as f32;
    let mut rank = vec![0.0f32; n];
    for (pos, &i) in order.iter().enumerate() {
        rank[i] = pos as f32 / denom;
    }
    rank
}

impl EpochTransform for Epoch3 {
    fn epoch(&self) -> u8 {
        3
    }

    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        let n = prev.len();
        if n == 0 {
            return Vec::new();
        }

        // 1. Plate partition + per-plate kind/drift — the cross-hex `plates`
        //    structure. (Recomputed by `partition` when a caller wants to record
        //    it; deterministic, so the two always agree.)
        let part = self.partition(ctx, prev);

        // Crust buoyancy ranked across the planet: the isostatic base elevation and
        // the per-hex continental flag both read off it (continents where the light
        // crust floated up — Epoch 2 → Epoch 3 continuity).
        let ranks = buoyancy_ranks(prev);
        let cont_threshold = 1.0 - self.continental_fraction.clamp(0.0, 1.0);

        // Fixed mantle hotspots (in the mantle frame). A plate drifting over one
        // leaves an uplifted volcanic chain trailing along its motion.
        let hotspots = make_hotspots(self.hotspots, ctx.seed);

        // Within-epoch drift time: motion is a *rate*, so accumulated deformation
        // (uplift / rift) and the distance a plate has carried off a hotspot both
        // scale with elapsed time. Normalised to the nominal duration, so the
        // nominal value leaves the rate as-is (today's output).
        let drift = self.duration as f32 / NOMINAL_DURATION as f32;

        // 2. Per hex: base elevation by crust buoyancy (isostasy); boundary by the
        //    strongest relative motion across a shared edge; add mountains / rifts /
        //    hotspots on top.
        let mut out: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = prev[i].clone();
                let p = part.plate[i];
                s.plate = p as u16;
                let motion_p = part.plates[p].motion;
                // Isostatic base: lerp the elevation ramp by this hex's own crust
                // buoyancy. A hex reads continental when its buoyancy is in the top
                // `continental_fraction` band (so the per-hex flag follows the local
                // crust, while the plate's flag follows its mean).
                let t = ranks[i];
                s.continental = t >= cont_threshold;
                let mut elev = self.oceanic_base + (self.continental_base - self.oceanic_base) * t;

                let mut boundary = Boundary::Interior;
                let mut strongest = 0.0f32; // signed: + closing, - opening
                for &nb in &ctx.neighbors[i] {
                    let q = part.plate[nb as usize];
                    if q == p {
                        continue;
                    }
                    // Direction across the edge on the sphere, and the plates'
                    // relative motion projected onto it. + = closing, - = opening.
                    let across = (ctx.dirs[nb as usize] - ctx.dirs[i]).normalize_or_zero();
                    let closing = -(motion_p - part.plates[q].motion).dot(across);
                    if closing.abs() > strongest.abs() {
                        strongest = closing;
                    }
                }
                // Boundary *type* is the time-independent sign of relative motion;
                // the deformation *magnitude* accumulates with drift time.
                if strongest > self.boundary_threshold {
                    boundary = Boundary::Convergent;
                    let c = (strongest * drift).clamp(0.0, 1.0);
                    // Orogeny rides on buoyant crust: continental collision throws up
                    // the full belt, ocean-ocean convergence only an island arc — so
                    // mountains reinforce the inherited continents instead of
                    // overriding the crust-driven land/ocean pattern.
                    let continentality = OCEANIC_OROGENY + (1.0 - OCEANIC_OROGENY) * t;
                    let belt = c * continentality;
                    elev += self.mountain_uplift * belt;
                    s.orogeny = belt; // fold intensity → mountain relief in the field
                    s.volcanic = (s.volcanic + 0.5 * c).clamp(0.0, 1.0);
                } else if strongest < -self.boundary_threshold {
                    boundary = Boundary::Divergent;
                    elev -= self.rift_drop * (-strongest * drift).clamp(0.0, 1.0);
                } else if strongest != 0.0 {
                    boundary = Boundary::Transform;
                }

                // Hotspot volcanism: uplift trailing the plate's motion past each
                // plume (a comet shape — a blob at the plume, a long tail
                // downstream → an island/seamount chain).
                if !hotspots.is_empty() && self.hotspot_uplift > 0.0 {
                    let cell = ctx.dirs[i];
                    // Plate drift projected into the cell's tangent plane.
                    let m_t = (motion_p - motion_p.dot(cell) * cell).normalize_or_zero();
                    // Longer drift time → the plate has carried the cell further off
                    // the plume → a longer island/seamount chain.
                    let trail = (self.hotspot_trail * drift).max(1e-3);
                    let mut lift = 0.0f32;
                    for &hs in &hotspots {
                        let off = cell - hs;
                        let along = off.dot(m_t);
                        let cross = (off - along * m_t).length();
                        // Long tail downstream (+motion), short falloff upstream.
                        let an = if along >= 0.0 { along / trail } else { -along / HOTSPOT_WIDTH };
                        let cn = cross / HOTSPOT_WIDTH;
                        lift += (-(an * an + cn * cn)).exp();
                    }
                    elev += self.hotspot_uplift * lift;
                    s.volcanic = (s.volcanic + 0.6 * lift.min(1.0)).clamp(0.0, 1.0);
                }

                s.boundary = boundary;
                s.elevation = elev.clamp(-1.0, 1.0);
                s
            })
            .collect();

        // 3. Crust age: sea floor is created at the divergent ridges and ages with
        //    distance away from them (toward the subducting margins); continental
        //    crust is old, stable craton. BFS the hop distance from every ridge.
        let dist = ridge_distance(n, &out, ctx.neighbors);
        let max_d = dist.iter().copied().filter(|&d| d != u32::MAX).max().unwrap_or(0);
        for (i, s) in out.iter_mut().enumerate() {
            let d_norm = if max_d == 0 {
                1.0
            } else {
                match dist[i] {
                    u32::MAX => 1.0,
                    d => d as f32 / max_d as f32,
                }
            };
            s.plate_age = if s.continental {
                self.max_age * (0.6 + 0.4 * d_norm)
            } else {
                self.max_age * d_norm
            };
        }

        out
    }
}

/// Pick up to `k` distinct seed hexes from `0..n`, deterministically.
fn pick_seeds(n: usize, k: usize, seed: u64) -> Vec<usize> {
    let mut seeds = Vec::with_capacity(k);
    let mut tries = 0u64;
    while seeds.len() < k && tries < (k as u64 + 1) * 1000 {
        let idx = ((rand01(seed ^ tries.wrapping_mul(0x2545_F491_4F6C_DD1D)) * n as f64) as usize).min(n - 1);
        if !seeds.contains(&idx) {
            seeds.push(idx);
        }
        tries += 1;
    }
    seeds
}

/// `k` mantle hotspots spread uniformly over the unit sphere, deterministic from
/// `seed`. `y` (= sin latitude) uniform gives an equal-area spread.
fn make_hotspots(k: usize, seed: u64) -> Vec<Vec3> {
    (0..k)
        .map(|h| {
            let a = seed ^ (h as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let y = rand01(a) as f32 * 2.0 - 1.0;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let phi = rand01(a ^ 0x5151_5151) as f32 * std::f32::consts::TAU;
            Vec3::new(r * phi.cos(), y, r * phi.sin())
        })
        .collect()
}

/// Multi-source BFS giving each hex's hop distance to the nearest **divergent**
/// boundary (a spreading ridge), or `u32::MAX` if no ridge is reachable. Drives
/// the sea-floor age gradient.
fn ridge_distance(n: usize, states: &[HexState], neighbors: &[Vec<u32>]) -> Vec<u32> {
    let mut dist = vec![u32::MAX; n];
    let mut frontier = Vec::new();
    for (i, s) in states.iter().enumerate() {
        if s.boundary == Boundary::Divergent {
            dist[i] = 0;
            frontier.push(i);
        }
    }
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for &h in &frontier {
            let d = dist[h] + 1;
            for &nb in &neighbors[h] {
                let nb = nb as usize;
                if nb < n && dist[nb] == u32::MAX {
                    dist[nb] = d;
                    next.push(nb);
                }
            }
        }
        frontier = next;
    }
    dist
}

/// Multi-source BFS over the neighbour graph: each hex's plate is the nearest
/// seed by graph distance (ties go to whichever wavefront reached it first).
fn grow_plates(n: usize, seeds: &[usize], neighbors: &[Vec<u32>]) -> Vec<usize> {
    let mut plate = vec![usize::MAX; n];
    let mut frontier = Vec::new();
    for (p, &s) in seeds.iter().enumerate() {
        if plate[s] == usize::MAX {
            plate[s] = p;
            frontier.push(s);
        }
    }
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for &h in &frontier {
            let p = plate[h];
            for &nb in &neighbors[h] {
                let nb = nb as usize;
                if nb < n && plate[nb] == usize::MAX {
                    plate[nb] = p;
                    next.push(nb);
                }
            }
        }
        frontier = next;
    }
    // Any hex unreached (disconnected) falls to plate 0.
    for p in plate.iter_mut() {
        if *p == usize::MAX {
            *p = 0;
        }
    }
    plate
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldstate::Composition;

    use crate::state::HexState;

    /// A ring of `n` hexes (each neighbours its two ring-mates) with directions
    /// spread around the equator — a minimal connected graph for the plate logic.
    fn ring(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let dirs = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors = (0..n)
            .map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32])
            .collect();
        (dirs, neighbors)
    }

    fn ctx_for<'a>(dirs: &'a [Vec3], neighbors: &'a [Vec<u32>]) -> EpochCtx<'a> {
        // Epoch 3 never reads the tables, so a throwaway empty one is fine — but
        // the field requires a reference, so load the real one.
        EpochCtx { tables: tables_leak(), dirs, neighbors, seed: 2024 }
    }

    fn tables_leak() -> &'static flicker_materials::Tables {
        use flicker_materials::{JsonTableSource, Tables};
        use std::sync::OnceLock;
        static T: OnceLock<Tables> = OnceLock::new();
        T.get_or_init(|| {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
            Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
        })
    }

    #[test]
    fn plates_partition_every_hex() {
        let n = 30;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let out = Epoch3::default().apply(&ctx, &prev);
        assert_eq!(out.len(), n);
        for s in &out {
            assert!((s.plate as usize) < Epoch3::default().plates, "plate id out of range");
            assert!(s.elevation.is_finite() && (-1.0..=1.0).contains(&s.elevation));
        }
        // Multiple plates on a ring ⇒ real boundaries and a spread of elevations.
        let mut elevations: Vec<f32> = out.iter().map(|s| s.elevation).collect();
        elevations.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(elevations.last().unwrap() - elevations[0] > 0.1, "elevation is flat — no tectonics");
        assert!(out.iter().any(|s| s.boundary != Boundary::Interior), "no plate boundaries formed");
    }

    #[test]
    fn orogeny_only_on_convergent_boundaries() {
        let n = 30;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let out = Epoch3::default().apply(&ctx, &prev);
        for s in &out {
            match s.boundary {
                Boundary::Convergent => assert!(s.orogeny > 0.0, "convergent hex has no orogeny"),
                _ => assert_eq!(s.orogeny, 0.0, "orogeny off a convergent boundary"),
            }
        }
    }

    #[test]
    fn deterministic_for_a_seed() {
        let n = 24;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let a = Epoch3::default().apply(&ctx, &prev);
        let b = Epoch3::default().apply(&ctx, &prev);
        assert_eq!(a, b);
    }

    #[test]
    fn partition_records_drifting_plates_covering_every_hex() {
        let n = 40;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let part = Epoch3::default().partition(&ctx, &prev);

        assert_eq!(part.plates.len(), Epoch3::default().plates, "one record per plate");
        let mut seen = 0usize;
        for (p, plate) in part.plates.iter().enumerate() {
            assert_eq!(plate.id as usize, p, "plate id matches its index");
            assert!((plate.motion.length() - 1.0).abs() < 1e-3, "plate drift isn't a unit vector");
            seen += plate.members.len();
        }
        // Membership partitions the hexes: every hex in exactly one plate.
        assert_eq!(seen, n, "plate membership doesn't cover every hex once");
        for (i, &p) in part.plate.iter().enumerate() {
            assert!(part.plates[p].members.contains(&(i as u32)), "hex missing from its plate");
        }
        // The per-hex layer agrees with the cross-hex record it was built from.
        let out = Epoch3::default().apply(&ctx, &prev);
        for (i, s) in out.iter().enumerate() {
            assert_eq!(s.plate as usize, part.plate[i], "layer disagrees with the partition");
        }
    }

    #[test]
    fn more_drift_time_accumulates_more_relief() {
        let n = 60;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        // Positive relief: convergent uplift + hotspot chains. With hotspots on,
        // longer drift time grows the chains and the belts.
        let relief = |layer: &[HexState]| layer.iter().map(|s| s.elevation.max(0.0)).sum::<f32>();
        let young = Epoch3 { hotspots: 6, hotspot_uplift: 1.0, duration: 1, ..Epoch3::default() }
            .apply(&ctx, &prev);
        let old = Epoch3 { hotspots: 6, hotspot_uplift: 1.0, duration: 10, ..Epoch3::default() }
            .apply(&ctx, &prev);
        assert!(
            relief(&old) > relief(&young),
            "more drift time should accumulate more relief ({} vs {})",
            relief(&old),
            relief(&young)
        );
    }

    #[test]
    fn plate_age_is_young_at_ridges_and_old_on_continents() {
        let n = 48;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let e3 = Epoch3::default();
        let out = e3.apply(&ctx, &prev);

        // Ages are finite and bounded by max_age.
        assert!(out
            .iter()
            .all(|s| s.plate_age.is_finite() && (0.0..=e3.max_age + 1.0).contains(&s.plate_age)));
        // Continental crust is old (cratonic) — at least 0.6 of the max age.
        for s in &out {
            if s.continental {
                assert!(s.plate_age >= 0.6 * e3.max_age - 1.0, "continental crust read too young");
            }
        }
        // Where an oceanic spreading ridge formed, the crust on it is the youngest
        // and the field grades older away from it.
        let oceanic_ridge: Vec<f32> = out
            .iter()
            .filter(|s| s.boundary == Boundary::Divergent && !s.continental)
            .map(|s| s.plate_age)
            .collect();
        if !oceanic_ridge.is_empty() {
            let ridge_age = oceanic_ridge.iter().copied().fold(f32::MAX, f32::min);
            let oldest = out.iter().map(|s| s.plate_age).fold(f32::MIN, f32::max);
            assert!(oldest > ridge_age, "no crust-age gradient away from the ridge");
        }
    }

    /// The isostasy continuity guarantee: base elevation rises with Epoch 2's crust
    /// buoyancy, so continents land where the light crust floated up (Epoch 2 →
    /// Epoch 3 visibly continues). Boundary/hotspot relief is switched off so the
    /// elevation *is* the buoyancy base, with a uniform single-element crust so the
    /// buoyancy ranks purely by thickness.
    #[test]
    fn elevation_follows_crust_buoyancy() {
        let n = 40;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        // Ascending crust fraction; same (light) crust material everywhere.
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14, 1000.0)]); // silica
                s.crust_fraction = i as f64 / (n - 1) as f64;
                s
            })
            .collect();
        let e3 = Epoch3 { mountain_uplift: 0.0, rift_drop: 0.0, hotspots: 0, ..Epoch3::default() };
        let out = e3.apply(&ctx, &prev);

        // Elevation is monotone non-decreasing in crust buoyancy (fed ascending).
        for w in out.windows(2) {
            assert!(
                w[1].elevation >= w[0].elevation - 1e-6,
                "elevation should rise with crust buoyancy ({} -> {})",
                w[0].elevation,
                w[1].elevation
            );
        }
        // The least-buoyant hex bottoms out at the ocean base, the most-buoyant tops
        // out at the continental base — a full continent↔ocean spread.
        assert!((out[0].elevation - e3.oceanic_base).abs() < 1e-5, "least-buoyant hex isn't ocean floor");
        assert!(
            (out[n - 1].elevation - e3.continental_base).abs() < 1e-5,
            "most-buoyant hex isn't continental high"
        );
        // And the buoyant end reads continental, the dense end oceanic.
        assert!(out[n - 1].continental, "most-buoyant hex should read continental");
        assert!(!out[0].continental, "least-buoyant hex should read oceanic");
    }

    /// The repurposed `continental_fraction` knob is a land/ocean balance: it sets
    /// roughly what fraction of the surface reads continental, and the shapes follow
    /// the crust (the most-buoyant hexes), not a random roll.
    #[test]
    fn continental_fraction_sets_the_land_share() {
        let n = 50;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14, 1000.0)]);
                s.crust_fraction = i as f64 / (n - 1) as f64;
                s
            })
            .collect();
        let land = |frac: f32| {
            let e3 = Epoch3 { continental_fraction: frac, ..Epoch3::default() };
            e3.apply(&ctx, &prev).iter().filter(|s| s.continental).count()
        };
        // More balance → more land, and the share tracks the knob (±a hex of rounding).
        assert!(land(0.2) < land(0.5) && land(0.5) < land(0.8), "land share should track the knob");
        let quarter = land(0.25);
        assert!((quarter as i32 - n as i32 / 4).abs() <= 2, "≈a quarter should read continental, got {quarter}");
    }
}
