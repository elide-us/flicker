//! **Material Catalog** — every slot of the material byte, on one screen.
//!
//! The Component Catalog's sibling for materials: where that bench shows one live copy of
//! every Rust component, this one shows all 256 slots of the voxel material byte
//! ([`MaterialId`], a `u8`) as sixteen rows of sixteen boxes, row-major, so a slot's id is
//! its row head plus its column head. It is a VIEWING tool — the registry of what the
//! engine holds per slot. Making and binding materials is the MaterialBench's job and
//! stays there; nothing here edits anything.
//!
//! What a box shows, per slot:
//! - **assigned** (a `materials.json` row): the material as a LIT SPHERE — the engine's
//!   material sample ([`MaterialSample`]) seated in the slot's authored `surface` and drawn
//!   under the scene's own [`SAMPLE_STAGE_SOURCE`] stage (HDR, bloom, tonemap), wearing the
//!   flat PBR its index row reads as ([`FlatMaterial::of`]: the colour the mesh pass draws a
//!   voxel of that id with, its family's finish, and its own colour as Emit for an
//!   emissive-class row, at the stage's authored gain) — with the material's name under it;
//! - **generated** (a bundle promoted to `package/materials/<id:03>/`, the P3 DEFINED
//!   predicate [`flicker_content::bundle_at`]): the same sphere wearing the bundle's maps,
//!   decoded on the worker pool and uploaded when they land (the flat stand-in until then);
//! - **free**: no sample — the authored well, nothing under it;
//! - **emissive** (the top eight slots, [`EMISSIVE_FIRST`]`..=255`, held FOR emissive-class
//!   materials — Aaron 2026-10-20): an emissive row there is ASSIGNED like any other (the eight
//!   curated glows: their spheres burn in their palette colour at the stage's gain); an emissive
//!   slot that holds no row yet shows the well with "Emissive" under it.
//!
//! # Posters
//!
//! Every box is authored `rate: poster`: a sphere renders once and keeps its image, and
//! re-renders only when the sample's dirty channel fires — its bundle's maps landing as NEW
//! textures (the catalog never rewrites a texture in place, so it never needs the sample's
//! `mark_dirty`). A screen of spheres costs one pass per sphere ONCE, then one composite per
//! sphere per frame.
//!
//! # The five-line split
//!
//! `materialcatalog.scene.json` AUTHORS the whole tree — the heads and all 256 cells, each a
//! `surface` on the scene's `mc_sample` stage whose panel is the well, and a `text` bound to
//! that slot's name — this scene's style blocks, that stage (its look and its emissive gain),
//! and the size a bundle's maps decode to (`params.sample_map_px`). `materialcatalog.lua`
//! derives the labels of the emissive block from the two numbers the behaviour publishes.
//! This crate is DRAWING CODE: it seats one material sample per assigned slot in that slot's
//! authored surface (by node id), feeds it its maps, publishes the slot names and the tally as
//! data, and routes the declared intents (Menu, and each box's `mc_open_<id>`). No node is
//! composed here and no style is written here.
//!
//! # The inspector
//!
//! Every box is a pressable `panel` (a click, or a pad Confirm on the box under the cursor)
//! firing `mc_open_<id>`; the dispatcher pushes the [`MaterialInspector`] on that slot — the
//! SHARED modal tree `scenes/shared/material_inspector.scene.json`, hosted by a scene of its
//! own ([`inspector`]) because what it shows is live while this scene is frozen beneath it.
//! Its answer is only ever "closed": the posters here keep their images, so `modal_closed`
//! has nothing to do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use flicker::render::{FrameGraph, Renderer, StageDef, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, SceneDef, SurfaceSlot, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_content::MaterialBundle;
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_materials::{
    slot_accepts, JsonTableSource, MaterialBand, MaterialDef, MaterialId, RenderClass, Tables,
    EMISSIVE_FIRST,
};
use flicker_materialview::{
    decode_bundle_maps_with, sample_stage_problems, upload_decoded, DecodedMaps, FlatMaterial,
    FlatTextures, MaterialSample, OwnedMaps, SampleBodies, SampleMaps,
};
use flicker_shell::{PauseScene, Theme};
use flicker_worker::WorkerPool;

pub mod inspector;
pub use inspector::{Held, InspectorFacts, MaterialInspector, Recipe, INSPECTOR_ID};

/// The pair script — the scene's LOGIC half, by name (five-line architecture).
const MC_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/materialcatalog.lua");
/// The shipped scene file — the tests' copy of the authored tree (the runtime receives
/// the same file through the manifest `SceneDef`).
#[cfg(test)]
const MC_SCENE: &str =
    include_str!("../../../../content/sensorium/scenes/materialcatalog.scene.json");

/// How many slots the material byte has — the cell count.
pub const SLOTS: usize = MaterialId::MAX as usize + 1;
/// Boxes per row: the byte's low nibble.
pub const GRID_COLS: usize = 16;
/// The slots held for emissive materials (`EMISSIVE_FIRST..=255`).
pub const EMISSIVE_SLOTS: usize = SLOTS - EMISSIVE_FIRST as usize;
/// The stage every box's sphere is drawn under (`stages.<source>` in the scene file) — its
/// look, its HDR resolve, its bloom and its emissive gain are DATA there, not constants here.
pub const SAMPLE_STAGE_SOURCE: &str = "mc_sample";
/// The scene param naming the edge, in texels, a bundle's maps decode to.
pub const MAP_PX_PARAM: &str = "sample_map_px";

/// The authored ids and binds, spelled once: the scene file names them, the gates pin
/// the file to these.
pub fn cell_id(slot: MaterialId) -> String {
    format!("mc_slot_{slot}")
}
pub fn box_id(slot: MaterialId) -> String {
    format!("mc_box_{slot}")
}
pub fn name_key(slot: MaterialId) -> String {
    format!("mc_name_{slot}")
}
/// The action a slot's box fires — the one name a click and a pad Confirm both produce, the
/// dispatcher's cue to open the inspector on that slot.
pub fn open_action(slot: MaterialId) -> String {
    format!("{OPEN_PREFIX}{slot}")
}
const OPEN_PREFIX: &str = "mc_open_";

/// The slot whose box fired `mc_open_<id>` in `results`, if one did — the lowest, should a
/// frame ever carry two. An action that names no slot of the byte opens nothing.
pub fn opened_slot(results: &ValueMap) -> Option<MaterialId> {
    results
        .entries()
        .filter(|(name, _)| results.is_on(name))
        .filter_map(|(name, _)| name.strip_prefix(OPEN_PREFIX)?.parse::<MaterialId>().ok())
        .min()
}

/// Whether `slot` is held FOR emissive materials — it takes an emissive row and refuses every
/// other class. Asked of the ONE slot predicate ([`slot_accepts`]), so the catalog spells no
/// boundary of its own.
pub fn held_for_emissive(slot: MaterialId) -> bool {
    slot_accepts(slot, Some(RenderClass::Emissive))
        && [
            RenderClass::Blendable,
            RenderClass::HardEdge,
            RenderClass::Translucent,
        ]
        .into_iter()
        .all(|class| !slot_accepts(slot, Some(class)))
}

/// The slots `package_root` holds a bundle for — the P3 DEFINED predicate looped over the
/// whole byte.
pub fn generated_slots(package_root: &Path) -> Vec<MaterialId> {
    (0..=MaterialId::MAX)
        .filter(|&id| flicker_content::bundle_at(package_root, id).is_some())
        .collect()
}

/// The edge, in texels, a promoted bundle's maps are decoded to — the scene's
/// `params.sample_map_px`, Aaron's to tune (never a constant here). `None`, with a warn, when
/// it is unauthored or not a whole number above zero: the bundles then wear their flat
/// stand-in — visibly not their maps, and said so.
pub fn sample_map_px(params: &serde_json::Map<String, serde_json::Value>) -> Option<u32> {
    decode_px(params, MAP_PX_PARAM, "materialcatalog.scene.json")
}

/// THE decode-size reader every sample host here shares: `params.<key>` of the authored file
/// `file`, a whole number above zero, else `None` with a warn naming the file and the key —
/// the bundle then wears its flat stand-in, visibly not its maps, and said so.
pub(crate) fn decode_px(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    file: &str,
) -> Option<u32> {
    let authored = params.get(key);
    let px = authored
        .and_then(|v| {
            v.as_u64().or_else(|| {
                v.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
        })
        .filter(|&n| n > 0)
        .and_then(|n| u32::try_from(n).ok());
    if px.is_none() {
        tracing::warn!(
            "{file} `params.{key}` is {authored:?}, not a whole number above zero — a promoted \
             bundle wears its flat stand-in instead of its maps"
        );
    }
    px
}

/// One generated slot's maps, decoded off the worker — `render` uploads them (a `Renderer`
/// exists there and not before).
pub(crate) struct Decoded {
    pub slot: MaterialId,
    pub maps: DecodedMaps,
}

/// One bundle's decode, as a worker runs it — the catalog's spheres and the inspector's
/// sample alike, each with its own scene's `cancel` flag. The flag is read BEFORE the
/// decode — a job still queued when its scene left starts nothing — BETWEEN the maps — a
/// running decode stops after the map in hand ([`decode_bundle_maps_with`]) — and again
/// before the send — a decode that finished after its scene left is dropped, not delivered.
/// So leaving never waits on maps nobody will see: a worker is free again after at most one
/// map's decode, and a pool's join waits no longer than that.
pub(crate) fn decode_job(
    slot: MaterialId,
    bundle: &MaterialBundle,
    px: u32,
    cancel: &AtomicBool,
    tx: &Sender<Decoded>,
) {
    if cancel.load(Ordering::Acquire) {
        return;
    }
    // `None` is a cancel, or was warned by the decode (a missing or unreadable map): the
    // slot keeps its flat stand-in — never drawn half-dressed.
    let cancelled = || cancel.load(Ordering::Acquire);
    let Some(maps) = decode_bundle_maps_with(bundle, px, &cancelled) else {
        return;
    };
    if cancel.load(Ordering::Acquire) {
        return;
    }
    // A closed receiver just means the catalog went away mid-decode.
    let _ = tx.send(Decoded { slot, maps });
}

/// Everything wrong with a compiled lit-sample stage, each naming `stages.<source>`: the
/// material sample's own checks ([`sample_stage_problems`] — the predicate every sample
/// backstops with) and the HDR resolve a glow needs ([`flicker::ui::hdr_resolve_problems`] —
/// the predicate the shipped-stage gate holds every lit stage to). Empty when the stage draws
/// as authored. The ONE check both hosts here run — the catalog against `mc_sample` (ONCE, in
/// `build`, so a broken stage is named once rather than once per sphere) and the inspector
/// against its own `inspector_sample`.
pub(crate) fn sample_stage_check(stage: &StageDef, source: &str) -> Vec<String> {
    let mut problems = sample_stage_problems(stage, source);
    match flicker::ui::hdr_resolve_problems(stage, source) {
        Some(breaches) => problems.extend(breaches),
        None => problems.push(format!(
            "stages.{source}: no rgba16f `hdr` attachment is declared — an Emit at the \
             authored gain cannot exceed white, and its bloom has no HDR to spread"
        )),
    }
    problems
}

/// What one sphere wears.
pub(crate) enum SlotMaps {
    /// The flat PBR read off the index row — 1×1 textures BORROWED from the scene's shared
    /// [`FlatTextures`] cache, which frees them.
    Flat(SampleMaps),
    /// A promoted bundle's maps, uploaded here and OWNED here: released on a swap or on exit.
    Bundle(OwnedMaps),
}

impl SlotMaps {
    pub(crate) fn sample_maps(&self) -> &SampleMaps {
        match self {
            SlotMaps::Flat(maps) => maps,
            SlotMaps::Bundle(maps) => maps,
        }
    }
}

/// One ASSIGNED slot's lit sphere: the engine's material sample, the box it is seated in,
/// and what it wears.
pub(crate) struct SlotSample {
    pub slot: MaterialId,
    /// The authored `surface` this sample is seated in — [`box_id`], spelled once.
    pub node: String,
    /// The flat PBR the index row reads as: what the sphere wears until (or unless) its
    /// bundle's maps land.
    pub flat: FlatMaterial,
    sample: MaterialSample,
    /// `None` until the first render makes the flat maps (a `Renderer` exists there).
    maps: Option<SlotMaps>,
}

/// The catalog scene. Everything a frame needs lives here; the shell drives it through
/// the [`Scene`] trait.
pub struct MaterialCatalog {
    /// The AUTHORED tree off the manifest's def, walked every frame.
    tree: Option<UiNode>,
    /// The screen's declared signal→result intents, read off the tree ONCE.
    ui_intents: UiIntents,
    /// Token-resolved styles: the theme + this scene's own blocks and stages.
    ui_styles: serde_json::Value,
    hud_commands: Vec<HudCommand>,
    ui_state: UiState,
    /// The pair script. `None` only if it failed to load — the emissive block then
    /// shows no label (visibly wrong, not silently).
    script: Option<ScriptHost>,
    theme: Option<Theme>,
    /// The HUD's texture table — the theme's (white, the Muse, the pad-glyph atlas the
    /// authored `pad_glyphs.tex` indexes). Built in `enter`.
    textures: Vec<TextureHandle>,

    /// The material index's rows, sorted by id. Empty if the tables could not be read —
    /// the catalog then shows 256 free slots, visibly wrong rather than silently wrong.
    materials: Vec<MaterialDef>,
    /// The slots the PACKAGE holds a bundle for, measured once at construction.
    generated: Vec<MaterialId>,
    /// The authoring bands of the byte (`materials.json` `_meta.bands`), read once — the
    /// inspector names the band a slot is filed under. Empty (warned) if unreadable.
    bands: Vec<MaterialBand>,
    /// The two roots of the promotion path, resolved once: what the inspector probes for a
    /// slot's bundle (the package first, then staging).
    package: PathBuf,
    staging: PathBuf,
    /// The authored sample stage, compiled and checked ONCE at construction. `None` — not
    /// authored (warned by the compiler) or refused by [`sample_stage_check`] (warned once,
    /// naming every breach) — draws no sphere at all: every box shows its well.
    stage: Option<StageDef>,
    /// One lit sample per ASSIGNED slot, in id order — an emissive slot's glow included. A
    /// free slot (or an emissive slot that holds no row yet) has none.
    samples: Vec<SlotSample>,
    /// The sphere (and plane) every sample is drawn with — built on the first render that
    /// draws one, given back on exit.
    bodies: Option<SampleBodies>,
    /// The flat PBR textures the flat samples share, made once each, freed on exit.
    flats: FlatTextures,
    /// Whether this visit's first walk has been checked for assigned slots with no box.
    seats_checked: bool,
    /// Decoded bundle maps waiting for a `Renderer` to upload them.
    pending: Vec<Decoded>,
    decoded_rx: Receiver<Decoded>,
    /// Set on exit: every decode job still queued starts nothing, one running stops after
    /// the map in hand, and none delivers (`decode_job`).
    cancel: Arc<AtomicBool>,
    /// The ONE long-lived decode pool: the spheres' jobs, and every inspector this scene
    /// opens borrows it for its own — so closing an inspector never joins a thread.
    pool: WorkerPool,
}

impl MaterialCatalog {
    /// The runtime constructor — the manifest hands in the authored `SceneDef`; the
    /// content roots come from the roots service (the scene never spells a path).
    pub fn new(def: &SceneDef) -> Self {
        let roots = flicker_content::roots();
        Self::build(def, roots.data(), roots.package(), roots.staging())
    }

    /// A catalog on the shipped scene file whose promotion path is read from `package` and
    /// `staging` — the seam a test drives against scratch trees.
    #[cfg(test)]
    pub(crate) fn shipped_with_roots(package: PathBuf, staging: PathBuf) -> Self {
        let def = SceneDef::parse("materialcatalog", MC_SCENE)
            .expect("the shipped materialcatalog.scene.json parses");
        Self::build(&def, flicker_content::roots().data(), package, staging)
    }

    fn build(def: &SceneDef, data_dir: PathBuf, package: PathBuf, staging: PathBuf) -> Self {
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let tree = def.tree.clone();
        if tree.is_none() {
            tracing::error!("scene '{}' declares no `tree` — no UI", def.id);
        }
        let ui_intents = tree.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(MC_SCRIPT, "materialcatalog.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("materialcatalog.lua failed to load — raw model values only: {e}");
                None
            }
        };

        let source = JsonTableSource::new(&data_dir);
        let bands = source.load_material_bands().unwrap_or_else(|e| {
            tracing::warn!("material bands unreadable at {}: {e}", data_dir.display());
            Vec::new()
        });
        let materials = match Tables::from_source(&source) {
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
        // The sample stage is compiled once here, not re-parsed every frame; an absent one
        // is warned by the compiler and draws no sphere. A compiled one is CHECKED once here
        // too: a broken stage is named once and refused — no sphere is drawn on it, so the
        // samples' own per-instance checks (the backstop) never repeat it once per sphere.
        let stage = flicker::ui::stage_def(&ui_styles, SAMPLE_STAGE_SOURCE).and_then(|stage| {
            let problems = sample_stage_check(&stage, SAMPLE_STAGE_SOURCE);
            if problems.is_empty() {
                return Some(stage);
            }
            tracing::warn!(
                "stages.{SAMPLE_STAGE_SOURCE} cannot draw the catalog's spheres — every box shows \
                 its well until it is fixed: {}",
                problems.join("; ")
            );
            None
        });
        let map_px = sample_map_px(&def.params);
        // One sample per ASSIGNED slot, wearing its row's flat PBR until its maps say
        // otherwise — an emissive row glowing in its own colour. A free slot (or an emissive
        // slot that holds no row yet) gets none: its box is the authored well.
        let samples: Vec<SlotSample> = materials
            .iter()
            .map(|m| SlotSample {
                slot: m.id,
                node: box_id(m.id),
                flat: FlatMaterial::of(m),
                sample: MaterialSample::new(SAMPLE_STAGE_SOURCE),
                maps: None,
            })
            .collect();

        // Which slots the PACKAGE defines (plan 30FE7F58 P3: a slot is DEFINED iff its
        // bundle is in package/). Measured once; a bundle promoted while the catalog is
        // open shows on the next visit, like every other package read.
        let generated = generated_slots(&package);

        // Decode each generated slot's maps on the worker — seven 2K PNGs are tens of
        // milliseconds each, which is not the frame thread's to pay. Results land on
        // `decoded_rx`; `render` uploads them as they arrive and the sphere re-renders.
        let (tx, decoded_rx) = mpsc::channel();
        let pool = WorkerPool::with_default_size();
        let cancel = Arc::new(AtomicBool::new(false));
        for &slot in &generated {
            if !samples.iter().any(|s| s.slot == slot) {
                tracing::warn!(
                    "material slot {slot} has a bundle in the package but no index row — the \
                     catalog draws no sphere for it"
                );
                continue;
            }
            let Some(px) = map_px else { continue };
            let Some(bundle) = flicker_content::bundle_at(&package, slot) else {
                continue;
            };
            let (tx, cancel) = (tx.clone(), Arc::clone(&cancel));
            pool.submit(move || decode_job(slot, &bundle, px, &cancel, &tx));
        }

        Self {
            tree,
            ui_intents,
            ui_styles,
            hud_commands: Vec::new(),
            ui_state: UiState::default(),
            script,
            theme: None,
            textures: Vec::new(),
            materials,
            generated,
            bands,
            package,
            staging,
            stage,
            samples,
            bodies: None,
            flats: FlatTextures::new(),
            seats_checked: false,
            pending: Vec::new(),
            decoded_rx,
            cancel,
            pool,
        }
    }

    /// The RAW runtime variables the pair script and the tree bind: each assigned slot's
    /// name (data, never a token), the two numbers the script derives the emissive block
    /// from, and the tally pre-formatted as digits (the tree never does arithmetic). The
    /// tally partitions the byte: ASSIGNED (every row — the emissive slots' glows among
    /// them), EMISSIVE OPEN (emissive slots that hold no row yet — [`held_for_emissive`],
    /// the one slot predicate), and FREE (the rest).
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::default();
        for def in &self.materials {
            m.set(name_key(def.id), def.name.clone());
        }
        m.set("mc_slot_count", SLOTS as f64);
        m.set("mc_emissive_first", EMISSIVE_FIRST as f64);
        let assigned = self.materials.len();
        let open = (0..=MaterialId::MAX)
            .filter(|&slot| held_for_emissive(slot))
            .filter(|&slot| self.materials.iter().all(|def| def.id != slot))
            .count();
        m.set("mc_assigned", assigned.to_string());
        m.set("mc_generated", self.generated.len().to_string());
        m.set("mc_emissive", open.to_string());
        m.set("mc_free", SLOTS.saturating_sub(assigned + open).to_string());
        m
    }

    /// The frame's full model: the raw variables plus the pair script's derived values.
    pub(crate) fn model(&self) -> ValueMap {
        let raw = self.hud_model();
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("materialcatalog: publishing the model to the script failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => m.extend(derived),
                Ok(None) => {}
                Err(e) => tracing::error!("materialcatalog: derive() failed: {e}"),
            }
        }
        m
    }

    /// Drain the worker's decoded bundles into the upload queue. Pure (no GPU), so a test
    /// can watch them arrive.
    pub(crate) fn take_decoded(&mut self) -> usize {
        while let Ok(d) = self.decoded_rx.try_recv() {
            self.pending.push(d);
        }
        self.pending.len()
    }

    /// Seat every sample in the surface its box reserved this frame, found by node id.
    /// Returns the assigned slots whose box reserved none, and names them in a warn the first
    /// time — an assigned material the catalog cannot show is an authoring hole, never a
    /// silent empty box.
    pub(crate) fn seat_samples(&mut self, surfaces: &[SurfaceSlot]) -> Vec<MaterialId> {
        let by_id: HashMap<&str, &SurfaceSlot> =
            surfaces.iter().map(|s| (s.id.as_str(), s)).collect();
        let mut unseated = Vec::new();
        for s in &mut self.samples {
            let seat = by_id.get(s.node.as_str()).copied();
            if seat.is_none() {
                unseated.push(s.slot);
            }
            s.sample.seat(seat);
        }
        if !self.seats_checked {
            self.seats_checked = true;
            for &slot in &unseated {
                tracing::warn!(
                    "material slot {slot} is assigned but no `surface` `{}` reserves its box — \
                     the catalog cannot draw its sphere",
                    box_id(slot)
                );
            }
        }
        unseated
    }

    /// Tell every decode job still queued, or still running, that nobody will wear its
    /// maps: queued ones start nothing, a running one delivers nothing (`decode_job`).
    pub(crate) fn cancel_decodes(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Declare every seated sphere: upload what the worker delivered (a bundle's maps
    /// replace the stand-in, and the old upload is given back), make the flat maps a sample
    /// has not worn yet, then one pass per sample under the authored stage. A poster's pass
    /// runs only when its dirty channel says the image changed; its composite runs every
    /// frame.
    fn draw_samples(&mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'_>) {
        let Some(stage) = self.stage.as_ref() else {
            // No stage, no sphere (the compiler said so): nothing will ever wear these.
            self.pending.clear();
            return;
        };
        if self.samples.is_empty() {
            return;
        }
        // The stage's authored draw-time gain on a material's Emit — the glow's brightness
        // in THIS stage's HDR space, handed to every map set the spheres wear.
        let gain = stage.emissive_gain;
        for d in self.pending.drain(..) {
            let Some(s) = self.samples.iter_mut().find(|s| s.slot == d.slot) else {
                continue;
            };
            // Upload BEFORE releasing: the renderer reuses a freed slot, so maps uploaded
            // after the old ones were freed could carry the very handles the sample last drew
            // with — its dirty channel would see nothing change, and the poster would keep
            // the old image.
            let maps = upload_decoded(renderer, &d.maps, gain);
            if let Some(SlotMaps::Bundle(old)) = s.maps.replace(SlotMaps::Bundle(maps)) {
                old.release(renderer);
            }
        }
        // Built on the first render that draws a sphere: a `Renderer` exists here and not
        // before, and building on first use leaves no init step to forget.
        let bodies = self
            .bodies
            .get_or_insert_with(|| SampleBodies::new(renderer));
        let base = fg.base_layer();
        let flats = &mut self.flats;
        for s in &mut self.samples {
            let maps = s
                .maps
                .get_or_insert_with(|| SlotMaps::Flat(flats.maps_for(renderer, &s.flat, gain)));
            s.sample
                .render(renderer, fg, base, bodies, maps.sample_maps(), stage);
        }
    }

    /// The inspector on `slot`: its index row, the band its id is filed under, and what the
    /// promotion path holds for it — the package first, then staging — gathered now through
    /// the roots this scene resolved; its bundle's maps decode on THIS scene's pool. Pure (no
    /// GPU): the dispatcher pushes it, a test opens it.
    pub(crate) fn inspect(&self, slot: MaterialId) -> MaterialInspector {
        let facts = InspectorFacts::gather(
            slot,
            &self.materials,
            &self.bands,
            &self.package,
            &self.staging,
        );
        MaterialInspector::new(facts, &self.pool)
    }

    // ── read-only accessors, for tests ─────────────────────────────────────────

    /// The authored tree — a clone for a gate or a test to walk.
    pub fn authored_tree(&self) -> Option<UiNode> {
        self.tree.clone()
    }
}

impl Scene for MaterialCatalog {
    /// The inspector closed. Its only answer is "closed" (Esc / pad-B / its Close button all
    /// report `cancelled`), and nothing here changed while it was up — the posters keep their
    /// images — so there is nothing to fold.
    fn modal_closed(&mut self, _modal: &str, _result: &str, _payload: Option<&str>) {}

    fn enter(&mut self, renderer: &mut Renderer) {
        let theme = Theme::build(renderer);
        self.textures = theme.lua_textures().iter().map(|(_, h)| *h).collect();
        self.theme = Some(theme);
        self.seats_checked = false;
        renderer.window().set_title("Material Catalog");
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        self.take_decoded();
        let Some(tree) = self.tree.take() else {
            return Transition::None;
        };
        let model = self.model();
        let snap = UiInput::from_input(input, renderer.size());
        let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        let over_hud = frame.results.is_on("hud_hit");
        // Each assigned slot's box reserved a seat; `render` draws its sphere into it.
        self.seat_samples(&frame.surfaces);
        for s in &mut self.samples {
            s.sample.tick(dt);
        }
        self.hud_commands = frame.commands;
        let mut results = frame.results;

        // The input seam (input-P3): the PUMP resolved this frame's events — the scene
        // owns no Resolver. One dispatch through the walker, which fires the screen's
        // DECLARED intents (Menu) as result names; navigation never reads a raw key. The
        // frame's rects ride along, so the slot pane's authored `nav: grid` walks the boxes
        // as the grid lays them out (Down is the box below).
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&tree, &model)
            .with_rects(&frame.rects)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
        }
        self.tree = Some(tree);

        // A box was pressed — a click, or a pad Confirm on the box under the cursor; both
        // arrive as the box's one `mc_open_<id>` name. Open the inspector on that slot, its
        // facts gathered now (what the promotion path holds for it can have changed since
        // this scene measured the package).
        if let (Some(slot), Some(theme)) = (opened_slot(&results), self.theme) {
            return Transition::Push(Box::new(self.inspect(slot).with_theme(theme)));
        }

        // Menu opens the shell's pause overlay (the screen's declared `on_menu`).
        if results.is_on("pause_open") {
            if let Some(theme) = self.theme {
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

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // The spheres are offscreen surfaces composited into the rects the walk reserved;
        // the HUD replay is the screen surface's final 2D, declared as one overlay.
        self.draw_samples(renderer, fg);
        if let Some(&white) = self.textures.first() {
            let hud_commands = &self.hud_commands;
            let textures = &self.textures;
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
        }
    }

    /// Give every GPU resource the spheres hold back on the way out — each sample's
    /// offscreen target, each bundle upload, the shared flat textures and the bodies'
    /// meshes. The renderer owns them and the scene holds only handles, so teardown never
    /// reclaims them on its own (rule 728E682F).
    fn exit(&mut self, renderer: &mut Renderer) {
        // First, so the queued decodes stop while the GPU resources are given back.
        self.cancel_decodes();
        for s in &mut self.samples {
            s.sample.release(renderer);
            // A flat set only borrows the shared cache's textures; dropping it frees nothing.
            if let Some(SlotMaps::Bundle(maps)) = s.maps.take() {
                maps.release(renderer);
            }
        }
        std::mem::take(&mut self.flats).release(renderer);
        if let Some(bodies) = self.bodies.take() {
            bodies.release(renderer);
        }
        self.pending.clear();
    }
}

/// The scene factory the launcher roster registers — the manifest resolves
/// `materialcatalog.scene.json` and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(MaterialCatalog::new(def))
}

#[cfg(test)]
mod tests;
