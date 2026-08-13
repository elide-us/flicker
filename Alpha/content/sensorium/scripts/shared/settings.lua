-- Prism Settings — a DECLARATIVE component tree (the Rust walker `run_ui` owns
-- layout, draw, and hit-test). Replaces the old immediate-mode `M.update`/`M.draw`
-- pixel-math + the `Widgets` toolkit. Template-free (201F4F51): the window is a
-- styled `stack` + `rune_corners` authored inline, and navigation is the native
-- `paged_menu` (PTT) in its LEFT page-rail mode — a VERTICAL page rail (Video /
-- Audio / Input) beside a horizontal tab rail (the Input sub-tabs) over the
-- scrolling content — with a footer (Restore / Apply / Save-and-Close) below.
--
-- Two-way contract lives in the node data (same channels every walker scene uses):
--   * `action`       — a momentary event the scene reads by id (go_video, settings_apply…).
--   * `bind`         — a Model key read for the value + written back (video_vsync, scroll_off…).
--   * `text_bind`    — a Model key whose PRE-FORMATTED string a text/button shows (bind_<id>).
--   * `visible_bind` — a Model key gating a subtree (sec_video, sub_keyboard…).
--   * `enabled_bind` — a Model key gating a control; unwired PREVIEW rows point it at
--                      `off` (always false) so their control is inert.
--   * `style`/`color`— dotted paths into ui_theme.json (palette single-sourced in
--                      theme.tokens); `color_bind` names a Model key holding such a path.
-- Layout / labels all come from `UI.settings`, so the tree and the engine cannot drift.
--
-- A row's `wired` flag marks whether it is bound to a real backend. Unwired rows
-- (FOV, brightness, all of Audio, aim/edge-pan, the output device) render their
-- control INERT + a bronze "PREVIEW" badge chip beside it, and dim the row label —
-- they are a layout preview, not a live control.

local M = {}

-- ── ergonomic node constructors (tag a table with its component kind) ──
local function tag(kind)
  return function(t) t.component = kind; return t end
end
local Screen  = tag("screen")
local Cell = tag("cell")
local Row     = tag("row")
local Stack   = tag("stack")
local Text    = tag("text")
local Button  = tag("button")
local Select  = tag("select")
local Toggle  = tag("toggle")
local Slider  = tag("slider")
local Pill    = tag("pill_toggle")
local Badge   = tag("badge")
local Opt     = tag("option")   -- pure DATA child of a select/pill (never drawn)
local Tabs        = tag("tabs")          -- the VERTICAL page rail (Video/Audio/Input)
local Paged       = tag("paged_menu")    -- the PTT page/tab control (native kind)
local PopupPanel  = tag("popup_panel")   -- native titled modal slab (the dialogs)
local Runes       = tag("rune_corners")  -- the window's four carved corner glyphs

-- Wired rows bind to the scene's canonical Model keys; everything else is a
-- read-only `pv_<id>` preview key the scene publishes with a fixed default.
local BINDS = {
  display_mode = "video_display_mode",
  resolution   = "video_resolution",
  quality      = "video_quality",
  vsync        = "video_vsync",
  fps_limit    = "video_fps_limit",
  m_look       = "look_sens_pct",         -- 0..100 display space; scene maps to backend
  m_invert     = "input_mouse_invert_pitch",
}

-- ── small text helpers ──────────────────────────────────────────────
-- `box` = the node's main-axis LENGTH (line height in a column); `glyph` = font size.
local function line(text, box, glyph, color, font, align)
  return Text { text = text, size = box, text_size = glyph, color = color, font = font or "body", align = align }
end

-- ── select / pill option children. An option's value is its 0-based INDEX, and an
--    index is a NUMBER (the strip boundary, enforced by the engine's hit arms) — the
--    scene reads the index straight back off the bind. ──
local function options_of(r)
  local out = {}
  for i, label in ipairs(r.options or {}) do
    out[#out + 1] = Opt { value = i - 1, label = label }
  end
  return out
end

-- ── one control widget for a data row (dropdown→select, segment→pill, cycler→select,
--    toggle→toggle, slider→slider, static→text) ──
local CTRL_W = 210

local function control_node(r, wired)
  local key = BINDS[r.id] or ("pv_" .. r.id)
  local off = (not wired) and "off" or nil      -- inert when unwired
  local k = r.kind
  if k == "toggle" then
    return Toggle { id = "c_" .. r.id, bind = key, size = 56, style = "settings.controls.toggle", enabled_bind = off }
  elseif k == "slider" then
    return Slider {
      id = "c_" .. r.id, bind = key, size = CTRL_W,
      min = r.min or 0, max = r.max or 100, value_w = 46, slider_h = 8,
      decimals = 0, suffix = r.suffix, style = "settings.controls.slider", enabled_bind = off,
    }
  elseif k == "dropdown" or k == "cycler" then
    return Select {
      id = "c_" .. r.id, bind = key, size = CTRL_W,
      style = "settings.controls", enabled_bind = off, children = options_of(r),
    }
  elseif k == "segment" then
    return Pill {
      id = "c_" .. r.id, bind = key, size = math.max(CTRL_W, 60 * #(r.options or {})),
      style = "settings.controls.pill", enabled_bind = off, children = options_of(r),
    }
  elseif k == "static" then
    return line(r.value or "", CTRL_W, 15, "settings.controls.field.label", "body", "right")
  end
  return Stack { size = CTRL_W }
end

-- ── one settings row: name (+desc) on the left, control (+ PREVIEW badge) right ──
local function ctrl_row(r)
  local S = UI.settings
  local wired = r.wired == true
  local name_color = wired and "settings.row.name_color" or "settings.row.desc_color"

  -- Vertically CENTER the name/desc block in the row (grow spacers top+bottom) so the
  -- label lines up with its control instead of sitting at the row's top edge.
  local left = { Stack { grow = 1 } }
  left[#left + 1] = line(r.name or "", S.row.name_size + 3, S.row.name_size, name_color, "body", "left")
  if r.desc then
    left[#left + 1] = line(r.desc, S.row.desc_size + 3, S.row.desc_size, "settings.row.desc_color", "body", "left")
  end
  left[#left + 1] = Stack { grow = 1 }

  local right = { Stack { grow = 1 } }
  if not wired then
    right[#right + 1] = Badge { size = 72, tone = "bronze", label = "$set_preview", style = "badge" }
  end
  right[#right + 1] = control_node(r, wired)

  return Row {
    size = S.row.h,
    children = {
      Cell { grow = 1, gap = 2, children = left },
      Row { size = CTRL_W + 88, gap = 8, children = right },
    },
  }
end

-- ── a group header line ──
local function group_head(name)
  local S = UI.settings
  return line(name, 30, S.row.group_size, "settings.row.group_color", "label", "left")
end

-- Append every group's header + rows (video / audio / mouse share this shape).
local function add_groups(out, groups)
  for _, g in ipairs(groups or {}) do
    out[#out + 1] = group_head(g.name)
    for _, r in ipairs(g.rows or {}) do
      out[#out + 1] = ctrl_row(r)
    end
    out[#out + 1] = Stack { size = 10 }
  end
end

-- ── section: VIDEO ──
local function video_section()
  local kids = {}
  add_groups(kids, UI.settings.video.groups)
  return Cell { visible_bind = "sec_video", gap = 0, children = kids }
end

-- ── section: AUDIO (a "not yet implemented" notice, then the preview groups) ──
local function audio_section()
  local st = UI.settings.audio.stub
  local kids = {
    line(st.title, 22, 10, "settings.audio.stub.title_color", "label", "left"),
    line(st.body, 22, 14, "settings.audio.stub.body_color", "body", "left"),
    Stack { size = 12 },
  }
  add_groups(kids, UI.settings.audio.groups)
  return Cell { visible_bind = "sec_audio", gap = 0, children = kids }
end

-- ── input · keyboard: a rebind banner (while capturing) + one keycap button per action ──
local function keyboard_tab()
  local S = UI.settings
  local kids = {
    line("$set_press_any_key_to_bind_esc_to_cancel_back",
      24, 14, "settings.rebind_banner.text_color", "body", "left"),
  }
  kids[1].visible_bind = "rebinding"
  for _, g in ipairs(S.input.keyboard.groups) do
    kids[#kids + 1] = group_head(g.name)
    for _, a in ipairs(g.actions) do
      kids[#kids + 1] = Row {
        size = 42,
        children = {
          Cell { grow = 1, children = { Stack { grow = 1 }, line(a.label, 18, 16, "settings.row.name_color", "body", "left"), Stack { grow = 1 } } },
          -- keycap: shows the current binding (`bind_<id>`), fires `rebind_<id>`; the
          -- scene owns the capture. Styled as a stone button (the walker button reads
          -- fill_top/fill_bot, which settings.controls.keycap does not carry).
          Button { id = "kc_" .. a.id, action = "rebind_" .. a.id, text_bind = "bind_" .. a.id,
                   size = S.controls.keycap.w, label_size = S.controls.keycap.label_size,
                   style = "modal.buttons.variants.secondary" },
        },
      }
    end
    kids[#kids + 1] = Stack { size = 10 }
  end
  return Cell { visible_bind = "sub_keyboard", gap = 0, children = kids }
end

-- ── input · mouse: the pointer + commander groups (m_look / m_invert wired) ──
local function mouse_tab()
  local kids = {}
  add_groups(kids, UI.settings.input.mouse.groups)
  return Cell { visible_bind = "sub_mouse", gap = 0, children = kids }
end

-- Controller-profile selector options, DATA-driven from the `PROFILES` global (the named
-- InputProfiles the shell publishes, spec §7.3). Falls back to a single "Default" when
-- unpublished (e.g. the build-time tree smoke test builds with no PROFILES global).
-- An option's value is its INDEX into PROFILES; the scene maps the index back to the
-- profile's stable name (the strip boundary: an index is a number).
local function profile_opts()
  local opts = {}
  if PROFILES then
    for i, p in ipairs(PROFILES) do
      opts[#opts + 1] = Opt { value = i - 1, label = p.label }
    end
  end
  if #opts == 0 then
    opts[1] = Opt { value = 0, label = "$set_default" }
  end
  return opts
end

-- ── input · controller: a PROFILE selector (the named InputProfiles) + the info notes ──
local function controller_tab()
  local c = UI.settings.input.controller
  return Cell {
    visible_bind = "sub_controller", gap = 0,
    children = {
      group_head("$set_controller_profile"),
      Row {
        size = 50,
        children = {
          Cell { grow = 1, children = { Stack { size = 8 }, line("$set_active_profile", 20, 16, "settings.row.name_color", "body", "left") } },
          Row { size = CTRL_W + 88, gap = 8, children = {
            Stack { grow = 1 },
            Select { id = "ctrl_profile", bind = "ctrl_profile", size = CTRL_W, style = "settings.controls",
                     children = profile_opts() },
          } },
        },
      },
      Stack { size = 16 },
      line(c.title, 30, c.title_size, "settings.input.controller.title_color", "display", "left"),
      line(c.body, 26, 15, "settings.input.controller.body_color", "body", "left"),
    },
  }
end

-- ── page rail: a VERTICAL `tabs` strip = the paged_menu's PAGE rail. Bound to
--    `settings_page` (0=video / 1=audio / 2=input); the selected cell wears the
--    primary button style, the idle cells the secondary — the vertical rail IS the
--    active-section indicator now (the old `go_<id>` nav rail + `nav_<id>_style`
--    bind are gone). Clicking a cell reports the new INDEX; the scene maps it back
--    to the `sec_*` radio. ──
local function page_rail()
  return Tabs {
    id = "settings_page", bind = "settings_page", vertical = true, gap = 8,
    tab_active = "modal.buttons.variants.primary",
    tab_idle = "modal.buttons.variants.secondary",
    children = {
      Opt { value = 0, label = "$set_video" },
      Opt { value = 1, label = "$set_audio" },
      Opt { value = 2, label = "$set_input" },
    },
  }
end

-- ── tab rail: the Input sub-tabs as a segmented pill = the paged_menu's TAB rail.
--    Two-way `input_subtab` (the bind carries the sub-tab INDEX; the scene maps it
--    back to the `sub_*` surface). The paged_menu shows it ONLY on the Input page
--    (its `tabs_shown = "input_page_active"` gate), so no `visible_bind` here. ──
local function tab_rail()
  return Pill {
    id = "input_subtab", bind = "input_subtab",
    size = 330, style = "settings.controls.pill",
    children = {
      Opt { value = 0, label = "$set_keyboard" },
      Opt { value = 1, label = "$set_mouse" },
      Opt { value = 2, label = "$set_controller" },
    },
  }
end

-- ── the scrolling content well: the active section's rows, gated by visible_bind ──
local function content_scroll()
  return {
    component = "list",
    id = "settings_scroll", bind = "scroll_off", scroll_speed = 46,
    grow = 1, pad = 6,
    children = {
      video_section(),
      audio_section(),
      Cell { visible_bind = "sec_input", gap = 0, children = { keyboard_tab(), mouse_tab(), controller_tab() } },
    },
  }
end

-- ── footer controls (the bottom Row of the window's content cell) ──
-- APPLY saves without closing (flash); SAVE AND CLOSE (primary) saves and pops. The
-- titlebar × fires `settings_close`, which confirms first when there are unsaved edits.
local function footer_children()
  local F = UI.settings.footer
  return {
    Button { id = "restore", action = "settings_restore", label = F.restore, size = 180, label_size = 12,
             style = "modal.buttons.variants.secondary" },
    line(F.applied, 180, 12, "settings.footer.applied_color", "label", "left"), -- flash, gated below
    Stack { grow = 1 },
    Button { id = "apply", action = "settings_apply", label = F.apply, size = 104, label_size = 14,
             style = "modal.buttons.variants.secondary" },
    Button { id = "save_close", action = "settings_back", label = F.save_close, size = 168, label_size = 14,
             style = "modal.buttons.variants.primary" },
  }
end

-- ── modal dialogs (children of the scene root; gated by a Model flag the scene sets) ──
-- Each is a full-bleed dim scrim (`screens.pause`) gated by `visible_bind`, holding a
-- native `popup_panel` — the carved slab draws its own titled chrome and flows its
-- authored children (a message line + the action buttons) as items. The scene enforces
-- modality in Rust (while a flag is set it processes only that dialog's actions), so the
-- overlay is purely visual; the button `action`s + gates are unchanged from before.
local function overlay(gate, panel)
  return Cell {
    visible_bind = gate, anchor = "top_left", width_frac = 1.0, height_frac = 1.0,
    style = "screens.pause", children = { panel },
  }
end

local function dialogs()
  local D = UI.settings.dialogs
  return {
    -- Unsaved-changes confirm (fired by × / Esc when the buffer is dirty).
    overlay("confirm_close", PopupPanel {
      title = D.close_title, title_size = 26, panel_style = "modal.panel",
      anchor = "center", layer = 2,
      children = {
        line(D.close_msg, 44, 15, "settings.row.name_color", "body", "center"),
        Button { action = "confirm_save",    label = D.save,    size = 46, label_size = 14, style = "modal.buttons.variants.primary" },
        Button { action = "confirm_discard", label = D.discard, size = 46, label_size = 14, style = "modal.buttons.variants.danger" },
        Button { action = "confirm_cancel",  label = D.cancel,  size = 46, label_size = 14, style = "modal.buttons.variants.secondary" },
      },
    }),
    -- Restore-defaults acknowledgement (single OK).
    overlay("restore_note", PopupPanel {
      title = D.restore_title, title_size = 26, panel_style = "modal.panel",
      anchor = "center", layer = 2,
      children = {
        line(D.restore_msg, 44, 15, "settings.row.name_color", "body", "center"),
        Button { action = "restore_ok", label = D.ok, size = 46, label_size = 14, style = "modal.buttons.variants.primary" },
      },
    }),
  }
end

function M.tree()
  if not UI or not UI.settings then
    -- Even the degenerate no-UI root keeps its declared input binding (S9):
    -- the screen IS the declaration, so Esc→close must not depend on layout.
    return Screen { id = "settings", on_cancel = "settings_close" }
  end
  local S = UI.settings
  local footer = footer_children()
  footer[2].visible_bind = "applied" -- the "SETTINGS APPLIED" flash

  -- The window is HAND-AUTHORED now (template tier is gone): a styled `stack` carrying
  -- the `settings.window` chrome bg, an inset content `cell` (titlebar · paged_menu ·
  -- footer), and a `rune_corners` overlay that paints the four carved corners on top.
  -- The `edge` inset clears the corner runes (~30, the frame's old clearance constant).
  local edge = S.titlebar.pad_x
  local window = Stack {
    style = "settings.window", anchor = "center", width = S.window.w, height = S.window.h,
    children = {
      Cell {
        anchor = "top_left", width_frac = 1.0, height_frac = 1.0, pad = edge,
        children = {
          -- Titlebar: the settings title (left) + the × close control (right, danger).
          -- The × fires `settings_close` — the SAME intent Esc/pad-B emit — so the
          -- scene's confirm-if-dirty ladder handles both alike.
          Row {
            size = S.titlebar.h,
            children = {
              Cell { grow = 1, children = {
                Stack { grow = 1 },
                line(S.titlebar.title, S.titlebar.title_size + 4, S.titlebar.title_size, "settings.titlebar.title_color", "display", "left"),
                Stack { grow = 1 },
              } },
              Cell { size = 46, children = {
                Stack { grow = 1 },
                Button { id = "close", action = "settings_close", label = "×", size = 34, label_size = 20, style = "modal.buttons.variants.danger" },
                Stack { grow = 1 },
              } },
            },
          },
          -- The PTT page/tab control in LEFT page-rail mode: the vertical page rail
          -- (Video/Audio/Input) as a fixed `page_w` column, the horizontal Input tab
          -- rail (shown only on the Input page via `input_page_active`), and the
          -- scrolling content below — the gated sections the scene publishes.
          Paged {
            page_side = "left", page_w = S.nav.w, grow = 1,
            tabs_shown = "input_page_active",
            children = { page_rail(), tab_rail(), content_scroll() },
          },
          -- Footer: Restore / applied-flash / Apply / Save-and-Close.
          Row { size = S.footer.h, children = footer },
        },
      },
      Runes { style = "settings.runes", anchor = "top_left", width_frac = 1.0, height_frac = 1.0 },
    },
  }

  local children = {
    -- Dim scrim behind the modal (translucent; reuses the pause overlay token).
    Cell { anchor = "top_left", width_frac = 1.0, height_frac = 1.0, style = "screens.pause" },
    window,
  }
  -- Modal dialogs sit last so they overlay the window when their gate is set.
  for _, d in ipairs(dialogs()) do children[#children + 1] = d end
  -- The screen's input DECLARATION (S9): Cancel (Esc / pad B through the shell's
  -- Menu-context bus) fires `settings_close` — the SAME result name the × close
  -- button emits, so the scene's confirm-if-dirty ladder handles both alike.
  return Screen { id = "settings", on_cancel = "settings_close", children = children }
end

return M
