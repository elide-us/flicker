-- In-scene HUD for flicker-assetpipeline ("Kilnworks") — DECLARATIVE component tree.
--
-- Built to the design mock (DesignSync project 2fc44682, "Asset Pipeline.dc.html"): a TOP
-- bar (wordmark · asset name · the six-step rail), a floating RIGHT inspector whose body
-- is whatever the current step genuinely knows, and a BOTTOM action bar (step title +
-- hint, Back / Next, and the Load button).
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
--   * `visible_bind` — a Model key gating a subtree (`has_asset` / `no_asset`).
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
local Page = tag("page")
local Stack = tag("stack")
local Column = tag("column")
local Row = tag("row")
local Panel = tag("panel")
local Text = tag("text")
local Button = tag("button")
local Checkbox = tag("checkbox")
local Slider = tag("slider")
local Stage = tag("stage")

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
    line(P, "Kilnworks Bench", H.title_size, "assetpipeline.col.ink", "body", H.title_w),
    bound(P, "asset_name", H.name_size, "assetpipeline.col.ink", "display", H.name_w),
    Stack { grow = 1 },
    line(P, "CLAY ENGINE", H.meta_size, "assetpipeline.col.faint", "label", H.meta_w, "right"),
  }
end

-- ── TAB BAR content: the six pipeline steps, drawn Loomforge-style. NON-INTERACTIVE — the tabs
-- carry no `action`, so a click does nothing; the footer Back / Next moves the step. The current
-- step lights via `style_bind` (the walker resolves the active/idle style path from the Model), and
-- Rust owns each label (Conform reads its role). The Workbench template owns the styled strip
-- (`assetpipeline.tab_bar`; height/pad/gap from `P.tab_bar`), so this returns just the six buttons. ──
local function tab_children(P)
  local T = P.tab_bar
  local tabs = {}
  for i = 0, 5 do
    tabs[#tabs + 1] = Button {
      grow = 1,
      text_bind = "tab_" .. i,
      style_bind = "tab_" .. i .. "_style",
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
      Panel { visible_bind = selected, width_frac = 1, style = "assetpipeline.rowsel" },
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

-- ── STAGE 0 · LOAD PICKER — WHICH riggable mesh to import. Most source folders hold several: a
-- weapon set is four or five pieces, an outfit folder is tops / pants / gloves / shoes. So the
-- folder offers a choice instead of being refused — pick a piece, import it, come back for the
-- next. Shown only when there IS a choice (`on_load_pick`). ──
local function pick_stage(P)
  local A, I, R = P.attach, P.inspector, P.rig
  local rows = {}
  for i = 0, 5 do -- PICK_ROWS — matched to Rust
    rows[#rows + 1] =
      select_row("pick_sel_" .. i, "pick_" .. i, "pick_" .. i .. "_on", A.row_h, A.row_size)
  end
  return Column {
    visible_bind = "on_load_pick",
    gap = I.sub_gap,
    children = {
      head(P, "MESHES IN THIS FOLDER"),
      Panel { style = "assetpipeline.well", pad = 4, children = { Column { gap = 0, children = rows } } },
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

-- ── STAGE 1 · ANALYZE — the GROUND-RECKONING control. World reckoning is Z-up (ground = XY), but
-- sources arrive however they were authored, so this stands the asset up in 90° steps before
-- anything downstream reads its axes. Exact quarter-turns: four presses return it precisely. ──
local function orient_stage(P)
  local I, R = P.inspector, P.rig
  local function turn(axis, label)
    return Button {
      id = "orient_" .. axis, action = "orient_" .. axis, label = label,
      size = R.page_w, label_size = R.btn_size, style = "assetpipeline.choice",
    }
  end
  return Column {
    visible_bind = "on_analyze",
    gap = I.sub_gap,
    children = {
      head(P, "GROUND RECKONING · 90° STEPS"),
      bound(P, "orient_label", I.head_size, "assetpipeline.col.ink", "body"),
      Row {
        gap = 6, height = R.btn_h,
        children = { turn("x", "ROT X"), turn("y", "ROT Y"), turn("z", "ROT Z") },
      },
    },
  }
end

-- ── STAGE 2 · CLASSIFY — the detected card, the class choice, the prop sub-type block. ──
local function classify_stage(P)
  local C, I = P.classify, P.inspector
  local cls, sub = {}, {}
  for i = 0, 2 do
    cls[#cls + 1] = choice(P, "cls_" .. i, C.row_h, C.row_size)
  end
  for i = 0, 3 do
    -- The design dims the whole sub-type block unless the class is Prop; `enabled_bind` is
    -- exactly that gate, and the engine ignores the action too, so the two cannot disagree.
    sub[#sub + 1] = choice(P, "sub_" .. i, C.row_h, C.row_size, "cls_is_prop")
  end
  return Column {
    visible_bind = "on_classify",
    gap = I.sub_gap,
    children = {
      Panel {
        height = C.card_h, pad = 8, style = "assetpipeline.well",
        children = {
          Row {
            gap = 8,
            height = C.card_size,
            children = {
              bound(P, "cls_detected", C.card_size, "assetpipeline.col.ink", "label", C.card_w),
              Stack { grow = 1 },
              bound(P, "cls_confidence", C.conf_size, "assetpipeline.map.ok", "body", C.conf_w, "right"),
            },
          },
        },
      },
      head(P, "OVERRIDE CLASSIFICATION"),
      Column { gap = 2, children = cls },
      head(P, "PROP SUB-TYPE"),
      Column { gap = 2, pad = C.indent, children = sub },
    },
  }
end

-- ── STAGE 3 · RIG CONFORM — the paged bone map + the selected bone's offsets. ──
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
  return Column {
    visible_bind = "on_conform_skeleton",
    gap = I.sub_gap,
    children = {
      bound(P, "rig_headline", I.head_size, "assetpipeline.col.ink", "body"),
      bound(P, "rig_legend", R.page_size, "assetpipeline.col.dim", "body"),
      head(P, "BONE MAP"),
      Panel { style = "assetpipeline.well", pad = 4, children = { Column { gap = 0, children = rows } } },
      Row {
        gap = 6, height = R.page_h,
        children = {
          Button { id = "bone_prev", action = "bone_prev", label = "▲", size = R.page_h, label_size = R.page_size, enabled_bind = "bone_prev_enabled", style = "assetpipeline.choice" },
          Button { id = "bone_next", action = "bone_next", label = "▼", size = R.page_h, label_size = R.page_size, enabled_bind = "bone_next_enabled", style = "assetpipeline.choice" },
          bound(P, "bone_page", R.page_size, "assetpipeline.col.faint", "body", R.page_w),
        },
      },
      bound(P, "rig_sel", I.head_size, "assetpipeline.col.ink", "body"),
      offset(P, "off_x", "Translate X", R.off_range, R.slider_h),
      offset(P, "off_y", "Translate Y", R.off_range, R.slider_h),
      offset(P, "off_z", "Translate Z", R.off_range, R.slider_h),
      offset(P, "off_roll", "Roll", R.roll_range, R.slider_h),
      Row {
        gap = 6, height = R.btn_h,
        children = {
          Button { id = "bone_reset", action = "bone_reset", label = "RESET BONE", size = R.btn_w, label_size = R.btn_size, style = "modal.buttons.variants.secondary" },
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
  return Column {
    visible_bind = "on_attach_char",
    gap = I.sub_gap,
    children = {
      head(P, "RIG ATTACH POINTS"),
      Panel { style = "assetpipeline.well", pad = 4, children = { Column { gap = 0, children = rows } } },
      bound(P, "att_sel", I.head_size, "assetpipeline.col.ink", "body"),
      offset(P, "att_x", "Off X", A.off_range, A.slider_h),
      offset(P, "att_y", "Off Y", A.off_range, A.slider_h),
      offset(P, "att_z", "Off Z", A.off_range, A.slider_h),
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
  return Column {
    -- The prop/garment's RIG PAGE. Its rig is not a skeleton conform — it is this attach binding
    -- (mount socket + placement), so it lives on Conform under the Mount role.
    visible_bind = "on_conform_mount",
    gap = I.sub_gap,
    children = {
      head(P, "MOUNT SOCKET"),
      Panel { style = "assetpipeline.well", pad = 4, children = { Column { gap = 0, children = socks } } },
      Row {
        gap = 6, height = R.page_h,
        children = {
          Button { id = "sock_prev", action = "sock_prev", label = "▲", size = R.page_h, label_size = R.page_size, enabled_bind = "sock_prev_enabled", style = "assetpipeline.choice" },
          Button { id = "sock_next", action = "sock_next", label = "▼", size = R.page_h, label_size = R.page_size, enabled_bind = "sock_next_enabled", style = "assetpipeline.choice" },
          bound(P, "sock_page", R.page_size, "assetpipeline.col.faint", "body", R.page_w),
        },
      },
      bound(P, "fit_socket", I.head_size, "assetpipeline.col.ink", "body"),
      head(P, "OFFSET · cm"),
      offset(P, "fit_ox", "Offset X", A.off_range, A.slider_h),
      offset(P, "fit_oy", "Offset Y", A.off_range, A.slider_h),
      offset(P, "fit_oz", "Offset Z", A.off_range, A.slider_h),
      head(P, "ROTATE · deg"),
      offset(P, "fit_rx", "Rotate X", 180, A.slider_h),
      offset(P, "fit_ry", "Rotate Y", 180, A.slider_h),
      offset(P, "fit_rz", "Rotate Z", 180, A.slider_h),
      head(P, "SCALE"),
      scale(P, "fit_sx", "Scale X", A.slider_h),
      scale(P, "fit_sy", "Scale Y", A.slider_h),
      scale(P, "fit_sz", "Scale Z", A.slider_h),
      -- Scale-ALL rides on top of the three: resize without reshaping.
      scale(P, "fit_scale", "All", A.slider_h),
    },
  }
end

-- ── STAGE 3 (ANIMATION) · CLIPS — the third Conform role, and deliberately a STATEMENT rather
-- than a panel. Clip import/retarget is not routed through the editor yet; showing sliders that
-- addressed nothing is precisely the dead-page trap the role dispatch exists to close, so this
-- role says so plainly and `can_advance` stops the wizard here. ──
local function clip_stage(P)
  local I, R = P.inspector, P.rig
  return Column {
    visible_bind = "on_conform_clip",
    gap = I.sub_gap,
    children = {
      head(P, "CLIPS"),
      line(P, "Clip import is not wired into the editor yet.", I.head_size, "assetpipeline.col.ink", "body"),
      line(P, "An animation set cannot be baked from here.", R.page_size, "assetpipeline.col.dim", "body"),
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
  return Column {
    visible_bind = "on_review",
    gap = I.sub_gap,
    children = {
      head(P, "ENGINE REQUIREMENTS"),
      Column { gap = 0, children = rows },
      Button {
        id = "commit", action = "commit", text_bind = "commit_label",
        size = V.commit_h, label_size = V.commit_size,
        enabled_bind = "commit_enabled",
        style = "modal.buttons.variants.primary",
      },
      -- Closes the loop for a multi-piece folder: back to the mesh picker for the next piece,
      -- instead of walking Back through five stages. Only shown once THIS piece is committed.
      Button {
        id = "next_piece", action = "next_piece", label = "IMPORT NEXT PIECE",
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
  return Panel {
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
        label = "Skeleton",
        size = I.line_h,
        label_size = I.line_size,
        style = "assetpipeline.checkbox",
      },
      -- The clay Golem behind the piece being fitted. Pre-loaded with the scene, so this is a
      -- pure visibility flip — a prop/outfit is placed against a BODY, not a stick figure.
      Checkbox {
        id = "show_base",
        bind = "show_base",
        label = "Reference body",
        size = I.line_h,
        label_size = I.line_size,
        style = "assetpipeline.checkbox",
      },
      -- The auto-fit COLLISION overlay: per-bone capsules + leaf "joint ball" spheres over the posed
      -- rig (WS-D), wired to `flicker-mechanics`'s auto-fit. Off by default — a diagnostic view.
      Checkbox {
        id = "show_collision",
        bind = "show_collision",
        label = "Collision",
        size = I.line_h,
        label_size = I.line_size,
        style = "assetpipeline.checkbox",
      },
      -- Per-view flips moved ONTO the panels: click an ortho quad's corner label (LEFT / TOP /
      -- FRONT) to swap it to its opposite side; the label text updates to report which side shows.
      -- Handled in the scene against `QuadGrid::flipped`, so there is no HUD checkbox for it.
      pick_stage(P),
      orient_stage(P),
      classify_stage(P),
      -- The three Conform ROLES, then the character-only Attach page.
      conform_stage(P),
      fit_stage(P),
      clip_stage(P),
      attach_stage(P),
      review_stage(P),
      Column { gap = I.gap, children = body },
    },
  }
end

-- ── WORK AREA viewport: the framed RTT holder (the 2×2 editor viewport). The holder is a `stage`
-- node — it reserves its inset rect and the scene renders the QuadGrid INTO it, so the four views sit
-- inside the frame. `grow = 1` gives the holder whatever width the fixed rail leaves. The Workbench
-- template owns the work-area Row (`grow = 1`; pad/gap from `P.body`) and lays this `viewport` slot
-- beside the `rail` slot (the `inspector` panel, first-class beside the views — not floating). ──
local function work_viewport(P)
  return Stage { id = "editor_quad", source = "editor_quad", grow = 1, style = "assetpipeline.holder" }
end

-- ── FOOTER content: step title + hint on the left, the actions on the right. `Load` shows only
-- with no asset open; Back / Next gate on the engine's `can_advance`. The Workbench template owns
-- the full-bleed bottom bar (`assetpipeline.header`; height/pad/gap from `P.footer`) and its inner
-- button Row (gap `F.gap`, height `F.btn_h`), so this returns just that Row's children — the
-- controls are unchanged. ──
local function footer_children(P)
  local F = P.footer
  return {
    Column {
      grow = 1,
      children = {
        bound(P, "step_title", F.title_size, "assetpipeline.col.ink", "display"),
        bound(P, "step_hint", F.hint_size, "assetpipeline.col.ink_dim", "body"),
      },
    },
    Button {
      id = "load", action = "load", label = "LOAD ASSET",
      size = F.load_w, label_size = F.btn_size,
      visible_bind = "no_asset",
      style = "modal.buttons.variants.primary",
    },
    Button {
      id = "back", action = "back", label = "BACK",
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
    return Page { id = "assetpipeline" }
  end
  local P = UI.assetpipeline
  local H, T, B, F = P.header, P.tab_bar, P.body, P.footer
  -- The `workbench` template (flicker-widgets) owns the full-screen skeleton — a full-bleed header
  -- bar · a tab strip · a work area (viewport beside a fixed rail) · a full-bleed footer bar — the
  -- SAME structure this scene built by hand. The bar heights/pads/gaps + styles pass through from
  -- `UI.assetpipeline`, and each region's CONTENT is a slot, so the render is unchanged while the
  -- arrangement becomes a reusable, shared template.
  return Page {
    id = "assetpipeline",
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
    },
  }
end

return M
