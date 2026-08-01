-- flicker-poc-chemistry HUD — DECLARATIVE component tree: the sim's text
-- readout (loading banner · title/stats/interior/state/crust lines) plus the
-- text-only CONSERVATION LEDGER panel (top-right), replacing the scene's
-- immediate `draw_text` block for those pieces. `M.tree()` returns component
-- instances; the Rust walker (flicker-widgets `run_ui`) owns layout, draw, and
-- hit-test.
--
-- Still scene-drawn (FLAGGED, S10): the bulk-seed element panel and the
-- tectonic-event log — their per-row colours are DATA (element_rgb / per-event
-- tints), and the walker's colour channel is dotted style paths by design.
--
--   * Every live line rides `text_bind` — the scene pre-formats each string
--     (Rust owns the formatting, as everywhere).
--   * `loading` / `loaded` visible_binds gate the two states; `play_state` +
--     `play_state_color` (a color_bind holding a dotted path) drive the
--     PLAYING/PAUSED word; `ledger_status_color` recolours the Σ line.
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
local Text = tag("text")

-- The pre-sim loading banner (visible while `loading` is on).
local function loading_block()
  return Cell {
    anchor = "top_left",
    offset = { 40, 60 },
    width = 900,
    gap = 0,
    visible_bind = "loading",
    children = {
      Text {
        text = "$chem_generating_planet",
        size = 44,
        text_size = 30,
        color = "chemistry.title.color",
      },
      Text {
        text = "$chem_freq_96_92_162_cells_bulk_accretion_seed",
        size = 20,
        text_size = 16,
        color = "chemistry.dim.color",
      },
    },
  }
end

-- The live readout block (top-left, visible once a snapshot exists). Row
-- heights reproduce the legacy line positions: title 24→58, stats 58→84,
-- interior 84→108, state 108→132, crust 132→…
local function readout_block()
  return Cell {
    anchor = "top_left",
    offset = { 24, 24 },
    width = 980,
    gap = 0,
    visible_bind = "loaded",
    children = {
      Text {
        text = "$chem_flicker_chemistry_sim_m2_layer_stack",
        size = 34,
        text_size = 24,
        color = "chemistry.title.color",
      },
      Text { text_bind = "stats", size = 26, text_size = 17, color = "chemistry.text.color" },
      Text {
        text_bind = "interior",
        size = 24,
        text_size = 15,
        color = "chemistry.interior.color",
      },
      Row {
        size = 24,
        children = {
          -- The state word occupies the fixed 86px column (24→110) the hint
          -- line always started after.
          Text {
            text_bind = "play_state",
            size = 86,
            text_size = 14,
            color_bind = "play_state_color",
          },
          Text { text_bind = "hints", grow = 1, text_size = 14, color = "chemistry.dim.color" },
        },
      },
      Text { text_bind = "crust", size = 24, text_size = 15, color = "chemistry.crust.color" },
    },
  }
end

-- The conservation ledger (top-right): title · Σ status line · six mass rows,
-- every string pre-formatted by the scene.
local function ledger_panel()
  local lg = UI.chemistry.ledger
  local rows = {
    Text {
      text = "$chem_conservation_ledger",
      size = 22,
      text_size = 14,
      color = "chemistry.title.color",
    },
    Text {
      text_bind = "ledger_status",
      size = 24,
      text_size = 13,
      color_bind = "ledger_status_color",
    },
  }
  for i = 1, 6 do
    rows[#rows + 1] = Text {
      text_bind = "ledger_" .. i,
      size = lg.row_h,
      text_size = 13,
      color = "chemistry.text.color",
    }
  end
  return Cell {
    anchor = "top_right",
    offset = { -16, 20 },
    width = lg.w,
    pad = lg.pad,
    gap = 0,
    style = "chemistry.ledger.panel",
    visible_bind = "loaded",
    children = rows,
  }
end

function M.tree()
  if not UI or not UI.chemistry then
    -- The degenerate no-UI root keeps the declared binding (S9).
    return Screen { id = "chemistry", on_menu = "pause_open" }
  end
  return Screen {
    id = "chemistry",
    on_menu = "pause_open",
    children = {
      loading_block(),
      readout_block(),
      ledger_panel(),
    },
  }
end

return M
