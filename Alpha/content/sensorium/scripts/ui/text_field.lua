-- ui.text_field — a single-line text input in a sunk-black well (DS `TextField`):
-- the well panel (top/bot fill, rounded, bordered), then the bound value — or the
-- dim `placeholder` when the value is empty — left-aligned and vertically centred.
-- The border is a state channel: the rune-light focus ring (2px, `caret` colour)
-- while this field owns keyboard focus, the bronze hover edge under the pointer,
-- else the resting edge. While focused a block caret sits at the END of the text,
-- positioned by REAL glyph measurement: the module emits `core.caret` with the
-- buffer as its `prefix` and the render bridge measures the shaped Unicode string
-- (text ruling 2026-07-31 — never `chars × advance`).
--
-- This module owns the field's draw AND hit; the KEYBOARD is deliberately NOT
-- here: focus is held by the walker (`state.focus`, claimed through this hit's
-- `focus` verdict and cleared by the generic clicked-frame rule), typed chars and
-- backspace fold into the bound string by the walker's generic Rust `fold_typed`
-- pass, and the every-frame value report is `echo_binds` — a typing frame with a
-- parked pointer must never cross into Lua.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (the bound string — USER DATA, never stringtable-resolved),
--   placeholder (display text, crossed already resolved), focused (this field owns
--   the walker's keyboard focus), mx, my (pointer, for the hover edge),
--   label_size / text_pad / caret_w (node props), layer, style { top, bot, border,
--   hover_border, caret, focus_border?, placeholder, label, label_size, radius }.

local core = require("ui.core")

local M = {}

local INK = { 0.871, 0.847, 0.788, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local STONE = { 0.055, 0.063, 0.086, 1.0 }
local RUNE = { 0.435, 0.592, 1.0, 1.0 }

function M.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local focused = props.focused == true
  local hovered = core.point_in(props.mx, props.my, r)

  -- The sunk-black well. Border: the rune-light ring when focused, the bronze
  -- hover edge on hover, else the resting edge.
  local top = core.first_color(s, { "top", "fill_top", "bg" }, STONE)
  local bot = core.first_color(s, { "bot", "fill_bot", "bg" }, top)
  local border
  if focused then
    border = core.first_color(s, { "caret", "focus_border", "border" }, RUNE)
  elseif hovered then
    border = core.first_color(s, { "hover_border", "border" }, DIM)
  else
    border = core.first_color(s, { "border" }, STONE)
  end
  local border_w = focused and 2 or 1
  core.panel(cmds, r, {
    fill = top,
    fill2 = bot,
    radius = core.jnum(s, "radius", 3),
    border = (border[4] > 0) and border_w or 0,
    border_color = border,
    layer = layer,
  })

  -- Value (or the placeholder when empty), left-aligned, vertically centred. The
  -- value is user data — never string-resolved (the placeholder crossed resolved).
  local lsz = core.jnum(s, "label_size", core.jnum(props, "label_size", 14))
  local pad_x = core.jnum(props, "text_pad", 8)
  local value = type(props.bind_value) == "string" and props.bind_value or ""
  local shown, color
  if value == "" then
    shown = props.placeholder or ""
    color = core.first_color(s, { "placeholder" }, DIM)
  else
    shown = value
    color = core.first_color(s, { "label", "color" }, INK)
  end
  local tx = r.x + pad_x
  local ty = r.y + (r.h - lsz) * 0.5
  core.text(cmds, tx, ty, shown, lsz, color, "left", "body", layer)

  -- Block caret at the END of the text while focused: the whole buffer rides as
  -- the caret's `prefix` for the render bridge to measure, clamped inside the well.
  if focused then
    local cw = core.jnum(props, "caret_w", 2)
    core.caret(cmds, tx, ty, cw, lsz, value, lsz, core.first_color(s, { "caret" }, RUNE),
      layer, "body", r.x + r.w - pad_x - cw)
  end
end

-- The well claims on hover; a click inside claims KEYBOARD focus through the
-- verdict (the walker holds focus by node id — the clicked frame's generic clear
-- plus this claim is the whole focus life cycle). Typed chars never arrive here:
-- the walker's Rust `fold_typed` pass owns the keyboard.
function M.hit(mx, my, r, props, click)
  local v = { hit = core.point_in(mx, my, r) }
  if click and v.hit then
    v.focus = true
  end
  return v
end

return M
