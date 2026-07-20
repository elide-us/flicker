//! Immediate-mode input settings panel with vertical tabs.
//!
//! [`InputSettingsPanel`] renders a full settings menu over the engine's
//! existing 2D draw primitives via the [`GuiRenderer`] trait. It covers:
//!
//! - **Key Bindings** — rebindable keyboard/mouse actions
//! - **Gamepad** — rebindable gamepad button/axis actions
//! - **Mouse** — sensitivity and invert toggles
//! - **Gamepad Settings** — stick sensitivity, deadzone, shape, invert
//! - **Movement** — move speed
//!
//! Changes are buffered locally — call [`InputSettingsPanel::into_apply`]
//! to consume the panel and get the final configs.
//!
//! # Usage
//!
//! ```ignore
//! let mut panel = InputSettingsPanel::new(&input_map, &controls, &gamepad_config);
//!
//! // each frame:
//! if settings_open {
//!     panel.update(&input_state, cursor_pos, cursor_clicked);
//!     panel.draw(&mut renderer_wrapper);
//!     if panel.should_close() {
//!         let (map, ctrl, gp_cfg) = panel.into_apply();
//!         // persist the new settings
//!     }
//! }
//! ```

use std::collections::{HashMap, HashSet};

use glam::Vec2;

use super::bindings::{
    AbstractControls, Action, AxisDirection, InputBinding, InputMap,
};
use super::{DeadzoneShape, GamepadAxis, GamepadButton, GamepadConfig, Key, MouseButton, InputState};

// ───────────────────────────────────────────────────────────────────
// GuiRenderer trait
// ───────────────────────────────────────────────────────────────────

/// Abstraction over the engine's 2D draw primitives. Implement this
/// on a wrapper around [`flicker_render::Renderer`] (or any other
/// backend) to draw the settings panel.
pub trait GuiRenderer {
    /// Draw a filled rectangle at `pos` (top-left) with `size`.
    fn draw_rect(&mut self, pos: Vec2, size: Vec2, color: [f32; 4]);
    /// Draw a filled triangle from three vertices.
    fn draw_triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: [f32; 4]);
    /// Draw `text` at `pos` with given `size` (pixels) and `color`.
    fn draw_text(&mut self, text: &str, pos: Vec2, size: f32, color: [f32; 4]);
    /// Measure `text` at `size` pixels, returning (width, height).
    fn measure_text(&mut self, text: &str, size: f32) -> Vec2;
    /// Current screen dimensions in pixels.
    fn screen_size(&self) -> Vec2;
}

// ───────────────────────────────────────────────────────────────────
// Gothic theme colors
// ───────────────────────────────────────────────────────────────────

// Prism palette. This Rust settings GUI is superseded by the shell's Lua
// settings scene, but kept on-canon so no gold survives. `COL_GOLD*` names now
// hold Prism bronze; interactive tints are sapphire.
const COL_BG: [f32; 4]         = [0.055, 0.063, 0.086, 0.94];
const COL_PANEL: [f32; 4]      = [0.149, 0.169, 0.208, 1.0];
const COL_PANEL_ALT: [f32; 4]  = [0.110, 0.125, 0.161, 1.0];
const COL_TITLE: [f32; 4]      = [0.906, 0.882, 0.824, 1.0];
const COL_LABEL: [f32; 4]      = [0.871, 0.847, 0.788, 1.0];
const COL_LABEL_DIM: [f32; 4]  = [0.561, 0.541, 0.490, 1.0];
const COL_GOLD: [f32; 4]       = [0.722, 0.592, 0.353, 1.0];
const COL_GOLD_LINE: [f32; 4]  = [0.431, 0.353, 0.204, 0.6];
const COL_HOVER: [f32; 4]      = [0.141, 0.247, 0.471, 0.15];
const COL_ACTIVE: [f32; 4]     = [0.141, 0.247, 0.471, 0.3];
const COL_SLIDER_TRACK: [f32; 4]  = [0.055, 0.063, 0.086, 1.0];
const COL_SLIDER_FILL: [f32; 4]   = [0.141, 0.247, 0.471, 0.8];
const COL_SLIDER_HANDLE: [f32; 4] = [0.227, 0.353, 0.627, 1.0];
const COL_CHECKBOX: [f32; 4]      = [0.078, 0.090, 0.122, 1.0];
const COL_CHECKBOX_CHECK: [f32; 4]= [0.227, 0.353, 0.627, 1.0];
const COL_REBIND: [f32; 4]    = [0.141, 0.247, 0.471, 0.30];
const COL_DIVIDER: [f32; 4]   = [0.431, 0.353, 0.204, 0.35];

// ───────────────────────────────────────────────────────────────────
// Layout constants
// ───────────────────────────────────────────────────────────────────

const TAB_WIDTH: f32      = 160.0;
const PANEL_MARGIN: f32   = 40.0;
const ROW_HEIGHT: f32     = 32.0;
const LABEL_SIZE: f32     = 16.0;
const TITLE_SIZE: f32     = 22.0;
const TAB_LABEL_SIZE: f32 = 15.0;
const HEADER_SIZE: f32    = 18.0;
const SLIDER_HEIGHT: f32  = 8.0;
const SLIDER_HANDLE_W: f32 = 14.0;
const CHECKBOX_SIZE: f32  = 20.0;
const CONTENT_PAD: f32    = 24.0;

// ───────────────────────────────────────────────────────────────────
// Tabs
// ───────────────────────────────────────────────────────────────────

const ALL_TABS: [Tab; 5] = [
    Tab::KeyBindings,
    Tab::Gamepad,
    Tab::Mouse,
    Tab::GamepadSettings,
    Tab::Movement,
];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Tab {
    KeyBindings,
    Gamepad,
    Mouse,
    GamepadSettings,
    Movement,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::KeyBindings => "Key Bindings",
            Self::Gamepad => "Gamepad",
            Self::Mouse => "Mouse",
            Self::GamepadSettings => "Gamepad Settings",
            Self::Movement => "Movement",
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Action categories
// ───────────────────────────────────────────────────────────────────

const KB_ACTIONS: &[Action] = &[
    Action::MoveForward,
    Action::MoveBackward,
    Action::StrafeLeft,
    Action::StrafeRight,
    Action::MoveUp,
    Action::MoveDown,
    Action::Jump,
    Action::Sprint,
    Action::Crouch,
    Action::Interact,
    Action::Reload,
    Action::PrimaryAction,
    Action::SecondaryAction,
    Action::Confirm,
    Action::Cancel,
    Action::Menu,
    Action::Inventory,
    Action::Map,
    Action::Quit,
];

const GP_ACTIONS: &[Action] = &[
    Action::MoveForward,
    Action::MoveBackward,
    Action::StrafeLeft,
    Action::StrafeRight,
    Action::LookUp,
    Action::LookDown,
    Action::LookLeft,
    Action::LookRight,
    Action::Jump,
    Action::Sprint,
    Action::Crouch,
    Action::Interact,
    Action::Reload,
    Action::PrimaryAction,
    Action::SecondaryAction,
    Action::Confirm,
    Action::Cancel,
    Action::Menu,
    Action::Inventory,
    Action::Map,
    Action::Quit,
];

// ───────────────────────────────────────────────────────────────────
// Rebind state
// ───────────────────────────────────────────────────────────────────

struct RebindState {
    action: Action,
    slot: usize,
    for_gamepad: bool,
}

// ───────────────────────────────────────────────────────────────────
// Slider drag state
// ───────────────────────────────────────────────────────────────────

/// Encodes which slider is being dragged. The high 8 bits are the tab
/// ordinal, the low 56 bits are a per-tab slider index.
struct SliderDrag {
    id: u64,
    track_x: f32,
    track_w: f32,
    min: f32,
    max: f32,
}

fn slider_id(tab: Tab, index: u8) -> u64 {
    let tab_ord = match tab {
        Tab::KeyBindings => 0u64,
        Tab::Gamepad => 1,
        Tab::Mouse => 2,
        Tab::GamepadSettings => 3,
        Tab::Movement => 4,
    };
    (tab_ord << 56) | index as u64
}

// ───────────────────────────────────────────────────────────────────
// InputSettingsPanel
// ───────────────────────────────────────────────────────────────────

/// The main input settings panel.
///
/// Holds local copies of [`InputMap`], [`AbstractControls`], and
/// [`GamepadConfig`] so changes are buffered until the panel is closed.
pub struct InputSettingsPanel {
    active_tab: Tab,
    visible: bool,
    // Buffered config (local edits)
    input_map: InputMap,
    controls: AbstractControls,
    gamepad_config: GamepadConfig,
    // Interaction state
    rebind: Option<RebindState>,
    slider_drag: Option<SliderDrag>,
    close_requested: bool,
    // Edge-detection bookkeeping
    prev_keys: HashSet<Key>,
    prev_mouse_left: bool,
    prev_mouse_right: bool,
    prev_mouse_middle: bool,
    prev_mouse_back: bool,
    prev_mouse_forward: bool,
    prev_gamepad_buttons: HashSet<GamepadButton>,
    prev_gamepad_axes: HashMap<GamepadAxis, f32>,
    // Cursor state (set via update)
    cursor_pos: Vec2,
    cursor_clicked: bool,
    // Layout cache (set via draw, used by click handlers)
    screen_size: Vec2,
    deadzone_shape_rects: Vec<(f32, f32, f32)>,
}

impl InputSettingsPanel {
    /// Create a new panel seeded from the current settings.
    pub fn new(
        input_map: &InputMap,
        controls: &AbstractControls,
        gamepad_config: &GamepadConfig,
    ) -> Self {
        Self {
            active_tab: Tab::KeyBindings,
            visible: false,
            input_map: input_map.clone(),
            controls: *controls,
            gamepad_config: gamepad_config.clone(),
            rebind: None,
            slider_drag: None,
            close_requested: false,
            prev_keys: HashSet::new(),
            prev_mouse_left: false,
            prev_mouse_right: false,
            prev_mouse_middle: false,
            prev_mouse_back: false,
            prev_mouse_forward: false,
            prev_gamepad_buttons: HashSet::new(),
            prev_gamepad_axes: HashMap::new(),
            cursor_pos: Vec2::ZERO,
            cursor_clicked: false,
            screen_size: Vec2::ZERO,
            deadzone_shape_rects: Vec::new(),
        }
    }

    /// Toggle visibility. Resets rebind state when hiding.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if !self.visible {
            self.rebind = None;
        }
    }

    /// Whether the panel is currently visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Whether the user requested to close the panel (Escape).
    pub fn should_close(&self) -> bool {
        self.close_requested
    }

    /// Consume the panel and return the final configs.
    pub fn into_apply(self) -> (InputMap, AbstractControls, GamepadConfig) {
        (self.input_map, self.controls, self.gamepad_config)
    }

    /// Extract the final configs without consuming. Panel stays alive
    /// (with defaults reset) after this call.
    pub fn take_apply(&mut self) -> (InputMap, AbstractControls, GamepadConfig) {
        let map = std::mem::replace(&mut self.input_map, InputMap::empty());
        let controls = std::mem::replace(&mut self.controls, AbstractControls::default());
        let gp_cfg = std::mem::replace(&mut self.gamepad_config, GamepadConfig::default());
        self.close_requested = false;
        self.visible = false;
        (map, controls, gp_cfg)
    }

    /// Buffered input map (read-only access).
    pub fn input_map(&self) -> &InputMap {
        &self.input_map
    }

    /// Buffered abstract controls (read-only access).
    pub fn controls(&self) -> &AbstractControls {
        &self.controls
    }

    /// Buffered gamepad config (read-only access).
    pub fn gamepad_config(&self) -> &GamepadConfig {
        &self.gamepad_config
    }

    // ── Update ──

    /// Process a frame of interaction. Call once per frame before `draw`.
    ///
    /// - `input` — the engine's polled input snapshot.
    /// - `cursor_pos` — mouse position in pixels (top-left origin).
    /// - `cursor_clicked` — true on the frame the primary mouse button
    ///   transitions up→down (edge).
    pub fn update(&mut self, input: &InputState, cursor_pos: Vec2, cursor_clicked: bool) {
        if !self.visible {
            return;
        }
        self.cursor_pos = cursor_pos;
        self.cursor_clicked = cursor_clicked;

        // ── Escape: cancel rebind or close panel ──
        let escape_just_pressed =
            input.key_down(Key::Escape) && !self.prev_keys.contains(&Key::Escape);
        if escape_just_pressed {
            if self.rebind.is_some() {
                self.rebind = None;
            } else {
                self.close_requested = true;
            }
            self.update_prev(input);
            return;
        }

        // ── Rebind capture ──
        if let Some(ref rb) = self.rebind {
            let captured = Self::capture_input(
                input,
                &self.prev_keys,
                &self.prev_mouse_left,
                &self.prev_mouse_right,
                &self.prev_mouse_middle,
                &self.prev_mouse_back,
                &self.prev_mouse_forward,
                &self.prev_gamepad_buttons,
                &self.prev_gamepad_axes,
                rb.for_gamepad,
            );
            if let Some(binding) = captured {
                let action = rb.action;
                let slot = rb.slot;
                // Conflict detection: if this input is already bound
                // to another action, unbind it there first
                if let Some(conflict_action) = self.input_map.action_for(binding) {
                    if conflict_action != action {
                        self.input_map.unbind(conflict_action, binding);
                    }
                }
                // Remove old binding at this slot if it exists
                let old_bindings: Vec<InputBinding> =
                    self.input_map.bindings_for(action).to_vec();
                if slot < old_bindings.len() {
                    self.input_map.unbind(action, old_bindings[slot]);
                }
                self.input_map.bind(action, binding);
                self.rebind = None;
            }
            self.update_prev(input);
            return;
        }

        // ── Slider drag ──
        if let Some(ref drag) = self.slider_drag {
            if !input.mouse_left {
                self.slider_drag = None;
            } else {
                let t = ((cursor_pos.x - drag.track_x) / drag.track_w).clamp(0.0, 1.0);
                let new_val = drag.min + t * (drag.max - drag.min);
                let drag_id = drag.id;
                self.apply_slider_value(drag_id, new_val);
            }
            self.update_prev(input);
            return;
        }

        // ── Tab bar click ──
        if cursor_clicked {
            if let Some(tab) = self.hit_test_tabs(cursor_pos) {
                self.active_tab = tab;
            }
        }

        // ── Per-tab widget clicks ──
        if cursor_clicked {
            match self.active_tab {
                Tab::KeyBindings => self.click_binding_rows(cursor_pos, false),
                Tab::Gamepad => self.click_binding_rows(cursor_pos, true),
                Tab::Mouse => self.click_mouse_tab(cursor_pos),
                Tab::GamepadSettings => self.click_gamepad_settings_tab(cursor_pos),
                Tab::Movement => self.click_movement_tab(cursor_pos),
            }
        }

        self.update_prev(input);
    }

    fn apply_slider_value(&mut self, id: u64, value: f32) {
        match id {
            x if x == slider_id(Tab::Mouse, 0) => self.controls.mouse_sensitivity = value,
            x if x == slider_id(Tab::GamepadSettings, 0) => self.controls.stick_sensitivity = value,
            x if x == slider_id(Tab::GamepadSettings, 1) => self.gamepad_config.left_stick_deadzone = value,
            x if x == slider_id(Tab::GamepadSettings, 2) => self.gamepad_config.trigger_threshold = value,
            x if x == slider_id(Tab::GamepadSettings, 3) => self.gamepad_config.right_stick_deadzone = value,
            x if x == slider_id(Tab::Movement, 0) => self.controls.move_speed = value,
            _ => {}
        }
    }

    // ── Draw ──

    /// Draw the full settings panel. Call each frame when visible.
    pub fn draw(&mut self, r: &mut dyn GuiRenderer) {
        if !self.visible {
            return;
        }
        let screen = r.screen_size();
        self.screen_size = screen;
        let (panel_pos, panel_size) = Self::panel_geometry(screen);

        // Background dim
        r.draw_rect(Vec2::ZERO, screen, [0.0, 0.0, 0.0, 0.5]);

        // Main panel background
        r.draw_rect(panel_pos, panel_size, COL_BG);

        // Gold border lines
        let lw = 2.0;
        r.draw_rect(panel_pos, Vec2::new(panel_size.x, lw), COL_GOLD_LINE);
        r.draw_rect(
            Vec2::new(panel_pos.x, panel_pos.y + panel_size.y - lw),
            Vec2::new(panel_size.x, lw),
            COL_GOLD_LINE,
        );
        r.draw_rect(panel_pos, Vec2::new(lw, panel_size.y), COL_GOLD_LINE);
        r.draw_rect(
            Vec2::new(panel_pos.x + panel_size.x - lw, panel_pos.y),
            Vec2::new(lw, panel_size.y),
            COL_GOLD_LINE,
        );

        // Title
        let title = "Input Settings";
        let title_m = r.measure_text(title, TITLE_SIZE);
        r.draw_text(
            title,
            Vec2::new(panel_pos.x + TAB_WIDTH + CONTENT_PAD, panel_pos.y + 16.0),
            TITLE_SIZE,
            COL_TITLE,
        );

        // Gold divider under title
        let title_div_y = panel_pos.y + 16.0 + title_m.y + 8.0;
        r.draw_rect(
            Vec2::new(panel_pos.x + TAB_WIDTH + CONTENT_PAD, title_div_y),
            Vec2::new(panel_size.x - TAB_WIDTH - CONTENT_PAD * 2.0, 1.0),
            COL_GOLD_LINE,
        );

        // Tab bar
        self.draw_tabs(r, panel_pos, panel_size);

        // Vertical divider between tabs and content
        r.draw_rect(
            Vec2::new(panel_pos.x + TAB_WIDTH, panel_pos.y + 8.0),
            Vec2::new(1.0, panel_size.y - 16.0),
            COL_DIVIDER,
        );

        // Content area
        let content_x = panel_pos.x + TAB_WIDTH + CONTENT_PAD;
        let content_y = title_div_y + 12.0;
        let content_w = panel_size.x - TAB_WIDTH - CONTENT_PAD * 2.0;

        match self.active_tab {
            Tab::KeyBindings => self.draw_binding_rows(r, content_x, content_y, content_w, false),
            Tab::Gamepad => self.draw_binding_rows(r, content_x, content_y, content_w, true),
            Tab::Mouse => self.draw_mouse_tab(r, content_x, content_y, content_w),
            Tab::GamepadSettings => self.draw_gamepad_settings_tab(r, content_x, content_y, content_w),
            Tab::Movement => self.draw_movement_tab(r, content_x, content_y, content_w),
        }

        // Rebind overlay
        if self.rebind.is_some() {
            self.draw_rebind_overlay(r, screen);
        }
    }

    // ───────────────────────────────────────────────────────────────
    // Panel geometry
    // ───────────────────────────────────────────────────────────────

    /// Compute panel position and size from screen dimensions.
    fn panel_geometry(screen: Vec2) -> (Vec2, Vec2) {
        let pw = screen.x - PANEL_MARGIN * 2.0;
        let ph = screen.y - PANEL_MARGIN * 2.0;
        (Vec2::new(PANEL_MARGIN, PANEL_MARGIN), Vec2::new(pw, ph))
    }

    /// Compute tab height given the panel height.
    fn tab_height(panel_h: f32) -> f32 {
        (panel_h - 4.0) / ALL_TABS.len() as f32
    }

    // ───────────────────────────────────────────────────────────────
    // Tab bar
    // ───────────────────────────────────────────────────────────────

    fn hit_test_tabs(&self, cursor: Vec2) -> Option<Tab> {
        let screen = if self.screen_size.x > 0.0 && self.screen_size.y > 0.0 {
            self.screen_size
        } else {
            Vec2::new(1920.0, 1080.0)
        };
        let (_panel_pos, panel_size) = Self::panel_geometry(screen);
        let tab_h = Self::tab_height(panel_size.y);
        let x = PANEL_MARGIN;
        let y = PANEL_MARGIN + 2.0;

        for (i, tab) in ALL_TABS.iter().enumerate() {
            let ty = y + i as f32 * tab_h;
            if cursor.x >= x
                && cursor.x <= x + TAB_WIDTH
                && cursor.y >= ty
                && cursor.y <= ty + tab_h
            {
                return Some(*tab);
            }
        }
        None
    }

    fn draw_tabs(&self, r: &mut dyn GuiRenderer, panel_pos: Vec2, panel_size: Vec2) {
        let tab_h = Self::tab_height(panel_size.y);
        let x = panel_pos.x;
        let y = panel_pos.y + 2.0;

        for (i, tab) in ALL_TABS.iter().enumerate() {
            let ty = y + i as f32 * tab_h;
            let is_active = *tab == self.active_tab;
            let hovered = self.cursor_pos.x >= x
                && self.cursor_pos.x <= x + TAB_WIDTH
                && self.cursor_pos.y >= ty
                && self.cursor_pos.y <= ty + tab_h;

            let bg = if is_active {
                COL_ACTIVE
            } else if hovered {
                COL_HOVER
            } else {
                [0.0; 4]
            };
            r.draw_rect(Vec2::new(x, ty), Vec2::new(TAB_WIDTH, tab_h), bg);

            if is_active {
                r.draw_rect(Vec2::new(x, ty), Vec2::new(3.0, tab_h), COL_GOLD);
            }

            let label = tab.label();
            let label_m = r.measure_text(label, TAB_LABEL_SIZE);
            let col = if is_active { COL_TITLE } else { COL_LABEL };
            r.draw_text(
                label,
                Vec2::new(x + 16.0, ty + (tab_h - label_m.y) / 2.0),
                TAB_LABEL_SIZE,
                col,
            );
        }
    }

    // ───────────────────────────────────────────────────────────────
    // Binding rows (Key Bindings & Gamepad tabs)
    // ───────────────────────────────────────────────────────────────

    fn draw_binding_rows(
        &self,
        r: &mut dyn GuiRenderer,
        x: f32,
        y: f32,
        w: f32,
        for_gamepad: bool,
    ) {
        let actions = if for_gamepad { GP_ACTIONS } else { KB_ACTIONS };
        let header = if for_gamepad {
            "Action                           Gamepad Binding"
        } else {
            "Action                           Key / Mouse Binding"
        };
        r.draw_text(header, Vec2::new(x, y), HEADER_SIZE, COL_TITLE);
        r.draw_rect(
            Vec2::new(x, y + HEADER_SIZE + 4.0),
            Vec2::new(w, 1.0),
            COL_DIVIDER,
        );

        let mut cy = y + HEADER_SIZE + 12.0;
        let half_w = w * 0.48;

        for &action in actions {
            let row_bg = if (cy as i32 / ROW_HEIGHT as i32) % 2 == 0 {
                COL_PANEL
            } else {
                COL_PANEL_ALT
            };
            r.draw_rect(Vec2::new(x, cy), Vec2::new(w, ROW_HEIGHT), row_bg);

            // Action name
            let action_str = format!("{action}");
            r.draw_text(&action_str, Vec2::new(x + 8.0, cy + 6.0), LABEL_SIZE, COL_LABEL);

            // Binding(s)
            let bindings = self.input_map.bindings_for(action);
            let is_rebinding = self
                .rebind
                .as_ref()
                .map_or(false, |rb| rb.action == action && rb.for_gamepad == for_gamepad);

            if is_rebinding {
                r.draw_rect(
                    Vec2::new(x + half_w, cy + 2.0),
                    Vec2::new(w - half_w - 8.0, ROW_HEIGHT - 4.0),
                    COL_REBIND,
                );
                r.draw_text(
                    "Press any input...",
                    Vec2::new(x + half_w + 8.0, cy + 6.0),
                    LABEL_SIZE,
                    COL_TITLE,
                );
            } else {
                let label = if bindings.is_empty() {
                    "< unbound >".to_string()
                } else {
                    bindings
                        .iter()
                        .map(|b| format!("{b}"))
                        .collect::<Vec<_>>()
                        .join(",  ")
                };
                let col = if bindings.is_empty() { COL_LABEL_DIM } else { COL_LABEL };
                r.draw_text(&label, Vec2::new(x + half_w + 8.0, cy + 6.0), LABEL_SIZE, col);

                let hovered = self.cursor_pos.x >= x + half_w
                    && self.cursor_pos.x <= x + w
                    && self.cursor_pos.y >= cy
                    && self.cursor_pos.y <= cy + ROW_HEIGHT;
                if hovered {
                    r.draw_rect(
                        Vec2::new(x + half_w, cy),
                        Vec2::new(w - half_w, ROW_HEIGHT),
                        COL_HOVER,
                    );
                }
            }

            cy += ROW_HEIGHT;
        }
    }

    fn click_binding_rows(&mut self, cursor: Vec2, for_gamepad: bool) {
        let screen = self.cursor_screen_estimate();
        let (panel_pos, panel_size) = Self::panel_geometry(screen);
        let content_x = panel_pos.x + TAB_WIDTH + CONTENT_PAD;
        let content_w = panel_size.x - TAB_WIDTH - CONTENT_PAD * 2.0;
        let half_w = content_w * 0.48;
        // Approximate content_y since we can't borrow GuiRenderer here
        let content_y = panel_pos.y + 16.0 + TITLE_SIZE + 8.0 + 12.0;

        let actions = if for_gamepad { GP_ACTIONS } else { KB_ACTIONS };
        let mut cy = content_y + HEADER_SIZE + 12.0;

        for &action in actions {
            if cursor.x >= content_x + half_w
                && cursor.x <= content_x + content_w
                && cursor.y >= cy
                && cursor.y <= cy + ROW_HEIGHT
            {
                let slot = self.input_map.bindings_for(action).len();
                self.rebind = Some(RebindState {
                    action,
                    slot,
                    for_gamepad,
                });
                return;
            }
            cy += ROW_HEIGHT;
        }
    }

    // ───────────────────────────────────────────────────────────────
    // Mouse tab
    // ───────────────────────────────────────────────────────────────

    fn draw_mouse_tab(&self, r: &mut dyn GuiRenderer, x: f32, y: f32, w: f32) {
        let mut cy = y;
        r.draw_text("Mouse Look", Vec2::new(x, cy), HEADER_SIZE, COL_TITLE);
        cy += HEADER_SIZE + 12.0;

        cy = self.draw_slider(r, x, cy, w, "Sensitivity", self.controls.mouse_sensitivity, 0.001, 0.02);
        cy += 8.0;
        cy = self.draw_checkbox(r, x, cy, "Invert Pitch", self.controls.invert_mouse_pitch);
        cy += 4.0;
        cy = self.draw_checkbox(r, x, cy, "Invert Yaw", self.controls.invert_mouse_yaw);
        let _ = cy;
    }

    fn click_mouse_tab(&mut self, cursor: Vec2) {
        let screen = self.cursor_screen_estimate();
        let (panel_pos, panel_size) = Self::panel_geometry(screen);
        let x = panel_pos.x + TAB_WIDTH + CONTENT_PAD;
        let w = panel_size.x - TAB_WIDTH - CONTENT_PAD * 2.0;
        let cy = panel_pos.y + 16.0 + TITLE_SIZE + 8.0 + 12.0 + HEADER_SIZE + 12.0;

        // Sensitivity slider
        let slider_y = cy;
        let track_y = slider_y + ROW_HEIGHT - 4.0;
        let track_w = w - 60.0;
        let track_x = x + 30.0;
        if self.hit_track(cursor, track_x, track_y, track_w) {
            let t = ((cursor.x - track_x) / track_w).clamp(0.0, 1.0);
            self.controls.mouse_sensitivity = 0.001 + t * (0.02 - 0.001);
            self.slider_drag = Some(SliderDrag {
                id: slider_id(Tab::Mouse, 0),
                track_x,
                track_w,
                min: 0.001,
                max: 0.02,
            });
            return;
        }

        // Checkboxes
        let mut cy2 = cy + ROW_HEIGHT + 4.0 + 8.0; // past slider
        if self.hit_checkbox(cursor, x, cy2) {
            self.controls.invert_mouse_pitch = !self.controls.invert_mouse_pitch;
            return;
        }
        cy2 += ROW_HEIGHT + 4.0 + 4.0;
        if self.hit_checkbox(cursor, x, cy2) {
            self.controls.invert_mouse_yaw = !self.controls.invert_mouse_yaw;
        }
    }

    // ───────────────────────────────────────────────────────────────
    // Gamepad Settings tab
    // ───────────────────────────────────────────────────────────────

    fn draw_gamepad_settings_tab(&mut self, r: &mut dyn GuiRenderer, x: f32, y: f32, w: f32) {
        let mut cy = y;
        r.draw_text("Gamepad Look", Vec2::new(x, cy), HEADER_SIZE, COL_TITLE);
        cy += HEADER_SIZE + 12.0;

        cy = self.draw_slider(r, x, cy, w, "Stick Sensitivity", self.controls.stick_sensitivity, 0.5, 8.0);
        cy += 8.0;
        cy = self.draw_checkbox(r, x, cy, "Invert Stick Pitch", self.controls.invert_stick_pitch);
        cy += 4.0;
        cy = self.draw_checkbox(r, x, cy, "Invert Stick Yaw", self.controls.invert_stick_yaw);
        cy += 16.0;

        r.draw_text("Deadzone", Vec2::new(x, cy), HEADER_SIZE, COL_TITLE);
        cy += HEADER_SIZE + 12.0;

        cy = self.draw_slider(r, x, cy, w, "Left Stick Deadzone", self.gamepad_config.left_stick_deadzone, 0.0, 0.5);
        cy += 4.0;
        cy = self.draw_slider(r, x, cy, w, "Right Stick Deadzone", self.gamepad_config.right_stick_deadzone, 0.0, 0.5);
        cy += 8.0;
        cy = self.draw_deadzone_selector(r, x, cy);
        cy += 8.0;
        cy = self.draw_slider(r, x, cy, w, "Trigger Threshold", self.gamepad_config.trigger_threshold, 0.1, 0.9);
        let _ = cy;
    }

    fn click_gamepad_settings_tab(&mut self, cursor: Vec2) {
        let screen = self.cursor_screen_estimate();
        let (panel_pos, panel_size) = Self::panel_geometry(screen);
        let x = panel_pos.x + TAB_WIDTH + CONTENT_PAD;
        let w = panel_size.x - TAB_WIDTH - CONTENT_PAD * 2.0;
        let base_y = panel_pos.y + 16.0 + TITLE_SIZE + 8.0 + 12.0 + HEADER_SIZE + 12.0;
        let track_w = w - 60.0;
        let track_x = x + 30.0;

        // Stick Sensitivity slider
        let mut cy = base_y;
        let track_y = cy + ROW_HEIGHT - 4.0;
        if self.hit_track(cursor, track_x, track_y, track_w) {
            let t = ((cursor.x - track_x) / track_w).clamp(0.0, 1.0);
            self.controls.stick_sensitivity = 0.5 + t * (8.0 - 0.5);
            self.slider_drag = Some(SliderDrag {
                id: slider_id(Tab::GamepadSettings, 0),
                track_x,
                track_w,
                min: 0.5,
                max: 8.0,
            });
            return;
        }
        cy += ROW_HEIGHT + 4.0 + 8.0;

        // Invert Stick Pitch
        if self.hit_checkbox(cursor, x, cy) {
            self.controls.invert_stick_pitch = !self.controls.invert_stick_pitch;
            return;
        }
        cy += ROW_HEIGHT + 4.0 + 4.0;

        // Invert Stick Yaw
        if self.hit_checkbox(cursor, x, cy) {
            self.controls.invert_stick_yaw = !self.controls.invert_stick_yaw;
            return;
        }
        cy += ROW_HEIGHT + 4.0 + 16.0 + HEADER_SIZE + 12.0;

        // Left Stick Deadzone slider
        let track_y = cy + ROW_HEIGHT - 4.0;
        if self.hit_track(cursor, track_x, track_y, track_w) {
            let t = ((cursor.x - track_x) / track_w).clamp(0.0, 1.0);
            self.gamepad_config.left_stick_deadzone = t * 0.5;
            self.slider_drag = Some(SliderDrag {
                id: slider_id(Tab::GamepadSettings, 1),
                track_x,
                track_w,
                min: 0.0,
                max: 0.5,
            });
            return;
        }
        cy += ROW_HEIGHT + 4.0 + 4.0;

        // Right Stick Deadzone slider
        let track_y = cy + ROW_HEIGHT - 4.0;
        if self.hit_track(cursor, track_x, track_y, track_w) {
            let t = ((cursor.x - track_x) / track_w).clamp(0.0, 1.0);
            self.gamepad_config.right_stick_deadzone = t * 0.5;
            self.slider_drag = Some(SliderDrag {
                id: slider_id(Tab::GamepadSettings, 3),
                track_x,
                track_w,
                min: 0.0,
                max: 0.5,
            });
            return;
        }
        cy += ROW_HEIGHT + 4.0 + 8.0;

        // Deadzone shape selector (uses cached layout from draw)
        for &(sx, sw, _) in &self.deadzone_shape_rects {
            if cursor.x >= sx && cursor.x <= sx + sw && cursor.y >= cy && cursor.y <= cy + ROW_HEIGHT {
                let idx = self.deadzone_shape_rects.iter().position(|&(x, _, _)| x == sx).unwrap();
                let options = [DeadzoneShape::Circular, DeadzoneShape::PerAxis];
                self.gamepad_config.deadzone_shape = options[idx];
                return;
            }
        }
        cy += ROW_HEIGHT + 4.0 + 8.0;

        // Trigger Threshold slider
        let track_y = cy + ROW_HEIGHT - 4.0;
        if self.hit_track(cursor, track_x, track_y, track_w) {
            let t = ((cursor.x - track_x) / track_w).clamp(0.0, 1.0);
            self.gamepad_config.trigger_threshold = 0.1 + t * (0.9 - 0.1);
            self.slider_drag = Some(SliderDrag {
                id: slider_id(Tab::GamepadSettings, 2),
                track_x,
                track_w,
                min: 0.1,
                max: 0.9,
            });
        }
    }

    // ───────────────────────────────────────────────────────────────
    // Movement tab
    // ───────────────────────────────────────────────────────────────

    fn draw_movement_tab(&self, r: &mut dyn GuiRenderer, x: f32, y: f32, w: f32) {
        let mut cy = y;
        r.draw_text("Movement", Vec2::new(x, cy), HEADER_SIZE, COL_TITLE);
        cy += HEADER_SIZE + 12.0;
        cy = self.draw_slider(r, x, cy, w, "Move Speed", self.controls.move_speed, 10.0, 500.0);
        let _ = cy;
    }

    fn click_movement_tab(&mut self, cursor: Vec2) {
        let screen = self.cursor_screen_estimate();
        let (panel_pos, panel_size) = Self::panel_geometry(screen);
        let x = panel_pos.x + TAB_WIDTH + CONTENT_PAD;
        let w = panel_size.x - TAB_WIDTH - CONTENT_PAD * 2.0;
        let cy = panel_pos.y + 16.0 + TITLE_SIZE + 8.0 + 12.0 + HEADER_SIZE + 12.0;
        let track_x = x + 30.0;
        let track_w = w - 60.0;
        let track_y = cy + ROW_HEIGHT - 4.0;

        if self.hit_track(cursor, track_x, track_y, track_w) {
            let t = ((cursor.x - track_x) / track_w).clamp(0.0, 1.0);
            self.controls.move_speed = 10.0 + t * (500.0 - 10.0);
            self.slider_drag = Some(SliderDrag {
                id: slider_id(Tab::Movement, 0),
                track_x,
                track_w,
                min: 10.0,
                max: 500.0,
            });
        }
    }

    // ───────────────────────────────────────────────────────────────
    // Widget: slider
    // ───────────────────────────────────────────────────────────────

    fn draw_slider(
        &self,
        r: &mut dyn GuiRenderer,
        x: f32,
        y: f32,
        w: f32,
        label: &str,
        value: f32,
        min: f32,
        max: f32,
    ) -> f32 {
        // Label
        r.draw_text(label, Vec2::new(x, y + 2.0), LABEL_SIZE, COL_LABEL);

        // Value readout
        let val_str = format!("{:.3}", value);
        let val_m = r.measure_text(&val_str, LABEL_SIZE);
        r.draw_text(
            &val_str,
            Vec2::new(x + w - val_m.x, y + 2.0),
            LABEL_SIZE,
            COL_LABEL_DIM,
        );

        let track_y = y + ROW_HEIGHT - 4.0;
        let track_w = w - 60.0;
        let track_x = x + 30.0;

        // Track
        r.draw_rect(
            Vec2::new(track_x, track_y),
            Vec2::new(track_w, SLIDER_HEIGHT),
            COL_SLIDER_TRACK,
        );

        // Fill
        let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let fill_w = track_w * t;
        r.draw_rect(
            Vec2::new(track_x, track_y),
            Vec2::new(fill_w, SLIDER_HEIGHT),
            COL_SLIDER_FILL,
        );

        // Handle
        let handle_x = track_x + fill_w - SLIDER_HANDLE_W / 2.0;
        r.draw_rect(
            Vec2::new(handle_x, track_y - 3.0),
            Vec2::new(SLIDER_HANDLE_W, SLIDER_HEIGHT + 6.0),
            COL_SLIDER_HANDLE,
        );

        // Hover glow
        if self.hit_track(self.cursor_pos, track_x, track_y, track_w) {
            r.draw_rect(
                Vec2::new(track_x, track_y - 1.0),
                Vec2::new(track_w, SLIDER_HEIGHT + 2.0),
                COL_HOVER,
            );
        }

        y + ROW_HEIGHT + 4.0
    }

    // ───────────────────────────────────────────────────────────────
    // Widget: checkbox
    // ───────────────────────────────────────────────────────────────

    fn draw_checkbox(
        &self,
        r: &mut dyn GuiRenderer,
        x: f32,
        y: f32,
        label: &str,
        checked: bool,
    ) -> f32 {
        r.draw_text(
            label,
            Vec2::new(x + CHECKBOX_SIZE + 8.0, y + 4.0),
            LABEL_SIZE,
            COL_LABEL,
        );

        let bx = x;
        let by = y + 3.0;
        r.draw_rect(
            Vec2::new(bx, by),
            Vec2::new(CHECKBOX_SIZE, CHECKBOX_SIZE),
            COL_CHECKBOX,
        );

        if checked {
            r.draw_rect(
                Vec2::new(bx + 3.0, by + 3.0),
                Vec2::new(CHECKBOX_SIZE - 6.0, CHECKBOX_SIZE - 6.0),
                COL_CHECKBOX_CHECK,
            );
        }

        y + ROW_HEIGHT + 4.0
    }

    // ───────────────────────────────────────────────────────────────
    // Widget: deadzone shape selector
    // ───────────────────────────────────────────────────────────────

    fn draw_deadzone_selector(&mut self, r: &mut dyn GuiRenderer, x: f32, y: f32) -> f32 {
        r.draw_text("Deadzone Shape", Vec2::new(x, y + 2.0), LABEL_SIZE, COL_LABEL);

        let options = [DeadzoneShape::Circular, DeadzoneShape::PerAxis];
        let mut ox = x + 160.0;
        self.deadzone_shape_rects.clear();
        for &shape in &options {
            let label = format!("{shape}");
            let is_active = self.gamepad_config.deadzone_shape == shape;
            let lm = r.measure_text(&label, LABEL_SIZE);
            let bw = lm.x + 16.0;

            self.deadzone_shape_rects.push((ox, bw, if is_active { 1.0 } else { 0.0 }));

            let bg = if is_active { COL_ACTIVE } else { [0.0; 4] };
            r.draw_rect(Vec2::new(ox, y), Vec2::new(bw, ROW_HEIGHT), bg);

            if is_active {
                r.draw_rect(Vec2::new(ox, y), Vec2::new(2.0, ROW_HEIGHT), COL_GOLD);
            }

            let col = if is_active { COL_TITLE } else { COL_LABEL };
            r.draw_text(&label, Vec2::new(ox + 8.0, y + 6.0), LABEL_SIZE, col);

            if self.cursor_pos.x >= ox
                && self.cursor_pos.x <= ox + bw
                && self.cursor_pos.y >= y
                && self.cursor_pos.y <= y + ROW_HEIGHT
            {
                r.draw_rect(Vec2::new(ox, y), Vec2::new(bw, ROW_HEIGHT), COL_HOVER);
            }

            ox += bw + 4.0;
        }

        y + ROW_HEIGHT + 4.0
    }

    // ───────────────────────────────────────────────────────────────
    // Rebind overlay
    // ───────────────────────────────────────────────────────────────

    fn draw_rebind_overlay(&self, r: &mut dyn GuiRenderer, screen: Vec2) {
        let rb = self.rebind.as_ref().unwrap();
        r.draw_rect(Vec2::ZERO, screen, [0.0, 0.0, 0.0, 0.6]);

        let box_w = 400.0;
        let box_h = 120.0;
        let bx = (screen.x - box_w) / 2.0;
        let by = (screen.y - box_h) / 2.0;

        r.draw_rect(Vec2::new(bx, by), Vec2::new(box_w, box_h), COL_BG);

        let lw = 2.0;
        r.draw_rect(Vec2::new(bx, by), Vec2::new(box_w, lw), COL_GOLD_LINE);
        r.draw_rect(
            Vec2::new(bx, by + box_h - lw),
            Vec2::new(box_w, lw),
            COL_GOLD_LINE,
        );
        r.draw_rect(Vec2::new(bx, by), Vec2::new(lw, box_h), COL_GOLD_LINE);
        r.draw_rect(
            Vec2::new(bx + box_w - lw, by),
            Vec2::new(lw, box_h),
            COL_GOLD_LINE,
        );

        let title = if rb.for_gamepad {
            "Press a gamepad button..."
        } else {
            "Press a key or mouse button..."
        };
        let tm = r.measure_text(title, HEADER_SIZE);
        r.draw_text(
            title,
            Vec2::new(bx + (box_w - tm.x) / 2.0, by + 24.0),
            HEADER_SIZE,
            COL_TITLE,
        );

        let action_str = format!("{}", rb.action);
        let am = r.measure_text(&action_str, LABEL_SIZE);
        r.draw_text(
            &action_str,
            Vec2::new(bx + (box_w - am.x) / 2.0, by + 56.0),
            LABEL_SIZE,
            COL_LABEL,
        );

        let hint = "Press Escape to cancel";
        let hm = r.measure_text(hint, LABEL_SIZE * 0.85);
        r.draw_text(
            hint,
            Vec2::new(bx + (box_w - hm.x) / 2.0, by + box_h - 28.0),
            LABEL_SIZE * 0.85,
            COL_LABEL_DIM,
        );
    }

    // ───────────────────────────────────────────────────────────────
    // Hit-testing helpers
    // ───────────────────────────────────────────────────────────────

    fn hit_track(&self, cursor: Vec2, track_x: f32, track_y: f32, track_w: f32) -> bool {
        cursor.y >= track_y - 6.0
            && cursor.y <= track_y + SLIDER_HEIGHT + 6.0
            && cursor.x >= track_x
            && cursor.x <= track_x + track_w
    }

    fn hit_checkbox(&self, cursor: Vec2, x: f32, y: f32) -> bool {
        cursor.x >= x
            && cursor.x <= x + CHECKBOX_SIZE + 200.0
            && cursor.y >= y
            && cursor.y <= y + ROW_HEIGHT
    }

    /// Estimate screen size from stored draw info or cursor position.
    /// The draw path sets `screen_size` from `GuiRenderer::screen_size()`.
    /// The click path uses this for layout math.
    fn cursor_screen_estimate(&self) -> Vec2 {
        if self.screen_size.x > 0.0 && self.screen_size.y > 0.0 {
            return self.screen_size;
        }
        Vec2::new(
            (self.cursor_pos.x + PANEL_MARGIN + 100.0).max(880.0),
            (self.cursor_pos.y + PANEL_MARGIN + 100.0).max(720.0),
        )
    }

    // ───────────────────────────────────────────────────────────────
    // Input capture for rebind
    // ───────────────────────────────────────────────────────────────

    pub fn capture_input(
        input: &InputState,
        prev_keys: &HashSet<Key>,
        prev_mouse_left: &bool,
        prev_mouse_right: &bool,
        prev_mouse_middle: &bool,
        prev_mouse_back: &bool,
        prev_mouse_forward: &bool,
        prev_gp_buttons: &HashSet<GamepadButton>,
        prev_gp_axes: &HashMap<GamepadAxis, f32>,
        for_gamepad: bool,
    ) -> Option<InputBinding> {
        if for_gamepad {
            if let Some(gp) = input.gamepad(0) {
                for &btn in &ALL_GAMEPAD_BUTTONS {
                    if gp.button_down(btn) && !prev_gp_buttons.contains(&btn) {
                        return Some(InputBinding::GamepadButton(btn));
                    }
                }
                for &axis in &ALL_GAMEPAD_AXES {
                    let val = gp.axis_value(axis);
                    let prev = prev_gp_axes.get(&axis).copied().unwrap_or(0.0);
                    if prev <= 0.7 && val > 0.7 {
                        return Some(InputBinding::GamepadAxis {
                            axis,
                            direction: AxisDirection::Positive,
                        });
                    } else if prev >= -0.7 && val < -0.7 {
                        return Some(InputBinding::GamepadAxis {
                            axis,
                            direction: AxisDirection::Negative,
                        });
                    }
                }
            }
        } else {
            for &key in &ALL_KEYS {
                if input.key_down(key) && !prev_keys.contains(&key) {
                    return Some(InputBinding::Key(key));
                }
            }
            if input.mouse_left && !prev_mouse_left {
                return Some(InputBinding::MouseButton(MouseButton::Left));
            }
            if input.mouse_right && !prev_mouse_right {
                return Some(InputBinding::MouseButton(MouseButton::Right));
            }
            if input.mouse_middle && !prev_mouse_middle {
                return Some(InputBinding::MouseButton(MouseButton::Middle));
            }
            if input.mouse_back && !prev_mouse_back {
                return Some(InputBinding::MouseButton(MouseButton::Back));
            }
            if input.mouse_forward && !prev_mouse_forward {
                return Some(InputBinding::MouseButton(MouseButton::Forward));
            }
        }
        None
    }

    fn update_prev(&mut self, input: &InputState) {
        self.prev_keys.clear();
        for &key in &ALL_KEYS {
            if input.key_down(key) {
                self.prev_keys.insert(key);
            }
        }
        self.prev_mouse_left = input.mouse_left;
        self.prev_mouse_right = input.mouse_right;
        self.prev_mouse_middle = input.mouse_middle;
        self.prev_mouse_back = input.mouse_back;
        self.prev_mouse_forward = input.mouse_forward;
        self.prev_gamepad_buttons.clear();
        self.prev_gamepad_axes.clear();
        if let Some(gp) = input.gamepad(0) {
            for &btn in &ALL_GAMEPAD_BUTTONS {
                if gp.button_down(btn) {
                    self.prev_gamepad_buttons.insert(btn);
                }
            }
            for &axis in &ALL_GAMEPAD_AXES {
                self.prev_gamepad_axes.insert(axis, gp.axis_value(axis));
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Input lists for rebind capture
// ───────────────────────────────────────────────────────────────────

const ALL_GAMEPAD_BUTTONS: [GamepadButton; 21] = [
    GamepadButton::South,
    GamepadButton::East,
    GamepadButton::North,
    GamepadButton::West,
    GamepadButton::LeftBumper,
    GamepadButton::RightBumper,
    GamepadButton::LeftTrigger,
    GamepadButton::RightTrigger,
    GamepadButton::Select,
    GamepadButton::Start,
    GamepadButton::Guide,
    GamepadButton::Mode,
    GamepadButton::LeftStick,
    GamepadButton::RightStick,
    GamepadButton::DPadUp,
    GamepadButton::DPadDown,
    GamepadButton::DPadLeft,
    GamepadButton::DPadRight,
    GamepadButton::Touchpad,
    GamepadButton::C,
    GamepadButton::Z,
];

const ALL_GAMEPAD_AXES: [GamepadAxis; 6] = [
    GamepadAxis::LeftStickX,
    GamepadAxis::LeftStickY,
    GamepadAxis::RightStickX,
    GamepadAxis::RightStickY,
    GamepadAxis::LeftTrigger,
    GamepadAxis::RightTrigger,
];

const ALL_KEYS: [Key; 103] = [
    Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H,
    Key::I, Key::J, Key::K, Key::L, Key::M, Key::N, Key::O, Key::P,
    Key::Q, Key::R, Key::S, Key::T, Key::U, Key::V, Key::W, Key::X,
    Key::Y, Key::Z,
    Key::Digit0, Key::Digit1, Key::Digit2, Key::Digit3, Key::Digit4,
    Key::Digit5, Key::Digit6, Key::Digit7, Key::Digit8, Key::Digit9,
    Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6,
    Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
    Key::Up, Key::Down, Key::Left, Key::Right,
    Key::LeftShift, Key::RightShift, Key::LeftControl, Key::RightControl,
    Key::LeftAlt, Key::RightAlt, Key::LeftSuper, Key::RightSuper,
    Key::Space, Key::Enter, Key::Escape, Key::Tab, Key::Backspace,
    Key::Delete, Key::Insert, Key::Home, Key::End, Key::PageUp, Key::PageDown,
    Key::PrintScreen, Key::ScrollLock, Key::Pause,
    Key::Minus, Key::Equal, Key::LeftBracket, Key::RightBracket,
    Key::Backslash, Key::Semicolon, Key::Apostrophe,
    Key::Comma, Key::Period, Key::Slash, Key::Grave,
    Key::Numpad0, Key::Numpad1, Key::Numpad2, Key::Numpad3, Key::Numpad4,
    Key::Numpad5, Key::Numpad6, Key::Numpad7, Key::Numpad8, Key::Numpad9,
    Key::NumpadAdd, Key::NumpadSubtract, Key::NumpadMultiply, Key::NumpadDivide,
    Key::NumpadDecimal, Key::NumpadEnter, Key::NumpadEqual, Key::NumLock,
];

// ───────────────────────────────────────────────────────────────────
// Standalone rebind capture (usable from Lua-driven settings)
// ───────────────────────────────────────────────────────────────────

/// Lightweight rebind capture state, usable independently from
/// [`InputSettingsPanel`]. The Lua settings screen drives this via
/// the Rust host to perform key/gamepad rebinding.
pub struct RebindCapture {
    /// The action being rebound, if any.
    action: Option<Action>,
    /// Whether this rebind targets a gamepad binding.
    for_gamepad: bool,
    /// Previous-frame input snapshots for edge detection.
    prev_keys: HashSet<Key>,
    prev_mouse_left: bool,
    prev_mouse_right: bool,
    prev_mouse_middle: bool,
    prev_mouse_back: bool,
    prev_mouse_forward: bool,
    prev_gamepad_buttons: HashSet<GamepadButton>,
    prev_gamepad_axes: HashMap<GamepadAxis, f32>,
}

impl RebindCapture {
    pub fn new() -> Self {
        Self {
            action: None,
            for_gamepad: false,
            prev_keys: HashSet::new(),
            prev_mouse_left: false,
            prev_mouse_right: false,
            prev_mouse_middle: false,
            prev_mouse_back: false,
            prev_mouse_forward: false,
            prev_gamepad_buttons: HashSet::new(),
            prev_gamepad_axes: HashMap::new(),
        }
    }

    /// Start rebinding `action`. Set `for_gamepad` to capture gamepad input.
    pub fn start(&mut self, action: Action, for_gamepad: bool) {
        self.action = Some(action);
        self.for_gamepad = for_gamepad;
    }

    /// Cancel the current rebind.
    pub fn cancel(&mut self) {
        self.action = None;
    }

    /// Whether a rebind is in progress.
    pub fn is_active(&self) -> bool {
        self.action.is_some()
    }

    /// The action being rebound, if any.
    pub fn current_action(&self) -> Option<Action> {
        self.action
    }

    /// Whether the current rebind targets gamepad.
    pub fn is_gamepad(&self) -> bool {
        self.for_gamepad
    }

    /// Poll for a captured input. Returns `Some((action, binding))` when the
    /// user presses a button/key, or `None` if still waiting. Resolves
    /// conflicts by unbinding the input from any other action first.
    pub fn poll(
        &mut self,
        input: &InputState,
        input_map: &mut InputMap,
    ) -> Option<(Action, InputBinding)> {
        let action = self.action?;

        let captured = InputSettingsPanel::capture_input(
            input,
            &self.prev_keys,
            &self.prev_mouse_left,
            &self.prev_mouse_right,
            &self.prev_mouse_middle,
            &self.prev_mouse_back,
            &self.prev_mouse_forward,
            &self.prev_gamepad_buttons,
            &self.prev_gamepad_axes,
            self.for_gamepad,
        );

        self.update_prev(input);

        let binding = captured?;

        // Conflict detection: unbind from other actions
        if let Some(conflict_action) = input_map.action_for(binding) {
            if conflict_action != action {
                input_map.unbind(conflict_action, binding);
            }
        }

        // Remove old binding at slot 0 if it exists
        let old_bindings: Vec<InputBinding> = input_map.bindings_for(action).to_vec();
        if !old_bindings.is_empty() {
            input_map.unbind(action, old_bindings[0]);
        }

        input_map.bind(action, binding);
        self.action = None;

        Some((action, binding))
    }

    fn update_prev(&mut self, input: &InputState) {
        self.prev_keys.clear();
        for &key in &ALL_KEYS {
            if input.key_down(key) {
                self.prev_keys.insert(key);
            }
        }
        self.prev_mouse_left = input.mouse_left;
        self.prev_mouse_right = input.mouse_right;
        self.prev_mouse_middle = input.mouse_middle;
        self.prev_mouse_back = input.mouse_back;
        self.prev_mouse_forward = input.mouse_forward;
        self.prev_gamepad_buttons.clear();
        self.prev_gamepad_axes.clear();
        if let Some(gp) = input.gamepad(0) {
            for &btn in &ALL_GAMEPAD_BUTTONS {
                if gp.button_down(btn) {
                    self.prev_gamepad_buttons.insert(btn);
                }
            }
            for &axis in &ALL_GAMEPAD_AXES {
                self.prev_gamepad_axes.insert(axis, gp.axis_value(axis));
            }
        }
    }
}
