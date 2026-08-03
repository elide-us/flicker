-- ui.sprite — an image node: blit the texture named by the `tex` prop into the rect,
-- tinted white × `alpha`. No `tex` → nothing.
--
-- props: tex (engine texture id), alpha (default 1), layer.

local core = require("ui.core")

local sprite = {}

-- Presentational: never claims the pointer, never interacts. The walker answers
-- this in Rust with zero Lua crossings; `hit` below is never called.
sprite.hit_shape = "none"

function sprite.draw(cmds, r, props)
  if props.tex == nil then
    return
  end
  core.sprite(cmds, r, math.floor(props.tex), core.jnum(props, "alpha", 1), props.layer)
end

-- Never dispatched — `hit_shape` above answers the hit in Rust. The body stays
-- because the library registers a COMPONENT by its draw + hit pair (see
-- flicker-script's `probe_component_kinds`).
function sprite.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return sprite
