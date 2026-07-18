//! The M2 crust stages (spec §7.5 4–8): crust generation, arc magmatism,
//! subduction, and the relief that orogeny integrates. Airy isostasy itself is the
//! derived [`elevation_m`](crate::column::elevation_m) function, not a stage.
//!
//! **What M2 must produce (spec §10):** *a bimodal hypsometric curve* and an
//! Earth-like crust, **grown, not seeded**. Both fall out of one idea — the crust
//! is partial melt drawn from the (now Fe-depleted) mantle, and *how refined* the
//! melt is decides everything:
//!
//! - **Spreading centres** (divergent flow) tap a mafic melt → dense, thin
//!   **oceanic** basalt that sits low and is recycled by subduction.
//! - **Convergent arcs** tap a refined, silica-rich melt → light, buoyant
//!   **continental** crust that rides high and is *not* subducted, so it thickens
//!   and persists.
//!
//! The two densities (from composition, [`density_kg_m3`](crate::column::density_kg_m3))
//! and the two thicknesses (oceanic recycled, continental accumulating) both push
//! the same way through **absolute Airy isostasy** → two clusters of elevation.
//! Nobody sets "continents are high"; it emerges.
//!
//! Where crust forms is read from the plate flow's **divergence** (built in M1):
//! outflow = a ridge, inflow = a trench. Every gram moves mantle↔crust through the
//! conserved [`deposit`](crate::column::Column::deposit) /
//! [`subduct_top`](crate::column::Column::subduct_top) pair; the audit confirms it.

use flicker_materials::ElementId;

use glam::Vec3;

use crate::interior::tangent_gradient;
use crate::column::FormationProcess;
use crate::planet::World;
use crate::stage::{Stage, StageRng};

/// Basaltic (mafic) melt affinity per element — the fraction of the mantle's
/// mass of that element that a spreading-centre melt draws up. Keeps real Mg/Fe/Ca
/// (mafic → dense), so oceanic crust sits low.
fn oceanic_affinity(e: ElementId) -> f64 {
    match e {
        8 => 0.55,  // O
        12 => 0.35, // Mg — mafic melts carry magnesium
        14 => 0.55, // Si
        26 => 0.50, // Fe
        20 => 0.70, // Ca
        13 => 0.65, // Al
        11 => 0.60, // Na
        19 => 0.60, // K
        22 => 0.60, // Ti
        16 => 0.30, // S
        _ => 0.30,
    }
}

/// Refined felsic melt affinity per element — a more evolved, silica-rich melt.
/// Strips the incompatibles (Si/Al/K/Na) and leaves Mg/Fe behind, so continental
/// crust is light and rides high.
fn continental_affinity(e: ElementId) -> f64 {
    match e {
        8 => 0.55,  // O
        12 => 0.08, // Mg — left in the residue (felsic → Mg-poor)
        14 => 0.70, // Si — silica-rich
        26 => 0.15, // Fe
        20 => 0.30, // Ca
        13 => 0.70, // Al
        11 => 0.90, // Na
        19 => 0.95, // K — the most incompatible
        3 => 0.92,  // Li
        92 => 0.95, // U (concentrates in continental crust — real)
        22 => 0.30, // Ti
        _ => 0.30,
    }
}

/// Crust solidifies only once a cell's mantle has cooled below this proxy solidus.
/// A magma ocean grows no crust; as the planet cools, crust **freezes over the
/// whole surface** (a temperature — i.e. chemistry — gate, never the tick number).
/// One well-mixed mantle temperature stands in for the surface at M1/M2, and the
/// real surface is far cooler, so this is an abstraction of "the surface can now
/// solidify," not a literal basalt solidus.
const SOLIDUS_K: f64 = 3700.0;
/// How fast a cooled cell approaches its saturation crust thickness, per Myr.
const CRUST_GEN_RATE: f64 = 0.03;
/// Saturation thickness targets, as a fraction of the cell's initial mass (so the
/// target is grid-frequency-independent). Oceanic basalt saturates **thin** — a
/// fixed few-km layer that blankets the seafloor; continental crust builds far
/// **thicker** because arc magmatism keeps adding and it is not recycled. The two
/// targets (~8×) are the thickness half of the bimodal hypsometry; density is the
/// other half.
const OCEANIC_SAT_FRAC: f64 = 0.0015;
const CONTINENTAL_SAT_FRAC: f64 = 0.012;
/// Fraction of the top oceanic bed consumed per Myr at full convergence — the
/// seafloor recycles (oceanic crust ages to ~100–200 Myr; continents to Gyr).
const SUBDUCTION_RATE: f64 = 0.03;
/// Below this normalised |divergence| a cell is plate interior — neither ridge nor
/// trench — and does nothing.
const BOUNDARY_CUTOFF: f64 = 0.05;

/// Relief gained per Myr at full convergence where continental crust collides
/// (orogeny raises mountains — it moves mass + integrates relief, never elevation).
const OROGENY_UPLIFT_M: f64 = 400.0;
/// Relief lost per Myr everywhere (stand-in for erosional decay until M6).
const RELIEF_DECAY: f64 = 0.002;

/// Surface divergence of the convection velocity per cell: `∂(v·e1)/∂e1 +
/// ∂(v·e2)/∂e2`, each a **least-squares gradient** over the neighbours (the same
/// geometry-aware fit the flow uses) so it does NOT print the icosphere seams
/// through. A raw neighbour sum pins continents to the shard boundaries, and badly
/// so at high resolution. Positive = spreading (a ridge), negative = converging (a
/// trench). Read-only — computed before any stage mutates.
fn divergence_field(world: &World) -> Vec<f32> {
    let dirs = &world.grid.dirs;
    let vel = &world.mantle.velocity;
    (0..world.mantle.n_cells())
        .map(|i| {
            let pi = dirs[i];
            let seed = if pi.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            let e1 = (seed - pi * seed.dot(pi)).normalize_or_zero();
            let e2 = pi.cross(e1);
            let vi1 = vel[i].dot(e1);
            let vi2 = vel[i].dot(e2);
            // ∂/∂e1 of (v·e1) and ∂/∂e2 of (v·e2) — the surface divergence, from the
            // geometry-aware least-squares gradient, not a grid-imprinting raw sum.
            let g1 = tangent_gradient(
                pi,
                world.grid.neighbors[i].iter().map(|&j| (dirs[j as usize], vel[j as usize].dot(e1) - vi1)),
            );
            let g2 = tangent_gradient(
                pi,
                world.grid.neighbors[i].iter().map(|&j| (dirs[j as usize], vel[j as usize].dot(e2) - vi2)),
            );
            g1.dot(e1) + g2.dot(e2)
        })
        .collect()
}

/// The largest |divergence|, for normalising the boundary activity to `[-1, 1]`
/// (so the generation rate is scale-independent of the convection speed).
fn max_abs(field: &[f32]) -> f64 {
    field.iter().map(|d| d.abs()).fold(0.0f32, f32::max).max(1e-9) as f64
}

/// **CrustGeneration** — the surface freezes into crust. A cell grows crust once
/// its mantle has cooled below the [`SOLIDUS_K`] proxy, so crust **covers the whole
/// surface** as the planet cools (bare mantle survives only where it is still a
/// magma ocean). Convergent arcs refine felsic *continental* crust (thick,
/// buoyant); ridges **and plate interiors alike** solidify mafic *oceanic* crust —
/// so the seafloor is blanketed and continents are the exceptions, as on a real
/// planet. Each melt is drawn from the mantle cell by affinity and deposited on top
/// of the column — mantle→crust, conserved.
pub struct CrustGeneration;

impl Stage for CrustGeneration {
    fn name(&self) -> &'static str {
        "CrustGeneration"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let div = divergence_field(world);
        let maxd = max_abs(&div);
        let elems: Vec<ElementId> = world.mantle.elements().to_vec();
        let m0_per_cell = world.budget.total() / world.mantle.n_cells() as f64;

        for (cell, &dv) in div.iter().enumerate() {
            // A magma-ocean cell grows no crust; crust freezes over the surface only
            // once the mantle has cooled below the solidus proxy.
            if world.mantle.temp_k[cell] > SOLIDUS_K {
                continue;
            }
            let d = dv as f64 / maxd; // -1 (trench) .. +1 (ridge)
            // Convergent arcs → felsic continental crust; ridges AND interiors →
            // mafic oceanic crust (the default cooled seafloor).
            let (process, affinity, sat_frac): (FormationProcess, fn(ElementId) -> f64, f64) =
                if d < -BOUNDARY_CUTOFF {
                    (FormationProcess::ContinentalArc, continental_affinity, CONTINENTAL_SAT_FRAC)
                } else {
                    (FormationProcess::OceanicCrust, oceanic_affinity, OCEANIC_SAT_FRAC)
                };

            // Negative feedback: growth slows to zero as the crust reaches its
            // saturation thickness. Oceanic saturates thin, continental thick.
            let fill = (1.0 - world.columns[cell].mass_kg() / (sat_frac * m0_per_cell)).max(0.0);
            if fill <= 0.0 {
                continue;
            }
            let step = CRUST_GEN_RATE * fill * dt_myr;
            let mut melt: Vec<(ElementId, f64)> = Vec::new();
            for &e in &elems {
                let want = step * affinity(e) * world.mantle.mass(cell, e);
                if want > 0.0 {
                    let took = world.mantle.remove(cell, e, want);
                    if took > 0.0 {
                        melt.push((e, took));
                    }
                }
            }
            if !melt.is_empty() {
                world.columns[cell].deposit(process, world.tick_myr, &melt);
            }
        }
    }
}

/// **Subduction** — at convergent cells, the youngest oceanic crust is consumed and
/// returned to the mantle (crust→mantle, conserved); continental crust is *not*
/// subducted, so it persists. Collisions of continental crust integrate **relief**
/// (orogeny), and relief decays slowly everywhere. Slab dehydration → volatiles is
/// deferred to M3.
pub struct Subduction;

impl Stage for Subduction {
    fn name(&self) -> &'static str {
        "Subduction"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let div = divergence_field(world);
        let maxd = max_abs(&div);

        for (cell, &dv) in div.iter().enumerate() {
            let d = dv as f64 / maxd;

            // Everywhere: relief relaxes (erosional decay stand-in until M6).
            world.columns[cell].relief_m *= 1.0 - RELIEF_DECAY * dt_myr;

            if d < -BOUNDARY_CUTOFF {
                let convergence = -d;
                // Consume the top oceanic bed → back to the mantle.
                let frac = (SUBDUCTION_RATE * convergence * dt_myr).min(1.0);
                let returned = world.columns[cell].subduct_top(FormationProcess::OceanicCrust, frac);
                for (e, m) in returned {
                    world.mantle.add(cell, e, m);
                }
                // Continental crust here instead means a collision → uplift (relief).
                if crate::column::crust_kind(&world.columns[cell])
                    == crate::column::CrustKind::Continental
                {
                    world.columns[cell].relief_m += OROGENY_UPLIFT_M * convergence * dt_myr;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::budget::Budget;
    use crate::column::{crust_kind, elevation_m, CrustKind};
    use crate::config::content_data_dir;
    use crate::planet::World;
    use crate::scheduler::Scheduler;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    /// A world run through the full formation pipeline for `ticks` Myr.
    fn run(freq: u32, seed: u64, ticks: usize) -> World {
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, seed);
        let mut s = Scheduler::new(crate::formation_stages(), seed);
        for _ in 0..ticks {
            s.step(&mut w, 1.0, None); // conservation audited every tick
        }
        w
    }

    #[test]
    fn crust_covers_most_of_the_surface() {
        // The reframe: crust freezes over the whole cooled surface, not just at plate
        // boundaries. Most cells should carry crust once the mantle has cooled below
        // the solidus — a planet with a seafloor, not bare mantle with painted seams.
        let w = run(6, 7, 260);
        let covered = w.columns.iter().filter(|c| !c.layers.is_empty()).count();
        let frac = covered as f64 / w.columns.len() as f64;
        assert!(frac > 0.6, "crust should cover most of the surface, got {:.0}%", frac * 100.0);
    }

    #[test]
    fn crust_grows_from_the_mantle_conserved() {
        // Crust is an OUTPUT: none at t=0, some after the sim runs, and every gram
        // came from the mantle (the audit inside step() proves conservation).
        let w = run(6, 7, 200);
        let crust: f64 = w.columns.iter().map(|c| c.mass_kg()).sum();
        assert!(crust > 0.0, "crust grew from mantle melt");
        assert!(crust < 0.05 * w.budget.total(), "crust is a thin skin, not the planet");
    }

    #[test]
    fn hypsometry_is_bimodal() {
        // THE M2 gate: two populations of crust — dense low-lying oceanic and light
        // high-standing continental — at clearly separated elevations. Nobody set
        // the heights; they come from composition (density) through Airy isostasy.
        let w = run(6, 5, 240);
        let mut ocean = Vec::new();
        let mut cont = Vec::new();
        for col in &w.columns {
            match crust_kind(col) {
                CrustKind::Oceanic => ocean.push(elevation_m(col)),
                CrustKind::Continental => cont.push(elevation_m(col)),
                CrustKind::Undifferentiated => {}
            }
        }
        assert!(!ocean.is_empty(), "some ocean floor formed");
        assert!(!cont.is_empty(), "some continents formed");
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (om, cm) = (mean(&ocean), mean(&cont));
        assert!(cm > om, "continents ride higher than ocean floor ({cm:.0} m vs {om:.0} m)");
        // A real gap, not two overlapping blurs — the bimodality.
        assert!(cm > om * 1.5, "the two modes are clearly separated");
    }

    #[test]
    fn oceanic_crust_is_denser_than_continental() {
        // The mechanism behind the gate: read densities off the two crust types.
        use crate::column::density_kg_m3;
        let w = run(6, 9, 200);
        let mut od = Vec::new();
        let mut cd = Vec::new();
        for col in &w.columns {
            if let Some(top) = col.layers.last() {
                match crust_kind(col) {
                    CrustKind::Oceanic => od.push(density_kg_m3(top)),
                    CrustKind::Continental => cd.push(density_kg_m3(top)),
                    _ => {}
                }
            }
        }
        if !od.is_empty() && !cd.is_empty() {
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
            assert!(mean(&od) > mean(&cd), "oceanic (mafic) crust is denser than continental (felsic)");
        }
    }

    #[test]
    fn crust_does_not_track_the_shard_seams() {
        // Objective grid-ghost metric. A shard "seam" cell borders a different icosa
        // face; those are the geometrically-irregular cells. If the physics is
        // topology-blind, continents form no more often on seam cells than off them,
        // so p_seam / p_interior ≈ 1. A raw-sum operator imprints the seams → ratio ≫ 1.
        let measure = |freq: u32| {
            let w = run(freq, 5, 240);
            let shard = &w.grid.shard;
            let neigh = &w.grid.neighbors;
            let (mut seam_cont, mut seam_n, mut int_cont, mut int_n) = (0usize, 0usize, 0usize, 0usize);
            for (i, col) in w.columns.iter().enumerate() {
                let cont = (crust_kind(col) == CrustKind::Continental) as usize;
                if neigh[i].iter().any(|&j| shard[j as usize] != shard[i]) {
                    seam_n += 1;
                    seam_cont += cont;
                } else {
                    int_n += 1;
                    int_cont += cont;
                }
            }
            let p_seam = seam_cont as f64 / seam_n.max(1) as f64;
            let p_int = int_cont as f64 / int_n.max(1) as f64;
            let ratio = p_seam / p_int.max(1e-9);
            eprintln!("freq {freq}: seam-continental {p_seam:.3} vs interior {p_int:.3} → ratio {ratio:.2}");
            ratio
        };
        let r6 = measure(6);
        let r24 = measure(24);
        let r48 = measure(48);
        // Regression guard at the levels the velocity + divergence least-squares
        // operators reach today (pre-fix freq48 was 2.46). freq24 is now neutral;
        // freq48 still carries residual imprinting that GROWS with resolution — its
        // source is the non-equal-area grid (ISEA equal-area / M-1, still pending),
        // NOT the operators. Target once ISEA lands: < 1.1 at every resolution. These
        // bounds catch a regression; they are NOT an acceptance of the freq48 residual.
        assert!(r6 < 1.10, "freq6 shard imprinting regressed: {r6:.2}");
        assert!(r24 < 1.15, "freq24 shard imprinting regressed: {r24:.2}");
        assert!(r48 < 1.70, "freq48 shard imprinting regressed: {r48:.2}");
    }
}
