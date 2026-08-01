-- ui.badge — a small rounded chip with a centred label. A `solid` badge fills bronze;
-- otherwise its `tone` (accent / bronze / neutral) selects a bg/label colour pair.
-- This module owns the badge's WHOLE definition — draw AND hit: the claim region is
-- the PILL (a style may inset it inside the node rect), and a badge only claims —
-- it never binds or fires.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: label (the chip text), solid (bool), tone (accent/bronze/neutral), label_size,
--   layer, style { pad, h, radius, border?, border_w, <tone>_bg / <tone>_label,
--   solid_bg / solid_label, label_size }.

local core = require("ui.core")

local badge = {}

local INK = { 0.871, 0.847, 0.788, 1.0 }
local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local STONE = { 0.055, 0.063, 0.086, 1.0 }
local SAP = { 0.141, 0.247, 0.471, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- The pill: `pad`-inset horizontally, style-`h` tall, vertically centred — THE
-- geometry, shared by draw and hit so they can never disagree.
local function pill_rect(r, s)
  local pad = core.jnum(s, "pad", 0)
  local sh = core.jnum(s, "h", r.h)
  local h = (r.h > 0) and math.min(sh, r.h) or sh
  return {
    x = r.x + pad,
    y = r.y + math.max((r.h - h) * 0.5, 0),
    w = math.max(r.w - pad * 2, 0),
    h = h,
  }
end

function badge.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer

  local pill = pill_rect(r, s)
  local radius = core.jnum(s, "radius", pill.h * 0.5)

  -- `solid` (filled bronze) wins; else the tone selects its bg/label pair.
  local bg, label_col
  if props.solid == true then
    bg = core.first_color(s, { "solid_bg" }, INK)
    label_col = core.first_color(s, { "solid_label" }, STONE)
  else
    local tone = props.tone or "neutral"
    if tone == "accent" then
      bg = core.first_color(s, { "accent_bg" }, SAP)
      label_col = core.first_color(s, { "accent_label" }, INK)
    elseif tone == "bronze" then
      bg = core.first_color(s, { "bronze_bg" }, STONE)
      label_col = core.first_color(s, { "bronze_label" }, INK)
    else
      bg = core.first_color(s, { "neutral_bg" }, PANEL)
      label_col = core.first_color(s, { "neutral_label" }, DIM)
    end
  end

  local border = core.first_color(s, { "border" }, CLEAR)
  core.panel(cmds, pill, {
    fill = bg,
    radius = radius,
    border = (border[4] > 0) and core.jnum(s, "border_w", 1) or 0,
    border_color = border,
    layer = layer,
  })
  local lsz = core.jnum(s, "label_size", core.jnum(props, "label_size", 11))
  core.text(cmds, pill.x + pill.w * 0.5, pill.y + (pill.h - lsz) * 0.5, props.label or "", lsz, label_col,
    "center", "label", layer)
end

-- Claim region = the pill (NOT the whole node rect — a style may inset it). A
-- badge is a status chip over UI surface: it claims the pointer so the scene
-- can't pick through, but never binds or fires.
function badge.hit(mx, my, r, props)
  return core.point_in(mx, my, pill_rect(r, props.style or {}))
end

return badge
