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
  -- The POPUP PANEL card's dismissable toggle (ruling DA0E1B57) — seeded ON, because
  -- that is the component's own default: a modal is never a trap unless something is
  -- deliberately holding it. Untick the card's checkbox and the walker starts swallowing
  -- Cancel for that slab, exactly as `scripts/shared/busy.lua` does while work runs.
  cat_popup_dismissable = true,
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

  -- ── RECIPES page (the canonical arrangements) ──
  cat_rec_field_val = "vertex_cache",
  cat_rec_select_val = 0,
  cat_rec_stepper_val = 4,
  cat_rec_gauge_val = 0.62,
}

-- The TREE ROW recipe's depth, as the ARRANGE BIND the walker reads for an
-- anchored node (`<id>_off_x`). A hierarchy is rows plus an indent VALUE — never
-- a nest of containers, and never a `tree_row` kind (retired 2026-08-13).
local TREE_INDENT = { cat_rec_tree_r1 = 0, cat_rec_tree_r2 = 18, cat_rec_tree_r3 = 36 }

-- The GADGET filler card's authored mode gate: the scene publishes the mode
-- NAMES and the bench maps them (`modes_from_names`). The filler never reads the
-- Model — that is the five-line split at the filler seam. Translate only here, so
-- the card shows one handle set and its Aim -> Locked -> Modify colours.
local GADGET_MODES = "translate"

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

  -- The tree-row recipe's indents, and the gadget card's mode gate.
  for id, off in pairs(TREE_INDENT) do
    out[id .. "_off_x"] = off
  end
  out.cat_gadget_modes = GADGET_MODES

  return out
end

return M
