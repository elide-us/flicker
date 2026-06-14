//! `SnapMapSegment`: the **navigator's horizon panel**. A small, flat hex-flower
//! drawn on its own, above and centred between the two map structures, showing
//! the tiles the engine snaps around the player — with a Logo-style **turtle**
//! triangle marking the player at its centre.
//!
//! This is the data-viz payoff: walk the world and watch which tiles fall into
//! place around you, including across the toroidal seams, to confirm a coherent
//! travel experience. (Slice 7a: the panel + turtle + the radius-1 horizon of
//! the current player tile. Navigate mode and the N-ring horizon land next.)

use flicker::render::{Mat4, Renderer, Vec3};

use crate::map_structure::{draw_hex, TileAssets};
use crate::snap_map::horizon;
use crate::{sep, HexInst, HEX_SIZE, HEX_SPACING};

/// Height above the flat maps the panel floats at.
const PANEL_Y: f32 = 5000.0;
/// Scale the world-size hexes are drawn at in the panel (a small inset map).
const PANEL_SCALE: f32 = 0.55;
/// Only draw tiles whose (scrolled) centre is within this of the turtle, so the
/// panel shows just the tile underfoot — one tile in a tile's interior, two at
/// an edge, three at a join — never the whole rosette. A wider horizon slider
/// will raise this later.
const HORIZON_RADIUS: f32 = HEX_SPACING;
/// Turtle size (a fraction of a hex) and its bright colour.
const TURTLE_SIZE: f32 = HEX_SIZE * 0.62;
const TURTLE_COLOR: [f32; 4] = [0.25, 1.0, 0.85, 1.0];
/// Lift the turtle a hair above the panel hexes so it reads on top.
const TURTLE_LIFT: f32 = 80.0;

/// The horizon panel. Stateless for now — the player tile and (later) the
/// navigate-mode position live on the scene; this draws the panel for them.
pub struct SnapMapSegment;

impl SnapMapSegment {
    /// Draw the panel: the tiles under and around the turtle's sub-tile position
    /// `offset` inside `player_tile`, scrolled so the turtle stays centred and the
    /// world slides beneath it. Centred between the two maps and lifted above
    /// them. Only the tile(s) the turtle is actually on are drawn (see
    /// [`HORIZON_RADIUS`]).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        renderer: &mut Renderer,
        assets: TileAssets,
        hexes: &[HexInst],
        within: &[Vec<u32>],
        rings: usize,
        player_tile: u32,
        offset: Vec3,
        heading: f32,
    ) {
        // Centre between the two map columns (north at x=0, south at x=sep),
        // lifted above the flat maps; the panel's own little hex frame is scaled
        // down into place. The map stays north-up — only the turtle rotates.
        let panel = Vec3::new(sep(rings) * 0.5, PANEL_Y, 0.0);
        let xform = Mat4::from_translation(panel) * Mat4::from_scale(Vec3::splat(PANEL_SCALE));

        // The local patch (centre tile + its neighbours), each scrolled by the
        // turtle's offset so the turtle sits at the panel centre and the world
        // moves under it. Cull to the tile(s) actually underfoot.
        for (number, local) in horizon(hexes, within, rings, player_tile) {
            let scrolled = local - offset;
            if scrolled.length() > HORIZON_RADIUS {
                continue;
            }
            draw_hex(renderer, assets, scrolled, number, false, Mat4::IDENTITY, &xform);
        }
        draw_turtle(renderer, &xform, heading);
    }
}

/// Draw the turtle: a flat triangle at the panel centre, lifted just above the
/// hexes, rotated to its `heading` (0 = north/+Z, +heading toward west). Marks
/// the player and shows which way it faces; the world scrolls under it.
fn draw_turtle(renderer: &mut Renderer, xform: &Mat4, heading: f32) {
    let s = TURTLE_SIZE;
    let y = TURTLE_LIFT;
    let tip = Vec3::new(0.0, y, s); // nose → forward
    let bl = Vec3::new(s * 0.6, y, -s * 0.7);
    let br = Vec3::new(-s * 0.6, y, -s * 0.7);
    // Heading 0 faces +Z; `from_rotation_y(heading)` swings +Z toward +X (west).
    let rot = Mat4::from_rotation_y(heading);
    let t = |p: Vec3| xform.transform_point3(rot.transform_point3(p));
    renderer.draw_lines(
        &[(t(tip), t(bl)), (t(bl), t(br)), (t(br), t(tip))],
        TURTLE_COLOR,
    );
}
