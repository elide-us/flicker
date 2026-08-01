-- ui.toggle — a Prism switch: a rounded pill flipping its bound bool. ON draws an
-- on_top→on_bot gradient pill with the knob at the right; OFF draws an off_bg pill with
-- the knob at the left. This module owns the toggle's WHOLE definition — draw AND hit:
-- the tight region is the pill (the rest of the row is inert), and a click inside it
-- flips the bound bool.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (bool), layer, style {
--   w = 50, h = 25, on_top, on_bot, on_border, knob_on,
--   off_bg, off_border, knob_off }.

local core = require("ui.core")

local toggle = {}

local SAP = { 0.141, 0.247, 0.471, 1.0 }
local RUNE = { 0.435, 0.592, 1.0, 1.0 }
local STONE = { 0.055, 0.063, 0.086, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- The pill: a style-sized `w`×`h` switch at the rect's left edge, vertically
-- centred — THE geometry, shared by draw and hit so they can never disagree.
local function pill_rect(r, s)
  local w = core.jnum(s, "w", 50)
  local h = core.jnum(s, "h", 25)
  return { x = r.x, y = r.y + (r.h - h) * 0.5, w = w, h = h }
end

function toggle.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local pill = pill_rect(r, s)
  local radius = pill.h * 0.5
  local knob = math.max(pill.h - 6, 0)
  local kr = knob * 0.5

  if props.bind_value == true then
    local border = core.first_color(s, { "on_border" }, CLEAR)
    core.panel(cmds, pill, {
      fill = core.first_color(s, { "on_top" }, SAP),
      fill2 = core.first_color(s, { "on_bot" }, SAP),
      radius = radius,
      border = (border[4] > 0) and 1 or 0,
      border_color = border,
      layer = layer,
    })
    -- Knob sits at the RIGHT end when on.
    core.panel(cmds, { x = pill.x + pill.w - pill.h + 3, y = pill.y + 3, w = knob, h = knob }, {
      fill = core.first_color(s, { "knob_on" }, RUNE),
      radius = kr,
      layer = layer,
    })
  else
    local border = core.first_color(s, { "off_border" }, CLEAR)
    core.panel(cmds, pill, {
      fill = core.first_color(s, { "off_bg" }, STONE),
      radius = radius,
      border = (border[4] > 0) and 1 or 0,
      border_color = border,
      layer = layer,
    })
    -- Knob sits at the LEFT end when off.
    core.panel(cmds, { x = pill.x + 3, y = pill.y + 3, w = knob, h = knob }, {
      fill = core.first_color(s, { "knob_off" }, DIM),
      radius = kr,
      layer = layer,
    })
  end
end

-- Tight region = the pill only (the rest of the row is inert); a click inside
-- flips the bound bool. Idle-frame echo is the walker's generic pass.
function toggle.hit(mx, my, r, props, click)
  local over = core.point_in(mx, my, pill_rect(r, props.style or {}))
  local v = { hit = over }
  if over and click then
    v.value = not (props.bind_value == true)
  end
  return v
end

return toggle
