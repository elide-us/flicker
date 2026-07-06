-- Intro splash: a sequence of logos, each fading in, holding, then fading out,
-- before the menu. Driven by ui_elements.json (`UI.logo`) + the engine `Model`
-- (elapsed seconds + each image's pixel size). Edit UI.logo to change images,
-- timing, fit, or backdrop — no recompile. Plain Lua.
--
--   update(...) -> { done = bool }   -- true once the whole sequence has played
--   draw(sw, sh) -> { backdrop, <current image> }
--
-- `UI.logo.mode` selects how each image is sized:
--   "fill"  — stretch edge-to-edge to the full frame (the sol2 default)
--   "cover" — scale to cover the frame, preserve aspect, centre (crops overflow)
--   "fit"   — letterbox inside `UI.logo.fit` of the frame, preserve aspect

local M = {}

local function clamp(v, lo, hi)
  if v < lo then
    return lo
  elseif v > hi then
    return hi
  else
    return v
  end
end

-- The current 1-based image index, its fade alpha (0..1), and the total
-- sequence length, from `UI.logo` timing and `Model.elapsed`.
local function timeline()
  local lg = UI.logo
  local per = lg.fade * 2 + lg.hold -- fade in + hold + fade out
  local total = per * #lg.images
  local elapsed = Model.elapsed or 0
  local idx = math.floor(elapsed / per) + 1
  local t = elapsed - (idx - 1) * per
  local alpha = 1
  if t < lg.fade then
    alpha = t / lg.fade
  elseif t > lg.fade + lg.hold then
    alpha = 1 - (t - lg.fade - lg.hold) / lg.fade
  end
  return idx, clamp(alpha, 0, 1), total
end

-- The destination rect for the active image given its native size and the fit mode.
local function placement(mode, iw, ih, sw, sh)
  if mode == "fill" then
    return 0, 0, sw, sh
  end
  local scale
  if mode == "cover" then
    scale = math.max(sw / iw, sh / ih)
  else -- "fit"
    local fit = UI.logo.fit or 0.9
    scale = math.min(sw * fit / iw, sh * fit / ih)
  end
  local w, h = iw * scale, ih * scale
  return (sw - w) * 0.5, (sh - h) * 0.5, w, h
end

function M.update(mx, my, clicked, sw, sh, down)
  if not (UI and Model) then
    return {}
  end
  local _, _, total = timeline()
  return { done = (Model.elapsed or 0) >= total }
end

function M.draw(sw, sh)
  if not (UI and Model) then
    return {}
  end
  local lg = UI.logo
  local bg = lg.backdrop
  local cmds =
    { { kind = "rect", x = 0, y = 0, w = sw, h = sh, r = bg[1], g = bg[2], b = bg[3], a = bg[4] } }

  local idx, alpha = timeline()
  local tex = idx <= #lg.images and Textures[lg.images[idx]]
  if tex then
    local iw = Model["img" .. idx .. "_w"] or 1
    local ih = Model["img" .. idx .. "_h"] or 1
    local x, y, w, h = placement(lg.mode or "fit", iw, ih, sw, sh)
    cmds[#cmds + 1] =
      { kind = "sprite", tex = tex, x = x, y = y, w = w, h = h, r = 1, g = 1, b = 1, a = alpha }
  end
  return cmds
end

return M
