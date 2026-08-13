-- Solar Birth — the scene's LOGIC (the SceneName.lua half of the pair).
--
-- The engine publishes the RAW runtime variables into `Model` each frame:
--   segment          -- the flight's current segment name (text)
--   progress_pct     -- 0..100 (number); 100 = the fly-in has settled
--   sys, approaching, settled   -- stringtable copy, pre-resolved
--
-- `derive()` composes the phase line + hint the readout panel binds. Copy is
-- composed here in CONTENT — the engine only supplies data and resolved tokens.

local M = {}

function M.derive()
  local sys = (Model and Model.sys) or ""
  local seg = (Model and Model.segment) or ""
  local pct = (Model and Model.progress_pct) or 0
  local phase
  if pct >= 100 then
    phase = string.format("%s · %s · %s", sys, seg, (Model and Model.settled) or "")
  else
    phase =
      string.format("%s · %s · %s %.0f%%", sys, seg, (Model and Model.approaching) or "", pct)
  end
  return {
    phase = phase,
  }
end

return M
