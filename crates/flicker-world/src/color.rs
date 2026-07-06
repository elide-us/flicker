//! Per-cell colouring of the planet by epoch field.
//!
//! Reuses the engine's packed-material convention (hex-world's `pack_ramp`): the
//! mesh shader resolves `material = primary | secondary<<12 | blend<<24` to
//! `mix(palette[primary], palette[secondary], blend/255)`. So a continuous field
//! becomes a heatmap with no shader change — we just pick two palette stops and a
//! blend. Indices below mirror `flicker-render`'s `material_index_color` palette.

use flicker_materials::ElementId;
use flicker_worldgen::{Biome, HexState, LifeStage};

use crate::world::Ranges;

// Palette indices from flicker-render/src/shaders/mesh.wgsl.
const MID_WATER: u32 = 2;
const LAVA: u32 = 11;
const ICE: u32 = 12;
const DESERT: u32 = 17;
const SAVANNA: u32 = 18;
const GRASSLAND: u32 = 19;
const FOREST: u32 = 20;
const RAINFOREST: u32 = 21;
const TAIGA: u32 = 22;
const TUNDRA: u32 = 23;
const ROCK_HARD: u32 = 25;
const ORE_IRON: u32 = 26;
const ORE_COPPER: u32 = 27;
const ORE_GOLD: u32 = 28;

/// Which epoch field the planet is coloured by.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ViewMode {
    Elevation,
    Biome,
    Plates,
    Temperature,
    Precipitation,
    Prebiotic,
    Life,
    Deposits,
    Flow,
    Sediment,
    Watersheds,
    Composition,
    Crust,
    Terrain,
}

impl ViewMode {
    pub fn next(self) -> Self {
        match self {
            ViewMode::Elevation => ViewMode::Biome,
            ViewMode::Biome => ViewMode::Plates,
            ViewMode::Plates => ViewMode::Temperature,
            ViewMode::Temperature => ViewMode::Precipitation,
            ViewMode::Precipitation => ViewMode::Prebiotic,
            ViewMode::Prebiotic => ViewMode::Life,
            ViewMode::Life => ViewMode::Deposits,
            ViewMode::Deposits => ViewMode::Flow,
            ViewMode::Flow => ViewMode::Sediment,
            ViewMode::Sediment => ViewMode::Watersheds,
            ViewMode::Watersheds => ViewMode::Composition,
            ViewMode::Composition => ViewMode::Crust,
            ViewMode::Crust => ViewMode::Terrain,
            ViewMode::Terrain => ViewMode::Elevation,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Elevation => "elevation",
            ViewMode::Biome => "biome",
            ViewMode::Plates => "plates",
            ViewMode::Temperature => "temperature",
            ViewMode::Precipitation => "precipitation",
            ViewMode::Prebiotic => "prebiotic",
            ViewMode::Life => "life",
            ViewMode::Deposits => "deposits",
            ViewMode::Flow => "flow",
            ViewMode::Sediment => "sediment",
            ViewMode::Watersheds => "watersheds",
            ViewMode::Composition => "composition",
            ViewMode::Crust => "crust",
            ViewMode::Terrain => "terrain",
        }
    }

    /// 1-based index matching the `UI.hud.controls.view.options` order.
    pub fn index(self) -> u32 {
        match self {
            ViewMode::Elevation => 1,
            ViewMode::Biome => 2,
            ViewMode::Plates => 3,
            ViewMode::Temperature => 4,
            ViewMode::Precipitation => 5,
            ViewMode::Prebiotic => 6,
            ViewMode::Life => 7,
            ViewMode::Deposits => 8,
            ViewMode::Flow => 9,
            ViewMode::Sediment => 10,
            ViewMode::Watersheds => 11,
            ViewMode::Composition => 12,
            ViewMode::Crust => 13,
            ViewMode::Terrain => 14,
        }
    }

    /// Inverse of [`index`](Self::index); out-of-range falls back to elevation.
    pub fn from_index(i: u32) -> Self {
        match i {
            2 => ViewMode::Biome,
            3 => ViewMode::Plates,
            4 => ViewMode::Temperature,
            5 => ViewMode::Precipitation,
            6 => ViewMode::Prebiotic,
            7 => ViewMode::Life,
            8 => ViewMode::Deposits,
            9 => ViewMode::Flow,
            10 => ViewMode::Sediment,
            11 => ViewMode::Watersheds,
            12 => ViewMode::Composition,
            13 => ViewMode::Crust,
            14 => ViewMode::Terrain,
            _ => ViewMode::Elevation,
        }
    }
}

/// Pack a two-stop ramp into the shader's material word (see module doc).
pub fn pack_ramp(cold: u32, hot: u32, t: f32) -> u32 {
    let b = (t.clamp(0.0, 1.0) * 255.0) as u32;
    (cold & 0xFFF) | ((hot & 0xFFF) << 12) | ((b & 0xFF) << 24)
}

fn solid(index: u32) -> u32 {
    pack_ramp(index, index, 0.0)
}

fn norm(v: f32, lo: f32, hi: f32) -> f32 {
    if hi > lo {
        ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The packed material for a cell under the given view mode.
pub fn cell_material(mode: ViewMode, s: &HexState, r: &Ranges) -> u32 {
    match mode {
        // A continuous hypsometric relief: ocean depths → shore → lowland →
        // highland → snow. Reads the tectonic terrain like a physical map; the
        // ocean is only drawn where water stands, so Epoch 3 is a dry world and the
        // seas arrive at Epoch 4 (see `relief_color`).
        ViewMode::Elevation => relief_color(s, r),
        ViewMode::Biome => solid(biome_index(s.biome)),
        ViewMode::Plates => solid(PLATE_PALETTE[s.plate as usize % PLATE_PALETTE.len()]),
        ViewMode::Temperature => pack_ramp(MID_WATER, LAVA, norm(s.temperature, r.temp_min, r.temp_max)),
        // Baseline precipitation (Epoch 4): arid tan → green → humid blue. The
        // moisture field the runtime water cycle starts from. Already `0..1`.
        ViewMode::Precipitation => direct(gradient(&PRECIP_STOPS, s.precipitation.clamp(0.0, 1.0))),
        // Prebiotic chemistry (Epoch 4): barren dark → algal green → rich amber
        // "primordial soup" where life precursors brew. Already `0..1`.
        ViewMode::Prebiotic => direct(gradient(&PREBIOTIC_STOPS, s.prebiotic.clamp(0.0, 1.0))),
        // Life thread (Epochs 4–6): a discrete tint per stage, living stages
        // brightening with standing biomass.
        ViewMode::Life => life_color(s.life_stage, s.biomass),
        // Preservation deposits (Epoch 6): coal/oil (carbon → near-black) vs chalk
        // (calcium carbonate → white), brightness by accumulated mass.
        ViewMode::Deposits => deposit_color(s, r.dep_max),
        // Drainage flow (Epoch 6): log-scaled so trunk rivers stand out; land ramps
        // dry → river-blue, ocean flat. The water sim's starting flow field.
        ViewMode::Flow => flow_color(s, r),
        // Deposited sediment (Epoch 6): bare rock → sediment tan, the conveyor's
        // payload pooling in basins and along coasts.
        ViewMode::Sediment => direct(gradient(&SEDIMENT_STOPS, norm(s.sediment, 0.0, r.sediment_max))),
        // Drainage basins (Epoch 6): a distinct tint per watershed so the basins
        // that share an outlet read as one region.
        ViewMode::Watersheds => solid(PLATE_PALETTE[s.watershed as usize % PLATE_PALETTE.len()]),
        // The primordial foundation: a continuous, amount-weighted blend of the
        // cell's material colours over a dark molten base — so the *mix* (and the
        // element sliders) shift the colour smoothly, not in discrete jumps.
        ViewMode::Composition => composition_color(s),
        // Differentiation: where the light crust stayed thin (heavy metals near
        // the surface) it reads molten; where it thickened, cooled silicate.
        ViewMode::Crust => {
            let t = norm(s.crust_fraction as f32, r.crust_min, r.crust_max);
            let molten = [0.72, 0.27, 0.08];
            let solid = [0.20, 0.21, 0.25];
            direct([
                molten[0] + (solid[0] - molten[0]) * t,
                molten[1] + (solid[1] - molten[1]) * t,
                molten[2] + (solid[2] - molten[2]) * t,
            ])
        }
        // The Terrain view samples the sub-hex field per-vertex in `globe` (relief
        // displacement + hardness tint); this flat per-hex path is just a fallback.
        ViewMode::Terrain => relief_color(s, r),
    }
}

/// Direct RGB material (the shader's `primary == 0xFFF` escape; RGB666 in the
/// upper bits). Lets a view express any colour, not just a palette ramp.
fn direct(rgb: [f32; 3]) -> u32 {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 63.0).round() as u32) & 0x3F;
    0xFFF | (q(rgb[0]) << 12) | (q(rgb[1]) << 18) | (q(rgb[2]) << 24)
}

/// Terrain tint for the sub-hex relief view: ocean blue where submerged, else a
/// **hardness** ramp (soft tan → hard pale grey) with an ore-vein rust streak.
/// This is what makes the within-hex shape read as *material* — hard rock pale and
/// standing, soft rock dark and planed.
pub fn hardness_terrain(hardness: f32, vein: f32, submerged: bool) -> u32 {
    if submerged {
        return direct([0.10, 0.22, 0.38]);
    }
    let t = (hardness / 10.0).clamp(0.0, 1.0);
    let soft = [0.46, 0.38, 0.28];
    let hard = [0.72, 0.72, 0.75];
    let mut rgb = [
        soft[0] + (hard[0] - soft[0]) * t,
        soft[1] + (hard[1] - soft[1]) * t,
        soft[2] + (hard[2] - soft[2]) * t,
    ];
    let v = vein.clamp(0.0, 1.0) * 0.6;
    let ore = [0.50, 0.16, 0.09];
    rgb = [
        rgb[0] + (ore[0] - rgb[0]) * v,
        rgb[1] + (ore[1] - rgb[1]) * v,
        rgb[2] + (ore[2] - rgb[2]) * v,
    ];
    direct(rgb)
}

/// Hypsometric relief. Ocean is drawn only where water actually stands
/// (`water_depth`), so a pre-hydrosphere world (Epoch 3) shows its **dry** basins
/// as low ground and the seas visibly *arrive* at Epoch 4 as the water fills them.
/// Dry land ramps from the coastline once a world has seas, else from the deepest
/// ground so the bare tectonic topography still reads top to bottom.
fn relief_color(s: &HexState, r: &Ranges) -> u32 {
    if s.water_depth > 0.0 {
        let span = (s.sea_level - r.elev_min).max(1.0e-4);
        let t = (s.water_depth / span).clamp(0.0, 1.0); // 0 shore .. 1 abyss
        direct(gradient(&OCEAN_STOPS, t))
    } else {
        let floor = if r.max_depth > 0.0 { s.sea_level } else { r.elev_min };
        let span = (r.elev_max - floor).max(1.0e-4);
        let t = ((s.elevation - floor) / span).clamp(0.0, 1.0); // 0 shore/low .. 1 peak
        direct(gradient(&LAND_STOPS, t))
    }
}

/// Piecewise-linear colour ramp through ascending `(stop, rgb)` pairs.
fn gradient(stops: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    let mut i = 0;
    while i + 1 < stops.len() && t > stops[i + 1].0 {
        i += 1;
    }
    let (t0, c0) = stops[i];
    let (t1, c1) = stops[(i + 1).min(stops.len() - 1)];
    let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
    [
        c0[0] + (c1[0] - c0[0]) * f,
        c0[1] + (c1[1] - c0[1]) * f,
        c0[2] + (c1[2] - c0[2]) * f,
    ]
}

/// Ocean: shore-shallow (t=0) → abyssal navy (t=1).
const OCEAN_STOPS: [(f32, [f32; 3]); 3] = [
    (0.0, [0.32, 0.55, 0.66]),
    (0.45, [0.12, 0.30, 0.55]),
    (1.0, [0.02, 0.06, 0.20]),
];

/// Precipitation: arid tan (t=0) → grassy green → humid deep blue (t=1).
const PRECIP_STOPS: [(f32, [f32; 3]); 3] = [
    (0.0, [0.80, 0.72, 0.46]),
    (0.5, [0.35, 0.62, 0.45]),
    (1.0, [0.10, 0.34, 0.62]),
];

/// Prebiotic: barren dark (t=0) → algal green → rich amber soup (t=1).
const PREBIOTIC_STOPS: [(f32, [f32; 3]); 3] = [
    (0.0, [0.08, 0.10, 0.11]),
    (0.5, [0.30, 0.46, 0.22]),
    (1.0, [0.80, 0.72, 0.28]),
];

/// Land: shore green (t=0) → lowland → highland brown → rock → snow (t=1).
const LAND_STOPS: [(f32, [f32; 3]); 5] = [
    (0.0, [0.28, 0.46, 0.24]),
    (0.30, [0.45, 0.52, 0.28]),
    (0.55, [0.50, 0.42, 0.27]),
    (0.80, [0.54, 0.51, 0.49]),
    (1.0, [0.95, 0.95, 0.97]),
];

/// A cell's foundation colour: each surface element contributes its muted,
/// primordial tint weighted by its fraction of the composition. Iron reads as
/// molten rust, carbon near-black, silicates blue-grey rock — so the material
/// provinces (and slider edits) show as soft, continuous washes.
fn composition_color(s: &HexState) -> u32 {
    let comp = s.surface();
    let total = comp.total();
    if total <= 0.0 {
        return direct([0.05, 0.04, 0.04]);
    }
    let mut rgb = [0.0f32; 3];
    for (el, amount) in comp.iter() {
        let f = (amount / total) as f32;
        let c = element_rgb(el);
        rgb[0] += f * c[0];
        rgb[1] += f * c[1];
        rgb[2] += f * c[2];
    }
    // Keep it dark/molten — a foundation, not a finished surface.
    direct([rgb[0] * 0.9, rgb[1] * 0.9, rgb[2] * 0.9])
}

/// Muted, dark "primordial" tint per element (RGB 0..1). Unknown → dark rock.
fn element_rgb(el: ElementId) -> [f32; 3] {
    match el {
        1 => [0.20, 0.28, 0.34],  // H  faint blue-grey
        6 => [0.09, 0.09, 0.10],  // C  near-black (carbon)
        7 => [0.24, 0.28, 0.30],  // N  pale grey
        8 => [0.22, 0.26, 0.34],  // O  blue-grey (silicate oxygen)
        11 => [0.34, 0.31, 0.20], // Na dull yellow
        13 => [0.36, 0.37, 0.40], // Al light grey
        14 => [0.31, 0.27, 0.21], // Si tan rock (silica)
        15 => [0.30, 0.25, 0.17], // P  brown
        16 => [0.44, 0.39, 0.15], // S  sulphur yellow
        17 => [0.26, 0.34, 0.24], // Cl faint green
        19 => [0.33, 0.24, 0.31], // K  faint violet
        20 => [0.42, 0.42, 0.39], // Ca pale stone
        22 => [0.34, 0.37, 0.41], // Ti steel
        26 => [0.48, 0.16, 0.09], // Fe molten rust-red
        _ => [0.18, 0.17, 0.16],  // dark rock
    }
}

/// Life-thread tint: a base colour per stage, the living stages brightened by
/// standing biomass so denser life reads stronger.
fn life_color(stage: LifeStage, biomass: f32) -> u32 {
    let base = match stage {
        LifeStage::Barren => [0.12, 0.12, 0.13],    // bare rock
        LifeStage::Prebiotic => [0.55, 0.47, 0.20], // amber soup
        LifeStage::Microbial => [0.16, 0.52, 0.42], // teal mats
        LifeStage::Fungal => [0.40, 0.42, 0.20],    // olive (Epoch 6)
        LifeStage::Floral => [0.26, 0.62, 0.24],    // green (Epoch 6)
    };
    let lit = if stage >= LifeStage::Microbial {
        0.4 + 0.6 * biomass.clamp(0.0, 1.0)
    } else {
        1.0
    };
    direct([base[0] * lit, base[1] * lit, base[2] * lit])
}

/// Drainage-flow tint: ocean flat deep-blue; land ramps dry → river-blue on a
/// **log** scale (flow accumulates exponentially down trunks, so log keeps the
/// tributaries visible rather than only the main stem).
fn flow_color(s: &HexState, r: &Ranges) -> u32 {
    if s.water_depth > 0.0 {
        return direct([0.05, 0.09, 0.18]);
    }
    let t = if r.flow_max > 1.0 {
        ((1.0 + s.flow).ln() / (1.0 + r.flow_max).ln()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    direct(gradient(&FLOW_STOPS, t))
}

/// Flow: dry upland (t=0) → stream → bright trunk river (t=1).
const FLOW_STOPS: [(f32, [f32; 3]); 3] = [
    (0.0, [0.20, 0.19, 0.16]),
    (0.5, [0.24, 0.44, 0.60]),
    (1.0, [0.42, 0.76, 0.96]),
];

/// Sediment: bare rock (t=0) → deposited tan/silt (t=1).
const SEDIMENT_STOPS: [(f32, [f32; 3]); 2] = [
    (0.0, [0.15, 0.15, 0.16]),
    (1.0, [0.64, 0.52, 0.33]),
];

/// Preservation-deposit tint: blend coal/oil (carbon, near-black) → chalk (calcium
/// carbonate, white) by the calcium share, brightness scaled by total mass in the
/// layer. Barren grey where nothing was preserved.
fn deposit_color(s: &HexState, dep_max: f32) -> u32 {
    let c = s.deposits.amount(6) as f32; // carbon — coal / oil
    let ca = s.deposits.amount(20) as f32; // calcium — chalk carbonate
    let total = c + ca;
    if total <= 1e-6 {
        return direct([0.10, 0.10, 0.11]); // barren rock
    }
    let chalk_frac = (ca / total).clamp(0.0, 1.0);
    let coal = [0.06, 0.05, 0.05];
    let chalk = [0.92, 0.92, 0.86];
    let base = [
        coal[0] + (chalk[0] - coal[0]) * chalk_frac,
        coal[1] + (chalk[1] - coal[1]) * chalk_frac,
        coal[2] + (chalk[2] - coal[2]) * chalk_frac,
    ];
    // A floor so present-but-small deposits still read against the barren rock.
    let lit = 0.3 + 0.7 * norm(total, 0.0, dep_max);
    direct([base[0] * lit, base[1] * lit, base[2] * lit])
}

fn biome_index(b: Biome) -> u32 {
    match b {
        Biome::Ocean => MID_WATER,
        Biome::Ice => ICE,
        Biome::Tundra => TUNDRA,
        Biome::Taiga => TAIGA,
        Biome::Grassland => GRASSLAND,
        Biome::Forest => FOREST,
        Biome::Rainforest => RAINFOREST,
        Biome::Savanna => SAVANNA,
        Biome::Desert => DESERT,
        Biome::Alpine => ROCK_HARD,
    }
}

/// Distinct palette stops cycled per plate id, so adjacent plates contrast.
const PLATE_PALETTE: [u32; 12] = [
    DESERT, SAVANNA, GRASSLAND, FOREST, RAINFOREST, TAIGA, TUNDRA, LAVA, ICE, ORE_IRON, ORE_COPPER,
    ORE_GOLD,
];
