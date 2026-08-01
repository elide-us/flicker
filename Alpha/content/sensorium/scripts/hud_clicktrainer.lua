-- flicker-clicktrainer HUD — DECLARATIVE component tree (the S10 convergence of
-- the reference vector-UI-over-2D-sprite blend). The carved-stone stats panel
-- (top-left) is DECLARED here: `M.tree()` returns component instances and the
-- Rust walker (flicker-widgets `run_ui`) owns layout, draw, and hit-test. No
-- pixel math and no per-frame code — the old `update`/`draw` (panel layout,
-- `Widgets.button_update`, command emission) died with the immediate path.
--
-- The click-routing contract is unchanged, now structural:
--   * `hud_hit` — the walker reports the pointer over the panel, and the scene's
--     WalkerHandler layer swallows the click before the gameplay base can score
--     it (panel click ≠ game miss).
--   * `reset`  — the RESET button's `action`, read via `results.is_on("reset")`.
--   * `on_menu = "pause_open"` — the screen's input DECLARATION (S9): Menu
--     (Esc / pad Start) fires `pause_open`; the scene maps it onto its pause
--     push. The scene root's hardcoded Menu arm is gone.
--
-- Layout / labels / colours all come from `UI.clicktrainer` (ui_elements.json,
-- palette single-sourced in `theme.tokens`) — one source of truth, no recompile.

local M = {}

-- Ergonomic constructors: each tags a node table with its component kind.
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
local Button = tag("button")

-- One stat row: label (left, dim) · value (right, bright; accuracy in the
-- accent colour). Values ride `text_bind` — the scene publishes pre-formatted
-- strings (`hits`, `accuracy`, `react_last`, …) in its Model each frame.
local function stat_row(ct, row)
  local value_color = (row.id == "accuracy") and "clicktrainer.accent" or "clicktrainer.value_color"
  return Row {
    size = ct.row_h,
    children = {
      Text {
        text = row.label,
        grow = 1,
        text_size = ct.label_size,
        color = "clicktrainer.label_color",
      },
      Text {
        text_bind = row.id,
        grow = 1,
        align = "right",
        text_size = ct.value_size,
        color = value_color,
        font = "label",
      },
    },
  }
end

function M.tree()
  if not UI or not UI.clicktrainer then
    -- The degenerate no-UI root keeps the declared binding (S9).
    return Screen { id = "clicktrainer", on_menu = "pause_open" }
  end
  local ct = UI.clicktrainer

  local col = {}
  col[#col + 1] = Text {
    text = ct.title.text,
    size = ct.title.size + 4,
    text_size = ct.title.size,
    color = "clicktrainer.title.color",
    font = "display",
  }
  col[#col + 1] = Text {
    text = ct.subtitle.text,
    size = ct.subtitle.size + 10,
    text_size = ct.subtitle.size,
    color = "clicktrainer.subtitle.color",
  }
  -- Bronze divider + its breathing room (the old `c += 1 + 12`).
  col[#col + 1] = Cell { size = 1, style = "clicktrainer.divider" }
  col[#col + 1] = Stack { size = 12 }
  for _, row in ipairs(ct.rows) do
    col[#col + 1] = stat_row(ct, row)
  end
  col[#col + 1] = Stack { size = ct.reset.gap_top }
  -- RESET — the shared modal secondary-button kit (hover-lit via ui/button.lua).
  col[#col + 1] = Button {
    id = "reset",
    action = "reset",
    label = ct.reset.label,
    size = ct.reset.h,
    label_size = 14,
    style = "modal.buttons.variants.secondary",
  }
  col[#col + 1] = Stack { size = ct.hint.gap_top }
  col[#col + 1] = Text {
    text = ct.hint.text,
    size = ct.hint.size,
    text_size = ct.hint.size,
    color = "clicktrainer.hint.color",
    font = "label",
  }

  return Screen {
    id = "clicktrainer",
    on_menu = "pause_open",
    children = {
      Cell {
        id = "stats_panel",
        anchor = "top_left",
        offset = { ct.margin, ct.margin },
        width = ct.panel.w,
        pad = ct.panel.pad,
        gap = 0,
        style = "clicktrainer.panel",
        children = col,
      },
    },
  }
end

return M
