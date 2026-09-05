-- Busy / progress overlay — the SHARED example pair-script (the SceneName.lua half of the
-- pair; five-line architecture 491BD9BB). The pop-up's LAYOUT is the static tree in
-- `scenes/shared/busy.scene.json`; the bar's fraction, the done flag and the caller's
-- Cancel option are hardened Rust (`SharedModal` reads the shared `ModalProgress` handle
-- the host that opened the modal wrote). This untrusted, end-user-editable layer holds NO
-- structure and NO logic that affects hardened behavior (69E82FE7) — it may only operate
-- exposed knobs.
--
-- WHY THIS FILE EXISTS: it owns the ONE knob this modal has — WHETHER IT MAY BE
-- DISMISSED (ruling DA0E1B57, Aaron 2026-09-04: "if it's dismissable or not should be a
-- behavioral toggle on the component"). `popup_panel` carries `dismissable` /
-- `dismissable_bind`; the walker swallows Esc / pad-B for the screen while the topmost
-- slab reads false. Everything else about this modal is the caller's params.
--
-- THE RULE, in one line: you may leave a busy modal when there is something to abort
-- (the caller declared it cancellable) or when there is nothing left to wait for (the
-- work is done). Mid-work with nothing to cancel, Cancel is refused — the operation is
-- running and there is no honest way to stop it, so the overlay does not pretend.
--
-- The engine publishes the RAW state into `Model` each frame — `modal_cancellable` (the
-- caller declared a cancel option), `modal_progress` (0..1) and `modal_done` (the work
-- finished, or a modal with no work handle at all, which is done before it starts).
-- `arrange()` folds them into the ONE derived flag the tree binds. A missing key reads
-- as nil ⇒ falsy, and the component's own default (dismissable) catches a modal this
-- script never saw — the toggle can be lost, the way out cannot (B89FAC21).
--
-- MenuView folds this each frame (set_model ▸ arrange ▸ to_model): each `{ on = bool }`
-- flattens onto its bind key, so `modal_dismissable` lands where the tree's
-- `dismissable_bind` reads it. Flip the expression in your own copy to vary the policy —
-- e.g. `on = true` for a bar you may always walk away from.

local M = {}

function M.arrange()
  local cancellable = (Model and Model.modal_cancellable) or false
  local done = (Model and Model.modal_done) or false
  return {
    ["modal_dismissable"] = { on = cancellable or done },
  }
end

return M
