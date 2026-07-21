-- In-scene HUD for flicker-pocclusters — DECLARATIVE component tree.
--
-- The whole HUD is DECLARED, not drawn: `M.tree()` returns a tree of component
-- instances and the Rust walker (flicker-widgets `run_ui`) owns layout, draw, and
-- hit-test. This script holds no pixel math and no per-frame code — the walker
-- redraws the cached tree each frame with fresh Model bindings. See MCP:
-- "UI re-architecture RESOLVED".
--
-- The two-way contract lives in the node data:
--   * `bind`         — a Model key read for the current value + written back on
--                      edit (checkbox `wireframe`, slider `move_speed`).
--   * `text_bind`    — a Model key whose pre-formatted string a text node shows
--                      (the stat lines; Rust owns the formatting — no printf here).
--   * `visible_bind` — a Model key gating a subtree (`walk` gates the walk stat).
--   * `style`        — a dotted path into `ui_elements.json` (colours/sizes; the
--                      palette stays single-sourced in `theme.tokens`).
-- Layout / labels / ranges all come from `UI.pocclusters` — one source of truth, so
-- the tree and the engine cannot drift apart. Change a toggle here (or in the JSON)
-- and the whole panel follows; no recompile.

local M = {}

-- Ergonomic constructors: each tags a node table with its component kind, so a
-- screen reads as composition — `Checkbox{...}` inside `Column{ children }`.
local function tag(kind)
  return function(t)
    t.component = kind
    return t
  end
end
local Page = tag("page")
local Column = tag("column")
local Text = tag("text")
local Checkbox = tag("checkbox")
local Slider = tag("slider")

-- ── Stat readout (top-left): one text node per pre-formatted Model line ──
local function stats_cluster()
  local s = UI.pocclusters.stats
  local function line(bind, color)
    return Text {
      text_bind = bind,
      size = s.row_h,
      text_size = s.size,
      color = color or "pocclusters.stats.color",
    }
  end
  return Column {
    anchor = "top_left",
    offset = { s.margin, s.top },
    gap = 0,
    children = {
      line("stat_title"),
      line("stat_pos"),
      line("stat_clusters"),
      line("stat_config"),
      line("stat_diag"),
      Text {
        text = "press Escape to quit",
        size = s.row_h,
        text_size = s.size,
        color = "pocclusters.stats.color",
      },
      line("stat_pick", "pocclusters.stats.pick_color"),
      -- Walk readout only while surface-walk is on (its Model line is empty otherwise).
      Text {
        text_bind = "stat_walk",
        size = s.row_h,
        text_size = s.size,
        color = "pocclusters.stats.walk_color",
        visible_bind = "walk",
      },
    },
  }
end

-- ── Debug-toggle checkboxes (below the stats) — data-driven from the item list ──
local function toggles_cluster()
  local t = UI.pocclusters.toggles
  local kids = {
    Text {
      text = t.header,
      size = t.header_size + 8,
      text_size = t.header_size,
      color = "pocclusters.toggles.header_color",
      font = "label",
    },
  }
  for _, item in ipairs(t.items) do
    kids[#kids + 1] = Checkbox {
      id = item.key,
      bind = item.key,
      label = item.label,
      size = t.row_h,
      box = t.box,
      label_x = t.label_x,
      label_size = t.label_size,
      style = "pocclusters.toggles.checkbox",
    }
  end
  return Column {
    anchor = "top_left",
    offset = { t.margin, t.top },
    gap = 0,
    children = kids,
  }
end

-- ── Control sliders (bottom-left): move speed + look sensitivity ──
local function controls_cluster()
  local c = UI.pocclusters.controls
  local kids = {
    Text {
      text = c.header,
      size = c.header_size + 8,
      text_size = c.header_size,
      color = "pocclusters.controls.header_color",
      font = "label",
    },
  }
  for _, row in ipairs(c.rows) do
    kids[#kids + 1] = Slider {
      id = row.key,
      bind = row.key,
      label = row.label,
      size = c.row_h,
      slider_h = c.slider_h,
      label_w = c.label_w,
      value_w = c.value_w,
      min = row.min,
      max = row.max,
      decimals = row.decimals,
      suffix = row.suffix,
      style = "pocclusters.controls.slider",
    }
  end
  return Column {
    anchor = "bottom_left",
    offset = { c.margin, -c.margin_b },
    width = c.w,
    gap = 0,
    children = kids,
  }
end

function M.tree()
  if not UI or not UI.pocclusters then
    return Page { id = "pocclusters" }
  end
  return Page {
    id = "pocclusters",
    children = {
      stats_cluster(),
      toggles_cluster(),
      controls_cluster(),
    },
  }
end

return M
