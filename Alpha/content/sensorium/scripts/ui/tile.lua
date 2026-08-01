-- ui.tile — a slot cell: LIT (style) when its enabled binding is loaded, EMPTY
-- (style_off) otherwise. Its fill is `hot` when loaded AND its bound value is true, else
-- `cell`; a centred label sits on top. The Lua twin of the engine's draw_tile.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) + M.hit(mx, my, rect).
-- props: enabled (loaded), bind_value (bool), label, layer,
--   style (loaded block), style_off (empty block) — each { hot, cell, label, label_size }.

local core = require("ui.core")

local tile = {}

-- Full-rect control: its whole box is the interactive region, and its only
-- interaction is hover-claim + click-toggles-`bind`. The walker answers that
-- generically in Rust with zero Lua crossings; `hit` below is never called.
tile.hit_shape = "rect"

local SAP = { 0.141, 0.247, 0.471, 1.0 }
local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local INK = { 0.871, 0.847, 0.788, 1.0 }

function tile.draw(cmds, r, props)
  local loaded = props.enabled == true
  local s = loaded and (props.style or {}) or (props.style_off or {})
  local on = loaded and (props.bind_value == true)
  local fill = on and core.first_color(s, { "hot" }, SAP) or core.first_color(s, { "cell" }, PANEL)
  core.panel(cmds, r, { fill = fill, layer = props.layer })

  -- The label always draws (an empty label is an empty text command, like draw_tile).
  local lsz = core.jnum(s, "label_size", 12)
  core.text(cmds, r.x + r.w * 0.5, r.y + (r.h - lsz) * 0.5, props.label or "", lsz,
    core.first_color(s, { "label" }, INK), "center", "label", props.layer)
end

function tile.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return tile
