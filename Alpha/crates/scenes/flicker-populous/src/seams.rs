//! **The molten layer's first fact: where the heat comes up.**
//!
//! The mantle under the crust is not uniformly hot. It convects in a handful of
//! huge, slow cells; heat wells up along the boundaries where cells meet and
//! sinks in their interiors. Seen from above that is a BUBBLE MAP: large cool
//! bubbles (the cell interiors) rimmed by hot seams (the boundaries), hottest
//! where three cells meet — the points a deep-crust layer will later focus into
//! volcanoes.
//!
//! This module is that field, and nothing else: N random convection-cell seeds
//! on the sphere, a per-tile HEAT in `0..1` derived from how close a tile
//! stands to a boundary between cells, and a handful of HOT SPOTS — mantle
//! plumes that burn through wherever they are, seam or no seam (the Hawaiis
//! to the seams' ridges). It is DATA — the seams tab paints it
//! through the shared heat ramp ([`flicker_globe::temp_color`]) and the hex
//! stack reads a column's own value from it; neither meaning lives here.
//!
//! **Transformation, not outcome (rule 935269B7):** nothing here places a seam.
//! The seeds are random, the metric is geometry, and the seams are wherever the
//! seeds' boundaries fall. The editorial controls are counts and the re-roll —
//! how many cells, how many plumes, and which world — never a position.

use flicker::render::Vec3;

use crate::map::HexMap;

/// The fewest convection cells the dial offers — two hemispheres of cool with
/// one great seam between them.
pub const MIN_CELLS: u32 = 2;
/// The most — a busy mantle, seams everywhere.
pub const MAX_CELLS: u32 = 12;
/// Where the bench opens (Aaron 2026-08-25, functional pass): a full mantle
/// of cells.
pub const DEFAULT_CELLS: u32 = 12;

/// The fewest hot spots the dial offers — none: a pure seam field.
pub const MIN_SPOTS: u32 = 0;
/// The most — a plume-riddled mantle.
pub const MAX_SPOTS: u32 = 12;
/// Where the bench opens (same pass): a busy sky of plumes.
pub const DEFAULT_SPOTS: u32 = 8;

/// A hot spot's angular radius. FIXED, not scaled by the cell count: a plume
/// is its own thing — it does not grow because the convection pattern
/// coarsened. About a dozen tiles across at the standard map size.
const SPOT_RADIUS: f32 = 0.07;
/// A spot's centre heat — white-hot on the shared ramp, hot enough that its
/// core clears the crust's breakthrough floor and vents.
const SPOT_PEAK: f32 = 0.92;
/// The spot stream's offset off the field's one roll, so the spots and the
/// cell seeds are INDEPENDENT draws of the same world: re-count the cells and
/// the spots stand still, and vice versa.
const SPOT_STREAM: u64 = 0x5851_F42D_4C95_7F2D;

// ── the RIFTS (Aaron 2026-08-25: "not all seams have to join — a seam can
// split and dive back into the crust before it joins another spot"; these
// splits will cut plates and drive the motion layer) ──
/// How many rifts per convection cell the field grows — each a crack that
/// BRANCHES off a seam and dies out inside a cell instead of joining.
const RIFTS_PER_CELL: u32 = 1;
/// A rift's heat where it leaves its parent seam — hot enough to vent near
/// the root, cooling to NOTHING at the dead end.
const RIFT_ROOT_PEAK: f32 = 0.8;
/// A rift's lateral half-width, as a fraction of a cell's angular radius —
/// a crack, visibly narrower than the parent seam's glow.
const RIFT_BAND_FRAC: f32 = 0.12;
/// A rift's length range, as fractions of the cell radius — long enough to
/// cut visibly into a cell, short enough to die before the far seam.
const RIFT_LEN_MIN: f32 = 0.35;
const RIFT_LEN_SPAN: f32 = 0.45;
/// Sample points along a rift's arc — the polyline its heat falls off from.
const RIFT_SAMPLES: usize = 12;
/// The total turn a rift may curve through over its length, radians either
/// way — an organic crack, not a ruled line.
const RIFT_CURVE: f32 = 1.2;
/// The rift stream's offset off the one roll — independent of the spot draws;
/// the ROOTS ride the current seeds, so rifts move with their seams.
const RIFT_STREAM: u64 = 0x94D0_49BB_1331_11EB;

/// How far from a boundary the heat glow reaches, as a fraction of a cell's own
/// characteristic angular radius (`√(4π/cells)/2`). Scale-free on purpose: two
/// huge cells get a broad seam, twelve small ones get tight seams, and the
/// bubbles stay bubbles at every count.
const SEAM_BAND: f32 = 0.45;

// ── the ALONG-SEAM variation (Aaron 2026-08-25: seams are BANDS of heat,
// not solid lines — they bunch and stretch and rise and dive; a long seam
// should fade in places where cooler material strides over it) ──
/// The modulation field's waves: (count, freq_min, freq_span). Mid and short
/// wavelengths, so a seam of a cell-radius's length crosses several highs and
/// lows — the dives and rises.
const VARY_WAVES: [(usize, f32, f32); 2] = [(4, 5.0, 6.0), (3, 14.0, 12.0)];
/// The modulation's saturating swing. The raw wave sum is clamped into
/// [−1, 1]; big swing = real time spent at both rails — full dives and
/// bunched hot stretches, not a gentle ripple.
const VARY_SWING: f32 = 1.5;
/// What a full DIVE leaves of the seam's heat — cooler material striding over
/// the hot line, not the line ceasing to exist.
const DIVE_FLOOR: f32 = 0.08;
/// What a full RISE pushes it to — a bunched stretch runs hotter than the
/// plain line (the final heat still clamps at 1).
const RISE_CEIL: f32 = 1.15;
/// The band-WIDTH field's waves and its width range, as factors on the seam
/// band: the glow pinches to a thread and swells to a broad band.
const WIDTH_WAVES: (usize, f32, f32) = (3, 4.0, 7.0);
const WIDTH_MIN: f32 = 0.55;
const WIDTH_SPAN: f32 = 1.05;
/// The variation stream's offset off the one roll.
const VARY_STREAM: u64 = 0xA24B_AED4_963E_E407;

/// How the two boundary reads mix into one heat value: the seam line itself
/// carries this share, and the triple-junction read carries the rest — so an
/// ordinary seam tops out ORANGE on the shared ramp while the meeting points
/// push toward white-hot: the volcanic points of the bubble map.
const SEAM_WEIGHT: f32 = 0.62;

/// **The molten heat field.** N convection-cell seeds and the per-tile heat
/// their boundaries induce, over one [`HexMap`] tiling.
pub struct SeamField {
    /// How many convection cells were asked for, clamped to the offered range.
    cells: u32,
    /// How many hot spots, clamped likewise.
    spots: u32,
    /// The roll that placed the seeds — kept so the same world can be rebuilt
    /// at a new map size without moving its seams.
    seed: u64,
    /// The cell seeds: unit directions on the sphere.
    seeds: Vec<Vec3>,
    /// The hot-spot centres: unit directions, an independent stream of the
    /// same roll.
    spot_dirs: Vec<Vec3>,
    /// The rifts: each a sampled arc branching off a seam, `(point, peak)` per
    /// sample with the peak tapering to zero at the dead end. DATA for the
    /// coming motion layer as much as heat for this one.
    rifts: Vec<Vec<(Vec3, f32)>>,
    /// The along-seam variation field's scalar waves `(axis, amp, freq, phase)`
    /// — intensity — and the band-width field's.
    vary_waves: Vec<(Vec3, f32, f32, f32)>,
    width_waves: Vec<(Vec3, f32, f32, f32)>,
    /// Per-tile heat, `0..1` — cool bubble interiors at 0, seams hot, triple
    /// junctions hotter, spot cores hottest. Indexed by `TileId` like every
    /// per-tile layer.
    heat: Vec<f32>,
}

impl SeamField {
    /// Roll a field of `cells` seeds and `spots` plumes with `seed` and derive
    /// the heat for every tile of `map`.
    pub fn new(map: &HexMap, cells: u32, spots: u32, seed: u64) -> Self {
        let mut field = Self {
            cells: cells.clamp(MIN_CELLS, MAX_CELLS),
            spots: spots.clamp(MIN_SPOTS, MAX_SPOTS),
            seed,
            seeds: Vec::new(),
            spot_dirs: Vec::new(),
            rifts: Vec::new(),
            vary_waves: Vec::new(),
            width_waves: Vec::new(),
            heat: Vec::new(),
        };
        field.rebuild(map);
        field
    }

    /// How many convection cells the field was rolled with.
    pub fn cells(&self) -> u32 {
        self.cells
    }

    /// How many hot spots.
    pub fn spots(&self) -> u32 {
        self.spots
    }

    /// The hot-spot centres — for a view that marks them, and for tests.
    pub fn spot_dirs(&self) -> &[Vec3] {
        &self.spot_dirs
    }

    /// The rifts — each an arc of `(point, peak)` samples branching off a
    /// seam and tapering to nothing. The coming motion layer reads these; the
    /// heat map already shows them.
    pub fn rifts(&self) -> &[Vec<(Vec3, f32)>] {
        &self.rifts
    }

    /// The roll that placed the seeds.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// A tile's heat, `0..1`. Out-of-range asks read as cool rather than
    /// panicking — a viewer's question, and a hole is cold.
    pub fn heat(&self, tile: u32) -> f32 {
        self.heat.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Every tile's heat, for a shell's colour closure.
    pub fn heats(&self) -> &[f32] {
        &self.heat
    }

    /// Re-roll the seeds (a new random world) over the same map.
    pub fn randomize(&mut self, map: &HexMap) {
        self.seed = fastrand::u64(..);
        self.rebuild(map);
    }

    /// Change the cell count, keeping the roll — the first `n` seeds of the
    /// same sequence, so dialing up grows the same world rather than replacing
    /// it. A no-op at the current count.
    pub fn set_cells(&mut self, map: &HexMap, cells: u32) {
        let cells = cells.clamp(MIN_CELLS, MAX_CELLS);
        if cells == self.cells {
            return;
        }
        self.cells = cells;
        self.rebuild(map);
    }

    /// Change the spot count, keeping the roll — the same prefix law as the
    /// cells, on the spots' own stream. A no-op at the current count.
    pub fn set_spots(&mut self, map: &HexMap, spots: u32) {
        let spots = spots.clamp(MIN_SPOTS, MAX_SPOTS);
        if spots == self.spots {
            return;
        }
        self.spots = spots;
        self.rebuild(map);
    }

    /// A saturating scalar wave field: the raw sum clamped into [−1, 1].
    fn wave_raw(waves: &[(Vec3, f32, f32, f32)], p: Vec3) -> f32 {
        waves
            .iter()
            .map(|(axis, amp, freq, phase)| amp * (freq * p.dot(*axis) + phase).sin())
            .sum::<f32>()
            .clamp(-1.0, 1.0)
    }

    /// The along-seam INTENSITY at `p`: [`DIVE_FLOOR`]..[`RISE_CEIL`]. A seam
    /// crossing a low stretch dives under cooler material; a high stretch
    /// bunches and runs hotter than the plain line.
    fn vary(&self, p: Vec3) -> f32 {
        let raw = Self::wave_raw(&self.vary_waves, p);
        let mid = (DIVE_FLOOR + RISE_CEIL) * 0.5;
        mid + (RISE_CEIL - DIVE_FLOOR) * 0.5 * raw
    }

    /// The band-WIDTH factor at `p`: the glow pinches to a thread and swells
    /// to a broad band along the same run.
    fn band_width(&self, p: Vec3) -> f32 {
        WIDTH_MIN + WIDTH_SPAN * (0.5 + 0.5 * Self::wave_raw(&self.width_waves, p))
    }

    /// The map was rebuilt (a new size) — derive the heat for the new tiling
    /// from the SAME seeds: the world's seams do not move when its map does.
    pub fn rebuild(&mut self, map: &HexMap) {
        let mut rng = fastrand::Rng::with_seed(self.seed);
        self.seeds = (0..self.cells)
            .map(|_| {
                // Uniform on the sphere: z uniform in −1..1, longitude uniform.
                let z = rng.f32() * 2.0 - 1.0;
                let a = rng.f32() * std::f32::consts::TAU;
                let r = (1.0 - z * z).max(0.0).sqrt();
                Vec3::new(r * a.cos(), z, r * a.sin())
            })
            .collect();

        // The hot spots ride their OWN stream of the same roll: independent of
        // the cell draws, so either count can change without moving the other.
        let mut spot_rng = fastrand::Rng::with_seed(self.seed.wrapping_add(SPOT_STREAM));
        self.spot_dirs = (0..self.spots)
            .map(|_| {
                let z = spot_rng.f32() * 2.0 - 1.0;
                let a = spot_rng.f32() * std::f32::consts::TAU;
                let r = (1.0 - z * z).max(0.0).sqrt();
                Vec3::new(r * a.cos(), z, r * a.sin())
            })
            .collect();

        // A cell's characteristic angular radius: N equal caps tile 4π sr.
        let cell_radius = (4.0 * std::f32::consts::PI / self.cells as f32).sqrt() * 0.5;

        // The ALONG-SEAM variation fields — their own stream, fixed sizes, so
        // neither count dial moves them. Drawn before the rifts, whose peaks
        // ride the same intensity.
        let mut vr = fastrand::Rng::with_seed(self.seed.wrapping_add(VARY_STREAM));
        let unit = |r: &mut fastrand::Rng| {
            let z = r.f32() * 2.0 - 1.0;
            let a = r.f32() * std::f32::consts::TAU;
            let rr = (1.0 - z * z).max(0.0).sqrt();
            Vec3::new(rr * a.cos(), z, rr * a.sin())
        };
        self.vary_waves.clear();
        let wave_total: usize = VARY_WAVES.iter().map(|(c, _, _)| c).sum();
        for (count, fmin, fspan) in VARY_WAVES {
            for _ in 0..count {
                self.vary_waves.push((
                    unit(&mut vr),
                    VARY_SWING * (0.5 + vr.f32()) * 2.0 / wave_total as f32,
                    fmin + vr.f32() * fspan,
                    vr.f32() * std::f32::consts::TAU,
                ));
            }
        }
        let (wcount, wfmin, wfspan) = WIDTH_WAVES;
        self.width_waves = (0..wcount)
            .map(|_| {
                (
                    unit(&mut vr),
                    (0.5 + vr.f32()) * 2.0 / wcount as f32,
                    wfmin + vr.f32() * wfspan,
                    vr.f32() * std::f32::consts::TAU,
                )
            })
            .collect();

        // The RIFTS: their own stream of the roll, their ROOTS on the current
        // seams — so they move with the seams and stand still under the spots
        // dial. Each rift: a root projected onto the bisector of its two
        // nearest seeds (a point ON a seam), marched perpendicularly INTO one
        // of the two cells as a gently curving arc that dies out — a split
        // that never joins another seam.
        let mut rr = fastrand::Rng::with_seed(self.seed.wrapping_add(RIFT_STREAM));
        self.rifts.clear();
        if self.seeds.len() >= 2 {
            for _ in 0..(self.cells * RIFTS_PER_CELL) {
                let z = rr.f32() * 2.0 - 1.0;
                let a = rr.f32() * std::f32::consts::TAU;
                let rad = (1.0 - z * z).max(0.0).sqrt();
                let mut q = Vec3::new(rad * a.cos(), z, rad * a.sin());
                // Project onto the LOCAL seam: the bisector of the two nearest
                // seeds — iterated, because one projection can slide the point
                // into a third cell's territory, off the true line. A few
                // rounds settle it on the seam that actually runs there.
                let nearest_two = |p: Vec3| {
                    let mut n1 = (f32::MIN, 0usize);
                    let mut n2 = (f32::MIN, 0usize);
                    for (i, sd) in self.seeds.iter().enumerate() {
                        let d = p.dot(*sd);
                        if d > n1.0 {
                            n2 = n1;
                            n1 = (d, i);
                        } else if d > n2.0 {
                            n2 = (d, i);
                        }
                    }
                    (n1.1, n2.1)
                };
                let mut pair = nearest_two(q);
                let mut axis = self.seeds[pair.0] - self.seeds[pair.1];
                for _ in 0..4 {
                    q = (q - axis * (q.dot(axis) / axis.length_squared().max(1e-6)))
                        .normalize_or_zero();
                    let now = nearest_two(q);
                    if now == pair {
                        break;
                    }
                    pair = now;
                    axis = self.seeds[pair.0] - self.seeds[pair.1];
                }
                let root = q;
                // A SPLAY off the seam — not a perpendicular ray: the heading
                // mixes the across-seam direction with the seam's own tangent
                // at a shallow angle, so the fork reads as the seam splitting
                // rather than a streak shooting off it.
                let mut perp = (axis - root * root.dot(axis)).normalize_or_zero();
                if rr.bool() {
                    perp = -perp;
                }
                let mut along = root.cross(perp).normalize_or_zero();
                if rr.bool() {
                    along = -along;
                }
                let splay = (25.0 + rr.f32() * 30.0).to_radians();
                let t = (perp * splay.sin() + along * splay.cos()).normalize_or_zero();
                let len = (RIFT_LEN_MIN + rr.f32() * RIFT_LEN_SPAN) * cell_radius;
                let step = len / RIFT_SAMPLES as f32;
                let turn = (rr.f32() * 2.0 - 1.0) * RIFT_CURVE / RIFT_SAMPLES as f32;
                let mut p = root;
                let mut samples = Vec::with_capacity(RIFT_SAMPLES);
                let mut t = t;
                for k in 0..RIFT_SAMPLES {
                    // The peak fades along the arc — hot where it left the
                    // seam, NOTHING at the dead end — and RIDES the same
                    // intensity field as its parent seam, so a rift dives and
                    // rises with the band it split from.
                    let frac = k as f32 / (RIFT_SAMPLES - 1) as f32;
                    samples.push((p, RIFT_ROOT_PEAK * (1.0 - frac) * self.vary(p).min(1.0)));
                    // March the geodesic, then curve the heading a little.
                    let (sn, cs) = step.sin_cos();
                    let np = (p * cs + t * sn).normalize_or_zero();
                    t = (t * cs - p * sn).normalize_or_zero();
                    let (tsn, tcs) = turn.sin_cos();
                    t = (t * tcs + np.cross(t) * tsn).normalize_or_zero();
                    t = (t - np * np.dot(t)).normalize_or_zero();
                    p = np;
                }
                self.rifts.push(samples);
            }
        }
        self.derive_heat(map);
    }

    /// **The slow geological drift** (Aaron 2026-08-25: upwelling seams and
    /// volcanic dots SHIFT over much longer timelines — seams grow and
    /// shrink, volcanoes go dormant, new ones form). Advances the intensity
    /// and width fields' phases a little and re-derives the heat: the bands
    /// breathe, their hot stretches migrate — and a crust re-derive on the
    /// drifted field is what retires old vents and lights new ones. Seeds,
    /// spots and rift geometry stand still: the pattern drifts, the world
    /// does not re-roll.
    pub fn drift(&mut self, map: &HexMap, amount: f32) {
        for w in &mut self.vary_waves {
            w.3 += amount;
        }
        for w in &mut self.width_waves {
            w.3 += amount * 0.6;
        }
        self.derive_heat(map);
    }

    /// Recompute the heat over the CURRENT geometry and wave phases — the
    /// tail of [`rebuild`](Self::rebuild), callable on its own so a phase
    /// [`drift`](Self::drift) re-derives without re-rolling anything.
    fn derive_heat(&mut self, map: &HexMap) {
        let cell_radius = (4.0 * std::f32::consts::PI / self.cells as f32).sqrt() * 0.5;
        let band = SEAM_BAND * cell_radius;
        let rift_band = RIFT_BAND_FRAC * cell_radius;
        // Skip the exp for tiles clearly outside a rift's glow.
        let rift_near = (rift_band * 3.0).cos();

        let dirs = &map.grid().dirs;
        self.heat = dirs
            .iter()
            .map(|d| {
                // Angular distance to the three nearest seeds. The boundary
                // metric is their DIFFERENCES: on a seam the two nearest seeds
                // are equally far (d2−d1 → 0); at a triple junction the third
                // is too (d3−d1 → 0).
                let (mut d1, mut d2, mut d3) = (f32::MAX, f32::MAX, f32::MAX);
                for s in &self.seeds {
                    let a = d.dot(*s).clamp(-1.0, 1.0).acos();
                    if a < d1 {
                        (d1, d2, d3) = (a, d1, d2);
                    } else if a < d2 {
                        (d2, d3) = (a, d2);
                    } else if a < d3 {
                        d3 = a;
                    }
                }
                // The band is a LIVING one: its width and its intensity both
                // vary along the run — it bunches, stretches, rises, and
                // DIVES under cooler material where the intensity bottoms out.
                let band_local = band * self.band_width(*d);
                let seam = 1.0 - ((d2 - d1) / band_local).clamp(0.0, 1.0);
                let junction = if d3 < f32::MAX {
                    1.0 - ((d3 - d1) / band_local).clamp(0.0, 1.0)
                } else {
                    0.0 // two cells have no triple junction
                };
                let boundary =
                    (SEAM_WEIGHT * seam + (1.0 - SEAM_WEIGHT) * junction) * self.vary(*d);
                // A plume burns wherever it is: a white-hot gaussian core that
                // falls off over SPOT_RADIUS. The tile reads the HOTTEST source
                // over it — heat sources do not stack past the hottest one.
                let plume = self
                    .spot_dirs
                    .iter()
                    .map(|s| {
                        let a = d.dot(*s).clamp(-1.0, 1.0).acos() / SPOT_RADIUS;
                        SPOT_PEAK * (-a * a).exp()
                    })
                    .fold(0.0f32, f32::max);
                // A rift is a narrow crack: its samples' peaks, laterally
                // faded — hottest at the seam it left, dead at its far end.
                let mut rift = 0.0f32;
                for arc in &self.rifts {
                    for (sp, peak) in arc {
                        let dot = d.dot(*sp);
                        if dot > rift_near {
                            let a = dot.clamp(-1.0, 1.0).acos() / rift_band;
                            rift = rift.max(peak * (-a * a).exp());
                        }
                    }
                }
                boundary.max(plume).max(rift).min(1.0)
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;

    /// **The field is the shape it claims.** One heat per tile, all inside
    /// `0..1`, the asked cell count clamped into the offered dial range.
    #[test]
    fn the_field_covers_the_map_inside_the_offered_range() {
        let map = HexMap::new(MIN_FREQ);
        let field = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 7);
        assert_eq!(field.heats().len(), map.len());
        assert!(field.heats().iter().all(|h| (0.0..=1.0).contains(h)));
        assert_eq!(SeamField::new(&map, 0, 0, 7).cells(), MIN_CELLS);
        assert_eq!(SeamField::new(&map, 99, 99, 7).cells(), MAX_CELLS);
        assert_eq!(SeamField::new(&map, 99, 99, 7).spots(), MAX_SPOTS);
        // Out-of-range reads are cool, not a panic.
        assert_eq!(field.heat(u32::MAX), 0.0);
    }

    /// **Bubbles of cool with edges of hot.** A tile standing at a seed (deep
    /// inside its cell) is cold; the hottest tile on the map stands near a
    /// boundary — and the map has BOTH in quantity: this is a bubble map, not
    /// a wash.
    #[test]
    fn interiors_are_cool_and_seams_are_hot() {
        let map = HexMap::new(MIN_FREQ);
        let field = SeamField::new(&map, DEFAULT_CELLS, 0, 42);
        let cold = field.heats().iter().filter(|h| **h < 0.1).count();
        let hot = field.heats().iter().filter(|h| **h > 0.5).count();
        assert!(
            cold > map.len() / 4,
            "the bubbles' interiors are cool: {cold}/{}",
            map.len()
        );
        assert!(hot > 0, "and the seams between them are hot");
        // The seam metric peaks where two cells actually meet: the hottest
        // tile's two nearest seeds are near-equidistant.
        let (hottest, _) = field
            .heats()
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("tiles exist");
        let d = map.direction(hottest as u32);
        let mut dists: Vec<f32> = field
            .seeds
            .iter()
            .map(|s| d.dot(*s).clamp(-1.0, 1.0).acos())
            .collect();
        dists.sort_by(f32::total_cmp);
        assert!(
            dists[1] - dists[0] < 0.05,
            "the hottest tile stands on a boundary: Δ={}",
            dists[1] - dists[0]
        );
    }

    /// **The roll is the identity.** The same seed rebuilds the same field at
    /// any map size; a re-roll moves the seams; a cell-count change at the same
    /// roll KEEPS the shared prefix of seeds (dialing up grows the world).
    #[test]
    fn the_seed_is_the_world_and_rerolls_move_it() {
        let map = HexMap::new(MIN_FREQ);
        let a = SeamField::new(&map, 5, 3, 1234);
        let b = SeamField::new(&map, 5, 3, 1234);
        assert_eq!(a.heats(), b.heats(), "same roll, same world");

        let mut c = SeamField::new(&map, 5, 3, 1234);
        c.randomize(&map);
        assert_ne!(c.seed(), 1234, "a re-roll takes a new seed");
        assert_ne!(a.heats(), c.heats(), "and the seams moved");

        let mut d = SeamField::new(&map, 5, 3, 1234);
        d.set_cells(&map, 7);
        assert_eq!(d.cells(), 7);
        for (i, s) in a.seeds.iter().enumerate() {
            assert_eq!(*s, d.seeds[i], "seed {i} survives the dial");
        }
        // The spots are an INDEPENDENT stream of the same roll: the cells dial
        // does not move them, and their own dial keeps the shared prefix.
        assert_eq!(a.spot_dirs(), d.spot_dirs(), "cells dial leaves the spots");
        d.set_spots(&map, 6);
        assert_eq!(d.spots(), 6);
        assert_eq!(
            &d.spot_dirs()[..3],
            a.spot_dirs(),
            "the spots dial keeps the shared prefix"
        );
    }

    /// **A rift SPLITS off a seam and dies before joining anything.** Every
    /// rift's root lies ON a seam (equidistant to its two nearest seeds), its
    /// peak fades monotonically to ZERO at the far end (the dead end — it
    /// never carries seam-grade heat into a junction), its length stays
    /// inside a cell's radius, and the field is deterministic from the roll —
    /// while the SPOTS dial, an independent stream, moves no rift.
    #[test]
    fn rifts_split_off_seams_and_die_out() {
        let map = HexMap::new(MIN_FREQ);
        let field = SeamField::new(&map, DEFAULT_CELLS, 0, 42);
        let cell_radius = (4.0 * std::f32::consts::PI / field.cells() as f32).sqrt() * 0.5;
        assert_eq!(
            field.rifts().len(),
            (field.cells() * RIFTS_PER_CELL) as usize,
            "one rift per cell"
        );
        for (r, arc) in field.rifts().iter().enumerate() {
            assert_eq!(arc.len(), RIFT_SAMPLES);
            // The ROOT sits on a seam: its two nearest seeds are equidistant.
            let (root, first_peak) = arc[0];
            let mut dists: Vec<f32> = field
                .seeds
                .iter()
                .map(|sd| root.dot(*sd).clamp(-1.0, 1.0).acos())
                .collect();
            dists.sort_by(f32::total_cmp);
            assert!(
                dists[1] - dists[0] < 0.02,
                "rift {r}'s root stands on a seam: Δ={}",
                dists[1] - dists[0]
            );
            // The root's peak is the fade envelope TIMES the band's local
            // intensity — a rift born on a dived stretch is born cool.
            assert!(
                (0.0..=RIFT_ROOT_PEAK + 1e-6).contains(&first_peak),
                "rift {r}'s root peak sits inside the envelope: {first_peak}"
            );
            // The fade ENVELOPE holds at every sample (the intensity may rise
            // and dive along the arc, but never above the fading ceiling)…
            for (k, (_, peak)) in arc.iter().enumerate() {
                let frac = k as f32 / (RIFT_SAMPLES - 1) as f32;
                assert!(
                    *peak <= RIFT_ROOT_PEAK * (1.0 - frac) + 1e-6,
                    "rift {r} sample {k} breaks the fade envelope"
                );
            }
            assert_eq!(arc[RIFT_SAMPLES - 1].1, 0.0, "…to nothing at the tip");
            // …inside the cell: the arc never runs past the cell radius.
            let tip = arc[RIFT_SAMPLES - 1].0;
            let run = root.dot(tip).clamp(-1.0, 1.0).acos();
            assert!(
                run <= (RIFT_LEN_MIN + RIFT_LEN_SPAN) * cell_radius + 1e-3,
                "rift {r} dies inside the cell, ran {run}"
            );
        }
        // Not every rift is born on a dive: at least one carries real root
        // heat (which of the six land on hot stretches is the roll's call).
        assert!(
            field.rifts().iter().any(|a| a[0].1 >= 0.25),
            "some rift leaves the seam hot: peaks {:?}",
            field.rifts().iter().map(|a| a[0].1).collect::<Vec<_>>()
        );
        // Determinism + spot independence: the same roll grows the same
        // rifts, and the spots dial (its own stream) moves none of them.
        let again = SeamField::new(&map, DEFAULT_CELLS, 0, 42);
        assert_eq!(field.rifts(), again.rifts());
        let mut spotted = SeamField::new(&map, DEFAULT_CELLS, 0, 42);
        spotted.set_spots(&map, 6);
        assert_eq!(field.rifts(), spotted.rifts(), "spots move no rift");

        // And the rifts REACH THE MAP: some tile outside every seam's glow
        // (boundary heat ~0, no spots in this field) still reads hot — the
        // crack cutting into a cool bubble interior.
        let cut = map.tiles().any(|t| {
            let d = map.direction(t);
            let mut dd: Vec<f32> = field
                .seeds
                .iter()
                .map(|sd| d.dot(*sd).clamp(-1.0, 1.0).acos())
                .collect();
            dd.sort_by(f32::total_cmp);
            let off_seam = (dd[1] - dd[0]) > SEAM_BAND * cell_radius;
            off_seam && field.heat(t) > 0.35
        });
        assert!(cut, "a rift carries heat into a bubble interior");
    }

    /// **The seams are LIVING BANDS, not solid lines.** Walking the tiles that
    /// stand ON the boundary line (d2−d1 within a whisker), the heat must
    /// span a real range: stretches near full strength (the bunched rises),
    /// stretches diving under cooler material (near the dive floor), and a
    /// spread between — never one flat temperature down the line. The band's
    /// WIDTH varies too: the glow's reach differs along the run.
    #[test]
    fn seams_are_living_bands_that_rise_and_dive() {
        let map = HexMap::new(MIN_FREQ);
        let field = SeamField::new(&map, DEFAULT_CELLS, 0, 42);
        let cell_radius = (4.0 * std::f32::consts::PI / field.cells() as f32).sqrt() * 0.5;
        let mut on_line: Vec<f32> = Vec::new();
        for t in map.tiles() {
            let d = map.direction(t);
            let mut dd: Vec<f32> = field
                .seeds
                .iter()
                .map(|sd| d.dot(*sd).clamp(-1.0, 1.0).acos())
                .collect();
            dd.sort_by(f32::total_cmp);
            if dd[1] - dd[0] < 0.02 {
                on_line.push(field.heat(t));
            }
        }
        assert!(on_line.len() > 100, "the line is sampled in quantity");
        let hi = on_line.iter().copied().fold(0.0f32, f32::max);
        let lo = on_line.iter().copied().fold(1.0f32, f32::min);
        assert!(hi > 0.65, "bunched stretches run hot: {hi}");
        assert!(lo < 0.15, "…and dives go under cooler material: {lo}");
        let mean = on_line.iter().sum::<f32>() / on_line.len() as f32;
        let var = on_line.iter().map(|h| (h - mean).powi(2)).sum::<f32>() / on_line.len() as f32;
        assert!(
            var.sqrt() > 0.12,
            "the temperature genuinely varies along the line: σ={}",
            var.sqrt()
        );
        // Width: the glow's reach at a fixed off-line distance differs along
        // the run — a pinched thread somewhere, a broad band somewhere else.
        let probe = SEAM_BAND * cell_radius * 0.6;
        let mut off_line: Vec<f32> = Vec::new();
        for t in map.tiles() {
            let d = map.direction(t);
            let mut dd: Vec<f32> = field
                .seeds
                .iter()
                .map(|sd| d.dot(*sd).clamp(-1.0, 1.0).acos())
                .collect();
            dd.sort_by(f32::total_cmp);
            if (dd[1] - dd[0] - probe).abs() < 0.01 {
                off_line.push(field.heat(t));
            }
        }
        let ohi = off_line.iter().copied().fold(0.0f32, f32::max);
        let olo = off_line.iter().copied().fold(1.0f32, f32::min);
        assert!(
            ohi > 0.25 && olo < 0.05,
            "the band swells past the probe here and pinches short of it there: {olo}..{ohi}"
        );
    }

    /// **The drift breathes the field without re-rolling the world** (Aaron
    /// 2026-08-25: seams slowly grow and shrink, volcanoes go dormant and
    /// new ones form, over much longer timelines). After a drift: the heat
    /// moved but stays in range; the seeds, spots and rift arcs stand
    /// exactly still; the change is SLOW (most tiles barely move); and the
    /// crust re-derived on the drifted field retires some vents and lights
    /// others while keeping a stable core — dormancy and birth, not a
    /// re-roll.
    #[test]
    fn the_drift_breathes_the_field_and_shifts_the_vents() {
        use crate::crust::CrustField;
        use crate::map::TileId;
        let map = HexMap::new(MIN_FREQ);
        let mut field = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 42);
        let heats0 = field.heats().to_vec();
        let seeds0 = field.seeds.clone();
        let spots0 = field.spot_dirs().to_vec();
        let rifts0 = field.rifts().to_vec();
        let vents0: std::collections::HashSet<TileId> = CrustField::derive(&map, &field)
            .vents()
            .iter()
            .copied()
            .collect();

        for _ in 0..6 {
            field.drift(&map, 0.06);
        }
        assert_ne!(field.heats(), &heats0[..], "the field breathed");
        assert!(field.heats().iter().all(|h| (0.0..=1.0).contains(h)));
        assert_eq!(field.seeds, seeds0, "the cells stand still");
        assert_eq!(field.spot_dirs(), &spots0[..], "the plumes stand still");
        assert_eq!(field.rifts(), &rifts0[..], "the rift arcs stand still");
        // SLOW: the median tile's change is small.
        let mut deltas: Vec<f32> = field
            .heats()
            .iter()
            .zip(&heats0)
            .map(|(a, b)| (a - b).abs())
            .collect();
        deltas.sort_by(f32::total_cmp);
        assert!(
            deltas[deltas.len() / 2] < 0.1,
            "a drift is a breath, not a re-roll: median Δ {}",
            deltas[deltas.len() / 2]
        );

        let vents1: std::collections::HashSet<TileId> = CrustField::derive(&map, &field)
            .vents()
            .iter()
            .copied()
            .collect();
        let kept = vents0.intersection(&vents1).count();
        assert!(!vents1.is_empty() && !vents0.is_empty());
        assert!(
            vents0.difference(&vents1).count() > 0 || vents1.difference(&vents0).count() > 0,
            "some volcano went dormant or was born"
        );
        assert!(
            kept * 3 >= vents0.len(),
            "…while a stable core persists: kept {kept} of {}",
            vents0.len()
        );
    }

    /// **A hot spot is a white-hot core, seam or no seam.** The tile nearest a
    /// plume's centre reads near the spot peak — hotter than any pure seam tile
    /// can reach — and a zero-spot field is exactly the pure seam field.
    #[test]
    fn spots_burn_white_hot_wherever_they_are() {
        let map = HexMap::new(MIN_FREQ);
        let none = SeamField::new(&map, DEFAULT_CELLS, 0, 9);
        let some = SeamField::new(&map, DEFAULT_CELLS, 4, 9);
        assert!(none.spot_dirs().is_empty());
        assert_eq!(some.spot_dirs().len(), 4);
        for centre in some.spot_dirs() {
            let (tile, _) = map
                .grid()
                .dirs
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.dot(*centre).total_cmp(&b.1.dot(*centre)))
                .expect("tiles exist");
            assert!(
                some.heat(tile as u32) > 0.85,
                "the plume's core tile burns white-hot: {}",
                some.heat(tile as u32)
            );
        }
        // The spot field only ADDS heat — nothing cools, and far from every
        // spot the two fields agree.
        for t in 0..map.len() as u32 {
            assert!(some.heat(t) >= none.heat(t) - 1e-6, "tile {t} cooled");
        }
    }
}
