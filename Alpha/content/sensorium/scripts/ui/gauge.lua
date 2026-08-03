-- ui.gauge — a read-only condition gauge: a stone track with a highlighted
-- "habitable band" between `lo` and `hi`, and a marker at the bound value —
-- green (marker_in) when the value sits inside the band, caution-coloured
-- (marker) outside it. A nil / negative bound value means "no signal yet": the
-- bar is washed with `no_signal` and no marker is drawn. The declarative port
-- of the retired `Widgets.gauge_draw` (flicker-pocepochs habitability panel).
--
-- Pure display: the value + band live in the engine Model (the habitability
-- observer publishes them); this module owns only how they DRAW.
--
-- The COMPONENT interface: M.draw(cmds, rect, props); presentational
-- (`hit_shape = "none"`), so `hit` is never dispatched.
-- props: bind_value (number 0..1; < 0 ⇒ no signal), lo, hi (band, 0..1),
--   layer, style { track, band, marker, marker_in?, no_signal?, sheen?,
--   marker_w? }.

local core = require("ui.core")

local gauge = {}

-- Presentational: never claims the pointer, never interacts. The walker answers
-- this in Rust with zero Lua crossings; `hit` below is never called.
gauge.hit_shape = "none"

local TRACK = { 0.039, 0.047, 0.063, 1.0 }
local BAND = { 0.184, 0.616, 0.357, 0.55 }
local MARKER = { 0.941, 0.804, 0.416, 1.0 }

function gauge.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local lo = core.jnum(props, "lo", 0.3)
  local hi = core.jnum(props, "hi", 0.7)

  core.rect(cmds, r, core.first_color(s, { "track" }, TRACK), layer)
  -- The habitable band (the green zone in the middle of the bar).
  core.rect(
    cmds,
    { x = r.x + r.w * lo, y = r.y, w = r.w * (hi - lo), h = r.h },
    core.first_color(s, { "band" }, BAND),
    layer
  )
  local sheen = s.sheen
  if type(sheen) == "table" and #sheen >= 4 then
    core.rect(cmds, { x = r.x, y = r.y, w = r.w, h = 1 }, sheen, layer)
  end

  local value = props.bind_value
  if type(value) ~= "number" or value < 0 then
    -- No signal yet: grey the whole bar out, no marker.
    local wash = s.no_signal
    if type(wash) == "table" and #wash >= 4 then
      core.rect(cmds, r, wash, layer)
    end
    return
  end

  local t = math.max(0, math.min(1, value))
  local inb = value >= lo and value <= hi
  local mc = inb and core.first_color(s, { "marker_in", "marker" }, MARKER)
    or core.first_color(s, { "marker" }, MARKER)
  local mw = core.jnum(s, "marker_w", 4)
  core.rect(cmds, { x = r.x + r.w * t - mw * 0.5, y = r.y - 3, w = mw, h = r.h + 6 }, mc, layer)
end

-- Never dispatched — `hit_shape` above answers the hit in Rust. The body stays
-- because the library registers a COMPONENT by its draw + hit pair (see
-- flicker-script's `probe_component_kinds`).
function gauge.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return gauge
