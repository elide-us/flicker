//! **The tile as a field of vectors** — the 2K hex handed to the engine as
//! the LOD8 cluster field it is: one vector per pixel of the mask, at the
//! pixel's composite (the cluster's LOD8 vector — `cluster_lod(…, 8).base_m`
//! is the same number), wearing the tile's own picture, the surface
//! contoured between the vectors the engine's to draw
//! ([`flicker_voxel::QuadField::surface_meshes`]).
//!
//! No calculation lives here that the tile does not already hold (the rule:
//! the tile is the same algorithm): the field is a re-reading of the maps
//! in the engine's units, nothing more.

use crate::mask::TILE_DIM;
use crate::materialize::Tile;
use flicker_voxel::QuadField;

/// The tile's quads: every pixel of the mask at its composite height, in
/// the engine's voxels, coloured by `picture` — the tile's own RGBA at the
/// map's full 2K (`Tile::shaded_rgba(1, …)`, `TILE_DIM²` pixels), the pixel
/// of the quad. Row-major, the mask's order.
pub fn quad_field(tile: &Tile, picture: &[u8]) -> QuadField {
    let dim = TILE_DIM as usize;
    assert_eq!(
        picture.len(),
        dim * dim * 4,
        "the field wears the 2K picture, one pixel a quad"
    );
    QuadField::build(
        TILE_DIM,
        |x, y| tile.mask.contains(x, y),
        |x, y| tile.composite_m(x, y),
        |x, y| {
            let o = ((y as usize) * dim + x as usize) * 4;
            [picture[o], picture[o + 1], picture[o + 2]]
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::{HexMask, TileFrame, PIXELS_PER_TILE, TILE_SPAN_M};
    use glam::Vec3;

    /// A tile whose hexagon is SMALL — fifty pixels to a corner — so the
    /// gate walks every quad in a moment: the cell's frame and mask built
    /// as the migration builds them, one stratum shaped by a roll and a
    /// step, the rest of the tile outside the mask.
    fn small_tile() -> Tile {
        let radius_m = 6.371e6f64;
        let per_px = TILE_SPAN_M / f64::from(TILE_DIM);
        let rho = (50.0 * per_px / radius_m).atan();
        let centre = Vec3::Z;
        let outline: Vec<Vec3> = (0..6)
            .map(|k| {
                let th = k as f64 * std::f64::consts::TAU / 6.0;
                Vec3::new(
                    (rho.sin() * th.cos()) as f32,
                    (rho.sin() * th.sin()) as f32,
                    rho.cos() as f32,
                )
            })
            .collect();
        let frame = TileFrame::new(7, centre, &outline, radius_m);
        let mask = HexMask::new(&frame, &outline);
        let mut bed = vec![0.0f32; PIXELS_PER_TILE];
        for (x, y) in mask.iter() {
            let h = 1200.0 + (x as f32 * 0.31).sin() * 4.0 + f32::from((y % 7 == 0) as u8) * 2.5;
            bed[(y as usize) * TILE_DIM as usize + x as usize] = h;
        }
        Tile {
            cell: 7,
            frame,
            mask,
            strata: vec![bed],
            materials: vec![1],
            skirt: Default::default(),
            pixel_born: 0,
        }
    }

    /// **The tile is its mask in vectors**: one vector a mask pixel, each at
    /// its pixel's composite in voxels (the LOD8 vector), wearing its pixel
    /// of the picture; the surface spans every 2×2 block of them, one vertex
    /// a cluster at its vector, in world voxels.
    #[test]
    fn a_tile_is_its_mask_in_vectors_at_the_pixels_composite() {
        let tile = small_tile();
        let (picture, dim) = tile.shaded_rgba(1, 1000.0, 2000.0);
        assert_eq!(dim, TILE_DIM);
        let field = quad_field(&tile, &picture);
        assert_eq!(field.quads.len(), tile.mask.count());
        assert!(tile.mask.count() > 5000 && tile.mask.count() < 9000);
        let per_voxel = flicker_voxel::voxel_span_m() as f32;
        for q in &field.quads {
            assert!(tile.mask.contains(q.x, q.y));
            assert_eq!(q.height, tile.composite_m(q.x, q.y) / per_voxel);
            let o = ((q.y as usize) * TILE_DIM as usize + q.x as usize) * 4;
            assert_eq!(q.rgb, [picture[o], picture[o + 1], picture[o + 2]]);
        }
        // The surface: a quad across every 2×2 block of the mask (the rim
        // costs a row and a column), six indices a quad, in world voxels.
        let regions = field.surface_meshes(256);
        let quads: usize = regions.iter().map(|r| r.quads).sum();
        assert!(
            quads > field.quads.len() * 9 / 10 && quads < field.quads.len(),
            "{quads} surface quads over {} vectors",
            field.quads.len()
        );
        assert_eq!(
            regions.iter().map(|r| r.mesh.indices.len()).sum::<usize>(),
            quads * 6
        );
        let q = field
            .at(TILE_DIM / 2, TILE_DIM / 2)
            .expect("the hex's middle");
        let e = field
            .at(TILE_DIM / 2 + 1, TILE_DIM / 2)
            .expect("its east neighbour");
        let (p, pe) = (field.vertex_position(q), field.vertex_position(e));
        assert_eq!(p[1], q.height, "the vertex stands at the vector");
        assert_eq!(pe[0] - p[0], flicker_voxel::CLUSTER_DIM as f32);
        assert_eq!(pe[2], p[2]);
    }
}
