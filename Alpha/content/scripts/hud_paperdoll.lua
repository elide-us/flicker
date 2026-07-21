-- In-scene HUD for flicker-paperdoll — DECLARATIVE component tree.
--
-- The whole HUD is now DECLARED, not drawn: `M.tree()` returns a tree of
-- component instances and the Rust walker (flicker-widgets `run_ui`) owns layout,
-- draw, and hit-test. This script holds no pixel math and no per-frame code — the
-- old `update`/`draw` (inventory hit-testing, `fit_rects`, command emission) all
-- moved into Rust templates. See MCP: "UI re-architecture RESOLVED".
--
-- The two-way contract is unchanged and lives in the node data:
--   * `bind`          — a Model key read for the current value + written back on edit
--                       (checkbox `show_mesh`, slider `fit_u`, cell `slot_<key>`).
--   * `action`        — an event name emitted on click (`attack`, `fit_record`).
--   * `visible_bind`  — a Model key gating a subtree (`fit_active`, `animate`).
--   * `enabled_bind`  — a Model key gating a control (a slot with no piece loaded).
--   * `style`         — a dotted path into `ui_elements.json` (colours/sizes; the
--                       palette stays single-sourced in `theme.tokens`).
-- Layout / labels / ranges still come from `UI.paperdoll` — one source of truth,
-- so the tree and the engine's keyboard nav (which reads the same `fit.rows`)
-- cannot drift.

local M = {}

-- Ergonomic constructors: each tags a node table with its component kind, so a
-- screen reads as composition — `Button{...}` placed inside `Column{ children }`.
-- (These will graduate to a shared `ui` module once a second screen needs them.)
local function tag(kind)
  return function(t)
    t.component = kind
    return t
  end
end
local Page = tag("page")
local Column = tag("column")
local Row = tag("row")
local Stack = tag("stack")
local Text = tag("text")
local Button = tag("button")
local Checkbox = tag("checkbox")
local Slider = tag("slider")
local Cell = tag("cell")

-- Per-fit-group display formatting (mirrors the old `fit_rects` fmt choice): the
-- rotation rows read out signed degrees, offsets signed cm, scales a bare factor.
local function row_format(group)
  if group == "rotate" then
    return 0, true, "\u{00B0}"
  elseif group == "offset" then
    return 1, true, ""
  else
    return 2, false, ""
  end
end

-- ── The view toggles + Attack (top-left column) ──
local function view_cluster()
  local v = UI.paperdoll.view
  local kids = {}
  for _, item in ipairs(v.items) do
    kids[#kids + 1] = Checkbox {
      id = item.key,
      bind = item.key,
      label = item.label,
      size = v.row_h,
      box = v.box,
      label_x = v.label_x,
      label_size = v.label_size,
      style = "paperdoll.view.checkbox",
    }
  end
  -- Attack is only meaningful while animating; the spacer before it hides too, so
  -- the column closes up cleanly in Pose mode.
  kids[#kids + 1] = Stack { size = v.attack.gap_y, visible_bind = "animate" }
  kids[#kids + 1] = Button {
    id = "attack",
    action = "attack",
    label = v.attack.label,
    size = v.attack.h,
    label_size = v.attack.style.label_size,
    visible_bind = "animate",
    style = "paperdoll.view.attack.style",
  }
  return Column {
    anchor = "top_left",
    offset = { v.margin, v.top },
    width = v.attack.w,
    gap = 0,
    children = kids,
  }
end

-- ── The fit gadget (right-hand panel, only while a piece is selected) ──
local function fit_cluster()
  local f = UI.paperdoll.fit
  local kids = {}

  -- Header names the piece being fitted (bone read-out omitted in this first pass).
  kids[#kids + 1] = Text {
    text_bind = "fit_name",
    prefix = f.header .. " \u{00B7} ",
    size = f.header_size + 6,
    text_size = f.header_size,
    color = "paperdoll.fit.header_color",
  }

  -- One slider per fit row, ranges + formatting from the row's group block — the
  -- SAME `fit.rows` list the engine walks for ←/→ nudges and ↑/↓ focus.
  for _, row in ipairs(f.rows) do
    local g = f[row.group]
    local decimals, plus, suffix = row_format(row.group)
    kids[#kids + 1] = Slider {
      id = row.key,
      bind = row.key,
      label = row.label,
      size = f.row_h,
      slider_h = f.slider_h,
      label_w = f.label_w,
      value_w = f.value_w,
      min = g.min,
      max = g.max,
      decimals = decimals,
      plus = plus,
      suffix = suffix,
      focus_group = "fit_focus",
      style = "paperdoll.fit.slider",
    }
  end

  kids[#kids + 1] = Button {
    id = "fit_record",
    action = "fit_record",
    label = f.record.label,
    size = f.record.h,
    label_size = 13,
    style = "paperdoll.fit.button",
  }
  -- Record confirmation (green ok; the red failure tint is a later refinement).
  kids[#kids + 1] = Text {
    text_bind = "fit_status",
    size = f.status.size + f.status.gap_y * 2,
    text_size = f.status.size,
    color = "paperdoll.fit.status.ok",
  }

  return Column {
    id = "fit",
    anchor = "top_right",
    offset = { -f.margin, f.margin },
    width = f.w,
    pad = f.pad,
    gap = 0,
    visible_bind = "fit_active",
    style = "paperdoll.fit",
    children = kids,
  }
end

-- ── The inventory bar (bottom-centre) ──
local function inventory_cluster()
  local inv = UI.paperdoll.inventory
  local cells = {}
  for _, slot in ipairs(inv.slots) do
    cells[#cells + 1] = Cell {
      id = slot.key,
      bind = "slot_" .. slot.key,
      enabled_bind = "slot_" .. slot.key .. "_loaded",
      label = slot.label,
      size = inv.slot,
      height = inv.slot,
      style = "paperdoll.inventory.cell",
      style_off = "paperdoll.inventory.empty",
    }
  end
  local bar = Row {
    style = "paperdoll.inventory",
    pad = inv.pad,
    gap = inv.gap,
    children = cells,
  }
  return Column {
    anchor = "bottom",
    offset = { 0, -inv.margin_b },
    gap = 4,
    children = {
      Text {
        text = inv.header,
        size = inv.header_size + 2,
        text_size = inv.header_size,
        color = "paperdoll.inventory.header_color",
        font = "label",
      },
      bar,
    },
  }
end

function M.tree()
  if not UI or not UI.paperdoll then
    return Page { id = "paperdoll" }
  end
  local s = UI.paperdoll.stats
  return Page {
    id = "paperdoll",
    children = {
      view_cluster(),
      fit_cluster(),
      inventory_cluster(),
      -- The stat readout (was the POC's `draw_text` line).
      Text {
        text_bind = "stats",
        anchor = "top_left",
        offset = { s.margin, s.top },
        size = s.size,
        color = "paperdoll.stats.color",
      },
    },
  }
end

return M
