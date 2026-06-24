//! The viewer scene: an oblique orbit-camera look at a forming system, **playing back
//! a recorded formation run**. Embryos orbit, scatter, collide (with impact flashes),
//! merge or shatter, and a few settle into protoplanets; the disk's leftover dust
//! fades as it accretes. The HUD lists the survivors with a habitability verdict and,
//! for the selected one, the element-abundance vector it would hand to Epoch 1.

use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;
use std::time::Duration;

use flicker::app::{InputState, Key};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, SceneLighting, TextureHandle, Vec2,
    Vec3, VolumetricDisk, MAX_VOLUMETRIC_BODIES,
};
use flicker::scene::{Scene, Transition};
use flicker_celestial::hex::{hex_freq_for_giant, hex_freq_for_radius};
use flicker_materials::Tables;
use flicker_worldgen::HexState;

use crate::astro::{earth_masses, G, M_EARTH, M_STAR};
use crate::body::{cleared_neighborhood, Body, BodyKind};
use crate::camera::OrbitCam;
use crate::disk::{self, DISK_INNER, DISK_OUTER, HZ_INNER, HZ_OUTER, SNOW_LINE};
use crate::habitability::assess;
use crate::material::{load_tables, Composition, MaterialClass};
use crate::planet;
use crate::sim::{self, Timeline};
use crate::worldglobe;

const TEXT: [f32; 4] = [0.90, 0.92, 0.97, 1.0];
const DIM: [f32; 4] = [0.62, 0.66, 0.78, 1.0];
const GOOD: [f32; 4] = [0.55, 0.92, 0.60, 1.0];
/// Frozen / committed state — Space holds the configuration as the Epoch-1 seed.
const FROZEN: [f32; 4] = [0.40, 0.82, 1.0, 1.0];
/// The time slider's `0..1` maps to this many millions of years (cosmetic — see `sim`).
/// ~150 Myr spans a realistic terrestrial giant-impact phase.
const RUN_MYR: f32 = 150.0;
/// Sim-years of orbital coast advanced per second of playback **once the system settles**. The
/// formation is a fast geological time-lapse; the coast drops to this **calm, watchable orbital
/// pace** so a settled body (a gas giant especially) is stable enough to track while its surface
/// evolves — not the old `span_year × speed`, which whipped bodies thousands of orbits/sec into a
/// blur. A planet at 1 AU takes ~`1/COAST_YEARS_PER_SEC` s per orbit; outer giants are slower.
/// Tuning knob — raise for livelier orbits, lower to nearly still the system for close study.
const COAST_YEARS_PER_SEC: f64 = 0.3;
/// Physical radii are ~10⁻⁵ AU; scale them up so bodies are visible at disk scale.
/// (Halved again from 3000/0.09/1.2 — bodies were reading ~2× too large overall.)
const VISUAL_INFLATION: f64 = 1500.0;
const MIN_BODY: f32 = 0.045;
const MAX_BODY: f32 = 0.6;
/// Below this mass a survivor is a leftover planetesimal / belt object, not a planet.
const BELT_MASS: f64 = 0.03 * M_EARTH;
/// Moons drawn per body (the most massive few), as small hex globes.
const MAX_MOON_SPHERES: usize = 3;
/// Coarse hex resolution used for actively-forming bodies (many at once) and for moons —
/// the viewer LOD; settled/frozen bodies use their full size-based `hex_freq` instead.
const COARSE_FREQ: u32 = 6;
/// Ring driver — a captured satellite below this mass is tidally shredded (Roche-limit
/// disruption) into a ring instead of surviving as a moon. We don't track moon orbits, so
/// satellite *size* proxies "inside the Roche limit": small bodies disrupt, big ones survive.
const RING_MOON_MAX: f64 = 0.04 * M_EARTH;
/// Minimum shredded mass for a visible ring; below it the debris is too sparse to show.
const RING_MIN_MASS: f64 = 0.003 * M_EARTH;
/// Shredded mass that reads as a full-strength (brightest) ring.
const RING_REF_MASS: f64 = 0.05 * M_EARTH;

/// Mega-years advanced per evolution step. A tuning knob.
const EVO_STEP_MYR: f64 = 0.5;
/// Simulated age (MYR) at which a body's evolution **pegs** (stops stepping): a solid finishes
/// differentiating, a gas giant finishes developing its swirl. After this it just holds its baked
/// state — no more steps, no more uploads. A tuning knob (later replaced by the convergence gate).
const EVO_COMPLETE_MYR: f64 = 10.0;
/// Real seconds a body holds each flat-shaded state before swapping to the next — the art-directed
/// evolution cadence. **Nothing redraws between steps** (no per-frame upload); a step is one cheap
/// sim step + one mesh swap. A tuning knob.
const STEP_INTERVAL: f32 = 1.2;
/// **Viewer LOD** for an evolving globe — the icosphere `freq` it's *rendered* at, capped here so a
/// step's mesh build + upload stays cheap. The body's real size→hex budget (Earth ≈ 100) is the
/// **data** (verified in `flicker-celestial::hex` tests); rendering an evolving 100k-cell globe each
/// step craters the frame rate, and at disk distance the hexes are sub-pixel anyway. Size shows
/// through the body's on-screen *size*, not its tile count. Smaller bodies keep their own `freq`.
const VIEWER_EVO_FREQ: u32 = 20;

/// A body's cached globe: its current evolution state (a grid of [`HexState`]), the **flat-shaded**
/// mesh of that state, and the per-body clock. Every body evolves through the system cutoff
/// (Epoch 3): a **gaseous** body advects its liquefied air ([`worldglobe::step_gas`]); a **solid**
/// body differentiates a crust ([`worldglobe::step_solid`] → Epoch 2, iron draining out as it
/// matures). The evolution is shown by **swapping** the flat mesh to the next state once per
/// `STEP_INTERVAL` — no crossfade, no per-frame work. Once `EVO_COMPLETE_MYR` is reached the body
/// pegs (static). The icosphere is cached in `topo` so a swap never rebuilds it. Evicted when no
/// longer drawn.
struct Globe {
    /// Cached icosphere topology — built **once** and reused for every step + mesh swap, so the
    /// (expensive) icosphere is never rebuilt per frame.
    topo: worldglobe::Topo,
    /// World seed (the shared `EpochCtx` seed for the solid stages).
    seed: u64,
    /// The body's current evolution state.
    state: Vec<HexState>,
    /// Simulated age (MYR) — drives the solid differentiation `settle` and the peg.
    age_myr: f64,
    mesh: MeshHandle,
    /// Real-time accumulator toward the next step (the art cadence).
    accum: f32,
    /// Evolution complete → holds its baked state (no more steps or uploads).
    pegged: bool,
    /// Gas (advects) vs solid (differentiates).
    gas: bool,
    /// Surface glossiness `0..1` — a wet/icy specular sheen for water-rich (liquid) worlds; 0 for
    /// dry rock and gas (matte). Fixed per body (its water content doesn't change here).
    gloss: f32,
}

impl Globe {
    /// Materialise a globe from its initial state `states` (Epoch-1 seed / gas swirl) on the cached
    /// `topo`, uploading the flat-shaded mesh of that state. `gloss` gives liquid worlds a sheen.
    fn new(renderer: &mut Renderer, topo: worldglobe::Topo, states: Vec<HexState>, seed: u64, gas: bool, gloss: f32) -> Self {
        let (v, i) = worldglobe::globe_mesh(&states, &topo);
        let mesh = renderer.upload_mesh(&v, MeshIndices::U32(&i));
        // Stagger each body's step phase across `[0, STEP_INTERVAL)` (deterministic from `seed`) so
        // bodies don't all rebuild on the same frame — spreads the swaps out, no synchronized hitch.
        let accum = (seed % 997) as f32 / 997.0 * STEP_INTERVAL;
        Self { topo, seed, state: states, age_myr: 0.0, mesh, accum, pegged: false, gas, gloss }
    }

    /// One evolution step: gas advects its air; a solid differentiates at the `settle` its age maps to.
    fn step_once(state: &[HexState], topo: &worldglobe::Topo, seed: u64, gas: bool, age_myr: f64, tables: &Tables) -> Vec<HexState> {
        if gas {
            worldglobe::step_gas(state, topo, EVO_STEP_MYR)
        } else {
            let settle = (age_myr / EVO_COMPLETE_MYR).clamp(0.0, 1.0);
            worldglobe::step_solid(state, tables, topo, seed, settle)
        }
    }

    /// Advance the evolution on the art cadence, then draw. The body holds its current flat-shaded
    /// state; every `STEP_INTERVAL` real seconds it takes **one** cheap sim step and **swaps** the
    /// mesh to the new state — at most one re-upload per frame, none in between (the cached `topo`
    /// makes the swap cheap — no icosphere rebuild). Once `EVO_COMPLETE_MYR` is reached the body
    /// pegs and never re-uploads again.
    fn advance(&mut self, renderer: &mut Renderer, tables: &Tables, dt: f32, model: Mat4) {
        if !self.pegged {
            self.accum += dt;
            if self.accum >= STEP_INTERVAL {
                self.accum = 0.0; // one step per advance — a long frame can't burst many
                self.age_myr += EVO_STEP_MYR;
                self.state = Self::step_once(&self.state, &self.topo, self.seed, self.gas, self.age_myr, tables);
                let (v, i) = worldglobe::globe_mesh(&self.state, &self.topo);
                renderer.free_mesh(self.mesh);
                self.mesh = renderer.upload_mesh(&v, MeshIndices::U32(&i));
                if self.age_myr >= EVO_COMPLETE_MYR {
                    self.pegged = true;
                }
            }
        }
        renderer.draw_mesh(self.mesh, model, MeshDrawOptions { gloss: self.gloss, ..Default::default() });
    }
}

pub struct SolarSystem {
    cam: OrbitCam,
    tables: Tables,
    timeline: Timeline,
    seed: u64,
    /// Seeding-supernova size `0..1` — the master initial-condition dial.
    supernova: f64,
    /// Normalised playback time `0..1` (the formation phase).
    t: f32,
    play: bool,
    speed: f32,
    /// Once the formation reaches `t = 1`, the settled system **keeps orbiting** (an exact
    /// Keplerian coast) instead of freezing — `coast_year` is the sim-time elapsed since.
    coasting: bool,
    coast_year: f64,
    /// A free-running clock (seconds, advances while playing) for cosmetic animation — moon
    /// orbits and the habitable-ring pulse — so they keep moving through the coast too.
    anim_time: f32,
    /// The `anim_time` at the previous frame, so `render` can derive this frame's evolution
    /// advance (the clock pauses with playback, freezing the simulation — never per-frame busywork).
    last_evo_time: f32,
    /// The bodies at the current instant: the recorded snapshot while forming, the final
    /// system Kepler-advanced while coasting. Render, list, rings, and export all read this.
    live: Vec<Body>,
    /// This frame's uploaded body-sphere meshes, freed at the start of the next render.
    body_meshes: Vec<MeshHandle>,
    /// Per-settled-body globes, cached by composition+freq (`globe_key`): each holds the two
    /// materialised [`HexWorld`] grids the surface morphs between (the stored material truth the
    /// evolution iteration steps over) and the disposable mesh derived from them. Built once, the
    /// mesh re-derived only when its blend level changes, evicted when the body stops being drawn.
    globe_cache: HashMap<u64, Globe>,
    /// A unit banded ring annulus uploaded once; drawn per giant (tilted, scaled, tinted).
    ring_mesh: Option<MeshHandle>,
    selected: usize,
    /// While true the camera is choreographed by the formation clock; the first drag hands
    /// manual control back, a reseed re-arms it.
    cinematic: bool,
    disc: Option<TextureHandle>,
    ring: Option<TextureHandle>,
    prev_space: bool,
    prev_r: bool,
    prev_up: bool,
    prev_down: bool,
    prev_lbracket: bool,
    prev_rbracket: bool,
}

impl SolarSystem {
    pub fn new() -> Self {
        let seed = 0xACC2_E71D;
        let supernova = disk::random_supernova(seed);
        let timeline = sim::run(seed, supernova);
        let live = timeline.snapshots.first().map(|s| s.bodies.clone()).unwrap_or_default();
        Self {
            cam: OrbitCam::new(DISK_OUTER as f32),
            tables: load_tables(),
            timeline,
            seed,
            supernova,
            t: 0.0,
            play: true,
            speed: 0.020, // a slow, ~50 s cinematic pass (eased back from the old frantic 0.032)
            coasting: false,
            coast_year: 0.0,
            anim_time: 0.0,
            last_evo_time: 0.0,
            live,
            body_meshes: Vec::new(),
            globe_cache: HashMap::new(),
            ring_mesh: None,
            selected: 0,
            cinematic: true,
            disc: None,
            ring: None,
            prev_space: false,
            prev_r: false,
            prev_up: false,
            prev_down: false,
            prev_lbracket: false,
            prev_rbracket: false,
        }
    }

    /// Re-run the formation sim for the current seed + supernova size and reset playback.
    fn rerun(&mut self) {
        self.timeline = sim::run(self.seed, self.supernova);
        self.t = 0.0;
        self.coasting = false;
        self.coast_year = 0.0;
        self.live = self.timeline.snapshots.first().map(|s| s.bodies.clone()).unwrap_or_default();
        self.selected = 0;
        self.cinematic = true; // re-arm the choreographed camera for the new run
        self.play = true; // ...and *resume* it — a fresh roll always plays its cinematic, even
                          // if the previous system was frozen to lock its Epoch-1 seed.
    }

    /// New random system: fresh seed *and* a fresh random supernova size.
    fn reseed(&mut self) {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.supernova = disk::random_supernova(self.seed);
        self.rerun();
    }

    /// Nudge the supernova-size dial and re-form the same seed's disk with it.
    fn dial_supernova(&mut self, delta: f64) {
        self.supernova = (self.supernova + delta).clamp(0.0, 1.0);
        self.rerun();
    }
}

impl Default for SolarSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// A closed ring of `segs` line segments in the disk plane (XZ) at `radius`.
fn ring(radius: f32, segs: usize) -> Vec<(Vec3, Vec3)> {
    let p = |i: usize| {
        let a = i as f32 / segs as f32 * TAU;
        Vec3::new(radius * a.cos(), 0.0, radius * a.sin())
    };
    (0..segs).map(|i| (p(i), p(i + 1))).collect()
}

/// The body's closed orbital ellipse as line segments (world AU), reconstructed from its
/// state vector: angular momentum `h = r×v` fixes the orbit plane, the eccentricity vector
/// points to periapsis, and `r(θ) = p/(1+e·cosθ)` traces the conic. Empty for an unbound
/// (hyperbolic) body — there's no closed orbit to draw.
fn orbit_ellipse(body: &Body) -> Vec<(Vec3, Vec3)> {
    use glam::DVec3;
    let mu = G * (M_STAR + body.mass);
    let r_vec = body.pos;
    let v_vec = body.vel;
    let r = r_vec.length();
    if r <= 0.0 || mu <= 0.0 {
        return Vec::new();
    }
    let v2 = v_vec.length_squared();
    if 0.5 * v2 - mu / r >= 0.0 {
        return Vec::new(); // unbound
    }
    let h_vec = r_vec.cross(v_vec);
    let h2 = h_vec.length_squared();
    if h2 <= 0.0 {
        return Vec::new();
    }
    let p = h2 / mu; // semi-latus rectum
    let e_vec = ((v2 - mu / r) * r_vec - r_vec.dot(v_vec) * v_vec) / mu;
    let e = e_vec.length();
    if e >= 1.0 {
        return Vec::new();
    }
    let n_hat = h_vec.normalize();
    let p_hat = if e > 1e-6 {
        e_vec / e
    } else {
        // Circular orbit: periapsis is arbitrary — pick any in-plane axis.
        let aref = if n_hat.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
        (aref - n_hat * aref.dot(n_hat)).normalize()
    };
    let q_hat = n_hat.cross(p_hat);
    const N: usize = 96;
    let point = |theta: f64| -> Vec3 {
        let rr = p / (1.0 + e * theta.cos());
        ((p_hat * theta.cos() + q_hat * theta.sin()) * rr).as_vec3()
    };
    (0..N)
        .map(|i| {
            let t0 = i as f64 / N as f64 * std::f64::consts::TAU;
            let t1 = (i + 1) as f64 / N as f64 * std::f64::consts::TAU;
            (point(t0), point(t1))
        })
        .collect()
}

/// Advance a **settled** body along its Keplerian orbit by `dt_year`, returning a clone with
/// updated position/velocity. The formation is over (no more collisions), so each survivor
/// simply coasts on the very conic [`orbit_ellipse`] draws — the same `μ = G(M☆ + m)`, so the
/// body stays exactly on its rendered ellipse. Exact, cheap, drift-free; this is what lets the
/// finished system "keep going" forever instead of freezing. Unbound/degenerate orbits pass
/// through unchanged (there is no closed orbit to coast).
fn kepler_advance(body: &Body, dt_year: f64) -> Body {
    use glam::DVec3;
    let mut out = body.clone();
    let mu = G * (M_STAR + body.mass);
    let r_vec = body.pos;
    let v_vec = body.vel;
    let r = r_vec.length();
    if r <= 0.0 || mu <= 0.0 {
        return out;
    }
    let v2 = v_vec.length_squared();
    if 0.5 * v2 - mu / r >= 0.0 {
        return out; // unbound — let it drift on its recorded state
    }
    let a = -mu / (v2 - 2.0 * mu / r); // = -μ / (2·energy)
    let h_vec = r_vec.cross(v_vec);
    if h_vec.length_squared() <= 0.0 {
        return out;
    }
    let e_vec = ((v2 - mu / r) * r_vec - r_vec.dot(v_vec) * v_vec) / mu;
    let e = e_vec.length();
    if e >= 1.0 {
        return out;
    }
    let n_hat = h_vec.normalize();
    let p_hat = if e > 1e-6 {
        e_vec / e
    } else {
        // Circular orbit: periapsis is arbitrary — pick any in-plane axis (as orbit_ellipse does).
        let aref = if n_hat.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
        (aref - n_hat * aref.dot(n_hat)).normalize()
    };
    let q_hat = n_hat.cross(p_hat);
    // Current true → eccentric → mean anomaly, advance the mean anomaly, solve back to true.
    let nu0 = r_vec.dot(q_hat).atan2(r_vec.dot(p_hat));
    let ecc0 = ((1.0 - e).sqrt() * (nu0 * 0.5).sin()).atan2((1.0 + e).sqrt() * (nu0 * 0.5).cos());
    let m0 = ecc0 - e * ecc0.sin();
    let mean_motion = (mu / (a * a * a)).sqrt();
    let m = m0 + mean_motion * dt_year;
    let mut ea = m; // Newton iteration on Kepler's equation M = E − e·sin E
    for _ in 0..8 {
        ea -= (ea - e * ea.sin() - m) / (1.0 - e * ea.cos());
    }
    let nu = 2.0 * ((1.0 + e).sqrt() * (ea * 0.5).sin()).atan2((1.0 - e).sqrt() * (ea * 0.5).cos());
    let rr = a * (1.0 - e * ea.cos());
    let p = a * (1.0 - e * e);
    let speed = (mu / p).sqrt();
    out.pos = (p_hat * nu.cos() + q_hat * nu.sin()) * rr;
    out.vel = (p_hat * (-nu.sin()) + q_hat * (e + nu.cos())) * speed;
    out
}

/// A stable per-composition key for caching a planet's composed globe — so a settled planet
/// reuses one built mesh across frames (composition only changes at collisions, never during the
/// coast). FNV-1a over the five class masses, rounded to shed float noise.
fn globe_key(comp: &Composition) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &m in &comp.mass {
        let q = (m * 1.0e12) as u64; // masses ~1e-6..1e-3 M☉
        h = (h ^ q).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// A gentle, per-giant tilt for its ring plane (derived from position for variety) so rings
/// read as tilted discs rather than lying flat in every orbit plane.
fn ring_tilt(pos: Vec3) -> Mat4 {
    let ax = 0.38 + 0.30 * (pos.x * 0.6).sin();
    let az = 0.22 * (pos.z * 0.5).cos();
    Mat4::from_rotation_x(ax) * Mat4::from_rotation_z(az)
}

/// A giant's emergent ring (a render-time classifier, like the habitability verdict — it never
/// feeds the sim). `ice` is the icy fraction of the shredded debris (→ a bright water-ice ring
/// vs a dark rocky one), `strength` its normalised mass (→ prominence).
struct RingSpec {
    ice: f32,
    strength: f32,
}

/// **Procedural ring driver.** A giant grows a ring from satellites it **tidally shredded** —
/// captured bodies small enough to disrupt inside the Roche limit (`RING_MOON_MAX`). The ring
/// *is* that debris; bigger captured moons survive (rendered as spheres). So rings are
/// **conditional**: a giant that shredded nothing has none, an icy shred makes a bright ring, a
/// rocky one a dark faint ring. Not art-applied to every giant.
fn ring_spec(body: &Body) -> Option<RingSpec> {
    if body.kind != BodyKind::Giant {
        return None;
    }
    let mut mass = 0.0;
    let mut ice = 0.0;
    for m in &body.moons {
        if m.mass < RING_MOON_MAX {
            mass += m.mass;
            ice += m.comp.get(MaterialClass::Ice);
        }
    }
    if mass < RING_MIN_MASS {
        return None;
    }
    Some(RingSpec {
        ice: (ice / mass) as f32,
        strength: (mass / RING_REF_MASS).clamp(0.2, 1.0) as f32,
    })
}

/// The choreographed camera pose `(yaw, pitch, distance)` at formation time `t ∈ [0,1]`.
/// It opens **just below the disk plane, edge-on and well out** — looking *through* the dust
/// toward the star (which the cloud occludes, with rays leaking through the gaps) — then
/// gently **rises above** the plane and **glides inward** as the cloud clears and the planets
/// settle. A slow, languid Star-Trek-titles pass. Eased so the motion is gentle.
fn cinematic_pose(t: f32) -> (f32, f32, f32) {
    let e = {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t) // smoothstep
    };
    let lerp = |a: f32, b: f32| a + (b - a) * e;
    let outer = DISK_OUTER as f32;
    let yaw = lerp(0.15, 1.05); // slow drift for parallax
    let pitch = lerp(-0.12, 0.66); // just below the plane (edge-on) → above it
    let distance = lerp(outer * 2.72, outer * 0.52); // open ~15% tighter, looking through the cloud → in
    (yaw, pitch, distance)
}

/// Visible billboard size (AU) for a body of the given physical radius and kind.
fn body_size(radius_au: f64, kind: BodyKind) -> f32 {
    let base = (radius_au * VISUAL_INFLATION) as f32;
    let lo = if kind == BodyKind::Debris { 0.035 } else { MIN_BODY };
    base.clamp(lo, MAX_BODY)
}

/// Which bodies the protoplanet list shows (indices into `bodies`): the most massive few,
/// **plus** every currently-habitable world — so every blue-ringed planet is also listed
/// (no "two rings, one row" mismatch). Sorted largest-first.
fn displayed(bodies: &[Body]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..bodies.len()).collect();
    idx.sort_by(|&a, &b| bodies[b].mass.total_cmp(&bodies[a].mass));
    let mut shown: Vec<usize> = idx.iter().copied().take(8).collect();
    for &i in &idx {
        if shown.len() >= 12 {
            break;
        }
        if !shown.contains(&i) && assess(&bodies[i]).playable {
            shown.push(i);
        }
    }
    shown
}

impl Scene for SolarSystem {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.006, 0.008, 0.014, 1.0]; // deep space
        self.disc = Some(renderer.load_texture(&disk::disc_texture(), 16, 16));
        self.ring = Some(renderer.load_texture(&disk::ring_texture(), 32, 32));
        if self.ring_mesh.is_none() {
            // A unit ring annulus (radii in giant-radii); coloured per giant at draw time.
            let (rv, ri) = planet::ring_mesh(1.35, 1.95, 72, 9);
            self.ring_mesh = Some(renderer.upload_mesh(&rv, MeshIndices::U32(&ri)));
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        // Camera: while the cinematic is armed it's driven by the formation clock, then keeps a
        // slow orbit once the system settles; the first drag hands manual control back.
        self.cam.update(input, !self.cinematic);
        if self.cinematic {
            if input.mouse_left {
                self.cinematic = false; // user grabbed the camera
            } else if self.coasting {
                // Settled system: hold a stable settled view (the old comet flyby is gone — it
                // swept the camera every frame, so a slow frame read as a random new angle). Drag
                // to take manual orbit control.
                let (yaw, pitch, distance) = cinematic_pose(1.0);
                self.cam.set_pose(yaw, pitch, distance);
            } else {
                let (yaw, pitch, distance) = cinematic_pose(self.t);
                self.cam.set_pose(yaw, pitch, distance);
            }
        }

        let space = input.key_down(Key::Space);
        if space && !self.prev_space {
            self.play = !self.play;
        }
        self.prev_space = space;

        let r = input.key_down(Key::R);
        if r && !self.prev_r {
            self.reseed();
        }
        self.prev_r = r;

        // [ / ] dial the seeding-supernova size down/up and re-form this seed's disk.
        let lb = input.key_down(Key::LeftBracket);
        if lb && !self.prev_lbracket {
            self.dial_supernova(-0.08);
        }
        self.prev_lbracket = lb;
        let rb = input.key_down(Key::RightBracket);
        if rb && !self.prev_rbracket {
            self.dial_supernova(0.08);
        }
        self.prev_rbracket = rb;

        // Up/Down cycle the selected body in the (live) protoplanet list.
        let n = displayed(self.current_bodies()).len().max(1);
        let up = input.key_down(Key::Up);
        if up && !self.prev_up {
            self.selected = (self.selected + n - 1) % n;
        }
        self.prev_up = up;
        let down = input.key_down(Key::Down);
        if down && !self.prev_down {
            self.selected = (self.selected + 1) % n;
        }
        self.prev_down = down;

        // Advance the clock. ←/→ scrub the formation, Space-driven playback otherwise. Once the
        // formation reaches the end the system doesn't freeze — it settles into a Keplerian
        // **coast** and just keeps orbiting (no pause, no 150 Myr stop).
        if self.play {
            self.anim_time += dt.as_secs_f32();
        }
        let scrub = dt.as_secs_f32() * 0.4;
        if input.key_down(Key::Left) {
            // Rewinding drops out of the coast and back into the recorded formation.
            self.coasting = false;
            self.coast_year = 0.0;
            self.t = (self.t - scrub).max(0.0);
        }
        if input.key_down(Key::Right) {
            if self.coasting {
                self.coast_year += scrub as f64 * self.coast_rate();
            } else {
                self.t += scrub;
            }
        }
        if self.play {
            if self.coasting {
                self.coast_year += dt.as_secs_f32() as f64 * self.coast_rate();
            } else {
                self.t += dt.as_secs_f32() * self.speed;
            }
        }
        if !self.coasting && self.t >= 1.0 {
            self.t = 1.0;
            self.coasting = true; // cross from forming into the orbiting coast
        }
        self.refresh_live();
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        renderer.set_camera(&self.cam.camera());
        let Some(disc) = self.disc else { return };

        // Deep-space galactic background: the sky pass renders a Milky Way band + star field
        // at "night", so we push the sun *and* moon below the horizon (full night, no discs)
        // and set a near-black sky gradient. The dust then composites over it — dense dust
        // occludes the stars into dark lanes (the galactic-core look).
        renderer.draw_sky();
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.0, -1.0, 0.0),
            sun_color: Vec3::ZERO,
            moon_dir: Vec3::new(0.0, -1.0, 0.0),
            moon_color: Vec3::ZERO,
            // The star is a **point light at the origin**: every planet mesh is shaded
            // per-fragment from its own direction to the star (a correct day/night terminator),
            // over a faint ambient floor so the night side isn't pure black. The parallel
            // sun/moon lights stay off, so the starfield sky stays dark.
            ambient: Vec3::splat(0.07),
            point_pos: Vec3::ZERO,
            point_color: Vec3::new(1.0, 0.94, 0.84), // warm starlight
            sky_zenith: Vec3::new(0.004, 0.006, 0.014),
            sky_horizon: Vec3::new(0.007, 0.010, 0.022),
            ..SceneLighting::default()
        });

        // (The star is rendered *inside* the volumetric pass so the dust occludes it — no
        // separate star billboard here, or it'd draw on top and never be blocked.)

        // The protoplanetary disk — a real **volumetric** raymarched dust cloud (a shader,
        // not sprites). Its density dissipates inside-out with the formation clock and carves
        // annular gaps at the forming planets' orbits, so the cloud *is* the disk being
        // consumed, driven by the sim — not a decorative field on a timer.
        self.set_disk_cloud(renderer);

        // The bodies at the current instant — recorded while forming, Kepler-coasted once
        // settled. Planets/giants are **composed into star-lit 3D spheres from their element
        // composition** (gas giants get swirling banded atmospheres), with an atmospheric glow
        // halo and small lit moon spheres; belt objects stay dim billboards.
        //
        // Free last frame's meshes (drawn last frame → safe now), then rebuild this frame's so
        // the day/night terminator stays correct as the bodies orbit. Cheap vs the volumetric.
        for h in self.body_meshes.drain(..) {
            renderer.free_mesh(h);
        }
        // Snapshot what we draw so we can upload meshes while pushing handles into
        // `self.body_meshes` (we can't hold a `&self.live` borrow across that mutation).
        struct Draw {
            pos: Vec3,
            comp: Composition,
            size: f32,
            /// Hex-globe resolution (spec §8): a solid world scales `freq` with its size
            /// (Mercury≈48 / Earth≈100); a gas giant is pinned at the Mercury count.
            hex_freq: u32,
            belt: bool,
            moons: Vec<Composition>,
            ring: Option<RingSpec>,
        }
        let draws: Vec<Draw> = self
            .live
            .iter()
            .map(|b| {
                let giant = b.kind == BodyKind::Giant;
                Draw {
                    pos: b.pos.as_vec3(),
                    comp: b.comp.clone(),
                    size: body_size(b.physical_radius(), b.kind),
                    // Both scale by real size; a giant just uses half the tiles (coarser hexes).
                    hex_freq: if giant {
                        hex_freq_for_giant(b.physical_radius())
                    } else {
                        hex_freq_for_radius(b.physical_radius())
                    },
                    belt: b.kind == BodyKind::Debris || b.mass < BELT_MASS,
                    // Surviving moons only — the small ones a giant shredded become its ring.
                    moons: b
                        .moons
                        .iter()
                        .filter(|m| !(giant && m.mass < RING_MOON_MAX))
                        .map(|m| m.comp.clone())
                        .collect(),
                    ring: ring_spec(b),
                }
            })
            .collect();

        // This frame's evolution advance, in crossfade-fractions (≡ simulation steps): the wall
        // time elapsed since last frame × the pace. Derived from `anim_time`, which only advances
        // during playback, so pausing freezes the simulation. Clamped so a long frame can't burst.
        // Real seconds since last frame (the evolution cadence runs on this, capped one step/frame).
        // `anim_time` only advances during playback, so pausing freezes the evolution.
        let dt = (self.anim_time - self.last_evo_time).max(0.0);
        self.last_evo_time = self.anim_time;

        let mut globe_used: HashSet<u64> = HashSet::new();
        for d in &draws {
            if d.belt {
                // Leftover planetesimal / belt object — too small to sphere; a dim grey dot.
                renderer.draw_billboard_additive(disc, d.pos, Vec2::splat(0.06), Vec2::ZERO, Vec2::ONE, [0.60, 0.62, 0.68, 0.55]);
                continue;
            }
            let model = Mat4::from_translation(d.pos) * Mat4::from_scale(Vec3::splat(d.size));
            // ONE builder for EVERY body — a composition hex globe (Epoch-1), cached by
            // composition+freq and lit by the star point light. Giants are simply capped to a
            // coarse freq via `hex_freq`; nothing renders any other way. Coarse while actively
            // forming (many bodies), full resolution once stopped/settled.
            // Render at a capped viewer LOD: settled bodies use their size-derived `hex_freq` but
            // clamped to `VIEWER_EVO_FREQ` so an evolving globe's per-step mesh build + upload stays
            // cheap; coarse while actively forming (many transient bodies churning).
            let freq = if self.coasting || !self.play {
                d.hex_freq.min(VIEWER_EVO_FREQ)
            } else {
                COARSE_FREQ
            };
            let key = globe_key(&d.comp) ^ ((freq as u64) << 40);
            globe_used.insert(key);
            if !self.globe_cache.contains_key(&key) {
                // The composition picks the distribution: a gas-dominated body is liquefied air
                // (differential-rotation swirl), everything else an Epoch-1 solid spread. That grid
                // is the initial state (S₀); the `Globe` then evolves it through Epoch 3 — a gas
                // giant advects its air, a solid differentiates its crust (iron draining out). The
                // icosphere is built **once** here and cached in the `Globe` (never rebuilt per frame).
                let topo = worldglobe::Topo::new(freq);
                let is_gas = d.comp.dominant() == Some(MaterialClass::Gas);
                let states = if is_gas {
                    let n_bands = (6.0 + (d.comp.total() / M_EARTH).max(1.0).log10() as f32 * 4.0).clamp(6.0, 18.0);
                    worldglobe::materialize_gas(&topo, n_bands)
                } else {
                    let abundance = d.comp.to_epoch1_abundance(&self.tables);
                    worldglobe::materialize_solid(&self.tables, abundance, &topo, key)
                };
                // Liquid look: water/ice-rich solids get a wet/icy sheen (gloss scales with water
                // content; full by ~50% water); gas and dry rock stay matte. Tweak the factor here.
                let gloss = if is_gas { 0.0 } else { (d.comp.water_fraction() * 2.0).clamp(0.0, 1.0) as f32 };
                self.globe_cache.insert(key, Globe::new(renderer, topo, states, key, is_gas, gloss));
            }
            self.globe_cache.get_mut(&key).unwrap().advance(renderer, &self.tables, dt, model);

            // A ring — only on giants that actually shredded a satellite (`ring_spec`), its
            // brightness from the debris mass and its hue from how icy that debris was. The
            // cached unit ring is tilted, scaled to the giant, and tinted accordingly.
            if let (Some(spec), Some(rh)) = (&d.ring, self.ring_mesh) {
                let bright = 0.45 + 0.55 * spec.strength;
                let tint = [
                    (0.55 + 0.29 * spec.ice) * bright, // rocky tan → icy white-blue
                    (0.50 + 0.37 * spec.ice) * bright,
                    (0.46 + 0.47 * spec.ice) * bright,
                    1.0,
                ];
                let model = Mat4::from_translation(d.pos) * ring_tilt(d.pos) * Mat4::from_scale(Vec3::splat(d.size));
                renderer.draw_mesh(rh, model, MeshDrawOptions { wireframe: false, tint, ..Default::default() });
            }

            // Moons — the same scheme, just small: a composition hex globe each (cached,
            // coarse), on a tilted orbit so they clear the body's silhouette. No glow.
            let n = d.moons.len().min(MAX_MOON_SPHERES);
            for (i, mcomp) in d.moons.iter().take(n).enumerate() {
                let orbit_r = d.size * (1.5 + 0.4 * i as f32) + 0.2;
                let phase = self.anim_time * 0.5 + i as f32 * (TAU / n as f32 + 1.7);
                let incl = 0.5_f32;
                let off = Vec3::new(orbit_r * phase.cos(), orbit_r * incl.sin() * phase.sin(), orbit_r * incl.cos() * phase.sin());
                let mpos = d.pos + off;
                let msize = (d.size * 0.22).clamp(0.12, 0.34);
                let mkey = globe_key(mcomp) ^ ((COARSE_FREQ as u64) << 40);
                globe_used.insert(mkey);
                if !self.globe_cache.contains_key(&mkey) {
                    let topo = worldglobe::Topo::new(COARSE_FREQ);
                    let abundance = mcomp.to_epoch1_abundance(&self.tables);
                    let states = worldglobe::materialize_solid(&self.tables, abundance, &topo, mkey);
                    // Moons are solid → they differentiate a crust, then hold; icy moons get a sheen.
                    let gloss = (mcomp.water_fraction() * 2.0).clamp(0.0, 1.0) as f32;
                    self.globe_cache.insert(mkey, Globe::new(renderer, topo, states, mkey, false, gloss));
                }
                let mmodel = Mat4::from_translation(mpos) * Mat4::from_scale(Vec3::splat(msize));
                self.globe_cache.get_mut(&mkey).unwrap().advance(renderer, &self.tables, dt, mmodel);
            }
        }
        // Evict cached globes whose planet is no longer drawn (reseed / scrubbed back to forming).
        let stale: Vec<u64> = self.globe_cache.keys().copied().filter(|k| !globe_used.contains(k)).collect();
        for k in stale {
            if let Some(g) = self.globe_cache.remove(&k) {
                renderer.free_mesh(g.mesh);
            }
        }

        self.flashes(renderer, disc);

        // Orbit ellipses for the actual planets/giants (skip debris + tiny belt objects),
        // reconstructed from each body's state vector — the real shapes, incl. eccentric ones.
        for b in self.current_bodies() {
            if b.kind == BodyKind::Debris || b.mass < BELT_MASS {
                continue;
            }
            let col = if b.kind == BodyKind::Giant {
                [0.65, 0.55, 0.42, 0.22]
            } else {
                [0.45, 0.55, 0.78, 0.22]
            };
            renderer.draw_lines(&orbit_ellipse(b), col);
        }

        // Reference rings: snow line + habitable-zone band.
        renderer.draw_lines(&ring(SNOW_LINE as f32, 128), [0.55, 0.80, 0.95, 0.45]);
        renderer.draw_lines(&ring(HZ_INNER as f32, 128), [0.45, 0.85, 0.50, 0.5]);
        renderer.draw_lines(&ring(HZ_OUTER as f32, 128), [0.45, 0.85, 0.50, 0.5]);

        // As the system settles, ring the viable starter worlds in blue.
        self.mark_viable_worlds(renderer);

        self.hud(renderer);
    }
}

impl SolarSystem {
    /// The bodies at the current playback instant — what the render, the list, the rings and
    /// the export all read, so they always agree and update live as the system evolves. Kept
    /// current by [`Self::refresh_live`] each update.
    fn current_bodies(&self) -> &[Body] {
        &self.live
    }

    /// Sim-years of orbital coast per second of playback. The formation is a fast geological
    /// time-lapse, but the settled system drops to [`COAST_YEARS_PER_SEC`] — a calm, watchable
    /// orbital pace — so a gas giant is a stable object you can track while its surface evolves,
    /// instead of the old `span_year × speed` blur (thousands of orbits per second).
    fn coast_rate(&self) -> f64 {
        COAST_YEARS_PER_SEC
    }

    /// Recompute [`Self::live`] for the current instant: the recorded snapshot while the
    /// formation plays, or the final settled system Kepler-advanced by `coast_year` once it
    /// has crossed into the coast.
    fn refresh_live(&mut self) {
        self.live = if self.coasting {
            match self.timeline.snapshots.last() {
                Some(s) => s.bodies.iter().map(|b| kepler_advance(b, self.coast_year)).collect(),
                None => Vec::new(),
            }
        } else {
            self.timeline
                .sample(self.t as f64)
                .map(|s| s.bodies.clone())
                .unwrap_or_default()
        };
    }

    /// Configure the volumetric dust cloud for this frame: disk geometry + the formation
    /// clock + annular gaps at the current planets'/giants' orbits (wider for more massive
    /// bodies, via the Hill radius). The renderer raymarches it behind the bodies.
    fn set_disk_cloud(&self, renderer: &mut Renderer) {
        // Only **giants** carve gaps — they're the bodies that actually open a wide annular
        // gap (the ALMA look). Carving one per embryo would shred the whole cloud to nothing.
        let mut gaps: Vec<(f32, f32)> = self
            .current_bodies()
            .iter()
            .filter(|b| b.kind == BodyKind::Giant)
            .map(|b| {
                let (a, _) = b.orbital_elements();
                let r_hill = a * (b.mass / (3.0 * M_STAR)).cbrt();
                let width = (r_hill * 9.0).clamp(0.6, DISK_OUTER * 0.3);
                (a as f32, width as f32)
            })
            .collect();
        gaps.truncate(MAX_VOLUMETRIC_BODIES);
        renderer.set_volumetric_disk(VolumetricDisk {
            inner: DISK_INNER as f32,
            outer: DISK_OUTER as f32,
            snow_line: SNOW_LINE as f32,
            scale_height: 0.07,
            density: 2.2, // enough opacity to occlude the starfield into dark dust lanes
            formation: self.t,
            time: self.t * 10.0, // a few inner-disk rotations of swirl over the run
            tint: Vec3::new(0.07, 0.06, 0.09), // dark dust — visible by occluding the stars
            glow: Vec3::new(1.0, 0.55, 0.26),  // warm star-lit centre
            gaps,
        });
    }

    /// Draw a pulsing blue ring around each **currently** habitable world — the same bodies
    /// the list marks PLAYABLE, so the rings and the list always agree as the system evolves.
    fn mark_viable_worlds(&self, renderer: &mut Renderer) {
        let Some(ringtex) = self.ring else { return };
        let pulse = 0.55 + 0.25 * (self.anim_time * 2.5).sin().abs();
        for body in self.current_bodies() {
            if !assess(body).playable {
                continue;
            }
            let size = (body_size(body.physical_radius(), body.kind) * 2.6).max(0.9);
            renderer.draw_billboard_additive(
                ringtex,
                body.pos.as_vec3(),
                Vec2::splat(size),
                Vec2::ZERO,
                Vec2::ONE,
                [0.35, 0.65, 1.0, pulse],
            );
        }
    }

    /// Bright expanding billboards for collisions near the current playback time.
    fn flashes(&self, renderer: &mut Renderer, disc: TextureHandle) {
        let now = self.t as f64 * self.timeline.span_year;
        let window = (self.timeline.span_year / 90.0).max(1.0e-3);
        for ev in &self.timeline.events {
            let age = now - ev.t_year;
            if age < 0.0 || age > window {
                continue;
            }
            let k = 1.0 - (age / window) as f32; // 1 at impact → 0 fading out
            let size = 0.6 + (1.0 - k) * 2.4;
            let col = match ev.regime {
                crate::collide::Regime::Disruption => [1.0, 0.97, 0.9],
                crate::collide::Regime::Erosion => [1.0, 0.8, 0.5],
                crate::collide::Regime::HitAndRun => [0.8, 0.9, 1.0],
                crate::collide::Regime::Merge => [1.0, 0.88, 0.6],
                crate::collide::Regime::Capture => [0.7, 0.8, 1.0], // a gentle moon capture
            };
            renderer.draw_billboard_additive(
                disc,
                ev.site.as_vec3(),
                Vec2::splat(size),
                Vec2::ZERO,
                Vec2::ONE,
                [col[0], col[1], col[2], 0.7 * k],
            );
        }
    }

    fn hud(&self, renderer: &mut Renderer) {
        renderer.draw_text("flicker · solar-system formation", Vec2::new(16.0, 16.0), 24.0, TEXT);
        // Cosmetic clock: formation maps 0..1 → 0..RUN_MYR; the coast keeps climbing past it.
        let span = self.timeline.span_year.max(1.0e-9);
        let sim_year = if self.coasting {
            span + self.coast_year
        } else {
            self.t as f64 * span
        };
        let myr = (sim_year / span) as f32 * RUN_MYR;
        // Space = freeze: a paused run is the **committed Epoch-1 seed** — the current
        // configuration (all bodies + their live compositions) is held fixed at this instant.
        let (status, status_col) = if self.play {
            let phase = if self.coasting { "coasting" } else { "forming" };
            (format!("▶ playing · {phase}"), [0.96, 0.86, 0.60, 1.0])
        } else {
            ("❚❚ FROZEN · Epoch-1 seed locked".to_string(), FROZEN)
        };
        renderer.draw_text(
            &format!("T = {myr:.1} Myr (scaled)   {status}"),
            Vec2::new(16.0, 50.0),
            18.0,
            status_col,
        );
        renderer.draw_text(
            "Space play/freeze (lock Epoch-1 seed) · ←/→ scrub · ↑/↓ select · [ ] supernova · R reseed · drag · wheel · Esc",
            Vec2::new(16.0, 74.0),
            13.0,
            DIM,
        );

        let tl = &self.timeline;
        let neb = &tl.nebula;
        let bodies = self.current_bodies();
        // Live IAU classification of the bodies *at this instant*: giant, a planet that has
        // cleared its orbital band, or a dwarf/belt object that hasn't. These evolve as you
        // play — early on a packed disk is mostly dwarfs, settling into a few planets.
        let giants = bodies.iter().filter(|b| b.kind == BodyKind::Giant).count();
        let planets = (0..bodies.len())
            .filter(|&i| bodies[i].kind != BodyKind::Giant && cleared_neighborhood(i, bodies))
            .count();
        let dwarfs = bodies.len() - giants - planets;
        let impacts_so_far = tl
            .events
            .iter()
            .filter(|e| e.t_year <= self.t as f64 * tl.span_year)
            .count();
        renderer.draw_text(
            &format!(
                "supernova {:.2} → disk {:.1}× MMSN · metallicity {:.1}× · {} embryos",
                neb.supernova_size,
                neb.sigma_1au / disk::SIGMA_MMSN,
                neb.metallicity,
                tl.seeded,
            ),
            Vec2::new(16.0, 98.0),
            13.0,
            [0.78, 0.74, 0.92, 1.0],
        );
        renderer.draw_text(
            &format!(
                "→ {} planets · {} giants · {} dwarf/belt · {} ejected · {} consumed · {} impacts",
                planets, giants, dwarfs, tl.ejected, tl.consumed, impacts_so_far
            ),
            Vec2::new(16.0, 116.0),
            13.0,
            DIM,
        );
        renderer.draw_text("snow line (blue) · habitable zone (green)", Vec2::new(16.0, 134.0), 12.0, DIM);

        // Protoplanet panel — the *current* bodies (largest-first + any habitable world),
        // updating live as the system evolves.
        let mut y = 158.0;
        renderer.draw_text("protoplanets (a · mass · water · verdict):", Vec2::new(16.0, y), 14.0, TEXT);
        y += 24.0;
        let shown = displayed(bodies);
        let sel = self.selected.min(shown.len().saturating_sub(1));
        for (row, &bi) in shown.iter().enumerate() {
            let body = &bodies[bi];
            let (a, _e) = body.orbital_elements();
            let mass_e = earth_masses(body.mass);
            let water = body.comp.water_fraction() * 100.0;
            let dom = body.dominant().map(MaterialClass::label).unwrap_or("—");
            let v = assess(body);
            let marker = if row == sel { "▶ " } else { "  " };
            let kind = if body.kind == BodyKind::Giant {
                "giant"
            } else if cleared_neighborhood(bi, bodies) {
                dom
            } else {
                "dwarf"
            };
            let moons = match body.moons.len() {
                0 => String::new(),
                1 => " ☾1".to_string(),
                n => format!(" ☾{n}"),
            };
            let line = format!(
                "{marker}{a:>5.2} AU  {mass_e:>6.2} M⊕  {water:>4.1}%  {kind:<6}{moons}  {}",
                v.summary()
            );
            let color = if v.playable { GOOD } else { DIM };
            renderer.draw_text(&line, Vec2::new(16.0, y), 13.0, color);
            y += 19.0;
        }

        self.export_panel(renderer, y + 8.0, shown.get(sel).map(|&bi| &bodies[bi]));
    }

    /// The selected survivor's element-abundance vector — the Epoch-1 hand-off.
    fn export_panel(&self, renderer: &mut Renderer, mut y: f32, body: Option<&Body>) {
        let Some(body) = body else {
            return;
        };
        // Frozen (Space) = this is the locked seed; playing = a live preview that updates as
        // the configuration evolves. Same numbers either way — freezing just holds them fixed.
        let (label, col) = if self.play {
            ("Epoch-1 composition (live · Space freezes the seed)", TEXT)
        } else {
            ("Epoch-1 SEED — LOCKED", FROZEN)
        };
        renderer.draw_text(
            &format!(
                "{label} · selected {:.2} M⊕, {:.1} g/cm³:",
                earth_masses(body.mass),
                body.comp.density()
            ),
            Vec2::new(16.0, y),
            14.0,
            col,
        );
        y += 22.0;
        let ab = body.comp.to_epoch1_abundance(&self.tables);
        let mut rows: Vec<(String, f64)> = ab.into_iter().collect();
        // Sort by abundance, breaking ties by symbol. The abundance map is a HashMap with
        // a randomized per-instance iteration order, and it's rebuilt every frame — without
        // a deterministic tie-break, equal-valued elements (e.g. several at 6.8) reshuffle
        // every frame and the row visibly thrashes.
        rows.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        // Fixed-width tokens (symbol padded to 2 so single- and two-letter symbols like
        // `Mg` line up) with a clear separator, wrapped to 5 per line so a long mix stays
        // inside the panel rather than crowding into the next symbol.
        let tokens: Vec<String> = rows
            .iter()
            .take(10)
            .map(|(s, v)| format!("{s:<2} {v:>4.1}"))
            .collect();
        for chunk in tokens.chunks(5) {
            renderer.draw_text(&chunk.join("     "), Vec2::new(16.0, y), 13.0, DIM);
            y += 18.0;
        }

        // Captured moons of the selected body (mass + type), if any.
        if !body.moons.is_empty() {
            y += 4.0;
            let moons: Vec<String> = body
                .moons
                .iter()
                .map(|m| {
                    let kind = m.comp.dominant().map(MaterialClass::label).unwrap_or("—");
                    format!("{:.3} M⊕ {kind}", earth_masses(m.mass))
                })
                .collect();
            renderer.draw_text(
                &format!("moons: {}", moons.join(" · ")),
                Vec2::new(16.0, y),
                13.0,
                [0.78, 0.80, 0.90, 1.0],
            );
        }
    }
}
