//! **The world cluster map** — the gameplay world's address space.
//!
//! The baked planet is addressed the VoxelFarm way (Aaron 2026-08-28,
//! ratified): one toroidal cluster grid, every cluster named by a single
//! bit-split `u64` — [`WorldClusterId`]. The grid is an equirectangular
//! atlas of the planet: `x` runs around the equator and **wraps** (the
//! toroidal axis), `z` runs pole-ward between the trimmed caps, and one
//! atlas cell is one 128-ft cluster ([`clayengine::cluster_span_m`]).
//! Gameplay space is flat-toroidal like a VoxelFarm world; the projection
//! from the hex sphere happens ONCE, at bake time, and the distortion is
//! absorbed there.
//!
//! # Bit layout (LOCKED — Aaron ratified 2026-08-28)
//!
//! `u64` packed `[LOD: 4][y: 12][x: 24][z: 24]` from high to low:
//!
//! | field | bits | range            | meaning                          |
//! |-------|------|------------------|----------------------------------|
//! | LOD   |  4   | `0..=15` (use 0..=8) | sample stride `2^L` in-cluster |
//! | y     | 12   | `0..=4095`       | vertical cluster (× 128 ft ≈ 99 mi of column) |
//! | x     | 24   | `0..=16_777_215` | equatorial ring — WRAPS          |
//! | z     | 24   | `0..=16_777_215` | pole-ward row                    |
//!
//! LOD keeps the builder ladder's meaning (`0` = full 256³, `8` = the one
//! heightmap-dot vector — [`clayengine::MAX_LOD`]); it is 4 bits wide so the
//! pack stays byte-aligned, with values above 8 unused — the "fewer LOD bits
//! than VoxelFarm" ratification: our per-cluster ladder ends at 8 and no
//! cross-cluster merge levels are defined.
//!
//! # Who wraps
//!
//! The FIELD is storage; the [`AtlasFrame`] is geometry. Wrap arithmetic
//! (`AtlasFrame::wrap_x`) is modular over the frame's actual width — never
//! over the 24-bit field, which is headroom (a freq-96 planet is ~1.03M
//! clusters around; the field holds 16.7M).

use clayengine::cluster_span_m;
use flicker_worldgrid::Sphere;
use glam::DVec3;

/// Cluster address in the world cluster map: `(LOD, y, x, z)` packed into a
/// single `u64`. See the [module docs](self) for the ratified layout.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct WorldClusterId(u64);

impl WorldClusterId {
    /// Largest legal LOD (15 — exhausts the field; only `0..=8` is used).
    pub const MAX_LOD: u8 = 0xF;
    /// Largest legal `y` (4095).
    pub const MAX_Y: u16 = 0xFFF;
    /// Largest legal `x` / `z` (16,777,215).
    pub const MAX_XZ: u32 = 0xFF_FFFF;

    const LOD_SHIFT: u64 = 60;
    const Y_SHIFT: u64 = 48;
    const X_SHIFT: u64 = 24;

    /// Construct from components. Panics if any component exceeds its field
    /// width — an address is either legal or a bug, never quietly masked.
    #[inline]
    #[must_use]
    pub const fn new(lod: u8, y: u16, x: u32, z: u32) -> Self {
        assert!(lod <= Self::MAX_LOD, "WorldClusterId: lod overflows 4 bits");
        assert!(y <= Self::MAX_Y, "WorldClusterId: y overflows 12 bits");
        assert!(x <= Self::MAX_XZ, "WorldClusterId: x overflows 24 bits");
        assert!(z <= Self::MAX_XZ, "WorldClusterId: z overflows 24 bits");
        Self(
            ((lod as u64) << Self::LOD_SHIFT)
                | ((y as u64) << Self::Y_SHIFT)
                | ((x as u64) << Self::X_SHIFT)
                | (z as u64),
        )
    }

    /// The raw packed `u64` — the serialized form.
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Inverse of [`Self::bits`] — for round-tripping known-valid storage.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[inline]
    #[must_use]
    pub const fn lod(self) -> u8 {
        ((self.0 >> Self::LOD_SHIFT) & 0xF) as u8
    }

    #[inline]
    #[must_use]
    pub const fn y(self) -> u16 {
        ((self.0 >> Self::Y_SHIFT) & 0xFFF) as u16
    }

    #[inline]
    #[must_use]
    pub const fn x(self) -> u32 {
        ((self.0 >> Self::X_SHIFT) & 0xFF_FFFF) as u32
    }

    #[inline]
    #[must_use]
    pub const fn z(self) -> u32 {
        (self.0 & 0xFF_FFFF) as u32
    }
}

impl std::fmt::Debug for WorldClusterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WorldClusterId {{ lod: {}, y: {}, x: {}, z: {}, bits: 0x{:016X} }}",
            self.lod(),
            self.y(),
            self.x(),
            self.z(),
            self.0
        )
    }
}

/// **The atlas frame** — where every cluster of a planet's world map sits.
///
/// Equirectangular over the planet's sphere: one column of the atlas is one
/// cluster of equatorial arc, one row is one cluster of meridian arc, and
/// the pole caps beyond `trim_deg` latitude are cut (the WorldMap contract's
/// trim — the caps compress toward degenerate columns and belong to no
/// gameplay). `x` wraps toroidally; `z` clamps at the trimmed edge.
#[derive(Clone, Debug)]
pub struct AtlasFrame {
    /// The icosphere frequency the planet was rolled at.
    pub freq: u32,
    /// Planet radius, metres — from the canon size model.
    pub radius_m: f64,
    /// Clusters around the equator — the wrap width.
    pub width: u32,
    /// Cluster rows between the trimmed caps.
    pub height: u32,
    /// Latitude the caps are trimmed at, degrees.
    pub trim_deg: f64,
}

impl AtlasFrame {
    /// Frame a planet of `freq` with caps trimmed at `trim_deg` latitude.
    pub fn new(freq: u32, trim_deg: f64) -> Self {
        let radius_m = clayengine::diameter_mi(freq) * clayengine::METERS_PER_MILE / 2.0;
        let cm = cluster_span_m();
        let width = (std::f64::consts::TAU * radius_m / cm).round() as u32;
        assert!(
            width <= WorldClusterId::MAX_XZ,
            "planet too large for the 24-bit x field"
        );
        let lat_span = (180.0 - 2.0 * trim_deg).to_radians();
        let height = (lat_span * radius_m / cm).round() as u32;
        assert!(
            height <= WorldClusterId::MAX_XZ,
            "planet too large for the 24-bit z field"
        );
        Self {
            freq,
            radius_m,
            width,
            height,
            trim_deg,
        }
    }

    /// Toroidal wrap on the equatorial axis: any signed column index onto
    /// `0..width`.
    #[inline]
    pub fn wrap_x(&self, x: i64) -> u32 {
        x.rem_euclid(self.width as i64) as u32
    }

    /// The unit-sphere direction at the CENTRE of atlas cell `(x, z)`.
    /// Longitude runs with `x` (wrapping); latitude runs from `+90−trim`
    /// at `z = 0` down to `−90+trim` at `z = height`.
    pub fn dir(&self, x: u32, z: u32) -> DVec3 {
        let lon = (x as f64 + 0.5) / self.width as f64 * std::f64::consts::TAU;
        let lat_top = (90.0 - self.trim_deg).to_radians();
        let lat_span = (180.0 - 2.0 * self.trim_deg).to_radians();
        let lat = lat_top - (z as f64 + 0.5) / self.height as f64 * lat_span;
        DVec3::new(
            lat.cos() * lon.cos(),
            lat.sin(),
            lat.cos() * lon.sin(),
        )
    }
}

/// Nearest-cell index over a planet's hex grid — which hex OWNS a direction.
///
/// The grid offers no spatial query, and 4M-pixel regions cannot afford a
/// 92K-cell scan each. Buckets over (lon, lat) sized well under the cell
/// spacing make the candidate set a handful; nearest-by-dot picks the owner.
/// Deterministic: same grid, same buckets, same owner — ties cannot occur off
/// a measure-zero set, and the bucket walk visits candidates in index order.
pub struct CellIndex {
    buckets: Vec<Vec<u32>>,
    lon_n: usize,
    lat_n: usize,
}

impl CellIndex {
    pub fn new(grid: &Sphere) -> Self {
        // Bucket span ≈ half the cell spacing: candidates stay few and every
        // true nearest is inside the 3×3 bucket neighbourhood searched.
        let spacing = (4.0 * std::f64::consts::PI / grid.len() as f64).sqrt();
        let lat_n = ((std::f64::consts::PI / (spacing * 0.5)).ceil() as usize).max(4);
        let lon_n = lat_n * 2;
        let mut buckets = vec![Vec::new(); lon_n * lat_n];
        for (i, d) in grid.dirs.iter().enumerate() {
            let d = d.as_dvec3().normalize();
            let (bx, by) = Self::bucket_of(d, lon_n, lat_n);
            buckets[by * lon_n + bx].push(i as u32);
        }
        Self {
            buckets,
            lon_n,
            lat_n,
        }
    }

    fn bucket_of(d: DVec3, lon_n: usize, lat_n: usize) -> (usize, usize) {
        let lon = d.z.atan2(d.x).rem_euclid(std::f64::consts::TAU);
        let lat = d.y.clamp(-1.0, 1.0).asin() + std::f64::consts::FRAC_PI_2;
        let bx = ((lon / std::f64::consts::TAU * lon_n as f64) as usize).min(lon_n - 1);
        let by = ((lat / std::f64::consts::PI * lat_n as f64) as usize).min(lat_n - 1);
        (bx, by)
    }

    /// The hex whose centre is nearest `d` — the owner of that ground.
    pub fn owner(&self, grid: &Sphere, d: DVec3) -> u32 {
        let (bx, by) = Self::bucket_of(d, self.lon_n, self.lat_n);
        let mut best = 0u32;
        let mut best_dot = f64::MIN;
        for dy in -1i64..=1 {
            let y = by as i64 + dy;
            if y < 0 || y >= self.lat_n as i64 {
                continue;
            }
            for dx in -1i64..=1 {
                let x = (bx as i64 + dx).rem_euclid(self.lon_n as i64);
                for &c in &self.buckets[y as usize * self.lon_n + x as usize] {
                    let dot = grid.dirs[c as usize].as_dvec3().normalize().dot(d);
                    if dot > best_dot {
                        best_dot = dot;
                        best = c;
                    }
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldgrid::icosphere;

    #[test]
    fn world_cluster_id_round_trips_and_isolates_fields() {
        let probes = [
            (0u8, 0u16, 0u32, 0u32),
            (15, 4095, 0xFF_FFFF, 0xFF_FFFF),
            (8, 0, 0xFF_FFFF, 0),
            (0, 4095, 0, 0),
            (0, 0, 0, 0xFF_FFFF),
            (3, 77, 1_026_030, 513_000),
        ];
        for (lod, y, x, z) in probes {
            let id = WorldClusterId::new(lod, y, x, z);
            assert_eq!(
                (id.lod(), id.y(), id.x(), id.z()),
                (lod, y, x, z),
                "round trip failed"
            );
            assert_eq!(WorldClusterId::from_bits(id.bits()), id);
        }
        // Field isolation at the packed level.
        assert_eq!(WorldClusterId::new(15, 0, 0, 0).bits(), 0xF << 60);
        assert_eq!(WorldClusterId::new(0, 0xFFF, 0, 0).bits(), 0xFFF << 48);
        assert_eq!(WorldClusterId::new(0, 0, 0xFF_FFFF, 0).bits(), 0xFF_FFFF << 24);
        assert_eq!(WorldClusterId::new(0, 0, 0, 0xFF_FFFF).bits(), 0xFF_FFFF);
        assert_eq!(
            WorldClusterId::new(15, 0xFFF, 0xFF_FFFF, 0xFF_FFFF).bits(),
            u64::MAX
        );
    }

    #[test]
    #[should_panic(expected = "x overflows")]
    fn world_cluster_id_rejects_overflow() {
        let _ = WorldClusterId::new(0, 0, 0x100_0000, 0);
    }

    #[test]
    fn a_standard_planet_fits_the_ratified_fields() {
        let frame = AtlasFrame::new(clayengine::STANDARD_FREQ, 10.0);
        // ~1.03M clusters around a freq-96 equator — comfortably in 24 bits.
        assert!(frame.width > 1_000_000 && frame.width < 1_100_000);
        assert!(frame.height < frame.width);
        // The wrap is toroidal over the frame's REAL width, not the field.
        assert_eq!(frame.wrap_x(-1), frame.width - 1);
        assert_eq!(frame.wrap_x(frame.width as i64), 0);
    }

    #[test]
    fn atlas_directions_land_on_the_sphere_and_wrap_continuously() {
        let frame = AtlasFrame::new(48, 10.0);
        for (x, z) in [(0, 0), (frame.width - 1, frame.height - 1), (7, 900)] {
            let d = frame.dir(x, z);
            assert!((d.length() - 1.0).abs() < 1e-12);
        }
        // The seam between the last column and column 0 is one cluster of
        // arc, same as any interior step — the wrap is seamless.
        let mid = frame.height / 2;
        let step_interior = frame.dir(1, mid).dot(frame.dir(2, mid)).acos();
        let step_wrap = frame.dir(frame.width - 1, mid).dot(frame.dir(0, mid)).acos();
        assert!((step_interior - step_wrap).abs() < 1e-9);
    }

    #[test]
    fn the_cell_index_agrees_with_brute_force_nearest() {
        let grid = icosphere(6);
        let index = CellIndex::new(&grid);
        let frame = AtlasFrame::new(6, 10.0);
        // Probe a scatter of atlas cells; the bucketed owner must equal the
        // exhaustive nearest.
        for (x, z) in [(0u32, 0u32), (5, 40), (500, 300), (1000, 137)] {
            let x = x % frame.width;
            let z = z % frame.height;
            let d = frame.dir(x, z);
            let brute = (0..grid.len())
                .max_by(|&a, &b| {
                    grid.dirs[a]
                        .as_dvec3()
                        .normalize()
                        .dot(d)
                        .total_cmp(&grid.dirs[b].as_dvec3().normalize().dot(d))
                })
                .unwrap() as u32;
            assert_eq!(index.owner(&grid, d), brute, "owner mismatch at ({x},{z})");
        }
    }
}
