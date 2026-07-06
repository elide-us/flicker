//! Display settings (window mode + resolution) for the flicker-csg client,
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

/// A full display setting: presentation mode + resolution.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DisplaySetting {
    pub mode: DisplayMode,
    pub res: Resolution,
}

impl DisplaySetting {
    /// The startup default: 1080p windowed. (The runner opens a 960×540
    /// *logical* window, which is 1920×1080 physical on a 2× display.)
    pub const DEFAULT: DisplaySetting = DisplaySetting {
        mode: DisplayMode::Windowed,
        res: Resolution { w: 1920, h: 1080 },
    };

    /// Apply this setting to the window via the renderer.
    pub fn apply(self, renderer: &Renderer) {
        match self.mode {
            DisplayMode::Windowed => renderer.set_windowed(self.res.w, self.res.h),
            DisplayMode::BorderlessFullscreen => renderer.set_borderless_fullscreen(),
            DisplayMode::ExclusiveFullscreen => {
                renderer.set_exclusive_fullscreen(self.res.w, self.res.h);
            }
        }
    }
}

/// Process-wide current setting (the window is the real state; this mirrors it
/// so the confirm overlay can revert).
static CURRENT: Mutex<DisplaySetting> = Mutex::new(DisplaySetting::DEFAULT);

/// The last-applied display setting.
pub fn current() -> DisplaySetting {
    *CURRENT.lock().expect("display settings lock")
}

/// Record `setting` as current (call right after applying it to the window) and
/// persist it to `settings.json`.
pub fn set_current(setting: DisplaySetting) {
    *CURRENT.lock().expect("display settings lock") = setting;
    save_to_disk(setting);
}

/// Path to the persisted settings file (next to the crate, like `bake/`).
fn settings_path() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/settings.json"))
}

/// Load the persisted display setting from `settings.json` into [`CURRENT`], if
/// the file exists and parses. Best-effort: a missing or invalid file leaves
/// the default in place. Call once at startup, before the window opens.
pub fn load_from_disk() {
    let path = settings_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    match serde_json::from_slice::<DisplaySetting>(&bytes) {
        Ok(setting) => {
            *CURRENT.lock().expect("display settings lock") = setting;
            tracing::info!("loaded display settings from {}", path.display());
        }
        Err(e) => tracing::warn!("ignoring invalid {}: {e}", path.display()),
    }
}

/// Write `setting` to `settings.json` (best-effort; logs on failure).
fn save_to_disk(setting: DisplaySetting) {
    let path = settings_path();
    match serde_json::to_vec_pretty(&setting) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!("failed to write {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("failed to serialize display settings: {e}"),
    }
}
