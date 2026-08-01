-- ui.select — a dropdown: a `field` (always drawn: panel + selected/placeholder label +
-- a downward caret) and, while THIS select is open, a `menu` popup of option rows (the
-- selected row highlighted `sel_bg`, the hovered row `hover_bg`), lifted one sub-layer
-- above the field. This module owns the select's WHOLE definition — draw AND hit: the
-- field claims; while open the popup (which lies BELOW the node rect) claims too; a
-- click toggles open / picks the option row under the pointer / closes on any click.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: open (is this select open — mirrors the walker's retained popup owner),
--   bind_value (selected value text), children ({ value, label } options),
--   placeholder (field text when nothing is selected), mx, my (pointer, for option
--   hover), layer, style { field {...}, menu {...} }.

local core = require("ui.core")

local M = {} -- not named `select` (that shadows Lua's stdlib `select`)

-- The popup's outer rect: flush under the field (6px gap), `row_h` per option — THE
-- geometry, shared by draw and hit so they can never disagree.
local function menu_rect(r, menu, n)
  local row_h = core.jnum(menu, "row_h", 30)
  return { x = r.x, y = r.y + r.h + 6, w = r.w, h = row_h * n }, row_h
end

-- An option child's bound value: its string `value`, else its string `label`, else "".
local function option_value(child)
  if type(child.value) == "string" then
    return child.value
  elseif type(child.label) == "string" then
    return child.label
  end
  return ""
end

local INK = { 0.871, 0.847, 0.788, 1.0 }
local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local STONE = { 0.055, 0.063, 0.086, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local SAP = { 0.141, 0.247, 0.471, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- A small downward caret from 4 stacked rects (mirrors draw_caret; no glyph-font dep).
local function draw_caret(cmds, cx, cy, size, color, layer)
  for i = 0, 3 do
    local w = size * (1 - i / 4)
    core.rect(cmds, { x = cx - w * 0.5, y = cy - 1 + i, w = w, h = 1 }, color, layer)
  end
end

-- The field's display text + whether it is the (dim) placeholder (mirrors select_display).
local function display(props, kids)
  local cur = props.bind_value
  if type(cur) == "string" and cur ~= "" then
    for i = 1, #kids do
      if (kids[i].value or kids[i].label) == cur then
        return kids[i].label or kids[i].value or cur, false
      end
    end
    return cur, false
  end
  return props.placeholder or "", true
end

function M.draw(cmds, r, props)
  local base = props.style or {}
  local field = base.field or {}
  local menu = base.menu or {}
  local layer = props.layer
  local kids = props.children or {}

  -- ── field (always drawn) ──
  local ftop = core.first_color(field, { "top" }, PANEL)
  local fbot = core.first_color(field, { "bot" }, ftop)
  local fborder = core.first_color(field, { "border" }, CLEAR)
  core.panel(cmds, r, {
    fill = ftop,
    fill2 = fbot,
    radius = core.jnum(field, "radius", 3),
    border = (fborder[4] > 0) and 1 or 0,
    border_color = fborder,
    layer = layer,
  })
  local lsz = core.jnum(field, "label_size", 15)
  local label, placeholder = display(props, kids)
  local lc = placeholder and core.first_color(field, { "placeholder", "caret", "label" }, DIM)
    or core.first_color(field, { "label" }, INK)
  core.text(cmds, r.x + 14, r.y + (r.h - lsz) * 0.5, label, lsz, lc, "left", "body", layer)
  draw_caret(cmds, r.x + r.w - 16, r.y + r.h * 0.5, 9, core.first_color(field, { "caret", "label" }, DIM), layer)

  -- ── popup (only while open), lifted one sub-layer above the field ──
  if props.open ~= true then
    return
  end
  local plyr = layer + 1
  local n = #kids
  local mrect, row_h = menu_rect(r, menu, n)
  local mtop = core.first_color(menu, { "top" }, STONE)
  local mbot = core.first_color(menu, { "bot" }, mtop)
  local mborder = core.first_color(menu, { "border" }, CLEAR)
  core.panel(cmds, mrect, {
    fill = mtop,
    fill2 = mbot,
    radius = core.jnum(menu, "radius", 3),
    border = (mborder[4] > 0) and 1 or 0,
    border_color = mborder,
    layer = plyr,
  })
  local msz = core.jnum(menu, "label_size", 15)
  local cur = props.bind_value
  for i = 1, n do
    local child = kids[i]
    local ry = mrect.y + (i - 1) * row_h
    local selected = (child.value or child.label) == cur
    if selected then
      core.rect(cmds, { x = r.x + 4, y = ry, w = r.w - 8, h = row_h },
        core.first_color(menu, { "sel_bg" }, SAP), plyr)
    elseif core.point_in(props.mx, props.my, { x = r.x, y = ry, w = r.w, h = row_h }) then
      core.rect(cmds, { x = r.x + 4, y = ry, w = r.w - 8, h = row_h },
        core.first_color(menu, { "hover_bg" }, STONE), plyr)
    end
    local rc = selected and core.first_color(menu, { "sel_label", "label" }, INK)
      or core.first_color(menu, { "label" }, INK)
    core.text(cmds, r.x + 14, ry + (row_h - msz) * 0.5, child.label or child.value or "", msz, rc,
      "left", "body", plyr)
  end
end

-- The field claims; while open the popup below it claims too (the walker keeps an
-- open popup's owner in the candidate set even though the rows lie OUTSIDE the node
-- rect). A click: on the closed field → open; while open → pick the option row
-- under the pointer (if any) and close — ANY click closes, matching the settings
-- dropdown. The selected-value echo on idle frames is the walker's generic pass.
function M.hit(mx, my, r, props, click)
  local base = props.style or {}
  local menu = base.menu or {}
  local kids = props.children or {}
  local open = props.open == true
  local mrect, row_h = menu_rect(r, menu, #kids)
  local v = { hit = core.point_in(mx, my, r) or (open and core.point_in(mx, my, mrect)) }
  if click then
    if open then
      v.open = false
      if core.point_in(mx, my, mrect) then
        for i = 1, #kids do
          if core.point_in(mx, my, { x = r.x, y = mrect.y + (i - 1) * row_h, w = r.w, h = row_h }) then
            v.value = option_value(kids[i])
          end
        end
      end
    elseif core.point_in(mx, my, r) then
      v.open = true
    end
  end
  return v
end

return M
