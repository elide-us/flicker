//! The God Mode scene — a **thin renderer** over the sim thread that draws the
//! world as a **stack of concentric layer meshes**.
//!
//! Each layer is its own mesh at its own radius, drawn where it exists and stacked
//! bottom→top: the **core** (inner sphere), the **mantle** shell above it (always
//! there, still convecting — never replaced), then each **crust bed** as a sparse
//! shell above the mantle (holes where the surface is still bare magma). Forming
//! crust *adds* an outer shell; it never recolours the mantle. The stack grows as
//! the chemistry produces layers (water + atmosphere, sediment beds, … each just
//! registers another shell). Occlusion shows the outermost shell that exists at
//! each spot, and **C** cuts a wedge out of every shell above the core so the
//! stack reads in section.
//!
//! The sim runs on its own thread ([`crate::sim_thread`], spec §11); this scene
//! only sends commands (Space play/pause · R reseed · Down restart) and draws the
//! latest snapshot. **V** cycles the field the globe is coloured by: the interior
//! reads (temperature / differentiation / plates / seams) recolour the mantle,
//! and the surface reads (elevation against the solved sea level / how many beds a
//! column has stacked) recolour the crust — recolouring only, never a replacement
//! of the stack.

use std::time::Duration;

use flicker::render::{FrameGraph, Renderer, TextureHandle, Vec2, Vec3};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, UiNode, Value, ValueMap};
use flicker::ui::{render_hud, run_ui, strings, UiInput, UiIntents, UiState, WalkerHandler};
use flicker_input_core::{
    AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState,
};
use flicker_input_core::{Fired, Resolver};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};

use crate::globe_view;
use crate::route::RootHandler;
use crate::sim_thread::{
    CellView, SeedSpec, SimCommand, SimHandle, Snapshot, TilePreview, BED_CONTINENTAL, BED_OCEANIC,
    SHELF_BED, SHELF_CLASS, SHELF_EDGE, SHELF_EXPOSED, SHELF_LAND, SHELF_NONE, SHELF_SHELF,
};
use flicker_globe::{self as globe, GlobeWorld, ShellSpec, RADIUS};
use flicker_poc_chemistry::{ProcessDef, PLANET_FREQ};

/// The Starter's one-click input bundles — see
/// [`apply_preset`](GodModeScene::apply_preset).
#[derive(Clone, Copy)]
enum Preset {
    Mercury,
    Venus,
    Earth,
    Mars,
    Europa,
}
use flicker_poc_chemistry::{Levers, PlateEvent};

/// Radii of the shells (exaggerated for legibility — real crust is a hair-thin
/// rind on the mantle; here the gaps are opened up so the stack is visible and the
/// cutaway can show it in section). Bottom → top.
const R_CORE: f32 = 0.50 * RADIUS;
const R_MANTLE: f32 = 0.960 * RADIUS;
const R_OCEANIC: f32 = 0.985 * RADIUS;
const R_CONTINENTAL: f32 = 1.000 * RADIUS;

/// Shell base colours (before lighting).
const CORE_COLOR: [f32; 3] = [0.95, 0.45, 0.20]; // molten metal
const OCEANIC_COLOR: [f32; 3] = [0.15, 0.22, 0.33]; // dark mafic sea floor
const CONTINENTAL_COLOR: [f32; 3] = [0.60, 0.54, 0.40]; // pale silicic land
/// The mantle drawn neutral, so a surface field on the crust above reads clearly.
const BARE_MANTLE_COLOR: [f32; 3] = [0.16, 0.15, 0.17];

/// Process rows the panel can show. Deliberate **headroom** over the pipeline's
/// current length: an unused row rides `proc_<n>_shown = false` and the walker's
/// flow layout skips invisible children entirely, so it occupies nothing. Adding
/// a stage therefore needs no change here or in the Lua — which is the point,
/// because this list is meant to keep growing.
const PROCESS_ROWS: usize = 20;

/// Rows in the world-event bank. The sim keeps 8 plate events and 6 gate
/// events; this is the window onto the merged, newest-first stream, and it
/// matches `godmode_events`.
const EVENT_ROWS: usize = 8;

// ── The air shells (the classified exhale, task: sky tier) ──
/// First air shell sits just off the ground; each lighter gas one step higher.
const R_AIR_BASE: f32 = 1.035;
const R_AIR_STEP: f32 = 0.022;
/// Column mass at which a gas shell WANTS to close into a full lid, kg/m² — the
/// scale of Earth's own total column. Raw coverage saturates here; the stack
/// squeeze below is what keeps saturation from actually hiding the planet.
const FULL_AIR_KG_M2: f64 = 1.0e4;
/// No single shell's raw coverage passes this — keeps its optical depth finite
/// so the stack squeeze always has room to work.
const MAX_SHELL_COVERAGE: f64 = 0.95;
/// Ceiling on the whole stack's combined optical depth: e^−0.4 ≈ two-thirds of
/// the surface cells stay visible through the densest possible sky. The veil
/// says how thick the air is; it is never allowed to say nothing else. (Was 0.7
/// — a half-hidden globe read as too dense on the bench; **A** still toggles
/// the veil off entirely.)
const MAX_STACK_TAU: f64 = 0.4;

/// What each gas looks like — display tints for the sky tier's vocabulary
/// (catalog ids). A gas the vocabulary has not met yet renders neutral grey
/// rather than invisibly.
fn gas_tint(gas: u16) -> [f32; 3] {
    use flicker_poc_chemistry::atmosphere as sky;
    match gas {
        sky::WATER_VAPOUR => [0.93, 0.95, 0.99], // steam — near-white
        sky::CARBON_DIOXIDE => [0.87, 0.58, 0.30], // hotbox amber
        sky::NITROGEN => [0.45, 0.62, 0.92],     // temperate blue
        sky::SULFUR_DIOXIDE => [0.92, 0.86, 0.35], // volcanic yellow
        sky::HYDROGEN_CHLORIDE => [0.62, 0.86, 0.55], // acid green
        sky::METHANE => [0.72, 0.45, 0.85],      // reducing violet — a young world
        flicker_poc_chemistry::biosphere::OXYGEN => [0.55, 0.92, 0.88], // the biosignature
        _ => [0.70, 0.72, 0.75],
    }
}

/// The `rtt` node the globe is drawn into — the one name the proto's `id` and
/// the scene's slot lookup share.
const GLOBE_SLOT: &str = "gm_globe";

/// The field tabs: the action a button fires, and the view it selects. One
/// table, so the tab strip, the dispatcher and the lit-tab styling can never
/// disagree about how many views there are or what they are called.
/// Each row is `(action id, view, label token, which shell it paints)`. The
/// TARGET rides here because it used to live nowhere: the binding was implicit
/// in two separate `match` arms — one in the mantle builder, one in
/// `crust_color` — and nothing declared which shell a view belonged to. A field
/// absent from both painted NOTHING, silently, and Plates/Seams sat in the
/// wrong arm for as long as the bench has existed: they coloured the mantle at
/// 0.960 while the crust shells above them kept their rock colours, so plate
/// colour was visible only through holes in the crust or in cutaway (Aaron:
/// *"the plates and seams visualization are basically useless… it just colors
/// the shell under the lower layer"*). The label rides here too so the
/// strip's buttons and anything else that has to NAME a view — the pause card's
/// switch — read one roster; `the_view_roster_agrees_with_itself` pins these
/// against the authored buttons.
const FIELD_ACTIONS: [(&str, Field, &str, Paints); 10] = [
    (
        "field_temperature",
        Field::Temperature,
        "$chem_field_heat",
        Paints::Interior,
    ),
    (
        "field_differentiation",
        Field::Differentiation,
        "$chem_field_core",
        Paints::Interior,
    ),
    (
        "field_plates",
        Field::Plates,
        "$chem_field_plates",
        Paints::Surface,
    ),
    (
        "field_seams",
        Field::Seams,
        "$chem_field_seams",
        Paints::Surface,
    ),
    (
        "field_elevation",
        Field::Elevation,
        "$chem_field_relief",
        Paints::Surface,
    ),
    (
        "field_coast",
        Field::Coast,
        "$chem_field_coast",
        Paints::Surface,
    ),
    (
        "field_motion",
        Field::Motion,
        "$chem_field_motion",
        Paints::Overlay,
    ),
    (
        "field_rain",
        Field::Rain,
        "$chem_field_rain",
        Paints::Surface,
    ),
    (
        "field_strata",
        Field::Strata,
        "$chem_field_strata",
        Paints::Surface,
    ),
    ("field_ore", Field::Ore, "$chem_field_ore", Paints::Surface),
];

/// The stringtable token naming a view — the roster's own label, so the pause
/// card cannot call a view something the button does not.
fn view_token(field: Field) -> &'static str {
    FIELD_ACTIONS
        .iter()
        .find(|&&(_, f, _, _)| f == field)
        .map(|&(_, _, token, _)| token)
        .unwrap_or("$chem_field_heat")
}

/// Chips in a legend ramp strip — the sampled gradient the continuous views
/// explain themselves with.
const LEGEND_STRIP: usize = 8;

/// The extremes the active frame's ramps are stretched over — cached when a
/// snapshot arrives, because the legend prints them every frame and a 92k-cell
/// sweep per frame is exactly the recompute rule 405F7034 forbids.
#[derive(Default)]
struct LegendRanges {
    tmin: f32,
    tmax: f32,
    /// The relief ramp's ends — the 2nd and 98th elevation PERCENTILES, not the
    /// min and max. A hypsometry is brutally skewed: nearly every column is
    /// thin rind within metres of one height and a handful of collision piles
    /// spike kilometres above it, so a min-max stretch hands the entire grey
    /// range to the outliers and paints the planet pure black with a few light
    /// strips (Aaron's 500 My black ball). Percentiles spend the greys on the
    /// ninety-six percent of the world that actually varies; the outliers
    /// clamp, and the legend's "≤ / ≥" labels say so.
    elo: f32,
    ehi: f32,
    wettest: f32,
    deepest: u8,
    richest: f32,
}

impl LegendRanges {
    fn from_cells(cells: &[CellView]) -> Self {
        if cells.is_empty() {
            return Self::default();
        }
        let mut r = Self {
            tmin: f32::MAX,
            tmax: f32::MIN,
            ..Self::default()
        };
        for c in cells {
            r.tmin = r.tmin.min(c.temp_k);
            r.tmax = r.tmax.max(c.temp_k);
            r.wettest = r.wettest.max(c.rain);
            r.deepest = r.deepest.max(c.strata);
            r.richest = r.richest.max(c.ore);
        }
        // The elevation percentiles — two O(n) selects, once per snapshot.
        let mut elev: Vec<f32> = cells.iter().map(|c| c.elevation_m).collect();
        let last = elev.len() - 1;
        let (p_lo, p_hi) = (last * 2 / 100, last * 98 / 100);
        let cmp = |a: &f32, b: &f32| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal);
        r.elo = *elev.select_nth_unstable_by(p_lo, cmp).1;
        r.ehi = *elev.select_nth_unstable_by(p_hi, cmp).1;
        // A world of one height still needs a span to divide by.
        if r.ehi - r.elo < 1.0 {
            r.ehi = r.elo + 1.0;
        }
        r
    }
}

/// A lever's reader and writer, so the rack can be a table instead of twelve
/// near-identical arms.
type LeverGet = fn(&Levers) -> f64;
type LeverSet = fn(&mut Levers, f64);

/// **The rate levers**, each as a MULTIPLE of the physics as written (`1.0` =
/// exactly what the process chose for itself). Sharing one vocabulary is what
/// lets them share one range, one guard and one echo — and it means adding a
/// lever is one row here plus one row in the rack proto.
///
/// The two water levers are deliberately absent: a delivery budget and a
/// coverage fraction are not multiples of anything, so they keep their own
/// arms with their own units.
const LEVERS: &[(&str, LeverGet, LeverSet)] = &[
    (
        "lv_veneer",
        |l| l.veneer_budget_kg,
        |l, v| l.veneer_budget_kg = v,
    ),
    ("lv_core_heat", |l| l.core_heat, |l, v| l.core_heat = v),
    ("lv_stellar", |l| l.stellar_heat, |l, v| l.stellar_heat = v),
    (
        "lv_crust_gen",
        |l| l.crust_gen_rate,
        |l, v| l.crust_gen_rate = v,
    ),
    ("lv_arc", |l| l.arc_return, |l, v| l.arc_return = v),
    ("lv_outgas", |l| l.outgas_rate, |l, v| l.outgas_rate = v),
    (
        "lv_eruption",
        |l| l.eruption_rate,
        |l, v| l.eruption_rate = v,
    ),
    (
        "lv_production",
        |l| l.production_rate,
        |l, v| l.production_rate = v,
    ),
    (
        "lv_decomposer",
        |l| l.decomposer_niche_kg,
        |l, v| l.decomposer_niche_kg = v,
    ),
    (
        "lv_yield",
        |l| l.yield_strain as f64,
        |l, v| l.yield_strain = v as f32,
    ),
    ("lv_erosion", |l| l.erosion_rate, |l, v| l.erosion_rate = v),
    ("lv_leach", |l| l.leach_rate, |l, v| l.leach_rate = v),
];

/// A checkbox's written-back state. Distinct from [`ValueMap::is_on`], which
/// cannot tell "absent" from "present and false" — and for a toggle those are
/// opposite instructions: leave it alone, versus turn it off.
fn toggled(results: &ValueMap, key: &str) -> Option<bool> {
    match results.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Deterministic per-cell stipple: does cell `i` carry shell `k` at this
/// coverage? The mesh path has no alpha, so DENSITY is the honest channel — a
/// shell's stipple density follows its column mass (via [`veil_coverages`]),
/// and a hash (not a random draw) keeps the veil stable frame to frame.
fn stippled(i: usize, k: usize, coverage: f64) -> bool {
    let h = (i as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add(k as u32 * 97);
    ((h >> 8) % 1000) < (coverage * 1000.0) as u32
}

// ── Motion arrows ([`Field::Motion`]) ──
/// How many headings the view aims to draw, whatever the planet's resolution —
/// enough to read the flow, sparse enough to still see the ground under it.
/// Sampled through the same stable [`stippled`] hash as the air veils (with its
/// own `k`, so the two patterns do not correlate), which is what keeps the
/// arrows from crawling as the camera moves.
const MOTION_ARROWS: usize = 2600;
/// Below this a column is not going anywhere worth drawing.
const MOTION_FLOOR: f32 = 0.02;
/// Legibility gain on arrow length. A full step is ONE HEX, which at freq 96 is
/// about 1% of the radius — true to scale and far too small to read — so the
/// shafts are drawn longer. The same exaggeration the shell radii already
/// carry, and for the same reason.
const MOTION_GAIN: f32 = 3.2;
/// Arrowhead barb length, as a fraction of the shaft.
const MOTION_BARB: f32 = 0.34;
/// Radius the headings are drawn at — just clear of the outermost rock, so they
/// read over the ground without z-fighting it.
const R_MOTION: f32 = 1.012 * RADIUS;

// ── The graticule ──
/// Radius the reference lines are drawn at — clear of the outermost rock and of
/// the motion arrows, so the grid reads as a frame around the world rather than
/// something lying on it. The frame itself is the SHARED [`globe::graticule`]
/// (one grid for every bench's globe; Populous draws the identical value).
const R_GRID: f32 = 1.022 * RADIUS;

/// **Where the ground is going**, as one arrow per sampled column, grouped by
/// plate colour so a raft reads as a raft.
///
/// The shaft is the column's own carried heading and its length is how far
/// along its next one-hex step it has come — the honest read, because a plate
/// STEPS: between steps a velocity arrow would sit frozen while this one fills.
/// Each arrow therefore points at the cell the column is about to move into,
/// and lengthens until it does.
fn motion_arrows(
    dirs: &[Vec3],
    cells: &[CellView],
    plates: bool,
    visible: impl Fn(usize) -> bool,
) -> globe_view::Arrows {
    let n = dirs.len();
    if n == 0 {
        return Vec::new();
    }
    // Mean centre-to-centre spacing on the unit sphere: equal-area cells, so
    // √(area) = √(4π/N). Derived from the grid in play rather than pinned to a
    // frequency, exactly as `cell_area_m2` is.
    let step_len = RADIUS * (4.0 * std::f32::consts::PI / n as f32).sqrt() * MOTION_GAIN;
    let coverage = (MOTION_ARROWS as f64 / n as f64).min(1.0);
    let mut groups: globe_view::Arrows = Vec::new();
    for (i, c) in cells.iter().enumerate().take(n) {
        if c.motion_step < MOTION_FLOOR || !visible(i) || !stippled(i, 977, coverage) {
            continue;
        }
        let heading = c.motion_dir;
        if heading.length_squared() < 1e-12 {
            continue;
        }
        let from = dirs[i] * R_MOTION;
        let shaft = heading * (step_len * c.motion_step);
        let to = from + shaft;
        // The barbs, laid in the tangent plane at this cell so the head reads
        // from any angle the globe is turned to.
        let side = dirs[i].cross(heading).normalize_or_zero() * (shaft.length() * MOTION_BARB);
        let back = shaft.normalize_or_zero() * (shaft.length() * MOTION_BARB);
        // Diffuse lithosphere (plate 0) is nobody's raft; grey says so.
        let rgb = if plates {
            plate_color(c.plate)
        } else {
            [0.85, 0.88, 0.95]
        };
        let color = [rgb[0], rgb[1], rgb[2], 1.0];
        let slot = match groups.iter_mut().find(|(k, _)| *k == color) {
            Some(s) => &mut s.1,
            None => {
                groups.push((color, Vec::new()));
                &mut groups.last_mut().expect("just pushed").1
            }
        };
        slot.push((from, to));
        slot.push((to, to - back + side));
        slot.push((to, to - back - side));
    }
    groups
}

/// The stipple coverage of each air shell, in snapshot order: the sqrt-scaled
/// column read (a trace shows as flecks), **squeezed so the stack's combined
/// occlusion never hides the world**. Every shell's optical depth is scaled by
/// one shared factor, so the between-gas ratios — the actual read — survive the
/// cap. Without this, a magma-era burst saturates all five shells and the
/// readout turns into a white ball with a planet somewhere inside it.
fn veil_coverages(shells: &[(u16, f32)]) -> Vec<f64> {
    let raw: Vec<f64> = shells
        .iter()
        .map(|&(_, column)| {
            (column as f64 / FULL_AIR_KG_M2)
                .sqrt()
                .clamp(0.0, MAX_SHELL_COVERAGE)
        })
        .collect();
    let tau: f64 = raw.iter().map(|&c| -(1.0 - c).ln()).sum();
    let squeeze = if tau > MAX_STACK_TAU {
        MAX_STACK_TAU / tau
    } else {
        1.0
    };
    // (1−c)^squeeze = e^(−squeeze·τ) — the τ-space scale, back in coverage.
    raw.into_iter()
        .map(|c| 1.0 - (1.0 - c).powf(squeeze))
        .collect()
}

/// A fresh seed from the wall clock — a new initial condition (spec §3.5). Launch
/// and **R** roll a new one; a given run stays deterministic.
fn clock_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Which shell a view paints — declared per row in [`FIELD_ACTIONS`], so the
/// field→shell binding is DATA rather than two `match` arms that can disagree
/// or silently omit a field.
#[derive(Copy, Clone, PartialEq, Debug)]
enum Paints {
    /// The mantle shell, under the crust — a read of the interior.
    Interior,
    /// The crust shells: what is happening ON the world, where it can be seen.
    Surface,
    /// Nothing. The ground keeps its own rock colour and the view draws its own
    /// geometry over the top.
    Overlay,
}

/// Which shell this view paints. One lookup, one roster.
fn paints(field: Field) -> Paints {
    FIELD_ACTIONS
        .iter()
        .find(|&&(_, f, _, _)| f == field)
        .map(|&(_, _, _, p)| p)
        .unwrap_or(Paints::Surface)
}

/// Which field the globe is coloured by.
///
/// The first four are **interior** reads and recolour the mantle shell, with the
/// crust shells drawn above in their own colours. The rest are **surface** reads
/// and recolour the crust itself, because that is where those fields live.
///
/// [`Field::Motion`] is the exception and deliberately so: it recolours nothing,
/// because what it has to say is a DIRECTION. It leaves the ground in its own
/// rock colours — so you can see what is moving, not just that something is —
/// and draws the headings over the top.
#[derive(Copy, Clone, PartialEq, Debug)]
enum Field {
    Temperature,
    Differentiation,
    Plates,
    Seams,
    Elevation,
    Coast,
    Motion,
    Rain,
    Strata,
    Ore,
}

impl Field {
    /// The next view in the strip — the ONE ordering, shared by the tab keys and
    /// [`FIELD_ACTIONS`] (pinned equal by `the_view_roster_agrees_with_itself`).
    fn cycle(self) -> Self {
        match self {
            Field::Temperature => Field::Differentiation,
            Field::Differentiation => Field::Plates,
            Field::Plates => Field::Seams,
            Field::Seams => Field::Elevation,
            Field::Elevation => Field::Coast,
            Field::Coast => Field::Motion,
            Field::Motion => Field::Rain,
            Field::Rain => Field::Strata,
            Field::Strata => Field::Ore,
            Field::Ore => Field::Temperature,
        }
    }

    /// The view `processes.json` names, e.g. `"heat"` or `"motion"`.
    ///
    /// The vocabulary is the **button's own label token** without its prefix —
    /// which is to say, content names a view by the word printed on the button
    /// a maintainer would press. Deriving it from [`FIELD_ACTIONS`] rather than
    /// listing it again is what keeps that true: rename the label and the
    /// content name follows, with `every_authored_view_names_a_real_one` there
    /// to catch the entries that did not.
    fn from_view(name: &str) -> Option<Self> {
        FIELD_ACTIONS
            .iter()
            .find(|(_, _, token, _)| token.strip_prefix("$chem_field_") == Some(name))
            .map(|&(_, field, _, _)| field)
    }

    /// The name content uses for this view — the inverse of [`Field::from_view`].
    fn view_name(self) -> &'static str {
        view_token(self).trim_start_matches("$chem_field_")
    }
}

pub struct GodModeScene {
    // ── sim (on its own thread) ──
    sim: SimHandle,
    seed: u64,

    // ── static topology (received once) ──
    dirs: Vec<Vec3>,
    outlines: Vec<Vec<Vec3>>,
    budget_dist: Vec<(u8, String, f64)>,
    /// `processes.json`'s own entries, keyed by stage name — the gate card's
    /// WHAT and WATCH-FOR prose, and the `view` each process says shows it
    /// working. The file the maintainer edits is what the bench says and which
    /// instrument it points at.
    process_defs: std::collections::HashMap<String, ProcessDef>,
    gas_names: Vec<(u16, String)>,
    ready: bool,

    // ── latest published frame ──
    snap: Option<Snapshot>,
    last_gen: u64,

    // ── view ──
    /// **The globe, whole.** The shell stack, the offscreen target, the authored
    /// stage, the orbit camera and the rect the walker reserved all live in the
    /// one component — this bench publishes what the planet is MADE OF and the
    /// world owns the picture of it.
    world: GlobeWorld,
    field: Field,
    /// Cutaway: drop a 90° wedge out of every shell above the core, so the stack
    /// reads in section instead of only from outside.
    cut: bool,
    /// Draw the classified air shells (A toggles) — on by default: the exhale
    /// is the point.
    air: bool,
    /// Draw the reference frame (equator, tropics, polar circles, prime
    /// meridian, and a 30° grid). Off by default — it is a frame for reading
    /// positions, not something to look at the planet through.
    grid: bool,
    /// The gate console popup (G, the processes chip, or the pause summary's
    /// GATES button) — the simulation's control surface, closed by default.
    gates_open: bool,
    /// The bulk-seed element panel (B toggles) — reference data, off by
    /// default: the screen does not carry everything at once.
    seed_shown: bool,
    /// The Starter console (S, or the pause summary) — the NEW-WORLD knobs.
    starter_open: bool,
    /// The Starter's knob roster, `(atomic number, symbol)` — from StaticData.
    seed_elements: Vec<(u8, String)>,
    /// Pending endowment multipliers, one per roster entry. Boundary conditions
    /// for the NEXT forge — dialing them moves nothing in the running world.
    pending_scales: Vec<f64>,
    /// Pending planet size for the next forge, in icosphere frequency.
    pending_freq: u32,
    /// A new StaticData arrived (a forge changed size or endowment): the render
    /// thread must drop every mesh built on the old topology.
    topology_stale: bool,
    /// The `at_myr` of the newest gate transition the maintainer has already
    /// read. The pause summary shows a transition exactly once: it is a report
    /// of something that just happened, not a state of the world.
    gate_ack: f64,
    /// The tile last looked inside — the truth migration for one cell, shown as the
    /// ground it produced. Uploaded once when it arrives, not per frame.
    tile: Option<(TextureHandle, u32, String)>,
    /// The active frame's ramp extremes — what the legend's end labels print.
    ranges: LegendRanges,
    /// Whether the erosion batch is running. A scene-side MIRROR: `ErodeToggle`
    /// is fire-and-forget to the worker thread and the sim publishes no echo of
    /// it, so both sides start false and stay in step by counting the same
    /// presses.
    eroding: bool,

    // ── shell ──
    theme: Option<Theme>,
    white: Option<TextureHandle>,

    // ── input bus (spec §5/§9) ──
    /// World-context bindings (Esc → `Menu`); the resolver resolves the active map.
    /// The camera + the discrete keys below stay raw, so only `Menu` rides the bus.
    bindings: ContextualBindings,
    /// Gamepad axis/deadzone config, handed to the resolver and the pause overlay.
    gamepad_config: GamepadConfig,
    /// Stateful edge resolver — the single home of previous-frame state (replaces the
    /// hand-rolled `prev_menu` bool).
    resolver: Resolver,
    /// Reused `Fired` scratch buffer (no per-frame alloc — RT-7).
    ev: Vec<Fired>,
    /// The router's per-frame request queue (context/focus intents; none arise here).
    route: RouteCtx,
    /// Monotonic frame tick — the resolver's `TickTime` (NOT wall-clock — spec §3.2a).
    tick: u64,

    // ── The declarative HUD (S10): a walker tree replaces the immediate text
    // readout + conservation ledger. The tree + the screen's declared intents are
    // cached at enter; every control draws in the engine, so there is no script
    // host here at all. ──
    ui_intents: UiIntents,
    ui_styles: serde_json::Value,
    ui_state: UiState,
    /// Draw commands stashed by `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// Intent names fired last frame — republished ONCE into the next Model as
    /// the transient `sig_<name>` mirror (S9 ruling), then dropped.
    fired_sigs: Vec<String>,
}

impl GodModeScene {
    pub fn new() -> Self {
        let seed = clock_seed();
        // Read here rather than in `enter`: the globe's authored look
        // (`stages.godmode_globe` — the light it is seen by and the backdrop it
        // sits on) is what the world is BUILT from, so the styles must exist
        // before the world does.
        let ui_styles = flicker::ui::load_shared_styles(Some(&crate::scene_styles()));
        let world = GlobeWorld::new(globe_view::STAGE_SOURCE, &ui_styles, None);
        Self {
            sim: SimHandle::spawn(seed),
            seed,
            dirs: Vec::new(),
            outlines: Vec::new(),
            budget_dist: Vec::new(),
            process_defs: std::collections::HashMap::new(),
            gas_names: Vec::new(),
            ready: false,
            snap: None,
            last_gen: 0,
            world,
            field: Field::Temperature,
            cut: false,
            air: true,
            grid: false,
            gates_open: false,
            seed_shown: false,
            starter_open: false,
            seed_elements: Vec::new(),
            pending_scales: Vec::new(),
            pending_freq: PLANET_FREQ,
            topology_stale: false,
            gate_ack: f64::NEG_INFINITY,
            tile: None,
            ranges: LegendRanges::default(),
            eroding: false,
            theme: None,
            white: None,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            ui_intents: UiIntents::default(),
            ui_styles,
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            fired_sigs: Vec::new(),
        }
    }

    /// **Write the `legend.*` style block** the legend card's swatches resolve
    /// through — GENERATED from the very colour functions that paint the globe,
    /// at enter, so the chip beside a word and the hex on the sphere are one
    /// source. Nothing here is authored; ui_theme.json never carries a copy
    /// that could drift.
    fn inject_legend_styles(&mut self) {
        let rgba = |c: [f32; 3]| serde_json::json!([c[0], c[1], c[2], 1.0]);
        let mut block = serde_json::Map::new();
        // The ramp strips, each sampled from its own paint function at the
        // same normalised stops the strip's chips sit at.
        for k in 0..LEGEND_STRIP {
            let t = k as f32 / (LEGEND_STRIP - 1) as f32;
            block.insert(format!("heat_{k}"), rgba(temp_color(t)));
            block.insert(format!("core_{k}"), rgba(diff_color(t)));
            block.insert(format!("relief_{k}"), rgba(elevation_color(t, 0.0, 1.0)));
            block.insert(format!("rain_{k}"), rgba(rain_color(t, 1.0)));
            block.insert(
                format!("strata_{k}"),
                rgba(strata_color(k as u8, (LEGEND_STRIP - 1) as u8)),
            );
            // Ore's ramp is logarithmic, so the strip walks the exponent.
            block.insert(
                format!("ore_{k}"),
                rgba(ore_color(1000.0f32.powf(t), 1000.0)),
            );
        }
        // The categorical swatches — the four grounds and their lit coastline,
        // the three seam kinds, and the plate/loose pair.
        block.insert("coast_bare".into(), rgba(coast_color(SHELF_NONE)));
        block.insert("coast_land".into(), rgba(coast_color(SHELF_LAND)));
        block.insert("coast_shelf".into(), rgba(coast_color(SHELF_SHELF)));
        block.insert("coast_bed".into(), rgba(coast_color(SHELF_BED)));
        block.insert("coast_exposed".into(), rgba(coast_color(SHELF_EXPOSED)));
        block.insert(
            "coast_edge".into(),
            rgba(coast_color(SHELF_LAND | SHELF_EDGE)),
        );
        block.insert("seam_int".into(), rgba(seam_color(0)));
        block.insert("seam_div".into(), rgba(seam_color(1)));
        block.insert("seam_conv".into(), rgba(seam_color(2)));
        block.insert("seam_trans".into(), rgba(seam_color(3)));
        block.insert("plate_sample".into(), rgba(plate_color(7)));
        block.insert("plate_loose".into(), rgba(plate_color(0)));
        // The invisible swatch a text-only row hangs its indent on.
        block.insert("blank".into(), serde_json::json!([0.0, 0.0, 0.0, 0.0]));
        if let Some(styles) = self.ui_styles.as_object_mut() {
            styles.insert("legend".into(), serde_json::Value::Object(block));
        }
    }

    /// **What the colours mean, for the view being looked at** — the legend
    /// card's model. Categorical views publish swatch+label rows; continuous
    /// ones publish the ramp strip with its ends labelled from the SAME
    /// extremes the mesh ramp stretched over ([`LegendRanges`]), so the card
    /// and the sphere describe one picture.
    fn legend_model(&self, m: &mut ValueMap) {
        let path = |name: &str| format!("legend.{name}");
        let row = |m: &mut ValueMap, n: usize, swatch: &str, label: String| {
            m.set(format!("legend_r{n}_shown"), true);
            m.set(format!("legend_r{n}_c"), path(swatch));
            m.set(format!("legend_r{n}"), label);
        };
        let strip = |m: &mut ValueMap, family: &str, lo: String, hi: String| {
            m.set("legend_strip_shown", true);
            for k in 0..LEGEND_STRIP {
                m.set(format!("legend_g{k}"), path(&format!("{family}_{k}")));
            }
            m.set("legend_lo", lo);
            m.set("legend_hi", hi);
        };
        let t = |token: &str| strings::resolve(token).into_owned();

        m.set("legend_shown", true);
        let r = &self.ranges;
        match self.field {
            Field::Temperature => strip(
                m,
                "heat",
                format!("{} · {:.0} K", t("$chem_legend_cold"), r.tmin),
                format!("{:.0} K · {}", r.tmax, t("$chem_legend_hot")),
            ),
            Field::Differentiation => strip(
                m,
                "core",
                t("$chem_legend_diff_none"),
                t("$chem_legend_diff_done"),
            ),
            Field::Plates => {
                row(m, 1, "plate_sample", t("$chem_legend_plate_each"));
                row(m, 2, "plate_loose", t("$chem_legend_plate_loose"));
            }
            Field::Seams => {
                row(m, 1, "seam_conv", t("$chem_legend_seam_conv"));
                row(m, 2, "seam_div", t("$chem_legend_seam_div"));
                row(m, 3, "seam_trans", t("$chem_legend_seam_trans"));
                row(m, 4, "seam_int", t("$chem_legend_seam_int"));
            }
            Field::Elevation => strip(
                m,
                "relief",
                format!("\u{2264} {:.0} m", r.elo),
                format!("\u{2265} {:.0} m", r.ehi),
            ),
            Field::Coast => {
                row(m, 1, "coast_land", t("$chem_legend_land"));
                row(m, 2, "coast_shelf", t("$chem_legend_shelf"));
                row(m, 3, "coast_bed", t("$chem_legend_ocean_bed"));
                row(m, 4, "coast_exposed", t("$chem_legend_exposed"));
                row(m, 5, "coast_bare", t("$chem_legend_bare"));
                row(m, 6, "coast_edge", t("$chem_legend_coastline"));
            }
            Field::Motion => {
                row(m, 1, "blank", t("$chem_legend_motion_dir"));
                row(m, 2, "blank", t("$chem_legend_motion_len"));
                row(m, 3, "plate_sample", t("$chem_legend_motion_col"));
            }
            Field::Rain => strip(
                m,
                "rain",
                t("$chem_legend_dry"),
                format!("{:.2} m", r.wettest),
            ),
            Field::Strata => strip(
                m,
                "strata",
                "1".to_string(),
                format!("{} {}", r.deepest.max(1), t("$chem_legend_beds")),
            ),
            Field::Ore => strip(
                m,
                "ore",
                t("$chem_legend_ore_bg"),
                format!(
                    "{:.0}\u{00d7} · {}",
                    r.richest.max(1.0),
                    t("$chem_legend_ore_rich")
                ),
            ),
        }
    }

    /// The per-frame HUD model: every readout line pre-formatted (the tree's
    /// `text_bind`s display them verbatim), the `loading`/`loaded` state gates,
    /// the state-word + ledger-status colour paths (`color_bind`s), plus the
    /// transient `sig_<name>` mirror of last frame's fired intents.
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::new();
        match self.snap.as_ref() {
            None => {
                m.set("loading", true);
            }
            Some(snap) => {
                let s = &snap.state;
                m.set("loaded", true);
                // Every English word below is a stringtable token (Model-channel
                // strings gate); the numbers and unit symbols compose around them.
                m.set(
                    "stats",
                    format!(
                        "{} {}  ·  {:.1} My  ·  {} {}",
                        strings::resolve("$chem_tick"),
                        snap.tick,
                        snap.tick_myr,
                        snap.swept_cells,
                        strings::resolve("$chem_cells"),
                    ),
                );
                // The readout lives in a 332-wide column now, so each reading
                // is two short lines rather than one wide one. Splitting at
                // publish keeps the composition in Rust where the rest of the
                // formatting already is.
                let core_pct = s.core_mass_kg / s.planet_mass_kg.max(1.0) * 100.0;
                m.set(
                    "interior",
                    format!(
                        "{} {core_pct:.1}%  ·  {} {:.0}%",
                        strings::resolve("$chem_core"),
                        strings::resolve("$chem_differentiated"),
                        s.differentiation_frac * 100.0,
                    ),
                );
                m.set(
                    "interior2",
                    format!(
                        "{} {:.0} K  ·  {} {}  ·  {} {:.0} TW",
                        strings::resolve("$chem_mantle"),
                        s.mean_mantle_temp_k,
                        snap.plate_count,
                        strings::resolve("$chem_plates"),
                        strings::resolve("$chem_radiogenic"),
                        s.radiogenic_power_tw,
                    ),
                );

                // ── The transport: what the buttons say and where the dial
                //    stands. The dial echoes the sim's OWN rate, so a clamped
                //    request springs back instead of lying. ──
                m.set(
                    "play_label",
                    strings::resolve(if snap.playing {
                        "$chem_pause"
                    } else {
                        "$chem_play"
                    })
                    .into_owned(),
                );
                m.set("rate", snap.rate_hz as f64);
                m.set("cut", self.cut);
                m.set("air", self.air);
                m.set("grid", self.grid);
                m.set(
                    "erode_label",
                    strings::resolve(if self.eroding {
                        "$chem_erode_off"
                    } else {
                        "$chem_erode_on"
                    })
                    .into_owned(),
                );
                // The pixel stage's era gate: RAIN ON waits for the five-axis
                // life-supporting light; RAIN OFF is always reachable.
                m.set(
                    "rain_allowed",
                    self.eroding || snap.habitability.life_supporting,
                );
                // What the active view's colours mean — the card floating under
                // the globe.
                self.legend_model(&mut m);

                // ── The tab strip, in three brightnesses. ──
                //
                // Ten equally-lit buttons tell you nothing about a world that
                // spends four and a half billion years doing very different
                // things. So: the view you are IN is brightest; a view some
                // RUNNING process says shows its work is normal; the rest go
                // dim. The strip is then a reading of the era — during the
                // magma age heat and core stand out, once the conveyor starts
                // motion does, and when the rain arrives so does rain.
                //
                // A SUGGESTION and never a seizure: nothing here changes the
                // view, it only says where something is happening.
                let suggested: Vec<Field> = snap
                    .processes
                    .iter()
                    .filter(|p| p.running())
                    .filter_map(|p| self.process_defs.get(p.name))
                    .filter_map(|def| Field::from_view(&def.view))
                    .collect();
                for (action, field, _, _) in FIELD_ACTIONS {
                    m.set(
                        format!("{action}_style"),
                        if self.field == field {
                            "modal.buttons.variants.primary"
                        } else if suggested.contains(&field) {
                            "modal.buttons.variants.secondary"
                        } else {
                            "modal.buttons.variants.ghost"
                        },
                    );
                }
                // Every rate lever echoes as a MULTIPLE of the physics as
                // written, which is the same vocabulary the rack's sliders and
                // the dispatcher's guard use.
                let base = Levers::default();
                for &(key, get, _) in LEVERS {
                    let d = get(&base);
                    m.set(
                        key,
                        if d.abs() > 0.0 {
                            get(&snap.levers) / d
                        } else {
                            0.0
                        },
                    );
                }
                m.set(
                    "water_infall",
                    snap.levers.water_budget_kg / flicker_poc_chemistry::surface::DEFAULT_WATER_KG,
                );
                m.set("water_coverage", snap.levers.water_coverage_target);

                // ── The tile inspector's picture and caption. ──
                m.set("has_tile", self.tile.is_some());
                if let Some((_, _, caption)) = self.tile.as_ref() {
                    // strings-gate-exempt: the caption is sim-composed measurement
                    // text (cell id, bed count, height range), not UI copy.
                    m.set("tile_caption", caption.clone());
                }

                let (word, color) = if snap.playing {
                    ("$chem_playing", "chemistry.playing.color")
                } else {
                    ("$chem_paused", "chemistry.paused.color")
                };
                m.set("play_state", strings::resolve(word).into_owned());
                m.set("play_state_color", color);
                // ── The process panel: what every stage is doing, and why. ──
                //
                // Three states, and the third is the one that earns its keep: a
                // process that is neither held nor running is WAITING ON THE WORLD,
                // and the row says what for. Reporting that as "stopped" would hide
                // the difference between a bug and a planet that is not ready yet.
                for (i, p) in snap.processes.iter().enumerate().take(PROCESS_ROWS) {
                    let n = i + 1;
                    let (mark, state, color) = if p.held {
                        (
                            "\u{2298}",
                            strings::resolve("$chem_held").into_owned(),
                            "chemistry.held",
                        )
                    } else if p.ready {
                        (
                            "\u{25cf}",
                            strings::resolve("$chem_running").into_owned(),
                            "chemistry.ok",
                        )
                    } else {
                        (
                            "\u{25cb}",
                            strings::resolve("$chem_waiting").into_owned(),
                            "chemistry.waiting",
                        )
                    };
                    m.set(format!("proc_{n}"), format!("{mark} {:<18}{state}", p.name));
                    m.set(format!("proc_{n}_color"), color);
                    m.set(format!("proc_{n}_shown"), true);
                    // The console row's control: holding is ARM/RELEASE, the one
                    // sanctioned per-process lever.
                    m.set(
                        format!("hold_{n}_label"),
                        strings::resolve(if p.held {
                            "$chem_release"
                        } else {
                            "$chem_hold"
                        })
                        .into_owned(),
                    );
                }
                for n in snap.processes.len() + 1..=PROCESS_ROWS {
                    m.set(format!("proc_{n}_shown"), false);
                }

                // The processes CHIP — the one line that stands in for the whole
                // list on the default screen. Clicking it (or G) opens the gate
                // console; the counts say whether opening it is worth your time.
                let (mut running, mut waiting, mut held) = (0usize, 0usize, 0usize);
                for p in &snap.processes {
                    if p.held {
                        held += 1;
                    } else if p.ready {
                        running += 1;
                    } else {
                        waiting += 1;
                    }
                }
                // The chip SAYS it is the gates door — it spent its first day
                // reading as a status line, and the only labelled way in that
                // anyone found was the pause summary's GATES… button.
                let mut chip = format!(
                    "\u{2699} {}  ·  {running} {}  ·  {waiting} {}",
                    strings::resolve("$chem_gates_chip"),
                    strings::resolve("$chem_running"),
                    strings::resolve("$chem_waiting"),
                );
                if held > 0 {
                    chip.push_str(&format!("  ·  {held} {}", strings::resolve("$chem_held")));
                }
                m.set("proc_summary", chip);
                m.set("gates_open", self.gates_open);
                // The coverage slider shows the lever as it stands (1.00 = no
                // cutoff — the disabled position is on the scale, not a mode).
                m.set(
                    "water_coverage",
                    snap.levers.water_coverage_target.clamp(0.0, 1.0),
                );
                // And the infall dial as a multiple of the Earth-scale default —
                // the third water control: H endowment (Starter), delivery
                // (this), coverage cutoff (below it).
                m.set(
                    "water_infall",
                    (snap.levers.water_budget_kg
                        / flicker_poc_chemistry::surface::DEFAULT_WATER_KG)
                        .clamp(0.0, 10.0),
                );

                // ── The Starter: NEW-WORLD knobs, published from the scene's
                //    pending state (not the snapshot — these are conditions for
                //    the NEXT forge, and the running world ignores them). ──
                m.set("starter_open", self.starter_open);
                for (i, (_, sym)) in self.seed_elements.iter().enumerate() {
                    let n = i + 1;
                    m.set(format!("seed_el_{n}_label"), sym.clone());
                    m.set(
                        format!("seed_el_{n}"),
                        self.pending_scales.get(i).copied().unwrap_or(1.0),
                    );
                    m.set(format!("seed_el_{n}_shown"), true);
                }
                for n in self.seed_elements.len() + 1..=12 {
                    m.set(format!("seed_el_{n}_shown"), false);
                }
                m.set("seed_freq", self.pending_freq as f64);
                m.set(
                    "seed_cells",
                    format!(
                        "{} {}",
                        10 * self.pending_freq * self.pending_freq + 2,
                        strings::resolve("$chem_cells"),
                    ),
                );

                // The hints line is gone: every key it advertised is a visible
                // control now, and a surface that has to explain its own
                // keyboard shortcuts in a status line is one that did not put
                // them on screen.
                m.set(
                    "crust",
                    format!(
                        "{} {:.3}%  ·  {} {:.0}%  ·  {} {:.1}/{}",
                        strings::resolve("$chem_crust"),
                        s.crust_frac * 100.0,
                        strings::resolve("$chem_continental"),
                        s.continental_frac * 100.0,
                        strings::resolve("$chem_strata"),
                        s.mean_strata,
                        s.max_strata,
                    ),
                );
                m.set(
                    "crust2",
                    format!(
                        "{} {:.0} m  ·  {} {:.0} m  ·  {} {:.0}%",
                        strings::resolve("$chem_mean_elevation"),
                        s.mean_elevation_m,
                        strings::resolve("$chem_sea_level"),
                        s.sea_level_m,
                        strings::resolve("$chem_submerged"),
                        s.submerged_frac * 100.0,
                    ),
                );

                // ── The air line: what the mantle has exhaled, and what it does. ──
                //
                // The gas label is catalog data (formula), same register as the
                // element symbols in the seed panel. Airless is a state worth a
                // word of its own: "no line" would read as a bug, not a fact.
                // **`air_line`, not `air`.** The checkbox binds `air`, and this
                // readout used to overwrite it with its own TEXT — so the box
                // read a string, `is_on` said false, and it snapped back off the
                // instant it was ticked (Aaron, 2026-08-06). One Model, one key
                // per meaning.
                m.set(
                    "air_line",
                    match s.dominant_gas {
                        None => format!(
                            "{} {}",
                            strings::resolve("$chem_air"),
                            strings::resolve("$chem_air_none"),
                        ),
                        Some(id) => {
                            let gas = self
                                .gas_names
                                .iter()
                                .find(|(g, _)| *g == id)
                                .map(|(_, n)| n.as_str())
                                .unwrap_or("?");
                            format!(
                                "{} {gas}  ·  {} {}  ·  {} +{:.0} K",
                                strings::resolve("$chem_air"),
                                strings::resolve("$chem_pco2"),
                                fmt_pressure(s.p_co2),
                                strings::resolve("$chem_greenhouse"),
                                s.greenhouse_k,
                            )
                        }
                    },
                );

                // ── Why the run stopped. A gate moving is the world crossing one
                //    of its own condition thresholds, and the sim pauses itself
                //    there; this line is what it stopped on. (The stage name is
                //    an identifier — the same string the conservation audit would
                //    name — not display copy.)
                match snap.gate_events.last() {
                    None => m.set("gate_shown", false),
                    Some(g) => {
                        m.set("gate_shown", true);
                        m.set(
                            "gate",
                            format!(
                                "\u{23f8} {} {}  ·  {:.0} My",
                                g.stage,
                                strings::resolve(if g.opened {
                                    "$chem_gate_opened"
                                } else {
                                    "$chem_gate_shut"
                                }),
                                g.at_myr,
                            ),
                        );
                        m.set(
                            "gate_color",
                            if g.opened {
                                "chemistry.ok"
                            } else {
                                "chemistry.waiting"
                            },
                        );

                        // ── The pause summary. A gate moving is the world
                        //    crossing one of its own thresholds — an epoch
                        //    boundary in embryo — so the sim stops there and
                        //    says WHY, once. Shown only while the run is
                        //    actually stopped and the maintainer has not yet
                        //    acknowledged this transition.
                        let unread = g.at_myr > self.gate_ack;
                        m.set("gate_pause_shown", !snap.playing && unread);
                        m.set(
                            "gate_headline",
                            format!(
                                "{} {}",
                                g.stage,
                                strings::resolve(if g.opened {
                                    "$chem_gate_opened"
                                } else {
                                    "$chem_gate_shut"
                                }),
                            ),
                        );
                        m.set(
                            "gate_why",
                            strings::resolve(gate_reason(g.stage, g.opened)).into_owned(),
                        );
                        m.set(
                            "gate_cause",
                            format!(
                                "{} {:.0} My  ·  {} {:.1}%  ·  {} {:.0} K  ·  {} {}",
                                strings::resolve("$chem_gate_at"),
                                g.at_myr,
                                strings::resolve("$chem_crust"),
                                s.crust_frac * 100.0,
                                strings::resolve("$chem_mantle"),
                                s.mean_mantle_temp_k,
                                strings::resolve("$chem_life"),
                                strings::resolve(snap.life.token()),
                            ),
                        );
                        m.set(
                            "gate_effect",
                            strings::resolve("$chem_gate_effect").into_owned(),
                        );
                        // The card's WHAT and WATCH-FOR paragraphs come from
                        // processes.json (via StaticData) — authored content,
                        // the same file that defines the gate itself.
                        let def = self.process_defs.get(g.stage);
                        let (what, watch) = def
                            .map(|d| (d.summary.clone(), d.watch.clone()))
                            .unwrap_or_default();
                        // …and so does the instrument to look at. The card stops
                        // at telling you the world changed and makes going to
                        // SEE it one press — but only when the process that
                        // moved actually has something to show, so the button is
                        // absent rather than lying on the six that do not.
                        let view = def.and_then(|d| Field::from_view(&d.view));
                        m.set("gate_view_shown", view.is_some());
                        if let Some(v) = view {
                            m.set(
                                "gate_view_label",
                                strings::resolve(view_token(v)).into_owned(),
                            );
                        }
                        m.set("gate_what", what);
                        m.set(
                            "gate_watch",
                            if watch.is_empty() {
                                String::new()
                            } else {
                                format!("{} {watch}", strings::resolve("$chem_gate_watch_head"))
                            },
                        );
                        // Progress scales: how far the slow ledgers have come.
                        // The lever is a reference-scale (freq-96) budget; the
                        // delivered ledger is this world's own, so the progress
                        // read sizes the lever the same way the sim does.
                        let water_goal = snap.levers.water_budget_kg
                            * flicker_poc_chemistry::size_scale(snap.cells.len()).powi(3);
                        let water_pct = if water_goal > 0.0 {
                            (s.delivered_water_kg / water_goal).min(1.0) * 100.0
                        } else {
                            0.0
                        };
                        m.set(
                            "gate_progress",
                            format!(
                                "{} {water_pct:.0}%  ·  {} {:.2e} kg  ·  {} {:.0}%  ·  {} {:.0}%",
                                strings::resolve("$chem_prog_water"),
                                strings::resolve("$chem_prog_bound"),
                                s.compounds_kg,
                                strings::resolve("$chem_prog_lid"),
                                s.lid_frac * 100.0,
                                strings::resolve("$chem_prog_core"),
                                s.differentiation_frac * 100.0,
                            ),
                        );
                    }
                }

                // ── The life line: how far the biosphere got, and what it has
                //    put in the ground. Fuel appears only once something has
                //    been buried long enough to cook. ──
                m.set(
                    "life",
                    format!(
                        "{} {}  ·  {} {}",
                        strings::resolve("$chem_life"),
                        strings::resolve(snap.life.token()),
                        strings::resolve("$chem_tissue"),
                        fmt_mass(snap.tissue_kg),
                    ),
                );
                // **Where the water is.** Every tick the ledger balances, so a
                // shrinking sea is always one of the other three growing — and
                // saying so on screen is the difference between a mechanism and
                // a suspected leak.
                m.set(
                    "water",
                    format!(
                        "{}  {} {}  ·  {} {}  ·  {} {}  ·  {} {}",
                        strings::resolve("$chem_water_line"),
                        strings::resolve("$chem_water_sea"),
                        fmt_mass(snap.water_sea_kg),
                        strings::resolve("$chem_water_sky"),
                        fmt_mass(snap.water_sky_kg),
                        strings::resolve("$chem_water_life"),
                        fmt_mass(snap.water_life_kg),
                        strings::resolve("$chem_water_stone"),
                        fmt_mass(snap.water_stone_kg),
                    ),
                );
                m.set(
                    "life2",
                    format!(
                        "{} {}  ·  {} {}",
                        strings::resolve("$chem_coal"),
                        fmt_mass(snap.coal_kg),
                        strings::resolve("$chem_oils"),
                        fmt_mass(snap.oils_kg),
                    ),
                );

                // ── The event log: plate life and gate transitions on ONE
                //    clock, newest first. Two sources, one reading — which is
                //    the point: "a plate died" and "volcanism shut" are the
                //    same kind of fact about a world that just changed, and
                //    reading them in two places is how you miss that they
                //    happened together. A fixed bank of EVENT_ROWS, per the
                //    dynamic-rows pattern.
                let mut events: Vec<(f64, String, &str)> = snap
                    .recent_events
                    .iter()
                    .map(|(myr, ev)| {
                        let (txt, color) = fmt_event(ev);
                        (*myr, txt, color)
                    })
                    .chain(snap.gate_events.iter().map(|g| {
                        (
                            g.at_myr,
                            format!(
                                "{} {}",
                                g.stage,
                                strings::resolve(if g.opened {
                                    "$chem_gate_opened"
                                } else {
                                    "$chem_gate_shut"
                                }),
                            ),
                            if g.opened {
                                "chemistry.ok"
                            } else {
                                "chemistry.held"
                            },
                        )
                    }))
                    .collect();
                events.sort_by(|a, b| b.0.total_cmp(&a.0));
                for n in 1..=EVENT_ROWS {
                    match events.get(n - 1) {
                        Some((myr, txt, color)) => {
                            m.set(format!("ev_{n}"), format!("{myr:>6.0} My  {txt}"));
                            m.set(format!("ev_{n}_color"), *color);
                            m.set(format!("ev_{n}_shown"), true);
                        }
                        None => m.set(format!("ev_{n}_shown"), false),
                    }
                }

                // ── The conservation ledger (text-only; the walker panel shows it). ──
                let present = s.core_mass_kg
                    + s.mantle_mass_kg
                    + s.crust_mass_kg
                    + s.atmosphere_mass_kg
                    + s.ocean_mass_kg
                    + s.escaped_mass_kg;
                let expected = s.planet_mass_kg + s.delivered_mass_kg;
                let balanced = (present - expected).abs() <= 1e-6 * expected.max(1.0);
                let total = expected.max(1.0);
                let pct = |mass: f64| mass / total * 100.0;
                let (status, color) = if balanced {
                    ("$chem_balanced", "chemistry.ok")
                } else {
                    ("$chem_broken", "chemistry.bad")
                };
                m.set(
                    "ledger_status",
                    format!("Σ {}  ·  {}", fmt_mass(expected), strings::resolve(status)),
                );
                m.set("ledger_status_color", color);
                let rows: [(&str, f64); 6] = [
                    ("$chem_ledger_mantle", s.mantle_mass_kg),
                    ("$chem_ledger_core", s.core_mass_kg),
                    ("$chem_ledger_crust", s.crust_mass_kg),
                    ("$chem_ledger_atmosphere", s.atmosphere_mass_kg),
                    ("$chem_ledger_ocean", s.ocean_mass_kg),
                    ("$chem_ledger_escaped", s.escaped_mass_kg),
                ];
                for (i, (label, mass)) in rows.iter().enumerate() {
                    m.set(
                        format!("ledger_{}", i + 1),
                        format!("{:<11}{:>6.2}%", strings::resolve(label), pct(*mass)),
                    );
                }

                // ── The life-supporting gauges + the gate light. Every display
                //    word rides a token (the observer hands over token KEYS). ──
                let h = &snap.habitability;
                for (i, ax) in h.axes.iter().enumerate() {
                    let n = i + 1;
                    let live = ax.signal.is_some();
                    let (status, status_color) = if live {
                        if ax.in_band() {
                            ("$chem_in_band", "pocepochs.hab.status_in")
                        } else {
                            ("$chem_out_of_band", "pocepochs.hab.status_out")
                        }
                    } else {
                        ("$chem_no_signal_yet", "pocepochs.hab.status_dead")
                    };
                    m.set(format!("a{n}_name"), strings::resolve(ax.name).into_owned());
                    m.set(
                        format!("a{n}_name_color"),
                        if live {
                            "pocepochs.hab.name_live"
                        } else {
                            "pocepochs.hab.name_dead"
                        },
                    );
                    m.set(format!("a{n}_v"), ax.signal.unwrap_or(-1.0)); // −1 = no signal
                    m.set(
                        format!("a{n}_lolab"),
                        strings::resolve(ax.low_label).into_owned(),
                    );
                    m.set(
                        format!("a{n}_hilab"),
                        strings::resolve(ax.high_label).into_owned(),
                    );
                    m.set(
                        format!("a{n}_status"),
                        strings::resolve(status).into_owned(),
                    );
                    m.set(format!("a{n}_status_color"), status_color);
                }
                let total = h.axes.len();
                if h.life_supporting {
                    // The lamp's OWN key. It was `life` once, which is the
                    // readout's life-line TEXT bind — so a life-supporting world
                    // replaced its own biosphere readout with the bool `true`
                    // and the lamp never lit, both from one name collision.
                    m.set("life_light", true);
                    m.set(
                        "verdict",
                        strings::resolve("$chem_life_supporting").into_owned(),
                    );
                    m.set("verdict_color", "pocepochs.hab.verdict_life");
                } else {
                    m.set("no_life", true);
                    m.set(
                        "verdict",
                        format!(
                            "{} / {total} {}",
                            h.axes_in_band,
                            strings::resolve("$chem_axes_in_band")
                        ),
                    );
                    m.set("verdict_color", "pocepochs.hab.verdict_count");
                }
                m.set(
                    "observed",
                    format!(
                        "{} / {total} {}",
                        h.axes_live,
                        strings::resolve("$chem_observed")
                    ),
                );
            }
        }
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }
}

impl Default for GodModeScene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene for GodModeScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0]; // deep space
        let theme = Theme::build(renderer);
        self.white = Some(theme.lua_textures()[0].1); // id 0 = "white"
        self.theme = Some(theme);

        // The declarative HUD is pure DATA: there is deliberately no
        // `hud_chemistry.lua` — a per-scene HUD script that hand-composes the
        // surface is the legacy idiom (rule E5AFBBAB); composition lives in
        // `ui_templates.json` and this scene only configures it.
        // The legend's swatch colours, GENERATED from the same colour functions
        // that paint the globe — never authored, so they cannot drift from the
        // sphere they explain.
        self.inject_legend_styles();
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.world.free(renderer);
        // The sim thread shuts down when `self.sim` (SimHandle) drops.
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        _signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        // Walk the cached HUD tree: layout + hit-test + draw in one pass. The
        // ledger panel is a styled container, so the pointer over it sets
        // `hud_hit` — fed to the walker layer below as this frame's
        // pointer-consume (the camera stays a raw poll, unchanged).
        let over_hud;
        let mut results;
        {
            let tree = self.build_tree();
            self.ui_intents = UiIntents::of(&tree);
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                screen: renderer.size(),
                typed: String::new(),
                backspace: false,
                wheel: input.mouse_wheel_delta,
            };
            let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
            over_hud = frame.results.is_on("hud_hit");
            // Where the globe landed. The walker RESERVES this rect and never
            // fills it (it runs late; offscreen passes must run first), so the
            // hand-off is: read it here, draw into it at the top of `render`.
            self.world.place(frame.rtt_rect(GLOBE_SLOT));
            // The CLICK channel. A button's `action` fires into the frame's
            // results — reading only `hud_hit` and dropping the rest is exactly
            // how CARRY ON came to do nothing: the click fired into a struct
            // nobody read, while the keyboard path (walker intents) worked.
            results = frame.results.clone();
            self.hud_commands = frame.commands;
        }

        // ── The input seam (spec §5/§9): ONE resolve + ONE dispatch replaces the raw
        // `prev_menu` edge. The resolver owns the `Menu` (Esc) press edge; the walker
        // layer's DECLARED `on_menu` intent (S10) is the pause-open edge. The orbit
        // camera + the discrete data-viewer keys below stay on the raw snapshot, but
        // read its edge log rather than hand-rolled `*_prev` bools. `ev` is the
        // REUSED `Fired` buffer; the `InputEvent` list is a short-lived local
        // (it borrows this frame's snapshot — RT-7).
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver.resolve_frame(
            &self.bindings,
            &self.gamepad_config,
            input,
            self.tick,
            &mut self.ev,
        );
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut root, &mut walker];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // Standard post-dispatch seam; no handler pushes context intents here.
        let focus_change = apply_context_requests(&mut self.bindings, &self.route.requests);
        walker.apply_focus(focus_change);
        self.fired_sigs = walker.take_fired();
        self.route.requests.clear();

        // Fold the fired intent names in beside the click results, so both
        // channels reach the ONE dispatcher identically — the Sablework idiom.
        // A click on CARRY ON and the pad's Confirm are the same event from here.
        for name in &self.fired_sigs {
            results.set(name.clone(), true);
        }

        // ONE dispatcher for both channels. It is a method rather than inline so
        // a gate can drive it directly: an arm that lives in the frame loop can
        // only be tested by running a frame, and a gate that cannot run is a
        // gate that stops being written.
        for cmd in self.apply_results(&results) {
            self.sim.send(cmd);
        }

        // The screen DECLARED `on_menu = "pause_open"` (S9/S10): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto the
        // shell pause push — the root's hardcoded Menu arm is gone.
        if results.is_on("pause_open") {
            let theme = self.theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                self.bindings.active_map(),
                &AbstractControls::default(),
                &self.gamepad_config,
            )));
        }

        // Static data arrives at spawn AND after every forge — a new size is a
        // new topology, a new endowment is a new seed readout. Always taken;
        // the meshes built on the old sphere are freed on the render pass.
        if let Some(s) = self.sim.take_static() {
            self.dirs = s.dirs;
            self.outlines = s.outlines;
            self.budget_dist = s.budget_dist;
            // A `view` the strip does not have is a typo in content, and content
            // typos are loud here rather than quietly inert: the process simply
            // would never light an instrument, and nobody would know why.
            for p in &s.processes {
                if !p.view.is_empty() && Field::from_view(&p.view).is_none() {
                    tracing::warn!(
                        "processes.json: '{}' names view '{}', which this bench does not have. \
                         The views are: {}",
                        p.runs,
                        p.view,
                        FIELD_ACTIONS
                            .iter()
                            .map(|&(_, f, _, _)| f.view_name())
                            .collect::<Vec<_>>()
                            .join(" · "),
                    );
                }
            }
            self.process_defs = s
                .processes
                .into_iter()
                .map(|p| (p.runs.clone(), p))
                .collect();
            self.gas_names = s.gas_names;
            if self.pending_scales.len() != s.seed_elements.len() {
                self.pending_scales = vec![1.0; s.seed_elements.len()];
            }
            self.seed_elements = s.seed_elements;
            if self.ready {
                self.topology_stale = true;
                self.world.mark_dirty();
            }
            self.ready = true;
        }

        // **No raw keys.** Every one of the twelve this scene used to poll
        // (Space/Down/R/S/V/A/G/B/C/T/E/1-9) is now a control on the bench or a
        // DECLARED intent on the screen root, which is what "the modern input
        // system" means: a pad press, a key and a click become the same event
        // before the scene sees any of them. A half-migrated surface answering
        // two input vocabularies at once is a tracked defect, so they went
        // together.
        // One camera line, and it is the WORLD's: the look and zoom SIGNALS, gated by the
        // focus of the panel the world names (this bench names none yet, so the globe
        // stays pointer-flown), and the pointer drag latching inside the world's own rect.
        // Disabled bench (its input-P3 is task #3): it still self-resolves, so it resolves
        // the look tuple from its OWN bindings and hands GlobeWorld the result — the world
        // no longer takes a resolver.
        let look =
            GlobeWorld::look_from(|s| self.bindings.signal_axis(s, input, &self.gamepad_config));
        self.world
            .update(dt.as_secs_f32(), input, look, self.ui_state.focused());
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        self.collect_tile(renderer);
        if !self.ready {
            // The loading banner rides the walker tree (`loading` visible_bind).
            self.draw_hud(renderer);
            return;
        }

        // A forge replaced the topology: every mesh built on the old sphere is
        // stale. The rebuild below runs on the new dirs this same frame.
        if self.topology_stale {
            self.world.mark_dirty();
            self.topology_stale = false;
        }

        // Pull the newest frame; rebuild the shells if it advanced.
        if let Some(s) = self.sim.latest_if_newer(self.last_gen) {
            self.last_gen = s.gen;
            self.ranges = LegendRanges::from_cells(&s.cells);
            self.snap = Some(s);
            self.world.mark_dirty();
        }

        if self.world.dirty() {
            // Split the borrow: the world is written while the snapshot and the
            // topology it is built from are read.
            let Self {
                world,
                snap,
                dirs,
                outlines,
                ranges,
                field,
                cut,
                air,
                grid,
                ..
            } = self;
            let mut shells: Vec<ShellSpec> = Vec::new();
            let mut arrows: globe_view::Arrows = Vec::new();
            // The core sits UNDER everything, so it leads the list: the world
            // draws the shells in order and owns only the picture, not what a
            // given bench's planet is made of.
            if !dirs.is_empty() {
                shells.push(ShellSpec {
                    dirs,
                    outlines,
                    radius: R_CORE,
                    inset: 0.0,
                    color: Box::new(|_| Some(CORE_COLOR)),
                    cell_radius: None, // every simulated shell is a sphere
                });
            }
            if let Some(snap) = snap.as_ref() {
                let view = *field;
                let dirs: &[Vec3] = dirs;
                let outlines: &[Vec<Vec3>] = outlines;
                let cells = &snap.cells;
                let (tmin, tmax) = cells.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
                    (lo.min(c.temp_k), hi.max(c.temp_k))
                });
                let tspan = (tmax - tmin).max(1.0);
                let light = Vec3::new(0.4, 0.7, 0.55).normalize();
                let lit = move |i: usize, base: [f32; 3]| {
                    let l = (dirs[i].dot(light) * 0.5 + 0.5).clamp(0.25, 1.0);
                    [base[0] * l, base[1] * l, base[2] * l]
                };
                // Every shell above the core drops the wedge, so the cut reveals the
                // whole stack in section rather than one layer at a time.
                let cut = *cut;
                let sliced = move |i: usize| cut && globe::in_wedge(dirs[i]);

                // Range of the surface fields, so their ramps span what is actually
                // there rather than a guessed scale. Elevation stretches over the
                // cached PERCENTILES, not min-max — see [`LegendRanges::elo`].
                let (elo, ehi) = (ranges.elo, ranges.ehi);
                let deepest = cells.iter().map(|c| c.strata).max().unwrap_or(0).max(1);
                let richest = cells.iter().map(|c| c.ore).fold(1.0f32, f32::max);
                let wettest = cells.iter().map(|c| c.rain).fold(0.0f32, f32::max);

                // Mantle shell — always present, coloured by the selected interior
                // field. A surface field leaves it neutral so the crust above reads
                // clearly against it.
                let mantle = move |i: usize| {
                    if sliced(i) {
                        return None;
                    }
                    let c = &cells[i];
                    // The ROSTER decides, not this match. A surface read leaves
                    // the interior neutral so what is happening ON the world
                    // reads against it.
                    let base = if paints(view) == Paints::Interior {
                        match view {
                            Field::Differentiation => diff_color(c.differentiation),
                            _ => temp_color((c.temp_k - tmin) / tspan),
                        }
                    } else {
                        BARE_MANTLE_COLOR
                    };
                    Some(lit(i, base))
                };
                shells.push(ShellSpec {
                    dirs,
                    outlines,
                    radius: R_MANTLE,
                    inset: 0.0,
                    color: Box::new(mantle),
                    cell_radius: None,
                });

                // What a crust cell is painted with: its own rock colour normally,
                // or the selected surface field when one is showing.
                let crust_color = move |i: usize, rock: [f32; 3]| -> [f32; 3] {
                    let c = &cells[i];
                    if paints(view) != Paints::Surface {
                        // Interior reads and Motion say nothing about what the
                        // ground IS, so it keeps its own rock colour.
                        return rock;
                    }
                    match view {
                        Field::Elevation => elevation_color(c.elevation_m, elo, ehi),
                        Field::Coast => coast_color(c.coast),
                        Field::Rain => rain_color(c.rain, wettest),
                        Field::Strata => strata_color(c.strata, deepest),
                        Field::Ore => ore_color(c.ore, richest),
                        // **Plates and seams paint the SURFACE**, where they can
                        // actually be seen. They used to colour the mantle shell
                        // underneath the crust, which is why the plate view read
                        // as "the shell under the lower layer changed colour".
                        Field::Plates => plate_color(c.plate),
                        Field::Seams => seam_color(c.seam),
                        _ => rock,
                    }
                };

                // The two crust shells — sparse: a bed exists only where the
                // simulation put one, and a `None` cell leaves the mantle bare.
                for (radius, bed, base) in [
                    (R_OCEANIC, BED_OCEANIC, OCEANIC_COLOR),
                    (R_CONTINENTAL, BED_CONTINENTAL, CONTINENTAL_COLOR),
                ] {
                    shells.push(ShellSpec {
                        dirs,
                        outlines,
                        radius,
                        inset: 0.0,
                        color: Box::new(move |i: usize| {
                            (!sliced(i) && cells[i].beds & bed != 0)
                                .then(|| lit(i, crust_color(i, base)))
                        }),
                        cell_radius: None,
                    });
                }

                // The headings. Line geometry rather than colour, because the
                // thing being reported is a DIRECTION — and only in the view
                // that asks for it, so nothing pays for it otherwise.
                if view == Field::Motion {
                    arrows = motion_arrows(dirs, cells, true, |i| !sliced(i));
                }
                // The reference frame rides the same line channel — one pass,
                // two consumers, and it is available in every view because
                // "where is this" is a question every view raises.
                if *grid {
                    arrows.extend(globe::graticule(R_GRID));
                }

                // The exhale, classified: one stippled veil per gas, heaviest
                // lowest. Coverage is sqrt-scaled so a trace gas still shows as
                // scattered flecks instead of vanishing.
                //
                // **A veil is never a lid.** A magma-era burst pushes every
                // column so far past FULL_AIR_KG_M2 that raw coverage closes
                // all five shells solid, and five closed opaque shells are a
                // white ball with a planet somewhere inside it. So the STACK's
                // combined occlusion is capped: every shell's optical depth is
                // squeezed by one shared factor, which keeps the between-gas
                // ratios — the actual read — intact while the world underneath
                // stays visible under any sky whatsoever.
                if *air {
                    let cover = veil_coverages(&snap.air_shells);
                    for (k, (&(gas, _), &coverage)) in
                        snap.air_shells.iter().zip(&cover).enumerate()
                    {
                        if coverage < 0.004 {
                            continue;
                        }
                        let tint = gas_tint(gas);
                        let radius = RADIUS * (R_AIR_BASE + k as f32 * R_AIR_STEP);
                        shells.push(ShellSpec {
                            dirs,
                            outlines,
                            radius,
                            inset: 0.0,
                            color: Box::new(move |i: usize| {
                                (!sliced(i) && stippled(i, k, coverage)).then(|| lit(i, tint))
                            }),
                            cell_radius: None,
                        });
                    }
                }
            }
            world.set_shells(shells);
            world.set_arrows(arrows);
        }

        // The globe goes into the rect the walker reserved for it — FIRST.
        // `FrameGraph::execute` resets the shared per-frame draw queues, so an
        // offscreen pass declared after a main-frame draw would throw that draw
        // away. The world's rect is `None` whenever the viewport is not on
        // screen, and then the pass simply does not happen.
        {
            let mut fg = FrameGraph::new();
            let layer = renderer.layer();
            self.world.render(renderer, &mut fg, layer);
            fg.execute(renderer);
        }
        self.draw_hud(renderer);
    }
}

impl GodModeScene {
    /// This frame's screen. The template tier this bench composed against has been
    /// removed; the bench is not in the launcher roster, so `build_tree` returns an
    /// empty `screen` placeholder rather than rebuilding a UI ad-hoc.
    fn build_tree(&self) -> UiNode {
        UiNode {
            component: "screen".to_string(),
            id: "godmode".to_string(),
            ..Default::default()
        }
    }

    /// The cell being looked at: the one whose direction is closest to the camera.
    /// The globe is centred on the origin, so "toward the camera" is the point of it
    /// the maintainer is actually facing.
    fn facing_cell(&self) -> Option<u32> {
        self.world.facing(&self.dirs).map(|i| i as u32)
    }

    /// Take a freshly materialised tile off the sim thread and upload it once.
    fn collect_tile(&mut self, renderer: &mut Renderer) {
        let Some(TilePreview { rgba, dim, caption }) = self.sim.take_tile() else {
            return;
        };
        // The previous preview's texture is simply replaced; the renderer has no
        // free_texture, and a handful of 512² images over a session is nothing.
        let handle = renderer.load_texture(&rgba, dim, dim);
        self.tile = Some((handle, dim, caption));
    }

    /// Stage a preset: **the whole bundle** — endowment scales + planet size
    /// into the Starter's pending knobs, and the water levers plus the star's
    /// own brightness set live. **An input bundle, never an outcome**: these
    /// are the conditions the namesake world suggests (its size, its
    /// composition, its delivery, its orbit's sunlight), and what actually
    /// forms from them is the simulation's business. The user still presses
    /// FORGE.
    fn apply_preset(&mut self, preset: Preset) -> Option<SimCommand> {
        let (scales, freq, infall, coverage, stellar): (&[(&str, f64)], u32, f64, f64, f64) =
            match preset {
                // Iron-heavy, volatile-starved, small, seared, and the comets
                // never came.
                Preset::Mercury => (
                    &[("Fe", 2.0), ("S", 1.5), ("H", 0.1), ("C", 0.3), ("N", 0.3)],
                    24,
                    0.05,
                    1.0,
                    6.7,
                ),
                // Earth-sized carbon hotbox under a nearer star: nearly dry,
                // choking on its own CO₂.
                Preset::Venus => (
                    &[("C", 2.5), ("H", 0.3), ("N", 0.8), ("S", 1.2)],
                    96,
                    0.1,
                    1.0,
                    1.9,
                ),
                // The base seed, exactly as accretion.json ships it.
                Preset::Earth => (&[], 96, 1.0, 1.0, 1.0),
                // Smaller, drier, thinner, colder — half the volatiles, a
                // modest delivery, a distant sun.
                Preset::Mars => (
                    &[("H", 0.4), ("C", 0.5), ("N", 0.4), ("S", 0.8), ("Fe", 1.2)],
                    48,
                    0.3,
                    1.0,
                    0.43,
                ),
                // Small, rock-light, far from the light, and drowned from
                // outside: ten Earths of delivery onto a body that could never
                // exhale that much.
                Preset::Europa => (&[("H", 1.5), ("Si", 0.8), ("Fe", 0.8)], 12, 10.0, 1.0, 0.04),
            };
        for slot in self.pending_scales.iter_mut() {
            *slot = 1.0;
        }
        for &(sym, f) in scales {
            if let Some(i) = self.seed_elements.iter().position(|(_, s)| s == sym) {
                self.pending_scales[i] = f;
            }
        }
        self.pending_freq = freq;
        self.snap.as_ref().map(|s| {
            SimCommand::SetLevers(Levers {
                water_budget_kg: infall * flicker_poc_chemistry::surface::DEFAULT_WATER_KG,
                water_coverage_target: coverage,
                stellar_heat: stellar,
                ..s.levers
            })
        })
    }

    /// **THE dispatcher** — every fired result, from either channel, in one
    /// place. Returns the sim commands it wants sent rather than sending them,
    /// which is what lets a test assert what a control DID: the scene's other
    /// side effects are channel sends, and a returned command is observable
    /// where a send into a live thread is not.
    ///
    /// Pure over scene state otherwise: view toggles and pending Starter knobs
    /// are written here, and `update` does nothing but post the commands.
    fn apply_results(&mut self, results: &ValueMap) -> Vec<SimCommand> {
        let mut out = Vec::new();

        // ── The transport. ──
        if results.is_on("toggle_play") {
            out.push(SimCommand::TogglePlay);
        }
        if results.is_on("reset") {
            // A reset is a REBIRTH of the run: the sim clears its gate log, so
            // the scene's read-high-water clears with it — otherwise the same
            // gate at the same `at_myr` reads as already-acknowledged and the
            // pause summary never shows again (the second-run silence Aaron hit).
            self.gate_ack = f64::NEG_INFINITY;
            out.push(SimCommand::Reset);
        }
        if results.is_on("reseed") {
            // The transport's RESEED — the same act as the Starter's FORGE
            // WORLD (fresh seed, pending size + endowment), surfaced beside
            // RESET because rebirthing the molten world is a first-class
            // gesture, not a settings-menu excursion.
            out.push(self.forge());
        }
        // The rate dial, guarded against its own echo so a drag does not send
        // sixty identical commands a second.
        if let Some(v) = results.number("rate") {
            if let Some(s) = self.snap.as_ref() {
                let hz = v.clamp(1.0, 120.0) as f32;
                if (hz - s.rate_hz).abs() > 0.5 {
                    out.push(SimCommand::SetRate(hz));
                }
            }
        }

        // ── The view. These change what the maintainer SEES, never the world:
        //    no command leaves the scene, only a mesh rebuild. ──
        for (action, field, _, _) in FIELD_ACTIONS {
            if results.is_on(action) {
                self.field = field;
                self.world.mark_dirty();
            }
        }
        // The pause card's SHOW ME: go straight to the view that shows what just
        // changed, and acknowledge the transition on the way — looking at it IS
        // reading it, so the card has done its job and should not re-appear.
        // The run stays stopped: the point was to LOOK.
        if results.is_on("gate_view") {
            if let Some(g) = self.snap.as_ref().and_then(|s| s.gate_events.last()) {
                if let Some(v) = self
                    .process_defs
                    .get(g.stage)
                    .and_then(|d| Field::from_view(&d.view))
                {
                    self.field = v;
                    self.world.mark_dirty();
                }
                self.gate_ack = g.at_myr;
            }
        }
        if results.is_on("field_next") {
            self.field = self.field.cycle();
            self.world.mark_dirty();
        }
        if results.is_on("field_prev") {
            // Cycling backwards is cycling forwards N−1 times — one law, read
            // both ways, rather than a second match to keep in step.
            for _ in 0..FIELD_ACTIONS.len().saturating_sub(1) {
                self.field = self.field.cycle();
            }
            self.world.mark_dirty();
        }
        if let Some(v) = toggled(results, "cut") {
            if self.cut != v {
                self.cut = v;
                self.world.mark_dirty();
            }
        }
        if let Some(v) = toggled(results, "air") {
            if self.air != v {
                self.air = v;
                self.world.mark_dirty();
            }
        }
        if let Some(v) = toggled(results, "grid") {
            if self.grid != v {
                self.grid = v;
                self.world.mark_dirty();
            }
        }
        if results.is_on("seed_toggle") {
            self.seed_shown = !self.seed_shown;
        }

        // ── The tile inspector. ──
        if results.is_on("inspect") {
            // Look inside whatever cell is facing you. The camera orbits the
            // origin, so the point of the globe nearest the camera is the one
            // being looked at.
            if let Some(cell) = self.facing_cell() {
                out.push(SimCommand::Inspect(cell));
            }
        }
        if results.is_on("erode") {
            // Rain on the inspected tile: the per-pixel erosion batch, on its
            // own background thread. The scene mirrors the flag because the
            // command is fire-and-forget and the sim publishes no echo of it;
            // both sides start false, so the mirror is honest from the start.
            //
            // **The pixel stage is gated on LIFE** — the design's era gate: the
            // aggregate era ends, and per-pixel work begins, only once the
            // five-axis light says the world can sustain life. RAIN OFF is
            // always allowed; RAIN ON asks the light first (the button is
            // disabled until it turns, so this arm is the belt to that brace).
            let alive = self
                .snap
                .as_ref()
                .is_some_and(|s| s.habitability.life_supporting);
            if self.eroding || alive {
                self.eroding = !self.eroding;
                out.push(SimCommand::ErodeToggle);
            }
        }

        // ── The lever rack. Every rate lever rides a MULTIPLE of the physics
        //    as written, so one guard and one vocabulary cover all of them. A
        //    lever sets a CONDITION; none of them writes a result. ──
        if let Some(s) = self.snap.as_ref() {
            let base = Levers::default();
            let mut next = s.levers;
            let mut moved = false;
            for &(key, get, set) in LEVERS {
                let Some(v) = results.number(key) else {
                    continue;
                };
                let want = v.clamp(0.0, 4.0) * get(&base);
                if (want - get(&s.levers)).abs() > 1e-3 * get(&base).abs().max(1e-9) {
                    set(&mut next, want);
                    moved = true;
                }
            }
            if moved {
                out.push(SimCommand::SetLevers(next));
            }
        }

        // The pause summary's RESUME — declared as data (`on_cancel` on the
        // screen, `action` on the button), so the click, the pad and Esc are
        // the same event here. Acknowledging the transition is what dismisses
        // it; letting the run go again is the same gesture, because a summary
        // the maintainer has read has done its job.
        if results.is_on("gate_resume") {
            if let Some(g) = self.snap.as_ref().and_then(|s| s.gate_events.last()) {
                self.gate_ack = g.at_myr;
            }
            if self.snap.as_ref().is_some_and(|s| !s.playing) {
                out.push(SimCommand::TogglePlay);
            }
        }

        // The gate console — the popup with the simulation's gate CONTROLS.
        // Per-row HOLD/RELEASE is the sanctioned ARM/RELEASE lever: the
        // maintainer guides formation by holding a process, never by writing a
        // result.
        if results.is_on("gates_open") {
            self.gates_open = true;
        }
        if results.is_on("gates_close") {
            self.gates_open = false;
        }
        for n in 1..=PROCESS_ROWS {
            if results.is_on(&format!("hold_{n}")) {
                if let Some(p) = self.snap.as_ref().and_then(|s| s.processes.get(n - 1)) {
                    out.push(SimCommand::Hold {
                        stage: p.name.to_string(),
                        held: !p.held,
                    });
                }
            }
        }

        // The WATER slider — the infall's coverage cutoff, a boundary-input
        // condition (never an outcome writer: the world still decides how much
        // water any coverage takes, and the hypsometry can drift it afterwards).
        // Sent only on a real change, so a hover does not rebuild the pipeline
        // sixty times a second.
        if let Some(v) = results.number("water_coverage") {
            if let Some(s) = self.snap.as_ref() {
                if (v - s.levers.water_coverage_target).abs() > 5e-3 {
                    out.push(SimCommand::SetLevers(Levers {
                        water_coverage_target: v.clamp(0.0, 1.0),
                        ..s.levers
                    }));
                }
            }
        }
        // The INFALL dial — how much water the outer system sends at all, as a
        // multiple of the Earth-scale delivery. The same lever class as
        // coverage: a boundary input, changed live, and the world does with it
        // whatever it does.
        if let Some(v) = results.number("water_infall") {
            if let Some(s) = self.snap.as_ref() {
                let kg = v.clamp(0.0, 10.0) * flicker_poc_chemistry::surface::DEFAULT_WATER_KG;
                let step = 0.01 * flicker_poc_chemistry::surface::DEFAULT_WATER_KG;
                if (kg - s.levers.water_budget_kg).abs() > step {
                    out.push(SimCommand::SetLevers(Levers {
                        water_budget_kg: kg,
                        ..s.levers
                    }));
                }
            }
        }

        // Presets: one-click INPUT BUNDLES. Each stages the Starter's pending
        // knobs and sets the two water levers; the user still FORGEs. Named for
        // the world that inspired them — nothing anywhere guarantees the result.
        // The Starter's knobs write into PENDING scene state — nothing reaches
        // the sim until FORGE births a world from them. No change-guard needed:
        // a drag is just a local number until the button.
        for n in 1..=12usize {
            if let Some(v) = results.number(&format!("seed_el_{n}")) {
                if let Some(slot) = self.pending_scales.get_mut(n - 1) {
                    *slot = v.clamp(0.0, 3.0);
                }
            }
        }
        if let Some(v) = results.number("seed_freq") {
            self.pending_freq = (v.round() as u32).clamp(6, 96);
        }

        // Presets run AFTER the knob reads, deliberately: both write the same
        // pending state, and a preset click's own frame can carry the sliders'
        // standing values — handled first, the preset's staged bundle was
        // silently clobbered back the same frame, and the buttons appeared to
        // change nothing but the infall (the exact symptom reported).
        for (action, preset) in [
            ("preset_mercury", Preset::Mercury),
            ("preset_venus", Preset::Venus),
            ("preset_earth", Preset::Earth),
            ("preset_mars", Preset::Mars),
            ("preset_europa", Preset::Europa),
        ] {
            if results.is_on(action) {
                out.extend(self.apply_preset(preset));
            }
        }
        if results.is_on("starter_open") {
            self.starter_open = true;
        }
        if results.is_on("starter_close") {
            self.starter_open = false;
        }
        if results.is_on("forge") {
            out.push(self.forge());
        }

        out
    }

    /// Birth a new world from the Starter's pending knobs — a fresh seed, the
    /// pending size, the pending endowment. The transport's RESEED and the
    /// Starter's FORGE button are the same act.
    fn forge(&mut self) -> SimCommand {
        // A new world has no read gates: clear the acknowledgement high-water
        // so its first transition pauses-and-tells like the first run did.
        self.gate_ack = f64::NEG_INFINITY;
        self.seed = clock_seed();
        let scales = self
            .seed_elements
            .iter()
            .zip(&self.pending_scales)
            .map(|(&(e, _), &f)| (e, f))
            .collect();
        SimCommand::Reseed(SeedSpec {
            seed: self.seed,
            freq: self.pending_freq,
            scales,
        })
    }

    /// The HUD: the walker commands stashed by `update`, plus the ONE panel
    /// still scene-drawn — the bulk-seed element swatches.
    ///
    /// That panel is the sanctioned exception (S10) and the reason is specific:
    /// its per-row colour comes from `element_rgb`, a per-DATUM value with a
    /// hash fallback for elements nobody has picked a colour for, and the
    /// walker's colour channel is dotted style paths. The tectonic event log
    /// used to sit here too and no longer does — its colours were per-KIND, a
    /// finite set, so it became `godmode_events` like everything else. An
    /// exception that stops being true is just drift with a comment on it.
    ///
    /// The tile preview rides `render_hud`'s texture slice as slot 0, so the
    /// inspector's picture is a `sprite` in the tree rather than a fourth
    /// hand-placed panel.
    fn draw_hud(&self, renderer: &mut Renderer) {
        if let Some(white) = self.white {
            let tex: Vec<TextureHandle> = self
                .tile
                .as_ref()
                .map(|(t, _, _)| vec![*t])
                .unwrap_or_default();
            render_hud(renderer, &self.hud_commands, white, &tex);
        }

        // ── Bulk-seed element distribution — the one immediate panel left. ──
        let Some(white) = self.white else { return };
        if self.seed_shown && !self.budget_dist.is_empty() {
            renderer.set_layer(20.0);
            let gold = [0.722, 0.592, 0.353, 1.0]; // Prism bronze (structural accent)
            let text = [0.85, 0.87, 0.92, 1.0];
            let (pad, sw, row_h, panel_w) = (12.0f32, 12.0f32, 18.0f32, 210.0f32);
            let panel_h = pad + 24.0 + self.budget_dist.len() as f32 * row_h + pad;
            let (px, py) = (16.0f32, 158.0f32);
            renderer.draw_sprite(
                white,
                Vec2::new(px, py),
                Vec2::new(panel_w, panel_h),
                [0.05, 0.06, 0.08, 0.94],
            );
            renderer.draw_text(
                "BULK ACCRETION SEED",
                Vec2::new(px + pad, py + pad),
                14.0,
                gold,
            );
            let mut ry = py + pad + 24.0;
            for (num, sym, pct) in &self.budget_dist {
                let c = element_rgb(*num);
                renderer.draw_sprite(
                    white,
                    Vec2::new(px + pad, ry + 2.0),
                    Vec2::new(sw, sw),
                    [c[0], c[1], c[2], 1.0],
                );
                renderer.draw_text(
                    &format!("{sym}   {pct:.1}%"),
                    Vec2::new(px + pad + sw + 8.0, ry),
                    13.0,
                    text,
                );
                ry += row_h;
            }
        }
    }
}

/// Elevation against the sea the planet actually holds: submerged ground runs
/// through blues by depth, land through greens into the pale of high ground. The
/// shoreline is wherever the ramp turns — nobody draws it.
/// **Relief, as a relief map**: pure greyscale, black lowest → white highest,
/// spanning what the world actually has.
///
/// No sea in it at all, which is the point of separating this from
/// [`coast_color`]: mixing height and water into one ramp is what made every
/// world read as "blue with tan lines" regardless of how much of it was
/// actually under water. Height is one question; where the sea stands is
/// another, and each now has a view that answers only itself.
fn elevation_color(elev: f32, lo: f32, hi: f32) -> [f32; 3] {
    let t = ((elev - lo) / (hi - lo).max(1e-3)).clamp(0.0, 1.0);
    [t, t, t]
}

/// **What this ground IS against the sea** — the four grounds, plus a lit
/// coastline where the class changes.
///
/// This is the view the rock colours cannot give: in them a continental shelf
/// and a deep ocean bed are both simply "under water", and a bare magma world
/// and a drowned one are both simply dark. Here they separate.
fn coast_color(coast: u8) -> [f32; 3] {
    let base = match coast & SHELF_CLASS {
        SHELF_LAND => [0.55, 0.50, 0.36],    // land — dry continent
        SHELF_SHELF => [0.36, 0.68, 0.70],   // shelf — drowned continent
        SHELF_BED => [0.10, 0.16, 0.34],     // deep ocean bed
        SHELF_EXPOSED => [0.42, 0.30, 0.26], // exposed floor — rare
        _ => [0.13, 0.12, 0.14],             // no crust — bare mantle
    };
    // The coastline itself, brightened rather than outlined: an edge cell is one
    // whose neighbour is a different ground, which is exactly where a shore, a
    // shelf break or a rift margin is.
    if coast & SHELF_EDGE != 0 {
        lerp3(base, [1.0, 1.0, 1.0], 0.45)
    } else {
        base
    }
}

/// **Where the rain actually falls**, m of water per tick — the weather field
/// erosion cuts with, which until now was computed and thrown away every tick.
///
/// Square-rooted because rainfall spans orders of magnitude between a coast and
/// a continental interior, and a linear ramp shows the coast and nothing else.
/// A dry world reads honestly flat: no rain is a fact about the world, not a
/// scale to stretch.
fn rain_color(rain: f32, wettest: f32) -> [f32; 3] {
    if rain <= 0.0 {
        return [0.13, 0.12, 0.14];
    }
    let t = (rain / wettest.max(1e-9)).clamp(0.0, 1.0).sqrt();
    lerp3([0.16, 0.20, 0.24], [0.40, 0.95, 0.90], t)
}

/// How much history a column is carrying: bare rock through to a deep, banded
/// stack. `deepest` scales the ramp to the tallest stack in play, so the view
/// stays legible whether the world has two beds or twenty.
fn strata_color(beds: u8, deepest: u8) -> [f32; 3] {
    if beds == 0 {
        return [0.18, 0.18, 0.20];
    }
    let t = (beds as f32 / deepest as f32).clamp(0.0, 1.0);
    lerp3([0.30, 0.34, 0.42], [0.96, 0.82, 0.42], t)
}

/// How rich the best metal seam under a cell is, against the richest anywhere on
/// the planet. Ore spans orders of magnitude, so the ramp is logarithmic —
/// otherwise one exceptional deposit flattens every other into the background and
/// the map shows a single bright dot on black.
fn ore_color(enrichment: f32, richest: f32) -> [f32; 3] {
    if enrichment <= 1.0 {
        return [0.14, 0.14, 0.16];
    }
    let t = (enrichment.ln() / richest.max(1.001).ln()).clamp(0.0, 1.0);
    lerp3([0.20, 0.22, 0.30], [1.0, 0.84, 0.35], t)
}

/// Linear blend of two RGB triples.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Temperature ramp over a normalised value: cool deep-blue → red → white-hot.
///
/// **Relative to the frame's own min/max, and that is the point.** The heat
/// view's job is to show WHERE the heat is — the plumes and downwellings the
/// convection field carries and volcanism feeds on — and the interesting
/// structure is ±350 K riding on a 4000 K ball: an absolute scale painted the
/// whole magma era one flat colour and told the maintainer nothing (the white
/// ball, 2026-08-06 — an era of uniform colour is an instrument reading
/// blank).
fn temp_color(x: f32) -> [f32; 3] {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        lerp3([0.10, 0.16, 0.55], [0.90, 0.35, 0.12], x * 2.0)
    } else {
        lerp3([0.90, 0.35, 0.12], [1.0, 0.95, 0.85], (x - 0.5) * 2.0)
    }
}

/// Core-formation progress: undifferentiated slate → differentiated gold.
fn diff_color(d: f32) -> [f32; 3] {
    lerp3([0.12, 0.13, 0.18], [0.95, 0.75, 0.30], d.clamp(0.0, 1.0))
}

/// A stable, distinct hue per persistent plate id (golden-ratio rotation). Because
/// the observer keeps a plate's id across ticks, its colour no longer flickers as it
/// drifts. Diffuse lithosphere (id 0) is neutral grey.
fn plate_color(id: u32) -> [f32; 3] {
    if id == 0 {
        return [0.22, 0.23, 0.26];
    }
    let h = (id as f32 * 0.618_034).fract() * std::f32::consts::TAU;
    [
        0.45 + 0.4 * h.cos(),
        0.45 + 0.4 * (h + 2.094).cos(),
        0.45 + 0.4 * (h + 4.188).cos(),
    ]
}

/// Seam class → colour: divergent ridge (blue), convergent trench (red), transform
/// (amber); interior / diffuse is dim.
fn seam_color(code: u8) -> [f32; 3] {
    match code {
        1 => [0.30, 0.55, 0.95], // divergent — spreading ridge
        2 => [0.90, 0.30, 0.25], // convergent — trench / collision
        3 => [0.95, 0.80, 0.30], // transform — strike-slip
        _ => [0.20, 0.21, 0.24], // interior / diffuse
    }
}

/// Muted colour per element (atomic number) for the distribution swatches.
fn element_rgb(number: u8) -> [f32; 3] {
    match number {
        26 => [0.56, 0.28, 0.18],
        8 => [0.55, 0.55, 0.60],
        14 => [0.72, 0.66, 0.52],
        12 => [0.58, 0.72, 0.55],
        16 => [0.86, 0.78, 0.36],
        28 => [0.70, 0.72, 0.74],
        20 => [0.80, 0.78, 0.72],
        13 => [0.66, 0.66, 0.70],
        x => {
            let h = (x as f32 * 0.137).fract();
            [0.40 + 0.30 * h, 0.35, 0.52 - 0.20 * h]
        }
    }
}

/// Format a mass in kg with a compact mantissa/exponent (e.g. `5.972e24 kg`).
fn fmt_mass(kg: f64) -> String {
    if kg <= 0.0 {
        return "0 kg".to_string();
    }
    let exp = kg.log10().floor() as i32;
    let mantissa = kg / 10f64.powi(exp);
    format!("{mantissa:.3}e{exp} kg")
}

/// **What a gate moving actually means** — one line per condition the pipeline
/// gates on, in the maintainer's language rather than the stage's.
///
/// A stage name is an identifier; this is the display copy that says what the
/// world just did. Stages with no gate of their own never reach here, and one
/// that grows a gate later falls to the generic line until it earns a sentence.
fn gate_reason(stage: &str, opened: bool) -> &'static str {
    match (stage, opened) {
        ("Volcanism", true) => "$chem_why_volcanism_open",
        ("Volcanism", false) => "$chem_why_volcanism_shut",
        ("Biosphere", true) => "$chem_why_biosphere_open",
        ("Biosphere", false) => "$chem_why_biosphere_shut",
        ("Maturation", true) => "$chem_why_maturation_open",
        ("CarbonSink", true) => "$chem_why_carbonsink_open",
        ("WaterCycle", true) => "$chem_why_watercycle_open",
        ("Outgassing", false) => "$chem_why_outgassing_shut",
        ("CoreFormation", false) => "$chem_why_coreformation_shut",
        ("LateVeneer", true) => "$chem_why_lateveneer_open",
        _ => "$chem_why_generic",
    }
}

/// Pressure for the air readout: bar once the gas weighs in bar, millibar below
/// — an Earth-like pCO₂ is a fraction of a millibar, a steam burst is tens of
/// bar, and one fixed format would flatten one end or the other to noise.
fn fmt_pressure(pa: f64) -> String {
    if pa >= 1e4 {
        format!("{:.1} bar", pa / 1e5)
    } else {
        format!("{:.2} mbar", pa / 100.0)
    }
}

/// Format a plate life-event for the events panel, with a colour cue.
fn fmt_event(e: &PlateEvent) -> (String, &'static str) {
    // Label through the stringtable, colour through a dotted style path. Both
    // used to be literals here — an inline English word and a raw rgba — which
    // is precisely what kept this panel scene-drawn. The kinds are a finite
    // set, so both channels are ordinary data and the panel is ordinary UI.
    match e {
        PlateEvent::Born(id) => (
            format!("{} P{id}", strings::resolve("$chem_ev_born")),
            "chemistry.ok",
        ),
        PlateEvent::Died(id) => (
            format!("{} P{id}", strings::resolve("$chem_ev_died")),
            "chemistry.dim.color",
        ),
        PlateEvent::Merged { from, into } => (
            format!(
                "{} {}→P{into}",
                strings::resolve("$chem_ev_merge"),
                from.len() + 1
            ),
            "chemistry.interior.color",
        ),
        PlateEvent::Split { from, into } => (
            format!(
                "{} P{from}→{}",
                strings::resolve("$chem_ev_split"),
                into.len()
            ),
            "chemistry.crust.color",
        ),
    }
}

#[cfg(test)]
mod tests;
