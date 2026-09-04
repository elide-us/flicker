//! **The gameplay bake** — a region of a committed planet, materialised into
//! the world cluster map.
//!
//! Input: a [`PlanetSource`] (the Populous bench's `.epoch`, stood back up)
//! and a set of REGION hexes. Output: atlas-space map planes — one `f32`
//! height and one `u8` material per cluster column — over the atlas
//! rectangle those hexes cover, addressed by [`WorldClusterId`] coordinates.
//! This is the LOD-8 heightmap-dot tier of the sparse ladder: one vector per
//! cluster, for every cluster in the region, and the source every finer rung
//! samples.
//!
//! # The same two guarantees as the T6 tiles, in atlas space
//!
//! **Continuity**: a pixel's ground is the global relief field sampled at
//! the pixel's own position on the sphere ([`crate::shape`]) — two pixels on
//! either side of a hex-ownership boundary ask the same function, so the
//! ground cannot step there. **Conservation** (gameplay-volume form): over
//! the pixels a region hex OWNS, heights sum to exactly the hex's ledger
//! thickness × its owned-pixel count — the residual against the raw field is
//! absorbed only in the hex's interior, zero at its rim, exactly as the T6
//! migration does it. Pixels owned by hexes OUTSIDE the region (the rect's
//! margins) carry the raw field for continuity but no conservation claim —
//! the bake's authority ends at its region.
//!
//! Deterministic: same epoch, same region, same planes.

use std::collections::HashMap;

use glam::DVec3;

use crate::atlas::{AtlasFrame, CellIndex};
use crate::shape::Neighbourhood;
use crate::source::{PlanetSource, TileSource};

/// One baked region: atlas-rect planes plus the per-hex audit.
pub struct RegionBake {
    /// The frame the coordinates live in.
    pub frame: AtlasFrame,
    /// The rect's origin column (wraps toroidally) and top row.
    pub x0: u32,
    pub z0: u32,
    /// Rect extent, clusters.
    pub width: u32,
    pub height: u32,
    /// Ground height per cluster column, metres — row-major over the rect,
    /// `height[r * width + c]` at atlas `(wrap_x(x0 + c), z0 + r)`.
    pub heights: Vec<f32>,
    /// Exposed material per cluster column (the source's material codes;
    /// `0` = no solid ground).
    pub materials: Vec<u8>,
    /// The region hexes and, for each, (owned-pixel count, conserved target
    /// thickness_m) — the audit trail the conservation gate checks.
    pub hexes: Vec<(u32, u64, f64)>,
    /// Sea level, metres — solved once from the epoch's conserved water.
    pub sea_level_m: f64,
}

/// The region hexes within `rings` neighbour steps of `center`.
pub fn region_rings(src: &PlanetSource, center: u32, rings: u32) -> Vec<u32> {
    let grid = src.grid();
    let mut seen = vec![false; grid.len()];
    let mut out = vec![center];
    seen[center as usize] = true;
    let mut edge = vec![center];
    for _ in 0..rings {
        let mut next = Vec::new();
        for &c in &edge {
            for &n in &grid.neighbors[c as usize] {
                if !seen[n as usize] {
                    seen[n as usize] = true;
                    out.push(n);
                    next.push(n);
                }
            }
        }
        edge = next;
    }
    out
}

/// Bake `region` (hex ids) of the planet into atlas planes.
pub fn bake_region(src: &PlanetSource, frame: &AtlasFrame, region: &[u32]) -> RegionBake {
    let grid = src.grid();
    let index = CellIndex::new(grid);

    // ── The rect: every atlas cell whose centre could be owned by a region
    // hex — the hexes' bounding box plus one hex of margin for the rims. ──
    let (x0, z0, width, height) = rect_for(frame, grid, region);
    let n = width as usize * height as usize;

    // ── Ownership + raw field, one pass. Neighbourhoods are per-owner and
    // cached — the expensive gather runs once per hex, not per pixel. ──
    let mut owner = vec![u32::MAX; n];
    let mut heights = vec![0.0f32; n];
    let mut hoods: HashMap<u32, Neighbourhood> = HashMap::new();
    for r in 0..height {
        for c in 0..width {
            let (ax, az) = (frame.wrap_x(x0 as i64 + c as i64), z0 + r);
            let d: DVec3 = frame.dir(ax, az);
            let own = index.owner(grid, d);
            let i = r as usize * width as usize + c as usize;
            owner[i] = own;
            let hood = hoods
                .entry(own)
                .or_insert_with(|| Neighbourhood::around(src, own as usize));
            heights[i] = hood.relief_at(d).max(0.0) as f32;
        }
    }

    // ── Conservation, per REGION hex: absorb the residual between the raw
    // field and the ledger's thickness over the hex's owned pixels, weighted
    // toward the interior so the rim stays exactly the field (the T6 rule,
    // in atlas space). ──
    let mut hexes = Vec::with_capacity(region.len());
    for &cell in region {
        let owned: Vec<usize> = (0..n).filter(|&i| owner[i] == cell).collect();
        if owned.is_empty() {
            hexes.push((cell, 0, src.thickness_m(cell as usize)));
            continue;
        }
        // Interior weight: how deep inside the ownership patch a pixel sits
        // (0 on the patch's rim), by the same cheap ring probe as T6.
        let interior: Vec<f64> = owned
            .iter()
            .map(|&i| interior_weight(&owner, cell, i, width as usize, height as usize))
            .collect();
        let want = src.thickness_m(cell as usize);
        let laid: f64 = owned.iter().map(|&i| f64::from(heights[i])).sum();
        let target = want * owned.len() as f64;
        let interior_sum: f64 = interior.iter().sum();
        if interior_sum > 0.0 {
            let per_unit = (target - laid) / interior_sum;
            for (k, &i) in owned.iter().enumerate() {
                heights[i] = (f64::from(heights[i]) + per_unit * interior[k]).max(0.0) as f32;
            }
        }
        hexes.push((cell, owned.len() as u64, want));
    }

    // ── The material plane: the exposed (top) bed of the owning hex. The
    // conformable drape keeps bed proportions uniform within a hex, so what
    // is exposed is the hex's top bed everywhere it has ground; per-pixel
    // material variation is erosion's to earn later. ──
    let mut top_mat: HashMap<u32, u8> = HashMap::new();
    let mut materials = vec![0u8; n];
    for i in 0..n {
        if heights[i] <= 0.0 {
            continue;
        }
        let own = owner[i];
        let m = *top_mat.entry(own).or_insert_with(|| {
            src.beds_m(own as usize)
                .last()
                .map(|b| b.material)
                .unwrap_or(0)
        });
        materials[i] = m;
    }

    RegionBake {
        frame: frame.clone(),
        x0,
        z0,
        width,
        height,
        heights,
        materials,
        hexes,
        sea_level_m: src.sea_level_m(),
    }
}

/// The atlas rect covering `region`'s hexes plus a one-hex margin, clamped
/// to the frame vertically; the x span wraps.
fn rect_for(
    frame: &AtlasFrame,
    grid: &flicker_worldgrid::Sphere,
    region: &[u32],
) -> (u32, u32, u32, u32) {
    // A hex spans TILE_DIM clusters; half of that plus slack is margin
    // enough on every side of the centres' bounding box.
    let margin = (crate::mask::TILE_DIM / 2 + crate::mask::TILE_DIM / 8) as i64;
    let (mut lon_min, mut lon_max) = (f64::MAX, f64::MIN);
    let (mut z_min, mut z_max) = (i64::MAX, i64::MIN);
    let lat_top = (90.0 - frame.trim_deg).to_radians();
    let lat_span = (180.0 - 2.0 * frame.trim_deg).to_radians();
    for &c in region {
        let d = grid.dirs[c as usize].as_dvec3().normalize();
        // Longitude relative to the FIRST hex, so a region straddling the
        // wrap seam stays a contiguous span instead of the whole equator.
        let lon = d.z.atan2(d.x);
        let base = grid.dirs[region[0] as usize].as_dvec3().normalize();
        let base_lon = base.z.atan2(base.x);
        let rel = (lon - base_lon + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        lon_min = lon_min.min(rel);
        lon_max = lon_max.max(rel);
        let lat = d.y.clamp(-1.0, 1.0).asin();
        let z = ((lat_top - lat) / lat_span * frame.height as f64) as i64;
        z_min = z_min.min(z);
        z_max = z_max.max(z);
    }
    let base = grid.dirs[region[0] as usize].as_dvec3().normalize();
    let base_lon = base.z.atan2(base.x).rem_euclid(std::f64::consts::TAU);
    let base_x = (base_lon / std::f64::consts::TAU * frame.width as f64) as i64;
    let lon_to_cols = |l: f64| (l / std::f64::consts::TAU * frame.width as f64) as i64;
    let x_lo = base_x + lon_to_cols(lon_min) - margin;
    let x_hi = base_x + lon_to_cols(lon_max) + margin;
    let z_lo = (z_min - margin).max(0);
    let z_hi = (z_max + margin).min(frame.height as i64 - 1);
    let width = (x_hi - x_lo + 1).min(frame.width as i64) as u32;
    (
        frame.wrap_x(x_lo),
        z_lo as u32,
        width,
        (z_hi - z_lo + 1) as u32,
    )
}

/// How deep inside its hex's ownership patch a pixel sits — 0 when any probe
/// within `REACH` leaves the patch (the rim), rising toward the interior.
/// The T6 boundary-distance rule, over ownership instead of a hex mask.
fn interior_weight(owner: &[u32], cell: u32, i: usize, width: usize, height: usize) -> f64 {
    const REACH: usize = 3;
    let (c, r) = (i % width, i / width);
    if c < REACH || r < REACH || c + REACH >= width || r + REACH >= height {
        return 0.0;
    }
    for step in 1..=REACH {
        if owner[i - step] != cell
            || owner[i + step] != cell
            || owner[i - step * width] != cell
            || owner[i + step * width] != cell
        {
            return (step - 1) as f64;
        }
    }
    REACH as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldengine::{PlanetEpoch, PlanetEra, PlanetLedger, PlanetRecipe};

    /// A tiny synthetic planet epoch: freq 1 (12 hexes), a distinct stack
    /// per cell so ownership and conservation are distinguishable.
    fn tiny_epoch() -> PlanetEpoch {
        let freq = 1;
        let tiles = flicker_worldgrid::cell_count(freq);
        let mut ledger = PlanetLedger {
            base: vec![0.3; tiles],
            l3_h: vec![0.0; tiles],
            l3_hard: vec![1.0; tiles],
            l4_h: vec![0.0; tiles],
            l4_hard: vec![1.0; tiles],
            strike: vec![0.0; tiles],
            l3_dip: vec![0.0; tiles],
            l4_dip: vec![0.0; tiles],
            graded: vec![0.0; tiles],
            dissolved: vec![0.0; tiles],
            rock: vec![0.05; tiles],
            rock_hard: vec![1.0; tiles],
            sediment: vec![0.02; tiles],
            bed_hard: vec![1.0; tiles],
            pressure: vec![0.0; tiles],
            edge: vec![0.0; tiles],
            edge_age: vec![0; tiles],
            drift: vec![0.0; tiles],
            suspend: vec![0.0; tiles],
            ice: vec![0.0; tiles],
            sst: vec![0.5; tiles],
            discharge: vec![0.0; tiles],
            moist: vec![0.0; tiles],
            rain: vec![0.0; tiles],
            veg: vec![0.0; tiles],
            vein: vec![0; tiles],
            vein_node_of: vec![0; tiles],
        };
        // One tall cell, one vein cell — features to see in the maps.
        ledger.base[3] = 1.1;
        ledger.l3_h[5] = 0.2;
        ledger.vein[5] = 2;
        let era = PlanetEra {
            ticks: 100,
            eruptions: 0,
            steps: 0,
            heals: 0,
            water_volume: 0.5,
            ice_locked: 0.0,
            climate_base: 0.5,
            temp: 0.5,
            deep_temp: 0.35,
            greenhouse: 0.0,
            water_target: 0.5,
            veg_target: 0.3,
            veg_thirst: 1.0,
            green_share: 0.0,
            resources_ensured: false,
            // The 0a well ledger — a restored planet without it resumes
            // minting; a fixture without it does not compile.
            well: 0.0,
            sunk: 0.0,
            delaminations: 0,
        };
        PlanetEpoch::new(
            PlanetRecipe {
                freq,
                seed: 9,
                cells: 3,
                spots: 1,
            },
            era,
            ledger,
            vec![vec![0.0; tiles]; 3],
            Vec::new(),
            Vec::new(),
            "bake test planet",
        )
    }

    /// **The gameplay-volume trial balance, and continuity at the rim.**
    /// A one-hex region bake holds exactly the hex's ledger thickness over
    /// its owned pixels, and its rim pixels stand exactly on the global
    /// field — the two T6 guarantees, in atlas space.
    #[test]
    fn a_region_bake_conserves_and_keeps_its_rim_on_the_field() {
        let src = PlanetSource::new(tiny_epoch(), 2_000.0);
        let frame = AtlasFrame::new(src.epoch().recipe.freq, 10.0);
        let cell = 3u32; // the tall one
        let bake = bake_region(&src, &frame, &[cell]);

        let (_, owned, want) = bake.hexes[0];
        assert!(owned > 0, "the hex owns pixels in its own rect");
        let n = bake.width as usize * bake.height as usize;
        assert_eq!(bake.heights.len(), n);

        // Conservation: Σ heights over owned pixels = want × owned, to f32
        // accumulation tolerance.
        let index = CellIndex::new(src.grid());
        let mut sum = 0.0f64;
        let mut rim_checked = 0;
        let hood = Neighbourhood::around(&src, cell as usize);
        for r in 0..bake.height {
            for c in 0..bake.width {
                let i = r as usize * bake.width as usize + c as usize;
                let (ax, az) = (bake.frame.wrap_x(bake.x0 as i64 + c as i64), bake.z0 + r);
                let d = bake.frame.dir(ax, az);
                if index.owner(src.grid(), d) != cell {
                    continue;
                }
                sum += f64::from(bake.heights[i]);
                // A rim pixel (an ownership neighbour differs) must stand on
                // the raw field — the absorption never moves an edge.
                let on_rim = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)].iter().any(
                    |&(dc, dr)| {
                        let (nc, nr) = (c as i64 + dc, r as i64 + dr);
                        if nc < 0
                            || nr < 0
                            || nc >= bake.width as i64
                            || nr >= bake.height as i64
                        {
                            return true;
                        }
                        let nd = bake
                            .frame
                            .dir(bake.frame.wrap_x(bake.x0 as i64 + nc), bake.z0 + nr as u32);
                        index.owner(src.grid(), nd) != cell
                    },
                );
                if on_rim && rim_checked < 200 {
                    rim_checked += 1;
                    let field = hood.relief_at(d).max(0.0);
                    assert!(
                        (f64::from(bake.heights[i]) - field).abs() <= 1e-3 * field.max(1.0),
                        "a rim pixel moved off the field"
                    );
                }
            }
        }
        assert!(rim_checked > 0, "the hex has a rim");
        let target = want * owned as f64;
        assert!(
            (sum - target).abs() <= 1e-4 * target.max(1.0),
            "conservation: laid {sum} vs target {target}"
        );

        // The material plane exposes the hex's top bed (sediment) on ground.
        let some_owned = (0..n).find(|&i| bake.heights[i] > 0.0).expect("ground");
        assert_eq!(bake.materials[some_owned], crate::source::MAT_SEDIMENT);
    }

    /// The vein hex's exposed stratum would surface its ore code if the
    /// stratum were on top; with sediment above it stays buried — the
    /// material plane reads the TOP bed, never a middle one.
    #[test]
    fn the_material_plane_reads_the_top_bed() {
        let mut epoch = tiny_epoch();
        // Strip the loose cover on the vein cell: the stratum IS the top.
        epoch.ledger.rock[5] = 0.0;
        epoch.ledger.sediment[5] = 0.0;
        let src = PlanetSource::new(epoch, 2_000.0);
        let frame = AtlasFrame::new(src.epoch().recipe.freq, 10.0);
        let bake = bake_region(&src, &frame, &[5]);
        let n = bake.width as usize * bake.height as usize;
        let index = CellIndex::new(src.grid());
        let exposed = (0..n).find(|&i| {
            let (c, r) = (i % bake.width as usize, i / bake.width as usize);
            let d = bake
                .frame
                .dir(bake.frame.wrap_x(bake.x0 as i64 + c as i64), bake.z0 + r as u32);
            bake.heights[i] > 0.0 && index.owner(src.grid(), d) == 5
        });
        let i = exposed.expect("the vein hex has ground");
        assert_eq!(
            bake.materials[i],
            crate::source::MAT_VEIN_BASE + 1,
            "vein kind 2 surfaces as its own material code"
        );
    }

    /// The sea solve is monotone sane: more water, higher level; no water,
    /// level zero.
    #[test]
    fn the_sea_level_follows_the_water()  {
        let dry = {
            let mut e = tiny_epoch();
            e.era.water_volume = 0.0;
            PlanetSource::new(e, 2_000.0).sea_level_m()
        };
        let low = PlanetSource::new(tiny_epoch(), 2_000.0).sea_level_m();
        let high = {
            let mut e = tiny_epoch();
            e.era.water_volume = 2.0;
            PlanetSource::new(e, 2_000.0).sea_level_m()
        };
        assert_eq!(dry, 0.0);
        assert!(low > 0.0 && high > low, "sea rises with the volume: {low} vs {high}");
    }
}
