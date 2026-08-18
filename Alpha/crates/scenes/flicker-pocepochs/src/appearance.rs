//! **What the simulation LOOKS like** — this bench's data→colour vocabulary, and
//! nothing else.
//!
//! What used to live beside this in a private `globe.rs` — a second `RADIUS`, a
//! second `in_wedge`, a second `build_shell` and a second `OrbitCam` — is gone:
//! the mesh, the stage, the offscreen target and the camera all belong to
//! [`flicker_globe::GlobeWorld`], which is the ONE globe in Prism (rule
//! DDD070C7). The per-column radius this bench needed was absorbed there as
//! [`flicker_globe::ShellSpec::cell_radius`], so the layer-stack view is now the
//! shared builder answering a different question, not a fork of it.
//!
//! What stays here is the half no shared crate can own: a `flicker_worldengine`
//! `HexState` turned into a stack of sized, classified layers and a colour per
//! cell. Those colours are **per-DATUM** (an element nobody has picked a hue for
//! is hashed one), which is the sanctioned exception the authoring guide names —
//! the walker's colour channel is dotted style paths, and a per-datum colour has
//! no path.

use flicker_globe::RADIUS;
use flicker_worldengine::{classify, HexState, LayerKind, Phase, Tables};

/// How the planet is coloured. `V` cycles them.
#[derive(Copy, Clone, PartialEq)]
pub enum ViewMode {
    /// The dominant surface material — the top layer of the column (the mantle, then crust).
    Material,
    /// The mantle convection heat: cold downwelling (blue) vs hot upwelling (orange).
    Heat,
    /// The **layer stack** — core / mantle / crust drawn as concentric shells.
    Layers,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Material => "material",
            ViewMode::Heat => "convection heat",
            ViewMode::Layers => "layer stack",
        }
    }
}

/// Cycle to the next view (the `V` key).
pub fn cycle_view(mode: ViewMode) -> ViewMode {
    match mode {
        ViewMode::Material => ViewMode::Heat,
        ViewMode::Heat => ViewMode::Layers,
        ViewMode::Layers => ViewMode::Material,
    }
}

/// Base radius of the interior ball — the mantle surface. The crust / ocean / atmosphere stack
/// OUTWARD from here, each sized by its physical volume (see [`cell_stack`]).
pub const R_BASE: f32 = 0.85 * RADIUS;
/// Legibility knobs for the physical-thickness stack (tune live in the app): a floor so a thin
/// layer still shows, and the scale on the cube-root-compressed volume ratio (the raw gas-vs-
/// rock volume range is ~1000×, cube-root keeps it legible while preserving the ordering).
const STACK_MIN_T: f32 = 4.0;
const STACK_K: f32 = 30.0;

/// One classified, physically-sized layer in a cell's drawn stack.
pub struct StackLayer {
    pub kind: LayerKind,
    /// Cumulative outer radius (world units) — the layer's drawn top.
    pub outer_r: f32,
    /// Derived colour of what this layer IS (from [`classify`]).
    pub color: [f32; 3],
}

/// Build a cell's drawn stack: **classify** each layer (from composition, the hex's own
/// temperature, and pressure → what it IS) and size it by **physical volume** (mass ÷ phase
/// density, cube-root-compressed for legibility), accumulating OUTWARD from the mantle ball.
/// The mantle is the base ball at [`R_BASE`]; the core is data (not drawn). Pressure falls with
/// stack height (deep = high). A derived read — the sim stores only the material + temperature.
pub fn cell_stack(cell: &HexState, tables: &Tables) -> Vec<StackLayer> {
    let layers = cell.column.layers();
    if layers.is_empty() {
        return Vec::new();
    }
    let temp = cell.temperature; // the hex's own temperature (K)
    let n = layers.len().max(2);
    // The mantle's volume is the per-cell reference the above-mantle layers scale against.
    let mantle_vol = cell
        .column
        .find(LayerKind::Mantle)
        .map(|i| {
            let l = &layers[i];
            let c = classify(&l.composition, &l.compounds, temp, 1.0, tables);
            (l.mass() / c.density_g_cm3.max(1e-4) as f64).max(1e-9)
        })
        .unwrap_or(1.0);

    let mut out = Vec::new();
    let mut r = R_BASE;
    for (i, l) in layers.iter().enumerate() {
        if l.kind == LayerKind::Core {
            continue; // the core is data, folded into the base ball — never a nested sphere
        }
        let pressure = 1.0 - (i as f32 / (n - 1) as f32); // deep = high, top (air) = low
        let c = classify(&l.composition, &l.compounds, temp, pressure, tables);
        if l.kind == LayerKind::Mantle {
            out.push(StackLayer {
                kind: l.kind,
                outer_r: R_BASE,
                color: c.color,
            });
            r = R_BASE;
        } else {
            let vol = (l.mass() / c.density_g_cm3.max(1e-4) as f64).max(0.0);
            let rel = (vol / mantle_vol) as f32;
            r += STACK_MIN_T + STACK_K * rel.cbrt();
            out.push(StackLayer {
                kind: l.kind,
                outer_r: r,
                color: c.color,
            });
        }
    }
    out
}

/// Legend colour for a phase (the derived layer-stack view keys on phase, not a fixed kind).
pub fn phase_color(phase: Phase) -> [f32; 3] {
    match phase {
        Phase::Solid => [0.60, 0.56, 0.48],
        Phase::Molten => [0.95, 0.45, 0.20],
        Phase::Liquid => [0.15, 0.40, 0.78],
        Phase::Gas => [0.70, 0.76, 0.86],
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Muted colour per dominant surface element (atomic number).
fn element_color(dom: Option<u8>) -> [f32; 3] {
    match dom {
        Some(8) => [0.55, 0.55, 0.60],  // O
        Some(14) => [0.72, 0.66, 0.52], // Si — silicate tan
        Some(13) => [0.66, 0.66, 0.70], // Al
        Some(26) => [0.56, 0.28, 0.18], // Fe — rust
        Some(20) => [0.80, 0.78, 0.72], // Ca — pale carbonate
        Some(6) => [0.12, 0.12, 0.14],  // C — dark
        Some(11) => [0.78, 0.74, 0.60], // Na
        Some(19) => [0.70, 0.66, 0.40], // K
        Some(12) => [0.58, 0.62, 0.55], // Mg
        Some(22) => [0.44, 0.45, 0.50], // Ti
        Some(1) => [0.30, 0.45, 0.70],  // H
        Some(x) => {
            let h = (x as f32 * 0.137).fract();
            [0.40 + 0.30 * h, 0.35, 0.52 - 0.20 * h]
        }
        None => [0.20, 0.20, 0.24],
    }
}

/// The material colour of an element (by atomic number) — for the distribution readout.
pub fn element_rgb(number: u8) -> [f32; 3] {
    element_color(Some(number))
}

/// Convection-heat ramp: cold downwelling (deep blue) → neutral (dark) → hot upwelling (orange).
fn heat_color(heat: f32) -> [f32; 3] {
    let t = heat.clamp(0.0, 1.0);
    if t < 0.5 {
        lerp3([0.10, 0.22, 0.58], [0.12, 0.10, 0.16], t * 2.0)
    } else {
        lerp3([0.12, 0.10, 0.16], [0.96, 0.56, 0.16], (t - 0.5) * 2.0)
    }
}

/// The dominant surface material of a cell — the top **non-gas** layer (the atmosphere is
/// see-through, so Material view reads the ocean/crust beneath it). Falls back to the flat
/// surface if the column is empty.
fn surface_dominant(c: &HexState) -> Option<u8> {
    c.column
        .layers()
        .iter()
        .rev()
        .find(|l| l.kind != LayerKind::Atmosphere)
        .map(|l| l.composition.dominant())
        .unwrap_or_else(|| c.surface().dominant())
}

/// A cell's surface material colour (Material view) — the top layer's dominant element.
pub fn material_color(c: &HexState) -> [f32; 3] {
    element_color(surface_dominant(c))
}

/// A cell's temperature colour (Heat view) — the hex's own temperature as a `0..1` read.
pub fn cell_heat_color(c: &HexState) -> [f32; 3] {
    heat_color(flicker_worldengine::cooling::normalized(c.temperature))
}

/// The legend for a view: `(label, colour)` rows the scene draws as a swatch key.
pub fn legend_entries(mode: ViewMode) -> Vec<(&'static str, [f32; 3])> {
    match mode {
        ViewMode::Material => vec![
            ("O oxygen", element_color(Some(8))),
            ("Si silicate", element_color(Some(14))),
            ("Al aluminium", element_color(Some(13))),
            ("Fe iron", element_color(Some(26))),
            ("Ca calcium", element_color(Some(20))),
            ("Mg magnesium", element_color(Some(12))),
            ("C carbon", element_color(Some(6))),
        ],
        ViewMode::Heat => vec![
            ("cold downwelling", [0.20, 0.34, 0.72]),
            ("neutral", [0.35, 0.33, 0.40]),
            ("hot upwelling", [0.96, 0.62, 0.28]),
        ],
        ViewMode::Layers => vec![
            ("solid rock", phase_color(Phase::Solid)),
            ("molten", phase_color(Phase::Molten)),
            ("liquid (water)", phase_color(Phase::Liquid)),
            ("gas (air)", phase_color(Phase::Gas)),
        ],
    }
}
