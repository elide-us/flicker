-- Generic gothic modal component, driven by ui_elements.json.
--
-- One script renders the menu, the pause overlay, and the confirm dialog: they
-- share the `UI.modal` style (panel / title / subtitle / buttons) and differ
-- only in their `UI.screens[Model.screen]` instance (overlay colour, title,
-- button items) plus an optional `Model.subtitle` for dynamic text (the confirm
-- countdown). The host selects the screen by publishing `Model.screen`. This
-- script owns only behaviour — centring, hit-testing, hover; everything visual
-- is data. Returns `{ <item id> = true }` as a momentary action on click.

local M = {}

-- Hovered item index this frame (set in update, read in draw).
local hover = nil

local function screen()
  return Model and UI and UI.screens[Model.screen]
end

-- Centre the shared panel and lay out the current screen's buttons.
local function layout(sw, sh)
  local panel = UI.modal.panel
  local px = math.floor((sw - panel.w) * 0.5 + 0.5)
  local py = math.floor((sh - panel.h) * 0.5 + 0.5)
  local b = UI.modal.buttons
  local bx = px + (panel.w - b.w) * 0.5
  local items = {}
  for i, item in ipairs(screen().items) do
    items[i] = {
      id = item.id,
      label = item.label,
      x = bx,
      y = py + b.first_y + (i - 1) * b.gap_y,
      w = b.w,
      h = b.h,
    }
  end
  return { px = px, py = py, pw = panel.w, ph = panel.h, items = items }
end

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

function M.update(mx, my, clicked, sw, sh)
  if not screen() then
    return {}
  end
  local l = layout(sw, sh)
  hover = nil
  for i, it in ipairs(l.items) do
    if point_in(mx, my, it) then
      hover = i
    end
  end
  local actions = {}
  if clicked and hover then
    actions[l.items[hover].id] = true
  end
  return actions
end

local function unpack_color(c)
  return c[1], c[2], c[3], c[4]
end

local function rect(cmds, x, y, w, h, c)
  local r, g, b, a = unpack_color(c)
  cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = r, g = g, b = b, a = a }
end

local function outline(cmds, rr, t, c)
  rect(cmds, rr.x, rr.y, rr.w, t, c)
  rect(cmds, rr.x, rr.y + rr.h - t, rr.w, t, c)
  rect(cmds, rr.x, rr.y, t, rr.h, c)
  rect(cmds, rr.x + rr.w - t, rr.y, t, rr.h, c)
end

local function centered(cmds, x, y, str, size, c)
  local r, g, b, a = unpack_color(c)
  cmds[#cmds + 1] =
    { kind = "text", x = x, y = y, text = str, size = size, align = "center", r = r, g = g, b = b, a = a }
end

function M.draw(sw, sh)
  if not screen() then
    return {}
  end
  local s = screen()
  local m = UI.modal
  local l = layout(sw, sh)
  local cmds = {}

  -- Overlay (backdrop / scrim / dim — a full-screen tinted rect).
  rect(cmds, 0, 0, sw, sh, s.overlay)

  -- Panel + title.
  cmds[#cmds + 1] =
    { kind = "sprite", tex = Textures[m.panel.texture], x = l.px, y = l.py, w = l.pw, h = l.ph }
  centered(cmds, l.px + l.pw * 0.5, l.py + m.title.offset_y, s.title, m.title.size, m.title.color)

  -- Optional dynamic subtitle (e.g. the confirm countdown).
  if Model.subtitle and m.subtitle then
    centered(
      cmds,
      l.px + l.pw * 0.5,
      l.py + m.subtitle.offset_y,
      Model.subtitle,
      m.subtitle.size,
      m.subtitle.color
    )
  end

  -- Buttons + centred labels (with hover sheen + gold outline).
  local b = m.buttons
  for i, it in ipairs(l.items) do
    cmds[#cmds + 1] =
      { kind = "sprite", tex = Textures[b.texture], x = it.x, y = it.y, w = it.w, h = it.h }
    local label_color = b.label_color
    if hover == i then
      rect(cmds, it.x, it.y, it.w, it.h, b.sheen_color)
      outline(cmds, it, 2, b.gold_color)
      label_color = b.hover_color
    end
    centered(cmds, it.x + it.w * 0.5, it.y + (it.h - b.label_size) * 0.5, it.label, b.label_size, label_color)
  end

  return cmds
end

return M
