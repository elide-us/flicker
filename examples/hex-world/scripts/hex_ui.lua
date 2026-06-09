-- hex-world view/sim controls, driven by Lua + `ui_elements.json` (the `UI.hud`
-- section) and the engine `Model` (live values published each frame). Layout /
-- colours / labels live in the JSON; this script owns behaviour: the checkbox
-- click-state the engine queries, the widget interaction, and formatting the
-- stat strings from the Model. Edit ui_elements.json to restyle — no recompile.
--
-- The reusable `Widgets` toolkit (slider / dropdown / button) is provided by
-- flicker-ui; values for the slider/dropdown live in the engine Model (two-way),
-- only transient drag/open state is kept here.

local M = {}

-- Checkbox state, keyed by each checkbox's `key` (the names the engine queries).
-- Seeded so the overlays the world ships with read ON from frame 1.
local checked = { graticule = true, billboards = true, sim_paused = false }

-- Transient widget interaction state (slider drag / dropdown open), keyed by id.
local widget_state = {}

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

local function checkbox_items()
  return UI.hud.checkboxes.items
end

-- The i-th checkbox's clickable square, from the JSON geometry.
local function box_rect(i)
  local cb = UI.hud.checkboxes
  return { x = cb.origin[1], y = cb.origin[2] + (i - 1) * cb.row_h, w = cb.box, h = cb.box }
end

-- Control widget rects (slider / button / dropdown), stacked from origin_y. The
-- dropdown is last so its open list extends down into empty space.
local function control_rects()
  local c = UI.hud.controls
  local rh = c.row_h
  local y = c.origin_y
  return c, {
    speed = { x = c.widget_x, y = y, w = c.speed.w, h = c.speed.h },
    reset = { x = c.label_x, y = y + rh, w = c.reset.w, h = c.reset.h },
    view = { x = c.widget_x, y = y + rh * 2, w = c.view.w, h = c.view.h },
  }
end

function M.update(mx, my, clicked, sw, sh, down)
  if not UI then
    return {}
  end
  local s = {}

  if clicked then
    for i, item in ipairs(checkbox_items()) do
      if point_in(mx, my, box_rect(i)) then
        checked[item.key] = not checked[item.key]
      end
    end
  end
  for _, item in ipairs(checkbox_items()) do
    s[item.key] = checked[item.key] or false
  end

  -- Interactive widgets, wired two-way: the value comes from the Model, the new
  -- value goes back in `s`, the host applies it (so next frame Model reflects it).
  if Model and Widgets then
    local c, r = control_rects()
    s.sim_speed =
      Widgets.slider_update(widget_state, "speed", r.speed, mx, my, clicked, down, Model.sim_speed or 1, c.speed.min, c.speed.max)
    local cur = math.floor((Model.view_mode or 1) + 0.5)
    s.view_mode = Widgets.dropdown_update(widget_state, "view", r.view, mx, my, clicked, #c.view.options, cur)
    s.reset_camera = Widgets.button_update(r.reset, mx, my, clicked)
  end

  -- Tell the engine the pointer is over the HUD so it suppresses the world-pick
  -- on the same press.
  if UI.hud.capture then
    s.ui_capture = point_in(mx, my, UI.hud.capture)
  end
  return s
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

  line(s.title, string.format("hex-world — %.0f hexes (R=%.0f)", Model.hex_count or 0, Model.rings or 0))
  line(
    s.cam,
    string.format(
      "pos (%.0f, %.0f, %.0f)  yaw %.2f  pitch %.2f",
      Model.pos_x or 0,
      Model.pos_y or 0,
      Model.pos_z or 0,
      Model.yaw or 0,
      Model.pitch or 0
    )
  )
  local sel = Model.selected or -1
  if sel >= 0 then
    line(s.sel, string.format("selected hex: %.0f  (click again to close)", sel))
  else
    line(s.sel, "selected hex: none (left-click a hex to inspect)")
  end
  if Model.sim_paused then
    line(s.sim, string.format("sim: PAUSED   t=%.1f", Model.sim_time or 0))
  else
    line(s.sim, string.format("sim: running ×%.2f   t=%.1f", Model.sim_speed or 1, Model.sim_time or 0))
  end
  if math.floor((Model.view_mode or 1) + 0.5) == 2 then
    line(
      s.view,
      string.format("view: Local — focus hex %.0f · %.0f tiles · 1 hex ≈ 49.6 mi", Model.selected or -1, Model.local_tiles or 0)
    )
  else
    line(s.view, "view: World (overview)")
  end
end

-- The view/sim toggle checkboxes, from UI.hud.checkboxes.
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
    cmds[#cmds + 1] = { kind = "rect", x = r.x, y = r.y, w = r.w, h = r.h, r = bc[1], g = bc[2], b = bc[3], a = bc[4] }
    local fill = checked[item.key] and cb.fill_on or cb.fill_off
    cmds[#cmds + 1] =
      { kind = "rect", x = r.x + 2, y = r.y + 2, w = r.w - 4, h = r.h - 4, r = fill[1], g = fill[2], b = fill[3], a = fill[4] }
    local lc = cb.label_color
    cmds[#cmds + 1] =
      { kind = "text", x = r.x + r.w + 10, y = r.y + 1, text = item.label, size = cb.label_size, r = lc[1], g = lc[2], b = lc[3], a = lc[4] }
  end
end

-- The interactive controls: sim-speed slider, reset-camera button, view dropdown.
local function controls(cmds)
  if not (Model and Widgets) then
    return
  end
  local c, r = control_rects()
  local lc = c.label_color
  local function lbl(text, y)
    cmds[#cmds + 1] =
      { kind = "text", x = c.label_x, y = y + 1, text = text, size = c.label_size, r = lc[1], g = lc[2], b = lc[3], a = lc[4] }
  end

  lbl(c.speed.label, r.speed.y)
  Widgets.slider_draw(cmds, r.speed, Model.sim_speed or 1, c.speed.min, c.speed.max, c.slider_style)

  Widgets.button_draw(cmds, r.reset, c.reset.label, c.button_style)

  lbl(c.view.label, r.view.y)
  local cur = math.floor((Model.view_mode or 1) + 0.5)
  Widgets.dropdown_draw(cmds, widget_state, "view", r.view, c.view.options, cur, c.dropdown_style)
end

function M.draw(sw, sh)
  if not UI then
    return {}
  end
  local cmds = {}
  stats(cmds)
  checkboxes(cmds)
  controls(cmds)
  return cmds
end

return M
