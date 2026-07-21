-- Prism Settings "workbench" — a windowed settings screen: title bar + left
-- category rail (Video / Audio / Input, each keyed to a Septisigil gem) + right
-- scrolling content + footer (Restore / Back / Apply). Driven by `UI.settings`
-- (ui_elements.json); current values flow in via `Model`, changes flow out as
-- results the shell persists.
--
-- Chrome + controls are VECTOR `panel` commands (rounded-rect + gradient +
-- border). Interaction reuses the shared `Widgets` toolkit (slider / dropdown /
-- radio / checkbox hit-tests); this script owns the layout + the flat drawing.
-- A row's `wired` flag marks whether it is bound to a real backend — inert rows
-- (FOV, brightness, edge-pan, all of Audio, the controller device) are a layout
-- preview and keep their value locally.

local M = {}

-- ── state ──────────────────────────────────────────────────────────
local section = "video"        -- active category: video / audio / input
local input_tab = "keyboard"   -- active input sub-tab
local scroll = 0.0             -- content scroll offset (px)
local ws = {}                  -- transient widget state (drag / open flags)
local lv = {}                  -- local values for inert (unwired) controls
local applied = 0.0            -- "settings applied" flash countdown (s)

local SCROLL_SPEED = 46.0

-- ── small helpers ──────────────────────────────────────────────────
local function clamp(v, lo, hi) if v < lo then return lo elseif v > hi then return hi else return v end end
local function point_in(px, py, x, y, w, h) return px >= x and px <= x + w and py >= y and py <= y + h end
local function mnum(k, d) local v = Model[k]; if v == nil then return d end; return v end
local function mbool(k, d) local v = Model[k]; if v == nil then return d end; return v end
local function lval(id, d) if lv[id] == nil then lv[id] = d end; return lv[id] end
local function with_alpha(c, a) return { c[1], c[2], c[3], a } end

-- ── draw emitters ──────────────────────────────────────────────────
local function panel(cmds, x, y, w, h, c0, c1, grad, radius, border, bcolor, feather, layer)
  local cmd = { kind = "panel", x = x, y = y, w = w, h = h,
    r = c0[1], g = c0[2], b = c0[3], a = c0[4],
    grad = grad or 0, radius = radius or 0, border = border or 0, feather = feather or 0, layer = layer or 0 }
  if c1 then cmd.r2 = c1[1]; cmd.g2 = c1[2]; cmd.b2 = c1[3]; cmd.a2 = c1[4] end
  if bcolor then cmd.br = bcolor[1]; cmd.bg = bcolor[2]; cmd.bb = bcolor[3]; cmd.ba = bcolor[4] end
  cmds[#cmds + 1] = cmd
end
local function rect(cmds, x, y, w, h, c, layer) panel(cmds, x, y, w, h, c, nil, 0, 0, 0, nil, 0, layer) end
local function text(cmds, x, y, str, size, c, align, font, layer)
  cmds[#cmds + 1] = { kind = "text", x = x, y = y, text = str, size = size, align = align or "left",
    font = font, r = c[1], g = c[2], b = c[3], a = c[4] * (Model._alpha or 1.0), layer = layer or 0 }
end
-- A small downward caret from stacked rows (avoids a glyph-font dependency).
local function caret(cmds, cx, cy, s, c, layer)
  for i = 0, 3 do
    local w = s * (1 - i / 4)
    rect(cmds, cx - w * 0.5, cy - 1 + i, w, 1, c, layer)
  end
end

-- ── window layout ──────────────────────────────────────────────────
local function layout(sw, sh)
  local S = UI.settings
  local win = S.window
  local m = win.min_margin
  local w = math.min(win.w, sw - m * 2)
  local h = math.min(win.h, sh - m * 2)
  local x = math.floor((sw - w) * 0.5 + 0.5)
  local y = math.floor((sh - h) * 0.5 + 0.5)
  local tb = S.titlebar.h
  local fb = S.footer.h
  local nav_w = S.nav.w
  local body_y = y + tb
  local body_h = h - tb - fb
  return {
    x = x, y = y, w = w, h = h, r = x + w, b = y + h,
    tb = tb, fb = fb,
    body_y = body_y, body_h = body_h, footer_y = y + h - fb,
    nav_w = nav_w,
    cx = x + nav_w,               -- content left
    cw = w - nav_w,               -- content width
  }
end

-- ── nav rail ───────────────────────────────────────────────────────
local function nav_rects(l)
  local S = UI.settings
  local nav = S.nav
  local out = {}
  local yy = l.body_y + nav.pad + 22
  for i, it in ipairs(S.nav_items) do
    out[i] = { id = it.id, y = yy, x = l.x + nav.pad, w = l.nav_w - nav.pad * 2, h = nav.row_h, item = it }
    yy = yy + nav.row_h + nav.gap
  end
  return out
end

-- ── content header (fixed) + scroll region ─────────────────────────
local function content_regions(l)
  local S = UI.settings
  local head_h = (section == "input") and 78 or 68
  local pad = S.content.pad_x
  return {
    hx = l.cx + pad, hy = l.body_y + S.content.pad_top, hw = l.cw - pad * 2,
    head_bottom = l.body_y + head_h,
    sx = l.cx + pad, sy = l.body_y + head_h + 10,
    sw = l.cw - pad * 2, sh = l.footer_y - (l.body_y + head_h + 10) - 6,
  }
end

-- Build the ordered, scrollable content items for the active section. Each item:
-- { kind, h, ... }. Heights drive scroll; kinds drive draw + hit-test.
local function build_items()
  local S = UI.settings
  local items = {}
  local row_h, group_gap = S.row.h, S.row.group_gap
  local function add_groups(groups)
    for _, g in ipairs(groups) do
      items[#items + 1] = { kind = "group", name = g.name, h = 30 }
      for _, r in ipairs(g.rows or {}) do
        items[#items + 1] = { kind = "row", row = r, h = row_h }
      end
      items[#items + 1] = { kind = "gap", h = group_gap - 12 }
    end
  end
  if section == "video" then
    add_groups(S.video.groups)
  elseif section == "audio" then
    items[#items + 1] = { kind = "banner", h = 70 }
    add_groups(S.audio.groups)
  elseif section == "input" then
    if input_tab == "keyboard" then
      if Model.rebind_action then items[#items + 1] = { kind = "rebind", h = 52 } end
      for _, g in ipairs(S.input.keyboard.groups) do
        items[#items + 1] = { kind = "group", name = g.name, h = 30 }
        for _, a in ipairs(g.actions) do
          items[#items + 1] = { kind = "key", action = a, h = 42 }
        end
        items[#items + 1] = { kind = "gap", h = group_gap - 12 }
      end
    elseif input_tab == "mouse" then
      add_groups(S.input.mouse.groups)
    else
      items[#items + 1] = { kind = "empty", h = 320 }
    end
  end
  return items
end

local function items_total_h(items)
  local t = 0
  for _, it in ipairs(items) do t = t + it.h end
  return t
end

-- ── control value access ───────────────────────────────────────────
-- Slider display value (in the row's min..max), reading Model when wired.
local function slider_value(r)
  if r.wired and r.id == "m_look" then
    local b = mnum("input_mouse_sensitivity", (r.backend_min + r.backend_max) * 0.5)
    return clamp((b - r.backend_min) / (r.backend_max - r.backend_min) * 100.0, r.min, r.max)
  end
  return lval(r.id, r.default or r.min)
end

local function index_value(r)  -- 0-based selected index for dropdown/segment/cycler
  if r.wired then
    if r.id == "display_mode" then return math.floor(mnum("video_display_mode", 0)) end
    if r.id == "resolution" then return math.floor(mnum("video_resolution", 0)) end
    if r.id == "quality" then return math.floor(mnum("video_quality", 0)) end
    if r.id == "fps_limit" then return math.floor(mnum("video_fps_limit", 0)) end
  end
  return lval(r.id, r.default or 0)
end

local function bool_value(r)
  if r.wired then
    if r.id == "vsync" then return mbool("video_vsync", true) end
    if r.id == "m_invert" then return mbool("input_mouse_invert_pitch", false) end
  end
  return lval(r.id .. "_b", r.default_on or false)
end

-- ── update ─────────────────────────────────────────────────────────
function M.update(mx, my, clicked, sw, sh, down)
  local S = UI.settings
  local l = layout(sw, sh)
  local results = {}
  down = down or false

  -- nav
  for _, n in ipairs(nav_rects(l)) do
    if clicked and point_in(mx, my, n.x, n.y, n.w, n.h) and section ~= n.id then
      section = n.id; scroll = 0
    end
  end

  -- title-bar close (×)
  local tbc = S.titlebar.close
  local cx0 = l.r - S.titlebar.pad_x - tbc.size
  if clicked and point_in(mx, my, cx0, l.y + (l.tb - tbc.size) * 0.5, tbc.size, tbc.size) then
    results.settings_back = true
  end

  -- input sub-tab pills
  if section == "input" then
    local reg = content_regions(l)
    local tabs = S.input.tabs
    local pw, ph = 108, S.controls.pill.h
    local tx = l.r - S.content.pad_x - pw * #tabs - S.controls.pill.pad
    for i, t in ipairs(tabs) do
      local rx = tx + (i - 1) * pw
      if clicked and point_in(mx, my, rx, reg.hy + 8, pw, ph) and input_tab ~= t.id then
        input_tab = t.id; scroll = 0
      end
    end
  end

  -- scroll wheel over content
  local reg = content_regions(l)
  local items = build_items()
  local total = items_total_h(items)
  local maxscroll = math.max(0, total - reg.sh)
  if point_in(mx, my, l.cx, reg.sy, l.cw, reg.sh) then
    scroll = scroll - mnum("scroll", 0) * SCROLL_SPEED
  end
  scroll = clamp(scroll, 0, maxscroll)

  -- walk scrollable items, hit-test the visible controls
  local yy = reg.sy - scroll
  for _, it in ipairs(items) do
    local sy = yy
    yy = yy + it.h
    if sy + it.h > reg.sy and sy < reg.sy + reg.sh then
      if it.kind == "row" then
        local r = it.row
        local ctrl_w = S.row.ctrl_w
        local rx = reg.sx + reg.sw - ctrl_w
        local cyc = sy + (S.row.h - 40) * 0.5
        if r.kind == "slider" then
          local vw, sw2 = 46, ctrl_w - 46 - 12
          local rr = { x = rx, y = cyc + 16, w = sw2, h = 8 }
          local val = slider_value(r)
          local nv = Widgets.slider_update(ws, "sl_" .. r.id, rr, mx, my, clicked, down, val, r.min, r.max)
          if nv ~= val then
            if r.wired and r.id == "m_look" then
              results.input_mouse_sensitivity = r.backend_min + (nv / 100.0) * (r.backend_max - r.backend_min)
            elseif not r.wired then
              lv[r.id] = nv
            end
          end
        elseif r.kind == "toggle" then
          local tr = { x = rx + ctrl_w - S.controls.toggle.w, y = cyc + 8, w = S.controls.toggle.w, h = S.controls.toggle.h }
          local cur = bool_value(r)
          local nv = Widgets.checkbox_update(tr, mx, my, clicked, cur)
          if nv ~= cur then
            if r.wired and r.id == "vsync" then results.video_vsync = nv
            elseif r.wired and r.id == "m_invert" then results.input_mouse_invert_pitch = nv
            else lv[r.id .. "_b"] = nv end
          end
        elseif r.kind == "segment" then
          local sg = S.controls.segment
          local sr = { x = rx + sg.pad, y = cyc + sg.pad, w = ctrl_w - sg.pad * 2, h = sg.h - sg.pad * 2 }
          local sel = index_value(r)
          local nv = Widgets.radio_update(sr, mx, my, clicked, #r.options, sel + 1) - 1
          if nv ~= sel then
            if r.wired then results["video_" .. r.id] = nv else lv[r.id] = nv end
          end
        elseif r.kind == "cycler" then
          local bw = 38
          local n = #r.options
          local sel = index_value(r)
          if clicked and point_in(mx, my, rx, cyc, bw, 40) then sel = (sel - 1 + n) % n
          elseif clicked and point_in(mx, my, rx + ctrl_w - bw, cyc, bw, 40) then sel = (sel + 1) % n end
          local cur = index_value(r)
          if sel ~= cur then
            if r.wired then results["video_" .. r.id] = sel else lv[r.id] = sel end
          end
        elseif r.kind == "dropdown" then
          -- Hand-rolled to match draw_dropdown's menu geometry (a 6px gap, then
          -- menu.row_h rows) — the shared Widgets.dropdown_update assumes a
          -- different layout.
          local fh = S.controls.field.h
          local mrow = S.controls.menu.row_h
          local ddk = "dd_" .. r.id
          if clicked then
            if ws[ddk] then
              local my0 = cyc + fh + 6
              for i = 1, #r.options do
                if point_in(mx, my, rx, my0 + (i - 1) * mrow, ctrl_w, mrow) then
                  local nv = i - 1
                  if r.wired then results["video_" .. r.id] = nv else lv[r.id] = nv end
                end
              end
              ws[ddk] = nil
            elseif point_in(mx, my, rx, cyc, ctrl_w, fh) then
              ws[ddk] = true
            end
          end
        end
      elseif it.kind == "key" then
        local a = it.action
        local ctrl_w = S.controls.keycap.w
        local kr = { x = reg.sx + reg.sw - ctrl_w, y = sy + (it.h - S.controls.keycap.h) * 0.5, w = ctrl_w, h = S.controls.keycap.h }
        if clicked and point_in(mx, my, kr.x, kr.y, kr.w, kr.h) then
          results.settings_rebind_active = true
          results.settings_rebind_action = a.id
          results.settings_rebind_gamepad = false
        end
      end
    end
  end

  -- footer
  local fy = l.footer_y
  local pad = S.footer.pad_x
  -- restore (left text button)
  if clicked and point_in(mx, my, l.x + pad, fy + (l.fb - 22) * 0.5, 150, 22) then
    results.settings_restore = true
  end
  -- back + apply (right)
  local bw, aw, bh = 96, 104, 34
  local ax = l.r - pad - aw
  local bx = ax - 12 - bw
  local by = fy + (l.fb - bh) * 0.5
  if clicked and point_in(mx, my, bx, by, bw, bh) then results.settings_back = true end
  if clicked and point_in(mx, my, ax, by, aw, bh) then results.settings_apply = true; applied = 2.0 end

  -- rebind: any click cancels
  if Model.rebind_action and clicked then results.settings_rebind_cancel = true end

  results.settings_input_subtab = input_tab
  return results
end

-- ── control drawing ────────────────────────────────────────────────
local L = 1  -- content layer; open dropdown menus draw at L+1

local function draw_slider(cmds, r, rx, cyc, ctrl_w)
  local S = UI.settings.controls.slider
  local val = slider_value(r)
  local vw, sw2 = 46, ctrl_w - 46 - 12
  local tx, ty = rx, cyc + 16
  local t = clamp((val - r.min) / (r.max - r.min), 0, 1)
  panel(cmds, tx, ty, sw2, S.h, S.track, nil, 0, 4, 1, S.border, 0, L)
  if t > 0 then panel(cmds, tx, ty, sw2 * t, S.h, S.fill_lo, S.fill_hi, 2, 4, 0, nil, 0, L) end
  local kx = tx + sw2 * t
  panel(cmds, kx - S.knob * 0.5, ty + S.h * 0.5 - S.knob * 0.5, S.knob, S.knob, S.knob_top, S.knob_bot, 1, S.knob * 0.5, 1, S.knob_border, 0, L)
  panel(cmds, kx - 2, ty + S.h * 0.5 - 2, 4, 4, S.knob_dot, nil, 0, 2, 0, nil, 0, L)
  local suffix = r.suffix or ""
  text(cmds, rx + ctrl_w, cyc + 12, string.format("%d", math.floor(val + 0.5)) .. suffix, UI.settings.row.value_size, UI.settings.row.value_color, "right", "label", L)
end

local function draw_toggle(cmds, r, rx, cyc, ctrl_w)
  local S = UI.settings.controls.toggle
  local on = bool_value(r)
  local tx = rx + ctrl_w - S.w
  local ty = cyc + 8
  if on then
    panel(cmds, tx, ty, S.w, S.h, S.on_top, S.on_bot, 1, S.h * 0.5, 1, S.on_border, 0, L)
    panel(cmds, tx + S.w - S.h + 3, ty + 3, S.h - 6, S.h - 6, S.knob_on, nil, 0, (S.h - 6) * 0.5, 0, nil, 0, L)
  else
    panel(cmds, tx, ty, S.w, S.h, S.off_bg, nil, 0, S.h * 0.5, 1, S.off_border, 0, L)
    panel(cmds, tx + 3, ty + 3, S.h - 6, S.h - 6, S.knob_off, nil, 0, (S.h - 6) * 0.5, 0, nil, 0, L)
  end
end

local function draw_segment(cmds, r, rx, cyc, ctrl_w)
  local S = UI.settings.controls.segment
  local sel = index_value(r)
  panel(cmds, rx, cyc, ctrl_w, S.h, S.bg, nil, 0, S.radius, 1, S.border, 0, L)
  local n = #r.options
  local cw = (ctrl_w - S.pad * 2) / n
  for i = 1, n do
    local x = rx + S.pad + (i - 1) * cw
    local active = (i - 1) == sel
    if active then
      panel(cmds, x + 1, cyc + S.pad, cw - 2, S.h - S.pad * 2, S.active_top, S.active_bot, 1, S.radius - 1, 0, nil, 0, L)
    end
    text(cmds, x + cw * 0.5, cyc + (S.h - S.label_size) * 0.5, r.options[i], S.label_size, active and S.active_label or S.label, "center", "label", L)
  end
end

local function draw_cycler(cmds, r, rx, cyc, ctrl_w)
  local S = UI.settings.controls.cycler
  local sel = index_value(r)
  local bw = 38
  panel(cmds, rx, cyc, bw, 40, S.btn_top, S.btn_bot, 1, 3, 1, S.btn_border, 0, L)
  panel(cmds, rx + ctrl_w - bw, cyc, bw, 40, S.btn_top, S.btn_bot, 1, 3, 1, S.btn_border, 0, L)
  text(cmds, rx + bw * 0.5, cyc + 10, "‹", 18, S.arrow, "center", "label", L)
  text(cmds, rx + ctrl_w - bw * 0.5, cyc + 10, "›", 18, S.arrow, "center", "label", L)
  panel(cmds, rx + bw, cyc, ctrl_w - bw * 2, 40, S.mid, nil, 0, 0, 0, nil, 0, L)
  text(cmds, rx + ctrl_w * 0.5, cyc + (40 - S.label_size) * 0.5, r.options[sel + 1] or "?", S.label_size, S.label, "center", "label", L)
end

local function draw_dropdown(cmds, r, rx, cyc, ctrl_w)
  local S = UI.settings.controls.field
  local sel = index_value(r)
  panel(cmds, rx, cyc, ctrl_w, S.h, S.top, S.bot, 1, S.radius, 1, S.border, 0, L)
  text(cmds, rx + 14, cyc + (S.h - S.label_size) * 0.5, r.options[sel + 1] or "?", S.label_size, S.label, "left", nil, L)
  caret(cmds, rx + ctrl_w - 16, cyc + S.h * 0.5, 9, S.caret, L)
  if ws["dd_" .. r.id] then
    local M2 = UI.settings.controls.menu
    local mh = M2.row_h * #r.options
    local my0 = cyc + S.h + 6
    panel(cmds, rx, my0, ctrl_w, mh, M2.top, M2.bot, 1, M2.radius, 1, M2.border, 0, L + 1)
    for i = 1, #r.options do
      local ry = my0 + (i - 1) * M2.row_h
      local isel = (i - 1) == sel
      if isel then rect(cmds, rx + 4, ry, ctrl_w - 8, M2.row_h, M2.sel_bg, L + 1) end
      text(cmds, rx + 14, ry + (M2.row_h - M2.label_size) * 0.5, r.options[i], M2.label_size, isel and M2.sel_label or M2.label, "left", nil, L + 1)
    end
  end
end

local function draw_static(cmds, r, rx, cyc, ctrl_w)
  local S = UI.settings.controls.field
  panel(cmds, rx, cyc, ctrl_w, S.h, S.top, S.bot, 1, S.radius, 1, S.border, 0, L)
  text(cmds, rx + 14, cyc + (S.h - S.label_size) * 0.5, r.value or "", S.label_size, S.label, "left", nil, L)
  caret(cmds, rx + ctrl_w - 16, cyc + S.h * 0.5, 9, S.caret, L)
end

local function draw_row(cmds, reg, it, sy)
  local S = UI.settings
  local r = it.row
  local rx0 = reg.sx
  text(cmds, rx0, sy + 8, r.name, S.row.name_size, S.row.name_color, "left", nil, L)
  if r.desc then text(cmds, rx0, sy + 8 + S.row.name_size + 2, r.desc, S.row.desc_size, S.row.desc_color, "left", "body", L) end
  local ctrl_w = S.row.ctrl_w
  local rx = rx0 + reg.sw - ctrl_w
  local cyc = sy + (S.row.h - 40) * 0.5
  if r.kind == "slider" then draw_slider(cmds, r, rx, cyc, ctrl_w)
  elseif r.kind == "toggle" then draw_toggle(cmds, r, rx, cyc, ctrl_w)
  elseif r.kind == "segment" then draw_segment(cmds, r, rx, cyc, ctrl_w)
  elseif r.kind == "cycler" then draw_cycler(cmds, r, rx, cyc, ctrl_w)
  elseif r.kind == "dropdown" then draw_dropdown(cmds, r, rx, cyc, ctrl_w)
  elseif r.kind == "static" then draw_static(cmds, r, rx, cyc, ctrl_w) end
  rect(cmds, rx0, sy + S.row.h - 1, reg.sw, 1, S.row.seam, L)
end

local function draw_keycap(cmds, reg, it, sy)
  local S = UI.settings
  local a = it.action
  text(cmds, reg.sx, sy + (it.h - 16) * 0.5, a.label, 16, S.row.name_color, "left", nil, L)
  local kc = S.controls.keycap
  local kx = reg.sx + reg.sw - kc.w
  local ky = sy + (it.h - kc.h) * 0.5
  local rebinding = Model.rebind_action == a.id
  local key = Model["bind_" .. a.id]
  if rebinding then
    panel(cmds, kx, ky, kc.w, kc.h, kc.rebind_top, kc.rebind_bot, 1, kc.radius, 1, kc.rebind_border, 0, L)
    text(cmds, kx + kc.w * 0.5, ky + (kc.h - kc.label_size) * 0.5, "Press a key…", kc.label_size, kc.rebind_label, "center", "label", L)
  elseif key == nil or key == "" then
    panel(cmds, kx, ky, kc.w, kc.h, kc.unbound_bg, nil, 0, kc.radius, 1, kc.unbound_border, 0, L)
    text(cmds, kx + kc.w * 0.5, ky + (kc.h - kc.label_size) * 0.5, "‹ unbound ›", kc.label_size, kc.unbound_label, "center", "label", L)
  else
    panel(cmds, kx, ky, kc.w, kc.h, kc.top, kc.bot, 1, kc.radius, 1, kc.border, 0, L)
    text(cmds, kx + kc.w * 0.5, ky + (kc.h - kc.label_size) * 0.5, tostring(key), kc.label_size, kc.label, "center", "label", L)
  end
end

-- ── draw ───────────────────────────────────────────────────────────
function M.draw(sw, sh)
  local S = UI.settings
  local l = layout(sw, sh)
  local cmds = {}

  -- scrim + window (shadow, panel), at layer 0
  rect(cmds, 0, 0, sw, sh, UI.theme.tokens.scrim, 0)
  local win = S.window
  local g = win.shadow_grow
  panel(cmds, l.x - g, l.y - g + win.shadow_dy, l.w + 2 * g, l.h + 2 * g, win.shadow, nil, 0, win.radius + g, 0, nil, win.shadow_feather, 0)
  panel(cmds, l.x, l.y, l.w, l.h, win.fill_top, win.fill_bot, 1, win.radius, 1, win.border, 0, 0)

  -- rune-inlay corner dots
  local ru = S.runes
  local ins = ru.inset
  panel(cmds, l.x + ins, l.y + ins, ru.size, ru.size, ru.top, nil, 0, ru.size, 0, nil, 0, L)
  panel(cmds, l.r - ins - ru.size, l.y + ins, ru.size, ru.size, ru.top, nil, 0, ru.size, 0, nil, 0, L)
  panel(cmds, l.x + ins, l.b - ins - ru.size, ru.size, ru.size, ru.bot, nil, 0, ru.size, 0, nil, 0, L)
  panel(cmds, l.r - ins - ru.size, l.b - ins - ru.size, ru.size, ru.size, ru.bot, nil, 0, ru.size, 0, nil, 0, L)

  -- ── title bar ──
  local tbar = S.titlebar
  panel(cmds, l.x, l.y, l.w, l.tb, with_alpha(tbar.wash, tbar.wash[4]), with_alpha(tbar.wash, 0), 1, win.radius, 0, nil, 0, L)
  rect(cmds, l.x, l.y + l.tb - 1, l.w, 1, tbar.seam, L)
  panel(cmds, l.x + tbar.pad_x, l.y + l.tb * 0.5 - 4, 8, 8, tbar.mark, nil, 0, 2, 0, nil, 0, L)
  text(cmds, l.x + tbar.pad_x + 18, l.y + (l.tb - tbar.title_size) * 0.5, tbar.title, tbar.title_size, tbar.title_color, "left", "display", L)
  local tbc = tbar.close
  local cx0 = l.r - tbar.pad_x - tbc.size
  local cy0 = l.y + (l.tb - tbc.size) * 0.5
  text(cmds, cx0 - 16, l.y + (l.tb - tbar.hint_size) * 0.5, tbar.hint, tbar.hint_size, tbar.hint_color, "right", "label", L)
  panel(cmds, cx0, cy0, tbc.size, tbc.size, tbc.bg, nil, 0, tbc.radius, 1, tbc.border, 0, L)
  text(cmds, cx0 + tbc.size * 0.5, cy0 + tbc.size * 0.5 - 8, "×", 16, tbc.label, "center", nil, L)

  -- ── nav rail ──
  local nav = S.nav
  rect(cmds, l.x + l.nav_w - 1, l.body_y, 1, l.body_h, nav.seam, L)
  text(cmds, l.x + nav.pad + 8, l.body_y + nav.pad, nav.header, nav.header_size, nav.header_color, "left", "label", L)
  for _, n in ipairs(nav_rects(l)) do
    local active = section == n.id
    local it = n.item
    if active then
      panel(cmds, n.x, n.y, n.w, n.h, nav.active_top, nav.active_bot, 2, 3, 0, nil, 0, L)
      rect(cmds, n.x, n.y, 2, n.h, nav.active_border, L)
    end
    local gem = with_alpha(it.gem, active and 1.0 or 0.5)
    panel(cmds, n.x + 13, n.y + n.h * 0.5 - nav.gem * 0.5, nav.gem, nav.gem, gem, nil, 0, nav.gem * 0.5, 0, nil, 0, L)
    text(cmds, n.x + 13 + nav.gem + 12, n.y + (n.h - nav.label_size) * 0.5, it.label, nav.label_size, active and nav.active_label or nav.inactive_label, "left", "label", L)
  end
  -- nav footer block
  local nfy = l.footer_y - 52
  rect(cmds, l.x + nav.pad + 8, nfy, l.nav_w - nav.pad * 2 - 16, 1, UI.theme.tokens.hairline, L)
  text(cmds, l.x + nav.pad + 8, nfy + 12, nav.footer_title, nav.header_size, nav.footer_title_color, "left", "label", L)
  text(cmds, l.x + nav.pad + 8, nfy + 26, nav.footer_sub, 12, nav.footer_sub_color, "left", "body", L)

  -- ── content header ──
  local reg = content_regions(l)
  local navit
  for _, ni in ipairs(S.nav_items) do if ni.id == section then navit = ni end end
  text(cmds, reg.hx, reg.hy, navit.kicker, S.content.kicker_size, navit.kicker_color, "left", "label", L)
  text(cmds, reg.hx, reg.hy + 16, navit.title, S.content.title_size, S.content.title_color, "left", "display", L)
  rect(cmds, l.cx, reg.head_bottom, l.cw, 1, S.content.seam, L)

  -- input sub-tab pills (in the header, right)
  if section == "input" then
    local P = S.controls.pill
    local tabs = S.input.tabs
    local pw = 108
    local tx = l.r - S.content.pad_x - pw * #tabs - P.pad
    panel(cmds, tx - P.pad, reg.hy + 8 - P.pad, pw * #tabs + P.pad * 2, P.h + P.pad * 2, P.bg, nil, 0, P.radius, 1, P.border, 0, L)
    for i, t in ipairs(tabs) do
      local rx = tx + (i - 1) * pw
      local active = input_tab == t.id
      if active then panel(cmds, rx, reg.hy + 8, pw, P.h, P.active_top, P.active_bot, 1, P.radius, 0, nil, 0, L) end
      text(cmds, rx + pw * 0.5, reg.hy + 8 + (P.h - P.label_size) * 0.5, t.label, P.label_size, active and P.active_label or P.label, "center", "label", L)
    end
  end

  -- ── scrollable content ──
  local items = build_items()
  local total = items_total_h(items)
  local maxscroll = math.max(0, total - reg.sh)
  scroll = clamp(scroll, 0, maxscroll)
  local yy = reg.sy - scroll
  for _, it in ipairs(items) do
    local sy = yy
    yy = yy + it.h
    if sy + it.h > reg.sy and sy < reg.sy + reg.sh then
      if it.kind == "group" then
        text(cmds, reg.sx, sy + 10, it.name, S.row.group_size, S.row.group_color, "left", "label", L)
        rect(cmds, reg.sx + 120, sy + 10 + S.row.group_size * 0.5, reg.sw - 120, 1, UI.theme.tokens.hairline, L)
      elseif it.kind == "row" then
        draw_row(cmds, reg, it, sy)
      elseif it.kind == "key" then
        draw_keycap(cmds, reg, it, sy)
      elseif it.kind == "banner" then
        local st = S.audio.stub
        panel(cmds, reg.sx, sy, reg.sw, 58, with_alpha(st.top, st.top[4]), with_alpha(st.top, 0), 1, 3, 1, st.border, 0, L)
        text(cmds, reg.sx + 18, sy + 12, st.title, 10, st.title_color, "left", "label", L)
        text(cmds, reg.sx + 18, sy + 28, st.body, 14, st.body_color, "left", "body", L)
      elseif it.kind == "rebind" then
        local rb = S.rebind_banner
        panel(cmds, reg.sx, sy, reg.sw, 40, rb.top, rb.bot, 1, 3, 1, rb.border, 0, L)
        text(cmds, reg.sx + 16, sy + 12, rb.text .. "  ·  " .. rb.text2, 15, rb.text_color, "left", "body", L)
      elseif it.kind == "empty" then
        local c = S.input.controller
        local cxm = l.cx + l.cw * 0.5
        panel(cmds, cxm - 32, sy + 30, 64, 64, c.ring_bg, nil, 0, 32, 2, c.ring, 0, L)
        text(cmds, cxm, sy + 112, c.title, c.title_size, c.title_color, "center", "display", L)
        text(cmds, cxm, sy + 148, c.body, 15, c.body_color, "center", "body", L)
        -- detect button + enable toggle
        local dw, dh = 200, 40
        panel(cmds, cxm - dw * 0.5, sy + 190, dw, dh, S.controls.field.top, S.controls.field.bot, 1, 3, 1, S.controls.field.border, 0, L)
        text(cmds, cxm, sy + 190 + (dh - 12) * 0.5, c.detect, 12, S.row.name_color, "center", "label", L)
      end
    end
  end

  -- scrollbar
  if maxscroll > 0 then
    local vh = reg.sh
    local th = math.max(28, vh * (vh / total))
    local ty = reg.sy + (scroll / maxscroll) * (vh - th)
    rect(cmds, reg.sx + reg.sw + 12, reg.sy, 4, vh, UI.theme.tokens.well, L)
    panel(cmds, reg.sx + reg.sw + 12, ty, 4, th, S.content.scrollbar, nil, 0, 2, 0, nil, 0, L)
  end

  -- audio dim overlay (the whole content, minus the notice)
  if section == "audio" then
    -- (kept subtle: the notice already says it's a preview)
  end

  -- ── footer ──
  local fy = l.footer_y
  local F = S.footer
  rect(cmds, l.x, fy, l.w, 1, F.seam, L)
  text(cmds, l.x + F.pad_x, fy + (l.fb - 11) * 0.5, F.restore, 11, F.restore_color, "left", "label", L)
  if applied > 0 then
    text(cmds, l.x + F.pad_x + 170, fy + (l.fb - 11) * 0.5, F.applied, 11, F.applied_color, "left", "label", L)
  end
  local bw, aw, bh = 96, 104, 34
  local ax = l.r - F.pad_x - aw
  local bx = ax - 12 - bw
  local by = fy + (l.fb - bh) * 0.5
  local V = UI.modal.buttons.variants
  local vb = V.secondary
  panel(cmds, bx, by, bw, bh, vb.fill_top, vb.fill_bot, 1, 3, 1, vb.border, 0, L)
  text(cmds, bx + bw * 0.5, by + (bh - 14) * 0.5, F.back, 14, vb.label, "center", "label", L)
  local vp = V.primary
  panel(cmds, ax, by, aw, bh, vp.fill_top, vp.fill_bot, 1, 3, 1, vp.border, 0, L)
  text(cmds, ax + aw * 0.5, by + (bh - 14) * 0.5, F.apply, 14, vp.label, "center", "label", L)

  return cmds
end

-- The flash timer decays via the shell's dt through Model (best-effort: decay a
-- little each draw; the shell also clears it). Kept simple.
local function tick() if applied > 0 then applied = applied - 0.016 end end
local _draw = M.draw
M.draw = function(sw, sh) tick(); return _draw(sw, sh) end

return M
