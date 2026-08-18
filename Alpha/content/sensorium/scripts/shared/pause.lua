-- Pause overlay — the SHARED example pair-script (the SceneName.lua half of the pair;
-- five-line architecture 491BD9BB). The pop-up's LAYOUT is the static tree in
-- `scenes/shared/pause.scene.json`; every component AND its action (Resume / Settings /
-- Main Menu / Quit → the transitions `PauseScene` runs) is hardened Rust. This
-- untrusted, end-user-editable layer holds NO structure and NO logic that affects
-- hardened behavior — the client is in the enemy's hands, so a tree-builder or a
-- transition decided here would be an exploit surface (69E82FE7). It may only operate
-- exposed knobs.
--
-- WHY THIS FILE EXISTS: it states the DEFAULTS a human overrides when authoring their own
-- pause-style screen. It is the worked example for the shared modal — copy it into your
-- scene beside a copy of the tree and vary it. `arrange()` lights each optional item's
-- visibility slice and the tree gates the matching button on it (`visible_bind`), exactly
-- as `settings.lua` gates the settings sections and `Main.lua` gates the realm pages.
--
-- The defaults below show every optional item. Flip one to `false` in your copy to drop
-- it — e.g. hide Quit on a build with no desktop to return to, or Main Menu mid-tutorial.
-- Resume is deliberately NOT gated here: it is the always-present way out, and Esc /
-- pad-B resumes regardless (the tree's `on_cancel = resume`), so no override can strand
-- the player.
--
-- MenuView folds this each frame (set_model ▸ arrange ▸ to_model): each `{ on = bool }`
-- flattens onto its `visible_bind` key. A missing slice reads as off (hidden), so the
-- embedded default here is the source of the shown-by-default behavior — and a drift gate
-- pins these keys to the tree's `visible_bind`s so the pair cannot fall out of step.

local M = {}

function M.arrange()
  -- The default pause menu offers all three optional destinations. Override any to
  -- `false` in your own copy; Resume is always present and needs no slice.
  return {
    ["show_settings"]  = { on = true },
    ["show_main_menu"] = { on = true },
    ["show_quit"]      = { on = true },
  }
end

return M
