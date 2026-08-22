//! [`MantleField`] — the per-cell interior (spec §5.1), the substrate the M1
//! stages act on.
//!
//! At M0 the whole budget sat in one global `mantle` [`Composition`]. That cannot
//! convect, cannot heat locally, cannot differentiate cell-by-cell — so M1
//! distributes the mantle across the 92,162 lateral cells. Each cell carries its
//! element mass, a temperature, a convection velocity, and a differentiation
//! progress.
//!
//! **Representation.** A dense struct-of-arrays keyed by a compact element index
//! (the ~28 budget elements), not 92k [`Composition`] `BTreeMap`s — the correct
//! technique for a field ticked repeatedly (memory `less-code-every-calculation-counts`):
//! `[f64; k]` per cell is ~20 MB and cache-friendly; 92k B-trees would be brute
//! force. A cached per-element `totals` makes the conservation audit O(k), not
//! O(cells·k).
//!
//! **Conservation.** Mass moves only through [`add`](MantleField::add) /
//! [`remove`](MantleField::remove) (the same clamp-and-report contract as
//! [`Composition`](flicker_worldstate::Composition)); `totals` is updated in
//! lockstep so `element_mass` is exact. At t=0 the field holds the whole budget:
//! each element's cached total is set to *exactly* the accreted mass (and the audit
//! reads that authoritative total); the per-cell masses sum to it up to float
//! rounding.

use flicker_materials::ElementId;
use flicker_worldgrid::Sphere;
use glam::Vec3;

use crate::budget::Budget;
use crate::stage::StageRng;

/// Accretional magma-ocean temperature at t=0, K — the undifferentiated hot ball.
pub const MAGMA_OCEAN_K: f64 = 4000.0;

/// Amplitude of the coherent thermal perturbation that seeds convection, K. Small
/// relative to [`MAGMA_OCEAN_K`]: the ball is hot everywhere, gently lumpy.
const THERMAL_PERTURB_K: f64 = 350.0;

/// Random low-frequency spherical waves summed into the initial thermal field —
/// **coherent** plume-scale blobs, never per-cell white noise (which would read as
/// the very noise field §6.1 warns against, before a tick even runs).
const THERMAL_WAVES: usize = 6;

/// The per-cell interior state. Lateral cells only at M1 — the three radial shells
/// of §5.1 collapse to one well-mixed mantle column per cell (radial structure is
/// a later refinement; the M1 output — core/mantle separation — does not need it).
pub struct MantleField {
    n_cells: usize,
    /// Compact index → atomic number (the budget's elements).
    elems: Vec<ElementId>,
    /// Atomic number → compact index; `-1` for absent (non-budget) elements.
    index: [i16; 256],
    /// Dense per-cell mass: `mass[cell * k + ei]` kg.
    mass: Vec<f64>,
    /// Cached Σ over cells per compact index — O(1) `element_mass` for the audit.
    totals: Vec<f64>,
    /// Per-cell temperature, K — **stored, integrated** by the thermal stage.
    pub temp_k: Vec<f64>,
    /// Per-cell tangential convection velocity — re-derived each tick from the
    /// temperature field, kept for the same tick's advection + plate traction.
    pub velocity: Vec<Vec3>,
    /// Per-cell core-formation progress `d ∈ [0,1]` — **integrated** state (like
    /// `relief_m`), how far core formation has drained this cell's metal.
    pub differentiation: Vec<f64>,
}

impl MantleField {
    /// Seed a homogeneous hot ball on `grid`: every cell gets `budget/N` of each
    /// element (the float-division remainder folded into the last cell), a
    /// magma-ocean temperature with a coherent perturbation, zero velocity, zero
    /// differentiation. Each element's **cached total** is set to the accreted mass
    /// exactly — the audit reads that — so conservation is exact at t=0; the
    /// per-cell masses sum to it to within float rounding.
    pub fn seed(budget: &Budget, grid: &Sphere, seed: u64) -> Self {
        let n_cells = grid.len();
        let elems: Vec<ElementId> = budget.iter().map(|(e, _)| e).collect();
        let k = elems.len();

        let mut index = [-1i16; 256];
        for (ei, &e) in elems.iter().enumerate() {
            index[e as usize] = ei as i16;
        }

        let mut mass = vec![0.0f64; n_cells * k];
        let mut totals = vec![0.0f64; k];
        for (ei, &e) in elems.iter().enumerate() {
            let total = budget.accreted(e);
            let per = total / n_cells as f64;
            for cell in 0..n_cells - 1 {
                mass[cell * k + ei] = per;
            }
            // The last cell absorbs the float-division residue.
            mass[(n_cells - 1) * k + ei] = total - per * (n_cells - 1) as f64;
            // `totals` is authoritative and set to the exact budget, so the seed
            // audit balances to the last ULP.
            totals[ei] = total;
        }

        Self {
            n_cells,
            elems,
            index,
            mass,
            totals,
            temp_k: initial_thermal_field(grid, seed),
            velocity: vec![Vec3::ZERO; n_cells],
            differentiation: vec![0.0; n_cells],
        }
    }

    /// Number of lateral cells.
    pub fn n_cells(&self) -> usize {
        self.n_cells
    }

    /// The elements this field tracks (the budget's, in atomic-number order).
    pub fn elements(&self) -> &[ElementId] {
        &self.elems
    }

    /// Compact index of `element`, if it is a tracked (budget) element.
    #[inline]
    fn ei(&self, element: ElementId) -> Option<usize> {
        let ix = self.index[element as usize];
        (ix >= 0).then_some(ix as usize)
    }

    /// Mass of `element` in `cell`, kg (0 if the element is not tracked).
    pub fn mass(&self, cell: usize, element: ElementId) -> f64 {
        match self.ei(element) {
            Some(ei) => self.mass[cell * self.elems.len() + ei],
            None => 0.0,
        }
    }

    /// Deposit `amount` of `element` into `cell` (non-positive is a no-op). Only
    /// tracked (budget) elements can be held — nothing else is ever added to the
    /// mantle at M1.
    pub fn add(&mut self, cell: usize, element: ElementId, amount: f64) {
        if amount <= 0.0 {
            return;
        }
        let Some(ei) = self.ei(element) else {
            debug_assert!(false, "mantle add of non-budget element {element}");
            return;
        };
        self.mass[cell * self.elems.len() + ei] += amount;
        self.totals[ei] += amount;
    }

    /// Remove up to `amount` of `element` from `cell`, returning what was actually
    /// taken — the conservation-safe transaction primitive (never yields more than
    /// present). Updates the cached total in lockstep.
    pub fn remove(&mut self, cell: usize, element: ElementId, amount: f64) -> f64 {
        if amount <= 0.0 {
            return 0.0;
        }
        let Some(ei) = self.ei(element) else {
            return 0.0;
        };
        let idx = cell * self.elems.len() + ei;
        let taken = amount.min(self.mass[idx]);
        self.mass[idx] -= taken;
        self.totals[ei] -= taken;
        taken
    }

    /// Total mass of `element` across every cell (O(1) — cached). The audit's
    /// per-element mantle term.
    pub fn element_mass(&self, element: ElementId) -> f64 {
        match self.ei(element) {
            Some(ei) => self.totals[ei],
            None => 0.0,
        }
    }

    /// Total mantle mass across all cells and elements.
    pub fn total_mass(&self) -> f64 {
        self.totals.iter().sum()
    }

    /// Total mass in one cell (all elements) — the viewer's per-cell fraction reads.
    pub fn cell_mass(&self, cell: usize) -> f64 {
        let k = self.elems.len();
        self.mass[cell * k..cell * k + k].iter().sum()
    }
}

/// A deterministic coherent thermal field: a handful of random low-frequency
/// spherical waves. The wave set is a pure function of `seed`, so the seven Home
/// worlds' initial convection patterns differ per seed (§3.5) yet each run is
/// reproducible (§11).
fn initial_thermal_field(grid: &Sphere, seed: u64) -> Vec<f64> {
    let mut rng = StageRng::new(seed ^ 0x00DE_CADE_C0FF_EE00);
    let waves: Vec<([f64; 3], f64, f64)> = (0..THERMAL_WAVES)
        .map(|_| {
            let dir = random_unit(&mut rng);
            let freq = 1.5 + 3.0 * rng.next_f64(); // low spatial frequency
            let phase = rng.next_f64() * std::f64::consts::TAU;
            (dir, freq, phase)
        })
        .collect();

    grid.dirs
        .iter()
        .map(|p| {
            let p = [p.x as f64, p.y as f64, p.z as f64];
            let mut s = 0.0;
            for (dir, freq, phase) in &waves {
                let dot = p[0] * dir[0] + p[1] * dir[1] + p[2] * dir[2];
                s += (dot * freq + phase).sin();
            }
            MAGMA_OCEAN_K + THERMAL_PERTURB_K * (s / THERMAL_WAVES as f64)
        })
        .collect()
}

/// A uniformly-random unit vector (Marsaglia's z / azimuth method).
fn random_unit(rng: &mut StageRng) -> [f64; 3] {
    let z = 2.0 * rng.next_f64() - 1.0;
    let t = std::f64::consts::TAU * rng.next_f64();
    let r = (1.0 - z * z).max(0.0).sqrt();
    [r * t.cos(), r * t.sin(), z]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    fn budget() -> Budget {
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        Budget::from_dir(&dir, &t).expect("budget")
    }

    #[test]
    fn seed_holds_the_whole_budget_exactly() {
        let b = budget();
        let grid = icosphere(6);
        let m = MantleField::seed(&b, &grid, 1);
        // Every element's mantle total equals its accreted mass to the last ULP —
        // the field IS the undifferentiated ball at t=0.
        for (e, accreted) in b.iter() {
            assert_eq!(
                m.element_mass(e),
                accreted,
                "element {e} not conserved at seed"
            );
        }
        assert_eq!(m.n_cells(), grid.len());
    }

    #[test]
    fn remove_is_clamped_and_tracked() {
        let b = budget();
        let grid = icosphere(4);
        let mut m = MantleField::seed(&b, &grid, 1);
        let fe_before = m.element_mass(26);
        let in_cell0 = m.mass(0, 26);
        // Over-remove from one cell: takes only what the cell held, and the total
        // drops by exactly that.
        let taken = m.remove(0, 26, in_cell0 * 10.0);
        assert_eq!(taken, in_cell0);
        assert_eq!(m.mass(0, 26), 0.0);
        assert_eq!(m.element_mass(26), fe_before - in_cell0);
    }

    #[test]
    fn initial_field_is_hot_and_coherent() {
        let grid = icosphere(8);
        let field = initial_thermal_field(&grid, 7);
        // Hot everywhere (a magma ocean), lumpy but not wild.
        for &t in &field {
            assert!(t > MAGMA_OCEAN_K - 2.0 * THERMAL_PERTURB_K, "too cold: {t}");
            assert!(t < MAGMA_OCEAN_K + 2.0 * THERMAL_PERTURB_K, "too hot: {t}");
        }
        // Different seeds → different fields (per-seed Home worlds).
        let other = initial_thermal_field(&grid, 8);
        assert!(
            field.iter().zip(&other).any(|(a, b)| a != b),
            "seed had no effect"
        );
    }
}
