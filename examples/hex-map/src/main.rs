//! hex-map: the first brick of the world hex-map client. It establishes the
//! coordinate frame the map ordering will be built on top of, by drawing:
//!
//!   * an **XYZ compass** through the origin — red X, green Y, blue Z, with the
//!     world convention `+X = west, +Y = up, +Z = north` (right-handed, Y-up,
//!     matching voxel-cluster); each axis's positive half is bright with a
//!     pyramid arrowhead, the negative half dim, so `+`/`-` reads at a glance;
//!   * **flat-top hexagons**, 2048 units corner-to-corner (the two points face
//!     east/west: +X = west, −X = east), drawn flat on the ground plane as
//!     rasterized line wireframes — with a **dot at each corner**, each edge in
//!     its own colour and labelled `a`–`f` (clockwise from the west point);
//!   * a **number billboard** over each hex centre — the per-hex map-ordering
//!     label. The right map is numbered centre-outward as one clockwise spiral:
//!     0 centre, ring 1 (1–6), ring 2 (7–18, hand-built), ring 3 (19–36, grown
//!     by the [`first_ring`] formula). Each ring ends on its SW corner, so the
//!     map ends on a corner (36 = 3f). Bump the ring count and the spiral grows
//!     in place.
//!   * a **second "left map"** to the west (screen-left), **record-flipped**
//!     about the N-S axis (Y down, west↔east mirrored; b/e stay N/S), numbered
//!     outer-ring-inward and continuing the count (37–73) so a new ring lands in
//!     the middle of the whole sequence; a second compass sits on its centre.
//!   * a **roll wheel** south of each map: left-drag to roll it about its N-S
//!     axis, 0–180° (tops always tilt away from each other).
//!   * **world-gen terrain** on every tile: the hex-world six-epoch stack
//!     (`flicker-worldgen`) runs over this layout's numbering and adjacency, and
//!     each tile draws the consolidated top layer — the Epoch-6 eroded ground
//!     with its sea — in place of the grey fill, while **all six epoch layers
//!     stay resident** for the ongoing generation tweaks (see `terrain.rs`).
//!     The ring slider rebuilds the whole world.
//!
//! It follows the voxel-cluster application model end to end: a [`Scene`] driven
//! by [`SceneManager`]/[`run`], the [`InputMap`]/[`AbstractControls`] input
//! system, and a Lua-scripted HUD (`scripts/hud.lua` + `ui_elements.json`)
//! published through the engine `Model`.
//!
//! Camera (MMO-style):
//!   * WASD — move forward/back and strafe in the camera's facing.
//!   * R / F — rise / descend (world up / down).
//!   * Right-drag — free-look yaw + pitch.
//!   * Escape — quit.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, AbstractControls, Action, InputMap, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshHandle, MeshIndices, Renderer, SceneLighting, TextureHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};
use flicker_materials::Tables;

mod gadget;
mod geom;
mod map_structure;
mod snap_map;
mod snap_segment;
mod terrain;
mod text;
mod topology;

// The hex geometry primitives, the spacing/size constants, `HexInst`, and the
// two-map layout builders live in `geom`; re-export them at the crate root so
// `crate::…` paths (here and in `terrain`) keep resolving as the client is
// split into modules.
pub use geom::*;
// Bitmap-text helpers: the atlas/disc texture builders.
use text::{build_disc_texture, build_glyph_atlas};
// One hemisphere: its tiles' fence fold + the wheel/compass gadget that places
// and turns it, plus the shared tile-draw primitive and its GPU-handle bundle.
use map_structure::{draw_tile, MapStructure, TileAssets};
// The click-to-select fold-in, and the flat horizon the navigator steps over.
use snap_map::{horizon, SnapMap};
// The navigator's horizon panel: the turtle + the snapped tiles around it.
use snap_segment::SnapMapSegment;

const HUD_SCRIPT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/hud.lua");
const UI_ELEMENTS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui_elements.json");

/// Colour of the ring-count slider handle (a soft cyan, distinct from the edge
/// hues and the yellow roll wheels).
const GUIDE_COLOR: [f32; 4] = [0.55, 0.85, 1.0, 0.9];

/// Ring-count range the slider spans, and the start value. Each map has a centre
/// tile plus `rings` rings; the fence view divides the quarter-turn (centre →
/// equator) evenly across `rings`, so ring `k` tilts `k·(90°/rings)`.
const MIN_RINGS: usize = 1;
const MAX_RINGS: usize = 5;
const DEFAULT_RINGS: usize = 3;

// Ring-count slider (screen-space, pixels from the top-left, below the HUD
// stats block). Drag the handle 1‥5 to grow/shrink both maps.
const SLIDER_X0: f32 = 20.0;
const SLIDER_X1: f32 = 220.0;
const SLIDER_Y: f32 = 210.0;
const SLIDER_TRACK_H: f32 = 6.0;
const SLIDER_HANDLE_W: f32 = 14.0;
const SLIDER_HANDLE_H: f32 = 28.0;
/// Cursor distance (px) within which a press grabs the slider.
const SLIDER_HIT_PAD: f32 = 22.0;

/// World seed driving the six-epoch world-gen stack (hex-world's test seed).
const WORLD_SEED: u64 = 0x0EC0_DE01;
/// Hover hit-test rate — the mouse pick runs this many times a second, not every
/// frame.
const PICK_HZ: f32 = 15.0;
/// Vertical FOV used to build the pick ray; matches the render camera.
const PICK_FOV_Y: f32 = std::f32::consts::PI / 3.0; // 60°
/// Turtle travel speed in navigate mode (world units/second) — slow, ~2s to
/// cross a tile, so the world scrolls gently under the turtle.
const NAV_SPEED: f32 = 1100.0;
/// Turtle turn rate in navigate mode (radians/second) for A/D.
const TURN_SPEED: f32 = 1.6;

// ───────────────────────────────────────────────────────────────────
// Drawing & widget helpers (compass, wheel, rosette — modularised in
// later slices into gadget.rs / snap_map.rs / map_structure.rs)
// ───────────────────────────────────────────────────────────────────

// ───────────────────────────────────────────────────────────────────
// Scene
// ───────────────────────────────────────────────────────────────────

/// Which roll wheel the cursor is currently dragging.
#[derive(Copy, Clone, PartialEq)]
enum Wheel {
    Right,
    Left,
}

struct HexScene {
    /// Camera eye position (world).
    position: Vec3,
    /// Look angles, right-handed Y-up. yaw 0 faces +Z.
    yaw: f32,
    pitch: f32,
    /// Cursor at the previous look frame, for the right-drag delta.
    last_look_cursor: Option<Vec2>,
    bindings: InputMap,
    controls: AbstractControls,

    /// The two hemispheres — each owns its tiles' fence fold and its roll
    /// wheel/compass gadget. The north starts upright; the south
    /// (record-flipped) starts rolled to π, tops tilting apart.
    north: MapStructure,
    south: MapStructure,
    /// The roll wheel being left-dragged, and the cursor at the previous frame.
    drag: Option<Wheel>,
    drag_cursor: Vec2,
    /// True while the ring-count slider handle is being dragged.
    slider_drag: bool,

    /// Number of rings per map (1‥5), set by the slider. Drives the tile layout,
    /// dome radius/divisions, and equator yaw.
    num_rings: usize,
    /// Per-hex placement for the current ring count (number, centre, flip, dome
    /// centre, which map), shared by drawing and the pick test. Rebuilt on every
    /// ring-count change; roll-independent otherwise.
    hexes: Vec<HexInst>,
    /// Same-map neighbours per tile number (rebuilt with `hexes`).
    within: Vec<Vec<u32>>,
    /// Number of the hex the cursor is hovering, if any (mouse-pick result).
    hovered: Option<u32>,
    /// The clicked/selected hex, if any (toggled by left-click). While set, its
    /// six neighbours fold flat into its tangent plane, aligned to its edges,
    /// instead of pushing outward.
    selected: Option<u32>,
    /// The tile the navigator's turtle is currently within.
    player_tile: u32,
    /// The turtle's sub-tile position inside `player_tile`, in world units in the
    /// hex plane (`y = 0`). Stays within the hexagon; when it crosses an edge the
    /// player tile becomes the neighbour there and the offset re-centres. The
    /// world scrolls by this under the fixed turtle.
    player_offset: Vec3,
    /// The turtle's heading (yaw, radians; 0 = north). A/D turn it; W/S move
    /// along it (Logo-turtle style). The map stays north-up; the turtle rotates.
    heading: f32,
    /// Tiles to stretch this frame: the hovered tile plus its six neighbours
    /// (same-map + across-equator). Recomputed with `hovered` at [`PICK_HZ`].
    highlight: Vec<u32>,
    /// Seconds since the last hover pick — the hit-test runs at [`PICK_HZ`].
    pick_accum: f32,

    // GPU handles, uploaded once in `enter`.
    white: Option<TextureHandle>,
    dot: Option<TextureHandle>,
    glyphs: Option<TextureHandle>,
    /// Unit translucent hexagon face, drawn per tile as the pickable surface.
    fill_mesh: Option<MeshHandle>,
    script: Option<ScriptHost>,

    /// Materials vocabulary the epochs run on; `None` if the JSON tables failed
    /// to load (tiles then keep the plain grey fill).
    tables: Option<Tables>,
    /// The generated world — all six epoch layers per tile, retained so the
    /// ongoing generation tweaks can re-read any layer. Drawing uses only the
    /// consolidated top layer.
    world: Option<terrain::WorldGen>,
    /// Per-tile ground + sea meshes (indexed by tile number).
    terrain: Vec<terrain::TileTerrain>,
    /// World/meshes need (re)building next frame — set at startup and by the
    /// ring slider; the upload needs the `&mut Renderer` only `render` holds.
    terrain_dirty: bool,
    /// Fence view (toggle `G`): every ring tilts up by its even subdivision of
    /// the quarter-turn (`k·90°/rings`) — centre flat, equator vertical as six
    /// walls per map — folding the flat map into a faceted dome.
    fence: bool,
    /// Rising-edge latch for the fence toggle key, so one tap flips it.
    fence_key_was_down: bool,
    /// Navigate mode (toggle `N`): WASD scrolls the world smoothly under the
    /// fixed turtle (camera fly suspended), so the panel shows the tiles the
    /// engine snaps as you travel.
    navigate: bool,
    /// Rising-edge latch for the navigate toggle key.
    nav_key_was_down: bool,
}

impl HexScene {
    fn new() -> Self {
        let hexes = build_hex_instances(DEFAULT_RINGS);
        let within = build_within_neighbors(&hexes);
        Self {
            // Above and south, angled down — framed for both ring-3 maps plus
            // the roll wheels to their south.
            position: Vec3::new(8800.0, 8600.0, -15500.0),
            yaw: 0.0,    // face +Z (north)
            pitch: -0.5, // look down at it
            last_look_cursor: None,
            bindings: InputMap::wasd_and_mouse(),
            controls: AbstractControls {
                // Two big charts side by side (~33 k across) — fly fast.
                move_speed: 4000.0,
                ..AbstractControls::default()
            },
            north: MapStructure::north(wheel_z(DEFAULT_RINGS)),
            south: {
                let cx = sep(DEFAULT_RINGS);
                MapStructure::south(cx, Vec3::new(cx, 0.0, left_center().z), wheel_z(DEFAULT_RINGS))
            },
            drag: None,
            drag_cursor: Vec2::ZERO,
            slider_drag: false,
            num_rings: DEFAULT_RINGS,
            hexes,
            within,
            hovered: None,
            selected: None,
            player_tile: 0,
            player_offset: Vec3::ZERO,
            heading: 0.0,
            highlight: Vec::new(),
            pick_accum: 0.0,
            white: None,
            dot: None,
            glyphs: None,
            fill_mesh: None,
            script: None,
            tables: terrain::load_tables(),
            world: None,
            terrain: Vec::new(),
            terrain_dirty: true,
            fence: false,
            fence_key_was_down: false,
            navigate: false,
            nav_key_was_down: false,
        }
    }

    /// Unit vector the camera looks along, from yaw/pitch. Right-handed Y-up.
    fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos())
    }

    /// Horizontal forward (ignores pitch so WASD stays in the XZ plane).
    fn move_forward(&self) -> Vec3 {
        let f = self.forward();
        Vec3::new(f.x, 0.0, f.z).normalize_or_zero()
    }

    /// Horizontal "right" relative to facing (matches the renderer's view basis).
    fn move_right(&self) -> Vec3 {
        let flat = self.move_forward();
        flat.cross(Vec3::Y).normalize_or_zero()
    }

    /// This frame's map transforms (north, south) — each map rolls about its own
    /// N-S column, the south the opposite way so their tops tilt apart. Shared by
    /// `render` (drawing) and `pick` (hit-test) so both agree.
    fn map_transforms(&self) -> (Mat4, Mat4) {
        (self.north.transform(), self.south.transform())
    }

    /// The map structure tile `inst` belongs to (its fence fold + placement).
    fn map_of(&self, inst: &HexInst) -> &MapStructure {
        if inst.left {
            &self.south
        } else {
            &self.north
        }
    }

    /// Set the ring count (clamped to `MIN_RINGS‥MAX_RINGS`) and, if it changed,
    /// rebuild the tile layout and neighbour table, slide the south map out to
    /// the wider separation, and clear the hover/selection (tile numbers change
    /// with the count).
    fn set_rings(&mut self, rings: usize) {
        let rings = rings.clamp(MIN_RINGS, MAX_RINGS);
        if rings == self.num_rings {
            return;
        }
        self.num_rings = rings;
        self.hexes = build_hex_instances(rings);
        self.within = build_within_neighbors(&self.hexes);
        // Larger maps need more room — slide the south map out to the new centre
        // column so the two never overlap, and move both maps' wheels further
        // south so a bigger map can't draw over them. The rolls are kept.
        self.north.set_placement(0.0, wheel_z(rings));
        self.south.set_placement(sep(rings), wheel_z(rings));
        self.hovered = None;
        self.selected = None;
        self.player_tile = 0; // tile numbers change with the ring count
        self.player_offset = Vec3::ZERO;
        self.heading = 0.0;

        self.highlight.clear();
        // The tile set changed, so the whole world regenerates (next render,
        // which holds the `&mut Renderer` the mesh upload needs).
        self.terrain_dirty = true;
    }

    /// Free the old terrain meshes and, when the vocabulary is loaded, run the
    /// six-epoch stack over the current tile set and upload each tile's
    /// top-layer meshes. The across-equator adjacency is read at the rest pose
    /// so wheel state can't change the generated world.
    fn rebuild_terrain(&mut self, renderer: &mut Renderer) {
        for t in self.terrain.drain(..) {
            renderer.free_mesh(t.ground);
            if let Some(w) = t.water {
                renderer.free_mesh(w);
            }
        }
        self.world = None;
        let Some(tables) = self.tables.as_ref() else { return };
        let world = terrain::WorldGen::generate(
            tables,
            &self.hexes,
            &self.within,
            self.num_rings,
            WORLD_SEED,
        );
        self.terrain = world.upload_meshes(tables, &self.hexes, renderer);
        self.world = Some(world);
    }

    /// Screen-X of the slider handle for the current ring count.
    fn slider_handle_x(&self) -> f32 {
        let t = (self.num_rings - MIN_RINGS) as f32 / (MAX_RINGS - MIN_RINGS) as f32;
        SLIDER_X0 + t * (SLIDER_X1 - SLIDER_X0)
    }

    /// Ring count for a cursor at screen-X `x` (snapped to an integer step).
    fn slider_value(&self, x: f32) -> usize {
        let t = ((x - SLIDER_X0) / (SLIDER_X1 - SLIDER_X0)).clamp(0.0, 1.0);
        MIN_RINGS + (t * (MAX_RINGS - MIN_RINGS) as f32).round() as usize
    }

    /// Whether a cursor at `p` is close enough to the slider to grab it.
    fn over_slider(&self, p: Vec2) -> bool {
        p.x >= SLIDER_X0 - SLIDER_HIT_PAD
            && p.x <= SLIDER_X1 + SLIDER_HIT_PAD
            && (p.y - SLIDER_Y).abs() <= SLIDER_HIT_PAD
    }

    /// Build a world-space pick ray (origin, unit dir) through the pixel at
    /// `cursor` for a `viewport`-sized window, from the camera basis. Mirrors
    /// voxel-cluster's `build_pick_ray`.
    fn build_pick_ray(&self, cursor: Vec2, viewport: Vec2) -> (Vec3, Vec3) {
        let f = self.forward();
        let r = f.cross(Vec3::Y).normalize_or_zero();
        let u = r.cross(f).normalize_or_zero();
        let aspect = viewport.x / viewport.y;
        let t = (PICK_FOV_Y * 0.5).tan();
        // +0.5 so the ray passes through the pixel centre.
        let ndc_x = 2.0 * (cursor.x + 0.5) / viewport.x - 1.0;
        let ndc_y = 1.0 - 2.0 * (cursor.y + 0.5) / viewport.y;
        let dir = (f + r * (ndc_x * aspect * t) + u * (ndc_y * t)).normalize_or_zero();
        (self.position, dir)
    }

    /// Cast a ray through `cursor` and return the `number` of the nearest hex
    /// face it hits (resting positions, so hover stays put while the highlight
    /// stretches). `None` if the cursor is over empty space.
    fn pick(&self, cursor: Vec2, viewport: Vec2) -> Option<u32> {
        if viewport.x <= 0.0 || viewport.y <= 0.0 {
            return None;
        }
        let (m_right, m_left) = self.map_transforms();
        let (origin, dir) = self.build_pick_ray(cursor, viewport);
        let mut best: Option<(f32, u32)> = None;
        for inst in &self.hexes {
            let xform = if inst.left { &m_left } else { &m_right };
            let map = self.map_of(inst);
            let fc = map.tile_fence_center(inst, &self.hexes, self.num_rings, self.fence);
            let center = xform.transform_point3(fc);
            let tilt = map.tile_tilt(inst, &self.hexes, self.num_rings, self.fence);
            let corners = hex_world_corners(fc, inst.flip, tilt, xform);
            for i in 0..6 {
                if let Some(t) = ray_triangle(origin, dir, center, corners[i], corners[(i + 1) % 6])
                {
                    if best.is_none_or(|(bt, _)| t < bt) {
                        best = Some((t, inst.number));
                    }
                }
            }
        }
        best.map(|(_, n)| n)
    }

    /// The neighbours of tile `n`: its same-map neighbours ([`Self::within`])
    /// first, then — if it's an equator tile — its across-equator twins from the
    /// deterministic σ-zipper ([`topology::Topology::equator_partners`]). No
    /// proximity, so the set is roll-independent. Interior tiles already hold six
    /// within-neighbours; equator **edge** tiles reach six (four within + two
    /// cross), equator **corner** tiles five (three within + two cross).
    fn neighbors_of(&self, n: u32) -> Vec<u32> {
        let mut out = self.within[n as usize].clone();
        for p in topology::Topology::new(self.num_rings).equator_partners(n) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// The seven-tile highlight set for the current hover: the hovered tile plus
    /// its six neighbours.
    fn compute_highlight(&self) -> Vec<u32> {
        let Some(h) = self.hovered else {
            return Vec::new();
        };
        let mut hi = Vec::with_capacity(7);
        hi.push(h);
        hi.extend(self.neighbors_of(h));
        hi
    }

    /// The neighbour of `tile` in world direction `dir` (unit) and its position in
    /// `tile`'s horizon layout, or `None` if no neighbour lies that way (a
    /// pentagon-defect corner with a missing edge). Reads the horizon — same-map
    /// neighbours and across-equator twins alike — so a crossing onto the south
    /// map happens on the same outward edge it's drawn on.
    fn neighbor_in_dir(&self, tile: u32, dir: Vec3) -> Option<(u32, Vec3)> {
        horizon(&self.hexes, &self.within, self.num_rings, tile)
            .into_iter()
            .skip(1) // skip the centre tile itself
            .map(|(n, off)| (n, off, off.normalize_or_zero().dot(dir)))
            .filter(|&(_, _, d)| d > 0.5)
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(n, off, _)| (n, off))
    }

    /// Keep the turtle inside its movement cell: while the offset pokes past a
    /// cell edge, hand it onto the tile the topology joins there and **reset the
    /// offset into that tile's frame** (a translation) — within a map and across
    /// the equator alike. The movement cell tiles **gap-free** (apothem
    /// `HEX_SPACING/2`, not the smaller visual apothem), so a crossing lands the
    /// turtle cleanly just inside the next tile.
    ///
    /// Crossing happens on the **geometric** edge you walk into (the same edge the
    /// neighbour is drawn on), so there are no walls on the outward edges — only
    /// the 12 genuine pentagon-defect corners, where a 6th neighbour truly
    /// doesn't exist, clamp (and you simply step around them). The seam frame is
    /// faked, which at 50-mile tiles is invisible.
    fn resolve_crossings(&mut self) {
        let cell = HEX_SPACING * 0.5;
        for _ in 0..4 {
            let off = self.player_offset;
            let Some((e, over)) = (0..6)
                .map(|e| (e, off.dot(edge_normal(e)) - cell))
                .filter(|&(_, over)| over > 0.0)
                .max_by(|a, b| a.1.total_cmp(&b.1))
            else {
                break; // inside the cell
            };
            match self.neighbor_in_dir(self.player_tile, edge_normal(e)) {
                Some((n, n_off)) => {
                    // Crossing onto the *other* map means crossing the equator,
                    // and the south map is **record-flipped** (x-mirror). Carry
                    // the offset and heading through that flip, or "forward" lands
                    // pointing back and the turtle ping-pongs across the seam.
                    let cross_maps =
                        self.hexes[self.player_tile as usize].flip != self.hexes[n as usize].flip;
                    self.player_tile = n;
                    let mut new_off = off - n_off; // reset into the new tile
                    if cross_maps {
                        new_off = Vec3::new(-new_off.x, new_off.y, new_off.z);
                        self.heading = -self.heading;
                    }
                    self.player_offset = new_off;
                }
                None => {
                    // Pentagon-defect corner / world edge — clamp just inside.
                    self.player_offset -= edge_normal(e) * over;
                    break;
                }
            }
        }
    }

    /// While navigate mode is on, draw the turtle on the **main map** too — at
    /// the player's actual tile + sub-tile position, riding that map's roll — so
    /// you can see where on the hemisphere the panel's turtle really is (a check
    /// on the stitching). Mirrored for the record-flipped south map.
    fn draw_map_turtle(&self, renderer: &mut Renderer, m_right: &Mat4, m_left: &Mat4) {
        let inst = &self.hexes[self.player_tile as usize];
        let xform = if inst.left { m_left } else { m_right };
        // Offset and heading read in the tile's drawn frame (the south map is
        // record-flipped, so mirror x and negate the heading).
        let off = if inst.flip {
            Vec3::new(-self.player_offset.x, 0.0, self.player_offset.z)
        } else {
            self.player_offset
        };
        let heading = if inst.flip { -self.heading } else { self.heading };
        // The tile's world position (on its possibly flipped/rolled face), then
        // lift the marker straight up in **world** space so it sits on top and
        // stays visible whichever way the map is flipped or rolled.
        let base = xform.transform_point3(inst.center + off);
        let pos = base + Vec3::new(0.0, 600.0, 0.0);
        let s = HEX_SIZE * 0.5;
        let rot = Mat4::from_rotation_y(heading);
        let v = |local: Vec3| pos + rot.transform_vector3(local);
        let tip = v(Vec3::new(0.0, 0.0, s));
        let bl = v(Vec3::new(s * 0.6, 0.0, -s * 0.7));
        let br = v(Vec3::new(-s * 0.6, 0.0, -s * 0.7));
        let col = [0.25, 1.0, 0.85, 1.0];
        renderer.draw_lines(&[(tip, bl), (bl, br), (br, tip)], col);
    }

    /// Per-frame values handed to the HUD script as the `Model` global.
    fn hud_model(&self) -> ValueMap {
        ValueMap::new()
            .with("pos_x", self.position.x)
            .with("pos_y", self.position.y)
            .with("pos_z", self.position.z)
            .with("yaw", self.yaw)
            .with("pitch", self.pitch)
    }

    /// Draw hexagon `number` centred at `center`: each edge in its `EDGE_COLORS`
    /// hue with its a–f letter at the midpoint, a dot at every corner, and the
    /// number billboard floating over the centre. Every hex shares the same
    /// orientation and edge labelling, so adjacency reads straight off the
    /// colours/letters (e.g. hex 0's edge `a` faces hex 1's edge `d`).
    ///
    /// Draw the ring-count slider (2D, screen-space): a track with a tick per
    /// ring count, a handle at the current value, and a `Rings / Tiles` readout.
    fn draw_slider(&self, renderer: &mut Renderer) {
        let Some(white) = self.white else { return };
        // Track.
        renderer.draw_sprite(
            white,
            Vec2::new(SLIDER_X0, SLIDER_Y - SLIDER_TRACK_H * 0.5),
            Vec2::new(SLIDER_X1 - SLIDER_X0, SLIDER_TRACK_H),
            [0.24, 0.27, 0.33, 0.85],
        );
        // A tick at each integer ring count.
        for n in MIN_RINGS..=MAX_RINGS {
            let t = (n - MIN_RINGS) as f32 / (MAX_RINGS - MIN_RINGS) as f32;
            let x = SLIDER_X0 + t * (SLIDER_X1 - SLIDER_X0);
            renderer.draw_sprite(
                white,
                Vec2::new(x - 1.0, SLIDER_Y - 9.0),
                Vec2::new(2.0, 18.0),
                [0.45, 0.50, 0.58, 0.9],
            );
        }
        // Handle (cyan, matching the guide circles).
        let hx = self.slider_handle_x();
        renderer.draw_sprite(
            white,
            Vec2::new(hx - SLIDER_HANDLE_W * 0.5, SLIDER_Y - SLIDER_HANDLE_H * 0.5),
            Vec2::new(SLIDER_HANDLE_W, SLIDER_HANDLE_H),
            GUIDE_COLOR,
        );
        // Readout above the track.
        let tiles = 2 * (1 + 3 * self.num_rings * (self.num_rings + 1));
        let label = format!("Rings {}   ({} tiles)", self.num_rings, tiles);
        renderer.draw_text(&label, Vec2::new(SLIDER_X0, SLIDER_Y - 32.0), 18.0, [0.90, 0.95, 1.0, 1.0]);
        // Fence-view toggle state + key hint, below the track.
        let fence = format!("Fence [G]: {}", if self.fence { "on" } else { "off" });
        renderer.draw_text(&fence, Vec2::new(SLIDER_X0, SLIDER_Y + 16.0), 18.0, [0.90, 0.95, 1.0, 1.0]);
        // Navigate-mode state + the tile the turtle is on.
        let nav = format!(
            "Navigate [N]: {}   (turtle on {})",
            if self.navigate { "on — WASD scrolls" } else { "off" },
            self.player_tile
        );
        let nav_color = if self.navigate { [0.4, 1.0, 0.85, 1.0] } else { [0.90, 0.95, 1.0, 1.0] };
        renderer.draw_text(&nav, Vec2::new(SLIDER_X0, SLIDER_Y + 38.0), 18.0, nav_color);
    }

}

impl Scene for HexScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // 1×1 white pixel — tinted to build solid HUD quads in `render_hud`.
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // Soft disc for the corner dots, and the digit atlas for the label.
        self.dot = Some(renderer.load_texture(&build_disc_texture(), 16, 16));
        let (atlas, atlas_w, atlas_h) = build_glyph_atlas();
        self.glyphs = Some(renderer.load_texture(&atlas, atlas_w, atlas_h));
        // Unit translucent hex face, instanced per tile via draw_mesh.
        let (verts, idx) = build_hex_fill_mesh();
        self.fill_mesh = Some(renderer.upload_mesh(&verts, MeshIndices::U32(&idx)));

        // Scripted HUD: layout in ui_elements.json (`UI.hud`), behaviour in Lua.
        match ScriptHost::from_file(HUD_SCRIPT_PATH) {
            Ok(s) => {
                load_ui_json(&s, UI_ELEMENTS_PATH);
                load_widgets(&s);
                tracing::info!("loaded HUD script from {HUD_SCRIPT_PATH}");
                self.script = Some(s);
            }
            Err(e) => tracing::error!("HUD script load failed: {e}"),
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // The wasd_and_mouse preset binds Escape to Menu (its later Escape bind
        // wins under the one-input-one-action rule), so check the key directly
        // for this menu-less demo.
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        // Fence view toggle (G): outer-ring tiles stand up as six walls per map.
        // Rising-edge latched so a single tap flips it.
        let g_down = input.key_down(Key::G);
        if g_down && !self.fence_key_was_down {
            self.fence = !self.fence;
        }
        self.fence_key_was_down = g_down;
        // Navigate mode toggle (N): WASD scrolls the world under the turtle.
        let n_down = input.key_down(Key::N);
        if n_down && !self.nav_key_was_down {
            self.navigate = !self.navigate;
        }
        self.nav_key_was_down = n_down;
        let dt_s = dt.as_secs_f32();

        // Roll wheels: left-click a wheel to grab it, drag vertically to roll its
        // map about the N-S axis (clamped to 0..π). Hit-tested by projecting each
        // wheel centre to the screen.
        let screen = renderer.size();
        let aspect = if screen.y > 0.0 { screen.x / screen.y } else { 1.0 };
        let view_proj = Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 100000.0,
        }
        .view_projection(aspect);
        if input.mouse_left_pressed {
            if self.over_slider(input.mouse_position) {
                // Grab the ring-count slider — this press neither rolls nor picks.
                self.slider_drag = true;
                self.set_rings(self.slider_value(input.mouse_position.x));
            } else {
                let cursor = input.mouse_position;
                self.drag = if self.north.wheel_hit(view_proj, screen, cursor) {
                    Some(Wheel::Right)
                } else if self.south.wheel_hit(view_proj, screen, cursor) {
                    Some(Wheel::Left)
                } else {
                    None
                };
                self.drag_cursor = input.mouse_position;
                // A click that didn't grab a wheel toggles tile selection: pick
                // the tile under the cursor; clicking the selected tile again
                // clears it, clicking another selects it, empty space clears it.
                if self.drag.is_none() {
                    self.selected = match self.pick(input.mouse_position, screen) {
                        Some(n) if self.selected == Some(n) => None,
                        other => other,
                    };
                    // Drop the navigator's turtle onto the clicked tile's centre,
                    // facing north.
                    if let Some(n) = self.selected {
                        self.player_tile = n;
                        self.player_offset = Vec3::ZERO;
                        self.heading = 0.0;
                    }
                }
            }
        }
        if input.mouse_left {
            if self.slider_drag {
                self.set_rings(self.slider_value(input.mouse_position.x));
            } else if let Some(wheel) = self.drag {
                let dy = input.mouse_position.y - self.drag_cursor.y;
                self.drag_cursor = input.mouse_position;
                match wheel {
                    Wheel::Right => self.north.apply_drag(dy),
                    Wheel::Left => self.south.apply_drag(dy),
                }
            }
        } else {
            self.drag = None;
            self.slider_drag = false;
        }

        // Look: right-drag, with sensitivity/invert applied by the config —
        // suspended in navigate mode so the camera holds still while you travel.
        if input.mouse_right && !self.navigate {
            if let Some(prev) = self.last_look_cursor {
                let (dyaw, dpitch) = self.controls.look_delta_mouse(input.mouse_position - prev);
                self.yaw -= dyaw;
                self.pitch = (self.pitch + dpitch).clamp(-1.5, 1.5);
            }
            self.last_look_cursor = Some(input.mouse_position);
        } else {
            self.last_look_cursor = None;
        }

        if self.navigate {
            // Navigate mode (Logo-turtle): A/D turn the heading, W/S move along
            // it. The map stays north-up; the world scrolls under the fixed
            // turtle. When the offset leaves the current hexagon the player tile
            // becomes the neighbour the engine joins there — across the equator
            // seam included — so travel stays continuous.
            if input.input_active(&self.bindings, Action::StrafeLeft) {
                self.heading += TURN_SPEED * dt_s; // turn left (toward west)
            }
            if input.input_active(&self.bindings, Action::StrafeRight) {
                self.heading -= TURN_SPEED * dt_s; // turn right (toward east)
            }
            // Forward along the heading (0 = north/+Z, +heading toward west/+X).
            let fwd = Vec3::new(self.heading.sin(), 0.0, self.heading.cos());
            let mut move_dir = Vec3::ZERO;
            if input.input_active(&self.bindings, Action::MoveForward) {
                move_dir += fwd;
            }
            if input.input_active(&self.bindings, Action::MoveBackward) {
                move_dir -= fwd;
            }
            if move_dir.length_squared() > 0.0 {
                self.player_offset += move_dir.normalize() * NAV_SPEED * dt_s;
                self.resolve_crossings();
            }
        } else {
            // Move: free 6-DOF fly in the camera's facing.
            let mut motion = Vec3::ZERO;
            if input.input_active(&self.bindings, Action::MoveForward) {
                motion += self.move_forward();
            }
            if input.input_active(&self.bindings, Action::MoveBackward) {
                motion -= self.move_forward();
            }
            if input.input_active(&self.bindings, Action::StrafeRight) {
                motion += self.move_right();
            }
            if input.input_active(&self.bindings, Action::StrafeLeft) {
                motion -= self.move_right();
            }
            if input.input_active(&self.bindings, Action::MoveUp) {
                motion += Vec3::Y;
            }
            if input.input_active(&self.bindings, Action::MoveDown) {
                motion -= Vec3::Y;
            }
            if motion.length_squared() > 0.0 {
                self.position += motion.normalize() * self.controls.move_speed * dt_s;
            }
        }

        // Hover hit-test, rate-limited to PICK_HZ (not every frame): ray-cast the
        // cursor against the tile faces and remember which tile is under it. The
        // hovered tile is stretched outward in `render`.
        self.pick_accum += dt_s;
        if self.pick_accum >= 1.0 / PICK_HZ {
            self.pick_accum = 0.0;
            // Don't hover-pick through the slider widget.
            self.hovered = if self.over_slider(input.mouse_position) {
                None
            } else {
                self.pick(input.mouse_position, screen)
            };
            self.highlight = self.compute_highlight();
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // (Re)generate the world + terrain meshes when the tile set changed —
        // here because the mesh upload needs the `&mut Renderer`.
        if self.terrain_dirty {
            self.terrain_dirty = false;
            self.rebuild_terrain(renderer);
        }

        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 100000.0,
        });

        // A plain default sky gives a horizon to orient against.
        renderer.set_scene(SceneLighting::default());
        renderer.draw_sky();

        // Both maps' roll/placement transforms (see `map_transforms`): each map
        // rolls about its own N-S column — the north about world Z, the south the
        // opposite way about its own column. Shared with the mouse pick so
        // drawing and hit-test agree.
        let (m_right, m_left) = self.map_transforms();

        // Compass at each map's centre (carried by its gadget's roll).
        self.north.paint_compass(renderer);
        self.south.paint_compass(renderer);

        // When a tile is clicked/selected, snap its neighbours flat against its
        // edges (the snap map around the selected tile).
        let snap = self
            .selected
            .map(|s| SnapMap::build(&self.hexes, &self.within, self.num_rings, s, &m_right, &m_left));

        // GPU handles the tile draw needs, bundled once for the loop.
        let assets = TileAssets { fill_mesh: self.fill_mesh, glyphs: self.glyphs, dot: self.dot };

        // Every tile of both maps: a 25%-grey translucent face (the pickable
        // surface) plus its coloured wireframe + labels (`draw_tile`). A selected
        // tile's neighbours fold in (rosette); the hovered tile and its six
        // neighbours brighten (the hover highlight). The fill insets a hair so
        // edges draw in front.
        for inst in &self.hexes {
            let own = if inst.left { &m_left } else { &m_right };
            let terrain = self.terrain.get(inst.number as usize);
            if let Some(sm) = &snap {
                // A snapped neighbour: draw flat at its slot.
                if let Some(&(_, slot)) = sm.slots.iter().find(|(num, _)| *num == inst.number) {
                    let rx = if sm.left { &m_left } else { &m_right };
                    draw_tile(renderer, assets, terrain, slot, Mat4::IDENTITY, rx, sm.flip, inst.number, true);
                    continue;
                }
                // The selected tile itself: its resting spot, highlighted.
                if sm.center == inst.number {
                    draw_tile(renderer, assets, terrain, inst.center, Mat4::IDENTITY, own, inst.flip, inst.number, true);
                    continue;
                }
            }
            // Hover highlight: the lit tile (hovered + its neighbours) brightens.
            // In fence view the outer ring stands up as flat walls (`tile_tilt`
            // + `tile_fence_center` relay the equator tiles into coplanar rows).
            let lit = self.highlight.contains(&inst.number);
            let map = self.map_of(inst);
            let tilt = map.tile_tilt(inst, &self.hexes, self.num_rings, self.fence);
            let center = map.tile_fence_center(inst, &self.hexes, self.num_rings, self.fence);
            draw_tile(renderer, assets, terrain, center, tilt, own, inst.flip, inst.number, lit);
        }

        // Roll wheels, south of each map on its N-S axis (left-drag to roll).
        self.north.paint_wheel(renderer);
        self.south.paint_wheel(renderer);

        // The navigator's horizon panel — the turtle and the tiles the engine
        // snaps around it, scrolled by the turtle's sub-tile offset — above and
        // centred between the two maps.
        SnapMapSegment::draw(
            renderer,
            assets,
            &self.hexes,
            &self.within,
            self.num_rings,
            self.player_tile,
            self.player_offset,
            self.heading,
        );
        // While navigating, also mark the turtle on the hemisphere map itself.
        if self.navigate {
            self.draw_map_turtle(renderer, &m_right, &m_left);
        }

        // Scripted HUD: publish the live model, then draw the script's commands.
        if let (Some(script), Some(white)) = (self.script.as_ref(), self.white) {
            let screen = renderer.size();
            if let Err(e) = script.set_model(&self.hud_model()) {
                tracing::error!("HUD model publish failed: {e}");
            }
            match script.draw(screen.x, screen.y) {
                Ok(cmds) => render_hud(renderer, &cmds, white, &[]),
                Err(e) => tracing::error!("HUD draw failed: {e}"),
            }
        }

        // Ring-count slider, on top of the HUD.
        renderer.set_layer(1000.0);
        self.draw_slider(renderer);
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "hex_map=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    run(SceneManager::new(Box::new(HexScene::new())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Load the *real* HUD script + layout and run a frame, so a Lua
    //! syntax/runtime error (or a `UI.hud` key the script reads but the JSON
    //! forgot to define) fails the build rather than only the running app.
    use super::*;
    use flicker::app::InputState;

    #[test]
    fn highlight_follows_the_fold_topology() {
        let mut s = HexScene::new();

        // Interior tiles (right centre + ring 1 + ring 2 = 0..=18) have all six
        // neighbours within their own map.
        for n in 0..=18u32 {
            assert_eq!(s.within[n as usize].len(), 6, "interior tile {n}");
        }
        // Equator tiles (right ring 3 = 19..=36) come up short — hex-ring corners
        // keep 3 same-map neighbours, straight-run edges keep 4 — and each gains
        // exactly two across-equator twins from the σ-zipper.
        for n in 19..=36u32 {
            let w = s.within[n as usize].len();
            assert!(w == 3 || w == 4, "equator tile {n} within = {w}");
            assert_eq!(s.neighbors_of(n).len(), w + 2, "equator tile {n} twins");
        }

        // Hovering lights the tile plus its neighbours: seven for interior and
        // equator-edge tiles (six neighbours), six for the equator corners
        // (five — three within + two cross). Never proximity, so roll-stable.
        for n in 0..s.hexes.len() as u32 {
            s.hovered = Some(n);
            let hl = s.compute_highlight();
            let want = 1 + s.neighbors_of(n).len();
            assert_eq!(hl.len(), want, "tile {n} highlight {hl:?}");
            assert!((6..=7).contains(&hl.len()), "tile {n} highlight {} off-range", hl.len());
            assert!(hl.contains(&n), "tile {n} not in its own highlight");
            assert!(hl[1..].iter().all(|&m| m != n), "tile {n} duplicated");
        }
    }

    #[test]
    fn snap_map_folds_neighbours() {
        let s = HexScene::new();
        let (mr, ml) = s.map_transforms();
        for n in 0..s.hexes.len() as u32 {
            let sm = SnapMap::build(&s.hexes, &s.within, s.num_rings, n, &mr, &ml);
            assert_eq!(sm.center, n);
            // The fold-in places exactly the tile's neighbours — six for most,
            // five for the equator corners.
            assert_eq!(sm.slots.len(), s.neighbors_of(n).len(), "tile {n} snap size");
            let mut got: Vec<u32> = sm.slots.iter().map(|(m, _)| *m).collect();
            got.sort();
            let mut want = s.neighbors_of(n);
            want.sort();
            assert_eq!(got, want, "tile {n} snap ≠ its neighbours");
            // ...each on a distinct edge slot (no two neighbours stacked).
            let m = sm.slots.len();
            for i in 0..m {
                for j in (i + 1)..m {
                    assert!(
                        (sm.slots[i].1 - sm.slots[j].1).length() > 1.0,
                        "tile {n} slots {i},{j} coincide"
                    );
                }
            }
        }
    }

    #[test]
    fn dynamic_ring_counts() {
        let mut s = HexScene::new();
        for rings in MIN_RINGS..=MAX_RINGS {
            s.set_rings(rings);
            assert_eq!(s.num_rings, rings);
            // Two maps, each a centre + `rings` rings of 6k tiles.
            let expect = 2 * (1 + 3 * rings * (rings + 1));
            assert_eq!(s.hexes.len(), expect, "{rings} rings → tile count");
            // The fold topology survives at every ring count: each tile lights
            // itself plus its neighbours — seven for full tiles, six for the
            // equator corners (and at one ring every equator tile is a corner).
            for n in 0..s.hexes.len() as u32 {
                s.hovered = Some(n);
                let hl = s.compute_highlight().len();
                assert_eq!(hl, 1 + s.neighbors_of(n).len(), "{rings} rings, tile {n}");
                assert!((6..=7).contains(&hl), "{rings} rings, tile {n} highlight {hl}");
            }
        }
        // One ring per map is the "map of 14": two centres + two rings of six.
        s.set_rings(1);
        assert_eq!(s.hexes.len(), 14);
        // Clamped to the slider range.
        s.set_rings(99);
        assert_eq!(s.num_rings, MAX_RINGS);
        s.set_rings(0);
        assert_eq!(s.num_rings, MIN_RINGS);
    }

    #[test]
    fn hud_script_runs() {
        let host = ScriptHost::from_file(HUD_SCRIPT_PATH).expect("load hud.lua");
        load_ui_json(&host, UI_ELEMENTS_PATH); // exposes `UI.hud`
        load_widgets(&host);

        let model = ValueMap::new()
            .with("pos_x", 1.0_f32)
            .with("pos_y", 2.0_f32)
            .with("pos_z", 3.0_f32)
            .with("yaw", 0.5_f32)
            .with("pitch", -0.25_f32);
        host.set_model(&model).expect("publish model");

        let input = InputState::new();
        host.update(&input, 1280.0, 720.0).expect("update runs");
        let cmds = host.draw(1280.0, 720.0).expect("draw runs");
        assert!(!cmds.is_empty(), "HUD emits draw commands");
    }
}
