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
//! # The UI is DATA
//!
//! Following the Quartermaster, not the older benches: this scene owns no HUD Lua
//! and composes nothing. `ui_templates.json` holds the `sablework_console` proto
//! (a `workbench` carrying a fixed bank of six `synth_voice` rows); [`build_tree`]
//! emits ONE instance node, declares the screen's input as `on_<signal>` props,
//! and calls `expand` at the end — the single seam, so the scene and every gate
//! walk the same tree.
//!
//! # Why the swatch is one sprite and not nine
//!
//! The preview must show the SEAM, which means showing the swatch repeated. The
//! scene tiles the baked map into a `tiles × tiles` buffer on the CPU and uploads
//! that, so the tree holds one `sprite` node per map instead of nine. The six
//! textures are created once at a fixed size and rewritten **in place**
//! (`Renderer::update_texture`), so their ids never change and the HUD tree can
//! name them as constants.
//!
//! Only the map you are LOOKING at is uploaded; the others are marked dirty and
//! uploaded when you switch to them. Pushing all six every preview frame would be
//! ~14 MB of bus traffic per knob movement to show 2.4 MB of it.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, UiNode, Value, ValueMap};
use flicker::ui::{
    builtin_templates, expand, load_styles, render_hud, run_ui, TemplateRegistry, UiInput,
    UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{
    AbstractControls, ContextualBindings, Fired, GamepadConfig, InputMap, InputState, Resolver,
};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
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

/// The console's COMPOSITION lives in `ui_templates.json` as this proto; the scene
/// only configures it. There is deliberately no `hud_sablework.lua` — a per-scene
/// script that composed the surface would put composition back in code.
const CONSOLE_TEMPLATE: &str = "sablework_console";

/// The scene's layout + `$token` styles live in the shared `ui_elements.json` —
/// the ONE global UI-element definition + Prism palette every prism-alpha scene
/// reads — under the `sablework` key. NOT a per-scene copy: a second file would
/// need its own `theme.tokens`, forking the palette.
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_elements.json");

/// How many times the swatch repeats per axis in the preview. Read from
/// `ui_elements.json` at load so the Lua and the scene cannot disagree; this is
/// the fallback for a malformed file, and it is deliberately the same value the
/// JSON ships so a missing key degrades to the intended look rather than to
/// something that merely happens not to crash.
const DEFAULT_TILES: u32 = 3;

/// The style a rack row draws with, selected or not. Dotted paths, so they are
/// wiring rather than display copy.
const ROW_STYLE: &str = "sablework.row";
const ROW_STYLE_SEL: &str = "sablework.row_sel";
const BTN_STYLE: &str = "sablework.button";
const BTN_STYLE_ON: &str = "sablework.button_on";

/// The preview map buttons, in [`MapKind::ALL`] order. The id is BOTH the node id
/// (so the button fires it) and the Model-key stem for its `_style` / `_shown`
/// binds — one vocabulary shared with `hud_sablework.lua`'s `MAPS`.
const MAP_IDS: [&str; 7] = [
    "map_base",
    "map_normal",
    "map_rough",
    "map_metal",
    "map_ao",
    "map_height",
    "map_emit",
];

/// The LIT view's tab id. Deliberately not a seventh `MAP_IDS` entry: the six are
/// each one `MapKind` the swatch blits, while this one is a rendered sub-scene of
/// all of them at once. Sharing the `map_*` naming keeps ONE tab vocabulary; the
/// index `MAP_IDS.len()` is its slot in the selection.
const LIT_ID: &str = "map_lit";

/// How many tabs the view selector has — the six flat maps plus the lit view.
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
    /// The map the swatch shows, an index into [`MapKind::ALL`].
    sel_map: usize,
    /// The 256-material index, for the binding picker's labels. Empty if the
    /// tables could not be read — the bench still runs unbound.
    materials: Vec<(MaterialId, String)>,
    /// Which OFFERED bake rung a Commit uses, an index into the offered list.
    size_rung: usize,

    // ── preview ──
    /// Repeats per axis in the swatch. From `ui_elements.json`.
    tiles: u32,
    /// The six preview textures, in [`MapKind::ALL`] order. Created once at
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

    /// The proto registry `build_tree` expands against — built once.
    templates: TemplateRegistry,
    ui_intents: UiIntents,
    ui_state: UiState,
    ui_styles: serde_json::Value,
    hud_commands: Vec<HudCommand>,
    ui_theme: Option<Theme>,
    white: Option<TextureHandle>,

    bindings: ContextualBindings,
    gamepad_config: GamepadConfig,
    resolver: Resolver,
    ev: Vec<Fired>,
    tick: u64,
}

impl Default for Sablework {
    fn default() -> Self {
        Self::new()
    }
}

impl Sablework {
    pub fn new() -> Self {
        let ui_styles = load_styles(HUD_UI_ELEMENTS);
        let tiles = ui_styles
            .get("sablework")
            .and_then(|s| s.get("preview"))
            .and_then(|p| p.get("tiles"))
            .and_then(|t| t.as_u64())
            .map(|t| t.clamp(1, 4) as u32)
            .unwrap_or(DEFAULT_TILES);

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
            tiles,
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
            templates: builtin_templates(),
            ui_intents: UiIntents::default(),
            ui_state: UiState::default(),
            ui_styles,
            hud_commands: Vec::new(),
            ui_theme: None,
            white: None,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            tick: 0,
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
    /// sample binds all six simultaneously — a map left stale there would shade
    /// the surface with the previous recipe's roughness.
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

    /// Everything `hud_sablework.lua` binds. Display copy is published as
    /// `$token`s (localised at the draw boundary); numbers are pre-formatted here
    /// so the tree never does arithmetic.
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::default();

        for (i, ch) in self.recipe.channels.iter().enumerate() {
            let n = i + 1;
            m.set(format!("ch{n}_on"), ch.enabled);
            m.set(format!("ch{n}_name"), format!("$sw_ch{n}"));
            m.set(format!("ch{n}_source"), format!("$sw_src_{}", ch.source.id()));
            m.set(format!("ch{n}_blend"), format!("$sw_blend_{}", ch.blend.id()));
            m.set(format!("ch{n}_scale"), ch.scale as f64);
            m.set(format!("ch{n}_octaves"), ch.octaves as f64);
            m.set(format!("ch{n}_warp"), ch.warp);
            m.set(format!("ch{n}_amount"), ch.amount);
            m.set(
                format!("ch{n}_style"),
                if i == self.sel_ch { ROW_STYLE_SEL } else { ROW_STYLE },
            );
        }

        for (i, id) in MAP_IDS.iter().chain([&LIT_ID]).enumerate() {
            let on = i == self.sel_map;
            m.set(format!("{id}_shown"), on);
            m.set(format!("{id}_style"), if on { BTN_STYLE_ON } else { BTN_STYLE });
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

    /// This frame's screen — the input declaration plus ONE configured console.
    ///
    /// Rebuilt every frame: re-expansion is what keeps a template PARAM live (a
    /// data-child's props are not model-resolved), and it is cheap because the
    /// walker's draw cache is structural — an identical tree replays for free.
    pub fn build_tree(&self, _screen: Vec2) -> UiNode {
        let mut page = UiNode { component: "screen".into(), ..Default::default() };
        // The screen's input DECLARATION. Everything the bench reacts to is named
        // here as DATA, so a pad press, a key and a click are the same event by the
        // time the dispatcher sees it — and the scene hand-rolls no `esc_prev`.
        //
        // Controller is the floor, using only the ratified vocabulary: the bumpers
        // (`Tab*`) walk the MAP tabs — six views of one artifact is exactly what a
        // tab is. NOTHING ELSE is declared: Confirm, Cancel and `Panel*` are the
        // WALKER's on every screen in Prism (it activates the focused control,
        // backs out, and cycles the panels), and naming one here bound four dead
        // result names while statically killing activation on this bench —
        // violation F1, 2026-08-09. Selecting a VOICE is movement WITHIN the rack
        // panel, so it belongs to the d-pad on the walker's focus graph; touching
        // any of a voice's controls also selects it.
        for (signal, result) in [
            ("on_menu", "pause_open"),
            ("on_tab_next", "map_next"),
            ("on_tab_prev", "map_prev"),
        ] {
            page.props.insert(signal.into(), Value::Text(result.into()));
        }
        page.children = vec![UiNode {
            template: Some(CONSOLE_TEMPLATE.into()),
            ..Default::default()
        }];
        // Expanded HERE, not at the call sites, so the scene and every gate walk the
        // SAME tree. An unresolved proto would otherwise draw a bare box in the app
        // while the tests inspected a `template` node they never opened.
        expand(page, &self.templates)
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
}

impl Scene for Sablework {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        self.ui_theme = Some(Theme::build(renderer));

        // Six preview textures, created ONCE at the tiled size and rewritten in
        // place forever after — which is what lets the HUD tree name them by a
        // constant id. Base colour is the only sRGB map; the rest carry data, and
        // uploading them through the colour path would gamma-correct numbers that
        // were never a colour.
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

    fn update(&mut self, dt: Duration, input: &InputState, _signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        self.collect_bakes(dt);
        while let Ok(state) = self.commit_rx.try_recv() {
            if let CommitState::Failed(ref why) = state {
                tracing::error!("commit failed: {why}");
            }
            self.commit_state = state;
        }

        if let Some((map, _, _)) = flicker_shell::take_pending_input() {
            self.bindings = ContextualBindings::new(map);
        }

        let screen = renderer.size();
        let tree = self.build_tree(screen);
        self.ui_intents = UiIntents::of(&tree);
        let model = self.hud_model();
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

        // ONE resolve, ONE dispatch — the walker layer consumes the screen's
        // declared intents, so navigation never reads a raw key.
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
        let events: Vec<InputEvent> =
            self.ev.iter().map(|f| InputEvent::from_fired(f, ctx, input)).collect();

        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        let mut route = RouteCtx::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        let focus_change = apply_context_requests(&mut self.bindings, &route.requests);
        walker.apply_focus(focus_change);

        // Fold the fired intent names in beside the click results, so both
        // channels reach the ONE dispatcher identically.
        let mut results = frame.results.clone();
        for name in walker.take_fired() {
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
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    self.bindings.active_map(),
                    &AbstractControls::default(),
                    &self.gamepad_config,
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
            // The lit view wears every map at once, so all six must be current —
            // not just the one a flat swatch would be showing.
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

/// The scene factory the launcher roster registers.
pub fn scene() -> Box<dyn Scene> {
    Box::new(Sablework::new())
}

#[cfg(test)]
mod tests;
