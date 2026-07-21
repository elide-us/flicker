-- ui.layout — the composable layout engine.
--
-- Screens are DESCRIBED as a tree of composable containers + component leaves,
-- and `resolve` turns that tree into absolute rects — so a layout is authored by
-- composition (rows / columns / stacks with sizing + spacing) instead of the
-- hand-computed pixel math the old screens carried. The tree is plain data, so it
-- can live in ui_elements.json and be edited without touching Lua.
--
-- A node is:
--   container = { type = "row" | "column" | "stack",
--                 gap? = <px between children>,
--                 pad? = <px inset on all sides>,
--                 size? / grow?,           -- how THIS node claims space in its parent
--                 children = { <node>, ... } }
--   leaf      = { type = "leaf", id = "<component slot id>", size? / grow? }
--
-- Sizing along the parent's MAIN axis (row = x, column = y): `size = <px>` is
-- fixed; `grow = <weight>` shares the leftover space by weight. The CROSS axis
-- fills the container (minus pad). `stack` overlays all children on the same rect
-- (for backgrounds/scrims under content).
--
-- `resolve(node, rect)` returns a flat, draw-ordered list of `{ id, rect }` for
-- every leaf. The caller draws each leaf's component into its rect and hit-tests
-- the same rect — one source of truth for geometry.

local layout = {}

local function inset(r, p)
  p = p or 0
  return { x = r.x + p, y = r.y + p, w = r.w - 2 * p, h = r.h - 2 * p }
end

local function resolve_into(node, rect, out)
  local t = node.type
  if t == "leaf" then
    out[#out + 1] = { id = node.id, rect = rect }
    return
  end

  local area = inset(rect, node.pad)
  local children = node.children or {}

  if t == "stack" then
    for _, child in ipairs(children) do
      resolve_into(child, area, out)
    end
    return
  end

  local horizontal = (t == "row")
  local gap = node.gap or 0
  local n = #children
  local main = horizontal and area.w or area.h

  -- First pass: total fixed length + total grow weight.
  local fixed, grow_total = 0, 0
  for _, c in ipairs(children) do
    if c.grow then
      grow_total = grow_total + c.grow
    else
      fixed = fixed + (c.size or 0)
    end
  end
  local free = main - fixed - gap * math.max(0, n - 1)

  -- Second pass: place each child along the main axis, filling the cross axis.
  local pos = horizontal and area.x or area.y
  for _, c in ipairs(children) do
    local len
    if c.grow then
      len = (grow_total > 0) and (free * c.grow / grow_total) or 0
    else
      len = c.size or 0
    end
    local r
    if horizontal then
      r = { x = pos, y = area.y, w = len, h = area.h }
    else
      r = { x = area.x, y = pos, w = area.w, h = len }
    end
    resolve_into(c, r, out)
    pos = pos + len + gap
  end
end

--- Resolve a layout tree within `rect` → a flat list of `{ id, rect }` leaves.
function layout.resolve(node, rect)
  local out = {}
  resolve_into(node, rect, out)
  return out
end

return layout
