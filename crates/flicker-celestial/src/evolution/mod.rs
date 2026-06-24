//! **Evolution** — iterating a body's [`HexWorld`] forward in time by **moving conserved material
//! between cells**. This is the *iteration engine* the one-shot epoch pipeline always lacked: the
//! world-gen epochs, run **per body and stepped** on a mega-year clock, with the surrounding system
//! as their missing context (the star as a heat source, orbital insolation, event onsets).
//!
//! The grid is the truth and the mesh a disposable read-out: a [`step`](BodyEvolution::step)
//! **physically transports** material — it never paints. [`step_world`] is the single entry point,
//! **dispatched by composition** (one framework, never parallel code paths): a gaseous world runs
//! zonal differential-rotation [`advect_zonal`]; a solid world will run the Epoch-2 density sort
//! (next slice). It is *pure*, so the celestial sim runs it off-thread (`flicker-worker`) on the
//! last completed grid while the display crossfades between completed states — never a hard snap.
//! Topology stays **outside** this crate: a step reads `dirs`/`neighbors` from [`StepCtx`], supplied
//! by the caller's icosphere (the epochs were always topology-agnostic).
//!
//! # Emergent stages, no terminations
//!
//! A body advances to the next [`Stage`] only when its **data physically supports it** — there are
//! deliberately **no per-type terminations**. A gas giant simply never satisfies the requirements
//! past stratification; a hot inner world never cools past its molten stage; an HZ rocky world keeps
//! qualifying all the way to life. The endpoint is an emergent property of composition + insolation,
//! never a coded stop. Because most bodies stall early, the deep, expensive stages only ever run on
//! the few worlds that earn them — so the same emergence that makes it realistic also bounds the
//! compute (see `docs/flicker-celestial-evolution-handoff.md`). (The advance *gate* is a later slice;
//! this one lands the first real transform.)

use glam::Vec3;

use crate::model::world::HexWorld;

mod advect;

pub use advect::advect_zonal;

/// What a step reads about the body besides its grid: the **topology** — each cell's unit-sphere
/// direction and its neighbours (variable length: 5 for the twelve pentagons, 6 for hexes) — and,
/// in later slices, the **environment** the celestial sim supplies (star luminosity, insolation,
/// event onsets). Topology is passed in rather than owned so `flicker-celestial` stays
/// topology-agnostic; the caller hands over its icosphere's `dirs`/`neighbors` (the same `EpochCtx`
/// shape the world-gen epochs consume). `dirs` and `neighbors` are parallel to the grid's cells.
pub struct StepCtx<'a> {
    /// Unit-sphere centre direction of each cell.
    pub dirs: &'a [Vec3],
    /// Adjacency per cell — the transport graph the advection moves material along.
    pub neighbors: &'a [Vec<u32>],
}

/// A body's evolution stage — the world-gen epochs, run per body and iterated on the MYR clock.
/// The numeric order is the sequence; advancing along it is gated by the data (see the module
/// docs), never by the body's type.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Epoch 1 — composition seeded from the aggregation bands (the materialised distribution).
    Aggregation,
    /// Epoch 2 — stratification: density sort (solids) / advective sorting (gas giants).
    Stratification,
    /// Epoch 3 — crust hardens, plates.
    Tectonics,
    /// Epoch 4 — hydrosphere + atmosphere.
    Hydrosphere,
    /// Epoch 5 — mineralization (ore veins).
    Mineralization,
    /// Epoch 6 — biosphere: life, with event onsets (innovations, impacts, extinctions).
    Biosphere,
}

impl Stage {
    /// The next stage in the sequence, or `None` past the last.
    pub fn next(self) -> Option<Stage> {
        use Stage::*;
        Some(match self {
            Aggregation => Stratification,
            Stratification => Tectonics,
            Tectonics => Hydrosphere,
            Hydrosphere => Mineralization,
            Mineralization => Biosphere,
            Biosphere => return None,
        })
    }
}

/// The iteration engine's per-body state: the evolving surface grid, the stage it has reached, and
/// its simulated age. Owned and stepped by the celestial sim — a worker computes the next state
/// (via [`step_world`]) while the display interpolates between completed ones, so the sim is never
/// blocked and the planet morphs smoothly rather than snapping.
#[derive(Clone, Debug)]
pub struct BodyEvolution {
    /// The body's surface material (conserved across every step).
    pub world: HexWorld,
    /// The furthest stage reached — emergent, gate-driven (see module docs).
    pub stage: Stage,
    /// Simulated age in millions of years.
    pub age_myr: f64,
}

impl BodyEvolution {
    /// A freshly-aggregated body (Epoch 1) at age zero.
    pub fn new(world: HexWorld) -> Self {
        Self { world, stage: Stage::Aggregation, age_myr: 0.0 }
    }

    /// Advance the body by `dt_myr`: transport its material one step ([`step_world`]) and age it.
    /// `world`'s total mass stays invariant by construction (every transform *moves* material,
    /// never creates it). The emergent gate that promotes `stage` is a future slice.
    pub fn step(&mut self, dt_myr: f64, ctx: &StepCtx) {
        self.world = step_world(&self.world, dt_myr, ctx);
        self.age_myr += dt_myr.max(0.0);
    }
}

/// Advance a grid one step, **dispatched by composition** — one framework, never parallel paths.
/// A gaseous world (liquefied air) runs zonal differential-rotation [`advect_zonal`]; a solid world
/// will run the Epoch-2 density sort (next slice) — for now it *holds*, which is the correct
/// emergent behaviour: a body simply doesn't run a transform its data doesn't call for. Pure and
/// conserved; this is the function the worker runs on the last completed grid to produce the next.
pub fn step_world(world: &HexWorld, dt_myr: f64, ctx: &StepCtx) -> HexWorld {
    if gaseous(world) {
        advect_zonal(world, dt_myr, ctx)
    } else {
        world.clone()
    }
}

/// Prism atomic numbers of the nebular gases — a world over half H+He by mass is liquefied air
/// (a gas giant), which evolves by advection rather than the solid density sort.
const H: flicker_materials::ElementId = 1;
const HE: flicker_materials::ElementId = 2;

/// Whether the grid is gas-dominated (H+He over half its total mass) — the dispatch test. An empty
/// grid is not gaseous (nothing to advect).
fn gaseous(world: &HexWorld) -> bool {
    let total = world.total();
    if total <= 0.0 {
        return false;
    }
    let gas: f64 = world.cells().iter().map(|c| c.amount(H) + c.amount(HE)).sum();
    gas > 0.5 * total
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldstate::Composition;

    #[test]
    fn stages_run_in_order_and_terminate_open() {
        assert_eq!(Stage::Aggregation.next(), Some(Stage::Stratification));
        assert_eq!(Stage::Biosphere.next(), None, "no stage past the last");
    }

    #[test]
    fn fresh_body_starts_aggregated_and_ages_conserving_mass() {
        let mut e = BodyEvolution::new(HexWorld::new(48, 7));
        assert_eq!(e.stage, Stage::Aggregation);
        assert_eq!(e.age_myr, 0.0);
        let before = e.world.total();
        let dirs = vec![Vec3::X; 7];
        let neighbors = vec![Vec::new(); 7];
        let ctx = StepCtx { dirs: &dirs, neighbors: &neighbors };
        e.step(50.0, &ctx);
        assert_eq!(e.age_myr, 50.0);
        assert_eq!(e.world.total(), before, "stepping never changes total mass");
    }

    #[test]
    fn dispatch_advects_gas_but_holds_solid() {
        // One ring cell flowing into the next, so any advection shows as a change at cell 0.
        let dirs = vec![Vec3::X, Vec3::new(0.0, 0.0, -1.0)];
        let neighbors = vec![vec![1u32], vec![0u32]];
        let ctx = StepCtx { dirs: &dirs, neighbors: &neighbors };

        // Gas world (H/He) → advects → cell 0 loses material to its neighbour.
        let gas = HexWorld::from_cells(1, vec![Composition::from_iter([(H, 80.0), (HE, 20.0)]), Composition::new()]);
        let stepped_gas = step_world(&gas, 1.0, &ctx);
        assert!(stepped_gas.cell(0).unwrap().total() < 100.0, "gas world transports material");
        assert_eq!(stepped_gas.total(), gas.total(), "and conserves it");

        // Solid world (O/Fe) → holds (density sort is the next slice) → unchanged.
        let solid = HexWorld::from_cells(1, vec![Composition::from_iter([(8, 80.0), (26, 20.0)]), Composition::new()]);
        assert_eq!(step_world(&solid, 1.0, &ctx), solid, "solid world holds for now");
    }
}
