-- In-scene HUD for flicker-assetpipeline ("Clayworks") — DECLARATIVE component tree.
--
-- Built to the design mock (DesignSync project 2fc44682, "Asset Pipeline.dc.html"): a TOP
-- bar (wordmark · asset name · the step rail — Workflow · Rig · Attach · Review), a floating RIGHT
-- inspector whose body is whatever the current step genuinely knows, and a BOTTOM action bar (step
-- title + hint, Back / Next). The workflow-selector overlay is the entry; there is no Load button.
--
-- `M.tree()` returns a tree of component instances; the Rust walker (flicker-widgets
-- `run_ui`) owns layout, draw, and hit-test. No pixel math / no per-frame code here.
--
-- Two-way contract lives in the node data:
--   * `action`       — a momentary event the engine reads by id (`load`, `back`, `next`).
--   * `bind`         — a Model key read for the value + written back (`show_skeleton`).
--   * `text_bind`    — a Model key whose PRE-FORMATTED string a text node shows. Rust owns
--                      all formatting (`hud_model`) — there is no printf here, which is why
--                      the rail marks and every readout arrive as finished strings.
--   * `visible_bind` — a Model key gating a subtree (`has_asset` / `on_task`).
--   * `enabled_bind` — a Model key gating a control (`back_enabled` / `next_enabled`), so
--                      the wizard cannot be walked past a stage whose input is missing.
--   * `style`/`color`— dotted paths into `ui_elements.json` (palette single-sourced in its
--                      theme.tokens — the one global UI-element definition).
-- Layout / labels all come from `UI.assetpipeline`, so the tree and the engine cannot drift.

local M = {}

-- Ergonomic constructors: each tags a node table with its component kind, so a screen
-- reads as composition — `Button{...}` inside `Row{ children }`.
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
local Button = tag("button")
local Checkbox = tag("checkbox")
local Slider = tag("slider")
local Rtt = tag("rtt")

-- A plain literal text line. `w` (optional) is the node's LAYOUT WIDTH along a Row's main
-- axis, kept DISTINCT from the font `size` — the walker's `size` doubles as main-axis length
-- (WIDTH inside a Row), so a text in a Row needs its own width or it gets a font-size-wide slot
-- and overlaps its neighbour. `align` right/centre-anchors the glyphs within that slot, so a
-- trailing item (after a `Stack{grow=1}`) sits flush to a panel's inner edge instead of drawing
-- off it. In a COLUMN callers pass neither: there `size` is the line HEIGHT, exactly as before.
local function line(P, text, size, color, font, w, align)
  return Text { text = text, size = w or size, text_size = size, color = color, font = font or "body", align = align }
end

-- A text line whose content comes from the Model, pre-formatted in Rust. `w` / `align` as `line`.
local function bound(P, key, size, color, font, w, align)
  return Text { text_bind = key, size = w or size, text_size = size, color = color, font = font or "body", align = align }
end

-- ── HEADER BAR content: wordmark · asset name · engine tag. The Workbench template owns the
-- full-bleed styled bar (`assetpipeline.header`; height/pad/gap from `P.header`), so this returns
-- just its children and the bar chrome is shared with every other bench. ──
local function header_children(P)
  local H = P.header
  return {
    -- Matches the Loomforge Bench header exactly (`label_node("Loomforge Bench", 21.0,
    -- "loomforge.title.color", Some(210.0))`): mixed case, BODY font (the walker's default —
    -- Loomforge sets no `font` prop), 21pt, and `$ink_bright`, which `col.ink` already is.
    line(P, "$ap_clayworks_bench", H.title_size, "assetpipeline.col.ink", "body", H.title_w),
    bound(P, "asset_name", H.name_size, "assetpipeline.col.ink", "display", H.name_w),
    Stack { grow = 1 },
    line(P, "$ap_clay_engine", H.meta_size, "assetpipeline.col.faint", "label", H.meta_w, "right"),
  }
end

-- ── TAB BAR content: the pipeline steps (Workflow · Rig · Attach · Review), drawn Loomforge-style.
-- NON-INTERACTIVE — the tabs carry no `action`, so a click does nothing; the footer Back / Next moves
-- the step. The current step lights via `style_bind` (the walker resolves the active/idle style path
-- from the Model), and Rust owns each label (Conform reads its role). The Workbench template owns the
-- styled strip (`assetpipeline.tab_bar`; height/pad/gap from `P.tab_bar`), so this returns just the
-- buttons. ──
local function tab_children(P)
  local T = P.tab_bar
  local tabs = {}
  -- One slot per Step::ALL entry; a step that does not apply to this asset (e.g. Attach for a prop)
  -- drops out via `tab_i_show`, so a hidden slot leaves the row's grow layout.
  for i = 0, 3 do
    tabs[#tabs + 1] = Button {
      grow = 1,
      text_bind = "tab_" .. i,
      style_bind = "tab_" .. i .. "_style",
      visible_bind = "tab_" .. i .. "_show",
      label_size = T.tab_size,
    }
  end
  return tabs
end

-- A section heading inside the inspector.
local function head(P, text)
  return line(P, text, P.inspector.head_size, "assetpipeline.col.dim", "label")
end

-- A SELECTABLE LIST ROW: a selection wash that appears only while the row is current, an
-- invisible full-width click target over it, and the bound caption on top. Kept in one helper
-- because the bone map and the attach list are the same row with different bindings.
--
-- The click target must stay enabled whichever row is selected — gating it on the selection
-- would make every OTHER row unclickable, which is the one arrangement that cannot work.
local function select_row(id, caption, selected, h, size, color_key)
  local text = { text_bind = caption, size = h, text_size = size, font = "body" }
  if color_key then
    text.color_bind = color_key
  else
    text.color = "assetpipeline.col.ink_dim"
  end
  -- The wash + click target OVERLAY inside the Stack, which places each child by its own
  -- measured box; `width_frac = 1` spans them across the full row (the Column already gave the
  -- Stack the panel's width) so the selection highlight shows and the whole row is clickable —
  -- not a `size`-wide (row-height) sliver at the left edge.
  return Stack {
    height = h,
    children = {
      Cell { visible_bind = selected, width_frac = 1, style = "assetpipeline.rowsel" },
      Button { id = id, action = id, width_frac = 1, size = h, style = "assetpipeline.rowsel_off" },
      Text(text),
    },
  }
end

-- An EXCLUSIVE choice row. The walker has no radio component and does not need one: exclusivity
-- is state, Rust already owns it, and a radio is just a button whose caption carries the selected
-- glyph. So the caption rides `text_bind` and the click rides `action` — the same two channels
-- every other control uses.
local function choice(P, key, h, size, enabled_key)
  return Button {
    id = key, action = key, text_bind = key,
    size = h, label_size = size,
    enabled_bind = enabled_key,
    style = "assetpipeline.choice",
  }
end

-- One offset slider. `min`/`max` come from the JSON so the engine and the tree cannot disagree
-- about what a full-track drag means.
local function offset(P, key, label, range, h)
  return Slider {
    id = key, bind = key, label = label,
    size = h, slider_h = P.slider.track_h,
    label_w = 96, value_w = 52, decimals = 1, plus = true,
    min = -range, max = range,
    style = "assetpipeline.slider",
  }
end

-- A scale slider: multiplicative, so it runs 0.1 → 3.0 about 1.0 and never crosses zero (a zero
-- axis flattens the mesh). Same shape as `offset`, different range and no +/- sign.
local function scale(P, key, label, h)
  return Slider {
    id = key, bind = key, label = label,
    size = h, slider_h = P.slider.track_h,
    label_w = 96, value_w = 52, decimals = 2,
    min = 0.1, max = 3.0,
    style = "assetpipeline.slider",
  }
end

-- ── STAGE 0 · TASK — the IMPORT workflow selector, the bench's first page. Built from the STANDARD
-- Prism UI: the `window` template supplies the CHROME (chiseled frame, glowing CORNER RUNES, title
-- bar) and the `option_grid` template supplies the reusable SELECTOR FIELD (heading · sunk well of a
-- flowing card grid · hint). This scene owns each workflow TILE and the WORKFLOWS list; the field
-- flows however many there are into rows — an ARBITRARY, growing catalogue, not a fixed set. A card
-- DECLARES its AssetClass (+ Prop sub-type) in Rust AND opens the folder dialog in one click.
--
-- One workflow tile = a Stack overlaying the card SURFACE (`assetpipeline.panel`), a CONTENT column
-- (rune icon slot → title → italic flavour → format tag), and a transparent CLICK+HOVER button
-- (`assetpipeline.card`: idle shows the $edge3 edge; hover lights the DS 'selected ring'). The rune
-- sits centred in an inset WELL. The flavour line WRAPS to the card width (`wrap = true` over a
-- reserved height) — the only change from the original tile; sizes live in `P.import`.
local function import_card(P, wf)
  local M = P.import
  return Stack {
    width = M.card_size, height = M.card_size,   -- square, fixed
    children = {
      -- Card surface (drawn first, under everything).
      Cell { width_frac = 1, height_frac = 1, style = "assetpipeline.panel" },
      -- Content: rune icon slot, title, italic flavour (WRAPS), format tag.
      Cell {
        width_frac = 1, height_frac = 1, pad = M.pad, gap = M.gap,
        children = {
          -- Icon slot — an inset well with the rune centred inside it.
          Stack {
            height = M.slot_h, width_frac = 1,
            children = {
              Cell { width_frac = 1, height_frac = 1, style = "assetpipeline.well" },
              Text {
                text = wf.rune, font = "rune", align = "center", anchor = "center",
                width_frac = 1, height = M.rune_size, text_size = M.rune_size,
                color = "assetpipeline.col.sapphire",
              },
            },
          },
          Text { text = wf.title, font = "display", size = M.title_h, text_size = M.title_size, color = "assetpipeline.col.ink" },
          -- Italic flavour, WRAPPED to the card width over a reserved (multi-line) height.
          Text { text = wf.desc, font = "body", italic = true, wrap = true, size = M.desc_h, text_size = M.desc_size, color = "assetpipeline.col.dim" },
          -- Push the format tag to the card's bottom edge (the design's margin-top:auto).
          Stack { grow = 1 },
          Text { text = wf.tag, font = "label", tracking = 0.16, size = M.tag_h, text_size = M.tag_size, color = "assetpipeline.col.faint" },
        },
      },
      -- Click + hover layer: pure affordance; hover styles supply the DS 'selected ring'.
      Button { id = wf.id, action = wf.id, width_frac = 1, height_frac = 1, style = "assetpipeline.card" },
    },
  }
end

-- The workflow catalog — an ARBITRARY, growing list. Adding a workflow = one row HERE (+ its Rust
-- class/sub-type mapping in `update()`); the `option_grid` field flows however many there are.
local WORKFLOWS = {
  { id = "import_character", rune = "\u{16D7}", title = "$ap_character",
    desc = "$ap_a_soul_given_form_mesh_skeleton_and_stat", tag = "$ap_fbx_glb_rigged" },
  { id = "import_accessory", rune = "\u{16B7}", title = "$ap_accessory",
    desc = "$ap_worn_things_blades_circlets_loomsilk_bou", tag = "$ap_fbx_glb_attach_point" },
  { id = "import_prop", rune = "\u{16A6}", title = "$ap_prop",
    desc = "$ap_objects_of_the_world_answering_to_no_bea", tag = "$ap_fbx_glb_static" },
  { id = "import_animation", rune = "\u{16D6}", title = "$ap_animation",
    desc = "$ap_motion_inscribed_upon_a_skeleton_already", tag = "$ap_fbx_bvh_clip" },
}

-- Build one option tile per workflow — the `cards` slot the `option_grid` template arranges into
-- its flowing grid. The number is arbitrary; add a WORKFLOWS entry and it appears.
local function workflow_cards(P)
  local cards = {}
  for _, wf in ipairs(WORKFLOWS) do
    cards[#cards + 1] = import_card(P, wf)
  end
  return cards
end

-- The Import selector is a CENTERED WINDOW over the whole bench (a dim scrim + the standard `window`
-- template, itself a preset over `frame`) — mounted at the PAGE ROOT (see `M.tree`), gated `on_task`,
-- at a high layer over the quad viewport. RESPONSIVE (`w_frac`/`h_frac`, ~0.82) with NO close button
-- (a workflow must be chosen). The window builds the "$ap_import" title bar + chiseled border + corner
-- runes; its `content` slot is the reusable `option_grid` selector. No hand-authored chrome.
local function task_overlay(P)
  local M = P.import
  return Stack {
    visible_bind = "on_task",
    anchor = "top_left", width_frac = 1.0, height_frac = 1.0, layer = 9,
    children = {
      -- Dim scrim over the (dimmed) bench behind.
      Cell { anchor = "top_left", width_frac = 1.0, height_frac = 1.0, style = "assetpipeline.import_scrim" },
      -- The standard Prism window (a preset over `frame`): "$ap_import" title bar + border + corner runes.
      {
        template = "window",
        title = "$ap_import", title_size = M.head_size,
        w_frac = M.win_frac, h_frac = M.win_frac,
        has_close = false,
        slots = {
          -- CONTENT: the reusable SELECTOR FIELD — heading · a sunk well of the flowing workflow-card
          -- grid · a bottom hint. `cols`-per-row; grows to fill the window's content region.
          content = {
            {
              template = "option_grid",
              cols = M.cols, gap = M.win_gap, grid_gap = M.grid_gap, well_pad = M.grid_pad,
              well_style = "assetpipeline.well",
              heading = "$ap_choose_a_workflow", heading_size = M.sec_size, heading_color = "assetpipeline.col.bronze",
              subtitle = "$ap_what_manner_of_thing_will_you_weave_into",
              subtitle_size = M.subtitle_size, subtitle_color = "assetpipeline.col.dim",
              hint = "$ap_select_a_workflow_to_begin", hint_size = M.hint_size, hint_color = "assetpipeline.col.dim",
              slots = { cards = workflow_cards(P) },
            },
          },
        },
      },
    },
  }
end

-- ── PIECE PICKER — WHICH riggable mesh to import. Most source folders hold several: a weapon set
-- is four or five pieces, an outfit folder is tops / pants / gloves / shoes. So the folder offers a
-- choice instead of being refused — pick a piece, import it, come back for the next. Lives INLINE on
-- the rig stage, shown only when the folder holds a choice (`on_pick`). ──
local function pick_stage(P)
  local A, I, R = P.attach, P.inspector, P.rig
  local rows = {}
  for i = 0, 5 do -- PICK_ROWS — matched to Rust
    rows[#rows + 1] =
      select_row("pick_sel_" .. i, "pick_" .. i, "pick_" .. i .. "_on", A.row_h, A.row_size)
  end
  return Cell {
    visible_bind = "on_pick",
    gap = I.sub_gap,
    children = {
      head(P, "$ap_meshes_in_this_folder"),
      Cell { style = "assetpipeline.well", pad = 4, children = { Cell { gap = 0, children = rows } } },
      Row {
        gap = 6, height = R.page_h,
        children = {
          Button { id = "pick_prev", action = "pick_prev", label = "▲", size = R.page_h, label_size = R.page_size, enabled_bind = "pick_prev_enabled", style = "assetpipeline.choice" },
          Button { id = "pick_next", action = "pick_next", label = "▼", size = R.page_h, label_size = R.page_size, enabled_bind = "pick_next_enabled", style = "assetpipeline.choice" },
          bound(P, "pick_page", R.page_size, "assetpipeline.col.faint", "body", R.page_w),
        },
      },
    },
  }
end

-- ── RIG CONFORM — the paged bone map + the selected bone's offsets. ──
local function conform_stage(P)
  local R, I = P.rig, P.inspector
  local rows = {}
  for i = 0, R.rows - 1 do
    -- The row's caption AND colour are both bound: the caption carries the name + tag, the
    -- colour carries the conform provenance (ok / review / auto) from the one shared key.
    rows[#rows + 1] = select_row(
      "bone_sel_" .. i, "bone_" .. i, "bone_" .. i .. "_on",
      R.row_h, R.row_size, "bone_" .. i .. "_color"
    )
  end
  return Cell {
    visible_bind = "on_conform_skeleton",
    gap = I.sub_gap,
    children = {
      bound(P, "rig_headline", I.head_size, "assetpipeline.col.ink", "body"),
      bound(P, "rig_legend", R.page_size, "assetpipeline.col.dim", "body"),
      head(P, "$ap_bone_map"),
      Cell { style = "assetpipeline.well", pad = 4, children = { Cell { gap = 0, children = rows } } },
      Row {
        gap = 6, height = R.page_h,
        children = {
          Button { id = "bone_prev", action = "bone_prev", label = "▲", size = R.page_h, label_size = R.page_size, enabled_bind = "bone_prev_enabled", style = "assetpipeline.choice" },
          Button { id = "bone_next", action = "bone_next", label = "▼", size = R.page_h, label_size = R.page_size, enabled_bind = "bone_next_enabled", style = "assetpipeline.choice" },
          bound(P, "bone_page", R.page_size, "assetpipeline.col.faint", "body", R.page_w),
        },
      },
      bound(P, "rig_sel", I.head_size, "assetpipeline.col.ink", "body"),
      -- The in-scene gizmo's mode toggle. Exclusive choice, Rust owns the selection (like the
      -- class rows), so each button's caption rides `text_bind` carrying the ◉/○ glyph + label.
      -- Translate is the only mode that drags in slice 1; Rotate/Scale draw their handles but are
      -- inert. Stacked full-width via the proven `choice` helper — no new JSON params.
      head(P, "$ap_gizmo_mode"),
      Cell { gap = 2, children = {
        choice(P, "mode_translate", R.btn_h, R.btn_size),
        choice(P, "mode_rotate", R.btn_h, R.btn_size),
        choice(P, "mode_scale", R.btn_h, R.btn_size),
      } },
      -- Ortho-view joint moves REPOSITION the rest skeleton (mesh stays); mirror to the _l/_r twin by
      -- default. Bake Skin re-weights the mesh to the corrected bones (the Meshy-skin replacement).
      Checkbox { id = "mirror", bind = "mirror", label = "$ap_mirror_joints", size = R.btn_h, label_size = R.btn_size, style = "assetpipeline.checkbox" },
      Button { id = "bake_skin", action = "bake_skin", label = "$ap_bake_skin", size = R.btn_h, label_size = R.btn_size, style = "modal.buttons.variants.primary" },
      offset(P, "off_x", "$ap_translate_x", R.off_range, R.slider_h),
      offset(P, "off_y", "$ap_translate_y", R.off_range, R.slider_h),
      offset(P, "off_z", "$ap_translate_z", R.off_range, R.slider_h),
      offset(P, "off_roll", "$ap_roll", R.roll_range, R.slider_h),
      Row {
        gap = 6, height = R.btn_h,
        children = {
          Button { id = "bone_reset", action = "bone_reset", label = "$ap_reset_bone", size = R.btn_w, label_size = R.btn_size, style = "modal.buttons.variants.secondary" },
        },
      },
    },
  }
end

-- ── STAGE 4 (CHARACTER) · ATTACH — the six socket rows + the selected point's offsets. Shown for a
-- Skin; a prop/garment sees `fit_stage` below instead (both gated under `on_attach`). ──
local function attach_stage(P)
  local A, I = P.attach, P.inspector
  local rows = {}
  for i = 0, A.rows - 1 do
    rows[#rows + 1] = select_row(
      "att_sel_" .. i, "att_" .. i, "att_" .. i .. "_on", A.row_h, A.row_size
    )
  end
  return Cell {
    visible_bind = "on_attach_char",
    gap = I.sub_gap,
    children = {
      head(P, "$ap_rig_attach_points"),
      Cell { style = "assetpipeline.well", pad = 4, children = { Cell { gap = 0, children = rows } } },
      bound(P, "att_sel", I.head_size, "assetpipeline.col.ink", "body"),
      offset(P, "att_x", "$ap_off_x", A.off_range, A.slider_h),
      offset(P, "att_y", "$ap_off_y", A.off_range, A.slider_h),
      offset(P, "att_z", "$ap_off_z", A.off_range, A.slider_h),
    },
  }
end

-- ── STAGE 4 (NON-character) · FIT — the mount-socket picker + offset/rotation/scale sliders. A
-- prop or garment mounts to ONE socket (unlike the character's six points); this is the human-in-
-- the-loop point where the user PICKS that socket and tunes the placement, verifying it against the
-- viewport before Commit bakes exactly what they approved. ──
local function fit_stage(P)
  local A, I, R = P.attach, P.inspector, P.rig
  local socks = {}
  for i = 0, 5 do -- SOCK_ROWS — matched to Rust SOCKET_ROWS
    socks[#socks + 1] =
      select_row("sock_sel_" .. i, "sock_" .. i, "sock_" .. i .. "_on", A.row_h, A.row_size)
  end
  return Cell {
    -- The prop/garment's RIG PAGE. Its rig is not a skeleton conform — it is this attach binding
    -- (mount socket + placement), so it lives on Conform under the Mount role.
    visible_bind = "on_conform_mount",
    gap = I.sub_gap,
    children = {
      head(P, "$ap_mount_socket"),
      Cell { style = "assetpipeline.well", pad = 4, children = { Cell { gap = 0, children = socks } } },
      Row {
        gap = 6, height = R.page_h,
        children = {
          Button { id = "sock_prev", action = "sock_prev", label = "▲", size = R.page_h, label_size = R.page_size, enabled_bind = "sock_prev_enabled", style = "assetpipeline.choice" },
          Button { id = "sock_next", action = "sock_next", label = "▼", size = R.page_h, label_size = R.page_size, enabled_bind = "sock_next_enabled", style = "assetpipeline.choice" },
          bound(P, "sock_page", R.page_size, "assetpipeline.col.faint", "body", R.page_w),
        },
      },
      bound(P, "fit_socket", I.head_size, "assetpipeline.col.ink", "body"),
      head(P, "$ap_offset_cm"),
      offset(P, "fit_ox", "$ap_offset_x", A.off_range, A.slider_h),
      offset(P, "fit_oy", "$ap_offset_y", A.off_range, A.slider_h),
      offset(P, "fit_oz", "$ap_offset_z", A.off_range, A.slider_h),
      head(P, "$ap_rotate_deg"),
      offset(P, "fit_rx", "$ap_rotate_x", 180, A.slider_h),
      offset(P, "fit_ry", "$ap_rotate_y", 180, A.slider_h),
      offset(P, "fit_rz", "$ap_rotate_z", 180, A.slider_h),
      head(P, "$ap_scale"),
      scale(P, "fit_sx", "$ap_scale_x", A.slider_h),
      scale(P, "fit_sy", "$ap_scale_y", A.slider_h),
      scale(P, "fit_sz", "$ap_scale_z", A.slider_h),
      -- Scale-ALL rides on top of the three: resize without reshaping.
      scale(P, "fit_scale", "$ap_all", A.slider_h),
    },
  }
end

-- ── STAGE 3 (ANIMATION) · CLIPS — the third Conform role, and deliberately a STATEMENT rather
-- than a panel. Clip import/retarget is not routed through the editor yet; showing sliders that
-- addressed nothing is precisely the dead-page trap the role dispatch exists to close, so this
-- role says so plainly and `can_advance` stops the wizard here. ──
local function clip_stage(P)
  local I, R = P.inspector, P.rig
  return Cell {
    visible_bind = "on_conform_clip",
    gap = I.sub_gap,
    children = {
      head(P, "$ap_clips"),
      line(P, "$ap_clip_import_is_not_wired_into_the_editor", I.head_size, "assetpipeline.col.ink", "body"),
      line(P, "$ap_an_animation_set_cannot_be_baked_from_he", R.page_size, "assetpipeline.col.dim", "body"),
    },
  }
end

-- ── STAGE 5 · REVIEW — the engine requirements + the commit handoff. ──
local function review_stage(P)
  local V, I = P.review, P.inspector
  local rows = {}
  for i = 0, V.rows - 1 do
    rows[#rows + 1] = Text {
      text_bind = "req_" .. i,
      color_bind = "req_" .. i .. "_color",
      size = V.row_h, text_size = V.row_size, font = "body",
    }
  end
  return Cell {
    visible_bind = "on_review",
    gap = I.sub_gap,
    children = {
      head(P, "$ap_engine_requirements"),
      Cell { gap = 0, children = rows },
      Button {
        id = "commit", action = "commit", text_bind = "commit_label",
        size = V.commit_h, label_size = V.commit_size,
        enabled_bind = "commit_enabled",
        style = "modal.buttons.variants.primary",
      },
      -- Closes the loop for a multi-piece folder: back to the mesh picker for the next piece,
      -- instead of walking Back through five stages. Only shown once THIS piece is committed.
      Button {
        id = "next_piece", action = "next_piece", label = "$ap_import_next_piece",
        size = V.commit_h, label_size = V.commit_size,
        visible_bind = "has_committed",
        style = "modal.buttons.variants.secondary",
      },
    },
  }
end

-- ── INSPECTOR: the floating right panel. Its body is `INSPECTOR_LINES` bound rows (the EVIDENCE
-- behind the current step), under the controls that step owns. ──
local function inspector(P)
  local I = P.inspector
  local body = {}
  for i = 0, 7 do
    body[#body + 1] = Text {
      text_bind = "insp_" .. i,
      size = I.line_h,
      text_size = I.line_size,
      font = "body",
      color = "assetpipeline.col.ink_dim",
    }
  end
  return Cell {
    width = I.width,
    pad = I.pad,
    gap = I.gap,
    style = "assetpipeline.panel",
    children = {
      Row {
        gap = 8,
        height = I.title_size,
        children = {
          bound(P, "insp_title", I.title_size, "assetpipeline.col.ink", "display", I.title_w),
          Stack { grow = 1 },
          bound(P, "insp_badge", I.badge_size, "assetpipeline.col.bronze", "label", I.badge_w, "right"),
        },
      },
      Checkbox {
        id = "show_skeleton",
        bind = "show_skeleton",
        label = "$ap_skeleton",
        size = I.line_h,
        label_size = I.line_size,
        style = "assetpipeline.checkbox",
      },
      -- The clay Golem behind the piece being fitted. Pre-loaded with the scene, so this is a
      -- pure visibility flip — a prop/outfit is placed against a BODY, not a stick figure.
      Checkbox {
        id = "show_base",
        bind = "show_base",
        label = "$ap_reference_body",
        size = I.line_h,
        label_size = I.line_size,
        style = "assetpipeline.checkbox",
      },
      -- The auto-fit COLLISION overlay: per-bone capsules + leaf "joint ball" spheres over the posed
      -- rig (WS-D), wired to `flicker-mechanics`'s auto-fit. Off by default — a diagnostic view.
      Checkbox {
        id = "show_collision",
        bind = "show_collision",
        label = "$ap_collision",
        size = I.line_h,
        label_size = I.line_size,
        style = "assetpipeline.checkbox",
      },
      -- Per-view flips moved ONTO the panels: click an ortho quad's corner label (LEFT / TOP /
      -- FRONT) to swap it to its opposite side; the label text updates to report which side shows.
      -- Handled in the scene against `QuadGrid::flipped`, so there is no HUD checkbox for it.
      -- (task_stage moved OUT of the rail — it is a centered Page-root overlay, see `task_overlay`.)
      -- The inline piece picker (shown only for a multi-mesh folder), then the three Conform ROLES
      -- and the character-only Attach page.
      pick_stage(P),
      conform_stage(P),
      fit_stage(P),
      clip_stage(P),
      attach_stage(P),
      review_stage(P),
      Cell { gap = I.gap, children = body },
    },
  }
end

-- ── WORK AREA viewport: the framed RTT holder (the 2×2 editor viewport). The holder is a `stage`
-- node — it reserves its inset rect and the scene renders the QuadGrid INTO it, so the four views sit
-- inside the frame. `grow = 1` gives the holder whatever width the fixed rail leaves. The Workbench
-- template owns the work-area Row (`grow = 1`; pad/gap from `P.body`) and lays this `viewport` slot
-- beside the `rail` slot (the `inspector` panel, first-class beside the views — not floating). ──
local function work_viewport(P)
  return Rtt { id = "editor_quad", source = "editor_quad", grow = 1, style = "assetpipeline.holder" }
end

-- ── FOOTER content: step title + hint on the left, the actions on the right. The picker is now
-- reached by clicking an Import card (not a footer button); Back / Next gate on `can_advance`. The
-- Workbench template owns
-- the full-bleed bottom bar (`assetpipeline.header`; height/pad/gap from `P.footer`) and its inner
-- button Row (gap `F.gap`, height `F.btn_h`), so this returns just that Row's children — the
-- controls are unchanged. ──
local function footer_children(P)
  local F = P.footer
  return {
    Cell {
      grow = 1,
      children = {
        bound(P, "step_title", F.title_size, "assetpipeline.col.ink", "display"),
        bound(P, "step_hint", F.hint_size, "assetpipeline.col.ink_dim", "body"),
      },
    },
    Button {
      id = "back", action = "back", label = "$ap_back",
      size = F.back_w, label_size = F.btn_size,
      enabled_bind = "back_enabled",
      style = "modal.buttons.variants.secondary",
    },
    Button {
      id = "next", action = "next", text_bind = "next_label",
      size = F.next_w, label_size = F.btn_size,
      enabled_bind = "next_enabled",
      style = "modal.buttons.variants.primary",
    },
  }
end

function M.tree()
  if not UI or not UI.assetpipeline then
    -- The degenerate no-UI root keeps the declared binding (S9).
    return Screen { id = "assetpipeline", on_menu = "pause_open" }
  end
  local P = UI.assetpipeline
  local H, T, B, F = P.header, P.tab_bar, P.body, P.footer
  -- The `workbench` template (flicker-widgets) owns the full-screen skeleton — a full-bleed header
  -- bar · a tab strip · a work area (viewport beside a fixed rail) · a full-bleed footer bar — the
  -- SAME structure this scene built by hand. The bar heights/pads/gaps + styles pass through from
  -- `UI.assetpipeline`, and each region's CONTENT is a slot, so the render is unchanged while the
  -- arrangement becomes a reusable, shared template.
  return Screen {
    id = "assetpipeline",
    -- The screen's input DECLARATION (S9): Menu (Esc / pad Start) fires
    -- `pause_open`; the scene maps the fired name onto its pause push.
    on_menu = "pause_open",
    children = {
      {
        template = "workbench",
        header_h = H.h, header_pad = H.pad, header_gap = H.gap, header_style = "assetpipeline.header",
        tab_h = T.h, tab_pad = T.pad, tab_gap = T.gap, tab_style = "assetpipeline.tab_bar",
        body_pad = B.margin, body_gap = B.gap,
        footer_h = F.height, footer_pad = F.pad, footer_gap = F.gap, footer_btn_h = F.btn_h,
        footer_style = "assetpipeline.header",
        slots = {
          header = header_children(P),
          tabs = tab_children(P),
          viewport = { work_viewport(P) },
          rail = { inspector(P) },
          footer = footer_children(P),
        },
      },
      -- The Import workflow panel: a centered window OVER the whole bench on the Task step
      -- (Page overlays its children by anchor; this one is full-screen + gated `on_task`).
      task_overlay(P),
    },
  }
end

return M
