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
--   * `MENU.mode` / `MENU.note` / `MENU.panel_head` — the MODE-TIER page fields
--     (shell-published, realm-agnostic here): a non-empty `mode` marks a tier-2
--     page (its root declares `on_cancel = "menu_back"`, so Escape = the BACK
--     button); `note` (a `$token`) rides the popup footer (the DM page's
--     under-construction note); `panel_head = false` drops the scene panel's
--     header block (the Adventurer page shows exactly its entry, no other notes).
-- Colours ride as dotted `style`/`color` paths into the token-resolved ui_elements.json.

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
local Button = tag("button")
local Sprite = tag("sprite")

-- Sub-layers: the backdrop (gradient + Muse + scrim + thread) sits BELOW the popup /
-- panel + their buttons/text (a sprite otherwise draws over a same-layer panel).
local L_BG = 0
local L_UI = 1

-- ── shared popup pieces ──────────────────────────────────────────────

-- One Button per published `MENU.items` entry — the raw list, shared by the
-- popup TEMPLATE slots of all three screens, so they build buttons identically.
local function item_buttons(m)
  local b = m.buttons
  local kids = {}
  for i, it in ipairs((MENU and MENU.items) or {}) do
    local btn = {
      id = it.id, action = it.id, label = it.label,
      size = b.h, label_size = b.label_size,
      style = "modal.buttons.variants." .. (it.variant or "secondary"),
    }
    -- Directional-nav (spec §8): popup buttons form a per-screen focus group,
    -- ordered top-to-bottom, so d-pad / arrows move between them (and on the
    -- launcher MENU screen the bumpers cross to the "scenes" group of LOAD buttons).
    -- Authored for EVERY popup modal now — menu, pause, and confirm — so all are
    -- pad-navigable; the group id is the screen name so each screen is
    -- self-contained. Pause/confirm buttons flow through here into their template
    -- slots, so this reaches them too.
    btn.tab_group = MENU.screen
    btn.nav_ordinal = i - 1
    kids[#kids + 1] = Button(btn)
  end
  return kids
end

-- The gothic popup panel (title · subtitle · divider · buttons · footer) — the
-- shared `popup_panel` data TEMPLATE (the same panel `popup_menu` nests for the
-- pause screen), instantiated directly so the menu / launcher popup and the
-- pause popup are ONE definition. Width fixed (fills its column in the launcher,
-- or the popup's own width); heights auto (`text_size` + the engine's leading).
local function popup(m, screen)
  -- A published page note (the DM tier's "$dm_coming_soon") rides the popup's
  -- footer slot; otherwise the screen's own footer (if any) stays.
  local note = MENU.note
  local footer = (note ~= nil and note ~= "" and note) or screen.footer
  return {
    template = "popup_panel",
    id = "popup",
    title = screen.title, title_size = screen.title_size or m.title.size,
    subtitle = screen.subtitle, subtitle_size = m.subtitle.size,
    divider = true,
    footer = footer, footer_size = m.footer.size,
    panel_w = m.panel.w, panel_pad = m.panel.pad_x, panel_gap = 16,
    items_gap = m.buttons.gap,
    layer = L_UI,
    slots = { items = item_buttons(m) },
  }
end

-- ── scene-selection panel (launcher) ─────────────────────────────────

-- One scene row: [96² bronze-framed preview] · [mode/name/desc/meta column] · [LOAD].
-- Fixed height so the preview stays ~square (the walker fills a flow child's cross-axis).
local ROW_H = 126

local function scene_row(sc, ord)
  -- Bronze frame (pad) around a dark inner box — a screenshot placeholder until RTT
  -- thumbnails land.
  local preview = Cell {
    size = 96, pad = 3, style = "menu.preview_frame",
    children = { Cell { grow = 1, style = "menu.preview_inner" } },
  }
  -- Details: mode (accent) · name (large) · desc (italic) · region+meta (small caps).
  local meta = (sc.region and sc.region ~= "" and (sc.region .. "  \u{00B7}  " .. (sc.meta or ""))) or (sc.meta or "")
  local details = Cell {
    grow = 1, gap = 3,
    children = {
      Text { text = sc.mode or "", size = 14, text_size = 10, color = "menu.mode", font = "label" },
      Text { text = sc.name or "", size = 32, text_size = 28, color = "menu.name", font = "display" },
      Text { text = sc.desc or "", size = 34, text_size = 14, color = "menu.row_desc", font = "body" },
      Text { text = meta, size = 14, text_size = 9, color = "menu.meta", font = "label" },
    },
  }
  -- LOAD: the shared button template, action = scene id; pushed to the row's bottom-right
  -- by a grow spacer above it (the walker fills a flow child's cross-axis, so a bare
  -- button would be row-tall).
  local load = Cell {
    size = 150,
    children = {
      Stack { grow = 1 },
      -- The LOAD buttons form the "scenes" focus group (spec §8): d-pad / arrows
      -- move row-to-row, the bumpers cross back to the "menu" popup group.
      Button { id = sc.id, action = sc.id, label = "$menu_load", size = 42, label_size = 12,
               style = "modal.buttons.variants.primary",
               tab_group = "scenes", nav_ordinal = ord or 0 },
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
  local head = Cell {
    gap = 4, pad = 26,
    children = {
      Text { text = "$menu_demo_caption", size = 14, text_size = 10, color = "menu.caption", font = "label" },
      Text { text = "$menu_select_a_scene", size = 48, text_size = 42, color = "menu.title", font = "display" },
      -- STRINGS-GATE EXEMPT (S10): a composed-dynamic string — the live count is
      -- concatenated with the caption at build, and the stringtable deliberately
      -- has no format language. Localise by splitting count/caption nodes when a
      -- second locale lands.
      Text { text = #scenes .. " scenes available", size = 16, text_size = 10, color = "menu.note", font = "label" },
    },
  }
  local rows = {}
  for i, sc in ipairs(scenes) do
    rows[#rows + 1] = scene_row(sc, i - 1)
  end
  local body = Cell { grow = 1, pad = 30, gap = 16, children = rows }
  -- `MENU.panel_head = false` (the Adventurer tier page) drops the header block +
  -- its divider: the panel shows exactly its rows, no caption/title/count notes.
  local kids = {}
  if MENU.panel_head ~= false then
    kids[#kids + 1] = head
    kids[#kids + 1] = Cell { size = 1, style = "menu.divider" }
  end
  kids[#kids + 1] = body
  return Cell {
    id = "scene_panel", grow = 1, layer = L_UI, style = "menu.panel",
    children = kids,
  }
end

-- ── backdrop (shared by all three screens) ───────────────────────────
local function backdrop(screen)
  local kids = {}
  -- Full-screen gradient (menu) / flat overlay (pause/confirm).
  kids[#kids + 1] = Cell {
    anchor = "top_left", width_frac = 1.0, height_frac = 1.0, layer = L_BG,
    style = "screens." .. MENU.screen,
  }
  -- The Muse — aspect-locked SQUARE pinned to the RIGHT edge, vertically centred,
  -- spanning the full width so she fills leftward (her BAKED left-edge dissolve —
  -- theme.rs authored it for a right-margin draw — fades her into the menu) and
  -- spills past top/bottom as the window widens: cover, never letterbox.
  if screen.muse and Textures and Textures.muse then
    kids[#kids + 1] = Sprite {
      anchor = "right", width_frac = 1.0, aspect = 1.0,
      tex = Textures.muse, alpha = (UI.menu and UI.menu.muse_alpha) or 0.34, layer = L_BG,
    }
    -- Fade overlay: two horizontal scrims (dark edges → clear centre) so the Muse reads
    -- through the middle while the popup / panel stay legible over the sides.
    kids[#kids + 1] = Cell { anchor = "top_left", width_frac = 0.5, height_frac = 1.0, layer = L_BG, style = "menu.scrim_l" }
    kids[#kids + 1] = Cell { anchor = "top_right", width_frac = 0.5, height_frac = 1.0, layer = L_BG, style = "menu.scrim_r" }
  end
  -- Prism thread accent down the left edge.
  if screen.spectrum then
    kids[#kids + 1] = Cell { anchor = "top_left", width = 4, height_frac = 1.0, layer = L_BG, style = "menu.thread" }
  end
  return kids
end

function M.tree()
  if not UI or not UI.modal or not MENU then
    return Screen { id = "menu" }
  end
  local m = UI.modal
  local screen = (UI.screens and UI.screens[MENU.screen]) or {}

  -- Pause + display-confirm are single-popup modals: build them from the shared
  -- `popup_menu` / `choice_dialog` TEMPLATES (Rust builders, expanded by the walker in
  -- MenuView), so the modal chrome lives in ONE place. The MENU screen (below) keeps its
  -- bespoke launcher / hero composition — a two-column master-detail, not a popup.
  if MENU.screen == "confirm" then
    return {
      template = "choice_dialog",
      title = screen.title, title_size = screen.title_size or m.title.size,
      subtitle_bind = "subtitle", subtitle_size = m.countdown.size,
      overlay_style = "screens.confirm",
      panel_w = m.panel.w, panel_pad = m.panel.pad_x, gap = 16,
      btn_gap = m.buttons.gap,
      slots = { buttons = item_buttons(m) },
    }
  elseif MENU.screen == "pause" then
    return {
      template = "popup_menu",
      -- Placement is data now: the popup anchors where the instance says (the
      -- template's `@anchor` / `@offset_x`), replacing the old `layout` switch.
      anchor = (screen.layout == "left") and "left" or "center",
      offset_x = (screen.layout == "left") and 150 or 0,
      title = screen.title, title_size = screen.title_size or m.title.size,
      subtitle = screen.subtitle, subtitle_size = m.subtitle.size,
      footer = screen.footer, footer_size = m.footer.size,
      divider = true,
      overlay_style = "screens.pause",
      panel_w = m.panel.w, panel_pad = m.panel.pad_x, panel_gap = 16,
      items_gap = m.buttons.gap,
      slots = { items = item_buttons(m) },
    }
  end

  local left = screen.layout == "left"

  local kids = backdrop(screen)

  if MENU.scene_select then
    -- LAUNCHER: popup (left, fixed width, top-anchored via a grow spacer) + scene panel
    -- (right, fills). One inset row spans the content area.
    kids[#kids + 1] = Row {
      anchor = "top_left", width_frac = 1.0, height_frac = 1.0, pad = 46, gap = 44, layer = L_UI,
      children = {
        Cell { size = m.panel.w, children = { popup(m, screen), Stack { grow = 1 } } },
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

  local root = Screen { id = "menu", children = kids }
  -- A tier-2 mode page (non-empty `MENU.mode`) declares the BACK intent on its
  -- root: Escape / pad-B rides the menu mini-bus (S9) to the SAME `menu_back`
  -- result the BACK button fires, and the scene pops to the root menu.
  if MENU.mode ~= nil and MENU.mode ~= "" then
    root.on_cancel = "menu_back"
  end
  return root
end

return M
