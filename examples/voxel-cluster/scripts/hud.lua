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
-- (VoxelCluster reads "wireframe" / "corner_arrows" / "navmesh" /
-- "surface_walk" / "camera_lod" / "lod_billboards").
local checkboxes = {
  { key = "wireframe",     label = "Wireframe overlay",          checked = false },
  { key = "corner_arrows", label = "Corner-vector arrows",       checked = false },
  { key = "navmesh",       label = "Navmesh wireframe",          checked = false },
  { key = "surface_walk",  label = "Surface walk (gen nav)",     checked = false },
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

-- Left-column engine stats, read from the `Model` global the host publishes
-- each frame (flicker-script's data-model channel). These lines used to be
-- hardcoded `draw_text` in Rust; owning them here means the layout/formatting
-- is editable without a recompile. Colours/positions mirror the old readout.
local DIM = { 0.75, 0.85, 0.95 }
local function stats(cmds)
  if not Model then
    return
  end
  local function line(y, size, c, text)
    cmds[#cmds + 1] =
      { kind = "text", x = 16, y = y, text = text, size = size, r = c[1], g = c[2], b = c[3] }
  end

  local controls
  if Model.walk then
    controls = "walk — WASD on surface, gravity, right-drag look"
  else
    controls = "fly — WASD move, R/F up/down, right-drag look"
  end
  line(
    16,
    22,
    { 1, 1, 1 },
    string.format("voxel cluster — %.0f×%.0f field — %s", Model.field_dim, Model.field_dim, controls)
  )
  line(
    44,
    16,
    DIM,
    string.format(
      "pos: (%.0f, %.0f, %.0f)  yaw: %.2f  pitch: %.2f",
      Model.pos_x,
      Model.pos_y,
      Model.pos_z,
      Model.yaw,
      Model.pitch
    )
  )
  line(
    64,
    16,
    DIM,
    string.format("clusters: %.0f   extent: %.0f³ voxels each", Model.cluster_count, Model.cluster_dim)
  )
  line(
    84,
    16,
    DIM,
    string.format(
      "config — speed: %.0f  sens: %.4f  invert-Y: %s  invert-X: %s",
      Model.move_speed,
      Model.look_sens,
      tostring(Model.invert_y),
      tostring(Model.invert_x)
    )
  )
  line(
    104,
    16,
    DIM,
    string.format(
      "corner arrows stored: %.0f   nav clusters (rings 0–2): %.0f",
      Model.corner_arrows,
      Model.nav_count
    )
  )
  line(124, 16, DIM, "press Escape to quit")

  local pick
  if Model.has_pick then
    pick = string.format(
      "pick: (%.0f, %.0f, %.0f, lod %.0f) p = (%.0f, %.0f, %.0f)",
      Model.pick_cx,
      Model.pick_cy,
      Model.pick_cz,
      Model.pick_lod,
      Model.pick_px,
      Model.pick_py,
      Model.pick_pz
    )
  else
    pick = "pick: (none — left-click a face)"
  end
  line(144, 16, { 0.95, 0.85, 0.60 }, pick)

  if Model.walk then
    local ground = "—"
    if Model.has_ground then
      ground = string.format("%.0f", Model.ground_y)
    end
    local grounded = Model.grounded and "grounded" or "airborne"
    line(
      164,
      16,
      { 0.6, 0.95, 0.7 },
      string.format("walk: %s   ground y: %s   vy: %+.1f", grounded, ground, Model.vy)
    )
  end
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

  -- Left-column engine stats (from Model), then the checkbox panel below.
  stats(cmds)

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
