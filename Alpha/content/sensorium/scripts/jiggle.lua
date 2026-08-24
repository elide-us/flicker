-- Jiggle Bucket — the scene's LOGIC (the SceneName.lua half of the pair).
--
-- The engine publishes the RAW runtime variables into `Model` each frame:
--   score, best   -- point totals (numbers)
--   combo         -- current merge-cascade multiplier (0/1 = none)
--
-- `derive()` turns those into the display values the HUD binds — formatted here in
-- CONTENT, not in engine code (five-line split).

local M = {}

local function count(n)
  return string.format("%d", math.floor((n or 0) + 0.5))
end

function M.derive()
  local combo = (Model and Model.combo) or 0
  return {
    score = count(Model and Model.score),
    best = count(Model and Model.best),
    combo = (combo >= 2) and string.format("×%d", math.floor(combo)) or "—",
  }
end

return M
