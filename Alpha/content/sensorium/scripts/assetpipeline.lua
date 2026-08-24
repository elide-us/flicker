-- Clayworks Bench (assetpipeline) — the scene's LOGIC (the SceneName.lua half
-- of the pair; bench migration 2026-08-19).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   wf_task_state / wf_conform_state / wf_preview_state / wf_attach_state / wf_review_state
--       -- each rail chip's step state: "active" | "visited" | "todo"
--   pick_sel / pick_window, bone_sel / bone_window,
--   sock_sel / sock_window, att_sel_idx
--       -- the four bank cursors, NUMBERS (an index is a number — 1B64FF03);
--          the window is the bank's first visible row
--   …plus every readout/label bind the tree names (pre-formatted in Rust).
--
-- `derive()` owns the PRESENTATION selection only: which chip style block a
-- rail chip wears, and which slot wash a bank row wears. Every value returned
-- is a dotted style path into the scene's own style blocks.

local M = {}

local CHIPS = { "task", "prep", "conform", "preview", "attach", "review" }

local ROWSEL = "assetpipeline.rowsel"
local ROWSEL_OFF = "assetpipeline.rowsel_off"
local ROWS = 6

local function n(key)
  return (Model and Model[key]) or 0
end

-- One bank's six row washes: the visible row holding the selection wears the
-- accent wash, the rest draw (almost) nothing under their bound labels.
local function bank(out, prefix, sel, window)
  for i = 0, ROWS - 1 do
    out[prefix .. i .. "_sty"] = (window + i == sel) and ROWSEL or ROWSEL_OFF
  end
end

function M.derive()
  local out = {}

  -- The step rail: each chip's style path from the state the runtime published.
  for _, id in ipairs(CHIPS) do
    local state = (Model and Model["wf_" .. id .. "_state"]) or "todo"
    out["wf_" .. id .. "_style"] = "workflow.chip." .. state
  end

  -- The four slot banks' selection washes.
  bank(out, "pick_", n("pick_sel"), n("pick_window"))
  bank(out, "bone_", n("bone_sel"), n("bone_window"))
  bank(out, "sock_", n("sock_sel"), n("sock_window"))
  bank(out, "att_", n("att_sel_idx"), 0)

  return out
end

return M
