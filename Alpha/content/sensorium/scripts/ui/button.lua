-- ui.button — a Prism vector button: an SDF panel slab + a centred label, with a
-- hover state, an optional sapphire glow halo, and per-variant fill/border/label
-- (primary / secondary / danger). This COMPONENT owns the button's whole draw
-- definition; the walker (`run_ui`) lays it out and renders what it emits. It is the
-- Lua twin of the engine's `draw_button` — command-for-command identical.
--
-- The abstract COMPONENT interface (shared by every file in `content/scripts/ui/`):
--   * `M.draw(cmds, rect, props)` — emit HudCommands to fill `rect`.
--   * `M.hit(mx, my, rect)`       — return true if the pointer is over it.
-- A component owns no layout: it draws into the `rect` the layout engine gives it.
--
-- `props` (assembled by the walker's `component_props`):
--   label        -- the caption text
--   hot          -- hover/focus state (from the walker)
--   layer        -- painter's-order sub-layer
--   label_size?  -- the node's label_size override (fallback under the style block)
--   style        -- the resolved `modal.buttons.variants.<v>` block: any of
--                   fill_top / fill_bot / border / label / glow +
--                   hover_top / hover_bot / hover_border / hover_label, plus optional
--                   radius / border_w / label_size. Each colour is a Prism rgba;
--                   missing keys fall down the alias chain, then to the Prism default.

local core = require("ui.core")

local button = {}

-- Full-rect control: its whole box is the interactive region, and its only
-- interaction is hover-claim + click-fires-`action`. The walker answers that
-- generically in Rust with zero Lua crossings; `hit` below is never called.
button.hit_shape = "rect"

-- Prism draw-side fallbacks (mirror the walker's palette consts) — only reached when a
-- style block omits the key; live variant blocks provide their own colours.
local SAP = { 0.141, 0.247, 0.471, 1.0 }
local INK = { 0.871, 0.847, 0.788, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

function button.draw(cmds, r, props)
  local s = props.style or {}
  local hot = props.hot
  local layer = props.layer

  -- Optional sapphire glow halo behind the button, only on hover.
  local glow = core.first_color(s, { "glow" }, CLEAR)
  if glow[4] > 0 and hot then
    core.panel(cmds, { x = r.x - 3, y = r.y - 3, w = r.w + 6, h = r.h + 6 }, {
      fill = glow,
      radius = core.jnum(s, "radius", 3) + 3,
      feather = 4,
      layer = layer,
    })
  end

  -- Fill / border / label pick their hover vs idle stops down the alias chain — the
  -- button OWNS this state→style logic (the walker's `draw_button` used to).
  local top, bot, border, label_color
  if hot then
    top = core.first_color(s, { "hover_top", "hot", "fill_top", "cell", "fill" }, SAP)
    bot = core.first_color(s, { "hover_bot", "hot", "fill_bot", "cell", "fill" }, top)
    border = core.first_color(s, { "hover_border", "border" }, CLEAR)
    label_color = core.first_color(s, { "hover_label", "label" }, INK)
  else
    top = core.first_color(s, { "fill_top", "cell", "fill" }, SAP)
    bot = core.first_color(s, { "fill_bot", "cell", "fill" }, top)
    border = core.first_color(s, { "border" }, CLEAR)
    label_color = core.first_color(s, { "label" }, INK)
  end

  core.panel(cmds, r, {
    fill = top,
    fill2 = bot,
    radius = core.jnum(s, "radius", 3),
    border = (border[4] > 0) and core.jnum(s, "border_w", 1) or 0,
    border_color = border,
    layer = layer,
  })

  local lsz = core.jnum(s, "label_size", props.label_size or 14)
  core.text(
    cmds,
    r.x + r.w * 0.5,
    r.y + (r.h - lsz) * 0.5,
    props.label,
    lsz,
    label_color,
    "center",
    "label",
    layer
  )
end

function button.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return button
