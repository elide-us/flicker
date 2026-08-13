-- In-scene HUD for flicker-pocclusters ("Cluster Editor") — DECLARATIVE component tree.
--
-- Two panels, built to the design mock (DesignSync project a39150df,
-- "Cluster Editor HUD.dc.html"): a LEFT `main` panel (header + mode pill, a Readout
-- box, the 6 debug-overlay toggles in a 2-column grid, a Move-speed slider) and a
-- RIGHT `inspector` panel shown only when a face is picked (header + subtitle + a
-- 7-column table over the selected voxel's 8 corners: local vector + world position).
--
-- `M.tree()` returns a tree of component instances; the Rust walker (flicker-widgets
-- `run_ui`) owns layout, draw, and hit-test. No pixel math / no per-frame code here.
--
-- Two-way contract lives in the node data:
--   * `bind`         — a Model key read for the value + written back (checkbox `wireframe`,
--                      slider `move_speed`).
--   * `text_bind`    — a Model key whose pre-formatted string a text node shows (Rust owns
--                      the formatting in `hud_model` — no printf here).
--   * `visible_bind` — a Model key gating a subtree (`walk` row, `has_pick`/`no_pick`,
--                      the whole inspector panel).
--   * `style`/`color`— dotted paths into `ui_theme.json` (palette single-sourced
--                      in its theme.tokens — the one global UI-element definition).
-- Layout / labels all come from `UI.pocclusters` — one source of truth, so the tree and
-- the engine cannot drift. Change a toggle or a column here (or in the JSON) and the
-- whole panel follows; no recompile.

local M = {}

-- Ergonomic constructors: each tags a node table with its component kind, so a screen
-- reads as composition — `Checkbox{...}` inside `Cell { children }`.
local function tag(kind)
  return function(t)
    t.component = kind
    return t
  end
end
local Screen = tag("screen")
local Stack = tag("stack")
local Cell = tag("cell")
local Row = tag("row")
local Text = tag("text")
local Checkbox = tag("checkbox")
local Slider = tag("slider")

-- A group's small-caps section label (a column child; `size` = its row height).
local function section(P, name)
  return Text {
    text = name,
    size = P.layout.section_h,
    text_size = P.layout.section_size,
    font = "label",
    color = "pocclusters.col.label",
  }
end

-- ── Corner runes: 4 Elder Futhark glyphs at a panel's corners (DesignSync
-- a39150df). Top two glow — rune-light glyph over a feathered halo Panel that
-- reuses the panel SDF's soft `feather`; bottom two are static bronze. Returns a
-- flat list of overlay nodes to sit in a panel Stack beside its content column. ──
local function corner_runes(P, glyphs)
  local u = P.runes
  local i = u.inset
  local corners = {
    { "top_left", i, i },
    { "top_right", -i, i },
    { "bottom_left", i, -i },
    { "bottom_right", -i, -i },
  }
  local out = {}
  for k = 1, 4 do
    local anchor, ox, oy = corners[k][1], corners[k][2], corners[k][3]
    local top = k <= 2
    if top then
      out[#out + 1] = Cell {
        anchor = anchor,
        offset = { ox, oy },
        width = u.cell,
        height = u.cell,
        style = "pocclusters.rune_halo",
      }
    end
    out[#out + 1] = Text {
      anchor = anchor,
      offset = { ox, oy },
      width = u.cell,
      height = u.cell,
      text = glyphs[k],
      font = "rune",
      align = "center",
      text_size = top and u.size or u.size_b,
      color = top and "pocclusters.col.rune" or "pocclusters.col.bronze_deep",
    }
  end
  return out
end

-- ── Header: panel icon + title + a Fly/Walk mode pill ──
local function header(P)
  local h = P.header
  return Row {
    size = h.row_h,
    children = {
      -- Lead/trail spacers clear the top corner runes; a bronze ✦ marks the panel.
      Cell { size = h.lead },
      Text { size = h.icon_w, height = h.row_h, text = h.icon, text_size = h.icon_size, font = "display", color = "pocclusters.col.bronze", align = "center" },
      Text {
        grow = 1,
        height = h.row_h,
        text = h.title,
        text_size = h.title_size,
        font = "display",
        color = "pocclusters.col.title",
      },
      -- The pill is a bordered tag; its label is centre-anchored inside the stack.
      Stack {
        size = h.pill_w,
        height = h.pill_h,
        style = "pocclusters.pill",
        children = {
          Text {
            anchor = "center",
            text_bind = "mode_tag",
            text_size = h.pill_size,
            font = "label",
            color = "pocclusters.col.accent",
            align = "center",
          },
        },
      },
      Cell { size = h.trail },
    },
  }
end

-- ── Readout box: title line, Cam/Grid/Move/Diag rows, Pick, a walk-only row ──
local function readout(P)
  local r = P.readout
  local kids = {
    Text {
      size = r.title_h,
      text_bind = "title_line",
      text_size = r.title_size,
      font = "display",
      color = "pocclusters.col.title",
    },
  }
  -- Label → value rows (label fixed-width, value fills).
  for _, row in ipairs(r.rows) do
    kids[#kids + 1] = Row {
      size = r.row_h,
      children = {
        Text { size = r.label_w, height = r.row_h, text = row.label, text_size = r.label_size, font = "label", color = "pocclusters.col.label" },
        Text { grow = 1, height = r.row_h, text_bind = row.bind, text_size = r.value_size, font = "label", tracking = 0, color = "pocclusters.col.value" },
      },
    }
  end
  -- Divider rule, then the pick row (blue value when a face is selected, dim "none" otherwise).
  kids[#kids + 1] = Cell { size = 1, style = "pocclusters.divider" }
  kids[#kids + 1] = Row {
    size = r.row_h,
    children = {
      Text { size = r.label_w, height = r.row_h, text = "$pc_pick", text_size = r.label_size, font = "label", color = "pocclusters.col.label" },
      Text { grow = 1, height = r.row_h, text_bind = "pick_val", visible_bind = "has_pick", text_size = r.value_size, font = "label", tracking = 0, color = "pocclusters.col.accent" },
      Text { grow = 1, height = r.row_h, text = "$pc_none", visible_bind = "no_pick", text_size = r.value_size, font = "label", color = "pocclusters.col.dim" },
    },
  }
  -- Walk readout — the whole row is hidden outside surface-walk mode.
  kids[#kids + 1] = Row {
    size = r.row_h,
    visible_bind = "walk",
    children = {
      Text { size = r.label_w, height = r.row_h, text = "$pc_walk", text_size = r.label_size, font = "label", color = "pocclusters.col.amber" },
      Text { grow = 1, height = r.row_h, text_bind = "walk_val", text_size = r.value_size, font = "label", tracking = 0, color = "pocclusters.col.amber" },
    },
  }
  return Cell {
    gap = 0,
    children = {
      section(P, r.section),
      Cell { style = "pocclusters.box", pad = r.pad, gap = r.gap, children = kids },
      -- "Press [Esc] for the menu" — Esc in a bordered keycap (mock flourish).
      Row {
        size = r.esc_h,
        gap = 6,
        children = {
          Text { size = 42, height = r.esc_h, text = r.esc_pre, text_size = r.esc_size, font = "body", color = "pocclusters.col.dim" },
          Stack { size = r.key_w, height = r.key_h, style = "pocclusters.keycap", children = {
            Text { anchor = "center", text = r.esc_key, text_size = r.key_size, font = "label", color = "pocclusters.col.value", align = "center" },
          } },
          Text { grow = 1, height = r.esc_h, text = r.esc_post, text_size = r.esc_size, font = "body", color = "pocclusters.col.dim" },
        },
      },
    },
  }
end

-- ── Debug-overlay toggles: 6 checkboxes in a 2-column grid ──
local function toggles(P)
  local t = P.toggles
  local function cb(item)
    return Checkbox {
      id = item.key,
      bind = item.key,
      label = item.label,
      size = t.row_h,
      box = t.box,
      label_x = t.label_x,
      label_size = t.label_size,
      style = "pocclusters.checkbox",
    }
  end
  local left, right = {}, {}
  for i, item in ipairs(t.items) do
    local col = (i <= 3) and left or right
    col[#col + 1] = cb(item)
  end
  return Cell {
    gap = 0,
    children = {
      section(P, t.section),
      Row {
        size = t.row_h * 3 + t.gap * 2,
        gap = 18,
        children = {
          Cell { grow = 1, gap = t.gap, children = left },
          Cell { grow = 1, gap = t.gap, children = right },
        },
      },
    },
  }
end

-- ── Controls: Move speed slider with a value readout + a min·unit·max scale ──
local function controls(P)
  local c = P.controls
  return Cell {
    gap = 0,
    children = {
      section(P, c.section),
      Row {
        size = c.row_h,
        children = {
          Text { grow = 1, height = c.row_h, text = c.label, text_size = c.label_size, font = "label", color = "pocclusters.col.value" },
          Text { size = 90, height = c.row_h, text_bind = "speed_val", text_size = c.value_size, font = "label", tracking = 0, color = "pocclusters.col.accent", align = "right" },
        },
      },
      Slider {
        size = c.track_h,
        bind = "move_speed",
        min = c.min,
        max = c.max,
        slider_h = c.slider_h,
        style = "pocclusters.slider",
      },
      Row {
        size = c.scale_h,
        children = {
          Text { grow = 1, height = c.scale_h, text = c.min_label, text_size = c.scale_size, font = "label", color = "pocclusters.col.label" },
          Text { grow = 1, height = c.scale_h, text = c.unit, text_size = c.scale_size, font = "label", color = "pocclusters.col.dim", align = "center" },
          Text { grow = 1, height = c.scale_h, text = c.max_label, text_size = c.scale_size, font = "label", color = "pocclusters.col.label", align = "right" },
        },
      },
    },
  }
end

-- ── Main (left) panel — content column overlaid with corner runes in a Stack ──
local function main_panel(P)
  local L = P.layout
  local content = Cell {
    width = L.panel_w,
    pad = L.pad,
    gap = L.gap,
    children = { header(P), readout(P), toggles(P), controls(P) },
  }
  local kids = { content }
  for _, rune in ipairs(corner_runes(P, P.runes.main)) do
    kids[#kids + 1] = rune
  end
  return Stack {
    anchor = "top_left",
    offset = { L.margin, L.margin },
    style = "pocclusters.panel",
    children = kids,
  }
end

-- ── Inspector (right) panel — the selected voxel's 8-corner table ──
local function inspector(P)
  local n = P.inspector
  -- Header row of the table.
  local head = {}
  for i, label in ipairs(n.headers) do
    head[#head + 1] = Text {
      size = (i == 1) and n.name_w or n.coord_w,
      height = n.head_h,
      text = label,
      text_size = n.head_size,
      font = "label",
      color = (i >= 5) and "pocclusters.col.bronze_deep" or "pocclusters.col.label",
      align = (i == 1) and "left" or "right",
    }
  end
  local rows = { Row { size = n.head_h, children = head } }
  -- One row per cube corner: name · local X/Y/Z · world wX/wY/wZ (bound by name).
  for i = 0, 7 do
    local cells = {
      Text { size = n.name_w, height = n.row_h, text_bind = "insp_c" .. i .. "_name", text_size = n.cell_size, font = "label", tracking = 0, color = "pocclusters.col.bronze", align = "left" },
    }
    for _, ax in ipairs({ "lx", "ly", "lz", "wx", "wy", "wz" }) do
      cells[#cells + 1] = Text {
        size = n.coord_w,
        height = n.row_h,
        text_bind = "insp_c" .. i .. "_" .. ax,
        text_size = n.cell_size,
        font = "label",
        -- Numeric column: untracked so the fixed 44px cells stay aligned (the caps
        -- headers/labels keep the Label role's default letter-spacing).
        tracking = 0,
        color = (ax:sub(1, 1) == "w") and "pocclusters.col.dim" or "pocclusters.col.value",
        align = "right",
      }
    end
    rows[#rows + 1] = Row { size = n.row_h, children = cells }
  end
  local content = Cell {
    width = n.panel_w,
    pad = n.pad,
    gap = n.gap,
    children = {
      Row { size = n.title_h, children = {
        Cell { size = n.lead },
        Text { size = n.icon_w, height = n.title_h, text = n.icon, text_size = n.icon_size, font = "display", color = "pocclusters.col.bronze", align = "center" },
        Text { grow = 1, height = n.title_h, text_bind = "insp_title", text_size = n.title_size, font = "display", color = "pocclusters.col.title" },
      } },
      Text { size = n.sub_h, text_bind = "insp_sub", text_size = n.sub_size, font = "label", color = "pocclusters.col.accent" },
      Cell { style = "pocclusters.box", pad = n.pad_box, gap = 0, children = rows },
      -- Legend: ◆ local corner  |  wX/Y/Z world space (colour-coded like the columns).
      Row { size = n.legend_h, children = {
        Text { grow = 1, height = n.legend_h, text = n.legend_local, text_size = n.legend_size, font = "label", color = "pocclusters.col.bronze", align = "right" },
        Cell { size = n.legend_gap },
        Text { grow = 1, height = n.legend_h, text = n.legend_world, text_size = n.legend_size, font = "label", color = "pocclusters.col.bronze_deep", align = "left" },
      } },
    },
  }
  local kids = { content }
  for _, rune in ipairs(corner_runes(P, P.runes.inspector)) do
    kids[#kids + 1] = rune
  end
  return Stack {
    anchor = "top_right",
    offset = { -n.margin, n.margin },
    style = "pocclusters.panel",
    visible_bind = "has_pick",
    children = kids,
  }
end

-- ── Celestial Cycle (bottom-right) — the recovered day/night controls, restyled ──
-- Two groups: "Sky · the paired hands" (Sun + Moon in a box well) and
-- "World · independent" (Year/Speed/Fog/Latitude/Epoch), then View toggles. Each
-- slider is a two-way `bind` in the celestial state's math units; the value readout
-- rides a `text_bind` (Rust owns the HH:MM / phase / month / etc. formatting).
local function celestial_panel(P)
  local c = P.celestial
  if not c then
    return nil
  end
  local tg = P.toggles
  -- label + right-aligned value readout, then the track.
  local function cc_row(spec)
    return Cell {
      gap = c.ctrl_gap,
      children = {
        Row {
          size = c.row_h,
          children = {
            Text { grow = 1, height = c.row_h, text = spec.label, text_size = c.label_size, font = "label", color = "pocclusters.col.label" },
            Text { size = c.value_w, height = c.row_h, text_bind = spec.val_bind, text_size = c.value_size, font = "label", tracking = 0, color = "pocclusters.col.accent", align = "right" },
          },
        },
        Slider { size = c.track_h, bind = spec.bind, min = spec.min, max = spec.max, slider_h = c.slider_h, style = "pocclusters.slider" },
      },
    }
  end
  local function cc_view(key, label)
    return Checkbox { id = key, bind = key, label = label, size = tg.row_h, box = tg.box, label_x = tg.label_x, label_size = tg.label_size, style = "pocclusters.checkbox" }
  end
  -- Sky group — Sun + Moon paired inside a "box" well.
  local sky = Cell {
    gap = 0,
    children = {
      section(P, c.sky_section),
      Cell { style = "pocclusters.box", pad = c.well_pad, gap = c.group_gap, children = { cc_row(c.sun), cc_row(c.moon) } },
    },
  }
  local world = Cell {
    gap = c.group_gap,
    children = {
      section(P, c.world_section),
      cc_row(c.year), cc_row(c.speed), cc_row(c.fog), cc_row(c.latitude), cc_row(c.epoch),
    },
  }
  local views = Cell {
    gap = 0,
    children = {
      section(P, c.views_section),
      Cell { gap = tg.gap, children = { cc_view("constellations", c.view_constellations), cc_view("planets", c.view_planets), cc_view("celestial_paths", c.view_paths) } },
    },
  }
  local content = Cell {
    width = c.panel_w,
    pad = c.pad,
    gap = c.gap,
    children = {
      Row { size = c.title_h, children = {
        Cell { size = c.lead },
        Text { size = c.icon_w, height = c.title_h, text = c.icon, text_size = c.icon_size, font = "display", color = "pocclusters.col.bronze", align = "center" },
        Text { grow = 1, height = c.title_h, text = c.title, text_size = c.title_size, font = "display", color = "pocclusters.col.title" },
      } },
      Text { size = c.hint_h, text = c.hint, text_size = c.hint_size, font = "body", italic = true, color = "pocclusters.col.dim" },
      sky,
      world,
      views,
    },
  }
  local kids = { content }
  for _, rune in ipairs(corner_runes(P, c.runes)) do
    kids[#kids + 1] = rune
  end
  return Stack {
    anchor = "bottom_right",
    offset = { -c.margin, -c.margin },
    style = "pocclusters.panel",
    children = kids,
  }
end

function M.tree()
  if not UI or not UI.pocclusters then
    -- The degenerate no-UI root keeps the declared binding (S9): Esc→pause
    -- must not depend on the layout constants loading.
    return Screen { id = "pocclusters", on_menu = "pause_open" }
  end
  local P = UI.pocclusters
  local children = { main_panel(P), inspector(P) }
  local cc = celestial_panel(P)
  if cc then
    children[#children + 1] = cc
  end
  -- The screen's input DECLARATION (S9): the Menu signal (Esc / pad Start)
  -- fires `pause_open`; the scene maps the fired name onto its pause push.
  -- While chat owns the keyboard the TextEntry capture layer still starves
  -- this structurally — the declaration changes WHO consumes Menu, not when.
  return Screen { id = "pocclusters", on_menu = "pause_open", children = children }
end

return M
