-- ui.resource_gauge — the DS ResourceGauge: a sunk stone track with a lit
-- gradient fill, per-tone colours (health / mana / stamina / cast), an optional
-- caps label row above (label left, readout right), and a `low` warning state
-- that swaps the border + label to blood. Distinct from `gauge` (the band+marker
-- habitability read-out): this is a FILLED FRACTION bar.
--
-- props:
--   bind_value    -- the fill fraction 0..1 (the node's `bind`)
--   tone          -- "health" | "mana" | "stamina" | "cast" (default health)
--   label         -- optional caps label ($token) — drawn above the track
--   readout       -- optional pre-formatted value text (via `readout_bind`)
--   low           -- warning state (via `low_bind`)
--   style         -- the resolved `resource_gauge` block: track / border /
--                    low_border / radius / pad / sheen + <tone>_top/_bot/_label
--                    (+ cast_border) + label_size / readout_size / readout_color

local core = require("ui.core")

local resource_gauge = {}

-- Read-only display: never claims the pointer.
resource_gauge.hit_shape = "none"

local WELL = { 0.039, 0.047, 0.063, 1.0 }
local INK = { 0.871, 0.847, 0.788, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

function resource_gauge.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local tone = props.tone or "health"
  local low = props.low == true

  -- Optional label row above the track (bar variant); the track takes the rest.
  local label_sz = core.jnum(s, "label_size", 10)
  local track_y, track_h = r.y, r.h
  if props.label and props.label ~= "" then
    local label_color = low and core.first_color(s, { "low_label" }, INK)
      or core.first_color(s, { tone .. "_label", "label" }, INK)
    core.text(cmds, r.x, r.y, props.label, label_sz, label_color, "left", "label", layer)
    if props.readout and props.readout ~= "" then
      core.text(cmds, r.x + r.w, r.y, props.readout, core.jnum(s, "readout_size", 12),
        core.first_color(s, { "readout_color" }, INK), "right", "label", layer)
    end
    local row = label_sz + 6
    track_y, track_h = r.y + row, r.h - row
  end
  if track_h <= 2 then return end

  -- The sunk track: flat well + hairline border; `low` swaps to the blood rim,
  -- `cast` carries its own bronze rim.
  local radius = math.min(core.jnum(s, "radius", 10), track_h * 0.5)
  local border = low and core.first_color(s, { "low_border", "border" }, CLEAR)
    or core.first_color(s, { tone .. "_border", "border" }, CLEAR)
  core.panel(cmds, { x = r.x, y = track_y, w = r.w, h = track_h }, {
    fill = core.first_color(s, { "track" }, WELL),
    radius = radius,
    border = (border[4] > 0) and 1 or 0,
    border_color = border,
    layer = layer,
  })

  -- The lit fill: the tone's two-stop gradient, inset by `pad`, plus a 1px sheen.
  local pad = core.jnum(s, "pad", 2)
  local frac = math.max(0.0, math.min(1.0, props.bind_value or 0.0))
  local fw = (r.w - 2 * pad) * frac
  if fw > 1 then
    local top = core.first_color(s, { tone .. "_top" }, INK)
    local fr = { x = r.x + pad, y = track_y + pad, w = fw, h = track_h - 2 * pad }
    core.panel(cmds, fr, {
      fill = top,
      fill2 = core.first_color(s, { tone .. "_bot" }, top),
      radius = math.max(0, radius - pad),
      layer = layer,
    })
    local sheen = core.first_color(s, { "sheen" }, CLEAR)
    if sheen[4] > 0 then
      core.rect(cmds, { x = fr.x + 1, y = fr.y, w = math.max(0, fr.w - 2), h = 1 }, sheen, layer)
    end
  end
end

-- Never dispatched — `hit_shape` above answers the hit in Rust. The body stays
-- because the library registers a COMPONENT by its draw + hit pair.
function resource_gauge.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return resource_gauge
