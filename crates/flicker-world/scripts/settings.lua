-- Settings workbench: a framed centred panel with a left nav (Camera / Audio /
-- Video / Key Mappings) and a right content pane of data-driven controls
-- (sliders / toggles / dropdowns from `UI.settings.pages[<page>].controls`).
-- Each control's value is `Model["<page>_<id>"]`; changed values are returned
-- under the same key. Active page is kept here; Back returns `{ back = true }`.

local M = {}

local active = 1
local ws = {}

-- draw helpers ---------------------------------------------------------------

local function rect(cmds, x, y, w, h, c)
  cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = c[1], g = c[2], b = c[3], a = c[4] }
end

local function text(cmds, x, y, str, size, c)
  cmds[#cmds + 1] = { kind = "text", x = x, y = y, text = str, size = size, r = c[1], g = c[2], b = c[3], a = c[4] }
end

local function text_c(cmds, x, y, str, size, c)
  cmds[#cmds + 1] = { kind = "text", x = x, y = y, text = str, size = size, align = "center", r = c[1], g = c[2], b = c[3], a = c[4] }
end

local function border(cmds, x, y, w, h, c)
  rect(cmds, x, y, w, 2, c)
  rect(cmds, x, y + h - 2, w, 2, c)
  rect(cmds, x, y, 2, h, c)
  rect(cmds, x + w - 2, y, 2, h, c)
end

local function checkbox(cmds, r, on, st)
  rect(cmds, r.x, r.y, st.size, st.size, st.box)
  border(cmds, r.x, r.y, st.size, st.size, st.border)
  if on then
    rect(cmds, r.x + 4, r.y + 4, st.size - 8, st.size - 8, st.check)
  end
end

local function point_in(px, py, x, y, w, h)
  return px >= x and px <= x + w and py >= y and py <= y + h
end

-- layout ---------------------------------------------------------------------

local function nav()
  return UI.settings.nav
end

local function active_page()
  local n = nav()[active]
  return n and n.id or "camera"
end

local function panel_xy(sw, sh)
  local p = UI.settings.panel
  return math.floor((sw - p.w) * 0.5), math.floor((sh - p.h) * 0.5)
end

local function nav_rect(px, py, i)
  local ns = UI.settings.nav_style
  return { x = px + ns.x, y = py + ns.y + (i - 1) * ns.row_h, w = ns.w, h = ns.row_h }
end

local function back_rect(px, py)
  local b = UI.settings.back
  return { x = px + b.x, y = py + b.y, w = b.w, h = b.h }
end

local function controls(page)
  local p = UI.settings.pages[page]
  return p and p.controls or nil
end

local function row_y(px, py, i)
  local c = UI.settings.content
  return py + c.y + (i - 1) * c.row_h
end

local function widget_rect(px, py, i, ctl)
  local c = UI.settings.content
  local y = row_y(px, py, i)
  local x = px + c.x + c.widget_x
  if ctl.kind == "slider" then
    return { x = x, y = y + 10, w = c.widget_w, h = 8 }
  elseif ctl.kind == "toggle" then
    local sz = UI.settings.checkbox.size
    return { x = x, y = y + (c.row_h - sz) * 0.5, w = sz, h = sz }
  else
    return { x = x, y = y + 3, w = c.widget_w, h = 22 }
  end
end

-- interaction ----------------------------------------------------------------

function M.update(mx, my, clicked, sw, sh, down)
  if not (UI and Widgets) then
    return {}
  end
  local px, py = panel_xy(sw, sh)
  local s = {}

  -- While a rebind overlay is up, a click cancels and nothing else processes.
  if Model and Model.rebind_active then
    if clicked then
      s.settings_rebind_cancel = true
    end
    return s
  end

  for i = 1, #nav() do
    local r = nav_rect(px, py, i)
    if clicked and point_in(mx, my, r.x, r.y, r.w, r.h) then
      active = i
    end
  end

  local br = back_rect(px, py)
  if clicked and point_in(mx, my, br.x, br.y, br.w, br.h) then
    s.back = true
  end

  local page = active_page()
  local cs = controls(page)
  if cs then
    for i, ctl in ipairs(cs) do
      local key = page .. "_" .. ctl.id
      local w = widget_rect(px, py, i, ctl)
      if ctl.kind == "slider" then
        s[key] = Widgets.slider_update(ws, key, w, mx, my, clicked, down, Model[key] or 0, ctl.min, ctl.max)
      elseif ctl.kind == "toggle" then
        local v = Model[key]
        if clicked and point_in(mx, my, w.x, w.y, w.w, w.h) then
          v = not v
        end
        s[key] = v and true or false
      elseif ctl.kind == "choice" then
        local cur = math.floor((Model[key] or 0) + 0.5) + 1
        local sel = Widgets.dropdown_update(ws, key, w, mx, my, clicked, #ctl.options, cur)
        s[key] = sel - 1
      end
    end
  elseif page == "keymap" then
    local km = UI.settings.pages.keymap
    local c = UI.settings.content
    local cw = UI.settings.panel.w - c.x - 24
    for i, act in ipairs(km.actions) do
      local ry = py + c.y + (i - 1) * km.row_h
      if clicked and point_in(mx, my, px + c.x, ry, cw, km.row_h) then
        s.settings_rebind_active = true
        s.settings_rebind_action = act.id
      end
    end
  end

  return s
end

-- rendering ------------------------------------------------------------------

function M.draw(sw, sh)
  if not (UI and Widgets) then
    return {}
  end
  local set = UI.settings
  local px, py = panel_xy(sw, sh)
  local cmds = {}

  rect(cmds, 0, 0, sw, sh, { 0.04, 0.05, 0.07, 0.7 })
  rect(cmds, px, py, set.panel.w, set.panel.h, set.panel.bg)
  border(cmds, px, py, set.panel.w, set.panel.h, set.panel.border)

  local t = set.title
  text_c(cmds, px + set.panel.w * 0.5, py + t.offset_y, t.text, t.size, t.color)
  rect(cmds, px + set.content.x - 16, py + 70, 1, set.panel.h - 122, set.nav_style.rule)

  local ns = set.nav_style
  for i, item in ipairs(nav()) do
    local r = nav_rect(px, py, i)
    if i == active then
      rect(cmds, r.x, r.y, r.w, r.h, ns.active_bg)
    end
    local col = (i == active) and ns.active or ns.label
    text(cmds, r.x + 12, r.y + (r.h - ns.label_size) * 0.5, item.label, ns.label_size, col)
  end

  local c = set.content
  local page = active_page()
  local cs = controls(page)
  if cs then
    for i, ctl in ipairs(cs) do
      local key = page .. "_" .. ctl.id
      local ry = row_y(px, py, i)
      text(cmds, px + c.x + c.label_x, ry + (c.row_h - c.label_size) * 0.5, ctl.label, c.label_size, c.label_color)
      local w = widget_rect(px, py, i, ctl)
      if ctl.kind == "slider" then
        Widgets.slider_draw(cmds, w, Model[key] or 0, ctl.min, ctl.max, set.slider_style)
        text(cmds, px + c.x + c.value_x, ry + (c.row_h - c.label_size) * 0.5, string.format(ctl.fmt or "%.2f", Model[key] or 0), c.label_size, c.value_color)
      elseif ctl.kind == "toggle" then
        checkbox(cmds, w, Model[key], set.checkbox)
      elseif ctl.kind == "choice" then
        local cur = math.floor((Model[key] or 0) + 0.5) + 1
        Widgets.dropdown_draw(cmds, ws, key, w, ctl.options, cur, set.dropdown_style)
      end
    end
  elseif page == "keymap" then
    local km = set.pages.keymap
    local cw = set.panel.w - c.x - 24
    for i, act in ipairs(km.actions) do
      local ry = py + c.y + (i - 1) * km.row_h
      if not Model.rebind_active and point_in(Model.mx or -1, Model.my or -1, px + c.x, ry, cw, km.row_h) then
        rect(cmds, px + c.x, ry, cw, km.row_h, km.hot_bg)
      end
      text(cmds, px + c.x + km.label_x, ry + (km.row_h - km.label_size) * 0.5, act.label, km.label_size, km.label_color)
      text(cmds, px + c.x + km.bind_x, ry + (km.row_h - km.label_size) * 0.5, Model["bind_" .. act.id] or "-", km.label_size, km.bind_color)
    end
    if km.hint then
      text(cmds, px + c.x + km.label_x, py + c.y + #km.actions * km.row_h + 12, km.hint, c.label_size, c.value_color)
    end
    if Model.rebind_active then
      local ro = km.rebind_overlay
      rect(cmds, 0, 0, sw, sh, { 0, 0, 0, 0.6 })
      local ox = math.floor((sw - ro.w) * 0.5)
      local oy = math.floor((sh - ro.h) * 0.5)
      rect(cmds, ox, oy, ro.w, ro.h, ro.bg)
      border(cmds, ox, oy, ro.w, ro.h, ro.border)
      text_c(cmds, ox + ro.w * 0.5, oy + 26, "Press a key or mouse button...", ro.title_size, ro.title_color)
      text_c(cmds, ox + ro.w * 0.5, oy + 56, Model.rebind_action or "", ro.title_size, km.bind_color)
      text_c(cmds, ox + ro.w * 0.5, oy + ro.h - 28, "Click to cancel", ro.hint_size, ro.hint_color)
    end
  else
    local note = (set.pages[page] or {}).note or ""
    text(cmds, px + c.x + c.label_x, py + c.y + 4, note, c.label_size, c.value_color)
  end

  local b = set.back
  local br = back_rect(px, py)
  local hot = point_in(Model.mx or -1, Model.my or -1, br.x, br.y, br.w, br.h)
  rect(cmds, br.x, br.y, br.w, br.h, hot and b.hot or b.cell)
  text_c(cmds, br.x + br.w * 0.5, br.y + (br.h - b.label_size) * 0.5, b.label, b.label_size, b.label_color)

  return cmds
end

return M
