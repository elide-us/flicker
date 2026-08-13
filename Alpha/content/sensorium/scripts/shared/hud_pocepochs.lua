-- flicker-pocepochs HUD — DECLARATIVE component tree: the sim's text readout
-- (title/stats/state/hints, top-left) plus the LIFE-SUPPORTING CONDITIONS panel
-- (bottom-right — five `ui.gauge` condition rows + the verdict light), replacing
-- both the scene's immediate `draw_text` block and the retired
-- `habitability_hud.lua` (which drew the panel through `Widgets.gauge_draw`).
-- `M.tree()` returns component instances; the Rust walker (flicker-widgets
-- `run_ui`) owns layout, draw, and hit-test.
--
-- Still scene-drawn (FLAGGED, S10): the surface-view legend and the
-- element-distribution panel — their per-row swatch colours are DATA
-- (element_rgb), and the walker's colour channel is dotted style paths by design.
--
--   * Per-axis values ride the same flat Model the observer always published
--     (`a<i>_v` on the gauge's `bind`, `a<i>_name`/`a<i>_lolab`/`a<i>_hilab` on
--     text_binds) plus the S10 derived keys the scene now pre-formats:
--     `a<i>_status` + `a<i>_status_color`, `a<i>_name_color`, `verdict` +
--     `verdict_color`, `observed`, `air`, `life`/`no_life`.
--   * Each axis's band (`lo`/`hi`) is STATIC observer data, published once at
--     enter as the `HAB` data global (`{ { lo, hi }, … }`) and baked into the
--     gauge nodes as props.
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
local Gauge = tag("gauge")

-- The top-left readout. Row heights reproduce the legacy line positions:
-- title 24→60, stats 60→88, then the state word + hints line.
local function readout_block()
  return Cell {
    anchor = "top_left",
    offset = { 24, 24 },
    width = 980,
    gap = 0,
    children = {
      Text {
        text = "$poc_flicker_planet_simulation",
        size = 38,
        text_size = 28,
        color = "pocepochs.title.color",
      },
      Text { text_bind = "stats", size = 28, text_size = 17, color = "pocepochs.text.color" },
      Row {
        size = 20,
        children = {
          -- The state word occupies the fixed 72px column (24→96) the hint
          -- line always started after.
          Text {
            text_bind = "play_state",
            size = 72,
            text_size = 14,
            color_bind = "play_state_color",
          },
          Text { text_bind = "hints", grow = 1, text_size = 14, color = "pocepochs.dim.color" },
        },
      },
    },
  }
end

-- One condition-axis row: name + status (17px) · the gauge bar (12px) ·
-- a 2px gap · the low/high end captions (11px) — 42px total (hab.row_h).
local function axis_row(hab, i, band)
  local n = tostring(i)
  return Cell {
    size = hab.row_h,
    gap = 0,
    children = {
      Row {
        size = 17,
        children = {
          Text {
            text_bind = "a" .. n .. "_name",
            grow = 1,
            text_size = 13,
            color_bind = "a" .. n .. "_name_color",
          },
          Text {
            text_bind = "a" .. n .. "_status",
            grow = 1,
            align = "right",
            text_size = 11,
            color_bind = "a" .. n .. "_status_color",
          },
        },
      },
      Gauge {
        size = hab.bar_h,
        bind = "a" .. n .. "_v",
        lo = band.lo,
        hi = band.hi,
        style = "pocepochs.hab.gauge",
      },
      Stack { size = 2 },
      Row {
        size = 11,
        children = {
          Text {
            text_bind = "a" .. n .. "_lolab",
            grow = 1,
            text_size = 10,
            color = "pocepochs.hab.caption.color",
          },
          Text {
            text_bind = "a" .. n .. "_hilab",
            grow = 1,
            align = "right",
            text_size = 10,
            color = "pocepochs.hab.caption.color",
          },
        },
      },
    },
  }
end

-- The footer: verdict light + verdict + observed count (one 14px row), then the
-- atmosphere-kind line — 50px total (hab.foot_h).
local function verdict_footer(hab)
  return Cell {
    size = hab.foot_h,
    gap = 0,
    children = {
      Stack { size = 6 },
      Row {
        size = 14,
        children = {
          Cell { size = 14, style = "pocepochs.hab.light_on", visible_bind = "life" },
          Cell { size = 14, style = "pocepochs.hab.light_off", visible_bind = "no_life" },
          Stack { size = 10 },
          Text { text_bind = "verdict", grow = 1, text_size = 14, color_bind = "verdict_color" },
          Text {
            text_bind = "observed",
            grow = 1,
            align = "right",
            text_size = 11,
            color = "pocepochs.hab.observed.color",
          },
        },
      },
      Stack { size = 6 },
      Text { text_bind = "air", size = 12, text_size = 12, color = "pocepochs.hab.air.color" },
    },
  }
end

-- The LIFE-SUPPORTING CONDITIONS panel (bottom-right), one axis row per HAB
-- band entry.
local function hab_panel()
  local hab = UI.pocepochs.hab
  local col = {
    Text {
      text = "$poc_life_supporting_conditions",
      size = hab.head_h,
      text_size = 15,
      color = "pocepochs.hab.header.color",
    },
  }
  for i, band in ipairs(HAB or {}) do
    col[#col + 1] = axis_row(hab, i, band)
  end
  col[#col + 1] = verdict_footer(hab)
  return Cell {
    anchor = "bottom_right",
    offset = { -32, -20 },
    width = hab.w,
    pad = hab.pad,
    gap = 0,
    style = "pocepochs.hab.panel",
    children = col,
  }
end

function M.tree()
  if not UI or not UI.pocepochs then
    -- The degenerate no-UI root keeps the declared binding (S9).
    return Screen { id = "pocepochs", on_menu = "pause_open" }
  end
  return Screen {
    id = "pocepochs",
    on_menu = "pause_open",
    children = {
      readout_block(),
      hab_panel(),
    },
  }
end

return M
