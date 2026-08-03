-- ui.checkbox — a Prism checkbox: a `box`-sized square (style `box` fill + optional
-- 1px border) at the rect's top-left, an inset `check` square when the bound value is
-- true, and an optional row label to its right. This module owns the checkbox's WHOLE
-- definition — draw AND hit: the tight region is the box (the label row is inert),
-- and a click inside it toggles the bound bool.
--
-- The abstract COMPONENT interface (shared by every file in `content/sensorium/scripts/ui/`):
--   * `M.draw(cmds, rect, props)` — emit HudCommands to fill `rect`.
--   * `M.hit(mx, my, rect, props, click, down)` — return a hit VERDICT: a bare
--     boolean (just hover), or a table { hit, value, activate, capture, open,
--     focus, group_focus, activate_child } the walker applies generically.
--     `click` is this frame's press edge (pre-gated on enabled); `down` the held state.
--
-- `props` (assembled by the walker's `component_props`):
--   bind_value  -- the bound bool (checked when == true); nil → unchecked
--   box         -- the square's side length (node prop, default 14)
--   label       -- the row caption (drawn to the right of the box)
--   label_x     -- caption x offset from the rect left (default box + 8)
--   label_size  -- caption size (default 13)
--   layer       -- painter's-order sub-layer
--   style       -- { box = fill, check = tick fill, border? , pad? = tick inset (default 3) }

local core = require("ui.core")

local checkbox = {}

-- The box: a `box`-sized square at the rect's top-left — THE geometry, shared by
-- draw and hit so they can never disagree.
local function box_rect(r, props)
  local size = core.jnum(props, "box", 14)
  return { x = r.x, y = r.y, w = size, h = size }
end

-- Prism draw-side fallbacks (mirror the walker's palette consts).
local INK = { 0.871, 0.847, 0.788, 1.0 }
local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

function checkbox.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local bx = box_rect(r, props)

  -- The box: `box` fill + an optional 1px border.
  local border = core.first_color(s, { "border" }, CLEAR)
  core.panel(cmds, bx, {
    fill = core.first_color(s, { "box" }, PANEL),
    border = (border[4] > 0) and 1 or 0,
    border_color = border,
    layer = layer,
  })

  -- The check: an inset filled square when the bound value is true.
  if props.bind_value == true then
    local p = core.jnum(s, "pad", 3)
    core.panel(cmds, { x = bx.x + p, y = bx.y + p, w = bx.w - 2 * p, h = bx.h - 2 * p }, {
      fill = core.first_color(s, { "check" }, INK),
      layer = layer,
    })
  end

  -- The row label sits to the right of the box (INK, body face — like draw_checkbox).
  if props.label and props.label ~= "" then
    local lx = r.x + core.jnum(props, "label_x", bx.w + 8)
    local lsz = core.jnum(props, "label_size", 13)
    core.text(cmds, lx, bx.y + (bx.h - lsz) * 0.5, props.label, lsz, INK, "left", "body", layer)
  end
end

-- Tight region = the box only (a label-row click is inert); a click inside
-- toggles the bound bool. Two-way echo on idle frames is the walker's generic pass.
function checkbox.hit(mx, my, r, props, click)
  local over = core.point_in(mx, my, box_rect(r, props))
  local v = { hit = over }
  if over and click then
    v.value = not (props.bind_value == true)
  end
  return v
end

return checkbox
