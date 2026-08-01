-- ui.tooltip — a small info card: an optional styled backdrop, then an optional element
-- rune badge (top-left, Rune face) and a name headline (Display) with a dim meta line
-- (Body) below it, in the text column right of the rune. The Lua twin of draw_tooltip.
--
-- props (rune/name/meta resolved by the walker from `<field>_bind` or literal props;
-- absent when empty): rune, name, meta, rune_color (resolved rgba), layer,
-- style { pad, name_size, meta_size, gap, name_color, meta_color, + bg/border/radius }.

local core = require("ui.core")

local tooltip = {}

-- Presentational: a cursor-following tip must never steal the pointer (it would eat
-- every click). The walker answers this in Rust with zero Lua crossings; `hit`
-- below is never called.
tooltip.hit_shape = "none"

local INK = { 0.871, 0.847, 0.788, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local RUNE = { 0.435, 0.592, 1.0, 1.0 }

function tooltip.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer

  -- Backdrop through the one styled-box path, when the node carries a style.
  if props.style ~= nil then
    core.panel_bg(cmds, r, s, layer)
  end

  local pad = core.jnum(s, "pad", 12)
  local inner = { x = r.x + pad, y = r.y + pad, w = r.w - 2 * pad, h = r.h - 2 * pad }
  local name_sz = core.jnum(s, "name_size", 16)
  local meta_sz = core.jnum(s, "meta_size", 12)
  local line_gap = core.jnum(s, "gap", 4)

  -- Optional element rune badge, top-left (the text column then indents past it).
  local indent = 0
  if props.rune and props.rune ~= "" then
    core.text(cmds, inner.x, inner.y, props.rune, name_sz, props.rune_color or RUNE, "left", "rune", layer)
    indent = name_sz * 1.3 + 4
  end

  -- Name headline, then the dim meta line one row below it.
  if props.name and props.name ~= "" then
    core.text(cmds, inner.x + indent, inner.y, props.name, name_sz,
      core.first_color(s, { "name_color" }, INK), "left", "display", layer)
  end
  if props.meta and props.meta ~= "" then
    core.text(cmds, inner.x + indent, inner.y + name_sz + line_gap, props.meta, meta_sz,
      core.first_color(s, { "meta_color" }, DIM), "left", "body", layer)
  end
end

function tooltip.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return tooltip
