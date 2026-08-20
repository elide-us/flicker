-- Controller Tester (controllertester) — the scene's LOGIC (the SceneName.lua
-- half of the pair; bench migration 2026-08-19, ruling DC217431).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   active_ctx                                   -- "world" / "menu" / "radial" / "textentry"
--   connected (bool), slots, tick                -- device presence + the demo resolver's clock
--   mouse_x, mouse_y, mouse_l, mouse_r, mouse_m  -- the pointer readout's raws
-- plus RESOLVED WORD variables (localization lives in the stringtable, resolved
-- engine-side; this script only PICKS and COMPOSES): w_gamepad0, w_connected,
-- w_not_detected, w_slots, w_tick, w_mouse.
--
-- Only the CHROME rides this pair — the diagnostic panels below it are the
-- sanctioned sub-signal feed, scene-drawn straight from the device snapshot.

local M = {}

local CONTEXTS = { "world", "menu", "radial", "textentry" }

local function n(key)
  return (Model and Model[key]) or 0
end

local function w(key)
  return (Model and Model[key]) or ""
end

local function b(key)
  return (Model and Model[key]) or false
end

function M.derive()
  local out = {}

  -- The context tab washes: the selected context's tab lights, the rest idle.
  local active = w("active_ctx")
  for _, ctx in ipairs(CONTEXTS) do
    out["ctx_" .. ctx .. "_style"] = (ctx == active) and "controllertester.tab_active"
      or "controllertester.tab_idle"
  end

  -- The device status line + its wash.
  local present = b("connected")
  out.status = string.format("%s %s   ·   %d %s   ·   %s %d",
    w("w_gamepad0"), present and w("w_connected") or w("w_not_detected"),
    n("slots"), w("w_slots"), w("w_tick"), n("tick"))
  out.status_color = present and "controllertester.ok" or "controllertester.off"

  -- The pointer readout (L/R/M are device identifiers, not copy).
  local flag = function(key) return b(key) and 1 or 0 end
  out.mouse_line = string.format("%s %.0f,%.0f  L%d R%d M%d",
    w("w_mouse"), n("mouse_x"), n("mouse_y"), flag("mouse_l"), flag("mouse_r"), flag("mouse_m"))

  return out
end

return M
