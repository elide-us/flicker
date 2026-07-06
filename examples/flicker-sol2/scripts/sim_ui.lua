-- flicker-sol2 simulation HUD: a bottom-right control panel and a top-right
-- stats readout, both pure Lua over the engine `Model` + the `Widgets` toolkit
-- (flicker-ui). Layout/colours live in ui_elements.json (`UI.sim`, `UI.readout`);
-- this script owns behaviour. Widget values flow through the Model (two-way,
-- keyed by id); only transient drag/open state is kept here.
--
--   update(...) -> { <id> = value, ui_capture = bool }
--   draw(sw, sh) -> draw commands (stats overlay + control panel)
--
-- The panel is anchored to the bottom-right *quarter* of the window and the
-- readout to the top-right, both derived from the screen size, so they track
-- the window. The simulation graphics themselves are drawn by the engine (Rust).

local M = {}

-- Transient widget interaction state (slider drag), keyed by widget id.
local ws = {}

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

-- The control panel occupies the bottom-right quarter, inset by the margin.
local function panel_rect(sw, sh)
  local s = UI.sim
  local m = s.margin
  local pw = math.max(sw * 0.5 - m * 1.5, 280)
  local ph = math.max(sh * 0.5 - m * 1.5, 240)
  return { x = sw - pw - m, y = sh - ph - m, w = pw, h = ph }
end

-- Lay out every interactive element inside the panel and return their rects, so
-- update (hit-test) and draw (render) share one geometry.
local function layout(sw, sh)
  local s = UI.sim
  local p = panel_rect(sw, sh)
  local pad = s.pad
  local cx = p.x + pad
  local cw = p.w - pad * 2
  local y = p.y + pad

  local header_y = y
  y = y + s.header_size + 10

  -- Phase nav: three buttons across the top.
  local ph = s.phase
  local pw_each = (cw - ph.gap * 2) / 3
  local phase = {}
  for i, it in ipairs(ph.items) do
    phase[i] = {
      id = it.id, label = it.label, idx = i,
      r = { x = cx + (i - 1) * (pw_each + ph.gap), y = y, w = pw_each, h = ph.row_h },
    }
  end
  y = y + ph.row_h + 12

  -- Dial sliders: label | track | value.
  local sliders = {}
  local sx = cx + s.label_w
  local track_w = cw - s.label_w - s.value_w
  for i, d in ipairs(s.sliders) do
    local row_y = y + (i - 1) * s.row_h
    sliders[i] = {
      def = d, y = row_y, label_x = cx, value_x = sx + track_w + 8,
      r = { x = sx, y = row_y + (s.row_h - s.slider_h) * 0.5, w = track_w, h = s.slider_h },
    }
  end
  y = y + #s.sliders * s.row_h + 8

  -- Toggles: inline checkboxes across.
  local toggles = {}
  local box = 18
  local tw_each = cw / #s.toggles
  for i, t in ipairs(s.toggles) do
    local tx = cx + (i - 1) * tw_each
    toggles[i] = {
      id = t.id, label = t.label, label_x = tx + box + 6, label_y = y + 3,
      r = { x = tx, y = y, w = box, h = box },
    }
  end
  y = y + box + 12

  -- Action buttons: inline across.
  local buttons = {}
  local bw_each = (cw - ph.gap * (#s.buttons - 1)) / #s.buttons
  for i, b in ipairs(s.buttons) do
    buttons[i] = {
      id = b.id, label = b.label,
      r = { x = cx + (i - 1) * (bw_each + ph.gap), y = y, w = bw_each, h = 28 },
    }
  end

  return p, header_y, phase, sliders, toggles, buttons
end

function M.update(mx, my, clicked, sw, sh, down)
  if not (UI and Model and Widgets) then
    return {}
  end
  local s = {}
  local p, _, phase, sliders, toggles, buttons = layout(sw, sh)

  for _, b in ipairs(phase) do
    -- Phase 3 (Planet) is informational only here — it lives in flicker-world.
    if b.id ~= "phase3" then
      s[b.id] = Widgets.button_update(b.r, mx, my, clicked)
    end
  end
  for _, row in ipairs(sliders) do
    local id = row.def.id
    s[id] = Widgets.slider_update(ws, id, row.r, mx, my, clicked, down, Model[id] or 0, row.def.min, row.def.max)
  end
  for _, t in ipairs(toggles) do
    s[t.id] = Widgets.checkbox_update(t.r, mx, my, clicked, Model[t.id] == true)
  end
  for _, b in ipairs(buttons) do
    s[b.id] = Widgets.button_update(b.r, mx, my, clicked)
  end

  s.ui_capture = point_in(mx, my, p)
  return s
end

-- ── drawing ────────────────────────────────────────────────────────────────

local function text(cmds, x, y, str, size, c, align)
  cmds[#cmds + 1] =
    { kind = "text", x = x, y = y, text = str, size = size, align = align, r = c[1], g = c[2], b = c[3], a = c[4] }
end

-- Top-right stats overlay, built from the Model.
local function readout(cmds, sw, sh)
  local r = UI.readout
  local x = sw - r.width - r.margin_x

  -- Collect the lines first so a backing panel can be sized to them. `gap` is extra
  -- space *above* a line (a small section break).
  local lines = {}
  local function add(str, size, c, gap)
    lines[#lines + 1] = { str = str, size = size, c = c, gap = gap or 0 }
  end

  add("flicker · sol2", r.title_size, r.title_color)
  local phase = math.floor((Model.phase or 1) + 0.5)
  add((phase >= 2) and "Phase 2 · Collapse" or "Phase 1 · Distribution", r.head_size, r.phase_color)
  add(string.format("cloud mass %.2f M_sun  ·  metals %.1f%%", Model.mass or 0, Model.metals_pct or 0), r.label_size, r.accent_color, r.gap)
  add(string.format("conserved sum %.3f M_sun", Model.cloud_sum or 0), r.label_size, r.good_color)
  add(string.format("focus %s (%s)  %.1f AU  ·  %d dots", Model.focus_sym or "?", Model.focus_name or "", Model.focus_au or 0, math.floor((Model.candidates or 0) + 0.5)), r.label_size, r.label_color)
  if Model.mass_line and Model.mass_line ~= "" then
    add(Model.mass_line, r.label_size, r.label_color)
  end
  if Model.ignited == true then
    add(string.format("COLLAPSE  ·  t %.0f yr  ·  %d bodies", Model.sim_time or 0, math.floor((Model.bodies or 0) + 0.5)), r.label_size, r.accent_color, r.gap)
    add(string.format("star %.3f  ·  sum %.3f / %.3f M_sun", Model.star_mass or 0, Model.sum_mass or 0, Model.init_mass or 0), r.label_size, r.label_color)
    if Model.type_line and Model.type_line ~= "" then
      add(Model.type_line, r.label_size, r.label_color)
    end
  end

  -- A subtle backing panel keeps the text legible over the busy ring field.
  local total = 0
  for _, ln in ipairs(lines) do
    total = total + ln.gap + ln.size + 4
  end
  if r.bg then
    local pad = 8
    Widgets.panel_draw(cmds, { x = x - pad, y = r.top_y - pad, w = r.width + pad * 2, h = total + pad * 2 }, { bg = r.bg })
  end

  local y = r.top_y
  for _, ln in ipairs(lines) do
    y = y + ln.gap
    text(cmds, x, y, ln.str, ln.size, ln.c)
    y = y + ln.size + 4
  end
end

-- Bottom-right control panel.
local function panel(cmds, sw, sh)
  local s = UI.sim
  local p, header_y, phase, sliders, toggles, buttons = layout(sw, sh)
  Widgets.panel_draw(cmds, p, { bg = s.panel_bg, border = s.panel_border })
  text(cmds, p.x + s.pad, header_y, s.header, s.header_size, s.header_color)

  -- Phase nav: the active phase is lit; a hovered phase warms; Phase 3 is a disabled signpost.
  local ph = s.phase
  local cur = math.floor((Model.phase or 1) + 0.5)
  local hx, hy = Model.mx or -1, Model.my or -1
  for _, b in ipairs(phase) do
    local fill, lc
    if b.id == "phase3" then
      fill, lc = ph.disabled, ph.disabled_label
    elseif b.idx == cur then
      fill, lc = ph.active, ph.active_label
    elseif point_in(hx, hy, b.r) then
      fill, lc = ph.hot or ph.cell, ph.label
    else
      fill, lc = ph.cell, ph.label
    end
    cmds[#cmds + 1] = { kind = "rect", x = b.r.x, y = b.r.y, w = b.r.w, h = b.r.h, r = fill[1], g = fill[2], b = fill[3], a = fill[4] }
    text(cmds, b.r.x + b.r.w * 0.5, b.r.y + (b.r.h - ph.label_size) * 0.5, b.label, ph.label_size, lc, "center")
  end

  -- Dial sliders.
  for _, row in ipairs(sliders) do
    local d = row.def
    local val = Model[d.id] or 0
    text(cmds, row.label_x, row.y + 1, d.label, s.label_size, s.label_color)
    Widgets.slider_draw(cmds, row.r, val, d.min, d.max, s.slider_style)
    text(cmds, row.value_x, row.y + 1, string.format(d.fmt or "%.2f", val), s.value_size, s.value_color)
  end

  -- Toggles (checkboxes).
  for _, t in ipairs(toggles) do
    Widgets.checkbox_draw(cmds, t.r, Model[t.id] == true, s.checkbox_style)
    text(cmds, t.label_x, t.label_y, t.label, s.label_size, s.label_color)
  end

  -- Action buttons (hover-highlighted).
  for _, b in ipairs(buttons) do
    Widgets.button_draw(cmds, b.r, b.label, s.button_style, point_in(hx, hy, b.r))
  end
end

function M.draw(sw, sh)
  if not (UI and Model and Widgets) then
    return {}
  end
  local cmds = {}
  readout(cmds, sw, sh)
  panel(cmds, sw, sh)
  return cmds
end

return M
