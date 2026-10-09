//! SHAPE GRAPH — a mesh's own STRUCTURE, in a vocabulary with no anatomy in it (spec 04803E0C
//! under Aaron's law 513E5F78: *"I might make a creature that has seven torsos and 28 arms and
//! four tails and six wings. There's no way to know what it is that I'm going to be rigging"*).
//!
//! The body-masked [`Flesh`] is thinned to a ONE-CELL-WIDE CURVE SKELETON, that curve is walked
//! into a graph of junctions and the paths between them, and every path is classified by nothing
//! but its own thickness and where gravity and the symmetry plane put it:
//!
//! - [`Core`] — a path at least [`CORE_FRACTION`] as thick as the body's thickest flesh. A barrel,
//!   a torso, a skull. Cores joined end to end through a thin path are a CHAIN (a neck between a
//!   barrel and a head; seven torsos with waists between them), and that chain is what the recipe's
//!   trunks and mounts are matched against.
//! - [`Limb`] — everything else that hangs off ONE core: the tree of thin paths rooted at a single
//!   attachment, with its LEAD (the path out to the end reaching farthest from the attachment)
//!   and the rest of its fan. A leg, an arm, a tail, a wing, a trunk-that-is-a-nose — the graph
//!   does not know or care. A thin component that stands on the floor on BOTH sides of the plane
//!   is two limbs that met under the body, and is cut at the plane into one limb per side.
//! - A limb whose fan is BROAD and THIN is a [`Limb::sheet`]: a spread wing thins to a fan of
//!   curves, never to one tube (Aaron's amendment, ruling 0EE33D8F).
//! - [`Pair`] — two limbs on opposite sides of the body's symmetry plane attached within
//!   [`PAIR_ARC`] of each other along the same core. A shoulder pair, a hip pair, a pair of wings.
//!
//! Nothing here is measured in "hip height" or "a fraction of the stature": every threshold is a
//! fraction of the BODY'S OWN largest inscribed radius, of a core's own arc length, or of the
//! grid's cell. That is what lets the same walk read a horse, a bat and a seven-trunk box monster.
//!
//! The thinning is DISTANCE-ORDERED HOMOTOPIC THINNING: border cells are deleted in order of
//! increasing inscribed radius, a cell only when deleting it changes no topology (a SIMPLE POINT —
//! Bertrand's two conditions, evaluated on the 3×3×3 neighbourhood as bit masks) and never when it
//! is already the end of a curve. That is the standard curve-skeleton guarantee — the skeleton is
//! homotopy-equivalent to the body, centred by the distance order, and one cell wide.

use std::collections::HashSet;
use std::time::Instant;

use glam::Vec3;

use crate::flesh::Flesh;

/// A path is a CORE when its median inscribed radius is at least this fraction of the body's
/// largest. The same half-the-barrel cut [`Flesh::core`] uses, for the same reason: half the
/// thickest flesh leaves every limb, neck, tail and ear outside while keeping the whole trunk.
pub const CORE_FRACTION: f32 = 0.5;
/// A twig shorter than this many cells of arc is surface noise (a wrinkle, a fold, a hair's
/// stub) and is pruned off the skeleton, repeatedly, until nothing more is that short.
const PRUNE_CELLS: f32 = 2.5;
/// ...and so is one whose flesh is thinner than this many cells: below one cell the "radius" is
/// the voxel grid talking about itself.
const NOISE_CELLS: f32 = 0.9;
/// A twig is also noise when it is shorter than this many INSCRIBED RADII of the flesh it grows
/// out of. Thinning protects every tip it finds, and a box's own corners, a shoulder's bulge and
/// an ear are all tips: what tells a limb from a bump is that a limb goes somewhere the body it
/// leaves is not already — measured in the body's own thickness, so the same number reads a mouse
/// and an elephant. The matcher asks the same of a NECK (`conform::Matcher::neck`): a tube that
/// runs no further out of the trunk than this is a bump on its front cap, not a neck carrying a
/// head.
pub(crate) const SPUR_RADII: f32 = 1.5;
/// A run of one class inside a path must be at least this many samples long to split the path in
/// two — a couple of cells of thick flesh in the middle of a neck is not a second torso.
const CLASS_RUN: usize = 3;
/// Two limbs on opposite sides of the plane may pair up only when their attachments sit within
/// this much of the core's own arc length of each other — wide, because a thinned junction lands
/// where the flesh lets it and a pair's two halves are rarely found at the same cell — or when
/// their tips mirror each other WHOLE, which says the same thing better (see the pairs pass).
pub const PAIR_ARC: f32 = 0.25;
/// What actually decides a pair: one limb's tip REFLECTED across the symmetry plane must land
/// within this fraction of the shorter limb's own length of the other's tip — across the plane
/// and against gravity only, when either limb stands on the floor, because a standing pair is
/// caught mid-stride and a stride runs along the rig's forward. A forelimb and a hindlimb on the
/// same side of a barrel can sit a similar distance along it; only the mirror says which two
/// limbs are the SAME limb on opposite sides.
const MIRROR_FRACTION: f32 = 0.5;
/// A limb REACHES THE GROUND when its lead's tip is within this fraction of the body's height of
/// the lowest flesh — gravity, the one direction the world supplies.
const GROUND_BAND: f32 = 0.10;
/// A limb is a SHEET when its flesh is this many times BROADER than it is THIN, measured across
/// the limb at samples along its lead. A tube is about as wide as it is deep whatever its
/// cross-section; a spread wing is a plate (ruling 0EE33D8F). Three is well clear of an oval limb
/// and well under any membrane.
pub(crate) const SHEET_RATIO: f32 = 3.0;
/// How many cross-sections along the lead the sheet test takes, and how many must read before it
/// will say anything at all.
const SHEET_SAMPLES: usize = 9;
const SHEET_READS: usize = 3;
/// How far apart two pieces of the same wing may leave the body and still be gathered into one
/// sheet — as a fraction of the longer piece's OWN length, because a wing's seam is as long as
/// its chord and has nothing to do with how thick the trunk it leaves is.
const SHEET_GATHER: f32 = 0.5;
/// A limb counts as ON THE MIDLINE (a tail, a neck stub, a horn between the eyes) when its lead
/// strays no further off the symmetry plane than this fraction of the core's own thickness. A
/// quarter, not a half: a barrel is as thick as its legs are far apart, so half of it calls a leg
/// midline (measured on the horse fixture — four legs, no pairs).
const MIDLINE_FRACTION: f32 = 0.25;

/// The 26 cell offsets around a cell.
const NEIGHBOURS: [[i64; 3]; 26] = {
    let mut out = [[0_i64; 3]; 26];
    let (mut n, mut i) = (0, 0);
    while i < 27 {
        let (a, b, c) = (i / 9 - 1, (i / 3) % 3 - 1, i % 3 - 1);
        if a != 0 || b != 0 || c != 0 {
            out[n] = [c, b, a];
            n += 1;
        }
        i += 1;
    }
    out
};

/// Bit masks over a 3×3×3 neighbourhood indexed `(dz+1)·9 + (dy+1)·3 + (dx+1)`: which cells are
/// 6-adjacent to each cell, which are 26-adjacent, the 18-neighbourhood, and the six faces.
/// Precomputed so the simple-point test below is a handful of bit operations per cell.
const MASKS: ([u32; 27], [u32; 27], u32, u32) = {
    let (mut a6, mut a26) = ([0_u32; 27], [0_u32; 27]);
    let (mut n18, mut faces) = (0_u32, 0_u32);
    let mut k = 0;
    while k < 27 {
        let (kx, ky, kz) = (
            (k % 3) as i32 - 1,
            ((k / 3) % 3) as i32 - 1,
            (k / 9) as i32 - 1,
        );
        let l1 = kx.abs() + ky.abs() + kz.abs();
        if l1 == 1 {
            faces |= 1 << k;
        }
        if l1 == 1 || l1 == 2 {
            n18 |= 1 << k;
        }
        let mut m = 0;
        while m < 27 {
            let (mx, my, mz) = (
                (m % 3) as i32 - 1,
                ((m / 3) % 3) as i32 - 1,
                (m / 9) as i32 - 1,
            );
            let (dx, dy, dz) = ((kx - mx).abs(), (ky - my).abs(), (kz - mz).abs());
            if m != k && dx <= 1 && dy <= 1 && dz <= 1 {
                a26[k] |= 1 << m;
                if dx + dy + dz == 1 {
                    a6[k] |= 1 << m;
                }
            }
            m += 1;
        }
        k += 1;
    }
    (a6, a26, n18, faces)
};

/// The centre of a 3×3×3 neighbourhood.
const MID: usize = 13;

/// Flood `seed`'s component of `set` through `adj`, as a bit mask.
fn flood(seed: usize, set: u32, adj: &[u32; 27]) -> u32 {
    let mut seen = 1_u32 << seed;
    let mut front = seen;
    while front != 0 {
        let (mut next, mut f) = (0_u32, front);
        while f != 0 {
            let k = f.trailing_zeros() as usize;
            f &= f - 1;
            next |= adj[k] & set & !seen;
        }
        seen |= next;
        front = next;
    }
    seen
}

/// IS THIS CELL SIMPLE — can it be deleted without changing the body's topology? Bertrand's two
/// conditions on the 3×3×3 neighbourhood `nb` (bit `k` set = that cell is flesh): the flesh around
/// it is ONE 26-connected component, and the background around it has exactly ONE 6-connected
/// component inside the 18-neighbourhood that touches a face of the cell. Deleting a simple point
/// never merges two parts, never splits one, never opens or closes a hole.
fn simple(nb: u32) -> bool {
    let (a6, a26, n18, faces) = &MASKS;
    let object = nb & !(1 << MID);
    if object == 0 {
        return false; // an isolated cell IS the component — deleting it would lose a part
    }
    if flood(object.trailing_zeros() as usize, object, a26) != object {
        return false;
    }
    let mut rest = n18 & !nb & !(1 << MID);
    let mut touching = 0;
    while rest != 0 {
        let seen = flood(rest.trailing_zeros() as usize, rest, a6);
        if seen & faces != 0 {
            touching += 1;
            if touching > 1 {
                return false;
            }
        }
        rest &= !seen;
    }
    touching == 1
}

/// IS THIS CELL THE END OF A CURVE — the one thing thinning must never delete, or a limb unravels
/// from its tip inward, pass after pass, until the whole limb is gone (measured: a biped fixture
/// lost both legs and its lower torso and kept only the line across its shoulders).
///
/// A cell is an end when its flesh neighbours are a CLIQUE — all mutually 26-adjacent, so the cell
/// caps them rather than joining two ways. One neighbour is the straight case; a curve that kinks
/// diagonally ends on a cell with TWO neighbours that touch each other, which the bare
/// count-the-neighbours test calls an interior cell and throws away.
fn curve_end(nb: u32) -> bool {
    let (_, a26, ..) = &MASKS;
    let object = nb & !(1 << MID);
    if object.count_ones() <= 1 {
        return true;
    }
    let mut f = object;
    while f != 0 {
        let k = f.trailing_zeros() as usize;
        f &= f - 1;
        if object & !(1 << k) & !a26[k] != 0 {
            return false;
        }
    }
    true
}

/// ONE CORE of the body — a chain of thick flesh, oriented REAR/BOTTOM → FRONT/TOP.
///
/// Which end is the front is decided by the core this one is CHAINED to through a thin path (a
/// head is a small core on the end of a neck — the front is the end nearer it); with no chained
/// core nothing measured says which end is which, and the rig's own forward decides, which is the
/// human's facing knob (69F4B20D, 3A61D440).
#[derive(Debug, Clone, PartialEq)]
pub struct Core {
    /// The centreline, rear/bottom first.
    pub path: Vec<Vec3>,
    /// The inscribed radius at each point of [`Core::path`].
    pub radii: Vec<f32>,
    /// The arc length of the whole path (cm).
    pub arc: f32,
    /// The median inscribed radius along it (cm) — how fat this core is.
    pub radius: f32,
    /// Does this core stand UP? Its own axis against gravity: a biped's torso is upright, a
    /// barrel is not. The recipe's [`flicker_skeletal::format::Orientation`] must agree with it.
    pub upright: bool,
}

impl Core {
    /// The REAR (a lying core) or BOTTOM (an upright one) end.
    pub fn rear(&self) -> Vec3 {
        self.path.first().copied().unwrap_or(Vec3::ZERO)
    }

    /// The FRONT (a lying core) or TOP (an upright one) end.
    pub fn front(&self) -> Vec3 {
        self.path.last().copied().unwrap_or(Vec3::ZERO)
    }

    /// The point a fraction `t` along the core, 0 at the rear/bottom and 1 at the front/top,
    /// measured in ARC LENGTH so an S-bent back divides evenly.
    pub fn at(&self, t: f32) -> Vec3 {
        let cum = arcs(&self.path);
        let want = t.clamp(0.0, 1.0) * self.arc;
        for i in 1..self.path.len() {
            if cum[i] >= want {
                let seg = (cum[i] - cum[i - 1]).max(1e-6);
                return self.path[i - 1].lerp(self.path[i], (want - cum[i - 1]) / seg);
            }
        }
        self.front()
    }

    /// The core's own inscribed radius a fraction `t` along it — how thick the body is there.
    pub fn radius_at(&self, t: f32) -> f32 {
        let i = ((t.clamp(0.0, 1.0) * (self.radii.len().max(1) - 1) as f32).round() as usize)
            .min(self.radii.len().saturating_sub(1));
        self.radii.get(i).copied().unwrap_or(0.0)
    }

    /// Where `p` sits along the core, 0 at the rear/bottom and 1 at the front/top.
    pub(crate) fn t_of(&self, p: Vec3) -> f32 {
        let cum = arcs(&self.path);
        let mut best = (f32::MAX, 0.0_f32);
        for (i, q) in self.path.iter().enumerate() {
            let d = q.distance_squared(p);
            if d < best.0 {
                best = (d, cum[i] / self.arc.max(1e-6));
            }
        }
        best.1.clamp(0.0, 1.0)
    }
}

/// ONE LIMB — the whole tree of thin paths hanging off a core at ONE attachment. A leg, an arm, a
/// tail, a wing, a horn, a trunk-that-is-a-nose: the graph says where it attaches, which way it
/// goes, how far it reaches, which side of the plane it is on and whether it touches the ground,
/// and nothing else.
#[derive(Debug, Clone, PartialEq)]
pub struct Limb {
    /// The core it hangs off.
    pub core: usize,
    /// Where it leaves that core (world cm).
    pub at: Vec3,
    /// That attachment's position ALONG the core, 0 at the rear/bottom, 1 at the front/top.
    pub t: f32,
    /// Which side of the symmetry plane the limb's own flesh lies on: +1, −1, or 0 on the midline.
    /// A limb inside the midline band that STANDS and pairs with a standing twin across the plane
    /// (a leg stepping in under the body) is on the side it paired from.
    pub side: f32,
    /// A BROAD, THIN fan rather than a tube — a spread wing (ruling 0EE33D8F).
    pub sheet: bool,
    /// THE LEAD: the path from the attachment out to the end of the tree that reaches FARTHEST
    /// from it (the end on the floor instead, for a tube whose far end hangs just above it). A
    /// tube's whole length; a sheet's leading edge.
    pub lead: Vec<Vec3>,
    /// The inscribed radius at each point of [`Limb::lead`].
    pub lead_r: Vec<f32>,
    /// The lead's arc length (cm).
    pub arc: f32,
    /// The rest of the fan: every other attachment-to-end path with its radii, longest first.
    pub fan: Vec<(Vec<Vec3>, Vec<f32>)>,
    /// Does the lead END on the floor? The one thing gravity decides.
    pub grounded: bool,
}

impl Limb {
    /// The far end of the lead.
    pub fn tip(&self) -> Vec3 {
        self.lead.last().copied().unwrap_or(self.at)
    }

    /// The point a fraction `t` along the lead, in arc length from the attachment.
    pub fn at_lead(&self, t: f32) -> Vec3 {
        let cum = arcs(&self.lead);
        let want = t.clamp(0.0, 1.0) * self.arc;
        for i in 1..self.lead.len() {
            if cum[i] >= want {
                let seg = (cum[i] - cum[i - 1]).max(1e-6);
                return self.lead[i - 1].lerp(self.lead[i], (want - cum[i - 1]) / seg);
            }
        }
        self.tip()
    }
}

/// A thin path joining TWO cores — the CHAIN the recipe's mounted trunks and its head are matched
/// along: a neck between a barrel and a skull, a waist between two torsos.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub cores: [usize; 2],
    /// Where the link meets each core, in the same order.
    pub ends: [Vec3; 2],
    /// The link's own curve from `ends[0]` to `ends[1]` — the shortest way through it, so a neck
    /// bent to the side is FOLLOWED where it goes and not cut across by the chord between its ends.
    pub path: Vec<Vec3>,
    pub arc: f32,
}

/// TWO LIMBS ACROSS THE PLANE, attached within [`PAIR_ARC`] of each other along one core — what a
/// recipe's shoulder pair or hip pair is matched to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    /// The limb on the +x side of the plane, and the one on the −x side.
    pub l: usize,
    pub r: usize,
    pub core: usize,
    /// The pair's position along that core (the mean of the two attachments).
    pub t: f32,
}

/// THE GRAPH — everything above, read off one mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeGraph {
    pub cores: Vec<Core>,
    pub limbs: Vec<Limb>,
    pub links: Vec<Link>,
    pub pairs: Vec<Pair>,
    /// The body's symmetry plane (cm) — [`Flesh::core`]'s, the one measurement nothing disturbs.
    pub plane_x: f32,
    /// The lowest flesh and the body's full height (cm).
    pub floor: f32,
    pub height: f32,
    /// The largest inscribed radius anywhere in the body (cm) — what every class cut is a
    /// fraction of.
    pub max_radius: f32,
    pub cell: f32,
    /// How many twigs the prune took off, and how long the whole read took.
    pub twigs: usize,
    pub build_ms: u128,
}

/// ONE CANDIDATE PATH of the thinned walk, in the form the classifier hands to the component
/// pass: the two graph nodes it runs between, its points, the inscribed radius at each of them,
/// and whether its median radius read as CORE.
type Piece = (usize, usize, Vec<Vec3>, Vec<f32>, bool);

/// A path through the graph as its points and the inscribed radius at each of them.
type Trail = (Vec<Vec3>, Vec<f32>);

/// The cumulative arc length at each point of a path (0 at its first point).
fn arcs(path: &[Vec3]) -> Vec<f32> {
    let mut cum = Vec::with_capacity(path.len());
    let mut total = 0.0;
    for (i, p) in path.iter().enumerate() {
        if i > 0 {
            total += path[i - 1].distance(*p);
        }
        cum.push(total);
    }
    cum
}

/// The median of a slice of radii.
pub(crate) fn median(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s[s.len() / 2]
}

/// One walked path of the curve skeleton, between two nodes.
struct Seg {
    a: usize,
    b: usize,
    cells: Vec<usize>,
}

/// The skeleton as walked: node clusters (their world centres) and the paths between them.
struct Walk {
    nodes: Vec<Vec3>,
    segs: Vec<Seg>,
}

impl ShapeGraph {
    /// Thin `flesh` to its curve skeleton and read the graph off it. `None` when the field holds
    /// no flesh at all — a caller's cue to leave the composed rest where it stands (4BB12A75),
    /// never to move it somewhere invented.
    pub fn build(flesh: &Flesh) -> Option<ShapeGraph> {
        READS.with(|n| n.set(n.get() + 1));
        let started = Instant::now();
        let (solid, radius, dims) = flesh.field();
        let max_radius = radius.iter().copied().fold(0.0_f32, f32::max);
        if max_radius <= 0.0 {
            return None;
        }
        let cell = flesh.cell();
        // THE WORLD'S TWO MEASUREMENTS, read before the walk because the walk needs them: the
        // body's symmetry plane ([`Flesh::core`]'s, the one measurement nothing disturbs) and the
        // floor it stands on, with the band above it that counts as ON the floor.
        let plane_x = flesh.core().map_or(0.0, |c| c.plane_x);
        let (mut floor, mut ceil) = (f32::MAX, f32::MIN);
        for (i, &s) in solid.iter().enumerate() {
            if s {
                let z = flesh.centre_of(i).z;
                floor = floor.min(z);
                ceil = ceil.max(z);
            }
        }
        let height = (ceil - floor).max(cell);
        let ground_z = floor + GROUND_BAND * height;
        let mut keep = thin(solid, radius, dims);

        // The surviving cells, with their world positions and their own inscribed radii.
        let prune_arc = PRUNE_CELLS * cell;
        let noise = NOISE_CELLS * cell;
        let mut twigs = 0;
        let walk = loop {
            let walk = walk_skeleton(&keep, dims, flesh);
            // A LEAF path — one whose far node is an end of the curve — that is barely longer
            // than a cell, or thinner than the grid can speak about, is the mesh's surface
            // detail and not its structure. Cut it and walk again: cutting a twig can turn the
            // junction it hung on into a plain bend, and the paths either side into one.
            let mut degree = vec![0_usize; walk.nodes.len()];
            for s in &walk.segs {
                degree[s.a] += 1;
                degree[s.b] += 1;
            }
            let mut cut = 0;
            for s in &walk.segs {
                let leaf = degree[s.a] == 1 || degree[s.b] == 1;
                if !leaf || walk.segs.len() == 1 {
                    continue;
                }
                let path: Vec<Vec3> = s.cells.iter().map(|&i| flesh.centre_of(i)).collect();
                let rad: Vec<f32> = s.cells.iter().map(|&i| radius[i]).collect();
                let arc = arcs(&path).last().copied().unwrap_or(0.0);
                // The flesh the twig grows OUT of: the radius at its junction end (both ends when
                // the twig floats free of everything).
                let root_r = match (degree[s.a] > 1, degree[s.b] > 1) {
                    (true, false) => rad[0],
                    (false, true) => *rad.last().expect("a segment has cells"),
                    _ => rad[0].max(*rad.last().expect("a segment has cells")),
                };
                // ...AND A LEAF THAT CARRIES THE BODY is never one, however short it is against the
                // flesh it leaves: a low-slung body's legs are shorter than its barrel is thick,
                // and what tells them from a bump is gravity, the one direction the world supplies
                // — they hold the body off the floor ([`carries`]).
                let (top, end) = if degree[s.a] > 1 {
                    (path[0], *path.last().expect("a segment has cells"))
                } else {
                    (*path.last().expect("a segment has cells"), path[0])
                };
                let carried = carries(top, end, ground_z, GROUND_BAND * height);
                let reach = if degree[s.a].max(degree[s.b]) > 1 && carried {
                    prune_arc
                } else {
                    prune_arc.max(SPUR_RADII * root_r)
                };
                if arc > reach && median(&rad) > noise {
                    continue;
                }
                // The junction end of the twig stays — it belongs to the paths that go on.
                let keep_a = degree[s.a] > 1;
                let keep_b = degree[s.b] > 1;
                for (k, &i) in s.cells.iter().enumerate() {
                    let first = k == 0;
                    let last = k + 1 == s.cells.len();
                    if (first && keep_a) || (last && keep_b) {
                        continue;
                    }
                    keep[i] = false;
                }
                cut += 1;
            }
            twigs += cut;
            if cut == 0 {
                break walk;
            }
        };

        // ── CLASS: every path is split where its flesh crosses the core cut, so a neck running
        // into a skull is a thin path and a thick one, not one path of muddled thickness. That
        // split is what DETECTS the head instead of assuming a canon offset for it.
        let cut = CORE_FRACTION * max_radius;
        let mut nodes = walk.nodes.clone();
        let mut pieces: Vec<Piece> = Vec::new();
        for s in &walk.segs {
            // A SELF-LOOP — a path from a node back to itself — is a voxel artefact: a couple of
            // cells that close on each other at a limb's tip. Kept, it adds two to that node's
            // degree, the tip stops counting as an END, and the limb's real reach is never found
            // (measured on the horse fixture: a four-cell loop at each left hoof hid an 83 cm leg
            // behind an 18 cm corner spur).
            if s.a == s.b {
                continue;
            }
            let path: Vec<Vec3> = s.cells.iter().map(|&i| flesh.centre_of(i)).collect();
            let rad: Vec<f32> = s.cells.iter().map(|&i| radius[i]).collect();
            // The boundary between two runs is ONE node, shared by the pieces either side of it,
            // and both pieces carry the boundary sample — a neck that runs into a skull has to
            // stay JOINED to it, or the head is a core nothing chains to and the neck a limb
            // hanging off nothing.
            let mut prev = s.a;
            let runs = class_runs(&rad, cut);
            for (k, &(lo, hi, thick)) in runs.iter().enumerate() {
                let b = if k + 1 == runs.len() {
                    s.b
                } else {
                    nodes.push(path[hi]);
                    nodes.len() - 1
                };
                let start = lo.saturating_sub(1);
                pieces.push((
                    prev,
                    b,
                    path[start..=hi].to_vec(),
                    rad[start..=hi].to_vec(),
                    thick,
                ));
                prev = b;
            }
        }

        // ── CORES, LIMBS and LINKS — read again, from the class split's own pieces, whenever a
        // core turns out to be a BULGE ON A LEG (the end of the loop says what that is).
        let (fresh_pieces, fresh_nodes) = (pieces.len(), nodes.len());
        let (mut cores, mut limbs, mut links) = loop {
            pieces.truncate(fresh_pieces);
            nodes.truncate(fresh_nodes);
            // ── CORES: the connected components of the thick pieces, each laid out as its longest
            // path through itself.
            let thick: Vec<usize> = (0..pieces.len()).filter(|&i| pieces[i].4).collect();
            let mut cores: Vec<Core> = Vec::new();
            let mut core_pieces: Vec<Vec<usize>> = Vec::new();
            let mut core_of_node = vec![usize::MAX; nodes.len()];
            for comp in components(&pieces, &thick, nodes.len()) {
                let cells: Vec<(Vec3, f32)> = comp
                    .iter()
                    .flat_map(|&p| pieces[p].2.iter().copied().zip(pieces[p].3.iter().copied()))
                    .collect();
                let Some((path, radii, axis)) = centreline(&cells, cell) else {
                    continue;
                };
                let arc = arcs(&path).last().copied().unwrap_or(0.0);
                let k = cores.len();
                for &p in &comp {
                    core_of_node[pieces[p].0] = k;
                    core_of_node[pieces[p].1] = k;
                }
                core_pieces.push(comp);
                cores.push(Core {
                    radius: median(&radii),
                    path,
                    radii,
                    arc,
                    // UPRIGHT is the core's OWN axis against gravity, nothing to do with its box.
                    upright: axis.z.abs() > axis.truncate().length(),
                });
            }
            if cores.is_empty() {
                return None;
            }

            // ── LIMBS and LINKS: the connected components of the THIN pieces. One that touches a
            // single core hangs off it; one that touches two joins them; one that touches none is
            // loose flesh and is reported as a spare.
            let thin_pieces: Vec<usize> = (0..pieces.len()).filter(|&i| !pieces[i].4).collect();
            let mut limbs: Vec<Limb> = Vec::new();
            let mut links: Vec<Link> = Vec::new();
            let mut comps = components(&pieces, &thin_pieces, nodes.len());
            let mut next = 0;
            while next < comps.len() {
                let comp = std::mem::take(&mut comps[next]);
                next += 1;
                let mut roots: Vec<usize> = Vec::new();
                #[allow(unused_mut)]
                for &p in &comp {
                    for n in [pieces[p].0, pieces[p].1] {
                        if core_of_node[n] != usize::MAX && !roots.contains(&n) {
                            roots.push(n);
                        }
                    }
                }
                // WHICH CORES it touches, not how many nodes: a wing leaves the trunk along a SEAM
                // and crosses out of its flesh at two or three junctions, all on the same core. That
                // is one limb attached in several places, never a link from a body to itself.
                let mut touched: Vec<usize> = Vec::new();
                for &r in &roots {
                    if !touched.contains(&core_of_node[r]) {
                        touched.push(core_of_node[r]);
                    }
                }
                // NOTHING IS SILENTLY LOST. A thin component whose junction with the body did not
                // end up on a core node — the thicket a trunk thins to does not always hand every
                // limb a node of its own — is attached to the core it is NEAREST. Dropping it
                // instead loses a whole leg without a word (measured on the horse fixture: the two
                // left legs vanished and only the right pair was ever matched).
                if touched.is_empty() {
                    let near = |n: usize| {
                        (0..cores.len())
                            .map(|c| {
                                let d = cores[c]
                                    .path
                                    .iter()
                                    .fold(f32::MAX, |m, p| m.min(p.distance(nodes[n])));
                                (c, d)
                            })
                            .min_by(|a, b| a.1.total_cmp(&b.1))
                    };
                    let best = comp_nodes(&pieces, &comp)
                        .into_iter()
                        .filter_map(|n| near(n).map(|(c, d)| (n, c, d)))
                        .min_by(|a, b| a.2.total_cmp(&b.2));
                    if let Some((n, c, _)) = best {
                        roots.push(n);
                        touched.push(c);
                        core_of_node[n] = c;
                    }
                }
                match touched.len() {
                    0 => {}
                    1 => {
                        // TWO LIMBS THAT MET UNDER THE BODY. One limb stands on one side of the
                        // symmetry plane; a thin component whose flesh reaches the FLOOR on both sides
                        // of it is two limbs whose curves joined below the trunk — a low belly, tail
                        // hair tangled between two hind legs, two hooves crossed mid-stride. It is CUT
                        // at the plane and each side walked again as a component of its own, attached
                        // where ITS side meets the core, so no limb's lead ever crosses the plane.
                        let band = MIDLINE_FRACTION * cores[touched[0]].radius;
                        if stands_both_sides(&pieces, &comp, plane_x, band, ground_z) {
                            for side in cut_at_plane(&mut pieces, &mut nodes, &comp, plane_x, band)
                            {
                                comps.extend(components(&pieces, &side, nodes.len()));
                            }
                            core_of_node.resize(nodes.len(), usize::MAX);
                            continue;
                        }
                        // The middle of the seam is the attachment: the root nearest the roots' own
                        // centre, so a wing hangs from the middle of where it leaves the body.
                        let mid =
                            roots.iter().map(|&r| nodes[r]).sum::<Vec3>() / roots.len() as f32;
                        let root = *roots
                            .iter()
                            .min_by(|&&a, &&b| {
                                nodes[a].distance(mid).total_cmp(&nodes[b].distance(mid))
                            })
                            .expect("a touched core has a root");
                        let (lead, lead_r, fan) = lead_and_fan(&pieces, &comp, &nodes, root);
                        if lead.len() < 2 {
                            continue;
                        }
                        let arc = arcs(&lead).last().copied().unwrap_or(0.0);
                        limbs.push(Limb {
                            core: core_of_node[root],
                            at: nodes[root],
                            t: 0.0,
                            side: 0.0,
                            sheet: false,
                            arc,
                            grounded: false,
                            fan,
                            lead,
                            lead_r,
                        });
                    }
                    _ => {
                        let a = *roots
                            .iter()
                            .find(|&&r| core_of_node[r] == touched[0])
                            .expect("a touched core has a root");
                        let b = *roots
                            .iter()
                            .find(|&&r| core_of_node[r] == touched[1])
                            .expect("a touched core has a root");
                        let (_, prev) = geodesic(&pieces, &comp, nodes.len(), a);
                        let (path, _) = geodesic_path(&pieces, &prev, a, b);
                        links.push(Link {
                            cores: [touched[0], touched[1]],
                            ends: [nodes[a], nodes[b]],
                            arc: arcs(&path).last().copied().unwrap_or(0.0),
                            path,
                        });
                    }
                }
            }

            // ── A BULGE ON A LEG IS NOT A TRUNK. The class cut is half the body's thickest flesh, and
            // on a slim body a knee or a thigh is that thick: it reads as a small core, chained to
            // the torso through the thigh above it, with the shin below it hanging off it as a limb
            // — and a leg split across two cores pairs with nothing. A core chained to ONE larger
            // core, every limb of which (beyond a spur) carries the body down to the floor, is such
            // a bulge: its pieces are read as thin and the body is read again, the thigh, the bulge
            // and the shin one limb of the core the thigh leads to. A head carries nothing to the
            // floor, and a mounted torso carries arms.
            let knees: Vec<usize> = (0..cores.len())
                .filter(|&k| {
                    let size = |c: usize| cores[c].arc * cores[c].radius;
                    let mut chained: Vec<usize> = links
                        .iter()
                        .filter_map(|l| match (l.cores[0] == k, l.cores[1] == k) {
                            (true, _) => Some(l.cores[1]),
                            (_, true) => Some(l.cores[0]),
                            _ => None,
                        })
                        .collect();
                    chained.sort_unstable();
                    chained.dedup();
                    let own: Vec<&Limb> = limbs
                        .iter()
                        .filter(|l| l.core == k && l.arc > SPUR_RADII * cores[k].radius)
                        .collect();
                    chained.len() == 1
                        && size(chained[0]) > size(k)
                        && !own.is_empty()
                        && own.iter().all(|l| l.tip().z <= ground_z)
                })
                .collect();
            if knees.is_empty() {
                break (cores, limbs, links);
            }
            for &k in &knees {
                for &p in &core_pieces[k] {
                    pieces[p].4 = false;
                }
            }
        };

        // ── ORIENT each core, now that the chain and the limbs hanging off it are known.
        for k in 0..cores.len() {
            if front_is_first(&cores, &links, k) {
                cores[k].path.reverse();
                cores[k].radii.reverse();
            }
        }

        // ── Each limb's place on its core, its side of the plane, whether it reaches the ground,
        // and whether its fan is a sheet.
        for limb in &mut limbs {
            // GRAVITY PICKS THE END A LIMB STANDS ON. The lead is the geodesic to the end that
            // reaches FARTHEST, and on a leg whose fan holds the hoof beside a dew-claw or a
            // fetlock tuft a centimetre longer, the farthest end is not the one on the floor.
            // Where an end of the tree reaches the ground and the lead's does not, the LOWEST such
            // end becomes the lead: "the ground joint at the tube's end" (spec 04803E0C §3) only
            // holds when the tube's end is the end that reaches the ground. Gravity is the one
            // direction the world supplies and already tells a leg from an arm here; this is the
            // same reading, one level down — and only for a lead that ends JUST ABOVE the floor,
            // within a second band of it (a tuft beside the hoof): a limb whose far end stands
            // high in the air is not standing, and a strand of tail on the floor under a folded
            // wing is not the end the wing stands on.
            if limb.tip().z > ground_z && limb.tip().z <= ground_z + GROUND_BAND * height {
                let mut low: Option<(usize, f32)> = None;
                for (i, (p, _)) in limb.fan.iter().enumerate() {
                    let Some(end) = p.last() else { continue };
                    if end.z <= ground_z && low.is_none_or(|(_, z)| end.z < z) {
                        low = Some((i, end.z));
                    }
                }
                if let Some((i, _)) = low {
                    let (path, radii) = limb.fan.remove(i);
                    let lead = std::mem::replace(&mut limb.lead, path);
                    let lead_r = std::mem::replace(&mut limb.lead_r, radii);
                    limb.fan.push((lead, lead_r));
                    limb.arc = arcs(&limb.lead).last().copied().unwrap_or(0.0);
                }
            }
            let core = &cores[limb.core];
            limb.t = core.t_of(limb.at);
            let off = limb.lead.iter().map(|p| p.x - plane_x).sum::<f32>() / limb.lead.len() as f32;
            limb.side = if off.abs() > MIDLINE_FRACTION * core.radius {
                off.signum()
            } else {
                0.0
            };
            limb.grounded = limb.tip().z <= ground_z;
            limb.sheet = is_sheet(flesh, limb);
        }
        // THE SAME SPUR RULE, now that each limb is exactly what LEAVES the core: a limb shorter
        // than [`SPUR_RADII`] of the core's own thickness where it attaches is a bump on the body
        // and not a limb of it. Before the class split this cannot be judged — the walk's path
        // starts at a junction deep inside the trunk, so a corner of a box measures the whole
        // half-width of the box and passes; after it, the same corner is a 7 cm stub off flesh
        // 10 cm thick.
        // A limb that CARRIES THE BODY ([`carries`]) is kept whatever its length — the walk's
        // prune spares it for the same reason.
        let before = limbs.len();
        limbs.retain(|l| {
            l.arc > SPUR_RADII * cores[l.core].radius
                || (l.arc > prune_arc && carries(l.at, l.tip(), ground_z, GROUND_BAND * height))
        });
        twigs += before - limbs.len();
        merge_sheets(&mut limbs);

        // ── PAIRS: opposite sides, same core, attached within PAIR_ARC of each other. The
        // closest partner first, each limb used once.
        //
        // THE SIDE A LIMB PAIRS FROM is its own ([`Limb::side`]) — or, for a limb whose lead
        // averages inside the midline band, the side it LEAVES the band on, when it STANDS: a tube
        // on the floor whose lead drops at least as far as it runs across and gets clearly off the
        // plane on its way down. A leg stepping in under the body leaves a junction on the midline
        // and puts its hoof back near it, and its average sits on the band's edge — read again a
        // sub-cell off it flips, and the pair with it (A31C0FAE: the Camel's +x hind leg averaged
        // +4.84 against a 4.85 band on its faced re-read, +5.0 to +5.6 on the reads a sub-cell off;
        // the Ram's, the Donkey's and the Wolf's the same, measured on the family re-read
        // eight ways). No statistic of one limb separates that leg from a tail (AF58447A): the
        // PAIR does — a standing tube beside a standing twin across the plane. A tail lying along
        // the floor runs further than it drops, a sheet is a wing or a fan, and a strand on the
        // plane never leaves the band.
        let band = |l: &Limb| MIDLINE_FRACTION * cores[l.core].radius;
        let pairs_from = |l: &Limb| -> f32 {
            if l.side != 0.0 || l.sheet || !l.grounded {
                return l.side;
            }
            let far = l.lead.iter().map(|p| p.x - plane_x).fold(0.0_f32, |m, x| {
                if x.abs() > m.abs() {
                    x
                } else {
                    m
                }
            });
            let (drop, run) = (l.at.z - l.tip().z, (l.tip() - l.at).truncate().length());
            if drop >= run && far.abs() > band(l) {
                far.signum()
            } else {
                0.0
            }
        };
        let side: Vec<f32> = limbs.iter().map(pairs_from).collect();
        let mut pairs: Vec<Pair> = Vec::new();
        let mut taken = vec![false; limbs.len()];
        let mut cands: Vec<(f32, usize, usize)> = Vec::new();
        let mirror = |p: Vec3| Vec3::new(2.0 * plane_x - p.x, p.y, p.z);
        for i in 0..limbs.len() {
            for j in 0..limbs.len() {
                let (a, b) = (&limbs[i], &limbs[j]);
                if a.core != b.core || side[i] <= 0.0 || side[j] >= 0.0 {
                    continue;
                }
                // ...and a standing limb read off its way down pairs only with a standing twin.
                if (a.side == 0.0 || b.side == 0.0) && !(a.grounded && b.grounded) {
                    continue;
                }
                // THE STRIDE is not a difference: every source is generated mid-stride, and a limb
                // that STANDS swings about its socket along the one horizontal direction the
                // symmetry plane contains — the rig's forward, Y — so the two halves of a standing
                // pair put their tips a stride apart along it. Between limbs of which at least one
                // stands on the floor the mirror is judged ACROSS the plane and against gravity
                // only; limbs in the air (arms, wings, a pair of ears) do not stride and are judged
                // whole. The full distance still ranks the candidates, nearest first.
                let gap = a.tip() - mirror(b.tip());
                let reach = MIRROR_FRACTION * a.arc.min(b.arc);
                let apart = if a.grounded || b.grounded {
                    gap.x.hypot(gap.z)
                } else {
                    gap.length()
                };
                // THE ATTACHMENTS decide only where the mirror cannot. Two limbs whose tips mirror
                // each other WHOLE — across the plane, along the body and against gravity — are a
                // pair wherever the thinning put their junctions: a junction lands where the flesh
                // lets it, and on a barrel splaying into two thighs a hind pair's two junctions
                // read a quarter of the core apart on one read and a sixth on the next (the
                // Squirrel, the goats — A31C0FAE). The stride-blind reading still needs
                // [`PAIR_ARC`]: there it is all that tells a fore limb from a hind one.
                let near = (a.t - b.t).abs() <= PAIR_ARC || gap.length() <= reach;
                if near && apart <= reach {
                    cands.push((gap.length(), i, j));
                }
            }
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, i, j) in cands {
            if taken[i] || taken[j] {
                continue;
            }
            taken[i] = true;
            taken[j] = true;
            // A limb that paired from inside the band is on the side it paired from.
            limbs[i].side = 1.0;
            limbs[j].side = -1.0;
            pairs.push(Pair {
                l: i,
                r: j,
                core: limbs[i].core,
                t: 0.5 * (limbs[i].t + limbs[j].t),
            });
        }
        pairs.sort_by(|a, b| a.core.cmp(&b.core).then(a.t.total_cmp(&b.t)));

        // The biggest core first — the recipe's root trunk is matched to it, and everything that
        // refers to a core by index must survive the sort.
        let mut order: Vec<usize> = (0..cores.len()).collect();
        order.sort_by(|&a, &b| {
            (cores[b].arc * cores[b].radius).total_cmp(&(cores[a].arc * cores[a].radius))
        });
        let mut rank = vec![0_usize; cores.len()];
        for (new, &old) in order.iter().enumerate() {
            rank[old] = new;
        }
        let cores: Vec<Core> = order.iter().map(|&i| cores[i].clone()).collect();
        for l in &mut limbs {
            l.core = rank[l.core];
        }
        for l in &mut links {
            l.cores = [rank[l.cores[0]], rank[l.cores[1]]];
        }
        for p in &mut pairs {
            p.core = rank[p.core];
        }

        Some(ShapeGraph {
            cores,
            limbs,
            links,
            pairs,
            plane_x,
            floor,
            height,
            max_radius,
            cell,
            twigs,
            build_ms: started.elapsed().as_millis(),
        })
    }

    /// Every limb that belongs to no [`Pair`] — a tail, a single horn, one wing of a pair the
    /// graph could not separate. Index into [`ShapeGraph::limbs`].
    pub fn unpaired(&self) -> Vec<usize> {
        let paired: HashSet<usize> = self.pairs.iter().flat_map(|p| [p.l, p.r]).collect();
        (0..self.limbs.len())
            .filter(|i| !paired.contains(i))
            .collect()
    }

    /// Every core and limb spelled out, one per line — what a sweep diagnostic prints when the
    /// summary count is not the whole story.
    pub fn detail(&self) -> String {
        let mut out = String::new();
        for (i, c) in self.cores.iter().enumerate() {
            out.push_str(&format!(
                "  core {i} {} arc {:.0} r {:.1} rear {:.0?} front {:.0?}\n",
                if c.upright { "upright" } else { "lying  " },
                c.arc,
                c.radius,
                c.rear().to_array(),
                c.front().to_array(),
            ));
        }
        for (i, l) in self.limbs.iter().enumerate() {
            out.push_str(&format!(
                "  limb {i} core {} t {:.2} side {:+.0} {} arc {:.0} ends {} {} tip {:.0?}\n",
                l.core,
                l.t,
                l.side,
                if l.sheet { "SHEET" } else { "tube " },
                l.arc,
                1 + l.fan.len(),
                if l.grounded { "ground" } else { "      " },
                l.tip().to_array(),
            ));
            for (f, _) in &l.fan {
                out.push_str(&format!(
                    "      fan {:.0} cm to {:.0?}\n",
                    arcs(f).last().copied().unwrap_or(0.0),
                    f.last().map(|p| p.to_array()).unwrap_or_default()
                ));
            }
        }
        for p in &self.pairs {
            out.push_str(&format!(
                "  pair core {} t {:.2} limbs {}/{}\n",
                p.core, p.t, p.l, p.r
            ));
        }
        out
    }

    /// One line per body for a sweep table: what the graph found.
    pub fn summary(&self) -> String {
        let sheets = self.limbs.iter().filter(|l| l.sheet).count();
        let ground = self.limbs.iter().filter(|l| l.grounded).count();
        format!(
            "cores {} tubes {} sheets {} pairs {} unpaired {} grounded {} links {} twigs {} \
             max_r {:.1} plane {:.2} {} ms",
            self.cores.len(),
            self.limbs.len() - sheets,
            sheets,
            self.pairs.len(),
            self.unpaired().len(),
            ground,
            self.links.len(),
            self.twigs,
            self.max_radius,
            self.plane_x,
            self.build_ms,
        )
    }
}

/// One component of a vector by axis index.
fn comp_of(v: Vec3, axis: usize) -> f32 {
    v.to_array()[axis.min(2)]
}

/// IS THIS LIMB A SHEET — is the FLESH around it a plate rather than a tube? At samples along the
/// lead, the body is measured across the two axes least aligned with it: a tube reads about the
/// same on both, a spread wing reads its chord on one and its membrane on the other (Aaron's
/// amendment, ruling 0EE33D8F — a wing is *thin in one direction, broad in the other two*).
///
/// Read off the FLESH and not off the skeleton, because a plate only three cells thick thins to a
/// single curve, not to the fan a thicker one gives: the curve count says nothing, the
/// cross-section says everything.
fn is_sheet(flesh: &Flesh, limb: &Limb) -> bool {
    let dir = (limb.tip() - limb.at).normalize_or_zero();
    if dir.length_squared() < 0.5 {
        return false;
    }
    let mut axes = [0_usize, 1, 2];
    axes.sort_by(|&a, &b| comp_of(dir, a).abs().total_cmp(&comp_of(dir, b).abs()));
    let across = |at: Vec3, axis: usize| -> Option<f32> {
        let x = comp_of(at, axis);
        flesh
            .runs(at, axis)
            .into_iter()
            .find(|r| r.0 <= x && r.1 >= x)
            .map(|r| r.1 - r.0)
    };
    let mut ratios: Vec<f32> = Vec::new();
    // The attachment itself is inside the body and reads the body's own girth — start past it.
    for k in 1..SHEET_SAMPLES {
        let at = limb.at_lead(k as f32 / SHEET_SAMPLES as f32);
        if let (Some(a), Some(b)) = (across(at, axes[0]), across(at, axes[1])) {
            if a.min(b) > 0.0 {
                ratios.push(a.max(b) / a.min(b));
            }
        }
    }
    ratios.len() >= SHEET_READS && median(&ratios) >= SHEET_RATIO
}

/// DOES THIS CURVE CARRY THE BODY — run from `top`, a whole ground band clear of the band on the
/// floor, down to `end` inside that band? Gravity is the one direction the world supplies: a leg
/// holds the body off the floor however short it is against the barrel it holds up, and a bump on
/// a body lying ON the floor (a box's bottom corner) starts inside the lowest bands and does not.
fn carries(top: Vec3, end: Vec3, ground_z: f32, band: f32) -> bool {
    end.z <= ground_z && top.z > ground_z + band
}

/// DOES A THIN COMPONENT STAND ON BOTH SIDES OF THE PLANE — flesh on the floor more than `band`
/// to the left of it AND more than `band` to the right? One limb stands on one side. A head, a
/// pair of antlers or a fanned tail spreads across the plane too, but in the air: only two limbs
/// that each reach the floor on their own side say that the component is two.
fn stands_both_sides(
    pieces: &[Piece],
    comp: &[usize],
    plane_x: f32,
    band: f32,
    ground_z: f32,
) -> bool {
    let (mut left, mut right) = (false, false);
    for q in comp.iter().flat_map(|&p| pieces[p].2.iter()) {
        if q.z <= ground_z {
            left |= q.x - plane_x > band;
            right |= q.x - plane_x < -band;
        }
    }
    left && right
}

/// CUT A THIN COMPONENT AT THE SYMMETRY PLANE into the pieces on each side of it — `[+x, -x]`,
/// as indices of new pieces appended to `pieces`. The two legs of a pair can meet UNDER the body
/// — a low belly between them, tail hair tangled across them, two hooves crossed mid-stride — and
/// then their thinned curves are ONE component, which walked from one attachment hands back one
/// limb with a leg at each end of it and a lead that crosses the plane.
///
/// Every piece is walked point by point and cut where it CROSSES the plane: the new piece ends on
/// the last point on the old side and the next begins on the first point past it, each on a node
/// of its own, so the sides share nothing but the cut. `band` is hysteresis — a curve wandering
/// less than that across the plane (the shaft of a tail lying on it) is not cut for it, and a
/// piece that never leaves the band stays whole on the side its flesh leans to.
fn cut_at_plane(
    pieces: &mut Vec<Piece>,
    nodes: &mut Vec<Vec3>,
    comp: &[usize],
    plane_x: f32,
    band: f32,
) -> [Vec<usize>; 2] {
    let mut sides: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for &p in comp {
        let (a, b, pts, rad, thick) = pieces[p].clone();
        let off: Vec<f32> = pts.iter().map(|q| q.x - plane_x).collect();
        // The side it starts on is the first one it clearly reaches.
        let lean = off
            .iter()
            .copied()
            .find(|o| o.abs() > band)
            .unwrap_or_else(|| off.iter().sum());
        let mut cur = if lean < 0.0 { -1.0_f32 } else { 1.0 };
        if pts.len() < 2 {
            sides[usize::from(cur < 0.0)].push(p);
            continue;
        }
        let (mut lo, mut from, mut last) = (0_usize, a, 0_usize);
        let mut cuts: Vec<(usize, usize, usize, usize, f32)> = Vec::new();
        for (i, &o) in off.iter().enumerate() {
            if o * cur >= 0.0 {
                last = i;
            } else if o * cur < -band {
                // It CROSSED, between `last` and `last + 1`: a node at each, one per side —
                // unless the run behind is a single point, which simply goes with the side ahead.
                if last > lo {
                    nodes.push(pts[last]);
                    cuts.push((from, nodes.len() - 1, lo, last, cur));
                    nodes.push(pts[last + 1]);
                    from = nodes.len() - 1;
                    lo = last + 1;
                }
                cur = -cur;
                last = i;
            }
        }
        cuts.push((from, b, lo, pts.len() - 1, cur));
        for (f, t, l, h, side) in cuts {
            if h > l {
                pieces.push((f, t, pts[l..=h].to_vec(), rad[l..=h].to_vec(), thick));
                sides[usize::from(side < 0.0)].push(pieces.len() - 1);
            }
        }
    }
    sides
}

/// GATHER A WING THAT ARRIVED IN PIECES. A spread wing leaves the body over a SEAM, not at a
/// point: its leading edge and its membrane cross out of the trunk's flesh at their own junctions,
/// so the thin components come back as two or three limbs side by side instead of one fan.
///
/// Any group on the same core and the same side of the plane whose attachments lie within
/// [`SHEET_GATHER`] of the core's own thickness of each other, and of which at least one member
/// reads as a sheet, becomes ONE limb: the longest member's path is the LEADING PATH and the rest
/// are its trailing fan (ruling 0EE33D8F). A leg and a tail are never gathered — one is on the
/// midline, the other far along the core, and neither reads as a sheet.
fn merge_sheets(limbs: &mut Vec<Limb>) {
    let mut out: Vec<Limb> = Vec::new();
    let mut taken = vec![false; limbs.len()];
    for i in 0..limbs.len() {
        if taken[i] {
            continue;
        }
        if !limbs[i].sheet {
            continue;
        }
        let reach = SHEET_GATHER * limbs[i].arc;
        // ONLY sheets are gathered. A leg beside a wing is a tube and stays its own limb however
        // close to the wing it leaves the body.
        let mut group: Vec<usize> = (0..limbs.len())
            .filter(|&j| {
                !taken[j]
                    && limbs[j].sheet
                    && limbs[j].core == limbs[i].core
                    && limbs[j].side == limbs[i].side
                    && limbs[j].side != 0.0
                    && limbs[j].at.distance(limbs[i].at) <= reach.max(SHEET_GATHER * limbs[j].arc)
            })
            .collect();
        if group.len() < 2 {
            continue;
        }
        // The longest member leads; the others trail behind it.
        group.sort_by(|&a, &b| limbs[b].arc.total_cmp(&limbs[a].arc));
        let mut lead = limbs[group[0]].clone();
        for &j in &group[1..] {
            lead.fan
                .push((limbs[j].lead.clone(), limbs[j].lead_r.clone()));
            lead.fan.extend(limbs[j].fan.iter().cloned());
        }
        lead.sheet = true;
        for &j in &group {
            taken[j] = true;
        }
        out.push(lead);
    }
    for (i, l) in limbs.drain(..).enumerate() {
        if !taken[i] {
            out.push(l);
        }
    }
    *limbs = out;
}

/// THE CENTRELINE OF A CHUNKY COMPONENT — its cells binned along their own principal direction,
/// one point per bin at that slice's middle. A torso or a barrel is not a curve: thinning it
/// leaves a little thicket, and the LONGEST PATH through that thicket is a diagonal across the
/// box (measured: a biped's read as the line across its shoulders). The principal axis is the
/// body's own, and the slice means are the line down the middle of it.
///
/// Returns the path, the radius at each point, and the axis — `None` for a component too small to
/// have a direction.
fn centreline(cells: &[(Vec3, f32)], cell: f32) -> Option<(Vec<Vec3>, Vec<f32>, Vec3)> {
    if cells.len() < 2 {
        return None;
    }
    let pts: Vec<Vec3> = cells.iter().map(|c| c.0).collect();
    let mean = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
    // The dominant eigenvector of the scatter, by power iteration on its covariance.
    let mut cov = [[0.0_f64; 3]; 3];
    for p in &pts {
        let d = (*p - mean).to_array().map(|v| v as f64);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let mut axis = Vec3::new(0.577, 0.511, 0.637).normalize();
    for _ in 0..32 {
        let a = axis.to_array().map(|v| v as f64);
        let r = [0, 1, 2].map(|i| cov[i][0] * a[0] + cov[i][1] * a[1] + cov[i][2] * a[2]);
        let n = Vec3::new(r[0] as f32, r[1] as f32, r[2] as f32);
        if n.length_squared() < 1e-12 {
            break;
        }
        axis = n.normalize();
    }
    let along = |p: Vec3| (p - mean).dot(axis);
    let (lo, hi) = cells.iter().fold((f32::MAX, f32::MIN), |(l, h), c| {
        (l.min(along(c.0)), h.max(along(c.0)))
    });
    // Two cells to a slice, so a slice holds enough of the medial thicket for its mean to be the
    // middle of the body and not wherever that slice's few cells happened to fall.
    let step = 2.0 * cell;
    let n = (((hi - lo) / step).round() as usize).max(1);
    let mut sum = vec![(Vec3::ZERO, 0.0_f32, 0_u32); n + 1];
    for c in cells {
        let k = (((along(c.0) - lo) / step).round() as usize).min(n);
        sum[k].0 += c.0;
        sum[k].1 += c.1;
        sum[k].2 += 1;
    }
    let (mut path, mut radii) = (Vec::new(), Vec::new());
    for (p, r, k) in sum {
        if k > 0 {
            path.push(p / k as f32);
            radii.push(r / k as f32);
        }
    }
    // One three-point smoothing pass: the slice means of a lumpy body zig-zag, and an arc length
    // measured over the zig-zag is longer than the body is.
    if path.len() > 2 {
        let raw = path.clone();
        for i in 1..raw.len() - 1 {
            path[i] = (raw[i - 1] + raw[i] + raw[i + 1]) / 3.0;
        }
    }
    (path.len() >= 2).then_some((path, radii, axis))
}

/// DISTANCE-ORDERED HOMOTOPIC THINNING — the curve skeleton of an occupancy field. Border cells
/// are deleted in order of increasing inscribed radius, so the skeleton settles on the body's
/// medial curve rather than on whichever side the scan reached first; a cell goes only when it is
/// a [`simple`] point, and never when it is already the end of a curve (one neighbour or none),
/// which is what keeps a limb's tip where the limb ends.
fn thin(solid: &[bool], radius: &[f32], dims: [usize; 3]) -> Vec<bool> {
    let (a6, a26, ..) = &MASKS;
    let mut keep = solid.to_vec();
    let (w, h) = (dims[0], dims[1]);
    let plane = w * h;
    // A cell's 26 neighbour OFFSETS in the linear index, and the 6 face ones. The grid is padded
    // by a cell all round (`Flesh::raster`), so a solid cell is never on the boundary and the
    // offsets are always in range.
    let step = |o: [i64; 3]| o[0] + w as i64 * o[1] + plane as i64 * o[2];
    let off26: Vec<i64> = NEIGHBOURS.iter().map(|o| step(*o)).collect();
    let bit: Vec<usize> = NEIGHBOURS
        .iter()
        .map(|o| ((o[2] + 1) * 9 + (o[1] + 1) * 3 + (o[0] + 1)) as usize)
        .collect();
    let faces: Vec<i64> = (0..26)
        .filter(|&k| {
            let o = NEIGHBOURS[k];
            o[0].abs() + o[1].abs() + o[2].abs() == 1
        })
        .map(|k| off26[k])
        .collect();
    let total = keep.len();
    let edge = move |i: usize| {
        i < plane + w + 1 || i + plane + w + 1 >= total || i.is_multiple_of(w) || (i % w) + 1 == w
    };
    let _ = (a6, a26);
    loop {
        let mut border: Vec<(u32, usize)> = Vec::new();
        for i in 0..keep.len() {
            if !keep[i] || edge(i) {
                continue;
            }
            if faces.iter().any(|&d| !keep[(i as i64 + d).max(0) as usize]) {
                // Sort key: the inscribed radius in thousandths of a cm, so the order is stable.
                border.push(((radius[i] * 1000.0) as u32, i));
            }
        }
        border.sort_unstable();
        let mut removed = 0;
        for (_, i) in border {
            if !keep[i] {
                continue;
            }
            let mut nb = 1_u32 << MID;
            for k in 0..26 {
                if keep[(i as i64 + off26[k]).max(0) as usize] {
                    nb |= 1 << bit[k];
                }
            }
            if curve_end(nb) || !simple(nb) {
                continue;
            }
            keep[i] = false;
            removed += 1;
        }
        if removed == 0 {
            break keep;
        }
    }
}

/// Walk the thinned cells into NODES (the ends and junctions of the curve, adjacent ones clustered
/// into one) and the PATHS between them.
fn walk_skeleton(keep: &[bool], dims: [usize; 3], flesh: &Flesh) -> Walk {
    let (w, h) = (dims[0], dims[1]);
    let plane = w * h;
    let off: Vec<i64> = NEIGHBOURS
        .iter()
        .map(|o| o[0] + w as i64 * o[1] + plane as i64 * o[2])
        .collect();
    let cells: Vec<usize> = (0..keep.len()).filter(|&i| keep[i]).collect();
    let mut id = vec![usize::MAX; keep.len()];
    for (k, &i) in cells.iter().enumerate() {
        id[i] = k;
    }
    let nbrs = |k: usize| -> Vec<usize> {
        let i = cells[k] as i64;
        off.iter()
            .filter_map(|&d| {
                let j = (i + d) as usize;
                (j < keep.len() && keep[j]).then(|| id[j])
            })
            .collect()
    };
    // A node cell is an END (one neighbour or none) or a JUNCTION (three or more). Adjacent node
    // cells are ONE node — a thinned junction is often a couple of cells wide.
    let mut node_of = vec![usize::MAX; cells.len()];
    let mut nodes: Vec<Vec3> = Vec::new();
    for k in 0..cells.len() {
        if node_of[k] != usize::MAX || nbrs(k).len() == 2 {
            continue;
        }
        let n = nodes.len();
        let mut group = vec![k];
        node_of[k] = n;
        let mut stack = vec![k];
        while let Some(c) = stack.pop() {
            for m in nbrs(c) {
                if node_of[m] == usize::MAX && nbrs(m).len() != 2 {
                    node_of[m] = n;
                    group.push(m);
                    stack.push(m);
                }
            }
        }
        let sum: Vec3 = group.iter().map(|&g| flesh.centre_of(cells[g])).sum();
        nodes.push(sum / group.len() as f32);
    }
    // A ring with no junction and no end anywhere on it: make its first cell a node so the ring
    // is one path back to itself rather than nothing at all.
    if nodes.is_empty() && !cells.is_empty() {
        node_of[0] = 0;
        nodes.push(flesh.centre_of(cells[0]));
    }

    let mut segs: Vec<Seg> = Vec::new();
    let mut walked = vec![false; cells.len()];
    let mut direct: HashSet<(usize, usize)> = HashSet::new();
    for k in 0..cells.len() {
        let Some(from) = (node_of[k] != usize::MAX).then_some(node_of[k]) else {
            continue;
        };
        for first in nbrs(k) {
            if node_of[first] == from {
                continue;
            }
            if node_of[first] != usize::MAX {
                // Two nodes touching: the path between them is the two cells themselves.
                let key = (k.min(first), k.max(first));
                if direct.insert(key) {
                    segs.push(Seg {
                        a: from,
                        b: node_of[first],
                        cells: vec![cells[k], cells[first]],
                    });
                }
                continue;
            }
            if walked[first] {
                continue;
            }
            let (mut prev, mut cur) = (k, first);
            let mut path = vec![cells[k]];
            let end = loop {
                walked[cur] = true;
                path.push(cells[cur]);
                let next = nbrs(cur)
                    .into_iter()
                    .find(|&m| m != prev && m != cur && (node_of[m] != usize::MAX || !walked[m]));
                match next {
                    Some(m) if node_of[m] != usize::MAX => {
                        path.push(cells[m]);
                        break node_of[m];
                    }
                    Some(m) => {
                        prev = cur;
                        cur = m;
                    }
                    // A chain that runs out without meeting a node (a ring the walk entered
                    // sideways): close it on a node of its own so nothing is lost.
                    None => {
                        nodes.push(flesh.centre_of(cells[cur]));
                        node_of[cur] = nodes.len() - 1;
                        break nodes.len() - 1;
                    }
                }
            };
            segs.push(Seg {
                a: from,
                b: end,
                cells: path,
            });
        }
    }
    Walk { nodes, segs }
}

/// The maximal runs of one CLASS along a radius profile, as `(first, last, thick)`. A run shorter
/// than [`CLASS_RUN`] is absorbed into the run before it — a couple of cells of thick flesh in
/// the middle of a neck is not a second torso.
fn class_runs(rad: &[f32], cut: f32) -> Vec<(usize, usize, bool)> {
    let mut runs: Vec<(usize, usize, bool)> = Vec::new();
    for (i, &r) in rad.iter().enumerate() {
        let thick = r >= cut;
        match runs.last_mut() {
            Some(last) if last.2 == thick => last.1 = i,
            _ => runs.push((i, i, thick)),
        }
    }
    let mut out: Vec<(usize, usize, bool)> = Vec::new();
    for run in runs {
        let short = run.1 - run.0 + 1 < CLASS_RUN;
        match out.last_mut() {
            Some(last) if short || last.2 == run.2 => last.1 = run.1,
            _ => out.push(run),
        }
    }
    // A path whose whole profile is one short run keeps its class rather than vanishing.
    if out.is_empty() && !rad.is_empty() {
        out.push((0, rad.len() - 1, median(rad) >= cut));
    }
    out
}

/// The connected components of `chosen` pieces, joined where they share a node.
fn components(pieces: &[Piece], chosen: &[usize], nodes: usize) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..nodes).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for &p in chosen {
        let (a, b) = (
            find(&mut parent, pieces[p].0),
            find(&mut parent, pieces[p].1),
        );
        parent[a] = b;
    }
    let mut by_root: Vec<(usize, Vec<usize>)> = Vec::new();
    for &p in chosen {
        let r = find(&mut parent, pieces[p].0);
        match by_root.iter_mut().find(|(k, _)| *k == r) {
            Some((_, v)) => v.push(p),
            None => by_root.push((r, vec![p])),
        }
    }
    by_root.into_iter().map(|(_, v)| v).collect()
}

/// Every node a component touches.
fn comp_nodes(pieces: &[Piece], comp: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for &p in comp {
        for n in [pieces[p].0, pieces[p].1] {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// THE GEODESIC TREE from `root` through a component: the SHORTEST way to every node in it, and
/// the piece each one was reached by. A limb's reach is how far away its far end is, and a wing
/// whose leading edge and membrane meet again at the tip is a RING — walked for the LONGEST path
/// it reports a lead that goes out along one side and back along the other (measured: 145 cm of
/// "leading path" on a 59 cm wing). The shortest way to the farthest end is the leading path.
fn geodesic(
    pieces: &[Piece],
    comp: &[usize],
    nodes: usize,
    root: usize,
) -> (Vec<f32>, Vec<Option<(usize, usize)>>) {
    let mut dist = vec![f32::MAX; nodes];
    let mut prev: Vec<Option<(usize, usize)>> = vec![None; nodes];
    dist[root] = 0.0;
    // The components are small; a plain relax-until-stable is shorter than a heap and as fast.
    for _ in 0..=comp.len() {
        let mut moved = false;
        for &p in comp {
            let (a, b) = (pieces[p].0, pieces[p].1);
            let w = arcs(&pieces[p].2).last().copied().unwrap_or(0.0);
            for (from, to) in [(a, b), (b, a)] {
                if dist[from] < f32::MAX && dist[from] + w < dist[to] - 1e-4 {
                    dist[to] = dist[from] + w;
                    prev[to] = Some((p, from));
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
    (dist, prev)
}

/// The path `root` → `target` through a [`geodesic`] tree, as its points and their radii.
fn geodesic_path(
    pieces: &[Piece],
    prev: &[Option<(usize, usize)>],
    root: usize,
    target: usize,
) -> Trail {
    let mut legs: Vec<(usize, usize)> = Vec::new();
    let mut at = target;
    while at != root {
        let Some((p, from)) = prev[at] else {
            return (Vec::new(), Vec::new());
        };
        legs.push((p, from));
        at = from;
        if legs.len() > prev.len() {
            return (Vec::new(), Vec::new());
        }
    }
    legs.reverse();
    let (mut path, mut radii) = (Vec::new(), Vec::new());
    for (p, from) in legs {
        let mut pts = pieces[p].2.clone();
        let mut rs = pieces[p].3.clone();
        if pieces[p].0 != from {
            pts.reverse();
            rs.reverse();
        }
        if path.is_empty() {
            path.push(pts[0]);
            radii.push(rs[0]);
        }
        path.extend_from_slice(&pts[1..]);
        radii.extend_from_slice(&rs[1..]);
    }
    (path, radii)
}

/// A limb's LEAD and its FAN: the geodesics from `root` to every end of the component, the lead
/// the one to the end that REACHES FARTHEST from the attachment and the fan the rest, farthest
/// first.
///
/// FARTHEST IN SPACE, not along the tree. The tree's paths are the shortest ways out (a ring —
/// a wing whose leading edge and membrane meet again — must not be walked round), but the end a
/// limb GOES to is the one furthest from where it leaves the body: a wing that also touches a
/// fold of tail feathers reaches its tip, not the strand whose path wanders round the seam to
/// get there (measured on a real crow: a 297 cm "lead" down a tail strand 50 cm from the
/// shoulder, while the wing tip stood 130 cm out).
fn lead_and_fan(
    pieces: &[Piece],
    comp: &[usize],
    nodes: &[Vec3],
    root: usize,
) -> (Vec<Vec3>, Vec<f32>, Vec<Trail>) {
    let (dist, prev) = geodesic(pieces, comp, nodes.len(), root);
    let mut degree = vec![0_usize; nodes.len()];
    for &p in comp {
        degree[pieces[p].0] += 1;
        degree[pieces[p].1] += 1;
    }
    let mut ends: Vec<usize> = comp_nodes(pieces, comp)
        .into_iter()
        .filter(|&n| n != root && degree[n] == 1 && dist[n] < f32::MAX)
        .collect();
    // A component that is all ring and no end still has a farthest node.
    if ends.is_empty() {
        ends = comp_nodes(pieces, comp)
            .into_iter()
            .filter(|&n| n != root && dist[n] < f32::MAX)
            .collect();
    }
    let reach = |n: usize| nodes[n].distance(nodes[root]);
    ends.sort_by(|&a, &b| reach(b).total_cmp(&reach(a)));
    let Some(&tip) = ends.first() else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let (lead, lead_r) = geodesic_path(pieces, &prev, root, tip);
    let fan: Vec<(Vec<Vec3>, Vec<f32>)> = ends
        .iter()
        .skip(1)
        .take(16)
        .map(|&e| geodesic_path(pieces, &prev, root, e))
        .filter(|(p, _)| p.len() >= 2)
        .collect();
    (lead, lead_r, fan)
}

/// Should core `k`'s path be REVERSED so its FRONT/TOP is last? See [`Core`] for the two rules.
fn front_is_first(cores: &[Core], links: &[Link], k: usize) -> bool {
    let (a, b) = (cores[k].path[0], *cores[k].path.last().expect("non-empty"));
    if cores[k].upright {
        return a.z > b.z;
    }
    // A CHAINED core — the head on the end of a neck. The smallest one chained to this one wins:
    // a head is the small mass ahead, where a mounted torso is another trunk.
    let mut head: Option<(f32, Vec3)> = None;
    for l in links {
        let other = match (l.cores[0] == k, l.cores[1] == k) {
            (true, _) => (l.cores[1], l.ends[0]),
            (_, true) => (l.cores[0], l.ends[1]),
            _ => continue,
        };
        let size = cores[other.0].arc * cores[other.0].radius;
        if head.is_none_or(|(s, _)| size < s) {
            head = Some((size, other.1));
        }
    }
    if let Some((_, at)) = head {
        return at.distance(a) < at.distance(b);
    }
    // NO CHAINED CORE, AND SO NOTHING MEASURED SAYS WHICH END IS WHICH. The RIG's own forward
    // decides, which is the human's facing knob and not a guess here (69F4B20D, 3A61D440).
    //
    // A MIDLINE LIMB'S OWN THICKNESS USED TO DECIDE THIS and it is gone: "the heavy midline limb
    // is the head, the thin one is the tail" is ANATOMY (Aaron's law 513E5F78), and it is not
    // even true of the family — a beaver's, a crocodile's and a fox's tail all out-thicken their
    // heads. Measured on the 32 swept quadrupeds it turned THIRTEEN of them round (the Camel's
    // neck matched as its tail, the Goat's pelvis composed out past its nose), and every one of
    // those thirteen is faced the right way by the line below. Its predecessor — the LONGEST
    // midline limb is the tail — is dead for the same reason (906665EF).
    a.y < b.y
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// WHICH LIMB'S FLESH — the one question the SKIN BIND asks of the graph (`bake::bake_skin`): a
// limb's skin is its TUBE's flesh, so re-posing the limb carries nothing that is not the limb.
// ─────────────────────────────────────────────────────────────────────────────────────────────

impl ShapeGraph {
    /// WHICH LIMB'S FLESH `p` IS, and how much of it.
    ///
    /// Every piece of the graph is a string of MEDIAL BALLS — each point of its curve with the
    /// inscribed radius there — and `p` is flesh of the piece whose balls come nearest to holding
    /// it: the POWER distance `|p − c| − r(c)`, zero on a ball's skin and negative inside it. A point
    /// on a limb sits on that limb's balls and a body's width off the trunk's; a point on the belly
    /// the other way round. Nothing here knows what a leg is.
    ///
    /// A limb's own flesh is its TUBE: the balls of its LEAD, from `begin[l]` (arc along the lead
    /// from the attachment) on. A lead starts at a junction DEEP IN THE CORE, and the run before
    /// `begin` is trunk flesh however thin the class split called it. The rest of a limb's fan is
    /// not its tube — on a flat body the fan also takes the thin flat edges of the trunk that
    /// happened to join the limb's component, and a toe or a membrane is held by the lead's own
    /// balls near it well enough. `None` makes `l` a NEIGHBOUR: its whole curve, fan and all, counts
    /// as the rest of the body, and it is never the answer.
    ///
    /// Returns the limb with a `begin` whose tube comes nearest, and its SHARE of `p`: `1` where the
    /// tube holds `p` a band better than the rest of the body does (every core, every neighbour, and
    /// every lead's run before its own `begin`), `0` a band the other way, a straight ramp between
    /// — the JUNCTION, where a limb's flesh blends into the trunk's. The band is half the lead's own
    /// radius where the limb begins, which on a tube puts whole-limb flesh about one radius past its
    /// begin and none about one radius before it. Two limbs that both have a `begin` never shorten
    /// each other's share: tubes that touch meet at a seam, they do not hand the seam to the trunk.
    /// `None` when no such limb holds any share of `p`.
    pub fn limb_membership(&self, p: Vec3, begin: &[Option<f32>]) -> Option<(usize, f32)> {
        let power = |c: &Vec3, r: &f32| p.distance(*c) - r;
        let mut rest = self
            .cores
            .iter()
            .flat_map(|k| k.path.iter().zip(&k.radii))
            .map(|(c, r)| power(c, r))
            .fold(f32::INFINITY, f32::min);
        // The nearest tube with a `begin`: (limb, its own nearest ball, its band).
        let mut best: Option<(usize, f32, f32)> = None;
        for (l, limb) in self.limbs.iter().enumerate() {
            let Some(from) = begin.get(l).copied().flatten() else {
                for (path, radii) in std::iter::once((&limb.lead, &limb.lead_r))
                    .chain(limb.fan.iter().map(|(p, r)| (p, r)))
                {
                    for (c, r) in path.iter().zip(radii) {
                        rest = rest.min(power(c, r));
                    }
                }
                continue;
            };
            let (mut own, mut band, mut arc) = (f32::INFINITY, None, 0.0);
            for (k, (c, r)) in limb.lead.iter().zip(&limb.lead_r).enumerate() {
                if k > 0 {
                    arc += limb.lead[k - 1].distance(*c);
                }
                if arc >= from {
                    own = own.min(power(c, r));
                    band.get_or_insert(0.5 * r);
                } else {
                    rest = rest.min(power(c, r));
                }
            }
            if best.is_none_or(|b| own < b.1) {
                best = Some((l, own, band.unwrap_or(0.0)));
            }
        }
        let (l, own, band) = best?;
        let share = (0.5 + (rest - own) / (2.0 * band.max(self.cell))).clamp(0.0, 1.0);
        (share > 0.0).then_some((l, share))
    }

    /// How many graphs THIS THREAD has read off a flesh field so far ([`ShapeGraph::build`] is a
    /// mesh-wide thinning pass). A body is read ONCE and the read is passed on — the fit's
    /// [`Body`] through `conform::rig_raw_mesh` to the skin bind, the stance normaliser and the
    /// bench's rail — and this count is what that promise is gated by.
    pub fn reads() -> u64 {
        READS.with(std::cell::Cell::get)
    }
}

thread_local! {
    /// The count behind [`ShapeGraph::reads`].
    static READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// THE BODY AS READ — one mesh's flesh and the graph thinned from it, read ONCE and passed on.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A MESH'S BODY AS READ: its body-masked [`Flesh`] and the [`ShapeGraph`] thinned from it (`None`
/// when the flesh holds no structure to thin). Both are mesh-wide passes over the SAME geometry, so
/// the body is read once — by the fit (`conform::fit_baseline_to_mesh`, which hands it back in its
/// `FitReport`) — and every later question about that mesh is asked of this read: which limb's flesh
/// a vertex is (the skin bind), where a standing limb's FOOT is and how high it stands
/// (`bake::Foot`), and the silhouette a squared limb is held to. Only moving the mesh's vertices
/// makes a read stale; moving its bones never does.
pub struct Body {
    pub flesh: Flesh,
    pub graph: Option<ShapeGraph>,
}

impl Body {
    /// READ `model`'s body: its [`Flesh::build_body`] field and the graph thinned from it.
    pub fn read(model: &crate::fbx::RawModel) -> Body {
        let flesh = Flesh::build_body(model);
        let graph = ShapeGraph::build(&flesh);
        Body { flesh, graph }
    }
}

impl std::fmt::Debug for Body {
    /// The read in one line — the field's cell and the graph's own summary, never the voxels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body")
            .field("cell", &self.flesh.cell())
            .field("graph", &self.graph.as_ref().map(ShapeGraph::summary))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::RawModel;
    use crate::flesh::fixtures::{box_mesh, merge, tube};

    /// A box between two corners — [`crate::flesh::fixtures::box_mesh`] in Vec3 terms.
    fn boxy(lo: Vec3, hi: Vec3) -> RawModel {
        box_mesh(lo.x, hi.x, lo.y, hi.y, lo.z, hi.z)
    }

    /// A round tube of constant radius from `a` to `b`.
    fn rod(a: Vec3, b: Vec3, r: f32) -> RawModel {
        tube(&[a, b], &[r, r])
    }

    fn graph(m: &RawModel) -> ShapeGraph {
        ShapeGraph::build(&Flesh::build(m)).expect("a body with flesh in it has a graph")
    }

    /// A BOX QUADRUPED: a barrel, four legs at its corners, a tail behind and a head box in front
    /// on a neck. No anatomy anywhere — the graph must find the structure from the shape alone.
    fn box_quadruped() -> RawModel {
        let mut parts = vec![boxy(
            Vec3::new(-14.0, -40.0, 60.0),
            Vec3::new(14.0, 40.0, 92.0),
        )];
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let (x, y) = (sx * 10.0_f32, sy * 32.0_f32);
            parts.push(rod(Vec3::new(x, y, 0.0), Vec3::new(x, y, 66.0), 4.0));
        }
        // The tail behind, and a neck carrying a head box forward.
        parts.push(rod(
            Vec3::new(0.0, 38.0, 78.0),
            Vec3::new(0.0, 88.0, 78.0),
            3.5,
        ));
        parts.push(rod(
            Vec3::new(0.0, -38.0, 84.0),
            Vec3::new(0.0, -62.0, 92.0),
            5.0,
        ));
        parts.push(boxy(
            Vec3::new(-9.0, -86.0, 84.0),
            Vec3::new(9.0, -60.0, 104.0),
        ));
        merge(parts)
    }

    #[test]
    fn a_box_quadruped_reads_as_a_core_chain_with_two_pairs_and_one_rear_midline_tube() {
        let g = graph(&box_quadruped());
        // A barrel and a head — two cores, chained by the neck.
        assert_eq!(g.cores.len(), 2, "barrel + head: {}", g.summary());
        assert!(!g.cores[0].upright, "a barrel lies down: {}", g.summary());
        assert!(!g.links.is_empty(), "the neck chains them: {}", g.summary());
        assert_eq!(g.pairs.len(), 2, "two leg pairs: {}", g.summary());
        // One pair near each end of the barrel, and both pairs on the ground.
        let mut ts: Vec<f32> = g.pairs.iter().map(|p| p.t).collect();
        ts.sort_by(f32::total_cmp);
        assert!(ts[0] < 0.35 && ts[1] > 0.65, "a pair at each end: {ts:?}");
        for p in &g.pairs {
            assert!(
                g.limbs[p.l].grounded && g.limbs[p.r].grounded,
                "legs reach the ground: {}",
                g.summary()
            );
        }
        // The tail: an unpaired midline limb at the REAR (t near 0 — the front is the head end).
        let tail = g
            .unpaired()
            .into_iter()
            .map(|i| &g.limbs[i])
            .filter(|l| l.side == 0.0 && !l.grounded)
            .max_by(|a, b| a.arc.total_cmp(&b.arc))
            .expect("a rear midline tube");
        assert!(tail.t < 0.3, "the tail is at the rear: t {}", tail.t);
    }

    #[test]
    fn a_standing_biped_puts_its_pairs_at_the_bottom_and_the_top_of_an_upright_core() {
        let mut parts = vec![boxy(
            Vec3::new(-16.0, -9.0, 80.0),
            Vec3::new(16.0, 9.0, 140.0),
        )];
        for sx in [1.0_f32, -1.0] {
            parts.push(rod(
                Vec3::new(sx * 9.0, 0.0, 0.0),
                Vec3::new(sx * 9.0, 0.0, 88.0),
                5.0,
            ));
            parts.push(rod(
                Vec3::new(sx * 12.0, 0.0, 132.0),
                Vec3::new(sx * 44.0, 0.0, 132.0),
                4.5,
            ));
        }
        parts.push(rod(
            Vec3::new(0.0, 0.0, 136.0),
            Vec3::new(0.0, 0.0, 154.0),
            5.0,
        ));
        parts.push(boxy(
            Vec3::new(-11.0, -11.0, 152.0),
            Vec3::new(11.0, 11.0, 176.0),
        ));
        let g = graph(&merge(parts));
        assert!(g.cores[0].upright, "a standing trunk: {}", g.summary());
        assert_eq!(g.pairs.len(), 2, "legs + arms: {}", g.summary());
        let legs = g
            .pairs
            .iter()
            .find(|p| g.limbs[p.l].grounded)
            .expect("a grounded pair");
        let arms = g
            .pairs
            .iter()
            .find(|p| !g.limbs[p.l].grounded)
            .expect("a raised pair");
        assert!(
            legs.t < arms.t,
            "legs below the arms on an upright core: {} vs {}",
            legs.t,
            arms.t
        );
        assert!(
            legs.t < 0.35 && arms.t > 0.6,
            "at the ends: {legs:?} {arms:?}"
        );
    }

    #[test]
    fn a_seven_trunk_monster_reads_seven_cores_and_fourteen_pairs_in_order() {
        let mut parts: Vec<RawModel> = Vec::new();
        for k in 0..7 {
            let y = k as f32 * 52.0;
            parts.push(boxy(
                Vec3::new(-13.0, y - 18.0, 60.0),
                Vec3::new(13.0, y + 18.0, 88.0),
            ));
            if k > 0 {
                // A WAIST between torsos — what makes them seven and not one.
                parts.push(rod(
                    Vec3::new(0.0, y - 36.0, 74.0),
                    Vec3::new(0.0, y - 16.0, 74.0),
                    4.5,
                ));
            }
            for (sx, sy) in [(1.0_f32, 1.0_f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                parts.push(rod(
                    Vec3::new(sx * 8.0, y + sy * 12.0, 68.0),
                    Vec3::new(sx * 40.0, y + sy * 12.0, 68.0),
                    3.5,
                ));
            }
        }
        let g = graph(&merge(parts));
        assert_eq!(g.cores.len(), 7, "seven torsos: {}", g.summary());
        assert_eq!(g.pairs.len(), 14, "28 arms in 14 pairs: {}", g.summary());
        assert_eq!(g.links.len(), 6, "six waists chain them: {}", g.summary());
        // Every core carries exactly two pairs.
        for k in 0..7 {
            assert_eq!(
                g.pairs.iter().filter(|p| p.core == k).count(),
                2,
                "core {k} carries two pairs: {}",
                g.summary()
            );
        }
    }

    /// THE GATE ON THE DELETED THICKNESS RULE. A headless body with a FAT tail behind and a
    /// THIN stub in front: nothing measured says which end is which, so the rig's own forward
    /// does (69F4B20D). Under the rule this replaces — "the thickest midline limb is the head" —
    /// the tail won and the animal was composed backwards, which is what turned thirteen of the
    /// thirty-two swept quadrupeds round.
    #[test]
    fn a_fat_tail_behind_a_headless_body_does_not_turn_it_round() {
        let mut parts = vec![boxy(
            Vec3::new(-14.0, -40.0, 60.0),
            Vec3::new(14.0, 40.0, 92.0),
        )];
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let (x, y) = (sx * 10.0_f32, sy * 32.0_f32);
            parts.push(rod(Vec3::new(x, y, 0.0), Vec3::new(x, y, 66.0), 4.0));
        }
        // A FAT tail at +y (the rear) and a THIN one at −y (the front). No head core either way.
        parts.push(rod(
            Vec3::new(0.0, 38.0, 78.0),
            Vec3::new(0.0, 96.0, 78.0),
            8.0,
        ));
        parts.push(rod(
            Vec3::new(0.0, -38.0, 78.0),
            Vec3::new(0.0, -96.0, 78.0),
            3.0,
        ));
        let g = graph(&merge(parts));
        let core = &g.cores[0];
        assert!(
            g.links.is_empty(),
            "no chained core, so nothing measured decides"
        );
        assert!(
            core.front().y < core.rear().y,
            "with nothing measured to say which end is which the RIG's forward decides, so the \
             front is at −y: rear {:?} front {:?}",
            core.rear().to_array(),
            core.front().to_array()
        );
    }

    /// THE GATE ON GRAVITY PICKING A LIMB'S END. A leg that forks near the bottom into a foot ON
    /// THE FLOOR and a slightly LONGER spur that stops short: the lead is the one on the floor,
    /// because "the ground joint at the tube's end" only holds when the tube's end is the end
    /// that reaches the ground.
    #[test]
    fn a_leg_leads_with_the_end_on_the_floor_and_not_the_longer_spur() {
        let parts = vec![
            boxy(Vec3::new(-14.0, -40.0, 60.0), Vec3::new(14.0, 40.0, 92.0)),
            // The leg proper, down to the floor.
            rod(Vec3::new(10.0, 30.0, 0.0), Vec3::new(10.0, 30.0, 66.0), 4.0),
            // A spur off it at knee height, longer than what is left of the leg below, and it
            // stops well clear of the floor.
            rod(
                Vec3::new(10.0, 30.0, 34.0),
                Vec3::new(10.0, 78.0, 26.0),
                3.5,
            ),
            // ...and its twin, so the body is symmetric and the walk has a pair to read.
            rod(
                Vec3::new(-10.0, 30.0, 0.0),
                Vec3::new(-10.0, 30.0, 66.0),
                4.0,
            ),
            rod(
                Vec3::new(-10.0, 30.0, 34.0),
                Vec3::new(-10.0, 78.0, 26.0),
                3.5,
            ),
        ];
        let g = graph(&merge(parts));
        let leg = g
            .limbs
            .iter()
            .filter(|l| l.side > 0.0)
            .max_by(|a, b| a.arc.total_cmp(&b.arc))
            .expect("the fixture has a right-hand limb");
        assert!(
            leg.grounded,
            "the limb reaches the ground, tip {:?}",
            leg.tip().to_array()
        );
        assert!(
            leg.tip().z - g.floor <= GROUND_BAND * g.height,
            "its LEAD ends on the floor and not up the spur, tip {:?} floor {:.1}",
            leg.tip().to_array(),
            g.floor
        );
    }

    #[test]
    fn a_legless_trunk_with_a_long_tail_is_one_core_and_one_midline_tube() {
        let g = graph(&merge(vec![
            boxy(Vec3::new(-13.0, -30.0, 40.0), Vec3::new(13.0, 30.0, 68.0)),
            rod(Vec3::new(0.0, 28.0, 54.0), Vec3::new(0.0, 150.0, 54.0), 4.0),
        ]));
        assert_eq!(g.cores.len(), 1, "one body: {}", g.summary());
        assert!(g.pairs.is_empty(), "no limb pairs: {}", g.summary());
        let tail = g
            .unpaired()
            .into_iter()
            .map(|i| &g.limbs[i])
            .max_by(|a, b| a.arc.total_cmp(&b.arc))
            .expect("the tail");
        assert_eq!(tail.side, 0.0, "the tail is on the midline");
        assert!(tail.arc > 60.0, "a long tail: {} cm", tail.arc);
    }

    #[test]
    fn two_flat_wings_at_the_front_top_read_as_sheets_with_leading_paths() {
        let mut parts = vec![boxy(
            Vec3::new(-9.0, -18.0, 56.0),
            Vec3::new(9.0, 18.0, 92.0),
        )];
        for sx in [1.0_f32, -1.0] {
            // A SPREAD WING: broad in span AND chord, a few cells thin — a plate, not a tube.
            let (x0, x1) = (sx * 7.0, sx * 66.0);
            parts.push(boxy(
                Vec3::new(x0.min(x1), -22.0, 82.0),
                Vec3::new(x0.max(x1), 18.0, 86.0),
            ));
        }
        let g = graph(&merge(parts));
        let sheets: Vec<&Limb> = g.limbs.iter().filter(|l| l.sheet).collect();
        assert_eq!(
            sheets.len(),
            2,
            "two wings: {}\n{}",
            g.summary(),
            g.detail()
        );
        for s in &sheets {
            assert!(
                s.side != 0.0,
                "a wing has a side of its own\n{}",
                g.detail()
            );
            assert!(
                s.arc > 40.0,
                "the leading path spans the wing: {:.0} cm\n{}",
                s.arc,
                g.detail()
            );
        }
        assert_eq!(
            sheets[0].side * sheets[1].side,
            -1.0,
            "one wing each side: {}",
            g.summary()
        );
        // ...and they are a PAIR, which is what a recipe's wing modules are matched to.
        assert_eq!(
            g.pairs.len(),
            1,
            "the wings pair: {}\n{}",
            g.summary(),
            g.detail()
        );
        assert!(
            g.limbs[g.pairs[0].l].sheet && g.limbs[g.pairs[0].r].sheet,
            "the pair is the two sheets: {}",
            g.detail()
        );
    }

    /// Every lead point of `l` on its own side of the plane, a cell of grid noise allowed — a
    /// limb's lead never crosses the plane.
    fn lead_keeps_its_side(g: &ShapeGraph, l: &Limb) -> bool {
        l.lead.iter().all(|p| (p.x - g.plane_x) * l.side >= -g.cell)
    }

    /// THE GATE ON THE PLANE CUT. Two hind tubes joined by a low bar between them are ONE thin
    /// component, walked from one attachment into one limb with a leg at each end of it — the
    /// merged second pair the fresh sweep found on a third of the bodies. Its flesh reaches the
    /// floor on both sides of the plane, so it is cut there: two limbs, each on its own side,
    /// each led down its own tube, and they pair.
    #[test]
    fn two_hind_tubes_joined_by_a_low_belly_bar_are_two_limbs_and_one_pair() {
        let mut parts = vec![boxy(
            Vec3::new(-14.0, -40.0, 60.0),
            Vec3::new(14.0, 40.0, 92.0),
        )];
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let (x, y) = (sx * 10.0_f32, sy * 32.0_f32);
            parts.push(rod(Vec3::new(x, y, 0.0), Vec3::new(x, y, 66.0), 4.0));
        }
        // THE BAR: the two hind legs joined under the belly.
        parts.push(rod(
            Vec3::new(-10.0, 32.0, 44.0),
            Vec3::new(10.0, 32.0, 44.0),
            3.5,
        ));
        let g = graph(&merge(parts));
        assert_eq!(
            g.pairs.len(),
            2,
            "hind and fore: {}\n{}",
            g.summary(),
            g.detail()
        );
        let hind = g
            .pairs
            .iter()
            .find(|p| g.limbs[p.l].at.y > 0.0)
            .expect("a pair under the rear of the barrel");
        for i in [hind.l, hind.r] {
            let l = &g.limbs[i];
            assert!(l.grounded, "each hind limb stands:\n{}", g.detail());
            assert!(
                lead_keeps_its_side(&g, l),
                "a hind limb's lead crosses the plane:\n{}",
                g.detail()
            );
        }
        assert_eq!(
            g.limbs[hind.l].side * g.limbs[hind.r].side,
            -1.0,
            "one hind limb each side:\n{}",
            g.detail()
        );
    }

    /// THE GATE ON THE SPUR RULE UNDER A LOW-SLUNG BODY. A barrel 60 cm thick on legs that stand
    /// 16 cm under its belly: every leg is shorter than the barrel's own inscribed radius, which
    /// is exactly what the spur rule calls a bump. A limb that holds the body off the floor is not
    /// one — the legs are found, and pair.
    #[test]
    fn a_low_slung_body_stands_on_legs_shorter_than_its_barrel_is_thick() {
        let mut parts = vec![boxy(
            Vec3::new(-30.0, -60.0, 16.0),
            Vec3::new(30.0, 60.0, 96.0),
        )];
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let (x, y) = (sx * 20.0_f32, sy * 45.0_f32);
            parts.push(rod(Vec3::new(x, y, 0.0), Vec3::new(x, y, 20.0), 5.0));
        }
        let g = graph(&merge(parts));
        assert!(
            16.0 < g.max_radius,
            "the fixture's legs are shorter than the barrel is thick: {:.1}",
            g.max_radius
        );
        let standing = g.limbs.iter().filter(|l| l.grounded).count();
        assert_eq!(standing, 4, "four legs: {}\n{}", g.summary(), g.detail());
        assert_eq!(
            g.pairs.len(),
            2,
            "two pairs: {}\n{}",
            g.summary(),
            g.detail()
        );
    }

    /// THE GATE ON THE STRIDE. Every source is generated mid-stride: one hind hoof 30 cm ahead of
    /// its socket, its twin 30 cm behind. Reflected across the plane the two tips land 60 cm
    /// apart — twice what a half-length mirror allows — yet across the plane and against gravity
    /// they agree exactly, and that is the pair.
    #[test]
    fn a_pair_in_mid_stride_is_still_a_pair() {
        let mut parts = vec![boxy(
            Vec3::new(-14.0, -40.0, 60.0),
            Vec3::new(14.0, 40.0, 92.0),
        )];
        for sx in [1.0_f32, -1.0] {
            let x = sx * 10.0;
            parts.push(rod(
                Vec3::new(x, -32.0, 0.0),
                Vec3::new(x, -32.0, 66.0),
                4.0,
            ));
            // The hind legs swing opposite ways from the same socket.
            parts.push(rod(
                Vec3::new(x, 32.0 + sx * 30.0, 0.0),
                Vec3::new(x, 32.0, 66.0),
                4.0,
            ));
        }
        let g = graph(&merge(parts));
        assert_eq!(
            g.pairs.len(),
            2,
            "hind and fore: {}\n{}",
            g.summary(),
            g.detail()
        );
        let hind = g
            .pairs
            .iter()
            .find(|p| g.limbs[p.l].at.y > 0.0)
            .expect("the striding pair");
        let (a, b) = (g.limbs[hind.l].tip(), g.limbs[hind.r].tip());
        assert!(
            (a.y - b.y).abs() > 40.0,
            "the fixture strides: tips {a} and {b}"
        );
    }

    /// THE GATE ON A BULGE THAT READS AS A CORE. A slim standing body whose left knee is as thick
    /// as half its thickest flesh: the knee is a small core of its own, chained to the torso by
    /// the thigh, and the shin hangs off it — a leg split across two cores pairs with nothing.
    /// A core chained to one larger core that carries nothing but limbs to the floor is a bulge
    /// on a leg: it is read as thin, and the legs pair.
    #[test]
    fn a_knee_as_thick_as_half_the_body_is_a_bulge_on_the_leg_not_a_trunk() {
        let mut parts = vec![boxy(
            Vec3::new(-16.0, -9.0, 80.0),
            Vec3::new(16.0, 9.0, 140.0),
        )];
        for sx in [1.0_f32, -1.0] {
            parts.push(rod(
                Vec3::new(sx * 9.0, 0.0, 0.0),
                Vec3::new(sx * 9.0, 0.0, 88.0),
                5.0,
            ));
            parts.push(rod(
                Vec3::new(sx * 12.0, 0.0, 132.0),
                Vec3::new(sx * 44.0, 0.0, 132.0),
                4.5,
            ));
        }
        // THE BULGE, on one knee only: the leg swelling to half again its own thickness.
        parts.push(tube(
            &[
                Vec3::new(-9.0, 0.0, 30.0),
                Vec3::new(-9.0, 0.0, 44.0),
                Vec3::new(-9.0, 0.0, 58.0),
            ],
            &[5.0, 7.5, 5.0],
        ));
        parts.push(rod(
            Vec3::new(0.0, 0.0, 136.0),
            Vec3::new(0.0, 0.0, 154.0),
            5.0,
        ));
        parts.push(boxy(
            Vec3::new(-11.0, -11.0, 152.0),
            Vec3::new(11.0, 11.0, 176.0),
        ));
        let g = graph(&merge(parts));
        assert_eq!(
            g.cores.len(),
            2,
            "a torso and a head, no knee: {}\n{}",
            g.summary(),
            g.detail()
        );
        assert_eq!(
            g.pairs.len(),
            2,
            "legs + arms: {}\n{}",
            g.summary(),
            g.detail()
        );
        let legs = g
            .pairs
            .iter()
            .find(|p| g.limbs[p.l].grounded && g.limbs[p.r].grounded)
            .expect("the legs pair");
        assert_eq!(legs.core, 0, "on the torso: {}", g.detail());
    }

    /// `m` with every vertex moved by up to `amp` along each axis — the same point always the
    /// same way (keyed on its own position and on `k`), so a closed surface stays closed.
    fn jittered(m: &RawModel, amp: f32, k: u64) -> RawModel {
        let mut out = m.clone();
        let unit = |p: [f32; 3], axis: u64| -> f32 {
            let mut s: u64 = 0x9E37_79B9_7F4A_7C15 ^ k.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ axis;
            for v in p {
                s ^= u64::from(v.to_bits()).wrapping_mul(0x2545_F491_4F6C_DD1D);
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
            }
            (s >> 11) as f32 / (1_u64 << 53) as f32 * 2.0 - 1.0
        };
        for v in &mut out.vertices {
            let p = v.p;
            v.p = [0, 1, 2].map(|a| p[a] + amp * unit(p, a as u64));
        }
        out
    }

    /// THE GATE ON A READ THAT HOLDS STILL (A31C0FAE: three bodies lost a pair when a mesh whose
    /// only change was its turned head was read again). A box quadruped whose right hind leg
    /// steps in under the body, its hoof back past the plane: that leg's lead averages inside the
    /// midline band, so its side — and the pair — hung on a sub-cell. Read seven times, every
    /// vertex jittered by up to 0.3 of a cell: the same cores, limbs and pairs every time, and the
    /// stepping leg in its pair every time.
    #[test]
    fn a_read_holds_its_counts_under_a_third_of_a_cell_of_jitter() {
        let mut parts = vec![boxy(
            Vec3::new(-14.0, -40.0, 60.0),
            Vec3::new(14.0, 40.0, 92.0),
        )];
        for sx in [1.0_f32, -1.0] {
            parts.push(rod(
                Vec3::new(sx * 10.0, -32.0, 0.0),
                Vec3::new(sx * 10.0, -32.0, 66.0),
                4.0,
            ));
        }
        parts.push(rod(
            Vec3::new(-10.0, 32.0, 0.0),
            Vec3::new(-10.0, 32.0, 66.0),
            4.0,
        ));
        parts.push(tube(
            &[
                Vec3::new(8.0, 32.0, 66.0),
                Vec3::new(3.0, 42.0, 34.0),
                Vec3::new(-2.0, 52.0, 0.0),
            ],
            &[4.0, 3.8, 3.6],
        ));
        parts.push(rod(
            Vec3::new(0.0, 38.0, 78.0),
            Vec3::new(0.0, 88.0, 78.0),
            3.5,
        ));
        let m = merge(parts);
        let cell = Flesh::build(&m).cell();
        let counts = |g: &ShapeGraph| (g.cores.len(), g.limbs.len(), g.pairs.len());
        let first = graph(&m);
        assert_eq!(
            first.pairs.len(),
            2,
            "the leg stepping under the body pairs with its twin: {}\n{}",
            first.summary(),
            first.detail()
        );
        for k in 1..7 {
            let g = graph(&jittered(&m, 0.3 * cell, k));
            assert_eq!(
                counts(&g),
                counts(&first),
                "read {k} jittered 0.3 cell: {}\n{}",
                g.summary(),
                g.detail()
            );
        }
    }

    /// THE GATE ON THE MIRROR BEFORE THE ATTACHMENTS. Two hind legs whose hooves mirror each
    /// other exactly — but the left one's thigh runs forward along the belly and joins the barrel
    /// halfway along it, so its junction lands half the core from its twin's (the Squirrel's and
    /// the goats' junctions wander that far between two reads a sub-cell apart). Tips that mirror
    /// WHOLE are a pair wherever the thinning put the junctions.
    #[test]
    fn a_pair_whose_tips_mirror_whole_is_a_pair_wherever_its_junctions_land() {
        let mut parts = vec![boxy(
            Vec3::new(-14.0, -40.0, 60.0),
            Vec3::new(14.0, 40.0, 92.0),
        )];
        for sx in [1.0_f32, -1.0] {
            parts.push(rod(
                Vec3::new(sx * 10.0, -32.0, 0.0),
                Vec3::new(sx * 10.0, -32.0, 66.0),
                4.0,
            ));
        }
        parts.push(rod(
            Vec3::new(10.0, 32.0, 0.0),
            Vec3::new(10.0, 32.0, 66.0),
            4.0,
        ));
        parts.push(tube(
            &[
                Vec3::new(-10.0, 0.0, 64.0),
                Vec3::new(-10.0, 18.0, 44.0),
                Vec3::new(-10.0, 32.0, 0.0),
            ],
            &[4.0, 4.0, 4.0],
        ));
        let g = graph(&merge(parts));
        let hind = g
            .limbs
            .iter()
            .position(|l| l.tip().y > 20.0 && l.tip().x < 0.0)
            .expect("the left hind leg is a limb");
        let twin = g
            .limbs
            .iter()
            .position(|l| l.tip().y > 20.0 && l.tip().x > 0.0)
            .expect("the right hind leg is a limb");
        assert!(
            (g.limbs[hind].t - g.limbs[twin].t).abs() > PAIR_ARC,
            "the fixture's junctions land apart: {}",
            g.detail()
        );
        assert!(
            g.pairs
                .iter()
                .any(|p| (p.l, p.r) == (twin, hind) || (p.l, p.r) == (hind, twin)),
            "the hind legs pair: {}\n{}",
            g.summary(),
            g.detail()
        );
    }

    #[test]
    fn the_graph_of_a_128_cubed_field_is_read_in_about_a_second() {
        let m = box_quadruped();
        let flesh = Flesh::build(&m);
        let started = Instant::now();
        let g = ShapeGraph::build(&flesh).expect("a graph");
        let ms = started.elapsed().as_millis();
        println!("shape graph: {} ({ms} ms wall)", g.summary());
        assert!(ms < 4000, "the shape graph took {ms} ms");
    }
}
