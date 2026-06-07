//! Layered field stack for a **single hex** — the data side of the water-cycle
//! POC, now with a ticking simulation.
//!
//! Two **render classes** of layer, a distinction that is load-bearing:
//!
//! - **Substance** (`ground`, `water`, `ice`, `lava`, `cloud`) — real material
//!   with a thickness; rendered as **meshes**. `water`'s surface *level* is
//!   `ground + water` (computed against the contour below), not a global plane.
//! - **Influence fields** (`temperature`, `pressure`, `wind`, `humidity`) —
//!   scalar/vector fields whose job is to *drive other layers*; rendered as
//!   flat, colour-shaded **heatmaps**, never as relief.
//!
//! The realized composite (substance only) is the LOD8 macro surface — one cell
//! ≈ one cluster. Cluster expansion / contour bake is the layer *below* this and
//! is out of scope here.
//!
//! [`LayerStack::tick`] advances a minimal but conservative water cycle:
//! insolation→temperature, pressure→wind, evaporation, humidity advection,
//! orographic condensation→precipitation, downhill runoff, ice melt. Water
//! mass (`water + ice + humidity`) is conserved across a tick (closed
//! boundaries) — the invariant the whole design rests on, and the one tested.
//!
//! Reusable nucleus (pure data); `main.rs` only visualizes it.

// Reusable nucleus: it intentionally exposes more (heatmap ramps, all the field
// layers, conservation accounting) than any single demo renders. A demo using a
// subset isn't dead code — it's the library surface for the next one.
#![allow(dead_code)]

use flicker::render::{MeshVertex, Vec2, Vec3};
use flicker_primitive::heightmap::world_height;

/// Grid resolution across the hex footprint (`G × G` cells / vertices).
pub const G: usize = 64;

/// Hex size: centre to the **E/W point**, world units. Flat-top orientation —
/// the points face E/W, so this is the wide (point-to-point) axis that maps to
/// the 2048-cluster texture width (≈1024 ft per hex).
pub const HEX_SIZE: f32 = 130.0;
/// Apothem: centre to the N/S **flat edge** = √3/2 · size (half the N/S height).
pub const HEX_HALF_W: f32 = HEX_SIZE * 0.866_025;

/// Vertical exaggeration so the gentle `[96, 160]` heightmap reads in the viewer.
const RELIEF_GAIN: f32 = 2.0;
/// Centre of the heightmap output band (matches `flicker-primitive`).
const BASE_HEIGHT: f32 = 128.0;
/// World→display vertical scale for rendering elevations/thicknesses.
pub const VSCALE: f32 = 1.1;

// --- Simulation constants (POC tuning; arbitrary units) ---
// The sun is a planar day/night wave sweeping across *world* coordinates, so
// joined hexes share one continuous terminator instead of each lighting itself.
const SUN_WAVELENGTH: f32 = HEX_SIZE * 8.0; // warm band spans ~several hexes
const SUN_DIR: (f32, f32) = (0.95, 0.31); // sweep direction across the world
const SUN_OMEGA: f32 = 0.6; // temporal sweep rate (rad/s)
const T_BASE: f32 = 12.0; // baseline temperature
const T_AMP: f32 = 46.0; // day-side insolation amplitude
const LAPSE: f32 = 20.0; // cooling per unit normalized altitude
const FREEZE: f32 = 4.0; // below this, precip falls as snow / water freezes
const EVAP_T0: f32 = 8.0; // evaporation only above this temperature
const EVAP_RATE: f32 = 0.05; // per (temp − T0) per second
const WIND_GAIN: f32 = 0.06; // pressure-gradient → wind speed
const PREVAILING: (f32, f32) = (0.25, 0.12); // constant background drift
const ADV_CFL: f32 = 0.24; // per-axis advected fraction clamp (stability)
const CAP0: f32 = 7.0; // base air moisture capacity
const PRECIP_RATE: f32 = 0.6; // fraction of supersaturation that rains out / s
const RUNOFF_RATE: f32 = 0.6; // downhill transfer aggressiveness
const RUNOFF_ITERS: usize = 2;
const MELT_RATE: f32 = 0.08;

// --- Upper-atmosphere radiative cascade (all heatmaps, no mass) ---
// Incoming stellar flux is absorbed band-by-band on the way down: the
// thermosphere takes the hardest radiation, the ozone layer takes UV, and
// whatever survives reaches the surface. Each layer's "temperature" is just
// what it absorbed — pure heatmap shit from the star.
const A_THERMO: f32 = 0.18; // fraction of top-of-atmosphere flux the thermosphere eats
const A_UV: f32 = 0.40; // fraction of the remainder ozone eats (at full ozone)
const THERMO_BASE: f32 = -45.0; // night-side thermosphere ≈ space cold
const THERMO_AMP: f32 = 240.0; // day-side thermosphere "faces off" (huge swing)
const STRATO_BASE: f32 = -8.0;
const STRATO_GAIN: f32 = 70.0; // warming per unit UV absorbed (the inversion)

// Climate is integrated weather: a slow exponential average of temperature and
// moisture. Biomes classify off *climate* so they stay stable while the
// day/night weather flickers. Rate ≈ 1/250 ticks ≈ averages over ~a day.
const CLIMATE_RATE: f32 = 0.004;
/// Altitude above which land is bare alpine rock regardless of climate.
const ALPINE_ALT: f32 = 0.85;

// Material indices into the mesh shader's demo palette (`mesh.wgsl`).
pub const M_WATER_DEEP: u32 = 1;
pub const M_WATER_MID: u32 = 2;
pub const M_WATER_SHALLOW: u32 = 3;
pub const M_FOAM: u32 = 5;
pub const M_CLOUD: u32 = 7;
pub const M_STONE: u32 = 10;
pub const M_LAVA: u32 = 11;
pub const M_ICE: u32 = 12;
pub const M_LAND: u32 = 13;
pub const M_AURORA: u32 = 14;
pub const M_UV: u32 = 15;
pub const M_VOID: u32 = 16;
// Biome materials (climate-classified land).
pub const M_DESERT: u32 = 17;
pub const M_SAVANNA: u32 = 18;
pub const M_GRASSLAND: u32 = 19;
pub const M_FOREST: u32 = 20;
pub const M_RAINFOREST: u32 = 21;
pub const M_TAIGA: u32 = 22;
pub const M_TUNDRA: u32 = 23;

/// Flat-top column half-height at normalized lon `a = x / size`: 1.0 across the
/// flat band `|a| ≤ ½`, tapering to the E/W points at `|a| = 1`.
fn band_width(a: f32) -> f32 {
    let a = a.abs();
    if a <= 0.5 {
        1.0
    } else {
        ((1.0 - a) / 0.5).max(0.0)
    }
}

/// Local XZ position of grid cell `(i, j)`, hex centred at the origin. **Flat-top:**
/// `x` (E/W) is the wide point-to-point axis at full extent; `z` (N/S) is the
/// edge-to-edge axis, masked to the hex outline.
pub fn cell_local(i: usize, j: usize) -> (f32, f32) {
    let x = (i as f32 / (G - 1) as f32 * 2.0 - 1.0) * HEX_SIZE;
    let wf = band_width(x / HEX_SIZE);
    let z = (j as f32 / (G - 1) as f32 * 2.0 - 1.0) * HEX_HALF_W * wf;
    (x, z)
}

#[inline]
fn idx(i: usize, j: usize) -> usize {
    j * G + i
}

/// Min/max of a field, for normalizing a heatmap ramp.
pub fn minmax(field: &[f32]) -> (f32, f32) {
    field.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| {
        (a.min(v), b.max(v))
    })
}

/// The 33rd/67th percentile of a sample (sorted in place), for splitting a
/// field into thirds. Returns a degenerate spread if there's too little data.
fn terciles(v: &mut [f32]) -> (f32, f32) {
    if v.len() < 3 {
        return (f32::MAX, f32::MAX);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 3], v[2 * v.len() / 3])
}

/// Pack a two-stop colour ramp into the shader's material word: `primary = cold`,
/// `secondary = hot`, `blend = t`. The mesh shader resolves it to
/// `mix(cold, hot, t)`, so a flat sheet becomes a continuous heatmap with no
/// shader change.
pub fn pack_ramp(cold: u32, hot: u32, t: f32) -> u32 {
    let b = (t.clamp(0.0, 1.0) * 255.0) as u32;
    (cold & 0xFFF) | ((hot & 0xFFF) << 12) | ((b & 0xFF) << 24)
}

/// Jacobi diffusion (blur) of a field in place, clamped at the boundary.
fn diffuse(field: &mut [f32], rate: f32, iters: usize) {
    let mut next = field.to_vec();
    for _ in 0..iters {
        for j in 0..G {
            for i in 0..G {
                let k = idx(i, j);
                let l = field[idx(i.saturating_sub(1), j)];
                let r = field[idx((i + 1).min(G - 1), j)];
                let d = field[idx(i, j.saturating_sub(1))];
                let u = field[idx(i, (j + 1).min(G - 1))];
                next[k] = field[k] + rate * ((l + r + d + u) * 0.25 - field[k]);
            }
        }
        field.copy_from_slice(&next);
    }
}

/// The stacked layers for one hex. Each `Vec` is row-major `G × G`.
pub struct LayerStack {
    // --- substance (meshes) ---
    pub ground: Vec<f32>,
    pub water: Vec<f32>,
    pub ice: Vec<f32>,
    pub lava: Vec<f32>,
    pub cloud: Vec<f32>, // derived saturation, drawn as a mesh deck
    // --- influence fields (heatmaps) ---
    pub temperature: Vec<f32>, // troposphere / surface
    pub pressure: Vec<f32>,
    pub humidity: Vec<f32>,
    pub wind_x: Vec<f32>,
    pub wind_z: Vec<f32>,
    // --- upper-atmosphere heatmaps (radiative cascade from the star) ---
    pub stratosphere: Vec<f32>, // warmed by ozone UV absorption (the inversion)
    pub thermosphere: Vec<f32>, // hardest radiation; extreme day/night swing
    pub ozone: Vec<f32>,        // UV-absorbing amount, 0..1 (a "hole" lets UV through)
    // --- climate (integrated weather → drives biomes) ---
    pub climate_temp: Vec<f32>,     // slow average of temperature
    pub climate_moisture: Vec<f32>, // slow average of humidity
    biome_t: (f32, f32),            // temperature terciles (33rd, 67th) over eligible land
    biome_m: (f32, f32),            // moisture terciles
    // --- reference scalars ---
    pub snow_line: f32,
    pub relief_lo: f32,
    pub relief_span: f32,
    pub time: f32,
    /// World-space origin of this hex's footprint, so insolation (and any other
    /// world-positioned field) is continuous across joined tiles.
    pub world_origin: Vec2,
}

impl LayerStack {
    /// Generate the initial stack by sampling the world heightmap at `world_off`.
    pub fn generate(world_off: Vec2) -> Self {
        let n = G * G;
        let mut ground = vec![0.0f32; n];
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for j in 0..G {
            for i in 0..G {
                let (x, z) = cell_local(i, j);
                let raw = world_height(world_off.x + x, world_off.y + z);
                let e = BASE_HEIGHT + (raw - BASE_HEIGHT) * RELIEF_GAIN;
                ground[idx(i, j)] = e;
                lo = lo.min(e);
                hi = hi.max(e);
            }
        }
        let span = (hi - lo).max(1.0);
        let sea_level = lo + span * 0.40;
        let snow_line = lo + span * 0.82;

        // Initial water: a resting sea in the basins below sea level.
        let mut water = vec![0.0f32; n];
        for k in 0..n {
            water[k] = (sea_level - ground[k]).max(0.0);
        }
        // Initial ice caps on the cold, dry high ground.
        let mut ice = vec![0.0f32; n];
        for k in 0..n {
            if water[k] == 0.0 && ground[k] > snow_line {
                ice[k] = (ground[k] - snow_line) * 0.9;
            }
        }
        // A lava pool from a vent at the lowest dry cell.
        let mut lava = vec![0.0f32; n];
        let (mut vent, mut vmin) = (0usize, f32::INFINITY);
        for k in 0..n {
            if water[k] == 0.0 && ground[k] < vmin {
                vmin = ground[k];
                vent = k;
            }
        }
        if vmin.is_finite() {
            let (vi, vj) = (vent % G, vent / G);
            let rim = vmin + span * 0.22;
            let r = G as f32 / 4.0;
            for j in 0..G {
                for i in 0..G {
                    let k = idx(i, j);
                    if water[k] != 0.0 {
                        continue;
                    }
                    let (dx, dz) = (i as f32 - vi as f32, j as f32 - vj as f32);
                    let d = (dx * dx + dz * dz).sqrt();
                    if d < r {
                        lava[k] = (rim - ground[k]).max(0.0) * (1.0 - d / r);
                    }
                }
            }
        }

        // Start the air moist so the cycle has something to rain out.
        let humidity = vec![5.0f32; n];

        let mut s = Self {
            ground,
            water,
            ice,
            lava,
            cloud: vec![0.0; n],
            temperature: vec![T_BASE; n],
            pressure: vec![0.0; n],
            humidity: humidity.clone(),
            wind_x: vec![0.0; n],
            wind_z: vec![0.0; n],
            stratosphere: vec![STRATO_BASE; n],
            thermosphere: vec![THERMO_BASE; n],
            ozone: vec![1.0; n],
            climate_temp: vec![T_BASE; n],
            climate_moisture: humidity,
            biome_t: (0.0, 1.0),
            biome_m: (0.0, 1.0),
            snow_line,
            relief_lo: lo,
            relief_span: span,
            time: 0.0,
            world_origin: world_off,
        };
        // Seed climate from one thermal pass so biomes are sensible from frame 1.
        s.update_thermal();
        s.climate_temp.copy_from_slice(&s.temperature);
        s.refresh_biome_bands();
        s
    }

    #[inline]
    fn altitude(&self, k: usize) -> f32 {
        (self.ground[k] - self.relief_lo) / self.relief_span
    }

    /// Advance the water cycle by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        self.time += dt;
        self.update_thermal();
        self.update_wind();
        self.evaporate(dt);
        self.advect_humidity(dt);
        self.condense_precipitate(dt);
        self.runoff();
        self.melt_ice(dt);
        self.update_climate();
    }

    /// Integrate weather into climate (a slow exponential average), and refresh
    /// the climate ranges biome classification reads.
    fn update_climate(&mut self) {
        for k in 0..G * G {
            self.climate_temp[k] += CLIMATE_RATE * (self.temperature[k] - self.climate_temp[k]);
            self.climate_moisture[k] += CLIMATE_RATE * (self.humidity[k] - self.climate_moisture[k]);
        }
        self.refresh_biome_bands();
    }

    /// Recompute the temperature/moisture tercile thresholds over the
    /// Whittaker-eligible land (above freezing, below the alpine line), so each
    /// climate band is ~1/3 of the land and a full spread of biomes emerges
    /// instead of one dominant type.
    fn refresh_biome_bands(&mut self) {
        let mut ts = Vec::new();
        let mut ms = Vec::new();
        for k in 0..G * G {
            if self.water[k] <= 0.5 && self.altitude(k) <= ALPINE_ALT && self.climate_temp[k] >= FREEZE {
                ts.push(self.climate_temp[k]);
                ms.push(self.climate_moisture[k]);
            }
        }
        self.biome_t = terciles(&mut ts);
        self.biome_m = terciles(&mut ms);
    }

    /// Classify the biome of land cell `(i, j)` from its **climate** (not the
    /// flickering weather): a Whittaker-style temperature×moisture grid, with
    /// freezing and alpine overrides. Temperature & moisture are taken relative
    /// to the map's own range so a spread of biomes always emerges. Returns a
    /// palette material; callers handle water/lava/surface-ice themselves.
    pub fn biome_material(&self, i: usize, j: usize) -> u32 {
        let k = idx(i, j);
        if self.altitude(k) > ALPINE_ALT {
            return M_STONE; // bare alpine rock
        }
        let (ct, cm) = (self.climate_temp[k], self.climate_moisture[k]);
        if ct < FREEZE {
            return if cm > self.biome_m.0 { M_TAIGA } else { M_TUNDRA };
        }
        let tcol = (ct >= self.biome_t.0) as u32 + (ct >= self.biome_t.1) as u32; // 0 cold..2 hot
        let mrow = (cm >= self.biome_m.0) as u32 + (cm >= self.biome_m.1) as u32; // 0 dry..2 wet
        match (tcol, mrow) {
            (0, 0) => M_TUNDRA,
            (0, _) => M_TAIGA,
            (1, 0) | (1, 1) => M_GRASSLAND,
            (1, _) => M_FOREST,
            (2, 0) => M_DESERT,
            (2, 1) => M_SAVANNA,
            _ => M_RAINFOREST,
        }
    }

    /// Top-of-atmosphere stellar flux at cell `(i, j)`: a planar day/night wave
    /// in world coordinates, `0` (night) .. `1` (noon). World-positioned so the
    /// terminator runs continuously across joined hexes.
    fn insolation(&self, i: usize, j: usize) -> f32 {
        let (lx, lz) = cell_local(i, j);
        let wx = self.world_origin.x + lx;
        let wz = self.world_origin.y + lz;
        let k = std::f32::consts::TAU / SUN_WAVELENGTH;
        let phase = k * (wx * SUN_DIR.0 + wz * SUN_DIR.1) - self.time * SUN_OMEGA;
        0.5 + 0.5 * phase.cos()
    }

    /// The vertical thermal stack — a radiative cascade from the star down.
    /// Incoming flux is absorbed band-by-band (thermosphere → ozone → surface);
    /// each layer's temperature is what it absorbed, and only the *residual*
    /// flux heats the troposphere — so the upper atmosphere genuinely modulates
    /// surface temperature (e.g. an ozone hole lets more through). All heatmaps,
    /// derived each tick, not conserved.
    fn update_thermal(&mut self) {
        for j in 0..G {
            for i in 0..G {
                let k = idx(i, j);
                let s0 = self.insolation(i, j);

                // Thermosphere: eats the hardest radiation, huge day/night swing.
                self.thermosphere[k] = THERMO_BASE + THERMO_AMP * s0;
                let r1 = s0 * (1.0 - A_THERMO);

                // Ozone/stratosphere: absorbs UV → warms with altitude (inversion).
                let absorbed = r1 * A_UV * self.ozone[k];
                self.stratosphere[k] = STRATO_BASE + STRATO_GAIN * absorbed;
                let surface_flux = r1 * (1.0 - A_UV * self.ozone[k]);

                // Troposphere/surface: only the residual flux, minus albedo & lapse.
                let albedo = if self.water[k] > 0.0 {
                    0.15
                } else if self.ice[k] > 0.0 {
                    0.45
                } else {
                    0.0
                };
                self.temperature[k] =
                    T_BASE + T_AMP * surface_flux * (1.0 - albedo) - LAPSE * self.altitude(k);
            }
        }
        diffuse(&mut self.temperature, 0.25, 2);
        diffuse(&mut self.stratosphere, 0.2, 1);
        diffuse(&mut self.thermosphere, 0.2, 1);
    }

    /// Pressure falls where it is warm; wind blows down the pressure gradient
    /// (cold→warm) plus a constant prevailing drift.
    fn update_wind(&mut self) {
        for k in 0..G * G {
            self.pressure[k] = -self.temperature[k];
        }
        for j in 0..G {
            for i in 0..G {
                let k = idx(i, j);
                let px = self.pressure[idx((i + 1).min(G - 1), j)]
                    - self.pressure[idx(i.saturating_sub(1), j)];
                let pz = self.pressure[idx(i, (j + 1).min(G - 1))]
                    - self.pressure[idx(i, j.saturating_sub(1))];
                self.wind_x[k] = -px * WIND_GAIN + PREVAILING.0;
                self.wind_z[k] = -pz * WIND_GAIN + PREVAILING.1;
            }
        }
    }

    /// Surface water evaporates into the air as a function of temperature.
    fn evaporate(&mut self, dt: f32) {
        for k in 0..G * G {
            if self.water[k] > 0.0 {
                let e = (EVAP_RATE * (self.temperature[k] - EVAP_T0).max(0.0) * dt).min(self.water[k]);
                self.water[k] -= e;
                self.humidity[k] += e;
            }
        }
    }

    /// Conservative upwind advection of humidity along the wind, with closed
    /// boundaries (no flux leaves the domain ⇒ humidity is conserved).
    fn advect_humidity(&mut self, dt: f32) {
        let mut delta = vec![0.0f32; G * G];
        for j in 0..G {
            for i in 0..G {
                let k = idx(i, j);
                let h = self.humidity[k];
                if h <= 0.0 {
                    continue;
                }
                let fx = (self.wind_x[k] * dt).clamp(-ADV_CFL, ADV_CFL);
                let fz = (self.wind_z[k] * dt).clamp(-ADV_CFL, ADV_CFL);
                if fx > 0.0 && i < G - 1 {
                    let q = h * fx;
                    delta[k] -= q;
                    delta[idx(i + 1, j)] += q;
                } else if fx < 0.0 && i > 0 {
                    let q = h * -fx;
                    delta[k] -= q;
                    delta[idx(i - 1, j)] += q;
                }
                if fz > 0.0 && j < G - 1 {
                    let q = h * fz;
                    delta[k] -= q;
                    delta[idx(i, j + 1)] += q;
                } else if fz < 0.0 && j > 0 {
                    let q = h * -fz;
                    delta[k] -= q;
                    delta[idx(i, j - 1)] += q;
                }
            }
        }
        for (h, d) in self.humidity.iter_mut().zip(&delta) {
            *h += *d;
        }
    }

    /// Air holds less moisture when cold and at altitude (orographic lift); the
    /// supersaturated excess rains out — as water if warm, ice if freezing.
    /// `cloud` records saturation for the cloud deck. Humidity→water/ice conserves.
    fn condense_precipitate(&mut self, dt: f32) {
        let rate = (PRECIP_RATE * dt).min(1.0);
        for k in 0..G * G {
            let temp_factor = ((self.temperature[k] - FREEZE) / T_AMP).clamp(0.1, 1.0);
            let cap = CAP0 * temp_factor * (1.0 - 0.6 * self.altitude(k));
            self.cloud[k] = (self.humidity[k] / cap.max(0.01)).clamp(0.0, 1.5);
            let excess = (self.humidity[k] - cap).max(0.0);
            if excess > 0.0 {
                let p = excess * rate;
                self.humidity[k] -= p;
                if self.temperature[k] > FREEZE {
                    self.water[k] += p;
                } else {
                    self.ice[k] += p;
                }
            }
        }
    }

    /// Water flows downhill over the solid surface (`ground + ice + lava`),
    /// pooling in basins to a level set by the contour. Conservative, closed
    /// boundaries; a few relaxation sub-steps for stability.
    fn runoff(&mut self) {
        for _ in 0..RUNOFF_ITERS {
            let mut delta = vec![0.0f32; G * G];
            for j in 0..G {
                for i in 0..G {
                    let k = idx(i, j);
                    if self.water[k] <= 0.0 {
                        continue;
                    }
                    let solid = self.ground[k] + self.ice[k] + self.lava[k];
                    let h = solid + self.water[k];
                    let nbrs = [
                        (i > 0).then(|| idx(i - 1, j)),
                        (i < G - 1).then(|| idx(i + 1, j)),
                        (j > 0).then(|| idx(i, j - 1)),
                        (j < G - 1).then(|| idx(i, j + 1)),
                    ];
                    let mut drops = [0.0f32; 4];
                    let mut total = 0.0;
                    for (s, nb) in drops.iter_mut().zip(nbrs) {
                        if let Some(nb) = nb {
                            let hn = self.ground[nb] + self.ice[nb] + self.lava[nb] + self.water[nb];
                            *s = (h - hn).max(0.0);
                            total += *s;
                        }
                    }
                    if total <= 0.0 {
                        continue;
                    }
                    // Move at most half the head, scaled, split by relative drop.
                    let movable = (self.water[k]).min(total * 0.5) * RUNOFF_RATE;
                    for (s, nb) in drops.iter().zip(nbrs) {
                        if let Some(nb) = nb {
                            if *s > 0.0 {
                                let share = movable * (*s / total);
                                delta[k] -= share;
                                delta[nb] += share;
                            }
                        }
                    }
                }
            }
            for (w, d) in self.water.iter_mut().zip(&delta) {
                *w += *d;
            }
        }
    }

    /// Ice melts back to water where it is above freezing.
    fn melt_ice(&mut self, dt: f32) {
        for k in 0..G * G {
            if self.ice[k] > 0.0 && self.temperature[k] > FREEZE {
                let m = (MELT_RATE * (self.temperature[k] - FREEZE) * dt).min(self.ice[k]);
                self.ice[k] -= m;
                self.water[k] += m;
            }
        }
    }

    /// Conserved total: liquid + frozen + airborne water.
    pub fn total_water(&self) -> f64 {
        (0..G * G)
            .map(|k| (self.water[k] + self.ice[k] + self.humidity[k]) as f64)
            .sum()
    }

    /// The realized (composite) surface at `(i, j)`: display height and the
    /// material of whichever **substance** layer is on top. Water's level is
    /// `ground + … + water` — local to the contour, so each basin gets its own
    /// surface. Influence fields never appear here.
    pub fn realized(&self, i: usize, j: usize) -> (f32, u32) {
        let k = idx(i, j);
        let h = self.ground[k] + self.lava[k] + self.ice[k] + self.water[k];
        let mat = if self.water[k] > 0.5 {
            if self.water[k] > 28.0 {
                M_WATER_DEEP
            } else if self.water[k] > 10.0 {
                M_WATER_MID
            } else {
                M_WATER_SHALLOW
            }
        } else if self.lava[k] > 0.5 {
            M_LAVA
        } else if self.ice[k] > 0.5 {
            M_ICE
        } else {
            self.biome_material(i, j) // climate-classified land (handles alpine)
        };
        ((h - BASE_HEIGHT) * VSCALE, mat)
    }
}

/// Build a hex sheet mesh from per-cell closures. `height` is added to
/// `place_y` (display units); a quad is emitted only when all four corners are
/// `present` (sparse layers render just their footprint). A flat `height` with a
/// [`pack_ramp`] material gives a heatmap; a relief `height` gives a surface.
pub fn build_sheet<H, M, P>(
    place_y: f32,
    height: H,
    material: M,
    present: P,
) -> (Vec<MeshVertex>, Vec<u32>)
where
    H: Fn(usize, usize) -> f32,
    M: Fn(usize, usize) -> u32,
    P: Fn(usize, usize) -> bool,
{
    let mut pos = Vec::with_capacity(G * G);
    for j in 0..G {
        for i in 0..G {
            let (x, z) = cell_local(i, j);
            pos.push(Vec3::new(x, place_y + height(i, j), z));
        }
    }
    let at = |i: usize, j: usize| pos[idx(i, j)];

    let mut vertices = Vec::with_capacity(G * G);
    for j in 0..G {
        for i in 0..G {
            let du = at((i + 1).min(G - 1), j) - at(i.saturating_sub(1), j);
            let dv = at(i, (j + 1).min(G - 1)) - at(i, j.saturating_sub(1));
            let mut n = du.cross(dv).normalize_or(Vec3::Y);
            if n.y < 0.0 {
                n = -n;
            }
            vertices.push(MeshVertex {
                position: at(i, j).to_array(),
                normal: n.to_array(),
                material: material(i, j),
            });
        }
    }

    let mut indices = Vec::new();
    let id = |i: usize, j: usize| idx(i, j) as u32;
    for j in 0..G - 1 {
        for i in 0..G - 1 {
            if present(i, j) && present(i + 1, j) && present(i, j + 1) && present(i + 1, j + 1) {
                let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i, j + 1), id(i + 1, j + 1));
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }

    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> LayerStack {
        LayerStack::generate(Vec2::new(1234.0, 5678.0))
    }

    #[test]
    fn thresholds_lie_within_the_relief() {
        let s = stack();
        let (lo, hi) = minmax(&s.ground);
        assert!(lo < s.snow_line && s.snow_line < hi);
    }

    #[test]
    fn water_mass_is_conserved_across_many_ticks() {
        let mut s = stack();
        let start = s.total_water();
        for _ in 0..300 {
            s.tick(0.05);
        }
        let end = s.total_water();
        let drift = (end - start).abs() / start;
        assert!(drift < 1e-3, "H2O drift {drift} (start {start}, end {end})");
    }

    #[test]
    fn the_sim_stays_finite_and_bounded() {
        let mut s = stack();
        for _ in 0..300 {
            s.tick(0.05);
        }
        let n = G * G;
        for k in 0..n {
            for v in [
                s.water[k],
                s.ice[k],
                s.humidity[k],
                s.temperature[k],
                s.stratosphere[k],
                s.thermosphere[k],
            ] {
                assert!(v.is_finite(), "non-finite field value");
            }
            assert!(s.water[k] >= -1e-3 && s.ice[k] >= -1e-3 && s.humidity[k] >= -1e-3);
        }
    }

    #[test]
    fn thermosphere_swings_hard_between_day_and_night() {
        // As the terminator sweeps, the thermosphere should span near-noon to
        // near-night somewhere across space-and-time — the star burning our
        // faces off by day, ≈ space-cold by night.
        let mut s = stack();
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for _ in 0..100 {
            s.tick(0.05);
            let (l, h) = minmax(&s.thermosphere);
            lo = lo.min(l);
            hi = hi.max(h);
        }
        assert!(hi - lo > THERMO_AMP * 0.5, "thermosphere swing only {}", hi - lo);
    }

    #[test]
    fn ozone_shields_the_surface() {
        // Two identical worlds, one with an ozone hole: more UV reaches the
        // ground, so its surface runs hotter — proving the cascade couples down.
        let mut shielded = stack();
        let mut hole = stack();
        hole.ozone.iter_mut().for_each(|o| *o = 0.0);
        shielded.tick(0.05);
        hole.tick(0.05);
        let mean = |f: &[f32]| f.iter().sum::<f32>() / f.len() as f32;
        assert!(
            mean(&hole.temperature) > mean(&shielded.temperature),
            "ozone hole {} not warmer than shielded {}",
            mean(&hole.temperature),
            mean(&shielded.temperature)
        );
    }

    #[test]
    fn temperature_field_moves_with_the_sun() {
        let mut s = stack();
        s.tick(0.05);
        let first = s.temperature.clone();
        for _ in 0..20 {
            s.tick(0.05);
        }
        let changed = (0..G * G).filter(|&k| (s.temperature[k] - first[k]).abs() > 0.5).count();
        assert!(changed > G * G / 10, "temperature barely moved: {changed} cells");
    }

    #[test]
    fn biomes_differentiate_from_climate() {
        // Let the climate settle, then land should carry several distinct biomes
        // (not one flat type) — the payoff of classifying off integrated weather.
        let mut s = stack();
        for _ in 0..400 {
            s.tick(0.05);
        }
        let mut kinds = std::collections::HashSet::new();
        for j in 0..G {
            for i in 0..G {
                if s.water[i + j * G] <= 0.5 {
                    kinds.insert(s.biome_material(i, j));
                }
            }
        }
        assert!(kinds.len() >= 5, "only {} biome kinds: {:?}", kinds.len(), kinds);
    }

    #[test]
    fn realized_is_finite_with_valid_materials() {
        let s = stack();
        for j in 0..G {
            for i in 0..G {
                let (y, m) = s.realized(i, j);
                assert!(y.is_finite() && (1..=23).contains(&m));
            }
        }
    }
}
