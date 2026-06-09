# Handoff — Input Settings Panel: bug fixes and menu integration

> Builds on `docs/ui.md` (UI architecture), `docs/architecture.md` (engine/crate
> layering), and the scene system in `flicker-scene`. Re-verify line/symbol
> references — they drift.

## What changed

Three groups of changes landed together:

1. **Bug fixes** in `InputSettingsPanel` (`crates/flicker-core/src/input/settings_gui.rs`)
2. **Settings button** added to the pause modal (`examples/voxel-cluster/`)
3. **Rebindable menu toggle key** via `Action::Menu` (`bindings.rs` + `main.rs`)

`cargo check --workspace` clean. `cargo test -p flicker-core` passes (41 tests).
Clippy: only two pre-existing warnings in `main.rs` (untouched code).

---

## Part 1 — Bug fixes in `settings_gui.rs`

### 1a. Deadzone slider now writes to the correct fields

**Before:** The "Stick Deadzone" slider wrote to `controls.stick_deadzone`, but
the gamepad code reads `gamepad_config.left_stick_deadzone` /
`gamepad_config.right_stick_deadzone` (`mod.rs:348,357`). The slider had no
effect.

**After:** Two separate sliders — "Left Stick Deadzone" and "Right Stick
Deadzone" — write directly to `gamepad_config.left_stick_deadzone` and
`gamepad_config.right_stick_deadzone`. Slider IDs shifted:
- `GamepadSettings, 1` → left deadzone
- `GamepadSettings, 3` → right deadzone
- `GamepadSettings, 2` → trigger threshold (unchanged)

### 1b. `ALL_GAMEPAD_BUTTONS` now includes `Mode`

Array size `20` → `21`. `GamepadButton::Mode` inserted after `Guide`. The button
can now be captured during rebinding.

### 1c. `ALL_KEYS` now includes all numpad keys

Array size `91` → `103`. Added: `Numpad0`–`Numpad9`, `NumpadDecimal`,
`NumpadEqual`.

### 1d. Mouse buttons (right/middle/back/forward) now use edge detection

Added fields: `prev_mouse_right`, `prev_mouse_middle`, `prev_mouse_back`,
`prev_mouse_forward` (all `bool`). `capture_input` now gates on `!prev_*` for
all mouse buttons, matching the keyboard and left-click behavior. `update_prev`
tracks all five mouse button states.

### 1e. Gamepad axes now use edge detection

Added field `prev_gamepad_axes: HashMap<GamepadAxis, f32>`. `capture_input` now
only captures on a threshold crossing: previous value ≤ 0.7 and current value >
0.7 (for positive direction), or previous ≥ -0.7 and current < -0.7 (for
negative). `update_prev` stores current axis values.

### 1f. Tab hit-testing uses real screen size

Added field `screen_size: Vec2`. Set at the top of `draw()` from
`GuiRenderer::screen_size()`. `hit_test_tabs` uses it instead of the hardcoded
1920×1080. Falls back to the estimate if `draw()` hasn't been called yet.

### 1g. Deadzone shape selector hit-test matches draw

Added field `deadzone_shape_rects: Vec<(f32, f32, f32)>` (x, width,
active_flag). `draw_deadzone_selector` populates it using `measure_text()`.
`click_gamepad_settings_tab` reads the cached layout instead of the old
`label.len() * 8.0` estimate.

### 1h. `take_apply` resets panel state

`take_apply()` now resets `close_requested = false` and `visible = false` after
extracting the configs. The panel is reusable after this call.

---

## Part 2 — Settings button in pause menu

### 2a. `ui_elements.json`

Added `{ "id": "settings", "label": "SETTINGS" }` to `screens.pause.items`
between `"resume"` and `"quit"`. The panel is 384px tall; three buttons at
`first_y=146`, `gap_y=76` fit comfortably (last button bottom at 352px).

### 2b. `PauseScene::update`

Added handler: `if actions.is_on("settings") { self.input_panel.toggle(); }`.
Clicking SETTINGS opens/closes the input panel, same as the Tab shortcut (which
is kept as an alternate entry point).

---

## Part 3 — Rebindable menu toggle key

### 3a. `Action::Menu` bound to Escape in keyboard presets

In `bindings.rs`, both `wasd_and_mouse()` and `esdf_and_mouse()` now bind
`Action::Menu` to `Key::Escape`. The gamepad preset already bound
`Action::Menu` to `GamepadButton::Start`.

### 3b. `InputMap::action_pressed`

New method on `InputMap`:
```rust
pub fn action_pressed(&self, action: Action, input: &InputState) -> bool
```
Iterates `bindings_for(action)` and checks each binding against the current
`InputState`. Covers keys, mouse buttons, gamepad buttons, and gamepad axes.

### 3c. `GameScene` uses `Action::Menu`

Replaced the hardcoded `input.key_down(Key::Escape)` check with:
```rust
let menu_down = self.bindings.action_pressed(Action::Menu, input);
let menu_pressed = menu_down && !self.menu_prev;
```
Added `menu_prev: bool` field (initialized `false`). Removed `escape_prev`
(no longer used).

### 3d. `PauseScene` uses `Action::Menu`

Same pattern. Added `bindings: InputMap` field (cloned from the constructor
arg) and `menu_prev: bool` (initialized `true` to avoid triggering on the
first frame). Replaced the Escape-to-resume check with `action_pressed`.

### 3e. Gamepad preset

`Action::Menu` was already bound to `GamepadButton::Start` in
`gamepad_default()`. Verified — no change needed.

---

## Files changed

| File | Changes |
|---|---|
| `crates/flicker-core/src/input/settings_gui.rs` | Bug fixes 1a–1h |
| `crates/flicker-core/src/input/bindings.rs` | `Action::Menu` keyboard bindings, `action_pressed` method |
| `examples/voxel-cluster/ui_elements.json` | SETTINGS button in pause screen |
| `examples/voxel-cluster/src/main.rs` | `PauseScene`/`GameScene` use `Action::Menu`, settings button handler |

## Invariants preserved

1. **Buffered changes.** `InputSettingsPanel` still holds local copies; changes
   only commit via `into_apply()` / `take_apply()`.
2. **`measure_text` needs `&mut Renderer`.** `draw()` now takes `&mut self` to
   store `screen_size` and `deadzone_shape_rects`. Call sites already had `&mut
   self` available.
3. **Lua boundary unchanged.** The input settings panel is pure Rust
   (`GuiRenderer` trait). The Lua modal system only handles the pause menu
   buttons.
4. **Scene model.** `PauseScene` is an overlay. The input panel is drawn inside
   `PauseScene::render`. The `INPUT_SETTINGS` static mutex bridges the scene
   boundary.
5. **Edge detection everywhere.** All rebind capture (keyboard, mouse buttons,
   gamepad buttons, gamepad axes) uses edge detection.
