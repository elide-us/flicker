//! **MaterialBench** — the MATERIAL DESIGNER.
//!
//! A node-graph editor over [`flicker_texture::MaterialGraph`]: generators, filters,
//! blends with mask ports and colour nodes, wired into ONE output whose seven ports give
//! every map its own chain. The LEFT column lists the nodes, the CENTRE seats the
//! [`flicker_canvas::GraphCanvas`] over the seven map views and the lit sample, the RIGHT
//! column holds the selected node's parameters (one authored cell per node kind) over the
//! output stage. The rack the bench used to be is not gone: every Roll is a rack patch,
//! arriving through [`MaterialGraph::from_recipe`]. The PRESET LIBRARY is data — New (the
//! scene's authored `params.new_material`), then the FACTORY LIBRARY: every v2 graph of the
//! content data dir's [`FACTORY_MATERIALS_FILE`], verbatim and in file order.
//!
//! # The scene is a PAIR (five-line architecture)
//!
//! `materialbench.scene.json` authors the WHOLE tree (every row, cell and control, the
//! per-kind parameter cells, the swatch strip's prototype) and this scene's style
//! blocks; `materialbench.lua` derives the presentation (row and swatch washes, the
//! parameter-cell and page gates) from the RAW model this behaviour publishes; the Rust
//! component kinds draw. Variable-length content is DATA expanded into the authored
//! prototypes by the walker's own repeater (`instantiate_rows`): `rows_from` on a `list`
//! / `row` for the node list, the materials list and the swatch strip, and on a `select`
//! for its OPTIONS — the leading choice the scene authors (None, Unbound, New) kept, its
//! LAST child the prototype each row clones, numbered on from it. The "Rust fills the
//! static scene's container" pattern is RETIRED for options: this bench adds no node to
//! any tree, composes no control and injects no style — it answers the row sources
//! ([`MaterialBench`]'s `rows`) and publishes each option's label (`<source>_opt<j>`)
//! into the Model every frame.
//!
//! # One dispatcher, one document
//!
//! Both input channels land in [`route::apply`] as result names and the canvas's
//! gestures in [`route::canvas`]; every edit either asks for is one
//! [`designer::Designer`] mutation. An edit bumps a generation and submits a
//! [`bake_graph`] to the WORKER — the 2K baseline is ~360 ms and even the 256² preview
//! ~10 ms, neither belongs on the frame thread — and the newest result wins. A graph that
//! cannot bake says why in the status line — a stringtable token per kind of
//! `GraphError`, the node, port or parameter it names beside it as data — and the swatch
//! keeps the last image that did.
//!
//! Binding a slot whose look is already COMMITTED — its bundle in `staging/`, else in
//! `package/` where a promotion moved it — is how a made material (the eight emissive glows
//! included) is edited where it was made. Picking such a slot ALWAYS asks, through the
//! shared `choice_dialog`: OPEN its committed look (the option says so when it would
//! discard unsaved edits), BIND the document in hand there (the next Commit replaces the
//! look), or keep editing (`MaterialBench::bind_slot`). Every other whole-document load — a
//! preset, Roll — asks too when the graph holds edits not yet loaded or committed (the
//! shared `unsaved_changes` preset); a slot binding is not an edit. `update` pushes the
//! prompt; its answer comes back through `Scene::modal_closed` into the ONE dispatcher, and
//! a cancel hands the control that asked back to the Model. Commit weighs the bound slot by
//! the ONE slot law ([`flicker_materials::slot_accepts`]) with its index row's render
//! class, and WARNS (never refuses) when an emissive material's look does not glow.
//!
//! # Why the swatch is one sprite and not nine
//!
//! The preview must show the SEAM, so the scene tiles each baked map into a
//! `tiles × tiles` buffer on the CPU and uploads that. The seven preview textures are
//! created once and rewritten **in place**, so the authored tree names them by constant
//! `tex` index — after the theme's own textures in the HUD's table ([`MAP_TEX_BASE`]).
//! Only the map you are LOOKING at is uploaded; the rest upload when shown.
//!
//! # The Lit view is the engine's material sample
//!
//! The LIT tab is a `flicker_materialview::MaterialSample` seated in the `mtl_lit`
//! surface, drawn under this bench's authored stage ([`LIT_STAGE_SOURCE`]) at that
//! stage's authored emissive gain, wearing the SAME seven preview textures
//! ([`SampleMaps::of_set`]); every in-place rewrite calls `MaterialSample::mark_dirty`.
//! Its ZOOM — how close the sample is drawn — is the view's, not the document's: the
//! authored [`LIT_ZOOM`] slider and the wheel over the seat move one number, by the
//! engine's one pair of laws (`MaterialSample::set_zoom` / `zoom_by`), inside the range and
//! step that slider authors ([`ZoomDial`], read off the tree — the one source).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use flicker::render::{FrameGraph, Rect, Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    instantiate_rows, render_hud, run_ui, strings, Row, SceneDef, SurfacePointer, SurfaceSlot,
    UiFrame, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_canvas::{CanvasMetrics, CanvasMode, CanvasStyle, GraphCanvas};
use flicker_input_core::{AbstractControls, GamepadConfig, InputContext, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_materials::{JsonTableSource, MaterialDef, MaterialId, RenderClass, Tables};
use flicker_materialview::graph_view::{self, canvas_style, with_nodes, CardLabels, GraphCards};
use flicker_materialview::{GlowPalette, MaterialSample, SampleBodies, SampleMaps, ZoomDial};
use flicker_shell::{ModalOption, ModalParams, PauseScene, SharedModal, Theme};
use flicker_texture::{
    bake_graph, GraphError, MapKind, MapSet, MaterialGraph, Node, NodeKind, Output, OutputStage,
    Scalar, BAKE_DEFAULT, PREVIEW_SIZE,
};
use flicker_worker::WorkerPool;

pub mod commit;
pub mod designer;
pub mod route;

use designer::{error_parts, Designer, OutPort, Sink, OUTPUT_CARD};

/// The stage source the Lit view is authored under (`stages.<source>` in the scene file).
/// The look — lighting, framing, the HDR resolve and its bloom — is DATA there.
pub const LIT_STAGE_SOURCE: &str = "materialbench_lit";

/// The `surface` node the graph canvas is seated in.
pub const CANVAS_SLOT: &str = "mtl_canvas";

/// The `surface` node the lit sample is seated in.
pub const LIT_SLOT: &str = "mtl_lit";

/// The Lit view's ZOOM slider — its node id and its bind, one name. Its authored `min` /
/// `max` / `step` ARE the zoom's range and the wheel's notch ([`ZoomDial::authored`]).
pub const LIT_ZOOM: &str = "lit_zoom";

/// The scene param holding the NEW-material default — a v2 material graph document,
/// authored as data (the first entry of the preset select).
pub const NEW_MATERIAL_PARAM: &str = "new_material";

/// The file, in the content data dir, the FACTORY LIBRARY is authored in — the preset
/// select's entries after New, in its order, one v2 material graph each (the designer's own
/// document, bound to a real materials.json row). Read through [`commit::graphs_in`]; the
/// headless `examples/commit_library.rs` commits the same file by this name.
pub const FACTORY_MATERIALS_FILE: &str = "factory_materials.json";

/// The scene param holding the strength a glow STARTS at when a colour is picked on a
/// material with none — authored, so the glow is visible the moment it exists.
pub const NEW_GLOW_STRENGTH_PARAM: &str = "new_glow_strength";

/// The notice a slot pick leaves when the slot's committed document — staged or promoted —
/// will not read or load: the pick re-binds the document in hand, and the status line says
/// why nothing opened, the document's path and the loader's reason beside it as data.
pub const COMMITTED_UNREADABLE: &str = "$mtl_committed_unreadable";

/// The warning the status line shows when a Commit STAGED a look that does not glow for a
/// slot whose material is emissive — legal (the slot law weighs the row, not the look),
/// almost surely a mistake. Never a refusal.
pub const NO_EMIT_WARNING: &str = "$mtl_warn_no_emit";

/// The shared tree every prompt of this bench is: the `choice_dialog`.
pub const PROMPT_TREE: &str = "choice_dialog";
/// A prompt's REPLACE answer — the bench's own action name, handed to the shared modal as
/// an option and returned verbatim through `modal_closed`: the waiting document becomes the
/// one in hand (a slot's committed look opens; a preset or a Roll loads — discarding unsaved
/// edits, which the prompt says).
pub const ASK_REPLACE: &str = "ask_replace";
/// The committed-look prompt's BIND-HERE answer: the document in hand stays and is bound to
/// the picked slot — its next Commit replaces the slot's committed look.
pub const ASK_BIND_HERE: &str = "ask_bind_here";
/// Every prompt's KEEP-EDITING answer — also its back-out (Esc / pad-B), so a stray press
/// never loses the work: nothing changes and the control that asked goes back to the Model.
pub const ASK_KEEP: &str = "ask_keep";

/// The scene param holding the canvas's READABLE zoom floor: a graph is framed whole only
/// while that takes no smaller a zoom than this; past it the camera frames the selection
/// at the floor (`GraphCanvas::fit_legible`).
pub const CANVAS_FIT_FLOOR_PARAM: &str = "canvas_fit_floor";

/// The scene's style block the graph canvas's ink is read from (`materialbench.canvas.<slot>`,
/// every slot a `$token` — [`graph_view::canvas_style`]).
pub const CANVAS_INK: &str = "materialbench.canvas";
/// The scene file, as a warning names it.
const SW_SCENE_FILE: &str = "materialbench.scene.json";

/// The pair script — the scene's LOGIC half, by name (five-line architecture).
const SW_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/materialbench.lua");
/// The shipped scene file — the tests' copy of the authored tree (the runtime receives
/// the same file through the manifest `SceneDef`).
const SW_SCENE: &str =
    include_str!("../../../../content/sensorium/scenes/materialbench.scene.json");

/// How many times the swatch repeats per axis in the preview — read from the merged
/// styles (`materialbench.preview.tiles`) at `enter`; the fallback for a malformed file
/// is the value the JSON ships.
const DEFAULT_TILES: u32 = 3;

/// The view-cell ids, in [`MapKind::ALL`] order — ONE vocabulary shared with the
/// authored tree's `visible_bind` cells and `materialbench.lua`'s `MAPS`.
pub const MAP_IDS: [&str; 7] = [
    "map_base",
    "map_normal",
    "map_rough",
    "map_metal",
    "map_ao",
    "map_height",
    "map_emit",
];

/// The LIT view's cell id — the eighth position of the view tabs' bound number.
pub const LIT_ID: &str = "map_lit";

/// How many tabs the view selector has — the seven flat maps plus the lit view.
pub const VIEW_COUNT: usize = MAP_IDS.len() + 1;

/// Where the seven preview textures start in the HUD's texture table: after the
/// theme's own (`white`, `muse`, `pad_glyphs` — [`Theme::TEXTURE_NAMES`]), so the
/// footer's `pad_glyphs.tex` names the glyph atlas and not a swatch.
pub const MAP_TEX_BASE: usize = Theme::TEXTURE_NAMES.len();

/// Where the glow swatches' 1×1 textures start in the HUD's texture table.
pub const SWATCH_TEX_BASE: usize = MAP_TEX_BASE + MapKind::ALL.len();

/// What a pending fit frames when the whole graph does not fit legibly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framing {
    /// A document just loaded: the OUTPUT card — the result is what the author reads
    /// first.
    Output,
    /// Reset view: the selected node's card (the output's when nothing is selected).
    Selection,
}

/// One baked preview, tagged with the edit that asked for it.
struct BakeResult {
    generation: u64,
    maps: Result<MapSet, GraphError>,
    /// The bake's own wall time on the worker.
    ms: f64,
}

/// What picking a material slot did to the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Binding {
    /// Nothing: the pick named no option, or the slot already bound.
    Unchanged,
    /// The document is now the appearance of another slot (or of none) — the maps bake
    /// the same, so it is no image edit.
    Rebound,
    /// The slot has a COMMITTED look: nothing has changed yet — the committed-look prompt is
    /// armed, and its answer opens the look, binds the work here, or keeps editing.
    Asked,
}

/// A whole document waiting on a prompt's answer.
pub(crate) struct Pending {
    /// What an [`ASK_REPLACE`] makes the document.
    pub(crate) graph: MaterialGraph,
    /// What it is — and so what the prompt over it offers.
    pub(crate) kind: PendingKind,
}

/// What a waiting document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingKind {
    /// The committed look of the slot a pick named: Open it, Bind the work here, or Keep
    /// editing (the dropdown goes back to the Model).
    Committed { slot: MaterialId },
    /// A whole-document load over UNSAVED edits — a preset (its index) or a Roll (`None`):
    /// Discard and load, or Keep editing (`control`, when the load came from one, goes
    /// back to the Model).
    Load {
        preset: Option<usize>,
        control: Option<&'static str>,
    },
}

/// A Commit in flight, as it started.
#[derive(Clone, Copy, Debug)]
struct Committing {
    /// The designer's mutation count — what its success marks saved.
    mutations: u64,
    /// The look it stages does not glow, for a slot whose material is emissive.
    dull: bool,
}

/// Where a Commit has got to. The bench stays fully live through one — a 2K bake is
/// ~360 ms, so it runs on the worker like every other bake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitState {
    Idle,
    Working,
    /// Staged, with the folder it landed in.
    Done(String),
    /// Why it failed: a stringtable TOKEN (the copy) and the specifics beside it as raw
    /// data — the node and port a graph error names, the path and cause of a write that
    /// failed. Never English composed in Rust.
    Failed {
        token: &'static str,
        detail: String,
    },
}

/// An open in-place material rename. The FIELD is the authored `text_field`
/// (`rename_field`, bound to `rename_draft`); its text-entry session is the walker's.
struct MaterialRename {
    /// The byte id being relabelled — the STRICT key the write targets.
    id: MaterialId,
    draft: String,
    /// The field's session has been seen open — after which a frame with no session
    /// open or requested means the edit was abandoned.
    armed: bool,
}

/// The counts the frame tree's expansion depends on — one per row source (the node list,
/// the materials, the presets, the palette, the output stage's two source lists and the
/// selected node's per-port candidate lists). The tree is re-derived only when one moves.
type FrameKey = Vec<usize>;

pub struct MaterialBench {
    /// The document and the selection in it — the ONLY thing the dispatcher edits.
    pub(crate) designer: Designer,
    /// The preset library: `[0]` the authored NEW-material default, then the factory
    /// library — every graph of [`FACTORY_MATERIALS_FILE`], verbatim and in file order
    /// ([`preset_library`]).
    pub(crate) presets: Vec<MaterialGraph>,
    /// Which preset was loaded last (`None` after a Roll).
    pub(crate) preset: Option<usize>,
    /// The view the centre shows — `0..MAP_IDS.len()` the flat maps, then LIT.
    pub(crate) sel_map: usize,
    /// Which PAGE the bench shows: 0 = the designer, 1 = the materials browser.
    pub(crate) page: usize,
    /// The material index's rows, by id — the binding picker's labels, and each slot's
    /// render class (what the slot law weighs a Commit against).
    pub(crate) materials: Vec<MaterialDef>,
    /// The curated glow palette (`palettes.json`).
    pub(crate) palette: GlowPalette,
    /// Which OFFERED bake rung a Commit uses.
    pub(crate) size_rung: usize,
    /// The data dir the material index was read from (a rename writes back through it).
    pub(crate) data_dir: std::path::PathBuf,
    rename: Option<MaterialRename>,
    /// The two lists' scroll offsets (their binds echo them).
    pub(crate) node_scroll: f64,
    pub(crate) mat_scroll: f64,
    /// The Insert button asked for the insertion menu — `update` opens it.
    pub(crate) ask_insert: bool,

    // ── the graph canvas ──
    pub(crate) canvas: GraphCanvas,
    pub(crate) canvas_mode: CanvasMode,
    /// This frame's cards and edges — the engine's one projection of the document
    /// ([`GraphCards::project`]) — re-derived when the document's revision moves.
    pub(crate) canvas_content: GraphCards,
    canvas_rev: Option<u64>,
    /// Frame the graph on the next seated frame — owed after a load (a new document,
    /// framed on its output) and on Reset view (framed on the selection).
    pub(crate) fit_pending: Option<Framing>,
    /// The zoom a fit never goes below (`params.canvas_fit_floor`).
    fit_floor: f32,
    /// The canvas ink, resolved from this scene's `materialbench.canvas` block at `enter`.
    canvas_style: CanvasStyle,

    // ── preview ──
    tiles: u32,
    /// The preview textures, in [`MapKind::ALL`] order — created once at
    /// `PREVIEW_SIZE * tiles` square and rewritten in place.
    tex: Vec<TextureHandle>,
    /// One 1×1 texture per palette entry — the swatch strip's sprites.
    swatches: Vec<TextureHandle>,
    /// The HUD's texture table: the theme's, the previews, the swatches.
    hud_tex: Vec<TextureHandle>,
    latest: Option<MapSet>,
    dirty: [bool; MapKind::ALL.len()],

    // ── the bake worker ──
    pool: WorkerPool,
    tx: Sender<BakeResult>,
    rx: Receiver<BakeResult>,
    generation: u64,
    shown: u64,
    last_bake_ms: f64,
    /// Why the newest preview bake failed — the status line's first say.
    bake_error: Option<GraphError>,
    pub(crate) commit_state: CommitState,
    /// The Commit in flight, as it started — its mutation count is what its success marks
    /// saved ([`designer::Designer::saved_through`]).
    committing: Option<Committing>,
    /// The warning the last STAGED Commit left (a dull emissive look): its token, its
    /// detail as data, and the designer's mutation count when it landed — it speaks of the
    /// STAGED look, so it is the status line's last say only until the next graph edit (or
    /// the next Commit or load), after which it is retired ([`Self::retire_stale_warning`]).
    commit_warning: Option<(&'static str, String, u64)>,
    commit_tx: Sender<CommitState>,
    pub(crate) commit_rx: Receiver<CommitState>,
    /// The staging root a Commit writes into, and a slot pick opens a committed look from
    /// first — resolved ONCE at construction.
    pub(crate) staging: PathBuf,
    /// The package root a slot pick opens a PROMOTED look from (a promotion moves the
    /// bundle out of staging) — resolved once at construction, read only.
    pub(crate) package: PathBuf,
    /// The document waiting on a prompt's answer.
    pub(crate) pending: Option<Pending>,
    /// A prompt over `pending`, armed for THIS frame's `update` to push as the shared
    /// `choice_dialog` (the dispatcher returns no transition, so it arms and `update` —
    /// which owns the `Transition` — opens it, the same hand-off the insertion menu uses).
    pub(crate) prompt_armed: bool,

    // ── engine plumbing ──
    pub(crate) sample: MaterialSample,
    /// The Lit view's zoom range and wheel notch, read once off the authored [`LIT_ZOOM`]
    /// slider ([`ZoomDial::OFF`] — the stage's framing only — when it will not read).
    pub(crate) zoom: ZoomDial,
    bodies: Option<SampleBodies>,
    lit_slot: Option<SurfaceSlot>,
    lit_stage: Option<flicker::render::StageDef>,

    /// The AUTHORED tree off the manifest's def — never mutated.
    authored: Option<UiNode>,
    /// The tree the walker walks: the authored one with its data-driven rows (and
    /// options) expanded, re-derived when a [`FrameKey`] count moves.
    frame_tree: Option<UiNode>,
    frame_key: FrameKey,
    /// This scene's own style blocks (`def.styles`).
    pub(crate) scene_styles_json: Option<serde_json::Value>,
    script: Option<ScriptHost>,
    ui_intents: UiIntents,
    ui_state: UiState,
    ui_styles: serde_json::Value,
    hud_commands: Vec<HudCommand>,
    ui_theme: Option<Theme>,
}

impl Default for MaterialBench {
    fn default() -> Self {
        Self::shipped()
    }
}

/// The strength a NEW glow starts at — `params.new_glow_strength`. A missing or
/// malformed value is loud and falls back to the crate's own output-stage default.
fn new_glow_strength(params: &serde_json::Map<String, serde_json::Value>) -> f32 {
    match params
        .get(NEW_GLOW_STRENGTH_PARAM)
        .and_then(serde_json::Value::as_f64)
    {
        Some(v) if v.is_finite() && v >= 0.0 => v as f32,
        _ => {
            let fallback = OutputStage::default().emissive_strength;
            tracing::error!(
                "materialbench: the scene authors no usable params.{NEW_GLOW_STRENGTH_PARAM} \
                 — a new glow starts at the crate default {fallback}"
            );
            fallback
        }
    }
}

/// The row sources the node-input selects repeat from — `in0`, `in1`, … — one per port
/// the widest node kind has.
fn input_sources() -> usize {
    NodeKind::catalog()
        .iter()
        .map(|k| k.ports().len())
        .max()
        .unwrap_or(0)
}

/// A palette colour as a 1×1 swatch texel (the recipe's colour domain — byte / 255).
fn swatch_texel(rgb: [f32; 3]) -> [u8; 4] {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), 255]
}

/// The staging and package trees [`MaterialBench::shipped`] commits into and opens from:
/// the content roots service's own outside test; under test scratch paths no other bench
/// shares (pid + call counter) and nothing creates until that bench commits — so no test
/// reads what happens to be staged or promoted on the machine running it, or what another
/// test committed.
#[cfg(not(test))]
fn shipped_roots() -> (PathBuf, PathBuf) {
    let roots = flicker_content::roots();
    (roots.staging(), roots.package())
}

#[cfg(test)]
fn shipped_roots() -> (PathBuf, PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BENCH: AtomicU64 = AtomicU64::new(0);
    let seq = BENCH.fetch_add(1, Ordering::Relaxed);
    let tree =
        std::env::temp_dir().join(format!("flicker_mtl_shipped_{}_{seq}", std::process::id()));
    (tree.join("staging"), tree.join("package"))
}

/// The FACTORY LIBRARY: every material graph of [`FACTORY_MATERIALS_FILE`] in the content
/// data dir `data_dir`, VERBATIM and in FILE order — read through [`commit::graphs_in`] (each
/// entry through `load_material_json`, exactly as a staged bundle's document and the headless
/// `examples/commit_library.rs` read it), so the look a preset opens is the look that file
/// commits. Nothing is converted, snapped or re-bound on the way in.
///
/// A missing or unreadable file, a file that authors no entry, or ANY entry that is not a
/// material document, is loud — the path and the reason (`graphs_in` names the entry's
/// position and the loader's) — and the library is EMPTY, so the preset select offers New
/// alone. All or nothing: a library that quietly skipped its broken entry would hide the
/// break behind a plausible list.
fn factory_library(data_dir: &Path) -> Vec<MaterialGraph> {
    let path = data_dir.join(FACTORY_MATERIALS_FILE);
    std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| commit::graphs_in(&text))
        .and_then(|graphs| {
            if graphs.is_empty() {
                Err("its `graphs` array is empty".to_string())
            } else {
                Ok(graphs)
            }
        })
        .unwrap_or_else(|e| {
            tracing::error!(
                "materialbench: the factory library {} did not load ({e}) — the preset select \
                 offers New alone",
                path.display()
            );
            Vec::new()
        })
}

/// The preset library: `[0]` the authored NEW default (`params.new_material`), then the
/// [`factory_library`] read from `data_dir`. A missing or malformed New is loud and opens
/// the library's first entry instead — or, with the library empty too (unloadable, or
/// authoring no entry), a [`blank_graph`]: the bench always opens on a document, and the
/// errors say why.
fn preset_library(
    params: &serde_json::Map<String, serde_json::Value>,
    data_dir: &Path,
) -> Vec<MaterialGraph> {
    let factory = factory_library(data_dir);
    let authored = match params
        .get(NEW_MATERIAL_PARAM)
        .map(|v| serde_json::from_value::<MaterialGraph>(v.clone()))
    {
        Some(Ok(graph)) => Some(graph),
        Some(Err(e)) => {
            tracing::error!(
                "materialbench: params.{NEW_MATERIAL_PARAM} is not a material graph ({e}) — \
                 New opens the factory library's first entry"
            );
            None
        }
        None => {
            tracing::error!(
                "materialbench: the scene authors no params.{NEW_MATERIAL_PARAM} — New opens \
                 the factory library's first entry"
            );
            None
        }
    };
    let new = authored
        .or_else(|| factory.first().cloned())
        .unwrap_or_else(|| {
            tracing::error!(
                "materialbench: no New and no factory library entry to stand in for it — New \
                 opens a blank graph"
            );
            blank_graph()
        });
    std::iter::once(new).chain(factory).collect()
}

/// What New opens when nothing authored is left to open — `params.new_material` broken AND
/// the factory library empty: unbound, no nodes, the crate's fresh [`Output`] (every map
/// resting: a neutral grey, flat and matte — it bakes and has no problems). Named after the
/// param it stands in for, so the recipe line points at what to fix. Never a look: only
/// somewhere for the author to start inserting nodes.
fn blank_graph() -> MaterialGraph {
    MaterialGraph {
        version: flicker_texture::graph::GRAPH_VERSION,
        id: NEW_MATERIAL_PARAM.into(),
        name: NEW_MATERIAL_PARAM.into(),
        material: None,
        seed: 0,
        nodes: Vec::new(),
        output: Output::default(),
    }
}

/// What a fresh bench OPENS ON: the document, the preset it IS (when it is one), and why a
/// committed look did not open (when one would not).
///
/// The preset at `first` — the factory library's first entry — unless the slot it binds
/// already holds a COMMITTED look (staged, else promoted: [`commit::committed`]). Then THAT
/// look opens, because re-picking the slot already bound asks nothing
/// ([`MaterialBench::bind_slot`]): a bench that opened on the factory entry would never
/// offer the committed look, and its first Commit would replace it unasked. Nothing is
/// unsaved at startup, so there is nothing to ask — this is the committed-look prompt's
/// OPEN, taken for the author. The look is still the preset when it IS the entry (the
/// library committed byte-for-byte as the file has it); otherwise the select reads custom,
/// as it does over any committed look.
///
/// A committed document that will not read or load is loud — the slot, both roots and the
/// loader's reason — and the factory entry opens, the reason handed back for the status
/// line's [`COMMITTED_UNREADABLE`], as a slot pick names one.
fn opening(
    presets: &[MaterialGraph],
    first: usize,
    staging: &Path,
    package: &Path,
) -> (MaterialGraph, Option<usize>, Option<String>) {
    let entry = &presets[first];
    let Some(slot) = entry.material else {
        return (entry.clone(), Some(first), None);
    };
    match commit::committed(staging, package, slot) {
        Ok(None) => (entry.clone(), Some(first), None),
        Ok(Some(look)) => {
            let preset = (look == *entry).then_some(first);
            (look, preset, None)
        }
        Err(why) => {
            tracing::error!(
                slot,
                staging = %staging.display(),
                package = %package.display(),
                "materialbench: the opening slot's committed look will not open ({why}) — the \
                 factory entry opens instead"
            );
            (entry.clone(), Some(first), Some(why))
        }
    }
}

impl MaterialBench {
    /// The runtime constructor — the manifest hands in the authored `SceneDef`; Commit
    /// writes into the content tree's `staging/`, and a slot pick opens a committed look
    /// from there or from `package/` — both asked of the content roots service, as is the
    /// `data/` the material index, the glow palette and the factory library are read from.
    /// It opens on the factory library's first entry — or, once that entry's slot holds a
    /// COMMITTED look, on that look: what was made opens where it was made, and the first
    /// Commit never replaces a committed look the author was not shown.
    pub fn new(def: &SceneDef) -> Self {
        let roots = flicker_content::roots();
        Self::from_parts(
            def.tree.clone(),
            def.styles.clone(),
            &def.params,
            (roots.staging(), roots.package()),
        )
    }

    /// A bench on the SHIPPED scene file — the seam a test drives without an app. Under
    /// test it commits into, and opens from, EMPTY scratch trees — never the live ones:
    /// what a slot pick opens must not hang on whatever happens to be staged or promoted on
    /// the machine running the suite (a test that commits aims its own roots). What it
    /// READS — the material index, the palette, the factory library — is the shipped
    /// `data/` (read only), so a test weighs the presets that ship. Its scratch trees hold
    /// nothing committed, so it always opens on the factory library's first entry; a test
    /// of the committed-look opening builds its bench over roots it filled.
    pub fn shipped() -> Self {
        let def = SceneDef::parse("materialbench", SW_SCENE)
            .expect("the shipped materialbench.scene.json parses");
        Self::from_parts(def.tree, def.styles, &def.params, shipped_roots())
    }

    /// The ONE constructor behind both seams: the authored tree, this scene's style blocks
    /// and params, and the `(staging, package)` roots a Commit writes into and a committed
    /// look opens from. It opens on [`opening`]'s choice — the factory library's first
    /// entry, or the committed look of the slot that entry binds — with a committed document
    /// that would not open named on the status line ([`COMMITTED_UNREADABLE`]).
    fn from_parts(
        authored: Option<UiNode>,
        scene_styles_json: Option<serde_json::Value>,
        params: &serde_json::Map<String, serde_json::Value>,
        (staging, package): (PathBuf, PathBuf),
    ) -> Self {
        if authored.is_none() {
            tracing::error!("materialbench: the scene def declares no `tree` — no UI will draw");
        }
        let ui_intents = authored.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(SW_SCRIPT, "materialbench.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("materialbench.lua failed to load — raw model values only: {e}");
                None
            }
        };

        // The material index, through the roots service — the bench asks where content
        // lives, it never spells out a path. An index that will not load is survivable:
        // the picker offers nothing (visibly wrong).
        let data_dir = flicker_content::roots().data();
        let materials = match Tables::from_source(&JsonTableSource::new(&data_dir)) {
            Ok(t) => {
                let mut rows = t.materials().to_vec();
                rows.sort_by_key(|m| m.id);
                rows
            }
            Err(e) => {
                tracing::warn!("material index unreadable at {}: {e}", data_dir.display());
                Vec::new()
            }
        };
        let palette = GlowPalette::load(&data_dir);
        let presets = preset_library(params, &data_dir);
        let glow_strength = new_glow_strength(params);
        let fit_floor = graph_view::fit_floor(params, CANVAS_FIT_FLOOR_PARAM, SW_SCENE_FILE);
        // Open on the factory library's first entry — a bound material, so Commit is live at
        // once — or on New when the library did not load (New is always [0]); over that
        // entry's committed look when its slot holds one (`opening`).
        let first = 1.min(presets.len() - 1);
        let (opened, preset, unreadable) = opening(&presets, first, &staging, &package);
        let size_rung = flicker_texture::size::offered()
            .position(|r| r.px == BAKE_DEFAULT)
            .unwrap_or(0);
        // The Lit view's zoom range and notch: the authored slider's own numbers. A bench
        // with no tree was named above, and has no slider to read.
        let zoom = authored.as_ref().map_or(ZoomDial::OFF, |tree| {
            ZoomDial::authored(tree, LIT_ZOOM, SW_SCENE_FILE)
        });

        let (tx, rx) = mpsc::channel();
        let (commit_tx, commit_rx) = mpsc::channel();
        let mut me = Self {
            designer: Designer::new(opened, glow_strength),
            presets,
            preset,
            sel_map: 0,
            page: 0,
            materials,
            palette,
            size_rung,
            data_dir,
            rename: None,
            node_scroll: 0.0,
            mat_scroll: 0.0,
            ask_insert: false,
            canvas: GraphCanvas::new(CanvasMetrics::default()),
            canvas_mode: CanvasMode::Select,
            canvas_content: GraphCards::default(),
            canvas_rev: None,
            fit_pending: Some(Framing::Output),
            fit_floor,
            canvas_style: CanvasStyle::blank(),
            tiles: DEFAULT_TILES,
            tex: Vec::new(),
            swatches: Vec::new(),
            hud_tex: Vec::new(),
            latest: None,
            dirty: [false; MapKind::ALL.len()],
            pool: WorkerPool::with_default_size(),
            tx,
            rx,
            generation: 0,
            shown: 0,
            last_bake_ms: 0.0,
            bake_error: None,
            commit_state: CommitState::Idle,
            committing: None,
            commit_warning: None,
            commit_tx,
            commit_rx,
            staging,
            package,
            pending: None,
            prompt_armed: false,
            sample: MaterialSample::new(LIT_STAGE_SOURCE),
            zoom,
            bodies: None,
            lit_slot: None,
            lit_stage: None,
            authored,
            frame_tree: None,
            frame_key: Vec::new(),
            scene_styles_json,
            script,
            ui_intents,
            ui_state: UiState::default(),
            ui_styles: serde_json::Value::Null,
            hud_commands: Vec::new(),
            ui_theme: None,
        };
        if let Some(why) = unreadable {
            me.designer.refuse_with(COMMITTED_UNREADABLE, why);
        }
        me.refresh_frame_tree();
        me.request_bake();
        me
    }

    /// Load the merged styles — the theme, the shared satellites and THIS scene's own
    /// blocks — and compile, once, what reads them: the lit stage, the canvas ink, the
    /// preview's tiling. Never per frame.
    pub(crate) fn adopt_styles(&mut self) {
        self.ui_styles = flicker::ui::load_shared_styles(self.scene_styles_json.as_ref());
        self.lit_stage = flicker::ui::stage_def(&self.ui_styles, LIT_STAGE_SOURCE);
        self.canvas_style = canvas_style(&self.ui_styles, CANVAS_INK);
        self.tiles = self
            .ui_styles
            .get("materialbench")
            .and_then(|s| s.get("preview"))
            .and_then(|p| p.get("tiles"))
            .and_then(|t| t.as_u64())
            .map(|t| t.clamp(1, 4) as u32)
            .unwrap_or(DEFAULT_TILES);
    }

    /// The edge length of one preview texture: the swatch, repeated.
    fn tex_px(&self) -> u32 {
        PREVIEW_SIZE * self.tiles
    }

    /// Ask for a fresh preview — cheap and safe on every edit: the job captures a clone
    /// of the graph, and a result that lands after a newer edit is dropped on arrival.
    pub(crate) fn request_bake(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let graph = self.designer.graph().clone();
        let tx = self.tx.clone();
        self.pool.submit(move || {
            let started = Instant::now();
            let maps = bake_graph(&graph, PREVIEW_SIZE);
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            // A closed receiver just means the bench went away mid-bake.
            let _ = tx.send(BakeResult {
                generation,
                maps,
                ms,
            });
        });
    }

    /// Drain the worker channel, keeping only the newest result. A failed bake keeps the
    /// last image that baked and says why.
    fn collect_bakes(&mut self) {
        let mut newest: Option<BakeResult> = None;
        while let Ok(r) = self.rx.try_recv() {
            if r.generation > newest.as_ref().map_or(self.shown, |n| n.generation) {
                newest = Some(r);
            }
        }
        let Some(r) = newest else {
            return;
        };
        self.shown = r.generation;
        self.last_bake_ms = r.ms;
        match r.maps {
            Ok(maps) => {
                self.latest = Some(maps);
                // Every map is now stale on the GPU; they upload as they are looked at.
                self.dirty = [true; MapKind::ALL.len()];
                self.bake_error = None;
            }
            Err(e) => {
                if self.bake_error.as_ref() != Some(&e) {
                    tracing::warn!("materialbench: the preview did not bake — {e}");
                }
                self.bake_error = Some(e);
            }
        }
    }

    /// Repeat `map` into a `tiles × tiles` buffer — the swatch shown against itself,
    /// which is the only way a seam is visible.
    fn tiled(&self, map: &flicker_texture::Map) -> Vec<u8> {
        let (src, n, t) = (&map.pixels, map.size as usize, self.tiles as usize);
        let w = n * t;
        let mut out = vec![0u8; w * w * 4];
        for y in 0..w {
            let row = (y % n) * n * 4;
            for tx in 0..t {
                let dst = (y * w + tx * n) * 4;
                out[dst..dst + n * 4].copy_from_slice(&src[row..row + n * 4]);
            }
        }
        out
    }

    /// Push the currently-shown map to the GPU if it has changed.
    fn upload_shown(&mut self, renderer: &mut Renderer) {
        if !self.dirty.get(self.sel_map).copied().unwrap_or(false) {
            return;
        }
        let Some(handle) = self.tex.get(self.sel_map).copied() else {
            return;
        };
        let Some(set) = self.latest.as_ref() else {
            return;
        };
        let Some(map) = set.get(MapKind::ALL[self.sel_map]) else {
            return;
        };
        let pixels = self.tiled(map);
        if renderer.update_texture(handle, &pixels) {
            self.dirty[self.sel_map] = false;
            // Rewritten IN PLACE under the same handle: the lit sample's dirty channel
            // compares handles, so it must be told.
            self.sample.mark_dirty();
        } else {
            // A size disagreement — loud, because a silently stale swatch would have the
            // author dialling against an image that is not their graph.
            tracing::warn!(
                "preview upload rejected for {:?}: {} bytes for a {}² texture",
                MapKind::ALL[self.sel_map],
                pixels.len(),
                self.tex_px()
            );
        }
    }

    /// Push EVERY stale map — the lit sample binds all of them at once.
    fn upload_all(&mut self, renderer: &mut Renderer) {
        for i in 0..MapKind::ALL.len() {
            if !self.dirty[i] {
                continue;
            }
            let (Some(handle), Some(set)) = (self.tex.get(i).copied(), self.latest.as_ref()) else {
                continue;
            };
            let Some(map) = set.get(MapKind::ALL[i]) else {
                continue;
            };
            let pixels = self.tiled(map);
            if renderer.update_texture(handle, &pixels) {
                self.dirty[i] = false;
                self.sample.mark_dirty();
            }
        }
    }

    // ── the frame tree ───────────────────────────────────────────────────────────

    fn frame_key(&self) -> FrameKey {
        let mut key = vec![
            self.designer.graph().nodes.len(),
            self.materials.len(),
            self.presets.len(),
            self.palette.len(),
        ];
        key.extend(
            [OutPort::Height, OutPort::Base]
                .iter()
                .map(|p| self.designer.candidates(&Sink::Output(*p)).len()),
        );
        key.extend((0..input_sources()).map(|k| self.input_candidates(k).len()));
        key
    }

    /// Re-derive the walked tree when a count it expands moved: the authored tree with
    /// every `rows_from` prototype expanded by the walker's repeater ([`instantiate_rows`]
    /// — lists, rows and select options alike). The labels themselves ride the Model
    /// every frame, so the tree only changes when a COUNT does.
    pub(crate) fn refresh_frame_tree(&mut self) {
        let key = self.frame_key();
        if self.frame_tree.is_some() && self.frame_key == key {
            return;
        }
        let Some(authored) = self.authored.as_ref() else {
            return;
        };
        let mut scratch = ValueMap::default();
        let tree = instantiate_rows(authored, &mut scratch, &|source| self.rows(source));
        self.frame_tree = Some(tree);
        self.frame_key = key;
    }

    /// The candidates the selected node's input `port` may be wired from — empty when no
    /// node is selected or it has no such port.
    fn input_candidates(&self, port: usize) -> Vec<&str> {
        match self.designer.selected() {
            Some((_, n)) if port < n.kind.ports().len() => self.designer.candidates(&Sink::Input {
                node: n.key.clone(),
                port,
            }),
            _ => Vec::new(),
        }
    }

    /// The candidates a source list offers, as rows (`key · Kind`).
    fn node_rows(&self, keys: Vec<&str>) -> Vec<Row> {
        keys.into_iter()
            .filter_map(|k| self.designer.index_of(k))
            .map(|i| {
                let n = &self.designer.graph().nodes[i];
                Row::new(n.key.clone(), self.node_label(n))
            })
            .collect()
    }

    /// The data-driven rows the authored prototypes expand from — the scene's row
    /// sources, by name:
    ///
    /// * `nodes` — the node list;
    /// * `materials` — the material index (the materials page's list and the binding
    ///   dropdown's options after Unbound);
    /// * `preset` — the factory library (the preset select's options after New);
    /// * `glow` — the curated palette (the swatch strip, and the glow-colour options);
    /// * `field` / `colour` — what an output port of that kind may be wired from;
    /// * `in<k>` — what the SELECTED node's input `k` may be wired from: only what the
    ///   graph would accept there (that kind of signal; never the node itself nor
    ///   anything it feeds), so no option on offer is one the graph refuses.
    fn rows(&self, source: &str) -> Option<Vec<Row>> {
        match source {
            "nodes" => Some(
                self.designer
                    .graph()
                    .nodes
                    .iter()
                    .map(|n| Row::new(n.key.clone(), self.node_label(n)))
                    .collect(),
            ),
            "materials" => Some(
                self.materials
                    .iter()
                    .map(|def| Row::new(def.id.to_string(), def.name.clone()))
                    .collect(),
            ),
            "preset" => Some(
                self.presets
                    .iter()
                    .skip(1)
                    .map(|g| Row::new(g.id.clone(), g.name.clone()))
                    .collect(),
            ),
            "glow" => Some(
                self.palette
                    .iter()
                    .map(|c| Row::new(c.id.clone(), c.id.clone()))
                    .collect(),
            ),
            "field" => {
                Some(self.node_rows(self.designer.candidates(&Sink::Output(OutPort::Height))))
            }
            "colour" => {
                Some(self.node_rows(self.designer.candidates(&Sink::Output(OutPort::Base))))
            }
            _ => {
                let port: usize = source.strip_prefix("in")?.parse().ok()?;
                (port < input_sources()).then(|| self.node_rows(self.input_candidates(port)))
            }
        }
    }

    /// A node as a list row and a source option reads: `key · Kind`.
    fn node_label(&self, n: &Node) -> String {
        graph_view::node_heading(&n.key, &graph_view::kind_name(n.kind.id()))
    }

    // ── the Model ──────────────────────────────────────────────────────────────

    /// The RAW runtime variables the pair script and the tree bind. Display copy is a
    /// `$token` (localised at the draw boundary) or data (a node key, a material name);
    /// numbers are pre-formatted here so the tree never does arithmetic. Presentation —
    /// washes, cell and page gates — is `materialbench.lua`'s `derive()`.
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::default();
        let d = &self.designer;
        let g = d.graph();

        m.set("sel_map", self.sel_map as f64);
        m.set("sel_page", self.page as f64);
        m.set("sel_preset", self.preset_option());

        // Every option label, every frame, under the key each select's prototype binds
        // (`<source>_opt{row}`) — the tree only knows how MANY there are.
        for source in ["preset", "field", "colour", "glow", "materials"] {
            let key = match source {
                "materials" => "material",
                other => other,
            };
            for (j, row) in self
                .rows(source)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                let label = match source {
                    // A palette entry's name is copy: resolved here, since an option's
                    // bound label is drawn as given.
                    "glow" => strings::resolve(&format!("$mtl_pal_{}", row.id)).into_owned(),
                    _ => row.label,
                };
                m.set(format!("{key}_opt{j}"), label);
            }
        }
        for k in 0..input_sources() {
            for (j, row) in self
                .rows(&format!("in{k}"))
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                m.set(format!("in{k}_opt{j}"), row.label);
            }
        }

        // The node list.
        m.set("node_count", g.nodes.len() as f64);
        for (i, n) in g.nodes.iter().enumerate() {
            m.set(format!("nl{i}_label"), self.node_label(n));
            m.set(format!("nl{i}_key"), n.key.clone());
        }
        m.set("nl_scroll", self.node_scroll);

        // The selected node: which cell shows, its dials, its wires.
        match d.selected() {
            Some((i, n)) => {
                m.set("sel_node_index", i as f64);
                m.set("sel_kind", n.kind.id());
                m.set("sel_node_label", self.node_label(n));
                for (key, value) in designer::knobs(&n.kind, &self.palette) {
                    m.set(key, value);
                }
                for port in 0..n.kind.ports().len() {
                    let sink = Sink::Input {
                        node: n.key.clone(),
                        port,
                    };
                    m.set(format!("node_in{port}"), d.source_option(&sink));
                }
            }
            None => {
                m.set("sel_node_index", -1.0);
                m.set("sel_kind", "");
                m.set("sel_node_label", "—");
            }
        }

        // The output stage.
        let out = &g.output;
        m.set("out_base", d.source_option(&Sink::Output(OutPort::Base)));
        m.set(
            "out_height",
            d.source_option(&Sink::Output(OutPort::Height)),
        );
        m.set("out_relief", out.relief as f64);
        for (port, src, level) in [
            (OutPort::Roughness, "out_rough_src", "out_rough"),
            (OutPort::Metalness, "out_metal_src", "out_metal"),
            (OutPort::Ao, "out_ao_src", "out_ao"),
        ] {
            m.set(src, d.source_option(&Sink::Output(port)));
            if let Some(Scalar::Const(c)) = d.level(port) {
                m.set(level, *c as f64);
            }
        }
        m.set("out_emit_on", out.emit.is_some());
        m.set("out_emit_color", d.emit_option(&self.palette));
        if let Some(e) = &out.emit {
            m.set(
                "out_emit_mask",
                d.source_option(&Sink::Output(OutPort::EmitMask)),
            );
            m.set("out_emit_strength", e.strength as f64);
        }

        // The glow palette: the texture slot each swatch sprite draws (the names ride
        // `glow_opt<j>` above).
        m.set("glow_count", self.palette.len() as f64);
        for j in 0..self.palette.len() {
            m.set(format!("glow{j}_tex"), (SWATCH_TEX_BASE + j) as f64);
        }

        m.set(
            "canvas_mode",
            match self.canvas_mode {
                CanvasMode::Link => 1.0,
                _ => 0.0,
            },
        );
        m.set("lit_body_label", self.sample.body.token());
        m.set("lit_spin", self.sample.spinning);
        m.set(LIT_ZOOM, f64::from(self.sample.zoom));

        // Composed from DATA — the graph's name, its slot, its seed — and the one piece
        // of copy in it resolved first: a `$token` mid-line would draw raw.
        let material = g.material.map_or_else(
            || strings::resolve("$mtl_unbound").into_owned(),
            |id| id.to_string(),
        );
        m.set(
            "recipe_line",
            format!("{} · {} · {:#x}", g.name, material, g.seed),
        );
        m.set(
            "preview_info",
            format!("{}² · {}×{}", PREVIEW_SIZE, self.tiles, self.tiles),
        );
        m.set("bake_ms", self.last_bake_ms);
        m.set(
            "bake_info",
            format!(
                "{}² · {}×{} · {:.0} ms", // strings-gate-exempt: unit symbol, not copy
                PREVIEW_SIZE, self.tiles, self.tiles, self.last_bake_ms
            ),
        );
        m.set("problem_count", d.problems().len() as f64);
        m.set("problems_label", d.problems().len().to_string());
        let (status, detail) = self.status_line();
        m.set("status_line", status);
        m.set("status_detail", detail);

        // The material binding: the dropdown's option labels ride `material_opt<j>`
        // above, the materials page's rows `mat_row_<j>`; the control echoes the bound
        // option INDEX (see `material_option`).
        for (i, def) in self.materials.iter().enumerate() {
            m.set(format!("mat_row_{i}"), def.name.clone());
        }
        m.set("mat_scroll", self.mat_scroll);
        m.set("sel_material", self.material_option());
        m.set("renaming", self.is_renaming());
        m.set("material_bound", g.material.is_some());
        m.set(
            "material_id_label",
            g.material
                .map_or_else(|| "—".to_string(), |id| format!("#{id}")),
        );
        m.set(
            "rename_draft",
            self.rename_draft().unwrap_or_default().to_string(),
        );
        m.set("size_label", self.bake_rung().label);
        let (commit_status, commit_detail) = match &self.commit_state {
            CommitState::Idle => ("$mtl_commit_idle".to_string(), String::new()),
            CommitState::Working => ("$mtl_commit_working".to_string(), String::new()),
            // The folder it landed in — a path, so the reviewer can go look.
            CommitState::Done(dir) => (dir.clone(), String::new()),
            CommitState::Failed { token, detail } => (token.to_string(), detail.clone()),
        };
        m.set("commit_status", commit_status);
        m.set("commit_detail", commit_detail);
        m
    }

    /// What the status line says, worst first — why the preview did not bake, why the
    /// last edit was refused, else the first of the graph's problems — as the TOKEN that
    /// says it (a stringtable key per kind of error, localized at the draw) and the
    /// specifics beside it as raw data (the node, port or parameter a graph error names;
    /// a reviewer needs them). Both empty when there is nothing to say.
    pub(crate) fn status_line(&self) -> (String, String) {
        if let Some(e) = &self.bake_error {
            let (token, detail) = error_parts(e);
            return (token.to_string(), detail);
        }
        if let Some(notice) = self.designer.notice() {
            return (
                notice.to_string(),
                self.designer.notice_detail().to_string(),
            );
        }
        if let Some(e) = self.designer.problems().first() {
            let (token, detail) = error_parts(e);
            return (token.to_string(), detail);
        }
        match &self.commit_warning {
            Some((token, detail, at)) if *at == self.designer.mutations() => {
                (token.to_string(), detail.clone())
            }
            _ => (String::new(), String::new()),
        }
    }

    /// The frame's full model: the raw variables plus the pair script's derived values.
    pub(crate) fn model(&self) -> ValueMap {
        let raw = self.hud_model();
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("materialbench: publishing the model to the script failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => m.extend(derived),
                Ok(None) => {}
                Err(e) => tracing::error!("materialbench: derive() failed: {e}"),
            }
        }
        m
    }

    /// The bake rung a Commit will use.
    pub fn bake_rung(&self) -> &'static flicker_texture::BakeSize {
        flicker_texture::size::offered()
            .nth(self.size_rung)
            .or_else(|| flicker_texture::size::offered().next())
            .expect("the ladder always offers at least the baseline")
    }

    /// Step the bake rung — only OFFERED rungs are reachable (4K/8K stay off the picker
    /// until the engine-level memory budget lands).
    pub(crate) fn step_size(&mut self) {
        let n = flicker_texture::size::offered().count().max(1);
        self.size_rung = (self.size_rung + 1) % n;
    }

    /// The option the material dropdown echoes: 0 = Unbound, `1 + i` = the index's
    /// `i`-th material, and -1 (the select's placeholder) for a slot the index does not
    /// list — bound all the same, which the dispatcher must never read as Unbound.
    pub(crate) fn material_option(&self) -> f64 {
        match self.designer.graph().material {
            None => 0.0,
            Some(id) => self
                .materials
                .iter()
                .position(|def| id == def.id)
                .map_or(-1.0, |i| (i + 1) as f64),
        }
    }

    /// Bind (or unbind) the material from a dropdown pick: option 0 = Unbound, option
    /// `1 + i` = the `i`-th material. A value that names no option (the placeholder, past
    /// the list) binds nothing. See [`Self::bind_slot`] for what a pick does.
    pub(crate) fn set_material_by_option(&mut self, opt: f64) -> Binding {
        let pick = opt.round();
        if pick < 0.0 {
            return Binding::Unchanged;
        }
        let material = match pick as usize {
            0 => None,
            i => match self.materials.get(i - 1) {
                Some(def) => Some(def.id),
                None => return Binding::Unchanged,
            },
        };
        self.bind_slot(material)
    }

    /// Bind the document to `material` (`None` unbinds). A slot whose look has already
    /// been COMMITTED (its bundle sits in `staging/`, or in `package/` where a promotion
    /// moved it — [`commit::committed`]) ALWAYS asks first, through the committed-look
    /// prompt ([`Binding::Asked`]): OPEN that look (the designer's one whole-document load;
    /// the option says so when it would discard unsaved edits), BIND the document in hand
    /// there (its next Commit replaces the look), or keep editing — the answer comes back
    /// through `modal_closed` ([`Self::replace_pending`], [`Self::bind_here`],
    /// [`Self::keep_editing`]). Nothing changes before it does.
    ///
    /// A slot with nothing committed only re-binds the document in hand. A committed
    /// document that will not read or load is named — in the log, and as the status line's
    /// notice with its path and reason as data — and the pick only re-binds. Re-picking the
    /// slot already bound changes nothing.
    pub(crate) fn bind_slot(&mut self, material: Option<MaterialId>) -> Binding {
        if material == self.designer.graph().material {
            return Binding::Unchanged;
        }
        if let Some(slot) = material {
            match commit::committed(&self.staging, &self.package, slot) {
                Ok(Some(graph)) => {
                    self.ask(Pending {
                        graph,
                        kind: PendingKind::Committed { slot },
                    });
                    return Binding::Asked;
                }
                Ok(None) => {}
                Err(why) => {
                    tracing::warn!(
                        slot,
                        "materialbench: the committed material will not open — {why}"
                    );
                    self.designer.bind(material);
                    self.designer.refuse_with(COMMITTED_UNREADABLE, why);
                    return Binding::Rebound;
                }
            }
        }
        if self.designer.bind(material) {
            Binding::Rebound
        } else {
            Binding::Unchanged
        }
    }

    /// A whole-document LOAD — a preset (`Some(index)`) or a Roll (`None`): now, or — when
    /// the graph holds edits not yet loaded or committed ([`designer::Designer::unsaved`])
    /// — not before the shared unsaved-changes prompt says Discard; Keep editing hands
    /// `control` (the select the load came from, if any) back to the Model. Whether the
    /// image changed now.
    pub(crate) fn load_or_ask(
        &mut self,
        graph: MaterialGraph,
        preset: Option<usize>,
        control: Option<&'static str>,
    ) -> bool {
        if self.designer.unsaved() {
            self.ask(Pending {
                graph,
                kind: PendingKind::Load { preset, control },
            });
            return false;
        }
        self.load_document(graph, preset);
        true
    }

    /// Make `graph` the document: the designer's one whole-document load, `preset` the one
    /// it came from (`None` for a committed look or a Roll), a new workspace framed on its
    /// output — and the last Commit's warning, about another document, goes.
    fn load_document(&mut self, graph: MaterialGraph, preset: Option<usize>) {
        self.designer.load(graph);
        self.preset = preset;
        self.commit_warning = None;
        self.new_workspace();
    }

    /// Arm a prompt over `pending` for THIS frame's `update` to push.
    fn ask(&mut self, pending: Pending) {
        self.pending = Some(pending);
        self.prompt_armed = true;
    }

    /// The prompt over what is waiting: the committed-look choice, or the shared
    /// unsaved-changes preset over a load. `None` when nothing waits.
    pub(crate) fn prompt(&self) -> Option<ModalParams> {
        Some(match self.pending.as_ref()?.kind {
            PendingKind::Committed { .. } => Self::committed_prompt(self.designer.unsaved()),
            PendingKind::Load { .. } => ModalParams::unsaved_changes(ASK_REPLACE, ASK_KEEP),
        })
    }

    /// The committed-look prompt's three answers, in the `choice_dialog`'s slot order: its
    /// label token and the bench's own action name. OPEN reads as a discard when the work in
    /// hand is unsaved, so the loss is explicit.
    pub(crate) fn committed_answers(unsaved: bool) -> [(&'static str, &'static str); 3] {
        let open = if unsaved {
            "$mtl_open_committed_discard"
        } else {
            "$mtl_open_committed"
        };
        [
            (open, ASK_REPLACE),
            ("$mtl_bind_here", ASK_BIND_HERE),
            ("$modal_lbl_keep_editing", ASK_KEEP),
        ]
    }

    /// The committed-look prompt: Open (primary), Bind here, Keep editing — the last also its
    /// back-out.
    pub(crate) fn committed_prompt(unsaved: bool) -> ModalParams {
        let [open, bind, keep] = Self::committed_answers(unsaved);
        let keep = ModalOption::secondary(keep.0, keep.1);
        ModalParams::new()
            .title("$mtl_committed_title")
            .body("$mtl_committed_body")
            .option(ModalOption::primary(open.0, open.1))
            .option(ModalOption::secondary(bind.0, bind.1))
            .option(keep.clone())
            .cancellable(keep)
    }

    /// [`ASK_REPLACE`]: the waiting document becomes the one in hand — a committed look
    /// opens, a preset or a Roll loads. Whether anything changed (an image edit).
    pub(crate) fn replace_pending(&mut self) -> bool {
        let Some(Pending { graph, kind }) = self.pending.take() else {
            return false;
        };
        let preset = match kind {
            PendingKind::Committed { .. } => None,
            PendingKind::Load { preset, .. } => preset,
        };
        self.load_document(graph, preset);
        true
    }

    /// [`ASK_BIND_HERE`]: the work in hand STAYS and is bound to the picked slot — its next
    /// Commit replaces the slot's committed look. Only the committed-look prompt offers it;
    /// over anything else it keeps the work.
    pub(crate) fn bind_here(&mut self) {
        match self.pending.as_ref().map(|p| p.kind) {
            Some(PendingKind::Committed { slot }) => {
                self.pending = None;
                self.designer.bind(Some(slot));
            }
            _ => self.keep_editing(),
        }
    }

    /// [`ASK_KEEP`] (or any close of a prompt it did not name): nothing changes — the
    /// waiting document is dropped and the control that asked is handed back to the Model.
    pub(crate) fn keep_editing(&mut self) {
        let control = match self.pending.take().map(|p| p.kind) {
            Some(PendingKind::Committed { .. }) => Some("sel_material"),
            Some(PendingKind::Load { control, .. }) => control,
            None => None,
        };
        if let Some(bind) = control {
            self.decline(bind);
        }
    }

    /// The render class of the index row `slot` binds — `None` for no slot, or one the
    /// index does not list. What a Commit hands the slot law.
    pub(crate) fn class_of(&self, slot: Option<MaterialId>) -> Option<RenderClass> {
        let slot = slot?;
        self.materials
            .iter()
            .find(|def| slot == def.id)
            .and_then(|def| def.render_class)
    }

    /// The option the preset select shows: the preset loaded last, but only while the
    /// document IS it — after the first edit (or a Roll) it is -1, the select's
    /// placeholder, so re-picking that preset is a pick (it reloads), not an echo.
    pub(crate) fn preset_option(&self) -> f64 {
        match self.preset {
            Some(p) if self.designer.pristine() => p as f64,
            _ => -1.0,
        }
    }

    // ── the material rename (an authored-name edit) ──────────────────────────────

    pub(crate) fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    pub(crate) fn rename_draft(&self) -> Option<&str> {
        self.rename.as_ref().map(|r| r.draft.as_str())
    }

    /// Open a rename on the BOUND material. A no-op when nothing is bound.
    pub(crate) fn begin_rename(&mut self) {
        let Some(id) = self.designer.graph().material else {
            return;
        };
        let name = self
            .materials
            .iter()
            .find(|def| id == def.id)
            .map(|def| def.name.clone())
            .unwrap_or_default();
        self.rename = Some(MaterialRename {
            id,
            draft: name,
            armed: false,
        });
        self.ui_state.open_text_entry("rename_field");
    }

    pub(crate) fn cancel_rename(&mut self) {
        self.rename = None;
    }

    pub(crate) fn set_rename_draft(&mut self, text: &str) {
        if let Some(r) = self.rename.as_mut() {
            r.draft = text.to_string();
        }
    }

    /// Once the field's session has been seen open, a frame with it neither open nor
    /// requested means the user left the field — abandoned, exactly like Escape.
    fn watch_rename_session(&mut self) {
        let Some(r) = self.rename.as_mut() else {
            return;
        };
        if self.ui_state.text_entry() {
            r.armed = true;
        } else if r.armed && !self.ui_state.text_entry_requested() {
            self.cancel_rename();
        }
    }

    /// Commit the rename STRICTLY by byte id; an empty name is refused (the field stays
    /// open), a write error is logged and the field stays open.
    pub(crate) fn commit_rename(&mut self) {
        let Some(r) = self.rename.as_ref() else {
            return;
        };
        let name = r.draft.trim().to_string();
        if name.is_empty() {
            self.ui_state.open_text_entry("rename_field");
            return;
        }
        let id = r.id;
        match JsonTableSource::new(&self.data_dir).save_material_name(id, &name) {
            Ok(()) => {
                if let Some(def) = self.materials.iter_mut().find(|def| id == def.id) {
                    def.name = name;
                }
                self.rename = None;
            }
            Err(e) => {
                tracing::error!("material rename failed to write: {e}");
                self.ui_state.open_text_entry("rename_field");
            }
        }
    }

    /// Bake at the chosen rung and write the bundle into `staging/` — on the WORKER.
    pub(crate) fn start_commit(&mut self) {
        if self.commit_state == CommitState::Working {
            return; // one at a time; a second click is not a second artifact
        }
        self.commit_state = CommitState::Working;
        // What this commit stages is the graph as it stands NOW: its success saves exactly
        // that much (`commit_landed`), and an edit made while it bakes stays unsaved.
        let graph = self.designer.graph().clone();
        // An emissive material's look that does not glow is legal (the slot law weighs the
        // row, not the look) but almost surely a mistake: its Commit WARNS when it lands.
        self.committing = Some(Committing {
            mutations: self.designer.mutations(),
            dull: self.class_of(graph.material) == Some(RenderClass::Emissive) && !graph.glows(),
        });
        self.commit_warning = None;
        // The slot law weighs the commit by the class of the index row the slot binds.
        let class = self.class_of(graph.material);
        let size = self.bake_rung().px;
        let staging = self.staging.clone();
        let tx = self.commit_tx.clone();
        self.pool.submit(move || {
            let state = match commit::commit(&graph, class, size, &staging) {
                Ok(out) => CommitState::Done(out.dir.display().to_string()),
                // A refusal is authored copy: the bench publishes its TOKEN and the
                // specifics go to the log.
                Err(commit::CommitError::Refused(why)) => {
                    tracing::warn!(
                        graph = %graph.id,
                        slot = ?graph.material,
                        ?why,
                        "commit refused"
                    );
                    CommitState::Failed {
                        token: why.token(),
                        detail: String::new(),
                    }
                }
                // A graph that cannot bake: the token for its kind of error, and the node
                // and port it names as data.
                Err(commit::CommitError::Graph(e)) => {
                    let (token, detail) = error_parts(&e);
                    CommitState::Failed { token, detail }
                }
                // A write that failed: the cause the platform gives, as data.
                Err(commit::CommitError::Io(e)) => CommitState::Failed {
                    token: "$mtl_commit_io_failed",
                    detail: e.to_string(),
                },
            };
            let _ = tx.send(state);
        });
    }

    /// A Commit's report, back from the worker: publish it — and when it STAGED, the work
    /// it staged is saved (the unsaved-changes guard stands down until the next edit), and
    /// a dull emissive look leaves its warning on the status line. A refused or failed
    /// commit saves nothing.
    pub(crate) fn commit_landed(&mut self, state: CommitState) {
        if let CommitState::Failed { token, detail } = &state {
            tracing::error!("commit failed: {token} {detail}");
        }
        if let (Some(started), CommitState::Done(dir)) = (self.committing.take(), &state) {
            self.designer.saved_through(started.mutations);
            if started.dull {
                self.commit_warning =
                    Some((NO_EMIT_WARNING, dir.clone(), self.designer.mutations()));
            }
        }
        self.commit_state = state;
    }

    /// Aim a Commit at a different staging root — a test's seam (the app resolves its
    /// root once at construction).
    #[cfg(test)]
    pub(crate) fn set_staging_root(&mut self, root: PathBuf) {
        self.staging = root;
    }

    /// Open promoted looks from a different package root — a test's seam, as above.
    #[cfg(test)]
    pub(crate) fn set_package_root(&mut self, root: PathBuf) {
        self.package = root;
    }

    // ── the graph canvas ─────────────────────────────────────────────────────────

    /// Re-derive the canvas content when the document's revision moved, and forget the
    /// hand placement of every card that is gone (a deleted node's).
    pub(crate) fn sync_canvas(&mut self) {
        if self.canvas_rev == Some(self.designer.rev()) {
            return;
        }
        // The engine's ONE projection of a graph onto cards, worded by the stringtable as it
        // stands now (each kind's name, the output card's heading and its port labels); the
        // selected node inks its card and its wires, the selected wire itself.
        let selected_edge = self.designer.selected_edge().map(Sink::shared);
        self.canvas_content = GraphCards::project(
            self.designer.graph(),
            &CardLabels::stringtable(),
            self.designer.selected_key(),
            selected_edge.as_ref(),
        );
        self.canvas_rev = Some(self.designer.rev());
        let keys = &self.canvas_content.keys;
        self.canvas
            .retain_placements(|k| keys.iter().any(|c| c == k));
    }

    /// A new document is a new workspace: the last one's camera and hand placements go,
    /// and the next seated frame frames the whole graph.
    pub(crate) fn new_workspace(&mut self) {
        self.canvas.reset_view();
        self.fit_pending = Some(Framing::Output);
    }

    /// The document DECLINED the edit a control reported on `bind` (refused it, or it
    /// changed nothing): hand the control back to the Model, so it shows — and echoes —
    /// what the document kept instead of holding the declined value forever.
    pub(crate) fn decline(&mut self, bind: &str) {
        self.ui_state.release_local(bind);
    }

    /// Seat, arrange and lay out the canvas: every card that has no hand placement yet is
    /// pinned in free grid space (so inserting or deleting a node never shifts the rest),
    /// and an owed fit frames the whole graph.
    fn arrange_canvas(&mut self, seat: Rect) {
        self.sync_canvas();
        self.canvas.seat(seat);
        let keys = self.canvas_content.key_refs();
        self.canvas.pin(&keys);
        if let Some(framing) = self.fit_pending.take() {
            // Legibly: the whole graph while that reads; past the floor, at the floor on
            // the output (a load) or the selection (Reset view).
            let focus = match framing {
                Framing::Output => OUTPUT_CARD,
                Framing::Selection => self.designer.selected_key().unwrap_or(OUTPUT_CARD),
            };
            self.canvas.fit_legible(&keys, self.fit_floor, focus);
        }
        self.canvas.layout_with_ports(
            &keys,
            &self.canvas_content.ports,
            &self.canvas_content.edges,
        );
    }

    /// Seat, lay out and drive the canvas with this frame's pointer sample, and hand its
    /// gestures to the dispatcher. Unseated (the page is hidden) it abandons any gesture
    /// in flight. Returns whether the image changed.
    ///
    /// The pointer is the WALKER's to give: its sample for this surface is present while
    /// the cursor is over the canvas with nothing painted over it claiming, or while a
    /// press that began there is held — the panel AROUND the canvas claims its rect
    /// (`hud_hit` is true all over the canvas), and is rightly not what occludes it. What
    /// does is an open dropdown's menu, which the walker answers itself (`popup_hit`: the
    /// cursor is in the menu, or this frame's click was spent closing it) — no press, wheel
    /// notch or pan of that frame is the canvas's. A link released OUTSIDE the seat is
    /// abandoned — a card a pan pushed past the seat's edge is clipped from view, and a
    /// drop must land on something the author can see.
    pub(crate) fn drive_canvas(
        &mut self,
        slot: Option<&SurfaceSlot>,
        pointer: Option<&SurfacePointer>,
        middle: bool,
        popup_hit: bool,
    ) -> bool {
        let Some(slot) = slot else {
            self.canvas.cancel();
            return false;
        };
        let seat = graph_view::seat_of(slot);
        self.arrange_canvas(seat);
        self.canvas.set_mode(self.canvas_mode);
        // The walker's sample, gated the one way every graph host gates it: the left
        // button's press, never under an open menu; the wheel and a middle-drag pan only
        // over the seat.
        let (sample, over_seat) = graph_view::shape_pointer(pointer, seat, middle, popup_hit);
        let keys = self.canvas_content.key_refs();
        let mut events = self.canvas.pointer(&sample, &keys);
        if !over_seat {
            events.linked = None;
        }
        let canvas = &self.canvas;
        let release = events.linked.and_then(|_| {
            with_nodes(&self.canvas_content, |nodes| {
                canvas.port_at(sample.cursor, nodes)
            })
        });
        route::canvas(self, &events, release)
    }

    /// Walk the frame tree on the bench's OWN UI state against `model` — seating the lit
    /// sample and keeping the walker's draw commands for this frame's HUD — and hand back
    /// the rest of the walk: its results, rects, surfaces and popup answer.
    pub(crate) fn walk(&mut self, tree: &UiNode, model: &ValueMap, input: &UiInput) -> UiFrame {
        let mut frame = run_ui(tree, model, &self.ui_styles, input, &mut self.ui_state);
        self.lit_slot = frame.surface(LIT_SLOT).cloned();
        self.sample.seat(self.lit_slot.as_ref());
        self.hud_commands = std::mem::take(&mut frame.commands);
        frame
    }

    /// The wheel over the Lit view's seat ZOOMS the sample — one authored `step` per notch,
    /// inside the authored range, by the engine's one wheel law (`MaterialSample::zoom_by`,
    /// the inspector's too). The walker's sample for the seat is gated the one way every host
    /// gates a surface's pointer ([`graph_view::shape_pointer`]): it counts only while the
    /// cursor is over the seat and no open menu owns it. Not an image edit — the zoom is the
    /// view's — and the slider follows through the next frame's Model. Unseated (another
    /// view showing), there is no seat to wheel over.
    pub(crate) fn zoom_lit(&mut self, pointer: Option<&SurfacePointer>, popup_hit: bool) {
        let Some(slot) = self.lit_slot.as_ref() else {
            return;
        };
        let (shaped, _) =
            graph_view::shape_pointer(pointer, graph_view::seat_of(slot), false, popup_hit);
        if shaped.inside && shaped.wheel != 0.0 {
            self.sample
                .zoom_by(shaped.wheel, self.zoom.step, self.zoom.range());
        }
    }

    /// Settle a walked frame: dispatch `results` (the walk's, with any fired intent names
    /// folded in) against the Model the frame was drawn from, drive the canvas with the
    /// walker's own sample for its seat — deferring to an open menu — and the Lit view's
    /// zoom wheel with its own, re-bake on an edit, and draw the canvas over the walker's
    /// commands. Whether the image changed.
    pub(crate) fn settle(
        &mut self,
        model: &ValueMap,
        frame: &UiFrame,
        results: &ValueMap,
        middle: bool,
    ) -> bool {
        let slot = frame.surface(CANVAS_SLOT).cloned();
        let pointer = frame.surface_pointer(CANVAS_SLOT).cloned();
        // The controls first (against the Model this frame was drawn from), then the
        // canvas's gestures and the lit sample's wheel.
        let mut edited = route::apply(self, model, results);
        edited |= self.drive_canvas(slot.as_ref(), pointer.as_ref(), middle, frame.popup_hit);
        self.zoom_lit(frame.surface_pointer(LIT_SLOT), frame.popup_hit);
        if edited {
            self.request_bake();
        }
        self.retire_stale_warning();
        self.draw_canvas(slot.as_ref());
        edited
    }

    /// The last Commit's warning speaks of the STAGED look: once the graph has been edited
    /// since (any mutation — the designer's unsaved counter moved), it no longer describes
    /// what is on screen, and goes.
    pub(crate) fn retire_stale_warning(&mut self) {
        if self
            .commit_warning
            .as_ref()
            .is_some_and(|(_, _, at)| *at != self.designer.mutations())
        {
            self.commit_warning = None;
        }
    }

    /// Draw the canvas into this frame's HUD commands, over the walker's own — after the
    /// dispatcher ran, so a pick shows on the frame it was made — inside a SCISSOR on its
    /// seat: a panned or zoomed card lies wherever the view put it, and must not paint
    /// over the panels around the canvas (the Component Catalog's filler-run clip).
    fn draw_canvas(&mut self, slot: Option<&SurfaceSlot>) {
        let Some(slot) = slot else {
            return;
        };
        self.arrange_canvas(graph_view::seat_of(slot));
        let (canvas, content, style) = (&self.canvas, &self.canvas_content, &self.canvas_style);
        let out = &mut self.hud_commands;
        out.push(HudCommand::Clip {
            rect: Some([slot.x, slot.y, slot.w, slot.h]),
        });
        with_nodes(content, |nodes| {
            canvas.draw(nodes, &content.edges, style, slot.layer, out)
        });
        out.push(HudCommand::Clip { rect: None });
    }

    /// The insertion menu — the shared popup chrome, one row per node kind in menu order
    /// (the pad path; the radial menu is a later slice). Each row fires `insert_<kind>`,
    /// which comes back through `modal_closed` to the dispatcher.
    fn insert_menu() -> ModalParams {
        let mut params = ModalParams::new().title("$mtl_insert_title");
        for kind in NodeKind::catalog() {
            params = params.option(ModalOption::secondary(
                graph_view::kind_token(kind.id()),
                format!("insert_{}", kind.id()),
            ));
        }
        params.cancellable(ModalOption::secondary("$mtl_cancel", "insert_cancel"))
    }

    // ── read-only accessors, for tests ─────────────────────────────────────────

    pub fn graph(&self) -> &MaterialGraph {
        self.designer.graph()
    }

    /// The map the swatch shows (on the LIT tab: the base colour, the lit albedo).
    pub fn selected_map(&self) -> MapKind {
        MapKind::ALL[self.sel_map.min(MapKind::ALL.len() - 1)]
    }

    pub fn showing_lit(&self) -> bool {
        self.sel_map == MAP_IDS.len()
    }

    /// The generation of the newest preview handed to the scene.
    pub fn shown_generation(&self) -> u64 {
        self.shown
    }

    /// The AUTHORED tree, as the scene file wrote it.
    pub fn authored_tree(&self) -> Option<UiNode> {
        self.authored.clone()
    }

    /// The tree the walker walks this frame (every `rows_from` expanded).
    pub fn frame_tree(&mut self) -> Option<UiNode> {
        self.refresh_frame_tree();
        self.frame_tree.clone()
    }
}

impl Scene for MaterialBench {
    /// The rename field owns the keyboard while its session is open.
    fn input_context(&self) -> Option<InputContext> {
        self.ui_state
            .text_entry()
            .then_some(InputContext::TextEntry)
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        let theme = Theme::build(renderer);
        self.ui_theme = Some(theme);
        self.adopt_styles();

        // The preview textures, created ONCE at the tiled size and rewritten in place.
        // Base colour is the only sRGB map; the rest carry data.
        let px = self.tex_px();
        let blank = vec![0u8; (px * px * 4) as usize];
        self.tex = MapKind::ALL
            .iter()
            .map(|kind| {
                if kind.is_color() {
                    renderer.load_texture(&blank, px, px)
                } else {
                    renderer.load_texture_linear(&blank, px, px)
                }
            })
            .collect();
        // One 1×1 texture per palette colour — the swatch strip's sprites name them by
        // the texture slot the Model publishes (`glow<i>_tex`).
        self.swatches = self
            .palette
            .iter()
            .map(|c| renderer.load_texture(&swatch_texel(c.rgb), 1, 1))
            .collect();
        self.hud_tex = theme
            .lua_textures()
            .iter()
            .map(|(_, h)| *h)
            .chain(self.tex.iter().copied())
            .chain(self.swatches.iter().copied())
            .collect();
        self.dirty = [true; MapKind::ALL.len()];

        renderer.window().set_title("MaterialBench");
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        self.collect_bakes();
        while let Ok(state) = self.commit_rx.try_recv() {
            self.commit_landed(state);
        }

        self.refresh_frame_tree();
        let Some(tree) = self.frame_tree.take() else {
            return Transition::None;
        };
        let screen = renderer.size();
        self.watch_rename_session();
        let model = self.model();
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen,
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        // The `surface` nodes reserve the lit sample's seat and the canvas's (`None` while
        // their page or tab is not showing); the walk seats the sample.
        let frame = self.walk(&tree, &model, &snap);
        let over_hud = frame.results.is_on("hud_hit");
        self.sample.tick(dt);

        // ── The input seam: the PUMP resolved this frame's events; one dispatch through
        // the walker (focus graph, pointer consumption, the declared intents). ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&tree, &model)
            .with_rects(&frame.rects)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let fired = walker.take_fired();
        self.frame_tree = Some(tree);

        // Fold the fired intent names in beside the click results, so both channels
        // reach the ONE dispatcher identically.
        let mut results = frame.results.clone();
        for name in fired {
            results.set(name, true);
        }
        if let Some(t) = results.text("rename_draft") {
            self.set_rename_draft(t);
        }

        // The dispatcher owns every edit and says whether the image changed; the canvas
        // is driven and drawn with the walker's own answers.
        self.settle(&model, &frame, &results, input.mouse_middle);

        // The insertion menu: the shared popup modal, opened the way pause is, its pick
        // coming back through `modal_closed`.
        if std::mem::take(&mut self.ask_insert) {
            if let Some(theme) = self.ui_theme {
                return Transition::Push(Box::new(SharedModal::open(
                    theme,
                    "popup_menu",
                    Self::insert_menu(),
                )));
            }
        }
        // The prompt a slot pick or a load armed: the SHARED `choice_dialog`, opened by id
        // with the bench's own action names as its options and pushed exactly the way the
        // insertion menu is; its answer comes back through `modal_closed`. With no theme to
        // draw it (never, past `enter`) the safe answer is taken: keep the work.
        if std::mem::take(&mut self.prompt_armed) {
            match (self.ui_theme, self.prompt()) {
                (Some(theme), Some(params)) => {
                    return Transition::Push(Box::new(SharedModal::open(
                        theme,
                        PROMPT_TREE,
                        params,
                    )));
                }
                _ => self.keep_editing(),
            }
        }
        if results.is_on("pause_open") && !self.is_renaming() {
            if let Some(theme) = self.ui_theme {
                let pause_map = flicker_shell::input_profile()
                    .context_map("World")
                    .cloned()
                    .unwrap_or_else(InputMap::wasd_and_mouse);
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    &pause_map,
                    &AbstractControls::default(),
                    &GamepadConfig::default(),
                )));
            }
        }
        Transition::None
    }

    /// A shared modal closed over the bench (the insertion menu, a prompt): its answer is a
    /// fired result name, folded into the ONE dispatcher like a click. A prompt closed any
    /// way but its named answers — a back-out it did not name — keeps the work: the safe
    /// answer, never a silent discard.
    fn modal_closed(&mut self, modal: &str, result: &str, _payload: Option<&str>) {
        let named = result == ASK_REPLACE || result == ASK_BIND_HERE;
        let result = if modal == PROMPT_TREE && self.pending.is_some() && !named {
            ASK_KEEP
        } else {
            result
        };
        let mut r = ValueMap::new();
        r.set(result, true);
        let published = self.model();
        if route::apply(self, &published, &r) {
            self.request_bake();
        }
        self.retire_stale_warning();
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        self.upload_shown(renderer);
        // The lit sample is an offscreen surface composited into the rect the walk
        // reserved; the HUD replay is the screen surface's final 2D.
        if self.lit_slot.is_some() && self.lit_stage.is_some() {
            self.upload_all(renderer);
            let base = fg.base_layer();
            let bodies = self
                .bodies
                .get_or_insert_with(|| SampleBodies::new(renderer));
            if let Some(stage) = self.lit_stage.as_ref() {
                if let Some(maps) = SampleMaps::of_set(&self.tex, stage.emissive_gain) {
                    self.sample.render(renderer, fg, base, bodies, &maps, stage);
                }
            }
        }
        if let Some(&white) = self.hud_tex.first() {
            let (hud_commands, textures) = (&self.hud_commands, &self.hud_tex);
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
        }
    }

    /// Give the GPU resources back on the way out — the lit sample's offscreen target and
    /// the bodies' meshes, then the preview and swatch textures this bench uploaded. The
    /// renderer owns them all and the scene holds only handles (rule 728E682F).
    fn exit(&mut self, renderer: &mut Renderer) {
        self.sample.release(renderer);
        if let Some(bodies) = self.bodies.take() {
            bodies.release(renderer);
        }
        for t in self.tex.drain(..).chain(self.swatches.drain(..)) {
            renderer.free_texture(t);
        }
        self.hud_tex.clear();
    }
}

/// The scene factory the launcher roster registers.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(MaterialBench::new(def))
}

#[cfg(test)]
mod tests;
