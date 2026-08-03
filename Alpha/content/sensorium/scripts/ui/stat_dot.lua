-- ui.stat_dot — the DS StatDot: a single Septisigil gem-dot keyed by prism hue
-- (white / yellow / red / orange / black / blue / green — one colour = one stat
-- + two schools). Used for stat lists, school tags, and legends. The glow ring
-- is on by default; `glow = false` flattens it.
--
-- props:
--   hue           -- prism hue name (default "blue"; or via `hue_bind`)
--   glow          -- glow ring on/off (default on)
--   style         -- the resolved `stat_dot` block: border + hues.<hue>{fill,glow}

local core = require("ui.core")

local stat_dot = {}

-- Presentational: never claims the pointer.
stat_dot.hit_shape = "none"

local BLUE = { 0.176, 0.373, 0.69, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- A nested style value as a colour, else `dflt` (first_color only walks flat keys).
local function col(c, dflt)
  if type(c) == "table" and c[4] ~= nil then return c end
  return dflt
end

function stat_dot.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local d = math.min(r.w, r.h)
  local cx, cy = r.x + (r.w - d) * 0.5, r.y + (r.h - d) * 0.5
  local hue = (s.hues or {})[props.hue or "blue"] or {}

  if props.glow ~= false then
    local glow = col(hue.glow, CLEAR)
    if glow[4] > 0 then
      core.panel(cmds, { x = cx - 4, y = cy - 4, w = d + 8, h = d + 8 }, {
        fill = glow, radius = (d + 8) * 0.5, feather = 5, layer = layer,
      })
    end
  end

  core.panel(cmds, { x = cx, y = cy, w = d, h = d }, {
    fill = col(hue.fill, BLUE),
    radius = d * 0.5,
    border = 1,
    border_color = core.first_color(s, { "border" }, CLEAR),
    layer = layer,
  })
end

-- Never dispatched — `hit_shape` above answers the hit in Rust. The body stays
-- because the library registers a COMPONENT by its draw + hit pair.
function stat_dot.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return stat_dot
