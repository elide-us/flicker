//! **The tectonic conveyor** — the mechanism that makes plates *move things*.
//!
//! Every earlier attempt at this simulation could find plates and then did nothing
//! with them: coherent blobs of cells were identified, coloured, and left sitting
//! still. Nothing subducted, no range was pushed up, no ocean opened. This module
//! is the missing half — the part where a column of rock is **physically carried**
//! from one cell to the next, and where what happens at the seams happens because
//! two stacks arrived at the same place.
//!
//! # The four rules
//!
//! 1. **Rigidity is causal; plates are its consequence.** Two neighbouring cells
//!    move as one body when there is coherent lithosphere joining them *and* they
//!    are already moving alike. Bare magma transmits no stress, so a magma ocean
//!    has no plates — correctly, and without anybody saying so. Domains fall out of
//!    the union-find; nothing declares how many there are.
//!
//! 2. **A rigid body on a sphere turns about an axis.** Per domain, the mantle-drag
//!    velocities of its members are fitted by least squares to a single **Euler
//!    pole** `ω`, and every member then moves at `ω × r`. This one step is what
//!    turns per-cell noise into rafts: interiors go quiet, and deformation
//!    concentrates at the edges — which is where the geology is.
//!
//! 3. **Everything moves, and the world stays full.** Each column integrates its
//!    displacement; when it has been carried one cell spacing it **steps** to the
//!    neighbour it is heading toward. After every step each cell holds exactly one
//!    column — a checked invariant. Two stacks arriving at one cell is a
//!    **collision**; a cell nobody arrives at is a **vacancy**. Both are resolved
//!    by what the material is, never by what the map should look like.
//!
//! 4. **Mass is moved, never made.** A step is a whole-stack relocation — the
//!    ledgers travel intact, so relocation is exactly conservative by construction.
//!    Subducted rock is credited to the mantle it sank into; new crust is debited
//!    from the mantle it melted out of.
//!
//! # What emerges (nothing here asserts any of it)
//!
//! A dense stack that loses a collision sinks — **subduction** — and its volatiles
//! and incompatible elements go back into the melt, which is the distillation
//! column ore bodies come out of. A buoyant stack cannot sink, so when two of them
//! meet the loser is thrust onto the winner and the stack **thickens** — and
//! absolute isostasy then *reads out* a mountain range. Nowhere does a rule say
//! "continents collide into mountains". Where nobody arrives, mantle decompresses
//! and freezes new thin crust — a **spreading ridge**, riding low because it is
//! dense. Rifts, basins and ranges are all the same three rules meeting different
//! material.

use glam::{Mat3, Vec3};

use flicker_materials::ElementId;

use crate::column::{
    density_kg_m3, elevation_m, Column, FormationProcess, Layer, SUBDUCTABLE_DENSITY,
};
use crate::observer::segment_where;
use crate::planet::{sea_level_m, World};
use crate::stage::{Stage, StageRng};

/// Lithosphere breaks where the **strain** across a contact runs above this
/// multiple of the planet's mean strain — so boundaries form where the flow is
/// most sheared, and the interiors that are left ride as bodies. Higher → fewer,
/// larger plates; the count itself is emergent and nothing fixes it.
///
/// A *relative* yield, because there is no rock-strength model yet: real
/// lithosphere fails at an absolute stress set by its material and temperature,
/// and when the rock tier lands this compares against that instead. Using the
/// velocity difference alone (rather than strain) is what collapsed the surface
/// into ONE global plate — a smooth field always has small neighbour differences,
/// however sheared it is over a distance.
pub const DEFAULT_YIELD_STRAIN: f32 = 1.0;

/// A coupled domain smaller than this is not a plate — it is loose lithosphere,
/// carried by the flow rather than riding as a body.
const MIN_PLATE_CELLS: usize = 8;

/// Regulariser on the Euler-pole normal equations, so a domain whose cells happen
/// to be nearly collinear still yields a finite axis instead of exploding.
const POLE_REGULARISER: f32 = 1.0e-3;

/// How much of a sunken slab comes back up as arc melt, before refining. The rest
/// stays in the mantle.
///
/// Drawn from **the slab**, not from fresh mantle — which is what keeps the crust
/// finite. An arc fed by the mantle is a source with no sink, and the crust grows
/// without bound (measured: ocean floor 37 km thick and still climbing, with the
/// hypsometry washed flat because thickness swamped density). An arc fed by the
/// slab is *recycling*: a stack goes down, a refined fraction of it returns, and
/// the difference stays below. It is also the distillation column ore bodies come
/// out of — each pass down and back concentrates what partitions into the melt.
pub const DEFAULT_ARC_RETURN: f64 = 0.3;

/// **Flux melting.** Water dissolved in rock lowers its melting point, so a slab
/// that went down wet yields far more melt than a dry one at the same depth and
/// temperature — this is why Earth's arcs sit above *oceanic* trenches and why
/// they are the wettest, most explosive volcanoes on the planet.
///
/// A stack that stood under the sea is a wet stack, so this multiplies the return
/// of any slab that subducted from below sea level. The chain runs all the way
/// back to the boundary input: more water delivered → a higher solved sea level →
/// more of the world submerged → wetter slabs → more arc melt and more mountains.
/// Nobody connects water to volcanism; the sea level solve already did.
const WET_FLUX_GAIN: f64 = 2.0;

/// Fraction of the mantle cell drawn up where a vacancy opens — decompression melt
/// at a spreading centre, mafic and thin.
const RIDGE_MELT_FRACTION: f64 = 0.0004;

/// The temperature a subducting stack carries down with it, K — surface rock,
/// which is what a slab is made of. See the cooling term in [`collide`].
const SLAB_SURFACE_K: f64 = 300.0;

/// **The two ends of the circuit, counted.** Mass the seams swallowed and mass
/// the seams welled back up, kg, since the last [`take_seam_flux`].
///
/// OBSERVATION ONLY — nothing reads these to decide anything, and removing them
/// changes no outcome. They exist because "we sink material, and what we sink
/// has to come back" is an **aggregate** claim about the whole planet, and the
/// only way to know whether the circuit closes is to count both ends. It is not
/// per-cell provenance and must never become that: the planet is the well.
static SUNK_BITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WELLED_BITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn add_flux(counter: &std::sync::atomic::AtomicU64, kg: f64) {
    use std::sync::atomic::Ordering::Relaxed;
    let _ = counter.fetch_update(Relaxed, Relaxed, |bits| {
        Some((f64::from_bits(bits) + kg).to_bits())
    });
}

/// Read and reset both counters: `(subducted_kg, welled_kg)`.
pub fn take_seam_flux() -> (f64, f64) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        f64::from_bits(SUNK_BITS.swap(0, Relaxed)),
        f64::from_bits(WELLED_BITS.swap(0, Relaxed)),
    )
}

/// How far a cell's temperature falls per unit of its own mass drawn off as
/// melt, K. Melting takes the heat of fusion out of what stays behind, so a
/// vent that keeps erupting cools its own source and the hot spot has to move.
/// Sized to bite measurably into a plume's anomaly over an eruptive era
/// without quenching it inside one tick.
const MELT_LATENT_K: f64 = 4.0e4;

/// The angular spacing between a cell and its neighbours, in unit-sphere radians —
/// one "step" for the conveyor. Equal-area makes this near-uniform, which is what
/// lets one step mean one distance anywhere on the planet.
///
/// Public because the motion READ answers to it too: a bench arrow that showed
/// progress toward some other distance would be pointing at a step the conveyor
/// is not about to take.
pub fn cell_spacing(world: &World, cell: usize) -> f32 {
    let pi = world.grid.dirs[cell];
    let nb = &world.grid.neighbors[cell];
    if nb.is_empty() {
        return f32::MAX;
    }
    nb.iter()
        .map(|&j| (world.grid.dirs[j as usize] - pi).length())
        .sum::<f32>()
        / nb.len() as f32
}

/// Fit a single rotation axis to a domain's velocities by least squares.
///
/// A rigid motion on the sphere is `v = ω × r`, so the residual to minimise is
/// `Σ |ω × rᵢ − vᵢ|²`. Differentiating gives the normal equations
/// `Σ(I − rᵢrᵢᵀ) ω = Σ(rᵢ × vᵢ)` — a 3×3 solve, and the whole of what makes a
/// plate a body rather than a bag of cells.
fn euler_pole(members: impl Iterator<Item = (Vec3, Vec3)>) -> Vec3 {
    let (mut m, mut b) = (Mat3::ZERO, Vec3::ZERO);
    let mut n = 0u32;
    for (r, v) in members {
        m += Mat3::IDENTITY - Mat3::from_cols(r * r.x, r * r.y, r * r.z);
        b += r.cross(v);
        n += 1;
    }
    if n == 0 {
        return Vec3::ZERO;
    }
    let m = m + Mat3::IDENTITY * (POLE_REGULARISER * n as f32);
    let solved = m.inverse() * b;
    if solved.is_finite() {
        solved
    } else {
        Vec3::ZERO
    }
}

/// **Conveyor** — find the bodies, give each one an axis, carry them, and let them
/// step.
///
/// Motion and the step are **one transaction**, because a plate is a body: the
/// coupling that decides who belongs to it is the same coupling that decides who
/// steps with it, and nothing may observe the world between the two.
///
/// The step is taken **by the plate, not by the cell**. That is the rigidity
/// assumption made discrete, and it is load-bearing: letting each column step the
/// moment its own accumulator filled tore rafts apart — cells drifted across a
/// plate's interior at slightly different rates, so they stepped at slightly
/// different times, and ~80% of steps opened a hole or a pile-up *inside* a plate
/// that was supposed to be moving as one piece. Stepping the body instead leaves
/// interiors quiet: a cell steps into ground its own plate-mate has just left, and
/// the only collisions are at a leading edge and the only vacancies at a trailing
/// one. Which is the picture — the geology happens at the edges.
pub struct Conveyor {
    /// Strain, as a multiple of the planet's mean, at which lithosphere yields.
    pub yield_strain: f32,
    /// How much of a sunken slab returns as arc melt.
    pub arc_return: f64,
}

impl Default for Conveyor {
    /// The physics as written.
    fn default() -> Self {
        Self { yield_strain: DEFAULT_YIELD_STRAIN, arc_return: DEFAULT_ARC_RETURN }
    }
}

impl Stage for Conveyor {
    fn name(&self) -> &'static str {
        "Conveyor"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let n = world.columns.len();
        if n == 0 {
            return;
        }
        let (labels, members) = self.bodies(world);
        self.carry(world, &labels, &members, dt_myr);
        self.step(world, &labels, &members);
    }
}

impl Conveyor {
    /// Segment the surface into bodies: lithosphere must join two cells (bare magma
    /// transmits no stress, so a magma ocean has no plates — correctly, and without
    /// anybody saying so), and the contact between them must not have yielded.
    /// Returns the per-cell label and the members of each body, bucketed once.
    fn bodies(&self, world: &World) -> (Vec<u32>, Vec<Vec<usize>>) {
        let n = world.columns.len();
        let vel = &world.mantle.velocity;
        let has_crust: Vec<bool> = world.columns.iter().map(|c| !c.layers.is_empty()).collect();

        let strain = |i: usize, j: usize| -> f32 {
            let gap = (world.grid.dirs[i] - world.grid.dirs[j]).length().max(1e-9);
            (vel[i] - vel[j]).length() / gap
        };
        let (mut sum, mut count) = (0.0f32, 0usize);
        for i in 0..n {
            for &j in &world.grid.neighbors[i] {
                let j = j as usize;
                if j > i && has_crust[i] && has_crust[j] {
                    sum += strain(i, j);
                    count += 1;
                }
            }
        }
        // +ε so a still world is one body rather than n singletons.
        let yield_at = self.yield_strain * sum / count.max(1) as f32 + 1e-9;

        let (labels, n_plates, _) = segment_where(world, MIN_PLATE_CELLS, &|i, j| {
            has_crust[i] && has_crust[j] && strain(i, j) < yield_at
        });
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); n_plates + 1];
        for (cell, &label) in labels.iter().enumerate() {
            members[label as usize].push(cell);
        }
        (labels, members)
    }

    /// Fit each body one axis and carry its columns at `ω × r`; loose lithosphere
    /// (label `0`) just drifts with the flow it sits in. Every column banks the
    /// displacement until its body has earned a step.
    fn carry(&self, world: &mut World, labels: &[u32], members: &[Vec<usize>], dt_myr: f64) {
        let vel = world.mantle.velocity.clone();
        let poles: Vec<Vec3> = members
            .iter()
            .enumerate()
            .map(|(plate, cells)| {
                if plate == 0 {
                    Vec3::ZERO
                } else {
                    euler_pole(cells.iter().map(|&i| (world.grid.dirs[i], vel[i])))
                }
            })
            .collect();

        let dt = dt_myr as f32;
        for cell in 0..world.columns.len() {
            let r = world.grid.dirs[cell];
            let v = match labels[cell] {
                0 => vel[cell],
                p => poles[p as usize].cross(r),
            };
            // Keep the carried displacement in the cell's tangent plane; radial
            // drift is meaningless on a sphere.
            let d = world.columns[cell].accum_disp + v * dt;
            world.columns[cell].accum_disp = d - r * d.dot(r);
        }
    }

    /// Spend the carried distance: any body that has been carried a full cell
    /// spacing moves, **all of it at once**, each column into the neighbour that
    /// lies along the body's heading. Then settle the world so every cell holds
    /// exactly one column again — which is what turns a leading edge into a
    /// collision and a trailing edge into open ground.
    fn step(&self, world: &mut World, labels: &[u32], members: &[Vec<usize>]) {
        let n = world.columns.len();
        let mut destination: Vec<usize> = (0..n).collect();
        // Where the sea stands, read BEFORE anything is lifted — a collision needs
        // to know whether the stack going down was standing under water, because
        // a wet slab melts far more readily than a dry one (`WET_FLUX_GAIN`).
        let (sea, area) = (sea_level_m(world), world.cell_area_m2());

        // A body steps when the mean DISTANCE its members have been carried
        // reaches a cell spacing — the magnitudes averaged as scalars, never the
        // vectors: a rotation's displacement vectors cancel around the sphere
        // while every member has genuinely travelled, so the vector mean starved
        // large plates of steps and handed the survivors a meaningless heading.
        let mut moving: Vec<bool> = vec![false; members.len()];
        for (plate, cells) in members.iter().enumerate().skip(1) {
            if cells.is_empty() {
                continue;
            }
            let carried: f32 = cells
                .iter()
                .map(|&c| world.columns[c].accum_disp.length())
                .sum::<f32>()
                / cells.len() as f32;
            let spacing: f32 =
                cells.iter().map(|&c| cell_spacing(world, c)).sum::<f32>() / cells.len() as f32;
            moving[plate] = carried >= spacing;
        }

        for cell in 0..n {
            let d = world.columns[cell].accum_disp;
            let spacing = cell_spacing(world, cell);
            let due = match labels[cell] {
                0 => d.length() >= spacing,
                // A member of a stepping body moves with it — along its OWN
                // carried vector. `ω × r` is a local direction and a local
                // distance: one shared heading for a whole body is what pinched
                // section edges into each other at the lattice twists around the
                // pentagons, and near the body's own pole the rotation honestly
                // carries a member nowhere, so it honestly stays.
                p => moving[p as usize] && d.length() >= 0.5 * spacing,
            };
            if !due {
                continue;
            }
            let heading = d.normalize_or_zero();
            if heading == Vec3::ZERO {
                continue;
            }
            let here = world.grid.dirs[cell];
            let Some(to) = world.grid.neighbors[cell]
                .iter()
                .map(|&j| j as usize)
                .max_by(|&a, &b| {
                    let score = |c: usize| (world.grid.dirs[c] - here).normalize_or_zero().dot(heading);
                    score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)
                })
            else {
                continue;
            };
            destination[cell] = to;
            // Spend exactly the step taken and keep the remainder, so a body that
            // is moving fast keeps its momentum instead of being reset each time.
            let there = world.grid.dirs[to];
            let left = world.columns[cell].accum_disp - (there - here);
            world.columns[cell].accum_disp = left - there * left.dot(there);
        }

        const NONE: u32 = u32::MAX;
        // ── Same-body dedup: lattice noise is not geology. ──
        // Around the twelve pentagons and the shard seams the grid's orientation
        // twists, so ONE physical heading quantizes onto DIFFERENT edges for
        // adjacent members of the same rigid body — two cells of one plate end up
        // contending for one cell. Feeding that pair to `collide` subducts a
        // body's own rock along grid lines (the seam "overwrite" seen in-window:
        // values vanishing into the mantle wherever sections pinched). Instead
        // the first claimant keeps the step — any deterministic choice serves,
        // this is noise — and the other UN-STEPS: destination back home, stride
        // refunded into its accumulator for a later tick. Stationary members
        // claim their own ground first, so a raft never subducts into a member
        // the rotation left in place. Runs to fixpoint (each pass only un-steps,
        // so it terminates); label 0 is exempt — two loose slivers meeting is
        // genuine geology and keeps its collision.
        loop {
            let mut claimed: Vec<u32> = vec![NONE; n];
            for from in 0..n {
                if labels[from] != 0 && destination[from] == from {
                    claimed[from] = from as u32;
                }
            }
            let mut changed = false;
            for from in 0..n {
                let to = destination[from];
                if to == from || labels[from] == 0 {
                    continue;
                }
                let c = claimed[to];
                if c == NONE {
                    claimed[to] = from as u32;
                } else if labels[c as usize] == labels[from] {
                    destination[from] = from;
                    let here = world.grid.dirs[from];
                    let there = world.grid.dirs[to];
                    let d = world.columns[from].accum_disp + (there - here);
                    world.columns[from].accum_disp = d - here * d.dot(here);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // ── The buoyant standoff: LAND DOES NOT RIDE OVER LAND. ──
        //
        // Two stacks that both refuse to sink used to be settled by `collide`:
        // one took the ground and wore the other, and the ground the loser came
        // from — nobody left to claim it — froze a fresh mafic floor. Mass was
        // conserved and AREA was not. Every such meeting turned two land cells
        // into one taller land cell plus one cell of new sea floor, so
        // continental crust could only ever concentrate: a one-hex spike, a
        // ring of young floor beside it that ages, densifies and founders (the
        // black hexes), belts that stay one hex wide, and plates that eat their
        // own leading edge instead of welding to what they meet.
        //
        // What a real continent does when it meets something it cannot push
        // under is STOP. The boundary locks, and the shortening goes into
        // crumpling instead of into anybody's mantle. So the step is REFUSED —
        // same un-step idiom as the dedup above, destination home and the
        // stride refunded — and both cells keep their ground. Nothing is
        // subducted here, so nothing needs conserving: this only decides who
        // moves.
        //
        // Only against ground that is still going to BE there: a cell whose
        // occupant is leaving for somewhere else is free to step into. Two
        // kinds of ground hold — one that is staying put, and one that is
        // driving straight back at us, which is the head-on continental
        // collision and the whole reason this exists. A cycle of stacks
        // rotating through one another holds nobody up and is left alone.
        //
        // Runs to fixpoint because each pass only un-steps, which is also what
        // resolves chains: arresting one stack makes it stationary ground for
        // the stack behind it, so the shortening propagates back into the plate
        // exactly as it should.
        let mut arrested: Vec<(usize, usize)> = Vec::new();
        loop {
            let mut changed = false;
            for from in 0..n {
                let to = destination[from];
                if to == from || (destination[to] != to && destination[to] != from) {
                    continue;
                }
                if !unsubductable(&world.columns[from]) || !unsubductable(&world.columns[to]) {
                    continue;
                }
                destination[from] = from;
                let (here, there) = (world.grid.dirs[from], world.grid.dirs[to]);
                let d = world.columns[from].accum_disp + (there - here);
                world.columns[from].accum_disp = d - here * d.dot(here);
                arrested.push((from, to));
                changed = true;
            }
            if !changed {
                break;
            }
        }

        // ── Lift everything, then settle it. ──
        // Who arrives where, as an intrusive chain through two flat arrays rather
        // than a Vec per cell: this runs every tick over every cell on the planet,
        // and 92k heap allocations a tick is a cost with nothing to show for it.
        let (mut head, mut next) = (vec![NONE; n], vec![NONE; n]);
        for (from, &to) in destination.iter().enumerate() {
            next[from] = head[to];
            head[to] = from as u32;
        }
        let mut lifted: Vec<Option<Column>> = world
            .columns
            .iter_mut()
            .map(|c| Some(std::mem::replace(c, Column::empty(c.cell_id))))
            .collect();

        let mut here: Vec<Column> = Vec::with_capacity(4);
        let mut vacancies: Vec<usize> = Vec::new();
        let mut well = 0.0f64;
        for cell in 0..n {
            here.clear();
            let mut at = head[cell];
            while at != NONE {
                if let Some(col) = lifted[at as usize].take() {
                    here.push(col);
                }
                at = next[at as usize];
            }
            match here.len() {
                0 => vacancies.push(cell),
                1 => {
                    let mut col = here.pop().expect("one arrival");
                    col.cell_id = cell as u32;
                    world.columns[cell] = col;
                }
                _ => collide(
                    world,
                    cell,
                    std::mem::take(&mut here),
                    self.arc_return,
                    sea,
                    area,
                    &mut well,
                ),
            }
        }
        // **Count what the well took, then spend it at the fountains.** One
        // planet-wide number split evenly between the gaps that opened — no
        // cell knows where any of it came from, because the planet is the well.
        // Whatever the gaps cannot take stays in the mantle where it sank.
        let share = if vacancies.is_empty() {
            0.0
        } else {
            well.max(0.0) / vacancies.len() as f64
        };
        for cell in std::mem::take(&mut vacancies) {
            open_ground(world, cell, share);
        }

        // ── The crumple: shortening a locked boundary cannot spend on motion. ──
        //
        // The push does not stop because the step was refused. Convergence that
        // cannot move ground has to go somewhere, and where it goes is up: the
        // advancing stack's COVER is thrust over the block resisting it, one
        // sheet at a time, while its own basement stays where it is. That is
        // the shape of a real collision margin — a thickening belt on the
        // overridden side, and behind it a foreland that has been stripped of
        // its cover but is still standing, still land.
        //
        // **The basement never goes.** A stack down to its last bed thrusts
        // nothing, which is what keeps the trailing cell continental instead of
        // recreating the vacancy this whole mechanism exists to prevent — and it
        // is self-limiting for free: a belt strips its foreland's cover, then
        // stops, until sediment and arc melt build a new cover for it to take.
        //
        // Whole beds, through the same `pile_on` a collision uses, so every bed
        // keeps its elements, its minerals, its densification and its history —
        // a fractional split would drop the mineral ledger on the floor, since
        // `Layer::release` discards what it removes.
        // **Which way the sheet goes is the same question a collision asks**:
        // the lighter block rides, so the DENSER one is overridden and it is
        // the denser one's cover that is scraped off. Without that asymmetry a
        // head-on pair simply trades covers back and forth every tick and
        // builds nothing — thickening needs an upper plate and a lower one.
        // Ties break on cell id so a run stays deterministic.
        // **A thrust SPENDS the shortening it came from.** The arrest above
        // refunds the stride — the push did not go away just because the step
        // was refused — but a refund with nothing debiting it means the contact
        // re-earns its step on the very next tick without accumulating any new
        // convergence, and thrusts another whole bed. Every tick, for as long as
        // the boundary is locked, at a rate set by the TICK COUNT rather than by
        // how fast the plates are actually closing.
        //
        // Measured after the layer cake landed: slope p90 pegged exactly at
        // `REPOSE_SLOPE` across a tenth of the planet — mass wasting clamping
        // everywhere because relief was being built faster than anything could
        // relax it. Doubling the bed count (1.9 → 3.9) doubled what an unpaced
        // crumple had to move.
        //
        // Spending one cell spacing per sheet is what makes the rate physical:
        // convergence goes into shortening, and a plate closing at some speed
        // thrusts at that speed however the timestep is chopped. A contact that
        // is arrested but thrusts NOTHING — wrong side, or down to its last bed
        // — keeps its accumulation, which is also right: the push is still
        // there and still building.
        for (from, to) in arrested {
            let (mine, theirs) =
                (world.columns[from].mean_density(), world.columns[to].mean_density());
            let overridden = mine > theirs || (mine == theirs && from > to);
            if !overridden || world.columns[from].layers.len() < 2 {
                continue;
            }
            let spacing = cell_spacing(world, from);
            let sheet = world.columns[from].layers.pop().expect("checked: at least two beds");
            world.columns[to].pile_on(vec![sheet]);
            let d = world.columns[from].accum_disp;
            let carried = d.length();
            world.columns[from].accum_disp =
                if carried > spacing { d * ((carried - spacing) / carried) } else { Vec3::ZERO };
        }

        audit_occupancy(world, "Conveyor");
    }
}

/// **Can this bed stop a descending slab?** Igneous AND too buoyant to follow.
///
/// Sediment, peat and vein fill are weak, wet and metres thin; they ride down
/// with the basement they were lying on, exactly as the sediment on a real
/// subducting plate does (see the long note at the use site in [`collide`] for
/// what letting them arrest it cost).
///
/// **The one buoyancy predicate**, shared by the collision scan and the
/// standoff test in [`Conveyor::step`] — if those two could disagree about what
/// sinks, a stack could be refused a step for being unsinkable and then be sunk
/// on arrival.
fn bed_arrests(bed: &Layer) -> bool {
    !matches!(
        bed.formed_by,
        FormationProcess::Sediment | FormationProcess::Organic | FormationProcess::Hydrothermal
    ) && density_kg_m3(bed) <= SUBDUCTABLE_DENSITY
}

/// **Is this stack unsinkable?** Its lowest bed decides: the leading edge goes
/// under first, so if the basement itself is too buoyant to follow, nothing
/// below the surface can be pushed down and the stack cannot be disposed of at
/// all. An empty column is not unsinkable — bare mantle is overridden freely.
fn unsubductable(col: &Column) -> bool {
    col.layers.first().is_some_and(bed_arrests)
}

/// **A vacancy.** Nobody arrived, so the mantle beneath is uncovered and
/// decompresses: it melts and freezes a thin mafic crust. Dense, low-riding — read
/// by isostasy as ocean floor, which is what a spreading ridge leaves behind.
fn open_ground(world: &mut World, cell: usize, from_the_well_kg: f64) {
    let mut melt = draw_melt(world, cell, RIDGE_MELT_FRACTION, crate::crust::oceanic_affinity);
    // **The fountain runs on what the well took in.** Decompression melt alone
    // is blind to what the seams ate, which is why welling returned 8.7% of
    // subduction and the rest sat stranded under the trenches. This asks for
    // this gap's share of the planet's intake instead — an AGGREGATE number,
    // with no idea which cell any of it came from, drawn like all ridge melt
    // from the mantle right here. Decompression is the floor, so a world with
    // no subduction still spreads.
    let have: f64 = melt.iter().map(|&(_, m)| m).sum();
    if from_the_well_kg > have {
        let reachable: f64 = world
            .mantle
            .elements()
            .iter()
            .map(|&e| world.mantle.mass(cell, e) * crate::crust::oceanic_affinity(e))
            .sum();
        if reachable > 0.0 {
            let fraction = (from_the_well_kg - have) / reachable;
            melt.extend(draw_melt(world, cell, fraction, crate::crust::oceanic_affinity));
        }
    }
    add_flux(&WELLED_BITS, melt.iter().map(|&(_, m)| m).sum());
    let mut col = Column::empty(cell as u32);
    if !melt.is_empty() {
        col.deposit(FormationProcess::OceanicCrust, world.tick_myr, &melt);
    }
    world.columns[cell] = col;
}

/// **A collision.** More than one stack wants this ground, and only one can ride
/// it: the most buoyant. What becomes of the others is decided by what they are
/// made of, not by which one arrived.
///
/// The loser is pushed under leading-edge first, and sinks only as far as its own
/// rock allows: dense beds return to the mantle, and the first bed too buoyant to
/// follow stops the slab, so everything above it is thrust onto the winner instead.
/// Sea floor, dense all the way down, goes back entire. A stack carrying refined
/// rock keeps that rock, which accumulates on the overriding plate. Two buoyant
/// stacks meeting therefore lose nothing at all: the loser rides bodily onto the
/// winner and the pile **thickens** — and isostasy reads a mountain range off it
/// afterwards, without any rule mentioning mountains.
///
/// Where a slab does go down it drives melting above it, and that melt is refined:
/// this is where continental crust comes from, and it now happens **because**
/// something actually subducted.
fn collide(
    world: &mut World,
    cell: usize,
    mut contenders: Vec<Column>,
    arc_return: f64,
    sea_level: f64,
    area: f64,
    well: &mut f64,
) {
    // The lightest rides. Ties broken by cell id so a run stays deterministic.
    contenders.sort_by(|a, b| {
        a.mean_density()
            .partial_cmp(&b.mean_density())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cell_id.cmp(&b.cell_id))
    });
    let mut winner = contenders.remove(0);
    winner.cell_id = cell as u32;
    // What went down, already weighted by how much of it comes back up: a slab
    // that subducted from under the sea carries water down with it and melts far
    // more readily on the way (flux melting).
    let mut returning: Vec<(ElementId, f64)> = Vec::new();

    for mut loser in contenders {
        let wet = elevation_m(&loser, area) < sea_level;
        let ret = arc_return * if wet { WET_FLUX_GAIN } else { 1.0 };
        // Bottom-first, and only as far as the rock will go: the leading edge is
        // pushed under, and each bed sinks while it is dense enough to. The first
        // bed too light to follow stops the slab — it delaminates, and everything
        // above it is scraped onto the overriding plate.
        //
        // This one rule is why continents survive. Sea floor is dense all the way
        // down and goes back to the mantle entire; a stack carrying refined,
        // buoyant rock cannot take it down with it, so that rock accumulates on the
        // winner instead. Nothing anywhere says "continental crust does not
        // subduct" — it simply cannot.
        let beds = loser.take_all();
        let mut scraped = Vec::new();
        let mut sinking = true;
        let mut sank_kg = 0.0;
        for bed in beds {
            // **Only rock can stop a slab.** A bed arrests the descent when it
            // is igneous AND too buoyant to follow — a veneer never does.
            //
            // Sediment, peat and vein fill are weak, wet and metres thin; they
            // ride down with the basement they were lying on, exactly as the
            // sediment on a real subducting plate does. Letting them arrest it
            // was quietly the largest homogeniser in the model: the FIRST soft
            // film stopped the scan, so every collision scraped the loser's
            // whole mixed veneer onto the winner, every stack drifted toward
            // the same composition, and the whole world settled to one drowned
            // mean elevation (Aaron, 2026-08-06 — "the land just eventually
            // averages out"). It also broke the distillation loop: mud that
            // never goes down never comes back refined, so continents could
            // never purify into anything light enough to stand up.
            let arrests = bed_arrests(&bed);
            if sinking && !arrests {
                for (e, m) in bed.elements.iter() {
                    world.mantle.add(cell, e, m);
                    returning.push((e, m * ret));
                    *well += m;
                    sank_kg += m;
                }
            } else {
                sinking = false;
                scraped.push(bed);
            }
        }
        // **The slab takes its cold down with it.** Surface rock is hundreds of
        // degrees below the interior it sinks into, and mixing it in cools that
        // interior — the one place in this pipeline where moving mass also
        // moves heat. It is what makes the convection pattern answer to the
        // tectonics instead of running forever on its seed: a cell that has
        // been swallowing slabs becomes a cold downwelling, the flow reorganises
        // around it, and the conveyor's convergence MIGRATES rather than
        // grinding the same ground for four billion years.
        add_flux(&SUNK_BITS, sank_kg);
        if sank_kg > 0.0 {
            let cell_mass = world.mantle.cell_mass(cell);
            if cell_mass > 0.0 {
                let t = world.mantle.temp_k[cell];
                let share = (sank_kg / cell_mass).clamp(0.0, 1.0);
                world.mantle.temp_k[cell] = t - share * (t - SLAB_SURFACE_K);
            }
        }
        winner.pile_on(scraped);
    }

    // What went down drives what comes back up, refined on the way: the
    // incompatible elements partition into the melt and the refractory ones stay
    // behind. Nothing is created — the return is drawn from the mantle the slab
    // was just credited to.
    if !returning.is_empty() {
        let mut melt = Vec::new();
        for (e, m) in returning {
            // `m` already carries the arc return and its flux-melting boost.
            let want = m * crate::crust::continental_affinity(e);
            if want <= 0.0 {
                continue;
            }
            let got = world.mantle.remove(cell, e, want);
            if got > 0.0 {
                // Already back at the surface here, so the well never held it
                // and the spreading edge must not spend it a second time.
                *well -= got;
                melt.push((e, got));
            }
        }
        if !melt.is_empty() {
            winner.deposit(FormationProcess::ContinentalArc, world.tick_myr, &melt);
        }
    }
    world.columns[cell] = winner;
}

/// Draw a melt out of one mantle cell: `fraction` of each element it holds, scaled
/// by how readily that element enters this kind of melt. The debit half of a
/// conserved move — the caller deposits exactly what comes back.
///
/// **The one melt-draw**, shared by the ridge here and by eruptions
/// ([`Volcanism`](crate::crust::Volcanism)) — what a melt takes out of the rock
/// is one law, and only the trigger differs.
pub(crate) fn draw_melt(
    world: &mut World,
    cell: usize,
    fraction: f64,
    affinity: fn(ElementId) -> f64,
) -> Vec<(ElementId, f64)> {
    let elements: Vec<ElementId> = world.mantle.elements().to_vec();
    let before = world.mantle.cell_mass(cell);
    let mut melt = Vec::new();
    let mut drawn = 0.0;
    for e in elements {
        let want = world.mantle.mass(cell, e) * fraction * affinity(e);
        if want <= 0.0 {
            continue;
        }
        let got = world.mantle.remove(cell, e, want);
        if got > 0.0 {
            drawn += got;
            melt.push((e, got));
        }
    }
    // **Making melt costs heat.** The latent heat leaves with the liquid, so
    // the rock left behind is colder than it was — which is why a vent cannot
    // sit in one place forever living off the same anomaly. With the plume
    // damping itself and slabs chilling where they sink, the temperature field
    // finally answers to what the tectonics DID, instead of coasting on the
    // pattern it was seeded with.
    if drawn > 0.0 && before > 0.0 {
        let t = world.mantle.temp_k[cell];
        world.mantle.temp_k[cell] = (t - MELT_LATENT_K * (drawn / before)).max(SLAB_SURFACE_K);
    }
    melt
}

/// Drive one reshuffle directly, with the bodies named by hand — same reason
/// as [`collide_for_test`]: reaching the standoff through a whole tick means
/// testing the segmentation at the same time, and which cells the segmentation
/// happens to group is not the claim under test.
#[cfg(test)]
pub(crate) fn step_for_test(
    conveyor: &Conveyor,
    world: &mut World,
    labels: &[u32],
    members: &[Vec<usize>],
) {
    conveyor.step(world, labels, members)
}

/// Drive one collision directly — the seam behaviours (who sinks, what rides
/// down with it, how much cold the slab carries) are the tectonic claims worth
/// testing on their own, and reaching them through a whole conveyor step means
/// testing the segmentation at the same time.
#[cfg(test)]
pub(crate) fn collide_for_test(
    world: &mut World,
    cell: usize,
    contenders: Vec<Column>,
    arc_return: f64,
    sea_level: f64,
    area: f64,
) {
    collide(world, cell, contenders, arc_return, sea_level, area, &mut 0.0)
}

/// Every cell holds exactly one column, and each column knows the cell it stands
/// on. Checked after every reshuffle — the world is full, or the run is wrong.
pub fn audit_occupancy(world: &World, after_stage: &str) {
    for (cell, col) in world.columns.iter().enumerate() {
        if col.cell_id as usize != cell {
            panic!(
                "occupancy broken after stage '{after_stage}': cell {cell} holds a column that \
                 thinks it stands on {}. Every cell holds exactly one column.",
                col.cell_id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::Scheduler;

    /// The world stays **full**: after a step every cell holds exactly one column,
    /// and each column knows the cell it stands on. This is the invariant the whole
    /// reshuffle is built around — it is what forces a leading edge to resolve as a
    /// collision and a trailing edge to be filled.
    #[test]
    fn the_world_stays_full_through_every_step() {
        let mut w = crusted_world(6, 3);
        let mut sched = Scheduler::new(vec![Box::new(Conveyor::default())], 3);
        for _ in 0..40 {
            // Give the columns something to spend so steps actually happen.
            for cell in 0..w.columns.len() {
                let r = w.grid.dirs[cell];
                let east = Vec3::Y.cross(r).normalize_or_zero();
                w.mantle.velocity[cell] = east * 0.01;
            }
            sched.step(&mut w, 1.0, None);
            audit_occupancy(&w, "test");
            assert_eq!(w.columns.len(), w.grid.len());
        }
    }

    /// Carrying rock across the planet moves mass and never makes it. The audit is
    /// the same harness every other stage answers to; a relocation that leaked
    /// would panic naming the conveyor.
    #[test]
    fn carrying_the_world_conserves_every_gram() {
        let mut w = crusted_world(6, 11);
        let mut sched = Scheduler::new(vec![Box::new(Conveyor::default())], 11);
        for _ in 0..30 {
            for cell in 0..w.columns.len() {
                let r = w.grid.dirs[cell];
                w.mantle.velocity[cell] = Vec3::Z.cross(r).normalize_or_zero() * 0.008;
            }
            sched.step(&mut w, 1.0, None);
            w.audit("Conveyor");
        }
    }

    /// A sea level below every possible column — nothing is submerged, so a
    /// collision here tests subduction alone with flux melting out of the picture.
    const DRY_WORLD: f64 = -1.0e9;

    /// **Flux melting: water makes volcanoes.** Two identical collisions, differing
    /// only in whether the slab that went down was standing under the sea. The wet
    /// one returns more arc melt — because dissolved water lowers the melting point,
    /// which is why Earth's arcs stand over oceanic trenches.
    ///
    /// The full chain is the point: the water budget is a boundary input, sea level
    /// is *solved* from the water actually delivered, and submersion is read off
    /// that. So pouring more water on a world gives it more volcanism, and no rule
    /// anywhere connects the two.
    #[test]
    fn a_wet_slab_yields_more_arc_melt_than_a_dry_one() {
        let arc_melt_with = |sea: f64| {
            let mut w = crusted_world(6, 23);
            let cell = 0usize;
            let area = w.cell_area_m2();
            let mut loser = Column::empty(cell as u32);
            loser.layers.push(bed(&[(12, 4.0e18), (26, 4.0e18)])); // mafic — subducts
            let mut winner = Column::empty(cell as u32);
            winner.layers.push(bed(&[(14, 5.0e18), (19, 2.0e18)]));
            let before = w.columns[cell].mass_kg();
            collide_for_test(&mut w, cell, vec![winner, loser], DEFAULT_ARC_RETURN, sea, area);
            let _ = before;
            w.columns[cell]
                .layers
                .iter()
                .filter(|l| l.formed_by == FormationProcess::ContinentalArc)
                .map(|l| l.mass_kg())
                .sum::<f64>()
        };
        // The same slab, once from dry land and once from under water.
        let dry = arc_melt_with(DRY_WORLD);
        let wet = arc_melt_with(1.0e9);
        assert!(dry > 0.0, "a dry slab still feeds an arc: {dry:.3e}");
        assert!(
            wet > dry * 1.5,
            "the wet slab melts far more readily: wet {wet:.3e} vs dry {dry:.3e}"
        );
    }

    /// **Why continents survive.** A slab sinks only as far as its own rock allows:
    /// dense beds go back to the mantle, and the first bed too buoyant to follow
    /// stops it, so everything above is thrust onto the winner instead. Nothing says
    /// "continental crust does not subduct" — it simply cannot.
    #[test]
    fn a_slab_sinks_only_as_far_as_its_rock_allows() {
        let mut w = crusted_world(6, 7);
        let cell = 0usize;
        let before_mantle = w.mantle.element_mass(14);

        // A loser built bottom-up: dense floor, then buoyant rock, then more dense.
        let mut loser = Column::empty(cell as u32);
        loser.layers.push(bed(&[(12, 3.0e18), (26, 3.0e18)])); // mafic — sinks
        loser.layers.push(bed(&[(14, 4.0e18), (19, 1.0e18)])); // felsic — stops it
        loser.layers.push(bed(&[(12, 2.0e18), (26, 2.0e18)])); // above the stop
        let mut winner = Column::empty(cell as u32);
        winner.layers.push(bed(&[(14, 5.0e18), (19, 2.0e18)]));

        let area = w.cell_area_m2();
        collide_for_test(&mut w, cell, vec![winner, loser], DEFAULT_ARC_RETURN, DRY_WORLD, area);

        let survived = &w.columns[cell];
        assert!(survived.layers.len() >= 3, "the buoyant rock and what rode above it stayed");
        assert!(
            survived.element_mass(14) > 4.0e18,
            "the felsic bed was thrust onto the winner, not swallowed"
        );
        assert!(
            w.mantle.element_mass(12) > 0.0 && w.mantle.element_mass(14) >= before_mantle,
            "the dense floor went back to the mantle"
        );
    }

    /// Two buoyant stacks meeting lose nothing at all — the loser rides bodily onto
    /// the winner and the pile thickens. Isostasy reads a range off it afterwards;
    /// no rule here mentions mountains.
    #[test]
    fn two_buoyant_stacks_thicken_instead_of_sinking() {
        let mut w = crusted_world(6, 13);
        let cell = 0usize;
        let mut a = Column::empty(cell as u32);
        a.layers.push(bed(&[(14, 6.0e18), (19, 3.0e18)]));
        let mut b = Column::empty(cell as u32);
        b.layers.push(bed(&[(14, 5.0e18), (19, 2.5e18)]));
        let total = a.mass_kg() + b.mass_kg();

        let area = w.cell_area_m2();
        collide_for_test(&mut w, cell, vec![a, b], DEFAULT_ARC_RETURN, DRY_WORLD, area);

        let piled = &w.columns[cell];
        assert_eq!(piled.layers.len(), 2, "neither stack was swallowed");
        assert!(
            (piled.mass_kg() - total).abs() < 1e-6 * total,
            "every gram of both is standing on this cell"
        );
    }

    /// Ground nobody arrived at does not stay bare: the mantle beneath decompresses
    /// and freezes new crust — dense, thin, and drawn from that cell's own mantle.
    #[test]
    fn open_ground_is_filled_by_the_mantle_beneath_it() {
        let mut w = crusted_world(6, 17);
        let cell = 4usize;
        let before = w.mantle.total_mass();
        w.columns[cell] = Column::empty(cell as u32);

        open_ground(&mut w, cell, 0.0);

        assert!(!w.columns[cell].layers.is_empty(), "the vacancy filled");
        assert!(
            w.mantle.total_mass() < before,
            "the new crust came OUT of the mantle, it was not conjured"
        );
        assert!(
            w.columns[cell].mean_density() > SUBDUCTABLE_DENSITY,
            "fresh ridge crust is dense — it rides low, which is what makes ocean floor"
        );
    }


    fn bed(elements: &[(ElementId, f64)]) -> crate::column::Layer {
        let mut c = flicker_worldstate::Composition::new();
        for &(e, m) in elements {
            c.add(e, m);
        }
        crate::column::Layer {
            elements: c,
            minerals: flicker_worldstate::CompoundLedger::new(),
            formed_at_myr: 0.0,
            formed_by: FormationProcess::OceanicCrust,
            peak_pt: (0.0, 0.0),
            cooled: 0.0,
            eclogitised: 0.0,
        }
    }

    /// A world that has already frozen a crust, so there is lithosphere to couple.
    fn crusted_world(freq: u32, seed: u64) -> World {
        use crate::budget::Budget;
        use crate::config::content_data_dir;
        use flicker_materials::{JsonTableSource, Tables};
        use flicker_worldgrid::icosphere;
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, seed);
        let mut sched = Scheduler::new(crate::formation_stages(std::sync::Arc::clone(&t), &w, &crate::Levers::brisk()), seed);
        for _ in 0..90 {
            sched.step(&mut w, 1.0, None);
        }
        w
    }

    /// A world with a COMPLETE lid (every cell crusted, conserved) and nothing
    /// else run — the clean stage for driving the conveyor by hand.
    fn lidded_world(freq: u32, seed: u64) -> World {
        use crate::budget::Budget;
        use crate::config::content_data_dir;
        use flicker_materials::{JsonTableSource, Tables};
        use flicker_worldgrid::icosphere;
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, seed);
        crate::planet::freeze_lid(&mut w);
        w
    }

    /// Every column replaced by a felsic stack drawn from its own mantle cell —
    /// an all-continental world, so every meeting is buoyant-against-buoyant.
    /// Two beds, because the crumple thrusts the COVER and keeps the basement,
    /// and a one-bed world could not show the difference.
    fn continental_world(freq: u32, seed: u64) -> World {
        let mut w = lidded_world(freq, seed);
        for cell in 0..w.columns.len() {
            for bed in w.columns[cell].take_all() {
                for (e, m) in bed.elements.iter() {
                    w.mantle.add(cell, e, m);
                }
            }
            // A basement and a cover, laid by two DIFFERENT processes — which
            // is what starts a bed now that a stratum is a depositional event
            // rather than a composition contrast (ruling B, 2026-08-07). A
            // one-bed column has no cover to thrust, so the crumple would have
            // nothing to act on. Masses sized to a freq-6 planetoid's cells,
            // which hold only a few 10^18 kg all told.
            let beds = [(13u8, FormationProcess::ContinentalArc), (11, FormationProcess::Volcanic)];
            for (k, (cover, process)) in beds.into_iter().enumerate() {
                let mut melt = Vec::new();
                for (e, want) in [(14u8, 2.0e17), (cover, 5.0e16)] {
                    let got = w.mantle.remove(cell, e, want);
                    if got > 0.0 {
                        melt.push((e, got));
                    }
                }
                w.columns[cell].deposit(process, k as f64, &melt);
            }
        }
        w.audit("continental fixture");
        w
    }

    /// **LAND DOES NOT RIDE OVER LAND** (Aaron, 2026-08-06: *"it sounds exactly
    /// like the conditions under which we should be pushing hexes together to
    /// make them cohesive continental plates"*).
    ///
    /// Drive an all-continental world into itself. Every meeting is between two
    /// stacks that refuse to sink, so no ground may be vacated and no sea floor
    /// may be manufactured: the count of cells carrying unsinkable basement is
    /// the world's LAND AREA, and it must not fall. Before the standoff every
    /// such meeting spent one cell — the loser rode onto the winner and the
    /// ground it left froze a fresh mafic floor — so continental area drained
    /// away one hex per collision, which is where the one-hex spikes and the
    /// black ring beside them came from.
    #[test]
    fn continents_that_collide_keep_their_ground() {
        let mut w = continental_world(6, 5);
        let land = |w: &World| w.columns.iter().filter(|c| unsubductable(c)).count();
        let before = land(&w);
        assert_eq!(before, w.columns.len(), "the fixture starts all-continental");

        // TWO hemispheres driving head-on into each other at the equator, with
        // the bodies named by hand. It has to be two BODIES: same-body
        // contention is lattice noise and the dedup above rightly un-steps it,
        // so a single global plate never reaches the standoff at all. The
        // displacement is set directly rather than accumulated from a velocity
        // field, so the claim under test is the reshuffle and not the
        // segmentation's opinion of where a boundary belongs.
        let axis = Vec3::Y;
        let n = w.columns.len();
        let mut labels = vec![0u32; n];
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); 3];
        for cell in 0..n {
            let north = axis.dot(w.grid.dirs[cell]) >= 0.0;
            labels[cell] = if north { 1 } else { 2 };
            members[labels[cell] as usize].push(cell);
        }
        let conveyor = Conveyor::default();
        for _ in 0..40 {
            for (cell, &label) in labels.iter().enumerate() {
                let r = w.grid.dirs[cell];
                let tangent = axis - r * axis.dot(r);
                let sign = if label == 1 { -1.0 } else { 1.0 };
                let sp = cell_spacing(&w, cell);
                w.columns[cell].accum_disp = tangent.normalize_or_zero() * sign * sp * 1.2;
            }
            step_for_test(&conveyor, &mut w, &labels, &members);
            w.audit("Conveyor");
            assert_eq!(
                land(&w),
                before,
                "a collision spent continental ground: {} of {before} cells left",
                land(&w),
            );
        }
        // And the convergence did real work rather than simply freezing: the
        // cover has been thrust, so somewhere is carrying more beds than it
        // started with.
        let deepest = w.columns.iter().map(|c| c.layers.len()).max().unwrap_or(0);
        assert!(deepest > 2, "the crumple thickened a belt: deepest stack is {deepest} beds");
    }

    /// **A THRUST COSTS CONVERGENCE.** The crumple used to fire once per locked
    /// contact per TICK: the arrest refunds the stride, nothing debited it, so a
    /// contact re-earned its step every tick without the plates closing any
    /// further. The belt then thickened at a rate set by the timestep rather
    /// than by how fast anything was actually moving — and after the layer cake
    /// doubled the bed count it pegged slope p90 at the angle of repose across a
    /// tenth of the planet.
    ///
    /// The test the old one could not be: the fixture above re-sets
    /// `accum_disp` every iteration, so it re-earns the step by hand and the
    /// pacing never showed. Here the convergence is banked ONCE and the world is
    /// left to run — so the sheets thrust are bounded by the convergence
    /// supplied, not by how many ticks it is chopped into.
    #[test]
    fn the_crumple_spends_the_shortening_it_thrusts() {
        let mut w = continental_world(6, 5);
        let axis = Vec3::Y;
        let n = w.columns.len();
        let mut labels = vec![0u32; n];
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); 3];
        for cell in 0..n {
            let north = axis.dot(w.grid.dirs[cell]) >= 0.0;
            labels[cell] = if north { 1 } else { 2 };
            members[labels[cell] as usize].push(cell);
        }

        // **Deep columns, or the test cannot see the defect.** The standard
        // fixture carries two beds and the basement never goes, so every cell
        // can thrust exactly ONE sheet whatever the pacing — which is precisely
        // why the belt test above passed both before and after this bug existed.
        // Six beds gives a foreland something to strip.
        const DEEP: usize = 6;
        for cell in 0..n {
            for k in 2..DEEP {
                let mut melt = Vec::new();
                for (e, want) in [(14u8, 5.0e16), (13u8, 2.0e16)] {
                    let got = w.mantle.remove(cell, e, want);
                    if got > 0.0 {
                        melt.push((e, got));
                    }
                }
                // Alternating processes: a bed is a depositional EVENT now.
                let process = if k % 2 == 0 {
                    FormationProcess::Volcanic
                } else {
                    FormationProcess::ContinentalArc
                };
                w.columns[cell].deposit(process, k as f64, &melt);
            }
        }
        w.audit("deep fixture");
        let shallowest_before = w.columns.iter().map(|c| c.layers.len()).min().unwrap_or(0);
        assert!(shallowest_before >= DEEP - 1, "the fixture really is deep");

        // Bank TWO cell-spacings of convergence, once, and never top it up.
        let banked = 2.0f32;
        for cell in 0..n {
            let r = w.grid.dirs[cell];
            let tangent = axis - r * axis.dot(r);
            let sign = if labels[cell] == 1 { -1.0 } else { 1.0 };
            let sp = cell_spacing(&w, cell);
            w.columns[cell].accum_disp = tangent.normalize_or_zero() * sign * sp * banked;
        }
        let beds_before: usize = w.columns.iter().map(|c| c.layers.len()).sum();

        let conveyor = Conveyor::default();
        for _ in 0..30 {
            step_for_test(&conveyor, &mut w, &labels, &members);
            w.audit("Conveyor");
        }

        // Thrusting MOVES a bed between columns, so the planet's total bed count
        // cannot change — that is the conservation half.
        let beds_after: usize = w.columns.iter().map(|c| c.layers.len()).sum();
        assert_eq!(beds_before, beds_after, "thrusting moves beds, it does not make them");

        // And the pacing half: a foreland can only give up as many sheets as the
        // convergence paid for. Unpaced, thirty ticks strip every foreland to
        // its basement regardless of how little the plates actually closed.
        let stripped = w.columns.iter().map(|c| c.layers.len()).min().unwrap_or(0);
        assert!(
            stripped as f32 >= DEEP as f32 - banked - 1.0,
            "a foreland cannot be stripped faster than it converges: {stripped} beds left \
             of {DEEP} after banking {banked} spacings over 30 ticks"
        );
    }

    /// The standoff is about BUOYANCY, not about stopping the conveyor: sea
    /// floor is dense all the way down, so it still goes under a continent and
    /// still comes back as arc melt. Without this the fix would have quietly
    /// ended subduction — and with it, trenches, arcs and the whole
    /// distillation loop.
    #[test]
    fn sea_floor_still_subducts_under_a_continent() {
        let mut w = crusted_world(6, 7);
        let cell = 0usize;
        let mafic = bed(&[(12, 3.0e18), (26, 3.0e18)]);
        let felsic = bed(&[(14, 5.0e18), (13, 1.5e18)]);
        assert!(!bed_arrests(&mafic), "sea floor sinks");
        assert!(bed_arrests(&felsic), "refined rock does not");

        let mut loser = Column::empty(cell as u32);
        loser.layers.push(mafic);
        let mut winner = Column::empty(cell as u32);
        winner.layers.push(felsic);
        assert!(!unsubductable(&loser), "an all-mafic stack is disposable");
        assert!(unsubductable(&winner), "a felsic stack is not");

        let before = w.mantle.element_mass(12);
        let area = w.cell_area_m2();
        collide_for_test(&mut w, cell, vec![winner, loser], DEFAULT_ARC_RETURN, DRY_WORLD, area);
        assert!(w.mantle.element_mass(12) > before, "the slab went down");
    }

    /// **DOES THE WHOLE STACK TRAVEL, OR ONLY ITS BASE?** (Aaron in-window,
    /// 2026-08-06, offered as an observation rather than a diagnosis: *"it
    /// seems like only the lowest layer of the hex stack is being moved… when
    /// the plates move, everything in the tile that is a ground layer should be
    /// moving as well."*)
    ///
    /// It matters beyond the look of it: burial depth is position in the stack,
    /// so a column that arrived without its cover would carry the wrong
    /// `peak_pt` and every read that depends on depth — metamorphic grade
    /// first — would be reading fiction.
    ///
    /// Marks one column's beds with stamps nothing else uses, steps the
    /// conveyor, and requires all of them to be found together, in order, with
    /// their masses intact.
    #[test]
    fn a_moving_plate_carries_its_whole_stack() {
        let mut w = continental_world(6, 21);
        let n = w.columns.len();
        // Three beds nothing else could be confused with, bottom → top.
        let stamps = [101.0f64, 102.0, 103.0];
        let source = 40usize;
        // Clear the fixture cell the conserved way — `layers.clear()` would
        // simply destroy the rock, and the harness rightly refuses that.
        for bed in w.columns[source].take_all() {
            for (e, m) in bed.elements.iter() {
                w.mantle.add(source, e, m);
            }
        }
        let mut masses = Vec::new();
        // Each bed a genuinely different rock — like accretes to like, so beds
        // of one composition merge into one bed (correctly) and there would be
        // nothing to tell apart.
        for (k, (stamp, cover)) in stamps.iter().zip([13u8, 11, 19]).enumerate() {
            let mut melt = Vec::new();
            for (e, want) in [(14u8, 2.0e17 * (k + 1) as f64), (cover, 8.0e16)] {
                let mut want = want;
                for src in 0..w.mantle.n_cells() {
                    if want <= 0.0 {
                        break;
                    }
                    let took = w.mantle.remove(src, e, want);
                    if took > 0.0 {
                        melt.push((e, took));
                    }
                    want -= took;
                }
            }
            // Alternating processes, because a bed is a depositional EVENT now:
            // three arc pulses onto un-lithified arc rock are correctly ONE bed,
            // and this fixture needs three.
            let process =
                if k == 1 { FormationProcess::Volcanic } else { FormationProcess::ContinentalArc };
            w.columns[source].deposit(process, *stamp, &melt);
            masses.push(w.columns[source].layers.last().expect("just laid").mass_kg());
        }
        assert_eq!(w.columns[source].layers.len(), 3, "the fixture is a three-bed stack");
        w.audit("stack fixture");

        // One body, one heading — a plain translation, so the stack has
        // somewhere to go and the claim is about carriage, not segmentation.
        let axis = w.grid.dirs[source];
        let labels = vec![1u32; n];
        let members: Vec<Vec<usize>> = vec![Vec::new(), (0..n).collect()];
        for cell in 0..n {
            let r = w.grid.dirs[cell];
            let tangent = (axis.cross(r)).cross(r);
            let sp = cell_spacing(&w, cell);
            w.columns[cell].accum_disp = tangent.normalize_or_zero() * sp * 1.2;
        }
        step_for_test(&Conveyor::default(), &mut w, &labels, &members);
        w.audit("Conveyor");

        // Where did the marked beds end up?
        let mut homes: Vec<(usize, Vec<f64>)> = Vec::new();
        for (cell, col) in w.columns.iter().enumerate() {
            let found: Vec<f64> = col
                .layers
                .iter()
                .filter(|l| stamps.contains(&l.formed_at_myr))
                .map(|l| l.formed_at_myr)
                .collect();
            if !found.is_empty() {
                homes.push((cell, found));
            }
        }
        assert_eq!(
            homes.len(),
            1,
            "the stack was split across {} cells — the cover did not travel with its base: {homes:?}",
            homes.len()
        );
        let (landed, found) = &homes[0];
        assert_eq!(found.as_slice(), &stamps, "all three beds arrived, still in order");
        let arrived = &w.columns[*landed];
        for (k, stamp) in stamps.iter().enumerate() {
            let bed = arrived
                .layers
                .iter()
                .find(|l| l.formed_at_myr == *stamp)
                .expect("checked present");
            assert!(
                (bed.mass_kg() - masses[k]).abs() < 1.0,
                "bed {stamp} arrived with {:.3e} kg, not the {:.3e} it left with",
                bed.mass_kg(),
                masses[k]
            );
        }
    }

    /// **The seam defect Aaron saw in-window.** Around the twelve pentagons the
    /// grid's orientation twists, and a rigid body driven across the twist had
    /// its own cells pinched into one another and fed to the mantle — section
    /// edges "overwriting" material along grid lines (conserved, so it read as
    /// overwrite while actually being self-subduction). A rigid body's own
    /// motion must never subduct any of itself: across many steps of a rotation
    /// centred ON a pentagon — the hardest lattice case — the mantle must never
    /// GAIN from the crust. (It may only lose a little, to ridge fill at true
    /// trailing edges.)
    #[test]
    fn a_rigid_body_never_feeds_itself_to_the_mantle() {
        let mut w = lidded_world(6, 5);
        let pent =
            w.grid.is_pentagon.iter().position(|&b| b).expect("an icosphere has pentagons");
        let axis = w.grid.dirs[pent];
        // Yield high enough that the uniform-strain rigid field reads as ONE
        // body — the claim under test is about a body's own motion, not about
        // where segmentation draws its boundaries.
        let conveyor = Conveyor { yield_strain: 1.0e6, ..Conveyor::default() };
        let mut sched = Scheduler::new(vec![Box::new(conveyor)], 5);
        let ticks = 80;
        let mut fed_in = 0.0f32;
        for _ in 0..ticks {
            for cell in 0..w.columns.len() {
                let r = w.grid.dirs[cell];
                let v = axis.cross(r) * 0.02;
                w.mantle.velocity[cell] = v;
                fed_in += v.length();
            }
            let mantle_before = w.mantle.total_mass();
            sched.step(&mut w, 1.0, None);
            w.audit("Conveyor");
            let mantle_after = w.mantle.total_mass();
            assert!(
                mantle_after <= mantle_before + 1.0,
                "the body fed {:.3e} kg of itself to the mantle at a lattice twist",
                mantle_after - mantle_before
            );
        }
        // And the run genuinely moved: most of the distance carried in was SPENT
        // in steps. (A standing balance of a few spacings per cell is honest —
        // oblique lattice edges spend a stride's worth of vector per step, not a
        // stride's worth of progress — but the old vector-mean trigger starved a
        // rotation entirely, because its mean displacement cancels to zero, and
        // that must never come back.)
        let standing: f32 = w.columns.iter().map(|c| c.accum_disp.length()).sum();
        assert!(
            standing < 0.5 * fed_in,
            "stepping barely spent what the rotation carried in: {standing} of {fed_in}"
        );
    }



    /// The fit recovers the rotation it was given: sample a rigid field and get its
    /// own axis back. Without this a "plate" is just a bag of cells that happen to
    /// be adjacent.
    #[test]
    fn the_pole_fit_recovers_a_rigid_rotation() {
        let omega = Vec3::new(0.3, -0.7, 0.2);
        let points: Vec<Vec3> = (0..40)
            .map(|k| {
                let a = k as f32 * 0.37;
                Vec3::new(a.cos(), (a * 1.7).sin(), (a * 0.9).cos()).normalize()
            })
            .collect();
        let fitted = euler_pole(points.iter().map(|&r| (r, omega.cross(r))));
        assert!(
            (fitted - omega).length() < 1e-2,
            "fitted {fitted:?} from a field generated by {omega:?}"
        );
    }

    /// A field that is not a rotation still yields a finite axis rather than
    /// blowing up — the regulariser doing its job on a degenerate domain.
    #[test]
    fn the_pole_fit_stays_finite_on_a_degenerate_domain() {
        let r = Vec3::X;
        let fitted = euler_pole(std::iter::repeat((r, Vec3::Y)).take(3));
        assert!(fitted.is_finite());
    }
}



