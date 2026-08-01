-- ui.rune_corners — a decoration: four Elder-Futhark glyphs inset from the rect's four
-- corners, the TOP pair in rune-light (`top`), the BOTTOM pair in dim bronze (`bot`). A
-- pure overlay (no bind). The Lua twin of the engine's draw_rune_corners.
--
-- props: tl, tr, bl, br (corner glyphs, with defaults), glyph_size, layer,
--   style { inset, size, top, bot }.

local core = require("ui.core")

local rune_corners = {}

-- A pure overlay decoration: never claims the pointer. The walker answers this in
-- Rust with zero Lua crossings; `hit` below is never called.
rune_corners.hit_shape = "none"

local RUNE = { 0.435, 0.592, 1.0, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }

function rune_corners.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local inset = core.jnum(s, "inset", 8)
  local size = core.jnum(props, "glyph_size", core.jnum(s, "size", 14))
  local glow = core.first_color(s, { "top" }, RUNE)
  local bronze = core.first_color(s, { "bot" }, DIM)
  local tl = props.tl or "ᛞ"
  local tr = props.tr or "ᛝ"
  local bl = props.bl or "ᚨ"
  local br = props.br or "ᛟ"

  -- Top pair (rune-light glow), inset from the two top corners.
  core.text(cmds, r.x + inset, r.y + inset, tl, size, glow, "left", "rune", layer)
  core.text(cmds, r.x + r.w - inset, r.y + inset, tr, size, glow, "right", "rune", layer)
  -- Bottom pair (dim bronze), inset up so the glyph box mirrors the top pair.
  local by = r.y + r.h - inset - size
  core.text(cmds, r.x + inset, by, bl, size, bronze, "left", "rune", layer)
  core.text(cmds, r.x + r.w - inset, by, br, size, bronze, "right", "rune", layer)
end

function rune_corners.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return rune_corners
