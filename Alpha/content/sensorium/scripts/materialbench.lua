-- MaterialBench — the scene's LOGIC (the SceneName.lua half of the pair): the
-- MATERIAL DESIGNER's presentation.
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   node_count, sel_node_index  -- the node list's length and the selected row (-1: none)
--   sel_kind                    -- the selected node's kind id ("" when none is selected)
--   sel_map, sel_page           -- the view tab (0..7, LIT last) and the page (0 | 1)
--   out_*_src, out_emit_color   -- the output stage's source options (0 = constant / none)
--   glow_count                  -- how many swatches the glow palette has
--   renaming, material_bound, status_line, problem_count, … -- values + text
--
-- `derive()` owns the presentation logic: the node-row and swatch washes, the one
-- parameter cell `sel_kind` names, the view and page gates, the rename gates, the
-- constant sliders' gates. Every value returned is a scalar; style values are dotted
-- paths — Rust never publishes one.
--
-- KINDS and MAPS must match the authored tree (materialbench.scene.json) and the
-- graph's NodeKind::catalog() / MapKind::ALL — the crate's drift gates pin all three.

local M = {}

local ROW = "materialbench.node_row"
local ROW_SEL = "materialbench.node_row_sel"
local SWATCH = "materialbench.swatch"
local SWATCH_SEL = "materialbench.swatch_sel"

local KINDS = {
  "noise", "constant", "gradient", "shape",
  "levels", "contrast", "invert", "blur", "threshold", "band", "curvature",
  "warp", "tile", "modulate", "occlusion",
  "blend", "ramp", "palette", "mix_color",
}

local MAPS = {
  "map_base", "map_normal", "map_rough", "map_metal",
  "map_ao", "map_height", "map_emit",
}

function M.derive()
  local out = {}
  local m = Model or {}

  -- The node list: the selected row wears the wash, the rest rest.
  local sel = m.sel_node_index or -1
  for i = 0, (m.node_count or 0) - 1 do
    out["nl" .. i .. "_sty"] = (sel == i) and ROW_SEL or ROW
  end

  -- Exactly one parameter cell shows — the selected node's kind; none (and the hint)
  -- while nothing is selected.
  local kind = m.sel_kind or ""
  for _, id in ipairs(KINDS) do
    out["cell_" .. id .. "_shown"] = (kind == id)
  end
  out.no_node_shown = (kind == "")

  -- Exactly one view cell shows: the flat map picked by the tabs, or the LIT sample on
  -- the last tab.
  local view = m.sel_map or 0
  for i, id in ipairs(MAPS) do
    out[id .. "_shown"] = (view == i - 1)
  end
  out.lit_shown = (view == #MAPS)

  -- A scalar output port's constant slider shows only while no field drives it.
  out.out_rough_const = (m.out_rough_src or 0) == 0
  out.out_metal_const = (m.out_metal_src or 0) == 0
  out.out_ao_const = (m.out_ao_src or 0) == 0

  -- The glow swatches: the one the glow wears is ringed (option 1 + i is swatch i).
  local glow = (m.out_emit_color or 0) - 1
  for i = 0, (m.glow_count or 0) - 1 do
    out["glow" .. i .. "_sty"] = (glow == i) and SWATCH_SEL or SWATCH
  end

  -- The designer <-> materials page switch: gate the two page bodies, wash the two tabs.
  local page = m.sel_page or 0
  out.page_bench_shown = (page == 0)
  out.page_materials_shown = (page == 1)
  out.page_bench_sty = (page == 0) and "materialbench.button_on" or "materialbench.button"
  out.page_materials_sty = (page == 1) and "materialbench.button_on" or "materialbench.button"

  -- The material rename: the dropdown yields to a draft field while editing, and the
  -- Rename affordance shows only when a material is bound and no edit is open.
  local renaming = m.renaming or false
  local bound = m.material_bound or false
  out.not_renaming = not renaming
  out.can_rename = bound and not renaming

  -- The status line and the problem count speak only when they have something to say.
  out.status_shown = (m.status_line or "") ~= ""
  out.problems_shown = (m.problem_count or 0) > 0

  return out
end

return M
