-- Populous bench — the Lua ORCHESTRATION layer.
--
-- The bench's whole component tree is authored as DATA (a static tree). This
-- script's ONE job is `arrange()`: given the currently-selected page/tab — a
-- two-way-bound value the engine publishes into `Model` — return WHICH slice of
-- the tree is shown.
--
-- It NEVER touches per-frame data. The size dial's number and the stat readouts
-- flow engine<->component directly and never pass through here; `arrange()` reads
-- only the SELECTION, on change, and returns on/off. That split is the ratified
-- contract (DD9F3836 + its on-change rider, 2026-08-11).
--
-- Gating is by SELECTION, not by content: every component authored for a given
-- (page, tab) is wrapped in the tree on the key `shown_p<page>_t<tab>`. `arrange()`
-- lights the current one and darkens the rest. A new tab or page is one more line
-- here and one more gated slice in the tree — never a change to HOW it works.

local M = {}

function M.arrange()
  local page = (Model and Model.page) or 0
  local tab = (Model and Model.tab) or 0
  return {
    -- Page-level keys: which tab RAIL shows (one rail per page — the catalog's
    -- per-page PTT pattern) and which centre-pane view is lit.
    ["shown_page0"] = { on = (page == 0) },              -- the WORLD page (the globe)
    ["shown_page1"] = { on = (page == 1) },              -- the HEX page (one column)
    -- Slice keys: one per (page, tab), lighting that tab's pane interiors.
    ["shown_p0_t0"] = { on = (page == 0 and tab == 0) }, -- the MAP tab's slice
    ["shown_p0_t1"] = { on = (page == 0 and tab == 1) }, -- the SEAMS tab's slice
    ["shown_p0_t2"] = { on = (page == 0 and tab == 2) }, -- the CRUST tab's slice
    ["shown_p0_t3"] = { on = (page == 0 and tab == 3) }, -- the EVOLVE tab's slice
    ["shown_p1_t0"] = { on = (page == 1 and tab == 0) }, -- the STACK tab's slice
  }
end

return M
