-- Loading — the pre-load screen's pair script (page 3 of the intro chain).
--
-- The LOADING COMPONENT tree (backdrop, notice, progress bar) is authored in
-- scenes/Loading.scene.json; the `loading` behaviour (flicker-shell) runs the
-- shader-compile phase as a SIMULATED timer and publishes `loading_progress`
-- (0..1) into the Model each frame. The progress bar binds that value directly;
-- derive() only has to turn it into the human percent readout the label row shows
-- (`progress_pct` → the `text_bind` on the percent text). Copy the engine hands us
-- is content — the "%d%%" wording lives here, not in Rust (five-line architecture).
--
-- react() is the orchestration: the engine reports `done` when the timeline
-- completes (this is what will gate the real pre-load), or `cancel` on Esc / B.
-- The returned intent (`next` / `exit`) is FIRED as the scene's result and routed
-- by the scene FILE's `exits`. `confirm` is intentionally ignored — a click must
-- not skip a load in progress (the whole point of the "do not close" notice).

local M = {}

function M.derive()
  local p = (Model and Model.loading_progress) or 0
  if p < 0 then p = 0 elseif p > 1 then p = 1 end
  return {
    progress_pct = string.format("%d%%", math.floor(p * 100 + 0.5)),
  }
end

function M.react(sig)
  if sig.cancel then return { exit = true } end
  if sig.done then return { next = true } end
  return {}
end

return M
