-- Sablework Bench — the scene's LOGIC (the SceneName.lua half of the pair).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   sel_ch             -- the selected voice, a NUMBER 0..5
--   sel_map            -- the view tab, a NUMBER 0..7 (7 flat maps, then LIT)
--   ch<n>_on / ch<n>_scale / … / recipe_line / commit_status …  -- values + text
--
-- `derive()` owns the presentation logic: the rack-row washes off `sel_ch`, and
-- the view-cell visibility gates off `sel_map`. Every value returned is a
-- scalar; style values are dotted paths.
--
-- VOICES and MAPS must match the authored tree (sablework.scene.json) and the
-- instrument's CHANNEL_COUNT / MapKind::ALL — the crate's drift gates pin all
-- three to each other.

local M = {}

local ROW = "sablework.row"
local ROW_SEL = "sablework.row_sel"

local VOICES = 6
local MAPS = {
  "map_base", "map_normal", "map_rough", "map_metal",
  "map_ao", "map_height", "map_emit",
}

function M.derive()
  local out = {}

  -- The selected voice's row wears the wash; the rest rest.
  local sel = (Model and Model.sel_ch) or 0
  for n = 1, VOICES do
    out["ch" .. n .. "_sty"] = (sel == n - 1) and ROW_SEL or ROW
  end

  -- Exactly one view cell shows: the flat map picked by the tabs, or the LIT
  -- turntable on the last tab.
  local view = (Model and Model.sel_map) or 0
  for i, id in ipairs(MAPS) do
    out[id .. "_shown"] = (view == i - 1)
  end
  out.lit_shown = (view == #MAPS)

  -- The material rename: the dropdown yields to a draft field while editing, and the Rename
  -- affordance shows only when a material is bound and no edit is open. (`renaming` is
  -- published raw by the behaviour; these are the derived companions the tree gates on.)
  local renaming = (Model and Model.renaming) or false
  local bound = (Model and Model.material_bound) or false
  out.not_renaming = not renaming
  out.can_rename = bound and not renaming

  return out
end

return M
