-- Unified settings screen: Audio / Video / Input tabs.
--
-- Driven by `UI.settings` (from ui_elements.json). Current values flow in via
-- `Model` (set by the host each frame); changes flow out as momentary actions
-- or value updates in the return table. The host persists to `settings.json`.
--
-- Model keys are flat (e.g. `Model.audio_master`) since ValueMap only supports
-- scalars. The script reads them with `Model["key"]` syntax.

local M = {}

-- ── state ──────────────────────────────────────────────────────────
local active_tab   = 1           -- 1=Audio, 2=Video, 3=Input
local input_subtab = 1           -- 1=Keyboard, 2=Mouse, 3=Controller (within Input tab)
local hover        = nil         -- hovered item index
local widget_state = {}          -- transient widget state (slider drags, dropdown open flags)
local rebind_action = nil        -- action id currently being rebound (string) or nil
local rebind_for_gamepad = false -- true when rebinding a gamepad action
local scroll_offset = 0.0        -- scroll offset for keyboard/controller binding lists
local scroll_vel = 0.0           -- smooth scroll velocity

local TAB_NAMES = { "AUDIO", "VIDEO", "INPUT" }
local SCROLL_SPEED = 48.0  -- pixels per scroll tick

-- ── helpers ────────────────────────────────────────────────────────
local function clamp(v, lo, hi)
  if v < lo then return lo elseif v > hi then return hi else return v end
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

local function outline(cmds, rr, t, c)
  rect(cmds, rr.x, rr.y, rr.w, t, c)
  rect(cmds, rr.x, rr.y + rr.h - t, rr.w, t, c)
  rect(cmds, rr.x, rr.y, t, rr.h, c)
  rect(cmds, rr.x + rr.w - t, rr.y, t, rr.h, c)
end

local function text(cmds, x, y, str, size, c, align, layer)
  local r, g, b, a = rgba(c)
  cmds[#cmds + 1] = { kind = "text", x = x, y = y, text = str, size = size,
    align = align or "left", r = r, g = g, b = b, a = a, layer = layer }
end

local function centered(cmds, x, y, str, size, c, layer)
  text(cmds, x, y, str, size, c, "center", layer)
end

local function sprite(cmds, tex, x, y, w, h, c, layer)
  local r, g, b, a = rgba(c)
  cmds[#cmds + 1] = { kind = "sprite", tex = tex, x = x, y = y, w = w, h = h,
    r = r, g = g, b = b, a = a, layer = layer }
end

-- ── scrollbar ──────────────────────────────────────────────────────
-- Draws a thin gold scrollbar on the right edge of the content area.
local function draw_scrollbar(cmds, l, total_h, scroll, style)
  local visible_h = l.content_h
  if total_h <= visible_h then return end
  local bar_x = l.content_x + l.content_w - 6
  local bar_w = 4
  local thumb_h = math.max(24, visible_h * (visible_h / total_h))
  local max_scroll = total_h - visible_h
  local thumb_y = l.content_y + (scroll / max_scroll) * (visible_h - thumb_h)
  rect(cmds, bar_x, l.content_y, bar_w, visible_h, { 0.08, 0.09, 0.11, 0.6 })
  rect(cmds, bar_x, thumb_y, bar_w, thumb_h, style.gold)
end

-- Returns whether a row at world-y `row_y` with height `rh` is fully inside
-- the clip region (after applying scroll offset). clip_top defaults to
-- l.content_y. If yes, returns the screen-y to draw at; otherwise returns nil.
local function scroll_clip(l, row_y, rh, scroll, clip_top)
  local top = clip_top or l.content_y
  local sy = row_y - scroll
  if sy + rh <= top or sy >= l.content_y + l.content_h then
    return nil
  end
  return sy
end

-- ── read Model helpers ─────────────────────────────────────────────
local function model_number(key, default)
  local v = Model[key]
  if v == nil then return default end
  return v
end

local function model_bool(key, default)
  local v = Model[key]
  if v == nil then return default end
  return v
end

-- ── layout ─────────────────────────────────────────────────────────
local function layout(sw, sh)
  local s = UI.settings
  local pw, ph = s.panel.w, s.panel.h
  local px = math.floor((sw - pw) * 0.5 + 0.5)
  local py = math.floor((sh - ph) * 0.5 + 0.5)
  local t = s.tabs
  local content_y = py + s.content.y
  return {
    px = px, py = py, pw = pw, ph = ph,
    tab_y = py + t.y,
    tab_h = t.h,
    tab_gap = t.gap,
    content_x = px + s.content.pad_x,
    content_y = content_y,
    content_w = pw - s.content.pad_x * 2,
    content_h = ph - (content_y - py) - 60,
    back_x = px + (pw - s.back_button.w) * 0.5,
    back_y = py + ph - s.back_button.h - 14,
    back_w = s.back_button.w,
    back_h = s.back_button.h,
  }
end

local function tab_rect(l, i)
  local s = UI.settings
  local t = s.tabs
  local pad = s.content.pad_x
  local tab_w = l.pw - pad * 2
  local tw = (tab_w - (3 - 1) * t.gap) / 3
  local x = l.px + pad + (i - 1) * (tw + t.gap)
  return { x = x, y = l.tab_y, w = tw, h = t.h }
end

-- ── slider widget ──────────────────────────────────────────────────
local function slider_r(l, x, y, w)
  return { x = x, y = y, w = w, h = 8 }
end

local function slider_update(id, r, mx, my, clicked, down, value, min, max)
  if clicked and point_in(mx, my, r.x, r.y - 6, r.w, r.h + 12) then
    widget_state["sl_" .. id] = true
  end
  if not down then
    widget_state["sl_" .. id] = nil
  end
  if widget_state["sl_" .. id] then
    value = min + clamp((mx - r.x) / r.w, 0, 1) * (max - min)
  end
  return value
end

local function slider_draw(cmds, r, value, min, max, style)
  local t = clamp((value - min) / (max - min), 0, 1)
  rect(cmds, r.x, r.y, r.w, r.h, style.track)
  rect(cmds, r.x, r.y, r.w * t, r.h, style.fill)
  local hw = style.handle_w
  rect(cmds, r.x + r.w * t - hw * 0.5, r.y - 4, hw, r.h + 8, style.handle)
end

-- ── checkbox widget ────────────────────────────────────────────────
local function checkbox_r(l, x, y, size)
  return { x = x, y = y + 3, w = size, h = size }
end

local function checkbox_draw(cmds, r, checked, style)
  rect(cmds, r.x, r.y, r.w, r.h, style.box)
  if checked then
    local inset = 4
    rect(cmds, r.x + inset, r.y + inset, r.w - inset * 2, r.h - inset * 2, style.check)
  end
end

-- ── dropdown widget ────────────────────────────────────────────────
local function dropdown_update(id, r, mx, my, clicked, count)
  if clicked then
    if widget_state["dd_" .. id] then
      local row_h = UI.settings.style.dropdown.row_h
      for i = 1, count do
        local ry = r.y + r.h * i
        if point_in(mx, my, r.x, ry, r.w, row_h) then
          widget_state["dd_val_" .. id] = i
        end
      end
      widget_state["dd_" .. id] = nil
    elseif point_in(mx, my, r.x, r.y, r.w, r.h) then
      widget_state["dd_" .. id] = true
    end
  end
end

local function dropdown_draw(cmds, r, options, selected, style, id)
  rect(cmds, r.x, r.y, r.w, r.h, style.cell)
  text(cmds, r.x + 8, r.y + 3, options[selected] or "?", style.label_size, style.label, "left")
  local cx = r.x + r.w - 14
  local cy = r.y + r.h * 0.5
  local s = 4
  local open = widget_state["dd_" .. id]
  if open then
    rect(cmds, cx - s, cy - s * 0.3, s * 2, s * 0.6, style.label)
  else
    rect(cmds, cx - s, cy + s * 0.3, s * 2, s * 0.6, style.label)
  end
  if open then
    local row_h = style.row_h
    for i = 1, #options do
      local ry = r.y + r.h * i
      local fill = (i == selected) and style.hot or style.cell
      rect(cmds, r.x, ry, r.w, row_h, fill, 1)
      text(cmds, r.x + 8, ry + 3, options[i], style.label_size, style.label, "left", 1)
    end
  end
end

-- ── draw: AUDIO tab ────────────────────────────────────────────────
local function draw_audio(cmds, l, style)
  local s = UI.settings.audio
  local y = l.content_y
  local lx = l.content_x + 8
  local sx = l.content_x + 192
  local sw = l.content_w - 264

  for i, sl in ipairs(s.sliders) do
    local val = model_number("audio_" .. sl.id, sl.default)
    text(cmds, lx, y + 2, sl.label, style.label_size, style.label_color)
    local r = slider_r(l, sx, y + style.row_height - 12, sw)
    slider_draw(cmds, r, val, sl.min, sl.max, style.slider)
    local val_str = string.format("%.2f", val)
    text(cmds, sx + sw + 12, y + 2, val_str, style.label_size, style.label_dim)
    y = y + style.row_height + 8
  end
end

-- ── draw: VIDEO tab ────────────────────────────────────────────────
local function draw_video(cmds, l, style)
  local s = UI.settings.video
  local y = l.content_y
  local lx = l.content_x + 8
  local wx = l.content_x + 192
  local ww = l.content_w - 200
  local dd_style = style.dropdown

  -- Display Mode
  text(cmds, lx, y + 2, "Display Mode", style.label_size, style.label_color)
  local mode_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
  local mode_sel = model_number("video_display_mode", 1)
  dropdown_update("display_mode", mode_r, Model.mx, Model.my, Model.clicked, #s.display_modes)
  local new_mode = widget_state["dd_val_display_mode"]
  if new_mode then mode_sel = new_mode; widget_state["dd_val_display_mode"] = nil end
  dropdown_draw(cmds, mode_r, s.display_modes, mode_sel, dd_style, "display_mode")
  y = y + style.row_height + 12

  -- Resolution
  text(cmds, lx, y + 2, "Resolution", style.label_size, style.label_color)
  local res_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
  local res_sel = model_number("video_resolution", 3)
  dropdown_update("resolution", res_r, Model.mx, Model.my, Model.clicked, #s.resolutions)
  local new_res = widget_state["dd_val_resolution"]
  if new_res then res_sel = new_res; widget_state["dd_val_resolution"] = nil end
  dropdown_draw(cmds, res_r, s.resolutions, res_sel, dd_style, "resolution")
  y = y + style.row_height + 12

  -- Quality
  text(cmds, lx, y + 2, "Graphics Quality", style.label_size, style.label_color)
  local qual_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
  local qual_sel = model_number("video_quality", 3)
  dropdown_update("quality", qual_r, Model.mx, Model.my, Model.clicked, #s.quality_levels)
  local new_qual = widget_state["dd_val_quality"]
  if new_qual then qual_sel = new_qual; widget_state["dd_val_quality"] = nil end
  dropdown_draw(cmds, qual_r, s.quality_levels, qual_sel, dd_style, "quality")
  y = y + style.row_height + 12

  -- VSync
  local vsync = model_bool("video_vsync", true)
  text(cmds, lx + style.checkbox.size + 8, y + 2, "VSync", style.label_size, style.label_color)
  local cb_r = checkbox_r(l, lx, y, style.checkbox.size)
  if Model.clicked and point_in(Model.mx, Model.my, cb_r.x - 4, cb_r.y - 6, cb_r.w + 8, cb_r.h + 12) then
    vsync = not vsync
  end
  checkbox_draw(cmds, cb_r, vsync, style.checkbox)
  y = y + style.row_height + 8

  -- FPS Limit
  text(cmds, lx, y + 2, "FPS Limit", style.label_size, style.label_color)
  local fps_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
  local fps_sel = model_number("video_fps_limit", 2)
  dropdown_update("fps_limit", fps_r, Model.mx, Model.my, Model.clicked, #s.fps_limits)
  local new_fps = widget_state["dd_val_fps_limit"]
  if new_fps then fps_sel = new_fps; widget_state["dd_val_fps_limit"] = nil end
  dropdown_draw(cmds, fps_r, s.fps_limits, fps_sel, dd_style, "fps_limit")
  y = y + style.row_height + 12
end

-- ── draw: INPUT tab ────────────────────────────────────────────────
local function draw_input_section_header(cmds, x, y, w, label, style)
  rect(cmds, x, y, w, style.row_height, style.section_bg)
  text(cmds, x + 8, y + 8, label, style.section_header_size, style.section_header_color)
  return y + style.row_height + 4
end

local function draw_keyboard(cmds, l, style)
  local s = UI.settings.input.sections[1]
  local lx = l.content_x + 8
  local bx = l.content_x + l.content_w * 0.48
  local bw = l.content_w * 0.48
  local rh = style.row_height

  -- Section header (fixed, not scrolled)
  local header_y = l.content_y
  rect(cmds, l.content_x, header_y, l.content_w, style.row_height, style.section_bg)
  text(cmds, l.content_x + 8, header_y + 8, s.label, style.section_header_size, style.section_header_color)
  header_y = header_y + style.row_height + 4

  -- Column headers (fixed, not scrolled)
  text(cmds, lx, header_y, "Action", style.header_size, style.header_color)
  text(cmds, bx, header_y, "Key Binding", style.header_size, style.header_color)
  rect(cmds, l.content_x, header_y + style.header_size + 4, l.content_w, 1, style.divider)
  local rows_y_start = header_y + style.header_size + 12

  -- Compute total rows height and clamp scroll
  local total_rows_h = #s.actions * rh
  local header_used = (rows_y_start - l.content_y)
  local visible_rows_h = l.content_h - header_used
  local max_scroll = math.max(0, total_rows_h - visible_rows_h)
  scroll_offset = clamp(scroll_offset, 0, max_scroll)

  -- Draw scrollbar
  draw_scrollbar(cmds, l, total_rows_h, scroll_offset, style)

  -- Draw rows (scrolled, clipped below header divider)
  for i, action in ipairs(s.actions) do
    local row_y = rows_y_start + (i - 1) * rh
    local sy = scroll_clip(l, row_y, rh, scroll_offset, rows_y_start)
    if sy then
      local row_bg = (i % 2 == 1) and style.section_bg or { 0.14, 0.15, 0.18, 1.0 }
      rect(cmds, l.content_x, sy, l.content_w, rh, row_bg)
      text(cmds, lx, sy + 6, action.label, style.label_size, style.label_color)

      if rebind_action == action.id and not rebind_for_gamepad then
        rect(cmds, bx, sy + 2, bw - 8, rh - 4, style.rebind_bg)
        text(cmds, bx + 8, sy + 6, "Press any key...", style.label_size, style.header_color)
      else
        local binding_str = "< unbound >"
        local col = style.label_dim
        text(cmds, bx + 8, sy + 6, binding_str, style.label_size, col)
        if point_in(Model.mx, Model.my, bx, sy, bw, rh) then
          rect(cmds, bx, sy, bw, rh, { 0.83, 0.67, 0.39, 0.1 })
        end
      end
    end
  end

  -- Redraw the fixed header area on top to cover any row bleed
  rect(cmds, l.content_x, l.content_y, l.content_w, rows_y_start - l.content_y, { 0.06, 0.07, 0.09, 1.0 })
  rect(cmds, l.content_x, l.content_y, l.content_w, style.row_height, style.section_bg)
  text(cmds, l.content_x + 8, l.content_y + 8, s.label, style.section_header_size, style.section_header_color)
  text(cmds, lx, header_y, "Action", style.header_size, style.header_color)
  text(cmds, bx, header_y, "Key Binding", style.header_size, style.header_color)
  rect(cmds, l.content_x, header_y + style.header_size + 4, l.content_w, 1, style.divider)
end

local function draw_mouse(cmds, l, style)
  local s = UI.settings.input.sections[2]
  local y = l.content_y
  local lx = l.content_x + 8
  local sx = l.content_x + 192
  local sw = l.content_w - 264
  local rh = style.row_height

  y = draw_input_section_header(cmds, l.content_x, y, l.content_w, s.label, style)

  for _, sl in ipairs(s.sliders) do
    local val = model_number("input_mouse_" .. sl.id, sl.default)
    text(cmds, lx, y + 2, sl.label, style.label_size, style.label_color)
    local r = slider_r(l, sx, y + rh - 12, sw)
    slider_draw(cmds, r, val, sl.min, sl.max, style.slider)
    local val_str = string.format("%.4f", val)
    text(cmds, sx + sw + 12, y + 2, val_str, style.label_size, style.label_dim)
    y = y + rh + 8
  end

  y = y + 4
  for _, cb in ipairs(s.checkboxes) do
    local checked = model_bool("input_mouse_" .. cb.id, cb.default)
    text(cmds, lx + style.checkbox.size + 8, y + 2, cb.label, style.label_size, style.label_color)
    local cb_r = checkbox_r(l, lx, y, style.checkbox.size)
    checkbox_draw(cmds, cb_r, checked, style.checkbox)
    y = y + rh + 4
  end
end

local function draw_controller_bindings(cmds, l, style, s, y_start, scroll)
  local lx = l.content_x + 8
  local bx = l.content_x + l.content_w * 0.48
  local bw = l.content_w * 0.48
  local rh = style.row_height

  local sy = scroll_clip(l, y_start, style.header_size + 12, scroll)
  if sy then
    text(cmds, lx, sy, "Action", style.header_size, style.header_color)
    text(cmds, bx, sy, "Gamepad Binding", style.header_size, style.header_color)
    rect(cmds, l.content_x, sy + style.header_size + 4, l.content_w, 1, style.divider)
  end
  local y = y_start + style.header_size + 12

  for i, action in ipairs(s.actions) do
    local row_y = y + (i - 1) * rh
    local sy2 = scroll_clip(l, row_y, rh, scroll)
    if sy2 then
      local row_bg = (i % 2 == 1) and style.section_bg or { 0.14, 0.15, 0.18, 1.0 }
      rect(cmds, l.content_x, sy2, l.content_w, rh, row_bg)
      text(cmds, lx, sy2 + 6, action.label, style.label_size, style.label_color)

      if rebind_action == action.id and rebind_for_gamepad then
        rect(cmds, bx, sy2 + 2, bw - 8, rh - 4, style.rebind_bg)
        text(cmds, bx + 8, sy2 + 6, "Press a button...", style.label_size, style.header_color)
      else
        local binding_str = "< unbound >"
        local col = style.label_dim
        text(cmds, bx + 8, sy2 + 6, binding_str, style.label_size, col)
        if point_in(Model.mx, Model.my, bx, sy2, bw, rh) then
          rect(cmds, bx, sy2, bw, rh, { 0.83, 0.67, 0.39, 0.1 })
        end
      end
    end
  end
end

local function draw_controller(cmds, l, style)
  local s = UI.settings.input.sections[3]
  local lx = l.content_x + 8
  local sx = l.content_x + 192
  local sw = l.content_w - 264
  local rh = style.row_height
  local dd_style = style.dropdown

  -- Compute total content height for scroll clamping
  local fixed_h = style.row_height + 4  -- section header
  fixed_h = fixed_h + #s.sliders * (rh + 8)
  fixed_h = fixed_h + 4 + #s.checkboxes * (rh + 4)
  fixed_h = fixed_h + 4 + #s.dropdowns * (rh + 8)
  fixed_h = fixed_h + 8  -- gap before bindings
  local bindings_header_h = style.header_size + 12
  local bindings_rows_h = #s.actions * rh
  local total_h = fixed_h + bindings_header_h + bindings_rows_h
  local max_scroll = math.max(0, total_h - l.content_h)
  scroll_offset = clamp(scroll_offset, 0, max_scroll)

  -- Draw scrollbar
  draw_scrollbar(cmds, l, total_h, scroll_offset, style)

  -- Section header (fixed, not scrolled)
  local y = l.content_y
  rect(cmds, l.content_x, y, l.content_w, style.row_height, style.section_bg)
  text(cmds, l.content_x + 8, y + 8, s.label, style.section_header_size, style.section_header_color)
  y = y + style.row_height + 4

  -- Settings (scrolled)
  for _, sl in ipairs(s.sliders) do
    local val = model_number("input_ctrl_" .. sl.id, sl.default)
    local sy = scroll_clip(l, y, rh, scroll_offset)
    if sy then
      text(cmds, lx, sy + 2, sl.label, style.label_size, style.label_color)
      local r = slider_r(l, sx, sy + rh - 12, sw)
      slider_draw(cmds, r, val, sl.min, sl.max, style.slider)
      local val_str = string.format("%.2f", val)
      text(cmds, sx + sw + 12, sy + 2, val_str, style.label_size, style.label_dim)
    end
    y = y + rh + 8
  end

  y = y + 4
  for _, cb in ipairs(s.checkboxes) do
    local checked = model_bool("input_ctrl_" .. cb.id, cb.default)
    local sy = scroll_clip(l, y, rh, scroll_offset)
    if sy then
      text(cmds, lx + style.checkbox.size + 8, sy + 2, cb.label, style.label_size, style.label_color)
      local cb_r = checkbox_r(l, lx, sy, style.checkbox.size)
      checkbox_draw(cmds, cb_r, checked, style.checkbox)
    end
    y = y + rh + 4
  end

  y = y + 4
  for _, dd in ipairs(s.dropdowns) do
    local sel = model_number("input_ctrl_" .. dd.id, dd.default)
    local sy = scroll_clip(l, y, rh, scroll_offset)
    if sy then
      text(cmds, lx, sy + 2, dd.label, style.label_size, style.label_color)
      local dr = { x = sx, y = sy, w = sw, h = dd_style.row_h }
      dropdown_update(dd.id, dr, Model.mx, Model.my, Model.clicked, #dd.options)
      local new_sel = widget_state["dd_val_" .. dd.id]
      if new_sel then sel = new_sel - 1; widget_state["dd_val_" .. dd.id] = nil end
      dropdown_draw(cmds, dr, dd.options, sel + 1, dd_style, dd.id)
    else
      -- Still update dropdown state even when clipped
      local dr = { x = sx, y = y, w = sw, h = dd_style.row_h }
      dropdown_update(dd.id, dr, Model.mx, Model.my, Model.clicked, #dd.options)
      local new_sel = widget_state["dd_val_" .. dd.id]
      if new_sel then sel = new_sel - 1; widget_state["dd_val_" .. dd.id] = nil end
    end
    y = y + rh + 8
  end

  y = y + 8
  -- Bindings header and rows (scrolled)
  draw_controller_bindings(cmds, l, style, s, y, scroll_offset)
end

local function draw_input(cmds, l, style)
  local sub_names = { "KEYBOARD", "MOUSE", "CONTROLLER" }
  local sub_w = l.content_w / 3
  for i, name in ipairs(sub_names) do
    local sx = l.content_x + (i - 1) * sub_w
    local is_active = (i == input_subtab)
    if is_active then
      rect(cmds, sx, l.content_y, sub_w, style.row_height, { 0.14, 0.15, 0.18, 0.6 })
    end
    local col = is_active and style.header_color or style.label_dim
    centered(cmds, sx + sub_w * 0.5, l.content_y + 8, name, style.section_header_size, col)
    if is_active then
      rect(cmds, sx, l.content_y + style.row_height - 2, sub_w, 2, style.gold)
    end
    if point_in(Model.mx, Model.my, sx, l.content_y, sub_w, style.row_height) then
      rect(cmds, sx, l.content_y, sub_w, style.row_height, { 0.83, 0.67, 0.39, 0.1 })
    end
  end

  local inner_y = l.content_y + style.row_height + 8
  local inner_h = l.content_h - (inner_y - l.content_y)
  local inner_l = { content_x = l.content_x, content_y = inner_y, content_w = l.content_w, content_h = inner_h }

  if input_subtab == 1 then
    draw_keyboard(cmds, inner_l, style)
  elseif input_subtab == 2 then
    draw_mouse(cmds, inner_l, style)
  else
    draw_controller(cmds, inner_l, style)
  end
end

-- ── update ─────────────────────────────────────────────────────────
function M.update(mx, my, clicked, sw, sh, down)
  local l = layout(sw, sh)
  local style = UI.settings.style
  local results = {}

  hover = nil

  -- ── tab clicks ──
  for i = 1, 3 do
    local tr = tab_rect(l, i)
    if clicked and point_in(mx, my, tr.x, tr.y, tr.w, tr.h) then
      if active_tab ~= i then
        scroll_offset = 0
      end
      active_tab = i
    end
  end

  -- ── input sub-tab clicks ──
  if active_tab == 3 then
    local sub_w = l.content_w / 3
    for i = 1, 3 do
      local sx = l.content_x + (i - 1) * sub_w
      if clicked and point_in(mx, my, sx, l.content_y, sub_w, style.row_height) then
        if input_subtab ~= i then
          scroll_offset = 0
        end
        input_subtab = i
      end
    end
  end

  -- ── scroll wheel ──
  local scroll_delta = model_number("scroll", 0) * SCROLL_SPEED
  if active_tab == 3 and (input_subtab == 1 or input_subtab == 3) then
    scroll_offset = scroll_offset - scroll_delta
  end

  -- ── tab-specific update logic ──
  if active_tab == 1 then
    local s = UI.settings.audio
    local slider_w = l.content_w - 200
    local sx = l.content_x + 192
    local y = l.content_y
    for _, sl in ipairs(s.sliders) do
      local val = model_number("audio_" .. sl.id, sl.default)
      local r = slider_r(l, sx, y + style.row_height - 12, slider_w)
      val = slider_update(sl.id, r, mx, my, clicked, down or false, val, sl.min, sl.max)
      results["audio_" .. sl.id] = val
      y = y + style.row_height + 8
    end

  elseif active_tab == 2 then
    local s = UI.settings.video
    local lx = l.content_x + 8
    local wx = l.content_x + 192
    local ww = l.content_w - 200
    local dd_style = style.dropdown
    local y = l.content_y

    local mode_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
    dropdown_update("display_mode", mode_r, mx, my, clicked, #s.display_modes)
    local new_mode = widget_state["dd_val_display_mode"]
    if new_mode then results.video_display_mode = new_mode; widget_state["dd_val_display_mode"] = nil end
    y = y + style.row_height + 12

    local res_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
    dropdown_update("resolution", res_r, mx, my, clicked, #s.resolutions)
    local new_res = widget_state["dd_val_resolution"]
    if new_res then results.video_resolution = new_res; widget_state["dd_val_resolution"] = nil end
    y = y + style.row_height + 12

    local qual_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
    dropdown_update("quality", qual_r, mx, my, clicked, #s.quality_levels)
    local new_qual = widget_state["dd_val_quality"]
    if new_qual then results.video_quality = new_qual; widget_state["dd_val_quality"] = nil end
    y = y + style.row_height + 12

    local vsync = model_bool("video_vsync", true)
    local cb_r = checkbox_r(l, lx, y, style.checkbox.size)
    if clicked and point_in(mx, my, cb_r.x - 4, cb_r.y - 6, cb_r.w + 8, cb_r.h + 12) then
      vsync = not vsync
      results.video_vsync = vsync
    end
    y = y + style.row_height + 8

    local fps_r = { x = wx, y = y, w = ww, h = dd_style.row_h }
    dropdown_update("fps_limit", fps_r, mx, my, clicked, #s.fps_limits)
    local new_fps = widget_state["dd_val_fps_limit"]
    if new_fps then results.video_fps_limit = new_fps; widget_state["dd_val_fps_limit"] = nil end

  elseif active_tab == 3 then
    if input_subtab == 1 then
      local s = UI.settings.input.sections[1]
      local bx = l.content_x + l.content_w * 0.48
      local bw = l.content_w * 0.48
      local rh = style.row_height
      local rows_y_start = l.content_y + style.row_height + 4 + style.header_size + 12
      for i, action in ipairs(s.actions) do
        local row_y = rows_y_start + (i - 1) * rh
        local sy = scroll_clip(l, row_y, rh, scroll_offset)
        if sy and clicked and point_in(mx, my, bx, sy, bw, rh) then
          if rebind_action == action.id and not rebind_for_gamepad then
            rebind_action = nil
          else
            rebind_action = action.id
            rebind_for_gamepad = false
          end
        end
      end

    elseif input_subtab == 2 then
      local s = UI.settings.input.sections[2]
      local lx = l.content_x + 8
      local sx = l.content_x + 192
      local sw_local = l.content_w - 200
      local rh = style.row_height
      local y = l.content_y + style.row_height + 8 + style.header_size + 12

      for _, sl in ipairs(s.sliders) do
        local val = model_number("input_mouse_" .. sl.id, sl.default)
        local r = slider_r(l, sx, y + rh - 12, sw_local)
        val = slider_update(sl.id, r, mx, my, clicked, down or false, val, sl.min, sl.max)
        results["input_mouse_" .. sl.id] = val
        y = y + rh + 8
      end

      y = y + 4
      for _, cb in ipairs(s.checkboxes) do
        local checked = model_bool("input_mouse_" .. cb.id, cb.default)
        local cb_r = checkbox_r(l, lx, y, style.checkbox.size)
        if clicked and point_in(mx, my, cb_r.x - 4, cb_r.y - 6, cb_r.w + 8, cb_r.h + 12) then
          results["input_mouse_" .. cb.id] = not checked
        end
        y = y + rh + 4
      end

    elseif input_subtab == 3 then
      local s = UI.settings.input.sections[3]
      local lx = l.content_x + 8
      local sx = l.content_x + 192
      local sw_local = l.content_w - 264
      local rh = style.row_height
      local dd_style = style.dropdown

      -- Compute fixed content heights to get to bindings
      local y = l.content_y + style.row_height + 4
      for _, sl in ipairs(s.sliders) do
        local val = model_number("input_ctrl_" .. sl.id, sl.default)
        local sy = scroll_clip(l, y, rh, scroll_offset)
        if sy then
          local r = slider_r(l, sx, sy + rh - 12, sw_local)
          val = slider_update(sl.id, r, mx, my, clicked, down or false, val, sl.min, sl.max)
        end
        results["input_ctrl_" .. sl.id] = val
        y = y + rh + 8
      end

      y = y + 4
      for _, cb in ipairs(s.checkboxes) do
        local checked = model_bool("input_ctrl_" .. cb.id, cb.default)
        local sy = scroll_clip(l, y, rh, scroll_offset)
        if sy then
          local cb_r = checkbox_r(l, lx, sy, style.checkbox.size)
          if clicked and point_in(mx, my, cb_r.x - 4, cb_r.y - 6, cb_r.w + 8, cb_r.h + 12) then
            results["input_ctrl_" .. cb.id] = not checked
          end
        end
        y = y + rh + 4
      end

      y = y + 4
      for _, dd in ipairs(s.dropdowns) do
        local sel = model_number("input_ctrl_" .. dd.id, dd.default)
        local sy = scroll_clip(l, y, rh, scroll_offset)
        local dr = { x = sx, y = sy or y, w = sw_local, h = dd_style.row_h }
        dropdown_update(dd.id, dr, mx, my, clicked, #dd.options)
        local new_sel = widget_state["dd_val_" .. dd.id]
        if new_sel then results["input_ctrl_" .. dd.id] = new_sel - 1; widget_state["dd_val_" .. dd.id] = nil end
        y = y + rh + 8
      end

      y = y + 8
      -- Bindings header and rows (scrolled)
      local bx = l.content_x + l.content_w * 0.48
      local bw = l.content_w * 0.48
      local bindings_header_h = style.header_size + 12
      local rows_y_start = y + bindings_header_h
      for i, action in ipairs(s.actions) do
        local row_y = rows_y_start + (i - 1) * rh
        local sy = scroll_clip(l, row_y, rh, scroll_offset)
        if sy and clicked and point_in(mx, my, bx, sy, bw, rh) then
          if rebind_action == action.id and rebind_for_gamepad then
            rebind_action = nil
          else
            rebind_action = action.id
            rebind_for_gamepad = true
          end
        end
      end
    end
  end

  -- ── back button ──
  if clicked and point_in(mx, my, l.back_x, l.back_y, l.back_w, l.back_h) then
    results.settings_back = true
  end

  -- ── rebind overlay ──
  if rebind_action then
    results.settings_rebind_active = true
    results.settings_rebind_action = rebind_action
    results.settings_rebind_gamepad = rebind_for_gamepad
    if clicked then
      results.settings_rebind_cancel = true
      rebind_action = nil
    end
  end

  results.settings_input_subtab = input_subtab

  return results
end

-- ── draw ───────────────────────────────────────────────────────────
function M.draw(sw, sh)
  local l = layout(sw, sh)
  local s = UI.settings
  local style = s.style
  local cmds = {}

  -- overlay
  rect(cmds, 0, 0, sw, sh, { 0.02, 0.02, 0.03, 0.64 })

  -- panel sprite
  sprite(cmds, Textures[s.panel.texture], l.px, l.py, l.pw, l.ph, { 1, 1, 1, 1 })

  -- title
  centered(cmds, l.px + l.pw * 0.5, l.py + s.title.offset_y, s.title.text, s.title.size, s.title.color)

  -- gold divider under title
  rect(cmds, l.px + 28, l.py + s.title.offset_y + s.title.size + 8, l.pw - 56, 1, style.gold_line)

  -- tab bar
  for i = 1, 3 do
    local tr = tab_rect(l, i)
    local is_active = (i == active_tab)
    local is_hovered = point_in(Model.mx or 0, Model.my or 0, tr.x, tr.y, tr.w, tr.h)

    if is_active then
      rect(cmds, tr.x, tr.y, tr.w, tr.h, style.active_bg or { 0.14, 0.15, 0.18, 0.6 })
    elseif is_hovered then
      rect(cmds, tr.x, tr.y, tr.w, tr.h, { 0.83, 0.67, 0.39, 0.1 })
    end

    local col = is_active and style.header_color or style.label_dim
    centered(cmds, tr.x + tr.w * 0.5, tr.y + 10, TAB_NAMES[i], style.label_size, col)

    if is_active then
      rect(cmds, tr.x, tr.y + tr.h - 2, tr.w, 2, style.gold)
    end
  end

  -- content area border
  rect(cmds, l.content_x - 4, l.content_y - 4, l.content_w + 8, l.content_h + 8, style.divider)

  -- tab content
  if active_tab == 1 then
    draw_audio(cmds, l, style)
  elseif active_tab == 2 then
    draw_video(cmds, l, style)
  else
    draw_input(cmds, l, style)
  end

  -- back button
  sprite(cmds, Textures[s.back_button.texture], l.back_x, l.back_y, l.back_w, l.back_h, { 1, 1, 1, 1 })
  local back_hover = point_in(Model.mx or 0, Model.my or 0, l.back_x, l.back_y, l.back_w, l.back_h)
  if back_hover then
    rect(cmds, l.back_x, l.back_y, l.back_w, l.back_h, { 0.85, 0.66, 0.34, 0.15 })
    outline(cmds, { x = l.back_x, y = l.back_y, w = l.back_w, h = l.back_h }, 2, { 0.85, 0.66, 0.32, 0.95 })
  end
  centered(cmds, l.back_x + l.back_w * 0.5, l.back_y + (l.back_h - s.back_button.label_size) * 0.5,
    s.back_button.label, s.back_button.label_size, style.label_color)

  -- rebind overlay
  if rebind_action then
    local ro = s.input.rebind_overlay
    rect(cmds, 0, 0, sw, sh, { 0, 0, 0, 0.6 })
    local bx = (sw - ro.w) * 0.5
    local by = (sh - ro.h) * 0.5
    rect(cmds, bx, by, ro.w, ro.h, ro.bg)
    outline(cmds, { x = bx, y = by, w = ro.w, h = ro.h }, 2, style.gold_line)

    local title = rebind_for_gamepad and "Press a gamepad button..." or "Press a key or mouse button..."
    centered(cmds, bx + ro.w * 0.5, by + 24, title, ro.title_size, ro.title_color)
    centered(cmds, bx + ro.w * 0.5, by + 56, rebind_action, style.label_size, style.label_color)
    centered(cmds, bx + ro.w * 0.5, by + ro.h - 28, "Click to cancel", ro.hint_size, ro.hint_color)
  end

  return cmds
end

return M
