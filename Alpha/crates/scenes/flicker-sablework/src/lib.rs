//! **Sablework Bench** — the texture synthesizer console.
//!
//! A face on [`flicker_texture`]: the six-voice channel rack on the left, the
//! tiled swatch in the middle, the output stage on the right. Every control is a
//! two-way `bind` into the recipe; nothing here computes an image, it only turns
//! knobs and shows what the instrument makes of them.
//!
//! # Nothing bakes on the frame thread
//!
//! The 2K baseline takes ~360 ms and even the 256² preview ~10 ms, so a bake is a
//! **worker job**: an edit bumps a generation counter and submits a bake that
//! captures a *clone* of the recipe; results arrive on a channel and the newest
//! one wins. A drag therefore never blocks — the image trails the slider by a
//! frame or two rather than stuttering with it. Stale results are dropped by
//! generation, so a fast drag does not queue up a backlog of images nobody wants.
//!
//! # The scene is a PAIR (five-line architecture)
//!
//! `sablework.scene.json` authors the tree + this scene's style blocks;
//! `sablework.lua` derives the presentation (row washes, view-cell visibility)
//! from the RAW model this behaviour publishes; the Rust component kinds draw.
//! The scene owns no resolver and no bindings — the pump hands it resolved
//! signals, the walker consumes the screen's declared intents, and both input
//! channels land in the ONE dispatcher ([`route::apply`]) as result names.
//!
//! # Why the swatch is one sprite and not nine
//!
//! The preview must show the SEAM, which means showing the swatch repeated. The
//! scene tiles the baked map into a `tiles × tiles` buffer on the CPU and uploads
//! that, so the tree holds one `sprite` node per map instead of nine. The
//! textures are created once at a fixed size and rewritten **in place**
//! (`Renderer::update_texture`), so their ids never change and the authored tree
//! names them by constant `tex` index, in [`MapKind::ALL`] order.
//!
//! Only the map you are LOOKING at is uploaded; the others are marked dirty and
//! uploaded when you switch to them. Pushing all of them every preview frame
//! would be ~14 MB of bus traffic per knob movement to show 2.4 MB of it.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use flicker::render::{Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_materials::{JsonTableSource, MaterialId, Tables};
use flicker_texture::{
    bake, presets, Channel, MapKind, MapSet, TextureRecipe, BAKE_DEFAULT, CHANNEL_COUNT,
    PREVIEW_SIZE,
};
use flicker_worker::WorkerPool;

pub mod commit;
pub mod lit;
pub mod route;

/// The one global UI-element definition + Prism palette every prism-alpha scene
/// reads. This scene's OWN style blocks ride its scene file (`def.styles`) and
/// merge over it — never a second theme file, which would fork the palette.
const HUD_UI_THEME: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_theme.json");

/// The pair script — the scene's LOGIC half, by name (five-line architecture).
const SW_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/sablework.lua");
/// The shipped scene file — the tests' copy of the authored tree (the runtime
/// receives the same file through the manifest `SceneDef`).
const SW_SCENE: &str = include_str!("../../../../content/sensorium/scenes/sablework.scene.json");

/// How many times the swatch repeats per axis in the preview. Read from the
/// merged styles (`sablework.preview.tiles`, authored in the scene file) at
/// `enter`; this is the fallback for a malformed file, and it is deliberately
/// the same value the JSON ships so a missing key degrades to the intended look
/// rather than to something that merely happens not to crash.
const DEFAULT_TILES: u32 = 3;

/// The view-cell ids, in [`MapKind::ALL`] order — ONE vocabulary shared with the
/// authored tree's `visible_bind` cells and `sablework.lua`'s `MAPS`. The drift
/// gates pin the three to each other.
pub const MAP_IDS: [&str; 7] = [
    "map_base",
    "map_normal",
    "map_rough",
    "map_metal",
    "map_ao",
    "map_height",
    "map_emit",
];

/// The LIT view's cell id. Deliberately not an eighth `MAP_IDS` entry: the seven
/// are each one `MapKind` the swatch blits, while this one is a rendered
/// sub-scene of all of them at once. The index `MAP_IDS.len()` is its slot in
/// the tabs' bound number.
pub const LIT_ID: &str = "map_lit";

/// How many tabs the view selector has — the seven flat maps plus the lit view.
pub const VIEW_COUNT: usize = MAP_IDS.len() + 1;

/// One baked preview, tagged with the edit that asked for it.
struct BakeResult {
    generation: u64,
    maps: MapSet,
}

/// Where a Commit has got to. The bench stays fully live through one — a 2K bake
/// is ~360 ms, so it runs on the worker like every other bake and this is what
/// the status line reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitState {
    Idle,
    Working,
    /// Staged, with the folder it landed in — a path, so the reviewer knows where
    /// to look without hunting.
    Done(String),
    Failed(String),
}

pub struct Sablework {
    /// The instrument's state — the thing every control edits and the only thing
    /// that gets saved.
    recipe: TextureRecipe,
    /// Which factory patch was loaded last, so the ‹ › buttons can step them.
    patch: usize,
    /// The voice the right-hand fine knobs edit, `0..CHANNEL_COUNT`.
    sel_ch: usize,
    /// The view the centre shows — an index into the tabs' bound number:
    /// `0..MAP_IDS.len()` are the flat maps, `MAP_IDS.len()` is the LIT view.
    sel_map: usize,
    /// The 256-material index, for the binding picker's labels. Empty if the
    /// tables could not be read — the bench still runs unbound.
    materials: Vec<(MaterialId, String)>,
    /// Which OFFERED bake rung a Commit uses, an index into the offered list.
    size_rung: usize,

    // ── preview ──
    /// Repeats per axis in the swatch. From the merged styles at `enter`.
    tiles: u32,
    /// The preview textures, in [`MapKind::ALL`] order. Created once at
    /// `PREVIEW_SIZE * tiles` square and rewritten in place forever after.
    tex: Vec<TextureHandle>,
    /// The newest baked set, and which of its maps have not reached the GPU yet.
    latest: Option<MapSet>,
    dirty: [bool; MapKind::ALL.len()],

    // ── the bake worker ──
    pool: WorkerPool,
    tx: Sender<BakeResult>,
    rx: Receiver<BakeResult>,
    /// Bumped by every edit; a result older than `shown` is discarded.
    generation: u64,
    shown: u64,
    /// Wall time of the last completed preview bake, for the readout.
    last_bake_ms: f64,
    /// The Commit in flight, if any.
    commit_state: CommitState,
    commit_tx: Sender<CommitState>,
    commit_rx: Receiver<CommitState>,

    // ── engine plumbing ──
    /// The lit sample — the material on a turning body under a fixed light.
    pub(crate) lit: lit::LitPreview,
    /// The rect the walker RESERVED for the lit view this frame, if it is showing.
    /// The tree owns the layout, so the sub-scene is placed by the same walk that
    /// draws everything else rather than by a second set of constants.
    lit_rect: Option<flicker::render::Rect>,

    /// The AUTHORED tree off the manifest's def, walked every frame. `take`n
    /// around the walk so the walker can borrow it beside the mutable UI state.
    authored: Option<UiNode>,
    /// This scene's own style blocks (`def.styles`) — merged over the theme at
    /// `enter`, the five-line home for the bench's values.
    scene_styles_json: Option<serde_json::Value>,
    /// The pair script. `None` only if it failed to load — the bench then runs
    /// on raw model values (no row washes, first view cell only).
    script: Option<ScriptHost>,
    /// The screen's declared signal→result intents, read off the tree ONCE.
    ui_intents: UiIntents,
    ui_state: UiState,
    ui_styles: serde_json::Value,
    hud_commands: Vec<HudCommand>,
    ui_theme: Option<Theme>,
    white: Option<TextureHandle>,
}

impl Default for Sablework {
    fn default() -> Self {
        Self::shipped()
    }
}

impl Sablework {
    /// The runtime constructor — the manifest hands in the authored `SceneDef`.
    pub fn new(def: &SceneDef) -> Self {
        Self::from_parts(def.tree.clone(), def.styles.clone())
    }

    /// A bench on the SHIPPED scene file — the seam a test drives without an
    /// app, exercising the same authored tree the runtime gets.
    pub fn shipped() -> Self {
        let def = SceneDef::parse("sablework", SW_SCENE)
            .expect("the shipped sablework.scene.json parses");
        Self::from_parts(def.tree, def.styles)
    }

    fn from_parts(authored: Option<UiNode>, scene_styles_json: Option<serde_json::Value>) -> Self {
        if authored.is_none() {
            tracing::error!("sablework: the scene def declares no `tree` — no UI will draw");
        }
        let ui_intents = authored.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(SW_SCRIPT, "sablework.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("sablework.lua failed to load — raw model values only: {e}");
                None
            }
        };

        // The material index, through the roots service — the bench asks where
        // content lives, it never spells out a path. A tree that will not load is
        // survivable: the picker offers nothing and every recipe stays unbound,
        // which is visibly wrong rather than silently wrong.
        let data_dir = flicker_content::roots().data();
        let materials = match Tables::from_source(&JsonTableSource::new(&data_dir)) {
            Ok(t) => {
                let mut rows: Vec<(MaterialId, String)> =
                    t.materials().iter().map(|m| (m.id, m.name.clone())).collect();
                rows.sort_by_key(|(id, _)| *id);
                rows
            }
            Err(e) => {
                tracing::warn!("material index unreadable at {}: {e}", data_dir.display());
                Vec::new()
            }
        };
        // Open on the ratified baseline rung, so a Commit does what the spec says
        // without anyone touching the control.
        let size_rung = flicker_texture::size::offered()
            .position(|r| r.px == BAKE_DEFAULT)
            .unwrap_or(0);

        let (tx, rx) = mpsc::channel();
        let (commit_tx, commit_rx) = mpsc::channel();
        let mut me = Self {
            // Open on the first factory patch: a rack of neutral sliders teaches
            // nothing, and an empty preview looks like a broken bench.
            recipe: presets::all().swap_remove(0),
            patch: 0,
            sel_ch: 0,
            sel_map: 0,
            materials,
            size_rung,
            tiles: DEFAULT_TILES,
            tex: Vec::new(),
            latest: None,
            dirty: [false; MapKind::ALL.len()],
            pool: WorkerPool::with_default_size(),
            tx,
            rx,
            generation: 0,
            shown: 0,
            last_bake_ms: 0.0,
            commit_state: CommitState::Idle,
            commit_tx,
            commit_rx,
            lit: lit::LitPreview::default(),
            lit_rect: None,
            authored,
            scene_styles_json,
            script,
            ui_intents,
            ui_state: UiState::default(),
            ui_styles: serde_json::Value::Null,
            hud_commands: Vec::new(),
            ui_theme: None,
            white: None,
        };
        me.request_bake();
        me
    }

    /// The edge length of one preview texture: the swatch, repeated.
    fn tex_px(&self) -> u32 {
        PREVIEW_SIZE * self.tiles
    }

    /// Ask for a fresh preview. Cheap and safe to call on every edit — the job
    /// captures a clone of the recipe, so the caller keeps editing while it runs,
    /// and a result that lands after a newer edit is dropped on arrival.
    fn request_bake(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let recipe = self.recipe.clone();
        let tx = self.tx.clone();
        self.pool.submit(move || {
            let maps = bake(&recipe, PREVIEW_SIZE);
            // A closed receiver just means the bench went away mid-bake.
            let _ = tx.send(BakeResult { generation, maps });
        });
    }

    /// Drain the worker channel, keeping only the newest result.
    fn collect_bakes(&mut self, dt: Duration) {
        let mut newest: Option<BakeResult> = None;
        while let Ok(r) = self.rx.try_recv() {
            if r.generation > newest.as_ref().map_or(self.shown, |n| n.generation) {
                newest = Some(r);
            }
        }
        if let Some(r) = newest {
            self.shown = r.generation;
            self.latest = Some(r.maps);
            // Every map is now stale on the GPU; they upload as they are looked at.
            self.dirty = [true; MapKind::ALL.len()];
            self.last_bake_ms = dt.as_secs_f64() * 1000.0;
        }
    }

    /// Repeat `map` into a `tiles × tiles` buffer — the swatch shown against
    /// itself, which is the only way a seam is visible.
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

    /// Push the currently-shown map to the GPU if it has changed. Called from
    /// `render`, which is where a `&mut Renderer` exists.
    fn upload_shown(&mut self, renderer: &mut Renderer) {
        if !self.dirty.get(self.sel_map).copied().unwrap_or(false) {
            return;
        }
        let Some(handle) = self.tex.get(self.sel_map).copied() else { return };
        let Some(set) = self.latest.as_ref() else { return };
        let Some(map) = set.get(MapKind::ALL[self.sel_map]) else { return };
        let pixels = self.tiled(map);
        if renderer.update_texture(handle, &pixels) {
            self.dirty[self.sel_map] = false;
        } else {
            // Size disagreement: the texture was made at a different edge length
            // than the bake produced. Loud, because a silently stale swatch would
            // have the author dialling against an image that is not their recipe.
            tracing::warn!(
                "preview upload rejected for {:?}: {} bytes for a {}² texture",
                MapKind::ALL[self.sel_map],
                pixels.len(),
                self.tex_px()
            );
        }
    }

    /// Push EVERY stale map. The flat swatch shows one at a time, but the lit
    /// sample binds all of them simultaneously — a map left stale there would
    /// shade the surface with the previous recipe's roughness.
    fn upload_all(&mut self, renderer: &mut Renderer) {
        for i in 0..MapKind::ALL.len() {
            if !self.dirty[i] {
                continue;
            }
            let (Some(handle), Some(set)) = (self.tex.get(i).copied(), self.latest.as_ref()) else {
                continue;
            };
            let Some(map) = set.get(MapKind::ALL[i]) else { continue };
            let pixels = self.tiled(map);
            if renderer.update_texture(handle, &pixels) {
                self.dirty[i] = false;
            }
        }
    }

    // ── the Model ──────────────────────────────────────────────────────────────

    /// The RAW runtime variables the pair script and the tree bind. Display copy
    /// is published as `$token`s (localised at the draw boundary); numbers are
    /// pre-formatted here so the tree never does arithmetic. Presentation logic
    /// (row washes, view-cell visibility) belongs to `sablework.lua`'s
    /// `derive()`, not here.
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::default();

        // The two cursors, as NUMBERS — the tabs bind one, derive() reads both.
        m.set("sel_ch", self.sel_ch);
        m.set("sel_map", self.sel_map);

        for (i, ch) in self.recipe.channels.iter().enumerate() {
            let n = i + 1;
            m.set(format!("ch{n}_on"), ch.enabled);
            m.set(format!("ch{n}_source_label"), format!("$sw_src_{}", ch.source.id()));
            m.set(format!("ch{n}_blend_label"), format!("$sw_blend_{}", ch.blend.id()));
            m.set(format!("ch{n}_scale"), ch.scale as f64);
            m.set(format!("ch{n}_octaves"), ch.octaves as f64);
            m.set(format!("ch{n}_warp"), ch.warp);
            m.set(format!("ch{n}_amount"), ch.amount);
        }

        m.set("lit_body_label", format!("$sw_body_{}", self.lit.body.id()));
        m.set("lit_spin", self.lit.spinning);

        let out = &self.recipe.out;
        for (key, value) in [
            ("relief", out.relief),
            ("roughness", out.roughness),
            ("roughness_mod", out.roughness_mod),
            ("metalness", out.metalness),
            ("metalness_mod", out.metalness_mod),
            ("ao", out.ao),
            ("emissive_strength", out.emissive_strength),
            ("emissive_band", out.emissive_band),
        ] {
            m.set(key, value as f64);
        }

        let ch = self.selected_channel();
        m.set("sel_voice", format!("$sw_ch{}", self.sel_ch + 1));
        m.set("sel_lacunarity", ch.lacunarity);
        m.set("sel_gain", ch.gain);
        m.set("sel_contrast", ch.contrast);
        m.set("sel_invert", ch.invert);

        // Composed from DATA, not from copy: the material name comes from the
        // recipe, the seed is a number, and the separators carry no words.
        let material = self
            .recipe
            .material
            .map(|id| id.to_string())
            .unwrap_or_else(|| "$sw_unbound".into());
        m.set("recipe_line", format!("{} · {} · {:#x}", self.recipe.name, material, self.recipe.seed));
        m.set(
            "preview_info",
            format!("{}² · {}×{}", PREVIEW_SIZE, self.tiles, self.tiles),
        );
        let rung = self.bake_rung();
        m.set(
            "bake_info",
            format!(
                "{}² · {}×{} · {:.0} ms", // strings-gate-exempt: unit symbol, not copy
                PREVIEW_SIZE,
                self.tiles,
                self.tiles,
                self.last_bake_ms
            ),
        );
        m.set("material_label", self.material_label());
        m.set("size_label", rung.label);
        m.set(
            "commit_status",
            match &self.commit_state {
                CommitState::Idle => "$sw_commit_idle".to_string(),
                CommitState::Working => "$sw_commit_working".to_string(),
                // The folder it landed in — a path, so the reviewer can go look.
                CommitState::Done(dir) => dir.clone(),
                CommitState::Failed(why) => why.clone(),
            },
        );
        m
    }

    /// The frame's full model: the raw variables plus the pair script's derived
    /// presentation values, folded over them.
    pub(crate) fn model(&self) -> ValueMap {
        let raw = self.hud_model();
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("sablework: publishing the model to the script failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => m.extend(derived),
                Ok(None) => {}
                Err(e) => tracing::error!("sablework: derive() failed: {e}"),
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

    /// Step the bake rung. Only OFFERED rungs are reachable: 4K/8K bake correctly
    /// but stay off the picker until the engine-level memory budget lands, and
    /// this is the one place that gate is consulted.
    pub(crate) fn step_size(&mut self) {
        let n = flicker_texture::size::offered().count().max(1);
        self.size_rung = (self.size_rung + 1) % n;
    }

    /// Step the material binding: unbound → each material in index order →
    /// unbound. Unbound is a real state, not an absence — a scratch surface you
    /// have not decided the identity of yet.
    pub(crate) fn step_material(&mut self) {
        if self.materials.is_empty() {
            return;
        }
        self.recipe.material = match self.recipe.material {
            None => Some(self.materials[0].0),
            Some(cur) => {
                let at = self.materials.iter().position(|(id, _)| *id == cur);
                match at {
                    Some(i) if i + 1 < self.materials.len() => Some(self.materials[i + 1].0),
                    // Past the end, or bound to an id the index does not carry:
                    // both land back at unbound rather than guessing.
                    _ => None,
                }
            }
        };
    }

    /// The material's display name, or the unbound token.
    fn material_label(&self) -> String {
        match self.recipe.material {
            Some(id) => self
                .materials
                .iter()
                .find(|(m, _)| *m == id)
                .map(|(_, name)| name.clone())
                // A binding the index cannot resolve must READ as broken rather
                // than as unbound — they are different problems.
                .unwrap_or_else(|| format!("#{id}")),
            None => "$sw_unbound".into(),
        }
    }

    /// Bake at the chosen rung and write the artifact folder into `staging/`.
    ///
    /// On the WORKER: the baseline rung is ~360 ms, and a bench that froze while
    /// committing would be a bench nobody commits from. The scene stays live and
    /// the status line reports.
    pub(crate) fn start_commit(&mut self) {
        if self.commit_state == CommitState::Working {
            return; // one at a time; a second click is not a second artifact
        }
        self.commit_state = CommitState::Working;
        let recipe = self.recipe.clone();
        let size = self.bake_rung().px;
        let staging = flicker_content::roots().staging();
        let tx = self.commit_tx.clone();
        self.pool.submit(move || {
            let state = match commit::commit(&recipe, size, &staging) {
                Ok(out) => CommitState::Done(out.dir.display().to_string()),
                Err(e) => CommitState::Failed(e.to_string()),
            };
            let _ = tx.send(state);
        });
    }

    fn selected_channel(&self) -> Channel {
        self.recipe.channels[self.sel_ch.min(CHANNEL_COUNT - 1)]
    }

    // ── read-only accessors, for tests ─────────────────────────────────────────

    pub fn recipe(&self) -> &TextureRecipe {
        &self.recipe
    }
    /// The map the swatch shows. On the LIT tab there is no single map — the
    /// sample wears all of them — so this reports the base colour, which is what
    /// the lit view's albedo is.
    pub fn selected_map(&self) -> MapKind {
        MapKind::ALL[self.sel_map.min(MapKind::ALL.len() - 1)]
    }

    /// Whether the LIT view is the one showing.
    pub fn showing_lit(&self) -> bool {
        self.sel_map == MAP_IDS.len()
    }
    pub fn selected_voice(&self) -> usize {
        self.sel_ch
    }
    /// The generation of the newest preview handed to the scene — a test's proof
    /// that an edit actually re-baked.
    pub fn shown_generation(&self) -> u64 {
        self.shown
    }
    /// The authored tree — a clone for a gate or a test to walk.
    pub fn authored_tree(&self) -> Option<UiNode> {
        self.authored.clone()
    }
}

impl Scene for Sablework {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        self.ui_theme = Some(Theme::build(renderer));
        // The theme, the shared satellites, and THIS scene's own style blocks
        // (`def.styles` — the five-line home for the bench's values).
        self.ui_styles =
            flicker::ui::load_styles_for(HUD_UI_THEME, self.scene_styles_json.as_ref());
        self.tiles = self
            .ui_styles
            .get("sablework")
            .and_then(|s| s.get("preview"))
            .and_then(|p| p.get("tiles"))
            .and_then(|t| t.as_u64())
            .map(|t| t.clamp(1, 4) as u32)
            .unwrap_or(DEFAULT_TILES);

        // The preview textures, created ONCE at the tiled size and rewritten in
        // place forever after — which is what lets the authored tree name them by
        // a constant `tex` index. Base colour is the only sRGB map; the rest
        // carry data, and uploading them through the colour path would
        // gamma-correct numbers that were never a colour.
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
        self.dirty = [true; MapKind::ALL.len()];

        renderer.window().set_title("Sablework Bench");
    }

    fn update(&mut self, dt: Duration, input: &InputState, signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        self.collect_bakes(dt);
        while let Ok(state) = self.commit_rx.try_recv() {
            if let CommitState::Failed(ref why) = state {
                tracing::error!("commit failed: {why}");
            }
            self.commit_state = state;
        }

        let Some(tree) = self.authored.take() else { return Transition::None };
        let screen = renderer.size();
        let model = self.model();
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
        };
        let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        let over_hud = frame.results.is_on("hud_hit");
        // The `rtt` node reserved a rect for the lit sub-scene; `render` draws into
        // it. `None` when the Lit tab is not showing, which is also what stops the
        // offscreen pass from costing anything.
        self.lit_rect = frame.rtt_rect("sw_lit");
        self.hud_commands = frame.commands;
        self.lit.tick(dt);

        // ── The input seam (input-P3): the PUMP resolved this frame's events —
        // the scene owns no Resolver. One dispatch through the walker, which owns
        // the focus graph (left stick between panels, d-pad inside one), consumes
        // the pointer while it is over the HUD, and fires the screen's DECLARED
        // intents (Menu / TabNext / TabPrev / PageNext / PagePrev) as result
        // names — navigation never reads a raw key. ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&tree, &model)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let fired = walker.take_fired();
        self.authored = Some(tree);

        // Fold the fired intent names in beside the click results, so both
        // channels reach the ONE dispatcher identically.
        let mut results = frame.results.clone();
        for name in fired {
            results.set(name, true);
        }

        // The dispatcher owns every edit AND decides whether one happened; a
        // re-bake is requested exactly when the recipe actually changed, so
        // hovering a slider does not queue work.
        if route::apply(self, &results) {
            self.request_bake();
        }

        if results.is_on("pause_open") {
            if let Some(theme) = self.ui_theme {
                // The pause overlay shows the PROFILE's map — the pump owns
                // bindings now (input-P3), the scene holds none.
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

    fn render(&mut self, renderer: &mut Renderer) {
        self.upload_shown(renderer);
        // The lit sample is an offscreen pass, so it runs BEFORE the 2D HUD — the
        // FrameGraph composites its result into the rect the walk reserved, under
        // the chrome the HUD then draws over.
        if let Some(rect) = self.lit_rect {
            // The lit view wears every map at once, so all of them must be
            // current — not just the one a flat swatch would be showing.
            self.upload_all(renderer);
            let mut fg = flicker::render::FrameGraph::new();
            self.lit.render(
                renderer,
                &mut fg,
                rect,
                renderer.layer(),
                &self.tex,
                lit::Stage::from_styles(&self.ui_styles, lit::STAGE_SOURCE),
            );
            fg.execute(renderer);
        }
        if let Some(white) = self.white {
            let tex = self.tex.clone();
            render_hud(renderer, &self.hud_commands, white, &tex);
        }
    }
}

/// The scene factory the launcher roster registers — the manifest resolves
/// `sablework.scene.json` and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(Sablework::new(def))
}

#[cfg(test)]
mod tests;
