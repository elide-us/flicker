//! flicker-script: a minimal client-only Luau scripting layer.
//!
//! The host embeds a Luau VM (via [`mlua`]) and runs Lua *modules* that own
//! pieces of UI logic — the in-game HUD (debug stats + feature checkboxes), the
//! front-end menu, and so on. Lua owns layout/state/interaction; the engine
//! owns rendering and data. This is the seam between them.
//!
//! ## Boundary contract (engine ↔ script) — strictly enforced
//!
//! **This crate is the *only* place the engine and Lua meet.** `mlua` is
//! confined here; no other crate in the workspace depends on it or touches the
//! VM. The types in this module are the **entire** contract surface, and every
//! value crossing the boundary is plain data — named scalars, draw commands,
//! the input snapshot — **never** an engine handle, GPU resource, or borrow.
//! (Consequently this crate deliberately does **not** depend on
//! `flicker-render`; it depends on `flicker-core` only for the engine's
//! [`InputState`] snapshot.) Treat this boundary as load-bearing: widen it only
//! by adding to these contract types, never by reaching across it elsewhere.
//!
//! Three channels, all in named-value / plain-data terms:
//!
//! 1. **Input** (engine → script): the interaction snapshot — mouse position,
//!    the left-click *edge* (`clicked`) and *held* state (`down`, for dragging),
//!    and screen size — passed to [`update`](ScriptHost::update) /
//!    [`draw`](ScriptHost::draw). (The host may also bind shared Lua library
//!    modules via [`set_lua_module`](ScriptHost::set_lua_module) — e.g. a
//!    reusable widgets toolkit — which is code, not data, but still confined to
//!    the VM.)
//! 2. **Data model** (engine → script): a [`ValueMap`] of named engine values
//!    (fps, positions, counts, a setting's current value, …) published each
//!    frame via [`ScriptHost::set_model`] and read by the script as the `Model`
//!    global. This is how a script renders live stats, or shows a slider's
//!    current value. The static `Textures` global
//!    ([`ScriptHost::set_texture_ids`]) is a sibling: name → engine texture id.
//! 3. **Results + draw** (script → engine): [`update`](ScriptHost::update)
//!    returns a [`ValueMap`] of named results (toggles, momentary actions,
//!    slider / value-box values); [`draw`](ScriptHost::draw) returns a
//!    `Vec<`[`HudCommand`]`>` the consumer renders.
//!
//! [`Value`] (bool / number / text) is the only currency crossing in either
//! direction. The boundary is validated at build time by this crate's
//! round-trip test (`model_round_trip`), which is why a strongly-typed Rust
//! contract here suffices and **no external binding-generation step is needed**
//! while the boundary stays Rust-internal (see `docs/ui.md`).
//!
//! The Lua side is plain Lua (no Luau-specific syntax) so the same script would
//! run on any Lua 5.x; it just happens to execute on the configured Luau VM.
//!
//! ## Script contract
//!
//! The loaded chunk must evaluate to a table exposing two functions:
//!
//! ```lua
//! local M = {}
//! -- Called once per frame. Read engine data from the `Model` global, hit-test
//! -- the click, and return a `{name = value}` table of results (bool / number /
//! -- text) — persistent toggles, momentary actions (true only on the firing
//! -- frame), or widget values. `sw`/`sh` are the screen size.
//! function M.update(mouse_x, mouse_y, clicked, sw, sh) ... return { ... } end
//! -- Called once per frame. Return a sequence of draw-command tables; see
//! -- HudCommand for the recognised shapes ("rect"/"sprite"/"panel"/"text"). Globals:
//! -- `Model` (engine data this frame) and `Textures` (name → sprite id).
//! function M.draw(sw, sh) return { ... } end
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

/// Horizontal alignment for a [`HudCommand::Text`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    /// `x` is the text's left edge (the default).
    #[default]
    Left,
    /// `x` is the text's horizontal *center*; the consumer measures the
    /// string and offsets it left by half its width. Used for centring
    /// titles/labels without the script needing font metrics.
    Center,
    /// `x` is the text's *right* edge; the consumer measures the string and
    /// offsets it left by its full width. Used for right-aligned panel values
    /// (status columns, readouts) without the script needing font metrics.
    Right,
}

/// The type role for a [`HudCommand::Text`] — a semantic face selector the
/// consumer maps to a concrete font family (the Prism design language: Display =
/// Cormorant Garamond, Label = Cinzel caps, Body = EB Garamond). Plain data on
/// the boundary — `flicker-script` has no renderer/font dependency, so (exactly
/// like [`TextAlign`]) the role is carried as data and the family mapping lives
/// in the render bridge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontRole {
    /// Display / headings & names.
    Display,
    /// Labels, caps, small meta.
    Label,
    /// Body / prose (the default).
    #[default]
    Body,
}

/// A draw command emitted by a HUD script for the engine to render.
///
/// Coordinates are in HUD pixel space (origin top-left), matching the
/// renderer's 2D conventions. Colors are RGBA in `0.0..=1.0`. `layer` is the
/// painter's-order sort key (higher draws on top), applied *relative* to the
/// scene's base layer by the consumer. These map onto the renderer's
/// `draw_sprite` ([`Self::Rect`] uses a 1×1 white texture tinted by `color`;
/// [`Self::Sprite`] uses an engine texture the host exposed via
/// [`ScriptHost::set_texture_ids`]) and `draw_text`.
#[derive(Clone, Debug, PartialEq)]
pub enum HudCommand {
    /// A solid filled rectangle.
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        layer: f32,
    },
    /// A textured quad drawn from an engine texture, referenced by the `id`
    /// the host registered with [`ScriptHost::set_texture_ids`]. `color`
    /// tints the sampled texel (`[1; 4]` for none).
    Sprite {
        tex: u32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        layer: f32,
    },
    /// A line of text positioned at `(x, y)` per `align`, in the face `font`.
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: [f32; 4],
        layer: f32,
        align: TextAlign,
        font: FontRole,
    },
    /// A **vector UI panel**: a rounded rectangle filled with a solid or 2-stop
    /// linear gradient (`color`→`color2` along `grad`: `0.0` solid, `1.0`
    /// vertical, `2.0` horizontal) and ringed with an optional `border`
    /// (`border_color`; thickness in px, `0.0` = none). `radius` is the corner
    /// radius (px); `feather` softens the outer edge (px, for soft drop shadows).
    /// Rendered as a signed-distance field in one draw by the consumer's
    /// `draw_ui_panel` — the flat Prism chrome (panels / buttons / wells),
    /// replacing the CPU-baked panel and button textures.
    Panel {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        color2: [f32; 4],
        grad: f32,
        radius: f32,
        border: f32,
        border_color: [f32; 4],
        feather: f32,
        layer: f32,
    },
}

/// A parent-relative anchor for an absolutely-placed node — the corner/edge its
/// box is pinned to before `offset` is applied. `None` on a node means it flows
/// in its parent's layout instead of being pinned. Used for HUD clusters pinned
/// to screen edges (paperdoll's top-left toggles / bottom inventory / right gadget).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiAnchor {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

/// A placed UI **component instance** — one node of the tree a screen declares
/// for the Rust component walker to lay out, draw, and hit-test. The inbound
/// counterpart to [`HudCommand`]: instead of the script emitting finished
/// geometry each frame, it names a Rust `component` template and supplies
/// plain-data props once; the walker owns draw / layout / hit-test. Honours the
/// [boundary contract](crate#boundary-contract-engine--script-—-strictly-enforced)
/// — every field is scalar data or child nodes, never a handle.
///
/// Conforms to the ContentForge `PageTreeNode` shape: `component` is the
/// definition reference, `children` order carries the sibling sequence, `bind` is
/// the field binding, and `action`/`visible_bind`/`enabled_bind` are the named
/// behaviours a node can drive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiNode {
    /// Stable identity within the tree (events, focus, layout cache). May be empty.
    pub id: String,
    /// Which Rust template renders this node (`row` / `column` / `panel` /
    /// `button` / `checkbox` / `slider` / `text` / `cell` / …). Required.
    pub component: String,
    /// Child nodes, in draw / sequence order.
    pub children: Vec<UiNode>,
    /// Parent-relative pin, if this node is placed absolutely (else it flows).
    pub anchor: Option<UiAnchor>,
    /// Pixel offset from the anchor (`[dx, dy]`).
    pub offset: [f32; 2],
    /// Fixed main-axis length in the parent's flow (mutually exclusive with `grow`).
    pub size: Option<f32>,
    /// Weight for sharing the parent's leftover main-axis space.
    pub grow: Option<f32>,
    /// Fixed width for an anchored box (`None` = measured from children / content).
    pub width: Option<f32>,
    /// Fixed height for an anchored box (`None` = measured from children / content).
    pub height: Option<f32>,
    /// Gap between children along the main axis (container nodes).
    pub gap: f32,
    /// Inset padding on all sides (container nodes).
    pub pad: f32,
    /// Scalar prop overrides (label, style name, font role, range key, format, …).
    pub props: HashMap<String, Value>,
    /// Two-way data binding: the `Model` key this node reads its value from and
    /// writes edits back to (slider / checkbox value).
    pub bind: Option<String>,
    /// Event name emitted into the results map when the node is activated (clicked).
    pub action: Option<String>,
    /// `Model` key gating visibility — the node (and its subtree) is skipped when false.
    pub visible_bind: Option<String>,
    /// `Model` key gating interactivity — the node draws dim / inert when false.
    pub enabled_bind: Option<String>,
}

/// A single value crossing the engine↔script boundary — the *only* value
/// currency the [boundary contract](crate#boundary-contract-engine--script-—-strictly-enforced)
/// permits in either direction. Plain scalars only; no handles or references.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    /// All Lua numbers (and integers) marshal through `f64`.
    Number(f64),
    Text(String),
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Number(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Number(v as f64)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Number(v as f64)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::Number(v as f64)
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Self {
        Value::Number(v as f64)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}

/// A named-value map: the contract type for both the inbound **data model**
/// (engine → script, [`ScriptHost::set_model`]) and the outbound **results**
/// (script → engine, returned by [`ScriptHost::update`]). Names are defined by
/// whichever side fills it; the other side queries the names it cares about.
#[derive(Clone, Debug, Default)]
pub struct ValueMap {
    map: HashMap<String, Value>,
}

impl ValueMap {
    /// An empty map (to populate with [`set`](ValueMap::set)).
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert/replace a named value. Chains, so a model can be built inline:
    /// `ValueMap::new().with("fps", 60.0).with("name", "hi")`.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.map.insert(name.into(), value.into());
        self
    }

    /// Insert/replace a named value in place.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<Value>) {
        self.map.insert(name.into(), value.into());
    }

    /// The raw value for `name`, if present.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.map.get(name)
    }

    /// `true` iff `name` is present and `Bool(true)` — the toggle/action query.
    pub fn is_on(&self, name: &str) -> bool {
        matches!(self.map.get(name), Some(Value::Bool(true)))
    }

    /// The number for `name`, if present and numeric.
    pub fn number(&self, name: &str) -> Option<f64> {
        match self.map.get(name) {
            Some(Value::Number(n)) => Some(*n),
            _ => None,
        }
    }

    /// The text for `name`, if present and textual.
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.map.get(name) {
            Some(Value::Text(t)) => Some(t),
            _ => None,
        }
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
        // Fail fast if the contract is not met, rather than at the first frame.
        check_contract(&module)?;
        Ok(Self { lua, module })
    }

    /// Like [`new`](Self::new), but first registers a set of **requireable Lua
    /// modules** so the script (and modules) can `require("<name>")` each other —
    /// the seam for splitting the UI into one file per component (a shared `core`
    /// + `button`/`slider`/… + a `layout` engine) instead of one monolith.
    ///
    /// The VM is Luau (sandboxed — no stock `package`/`require`), so this installs
    /// a minimal `require`: each `(name, source)` is compiled to a loader function
    /// held in a private preload table; `require(name)` runs it **once** and
    /// caches the returned value (a table, typically), so modules are singletons
    /// and cyclic-free composition works. Modules are registered *before* the main
    /// chunk is evaluated, so top-level `require` calls in the main script resolve.
    /// Still plain Lua inside the VM — the data boundary is unchanged.
    pub fn new_with_modules(
        source: &str,
        chunk_name: &str,
        modules: &[(&str, &str)],
    ) -> Result<Self, ScriptError> {
        let lua = Lua::new();
        install_require(&lua, modules)?;
        let module: Table = lua.load(source).set_name(chunk_name).eval()?;
        check_contract(&module)?;
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

    /// Expose engine textures to the script as a global `Textures` table
    /// (`{ name = id, ... }`), so scripts can emit
    /// [`HudCommand::Sprite`]s referencing them by name (e.g.
    /// `Textures.panel`). `id` is whatever the host uses to look the
    /// texture back up when rendering. Call once after load (and again if
    /// the set changes, e.g. on theme rebuild).
    pub fn set_texture_ids(&self, ids: &[(&str, u32)]) -> Result<(), ScriptError> {
        let table = self.lua.create_table()?;
        for (name, id) in ids {
            table.set(*name, *id)?;
        }
        self.lua.globals().set("Textures", table)?;
        Ok(())
    }

    /// Publish the engine's per-frame **data model** to the script as the
    /// `Model` global (`{ name = value }`), so scripts can render live stats or
    /// show a widget's current value. Each [`Value`] marshals to its natural Lua
    /// type (bool / number / string). Call once per frame before
    /// [`update`](Self::update) / [`draw`](Self::draw); replaces the previous
    /// frame's `Model`.
    pub fn set_model(&self, model: &ValueMap) -> Result<(), ScriptError> {
        let table = self.lua.create_table()?;
        for (name, value) in &model.map {
            match value {
                Value::Bool(b) => table.set(name.as_str(), *b)?,
                Value::Number(n) => table.set(name.as_str(), *n)?,
                Value::Text(t) => table.set(name.as_str(), t.as_str())?,
            }
        }
        self.lua.globals().set("Model", table)?;
        Ok(())
    }

    /// Expose a JSON value to the script as the global `name`, recursively
    /// marshalled into Lua (objects → tables, arrays → 1-indexed tables,
    /// numbers/bools/strings → their Lua equivalents, null → nil). This is the
    /// **layout / config** inbound channel: it carries static, plain-data trees
    /// like `ui_elements.json` — still only data (no handles), so it honours the
    /// [boundary contract](crate#boundary-contract-engine--script-—-strictly-enforced).
    /// Typically called once at load (call again to hot-reload). Example:
    /// `host.set_global_json("UI", &serde_json::from_str(text)?)` lets scripts
    /// read `UI.menu.panel.w`.
    pub fn set_global_json(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<(), ScriptError> {
        let marshalled = json_to_lua(&self.lua, value)?;
        self.lua.globals().set(name, marshalled)?;
        Ok(())
    }

    /// Evaluate a shared Lua module (`source` must return a value, typically a
    /// table) and bind it to the global `name`, so screens can use it like a
    /// library — e.g. a reusable widgets toolkit (`Widgets.slider(...)`). The
    /// module is plain Lua and stays inside the VM, so it doesn't widen the
    /// data boundary. `chunk_name` labels it in errors. Call once at load.
    pub fn set_lua_module(
        &self,
        name: &str,
        source: &str,
        chunk_name: &str,
    ) -> Result<(), ScriptError> {
        let module: mlua::Value = self.lua.load(source).set_name(chunk_name).eval()?;
        self.lua.globals().set(name, module)?;
        Ok(())
    }

    /// Run the script's per-frame `update`, calling the Lua
    /// `update(mouse_x, mouse_y, clicked, sw, sh, down)`: the cursor position,
    /// the left-click *edge* (`clicked`), the screen size, and the *held* button
    /// state (`down`, for dragging — sliders etc.). `down` is last so older
    /// scripts that take only `(mx, my, clicked, sw, sh)` keep working. Returns
    /// the script's named results ([`ValueMap`]) — toggles, momentary actions
    /// (e.g. `start = true` only on the click frame), and widget values (a
    /// slider's number). Engine data the script reads comes from the `Model`
    /// global ([`Self::set_model`]).
    pub fn update(
        &self,
        input: &InputState,
        screen_w: f32,
        screen_h: f32,
    ) -> Result<ValueMap, ScriptError> {
        let update: Function = self.module.get("update")?;
        let mouse = input.mouse_position;
        let states: Table = update.call((
            mouse.x,
            mouse.y,
            input.mouse_left_pressed,
            screen_w,
            screen_h,
            input.mouse_left,
        ))?;

        let mut map = HashMap::new();
        for pair in states.pairs::<String, mlua::Value>() {
            let (name, value) = pair?;
            let value = match value {
                mlua::Value::Boolean(b) => Value::Bool(b),
                mlua::Value::Integer(i) => Value::Number(i as f64),
                mlua::Value::Number(n) => Value::Number(n),
                mlua::Value::String(s) => Value::Text(s.to_str()?.to_string()),
                other => {
                    tracing::warn!(
                        "script returned unsupported result type for '{name}': {}",
                        other.type_name()
                    );
                    continue;
                }
            };
            map.insert(name, value);
        }
        Ok(ValueMap { map })
    }

    /// Run the script's per-frame `draw` (given the screen size) and collect
    /// its HUD commands. Unrecognised command kinds are skipped with a warning
    /// rather than failing the frame.
    pub fn draw(&self, screen_w: f32, screen_h: f32) -> Result<Vec<HudCommand>, ScriptError> {
        let draw: Function = self.module.get("draw")?;
        let list: Table = draw.call((screen_w, screen_h))?;

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
                    layer: read_layer(&cmd)?,
                }),
                "sprite" => commands.push(HudCommand::Sprite {
                    tex: cmd.get("tex")?,
                    x: cmd.get("x")?,
                    y: cmd.get("y")?,
                    w: cmd.get("w")?,
                    h: cmd.get("h")?,
                    color: read_color(&cmd)?,
                    layer: read_layer(&cmd)?,
                }),
                "text" => commands.push(HudCommand::Text {
                    x: cmd.get("x")?,
                    y: cmd.get("y")?,
                    text: cmd.get("text")?,
                    size: cmd.get::<Option<f32>>("size")?.unwrap_or(16.0),
                    color: read_color(&cmd)?,
                    layer: read_layer(&cmd)?,
                    align: read_align(&cmd)?,
                    font: read_font(&cmd)?,
                }),
                "panel" => {
                    // `color` is stop 0; `color2` (r2..a2) defaults to it (solid);
                    // `border_color` (br..ba) defaults to transparent (no border).
                    let color = read_color(&cmd)?;
                    commands.push(HudCommand::Panel {
                        x: cmd.get("x")?,
                        y: cmd.get("y")?,
                        w: cmd.get("w")?,
                        h: cmd.get("h")?,
                        color,
                        color2: read_color_keys(&cmd, ("r2", "g2", "b2", "a2"), color)?,
                        grad: cmd.get::<Option<f32>>("grad")?.unwrap_or(0.0),
                        radius: cmd.get::<Option<f32>>("radius")?.unwrap_or(0.0),
                        border: cmd.get::<Option<f32>>("border")?.unwrap_or(0.0),
                        border_color: read_color_keys(&cmd, ("br", "bg", "bb", "ba"), [0.0; 4])?,
                        feather: cmd.get::<Option<f32>>("feather")?.unwrap_or(0.0),
                        layer: read_layer(&cmd)?,
                    });
                }
                other => tracing::warn!("hud script emitted unknown command kind '{other}'"),
            }
        }
        Ok(commands)
    }

    /// Build the screen's **component tree** by calling the module's optional
    /// `tree()` builder and parsing the returned node table into [`UiNode`]s —
    /// the inbound half of the component-UI boundary (the counterpart to
    /// [`draw`](Self::draw) on the legacy immediate path). `Ok(None)` when the
    /// module exposes no `tree` (a legacy `update`/`draw` screen).
    ///
    /// The engine caches the result and re-calls this only when the screen
    /// signals a structural change, so an unchanged tree costs nothing per frame
    /// — the walker redraws the cached tree with fresh `Model` bindings instead.
    pub fn ui_tree(&self) -> Result<Option<UiNode>, ScriptError> {
        let Some(tree_fn) = self.module.get::<Option<Function>>("tree")? else {
            return Ok(None);
        };
        let root: Table = tree_fn.call(())?;
        Ok(Some(parse_ui_node(&root)?))
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

/// Read an RGBA colour from four named channel keys, each channel defaulting to
/// the matching channel of `default` when omitted. Used for a
/// [`HudCommand::Panel`]'s second gradient stop (`r2..a2`, defaulting to the
/// first stop → a solid fill) and its border colour (`br..ba`, defaulting to
/// transparent → no border).
fn read_color_keys(
    cmd: &Table,
    keys: (&str, &str, &str, &str),
    default: [f32; 4],
) -> mlua::Result<[f32; 4]> {
    Ok([
        cmd.get::<Option<f32>>(keys.0)?.unwrap_or(default[0]),
        cmd.get::<Option<f32>>(keys.1)?.unwrap_or(default[1]),
        cmd.get::<Option<f32>>(keys.2)?.unwrap_or(default[2]),
        cmd.get::<Option<f32>>(keys.3)?.unwrap_or(default[3]),
    ])
}

/// Read a command's `layer` (painter's-order key), defaulting to `0.0`.
fn read_layer(cmd: &Table) -> mlua::Result<f32> {
    Ok(cmd.get::<Option<f32>>("layer")?.unwrap_or(0.0))
}

/// Read a text command's `align` (`"center"` → [`TextAlign::Center`], `"right"` →
/// [`TextAlign::Right`]; anything else, including omitted, → [`TextAlign::Left`]).
fn read_align(cmd: &Table) -> mlua::Result<TextAlign> {
    Ok(match cmd.get::<Option<String>>("align")?.as_deref() {
        Some("center") => TextAlign::Center,
        Some("right") => TextAlign::Right,
        _ => TextAlign::Left,
    })
}

/// Read a text command's `font` role (`"display"` → [`FontRole::Display`],
/// `"label"` → [`FontRole::Label`]; anything else, including omitted, →
/// [`FontRole::Body`], so scripts that never set a face get body prose).
fn read_font(cmd: &Table) -> mlua::Result<FontRole> {
    Ok(match cmd.get::<Option<String>>("font")?.as_deref() {
        Some("display") => FontRole::Display,
        Some("label") => FontRole::Label,
        _ => FontRole::Body,
    })
}

/// Node table keys the parser reads structurally — everything else in a node
/// table becomes a scalar entry in [`UiNode::props`]. Kept in one place so the
/// props sweep and the structural reads cannot disagree about what is a prop.
const UI_STRUCTURAL_KEYS: &[&str] = &[
    "id", "component", "type", "children", "anchor", "offset", "size", "grow", "width", "height",
    "gap", "pad", "bind", "action", "visible", "visible_bind", "enabled", "enabled_bind",
];

/// Map an anchor name to [`UiAnchor`]; unknown / absent → `None` (the node flows).
fn parse_anchor(name: Option<String>) -> Option<UiAnchor> {
    Some(match name?.as_str() {
        "top_left" => UiAnchor::TopLeft,
        "top" => UiAnchor::Top,
        "top_right" => UiAnchor::TopRight,
        "left" => UiAnchor::Left,
        "center" => UiAnchor::Center,
        "right" => UiAnchor::Right,
        "bottom_left" => UiAnchor::BottomLeft,
        "bottom" => UiAnchor::Bottom,
        "bottom_right" => UiAnchor::BottomRight,
        _ => return None,
    })
}

/// Parse one Lua node table (and, recursively, its `children`) into a [`UiNode`].
/// Known keys are read structurally; every remaining string-keyed **scalar** goes
/// into [`UiNode::props`] (tables — `children` / `offset` — are handled here and
/// never leak into props). `component` (or its alias `type`) is required.
fn parse_ui_node(t: &Table) -> mlua::Result<UiNode> {
    let component = t
        .get::<Option<String>>("component")?
        .or(t.get::<Option<String>>("type")?)
        .ok_or_else(|| mlua::Error::RuntimeError("ui node missing `component`".to_string()))?;

    let children = match t.get::<Option<Table>>("children")? {
        Some(list) => {
            let mut out = Vec::new();
            for item in list.sequence_values::<Table>() {
                out.push(parse_ui_node(&item?)?);
            }
            out
        }
        None => Vec::new(),
    };

    let offset = match t.get::<Option<Table>>("offset")? {
        Some(o) => [
            o.get::<Option<f32>>(1)?.unwrap_or(0.0),
            o.get::<Option<f32>>(2)?.unwrap_or(0.0),
        ],
        None => [0.0, 0.0],
    };

    // Everything not read structurally, if it is a scalar, is a prop.
    let mut props = HashMap::new();
    for pair in t.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        let mlua::Value::String(key) = key else { continue };
        let key = key.to_str()?.to_string();
        if UI_STRUCTURAL_KEYS.contains(&key.as_str()) {
            continue;
        }
        let value = match value {
            mlua::Value::Boolean(b) => Value::Bool(b),
            mlua::Value::Integer(i) => Value::Number(i as f64),
            mlua::Value::Number(n) => Value::Number(n),
            mlua::Value::String(s) => Value::Text(s.to_str()?.to_string()),
            // Tables/functions are not scalar props; the structural reads above
            // already took `children`/`offset`.
            _ => continue,
        };
        props.insert(key, value);
    }

    Ok(UiNode {
        id: t.get::<Option<String>>("id")?.unwrap_or_default(),
        component,
        children,
        anchor: parse_anchor(t.get::<Option<String>>("anchor")?),
        offset,
        size: t.get::<Option<f32>>("size")?,
        grow: t.get::<Option<f32>>("grow")?,
        width: t.get::<Option<f32>>("width")?,
        height: t.get::<Option<f32>>("height")?,
        gap: t.get::<Option<f32>>("gap")?.unwrap_or(0.0),
        pad: t.get::<Option<f32>>("pad")?.unwrap_or(0.0),
        props,
        bind: t.get::<Option<String>>("bind")?,
        action: t.get::<Option<String>>("action")?,
        visible_bind: t
            .get::<Option<String>>("visible_bind")?
            .or(t.get::<Option<String>>("visible")?),
        enabled_bind: t
            .get::<Option<String>>("enabled_bind")?
            .or(t.get::<Option<String>>("enabled")?),
    })
}

/// The module contract, checked at load so a malformed screen fails fast rather
/// than mid-frame: a screen must expose the immediate-mode pair (`update` +
/// `draw`, the [`HudCommand`] path) **or** a `tree` builder (the [`UiNode`]
/// component path).
fn check_contract(module: &Table) -> Result<(), ScriptError> {
    let has_update = module.get::<Option<Function>>("update")?.is_some();
    let has_draw = module.get::<Option<Function>>("draw")?.is_some();
    let has_tree = module.get::<Option<Function>>("tree")?.is_some();
    if (has_update && has_draw) || has_tree {
        Ok(())
    } else {
        Err(ScriptError::Lua(mlua::Error::RuntimeError(
            "script module must expose `update` + `draw` or `tree`".to_string(),
        )))
    }
}

/// Install a minimal Luau-safe `require`: compile each `(name, source)` module to
/// a loader function held in a private preload table, and expose a `require`
/// global that runs a module **once** and caches its result. Lets the UI be split
/// into one file per component that `require` a shared core + each other. See
/// [`ScriptHost::new_with_modules`].
fn install_require(lua: &Lua, modules: &[(&str, &str)]) -> Result<(), ScriptError> {
    let preload = lua.create_table()?;
    for (name, source) in modules {
        let loader = lua.load(*source).set_name(*name).into_function()?;
        preload.set(*name, loader)?;
    }
    // `require(name)` closes over the preload + a load cache (passed as the chunk's
    // varargs → captured as upvalues), so it needs none of the stock package lib.
    const REQUIRE_SRC: &str = r#"
        local preload, loaded = ...
        return function(name)
            local cached = loaded[name]
            if cached ~= nil then return cached end
            local loader = preload[name]
            if loader == nil then error("module '" .. tostring(name) .. "' is not registered", 2) end
            local result = loader(name)
            if result == nil then result = true end
            loaded[name] = result
            return result
        end
    "#;
    let loaded = lua.create_table()?;
    let require_fn: Function = lua
        .load(REQUIRE_SRC)
        .set_name("=require")
        .call((preload, loaded))?;
    lua.globals().set("require", require_fn)?;
    Ok(())
}

/// Recursively convert a JSON value into a Lua value (used by
/// [`ScriptHost::set_global_json`]). Arrays become 1-indexed tables so scripts
/// read `color[1]`; objects become string-keyed tables.
fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<mlua::Value> {
    use serde_json::Value as J;
    Ok(match value {
        J::Null => mlua::Value::Nil,
        J::Bool(b) => mlua::Value::Boolean(*b),
        J::Number(n) => mlua::Value::Number(n.as_f64().unwrap_or(0.0)),
        J::String(s) => mlua::Value::String(lua.create_string(s)?),
        J::Array(items) => {
            let table = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                table.set(i + 1, json_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
        J::Object(fields) => {
            let table = lua.create_table()?;
            for (key, val) in fields {
                table.set(key.as_str(), json_to_lua(lua, val)?)?;
            }
            mlua::Value::Table(table)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two peer modules + a main script that requires both — the per-file
    // component seam. `component` requires the shared `core`; the main script
    // requires both. Proves require() resolves peers and returns a singleton
    // (core loaded once, shared identity across requirers).
    const CORE_MODULE: &str = r#"
        local core = {}
        core.loads = (core.loads or 0) + 1
        function core.tag() return "core" end
        return core
    "#;
    const COMPONENT_MODULE: &str = r#"
        local core = require("core")
        return { made_by = core.tag(), core = core }
    "#;
    const MODULAR_MAIN: &str = r#"
        local core = require("core")
        local comp = require("component")
        local M = {}
        function M.update() return { same_core = core == comp.core } end
        function M.draw() return { { kind = "text", x = 0, y = 0, text = comp.made_by } } end
        return M
    "#;

    #[test]
    fn require_composes_per_file_modules() {
        let host = ScriptHost::new_with_modules(
            MODULAR_MAIN,
            "modular-main",
            &[("core", CORE_MODULE), ("component", COMPONENT_MODULE)],
        )
        .expect("modular host builds and require() resolves peers");
        // The component was built from the shared core, and the main script and
        // the component see the SAME core table (singleton, not two evaluations).
        let input = InputState::new();
        let out = host.update(&input, 0.0, 0.0).expect("update runs");
        assert!(out.is_on("same_core"), "require returns a cached singleton");
        let cmds = host.draw(0.0, 0.0).expect("draw runs");
        assert_eq!(
            cmds,
            vec![HudCommand::Text {
                x: 0.0,
                y: 0.0,
                text: "core".to_string(),
                size: 16.0,
                color: [1.0, 1.0, 1.0, 1.0],
                layer: 0.0,
                align: TextAlign::Left,
                font: FontRole::Body,
            }],
            "component drew text tagged by the required core"
        );
    }

    const SCRIPT: &str = r#"
        local M = {}
        local checked = false
        function M.update(mx, my, clicked, sw, sh)
            if clicked and mx >= 0 and mx <= 10 and my >= 0 and my <= 10 then
                checked = not checked
            end
            return { box = checked }
        end
        function M.draw(sw, sh)
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
        let toggles = host
            .update(&input_at(5.0, 5.0, false), 800.0, 600.0)
            .unwrap();
        assert!(!toggles.is_on("box"));

        // Click inside the box: on.
        let toggles = host
            .update(&input_at(5.0, 5.0, true), 800.0, 600.0)
            .unwrap();
        assert!(toggles.is_on("box"));

        // Click outside leaves it on (no flip).
        let toggles = host
            .update(&input_at(50.0, 50.0, true), 800.0, 600.0)
            .unwrap();
        assert!(toggles.is_on("box"));

        // Click inside again flips it back off.
        let toggles = host
            .update(&input_at(5.0, 5.0, true), 800.0, 600.0)
            .unwrap();
        assert!(!toggles.is_on("box"));
    }

    #[test]
    fn unknown_toggle_is_off() {
        let host = ScriptHost::new(SCRIPT, "test").unwrap();
        let toggles = host
            .update(&input_at(0.0, 0.0, false), 800.0, 600.0)
            .unwrap();
        assert!(!toggles.is_on("nope"));
    }

    #[test]
    fn draw_returns_rect_and_text() {
        let host = ScriptHost::new(SCRIPT, "test").unwrap();
        let cmds = host.draw(800.0, 600.0).unwrap();
        assert_eq!(
            cmds,
            vec![
                HudCommand::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    layer: 0.0,
                },
                HudCommand::Text {
                    x: 12.0,
                    y: 0.0,
                    text: "hi".to_string(),
                    size: 14.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    layer: 0.0,
                    align: TextAlign::Left,
                    font: FontRole::Body,
                },
            ]
        );
    }

    const SCREEN_SCRIPT: &str = r#"
        local M = {}
        function M.update(mx, my, clicked, sw, sh) return {} end
        function M.draw(sw, sh)
            return {
                { kind = "sprite", tex = Textures.panel, x = 4, y = 5, w = 6, h = 7,
                  layer = 2, a = 0.5 },
                { kind = "text", x = sw * 0.5, y = sh - 10, text = "FLICKER", size = 30,
                  align = "center", layer = 3 },
            }
        end
        return M
    "#;

    #[test]
    fn sprite_layer_and_align_parse() {
        let host = ScriptHost::new(SCREEN_SCRIPT, "screen").unwrap();
        host.set_texture_ids(&[("panel", 1), ("button", 2), ("white", 0)])
            .unwrap();
        let cmds = host.draw(800.0, 600.0).unwrap();
        assert_eq!(
            cmds,
            vec![
                HudCommand::Sprite {
                    tex: 1,
                    x: 4.0,
                    y: 5.0,
                    w: 6.0,
                    h: 7.0,
                    color: [1.0, 1.0, 1.0, 0.5],
                    layer: 2.0,
                },
                HudCommand::Text {
                    x: 400.0,
                    y: 590.0,
                    text: "FLICKER".to_string(),
                    size: 30.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    layer: 3.0,
                    align: TextAlign::Center,
                    font: FontRole::Body,
                },
            ]
        );
    }

    const PANEL_SCRIPT: &str = r#"
        local M = {}
        function M.update(mx, my, clicked, sw, sh) return {} end
        function M.draw(sw, sh)
            return {
                { kind = "panel", x = 10, y = 20, w = 100, h = 40,
                  r = 0.1, g = 0.2, b = 0.3, a = 1.0,
                  r2 = 0.4, g2 = 0.5, b2 = 0.6,
                  grad = 1, radius = 5, border = 1,
                  br = 0.7, bg = 0.7, bb = 0.8, ba = 1.0, layer = 2 },
                { kind = "panel", x = 0, y = 0, w = 8, h = 8, r = 1, g = 1, b = 1 },
            }
        end
        return M
    "#;

    #[test]
    fn panel_command_parses() {
        let host = ScriptHost::new(PANEL_SCRIPT, "panel").unwrap();
        let cmds = host.draw(800.0, 600.0).unwrap();
        assert_eq!(
            cmds,
            vec![
                HudCommand::Panel {
                    x: 10.0,
                    y: 20.0,
                    w: 100.0,
                    h: 40.0,
                    color: [0.1, 0.2, 0.3, 1.0],
                    color2: [0.4, 0.5, 0.6, 1.0], // a2 omitted → falls back to color.a
                    grad: 1.0,
                    radius: 5.0,
                    border: 1.0,
                    border_color: [0.7, 0.7, 0.8, 1.0],
                    feather: 0.0,
                    layer: 2.0,
                },
                HudCommand::Panel {
                    x: 0.0,
                    y: 0.0,
                    w: 8.0,
                    h: 8.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    color2: [1.0, 1.0, 1.0, 1.0], // no r2 → solid (== color)
                    grad: 0.0,
                    radius: 0.0,
                    border: 0.0,
                    border_color: [0.0, 0.0, 0.0, 0.0], // no br → transparent (no border)
                    feather: 0.0,
                    layer: 0.0,
                },
            ]
        );
    }

    // Echoes engine model values straight back as results, so a Rust assertion
    // validates the whole boundary round-trip: ValueMap → `Model` global → Lua
    // reads it → returns a result table → ValueMap, with types preserved.
    const ECHO_SCRIPT: &str = r#"
        local M = {}
        function M.update(mx, my, clicked, sw, sh)
            return { fps = Model.fps, label = Model.label, ready = Model.ready,
                     doubled = Model.fps * 2 }
        end
        function M.draw(sw, sh) return {} end
        return M
    "#;

    #[test]
    fn model_round_trip() {
        let host = ScriptHost::new(ECHO_SCRIPT, "echo").unwrap();
        let model = ValueMap::new()
            .with("fps", 60.0_f32)
            .with("label", "hello")
            .with("ready", true);
        host.set_model(&model).unwrap();

        let out = host
            .update(&input_at(0.0, 0.0, false), 800.0, 600.0)
            .unwrap();
        assert_eq!(out.number("fps"), Some(60.0)); // number survives the trip
        assert_eq!(out.text("label"), Some("hello")); // text survives
        assert!(out.is_on("ready")); // bool survives
        assert_eq!(out.number("doubled"), Some(120.0)); // Lua computed on the model
        assert_eq!(out.number("label"), None); // typed getters don't coerce
        assert!(!out.is_on("missing")); // absent → false
    }

    #[test]
    fn value_map_typed_accessors() {
        let m = ValueMap::new()
            .with("on", true)
            .with("n", 3.5_f64)
            .with("s", "x".to_string());
        assert!(m.is_on("on"));
        assert_eq!(m.number("n"), Some(3.5));
        assert_eq!(m.text("s"), Some("x"));
        assert_eq!(m.number("on"), None);
        assert!(!m.is_on("n"));
    }

    // Reads a nested JSON layout (objects, an array color, mixed types) back
    // out, validating set_global_json marshals the whole tree faithfully.
    const UI_SCRIPT: &str = r#"
        local M = {}
        function M.update(mx, my, clicked, sw, sh)
            return {
                w = UI.menu.panel.w,
                label = UI.menu.start.label,
                title_r = UI.menu.title.color[1],
                title_a = UI.menu.title.color[4],
            }
        end
        function M.draw(sw, sh) return {} end
        return M
    "#;

    #[test]
    fn set_global_json_marshals_nested_tree() {
        let host = ScriptHost::new(UI_SCRIPT, "ui").unwrap();
        let json = serde_json::json!({
            "menu": {
                "panel": { "w": 520, "h": 384 },
                "title": { "color": [0.83, 0.67, 0.39, 1.0] },
                "start": { "label": "START" },
            }
        });
        host.set_global_json("UI", &json).unwrap();

        let out = host
            .update(&input_at(0.0, 0.0, false), 800.0, 600.0)
            .unwrap();
        assert_eq!(out.number("w"), Some(520.0)); // nested object number
        assert_eq!(out.text("label"), Some("START")); // nested object string
        assert_eq!(out.number("title_r"), Some(0.83)); // array is 1-indexed
        assert_eq!(out.number("title_a"), Some(1.0)); // 4th array element
    }

    // A tree-only screen: no update/draw, just a `tree()` builder returning a
    // nested component tree (page → column → checkbox + button). Exercises the
    // whole UiNode parse: structural reads, the props sweep, bindings, anchor,
    // offset, and recursion.
    const UI_TREE_SCRIPT: &str = r#"
        local M = {}
        function M.tree()
          return {
            id = "root", component = "page", children = {
              { component = "column", anchor = "top_left", offset = { 16, 20 },
                gap = 6, pad = 4, children = {
                  { component = "checkbox", id = "mesh", label = "Mesh", bind = "show_mesh" },
                  { component = "button", id = "attack", label = "ATTACK", style = "primary",
                    action = "attack", visible_bind = "animate", size = 24 },
                } },
            },
          }
        end
        return M
    "#;

    #[test]
    fn ui_tree_parses_nested_component_tree() {
        let host = ScriptHost::new(UI_TREE_SCRIPT, "ui-tree").expect("tree-only module loads");
        let root = host.ui_tree().expect("ui_tree runs").expect("module exposes a tree");

        assert_eq!(root.component, "page");
        assert_eq!(root.id, "root");
        assert_eq!(root.children.len(), 1);

        let col = &root.children[0];
        assert_eq!(col.component, "column");
        assert_eq!(col.anchor, Some(UiAnchor::TopLeft));
        assert_eq!(col.offset, [16.0, 20.0]);
        assert_eq!(col.gap, 6.0);
        assert_eq!(col.pad, 4.0);
        assert_eq!(col.children.len(), 2);

        let cb = &col.children[0];
        assert_eq!(cb.component, "checkbox");
        assert_eq!(cb.bind.as_deref(), Some("show_mesh"));
        assert_eq!(cb.props.get("label"), Some(&Value::Text("Mesh".to_string())));

        let btn = &col.children[1];
        assert_eq!(btn.component, "button");
        assert_eq!(btn.action.as_deref(), Some("attack"));
        assert_eq!(btn.visible_bind.as_deref(), Some("animate"));
        assert_eq!(btn.size, Some(24.0));
        assert_eq!(btn.props.get("style"), Some(&Value::Text("primary".to_string())));
        // Structural keys never leak into props.
        assert!(!btn.props.contains_key("action"));
        assert!(!btn.props.contains_key("component"));
        assert!(!btn.props.contains_key("visible_bind"));
    }

    #[test]
    fn ui_tree_absent_on_legacy_module() {
        // A legacy update/draw screen exposes no `tree` → ui_tree() is None.
        let host = ScriptHost::new(SCRIPT, "legacy").unwrap();
        assert!(host.ui_tree().unwrap().is_none());
    }

    #[test]
    fn tree_only_module_satisfies_contract() {
        // update+draw absent but tree present → still a valid module.
        let src = r#"local M = {} function M.tree() return { component = "page" } end return M"#;
        assert!(ScriptHost::new(src, "tree-only").is_ok());
    }
}
