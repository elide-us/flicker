//! **The deep crust layer's first fact: where the heat gets THROUGH.**
//!
//! The deep crust is the thick bedrock lid over the molten layer. It does not
//! pass the mantle's heat everywhere — it channels it: only where the seam
//! heat beneath is concentrated enough does it PUSH through the bedrock, and
//! those breakthroughs are the volcanic vents. Seen from above the crust map
//! is mostly bedrock brown, dotted with lava where the layer below won.
//!
//! This module derives those vents from the molten [`SeamField`] — nothing
//! here places a volcano (rule 935269B7): a tile must stand in the hot band
//! (a seam or a plume core) to be a candidate at all, candidates are ranked by
//! their own heat plus a seeded jitter (the same roll as the seams, so a
//! re-roll moves the vents with them), and a TWO-SCALE greedy pass turns the
//! ranking into volcanic FIELDS: founders stand far apart, and each field
//! densely fills in around its founder at a few tiles' spacing — lumpy chains
//! with gaps between them, never an even scattering (Aaron 2026-08-25: an even
//! scattering reads as dots, not volcanism).

use crate::map::{HexMap, TileId};
use crate::seams::SeamField;

/// The least molten heat under a tile that can push through the bedrock. The
/// on-seam heat floor is the seam weight (0.62), so this keeps candidates in
/// the seam band proper — the crust never vents over a cool bubble interior
/// (a hot spot's core clears it too, and vents wherever it burns).
const VENT_HEAT: f32 = 0.66;
/// The UPWELL floor: the molten heat above which the crust actually passes
/// material — the generation ZONE the crust tab marks and the evolution's
/// per-tick upwelling samples from. Wider than the vent floor: every vent
/// stands inside a zone, not every zone tile vents.
pub(crate) const UPWELL_HEAT: f32 = 0.5;
/// How much the seeded jitter can outweigh raw heat in the ranking — enough to
/// shuffle same-heat seam tiles (semi-random), not enough to un-favour the
/// junctions and plume cores (the volcanic points still win their
/// neighbourhoods).
const VENT_JITTER: f32 = 0.2;

// ── the two-scale clustering (Aaron 2026-08-25: an even scattering reads as
// dots, not volcanism — "some weight that lumps them together"): FIELDS of
// vents stand far apart, and each field is a dense GLOB. ──
/// How far apart volcanic FIELDS stand, as a fraction of a convection cell's
/// characteristic angular radius — the between-glob spacing along a seam.
const CLUSTER_SEPARATION: f32 = 0.7;
/// How far a field's members reach around its founding vent, as a fraction of
/// the cell radius — the size of a glob.
const CLUSTER_RADIUS: f32 = 0.28;
/// The BASE spacing within a glob, in tile widths — small, so a field reads
/// as a dense chain of vents rather than one dot.
const MEMBER_SEPARATION_TILES: f32 = 3.2;
/// Each vent's claimed territory is the base spacing scaled by its own seeded
/// draw over this range (Aaron 2026-08-25: a FIXED spacing greedy-fills a
/// plume core into a perfectly even lattice, which reads odd — varying every
/// vent's claim breaks the lattice into organic clumps, a touch looser on
/// average).
const MEMBER_LOOSE_MIN: f32 = 0.55;
const MEMBER_LOOSE_SPAN: f32 = 1.2;
/// A candidate THIS hot founds a field regardless of the founding spacing —
/// a mantle PLUME CORE is its own volcano, even in the dead zone between
/// existing fields (without this, a fresh plume between two chains could be
/// forbidden from venting at all: heat with no breakthrough, a silent hole
/// in the map). Above every seam and junction shoulder: only the plume-core
/// grade overrides the spacing law.
const FOUND_HOT: f32 = 0.86;

/// **The deep crust's vent set.** Which tiles the molten heat breaks through,
/// derived from one [`SeamField`] over one [`HexMap`] tiling.
pub struct CrustField {
    /// The vents, in acceptance order (hottest-plus-jitter first).
    vents: Vec<TileId>,
    /// Per-tile membership, indexed by `TileId` like every per-tile layer.
    vent_at: Vec<bool>,
}

impl CrustField {
    /// Derive the vents the crust concedes to `seams` — re-run whenever the
    /// seam field re-rolls, re-counts or re-tiles, because the vents ARE its
    /// consequence.
    pub fn derive(map: &HexMap, seams: &SeamField) -> Self {
        // The seeded jitter — a DISTINCT stream off the same roll, so the
        // vents move when the seams do and stand still when they stand still.
        let mut rng = fastrand::Rng::with_seed(seams.seed().wrapping_add(0x9E37_79B9_7F4A_7C15));
        let jitter: Vec<f32> = (0..map.len()).map(|_| rng.f32()).collect();

        let mut candidates: Vec<TileId> = map
            .tiles()
            .filter(|t| seams.heat(*t) >= VENT_HEAT)
            .collect();
        candidates.sort_by(|a, b| {
            let score = |t: TileId| seams.heat(t) + VENT_JITTER * jitter[t as usize];
            score(*b).total_cmp(&score(*a))
        });

        // Two-scale greedy clustering: the ranking founds volcanic FIELDS far
        // apart (a hot tile clear of every existing field starts a new one),
        // and every candidate near a field joins it as long as it keeps a few
        // tiles of daylight from the vents already burning — so the dots come
        // out as dense, lumpy chains with gaps between fields, not an even
        // scattering.
        let cell_radius = (4.0 * std::f32::consts::PI / seams.cells() as f32).sqrt() * 0.5;
        // A tile's own angular radius from the tiling itself: N caps over 4π.
        let tile_radius = 2.0 / (map.len() as f32).sqrt();
        let founding = (CLUSTER_SEPARATION * cell_radius).cos();
        // A plume peak must stand at least HALF the founding spacing clear of
        // every field before the override applies — "lone" in field terms.
        let lone_founding = (0.5 * CLUSTER_SEPARATION * cell_radius).cos();
        let reach = (CLUSTER_RADIUS * cell_radius).cos();
        let mut fields: Vec<TileId> = Vec::new();
        // Each accepted vent with the cos of ITS OWN claimed radius — the
        // seeded per-vent variation that keeps a glob from packing into an
        // even lattice.
        let mut vents: Vec<(TileId, f32)> = Vec::new();
        let mut vent_at = vec![false; map.len()];
        for t in candidates {
            let d = map.direction(t);
            let in_reach = fields.iter().any(|f| d.dot(map.direction(*f)) > reach);
            // The plume-core override: a LONE local maximum of plume-grade
            // heat founds even inside the dead zone — a fresh plume always
            // vents — but a peak standing near an existing field belongs to
            // that field's volcanic region and earns no exemption (hot
            // junction shoulders must not blanket the seams with
            // just-beyond-reach micro-fields).
            let lone = fields
                .iter()
                .all(|f| d.dot(map.direction(*f)) < lone_founding);
            let plume_core = seams.heat(t) >= FOUND_HOT
                && lone
                && map
                    .neighbours(t)
                    .iter()
                    .all(|nb| seams.heat(*nb) <= seams.heat(t));
            let founds = !in_reach
                && (plume_core
                    || fields.iter().all(|f| d.dot(map.direction(*f)) < founding));
            if (in_reach || founds)
                && vents.iter().all(|(v, claim)| d.dot(map.direction(*v)) < *claim)
            {
                if founds {
                    fields.push(t);
                }
                let loose = MEMBER_LOOSE_MIN + MEMBER_LOOSE_SPAN * jitter[t as usize];
                let claim = (MEMBER_SEPARATION_TILES * loose * 2.0 * tile_radius).cos();
                vent_at[t as usize] = true;
                vents.push((t, claim));
            }
        }
        Self {
            vents: vents.into_iter().map(|(t, _)| t).collect(),
            vent_at,
        }
    }

    /// Whether the crust vents at `tile` — a lava dot on the map, a lava
    /// column in the stack. Out-of-range reads are bedrock: a hole is not a
    /// volcano.
    pub fn is_vent(&self, tile: TileId) -> bool {
        self.vent_at.get(tile as usize).copied().unwrap_or(false)
    }

    /// The vents themselves, in acceptance order.
    pub fn vents(&self) -> &[TileId] {
        &self.vents
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;
    use crate::seams::DEFAULT_CELLS;

    fn field() -> (HexMap, SeamField) {
        let map = HexMap::new(MIN_FREQ);
        let seams = SeamField::new(&map, DEFAULT_CELLS, crate::seams::DEFAULT_SPOTS, 42);
        (map, seams)
    }

    /// **Vents stand IN the heat, in dense LUMPS that stand apart.** Every
    /// vent's own heat clears the breakthrough floor (never a cool interior);
    /// within a glob vents keep only a few tiles of daylight (the density),
    /// the population is a real chain-map's worth, and the globs themselves
    /// are lumpy: many close pairs (a field) AND far pairs (the gaps between
    /// fields) — the two-scale shape, not an even scattering.
    #[test]
    fn vents_form_dense_lumps_that_stand_apart() {
        let (map, seams) = field();
        let crust = CrustField::derive(&map, &seams);
        // The floor allows for the seams' DIVES: a stretch of seam under
        // cooler material sheds its vent candidates, which is the point.
        assert!(crust.vents().len() >= 14, "got {}", crust.vents().len());
        // …and the map is MOSTLY bedrock: chains, not a wash.
        assert!(crust.vents().len() < map.len() / 20);
        assert!(!crust.is_vent(u32::MAX), "a hole is not a volcano");

        let tile_radius = 2.0 / (map.len() as f32).sqrt();
        // Every vent claims at LEAST the loosest floor of the jittered range —
        // the hard lower bound the per-vent variation can never dip under.
        let floor = MEMBER_SEPARATION_TILES * MEMBER_LOOSE_MIN * 2.0 * tile_radius;
        let cell_radius = (4.0 * std::f32::consts::PI / seams.cells() as f32).sqrt() * 0.5;
        let mut near = 0usize;
        let mut far = 0usize;
        let mut gaps: Vec<f32> = Vec::new();
        for (i, a) in crust.vents().iter().enumerate() {
            assert!(
                seams.heat(*a) >= VENT_HEAT,
                "vent {a} sits on heat {}",
                seams.heat(*a)
            );
            assert!(crust.is_vent(*a));
            let mut nearest = f32::MAX;
            for b in &crust.vents()[i + 1..] {
                let d = map
                    .direction(*a)
                    .dot(map.direction(*b))
                    .clamp(-1.0, 1.0)
                    .acos();
                assert!(
                    d >= floor * 0.999,
                    "vents {a} and {b} stand {d} apart, under the claim floor"
                );
                nearest = nearest.min(d);
                // The glob signature: pairs inside one field's reach…
                if d < CLUSTER_RADIUS * cell_radius {
                    near += 1;
                }
                // …and pairs a whole field-gap apart.
                if d > CLUSTER_SEPARATION * cell_radius {
                    far += 1;
                }
            }
            // The in-glob nearest-neighbour gaps, for the lattice check below.
            if nearest < CLUSTER_RADIUS * cell_radius {
                gaps.push(nearest);
            }
        }
        // Dived seam stretches thin the fields — and the narrowed spacing
        // (Aaron: fewer, more specific upwelling points) thins them further —
        // so the lump signature is a MODEST ratio of close pairs, not one
        // per vent.
        assert!(
            near * 3 >= crust.vents().len(),
            "the vents lump into fields: {near} close pairs over {}",
            crust.vents().len()
        );
        assert!(far > 0, "…and the fields stand apart");
        // **The glob is NOT a lattice** (Aaron: evenly clustered reads odd) —
        // the per-vent claim jitter must show up as real variation in the
        // in-glob nearest-neighbour spacing, not one repeated grid pitch.
        let (lo, hi) = gaps
            .iter()
            .fold((f32::MAX, 0.0f32), |(l, h), g| (l.min(*g), h.max(*g)));
        // (The narrowed spacing thins the close-pair sample, so the spread
        // bound is modest — the property is variation, not its magnitude.)
        assert!(
            hi / lo >= 1.25,
            "in-glob spacing varies (loosest {hi} vs tightest {lo}), not a lattice"
        );
    }

    /// **New plumes always reach the vent map** — the lone-peak override's
    /// gate (the spots dial once flaked on rolls where a fresh plume landed
    /// in the clustering dead zone and was forbidden from venting): the same
    /// roll with plumes must derive a DIFFERENT vent set than without, at
    /// several seeds — heat with no breakthrough is a silent hole.
    #[test]
    fn a_new_plume_always_changes_the_vents() {
        let map = HexMap::new(MIN_FREQ);
        for seed in [7u64, 42, 1234, 777] {
            let bare = SeamField::new(&map, DEFAULT_CELLS, 0, seed);
            let plumed = SeamField::new(&map, DEFAULT_CELLS, 12, seed);
            let a = CrustField::derive(&map, &bare);
            let b = CrustField::derive(&map, &plumed);
            assert_ne!(
                a.vents(),
                b.vents(),
                "seed {seed}: twelve plumes left the vent map untouched"
            );
        }
    }

    /// **The vents are the seams' consequence.** The same seam field derives
    /// the same vents; a re-roll moves them; and every derivation stays inside
    /// the CURRENT tiling's bounds.
    #[test]
    fn vents_follow_the_seam_roll() {
        let (map, mut seams) = field();
        let a = CrustField::derive(&map, &seams);
        let b = CrustField::derive(&map, &seams);
        assert_eq!(a.vents(), b.vents(), "same field, same vents");
        seams.randomize(&map);
        let c = CrustField::derive(&map, &seams);
        assert_ne!(a.vents(), c.vents(), "a re-roll moves the vents");
        assert!(c.vents().iter().all(|v| (*v as usize) < map.len()));
    }
}
