//! **The MATERIAL INSPECTOR** — the modal a pressed box of the Material Catalog opens
//! (Aaron 2026-10-21, A5972856): a centre-screen stage showing just that material and its
//! graph read-only, what the PACKAGE holds for the slot above it, and the material's
//! DEFINITION below.
//!
//! # A shared tree with a host of its own
//!
//! The tree is the shared modal `scenes/shared/material_inspector.scene.json` (ruling
//! 2A3592D0 R1 — pop-up modals are shared modal TREES). Its host is THIS scene rather than
//! the shell's `SharedModal`, the settings precedent, because what it shows is LIVE — a
//! turning lit sample on the tree's own stage and the graph on a canvas — and the scene
//! beneath a modal is frozen (`SceneManager::update` runs the top scene only), so the modal
//! scene must own them. The shell names this host in its `HOSTED_ELSEWHERE` registry (so
//! the param seam refuses the id by name) and exempts the file from its param registry.
//!
//! # Never a trap — the shell's mechanism, not a variant of it
//!
//! - The tree loads through [`hosted_modal_tree`]: THE parse path every shared modal uses,
//!   which writes the host's exit ([`MODAL_CANCEL`]) onto the root whatever the file
//!   authored — a file that will not parse still yields a back-out-able overlay.
//! - Esc / pad-B reach that exit through the walker; the Close button fires the same name.
//!   The answer is [`modal_cancelled`], the one transition the param seam answers with too.
//! - Whether the exit may be taken is the slab's `dismissable` toggle; the tree authors it
//!   `true` and nothing here ever holds it shut.
//! - `is_overlay` + [`InputContext::Menu`], exactly as `SharedModal`.
//!
//! # The five-line split
//!
//! The tree, its anchors, its stage, its own style blocks and its tunable numbers are the
//! shared file's. This module is the drawing code and the RAW publish: it gathers the facts
//! once ([`InspectorFacts::gather`] — pure, no GPU), publishes them as Model values (every
//! label beside them is a `$token` the tree authors; a state is a token per state), seats the
//! engine's lit sample ([`MaterialSample`]) in `insp_sample` — zoomed by the tree's own
//! `insp_zoom` slider and the wheel over it, inside the range that slider authors
//! ([`ZoomDial`]) — seats the node-graph filler
//! ([`GraphCanvas`], [`CanvasMode::Inspect`] — pan, zoom, pick; nothing moves, nothing links)
//! in `insp_canvas` with the cards the engine's ONE graph projection derives
//! ([`flicker_materialview::graph_view`] — the projection the MaterialBench draws through
//! too), expands the node list's one prototype per card through the walker's own repeater
//! ([`instantiate_rows`] — the pad's way to read a node), and answers the names its buttons
//! and rows fire. No node is composed here; no style is written here.
//!
//! # The decode never holds the frame
//!
//! A bundle's maps decode on the CATALOG's long-lived worker pool, handed in by reference —
//! this scene owns no pool, so closing it never joins a thread — and the job is cancelled
//! between maps ([`decode_bundle_maps_with`](flicker_materialview::decode_bundle_maps_with)),
//! so a close mid-decode frees that worker after at most the map in hand.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flicker::render::{FrameGraph, Renderer, StageDef, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, UiNode, ValueMap};
use flicker::ui::{
    focusables_of, instantiate_rows, render_hud, run_ui, strings, Row, SurfacePointer, SurfaceSlot,
    UiFrame, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_canvas::{CanvasMetrics, CanvasMode, CanvasStyle, GraphCanvas};
use flicker_content::{bundle_at, human_bytes, FileFacts, MaterialBundle};
use flicker_input_core::{InputContext, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_materials::{band_of, MaterialBand, MaterialDef, MaterialId, RenderClass};
use flicker_materialview::graph_view::{self, Card, CardLabels, GraphCards, OUTPUT_CARD};
use flicker_materialview::{
    map_path, upload_decoded, Body, FlatMaterial, FlatTextures, MaterialSample, SampleBodies,
    ZoomDial,
};
use flicker_shell::{hosted_modal_styles, hosted_modal_tree, modal_cancelled, Theme, MODAL_CANCEL};
use flicker_texture::graph::OUTPUT_PORTS;
use flicker_texture::{load_material_json, rung, MapKind, MaterialGraph};
use flicker_worker::WorkerPool;

use crate::{decode_job, decode_px, sample_stage_check, Decoded, SlotMaps};

/// The shared tree's id — the name the shell's `HOSTED_ELSEWHERE` registers this host
/// under, and the `modal` a close reports to the scene beneath.
pub const INSPECTOR_ID: &str = "material_inspector";
/// The shared tree, embedded like every shared modal (the manifest skips `scenes/shared/`).
pub(crate) const INSPECTOR_JSON: &str =
    include_str!("../../../../content/sensorium/scenes/shared/material_inspector.scene.json");
/// The file's name, as a warning names it.
const INSPECTOR_FILE: &str = "material_inspector.scene.json";
/// The stage the lit sample is drawn under (`stages.<source>` in the shared file).
pub const INSPECTOR_STAGE: &str = "inspector_sample";
/// The authored surfaces this host fills, spelled once.
pub(crate) const SAMPLE_SLOT: &str = "insp_sample";
pub(crate) const CANVAS_SLOT: &str = "insp_canvas";
/// The Sphere/Plane toggle's action — the MaterialBench's `lit_body`, under this tree's name.
pub(crate) const BODY_ACTION: &str = "insp_body";
/// The sample's ZOOM slider — its node id and its bind, one name (the MaterialBench's
/// `lit_zoom`, under this tree's name). Its authored `min` / `max` / `step` ARE the zoom's
/// range and the wheel's notch ([`ZoomDial::authored`]).
pub(crate) const ZOOM: &str = "insp_zoom";
/// The params this host reads (`params.<key>` in the shared file).
const MAP_PX_PARAM: &str = "map_px";
const FIT_FLOOR_PARAM: &str = "canvas_fit_floor";
/// The node list's row source and its scroll bind (`rows_from` / `bind` in the shared file).
const NODE_ROWS: &str = "insp_nodes";
pub(crate) const NODE_SCROLL: &str = "insp_node_scroll";
/// The tree's prefix for the canvas's ink (`inspector.canvas.<slot>`).
const CANVAS_INK: &str = "inspector.canvas";

/// The Model keys this host writes that are not one of [`InspectorFacts::model`]'s.
pub(crate) const TITLE: &str = "insp_title";
pub(crate) const BODY_LABEL: &str = "insp_body_label";
pub(crate) const NODE_LABEL: &str = "insp_node_label";
pub(crate) const NODE_INPUTS: &str = "insp_node_inputs";
pub(crate) const NODE_PARAMS: &str = "insp_node_params";
pub(crate) const GRAPH_NOTE: &str = "insp_graph_note";
pub(crate) const SWATCH_TEX: &str = "insp_swatch_tex";

/// The value a fact reads when there is nothing to say (a dash).
const NONE: &str = "$insp_none";
/// The word for a slot no material row holds.
const UNASSIGNED: &str = "$insp_unassigned";
/// What the read-out under the canvas says before a card is picked.
const NODE_HINT: &str = "$insp_node_hint";

/// The action the node list's row `row` fires — the prototype's `insp_nl_{row}`, spelled
/// once for the answer ladder.
pub(crate) fn row_action(row: usize) -> String {
    format!("insp_nl_{row}")
}

// ── the facts ────────────────────────────────────────────────────────────────

/// Where the promotion path holds a slot's bundle — asked of the PACKAGE first, then of
/// STAGING (plan 30FE7F58 P3: a slot is DEFINED iff the package holds its bundle; staging is
/// where the MaterialBench commits and the PackageBench promotes from).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Package,
    Staging,
    Nowhere,
}

impl Held {
    /// The state's token — one per state; the tree draws it resolved.
    fn token(self) -> &'static str {
        match self {
            Held::Package => "$insp_state_package",
            Held::Staging => "$insp_state_staging",
            Held::Nowhere => "$insp_state_none",
        }
    }
}

/// The bundle's recipe document, as the ONE material loader reads it.
#[derive(Debug)]
pub enum Recipe {
    /// No bundle — nothing to read.
    Absent,
    /// A bundle whose recipe does not read, or does not load as a v1 recipe or a v2 graph
    /// (warned, naming the file and why).
    Invalid,
    /// The graph, and the version its document DECLARES (a versionless document is the v1
    /// recipe form by definition — the loader's own rule).
    Loaded {
        version: u32,
        graph: Box<MaterialGraph>,
    },
}

/// Everything the inspector shows about one slot, gathered ONCE when it opens — pure data,
/// no GPU, so a gate reads exactly what a player sees.
pub struct InspectorFacts {
    pub slot: MaterialId,
    /// The slot's row of the material index; `None` for a FREE slot.
    pub row: Option<MaterialDef>,
    /// The authoring band the id is filed under (`materials.json` `_meta.bands`).
    pub band: Option<MaterialBand>,
    pub held: Held,
    pub bundle: Option<MaterialBundle>,
    pub recipe: Recipe,
    /// The map roles the bundle holds, in [`MapKind::ALL`] order.
    pub maps: Vec<MapKind>,
    /// The base colour map's pixel size, read off its header.
    pub map_size: Option<(u32, u32)>,
    /// The bundle's bytes on disk — the maps and the recipe at rest (its `.gz` twin).
    pub bytes: u64,
    /// The newest modification time among the bundle's files.
    pub modified: Option<SystemTime>,
}

impl InspectorFacts {
    /// Gather the facts of `slot`: its row (`rows` — the index), its band (`bands`), and
    /// what the promotion path holds for it — `package` asked first, then `staging`, each
    /// through the P3 bundle seam ([`bundle_at`], root-agnostic by design: the two sides are
    /// one relative path under different roots). The recipe is read through the bundle's
    /// gz-at-rest seam and loaded through the ONE loader every reader uses
    /// ([`load_material_json`], the PackageBench's too). Every failure is LOUD — warned,
    /// naming the slot and the file — and shows as a fact, never as a silent blank.
    pub fn gather(
        slot: MaterialId,
        rows: &[MaterialDef],
        bands: &[MaterialBand],
        package: &Path,
        staging: &Path,
    ) -> Self {
        let (held, bundle) = match bundle_at(package, slot) {
            Some(b) => (Held::Package, Some(b)),
            None => match bundle_at(staging, slot) {
                Some(b) => (Held::Staging, Some(b)),
                None => (Held::Nowhere, None),
            },
        };
        let recipe = bundle.as_ref().map_or(Recipe::Absent, read_recipe);
        let maps = bundle.as_ref().map_or_else(Vec::new, |b| {
            MapKind::ALL
                .into_iter()
                .filter(|&k| map_path(b, k).is_some())
                .collect()
        });
        let map_size = bundle.as_ref().and_then(base_map_size);
        let (bytes, modified) = bundle.as_ref().map_or((0, None), on_disk);
        Self {
            slot,
            row: rows.iter().find(|r| r.id == slot).cloned(),
            band: band_of(bands, slot).cloned(),
            held,
            bundle,
            recipe,
            maps,
            map_size,
            bytes,
            modified,
        }
    }

    /// The loaded graph, when there is one.
    pub fn graph(&self) -> Option<&MaterialGraph> {
        match &self.recipe {
            Recipe::Loaded { graph, .. } => Some(graph.as_ref()),
            _ => None,
        }
    }

    /// The facts as RAW Model values — every key the tree binds for them. Values are data
    /// (a name, a count, a size, a date, the index's own words) or a `$token` the walker
    /// resolves at draw (a state, a render class, a dash). The title is resolved here: the
    /// slab draws its live title as given.
    pub fn model(&self) -> ValueMap {
        let mut m = ValueMap::new();
        let none = || NONE.to_string();
        let title = match &self.row {
            Some(row) => row.name.clone(),
            None => strings::resolve(UNASSIGNED).into_owned(),
        };
        m.set(TITLE, title);

        // ── HEADER: what the promotion path holds ──
        m.set("insp_pkg_state", self.held.token());
        m.set("insp_pkg_note", self.note());
        let loaded = match &self.recipe {
            Recipe::Loaded { version, graph } => Some((*version, graph.as_ref())),
            _ => None,
        };
        // The version is the number the document declares; the label beside it says what
        // it is (`$insp_lbl_version`), so no prefix rides the value.
        m.set(
            "insp_pkg_version",
            loaded.map_or_else(none, |(v, _)| v.to_string()),
        );
        m.set(
            "insp_pkg_nodes",
            loaded.map_or_else(none, |(_, g)| g.nodes.len().to_string()),
        );
        m.set("insp_pkg_maps", self.map_set().unwrap_or_else(none));
        m.set("insp_pkg_size", self.size_line().unwrap_or_else(none));
        m.set(
            "insp_pkg_bytes",
            self.bundle
                .as_ref()
                .map_or_else(none, |_| human_bytes(self.bytes)),
        );
        m.set(
            "insp_pkg_modified",
            self.modified.and_then(utc_stamp).unwrap_or_else(none),
        );
        m.set(
            GRAPH_NOTE,
            match self.recipe {
                Recipe::Absent => "$insp_graph_none",
                Recipe::Invalid => "$insp_graph_invalid",
                Recipe::Loaded { .. } => "",
            },
        );

        // ── FOOTER: the definition ──
        // Decimal, then hex in the language's own alternate notation (`0x0A`).
        m.set("insp_def_id", format!("{} · {:#04X}", self.slot, self.slot));
        m.set(
            "insp_def_band",
            self.band.as_ref().map_or_else(none, |b| b.label.clone()),
        );
        let Some(row) = &self.row else {
            m.set("insp_def_name", UNASSIGNED);
            for key in [
                "insp_def_category",
                "insp_def_class",
                "insp_def_colour",
                "insp_def_extracts",
                "insp_def_represents",
                "insp_def_signature",
                "insp_def_hardness",
                "insp_def_brittleness",
                "insp_def_water",
                "insp_def_viscosity",
                "insp_def_density",
            ] {
                m.set(key, NONE);
            }
            return m;
        };
        let or_none = |s: String| if s.is_empty() { none() } else { s };
        m.set("insp_def_name", row.name.clone());
        m.set("insp_def_category", or_none(row.category.clone()));
        m.set("insp_def_class", class_token(row.render_class));
        m.set("insp_def_colour", hex(row.color));
        m.set(
            "insp_def_extracts",
            row.extracted_element.clone().unwrap_or_else(none),
        );
        m.set("insp_def_represents", or_none(row.represents.join(", ")));
        m.set("insp_def_signature", or_none(row.signature.join(" · ")));
        m.set("insp_def_hardness", num(f64::from(row.hardness)));
        m.set("insp_def_brittleness", num(f64::from(row.brittleness)));
        m.set("insp_def_water", num(f64::from(row.water_capacity)));
        m.set("insp_def_viscosity", num(f64::from(row.viscosity)));
        m.set("insp_def_density", num(f64::from(row.density_g_cm3)));
        // The swatch sprite's texture: the 1×1 index colour this host uploads AFTER the
        // theme's own textures in the HUD's table (`enter`). A free slot publishes none, so
        // its sprite draws nothing.
        m.set(SWATCH_TEX, Theme::TEXTURE_NAMES.len() as f64);
        m
    }

    /// What the state means, in words — one token per case, the worst first: a recipe that
    /// does not load; a bundle short of any of its seven maps (it holds a recipe, but a
    /// sample is drawn in a whole material or not at all, so it cannot be called promoted or
    /// staged as a look); then where the whole bundle is held.
    fn note(&self) -> &'static str {
        match (&self.recipe, self.held) {
            (Recipe::Invalid, _) => "$insp_note_invalid",
            (_, Held::Nowhere) => "$insp_note_none",
            _ if !self.whole() => "$insp_note_incomplete",
            (_, Held::Package) => "$insp_note_package",
            (_, Held::Staging) => "$insp_note_staging",
        }
    }

    /// Whether the bundle holds every one of its seven maps — what a sample needs to wear it.
    fn whole(&self) -> bool {
        self.maps.len() == MapKind::ALL.len()
    }

    /// The map roles the bundle holds, by their content-standard names — `None` (a dash)
    /// with no bundle, or a bundle that holds no map at all.
    fn map_set(&self) -> Option<String> {
        self.bundle.as_ref()?;
        (!self.maps.is_empty()).then(|| {
            self.maps
                .iter()
                .map(|k| k.role())
                .collect::<Vec<_>>()
                .join(" · ")
        })
    }

    /// The maps' pixel size, with its rung of the bake ladder when it is one.
    fn size_line(&self) -> Option<String> {
        let (w, h) = self.map_size?;
        Some(match rung(w).filter(|_| w == h) {
            Some(r) => format!("{w} × {h} · {}", r.label),
            None => format!("{w} × {h}"),
        })
    }
}

/// Read a bundle's recipe and load it — LOUD on either failure.
fn read_recipe(bundle: &MaterialBundle) -> Recipe {
    let text = match bundle.recipe_text() {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(
                "material inspector: slot {}'s recipe does not read: {e:#}",
                bundle.slot
            );
            return Recipe::Invalid;
        }
    };
    match load_material_json(&text) {
        Ok(graph) => Recipe::Loaded {
            version: declared_version(&text),
            graph: Box::new(graph),
        },
        Err(e) => {
            tracing::warn!(
                "material inspector: slot {}'s recipe {} does not load: {e}",
                bundle.slot,
                bundle.recipe.display()
            );
            Recipe::Invalid
        }
    }
}

/// The version a material document DECLARES — its `version`; one that declares none is v1.
fn declared_version(text: &str) -> u32 {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|doc| doc.get("version")?.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(1)
}

/// The base colour map's pixel size, read off its PNG header (no decode).
fn base_map_size(bundle: &MaterialBundle) -> Option<(u32, u32)> {
    let path = map_path(bundle, MapKind::BaseColor)?;
    match image::image_dimensions(&path) {
        Ok(size) => Some(size),
        Err(e) => {
            tracing::warn!(
                "material inspector: {} has no readable size: {e}",
                path.display()
            );
            None
        }
    }
}

/// The bundle's bytes on disk and its newest modification time — the maps and the recipe
/// as they sit AT REST (the recipe's `.gz` twin, found through [`FileFacts`]).
fn on_disk(bundle: &MaterialBundle) -> (u64, Option<SystemTime>) {
    let files = bundle
        .maps
        .iter()
        .map(|p| p.as_path())
        .chain(std::iter::once(bundle.recipe.as_path()));
    let mut bytes = 0;
    let mut newest = None;
    for facts in files.filter_map(FileFacts::of) {
        bytes += facts.size;
        let modified = std::fs::metadata(&facts.path)
            .and_then(|m| m.modified())
            .ok();
        newest = newest.max(modified);
    }
    (bytes, newest)
}

/// A render class's token — exhaustive, so a new class fails to compile here until the
/// stringtable names it.
fn class_token(class: Option<RenderClass>) -> &'static str {
    match class {
        Some(RenderClass::Blendable) => "$insp_class_blendable",
        Some(RenderClass::HardEdge) => "$insp_class_hard_edge",
        Some(RenderClass::Translucent) => "$insp_class_translucent",
        Some(RenderClass::Emissive) => "$insp_class_emissive",
        None => NONE,
    }
}

/// An index colour as `#RRGGBB` — the bytes the swatch shows.
fn hex(c: [f32; 3]) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", byte(c[0]), byte(c[1]), byte(c[2]))
}

/// A measured number as its shortest honest reading: at most two decimals, trailing zeros
/// dropped (`7`, `0.4`, `2.65`).
pub(crate) fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// A file time as `YYYY-MM-DD HH:MM` in UTC (the label says UTC), from the civil calendar
/// of the days since the epoch. `None` for a time before the epoch.
pub(crate) fn utc_stamp(t: SystemTime) -> Option<String> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Days → civil date (the proleptic Gregorian algorithm of H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60
    ))
}

// ── the graph view ───────────────────────────────────────────────────────────

/// A picked card, read out: its heading, what feeds it and its parameters — RAW text from
/// the graph's own data (its keys, port names, parameter names and values) beside the kind's
/// resolved name; every label around it is a `$token` the tree authors. A part with nothing
/// to say is the dash token.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReadOut {
    pub label: String,
    pub inputs: String,
    pub params: String,
}

impl ReadOut {
    /// Before anything is picked: the hint, and nothing to read beside it.
    fn hint() -> Self {
        Self {
            label: NODE_HINT.to_string(),
            inputs: NONE.to_string(),
            params: String::new(),
        }
    }

    fn publish(&self, m: &mut ValueMap) {
        m.set(NODE_LABEL, self.label.clone());
        m.set(NODE_INPUTS, self.inputs.clone());
        m.set(NODE_PARAMS, self.params.clone());
    }
}

/// The read-only graph: the loaded graph, its cards through the ONE projection
/// ([`GraphCards::project`]), the words on them, and the card the reader picked. No edit of
/// the document exists on this path — a pick re-projects, nothing else.
struct GraphView {
    graph: MaterialGraph,
    labels: CardLabels,
    cards: GraphCards,
    /// The picked card's key — a node's, or [`OUTPUT_CARD`].
    picked: Option<String>,
}

impl GraphView {
    fn new(graph: MaterialGraph) -> Self {
        let labels = CardLabels::stringtable();
        // Nothing reads as picked until the reader picks it.
        let cards = GraphCards::project(&graph, &labels, None, None);
        Self {
            graph,
            labels,
            cards,
            picked: None,
        }
    }

    /// Pick the card at index `card` — `None` (a press on empty canvas) picks nothing — and
    /// re-project, so the picked card and its wires wear the selection.
    fn pick(&mut self, card: Option<usize>) {
        self.picked = card.and_then(|i| self.cards.keys.get(i).cloned());
        self.cards = GraphCards::project(&self.graph, &self.labels, self.picked.as_deref(), None);
    }

    /// A card's heading as the list and the read-out show it: a node's `key · Kind` (its key
    /// is data, its kind's name the stringtable's), the Output card's own heading.
    fn heading(&self, card: Card) -> String {
        match card {
            Card::Node(i) => {
                let node = &self.graph.nodes[i];
                graph_view::node_heading(&node.key, &self.labels.kind_name(node.kind.id()))
            }
            Card::Output => self.labels.output_title.clone(),
        }
    }

    /// The node list's rows — one per card, in the canvas's order; a row's id is its card's
    /// key, its label the card's heading.
    fn rows(&self) -> Vec<Row> {
        self.cards
            .cards
            .iter()
            .zip(&self.cards.keys)
            .map(|(&card, key)| Row::new(key.clone(), self.heading(card)))
            .collect()
    }

    /// The picked card read out — a node: its heading, each of its ports with the key wired
    /// there (a dash where none is), its parameters; the Output: its heading, each WIRED port
    /// with its key, and the rest of its stage (relief, the constant levels, the glow's
    /// strength). The hint before anything is picked.
    fn read_out(&self) -> ReadOut {
        let picked = self
            .picked
            .as_deref()
            .and_then(|key| self.cards.keys.iter().position(|k| k == key))
            .map(|i| self.cards.cards[i]);
        let joined = |parts: Vec<String>| {
            if parts.is_empty() {
                NONE.to_string()
            } else {
                parts.join(" · ")
            }
        };
        match picked {
            Some(card @ Card::Node(i)) => {
                let node = &self.graph.nodes[i];
                let dash = strings::resolve(NONE);
                let inputs = node
                    .kind
                    .ports()
                    .iter()
                    .enumerate()
                    .map(|(p, port)| {
                        let src = node.inputs.get(p).and_then(Option::as_deref);
                        format!("{} {}", port.name, src.unwrap_or(&dash))
                    })
                    .collect();
                let params = pairs_of(&serde_json::to_value(&node.kind).ok(), &|_, _| false);
                ReadOut {
                    label: self.heading(card),
                    inputs: joined(inputs),
                    params: joined(params),
                }
            }
            Some(Card::Output) => {
                let out = &self.graph.output;
                let inputs = OUTPUT_PORTS
                    .iter()
                    .zip(out.sources())
                    .filter_map(|(port, src)| src.map(|key| format!("{} {key}", port.name)))
                    .collect();
                // A wired port reads out among the inputs; its parameters are the rest.
                let wired = |name: &str, v: &serde_json::Value| {
                    v.is_string() && OUTPUT_PORTS.iter().any(|p| p.name == name)
                };
                let params = pairs_of(&serde_json::to_value(out).ok(), &wired);
                ReadOut {
                    label: self.heading(Card::Output),
                    inputs: joined(inputs),
                    params: joined(params),
                }
            }
            None => ReadOut::hint(),
        }
    }
}

/// A serialized node (or output) as `name value` pairs: scalars as themselves, a list of
/// numbers as `[a, b, c]`, a list of anything else by its length, a nested object one level
/// down (`emit.strength 1`). The `kind` tag is the card's heading, not a parameter; `skip`
/// drops a pair by its (dotted) name and value.
fn pairs_of(
    value: &Option<serde_json::Value>,
    skip: &dyn Fn(&str, &serde_json::Value) -> bool,
) -> Vec<String> {
    fn scalar(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::Number(n) => n.as_f64().map(num),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Null => Some("-".to_string()),
            serde_json::Value::Array(items) => {
                let nums: Option<Vec<String>> = items.iter().map(|i| i.as_f64().map(num)).collect();
                Some(match nums {
                    Some(nums) => format!("[{}]", nums.join(", ")),
                    None => format!("×{}", items.len()),
                })
            }
            serde_json::Value::Object(_) => None,
        }
    }
    let mut out = Vec::new();
    let Some(serde_json::Value::Object(fields)) = value else {
        return out;
    };
    for (name, v) in fields {
        if name == "kind" {
            continue;
        }
        match (scalar(v), v) {
            (Some(text), _) if !skip(name, v) => out.push(format!("{name} {text}")),
            (None, serde_json::Value::Object(inner)) => {
                for (sub, w) in inner {
                    let path = format!("{name}.{sub}");
                    if skip(&path, w) {
                        continue;
                    }
                    let text = scalar(w).unwrap_or_else(|| "{…}".to_string());
                    out.push(format!("{path} {text}"));
                }
            }
            _ => {}
        }
    }
    out
}

// ── the scene ────────────────────────────────────────────────────────────────

/// The Material Inspector — an overlay scene hosting the shared `material_inspector` tree.
pub struct MaterialInspector {
    facts: InspectorFacts,
    /// The frame's Model: the facts, then the live read-outs (body, picked card, list).
    model: ValueMap,
    /// The shared tree, its exit written onto the root by [`hosted_modal_tree`], its node
    /// list expanded once per card by the walker's repeater.
    tree: UiNode,
    intents: UiIntents,
    /// The carrier's chrome ⊕ the tree's own blocks and stage, token-resolved.
    styles: serde_json::Value,
    ui_state: UiState,
    /// One-time default focus (the shared modals' rule): the lowest-ordinal control.
    nav_initialized: bool,
    hud_commands: Vec<HudCommand>,
    theme: Option<Theme>,
    /// The HUD's texture table: the theme's, then the index-colour swatch.
    textures: Vec<TextureHandle>,

    // ── the lit sample ──
    /// The tree's stage, compiled and checked once; `None` (warned) draws no sample.
    stage: Option<StageDef>,
    sample: MaterialSample,
    /// The sample's zoom range and wheel notch, read once off the tree's [`ZOOM`] slider
    /// ([`ZoomDial::OFF`] — the stage's framing only — when it will not read).
    zoom: ZoomDial,
    bodies: Option<SampleBodies>,
    /// The flat PBR the index row reads as — the stand-in until a bundle's maps land.
    flat: Option<FlatMaterial>,
    /// The flat stand-in's 1×1 textures — the swatch among them. Freed on exit.
    flats: FlatTextures,
    maps: Option<SlotMaps>,
    pending: Option<Decoded>,
    decoded_rx: Option<Receiver<Decoded>>,
    /// Set by `stop`: a decode still queued starts nothing; one running stops after
    /// the map in hand and delivers nothing.
    cancel: Arc<AtomicBool>,

    // ── the graph ──
    graph: Option<GraphView>,
    canvas: GraphCanvas,
    canvas_style: CanvasStyle,
    /// The zoom the graph is framed at on open (`params.canvas_fit_floor`).
    fit_floor: f32,
    fit_pending: bool,
}

impl MaterialInspector {
    /// The inspector on `facts` — GPU-free: the tree (its node list expanded), the styles, the
    /// stage, the Model and the graph view are set up here, and a bundle's maps are queued to
    /// decode on `pool` — the CATALOG's long-lived pool, borrowed: this scene owns no thread,
    /// so dropping it never joins one. The GPU half waits for [`Scene::enter`].
    pub fn new(facts: InspectorFacts, pool: &WorkerPool) -> Self {
        let doc: serde_json::Value = serde_json::from_str(INSPECTOR_JSON).unwrap_or_else(|e| {
            tracing::error!("{INSPECTOR_FILE} did not parse: {e}");
            serde_json::Value::Null
        });
        let hosted = hosted_modal_tree(INSPECTOR_JSON, INSPECTOR_ID);
        let own = own_blocks(&doc);
        let styles = flicker::ui::load_shared_styles(hosted_modal_styles(own.as_ref()).as_ref());
        let params = doc
            .get("params")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();

        // The stage, compiled and checked ONCE through the predicates every lit host here
        // shares: a broken stage is named once and refused — no sample, the well shows.
        let stage = flicker::ui::stage_def(&styles, INSPECTOR_STAGE).and_then(|stage| {
            let problems = sample_stage_check(&stage, INSPECTOR_STAGE);
            if problems.is_empty() {
                return Some(stage);
            }
            tracing::warn!(
                "stages.{INSPECTOR_STAGE} cannot draw the inspector's sample — its well shows \
                 until it is fixed: {}",
                problems.join("; ")
            );
            None
        });

        // A bundle's maps decode on a worker and land in `render`; the flat stand-in is worn
        // until then (the catalog's flow, through its own job). Measured 2026-10-21 at the
        // authored 512: a staged 256² glow bundle ~10 ms, a 2K bundle (the commit's baseline
        // rung) ~48 ms — three frames, and the 4K/8K rungs only grow — while the rest of
        // opening (the facts, this constructor) is ~8 ms. So the open never waits on pixels.
        let (decoded_rx, cancel) = Self::start_decode(&facts, &params, pool);

        let graph = facts.graph().cloned().map(GraphView::new);
        // The sample opens on the stage's own framing; its zoom moves inside the range and
        // by the notch the tree's own slider authors — the one source.
        let sample = MaterialSample::new(INSPECTOR_STAGE);
        let zoom = ZoomDial::authored(&hosted, ZOOM, INSPECTOR_FILE);
        let mut model = facts.model();
        model.set(BODY_LABEL, Body::Sphere.token());
        model.set(ZOOM, f64::from(sample.zoom));
        model.set(NODE_SCROLL, 0.0);
        graph
            .as_ref()
            .map_or_else(ReadOut::hint, GraphView::read_out)
            .publish(&mut model);
        // The node list: its ONE prototype expanded per card by the walker's own repeater,
        // once — a read-only graph never changes its cards — each row's label published into
        // the Model on its `label_bind` key. No graph, no rows.
        let rows = graph.as_ref().map_or_else(Vec::new, GraphView::rows);
        let tree = instantiate_rows(&hosted, &mut model, &|source| {
            (source == NODE_ROWS).then(|| rows.clone())
        });
        let intents = UiIntents::of(&tree);

        Self {
            flat: facts.row.as_ref().map(FlatMaterial::of),
            facts,
            model,
            tree,
            intents,
            canvas_style: graph_view::canvas_style(&styles, CANVAS_INK),
            styles,
            ui_state: UiState::new(),
            nav_initialized: false,
            hud_commands: Vec::new(),
            theme: None,
            textures: Vec::new(),
            stage,
            sample,
            zoom,
            bodies: None,
            flats: FlatTextures::new(),
            maps: None,
            pending: None,
            decoded_rx,
            cancel,
            graph,
            canvas: GraphCanvas::new(CanvasMetrics::default()),
            fit_floor: graph_view::fit_floor(&params, FIT_FLOOR_PARAM, INSPECTOR_FILE),
            fit_pending: true,
        }
    }

    /// Hand the inspector the theme its HUD draws with — the catalog's, built when it
    /// entered (the theme's textures are uploaded once and shared, never rebuilt per open).
    #[must_use]
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Queue a WHOLE bundle's maps on `pool` at the authored size. A bundle short of any map
    /// is not decoded (a sample is drawn in a whole material or not at all — the note says
    /// so), nor is one with no authored size (warned by [`decode_px`]).
    fn start_decode(
        facts: &InspectorFacts,
        params: &serde_json::Map<String, serde_json::Value>,
        pool: &WorkerPool,
    ) -> (Option<Receiver<Decoded>>, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let Some(bundle) = facts.bundle.as_ref().filter(|_| facts.whole()) else {
            return (None, cancel);
        };
        let Some(px) = decode_px(params, MAP_PX_PARAM, INSPECTOR_FILE) else {
            return (None, cancel);
        };
        let (tx, rx) = mpsc::channel();
        let job = MaterialBundle {
            slot: bundle.slot,
            dir: bundle.dir.clone(),
            recipe: bundle.recipe.clone(),
            maps: bundle.maps.clone(),
        };
        let flag = Arc::clone(&cancel);
        let slot = facts.slot;
        pool.submit(move || decode_job(slot, &job, px, &flag, &tx));
        (Some(rx), cancel)
    }

    // ── the GPU-free frame ────────────────────────────────────────────────────

    /// Take a bundle's decoded maps off the worker, if they have landed.
    pub(crate) fn take_decoded(&mut self) -> bool {
        if let Some(d) = self.decoded_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.pending = Some(d);
        }
        self.pending.is_some()
    }

    /// One walk of the tree on this host's own state: seat the sample, drive the canvas
    /// with the walker's own pointer sample for its seat, and keep the frame's HUD. GPU-free
    /// — `update` adds the pad's events and the answer; a gate drives exactly this.
    pub(crate) fn frame(&mut self, dt: Duration, input: &UiInput, middle: bool) -> UiFrame {
        self.take_decoded();
        // ONE-TIME default focus (the shared modals' MenuView rule): the controller opens
        // the inspector already on its first control, and a later click that moves the
        // focus sticks.
        if !self.nav_initialized {
            if let Some(first) = focusables_of(&self.tree, &self.model).into_iter().next() {
                if self.ui_state.focused().is_none() {
                    self.ui_state.request_focus(first.id);
                }
                self.nav_initialized = true;
            }
        }
        let mut frame = run_ui(
            &self.tree,
            &self.model,
            &self.styles,
            input,
            &mut self.ui_state,
        );
        // The node list's scroll offset rides its bind: the walker writes it (wheel,
        // clamped) and reads it back off the Model, so the host adopts what it echoes.
        if let Some(offset) = frame.results.number(NODE_SCROLL) {
            self.model.set(NODE_SCROLL, offset);
        }
        self.sample.seat(frame.surface(SAMPLE_SLOT));
        self.sample.tick(dt);
        self.zoom_sample(&frame);
        self.hud_commands = std::mem::take(&mut frame.commands);
        let slot = frame.surface(CANVAS_SLOT).cloned();
        let pointer = frame.surface_pointer(CANVAS_SLOT).cloned();
        self.drive_canvas(slot.as_ref(), pointer.as_ref(), middle, frame.popup_hit);
        frame
    }

    /// The sample's ZOOM, both ways in, then back out to the slider. The slider's value is an
    /// EDIT only when it is not the value the Model handed the slider this frame — an echo is
    /// never an edit (the MaterialBench dispatcher's rule, held against the Model the frame was
    /// drawn from: nothing writes this key between the walk and here) — and a value the sample
    /// declines (it clamps to where the zoom already is) hands the slider back to the Model.
    /// Then the wheel over the sample's seat, gated the one way every host gates a surface's
    /// pointer ([`graph_view::shape_pointer`]: over the seat, no open menu owning it). Both go
    /// through the engine's one pair of laws (`MaterialSample::set_zoom` / `zoom_by`) inside the
    /// range and notch the tree's slider authors; the Model carries the result to the slider.
    fn zoom_sample(&mut self, frame: &UiFrame) {
        if let Some(v) = frame.results.number(ZOOM) {
            if self.model.number(ZOOM) != Some(v)
                && !self.sample.set_zoom(v as f32, self.zoom.range())
            {
                self.ui_state.release_local(ZOOM);
            }
        }
        if let Some(slot) = frame.surface(SAMPLE_SLOT) {
            let pointer = frame.surface_pointer(SAMPLE_SLOT);
            let (shaped, _) = graph_view::shape_pointer(
                pointer,
                graph_view::seat_of(slot),
                false,
                frame.popup_hit,
            );
            if shaped.inside && shaped.wheel != 0.0 {
                self.sample
                    .zoom_by(shaped.wheel, self.zoom.step, self.zoom.range());
            }
        }
        self.model.set(ZOOM, f64::from(self.sample.zoom));
    }

    /// Read one frame's fired names and decide — the inspector's whole answer ladder.
    /// The host's exit ([`MODAL_CANCEL`] — Esc, pad-B and the Close button alike) closes
    /// with [`modal_cancelled`]; the body toggle swaps Sphere and Plane; a node row picks
    /// its card exactly as a click on the card does, and brings it into view.
    pub(crate) fn answer(&mut self, results: &ValueMap) -> Transition {
        if results.is_on(MODAL_CANCEL) {
            return modal_cancelled(INSPECTOR_ID);
        }
        if results.is_on(BODY_ACTION) {
            self.sample.body = self.sample.body.toggled();
            self.model.set(BODY_LABEL, self.sample.body.token());
        }
        if let Some(view) = self.graph.as_mut() {
            let row = (0..view.cards.keys.len()).find(|&row| results.is_on(&row_action(row)));
            if let Some(row) = row {
                view.pick(Some(row));
                self.canvas.reveal(&view.cards.keys[row]);
                view.read_out().publish(&mut self.model);
            }
        }
        Transition::None
    }

    /// Seat, frame, lay out and drive the read-only canvas, and draw it over the walker's
    /// commands inside a scissor on its seat (a panned card must not paint over the panels
    /// around it). The pointer is the walker's to give, shaped the one way every graph host
    /// shapes it ([`graph_view::shape_pointer`]). Unseated, or with no graph, it abandons
    /// any gesture in flight and draws nothing (the well and the note show).
    fn drive_canvas(
        &mut self,
        slot: Option<&SurfaceSlot>,
        pointer: Option<&SurfacePointer>,
        middle: bool,
        popup_hit: bool,
    ) {
        let (Some(slot), Some(view)) = (slot, self.graph.as_mut()) else {
            self.canvas.cancel();
            return;
        };
        let seat = graph_view::seat_of(slot);
        self.canvas.seat(seat);
        self.canvas.set_mode(CanvasMode::Inspect);
        let keys = view.cards.key_refs();
        // Pinned, every card holds a placement — so a row picked in the list can bring its
        // card into view (`reveal` pans to a placed card).
        self.canvas.pin(&keys);
        if self.fit_pending {
            // Legibly, framed on the Output card when the whole graph would only fit
            // smaller than the authored floor.
            self.canvas.fit_legible(&keys, self.fit_floor, OUTPUT_CARD);
            self.fit_pending = false;
        }
        self.canvas
            .layout_with_ports(&keys, &view.cards.ports, &view.cards.edges);
        let (sample, _) = graph_view::shape_pointer(pointer, seat, middle, popup_hit);
        let events = self.canvas.pointer(&sample, &keys);
        drop(keys);
        if let Some(press) = events.pressed {
            view.pick(press.card);
            view.read_out().publish(&mut self.model);
        }
        let mut out = Vec::new();
        let canvas = &self.canvas;
        let style = &self.canvas_style;
        graph_view::with_nodes(&view.cards, |nodes| {
            canvas.draw(nodes, &view.cards.edges, style, slot.layer, &mut out);
        });
        self.hud_commands.push(HudCommand::Clip {
            rect: Some([slot.x, slot.y, slot.w, slot.h]),
        });
        self.hud_commands.append(&mut out);
        self.hud_commands.push(HudCommand::Clip { rect: None });
    }

    /// Declare the sample: upload a bundle's maps when they land (BEFORE the old set is
    /// released — the renderer reuses a freed slot, and new maps carrying the old handles
    /// would never dirty the sample), then one pass under the tree's stage, live.
    fn draw_sample(&mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'_>) {
        let Some(stage) = self.stage.as_ref() else {
            // No stage, no sample (named once at open): nothing will wear these.
            self.pending = None;
            return;
        };
        if let Some(d) = self.pending.take() {
            let maps = upload_decoded(renderer, &d.maps, stage.emissive_gain);
            if let Some(SlotMaps::Bundle(old)) = self.maps.replace(SlotMaps::Bundle(maps)) {
                old.release(renderer);
            }
        }
        let Some(maps) = self.maps.as_ref() else {
            // A free slot whose bundle has not landed (or has none): nothing to wear yet.
            return;
        };
        let bodies = self
            .bodies
            .get_or_insert_with(|| SampleBodies::new(renderer));
        let base = fg.base_layer();
        self.sample
            .render(renderer, fg, base, bodies, maps.sample_maps(), stage);
    }

    /// Stop everything that would outlive this modal, GPU-free — the half of teardown a
    /// gate can time: the decode is told nobody will wear its maps (a queued job starts
    /// nothing; a running one stops after the map in hand and delivers nothing), anything
    /// it already delivered is dropped, and the canvas abandons any gesture in flight.
    /// Nothing here waits: the pool is the catalog's, and nothing is joined.
    pub(crate) fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.decoded_rx = None;
        self.pending = None;
        self.canvas.cancel();
    }

    // ── read-only accessors, for gates ───────────────────────────────────────

    pub fn facts(&self) -> &InspectorFacts {
        &self.facts
    }

    /// The frame's Model — what the tree binds.
    pub fn model(&self) -> &ValueMap {
        &self.model
    }

    /// The tree as hosted — the exit written onto its root, the node list expanded.
    pub fn tree(&self) -> &UiNode {
        &self.tree
    }

    #[cfg(test)]
    pub(crate) fn styles(&self) -> &serde_json::Value {
        &self.styles
    }

    #[cfg(test)]
    pub(crate) fn stage(&self) -> Option<&StageDef> {
        self.stage.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn ui_state(&mut self) -> &mut UiState {
        &mut self.ui_state
    }

    #[cfg(test)]
    pub(crate) fn intents(&self) -> &UiIntents {
        &self.intents
    }

    #[cfg(test)]
    pub(crate) fn canvas_cards(&self) -> &[flicker::render::Rect] {
        self.canvas.cards()
    }

    #[cfg(test)]
    pub(crate) fn canvas_keys(&self) -> Vec<String> {
        self.graph
            .as_ref()
            .map(|g| g.cards.keys.clone())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn picked_selected(&self) -> Vec<bool> {
        self.graph
            .as_ref()
            .map(|g| g.cards.selected.clone())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn body(&self) -> Body {
        self.sample.body
    }

    #[cfg(test)]
    pub(crate) fn hud(&self) -> &[HudCommand] {
        &self.hud_commands
    }

    #[cfg(test)]
    pub(crate) fn fit_floor(&self) -> f32 {
        self.fit_floor
    }

    #[cfg(test)]
    pub(crate) fn decoding(&self) -> bool {
        self.decoded_rx.is_some()
    }
}

impl Scene for MaterialInspector {
    fn is_overlay(&self) -> bool {
        true
    }

    /// Menu context, exactly as `SharedModal`: the pump resolves Esc / pad-B to `Cancel`
    /// (the root's injected exit) and the d-pad / Confirm for the controls.
    fn input_context(&self) -> Option<InputContext> {
        Some(InputContext::Menu)
    }

    /// The GPU half of opening: the HUD's texture table (the theme's, then the swatch) and
    /// the flat stand-in the sample wears until a bundle's maps land. The swatch IS the flat
    /// stand-in's base colour texel — the 1×1 index colour — so one upload serves both.
    fn enter(&mut self, renderer: &mut Renderer) {
        let Some(theme) = self.theme else {
            tracing::error!(
                "the material inspector opened without the catalog's theme — it cannot draw \
                 its HUD (Esc / pad-B still closes it)"
            );
            return;
        };
        self.textures = theme.lua_textures().iter().map(|(_, h)| *h).collect();
        if let Some(flat) = self.flat {
            let gain = self.stage.as_ref().map_or(1.0, |s| s.emissive_gain);
            let maps = self.flats.maps_for(renderer, &flat, gain);
            // At `Theme::TEXTURE_NAMES.len()` — the index `insp_swatch_tex` publishes.
            self.textures.push(maps.albedo);
            self.maps = Some(SlotMaps::Flat(maps));
        }
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        let snap = UiInput::from_input(input, renderer.size());
        let frame = self.frame(dt, &snap, input.mouse_middle);
        let mut results = frame.results;
        // The input seam (input-P3): the PUMP resolved this frame's Menu-context events;
        // one dispatch through the walker, which moves the focus, activates the focused
        // control on Confirm and fires the root's exit on Cancel (when the slab allows it —
        // it always does here).
        let hud_hit = results.is_on("hud_hit");
        let mut walker = WalkerHandler::hud(&mut self.ui_state, hud_hit)
            .with_nav(&self.tree, &self.model)
            .with_rects(&frame.rects)
            .with_intents(&self.intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
        }
        self.answer(&results)
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        self.draw_sample(renderer, fg);
        if let Some(&white) = self.textures.first() {
            let hud_commands = &self.hud_commands;
            let textures = &self.textures;
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
        }
    }

    /// Give back everything this modal holds, exactly once — the renderer owns the GPU
    /// side and the scene only handles, so teardown reclaims nothing on its own (rule
    /// 728E682F): first `stop` (the decode, the canvas's gesture — nothing waits),
    /// then the sample's target, a bundle's uploaded maps, the flat textures (the swatch
    /// among them, freed with the cache) and the bodies' meshes.
    fn exit(&mut self, renderer: &mut Renderer) {
        // First, so a decode still queued starts nothing while the GPU side is given back.
        self.stop();
        self.sample.release(renderer);
        if let Some(SlotMaps::Bundle(maps)) = self.maps.take() {
            maps.release(renderer);
        }
        std::mem::take(&mut self.flats).release(renderer);
        if let Some(bodies) = self.bodies.take() {
            bodies.release(renderer);
        }
        // The swatch's handle went with the flat cache.
        self.textures.truncate(Theme::TEXTURE_NAMES.len());
    }
}

/// The tree file's OWN blocks — its `styles`, and its `stages` folded under `stages` the
/// way the scene parser folds a scene's (the loader merges them INTO the stage library,
/// where the lighting presets live).
fn own_blocks(doc: &serde_json::Value) -> Option<serde_json::Value> {
    let mut blocks = doc
        .get("styles")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(stages) = doc.get("stages").and_then(serde_json::Value::as_object) {
        blocks.insert(
            "stages".to_string(),
            serde_json::Value::Object(stages.clone()),
        );
    }
    (!blocks.is_empty()).then_some(serde_json::Value::Object(blocks))
}

#[cfg(test)]
mod tests;
