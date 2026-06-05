//! Display settings (window mode + resolution) for the voxel-cluster example,
//! plus a tiny process-wide store so the choice persists across scenes — the
//! seed of "feature management beyond game state".
//!
//! The window is the real source of truth (a scene applies a change straight to
//! it via the [`Renderer`]); [`CURRENT`] mirrors the last-applied setting so the
//! settings panel can show the active selection and the confirm overlay can
//! revert it.

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

    pub fn label(self) -> &'static str {
        match self {
            DisplayMode::Windowed => "Windowed",
            DisplayMode::BorderlessFullscreen => "Fullscreen Window",
            DisplayMode::ExclusiveFullscreen => "Fullscreen",
        }
    }
}

/// A selectable resolution in physical pixels.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Resolution {
    pub w: u32,
    pub h: u32,
}

/// The smallest "debug" window — the lowest rung in the ladder (below the 720p
/// floor); the resolution list filters up from here.
pub const DEBUG_RES: Resolution = Resolution { w: 960, h: 540 };

/// Heights (px) offered above the 720p floor, before capping at native.
const LADDER_HEIGHTS: [u32; 3] = [720, 1080, 1440];

/// Build the resolution list for a monitor of physical size `monitor` (falls
/// back to a 1080p assumption when unknown): the debug default, the in-ratio
/// ladder entries strictly below native height, then Native. Deduped and kept
/// short (Apple-simple).
pub fn resolution_options(monitor: Option<(u32, u32)>) -> Vec<Resolution> {
    let (mw, mh) = monitor.unwrap_or((1920, 1080));
    let aspect = mw as f32 / mh.max(1) as f32;
    let mut out = vec![DEBUG_RES];
    for &h in &LADDER_HEIGHTS {
        if h < mh {
            // Even width matching the monitor aspect (720p floor enforced by
            // the table; native is added separately below).
            let w = (((h as f32 * aspect).round() as u32) / 2) * 2;
            out.push(Resolution { w, h });
        }
    }
    out.push(Resolution { w: mw, h: mh }); // Native
                                           // Dedup by (w, h), preserving order (tiny list, O(n²) is fine).
    let mut i = 0;
    while i < out.len() {
        if out[..i].contains(&out[i]) {
            out.remove(i);
        } else {
            i += 1;
        }
    }
    out
}

/// Human label for a resolution, marking the debug default and the native one.
pub fn resolution_label(r: Resolution, monitor: Option<(u32, u32)>) -> String {
    if r == DEBUG_RES {
        format!("{} × {}  (debug)", r.w, r.h)
    } else if monitor == Some((r.w, r.h)) {
        format!("Native  ({} × {})", r.w, r.h)
    } else {
        format!("{} × {}", r.w, r.h)
    }
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
/// so panels can show the selection and the confirm overlay can revert).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_1080p_is_debug_720_native() {
        assert_eq!(
            resolution_options(Some((1920, 1080))),
            vec![
                DEBUG_RES,
                Resolution { w: 1280, h: 720 },
                Resolution { w: 1920, h: 1080 },
            ]
        );
    }

    #[test]
    fn ladder_720p_monitor_is_debug_plus_native() {
        assert_eq!(
            resolution_options(Some((1280, 720))),
            vec![DEBUG_RES, Resolution { w: 1280, h: 720 }]
        );
    }

    #[test]
    fn ladder_4k_has_full_ladder() {
        assert_eq!(
            resolution_options(Some((3840, 2160))),
            vec![
                DEBUG_RES,
                Resolution { w: 1280, h: 720 },
                Resolution { w: 1920, h: 1080 },
                Resolution { w: 2560, h: 1440 },
                Resolution { w: 3840, h: 2160 },
            ]
        );
    }
}
