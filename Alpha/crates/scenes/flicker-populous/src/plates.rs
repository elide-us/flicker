//! **The tectonic shell: continents and ocean beds, drawn before they move.**
//!
//! The plates are the upper crust — the shell the erosion-era simulation will
//! actually evolve: ~20 rounds of plate movement, mountain building at the
//! collisions, oceans splitting continents. That phase STARTS from a solid,
//! already-defined plate scheme instead of accreting one over eight hours the
//! way God Mode does — which is this module: N plates tiled over the sphere,
//! each a continent or an ocean bed, with the plate boundaries between them.
//!
//! **Deliberately UNRELATED to the molten layers below (Aaron 2026-08-25):**
//! the plate scheme is its OWN roll — its own seed, its own cells, its own
//! seams. The molten seams under it matter later, when the simulation pushes
//! mountains up and splits valleys along them; they play no part in where the
//! plates themselves stand today.
//!
//! **Transformation, not outcome (rule 935269B7):** nothing here places a
//! continent. The plate seeds are random, membership is nearest-seed geometry,
//! a plate's KIND is a seeded draw, and the boundaries are wherever adjacency
//! disagrees. The editorial controls are the count and the re-roll.

use flicker::render::Vec3;

use crate::map::{HexMap, TileId};

/// The fewest plates the dial offers — a couple of vast shields.
pub const MIN_PLATES: u32 = 4;
/// The most — a mosaic of small plates.
pub const MAX_PLATES: u32 = 24;
/// Where the bench opens — about an Earth's worth of majors and minors.
pub const DEFAULT_PLATES: u32 = 12;

/// The share of plates that carry a CONTINENT rather than an ocean bed —
/// Earth-flavoured: most of the shell is sea floor.
const CONTINENT_FRAC: f32 = 0.35;

/// The plate shell's BASE heights per kind, in tile-width units — a continent
/// is THICK crust riding high, an ocean bed a thin veneer. The scheme's OWN
/// physicality: the views draw it, and the evolution era's ground ledger is
/// seeded from it.
#[allow(dead_code)] // the plates display tier — the era's LIVE class owns the line now
pub const CONTINENT_H_FRAC: f32 = 0.5;
pub const OCEAN_BED_H_FRAC: f32 = 0.125;
/// The SHELF band's base height — between bed and continent, so the era's
/// tick-zero map carries the same sandy margin the plates tab shows.
// The scheme's shelf band height is a display fact of the plates TAB only
// now — the era starts from bare floor (Aaron 2026-08-25: land is what the
// upwelling builds).
#[allow(dead_code)]
pub const SHELF_H_FRAC: f32 = 0.32;

// ── the LIQUID-FLOW warp (Aaron 2026-08-25: straight Voronoi edges are
// wrong — even "solid" crust flows and ebbs at geological scale, and the
// shapes should express a planet spinning on an axis) ──
/// The warp's OCTAVES: (waves, freq_min, freq_span, total amplitude in
/// radians). Energy at EVERY scale is the point — the first pass carried only
/// sub-edge frequencies and merely translated the polygons (Aaron: "you
/// literally did nothing"): an edge only stops reading as a line when the
/// warp has structure SMALLER than the edge. Bottom octave: continental
/// lobes; top octave: few-tile crinkle — bays, capes, ragged coasts.
const WARP_OCTAVES: [(usize, f32, f32, f32); 4] = [
    (4, 1.5, 2.0, 0.16),    // lobes
    (5, 5.0, 4.0, 0.09),    // wobble
    (5, 14.0, 10.0, 0.05),  // roughness
    (6, 60.0, 80.0, 0.028), // tile-scale crinkle (~±2 tiles)
];
/// The SPIN signature: a differential-rotation twist about the +Y axis,
/// radians of longitude at the poles (zero at the equator) — the east-west
/// smear a rotating body works into everything that flows on it.
const SPIN_SHEAR: f32 = 0.35;
/// The warp stream's offset off the field's one roll — its own stream, so the
/// plate-count dial never moves the warp and the warp never shifts the plate
/// records' prefix.
const WARP_STREAM: u64 = 0x2545_F491_4F6C_DD1D;

// ── the SHELF's varying width (Aaron: "no variation on the sandy edges, it's
// just plain lines" — a real margin swells and pinches) ──
/// Waves of the shelf-width field, and the width in RINGS it modulates:
/// width(t) = MEAN + VAR·field(t), field saturating in [−1, 1] — so the band
/// runs from a pinched single edge ring up to ~4 tiles of margin.
const SHELF_WAVES: usize = 3;
const SHELF_FREQ_MIN: f32 = 5.0;
const SHELF_FREQ_SPAN: f32 = 9.0;
const SHELF_WIDTH_MEAN: f32 = 1.6;
const SHELF_WIDTH_VAR: f32 = 1.6;
/// How many rings out from a kind edge the shelf can ever reach.
const SHELF_MAX_RINGS: u8 = 4;

/// One wave of the warp field: a vector amplitude riding a great-circle
/// sinusoid.
struct Wave {
    axis: Vec3,
    amp: Vec3,
    freq: f32,
    phase: f32,
}

/// **The plate scheme.** N plate seeds, every tile's plate membership, each
/// plate's kind, and the boundary tiles between plates — the state the
/// erosion era will start evolving from.
pub struct PlateField {
    /// How many plates were asked for, clamped to the offered range.
    plates: u32,
    /// The roll that placed the plates — INDEPENDENT of the molten field's.
    seed: u64,
    /// The plate seeds: unit directions on the sphere.
    seeds: Vec<Vec3>,
    /// Whether each plate carries a continent (`true`) or an ocean bed.
    continental: Vec<bool>,
    /// Per-tile plate membership, indexed by `TileId`.
    home: Vec<u32>,
    /// Per-tile boundary flag: a tile any of whose neighbours belongs to a
    /// different plate — where adjacency disagrees about MEMBERSHIP. Unpainted
    /// data: the evolution era's collision sites, not a surface look.
    boundary: Vec<bool>,
    /// Per-tile SHELF flag: a tile any of whose neighbours is the other KIND —
    /// the transitional zone where a bed meets a continent (Aaron 2026-08-25:
    /// the only edge the surface marks; bed–bed and land–land joins show
    /// nothing). Two tiles wide today; erosion cycles will widen it with
    /// sediment roll-off later.
    shelf: Vec<bool>,
    /// The liquid-flow warp field — its own stream of the roll, fixed size.
    waves: Vec<Wave>,
    /// The shelf-width field's waves (scalar: only `amp.x` is read).
    shelf_waves: Vec<Wave>,
}

impl PlateField {
    /// Roll a scheme of `plates` with `seed` over `map`.
    pub fn new(map: &HexMap, plates: u32, seed: u64) -> Self {
        let mut field = Self {
            plates: plates.clamp(MIN_PLATES, MAX_PLATES),
            seed,
            seeds: Vec::new(),
            continental: Vec::new(),
            home: Vec::new(),
            boundary: Vec::new(),
            shelf: Vec::new(),
            waves: Vec::new(),
            shelf_waves: Vec::new(),
        };
        field.rebuild(map);
        field
    }

    /// How many plates the scheme was rolled with.
    pub fn plates(&self) -> u32 {
        self.plates
    }

    /// The roll that placed the plates.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Which plate `tile` belongs to. Out-of-range asks read as plate 0.
    pub fn home(&self, tile: TileId) -> u32 {
        self.home.get(tile as usize).copied().unwrap_or(0)
    }

    /// Whether `tile` stands on a CONTINENT (`true`) or an ocean bed.
    pub fn is_continent(&self, tile: TileId) -> bool {
        self.continental
            .get(self.home(tile) as usize)
            .copied()
            .unwrap_or(false)
    }

    /// Whether `tile` stands on a plate boundary — a neighbour belongs to a
    /// different plate. DATA for the evolution era (collisions happen here);
    /// the surface does not paint it.
    pub fn is_boundary(&self, tile: TileId) -> bool {
        self.boundary.get(tile as usize).copied().unwrap_or(false)
    }

    /// Whether `tile` stands on CONTINENTAL SHELF — the transitional zone
    /// where a bed meets a continent (a neighbour is the other kind). The one
    /// edge the surface marks.
    pub fn is_shelf(&self, tile: TileId) -> bool {
        self.shelf.get(tile as usize).copied().unwrap_or(false)
    }

    /// Each plate's kind, by plate id — for legends and tests.
    pub fn kinds(&self) -> &[bool] {
        &self.continental
    }

    /// Each plate's seed direction, by plate id — where a motion arrow or a
    /// label stands for the plate.
    pub fn seed_dirs(&self) -> &[Vec3] {
        &self.seeds
    }

    /// Re-roll the whole scheme (a new random shell) over the same map.
    pub fn randomize(&mut self, map: &HexMap) {
        self.seed = fastrand::u64(..);
        self.rebuild(map);
    }

    /// Change the plate count, keeping the roll — the same prefix law as the
    /// molten dials: the first `n` seeds of the same sequence. A no-op at the
    /// current count.
    pub fn set_plates(&mut self, map: &HexMap, plates: u32) {
        let plates = plates.clamp(MIN_PLATES, MAX_PLATES);
        if plates == self.plates {
            return;
        }
        self.plates = plates;
        self.rebuild(map);
    }

    /// Where a tile LOOKS UP its plate: the tile's direction bent through the
    /// liquid-flow field (smooth vector waves) and then twisted by the spin
    /// shear — so plate edges come out as flowing lobes with an east-west
    /// smear, never great-circle straights. Membership geometry only: the
    /// tiles themselves never move.
    fn warp(&self, p: Vec3) -> Vec3 {
        let mut v = Vec3::ZERO;
        for w in &self.waves {
            v += w.amp * (w.freq * p.dot(w.axis) + w.phase).sin();
        }
        let q = (p + v).normalize_or_zero();
        // Differential rotation about +Y: no twist at the equator, full at
        // the poles — the signature of a body that spins.
        let ang = SPIN_SHEAR * q.y;
        let (sn, cs) = ang.sin_cos();
        Vec3::new(cs * q.x + sn * q.z, q.y, -sn * q.x + cs * q.z)
    }

    /// The map was rebuilt — derive the scheme for the new tiling from the
    /// SAME roll: the plates do not move when the map does.
    pub fn rebuild(&mut self, map: &HexMap) {
        // Each plate is ONE record of the stream — position and kind drawn
        // together — so the count dial keeps both the positions AND the kinds
        // of the plates it already had (the prefix law; drawing all kinds
        // after all seeds broke it, because the kinds' offset moved with the
        // count).
        let mut rng = fastrand::Rng::with_seed(self.seed);
        self.seeds.clear();
        self.continental.clear();
        for _ in 0..self.plates {
            let z = rng.f32() * 2.0 - 1.0;
            let a = rng.f32() * std::f32::consts::TAU;
            let r = (1.0 - z * z).max(0.0).sqrt();
            self.seeds.push(Vec3::new(r * a.cos(), z, r * a.sin()));
            self.continental.push(rng.f32() < CONTINENT_FRAC);
        }

        // The warp field rides its OWN stream of the roll: the count dial
        // neither moves it nor is moved by it.
        let mut wr = fastrand::Rng::with_seed(self.seed.wrapping_add(WARP_STREAM));
        let unit = |r: &mut fastrand::Rng| {
            let z = r.f32() * 2.0 - 1.0;
            let a = r.f32() * std::f32::consts::TAU;
            let rr = (1.0 - z * z).max(0.0).sqrt();
            Vec3::new(rr * a.cos(), z, rr * a.sin())
        };
        self.waves.clear();
        for (count, fmin, fspan, total) in WARP_OCTAVES {
            for _ in 0..count {
                self.waves.push(Wave {
                    axis: unit(&mut wr),
                    amp: unit(&mut wr) * (total * (0.5 + wr.f32()) * 2.0 / count as f32),
                    freq: fmin + wr.f32() * fspan,
                    phase: wr.f32() * std::f32::consts::TAU,
                });
            }
        }
        self.shelf_waves = (0..SHELF_WAVES)
            .map(|_| Wave {
                axis: unit(&mut wr),
                amp: Vec3::new(0.5 + wr.f32() * 0.5, 0.0, 0.0),
                freq: SHELF_FREQ_MIN + wr.f32() * SHELF_FREQ_SPAN,
                phase: wr.f32() * std::f32::consts::TAU,
            })
            .collect();

        let dirs = &map.grid().dirs;
        self.home = dirs
            .iter()
            .map(|d| {
                let p = self.warp(*d);
                let mut best = (f32::MIN, 0u32);
                for (i, s) in self.seeds.iter().enumerate() {
                    let dot = p.dot(*s);
                    if dot > best.0 {
                        best = (dot, i as u32);
                    }
                }
                best.1
            })
            .collect();
        // The seams of THIS shell: exactly the tiles whose neighbourhood
        // disagrees about membership (data), and the SHELF where it disagrees
        // about KIND (the painted transition between bed and continent).
        self.boundary = (0..map.len() as TileId)
            .map(|t| {
                let mine = self.home[t as usize];
                map.neighbours(t)
                    .iter()
                    .any(|n| self.home[*n as usize] != mine)
            })
            .collect();
        // The SHELF: a margin around every kind edge whose WIDTH varies over
        // the sphere — swelling to several tiles, pinching to the edge ring —
        // a smooth width field over a rings-from-the-edge distance sweep.
        let kind_of = |t: usize| self.continental[self.home[t] as usize];
        let mut dist = vec![u8::MAX; map.len()];
        let mut ring: Vec<TileId> = (0..map.len() as TileId)
            .filter(|t| {
                let mine = kind_of(*t as usize);
                map.neighbours(*t)
                    .iter()
                    .any(|n| kind_of(*n as usize) != mine)
            })
            .collect();
        for t in &ring {
            dist[*t as usize] = 0;
        }
        let mut d = 0u8;
        while !ring.is_empty() && d < SHELF_MAX_RINGS {
            d += 1;
            let mut next = Vec::new();
            for t in ring {
                for n in map.neighbours(t) {
                    if dist[*n as usize] == u8::MAX {
                        dist[*n as usize] = d;
                        next.push(*n);
                    }
                }
            }
            ring = next;
        }
        self.shelf = (0..map.len())
            .map(|t| {
                let rings = dist[t];
                if rings == u8::MAX {
                    return false;
                }
                let p = map.grid().dirs[t];
                let field: f32 = self
                    .shelf_waves
                    .iter()
                    .map(|w| w.amp.x * (w.freq * p.dot(w.axis) + w.phase).sin())
                    .sum();
                let width = SHELF_WIDTH_MEAN + SHELF_WIDTH_VAR * field.clamp(-1.0, 1.0);
                f32::from(rings) <= width
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;
    use crate::seams::SeamField;

    /// **The scheme covers the map and reads like a shell of plates.** Every
    /// tile has a home inside the plate count, every plate owns territory,
    /// both KINDS exist at the default count, the boundaries are exactly where
    /// adjacency disagrees — and the whole thing is INDEPENDENT of the molten
    /// roll: the same plate seed gives the same plates whatever the seam
    /// field is doing.
    #[test]
    fn plates_tile_the_map_in_both_kinds_with_true_boundaries() {
        let map = HexMap::new(MIN_FREQ);
        let field = PlateField::new(&map, DEFAULT_PLATES, 42);
        assert_eq!(field.home.len(), map.len());
        let mut owned = vec![0usize; field.plates() as usize];
        for t in map.tiles() {
            let h = field.home(t);
            assert!(h < field.plates(), "tile {t} homes inside the count");
            owned[h as usize] += 1;
        }
        assert!(owned.iter().all(|c| *c > 0), "every plate owns territory");
        assert!(
            field.kinds().iter().any(|c| *c) && field.kinds().iter().any(|c| !*c),
            "continents AND ocean beds"
        );
        // SHELF: a VARIABLE-WIDTH margin around the kind edges. Recompute the
        // rings-from-a-kind-edge distance independently, then assert the band:
        // every kind-edge tile is shelf; shelf never reaches past the maximum
        // ring; and the width actually VARIES — the margin swells (rings ≥ 2
        // somewhere) and pinches (a ring-1 tile that is NOT shelf somewhere) —
        // never a uniform stripe.
        let kind_edge = |t: TileId| {
            map.neighbours(t)
                .iter()
                .any(|n| field.is_continent(*n) != field.is_continent(t))
        };
        let mut dist = vec![u8::MAX; map.len()];
        let mut ring: Vec<TileId> = map.tiles().filter(|t| kind_edge(*t)).collect();
        for t in &ring {
            dist[*t as usize] = 0;
        }
        let mut d = 0u8;
        while !ring.is_empty() && d < 8 {
            d += 1;
            let mut next = Vec::new();
            for t in ring {
                for n in map.neighbours(t) {
                    if dist[*n as usize] == u8::MAX {
                        dist[*n as usize] = d;
                        next.push(*n);
                    }
                }
            }
            ring = next;
        }
        let (mut wide, mut pinched, mut shelves) = (false, false, 0usize);
        for t in map.tiles() {
            let want = map
                .neighbours(t)
                .iter()
                .any(|n| field.home(*n) != field.home(t));
            assert_eq!(
                field.is_boundary(t),
                want,
                "boundary is adjacency, tile {t}"
            );
            let rings = dist[t as usize];
            if rings == 0 {
                assert!(field.is_shelf(t), "a kind-edge tile is always shelf: {t}");
            }
            if field.is_shelf(t) {
                shelves += 1;
                assert!(rings <= 4, "shelf stays within the margin's reach: {t}");
                if rings >= 2 {
                    wide = true;
                }
            } else if rings == 1 {
                pinched = true;
            }
        }
        assert!(shelves > 0, "beds meet continents somewhere");
        assert!(wide, "the margin swells to a real band somewhere");
        assert!(pinched, "…and pinches to the bare edge somewhere else");
        assert!(
            field.boundary.iter().filter(|b| **b).count() > 0,
            "same-kind plate joins still exist as unpainted data"
        );
        // Clamps + out-of-range reads.
        assert_eq!(PlateField::new(&map, 0, 1).plates(), MIN_PLATES);
        assert_eq!(PlateField::new(&map, 99, 1).plates(), MAX_PLATES);
        assert_eq!(field.home(u32::MAX), 0);
        assert!(!field.is_boundary(u32::MAX));
        assert!(!field.is_shelf(u32::MAX));

        // INDEPENDENCE: the plate scheme never reads the molten field — the
        // same plate roll stands whatever the seams are rolled with.
        let again = PlateField::new(&map, DEFAULT_PLATES, 42);
        let _molten_a = SeamField::new(&map, 3, 1, 111);
        let _molten_b = SeamField::new(&map, 11, 9, 999);
        assert_eq!(field.home, again.home, "same roll, same plates");
        assert_eq!(field.kinds(), again.kinds());
    }

    /// **The edges FLOW — the warp is real and carries the spin.** Against
    /// the same seeds, the warped membership must disagree with a plain
    /// nearest-seed Voronoi over a real share of the map (the liquid-flow
    /// lobes; a zero-warp regression fails here), while still agreeing on the
    /// great majority (a warp, not a shuffle). And the warp field itself must
    /// twist east–west with opposite handedness in opposite hemispheres —
    /// the differential-rotation signature of a body spinning on +Y.
    #[test]
    fn the_plate_edges_flow_and_carry_the_spin() {
        let map = HexMap::new(MIN_FREQ);
        let field = PlateField::new(&map, DEFAULT_PLATES, 42);
        let mut moved = 0usize;
        for (t, d) in map.grid().dirs.iter().enumerate() {
            let mut best = (f32::MIN, 0u32);
            for (i, s) in field.seeds.iter().enumerate() {
                let dot = d.dot(*s);
                if dot > best.0 {
                    best = (dot, i as u32);
                }
            }
            if best.1 != field.home(t as TileId) {
                moved += 1;
            }
        }
        let frac = moved as f32 / map.len() as f32;
        assert!(frac > 0.05, "the warp bends real territory, got {frac}");
        assert!(frac < 0.5, "…while staying a warp, not a shuffle: {frac}");

        // The spin signature: the warp's longitudinal twist reverses across
        // the equator (differential rotation about +Y).
        let lon_twist = |p: Vec3| {
            let q = field.warp(p);
            // Signed longitude delta about +Y.
            (p.x * q.z - p.z * q.x).atan2(p.x * q.x + p.z * q.z)
        };
        let north = Vec3::new(0.6, 0.75, 0.0).normalize();
        let south = Vec3::new(0.6, -0.75, 0.0).normalize();
        // The noise waves also move longitude, so read the twist as the MEAN
        // over a ring of longitudes, which cancels the zero-mean waves and
        // leaves the shear.
        let ring_mean = |lat_dir: Vec3| {
            let n = 64;
            (0..n)
                .map(|k| {
                    let a = k as f32 / n as f32 * std::f32::consts::TAU;
                    let (sn, cs) = a.sin_cos();
                    // The latitude ring: lat_dir rotated about +Y.
                    let p = Vec3::new(
                        cs * lat_dir.x - sn * lat_dir.z,
                        lat_dir.y,
                        sn * lat_dir.x + cs * lat_dir.z,
                    )
                    .normalize();
                    lon_twist(p)
                })
                .sum::<f32>()
                / n as f32
        };
        let (tn, ts) = (ring_mean(north), ring_mean(south));
        assert!(
            tn * ts < 0.0,
            "opposite hemispheres twist opposite ways: north {tn}, south {ts}"
        );
        assert!(
            tn.abs() > 0.05 && ts.abs() > 0.05,
            "…and the twist is a real shear: north {tn}, south {ts}"
        );
    }

    /// **The roll is the identity, and the count dial grows the same world.**
    /// A re-roll moves the plates; the count dial keeps the shared prefix of
    /// seeds AND kinds.
    #[test]
    fn the_plate_roll_is_the_identity_and_the_dial_keeps_the_prefix() {
        let map = HexMap::new(MIN_FREQ);
        let a = PlateField::new(&map, 8, 7);
        let mut b = PlateField::new(&map, 8, 7);
        assert_eq!(a.home, b.home, "same roll, same shell");
        b.randomize(&map);
        assert_ne!(a.seed(), b.seed());
        assert_ne!(a.home, b.home, "a re-roll moves the plates");

        let mut c = PlateField::new(&map, 8, 7);
        c.set_plates(&map, 12);
        assert_eq!(c.plates(), 12);
        assert_eq!(&c.seeds[..8], &a.seeds[..], "the dial keeps the seeds");
        assert_eq!(&c.kinds()[..8], a.kinds(), "…and their kinds");
    }
}
