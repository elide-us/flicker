-- In-scene HUD for flicker-paperdoll, driven by Lua + `ui_elements.json` (the
-- `UI.paperdoll` section) and the engine `Model` (live values published each frame).
-- Layout / colours / labels live in the JSON; this script owns only behaviour.
--
-- Genuine INPUT stays on the keyboard and is not UI: the orbit camera (drag/wheel), and
-- while ANIMATING the state-machine drivers — WASD move, Shift run, C crouch, Space jump,
-- R reset, up/down cycle states. Attack is the one gameplay trigger exposed here, as a
-- button (below the view toggles); hit/death are not exposed.
--
-- Two-way widget contract, per widgets.lua: the current value comes from the Model,
-- the new value goes back in `states`, and the engine applies it — so the engine stays
-- the single source of truth and this script holds no duplicate state.
--
-- INVENTORY: one cell per equip slot. A cell is lit while its piece is worn, and drawn
-- dim + inert when the slot has no piece loaded (`Model.slot_<key>_loaded` false), so
-- the bar shows the whole slot set before the art arrives. Sheaths deliberately have no
-- cell — they are always worn (there are no sheath/unsheath animations yet).

local M = {}

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

local function mnum(key, dflt)
  local v = Model and Model[key]
  if type(v) == "number" then
    return v
  end
  return dflt
end

local function mbool(key)
  return Model and Model[key] == true
end

local function mtext(key)
  local v = Model and Model[key]
  return type(v) == "string" and v or ""
end

-- The fit rows come from UI.paperdoll.fit.rows — the SAME list the engine cycles focus and
-- nudges from, so layout and keyboard can't drift apart.
local function fit_rows()
  return UI.paperdoll.fit.rows
end

-- A slider's real hit region. `widgets.slider_update` grabs on a PADDED box (r.y-6,
-- r.h+12), not the drawn track — so anything else hit-testing a slider must use the same
-- box or it will disagree with the drag.
-- A row's range comes from `scale` or `offset` in the JSON, keyed off the row name — one
-- lookup shared by update and draw so they cannot disagree about a slider's mapping.
-- A row's range/nudge block, named by its own `group`.
local function fit_range(f, row)
  local g = f[row.group]
  return { min = g.min, max = g.max, dflt = (row.group == "offset") and 0 or 1 }
end

local SLIDER_GRAB_PAD = 6
local function grab_rect(r)
  return { x = r.x, y = r.y - SLIDER_GRAB_PAD, w = r.w, h = r.h + SLIDER_GRAB_PAD * 2 }
end

-- The inventory bar: a centred row of square cells along the bottom.
local function inventory_rects(sw, sh)
  local inv = UI.paperdoll.inventory
  local n = #inv.slots
  local bar_w = n * inv.slot + (n - 1) * inv.gap
  local panel = {
    x = (sw - bar_w) * 0.5 - inv.pad,
    y = sh - inv.margin_b - inv.slot - inv.pad,
    w = bar_w + inv.pad * 2,
    h = inv.slot + inv.pad * 2,
  }
  local cells = {}
  for i = 1, n do
    cells[i] = {
      x = panel.x + inv.pad + (i - 1) * (inv.slot + inv.gap),
      y = panel.y + inv.pad,
      w = inv.slot,
      h = inv.slot,
    }
  end
  return inv, panel, cells
end

-- The view toggles (a checkbox column top-left) plus the Attack button under them.
local function view_rects(sw, sh)
  local v = UI.paperdoll.view
  local boxes = {}
  for i = 1, #v.items do
    boxes[i] = { x = v.margin, y = v.top + (i - 1) * v.row_h, w = v.box, h = v.box }
  end
  local a = v.attack
  local atk = { x = v.margin, y = v.top + #v.items * v.row_h + a.gap_y, w = a.w, h = a.h }
  return v, boxes, atk
end

-- The fit gadget: a right-hand panel, shown only while a piece is selected. Returns the
-- panel rect plus each row's widget rect.
local function fit_rects(sw, sh)
  local f = UI.paperdoll.fit
  local rows = #fit_rows() + 1 -- one per fit row, + record
  local status_h = f.status.size + f.status.gap_y * 2
  local h = f.pad * 2 + f.header_size + 6 + rows * f.row_h + status_h
  local panel = { x = sw - f.margin - f.w, y = f.margin, w = f.w, h = h }
  local x = panel.x + f.pad + f.label_w
  local w = f.w - f.pad * 2 - f.label_w - f.value_w
  local y0 = panel.y + f.pad + f.header_size + 6
  local r = {}
  local rowr = {}
  for i = 1, #fit_rows() do
    r[i] = { x = x, y = y0 + (i - 1) * f.row_h + (f.row_h - f.slider_h) * 0.5, w = w, h = f.slider_h }
    -- The WHOLE row is the focus target — label, track and value. Clicking the track also
    -- DRAGS it, so focusing via the track meant losing the value you had just set; clicking
    -- the label focuses without touching it (user, 2026-07-16).
    rowr[i] = { x = panel.x + f.pad, y = y0 + (i - 1) * f.row_h, w = f.w - f.pad * 2, h = f.row_h }
  end
  local rec = { x = panel.x + f.pad, y = y0 + #fit_rows() * f.row_h, w = f.w - f.pad * 2, h = f.record.h }
  return f, panel, r, rec, y0, rowr
end

-- Widget state for slider drags, keyed by a stable id (see widgets.lua).
local widget_state = {}

function M.update(mx, my, clicked, sw, sh, down)
  if not UI or not UI.paperdoll or not Widgets then
    return {}
  end
  local states = {}
  -- Panels the cursor is over. The engine reads this to know a click was CONSUMED by the
  -- HUD and must not also pick through to the scene behind it.
  local over = false

  -- Inventory cells. A slot with no piece loaded is inert, so a stray click on an
  -- empty cell cannot equip something that does not exist.
  local inv, inv_panel, cells = inventory_rects(sw, sh)
  if point_in(mx, my, inv_panel) then
    over = true
  end
  for i, slot in ipairs(inv.slots) do
    local on = mbool("slot_" .. slot.key .. "_on")
    if mbool("slot_" .. slot.key .. "_loaded") then
      on = Widgets.checkbox_update(cells[i], mx, my, clicked, on)
    end
    states["slot_" .. slot.key] = on
  end

  -- View toggles.
  local v, boxes, atk = view_rects(sw, sh)
  for i, item in ipairs(v.items) do
    states[item.key] = Widgets.checkbox_update(boxes[i], mx, my, clicked, mbool(item.key))
  end
  -- Attack: a one-shot trigger, only meaningful while animating (it drives a runtime state).
  if mbool("animate") then
    if point_in(mx, my, atk) then over = true end
    states.attack = Widgets.button_update(atk, mx, my, clicked)
  end

  -- Fit gadget — only while a piece is selected, so the sliders can never edit a phantom.
  if mbool("fit_active") then
    local f, panel, r, rec, _, rowr = fit_rects(sw, sh)
    if point_in(mx, my, panel) then
      over = true
    end
    -- FOCUS: clicking a slider focuses it; the ENGINE then nudges the focused value with
    -- ←/→ (this script never sees the keyboard). Focus persists until another slider is
    -- clicked, so a nudge doesn't need the cursor held anywhere in particular.
    local focus = mtext("fit_focus")
    for i, row in ipairs(fit_rows()) do
      -- Focus on the WHOLE row (label included), not just the track — see `fit_rects`.
      -- The row covers the track's padded grab region, so the two can no longer disagree.
      if clicked and (point_in(mx, my, rowr[i]) or point_in(mx, my, grab_rect(r[i]))) then
        focus = row.key
      end
    end
    states.fit_focus = focus

    for i, row in ipairs(fit_rows()) do
      local g = fit_range(f, row)
      states[row.key] = Widgets.slider_update(widget_state, row.key, r[i], mx, my, clicked, down,
        mnum(row.key, g.dflt), g.min, g.max)
    end
    states.fit_record = Widgets.button_update(rec, mx, my, clicked)
  end

  -- A slider drag CAPTURES the mouse: widgets.lua keeps `widget_state[id]` set until the
  -- button releases, so report capture for the whole drag. Reporting only hover would let
  -- the orbit camera grab the moment the cursor left the panel mid-drag (user, 2026-07-16).
  for _, held in pairs(widget_state) do
    if held then
      over = true
      break
    end
  end
  states.hud_hit = over
  return states
end

function M.draw(sw, sh)
  local cmds = {}
  if not UI or not UI.paperdoll or not Widgets then
    return cmds
  end

  -- ── Inventory bar ──
  local inv, panel, cells = inventory_rects(sw, sh)
  Widgets.panel_draw(cmds, panel, { bg = inv.panel_bg, border = inv.panel_border })
  cmds[#cmds + 1] = {
    kind = "text",
    x = panel.x + inv.pad,
    y = panel.y - inv.header_size - 4,
    text = inv.header,
    size = inv.header_size,
    align = "left",
    r = inv.header_color[1],
    g = inv.header_color[2],
    b = inv.header_color[3],
    a = inv.header_color[4],
  }
  for i, slot in ipairs(inv.slots) do
    local loaded = mbool("slot_" .. slot.key .. "_loaded")
    local on = mbool("slot_" .. slot.key .. "_on")
    -- `hot` carries the lit (worn) state; an empty slot uses the dim style and can
    -- never read as lit.
    Widgets.button_draw(cmds, cells[i], slot.label, loaded and inv.cell or inv.empty, loaded and on)
  end

  -- ── View toggles ──
  local v, boxes = view_rects(sw, sh)
  for i, item in ipairs(v.items) do
    Widgets.checkbox_draw(cmds, boxes[i], mbool(item.key), v.checkbox)
    cmds[#cmds + 1] = {
      kind = "text",
      x = v.margin + v.label_x,
      y = boxes[i].y + (v.box - v.label_size) * 0.5,
      text = item.label,
      size = v.label_size,
      align = "left",
      r = v.label_color[1],
      g = v.label_color[2],
      b = v.label_color[3],
      a = v.label_color[4],
    }
  end
  if mbool("animate") then
    local _, _, atk = view_rects(sw, sh)
    Widgets.button_draw(cmds, atk, v.attack.label, v.attack.style, false)
  end

  -- ── Fit gadget (only while a piece is selected) ──
  if mbool("fit_active") then
    local f, panel, r, rec, y0 = fit_rects(sw, sh)
    Widgets.panel_draw(cmds, panel, { bg = f.panel_bg, border = f.panel_border })
    local function txt(x, y, str, size, c, align)
      cmds[#cmds + 1] = { kind = "text", x = x, y = y, text = str, size = size, align = align or "left",
                          r = c[1], g = c[2], b = c[3], a = c[4] }
    end
    -- Header names the piece AND the bone it hangs off, so it is never ambiguous which
    -- thing the sliders are moving.
    txt(panel.x + f.pad, panel.y + f.pad, f.header .. " · " .. (Model.fit_name or ""),
      f.header_size, f.header_color)
    txt(panel.x + panel.w - f.pad, panel.y + f.pad, "@" .. (Model.fit_bone or ""),
      f.value_size, f.value_color, "right")

    local rows = {}
    for i, row in ipairs(fit_rows()) do
      local g = fit_range(f, row)
      rows[i] = { key = row.key, label = row.label, min = g.min, max = g.max,
                  fmt = (row.group == "rotate") and "%+.0f\u{00B0}"
                        or (row.group == "offset") and "%+.1f" or "%.2f" }
    end
    local focus = mtext("fit_focus")
    for i, row in ipairs(rows) do
      local val = mnum(row.key, 0)
      local cy = y0 + (i - 1) * f.row_h
      local hot = (row.key == focus)
      txt(panel.x + f.pad, cy + (f.row_h - f.label_size) * 0.5, row.label, f.label_size,
        hot and f.focus_label or f.label_color)
      -- The focused row reads brighter so it is obvious which one ←/→ will move.
      local st = f.slider
      if hot then
        st = { track = f.focus_track, fill = f.focus_fill, handle = f.slider.handle,
               handle_w = f.slider.handle_w }
      end
      Widgets.slider_draw(cmds, r[i], val, row.min, row.max, st)
      txt(panel.x + panel.w - f.pad, cy + (f.row_h - f.value_size) * 0.5,
        string.format(row.fmt, val), f.value_size, f.value_color, "right")
    end
    Widgets.button_draw(cmds, rec, f.record.label, f.button, false)
    -- Record confirmation. The engine blanks it after a few seconds, so an old "saved" can
    -- never be mistaken for a fresh one. Red on failure — a silent failed write is worse
    -- than no button at all.
    local note = mtext("fit_status")
    if note ~= "" then
      local err = note:sub(1, 4) == "SAVE"
      txt(panel.x + f.pad, rec.y + rec.h + f.status.gap_y, note, f.status.size,
        err and f.status.err or f.status.ok)
    end
  end

  -- ── Stat readout (was the POC's draw_text line) ──
  local s = UI.paperdoll.stats
  cmds[#cmds + 1] = {
    kind = "text",
    x = s.margin,
    y = s.top,
    text = (Model and Model.stats) or "",
    size = s.size,
    align = "left",
    r = s.color[1],
    g = s.color[2],
    b = s.color[3],
    a = s.color[4],
  }
  return cmds
end

return M
