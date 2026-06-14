-- Reusable flat modal: a centred panel with a title and a vertical stack of
-- buttons, driven by ui_elements.json `modal` (style) + `screens[Model.screen]`
-- (overlay/title/items). Shared by the menu / pause / settings screens. Returns
-- `{ <item id> = clicked }`; the host turns those into scene transitions.

local M = {}

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

local function screen()
  return (UI.screens or {})[Model.screen or "menu"]
end

local function layout(sw, sh)
  local m = UI.modal
  local px = math.floor((sw - m.panel.w) * 0.5)
  local py = math.floor((sh - m.panel.h) * 0.5)
  local b = m.buttons
  local sc = screen()
  local rects = {}
  if sc and sc.items then
    for i, item in ipairs(sc.items) do
      rects[i] = {
        id = item.id,
        label = item.label,
        x = px + (m.panel.w - b.w) * 0.5,
        y = py + b.first_y + (i - 1) * b.gap_y,
        w = b.w,
        h = b.h,
      }
    end
  end
  return m, px, py, sc, rects
end

function M.update(mx, my, clicked, sw, sh, down)
  if not UI then
    return {}
  end
  local _, _, _, _, rects = layout(sw, sh)
  local s = {}
  for _, r in ipairs(rects) do
    s[r.id] = clicked and point_in(mx, my, r)
  end
  return s
end

local function rect(cmds, x, y, w, h, c)
  cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = c[1], g = c[2], b = c[3], a = c[4] }
end

function M.draw(sw, sh)
  if not UI then
    return {}
  end
  local m, px, py, sc, rects = layout(sw, sh)
  local cmds = {}

  if sc and sc.overlay then
    rect(cmds, 0, 0, sw, sh, sc.overlay)
  end
  rect(cmds, px, py, m.panel.w, m.panel.h, m.panel.bg)
  local bd = m.panel.border
  if bd then
    rect(cmds, px, py, m.panel.w, 2, bd)
    rect(cmds, px, py + m.panel.h - 2, m.panel.w, 2, bd)
    rect(cmds, px, py, 2, m.panel.h, bd)
    rect(cmds, px + m.panel.w - 2, py, 2, m.panel.h, bd)
  end

  local t = m.title
  cmds[#cmds + 1] = {
    kind = "text", x = px + m.panel.w * 0.5, y = py + t.offset_y,
    text = (sc and sc.title) or "", size = t.size, align = "center",
    r = t.color[1], g = t.color[2], b = t.color[3], a = t.color[4],
  }

  if Model.subtitle and Model.subtitle ~= "" then
    local st = m.subtitle
    cmds[#cmds + 1] = {
      kind = "text", x = px + m.panel.w * 0.5, y = py + st.offset_y,
      text = Model.subtitle, size = st.size, align = "center",
      r = st.color[1], g = st.color[2], b = st.color[3], a = st.color[4],
    }
  end

  local b = m.buttons
  local hx, hy = Model.mx or -1, Model.my or -1
  for _, r in ipairs(rects) do
    local fill = point_in(hx, hy, r) and b.hot or b.cell
    rect(cmds, r.x, r.y, r.w, r.h, fill)
    cmds[#cmds + 1] = {
      kind = "text", x = r.x + r.w * 0.5, y = r.y + (r.h - b.label_size) * 0.5,
      text = r.label, size = b.label_size, align = "center",
      r = b.label[1], g = b.label[2], b = b.label[3], a = b.label[4],
    }
  end

  return cmds
end

return M
