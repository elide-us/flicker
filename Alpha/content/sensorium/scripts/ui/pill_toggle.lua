-- ui.pill_toggle — a segmented control: a rounded well (bg + hairline border) split into
-- one segment per child; the active segment (whose child `value` == the bound selection)
-- gets an active_top→active_bot gradient pill inset 1px, and every segment draws its
-- child `label`. This module owns the pill_toggle's WHOLE definition — draw AND hit:
-- the WELL claims the pointer, and a click inside a segment cell selects that child's
-- `value` (the pad rim between well edge and cells selects nothing).
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (the selected text), children ({ value, label } per segment), layer,
--   style { pad = 3, h, radius = 15, border_w = 1, bg, border?, active_top, active_bot,
--   active_label, label, label_size = 11 }.

local core = require("ui.core")

local pill_toggle = {}

local SAP = { 0.141, 0.247, 0.471, 1.0 }
local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local INK = { 0.871, 0.847, 0.788, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- The well (a style-`h`-tall track, vertically centred) and one `pad`-inset cell per
-- child — THE geometry, shared by draw and hit so they can never disagree.
local function well_rect(r, s)
  local sh = core.jnum(s, "h", r.h)
  local h = (r.h > 0) and math.min(sh, r.h) or sh
  return { x = r.x, y = r.y + math.max((r.h - h) * 0.5, 0), w = r.w, h = h }
end

local function seg_rect(well, pad, n, i)
  local cw = (well.w - pad * 2) / n
  local ch = math.max(well.h - pad * 2, 0)
  return { x = well.x + pad + (i - 1) * cw, y = well.y + pad, w = cw, h = ch }
end

function pill_toggle.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local kids = props.children or {}
  local n = #kids

  local pad = core.jnum(s, "pad", 3)
  local well = well_rect(r, s)

  -- The well: `bg` fill + a hairline `border`.
  local radius = core.jnum(s, "radius", 15)
  local border = core.first_color(s, { "border" }, CLEAR)
  core.panel(cmds, well, {
    fill = core.first_color(s, { "bg" }, PANEL),
    radius = radius,
    border = (border[4] > 0) and core.jnum(s, "border_w", 1) or 0,
    border_color = border,
    layer = layer,
  })

  if n == 0 then
    return
  end
  local lsz = core.jnum(s, "label_size", 11)
  for i = 1, n do
    local child = kids[i]
    local seg = seg_rect(well, pad, n, i)
    local active = child.value ~= nil and child.value == props.bind_value
    if active then
      -- The floating highlight: an active_top→active_bot gradient inset 1px within the
      -- cell, radius one under the well's.
      core.panel(cmds, { x = seg.x + 1, y = seg.y, w = math.max(seg.w - 2, 0), h = seg.h }, {
        fill = core.first_color(s, { "active_top" }, SAP),
        fill2 = core.first_color(s, { "active_bot" }, SAP),
        radius = math.max(radius - 1, 0),
        layer = layer,
      })
    end
    local lc = active and core.first_color(s, { "active_label" }, INK)
      or core.first_color(s, { "label" }, DIM)
    core.text(cmds, seg.x + seg.w * 0.5, seg.y + (seg.h - lsz) * 0.5, child.label or "", lsz, lc,
      "center", "label", layer)
  end
end

-- The WELL claims; a click inside a segment cell selects that child's `value`
-- (last matching cell wins, mirroring the walker's old scan order). The pad rim
-- claims without selecting; idle echo is the walker's generic pass.
function pill_toggle.hit(mx, my, r, props, click)
  local s = props.style or {}
  local well = well_rect(r, s)
  local v = { hit = core.point_in(mx, my, well) }
  if click then
    local kids = props.children or {}
    local n = #kids
    if n > 0 then
      local pad = core.jnum(s, "pad", 3)
      for i = 1, n do
        if core.point_in(mx, my, seg_rect(well, pad, n, i)) and type(kids[i].value) == "string" then
          v.value = kids[i].value
        end
      end
    end
  end
  return v
end

return pill_toggle
