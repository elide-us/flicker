//! flicker-script: a minimal client-only Luau scripting layer.
//!
//! The host embeds a Luau VM (via [`mlua`]) and runs a single Lua
//! *module* that owns a small piece of HUD state — currently a set of
//! interactive checkboxes. Each frame the engine:
//!
//! 1. feeds the input snapshot to the script ([`ScriptHost::update`]),
//!    which hit-tests clicks and flips its own checkbox state, then
//!    returns the resulting named [`Toggles`];
//! 2. asks the script for its HUD draw list ([`ScriptHost::draw`]),
//!    which the engine renders.
//!
//! ## Layering
//!
//! This crate deliberately does **not** depend on `flicker-render`.
//! Scripts never touch the GPU; they emit plain-data [`HudCommand`]s
//! describing rectangles and text in HUD-pixel space, and the
//! consumer (which owns the renderer) turns those into draw calls.
//! That keeps UI *logic* in the script and UI *rendering* in the
//! engine, with the data structs here as the only contract between
//! them. The crate does depend on `flicker-core` for the engine's
//! [`InputState`] — the input snapshot, not the renderer.
//!
//! The Lua side is plain Lua (no Luau-specific syntax) so the same
//! script would run on any Lua 5.x; it just happens to execute on the
//! configured Luau VM.
//!
//! ## Script contract
//!
//! The loaded chunk must evaluate to a table exposing two functions:
//!
//! ```lua
//! local M = {}
//! -- Called once per frame. Hit-test the click, flip state, and
//! -- return a table of {name = bool} toggle states.
//! function M.update(mouse_x, mouse_y, clicked) ... return { ... } end
//! -- Called once per frame. Return a sequence of draw-command tables;
//! -- see HudCommand for the recognised shapes.
//! function M.draw() return { ... } end
//! return M
//! ```

use std::collections::HashMap;
use std::path::Path;

use flicker_core::InputState;
use mlua::{Function, Lua, Table};

/// Errors raised while loading or running a HUD script.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// The script file could not be read off disk.
    #[error("failed to read script file '{path}': {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    /// The Lua VM raised an error while loading or calling the script,
    /// or the script returned a value of an unexpected shape.
    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
}

/// A draw command emitted by a HUD script for the engine to render.
///
/// Coordinates are in HUD pixel space (origin top-left), matching the
/// renderer's 2D conventions. Colors are RGBA in `0.0..=1.0`. These
/// map directly onto the renderer's `draw_sprite` (for [`Self::Rect`],
/// using a 1×1 white texture tinted by `color`) and `draw_text`.
#[derive(Clone, Debug, PartialEq)]
pub enum HudCommand {
    /// A solid filled rectangle.
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    /// A line of text with its top-left at `(x, y)`.
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: [f32; 4],
    },
}

/// The set of named boolean toggles a HUD script exposes after an
/// [`update`](ScriptHost::update). Names are defined by the script
/// (e.g. `"wireframe"`); the consumer queries the ones it cares about.
#[derive(Clone, Debug, Default)]
pub struct Toggles {
    map: HashMap<String, bool>,
}

impl Toggles {
    /// The state of the named toggle, or `false` if the script does
    /// not define it.
    pub fn is_on(&self, name: &str) -> bool {
        self.map.get(name).copied().unwrap_or(false)
    }
}

/// A loaded HUD script and its Luau VM.
///
/// Owns the [`Lua`] state for its whole lifetime: mlua values such as
/// the module [`Table`] hold only a *weak* reference to the VM, so the
/// host must keep `lua` alive even though it is not read directly
/// after construction.
pub struct ScriptHost {
    // Keeps the Luau VM alive; the module table below borrows it weakly.
    #[allow(dead_code)]
    lua: Lua,
    module: Table,
}

impl ScriptHost {
    /// Load and evaluate a HUD script from a source string. `chunk_name`
    /// labels the chunk in error messages and stack traces.
    pub fn new(source: &str, chunk_name: &str) -> Result<Self, ScriptError> {
        let lua = Lua::new();
        let module: Table = lua.load(source).set_name(chunk_name).eval()?;
        // Fail fast if the contract is not met, rather than at the
        // first frame: both entry points must be present and callable.
        let _: Function = module.get("update")?;
        let _: Function = module.get("draw")?;
        Ok(Self { lua, module })
    }

    /// Load and evaluate a HUD script from a file on disk. The path is
    /// also used as the chunk name.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ScriptError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path).map_err(|source| ScriptError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::new(&source, &path.display().to_string())
    }

    /// Run the script's per-frame `update`, feeding it the current
    /// mouse position and whether the left button was *pressed this
    /// frame* (the click edge). Returns the script's named toggle
    /// states.
    pub fn update(&self, input: &InputState) -> Result<Toggles, ScriptError> {
        let update: Function = self.module.get("update")?;
        let mouse = input.mouse_position;
        let states: Table = update.call((mouse.x, mouse.y, input.mouse_left_pressed))?;

        let mut map = HashMap::new();
        for pair in states.pairs::<String, bool>() {
            let (name, on) = pair?;
            map.insert(name, on);
        }
        Ok(Toggles { map })
    }

    /// Run the script's per-frame `draw` and collect its HUD commands.
    /// Unrecognised command kinds are skipped with a warning rather
    /// than failing the frame.
    pub fn draw(&self) -> Result<Vec<HudCommand>, ScriptError> {
        let draw: Function = self.module.get("draw")?;
        let list: Table = draw.call(())?;

        let mut commands = Vec::new();
        for item in list.sequence_values::<Table>() {
            let cmd = item?;
            let kind: String = cmd.get("kind")?;
            match kind.as_str() {
                "rect" => commands.push(HudCommand::Rect {
                    x: cmd.get("x")?,
                    y: cmd.get("y")?,
                    w: cmd.get("w")?,
                    h: cmd.get("h")?,
                    color: read_color(&cmd)?,
                }),
                "text" => commands.push(HudCommand::Text {
                    x: cmd.get("x")?,
                    y: cmd.get("y")?,
                    text: cmd.get("text")?,
                    size: cmd.get::<Option<f32>>("size")?.unwrap_or(16.0),
                    color: read_color(&cmd)?,
                }),
                other => tracing::warn!("hud script emitted unknown command kind '{other}'"),
            }
        }
        Ok(commands)
    }
}

/// Read an RGBA color from a command table. Each channel defaults to
/// `1.0` when omitted, so scripts can write `r=…, g=…, b=…` and leave
/// alpha implicit.
fn read_color(cmd: &Table) -> mlua::Result<[f32; 4]> {
    Ok([
        cmd.get::<Option<f32>>("r")?.unwrap_or(1.0),
        cmd.get::<Option<f32>>("g")?.unwrap_or(1.0),
        cmd.get::<Option<f32>>("b")?.unwrap_or(1.0),
        cmd.get::<Option<f32>>("a")?.unwrap_or(1.0),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"
        local M = {}
        local checked = false
        function M.update(mx, my, clicked)
            if clicked and mx >= 0 and mx <= 10 and my >= 0 and my <= 10 then
                checked = not checked
            end
            return { box = checked }
        end
        function M.draw()
            return {
                { kind = "rect", x = 0, y = 0, w = 10, h = 10, r = 1, g = 1, b = 1 },
                { kind = "text", x = 12, y = 0, text = "hi", size = 14 },
            }
        end
        return M
    "#;

    fn input_at(x: f32, y: f32, clicked: bool) -> InputState {
        let mut input = InputState::new();
        input.mouse_position = glam::Vec2::new(x, y);
        input.mouse_left_pressed = clicked;
        input
    }

    #[test]
    fn missing_entry_points_fail_to_load() {
        match ScriptHost::new("return {}", "bad") {
            Err(ScriptError::Lua(_)) => {}
            Err(other) => panic!("expected a lua error, got {other:?}"),
            Ok(_) => panic!("expected loading a module without update/draw to fail"),
        }
    }

    #[test]
    fn click_inside_toggles_state() {
        let host = ScriptHost::new(SCRIPT, "test").unwrap();

        // No click: off.
        let toggles = host.update(&input_at(5.0, 5.0, false)).unwrap();
        assert!(!toggles.is_on("box"));

        // Click inside the box: on.
        let toggles = host.update(&input_at(5.0, 5.0, true)).unwrap();
        assert!(toggles.is_on("box"));

        // Click outside leaves it on (no flip).
        let toggles = host.update(&input_at(50.0, 50.0, true)).unwrap();
        assert!(toggles.is_on("box"));

        // Click inside again flips it back off.
        let toggles = host.update(&input_at(5.0, 5.0, true)).unwrap();
        assert!(!toggles.is_on("box"));
    }

    #[test]
    fn unknown_toggle_is_off() {
        let host = ScriptHost::new(SCRIPT, "test").unwrap();
        let toggles = host.update(&input_at(0.0, 0.0, false)).unwrap();
        assert!(!toggles.is_on("nope"));
    }

    #[test]
    fn draw_returns_rect_and_text() {
        let host = ScriptHost::new(SCRIPT, "test").unwrap();
        let cmds = host.draw().unwrap();
        assert_eq!(
            cmds,
            vec![
                HudCommand::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                },
                HudCommand::Text {
                    x: 12.0,
                    y: 0.0,
                    text: "hi".to_string(),
                    size: 14.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                },
            ]
        );
    }
}
