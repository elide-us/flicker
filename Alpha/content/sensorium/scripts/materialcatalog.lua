-- Material Catalog — the scene's LOGIC (the SceneName.lua half of the pair).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   mc_name_<id>        -- an ASSIGNED slot's material name (data; absent for a free slot)
--   mc_slot_count       -- how many slots the material byte has (a NUMBER, 256)
--   mc_emissive_first   -- the first slot of the block held for EMISSIVE materials (a NUMBER)
--   mc_assigned / mc_generated / mc_free / mc_emissive  -- the tally, pre-formatted digits
--
-- `derive()` owns the presentation logic: an emissive slot that holds no material yet has
-- no name, so its name row reads the stringtable's word for what the slot is held for; an
-- emissive slot that holds one (the eight curated glows) keeps its material's name. The
-- range comes from the behaviour's two published numbers — never a literal here, so the
-- block's boundary lives in exactly one place (flicker_materials::EMISSIVE_FIRST).

local M = {}

function M.derive()
  local out = {}

  local first = Model and Model.mc_emissive_first
  local count = Model and Model.mc_slot_count
  if first and count then
    -- Model numbers arrive as floats; `math.floor` makes the loop variable an integer so
    -- the key spells `mc_name_248`, never `mc_name_248.0`.
    for id = math.floor(first), math.floor(count) - 1 do
      if Model["mc_name_" .. id] == nil then
        out["mc_name_" .. id] = "$mc_emissive"
      end
    end
  end

  return out
end

return M
