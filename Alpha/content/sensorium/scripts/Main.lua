-- Main menu — the Lua ORCHESTRATION layer (the menu's PRIMARY input context).
--
-- The menu's whole component tree is authored as DATA (menu.tree.json). This
-- script's ONE job is `arrange()`: decide WHICH realm's scene-selector slice is
-- shown on the right, from WHICH realm button was last pressed. It never rebuilds
-- structure and never touches per-frame data — it reads the SELECTION and returns
-- on/off, exactly like populous.lua.
--
-- How a press reaches here (signal-subscription model, 67DEE93A): a realm button
-- fires `mode_<realm>` — the SAME signal from a mouse click OR a controller
-- Confirm on the focused button (37722F91). The engine mirrors a fired result
-- name for ONE frame as `Model.sig_<name>` (MenuView's S9 mirror). So a press
-- shows up here exactly once as `Model.sig_mode_<realm> == true`.
--
-- That mirror is TRANSIENT (one frame), so `arrange()` LATCHES it into a
-- PERSISTENT module field `M.page` — the ScriptHost VM lives for the scene, so the
-- field survives across `arrange()` calls. Mouse and pad fire the same signal, so
-- both set the same page: that is the controller==mouse fix (no cross-group nav is
-- needed to CHANGE realm — the mode buttons live in the always-reachable popup).
--
-- Gating is by SELECTION, not content: the tree wraps each realm's scene rows in a
-- slice keyed `shown_realm_<n>`; `arrange()` lights the current one and darkens the
-- rest (the `{ on = bool }` visibility the engine flattens onto each slice's
-- `visible_bind`). A new realm is one more row in `REALM_SIGNALS` here and one more
-- gated slice in the tree — never a change to HOW it works.

local M = {}

-- The resting page: 0 = the landing placeholder (Aaron's "Page 0"). The realm buttons
-- latch pages 1..4; the nav buttons stay visible throughout while the content panel pages.
M.page = 0

-- `sig_mode_<realm>` (the one-frame press mirror) -> that realm's page index, in
-- REALMS order (adventurer/dm/gamemaster/developer — the shell's REALM_* order).
-- The realm NAME lives only in the SIGNAL (the button's action is `mode_<realm>`);
-- the page is a NUMBER everywhere in the gating channel, and each `shown_realm_<n>`
-- slice keys off that number (an index is a number — 1B64FF03).
local REALM_SIGNALS = {
  { sig = "sig_mode_adventurer", page = 1 },
  { sig = "sig_mode_dm",         page = 2 },
  { sig = "sig_mode_gamemaster", page = 3 },
  { sig = "sig_mode_developer",  page = 4 },
}

function M.arrange()
  -- Latch this frame's transient press mirror into the persistent page.
  if Model then
    for _, r in ipairs(REALM_SIGNALS) do
      if Model[r.sig] then
        M.page = r.page
        break
      end
    end
    -- Cancel/deselect: a Cancel (Esc / pad-B) while a realm page is up fires
    -- `menu_back` (the screen's `on_cancel`, set only when a page is selected). Its
    -- one-frame mirror returns to the landing placeholder — page 0.
    if Model.sig_menu_back then
      M.page = 0
    end
  end
  -- Light exactly the current page's slice; darken the rest (page 0 = the landing
  -- placeholder, pages 1..4 = the realm scene selectors).
  return {
    ["shown_realm_0"] = { on = (M.page == 0) },
    ["shown_realm_1"] = { on = (M.page == 1) },
    ["shown_realm_2"] = { on = (M.page == 2) },
    ["shown_realm_3"] = { on = (M.page == 3) },
    ["shown_realm_4"] = { on = (M.page == 4) },
  }
end

return M
