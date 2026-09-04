//! Front-end shell scenes + the settings/config model (private to the crate).
//! Only [`run`], [`ShellConfig`], [`PauseScene`], and [`take_pending_input`] are
//! public (re-exported from the crate root); everything else — the splash/menu/
//! settings/pause scenes, their embedded Lua scripts + `ui_theme.json`, and
//! display/settings persistence — is internal.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use flicker::app::run_with_input;
use flicker::render::{FrameGraph, Renderer, TextureHandle};
use flicker::scene::{GotoMode, Scene, SceneInput, SceneManager, Transition};
use flicker::script::{parse_ui_json, HudCommand, ScriptHost, UiAnchor, UiNode, Value, ValueMap};
use flicker::ui::{
    focusables_of, render_hud, run_ui, SceneDef, SceneManifest, Section, Sections, UiInput,
    UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{
    AbstractControls, ActionSignal, ContextualBindings, GamepadConfig, InputBinding, InputContext,
    InputMap, InputProfile, InputState, Key, RebindCapture, SignalGroup,
};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx, Router};

use crate::display;
use crate::theme::Theme;

/// A factory that builds one of the client's scenes when its menu button is hit.
/// `Rc` (not `Box`) so the menu — and a "return to main menu" rebuild — can hold
/// the same factory set any number of times. The shell never names a scene type.
///
/// The factory receives the scene's authored [`SceneDef`] — the entry is the
/// CLIENT half of the behaviour registry, playing the file whose `behaviour`
/// names it. A file-less entry (a single-scene client) gets a synthetic def.
pub type SceneFactory = Rc<dyn Fn(&SceneDef) -> Box<dyn Scene>>;

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
        factory: impl Fn(&SceneDef) -> Box<dyn Scene> + 'static,
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
/// label in `ui_theme.json`; owned here now that the menu is data-driven).
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
    /// The SHIPPED APP's version (`env!("CARGO_PKG_VERSION")` in the client crate —
    /// prism-alpha's, not this library's). Non-empty ⇒ the menu shows it bottom-right
    /// and a one-shot GitHub Releases check may light the UPDATE AVAILABLE chip.
    /// Empty (the [`single`](ShellConfig::single) default) ⇒ no version line, no
    /// network — dev/POC clients never phone home.
    pub app_version: &'static str,
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
        // A single-scene client has no scene file; its factory ignores the
        // synthetic def the resolver hands file-less entries.
        Self {
            scenes: vec![SceneEntry::new("start", label, "primary", move |_| {
                factory()
            })],
            settings_dir,
            scene_select: false,
            app_version: "",
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

/// The one-shot update check's lifecycle (ratified architecture 2026-08-18:
/// in-app NOTIFY only — the launcher, arriving with the IdP work, owns actual
/// patching). Same run()-installs / menu-reads shape as [`SCENES`].
enum UpdateCheck {
    /// No check runs: no `app_version` declared, or the check thread ended
    /// without news (offline, current, or unparsable — all deliberately silent).
    Off,
    Pending(std::sync::mpsc::Receiver<flicker::net::update::UpdateInfo>),
    Done(flicker::net::update::UpdateInfo),
}

thread_local! {
    /// The shipped app's version + the update check's state. Set once by [`run`];
    /// the menu polls per frame. `thread_local` beside [`SCENES`] for the same
    /// reason — the whole shell lives on the winit thread.
    static APP_VERSION: RefCell<&'static str> = const { RefCell::new("") };
    static UPDATE: RefCell<UpdateCheck> = const { RefCell::new(UpdateCheck::Off) };
}

/// Where prism-alpha's releases live — the only repository the shell checks
/// against and the only host [`open_url`] will launch.
const RELEASES_OWNER: &str = "elide-us";
const RELEASES_REPO: &str = "flicker";

/// Advance the update check and hand back the latched result, if any. Drains
/// the receiver at most once per call (the menu calls once per frame).
fn poll_update() -> Option<flicker::net::update::UpdateInfo> {
    use std::sync::mpsc::TryRecvError;
    UPDATE.with(|u| {
        let mut u = u.borrow_mut();
        match &*u {
            UpdateCheck::Off => None,
            UpdateCheck::Done(info) => Some(info.clone()),
            UpdateCheck::Pending(rx) => match rx.try_recv() {
                Ok(info) => {
                    *u = UpdateCheck::Done(info.clone());
                    Some(info)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    // The check finished with nothing to say — stop polling.
                    *u = UpdateCheck::Off;
                    None
                }
            },
        }
    })
}

/// Open a URL in the player's default browser — fire-and-forget, warn on
/// failure. Guarded to our own release pages: the shell launches nothing else.
fn open_url(url: &str) {
    if !url.starts_with(concat!("https://github.com/", "elide-us/")) {
        tracing::warn!("refusing to open non-release URL {url}");
        return;
    }
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let spawned = std::process::Command::new("xdg-open").arg(url).spawn();
    match spawned {
        Ok(_) => tracing::info!("opened release page {url}"),
        Err(e) => tracing::warn!("could not open {url}: {e}"),
    }
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
        let lang = GAME_SETTINGS
            .lock()
            .map(|s| s.language.clone())
            .unwrap_or_default();
        let lang = if lang.is_empty() {
            "en-us".to_string()
        } else {
            lang
        };
        flicker::ui::strings::load_str(SHELL_STRINGS_JSON, &lang);
    }
    SCENE_SELECT.with(|s| s.set(config.scene_select));
    set_scenes(config.scenes);
    // Kick the one-shot update check for clients that declare a version (the
    // shipped game). Background thread; the menu polls; silence on any failure.
    APP_VERSION.with(|v| *v.borrow_mut() = config.app_version);
    if !config.app_version.is_empty() {
        let rx = flicker::net::update::check_github_latest(
            RELEASES_OWNER,
            RELEASES_REPO,
            config.app_version,
        );
        UPDATE.with(|u| *u.borrow_mut() = UpdateCheck::Pending(rx));
    }
    // Every shell client inherits the Prism pointer (theme-tinted hardware
    // cursor); when the pointer is hidden/captured elsewhere it simply isn't
    // shown — no visibility wiring here.
    // Boot from the MANIFEST: the scene folder was indexed on first use, and exactly
    // one file claimed `boot`. The engine never names a scene — it asks the manifest
    // which scene starts, and resolves it through the same roster every later
    // transition uses. The whole chain is authored data the shell can show you.
    let boot = manifest().boot().to_string();
    let manager = SceneManager::from_roster(&boot, Box::new(resolve_shell_scene))
        .unwrap_or_else(|| {
            panic!("boot scene '{boot}' did not resolve — its behaviour is unregistered")
        })
        .with_cursor(crate::theme::cursor_image());
    // Wire the central event pump: the runner resolves the device snapshot into signal
    // events for the active scene's context, from the player's profile bindings. Scenes
    // not yet migrated ignore them and still resolve internally, so this is behaviour-
    // preserving until each is converted.
    let bindings = ContextualBindings::from_profile(&input_profile());
    // The pump adopts a live World rebind each frame (input-P3 / S1c): it re-reads the
    // committed World map from the profile, so a key rebound in settings takes effect
    // without any scene owning a resolver. Non-draining, so un-migrated scenes still pick
    // up their own rebind via `take_pending_input`.
    let result = run_with_input(manager, bindings, GamepadConfig::default(), || {
        Some(current_world_map())
    });
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
        gs.display.res = display::Resolution {
            w: geom.width,
            h: geom.height,
        };
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
        Self {
            master: 0.8,
            music: 0.6,
            sfx: 0.7,
            voice: 0.9,
        }
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
        Self {
            quality: 3,
            vsync: true,
            fps_limit: 2,
        }
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
    GAME_SETTINGS
        .lock()
        .expect("settings lock")
        .input_profile
        .clone()
}

/// The current committed `World` map, cloned on its own (cheaper than [`input_profile`],
/// which clones the whole profile). The runner polls this each frame through the pump's
/// rebind seam (input-P3 / S1c) and writes it into the pump's bindings, so a live World
/// rebind reaches every scene consuming the pump — NON-draining (it never touches
/// [`take_pending_input`]), so a scene still polling its own rebind is unaffected.
pub fn current_world_map() -> InputMap {
    GAME_SETTINGS
        .lock()
        .expect("settings lock")
        .input_profile
        .context_map("World")
        .cloned()
        .unwrap_or_else(InputMap::wasd_and_mouse)
}

/// The intro splashes' pair scripts — ONE SCRIPT PER SCENE (the SceneName.lua
/// half of the pair; 1:1 human content, no shared script). Module form like every
/// pair script: `arrange()` configures the `splash` node's props (image + fade
/// timeline); `react()` turns the fired signals into the `next`/`exit` intents
/// the scene FILE routes.
const TEG_LOGO_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/TegLogo.lua");
const CE_LOGO_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/CeLogo.lua");

/// The pre-load screen's pair script (`Loading.lua`) — the `loading` behaviour's
/// module. `derive()` turns the engine-published `loading_progress` into the
/// percent readout; `react()` maps `done`→`next` / `cancel`→`exit`. Embedded like
/// every shell-scene script (the intro chain is compiled in; only benches load
/// their Lua off disk).
const LOADING_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/Loading.lua");

/// The splash scene's OWN script, by its id — the pair is by NAME.
fn splash_script(id: &str) -> (&'static str, &'static str) {
    match id {
        "CeLogo" => (CE_LOGO_SCRIPT, "CeLogo.lua"),
        _ => (TEG_LOGO_SCRIPT, "TegLogo.lua"),
    }
}

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
        let loaded = SceneManifest::load_dir(&dir)
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
const BEHAVIOURS: &[(&str, BehaviourBuilder)] = &[
    ("splash", build_splash),
    ("menu", build_menu),
    ("loading", build_loading),
];

/// The `splash` behaviour: play ONE image on a fade/hold timeline, then fire
/// `next` (or `exit`, when backed out of) and let the file route it.
fn build_splash(def: &SceneDef) -> Option<Box<dyn Scene>> {
    // The image and the fade timeline come from the pair script's `arrange()`
    // props, applied onto the tree's `splash` node at enter; `params.image`
    // remains the data fallback for a script that names none.
    Some(Box::new(LogoScene::new(def.clone())))
}

/// The `menu` behaviour: the shell's main menu, whose buttons come from the
/// client's registered [`SceneEntry`] set and launch BY ID.
fn build_menu(_def: &SceneDef) -> Option<Box<dyn Scene>> {
    Some(Box::new(MainMenuScene::new()))
}

/// The `loading` behaviour: the pre-load screen (page 3 of the intro chain). A
/// native component tree driven by a SIMULATED progress timer for now — see
/// [`LoadingScene`].
fn build_loading(def: &SceneDef) -> Option<Box<dyn Scene>> {
    Some(Box::new(LoadingScene::new(def.clone())))
}

/// The MAIN MENU's per-realm scene rows: every registered scene that carries `SceneInfo`
/// AND belongs to `realm`, as a [`SceneRow`] the launcher renders in that realm's page.
fn scene_rows_for(scenes: &[SceneEntry], realm: &str) -> Vec<SceneRow> {
    scenes
        .iter()
        .filter(|e| e.realms.iter().any(|r| r == realm))
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

/// Build the MAIN MENU tree: parse `Main.scene.json` (a tree of Rust component KINDS —
/// its former `frame` / `multi_view` / `paged_menu` are inlined onto kinds or native
/// components now, 201F4F51), then fill each realm's `scene_list_<n>` with that realm's
/// scene rows. `menu.lua`'s arrange() lights the page.
fn main_menu_tree(scenes: &[SceneEntry], muse_id: Option<usize>) -> UiNode {
    let empty = || UiNode {
        component: "surface".to_string(),
        ..Default::default()
    };
    let def: serde_json::Value = match serde_json::from_str(MAIN_SCENE_JSON) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Main.scene.json did not parse: {e}");
            return empty();
        }
    };
    let raw = match parse_ui_json(&def["tree"]) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Main.scene.json tree failed to parse: {e}");
            return empty();
        }
    };
    let mut tree = raw;
    // The Muse backdrop sprite (its theme texture index) — injected the same way the old
    // MenuView filled `muse_slot`, so the background image returns.
    if let Some(id) = muse_id {
        if let Some(slot) = find_by_id_mut(&mut tree, "muse_slot") {
            slot.children = vec![muse_sprite(id)];
        }
    }
    // One page slice per realm (1..4 — page 0 is the landing placeholder).
    for (realm, n) in [
        (REALM_ADVENTURER, 1),
        (REALM_DM, 2),
        (REALM_GAMEMASTER, 3),
        (REALM_DEVELOPER, 4),
    ] {
        if let Some(list) = find_by_id_mut(&mut tree, &format!("scene_list_{n}")) {
            list.children = scene_row_nodes(&scene_rows_for(scenes, realm));
        }
    }
    tree
}

/// The MAIN MENU — the Lua-ORCHESTRATED boot menu (the `menu` behaviour). Holds the
/// authored `Main.scene.json` tree + a held `menu.lua` `ScriptHost`; each frame `arrange()`
/// latches the page from the `sig_mode_<realm>` mirror and lights that page's slice. Realm
/// buttons page the PTT; a scene row launches BY ID; Settings / Quit as before.
///
/// Cutover note: the pause overlay's "MAIN MENU" now returns HERE too — `Goto {"Main",
/// ReplaceRoot}` rebuilds this scene, the Stage-B fix for controller scene-selection dying
/// on return (the legacy tier-push menu root had no navigable "scenes" group, so the pad
/// could not reach a scene while the mouse still hit-tested one). That legacy `MenuScene`
/// has since been deleted (task #6 / MCP 5099BC88), leaving this the single menu.
struct MainMenuScene {
    theme: Option<Theme>,
    view: Option<MenuView>,
    pending_input: Option<InputMap>,
    scenes: Rc<[SceneEntry]>,
}

impl MainMenuScene {
    fn new() -> Self {
        Self {
            theme: None,
            view: None,
            pending_input: None,
            scenes: scenes(),
        }
    }

    /// Map this frame's fired result names to a transition. A `mode_<realm>` result is NOT
    /// routed here — it fires, the engine mirrors `sig_mode_<realm>` one frame, and
    /// `menu.lua`'s arrange() latches the page; the nav buttons drive the PTT, no tier.
    fn route(&mut self, results: &ValueMap, renderer: &Renderer) -> Transition {
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
            let input_map = self.pending_input.take().unwrap_or_else(|| {
                input_profile()
                    .context_map("World")
                    .cloned()
                    .unwrap_or_else(InputMap::wasd_and_mouse)
            });
            return Transition::Push(Box::new(UnifiedSettingsScene::new(
                theme, &input_map, renderer,
            )));
        }
        if results.is_on("quit") {
            return Transition::Quit;
        }
        if results.is_on("open_update_page") {
            // Inline side effect, no transition: the player stays in the menu
            // while the release page opens in their browser.
            if let Some(info) = poll_update() {
                open_url(&info.url);
            }
        }
        Transition::None
    }

    /// The menu's per-frame model: the shipped version (bottom-right text) and
    /// the update chip's visibility. Values only — every display STRING in the
    /// tree is a `$token` (the chip's label lives in the stringtable).
    fn model(&self) -> ValueMap {
        let mut model = ValueMap::new();
        let version = APP_VERSION.with(|v| *v.borrow());
        if !version.is_empty() {
            model.set("app_version", format!("v{version}"));
        }
        if poll_update().is_some() {
            model.set("update_available", true);
        }
        model
    }
}

impl Scene for MainMenuScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        let theme = Theme::build(renderer);
        let muse_id = theme
            .lua_textures()
            .iter()
            .position(|(name, _)| *name == "muse");
        let tree = main_menu_tree(&self.scenes, muse_id);
        match ScriptHost::new(MENU_SCRIPT, "Main.lua") {
            Ok(script) => self.view = Some(MenuView::from_tree(&theme, tree, Some(script))),
            Err(e) => tracing::error!("Main.lua failed to load — the menu will not page: {e}"),
        }
        self.theme = Some(theme);
    }

    /// Menu context: the pump resolves arrows/d-pad → `Nav*`, Enter/A → `Confirm`,
    /// Esc/B → `Cancel` for the launcher (the map MenuView's walker consumes).
    fn input_context(&self) -> Option<InputContext> {
        Some(InputContext::Menu)
    }

    fn update(
        &mut self,
        _dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        let model = self.model();
        let results = match self.view.as_mut() {
            Some(view) => view.update(signals, input, renderer, &model),
            None => return Transition::None,
        };
        self.route(&results, renderer)
    }

    fn render<'f>(&'f mut self, _renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        if let Some(view) = self.view.as_ref() {
            fg.overlay(move |r| view.render(r));
        }
    }
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
    // Through the seam, not fs::read: an installed build serves this from the
    // mounted package.flk; a loose dev PNG reads identically (raw fallback).
    match flicker_content::package::read_bytes(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!(
                "scene '{id}': image {} could not be read: {e}",
                path.display()
            );
            Vec::new()
        }
    }
}

/// The shell's scene roster — the ids [`Transition::Goto`] resolves through.
///
/// The MANIFEST is authoritative: a scene the human authored is looked up by id and
/// built from the behaviour its file names — a SHELL behaviour ([`BEHAVIOURS`]) or a
/// CLIENT one (the registered [`SceneEntry`] whose id the file's `behaviour` names,
/// its factory receiving the def). So every launchable scene is a file in ONE folder,
/// and the roster entry is only the launcher metadata + the Rust that plays it.
/// Authored scenes win over any same-named entry, so a client cannot shadow the boot
/// chain with a bench of the same name.
///
/// A FILE-LESS registered entry (a single-scene client's `start`) still resolves,
/// with a synthetic def — that path carries no authored tree and never shadows a file.
///
/// No scene id appears in this function, and none may: that is the rule the manifest
/// exists to keep.
fn resolve_shell_scene(id: &str) -> Option<Box<dyn Scene>> {
    if let Some(def) = manifest().get(id) {
        if let Some((_, build)) = BEHAVIOURS.iter().find(|(name, _)| *name == def.behaviour) {
            return build(def);
        }
        if let Some(e) = scenes().iter().find(|e| e.id == def.behaviour) {
            return Some((e.factory)(def));
        }
        tracing::error!(
            "scene '{id}' names behaviour '{}', which is neither a shell behaviour \
             {:?} nor a registered scene entry — a scene file alone does not make a \
             scene run",
            def.behaviour,
            BEHAVIOURS.iter().map(|(n, _)| *n).collect::<Vec<_>>()
        );
        return None;
    }
    // Not an authored scene — a file-less registered entry (single-scene client).
    scenes()
        .iter()
        .find(|e| e.id == id)
        .map(|e| (e.factory)(&synthetic_def(id)))
}

/// The def a FILE-LESS roster entry is built with: id + behaviour = the entry's own
/// id, no tree, no exits. Exists so [`SceneFactory`] has one signature — a factory
/// that ignores its def costs nothing, and one that wants a tree fails loudly on
/// this (empty) one rather than silently building a different scene.
fn synthetic_def(id: &str) -> SceneDef {
    SceneDef::parse(id, &format!("{{\"behaviour\": {:?}}}", id))
        .expect("a bare behaviour-only scene def parses")
}

/// The behaviour names the SHELL builds itself (`splash`, `menu`, …) — exported so a
/// client's manifest gate can assert every OTHER behaviour has a registered entry.
pub fn builtin_behaviours() -> Vec<&'static str> {
    BEHAVIOURS.iter().map(|(n, _)| *n).collect()
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

/// The intents an intro splash fires for its scene FILE to route — the file maps
/// each name to the scene that follows; this crate never learns what those are.
/// `next` = the timeline completed or the player confirmed; `exit` = backed out.
const SPLASH_NEXT: &str = "next";
const SPLASH_EXIT: &str = "exit";

/// The signals reported to the pair script's `react()`: the splash component's
/// timeline completing, and the two skip activations off the ONE bus.
const SIG_DONE: &str = "done";
const SIG_CONFIRM: &str = "confirm";
const SIG_CANCEL: &str = "cancel";

/// Intro splash: plays ONE logo on the Rust `splash` component, reports the
/// fired signals to its pair script's `react()`, and fires the returned intent
/// ([`SPLASH_NEXT`] / [`SPLASH_EXIT`]) for its scene file to route.
struct LogoScene {
    /// This splash's scene FILE — its tree (or a synthesized one), its styles,
    /// and its `exits` (the routing DATA for the fired intents).
    def: SceneDef,
    /// The PAIR SCRIPT host (`<Id>.lua`), module form like every pair script:
    /// `arrange()` = the splash node's image + fade-timeline props, applied at
    /// enter; `react(sig)` = signals → the `next`/`exit` intent.
    script: Option<ScriptHost>,
    /// The walked tree: the def's authored tree, or a synthesized full-bleed
    /// `splash` node — with the script's arrange() props applied onto it.
    tree: Option<UiNode>,
    /// Token-resolved styles (embedded trio + this scene's own blocks).
    styles: serde_json::Value,
    ui_state: UiState,
    /// Draw commands from `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// `[white, logo]` — index 1 is the splash node's `tex`.
    textures: Vec<TextureHandle>,
    /// The logo's native pixel size — published to the Model for contain-fit.
    size: (u32, u32),
    elapsed: Duration,
}

impl LogoScene {
    /// A splash playing one image, routed by the scene file `def`.
    ///
    /// ONE image per scene. This used to be a single scene walking a LIST of logos on
    /// one Lua timeline, which hid "publisher, then engine, then menu" inside a content
    /// array. Each splash is now its own registered scene and the order is authored in
    /// its file — the same mechanism the main menu uses to launch a bench.
    fn new(def: SceneDef) -> Self {
        Self {
            def,
            script: None,
            tree: None,
            styles: serde_json::Value::Object(Default::default()),
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            textures: Vec::new(),
            size: (1, 1),
            elapsed: Duration::ZERO,
        }
    }

    /// The effective fade timeline: the splash node's props (authored in the file
    /// and/or overridden by the pair script's arrange()), then the component's own
    /// defaults — the same values `draw_splash` reads, so the clock and the drawn
    /// ramp cannot disagree.
    fn timeline_total(&self) -> f32 {
        let node = self.tree.as_ref().and_then(|t| find_splash(t));
        let num = |key: &str, dflt: f32| {
            node.and_then(|n| n.props.get(key))
                .and_then(|v| match v {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                })
                .unwrap_or(dflt)
        };
        let fade_in = num("fade_in", 0.6);
        // `draw_splash` defaults an unauthored fade-out to the fade-in — mirror it.
        fade_in + num("hold", 1.2) + num("fade_out", fade_in)
    }

    /// Per-frame model: elapsed seconds + the logo's native size.
    fn model(&self) -> ValueMap {
        ValueMap::new()
            .with("elapsed", self.elapsed.as_secs_f32())
            .with("img_w", self.size.0)
            .with("img_h", self.size.1)
    }
}

/// **The splash's ONE input responder** — the same [`InputHandler`] shape the benches'
/// scene roots use (`route.rs`), just a one-layer chain. It SUBSCRIBES to the Menu
/// activations that skip the intro — Confirm (A / Enter / Space), Cancel (B), Menu
/// (Esc / Start) — and CONSUMES them off the ONE bus, so the splash reacts to routed
/// signals instead of polling the raw event list in its update loop.
#[derive(Default)]
struct SplashSkip {
    /// Confirm (A / Enter / Space) — advance to the NEXT scene.
    advance: bool,
    /// Cancel / Menu (B / Esc / Start) — back out to the EXIT target.
    back_out: bool,
}

impl InputHandler for SplashSkip {
    fn subscribes(&self, signal: ActionSignal) -> bool {
        matches!(
            signal,
            ActionSignal::Confirm | ActionSignal::Cancel | ActionSignal::Menu
        )
    }

    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        match ev.signal {
            ActionSignal::Confirm => self.advance = true,
            _ => self.back_out = true,
        }
        Flow::Consumed
    }
}

/// Find the tree's splash node — the image node the pair script configures — by its STABLE
/// `id == "splash"`, NOT by kind. The `splash` component was folded into `sprite` (a splash is
/// a `sprite` with a fade timeline), and `apply_props` already targets this node by that same
/// id; keying on the retired `"splash"` kind found nothing, so `enter()` never read `image`
/// nor set `tex` and the logo rendered blank.
fn find_splash(node: &UiNode) -> Option<&UiNode> {
    if node.id == "splash" {
        return Some(node);
    }
    node.children.iter().find_map(find_splash)
}

/// [`find_splash`], mutably — for the engine-owned props (the loaded texture's index).
fn find_splash_mut(node: &mut UiNode) -> Option<&mut UiNode> {
    if node.id == "splash" {
        return Some(node);
    }
    node.children.iter_mut().find_map(find_splash_mut)
}

/// The synthesized tree for a splash scene whose file authors none: one
/// full-bleed `splash` node — the minimal arrangement, so an intro scene is a
/// PNG + a pair script + a scene file with no tree required.
fn synthesized_splash_tree() -> UiNode {
    let mut node = UiNode {
        // `sprite` (id "splash") — the `splash` kind was folded into `sprite`; a file-less
        // splash gets a bare full-bleed sprite the pair script configures with a fade.
        component: "sprite".to_string(),
        id: "splash".to_string(),
        anchor: UiAnchor::from_name("top_left"),
        ..Default::default()
    };
    node.props
        .insert("width_frac".to_string(), Value::Number(1.0));
    node.props
        .insert("height_frac".to_string(), Value::Number(1.0));
    let mut root = UiNode {
        component: "surface".to_string(),
        anchor: UiAnchor::from_name("top_left"),
        children: vec![node],
        ..Default::default()
    };
    root.props
        .insert("width_frac".to_string(), Value::Number(1.0));
    root.props
        .insert("height_frac".to_string(), Value::Number(1.0));
    root
}

impl Scene for LogoScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // 1 — the PAIR SCRIPT: a module like every pair script; its arrange()
        // carries the splash node's prop overrides (image + fade timeline).
        let (script_src, script_name) = splash_script(&self.def.id);
        match ScriptHost::new(script_src, script_name) {
            Ok(script) => self.script = Some(script),
            Err(e) => tracing::error!("splash pair script failed to load: {e}"),
        }
        // A file that cannot route `next` leaves the splash playing forever —
        // reported HERE, once, at enter, not per-frame and not silently.
        if !self.def.exits.contains_key(SPLASH_NEXT) {
            tracing::error!(
                "splash '{}' has no `{SPLASH_NEXT}` exit — it will play and then sit there",
                self.def.id
            );
        }

        // 2 — the tree: authored, or the synthesized full-bleed splash node; the
        // pair script's arrange() props are APPLIED onto it (Lua configuring the
        // features of the Rust-owned component — never rebuilding structure).
        let mut tree = self
            .def
            .tree
            .clone()
            .unwrap_or_else(synthesized_splash_tree);
        if let Some(script) = &self.script {
            match script.arrange() {
                Ok(Some(a)) => a.apply_props(&mut tree),
                Ok(None) => tracing::warn!(
                    "splash '{}': pair script exposes no arrange() — authored props only",
                    self.def.id
                ),
                Err(e) => tracing::error!("splash '{}': arrange() failed: {e}", self.def.id),
            }
        }

        // 3 — the image: the splash node's `image` prop (content-relative), with
        // `params.image` as the data fallback; the loaded texture is index 1.
        let rel = find_splash(&tree)
            .and_then(|n| n.props.get("image"))
            .and_then(|v| match v {
                Value::Text(t) => Some(t.clone()),
                _ => None,
            })
            .or_else(|| self.def.param_str("image").map(str::to_string));
        let image = match rel {
            Some(rel) => splash_image(&self.def.id, &rel),
            None => {
                tracing::error!(
                    "splash '{}': no `image` prop from its pair script and no params.image — \
                     backdrop only",
                    self.def.id
                );
                Vec::new()
            }
        };
        let mut textures = vec![renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1)];
        let mut sizes = (1, 1);
        if let Some((handle, w, h)) = load_image_texture(renderer, &image) {
            textures.push(handle);
            sizes = (w, h);
        }
        self.textures = textures;
        self.size = sizes;
        if let Some(node) = find_splash_mut(&mut tree) {
            node.props.insert("tex".to_string(), Value::Number(1.0));
        }
        self.tree = Some(tree);
        self.styles = flicker::ui::load_styles_strs_for(
            &[SHELL_UI_JSON, SHELL_STYLE_JSON],
            self.def.styles.as_ref(),
        );

        // Apply the persisted (or default) display setting now the window
        // exists — so a saved fullscreen/resolution choice takes effect at
        // launch.
        display::current().apply(renderer);
    }

    /// The splash runs in the **Menu** input context, so the pump resolves this frame's
    /// device input into Menu SIGNALS (Confirm = A / Enter / Space, Cancel = B, Menu =
    /// Esc / Start — all rebindable) rather than the World default (which has no Confirm).
    fn input_context(&self) -> Option<InputContext> {
        Some(InputContext::Menu)
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        self.elapsed += dt;
        // Skip is a SIGNAL on the ONE bus, not a raw key (rule 37722F91): Confirm
        // (A / Enter / Space, or a pointer click) ADVANCES; Cancel / Menu (B /
        // Esc / Start) BACKS OUT. The pair script's react() maps them to the
        // fired intent. Same responder shape the benches use, a one-layer chain.
        let mut root = SplashSkip::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut root];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let advance = root.advance || input.mouse_left_pressed;

        // The walker draws the Rust `splash` component from the frame's model; the
        // component owns the fade math, this scene owns only the clock + routing.
        if let Some(tree) = self.tree.as_ref() {
            let size = renderer.size();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                right_down: input.mouse_right,
                screen: size,
                wheel: 0.0,
                exclusive: false,
                motion: Default::default(),
            };
            let frame = run_ui(tree, &self.model(), &self.styles, &snap, &mut self.ui_state);
            self.hud_commands = frame.commands;
        }

        let done = self.elapsed.as_secs_f32() >= self.timeline_total();
        if !(done || advance || root.back_out) {
            return Transition::None;
        }
        // Something fired. Report the signals to the pair script's react() — the
        // ORCHESTRATION: it returns the intent (`next` / `exit`), which is FIRED
        // as this scene's result and routed by the scene FILE's exits. Called only
        // on a firing frame, never per frame.
        let mut sig = ValueMap::new();
        if done {
            sig.set(SIG_DONE, true);
        }
        if advance {
            sig.set(SIG_CONFIRM, true);
        }
        if root.back_out {
            sig.set(SIG_CANCEL, true);
        }
        if let Some(script) = &self.script {
            match script.react(&sig) {
                Ok(Some(intents)) => {
                    if intents.is_on(SPLASH_EXIT) {
                        return Transition::Fire(SPLASH_EXIT.to_string());
                    }
                    if intents.is_on(SPLASH_NEXT) {
                        return Transition::Fire(SPLASH_NEXT.to_string());
                    }
                    // The script ignored a skip — its call. A completed timeline
                    // with no intent would strand the player, so that advances.
                    if !done {
                        return Transition::None;
                    }
                    tracing::error!(
                        "splash '{}': timeline complete but react() named no intent — advancing",
                        self.def.id
                    );
                }
                Ok(None) => {} // react-less script: the engine mapping below.
                Err(e) => tracing::error!("splash '{}': react() failed: {e}", self.def.id),
            }
        }
        // No script to decide: Cancel backs out, everything else advances.
        if root.back_out {
            return Transition::Fire(SPLASH_EXIT.to_string());
        }
        Transition::Fire(SPLASH_NEXT.to_string())
    }

    fn render<'f>(&'f mut self, _renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        if let Some(&white) = self.textures.first() {
            let hud_commands = &self.hud_commands;
            let textures = &self.textures;
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
        }
    }

    /// A splash routes its fired intents (`next` / `exit`) through its scene file's
    /// exits — the kernel calls this after the scene fires [`Transition::Fire`].
    fn route(&self, result: &str) -> Option<Transition> {
        self.def.exit(result)
    }
}

/// The `loading` behaviour: the pre-load screen, page 3 of the intro chain
/// (TegLogo → CeLogo → **Loading** → Main).
///
/// Unlike the two logo splashes ([`LogoScene`] — a single full-bleed sprite on a
/// fade timeline), this walks a NATIVE component tree from its scene file: a dark
/// backdrop, the do-not-close notice, and a `resource_gauge` progress bar. The
/// shader-compile phase is a SIMULATED timer for now — `elapsed / params.seconds`
/// drives `loading_progress` (0..1), which the bar BINDS and the pair script's
/// `derive()` turns into the percent readout (`set_model`→`derive`→fold, the same
/// split the benches use). When the timeline completes the scene fires
/// [`SPLASH_NEXT`] and the file routes it — that completion is the seam the REAL
/// pre-load will gate. Cancel/Esc backs out via [`SPLASH_EXIT`]; a Confirm/click is
/// reported but the pair script ignores it, so a load in progress can't be skipped.
struct LoadingScene {
    /// This scene's FILE — its native tree, its styles, its `params` (the sim
    /// duration) and its `exits` (routing DATA for the fired intents).
    def: SceneDef,
    /// The PAIR SCRIPT host (`Loading.lua`), module form: `derive()` = the percent
    /// readout, `react(sig)` = signals → the `next`/`exit` intent.
    script: Option<ScriptHost>,
    /// The walked tree — the file's authored tree with any `arrange()` props applied.
    tree: Option<UiNode>,
    /// Token-resolved styles (embedded trio + this scene's own blocks).
    styles: serde_json::Value,
    ui_state: UiState,
    /// Draw commands from `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// The 1×1 white texture every solid HudCommand samples (this scene loads no
    /// images — the notice is components, not a baked PNG).
    white: Option<TextureHandle>,
    elapsed: Duration,
}

impl LoadingScene {
    fn new(def: SceneDef) -> Self {
        Self {
            def,
            script: None,
            tree: None,
            styles: serde_json::Value::Object(Default::default()),
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            white: None,
            elapsed: Duration::ZERO,
        }
    }

    /// The simulated load duration (seconds): `params.seconds`, floored to a sane
    /// minimum so the bar always has a span to fill and `done` can never fire on
    /// frame one from a zero/negative author value.
    fn sim_total(&self) -> f32 {
        self.def
            .params
            .get("seconds")
            .and_then(serde_json::Value::as_f64)
            .map_or(6.0, |s| s as f32)
            .max(0.5)
    }

    /// This frame's Model: the raw `loading_progress` (0..1) the ENGINE publishes,
    /// then the pair script's derived display values folded over it (the percent
    /// readout) — the same `set_model`→`derive`→fold split the benches run.
    fn model(&self, progress: f32) -> ValueMap {
        let raw = ValueMap::new().with("loading_progress", f64::from(progress));
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("loading '{}': publishing raw vars failed: {e}", self.def.id);
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("loading '{}': derive() failed: {e}", self.def.id),
            }
        }
        m
    }
}

impl Scene for LoadingScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // 1 — the PAIR SCRIPT (module form, like every pair script).
        match ScriptHost::new(LOADING_SCRIPT, "Loading.lua") {
            Ok(script) => self.script = Some(script),
            Err(e) => tracing::error!("loading pair script failed to load: {e}"),
        }
        // A file that cannot route `next` leaves the bar full and the scene stuck —
        // reported HERE, once, at enter, not per-frame and not silently.
        if !self.def.exits.contains_key(SPLASH_NEXT) {
            tracing::error!(
                "loading '{}': no `{SPLASH_NEXT}` exit — it will fill and then sit there",
                self.def.id
            );
        }
        // 2 — the tree: the native notice comes straight off the scene FILE (a
        // loading screen is DATA). The pair script's arrange() props, if any, are
        // applied onto it (parity with every pair script; `Loading.lua` has none).
        let mut tree = self.def.tree.clone().unwrap_or_else(|| {
            tracing::error!(
                "loading '{}': scene file has no `tree` — nothing to draw",
                self.def.id
            );
            UiNode {
                component: "surface".to_string(),
                ..Default::default()
            }
        });
        if let Some(script) = &self.script {
            match script.arrange() {
                Ok(Some(a)) => a.apply_props(&mut tree),
                Ok(None) => {}
                Err(e) => tracing::error!("loading '{}': arrange() failed: {e}", self.def.id),
            }
        }
        self.tree = Some(tree);
        self.styles = flicker::ui::load_styles_strs_for(
            &[SHELL_UI_JSON, SHELL_STYLE_JSON],
            self.def.styles.as_ref(),
        );
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // Apply the persisted (or default) display setting now the window exists.
        display::current().apply(renderer);
    }

    /// The loading screen runs in the **Menu** input context so the pump resolves
    /// both Confirm (A / click — clicks through) and Cancel (B / Esc — backs out).
    fn input_context(&self) -> Option<InputContext> {
        Some(InputContext::Menu)
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        self.elapsed += dt;
        // Skip is a SIGNAL on the ONE bus, not a raw key (rule 37722F91): Cancel /
        // Menu (B / Esc / Start) BACKS OUT; Confirm (A / click) CLICKS THROUGH —
        // `Loading.lua` now routes both to a transition. Same responder shape the
        // splashes use, a one-layer chain.
        let mut root = SplashSkip::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut root];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let advance = root.advance || input.mouse_left_pressed;

        // The simulated shader-compile progress: the fraction of the sim duration
        // elapsed, clamped. The bar binds `loading_progress`; `done` is the full bar.
        let progress = (self.elapsed.as_secs_f32() / self.sim_total()).clamp(0.0, 1.0);

        // Walk the native tree from this frame's model (raw progress + derived
        // percent). The walker draws the components; this scene owns only the clock.
        if let Some(tree) = self.tree.as_ref() {
            let size = renderer.size();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                right_down: input.mouse_right,
                screen: size,
                wheel: 0.0,
                exclusive: false,
                motion: Default::default(),
            };
            let frame = run_ui(
                tree,
                &self.model(progress),
                &self.styles,
                &snap,
                &mut self.ui_state,
            );
            self.hud_commands = frame.commands;
        }

        let done = progress >= 1.0;
        if !(done || advance || root.back_out) {
            return Transition::None;
        }
        // Something fired. Report the signals to the pair script's react() — the
        // ORCHESTRATION returns the intent (`next` / `exit`), fired as this scene's
        // result and routed by the scene FILE's exits. Called only on a firing frame.
        let mut sig = ValueMap::new();
        if done {
            sig.set(SIG_DONE, true);
        }
        if advance {
            sig.set(SIG_CONFIRM, true);
        }
        if root.back_out {
            sig.set(SIG_CANCEL, true);
        }
        if let Some(script) = &self.script {
            match script.react(&sig) {
                Ok(Some(intents)) => {
                    if intents.is_on(SPLASH_EXIT) {
                        return Transition::Fire(SPLASH_EXIT.to_string());
                    }
                    if intents.is_on(SPLASH_NEXT) {
                        return Transition::Fire(SPLASH_NEXT.to_string());
                    }
                    // The script named no intent for this signal — its call (e.g. a
                    // react-less or custom script). A completed timeline with no intent
                    // would strand the player, so that advances loudly.
                    if !done {
                        return Transition::None;
                    }
                    tracing::error!(
                        "loading '{}': timeline complete but react() named no intent — advancing",
                        self.def.id
                    );
                }
                Ok(None) => {} // react-less script: the engine mapping below.
                Err(e) => tracing::error!("loading '{}': react() failed: {e}", self.def.id),
            }
        }
        // No script to decide: Cancel backs out, a completed timeline advances.
        if root.back_out {
            return Transition::Fire(SPLASH_EXIT.to_string());
        }
        Transition::Fire(SPLASH_NEXT.to_string())
    }

    fn render<'f>(&'f mut self, _renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        if let Some(white) = self.white {
            let hud_commands = &self.hud_commands;
            fg.overlay(move |r| render_hud(r, hud_commands, white, &[white]));
        }
    }

    /// The loading screen routes its fired intents (`next` / `exit`) through its
    /// scene file's exits — the kernel calls this after [`Transition::Fire`].
    fn route(&self, result: &str) -> Option<Transition> {
        self.def.exit(result)
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
            view: MenuView::from_tree(
                &theme,
                parse_shared_modal(CONFIRM_SCENE_JSON, "confirm", None),
                shared_modal_script(CONFIRM_SCRIPT, "confirm.lua"),
            ),
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
        format!(
            "{} {secs}s",
            flicker::ui::strings::resolve("$menu_reverting_in")
        )
    }
}

impl Scene for ConfirmDisplayScene {
    fn is_overlay(&self) -> bool {
        true
    }

    /// Menu context: pad/keyboard nav + Confirm for the Keep / Revert buttons (Esc has
    /// no affordance here — Keep / Revert / timeout only, so the tree declares no
    /// `on_cancel`).
    fn input_context(&self) -> Option<InputContext> {
        Some(InputContext::Menu)
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        self.remaining -= dt.as_secs_f32();
        if self.remaining <= 0.0 {
            self.revert(renderer);
            return Transition::Pop;
        }
        // The `confirm` screen's flat overlay keeps the new resolution visible
        // behind the dialog; the live countdown rides the Model (`subtitle` bind).
        let model = ValueMap::new().with("subtitle", self.subtitle());
        let actions = self.view.update(signals, input, renderer, &model);
        if actions.is_on("keep") {
            return Transition::Pop;
        }
        if actions.is_on("revert") {
            self.revert(renderer);
            return Transition::Pop;
        }
        Transition::None
    }

    fn render<'f>(&'f mut self, _renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        let view = &self.view;
        fg.overlay(move |r| view.render(r));
    }
}

/// The unified settings Lua script, embedded so clients inherit it.
const SETTINGS_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/shared/settings.lua");

/// The SHARED pause + display-confirm example pair-scripts (`scripts/shared/pause|confirm.lua`).
/// Each is a thin `arrange()` that lights its modal's optional-item visibility slices — the
/// worked example a human copies to vary the pop-up in their own scene (the defaults the
/// hardened Rust already implements, re-stated as the override surface). Held by [`PauseScene`]
/// / [`ConfirmDisplayScene`] through [`MenuView`]'s pair-script slot and folded each frame; the
/// trees gate their buttons on the slices via `visible_bind`. Embedded like [`SETTINGS_SCRIPT`],
/// so a Lua error fails the build (`script_smoke`).
const PAUSE_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/shared/pause.lua");
const CONFIRM_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/shared/confirm.lua");

/// The main-menu Lua ORCHESTRATION script (`arrange()` only): it latches the
/// realm-button press mirror (`sig_mode_<realm>`) into a persistent page and lights
/// that realm's `shown_realm_<n>` scene slice. The reference is `populous.lua`; this
/// is the menu's PRIMARY input context (67DEE93A / EB527744). Distinct from the
/// RETIRED tree-builder `menu.lua` — this never builds structure, it only reads the
/// selection and returns on/off.
///
/// Stage 2 landed: `MainMenuScene` (the `menu` behaviour) loads it as the held
/// orchestration host — a normal embedded const now.
const MENU_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/Main.lua");

/// The authored Main Menu scene-def (`Main.scene.json`) — a two-column `row` workbench: the
/// nav Menu `popup_panel` + the selector `paged_menu` PTT, all Rust components (201F4F51).
/// `main_menu_tree` parses + expands it and fills the per-realm scene lists; `MainMenuScene`
/// orchestrates it with `menu.lua`. (The manifest also loads + expands it at boot.)
const MAIN_SCENE_JSON: &str = include_str!("../../../../content/sensorium/scenes/Main.scene.json");

/// The SHARED settings scene-def (`scenes/shared/settings.scene.json`): the settings
/// screen's data model + chrome `styles`, authored ONCE here instead of copied into
/// every scene file (Aaron 2026-08-14). Merged onto the shell-furniture carrier by
/// [`main_scene_styles`]. `scenes/shared/` is skipped by the manifest's folder index
/// (it walks top-level scene files only — [`SceneManifest::load_dir`] skips dirs), so
/// this is a direct include, never a roster scene.
const SETTINGS_SCENE_JSON: &str =
    include_str!("../../../../content/sensorium/scenes/shared/settings.scene.json");

/// The SHARED pause + display-confirm modal trees (`scenes/shared/pause|confirm.scene.json`):
/// the pop-up chrome authored ONCE as data (Aaron 2026-08-14 ruling 2A3592D0 — pop-up modals
/// are shared modal trees), replacing the retired Rust `menu_tree` builder. Loaded by
/// [`PauseScene`] / [`ConfirmDisplayScene`] via `parse_ui_json`; their `screens.*` / `modal`
/// chrome styles ride Main's carrier through [`main_scene_styles`] (never inlined here). Like
/// settings, `scenes/shared/` is skipped by the manifest folder index, so these are direct
/// includes, never roster scenes.
const PAUSE_SCENE_JSON: &str =
    include_str!("../../../../content/sensorium/scenes/shared/pause.scene.json");
const CONFIRM_SCENE_JSON: &str =
    include_str!("../../../../content/sensorium/scenes/shared/confirm.scene.json");

/// The embedded THEME — `ui_theme.json`, the one palette (`theme.tokens`) and
/// nothing else (five-line architecture, 491BD9BB). Component looks resolve their
/// dotted `style` paths against it; the shell-furniture blocks (`modal`/`screens`/…)
/// ride the scene files now (Main's `styles` via [`main_scene_styles`], settings via
/// [`SETTINGS_SCENE_JSON`]), not this file.
const SHELL_UI_JSON: &str = include_str!("../../../../content/sensorium/resources/ui_theme.json");
/// The embedded STYLE satellite — `ui_style.json`, for truly-global weight/effect
/// defaults, merged beside the theme at load. Currently an empty placeholder: per
/// rule 491BD9BB no per-component block lives here (component looks are Rust drawing
/// code defaults; scene values live in the scene files).
const SHELL_STYLE_JSON: &str =
    include_str!("../../../../content/sensorium/resources/ui_style.json");
/// The UI stringtable (`{ token: { locale: text } }`) — every shell display string is a
/// `$token` into this (text ruling 2026-07-31); tier-2 content, en-us seeded.
const SHELL_STRINGS_JSON: &str = include_str!("../../../../content/data/stringtable.json");

/// The MAIN scene's `styles` — the shell-furniture carrier. Pause / confirm are now
/// shared modal TREES (`scenes/shared/pause|confirm.scene.json`, Aaron 2026-08-14),
/// but their CHROME blocks (`modal`, `screens.pause`, `screens.confirm`) still ride
/// `Main.scene.json`: a shared modal references the carrier's styles, resolved here at
/// runtime (and merged host⊕own by the style-path gate) — the same split settings uses.
/// The SETTINGS block no longer lives inline in Main — it was moved to its own shared
/// scene file (`scenes/shared/settings.scene.json`) and is merged back onto the carrier
/// here. `None` before the manifest loads (tests build their own).
fn main_scene_styles() -> Option<serde_json::Value> {
    let mut styles = manifest().get("Main").and_then(|d| d.styles.clone())?;
    if let Some(obj) = styles.as_object_mut() {
        match serde_json::from_str::<serde_json::Value>(SETTINGS_SCENE_JSON) {
            Ok(sv) => match sv.get("styles").and_then(|s| s.get("settings")).cloned() {
                Some(block) => {
                    obj.insert("settings".to_string(), block);
                }
                None => tracing::error!("settings.scene.json carries no styles.settings block"),
            },
            Err(e) => tracing::error!("settings.scene.json did not parse: {e}"),
        }
    }
    Some(styles)
}

// Every shell screen is walker-driven (or, for the logo splash, plain
// immediate Lua) — none loads the legacy `Widgets` toolkit (S10). Each
// Lua-driven shell screen builds its own `ScriptHost` inline (see `LogoScene` /
// `MenuView` / `UnifiedSettingsScene`), registering textures + the `UI` global
// the same way.

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

/// Parse a SHARED modal scene-def (`scenes/shared/pause|confirm.scene.json`) into its
/// walker tree. The pop-up chrome is authored DATA now (Aaron 2026-08-14 ruling 2A3592D0 —
/// pop-up modals are shared modal trees), so this only PARSES; the old Rust `menu_tree`
/// builder is retired (its `.tree.json` predecessors died with 1F151933, but a `.scene.json`
/// shared tree is the ratified form, not that one). Best-effort like [`settings_tree`]: on a
/// parse failure it returns a bare `screen` that still declares the modal's `on_cancel` (so
/// Esc → close never depends on layout — the screen IS the declaration), and logs loud.
fn parse_shared_modal(json: &str, id: &str, on_cancel: Option<&str>) -> UiNode {
    let fallback = || {
        let mut n = UiNode {
            component: "surface".to_string(),
            id: id.to_string(),
            ..Default::default()
        };
        if let Some(oc) = on_cancel {
            n.props
                .insert("on_cancel".to_string(), Value::Text(oc.to_string()));
        }
        n
    };
    let def: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("{id}.scene.json did not parse: {e}");
            return fallback();
        }
    };
    match parse_ui_json(&def["tree"]) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("{id}.scene.json tree failed to parse: {e}");
            fallback()
        }
    }
}

/// Load a shared modal's example pair-script into a held host, or `None` (logged) on a Lua
/// error. `None` means the modal runs its static tree with the gated (optional) items hidden
/// — survivable by design: pause keeps Resume + `on_cancel`, confirm keeps its revert timeout.
/// The embedded scripts can only fail here on a syntax error, which `script_smoke` turns into
/// a build failure, so at runtime this is the human-authored-copy fallback, not a shell one.
fn shared_modal_script(src: &str, name: &str) -> Option<ScriptHost> {
    match ScriptHost::new(src, name) {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::error!("{name} failed to load — modal runs its static default: {e}");
            None
        }
    }
}

/// Find the first descendant (or self) whose `id` matches, mutably — the seam the
/// `menu` behaviour fills its authored containers through (button box, scene list,
/// muse slot).
fn find_by_id_mut<'a>(node: &'a mut UiNode, id: &str) -> Option<&'a mut UiNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter_mut().find_map(|c| find_by_id_mut(c, id))
}

/// One launcher scene row per registered scene — a `row` of primitive kinds:
/// [bronze-framed preview] · [mode / name / desc / meta column] · [LOAD]. The LOAD
/// button fires the scene id and joins the cross-panel "scenes" focus group; the
/// `region · meta` line is pre-joined so no string is composed at draw.
fn scene_row_nodes(scenes: &[SceneRow]) -> Vec<UiNode> {
    scenes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let meta = if s.region.is_empty() {
                s.meta.clone()
            } else {
                format!("{}  ·  {}", s.region, s.meta)
            };
            let mut preview_inner = kind("cell");
            preview_inner.grow = Some(1.0);
            preview_inner.props.insert(
                "style".to_string(),
                Value::Text("menu.preview_inner".to_string()),
            );
            let mut preview = kind("cell");
            preview.size = Some(96.0);
            preview.pad = 3.0;
            preview.props.insert(
                "style".to_string(),
                Value::Text("menu.preview_frame".to_string()),
            );
            preview.children = vec![preview_inner];

            let mut details = kind("cell");
            details.grow = Some(1.0);
            details.gap = 3.0;
            details.children = vec![
                row_text(&s.mode, 14.0, 10.0, "menu.mode", "label"),
                row_text(&s.name, 32.0, 28.0, "menu.name", "display"),
                row_text(&s.desc, 34.0, 14.0, "menu.row_desc", "body"),
                row_text(&meta, 14.0, 9.0, "menu.meta", "label"),
            ];

            let mut spacer = kind("stack");
            spacer.grow = Some(1.0);
            let mut load_btn = UiNode {
                component: "button".to_string(),
                id: s.id.clone(),
                action: Some(s.id.clone()),
                size: Some(42.0),
                tab_group: "scenes".to_string(),
                nav_ordinal: i as u32,
                ..Default::default()
            };
            load_btn
                .props
                .insert("label".to_string(), Value::Text("$menu_load".to_string()));
            load_btn
                .props
                .insert("label_size".to_string(), Value::Number(12.0));
            load_btn
                .props
                .insert("variant".to_string(), Value::Text("primary".to_string()));
            let mut load = kind("cell");
            load.size = Some(150.0);
            load.children = vec![spacer, load_btn];

            let mut row = kind("row");
            row.size = Some(126.0);
            row.pad = 15.0;
            row.gap = 20.0;
            row.props
                .insert("style".to_string(), Value::Text("menu.row".to_string()));
            row.children = vec![preview, details, load];
            row
        })
        .collect()
}

/// The launcher backdrop's Muse — an aspect-locked square pinned to the right edge,
/// drawn faint under the popup / panel (her baked left-edge dissolve fades her into
/// the menu). `tex` is her registered texture index.
fn muse_sprite(tex_id: usize) -> UiNode {
    let mut n = UiNode {
        component: "sprite".to_string(),
        anchor: UiAnchor::from_name("right"),
        ..Default::default()
    };
    n.props.insert("width_frac".to_string(), Value::Number(1.0));
    n.props.insert("aspect".to_string(), Value::Number(1.0));
    n.props
        .insert("tex".to_string(), Value::Number(tex_id as f64));
    n.props.insert("alpha".to_string(), Value::Number(0.34));
    n.props.insert("layer".to_string(), Value::Number(0.0));
    n
}

/// A bare component node of `component` kind.
fn kind(component: &str) -> UiNode {
    UiNode {
        component: component.to_string(),
        ..Default::default()
    }
}

/// A scene-row detail text line: a display string at `size` / `text_size` in a named
/// ink and font.
fn row_text(text: &str, size: f32, text_size: f32, color: &str, font: &str) -> UiNode {
    let mut n = kind("text");
    n.size = Some(size);
    n.props
        .insert("text".to_string(), Value::Text(text.to_string()));
    n.props
        .insert("text_size".to_string(), Value::Number(text_size as f64));
    n.props
        .insert("color".to_string(), Value::Text(color.to_string()));
    n.props
        .insert("font".to_string(), Value::Text(font.to_string()));
    n
}

#[cfg(test)]
mod menu_tree_tests {
    use super::*;

    /// Parse a shared modal scene-def const into its tree GPU-free (no Theme / textures) —
    /// the exact `parse_shared_modal` path `PauseScene` / `ConfirmDisplayScene` load.
    fn shared_tree(json: &str) -> UiNode {
        let def: serde_json::Value = serde_json::from_str(json).expect("shared modal parses");
        parse_ui_json(&def["tree"]).expect("shared modal tree parses")
    }

    /// Pause + display-confirm are SHARED modal trees now (`scenes/shared/*.scene.json`,
    /// Aaron 2026-08-14) — the `menu_tree` Rust builder is retired. Asserts each file parses
    /// into a full-bleed `screen` filled with real content, and that pause declares its
    /// `on_cancel = resume` back-out intent (S9).
    #[test]
    fn pause_and_confirm_build_from_scene_json() {
        let pause = shared_tree(PAUSE_SCENE_JSON);
        assert_eq!(pause.component, "surface", "pause → full-bleed popup page");
        assert!(
            !pause.children.is_empty(),
            "pause popup filled with real content"
        );
        assert_eq!(
            UiIntents::of(&pause).result_for(ActionSignal::Cancel),
            Some("resume"),
            "Esc / pad-B backs the pause overlay out to the game (on_cancel = resume)"
        );

        let confirm = shared_tree(CONFIRM_SCENE_JSON);
        assert_eq!(
            confirm.component, "surface",
            "confirm → full-bleed popup page"
        );
        assert!(
            !confirm.children.is_empty(),
            "confirm popup filled with real content"
        );
    }

    /// LIVE-PAIR GATE — the shared modals are example pairs now (Aaron 2026-08-17): each
    /// tree gates its optional buttons on `visible_bind` slices its pair script (`pause.lua`
    /// / `confirm.lua`) `arrange()` lights. A gated button the default script leaves dark
    /// would VANISH, so this pins every `visible_bind` in the shipped tree to a lit slice —
    /// the two halves cannot drift, the way settings' derived gates pin to its tree. Derived
    /// from the tree side (no hardcoded key list), so a new gated button must be lit too.
    #[test]
    fn pause_and_confirm_example_scripts_light_every_gated_button() {
        fn visible_binds(node: &UiNode, out: &mut Vec<String>) {
            if let Some(b) = &node.visible_bind {
                out.push(b.clone());
            }
            for c in &node.children {
                visible_binds(c, out);
            }
        }
        for (scene, script, name) in [
            (PAUSE_SCENE_JSON, PAUSE_SCRIPT, "pause.lua"),
            (CONFIRM_SCENE_JSON, CONFIRM_SCRIPT, "confirm.lua"),
        ] {
            let tree = shared_tree(scene);
            let mut binds = Vec::new();
            visible_binds(&tree, &mut binds);
            assert!(
                !binds.is_empty(),
                "{name}'s modal gates at least one optional item"
            );
            let host = ScriptHost::new(script, name).expect("example pair-script loads");
            let model = host
                .arrange()
                .expect("arrange runs")
                .expect("arrange present")
                .to_model();
            for b in &binds {
                assert!(
                    model.is_on(b),
                    "{name} arrange() must light `{b}` — a gated button the script leaves dark vanishes"
                );
            }
        }
    }

    /// RETIREMENT GATE (2026-08-14): the shared `modal.buttons.variants.*` blocks are
    /// gone — a button's look is the compiled `variant` prop ([`BtnVariant`]) now, and
    /// the settings page-rail wears `tab_active_variant`/`tab_idle_variant`. Assert the
    /// shipped Main carrier still holds the rest of its modal chrome (panel/title for
    /// pause + confirm) but NO `buttons` block, so the retired path can't silently
    /// regrow — a scene wearing it would draw magenta (fail-loud), and this makes the
    /// absence a build failure instead.
    #[test]
    fn modal_buttons_variants_are_retired_from_the_carrier() {
        let def: serde_json::Value =
            serde_json::from_str(MAIN_SCENE_JSON).expect("Main.scene.json parses");
        assert!(
            def["styles"]["modal"].is_object(),
            "Main still carries the modal chrome (panel/title/… for pause + confirm)"
        );
        assert!(
            def["styles"]["modal"].get("buttons").is_none(),
            "modal.buttons is retired — button looks are the Rust `variant` prop, not a \
             resolved style block"
        );
    }

    /// CATALOG COVERAGE GATE (S4a): every display token the signal/input catalog can emit
    /// resolves in the shipped stringtable. A `token()` with no seed renders as its raw
    /// `$stem` in-window (fail-loud, MCP `4BB12A75`); this turns a missing seed into a
    /// build failure, and covers the channel the derived page depends on (`8634C200`).
    #[test]
    fn every_catalog_token_resolves_in_the_shipped_table() {
        use flicker_input_core::{GamepadAxis, GamepadButton, MouseAxis, MouseButton};
        let table = flicker::ui::strings::flatten(SHELL_STRINGS_JSON, "en-us")
            .expect("shell stringtable flattens");
        let mut tokens: Vec<String> = Vec::new();
        tokens.extend(ActionSignal::ALL.iter().map(|s| s.token()));
        tokens.extend(SignalGroup::ALL.iter().map(|g| g.token().to_string()));
        tokens.extend(Key::ALL.iter().map(|k| k.token()));
        tokens.extend(MouseButton::ALL.iter().map(|mb| mb.token()));
        tokens.extend(GamepadButton::ALL.iter().map(|b| b.token()));
        tokens.extend(GamepadAxis::ALL.iter().map(|a| a.token()));
        tokens.push(MouseAxis::X.token().to_string());
        tokens.push(MouseAxis::Y.token().to_string());
        tokens.push("$bind_hold".to_string());
        let missing: Vec<&String> = tokens
            .iter()
            .filter(|t| !table.contains_key(t.strip_prefix('$').unwrap_or(t)))
            .collect();
        assert!(
            missing.is_empty(),
            "catalog tokens with no stringtable seed: {missing:?}"
        );
    }

    /// DERIVED-PAGE GATE (S4a): the keyboard page's keycaps are EXACTLY the Player-scope
    /// signals — one `kc_<name>` per [`ActionSignal::rebindable`], none beyond. This is the
    /// gate the old hand-authored list ↔ `KEYBOARD_ACTIONS` pairing never had: a mismatch
    /// used to ship a blank, dead keycap. Derivation + this gate make drift impossible.
    #[test]
    fn derived_keyboard_page_matches_the_rebindable_set() {
        fn collect(node: &UiNode, out: &mut Vec<String>) {
            if let Some(kc) = node.id.strip_prefix("kc_") {
                out.push(kc.to_string());
            }
            for c in &node.children {
                collect(c, out);
            }
        }
        let tree = settings_tree(&display::RESOLUTIONS);
        let mut caps = Vec::new();
        collect(&tree, &mut caps);
        caps.sort();
        let mut want: Vec<String> = ActionSignal::rebindable()
            .map(|s| s.name().to_string())
            .collect();
        want.sort();
        assert_eq!(
            caps, want,
            "keyboard keycaps must be exactly the rebindable signals"
        );
        assert_eq!(
            caps.len(),
            29,
            "the ruled Player set (19 base + souls tier incl. Grapple)"
        );
    }

    /// RETIREMENT GATE (S4a): the hand-authored keyboard schema (`styles.settings.input.
    /// keyboard` + the dead `.tabs`) and its 22 `$set_*` tokens are gone — the page derives
    /// from the catalog now. Guards against a reintroduction that forks the vocabulary again.
    #[test]
    fn the_retired_keyboard_schema_is_gone() {
        let def: serde_json::Value =
            serde_json::from_str(SETTINGS_SCENE_JSON).expect("settings.scene.json parses");
        let input = &def["styles"]["settings"]["input"];
        assert!(
            input.get("keyboard").is_none(),
            "styles.settings.input.keyboard is retired"
        );
        assert!(
            input.get("tabs").is_none(),
            "styles.settings.input.tabs is retired"
        );
        let table = flicker::ui::strings::flatten(SHELL_STRINGS_JSON, "en-us").expect("flatten");
        for stem in [
            "set_movement",
            "set_interface",
            "set_actions",
            "set_move_forward",
            "set_quit",
        ] {
            assert!(!table.contains_key(stem), "retired token `{stem}` lingers");
        }
    }

    /// STAGE-1 GATE (settings → scene pair, content-layer item #8): the settings screen's
    /// LAYOUT is now a STATIC tree in settings.scene.json — the untrusted Lua no longer
    /// builds structure (the client is in the enemy's hands; a Lua tree-builder is an
    /// exploit surface). Assert the static tree parses and carries the empty per-section
    /// fill containers the hardened Rust behaviour populates from the row schema.
    #[test]
    fn settings_scene_tree_parses_with_its_fill_containers() {
        let def: serde_json::Value =
            serde_json::from_str(SETTINGS_SCENE_JSON).expect("settings.scene.json parses");
        let mut tree = parse_ui_json(&def["tree"]).expect("settings static tree parses");
        assert_eq!(tree.id, "settings", "root is the settings screen");
        for id in [
            "video_rows",
            "audio_rows",
            "kb_rows",
            "mouse_rows",
            "controller_rows",
        ] {
            assert!(
                find_by_id_mut(&mut tree, id).is_some(),
                "fill container `{id}` is authored for the hardened Rust row-filler"
            );
        }
    }

    /// VOCABULARY GATE for the screens every client ships — including the launcher's
    /// mode tiers (root + the three tier-2 pages). A component kind the engine does
    /// not know draws NOTHING — the walker anchor-overlays its children and the draw
    /// arm falls through — so a typo or a name left behind by a rename is invisible
    /// until someone opens the window. This turns that into a build failure.
    #[test]
    fn the_shipped_screens_name_only_kinds_the_engine_knows() {
        // Pause + display-confirm are the shipped popup screens — SHARED modal trees now
        // (`scenes/shared/*.scene.json`, Aaron 2026-08-14). The live main menu's vocabulary
        // is gated by `the_main_menu_composes_from_the_rust_components`.
        for (screen, json) in [("pause", PAUSE_SCENE_JSON), ("confirm", CONFIRM_SCENE_JSON)] {
            let tree = shared_tree(json);
            assert!(
                flicker::ui::unknown_kinds(&tree).is_empty(),
                "shared modal '{screen}' names unknown kinds: {:?}",
                flicker::ui::unknown_kinds(&tree)
            );
            // The strings gate (S10): every display literal is a `$token`.
            assert!(
                flicker::ui::raw_display_literals(&tree).is_empty(),
                "shared modal '{screen}' ships raw display literals: {:?}",
                flicker::ui::raw_display_literals(&tree)
            );
        }

        // The settings screen is the STATIC scene now (`settings.scene.json`), filled with
        // hardened Rust rows ([`settings_tree`]) — the untrusted Lua composes NO structure.
        // Gate the PRODUCTION tree the scene walks: it must name only native component kinds
        // (`paged_menu` / `popup_panel` / `tabs` / …, no unknown kind → a kind draws nothing)
        // and ship no raw display literal (every label a `$token`).
        let tree = settings_tree(&display::RESOLUTIONS);
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "settings screen names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "settings screen ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );

        // The MODEL-CHANNEL strings gate (S10's blind side): display copy published
        // from Rust into the Model bypasses the tree gates above, so the crate
        // self-gates its OWN source — every `.set`/`.with` value must be a resolved
        // `$token`, a data shape, or carry an explicit `strings-gate-exempt` reason.
        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("shell.rs"));
        assert!(
            flags.is_empty(),
            "raw display copy published into the Model: {flags:?}"
        );
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
    // ── Directional-nav (spec §8) — keyboard + gamepad focus traversal of the menu
    //    buttons. The central PUMP resolves this frame's Menu-context edges (arrows /
    //    d-pad → `Nav*`, bumpers → `Tab*`, A/Enter → `Confirm`, B/Esc → `Cancel`) for the
    //    OWNER scene's declared `input_context()`; the walker consumes them and writes the
    //    shared focus id. The view owns no resolver/bindings (input-P3, 0569DA9B). ──
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
    /// The launcher tree's `visible_bind` / `text_bind` values (`has_scenes` /
    /// `panel_head` / `menu_footer`), computed once from the page + scenes and folded
    /// into every frame's model so the static tree reveals the right pieces.
    render_model: ValueMap,
    /// The Lua ORCHESTRATION host (`menu.lua`) for the MAIN MENU — held for the scene's
    /// life so `arrange()` runs each frame the page may have changed, latching the page
    /// from the `sig_mode_<realm>` mirror and returning the `shown_realm_<n>` slice
    /// visibility. `None` for the pause / confirm popups (static, no page).
    script: Option<ScriptHost>,
}

impl MenuView {
    /// A walker view over an already-parsed scene tree, optionally Lua-orchestrated.
    /// The MAIN MENU (`Main.scene.json`) passes its `menu.lua` host: `arrange()` runs each
    /// frame and folds the `shown_realm_<n>` page-slice visibility into the model. The
    /// pause / confirm popups (`scenes/shared/pause|confirm.scene.json`) pass `None` — static
    /// chrome, no page, empty `render_model`. Registers the theme textures, resolves styles
    /// against the shell-furniture carrier ([`main_scene_styles`]), and collects the tree's
    /// declarative `on_<signal>` intents (S9) once. Replaces the retired `menu_tree` Rust
    /// builder the old `new` expanded; a caller hands a fallback tree if its file failed to
    /// parse (best-effort).
    fn from_tree(theme: &Theme, tree: UiNode, script: Option<ScriptHost>) -> Self {
        let entries = theme.lua_textures();
        let textures: Vec<TextureHandle> = entries.iter().map(|(_, h)| *h).collect();
        let styles = flicker::ui::load_styles_strs_for(
            &[SHELL_UI_JSON, SHELL_STYLE_JSON],
            main_scene_styles().as_ref(),
        );
        let intents = UiIntents::of(&tree);
        Self {
            textures,
            tree: Some(tree),
            styles,
            ui_state: UiState::new(),
            commands: Vec::new(),
            intents,
            fired_sigs: Vec::new(),
            nav_initialized: false,
            render_model: ValueMap::new(),
            script,
        }
    }

    /// Walk the cached tree for one frame. `signals` carries the PUMP's resolved
    /// Menu-context events (the owner scene declares `input_context() = Menu`); `model`
    /// carries any per-frame binds (the confirm countdown's `subtitle`). Stashes the draw
    /// commands and returns the fired actions (`is_on("start")` / `is_on("main_menu")` …).
    fn update(
        &mut self,
        signals: &mut SceneInput,
        input: &InputState,
        renderer: &Renderer,
        model: &ValueMap,
    ) -> ValueMap {
        let Some(tree) = self.tree.as_ref() else {
            return ValueMap::new();
        };

        // The EFFECTIVE model the WHOLE frame agrees on: the screen's static render
        // values (has_scenes / panel_head / menu_footer) UNDER the caller's per-frame
        // model (e.g. the confirm countdown), then the one-frame `sig_<name>` mirror
        // (S9: names fired last frame ride ONE Model publish, then drop). `run_ui` draws
        // it, the nav walker flattens it, AND the default-focus seed reads it — ONE
        // model so a node gated visible by `has_scenes` is navigable exactly when it is
        // drawn. (Feeding the walker the bare `model` instead pruned the whole
        // `has_scenes` scene panel from nav/Confirm while `run_ui` still drew + clicked
        // it — a pad Confirm could never reach a button the mouse could; `focusables_of`
        // documents that nav and draw MUST share one model.)
        let mut eff = self.render_model.clone();
        eff.extend(model.clone());
        if !self.fired_sigs.is_empty() {
            UiIntents::mirror_into(&mut eff, &self.fired_sigs);
            self.fired_sigs.clear();
        }
        // The Lua ORCHESTRATION (main menu): arrange() reads the model — INCLUDING this
        // frame's `sig_<name>` mirror — to latch the page, and returns the page-slice
        // visibility (`shown_realm_<n>`) folded into the effective model the walker draws
        // and flattens for nav. (No-op for the static pause / confirm popups.)
        if let Some(script) = self.script.as_ref() {
            if let Err(e) = script.set_model(&eff) {
                tracing::error!("menu.lua: publishing the model failed: {e}");
            }
            match script.arrange() {
                Ok(Some(a)) => eff.extend(a.to_model()),
                Ok(None) => {}
                Err(e) => tracing::error!("menu.lua: arrange() failed: {e}"),
            }
        }

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
            if let Some(first) = focusables_of(tree, &eff).into_iter().next() {
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
            right_down: input.mouse_right,
            screen: size,
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(tree, &eff, &self.styles, &snap, &mut self.ui_state);
        self.commands = frame.commands;
        let mut results = frame.results;
        let hud_hit = results.is_on("hud_hit");

        // ── Directional nav (spec §8): the PUMP resolved this frame's menu edges (arrows
        //    / d-pad → Nav*, bumpers → Tab*, Enter/A → Confirm, Esc/B → Cancel) for the
        //    owner scene's declared `input_context()` (Menu) — the view owns no resolver.
        //    Dispatch `signals.events` through the walker layer, which writes the ONE
        //    shared focus id and turns a declared `on_cancel` (the pause overlay's
        //    `resume`) into a fired result name. `menu.lua` authors `tab_group` /
        //    `nav_ordinal` for EVERY popup (menu / pause / confirm), so all three are
        //    pad-navigable via this path. ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, hud_hit)
            .with_nav(tree, &eff)
            .with_intents(&self.intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        // ONE drain: a declared intent that fired AND a pad Confirm on a focused
        // button arrive as the same thing — a result name folded in exactly like a
        // click (`results.set(name, true)`) — and both queue for the one-frame
        // `sig_<name>` Model mirror above.
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
            self.fired_sigs.push(name);
        }

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

/// The settings overlay — a compliant SCENE PAIR: static layout in
/// `scenes/shared/settings.scene.json`, hardened Rust fills the rows ([`settings_tree`]),
/// and a thin `scripts/shared/settings.lua` `derive()` owns only section/sub-tab
/// visibility. The Input · Keyboard page DERIVES its rows from the signal catalog
/// ([`ActionSignal::rebindable`]); the Controller tab selects controller configs
/// ([`InputProfile::PRESET_NAMES`]). Commits buffered changes to the live scene on pop.
struct UnifiedSettingsScene {
    theme: Theme,
    /// The 1×1 white + theme textures for `render_hud` (`textures[0]` = white).
    textures: Vec<TextureHandle>,
    /// The STATIC settings tree ([`settings_tree`]) — parsed from
    /// `settings.scene.json` and filled with hardened Rust rows ONCE (walker-driven).
    tree: Option<UiNode>,
    /// The `settings.lua` pair-script host, held for its per-frame `derive()` — the
    /// scene's ONLY untrusted runtime behavior: it turns the published `settings_page`
    /// / `input_subtab` indices into the `sec_*` / `sub_*` visibility gates. It composes
    /// NO structure (the tree is the static scene) and touches NO hardened state.
    script: ScriptHost,
    /// Resolved `ui_theme.json` styles (dotted `style` paths resolve against it).
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
    /// Device-enumerated resolution rungs, snapshotted at construction (the monitor's video
    /// modes, or the static fallback when headless). The resolution select's options and the
    /// video_resolution index↔size mapping both read this — one per scene, never re-queried
    /// per frame (so the list can't shift mid-scene, e.g. a monitor unplug).
    resolutions: Vec<display::Resolution>,
    /// The screen's declared surface set (S8): the section rail + input sub-tab
    /// radio groups, the rebind banner + applied flash, and the two overlay
    /// dialogs. Owns every `visible_bind` gate `settings.lua` reads; published
    /// into the Model once per frame ([`Sections::publish`] in `model`).
    surfaces: Sections,
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
    /// signal bus); every OTHER Esc path rides the pump's Menu-context `Cancel`,
    /// which fires the screen's declared `settings_close` intent (S9).
    rebind_esc_prev: bool,
    /// Previous-frame Backspace state — **rebind-unbind only** (same raw-poll rationale as
    /// `rebind_esc_prev`): while capturing, a Backspace EDGE drops the current action's
    /// binding (the banner's advertised "Backspace to unbind"), caught before `poll` would
    /// capture Backspace as a new key.
    rebind_bs_prev: bool,
    /// True once any buffered setting or keybind differs from what was last persisted —
    /// gates the unsaved-changes confirm on close. Set on a real edit, cleared on commit.
    dirty: bool,
    // ── The input seam (input-P3, 0569DA9B): the scene owns NO resolver/bindings.
    //    The central PUMP resolves this frame's Menu-context edges (Esc/B → `Cancel`,
    //    arrows/d-pad → `Nav*`, …) for the scene's declared `input_context()`; the walker
    //    layer turns the screen's DECLARED `on_cancel = "settings_close"` into a fired
    //    result name. ──
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
fn settings_sections() -> Sections {
    let mut decls = vec![
        Section::new("sec_video").group("section").on(),
        Section::new("sec_audio").group("section"),
        Section::new("sec_input").group("section"),
    ];
    for (i, name) in INPUT_SUBTABS.iter().enumerate() {
        let s = Section::new(format!("sub_{name}")).group("subtab");
        decls.push(if i == 0 { s.on() } else { s });
    }
    decls.extend([
        Section::new("rebinding"),
        Section::new("applied"),
        Section::new("confirm_close").context("Menu"),
        Section::new("restore_note").context("Menu"),
    ]);
    Sections::new(decls)
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

// ─────────────────────────────────────────────────────────────────────────────
// SETTINGS ROW BUILDER — the HARDENED port of `settings.lua`'s row helpers
// (`control_node` / `ctrl_row` / `add_groups` / `keyboard_tab` / `controller_tab`).
// The untrusted, end-user-editable Lua must NOT compose structure — the client is in
// the enemy's hands, so a Lua tree-builder is an exploit surface (plan, spec 201F4F51).
// The ~39 data-driven rows are therefore built HERE, in compiled Rust, and filled into
// the static scene's empty per-section containers (`video_rows`/…) — the menu's ratified
// fill precedent (`find_by_id_mut` + a `*_nodes` builder, decision A4924753). Node shapes
// reproduce the Lua EXACTLY; the two-way bind wires are load-bearing (the `update` ladder
// reads them back every frame). The row SCHEMA is data (read from the scene); the layout
// numbers below are drawing code (five-line rule 491BD9BB), mirroring the Lua `CTRL_W`
// local + the `styles.settings.row` / `.controls.keycap` reads the retiring Lua made.
// ─────────────────────────────────────────────────────────────────────────────

/// The control column width (Lua `CTRL_W`); a row's control gutter is `CTRL_W + 88`.
const CTRL_W: f32 = 210.0;
/// Row height + name / desc / group glyph sizes (Lua `S.row.{h,name_size,desc_size,group_size}`).
const ROW_H: f32 = 50.0;
const ROW_NAME_SIZE: f32 = 17.0;
const ROW_DESC_SIZE: f32 = 13.0;
const ROW_GROUP_SIZE: f32 = 10.0;
/// Keycap button width + label size (Lua `S.controls.keycap.{w,label_size}`).
const KEYCAP_W: f32 = 156.0;
const KEYCAP_LABEL_SIZE: f64 = 12.0;

/// The video / audio / input ROW SCHEMA, read in place from `styles.settings.{video,
/// audio,input}` of the parsed scene (Stage 2 keeps it there; a later stage relocates
/// it). Serde ignores the sibling skin / `default` / backend fields — only the row DATA
/// is read; a missing optional key deserializes to `None`.
#[derive(serde::Deserialize)]
struct SettingsSchema {
    video: RowSection,
    audio: AudioSection,
    input: InputSection,
}

/// A section that is just a list of control-row groups (video / mouse).
#[derive(serde::Deserialize)]
struct RowSection {
    groups: Vec<RowGroup>,
}

/// One titled group of control rows.
#[derive(serde::Deserialize)]
struct RowGroup {
    name: String,
    rows: Vec<SettingRow>,
}

/// One data-driven control row. `wired` = bound to a real backend; an unwired row is an
/// inert PREVIEW (a bronze badge + `enabled_bind = "off"`). Missing optional keys are
/// `None`, so a row need only carry the fields its `kind` uses.
#[derive(serde::Deserialize)]
struct SettingRow {
    id: String,
    kind: String,
    name: String,
    desc: Option<String>,
    options: Option<Vec<String>>,
    min: Option<f64>,
    max: Option<f64>,
    suffix: Option<String>,
    value: Option<String>,
    wired: bool,
}

/// Audio: the "not yet implemented" stub notice + the preview groups.
#[derive(serde::Deserialize)]
struct AudioSection {
    stub: AudioStub,
    groups: Vec<RowGroup>,
}

#[derive(serde::Deserialize)]
struct AudioStub {
    title: String,
    body: String,
}

/// Input: the mouse groups and the controller notes. The KEYBOARD rebind list is no
/// longer schema data — it derives from the signal catalog ([`kb_rows_nodes`] iterates
/// [`ActionSignal::rebindable`]), so a signal added to the enum shows up on the page with
/// no hand-authored row (the ratified promise, MCP `C60AE43C §2`).
#[derive(serde::Deserialize)]
struct InputSection {
    mouse: RowSection,
    controller: ControllerSection,
}

#[derive(serde::Deserialize)]
struct ControllerSection {
    title: String,
    body: String,
    title_size: f64,
}

/// The Model key a row's control binds to — the canonical wired keys (the Lua `BINDS`
/// table), else a read-only `pv_<id>` preview key the scene publishes with a fixed
/// default. LOAD-BEARING: the `update` results ladder reads these exact names back every
/// frame, so the map must reproduce `settings.lua` byte-for-byte.
fn bind_key(id: &str) -> String {
    match id {
        "display_mode" => "video_display_mode".to_string(),
        "resolution" => "video_resolution".to_string(),
        "quality" => "video_quality".to_string(),
        "vsync" => "video_vsync".to_string(),
        "fps_limit" => "video_fps_limit".to_string(),
        "m_look" => "look_sens_pct".to_string(),
        "m_invert" => "input_mouse_invert_pitch".to_string(),
        other => format!("pv_{other}"),
    }
}

/// The LOCALIZED display for one bound control — every part rides the stringtable via the
/// catalog `token()` (S3), composed in Rust because a compound binding (an axis half, a
/// gated mouse-motion) has no single token. Published into the Model as a keycap's
/// `bind_<signal>` value; `node_text` passes it through untouched (already resolved, no `$`
/// sigil). Composition separators are non-alphabetic, so the tree strings-gate stays clean.
fn binding_label(b: &InputBinding) -> String {
    use flicker::ui::strings::resolve;
    match b {
        InputBinding::Key(k) => resolve(&k.token()).into_owned(),
        InputBinding::MouseButton(mb) => resolve(&mb.token()).into_owned(),
        InputBinding::GamepadButton(gb) => resolve(&gb.token()).into_owned(),
        InputBinding::GamepadAxis { axis, direction } => {
            format!("{} {direction}", resolve(&axis.token()))
        }
        InputBinding::MouseMotion {
            axis,
            direction,
            gate,
        } => {
            let base = format!("{} {direction}", resolve(axis.token()));
            match gate {
                Some(g) => format!("{base} ({} {})", resolve("$bind_hold"), resolve(&g.token())),
                None => base,
            }
        }
    }
}

/// The POSITIONAL `pad_glyphs` atlas cell name for a bound gamepad control — the
/// vendor-NEUTRAL vocabulary (concept 0F5E0201): the mapping names a POSITION
/// (`face_south`, `bumper_l`, …), and only the atlas ART speaks a vendor's dialect
/// (Xbox Ⓐ vs PS ✕ live at the same `face_south` cell). `None` for a keyboard/mouse
/// binding, or a pad control with no atlas cell (the system/vendor + Switch-style
/// buttons). The names match the `pad_glyphs.cells` map authored in the scene
/// styles; the walker turns a name into a sprite (an unknown/empty name draws
/// nothing — the deliberate fail-quiet the glyph face already has).
fn gamepad_glyph_name(b: &InputBinding) -> Option<&'static str> {
    use flicker_input_core::{GamepadAxis, GamepadButton};
    match b {
        InputBinding::GamepadButton(gb) => Some(match gb {
            GamepadButton::South => "face_south",
            GamepadButton::East => "face_east",
            GamepadButton::West => "face_west",
            GamepadButton::North => "face_north",
            GamepadButton::LeftBumper => "bumper_l",
            GamepadButton::RightBumper => "bumper_r",
            GamepadButton::LeftTrigger => "trigger_l",
            GamepadButton::RightTrigger => "trigger_r",
            GamepadButton::Start => "menu",
            GamepadButton::Select => "view",
            GamepadButton::DPadUp => "dpad_up",
            GamepadButton::DPadDown => "dpad_down",
            GamepadButton::DPadLeft => "dpad_left",
            GamepadButton::DPadRight => "dpad_right",
            GamepadButton::LeftStick => "stick_l",
            GamepadButton::RightStick => "stick_r",
            // No atlas cell: system/vendor buttons and the Switch-style extras.
            GamepadButton::Guide
            | GamepadButton::Mode
            | GamepadButton::Touchpad
            | GamepadButton::C
            | GamepadButton::Z => return None,
        }),
        InputBinding::GamepadAxis { axis, .. } => Some(match axis {
            GamepadAxis::LeftStickX | GamepadAxis::LeftStickY => "stick_l",
            GamepadAxis::RightStickX | GamepadAxis::RightStickY => "stick_r",
            GamepadAxis::LeftTrigger => "trigger_l",
            GamepadAxis::RightTrigger => "trigger_r",
        }),
        // Keyboard / mouse / mouse-motion are not pad glyphs.
        InputBinding::Key(_) | InputBinding::MouseButton(_) | InputBinding::MouseMotion { .. } => {
            None
        }
    }
}

/// Publish the device-adaptive control display for `signals` into `model` — the ONE
/// place a scene HUD turns an authored SIGNAL into the key/glyph the player will
/// press. Per signal it sets:
/// - `bind_<name>` — the localized text of the first keyboard/mouse binding (the
///   keycap face), via [`binding_label`]; empty when the signal has no kbm binding.
/// - `glyph_<name>` — the `pad_glyphs` atlas cell name of the first gamepad binding,
///   via [`gamepad_glyph_name`]; empty when it has none.
///
/// and stamps `input_device` from the live last-used-device monitor, so a
/// device-adaptive hint draws the keycap on kbm and the glyph on a pad. The scene
/// authors the signal on the hint; the walker picks the face by `input_device`.
/// (Settings' key-bindings page keeps its own slot-0 publish — it edits one specific
/// slot, a different need — and shares only [`binding_label`] with this.)
pub fn publish_signal_bindings(
    model: &mut ValueMap,
    map: &InputMap,
    signals: impl IntoIterator<Item = ActionSignal>,
) {
    model.set(
        "input_device",
        flicker::input_device::last_input_context().token(),
    );
    for sig in signals {
        let name = sig.name();
        let binds = map.bindings_for(sig);
        let cap = binds
            .iter()
            .copied()
            .find(|b| matches!(b, InputBinding::Key(_) | InputBinding::MouseButton(_)))
            .map(|b| binding_label(&b))
            .unwrap_or_default();
        model.set(format!("bind_{name}"), cap);
        let glyph = binds
            .iter()
            .find_map(gamepad_glyph_name)
            .unwrap_or_default();
        model.set(format!("glyph_{name}"), glyph);
    }
}

#[cfg(test)]
mod signal_display_tests {
    use super::*;
    use flicker_input_core::{GamepadButton, Key};

    #[test]
    fn glyph_names_are_positional() {
        // Vendor-NEUTRAL positions, not an Xbox dialect (concept 0F5E0201): West is
        // `face_west` (Xbox X / PS Square live at the same cell, different art).
        assert_eq!(
            gamepad_glyph_name(&InputBinding::GamepadButton(GamepadButton::West)),
            Some("face_west")
        );
        assert_eq!(
            gamepad_glyph_name(&InputBinding::GamepadButton(GamepadButton::LeftBumper)),
            Some("bumper_l")
        );
        assert_eq!(
            gamepad_glyph_name(&InputBinding::GamepadButton(GamepadButton::Start)),
            Some("menu")
        );
        // No atlas cell for the Guide button, and never for a keyboard key.
        assert_eq!(
            gamepad_glyph_name(&InputBinding::GamepadButton(GamepadButton::Guide)),
            None
        );
        assert_eq!(gamepad_glyph_name(&InputBinding::Key(Key::E)), None);
    }

    #[test]
    fn keycap_text_rides_the_stringtable_not_display() {
        // The kbm keycap MUST resolve the key NAME through the stringtable (the locale
        // axis, concept 0F5E0201) — never Rust Display/debug formatting. If someone
        // swaps `binding_label`'s Key arm to `format!("{k}")`, it diverges from the
        // token path and this fails. (P4 drift gate.)
        let via_label = binding_label(&InputBinding::Key(Key::E));
        let via_token = flicker::ui::strings::resolve(&Key::E.token()).into_owned();
        assert_eq!(via_label, via_token);
    }

    // These assert the glyph face + device stamp, which are DETERMINISTIC. The
    // keycap TEXT is `binding_label`'s concern — it resolves through the stringtable
    // (absent in a bare unit test) and `ValueMap::text` reports an empty value as
    // `None`, so `.unwrap_or("")` reads a missing/empty face as `""` either way.

    #[test]
    fn publish_sets_the_glyph_face_and_device() {
        // A signal bound to a key (kbm face) AND a pad button (glyph face).
        let mut map = InputMap::empty();
        map.bind(ActionSignal::Interact, InputBinding::Key(Key::E));
        map.bind(
            ActionSignal::Interact,
            InputBinding::GamepadButton(GamepadButton::West),
        );

        let mut m = ValueMap::new();
        publish_signal_bindings(&mut m, &map, [ActionSignal::Interact]);

        // Keys are `glyph_<name>` / `bind_<name>` where name is the stable PascalCase
        // ActionSignal name (e.g. "Interact") — the SAME form scene trees author.
        let gkey = format!("glyph_{}", ActionSignal::Interact.name());
        // The pad face resolves to the West-button POSITIONAL cell …
        assert_eq!(m.text(&gkey).unwrap_or(""), "face_west");
        // … and the device family is always stamped (kbm is the resting default).
        assert_eq!(m.text("input_device"), Some("kbm"));
    }

    #[test]
    fn publish_reflects_which_family_is_bound() {
        let gkey = format!("glyph_{}", ActionSignal::Interact.name());
        let bkey = format!("bind_{}", ActionSignal::Interact.name());

        // Pad only → glyph face set, keycap face empty.
        let mut pad = InputMap::empty();
        pad.bind(
            ActionSignal::Interact,
            InputBinding::GamepadButton(GamepadButton::South),
        );
        let mut m = ValueMap::new();
        publish_signal_bindings(&mut m, &pad, [ActionSignal::Interact]);
        assert_eq!(m.text(&gkey).unwrap_or(""), "face_south");
        assert_eq!(m.text(&bkey).unwrap_or(""), "");

        // Kbm only → glyph face empty (the keycap text is stringtable-resolved).
        let mut kbm = InputMap::empty();
        kbm.bind(ActionSignal::Interact, InputBinding::Key(Key::E));
        let mut m = ValueMap::new();
        publish_signal_bindings(&mut m, &kbm, [ActionSignal::Interact]);
        assert_eq!(m.text(&gkey).unwrap_or(""), "");
    }
}

/// A settings text line — the Lua `line(text, box, glyph, color, font, align)`: a `text`
/// node sized `box_len` on the main axis, glyph `glyph`, in a named ink / font, aligned.
fn settings_line(
    text: &str,
    box_len: f32,
    glyph: f32,
    color: &str,
    font: &str,
    align: &str,
) -> UiNode {
    let mut n = kind("text");
    n.size = Some(box_len);
    n.props
        .insert("text".to_string(), Value::Text(text.to_string()));
    n.props
        .insert("text_size".to_string(), Value::Number(glyph as f64));
    n.props
        .insert("color".to_string(), Value::Text(color.to_string()));
    n.props
        .insert("font".to_string(), Value::Text(font.to_string()));
    n.props
        .insert("align".to_string(), Value::Text(align.to_string()));
    n
}

/// A flex spacer (`Stack { grow }`) — centres the name / desc block in a row.
fn grow_stack(grow: f32) -> UiNode {
    let mut n = kind("stack");
    n.grow = Some(grow);
    n
}

/// A fixed-length spacer (`Stack { size }`) — the gap the Lua drops after each group.
fn fixed_stack(size: f32) -> UiNode {
    let mut n = kind("stack");
    n.size = Some(size);
    n
}

/// The bronze PREVIEW badge chip shown beside an unwired row's inert control.
fn preview_badge() -> UiNode {
    let mut n = kind("badge");
    n.size = Some(72.0);
    n.props
        .insert("tone".to_string(), Value::Text("bronze".to_string()));
    n.props
        .insert("label".to_string(), Value::Text("$set_preview".to_string()));
    n.props
        .insert("style".to_string(), Value::Text("badge".to_string()));
    n
}

/// A select / pill option's children: `value` is its 0-based INDEX (a number read
/// straight back off the bind), `label` its display string.
fn options_of(row: &SettingRow) -> Vec<UiNode> {
    row.options
        .iter()
        .flatten()
        .enumerate()
        .map(|(i, label)| {
            let mut n = kind("option");
            n.props.insert("value".to_string(), Value::Number(i as f64));
            n.props
                .insert("label".to_string(), Value::Text(label.clone()));
            n
        })
        .collect()
}

/// One control widget for a data row (`dropdown|cycler → select`, `segment → pill_toggle`,
/// `toggle → toggle`, `slider → slider`, `static → text`, else `stack`), bound to its
/// Model key. An unwired row points `enabled_bind` at the always-false `off` gate so its
/// control is inert (the Lua `control_node`).
fn control_node(row: &SettingRow) -> UiNode {
    let key = bind_key(&row.id);
    let off = !row.wired;
    let mut n = match row.kind.as_str() {
        "toggle" => {
            let mut n = kind("toggle");
            n.size = Some(56.0);
            n.props.insert(
                "style".to_string(),
                Value::Text("settings.controls.toggle".to_string()),
            );
            n.bind = Some(key);
            n
        }
        "slider" => {
            let mut n = kind("slider");
            n.size = Some(CTRL_W);
            n.props
                .insert("min".to_string(), Value::Number(row.min.unwrap_or(0.0)));
            n.props
                .insert("max".to_string(), Value::Number(row.max.unwrap_or(100.0)));
            n.props.insert("value_w".to_string(), Value::Number(46.0));
            n.props.insert("slider_h".to_string(), Value::Number(8.0));
            n.props.insert("decimals".to_string(), Value::Number(0.0));
            if let Some(suffix) = &row.suffix {
                n.props
                    .insert("suffix".to_string(), Value::Text(suffix.clone()));
            }
            n.props.insert(
                "style".to_string(),
                Value::Text("settings.controls.slider".to_string()),
            );
            n.bind = Some(key);
            n
        }
        "dropdown" | "cycler" => {
            let mut n = kind("select");
            n.size = Some(CTRL_W);
            n.props.insert(
                "style".to_string(),
                Value::Text("settings.controls".to_string()),
            );
            n.children = options_of(row);
            n.bind = Some(key);
            n
        }
        "segment" => {
            let count = row.options.as_ref().map_or(0, Vec::len);
            let mut n = kind("pill_toggle");
            n.size = Some(CTRL_W.max(60.0 * count as f32));
            n.props.insert(
                "style".to_string(),
                Value::Text("settings.controls.pill".to_string()),
            );
            n.children = options_of(row);
            n.bind = Some(key);
            n
        }
        "static" => {
            return settings_line(
                row.value.as_deref().unwrap_or(""),
                CTRL_W,
                15.0,
                "settings.controls.field.label",
                "body",
                "right",
            );
        }
        _ => {
            let mut n = kind("stack");
            n.size = Some(CTRL_W);
            return n;
        }
    };
    n.id = format!("c_{}", row.id);
    if off {
        n.enabled_bind = Some("off".to_string());
    }
    n
}

/// One settings row: name (+ desc) centred on the left, the control (+ a PREVIEW badge
/// for an unwired row) in the right gutter (the Lua `ctrl_row`).
fn ctrl_row(row: &SettingRow) -> UiNode {
    let name_color = if row.wired {
        "settings.row.name_color"
    } else {
        "settings.row.desc_color"
    };

    let mut left = kind("cell");
    left.grow = Some(1.0);
    left.gap = 2.0;
    left.children.push(grow_stack(1.0));
    left.children.push(settings_line(
        &row.name,
        ROW_NAME_SIZE + 3.0,
        ROW_NAME_SIZE,
        name_color,
        "body",
        "left",
    ));
    if let Some(desc) = &row.desc {
        left.children.push(settings_line(
            desc,
            ROW_DESC_SIZE + 3.0,
            ROW_DESC_SIZE,
            "settings.row.desc_color",
            "body",
            "left",
        ));
    }
    left.children.push(grow_stack(1.0));

    let mut right = kind("row");
    right.size = Some(CTRL_W + 88.0);
    right.gap = 8.0;
    right.children.push(grow_stack(1.0));
    if !row.wired {
        right.children.push(preview_badge());
    }
    right.children.push(control_node(row));

    let mut r = kind("row");
    r.size = Some(ROW_H);
    r.children = vec![left, right];
    r
}

/// A group header line (the Lua `group_head`).
fn group_head(name: &str) -> UiNode {
    settings_line(
        name,
        30.0,
        ROW_GROUP_SIZE,
        "settings.row.group_color",
        "label",
        "left",
    )
}

/// Append every group's header + rows, with a 10px gap after each (the Lua `add_groups`).
fn add_groups(out: &mut Vec<UiNode>, groups: &[RowGroup]) {
    for g in groups {
        out.push(group_head(&g.name));
        for row in &g.rows {
            out.push(ctrl_row(row));
        }
        out.push(fixed_stack(10.0));
    }
}

/// VIDEO section rows (fills `video_rows`).
fn video_rows_nodes(schema: &SettingsSchema) -> Vec<UiNode> {
    let mut out = Vec::new();
    add_groups(&mut out, &schema.video.groups);
    out
}

/// AUDIO section rows: the "not yet implemented" notice, then the preview groups
/// (fills `audio_rows`).
fn audio_rows_nodes(schema: &SettingsSchema) -> Vec<UiNode> {
    let stub = &schema.audio.stub;
    let mut out = vec![
        settings_line(
            &stub.title,
            22.0,
            10.0,
            "settings.audio.stub.title_color",
            "label",
            "left",
        ),
        settings_line(
            &stub.body,
            22.0,
            14.0,
            "settings.audio.stub.body_color",
            "body",
            "left",
        ),
        fixed_stack(12.0),
    ];
    add_groups(&mut out, &schema.audio.groups);
    out
}

/// INPUT · KEYBOARD rows: a rebind banner (shown while capturing) + one keycap button per
/// PLAYER-scope signal (fills `kb_rows`), DERIVED from the signal catalog — grouped by
/// [`SignalGroup`], each row keyed by the signal's stable `name()`. No hand-authored list:
/// a signal marked `RebindScope::Player` appears here automatically (MCP `C60AE43C §2`).
/// The keycap fires `rebind_<name>` and shows `bind_<name>`; the scene owns the capture.
fn kb_rows_nodes() -> Vec<UiNode> {
    let mut banner = settings_line(
        "$set_press_any_key_to_bind_esc_to_cancel_back",
        24.0,
        14.0,
        "settings.rebind_banner.text_color",
        "body",
        "left",
    );
    banner.visible_bind = Some("rebinding".to_string());
    let mut out = vec![banner];
    for group in SignalGroup::ALL {
        let mut rows = Vec::new();
        for sig in ActionSignal::rebindable().filter(|s| s.group() == *group) {
            let name = sig.name();
            let mut name_cell = kind("cell");
            name_cell.grow = Some(1.0);
            let label = sig.token();
            name_cell.children = vec![
                grow_stack(1.0),
                settings_line(
                    &label,
                    18.0,
                    16.0,
                    "settings.row.name_color",
                    "body",
                    "left",
                ),
                grow_stack(1.0),
            ];

            let mut cap = kind("button");
            cap.id = format!("kc_{name}");
            cap.action = Some(format!("rebind_{name}"));
            cap.size = Some(KEYCAP_W);
            cap.props
                .insert("text_bind".to_string(), Value::Text(format!("bind_{name}")));
            cap.props
                .insert("label_size".to_string(), Value::Number(KEYCAP_LABEL_SIZE));
            cap.props
                .insert("variant".to_string(), Value::Text("secondary".to_string()));

            let mut r = kind("row");
            r.size = Some(42.0);
            r.children = vec![name_cell, cap];
            rows.push(r);
        }
        // A group with no Player-scope member draws no header (e.g. Camera / Nav / Text).
        if rows.is_empty() {
            continue;
        }
        out.push(group_head(group.token()));
        out.append(&mut rows);
        out.push(fixed_stack(10.0));
    }
    out
}

/// INPUT · MOUSE rows: the pointer + commander groups (fills `mouse_rows`).
fn mouse_rows_nodes(schema: &SettingsSchema) -> Vec<UiNode> {
    let mut out = Vec::new();
    add_groups(&mut out, &schema.input.mouse.groups);
    out
}

/// The controller PROFILE selector options — data-driven from the built-in
/// [`InputProfile::PRESET_NAMES`] (spec §7.3): `value` is the 0-based index (the scene
/// maps it back to the profile's stable name), `label` the display string. This replaces
/// the Lua `PROFILES` global read — the roster is compiled, so there is always ≥1 option.
fn controller_options() -> Vec<UiNode> {
    InputProfile::PRESET_NAMES
        .iter()
        .enumerate()
        .map(|(i, (_name, label))| {
            let mut n = kind("option");
            n.props.insert("value".to_string(), Value::Number(i as f64));
            n.props
                .insert("label".to_string(), Value::Text((*label).to_string()));
            n
        })
        .collect()
}

/// INPUT · CONTROLLER: a profile selector (the named InputProfiles) + the info notes
/// (fills `controller_rows`; the Lua `controller_tab`).
fn controller_rows_nodes(schema: &SettingsSchema) -> Vec<UiNode> {
    let c = &schema.input.controller;

    let mut label_cell = kind("cell");
    label_cell.grow = Some(1.0);
    label_cell.children = vec![
        fixed_stack(8.0),
        settings_line(
            "$set_active_profile",
            20.0,
            16.0,
            "settings.row.name_color",
            "body",
            "left",
        ),
    ];

    let mut select = kind("select");
    select.id = "ctrl_profile".to_string();
    select.size = Some(CTRL_W);
    select.props.insert(
        "style".to_string(),
        Value::Text("settings.controls".to_string()),
    );
    select.children = controller_options();
    select.bind = Some("ctrl_profile".to_string());

    let mut select_row = kind("row");
    select_row.size = Some(CTRL_W + 88.0);
    select_row.gap = 8.0;
    select_row.children = vec![grow_stack(1.0), select];

    let mut profile_row = kind("row");
    profile_row.size = Some(50.0);
    profile_row.children = vec![label_cell, select_row];

    vec![
        group_head("$set_controller_profile"),
        profile_row,
        fixed_stack(16.0),
        settings_line(
            &c.title,
            30.0,
            c.title_size as f32,
            "settings.input.controller.title_color",
            "display",
            "left",
        ),
        settings_line(
            &c.body,
            26.0,
            15.0,
            "settings.input.controller.body_color",
            "body",
            "left",
        ),
    ]
}

/// Build the settings screen tree ONCE: parse the STATIC scene (`settings.scene.json`),
/// then FILL each empty per-section container with hardened Rust-built rows read from the
/// scene's row schema. The untrusted Lua composes NO structure (the client is in the
/// enemy's hands; security). Menu fill precedent: [`main_menu_tree`].
/// Assign the flat `settings_rows` nav group + a running `nav_ordinal` to every interactive
/// control in a freshly-built section, top-to-bottom (pre-order), so the d-pad walks the
/// rows in visual order. Recurses the row wrappers; only value controls + keycap buttons
/// become focusable (labels, spacers, badges and group heads stay inert). The rails are NOT
/// numbered — they are driven by L2/R2 (pages) and L1/R1 (sub-tabs), never the d-pad. The
/// footer's Restore/Apply/Save are authored at ordinals 9000+ so they follow the rows.
/// Nav-tier contract MCP `1B5F6BB8`.
fn number_steppables(nodes: &mut [UiNode], group: &str, next: &mut u32) {
    for n in nodes.iter_mut() {
        if !n.id.is_empty()
            && matches!(
                n.component.as_str(),
                "select" | "slider" | "toggle" | "pill_toggle" | "button"
            )
        {
            n.tab_group = group.to_string();
            n.nav_ordinal = *next;
            *next += 1;
        }
        number_steppables(&mut n.children, group, next);
    }
}

/// The resolution dropdown's options — DEVICE-enumerated (`display::enumerate`), not
/// authored: `value` is the 0-based index, `label` the `"W × H"` size (digits + `×`, so
/// the strings gate — which forbids alphabetic display literals — passes). Rust-built like
/// [`controller_options`]; the JSON row carries no `options` array.
fn resolution_options(list: &[display::Resolution]) -> Vec<UiNode> {
    list.iter()
        .enumerate()
        .map(|(i, r)| {
            let mut n = kind("option");
            n.props.insert("value".to_string(), Value::Number(i as f64));
            n.props.insert(
                "label".to_string(),
                Value::Text(format!("{} \u{00d7} {}", r.w, r.h)),
            );
            n
        })
        .collect()
}

fn settings_tree(resolutions: &[display::Resolution]) -> UiNode {
    let fallback = || {
        // Even the degenerate no-scene root keeps its S9 input declaration so Esc → close
        // never depends on layout (the screen IS the declaration).
        let mut n = UiNode {
            component: "surface".to_string(),
            id: "settings".to_string(),
            ..Default::default()
        };
        n.props.insert(
            "on_cancel".to_string(),
            Value::Text("settings_close".to_string()),
        );
        n
    };
    let def: serde_json::Value = match serde_json::from_str(SETTINGS_SCENE_JSON) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("settings.scene.json did not parse: {e}");
            return fallback();
        }
    };
    let mut tree = match parse_ui_json(&def["tree"]) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("settings.scene.json tree failed to parse: {e}");
            return fallback();
        }
    };
    let schema: SettingsSchema = match serde_json::from_value(def["styles"]["settings"].clone()) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("settings row schema (styles.settings) did not parse: {e}");
            return tree;
        }
    };
    for (id, mut nodes) in [
        ("video_rows", video_rows_nodes(&schema)),
        ("audio_rows", audio_rows_nodes(&schema)),
        ("kb_rows", kb_rows_nodes()),
        ("mouse_rows", mouse_rows_nodes(&schema)),
        ("controller_rows", controller_rows_nodes(&schema)),
    ] {
        // Each section's controls join the ONE flat `settings_rows` group, numbered from 1
        // top-to-bottom. Sections are never co-visible (the sec_*/sub_* visibility gates
        // prune the hidden ones from the nav ring), so the ordinals may repeat across them.
        let mut ord = 1;
        number_steppables(&mut nodes, "settings_rows", &mut ord);
        match find_by_id_mut(&mut tree, id) {
            Some(container) => container.children = nodes,
            None => {
                tracing::error!("settings fill container `{id}` missing from settings.scene.json")
            }
        }
    }
    // The resolution select's options are DEVICE-enumerated (per-monitor), so they are
    // filled here from the snapshot rather than authored in the scene JSON.
    match find_by_id_mut(&mut tree, "c_resolution") {
        Some(sel) => sel.children = resolution_options(resolutions),
        None => tracing::error!("settings resolution select `c_resolution` missing from the tree"),
    }
    tree
}

impl UnifiedSettingsScene {
    fn new(theme: Theme, input_map: &InputMap, renderer: &Renderer) -> Self {
        // Snapshot the monitor's resolution rungs ONCE (fallback to the static ladder when
        // headless). The tree's resolution options + the index↔size mapping read this.
        let resolutions = display::enumerate(&renderer.video_mode_sizes());
        let entries = theme.lua_textures();
        let textures: Vec<TextureHandle> = entries.iter().map(|(_, h)| *h).collect();
        let styles = flicker::ui::load_styles_strs_for(
            &[SHELL_UI_JSON, SHELL_STYLE_JSON],
            main_scene_styles().as_ref(),
        );
        // The `settings.lua` host is HELD (`script` field) only for its per-frame
        // `derive()` — the untrusted, end-user-editable Lua composes NO structure (the
        // layout is the static scene; the client is in the enemy's hands; security). Its
        // SOLE job is turning the published section / sub-tab INDICES into the `sec_*` /
        // `sub_*` visibility gates, so it needs no `UI` / `PROFILES` / texture globals — it
        // reads only the Model the scene publishes each frame.
        let script = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua").expect("settings.lua loads");
        // Build the declarative tree ONCE from the static scene: parse the authored
        // structure, then FILL each empty per-section container with hardened Rust-built
        // rows ([`settings_tree`]). The tree is fully-owned data and every control draws
        // in the engine — the menu's ratified fill precedent (`main_menu_tree`).
        let tree = Some(settings_tree(&resolutions));
        let settings = GAME_SETTINGS.lock().expect("settings lock").clone();
        // The screen's declarative bindings (S9), read off the filled root once.
        let intents = tree.as_ref().map(UiIntents::of).unwrap_or_default();
        Self {
            theme,
            textures,
            tree,
            script,
            styles,
            ui_state: UiState::new(),
            commands: Vec::new(),
            rebind: RebindCapture::new(),
            settings,
            input_map: input_map.clone(),
            resolutions,
            surfaces: settings_sections(),
            input_subtab: "keyboard".to_string(),
            ctrl_profile: "xbox_souls".to_string(),
            scroll_off: 0.0,
            applied: 0.0,
            rebind_esc_prev: false,
            rebind_bs_prev: false,
            dirty: false,
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

    /// The active section's index in the page rail (0 video / 1 audio / 2 input) —
    /// what the `settings_page` vertical `tabs` binds to (the resting echo) and what
    /// the dispatch reads back to switch sections.
    fn section_index(&self) -> usize {
        match self.section() {
            "audio" => 1,
            "input" => 2,
            _ => 0,
        }
    }

    /// The active sub-tab's position in [`INPUT_SUBTABS`] — what the `input_subtab`
    /// strip binds to.
    fn subtab_index(&self) -> usize {
        INPUT_SUBTABS
            .iter()
            .position(|s| *s == self.input_subtab)
            .unwrap_or(0)
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

        // ── Publish ONLY the flag surfaces the Lua does NOT own: the rebind banner + the
        //    applied flash, and the two modal-dialog gates. These are scene-state flags,
        //    NOT index-derived, so they stay Rust-published. The `sec_*` / `sub_*` section
        //    + sub-tab VISIBILITY is deliberately no longer published here — `settings.lua`
        //    `derive()` folds it in from the `settings_page` / `input_subtab` indices below
        //    (the scene's one untrusted knob). The `sec_*` / `sub_*` radios remain the
        //    internal section-state truth (`section()`); `model()` simply no longer
        //    PUBLISHES them — the Lua re-derives them from the index this still publishes. ──
        m.set("rebinding", self.surfaces.is_on("rebinding"));
        m.set("applied", self.surfaces.is_on("applied"));
        m.set("confirm_close", self.surfaces.is_on("confirm_close"));
        m.set("restore_note", self.surfaces.is_on("restore_note"));

        // ── the page rail echoes the resting section (0 video / 1 audio / 2 input) and the
        //    input sub-tab strip its index; the Lua derives `sec_*` / `sub_*` /
        //    `input_page_active` from these. The rail owns its active/idle styling via
        //    tab_active/tab_idle — no nav styling is published. ──
        m.set("settings_page", self.section_index() as f64);
        m.set("input_subtab", self.subtab_index() as f64);
        m.set("ctrl_profile", self.profile_index() as f64);

        // ── scroll (two-way offset; the wheel rides UiInput) + the inert gate ──
        m.set("scroll_off", self.scroll_off as f64);
        m.set("off", false); // unwired controls point `enabled_bind` here → inert

        // ── wired VIDEO (display mode + resolution ride the live DisplaySetting) ──
        let disp = display::current();
        m.set("video_display_mode", display::mode_index(disp.mode) as f64);
        m.set(
            "video_resolution",
            display::resolution_index(&self.resolutions, disp.res) as f64,
        );
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

        // Current keyboard bindings → the key caps show real keys, LOCALIZED
        // (`bind_<signal>`). Same derived set the page's rows come from, so a keycap and
        // its publish can never drift. Slot-0 binding only (the page edits one slot).
        for action in ActionSignal::rebindable() {
            let label = self
                .input_map
                .bindings_for(action)
                .first()
                .map(binding_label)
                .unwrap_or_default();
            m.set(format!("bind_{}", action.name()), label);
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
            gs.input_profile
                .set_context_map("World", self.input_map.clone());
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
    fn modal_flow(surfaces: &mut Sections, results: &ValueMap) -> Option<ModalFlow> {
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
    fn close_requested(surfaces: &mut Sections, results: &ValueMap, dirty: bool) -> bool {
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

    /// Menu context: the pump resolves Esc/pad-B → `Cancel`, arrows/d-pad → `Nav*`,
    /// Enter/A → `Confirm` for this overlay (the same map MenuView seeds from). The
    /// runner resolves it BEFORE `update`, so the walker below sees `signals.events`.
    fn input_context(&self) -> Option<InputContext> {
        Some(InputContext::Menu)
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
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
        let bs_down = input.key_down(Key::Backspace);
        let rebind_bs_edge = bs_down && !self.rebind_bs_prev;
        self.rebind_bs_prev = bs_down;

        // Derived surface flags: the banner mirrors the capture, the flash its timer.
        self.surfaces.set("rebinding", self.rebind.is_active());
        self.surfaces.set("applied", self.applied > 0.0);

        // One walker pass: lay out + hit-test + draw the cached tree. The wheel
        // rides `UiInput.wheel`; the `list` region under the pointer consumes it.
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen: size,
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        // Fold the untrusted `settings.lua` `derive()` in BEFORE the walk (mirrors the
        // clicktrainer HUD fold): the engine publishes the RAW Model, then the Lua turns the
        // published section / sub-tab INDICES into the `sec_*` / `sub_*` / `input_page_active`
        // VISIBILITY gates the walker reads. A publish / derive failure degrades to no gate
        // change (the tree — hardened Rust — still draws).
        let mut model = self.model();
        if let Err(e) = self.script.set_model(&model) {
            tracing::error!("settings: publishing model failed: {e}");
        }
        match self.script.derive() {
            Ok(Some(derived)) => {
                for (k, v) in derived.entries() {
                    model.set(k.clone(), v.clone());
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!("settings derive() failed: {e}"),
        }
        let frame = run_ui(tree, &model, &self.styles, &snap, &mut self.ui_state);
        self.commands = frame.commands;
        let mut results = frame.results;
        let hud_hit = results.is_on("hud_hit");
        self.fired_sigs.clear(); // last frame's mirror rode the walk above — done

        // ── The input seam (input-P3, 0569DA9B): the PUMP already resolved this frame's
        //    Menu-context edges (Esc/B → Cancel, arrows/d-pad → Nav*, …) for the scene's
        //    declared `input_context()` — the scene owns no Resolver. Dispatch
        //    `signals.events` through the walker layer, which turns the screen's DECLARED
        //    `on_cancel = "settings_close"` into a fired result name. Runs even while a
        //    rebind captures (the walker still draws), but the rebind branch below returns
        //    before the results ladder, so a fired name is simply dropped for that frame. ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, hud_hit)
            .with_nav(tree, &model)
            .with_intents(&self.intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        // Fired intents fold into results the SAME way a click does, and queue
        // for the one-frame `sig_<name>` Model mirror.
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
            self.fired_sigs.push(name);
        }
        // Section context wiring (S9): a dialog's show/hide edge (they carry context
        // "Menu") queues Push/PopContext into the pump's route, which the RUNNER reconciles
        // against the shared context stack after `update` — the scene no longer owns a
        // bindings stack. Focus stays the walker's, written directly during dispatch.
        self.surfaces.apply_section_contexts(signals.route);

        // ── Rebind capture (raw Esc or a click cancels; else grab the next input) ──
        // The walker still drew this frame (so the screen updates); its actions
        // — including a bus-fired `settings_close` — are ignored while capturing.
        if self.rebind.is_active() {
            if rebind_esc_edge || input.mouse_left_pressed {
                self.rebind.cancel();
            } else if rebind_bs_edge {
                // Backspace UNBINDS the current action (the banner's advertised behaviour);
                // caught before `poll`, which would otherwise capture Backspace as a key.
                if let Some((action, binding)) = self.rebind.unbind_current(&mut self.input_map) {
                    tracing::info!("unbound {action} (was {binding})");
                    self.dirty = true;
                }
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
        // The page rail reports the selected section INDEX (a number, like every
        // strip); map it back to the `sec_*` radio — the one truth for which section
        // shows. Guarded so the every-frame echo of the resting index is a no-op.
        if let Some(id) = results
            .number("settings_page")
            .and_then(|i| ["video", "audio", "input"].get(i as usize).copied())
        {
            if self.section() != id {
                self.surfaces.set_exclusive(&format!("sec_{id}"));
                self.scroll_off = 0.0;
            }
        }
        // Both strips report an INDEX; the scene maps it back to the name it stands for.
        if let Some(name) = results
            .number("input_subtab")
            .and_then(|i| INPUT_SUBTABS.get(i as usize))
        {
            if *name != self.input_subtab {
                self.surfaces.set_exclusive(&format!("sub_{name}"));
                self.input_subtab = name.to_string();
                self.scroll_off = 0.0;
            }
        }
        if let Some((name, _)) = results
            .number("ctrl_profile")
            .and_then(|i| InputProfile::PRESET_NAMES.get(i as usize))
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
            if let Some(res) = display::resolution_at(&self.resolutions, idx as usize) {
                if res != display::current().res {
                    if let Some(prev) =
                        apply_display_change(DisplayChange::Resolution(res), renderer)
                    {
                        return Transition::Push(Box::new(ConfirmDisplayScene::new(
                            self.theme, prev,
                        )));
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

        // ── Start rebind (a keycap button fires `rebind_<signal>`) — the SAME derived
        //    Player set the page's keycaps come from, so every keycap has a live handler.
        //    The live snapshot seeds the capture's edge baseline, so the very click /
        //    Confirm press that fired this action is prior state, never the captured
        //    binding (the settings self-capture bug, MCP 49DE0F2C). ──
        for action in ActionSignal::rebindable() {
            if results.is_on(&format!("rebind_{}", action.name())) {
                self.rebind.start(action, false, input);
                break;
            }
        }

        Transition::None
    }

    fn render<'f>(&'f mut self, _renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // Blit the commands stashed by `update`'s walker pass (`textures[0]` = white).
        if let Some(&white) = self.textures.first() {
            let commands = &self.commands;
            let textures = &self.textures;
            fg.overlay(move |r| render_hud(r, commands, white, textures));
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
    /// The game's input map at pause time — passed on to the settings overlay so a
    /// rebind starts from the live binds. Input for the pause menu ITSELF comes from the
    /// pump via `input_context()` (Menu); the scene reads no raw device.
    bindings: InputMap,
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
            view: MenuView::from_tree(
                &theme,
                parse_shared_modal(PAUSE_SCENE_JSON, "pause", Some("resume")),
                shared_modal_script(PAUSE_SCRIPT, "pause.lua"),
            ),
            theme,
            bindings: input_map.clone(),
        }
    }
}

impl Scene for PauseScene {
    fn is_overlay(&self) -> bool {
        true
    }

    /// Menu context: the pump resolves Esc/pad-B → `Cancel` (the pause tree declares
    /// `on_cancel = "resume"`, so a back-out resumes the game) and nav/Confirm for the
    /// buttons — the scene reads no raw device.
    fn input_context(&self) -> Option<InputContext> {
        Some(InputContext::Menu)
    }

    fn update(
        &mut self,
        _dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        // ── Modal buttons: Resume, or Esc/pad-B via the declared `on_cancel = "resume"`
        //    (both fold into `results` as the same `resume` name). ──
        let actions = self.view.update(signals, input, renderer, &ValueMap::new());
        if actions.is_on("resume") {
            return Transition::Pop;
        }
        if actions.is_on("settings") {
            return Transition::Push(Box::new(UnifiedSettingsScene::new(
                self.theme,
                &self.bindings,
                renderer,
            )));
        }
        if actions.is_on("main_menu") {
            // Unwind the whole stack (freeing the frozen game scene) back to a fresh
            // menu — the SAME boot menu, the orchestrated `MainMenuScene` (id "Main"),
            // NOT the legacy `MenuScene`. The old tier-push menu's root has no navigable
            // "scenes" focus group, so a controller could not select scenes on return
            // (the mouse hit-tests by rect, so it still could) — the bug this fixes.
            // `Goto{ReplaceRoot}` resolves "Main" through the shared registry
            // (`build_menu` → `MainMenuScene`), so boot and return are identical. The
            // Stage-B cutover (MCP 5099BC88); the legacy `MenuScene` is now deleted (task #6).
            return Transition::Goto {
                id: "Main".into(),
                mode: GotoMode::ReplaceRoot,
            };
        }
        if actions.is_on("quit") {
            return Transition::Quit;
        }
        Transition::None
    }

    fn render<'f>(&'f mut self, _renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        let view = &self.view;
        fg.overlay(move |r| view.render(r));
    }
}

#[cfg(test)]
mod script_smoke {
    //! Load the *embedded* shell scripts and run a frame against a representative
    //! model, so a Lua syntax/runtime error — or a `ui_theme.json` key a script
    //! reads but the embedded layout lacks — fails the build instead of only
    //! surfacing in the running app. The build-time check that keeps the shell's
    //! Rust↔Lua contract honest now that the scripts + layout live in this crate.
    use super::*;
    // The Menu-context edge tests synthesize events with a real resolver — the scenes
    // themselves own none any more (input-P3), so these are test-only imports.
    use flicker_input_core::{Fired, Resolver};

    /// **`menu.lua`'s `arrange()` latches the realm page and lights exactly one
    /// slice.** The main menu is the PRIMARY input context (67DEE93A): a realm button
    /// fires `mode_<realm>`, the engine mirrors it for ONE frame as `sig_mode_<realm>`
    /// (MenuView's S9 mirror), and `arrange()` latches that transient press into a
    /// PERSISTENT page — so the SAME signal from a mouse click or a pad Confirm shows
    /// the same realm, and the selection survives after the one-frame mirror drops.
    /// Gating is by `shown_realm_<n>` visibility, the exact populous `shown_p0_t*`
    /// pattern (`Arrangement::to_model` flattens each `{ on = bool }` onto its
    /// `visible_bind`). Stage 1 of the menu conversion (EB527744): menu.lua is proven
    /// here BEFORE `build_menu` is rewired onto it — additive, nothing else moves.
    #[test]
    fn menu_arrange_latches_the_realm_page_and_lights_one_slice() {
        let host = ScriptHost::new(MENU_SCRIPT, "Main.lua").expect("menu.lua loads");
        // One `arrange()` with an optional press mirror set — the exact seam MenuView
        // drives (set_model ▸ arrange ▸ to_model), on a host HELD across calls so the
        // persistent page (the whole point of the latch) is exercised.
        let arrange_with = |sig: Option<&str>| {
            let mut m = ValueMap::new();
            if let Some(s) = sig {
                m.set(s, true);
            }
            host.set_model(&m).expect("model publishes to menu.lua");
            host.arrange()
                .expect("arrange runs")
                .expect("arrange is present")
                .to_model()
        };

        // Resting state = the landing page (0): its slice is lit, the realms dark.
        let root = arrange_with(None);
        assert!(
            root.is_on("shown_realm_0"),
            "the landing page (0) is lit at rest"
        );
        for n in 1..=4 {
            assert!(
                !root.is_on(&format!("shown_realm_{n}")),
                "no realm lit at rest: {n}"
            );
        }

        // A realm button press mirrors `sig_mode_<realm>` for one frame → the page
        // latches and exactly that realm's slice lights.
        let adv = arrange_with(Some("sig_mode_adventurer"));
        assert!(adv.is_on("shown_realm_1"), "adventurer lights realm 1");
        assert!(!adv.is_on("shown_realm_2"));
        assert!(!adv.is_on("shown_realm_3"));
        assert!(!adv.is_on("shown_realm_4"));

        // Persistence: the transient mirror is gone this frame, but the latched page
        // holds — the selection does not blink off when the one-frame sig drops.
        let held = arrange_with(None);
        assert!(
            held.is_on("shown_realm_1"),
            "the page persists after the mirror drops"
        );

        // Switching realms moves the lit slice, and only ever one is lit.
        let dev = arrange_with(Some("sig_mode_developer"));
        assert!(dev.is_on("shown_realm_4"), "developer lights realm 4");
        assert!(!dev.is_on("shown_realm_1"), "the previous realm darkens");
        let lit = (0..=4)
            .filter(|n| dev.is_on(&format!("shown_realm_{n}")))
            .count();
        assert_eq!(lit, 1, "exactly one page slice is ever lit");
    }

    /// **The Main Menu composes entirely from Rust components.** `Main.scene.json` is the
    /// authored menu (a nav Menu `popup_panel` beside a selector `paged_menu` PTT); every
    /// node names a real component KIND (201F4F51 — no template tier), so it parses with
    /// NO unknown kinds. It also proves the shape `build_menu` will orchestrate: the
    /// realm/mode buttons fire `mode_<realm>`, and the PTT holds the 5 page slices
    /// `menu.lua`'s arrange() lights (`shown_realm_0..4`).
    #[test]
    fn the_main_menu_composes_from_the_rust_components() {
        let def: serde_json::Value =
            serde_json::from_str(MAIN_SCENE_JSON).expect("Main.scene.json parses");
        let tree = parse_ui_json(&def["tree"]).expect("the menu tree parses");

        assert_eq!(tree.component, "surface");
        assert_eq!(tree.id, "main_menu");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "the menu names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );

        fn walk<'a>(n: &'a UiNode, out: &mut Vec<&'a UiNode>) {
            out.push(n);
            for c in &n.children {
                walk(c, out);
            }
        }
        let mut nodes = Vec::new();
        walk(&tree, &mut nodes);

        // The nav panel's buttons — realm modes + settings/quit — all real `button`s.
        let actions: Vec<&str> = nodes
            .iter()
            .filter(|n| n.component == "button")
            .filter_map(|n| n.action.as_deref())
            .collect();
        for a in [
            "mode_adventurer",
            "mode_dm",
            "mode_gamemaster",
            "mode_developer",
            "settings",
            "quit",
        ] {
            assert!(
                actions.contains(&a),
                "the menu is missing the {a} button: {actions:?}"
            );
        }

        // The PTT's 5 page slices, gated on the keys menu.lua's arrange() lights.
        let gates: Vec<&str> = nodes
            .iter()
            .filter_map(|n| n.visible_bind.as_deref())
            .collect();
        for n in 0..=4 {
            let key = format!("shown_realm_{n}");
            assert!(
                gates.contains(&key.as_str()),
                "missing PTT gate {key}: {gates:?}"
            );
        }
    }

    /// B514A222 — the controller scene-selection guard, ported onto the LIVE menu
    /// (`main_menu_tree`); the deleted `MenuScene` twin drove the retired tier-push
    /// menu. A scene-panel LOAD button must be reachable by GAMEPAD focus, not only by
    /// the mouse — the Stage-B reroute (pause "MAIN MENU" → `MainMenuScene`) exists
    /// precisely because the old root left scene rows out of the nav ring. nav and draw
    /// must share one model: a realm slice's LOAD buttons are focusable exactly when
    /// that realm's `shown_realm_<n>` gate is lit, and pruned from nav when it is not
    /// (so the pad never walks into a hidden slice).
    #[test]
    fn scene_load_buttons_are_pad_navigable_on_the_live_menu() {
        // One Adventurer-realm scene (a realm + `SceneInfo` → it becomes a
        // `scene_list_1` row with a LOAD button whose id/action is the scene id). The
        // factory is never called here — `main_menu_tree` reads metadata only.
        let scenes =
            vec![
                SceneEntry::new("solarbirth", "Solar Birth", "primary", |_: &SceneDef| {
                    Box::new(MainMenuScene::new()) as Box<dyn Scene>
                })
                .with_realm(REALM_ADVENTURER)
                .with_info(SceneInfo::new(
                    "Solar Birth",
                    "Cinematic",
                    "Celestial",
                    "d",
                    "m",
                )),
            ];
        let tree = main_menu_tree(&scenes, None);

        // Adventurer is page 1 (`main_menu_tree`'s realm→n map). With `shown_realm_1`
        // lit, the slice is on screen and its LOAD button (id = scene id) is focusable —
        // the pad can reach it, byte-identical to what the mouse hit-tests.
        let shown = ValueMap::new().with("shown_realm_1", true);
        assert!(
            focusables_of(&tree, &shown)
                .iter()
                .any(|f| f.id == "solarbirth"),
            "the scene LOAD button is pad-navigable when its realm slice is shown"
        );
        // …and at the landing page (no realm lit) the hidden slice stays OUT of the nav
        // ring — the exact discrepancy B514A222 rode: a slice `run_ui` would not draw
        // must not put a focusable in the pad's path.
        assert!(
            !focusables_of(&tree, &ValueMap::new())
                .iter()
                .any(|f| f.id == "solarbirth"),
            "the hidden realm slice's LOAD button is pruned from nav under the bare model"
        );
    }

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
        let tree = host
            .ui_tree()
            .expect("tree parses")
            .expect("screen has a tree");
        let styles = flicker::ui::load_styles_str(
            r#"{ "btn": { "fill_top": [0.14, 0.25, 0.47, 1], "radius": 4,
                 "label": [0.9, 0.9, 0.85, 1], "label_size": 14 } }"#,
        );
        let input = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(200.0, 60.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(
            &tree,
            &ValueMap::new(),
            &styles,
            &input,
            &mut UiState::new(),
        );

        let panels: Vec<_> = frame
            .commands
            .iter()
            .filter(|c| matches!(c, HudCommand::Panel { .. }))
            .collect();
        let texts = frame
            .commands
            .iter()
            .filter(|c| matches!(c, HudCommand::Text { .. }))
            .count();
        assert_eq!(panels.len(), 1, "the button drew its slab");
        assert_eq!(texts, 1, "the button drew its label");
        assert!(
            frame
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Text { text, .. } if text == "OK")),
            "the button's top-level `label` prop reached the draw"
        );
        // Column pad 8 in a 200×60 screen → the button's flow rect is (8, 8, 184, 44).
        if let HudCommand::Panel { x, y, w, h, .. } = panels[0] {
            assert_eq!(
                (*x, *y, *w, *h),
                (8.0, 8.0, 184.0, 44.0),
                "layout engine placed the leaf"
            );
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
        let tree = host
            .ui_tree()
            .expect("tree parses")
            .expect("screen has a tree");
        let styles = flicker::ui::load_styles_str(
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
                right_down: false,
                screen: Vec2::new(200.0, 60.0),
                wheel: 0.0,
                exclusive: false,
                motion: Default::default(),
            };
            run_ui(&tree, &model, &styles, &input, &mut UiState::new())
                .commands
                .iter()
                .filter(|c| matches!(c, HudCommand::Panel { .. }))
                .count()
        };
        assert_eq!(
            panels_at(Vec2::new(-9.0, -9.0)),
            1,
            "idle button: just the slab"
        );
        assert_eq!(
            panels_at(Vec2::new(100.0, 30.0)),
            2,
            "hovered button: glow halo + slab"
        );
    }

    #[test]
    fn shared_modals_render_and_draw_their_buttons() {
        // The SHARED modal trees (`scenes/shared/pause|confirm.scene.json`) parse and DRAW:
        // the Rust walker renders the popup_panel chrome + every authored button, resolving
        // the shell-furniture styles (`modal`/`screens.*` from Main's carrier). Proves the
        // migrated files render at build time — the runtime path PauseScene/ConfirmDisplayScene
        // walk. Button labels are read back FROM the parsed tree, so the file is the one source.
        use flicker::render::Vec2;
        use flicker::script::HudCommand;

        fn button_labels(node: &UiNode, out: &mut Vec<String>) {
            if node.component == "button" {
                if let Some(Value::Text(l)) = node.props.get("label") {
                    out.push(l.clone());
                }
            }
            for c in &node.children {
                button_labels(c, out);
            }
        }

        let styles = flicker::ui::load_styles_strs_for(
            &[SHELL_UI_JSON, SHELL_STYLE_JSON],
            main_scene_styles().as_ref(),
        );
        for (screen, json, script) in [
            ("pause", PAUSE_SCENE_JSON, PAUSE_SCRIPT),
            ("confirm", CONFIRM_SCENE_JSON, CONFIRM_SCRIPT),
        ] {
            // The real production parse path (`PauseScene`/`ConfirmDisplayScene` load this).
            let tree = parse_shared_modal(json, screen, None);
            let mut labels = Vec::new();
            button_labels(&tree, &mut labels);
            assert!(
                !labels.is_empty(),
                "shared modal '{screen}' authors its buttons"
            );
            // The countdown subtitle, composed around its token exactly as the scene does —
            // PLUS the pair script's default `arrange()` gates. The optional buttons are
            // `visible_bind`-gated now, so the render folds the slices exactly as MenuView does
            // live; without them a gated button correctly would not draw.
            let mut model = ValueMap::new();
            model.set(
                "subtitle",
                format!("{} 9s", flicker::ui::strings::resolve("$menu_reverting_in")),
            );
            let host =
                ScriptHost::new(script, &format!("{screen}.lua")).expect("pair script loads");
            if let Some(a) = host.arrange().expect("arrange runs") {
                model.extend(a.to_model());
            }
            let snap = UiInput {
                mouse: Vec2::new(-1.0, -1.0),
                clicked: false,
                down: false,
                right_down: false,
                screen: Vec2::new(1920.0, 1080.0),
                wheel: 0.0,
                exclusive: false,
                motion: Default::default(),
            };
            let frame = run_ui(&tree, &model, &styles, &snap, &mut UiState::new());
            assert!(
                !frame.commands.is_empty(),
                "shared modal '{screen}' emits panel + buttons + text"
            );
            // Every authored button's label renders as a text command — the tree actually
            // produced its buttons. Labels are stringtable tokens, so the RESOLVED text lands.
            for label in &labels {
                let want = flicker::ui::strings::resolve(label);
                assert!(
                    frame
                        .commands
                        .iter()
                        .any(|c| matches!(c, HudCommand::Text { text, .. } if *text == want)),
                    "shared modal '{screen}' renders button label '{want}'"
                );
            }
            // The confirm modal's countdown is a LIVE bind the popup_panel chrome draws
            // (`subtitle_bind` → `subtitle_live`); the model's current text reaches a command.
            if screen == "confirm" {
                let want = model
                    .text("subtitle")
                    .expect("countdown published")
                    .to_string();
                assert!(
                    frame
                        .commands
                        .iter()
                        .any(|c| matches!(c, HudCommand::Text { text, .. } if *text == want)),
                    "the confirm modal draws the live countdown '{want}'"
                );
            }
        }
    }

    /// THE SPLASH PAIR GATE: each intro scene's pair script is a MODULE like every
    /// pair script — its `arrange()` configures the `splash` component (an `image`
    /// prop naming a readable content file, a positive fade timeline) and its
    /// `react()` maps the fired signals to the `next`/`exit` intents, which the
    /// scene FILE routes to authored scenes. (The drawing lives in the Rust
    /// `splash` component; there is no script update/draw to drive any more.)
    #[test]
    fn splash_pair_scripts_configure_the_splash() {
        let m = manifest();
        for (id, next) in [("TegLogo", "CeLogo"), ("CeLogo", "Loading")] {
            let (src, name) = splash_script(id);
            let host = ScriptHost::new(src, name).expect("the pair script loads");

            // arrange(): the splash entry carries the component's configuration.
            let a = host
                .arrange()
                .expect("arrange() runs")
                .expect("arrange() is exposed");
            let splash = a
                .components
                .get("splash")
                .unwrap_or_else(|| panic!("'{id}' arrange() configures the `splash` component"));
            assert!(splash.on, "'{id}' lights its splash");
            let rel = match splash.props.get("image") {
                Some(Value::Text(t)) => t.as_str(),
                other => panic!("'{id}': `image` prop names the splash image, got {other:?}"),
            };
            assert!(
                !splash_image(id, rel).is_empty(),
                "'{id}': image \"{rel}\" reads from the content root"
            );
            let secs = |key: &str| match splash.props.get(key) {
                Some(Value::Number(n)) => *n,
                _ => 0.0,
            };
            assert!(
                secs("fade_in") + secs("hold") + secs("fade_out") > 0.0,
                "'{id}' has a positive timeline"
            );

            // react(): the timeline completing advances, Cancel backs out.
            let advance = host
                .react(&ValueMap::new().with(SIG_DONE, true))
                .expect("react() runs")
                .expect("react() is exposed");
            assert!(
                advance.is_on(SPLASH_NEXT),
                "'{id}': `{SIG_DONE}` yields `{SPLASH_NEXT}`"
            );
            let back = host
                .react(&ValueMap::new().with(SIG_CANCEL, true))
                .expect("react() runs")
                .expect("react() is exposed");
            assert!(
                back.is_on(SPLASH_EXIT),
                "'{id}': `{SIG_CANCEL}` yields `{SPLASH_EXIT}`"
            );

            // The FILE routes both intents, to authored scenes.
            let def = m
                .get(id)
                .unwrap_or_else(|| panic!("'{id}' ships a scene file"));
            assert_eq!(
                goto_target(def.exit(SPLASH_NEXT)),
                Some((next.to_string(), GotoMode::Replace)),
                "'{id}' advances to {next}"
            );
            for intent in [SPLASH_NEXT, SPLASH_EXIT] {
                let (target, _) = goto_target(def.exit(intent))
                    .unwrap_or_else(|| panic!("'{id}' routes `{intent}` in its file"));
                assert!(
                    m.get(&target).is_some(),
                    "'{id}' routes `{intent}` to '{target}', which is an authored scene"
                );
            }
        }
    }

    /// The pre-load screen's pair script is a module like every other: `derive()`
    /// turns the engine's `loading_progress` into the percent readout the bar's
    /// label binds, and `react()` maps the timeline completing — or a Confirm click-
    /// through — to `next`, and a back-out to `exit`. Its FILE routes both intents to
    /// authored scenes.
    #[test]
    fn loading_pair_script_derives_percent_and_routes() {
        let host = ScriptHost::new(LOADING_SCRIPT, "Loading.lua").expect("the pair script loads");

        // derive(): loading_progress (0..1) → the human percent, composed in Lua.
        for (p, pct) in [(0.0_f64, "0%"), (0.42, "42%"), (1.0, "100%")] {
            host.set_model(&ValueMap::new().with("loading_progress", p))
                .expect("model publishes");
            let d = host
                .derive()
                .expect("derive() runs")
                .expect("derive() is exposed");
            assert_eq!(d.text("progress_pct"), Some(pct), "{p} reads as {pct}");
        }

        // react(): the timeline completing advances, Cancel backs out, a Confirm is
        // ignored (a load must not be click-skipped — the whole point of the notice).
        let done = host
            .react(&ValueMap::new().with(SIG_DONE, true))
            .expect("react() runs")
            .expect("react() is exposed");
        assert!(
            done.is_on(SPLASH_NEXT),
            "`{SIG_DONE}` yields `{SPLASH_NEXT}`"
        );
        let back = host
            .react(&ValueMap::new().with(SIG_CANCEL, true))
            .expect("react() runs")
            .expect("react() is exposed");
        assert!(
            back.is_on(SPLASH_EXIT),
            "`{SIG_CANCEL}` yields `{SPLASH_EXIT}`"
        );
        let confirmed = host
            .react(&ValueMap::new().with(SIG_CONFIRM, true))
            .expect("react() runs")
            .expect("react() is exposed");
        assert!(
            confirmed.is_on(SPLASH_NEXT),
            "`{SIG_CONFIRM}` clicks through to `{SPLASH_NEXT}`"
        );

        // The FILE routes both intents, to authored scenes.
        let m = manifest();
        let def = m.get("Loading").expect("Loading ships a scene file");
        for intent in [SPLASH_NEXT, SPLASH_EXIT] {
            let (target, _) = goto_target(def.exit(intent))
                .unwrap_or_else(|| panic!("Loading routes `{intent}` in its file"));
            assert!(
                m.get(&target).is_some(),
                "Loading routes `{intent}` to '{target}', which is an authored scene"
            );
        }
    }

    /// The pre-load screen sits BETWEEN the engine logo and the menu: CeLogo now
    /// advances to Loading, and Loading advances to Main. Proven off the DATA so a
    /// re-order is a file edit — the `loading` behaviour resolves by id like the rest.
    #[test]
    fn loading_is_page_three_of_the_intro_chain() {
        let m = manifest();
        assert_eq!(
            goto_target(m.get("CeLogo").expect("CeLogo ships").exit(SPLASH_NEXT)),
            Some(("Loading".to_string(), GotoMode::Replace)),
            "the engine splash advances to the pre-load screen"
        );
        assert_eq!(
            goto_target(m.get("Loading").expect("Loading ships").exit(SPLASH_NEXT)),
            Some(("Main".to_string(), GotoMode::Replace)),
            "the pre-load screen advances to the menu"
        );
        assert!(
            builtin_behaviours().contains(&"loading"),
            "`loading` is a shell builtin behaviour"
        );
        assert!(
            resolve_shell_scene("Loading").is_some(),
            "Loading resolves by id"
        );
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
        assert_eq!(
            manifest().boot(),
            "TegLogo",
            "the boot scene is authored, not compiled in"
        );
        for id in ["TegLogo", "CeLogo", "Loading", "Main"] {
            assert!(
                resolve_shell_scene(id).is_some(),
                "'{id}' is in the shell roster"
            );
        }
        assert!(
            resolve_shell_scene("no_such_scene").is_none(),
            "an unknown id resolves to None"
        );
    }

    /// Client-registered entries resolve on BOTH paths: an AUTHORED bench file builds
    /// through the entry whose id its `behaviour` names (the client half of the
    /// behaviour registry, receiving the def), and a FILE-LESS entry (a single-scene
    /// client's `start`) still resolves through the synthetic-def fallthrough.
    #[test]
    fn a_registered_bench_id_resolves_through_the_fallthrough() {
        fn dummy(_: &SceneDef) -> Box<dyn Scene> {
            Box::new(MainMenuScene::new())
        }
        set_scenes(vec![
            SceneEntry::new("solarbirth", "Solar Birth", "primary", dummy),
            SceneEntry::new("clicktrainer", "Click Trainer", "primary", dummy),
            SceneEntry::new("start", "ENTER WORLD", "primary", dummy),
        ]);
        assert!(
            resolve_shell_scene("solarbirth").is_some(),
            "an authored bench file builds via the entry its `behaviour` names"
        );
        assert!(resolve_shell_scene("clicktrainer").is_some());
        assert!(
            resolve_shell_scene("start").is_some(),
            "a file-less registered id resolves via the synthetic-def fallthrough"
        );
        assert!(
            resolve_shell_scene("no_such_scene").is_none(),
            "an id in neither the manifest nor the registry stays None"
        );
        set_scenes(Vec::new());
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
    /// REGRESSION (blank logos, 2026-08-17): the splash node is a `sprite` located by its
    /// stable id "splash" — the `splash` component was folded into `sprite`, so keying
    /// `find_splash` on the retired `"splash"` kind returned None and `enter()` never read the
    /// `image` prop nor set `tex`. Pins that every shipped splash (and the synthesized fallback)
    /// exposes a findable sprite logo node.
    #[test]
    fn find_splash_locates_the_sprite_logo_node() {
        for id in ["TegLogo", "CeLogo"] {
            let path = scenes_dir().join(format!("{id}{}", flicker::ui::SCENE_FILE_SUFFIX));
            let shipped = std::fs::read_to_string(&path).expect("the splash ships a file");
            let def = SceneDef::parse(id, &shipped).expect("splash scene parses");
            let tree = def.tree.expect("the splash authors a tree");
            let node = find_splash(&tree)
                .unwrap_or_else(|| panic!("{id}: find_splash locates the logo node"));
            assert_eq!(
                node.component, "sprite",
                "{id}: the logo node is a sprite (splash folded in)"
            );
            assert_eq!(node.id, "splash");
        }
        // A file-less splash's synthesized node is a findable sprite too.
        assert_eq!(
            find_splash(&synthesized_splash_tree())
                .expect("synthesized node findable")
                .component,
            "sprite",
        );
    }

    #[test]
    fn a_splash_exit_comes_from_its_file() {
        let path = scenes_dir().join(format!("TegLogo{}", flicker::ui::SCENE_FILE_SUFFIX));
        let shipped = std::fs::read_to_string(&path).expect("the publisher splash ships a file");
        let def = SceneDef::parse("TegLogo", &shipped).expect("it loads");
        assert_eq!(
            goto_target(def.exit(SPLASH_NEXT)),
            Some(("CeLogo".to_string(), GotoMode::Replace)),
            "as shipped, the publisher splash hands off to the engine splash"
        );

        // Re-author the file's target and reload: same code, different chain.
        let rerouted = shipped.replace("\"CeLogo\"", "\"Main\"");
        assert_ne!(rerouted, shipped, "the edit found the authored target");
        let def = SceneDef::parse("TegLogo", &rerouted).expect("edited loads");
        assert_eq!(
            goto_target(def.exit(SPLASH_NEXT)),
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
        assert!(
            m.len() >= 3,
            "the shell ships at least the two splashes and the menu"
        );
        for def in m.scenes() {
            for (result, target) in def.targets() {
                assert!(
                    m.get(target).is_some(),
                    "scene '{}' exits `{result}` to '{target}', which is no authored scene",
                    def.id
                );
            }
            // A SHELL behaviour must build here and now. A CLIENT behaviour is played
            // by the entry the client registers at run() — this crate cannot see those,
            // so the client's own manifest gate closes that half (it asserts every
            // non-builtin behaviour has a registered entry of the same name).
            if BEHAVIOURS.iter().any(|(n, _)| *n == def.behaviour) {
                assert!(
                    resolve_shell_scene(&def.id).is_some(),
                    "scene file '{}' has a Rust behaviour bound to it",
                    def.id
                );
            } else {
                assert!(
                    !def.behaviour.is_empty(),
                    "scene file '{}' must name the client behaviour that plays it",
                    def.id
                );
            }
        }
    }

    /// ABSENCE GATE: the `.tree.json` form is DEAD (violation 1F151933 — an invented
    /// parallel scene-def spelling). A UI arrangement is a `.scene.json` in the
    /// manifest folder, or a Rust composite the behaviour builds (pause/confirm).
    /// A stray file matching the form is the defect returning.
    #[test]
    fn the_tree_json_form_is_dead() {
        fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, hits);
                } else if p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".tree.json"))
                {
                    hits.push(p.display().to_string());
                }
            }
        }
        let mut hits = Vec::new();
        scan(&flicker_content::roots().sensorium(), &mut hits);
        assert!(
            hits.is_empty(),
            "the .tree.json form is dead (1F151933); found: {hits:?}"
        );
    }

    /// Both splashes must declare BOTH intents their behaviour fires. Without
    /// `next` a splash plays its fade and then sits there forever — the exact
    /// silent dead-end the scene file exists to make authorable; without `exit`
    /// backing out is a dead control.
    #[test]
    fn both_splashes_declare_next_and_exit() {
        let m = manifest();
        for id in ["TegLogo", "CeLogo"] {
            let def = m
                .get(id)
                .unwrap_or_else(|| panic!("'{id}' ships a scene file"));
            for intent in [SPLASH_NEXT, SPLASH_EXIT] {
                assert!(
                    def.exit(intent).is_some(),
                    "splash '{id}' routes the `{intent}` intent its behaviour fires"
                );
            }
        }
    }

    #[test]
    fn settings_tree_runs_every_section() {
        // The declarative `settings.lua` builds a component tree; the walker draws it.
        // This is the walker-drive analogue of the old immediate-mode smoke test: it
        // parses settings.lua (template-free now — no `expand`) and runs `run_ui` for each
        // section, asserting the section's marker content renders (a Lua typo or an unknown
        // kind would fall out here) — the same shape as `menu_template_tests`.
        use flicker::render::Vec2;
        use flicker::script::HudCommand;

        // The screen's display copy is `$token`s now (S10 strings gate); load the
        // shipped table so the walked commands carry the resolved en-us text.
        flicker::ui::strings::load_str(SHELL_STRINGS_JSON, "en-us");
        let styles = flicker::ui::load_styles_strs_for(
            &[SHELL_UI_JSON, SHELL_STYLE_JSON],
            main_scene_styles().as_ref(),
        );
        // The layout is the STATIC scene filled by hardened Rust rows — the PRODUCTION tree
        // ([`settings_tree`]); the untrusted Lua composes no structure. Its `derive()` still
        // drives the section / sub-tab VISIBILITY, so hold a host to fold it in like `update`.
        let tree = settings_tree(&display::RESOLUTIONS);
        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua").expect("load settings.lua");

        // The per-frame model the scene publishes for `(section, sub-tab)`: the RAW page /
        // sub-tab INDICES (`settings_page` / `input_subtab`) the Lua derive turns into the
        // `sec_*` / `sub_*` / `input_page_active` visibility gates, plus the control binds
        // (every strip selection a 0-based INDEX). This exercises the REAL path — the gates
        // come from the Lua derive fold, not from a hand-set model.
        let model = |section: &str, subtab: &str| {
            let mut m = ValueMap::new();
            let sec_idx = ["video", "audio", "input"]
                .iter()
                .position(|s| *s == section)
                .unwrap_or(0);
            m.set("settings_page", sec_idx as f64);
            m.set(
                "input_subtab",
                INPUT_SUBTABS.iter().position(|s| *s == subtab).unwrap_or(0) as f64,
            );
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
            // Fold the untrusted `settings.lua` derive the SAME way `update` does.
            host.set_model(&m).expect("publish settings model");
            if let Some(derived) = host.derive().expect("settings derive()") {
                for (k, v) in derived.entries() {
                    m.set(k.clone(), v.clone());
                }
            }
            m
        };
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1920.0, 1080.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let has = |cmds: &[HudCommand], s: &str| {
            cmds.iter()
                .any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
        };
        let run = |section: &str, subtab: &str| {
            run_ui(
                &tree,
                &model(section, subtab),
                &styles,
                &snap,
                &mut UiState::new(),
            )
            .commands
        };

        let video = run("video", "keyboard");
        // The hand-authored window chrome: a corner rune glyph proves `rune_corners` drew.
        assert!(has(&video, "ᛞ"), "window rune corners render");
        // The vertical page rail draws all three section labels on every page (it IS the
        // section indicator now), and the active video section's own rows render below.
        assert!(
            has(&video, "VIDEO") && has(&video, "INPUT"),
            "page rail shows the section labels"
        );
        assert!(has(&video, "Display Mode"), "video section rows");
        assert!(
            has(&run("audio", "keyboard"), "NOT YET IMPLEMENTED"),
            "audio stub"
        );
        assert!(
            has(&run("input", "keyboard"), "MOVEMENT"),
            "input keyboard groups"
        );
        assert!(
            has(&run("input", "mouse"), "Look Sensitivity"),
            "input mouse rows"
        );
        // Controller tab is a data-driven CONTROLLER-config selector (§7.3): the notes copy
        // renders, and the active controller config (`PRESET_NAMES[0]` = `xbox_souls`) shows
        // its label — proving the selector options came from the profile roster.
        let controller = run("input", "controller");
        assert!(
            has(&controller, "Choose a control profile"),
            "controller notes render"
        );
        assert!(
            has(&controller, "Default (Xbox)"),
            "selector shows the roster label for the default controller config"
        );
    }

    /// **The settings modal is pad-navigable** (nav-tier contract 1B5F6BB8): every control
    /// of the VISIBLE section joins the one flat `settings_rows` group with the footer,
    /// hidden sections are pruned from the ring, the rails are NOT focusable (they are the
    /// L2/R2 · L1/R1 tier), and the footer follows the rows. This is the root-cause fix —
    /// the settings tree used to carry zero `tab_group`, so the pad passed straight through.
    #[test]
    fn the_settings_modal_is_pad_navigable() {
        flicker::ui::strings::load_str(SHELL_STRINGS_JSON, "en-us");
        let tree = settings_tree(&display::RESOLUTIONS);
        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua").expect("load settings.lua");
        let model = |section: &str, subtab: &str| {
            let mut m = ValueMap::new();
            let sec = ["video", "audio", "input"]
                .iter()
                .position(|s| *s == section)
                .unwrap_or(0);
            m.set("settings_page", sec as f64);
            m.set(
                "input_subtab",
                INPUT_SUBTABS.iter().position(|s| *s == subtab).unwrap_or(0) as f64,
            );
            m.set("off", false);
            host.set_model(&m).expect("publish settings model");
            if let Some(derived) = host.derive().expect("settings derive()") {
                for (k, v) in derived.entries() {
                    m.set(k.clone(), v.clone());
                }
            }
            m
        };

        // Input · Keyboard: the ring is the keycaps + the footer, and NOTHING from the other
        // sections; the rails are absent; one flat group; the footer follows the keycaps.
        let kb = flicker::ui::focusables_of(&tree, &model("input", "keyboard"));
        let ids: Vec<&str> = kb.iter().map(|f| f.id.as_str()).collect();
        assert!(
            ids.contains(&"kc_MoveForward") && ids.contains(&"kc_Quit"),
            "keycaps are focusable"
        );
        assert!(
            ids.contains(&"restore") && ids.contains(&"apply") && ids.contains(&"save_close"),
            "the footer is in the ring",
        );
        assert!(
            !ids.iter().any(|id| id.starts_with("c_m_")),
            "mouse controls pruned off the keyboard sub-tab"
        );
        assert!(
            !ids.contains(&"settings_page") && !ids.contains(&"input_subtab"),
            "the rails are NOT focusable — they are the shoulder tier",
        );
        assert!(
            kb.iter().all(|f| f.group == "settings_rows"),
            "one flat settings group"
        );
        let restore = kb
            .iter()
            .find(|f| f.id == "restore")
            .expect("restore focusable");
        assert!(
            kb.iter()
                .filter(|f| f.id.starts_with("kc_"))
                .all(|f| f.ordinal < restore.ordinal),
            "the footer follows the rows",
        );

        // Video: the resolution select is focusable; no keycaps leak onto the video page.
        let vid = flicker::ui::focusables_of(&tree, &model("video", "keyboard"));
        let vids: Vec<&str> = vid.iter().map(|f| f.id.as_str()).collect();
        assert!(
            vids.contains(&"c_resolution"),
            "the resolution select is focusable"
        );
        assert!(
            !vids.iter().any(|id| id.starts_with("kc_")),
            "keycaps pruned off the video page"
        );
    }

    /// The settings root declares the four rail STEP intents, so L2/R2 (pages) and L1/R1
    /// (sub-tabs) fire the names the rails carry as `next_action`/`prev_action` — the strip
    /// then steps its own bind (no scene stepper, no new ladder arm). Nav-tier contract.
    #[test]
    fn the_settings_root_declares_the_rail_step_intents() {
        use flicker_input_core::ActionSignal;
        let intents = flicker::ui::UiIntents::of(&settings_tree(&display::RESOLUTIONS));
        assert_eq!(
            intents.result_for(ActionSignal::PageNext),
            Some("page_next")
        );
        assert_eq!(
            intents.result_for(ActionSignal::PagePrev),
            Some("page_prev")
        );
        assert_eq!(intents.result_for(ActionSignal::TabNext), Some("tab_next"));
        assert_eq!(intents.result_for(ActionSignal::TabPrev), Some("tab_prev"));
        assert_eq!(
            intents.result_for(ActionSignal::Cancel),
            Some("settings_close")
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
        let styles = flicker::ui::load_styles_strs_for(
            &[SHELL_UI_JSON, SHELL_STYLE_JSON],
            main_scene_styles().as_ref(),
        );
        // The PRODUCTION tree: the STATIC scene filled by hardened Rust rows ([`settings_tree`]).
        // The untrusted `settings.lua` derive drives section visibility; fold it as `update` does.
        let tree = settings_tree(&display::RESOLUTIONS);
        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua").expect("load settings.lua");

        // The published selection: index 2 (1920×1080), the shell's own default rung.
        const SHOWN: usize = 2;
        const PICK: usize = 0;
        let mut m = ValueMap::new();
        // RAW indices — the Lua derive produces `sec_*` / `sub_*` (video shows) from these.
        m.set("settings_page", 0.0);
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
        // Fold the untrusted derive the SAME way `update` does → the section visibility gates.
        host.set_model(&m).expect("publish settings model");
        if let Some(derived) = host.derive().expect("settings derive()") {
            for (k, v) in derived.entries() {
                m.set(k.clone(), v.clone());
            }
        }

        let at = |x: f32, y: f32, clicked: bool| UiInput {
            mouse: Vec2::new(x, y),
            clicked,
            down: clicked,
            right_down: false,
            screen: Vec2::new(1920.0, 1080.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
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
        run_ui(
            &tree,
            &m,
            &styles,
            &at(x + w * 0.5, y + h * 0.5, true),
            &mut state,
        );
        let row_y = y + h + 6.0 + 30.0 * PICK as f32 + 15.0;
        let f = run_ui(&tree, &m, &styles, &at(x + 20.0, row_y, true), &mut state);

        assert_eq!(
            f.results.number("video_resolution"),
            Some(PICK as f64),
            "the picked row reports its index as a NUMBER"
        );
        assert_eq!(
            f.results.text("video_resolution"),
            None,
            "…and never as text"
        );
        // …and that index is what moves the window: a different rung than the shown one.
        assert_ne!(
            display::resolution_at(&display::RESOLUTIONS, PICK),
            display::resolution_at(&display::RESOLUTIONS, SHOWN),
            "the pick names a different resolution — the change `update` applies"
        );
    }

    /// The resolution options are DEVICE-enumerated, not authored: the built select carries
    /// exactly one option per snapshot rung labelled `"W × H"`, and the scene JSON no longer
    /// ships the hard-coded list. (Strings-gate safety of the digit+`×` labels is covered by
    /// `the_shipped_screens_name_only_kinds_the_engine_knows` over the production tree.)
    #[test]
    fn the_resolution_options_are_device_enumerated() {
        let list = display::enumerate(&[(1280, 720), (1920, 1080), (2560, 1440)]);
        let mut tree = settings_tree(&list);
        let sel = find_by_id_mut(&mut tree, "c_resolution").expect("resolution select present");
        assert_eq!(
            sel.children.len(),
            list.len(),
            "one option per enumerated rung, device-built"
        );
        for (opt, r) in sel.children.iter().zip(&list) {
            let label = match opt.props.get("label") {
                Some(Value::Text(t)) => t.clone(),
                _ => String::new(),
            };
            assert_eq!(
                label,
                format!("{} \u{00d7} {}", r.w, r.h),
                "label is the W × H size"
            );
        }
        assert!(
            !SETTINGS_SCENE_JSON.contains("1920 \u{00d7} 1080"),
            "the hard-coded resolution options are gone from the scene JSON",
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

        // The settings screen, built exactly as the scene caches it: the STATIC tree
        // ([`settings_tree`]) filled by hardened Rust rows — the untrusted Lua composes no
        // structure, but the root's S9 `on_cancel` declaration is authored right in the scene.
        let tree = settings_tree(&display::RESOLUTIONS);

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
        let mut walker = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
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
        use flicker_input_router::{apply_context_requests, RouteCtx};

        let close = ValueMap::new().with("settings_close", true);
        let mut surfaces = settings_sections();

        // Dirty close → the confirm dialog goes up, nothing pops.
        assert!(!UnifiedSettingsScene::close_requested(
            &mut surfaces,
            &close,
            true
        ));
        assert!(
            surfaces.is_on("confirm_close"),
            "dirty close raises the confirm dialog"
        );

        // Its context flip routes through the S9 seam: Menu pushed while up.
        let mut route = RouteCtx::new();
        let mut bindings = ContextualBindings::new(InputMap::empty());
        surfaces.apply_section_contexts(&mut route);
        apply_context_requests(&mut bindings, &route.requests);
        route.requests.clear();
        assert_eq!(
            bindings.active(),
            InputContext::Menu,
            "the dialog holds its declared context"
        );

        // The dialog INTERCEPTS the next close intent: it dismisses the dialog
        // (Esc = Cancel), the overlay itself stays open.
        assert_eq!(
            UnifiedSettingsScene::modal_flow(&mut surfaces, &close),
            Some(ModalFlow::Stay)
        );
        assert!(
            !surfaces.is_on("confirm_close"),
            "the intent cancelled the dialog, not settings"
        );
        surfaces.apply_section_contexts(&mut route);
        apply_context_requests(&mut bindings, &route.requests);
        route.requests.clear();
        assert_eq!(
            bindings.active(),
            InputContext::World,
            "…and its context popped with it"
        );

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
        assert!(
            !surfaces.is_on("restore_note"),
            "the intent dismissed the ack"
        );

        // No dialog + clean → the close intent pops.
        assert_eq!(
            UnifiedSettingsScene::modal_flow(&mut surfaces, &close),
            None
        );
        assert!(UnifiedSettingsScene::close_requested(
            &mut surfaces,
            &close,
            false
        ));
    }

    /// PAIR-SCRIPT CONTRACT GATE (Stage 3): `settings.lua` is the modern `derive()`-only half
    /// of the settings pair and must NEVER regrow a `tree()` builder — that would put layout
    /// structure + control-selection logic back into the untrusted, end-user-editable layer,
    /// an exploit surface (the client is in the enemy's hands; the layout lives in the STATIC
    /// `settings.scene.json`, all behaviour in hardened Rust). Assert it loads on the pair
    /// contract, exposes NO `tree` hook (`ui_tree()` → `Ok(None)`), and that `derive()` — its
    /// one runtime knob — turns a published section index into the matching visibility gate.
    #[test]
    fn settings_lua_is_a_derive_only_pair_script_never_a_tree_builder() {
        let host = ScriptHost::new(SETTINGS_SCRIPT, "settings.lua").expect("settings.lua loads");
        // No `tree` hook — structure is the static scene, not a Lua builder.
        assert!(
            host.ui_tree().expect("ui_tree() probes cleanly").is_none(),
            "settings.lua must expose NO tree() — structure stays in settings.scene.json"
        );
        // `derive()` is the ONLY behaviour: a published section index → the `sec_*` gate.
        host.set_model(&ValueMap::new().with("settings_page", 1.0))
            .expect("publish the index");
        let derived = host
            .derive()
            .expect("derive() runs")
            .expect("settings.lua exposes derive()");
        assert!(
            derived.is_on("sec_audio"),
            "settings_page = 1 derives sec_audio visible"
        );
        assert!(
            !derived.is_on("sec_video"),
            "…and only that section (sec_video off)"
        );
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
            gs.input_profile
                .context_map("World")
                .unwrap()
                .action_for(InputBinding::Key(Key::W)),
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
        let reloaded_world = loaded
            .input_profile
            .context_map("World")
            .expect("World persists");
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
            gs.input_profile
                .context_map("World")
                .unwrap()
                .action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveForward),
            "missing profile defaults to WASD World",
        );
    }
}
