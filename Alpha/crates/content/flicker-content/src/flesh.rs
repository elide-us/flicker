//! FLESH — a mesh's own volume as a voxel field: occupancy plus the INSCRIBED RADIUS of every
//! solid cell. The guided-rig primitive (spec 76EB9552): one field answers both questions a
//! rigger has to ask of a mesh.
//!
//! *Where is the middle of the body mass* on the axis an orthographic drag cannot see —
//! [`Flesh::depth_at`], which resolves the column the joint sits in, so a LEFT-view drag at knee
//! height lands in the joint's OWN leg instead of the gap between the legs.
//!
//! *Where along a limb does the flesh NARROW* — [`narrowings`] over a [`Flesh::profile`]. Joints
//! are a tube's narrowings, never its bulges or its extremes: reading extremes is what put the
//! lizard's knee on the quadriceps bulge and its hock in the calf (incident D9D837FF).
//!
//! How it is built: rasterise the triangles conservatively into a grid, flood the EXTERIOR in
//! from the grid boundary through the cells no triangle touched, and every cell the flood never
//! reached is flesh — so a mesh with small holes or doubled interior walls still reads as solid,
//! and nothing needs a ray kernel (the open ruling B8D37267). A chamfer distance transform then
//! gives each flesh cell the radius of the largest ball centred there that fits inside the body;
//! the medial point near a position is the cell with the biggest radius, and a limb's joints are
//! the local minima of that radius along it.
//!
//! Frame and units are the rig's own — centimetres, Z up, forward −Y: a `RawModel` as
//! [`crate::conform::scale_mesh_to_stature`] leaves it. Build ONCE per mesh and share it (the
//! build is a mesh-wide pass; rebuilding per limb is the thing this type exists to avoid).

use std::collections::HashSet;

use glam::Vec3;

use crate::fbx::RawModel;

/// Cells along the longest bounding extent by default — ~1.3 cm cells on a 170 cm body.
pub(crate) const DEFAULT_CELLS: usize = 128;
/// The resolution floor/ceiling: below 32 a limb is thinner than a cell; above 192 a cube-ish
/// mesh costs more memory than the answer is worth.
const MIN_CELLS: usize = 32;
const MAX_CELLS: usize = 192;

/// THE CELL a body whose longest bounding extent is `span` is read at, at `cells` resolution.
/// [`Flesh::raster`] takes its own grid from here, so a caller asking what a flesh reading can
/// MEAN — the finest difference the grid could have resolved — asks exactly the question the
/// grid answered, instead of keeping a second copy of the arithmetic.
pub(crate) fn cell_for(span: f32, cells: usize) -> f32 {
    span / cells.clamp(MIN_CELLS, MAX_CELLS) as f32
}
/// Chamfer weights are integers (3 per face step, 4 per edge, 5 per corner) so equal distances
/// stay EXACTLY equal; a radius is the integer distance scaled back by this.
const CHAMFER: f32 = 3.0;

/// A run is a LIMB when it is no wider than this many inscribed radii measured at its own middle.
/// A tube read across its centre spans exactly 2r; the slack carries an oval limb and the voxel
/// grid's half cell. A torso's run is many radii wide and never qualifies — which is the whole
/// point (ruling F9F728CA: the horse's hip snapped to the body's centre because its run was read
/// as if it were the leg's).
const LIMB_RATIO: f32 = 2.6;
/// Fractions of joint→child sampled outward until the limb has separated from the body.
const LIMB_STEPS: [f32; 5] = [0.2, 0.4, 0.6, 0.8, 1.0];

/// THIN-PART GROW ([`Flesh::grow_thin`], spec 0A81088E): how much thicker than the seed's own
/// inscribed radius a cell may be and still count as the same thin part — a hair fall varies along
/// its length, a hair fall into a thigh does not.
const THIN_GROW: f32 = 1.5;
/// The grow threshold's floor and ceiling, in CELLS. The floor lets a one-cell sliver reach its
/// neighbours at all; the ceiling is what "thin" MEANS on a body — a hair fall, a mane, a tail's
/// hair, a cloth panel all sit inside eight cells (~10 cm on a 170 cm body), a torso never does, so
/// a click on fat flesh grows fat flesh's worth of nothing instead of flooding the whole figure.
const THIN_MIN_CELLS: f32 = 2.0;
const THIN_MAX_CELLS: f32 = 8.0;

/// THE TRUNK CORE ([`Flesh::core`]): a cell belongs to the body's thick middle when its inscribed
/// radius is at least this fraction of the body's largest. Half the barrel leaves out every limb,
/// the neck and the tail on the animals measured (the Horse's barrel reads ~24 cm, its cannon
/// bones ~5) while keeping the whole trunk, which is what the symmetry plane must average over.
const CORE_FRACTION: f32 = 0.5;

/// The 26 cell offsets around a cell — the connectivity a tie set is walked with.
const NEIGHBOURS: [[i64; 3]; 26] = {
    let mut out = [[0_i64; 3]; 26];
    let (mut n, mut i) = (0, 0);
    while i < 27 {
        let (a, b, c) = (i / 9 - 1, (i / 3) % 3 - 1, i % 3 - 1);
        if a != 0 || b != 0 || c != 0 {
            out[n] = [a, b, c];
            n += 1;
        }
        i += 1;
    }
    out
};

/// One component of a vector by axis index (0 = X, 1 = Y, 2 = Z).
fn comp(v: Vec3, axis: usize) -> f32 {
    v.to_array()[axis.min(2)]
}

/// Unreachable: a grid with no seed at all.
const FAR: u32 = u32::MAX / 4;

/// The two-pass 3-4-5 chamfer distance transform over a `dims` grid: every cell `seed` marks is a
/// source at distance 0, every other cell takes its integer chamfer distance to the nearest source.
/// Integer weights keep equal distances bit-equal, which is what lets `best` average a tied plateau
/// (a straight tube's axis) instead of picking whichever of its cells the scan reached first.
/// ONE transform, run both ways: seeded on the exterior it is the inscribed radius, seeded on the
/// flesh it is the distance OFF the body.
fn chamfer(seed: &[bool], dims: [usize; 3]) -> Vec<u32> {
    let total = seed.len();
    let at = |c: [usize; 3]| c[0] + dims[0] * (c[1] + dims[1] * c[2]);
    let mut d: Vec<u32> = seed.iter().map(|&s| if s { 0 } else { FAR }).collect();
    let half: Vec<([i64; 3], u32)> = (-1i64..=1)
        .flat_map(|dk| (-1i64..=1).flat_map(move |dj| (-1i64..=1).map(move |di| [dk, dj, di])))
        .filter(|o| (o[0], o[1], o[2]) < (0, 0, 0))
        .map(|o| (o, 2 + (o[0].abs() + o[1].abs() + o[2].abs()) as u32))
        .collect();
    for pass in 0..2 {
        for step in 0..total {
            // The second pass sweeps the grid backwards over the mirrored half-neighbourhood.
            let e = if pass == 0 { step } else { total - 1 - step };
            if d[e] == 0 {
                continue;
            }
            let c = [
                (e % dims[0]) as i64,
                ((e / dims[0]) % dims[1]) as i64,
                (e / (dims[0] * dims[1])) as i64,
            ];
            let mut best = d[e];
            for (o, w) in &half {
                let s = if pass == 0 { 1 } else { -1 };
                let n = [0, 1, 2].map(|a| c[a] + s * o[a]);
                if (0..3).any(|a| n[a] < 0 || n[a] >= dims[a] as i64) {
                    continue;
                }
                best = best
                    .min(d[at([n[0] as usize, n[1] as usize, n[2] as usize])].saturating_add(*w));
            }
            d[e] = best;
        }
    }
    d
}

/// One [`chamfer`] distance in cm. The integer transform counts [`CHAMFER`] per cell step, and half
/// a cell comes off because the nearest cell CENTRE of the other kind sits about half a cell past
/// the surface being measured to. The ONE conversion — the build's fields and [`Flesh::grow_thin`]
/// read the same transform through it.
fn chamfer_cm(v: u32, cell: f32) -> f32 {
    (v as f32 / CHAMFER - 0.5).max(0.0) * cell
}

/// The voxel occupancy of a mesh with its inscribed-radius field. See the module docs.
pub struct Flesh {
    /// World position of the LOW corner of cell (0, 0, 0).
    origin: Vec3,
    cell: f32,
    dims: [usize; 3],
    solid: Vec<bool>,
    /// The inscribed radius of every cell in cm — the distance to the nearest cell that is not
    /// flesh; 0 outside the body.
    radius: Vec<f32>,
    /// The same transform run the other way: every cell OUTSIDE the flesh carries its distance in
    /// cm to the nearest solid cell (0 inside). How far a garment vertex HANGS off the body — the
    /// region split's one measurement (spec 0A81088E).
    outside: Vec<f32>,
}

/// What [`Flesh::core`] measured: the thick middle of a body and the plane it is symmetric about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Core {
    /// The mean x of the core's cells — the body's symmetry plane.
    pub plane_x: f32,
    /// The core's bounding box (cm), limbs and neck excluded.
    pub lo: Vec3,
    pub hi: Vec3,
    /// The largest inscribed radius anywhere in the body (cm).
    pub radius: f32,
    /// THE BODY'S OWN LONG HORIZONTAL DIRECTION, in degrees about +Z from +X, folded into
    /// (−90, 90]: the axis the barrel runs along, as the core's cells measure it. An AXIS, not a
    /// heading — it says which way the body lies, never which end is the head.
    ///
    /// Measured over the CORE and not the bounding box, because a bull's horns, an elk's antlers
    /// and a ewe's wool coat are wider than the animal they are on: read off the box, seven of
    /// the seventeen hoofed sources claim to lie the wrong way round (2026-09-21 sweep).
    pub axis_deg: f32,
    /// The core's extent ALONG [`Core::axis_deg`] and ACROSS it (cm) — the barrel's own length
    /// and breadth however the body was modelled on the grid.
    pub along: f32,
    pub across: f32,
}

impl Flesh {
    /// Build at the default resolution (~1.3 cm cells on a 170 cm body).
    pub fn build(model: &RawModel) -> Flesh {
        Self::raster(model, DEFAULT_CELLS, None)
    }

    /// Build with `cells` cells along the longest bounding extent (clamped 32..=192).
    pub fn with_resolution(model: &RawModel, cells: usize) -> Flesh {
        Self::raster(model, cells, None)
    }

    /// Build from the vertices `keep` marks — a triangle is rasterised when ANY of its corners is
    /// kept, so the body stays SEALED at a seam a masked-out piece is welded to. The bounding box
    /// is the kept vertices' own, so a long hair fall no longer coarsens the whole grid.
    pub fn build_masked(model: &RawModel, keep: &[bool]) -> Flesh {
        Self::raster(model, DEFAULT_CELLS, Some(keep))
    }

    /// The field of the model's BODY: its tagged regions' vertices (hair, a tail's fall, a
    /// garment's hanging panels) masked out when it carries any, else the whole mesh. THE door
    /// for every fit and bench read — a tail-hair swamp must not be measured as flesh (2D31782B).
    pub fn build_body(model: &RawModel) -> Flesh {
        if model.regions.is_empty() {
            return Self::build(model);
        }
        let mut keep = vec![true; model.vertices.len()];
        for r in &model.regions {
            for &v in &r.verts {
                if let Some(k) = keep.get_mut(v as usize) {
                    *k = false;
                }
            }
        }
        Self::build_masked(model, &keep)
    }

    /// The grid is padded by one cell all round, so the exterior flood always has a clear boundary
    /// to start from even when the body touches its own bounding box.
    fn raster(model: &RawModel, cells: usize, keep: Option<&[bool]>) -> Flesh {
        let kept = |i: usize| keep.is_none_or(|k| k.get(i).copied().unwrap_or(false));
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut n_kept = 0;
        for (i, v) in model.vertices.iter().enumerate() {
            if !kept(i) {
                continue;
            }
            n_kept += 1;
            let p = Vec3::from_array(v.p);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let span = (hi - lo).max_element();
        if n_kept < 3 || !span.is_finite() || span <= 1e-6 {
            return Flesh {
                origin: Vec3::ZERO,
                cell: 1.0,
                dims: [0; 3],
                solid: Vec::new(),
                radius: Vec::new(),
                outside: Vec::new(),
            };
        }
        let cell = cell_for(span, cells);
        // HALF A CELL OF CLEARANCE round the padding, so no triangle can ever touch the outermost
        // layer. With the padding exactly one cell, a mesh with a FLAT FACE on its own bounding
        // box (a box fixture, a crate, a shield) marked that layer solid, the exterior flood then
        // had no clear boundary on that side, and every inscribed radius behind the face was
        // measured to the far side of the body instead — a 5 cm wall reading 30 cm thick.
        let dims = [0, 1, 2].map(|a| (comp(hi - lo, a) / cell).ceil() as usize + 3);
        let origin = lo - Vec3::splat(1.5 * cell);
        let total = dims[0] * dims[1] * dims[2];
        let at = |c: [usize; 3]| c[0] + dims[0] * (c[1] + dims[1] * c[2]);

        // ── THE SURFACE: conservative rasterisation. Every cell a triangle touches at all is
        // marked (the exact separating-axis test, not a point sample) — a cell the flood can slip
        // through is a hole, and one hole turns the whole body inside out.
        let mut solid = vec![false; total];
        let vert = |i: u32| {
            model
                .vertices
                .get(i as usize)
                .map(|v| Vec3::from_array(v.p))
        };
        for t in model.indices.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (vert(t[0]), vert(t[1]), vert(t[2])) else {
                continue;
            };
            if !t.iter().any(|&i| kept(i as usize)) {
                continue;
            }
            let tlo = a.min(b.min(c)) - origin;
            let thi = a.max(b.max(c)) - origin;
            let cl = [0, 1, 2].map(|x| (comp(tlo, x) / cell).floor().max(0.0) as usize);
            let ch = [0, 1, 2]
                .map(|x| ((comp(thi, x) / cell).floor() as usize).min(dims[x].saturating_sub(1)));
            for k in cl[2]..=ch[2] {
                for j in cl[1]..=ch[1] {
                    for i in cl[0]..=ch[0] {
                        let e = at([i, j, k]);
                        if !solid[e]
                            && tri_box(
                                [a, b, c],
                                origin
                                    + Vec3::new(i as f32 + 0.5, j as f32 + 0.5, k as f32 + 0.5)
                                        * cell,
                                0.5 * cell,
                            )
                        {
                            solid[e] = true;
                        }
                    }
                }
            }
        }

        // ── THE EXTERIOR: a 6-connected flood in from the grid boundary through every cell no
        // triangle touched. SOLID is then whatever the flood never reached — the body, plus any
        // sealed pocket inside it, which is flesh as far as a joint is concerned.
        let mut outside = vec![false; total];
        let mut stack: Vec<usize> = Vec::new();
        for k in 0..dims[2] {
            for j in 0..dims[1] {
                for i in 0..dims[0] {
                    let edge = i == 0
                        || j == 0
                        || k == 0
                        || i + 1 == dims[0]
                        || j + 1 == dims[1]
                        || k + 1 == dims[2];
                    let e = at([i, j, k]);
                    if edge && !solid[e] && !outside[e] {
                        outside[e] = true;
                        stack.push(e);
                    }
                }
            }
        }
        while let Some(e) = stack.pop() {
            let i = e % dims[0];
            let j = (e / dims[0]) % dims[1];
            let k = e / (dims[0] * dims[1]);
            for (a, step) in [
                (0usize, 1i64),
                (1, dims[0] as i64),
                (2, (dims[0] * dims[1]) as i64),
            ] {
                for dir in [-1i64, 1] {
                    let c = [i, j, k][a] as i64 + dir;
                    if c < 0 || c >= dims[a] as i64 {
                        continue;
                    }
                    let n = (e as i64 + dir * step) as usize;
                    if !solid[n] && !outside[n] {
                        outside[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        for (s, &out) in solid.iter_mut().zip(&outside) {
            *s = !out;
        }

        // ── THE INSCRIBED RADIUS: the chamfer transform seeded on the cells that are NOT flesh,
        // so each solid cell takes its distance to the surface. Run the OTHER way — seeded on the
        // flesh — the same transform gives every exterior cell its distance OFF the body, which is
        // how far a garment vertex hangs.
        // Half a cell comes off every distance: the nearest cell CENTRE of the other kind sits
        // about half a cell past the surface being measured to. A cell with flesh on one side
        // keeps half a cell of radius, so every solid cell still reads positive.
        let cm = |v: u32, far: f32| {
            if v >= FAR {
                far
            } else {
                chamfer_cm(v, cell)
            }
        };
        let radius = chamfer(&outside, dims)
            .iter()
            .map(|&v| cm(v, 0.0))
            .collect();
        let outside = chamfer(&solid, dims)
            .iter()
            .map(|&v| cm(v, f32::INFINITY))
            .collect();
        Flesh {
            origin,
            cell,
            dims,
            solid,
            radius,
            outside,
        }
    }

    /// The grid's cell size in cm.
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// Is `p` inside the flesh?
    pub fn contains(&self, p: Vec3) -> bool {
        self.cell_of(p).is_some_and(|c| self.solid[self.at(c)])
    }

    /// How far `p` sits OFF the body, in cm — 0 anywhere inside the flesh, and infinite past the
    /// grid (a point outside the body's own bounding box hangs by definition). The region split's
    /// one measurement: a garment vertex farther out than the hang threshold is HANGING, and one
    /// inside the body reads 0, which is rigid.
    pub fn distance_outside(&self, p: Vec3) -> f32 {
        match self.cell_of(p) {
            Some(c) => self.outside[self.at(c)],
            None => f32::INFINITY,
        }
    }

    /// The solid runs (`lo`, `hi` in cm) along `axis` (0 = X, 1 = Y, 2 = Z) through the column
    /// that contains `p` — `p`'s own coordinate on `axis` is not used, only the column it picks.
    /// A LEFT-view column through a knee crosses BOTH legs and yields two runs; one at the pelvis.
    pub fn runs(&self, p: Vec3, axis: usize) -> Vec<(f32, f32)> {
        let axis = axis.min(2);
        let Some(mut c) = self.column(p, axis) else {
            return Vec::new();
        };
        let mut out: Vec<(f32, f32)> = Vec::new();
        let mut start: Option<usize> = None;
        for i in 0..self.dims[axis] {
            c[axis] = i;
            let solid = self.solid[self.at(c)];
            match (solid, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    out.push(self.span(axis, s, i - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push(self.span(axis, s, self.dims[axis] - 1));
        }
        out
    }

    /// The MIDDLE of the run containing `p`'s own coordinate on `axis`, else of the run nearest
    /// it within `tol` (cm); `None` when no run is that close — the joint is off the mesh and
    /// belongs where the human put it.
    pub fn depth_at(&self, p: Vec3, axis: usize, tol: f32) -> Option<f32> {
        let x = comp(p, axis.min(2));
        self.nearest_run(p, axis, None)
            .filter(|&(lo, hi)| (lo - x).max(x - hi).max(0.0) <= tol)
            .map(|(lo, hi)| 0.5 * (lo + hi))
    }

    /// The hidden-axis coordinate for a joint on release (ruling F9F728CA): its own run's midpoint
    /// when that run is limb-sized (extent ≤ [`LIMB_RATIO`] × the inscribed radius at the run's
    /// midpoint — a tube through its centre spans 2r); a body run's midpoint only for a MIDLINE
    /// joint on axis 0 (the symmetry plane); else, with a limb child, the first sample along
    /// joint→child ([`LIMB_STEPS`]) whose nearest run is limb-sized supplies that run's midpoint
    /// (the limb has separated from the body: "aligned with the leg hole"); else `None` — keep the
    /// depth (a spine joint in FRONT/TOP view, a clavicle inside the chest, an empty column).
    ///
    /// THE SAME-SIDE GUARD (ruling 42AB9BA8, the mid-stride trap 38EA5048): a SIDED joint reading
    /// the symmetry axis considers only the runs that reach its OWN side of X = 0 — at its own
    /// column and all the way down the child walk. Every creature source is frozen mid-stride, so
    /// the twin of a dragged joint lands where its own leg ISN'T, and the nearest run there is the
    /// PLANTED leg across the centre line: both knees on one leg. None on its own side → `None`,
    /// and the depth the hand gave it stands. Midline joints and axes 1/2 are untouched.
    pub fn limb_depth(
        &self,
        joint: Vec3,
        child: Option<Vec3>,
        axis: usize,
        midline: bool,
    ) -> Option<f32> {
        let axis = axis.min(2);
        // A joint within half a cell of the plane has no side of its own — either run will do.
        let side =
            (!midline && axis == 0 && joint.x.abs() >= 0.5 * self.cell).then(|| joint.x.signum());
        let run = self.nearest_run(joint, axis, side)?;
        if let Some(mid) = self.limb_mid(joint, axis, run) {
            return Some(mid);
        }
        // A BODY run. The symmetry plane is the one answer the body itself can give.
        if midline && axis == 0 {
            return Some(0.5 * (run.0 + run.1));
        }
        let child = child?;
        LIMB_STEPS.iter().find_map(|&f| {
            let s = joint.lerp(child, f);
            self.limb_mid(s, axis, self.nearest_run(s, axis, side)?)
        })
    }

    /// The inscribed radius at `p` in cm (0 outside the flesh).
    pub fn radius_at(&self, p: Vec3) -> f32 {
        self.cell_of(p).map_or(0.0, |c| self.radius[self.at(c)])
    }

    /// THE TRUNK CORE — the cells whose inscribed radius is at least [`CORE_FRACTION`] of the
    /// body's largest: the thick middle of a body, with the limbs, the neck, the tail and the
    /// ears left out (they are all thinner than half the barrel). The one read that says WHERE
    /// THE BODY IS before any joint is placed on it (incident D81498B7 — the composed rest used
    /// to land on the bounding box instead, so a horse's withers floated above its back).
    ///
    /// `plane_x` is the core's mean x — the body's SYMMETRY PLANE, the line the rig's midline
    /// belongs on; a mean rather than a bbox centre so one raised limb or a turned head cannot
    /// drag it. `None` on a hollow or empty field (nothing solid), which is a caller's cue to
    /// leave the rest alone rather than move it somewhere invented (4BB12A75).
    pub fn core(&self) -> Option<Core> {
        let radius = self.radius.iter().copied().fold(0.0_f32, f32::max);
        if radius <= 0.0 {
            return None;
        }
        let cut = CORE_FRACTION * radius;
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let (mut sum_x, mut sum_y, mut n) = (0.0_f64, 0.0_f64, 0_usize);
        // The core's own cells, kept so the horizontal AXIS below is a second moment of the body
        // and not of its bounding box.
        let mut cells: Vec<[f32; 2]> = Vec::new();
        for z in 0..self.dims[2] {
            for y in 0..self.dims[1] {
                for x in 0..self.dims[0] {
                    let c = [x, y, z];
                    if self.radius[self.at(c)] < cut {
                        continue;
                    }
                    let p = self.centre(c);
                    lo = lo.min(p);
                    hi = hi.max(p);
                    sum_x += p.x as f64;
                    sum_y += p.y as f64;
                    cells.push([p.x, p.y]);
                    n += 1;
                }
            }
        }
        if n == 0 {
            return None;
        }
        // THE BODY'S LONG HORIZONTAL AXIS: the principal direction of the core's XY scatter.
        // `0.5·atan2(2·Sxy, Sxx − Syy)` is the major axis of the 2×2 covariance, which lands in
        // (−90°, 90°] — an axis, with no front or back to it.
        let (mx, my) = (sum_x / n as f64, sum_y / n as f64);
        let (mut sxx, mut syy, mut sxy) = (0.0_f64, 0.0_f64, 0.0_f64);
        for c in &cells {
            let (dx, dy) = (c[0] as f64 - mx, c[1] as f64 - my);
            sxx += dx * dx;
            syy += dy * dy;
            sxy += dx * dy;
        }
        let rad = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let (sin, cos) = (rad.sin() as f32, rad.cos() as f32);
        // The extents the cells actually reach along that axis and across it — a scatter's second
        // moment says which way, the extents say how far.
        let (mut a_lo, mut a_hi, mut c_lo, mut c_hi) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for c in &cells {
            let (dx, dy) = (c[0] - mx as f32, c[1] - my as f32);
            let (a, b) = (dx * cos + dy * sin, -dx * sin + dy * cos);
            a_lo = a_lo.min(a);
            a_hi = a_hi.max(a);
            c_lo = c_lo.min(b);
            c_hi = c_hi.max(b);
        }
        Some(Core {
            plane_x: (sum_x / n as f64) as f32,
            lo,
            hi,
            radius,
            axis_deg: (rad as f32).to_degrees(),
            along: a_hi - a_lo + self.cell,
            across: c_hi - c_lo + self.cell,
        })
    }

    /// THE THIN PART `seed` SITS IN, as the mesh vertices inside it (spec 0A81088E's "thin-part
    /// grow"): flood the flesh out from the seed's cell through everything no thicker than the seed
    /// is, and stop dead at the body. A click on a hair fall takes the whole fall and not the
    /// shoulder it lies on; a click on a sleeve takes the sleeve and not the arm.
    ///
    /// "Thicker than the seed" is the seed's own inscribed radius × [`THIN_GROW`], clamped into
    /// [`THIN_MIN_CELLS`]..[`THIN_MAX_CELLS`]. THE BODY is then the mesh OPENED by a ball of that
    /// size — every cell within `thin` of a cell whose inscribed radius admits the ball. The
    /// opening is the load-bearing half: a leg's SKIN is one cell thin by the radius alone, so a
    /// bare radius test would let the flood creep over the whole figure's surface; the skin lies
    /// inside the leg's own maximal ball, and the opening blocks it with the leg. What is left is
    /// exactly the parts too thin to hold the ball.
    ///
    /// `model` must be the mesh this field was built from (the cells are its own). A seed off the
    /// grid, or outside the flesh, selects nothing.
    pub fn grow_thin(&self, model: &RawModel, seed: Vec3) -> Vec<u32> {
        let Some(start) = self.cell_of(seed) else {
            return Vec::new();
        };
        if !self.solid[self.at(start)] {
            return Vec::new();
        }
        let thin = (self.radius[self.at(start)] * THIN_GROW)
            .clamp(THIN_MIN_CELLS * self.cell, THIN_MAX_CELLS * self.cell);
        let core: Vec<bool> = self.radius.iter().map(|&r| r >= thin).collect();
        let reach = chamfer(&core, self.dims);
        let body = |i: usize| chamfer_cm(reach[i], self.cell) <= thin;

        let mut grown = vec![false; self.solid.len()];
        grown[self.at(start)] = true;
        let mut stack = vec![start];
        while let Some(c) = stack.pop() {
            for o in NEIGHBOURS {
                let n = [0, 1, 2].map(|a| c[a] as i64 + o[a]);
                if (0..3).any(|a| n[a] < 0 || n[a] >= self.dims[a] as i64) {
                    continue;
                }
                let n = [0, 1, 2].map(|a| n[a] as usize);
                let i = self.at(n);
                if grown[i] || !self.solid[i] || body(i) {
                    continue;
                }
                grown[i] = true;
                stack.push(n);
            }
        }
        model
            .vertices
            .iter()
            .enumerate()
            .filter(|(_, v)| {
                self.cell_of(Vec3::from_array(v.p))
                    .is_some_and(|c| grown[self.at(c)])
            })
            .map(|(i, _)| i as u32)
            .collect()
    }

    /// The point within `r` of `p` with the largest inscribed radius — the local MEDIAL point.
    /// `p` itself when nothing solid is within `r`.
    pub fn centre_near(&self, p: Vec3, r: f32) -> Vec3 {
        self.best(p, r, None).map_or(p, |(c, _)| c)
    }

    /// `n` samples along a→b, each re-centred within `r` OF THE LINE — sideways onto the limb's
    /// medial point, never along it: the sample keeps its own place on a→b, so the radii stay a
    /// cross-section profile and [`narrowings`] can read the joints off them. (A plain ball
    /// search would hand every sample the thickest flesh within `r`, which dilates the profile
    /// and erases exactly the minima this exists to find.)
    pub fn profile(&self, a: Vec3, b: Vec3, n: usize, r: f32) -> Vec<(Vec3, f32)> {
        let n = n.max(2);
        let len = a.distance(b);
        let along = (b - a).normalize_or_zero();
        // The slice is half a sample apart thick, and never thinner than a cell.
        let half = (0.5 * len / (n - 1) as f32).max(0.5 * self.cell);
        (0..n)
            .map(|i| {
                let p = a.lerp(b, i as f32 / (n - 1) as f32);
                self.best(p, r, Some((along, -half, half)))
                    .unwrap_or((p, 0.0))
            })
            .collect()
    }

    /// THE TUBE TRACER: from `start` along `dir`, each step re-centring a step ahead on the local
    /// medial point, so the trace follows a tube round any curl — a tail down, sideways and back
    /// under the heels. The reach comes from the flesh's own thickness (clamped to the step), so a
    /// tapering tail sheds the legs it curls past. Stops when the radius collapses, when the
    /// advance stalls (the tip), or when the arc length reaches `max_len`. Lengths in cm.
    pub fn trace(&self, start: Vec3, dir: Vec3, step: f32, max_len: f32) -> Vec<(Vec3, f32)> {
        let mut out = Vec::new();
        let mut dir = dir.normalize_or_zero();
        if step <= 0.0 || dir.length_squared() < 0.5 {
            return out;
        }
        let (mut centre, mut reach, mut arc) = (start, 2.0 * step, 0.0_f32);
        loop {
            // Only flesh at least half a step AHEAD counts: a taper's thickest flesh is the way
            // the trace came from, and the medial point there would stall it on the spot.
            let ahead = Some((dir, -0.5 * step, f32::MAX));
            let Some((c, rad)) = self.best(centre + dir * step, reach, ahead) else {
                break;
            };
            if rad < 0.5 * self.cell {
                break; // the tube collapsed: past the tip
            }
            let advance = c - centre;
            if advance.length() < 0.25 * step {
                break; // the trace no longer moves on
            }
            arc += advance.length();
            dir = advance.normalize();
            centre = c;
            out.push((c, rad));
            reach = (1.6 * rad).clamp(0.6 * step, 2.0 * step);
            if arc >= max_len {
                break;
            }
        }
        out
    }

    /// The solid cell with the largest inscribed radius within `r` of `p`, averaged over the cells
    /// that TIE for it AND JOIN THE ONE NEAREST `p`: a straight tube's axis is a plateau of equal
    /// radii, and its centroid is the axis point nearest `p`, where a single winner would be
    /// whichever cell the scan happened to reach first — but a tie set that straddles a GAP (the
    /// other leg, in the same cross-section) must not average to the middle of the gap, which is
    /// the left-offset failure the elf fit shows (note 2D31782B). So the set is walked from the
    /// nearest tied cell and only its own connected run counts.
    ///
    /// `window = (dir, lo, hi)` narrows the ball to a slice of it: how far a cell may sit along
    /// `dir` of `p` — a thin slice for a cross-section read ([`Flesh::profile`]), a forward
    /// half-space for a tracer's next step ([`Flesh::trace`]).
    fn best(&self, p: Vec3, r: f32, window: Option<(Vec3, f32, f32)>) -> Option<(Vec3, f32)> {
        if self.solid.is_empty() || r <= 0.0 {
            return None;
        }
        let lo = self.clamped(p - Vec3::splat(r));
        let hi = self.clamped(p + Vec3::splat(r));
        let (mut top, mut tied) = (0.0_f32, Vec::<[usize; 3]>::new());
        for k in lo[2]..=hi[2] {
            for j in lo[1]..=hi[1] {
                for i in lo[0]..=hi[0] {
                    let rad = self.radius[self.at([i, j, k])];
                    if rad <= 0.0 || rad < top {
                        continue;
                    }
                    let c = self.centre([i, j, k]);
                    if c.distance(p) > r {
                        continue;
                    }
                    if let Some((dir, lo, hi)) = window {
                        let t = (c - p).dot(dir);
                        if t < lo || t > hi {
                            continue;
                        }
                    }
                    if rad > top {
                        top = rad;
                        tied.clear();
                    }
                    tied.push([i, j, k]);
                }
            }
        }
        let set: HashSet<[usize; 3]> = tied.iter().copied().collect();
        let seed = *tied.iter().min_by(|&&a, &&b| {
            self.centre(a)
                .distance_squared(p)
                .total_cmp(&self.centre(b).distance_squared(p))
        })?;
        let mut seen = HashSet::from([seed]);
        let mut stack = vec![seed];
        let (mut sum, mut n) = (Vec3::ZERO, 0_u32);
        while let Some(c) = stack.pop() {
            sum += self.centre(c);
            n += 1;
            for d in NEIGHBOURS {
                let nb = [0, 1, 2].map(|a| c[a] as i64 + d[a]);
                if nb.iter().any(|&v| v < 0) {
                    continue;
                }
                let nb = [nb[0] as usize, nb[1] as usize, nb[2] as usize];
                if set.contains(&nb) && seen.insert(nb) {
                    stack.push(nb);
                }
            }
        }
        (n > 0).then(|| (sum / n as f32, top))
    }

    /// The run on `axis` containing `p`'s own coordinate there, else the one NEAREST it — the one
    /// run walk [`Self::depth_at`] and [`Self::limb_depth`] both read. `side` (the sign of a sided
    /// joint's x, `None` for no preference) drops every run that does not REACH that side of
    /// X = 0 — the same-side guard; a run straddling the plane reaches both. `None` on an empty
    /// column, and on a column whose only flesh is across the plane.
    fn nearest_run(&self, p: Vec3, axis: usize, side: Option<f32>) -> Option<(f32, f32)> {
        let x = comp(p, axis.min(2));
        let gap = |&(lo, hi): &(f32, f32)| (lo - x).max(x - hi).max(0.0);
        let half = 0.5 * self.cell;
        self.runs(p, axis)
            .into_iter()
            .filter(|&(lo, hi)| match side {
                Some(s) if s > 0.0 => hi >= -half,
                Some(_) => lo <= half,
                None => true,
            })
            .min_by(|a, b| gap(a).total_cmp(&gap(b)))
    }

    /// The run's midpoint when the run is a LIMB and not the body — measured where the run's own
    /// middle is, so a limb read across its axis spans about two inscribed radii and a torso many.
    /// `None` for a body run, and for a sliver with no radius at all.
    fn limb_mid(&self, p: Vec3, axis: usize, run: (f32, f32)) -> Option<f32> {
        let mid = 0.5 * (run.0 + run.1);
        let mut q = p;
        q[axis] = mid;
        let r = self.radius_at(q);
        (r > 0.0 && run.1 - run.0 <= LIMB_RATIO * r).then_some(mid)
    }

    fn at(&self, c: [usize; 3]) -> usize {
        c[0] + self.dims[0] * (c[1] + self.dims[1] * c[2])
    }

    /// THE RAW FIELD — occupancy, the inscribed radius per cell, and the grid's dimensions, all
    /// in the ONE linear indexing `x + w·(y + h·z)`. The door [`crate::shape`] thins through: a
    /// curve skeleton is a whole-grid pass, and going through [`Flesh::contains`] per cell would
    /// re-resolve a position that is already an index.
    pub(crate) fn field(&self) -> (&[bool], &[f32], [usize; 3]) {
        (&self.solid, &self.radius, self.dims)
    }

    /// The world centre of a linear cell index of [`Flesh::field`].
    pub(crate) fn centre_of(&self, i: usize) -> Vec3 {
        let (w, h) = (self.dims[0], self.dims[1]);
        self.centre([i % w, (i / w) % h, i / (w * h)])
    }

    /// The column through `p` along `axis` — the cell coordinates on the OTHER two axes, so a
    /// joint hanging off the end of the body still reads the column it stands in. `None` when `p`
    /// is outside the grid on either of those two axes.
    fn column(&self, p: Vec3, axis: usize) -> Option<[usize; 3]> {
        let q = (p - self.origin) / self.cell;
        let c = [0, 1, 2].map(|a| if a == axis { 0.0 } else { comp(q, a).floor() });
        (0..3)
            .all(|a| a == axis || (c[a] >= 0.0 && c[a] < self.dims[a] as f32))
            .then(|| [0, 1, 2].map(|a| c[a] as usize))
    }

    /// The cell containing `p`, or `None` when `p` is outside the grid.
    fn cell_of(&self, p: Vec3) -> Option<[usize; 3]> {
        let q = (p - self.origin) / self.cell;
        let c = [0, 1, 2].map(|a| comp(q, a).floor());
        (0..3)
            .all(|a| c[a] >= 0.0 && c[a] < self.dims[a] as f32)
            .then(|| [0, 1, 2].map(|a| c[a] as usize))
    }

    /// `p`'s cell coordinates clamped into the grid (an empty grid clamps to 0, which the callers
    /// guard against before they index).
    fn clamped(&self, p: Vec3) -> [usize; 3] {
        let q = (p - self.origin) / self.cell;
        [0, 1, 2]
            .map(|a| (comp(q, a).floor().max(0.0) as usize).min(self.dims[a].saturating_sub(1)))
    }

    fn centre(&self, c: [usize; 3]) -> Vec3 {
        self.origin + Vec3::new(c[0] as f32 + 0.5, c[1] as f32 + 0.5, c[2] as f32 + 0.5) * self.cell
    }

    /// The cm extent of cells `a..=b` along `axis` (cell boundaries, not centres).
    fn span(&self, axis: usize, a: usize, b: usize) -> (f32, f32) {
        let o = comp(self.origin, axis);
        (o + a as f32 * self.cell, o + (b + 1) as f32 * self.cell)
    }
}

/// A WAIST's flesh must thicken again within this many of the waist's OWN radii on each side
/// ([`narrowings`]), and a thin run LONGER than that is a SHAFT — a bone, not a joint: a long
/// straight cannon is thin along its whole length and thickens only at its two ends, which is
/// where its joints are. Read as one waist it put a joint halfway down the Moose's cannon.
const WAIST_RADII: f32 = 2.0;

/// THE WAISTS of a limb — the joints its RADIUS states: the indices of the local minima of the
/// radius in a `profile` whose flesh rises again ON BOTH SIDES, WITHIN [`WAIST_RADII`] of the
/// minimum's own radius (never less than two cells), by at least 10 % of the higher flesh and at
/// least half a `cell` (the grain the radii are read at: a third of a cell is one voxel step).
/// The two end samples are ignored. A muscle bulge is a maximum and never qualifies (incident
/// D9D837FF: reading extremes put the lizard's knee on the quadriceps); a narrowing on a straight
/// run whose radius keeps falling on one side is never a waist. The radii are read through a
/// three-sample mean first, so one voxel's step is not a waist either.
///
/// A minimum is a PLATEAU of equal radius, and the rise is measured from the plateau's two
/// EDGES. A plateau no longer than the reach is a WAIST and reports one index, its middle; a
/// longer one is a SHAFT and reports its two ENDS, where the flesh thickens into the joints the
/// bone runs between — never its middle (Aaron on the Moose, 2026-09-29: *"there's still a bone
/// joint in the middle of the lower leg"*). A shallower dip beside a deeper one is measured
/// against the bump between them, so it falls to the prominence floor rather than doubling up.
pub fn narrowings(profile: &[(Vec3, f32)], cell: f32) -> Vec<usize> {
    const PROMINENCE: f32 = 0.10;
    const EQ: f32 = 1e-4;
    let n = profile.len();
    if n < 5 {
        return Vec::new();
    }
    let mut arc = vec![0.0_f32; n];
    for i in 1..n {
        arc[i] = arc[i - 1] + profile[i - 1].0.distance(profile[i].0);
    }
    let smooth: Vec<f32> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
            profile[a..=b].iter().map(|s| s.1).sum::<f32>() / (b - a + 1) as f32
        })
        .collect();
    let r = |i: usize| smooth[i];
    let mut out = Vec::new();
    let mut i = 1;
    while i < n - 1 {
        // The maximal plateau [i..=j] of equal radius, and whether the profile descends INTO it
        // from both sides — a single sample is a plateau of one.
        let mut j = i;
        while j + 1 < n - 1 && (r(j + 1) - r(i)).abs() <= EQ {
            j += 1;
        }
        if r(i - 1) > r(i) + EQ && r(j + 1) > r(j) + EQ && r(i) > 0.0 {
            let low = r(i);
            let reach = (WAIST_RADII * low).max(2.0 * cell);
            // The rise: walk out from the plateau's edge within the reach, keeping the highest
            // radius seen, until the profile drops BELOW this minimum (another, deeper narrowing)
            // or runs out.
            let rises = |back: bool| -> bool {
                let edge = if back { i } else { j };
                let (mut top, mut k) = (low, edge);
                loop {
                    match if back { k.checked_sub(1) } else { Some(k + 1) } {
                        Some(step) if step < n && (arc[step] - arc[edge]).abs() <= reach => {
                            k = step
                        }
                        _ => break,
                    }
                    if r(k) < low - EQ {
                        break;
                    }
                    top = top.max(r(k));
                }
                top - low >= (PROMINENCE * top).max(0.5 * cell)
            };
            if rises(true) && rises(false) {
                if arc[j] - arc[i] > reach {
                    out.extend([i, j]);
                } else {
                    out.push((i + j) / 2);
                }
            }
        }
        i = j + 1;
    }
    out
}

/// Exact triangle / axis-aligned-box overlap — the 13-axis separating-axis test (Akenine-Möller).
/// The box is `centre` ± `half` on each axis. Conservative rasterisation needs EVERY cell a
/// triangle touches: a cell the test misses is a hole, and one hole lets the exterior flood
/// swallow the whole body.
fn tri_box(tri: [Vec3; 3], centre: Vec3, half: f32) -> bool {
    let v = tri.map(|p| p - centre);
    let h = Vec3::splat(half);
    let lohi = |a: usize| {
        let (x, y, z) = (comp(v[0], a), comp(v[1], a), comp(v[2], a));
        (x.min(y).min(z), x.max(y).max(z))
    };
    // The three box normals.
    for a in 0..3 {
        let (lo, hi) = lohi(a);
        if lo > half || hi < -half {
            return false;
        }
    }
    let e = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    // The triangle's own plane.
    let nrm = e[0].cross(e[1]);
    if nrm.abs().dot(h) < nrm.dot(v[0]).abs() {
        return false;
    }
    // The nine edge × box-axis cross products.
    for edge in &e {
        for a in 0..3 {
            let mut unit = Vec3::ZERO;
            unit[a] = 1.0;
            let ax = unit.cross(*edge);
            let reach = ax.abs().dot(h);
            let (lo, hi) = {
                let (x, y, z) = (ax.dot(v[0]), ax.dot(v[1]), ax.dot(v[2]));
                (x.min(y).min(z), x.max(y).max(z))
            };
            if lo > reach || hi < -reach {
                return false;
            }
        }
    }
    true
}

/// Closed triangulated meshes built from parameters — the synthetic bodies every fit gate in the
/// crate measures against (`conform`'s leg/tail fixtures build on [`fixtures::tube`], so there is
/// ONE tube builder, not one per gate).
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::fbx::RawVertex;

    pub(crate) fn vert(p: Vec3) -> RawVertex {
        RawVertex {
            p: p.to_array(),
            n: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [0.0; 4],
        }
    }

    /// A closed box mesh (non-deduped, one vertex per corner of each triangle) — a stand-in raw
    /// character mesh.
    pub(crate) fn box_mesh(x0: f32, x1: f32, y0: f32, y1: f32, z0: f32, z1: f32) -> RawModel {
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ];
        let faces = [
            ([0usize, 1, 2, 3], [0.0, 0.0, -1.0]),
            ([4, 7, 6, 5], [0.0, 0.0, 1.0]),
            ([0, 4, 5, 1], [0.0, -1.0, 0.0]),
            ([3, 2, 6, 7], [0.0, 1.0, 0.0]),
            ([0, 3, 7, 4], [-1.0, 0.0, 0.0]),
            ([1, 5, 6, 2], [1.0, 0.0, 0.0]),
        ];
        let mut vertices = Vec::new();
        for (q, n) in faces {
            for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                for &vi in &tri {
                    let mut v = vert(Vec3::from_array(c[vi]));
                    v.n = n;
                    vertices.push(v);
                }
            }
        }
        let indices = (0..vertices.len() as u32).collect();
        RawModel {
            regions: Vec::new(),
            vertices,
            indices,
            bones: Vec::new(),
        }
    }

    /// A CLOSED tube of 12-gon rings every centimetre along the polyline `points`, the radius
    /// lerped between `radii` (one per point), with both ends capped. The rings' vertices are the
    /// mesh's vertices, so a fit that reads the vertex cloud sees the same tube the voxel field
    /// does.
    pub(crate) fn tube(points: &[Vec3], radii: &[f32]) -> RawModel {
        const K: usize = 12;
        let mut vertices: Vec<RawVertex> = Vec::new();
        let mut rings: Vec<usize> = Vec::new(); // the first vertex index of each ring
        for seg in 0..points.len().saturating_sub(1) {
            let (a, b) = (points[seg], points[seg + 1]);
            let len = a.distance(b).max(1e-3);
            let u = (b - a) / len;
            let side = u.cross(Vec3::X).normalize_or_zero();
            let side = if side.length_squared() < 0.5 {
                u.cross(Vec3::Y).normalize()
            } else {
                side
            };
            let up = side.cross(u);
            let mut d = 0.0;
            while d <= len {
                let f = d / len;
                let r = radii[seg] * (1.0 - f) + radii[seg + 1] * f;
                let c = a + u * d;
                rings.push(vertices.len());
                for k in 0..K {
                    let ang = k as f32 * std::f32::consts::TAU / K as f32;
                    vertices.push(vert(c + (side * ang.cos() + up * ang.sin()) * r));
                }
                d += 1.0;
            }
        }
        // Two triangles per quad between consecutive rings, then a fan cap at each end.
        let mut indices: Vec<u32> = Vec::new();
        for pair in rings.windows(2) {
            let (a, b) = (pair[0] as u32, pair[1] as u32);
            for k in 0..K as u32 {
                let n = (k + 1) % K as u32;
                indices.extend([a + k, b + k, b + n]);
                indices.extend([a + k, b + n, a + n]);
            }
        }
        for (ring, flip) in [
            (rings.first().copied(), false),
            (rings.last().copied(), true),
        ] {
            let Some(base) = ring.map(|r| r as u32) else {
                continue;
            };
            for k in 1..K as u32 - 1 {
                let t = [base, base + k, base + k + 1];
                indices.extend(if flip { [t[0], t[2], t[1]] } else { t });
            }
        }
        RawModel {
            regions: Vec::new(),
            vertices,
            indices,
            bones: Vec::new(),
        }
    }

    /// A LEG as the flesh really reads it: a bulge-narrow-bulge-narrow tube stacked along Z —
    /// thigh bulge 9 cm, KNEE 5, calf bulge 7, ANKLE 3.5, foot 5. The shape that tells a NARROWING
    /// read from the old extreme read, which landed on the bulges (incident D9D837FF).
    pub(crate) const BULGE_HIP: f32 = 92.0;
    pub(crate) const BULGE_KNEE: f32 = 52.0;
    pub(crate) const BULGE_ANKLE: f32 = 12.0;
    pub(crate) const BULGE_TOE: f32 = 2.0;
    /// The two bulges, which no joint may land on.
    pub(crate) const BULGE_THIGH: f32 = 74.0;
    pub(crate) const BULGE_CALF: f32 = 32.0;

    pub(crate) fn bulge_leg() -> RawModel {
        let z = |v: f32| Vec3::new(0.0, 0.0, v);
        tube(
            &[
                z(BULGE_HIP),
                z(BULGE_THIGH),
                z(BULGE_KNEE),
                z(BULGE_CALF),
                z(BULGE_ANKLE),
                z(BULGE_TOE),
            ],
            &[6.0, 9.0, 5.0, 7.0, 3.5, 5.0],
        )
    }

    /// Merge closed meshes into one model (indices rebased) — a body is several solids.
    pub(crate) fn merge(parts: Vec<RawModel>) -> RawModel {
        let mut out = RawModel {
            regions: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            bones: Vec::new(),
        };
        for p in parts {
            let base = out.vertices.len() as u32;
            out.vertices.extend(p.vertices);
            out.indices.extend(p.indices.iter().map(|i| i + base));
        }
        out
    }

    /// A box whose faces are subdivided `n`×`n` — a real triangle budget to build against
    /// (12·n² triangles).
    pub(crate) fn subdivided_box(lo: Vec3, hi: Vec3, n: usize) -> RawModel {
        let mut vertices: Vec<RawVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for side in 0..2 {
                let base = vertices.len() as u32;
                for a in 0..=n {
                    for b in 0..=n {
                        let mut p = [0.0; 3];
                        p[axis] = if side == 0 {
                            comp(lo, axis)
                        } else {
                            comp(hi, axis)
                        };
                        let f = |t: usize, ax: usize| {
                            comp(lo, ax) + (comp(hi, ax) - comp(lo, ax)) * (t as f32 / n as f32)
                        };
                        p[u] = f(a, u);
                        p[v] = f(b, v);
                        vertices.push(vert(Vec3::from_array(p)));
                    }
                }
                let w = (n + 1) as u32;
                for a in 0..n as u32 {
                    for b in 0..n as u32 {
                        let q = base + a * w + b;
                        indices.extend([q, q + 1, q + w + 1]);
                        indices.extend([q, q + w + 1, q + w]);
                    }
                }
            }
        }
        RawModel {
            regions: Vec::new(),
            vertices,
            indices,
            bones: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    /// THE MASK (spec 0A81088E): a hair blob welded onto a leg makes the leg's run read as wide as
    /// the blob — the tail-hair swamp that defeats the fit (2D31782B). Masking the blob's vertices
    /// out gives the leg its own width back, and the distance OFF the body reads the blob's own
    /// hang. Both fields come from the same build.
    #[test]
    fn a_masked_flesh_reads_the_leg_without_the_hair_on_it() {
        let leg = box_mesh(-5.0, 5.0, -5.0, 5.0, 0.0, 80.0);
        let hair = box_mesh(5.0, 30.0, -5.0, 5.0, 40.0, 75.0);
        let n_leg = leg.vertices.len();
        let both = merge(vec![leg, hair]);
        let mid = Vec3::new(0.0, 0.0, 60.0);

        let whole = Flesh::build(&both);
        let run = whole.runs(mid, 0);
        assert_eq!(run.len(), 1, "leg and hair are one run");
        assert!(
            run[0].1 - run[0].0 > 30.0,
            "the hair widens the leg's run to {:.1} cm",
            run[0].1 - run[0].0
        );

        let keep: Vec<bool> = (0..both.vertices.len()).map(|i| i < n_leg).collect();
        let body = Flesh::build_masked(&both, &keep);
        let run = body.runs(mid, 0);
        assert_eq!(run.len(), 1, "the leg alone is one run");
        let width = run[0].1 - run[0].0;
        assert!(
            (9.0..13.0).contains(&width),
            "the leg is limb-sized again: {width:.1} cm"
        );
        // The same field's OUTSIDE distance is what the split measures the hair by.
        assert_eq!(body.distance_outside(mid), 0.0, "inside the leg is rigid");
        assert!(
            body.distance_outside(Vec3::new(25.0, 0.0, 60.0)) > 15.0,
            "the far end of the hair hangs clear"
        );
    }

    /// THIN-PART GROW (spec 0A81088E, T2's "grow from click"): a 2 cm hair fall welded onto a
    /// 10 cm leg. A click in the fall takes the fall — and NOT the leg, whose one-cell skin a bare
    /// radius test would have let the flood creep all over. A click in the leg takes neither.
    #[test]
    fn a_grow_from_a_hair_fall_stops_at_the_leg_it_hangs_on() {
        let leg = box_mesh(-5.0, 5.0, -5.0, 5.0, 0.0, 80.0);
        let hair = subdivided_box(Vec3::new(5.0, -1.0, 40.0), Vec3::new(30.0, 1.0, 75.0), 4);
        let n_leg = leg.vertices.len();
        let both = merge(vec![leg, hair]);
        let f = Flesh::build(&both);

        let grown = f.grow_thin(&both, Vec3::new(20.0, 0.0, 60.0));
        assert!(!grown.is_empty(), "the fall grew something");
        assert!(
            grown.iter().all(|&v| v as usize >= n_leg),
            "every grown vertex is the fall's, not the leg's"
        );
        // What comes along is the fall CLEAR OF ITS WELD: every one of its 105 vertices past the
        // seam, and none of the 45 standing at x = 5 — those lie inside the leg's own maximal
        // ball, which is exactly where a hand-grown region's boundary belongs.
        let seam = 5.0 + f.cell();
        for (i, v) in both.vertices.iter().enumerate().skip(n_leg) {
            assert_eq!(
                grown.contains(&(i as u32)),
                v.p[0] > seam,
                "fall vertex {i} at {:?}",
                v.p
            );
        }

        // The leg is fat: a click in it grows nothing that reaches the fall.
        let inside = f.grow_thin(&both, Vec3::new(0.0, 0.0, 60.0));
        assert!(
            inside.iter().all(|&v| (v as usize) < n_leg),
            "a click in the leg never walks out along the fall"
        );
        // And a click off the mesh entirely selects nothing.
        assert!(f.grow_thin(&both, Vec3::new(0.0, 0.0, 500.0)).is_empty());
    }

    /// A SOLID BOX reads as solid: inside is flesh, outside is not, every axis' depth is the
    /// box's own middle, and a column crosses it in exactly one run with the box's extents.
    /// A FLAT FACE ON THE BOUNDING BOX still reads its own thickness. The padding clears the mesh
    /// by half a cell, so the exterior flood always has a boundary layer to start from: with the
    /// padding exactly ONE cell, a box's low faces landed on the cell 0/1 boundary, the conservative
    /// rasteriser marked the outermost layer solid, the flood never reached that side and every
    /// inscribed radius behind the face was measured to the FAR side of the body — 5 cm of wall
    /// reading 30 cm thick, which threw the trunk core's symmetry plane 8 cm off the middle of a
    /// horse fixture (incident D81498B7's alignment gate found it).
    #[test]
    fn a_face_on_the_bounding_box_still_reads_its_own_thickness() {
        let f = Flesh::build(&box_mesh(-25.0, 25.0, -55.0, 55.0, 0.0, 60.0));
        let probe = |x: f32| f.radius_at(Vec3::new(x, 0.0, 30.0));
        let (l, r) = (probe(-20.0), probe(20.0));
        assert!(
            (l - r).abs() < f.cell(),
            "both faces read the same wall: {l:.2} vs {r:.2}"
        );
        assert!(
            (l - 5.0).abs() < 2.0 * f.cell(),
            "5 cm in from a face reads about 5 cm of flesh, got {l:.2}"
        );
        let core = f.core().expect("a box has a core");
        assert!(
            core.plane_x.abs() < f.cell(),
            "the core centres on the box's own middle, got {:.2}",
            core.plane_x
        );
    }

    #[test]
    fn a_solid_box_is_flesh_with_one_run_per_column() {
        let (lo, hi) = (Vec3::new(-20.0, -10.0, 0.0), Vec3::new(20.0, 10.0, 120.0));
        let f = Flesh::build(&box_mesh(lo.x, hi.x, lo.y, hi.y, lo.z, hi.z));
        let mid = (lo + hi) * 0.5;
        assert!(f.contains(mid), "the box's middle is flesh");
        assert!(
            f.contains(Vec3::new(18.0, 8.0, 110.0)),
            "near a corner, inside"
        );
        assert!(
            !f.contains(mid + Vec3::new(0.0, 40.0, 0.0)),
            "outside is not flesh"
        );
        assert!(
            !f.contains(Vec3::new(0.0, 0.0, 150.0)),
            "above is not flesh"
        );
        for axis in 0..3 {
            let runs = f.runs(mid, axis);
            assert_eq!(runs.len(), 1, "one run along axis {axis}, got {runs:?}");
            let (a, b) = runs[0];
            assert!(
                (a - comp(lo, axis)).abs() <= f.cell() && (b - comp(hi, axis)).abs() <= f.cell(),
                "axis {axis} run {a}..{b} is the box's extent within a cell"
            );
            let d = f.depth_at(mid, axis, 5.0).expect("a depth on every axis");
            assert!(
                (d - comp(mid, axis)).abs() <= f.cell(),
                "axis {axis} depth {d} is the box's middle {}",
                comp(mid, axis)
            );
        }
        let far = f
            .depth_at(mid + Vec3::new(200.0, 0.0, 0.0), 0, 250.0)
            .expect("the column is the other two axes, not p's own coordinate on it");
        assert!(
            (far - comp(mid, 0)).abs() <= f.cell(),
            "the nearest run's middle, got {far}"
        );
        assert!(
            f.depth_at(mid + Vec3::new(200.0, 0.0, 0.0), 0, 5.0)
                .is_none(),
            "... but a joint further than the tolerance from any run has no depth"
        );
        assert!(
            f.depth_at(Vec3::new(0.0, 80.0, 60.0), 0, 5.0).is_none(),
            "a column that misses the box has no depth"
        );
    }

    /// TWO LEGS: a LEFT-view column (along X) at knee height crosses BOTH legs, and `depth_at`
    /// resolves the joint's OWN leg — the middle of its own tube, never the gap between the legs
    /// (the whole point of the ortho-drag depth read, spec 76EB9552).
    #[test]
    fn a_two_leg_body_resolves_the_joints_own_leg() {
        let legs = merge(vec![
            box_mesh(6.0, 16.0, -6.0, 6.0, 0.0, 80.0),
            box_mesh(-16.0, -6.0, -6.0, 6.0, 0.0, 80.0),
            box_mesh(-18.0, 18.0, -8.0, 8.0, 80.0, 105.0),
        ]);
        let f = Flesh::build(&legs);
        let knee = Vec3::new(11.0, 0.0, 45.0);
        let runs = f.runs(knee, 0);
        assert_eq!(
            runs.len(),
            2,
            "a knee-height column crosses two legs, got {runs:?}"
        );
        let d = f.depth_at(knee, 0, 6.0).expect("the left leg's own run");
        assert!(
            (d - 11.0).abs() <= f.cell(),
            "depth_at resolves the LEFT leg's middle (11), got {d}"
        );
        let d = f
            .depth_at(Vec3::new(-11.0, 0.0, 45.0), 0, 6.0)
            .expect("the right leg");
        assert!(
            (d + 11.0).abs() <= f.cell(),
            "and the right leg's (−11), got {d}"
        );
        let pelvis = Vec3::new(0.0, 0.0, 95.0);
        assert_eq!(
            f.runs(pelvis, 0).len(),
            1,
            "at pelvis height the column is one run"
        );
        let d = f.depth_at(pelvis, 0, 6.0).expect("the pelvis run");
        assert!(
            d.abs() <= f.cell(),
            "the pelvis' middle is the centre line, got {d}"
        );
    }

    /// The two-leg body of the gate above, plus a pelvis a hip can stand in.
    fn biped() -> RawModel {
        merge(vec![
            box_mesh(6.0, 16.0, -6.0, 6.0, 0.0, 80.0),
            box_mesh(-16.0, -6.0, -6.0, 6.0, 0.0, 80.0),
            box_mesh(-18.0, 18.0, -8.0, 8.0, 80.0, 105.0),
        ])
    }

    /// A QUADRUPED: a long low body (90 cm nose-to-tail) on four legs — the shape whose FRONT-view
    /// column IS the whole body length, where a run's midpoint is mid-body and nowhere near a hip.
    fn quadruped() -> RawModel {
        let mut parts = vec![box_mesh(-12.0, 12.0, -45.0, 45.0, 60.0, 90.0)];
        for x in [6.0_f32, -14.0] {
            for y in [-36.0_f32, 28.0] {
                parts.push(box_mesh(x, x + 8.0, y, y + 8.0, 0.0, 60.0));
            }
        }
        merge(parts)
    }

    /// DEPTH BY WHAT THE RUN IS (ruling F9F728CA), on a biped in the LEFT view. A hip stands in the
    /// PELVIS, where the two legs have merged into one run: its midpoint is the midline, and both
    /// hips would collapse onto it. `limb_depth` walks down the hip's own leg instead and takes the
    /// LIMB's midpoint — each hip over its own leg hole. A knee, already in its own limb run, is
    /// unchanged; a MIDLINE joint on the symmetry axis takes the body run's midpoint; an empty
    /// column answers nothing at all.
    #[test]
    fn a_hip_takes_its_depth_from_the_leg_below_it_not_the_pelvis_middle() {
        let f = Flesh::build(&biped());
        let cell = f.cell();
        // (a) the hip above the crotch, LEFT view (hidden axis X), its child down its own leg.
        for side in [1.0_f32, -1.0] {
            let hip = Vec3::new(11.0 * side, 0.0, 85.0);
            let knee = Vec3::new(11.0 * side, 0.0, 40.0);
            let old = f.depth_at(hip, 0, 40.0).expect("the pelvis run");
            assert!(
                old.abs() <= cell,
                "the run's own midpoint is the midline — the bug the ruling names: {old}"
            );
            let d = f
                .limb_depth(hip, Some(knee), 0, false)
                .expect("the leg below the hip");
            assert!(
                (d - 11.0 * side).abs() <= 2.0 * cell,
                "the hip lands over its own leg ({}), got {d}",
                11.0 * side
            );
        }
        // (c) a knee is already in its own limb run — today's answer, unchanged.
        let knee = Vec3::new(11.0, 0.0, 45.0);
        let d = f.limb_depth(knee, None, 0, false).expect("the knee's leg");
        assert!(
            (d - f.depth_at(knee, 0, 40.0).unwrap()).abs() < 1e-4,
            "a limb run still reads as `depth_at` does, got {d}"
        );
        // (d) a MIDLINE joint on axis 0 takes the body run's midpoint: the symmetry plane.
        let spine = Vec3::new(0.0, 0.0, 95.0);
        let d = f
            .limb_depth(spine, None, 0, true)
            .expect("the symmetry plane");
        assert!(d.abs() <= cell, "the midline is 0, got {d}");
        // (f) an empty column answers nothing — the hand is the only authority off the mesh.
        assert!(
            f.limb_depth(Vec3::new(0.0, 80.0, 45.0), None, 0, false)
                .is_none(),
            "a column that misses the body has no depth"
        );
    }

    /// THE HORSE HIP (ruling F9F728CA's own report): in the FRONT view the hidden axis runs
    /// nose-to-tail, so the column through the hip is the WHOLE body length and its midpoint is
    /// mid-body — visibly wrong. The hip takes its leg's Y instead. A midline spine joint in the
    /// same view has no limb to borrow from and KEEPS its depth rather than sliding to mid-body.
    #[test]
    fn a_quadrupeds_hip_reads_its_leg_in_the_front_view_and_the_spine_keeps_its_depth() {
        let f = Flesh::build(&quadruped());
        let cell = f.cell();
        // (b) the hind-left hip: the body run's midpoint is 0, the leg's is −32.
        let hip = Vec3::new(10.0, -32.0, 65.0);
        let old = f.depth_at(hip, 1, 100.0).expect("the body run");
        assert!(old.abs() <= 2.0 * cell, "the body's middle is 0, got {old}");
        let d = f
            .limb_depth(hip, Some(Vec3::new(10.0, -32.0, 30.0)), 1, false)
            .expect("the leg below the hip");
        assert!(
            (d + 32.0).abs() <= 2.0 * cell,
            "the hip lands over its own leg (−32), got {d}"
        );
        // (e) a midline spine joint whose child is the next spine joint never leaves the body run.
        let spine = Vec3::new(0.0, 0.0, 75.0);
        assert!(
            f.limb_depth(spine, Some(Vec3::new(0.0, 10.0, 75.0)), 1, true)
                .is_none(),
            "a body run with no limb anywhere along the child keeps the joint's own depth"
        );
    }

    /// A MID-STRIDE biped — the pose every creature source is frozen in: the RIGHT leg planted, the
    /// LEFT raised 20 cm. Below the raised foot, the only flesh in the column is the other leg's.
    fn mid_stride() -> RawModel {
        merge(vec![
            box_mesh(6.0, 16.0, -6.0, 6.0, 20.0, 80.0), // LEFT leg, raised
            box_mesh(-16.0, -6.0, -6.0, 6.0, 0.0, 80.0), // RIGHT leg, planted
            box_mesh(-18.0, 18.0, -8.0, 8.0, 80.0, 105.0),
        ])
    }

    /// THE SAME-SIDE GUARD (ruling 42AB9BA8, the trap 38EA5048). With Symmetry on, the twin of a
    /// dragged joint is placed at the mirrored spot — and on a mid-stride body the raised leg is
    /// NOT there. The unguarded read takes the nearest run, which is the PLANTED leg across the
    /// centre line: both knees on one leg. A sided joint now considers only the runs that reach its
    /// OWN side of X = 0 and keeps its depth when there are none. Its own leg present, nothing
    /// changes.
    #[test]
    fn a_sided_joint_never_takes_its_depth_from_the_flesh_across_the_centre_line() {
        let f = Flesh::build(&mid_stride());
        let cell = f.cell();
        // A LEFT knee dragged to where the raised leg USED to hang: only the right leg is there.
        let knee = Vec3::new(11.0, 0.0, 10.0);
        let wrong = f.depth_at(knee, 0, 40.0).expect("the right leg's run");
        assert!(
            (wrong + 11.0).abs() <= 2.0 * cell,
            "the unguarded read crosses to the planted leg (−11) — the trap: got {wrong}"
        );
        assert!(
            f.limb_depth(knee, Some(Vec3::new(11.0, 0.0, 2.0)), 0, false)
                .is_none(),
            "no flesh on the left of the plane — the hand's depth stands"
        );
        // The ordinary case is untouched: where its own leg IS, the knee takes its own leg's x.
        let knee = Vec3::new(11.0, 0.0, 45.0);
        let d = f.limb_depth(knee, None, 0, false).expect("its own leg");
        assert!(
            (d - 11.0).abs() <= 2.0 * cell,
            "the knee stays over its own leg (11), got {d}"
        );
        // A MIDLINE joint is untouched — the body run straddles the plane and answers as before.
        let spine = Vec3::new(0.0, 0.0, 95.0);
        assert!(
            f.limb_depth(spine, None, 0, true)
                .is_some_and(|d| d.abs() <= cell),
            "the midline still resolves to the symmetry plane"
        );
    }

    /// A TILTED TUBE: the medial point pulls a position 40 % of the radius off the axis back ONTO
    /// the axis, and the inscribed radius on the axis is the tube's own radius.
    #[test]
    fn the_medial_point_of_a_tilted_tube_is_its_axis() {
        let (a, b, r) = (
            Vec3::new(-20.0, -10.0, 10.0),
            Vec3::new(20.0, 15.0, 90.0),
            8.0,
        );
        let f = Flesh::build(&tube(&[a, b], &[r, r]));
        let u = (b - a).normalize();
        let side = u.cross(Vec3::Z).normalize();
        let on_axis = a.lerp(b, 0.5);
        let off = on_axis + side * (0.4 * r);
        let c = f.centre_near(off, 2.0 * r);
        let away = (c - a) - u * (c - a).dot(u);
        assert!(
            away.length() <= 0.5 * f.cell(),
            "centre_near lands on the axis, {:.2} cm off (cell {:.2})",
            away.length(),
            f.cell()
        );
        let got = f.radius_at(on_axis);
        assert!(
            (got - r).abs() <= f.cell(),
            "the inscribed radius on the axis is the tube's {r}, got {got:.2}"
        );
        assert_eq!(f.radius_at(a - u * 20.0), 0.0, "no radius outside the tube");
    }

    /// THE NARROWINGS ARE THE JOINTS (incident D9D837FF): a bulge-narrow-bulge-narrow leg —
    /// thigh bulge 9, KNEE 5, calf bulge 7, ANKLE 3.5, foot 5 — yields exactly two narrowings, at
    /// the knee and the ankle, and never on a bulge.
    #[test]
    fn narrowings_find_the_knee_and_the_ankle_not_the_bulges() {
        let (knee_z, ankle_z) = (BULGE_KNEE, BULGE_ANKLE);
        let z = |v: f32| Vec3::new(0.0, 0.0, v);
        let f = Flesh::build(&bulge_leg());
        let prof = f.profile(z(BULGE_HIP), z(BULGE_TOE), 60, 12.0);
        let found = narrowings(&prof, f.cell());
        let at: Vec<f32> = found.iter().map(|&i| prof[i].0.z).collect();
        assert_eq!(
            found.len(),
            2,
            "exactly two narrowings, got {at:?} of {:?}",
            prof.iter().map(|(_, r)| *r).collect::<Vec<_>>()
        );
        assert!(
            (at[0] - knee_z).abs() < 2.0,
            "the first narrowing is the KNEE ({knee_z}), got {}",
            at[0]
        );
        assert!(
            (at[1] - ankle_z).abs() < 2.0,
            "the second is the ANKLE ({ankle_z}), got {}",
            at[1]
        );
        for bulge in [BULGE_THIGH, BULGE_CALF] {
            assert!(
                at.iter().all(|&v| (v - bulge).abs() > 8.0),
                "no narrowing on the bulge at {bulge}, got {at:?}"
            );
        }
    }

    /// THE BUILD IS AFFORDABLE: a body-proportioned mesh at the pipeline's own triangle budget
    /// (150 000 tris — the decimate target every promoted body is baked at) voxelises well inside
    /// a second, in a `cargo test` build with no release optimisation.
    #[test]
    fn a_150k_triangle_body_voxelises_in_under_a_second() {
        let m = subdivided_box(
            Vec3::new(-30.0, -15.0, 0.0),
            Vec3::new(30.0, 15.0, 170.0),
            112,
        );
        let tris = m.indices.len() / 3;
        assert!(tris >= 150_000, "a real triangle budget, got {tris}");
        let t0 = std::time::Instant::now();
        let f = Flesh::build(&m);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!(
            "Flesh::build: {tris} tris in {ms:.0} ms (cell {:.2} cm)",
            f.cell()
        );
        assert!(f.contains(Vec3::new(0.0, 0.0, 85.0)), "the body is solid");
        assert!(ms < 500.0, "the build took {ms:.0} ms");
    }
}
