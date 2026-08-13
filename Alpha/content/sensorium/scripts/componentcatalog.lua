-- Component Catalog — the scene's LOGIC (the SceneName.lua half of the pair).
--
-- The engine publishes the RAW runtime variables into `Model` each frame:
--   section            -- index of the card at the top of the tray (number)
--   card_count         -- how many cards the tree ships (number)
--   cat_pm_page, cat_pm_tab   -- the Paged Menu card's two-way selections
--   cat_* value binds  -- each demo control's committed value (echoed back)
--
-- `derive()` owns the catalog's component logic:
--   * seeds every demo control's INITIAL value (all features on / mid-range),
--     yielding to the live committed value once one exists;
--   * lights the active nav bookmark (primary slab) and rests the others;
--   * gates the Paged Menu card's per-page tab rails + content off (page, tab).

local M = {}

-- The demo seeds — the values each card shows before anyone touches it.
local seeds = {
  cat_check_val = true,
  cat_toggle_val = true,
  cat_radio_val = "a",
  cat_tile_on = true,
  cat_tile_loaded = true,
  cat_pill_val = 1,
  cat_select_val = 1,
  cat_slider_val = 60,
  cat_stepper_val = 3,
  cat_field_val = "hello",
  cat_gauge_val = 0.5,
  cat_rgauge_val = 0.72,
  cat_pm_page = 0,
  cat_pm_tab = 0,
  cat_pm_tabs_shown = true,
  -- The splash card's clock, parked mid-hold so the demo shows at full alpha.
  elapsed = 0.9,
}

local NAV_ACTIVE = "modal.buttons.variants.primary"
local NAV_IDLE = "modal.buttons.variants.secondary"

function M.derive()
  local out = {}

  -- Seed any demo value the engine has not echoed a committed value for.
  for key, seed in pairs(seeds) do
    if Model == nil or Model[key] == nil then
      out[key] = seed
    end
  end

  -- The active bookmark wears the primary slab; the rest rest.
  local section = (Model and Model.section) or 0
  local count = (Model and Model.card_count) or 0
  for i = 0, count - 1 do
    out["nav_sty_" .. i] = (i == section) and NAV_ACTIVE or NAV_IDLE
  end

  -- The Paged Menu card: Page 1 shows Tab 1/2, Page 2 shows Tab 3/4.
  local page = (Model and Model.cat_pm_page) or seeds.cat_pm_page
  local tab = (Model and Model.cat_pm_tab) or seeds.cat_pm_tab
  out.cat_pm_on_p0 = page == 0
  out.cat_pm_on_p1 = page == 1
  out.cat_pm_p0_t0 = page == 0 and tab == 0
  out.cat_pm_p0_t1 = page == 0 and tab == 1
  out.cat_pm_p1_t0 = page == 1 and tab == 0
  out.cat_pm_p1_t1 = page == 1 and tab == 1

  return out
end

return M
