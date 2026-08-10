//! Where a tile's pixels actually are.
//!
//! A tile is a square of 2048² pixels, and a cell is a hexagon — so a tile has
//! corners that belong to nobody. The [`HexMask`] says which pixels are inside,
//! and the [`TileFrame`] says where on the sphere each one sits.
//!
//! The frame is deliberately **derived from the cell alone** — its centre and its
//! own first corner — so that materialising a tile needs nothing but the cell, and
//! two runs of the same world put every pixel in exactly the same place.

use glam::{DVec2, DVec3, Vec3};

/// Pixels across a tile. A hex *is* a 2048² map; this is not a quality setting.
pub const TILE_DIM: u32 = 2048;

/// Pixels in a whole tile, corners included.
pub const PIXELS_PER_TILE: usize = (TILE_DIM as usize) * (TILE_DIM as usize);

/// How far a tile spans, m — 2048 pixels at 128 ft each, the 49.65 mi a hex
/// measures flat to flat **at every frequency**. Re-exported from the world
/// constants so the pixel tier and the chemistry tier cannot drift apart; the
/// scale-chain guard in `lib.rs` pins this crate's own derivation to it.
pub use flicker_poc_chemistry::TILE_SPAN_M;

/// A cell's own frame: where its tile sits on the sphere and which way is up in it.
///
/// A gnomonic (tangent-plane) placement, which over 80 km on a 6,371 km sphere
/// distorts by parts in ten thousand — far below one pixel. It is not used to
/// decide what the ground does, only where a pixel is; what the ground does is a
/// function of that position, so the seam is continuous regardless.
#[derive(Clone, Debug)]
pub struct TileFrame {
    /// Cell index this tile belongs to.
    pub cell: u32,
    /// Unit direction of the cell centre.
    pub centre: DVec3,
    /// In-plane basis. `east` points at the cell's first corner, which makes the
    /// frame a property of the cell rather than of whoever asked for it.
    pub east: DVec3,
    pub north: DVec3,
    /// Planet radius, m — turns in-plane metres into an angle.
    pub radius_m: f64,
}

impl TileFrame {
    /// Build a cell's frame from its centre direction and its boundary corners.
    pub fn new(cell: u32, centre: Vec3, outline: &[Vec3], radius_m: f64) -> Self {
        let centre = centre.as_dvec3().normalize();
        // Toward the first corner, projected into the tangent plane. A cell with no
        // outline (it should not happen) still gets a stable, arbitrary frame rather
        // than a NaN one.
        let toward = outline
            .first()
            .map(|c| c.as_dvec3().normalize())
            .unwrap_or(if centre.x.abs() < 0.9 { DVec3::X } else { DVec3::Y });
        let east = (toward - centre * centre.dot(toward)).normalize_or_zero();
        let east = if east.length_squared() > 0.5 { east } else { centre.any_orthonormal_vector() };
        let north = centre.cross(east);
        Self { cell, centre, east, north, radius_m }
    }

    /// In-plane metres of a pixel's centre, measured from the middle of the tile.
    /// Pixel centres, not corners — so the map is symmetric about the cell.
    pub fn plane_m(&self, x: u32, y: u32) -> DVec2 {
        let half = TILE_DIM as f64 * 0.5;
        let per_px = TILE_SPAN_M / TILE_DIM as f64;
        DVec2::new(
            (x as f64 + 0.5 - half) * per_px,
            (y as f64 + 0.5 - half) * per_px,
        )
    }

    /// Where a pixel sits on the unit sphere.
    pub fn direction(&self, x: u32, y: u32) -> DVec3 {
        let p = self.plane_m(x, y) / self.radius_m;
        (self.centre + self.east * p.x + self.north * p.y).normalize()
    }

    /// Area of one pixel, m². Every pixel is the same size in the tangent plane,
    /// which is what makes a pixel a cluster-sized piece of ground anywhere.
    pub fn pixel_area_m2(&self) -> f64 {
        let per_px = TILE_SPAN_M / TILE_DIM as f64;
        per_px * per_px
    }
}

/// Which pixels of a tile are inside the cell.
///
/// The hexagon is taken from the cell's real boundary projected into its own
/// frame, so a pentagon masks correctly too — it is simply a five-sided polygon,
/// and nothing here counts sides.
#[derive(Clone, Debug)]
pub struct HexMask {
    inside: Vec<bool>,
    count: usize,
}

impl HexMask {
    /// Mask a cell's tile from its boundary polygon.
    pub fn new(frame: &TileFrame, outline: &[Vec3]) -> Self {
        // The boundary in the tile's own plane, in metres.
        let poly: Vec<DVec2> = outline
            .iter()
            .map(|c| {
                let d = c.as_dvec3().normalize();
                // Gnomonic: scale onto the tangent plane at the centre.
                let scale = frame.radius_m / d.dot(frame.centre).max(1e-9);
                DVec2::new(d.dot(frame.east) * scale, d.dot(frame.north) * scale)
            })
            .collect();

        let mut inside = vec![false; PIXELS_PER_TILE];
        let mut count = 0usize;
        if poly.len() >= 3 {
            for y in 0..TILE_DIM {
                for x in 0..TILE_DIM {
                    if point_in_polygon(frame.plane_m(x, y), &poly) {
                        inside[(y as usize) * TILE_DIM as usize + x as usize] = true;
                        count += 1;
                    }
                }
            }
        }
        Self { inside, count }
    }

    /// Whether this pixel belongs to the cell.
    pub fn contains(&self, x: u32, y: u32) -> bool {
        self.inside[(y as usize) * TILE_DIM as usize + x as usize]
    }

    /// How many pixels the cell owns — about 3.8M of the tile's 4.19M.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Walk every pixel inside the cell.
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..TILE_DIM).flat_map(move |y| {
            (0..TILE_DIM).filter_map(move |x| self.contains(x, y).then_some((x, y)))
        })
    }
}

/// Even-odd crossing test — the standard one, and enough for a convex cell
/// boundary.
fn point_in_polygon(p: DVec2, poly: &[DVec2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let straddles = (a.y > p.y) != (b.y > p.y);
        if straddles {
            let t = (p.y - a.y) / (b.y - a.y);
            if p.x < a.x + t * (b.x - a.x) {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldgrid::icosphere_with_outlines;

    fn frame_for(cell: usize) -> (TileFrame, Vec<Vec3>) {
        // The radius the grid implies — a coarse grid is a SMALLER planet, because
        // the tile span is fixed. Using Earth's radius here would put 50-mile tiles
        // on 300-mile cells and every pixel would fall inside its own hexagon.
        let (sphere, outlines) = icosphere_with_outlines(16);
        let outline = outlines[cell].clone();
        let r = crate::radius_for_freq(16);
        let frame = TileFrame::new(cell as u32, sphere.dirs[cell], &outline, r);
        (frame, outline)
    }

    /// The scale chain closes here and nowhere else: 2048 pixels at 128 ft is the
    /// 49.65 mi a hex measures across.
    #[test]
    fn a_tile_spans_a_hex() {
        let miles = TILE_SPAN_M / 1609.344;
        assert!((miles - 49.65).abs() < 0.01, "a tile spans {miles:.2} mi");
    }

    /// A cell owns most of its tile but not the corners — a hexagon inscribed in a
    /// square keeps about six-sevenths of it.
    #[test]
    fn the_corners_belong_to_nobody() {
        let (frame, outline) = frame_for(400);
        let mask = HexMask::new(&frame, &outline);
        let share = mask.count() as f64 / PIXELS_PER_TILE as f64;
        assert!(
            (0.5..0.95).contains(&share),
            "a hex should keep most of its tile, got {share:.3}"
        );
        // The very middle is always inside; the very corner never is.
        assert!(mask.contains(TILE_DIM / 2, TILE_DIM / 2));
        assert!(!mask.contains(0, 0));
    }

    /// Pixels land where they should: the centre pixel is the cell centre, and a
    /// pixel one span away has moved about one span.
    #[test]
    fn pixels_sit_where_the_frame_says() {
        let (frame, _) = frame_for(400);
        // The middle pixel is half a pixel off centre, because a pixel is a place
        // with a width and the map is symmetric about the cell.
        let mid = frame.direction(TILE_DIM / 2, TILE_DIM / 2);
        let off_m = mid.distance(frame.centre) * frame.radius_m;
        let per_px = TILE_SPAN_M / TILE_DIM as f64;
        assert!(off_m < per_px, "the middle pixel is within a pixel of the cell centre");
        let edge = frame.direction(TILE_DIM - 1, TILE_DIM / 2);
        let moved = edge.distance(frame.centre) * frame.radius_m;
        assert!(
            (moved - TILE_SPAN_M / 2.0).abs() < TILE_SPAN_M * 0.01,
            "half a span away, got {moved:.0} m of {:.0} m",
            TILE_SPAN_M / 2.0
        );
    }

    /// A pentagon is not a special case — it masks like anything else, because
    /// nothing here counts sides.
    #[test]
    fn a_pentagon_masks_like_any_other_cell() {
        let (sphere, outlines) = icosphere_with_outlines(16);
        let pent = (0..sphere.len()).find(|&i| sphere.is_pentagon[i]).expect("twelve of them");
        assert_eq!(outlines[pent].len(), 5);
        let r = crate::radius_for_freq(16);
        let frame = TileFrame::new(pent as u32, sphere.dirs[pent], &outlines[pent], r);
        let mask = HexMask::new(&frame, &outlines[pent]);
        assert!(mask.count() > 0, "a pentagon owns pixels too");
        assert!(mask.contains(TILE_DIM / 2, TILE_DIM / 2));
    }
}
