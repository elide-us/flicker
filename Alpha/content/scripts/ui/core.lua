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

--- A vector rounded panel — the SDF primitive (`HudCommand::Panel`): one draw =
--- rounded-rect + solid/2-stop-gradient fill + optional border + feather (soft
--- shadow). `s` = { fill, fill2?, grad?, radius?, border?, border_color?,
--- feather?, layer? }. `grad`: 1 = vertical, 2 = horizontal (default 1 when fill2
--- is set). `r` = { x, y, w, h }.
function core.panel(cmds, r, s)
  local fr, fg, fb, fa = rgba(s.fill)
  local cmd = {
    kind = "panel",
    x = r.x, y = r.y, w = r.w, h = r.h,
    r = fr, g = fg, b = fb, a = fa,
    radius = s.radius, border = s.border, feather = s.feather, layer = s.layer,
  }
  if s.fill2 then
    cmd.r2, cmd.g2, cmd.b2, cmd.a2 = rgba(s.fill2)
    cmd.grad = s.grad or 1
  end
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

--- A single line of text in a Prism face role ("display" | "label" | "body").
function core.text(cmds, x, y, str, size, c, align, font, layer)
  local rr, gg, bb, aa = rgba(c)
  cmds[#cmds + 1] = {
    kind = "text", x = x, y = y, text = str, size = size, align = align, font = font,
    r = rr, g = gg, b = bb, a = aa, layer = layer,
  }
end

return core
