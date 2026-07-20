//! Front-end shell scenes + the settings/config model (private to the crate).
//! Only [`run`], [`ShellConfig`], [`PauseScene`], and [`take_pending_input`] are
//! public (re-exported from the crate root); everything else — the splash/menu/
//! settings/pause scenes, their embedded Lua scripts + `ui_elements.json`, and
//! display/settings persistence — is internal.

use std::sync::Mutex;
use std::time::Duration;

use flicker::app::{
    run as run_app, AbstractControls, Action, GamepadConfig, InputMap, InputState, Key,
    RebindCapture,
};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json_str, load_widgets, render_hud};

use crate::display;
use crate::theme::Theme;

/// A boxed factory that builds the client's in-game scene when the player hits
/// START. The shell never names the game type; the client passes this in.
pub type GameSceneFactory = Box<dyn Fn() -> Box<dyn Scene>>;

/// What a client hands [`run`]: everything the shell needs that it can't know
/// itself. Currently just the game-scene factory; branding/title fields can be
/// added here later without touching the call site.
pub struct ShellConfig {
    /// Builds the in-game scene START launches.
    pub game_scene: GameSceneFactory,
    /// The app's project root, where the per-user `settings.json` (display mode/
    /// resolution, keybindings, audio) is read/written — usually
    /// `env!("CARGO_MANIFEST_DIR").into()` so each shell app keeps its own
    /// (gitignored) settings in its own root. `None` falls back to the current
    /// working directory.
    pub settings_dir: Option<std::path::PathBuf>,
    /// Label for the menu's game-launch button (the `start` item). `None` uses the
    /// default from `ui_elements.json` ("ENTER WORLD"); a client sets it to name its
    /// mode — e.g. the click trainer → "CLICK TRAINER".
    pub game_label: Option<String>,
}

/// Restore the persisted display setting, then run the whole front-end flow —
/// intro splash → menu → *the client's scene* → pause/settings — on the winit
/// loop. Blocks until the window closes. The one entry point a client calls.
pub fn run(config: ShellConfig) -> anyhow::Result<()> {
    display::set_settings_dir(config.settings_dir.clone());
    display::load_from_disk();
    run_app(SceneManager::new(Box::new(LogoScene::new(
        config.game_scene,
        config.game_label,
    ))))
}

/// Take any pending input-settings change made in the pause→settings overlay,
/// for the in-game scene to apply; `None` when nothing changed. (The push side
/// from the settings scene is a designed-but-unwired seam, so this returns
/// `None` today.)
pub fn take_pending_input() -> Option<(InputMap, AbstractControls, GamepadConfig)> {
    INPUT_SETTINGS.lock().ok().and_then(|mut p| p.take())
}

/// Full settings state persisted to `settings.json`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct GameSettings {
    audio: AudioSettings,
    video: VideoSettings,
    input: InputSettings,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct AudioSettings {
    master: f32,
    music: f32,
    sfx: f32,
    voice: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self { master: 0.8, music: 0.6, sfx: 0.7, voice: 0.9 }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct VideoSettings {
    display_mode: usize,
    resolution: usize,
    quality: usize,
    vsync: bool,
    fps_limit: usize,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self { display_mode: 1, resolution: 3, quality: 3, vsync: true, fps_limit: 2 }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct InputSettings {
    mouse_sensitivity: f32,
    sprint_sensitivity: f32,
    invert_pitch: bool,
    invert_yaw: bool,
    raw_input: bool,
    stick_sensitivity: f32,
    left_deadzone: f32,
    right_deadzone: f32,
    trigger_threshold: f32,
    invert_stick_pitch: bool,
    invert_stick_yaw: bool,
    deadzone_shape: usize,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.005,
            sprint_sensitivity: 0.005,
            invert_pitch: false,
            invert_yaw: false,
            raw_input: true,
            stick_sensitivity: 2.0,
            left_deadzone: 0.2,
            right_deadzone: 0.2,
            trigger_threshold: 0.5,
            invert_stick_pitch: false,
            invert_stick_yaw: false,
            deadzone_shape: 0,
        }
    }
}

impl GameSettings {
    fn settings_path() -> std::path::PathBuf {
        crate::display::settings_dir().join("settings.json")
    }

    fn save(&self) {
        let path = Self::settings_path();
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&path, bytes) {
                    tracing::warn!("failed to write {}: {e}", path.display());
                }
            }
            Err(e) => tracing::warn!("failed to serialize settings: {e}"),
        }
    }
}

static GAME_SETTINGS: Mutex<GameSettings> = Mutex::new(GameSettings {
    audio: AudioSettings { master: 0.8, music: 0.6, sfx: 0.7, voice: 0.9 },
    video: VideoSettings { display_mode: 1, resolution: 3, quality: 3, vsync: true, fps_limit: 2 },
    input: InputSettings {
        mouse_sensitivity: 0.005, sprint_sensitivity: 0.005, invert_pitch: false, invert_yaw: false,
        raw_input: true, stick_sensitivity: 2.0, left_deadzone: 0.2, right_deadzone: 0.2,
        trigger_threshold: 0.5, invert_stick_pitch: false, invert_stick_yaw: false, deadzone_shape: 0,
    },
});

/// Input settings changes pushed from the pause scene and consumed by
/// the game scene. `None` when no pending change exists.
static INPUT_SETTINGS: Mutex<Option<(InputMap, AbstractControls, GamepadConfig)>> =
    Mutex::new(None);

/// How long the logo splash shows before auto-advancing to the menu.
/// Lua-driven intro splash (`scripts/logo.lua`, `UI.logo`): a sequence of
/// full-screen logos that fade in / hold / fade out before the menu.
const LOGO_SCRIPT: &str = include_str!("../../../../content/scripts/logo.lua");

/// Intro logo images, in play order (publisher then engine), exposed to the
/// script as the `Textures` names in `UI.logo.images`. Embedded in the crate so
/// every client inherits the publisher/engine splash with no copied assets.
const LOGO_IMAGES: [(&str, &[u8]); 2] = [
    (
        "elideus",
        include_bytes!("../../../../content/assets/elideus_productions_yellow.png"),
    ),
    (
        "clay",
        include_bytes!("../../../../content/assets/clay_engine_infinity_grey.png"),
    ),
];

/// Decode an embedded PNG/JPEG and upload it as a texture, returning the handle
/// and its pixel size. Logs and yields `None` on failure (the splash degrades
/// to its backdrop rather than crashing).
fn load_image_texture(renderer: &mut Renderer, bytes: &[u8]) -> Option<(TextureHandle, u32, u32)> {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            Some((renderer.load_texture(&rgba, w, h), w, h))
        }
        Err(e) => {
            tracing::error!("failed to decode embedded logo image: {e}");
            None
        }
    }
}

/// Intro splash: plays the logo sequence (timeline + fade in `logo.lua`), then
/// replaces itself with the menu — also skippable with click / Space / Escape.
/// The hold-time is, in future, room to stream the menu's background scene.
struct LogoScene {
    script: Option<ScriptHost>,
    /// `[white, <logos…>]` — index = the texture id the script references.
    textures: Vec<TextureHandle>,
    /// Each logo's native pixel size, parallel to `UI.logo.images`, so the
    /// script can fit + centre them.
    sizes: Vec<(u32, u32)>,
    elapsed: Duration,
    /// The client's game-scene factory, carried through to the [`MenuScene`] so
    /// START can launch it. `Some` until this scene transitions.
    game_scene: Option<GameSceneFactory>,
    /// The menu's game-launch button label, carried to the [`MenuScene`].
    game_label: Option<String>,
}

impl LogoScene {
    fn new(game_scene: GameSceneFactory, game_label: Option<String>) -> Self {
        Self {
            script: None,
            textures: Vec::new(),
            sizes: Vec::new(),
            elapsed: Duration::ZERO,
            game_scene: Some(game_scene),
            game_label,
        }
    }

    /// Per-frame model: elapsed seconds + each logo's native size (`img1_w`…).
    fn model(&self) -> ValueMap {
        let mut model = ValueMap::new().with("elapsed", self.elapsed.as_secs_f32());
        for (i, &(w, h)) in self.sizes.iter().enumerate() {
            model.set(format!("img{}_w", i + 1), w);
            model.set(format!("img{}_h", i + 1), h);
        }
        model
    }
}

impl Scene for LogoScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // id 0 is the white pixel (rect fills / backdrop); the logos follow.
        let mut textures = vec![renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1)];
        let mut ids: Vec<(&str, u32)> = vec![("white", 0)];
        let mut sizes = Vec::new();
        for (name, bytes) in LOGO_IMAGES {
            if let Some((handle, w, h)) = load_image_texture(renderer, bytes) {
                ids.push((name, textures.len() as u32));
                textures.push(handle);
                sizes.push((w, h));
            } else {
                sizes.push((1, 1)); // keep `imgN` indices aligned if a load fails
            }
        }
        self.script = match ScriptHost::new(LOGO_SCRIPT, "logo.lua") {
            Ok(script) => {
                if let Err(e) = script.set_texture_ids(&ids) {
                    tracing::error!("logo texture registration failed: {e}");
                }
                expose_ui_elements(&script);
                load_widgets(&script);
                Some(script)
            }
            Err(e) => {
                tracing::error!("logo script load failed: {e}");
                None
            }
        };
        self.textures = textures;
        self.sizes = sizes;
        // Apply the persisted (or default) display setting now the window
        // exists — so a saved fullscreen/resolution choice takes effect at
        // launch.
        display::current().apply(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        self.elapsed += dt;
        let skip =
            input.mouse_left_pressed || input.key_down(Key::Space) || input.key_down(Key::Escape);
        // The script owns the timeline and reports `done` once it has played.
        let done = match self.script.as_ref() {
            Some(script) => {
                let model = self.model();
                let _ = script.set_model(&model);
                let size = renderer.size();
                script
                    .update(input, size.x, size.y)
                    .map(|r| r.is_on("done"))
                    .unwrap_or(true)
            }
            None => true,
        };
        if skip || done {
            return Transition::Replace(Box::new(MenuScene::new(
                self.game_scene
                    .take()
                    .expect("game factory present until the splash advances"),
                self.game_label.take(),
            )));
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(script) = self.script.as_ref() else {
            return;
        };
        let model = self.model();
        let _ = script.set_model(&model);
        let size = renderer.size();
        match script.draw(size.x, size.y) {
            Ok(commands) => render_hud(renderer, &commands, self.textures[0], &self.textures),
            Err(e) => tracing::error!("logo script draw failed: {e}"),
        }
    }
}

/// Seconds the confirm overlay waits before auto-reverting a display change.
const CONFIRM_SECS: f32 = 15.0;

/// A selection made in the settings dropdowns.
enum DisplayChange {
    Mode(display::DisplayMode),
}

/// Apply `change` to the window immediately and record it as current. Returns
/// `Some(previous)` when the change should be confirmed-or-reverted (any
/// resolution change, or switching to exclusive fullscreen); `None` when it is
/// safe to apply outright (windowed / borderless toggles).
fn apply_display_change(
    change: DisplayChange,
    renderer: &Renderer,
) -> Option<display::DisplaySetting> {
    let prev = display::current();
    let (next, confirm) = match change {
        DisplayChange::Mode(m) => (
            display::DisplaySetting {
                mode: m,
                res: prev.res,
            },
            matches!(m, display::DisplayMode::ExclusiveFullscreen),
        ),
    };
    next.apply(renderer);
    display::set_current(next);
    confirm.then_some(prev)
}

/// Confirm-or-revert overlay shown after a resolution / exclusive-fullscreen
/// change: the change is already applied, and this waits up to [`CONFIRM_SECS`]
/// for the player to Keep it — auto-reverting to `previous` on Revert or
/// timeout. Pushed as an overlay (same mechanism as the pause menu), so it
/// works over the menu or the pause screen.
struct ConfirmDisplayScene {
    modal: ModalUi,
    previous: display::DisplaySetting,
    remaining: f32,
}

impl ConfirmDisplayScene {
    fn new(theme: Theme, previous: display::DisplaySetting) -> Self {
        Self {
            modal: ModalUi::new(&theme, None),
            previous,
            remaining: CONFIRM_SECS,
        }
    }

    fn revert(&self, renderer: &Renderer) {
        self.previous.apply(renderer);
        display::set_current(self.previous);
    }

    /// The countdown subtitle the modal renders under the title.
    fn subtitle(&self) -> String {
        format!("Reverting in {}s", self.remaining.ceil().max(0.0) as i32)
    }
}

impl Scene for ConfirmDisplayScene {
    fn is_overlay(&self) -> bool {
        true
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        self.remaining -= dt.as_secs_f32();
        if self.remaining <= 0.0 {
            self.revert(renderer);
            return Transition::Pop;
        }
        // The shared modal's `confirm` screen — its `[0,0,0,0.25]` overlay is the
        // light dim that keeps the new resolution visible behind the dialog.
        let actions = self.modal.update(input, renderer, "confirm", None);
        if actions.is_on("keep") {
            return Transition::Pop;
        }
        if actions.is_on("revert") {
            self.revert(renderer);
            return Transition::Pop;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let subtitle = self.subtitle();
        self.modal.render(renderer, "confirm", Some(&subtitle));
    }
}

/// The unified settings Lua script, embedded so clients inherit it.
const SETTINGS_SCRIPT: &str = include_str!("../../../../content/scripts/settings.lua");

/// The shared gothic-modal script (menu / pause / confirm, selected per frame by
/// the `screen` model value), embedded.
const MODAL_SCRIPT: &str = include_str!("../../../../content/scripts/modal.lua");

/// The shell's declarative UI layout (`modal`/`screens`/`settings`/`logo`/
/// `loading`), embedded. The client's in-game HUD layout is separate.
const SHELL_UI_JSON: &str = include_str!("../../../../content/resources/ui_elements.json");

/// Expose the embedded shell `ui_elements.json` to `script` as the `UI` global,
/// so a screen reads its layout from named elements (`UI.modal.panel.w`) instead
/// of hardcoded constants. Logs and continues on failure (scripts guard
/// `if not UI`).
fn expose_ui_elements(script: &ScriptHost) {
    load_ui_json_str(script, SHELL_UI_JSON);
}

// `load_widgets` (and the embedded `widgets.lua` toolkit) live in `flicker-ui`
// and are imported above; `scripts/widgets.lua` was retired.

/// Load an embedded UI script `source` (named `chunk_name` for error messages),
/// register the theme's textures by name (index = id), expose the shell
/// `ui_elements.json` as the `UI` global, and the `Widgets` toolkit. Best-effort:
/// a load failure logs and yields `(None, textures)` so the scene degrades
/// gracefully. The shared front-door for every Lua-driven shell screen.
fn load_ui_script(
    source: &str,
    chunk_name: &str,
    theme: &Theme,
) -> (Option<ScriptHost>, Vec<TextureHandle>) {
    let entries = theme.lua_textures();
    let textures = entries.iter().map(|(_, handle)| *handle).collect();
    let script = match ScriptHost::new(source, chunk_name) {
        Ok(script) => {
            let ids: Vec<(&str, u32)> = entries
                .iter()
                .enumerate()
                .map(|(i, (name, _))| (*name, i as u32))
                .collect();
            if let Err(e) = script.set_texture_ids(&ids) {
                tracing::error!("texture registration failed for {chunk_name}: {e}");
            }
            expose_ui_elements(&script);
            load_widgets(&script);
            Some(script)
        }
        Err(e) => {
            tracing::error!("script load failed ({chunk_name}): {e}");
            None
        }
    };
    (script, textures)
}

/// A loaded `modal.lua` plus the theme textures it draws with — the shared
/// machinery behind every gothic-modal scene (menu / pause / confirm). The
/// scene picks which screen to show by name and supplies an optional dynamic
/// subtitle; this routes the per-frame `Model` (`screen` + `subtitle`) in, runs
/// the script, and renders its commands. Each scene keeps its own transitions.
struct ModalUi {
    script: Option<ScriptHost>,
    textures: Vec<TextureHandle>,
    /// Optional override for the menu's game-launch button label (the `start`
    /// item); `None` on pause/confirm. The modal script reads `Model.game_label`.
    game_label: Option<String>,
}

impl ModalUi {
    /// Load `modal.lua` + expose the theme textures and UI layout. `game_label`
    /// overrides the menu's launch-button label (the `start` item); `None` else.
    fn new(theme: &Theme, game_label: Option<String>) -> Self {
        let (script, textures) = load_ui_script(MODAL_SCRIPT, "modal.lua", theme);
        Self {
            script,
            textures,
            game_label,
        }
    }

    /// The per-frame model selecting the screen + its optional dynamic subtitle +
    /// the optional game-launch label override.
    fn model(&self, screen: &str, subtitle: Option<&str>) -> ValueMap {
        let mut model = ValueMap::new().with("screen", screen);
        if let Some(text) = subtitle {
            model.set("subtitle", text);
        }
        if let Some(label) = &self.game_label {
            model.set("game_label", label.as_str());
        }
        model
    }

    /// Run the script's `update` for `screen`; returns the fired actions
    /// (`is_on("start")` etc.). Empty if the script failed to load.
    fn update(
        &self,
        input: &InputState,
        renderer: &Renderer,
        screen: &str,
        subtitle: Option<&str>,
    ) -> ValueMap {
        let Some(script) = self.script.as_ref() else {
            return ValueMap::new();
        };
        let _ = script.set_model(&self.model(screen, subtitle));
        let size = renderer.size();
        script.update(input, size.x, size.y).unwrap_or_else(|e| {
            tracing::error!("modal update failed: {e}");
            ValueMap::new()
        })
    }

    /// Render `screen` via `render_hud`.
    fn render(&self, renderer: &mut Renderer, screen: &str, subtitle: Option<&str>) {
        let Some(script) = self.script.as_ref() else {
            return;
        };
        let _ = script.set_model(&self.model(screen, subtitle));
        let size = renderer.size();
        match script.draw(size.x, size.y) {
            Ok(commands) => render_hud(renderer, &commands, self.textures[0], &self.textures),
            Err(e) => tracing::error!("modal draw failed: {e}"),
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Unified Settings Scene
// ───────────────────────────────────────────────────────────────────

/// A full-screen settings overlay driven by `scripts/settings.lua`.
/// Replaces the old `SettingsPanel` (display dropdowns) and
/// `InputSettingsPanel` (tabbed input config). Returns a
/// [`SettingsResult`] when popped so the calling scene can apply changes.
struct UnifiedSettingsScene {
    theme: Theme,
    script: Option<ScriptHost>,
    textures: Vec<TextureHandle>,
    rebind: RebindCapture,
    /// Local copy of settings (edits buffered here, persisted on apply).
    settings: GameSettings,
    /// Current input map (mutated by rebinds).
    input_map: InputMap,
    /// Last mouse position for render-time model.
    last_cursor: Vec2,
    /// Last mouse down state for render-time model.
    last_down: bool,
    /// Last scroll wheel delta for render-time model.
    last_scroll: f32,
    /// Previous-frame Escape state, for edge-detecting Esc-to-close / cancel-rebind.
    esc_prev: bool,
}

impl UnifiedSettingsScene {
    fn new(theme: Theme, input_map: &InputMap) -> Self {
        let (script, textures) = load_ui_script(SETTINGS_SCRIPT, "settings.lua", &theme);
        let settings = GAME_SETTINGS.lock().expect("settings lock").clone();
        Self {
            theme,
            script,
            textures,
            rebind: RebindCapture::new(),
            settings,
            input_map: input_map.clone(),
            last_cursor: Vec2::ZERO,
            last_down: false,
            last_scroll: 0.0,
            esc_prev: false,
        }
    }

    /// Build the per-frame Model for the settings script.
    fn model(&self, sw: f32, sh: f32) -> ValueMap {
        let mut m = ValueMap::new()
            .with("mx", self.last_cursor.x as f64)
            .with("my", self.last_cursor.y as f64)
            .with("clicked", false)
            .with("down", self.last_down)
            .with("sw", sw as f64)
            .with("sh", sh as f64)
            .with("scroll", self.last_scroll as f64);

        // Audio settings (flat keys)
        m.set("audio_master", self.settings.audio.master as f64);
        m.set("audio_music", self.settings.audio.music as f64);
        m.set("audio_sfx", self.settings.audio.sfx as f64);
        m.set("audio_voice", self.settings.audio.voice as f64);

        // Video settings (flat keys)
        m.set("video_display_mode", self.settings.video.display_mode as f64);
        m.set("video_resolution", self.settings.video.resolution as f64);
        m.set("video_quality", self.settings.video.quality as f64);
        m.set("video_vsync", self.settings.video.vsync);
        m.set("video_fps_limit", self.settings.video.fps_limit as f64);

        // Input mouse settings (flat keys)
        m.set("input_mouse_sensitivity", self.settings.input.mouse_sensitivity as f64);
        m.set("input_mouse_sprint_sensitivity", self.settings.input.sprint_sensitivity as f64);
        m.set("input_mouse_invert_pitch", self.settings.input.invert_pitch);
        m.set("input_mouse_invert_yaw", self.settings.input.invert_yaw);
        m.set("input_mouse_raw_input", self.settings.input.raw_input);

        // Input controller settings (flat keys)
        m.set("input_ctrl_stick_sensitivity", self.settings.input.stick_sensitivity as f64);
        m.set("input_ctrl_left_deadzone", self.settings.input.left_deadzone as f64);
        m.set("input_ctrl_right_deadzone", self.settings.input.right_deadzone as f64);
        m.set("input_ctrl_trigger_threshold", self.settings.input.trigger_threshold as f64);
        m.set("input_ctrl_invert_stick_pitch", self.settings.input.invert_stick_pitch);
        m.set("input_ctrl_invert_stick_yaw", self.settings.input.invert_stick_yaw);
        m.set("input_ctrl_deadzone_shape", self.settings.input.deadzone_shape as f64);

        // Current keyboard bindings → the settings key caps show real keys
        // (`bind_<ActionId>`), instead of always "unbound". First binding wins.
        for (id, action) in KEYBOARD_ACTIONS {
            let label = self
                .input_map
                .bindings_for(*action)
                .first()
                .map(|b| b.to_string())
                .unwrap_or_default();
            m.set(format!("bind_{id}"), label);
        }

        // Rebind state
        if self.rebind.is_active() {
            if let Some(action) = self.rebind.current_action() {
                m.set("rebind_action", format!("{action}"));
                m.set("rebind_gamepad", self.rebind.is_gamepad());
            }
        }

        m
    }
}

impl Scene for UnifiedSettingsScene {
    fn is_overlay(&self) -> bool {
        true
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let Some(script) = self.script.as_ref() else {
            return Transition::Pop;
        };

        let size = renderer.size();
        self.last_cursor = input.mouse_position;
        self.last_down = input.mouse_left;
        self.last_scroll = input.mouse_wheel_delta;
        let esc_edge = input.key_down(Key::Escape) && !self.esc_prev;
        self.esc_prev = input.key_down(Key::Escape);

        // Run Lua update
        let _ = script.set_model(&self.model(size.x, size.y));
        let results = script.update(input, size.x, size.y).unwrap_or_else(|e| {
            tracing::error!("settings update failed: {e}");
            ValueMap::new()
        });

        // ── Handle rebind (Esc or a click cancels; else capture the next input) ──
        if self.rebind.is_active() {
            if esc_edge || results.is_on("settings_rebind_cancel") {
                self.rebind.cancel();
            } else if let Some((action, binding)) = self.rebind.poll(input, &mut self.input_map) {
                tracing::info!("rebound {action} to {binding}");
            }
            return Transition::None;
        }

        // ── Restore defaults (reset the local buffer; persisted on Apply/Back) ──
        if results.is_on("settings_restore") {
            self.settings = GameSettings::default();
            self.input_map = InputMap::wasd_and_mouse();
        }

        // ── Apply: persist without closing (the script shows the flash) ──
        if results.is_on("settings_apply") {
            let mut gs = GAME_SETTINGS.lock().expect("settings lock");
            *gs = self.settings.clone();
            gs.save();
        }

        // ── Back / close (Back button, × chip, or Esc): persist and pop ──
        if esc_edge || results.is_on("settings_back") {
            {
                let mut gs = GAME_SETTINGS.lock().expect("settings lock");
                *gs = self.settings.clone();
                gs.save();
            }
            return Transition::Pop;
        }

        // ── Apply audio changes ──
        if let Some(v) = results.number("audio_master") {
            self.settings.audio.master = v as f32;
        }
        if let Some(v) = results.number("audio_music") {
            self.settings.audio.music = v as f32;
        }
        if let Some(v) = results.number("audio_sfx") {
            self.settings.audio.sfx = v as f32;
        }
        if let Some(v) = results.number("audio_voice") {
            self.settings.audio.voice = v as f32;
        }

        // ── Apply video changes ──
        if let Some(v) = results.number("video_display_mode") {
            let new_mode = v as usize;
            if new_mode != self.settings.video.display_mode {
                self.settings.video.display_mode = new_mode;
                let mode = display::DisplayMode::ALL[new_mode.min(2)];
                let change = DisplayChange::Mode(mode);
                if let Some(prev) = apply_display_change(change, renderer) {
                    return Transition::Push(Box::new(ConfirmDisplayScene::new(self.theme, prev)));
                }
            }
        }
        if let Some(v) = results.number("video_resolution") {
            self.settings.video.resolution = v as usize;
        }
        if let Some(v) = results.number("video_quality") {
            self.settings.video.quality = v as usize;
        }
        if results.is_on("video_vsync") {
            self.settings.video.vsync = true;
        } else if let Some(flicker::script::Value::Bool(b)) = results.get("video_vsync") {
            self.settings.video.vsync = *b;
        }
        if let Some(v) = results.number("video_fps_limit") {
            self.settings.video.fps_limit = v as usize;
        }

        // ── Apply input mouse changes ──
        if let Some(v) = results.number("input_mouse_sensitivity") {
            self.settings.input.mouse_sensitivity = v as f32;
        }
        if let Some(v) = results.number("input_mouse_sprint_sensitivity") {
            self.settings.input.sprint_sensitivity = v as f32;
        }
        if let Some(flicker::script::Value::Bool(b)) = results.get("input_mouse_invert_pitch") {
            self.settings.input.invert_pitch = *b;
        }
        if let Some(flicker::script::Value::Bool(b)) = results.get("input_mouse_invert_yaw") {
            self.settings.input.invert_yaw = *b;
        }
        if let Some(flicker::script::Value::Bool(b)) = results.get("input_mouse_raw_input") {
            self.settings.input.raw_input = *b;
        }

        // ── Apply input controller changes ──
        if let Some(v) = results.number("input_controller_stick_sensitivity") {
            self.settings.input.stick_sensitivity = v as f32;
        }
        if let Some(v) = results.number("input_controller_left_deadzone") {
            self.settings.input.left_deadzone = v as f32;
        }
        if let Some(v) = results.number("input_controller_right_deadzone") {
            self.settings.input.right_deadzone = v as f32;
        }
        if let Some(v) = results.number("input_controller_trigger_threshold") {
            self.settings.input.trigger_threshold = v as f32;
        }
        if let Some(flicker::script::Value::Bool(b)) =
            results.get("input_controller_invert_stick_pitch")
        {
            self.settings.input.invert_stick_pitch = *b;
        }
        if let Some(flicker::script::Value::Bool(b)) =
            results.get("input_controller_invert_stick_yaw")
        {
            self.settings.input.invert_stick_yaw = *b;
        }
        if let Some(v) = results.number("input_controller_deadzone_shape") {
            self.settings.input.deadzone_shape = v as usize;
        }

        // ── Start rebind ──
        if results.is_on("settings_rebind_active") {
            if let Some(action_str) = results.text("settings_rebind_action") {
                let for_gamepad = results.is_on("settings_rebind_gamepad");
                if let Some(action) = parse_action(action_str) {
                    self.rebind.start(action, for_gamepad);
                }
            }
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(script) = self.script.as_ref() else {
            return;
        };

        let size = renderer.size();
        let _ = script.set_model(&self.model(size.x, size.y));

        match script.draw(size.x, size.y) {
            Ok(commands) => render_hud(renderer, &commands, self.textures[0], &self.textures),
            Err(e) => tracing::error!("settings draw failed: {e}"),
        }
    }
}

/// The keyboard actions the settings screen lists, in display order — the id
/// strings match `ui_elements.json`'s `settings.input.keyboard` groups (and
/// [`parse_action`]). Used to publish each action's current binding to the script.
const KEYBOARD_ACTIONS: &[(&str, Action)] = &[
    ("MoveForward", Action::MoveForward),
    ("MoveBackward", Action::MoveBackward),
    ("StrafeLeft", Action::StrafeLeft),
    ("StrafeRight", Action::StrafeRight),
    ("MoveUp", Action::MoveUp),
    ("MoveDown", Action::MoveDown),
    ("Jump", Action::Jump),
    ("Sprint", Action::Sprint),
    ("Crouch", Action::Crouch),
    ("Interact", Action::Interact),
    ("Inventory", Action::Inventory),
    ("Map", Action::Map),
    ("Menu", Action::Menu),
    ("PrimaryAction", Action::PrimaryAction),
    ("SecondaryAction", Action::SecondaryAction),
    ("Reload", Action::Reload),
    ("Confirm", Action::Confirm),
    ("Cancel", Action::Cancel),
    ("Quit", Action::Quit),
];

/// Parse an action string back into the `Action` enum.
fn parse_action(s: &str) -> Option<Action> {
    match s {
        "MoveForward" => Some(Action::MoveForward),
        "MoveBackward" => Some(Action::MoveBackward),
        "StrafeLeft" => Some(Action::StrafeLeft),
        "StrafeRight" => Some(Action::StrafeRight),
        "MoveUp" => Some(Action::MoveUp),
        "MoveDown" => Some(Action::MoveDown),
        "Jump" => Some(Action::Jump),
        "Sprint" => Some(Action::Sprint),
        "Crouch" => Some(Action::Crouch),
        "Interact" => Some(Action::Interact),
        "Reload" => Some(Action::Reload),
        "PrimaryAction" => Some(Action::PrimaryAction),
        "SecondaryAction" => Some(Action::SecondaryAction),
        "Confirm" => Some(Action::Confirm),
        "Cancel" => Some(Action::Cancel),
        "Menu" => Some(Action::Menu),
        "Inventory" => Some(Action::Inventory),
        "Map" => Some(Action::Map),
        "Quit" => Some(Action::Quit),
        "LookUp" => Some(Action::LookUp),
        "LookDown" => Some(Action::LookDown),
        "LookLeft" => Some(Action::LookLeft),
        "LookRight" => Some(Action::LookRight),
        _ => None,
    }
}

/// Main menu: a thin shell over the shared modal (`screen = "menu"`). The script
/// owns layout/labels/hit-testing; this scene routes the `start`/`settings`/`quit`
/// actions to transitions.
struct MenuScene {
    theme: Option<Theme>,
    modal: Option<ModalUi>,
    /// Pending input map changes from the settings overlay.
    pending_input: Option<InputMap>,
    /// The client's game-scene factory; consumed when START launches the game.
    game_scene: Option<GameSceneFactory>,
    /// Optional label for the game-launch button (see [`ShellConfig::game_label`]).
    game_label: Option<String>,
}

impl MenuScene {
    fn new(game_scene: GameSceneFactory, game_label: Option<String>) -> Self {
        Self {
            theme: None,
            modal: None,
            pending_input: None,
            game_scene: Some(game_scene),
            game_label,
        }
    }
}

impl Scene for MenuScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        let theme = Theme::build(renderer);
        self.modal = Some(ModalUi::new(&theme, self.game_label.clone()));
        self.theme = Some(theme);
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // The shared modal hit-tests the buttons and fires momentary actions.
        if let Some(modal) = self.modal.as_ref() {
            let actions = modal.update(input, renderer, "menu", None);
            if actions.is_on("start") {
                return Transition::Replace(
                    (self.game_scene.take().expect("game factory present until START"))(),
                );
            }
            if actions.is_on("settings") {
                let theme = self.theme.expect("theme built in enter");
                // Default to WASD so the settings key caps show real keys from the
                // menu (before any game bindings exist).
                let input_map = self
                    .pending_input
                    .take()
                    .unwrap_or_else(InputMap::wasd_and_mouse);
                return Transition::Push(Box::new(UnifiedSettingsScene::new(theme, &input_map)));
            }
            if actions.is_on("quit") {
                return Transition::Quit;
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(modal) = self.modal.as_ref() {
            modal.render(renderer, "menu", None);
        }
    }
}

/// Pause overlay pushed over the frozen game. Resume (or Escape) pops back to
/// the game; Quit exits. Reuses the game's already-uploaded [`Theme`].
///
/// The "SETTINGS" button opens the unified settings overlay (Audio/Video/Input).
/// On close, buffered input changes are pushed to the game scene via
/// [`INPUT_SETTINGS`].
pub struct PauseScene {
    theme: Theme,
    modal: ModalUi,
    bindings: InputMap,
    menu_prev: bool,
}

impl PauseScene {
    /// Build the pause overlay from the client's current [`Theme`] and input
    /// config. The client pushes this (via [`flicker_scene::Transition::Push`])
    /// when the player opens the pause menu.
    ///
    /// [`flicker_scene::Transition::Push`]: flicker::scene::Transition
    pub fn new(
        theme: Theme,
        input_map: &InputMap,
        _controls: &AbstractControls,
        _gamepad_config: &GamepadConfig,
    ) -> Self {
        Self {
            modal: ModalUi::new(&theme, None),
            theme,
            bindings: input_map.clone(),
            menu_prev: true,
        }
    }
}

impl Scene for PauseScene {
    fn is_overlay(&self) -> bool {
        true
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // ── Menu action: resume ──
        let menu_down = self.bindings.action_pressed(Action::Menu, input);
        let menu_pressed = menu_down && !self.menu_prev;
        self.menu_prev = menu_down;
        if menu_pressed {
            return Transition::Pop;
        }

        // ── Modal buttons ──
        let actions = self.modal.update(input, renderer, "pause", None);
        if actions.is_on("resume") {
            return Transition::Pop;
        }
        if actions.is_on("settings") {
            return Transition::Push(Box::new(UnifiedSettingsScene::new(
                self.theme,
                &self.bindings,
            )));
        }
        if actions.is_on("quit") {
            return Transition::Quit;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        self.modal.render(renderer, "pause", None);
    }
}

#[cfg(test)]
mod script_smoke {
    //! Load the *embedded* shell scripts and run a frame against a representative
    //! model, so a Lua syntax/runtime error — or a `ui_elements.json` key a script
    //! reads but the embedded layout lacks — fails the build instead of only
    //! surfacing in the running app. The build-time check that keeps the shell's
    //! Rust↔Lua contract honest now that the scripts + layout live in this crate.
    use super::*;

    #[test]
    fn modal_script_runs_every_screen() {
        let host = ScriptHost::new(MODAL_SCRIPT, "modal.lua").expect("load modal.lua");
        host.set_texture_ids(&[
            ("white", 0),
            ("panel", 1),
            ("settings_panel", 2),
            ("button", 3),
            ("muse", 4),
        ])
        .expect("register textures");
        expose_ui_elements(&host); // the embedded shell ui_elements.json
        let input = InputState::new();
        for screen in ["menu", "pause", "confirm"] {
            let mut model = ValueMap::new().with("screen", screen);
            if screen == "confirm" {
                model.set("subtitle", "Reverting in 9s");
            }
            host.set_model(&model).expect("publish screen");
            host.update(&input, 1920.0, 1080.0).expect("modal update");
            let cmds = host.draw(1920.0, 1080.0).expect("modal draw");
            assert!(
                !cmds.is_empty(),
                "modal screen '{screen}' emits overlay + panel + button commands"
            );
        }
    }

    #[test]
    fn logo_script_runs() {
        let host = ScriptHost::new(LOGO_SCRIPT, "logo.lua").expect("load logo.lua");
        host.set_texture_ids(&[("white", 0), ("elideus", 1), ("clay", 2)])
            .expect("register textures");
        expose_ui_elements(&host);
        let sizes = |elapsed: f32| {
            ValueMap::new()
                .with("elapsed", elapsed)
                .with("img1_w", 1920u32)
                .with("img1_h", 1080u32)
                .with("img2_w", 1672u32)
                .with("img2_h", 941u32)
        };
        let input = InputState::new();
        host.set_model(&sizes(0.3)).expect("publish model");
        let out = host.update(&input, 1920.0, 1080.0).expect("logo update");
        assert!(!out.is_on("done"), "sequence still playing at t=0.3");
        assert!(
            host.draw(1920.0, 1080.0).expect("logo draw").len() >= 2,
            "logo emits backdrop + first image"
        );
        host.set_model(&sizes(99.0)).expect("publish model");
        let out = host.update(&input, 1920.0, 1080.0).expect("logo update done");
        assert!(out.is_on("done"), "sequence done after it plays out");
    }

    #[test]
    fn settings_script_runs_all_sections() {
        use flicker::render::Vec2;
        use flicker::script::HudCommand;

        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua").expect("load settings.lua");
        host.set_texture_ids(&[
            ("white", 0),
            ("panel", 1),
            ("settings_panel", 2),
            ("button", 3),
            ("muse", 4),
        ])
        .expect("register textures");
        expose_ui_elements(&host);
        load_widgets(&host); // the workbench uses Widgets.* for hit-testing

        let (sw, sh) = (1920.0_f32, 1080.0_f32);
        let model = || {
            ValueMap::new()
                .with("scroll", 0.0)
                .with("video_display_mode", 0.0)
                .with("video_resolution", 2.0)
                .with("video_quality", 2.0)
                .with("video_vsync", true)
                .with("video_fps_limit", 1.0)
                .with("input_mouse_sensitivity", 0.005)
                .with("input_mouse_invert_pitch", false)
                .with("bind_MoveForward", "W")
        };
        let click = |x: f32, y: f32| {
            let mut i = InputState::new();
            i.mouse_position = Vec2::new(x, y);
            i.mouse_left_pressed = true;
            i
        };
        let has_text = |cmds: &[HudCommand], s: &str| {
            cmds.iter()
                .any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
        };
        // Run a frame's update+draw and assert a marker string rendered.
        let frame = |input: &InputState, marker: &str| {
            host.set_model(&model()).expect("model");
            host.update(input, sw, sh).expect("settings update");
            let cmds = host.draw(sw, sh).expect("settings draw");
            assert!(
                has_text(&cmds, marker),
                "settings screen did not render '{marker}'"
            );
        };

        // Nav rail + sub-tab pixel positions for a 1920×1080 window (see layout()).
        frame(&InputState::new(), "Video"); // default section
        frame(&click(488.0, 335.0), "NOT YET IMPLEMENTED"); // Audio nav → stub
        frame(&click(488.0, 382.0), "MOVEMENT"); // Input nav → keyboard bindings
        frame(&click(1459.0, 274.0), "No controller detected"); // Controller pill
        frame(&click(1351.0, 274.0), "Look Sensitivity"); // Mouse pill
    }
}
