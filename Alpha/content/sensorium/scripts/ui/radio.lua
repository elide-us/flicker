-- ui.radio — one option of an exclusive group: a circular box (radius clamps to a
-- circle) and, when this row's literal `value` equals the group's bound selection, a
-- filled round dot; plus a row label to the right. This module owns the radio's WHOLE
-- definition — draw AND hit: the tight region is the circle (the label row is inert),
-- and a click inside it writes this row's literal `value` into the group's bind.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (the group's selected text), value (this row's literal id), box
--   (circle side, default 14), label, label_x (default box+8), label_size (default 13),
--   layer, style { box, check, border?, pad? = dot inset (default 3), label }.

local core = require("ui.core")

local radio = {}

local INK = { 0.871, 0.847, 0.788, 1.0 }
local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- The circle's bounding box: a `box`-sized square at the rect's top-left — THE
-- geometry, shared by draw and hit so they can never disagree.
local function box_rect(r, props)
  local size = core.jnum(props, "box", 14)
  return { x = r.x, y = r.y, w = size, h = size }
end

function radio.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local bx = box_rect(r, props)

  -- The circular box (radius >= half-side → the shader clamps it to a circle).
  local border = core.first_color(s, { "border" }, CLEAR)
  core.panel(cmds, bx, {
    fill = core.first_color(s, { "box" }, PANEL),
    radius = bx.w * 0.5,
    border = (border[4] > 0) and 1 or 0,
    border_color = border,
    layer = layer,
  })

  -- Selected when this row's literal value equals the group's bound selection.
  if props.value ~= nil and props.value == props.bind_value then
    local p = core.jnum(s, "pad", 3)
    local dw = bx.w - 2 * p
    core.panel(cmds, { x = bx.x + p, y = bx.y + p, w = dw, h = bx.h - 2 * p }, {
      fill = core.first_color(s, { "check" }, INK),
      radius = dw * 0.5,
      layer = layer,
    })
  end

  -- The row label sits to the right of the circle (style `label` colour, INK default).
  if props.label and props.label ~= "" then
    local lx = r.x + core.jnum(props, "label_x", bx.w + 8)
    local lsz = core.jnum(props, "label_size", 13)
    core.text(cmds, lx, bx.y + (bx.h - lsz) * 0.5, props.label, lsz,
      core.first_color(s, { "label" }, INK), "left", "body", layer)
  end
end

-- Tight region = the circle only (the label row is inert). A click inside selects
-- THIS row: the verdict's value (the row's literal string id) sets the group key
-- unconditionally, so it wins over any sibling's echo whatever the placement order
-- (echoes only fill absent keys, and run after the whole hit pass).
function radio.hit(mx, my, r, props, click)
  local over = core.point_in(mx, my, box_rect(r, props))
  local v = { hit = over }
  if over and click and type(props.value) == "string" then
    v.value = props.value
  end
  return v
end

return radio
