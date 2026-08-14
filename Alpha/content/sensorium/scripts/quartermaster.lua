-- Quartermaster Bench — the scene's LOGIC (the SceneName.lua half of the pair).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   tab                -- "tab_files" | "tab_review"
--   pane               -- "tree" | "list" (the focused Files pane)
--   menu_sel           -- context-menu cursor (number), with has_menu
--   queue_len          -- staging-queue length (number)
--   has_menu/has_prompt/has_rename/has_clip/has_error, toast_<i>_on  -- gates
--   tree_slot_<s>_sel / row_slot_<s>_sel / rv_slot_<s>_sel           -- cursor bools
--   plus the windowed slot data (names/types/sizes/facts/warnings) as text.
--
-- `derive()` owns the presentation logic: page gates off the tab, pane-focus and
-- slot-wash STYLE PATHS off the raw booleans, and the menu-row highlight off
-- `menu_sel`. Every value returned is a scalar; style values are dotted paths.

local M = {}

local PANE_FOCUSED = "quartermaster.pane_focused"
local PANE_RESTING = "quartermaster.pane"
local TAB_ACTIVE = "quartermaster.tab_active"
local TAB_IDLE = "quartermaster.tab_idle"
local ROW_SEL = "quartermaster.rowsel"
local ROW_OFF = "quartermaster.rowsel_off"

local TREE_SLOTS, LIST_SLOTS, QUEUE_SLOTS = 12, 12, 8
local MENU_ROWS = 5

function M.derive()
  local out = {}
  local tab = (Model and Model.tab) or "tab_files"
  local pane = (Model and Model.pane) or "list"
  local review = tab == "tab_review"

  out.files_on = not review
  out.review_on = review
  out.tab_files_sty = (not review) and TAB_ACTIVE or TAB_IDLE
  out.tab_review_sty = review and TAB_ACTIVE or TAB_IDLE

  -- Pane focus: the walker convention's rim, driven by the bench's single focus.
  out.tree_pane_sty = (not review and pane == "tree") and PANE_FOCUSED or PANE_RESTING
  out.list_pane_sty = (not review and pane == "list") and PANE_FOCUSED or PANE_RESTING
  out.queue_pane_sty = review and PANE_FOCUSED or PANE_RESTING

  -- Slot washes: raw cursor booleans -> style paths.
  for s = 0, TREE_SLOTS - 1 do
    out["tree_slot_" .. s .. "_sty"] = (Model and Model["tree_slot_" .. s .. "_sel"]) and ROW_SEL or ROW_OFF
  end
  for s = 0, LIST_SLOTS - 1 do
    out["row_slot_" .. s .. "_sty"] = (Model and Model["row_slot_" .. s .. "_sel"]) and ROW_SEL or ROW_OFF
  end
  for s = 0, QUEUE_SLOTS - 1 do
    out["rv_slot_" .. s .. "_sty"] = (Model and Model["rv_slot_" .. s .. "_sel"]) and ROW_SEL or ROW_OFF
  end

  -- Context-menu highlight rides the pad cursor.
  local menu_sel = (Model and Model.menu_sel) or 0
  for i = 0, MENU_ROWS - 1 do
    out["menu_sty_" .. i] = ((Model and Model.has_menu) and i == menu_sel) and TAB_ACTIVE or TAB_IDLE
  end

  out.rv_empty = review and ((Model and Model.queue_len) or 0) == 0
  return out
end

return M
