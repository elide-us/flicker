-- ui.core — the shared draw + hit-test primitives every UI component builds on.
--
-- Part of the composable vector-UI component library (`content/scripts/ui/`):
-- each component lives in its own file and `require("ui.core")`, so the panel /
-- text / point_in logic lives HERE ONCE instead of being copy-pasted across
-- settings.lua / modal.lua / sim_ui.lua / … (the duplication this library ends).
--
-- Colours are Prism `$token` rgba tables (`{r,g,b,a}`, 0..1) resolved from
-- ui_elements.json. Emitters append plain-data HudCommands to `cmds`.

local core = {}

--- Point-in-rect hit test. `r` = { x, y, w, h }.
function core.point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

local function rgba(c)
  return c[1], c[2], c[3], c[4]
end
core.rgba = rgba

local function colors_equal(a, b)
  return a[1] == b[1] and a[2] == b[2] and a[3] == b[3] and a[4] == b[4]
end

--- First present rgba among `keys` in style table `s`, else `dflt`. Mirrors the
--- engine's `first_color`: a value counts as a colour only if it is a 4+ element
--- table (a numeric key like `radius` is skipped). The alias chain is how a component
--- owns its own state→style mapping (e.g. hover_top → hot → fill_top → cell → fill).
function core.first_color(s, keys, dflt)
  for _, k in ipairs(keys) do
    local c = s[k]
    if type(c) == "table" and #c >= 4 then
      return c
    end
  end
  return dflt
end

--- Numeric style value `s[key]`, else `dflt`. Mirrors the engine's `jnum`.
function core.jnum(s, key, dflt)
  local n = s[key]
  if type(n) == "number" then
    return n
  end
  return dflt
end

--- Format a numeric value like the walker's `fmt_val`: `decimals` places (default 2), an
--- optional leading `+` for a non-negative value when `plus` is set, then a `suffix`.
--- Reads `decimals` / `plus` / `suffix` from `props` (the node's own props).
function core.fmt_val(v, props)
  local dec = math.floor(core.jnum(props, "decimals", 2))
  local sign = (props.plus == true and v >= 0) and "+" or ""
  return sign .. string.format("%." .. dec .. "f", v) .. (props.suffix or "")
end

--- The styled-container backdrop (the walker's `draw_panel_bg`): a filled/gradient
--- rounded panel whose fill / border / grad come from a container style block through the
--- same key-aliases, so a Lua component draws a strip / well / card backdrop identically.
--- `grad` follows the block's explicit `grad`, else 0 (equal stops) / 1 (differing).
function core.panel_bg(cmds, r, s, layer)
  local top = core.first_color(s, { "fill_top", "bg_top", "overlay", "panel_bg", "bg", "fill", "color" },
    { 0.078, 0.09, 0.122, 1.0 })
  local bot = core.first_color(s, { "fill_bot", "bg_bot", "overlay", "panel_bg", "bg", "fill", "color" }, top)
  local border = core.first_color(s, { "panel_border", "border" }, { 0.0, 0.0, 0.0, 0.0 })
  core.panel(cmds, r, {
    fill = top,
    fill2 = bot,
    grad = s.grad,
    radius = core.jnum(s, "radius", 0),
    border = (border[4] > 0) and core.jnum(s, "border_w", 1) or 0,
    border_color = border,
    feather = core.jnum(s, "feather", 0),
    layer = layer,
  })
end

--- A vector rounded panel — the SDF primitive (`HudCommand::Panel`): one draw =
--- rounded-rect + solid/2-stop-gradient fill + optional border + feather (soft
--- shadow). `s` = { fill, fill2?, grad?, radius?, border?, border_color?, feather?,
--- layer? }. `fill2` defaults to `fill` (solid). `grad` (0 flat · 1 vertical · 2
--- horizontal) defaults to 0 when the two stops are equal, else 1 — mirroring the
--- engine's `panel()`, so a Lua component and its Rust twin emit an identical command.
--- `r` = { x, y, w, h }.
function core.panel(cmds, r, s)
  local fill = s.fill
  local fill2 = s.fill2 or fill
  local fr, fg, fb, fa = rgba(fill)
  local r2, g2, b2, a2 = rgba(fill2)
  local grad = s.grad
  if grad == nil then
    grad = colors_equal(fill, fill2) and 0 or 1
  end
  local cmd = {
    kind = "panel",
    x = r.x, y = r.y, w = r.w, h = r.h,
    r = fr, g = fg, b = fb, a = fa,
    r2 = r2, g2 = g2, b2 = b2, a2 = a2,
    grad = grad,
    radius = s.radius, border = s.border, feather = s.feather, layer = s.layer,
  }
  if s.border_color then
    cmd.br, cmd.bg, cmd.bb, cmd.ba = rgba(s.border_color)
  end
  cmds[#cmds + 1] = cmd
end

--- A flat tinted rect (the legacy primitive) — still handy for 1px rules/ticks.
function core.rect(cmds, r, c, layer)
  local rr, gg, bb, aa = rgba(c)
  cmds[#cmds + 1] = { kind = "rect", x = r.x, y = r.y, w = r.w, h = r.h, r = rr, g = gg, b = bb, a = aa, layer = layer }
end

--- A textured quad (`HudCommand::Sprite`) filling `r`, tinted white × `alpha`. `tex` is
--- the engine texture id (a node's `tex` prop / the `Textures` global).
function core.sprite(cmds, r, tex, alpha, layer)
  cmds[#cmds + 1] = {
    kind = "sprite", tex = tex,
    x = r.x, y = r.y, w = r.w, h = r.h,
    r = 1, g = 1, b = 1, a = alpha, layer = layer,
  }
end

--- A single line of text in a Prism face role ("display" | "label" | "body").
function core.text(cmds, x, y, str, size, c, align, font, layer)
  local rr, gg, bb, aa = rgba(c)
  cmds[#cmds + 1] = {
    kind = "text", x = x, y = y, text = str, size = size, align = align, font = font,
    r = rr, g = gg, b = bb, a = aa, layer = layer,
  }
end

--- A MEASURED text caret (`HudCommand::TextCaret`): the render bridge measures the
--- shaped `prefix` (the text before the caret) in `font`/`size` and draws a `w`×`h`
--- bar at `x + measured_width`, clamped right to `max_x`. Emitted by a focused text
--- field — the measurement lives render-side, so Lua stays glyph-free and no caret
--- is ever placed by a `chars × advance` estimate (text ruling 2026-07-31).
function core.caret(cmds, x, y, w, h, prefix, size, c, layer, font, max_x)
  local rr, gg, bb, aa = rgba(c)
  cmds[#cmds + 1] = {
    kind = "caret", x = x, y = y, w = w, h = h, prefix = prefix, size = size,
    r = rr, g = gg, b = bb, a = aa, layer = layer, font = font, max_x = max_x,
  }
end

return core
