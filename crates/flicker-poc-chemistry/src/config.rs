//! Invariant world constants (spec §2). Planet size is **fixed at Earth** — there
//! is no size slider. A hex *is* a 2048² heightmap at 128 ft/px, so 49.65 mi is a
//! consequence of the cluster geometry, not a tuning knob; the two independent
//! derivations (Goldberg cell size vs heightmap span) agree only at freq 96.

use std::path::PathBuf;

/// Icosphere subdivision frequency. **Fixed at 96** → `10·96² + 2 = 92_162` cells
/// (92_150 hexes + 12 pentagons), a hex ≈ 49.67 mi across — matching the
/// independent 2048 px × 128 ft = 49.65 mi span. The two agree only here.
pub const PLANET_FREQ: u32 = 96;

/// Cell count at [`PLANET_FREQ`]: `10·freq² + 2`.
pub const PLANET_CELLS: usize = (10 * PLANET_FREQ * PLANET_FREQ + 2) as usize;

/// Total planet mass (Earth), kg. The accretion budget sums to exactly this.
pub const PLANET_MASS_KG: f64 = 5.972e24;

/// Planet radius, m.
pub const PLANET_RADIUS_M: f64 = 6_371_000.0;

/// Area of one hex cell, m² (2048² px × (128 ft)² ≈ 5.534×10⁹).
///
/// **Equal-area is a PENDING prerequisite** (worldgrid Slice 3b / spec §2.1): the
/// current projection spreads hex area ~1.75×, so every *areal* derivation
/// (thickness, weathering flux, submerged fraction) inherits that error until
/// Slice 3b lands. Mass conservation — the M0 invariant — is areal-independent
/// (absolute masses), so it is unaffected; this constant is a nominal used only
/// by the derived thickness function today.
pub const CELL_AREA_M2: f64 = 5.534e9;

/// Nominal formation tick, Myr. The adaptive controller (spec §7.3) arrives with
/// the interior stages; M0 steps at this nominal rate.
pub const NOMINAL_DT_MYR: f64 = 1.0;

/// The repo content directory (`Alpha/content/data`), resolved relative to this
/// crate — mirrors `flicker-worldengine`'s `from_repo` seam.
pub fn content_data_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data"))
}
