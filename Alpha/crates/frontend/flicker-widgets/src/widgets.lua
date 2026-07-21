-- widgets.lua: reusable immediate-mode UI widgets for flicker Lua screens.
--
-- Embedded in `flicker-ui` (`include_str!`) and loaded by a host as the
-- `Widgets` global via `flicker_ui::load_widgets` (ScriptHost::set_lua_module).
-- Each widget is split into an `*_update` (hit-test + interaction, called from
-- the script's `update`) and an `*_draw` (emit commands, called from `draw`).
--
-- Values are NOT stored here: they live in the engine `Model` (two-way) — the
-- update returns the new value, the host applies it, and next frame `Model`
-- carries it for the draw. The only per-widget state the script must keep is
-- transient interaction (a slider's drag flag, a dropdown's open flag), passed
-- in via a `state` table keyed by a stable widget id.
--
-- Styles (colours/sizes) come from ui_elements.json — no hardcoded palette.

local W = {}

local function clamp(v, lo, hi)
  if v < lo then
    return lo
  elseif v > hi then
    return hi
  else
    return v
  end
end

local function point_in(px, py, x, y, w, h)
  return px >= x and px <= x + w and py >= y and py <= y + h
end

local function rgba(c)
  return c[1], c[2], c[3], c[4]
end

local function rect(cmds, x, y, w, h, c, layer)
  local r, g, b, a = rgba(c)
  cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = r, g = g, b = b, a = a, layer = layer }
end

-- `font` (optional) = the Prism face role: "display" (Cormorant, titles/names),
-- "label" (Cinzel, caps), or nil/"body" (EB Garamond, the default prose face).
local function label(cmds, x, y, str, size, c, align, layer, font)
  local r, g, b, a = rgba(c)
  cmds[#cmds + 1] =
    { kind = "text", x = x, y = y, text = str, size = size, align = align, font = font, r = r, g = g, b = b, a = a, layer = layer }
end

-- SLIDER -------------------------------------------------------------------
-- A horizontal track + draggable handle mapping x → [min, max]. Press anywhere
-- on the track to grab; drag while held; release to drop.
function W.slider_update(state, id, r, mx, my, clicked, down, value, min, max)
  if clicked and point_in(mx, my, r.x, r.y - 6, r.w, r.h + 12) then
    state[id] = true
  end
  if not down then
    state[id] = nil
  end
  if state[id] then
    value = min + clamp((mx - r.x) / r.w, 0, 1) * (max - min)
  end
  return value
end

-- `ticks` (optional): draw an N-division ruler behind the handle — N+1 marks
-- across the track, the two ends slightly taller. Older callers omit it and
-- get a plain slider. Tick colour is `s.tick` (falls back to the handle).
-- Prism material (all optional, gated on the style field so older callers are
-- unaffected): `s.fill_hi` = a 1px lit sheen along the top of the fill (the
-- carved sapphire/resource glow); `s.handle_hi` = a lit top edge on the handle.
function W.slider_draw(cmds, r, value, min, max, s, ticks)
  local t = clamp((value - min) / (max - min), 0, 1)
  rect(cmds, r.x, r.y, r.w, r.h, s.track)
  local fw = r.w * t
  rect(cmds, r.x, r.y, fw, r.h, s.fill)
  if s.fill_hi and fw > 0 then
    rect(cmds, r.x, r.y, fw, 1, s.fill_hi)
  end
  if ticks and ticks > 0 then
    local tc = s.tick or s.handle
    for i = 0, ticks do
      local edge = (i == 0 or i == ticks)
      local th = r.h + (edge and 12 or 6)
      rect(cmds, r.x + r.w * (i / ticks) - 1, r.y - (th - r.h) * 0.5, 2, th, tc)
    end
  end
  local hw = s.handle_w
  local hx = r.x + r.w * t - hw * 0.5
  rect(cmds, hx, r.y - 4, hw, r.h + 8, s.handle)
  if s.handle_hi then
    rect(cmds, hx, r.y - 4, hw, 1, s.handle_hi)
  end
end

-- STEPPER (numeric value box) ---------------------------------------------
-- A `[-] value [+]` box. Clicking a square end button steps by `step`,
-- clamped to [min, max]. (Keyboard text entry is a planned enhancement.)
function W.stepper_update(r, mx, my, clicked, value, step, min, max)
  if clicked then
    local bw = r.h
    if point_in(mx, my, r.x, r.y, bw, r.h) then
      value = value - step
    elseif point_in(mx, my, r.x + r.w - bw, r.y, bw, r.h) then
      value = value + step
    end
    value = clamp(value, min, max)
  end
  return value
end

function W.stepper_draw(cmds, r, value, s, fmt)
  local bw = r.h
  rect(cmds, r.x, r.y, r.w, r.h, s.box)
  rect(cmds, r.x, r.y, bw, r.h, s.btn)
  rect(cmds, r.x + r.w - bw, r.y, bw, r.h, s.btn)
  label(cmds, r.x + bw * 0.5, r.y + 2, "-", s.label_size, s.label, "center")
  label(cmds, r.x + r.w - bw * 0.5, r.y + 2, "+", s.label_size, s.label, "center")
  label(cmds, r.x + r.w * 0.5, r.y + 2, string.format(fmt or "%.0f", value), s.label_size, s.label, "center")
end

-- DROPDOWN -----------------------------------------------------------------
-- A header showing the current option; click to open the list, click an item
-- to select (returns its 1-based index), any click closes. The open list draws
-- at `layer = 1` (relative to the screen) so it covers whatever is beneath it.
function W.dropdown_update(state, id, r, mx, my, clicked, count, selected)
  if clicked then
    if state[id] then
      for i = 1, count do
        local iy = r.y + r.h * i
        if point_in(mx, my, r.x, iy, r.w, r.h) then
          selected = i
        end
      end
      state[id] = nil
    elseif point_in(mx, my, r.x, r.y, r.w, r.h) then
      state[id] = true
    end
  end
  return selected
end

function W.dropdown_draw(cmds, state, id, r, options, selected, s)
  rect(cmds, r.x, r.y, r.w, r.h, s.cell)
  label(cmds, r.x + 8, r.y + 3, options[selected], s.label_size, s.label, "left")
  if state[id] then
    for i = 1, #options do
      local iy = r.y + r.h * i
      local fill = (i == selected) and s.hot or s.cell
      rect(cmds, r.x, iy, r.w, r.h, fill, 1)
      label(cmds, r.x + 8, iy + 3, options[i], s.label_size, s.label, "left", 1)
    end
  end
end

-- BUTTON -------------------------------------------------------------------
-- A momentary push button: returns `true` on the frame it is clicked. Stateless
-- (no `state` table needed) — the caller treats the return as a one-shot action.
function W.button_update(r, mx, my, clicked)
  return clicked and point_in(mx, my, r.x, r.y, r.w, r.h)
end

-- Prism material (all optional, gated): `s.glow` = a soft sapphire halo drawn
-- behind (primary emphasis); `s.shadow` = a drop shadow under the slab;
-- `s.border` = a 1px frame; `s.hi` = a lit top edge just inside the frame.
function W.button_draw(cmds, r, text, s, hot)
  if s.glow then
    rect(cmds, r.x - 3, r.y - 3, r.w + 6, r.h + 6, s.glow)
  end
  if s.shadow then
    rect(cmds, r.x, r.y + 2, r.w, r.h, s.shadow)
  end
  local fill = hot and s.hot or s.cell
  rect(cmds, r.x, r.y, r.w, r.h, fill)
  if s.border then
    rect(cmds, r.x, r.y, r.w, 1, s.border)
    rect(cmds, r.x, r.y + r.h - 1, r.w, 1, s.border)
    rect(cmds, r.x, r.y, 1, r.h, s.border)
    rect(cmds, r.x + r.w - 1, r.y, 1, r.h, s.border)
  end
  if s.hi then
    rect(cmds, r.x + 1, r.y + 1, r.w - 2, 1, s.hi)
  end
  label(cmds, r.x + r.w * 0.5, r.y + (r.h - s.label_size) * 0.5, text, s.label_size, s.label, "center", nil, "label")
end

-- SECTION HEADER -----------------------------------------------------------
-- A full-width label bar marking a subsection (e.g. "KEYBOARD", "MOUSE",
-- "CONTROLLER"). Stateless — just draws a tinted rect + centered text.
function W.section_header_draw(cmds, r, text, s)
  rect(cmds, r.x, r.y, r.w, r.h, s.bg)
  label(cmds, r.x + 8, r.y + (r.h - s.label_size) * 0.5, text, s.label_size, s.label, "left", nil, "label")
end

-- CHECKBOX -----------------------------------------------------------------
-- A momentary toggle box: returns the *new* boolean state, flipping `value`
-- only on the frame the box is clicked. Stateless (the value lives in the
-- engine Model, like every other widget). `r` is the box rect.
function W.checkbox_update(r, mx, my, clicked, value)
  if clicked and point_in(mx, my, r.x, r.y, r.w, r.h) then
    return not value
  end
  return value
end

-- `s` = { box, check, border?, pad? }. Draws the box, an optional 1px border,
-- and an inset filled mark when `value` is true.
function W.checkbox_draw(cmds, r, value, s)
  rect(cmds, r.x, r.y, r.w, r.h, s.box)
  if s.border then
    rect(cmds, r.x, r.y, r.w, 1, s.border)
    rect(cmds, r.x, r.y + r.h - 1, r.w, 1, s.border)
    rect(cmds, r.x, r.y, 1, r.h, s.border)
    rect(cmds, r.x + r.w - 1, r.y, 1, r.h, s.border)
  end
  if value then
    local p = s.pad or 4
    rect(cmds, r.x + p, r.y + p, r.w - 2 * p, r.h - 2 * p, s.check)
  end
end

-- PANEL --------------------------------------------------------------------
-- A filled background rect with an optional 1px frame — the common backing for
-- a control cluster (stats overlay, knob panel). Stateless. `s` = { bg,
-- border? }. Draw it first, then place widgets/labels on top.
function W.panel_draw(cmds, r, s)
  rect(cmds, r.x, r.y, r.w, r.h, s.bg)
  if s.border then
    rect(cmds, r.x, r.y, r.w, 1, s.border)
    rect(cmds, r.x, r.y + r.h - 1, r.w, 1, s.border)
    rect(cmds, r.x, r.y, 1, r.h, s.border)
    rect(cmds, r.x + r.w - 1, r.y, 1, r.h, s.border)
  end
end

-- LAYOUT (rect-yielding helpers) -------------------------------------------
-- Pure geometry: turn a box + gaps into child rects so panels stop being
-- hand-placed absolute pixels. No interaction, no drawing — feed the rects to
-- the widgets. `r` = { x, y, w, h? }.
function W.layout_rows(r, row_h, gap, n)
  local out = {}
  for i = 1, n do
    out[i] = { x = r.x, y = r.y + (i - 1) * (row_h + gap), w = r.w, h = row_h }
  end
  return out
end

function W.layout_grid(r, cols, row_h, gap_x, gap_y, n)
  local out = {}
  local cw = (r.w - (cols - 1) * gap_x) / cols
  for i = 0, n - 1 do
    local c = i % cols
    local row = math.floor(i / cols)
    out[i + 1] = { x = r.x + c * (cw + gap_x), y = r.y + row * (row_h + gap_y), w = cw, h = row_h }
  end
  return out
end

-- RADIO / SEGMENTED --------------------------------------------------------
-- N side-by-side cells, one active. Click a cell to select it (returns its
-- 1-based index); a cell whose `disabled[i]` is set can't be picked. Generalises
-- the one-shot phase nav. Stateless (selection lives in Model).
function W.radio_update(r, mx, my, clicked, count, selected, disabled)
  if clicked then
    local cw = r.w / count
    for i = 1, count do
      if point_in(mx, my, r.x + (i - 1) * cw, r.y, cw, r.h) and not (disabled and disabled[i]) then
        selected = i
      end
    end
  end
  return selected
end

-- `s` = { cell, active, label, label_size, active_label?, disabled?, disabled_label? }.
function W.radio_draw(cmds, r, options, selected, s, disabled)
  local cw = r.w / #options
  for i = 1, #options do
    local x = r.x + (i - 1) * cw
    local off = disabled and disabled[i]
    local fill = off and (s.disabled or s.cell) or (i == selected and s.active or s.cell)
    rect(cmds, x, r.y, cw - 1, r.h, fill)
    local lc = off and (s.disabled_label or s.label) or (i == selected and (s.active_label or s.label) or s.label)
    label(cmds, x + cw * 0.5, r.y + (r.h - s.label_size) * 0.5, options[i], s.label_size, lc, "center", nil, "label")
  end
end

-- LEGEND / COLORBAR --------------------------------------------------------
-- A vertical key of colour swatches → labels for the active field view.
-- `stops` = { { color = {r,g,b,a}, label = "..." }, ... }. Pure drawing.
-- `s` = { swatch?, row_h?, label, label_size }.
function W.legend_draw(cmds, r, stops, s)
  local rh = s.row_h or 18
  local sw = s.swatch or 14
  for i = 1, #stops do
    local y = r.y + (i - 1) * rh
    rect(cmds, r.x, y, sw, sw, stops[i].color)
    label(cmds, r.x + sw + 6, y + (sw - s.label_size) * 0.5, stops[i].label, s.label_size, s.label, "left")
  end
end

-- GAUGE (green-zone bar with a live marker) --------------------------------
-- A read-only condition gauge: a track with a green "habitable band" in the middle
-- ([lo, hi] as 0..1 fractions of the track) and a marker pinned at `value` (0..1). The
-- marker is coloured `marker_in` when inside the band, else `marker`. A `nil`/negative
-- `value` means "no signal yet" — the bar is greyed with `no_signal` and no marker drawn.
-- Pure drawing; the value + bounds live in the engine Model (the habitability observer).
-- `s` = { track, band, marker, marker_in?, no_signal?, marker_w?, sheen? }.
-- `s.sheen` (optional) = a 1px lit line along the top of the track (carved-stone rim).
function W.gauge_draw(cmds, r, value, lo, hi, s)
  rect(cmds, r.x, r.y, r.w, r.h, s.track)
  -- the habitable band (green zone in the middle of the bar)
  rect(cmds, r.x + r.w * lo, r.y, r.w * (hi - lo), r.h, s.band)
  if s.sheen then
    rect(cmds, r.x, r.y, r.w, 1, s.sheen)
  end
  if value == nil or value < 0 then
    if s.no_signal then
      rect(cmds, r.x, r.y, r.w, r.h, s.no_signal)
    end
    return
  end
  local t = clamp(value, 0, 1)
  local inb = value >= lo and value <= hi
  local mc = inb and (s.marker_in or s.marker) or s.marker
  local mw = s.marker_w or 4
  rect(cmds, r.x + r.w * t - mw * 0.5, r.y - 3, mw, r.h + 6, mc)
end

-- LIST (fixed-height select / toggle rows) ---------------------------------
-- A vertical list of rows; click a row to act on it (returns the clicked 1-based
-- index this frame, else 0). Fixed height — scrolling is deferred boundary work
-- (needs a Model.scroll or a clip command). `items` = { { label, on? }, ... };
-- a row with `on` (bool) draws a toggle box, otherwise a plain selectable row.
function W.list_update(r, mx, my, clicked, count, row_h)
  if clicked then
    for i = 1, count do
      if point_in(mx, my, r.x, r.y + (i - 1) * row_h, r.w, row_h) then
        return i
      end
    end
  end
  return 0
end

-- `s` = { cell, hot, label, label_size, mark_on?, mark_off? }.
function W.list_draw(cmds, r, items, row_h, s, selected)
  for i = 1, #items do
    local y = r.y + (i - 1) * row_h
    local it = items[i]
    rect(cmds, r.x, y, r.w, row_h - 1, (i == selected) and s.hot or s.cell)
    if it.on ~= nil then
      local bs = row_h - 8
      rect(cmds, r.x + 4, y + 4, bs, bs, it.on and (s.mark_on or s.label) or (s.mark_off or s.cell))
      label(cmds, r.x + bs + 12, y + (row_h - s.label_size) * 0.5, it.label, s.label_size, s.label, "left")
    else
      label(cmds, r.x + 8, y + (row_h - s.label_size) * 0.5, it.label, s.label_size, s.label, "left")
    end
  end
end

-- TIMELINE (evolution scrubber) --------------------------------------------
-- A duration-weighted segment per epoch with a draggable 0..1 playhead and
-- optional group bands above. `segs` = { { label, edge } } where `edge` is the
-- cumulative RIGHT boundary in 0..1 (last = 1); `bands` (optional) is the same,
-- drawn as a thin strip above; `active` is the 1-based active epoch. Drag works
-- like the slider (press the bar to grab, drag while held). Playhead lives in Model.
function W.timeline_update(state, id, r, mx, my, clicked, down, playhead)
  if clicked and point_in(mx, my, r.x, r.y - 6, r.w, r.h + 12) then
    state[id] = true
  end
  if not down then
    state[id] = nil
  end
  if state[id] then
    playhead = clamp((mx - r.x) / r.w, 0, 1)
  end
  return playhead
end

-- `s` = { seg_even, seg_odd, seg_active, num_color, active_label_color?, playhead,
--         playhead_w?, label_size?, band?, band2?, band_h?, band_label?, band_label_size? }.
function W.timeline_draw(cmds, r, segs, playhead, s, bands, active)
  local lsz = s.label_size or 12
  if bands then
    local bh = s.band_h or 8
    local prev = 0
    for i = 1, #bands do
      local x0 = r.x + r.w * prev
      local x1 = r.x + r.w * bands[i].edge
      rect(cmds, x0, r.y - bh - 2, x1 - x0 - 1, bh, (i % 2 == 1) and s.band or (s.band2 or s.band))
      if bands[i].label then
        label(cmds, (x0 + x1) * 0.5, r.y - bh - 2, bands[i].label, s.band_label_size or lsz, s.band_label or s.num_color, "center")
      end
      prev = bands[i].edge
    end
  end
  local prev = 0
  for i = 1, #segs do
    local x0 = r.x + r.w * prev
    local x1 = r.x + r.w * segs[i].edge
    local fill = (i == active) and s.seg_active or ((i % 2 == 1) and s.seg_even or s.seg_odd)
    rect(cmds, x0, r.y, x1 - x0 - 1, r.h, fill)
    if segs[i].label then
      local lc = (i == active) and (s.active_label_color or s.num_color) or s.num_color
      label(cmds, (x0 + x1) * 0.5, r.y + (r.h - lsz) * 0.5, segs[i].label, lsz, lc, "center")
    end
    prev = segs[i].edge
  end
  local pw = s.playhead_w or 3
  rect(cmds, r.x + r.w * playhead - pw * 0.5, r.y - 4, pw, r.h + 8, s.playhead)
end

return W
