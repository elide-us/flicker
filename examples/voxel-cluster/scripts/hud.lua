-- voxel-cluster HUD: six interactive checkboxes that drive the
-- engine's debug toggles, replacing the old `1`/`2`/`\` key handling.
--
-- This is plain Lua (no Luau-specific syntax). The host runs it once
-- at startup; the returned module exposes:
--   * update(mouse_x, mouse_y, clicked) -> { name = bool, ... }
--   * draw() -> { <command>, ... }
-- where each command is a table with a `kind` of "rect" or "text".
-- See flicker-script's HudCommand for the recognised fields.

local M = {}

-- The toggles, in draw order. `key` is the name the engine queries
-- (VoxelCluster reads "wireframe" / "corner_arrows" / "center_lod" /
-- "navmesh" / "camera_lod" / "lod_billboards").
local checkboxes = {
  { key = "wireframe",     label = "Wireframe overlay",          checked = false },
  { key = "corner_arrows", label = "Corner-vector arrows",       checked = false },
  { key = "center_lod",    label = "Center cluster: LOD 1",       checked = false },
  { key = "navmesh",       label = "Navmesh wireframe",          checked = false },
  { key = "camera_lod",    label = "Camera-driven LOD",          checked = false },
  { key = "lod_billboards",label = "LOD billboards",             checked = false },
}

-- Panel layout, in HUD pixels (origin top-left).
local ORIGIN_X = 16
local ORIGIN_Y = 180
local ROW_H = 26
local BOX = 18

-- Top-left corner + size of the i-th checkbox's clickable square.
local function box_rect(i)
  return ORIGIN_X, ORIGIN_Y + (i - 1) * ROW_H, BOX, BOX
end

local function point_in(px, py, x, y, w, h)
  return px >= x and px <= x + w and py >= y and py <= y + h
end

-- Per-frame: flip a checkbox if the click landed on its square, then
-- report every toggle's state back to the engine.
function M.update(mx, my, clicked)
  if clicked then
    for i, cb in ipairs(checkboxes) do
      local x, y, w, h = box_rect(i)
      if point_in(mx, my, x, y, w, h) then
        cb.checked = not cb.checked
      end
    end
  end

  local states = {}
  for _, cb in ipairs(checkboxes) do
    states[cb.key] = cb.checked
  end
  return states
end

-- Per-frame: describe the panel as draw commands the engine renders.
function M.draw()
  local cmds = {}

  cmds[#cmds + 1] = {
    kind = "text",
    x = ORIGIN_X, y = ORIGIN_Y - 24,
    text = "HUD (scripted) - click to toggle",
    size = 16,
    r = 0.85, g = 0.85, b = 0.70,
  }

  for i, cb in ipairs(checkboxes) do
    local x, y, w, h = box_rect(i)

    -- White border box.
    cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = 1, g = 1, b = 1 }

    -- Inner fill: neon green when checked, dark grey when not.
    local fr, fg, fb = 0.15, 0.15, 0.18
    if cb.checked then
      fr, fg, fb = 0.20, 0.90, 0.40
    end
    cmds[#cmds + 1] = {
      kind = "rect",
      x = x + 2, y = y + 2, w = w - 4, h = h - 4,
      r = fr, g = fg, b = fb,
    }

    -- Label to the right of the box.
    cmds[#cmds + 1] = {
      kind = "text",
      x = x + w + 10, y = y + 1,
      text = cb.label,
      size = 16,
      r = 0.90, g = 0.90, b = 0.90,
    }
  end

  return cmds
end

return M
