//! HexWorld — the world-data **stack visualization** (a data view, not gameplay).
//!
//! Each hex is drawn as a vertical stack at **real scale** — one world unit =
//! one cluster = 128 ft, so a hex is 2048 units (≈49.6 mi) across and the column
//! is 256 units (≈6.2 mi) tall:
//!
//! - **Six epoch planes** at the bottom — the world-gen epochs, each a per-cell
//!   **relief mesh** sampled from that epoch's `HexState` field (the spatial
//!   hardness distribution → terrain: hard rock ridges, soft valleys), tinted by
//!   its dominant element. Epoch 1 composition, Epoch 2 differentiated crust,
//!   Epoch 3 plate-driven elevation, Epoch 4 floods the basins with oceans;
//!   Epochs 5-6 copy Epoch 4.
//! - **Nine surface-simulation bands** stacked on top — the existing band model
//!   drawn as colour-coded **empty shells**. The water-cycle sim still ticks
//!   underneath (its mechanics — heat, convection, precipitation — are kept); we
//!   simply no longer draw water/lava/ice as heightmaps, nor any band content.
//!   The sim's ground reads Epoch 6.
//!
//! Fly: WASD move, R/F up/down, hold right mouse to look, Esc quits.

mod layers;
#[allow(dead_code)] // adjacency/celestial kept; only layout + celestial_dir used here.
mod topology;

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, SceneLighting,
    TextureHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::{
    six_epoch_stack, Biome, CellSample, Epoch1, Epoch1Params, EpochCtx, FieldSampler, HexState,
    EPOCHS,
};
use flicker_worldstate::Composition;

use layers::{build_sheet, cell_local, pack_ramp, LayerStack, BANDS, BAND_BOUNDS, G, HEX_HALF_W, HEX_SIZE};
use topology::{HexCoord, HexMap, Hemisphere};

/// Rings per hemisphere at startup — the minimum (R=3 → 74 hexes), the smallest
/// test world.
const RINGS: u32 = 3;
/// Largest map the size slider allows: the minimum + 2 rings (R=5 → 182 hexes).
const MAX_RINGS: u32 = RINGS + 2;
/// World seed for the Epoch 1 element distribution.
const SEED: u64 = 0x0EC0_DE01;
/// Materials vocabulary directory (the JSON tables), relative to this example.
const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");

/// Fixed sim step; the kept water-cycle mechanics still tick (undrawn).
const SIM_DT: f32 = 0.05;
/// Ticks run on every hex at startup so the kept sim settles into a real state.
const SETTLE_TICKS: u32 = 50;
/// Hexes within this many rings of the north pole keep ticking after settle.
const ANIMATE_RINGS: u32 = 1;

/// Vertical exaggeration of the stack. `1.0` is true scale (a 256-unit column
/// under a 2048-wide hex — a thin slab); raise it to read the bands while flying.
const VEXAG: f32 = 1.0;
/// Vertical gap between the **exploded** epoch planes, world units. Deliberately
/// an order of magnitude larger than a band's real height (~28 u) so the six
/// epoch layers are easy to see and compare as their transforms diverge them.
/// Only the epoch stack is exploded — the sim bands stay at true altitude.
const EPOCH_GAP: f32 = 640.0;
/// Translucency of the (empty) band shells.
const BAND_ALPHA: f32 = 0.22;
/// Draw the 9 atmosphere band shells above the stack. Hidden while inspecting the
/// epoch data layout — the meshes are still built, so flip this to bring them back.
const SHOW_BANDS: bool = false;
/// Show floating hex-index labels over each hex (a debug aid for the layout).
const SHOW_HEX_LABELS: bool = true;
/// Height (world units) of the floating hex-index labels, above the stack.
const LABEL_Y: f32 = 350.0;

// Material index per band shell (bottom→top): the three zones — below (molten /
// rock / soil), terrain (lowland / hill / alpine), atmosphere (lower air / cloud
// deck / thin air). Colour only; the bands are drawn empty.
const BAND_MAT: [u32; BANDS] = [
    layers::M_LAVA,
    layers::M_STONE,
    layers::M_LAND,
    layers::M_GRASSLAND,
    layers::M_FOREST,
    layers::M_TUNDRA,
    layers::M_WATER_SHALLOW,
    layers::M_UV,
    layers::M_AURORA,
];

/// Default fly-camera pose, retuned for real scale (the R=3 world spans ~±10k
/// units): position, yaw, pitch.
const CAM_HOME: (Vec3, f32, f32) = (Vec3::new(0.0, 9000.0, -17000.0), 0.0, -0.45);
const MOVE_SPEED: f32 = 11000.0;
const LOOK_SENS: f32 = 0.005;

/// Flat XZ offset of `pos` within `ring` (flat-top axial basis: NE and N).
fn ring_offset(ring: u32, pos: u32) -> Vec2 {
    if ring == 0 {
        return Vec2::ZERO;
    }
    const DIRS: [(i32, i32); 6] = [(1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1), (1, -1)];
    let (mut q, mut r) = (DIRS[4].0 * ring as i32, DIRS[4].1 * ring as i32);
    let mut cells = Vec::with_capacity(6 * ring as usize);
    for &(dq, dr) in &DIRS {
        for _ in 0..ring {
            cells.push((q, r));
            q += dq;
            r += dr;
        }
    }
    let (cq, cr) = cells[pos as usize % cells.len()];
    let aq = Vec2::new(1.5 * HEX_SIZE, HEX_HALF_W);
    let ar = Vec2::new(0.0, 2.0 * HEX_HALF_W);
    aq * cq as f32 + ar * cr as f32
}

/// Flat-layout centre of a hemisphere's disc — the two discs drawn side by side,
/// spaced so an `rings`-radius disc clears the gap.
fn cluster_center(hemi: Hemisphere, rings: u32) -> Vec2 {
    let sep = 1.5 * HEX_SIZE * (rings as f32 + 0.5);
    match hemi {
        Hemisphere::North => Vec2::new(-sep, 0.0),
        Hemisphere::South => Vec2::new(sep, HEX_HALF_W),
    }
}

/// Rotate `v` by `k` sixths of a turn (`k·60°`) — a symmetry of the hex lattice,
/// so it re-embeds a disc without breaking tessellation.
fn rotate60(v: Vec2, k: u32) -> Vec2 {
    let (s, c) = (k as f32 * std::f32::consts::FRAC_PI_3).sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn hex_flat_pos(coord: HexCoord, rings: u32) -> Vec2 {
    let mut off = ring_offset(coord.ring, coord.pos);
    // Lay the two discs out as a **screw**, not a mirror: the array spirals out
    // from the north pole to the equator (…→36), then continues spiralling *in*
    // to the south pole (37→…). The south disc is the north disc **rotated 180°**
    // (`rot 5` = `rot 2` + 3), *never reflected* — so both halves wind the same
    // handedness (a continuous coil) instead of flipping into a mirror image. The
    // north rotation brings its equator end (36) to the seam; the south rotation
    // brings its equator start (37) to meet it. Rotations are 60° hex-lattice
    // symmetries, so each disc stays a valid hexagon-of-hexagons.
    match coord.hemi {
        Hemisphere::North => off = rotate60(off, 2),
        Hemisphere::South => off = rotate60(off, 5),
    }
    cluster_center(coord.hemi, rings) + off
}

/// XZ of flat-top hex corner `k`, centred at the origin.
fn hex_corner(k: usize) -> (f32, f32) {
    let a = k as f32 * std::f32::consts::FRAC_PI_3;
    (HEX_SIZE * a.cos(), HEX_SIZE * a.sin())
}

/// Hex corner `k` as a [`Vec2`].
fn hex_corner_vec(k: usize) -> Vec2 {
    let (x, z) = hex_corner(k);
    Vec2::new(x, z)
}

/// Outward unit normal of flat-top hex edge `k` (the edge between corner `k` and
/// corner `k+1`), centred at the origin.
fn edge_normal(k: usize) -> Vec2 {
    let a = (k as f32 + 0.5) * std::f32::consts::FRAC_PI_3;
    Vec2::new(a.cos(), a.sin())
}

/// Slot a neighbour into the best-aligned **free** hex edge for direction `dir`
/// (the screen-space offset to that neighbour). Free-edge fallback keeps the
/// assignment collision-safe even if two neighbours favour the same edge.
fn assign_to_edge(slots: &mut [Option<u32>; 6], dir: Vec2, nbr: u32) {
    let d = dir.normalize_or_zero();
    let mut order: [usize; 6] = [0, 1, 2, 3, 4, 5];
    order.sort_by(|&x, &y| {
        d.dot(edge_normal(y))
            .partial_cmp(&d.dot(edge_normal(x)))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for k in order {
        if slots[k].is_none() {
            slots[k] = Some(nbr);
            return;
        }
    }
}

/// This hex's logical neighbours indexed by hex edge. Same-hemisphere neighbours
/// land on the edge they sit across in the flat layout; equator-fold neighbours
/// (drawn on the far disc) fill the leftover perimeter edges — so the *data*
/// blend follows the true wrapped topology even though the render is two discs.
fn neighbor_edges(map: &HexMap, i: u32, rings: u32) -> [Option<u32>; 6] {
    let self_coord = map.coord(i);
    let self_pos = hex_flat_pos(self_coord, rings);
    let mut slots: [Option<u32>; 6] = [None; 6];
    let mut fold: Vec<u32> = Vec::new();
    for nbr in map.neighbours(i) {
        let nc = map.coord(nbr);
        if nc.hemi == self_coord.hemi {
            assign_to_edge(&mut slots, hex_flat_pos(nc, rings) - self_pos, nbr);
        } else {
            fold.push(nbr);
        }
    }
    for nbr in fold {
        if let Some(k) = slots.iter().position(Option::is_none) {
            slots[k] = Some(nbr);
        }
    }
    slots
}

/// Blend a per-hex scalar across the hex from its six edge-neighbour values:
/// barycentric over the (centre, corner `c`, corner `c+1`) fan, where each corner
/// is the average of the centre and the two neighbours meeting there. Because a
/// shared corner averages the same three hexes from either side, adjacent hexes
/// agree along their shared edge — the field is **seam-free** across boundaries.
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
        let a = hex_corner_vec(c);
        let b = hex_corner_vec((c + 1) % 6);
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

/// The 6 side faces of an empty hexagonal-prism band `[y0, y1]` at the origin,
/// material `mat`. Double-sided so it reads from inside and out.
fn band_shell(y0: f32, y1: f32, mat: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(24);
    let mut inds = Vec::with_capacity(72);
    for k in 0..6 {
        let (x0, z0) = hex_corner(k);
        let (x1, z1) = hex_corner((k + 1) % 6);
        let (mx, mz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
        let nl = (mx * mx + mz * mz).sqrt().max(1e-6);
        let normal = [mx / nl, 0.0, mz / nl];
        let b = verts.len() as u32;
        for (x, y, z) in [(x0, y0, z0), (x1, y0, z1), (x1, y1, z1), (x0, y1, z0)] {
            verts.push(MeshVertex { position: [x, y, z], normal, material: mat });
        }
        inds.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]); // outward
        inds.extend_from_slice(&[b, b + 2, b + 1, b, b + 3, b + 2]); // inward
    }
    (verts, inds)
}

/// Vein intensity above which a cell is drawn as ore rather than rock.
const VEIN_SHOW: f32 = 0.08;

/// The ore-vein palette colour for the metal a vein carries.
fn ore_material(vein_element: Option<u8>) -> u32 {
    match vein_element {
        Some(26) => layers::M_ORE_IRON,   // Fe
        Some(29) => layers::M_ORE_COPPER, // Cu
        Some(79) => layers::M_ORE_GOLD,   // Au
        Some(47) => layers::M_ORE_SILVER, // Ag
        _ => layers::M_ORE_OTHER,
    }
}

/// Per-cell material word for a relief mesh. Bare rock is shaded by **hardness**
/// (soft/dark → hard/pale) — so the foliation bands and composition structure
/// read as colour instead of one flat per-hex tint — and a cell on an ore
/// **vein** is tinted toward its metal's colour by the vein intensity, so you
/// can see where the veins run. Packed as a two-stop ramp the mesh shader
/// resolves with no shader change.
fn cell_material(c: &CellSample, vein_element: Option<u8>) -> u32 {
    if c.vein > VEIN_SHOW {
        pack_ramp(layers::M_ROCK_HARD, ore_material(vein_element), c.vein.clamp(0.0, 1.0))
    } else {
        let hard = (c.hardness / 10.0).clamp(0.0, 1.0);
        pack_ramp(layers::M_ROCK_SOFT, layers::M_ROCK_HARD, hard)
    }
}

/// World-unit span over which the Epoch-6 ground fades from its biome colour up
/// to bare rock / snow on the peaks.
const SNOW_SPAN: f32 = 150.0;

/// The biome's surface palette colour (Epoch 6).
fn biome_material(b: Biome) -> u32 {
    match b {
        Biome::Ocean => layers::M_WATER_MID, // (the sea overlay covers it anyway)
        Biome::Ice => layers::M_ICE,
        Biome::Tundra => layers::M_TUNDRA,
        Biome::Taiga => layers::M_TAIGA,
        Biome::Grassland => layers::M_GRASSLAND,
        Biome::Forest => layers::M_FOREST,
        Biome::Rainforest => layers::M_RAINFOREST,
        Biome::Savanna => layers::M_SAVANNA,
        Biome::Desert => layers::M_DESERT,
        Biome::Alpine => layers::M_ROCK_HARD,
    }
}

/// Ground (Epoch 6) per-cell colour: the hex's biome at the lowlands, fading to
/// bare rock / snow on the cells that rise toward the peaks.
fn ground_material(c: &CellSample, biome: Biome, sea_world: f32) -> u32 {
    let snow_t = ((c.elevation - sea_world) / SNOW_SPAN).clamp(0.0, 1.0);
    pack_ramp(biome_material(biome), layers::M_ROCK_HARD, snow_t)
}

/// Run the full six-epoch world-gen stack for one seed over the hex map — the
/// per-hex direction/neighbour graph drives Epochs 1–6 exactly as it does at
/// startup, so changing the seed yields a different world.
fn build_stack(map: &HexMap, tables: &Tables, seed: u64) -> Vec<Vec<HexState>> {
    let e1 = Epoch1::new(tables, Epoch1Params::default(), seed);
    let dirs: Vec<Vec3> = (0..map.total())
        .map(|i| {
            let d = map.celestial_dir(i);
            Vec3::new(d[0], d[1], d[2])
        })
        .collect();
    let neighbors: Vec<Vec<u32>> = (0..map.total()).map(|i| map.neighbours(i)).collect();
    let ctx = EpochCtx { tables, dirs: &dirs, neighbors: &neighbors, seed };
    six_epoch_stack(&e1, &ctx)
}

/// Build every tile for a map: its flat position, kept water-cycle sim, six
/// epoch states (from the stack for `seed`), and edge-indexed neighbours — plus
/// the `animated` mask. Meshes are uploaded separately (needs a renderer). Used
/// by both startup and the map-size slider rebuild.
fn build_tiles(map: &HexMap, tables: Option<&Tables>, seed: u64, rings: u32) -> (Vec<Tile>, Vec<bool>) {
    let n = map.total() as usize;
    let stack = match tables {
        Some(t) => build_stack(map, t, seed),
        None => {
            tracing::error!("vocabulary failed to load; the stack will be empty");
            vec![vec![HexState::new(Composition::new()); n]; EPOCHS]
        }
    };
    let mut tiles = Vec::with_capacity(n);
    let mut animated = Vec::with_capacity(n);
    for i in 0..map.total() {
        let c = map.coord(i);
        let place = hex_flat_pos(c, rings);
        let epoch_states = (0..EPOCHS).map(|e| stack[e][i as usize].clone()).collect();
        tiles.push(Tile {
            place,
            stack: LayerStack::generate(place),
            epoch_states,
            relief: Vec::new(),
            water: None,
            nbr_by_edge: neighbor_edges(map, i, rings),
        });
        animated.push(c.hemi == Hemisphere::North && c.ring <= ANIMATE_RINGS);
    }
    (tiles, animated)
}

/// splitmix64 step — jump to a fresh, unrelated-looking seed (the "new seed"
/// button). Deterministic, so a given world is always reproducible.
fn next_seed(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// Generation-control overlay layout (screen pixels, top-left origin). A seed
// readout + three buttons (fresh seed, ±1), then a map-size (rings) slider.
const UI_PANEL: (f32, f32, f32, f32) = (8.0, 8.0, 260.0, 140.0);
const BTN_NEW: (f32, f32, f32, f32) = (16.0, 54.0, 150.0, 28.0);
const BTN_DEC: (f32, f32, f32, f32) = (172.0, 54.0, 36.0, 28.0);
const BTN_INC: (f32, f32, f32, f32) = (214.0, 54.0, 36.0, 28.0);
// Rings slider: a track from `X0`..`X1` at `Y`, with a generous grab area.
const SLIDER_X0: f32 = 16.0;
const SLIDER_X1: f32 = 196.0;
const SLIDER_Y: f32 = 120.0;
const SLIDER_HIT: (f32, f32, f32, f32) = (10.0, 110.0, 200.0, 24.0);

/// Is `m` inside rect `(x, y, w, h)`?
fn hit(r: (f32, f32, f32, f32), m: Vec2) -> bool {
    m.x >= r.0 && m.x <= r.0 + r.2 && m.y >= r.1 && m.y <= r.1 + r.3
}

/// Draw a labelled overlay button, highlighted when the pointer is over it.
fn ui_button(renderer: &mut Renderer, white: TextureHandle, r: (f32, f32, f32, f32), label: &str, hover: bool) {
    let bg = if hover {
        [0.28, 0.34, 0.46, 0.95]
    } else {
        [0.16, 0.18, 0.24, 0.95]
    };
    renderer.draw_sprite(white, Vec2::new(r.0, r.1), Vec2::new(r.2, r.3), bg);
    let tw = renderer.measure_text(label, 14.0).x;
    renderer.draw_text(label, Vec2::new(r.0 + (r.2 - tw) * 0.5, r.1 + 7.0), 14.0, [0.95, 0.96, 1.0, 1.0]);
}

/// One hex: its flat-layout position, its kept (undrawn) water-cycle sim, the six
/// epoch states (Epoch 1 first, Epoch 6 last), and one per-cell relief mesh per
/// epoch sampled from that epoch's state (so Epoch 5's ore veins are drawn).
struct Tile {
    place: Vec2,
    stack: LayerStack,
    epoch_states: Vec<HexState>,
    relief: Vec<MeshHandle>,
    /// Flat sea surface over the submerged cells (Epoch 4+); `None` if dry.
    water: Option<MeshHandle>,
    /// This hex's six logical neighbours indexed by hex edge (`None` where an
    /// edge has no neighbour — the equator-corner defect). Used to blend the
    /// per-hex tectonic state across boundaries so terrain joins seamlessly.
    nbr_by_edge: [Option<u32>; 6],
}

struct World {
    tiles: Vec<Tile>,
    /// Per-tile: does this hex keep ticking after the startup settle?
    animated: Vec<bool>,
    /// The 9 band shells, at their real altitudes, coloured per zone.
    band_shells: Vec<MeshHandle>,
    /// Vocabulary — kept for the life of the scene so the world can be
    /// regenerated on a new seed.
    tables: Option<Tables>,
    /// The hex topology, kept to rebuild the epoch stack on a new seed/size.
    map: HexMap,
    /// World seed driving the whole epoch stack; the overlay edits it.
    seed: u64,
    /// Rings per hemisphere — the map size, driven by the overlay slider.
    rings: u32,
    /// A seed change requested by the overlay this frame, applied in `render`
    /// (where the `&mut Renderer` needed to re-upload meshes is available).
    pending_seed: Option<u64>,
    /// A ring-count (map-size) change requested by the slider, applied in `render`.
    pending_rings: Option<u32>,
    /// Whether the map-size slider is currently being dragged.
    slider_drag: bool,
    /// 1×1 white texture for the overlay panel/buttons.
    white: Option<TextureHandle>,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    prev_mouse: Vec2,
    dragging: bool,
    sim_accum: f32,
}

impl World {
    fn new() -> Self {
        let map = HexMap::new(RINGS);
        let tables = Tables::from_source(&JsonTableSource::new(MATERIALS_DIR)).ok();
        // The per-cell relief/water meshes are uploaded later in `enter` (where
        // the renderer is available); here we build the per-hex data.
        let (tiles, animated) = build_tiles(&map, tables.as_ref(), SEED, RINGS);

        Self {
            tiles,
            animated,
            band_shells: Vec::new(),
            tables,
            map,
            seed: SEED,
            rings: RINGS,
            pending_seed: None,
            pending_rings: None,
            slider_drag: false,
            white: None,
            pos: CAM_HOME.0,
            yaw: CAM_HOME.1,
            pitch: CAM_HOME.2,
            prev_mouse: Vec2::ZERO,
            dragging: false,
            sim_accum: 0.0,
        }
    }

    /// Rebuild the whole world for a new ring count: a fresh `HexMap`, all tiles
    /// (the hex count changes), and all meshes. Keeps the current seed + camera.
    fn set_rings(&mut self, renderer: &mut Renderer, rings: u32) {
        let rings = rings.clamp(RINGS, MAX_RINGS);
        // Free the old meshes before the tiles holding their handles are dropped.
        for t in self.tiles.iter_mut() {
            for m in t.relief.drain(..) {
                renderer.free_mesh(m);
            }
            if let Some(w) = t.water.take() {
                renderer.free_mesh(w);
            }
        }
        self.map = HexMap::new(rings);
        self.rings = rings;
        let (tiles, animated) = build_tiles(&self.map, self.tables.as_ref(), self.seed, rings);
        self.tiles = tiles;
        self.animated = animated;
        if let Some(tables) = self.tables.as_ref() {
            Self::rebuild_meshes(&mut self.tiles, tables, renderer, self.seed);
        }
    }

    /// Rebuild every tile's per-cell relief + water meshes for `seed`, freeing
    /// the old ones first (so regeneration doesn't leak GPU buffers). Shared by
    /// the initial `enter` and each seed regeneration. The tectonic `elevation` /
    /// `orogeny` are blended across the six-neighbour graph so terrain joins.
    fn rebuild_meshes(tiles: &mut [Tile], tables: &Tables, renderer: &mut Renderer, seed: u64) {
        let sampler = FieldSampler::new(tables, seed);
        for t in tiles.iter_mut() {
            for m in t.relief.drain(..) {
                renderer.free_mesh(m);
            }
            if let Some(w) = t.water.take() {
                renderer.free_mesh(w);
            }
        }

        // Snapshot per-epoch tectonic scalars so the mutable tile loop can read
        // every hex's neighbours without aliasing.
        let n = tiles.len();
        let mut elev = vec![vec![0.0f32; n]; EPOCHS];
        let mut orog = vec![vec![0.0f32; n]; EPOCHS];
        for (h, t) in tiles.iter().enumerate() {
            for e in 0..EPOCHS {
                elev[e][h] = t.epoch_states[e].elevation;
                orog[e][h] = t.epoch_states[e].orogeny;
            }
        }
        let edge_vals = |field: &[f32], slots: &[Option<u32>; 6]| -> [Option<f32>; 6] {
            let mut out = [None; 6];
            for (k, slot) in slots.iter().enumerate() {
                out[k] = slot.map(|nb| field[nb as usize]);
            }
            out
        };

        for (h, tile) in tiles.iter_mut().enumerate() {
            let place = tile.place;
            let slots = tile.nbr_by_edge;
            let relief: Vec<MeshHandle> = (0..EPOCHS)
                .map(|e| {
                    let state = &tile.epoch_states[e];
                    let (ce, co) = (elev[e][h], orog[e][h]);
                    let (ne, no) = (edge_vals(&elev[e], &slots), edge_vals(&orog[e], &slots));
                    let cells: Vec<CellSample> = (0..G * G)
                        .map(|k| {
                            let (lx, lz) = cell_local(k % G, k / G);
                            let local = Vec2::new(lx, lz);
                            let be = blend_scalar(local, ce, &ne);
                            let bo = blend_scalar(local, co, &no);
                            sampler.sample_blended(state, place + local, be, bo)
                        })
                        .collect();
                    let is_ground = e == EPOCHS - 1;
                    let sea_world = state.sea_level * sampler.tectonic_scale;
                    let (v, idx) = build_sheet(
                        0.0,
                        |i, j| cells[j * G + i].elevation,
                        |i, j| {
                            let c = &cells[j * G + i];
                            if is_ground {
                                ground_material(c, state.biome, sea_world)
                            } else {
                                cell_material(c, state.vein_element)
                            }
                        },
                        |_, _| true,
                    );
                    renderer.upload_mesh(&v, MeshIndices::U32(&idx))
                })
                .collect();
            tile.relief = relief;

            let e4 = &tile.epoch_states[3];
            let (ce, co) = (elev[3][h], orog[3][h]);
            let (ne, no) = (edge_vals(&elev[3], &slots), edge_vals(&orog[3], &slots));
            let sea = e4.sea_level * sampler.tectonic_scale;
            let (wv, wi) = build_sheet(
                sea,
                |_, _| 0.0,
                |_, _| layers::M_WATER_MID,
                |i, j| {
                    let (lx, lz) = cell_local(i, j);
                    let local = Vec2::new(lx, lz);
                    let be = blend_scalar(local, ce, &ne);
                    let bo = blend_scalar(local, co, &no);
                    sampler.sample_blended(e4, place + local, be, bo).elevation < sea
                },
            );
            tile.water = (!wi.is_empty()).then(|| renderer.upload_mesh(&wv, MeshIndices::U32(&wi)));
        }
    }

    /// Regenerate the whole world for `seed`: re-run the epoch stack and rebuild
    /// all meshes. Called from `render` (which holds the `&mut Renderer`).
    fn regenerate(&mut self, renderer: &mut Renderer, seed: u64) {
        self.seed = seed;
        let Some(tables) = self.tables.as_ref() else { return };
        let stack = build_stack(&self.map, tables, seed);
        for (i, tile) in self.tiles.iter_mut().enumerate() {
            tile.epoch_states = (0..EPOCHS).map(|e| stack[e][i].clone()).collect();
        }
        Self::rebuild_meshes(&mut self.tiles, tables, renderer, seed);
    }

    /// The generation-control overlay: the seed readout + regenerate buttons,
    /// and the map-size (rings) slider.
    fn draw_overlay(&self, renderer: &mut Renderer) {
        let Some(white) = self.white else { return };
        let m = self.prev_mouse;
        // Draw the panel on a higher 2D layer than the hex labels so it covers
        // any number that falls behind it.
        renderer.set_layer(100.0);
        renderer.draw_sprite(
            white,
            Vec2::new(UI_PANEL.0, UI_PANEL.1),
            Vec2::new(UI_PANEL.2, UI_PANEL.3),
            [0.0, 0.0, 0.0, 0.55],
        );
        renderer.draw_text("world seed — click to regenerate", Vec2::new(16.0, 14.0), 13.0, [0.85, 0.87, 0.95, 1.0]);
        renderer.draw_text(&format!("{:#010x}", self.seed), Vec2::new(16.0, 32.0), 18.0, [0.55, 0.9, 1.0, 1.0]);
        ui_button(renderer, white, BTN_NEW, "new seed", hit(BTN_NEW, m));
        ui_button(renderer, white, BTN_DEC, "-1", hit(BTN_DEC, m));
        ui_button(renderer, white, BTN_INC, "+1", hit(BTN_INC, m));

        // Map-size slider: rings R..=MAX, the handle at the current value.
        renderer.draw_text(
            &format!("map size: {} rings ({} hexes)", self.rings, self.map.total()),
            Vec2::new(16.0, 98.0),
            13.0,
            [0.85, 0.87, 0.95, 1.0],
        );
        let span = (MAX_RINGS - RINGS).max(1) as f32;
        let t = (self.rings - RINGS) as f32 / span;
        let hx = SLIDER_X0 + t * (SLIDER_X1 - SLIDER_X0);
        // Track, then the draggable handle.
        renderer.draw_sprite(white, Vec2::new(SLIDER_X0, SLIDER_Y), Vec2::new(SLIDER_X1 - SLIDER_X0, 6.0), [0.30, 0.33, 0.42, 0.95]);
        let hot = self.slider_drag || hit(SLIDER_HIT, m);
        let knob = if hot { [0.65, 0.85, 1.0, 1.0] } else { [0.50, 0.62, 0.80, 1.0] };
        renderer.draw_sprite(white, Vec2::new(hx - 6.0, SLIDER_Y - 6.0), Vec2::new(12.0, 18.0), knob);
        renderer.draw_text(&format!("max {MAX_RINGS}"), Vec2::new(SLIDER_X1 + 8.0, SLIDER_Y - 4.0), 12.0, [0.6, 0.62, 0.7, 1.0]);
        renderer.set_layer(0.0);
    }

    fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, cy * cp)
    }

    fn camera(&self) -> Camera {
        Camera {
            position: self.pos,
            target: self.pos + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 300_000.0,
        }
    }

    /// Fly camera + the kept sim's tick (undrawn).
    fn step(&mut self, dt: f32, input: &InputState) {
        // Look: hold right mouse, drag to pitch/yaw.
        if input.mouse_right {
            if self.dragging {
                let d = input.mouse_position - self.prev_mouse;
                self.yaw -= d.x * LOOK_SENS;
                self.pitch = (self.pitch - d.y * LOOK_SENS).clamp(-1.5, 1.5);
            }
            self.dragging = true;
        } else {
            self.dragging = false;
        }
        self.prev_mouse = input.mouse_position;

        // Move: 6-DOF fly.
        let fwd = self.forward();
        let (sy, cy) = self.yaw.sin_cos();
        let right = Vec3::new(-cy, 0.0, sy);
        let speed = MOVE_SPEED * dt;
        let mut v = Vec3::ZERO;
        if input.key_down(Key::W) {
            v += fwd;
        }
        if input.key_down(Key::S) {
            v -= fwd;
        }
        if input.key_down(Key::D) {
            v += right;
        }
        if input.key_down(Key::A) {
            v -= right;
        }
        if input.key_down(Key::R) {
            v += Vec3::Y;
        }
        if input.key_down(Key::F) {
            v -= Vec3::Y;
        }
        self.pos += v * speed;

        // The kept water-cycle mechanics tick (undrawn) on the live ring.
        self.sim_accum += dt;
        let mut steps = 0;
        while self.sim_accum >= SIM_DT && steps < 4 {
            for (idx, tile) in self.tiles.iter_mut().enumerate() {
                if self.animated[idx] {
                    tile.stack.tick(SIM_DT);
                }
            }
            self.sim_accum -= SIM_DT;
            steps += 1;
        }
        if self.sim_accum > SIM_DT {
            self.sim_accum = 0.0;
        }
    }

    fn draw_stack(&self, renderer: &mut Renderer) {
        for tile in &self.tiles {
            if tile.relief.len() < EPOCHS {
                continue;
            }
            // Six exploded epoch planes below 0 — Epoch 1 lowest, Epoch 6 nearest
            // the bands. Each is that epoch's own per-cell relief mesh, coloured
            // per cell by hardness (soft→hard) with ore veins overlaid — no flat
            // per-hex tint, so the sub-hex structure and the veins read.
            for e in 0..EPOCHS {
                let mesh = tile.relief[e];
                let tint = [1.0, 1.0, 1.0, 1.0];
                let y = -((EPOCHS - e) as f32) * EPOCH_GAP;
                let model = Mat4::from_translation(Vec3::new(tile.place.x, y, tile.place.y));
                renderer.draw_mesh(mesh, model, MeshDrawOptions { wireframe: false, tint, ..Default::default() });
                // Epoch 4+ floods the basins with a flat sea over the relief.
                if e >= 3 {
                    if let Some(w) = tile.water {
                        renderer.draw_mesh(w, model, MeshDrawOptions { wireframe: false, tint: [1.0, 1.0, 1.0, 1.0], ..Default::default() });
                    }
                }
            }
            // Nine empty band shells at their real altitudes (0..Y_LAYERS) —
            // the atmosphere outlines, hidden for now while inspecting the layout.
            if SHOW_BANDS {
                let model = Mat4::from_translation(Vec3::new(tile.place.x, 0.0, tile.place.y));
                let shell_tint = [1.0, 1.0, 1.0, BAND_ALPHA];
                for &shell in &self.band_shells {
                    renderer.draw_mesh(shell, model, MeshDrawOptions { wireframe: false, tint: shell_tint, ..Default::default() });
                }
            }
        }
    }

    /// Floating hex-index labels: each hex's array index projected to the screen
    /// above its column — a debug aid for reading the data layout (which index
    /// sits where, north vs south disc, ring/pos order).
    fn draw_hex_labels(&self, renderer: &mut Renderer, camera: &Camera) {
        let size = renderer.size();
        let vp = camera.view_projection(size.x / size.y);
        for (i, tile) in self.tiles.iter().enumerate() {
            let world = Vec3::new(tile.place.x, LABEL_Y, tile.place.y);
            let Some(p) = project_to_screen(vp, world, size) else { continue };
            let dist = (world - self.pos).length().max(1.0);
            let px = (150_000.0 / dist).clamp(9.0, 28.0);
            let label = i.to_string();
            let tw = renderer.measure_text(&label, px).x;
            renderer.draw_text(&label, p - Vec2::new(tw * 0.5, 0.0), px, [1.0, 0.95, 0.35, 1.0]);
        }
    }
}

/// Project a world point to screen pixels; `None` if behind the camera.
fn project_to_screen(vp: Mat4, world: Vec3, size: Vec2) -> Option<Vec2> {
    let clip = vp * world.extend(1.0);
    if clip.w <= 0.001 {
        return None;
    }
    Some(Vec2::new(
        (clip.x / clip.w * 0.5 + 0.5) * size.x,
        (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * size.y,
    ))
}

impl Scene for World {
    fn enter(&mut self, renderer: &mut Renderer) {
        let live = self.animated.iter().filter(|&&a| a).count();
        println!(
            "HexWorld stack viz: {} hexes (R={RINGS}), real scale (1 unit = 1 cluster = 128 ft). \
             {live} sim hexes tick live (undrawn). Fly: WASD, R/F up/down, RMB look, Esc.",
            self.tiles.len()
        );
        // Settle the kept sim so its (undrawn) state is real.
        for _ in 0..SETTLE_TICKS {
            for tile in &mut self.tiles {
                tile.stack.tick(SIM_DT);
            }
        }
        // Nine empty band shells at their real altitudes.
        self.band_shells = (0..BANDS)
            .map(|b| {
                let y0 = BAND_BOUNDS[b] as f32 * VEXAG;
                let y1 = BAND_BOUNDS[b + 1] as f32 * VEXAG;
                let (v, i) = band_shell(y0, y1, BAND_MAT[b]);
                renderer.upload_mesh(&v, MeshIndices::U32(&i))
            })
            .collect();
        // A 1×1 white texture the seed-control overlay tints for its panel/buttons.
        self.white = Some(renderer.load_texture(&[255, 255, 255, 255], 1, 1));
        // Per-tile relief + water meshes for the current seed.
        if let Some(tables) = self.tables.as_ref() {
            Self::rebuild_meshes(&mut self.tiles, tables, renderer, self.seed);
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        // Overlay: left-click a seed button, or drag the map-size slider. Both
        // queue work applied in `render` (which holds the `&mut Renderer`).
        let m = input.mouse_position;
        if input.mouse_left_pressed {
            if hit(BTN_NEW, m) {
                self.pending_seed = Some(next_seed(self.seed));
            } else if hit(BTN_DEC, m) {
                self.pending_seed = Some(self.seed.wrapping_sub(1));
            } else if hit(BTN_INC, m) {
                self.pending_seed = Some(self.seed.wrapping_add(1));
            } else if hit(SLIDER_HIT, m) {
                self.slider_drag = true;
            }
        }
        if !input.mouse_left {
            self.slider_drag = false;
        }
        if self.slider_drag {
            let t = ((m.x - SLIDER_X0) / (SLIDER_X1 - SLIDER_X0)).clamp(0.0, 1.0);
            let target = RINGS + (t * (MAX_RINGS - RINGS) as f32).round() as u32;
            if target != self.rings && self.pending_rings.is_none() {
                self.pending_rings = Some(target);
            }
        }
        self.step(dt.as_secs_f32(), input);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(rings) = self.pending_rings.take() {
            self.set_rings(renderer, rings);
        }
        if let Some(seed) = self.pending_seed.take() {
            self.regenerate(renderer, seed);
        }
        let camera = self.camera();
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.4, 0.9, 0.35).normalize(),
            ..SceneLighting::default()
        });
        renderer.draw_sky();
        self.draw_stack(renderer);
        if SHOW_HEX_LABELS {
            self.draw_hex_labels(renderer, &camera);
        }
        self.draw_overlay(renderer);
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    run(SceneManager::new(Box::new(World::new())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_layout_tessellates_without_overlap() {
        // Every size the slider can select must lay out without overlapping hexes.
        for r in RINGS..=MAX_RINGS {
            let map = HexMap::new(r);
            let places: Vec<Vec2> = (0..map.total()).map(|i| hex_flat_pos(map.coord(i), r)).collect();
            for a in 0..places.len() {
                for b in a + 1..places.len() {
                    let d = (places[a] - places[b]).length();
                    assert!(d > HEX_HALF_W, "R={r}: hexes {a},{b} overlap (d = {d:.1})");
                }
            }
        }
    }

    #[test]
    fn each_ring_is_full_and_separated_by_hemisphere() {
        let map = HexMap::new(RINGS);
        let (mut north, mut south) = (0u32, 0u32);
        for i in 0..map.total() {
            match map.coord(i).hemi {
                Hemisphere::North => north += 1,
                Hemisphere::South => south += 1,
            }
        }
        assert_eq!(north, south);
        assert_eq!(north + south, 2 + 6 * RINGS * (RINGS + 1));
    }

    #[test]
    fn epoch_stack_is_six_layers_per_hex() {
        let w = World::new();
        for tile in &w.tiles {
            assert_eq!(tile.epoch_states.len(), EPOCHS);
        }
    }

    #[test]
    fn hemispheres_wind_the_same_way_a_screw_not_a_mirror() {
        // A screw keeps the chirality: the north equator ring and the south
        // equator ring, traced in index order, must wind the **same** rotational
        // direction (same signed-area sign). A mirror would flip one of them.
        let signed_area = |map: &HexMap, r: u32, hemi: Hemisphere| -> f32 {
            let eq: Vec<Vec2> = (0..map.total())
                .filter(|&i| map.coord(i).ring == r && map.coord(i).hemi == hemi)
                .map(|i| hex_flat_pos(map.coord(i), r))
                .collect();
            (0..eq.len())
                .map(|k| {
                    let (p, q) = (eq[k], eq[(k + 1) % eq.len()]);
                    p.x * q.y - q.x * p.y
                })
                .sum()
        };
        for r in RINGS..=MAX_RINGS {
            let map = HexMap::new(r);
            let n = signed_area(&map, r, Hemisphere::North);
            let s = signed_area(&map, r, Hemisphere::South);
            assert!(
                n.signum() == s.signum(),
                "R={r}: hemispheres wind oppositely (mirror), not the same (screw): N={n:.0} S={s:.0}"
            );
        }
    }

    #[test]
    fn interior_hexes_fill_all_six_edges_distinctly() {
        // The seam-free blend assumes each interior hex's six neighbours land on
        // six distinct edges (the flat layout agrees with the cube topology) — at
        // every map size the slider can select.
        for r in RINGS..=MAX_RINGS {
            let map = HexMap::new(r);
            for i in 0..map.total() {
                if map.coord(i).ring >= r {
                    continue; // equator hexes legitimately fold / have a 5-defect
                }
                let slots = neighbor_edges(&map, i, r);
                assert!(slots.iter().all(Option::is_some), "R={r}: hex {i} left an edge empty");
                for k in 0..6 {
                    for l in k + 1..6 {
                        assert_ne!(slots[k], slots[l], "R={r}: hex {i} put a neighbour on two edges");
                    }
                }
            }
        }
    }
}
