//! Front-end shell scenes + the settings/config model (private to the crate).
//! Only [`run`], [`ShellConfig`], [`PauseScene`], and [`take_pending_input`] are
//! public (re-exported from the crate root); everything else — the splash/menu/
//! settings/pause scenes, their embedded Lua scripts + `ui_elements.json`, and
//! display/settings persistence — is internal.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use flicker::app::run as run_app;
use flicker::render::{Renderer, TextureHandle};
use flicker::scene::{GotoMode, Scene, SceneInput, SceneManager, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    builtin_templates, expand, focusables_of, load_styles_str, load_ui_json_str, render_hud,
    run_ui, SceneDef, SceneManifest, Surface, Surfaces, UiInput, UiIntents, UiState,
    WalkerHandler,
};
use flicker_input_core::{
    AbstractControls, ActionSignal, ContextualBindings, Fired, GamepadConfig, InputMap,
    InputProfile, InputState, Key, RebindCapture, Resolver,
};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};

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

// ── Play-mode realms (the launcher's tier-1 → tier-2 map) ──────────────────
// A launcher's ROOT menu lists the three Prism play modes; each opens a tier-2
// page listing the scenes MEMBER of that realm. Membership is `SceneEntry::realms`
// (a tag list, so a tool can be shared across modes); `SceneInfo::mode` stays a
// pure display string. A realm-less entry (e.g. Click Trainer) stays a root-level
// launch button.

/// Adventurer mode ("Explore the World") — the player-facing tier-2 page.
pub const REALM_ADVENTURER: &str = "adventurer";
/// DM mode ("Build the World") — under construction; its tier-2 page is a note.
pub const REALM_DM: &str = "dm";
/// **GAME MASTER** — where the WORLD itself is authored: the planet simulation
/// and the benches that shape a map before anyone plays on it. Distinct from
/// [`REALM_DM`], which is about building an adventure inside a world that
/// already exists, and from [`REALM_DEVELOPER`], which is engine tooling.
pub const REALM_GAMEMASTER: &str = "gamemaster";
/// Developer mode — the scene-select launcher (benches / tools / POCs) as tier 2.
pub const REALM_DEVELOPER: &str = "developer";
/// The launcher root's mode tiers, in display order.
const REALMS: [&str; 4] =
    [REALM_ADVENTURER, REALM_DM, REALM_GAMEMASTER, REALM_DEVELOPER];

/// One launchable scene: a stable action `id`, its display `label`, its Prism style
/// `variant` (`primary`/`secondary`/`danger`), and the `factory` that builds it.
/// In the default menu it is one launch button; in a launcher (`scene_select`) it
/// becomes a scene-panel row on its realm's tier-2 page IF it carries [`SceneInfo`]
/// (and a realm), and otherwise stays a plain launch button in the popup. On click
/// the menu replaces itself with `factory()`.
pub struct SceneEntry {
    pub id: String,
    pub label: String,
    pub variant: String,
    pub factory: SceneFactory,
    /// Rich metadata for the scene-selection panel; `None` = a plain launch button.
    pub info: Option<SceneInfo>,
    /// The play-mode realms this scene belongs to ([`REALM_ADVENTURER`] /
    /// [`REALM_DM`] / [`REALM_DEVELOPER`]) — a list because tools are SHARED across
    /// modes. Empty = no realm: the entry stays on the launcher's root menu.
    pub realms: Vec<String>,
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
            realms: Vec::new(),
        }
    }

    /// Attach scene-selection-panel metadata (the launcher's rich row).
    pub fn with_info(mut self, info: SceneInfo) -> Self {
        self.info = Some(info);
        self
    }

    /// Tag the scene as a member of a play-mode realm (repeatable — tools are
    /// shared across modes). It then lists on that mode's tier-2 page.
    pub fn with_realm(mut self, realm: impl Into<String>) -> Self {
        self.realms.push(realm.into());
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
    GameSettings::load(); // unified settings.json → GAME_SETTINGS + seeds display::CURRENT
    // Load the UI stringtable for the persisted language (text ruling 2026-07-31):
    // shell display strings are `$token`s; `en-us` is the seed locale, and an unset
    // language means exactly that.
    {
        let lang = GAME_SETTINGS.lock().map(|s| s.language.clone()).unwrap_or_default();
        let lang = if lang.is_empty() { "en-us".to_string() } else { lang };
        flicker::ui::strings::load_str(SHELL_STRINGS_JSON, &lang);
    }
    SCENE_SELECT.with(|s| s.set(config.scene_select));
    set_scenes(config.scenes);
    // Every shell client inherits the Prism pointer (theme-tinted hardware
    // cursor); when the pointer is hidden/captured elsewhere it simply isn't
    // shown — no visibility wiring here.
    // Boot from the MANIFEST: the scene folder was indexed on first use, and exactly
    // one file claimed `boot`. The engine never names a scene — it asks the manifest
    // which scene starts, and resolves it through the same roster every later
    // transition uses. The whole chain is authored data the shell can show you.
    let boot = manifest().boot().to_string();
    let manager = SceneManager::from_roster(&boot, Box::new(resolve_shell_scene))
        .unwrap_or_else(|| panic!("boot scene '{boot}' did not resolve — its behaviour is unregistered"))
        .with_cursor(crate::theme::cursor_image());
    let result = run_app(manager);
    // The window is gone now; persist its final windowed size + position so the next
    // launch reopens the same way.
    persist_window_geometry();
    result
}

/// After the event loop exits, fold the window's final WINDOWED size + position into
/// the persisted display setting so the next launch reopens the same size in the same
/// spot. A fullscreen exit keeps the last stored windowed placement. Reads the
/// geometry flicker-app captured at exit (`exiting`).
fn persist_window_geometry() {
    let Some(geom) = flicker::app::last_window_geometry() else {
        return;
    };
    if geom.fullscreen {
        return; // a fullscreen exit isn't a windowed placement — keep the stored one
    }
    let snapshot = {
        let mut gs = GAME_SETTINGS.lock().expect("settings lock");
        gs.display.res = display::Resolution { w: geom.width, h: geom.height };
        gs.display.pos = Some([geom.x, geom.y]);
        gs.clone()
    };
    snapshot.save();
}

/// Take any pending input-settings change made in the pause→settings overlay, for
/// the in-game scene to apply LIVE; `None` when nothing changed since the last poll.
/// The settings scene pushes on Apply/Back (see `UnifiedSettingsScene::commit_settings`);
/// a scene SEEDS its initial values from [`input_controls`] at enter, then polls this
/// for later changes.
///
/// The `AbstractControls` carries only the LOOK settings the panel owns (mouse
/// sensitivity + invert); a scene applies those and KEEPS its own `move_speed` (a
/// gameplay control). `InputMap` carries the current keybinds.
pub fn take_pending_input() -> Option<(InputMap, AbstractControls, GamepadConfig)> {
    INPUT_SETTINGS.lock().ok().and_then(|mut p| p.take())
}

/// Full settings state persisted to `settings.json` — the ONE persisted struct with
/// the ONE writer (`save`), so nothing clobbers. The display setting is folded in
/// here: it used to be a separate DisplaySetting written to the SAME `settings.json`,
/// which overwrote (and was overwritten by) the game settings.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct GameSettings {
    audio: AudioSettings,
    video: VideoSettings,
    input: InputSettings,
    /// The persisted input PROFILE — per-context keybinds (World rebinds live here),
    /// analog tuning, gamepad config. This is the fix for the "rebinds lost on relaunch"
    /// gap (spec §7.2): `InputSettings` above is scalars only and never carried the
    /// `InputMap`. `#[serde(default)]` so an older `settings.json` without this key still
    /// loads (→ [`InputProfile::default`]).
    #[serde(default)]
    input_profile: InputProfile,
    /// Window mode + size + windowed position. The live value is mirrored in the
    /// `display` module's `CURRENT`; this is its persisted home.
    #[serde(default)]
    display: display::DisplaySetting,
    /// The UI language — selects the stringtable locale (tier-3 player config; text
    /// ruling 2026-07-31). Empty (the derived default / an older settings.json) reads
    /// as `en-us` at load; a future Settings dropdown writes it.
    #[serde(default)]
    language: String,
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
    quality: usize,
    vsync: bool,
    fps_limit: usize,
}

impl Default for VideoSettings {
    fn default() -> Self {
        // Display mode + resolution are NOT here: they belong to the single
        // `DisplaySetting` (`GameSettings.display`), which the Video-tab dropdowns
        // read/write directly via the `display` module — one source of truth.
        Self { quality: 3, vsync: true, fps_limit: 2 }
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

    /// Load the unified settings from `settings.json` into `GAME_SETTINGS`, and SEED
    /// the display module's live `CURRENT` from it. Best-effort: a missing/invalid
    /// file leaves the defaults in place. Call once at startup, before the window
    /// opens (the display setting is applied later, in `LogoScene::enter`).
    fn load() {
        let path = Self::settings_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        match serde_json::from_slice::<GameSettings>(&bytes) {
            Ok(mut loaded) => {
                display::seed(loaded.display);
                // A saved profile froze the default binds at save time — adopt
                // the current build's defaults for anything it leaves unbound,
                // BEFORE any consumer (the settings screen, the pending-input
                // push, a scene seed) reads it. Without this, every default
                // binding added after the save is dead hardware behind a
                // settings file.
                loaded.input_profile.backfill_from_presets();
                *GAME_SETTINGS.lock().expect("settings lock") = loaded;
                tracing::info!("loaded settings from {}", path.display());
            }
            Err(e) => tracing::warn!("ignoring invalid {}: {e}", path.display()),
        }
    }
}

/// Fold a display-setting change into the unified `GameSettings` and persist it —
/// `display::set_current` calls this so display + game settings share ONE file with
/// ONE writer (no clobber). The file write happens after the lock is released.
pub(crate) fn persist_display_setting(setting: display::DisplaySetting) {
    let snapshot = {
        let mut gs = GAME_SETTINGS.lock().expect("settings lock");
        gs.display = setting;
        gs.clone()
    };
    snapshot.save();
}

// `LazyLock` (not a bare `const Mutex::new`) because `input_profile` holds a populated
// `InputProfile` (`Vec`/`String`/`HashMap`) that cannot be built in a const initializer.
// The lazy seed is exactly `GameSettings::default()`, so the pre-load defaults match the
// derived `Default` with no hand-maintained duplicate literal to drift (`405F7034`).
static GAME_SETTINGS: LazyLock<Mutex<GameSettings>> =
    LazyLock::new(|| Mutex::new(GameSettings::default()));

/// Input settings changes pushed from the pause scene and consumed by
/// the game scene. `None` when no pending change exists.
static INPUT_SETTINGS: Mutex<Option<(InputMap, AbstractControls, GamepadConfig)>> =
    Mutex::new(None);

/// Build an [`AbstractControls`] from the persisted input settings — the mouse LOOK
/// settings the settings panel owns (sensitivity + invert). `move_speed` and the
/// gamepad fields stay at their defaults on purpose: a scene keeps its OWN
/// `move_speed` (a gameplay control) and merges only these look fields.
fn input_controls_from(settings: &GameSettings) -> AbstractControls {
    AbstractControls {
        mouse_sensitivity: settings.input.mouse_sensitivity,
        invert_mouse_pitch: settings.input.invert_pitch,
        invert_mouse_yaw: settings.input.invert_yaw,
        ..AbstractControls::default()
    }
}

/// Publish an input-settings change for the in-game scene to pick up on its next
/// [`take_pending_input`] poll — the push side of the settings→engine seam.
fn set_pending_input(map: InputMap, controls: AbstractControls, gamepad: GamepadConfig) {
    if let Ok(mut pending) = INPUT_SETTINGS.lock() {
        *pending = Some((map, controls, gamepad));
    }
}

/// The current input LOOK controls (mouse sensitivity + invert) from the persisted
/// settings — for a scene to SEED its controls when it enters, so the settings
/// panel's values apply from the first frame (not only after a live change). Pairs
/// with [`take_pending_input`] (live changes). `move_speed` stays the caller's to keep.
pub fn input_controls() -> AbstractControls {
    let settings = GAME_SETTINGS.lock().expect("settings lock").clone();
    input_controls_from(&settings)
}

/// The current persisted input [`InputProfile`] — the per-context keybinds (with the
/// World rebinds), analog tuning, and gamepad config. Parallels [`input_controls`]: a
/// scene / the settings overlay SEEDS its `InputMap` from this at enter, so a rebind
/// made last session is live from the first frame (spec §7.2). Live in-session changes
/// still flow through [`take_pending_input`].
pub fn input_profile() -> InputProfile {
    GAME_SETTINGS.lock().expect("settings lock").input_profile.clone()
}

/// The Lua-driven intro splash (`scripts/logo.lua`, timed by `UI.logo`): ONE
/// full-screen logo that fades in / holds / fades out, then reports `done`.
const LOGO_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/logo.lua");

/// Where the scene files live — the ONE thing the engine knows about scenes.
///
/// Asked of the content-roots service rather than spelled out, so the tree stays
/// relocatable (an app's `content.json` moves it) and no second path can drift.
/// The folder is READ AT RUNTIME, not embedded: authoring a scene without a
/// recompile is the entire point of the manifest — a `.scene.json` dropped in here
/// is in the roster the next time the app starts, and the boot chain can be
/// re-ordered by editing a file. The cost is that a broken tree is a startup
/// failure rather than a build failure, which is why [`manifest`] refuses to hand
/// back a half-loaded roster.
fn scenes_dir() -> std::path::PathBuf {
    flicker_content::roots().sensorium().join("scenes")
}

thread_local! {
    /// The indexed scenes folder, loaded once per process. `thread_local` (like
    /// [`SCENES`]) because the shell runs on the winit thread and a `SceneDef`
    /// carries a `UiNode`, which is not `Send`.
    static MANIFEST: RefCell<Option<Rc<SceneManifest>>> = const { RefCell::new(None) };
}

/// The scene manifest — indexing the folder on first use.
///
/// A folder that will not index is FATAL and panics with the loader's own message
/// (which names the offending file). There is nothing to limp on with: no roster
/// means no boot scene, and the alternative to a panic is a black window, which is
/// the exact failure the whole scene-file design exists to eliminate. [`run`] calls
/// this before it opens a window, so the failure lands in a terminal.
fn manifest() -> Rc<SceneManifest> {
    MANIFEST.with(|cell| {
        if let Some(m) = cell.borrow().as_ref() {
            return m.clone();
        }
        let dir = scenes_dir();
        let loaded = SceneManifest::load_dir(&dir, &builtin_templates())
            .unwrap_or_else(|e| panic!("scene manifest failed to load: {e}"));
        tracing::info!(
            "scene manifest: {} scene(s) in {} — boot '{}'",
            loaded.len(),
            dir.display(),
            loaded.boot()
        );
        let m = Rc::new(loaded);
        *cell.borrow_mut() = Some(m.clone());
        m
    })
}

/// Builds the Rust scene for one loaded scene file — the BEHAVIOUR half of the
/// split. `None` means the file named a behaviour that cannot be built (a missing
/// param), which the caller reports.
type BehaviourBuilder = fn(&SceneDef) -> Option<Box<dyn Scene>>;

/// The shell's BEHAVIOUR REGISTRY: the name a scene file may put in `behaviour`,
/// and the Rust impl that plays it.
///
/// This is the whole of what the engine knows, and it is deliberately not a list of
/// scenes. A scene file names a behaviour; the engine looks the NAME up here. So
/// adding a scene — a third splash, a second menu page, another bench front — is
/// dropping a file into `content/sensorium/scenes/`, and only a genuinely new KIND
/// of scene costs Rust: one entry in this table.
const BEHAVIOURS: &[(&str, BehaviourBuilder)] =
    &[("splash", build_splash), ("menu", build_menu)];

/// The `splash` behaviour: play the ONE image named by `params.image` on
/// `logo.lua`'s fade/hold timeline, then fire `done` and let the file route it.
fn build_splash(def: &SceneDef) -> Option<Box<dyn Scene>> {
    let Some(rel) = def.param_str("image") else {
        tracing::error!(
            "scene '{}' uses the `splash` behaviour but names no `params.image` — \
             a splash with no image is a black screen",
            def.id
        );
        return None;
    };
    Some(Box::new(LogoScene::new(splash_image(&def.id, rel), def.clone())))
}

/// The `menu` behaviour: the shell's main menu, whose buttons come from the
/// client's registered [`SceneEntry`] set and launch BY ID.
fn build_menu(_def: &SceneDef) -> Option<Box<dyn Scene>> {
    Some(Box::new(MenuScene::new()))
}

/// Read a splash image named RELATIVE TO THE CONTENT ROOT (so a new splash is a PNG
/// plus a scene file, with no Rust and no `include_bytes!`).
///
/// An unreadable image logs the full path and yields no bytes: the splash then
/// plays its backdrop and still advances, because a stuck intro is worse than a
/// missing logo. `every_shipped_splash_names_a_readable_image` is what makes this
/// loud rather than merely survivable.
fn splash_image(id: &str, rel: &str) -> Vec<u8> {
    let path = flicker_content::roots().root().join(rel);
    match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("scene '{id}': image {} could not be read: {e}", path.display());
            Vec::new()
        }
    }
}

/// The shell's scene roster — the ids [`Transition::Goto`] resolves through.
///
/// The MANIFEST comes first: a scene the human authored is looked up by id, and the
/// behaviour its file names is built from [`BEHAVIOURS`]. After it come the client's
/// registered [`SceneEntry`] benches, so `Goto { id: "populous" }` works from a menu
/// button with no extra wiring — a bench is a tool in the toolbox and appears on the
/// launcher, whereas a splash only ever needs to be reachable BY ID. Authored scenes
/// win either way, so a client cannot shadow the boot chain with a bench of the same
/// name.
///
/// No scene id appears in this function, and none may: that is the rule the manifest
/// exists to keep.
fn resolve_shell_scene(id: &str) -> Option<Box<dyn Scene>> {
    if let Some(def) = manifest().get(id) {
        let Some((_, build)) = BEHAVIOURS.iter().find(|(name, _)| *name == def.behaviour) else {
            tracing::error!(
                "scene '{id}' names behaviour '{}', which is not registered — known \
                 behaviours are {:?}; a scene file alone does not make a scene run",
                def.behaviour,
                BEHAVIOURS.iter().map(|(n, _)| *n).collect::<Vec<_>>()
            );
            return None;
        };
        return build(def);
    }
    // Not an authored scene — fall through to the client's registered benches.
    scenes().iter().find(|e| e.id == id).map(|e| (e.factory)())
}

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

/// The result an intro splash fires once it has played (or been skipped). Its
/// scene FILE maps this name to the scene that follows; this file never learns
/// what that is.
const SPLASH_DONE: &str = "done";

/// Intro splash: plays ONE logo (timeline + fade in `logo.lua`), then fires
/// [`SPLASH_DONE`] and lets its scene file route it — also skippable with click /
/// Space / Escape. The hold-time is, in future, room to stream the menu's
/// background scene.
struct LogoScene {
    /// The PNG this splash plays, registered to the script as the texture `logo`.
    /// Read from the content root at build time (a splash is a PNG + a scene file,
    /// no `include_bytes!`), so it is owned bytes, empty when the image was unreadable.
    image: Vec<u8>,
    /// This splash's scene FILE. It used to carry a `next: &'static str` — the
    /// successor named in Rust — which still made "publisher, then engine, then
    /// menu" a fact compiled into this file. The scene now fires a RESULT and the
    /// file's `exits` decide where that result goes.
    def: SceneDef,
    script: Option<ScriptHost>,
    /// `[white, logo]` — index = the texture id the script references.
    textures: Vec<TextureHandle>,
    /// The logo's native pixel size, so the script can fit + centre it.
    size: (u32, u32),
    elapsed: Duration,
}

impl LogoScene {
    /// A splash playing `image`, routed by the scene file `def`.
    ///
    /// ONE image per scene. This used to be a single scene walking a LIST of logos on
    /// one Lua timeline, which hid "publisher, then engine, then menu" inside a content
    /// array. Each splash is now its own registered scene and the order is authored in
    /// its file — the same mechanism the main menu uses to launch a bench.
    ///
    /// A file with no [`SPLASH_DONE`] exit would leave the splash playing forever, so
    /// it is reported HERE, once, at construction — not per-frame, and not silently.
    fn new(image: Vec<u8>, def: SceneDef) -> Self {
        if !def.exits.contains_key(SPLASH_DONE) {
            tracing::error!(
                "splash '{}' declares no `{SPLASH_DONE}` exit — it will play and then \
                 sit there; add one to its scene file",
                def.id
            );
        }
        Self {
            image,
            def,
            script: None,
            textures: Vec::new(),
            size: (1, 1),
            elapsed: Duration::ZERO,
        }
    }

    /// Per-frame model: elapsed seconds + the logo's native size.
    fn model(&self) -> ValueMap {
        ValueMap::new()
            .with("elapsed", self.elapsed.as_secs_f32())
            .with("img_w", self.size.0)
            .with("img_h", self.size.1)
    }
}

impl Scene for LogoScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // id 0 is the white pixel (rect fills / backdrop); this splash's logo follows.
        // It always registers as `logo`, whichever image this scene carries, so the
        // script never learns which logo it is drawing.
        let mut textures = vec![renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1)];
        let mut ids: Vec<(&str, u32)> = vec![("white", 0)];
        let mut sizes = (1, 1);
        if let Some((handle, w, h)) = load_image_texture(renderer, &self.image) {
            ids.push(("logo", textures.len() as u32));
            textures.push(handle);
            sizes = (w, h);
        }
        self.script = match ScriptHost::new(LOGO_SCRIPT, "logo.lua") {
            Ok(script) => {
                if let Err(e) = script.set_texture_ids(&ids) {
                    tracing::error!("logo texture registration failed: {e}");
                }
                expose_ui_elements(&script);
                Some(script)
            }
            Err(e) => {
                tracing::error!("logo script load failed: {e}");
                None
            }
        };
        self.textures = textures;
        self.size = sizes;
        // Apply the persisted (or default) display setting now the window
        // exists — so a saved fullscreen/resolution choice takes effect at
        // launch.
        display::current().apply(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, _signals: &mut SceneInput, renderer: &Renderer) -> Transition {
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
            // FIRE A RESULT, don't name a destination: this splash knows only that it
            // has finished. Its file turns `done` into a `Goto` — so the timer stays
            // Rust and the routing is data. A file with no such exit already logged at
            // construction; staying put is the loud failure (a stuck splash), never a
            // guessed successor.
            return self.def.exit(SPLASH_DONE).unwrap_or(Transition::None);
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
    Resolution(display::Resolution),
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
                pos: prev.pos,
            },
            matches!(m, display::DisplayMode::ExclusiveFullscreen),
        ),
        DisplayChange::Resolution(res) => (
            display::DisplaySetting {
                mode: prev.mode,
                res,
                pos: prev.pos,
            },
            // A windowed resize is low-risk (drag it back); confirm only when it
            // drives an exclusive-fullscreen mode.
            matches!(prev.mode, display::DisplayMode::ExclusiveFullscreen),
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
            view: MenuView::new(&theme, "confirm", &MenuPage::default(), &confirm_items(), &[]),
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
        let secs = self.remaining.ceil().max(0.0) as i32;
        format!("{} {secs}s", flicker::ui::strings::resolve("$menu_reverting_in"))
    }
}

impl Scene for ConfirmDisplayScene {
    fn is_overlay(&self) -> bool {
        true
    }

    fn update(&mut self, dt: Duration, input: &InputState, _signals: &mut SceneInput, renderer: &Renderer) -> Transition {
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
const SETTINGS_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/settings.lua");

/// The shared front-end modal, now a DECLARATIVE component tree (menu / pause /
/// confirm, selected by the published `MENU.screen`), embedded. Rendered by the
/// Rust component walker (`run_ui`); replaces the retired `modal.lua`.
const MENU_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/menu.lua");

/// The shell's declarative UI layout (`modal`/`screens`/`settings`/`logo`/
/// `loading`), embedded. The client's in-game HUD layout is separate.
const SHELL_UI_JSON: &str = include_str!("../../../../content/sensorium/resources/ui_elements.json");
/// The UI stringtable (`{ token: { locale: text } }`) — every shell display string is a
/// `$token` into this (text ruling 2026-07-31); tier-2 content, en-us seeded.
const SHELL_STRINGS_JSON: &str = include_str!("../../../../content/data/stringtable.json");

/// Expose the embedded shell `ui_elements.json` to `script` as the `UI` global,
/// so a screen reads its layout from named elements (`UI.modal.panel.w`) instead
/// of hardcoded constants. Logs and continues on failure (scripts guard
/// `if not UI`).
fn expose_ui_elements(script: &ScriptHost) {
    load_ui_json_str(script, SHELL_UI_JSON);
}

/// Publish the built-in input profiles to a settings script as the `PROFILES` data
/// global — the controller tab's selector options (spec §7.3). Each entry is
/// `{ value, label }`: `value` is the stable [`InputProfile::name`] (persisted), `label`
/// the display string. Variable-length structure, so it rides a data global like `MENU`
/// (not the flat Model). When unpublished (e.g. a build-time tree check), `settings.lua`
/// falls back to a single "Default" option.
fn publish_profiles(script: &ScriptHost) {
    let list: Vec<serde_json::Value> = InputProfile::PRESET_NAMES
        .iter()
        .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
        .collect();
    if let Err(e) = script.set_global_json("PROFILES", &serde_json::Value::Array(list)) {
        tracing::error!("PROFILES global publish failed: {e}");
    }
}

// Every shell screen is walker-driven (or, for the logo splash, plain
// immediate Lua) — none loads the legacy `Widgets` toolkit (S10). Each
// Lua-driven shell screen builds its own `ScriptHost` inline (see `LogoScene` /
// `MenuView` / `UnifiedSettingsScene`), registering textures + the `UI` global
// the same way.

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

/// The page-level MENU fields beyond the item/scene lists: which mode tier the
/// screen shows and the tier's presentation data. All ride the `MENU` global;
/// `menu.lua` stays realm-agnostic and reads only these.
struct MenuPage {
    /// The realm id of the tier-2 page ("" = the root / a mode-less menu). Non-empty
    /// makes `menu.lua` declare `on_cancel = "menu_back"` on the screen root, so
    /// Escape/pad-B fires the same result the BACK button does.
    mode: String,
    /// A note token rendered as the popup footer ("" = none) — the DM page's
    /// under-construction "$dm_coming_soon".
    note: String,
    /// Whether the scene panel renders its header block (caption / title / count).
    /// `false` on the Adventurer page: exactly its entry, no other notes.
    panel_head: bool,
}

impl Default for MenuPage {
    fn default() -> Self {
        Self { mode: String::new(), note: String::new(), panel_head: true }
    }
}

/// Publish the screen + its page fields + its button list (+ optional scene-panel
/// rows) to a menu script as the `MENU` data global (nested JSON — variable-length
/// *structure*, so it rides this channel, not the flat `Model`). `menu.lua`'s
/// `tree()` loops `MENU.items` (popup buttons) and `MENU.scenes` (panel rows);
/// `scene_select` gates the two-column launcher layout, `mode`/`note`/`panel_head`
/// the tier-2 page chrome.
fn publish_menu(
    script: &ScriptHost,
    screen: &str,
    page: &MenuPage,
    items: &[MenuItem],
    scenes: &[SceneRow],
) {
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
        "mode": page.mode,
        "note": page.note,
        "panel_head": page.panel_head,
        "scene_select": !scenes.is_empty(),
        "items": items_arr,
        "scenes": scenes_arr,
    });
    if let Err(e) = script.set_global_json("MENU", &menu) {
        tracing::error!("MENU global publish failed: {e}");
    }
}

/// The menu's standard trailing chrome buttons (after the launchable scenes).
/// Labels are stringtable tokens (`$…`), resolved at the draw boundary.
fn menu_chrome_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("settings", "$menu_settings", "secondary"),
        MenuItem::new("quit", "$menu_quit", "danger"),
    ]
}

/// The launcher root's mode buttons (tier 1): one per Prism play mode, in
/// [`REALMS`] order. Each fires `mode_<realm>`, which the menu scene turns into a
/// [`Transition::Push`] of that realm's tier-2 menu page.
fn mode_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new(format!("mode_{REALM_ADVENTURER}"), "$menu_explore_world", "primary"),
        MenuItem::new(format!("mode_{REALM_DM}"), "$menu_build_world", "primary"),
        MenuItem::new(format!("mode_{REALM_GAMEMASTER}"), "$menu_game_master", "primary"),
        MenuItem::new(format!("mode_{REALM_DEVELOPER}"), "$menu_developer_mode", "secondary"),
    ]
}

/// The tier-2 pages' leading BACK button — the same `menu_back` result the page
/// root's `on_cancel` intent (Escape / pad-B) fires; both Pop to the root menu.
fn back_item() -> MenuItem {
    MenuItem::new("menu_back", "$menu_back", "secondary")
}

/// The pause overlay's buttons — resume, settings, return to the main menu, quit.
fn pause_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("resume", "$menu_resume", "primary"),
        MenuItem::new("settings", "$menu_settings", "secondary"),
        MenuItem::new("main_menu", "$menu_main_menu", "secondary"),
        MenuItem::new("quit", "$menu_quit", "danger"),
    ]
}

/// The display-confirm dialog's buttons.
fn confirm_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("keep", "$menu_keep", "primary"),
        MenuItem::new("revert", "$menu_revert", "danger"),
    ]
}

#[cfg(test)]
mod menu_template_tests {
    use super::*;

    /// Build `menu.lua`'s tree for a screen GPU-free (no Theme / textures) and expand it
    /// through the walker template registry — the exact script path `MenuView` caches.
    fn expanded_tree(screen: &str, page: &MenuPage, items: &[MenuItem]) -> UiNode {
        let s = ScriptHost::new(MENU_SCRIPT, "menu.lua").expect("menu.lua parses + loads");
        expose_ui_elements(&s);
        publish_menu(&s, screen, page, items, &[]);
        let tree = s
            .ui_tree()
            .expect("menu.lua tree() builds")
            .expect("menu.lua exposes tree()");
        expand(tree, &builtin_templates())
    }

    /// A tier-2 page's `MenuPage` exactly as `MenuScene::page` derives it.
    fn tier_page(realm: &'static str) -> MenuPage {
        MenuScene::for_mode(realm).page()
    }

    fn has_unresolved_template(n: &UiNode) -> bool {
        n.template.is_some() || n.children.iter().any(has_unresolved_template)
    }

    /// Pause + display-confirm now route through the `popup_menu` / `choice_dialog`
    /// templates. This asserts `menu.lua` parses, `tree()` builds, and every template node
    /// fully expands with real content — a typo'd template name would fall back to an empty
    /// page (caught by the non-empty-children assert), and any unexpanded node is caught too.
    #[test]
    fn pause_and_confirm_bridge_through_templates() {
        let pause = expanded_tree("pause", &MenuPage::default(), &pause_items());
        assert_eq!(pause.component, "screen", "pause → popup_menu full-bleed page");
        assert!(!pause.children.is_empty(), "pause popup expanded to real content");
        assert!(!has_unresolved_template(&pause), "no template marker survives expand");

        let confirm = expanded_tree("confirm", &MenuPage::default(), &confirm_items());
        assert_eq!(confirm.component, "screen", "confirm → choice_dialog full-bleed page");
        assert!(!confirm.children.is_empty(), "confirm popup expanded to real content");
        assert!(!has_unresolved_template(&confirm), "no template marker survives expand");
    }

    /// The launcher MENU screen keeps its bespoke two-column composition (no templates) and
    /// must still build unchanged.
    #[test]
    fn menu_screen_still_builds_bespoke() {
        let menu = expanded_tree("menu", &MenuPage::default(), &menu_chrome_items());
        assert_eq!(menu.component, "screen");
        assert!(!menu.children.is_empty());
    }

    /// The tier-navigation intent wiring (S9): a tier-2 page's ROOT declares
    /// `on_cancel = "menu_back"` — so Escape/pad-B rides the mini-bus to the SAME
    /// result the BACK button fires and the scene pops to the root — while the
    /// root menu declares none (Escape at the root is a no-op, as before).
    #[test]
    fn tier2_pages_declare_the_back_intent_and_the_root_does_not() {
        for realm in REALMS {
            let tree = expanded_tree("menu", &tier_page(realm), &[back_item()]);
            let intents = UiIntents::of(&tree);
            assert_eq!(
                intents.result_for(ActionSignal::Cancel),
                Some("menu_back"),
                "tier-2 '{realm}' page root declares on_cancel = menu_back"
            );
        }
        let root = expanded_tree("menu", &MenuPage::default(), &mode_items());
        assert_eq!(
            UiIntents::of(&root).result_for(ActionSignal::Cancel),
            None,
            "the root menu declares no cancel intent"
        );
    }

    /// End-to-end for directional nav (spec §8): the MENU screen's popup buttons must
    /// carry the `tab_group`/`nav_ordinal` props authored in `menu.lua` AND survive
    /// `expand()`, so the walker can flatten them into focusables. A regression here
    /// silently kills d-pad / gamepad menu nav (build stays green, so this guards it).
    #[test]
    fn menu_buttons_carry_nav_groups() {
        fn collect(n: &UiNode, out: &mut Vec<(String, String, u32)>) {
            if !n.tab_group.is_empty() {
                out.push((n.id.clone(), n.tab_group.clone(), n.nav_ordinal));
            }
            for c in &n.children {
                collect(c, out);
            }
        }
        // No scenes published here, so the only focusables are the popup chrome
        // buttons — all in the "menu" group, ordered by their published position.
        let menu = expanded_tree("menu", &MenuPage::default(), &menu_chrome_items());
        let mut nav = Vec::new();
        collect(&menu, &mut nav);
        assert!(!nav.is_empty(), "menu popup exposes focusable buttons");
        assert!(nav.iter().all(|(_, g, _)| g == "menu"), "chrome buttons form the 'menu' group: {nav:?}");
        assert!(nav.iter().any(|(id, _, ord)| id == "settings" && *ord == 0), "SETTINGS is ordinal 0");
        assert!(nav.iter().any(|(id, _, ord)| id == "quit" && *ord == 1), "QUIT is ordinal 1");
    }

    /// VOCABULARY GATE for the screens every client ships — including the launcher's
    /// mode tiers (root + the three tier-2 pages). A component kind the engine does
    /// not know draws NOTHING — the walker anchor-overlays its children and the draw
    /// arm falls through — so a typo or a name left behind by a rename is invisible
    /// until someone opens the window. This turns that into a build failure.
    #[test]
    fn the_shipped_screens_name_only_kinds_the_engine_knows() {
        // The launcher root's mode buttons + Click-Trainer-style plain launch item
        // (a stringtable-token label, per the S10 strings gate).
        let mut root_items = mode_items();
        root_items.push(MenuItem::new("clicktrainer", "$ct_click_trainer", "primary"));
        root_items.extend(menu_chrome_items());
        let mut cases = vec![
            ("menu", MenuPage::default(), root_items),
            ("pause", MenuPage::default(), pause_items()),
            ("confirm", MenuPage::default(), confirm_items()),
        ];
        // Each tier-2 page's popup: BACK + the chrome (its scene rows are client
        // DATA riding the panel, exercised by the launcher render tests instead).
        for realm in REALMS {
            let mut items = vec![back_item()];
            items.extend(menu_chrome_items());
            cases.push(("menu", tier_page(realm), items));
        }
        for (screen, page, items) in cases {
            let tree = expanded_tree(screen, &page, &items);
            assert!(
                flicker::ui::unknown_kinds(&tree).is_empty(),
                "menu.lua screen '{screen}' (mode '{}') names unknown kinds: {:?}",
                page.mode,
                flicker::ui::unknown_kinds(&tree)
            );
            // The strings gate (S10): every display literal is a `$token`.
            assert!(
                flicker::ui::raw_display_literals(&tree).is_empty(),
                "menu.lua screen '{screen}' (mode '{}') ships raw display literals: {:?}",
                page.mode,
                flicker::ui::raw_display_literals(&tree)
            );
        }

        // settings.lua is built by its own scene, so exercise it the same way.
        let s = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua").expect("settings.lua loads");
        expose_ui_elements(&s);
        let tree = s
            .ui_tree()
            .expect("settings.lua tree() builds")
            .expect("settings.lua exposes tree()");
        let tree = expand(tree, &builtin_templates());
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "settings.lua names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "settings.lua ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );

        // The MODEL-CHANNEL strings gate (S10's blind side): display copy published
        // from Rust into the Model bypasses the tree gates above, so the crate
        // self-gates its OWN source — every `.set`/`.with` value must be a resolved
        // `$token`, a data shape, or carry an explicit `strings-gate-exempt` reason.
        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("shell.rs"));
        assert!(flags.is_empty(), "raw display copy published into the Model: {flags:?}");
    }
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
    // ── Directional-nav (spec §8) — keyboard + gamepad focus traversal of the
    //    menu buttons. The walker owns the shared focus id; these drive it. ──
    /// Edge resolver over the `Menu` context (owns prev-frame + press-times).
    resolver: Resolver,
    /// The `Menu` binding map (arrows / d-pad → `Nav*`, bumpers → `Tab*`, A/Enter →
    /// `Confirm`, B/Esc → `Cancel`), sourced from the canonical profile.
    bindings: ContextualBindings,
    gamepad: GamepadConfig,
    /// Router request queue (nav writes focus directly; `Cancel` queues a pop).
    route: RouteCtx,
    /// Reused `Fired` buffer — no per-frame alloc (spec RT-7).
    nav_ev: Vec<Fired>,
    /// Monotonic tick for the resolver's edge timing (spec §3.2a).
    nav_tick: u64,
    /// The screen's declarative signal bindings (S9), collected from the cached
    /// tree's ROOT `on_<signal>` props once at build. The walker layer consumes
    /// a declared signal and its fired result name folds into the returned
    /// results exactly like a click. (No menu screen declares one today; the
    /// machinery is uniform with the settings overlay.)
    intents: UiIntents,
    /// Result names fired last frame, republished ONCE into the next walk's
    /// Model as the transient `sig_<name>` mirror (S9 ruling), then dropped.
    fired_sigs: Vec<String>,
    /// One-time guard for the default-focus init: `false` until the first `update`
    /// that finds ≥1 focusable, then `true` forever. Keeps the initial highlight a
    /// ONE-TIME seed (not a per-frame re-focus), so a later click that clears/moves
    /// the shared focus id sticks and mouse motion never re-grabs it (see `update`).
    nav_initialized: bool,
}

impl MenuView {
    /// Load `menu.lua`, register the theme textures (so `Textures.muse` resolves),
    /// expose the shell layout + styles, publish the `screen` + `page` + `items`, and
    /// build the component tree ONCE — the parsed `UiNode` is fully owned, so the script
    /// host is dropped after. Best-effort: a failure leaves a view that draws nothing.
    fn new(
        theme: &Theme,
        screen: &str,
        page: &MenuPage,
        items: &[MenuItem],
        scenes: &[SceneRow],
    ) -> Self {
        let entries = theme.lua_textures();
        let textures: Vec<TextureHandle> = entries.iter().map(|(_, h)| *h).collect();
        let styles = load_styles_str(SHELL_UI_JSON);
        // The host is dropped once the tree is built: the parsed `UiNode` is fully
        // owned data, and every control draws in the engine, so nothing reads the VM
        // again. (It was retained for a stretch of 2026-07/08 as the Lua component
        // library the walker dispatched DRAW to; that tier is gone — 2026-08-10.)
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
                publish_menu(&s, screen, page, items, scenes); // the `MENU` data global
                let tree = match s.ui_tree() {
                    // Expand any `template` nodes (pause→popup_menu, confirm→choice_dialog)
                    // into their component subtree once, before the tree is cached — identity for a
                    // template-free tree (the launcher menu is unaffected).
                    Ok(Some(t)) => Some(expand(t, &builtin_templates())),
                    Ok(None) => {
                        tracing::error!("menu.lua exposes no tree()");
                        None
                    }
                    Err(e) => {
                        tracing::error!("menu tree build failed: {e}");
                        None
                    }
                };
                tree
            }
            Err(e) => {
                tracing::error!("menu.lua load failed: {e}");
                None
            }
        };
        // The `Menu` context map (nav / confirm / cancel on keyboard + pad) from the
        // canonical profile — single-sourced, not re-declared here (spec §7.1).
        let menu_map = InputProfile::default_profile()
            .context_map("Menu")
            .cloned()
            .unwrap_or_else(InputMap::empty);
        // The screen's declarative bindings (S9), read off the EXPANDED root once
        // — cached exactly like the tree it was collected from.
        let intents = tree.as_ref().map(UiIntents::of).unwrap_or_default();
        Self {
            textures,
            tree,
            styles,
            ui_state: UiState::new(),
            commands: Vec::new(),
            resolver: Resolver::new(),
            bindings: ContextualBindings::new(menu_map),
            gamepad: GamepadConfig::default(),
            route: RouteCtx::new(),
            nav_ev: Vec::new(),
            nav_tick: 0,
            intents,
            fired_sigs: Vec::new(),
            nav_initialized: false,
        }
    }

    /// Walk the cached tree for one frame. `model` carries any per-frame binds (the
    /// confirm countdown's `subtitle`). Stashes the draw commands and returns the
    /// fired actions (`is_on("start")` / `is_on("main_menu")` …).
    fn update(&mut self, input: &InputState, renderer: &Renderer, model: &ValueMap) -> ValueMap {
        let Some(tree) = self.tree.as_ref() else {
            return ValueMap::new();
        };

        // ── One-time default focus (spec §8 polish) ─────────────────────────────
        // On the FIRST frame this popup has focusable buttons, seed the shared focus
        // id with the TOP one (first in tree order = lowest `nav_ordinal`) when nothing
        // holds it yet — so a controller opens the menu already on its first item and
        // the first d-pad press moves FROM it (`nav` steps by ordinal within the group).
        // `nav_initialized` makes this a ONE-TIME seed, never a per-frame default: after
        // it fires, a click that clears/moves focus (run_ui de-focuses on a clicked
        // frame) STICKS, and moving the mouse without clicking never re-grabs the
        // highlight — a per-frame re-focus would fight both. Runs before `run_ui` so the
        // very first rendered frame already draws the top button highlighted.
        if !self.nav_initialized {
            if let Some(first) = focusables_of(tree, model).into_iter().next() {
                if self.ui_state.focused().is_none() {
                    self.ui_state.request_focus(first.id);
                }
                self.nav_initialized = true;
            }
        }

        let size = renderer.size();
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen: size,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
        };
        // The transient `sig_<name>` mirror (S9): names fired last frame ride
        // exactly ONE Model publish for scripts to observe, then drop. Costs a
        // Model clone only on the rare frame after an intent fired.
        let mirrored;
        let model = if self.fired_sigs.is_empty() {
            model
        } else {
            let mut m = model.clone();
            UiIntents::mirror_into(&mut m, &self.fired_sigs);
            self.fired_sigs.clear();
            mirrored = m;
            &mirrored
        };
        let frame = run_ui(tree, model, &self.styles, &snap, &mut self.ui_state);
        self.commands = frame.commands;
        let mut results = frame.results;
        let hud_hit = results.is_on("hud_hit");

        // ── Directional nav (spec §8): resolve this frame's menu edges (arrows /
        //    d-pad → Nav*, bumpers → Tab*, Enter/A → Confirm, Esc/B → Cancel), route
        //    them through the walker layer (which writes the ONE shared focus id),
        //    and fold a Confirm into `results` the SAME way a click does. `menu.lua`
        //    now authors `tab_group`/`nav_ordinal` for EVERY popup (menu / pause /
        //    confirm), so all three are pad-navigable via this path. ──
        self.nav_tick = self.nav_tick.wrapping_add(1);
        self.nav_ev.clear();
        self.resolver
            .resolve_frame(&self.bindings, &self.gamepad, input, self.nav_tick, &mut self.nav_ev);
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .nav_ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, hud_hit).with_nav(tree, model).with_intents(&self.intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // ONE drain: a declared intent that fired AND a pad Confirm on a focused
        // button arrive as the same thing — a result name folded in exactly like a
        // click (`results.set(name, true)`) — and both queue for the one-frame
        // `sig_<name>` Model mirror above.
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
            self.fired_sigs.push(name);
        }
        self.route.requests.clear();

        results
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
    /// The 1×1 white + theme textures for `render_hud` (`textures[0]` = white).
    textures: Vec<TextureHandle>,
    /// The declarative `settings.lua` tree, built + expanded ONCE (walker-driven).
    tree: Option<UiNode>,
    /// Resolved `ui_elements.json` styles (dotted `style` paths resolve against it).
    styles: serde_json::Value,
    /// Retained walker interaction state (open dropdown, slider drag capture).
    ui_state: UiState,
    /// Draw commands stashed by `update`'s `run_ui`, blitted in `render`.
    commands: Vec<HudCommand>,
    rebind: RebindCapture,
    /// Local copy of settings (edits buffered here, persisted on apply).
    settings: GameSettings,
    /// Current input map (mutated by rebinds).
    input_map: InputMap,
    /// The screen's declared surface set (S8): the section rail + input sub-tab
    /// radio groups, the rebind banner + applied flash, and the two overlay
    /// dialogs. Owns every `visible_bind` gate `settings.lua` reads; published
    /// into the Model once per frame ([`Surfaces::publish`] in `model`).
    surfaces: Surfaces,
    /// Active input sub-tab: "keyboard" / "mouse" / "controller" (two-way via the
    /// `input_subtab` pill bind; the `sub_*` gates mirror it through `surfaces`).
    input_subtab: String,
    /// Active controller profile (two-way via the `ctrl_profile` select bind).
    ctrl_profile: String,
    /// Scroll offset (px) of the content region — round-tripped through the `list`
    /// node's `scroll_off` bind, reset to 0 on a section / sub-tab change. The
    /// wheel itself rides `UiInput.wheel`; no Model plumbing.
    scroll_off: f32,
    /// "SETTINGS APPLIED" flash countdown (s), decayed by `dt`.
    applied: f32,
    /// Previous-frame Escape state — **rebind-cancel only**. Rebind capture keeps
    /// raw polling by design (it grabs arbitrary keys, so it cannot ride the
    /// signal bus); every OTHER Esc path goes through the mini-bus below, where
    /// the Menu-context map's `Cancel` fires the screen's declared
    /// `settings_close` intent (S9).
    rebind_esc_prev: bool,
    /// True once any buffered setting or keybind differs from what was last persisted —
    /// gates the unsaved-changes confirm on close. Set on a real edit, cleared on commit.
    dirty: bool,
    // ── The settings mini-bus (S9): the same resolve ▸ dispatch seam MenuView
    //    runs, so Esc/pad-B arrive as `Cancel` events the walker layer turns
    //    into the screen's DECLARED `settings_close` intent. ──
    /// Edge resolver over the Menu-context map (owns prev-frame + press-times).
    resolver: Resolver,
    /// The `Menu` binding map (Esc/B → `Cancel`, arrows/d-pad → `Nav*`, …),
    /// sourced from the canonical default profile — never re-declared here.
    bindings: ContextualBindings,
    gamepad: GamepadConfig,
    /// Router request queue; also receives the surface-context push/pops from
    /// [`Surfaces::apply_surface_contexts`] each frame.
    route: RouteCtx,
    /// Reused `Fired` buffer — no per-frame alloc (spec RT-7).
    ev: Vec<Fired>,
    /// Monotonic tick for the resolver's edge timing (spec §3.2a).
    tick: u64,
    /// The screen's declarative bindings (S9): `settings.lua`'s root declares
    /// `on_cancel = "settings_close"`. Collected once from the cached tree.
    intents: UiIntents,
    /// Intent names fired last frame — republished ONCE into the next Model as
    /// the transient `sig_<name>` mirror, then dropped.
    fired_sigs: Vec<String>,
}

/// The settings screen's **Screen declaration** (S8): every `visible_bind` gate
/// `settings.lua` reads, declared once. The category rail and the input sub-tab
/// strip are radio groups (`set_exclusive`); `rebinding` / `applied` are flags
/// derived from scene state each frame; `confirm_close` / `restore_note` are the
/// overlay dialogs — modal (the scene's update ladder processes only their
/// actions while shown), carrying their S9 input-context as data (surfaced in
/// `visibility_diff`, routed by nothing yet).
fn settings_surfaces() -> Surfaces {
    let mut decls = vec![
        Surface::new("sec_video").group("section").on(),
        Surface::new("sec_audio").group("section"),
        Surface::new("sec_input").group("section"),
    ];
    for (i, name) in INPUT_SUBTABS.iter().enumerate() {
        let s = Surface::new(format!("sub_{name}")).group("subtab");
        decls.push(if i == 0 { s.on() } else { s });
    }
    decls.extend([
        Surface::new("rebinding"),
        Surface::new("applied"),
        Surface::new("confirm_close").context("Menu"),
        Surface::new("restore_note").context("Menu"),
    ]);
    Surfaces::new(decls)
}

/// The input sub-tabs in STRIP ORDER. The `input_subtab` pill's option values are
/// indices into this list (an index is a number, everywhere) and each entry names
/// its `sub_<name>` visibility surface — one list, so the strip, the surfaces and
/// the scene's own sub-tab state cannot disagree.
const INPUT_SUBTABS: [&str; 3] = ["keyboard", "mouse", "controller"];

/// Backend range of the mouse look sensitivity (the `m_look` row's display slider
/// runs 0..100 over this — mapped here, so the slider stays a plain 0..100 bind).
const LOOK_SENS_MIN: f32 = 0.001;
const LOOK_SENS_MAX: f32 = 0.02;

/// The unwired PREVIEW controls' fixed values (from `settings.*.groups` `default`s),
/// published read-only as `pv_<id>` so the layout preview shows sensible numbers.
/// These rows are inert (their control's `enabled_bind` is the always-false `off`).
const PREVIEW_NUMS: &[(&str, f64)] = &[
    ("pv_fov", 90.0),
    ("pv_gamma", 50.0),
    ("pv_a_master", 80.0),
    ("pv_a_music", 70.0),
    ("pv_a_fx", 85.0),
    ("pv_a_amb", 55.0),
    ("pv_a_voice", 100.0),
    ("pv_m_aim", 65.0),
    ("pv_m_edge_speed", 55.0),
];
const PREVIEW_BOOLS: &[(&str, bool)] = &[("pv_a_subs", false), ("pv_m_edge", true)];

impl UnifiedSettingsScene {
    fn new(theme: Theme, input_map: &InputMap) -> Self {
        let entries = theme.lua_textures();
        let textures: Vec<TextureHandle> = entries.iter().map(|(_, h)| *h).collect();
        let styles = load_styles_str(SHELL_UI_JSON);
        // Build the declarative tree ONCE, then expand its `window` template into
        // components — the same cache point MenuView uses. The host is dropped after:
        // the expanded tree is fully-owned data and every control draws in the engine.
        let tree = match ScriptHost::new(SETTINGS_SCRIPT, "settings.lua") {
            Ok(s) => {
                let ids: Vec<(&str, u32)> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| (*name, i as u32))
                    .collect();
                if let Err(e) = s.set_texture_ids(&ids) {
                    tracing::error!("settings texture registration failed: {e}");
                }
                expose_ui_elements(&s); // the `UI` global (chrome config + styles)
                publish_profiles(&s); // the `PROFILES` global (controller-tab selector, §7.3)
                let tree = match s.ui_tree() {
                    // Expand the `window` template into its component subtree once, before caching.
                    Ok(Some(t)) => Some(expand(t, &builtin_templates())),
                    Ok(None) => {
                        tracing::error!("settings.lua exposes no tree()");
                        None
                    }
                    Err(e) => {
                        tracing::error!("settings tree build failed: {e}");
                        None
                    }
                };
                tree
            }
            Err(e) => {
                tracing::error!("settings.lua load failed: {e}");
                None
            }
        };
        let settings = GAME_SETTINGS.lock().expect("settings lock").clone();
        // The Menu-context map for the mini-bus, from the canonical profile
        // (single-sourced, spec §7.1) — the same seed MenuView uses.
        let menu_map = InputProfile::default_profile()
            .context_map("Menu")
            .cloned()
            .unwrap_or_else(InputMap::empty);
        // The screen's declarative bindings (S9), read off the EXPANDED root once.
        let intents = tree.as_ref().map(UiIntents::of).unwrap_or_default();
        Self {
            theme,
            textures,
            tree,
            styles,
            ui_state: UiState::new(),
            commands: Vec::new(),
            rebind: RebindCapture::new(),
            settings,
            input_map: input_map.clone(),
            surfaces: settings_surfaces(),
            input_subtab: "keyboard".to_string(),
            ctrl_profile: "default".to_string(),
            scroll_off: 0.0,
            applied: 0.0,
            rebind_esc_prev: false,
            dirty: false,
            resolver: Resolver::new(),
            bindings: ContextualBindings::new(menu_map),
            gamepad: GamepadConfig::default(),
            route: RouteCtx::new(),
            ev: Vec::new(),
            tick: 0,
            intents,
            fired_sigs: Vec::new(),
        }
    }

    /// The active category-rail section, read off the `section` radio group —
    /// the Screen declaration is the one truth for which section is showing.
    fn section(&self) -> &'static str {
        if self.surfaces.is_on("sec_audio") {
            "audio"
        } else if self.surfaces.is_on("sec_input") {
            "input"
        } else {
            "video"
        }
    }

    /// The active sub-tab's position in [`INPUT_SUBTABS`] — what the `input_subtab`
    /// strip binds to.
    fn subtab_index(&self) -> usize {
        INPUT_SUBTABS.iter().position(|s| *s == self.input_subtab).unwrap_or(0)
    }

    /// The active controller profile's position in the published `PROFILES` list —
    /// what the `ctrl_profile` select binds to.
    fn profile_index(&self) -> usize {
        InputProfile::PRESET_NAMES
            .iter()
            .position(|(name, _)| *name == self.ctrl_profile)
            .unwrap_or(0)
    }

    /// Build the per-frame Model the walker reads: the section/sub-tab gates + header
    /// text + nav styling (scene state), the scroll offset, and every control's
    /// value bind. The `select`/`pill_toggle` binds are 0-based index NUMBERS —
    /// which segment of an option strip is selected is an index, and an index is a
    /// number end to end; `update` maps the index back to the thing it names.
    fn model(&self) -> ValueMap {
        let mut m = ValueMap::new();

        // ── every visibility gate rides the Screen declaration — ONE publish ──
        self.surfaces.publish(&mut m);

        // ── header text + nav button styling, derived from the active section ──
        let section = self.section();
        for id in ["video", "audio", "input"] {
            let style = if section == id {
                "modal.buttons.variants.primary"
            } else {
                "modal.buttons.variants.secondary"
            };
            m.set(format!("nav_{id}_style"), style);
        }
        let (kicker, title, color) = match section {
            "audio" => ("$set_kicker_audio", "$set_title_audio", "theme.tokens.sig_yellow"),
            "input" => ("$set_kicker_input", "$set_title_input", "theme.tokens.sig_red"),
            _ => ("$set_kicker_video", "$set_title_video", "theme.tokens.sig_blue"),
        };
        m.set("kicker", flicker::ui::strings::resolve(kicker).into_owned());
        m.set("sec_title", flicker::ui::strings::resolve(title).into_owned());
        m.set("kicker_color_path", color);
        m.set("input_subtab", self.subtab_index() as f64);
        m.set("ctrl_profile", self.profile_index() as f64);

        // ── scroll (two-way offset; the wheel rides UiInput) + the inert gate ──
        m.set("scroll_off", self.scroll_off as f64);
        m.set("off", false); // unwired controls point `enabled_bind` here → inert

        // ── wired VIDEO (display mode + resolution ride the live DisplaySetting) ──
        let disp = display::current();
        m.set("video_display_mode", display::mode_index(disp.mode) as f64);
        m.set("video_resolution", display::resolution_index(disp.res) as f64);
        m.set("video_quality", self.settings.video.quality as f64);
        m.set("video_vsync", self.settings.video.vsync);
        m.set("video_fps_limit", self.settings.video.fps_limit as f64);

        // ── wired MOUSE (look sensitivity mapped backend → 0..100 display) ──
        let pct = ((self.settings.input.mouse_sensitivity - LOOK_SENS_MIN)
            / (LOOK_SENS_MAX - LOOK_SENS_MIN)
            * 100.0)
            .clamp(0.0, 100.0);
        m.set("look_sens_pct", pct as f64);
        m.set("input_mouse_invert_pitch", self.settings.input.invert_pitch);

        // ── unwired PREVIEW values (read-only; their controls are inert) ──
        for (k, v) in PREVIEW_NUMS {
            m.set(*k, *v);
        }
        for (k, v) in PREVIEW_BOOLS {
            m.set(*k, *v);
        }

        // Current keyboard bindings → the key caps show real keys (`bind_<ActionId>`).
        for (id, action) in KEYBOARD_ACTIONS {
            let label = self
                .input_map
                .bindings_for(*action)
                .first()
                .map(|b| b.to_string())
                .unwrap_or_default();
            m.set(format!("bind_{id}"), label);
        }

        // The transient `sig_<name>` mirror (S9): intent names fired last frame
        // ride exactly this ONE publish for the script side to observe (`update`
        // clears them right after the walk).
        UiIntents::mirror_into(&mut m, &self.fired_sigs);

        m
    }

    /// Persist the buffered settings to `GAME_SETTINGS` + `settings.json`, and PUSH
    /// the input portion (mouse look + current keybinds) to the live game scene via
    /// [`INPUT_SETTINGS`] — so a change in pause→settings reaches the running scene
    /// on its next [`take_pending_input`] poll. Scene-owned controls (`move_speed`)
    /// are deliberately NOT pushed (see [`input_controls_from`]).
    fn commit_settings(&self) {
        {
            let mut gs = GAME_SETTINGS.lock().expect("settings lock");
            // Preserve the LIVE display setting (maintained via `set_current`); the
            // buffered `self.settings.display` copy could be stale and clobber it.
            let display = gs.display;
            *gs = self.settings.clone();
            gs.display = display;
            // Fold the live-edited keybinds into the persisted profile's World context
            // BEFORE `save` — the single writer — so a rebind survives relaunch (the
            // spec §7.2 fix). The keyboard tab edits `self.input_map` (the World map);
            // the profile's other contexts (TextEntry / Menu) carry through unchanged.
            gs.input_profile.set_context_map("World", self.input_map.clone());
            gs.save();
        }
        set_pending_input(
            self.input_map.clone(),
            input_controls_from(&self.settings),
            GamepadConfig::default(),
        );
    }

    /// The modal-dialog slice of the update ladder, extracted as an associated
    /// fn so the dialog behaviour is unit-testable GPU-free: while a dialog
    /// surface is up it OWNS the frame — `Some(flow)` tells `update` what to do
    /// and nothing below the ladder runs. `settings_close` (the × click or the
    /// bus-fired Esc/B `Cancel` intent — one name, one path) is INTERCEPTED by
    /// whichever dialog is up: it dismisses the restore ack, and it cancels the
    /// unsaved-changes confirm rather than closing the overlay underneath it.
    fn modal_flow(surfaces: &mut Surfaces, results: &ValueMap) -> Option<ModalFlow> {
        // Restore-defaults acknowledgement: OK / the close intent dismisses it.
        if surfaces.is_on("restore_note") {
            if results.is_on("restore_ok") || results.is_on("settings_close") {
                surfaces.hide("restore_note");
            }
            return Some(ModalFlow::Stay);
        }
        // Unsaved-changes confirm: Save / Discard / Cancel (close intent = Cancel).
        if surfaces.is_on("confirm_close") {
            if results.is_on("confirm_save") {
                return Some(ModalFlow::CommitAndPop);
            }
            if results.is_on("confirm_discard") {
                return Some(ModalFlow::Pop);
            }
            if results.is_on("confirm_cancel") || results.is_on("settings_close") {
                surfaces.hide("confirm_close");
            }
            return Some(ModalFlow::Stay);
        }
        None
    }

    /// The Close request (the × button or the bus-fired Esc — both arrive as the
    /// declared `settings_close` result): confirm first when there are unsaved
    /// edits (the dialog surface goes up and the frame CONTINUES), else report
    /// `true` so `update` pops. Extracted for the same GPU-free tests.
    fn close_requested(surfaces: &mut Surfaces, results: &ValueMap, dirty: bool) -> bool {
        if results.is_on("settings_close") {
            if dirty {
                surfaces.show("confirm_close");
            } else {
                return true;
            }
        }
        false
    }
}

/// What the settings modal ladder decided this frame (see
/// [`UnifiedSettingsScene::modal_flow`]).
#[derive(Debug, PartialEq)]
enum ModalFlow {
    /// A dialog owns the frame — no transition, nothing below the ladder runs.
    Stay,
    /// Save-and-close from the confirm dialog: commit, then pop.
    CommitAndPop,
    /// Discard-and-close from the confirm dialog: pop without committing.
    Pop,
}

impl Scene for UnifiedSettingsScene {
    fn is_overlay(&self) -> bool {
        true
    }

    fn update(&mut self, dt: Duration, input: &InputState, _signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        let Some(tree) = self.tree.as_ref() else {
            return Transition::Pop;
        };

        let size = renderer.size();
        self.applied = (self.applied - dt.as_secs_f32()).max(0.0); // decay the flash
        // Raw Esc edge for the REBIND branch only (capture polls raw keys by
        // design). Every other Esc path rides the mini-bus below as `Cancel` →
        // the declared `settings_close` intent.
        let esc_down = input.key_down(Key::Escape);
        let rebind_esc_edge = esc_down && !self.rebind_esc_prev;
        self.rebind_esc_prev = esc_down;

        // Derived surface flags: the banner mirrors the capture, the flash its timer.
        self.surfaces.set("rebinding", self.rebind.is_active());
        self.surfaces.set("applied", self.applied > 0.0);

        // One walker pass: lay out + hit-test + draw the cached tree. The wheel
        // rides `UiInput.wheel`; the `list` region under the pointer consumes it.
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen: size,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
        };
        let model = self.model();
        let frame = run_ui(tree, &model, &self.styles, &snap, &mut self.ui_state);
        self.commands = frame.commands;
        let mut results = frame.results;
        let hud_hit = results.is_on("hud_hit");
        self.fired_sigs.clear(); // last frame's mirror rode the walk above — done

        // ── The mini-bus (S9): resolve this frame's Menu-context edges (Esc/B →
        //    Cancel, arrows/d-pad → Nav*, …) and dispatch them through the walker
        //    layer, which turns the screen's DECLARED bindings (`on_cancel =
        //    "settings_close"`) into fired result names. The bus runs even while
        //    a rebind captures — the resolver must see every edge to stay
        //    coherent — but the rebind branch below returns before the results
        //    ladder, so a fired name is simply dropped for that frame. ──
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver
            .resolve_frame(&self.bindings, &self.gamepad, input, self.tick, &mut self.ev);
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, hud_hit).with_nav(tree, &model).with_intents(&self.intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // Fired intents fold into results the SAME way a click does, and queue
        // for the one-frame `sig_<name>` Model mirror.
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
            self.fired_sigs.push(name);
        }
        // Surface context wiring (S9): flips recorded since the last frame (the
        // dialogs carry context "Menu") become Push/PopContext requests, then the
        // whole queue reconciles into the mini-bus bindings and the focus write
        // goes THROUGH the walker — the standard post-dispatch seam.
        self.surfaces.apply_surface_contexts(&mut self.route);
        let focus_change = apply_context_requests(&mut self.bindings, &self.route.requests);
        walker.apply_focus(focus_change);
        self.route.requests.clear();

        // ── Rebind capture (raw Esc or a click cancels; else grab the next input) ──
        // The walker still drew this frame (so the screen updates); its actions
        // — including a bus-fired `settings_close` — are ignored while capturing.
        if self.rebind.is_active() {
            if rebind_esc_edge || input.mouse_left_pressed {
                self.rebind.cancel();
            } else if let Some((action, binding)) = self.rebind.poll(input, &mut self.input_map) {
                tracing::info!("rebound {action} to {binding}");
                self.dirty = true;
            }
            return Transition::None;
        }

        // ── The modal dialogs own the frame while up (extracted ladder slice) ──
        if let Some(flow) = Self::modal_flow(&mut self.surfaces, &results) {
            return match flow {
                ModalFlow::Stay => Transition::None,
                ModalFlow::CommitAndPop => {
                    self.commit_settings();
                    Transition::Pop
                }
                ModalFlow::Pop => Transition::Pop,
            };
        }

        // ── Category rail + input sub-tab + controller profile (scene state) ──
        // Fold the reported offset back FIRST: the `list` bind echoes every frame
        // (the generic control contract), so a section/sub-tab change below must
        // come after it or its reset-to-top would be overwritten by the echo.
        if let Some(v) = results.number("scroll_off") {
            self.scroll_off = v as f32;
        }
        for id in ["video", "audio", "input"] {
            if results.is_on(&format!("go_{id}")) && self.section() != id {
                self.surfaces.set_exclusive(&format!("sec_{id}"));
                self.scroll_off = 0.0;
            }
        }
        // Both strips report an INDEX; the scene maps it back to the name it stands for.
        if let Some(name) = results.number("input_subtab").and_then(|i| INPUT_SUBTABS.get(i as usize))
        {
            if *name != self.input_subtab {
                self.surfaces.set_exclusive(&format!("sub_{name}"));
                self.input_subtab = name.to_string();
                self.scroll_off = 0.0;
            }
        }
        if let Some((name, _)) =
            results.number("ctrl_profile").and_then(|i| InputProfile::PRESET_NAMES.get(i as usize))
        {
            self.ctrl_profile = name.to_string();
        }

        // ── Restore defaults: reset the buffer, mark dirty, and pop the ack notice ──
        if results.is_on("settings_restore") {
            self.settings = GameSettings::default();
            self.input_map = InputMap::wasd_and_mouse();
            self.dirty = true;
            self.surfaces.show("restore_note");
        }

        // ── Apply: persist without closing (flash the confirmation) ──
        if results.is_on("settings_apply") {
            self.commit_settings();
            self.dirty = false;
            self.applied = 2.0;
        }

        // ── Save and Close: persist and pop ──
        if results.is_on("settings_back") {
            self.commit_settings();
            return Transition::Pop;
        }

        // ── Close (the × or the bus-fired Esc, both `settings_close`): confirm
        //    first when there are unsaved edits, else discard ──
        if Self::close_requested(&mut self.surfaces, &results, self.dirty) {
            return Transition::Pop;
        }

        // ── Apply video changes ──
        // Display mode + resolution edit the SINGLE DisplaySetting directly. The
        // select binds carry a 0-based index NUMBER; apply only on an ACTUAL change
        // (the binds report the current index every frame, so guard against
        // re-applying).
        if let Some(idx) = results.number("video_display_mode") {
            let idx = (idx as usize).min(display::DisplayMode::ALL.len() - 1);
            let mode = display::DisplayMode::ALL[idx];
            if mode != display::current().mode {
                if let Some(prev) = apply_display_change(DisplayChange::Mode(mode), renderer) {
                    return Transition::Push(Box::new(ConfirmDisplayScene::new(self.theme, prev)));
                }
            }
        }
        if let Some(idx) = results.number("video_resolution") {
            if let Some(res) = display::resolution_at(idx as usize) {
                if res != display::current().res {
                    if let Some(prev) = apply_display_change(DisplayChange::Resolution(res), renderer)
                    {
                        return Transition::Push(Box::new(ConfirmDisplayScene::new(self.theme, prev)));
                    }
                }
            }
        }
        // Each write only fires (and marks dirty) on an ACTUAL change — the binds report
        // the current value every frame, so an unguarded assignment would flag dirty forever.
        if let Some(q) = results.number("video_quality").map(|q| q as usize) {
            if q != self.settings.video.quality {
                self.settings.video.quality = q;
                self.dirty = true;
            }
        }
        if let Some(flicker::script::Value::Bool(b)) = results.get("video_vsync") {
            if *b != self.settings.video.vsync {
                self.settings.video.vsync = *b;
                self.dirty = true;
            }
        }
        if let Some(f) = results.number("video_fps_limit").map(|f| f as usize) {
            if f != self.settings.video.fps_limit {
                self.settings.video.fps_limit = f;
                self.dirty = true;
            }
        }

        // ── Apply input mouse changes (look slider is 0..100 display → backend) ──
        if let Some(pct) = results.number("look_sens_pct") {
            let sens = LOOK_SENS_MIN + (pct as f32 / 100.0) * (LOOK_SENS_MAX - LOOK_SENS_MIN);
            if (sens - self.settings.input.mouse_sensitivity).abs() > f32::EPSILON {
                self.settings.input.mouse_sensitivity = sens;
                self.dirty = true;
            }
        }
        if let Some(flicker::script::Value::Bool(b)) = results.get("input_mouse_invert_pitch") {
            if *b != self.settings.input.invert_pitch {
                self.settings.input.invert_pitch = *b;
                self.dirty = true;
            }
        }

        // ── Start rebind (a keycap button fires `rebind_<ActionId>`) ──
        for (id, action) in KEYBOARD_ACTIONS {
            if results.is_on(&format!("rebind_{id}")) {
                self.rebind.start(*action, false);
                break;
            }
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // Blit the commands stashed by `update`'s walker pass (`textures[0]` = white).
        if let Some(&white) = self.textures.first() {
            render_hud(renderer, &self.commands, white, &self.textures);
        }
    }
}

/// The keyboard actions the settings screen lists, in display order — the id
/// strings match `ui_elements.json`'s `settings.input.keyboard` groups. Used both
/// to publish each action's current binding (`bind_<id>`) and to dispatch a
/// `rebind_<id>` action fired by its keycap button back to the `ActionSignal`.
const KEYBOARD_ACTIONS: &[(&str, ActionSignal)] = &[
    ("MoveForward", ActionSignal::MoveForward),
    ("MoveBackward", ActionSignal::MoveBackward),
    ("StrafeLeft", ActionSignal::StrafeLeft),
    ("StrafeRight", ActionSignal::StrafeRight),
    ("MoveUp", ActionSignal::MoveUp),
    ("MoveDown", ActionSignal::MoveDown),
    ("Jump", ActionSignal::Jump),
    ("Sprint", ActionSignal::Sprint),
    ("Crouch", ActionSignal::Crouch),
    ("Interact", ActionSignal::Interact),
    ("Inventory", ActionSignal::Inventory),
    ("Map", ActionSignal::Map),
    ("Menu", ActionSignal::Menu),
    ("PrimaryAction", ActionSignal::PrimaryAction),
    ("SecondaryAction", ActionSignal::SecondaryAction),
    ("Reload", ActionSignal::Reload),
    ("Confirm", ActionSignal::Confirm),
    ("Cancel", ActionSignal::Cancel),
    ("Quit", ActionSignal::Quit),
];

/// Main menu: a thin shell over the shared [`MenuView`] (`screen = "menu"`). The
/// walker owns layout/hit-testing; this scene builds the button list from the scene
/// registry and routes each launch action + `settings`/`quit` to a transition.
///
/// In a launcher (`scene_select`) the menu is a TWO-TIER STACK of these scenes:
/// the root shows the play-mode buttons (+ realm-less launch buttons like Click
/// Trainer), and picking a mode PUSHES a second `MenuScene` carrying that realm —
/// its tier-2 page (the scene-select panel for its member scenes, or the DM note).
/// BACK / Escape on a tier-2 page POPS back to the still-live root (stack scenes:
/// the root stays frozen beneath, so its view — and focus — survive the round trip).
struct MenuScene {
    theme: Option<Theme>,
    view: Option<MenuView>,
    /// Pending input map changes from the settings overlay.
    pending_input: Option<InputMap>,
    /// The launchable scenes (the menu's launch buttons), from the shell registry.
    scenes: Rc<[SceneEntry]>,
    /// Which tier this menu shows: `None` = the root; `Some(realm)` = that mode's
    /// tier-2 page. Always `None` outside a launcher.
    mode: Option<&'static str>,
}

impl MenuScene {
    fn new() -> Self {
        Self {
            theme: None,
            view: None,
            pending_input: None,
            scenes: scenes(),
            mode: None,
        }
    }

    /// A tier-2 menu page for one play-mode realm (launcher only).
    fn for_mode(realm: &'static str) -> Self {
        Self { mode: Some(realm), ..Self::new() }
    }

    /// Whether `entry` belongs to `realm`.
    fn in_realm(entry: &SceneEntry, realm: &str) -> bool {
        entry.realms.iter().any(|r| r == realm)
    }

    /// The popup buttons. Default menu: one launch button per scene + SETTINGS/QUIT.
    /// Launcher root: the three MODE buttons + realm-less info-less scenes (e.g.
    /// Click Trainer) as plain launch buttons. Launcher tier-2 page: BACK + the
    /// realm's info-less scenes. Settings/Quit chrome always trails.
    fn items(&self) -> Vec<MenuItem> {
        let as_item =
            |e: &SceneEntry| MenuItem::new(e.id.clone(), e.label.clone(), e.variant.as_str());
        let mut items: Vec<MenuItem> = if !scene_select() {
            // Default (non-launcher) menu: every scene is a launch button.
            self.scenes.iter().map(as_item).collect()
        } else if let Some(realm) = self.mode {
            // Tier-2 page: BACK, then the realm's info-less scenes (info-bearing
            // members render as panel rows — the `SceneEntry::info` contract).
            std::iter::once(back_item())
                .chain(
                    self.scenes
                        .iter()
                        .filter(|e| e.info.is_none() && Self::in_realm(e, realm))
                        .map(as_item),
                )
                .collect()
        } else {
            // Launcher root: the mode tiers, then realm-less info-less scenes.
            mode_items()
                .into_iter()
                .chain(
                    self.scenes
                        .iter()
                        .filter(|e| e.info.is_none() && e.realms.is_empty())
                        .map(as_item),
                )
                .collect()
        };
        items.extend(menu_chrome_items());
        items
    }

    /// The scene-selection-panel rows — one per registered scene that carries
    /// `SceneInfo` AND belongs to this page's realm. Empty unless this client is a
    /// launcher (`scene_select`) on a tier-2 page, which is what gates the
    /// two-column layout (the root menu is the popup alone).
    fn scene_rows(&self) -> Vec<SceneRow> {
        if !scene_select() {
            return Vec::new();
        }
        let Some(realm) = self.mode else {
            return Vec::new();
        };
        self.scenes
            .iter()
            .filter(|e| Self::in_realm(e, realm))
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

    /// The page-level MENU fields for this tier (see [`MenuPage`]): tier-2 pages
    /// carry their realm (=> BACK/on_cancel); the DM page footers its
    /// under-construction note; the Adventurer page drops the panel header so it
    /// shows exactly its entry, nothing else.
    fn page(&self) -> MenuPage {
        MenuPage {
            mode: self.mode.unwrap_or("").to_string(),
            note: match self.mode {
                Some(REALM_DM) => "$dm_coming_soon".to_string(),
                _ => String::new(),
            },
            panel_head: self.mode != Some(REALM_ADVENTURER),
        }
    }
}

impl Scene for MenuScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        let theme = Theme::build(renderer);
        self.view =
            Some(MenuView::new(&theme, "menu", &self.page(), &self.items(), &self.scene_rows()));
        self.theme = Some(theme);
    }

    fn update(&mut self, _dt: Duration, input: &InputState, _signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        let results = match self.view.as_mut() {
            Some(view) => view.update(input, renderer, &ValueMap::new()),
            None => return Transition::None,
        };
        // ── Tier navigation (launcher only): a mode button pushes its tier-2 page;
        //    BACK (the button, or Escape/pad-B via the page root's declared
        //    `on_cancel` intent — one result name, one path) pops back to the root. ──
        if scene_select() {
            if self.mode.is_none() {
                for realm in REALMS {
                    if results.is_on(&format!("mode_{realm}")) {
                        return Transition::Push(Box::new(MenuScene::for_mode(realm)));
                    }
                }
            } else if results.is_on("menu_back") {
                return Transition::Pop;
            }
        }
        // A launch button fired → go to that scene BY ID. The button's action name is
        // already the entry's id, so a menu button and a splash's hand-off are now the
        // same mechanism: name a successor, let the manager resolve it. The menu no
        // longer builds the scene it launches.
        for entry in self.scenes.iter() {
            if results.is_on(&entry.id) {
                return Transition::Goto {
                    id: entry.id.clone(),
                    mode: GotoMode::Replace,
                };
            }
        }
        if results.is_on("settings") {
            let theme = self.theme.expect("theme built in enter");
            // Seed the settings key caps from the PERSISTED profile's World map (spec
            // §7.2) — so a rebind made last session shows immediately — falling back to
            // WASD only if the profile somehow lacks a World context.
            let input_map = self.pending_input.take().unwrap_or_else(|| {
                input_profile()
                    .context_map("World")
                    .cloned()
                    .unwrap_or_else(InputMap::wasd_and_mouse)
            });
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
            view: MenuView::new(&theme, "pause", &MenuPage::default(), &pause_items(), &[]),
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

    fn update(&mut self, _dt: Duration, input: &InputState, _signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        // ── Menu action: resume ──
        let menu_down = self.bindings.action_pressed(ActionSignal::Menu, input);
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
    fn a_lua_declared_button_lays_out_and_draws_through_run_ui() {
        use flicker::render::Vec2;
        use flicker::script::HudCommand;
        use flicker::ui::run_ui;

        // A screen whose tree is a column with one button leaf: Lua DECLARES, the engine
        // lays it out (the grid/flow engine) and draws it. End to end through run_ui —
        // the seam that used to dispatch the draw to `ui/button.lua` before the component
        // tier came back to Rust (2026-08-10); the picture is unchanged.
        const SCREEN: &str = r#"
            local M = {}
            function M.tree()
              return { component = "cell", pad = 8, children = {
                { component = "button", id = "OK", grow = 1, label = "OK", style = "btn" },
              } }
            end
            function M.update() return {} end
            function M.draw() return {} end
            return M
        "#;
        let host = ScriptHost::new(SCREEN, "s1-screen").expect("screen loads");
        let tree = host.ui_tree().expect("tree parses").expect("screen has a tree");
        let styles = load_styles_str(
            r#"{ "btn": { "fill_top": [0.14, 0.25, 0.47, 1], "radius": 4,
                 "label": [0.9, 0.9, 0.85, 1], "label_size": 14 } }"#,
        );
        let input = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            screen: Vec2::new(200.0, 60.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let frame = run_ui(&tree, &ValueMap::new(), &styles, &input, &mut UiState::new());

        let panels: Vec<_> =
            frame.commands.iter().filter(|c| matches!(c, HudCommand::Panel { .. })).collect();
        let texts = frame.commands.iter().filter(|c| matches!(c, HudCommand::Text { .. })).count();
        assert_eq!(panels.len(), 1, "the button drew its slab");
        assert_eq!(texts, 1, "the button drew its label");
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "OK")),
            "the button's top-level `label` prop reached the draw"
        );
        // Column pad 8 in a 200×60 screen → the button's flow rect is (8, 8, 184, 44).
        if let HudCommand::Panel { x, y, w, h, .. } = panels[0] {
            assert_eq!((*x, *y, *w, *h), (8.0, 8.0, 184.0, 44.0), "layout engine placed the leaf");
        }
    }

    #[test]
    fn button_glow_on_hover() {
        use flicker::render::Vec2;

        // A button draws a sapphire glow-halo panel BEHIND the slab only on hover: an
        // idle button emits 1 panel (the slab) + its label; a hovered one emits 2 panels
        // (glow halo + slab) + its label. Locks the component's hover behaviour.
        const SCREEN: &str = r#"
            local M = {}
            function M.tree()
              return { component = "cell", pad = 8, children = {
                { component = "button", id = "OK", grow = 1, label = "PLAY", style = "btn" },
              } }
            end
            function M.update() return {} end
            function M.draw() return {} end
            return M
        "#;
        let host = ScriptHost::new(SCREEN, "glow-screen").expect("screen loads");
        let tree = host.ui_tree().expect("tree parses").expect("screen has a tree");
        let styles = load_styles_str(
            r#"{ "btn": { "fill_top": [0.14, 0.25, 0.47, 1.0], "fill_bot": [0.10, 0.18, 0.34, 1.0],
                 "glow": [0.20, 0.40, 0.80, 0.5], "label": [0.86, 0.90, 1.0, 1.0],
                 "hover_top": [0.20, 0.32, 0.58, 1.0], "hover_bot": [0.14, 0.24, 0.44, 1.0] } }"#,
        );
        let model = ValueMap::new();
        // Count Panel commands with the pointer OFF the button (idle) vs INSIDE its rect
        // (8,8,184,44 → the point 100,30 is inside).
        let panels_at = |mouse: Vec2| {
            let input = UiInput {
                mouse,
                clicked: false,
                down: false,
                screen: Vec2::new(200.0, 60.0),
                typed: String::new(),
                backspace: false,
                wheel: 0.0,
            };
            run_ui(&tree, &model, &styles, &input, &mut UiState::new())
                .commands
                .iter()
                .filter(|c| matches!(c, HudCommand::Panel { .. }))
                .count()
        };
        assert_eq!(panels_at(Vec2::new(-9.0, -9.0)), 1, "idle button: just the slab");
        assert_eq!(panels_at(Vec2::new(100.0, 30.0)), 2, "hovered button: glow halo + slab");
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
            let host = ScriptHost::new(MENU_SCRIPT, "menu.lua")
                .expect("load menu.lua");
            host.set_texture_ids(&[
                ("white", 0),
                ("cell", 1),
                ("settings_panel", 2),
                ("button", 3),
                ("muse", 4),
            ])
            .expect("register textures");
            expose_ui_elements(&host);
            publish_menu(&host, screen, &MenuPage::default(), &items, &[]);
            let tree = host
                .ui_tree()
                .expect("tree parses")
                .expect("menu.lua exposes tree()");
            // Pause/confirm now return `template` nodes — expand them exactly as MenuView does.
            let tree = expand(tree, &builtin_templates());
            // The countdown subtitle, composed around its token exactly as the scene does.
            let model = ValueMap::new()
                .with("subtitle", format!("{} 9s", flicker::ui::strings::resolve("$menu_reverting_in")));
            let snap = UiInput {
                mouse: Vec2::new(-1.0, -1.0),
                clicked: false,
                down: false,
                screen: Vec2::new(1920.0, 1080.0),
                typed: String::new(),
                backspace: false,
                wheel: 0.0,
            };
            let frame =
                run_ui(&tree, &model, &styles, &snap, &mut UiState::new());
            assert!(
                !frame.commands.is_empty(),
                "menu screen '{screen}' emits panel + buttons + text"
            );
            // Every published item's label renders as a button text command — the
            // data-driven list actually produced its buttons. Labels are stringtable
            // tokens now, so what reaches a command is the RESOLVED text.
            for it in &items {
                let want = flicker::ui::strings::resolve(&it.label);
                assert!(
                    frame.commands.iter().any(
                        |c| matches!(c, HudCommand::Text { text, .. } if *text == want)
                    ),
                    "screen '{screen}' renders button label '{want}'"
                );
            }
        }
    }

    /// Walk one launcher menu page (screen "menu") GPU-free and return its draw
    /// commands — the shared harness for the tier-2 page render tests below.
    fn menu_page_commands(page: &MenuPage, items: &[MenuItem], scenes: &[SceneRow]) -> Vec<HudCommand> {
        use flicker::render::Vec2;

        // The page's labels are stringtable tokens ("$menu_load", "$menu_back", …) —
        // load the SHIPPED table so these tests prove the token→text path end to end.
        flicker::ui::strings::load_str(SHELL_STRINGS_JSON, "en-us");
        let styles = load_styles_str(SHELL_UI_JSON);
        let host = ScriptHost::new(MENU_SCRIPT, "menu.lua")
            .expect("load menu.lua");
        host.set_texture_ids(&[
            ("white", 0),
            ("cell", 1),
            ("settings_panel", 2),
            ("button", 3),
            ("muse", 4),
        ])
        .expect("register textures");
        expose_ui_elements(&host);
        publish_menu(&host, "menu", page, items, scenes);
        let tree = host
            .ui_tree()
            .expect("tree parses")
            .expect("menu.lua exposes tree()");
        let tree = expand(tree, &builtin_templates());
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        run_ui(&tree, &ValueMap::new(), &styles, &snap, &mut UiState::new())
            .commands
    }

    fn has_text(cmds: &[HudCommand], s: &str) -> bool {
        cmds.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
    }

    /// A tier-2 popup's items: BACK, then the settings/quit chrome.
    fn tier_items() -> Vec<MenuItem> {
        let mut items = vec![back_item()];
        items.extend(menu_chrome_items());
        items
    }

    #[test]
    fn menu_launcher_renders_a_row_per_scene() {
        // A launcher tier-2 page (`scene_select`, i.e. non-empty `MENU.scenes` — the
        // Developer mode launcher) builds the two-column layout: one scene row per
        // published scene, each row's name drawn and a LOAD button (the shared button
        // template) firing the scene id, under the panel header block.
        let page = MenuScene::for_mode(REALM_DEVELOPER).page();
        let scenes = vec![
            SceneRow {
                id: "pocclusters".into(),
                name: "Cluster Editor".into(),
                mode: "Tool".into(),
                region: "CSG / Voxel".into(),
                desc: "Voxel field.".into(),
                meta: "Clay 0.1".into(),
            },
            SceneRow {
                id: "pocepochs".into(),
                name: "Planet Simulation".into(),
                mode: "Simulation".into(),
                region: "World-Gen".into(),
                desc: "Epoch sim.".into(),
                meta: "Clay 0.1".into(),
            },
        ];
        let cmds = menu_page_commands(&page, &tier_items(), &scenes);
        for sc in &scenes {
            assert!(has_text(&cmds, &sc.name), "launcher renders scene row '{}'", sc.name);
        }
        let loads = cmds
            .iter()
            .filter(|c| matches!(c, HudCommand::Text { text, .. } if text == "LOAD"))
            .count();
        assert_eq!(loads, scenes.len(), "one LOAD button per scene");
        // The Developer launcher keeps the existing panel header + the BACK button.
        let title = flicker::ui::strings::resolve("$menu_select_a_scene");
        assert!(has_text(&cmds, &title), "developer page keeps the panel header");
        let back = flicker::ui::strings::resolve("$menu_back");
        assert!(has_text(&cmds, &back), "tier-2 popup carries BACK");
    }

    #[test]
    fn adventurer_page_shows_exactly_its_entry_and_no_notes() {
        // The Adventurer tier-2 page: EXACTLY its one entry (Solar Birth) — the panel
        // header block (caption / title / count note) is dropped and no
        // under-construction note appears; the popup still carries BACK.
        let page = MenuScene::for_mode(REALM_ADVENTURER).page();
        let scenes = vec![SceneRow {
            id: "solarbirth".into(),
            name: "Solar Birth".into(),
            mode: "Cinematic".into(),
            region: "Celestial".into(),
            desc: "A cinematic.".into(),
            meta: "Clay 0.1".into(),
        }];
        let cmds = menu_page_commands(&page, &tier_items(), &scenes);
        assert!(has_text(&cmds, "Solar Birth"), "the one entry renders");
        let loads = cmds
            .iter()
            .filter(|c| matches!(c, HudCommand::Text { text, .. } if text == "LOAD"))
            .count();
        assert_eq!(loads, 1, "exactly one LOAD button");
        for token in ["$menu_select_a_scene", "$menu_demo_caption", "$dm_coming_soon"] {
            let s = flicker::ui::strings::resolve(token);
            assert!(!has_text(&cmds, &s), "adventurer page has no '{s}' note");
        }
        assert!(
            !cmds.iter().any(
                |c| matches!(c, HudCommand::Text { text, .. } if text.ends_with("scenes available"))
            ),
            "adventurer page has no scene-count note"
        );
        let back = flicker::ui::strings::resolve("$menu_back");
        assert!(has_text(&cmds, &back), "tier-2 popup carries BACK");
    }

    #[test]
    fn dm_page_renders_the_coming_soon_note() {
        // The DM ("Build the World") tier-2 page: no scenes — a popup page whose
        // footer is the under-construction note, plus BACK to return to the root.
        let page = MenuScene::for_mode(REALM_DM).page();
        let cmds = menu_page_commands(&page, &tier_items(), &[]);
        let note = flicker::ui::strings::resolve("$dm_coming_soon");
        assert!(has_text(&cmds, &note), "DM page renders the '{note}' note");
        let back = flicker::ui::strings::resolve("$menu_back");
        assert!(has_text(&cmds, &back), "DM popup carries BACK");
        assert!(
            !cmds.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == "LOAD")),
            "DM page lists no scenes"
        );
    }

    /// **GAME MASTER is its own realm, not a rename of Dungeon Maker.** Both
    /// buttons exist, in order, and the new one carries the world-authoring
    /// scenes while DM keeps its under-construction note.
    #[test]
    fn the_game_master_realm_sits_beside_dungeon_maker() {
        fn dummy() -> Box<dyn Scene> {
            unreachable!("scene_rows() reads metadata only — never calls the factory")
        }
        // The two-column launcher layout is what publishes scene rows at all.
        SCENE_SELECT.with(|s| s.set(true));
        set_scenes(vec![SceneEntry::new("gm_scene", "GM Scene", "primary", dummy)
            .with_realm(REALM_GAMEMASTER)
            .with_info(SceneInfo::new("GM Scene", "Simulation", "World", "d", "m"))]);

        let gm = MenuScene::for_mode(REALM_GAMEMASTER);
        let rows: Vec<String> = gm.scene_rows().iter().map(|r| r.id.clone()).collect();
        assert_eq!(rows, ["gm_scene"], "GAME MASTER lists its own scenes");
        assert!(gm.page().note.is_empty(), "a realm with scenes footers no note");

        // Dungeon Maker is untouched: still present, still empty, still noted.
        let dm = MenuScene::for_mode(REALM_DM);
        assert!(dm.scene_rows().is_empty(), "the GM scene did not land in DM");
        assert_eq!(dm.page().note, "$dm_coming_soon", "DM keeps its note");

        // And the root lists BOTH, with GAME MASTER directly above Developer.
        let ids: Vec<String> =
            MenuScene::new().items().iter().map(|i| i.id.clone()).collect();
        let at = |id: &str| ids.iter().position(|x| x == id).expect("button present");
        assert!(at("mode_dm") < at("mode_gamemaster"), "Dungeon Maker survives above it");
        assert!(at("mode_gamemaster") < at("mode_developer"), "GAME MASTER sits above Developer");
    }

    /// One splash, one image, one exit — the script no longer sequences. Whichever
    /// logo the scene carries registers as `logo`, so this drives the script exactly
    /// as `LogoScene` does.
    #[test]
    fn logo_script_runs() {
        let host = ScriptHost::new(LOGO_SCRIPT, "logo.lua").expect("load logo.lua");
        host.set_texture_ids(&[("white", 0), ("logo", 1)]).expect("register textures");
        expose_ui_elements(&host);
        let at = |elapsed: f32| {
            ValueMap::new()
                .with("elapsed", elapsed)
                .with("img_w", 1920u32)
                .with("img_h", 1080u32)
        };
        let input = InputState::new();
        host.set_model(&at(0.3)).expect("publish model");
        let out = host.update(&input, 1920.0, 1080.0).expect("logo update");
        assert!(!out.is_on("done"), "still playing at t=0.3");
        assert!(
            host.draw(1920.0, 1080.0).expect("logo draw").len() >= 2,
            "logo emits backdrop + its image"
        );
        host.set_model(&at(99.0)).expect("publish model");
        let out = host.update(&input, 1920.0, 1080.0).expect("logo update done");
        assert!(out.is_on("done"), "done once this ONE splash has played");
    }

    /// The boot chain is reachable BY ID, end to end — the whole point of the roster.
    ///
    /// Resolving each id proves the chain exists as data rather than as constructor
    /// calls, and that a client's benches fall through the same lookup. A typo'd id
    /// must resolve to `None` so [`Transition::Goto`] can fail loud instead of
    /// stranding the player on a splash.
    #[test]
    fn the_default_scene_chain_resolves_by_id() {
        // The ids are the scene FILE names (minus `.scene.json`), and the boot scene
        // is whichever file claimed `boot` — never a constant in this code.
        assert_eq!(manifest().boot(), "TegLogo", "the boot scene is authored, not compiled in");
        for id in ["TegLogo", "CeLogo", "Main"] {
            assert!(resolve_shell_scene(id).is_some(), "'{id}' is in the shell roster");
        }
        assert!(resolve_shell_scene("no_such_scene").is_none(), "an unknown id resolves to None");
    }

    /// The successor named by a `Transition::Goto`, for asserting on a scene file's
    /// routing without a window (`Transition` carries a `Box<dyn Scene>` in its other
    /// arms, so it is neither `Debug` nor `PartialEq`).
    fn goto_target(t: Option<Transition>) -> Option<(String, GotoMode)> {
        match t {
            Some(Transition::Goto { id, mode }) => Some((id, mode)),
            _ => None,
        }
    }

    /// **A splash's successor comes out of its FILE, not out of this crate.**
    ///
    /// The proof is the second half: the same loader is handed the shipped file with
    /// its target rewritten, and the resolved successor moves with it. If the chain
    /// were still compiled in — as `LogoScene { next: "ce_logo" }` was — editing the
    /// file could not change anything and this test would fail.
    #[test]
    fn a_splash_exit_comes_from_its_file() {
        let path = scenes_dir().join(format!("TegLogo{}", flicker::ui::SCENE_FILE_SUFFIX));
        let shipped = std::fs::read_to_string(&path).expect("the publisher splash ships a file");
        let def = SceneDef::parse("TegLogo", &shipped, &builtin_templates()).expect("it loads");
        assert_eq!(
            goto_target(def.exit(SPLASH_DONE)),
            Some(("CeLogo".to_string(), GotoMode::Replace)),
            "as shipped, the publisher splash hands off to the engine splash"
        );

        // Re-author the file's target and reload: same code, different chain.
        let rerouted = shipped.replace("\"CeLogo\"", "\"Main\"");
        assert_ne!(rerouted, shipped, "the edit found the authored target");
        let def = SceneDef::parse("TegLogo", &rerouted, &builtin_templates()).expect("edited loads");
        assert_eq!(
            goto_target(def.exit(SPLASH_DONE)),
            Some(("Main".to_string(), GotoMode::Replace)),
            "the splash follows its file — the chain is data"
        );
    }

    /// GATE for every scene FILE the shell ships: it loads, it is registered under the
    /// id it declares, a Rust behaviour is bound to it, and every scene it exits to
    /// resolves in the roster. An exit to a scene nobody registered is the silent
    /// dead-end this gate exists to make loud (`Transition::Goto` would log and strand
    /// the player on the splash).
    ///
    /// The tree gates — no unexpanded `template`, no unknown component kind — are
    /// enforced by the loader itself, so an authored tree that fails either one never
    /// gets as far as `Ok` here.
    #[test]
    fn every_shipped_scene_file_loads_and_its_exits_resolve() {
        // The manifest indexed the real folder — so this walks what SHIPS, and every
        // assertion runs the actual runtime resolver, not a test stand-in.
        let m = manifest();
        assert!(m.len() >= 3, "the shell ships at least the two splashes and the menu");
        for def in m.scenes() {
            for (result, target) in def.targets() {
                assert!(
                    resolve_shell_scene(target).is_some(),
                    "scene '{}' exits `{result}` to '{target}', which is in no roster",
                    def.id
                );
            }
            assert!(
                resolve_shell_scene(&def.id).is_some(),
                "scene file '{}' has a Rust behaviour bound to it",
                def.id
            );
        }
    }

    /// Both splashes must declare the ONE result their Rust behaviour fires. Without
    /// it a splash plays its fade and then sits there forever — the exact silent
    /// dead-end the scene file exists to make authorable.
    #[test]
    fn both_splashes_declare_the_done_exit() {
        let m = manifest();
        for id in ["TegLogo", "CeLogo"] {
            let def = m.get(id).unwrap_or_else(|| panic!("'{id}' ships a scene file"));
            assert!(
                def.exit(SPLASH_DONE).is_some(),
                "splash '{id}' routes the `{SPLASH_DONE}` result its timer fires"
            );
        }
    }

    #[test]
    fn settings_tree_runs_every_section() {
        // The declarative `settings.lua` builds a component tree; the walker draws it.
        // This is the walker-drive analogue of the old immediate-mode smoke test: it
        // parses settings.lua, expands its `frame` template (Phase 3 migrated it off `window`), and
        // runs `run_ui` for each section, asserting the section's marker content renders (a Lua typo
        // or a bad template name would fall out here) — the same shape as `menu_template_tests`.
        use flicker::render::Vec2;
        use flicker::script::HudCommand;

        // The screen's display copy is `$token`s now (S10 strings gate); load the
        // shipped table so the walked commands carry the resolved en-us text.
        flicker::ui::strings::load_str(SHELL_STRINGS_JSON, "en-us");
        let styles = load_styles_str(SHELL_UI_JSON);
        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua")
            .expect("load settings.lua");
        host.set_texture_ids(&[("white", 0), ("cell", 1), ("settings_panel", 2)])
            .expect("register textures");
        expose_ui_elements(&host);
        publish_profiles(&host); // drive the controller-tab selector opts from PROFILES (§7.3)
        let tree = host
            .ui_tree()
            .expect("settings.lua tree() builds")
            .expect("settings.lua exposes tree()");
        // Expand the `frame` template exactly as `UnifiedSettingsScene::new` does.
        let tree = expand(tree, &builtin_templates());
        fn has_unresolved_template(n: &UiNode) -> bool {
            n.template.is_some() || n.children.iter().any(has_unresolved_template)
        }
        assert!(!has_unresolved_template(&tree), "the frame template fully expands");

        // The per-frame model the scene publishes for `(section, sub-tab)` — gates +
        // header text + the control binds (every strip selection is a 0-based INDEX).
        let model = |section: &str, subtab: &str| {
            let mut m = ValueMap::new();
            for id in ["video", "audio", "input"] {
                m.set(format!("sec_{id}"), section == id);
                m.set(format!("nav_{id}_style"), "modal.buttons.variants.secondary");
            }
            for id in ["keyboard", "mouse", "controller"] {
                m.set(format!("sub_{id}"), subtab == id);
            }
            let (kicker, title) = match section {
                "audio" => ("$set_kicker_audio", "$set_title_audio"),
                "input" => ("$set_kicker_input", "$set_title_input"),
                _ => ("$set_kicker_video", "$set_title_video"),
            };
            m.set("kicker", flicker::ui::strings::resolve(kicker).into_owned());
            m.set("sec_title", flicker::ui::strings::resolve(title).into_owned());
            m.set("kicker_color_path", "theme.tokens.sig_blue");
            m.set("input_subtab", INPUT_SUBTABS.iter().position(|s| *s == subtab).unwrap_or(0) as f64);
            m.set("ctrl_profile", 0.0);
            m.set("scroll_off", 0.0);
            m.set("off", false);
            m.set("rebinding", false);
            m.set("applied", false);
            m.set("video_display_mode", 0.0);
            m.set("video_resolution", 2.0);
            m.set("video_quality", 2.0);
            m.set("video_vsync", true);
            m.set("video_fps_limit", 1.0);
            m.set("look_sens_pct", 50.0);
            m.set("input_mouse_invert_pitch", false);
            m.set("bind_MoveForward", "W");
            m
        };
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let has = |cmds: &[HudCommand], s: &str| {
            cmds.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
        };
        let run = |section: &str, subtab: &str| {
            run_ui(&tree, &model(section, subtab), &styles, &snap, &mut UiState::new())
                .commands
        };

        let video = run("video", "keyboard");
        // The NEW window-template chrome: a corner rune glyph proves it expanded.
        assert!(has(&video, "ᛞ"), "window rune corners render");
        assert!(has(&video, "Video") && has(&video, "Display Mode"), "video section rows");
        assert!(has(&run("audio", "keyboard"), "NOT YET IMPLEMENTED"), "audio stub");
        assert!(has(&run("input", "keyboard"), "MOVEMENT"), "input keyboard groups");
        assert!(has(&run("input", "mouse"), "Look Sensitivity"), "input mouse rows");
        // Controller tab is now a data-driven profile SELECTOR (§7.3): the refreshed notes
        // copy renders, and the selected profile (`ctrl_profile = "default"`) shows the
        // label PROFILES supplied — proving the selector options came from the data global.
        let controller = run("input", "controller");
        assert!(has(&controller, "Choose a control profile"), "refreshed controller notes");
        assert!(
            has(&controller, "Default (Keyboard & Mouse)"),
            "selector shows the PROFILES-driven label for the active profile"
        );
    }

    /// **A settings dropdown pick is an INDEX, and an index is a NUMBER.** The real
    /// `settings.lua` resolution select, walked through the real engine-tier `select`
    /// (`component.rs`): the picked row lands on `video_resolution` as a number
    /// (never a stringified index),
    /// and that number names a DIFFERENT resolution than the one published — which is
    /// exactly the change `update` applies to the window.
    #[test]
    fn a_resolution_pick_reports_a_number_that_changes_the_resolution() {
        use flicker::render::Vec2;

        flicker::ui::strings::load_str(SHELL_STRINGS_JSON, "en-us");
        let styles = load_styles_str(SHELL_UI_JSON);
        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua")
            .expect("load settings.lua");
        host.set_texture_ids(&[("white", 0), ("cell", 1), ("settings_panel", 2)])
            .expect("register textures");
        expose_ui_elements(&host);
        publish_profiles(&host);
        let tree = expand(
            host.ui_tree().expect("settings.lua tree() builds").expect("tree()"),
            &builtin_templates(),
        );

        // The published selection: index 2 (1920×1080), the shell's own default rung.
        const SHOWN: usize = 2;
        const PICK: usize = 0;
        let mut m = ValueMap::new();
        for id in ["video", "audio", "input"] {
            m.set(format!("sec_{id}"), id == "video");
            m.set(format!("nav_{id}_style"), "modal.buttons.variants.secondary");
        }
        for id in INPUT_SUBTABS {
            m.set(format!("sub_{id}"), id == "keyboard");
        }
        m.set("kicker", "");
        m.set("sec_title", "");
        m.set("kicker_color_path", "theme.tokens.sig_blue");
        m.set("input_subtab", 0.0);
        m.set("ctrl_profile", 0.0);
        m.set("scroll_off", 0.0);
        m.set("off", false);
        m.set("rebinding", false);
        m.set("applied", false);
        m.set("video_display_mode", 0.0);
        m.set("video_resolution", SHOWN as f64);
        m.set("video_quality", 2.0);
        m.set("video_vsync", true);
        m.set("video_fps_limit", 1.0);
        m.set("look_sens_pct", 50.0);
        m.set("input_mouse_invert_pitch", false);

        let at = |x: f32, y: f32, clicked: bool| UiInput {
            mouse: Vec2::new(x, y),
            clicked,
            down: clicked,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let mut state = UiState::new();

        // Where the layout actually put the resolution dropdown this frame.
        let idle = run_ui(&tree, &m, &styles, &at(-9.0, -9.0, false), &mut state);
        let [x, y, w, h] = idle
            .rects
            .iter()
            .find(|(id, _)| id == "c_resolution")
            .map(|(_, r)| *r)
            .expect("the resolution select is placed");
        assert!(w > 0.0 && h > 0.0, "the select has real extent");

        // Click the field to open, then the PICK-th popup row (rows start 6px under
        // the field, `settings.controls.menu.row_h` = 30 tall).
        run_ui(&tree, &m, &styles, &at(x + w * 0.5, y + h * 0.5, true), &mut state);
        let row_y = y + h + 6.0 + 30.0 * PICK as f32 + 15.0;
        let f = run_ui(&tree, &m, &styles, &at(x + 20.0, row_y, true), &mut state);

        assert_eq!(
            f.results.number("video_resolution"),
            Some(PICK as f64),
            "the picked row reports its index as a NUMBER"
        );
        assert_eq!(f.results.text("video_resolution"), None, "…and never as text");
        // …and that index is what moves the window: a different rung than the shown one.
        assert_ne!(
            display::resolution_at(PICK),
            display::resolution_at(SHOWN),
            "the pick names a different resolution — the change `update` applies"
        );
    }

    /// S9 stage 4, end-to-end minus the GPU: `settings.lua`'s ROOT declares
    /// `on_cancel = "settings_close"`, and an Esc press runs the REAL path — the
    /// Menu-context map resolves it to a `Cancel` edge, the router dispatches it
    /// into the walker layer, and the declared intent fires the same result name
    /// the × button emits. The exact seam `UnifiedSettingsScene::update` runs.
    #[test]
    fn esc_in_settings_fires_settings_close_through_the_bus() {
        use flicker_input_router::{InputHandler, RouteCtx, Router};

        // The settings screen, built + expanded exactly as the scene caches it.
        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua")
            .expect("load settings.lua");
        expose_ui_elements(&host);
        publish_profiles(&host);
        let tree = host
            .ui_tree()
            .expect("settings.lua tree() builds")
            .expect("settings.lua exposes tree()");
        let tree = expand(tree, &builtin_templates());

        // The declaration is DATA on the root, collected once like the scene does.
        let intents = UiIntents::of(&tree);
        assert_eq!(
            intents.result_for(ActionSignal::Cancel),
            Some("settings_close"),
            "settings.lua's root declares on_cancel = settings_close"
        );

        // The mini-bus: the canonical Menu map (Esc → Cancel), a fresh resolver,
        // and the walker layer carrying the declaration.
        let menu_map = InputProfile::default_profile()
            .context_map("Menu")
            .cloned()
            .expect("default profile carries a Menu context");
        let bindings = ContextualBindings::new(menu_map);
        let gamepad = GamepadConfig::default();
        let mut resolver = Resolver::new();
        let mut ev: Vec<Fired> = Vec::new();

        // Frame 0 seeds the resolver; frame 1 presses Esc → a Cancel press edge.
        let idle = InputState::new();
        resolver.resolve_frame(&bindings, &gamepad, &idle, 0, &mut ev);
        assert!(ev.is_empty());
        let mut esc = InputState::new();
        esc.set_key(Key::Escape, true);
        resolver.resolve_frame(&bindings, &gamepad, &esc, 1, &mut ev);
        assert!(
            ev.iter().any(|f| f.signal == ActionSignal::Cancel),
            "Esc resolves to Cancel under the Menu map"
        );

        let events: Vec<InputEvent> = ev
            .iter()
            .map(|f| InputEvent::from_fired(f, bindings.active(), &esc))
            .collect();
        let mut ui = UiState::new();
        let mut route = RouteCtx::new();
        let model = ValueMap::new();
        let mut walker =
            WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model).with_intents(&intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        assert_eq!(
            walker.take_fired(),
            vec!["settings_close".to_string()],
            "the Esc press fired the DECLARED intent through the bus"
        );
    }

    /// The dirty-state ladder (Aaron's praised flow, S9-preserved): a close
    /// intent with unsaved edits raises the confirm dialog instead of popping;
    /// while a dialog is up the SAME intent is intercepted (dismisses the
    /// dialog, never closes the overlay under it); a clean close pops. Exercises
    /// the real extracted ladder slice (`modal_flow` / `close_requested`) plus
    /// the dialogs' S9 context wiring.
    #[test]
    fn settings_dialogs_intercept_the_close_intent() {
        use flicker_input_core::InputContext;
        use flicker_input_router::RouteCtx;

        let close = ValueMap::new().with("settings_close", true);
        let mut surfaces = settings_surfaces();

        // Dirty close → the confirm dialog goes up, nothing pops.
        assert!(!UnifiedSettingsScene::close_requested(&mut surfaces, &close, true));
        assert!(surfaces.is_on("confirm_close"), "dirty close raises the confirm dialog");

        // Its context flip routes through the S9 seam: Menu pushed while up.
        let mut route = RouteCtx::new();
        let mut bindings = ContextualBindings::new(InputMap::empty());
        surfaces.apply_surface_contexts(&mut route);
        apply_context_requests(&mut bindings, &route.requests);
        route.requests.clear();
        assert_eq!(bindings.active(), InputContext::Menu, "the dialog holds its declared context");

        // The dialog INTERCEPTS the next close intent: it dismisses the dialog
        // (Esc = Cancel), the overlay itself stays open.
        assert_eq!(
            UnifiedSettingsScene::modal_flow(&mut surfaces, &close),
            Some(ModalFlow::Stay)
        );
        assert!(!surfaces.is_on("confirm_close"), "the intent cancelled the dialog, not settings");
        surfaces.apply_surface_contexts(&mut route);
        apply_context_requests(&mut bindings, &route.requests);
        route.requests.clear();
        assert_eq!(bindings.active(), InputContext::World, "…and its context popped with it");

        // Save / Discard resolve through the dialog as before.
        surfaces.show("confirm_close");
        let save = ValueMap::new().with("confirm_save", true);
        assert_eq!(
            UnifiedSettingsScene::modal_flow(&mut surfaces, &save),
            Some(ModalFlow::CommitAndPop)
        );
        let discard = ValueMap::new().with("confirm_discard", true);
        assert_eq!(
            UnifiedSettingsScene::modal_flow(&mut surfaces, &discard),
            Some(ModalFlow::Pop)
        );
        surfaces.hide("confirm_close");

        // The restore ack intercepts the close intent the same way.
        surfaces.show("restore_note");
        assert_eq!(
            UnifiedSettingsScene::modal_flow(&mut surfaces, &close),
            Some(ModalFlow::Stay)
        );
        assert!(!surfaces.is_on("restore_note"), "the intent dismissed the ack");

        // No dialog + clean → the close intent pops.
        assert_eq!(UnifiedSettingsScene::modal_flow(&mut surfaces, &close), None);
        assert!(UnifiedSettingsScene::close_requested(&mut surfaces, &close, false));
    }

    #[test]
    fn launcher_tiers_route_scenes_by_realm() {
        // The mode-launcher map (Aaron-ratified): the ROOT menu lists the three mode
        // buttons + realm-less launch buttons (Click Trainer) + chrome; each tier-2
        // page lists ONLY its realm's info-bearing scenes as panel rows — Adventurer
        // exactly Solar Birth, Developer the dev tools (and NOT solarbirth /
        // clicktrainer), DM nothing (its page is the note).
        fn dummy() -> Box<dyn Scene> {
            unreachable!("items()/scene_rows() read metadata only — never call the factory")
        }
        set_scenes(vec![
            SceneEntry::new("solarbirth", "Solar Birth", "primary", dummy)
                .with_realm(REALM_ADVENTURER)
                .with_info(SceneInfo::new("Solar Birth", "Cinematic", "Celestial", "d", "m")),
            SceneEntry::new("clicktrainer", "CLICK TRAINER", "primary", dummy),
            SceneEntry::new("pocclusters", "Cluster Editor", "primary", dummy)
                .with_realm(REALM_DEVELOPER)
                .with_info(SceneInfo::new("Cluster Editor", "Tool", "CSG / Voxel", "d", "m")),
            SceneEntry::new("pocepochs", "Planet Simulation", "primary", dummy)
                .with_realm(REALM_DEVELOPER)
                .with_info(SceneInfo::new("Planet Simulation", "Simulation", "World-Gen", "d", "m")),
        ]);
        SCENE_SELECT.with(|s| s.set(true));

        // ROOT: the three modes, the realm-less info-less minigame, the chrome — and
        // NO panel rows (the root is the popup alone; the launcher panel is tier 2).
        let root = MenuScene::new();
        let ids: Vec<String> = root.items().iter().map(|i| i.id.clone()).collect();
        assert_eq!(
            ids,
            [
                "mode_adventurer",
                "mode_dm",
                "mode_gamemaster",
                "mode_developer",
                "clicktrainer",
                "settings",
                "quit"
            ]
        );
        assert!(root.scene_rows().is_empty(), "the root menu publishes no scene rows");
        assert_eq!(root.page().mode, "", "the root is mode-less");

        // ADVENTURER: exactly Solar Birth; popup = BACK + chrome; header suppressed.
        let adventurer = MenuScene::for_mode(REALM_ADVENTURER);
        let ids: Vec<String> = adventurer.items().iter().map(|i| i.id.clone()).collect();
        assert_eq!(ids, ["menu_back", "settings", "quit"]);
        let rows: Vec<String> = adventurer.scene_rows().iter().map(|r| r.id.clone()).collect();
        assert_eq!(rows, ["solarbirth"]);
        assert!(!adventurer.page().panel_head, "adventurer page drops the panel header");
        assert!(adventurer.page().note.is_empty(), "adventurer page carries no note");

        // DEVELOPER: the dev tools — and NOT solarbirth (moved) or clicktrainer (root).
        let developer = MenuScene::for_mode(REALM_DEVELOPER);
        let rows: Vec<String> = developer.scene_rows().iter().map(|r| r.id.clone()).collect();
        assert_eq!(rows, ["pocclusters", "pocepochs"]);
        assert!(developer.page().panel_head, "developer launcher keeps its header");

        // DM: no scenes; the page carries the under-construction note token.
        let dm = MenuScene::for_mode(REALM_DM);
        assert!(dm.scene_rows().is_empty());
        assert_eq!(dm.page().note, "$dm_coming_soon");

        // A shared tool lists on EVERY realm it is tagged with (multi-mode).
        set_scenes(vec![SceneEntry::new("sharedtool", "Shared Tool", "primary", dummy)
            .with_realm(REALM_ADVENTURER)
            .with_realm(REALM_DEVELOPER)
            .with_info(SceneInfo::new("Shared Tool", "Tool", "Both", "d", "m"))]);
        for realm in [REALM_ADVENTURER, REALM_DEVELOPER] {
            let rows = MenuScene::for_mode(realm).scene_rows();
            assert_eq!(rows.len(), 1, "shared tool lists in '{realm}'");
        }
    }
}

#[cfg(test)]
mod persistence {
    //! The spec §7.2 fix: a keybind rebind must survive relaunch. `save`/`load` go
    //! through the real `settings.json` (a user path, so the true relaunch round-trip is
    //! an IN-WINDOW check), but they serialize with `serde_json::to_vec_pretty` /
    //! `from_slice` — exactly what these tests exercise here, on the same private
    //! `GameSettings`, with the same `set_context_map("World", …)` mutation
    //! `commit_settings` performs. The analogue of the `InputMapData` round-trip test.
    use super::*;
    use flicker_input_core::InputBinding;

    /// Serialize/deserialize the exact way `GameSettings::{save,load}` do.
    fn round_trip(gs: &GameSettings) -> GameSettings {
        let bytes = serde_json::to_vec_pretty(gs).expect("GameSettings serializes");
        serde_json::from_slice(&bytes).expect("GameSettings deserializes")
    }

    #[test]
    fn rebind_survives_gamesettings_round_trip() {
        // Start from defaults (World = WASD): W is MoveForward.
        let mut gs = GameSettings::default();
        assert_eq!(
            gs.input_profile.context_map("World").unwrap().action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveForward),
        );

        // Rebind MoveForward onto the Up arrow in the World map, then fold it into the
        // profile — the exact step `commit_settings` runs before `save`.
        let mut world = gs.input_profile.context_map("World").cloned().unwrap();
        world.bind(ActionSignal::MoveForward, InputBinding::Key(Key::Up));
        gs.input_profile.set_context_map("World", world);

        // Persist → reload (the save/load serialization path, minus disk).
        let loaded = round_trip(&gs);

        // The seed source (`input_profile().context_map("World")`) now carries the rebind,
        // resolved through the STABLE NAME "World" (spec §7.1a) — this is what the menu
        // settings seed reads at line ~`unwrap_or_else`.
        let reloaded_world = loaded.input_profile.context_map("World").expect("World persists");
        assert_eq!(
            reloaded_world.action_for(InputBinding::Key(Key::Up)),
            Some(ActionSignal::MoveForward),
            "the rebind survived save→load",
        );
    }

    #[test]
    fn old_settings_without_profile_still_load() {
        // A pre-§7.2 settings.json has no `input_profile` key. `#[serde(default)]` must
        // let it load, filling the default profile (World = WASD) — old files still work.
        let legacy = r#"{
            "audio": { "master": 0.8, "music": 0.6, "sfx": 0.7, "voice": 0.9 },
            "video": { "quality": 3, "vsync": true, "fps_limit": 2 },
            "input": {
                "mouse_sensitivity": 0.005, "sprint_sensitivity": 0.005,
                "invert_pitch": false, "invert_yaw": false, "raw_input": true,
                "stick_sensitivity": 2.0, "left_deadzone": 0.2, "right_deadzone": 0.2,
                "trigger_threshold": 0.5, "invert_stick_pitch": false,
                "invert_stick_yaw": false, "deadzone_shape": 0
            }
        }"#;
        let gs: GameSettings = serde_json::from_str(legacy).expect("legacy settings still load");
        assert_eq!(
            gs.input_profile.context_map("World").unwrap().action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveForward),
            "missing profile defaults to WASD World",
        );
    }
}
