-- Display-confirm overlay — the SHARED example pair-script (the SceneName.lua half of the
-- pair; five-line architecture 491BD9BB). The pop-up's LAYOUT is the static tree in
-- `scenes/shared/confirm.scene.json`; the Keep / Revert actions and the live revert
-- countdown (the `subtitle` bind) are hardened Rust (`ConfirmDisplayScene`). This
-- untrusted, end-user-editable layer holds NO structure and NO logic that affects
-- hardened behavior (69E82FE7) — it may only operate exposed knobs.
--
-- WHY THIS FILE EXISTS: it states the DEFAULTS a human overrides when authoring their own
-- confirm-style screen — the worked example for the shared modal. `arrange()` lights each
-- button's visibility slice and the tree gates the button on it (`visible_bind`), the same
-- mechanism `settings.lua` / `Main.lua` use.
--
-- Both buttons show by default. Flip one to `false` in your copy to drop it. The
-- auto-revert TIMEOUT is the always-present safety net — this modal carries no
-- `on_cancel` by design (Keep / Revert / timeout only), so the pending change is never
-- left un-decided even if an override hid a button.
--
-- MenuView folds this each frame (set_model ▸ arrange ▸ to_model): each `{ on = bool }`
-- flattens onto its `visible_bind` key. A missing slice reads as off (hidden), so the
-- embedded default here is the source of the shown-by-default behavior — and a drift gate
-- pins these keys to the tree's `visible_bind`s so the pair cannot fall out of step.

local M = {}

function M.arrange()
  return {
    ["show_keep"]   = { on = true },
    ["show_revert"] = { on = true },
  }
end

return M
