-- Prism carved-stone modal — the shared menu / pause ("Sanctum") / display-confirm
-- component, driven by ui_elements.json (`UI.modal` style + `UI.screens[Model.screen]`
-- instance). One script, three screens: they share the drawn panel + button kit and
-- differ only in data (layout, title/subtitle, button items + variants, optional muse /
-- spectrum / footer / studio mark). The chrome is now VECTOR — `panel` HudCommands
-- (rounded-rect + 2-stop gradient + border + soft drop shadow), not baked sprites — so
-- the panel auto-sizes to its content and the sapphire / stone / rust buttons are pure
-- data. This script owns only behaviour + layout; every colour/size is a token.
--
-- Layering: the Muse is the one sprite, and the 2D pipelines draw ui-panels *before*
-- sprites within a layer — so the background sits at layer 0 (its panel, then the Muse
-- sprite on top of it), and the scrim / spectrum / popup / buttons / text all sit at
-- layer 1, above the Muse.

local M = {}

local L_BG = 0 -- background gradient / overlay + the Muse sprite
local L_UI = 1 -- scrim, spectrum, popup, buttons, text

-- Hovered item index this frame (set in update, read in draw).
local hover = nil

local function screen()
  return Model and UI and UI.screens and UI.screens[Model.screen]
end

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

-- ── draw-command emitters ──────────────────────────────────────────
local function panel(cmds, x, y, w, h, c0, c1, grad, radius, border, bcolor, feather, layer)
  local cmd = {
    kind = "panel", x = x, y = y, w = w, h = h,
    r = c0[1], g = c0[2], b = c0[3], a = c0[4],
    grad = grad or 0, radius = radius or 0, border = border or 0,
    feather = feather or 0, layer = layer or 0,
  }
  if c1 then
    cmd.r2 = c1[1]; cmd.g2 = c1[2]; cmd.b2 = c1[3]; cmd.a2 = c1[4]
  end
  if bcolor then
    cmd.br = bcolor[1]; cmd.bg = bcolor[2]; cmd.bb = bcolor[3]; cmd.ba = bcolor[4]
  end
  cmds[#cmds + 1] = cmd
end

local function sprite(cmds, tex, x, y, w, h, a, layer)
  cmds[#cmds + 1] =
    { kind = "sprite", tex = tex, x = x, y = y, w = w, h = h, r = 1, g = 1, b = 1, a = a, layer = layer or 0 }
end

local function label(cmds, x, y, str, size, c, align, font, layer)
  cmds[#cmds + 1] = {
    kind = "text", x = x, y = y, text = str, size = size, align = align or "left",
    font = font, r = c[1], g = c[2], b = c[3], a = c[4], layer = layer or 0,
  }
end

local function centered(cmds, cx, y, str, size, c, font, layer)
  label(cmds, cx, y, str, size, c, "center", font, layer)
end

-- An RGBA token clone with a replaced alpha (for the scrim's two fade stops).
local function with_alpha(c, a)
  return { c[1], c[2], c[3], a }
end

-- ── layout ─────────────────────────────────────────────────────────
-- Walk the vertical stack once to size the panel, then place everything from the
-- centred (or left-anchored) panel origin. `update` and `draw` share this so their
-- hit-tests and draws never drift.
local function layout(sw, sh)
  local m = UI.modal
  local s = screen()
  local p = m.panel
  local b = m.buttons
  local pad_x = p.pad_x
  local pw = p.w
  local inner_w = pw - pad_x * 2
  local title_size = s.title_size or m.title.size
  local items = s.items or {}

  -- Offsets from the panel top, accumulated in draw order.
  local c = p.pad_top
  local title_off = c
  c = c + title_size
  local sub_off
  if s.subtitle then
    sub_off = c + m.subtitle.gap_top
    c = c + m.subtitle.gap_top + m.subtitle.size
  end
  local cd_off
  if Model.subtitle then
    cd_off = c + m.countdown.gap_top
    c = c + m.countdown.gap_top + m.countdown.size
  end
  c = c + m.divider.gap
  local div_off = c
  c = c + 1 + m.divider.gap
  local btn_off = c
  local n = #items
  if n > 0 then
    c = c + n * b.h + (n - 1) * b.gap
  end
  local foot_off
  if s.footer then
    foot_off = c + m.footer.gap_top
    c = c + m.footer.gap_top + m.footer.size
  end
  local ph = c + p.pad_bottom

  local px
  if s.layout == "left" then
    px = math.floor(sw * 0.09 + 0.5)
  else
    px = math.floor((sw - pw) * 0.5 + 0.5)
  end
  local py = math.floor((sh - ph) * 0.5 + 0.5)

  local bx = px + pad_x
  local btns = {}
  for i, it in ipairs(items) do
    -- The game-launch button (`start`) takes an optional per-app label override
    -- (`Model.game_label`, from ShellConfig) so a client can name its mode.
    local lbl = (it.id == "start" and Model.game_label) or it.label
    btns[i] = {
      id = it.id, label = lbl, variant = it.variant or "secondary",
      x = bx, y = py + btn_off + (i - 1) * (b.h + b.gap), w = inner_w, h = b.h,
    }
  end

  return {
    px = px, py = py, pw = pw, ph = ph, pad_x = pad_x,
    cx = px + pw * 0.5,
    title_y = py + title_off, title_size = title_size,
    subtitle_y = sub_off and (py + sub_off) or nil,
    countdown_y = cd_off and (py + cd_off) or nil,
    divider_y = py + div_off,
    items = btns,
    footer_y = foot_off and (py + foot_off) or nil,
  }
end

-- ── update ─────────────────────────────────────────────────────────
function M.update(mx, my, clicked, sw, sh)
  if not screen() then
    return {}
  end
  local l = layout(sw, sh)
  hover = nil
  for i, it in ipairs(l.items) do
    if point_in(mx, my, it) then
      hover = i
    end
  end
  local actions = {}
  if clicked and hover then
    actions[l.items[hover].id] = true
  end
  return actions
end

-- ── draw ───────────────────────────────────────────────────────────
function M.draw(sw, sh)
  local s = screen()
  if not s then
    return {}
  end
  local m = UI.modal
  local l = layout(sw, sh)
  local cmds = {}

  -- Background (layer 0): a full-screen gradient (menu) or a flat overlay (pause/confirm).
  if s.bg_top and s.bg_bot then
    panel(cmds, 0, 0, sw, sh, s.bg_top, s.bg_bot, 1, 0, 0, nil, 0, L_BG)
  elseif s.overlay then
    panel(cmds, 0, 0, sw, sh, s.overlay, nil, 0, 0, 0, nil, 0, L_BG)
  end

  -- The Muse at the right margin (layer 0, sprite → drawn over the bg panel). Her
  -- native art is square; fit to 114% of the viewport height with the right edge at
  -- 99% of the width and the bottom at the floor (top / right overflow is clipped) —
  -- the design's `right:1%; bottom:0; height:114%; object-fit:contain`. The left-edge
  -- alpha fade is baked into the texture; `0.5` global opacity keeps her atmospheric.
  if s.muse and Textures and Textures.muse then
    local mh = sh * 1.14
    local mw = mh
    local mx = sw * 0.99 - mw
    local my = sh - mh
    sprite(cmds, Textures.muse, mx, my, mw, mh, 0.5, L_BG)
  end

  -- Left scrim (menu): a horizontal fade that dims the Muse under the popup.
  if s.scrim then
    panel(cmds, 0, 0, sw, sh, with_alpha(s.scrim, 0.88), with_alpha(s.scrim, 0.14), 2, 0, 0, nil, 0, L_UI)
  end

  -- Prism-spectrum thread on the left edge: the Septisigil, drawn as adjacent
  -- vertical gradient segments so the seven colours blend into one ribbon.
  if s.spectrum and m.spectrum then
    local sp = m.spectrum
    local pal = sp.palette
    local segs = #pal - 1
    local seg_h = sh / segs
    for i = 1, segs do
      panel(cmds, 0, (i - 1) * seg_h, sp.w, seg_h + 1,
        with_alpha(pal[i], sp.alpha), with_alpha(pal[i + 1], sp.alpha), 1, 0, 0, nil, 0, L_UI)
    end
  end

  -- Soft drop shadow, then the popup panel (both layer 1, panel before content).
  local p = m.panel
  if p.shadow then
    local g = p.shadow_grow or 0
    panel(cmds, l.px - g, l.py - g + (p.shadow_dy or 0), l.pw + 2 * g, l.ph + 2 * g,
      p.shadow, nil, 0, (p.radius or 0) + g, 0, nil, p.shadow_feather or 0, L_UI)
  end
  panel(cmds, l.px, l.py, l.pw, l.ph, p.fill_top, p.fill_bot, 1, p.radius, p.border_w, p.border, 0, L_UI)

  -- Title + optional subtitle + optional dynamic countdown.
  centered(cmds, l.cx, l.title_y, s.title, l.title_size, m.title.color, "display", L_UI)
  if l.subtitle_y then
    centered(cmds, l.cx, l.subtitle_y, s.subtitle, m.subtitle.size, m.subtitle.color, "label", L_UI)
  end
  if l.countdown_y then
    centered(cmds, l.cx, l.countdown_y, Model.subtitle, m.countdown.size, m.countdown.color, "body", L_UI)
  end

  -- Bronze divider rule under the title band.
  panel(cmds, l.px + l.pad_x, l.divider_y, l.pw - 2 * l.pad_x, 1, m.divider.color, nil, 0, 0, 0, nil, 0, L_UI)

  -- Buttons: pick the variant style, apply hover, draw an optional glow behind.
  local b = m.buttons
  for i, it in ipairs(l.items) do
    local v = b.variants[it.variant] or b.variants.secondary
    local hot = (hover == i)
    if v.glow then
      panel(cmds, it.x - 2, it.y - 2, it.w + 4, it.h + 4, v.glow, nil, 0, b.radius + 2, 0, nil, 6, L_UI)
    end
    local ft = hot and v.hover_top or v.fill_top
    local fb = hot and v.hover_bot or v.fill_bot
    local bc = hot and v.hover_border or v.border
    panel(cmds, it.x, it.y, it.w, it.h, ft, fb, 1, b.radius, b.border_w, bc, 0, L_UI)
    local lc = hot and v.hover_label or v.label
    centered(cmds, it.x + it.w * 0.5, it.y + (it.h - b.label_size) * 0.5, it.label, b.label_size, lc, "label", L_UI)
  end

  -- Footer hint (pause "Esc to return").
  if l.footer_y then
    centered(cmds, l.cx, l.footer_y, s.footer, m.footer.size, m.footer.color, "label", L_UI)
  end

  -- Studio mark, bottom-left under the popup column (menu).
  if s.studio then
    local st = m.studio
    label(cmds, l.px, sh - st.margin_b, s.studio, st.size, st.color, "left", "label", L_UI)
  end

  return cmds
end

return M
