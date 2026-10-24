//! **THE EPOCHS — the stone ocean moving.** Aaron 2026-10-15: *"layers of
//! waves, like an ocean made of stone moving at a geological scale"*; *"start
//! adding some of the continental motion and movement and work through the
//! compression and compaction physics … as epoch ticks."*
//!
//! The composed relief ([`crate::recipe::compose`]) is the initial state: the
//! shape era's planet, before the ledger era's erosion. Its crust is a field
//! of PARCELS, one per tile at epoch zero, each carrying its relief exactly;
//! the parcels MOVE and the tiles show whichever parcel stands on them, so the
//! waves travel intact — never resampled, never planed (rule: no erosion
//! without hardness).
//!
//! **Two kinds of crust.** The LAND MASSES the waves made — every connected
//! body of relief above the sea, with its shelf — are RAFTS: rigid, each
//! turning as one about the pole its push implies. The OCEAN FLOOR is fluid:
//! every floor parcel drifts with the push under it. The push is the molten
//! seams' (457D4F42: *"cells have no innate motion — the seams push"*),
//! downhill on the heat, and the heat BREATHES over the epochs (the seam
//! field's own temporal frequencies), so the motion reorganizes and the world
//! never freezes. Nothing is a Voronoi cell; no boundary is rolled — a first
//! cut rolled twelve plates whose edges cut the wave-made continents into
//! pieces that slid apart: *"it just cracks the world"*.
//!
//! **Compression.** Where parcels crowd — more of them within a ring than the
//! ring holds — floor goes UNDER: the lowest floor parcel is consumed, once,
//! and its slab thickens the crust standing there — on a LAND margin the
//! isostatic share (`uplift`) stands up across the overriding raft's parcels
//! in the neighbourhood, evenly, each up to the crust's ceiling (`summit`:
//! the most any column can be stood up — crust as thick as crust gets stops
//! rising and roots instead; Tibet widens, it does not spike), the rest
//! deepening the ROOT (a ledger: nothing vanishes); floor under floor roots the whole slab,
//! since island arcs are volcanic and the emitters' to raise. Floor under a
//! raft's edge is the Andes: a belt as wide as the neighbourhood, never one
//! column — a first cut stacked every slab on the single highest parcel and
//! after a thousand epochs 185 towers stood at up to 0.9 of the radius, hexes
//! floating in space over Aaron's globe. A FRAGMENT (a raft smaller than
//! `MIN_RAFT_PARCELS`, an island) takes no stand either; it docks when it
//! collides. Two RAFTS crowding have collided: they WELD into one raft and
//! move as one — continents do not go under each other.
//!
//! **Rifting.** A raft whose push pulls it two ways harder than `rift` of the
//! rate SPLITS along the line between the two pulls — Pangea's seam — and the
//! halves drift apart, young floor opening between them; a range that crossed
//! the line now starts on one continent and continues on the other.
//!
//! **Young floor.** Where no parcel stands, the crust has pulled apart: a new
//! floor PARCEL is born there at once, at ridge depth, wearing the recipe's
//! ocean layers as its texture — the waves of the sea floor, never a flat
//! slab — and joins the flow; and it SUBSIDES as it ages — with the square
//! root of its age, as cooling lithosphere does — toward THIS WORLD'S abyss
//! (the depth of the composed ocean's deep floor, read once when the epochs
//! stand up). Two earlier cuts: floor cooled toward 2.5× the ridge, a quarter
//! of the composed abyss, and the recycled sea floor shallowed into a flat
//! bank; and an opened tile stayed a parcel-less "young floor" for twenty
//! epochs before it spawned, so a five-tile band of shallow bank trailed every
//! ridge and every moving coast — fifteen percent of the planet, a speckle.
//!
//! Heights are fractions of the radius throughout. A tick is an epoch — a
//! geological unit of motion, not a clock. Transformations only (935269B7).

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::crust::CrustField;
use crate::map::{HexMap, TileId};
use crate::rocks::Rock;
use crate::recipe::{
    floor_texture, Composed, Nearest, Recipe,
};
use crate::resonance::{crater, Emitter, VolcanoShape, CRATER_REACH, VOLCANO_REACH};
use crate::seams::{push_field, SeamField};

/// **The epoch dials** — as data, beside the recipe.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpochRecipe {
    /// How far the fastest-pushed crust moves per tick, in TILES.
    pub rate: f32,
    /// The ISOSTATIC share: of the crust that goes under a parcel, how much
    /// stands up on it; the rest deepens the parcel's root. 0.2 is the
    /// floating-crust value; 1 stands the whole slab up.
    pub uplift: f32,
    /// Young floor's depth below the sea at the ridge, fraction of radius.
    pub ridge: f32,
    /// RIFTING: a raft splits when its push pulls it apart by more than this
    /// share of the rate (the mean residual push over the raft, after its
    /// rigid turn is taken out). Lower rifts sooner.
    pub rift: f32,
    /// The crust's CEILING: how much compression can stand up on any one
    /// column (fraction of radius) before that crust is as thick as crust
    /// gets and what goes under roots instead. A bound on the LIFT, never on
    /// the relief: the carried waves ride the lifted belt exactly, and a belt
    /// at the ceiling is the composed relief raised by the ceiling — Tibet
    /// with its hills on it, never a planed plateau.
    pub summit: f32,
    /// IMPACTS: how many strikes fall per thousand epochs (their sizes a
    /// power law from two tiles to [`IMPACT_LARGEST`]).
    pub impacts: f32,
}

impl Default for EpochRecipe {
    fn default() -> Self {
        Self {
            rate: 0.25,
            uplift: 0.2,
            ridge: 0.004,
            rift: 0.6,
            summit: 0.02,
            impacts: 4.0,
        }
    }
}

/// The largest impact's crater radius, radians — a strike whose wave covers
/// an eighth of the planet (Aaron's Chicxulub) — and the smallest, in tiles.
pub const IMPACT_LARGEST: f32 = 0.5;
pub const IMPACT_SMALLEST_TILES: f32 = 2.0;
/// The largest crater's bowl depth, fraction of radius; smaller craters are
/// shallower by the square root of their radius ratio.
pub const IMPACT_DEPTH: f32 = 0.012;
/// An eruption DROP: one event's waves reach this share of the standing
/// emitter's full-heat radius and stand this share of its height; a vent
/// erupts at a repush with this chance times its heat.
pub const DROP_REACH: f32 = 0.5;
pub const DROP_SHARE: f32 = 0.15;
pub const DROP_CHANCE: f32 = 0.5;

/// The composed ocean's ABYSS is read at this quantile of the composed relief
/// (the deep floor's depth below the sea) — where young floor cools to.
pub const ABYSS_QUANTILE: f32 = 0.1;
/// Floor this old (epochs) has cooled all the way to the abyss; depth grows
/// with the square root of age until then.
pub const FLOOR_COOLING_TICKS: f32 = 250.0;
/// The shelf: relief within this depth below the sea belongs to the land mass
/// beside it (fraction of radius).
pub const SHELF_DEPTH: f32 = 0.004;
/// Parcels beyond a two-ring neighbourhood's tiles that count as crowding.
const CROWD: usize = 4;
/// The oceanic SLAB a consumed floor parcel carries down, as crust (fraction
/// of radius): sea floor lies below the ridge, so its height above young floor
/// is nothing — what thickens the overriding margin is the slab itself (the
/// wedge and the arc), of which the isostatic share stands up.
pub const OCEAN_SLAB: f32 = 0.006;
/// How far past a tile's own parcels the shown parcel may be fetched, in
/// tile widths. A drifting lattice sampled by nearest tile leaves gaps whose
/// nearest parcel is within ~0.6 tiles; a tile just OUTSIDE a raft's edge has
/// its nearest raft parcel a full tile away — fetching that one dressed every
/// coast in a flickering fringe of one-tile islands. The FLOOR is fluid and
/// spreads: a floor parcel is fetched from farther, so the gaps a stretching
/// lattice leaves show the floor beside them, and young floor opens only where
/// the crust has truly pulled apart (one ring fetched 2 000 tiles of young
/// floor a tick as sampling noise, specks of shallow bank over every abyss).
const SNAP: f32 = 0.75;
const FLOOR_SNAP: f32 = 1.25;
/// The push is re-derived from the breathing heat this often (epochs), and
/// one epoch is this many of the heat's own ticks.
const REPUSH_EVERY: u64 = 10;
const HEAT_TICKS_PER_EPOCH: u64 = 12;
/// A raft smaller than this never splits.
const MIN_RIFT_PARCELS: usize = 120;
/// A raft smaller than this is a FRAGMENT — an island riding the flow: it
/// takes no stand-up when floor goes under it (an arc is volcanic) and docks
/// by welding when it collides.
pub const MIN_RAFT_PARCELS: usize = 12;

/// **The rafts**: per tile, the land mass (with its shelf) the tile belongs
/// to, or `u32::MAX` for ocean floor — connected components of the relief
/// above the shelf line.
pub struct Rafts {
    pub home: Vec<u32>,
    pub count: u32,
}

impl Rafts {
    pub fn from_relief(map: &HexMap, heights: &[f32], sea: f32) -> Self {
        let n = map.len();
        let mut home = vec![u32::MAX; n];
        let mut count = 0u32;
        let shelf = sea - SHELF_DEPTH;
        for s in 0..n as TileId {
            if home[s as usize] != u32::MAX || heights[s as usize] <= shelf {
                continue;
            }
            let mut stack = vec![s];
            home[s as usize] = count;
            while let Some(t) = stack.pop() {
                for nb in map.neighbours(t) {
                    let j = *nb as usize;
                    if home[j] == u32::MAX && heights[j] > shelf {
                        home[j] = count;
                        stack.push(*nb);
                    }
                }
            }
            count += 1;
        }
        Self { home, count }
    }

    /// A raft tile with an ocean neighbour.
    pub fn is_edge(&self, map: &HexMap, t: TileId) -> bool {
        self.home[t as usize] != u32::MAX
            && map
                .neighbours(t)
                .iter()
                .any(|nb| self.home[*nb as usize] == u32::MAX)
    }
}

/// **The rafts' turns from the push**: each raft's mean of `r × push` over its
/// tiles — a rigid turn about the pole the push implies, derived, never
/// rolled. `push` is per tile, in radians per tick.
pub fn raft_turns(map: &HexMap, rafts: &Rafts, push: &[Vec3]) -> Vec<Vec3> {
    let n = rafts.count as usize;
    let mut sum = vec![Vec3::ZERO; n];
    let mut count = vec![0u32; n];
    for t in 0..map.len() as TileId {
        let k = rafts.home[t as usize];
        if k == u32::MAX {
            continue;
        }
        sum[k as usize] += map.direction(t).cross(push[t as usize]);
        count[k as usize] += 1;
    }
    sum.iter()
        .zip(&count)
        .map(|(s, c)| if *c > 0 { *s / *c as f32 } else { Vec3::ZERO })
        .collect()
}

/// **How hard the floor closes on each raft edge**: at a raft tile with ocean
/// neighbours, the raft's velocity less the floor's beside it, along the
/// outward normal — positive where the floor goes under the land (the
/// subduction margin, where the mountains stand), zero inside and where the
/// margin is passive.
pub fn raft_closing(map: &HexMap, rafts: &Rafts, push: &[Vec3], omega: &[Vec3]) -> Vec<f32> {
    (0..map.len() as TileId)
        .map(|t| {
            let k = rafts.home[t as usize];
            if k == u32::MAX {
                return 0.0;
            }
            let p = map.direction(t);
            let mut out = Vec3::ZERO;
            let mut floor = Vec3::ZERO;
            let mut ocean = 0u32;
            for nb in map.neighbours(t) {
                if rafts.home[*nb as usize] == u32::MAX {
                    out += map.direction(*nb) - p;
                    floor += push[*nb as usize];
                    ocean += 1;
                }
            }
            if ocean == 0 {
                return 0.0;
            }
            let out = (out - p * p.dot(out)).normalize_or_zero();
            let v_raft = omega[k as usize].cross(p);
            let v_floor = floor / ocean as f32;
            (v_raft - v_floor).dot(out).max(0.0)
        })
        .collect()
}

/// What a tick did — the readouts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EpochStats {
    pub ticks: u64,
    /// Live rafts after this tick.
    pub rafts: u32,
    /// Floor parcels consumed this tick; tiles that became young floor.
    pub consumed: u32,
    pub opening: u32,
    /// Rafts welded; rafts split.
    pub welds: u32,
    pub rifts: u32,
    /// The tick's largest single-parcel thickening that stood up, and the
    /// largest crust that went under, fractions of the radius.
    pub uplifted: f32,
    pub subducted: f32,
    /// Crust that went under this tick, summed; what stood up; what rooted.
    pub consumed_crust: f32,
    pub thickened: f32,
    pub rooted: f32,
    /// The tick's EVENTS: vents that erupted (a drop of waves each) and
    /// strikes that fell.
    pub eruptions: u32,
    pub impacts: u32,
}

/// **The epochs' state**: the parcels, the rafts they ride, the breathing
/// heat and its push, and the relief as it stands after `ticks` epochs.
pub struct Epochs {
    // ── the parcels ──
    pos: Vec<Vec3>,
    material: Vec<f32>,
    root: Vec<f32>,
    /// What compression has stood up on the parcel — capped by the ceiling.
    lift: Vec<f32>,
    /// **The hardness ledger**: what the parcel's crust resists with, `0..1`
    /// — born from the layers that made it (the composition's channel), young
    /// floor born basalt-hard, and crust thickened toward the ceiling hardened
    /// toward metamorphic root. Carried with the parcel; what erosion reads.
    parcel_hardness: Vec<f32>,
    /// The raft a parcel rides, or `u32::MAX` for floor.
    raft: Vec<u32>,
    alive: Vec<bool>,
    /// The epoch a floor parcel was born at a ridge, or `u64::MAX` for the
    /// composed crust (old, fully cooled, never subsiding).
    born: Vec<u64>,
    // ── the rafts (union-find: a weld points a raft at the one it joined) ──
    parent: Vec<u32>,
    omega: Vec<Vec3>,
    /// Live parcels per raft id (its own, not its union's).
    size: Vec<u32>,
    /// This world's abyss: the composed deep floor's depth below the sea.
    abyss: f32,
    // ── the heat and its push, radians per tick, per tile ──
    seams: SeamField,
    push: Vec<Vec3>,
    /// The emitters' dials when the heat's emitters are on — every repush
    /// the hot vents may erupt a DROP of waves onto the crust under them.
    emitters: Option<VolcanoShape>,
    /// Events so far: eruptions and impacts.
    eruptions: u64,
    impacts: u64,
    // ── the present ──
    /// Per tile: the parcel shown.
    source: Vec<u32>,
    /// Per tile: the ocean layers' texture young floor born there wears.
    texture: Vec<f32>,
    heights: Vec<f32>,
    /// Per tile: the hardness of the parcel shown.
    hard: Vec<f32>,
    sea: f32,
    tile_angle: f32,
    stats: EpochStats,
}

impl Epochs {
    /// Stand the epochs up over the `composed` world (its heights, sea and
    /// hardness): rafts from the land masses, a parcel on every tile, the
    /// push from the recipe's heat.
    pub fn new(map: &HexMap, recipe: &Recipe, composed: &Composed, r: &EpochRecipe) -> Self {
        let n = map.len();
        let sea = composed.sea;
        let hardness = composed.hardness.clone();
        let composed = &composed.heights[..];
        let rafts = Rafts::from_relief(map, composed, sea);
        let mut size = vec![0u32; rafts.count as usize];
        for k in &rafts.home {
            if *k != u32::MAX {
                size[*k as usize] += 1;
            }
        }
        let abyss = {
            let mut sorted: Vec<f32> = composed.iter().copied().filter(|h| h.is_finite()).collect();
            sorted.sort_by(f32::total_cmp);
            let q = sorted
                .get(((sorted.len().max(1) - 1) as f32 * ABYSS_QUANTILE).round() as usize)
                .copied()
                .unwrap_or(sea - r.ridge);
            (sea - q).max(r.ridge)
        };
        let tile_angle = map.tile_angle();
        let mut e = Self {
            pos: (0..n as TileId).map(|t| map.direction(t)).collect(),
            material: composed.to_vec(),
            root: vec![0.0; n],
            lift: vec![0.0; n],
            hard: hardness.clone(),
            parcel_hardness: hardness,
            raft: rafts.home.clone(),
            alive: vec![true; n],
            born: vec![u64::MAX; n],
            parent: (0..rafts.count).collect(),
            omega: vec![Vec3::ZERO; rafts.count as usize],
            size,
            abyss,
            seams: SeamField::new(map, recipe.heat.cells, recipe.heat.spots, recipe.heat.seed),
            push: Vec::new(),
            emitters: (recipe.heat.on && recipe.heat.emitters_on).then_some(recipe.heat.emitters),
            eruptions: 0,
            impacts: 0,
            source: (0..n as u32).collect(),
            texture: floor_texture(map, recipe),
            heights: composed.to_vec(),
            sea,
            tile_angle,
            stats: EpochStats {
                rafts: rafts.count,
                ..EpochStats::default()
            },
        };
        e.repush(map, r);
        e
    }

    /// The raft a parcel rides now, through the welds.
    fn raft_of(&self, parcel: usize) -> u32 {
        let mut k = self.raft[parcel];
        while k != u32::MAX && self.parent[k as usize] != k {
            k = self.parent[k as usize];
        }
        k
    }

    fn root_raft(&self, mut k: u32) -> u32 {
        while self.parent[k as usize] != k {
            k = self.parent[k as usize];
        }
        k
    }

    /// Re-derive the push from the heat as it stands at this epoch, and every
    /// raft's turn from it; then let a raft the push tears at RIFT.
    fn repush(&mut self, map: &HexMap, r: &EpochRecipe) {
        self.seams
            .at_tick(map, self.stats.ticks * HEAT_TICKS_PER_EPOCH);
        let raw = push_field(map, &self.seams, 1.0, 1.0);
        let top = raw.iter().map(|v| v.length()).fold(0.0f32, f32::max);
        let scale = if top > 1e-9 {
            r.rate * self.tile_angle / top
        } else {
            0.0
        };
        self.push = raw.iter().map(|v| *v * scale).collect();
        self.derive_turns(map);
        self.rift(map, r);
    }

    /// Every live raft's turn: the mean of `pos × push` over its parcels.
    fn derive_turns(&mut self, map: &HexMap) {
        let finder = Nearest::new(map);
        let n = self.parent.len();
        let mut sum = vec![Vec3::ZERO; n];
        let mut count = vec![0u32; n];
        for i in 0..self.pos.len() {
            if !self.alive[i] {
                continue;
            }
            let k = self.raft_of(i);
            if k == u32::MAX {
                continue;
            }
            let t = finder.tile(self.pos[i]);
            sum[k as usize] += self.pos[i].cross(self.push[t]);
            count[k as usize] += 1;
        }
        for k in 0..n {
            self.omega[k] = if count[k] > 0 {
                sum[k] / count[k] as f32
            } else {
                Vec3::ZERO
            };
        }
        // A welded raft's members turn with the union.
        for k in 0..n {
            let root = self.root_raft(k as u32) as usize;
            self.omega[k] = self.omega[root];
        }
    }

    /// RIFTING: a live raft whose residual push (the push less its rigid turn)
    /// exceeds `rift` of the rate, over enough parcels, splits along the line
    /// between the two pulls: the parcels pulled one way along the residuals'
    /// principal direction become a new raft.
    fn rift(&mut self, map: &HexMap, r: &EpochRecipe) {
        let finder = Nearest::new(map);
        let step = r.rate * self.tile_angle;
        if step <= 0.0 {
            return;
        }
        let live: Vec<u32> = (0..self.parent.len() as u32)
            .filter(|k| self.parent[*k as usize] == *k)
            .collect();
        for k in live {
            let members: Vec<usize> = (0..self.pos.len())
                .filter(|i| self.alive[*i] && self.raft_of(*i) == k)
                .collect();
            if members.len() < MIN_RIFT_PARCELS {
                continue;
            }
            let w = self.omega[k as usize];
            let residual: Vec<Vec3> = members
                .iter()
                .map(|i| {
                    let p = self.pos[*i];
                    let v = self.push[finder.tile(p)] - w.cross(p);
                    v - p * p.dot(v)
                })
                .collect();
            let tension = residual.iter().map(|v| v.length()).sum::<f32>() / members.len() as f32;
            if tension <= r.rift * step {
                continue;
            }
            // The principal direction of the pulls (power iteration).
            let mut d = residual.iter().fold(Vec3::ZERO, |a, v| a + v.abs());
            if d.length_squared() < 1e-12 {
                d = Vec3::X;
            }
            for _ in 0..8 {
                let mut next = Vec3::ZERO;
                for v in &residual {
                    next += *v * v.dot(d);
                }
                if next.length_squared() < 1e-18 {
                    break;
                }
                d = next.normalize();
            }
            let new_id = self.parent.len() as u32;
            self.parent.push(new_id);
            self.omega.push(Vec3::ZERO);
            self.size.push(0);
            let mut moved = 0usize;
            for (i, v) in members.iter().zip(&residual) {
                if v.dot(d) > 0.0 {
                    self.raft[*i] = new_id;
                    moved += 1;
                }
            }
            if moved == 0 || moved == members.len() {
                // Nothing to split after all: fold the id back.
                for i in &members {
                    if self.raft[*i] == new_id {
                        self.raft[*i] = k;
                    }
                }
                self.parent.pop();
                self.omega.pop();
                self.size.pop();
                continue;
            }
            // The parcels moved carry their count: sizes are per id, so a
            // split raft's id keeps the parcels that stayed.
            let mut recount = vec![0u32; self.size.len()];
            for i in 0..self.pos.len() {
                if self.alive[i] && self.raft[i] != u32::MAX {
                    recount[self.raft[i] as usize] += 1;
                }
            }
            self.size = recount;
            self.stats.rifts += 1;
        }
        self.derive_turns(map);
        self.stats.rafts = (0..self.parent.len() as u32)
            .filter(|k| self.parent[*k as usize] == *k)
            .count() as u32;
    }

    /// **THE EVENTS of a repush** — the heat has breathed, and what it does
    /// to the crust it does now, onto the parcels standing where it falls
    /// (the hex-stack ledger: nothing is placed on a tile, material lands on
    /// the crust that is there and travels with it from then on):
    /// - **Eruption drops** (E40A9B58: *"drops of waves emanating out over
    ///   ticks"*): the crust's vents are re-derived on the heat as it stands
    ///   now, and each erupts with a chance of [`DROP_CHANCE`] × its heat —
    ///   one DROP: a rolled wave train ([`Emitter::rolled`], this vent, this
    ///   epoch) reaching [`DROP_REACH`] of the dial's radius and standing
    ///   [`DROP_SHARE`] of its height, basalt-hard where it lands.
    /// - **Impacts** (7F7A53DA: *"a crater, as it were, that covered a
    ///   quarter of the planet"*): with `impacts` strikes a thousand epochs,
    ///   a strike falls somewhere, its radius a power law between two tiles
    ///   and [`IMPACT_LARGEST`], its crater wave ([`crater`]) — bowl, rim,
    ///   ripples — scaled to a depth of [`IMPACT_DEPTH`] × √(radius share),
    ///   the bowl and rim BRECCIATED ([`Rock::ImpactBreccia`]): shattered rock the
    ///   era wears down, so a crater is an event with a lifetime, not a
    ///   monument.
    fn events(&mut self, map: &HexMap, r: &EpochRecipe, stats: &mut EpochStats) {
        let seed = self.seams.seed();
        let tick = self.stats.ticks;
        if let Some(shape) = self.emitters {
            let n = map.len();
            let tile_w = 4.0 / (n as f32).sqrt();
            let radius = shape.radius_tiles.max(0.5) * tile_w * DROP_REACH;
            let trains = shape.rings.round().max(1.0) as usize;
            let crust = CrustField::derive(map, &self.seams);
            for &v in crust.vents() {
                let heat = self.seams.heat(v).clamp(0.0, 1.0);
                let mut rng = fastrand::Rng::with_seed(
                    seed ^ (u64::from(v) + 1).wrapping_mul(0x4452_4F50_5F57_4156)
                        ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15),
                );
                if rng.f32() >= DROP_CHANCE * heat {
                    continue;
                }
                let e = Emitter::rolled(map.direction(v), heat, radius, trains, &mut rng);
                let amp = shape.amp * DROP_SHARE;
                let cull = (e.reach * VOLCANO_REACH).cos();
                self.land(
                    map,
                    e.dir,
                    cull,
                    |d| e.figure(shape.arms, d) * amp,
                    Rock::Basalt.resistance(),
                );
                self.eruptions += 1;
                stats.eruptions += 1;
            }
        }
        if r.impacts > 0.0 {
            let mut rng = fastrand::Rng::with_seed(
                seed ^ 0x494D_5041_4354_5321 ^ tick.wrapping_mul(0x2545_F491_4F6C_DD1D),
            );
            let chance = r.impacts * REPUSH_EVERY as f32 / 1000.0;
            if rng.f32() < chance {
                let z = rng.f32() * 2.0 - 1.0;
                let a = rng.f32() * std::f32::consts::TAU;
                let rr = (1.0 - z * z).max(0.0).sqrt();
                let centre = Vec3::new(rr * a.cos(), z, rr * a.sin());
                let smallest = IMPACT_SMALLEST_TILES * self.tile_angle;
                let u = rng.f32();
                // Most strikes small, a few vast: the exponent squared.
                let radius = smallest * (IMPACT_LARGEST / smallest).max(1.0).powf(u * u);
                let depth = IMPACT_DEPTH * (radius / IMPACT_LARGEST).sqrt();
                let cull = (radius * CRATER_REACH).cos();
                self.land(
                    map,
                    centre,
                    cull,
                    |d| crater(centre.dot(d).clamp(-1.0, 1.0).acos(), radius) * depth,
                    Rock::ImpactBreccia.resistance(),
                );
                self.impacts += 1;
                stats.impacts += 1;
            }
        }
    }

    /// **Land an event's waves on the crust**: for every tile within the cull
    /// of `centre`, the wave at the tile's direction is added to the parcel
    /// SHOWN there (it travels with that parcel from now on, the hex-stack
    /// ledger), and the parcel hardens toward `hardness` by how much landed
    /// against a slab's worth.
    fn land(
        &mut self,
        map: &HexMap,
        centre: Vec3,
        cull_cos: f32,
        wave: impl Fn(Vec3) -> f32,
        hardness: f32,
    ) {
        for t in 0..map.len() {
            let d = map.direction(t as TileId);
            if d.dot(centre) < cull_cos {
                continue;
            }
            let delta = wave(d);
            if delta.abs() < 1e-9 {
                continue;
            }
            let i = self.source[t] as usize;
            self.material[i] += delta;
            self.heights[t] += delta;
            let w = (delta.abs() / OCEAN_SLAB).min(1.0);
            self.parcel_hardness[i] += (hardness - self.parcel_hardness[i]) * w;
            self.hard[t] = self.parcel_hardness[i];
        }
    }

    /// **The era's write-back** (the forever loop): the ledger era has cut
    /// and laid `relief` over the tiles; the difference against the relief
    /// the crust shows is written into the parcel standing at each tile —
    /// eroded rock is gone from the crust that carried it, sediment is part
    /// of it — and the parcel takes the surface's hardness. From here the
    /// epochs carry the eroded world, and the era adopts the epoched one.
    pub fn absorb(&mut self, relief: &[f32], hardness: &[f32]) {
        for t in 0..self.heights.len() {
            let i = self.source[t] as usize;
            let delta = relief[t] - self.heights[t];
            self.material[i] += delta;
            self.heights[t] = relief[t];
            self.parcel_hardness[i] = hardness[t];
            self.hard[t] = hardness[t];
        }
    }

    /// Eruptions and impacts so far.
    pub fn events_so_far(&self) -> (u64, u64) {
        (self.eruptions, self.impacts)
    }

    /// Weld two rafts: the smaller id's union points at the larger's root;
    /// both turn as one from here on.
    fn weld(&mut self, a: u32, b: u32) -> bool {
        let (ra, rb) = (self.root_raft(a), self.root_raft(b));
        if ra == rb {
            return false;
        }
        let (keep, join) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[join as usize] = keep;
        let mean = (self.omega[keep as usize] + self.omega[join as usize]) * 0.5;
        self.omega[keep as usize] = mean;
        self.omega[join as usize] = mean;
        true
    }

    /// Young floor's depth below the sea at `age` epochs: the ridge, subsiding
    /// with the square root of its age toward this world's abyss, all the way
    /// there by [`FLOOR_COOLING_TICKS`].
    pub fn floor_depth(&self, age: u32, r: &EpochRecipe) -> f32 {
        let cooled = (age as f32 / FLOOR_COOLING_TICKS).clamp(0.0, 1.0).sqrt();
        r.ridge + (self.abyss - r.ridge).max(0.0) * cooled
    }

    /// The depth below the sea young floor cools to — the composed ocean's
    /// deep floor.
    pub fn abyss(&self) -> f32 {
        self.abyss
    }

    /// Live parcels riding raft `k` and every raft welded into it.
    fn union_size(&self, k: u32) -> u32 {
        let root = self.root_raft(k);
        (0..self.parent.len() as u32)
            .filter(|j| self.root_raft(*j) == root)
            .map(|j| self.size[j as usize])
            .sum()
    }

    /// **One epoch.**
    pub fn tick(&mut self, map: &HexMap, r: &EpochRecipe) -> EpochStats {
        let n = map.len();
        let mut stats = EpochStats {
            ticks: self.stats.ticks + 1,
            rifts: 0,
            ..EpochStats::default()
        };
        self.stats.rifts = 0;
        self.stats.ticks = stats.ticks;
        if stats.ticks.is_multiple_of(REPUSH_EVERY) {
            self.repush(map, r);
            stats.rifts = self.stats.rifts;
            self.events(map, r, &mut stats);
        }
        let finder = Nearest::new(map);
        // 1. The parcels move: rafts turn rigidly, floor drifts with the push.
        for i in 0..self.pos.len() {
            if !self.alive[i] {
                continue;
            }
            let p = self.pos[i];
            let k = self.raft_of(i);
            self.pos[i] = if k == u32::MAX {
                let v = self.push[finder.tile(p)];
                (p + (v - p * p.dot(v))).normalize_or_zero()
            } else {
                let w = self.omega[k as usize];
                let ang = w.length();
                if ang > 1e-12 {
                    Quat::from_axis_angle(w / ang, ang) * p
                } else {
                    p
                }
            };
        }
        // 1b. Young floor cools: a parcel born at a ridge subsides toward the
        // abyss with the square root of its age — thermal subsidence, the one
        // way a parcel's height falls, and only the floor the epochs made.
        for i in 0..self.pos.len() {
            if !self.alive[i] || self.born[i] == u64::MAX {
                continue;
            }
            let age = stats.ticks.saturating_sub(self.born[i]) as u32;
            if age >= 1 && (age as f32) <= FLOOR_COOLING_TICKS + 1.0 {
                self.material[i] -= self.floor_depth(age, r) - self.floor_depth(age - 1, r);
            }
        }
        // 2. Bin the parcels by the tile they stand on.
        let mut at: Vec<u32> = vec![u32::MAX; self.pos.len()];
        let mut bins: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (i, (p, slot)) in self.pos.iter().zip(at.iter_mut()).enumerate() {
            if !self.alive[i] {
                continue;
            }
            let t = finder.tile(*p);
            *slot = t as u32;
            bins[t].push(i as u32);
        }
        // 3. Compression: a neighbourhood two rings wide holding more parcels
        // than its tiles (by CROWD) is crust converging — the lowest FLOOR
        // parcel there goes under the highest parcel; two rafts crowding have
        // collided and weld. Two rings, not one: a drifting lattice sampled by
        // nearest tile puts 0, 1 or 2 parcels on a tile by chance, and a
        // one-ring count mistook that noise for compression and ate the floor.
        let share = r.uplift.max(0.0);
        let mut hood: Vec<TileId> = Vec::with_capacity(20);
        for t in 0..n as TileId {
            if bins[t as usize].len() < 2 {
                continue;
            }
            loop {
                hood.clear();
                hood.push(t);
                for nb in map.neighbours(t) {
                    if !hood.contains(nb) {
                        hood.push(*nb);
                    }
                    for nb2 in map.neighbours(*nb) {
                        if !hood.contains(nb2) {
                            hood.push(*nb2);
                        }
                    }
                }
                let ring: Vec<u32> = hood
                    .iter()
                    .flat_map(|h| bins[*h as usize].iter().copied())
                    .collect();
                let capacity = hood.len() + CROWD;
                if ring.len() <= capacity {
                    break;
                }
                let highest = *ring
                    .iter()
                    .max_by(|a, b| {
                        self.material[**a as usize].total_cmp(&self.material[**b as usize])
                    })
                    .expect("a crowded neighbourhood has parcels");
                let lowest_floor = ring
                    .iter()
                    .copied()
                    .filter(|i| self.raft_of(*i as usize) == u32::MAX)
                    .min_by(|a, b| {
                        self.material[*a as usize].total_cmp(&self.material[*b as usize])
                    });
                match lowest_floor {
                    Some(under) => {
                        let crust =
                            OCEAN_SLAB.max(self.material[under as usize] - (self.sea - r.ridge));
                        // The slab stands up only on a LAND margin (the Andes), and
                        // only on a raft large enough to be land, not a fragment;
                        // floor going under floor roots the whole slab — island
                        // arcs are the emitters' to raise, not a stack of specks.
                        let over = self.raft_of(highest as usize);
                        let onto_land =
                            over != u32::MAX && self.union_size(over) as usize >= MIN_RAFT_PARCELS;
                        let share_up = if onto_land { crust * share } else { 0.0 };
                        // The share stands EVENLY across the overriding raft's
                        // parcels in the neighbourhood that still have room under
                        // the ceiling — a belt, not a column; a uniform lift keeps
                        // the waves riding on it exactly, and the ceiling bounds
                        // each column's LIFT, never its relief (a first cut gave
                        // each parcel its room under one absolute height and planed
                        // the belt's top into a flat plateau). What no column can
                        // take roots.
                        let mut belt: Vec<u32> = Vec::new();
                        if share_up > 0.0 {
                            belt.extend(ring.iter().copied().filter(|i| {
                                self.raft_of(*i as usize) == over
                                    && self.lift[*i as usize] < r.summit
                            }));
                        }
                        let mut stands = 0.0f32;
                        if !belt.is_empty() {
                            let part = share_up / belt.len() as f32;
                            for i in &belt {
                                let j = *i as usize;
                                let take = part.min(r.summit - self.lift[j]);
                                self.material[j] += take;
                                self.lift[j] += take;
                                // Thickened crust hardens toward root: by the
                                // ceiling it is metamorphic.
                                if r.summit > 0.0 {
                                    self.parcel_hardness[j] +=
                                        (Rock::Quartzite.resistance() - self.parcel_hardness[j]) * (take / r.summit);
                                }
                                stands += take;
                                stats.uplifted = stats.uplifted.max(take);
                            }
                        }
                        self.root[highest as usize] += (crust - stands).max(0.0);
                        self.alive[under as usize] = false;
                        let tile = at[under as usize] as usize;
                        bins[tile].retain(|i| *i != under);
                        stats.consumed += 1;
                        stats.consumed_crust += crust;
                        stats.thickened += stands;
                        stats.rooted += (crust - stands).max(0.0);
                        stats.subducted = stats.subducted.max(crust);
                    }
                    None => {
                        // All rafts: the collision welds them, once.
                        let rafts: Vec<u32> =
                            ring.iter().map(|i| self.raft_of(*i as usize)).collect();
                        let first = rafts[0];
                        for k in rafts.iter().skip(1) {
                            if *k != first && self.weld(first, *k) {
                                stats.welds += 1;
                            }
                        }
                        break;
                    }
                }
            }
        }
        // 4. What each tile shows: its highest parcel; else the nearest parcel
        // within reach (a turn on a hex grid leaves one-tile gaps that are not
        // oceans); else the crust has pulled apart here and young floor is
        // born — a floor parcel at ridge depth wearing the ocean's texture,
        // in the flow from this epoch on.
        let snap_cos = (SNAP * self.tile_angle).cos();
        let floor_snap_cos = (FLOOR_SNAP * self.tile_angle).cos();
        for t in 0..n as TileId {
            let p = map.direction(t);
            let own = bins[t as usize]
                .iter()
                .copied()
                .max_by(|a, b| self.material[*a as usize].total_cmp(&self.material[*b as usize]));
            let shown = own.or_else(|| {
                // Every neighbour occupied: a one-tile hole in rigid crust is the
                // grid's, not an ocean — take the nearest parcel whatever its
                // distance; otherwise a raft parcel only within SNAP (a rigid
                // edge keeps its outline) and a floor parcel within FLOOR_SNAP
                // (the fluid floor spreads into the gaps it leaves).
                let ringed = map
                    .neighbours(t)
                    .iter()
                    .all(|nb| !bins[*nb as usize].is_empty());
                let mut best: Option<(f32, u32)> = None;
                for nb in map.neighbours(t) {
                    for i in &bins[*nb as usize] {
                        let d = self.pos[*i as usize].dot(p);
                        let reach = if self.raft_of(*i as usize) == u32::MAX {
                            floor_snap_cos
                        } else {
                            snap_cos
                        };
                        if (ringed || d >= reach) && best.is_none_or(|b| d > b.0) {
                            best = Some((d, *i));
                        }
                    }
                }
                best.map(|b| b.1)
            });
            let i = match shown {
                Some(i) => i,
                None => {
                    // Born at the ridge, textured, but under the sea: the
                    // floor's bubbles are not islands (those are the emitters').
                    let h = (self.sea - r.ridge + self.texture[t as usize])
                        .min(self.sea - 0.25 * r.ridge);
                    let i = self.pos.len() as u32;
                    self.pos.push(p);
                    self.material.push(h);
                    self.root.push(0.0);
                    self.lift.push(0.0);
                    self.parcel_hardness.push(Rock::Basalt.resistance());
                    self.raft.push(u32::MAX);
                    self.alive.push(true);
                    self.born.push(stats.ticks);
                    bins[t as usize].push(i);
                    stats.opening += 1;
                    i
                }
            };
            self.source[t as usize] = i;
            self.heights[t as usize] = self.material[i as usize];
            self.hard[t as usize] = self.parcel_hardness[i as usize];
        }
        stats.rafts = (0..self.parent.len() as u32)
            .filter(|k| self.parent[*k as usize] == *k)
            .count() as u32;
        self.stats = stats;
        stats
    }

    /// Run `count` epochs; the last tick's readouts.
    pub fn run(&mut self, map: &HexMap, r: &EpochRecipe, count: u32) -> EpochStats {
        let mut last = self.stats;
        for _ in 0..count {
            last = self.tick(map, r);
        }
        last
    }

    /// The crustal root under the parcel standing on `tile`: the overridden
    /// crust that went down instead of up.
    pub fn root(&self, tile: TileId) -> f32 {
        self.root[self.source[tile as usize] as usize]
    }

    /// The raft shown at `tile` (through the welds), or `None` for floor.
    pub fn raft_at(&self, tile: TileId) -> Option<u32> {
        match self.raft_of(self.source[tile as usize] as usize) {
            u32::MAX => None,
            k => Some(k),
        }
    }

    /// The age in epochs of the floor shown at `tile`, or `None` for the
    /// composed crust (old, never born in the epochs).
    pub fn floor_age_at(&self, tile: TileId) -> Option<u64> {
        match self.born[self.source[tile as usize] as usize] {
            u64::MAX => None,
            b => Some(self.stats.ticks.saturating_sub(b)),
        }
    }

    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// Per tile, the hardness of the crust shown — the ledger as it stands.
    pub fn hardness(&self) -> &[f32] {
        &self.hard
    }

    pub fn stats(&self) -> EpochStats {
        self.stats
    }

    pub fn ticks(&self) -> u64 {
        self.stats.ticks
    }

    /// One tile's turn, radians.
    pub fn tile_angle(&self) -> f32 {
        self.tile_angle
    }

    /// Live parcels.
    pub fn parcels(&self) -> usize {
        self.alive.iter().filter(|a| **a).count()
    }

    /// **The floor's census** — live parcels the epochs BORN (recycled floor),
    /// and tiles with two or more parcels standing on them (crust crowding
    /// toward consumption): the readings a sampling fault shows up in before
    /// the picture does.
    pub fn census(&self, map: &HexMap) -> (usize, usize) {
        let born = (0..self.pos.len())
            .filter(|i| self.alive[*i] && self.born[*i] != u64::MAX)
            .count();
        let finder = Nearest::new(map);
        let mut count = vec![0u16; map.len()];
        for (i, p) in self.pos.iter().enumerate() {
            if self.alive[i] {
                count[finder.tile(*p)] += 1;
            }
        }
        let stacked = count.iter().filter(|c| **c >= 2).count();
        (born, stacked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;
    use crate::recipe::compose;
    use crate::resonance::land_share;

    /// **The stone ocean moves and keeps its waves, its land and its ledger.**
    /// Rafts are the land masses; over a hundred epochs the floor flows, floor
    /// goes under where it crowds (what goes under stands up or roots), no
    /// pit opens inside the crust, young floor wears texture and deepens with
    /// age, the carried relief is never planed, land keeps its share, and the
    /// rafts stay whole — a continent is never cut by a rolled line. The
    /// EVENTS (drops, strikes) are their own procedure with their own gate
    /// below; this one runs the crust alone, so the ledger it reads is the
    /// compression's.
    #[test]
    fn the_stone_ocean_moves_and_keeps_its_waves() {
        let map = HexMap::new(MIN_FREQ);
        let mut recipe = Recipe::default_at(MIN_FREQ, 7);
        recipe.heat.emitters_on = false;
        let c = compose(&map, &recipe);
        let r = EpochRecipe {
            impacts: 0.0,
            ..EpochRecipe::default()
        };
        let mut e = Epochs::new(&map, &recipe, &c, &r);
        assert!(e.tile_angle() > 0.0 && e.tile_angle() < 0.1);
        assert_eq!(
            e.hardness(),
            &c.hardness[..],
            "born with the composition's hardness"
        );
        let rafts0 = e.stats().rafts;
        assert!(rafts0 >= 2, "the waves made land masses: {rafts0}");
        let top = e.push.iter().map(|v| v.length()).fold(0.0f32, f32::max);
        assert!(
            (top - r.rate * e.tile_angle()).abs() < 1e-6,
            "the fastest push is the rate"
        );
        let before = e.heights().to_vec();
        let (mut consumed, mut opening) = (0, 0);
        for _ in 0..100 {
            let s = e.tick(&map, &r);
            consumed += s.consumed;
            opening += s.opening;
            assert!(
                (s.consumed_crust - (s.thickened + s.rooted)).abs() < 1e-4,
                "what goes under stands up or roots"
            );
            assert!(
                s.thickened <= s.consumed_crust * r.uplift + 1e-4,
                "no more stands up than the share"
            );
        }
        assert_eq!(e.ticks(), 100);
        assert!(consumed > 0, "floor went under where it crowded");
        assert!(opening > 0, "young floor opened");
        assert_ne!(e.heights(), &before[..], "the relief moved");
        assert!(e.heights().iter().all(|h| h.is_finite()));
        // EVERY TILE SHOWS CRUST: no parcel-less tile, no pit, no fringe.
        assert!(e.source.iter().all(|s| (*s as usize) < e.pos.len()));
        // Young floor is a parcel: born at the ridge under the sea, subsiding
        // with age, never below the abyss and its texture.
        let (born, _) = e.census(&map);
        assert!(born > 0, "young floor was born as parcels");
        for i in 0..e.pos.len() {
            if e.alive[i] && e.born[i] != u64::MAX {
                let age = (e.ticks() - e.born[i]) as u32;
                assert!(e.material[i] < c.sea, "young floor is under the sea");
                assert!(
                    e.material[i]
                        >= c.sea - e.floor_depth(age, &r) - 0.25 * r.ridge - 1e-6
                            + e.texture.iter().copied().fold(0.0f32, f32::min),
                    "young floor lies no deeper than its age cools it"
                );
            }
        }
        assert!(e.floor_depth(60, &r) > e.floor_depth(1, &r));
        assert!(
            (e.floor_depth(1_000_000, &r) - e.abyss()).abs() < 1e-6,
            "old floor lies at this world's abyss"
        );
        assert!(
            e.abyss() > 2.0 * r.ridge,
            "the composed ocean is deeper than the ridge: abyss {}",
            e.abyss()
        );
        // NO TOWERS: compression lifts no column by more than the ceiling —
        // every parcel stands at what it was born with plus at most `summit`.
        for i in 0..map.len() {
            assert!(
                e.material[i] <= c.heights[i] + r.summit + 1e-6,
                "parcel {i} lifted past the ceiling: {} over {}",
                e.material[i] - c.heights[i],
                r.summit
            );
            assert!((e.lift[i] - (e.material[i] - c.heights[i])).abs() < 1e-5);
            // THE HARDNESS LEDGER: a lifted parcel hardened toward root, an
            // unlifted one is exactly as it was born; nothing softens.
            assert!(
                e.parcel_hardness[i] >= c.hardness[i] - 1e-6,
                "parcel {i} softened"
            );
            if e.lift[i] == 0.0 {
                assert_eq!(e.parcel_hardness[i], c.hardness[i]);
            } else {
                assert!(e.parcel_hardness[i] <= Rock::Quartzite.resistance() + 1e-6);
            }
        }
        assert!(e.hardness().iter().all(|h| (0.0..=1.0).contains(h)));
        for i in map.len()..e.pos.len() {
            if e.alive[i] {
                assert_eq!(
                    e.parcel_hardness[i], Rock::Basalt.resistance(),
                    "young floor is basalt"
                );
            }
        }
        // NO FLATTENING: a parcel's relief is exactly what it was born with,
        // plus what stood up on it — the waves travel, they are never planed.
        for i in 0..map.len() {
            assert!(
                e.material[i] >= c.heights[i] - 1e-7,
                "parcel {i} lost height"
            );
        }
        let rough = |h: &[f32], land_only: bool| {
            let mut sum = 0.0f32;
            let mut k = 0usize;
            for t in 0..map.len() as TileId {
                if land_only && h[t as usize] <= c.sea {
                    continue;
                }
                for nb in map.neighbours(t) {
                    sum += (h[t as usize] - h[*nb as usize]).abs();
                    k += 1;
                }
            }
            sum / k.max(1) as f32
        };
        let (r0, r1) = (rough(&before, true), rough(e.heights(), true));
        assert!(
            r1 > r0 * 0.8,
            "the land's waves stand: roughness {r0} → {r1}"
        );
        // NO WATER WORLD: the land keeps its share.
        let land0 = land_share(&map, |t| before[t as usize], c.sea);
        let land = land_share(&map, |t| e.heights()[t as usize], c.sea);
        assert!((land - land0).abs() < 0.08, "land {land0} → {land}");
        // RAFTS STAY WHOLE: every parcel of a land mass still rides a raft, and
        // the rafts shown are a whole number of connected bodies.
        for i in 0..map.len() {
            if e.alive[i] && e.raft[i] != u32::MAX {
                assert_ne!(e.raft_of(i), u32::MAX);
            }
        }
        assert!(e.stats().rafts >= 1);
    }

    /// **Events land on the crust and the era writes back.** Over a hundred
    /// epochs with the heat's emitters on, hot vents erupt DROPS (the counter
    /// and the stats agree, the relief rose where they fell and hardened
    /// toward basalt); with strikes made certain, an impact falls — a bowl
    /// with a rim, the crust under it brecciated soft; with the emitters off and
    /// no strikes, nothing lands. And `absorb` writes a relief back into the
    /// parcels exactly: the heights read it, the hardness follows, and the
    /// epochs carry the written world on.
    #[test]
    fn events_land_on_the_crust_and_the_era_writes_back() {
        let map = HexMap::new(MIN_FREQ);
        let recipe = Recipe::default_at(MIN_FREQ, 7);
        let c = compose(&map, &recipe);
        // Drops.
        let quiet = EpochRecipe {
            impacts: 0.0,
            ..EpochRecipe::default()
        };
        let mut e = Epochs::new(&map, &recipe, &c, &quiet);
        let mut erupted = 0u32;
        for _ in 0..100 {
            erupted += e.tick(&map, &quiet).eruptions;
        }
        assert!(erupted > 0, "hot vents erupted over a hundred epochs");
        assert_eq!(e.events_so_far(), (u64::from(erupted), 0));
        assert!(e.heights().iter().all(|h| h.is_finite()));
        // Nothing lands with the emitters off and no strikes.
        let mut off = recipe.clone();
        off.heat.emitters_on = false;
        let c_off = compose(&map, &off);
        let mut still = Epochs::new(&map, &off, &c_off, &quiet);
        for _ in 0..30 {
            let s = still.tick(&map, &quiet);
            assert_eq!((s.eruptions, s.impacts), (0, 0));
        }
        // A strike made certain: one repush, one crater.
        let certain = EpochRecipe {
            impacts: 1000.0 / REPUSH_EVERY as f32,
            ..EpochRecipe::default()
        };
        let mut hit = Epochs::new(&map, &off, &c_off, &certain);
        let before = hit.heights().to_vec();
        let mut struck = 0u32;
        for _ in 0..REPUSH_EVERY {
            struck += hit.tick(&map, &certain).impacts;
        }
        assert_eq!(struck, 1, "one strike at the repush");
        let lowest = (0..map.len())
            .min_by(|a, b| {
                (hit.heights()[*a] - before[*a]).total_cmp(&(hit.heights()[*b] - before[*b]))
            })
            .expect("tiles");
        assert!(
            hit.heights()[lowest] < before[lowest] - 1e-5,
            "the bowl is a hollow in the crust"
        );
        assert!(
            (0..map.len()).any(|t| hit.heights()[t] > before[t] + 1e-5),
            "the rim stands up"
        );
        // The bowl is BRECCIATED: the strike blends the crust it lowered
        // toward impact breccia's resistance by the share it landed, so struck ground reads
        // softer than the floor it struck — a crater is the soft thing the
        // era wears. (Read over every struck tile, not the one deepest at
        // the end: the floor keeps flowing after the strike, and the tile
        // deepest by then may show a young-floor parcel at 0.7 — which is
        // what the old "shocked hard" assertion was quietly reading.)
        let softened = (0..map.len())
            .filter(|t| {
                hit.heights()[*t] < before[*t] - 1e-5
                    && hit.hardness()[*t] < c_off.hardness[*t] - 0.05
            })
            .count();
        assert!(
            softened > 0,
            "the bowl is brecciated — struck ground reads softer than the floor it struck"
        );
        assert!(
            hit.hardness().iter().all(|h| *h >= Rock::ImpactBreccia.resistance() - 1e-6),
            "nothing reads softer than breccia"
        );
        // The write-back.
        let mut w = Epochs::new(&map, &off, &c_off, &quiet);
        w.tick(&map, &quiet);
        let eroded: Vec<f32> = w.heights().iter().map(|h| h - 0.001).collect();
        let soft = vec![0.3f32; map.len()];
        w.absorb(&eroded, &soft);
        assert_eq!(
            w.heights(),
            &eroded[..],
            "the heights read the written relief"
        );
        assert!(w.hardness().iter().all(|h| (*h - 0.3).abs() < 1e-6));
        let after = w.tick(&map, &quiet);
        assert!(after.ticks == 2 && w.heights().iter().all(|h| h.is_finite()));
    }

    /// **Rifting splits a raft the push tears at**, and welding joins two
    /// that collide: a rift threshold of zero splits every large raft at the
    /// first repush; a weld makes two rafts turn as one.
    #[test]
    fn rifts_split_and_welds_join() {
        let map = HexMap::new(MIN_FREQ);
        let recipe = Recipe::default_at(MIN_FREQ, 7);
        let c = compose(&map, &recipe);
        let eager = EpochRecipe {
            rift: 0.0,
            ..EpochRecipe::default()
        };
        let mut e = Epochs::new(&map, &recipe, &c, &eager);
        let rafts0 = Rafts::from_relief(&map, &c.heights, c.sea).count;
        assert!(
            e.stats().rafts > rafts0,
            "every large raft split at the first push: {} > {rafts0}",
            e.stats().rafts
        );
        let (a, b) = (0u32, 1u32);
        assert!(e.weld(a, b));
        assert_eq!(e.root_raft(a), e.root_raft(b));
        assert_eq!(
            e.omega[a as usize], e.omega[b as usize],
            "welded rafts turn as one"
        );
        assert!(!e.weld(a, b), "a weld is once");
        let never = EpochRecipe {
            rift: 1e9,
            ..EpochRecipe::default()
        };
        let f = Epochs::new(&map, &recipe, &c, &never);
        assert_eq!(f.stats().rafts, rafts0, "no rift at an infinite threshold");
    }
}
