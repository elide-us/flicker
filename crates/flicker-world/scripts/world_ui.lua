-- flicker-world HUD: planet stats, view/grid/epoch controls, and a per-epoch
-- knob panel — driven by Lua + `ui_elements.json` (the `UI.world` section) and the
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
  return { x = UI.world.controls.widget_x, y = w.y, w = w.w, h = w.h }
end

local function current_epoch()
  return math.floor((Model.epoch or 6) + 0.5)
end

-- The view dropdown is filtered per epoch: each epoch lists only the views that
-- mean something for it (UI.world.epochs[ep].views, by name). The engine
-- Model.view_mode stays a *global* 1-based index into UI.world.controls.view.options
-- (the Rust ViewMode order); these helpers translate between that index and the
-- per-epoch list shown in the dropdown.
local function epoch_views()
  local ep = UI.world.epochs[current_epoch()]
  if ep and ep.views and #ep.views > 0 then
    return ep.views
  end
  return UI.world.controls.view.options
end

local function view_global_index(name)
  for i, opt in ipairs(UI.world.controls.view.options) do
    if opt == name then return i end
  end
  return 1
end

-- Position of the current (global) view within the per-epoch list, or 1 if the
-- current view isn't offered this epoch.
local function view_local_pos(views)
  local cur = math.floor((Model.view_mode or 1) + 0.5)
  local name = UI.world.controls.view.options[cur]
  for i, v in ipairs(views) do
    if v == name then return i end
  end
  return 1
end

-- The selected epoch's param rows, each with its slider rect.
local function param_rows()
  local p = UI.world.params
  local ep = UI.world.epochs[current_epoch()]
  local rows = {}
  if ep then
    local y = p.origin_y
    for _, pr in ipairs(ep.params) do
      rows[#rows + 1] = {
        def = pr,
        y = y,
        r = { x = p.widget_x, y = y + (p.row_h - 13) * 0.5, w = p.slider_w, h = 13 },
      }
      y = y + p.row_h
    end
  end
  return p, rows
end

-- The element-mix grid (Epoch 1): a 2-column grid of compact sliders, each
-- emitting/reading `ab_<symbol>`.
local function element_rows()
  local e = UI.world.elements
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
        r = { x = x + e.label_w, y = y + (e.row_h - 11) * 0.5, w = e.slider_w, h = 11 },
      }
    end
  end
  return e, rows
end

-- The celestial (SKY) panel, right-anchored to the screen width: each row a
-- slider keyed by its CelestialState id. Returns the config, the row rects, the
-- panel-left x, and a capture rect (so dragging a slider doesn't also orbit).
local function celestial_rows(sw)
  local cz = UI.world.celestial
  if not cz then
    return nil, {}, 0, nil
  end
  local panel_x = sw - cz.margin_x - cz.width
  local rows = {}
  local y = cz.rows_y
  for _, r in ipairs(cz.rows) do
    rows[#rows + 1] = {
      def = r,
      x = panel_x,
      y = y,
      r = { x = panel_x + cz.slider_x_off, y = y + (cz.row_h - 14) * 0.5, w = cz.slider_w, h = 14 },
    }
    y = y + cz.row_h
  end
  local cap = { x = panel_x - 8, y = cz.top_y - 4, w = cz.width + cz.margin_x + 8, h = (y - cz.top_y) + 8 }
  return cz, rows, panel_x, cap
end

-- The bottom evolution-timeline bar: full width minus margins, ~1 inch tall.
-- Returns the config and its screen rect (the scrub track).
local function timeline_bar(sw, sh)
  local t = UI.world.timeline
  if not t then
    return nil, nil
  end
  local rect = {
    x = t.margin_x,
    y = sh - t.margin_bottom - t.height,
    w = sw - 2 * t.margin_x,
    h = t.height,
  }
  return t, rect
end

function M.update(mx, my, clicked, sw, sh, down)
  if not (UI and Model and Widgets) then
    return {}
  end
  local c = UI.world.controls
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

  local _, crows, _, ccap = celestial_rows(sw)
  for _, row in ipairs(crows) do
    local id = row.def.id
    s[id] = Widgets.slider_update(ws, id, row.r, mx, my, clicked, down, Model[id] or 0, row.def.min, row.def.max)
  end

  -- The bottom timeline scrubs as one wide 0..1 slider (drag = scrub the movie).
  local _, tlrect = timeline_bar(sw, sh)
  if tlrect then
    s.timeline = Widgets.slider_update(ws, "timeline", tlrect, mx, my, clicked, down, Model.timeline or 0, 0, 1)
  end

  local capture = UI.world.capture and point_in(mx, my, UI.world.capture)
  if ccap and point_in(mx, my, ccap) then
    capture = true
  end
  if tlrect and point_in(mx, my, tlrect) then
    capture = true
  end
  s.ui_capture = capture or false
  return s
end

local function stats(cmds)
  local s = UI.world.stats
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
  local c = UI.world.controls
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
  local ep = UI.world.epochs[current_epoch()]
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
    Widgets.slider_draw(cmds, row.r, val, 0, e.max, UI.world.params.slider_style)
  end
end

local function celestial(cmds, sw)
  local cz, rows, panel_x = celestial_rows(sw)
  if not cz then
    return
  end
  local hc = cz.header_color
  cmds[#cmds + 1] =
    { kind = "text", x = panel_x, y = cz.top_y, text = cz.title, size = cz.header_size, r = hc[1], g = hc[2], b = hc[3], a = hc[4] }
  local lc = cz.label_color
  local vc = cz.value_color
  for _, row in ipairs(rows) do
    local d = row.def
    local val = Model[d.id] or 0
    cmds[#cmds + 1] =
      { kind = "text", x = row.x, y = row.y, text = d.label, size = cz.label_size, r = lc[1], g = lc[2], b = lc[3], a = lc[4] }
    Widgets.slider_draw(cmds, row.r, val, d.min, d.max, cz.slider_style)
    cmds[#cmds + 1] = {
      kind = "text",
      x = panel_x + cz.value_x_off,
      y = row.y,
      text = string.format(d.fmt or "%.2f", val),
      size = cz.value_size,
      r = vc[1], g = vc[2], b = vc[3], a = vc[4],
    }
  end
end

-- The bottom evolution timeline: one labelled segment per epoch, with the
-- playhead at Model.timeline (0..1). The whole planet's history at a glance.
local function timeline(cmds, sw, sh)
  local t, r = timeline_bar(sw, sh)
  if not t then
    return
  end
  local epochs = UI.world.epochs or {}
  local n = #epochs
  if n == 0 then
    return
  end
  local function rect(x, y, w, h, c)
    cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = c[1], g = c[2], b = c[3], a = c[4] }
  end
  -- Frame + background.
  rect(r.x - 2, r.y - 2, r.w + 4, r.h + 4, t.border)
  rect(r.x, r.y, r.w, r.h, t.bg)
  cmds[#cmds + 1] =
    { kind = "text", x = r.x + 10, y = r.y + 6, text = t.title, size = t.title_size, r = t.title_color[1], g = t.title_color[2], b = t.title_color[3], a = t.title_color[4] }

  local cur = current_epoch() -- 1-based
  local seg_top = r.y + 26
  local seg_h = r.h - 26
  local label_y = r.y + r.h - 30
  -- Segment edges are duration-weighted (published as Model.tl_b1..tl_b{n-1});
  -- fall back to equal widths if absent.
  local function edge(i)
    if i <= 0 then return 0 end
    if i >= n then return 1 end
    return Model["tl_b" .. i] or (i / n)
  end
  for i = 1, n do
    local left = edge(i - 1)
    local sx = r.x + left * r.w
    local sw_i = (edge(i) - left) * r.w
    local fill = (i % 2 == 0) and t.seg_even or t.seg_odd
    if i == cur then
      fill = t.seg_active
    end
    rect(sx, seg_top, sw_i, seg_h, fill)
    if i > 1 then
      rect(sx, seg_top, 1, seg_h, t.divider)
    end
    cmds[#cmds + 1] =
      { kind = "text", x = sx + 6, y = seg_top + 2, text = tostring(i), size = t.num_size, r = t.num_color[1], g = t.num_color[2], b = t.num_color[3], a = t.num_color[4] }
    local lc = (i == cur) and t.active_label_color or t.label_color
    cmds[#cmds + 1] =
      { kind = "text", x = sx + 6, y = label_y, text = epochs[i].label, size = t.label_size, r = lc[1], g = lc[2], b = lc[3], a = lc[4] }
  end
  -- Playhead.
  local px = r.x + (Model.timeline or 0) * r.w
  rect(px - t.playhead_w * 0.5, seg_top - 2, t.playhead_w, seg_h + 2, t.playhead)
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
    celestial(cmds, sw)
    timeline(cmds, sw, sh)
  end
  return cmds
end

return M
