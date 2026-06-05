-- FLICKER main menu, driven from Lua.
--
-- Owns its own layout, hit-testing, and draw list, using the engine textures
-- the host exposes via `Textures` (panel / button / white). This mirrors the
-- gothic `modal_layout` + `draw_panel` that used to live in Rust
-- (examples/voxel-cluster/src/ui.rs) — the host still bakes the procedural
-- art; Lua just composes the screen. Plain Lua (runs on the Luau VM).
--
-- Contract (see flicker-script):
--   update(mx, my, clicked, sw, sh) -> { start=bool, quit=bool }  -- momentary
--   draw(sw, sh) -> { <command>, ... }                            -- rect/sprite/text

local M = {}

-- Gothic sizing + palette, mirroring the consts in ui.rs.
local PANEL_W, PANEL_H = 520, 384
local FRAME = 38
local BUTTON_W, BUTTON_H = 264, 54

local COL_BACKDROP = { 0.035, 0.04, 0.05, 1 }
local COL_TITLE = { 0.83, 0.67, 0.39, 1 }
local COL_LABEL = { 0.78, 0.81, 0.86, 1 }
local COL_LABEL_HOVER = { 0.96, 0.80, 0.42, 1 }
local COL_GOLD = { 0.85, 0.66, 0.32, 0.95 }
local COL_SHEEN = { 0.85, 0.66, 0.34, 0.15 }

-- Which button the cursor is over this frame ("top"/"bottom"/nil): set in
-- update, read in draw.
local hover = nil

-- Centre the fixed-size panel and stack its two buttons, from the screen size.
local function layout(sw, sh)
  local px = math.floor((sw - PANEL_W) * 0.5 + 0.5)
  local py = math.floor((sh - PANEL_H) * 0.5 + 0.5)
  local bx = px + (PANEL_W - BUTTON_W) * 0.5
  return {
    px = px,
    py = py,
    title_x = px + PANEL_W * 0.5,
    title_y = py + FRAME + 22,
    top = { x = bx, y = py + FRAME + 108, w = BUTTON_W, h = BUTTON_H },
    bottom = { x = bx, y = py + FRAME + 184, w = BUTTON_W, h = BUTTON_H },
  }
end

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

function M.update(mx, my, clicked, sw, sh)
  local l = layout(sw, sh)
  hover = nil
  if point_in(mx, my, l.top) then
    hover = "top"
  elseif point_in(mx, my, l.bottom) then
    hover = "bottom"
  end

  local actions = {}
  if clicked then
    if hover == "top" then
      actions.start = true
    elseif hover == "bottom" then
      actions.quit = true
    end
  end
  return actions
end

-- Append a solid colored rect.
local function rect(cmds, x, y, w, h, c)
  cmds[#cmds + 1] =
    { kind = "rect", x = x, y = y, w = w, h = h, r = c[1], g = c[2], b = c[3], a = c[4] }
end

-- A 2px gold border around rect `r`.
local function outline(cmds, r, t, c)
  rect(cmds, r.x, r.y, r.w, t, c)
  rect(cmds, r.x, r.y + r.h - t, r.w, t, c)
  rect(cmds, r.x, r.y, t, r.h, c)
  rect(cmds, r.x + r.w - t, r.y, t, r.h, c)
end

local function button(cmds, r, label, hovered)
  cmds[#cmds + 1] = { kind = "sprite", tex = Textures.button, x = r.x, y = r.y, w = r.w, h = r.h }
  local col = COL_LABEL
  if hovered then
    rect(cmds, r.x, r.y, r.w, r.h, COL_SHEEN) -- gold sheen
    outline(cmds, r, 2, COL_GOLD)
    col = COL_LABEL_HOVER
  end
  cmds[#cmds + 1] = {
    kind = "text",
    x = r.x + r.w * 0.5,
    y = r.y + (r.h - 22) * 0.5,
    text = label,
    size = 22,
    align = "center",
    r = col[1],
    g = col[2],
    b = col[3],
    a = col[4],
  }
end

function M.draw(sw, sh)
  local l = layout(sw, sh)
  local cmds = {}

  -- Opaque backdrop (nothing behind the menu).
  rect(cmds, 0, 0, sw, sh, COL_BACKDROP)

  -- Gothic panel + cartouche title.
  cmds[#cmds + 1] =
    { kind = "sprite", tex = Textures.panel, x = l.px, y = l.py, w = PANEL_W, h = PANEL_H }
  cmds[#cmds + 1] = {
    kind = "text",
    x = l.title_x,
    y = l.title_y,
    text = "FLICKER",
    size = 34,
    align = "center",
    r = COL_TITLE[1],
    g = COL_TITLE[2],
    b = COL_TITLE[3],
    a = COL_TITLE[4],
  }

  button(cmds, l.top, "START", hover == "top")
  button(cmds, l.bottom, "QUIT", hover == "bottom")

  return cmds
end

return M
