-- ui.list — a scrolling column region: an optional styled backdrop plus, when the
-- content overflows the viewport, a right-edge scrollbar (track + proportional
-- thumb with a 28px floor). This module owns the region's WHOLE definition — draw
-- AND hit: the whole rect claims the pointer, and a wheel tick over it folds into
-- the bound offset, clamped to the content. The LAYOUT stays the walker's: the
-- engine flows the children shifted by the bound offset and clips them to the
-- viewport (a structural primitive), and hands this module the resulting
-- `content_h` so the bar and the clamp can never disagree with the placement.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (the offset, number), content_h (the walker-measured content
--   height), pad_x / pad_y (the node's insets — the viewport is the padded rect),
--   scroll_speed (px per wheel notch, default 46), wheel (this frame's tick, a
--   live per-call field like bind_value), layer, style { bar_w, track, thumb,
--   + the shared container backdrop keys (panel_bg / fill / border / radius / …) }.

local core = require("ui.core")

local list = {}

local STONE = { 0.055, 0.063, 0.086, 1.0 }
local SAP = { 0.141, 0.247, 0.471, 1.0 }

-- The viewport: the node rect inset by its pads (extents floored at 0) — THE
-- geometry, shared by draw and hit so the bar and the clamp agree exactly.
local function inner_rect(r, props)
  local px = core.jnum(props, "pad_x", 0)
  local py = core.jnum(props, "pad_y", 0)
  return {
    x = r.x + px,
    y = r.y + py,
    w = math.max(r.w - px * 2, 0),
    h = math.max(r.h - py * 2, 0),
  }
end

function list.draw(cmds, r, props)
  local s = props.style
  local layer = props.layer
  -- The region's own backdrop (unclipped — the children carry the viewport clip,
  -- and the scrollbar must stay visible at the edge). Only when styled: an
  -- unstyled region is transparent structure.
  if s then
    core.panel_bg(cmds, r, s, layer)
  end
  s = s or {}

  local inner = inner_rect(r, props)
  local content_h = props.content_h or 0
  local max = content_h - inner.h
  if max > 0 then
    local offset = math.min(math.max(props.bind_value or 0, 0), max)
    local bw = core.jnum(s, "bar_w", 4)
    local bx = inner.x + inner.w - bw
    core.rect(cmds, { x = bx, y = inner.y, w = bw, h = inner.h },
      core.first_color(s, { "track" }, STONE), layer)
    -- Proportional thumb (viewport÷content of the viewport) with a 28px floor so
    -- a mile of content still leaves something to grab.
    local floor = math.min(28, inner.h)
    local thumb_h = math.min(math.max(inner.h * (inner.h / content_h), floor), inner.h)
    local ty = inner.y + (offset / max) * (inner.h - thumb_h)
    core.rect(cmds, { x = bx, y = ty, w = bw, h = thumb_h },
      core.first_color(s, { "thumb" }, SAP), layer)
  end
end

-- The whole rect claims (a click on the region must not pick through to the
-- scene); a wheel tick over it writes the offset — current value minus
-- wheel·speed, clamped to [0, content − viewport]. Wheel-less frames return the
-- bare claim, so the offset only moves when the wheel does.
function list.hit(mx, my, r, props)
  if not core.point_in(mx, my, r) then
    return false
  end
  local v = { hit = true }
  local wheel = props.wheel or 0
  if wheel ~= 0 then
    local inner = inner_rect(r, props)
    local max = math.max((props.content_h or 0) - inner.h, 0)
    local cur = props.bind_value or 0
    local speed = core.jnum(props, "scroll_speed", 46)
    v.value = math.min(math.max(cur - wheel * speed, 0), max)
  end
  return v
end

return list
