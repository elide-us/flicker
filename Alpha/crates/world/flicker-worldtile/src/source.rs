//! **The tile source** — where a planet's aggregate truth comes from.
//!
//! The migration machinery (shape + materialize) asks a planet exactly two
//! questions per cell: *how thick is the whole solid stack here* (the relief
//! field), and *what beds is it made of, bottom to top* (the conformable
//! drape). [`TileSource`] is those two questions as a trait, so the same
//! machinery serves BOTH planets we have:
//!
//! - the retired chemistry sim's [`flicker_poc_chemistry::World`] (the T6
//!   lineage — its tests keep guarding the mechanics), and
//! - **the Populous bench's committed planet** ([`PlanetSource`] over a
//!   [`flicker_worldengine::PlanetEpoch`]) — the V4 generation-mode path
//!   (two-modes spec, 2026-08-28), and
//! - **the V5 wave planet** ([`ResonanceSource`] over a
//!   [`flicker_worldengine::Composed`] — the WorldBench's composed, epoched or
//!   eroded world with its hardness), whose ground is a FIELD of position:
//!   the one recipe's layers — every slot, the bubbles, the emitters —
//!   evaluated exactly at every pixel direction, the fine ones the hex grid
//!   does not carry included (V5 slices 4–5, 2026-10-16).
//!
//! One trait, one mechanism, no parallel implementation. A source may lend
//! the migration its own reading of the ground between cell centres
//! ([`TileSource::relief_sampler`]); the neighbourhood blend is the default.

use flicker_poc_chemistry::{crust_thickness_m, density_kg_m3, World};
use flicker_worldengine::recipe::{seam_ribbon, Composed, MaskKind, Recipe};
use flicker_worldengine::{
    Bubbles, CrustField, HarmonicWave, HexMap, PlanetEpoch, PlanetWave, Rock, SeamField,
    VolcanoShape, Volcanoes, WaveMask,
};
use flicker_worldgrid::{icosphere_with_outlines, Sphere};
use glam::{DVec2, DVec3};

use crate::erosion::{BedProps, Eroder, ErosionParams, PassReport};
use crate::mask::{TILE_DIM, TILE_SPAN_M};
use crate::materialize::Tile;
use crate::shape::Neighbourhood;

/// One bed of a cell's stack: a material code and its thickness.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Bed {
    /// Material code — meaningful to the source's own registry; the baked
    /// index documents the mapping. For the chemistry world it is the
    /// stratum index; for a planet epoch it is a [`planet material
    /// code`](PlanetSource) below.
    pub material: u8,
    /// Thickness, metres.
    pub thickness_m: f64,
}

/// The two questions the migration asks of a planet — and the one reading a
/// source may lend it.
pub trait TileSource {
    /// The hex grid the planet stands on.
    fn grid(&self) -> &Sphere;
    /// Total solid-stack thickness at `cell`, metres — the relief field.
    fn thickness_m(&self, cell: usize) -> f64;
    /// The stack at `cell`, bottom → top. Zero-thickness beds are omitted.
    fn beds_m(&self, cell: usize) -> Vec<Bed>;
    /// **The ground as a function of position** for the pixels of `cell`'s
    /// tile, metres: a closure the migration asks at every pixel direction
    /// (and one ring past the rim for the skirt). The default is the T6
    /// neighbourhood blend of the columns' thickness — smooth, seam-exact,
    /// reading each centre as its own column. A source whose relief is a
    /// field of position overrides it to add the detail the blend cannot
    /// carry; it must stay a pure function of position so two tiles agree at
    /// their seam, and read the cell's own thickness at its centre.
    fn relief_sampler<'a>(&'a self, cell: usize) -> Box<dyn Fn(DVec3) -> f64 + Sync + 'a>
    where
        Self: Sized,
    {
        let hood = Neighbourhood::around(self, cell);
        Box::new(move |p| hood.relief_at(p))
    }
}

impl TileSource for World {
    fn grid(&self) -> &Sphere {
        &self.grid
    }

    fn thickness_m(&self, cell: usize) -> f64 {
        crust_thickness_m(&self.columns[cell], self.cell_area_m2())
    }

    fn beds_m(&self, cell: usize) -> Vec<Bed> {
        let area = self.cell_area_m2();
        self.columns[cell]
            .layers
            .iter()
            .enumerate()
            .filter_map(|(i, bed)| {
                let density = density_kg_m3(bed);
                (density > 0.0).then(|| Bed {
                    material: i as u8,
                    thickness_m: bed.mass_kg() / (density * area),
                })
            })
            .filter(|b| b.thickness_m > 0.0)
            .collect()
    }
}

/// Planet-epoch material codes — the baked map's `material` byte. Stable,
/// documented in the bake index; a vein-bearing stratum carries
/// `VEIN_BASE + (vein - 1)` so the ore identity survives into the texture.
pub const MAT_BASE: u8 = 1;
pub const MAT_STRATUM: u8 = 2;
pub const MAT_VOLCANIC: u8 = 3;
pub const MAT_ROCK: u8 = 4;
pub const MAT_SEDIMENT: u8 = 5;
/// The V5 wave planet's consolidated crust — the resonance stack's relief
/// (its hardness rides the cell's ledger, not the code).
pub const MAT_WAVE_CRUST: u8 = 6;
/// Vein-bearing stratum codes start here: `VEIN_BASE + vein_kind_index`.
pub const MAT_VEIN_BASE: u8 = 16;

/// **A committed Populous planet as a tile source.**
///
/// Wraps a validated [`PlanetEpoch`]: the grid is rebuilt from the recipe
/// (`icosphere(freq)` — the same deterministic roll the bench stood on), and
/// the ledger's per-hex solid stack becomes the beds. Ledger heights ride the
/// bench's tile-width units; `relief_scale_m` converts one unit to metres —
/// a bake LEVER (the bench's unit is its own visual convention, not a
/// physical claim), recorded in the bake index so the choice is data.
pub struct PlanetSource {
    epoch: PlanetEpoch,
    grid: Sphere,
    /// Each cell's hex outline on the unit sphere — the tile mask's geometry.
    outlines: Vec<Vec<glam::Vec3>>,
    /// Metres one ledger height-unit spans.
    relief_scale_m: f64,
    /// The sea's level in the same metres, solved once from the conserved
    /// water volume over the planet's own hypsometry (the bench's
    /// `resolve_sea`, replayed at bake scale).
    sea_level_m: f64,
}

impl PlanetSource {
    /// Stand the planet up from its epoch. `relief_scale_m` is the metres
    /// per ledger unit lever.
    pub fn new(epoch: PlanetEpoch, relief_scale_m: f64) -> Self {
        let (grid, outlines) = icosphere_with_outlines(epoch.recipe.freq);
        let mut src = Self {
            epoch,
            grid,
            outlines,
            relief_scale_m,
            sea_level_m: 0.0,
        };
        src.sea_level_m = src.solve_sea_level_m();
        src
    }

    /// The epoch this planet stands on.
    pub fn epoch(&self) -> &PlanetEpoch {
        &self.epoch
    }

    /// A cell's hex outline (unit-sphere corner loop).
    pub fn outline(&self, cell: usize) -> &[glam::Vec3] {
        &self.outlines[cell]
    }

    /// The planet's radius in metres — the canon size model at the recipe's
    /// frequency.
    pub fn radius_m(&self) -> f64 {
        clayengine::diameter_mi(self.epoch.recipe.freq) * clayengine::METERS_PER_MILE / 2.0
    }

    /// The solved sea level, metres above the datum the ledger heights ride.
    pub fn sea_level_m(&self) -> f64 {
        self.sea_level_m
    }

    /// The ledger's solid ground at a cell, in LEDGER units (pre-scale).
    fn ground_units(&self, cell: usize) -> f64 {
        let l = &self.epoch.ledger;
        f64::from(l.base[cell])
            + f64::from(l.l3_h[cell])
            + f64::from(l.l4_h[cell])
            + f64::from(l.rock[cell])
            + f64::from(l.sediment[cell])
    }

    /// The bench's sea solve, replayed over the file: find the level at
    /// which the standing water (conserved volume minus the share locked in
    /// ice, area-weighted exactly as the ledger weights it) fits the
    /// planet's hypsometry. Bisection — the fill volume is monotone in the
    /// level.
    fn solve_sea_level_m(&self) -> f64 {
        let n = self.grid.len();
        // Area weights relative to the mean cell — the ledger's own
        // convention for its conserved volume.
        let mean = self.grid.area.iter().map(|&a| f64::from(a)).sum::<f64>() / n as f64;
        let rel: Vec<f64> = self
            .grid
            .area
            .iter()
            .map(|&a| f64::from(a) / mean)
            .collect();
        let free_units =
            f64::from(self.epoch.era.water_volume) - f64::from(self.epoch.era.ice_locked);
        if free_units <= 0.0 {
            return 0.0;
        }
        let ground: Vec<f64> = (0..n).map(|i| self.ground_units(i)).collect();
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for &g in &ground {
            hi = hi.max(g);
        }
        hi += free_units; // a level above every column floods everything
        for _ in 0..64 {
            let mid = 0.5 * (lo + hi);
            let held: f64 = (0..n).map(|i| rel[i] * (mid - ground[i]).max(0.0)).sum();
            if held < free_units {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi) * self.relief_scale_m
    }
}

impl TileSource for PlanetSource {
    fn grid(&self) -> &Sphere {
        &self.grid
    }

    fn thickness_m(&self, cell: usize) -> f64 {
        self.ground_units(cell) * self.relief_scale_m
    }

    fn beds_m(&self, cell: usize) -> Vec<Bed> {
        let l = &self.epoch.ledger;
        let s = self.relief_scale_m;
        // The stratum bed carries the ore identity where a vein stands.
        let stratum_mat = match l.vein[cell] {
            0 => MAT_STRATUM,
            v => MAT_VEIN_BASE.saturating_add(v - 1),
        };
        [
            (MAT_BASE, f64::from(l.base[cell])),
            (stratum_mat, f64::from(l.l3_h[cell])),
            (MAT_VOLCANIC, f64::from(l.l4_h[cell])),
            (MAT_ROCK, f64::from(l.rock[cell])),
            (MAT_SEDIMENT, f64::from(l.sediment[cell])),
        ]
        .into_iter()
        .filter(|&(_, t)| t > 0.0)
        .map(|(material, t)| Bed {
            material,
            thickness_m: t * s,
        })
        .collect()
    }
}

/// **How the ball's relief becomes ground metres.** The V5 relief is a
/// fraction of the radius, exaggerated so the whole span reads on a globe
/// (the composed abyss-to-summit span is ~0.09 of the radius); the ground
/// underfoot is in true metres, where that span is Earth's Mariana-to-Everest
/// ~14 km — a forty-fold difference. The lever, like `PlanetSource`'s
/// `relief_scale_m`, is data a bake records; this is its opening value.
pub const V5_RELIEF_SCALE: f64 = 0.025;

/// One of the stack's layers as the ground reads it: the train, its heard
/// amplitude, how it is cut, and its contribution at every tile centre (what
/// the hex grid carries of it — the blend of these is subtracted so the exact
/// value adds only what the grid cannot hold).
struct LayerField {
    wave: HarmonicWave,
    amp: f32,
    cut: MaskKind,
    mask: Option<WaveMask>,
    tiles: Vec<f32>,
}

/// **A cluster's ladder rung**: its LOD8 vector (the pixel, metres above
/// the datum) and the `(CLUSTER_DIM >> lod)²` vectors of the asked level as
/// offsets over it, row-major. LOD8's one offset is zero.
#[derive(Clone, Debug)]
pub struct ClusterLod {
    pub lod: u8,
    pub base_m: f64,
    pub offsets_m: Vec<f32>,
}

impl ClusterLod {
    /// Vectors an axis.
    pub fn n(&self) -> usize {
        (clayengine::CLUSTER_DIM >> self.lod) as usize
    }

    /// The height above the datum of vector `i` (row-major), metres.
    pub fn height_m(&self, i: usize) -> f64 {
        self.base_m + f64::from(self.offsets_m[i])
    }

    /// The lowest and highest offsets, metres.
    pub fn span(&self) -> (f32, f32) {
        self.offsets_m
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), h| (lo.min(*h), hi.max(*h)))
    }
}

/// **The V5 wave planet as a tile source** — the WorldBench's world at
/// whatever stage it stands (composed, epoched, eroded: heights, sea and
/// hardness per tile, the era's sediment where it ran) read through the hex
/// grid, with the ground BETWEEN the centres a field of position:
///
/// `ground(p) = blend(heights)(p) + [fields(p) − blend(fields at tile centres)(p)]`
///
/// — the T6 blend honours every hex's own height at its centre and keeps the
/// seam exact; the FIELDS — the one recipe's layers, the fine slots the grid
/// does not carry among them, the bubbles' every octave, the emitters —
/// evaluated exactly at `p` with the SAME blend of their own tile samples
/// subtracted, add only the detail the hex grid could not carry (zero at
/// every centre, so the aggregate stays the pixels' rollup; a fine layer's
/// tile samples alias, and their blend is a smooth undulation of the layer's
/// own amplitude, which is the hex scale's to hold). Every term is a pure
/// function of position, so two hexes cannot disagree at their seam, and the
/// same `(recipe, world, cell)` is the same map. Heights ride the ball's
/// radius fraction; [`V5_RELIEF_SCALE`] × the planet's radius turns them into
/// metres above the world's deepest floor (the datum).
pub struct ResonanceSource<'a> {
    map: &'a HexMap,
    heights: Vec<f32>,
    sediment: Vec<f32>,
    /// The hex's hardness ledger, `0..1` per tile — what the cut reads.
    hardness: Vec<f32>,
    sea: f32,
    datum: f32,
    /// Metres per radius fraction.
    scale_m: f64,
    radius_m: f64,
    wave: Option<(PlanetWave, Option<WaveMask>, f32, Vec<f32>)>,
    layers: Vec<LayerField>,
    ribbon: Vec<f32>,
    /// The emitters' waves, exact at any direction.
    emitters: Option<Volcanoes>,
    /// BUBBLES ON BUBBLES — every octave, exact at any direction, with the
    /// field's (aliased) tile samples, subtracted as their blend.
    bubbles: Option<(Bubbles, Vec<f32>)>,
}

impl<'a> ResonanceSource<'a> {
    /// Stand the world up as a source: `world` is the stage the globe draws
    /// (heights, sea, hardness in radius units), `sediment` the era's loose
    /// sediment per tile in the same units where the era has run (`None`
    /// before it), `relief_scale` the metres lever ([`V5_RELIEF_SCALE`]).
    pub fn new(
        map: &'a HexMap,
        recipe: &Recipe,
        world: &Composed,
        sediment: Option<&[f32]>,
        relief_scale: f64,
    ) -> Self {
        let n = map.len();
        let radius_m = crate::radius_for_freq(map.freq());
        let scale_m = radius_m * relief_scale;
        let floor = world
            .heights
            .iter()
            .copied()
            .filter(|h| h.is_finite())
            .fold(f32::MAX, f32::min);
        let datum = if floor.is_finite() { floor } else { 0.0 };
        // The planetary wave and the stack's layers, each with its tile
        // samples — the same arithmetic `compose` runs, kept per layer.
        let wave = world_wave(map, recipe);
        let any_seam = recipe.stack.on
            && recipe
                .stack
                .slots
                .iter()
                .any(|s| s.on && s.mask == MaskKind::Seam);
        let ribbon = if any_seam {
            seam_ribbon(map, recipe)
        } else {
            Vec::new()
        };
        let mut layers = Vec::new();
        if recipe.stack.on {
            for (i, slot) in recipe.stack.slots.iter().enumerate() {
                if !slot.on {
                    continue;
                }
                let wave = HarmonicWave::new(
                    map,
                    recipe.stack.field_seed(i, slot.frequency),
                    slot.frequency,
                    slot.axes,
                );
                let amp = recipe.stack.heard(i, slot);
                let mask = (slot.mask == MaskKind::Region)
                    .then(|| WaveMask::new(map, recipe.stack.mask_seed_of(i, slot.frequency)));
                let tiles = (0..n)
                    .map(|t| {
                        let v = wave.value(t as u32);
                        match slot.mask {
                            MaskKind::Region => {
                                v * amp * mask.as_ref().map_or(1.0, |m| m.value(t as u32))
                            }
                            MaskKind::Seam => (0.5 + 0.5 * v) * amp * ribbon[t],
                            MaskKind::Off => v * amp,
                        }
                    })
                    .collect();
                layers.push(LayerField {
                    wave,
                    amp,
                    cut: slot.mask,
                    mask,
                    tiles,
                });
            }
        }
        let bubbles = recipe.bubbles.on.then(|| {
            let b = Bubbles::new(&recipe.bubbles);
            let tiles = (0..n).map(|t| b.eval(map.direction(t as u32))).collect();
            (b, tiles)
        });
        let emitters = (recipe.heat.on && recipe.heat.emitters_on).then(|| {
            let seams = SeamField::new(map, recipe.heat.cells, recipe.heat.spots, recipe.heat.seed);
            let crust = CrustField::derive(map, &seams);
            let shape: VolcanoShape = recipe.heat.emitters;
            Volcanoes::new(map, &seams, crust.vents(), shape)
        });
        Self {
            map,
            heights: world.heights.clone(),
            sediment: sediment.map_or_else(|| vec![0.0; n], <[f32]>::to_vec),
            hardness: world.hardness.clone(),
            sea: world.sea,
            datum,
            scale_m,
            radius_m,
            wave,
            layers,
            ribbon,
            emitters,
            bubbles,
        }
    }

    /// The planet's radius, metres — the canon size model at the map's
    /// frequency, the radius the tile frame must be built with.
    pub fn radius_m(&self) -> f64 {
        self.radius_m
    }

    /// **The hardness as a function of position** for the pixels of `cell`:
    /// the hex grid's hardness ledger ([`flicker_worldengine::Composed`]'s
    /// channel — the layers' weighed vote, the epochs' hardening, the era's
    /// sediment) read through the SAME neighbourhood blend the heights ride,
    /// so two hexes agree about the ground's resistance at their seam as
    /// they do about its height. No new ledger: the cut reads the one the
    /// planet keeps, finer.
    pub fn hardness_sampler(&self, cell: usize) -> impl Fn(DVec3) -> f32 + '_ {
        let hood = Neighbourhood::of(self.map.grid(), cell, |i| f64::from(self.hardness[i]));
        move |p: DVec3| hood.relief_at(p) as f32
    }

    /// The migrated beds' props for the pixel eroder, parallel to
    /// [`TileSource::beds_m`]: crust at unit resistance (the hex's hardness
    /// grades it pixel by pixel), the era's sediment loose.
    pub fn bed_props(&self, cell: usize) -> Vec<BedProps> {
        self.beds_m(cell)
            .iter()
            .map(|bed| {
                if bed.material == MAT_SEDIMENT {
                    BedProps {
                        density_kg_m3: 2200.0,
                        resistance: Rock::Shale.resistance(),
                    }
                } else {
                    BedProps {
                        density_kg_m3: 2700.0,
                        resistance: 1.0,
                    }
                }
            })
            .collect()
    }

    /// **The water over the hex**: `cycles` passes of the pixel eroder
    /// ([`Eroder::pass_graded`]) over a materialized tile — rain gathers
    /// downhill, cuts by the hex's hardness read at every pixel, carries, and
    /// settles; nothing cuts under the sea line; what leaves the rim is
    /// reported, not lost. The same water cycle the aggregate era runs over
    /// the hex map, at the pixel. Returns the passes' summed ledger.
    pub fn cut(&self, tile: &mut Tile, cycles: u32) -> PassReport {
        let n = TILE_DIM as usize * TILE_DIM as usize;
        let mut grade = vec![1.0f32; n];
        let hardness = self.hardness_sampler(tile.cell as usize);
        for (x, y) in tile.mask.iter() {
            grade[(y as usize) * TILE_DIM as usize + x as usize] =
                hardness(tile.frame.direction(x, y)).clamp(0.0, 1.0);
        }
        let props = self.bed_props(tile.cell as usize);
        let params = ErosionParams {
            sea_m: Some(self.sea_level_m()),
            ..ErosionParams::default()
        };
        let mut eroder = Eroder::new();
        let mut total = PassReport::default();
        for _ in 0..cycles {
            let r = eroder.pass_graded(tile, &props, &params, Some(&grade));
            total.eroded_kg += r.eroded_kg;
            total.deposited_kg += r.deposited_kg;
            total.exported_kg += r.exported_kg;
            total.lake_px = r.lake_px;
            total.outlet_px = r.outlet_px;
        }
        total
    }

    /// The world's highest column above the datum, metres — the top of the
    /// planet's height bands, what a tile's colours are read against.
    pub fn top_m(&self) -> f64 {
        let top = self.heights.iter().copied().fold(f32::MIN, f32::max);
        f64::from(top - self.datum).max(0.0) * self.scale_m
    }

    /// Metres one unit of the ball's height (a radius fraction) spans.
    pub fn scale_m(&self) -> f64 {
        self.scale_m
    }

    /// The sea, metres above the datum.
    pub fn sea_level_m(&self) -> f64 {
        f64::from(self.sea - self.datum) * self.scale_m
    }

    /// The world's deepest floor — what ground zero stands at, radius units.
    pub fn datum(&self) -> f32 {
        self.datum
    }

    /// A cell's hex outline (unit-sphere corner loop).
    pub fn outline(&self, cell: usize) -> &[glam::Vec3] {
        self.map.outline(cell as u32)
    }

    /// The map the world stands on.
    pub fn map(&self) -> &HexMap {
        self.map
    }

    /// **Materialise one hex** — the migration over this source with its own
    /// radius and outline.
    pub fn materialize(&self, cell: usize) -> crate::materialize::Tile {
        crate::materialize::materialize(self, cell, self.radius_m, self.outline(cell))
    }

    /// The stack's exact value at `p` (radius units): the planetary wave
    /// through its mask, every layer through its cut (the seam layers through
    /// the ribbon's blend at `p`), and the emitters' waves.
    fn stack_at(&self, p: DVec3, ribbon_at: f32) -> f64 {
        // The planetary wave, the masks and the emitters are kilometres wide:
        // f32 serves them; the stack's trains and the bubbles — the fields with
        // structure under the pixel — read in f64, so the ladder resolves a
        // voxel (Aaron 2026-10-16: the formula ramps down to LOD0).
        let pf = p.as_vec3();
        let mut h = f64::from(self.emitters.as_ref().map_or(0.0, |v| v.eval(pf)));
        h += self.bubbles.as_ref().map_or(0.0, |(b, _)| b.eval64(p));
        if let Some((wave, mask, amp, _)) = &self.wave {
            h += f64::from(wave.eval(pf) * amp * mask.as_ref().map_or(1.0, |m| m.eval(pf)));
        }
        for l in &self.layers {
            let v = l.wave.eval64(p);
            let amp = f64::from(l.amp);
            h += match l.cut {
                MaskKind::Region => {
                    v * amp * f64::from(l.mask.as_ref().map_or(1.0, |m| m.eval(pf)))
                }
                MaskKind::Seam => (0.5 + 0.5 * v) * amp * f64::from(ribbon_at),
                MaskKind::Off => v * amp,
            };
        }
        h
    }

    /// **A cluster's vectors at `lod`** — the ladder under pixel `(x, y)` of
    /// `tile` (Aaron 2026-10-16: *"each pixel in the hex heightmap is the
    /// LOD8 value … LOD0 = 256 vectors, LOD1 = 128 (stride 2) … LOD7 = 2"*):
    /// `CLUSTER_DIM >> lod` samples an axis, each the ONE recipe read at the
    /// centre of its `2^lod`-voxel block, plus the pixel's own residual over
    /// the field (the water's cut, the column's correction) carried between
    /// the four pixel centres around the sample, so the surface is
    /// continuous across clusters and LOD8 is the pixel itself. The vectors
    /// come as OFFSETS over the cluster's LOD8 vector (the pixel, f64): what
    /// a sparse cluster holds at instantiation is that one vector, what it
    /// resolves on approach is the finer ones over it, and millimetres over
    /// a few metres keep their place in f32 where 11 km above the datum would
    /// round them (Aaron: *"the difference in altitude might get down to
    /// millimeters, that's expected … make sure that the rolls and ridges
    /// that are defined actually express correctly at the LOD0 finest
    /// point"*). Row-major (`y` then `x`), the tile's frame.
    pub fn cluster_lod(&self, tile: &Tile, x: u32, y: u32, lod: u8) -> ClusterLod {
        let lod = lod.min(clayengine::MAX_LOD);
        let n = (clayengine::CLUSTER_DIM >> lod) as usize;
        let ground = self.relief_sampler(tile.cell as usize);
        let per_px = TILE_SPAN_M / f64::from(TILE_DIM);
        let voxel_m = per_px / f64::from(clayengine::CLUSTER_DIM);
        // The residual at the nine pixel centres around this one (every
        // sample lies within half a pixel of the centre): what the pixel
        // holds that the field alone does not.
        let residual = |px: i64, py: i64| -> f64 {
            let (cx, cy) = (
                px.clamp(0, i64::from(TILE_DIM) - 1) as u32,
                py.clamp(0, i64::from(TILE_DIM) - 1) as u32,
            );
            if !tile.mask.contains(cx, cy) {
                return 0.0;
            }
            f64::from(tile.composite_m(cx, cy)) - ground(tile.frame.direction(cx, cy))
        };
        let mut block = [[0.0f64; 3]; 3];
        for (j, row) in block.iter_mut().enumerate() {
            for (i, r) in row.iter_mut().enumerate() {
                *r = residual(i64::from(x) + i as i64 - 1, i64::from(y) + j as i64 - 1);
            }
        }
        let centre = tile.frame.plane_m(x, y);
        let spacing = voxel_m * f64::from(1u32 << lod);
        let half = f64::from(clayengine::CLUSTER_DIM) * 0.5 * voxel_m;
        // The LOD8 vector: the field at the pixel's centre plus the pixel's
        // own residual there — the pixel itself.
        let base_m = ground(tile.frame.direction(x, y)) + block[1][1];
        let mut offsets_m = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let off = DVec2::new(
                    (i as f64 + 0.5) * spacing - half,
                    (j as f64 + 0.5) * spacing - half,
                );
                let plane = centre + off;
                let f = ground(tile.frame.direction_m(plane));
                // Bilinear over the pixel centres: `u`, `v` in pixels from the
                // block's corner pixel (x − 1, y − 1).
                let u = (off.x / per_px + 1.0).clamp(0.0, 2.0);
                let v = (off.y / per_px + 1.0).clamp(0.0, 2.0);
                let (iu, iv) = ((u.floor() as usize).min(1), (v.floor() as usize).min(1));
                let (tu, tv) = (u - iu as f64, v - iv as f64);
                let r = block[iv][iu] * (1.0 - tu) * (1.0 - tv)
                    + block[iv][iu + 1] * tu * (1.0 - tv)
                    + block[iv + 1][iu] * (1.0 - tu) * tv
                    + block[iv + 1][iu + 1] * tu * tv;
                offsets_m.push((f + r - base_m) as f32);
            }
        }
        ClusterLod {
            lod,
            base_m,
            offsets_m,
        }
    }

    /// The fields' tile sample at `t` — what the hex grid carries of the
    /// stack (every slot, the fine ones aliased, their blend subtracted).
    fn fields_tile(&self, t: usize) -> f32 {
        let mut h = 0.0f32;
        if let Some((_, _, _, tiles)) = &self.wave {
            h += tiles[t];
        }
        for l in &self.layers {
            h += l.tiles[t];
        }
        if let Some(v) = &self.emitters {
            h += v.value(t as u32);
        }
        if let Some((_, tiles)) = &self.bubbles {
            h += tiles[t];
        }
        h
    }
}

/// The planetary wave as the ground reads it: the field, its mask (when on),
/// its amplitude and its tile samples.
fn world_wave(map: &HexMap, r: &Recipe) -> Option<(PlanetWave, Option<WaveMask>, f32, Vec<f32>)> {
    if !r.wave.on {
        return None;
    }
    let wave = PlanetWave::with_lobe(map, r.wave.seed, r.wave.lobe);
    let mask = r.wave.mask.on.then(|| WaveMask::new(map, r.wave.mask.seed));
    let tiles = (0..map.len())
        .map(|t| {
            wave.value(t as u32) * r.wave.amp * mask.as_ref().map_or(1.0, |m| m.value(t as u32))
        })
        .collect();
    Some((wave, mask, r.wave.amp, tiles))
}

impl TileSource for ResonanceSource<'_> {
    fn grid(&self) -> &Sphere {
        self.map.grid()
    }

    fn thickness_m(&self, cell: usize) -> f64 {
        f64::from(self.heights[cell] - self.datum).max(0.0) * self.scale_m
    }

    fn beds_m(&self, cell: usize) -> Vec<Bed> {
        let total = f64::from(self.heights[cell] - self.datum).max(0.0);
        let sed = f64::from(self.sediment[cell]).clamp(0.0, total);
        [(MAT_WAVE_CRUST, total - sed), (MAT_SEDIMENT, sed)]
            .into_iter()
            .filter(|&(_, t)| t > 0.0)
            .map(|(material, t)| Bed {
                material,
                thickness_m: t * self.scale_m,
            })
            .collect()
    }

    fn relief_sampler<'b>(&'b self, cell: usize) -> Box<dyn Fn(DVec3) -> f64 + Sync + 'b> {
        let grid = self.map.grid();
        let datum = f64::from(self.datum);
        let coarse = Neighbourhood::of(grid, cell, |i| f64::from(self.heights[i]) - datum);
        let fine = Neighbourhood::of(grid, cell, |i| f64::from(self.fields_tile(i)));
        let ribbon = (!self.ribbon.is_empty())
            .then(|| Neighbourhood::of(grid, cell, |i| f64::from(self.ribbon[i])));
        Box::new(move |p: DVec3| {
            let ribbon_at = ribbon.as_ref().map_or(0.0, |r| r.relief_at(p) as f32);
            let exact = self.stack_at(p, ribbon_at);
            let h = coarse.relief_at(p) + (exact - fine.relief_at(p));
            h.max(0.0) * self.scale_m
        })
    }
}

#[cfg(test)]
mod resonance_tests {
    use super::*;
    use flicker_worldengine::recipe::compose;
    use flicker_worldengine::HarmonicWave;
    use flicker_worldengine::MIN_FREQ;

    /// **The V5 ground is a field of position**: at every cell centre it reads
    /// the cell's own height exactly (the aggregate stays the rollup); two
    /// neighbouring cells read the SAME ground at points on their shared
    /// boundary (the seam, by construction); the same world samples the same
    /// ground twice; and between the centres the cluster-scale ground stands
    /// — the pixels are not the blend alone.
    #[test]
    fn the_v5_ground_is_a_field_of_position() {
        let map = HexMap::new(MIN_FREQ);
        let recipe = Recipe::default_at(MIN_FREQ, 7);
        let c = compose(&map, &recipe);
        let src = ResonanceSource::new(&map, &recipe, &c, None, V5_RELIEF_SCALE);
        assert!(src.radius_m() > 1.0e6);
        assert!(src.sea_level_m() > 0.0);
        let grid = src.grid();
        for cell in [0usize, 777, 4321, map.len() - 1] {
            let ground = src.relief_sampler(cell);
            let centre = grid.dirs[cell].as_dvec3().normalize();
            let here = ground(centre);
            let own = src.thickness_m(cell);
            assert!(
                (here - own).abs() < 1e-3 * own.max(1.0),
                "cell {cell}: the centre reads its own column: {here} vs {own}"
            );
            // The seam.
            for &j in &grid.neighbors[cell] {
                let j = j as usize;
                let other = src.relief_sampler(j);
                let b = grid.dirs[j].as_dvec3().normalize();
                for k in 0..5 {
                    let t = 0.3 + 0.1 * k as f64;
                    let seam = (centre * (1.0 - t) + b * t).normalize();
                    let (from_a, from_b) = (ground(seam), other(seam));
                    assert!(
                        (from_a - from_b).abs() < 1e-6 * from_a.abs().max(1.0),
                        "cells {cell}/{j} disagree at their seam: {from_a} vs {from_b}"
                    );
                }
            }
            // Determinism.
            let again = ResonanceSource::new(&map, &recipe, &c, None, V5_RELIEF_SCALE);
            let g2 = again.relief_sampler(cell);
            let probe =
                (centre + grid.dirs[grid.neighbors[cell][0] as usize].as_dvec3() * 0.2).normalize();
            assert_eq!(
                ground(probe),
                g2(probe),
                "the same world is the same ground"
            );
        }
        // The fine layers stand between the centres: a world with the slots
        // the grid does not carry switched OFF reads the blend plus the
        // carried layers' detail alone, and the difference across a short
        // walk is the fine layers' own texture — the same recipe, finer.
        let mut flat = recipe.clone();
        let carried = map.resolving_frequency();
        for slot in &mut flat.stack.slots {
            if slot.frequency > carried {
                slot.on = false;
            }
        }
        assert!(
            flat.stack.active() < recipe.stack.active(),
            "there are fine layers"
        );
        let bare = ResonanceSource::new(&map, &flat, &c, None, V5_RELIEF_SCALE);
        let cell = 777usize;
        let (g, b) = (src.relief_sampler(cell), bare.relief_sampler(cell));
        let centre = grid.dirs[cell].as_dvec3().normalize();
        let east = centre.any_orthonormal_vector();
        let mut differ = 0usize;
        for k in 1..40 {
            let p = (centre + east * (k as f64 * 0.0003)).normalize();
            if (g(p) - b(p)).abs() > 0.01 {
                differ += 1;
            }
        }
        assert!(
            differ > 20,
            "the fine layers are there: {differ}/39 samples differ"
        );
        // THE LADDER UNDER THE PIXEL: a cluster's vectors at every LOD from
        // the one recipe; LOD8 is the pixel; the levels hold 256 >> lod an
        // axis as offsets over the LOD8 vector; the surface runs continuous
        // across clusters; a coarser vector stands between the finer two it
        // covers; and a defined ROLL expresses at the voxel: the finest
        // train alone, read through the whole ladder, is its own closed form
        // to the millimetre.
        {
            let land = (0..map.len())
                .max_by(|a, b| c.heights[*a].total_cmp(&c.heights[*b]))
                .expect("tiles");
            let mut tile = src.materialize(land);
            src.cut(&mut tile, 2);
            let (x, y) = (1024u32, 1024u32);
            assert!(tile.mask.contains(x, y) && tile.mask.contains(x + 1, y));
            let lod8 = src.cluster_lod(&tile, x, y, 8);
            assert_eq!(
                (lod8.n(), lod8.offsets_m.len(), lod8.offsets_m[0]),
                (1, 1, 0.0)
            );
            assert!(
                (lod8.base_m - f64::from(tile.composite_m(x, y))).abs() < 1e-2,
                "LOD8 is the pixel: {} vs {}",
                lod8.base_m,
                tile.composite_m(x, y)
            );
            for lod in [7u8, 4, 1, 0] {
                let n = (clayengine::CLUSTER_DIM >> lod) as usize;
                let v = src.cluster_lod(&tile, x, y, lod);
                assert_eq!(
                    (v.n(), v.offsets_m.len()),
                    (n, n * n),
                    "LOD{lod} holds {n}²"
                );
                assert!(v.offsets_m.iter().all(|h| h.is_finite()));
                assert_eq!(v.base_m, lod8.base_m, "one LOD8 vector under every level");
            }
            let lod0 = src.cluster_lod(&tile, x, y, 0);
            let lod1 = src.cluster_lod(&tile, x, y, 1);
            let (lo, hi) = lod0.span();
            assert!(
                hi - lo < 200.0,
                "a cluster's relief is metres, not a cliff: {lo}–{hi}"
            );
            let dim = clayengine::CLUSTER_DIM as usize;
            for row in 0..dim {
                for col in 1..dim {
                    let d = (lod0.offsets_m[row * dim + col] - lod0.offsets_m[row * dim + col - 1])
                        .abs();
                    assert!(d < 2.0, "no step between voxels: {d} m");
                }
            }
            for row in 0..dim / 2 {
                for col in 0..dim / 2 {
                    let coarse = lod1.offsets_m[row * (dim / 2) + col];
                    let fine = [
                        lod0.offsets_m[(2 * row) * dim + 2 * col],
                        lod0.offsets_m[(2 * row) * dim + 2 * col + 1],
                        lod0.offsets_m[(2 * row + 1) * dim + 2 * col],
                        lod0.offsets_m[(2 * row + 1) * dim + 2 * col + 1],
                    ];
                    let (fl, fh) = fine
                        .iter()
                        .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
                    assert!(
                        coarse >= fl - 0.5 && coarse <= fh + 0.5,
                        "the coarser vector stands among the finer: {coarse} in {fl}..{fh}"
                    );
                }
            }
            // Across the seam between clusters: the last column of this one
            // and the first of the next differ by a voxel's slope.
            let next = src.cluster_lod(&tile, x + 1, y, 0);
            for row in 0..dim {
                let d = (lod0.height_m(row * dim + dim - 1) - next.height_m(row * dim)).abs();
                assert!(d < 2.0, "continuous across clusters: {d} m");
            }
            // THE ROLL EXPRESSES: the finest train alone — no wave, no bubbles,
            // no emitters, every other slot off — read through materialize and
            // the ladder, against the train's own closed form at every voxel
            // (its heard height, the tile's metres lever). The hex grid does
            // not carry this train; the ladder carries it to the millimetre.
            let mut one = recipe.clone();
            one.wave.on = false;
            one.heat.emitters_on = false;
            one.bubbles.on = false;
            let finest = one
                .stack
                .slots
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.frequency.total_cmp(&b.1.frequency))
                .map(|(i, _)| i)
                .expect("slots");
            for (i, s) in one.stack.slots.iter_mut().enumerate() {
                s.on = i == finest;
            }
            let slot = one.stack.slots[finest];
            assert!(
                slot.frequency > map.resolving_frequency(),
                "a train under the hex"
            );
            // The grid carries nothing of this train, so the composed world is
            // flat; stand it a few hundred metres over the datum (one far tile
            // holds the floor) so the train's troughs never meet the datum's
            // clamp — the T6 floor that no column goes under.
            let mut c1 = compose(&map, &one);
            for h in c1.heights.iter_mut() {
                *h += 0.002;
            }
            let far = (0..map.len())
                .min_by(|a, b| {
                    map.direction(*a as u32)
                        .dot(map.direction(land as u32))
                        .total_cmp(&map.direction(*b as u32).dot(map.direction(land as u32)))
                })
                .expect("tiles");
            c1.heights[far] = 0.0;
            let src1 = ResonanceSource::new(&map, &one, &c1, None, V5_RELIEF_SCALE);
            let tile1 = src1.materialize(land);
            let roll = src1.cluster_lod(&tile1, x, y, 0);
            let train = HarmonicWave::new(
                &map,
                one.stack.field_seed(finest, slot.frequency),
                slot.frequency,
                slot.axes,
            );
            let amp_m = f64::from(one.stack.heard(finest, &slot)) * src1.scale_m();
            let centre = tile1.frame.plane_m(x, y);
            let per_px = TILE_SPAN_M / f64::from(TILE_DIM);
            let voxel_m = per_px / f64::from(clayengine::CLUSTER_DIM);
            let half = f64::from(clayengine::CLUSTER_DIM) * 0.5 * voxel_m;
            let at_centre = train.eval64(tile1.frame.direction(x, y));
            let mask_at = |d: DVec3| -> f64 {
                match slot.mask {
                    MaskKind::Region => {
                        let m = flicker_worldengine::WaveMask::new(
                            &map,
                            one.stack.mask_seed_of(finest, slot.frequency),
                        );
                        f64::from(m.eval(d.as_vec3()))
                    }
                    _ => 1.0,
                }
            };
            let mut worst = 0.0f64;
            let (mut rises, mut falls) = (0usize, 0usize);
            for j in 0..dim {
                for i in 0..dim {
                    let plane = centre
                        + DVec2::new(
                            (i as f64 + 0.5) * voxel_m - half,
                            (j as f64 + 0.5) * voxel_m - half,
                        );
                    let d = tile1.frame.direction_m(plane);
                    let want = amp_m
                        * (train.eval64(d) * mask_at(d)
                            - at_centre * mask_at(tile1.frame.direction(x, y)));
                    let got = f64::from(roll.offsets_m[j * dim + i]);
                    worst = worst.max((got - want).abs());
                    if i > 0 {
                        let step = roll.offsets_m[j * dim + i] - roll.offsets_m[j * dim + i - 1];
                        if step > 0.0 {
                            rises += 1;
                        } else if step < 0.0 {
                            falls += 1;
                        }
                    }
                }
            }
            assert!(
                worst < 0.02,
                "the roll expresses at the voxel: worst {worst} m"
            );
            assert!(
                rises + falls > dim * dim / 2,
                "the voxels are not a plateau: {rises} rises, {falls} falls"
            );
        }
        // THE CUT: the water over a land hex cuts by the hex's hardness and
        // keeps its books; under the sea line nothing cuts.
        let land = (0..map.len())
            .max_by(|a, b| c.heights[*a].total_cmp(&c.heights[*b]))
            .expect("tiles");
        let (mut soft_w, mut hard_w) = (c.clone(), c.clone());
        soft_w.hardness = vec![0.3; map.len()];
        hard_w.hardness = vec![0.9; map.len()];
        let soft = ResonanceSource::new(&map, &recipe, &soft_w, None, V5_RELIEF_SCALE);
        let hard = ResonanceSource::new(&map, &recipe, &hard_w, None, V5_RELIEF_SCALE);
        let mut soft_tile = soft.materialize(land);
        let mut hard_tile = hard.materialize(land);
        let before = soft_tile.composite_m(1024, 1024);
        let props = soft.bed_props(land);
        let params = ErosionParams::default();
        let mass_before = crate::erosion::tile_mass_kg(&soft_tile, &props, &params);
        let rs = soft.cut(&mut soft_tile, 3);
        let rh = hard.cut(&mut hard_tile, 3);
        assert!(rs.eroded_kg > 0.0, "the water cut");
        assert!(
            rs.eroded_kg > rh.eroded_kg * 1.5,
            "soft ground loses more than hard: {} vs {}",
            rs.eroded_kg,
            rh.eroded_kg
        );
        let mass_after = crate::erosion::tile_mass_kg(&soft_tile, &props, &params);
        assert!(
            (mass_before - (mass_after + rs.exported_kg)).abs() < 1e-6 * mass_before,
            "the books balance: {mass_before} = {mass_after} + {}",
            rs.exported_kg
        );
        assert!(
            soft_tile.pixel_born > 0 || rs.deposited_kg == 0.0,
            "settled ground is a bed"
        );
        assert_ne!(
            soft_tile.composite_m(1024, 1024),
            before,
            "the ground moved"
        );
        let deep = (0..map.len())
            .min_by(|a, b| c.heights[*a].total_cmp(&c.heights[*b]))
            .expect("tiles");
        let mut sea_tile = src.materialize(deep);
        let under = src.cut(&mut sea_tile, 2);
        assert_eq!(under.eroded_kg, 0.0, "nothing cuts under the sea line");
        // The beds: crust alone before the era, sediment on top once it runs.
        assert_eq!(src.beds_m(cell).len(), 1);
        assert_eq!(src.beds_m(cell)[0].material, MAT_WAVE_CRUST);
        let sediment = vec![0.0005f32; map.len()];
        let with = ResonanceSource::new(&map, &recipe, &c, Some(&sediment), V5_RELIEF_SCALE);
        let beds = with.beds_m(cell);
        assert_eq!(beds.len(), 2);
        assert_eq!(beds[1].material, MAT_SEDIMENT);
        assert!((beds[0].thickness_m + beds[1].thickness_m - with.thickness_m(cell)).abs() < 1e-6);
    }
}
