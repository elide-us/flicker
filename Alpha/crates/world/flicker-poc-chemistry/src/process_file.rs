//! **The pipeline as content** — `Alpha/content/data/processes.json` is the
//! authority on which transformations run, in what order, behind what gate.
//!
//! The maintainer composes the world from this file: remove an entry and the
//! process leaves the pipeline, reorder and it reorders, retune a gate and the
//! process waits for the new condition — no Rust involved. Adding NEW physics
//! still means writing the named transformation in this crate; the file
//! composes and gates what exists (the same split as the material tables:
//! behaviour lives in data, mechanism lives in code).
//!
//! A **gate** is a condition over the planet's own solved state, measured every
//! tick. The grammar is deliberately small — comparisons over named state
//! reads and levers, composed with all/any — and it deliberately CANNOT read
//! the clock: nothing in this world is scheduled, so there is no field for the
//! tick or the date, and a process that should start "later" must name the
//! chemistry that makes it later.

use std::path::Path;

use serde::Deserialize;

use crate::planet::PlanetState;
use crate::stage::{Stage, StageRng};
use crate::Levers;

/// One pipeline entry: which transformation runs, why (for the console and the
/// maintainer), and behind what gate.
#[derive(Debug, Clone, Deserialize)]
pub struct ProcessDef {
    /// The Rust transformation this entry runs — must name a registered stage.
    pub runs: String,
    /// What the process does, in the maintainer's language (display/docs).
    #[serde(default)]
    pub summary: String,
    /// What should be SEEN happening while it runs — the gate card's "watch
    /// for" line, written toward the gameplay objective (continents worth
    /// keeping).
    #[serde(default)]
    pub watch: String,
    /// **Which view actually shows this process working**, by its bench name
    /// (`heat`, `motion`, `rain`, …) — so the surface can put the maintainer in
    /// front of the right instrument for the era the world is in, instead of
    /// leaving ten buttons equally lit at every moment of a 4.5-billion-year
    /// run.
    ///
    /// Deliberately OPTIONAL and deliberately often empty: several
    /// transformations genuinely have nothing to show on the globe (the veneer
    /// is invisible, crystallisation is "quiet on the globe, loud in the
    /// ledger"), and claiming otherwise would send someone to stare at a view
    /// where nothing is happening. The roster of legal names lives with the
    /// views themselves — this crate is GPU-free and has no opinion about
    /// benches — and the bench pins every authored name to a real view.
    #[serde(default)]
    pub view: String,
    /// The condition measured every tick; the stage runs only while it holds.
    pub gate: Gate,
}

#[derive(Debug, Clone, Deserialize)]
struct ProcessFile {
    processes: Vec<ProcessDef>,
}

/// A gate condition — the whole grammar.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Gate {
    /// `true` / `false` — always / never (the boundary inputs use `true`).
    Always(bool),
    /// Every branch must hold.
    All { all: Vec<Gate> },
    /// Any branch may hold.
    Any { any: Vec<Gate> },
    /// One comparison: a named read against a number or another named read.
    Cmp {
        read: String,
        op: String,
        value: serde_json::Value,
    },
}

impl Gate {
    /// Measure the gate against the world's sampled state and the live levers.
    pub fn holds(&self, state: &PlanetState, levers: &Levers) -> bool {
        match self {
            Gate::Always(b) => *b,
            Gate::All { all } => all.iter().all(|g| g.holds(state, levers)),
            Gate::Any { any } => any.iter().any(|g| g.holds(state, levers)),
            Gate::Cmp { read, op, value } => {
                let lhs = resolve(read, state, levers)
                    .unwrap_or_else(|| panic!("processes.json reads unknown field '{read}'"));
                let rhs = match value {
                    serde_json::Value::Number(n) => n.as_f64().expect("finite gate number"),
                    serde_json::Value::String(s) => {
                        resolve(s, state, levers).unwrap_or_else(|| {
                            panic!("processes.json compares against unknown field '{s}'")
                        })
                    }
                    other => {
                        panic!("processes.json gate value must be a number or a read: {other}")
                    }
                };
                match op.as_str() {
                    "<" => lhs < rhs,
                    "<=" => lhs <= rhs,
                    ">" => lhs > rhs,
                    ">=" => lhs >= rhs,
                    other => panic!("processes.json gate op '{other}' (use < <= > >=)"),
                }
            }
        }
    }
}

/// Resolve a gate read: a planet-state field by name, or `lever:<name>` for a
/// live lever. The roster here is the one the file's `_meta.reads` documents —
/// extend BOTH when a new read earns its place. The clock is deliberately
/// absent: gates measure chemistry, never time.
fn resolve(name: &str, state: &PlanetState, levers: &Levers) -> Option<f64> {
    if let Some(lever) = name.strip_prefix("lever:") {
        return lever_read(lever, levers);
    }
    Some(match name {
        "mean_mantle_temp_k" => state.mean_mantle_temp_k,
        "min_mantle_temp_k" => state.min_mantle_temp_k,
        "max_mantle_temp_k" => state.max_mantle_temp_k,
        "differentiation_frac" => state.differentiation_frac,
        "crust_frac" => state.crust_frac,
        "continental_frac" => state.continental_frac,
        "lid_frac" => state.lid_frac,
        "mean_elevation_m" => state.mean_elevation_m,
        "sea_level_m" => state.sea_level_m,
        "submerged_frac" => state.submerged_frac,
        "ocean_mass_kg" => state.ocean_mass_kg,
        "atmosphere_mass_kg" => state.atmosphere_mass_kg,
        "water_vapour_kg" => state.water_vapour_kg,
        "delivered_water_kg" => state.delivered_water_kg,
        "p_co2" => state.p_co2,
        "greenhouse_k" => state.greenhouse_k,
        "mean_strata" => state.mean_strata,
        "compounds_kg" => state.compounds_kg,
        _ => return None,
    })
}

/// The numeric levers a gate may read or compare against.
fn lever_read(name: &str, levers: &Levers) -> Option<f64> {
    Some(match name {
        "water_budget_kg" => levers.water_budget_kg,
        "water_coverage_target" => levers.water_coverage_target,
        "water_delivery_rate" => levers.water_delivery_rate,
        "veneer_budget_kg" => levers.veneer_budget_kg,
        "core_heat" => levers.core_heat,
        "stellar_heat" => levers.stellar_heat,
        "crust_gen_rate" => levers.crust_gen_rate,
        "arc_return" => levers.arc_return,
        "outgas_rate" => levers.outgas_rate,
        "eruption_rate" => levers.eruption_rate,
        "production_rate" => levers.production_rate,
        "decomposer_niche_kg" => levers.decomposer_niche_kg,
        "yield_strain" => levers.yield_strain as f64,
        "erosion_rate" => levers.erosion_rate,
        "leach_rate" => levers.leach_rate,
        _ => return None,
    })
}

/// Load the pipeline roster from the content directory. Loud on every failure —
/// the file is content, and a world forged against a broken roster is a world
/// forged wrong.
pub fn load_processes(dir: &Path) -> Vec<ProcessDef> {
    let path = dir.join("processes.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("processes.json missing at {}: {e}", path.display()));
    let file: ProcessFile = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("processes.json does not parse: {e}"));
    assert!(
        !file.processes.is_empty(),
        "processes.json names no processes at all"
    );
    file.processes
}

/// A registered transformation wrapped in its authored gate: the stage supplies
/// the physics, the file supplies the condition. The pipeline holds only these.
pub struct Gated {
    inner: Box<dyn Stage>,
    gate: Gate,
    levers: Levers,
}

impl Gated {
    pub fn new(inner: Box<dyn Stage>, gate: Gate, levers: Levers) -> Self {
        Self {
            inner,
            gate,
            levers,
        }
    }
}

impl Stage for Gated {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn is_live(&self, state: &PlanetState) -> bool {
        self.gate.holds(state, &self.levers)
    }

    fn tick(&self, world: &mut crate::planet::World, dt_myr: f64, rng: &mut StageRng) {
        self.inner.tick(world, dt_myr, rng);
    }
}

/// Fetch one process's authored gate by stage name — the probe the gate tests
/// use, so a test measures exactly what the shipped file says.
#[cfg(test)]
pub(crate) fn gate_of(name: &str) -> Gate {
    load_processes(&crate::config::content_data_dir())
        .into_iter()
        .find(|p| p.runs == name)
        .unwrap_or_else(|| panic!("processes.json does not name '{name}'"))
        .gate
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file parses, names only registered reads, and its gate grammar
    /// evaluates — the authoring loop's smoke test.
    #[test]
    fn the_shipped_roster_parses_and_measures() {
        let defs = load_processes(&crate::config::content_data_dir());
        assert!(defs.len() >= 10, "the pipeline has its processes");
        let state = PlanetState::default();
        let levers = Levers::default();
        for def in &defs {
            // Every gate measures without panicking on a default state — which
            // also proves every named read resolves.
            let _ = def.gate.holds(&state, &levers);
            assert!(!def.summary.is_empty(), "'{}' explains itself", def.runs);
        }
    }

    /// Gate numbers that are COUPLED to constants inside a transformation's own
    /// tick are pinned equal, so the file cannot silently drift into gating a
    /// stage open while its tick no-ops (the tick-0 defect class).
    #[test]
    fn coupled_gate_numbers_match_the_physics() {
        fn number_in(gate: &Gate, read: &str) -> Option<f64> {
            match gate {
                Gate::Cmp { read: r, value, .. } if r == read => value.as_f64(),
                Gate::All { all: v } | Gate::Any { any: v } => {
                    v.iter().find_map(|g| number_in(g, read))
                }
                _ => None,
            }
        }
        let crust = gate_of("CrustGeneration");
        assert_eq!(
            number_in(&crust, "min_mantle_temp_k"),
            Some(crate::crust::SOLIDUS_K),
            "CrustGeneration's gate and its tick must agree on the solidus"
        );
        let volc = gate_of("Volcanism");
        assert_eq!(
            number_in(&volc, "max_mantle_temp_k"),
            Some(crate::crust::ERUPTION_FLOOR_K),
            "Volcanism's gate and its tick must agree on the melt floor"
        );
        let core = gate_of("CoreFormation");
        assert_eq!(
            number_in(&core, "max_mantle_temp_k"),
            Some(crate::interior::FE_SEGREGATION_K),
            "CoreFormation's gate and its tick must agree on segregation heat"
        );
        let outgas = gate_of("Outgassing");
        assert_eq!(
            number_in(&outgas, "max_mantle_temp_k"),
            Some(crate::atmosphere::LOWEST_RELEASE_FLOOR_K),
            "Outgassing's gate and the gas vocabulary must agree on the lowest floor"
        );
        let life = gate_of("Biosphere");
        assert_eq!(
            number_in(&life, "lid_frac"),
            Some(crate::biosphere::LID_FOR_LIFE),
            "Biosphere's gate and its detection threshold must agree on the lid"
        );
        assert_eq!(
            number_in(&life, "submerged_frac"),
            Some(crate::biosphere::SEA_FOR_LIFE),
            "…and on the sea"
        );
        let veneer = gate_of("LateVeneer");
        assert_eq!(
            number_in(&veneer, "differentiation_frac"),
            Some(crate::infall::VENEER_AFTER_DIFFERENTIATION),
            "LateVeneer's gate and its construction must agree on the core threshold"
        );
    }
}
