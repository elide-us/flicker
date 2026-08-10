-- Intro splash: ONE full-screen logo that fades in, holds, then fades out.
-- Driven by ui_elements.json (`UI.logo` — timing, fit, backdrop) + the engine
-- `Model` (elapsed seconds + the image's pixel size). Edit UI.logo to change the
-- timing, fit, or backdrop — no recompile. Plain Lua.
--
--   update(...) -> { done = bool }   -- true once this splash has played
--   draw(sw, sh) -> { backdrop, <the image, faded> }
--
-- THE SEQUENCE IS NOT HERE ANY MORE. This script used to walk a LIST of images
-- (`UI.logo.images`) on one timeline, which made "publisher logo, then engine
-- logo, then menu" a fact buried in a Lua array. That ordering is a SCENE CHAIN
-- now: each splash is its own registered scene, and when it reports `done` its
-- SCENE FILE (`content/sensorium/scenes/<id>.json`, the `exits` map) says what
-- follows — `teg_logo` -> `ce_logo` -> `main_menu`. The order is authored where a
-- human can see and reorder it. One splash, one scene, one image.
--
-- The scene registers its image under the texture name `logo`, whichever image
-- that scene carries — so this script never learns which logo it is drawing.

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

-- This splash's fade alpha (0..1) and its total length, from `UI.logo` timing
-- and `Model.elapsed`.
local function timeline()
  local lg = UI.logo
  local total = lg.fade * 2 + lg.hold -- fade in + hold + fade out
  local t = Model.elapsed or 0
  local alpha = 1
  if t < lg.fade then
    alpha = t / lg.fade
  elseif t > lg.fade + lg.hold then
    alpha = 1 - (t - lg.fade - lg.hold) / lg.fade
  end
  return clamp(alpha, 0, 1), total
end

function M.update(mx, my, clicked, sw, sh, down)
  if not (UI and Model) then
    return {}
  end
  local _, total = timeline()
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

  local alpha = timeline()
  local tex = Textures.logo
  if tex then
    -- The image's native size comes from the Model; fit it inside `fit` of the
    -- screen, preserving aspect, and centre it.
    local iw = Model.img_w or 1
    local ih = Model.img_h or 1
    local scale = math.min(sw * lg.fit / iw, sh * lg.fit / ih)
    local w, h = iw * scale, ih * scale
    cmds[#cmds + 1] = {
      kind = "sprite",
      tex = tex,
      x = (sw - w) * 0.5,
      y = (sh - h) * 0.5,
      w = w,
      h = h,
      r = 1,
      g = 1,
      b = 1,
      a = alpha,
    }
  end
  return cmds
end

return M
