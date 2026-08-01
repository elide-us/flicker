-- flicker-solarbirth HUD — DECLARATIVE component tree: the cinematic's text
-- readout (title · live flight-phase line · controls hint · the fixed roster
-- legend), replacing the scene's immediate `draw_text` block. `M.tree()` returns
-- component instances; the Rust walker (flicker-widgets `run_ui`) owns layout,
-- draw, and hit-test. Bare text on space — no styled containers — so the readout
-- never claims `hud_hit` and drag-to-orbit keeps working across it.
--
--   * `phase` rides `text_bind` — the scene pre-formats "the Prism system · …"
--     each frame (Rust owns the formatting, as everywhere).
--   * The roster legend rows come from the `ROSTER` data global the scene
--     publishes once at enter (`{ { name }, … }`, inner → outer): the name
--     picks each row's colour path (`solarbirth.roster.<name>` — the HUD tint
--     palette in ui_elements.json), while the row TEXT ("Home  (rocky, moon)")
--     rides a `roster_<i>` Model bind — display DATA formatted by Rust, so the
--     strings gate stays clean (planet names + class labels are data, not copy).
--   * `on_menu = "pause_open"` is the screen's input DECLARATION (S9): the
--     walker layer consumes Menu and the scene maps the fired name onto its
--     pause push — the scene root's hardcoded Menu arm is gone.

local M = {}

local function tag(kind)
  return function(t)
    t.component = kind
    return t
  end
end
local Screen = tag("screen")
local Cell = tag("cell")
local Row = tag("row")
local Stack = tag("stack")
local Text = tag("text")

function M.tree()
  if not UI or not UI.solarbirth then
    -- The degenerate no-UI root keeps the declared binding (S9).
    return Screen { id = "solarbirth", on_menu = "pause_open" }
  end

  local col = {}
  -- Row heights reproduce the legacy line positions: title 16→50, phase 50→74,
  -- hint 74→104, roster header 104→126, then 18px legend rows indented 8px.
  col[#col + 1] = Text {
    text = "$sb_flicker_solarbirth",
    size = 34,
    text_size = 24,
    color = "solarbirth.title.color",
  }
  col[#col + 1] = Text {
    text_bind = "phase",
    size = 24,
    text_size = 16,
    color = "solarbirth.phase.color",
  }
  col[#col + 1] = Text {
    text = "$sb_drag_rotate_wheel_zoom_space_replay_fly",
    size = 30,
    text_size = 13,
    color = "solarbirth.hint.color",
  }
  col[#col + 1] = Text {
    text = "$sb_planets_inner_outer",
    size = 22,
    text_size = 14,
    color = "solarbirth.roster_header.color",
  }
  for i, p in ipairs(ROSTER or {}) do
    col[#col + 1] = Row {
      size = 18,
      children = {
        Stack { size = 8 },
        Text {
          text_bind = "roster_" .. i,
          grow = 1,
          text_size = 13,
          color = "solarbirth.roster." .. string.lower(p.name),
        },
      },
    }
  end

  return Screen {
    id = "solarbirth",
    on_menu = "pause_open",
    children = {
      Cell {
        anchor = "top_left",
        offset = { 16, 16 },
        width = 520,
        gap = 0,
        children = col,
      },
    },
  }
end

return M
