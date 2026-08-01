-- ui.tabs — an interactive horizontal tab strip: an optional background bar (the node's
-- own `style`), then one cell per CHILD tab laid across the inner width; the cell whose
-- child `value` == the bound selection styles from `tab_active`, the rest from `tab_idle`,
-- and every cell lights on hover. This module owns the strip's WHOLE definition — draw
-- AND hit: the strip claims the pointer (even between cells), and a click inside a tab
-- cell selects that child's `value`.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (selected tab value), children ({ value, label } each), style (strip
--   bg block, may be nil), tab_active, tab_idle (resolved blocks), gap, pad_x, pad_y (tab
--   layout), mx, my (pointer, for per-tab hover), layer.

local core = require("ui.core")

local tabs = {}

local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local INK = { 0.871, 0.847, 0.788, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- The `i`-th of `n` tab cells: the strip inset by pad_x/pad_y, split evenly along x
-- with `gap` between cells — THE geometry, shared by draw and hit so they can never
-- disagree.
local function tab_cell(r, props, n, i)
  local px = core.jnum(props, "pad_x", 0)
  local py = core.jnum(props, "pad_y", 0)
  local gap = core.jnum(props, "gap", 0)
  local inner = { x = r.x + px, y = r.y + py, w = r.w - 2 * px, h = r.h - 2 * py }
  local tw = math.max((inner.w - gap * (n - 1)) / n, 0)
  return { x = inner.x + (i - 1) * (tw + gap), y = inner.y, w = tw, h = inner.h }
end

-- One tab cell: fill/hover alias-chains + a centred label (mirrors draw_tab_cell).
local function draw_cell(cmds, r, s, label, hovered, layer)
  local top, bot, border, lc
  if hovered then
    top = core.first_color(s, { "hover_top", "hot", "fill_top", "active_top", "fill" }, PANEL)
    bot = core.first_color(s, { "hover_bot", "hot", "fill_bot", "active_bot", "fill" }, top)
    border = core.first_color(s, { "hover_border", "border" }, CLEAR)
    lc = core.first_color(s, { "hover_label", "active_label", "label" }, INK)
  else
    top = core.first_color(s, { "fill_top", "active_top", "fill" }, PANEL)
    bot = core.first_color(s, { "fill_bot", "active_bot", "fill" }, top)
    border = core.first_color(s, { "border" }, CLEAR)
    lc = core.first_color(s, { "active_label", "label" }, INK)
  end
  core.panel(cmds, r, {
    fill = top,
    fill2 = bot,
    radius = core.jnum(s, "radius", 3),
    border = (border[4] > 0) and core.jnum(s, "border_w", 1) or 0,
    border_color = border,
    layer = layer,
  })
  local lsz = core.jnum(s, "label_size", 13)
  core.text(cmds, r.x + r.w * 0.5, r.y + (r.h - lsz) * 0.5, label, lsz, lc, "center", "label", layer)
end

function tabs.draw(cmds, r, props)
  local layer = props.layer
  -- The strip background bar, only when the node carries a style.
  if props.style ~= nil then
    core.panel_bg(cmds, r, props.style, layer)
  end

  local kids = props.children or {}
  local n = #kids
  if n == 0 then
    return
  end
  local active_st = props.tab_active or {}
  local idle_st = props.tab_idle or {}
  for i = 1, n do
    local child = kids[i]
    local tr = tab_cell(r, props, n, i)
    local active = child.value ~= nil and child.value ~= "" and child.value == props.bind_value
    local s = active and active_st or idle_st
    local hovered = core.point_in(props.mx, props.my, tr)
    draw_cell(cmds, tr, s, child.label or child.text or "", hovered, layer)
  end
end

-- The strip claims (even between cells, so a click there doesn't pick through);
-- a click inside a tab cell selects that child's `value`. The default-to-first
-- echo on idle frames is the walker's generic pass.
function tabs.hit(mx, my, r, props, click)
  local v = { hit = core.point_in(mx, my, r) }
  local kids = props.children or {}
  local n = #kids
  for i = 1, n do
    if core.point_in(mx, my, tab_cell(r, props, n, i)) then
      v.hit = true
      if click and type(kids[i].value) == "string" then
        v.value = kids[i].value
      end
    end
  end
  return v
end

return tabs
