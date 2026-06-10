//! World-gen terrain for the hex-map layout — the hex-world **six-epoch
//! simulation** brought onto the properly ordered map.
//!
//! [`WorldGen::generate`] runs the full `flicker-worldgen` chain (Epoch 1
//! composition → … → Epoch 6 erosion/biomes) over this client's own tile
//! numbering and adjacency: same-map neighbours straight from the logical grid
//! ([`crate::build_within_neighbors`]), the across-equator pairing measured at
//! the maps' **rest pose** (right upright, left rolled face-down — so dragging
//! the roll wheels can never change the world), and a stub celestial direction
//! per tile from its dome latitude (right map = northern hemisphere, left =
//! southern; both equator rings meet at latitude 0).
//!
//! All **six epoch layers are retained** per tile ([`WorldGen::epoch_states`])
//! — the generation procedures are still being tweaked and later passes will
//! re-read the lower layers — but only the **consolidated top layer** is drawn:
//! [`WorldGen::upload_meshes`] builds one relief mesh per tile from the Epoch-6
//! state (eroded ground, biome-tinted, fading to bare rock/snow toward the
//! peaks) plus a flat sea sheet over the cells below its sea level. The per-hex
//! tectonic scalars are blended across the six edge-neighbours
//! ([`blend_scalar`]) so terrain joins across tile boundaries instead of
//! stepping at them.
//!
//! Record-flipped (left-map) tiles bake the mirror into the mesh: vertices stay
//! on the regular grid, but heights/materials read the mirrored column, so the
//! drawn terrain mirrors with the tile and logically adjacent edges still
//! correspond across the drawn layout.

use flicker::render::{Mat4, MeshHandle, MeshIndices, MeshVertex, Renderer, Vec2, Vec3};
use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::{
    six_epoch_stack, Biome, CellSample, Epoch1, Epoch1Params, EpochCtx, FieldSampler, HexState,
    EPOCHS,
};

use crate::{edge_normal, left_center, ring_dome_angle, HexInst, APOTHEM, HEX_SIZE, HEX_SPACING};

/// Materials vocabulary directory (the JSON tables), relative to this example.
const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");

/// Grid resolution across the hex footprint (`G × G` cells / vertices).
pub const G: usize = 64;

/// Display squash of the terrain relief. Raw cell elevations span roughly
/// ±340 world units (tectonic base ± orogenic ridges); halving keeps the peaks
/// under the floating number labels while the relief still reads.
const TERRAIN_VSCALE: f32 = 0.5;

/// World-unit span over which the ground fades from its biome colour up to
/// bare rock / snow above the sea level.
const SNOW_SPAN: f32 = 150.0;

/// North–south sampling shift of the left map's noise region, so its tiles
/// (whose logical frame overlaps the right map's) read a disjoint stretch of
/// the world fields. Comfortably past the widest (5-ring) map's diameter.
const LEFT_SAMPLE_SHIFT: f32 = 16.0 * HEX_SPACING;

// Material indices into the mesh shader's demo palette (`mesh.wgsl`), the same
// palette hex-world tints with.
const M_WATER_MID: u32 = 2;
const M_ICE: u32 = 12;
const M_DESERT: u32 = 17;
const M_SAVANNA: u32 = 18;
const M_GRASSLAND: u32 = 19;
const M_FOREST: u32 = 20;
const M_RAINFOREST: u32 = 21;
const M_TAIGA: u32 = 22;
const M_TUNDRA: u32 = 23;
const M_ROCK_HARD: u32 = 25;

/// Load the materials vocabulary the epochs run on; `None` (with a log) if the
/// JSON tables are missing, in which case tiles keep their plain grey fill.
pub fn load_tables() -> Option<Tables> {
    match Tables::from_source(&JsonTableSource::new(MATERIALS_DIR)) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::error!("materials vocabulary failed to load: {e}; tiles stay untextured");
            None
        }
    }
}

/// Pack a two-stop colour ramp into the shader's material word: `primary =
/// cold`, `secondary = hot`, `blend = t`. The mesh shader resolves it to
/// `mix(cold, hot, t)` with no shader change.
fn pack_ramp(cold: u32, hot: u32, t: f32) -> u32 {
    let b = (t.clamp(0.0, 1.0) * 255.0) as u32;
    (cold & 0xFFF) | ((hot & 0xFFF) << 12) | ((b & 0xFF) << 24)
}

/// The biome's surface palette colour (Epoch 6).
fn biome_material(b: Biome) -> u32 {
    match b {
        Biome::Ocean => M_WATER_MID, // (the sea sheet covers it anyway)
        Biome::Ice => M_ICE,
        Biome::Tundra => M_TUNDRA,
        Biome::Taiga => M_TAIGA,
        Biome::Grassland => M_GRASSLAND,
        Biome::Forest => M_FOREST,
        Biome::Rainforest => M_RAINFOREST,
        Biome::Savanna => M_SAVANNA,
        Biome::Desert => M_DESERT,
        Biome::Alpine => M_ROCK_HARD,
    }
}

/// Ground per-cell colour: the hex's biome at the lowlands, fading to bare
/// rock / snow on the cells that rise toward the peaks.
fn ground_material(c: &CellSample, biome: Biome, sea_world: f32) -> u32 {
    let snow_t = ((c.elevation - sea_world) / SNOW_SPAN).clamp(0.0, 1.0);
    pack_ramp(biome_material(biome), M_ROCK_HARD, snow_t)
}

/// Flat-top column half-height at normalized lon `a = x / size`: 1.0 across the
/// flat band `|a| ≤ ½`, tapering to the E/W points at `|a| = 1`.
fn band_width(a: f32) -> f32 {
    let a = a.abs();
    if a <= 0.5 {
        1.0
    } else {
        ((1.0 - a) / 0.5).max(0.0)
    }
}

/// Local XZ position of grid cell `(i, j)`, hex centred at the origin —
/// flat-top, `x` the full point-to-point axis, `z` masked to the hex outline.
/// Matches the wireframe hexagon ([`crate::hex_corners`]) exactly.
fn cell_local(i: usize, j: usize) -> (f32, f32) {
    let x = (i as f32 / (G - 1) as f32 * 2.0 - 1.0) * HEX_SIZE;
    let wf = band_width(x / HEX_SIZE);
    let z = (j as f32 / (G - 1) as f32 * 2.0 - 1.0) * APOTHEM * wf;
    (x, z)
}

#[inline]
fn idx(i: usize, j: usize) -> usize {
    j * G + i
}

/// Hex corner `k` in the local XZ plane, as a [`Vec2`].
fn corner_vec(k: usize) -> Vec2 {
    let a = k as f32 * std::f32::consts::FRAC_PI_3;
    Vec2::new(HEX_SIZE * a.cos(), HEX_SIZE * a.sin())
}

/// Blend a per-hex scalar across the hex from its six edge-neighbour values:
/// barycentric over the (centre, corner `c`, corner `c+1`) fan, where each
/// corner is the average of the centre and the two neighbours meeting there.
/// Because a shared corner averages the same three hexes from either side,
/// adjacent hexes agree along their shared edge — the field is seam-free
/// across boundaries.
fn blend_scalar(local: Vec2, center: f32, nbr_edge: &[Option<f32>; 6]) -> f32 {
    let corner_val = |c: usize| -> f32 {
        let (mut sum, mut n) = (center, 1.0f32);
        if let Some(v) = nbr_edge[(c + 5) % 6] {
            sum += v;
            n += 1.0;
        }
        if let Some(v) = nbr_edge[c % 6] {
            sum += v;
            n += 1.0;
        }
        sum / n
    };
    for c in 0..6 {
        let a = corner_vec(c);
        let b = corner_vec((c + 1) % 6);
        let det = a.x * b.y - a.y * b.x;
        if det.abs() < 1e-6 {
            continue;
        }
        let wa = (local.x * b.y - local.y * b.x) / det;
        let wb = (a.x * local.y - a.y * local.x) / det;
        let wc = 1.0 - wa - wb;
        if wa >= -1e-3 && wb >= -1e-3 && wc >= -1e-3 {
            return wc * center + wa * corner_val(c) + wb * corner_val((c + 1) % 6);
        }
    }
    center
}

/// Build a hex sheet mesh from per-cell closures. `height` is added to
/// `place_y`; a quad is emitted only when all four corners are `present`
/// (sparse layers — the sea — render just their footprint).
fn build_sheet<H, M, P>(place_y: f32, height: H, material: M, present: P) -> (Vec<MeshVertex>, Vec<u32>)
where
    H: Fn(usize, usize) -> f32,
    M: Fn(usize, usize) -> u32,
    P: Fn(usize, usize) -> bool,
{
    let mut pos = Vec::with_capacity(G * G);
    for j in 0..G {
        for i in 0..G {
            let (x, z) = cell_local(i, j);
            pos.push(Vec3::new(x, place_y + height(i, j), z));
        }
    }
    let at = |i: usize, j: usize| pos[idx(i, j)];

    let mut vertices = Vec::with_capacity(G * G);
    for j in 0..G {
        for i in 0..G {
            let du = at((i + 1).min(G - 1), j) - at(i.saturating_sub(1), j);
            let dv = at(i, (j + 1).min(G - 1)) - at(i, j.saturating_sub(1));
            let mut n = du.cross(dv).normalize_or(Vec3::Y);
            if n.y < 0.0 {
                n = -n;
            }
            vertices.push(MeshVertex {
                position: at(i, j).to_array(),
                normal: n.to_array(),
                material: material(i, j),
            });
        }
    }

    let mut indices = Vec::new();
    let id = |i: usize, j: usize| idx(i, j) as u32;
    for j in 0..G - 1 {
        for i in 0..G - 1 {
            if present(i, j) && present(i + 1, j) && present(i, j + 1) && present(i + 1, j + 1) {
                let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i, j + 1), id(i + 1, j + 1));
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }

    (vertices, indices)
}

/// Hex-grid ring of a logical offset from its map centre: solve the offset in
/// the axial basis (`edge_normal(0)`, `edge_normal(1)`) and take the hex
/// distance. Exact for every ring (unlike rounding the Euclidean radius, which
/// misreads the mid-side cells of rings 4+).
fn ring_of(off: Vec3) -> usize {
    let u = off.x / (0.866_025_4 * HEX_SPACING);
    let v = off.z / HEX_SPACING - 0.5 * u;
    let (u, v) = (u.round(), v.round());
    ((u.abs() + v.abs() + (u + v).abs()) * 0.5) as usize
}

/// Stub celestial direction per tile — its theoretical unit point on the
/// sphere, read off the dome: ring `k` of a `rings`-ring map sits `k·(90°/
/// rings)` down from the pole, azimuth from the tile's logical offset. The
/// right map is the northern hemisphere, the left the southern; both equator
/// rings land at latitude 0, where the fold joins them.
fn celestial_dirs(hexes: &[HexInst], rings: usize) -> Vec<Vec3> {
    hexes
        .iter()
        .map(|h| {
            let centre = if h.left { left_center() } else { Vec3::ZERO };
            let off = h.logical - centre;
            let k = ring_of(off);
            let theta = ring_dome_angle(k, rings);
            let phi = if k == 0 { 0.0 } else { off.z.atan2(off.x) };
            let y = if h.left { -theta.cos() } else { theta.cos() };
            Vec3::new(theta.sin() * phi.cos(), y, theta.sin() * phi.sin())
        })
        .collect()
}

/// World centre of a tile under the given map transforms.
fn tile_center(inst: &HexInst, m_right: &Mat4, m_left: &Mat4) -> Vec3 {
    if inst.left {
        m_left.transform_point3(inst.center)
    } else {
        m_right.transform_point3(inst.center)
    }
}

/// Across-equator neighbours per tile: for each tile short of six same-map
/// neighbours, the nearest equator tiles of the *other* map, measured at the
/// supplied (rest-pose) transforms — the static mirror of
/// `HexScene::neighbors_of`, so the sim's fold matches the highlight's.
fn equator_fill(
    hexes: &[HexInst],
    within: &[Vec<u32>],
    m_right: &Mat4,
    m_left: &Mat4,
) -> Vec<Vec<u32>> {
    hexes
        .iter()
        .map(|a| {
            let need = 6usize.saturating_sub(within[a.number as usize].len());
            if need == 0 {
                return Vec::new();
            }
            let origin = tile_center(a, m_right, m_left);
            let mut cands: Vec<(f32, u32)> = hexes
                .iter()
                .filter(|b| b.left != a.left && within[b.number as usize].len() < 6)
                .map(|b| ((tile_center(b, m_right, m_left) - origin).length(), b.number))
                .collect();
            cands.sort_by(|x, y| x.0.total_cmp(&y.0));
            cands.into_iter().take(need).map(|(_, n)| n).collect()
        })
        .collect()
}

/// The adjacency the epochs run on: same-map neighbours plus the equator fill,
/// **symmetrized** (nearest-fill pairing isn't inherently mutual, and the
/// plate/erosion passes want an undirected graph — if A pulls from B, B pushes
/// to A).
fn world_neighbors(within: &[Vec<u32>], fill: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let mut out: Vec<Vec<u32>> = within
        .iter()
        .zip(fill)
        .map(|(w, f)| {
            let mut v: Vec<u32> = w.iter().chain(f).copied().collect();
            v.sort_unstable();
            v.dedup();
            v
        })
        .collect();
    for i in 0..out.len() {
        for j in out[i].clone() {
            if !out[j as usize].contains(&(i as u32)) {
                out[j as usize].push(i as u32);
            }
        }
    }
    out
}

/// A tile's (≤6) neighbours indexed by hex edge, for the tectonic blend:
/// same-map neighbours land on the edge their logical offset points along;
/// across-equator neighbours fill the leftover perimeter edges — so the data
/// blend follows the wrapped topology even though the render is two maps.
fn edge_slots(inst: &HexInst, hexes: &[HexInst], within: &[u32], fill: &[u32]) -> [Option<u32>; 6] {
    let mut slots: [Option<u32>; 6] = [None; 6];
    for &n in within {
        let dir = (hexes[n as usize].logical - inst.logical).normalize_or_zero();
        let e = (0..6)
            .filter(|&e| slots[e].is_none())
            .max_by(|&a, &b| dir.dot(edge_normal(a)).total_cmp(&dir.dot(edge_normal(b))));
        if let Some(e) = e {
            slots[e] = Some(n);
        }
    }
    for &n in fill {
        if let Some(e) = slots.iter().position(Option::is_none) {
            slots[e] = Some(n);
        }
    }
    slots
}

/// Noise-sampling position of a tile: its logical centre, with the left map
/// shifted into its own stretch of the world fields (its logical frame
/// overlaps the right map's).
fn sample_place(inst: &HexInst) -> Vec2 {
    let shift = if inst.left { LEFT_SAMPLE_SHIFT } else { 0.0 };
    Vec2::new(inst.logical.x, inst.logical.z + shift)
}

/// One tile's uploaded terrain: the consolidated Epoch-6 ground relief and, if
/// any cell sits below the sea level, a flat sea sheet over the basins.
pub struct TileTerrain {
    pub ground: MeshHandle,
    pub water: Option<MeshHandle>,
}

/// The generated world: the full six-epoch stack per tile (retained — the
/// generation procedures are still being tweaked), plus what the mesh build
/// needs to realize the top layer.
pub struct WorldGen {
    /// World seed the stack was generated from (also drives the field sampler).
    pub seed: u64,
    /// Six epoch layers per tile, Epoch 1 at index 0 … Epoch 6 (the ground) at
    /// `EPOCHS - 1`, indexed by tile number.
    pub epoch_states: Vec<Vec<HexState>>,
    /// Per-tile edge-indexed neighbours, for the cross-boundary tectonic blend.
    nbr_by_edge: Vec<[Option<u32>; 6]>,
}

impl WorldGen {
    /// Run the six-epoch stack for `seed` over the map's tiles. `m_right` /
    /// `m_left` must be the **rest-pose** map transforms so the across-equator
    /// adjacency (and with it the whole world) is independent of the wheels.
    pub fn generate(
        tables: &Tables,
        hexes: &[HexInst],
        within: &[Vec<u32>],
        rings: usize,
        seed: u64,
        m_right: &Mat4,
        m_left: &Mat4,
    ) -> Self {
        let dirs = celestial_dirs(hexes, rings);
        let fill = equator_fill(hexes, within, m_right, m_left);
        let neighbors = world_neighbors(within, &fill);
        let e1 = Epoch1::new(tables, Epoch1Params::default(), seed);
        let ctx = EpochCtx { tables, dirs: &dirs, neighbors: &neighbors, seed };
        let stack = six_epoch_stack(&e1, &ctx);
        let epoch_states: Vec<Vec<HexState>> = (0..hexes.len())
            .map(|h| (0..EPOCHS).map(|e| stack[e][h].clone()).collect())
            .collect();
        let nbr_by_edge = hexes
            .iter()
            .map(|inst| {
                let n = inst.number as usize;
                edge_slots(inst, hexes, &within[n], &fill[n])
            })
            .collect();
        Self { seed, epoch_states, nbr_by_edge }
    }

    /// Build + upload each tile's top-layer meshes: the Epoch-6 ground relief
    /// (tectonic scalars blended across the edge-neighbours so terrain joins)
    /// and the sea sheet over its submerged cells. Record-flipped tiles read
    /// the mirrored column so the drawn terrain mirrors with the tile.
    pub fn upload_meshes(
        &self,
        tables: &Tables,
        hexes: &[HexInst],
        renderer: &mut Renderer,
    ) -> Vec<TileTerrain> {
        let sampler = FieldSampler::new(tables, self.seed);
        let last = EPOCHS - 1;
        let elev: Vec<f32> = self.epoch_states.iter().map(|s| s[last].elevation).collect();
        let orog: Vec<f32> = self.epoch_states.iter().map(|s| s[last].orogeny).collect();
        let edge_vals = |field: &[f32], slots: &[Option<u32>; 6]| -> [Option<f32>; 6] {
            let mut out = [None; 6];
            for (k, slot) in slots.iter().enumerate() {
                out[k] = slot.map(|nb| field[nb as usize]);
            }
            out
        };

        hexes
            .iter()
            .enumerate()
            .map(|(h, inst)| {
                let state = &self.epoch_states[h][last];
                let slots = &self.nbr_by_edge[h];
                let (ne, no) = (edge_vals(&elev, slots), edge_vals(&orog, slots));
                let place = sample_place(inst);
                let cells: Vec<CellSample> = (0..G * G)
                    .map(|k| {
                        let (lx, lz) = cell_local(k % G, k / G);
                        let local = Vec2::new(lx, lz);
                        let be = blend_scalar(local, elev[h], &ne);
                        let bo = blend_scalar(local, orog[h], &no);
                        sampler.sample_blended(state, place + local, be, bo)
                    })
                    .collect();
                // Record-flipped tiles read the mirrored column (the grid is
                // x-symmetric), so the mesh mirrors without re-winding.
                let at = |i: usize, j: usize| -> &CellSample {
                    let ii = if inst.flip { G - 1 - i } else { i };
                    &cells[idx(ii, j)]
                };
                let sea = state.sea_level * sampler.tectonic_scale;
                let (gv, gi) = build_sheet(
                    0.0,
                    |i, j| at(i, j).elevation * TERRAIN_VSCALE,
                    |i, j| ground_material(at(i, j), state.biome, sea),
                    |_, _| true,
                );
                let ground = renderer.upload_mesh(&gv, MeshIndices::U32(&gi));
                let (wv, wi) = build_sheet(
                    sea * TERRAIN_VSCALE,
                    |_, _| 0.0,
                    |_, _| M_WATER_MID,
                    |i, j| at(i, j).elevation < sea,
                );
                let water = (!wi.is_empty()).then(|| renderer.upload_mesh(&wv, MeshIndices::U32(&wi)));
                TileTerrain { ground, water }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_hex_instances, build_within_neighbors, ring_offsets, HexScene};

    fn world(rings: usize) -> (Vec<HexInst>, Vec<Vec<u32>>, WorldGen) {
        let tables = load_tables().expect("repo data/materials loads");
        let hexes = build_hex_instances(rings);
        let within = build_within_neighbors(&hexes);
        let (mr, ml) = HexScene::map_transforms_for(rings, 0.0, std::f32::consts::PI);
        let w = WorldGen::generate(&tables, &hexes, &within, rings, 0x0EC0_DE01, &mr, &ml);
        (hexes, within, w)
    }

    #[test]
    fn ring_of_matches_walked_rings() {
        // The axial-distance ring read must agree with the ring each offset was
        // walked on — including the mid-side cells of rings 4+ whose Euclidean
        // radius rounds to the wrong ring.
        for k in 1..=5 {
            for off in ring_offsets(k) {
                assert_eq!(ring_of(off), k, "offset {off:?}");
            }
        }
        assert_eq!(ring_of(Vec3::ZERO), 0);
    }

    #[test]
    fn six_epoch_layers_retained_per_tile() {
        let (hexes, _, w) = world(2);
        assert_eq!(w.epoch_states.len(), hexes.len());
        for s in &w.epoch_states {
            assert_eq!(s.len(), EPOCHS);
        }
        // The chain actually ran: tectonics wrote elevation, the hydrosphere a
        // sea level, and the final layer classifies biomes beyond bare ocean.
        assert!(w.epoch_states.iter().any(|s| s[EPOCHS - 1].elevation != 0.0));
        assert!(w.epoch_states.iter().all(|s| s[EPOCHS - 1].sea_level != 0.0));
        assert!(w.epoch_states.iter().any(|s| s[EPOCHS - 1].biome != Biome::Ocean));
    }

    #[test]
    fn celestial_dirs_split_the_hemispheres() {
        let rings = 3;
        let hexes = build_hex_instances(rings);
        let within = build_within_neighbors(&hexes);
        let dirs = celestial_dirs(&hexes, rings);
        for (h, d) in hexes.iter().zip(&dirs) {
            let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "tile {} not unit", h.number);
            if within[h.number as usize].len() < 6 {
                // Equator tiles of both maps meet at latitude 0.
                assert!(d.y.abs() < 1e-3, "equator tile {} at y {}", h.number, d.y);
            } else if h.left {
                assert!(d.y < 0.0, "left tile {} not southern", h.number);
            } else {
                assert!(d.y > 0.0, "right tile {} not northern", h.number);
            }
        }
        // The two poles: tile 0 (right centre) and the last tile (left centre).
        assert!((dirs[0].y - 1.0).abs() < 1e-4);
        assert!((dirs.last().unwrap().y + 1.0).abs() < 1e-4);
    }

    #[test]
    fn epoch_adjacency_is_symmetric_and_clean() {
        let rings = 2;
        let hexes = build_hex_instances(rings);
        let within = build_within_neighbors(&hexes);
        let (mr, ml) = HexScene::map_transforms_for(rings, 0.0, std::f32::consts::PI);
        let fill = equator_fill(&hexes, &within, &mr, &ml);
        let nbrs = world_neighbors(&within, &fill);
        for (i, ns) in nbrs.iter().enumerate() {
            // Interior tiles keep exactly their six same-map neighbours.
            if within[i].len() == 6 {
                assert_eq!(ns.len(), 6, "interior tile {i}");
            }
            for (k, &j) in ns.iter().enumerate() {
                assert_ne!(j as usize, i, "self-ref at {i}");
                assert!(!ns[..k].contains(&j), "dup {j} at {i}");
                assert!(nbrs[j as usize].contains(&(i as u32)), "{i}->{j} not mutual");
            }
        }
    }
}
