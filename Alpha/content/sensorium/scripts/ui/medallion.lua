-- ui.medallion — the DS PortraitMedallion: a circular gem well in a metal ring.
-- The ring is a named variant (bronze / sapphire / danger / ghost / gold — a
-- metal gradient pair under `style.rings`); sapphire is the active-player ring
-- and adds an outer rune-light halo + glow. The well shows a rune glyph; a
-- texture portrait waits on a circular sprite mask (primitive gap, noted in the
-- Engine catalog).
--
-- props:
--   ring          -- ring variant name (default "bronze"; or via `ring_bind`)
--   rune          -- fallback glyph (or via `rune_bind`)
--   rune_color    -- resolved rgba override for the glyph (dotted path prop)
--   style         -- the resolved `medallion` block: rim / bg_top / bg_bot /
--                    rune_color / ring_w / rings.<name>{top,bot,glow,halo}

local core = require("ui.core")

local medallion = {}

-- Presentational: never claims the pointer.
medallion.hit_shape = "none"

local BRONZE = { 0.722, 0.592, 0.353, 1.0 }
local BRONZE_DIM = { 0.431, 0.353, 0.204, 1.0 }
local RUNE = { 0.435, 0.592, 1.0, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- A nested style value as a colour, else `dflt` (first_color only walks flat keys).
local function col(c, dflt)
  if type(c) == "table" and c[4] ~= nil then return c end
  return dflt
end

function medallion.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local d = math.min(r.w, r.h)
  local cx, cy = r.x + (r.w - d) * 0.5, r.y + (r.h - d) * 0.5
  local ring = (s.rings or {})[props.ring or "bronze"] or {}

  -- Active halo: a feathered glow disc + a thin rune-light outline, outside the ring.
  local glow = col(ring.glow, CLEAR)
  if glow[4] > 0 then
    core.panel(cmds, { x = cx - 5, y = cy - 5, w = d + 10, h = d + 10 }, {
      fill = glow, radius = (d + 10) * 0.5, feather = 8, layer = layer,
    })
  end
  local halo = col(ring.halo, CLEAR)
  if halo[4] > 0 then
    core.panel(cmds, { x = cx - 5, y = cy - 5, w = d + 10, h = d + 10 }, {
      fill = CLEAR, radius = (d + 10) * 0.5, border = 1, border_color = halo, layer = layer,
    })
  end

  -- The metal ring: a full-round slab in the variant's gradient pair.
  local ring_top = col(ring.top, BRONZE)
  core.panel(cmds, { x = cx, y = cy, w = d, h = d }, {
    fill = ring_top,
    fill2 = col(ring.bot, BRONZE_DIM),
    radius = d * 0.5,
    layer = layer,
  })

  -- The gem well inside: dark radial look as a vertical pair, rimmed near-black.
  local ring_w = core.jnum(s, "ring_w", 3)
  local iw = d - 2 * ring_w
  if iw > 2 then
    local well_top = core.first_color(s, { "bg_top" }, CLEAR)
    core.panel(cmds, { x = cx + ring_w, y = cy + ring_w, w = iw, h = iw }, {
      fill = well_top,
      fill2 = core.first_color(s, { "bg_bot" }, well_top),
      radius = iw * 0.5,
      border = 2,
      border_color = core.first_color(s, { "rim" }, CLEAR),
      layer = layer,
    })
  end

  -- The rune glyph, centred in the well.
  if props.rune and props.rune ~= "" then
    local gsz = math.floor(d * 0.5 + 0.5)
    core.text(cmds, cx + d * 0.5, cy + (d - gsz) * 0.5, props.rune, gsz,
      props.rune_color or core.first_color(s, { "rune_color" }, RUNE), "center", "rune", layer)
  end
end

-- Never dispatched — `hit_shape` above answers the hit in Rust. The body stays
-- because the library registers a COMPONENT by its draw + hit pair.
function medallion.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return medallion
