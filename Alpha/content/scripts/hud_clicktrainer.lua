-- flicker-clicktrainer HUD — the reference blend of a VECTOR UI over a 2D SPRITE
-- game. A carved-stone stats panel (top-left) drawn with `panel` HudCommands,
-- driven by `UI.clicktrainer` (ui_elements.json) + the scene's `Model` (stat
-- strings). It owns two things the engine reads back:
--   * `hud_hit`  — the cursor is over the panel, so the scene must treat the
--     click as a UI click (RESET), NOT score it as a game hit/miss.
--   * `reset`    — the RESET button fired this frame.
--
--   update(mx, my, clicked, sw, sh, down) -> { hud_hit = bool, reset = bool }
--   draw(sw, sh) -> panel + stats + RESET button (vector), at layer 2 so it
--                   sits above the game's 2D target sprites (layer 0).

local M = {}

-- Layer for the whole HUD: above the game sprites (drawn at the scene base = 0),
-- below any pushed overlay (the pause modal is a whole scene-band above).
local L = 2

-- Hovered-reset flag (set in update, read in draw).
local reset_hover = false

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

local function mtext(k)
  return Model[k] or "—"
end

local function panel(cmds, x, y, w, h, c0, c1, grad, radius, border, bcolor, feather)
  local cmd = { kind = "panel", x = x, y = y, w = w, h = h,
    r = c0[1], g = c0[2], b = c0[3], a = c0[4],
    grad = grad or 0, radius = radius or 0, border = border or 0, feather = feather or 0, layer = L }
  if c1 then cmd.r2 = c1[1]; cmd.g2 = c1[2]; cmd.b2 = c1[3]; cmd.a2 = c1[4] end
  if bcolor then cmd.br = bcolor[1]; cmd.bg = bcolor[2]; cmd.bb = bcolor[3]; cmd.ba = bcolor[4] end
  cmds[#cmds + 1] = cmd
end

local function text(cmds, x, y, str, size, c, align, font)
  cmds[#cmds + 1] = { kind = "text", x = x, y = y, text = str, size = size, align = align or "left",
    font = font, r = c[1], g = c[2], b = c[3], a = c[4], layer = L }
end

-- Panel geometry + the reset-button rect, computed top-down so update + draw
-- agree.
local function layout()
  local ct = UI.clicktrainer
  local p = ct.panel
  local x, y = ct.margin, ct.margin
  local pad = p.pad
  local cx = x + pad
  local c = y + pad
  local title_y = c;                 c = c + ct.title.size + 4
  local sub_y = c;                   c = c + ct.subtitle.size + 10
  local div_y = c;                   c = c + 1 + 12
  local rows_y = c;                  c = c + #ct.rows * ct.row_h
  c = c + ct.reset.gap_top
  local reset_y = c;                 c = c + ct.reset.h
  c = c + ct.hint.gap_top
  local hint_y = c;                  c = c + ct.hint.size
  c = c + pad
  return {
    panel = { x = x, y = y, w = p.w, h = c - y },
    cx = cx, inner_w = p.w - pad * 2,
    title_y = title_y, sub_y = sub_y, div_y = div_y, rows_y = rows_y,
    reset = { x = cx, y = reset_y, w = p.w - pad * 2, h = ct.reset.h },
    hint_y = hint_y,
  }
end

function M.update(mx, my, clicked, sw, sh, down)
  if not (UI and UI.clicktrainer and Widgets) then
    return {}
  end
  local l = layout()
  reset_hover = point_in(mx, my, l.reset)
  return {
    -- A click anywhere on the panel is consumed by the HUD (see the scene's
    -- `!over_hud` gate), so it never scores as a game miss.
    hud_hit = point_in(mx, my, l.panel),
    reset = Widgets.button_update(l.reset, mx, my, clicked),
  }
end

function M.draw(sw, sh)
  if not (UI and UI.clicktrainer) then
    return {}
  end
  local ct = UI.clicktrainer
  local l = layout()
  local cmds = {}
  local p = ct.panel

  -- Soft drop shadow + the panel.
  local g = p.shadow_grow or 0
  panel(cmds, l.panel.x - g, l.panel.y - g + (p.shadow_dy or 0), l.panel.w + 2 * g, l.panel.h + 2 * g,
    p.shadow, nil, 0, (p.radius or 0) + g, 0, nil, p.shadow_feather or 0)
  panel(cmds, l.panel.x, l.panel.y, l.panel.w, l.panel.h, p.fill_top, p.fill_bot, 1, p.radius, p.border_w, p.border, 0)

  -- Title + subtitle + bronze divider.
  text(cmds, l.cx, l.title_y, ct.title.text, ct.title.size, ct.title.color, "left", "display")
  text(cmds, l.cx, l.sub_y, ct.subtitle.text, ct.subtitle.size, ct.subtitle.color, "left", "body")
  panel(cmds, l.cx, l.div_y, l.inner_w, 1, ct.divider, nil, 0, 0, 0, nil, 0)

  -- Stat rows: label left, value right (accuracy in the accent colour).
  for i, row in ipairs(ct.rows) do
    local ry = l.rows_y + (i - 1) * ct.row_h
    text(cmds, l.cx, ry, row.label, ct.label_size, ct.label_color, "left", "body")
    local vc = (row.id == "accuracy") and ct.accent or ct.value_color
    text(cmds, l.cx + l.inner_w, ry, mtext(row.id), ct.value_size, vc, "right", "label")
  end

  -- RESET button — the shared modal secondary-button kit (hover-lit).
  local v = UI.modal.buttons.variants.secondary
  local r = l.reset
  local ft = reset_hover and v.hover_top or v.fill_top
  local fb = reset_hover and v.hover_bot or v.fill_bot
  local bc = reset_hover and v.hover_border or v.border
  panel(cmds, r.x, r.y, r.w, r.h, ft, fb, 1, 3, 1, bc, 0)
  local lc = reset_hover and v.hover_label or v.label
  text(cmds, r.x + r.w * 0.5, r.y + (r.h - 14) * 0.5, ct.reset.label, 14, lc, "center", "label")

  -- Hint.
  text(cmds, l.cx, l.hint_y, ct.hint.text, ct.hint.size, ct.hint.color, "left", "label")

  return cmds
end

return M
