-- flicker-world HUD: planet stats, view/grid/epoch controls, and a per-epoch
-- knob panel — driven by Lua + `ui_elements.json` (the `UI.hud` section) and the
-- engine `Model` (live values published each frame). Layout/colours/labels live
-- in the JSON; this script owns behaviour. The reusable `Widgets` toolkit
-- (stepper / button / dropdown / slider) is from flicker-ui; widget values live
-- in the engine Model (two-way, keyed by param id), only transient open/drag
-- state is kept here.

local M = {}

-- Transient widget state (dropdown open, slider drag), keyed by widget id.
local ws = {}

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

local function ctrl_rect(w)
  return { x = UI.hud.controls.widget_x, y = w.y, w = w.w, h = w.h }
end

local function current_epoch()
  return math.floor((Model.epoch or 6) + 0.5)
end

-- The view dropdown is filtered per epoch: each epoch lists only the views that
-- mean something for it (UI.hud.epochs[ep].views, by name). The engine
-- Model.view_mode stays a *global* 1-based index into UI.hud.controls.view.options
-- (the Rust ViewMode order); these helpers translate between that index and the
-- per-epoch list shown in the dropdown.
local function epoch_views()
  local ep = UI.hud.epochs[current_epoch()]
  if ep and ep.views and #ep.views > 0 then
    return ep.views
  end
  return UI.hud.controls.view.options
end

local function view_global_index(name)
  for i, opt in ipairs(UI.hud.controls.view.options) do
    if opt == name then return i end
  end
  return 1
end

-- Position of the current (global) view within the per-epoch list, or 1 if the
-- current view isn't offered this epoch.
local function view_local_pos(views)
  local cur = math.floor((Model.view_mode or 1) + 0.5)
  local name = UI.hud.controls.view.options[cur]
  for i, v in ipairs(views) do
    if v == name then return i end
  end
  return 1
end

-- The selected epoch's param rows, each with its slider rect.
local function param_rows()
  local p = UI.hud.params
  local ep = UI.hud.epochs[current_epoch()]
  local rows = {}
  if ep then
    local y = p.origin_y
    for _, pr in ipairs(ep.params) do
      rows[#rows + 1] = {
        def = pr,
        y = y,
        r = { x = p.widget_x, y = y + (p.row_h - 8) * 0.5, w = p.slider_w, h = 8 },
      }
      y = y + p.row_h
    end
  end
  return p, rows
end

-- The element-mix grid (Epoch 1): a 2-column grid of compact sliders, each
-- emitting/reading `ab_<symbol>`.
local function element_rows()
  local e = UI.hud.elements
  local rows = {}
  if e then
    for i, item in ipairs(e.items) do
      local col = (i - 1) % e.cols
      local row = math.floor((i - 1) / e.cols)
      local x = e.col_x[col + 1]
      local y = e.origin_y + row * e.row_h
      rows[#rows + 1] = {
        sym = item.sym,
        x = x,
        y = y,
        r = { x = x + e.label_w, y = y + (e.row_h - 7) * 0.5, w = e.slider_w, h = 7 },
      }
    end
  end
  return e, rows
end

function M.update(mx, my, clicked, sw, sh, down)
  if not (UI and Model and Widgets) then
    return {}
  end
  local c = UI.hud.controls
  local s = {}

  local views = epoch_views()
  local view_pos = Widgets.dropdown_update(
    ws, "view", ctrl_rect(c.view), mx, my, clicked, #views, view_local_pos(views))
  s.view_mode = view_global_index(views[view_pos])
  s.freq = Widgets.stepper_update(ctrl_rect(c.freq), mx, my, clicked, Model.freq or 24, c.freq.step, c.freq.min, c.freq.max)
  s.epoch = Widgets.stepper_update(ctrl_rect(c.epoch), mx, my, clicked, Model.epoch or 6, c.epoch.step, c.epoch.min, c.epoch.max)
  s.reseed = Widgets.button_update(ctrl_rect(c.reseed), mx, my, clicked)

  local _, rows = param_rows()
  for _, row in ipairs(rows) do
    local id = row.def.id
    s[id] = Widgets.slider_update(ws, id, row.r, mx, my, clicked, down, Model[id] or 0, row.def.min, row.def.max)
  end

  if current_epoch() == 1 then
    local e, erows = element_rows()
    if e then
      for _, row in ipairs(erows) do
        local id = "ab_" .. row.sym
        s[id] = Widgets.slider_update(ws, id, row.r, mx, my, clicked, down, Model[id] or 0, 0, e.max)
      end
    end
  end

  if UI.hud.capture then
    s.ui_capture = point_in(mx, my, UI.hud.capture)
  end
  return s
end

local function stats(cmds)
  local s = UI.hud.stats
  local function line(spec, text)
    local cc = spec.color
    cmds[#cmds + 1] = { kind = "text", x = s.x, y = spec.y, text = text, size = spec.size, r = cc[1], g = cc[2], b = cc[3], a = cc[4] }
  end
  line(s.title, "flicker · world")
  line(s.cells, string.format("cells %.0f   freq %.0f   plates %.0f   basins %.0f", Model.cells or 0, Model.freq or 0, Model.plates or 0, Model.basins or 0))
  line(s.seed, "seed " .. (Model.seed or "?"))
  line(s.view, "field: " .. (Model.view_label or ""))
end

local function controls(cmds)
  local c = UI.hud.controls
  local lc = c.label_color
  local function lbl(text, y)
    cmds[#cmds + 1] = { kind = "text", x = c.label_x, y = y + 4, text = text, size = c.label_size, r = lc[1], g = lc[2], b = lc[3], a = lc[4] }
  end

  lbl(c.freq.label, c.freq.y)
  Widgets.stepper_draw(cmds, ctrl_rect(c.freq), Model.freq or 24, c.stepper_style, "%.0f")
  lbl(c.epoch.label, c.epoch.y)
  Widgets.stepper_draw(cmds, ctrl_rect(c.epoch), Model.epoch or 6, c.stepper_style, "%.0f")
  Widgets.button_draw(cmds, ctrl_rect(c.reseed), c.reseed.label, c.button_style)
  lbl(c.view.label, c.view.y)
  local views = epoch_views()
  Widgets.dropdown_draw(
    cmds, ws, "view", ctrl_rect(c.view), views, view_local_pos(views), c.dropdown_style)
end

local function params(cmds)
  local p, rows = param_rows()
  local ep = UI.hud.epochs[current_epoch()]
  local hc = p.header_color
  cmds[#cmds + 1] = {
    kind = "text",
    x = p.label_x,
    y = p.header_y,
    text = string.format("EPOCH %d — %s", current_epoch(), (ep and ep.label) or ""),
    size = p.header_size,
    r = hc[1], g = hc[2], b = hc[3], a = hc[4],
  }
  local lc = p.label_color
  local vc = p.value_color
  for _, row in ipairs(rows) do
    local pr = row.def
    local val = Model[pr.id] or 0
    cmds[#cmds + 1] =
      { kind = "text", x = p.label_x, y = row.y, text = pr.label, size = p.label_size, r = lc[1], g = lc[2], b = lc[3], a = lc[4] }
    Widgets.slider_draw(cmds, row.r, val, pr.min, pr.max, p.slider_style)
    cmds[#cmds + 1] = {
      kind = "text",
      x = p.widget_x + p.slider_w + 8,
      y = row.y,
      text = string.format(pr.fmt or "%.2f", val),
      size = p.value_size,
      r = vc[1], g = vc[2], b = vc[3], a = vc[4],
    }
  end
end

local function elements(cmds)
  if current_epoch() ~= 1 then
    return
  end
  local e, rows = element_rows()
  if not e then
    return
  end
  local hc = e.header_color
  cmds[#cmds + 1] =
    { kind = "text", x = e.col_x[1], y = e.header_y, text = e.header, size = e.header_size, r = hc[1], g = hc[2], b = hc[3], a = hc[4] }
  local lc = e.label_color
  for _, row in ipairs(rows) do
    local val = Model["ab_" .. row.sym] or 0
    cmds[#cmds + 1] =
      { kind = "text", x = row.x, y = row.y, text = row.sym, size = e.label_size, r = lc[1], g = lc[2], b = lc[3], a = lc[4] }
    Widgets.slider_draw(cmds, row.r, val, 0, e.max, UI.hud.params.slider_style)
  end
end

function M.draw(sw, sh)
  if not (UI and Model) then
    return {}
  end
  local cmds = {}
  stats(cmds)
  if Widgets then
    controls(cmds)
    params(cmds)
    elements(cmds)
  end
  return cmds
end

return M
