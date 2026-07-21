-- ui.button — a Prism vector button: an SDF panel slab + a centred Cinzel label,
-- with primary / secondary / danger variants and a hover state.
--
-- The abstract COMPONENT interface (shared by every file in `content/scripts/ui/`):
--   * `M.draw(cmds, rect, props)` — emit HudCommands to fill `rect`.
--   * `M.hit(mx, my, rect)`       — return true if the pointer is over it.
-- A component owns no layout: it draws into the `rect` the layout engine gives it.

local core = require("ui.core")

local button = {}

--- props = {
---   label,                       -- text
---   hot?,                        -- hover state (from hit() last frame)
---   layer?,
---   style = {                    -- a variant block from ui_elements.json
---     fill, fill2?, grad?, radius, border?, border_color?, glow?,
---     label_color, label_size,
---   },
--- }
function button.draw(cmds, r, props)
  local s = props.style
  -- Optional sapphire glow halo behind a primary button, when hovered.
  if s.glow and props.hot then
    core.panel(cmds, { x = r.x - 3, y = r.y - 3, w = r.w + 6, h = r.h + 6 }, {
      fill = s.glow,
      radius = (s.radius or 0) + 3,
      feather = 4,
      layer = props.layer,
    })
  end
  core.panel(cmds, r, {
    fill = s.fill,
    fill2 = s.fill2,
    grad = s.grad,
    radius = s.radius,
    border = s.border,
    border_color = s.border_color,
    layer = props.layer,
  })
  core.text(
    cmds,
    r.x + r.w * 0.5,
    r.y + (r.h - s.label_size) * 0.5,
    props.label,
    s.label_size,
    s.label_color,
    "center",
    "label",
    props.layer
  )
end

function button.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return button
