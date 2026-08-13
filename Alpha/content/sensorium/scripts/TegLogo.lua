-- TegLogo — the publisher splash's pair script.
--
-- The splash COMPONENT (drawing, fade math, hit) lives in the Rust widgets
-- crate. arrange() configures that object's features: the image it plays and
-- its fade timeline, as properties on the scene tree's `splash` node.
-- react() is the orchestration: the engine reports the fired signals — `done`
-- when the timeline completes, `confirm` on click / A / Enter / Space,
-- `cancel` on Esc / B — and the intent returned here (`next` or `exit`) is
-- routed to a scene by this scene's FILE (the `exits` map in
-- scenes/TegLogo.scene.json).

local M = {}

function M.arrange()
  return {
    splash = {
      on = true,
      image = "package/sensorium/assets/elideus_productions_yellow.png",
      fade_in = 0.6,
      hold = 1.2,
      fade_out = 0.6,
    },
  }
end

function M.react(sig)
  if sig.cancel then return { exit = true } end
  if sig.done or sig.confirm then return { next = true } end
  return {}
end

return M
