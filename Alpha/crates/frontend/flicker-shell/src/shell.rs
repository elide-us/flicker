//! Front-end shell scenes + the settings/config model (private to the crate).
//! Only [`run`], [`ShellConfig`], [`PauseScene`], and [`take_pending_input`] are
//! public (re-exported from the crate root); everything else — the splash/menu/
//! settings/pause scenes, their embedded Lua scripts + `ui_elements.json`, and
//! display/settings persistence — is internal.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use flicker::app::{
    run as run_app, AbstractControls, Action, GamepadConfig, InputMap, InputState, Key,
    RebindCapture,
};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    load_styles_str, load_ui_json_str, load_widgets, render_hud, run_ui, UiInput, UiState,
};

use crate::display;
use crate::theme::Theme;

/// A factory that builds one of the client's scenes when its menu button is hit.
/// `Rc` (not `Box`) so the menu — and a "return to main menu" rebuild — can hold
/// the same factory set any number of times. The shell never names a scene type.
pub type SceneFactory = Rc<dyn Fn() -> Box<dyn Scene>>;

/// Rich display metadata for the scene-selection panel (the launcher's scene
/// picker). A plain launch button (a single-scene client) needs only the
/// [`SceneEntry`] label; a panel row shows all of these.
#[derive(Clone)]
pub struct SceneInfo {
    /// The scene's display name (the row title).
    pub name: String,
    /// Play-mode / category tag, e.g. "Adventurer", "Commander", "Tool".
    pub mode: String,
    /// Short region / kind label, e.g. "Rigging POC".
    pub region: String,
    /// One-line italic description.
    pub desc: String,
    /// Small meta line (build / type / counts).
    pub meta: String,
}

impl SceneInfo {
    pub fn new(
        name: impl Into<String>,
        mode: impl Into<String>,
        region: impl Into<String>,
        desc: impl Into<String>,
        meta: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            mode: mode.into(),
            region: region.into(),
            desc: desc.into(),
            meta: meta.into(),
        }
    }
}

/// One launchable scene: a stable action `id`, its display `label`, its Prism style
/// `variant` (`primary`/`secondary`/`danger`), and the `factory` that builds it.
/// In the default menu it is one launch button; in a launcher (`scene_select`) it is
/// one scene-panel row, using its optional [`SceneInfo`]. On click the menu replaces
/// itself with `factory()`.
pub struct SceneEntry {
    pub id: String,
    pub label: String,
    pub variant: String,
    pub factory: SceneFactory,
    /// Rich metadata for the scene-selection panel; `None` = a plain launch button.
    pub info: Option<SceneInfo>,
}

impl SceneEntry {
    /// Build an entry (a plain launch button), wrapping `factory` in the shared `Rc`.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        variant: impl Into<String>,
        factory: impl Fn() -> Box<dyn Scene> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: variant.into(),
            factory: Rc::new(factory),
            info: None,
        }
    }

    /// Attach scene-selection-panel metadata (the launcher's rich row).
    pub fn with_info(mut self, info: SceneInfo) -> Self {
        self.info = Some(info);
        self
    }
}

/// The default launch-button label for a single-scene app (was the `start` item's
/// label in `ui_elements.json`; owned here now that the menu is data-driven).
const DEFAULT_LAUNCH_LABEL: &str = "ENTER WORLD";

/// What a client hands [`run`]: the scenes its menu launches, and where per-user
/// settings live. A single-scene client uses [`ShellConfig::single`]; a multi-scene
/// host (e.g. paperdoll → viewer + click-trainer) fills `scenes` directly.
pub struct ShellConfig {
    /// The launchable scenes, in menu order — each becomes a menu button (or, in a
    /// launcher, a scene-panel row).
    pub scenes: Vec<SceneEntry>,
    /// The app's project root, where the per-user `settings.json` (display mode/
    /// resolution, keybindings, audio) is read/written — usually
    /// `env!("CARGO_MANIFEST_DIR").into()`. `None` falls back to the cwd.
    pub settings_dir: Option<std::path::PathBuf>,
    /// Render the scenes as the right-hand SELECTION PANEL (a launcher's scene
    /// picker, using each entry's [`SceneInfo`]) instead of as launch buttons in the
    /// menu popup. `false` (the default via [`single`](ShellConfig::single)) keeps the
    /// plain-button menu every single-scene client uses.
    pub scene_select: bool,
}

impl ShellConfig {
    /// The common single-scene case: one launch button (`start`) whose `label`
    /// defaults to "ENTER WORLD" (`None`) — a client passes `Some(..)` to name its mode.
    pub fn single(
        settings_dir: Option<std::path::PathBuf>,
        label: Option<String>,
        factory: impl Fn() -> Box<dyn Scene> + 'static,
    ) -> Self {
        let label = label.unwrap_or_else(|| DEFAULT_LAUNCH_LABEL.to_string());
        Self {
            scenes: vec![SceneEntry::new("start", label, "primary", factory)],
            settings_dir,
            scene_select: false,
        }
    }
}

thread_local! {
    /// The launchable scenes, shared by the menu and — for "return to main menu" —
    /// the pause overlay, so either can (re)build the menu without threading the
    /// factory set through every scene. Set once by [`run`]. `thread_local` because
    /// the whole shell runs on the winit thread and `SceneFactory` is `Rc` (not `Send`).
    static SCENES: RefCell<Rc<[SceneEntry]>> = RefCell::new(Rc::from(Vec::<SceneEntry>::new()));
    /// Whether the menu shows the scene-selection PANEL (a launcher) vs plain launch
    /// buttons. Set once by [`run`], read when the menu is (re)built.
    static SCENE_SELECT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn set_scenes(scenes: Vec<SceneEntry>) {
    SCENES.with(|s| *s.borrow_mut() = Rc::from(scenes));
}

/// A cheap clone of the shared scene registry (the `Rc` is shared, not the data).
fn scenes() -> Rc<[SceneEntry]> {
    SCENES.with(|s| s.borrow().clone())
}

/// Whether this client is a launcher (scenes → selection panel).
fn scene_select() -> bool {
    SCENE_SELECT.with(|s| s.get())
}

/// Restore the persisted display setting, install the scene registry, then run the
/// whole front-end flow — intro splash → menu → *a client scene* → pause/settings —
/// on the winit loop. Blocks until the window closes. The one entry point a client calls.
pub fn run(config: ShellConfig) -> anyhow::Result<()> {
    display::set_settings_dir(config.settings_dir.clone());
    display::load_from_disk();
    SCENE_SELECT.with(|s| s.set(config.scene_select));
    set_scenes(config.scenes);
    run_app(SceneManager::new(Box::new(LogoScene::new())))
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
}

impl LogoScene {
    fn new() -> Self {
        Self {
            script: None,
            textures: Vec::new(),
            sizes: Vec::new(),
            elapsed: Duration::ZERO,
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
            return Transition::Replace(Box::new(MenuScene::new()));
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
    view: MenuView,
    previous: display::DisplaySetting,
    remaining: f32,
}

impl ConfirmDisplayScene {
    fn new(theme: Theme, previous: display::DisplaySetting) -> Self {
        Self {
            view: MenuView::new(&theme, "confirm", &confirm_items(), &[]),
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
        // The `confirm` screen's flat overlay keeps the new resolution visible
        // behind the dialog; the live countdown rides the Model (`subtitle` bind).
        let model = ValueMap::new().with("subtitle", self.subtitle());
        let actions = self.view.update(input, renderer, &model);
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
        self.view.render(renderer);
    }
}

/// The unified settings Lua script, embedded so clients inherit it.
const SETTINGS_SCRIPT: &str = include_str!("../../../../content/scripts/settings.lua");

/// The shared front-end modal, now a DECLARATIVE component tree (menu / pause /
/// confirm, selected by the published `MENU.screen`), embedded. Rendered by the
/// Rust component walker (`run_ui`); replaces the retired `modal.lua`.
const MENU_SCRIPT: &str = include_str!("../../../../content/scripts/menu.lua");

/// The shell's declarative UI layout (`modal`/`screens`/`settings`/`logo`/
/// `loading`), embedded. The client's in-game HUD layout is separate.
const SHELL_UI_JSON: &str = include_str!("../../../../content/resources/ui_elements.json");

// The composable vector-UI component library (`content/scripts/ui/`): a shared
// `core` + one component per file + the `layout` engine, registered as
// requireable Lua modules via `ScriptHost::new_with_modules`. (Foundation slice —
// screens migrate onto these next.)
#[cfg(test)]
const UI_CORE: &str = include_str!("../../../../content/scripts/ui/core.lua");
#[cfg(test)]
const UI_BUTTON: &str = include_str!("../../../../content/scripts/ui/button.lua");
#[cfg(test)]
const UI_LAYOUT: &str = include_str!("../../../../content/scripts/ui/layout.lua");

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

/// One item published to `menu.lua`'s data-driven button list: a stable action
/// `id`, its display `label`, and its Prism style `variant`. The engine reads back
/// `results.is_on(id)` to dispatch — the buttons are pure data.
struct MenuItem {
    id: String,
    label: String,
    variant: String,
}

impl MenuItem {
    fn new(id: impl Into<String>, label: impl Into<String>, variant: &str) -> Self {
        Self { id: id.into(), label: label.into(), variant: variant.to_string() }
    }
}

/// One scene-selection-panel row published to the launcher menu (the rich form of
/// a `SceneEntry` with `SceneInfo`). Its LOAD button fires `id`, the same action id
/// the popup buttons would.
struct SceneRow {
    id: String,
    name: String,
    mode: String,
    region: String,
    desc: String,
    meta: String,
}

/// Publish the screen + its button list (+ optional scene-panel rows) to a menu
/// script as the `MENU` data global (nested JSON — variable-length *structure*, so
/// it rides this channel, not the flat `Model`). `menu.lua`'s `tree()` loops
/// `MENU.items` (popup buttons) and `MENU.scenes` (panel rows); `scene_select` gates
/// the two-column launcher layout.
fn publish_menu(script: &ScriptHost, screen: &str, items: &[MenuItem], scenes: &[SceneRow]) {
    let items_arr: Vec<serde_json::Value> = items
        .iter()
        .map(|it| serde_json::json!({ "id": it.id, "label": it.label, "variant": it.variant }))
        .collect();
    let scenes_arr: Vec<serde_json::Value> = scenes
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id, "name": s.name, "mode": s.mode,
                "region": s.region, "desc": s.desc, "meta": s.meta,
            })
        })
        .collect();
    let menu = serde_json::json!({
        "screen": screen,
        "scene_select": !scenes.is_empty(),
        "items": items_arr,
        "scenes": scenes_arr,
    });
    if let Err(e) = script.set_global_json("MENU", &menu) {
        tracing::error!("MENU global publish failed: {e}");
    }
}

/// The menu's standard trailing chrome buttons (after the launchable scenes).
fn menu_chrome_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("settings", "SETTINGS", "secondary"),
        MenuItem::new("quit", "QUIT", "danger"),
    ]
}

/// The pause overlay's buttons — resume, settings, return to the main menu, quit.
fn pause_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("resume", "RETURN TO WORLD", "primary"),
        MenuItem::new("settings", "SETTINGS", "secondary"),
        MenuItem::new("main_menu", "MAIN MENU", "secondary"),
        MenuItem::new("quit", "QUIT", "danger"),
    ]
}

/// The display-confirm dialog's buttons.
fn confirm_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("keep", "KEEP", "primary"),
        MenuItem::new("revert", "REVERT", "danger"),
    ]
}

/// The walker-rendered front-end modal (menu / pause / confirm) — the shared
/// machinery behind every gothic-modal scene, replacing the legacy `ModalUi`.
/// Loads the shared `menu.lua` component tree, publishes its screen + button list
/// as the `MENU` data global, builds the tree ONCE, then each frame runs the Rust
/// component walker (`run_ui`) → draw commands + fired actions. The SAME control
/// for every screen and every app; only the published items differ.
struct MenuView {
    textures: Vec<TextureHandle>,
    tree: Option<UiNode>,
    styles: serde_json::Value,
    ui_state: UiState,
    commands: Vec<HudCommand>,
}

impl MenuView {
    /// Load `menu.lua`, register the theme textures (so `Textures.muse` resolves),
    /// expose the shell layout + styles, publish the `screen` + `items`, and build
    /// the component tree ONCE — the parsed `UiNode` is fully owned, so the script
    /// host is dropped after. Best-effort: a failure leaves a view that draws nothing.
    fn new(theme: &Theme, screen: &str, items: &[MenuItem], scenes: &[SceneRow]) -> Self {
        let entries = theme.lua_textures();
        let textures: Vec<TextureHandle> = entries.iter().map(|(_, h)| *h).collect();
        let styles = load_styles_str(SHELL_UI_JSON);
        let tree = match ScriptHost::new(MENU_SCRIPT, "menu.lua") {
            Ok(s) => {
                let ids: Vec<(&str, u32)> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| (*name, i as u32))
                    .collect();
                if let Err(e) = s.set_texture_ids(&ids) {
                    tracing::error!("menu texture registration failed: {e}");
                }
                expose_ui_elements(&s); // the `UI` global (chrome config + styles)
                load_widgets(&s); // parity with the other shell screens
                publish_menu(&s, screen, items, scenes); // the `MENU` data global
                match s.ui_tree() {
                    Ok(Some(t)) => Some(t),
                    Ok(None) => {
                        tracing::error!("menu.lua exposes no tree()");
                        None
                    }
                    Err(e) => {
                        tracing::error!("menu tree build failed: {e}");
                        None
                    }
                }
            }
            Err(e) => {
                tracing::error!("menu.lua load failed: {e}");
                None
            }
        };
        Self {
            textures,
            tree,
            styles,
            ui_state: UiState::new(),
            commands: Vec::new(),
        }
    }

    /// Walk the cached tree for one frame. `model` carries any per-frame binds (the
    /// confirm countdown's `subtitle`). Stashes the draw commands and returns the
    /// fired actions (`is_on("start")` / `is_on("main_menu")` …).
    fn update(&mut self, input: &InputState, renderer: &Renderer, model: &ValueMap) -> ValueMap {
        let Some(tree) = self.tree.as_ref() else {
            return ValueMap::new();
        };
        let size = renderer.size();
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen: size,
        };
        let frame = run_ui(tree, model, &self.styles, &snap, &mut self.ui_state);
        self.commands = frame.commands;
        frame.results
    }

    /// Blit the stashed commands (`textures[0]` is the 1×1 white for rect fills).
    fn render(&self, renderer: &mut Renderer) {
        if let Some(&white) = self.textures.first() {
            render_hud(renderer, &self.commands, white, &self.textures);
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

/// Main menu: a thin shell over the shared [`MenuView`] (`screen = "menu"`). The
/// walker owns layout/hit-testing; this scene builds the button list from the scene
/// registry and routes each launch action + `settings`/`quit` to a transition.
struct MenuScene {
    theme: Option<Theme>,
    view: Option<MenuView>,
    /// Pending input map changes from the settings overlay.
    pending_input: Option<InputMap>,
    /// The launchable scenes (the menu's launch buttons), from the shell registry.
    scenes: Rc<[SceneEntry]>,
}

impl MenuScene {
    fn new() -> Self {
        Self {
            theme: None,
            view: None,
            pending_input: None,
            scenes: scenes(),
        }
    }

    /// The popup buttons. Default menu: one launch button per scene + SETTINGS/QUIT.
    /// Launcher (`scene_select`): the scenes move to the right panel, so the popup is
    /// just the SETTINGS/QUIT chrome.
    fn items(&self) -> Vec<MenuItem> {
        if scene_select() {
            return menu_chrome_items();
        }
        let mut items: Vec<MenuItem> = self
            .scenes
            .iter()
            .map(|e| MenuItem::new(e.id.clone(), e.label.clone(), e.variant.as_str()))
            .collect();
        items.extend(menu_chrome_items());
        items
    }

    /// The scene-selection-panel rows — one per registered scene that carries
    /// `SceneInfo`. Empty unless this client is a launcher (`scene_select`), which is
    /// what gates the two-column layout.
    fn scene_rows(&self) -> Vec<SceneRow> {
        if !scene_select() {
            return Vec::new();
        }
        self.scenes
            .iter()
            .filter_map(|e| {
                let info = e.info.as_ref()?;
                Some(SceneRow {
                    id: e.id.clone(),
                    name: info.name.clone(),
                    mode: info.mode.clone(),
                    region: info.region.clone(),
                    desc: info.desc.clone(),
                    meta: info.meta.clone(),
                })
            })
            .collect()
    }
}

impl Scene for MenuScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        let theme = Theme::build(renderer);
        self.view = Some(MenuView::new(&theme, "menu", &self.items(), &self.scene_rows()));
        self.theme = Some(theme);
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let results = match self.view.as_mut() {
            Some(view) => view.update(input, renderer, &ValueMap::new()),
            None => return Transition::None,
        };
        // A launch button fired → replace the menu with that scene (the factory is
        // shared via `Rc`, so returning here any number of times is fine).
        for entry in self.scenes.iter() {
            if results.is_on(&entry.id) {
                return Transition::Replace((entry.factory)());
            }
        }
        if results.is_on("settings") {
            let theme = self.theme.expect("theme built in enter");
            // Default to WASD so the settings key caps show real keys from the
            // menu (before any game bindings exist).
            let input_map = self
                .pending_input
                .take()
                .unwrap_or_else(InputMap::wasd_and_mouse);
            return Transition::Push(Box::new(UnifiedSettingsScene::new(theme, &input_map)));
        }
        if results.is_on("quit") {
            return Transition::Quit;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(view) = self.view.as_ref() {
            view.render(renderer);
        }
    }
}

/// Pause overlay pushed over the frozen game. Resume (or Escape) pops back to
/// the game; Quit exits. Reuses the game's already-uploaded [`Theme`].
///
/// "SETTINGS" opens the unified settings overlay (Audio/Video/Input); on close,
/// buffered input changes reach the game scene via [`INPUT_SETTINGS`]. "MAIN MENU"
/// unwinds the whole scene stack (freeing the game) back to a fresh main menu.
pub struct PauseScene {
    theme: Theme,
    view: MenuView,
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
            view: MenuView::new(&theme, "pause", &pause_items(), &[]),
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
        let actions = self.view.update(input, renderer, &ValueMap::new());
        if actions.is_on("resume") {
            return Transition::Pop;
        }
        if actions.is_on("settings") {
            return Transition::Push(Box::new(UnifiedSettingsScene::new(
                self.theme,
                &self.bindings,
            )));
        }
        if actions.is_on("main_menu") {
            // Unwind the whole stack (freeing the frozen game scene) back to a
            // fresh menu, rebuilt from the shared scene registry.
            return Transition::ReplaceRoot(Box::new(MenuScene::new()));
        }
        if actions.is_on("quit") {
            return Transition::Quit;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        self.view.render(renderer);
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
    fn ui_component_library_composes() {
        // A demo screen: `require` the layout engine + the button component (each
        // its own file under content/scripts/ui/), lay two buttons out as a row,
        // and draw them. Proves the real component files compose end-to-end and
        // the layout engine resolves grow-sizing to pixels.
        const DEMO: &str = r#"
            local layout = require("ui.layout")
            local button = require("ui.button")
            local STYLE = { fill = {0.14,0.25,0.47,1}, radius = 4, border = 1,
              border_color = {0.23,0.35,0.63,1}, label_color = {0.9,0.9,0.85,1}, label_size = 14 }
            local TREE = { type = "row", gap = 10, pad = 8, children = {
              { type = "leaf", id = "OK", grow = 1 },
              { type = "leaf", id = "CANCEL", grow = 1 },
            } }
            local M = {}
            function M.update() return {} end
            function M.draw(sw, sh)
              local cmds = {}
              for _, leaf in ipairs(layout.resolve(TREE, { x = 0, y = 0, w = 200, h = 40 })) do
                button.draw(cmds, leaf.rect, { label = leaf.id, style = STYLE })
              end
              return cmds
            end
            return M
        "#;
        let host = ScriptHost::new_with_modules(
            DEMO,
            "ui-demo",
            &[
                ("ui.core", UI_CORE),
                ("ui.button", UI_BUTTON),
                ("ui.layout", UI_LAYOUT),
            ],
        )
        .expect("component modules load + require resolves");
        let cmds = host.draw(0.0, 0.0).expect("draw runs");
        let panels: Vec<_> = cmds
            .iter()
            .filter(|c| matches!(c, flicker::script::HudCommand::Panel { .. }))
            .collect();
        let texts = cmds
            .iter()
            .filter(|c| matches!(c, flicker::script::HudCommand::Text { .. }))
            .count();
        assert_eq!(panels.len(), 2, "two button panels");
        assert_eq!(texts, 2, "two button labels");
        // Layout: a 200px row, pad 8 → content x=8 w=184; gap 10; two grow=1 →
        // 87 each. So button 2's panel starts at x = 8 + 87 + 10 = 105.
        if let flicker::script::HudCommand::Panel { x, w, .. } = panels[0] {
            assert_eq!(*x, 8.0);
            assert_eq!(*w, 87.0);
        }
        if let flicker::script::HudCommand::Panel { x, .. } = panels[1] {
            assert_eq!(*x, 105.0);
        }
    }

    #[test]
    fn menu_tree_runs_every_screen() {
        // The shared `menu.lua` builds a component tree per screen from the published
        // `MENU` items; the Rust walker draws it. Proves the data-driven button list
        // AND the Rust↔Lua contract for all three shell screens at build time.
        use flicker::render::Vec2;
        use flicker::script::HudCommand;

        let styles = load_styles_str(SHELL_UI_JSON);
        let cases: [(&str, Vec<MenuItem>); 3] = [
            (
                "menu",
                vec![
                    MenuItem::new("start", "ENTER WORLD", "primary"),
                    MenuItem::new("clicktrainer", "CLICK TRAINER", "secondary"),
                    MenuItem::new("settings", "SETTINGS", "secondary"),
                    MenuItem::new("quit", "QUIT", "danger"),
                ],
            ),
            ("pause", pause_items()),
            ("confirm", confirm_items()),
        ];
        for (screen, items) in cases {
            let host = ScriptHost::new(MENU_SCRIPT, "menu.lua").expect("load menu.lua");
            host.set_texture_ids(&[
                ("white", 0),
                ("panel", 1),
                ("settings_panel", 2),
                ("button", 3),
                ("muse", 4),
            ])
            .expect("register textures");
            expose_ui_elements(&host);
            load_widgets(&host);
            publish_menu(&host, screen, &items, &[]);
            let tree = host
                .ui_tree()
                .expect("tree parses")
                .expect("menu.lua exposes tree()");
            let model = ValueMap::new().with("subtitle", "Reverting in 9s");
            let snap = UiInput {
                mouse: Vec2::new(-1.0, -1.0),
                clicked: false,
                down: false,
                screen: Vec2::new(1920.0, 1080.0),
            };
            let frame = run_ui(&tree, &model, &styles, &snap, &mut UiState::new());
            assert!(
                !frame.commands.is_empty(),
                "menu screen '{screen}' emits panel + buttons + text"
            );
            // Every published item's label renders as a button text command — the
            // data-driven list actually produced its buttons.
            for it in &items {
                assert!(
                    frame.commands.iter().any(
                        |c| matches!(c, HudCommand::Text { text, .. } if text == &it.label)
                    ),
                    "screen '{screen}' renders button label '{}'",
                    it.label
                );
            }
        }
    }

    #[test]
    fn menu_launcher_renders_a_row_per_scene() {
        // The launcher menu (`scene_select`, i.e. non-empty `MENU.scenes`) builds the
        // two-column layout: one scene row per published scene, each row's name drawn
        // and a LOAD button (the shared button template) firing the scene id.
        use flicker::render::Vec2;
        use flicker::script::HudCommand;

        let styles = load_styles_str(SHELL_UI_JSON);
        let host = ScriptHost::new(MENU_SCRIPT, "menu.lua").expect("load menu.lua");
        host.set_texture_ids(&[
            ("white", 0),
            ("panel", 1),
            ("settings_panel", 2),
            ("button", 3),
            ("muse", 4),
        ])
        .expect("register textures");
        expose_ui_elements(&host);
        load_widgets(&host);
        let items = menu_chrome_items(); // launcher popup = settings/quit only
        let scenes = vec![
            SceneRow {
                id: "solarbirth".into(),
                name: "Solar Birth".into(),
                mode: "Cinematic".into(),
                region: "Celestial".into(),
                desc: "A cinematic.".into(),
                meta: "Clay 0.1".into(),
            },
            SceneRow {
                id: "clicktrainer".into(),
                name: "Click Trainer".into(),
                mode: "Mini-Game".into(),
                region: "2D".into(),
                desc: "Aim drill.".into(),
                meta: "Clay 0.1".into(),
            },
        ];
        publish_menu(&host, "menu", &items, &scenes);
        let tree = host
            .ui_tree()
            .expect("tree parses")
            .expect("menu.lua exposes tree()");
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
        };
        let frame = run_ui(&tree, &ValueMap::new(), &styles, &snap, &mut UiState::new());
        for sc in &scenes {
            assert!(
                frame
                    .commands
                    .iter()
                    .any(|c| matches!(c, HudCommand::Text { text, .. } if text == &sc.name)),
                "launcher renders scene row '{}'",
                sc.name
            );
        }
        let loads = frame
            .commands
            .iter()
            .filter(|c| matches!(c, HudCommand::Text { text, .. } if text == "LOAD"))
            .count();
        assert_eq!(loads, scenes.len(), "one LOAD button per scene");
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
