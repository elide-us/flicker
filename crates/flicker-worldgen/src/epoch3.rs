//! **Epoch 3 — plate tectonic structuring** (world-gen spec).
//!
//! Partition the hex array into drifting **plates**, classify each hex's boundary
//! from its neighbours' plates and motion, and write a mean surface
//! **elevation**. The base elevation comes from **crust buoyancy** (isostasy):
//! the thick, light crust Epoch 2 floated up rides high (continents), the thin
//! dense crust sits low (ocean basins) — so continents appear *where Epoch 2's
//! light crust is* and Epoch 3 visibly continues Epoch 2. Plate boundaries then
//! carve on top: **convergent boundaries throw up mountain belts**, divergent
//! boundaries open rifts. The elevation it writes is the proto-heightmap the
//! erosion sim later refines. Needs the hex neighbour graph (from [`EpochCtx`]).

use std::cmp::Ordering;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use flicker_worldstate::Composition;

use crate::pipeline::{EpochCtx, EpochTransform, NOMINAL_DURATION};
use crate::state::{Boundary, HexState};

/// Epoch 3 parameters.
pub struct Epoch3 {
    /// Number of plates to seed.
    pub plates: usize,
    /// Land/ocean balance: the fraction of the surface that reads continental. The
    /// most-buoyant `continental_fraction` of hexes (top crust-buoyancy percentile)
    /// are flagged continental; the rest oceanic. Same dial purpose as before (more
    /// → more land), but the *shapes* now follow the material (Epoch 2 crust), not a
    /// random per-plate roll.
    pub continental_fraction: f32,
    /// High end of the isostatic elevation ramp — the base elevation of the most
    /// buoyant (thickest, lightest) crust.
    pub continental_base: f32,
    /// Low end of the isostatic elevation ramp — the base elevation of the least
    /// buoyant (thinnest, densest) crust.
    pub oceanic_base: f32,
    /// Max uplift added at a fully-convergent boundary (mountains).
    pub mountain_uplift: f32,
    /// Max drop at a fully-divergent boundary (rift).
    pub rift_drop: f32,
    /// Closing-speed magnitude above which a boundary counts as convergent /
    /// divergent (below it is a transform fault).
    pub boundary_threshold: f32,
    /// Number of fixed mantle hotspots (plumes). `0` disables hotspot volcanism
    /// entirely — the default, so the base tectonics output is unchanged.
    pub hotspots: usize,
    /// Peak uplift a hotspot adds where a plate sits over it.
    pub hotspot_uplift: f32,
    /// Trail length of the volcanic chain the drifting plate leaves downstream of
    /// a hotspot (in unit-sphere chord units) — larger = longer island chains.
    pub hotspot_trail: f32,
    /// Oldest crust age in millions of years — the age a stable plate interior /
    /// continental craton reaches. Sea floor ages from ~0 at a divergent ridge up
    /// toward this at the far (subducting) margin.
    pub max_age: f32,
    /// Within-epoch **drift time** on the shared clock — how long the plates move.
    /// Displacement = rate × time, so more time accumulates taller convergent belts,
    /// deeper rifts, and **longer hotspot island chains** (the plate has drifted
    /// further over the plume). Normalised to [`NOMINAL_DURATION`], so the nominal
    /// value reproduces today's output.
    pub duration: u32,
    /// Absolute-isostasy blend weight `0..1` (see [`Epoch3::apply`]): `0` = the
    /// original pure-percentile base elevation (the one-shot pipeline), which stretches
    /// any crust into a full continent↔ocean gradient; higher blends in the **absolute**
    /// crust thickness so near-mean crust reads as broad shelves and only genuinely
    /// piled crust rises — the fix for the rank-stretched "thin ridge spine" relief.
    /// The redesign engine sets it; default `0` (one-shot pipeline unchanged).
    pub isostasy_abs: f32,
    /// Airy crust-**pile** uplift scale (see [`Epoch3::apply`]): elevation added for
    /// crust piled above the planetary-mean thickness — the mountain source once plates
    /// have drifted and collided their crust. `0` disables it (the one-shot pipeline
    /// never drifts, so nothing piles). The redesign engine sets it from the
    /// mountain-uplift lever; default `0`.
    pub crust_pile: f32,
}

impl Default for Epoch3 {
    fn default() -> Self {
        Self {
            plates: 8,
            continental_fraction: 0.4,
            // The isostatic ramp dominates the elevation: continents sit well above
            // sea level (`0` until the hydrosphere), ocean basins well below, so the
            // gross land/ocean pattern reads as the (inherited) crust and the
            // coastline falls at the continental threshold (see `apply`). Tectonic
            // deformation then rides on top.
            continental_base: 0.4,
            oceanic_base: -0.6,
            mountain_uplift: 0.6,
            rift_drop: 0.25,
            boundary_threshold: 0.15,
            hotspots: 0,
            hotspot_uplift: 0.6,
            hotspot_trail: 0.4,
            max_age: 200.0,
            duration: NOMINAL_DURATION,
            isostasy_abs: 0.0,
            crust_pile: 0.0,
        }
    }
}

/// A tectonic plate — the spec's cross-hex `plates` record: a stable id, its kind
/// (continental vs oceanic), its rigid drift over the sphere, and the hexes that
/// belong to it. Output of [`Epoch3::partition`], recorded alongside the per-hex
/// layer (the per-hex [`HexState::plate`] is the reverse mapping).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plate {
    /// Stable plate id — also this plate's index in [`Partition::plates`].
    pub id: u16,
    /// Continental (rides high) vs oceanic (sits low) — a plate-level summary, set
    /// from the plate's **mean crust buoyancy** (the per-hex flag, used for crust
    /// age, is set from each hex's own buoyancy).
    pub continental: bool,
    /// Rigid-body drift direction over the unit sphere; the relative motion across
    /// a shared edge is what classifies a boundary convergent / divergent.
    pub motion: Vec3,
    /// Hex indices belonging to this plate.
    pub members: Vec<u32>,
}

/// The plate partition: each hex's plate index plus the cross-hex [`Plate`]
/// records. Produced by [`Epoch3::partition`]; [`Epoch3::apply`] runs the same
/// partition internally, so the per-hex layer and the cross-hex records agree.
#[derive(Clone, Debug, PartialEq)]
pub struct Partition {
    /// Per-hex plate index (`0..plates.len()`), indexed by hex.
    pub plate: Vec<usize>,
    /// One record per plate, indexed by plate id.
    pub plates: Vec<Plate>,
}

/// Cross-trail half-width of a hotspot chain (unit-sphere chord units).
const HOTSPOT_WIDTH: f32 = 0.07;

/// Fraction of convergent uplift a *fully oceanic* hex gets — a modest island arc.
/// Buoyant continental crust scales up to the full belt, so mountains build the
/// inherited continents rather than spawning random mid-ocean land (the thing that
/// made Epoch 3 read as disconnected from Epoch 2).
const OCEANIC_OROGENY: f32 = 0.25;

/// Per-step advance rate of the plate conveyor: a plate accumulates this much of a
/// whole-cell step each iteration and advances **one entire cell** once it reaches 1.0
/// — so the crust moves as coherent full-cell hops, never a fractional diffusive smear
/// (the old `advect_crust_step` moved 0.2 of every cell every step, which *blurred* the
/// material and re-derived the seams from the blur). At 0.2 a plate steps one cell every
/// 5 iterations — the anchor for the timeline calibration below.
const ADVECT_FRAC: f32 = 0.2;
/// `crust_fraction` of freshly produced ridge crust — the thin, young oceanic
/// lithosphere a divergent seam differentiates out of the local mantle.
const FRESH_CRUST_CF: f64 = 0.15;
/// Ridge melt differentiation cutoff (g/cm³): only formers at or below this density rise
/// into the fresh oceanic crust (the heavies stay in the mantle), so produced crust is a
/// real light differentiate, not a scaled copy of the whole column. Matches Epoch 2's
/// default `crust_density_max`.
const FRESH_CRUST_DENSITY_MAX: f64 = 3.5;
/// Crust-thickness (`crust_fraction`) gap above which a convergence is a **subduction**
/// (the thin/heavy slab dives) rather than a **collision** (similar buoyancy → both pile
/// into a belt). Below the gap, two continents collide and thicken; above it, the thin
/// plate subducts.
const SUBDUCT_CONTRAST: f64 = 0.2;
/// Fraction of a **subducting** slab's crust scraped onto the overriding plate (accreted
/// wedge / arc root); the rest recycles into the mantle — so trenches consume crust
/// instead of piling it, which also balances the ridge production over deep time.
const SUBDUCT_ACCRETE: f64 = 0.15;
/// Plate drift speed used to calibrate the iteration → time mapping (cm/year; Earth
/// plates run ~2–10, 5 is a mid value).
const PLATE_SPEED_CM_YR: f32 = 5.0;
/// One hex across in cm (49.65 mi). A plate crossing one hex ≈ `HEX_CM /
/// PLATE_SPEED` years ≈ 1.6 My — the physical anchor for the tectonic timeline.
const HEX_CM: f32 = 49.65 * 160_934.4;
/// **Real time one drift iteration represents** (million years): the crust advects
/// `ADVECT_FRAC` of a hex, and a full hex of plate motion takes `HEX_CM/PLATE_SPEED`
/// years. So iteration N of the tectonic scrubber = `N × MY_PER_TECTONIC_STEP` My.
pub const MY_PER_TECTONIC_STEP: f32 = ADVECT_FRAC * (HEX_CM / PLATE_SPEED_CM_YR / 1.0e6);

/// Slope of the absolute-isostasy tanh in [`Epoch3::apply`]: how sharply the base
/// elevation responds to a crust-thickness anomaly (fraction above/below the
/// planetary mean) before it saturates. Gentle, so ordinary crust contrasts make
/// gentle relief (broad shelves) rather than the rank-stretched full gradient.
const ISOSTASY_ABS_GAIN: f32 = 2.0;
/// Crust-thickness excess (fraction above the planetary mean) at which the Airy
/// pile uplift reaches half its ceiling — sets how much piling reads as a full belt.
/// Larger ⇒ crust must pile more before it lifts, so belts stay broad, not spiky.
const PILE_HALF: f32 = 0.6;

/// splitmix64 → `[0, 1)` for deterministic per-plate / per-seed randomness.
fn rand01(z: u64) -> f64 {
    let mut z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Neighbour-smoothing passes that turn E2's per-hex `crust_fraction` into a
/// large-scale buoyancy field — enough to suppress per-hex noise, but not so much
/// that the material provinces melt into a couple of half-planet blobs. Fewer
/// passes ⇒ more, smaller cratons with boundaries that hug the material structure.
const CRATON_SMOOTH_PASSES: u32 = 3;

/// The **plate-forming field**, derived from the MATERIAL: Epoch 2's crust buoyancy
/// (`crust_fraction`) smoothed to a large scale. Thick buoyant crust reads as a
/// **craton** (a stable plate core); the thin crust between cratons is where plates
/// break and collide. Because it is a **pure function of the upstream crust** — no
/// per-epoch seed noise — the plate layout is **stable across Epoch-3 reseeds**: the
/// tectonics are a consequence of the materials, not a fresh random field each roll.
fn craton_field(ctx: &EpochCtx, prev: &[HexState]) -> Vec<f32> {
    let n = prev.len();
    let mut f: Vec<f32> = prev.iter().map(|s| s.crust_fraction as f32).collect();
    for _ in 0..CRATON_SMOOTH_PASSES {
        let mut next = f.clone();
        for i in 0..n {
            let mut sum = f[i];
            let mut cnt = 1.0f32;
            for &nb in &ctx.neighbors[i] {
                sum += f[nb as usize];
                cnt += 1.0;
            }
            next[i] = sum / cnt;
        }
        f = next;
    }
    f
}

/// Plate cores = the strongest **craton centres** (local maxima of the smoothed
/// buoyancy field), up to `k`. Material-derived positions, stable across reseeds.
fn craton_seeds(ctx: &EpochCtx, field: &[f32], k: usize) -> Vec<usize> {
    let n = field.len();
    let mut maxima: Vec<usize> = (0..n)
        .filter(|&i| ctx.neighbors[i].iter().all(|&nb| field[i] >= field[nb as usize]))
        .collect();
    maxima
        .sort_by(|&a, &b| field[b].partial_cmp(&field[a]).unwrap_or(Ordering::Equal).then(a.cmp(&b)));
    maxima.truncate(k.max(1));
    if maxima.is_empty() {
        maxima.push(0);
    }
    maxima
}

/// A plate's rigid **drift**, derived from the material: the tangent direction from
/// its craton core (buoyancy max) toward its **thinnest-crust member** — the plate
/// leans toward the weak edge where it collides/subducts. Deterministic and
/// non-zero (a reference-tangent fallback for a uniform plate), so reseeding Epoch 3
/// leaves the drift (and thus the whole plate field) essentially unchanged.
fn plate_drift(ctx: &EpochCtx, seed: usize, members: &[u32], field: &[f32]) -> Vec3 {
    let d = ctx.dirs[seed];
    let low = members
        .iter()
        .copied()
        .min_by(|&a, &b| {
            field[a as usize].partial_cmp(&field[b as usize]).unwrap_or(Ordering::Equal)
        })
        .unwrap_or(seed as u32) as usize;
    let off = ctx.dirs[low] - d;
    let tang = off - off.dot(d) * d;
    if tang.length_squared() > 1e-8 {
        return tang.normalize_or_zero();
    }
    // Fallback tangent: a reference axis not parallel to `d`, projected to the plane.
    let axis = if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    (axis - axis.dot(d) * d).normalize_or_zero()
}

impl Epoch3 {
    /// Partition the hexes into drifting plates: grow a Voronoi partition over the
    /// neighbour graph from deterministic seed hexes, then give each plate a rigid
    /// 3D drift and a continental/oceanic kind from its **mean crust buoyancy**
    /// ([`buoyancy_ranks`] of `prev`). Returns the per-hex assignment and the
    /// cross-hex [`Plate`] records (the spec's `plates` structure). Deterministic
    /// from the seed + upstream crust, so calling it to **record** the cross-hex
    /// output yields the same partition [`apply`](Self::apply) uses internally.
    pub fn partition(&self, ctx: &EpochCtx, prev: &[HexState]) -> Partition {
        let n = ctx.dirs.len();
        if n == 0 {
            return Partition { plate: Vec::new(), plates: Vec::new() };
        }
        // Mantle convection drives it: seed plates at the upwelling cell centres,
        // then grow a Voronoi partition over the neighbour graph. Each plate's rigid
        // drift is the mean **mantle flow** (down-gradient, upwelling → downwelling)
        // over its members — so adjacent plates converge/diverge as the flow field
        // dictates, and mountains build where the convection genuinely collides
        // crust, not at random seams.
        let k = self.plates.max(1).min(n);
        let field = craton_field(ctx, prev);
        let seeds = craton_seeds(ctx, &field, k);
        let kk = seeds.len();
        let plate = grow_plates(n, &seeds, &field, ctx.neighbors);

        // Crust buoyancy ranked across the planet + per-plate members (for the mean
        // continental kind and the material-derived drift).
        let ranks = buoyancy_ranks(prev);
        let cont_threshold = 1.0 - self.continental_fraction.clamp(0.0, 1.0);
        let mut rank_sum = vec![0.0f32; kk];
        let mut count = vec![0u32; kk];
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); kk];
        for (i, &p) in plate.iter().enumerate() {
            rank_sum[p] += ranks.get(i).copied().unwrap_or(0.0);
            count[p] += 1;
            members[p].push(i as u32);
        }

        // Per-plate kind (from mean buoyancy) + rigid drift (material: the plate
        // leans toward its thinnest-crust edge).
        let plates: Vec<Plate> = (0..kk)
            .map(|p| {
                let mean_rank = if count[p] > 0 { rank_sum[p] / count[p] as f32 } else { 0.0 };
                let continental = mean_rank >= cont_threshold;
                let motion = plate_drift(ctx, seeds[p], &members[p], &field);
                Plate { id: p as u16, continental, motion, members: std::mem::take(&mut members[p]) }
            })
            .collect();
        Partition { plate, plates }
    }

    /// **Heat-driven plate partition** — the redesign path: plates form over Epoch 2's
    /// convection cells instead of the crust-thickness peaks. Seed at the **upwelling
    /// centres** (local maxima of the smoothed `heat` field) — their **count is emergent**
    /// (however many convection blooms the molten era produced), replacing the hard-coded
    /// `plates` seed count that pinned every world to the same number — then grow a
    /// watershed over the heat so seams settle in the **downwelling troughs** between
    /// cells. Each plate's summary `motion` is the mean convection `flow` of its members
    /// (the sim uses the per-cell `flow`; this is for the record/display). Continental kind
    /// still reads off crust buoyancy. Deterministic from the (frozen Epoch-2) heat.
    pub fn partition_heat(
        &self,
        ctx: &EpochCtx,
        prev: &[HexState],
        heat: &[f32],
        flow: &[Vec3],
    ) -> Partition {
        let n = ctx.dirs.len();
        if n == 0 {
            return Partition { plate: Vec::new(), plates: Vec::new() };
        }
        // One neighbour-average pass so the maxima are clean cell centres, not the ragged
        // clusters a raw field would give (a plateau of equal cells → many tiny plates).
        let mut field = heat.to_vec();
        let mut sm = field.clone();
        for i in 0..n {
            let mut sum = field[i];
            let mut cnt = 1.0f32;
            for &nb in &ctx.neighbors[i] {
                sum += field[nb as usize];
                cnt += 1.0;
            }
            sm[i] = sum / cnt;
        }
        field = sm;

        let seeds = heat_maxima(ctx, &field);
        let kk = seeds.len();
        let plate = grow_plates(n, &seeds, &field, ctx.neighbors);

        let ranks = buoyancy_ranks(prev);
        let cont_threshold = 1.0 - self.continental_fraction.clamp(0.0, 1.0);
        let mut rank_sum = vec![0.0f32; kk];
        let mut flow_sum = vec![Vec3::ZERO; kk];
        let mut count = vec![0u32; kk];
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); kk];
        for (i, &p) in plate.iter().enumerate() {
            rank_sum[p] += ranks.get(i).copied().unwrap_or(0.0);
            flow_sum[p] += flow.get(i).copied().unwrap_or(Vec3::ZERO);
            count[p] += 1;
            members[p].push(i as u32);
        }
        let plates: Vec<Plate> = (0..kk)
            .map(|p| {
                let mean_rank = if count[p] > 0 { rank_sum[p] / count[p] as f32 } else { 0.0 };
                let continental = mean_rank >= cont_threshold;
                let motion = flow_sum[p].normalize_or_zero();
                Plate { id: p as u16, continental, motion, members: std::mem::take(&mut members[p]) }
            })
            .collect();
        Partition { plate, plates }
    }

    /// **Plate conveyor over the convection flow** (the reality-like tectonic motion).
    /// Given a `part`ition (heat-derived — see [`partition_heat`](Self::partition_heat))
    /// and the per-cell convection **`flow`** field ([`convection_flow`]), move the plates
    /// as *coherent units* for `steps` iterations: each cell's **crust** (elements +
    /// thickness + plate id) is advected forward by whole cells along the **local flow**
    /// — not diffused, and not along a single straight per-plate vector (which is what
    /// made the old mountain belts straight). Because the flow curves around the
    /// convection cells, streamlines converge/diverge along **curved** lines:
    /// - **convergent** (streamlines merging, at the cold downwellings) → the arriving
    ///   crust **accretes**, piling onto the seam (→ thick crust → curved mountain belts)
    ///   and the thicker/buoyant plate's label claims the contested cell (it overrides);
    /// - **divergent** (a cell left with no inflow, at the hot upwellings) → a spreading
    ///   ridge **produces** thin fresh crust re-differentiated from the local mantle.
    ///
    /// The mantle `composition` (the conserved Epoch-1 ledger) is left untouched, so the
    /// element ledger is conserved by construction; the crust veneer moves/piles/produces
    /// as its own surface layer (the same crust-vs-composition split Epoch 2 established).
    /// Returns the moved cells **and the tracked partition** — the labels rode *with* the
    /// plates. One advance ≈ 5 iterations ≈ [`MY_PER_TECTONIC_STEP`]×5 My.
    ///
    /// (Slice 2 will add realistic subduction — crust returning to the mantle,
    /// heavy-under-light — and differentiated ridge melt.)
    pub fn drift_plates(
        &self,
        ctx: &EpochCtx,
        prev: &[HexState],
        mut part: Partition,
        flow: &[Vec3],
        steps: u32,
    ) -> (Vec<HexState>, Partition) {
        let n = prev.len();
        if n == 0 || steps == 0 {
            return (prev.to_vec(), part);
        }
        let mut plate_of = part.plate.clone();
        let mut cells = prev.to_vec();

        // Accumulate whole-cell advances: material hops one entire cell along the local
        // flow each time the phase crosses 1.0 — coherent, not a diffusive smear.
        let mut phase = 0.0f32;
        for _ in 0..steps {
            phase += ADVECT_FRAC;
            if phase < 1.0 {
                continue;
            }
            phase -= 1.0;
            advance_plates(&mut cells, &mut plate_of, ctx, flow);
        }

        // Rebuild per-plate membership from the migrated labels (ids + motions persist;
        // only which cells belong to each plate moved).
        let kk = part.plates.len();
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); kk];
        for (i, &p) in plate_of.iter().enumerate() {
            members[p].push(i as u32);
        }
        for (p, plate) in part.plates.iter_mut().enumerate() {
            plate.members = std::mem::take(&mut members[p]);
        }
        part.plate = plate_of;
        (cells, part)
    }
}

/// Per-hex **crust buoyancy** ranked in `0..1` across the planet: each hex's
/// `crust_fraction` (Epoch 2 — how much light crust floated up, a crust-thickness
/// proxy) turned into its percentile rank. Thick light crust → rank near 1 (rides
/// high, continents); thin dense crust → rank near 0 (sits low, ocean basins).
/// Ranking rather than an absolute min-max guarantees a full continent↔ocean
/// spread even when the absolute crust-fraction spread is thin — the same
/// percentile trick Epoch 4 uses to set sea level. This is the signal Epoch 3
/// turns into base elevation, so continents land where the light crust is.
pub fn buoyancy_ranks(prev: &[HexState]) -> Vec<f32> {
    let n = prev.len();
    if n == 0 {
        return Vec::new();
    }
    // Sort hex indices by crust buoyancy ascending; ties broken by index so the
    // ranking (and thus the whole epoch) stays deterministic.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        prev[a]
            .crust_fraction
            .partial_cmp(&prev[b].crust_fraction)
            .unwrap_or(Ordering::Equal)
            .then(a.cmp(&b))
    });
    let denom = (n - 1).max(1) as f32;
    let mut rank = vec![0.0f32; n];
    for (pos, &i) in order.iter().enumerate() {
        rank[i] = pos as f32 / denom;
    }
    rank
}

/// Blend fraction toward the neighbour mean per de-scatter pass — gentle, so single-cell
/// piles spread into ranges while the large-scale continent/ocean thickness stays.
const CRUST_DESCATTER: f64 = 0.5;

/// **De-scatter the drifted crust thickness before isostasy.** The whole-cell plate
/// conveyor ([`advance_plates`]) resolves each convergence/ridge *per cell*, so once the
/// first advance fires it can leave crust piled on isolated convergence cells and thinned
/// to [`FRESH_CRUST_CF`] on isolated ridge cells — a cell-scale `crust_fraction` scatter
/// that [`Epoch3::apply_with`]'s isostasy turns into a zig-zagged, single-cell-spiky
/// relief (the "noise field" that appears the instant plate transport starts). A couple
/// of gentle neighbour-average passes spread each pile/ridge over a few cells (broad
/// ranges + basins) while preserving the large-scale thickness structure. It relaxes only
/// the **derived thickness proxy** — the conserved `composition` ledger and the plate
/// labels are untouched — and the sharp seams are added later by [`run_orogeny`] from the
/// flow, so they stay crisp. Only the drift path needs it; the one-shot pipeline never
/// advances, so it never calls this.
pub fn smooth_crust_thickness(cells: &mut [HexState], ctx: &EpochCtx, passes: u32) {
    let n = cells.len();
    if n == 0 {
        return;
    }
    for _ in 0..passes {
        let snap: Vec<f64> = cells.iter().map(|c| c.crust_fraction).collect();
        for i in 0..n {
            let (mut sum, mut cnt) = (snap[i], 1.0f64);
            for &nb in &ctx.neighbors[i] {
                sum += snap[nb as usize];
                cnt += 1.0;
            }
            cells[i].crust_fraction = snap[i] + CRUST_DESCATTER * (sum / cnt - snap[i]);
        }
    }
}

impl EpochTransform for Epoch3 {
    fn epoch(&self) -> u8 {
        3
    }

    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        // The one-shot pipeline derives the partition fresh from `prev` and has no
        // convection flow, so it synthesizes a **uniform per-plate flow** (each cell moves
        // with its plate's single motion vector) — reproducing the pre-heat straight-seam
        // behaviour. The redesign engine passes the real per-cell convection flow.
        let part = self.partition(ctx, prev);
        let flow: Vec<Vec3> =
            (0..prev.len()).map(|i| part.plates[part.plate[i]].motion).collect();
        self.apply_with(ctx, prev, &part, &flow)
    }
}

impl Epoch3 {
    /// [`EpochTransform::apply`] against a **caller-supplied** partition + per-cell **flow**
    /// field — the seam plus isostasy/deformation layer. Boundaries are classified from the
    /// *local* flow (`(flow[i]−flow[nb])·across`), so with the engine's curved convection
    /// flow the convergent/divergent seams come out **curved**; the one-shot pipeline
    /// passes a uniform per-plate flow and gets the old straight boundaries. The engine also
    /// passes the partition [`drift_plates`](Self::drift_plates) tracked through the conveyor.
    pub fn apply_with(
        &self,
        ctx: &EpochCtx,
        prev: &[HexState],
        part: &Partition,
        flow: &[Vec3],
    ) -> Vec<HexState> {
        let n = prev.len();
        if n == 0 {
            return Vec::new();
        }

        // Crust buoyancy ranked across the planet: the isostatic base elevation and
        // the per-hex continental flag both read off it (continents where the light
        // crust floated up — Epoch 2 → Epoch 3 continuity).
        let ranks = buoyancy_ranks(prev);
        let cont_threshold = 1.0 - self.continental_fraction.clamp(0.0, 1.0);
        // Planetary-mean crust thickness — the datum the absolute isostasy and the
        // Airy pile uplift measure against (drift conserves the total, so it's stable).
        let cf_mean = (prev.iter().map(|s| s.crust_fraction).sum::<f64>() / n as f64) as f32;

        // Fixed mantle hotspots (in the mantle frame). A plate drifting over one
        // leaves an uplifted volcanic chain trailing along its motion.
        let hotspots = make_hotspots(self.hotspots, ctx.seed);

        // Within-epoch drift time: motion is a *rate*, so accumulated deformation
        // (uplift / rift) and the distance a plate has carried off a hotspot both
        // scale with elapsed time. Normalised to the nominal duration, so the
        // nominal value leaves the rate as-is (today's output).
        let drift = self.duration as f32 / NOMINAL_DURATION as f32;

        // 2. Per hex: base elevation by crust buoyancy (isostasy); boundary by the
        //    strongest relative motion across a shared edge; add mountains / rifts /
        //    hotspots on top.
        let mut out: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = prev[i].clone();
                let p = part.plate[i];
                s.plate = p as u16;
                let motion_p = part.plates[p].motion;
                // Isostatic base. The per-hex continental flag stays a pure percentile
                // (so `continental_fraction` remains an exact land-share knob), but the
                // base *elevation* blends the rank with an **absolute** Airy term (crust
                // thickness vs the planetary mean, tanh-compressed). Pure rank stretches
                // any crust into a full continent↔ocean gradient, so a barely-piled ridge
                // ranks to the top and only spines clear sea level; the absolute blend
                // keeps near-mean crust near datum (broad shelves) and lets only genuinely
                // thick crust rise.
                let rank = ranks[i];
                s.continental = rank >= cont_threshold;
                let base_t = if self.isostasy_abs > 0.0 && cf_mean > 0.0 {
                    let z = prev[i].crust_fraction as f32 / cf_mean - 1.0;
                    let t_abs = 0.5 + 0.5 * (z * ISOSTASY_ABS_GAIN).tanh();
                    (1.0 - self.isostasy_abs) * rank + self.isostasy_abs * t_abs
                } else {
                    rank
                };
                let mut elev =
                    self.oceanic_base + (self.continental_base - self.oceanic_base) * base_t;
                // Airy crust-pile: crust piled above the planetary-mean thickness
                // (drifted, collided crust) floats up as a mountain belt, saturating so
                // it broadens into a range instead of spiking. This is the single
                // convergent-mountain source — the iterative `run_orogeny` no longer adds
                // a collision belt, so the piled crust and the deformation never
                // double-count.
                if self.crust_pile > 0.0 && cf_mean > 0.0 {
                    let excess = (prev[i].crust_fraction as f32 / cf_mean - 1.0).max(0.0);
                    elev += self.crust_pile * (excess / (excess + PILE_HALF));
                }

                let mut boundary = Boundary::Interior;
                let mut strongest = 0.0f32; // signed: + closing, - opening
                for &nb in &ctx.neighbors[i] {
                    // The **local flow** across the edge (not a per-plate vector): +
                    // closes (compression → mountains, at the cold downwellings), − opens
                    // (a rift, at the hot upwellings). Because the flow curves with the
                    // convection cells, these seams come out curved. Classified across
                    // every edge — with the uniform per-plate flow the one-shot pipeline
                    // passes, intra-plate closing is 0, so it still only marks plate
                    // boundaries (old behaviour); the engine's curved flow marks the real
                    // ridges + trenches wherever the flow diverges/converges.
                    let across = (ctx.dirs[nb as usize] - ctx.dirs[i]).normalize_or_zero();
                    let closing = (flow[i] - flow[nb as usize]).dot(across);
                    if closing.abs() > strongest.abs() {
                        strongest = closing;
                    }
                }
                // Boundary *type* is the time-independent sign of relative motion;
                // the deformation *magnitude* accumulates with drift time.
                if strongest > self.boundary_threshold {
                    boundary = Boundary::Convergent;
                    let c = (strongest * drift).clamp(0.0, 1.0);
                    // Orogeny rides on buoyant crust: continental collision throws up
                    // the full belt, ocean-ocean convergence only an island arc — so
                    // mountains reinforce the inherited continents instead of
                    // overriding the crust-driven land/ocean pattern.
                    let continentality = OCEANIC_OROGENY + (1.0 - OCEANIC_OROGENY) * rank;
                    let belt = c * continentality;
                    elev += self.mountain_uplift * belt;
                    s.orogeny = belt; // fold intensity → mountain relief in the field
                    s.volcanic = (s.volcanic + 0.5 * c).clamp(0.0, 1.0);
                } else if strongest < -self.boundary_threshold {
                    boundary = Boundary::Divergent;
                    elev -= self.rift_drop * (-strongest * drift).clamp(0.0, 1.0);
                } else if strongest != 0.0 {
                    boundary = Boundary::Transform;
                }

                // Hotspot volcanism: uplift trailing the plate's motion past each
                // plume (a comet shape — a blob at the plume, a long tail
                // downstream → an island/seamount chain).
                if !hotspots.is_empty() && self.hotspot_uplift > 0.0 {
                    let cell = ctx.dirs[i];
                    // Plate drift projected into the cell's tangent plane.
                    let m_t = (motion_p - motion_p.dot(cell) * cell).normalize_or_zero();
                    // Longer drift time → the plate has carried the cell further off
                    // the plume → a longer island/seamount chain.
                    let trail = (self.hotspot_trail * drift).max(1e-3);
                    let mut lift = 0.0f32;
                    for &hs in &hotspots {
                        let off = cell - hs;
                        let along = off.dot(m_t);
                        let cross = (off - along * m_t).length();
                        // Long tail downstream (+motion), short falloff upstream.
                        let an = if along >= 0.0 { along / trail } else { -along / HOTSPOT_WIDTH };
                        let cn = cross / HOTSPOT_WIDTH;
                        lift += (-(an * an + cn * cn)).exp();
                    }
                    elev += self.hotspot_uplift * lift;
                    s.volcanic = (s.volcanic + 0.6 * lift.min(1.0)).clamp(0.0, 1.0);
                }

                s.boundary = boundary;
                s.elevation = elev.clamp(-1.0, 1.0);
                s
            })
            .collect();

        // 3. Crust age: sea floor is created at the divergent ridges and ages with
        //    distance away from them (toward the subducting margins); continental
        //    crust is old, stable craton. BFS the hop distance from every ridge.
        let dist = ridge_distance(n, &out, ctx.neighbors);
        let max_d = dist.iter().copied().filter(|&d| d != u32::MAX).max().unwrap_or(0);
        for (i, s) in out.iter_mut().enumerate() {
            let d_norm = if max_d == 0 {
                1.0
            } else {
                match dist[i] {
                    u32::MAX => 1.0,
                    d => d as f32 / max_d as f32,
                }
            };
            s.plate_age = if s.continental {
                self.max_age * (0.6 + 0.4 * d_norm)
            } else {
                self.max_age * d_norm
            };
        }

        out
    }
}

/// `k` mantle hotspots spread uniformly over the unit sphere, deterministic from
/// `seed`. `y` (= sin latitude) uniform gives an equal-area spread.
fn make_hotspots(k: usize, seed: u64) -> Vec<Vec3> {
    (0..k)
        .map(|h| {
            let a = seed ^ (h as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let y = rand01(a) as f32 * 2.0 - 1.0;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let phi = rand01(a ^ 0x5151_5151) as f32 * std::f32::consts::TAU;
            Vec3::new(r * phi.cos(), y, r * phi.sin())
        })
        .collect()
}

/// Multi-source BFS giving each hex's hop distance to the nearest **divergent**
/// boundary (a spreading ridge), or `u32::MAX` if no ridge is reachable. Drives
/// the sea-floor age gradient.
fn ridge_distance(n: usize, states: &[HexState], neighbors: &[Vec<u32>]) -> Vec<u32> {
    let mut dist = vec![u32::MAX; n];
    let mut frontier = Vec::new();
    for (i, s) in states.iter().enumerate() {
        if s.boundary == Boundary::Divergent {
            dist[i] = 0;
            frontier.push(i);
        }
    }
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for &h in &frontier {
            let d = dist[h] + 1;
            for &nb in &neighbors[h] {
                let nb = nb as usize;
                if nb < n && dist[nb] == u32::MAX {
                    dist[nb] = d;
                    next.push(nb);
                }
            }
        }
        frontier = next;
    }
    dist
}

/// Grow plates from `seeds` by a **watershed flood over the material `field`**
/// (highest crust first): each hex joins the plate whose flood — descending from a
/// craton peak — reaches it first. So plate boundaries settle in the **thin-crust
/// valleys** *between* cratons, tracking the material's own structure, instead of the
/// straight geometric midlines a plain distance-Voronoi draws. Deterministic (the
/// heap orders by a fixed-point field key, ties by index).
fn grow_plates(n: usize, seeds: &[usize], field: &[f32], neighbors: &[Vec<u32>]) -> Vec<usize> {
    use std::collections::BinaryHeap;
    let key = |v: f32| (v * 1_000_000.0) as i64; // fixed-point so f32 orders in the heap
    let mut plate = vec![usize::MAX; n];
    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    for (p, &s) in seeds.iter().enumerate() {
        if plate[s] == usize::MAX {
            plate[s] = p;
            heap.push((key(field[s]), s));
        }
    }
    // Pop the highest-crust frontier hex and claim its unassigned neighbours, so the
    // fronts descend the crust field and meet in the low (thin-crust) valleys.
    while let Some((_, h)) = heap.pop() {
        let p = plate[h];
        for &nb in &neighbors[h] {
            let nb = nb as usize;
            if nb < n && plate[nb] == usize::MAX {
                plate[nb] = p;
                heap.push((key(field[nb]), nb));
            }
        }
    }
    // Any hex unreached (disconnected) falls to plate 0.
    for p in plate.iter_mut() {
        if *p == usize::MAX {
            *p = 0;
        }
    }
    plate
}

/// The **convection surface flow** at each cell, from the Epoch-2 [`heat`](HexState::heat)
/// field: a unit tangent pointing **down the heat gradient** — from the hot upwelling
/// toward the cold downwelling, i.e. toward the cell's coldest neighbour. This is the
/// plate **motion** as a *per-cell, spatially-varying* field: because it curves around the
/// convection cells, the seams and mountain belts it drives come out **curved**, not the
/// straight spines a single per-plate vector produced. Zero at a downwelling sink (no
/// colder neighbour) or a locally flat patch.
pub fn convection_flow(ctx: &EpochCtx, heat: &[f32]) -> Vec<Vec3> {
    let n = heat.len().min(ctx.dirs.len());
    (0..n)
        .map(|i| {
            let d = ctx.dirs[i];
            let mut coldest = None;
            let mut cold_h = heat[i];
            for &nb in &ctx.neighbors[i] {
                let j = nb as usize;
                if heat[j] < cold_h {
                    cold_h = heat[j];
                    coldest = Some(j);
                }
            }
            match coldest {
                Some(j) => {
                    let to = ctx.dirs[j] - d;
                    (to - to.dot(d) * d).normalize_or_zero() // tangential, toward the cold
                }
                None => Vec3::ZERO,
            }
        })
        .collect()
}

/// Upwelling centres = local maxima of the heat field — the plate cores. Their **count is
/// emergent** (one per convection bloom), so the number of plates varies with the seed
/// instead of being pinned to a lever. Deterministic (ties by index; falls back to cell 0
/// so there is always at least one seed).
fn heat_maxima(ctx: &EpochCtx, heat: &[f32]) -> Vec<usize> {
    let n = heat.len();
    let mut seeds: Vec<usize> = (0..n)
        .filter(|&i| ctx.neighbors[i].iter().all(|&nb| heat[i] >= heat[nb as usize]))
        .collect();
    if seeds.is_empty() {
        seeds.push(0);
    }
    seeds
}

/// The neighbour of `i` the crust advances **into** — the one best aligned with the
/// per-cell `flow` projected into `i`'s tangent plane. `None` if the flow is tangentially
/// degenerate at `i` (a downwelling sink / flat patch), in which case the crust stays put.
/// A small positive-alignment floor keeps a sideways/back neighbour from ever being chosen.
fn down_drift(i: usize, ctx: &EpochCtx, motion: Vec3) -> Option<usize> {
    let d = ctx.dirs[i];
    let mt = (motion - motion.dot(d) * d).normalize_or_zero();
    if mt.length_squared() < 1e-8 {
        return None;
    }
    let mut best = None;
    let mut best_align = 1e-3f32; // require a real forward component
    for &nb in &ctx.neighbors[i] {
        let off = (ctx.dirs[nb as usize] - d).normalize_or_zero();
        let a = off.dot(mt);
        if a > best_align {
            best_align = a;
            best = Some(nb as usize);
        }
    }
    best
}

/// A spreading ridge differentiates a **thin veneer of fresh oceanic crust** out of the
/// local mantle: the **light formers** (density ≤ [`FRESH_CRUST_DENSITY_MAX`]) rise into a
/// young crust; the heavies stay below. The mantle [`composition`](HexState::composition)
/// is left intact (it's the conserved ledger; crust is a copy layer, as Epoch 2
/// established). Fills a cell a divergent seam left with no inflow, so the conveyor never
/// opens a hole.
fn produce_fresh_crust(
    mantle: &Composition,
    tables: &flicker_materials::Tables,
    crust: &mut Composition,
    cf: &mut f64,
) {
    if mantle.total() <= 0.0 {
        *cf = 0.0;
        return;
    }
    for (el, amt) in mantle.iter() {
        let light = tables
            .element_by_number(el)
            .map(|e| e.density_g_cm3 as f64 <= FRESH_CRUST_DENSITY_MAX)
            .unwrap_or(false);
        if light {
            crust.add(el, amt * FRESH_CRUST_CF);
        }
    }
    *cf = FRESH_CRUST_CF;
}

/// One **whole-cell advance** of every plate along the per-cell **`flow`** — the coherent,
/// non-diffusive transport that also does **realistic subduction** (slice 2). Each cell
/// hands its crust to its down-flow neighbour; at a convergence (a cell that several
/// streamlines merge onto) the buoyant, **thickest-crust** arrival is the **overrider** —
/// it keeps its crust and claims the cell — and each other arrival is resolved from the
/// buoyancy contrast:
/// - **collision** (similar `crust_fraction`, gap ≤ [`SUBDUCT_CONTRAST`]) → its crust piles
///   on fully, thickening the belt (continent-continent → curved mountains);
/// - **subduction** (a clearly thinner/heavier slab) → only [`SUBDUCT_ACCRETE`] of its
///   crust is scraped onto the overrider; the rest **recycles into the mantle** (dropped
///   from the crust veneer — the mantle `composition` already holds those elements).
///
/// A cell left with **no inflow** (divergence, at the hot upwellings) is a spreading ridge
/// and **produces** thin fresh differentiated crust. Trench recycling now balances ridge
/// production over deep time, so the crust cycles instead of growing without bound. The
/// conserved `composition` ledger is never moved.
fn advance_plates(cells: &mut [HexState], plate_of: &mut [usize], ctx: &EpochCtx, flow: &[Vec3]) {
    let n = cells.len();
    let dn: Vec<Option<usize>> = (0..n).map(|i| down_drift(i, ctx, flow[i])).collect();

    // Gather the cells whose crust flows onto each destination (a degenerate cell keeps
    // its own crust — it is its own sole pusher).
    let mut pushers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        pushers[dn[i].unwrap_or(i)].push(i);
    }

    let mut new_crust: Vec<Composition> = (0..n).map(|_| Composition::new()).collect();
    let mut new_cf = vec![0.0f64; n];
    let mut new_plate = plate_of.to_vec();

    for d in 0..n {
        if pushers[d].is_empty() {
            // Spreading ridge: differentiate thin fresh crust from the local mantle.
            produce_fresh_crust(&cells[d].composition, ctx.tables, &mut new_crust[d], &mut new_cf[d]);
            continue;
        }
        // The overrider = thickest-crust arrival (ties → lowest index for determinism).
        let over = *pushers[d]
            .iter()
            .max_by(|&&a, &&b| {
                cells[a]
                    .crust_fraction
                    .partial_cmp(&cells[b].crust_fraction)
                    .unwrap_or(Ordering::Equal)
                    .then(b.cmp(&a)) // higher cf, else lower index
            })
            .expect("non-empty");
        new_plate[d] = plate_of[over];
        // Start from the overrider's crust; resolve every other arrival by buoyancy.
        for (el, amt) in cells[over].crust.iter() {
            new_crust[d].add(el, amt);
        }
        new_cf[d] += cells[over].crust_fraction;
        for &i in &pushers[d] {
            if i == over {
                continue;
            }
            let contrast = cells[over].crust_fraction - cells[i].crust_fraction;
            let factor =
                if contrast > SUBDUCT_CONTRAST { SUBDUCT_ACCRETE } else { 1.0 }; // subduct vs collide
            for (el, amt) in cells[i].crust.iter() {
                new_crust[d].add(el, amt * factor);
            }
            new_cf[d] += cells[i].crust_fraction * factor;
        }
    }

    for i in 0..n {
        cells[i].crust = std::mem::take(&mut new_crust[i]);
        cells[i].crust_fraction = new_cf[i];
    }
    plate_of.copy_from_slice(&new_plate);
}

/// **One whole-cell advance of the plate conveyor** for the tick sim: transport every
/// plate's crust one cell along `flow` ([`advance_plates`]) and rebuild the partition's
/// per-plate membership from the migrated labels. The tick-sim's plate-drift process
/// calls this each time its phase accumulator crosses a whole cell (between advances the
/// crust sits still — the coherent-hop cadence), the single-step form of what the batch
/// [`Epoch3::drift_plates`] loops internally.
pub fn advance_conveyor(cells: &mut [HexState], part: &mut Partition, ctx: &EpochCtx, flow: &[Vec3]) {
    let n = cells.len();
    if n == 0 {
        return;
    }
    advance_plates(cells, &mut part.plate, ctx, flow);
    let kk = part.plates.len();
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); kk];
    for (i, &p) in part.plate.iter().enumerate() {
        if p < kk {
            members[p].push(i as u32);
        }
    }
    for (p, plate) in part.plates.iter_mut().enumerate() {
        plate.members = std::mem::take(&mut members[p]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldstate::Composition;

    use crate::state::HexState;

    /// A ring of `n` hexes (each neighbours its two ring-mates) with directions
    /// spread around the equator — a minimal connected graph for the plate logic.
    fn ring(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let dirs = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors = (0..n)
            .map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32])
            .collect();
        (dirs, neighbors)
    }

    fn ctx_for<'a>(dirs: &'a [Vec3], neighbors: &'a [Vec<u32>]) -> EpochCtx<'a> {
        // Epoch 3 never reads the tables, so a throwaway empty one is fine — but
        // the field requires a reference, so load the real one.
        EpochCtx { tables: tables_leak(), dirs, neighbors, seed: 2024 }
    }

    fn tables_leak() -> &'static flicker_materials::Tables {
        use flicker_materials::{JsonTableSource, Tables};
        use std::sync::OnceLock;
        static T: OnceLock<Tables> = OnceLock::new();
        T.get_or_init(|| {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
            Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
        })
    }

    #[test]
    fn plates_partition_every_hex() {
        let n = 30;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let out = Epoch3::default().apply(&ctx, &prev);
        assert_eq!(out.len(), n);
        for s in &out {
            assert!((s.plate as usize) < Epoch3::default().plates, "plate id out of range");
            assert!(s.elevation.is_finite() && (-1.0..=1.0).contains(&s.elevation));
        }
        // Multiple plates on a ring ⇒ real boundaries and a spread of elevations.
        let mut elevations: Vec<f32> = out.iter().map(|s| s.elevation).collect();
        elevations.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(elevations.last().unwrap() - elevations[0] > 0.1, "elevation is flat — no tectonics");
        assert!(out.iter().any(|s| s.boundary != Boundary::Interior), "no plate boundaries formed");
    }

    #[test]
    fn orogeny_only_on_convergent_boundaries() {
        let n = 30;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let out = Epoch3::default().apply(&ctx, &prev);
        for s in &out {
            match s.boundary {
                Boundary::Convergent => assert!(s.orogeny > 0.0, "convergent hex has no orogeny"),
                _ => assert_eq!(s.orogeny, 0.0, "orogeny off a convergent boundary"),
            }
        }
    }

    #[test]
    fn deterministic_for_a_seed() {
        let n = 24;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let a = Epoch3::default().apply(&ctx, &prev);
        let b = Epoch3::default().apply(&ctx, &prev);
        assert_eq!(a, b);
    }

    #[test]
    fn smooth_crust_thickness_spreads_spikes_without_touching_the_ledger() {
        let n = 24;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        // A single-cell crust pile on an otherwise uniform crust — the advance-plates scatter.
        let mut cells: Vec<HexState> = (0..n)
            .map(|_| {
                let mut s = HexState::new(Composition::from_iter([(14u8, 1000.0)]));
                s.crust_fraction = 0.5;
                s
            })
            .collect();
        cells[n / 2].crust_fraction = 2.0; // a single-cell spike (a convergence pile)
        let comp_before: f64 = cells.iter().map(|c| c.composition.total()).sum();
        smooth_crust_thickness(&mut cells, &ctx, 2);
        // The spike spread into a broad bump: the peak dropped, its neighbours rose.
        assert!(cells[n / 2].crust_fraction < 2.0, "the spike wasn't spread down");
        assert!(cells[n / 2 - 1].crust_fraction > 0.5, "the pile didn't spread to a neighbour");
        // Only the derived thickness moved — the conserved element ledger is untouched.
        let comp_after: f64 = cells.iter().map(|c| c.composition.total()).sum();
        assert_eq!(comp_before, comp_after, "de-scatter must not touch the element ledger");
        // A uniform thickness field has nothing to spread → unchanged.
        let mut flat: Vec<HexState> = (0..n)
            .map(|_| {
                let mut s = HexState::new(Composition::new());
                s.crust_fraction = 0.7;
                s
            })
            .collect();
        smooth_crust_thickness(&mut flat, &ctx, 2);
        assert!(
            flat.iter().all(|c| (c.crust_fraction - 0.7).abs() < 1e-9),
            "a uniform thickness field must stay uniform"
        );
    }

    #[test]
    fn partition_records_drifting_plates_covering_every_hex() {
        let n = 40;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let part = Epoch3::default().partition(&ctx, &prev);

        // The plate count is now EMERGENT (up to `plates` upwelling cells the mantle
        // convection actually produced) rather than a fixed number of random seeds.
        assert!(!part.plates.is_empty(), "no plates formed");
        assert!(part.plates.len() <= Epoch3::default().plates, "more plates than seeds");
        let mut seen = 0usize;
        for (p, plate) in part.plates.iter().enumerate() {
            assert_eq!(plate.id as usize, p, "plate id matches its index");
            assert!((plate.motion.length() - 1.0).abs() < 1e-3, "plate drift isn't a unit vector");
            seen += plate.members.len();
        }
        // Membership partitions the hexes: every hex in exactly one plate.
        assert_eq!(seen, n, "plate membership doesn't cover every hex once");
        for (i, &p) in part.plate.iter().enumerate() {
            assert!(part.plates[p].members.contains(&(i as u32)), "hex missing from its plate");
        }
        // The per-hex layer agrees with the cross-hex record it was built from.
        let out = Epoch3::default().apply(&ctx, &prev);
        for (i, s) in out.iter().enumerate() {
            assert_eq!(s.plate as usize, part.plate[i], "layer disagrees with the partition");
        }
    }

    /// The "always 8" fix: `partition_heat`'s plate count is **emergent from the
    /// convection cells** (one plate per heat bloom), not pinned to the `plates` lever —
    /// so a heat field with more blooms yields more plates.
    #[test]
    fn heat_partition_count_is_emergent_from_the_convection_cells() {
        let n = 60;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> =
            (0..n).map(|_| HexState::new(Composition::from_iter([(14u8, 1000.0)]))).collect();
        let count_for = |blooms: f32| {
            let heat: Vec<f32> = (0..n)
                .map(|i| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    (blooms * a).cos() * 0.5 + 0.5
                })
                .collect();
            let flow = convection_flow(&ctx, &heat);
            Epoch3::default().partition_heat(&ctx, &prev, &heat, &flow).plates.len()
        };
        // More convection blooms → more plates; the count follows the heat, not `plates`.
        let (few, many) = (count_for(2.0), count_for(5.0));
        assert!(many > few, "plate count should track the convection cells ({many} vs {few})");
        // And it is NOT the hard-coded default of 8.
        assert_ne!(few, 8, "plate count must be emergent, not the old fixed 8");
    }

    #[test]
    fn partition_is_material_derived_not_seed_derived() {
        // Same upstream crust, two *different* Epoch-3 seeds → identical plate
        // layout, because the partition is a pure function of the material
        // (crust_fraction), not the seed. This is what makes reseeding Epoch 3
        // predictable instead of rolling a whole new plate field.
        let n = 60;
        let (dirs, neighbors) = ring(n);
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                s.crust_fraction = (2.0 * a).sin() * 0.5 + 0.5; // two smooth cratons
                s
            })
            .collect();
        let ctx_a = EpochCtx { tables: tables_leak(), dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let ctx_b =
            EpochCtx { tables: tables_leak(), dirs: &dirs, neighbors: &neighbors, seed: 987_654 };
        let pa = Epoch3::default().partition(&ctx_a, &prev);
        let pb = Epoch3::default().partition(&ctx_b, &prev);
        assert_eq!(pa.plate, pb.plate, "plate layout must derive from the material, not the seed");
        let distinct: std::collections::BTreeSet<usize> = pa.plate.iter().copied().collect();
        assert!(distinct.len() >= 2, "expected multiple plates from the cratons, got {}", distinct.len());
    }

    /// The core coherence property: one whole-cell advance moves a plate's crust as a
    /// **lump** to the down-drift neighbour (conserving it), rather than smearing 20% of
    /// every cell into its neighbour (the old diffusive `advect_crust_step` — the "ripple"
    /// the viewer showed). An Fe marker rides intact to exactly one new cell.
    #[test]
    fn one_advance_moves_the_crust_as_a_coherent_lump() {
        let n = 24;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        // Single plate, a fixed drift direction; empty mantle so there's no ridge
        // production to muddy the conservation check (pure transfer).
        let mut cells: Vec<HexState> = (0..n)
            .map(|_| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14u8, 100.0)]);
                s.crust_fraction = 0.5;
                s
            })
            .collect();
        cells[0].crust.add(26, 500.0); // Fe lump marker on cell 0
        let mut plate_of = vec![0usize; n];
        // A uniform per-cell flow (one plate drifting in one direction).
        let flow = vec![Vec3::new(0.0, 0.0, 1.0); n];
        let mass0 = cells.iter().map(|s| s.crust.total()).sum::<f64>();

        advance_plates(&mut cells, &mut plate_of, &ctx, &flow);

        // Crust conserved (pure transfer, empty mantle → no production).
        let mass1 = cells.iter().map(|s| s.crust.total()).sum::<f64>();
        assert!((mass1 - mass0).abs() < 1e-6, "advance didn't conserve crust ({mass0} → {mass1})");
        // The Fe rode as ONE lump to ONE new cell — not diffused across many.
        let fe_total: f64 = cells.iter().map(|s| s.crust.amount(26)).sum();
        assert!((fe_total - 500.0).abs() < 1e-6, "Fe marker not conserved");
        let carriers = cells.iter().filter(|s| s.crust.amount(26) > 1.0).count();
        assert_eq!(carriers, 1, "the Fe marker smeared instead of moving as a lump");
        assert!(cells[0].crust.amount(26) < 1.0, "the plate didn't carry its material off cell 0");
    }

    /// Slice 2 — realistic subduction: at a convergence with a buoyancy contrast, the
    /// **heavy (thin-crust) slab subducts** (most of its crust recycles into the mantle
    /// rather than piling on the seam) while the **buoyant plate overrides**. The conserved
    /// `composition` ledger is untouched.
    #[test]
    fn subduction_recycles_the_heavy_slab_and_the_buoyant_plate_overrides() {
        // Three cells in a line 0-1-2 on a great circle; flow drives 0 and 2 into 1.
        let a = 0.2f32;
        let dirs = vec![
            Vec3::new(a.cos(), 0.0, -a.sin()),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(a.cos(), 0.0, a.sin()),
        ];
        let neighbors = vec![vec![1u32], vec![0u32, 2u32], vec![1u32]];
        let ctx = ctx_for(&dirs, &neighbors);
        // Cell 0 = heavy oceanic slab (thin crust, Fe marker); cell 2 = light continental
        // (thick); cell 1 = mid. Flow: 0 and 2 both push into 1 (a trench).
        let mk = |el: u8, crust: f64, cf: f64| {
            let mut s = HexState::new(Composition::from_iter([(el, 1000.0)]));
            s.crust = Composition::from_iter([(el, crust)]);
            s.crust_fraction = cf;
            s
        };
        let mut cells = vec![mk(26, 200.0, 0.2), mk(14, 500.0, 0.5), mk(14, 700.0, 0.7)];
        let mut plate_of = vec![0usize, 1, 2];
        let flow = vec![dirs[1] - dirs[0], Vec3::ZERO, dirs[1] - dirs[2]];
        let comp0: Vec<Composition> = cells.iter().map(|c| c.composition.clone()).collect();
        let crust_fe0: f64 = cells.iter().map(|c| c.crust.amount(26)).sum();

        advance_plates(&mut cells, &mut plate_of, &ctx, &flow);

        // The heavy slab's crust (its Fe) mostly recycled — only a sliver accreted, not
        // the full pile.
        let crust_fe1: f64 = cells.iter().map(|c| c.crust.amount(26)).sum();
        assert!(
            crust_fe1 < crust_fe0 * 0.5,
            "the heavy slab didn't subduct (crust Fe {crust_fe0} → {crust_fe1})"
        );
        // The buoyant plate (cell 2, plate 2) overrides the trench cell.
        assert_eq!(plate_of[1], 2, "the buoyant plate should override at the trench");
        // The conserved mantle ledger is untouched.
        for (i, s) in cells.iter().enumerate() {
            assert_eq!(s.composition, comp0[i], "subduction disturbed the mantle ledger");
        }
    }

    /// `drift_plates` (over the convection flow + a heat-seeded partition) leaves the
    /// conserved mantle ledger (`composition`) untouched, tracks the partition *through*
    /// the motion (every cell still labelled, membership covers the planet), and is
    /// deterministic.
    #[test]
    fn drift_plates_conserves_the_ledger_and_tracks_the_partition() {
        let n = 60;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        // A heat field with two blooms (→ two convection cells) + a crust pattern.
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                let cf = (2.0 * a).sin() * 0.5 + 0.5;
                let mut s = HexState::new(Composition::from_iter([(14u8, 1000.0)])); // mantle
                s.crust = Composition::from_iter([(14u8, cf * 100.0)]);
                s.crust_fraction = cf;
                s.heat = ((2.0 * a).cos() as f32) * 0.5 + 0.5; // two hot/cold cells
                s
            })
            .collect();
        let heat: Vec<f32> = prev.iter().map(|s| s.heat).collect();
        let flow = convection_flow(&ctx, &heat);
        let part = Epoch3::default().partition_heat(&ctx, &prev, &heat, &flow);

        let (cells, part_out) =
            Epoch3::default().drift_plates(&ctx, &prev, part.clone(), &flow, 12);

        // The conserved element ledger (the mantle `composition`) is never moved by the
        // conveyor — only the crust veneer travels.
        for (i, s) in cells.iter().enumerate() {
            assert_eq!(s.composition, prev[i].composition, "the conveyor disturbed the mantle ledger");
        }
        // The partition rode with the plates: every cell labelled, ids intact, motion is
        // a unit-or-zero summary vector, membership partitions the hexes.
        assert_eq!(part_out.plate.len(), n);
        let mut seen = 0usize;
        for (p, pl) in part_out.plates.iter().enumerate() {
            assert_eq!(pl.id as usize, p, "plate id must match its index");
            assert!(pl.motion.length() < 1.001, "plate motion summary should be unit-or-zero");
            seen += pl.members.len();
        }
        assert_eq!(seen, n, "tracked membership must still cover every hex once");
        // Deterministic.
        let (again, _) = Epoch3::default().drift_plates(&ctx, &prev, part, &flow, 12);
        assert_eq!(cells, again, "the conveyor must be deterministic");
    }

    // NOTE: the old `more_drift_time_accumulates_more_relief` test (apply's one-shot
    // orogeny scaling with `duration` on an equatorial ring) was retired: the
    // material-derived plate drift produces no convergence on a 1D ring (degenerate),
    // and the engine no longer uses apply's one-shot orogeny anyway. The deep-time
    // "more iterations ⇒ more relief" contract now lives on the real physics:
    // `tectonics::converging_plates_build_a_belt_that_grows_over_deep_time` (controlled
    // convergent setup) + worldengine `scrubbing_epoch3_iterations_builds_relief_progressively`
    // (the real sphere through the engine).

    #[test]
    fn plate_age_is_young_at_ridges_and_old_on_continents() {
        let n = 48;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n).map(|_| HexState::new(Composition::new())).collect();
        let e3 = Epoch3::default();
        let out = e3.apply(&ctx, &prev);

        // Ages are finite and bounded by max_age.
        assert!(out
            .iter()
            .all(|s| s.plate_age.is_finite() && (0.0..=e3.max_age + 1.0).contains(&s.plate_age)));
        // Continental crust is old (cratonic) — at least 0.6 of the max age.
        for s in &out {
            if s.continental {
                assert!(s.plate_age >= 0.6 * e3.max_age - 1.0, "continental crust read too young");
            }
        }
        // Where an oceanic spreading ridge formed, the crust on it is the youngest
        // and the field grades older away from it.
        let oceanic_ridge: Vec<f32> = out
            .iter()
            .filter(|s| s.boundary == Boundary::Divergent && !s.continental)
            .map(|s| s.plate_age)
            .collect();
        if !oceanic_ridge.is_empty() {
            let ridge_age = oceanic_ridge.iter().copied().fold(f32::MAX, f32::min);
            let oldest = out.iter().map(|s| s.plate_age).fold(f32::MIN, f32::max);
            assert!(oldest > ridge_age, "no crust-age gradient away from the ridge");
        }
    }

    /// The isostasy continuity guarantee: base elevation rises with Epoch 2's crust
    /// buoyancy, so continents land where the light crust floated up (Epoch 2 →
    /// Epoch 3 visibly continues). Boundary/hotspot relief is switched off so the
    /// elevation *is* the buoyancy base, with a uniform single-element crust so the
    /// buoyancy ranks purely by thickness.
    #[test]
    fn elevation_follows_crust_buoyancy() {
        let n = 40;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        // Ascending crust fraction; same (light) crust material everywhere.
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14, 1000.0)]); // silica
                s.crust_fraction = i as f64 / (n - 1) as f64;
                s
            })
            .collect();
        let e3 = Epoch3 { mountain_uplift: 0.0, rift_drop: 0.0, hotspots: 0, ..Epoch3::default() };
        let out = e3.apply(&ctx, &prev);

        // Elevation is monotone non-decreasing in crust buoyancy (fed ascending).
        for w in out.windows(2) {
            assert!(
                w[1].elevation >= w[0].elevation - 1e-6,
                "elevation should rise with crust buoyancy ({} -> {})",
                w[0].elevation,
                w[1].elevation
            );
        }
        // The least-buoyant hex bottoms out at the ocean base, the most-buoyant tops
        // out at the continental base — a full continent↔ocean spread.
        assert!((out[0].elevation - e3.oceanic_base).abs() < 1e-5, "least-buoyant hex isn't ocean floor");
        assert!(
            (out[n - 1].elevation - e3.continental_base).abs() < 1e-5,
            "most-buoyant hex isn't continental high"
        );
        // And the buoyant end reads continental, the dense end oceanic.
        assert!(out[n - 1].continental, "most-buoyant hex should read continental");
        assert!(!out[0].continental, "least-buoyant hex should read oceanic");
    }

    /// The repurposed `continental_fraction` knob is a land/ocean balance: it sets
    /// roughly what fraction of the surface reads continental, and the shapes follow
    /// the crust (the most-buoyant hexes), not a random roll.
    #[test]
    fn continental_fraction_sets_the_land_share() {
        let n = 50;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14, 1000.0)]);
                s.crust_fraction = i as f64 / (n - 1) as f64;
                s
            })
            .collect();
        let land = |frac: f32| {
            let e3 = Epoch3 { continental_fraction: frac, ..Epoch3::default() };
            e3.apply(&ctx, &prev).iter().filter(|s| s.continental).count()
        };
        // More balance → more land, and the share tracks the knob (±a hex of rounding).
        assert!(land(0.2) < land(0.5) && land(0.5) < land(0.8), "land share should track the knob");
        let quarter = land(0.25);
        assert!((quarter as i32 - n as i32 / 4).abs() <= 2, "≈a quarter should read continental, got {quarter}");
    }

    /// Fix #2 (the "thin ridge spine" driver): with `isostasy_abs > 0`, a
    /// **near-uniform** crust makes only a **small** elevation spread — the absolute
    /// Airy term keeps near-mean crust near datum — instead of the pure-percentile
    /// rank stretching those tiny thickness differences into a full continent↔ocean
    /// gradient (whose only emergent land, under a sea level, is thin spines).
    #[test]
    fn absolute_isostasy_compresses_a_near_uniform_crust() {
        let n = 40;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        // Crust that barely differs cell to cell (a tiny ramp around 1.0).
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14, 1000.0)]);
                s.crust_fraction = 1.0 + 0.02 * (i as f64 / (n - 1) as f64);
                s
            })
            .collect();
        let span = |e3: &Epoch3| {
            let out = e3.apply(&ctx, &prev);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for s in &out {
                lo = lo.min(s.elevation);
                hi = hi.max(s.elevation);
            }
            hi - lo
        };
        let base = Epoch3 { mountain_uplift: 0.0, rift_drop: 0.0, hotspots: 0, ..Epoch3::default() };
        let rank_span = span(&base); // isostasy_abs = 0 → pure rank → full range
        let abs_span = span(&Epoch3 { isostasy_abs: 0.8, ..base });
        // The pure-rank mapping stretches the tiny crust spread across the whole ramp;
        // the absolute blend collapses it toward datum.
        assert!(
            abs_span < 0.5 * rank_span,
            "absolute isostasy should compress a near-uniform crust ({abs_span} vs rank {rank_span})"
        );
    }

    /// Fix #1 (the single mountain source): crust piled above the planetary mean floats
    /// up into a belt via the Airy `crust_pile` term — the piled cells rise *more* than
    /// they would from the isostatic rank alone.
    #[test]
    fn piled_crust_floats_into_a_mountain_belt() {
        let n = 40;
        let (dirs, neighbors) = ring(n);
        let ctx = ctx_for(&dirs, &neighbors);
        // A baseline crust with a piled ridge at the middle few cells.
        let mid = n / 2;
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14, 1000.0)]);
                s.crust_fraction = if i.abs_diff(mid) <= 1 { 2.6 } else { 1.0 };
                s
            })
            .collect();
        // Isolate the pile term: no boundary orogeny, no absolute-blend, no rift.
        let base = Epoch3 { mountain_uplift: 0.0, rift_drop: 0.0, hotspots: 0, ..Epoch3::default() };
        let flat = base.apply(&ctx, &prev);
        let piled = Epoch3 { crust_pile: 0.6, ..base }.apply(&ctx, &prev);
        // The piled ridge rises with the crust-pile term engaged; the baseline crust
        // (below the mean) gets no pile uplift.
        assert!(
            piled[mid].elevation > flat[mid].elevation + 0.1,
            "piled crust didn't float into a belt ({} vs {})",
            piled[mid].elevation,
            flat[mid].elevation
        );
        let edge = (mid + n / 2) % n; // a baseline (thin-crust) cell far from the pile
        assert!(
            (piled[edge].elevation - flat[edge].elevation).abs() < 1e-4,
            "below-mean crust should get no pile uplift"
        );
    }
}
