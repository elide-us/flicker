-- Prism front-end modal — the shared menu / pause ("Sanctum") / display-confirm
-- control, a DECLARATIVE component tree (Rust walker `run_ui` owns layout, draw,
-- hit-test). Replaces the legacy `modal.lua`. One script, three screens (MENU.screen);
-- they share the popup + button kit and differ only in DATA.
--
-- The MENU screen has TWO forms, chosen by `MENU.scene_select`:
--   * default (single-scene clients): the centred/left popup with one launch button
--     per scene + settings/quit — `MENU.items`.
--   * LAUNCHER (prism-alpha): a two-column layout — the same popup on the LEFT (just
--     settings/quit chrome) + a SCENE-SELECTION PANEL on the RIGHT listing `MENU.scenes`
--     (one data-driven row each: bronze preview frame · mode · name · desc · meta · a
--     LOAD button whose `action` is the scene id). The row LOAD button REUSES the shared
--     `button` template; the panel/rows compose from panel/row/column/text/sprite. No
--     new widget types — everything is an existing walker component.
--
-- Data sources (all single-sourced, no per-app JSON mutation):
--   * `UI.screens[MENU.screen]` + `UI.modal` + `UI.menu` — chrome CONFIG / styles.
--   * `MENU.items`  — the popup buttons the engine publishes.
--   * `MENU.scenes` — the launcher's scene rows (id/name/mode/region/desc/meta).
-- Colours ride as dotted `style`/`color` paths into the token-resolved ui_elements.json.

local M = {}

local function tag(kind)
  return function(t)
    t.component = kind
    return t
  end
end
local Page = tag("page")
local Column = tag("column")
local Row = tag("row")
local Panel = tag("panel")
local Stack = tag("stack")
local Text = tag("text")
local Button = tag("button")
local Sprite = tag("sprite")

-- Sub-layers: the backdrop (gradient + Muse + scrim + thread) sits BELOW the popup /
-- panel + their buttons/text (a sprite otherwise draws over a same-layer panel).
local L_BG = 0
local L_UI = 1

-- ── shared popup pieces ──────────────────────────────────────────────

-- One centred text line in the popup column (`size` = row height, `text_size` = glyphs).
local function line(str, text_size, color, font, bind)
  local n = { size = text_size + 10, text_size = text_size, color = color, align = "center", font = font }
  if bind then n.text_bind = bind else n.text = str or "" end
  return Text(n)
end

-- The popup's vertical button stack, one Button per published `MENU.items` entry.
local function button_stack(m)
  local b = m.buttons
  local kids = {}
  for _, it in ipairs((MENU and MENU.items) or {}) do
    kids[#kids + 1] = Button {
      id = it.id, action = it.id, label = it.label,
      size = b.h, label_size = b.label_size,
      style = "modal.buttons.variants." .. (it.variant or "secondary"),
    }
  end
  return Column { gap = b.gap, children = kids }
end

-- The gothic popup panel (title · subtitle · countdown · divider · buttons · footer).
-- `width` fixed (fills its column in the launcher, or the popup's own width). Auto-heights.
local function popup(m, screen)
  local col = {}
  col[#col + 1] = line(screen.title, screen.title_size or m.title.size, "modal.title.color", "display")
  if screen.subtitle then
    col[#col + 1] = line(screen.subtitle, m.subtitle.size, "modal.subtitle.color", "label")
  end
  if MENU.screen == "confirm" then
    col[#col + 1] = line(nil, m.countdown.size, "modal.countdown.color", "body", "subtitle")
  end
  col[#col + 1] = Panel { size = 1, style = "modal.divider" }
  col[#col + 1] = button_stack(m)
  if screen.footer then
    col[#col + 1] = line(screen.footer, m.footer.size, "modal.footer.color", "label")
  end
  return Panel {
    id = "popup", width = m.panel.w, pad = m.panel.pad_x, gap = 16,
    layer = L_UI, style = "modal.panel", children = col,
  }
end

-- ── scene-selection panel (launcher) ─────────────────────────────────

-- One scene row: [96² bronze-framed preview] · [mode/name/desc/meta column] · [LOAD].
-- Fixed height so the preview stays ~square (the walker fills a flow child's cross-axis).
local ROW_H = 126

local function scene_row(sc)
  -- Bronze frame (pad) around a dark inner box — a screenshot placeholder until RTT
  -- thumbnails land.
  local preview = Panel {
    size = 96, pad = 3, style = "menu.preview_frame",
    children = { Panel { grow = 1, style = "menu.preview_inner" } },
  }
  -- Details: mode (accent) · name (large) · desc (italic) · region+meta (small caps).
  local meta = (sc.region and sc.region ~= "" and (sc.region .. "  \u{00B7}  " .. (sc.meta or ""))) or (sc.meta or "")
  local details = Column {
    grow = 1, gap = 3,
    children = {
      Text { text = sc.mode or "", size = 14, text_size = 10, color = "menu.mode", font = "label" },
      Text { text = sc.name or "", size = 30, text_size = 25, color = "menu.name", font = "display" },
      Text { text = sc.desc or "", size = 34, text_size = 14, color = "menu.row_desc", font = "body" },
      Text { text = meta, size = 14, text_size = 9, color = "menu.meta", font = "label" },
    },
  }
  -- LOAD: the shared button template, action = scene id; pushed to the row's bottom-right
  -- by a grow spacer above it (the walker fills a flow child's cross-axis, so a bare
  -- button would be row-tall).
  local load = Column {
    size = 150,
    children = {
      Stack { grow = 1 },
      Button { id = sc.id, action = sc.id, label = "LOAD", size = 42, label_size = 12,
               style = "modal.buttons.variants.primary" },
    },
  }
  return Row {
    size = ROW_H, pad = 15, gap = 20, style = "menu.row",
    children = { preview, details, load },
  }
end

-- The right panel: header (caption · title · count · blurb) · divider · scrolling rows.
local function scene_panel(m)
  local scenes = (MENU and MENU.scenes) or {}
  local head = Column {
    gap = 4, pad = 26,
    children = {
      Text { text = "DEMO BUILD \u{00B7} CLAY ENGINE", size = 14, text_size = 10, color = "menu.caption", font = "label" },
      Text { text = "Select a Scene", size = 40, text_size = 34, color = "menu.title", font = "display" },
      Text { text = #scenes .. " scenes available", size = 16, text_size = 10, color = "menu.note", font = "label" },
    },
  }
  local rows = {}
  for _, sc in ipairs(scenes) do
    rows[#rows + 1] = scene_row(sc)
  end
  local body = Column { grow = 1, pad = 30, gap = 16, children = rows }
  return Panel {
    id = "scene_panel", grow = 1, layer = L_UI, style = "menu.panel",
    children = { head, Panel { size = 1, style = "menu.divider" }, body },
  }
end

-- ── backdrop (shared by all three screens) ───────────────────────────
local function backdrop(screen)
  local kids = {}
  -- Full-screen gradient (menu) / flat overlay (pause/confirm).
  kids[#kids + 1] = Panel {
    anchor = "top_left", width_frac = 1.0, height_frac = 1.0, layer = L_BG,
    style = "screens." .. MENU.screen,
  }
  -- The Muse — aspect-locked SQUARE (no stretch), viewport-tall, dimmed, centred.
  if screen.muse and Textures and Textures.muse then
    kids[#kids + 1] = Sprite {
      anchor = "bottom", height_frac = 1.04, aspect = 1.0,
      tex = Textures.muse, alpha = (UI.menu and UI.menu.muse_alpha) or 0.34, layer = L_BG,
    }
    -- Fade overlay: two horizontal scrims (dark edges → clear centre) so the Muse reads
    -- through the middle while the popup / panel stay legible over the sides.
    kids[#kids + 1] = Panel { anchor = "top_left", width_frac = 0.5, height_frac = 1.0, layer = L_BG, style = "menu.scrim_l" }
    kids[#kids + 1] = Panel { anchor = "top_right", width_frac = 0.5, height_frac = 1.0, layer = L_BG, style = "menu.scrim_r" }
  end
  -- Prism thread accent down the left edge.
  if screen.spectrum then
    kids[#kids + 1] = Panel { anchor = "top_left", width = 4, height_frac = 1.0, layer = L_BG, style = "menu.thread" }
  end
  return kids
end

function M.tree()
  if not UI or not UI.modal or not MENU then
    return Page { id = "menu" }
  end
  local m = UI.modal
  local screen = (UI.screens and UI.screens[MENU.screen]) or {}
  local left = screen.layout == "left"

  local kids = backdrop(screen)

  if MENU.scene_select then
    -- LAUNCHER: popup (left, fixed width, top-anchored via a grow spacer) + scene panel
    -- (right, fills). One inset row spans the content area.
    kids[#kids + 1] = Row {
      anchor = "top_left", width_frac = 1.0, height_frac = 1.0, pad = 46, gap = 44, layer = L_UI,
      children = {
        Column { size = m.panel.w, children = { popup(m, screen), Stack { grow = 1 } } },
        scene_panel(m),
      },
    }
  else
    -- Default: the single popup, centred (pause/confirm) or left-hero (menu).
    local p = popup(m, screen)
    p.anchor = left and "left" or "center"
    p.offset = left and { 150, 0 } or { 0, 0 }
    kids[#kids + 1] = p
  end

  -- Studio mark, bottom-left (menu only).
  if screen.studio then
    kids[#kids + 1] = Text {
      text = screen.studio, anchor = "bottom_left", offset = { 26, -34 },
      size = m.studio.size + 4, text_size = m.studio.size, color = "modal.studio.color",
      font = "label", layer = L_UI,
    }
  end

  return Page { id = "menu", children = kids }
end

return M
