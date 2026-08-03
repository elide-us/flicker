-- ui.action_slot — the DS ActionSlot: a sunk hotbar recess with an inner bronze
-- rim, a rune glyph (Rune face, with a rune-light halo), a keybind tag top-left,
-- a charge count bottom-right, and a cooldown veil that recedes as `cd` falls
-- 1→0. `active` lights the sapphire ring + glow. Click fires the node's `action`
-- (`hit_shape = "rect"` — the walker answers the hit in Rust).
--
-- The DS cooldown is a conic wipe; the engine has no arc primitive yet, so the
-- veil is a top-down vertical wipe (height × cd) — same information, one rect.
--
-- props:
--   rune          -- the ability glyph (or via `rune_bind`)
--   key           -- keybind tag text ("Q", "R2")
--   charges       -- charge count text (via `charges_bind`)
--   cd            -- cooldown fraction 1→0 (via `cd_bind`)
--   active        -- sapphire ring state (via `active_bind`)
--   hot           -- hover (from the walker)
--   style         -- the resolved `action_slot` block

local core = require("ui.core")

local action_slot = {}

-- Full-rect control: hover claims, click fires `action`, zero Lua crossings.
action_slot.hit_shape = "rect"

local STONE = { 0.125, 0.141, 0.180, 1.0 }
local WELL = { 0.039, 0.047, 0.063, 1.0 }
local BRONZE = { 0.722, 0.592, 0.353, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

function action_slot.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local active = props.active == true
  local radius = core.jnum(s, "radius", 4)

  -- Active slots glow before the slab, like a hovered primary button.
  if active then
    local glow = core.first_color(s, { "active_glow" }, CLEAR)
    if glow[4] > 0 then
      core.panel(cmds, { x = r.x - 3, y = r.y - 3, w = r.w + 6, h = r.h + 6 }, {
        fill = glow, radius = radius + 3, feather = 4, layer = layer,
      })
    end
  end

  -- The recess: dark vertical fill + outer border (sapphire when active), then
  -- the faint inner bronze rim one pixel in.
  local border = active and core.first_color(s, { "active_border", "border" }, CLEAR)
    or core.first_color(s, { "border" }, CLEAR)
  core.panel(cmds, r, {
    fill = core.first_color(s, { "bg_top" }, STONE),
    fill2 = core.first_color(s, { "bg_bot" }, WELL),
    radius = radius,
    border = (border[4] > 0) and 1 or 0,
    border_color = border,
    layer = layer,
  })
  local rim = core.first_color(s, { "rim" }, CLEAR)
  if rim[4] > 0 then
    core.panel(cmds, { x = r.x + 1, y = r.y + 1, w = r.w - 2, h = r.h - 2 }, {
      fill = CLEAR, radius = math.max(0, radius - 1), border = 1, border_color = rim, layer = layer,
    })
  end

  -- The rune glyph, centred, with its halo behind.
  if props.rune and props.rune ~= "" then
    local gsz = math.floor(r.h * 0.42 + 0.5)
    local halo = core.first_color(s, { "rune_halo" }, CLEAR)
    if halo[4] > 0 then
      local hw = gsz * 1.2
      core.panel(cmds, { x = r.x + (r.w - hw) * 0.5, y = r.y + (r.h - hw) * 0.5, w = hw, h = hw }, {
        fill = halo, radius = hw * 0.5, feather = 6, layer = layer,
      })
    end
    core.text(cmds, r.x + r.w * 0.5, r.y + (r.h - gsz) * 0.5, props.rune, gsz,
      core.first_color(s, { "rune_color" }, BRONZE), "center", "rune", layer)
  end

  -- Keybind tag (top-left) and charge count (bottom-right).
  if props.key and props.key ~= "" then
    local ksz = core.jnum(s, "key_size", 10)
    core.text(cmds, r.x + 4, r.y + 2, props.key, ksz,
      core.first_color(s, { "key_color" }, BRONZE), "left", "label", layer)
  end
  if props.charges ~= nil and props.charges ~= "" then
    local csz = core.jnum(s, "charge_size", 11)
    local txt = type(props.charges) == "number" and string.format("%d", props.charges)
      or tostring(props.charges)
    core.text(cmds, r.x + r.w - 5, r.y + r.h - csz - 2, txt, csz,
      core.first_color(s, { "charge_color" }, BRONZE), "right", "label", layer)
  end

  -- The cooldown veil: a top-down wipe covering `cd` of the slot.
  local cd = math.max(0.0, math.min(1.0, props.cd or 0.0))
  if cd > 0.0 then
    local veil = core.first_color(s, { "cd_veil" }, CLEAR)
    core.panel(cmds, { x = r.x + 1, y = r.y + 1, w = r.w - 2, h = (r.h - 2) * cd }, {
      fill = veil, radius = math.max(0, radius - 1), layer = layer,
    })
  end
end

-- Never dispatched — `hit_shape` above answers the hit in Rust. The body stays
-- because the library registers a COMPONENT by its draw + hit pair.
function action_slot.hit(mx, my, r)
  return core.point_in(mx, my, r)
end

return action_slot
