-- Click Trainer — the scene's LOGIC (the SceneName.lua half of the pair).
--
-- The engine publishes the RAW runtime variables into `Model` each frame:
--   hits, misses          -- counters (numbers)
--   accuracy_pct          -- 0..100 (number)
--   stat_last_s, stat_best_s, stat_avg_s  -- seconds; < 0 means "no hit yet"
--
-- `derive()` turns those into the display values the HUD components bind:
-- readable strings, formatted here in CONTENT — not in engine code.

local M = {}

local function stat_text(seconds)
  if seconds == nil or seconds < 0 then
    return "—"
  end
  return string.format("%.0f ms", seconds * 1000.0)
end

function M.derive()
  local hits = (Model and Model.hits) or 0
  local misses = (Model and Model.misses) or 0
  local acc = (Model and Model.accuracy_pct) or 0
  return {
    hits = string.format("%d", hits),
    misses = string.format("%d", misses),
    accuracy = string.format("%.0f%%", acc),
    stat_last = stat_text(Model and Model.stat_last_s),
    stat_best = stat_text(Model and Model.stat_best_s),
    stat_avg = stat_text(Model and Model.stat_avg_s),
  }
end

return M
