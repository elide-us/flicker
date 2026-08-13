//! Display settings (window mode + resolution) for the flicker-shell front-end,
//! plus a tiny process-wide store so the choice persists across scenes — the
//! seed of "feature management beyond game state".
//!
//! The window is the real source of truth (a scene applies a change straight to
//! it via the [`Renderer`]); [`CURRENT`] mirrors the last-applied setting so the
//! confirm overlay can revert it.

use std::path::PathBuf;
use std::sync::Mutex;

use flicker::render::Renderer;
use serde::{Deserialize, Serialize};

/// Window presentation mode.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DisplayMode {
    Windowed,
    BorderlessFullscreen,
    ExclusiveFullscreen,
}

impl DisplayMode {
    /// The three modes, in dropdown order.
    pub const ALL: [DisplayMode; 3] = [
        DisplayMode::Windowed,
        DisplayMode::BorderlessFullscreen,
        DisplayMode::ExclusiveFullscreen,
    ];
}

/// A selectable resolution in physical pixels.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Resolution {
    pub w: u32,
    pub h: u32,
}

/// A full display setting: presentation mode + resolution + (windowed) position.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DisplaySetting {
    pub mode: DisplayMode,
    pub res: Resolution,
    /// Last windowed outer position (top-left, physical px). `None` = let the OS
    /// place the window (first launch). Persisted so the window reopens where it
    /// was; on restore it is clamped fully on-screen. Meaningless in fullscreen.
    #[serde(default)]
    pub pos: Option<[i32; 2]>,
}

impl DisplaySetting {
    /// The startup default: 1080p windowed, OS-placed. `apply` clamps the size to
    /// the monitor, so on a smaller screen the window shrinks to fit rather than
    /// opening larger than the display.
    pub const DEFAULT: DisplaySetting = DisplaySetting {
        mode: DisplayMode::Windowed,
        // A comfortably-sub-screen first-launch size (720p). `apply` clamps it down
        // further on a smaller monitor; the user's resized + persisted size takes
        // over after the first run.
        res: Resolution { w: 1280, h: 720 },
        pos: None,
    };

    /// Apply this setting to the window via the renderer. Windowed mode CLAMPS the
    /// size to the monitor (never larger than the screen) and, if a position is
    /// stored, restores it nudged fully on-screen.
    pub fn apply(self, renderer: &Renderer) {
        match self.mode {
            DisplayMode::Windowed => {
                let (mw, mh) = renderer.monitor_size().unwrap_or((self.res.w, self.res.h));
                let w = self.res.w.min(mw);
                let h = self.res.h.min(mh);
                renderer.set_windowed(w, h);
                if let Some([px, py]) = self.pos {
                    // Clamp so the whole window stays on-screen. The window is now
                    // ≤ the monitor, so a valid on-screen position always exists.
                    let max_x = mw.saturating_sub(w) as i32;
                    let max_y = mh.saturating_sub(h) as i32;
                    renderer.set_outer_position(px.clamp(0, max_x), py.clamp(0, max_y));
                }
            }
            DisplayMode::BorderlessFullscreen => renderer.set_borderless_fullscreen(),
            DisplayMode::ExclusiveFullscreen => {
                renderer.set_exclusive_fullscreen(self.res.w, self.res.h);
            }
        }
    }
}

impl Default for DisplaySetting {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Windowed resolution rungs the settings panel offers (physical px). These MUST stay
/// index-aligned with the `settings.video.resolution` dropdown options in
/// `ui_theme.json` — this array is the single source for that index↔size mapping.
/// `apply` clamps the chosen rung to the monitor, so a rung larger than the screen
/// simply fits to the screen.
pub const RESOLUTIONS: [Resolution; 6] = [
    Resolution { w: 1280, h: 720 },
    Resolution { w: 1600, h: 900 },
    Resolution { w: 1920, h: 1080 },
    Resolution { w: 2560, h: 1440 },
    Resolution { w: 3440, h: 1440 },
    Resolution { w: 3840, h: 2160 },
];

/// The dropdown index of a display mode (its position in [`DisplayMode::ALL`]).
pub fn mode_index(mode: DisplayMode) -> usize {
    DisplayMode::ALL.iter().position(|m| *m == mode).unwrap_or(0)
}

/// The resolution dropdown index CLOSEST to `res` (nearest rung by pixel distance),
/// so an off-ladder size (a drag-resized window) still shows a sensible selection.
pub fn resolution_index(res: Resolution) -> usize {
    RESOLUTIONS
        .iter()
        .enumerate()
        .min_by_key(|(_, r)| {
            let dw = i64::from(r.w) - i64::from(res.w);
            let dh = i64::from(r.h) - i64::from(res.h);
            dw * dw + dh * dh
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// The resolution at dropdown index `idx`, if in range.
pub fn resolution_at(idx: usize) -> Option<Resolution> {
    RESOLUTIONS.get(idx).copied()
}

/// Process-wide current setting (the window is the real state; this mirrors it
/// so the confirm overlay can revert).
static CURRENT: Mutex<DisplaySetting> = Mutex::new(DisplaySetting::DEFAULT);

/// The last-applied display setting.
pub fn current() -> DisplaySetting {
    *CURRENT.lock().expect("display settings lock")
}

/// Record `setting` as current (call right after applying it to the window) and
/// persist it — the shell folds it into the unified `settings.json` (one file, one
/// writer), so it no longer clobbers or is clobbered by the game settings.
pub fn set_current(setting: DisplaySetting) {
    {
        *CURRENT.lock().expect("display settings lock") = setting;
    }
    crate::shell::persist_display_setting(setting);
}

/// Seed [`CURRENT`] from a loaded setting WITHOUT persisting — used by the shell's
/// settings load so restoring the saved value doesn't immediately re-save it.
pub(crate) fn seed(setting: DisplaySetting) {
    *CURRENT.lock().expect("display settings lock") = setting;
}

/// Where the per-user `settings.json` is read/written — the running app's project
/// root, set once by the shell from `ShellConfig::settings_dir` so each shell app
/// keeps its own (gitignored) settings in its own root. Falls back to the current
/// working directory when unset. Shared by the display settings here and the full
/// settings in `shell.rs`.
static SETTINGS_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Set the directory the shell persists `settings.json` into (the app's project
/// root, usually its `CARGO_MANIFEST_DIR`). Call once at startup, before the shell
/// loads the settings.
pub fn set_settings_dir(dir: Option<PathBuf>) {
    *SETTINGS_DIR.lock().expect("settings dir lock") = dir;
}

/// The directory the per-user `settings.json` lives in — the app root if set, else
/// the current working directory.
pub fn settings_dir() -> PathBuf {
    SETTINGS_DIR
        .lock()
        .expect("settings dir lock")
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

// Display persistence is unified into the shell's `GameSettings` (one `settings.json`,
// one writer): `set_current` folds a change into it, and the shell's settings load
// seeds `CURRENT` via `seed`. The old DisplaySetting-only file I/O — which shared the
// same file with `GameSettings` and clobbered it — is gone.
