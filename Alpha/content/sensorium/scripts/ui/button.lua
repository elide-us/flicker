-- ui.button — a Prism vector button: an SDF panel slab + a centred label, with
-- hover + pressed states (press = 1px nudge + `press_*` stops), an optional
-- sapphire glow halo, and per-variant fill/border/label (primary / secondary /
-- danger). This COMPONENT owns the button's whole draw definition; the walker
-- (`run_ui`) lays it out and renders what it emits.
--
-- The abstract COMPONENT interface (shared by every file in `content/sensorium/scripts/ui/`):
--   * `M.draw(cmds, rect, props)` — emit HudCommands to fill `rect`.
--   * `M.hit(mx, my, rect, props, click, down)` — return a hit VERDICT: a bare
--     boolean (just hover), or a table { hit, value, activate, capture, open,
--     focus, group_focus, activate_child } the walker applies generically.
--     `ui/slider.lua` is the full-verdict exemplar: shared draw/hit geometry,
--     capture, group_focus.
--   * optional `M.hit_shape = "rect" | "none"` — declare a trivial hit geometry
--     and the walker answers the hit in Rust, never dispatching `M.hit`.
-- A component owns no layout: it draws into the `rect` the layout engine gives it.
--
-- `props` (assembled by the walker's `component_props`):
--   label        -- the caption text
--   hot          -- hover/focus state (from the walker)
--   pressed      -- hot + primary held (draws the press state)
--   layer        -- painter's-order sub-layer
--   label_size?  -- the node's label_size override (fallback under the style block)
--   style        -- the resolved `modal.buttons.variants.<v>` block: any of
--                   fill_top / fill_bot / border / label / glow +
--                   hover_top / hover_bot / hover_border / hover_label +
--                   press_top / press_bot / press_border / press_label, plus optional
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
  local pressed = props.pressed
  local layer = props.layer

  -- DS press contract: the whole slab (glow included) nudges down 1px while
  -- held; press_* stops pick the darker fill, falling to the idle fill for
  -- variants that define none.
  if pressed then r = { x = r.x, y = r.y + 1, w = r.w, h = r.h } end

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
  -- button OWNS this state→style logic (it moved out of the walker in S4).
  local top, bot, border, label_color
  if pressed then
    top = core.first_color(s, { "press_top", "fill_top", "cell", "fill" }, SAP)
    bot = core.first_color(s, { "press_bot", "fill_bot", "cell", "fill" }, top)
    border = core.first_color(s, { "press_border", "border" }, CLEAR)
    label_color = core.first_color(s, { "press_label", "label" }, INK)
  elseif hot then
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

-- Never dispatched — `hit_shape` above answers the hit in Rust. The body stays
-- because the library registers a COMPONENT by its draw + hit pair (see
-- flicker-script's `probe_component_kinds`).
function button.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return button
