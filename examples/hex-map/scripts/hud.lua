-- hex-map HUD: a read-only camera read-out (top-left) and a compass / controls
-- legend (bottom-left). Layout and colours live in ui_elements.json (`UI.hud`);
-- the live camera values arrive each frame in the `Model` global. Plain Lua;
-- edit ui_elements.json to restyle without recompiling.

local M = {}

-- Spread a `{r, g, b, a}` colour table into the r/g/b/a fields the engine reads.
local function text(cmds, x, y, t, size, col)
  cmds[#cmds + 1] = {
    kind = "text",
    x = x,
    y = y,
    text = t,
    size = size,
    r = col[1],
    g = col[2],
    b = col[3],
    a = col[4] or 1.0,
  }
end

-- No interactive widgets yet — the HUD is read-only this slice.
function M.update(_mx, _my, _clicked, _sw, _sh, _down)
  return {}
end

function M.draw(_sw, sh)
  local cmds = {}
  local m = Model or {}

  -- Camera read-out, top-left.
  local s = UI.hud.stats
  text(cmds, s.x, s.y, "HEX MAP", s.title_size, s.title_color)
  text(cmds, s.x, s.y + s.line_h,
    string.format("pos   %.0f, %.0f, %.0f", m.pos_x or 0, m.pos_y or 0, m.pos_z or 0),
    s.value_size, s.value_color)
  text(cmds, s.x, s.y + s.line_h * 2, string.format("yaw   %.2f", m.yaw or 0), s.value_size, s.value_color)
  text(cmds, s.x, s.y + s.line_h * 3, string.format("pitch %.2f", m.pitch or 0), s.value_size, s.value_color)

  -- Compass + controls legend, anchored to the bottom-left.
  local L = UI.hud.legend
  local y = sh - L.margin_b
  text(cmds, L.x, y, "COMPASS   (+ = bright arrow)", L.size, L.head_color)
  text(cmds, L.x, y + L.line_h, "X red    + west  / - east", L.size, L.x_color)
  text(cmds, L.x, y + L.line_h * 2, "Y green  + up    / - down", L.size, L.y_color)
  text(cmds, L.x, y + L.line_h * 3, "Z blue   + north / - south", L.size, L.z_color)
  text(cmds, L.x, y + L.line_h * 4, "WASD move  R/F up/down  RMB look  Esc quit", L.size - 2, L.hint_color)

  return cmds
end

return M
