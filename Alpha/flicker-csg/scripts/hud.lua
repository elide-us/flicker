-- In-game HUD for flicker-csg, driven by Lua + `ui_elements.json` (the `UI.hud`
-- section) and the engine `Model` (live values published each frame). Layout /
-- colours / labels live in the JSON; this script owns only behaviour: formatting
-- the stat strings from the Model, and the checkbox click-state the engine
-- queries. Edit ui_elements.json to move / restyle the HUD — no recompile.
--
-- Trimmed from the voxel-cluster POC: the celestial day/night panel (sun / moon
-- / year / speed / fog / latitude / epoch sliders) is gone — flicker-csg uses a
-- single fixed world light — so only the debug-toggle checkboxes and the
-- move-speed / sensitivity / locomotion controls remain. Plain Lua.

local M = {}

-- Persistent checkbox state, keyed by each checkbox's `key` (the names the
-- engine queries). Starts empty → nil reads as off; toggled on click.
local checked = {}

-- Transient widget interaction state (slider drag / dropdown open), keyed by a
-- stable widget id — see widgets.lua. Values themselves live in the Model.
local widget_state = {}

-- The interactive controls' widget rects, from UI.hud.controls (label on the
-- left, widget at `widget_x`).
local function control_rects(sh)
  local c = UI.hud.controls
  local rh = c.row_h
  -- Bottom-left, stacked upward, so the (now larger) checkbox table above never
  -- covers it. `margin_b` leaves room below the locomotion row for its dropdown
  -- to open on-screen.
  local loco_y = sh - c.margin_b
  local sens_y = loco_y - rh
  local speed_y = sens_y - rh
  return c, {
    speed = { x = c.widget_x, y = speed_y, w = c.speed.w, h = c.speed.h },
    sens = { x = c.widget_x, y = sens_y, w = c.sens.w, h = c.sens.h },
    loco = { x = c.widget_x, y = loco_y, w = c.locomotion.w, h = c.locomotion.h },
  }
end

local function checkbox_items()
  return UI.hud.checkboxes.items
end

-- The i-th checkbox's clickable square, from the JSON geometry.
local function box_rect(i)
  local cb = UI.hud.checkboxes
  return { x = cb.origin[1], y = cb.origin[2] + (i - 1) * cb.row_h, w = cb.box, h = cb.box }
end

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

function M.update(mx, my, clicked, sw, sh, down)
  if not UI then
    return {}
  end
  if clicked then
    for i, item in ipairs(checkbox_items()) do
      if point_in(mx, my, box_rect(i)) then
        checked[item.key] = not checked[item.key]
      end
    end
  end
  local states = {}
  for _, item in ipairs(checkbox_items()) do
    states[item.key] = checked[item.key] or false
  end

  -- Interactive controls (slider / stepper / dropdown), wired two-way: the
  -- current value comes from the Model, the new value goes back in `states`,
  -- and the host applies it (so next frame the Model reflects it).
  if Model and Widgets then
    local c, r = control_rects(sh)
    states.move_speed = Widgets.slider_update(
      widget_state,
      "speed",
      r.speed,
      mx,
      my,
      clicked,
      down,
      Model.move_speed or 0,
      c.speed.min,
      c.speed.max
    )
    states.look_sens = Widgets.stepper_update(
      r.sens,
      mx,
      my,
      clicked,
      Model.look_sens or 0,
      c.sens.step,
      c.sens.min,
      c.sens.max
    )
    local current = (Model.walk and 2) or 1
    states.locomotion =
      Widgets.dropdown_update(widget_state, "loco", r.loco, mx, my, clicked, #c.locomotion.options, current)
  end
  return states
end

-- Stat readouts: each named line's style (y / size / colour) comes from
-- UI.hud.stats.<id>; the text is formatted here from the engine Model.
local function stats(cmds)
  if not Model then
    return
  end
  local s = UI.hud.stats
  local function line(spec, text)
    local c = spec.color
    cmds[#cmds + 1] =
      { kind = "text", x = s.x, y = spec.y, text = text, size = spec.size, r = c[1], g = c[2], b = c[3], a = c[4] }
  end

  local controls
  if Model.walk then
    controls = "walk — WASD on surface, gravity, right-drag look"
  else
    controls = "fly — WASD move, R/F up/down, right-drag look"
  end
  line(s.title, string.format("flicker-csg — %.0f×%.0f field — %s", Model.field_dim, Model.field_dim, controls))
  line(
    s.pos,
    string.format(
      "pos: (%.0f, %.0f, %.0f)  yaw: %.2f  pitch: %.2f",
      Model.pos_x,
      Model.pos_y,
      Model.pos_z,
      Model.yaw,
      Model.pitch
    )
  )
  line(s.clusters, string.format("clusters: %.0f   extent: %.0f³ voxels each", Model.cluster_count, Model.cluster_dim))
  line(
    s.config,
    string.format(
      "config — speed: %.0f  sens: %.4f  invert-Y: %s  invert-X: %s",
      Model.move_speed,
      Model.look_sens,
      tostring(Model.invert_y),
      tostring(Model.invert_x)
    )
  )
  line(
    s.diag,
    string.format("corner arrows stored: %.0f   nav clusters (rings 0–2): %.0f", Model.corner_arrows, Model.nav_count)
  )
  line(s.escape, "press Escape to quit")

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
  line(s.pick, pick)

  if Model.walk then
    local ground = "—"
    if Model.has_ground then
      ground = string.format("%.0f", Model.ground_y)
    end
    local grounded = Model.grounded and "grounded" or "airborne"
    line(s.walk, string.format("walk: %s   ground y: %s   vy: %+.1f", grounded, ground, Model.vy))
  end
end

-- The feature-toggle checkbox panel, from UI.hud.checkboxes.
local function checkboxes(cmds)
  local cb = UI.hud.checkboxes
  local h = cb.header
  cmds[#cmds + 1] = {
    kind = "text",
    x = cb.origin[1],
    y = cb.origin[2] - 24,
    text = h.text,
    size = h.size,
    r = h.color[1],
    g = h.color[2],
    b = h.color[3],
    a = h.color[4],
  }
  for i, item in ipairs(checkbox_items()) do
    local r = box_rect(i)
    local bc = cb.border_color
    cmds[#cmds + 1] =
      { kind = "rect", x = r.x, y = r.y, w = r.w, h = r.h, r = bc[1], g = bc[2], b = bc[3], a = bc[4] }
    local fill = checked[item.key] and cb.fill_on or cb.fill_off
    cmds[#cmds + 1] = {
      kind = "rect",
      x = r.x + 2,
      y = r.y + 2,
      w = r.w - 4,
      h = r.h - 4,
      r = fill[1],
      g = fill[2],
      b = fill[3],
      a = fill[4],
    }
    local lc = cb.label_color
    cmds[#cmds + 1] = {
      kind = "text",
      x = r.x + r.w + 10,
      y = r.y + 1,
      text = item.label,
      size = cb.label_size,
      r = lc[1],
      g = lc[2],
      b = lc[3],
      a = lc[4],
    }
  end
end

-- Interactive controls: a move-speed slider, a sensitivity stepper, and a
-- locomotion dropdown — drawn from the Widgets toolkit, styled from JSON, with
-- live values from the Model.
local function controls(cmds, sh)
  if not (Model and Widgets) then
    return
  end
  local c, r = control_rects(sh)
  local lc = c.label_color
  local function lbl(text, y)
    cmds[#cmds + 1] = {
      kind = "text",
      x = c.label_x,
      y = y,
      text = text,
      size = c.label_size,
      r = lc[1],
      g = lc[2],
      b = lc[3],
      a = lc[4],
    }
  end

  lbl(c.speed.label, r.speed.y)
  Widgets.slider_draw(cmds, r.speed, Model.move_speed or 0, c.speed.min, c.speed.max, c.slider_style)
  lbl(c.sens.label, r.sens.y)
  Widgets.stepper_draw(cmds, r.sens, Model.look_sens or 0, c.stepper_style, c.sens.fmt)
  lbl(c.locomotion.label, r.loco.y)
  local current = (Model.walk and 2) or 1
  Widgets.dropdown_draw(cmds, widget_state, "loco", r.loco, c.locomotion.options, current, c.dropdown_style)
end

function M.draw(sw, sh)
  if not UI then
    return {}
  end
  local cmds = {}
  stats(cmds)
  checkboxes(cmds)
  controls(cmds, sh)
  return cmds
end

return M
