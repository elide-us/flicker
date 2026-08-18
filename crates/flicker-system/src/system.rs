//! The [`System`] facade — the top-level API a viewer (or a constraint search, or the Phase-3
//! planet sim) drives.
//!
//! It owns the whole pipeline state — the element set, the live [`SystemConfig`] levers, the
//! conserved cloud + mass layer (Phase 1), and the ignited collapse (Phase 2) — behind a small
//! set of verbs (`sync_distribution` / `reseed` / `ignite` / `clear` / `step`) and read models
//! (`state` / `epoch3_handoff` / `hot_spots`). Inputs come in as `SystemConfig`; outputs go out as
//! [`SystemState`] and [`Epoch3Handoff`] — the data contracts. No rendering, no engine deps.

use glam::Vec2;

use crate::cloud::CloudField;
use crate::collapse::{BodyType, Sim};
use crate::config::SystemConfig;
use crate::detect::{self, HotSpot};
use crate::mass::CloudMass;
use crate::model::{load_tables, Ejecta};

/// The whole star-system simulation, behind one handle.
pub struct System {
    ejecta: Ejecta,
    config: SystemConfig,
    cloud: CloudField,
    cloud_mass: CloudMass,
    sim: Option<Sim>,
}

impl System {
    /// Build a system from a config — loads the Prism vocabulary and derives the Phase-1 cloud +
    /// conserved mass layer. Not yet ignited (call [`ignite`](Self::ignite) to start Phase 2).
    pub fn new(config: SystemConfig) -> Self {
        let ejecta = Ejecta::from_tables(&load_tables());
        let cloud = CloudField::new(ejecta.elements.len(), config.seed, config.clump);
        let cloud_mass = CloudMass::derive(&ejecta, &config.mass);
        Self {
            ejecta,
            config,
            cloud,
            cloud_mass,
            sim: None,
        }
    }

    // ── read access (for the viewer / consumers) ──────────────────────────────────
    pub fn ejecta(&self) -> &Ejecta {
        &self.ejecta
    }
    pub fn config(&self) -> &SystemConfig {
        &self.config
    }
    pub fn config_mut(&mut self) -> &mut SystemConfig {
        &mut self.config
    }
    pub fn cloud(&self) -> &CloudField {
        &self.cloud
    }
    pub fn cloud_mass(&self) -> &CloudMass {
        &self.cloud_mass
    }
    pub fn sim(&self) -> Option<&Sim> {
        self.sim.as_ref()
    }
    pub fn is_ignited(&self) -> bool {
        self.sim.is_some()
    }

    // ── verbs ──────────────────────────────────────────────────────────────────────

    /// Re-derive the Phase-1 distribution from the current config dials (cheap; call after
    /// changing `cast` / `mass` / `clump`). Leaves any running collapse untouched.
    pub fn sync_distribution(&mut self) {
        self.cloud.strength = self.config.clump;
        self.cloud_mass = CloudMass::derive(&self.ejecta, &self.config.mass);
    }

    /// Roll a fresh cloud seed (a genuinely different distribution). Leaves any running collapse
    /// alone (the collapse is independent once ignited); pair with [`ignite`](Self::ignite) for a
    /// whole new system, or [`clear`](Self::clear) first to return to the distribution view.
    pub fn reseed(&mut self, seed: u32) {
        self.config.seed = seed;
        self.cloud.reseed(seed);
    }

    /// Ignite Phase 2: sample the conserved cloud into motes and begin the gravitational collapse.
    pub fn ignite(&mut self) {
        self.sim = Some(Sim::from_cloud(
            &self.ejecta,
            &self.config.cast,
            &self.cloud,
            &self.cloud_mass,
            self.config.motes_per_el,
            self.config.tuning,
        ));
    }

    /// Drop the collapse, back to the Phase-1 distribution view.
    pub fn clear(&mut self) {
        self.sim = None;
    }

    /// Advance the collapse by `dt_years` (no-op before ignition).
    pub fn step(&mut self, dt_years: f32) {
        if let Some(s) = self.sim.as_mut() {
            s.step(dt_years);
        }
    }

    // ── read models / contracts ─────────────────────────────────────────────────────

    /// The shear anchor — geometric-mean cast radius over the element set.
    pub fn anchor_au(&self) -> f32 {
        let (mut rmin, mut rmax) = (f32::MAX, 0.0f32);
        for el in &self.ejecta.elements {
            let d = self.config.cast.distance_au(el.atomic_mass);
            rmin = rmin.min(d);
            rmax = rmax.max(d);
        }
        (rmin.max(0.01) * rmax.max(0.01)).sqrt()
    }

    /// Phase-1 **hot spots** (overdensity seed sites, world-space) at sim-clock `time`.
    pub fn hot_spots(&self, time: f32) -> Vec<HotSpot> {
        detect::detect(
            &self.ejecta,
            &self.config.cast,
            &self.cloud,
            time,
            self.anchor_au(),
        )
    }

    /// A read-model snapshot of the live collapse — the output contract for status / UI / search.
    /// `None` before ignition.
    pub fn state(&self) -> Option<SystemState> {
        let sim = self.sim.as_ref()?;
        let bodies = (0..sim.mass.len())
            .filter(|&i| sim.alive[i])
            .map(|i| BodySnapshot {
                index: i,
                pos: sim.pos[i],
                vel: sim.vel[i],
                mass: sim.mass[i],
                ring_mass: sim.ring_mass[i],
                radius_au: sim.radius_au(i),
                kind: sim.classify(i),
                host: sim.orbit_host(i),
                playable: sim.is_playable(i),
            })
            .collect();
        Some(SystemState {
            time: sim.time,
            star_mass: sim.largest_mass(),
            total_mass: sim.total_mass(),
            init_total: sim.init_total(),
            bodies,
        })
    }

    /// The **Epoch-3 handoff** for the selected habitable-zone world — the readable material
    /// payload the single-world planet sim (Phase 3) is seeded from. `None` until a playable world
    /// exists. Picks the most-massive playable world (the strongest candidate when several qualify).
    pub fn epoch3_handoff(&self) -> Option<Epoch3Handoff> {
        let sim = self.sim.as_ref()?;
        let n_el = sim.n_el;
        // Most-massive playable body.
        let world = (0..sim.mass.len())
            .filter(|&i| sim.is_playable(i))
            .max_by(|&a, &b| sim.mass[a].total_cmp(&sim.mass[b]))?;
        let row = &sim.comp[world * n_el..(world + 1) * n_el];
        let composition = row
            .iter()
            .enumerate()
            .filter(|(_, &m)| m > 0.0)
            .map(|(e, &m)| ElementMass {
                symbol: self.ejecta.elements[e].symbol.clone(),
                number: self.ejecta.elements[e].number,
                mass_msun: m,
            })
            .collect();
        let moons = (0..sim.mass.len())
            .filter(|&j| sim.alive[j] && j != world && sim.orbit_host(j) == world)
            .count();
        Some(Epoch3Handoff {
            composition,
            total_mass_msun: sim.mass[world],
            orbit_radius_au: sim.pos[world].length(),
            star_mass_msun: sim.mass[0],
            moons,
            has_ring: sim.ring_mass[world] > 0.0,
        })
    }
}

/// One body in a [`SystemState`] snapshot. `index` is its stable per-frame id (also what `host`
/// refers to); `host == 0` ⇒ orbits the star (a planet), otherwise it's a moon of body `host`.
#[derive(Clone, Copy, Debug)]
pub struct BodySnapshot {
    pub index: usize,
    pub pos: Vec2,
    pub vel: Vec2,
    pub mass: f32,
    pub ring_mass: f32,
    pub radius_au: f32,
    pub kind: BodyType,
    pub host: usize,
    pub playable: bool,
}

impl BodySnapshot {
    /// The pinned central star (body 0).
    pub fn is_star(&self) -> bool {
        self.index == 0
    }
    /// A captured moon (orbits a planet, not the star).
    pub fn is_moon(&self) -> bool {
        self.host != 0
    }
}

/// Output snapshot of a running collapse — the read-model contract (counts + per-body state).
#[derive(Clone, Debug)]
pub struct SystemState {
    /// Elapsed simulated time (years).
    pub time: f32,
    /// Mass of the dominant body — the emergent star (M☉).
    pub star_mass: f32,
    /// Total live mass (M☉) — invariant; equals `init_total`.
    pub total_mass: f32,
    /// Starting total mass (M☉) — for a conservation check.
    pub init_total: f32,
    /// Every live body, star first.
    pub bodies: Vec<BodySnapshot>,
}

/// One element's conserved mass in a body's composition (the handoff payload unit).
#[derive(Clone, Debug)]
pub struct ElementMass {
    pub symbol: String,
    pub number: u8,
    pub mass_msun: f32,
}

/// The **Phase-2 → Phase-3 data contract**: the selected habitable-zone world's exact, conserved
/// elemental composition plus the context the planet sim needs to evolve it (its mass, orbital
/// distance and star mass for insolation, and whether it kept a moon / a ring).
#[derive(Clone, Debug)]
pub struct Epoch3Handoff {
    /// Exact per-element mass (M☉) of the world — the conserved material accounting.
    pub composition: Vec<ElementMass>,
    /// The world's total mass (M☉).
    pub total_mass_msun: f32,
    /// Orbital distance from the star (AU) — insolation context.
    pub orbit_radius_au: f32,
    /// The star's mass (M☉) — luminosity / insolation context.
    pub star_mass_msun: f32,
    /// Number of moons it holds (the "playable world with a moon" constraint reads this).
    pub moons: usize,
    /// Whether the world carries a ring.
    pub has_ring: bool,
}
