-- WorldBench V5 — the Lua ORCHESTRATION layer.
--
-- The bench's whole component tree is authored as DATA (worldbench.scene.json).
-- This script's ONE job is `arrange()`: given the currently-selected page/tab —
-- a two-way-bound value the engine publishes into `Model` — return WHICH slice
-- of the tree is shown. It never touches per-frame data (the ratified contract
-- DD9F3836 + its on-change rider).
--
-- Gating is by SELECTION: every component authored for a given (page, tab) is
-- wrapped on the key `shown_p<page>_t<tab>`. A new field tab is one more line.

local M = {}

function M.arrange()
  local page = (Model and Model.page) or 0
  local tab = (Model and Model.tab) or 0
  local out = { ["shown_page0"] = { on = (page == 0) } }
  -- One slice per tab of the WORLD page: wave, ridge, 8/9, 1/13, 1/15, coast, heat, emitters.
  for t = 0, 7 do
    out["shown_p0_t" .. t] = { on = (page == 0 and tab == t) }
  end
  return out
end

return M
