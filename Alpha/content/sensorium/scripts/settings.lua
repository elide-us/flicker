-- Prism Settings — a DECLARATIVE component tree (the Rust walker `run_ui` owns
-- layout, draw, and hit-test). Replaces the old immediate-mode `M.update`/`M.draw`
-- pixel-math + the `Widgets` toolkit. One windowed screen: a title bar + corner
-- runes (via the shared `window` TEMPLATE), a left category rail (Video / Audio /
-- Input), a scrolling content region, and a footer (Restore / Back / Apply).
--
-- Two-way contract lives in the node data (same channels every walker scene uses):
--   * `action`       — a momentary event the scene reads by id (go_video, settings_apply…).
--   * `bind`         — a Model key read for the value + written back (video_vsync, scroll_off…).
--   * `text_bind`    — a Model key whose PRE-FORMATTED string a text/button shows (bind_<id>).
--   * `visible_bind` — a Model key gating a subtree (sec_video, sub_keyboard…).
--   * `enabled_bind` — a Model key gating a control; unwired PREVIEW rows point it at
--                      `off` (always false) so their control is inert.
--   * `style`/`color`— dotted paths into ui_elements.json (palette single-sourced in
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
local Page    = tag("page")
local Column  = tag("column")
local Row     = tag("row")
local Panel   = tag("panel")
local Stack   = tag("stack")
local Text    = tag("text")
local Button  = tag("button")
local Select  = tag("select")
local Toggle  = tag("toggle")
local Slider  = tag("slider")
local Pill    = tag("pill_toggle")
local Badge   = tag("badge")
local Opt     = tag("option")   -- pure DATA child of a select/pill (never drawn)

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
local function bound(key, box, glyph, color, font, align)
  return Text { text_bind = key, size = box, text_size = glyph, color = color, font = font or "body", align = align }
end

-- ── select / pill option children (value = 0-based index string, matched by the
--    scene which parses the index back) ──
local function options_of(r)
  local out = {}
  for i, label in ipairs(r.options or {}) do
    out[#out + 1] = Opt { value = tostring(i - 1), label = label }
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
    right[#right + 1] = Badge { size = 72, tone = "bronze", label = "PREVIEW", style = "badge" }
  end
  right[#right + 1] = control_node(r, wired)

  return Row {
    size = S.row.h,
    children = {
      Column { grow = 1, gap = 2, children = left },
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
  return Column { visible_bind = "sec_video", gap = 0, children = kids }
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
  return Column { visible_bind = "sec_audio", gap = 0, children = kids }
end

-- ── input · keyboard: a rebind banner (while capturing) + one keycap button per action ──
local function keyboard_tab()
  local S = UI.settings
  local kids = {
    line("Press any key to bind  ·  Esc to cancel  ·  Backspace to unbind",
      24, 14, "settings.rebind_banner.text_color", "body", "left"),
  }
  kids[1].visible_bind = "rebinding"
  for _, g in ipairs(S.input.keyboard.groups) do
    kids[#kids + 1] = group_head(g.name)
    for _, a in ipairs(g.actions) do
      kids[#kids + 1] = Row {
        size = 42,
        children = {
          Column { grow = 1, children = { Stack { grow = 1 }, line(a.label, 18, 16, "settings.row.name_color", "body", "left"), Stack { grow = 1 } } },
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
  return Column { visible_bind = "sub_keyboard", gap = 0, children = kids }
end

-- ── input · mouse: the pointer + commander groups (m_look / m_invert wired) ──
local function mouse_tab()
  local kids = {}
  add_groups(kids, UI.settings.input.mouse.groups)
  return Column { visible_bind = "sub_mouse", gap = 0, children = kids }
end

-- ── input · controller: a PROFILE selector (one "Default" today) + the info notes ──
local function controller_tab()
  local c = UI.settings.input.controller
  return Column {
    visible_bind = "sub_controller", gap = 0,
    children = {
      group_head("CONTROLLER PROFILE"),
      Row {
        size = 50,
        children = {
          Column { grow = 1, children = { Stack { size = 8 }, line("Active Profile", 20, 16, "settings.row.name_color", "body", "left") } },
          Row { size = CTRL_W + 88, gap = 8, children = {
            Stack { grow = 1 },
            Select { id = "ctrl_profile", bind = "ctrl_profile", size = CTRL_W, style = "settings.controls",
                     children = { Opt { value = "default", label = "Default" } } },
          } },
        },
      },
      Stack { size = 16 },
      line(c.title, 30, c.title_size, "settings.input.controller.title_color", "display", "left"),
      line(c.body, 26, 15, "settings.input.controller.body_color", "body", "left"),
    },
  }
end

-- ── nav rail: CATEGORIES header, one button per section (active lit via style_bind),
--    a footer block. Buttons fire `go_<id>`; the scene tracks the active section. ──
local function nav_rail()
  local S = UI.settings
  local nav = S.nav
  local btn = function(id, label)
    return Button { id = "nav_" .. id, action = "go_" .. id, label = label, size = nav.row_h,
                    label_size = nav.label_size, style_bind = "nav_" .. id .. "_style" }
  end
  return Column {
    size = nav.w, pad = nav.pad, gap = nav.gap,
    children = {
      line(nav.header, 22, nav.header_size, "settings.nav.header_color", "label", "left"),
      btn("video", "VIDEO"),
      btn("audio", "AUDIO"),
      btn("input", "INPUT"),
      Stack { grow = 1 },
      line(nav.footer_title, 16, nav.header_size, "settings.nav.footer_title_color", "label", "left"),
      line(nav.footer_sub, 16, 12, "settings.nav.footer_sub_color", "body", "left"),
    },
  }
end

-- ── content header: kicker + section title, with the input sub-tab pills on the
--    right (only while the Input section is active). ──
local function content_header()
  local S = UI.settings
  return Row {
    size = 72,
    children = {
      Column { grow = 1, gap = 4, children = {
        Stack { size = 6 },
        bound("kicker", 16, S.content.kicker_size, "settings.content.title_color", "label", "left"),
        bound("sec_title", 40, S.content.title_size, "settings.content.title_color", "display", "left"),
      } },
      -- Input sub-tabs as a segmented pill (two-way `input_subtab`); matches the
      -- current pill sub-tabs and stays in the settings namespace.
      Pill {
        id = "input_subtab", bind = "input_subtab", visible_bind = "sec_input",
        size = 330, style = "settings.controls.pill",
        children = {
          Opt { value = "keyboard", label = "KEYBOARD" },
          Opt { value = "mouse", label = "MOUSE" },
          Opt { value = "controller", label = "CONTROLLER" },
        },
      },
    },
  }
end

-- ── the scrolling content well: the active section's rows, gated by visible_bind ──
local function content_scroll()
  return {
    component = "scroll",
    id = "settings_scroll", bind = "scroll_off", wheel = "wheel", scroll_speed = 46,
    grow = 1, pad = 6,
    children = {
      video_section(),
      audio_section(),
      Column { visible_bind = "sec_input", gap = 0, children = { keyboard_tab(), mouse_tab(), controller_tab() } },
    },
  }
end

-- ── footer controls (spliced into the window template's footer slot) ──
-- APPLY saves without closing (flash); SAVE AND CLOSE (primary) saves and pops. The
-- window's × fires `settings_close`, which confirms first when there are unsaved edits.
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
-- Both use the shared `choice_dialog` template. The scene enforces modality in Rust
-- (while a flag is set it processes only that dialog's actions), so the dim overlay is
-- purely visual.
local function dialogs()
  local D = UI.settings.dialogs
  return {
    -- Unsaved-changes confirm (fired by × / Esc when the buffer is dirty).
    {
      template = "choice_dialog", visible_bind = "confirm_close",
      title = D.close_title, title_size = 26, message = D.close_msg,
      overlay_style = "screens.pause", panel_style = "modal.panel",
      slots = { buttons = {
        Button { action = "confirm_save",    label = D.save,    size = 46, label_size = 14, style = "modal.buttons.variants.primary" },
        Button { action = "confirm_discard", label = D.discard, size = 46, label_size = 14, style = "modal.buttons.variants.danger" },
        Button { action = "confirm_cancel",  label = D.cancel,  size = 46, label_size = 14, style = "modal.buttons.variants.secondary" },
      } },
    },
    -- Restore-defaults acknowledgement (single OK).
    {
      template = "choice_dialog", visible_bind = "restore_note",
      title = D.restore_title, title_size = 26, message = D.restore_msg,
      overlay_style = "screens.pause", panel_style = "modal.panel",
      confirm_label = D.ok, confirm_action = "restore_ok", confirm_variant = "primary",
    },
  }
end

function M.tree()
  if not UI or not UI.settings then
    return Page { id = "settings" }
  end
  local S = UI.settings
  local footer = footer_children()
  footer[2].visible_bind = "applied" -- the "SETTINGS APPLIED" flash

  -- The `window` TEMPLATE owns the frame: title bar + content well + rune corners +
  -- footer bar. Its `content` slot is the rail | body layout; its `footer` slot is the
  -- Restore / Apply / Save-and-Close row. The × fires `settings_close` (confirm-if-dirty).
  local children = {
    -- Dim scrim behind the modal (translucent; reuses the pause overlay token).
    Panel { anchor = "top_left", width_frac = 1.0, height_frac = 1.0, style = "screens.pause" },
    {
      template = "window",
      title = S.titlebar.title, title_size = S.titlebar.title_size,
      w = S.window.w, h = S.window.h, style = "settings",
      title_h = S.titlebar.h, title_pad = S.titlebar.pad_x,
      close_action = "settings_close", close_style = "modal.buttons.variants.danger",
      footer_h = S.footer.h, footer_pad = S.footer.pad_x, footer_gap = 12,
      slots = {
        content = {
          Row {
            grow = 1,
            children = {
              nav_rail(),
              Column { grow = 1, pad = 24, gap = 12, children = { content_header(), content_scroll() } },
            },
          },
        },
        footer = footer,
      },
    },
  }
  -- Modal dialogs sit last so they overlay the window when their gate is set.
  for _, d in ipairs(dialogs()) do children[#children + 1] = d end
  return Page { id = "settings", children = children }
end

return M
