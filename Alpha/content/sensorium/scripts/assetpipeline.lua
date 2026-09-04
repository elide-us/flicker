-- Clayworks bench — the Lua ORCHESTRATION layer.
--
-- The bench's whole component tree is authored as DATA (assetpipeline.scene.json).
-- This script's ONE job is `arrange()`: given the SELECTION the engine publishes —
-- which WORKFLOW is open (`wf`, a name) and which STEP tab is selected (`tab`, the
-- rail's bound index) — return which slice of the tree is shown.
--
-- It NEVER touches per-frame data. Slider values, the facts column, the bone list and
-- the status lines flow engine<->component directly and never pass through here;
-- `arrange()` reads only the selection, on change, and returns on/off.
--
-- Gating is by SELECTION, not by content: every component authored for a step is
-- wrapped in the tree on the key `shown_t_<step>`; the step rail for a workflow on
-- `shown_wf_<workflow>`; the centre view on `shown_view_<kind>`. A new step or a new
-- workflow is one more entry in STEPS here and one more gated slice in the tree —
-- never a change to HOW it works.
local M = {}

-- Each workflow's step rail, in the rail's authored order (the `tab` index indexes it).
local STEPS = {
  character = { "source", "prep", "rig", "preview", "attach", "review" },
  prop      = { "source", "mount", "review" },
  animation = { "source", "clip", "review" },
}

-- Which centre view a step shows: the four-panel rig view while a body is being
-- prepared or rigged, the bake view on preview, the two-clip pair on the clip step.
local VIEW = {
  prep = "quad", rig = "quad", mount = "quad", preview = "bake", clip = "clip",
}

function M.arrange()
  local wf = (Model and Model.wf) or "character"
  local tab = (Model and Model.tab) or 0
  local steps = STEPS[wf] or STEPS.character
  local step = steps[tab + 1] or steps[1]
  local view = VIEW[step] or "none"
  local out = {
    ["shown_wf_character"] = { on = (wf == "character") },
    ["shown_wf_prop"]      = { on = (wf == "prop") },
    ["shown_wf_animation"] = { on = (wf == "animation") },
    ["shown_view_quad"]    = { on = (view == "quad") },
    ["shown_view_bake"]    = { on = (view == "bake") },
    ["shown_view_clip"]    = { on = (view == "clip") },
    ["shown_view_none"]    = { on = (view == "none") },
    -- The footer's one swap: the flow's LAST stop (review) shows EXPORT where every
    -- other stop shows NEXT.
    ["shown_ft_commit"]    = { on = (step == "review") },
    ["shown_ft_next"]      = { on = (step ~= "review") },
    -- The discard dialog: the engine publishes its runtime flag (`discard_open`, raised
    -- when Back leaves a dirty step) and this lights the modal's slice.
    ["shown_discard"]      = { on = ((Model and Model.discard_open) == true) },
  }
  for _, name in ipairs({ "source", "prep", "rig", "mount", "preview", "attach", "clip", "review" }) do
    out["shown_t_" .. name] = { on = (step == name) }
  end
  return out
end

-- The ORCHESTRATION half: given a scene-level signal, say where the rail goes. The
-- engine folds the returned writes into the same results drain a click lands in, so
-- `tab = 1` here IS a step change — the one place the flow's "what happens after"
-- lives (the successor of the old workflow runtime's wf_next).
--   loaded      — a folder opened into `sig.wf`: leave Source for the first working stop.
--   next_piece  — the next mesh of a multi-mesh folder was started: back to Source.
function M.react(sig)
  if sig.loaded then return { tab = 1 } end
  if sig.next_piece then return { tab = 0 } end
  return {}
end

return M
