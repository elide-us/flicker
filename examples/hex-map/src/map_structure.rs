//! `MapStructure`: one hemisphere of the world map — its tiles' **fence/dome
//! fold** and the **wheel + axis gadget** that turns and places it. The two
//! maps (north, south) are MapStructures.
//!
//! The tile *data* stays in the scene's shared `Vec<HexInst>` — the global
//! numbering drives the topology adjacency and the world-gen, and the
//! cross-equator stitch is inherently a both-maps relation — so a MapStructure
//! is a per-hemisphere **placement + behaviour view**, selecting its own tiles
//! by `HexInst::left == is_south` rather than owning them. It wraps the map's
//! [`WheelAxisGadget`] and exposes the roll/compass controls plus the fence
//! geometry the scene's draw and pick share.

use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, Renderer, TextureHandle, Vec2, Vec3,
};

use crate::gadget::WheelAxisGadget;
use crate::terrain::TileTerrain;
use crate::text::draw_text_billboard;
use crate::topology::Topology;
use crate::{flip_ns, hex_corners, ring_dome_angle, HexInst, HEX_SIZE, HEX_SPACING};

/// World diameter of the dot billboard at each hex corner.
const DOT_SIZE: f32 = 56.0;
/// World height of the centre label's glyphs.
const LABEL_SIZE: f32 = 360.0;
/// Per-edge colours, a–f. Each of the six edges gets a distinct hue so the
/// map-ordering work can name exactly which edge connects to which.
const EDGE_COLORS: [[f32; 4]; 6] = [
    [0.95, 0.26, 0.26, 1.0], // a — red
    [0.98, 0.62, 0.20, 1.0], // b — orange
    [0.96, 0.90, 0.32, 1.0], // c — yellow
    [0.42, 0.86, 0.46, 1.0], // d — green
    [0.36, 0.68, 0.98, 1.0], // e — blue
    [0.80, 0.50, 0.96, 1.0], // f — violet
];
/// World height of each edge's a–f letter glyph.
const EDGE_LABEL_SIZE: f32 = 220.0;
/// How far past the edge midpoint (outward from the centre) the letter sits.
const EDGE_LABEL_OUT: f32 = 150.0;
/// Tint (× the lit base colour) of a hovered/selected tile's terrain.
const TERRAIN_HOVER_TINT: [f32; 4] = [1.45, 1.45, 1.45, 1.0];
/// Tint (× the lit base colour) of a resting tile fill; `.w` is its opacity.
const FILL_TINT: [f32; 4] = [1.0, 1.0, 1.0, 0.25];
/// Tint of the hovered tile fill — brighter and more opaque.
const HOVER_TINT: [f32; 4] = [1.6, 1.6, 1.6, 0.55];
/// Tiny downward (−Y) offset of the fill below the wireframe plane, so the
/// coloured edges always draw in front of the fill (no z-fighting).
const FILL_INSET: f32 = 4.0;

/// The GPU handles the tile draw needs (uploaded once by the scene): the grey
/// fallback face, the glyph atlas, and the corner-dot texture. Bundled so the
/// draw primitive — reused by the main maps and the snap-map segment — stays a
/// clean call.
#[derive(Copy, Clone, Default)]
pub struct TileAssets {
    pub fill_mesh: Option<MeshHandle>,
    pub glyphs: Option<TextureHandle>,
    pub dot: Option<TextureHandle>,
}

/// One hemisphere: its gadget (roll wheel + compass) and which side it is.
pub struct MapStructure {
    /// `false` = north (right map), `true` = south (left, record-flipped).
    // Read by `owns`, which the draw/pick split (slice 4b) uses to filter the
    // shared tile slice to this map.
    #[allow(dead_code)]
    pub is_south: bool,
    gadget: WheelAxisGadget,
}

impl MapStructure {
    /// The north/right map: upright, rolling about world Z. `wheel_z` is the
    /// (ring-scaled) south position of its roll wheel.
    pub fn north(wheel_z: f32) -> Self {
        Self { is_south: false, gadget: WheelAxisGadget::north(wheel_z) }
    }

    /// The south/left map: record-flipped about its own column `cx`, starting
    /// rolled to π. `compass_origin` is the reflected map centre; `wheel_z` the
    /// (ring-scaled) south position of its roll wheel.
    pub fn south(cx: f32, compass_origin: Vec3, wheel_z: f32) -> Self {
        Self { is_south: true, gadget: WheelAxisGadget::south(cx, compass_origin, wheel_z) }
    }

    /// Whether tile `inst` belongs to this map. (Consumed by the draw/pick split
    /// in slice 4b, where each map iterates its own share of the tile slice.)
    #[allow(dead_code)]
    pub fn owns(&self, inst: &HexInst) -> bool {
        inst.left == self.is_south
    }

    /// Slide the whole unit — tiles' roll axis, wheel, and compass — to a new
    /// centre column `cx` and wheel south-position `wheel_z`, keeping its roll.
    /// Called when the ring count changes the map separation and extent so the
    /// map and its gadget move together.
    pub fn set_placement(&mut self, cx: f32, wheel_z: f32) {
        self.gadget.set_placement(cx, wheel_z);
    }

    /// This map's roll/placement transform — shared by draw and pick so they
    /// agree.
    pub fn transform(&self) -> Mat4 {
        self.gadget.transform()
    }

    /// Whether the cursor is over this map's roll wheel.
    pub fn wheel_hit(&self, view_proj: Mat4, screen: Vec2, cursor: Vec2) -> bool {
        self.gadget.wheel_hit(view_proj, screen, cursor)
    }

    /// Feed a vertical drag (pixels) into this map's roll.
    pub fn apply_drag(&mut self, dy_pixels: f32) {
        self.gadget.apply_drag(dy_pixels);
    }

    /// Draw this map's compass (carried by its roll).
    pub fn paint_compass(&self, renderer: &mut Renderer) {
        self.gadget.paint_compass(renderer);
    }

    /// Draw this map's roll wheel.
    pub fn paint_wheel(&self, renderer: &mut Renderer) {
        self.gadget.paint_wheel(renderer);
    }

    /// Fence frame for a non-pole tile of this map in fence view:
    /// `(side_start, p0, t, n, k)` — the tile's ring-side first tile, its centre
    /// `p0` (the facet anchor), the unit run direction `t` along the side, the
    /// outward unit normal `n` (⊥ `t`), and the ring index `k` (which sets the
    /// tilt). `None` when fence view is off or `inst` is a pole. All tiles of a
    /// side share `t`/`n`, so a tilted side is one flat facet
    /// ([`Topology::ring_side`] gives the grouping).
    fn fence_frame(
        &self,
        inst: &HexInst,
        hexes: &[HexInst],
        rings: usize,
        fence: bool,
    ) -> Option<(u32, Vec3, Vec3, Vec3, usize)> {
        if !fence {
            return None;
        }
        let (k, _, range) = Topology::new(rings).ring_side(inst.number)?;
        let start = range.start;
        let p0 = hexes[start as usize].center;
        let map_center = self.gadget.center();
        // Run along the side from the first two tiles (a clean side direction); a
        // single-tile side (ring 1) falls back to the radial's tangent.
        let raw_t = if k >= 2 {
            hexes[(start + 1) as usize].center - p0
        } else {
            let r = p0 - map_center;
            Vec3::new(-r.z, 0.0, r.x)
        };
        let t = Vec3::new(raw_t.x, 0.0, raw_t.z).normalize_or_zero();
        // Outward normal ⊥ t, flipped to point away from the map centre.
        let mut n = Vec3::new(t.z, 0.0, -t.x);
        if n.dot(p0 - map_center) < 0.0 {
            n = -n;
        }
        Some((start, p0, t, n, k))
    }

    /// Relaid centre of a fence tile: in fence view the `k` tiles of a ring side
    /// are spread evenly along the side line (`p0 + t · j · HEX_SPACING`) so they
    /// are collinear — and so, tilted, exactly coplanar (the corner tile no
    /// longer bends off the facet). The tile's own centre otherwise.
    pub fn tile_fence_center(
        &self,
        inst: &HexInst,
        hexes: &[HexInst],
        rings: usize,
        fence: bool,
    ) -> Vec3 {
        match self.fence_frame(inst, hexes, rings, fence) {
            Some((start, p0, t, _, _)) => p0 + t * ((inst.number - start) as f32 * HEX_SPACING),
            None => inst.center,
        }
    }

    /// Per-tile orientation. Flat (identity) normally; in fence view a tile tilts
    /// up by its ring's even subdivision of the quarter-turn — ring `k` of
    /// `rings` stands at `k·(90°/rings)`, so the centre stays flat, each inner
    /// ring tilts a little more, and the equator is fully vertical (the fences).
    /// A Z-rotation tilts the hex (a point swinging toward +Y); a Y-yaw turns the
    /// face outward along the side normal `n`. Shared across the side, so the
    /// facet is aligned; the map roll carries north up and the flipped south down.
    pub fn tile_tilt(&self, inst: &HexInst, hexes: &[HexInst], rings: usize, fence: bool) -> Mat4 {
        match self.fence_frame(inst, hexes, rings, fence) {
            Some((_, _, _, n, k)) => {
                let theta = ring_dome_angle(k, rings);
                let n_az = n.z.atan2(n.x);
                Mat4::from_rotation_y(std::f32::consts::PI - n_az) * Mat4::from_rotation_z(theta)
            }
            None => Mat4::IDENTITY,
        }
    }
}

/// Draw one tile: its translucent fill (its world-gen `terrain` top layer when
/// generated, else the grey fallback face, brighter `HOVER_TINT` when `lit`)
/// then its coloured wireframe + labels, at `center` with per-tile `tilt`
/// (fence stand-up; identity for a flat tile) under the map `xform`. A free
/// primitive so both the maps and the snap-map segment draw tiles the same way.
#[allow(clippy::too_many_arguments)]
pub fn draw_tile(
    renderer: &mut Renderer,
    assets: TileAssets,
    terrain: Option<&TileTerrain>,
    center: Vec3,
    tilt: Mat4,
    xform: &Mat4,
    flip: bool,
    number: u32,
    lit: bool,
) {
    if let Some(t) = terrain {
        // The world-gen top layer — the Epoch-6 ground relief with its sea —
        // rides the tile's placement (centre, fence tilt, then map roll).
        let model = *xform * Mat4::from_translation(center) * tilt;
        let tint = if lit { TERRAIN_HOVER_TINT } else { [1.0, 1.0, 1.0, 1.0] };
        renderer.draw_mesh(t.ground, model, MeshDrawOptions { wireframe: false, tint, ..Default::default() });
        if let Some(w) = t.water {
            renderer.draw_mesh(w, model, MeshDrawOptions { wireframe: false, tint, ..Default::default() });
        }
    } else if let Some(mesh) = assets.fill_mesh {
        // Fallback when the vocabulary failed to load: the plain grey fill.
        let model = *xform
            * Mat4::from_translation(center)
            * tilt
            * Mat4::from_translation(Vec3::new(0.0, -FILL_INSET, 0.0));
        let tint = if lit { HOVER_TINT } else { FILL_TINT };
        renderer.draw_mesh(mesh, model, MeshDrawOptions { wireframe: false, tint, ..Default::default() });
    }
    draw_hex(renderer, assets, center, number, flip, tilt, xform);
}

/// Draw the hex wireframe + labels at `center`, with per-tile `tilt` (fence
/// stand-up; identity when flat) under the map roll `xform`. A folded-in
/// neighbour passes identity and its own number.
pub fn draw_hex(
    renderer: &mut Renderer,
    assets: TileAssets,
    center: Vec3,
    number: u32,
    flip: bool,
    tilt: Mat4,
    xform: &Mat4,
) {
    let mut corners = hex_corners(center, HEX_SIZE);
    if flip {
        // Record-flip about the N-S axis through the centre: mirror west↔east.
        // b/e stay at N/S; the a/f and c/d edges swap sides and the colour
        // arrangement reverses left-to-right.
        for c in corners.iter_mut() {
            *c = flip_ns(*c, center);
        }
    }
    // Stand the tile up by `tilt` about its centre (fence view), then apply the
    // map's roll `xform`.
    let place = |p: Vec3| xform.transform_point3(center + tilt.transform_vector3(p - center));
    for i in 0..6 {
        let a = place(corners[i]);
        let b = place(corners[(i + 1) % 6]);
        let color = EDGE_COLORS[i];
        renderer.draw_lines(&[(a, b)], color);

        if let Some(glyphs) = assets.glyphs {
            // Nudge the letter outward from the centre (in the flat frame,
            // correct mirrored too) and lift it off the face, then tilt+roll it
            // with the hex.
            let mid = (corners[i] + corners[(i + 1) % 6]) * 0.5;
            let outward = Vec3::new(mid.x - center.x, 0.0, mid.z - center.z).normalize_or_zero();
            let pos = place(mid + outward * EDGE_LABEL_OUT + Vec3::new(0.0, EDGE_LABEL_SIZE * 0.5, 0.0));
            let letter = (b'a' + i as u8) as char;
            draw_text_billboard(renderer, glyphs, pos, &letter.to_string(), EDGE_LABEL_SIZE, color);
        }
    }
    if let Some(dot) = assets.dot {
        for c in corners {
            renderer.draw_billboard(
                dot,
                place(c),
                Vec2::splat(DOT_SIZE),
                Vec2::ZERO,
                Vec2::ONE,
                [1.0, 0.85, 0.25, 1.0],
            );
        }
    }
    if let Some(glyphs) = assets.glyphs {
        let label = place(center + Vec3::new(0.0, LABEL_SIZE * 0.5 + 40.0, 0.0));
        draw_text_billboard(renderer, glyphs, label, &number.to_string(), LABEL_SIZE, [1.0, 0.95, 0.6, 1.0]);
    }
}
