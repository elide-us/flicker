//! **Epoch 3 — plate tectonic structuring** (world-gen spec).
//!
//! Partition the hex array into drifting **plates**, classify each hex's boundary
//! from its neighbours' plates and motion, and write a mean surface
//! **elevation** — continental plates ride high, oceanic plates sit low,
//! **convergent boundaries throw up mountain belts**, divergent boundaries open
//! rifts. This is where continents and mountains first appear; the elevation it
//! writes is the proto-heightmap the erosion sim later refines. Needs the hex
//! neighbour graph (from [`EpochCtx`]).

use glam::Vec3;

use crate::pipeline::{EpochCtx, EpochTransform};
use crate::state::{Boundary, HexState};

/// Epoch 3 parameters.
pub struct Epoch3 {
    /// Number of plates to seed.
    pub plates: usize,
    /// Fraction of plates that are continental (vs oceanic).
    pub continental_fraction: f32,
    /// Base elevation of a continental plate interior.
    pub continental_base: f32,
    /// Base elevation of an oceanic plate interior.
    pub oceanic_base: f32,
    /// Max uplift added at a fully-convergent boundary (mountains).
    pub mountain_uplift: f32,
    /// Max drop at a fully-divergent boundary (rift).
    pub rift_drop: f32,
    /// Closing-speed magnitude above which a boundary counts as convergent /
    /// divergent (below it is a transform fault).
    pub boundary_threshold: f32,
}

impl Default for Epoch3 {
    fn default() -> Self {
        Self {
            plates: 8,
            continental_fraction: 0.4,
            continental_base: 0.25,
            oceanic_base: -0.45,
            mountain_uplift: 0.6,
            rift_drop: 0.25,
            boundary_threshold: 0.15,
        }
    }
}

/// splitmix64 → `[0, 1)` for deterministic per-plate / per-seed randomness.
fn rand01(z: u64) -> f64 {
    let mut z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
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
        let plates = self.plates.max(1).min(n);

        // 1. Seed plates at distinct hexes, then grow them over the neighbour
        //    graph (multi-source BFS → each hex joins its nearest seed = a
        //    Voronoi partition over hex adjacency).
        let seeds = pick_seeds(n, plates, ctx.seed);
        let plate = grow_plates(n, &seeds, ctx.neighbors);

        // 2. Per-plate type (continental/oceanic) and a 3D drift direction, both
        //    deterministic from the seed.
        let mut continental = vec![false; plates];
        let mut motion = vec![Vec3::ZERO; plates];
        for (p, (cont, mot)) in continental.iter_mut().zip(motion.iter_mut()).enumerate() {
            let h = ctx.seed ^ (p as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
            *cont = (rand01(h) as f32) < self.continental_fraction;
            let theta = rand01(h ^ 0xABCD) as f32 * std::f32::consts::TAU;
            let drift_y = rand01(h ^ 0x1234) as f32 * 2.0 - 1.0;
            *mot = Vec3::new(theta.cos(), drift_y, theta.sin()).normalize_or_zero();
        }

        // 3. Per hex: base elevation by plate type; boundary by the strongest
        //    relative motion across a shared edge; add mountains / rifts.
        (0..n)
            .map(|i| {
                let mut s = prev[i].clone();
                let p = plate[i];
                s.plate = p as u16;
                s.continental = continental[p];
                let mut elev = if continental[p] {
                    self.continental_base
                } else {
                    self.oceanic_base
                };

                let mut boundary = Boundary::Interior;
                let mut strongest = 0.0f32; // signed: + closing, - opening
                for &nb in &ctx.neighbors[i] {
                    let q = plate[nb as usize];
                    if q == p {
                        continue;
                    }
                    // Direction across the edge on the sphere, and the plates'
                    // relative motion projected onto it. + = closing, - = opening.
                    let across = (ctx.dirs[nb as usize] - ctx.dirs[i]).normalize_or_zero();
                    let closing = -(motion[p] - motion[q]).dot(across);
                    if closing.abs() > strongest.abs() {
                        strongest = closing;
                    }
                }
                if strongest > self.boundary_threshold {
                    boundary = Boundary::Convergent;
                    let c = strongest.clamp(0.0, 1.0);
                    elev += self.mountain_uplift * c;
                    s.orogeny = c; // fold intensity → mountain relief in the field
                    s.volcanic = (s.volcanic + 0.5 * c).clamp(0.0, 1.0);
                } else if strongest < -self.boundary_threshold {
                    boundary = Boundary::Divergent;
                    elev -= self.rift_drop * (-strongest).clamp(0.0, 1.0);
                } else if strongest != 0.0 {
                    boundary = Boundary::Transform;
                }

                s.boundary = boundary;
                s.elevation = elev.clamp(-1.0, 1.0);
                s
            })
            .collect()
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
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
            Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
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
}
