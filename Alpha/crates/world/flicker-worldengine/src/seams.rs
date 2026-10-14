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
//! through the shared heat ramp (`flicker_globe::temp_color`) and the hex
//! stack reads a column's own value from it; neither meaning lives here.
//!
//! **Transformation, not outcome (rule 935269B7):** nothing here places a seam.
//! The seeds are random, the metric is geometry, and the seams are wherever the
//! seeds' boundaries fall. The editorial controls are counts and the re-roll —
//! how many cells, how many plumes, and which world — never a position.
//!
//! **The field is a function of TIME as well as position** (resonance slice 1,
//! 2026-10-07): the along-seam intensity and width are sums of waves, and
//! every wave carries its OWN temporal frequency, so the components drift into
//! and out of constructive and destructive interference over geological time
//! — a seam stretch runs hot for an epoch, cools, and a different stretch
//! lights. Closed-form in `(position, tick, seed)`: phase = φ0 + ω·t, nothing
//! accumulates, nothing is serialized — the recipe plus the tick IS the field
//! state, which is what lets any later tier evaluate the field without
//! replaying the planet. Correlated history, not prettier randomness.

use glam::Vec3;

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

// ── the TEMPORAL structure (resonance slice 1, 2026-10-07): each wave's own
// period, in era ticks. Chosen DELIBERATELY across three geological
// timescales — long coherent epochs, intermediate cycles, short pulses — and
// pairwise distinct so no two components phase-lock; never random per wave
// (temporal white noise disguised as sinusoids is the failure mode). The
// bands sit beside the climate's own oscillator (ICE_AGE_PERIODS 830/2210) so
// geology and climate beat against each other rather than in step. ──
/// The intensity waves' periods, parallel to the `VARY_WAVES` draw order (the
/// four mid-wavelength waves, then the three short): two on the long band,
/// three intermediate, two short.
const VARY_PERIODS: [f32; 7] = [3400.0, 2800.0, 1100.0, 850.0, 700.0, 310.0, 230.0];
/// The width waves' periods — one per band.
const WIDTH_PERIODS: [f32; 3] = [3100.0, 950.0, 270.0];

/// How the two boundary reads mix into one heat value: the seam line itself
/// carries this share, and the triple-junction read carries the rest — so an
/// ordinary seam tops out ORANGE on the shared ramp while the meeting points
/// push toward white-hot: the volcanic points of the bubble map.
const SEAM_WEIGHT: f32 = 0.62;

/// One component of a scalar wave field over the sphere, evaluated at a tick:
/// `amp · sin(freq · (p·axis) + phase + omega · t)`. `omega` is the component's
/// OWN temporal frequency — the thing that makes the field resonate rather
/// than slide.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Wave {
    axis: Vec3,
    amp: f32,
    freq: f32,
    phase: f32,
    omega: f32,
}

impl Wave {
    fn at(&self, p: Vec3, t: f32) -> f32 {
        self.amp * (self.freq * p.dot(self.axis) + self.phase + self.omega * t).sin()
    }
}

/// **The molten heat field.** N convection-cell seeds and the per-tile heat
/// their boundaries induce, over one [`HexMap`] tiling, at one era tick.
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
    /// The rifts: each a sampled arc branching off a seam, `(point, envelope)`
    /// per sample — the fade ENVELOPE, `RIFT_ROOT_PEAK` at the root tapering
    /// to zero at the dead end. The live peak is the envelope times the
    /// intensity field at the current tick (applied in `derive_heat`), so a
    /// rift dives and rises with the band it split from. The geometry is
    /// static DATA for the motion layer; only its heat breathes.
    rifts: Vec<Vec<(Vec3, f32)>>,
    /// The along-seam variation field's waves — intensity — and the band-width
    /// field's.
    vary_waves: Vec<Wave>,
    width_waves: Vec<Wave>,
    /// The era tick `heat` was derived at — the field's one time coordinate.
    tick: u64,
    /// Per-tile heat, `0..1` — cool bubble interiors at 0, seams hot, triple
    /// junctions hotter, spot cores hottest. Indexed by `TileId` like every
    /// per-tile layer.
    heat: Vec<f32>,
}

impl SeamField {
    /// Roll a field of `cells` seeds and `spots` plumes with `seed` and derive
    /// the heat for every tile of `map` at tick zero.
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
            tick: 0,
            heat: Vec::new(),
        };
        field.rebuild(map);
        field
    }

    /// The era tick the heat stands at.
    pub fn tick(&self) -> u64 {
        self.tick
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

    /// The rifts — each an arc of `(point, envelope)` samples branching off a
    /// seam and tapering to nothing; the heat they carry at a tick is the
    /// envelope times the intensity field there. The coming motion layer reads
    /// these; the heat map already shows them.
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

    /// A saturating scalar wave field at tick `t`: the raw sum clamped into
    /// [−1, 1]. The clamp is what keeps the source BOUNDED however the
    /// components align.
    fn wave_raw(waves: &[Wave], p: Vec3, t: f32) -> f32 {
        waves
            .iter()
            .map(|w| w.at(p, t))
            .sum::<f32>()
            .clamp(-1.0, 1.0)
    }

    /// The along-seam INTENSITY at `p` and tick `t`: [`DIVE_FLOOR`]..
    /// [`RISE_CEIL`]. A seam crossing a low stretch dives under cooler
    /// material; a high stretch bunches and runs hotter than the plain line —
    /// and which stretches are which changes as the components beat.
    fn vary(&self, p: Vec3, t: f32) -> f32 {
        let raw = Self::wave_raw(&self.vary_waves, p, t);
        let mid = (DIVE_FLOOR + RISE_CEIL) * 0.5;
        mid + (RISE_CEIL - DIVE_FLOOR) * 0.5 * raw
    }

    /// The band-WIDTH factor at `p` and tick `t`: the glow pinches to a thread
    /// and swells to a broad band along the same run.
    fn band_width(&self, p: Vec3, t: f32) -> f32 {
        WIDTH_MIN + WIDTH_SPAN * (0.5 + 0.5 * Self::wave_raw(&self.width_waves, p, t))
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
        debug_assert_eq!(wave_total, VARY_PERIODS.len(), "one period per wave");
        let omega = |period: f32| std::f32::consts::TAU / period;
        for (count, fmin, fspan) in VARY_WAVES {
            for _ in 0..count {
                let k = self.vary_waves.len();
                self.vary_waves.push(Wave {
                    axis: unit(&mut vr),
                    amp: VARY_SWING * (0.5 + vr.f32()) * 2.0 / wave_total as f32,
                    freq: fmin + vr.f32() * fspan,
                    phase: vr.f32() * std::f32::consts::TAU,
                    omega: omega(VARY_PERIODS[k]),
                });
            }
        }
        let (wcount, wfmin, wfspan) = WIDTH_WAVES;
        debug_assert_eq!(wcount, WIDTH_PERIODS.len(), "one period per wave");
        self.width_waves = (0..wcount)
            .map(|k| Wave {
                axis: unit(&mut vr),
                amp: (0.5 + vr.f32()) * 2.0 / wcount as f32,
                freq: wfmin + vr.f32() * wfspan,
                phase: vr.f32() * std::f32::consts::TAU,
                omega: omega(WIDTH_PERIODS[k]),
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
                    // The ENVELOPE fades along the arc — hot where it left the
                    // seam, NOTHING at the dead end. The live heat rides the
                    // intensity field at the current tick (`derive_heat`), so
                    // a rift dives and rises with the band it split from.
                    let frac = k as f32 / (RIFT_SAMPLES - 1) as f32;
                    samples.push((p, RIFT_ROOT_PEAK * (1.0 - frac)));
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
    /// shrink, volcanoes go dormant, new ones form). Stands the field at era
    /// `tick` and re-derives the heat: every wave's phase is `φ0 + ω·tick`,
    /// so the bands breathe, their hot stretches migrate, and the components
    /// beat against each other — and a crust re-derive on the moved field is
    /// what retires old vents and lights new ones. Seeds, spots and rift
    /// geometry stand still: the pattern evolves, the world does not re-roll.
    /// Closed-form: the same `(seed, tick)` is the same field whether reached
    /// in one call or a thousand. A no-op at the current tick.
    pub fn at_tick(&mut self, map: &HexMap, tick: u64) {
        if tick == self.tick && !self.heat.is_empty() {
            return;
        }
        self.tick = tick;
        self.derive_heat(map);
    }

    /// Recompute the heat over the CURRENT geometry at the current tick — the
    /// tail of [`rebuild`](Self::rebuild), callable on its own so
    /// [`at_tick`](Self::at_tick) re-derives without re-rolling anything.
    fn derive_heat(&mut self, map: &HexMap) {
        let t = self.tick as f32;
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
                let band_local = band * self.band_width(*d, t);
                let seam = 1.0 - ((d2 - d1) / band_local).clamp(0.0, 1.0);
                let junction = if d3 < f32::MAX {
                    1.0 - ((d3 - d1) / band_local).clamp(0.0, 1.0)
                } else {
                    0.0 // two cells have no triple junction
                };
                let boundary =
                    (SEAM_WEIGHT * seam + (1.0 - SEAM_WEIGHT) * junction) * self.vary(*d, t);
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
                // A rift is a narrow crack: its samples' envelopes times the
                // band's intensity at this tick, laterally faded — hottest at
                // the seam it left, dead at its far end, born cool on a dived
                // stretch.
                let mut rift = 0.0f32;
                for arc in &self.rifts {
                    for (sp, envelope) in arc {
                        let dot = d.dot(*sp);
                        if dot > rift_near {
                            let a = dot.clamp(-1.0, 1.0).acos() / rift_band;
                            let peak = envelope * self.vary(*sp, t).min(1.0);
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
            // The stored value is the fade ENVELOPE: the root peak at the
            // root, falling to nothing at the tip. The live heat is this times
            // the band's intensity at the tick — a rift born on a dived
            // stretch is born cool (checked on the map below).
            assert!(
                (first_peak - RIFT_ROOT_PEAK).abs() < 1e-6,
                "rift {r}'s root carries the full envelope: {first_peak}"
            );
            for (k, (_, env)) in arc.iter().enumerate() {
                let frac = k as f32 / (RIFT_SAMPLES - 1) as f32;
                assert!(
                    (*env - RIFT_ROOT_PEAK * (1.0 - frac)).abs() < 1e-6,
                    "rift {r} sample {k} is the fade envelope"
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
        // heat at tick zero (which land on hot stretches is the roll's call)
        // — the envelope times the band's intensity where the root stands.
        let root_heat = |a: &Vec<(Vec3, f32)>| a[0].1 * field.vary(a[0].0, 0.0).min(1.0);
        assert!(
            field.rifts().iter().any(|a| root_heat(a) >= 0.25),
            "some rift leaves the seam hot: peaks {:?}",
            field.rifts().iter().map(root_heat).collect::<Vec<_>>()
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
    /// new ones form, over much longer timelines). Stood at a later tick: the
    /// heat moved but stays in range; the seeds, spots and rift arcs stand
    /// exactly still; the change is SLOW (most tiles barely move); and the
    /// crust re-derived on the moved field retires some vents and lights
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

        // ONE drift cadence of the bench (12 ticks): the breath the window
        // takes between two vent re-derives. (Measured 2026-10-07 over three
        // rolls: 74–85% of vents survive a cadence, ~50% survive three — the
        // crust's greedy derive relocates vents far across the map on small
        // heat shifts; banked as the open hysteresis item.)
        field.at_tick(&map, 12);
        assert_eq!(field.tick(), 12);
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
            kept * 3 >= vents0.len() * 2,
            "…while a stable core persists over a cadence: kept {kept} of {}",
            vents0.len()
        );
    }

    /// **The components have INDEPENDENT temporal frequencies, deliberately
    /// spread** (resonance slice 1): every wave's ω is distinct (no two
    /// components phase-lock), the periods span better than a decade (long
    /// epochs to short pulses), and none is temporal white noise — each is a
    /// fixed period, the same at every roll.
    #[test]
    fn the_waves_carry_distinct_periods_across_a_decade() {
        let map = HexMap::new(MIN_FREQ);
        let field = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 42);
        let other = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 7);
        let periods: Vec<f32> = field
            .vary_waves
            .iter()
            .chain(&field.width_waves)
            .map(|w| std::f32::consts::TAU / w.omega)
            .collect();
        assert_eq!(periods.len(), VARY_PERIODS.len() + WIDTH_PERIODS.len());
        for (i, a) in periods.iter().enumerate() {
            for b in &periods[i + 1..] {
                assert!(
                    (a - b).abs() > 1.0,
                    "two components share a period: {a} vs {b}"
                );
            }
        }
        let lo = periods.iter().copied().fold(f32::MAX, f32::min);
        let hi = periods.iter().copied().fold(0.0f32, f32::max);
        assert!(hi / lo > 10.0, "the periods span a decade: {lo}..{hi}");
        // The periods are the DESIGN, not the roll: another world beats on
        // the same timescales (its axes and phases differ, its clocks do not).
        for (a, b) in field.vary_waves.iter().zip(&other.vary_waves) {
            assert_eq!(a.omega, b.omega);
            assert_ne!(a.axis, b.axis);
        }
    }

    /// **The components INTERFERE: the field is not a sliding pattern.** At a
    /// fixed on-seam tile the heat over time is a beat, not a single tone —
    /// its swing over one window differs from its swing over another (a lone
    /// sinusoid, or a shared phase advance, repeats the same swing every
    /// cycle). And the whole field is BOUNDED at every tick however the
    /// components align.
    #[test]
    fn the_components_beat_and_the_field_stays_bounded() {
        let map = HexMap::new(MIN_FREQ);
        let mut field = SeamField::new(&map, DEFAULT_CELLS, 0, 42);
        // An on-line tile: the two nearest seeds near-equidistant.
        let on_line = map
            .tiles()
            .find(|t| {
                let d = map.direction(*t);
                let mut dd: Vec<f32> = field
                    .seeds
                    .iter()
                    .map(|sd| d.dot(*sd).clamp(-1.0, 1.0).acos())
                    .collect();
                dd.sort_by(f32::total_cmp);
                dd[1] - dd[0] < 0.01
            })
            .expect("a seam tile exists");
        const WINDOW: u64 = 700;
        let mut swings = Vec::new();
        for w in 0..4u64 {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for k in 0..WINDOW / 10 {
                field.at_tick(&map, w * WINDOW + k * 10);
                assert!(field.heats().iter().all(|h| (0.0..=1.0).contains(h)));
                let h = field.heat(on_line);
                lo = lo.min(h);
                hi = hi.max(h);
            }
            swings.push(hi - lo);
        }
        let smin = swings.iter().copied().fold(f32::MAX, f32::min);
        let smax = swings.iter().copied().fold(0.0f32, f32::max);
        assert!(
            smax - smin > 0.05,
            "the swing changes window to window (a beat): {swings:?}"
        );
    }

    /// **Temporal coherence: correlated history, not noise.** One tick on,
    /// the field is nearly the same everywhere (the median change is a
    /// whisker); the long band on, it has genuinely moved — and the same
    /// `(seed, tick)` is the same field whether stood up fresh or walked to,
    /// because nothing accumulates (closed-form replay).
    #[test]
    fn the_field_is_coherent_in_time_and_replays_closed_form() {
        let map = HexMap::new(MIN_FREQ);
        let mut walked = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 42);
        let at0 = walked.heats().to_vec();
        walked.at_tick(&map, 1);
        let mut d1: Vec<f32> = walked
            .heats()
            .iter()
            .zip(&at0)
            .map(|(a, b)| (a - b).abs())
            .collect();
        d1.sort_by(f32::total_cmp);
        assert!(
            d1[d1.len() / 2] < 1e-3 && d1[d1.len() - 1] < 0.05,
            "one tick is a whisker: median {} max {}",
            d1[d1.len() / 2],
            d1[d1.len() - 1]
        );
        for k in 2..=1700u64 {
            if k % 97 == 0 || k == 1700 {
                walked.at_tick(&map, k);
            }
        }
        let far: Vec<f32> = walked
            .heats()
            .iter()
            .zip(&at0)
            .map(|(a, b)| (a - b).abs())
            .collect();
        let moved = far.iter().filter(|d| **d > 0.1).count();
        assert!(
            moved > map.len() / 50,
            "the long band moves the field: {moved} tiles changed by >0.1"
        );
        let mut fresh = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 42);
        fresh.at_tick(&map, 1700);
        assert_eq!(
            fresh.heats(),
            walked.heats(),
            "same (seed, tick), same field"
        );
        // Standing at the current tick again derives nothing new.
        let before = walked.heats().to_vec();
        walked.at_tick(&map, 1700);
        assert_eq!(walked.heats(), &before[..]);
    }

    /// **No topology boundary is a geological boundary.** The twelve
    /// pentagons read the same global field as every hex: a pentagon's heat
    /// sits inside the span of its neighbours' heats at least as often as a
    /// hex tile's does (the field indexes by DIRECTION, never by side count),
    /// at tick zero and after the components have beaten.
    #[test]
    fn pentagons_read_the_same_field_as_the_hexes() {
        let map = HexMap::new(MIN_FREQ);
        let mut field = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 42);
        for tick in [0u64, 1234] {
            field.at_tick(&map, tick);
            let jump = |t: u32| {
                let nb = map.neighbours(t);
                let mean = nb.iter().map(|n| field.heat(*n)).sum::<f32>() / nb.len() as f32;
                (field.heat(t) - mean).abs()
            };
            let (mut pent, mut pent_n) = (0.0f32, 0usize);
            let mut hex_jumps: Vec<f32> = Vec::new();
            for t in map.tiles() {
                if map.neighbours(t).len() == 5 {
                    pent += jump(t);
                    pent_n += 1;
                } else {
                    hex_jumps.push(jump(t));
                }
            }
            assert_eq!(pent_n, 12);
            hex_jumps.sort_by(f32::total_cmp);
            let p90 = hex_jumps[hex_jumps.len() * 9 / 10];
            assert!(
                pent / 12.0 <= p90.max(0.02),
                "tick {tick}: pentagons jump {} vs the hexes' p90 {p90}",
                pent / 12.0
            );
        }
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
