-- Prism Settings — the SceneName.lua half of the settings pair (five-line
-- architecture 491BD9BB): the scene's ONLY runtime behavior. The LAYOUT is the
-- static tree in `scenes/shared/settings.scene.json`; every component is hardened
-- Rust; the hardened `UnifiedSettingsScene` owns all state, values, apply, and
-- rebind. This untrusted, end-user-editable layer therefore holds NO structure and
-- NO logic that affects hardened behavior — the client is in the enemy's hands, so
-- a tree-builder or control-selection logic here would be an exploit surface. It
-- may only operate exposed knobs, and here that is exactly ONE: section + input
-- sub-tab VISIBILITY, derived from the published index Model.
--
-- The engine publishes the RAW runtime variables into `Model` each frame; `derive()`
-- returns the DERIVED visibility gates the frame folds in before the walker runs
-- (`settings_page` → which section shows, `input_subtab` → which input sub-tab shows).

local M = {}

function M.derive()
  local page = (Model and Model.settings_page) or 0
  local sub  = (Model and Model.input_subtab) or 0
  return {
    sec_video = page == 0,
    sec_audio = page == 1,
    sec_input = page == 2,
    input_page_active = page == 2,
    sub_keyboard = sub == 0,
    sub_mouse = sub == 1,
    sub_controller = sub == 2,
  }
end

return M
