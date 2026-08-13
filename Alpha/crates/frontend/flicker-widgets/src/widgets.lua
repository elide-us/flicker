-- widgets.lua: the LEGACY immediate-mode UI widget toolkit — S10 residue.
--
-- ONE consumer remains: `crates/flicker-world/scripts/world_ui.lua` (the world
-- viewer's control HUD — dropdown / steppers / sliders / reseed button + its
-- duration-weighted timeline scrubber), flagged in the S10 convergence as the
-- last immediate-mode control surface. Every other screen is a declarative
-- component tree (`ui/<kind>.lua` + the Rust walker); when world_ui converges,
-- this file (and `WIDGETS_LUA` / `load_widgets`) is deleted with it. Only the
-- functions world_ui actually calls survive here — the rest of the toolkit
-- (checkbox / radio / gauge / list / timeline / panel / layout helpers) died
-- with their consumers.
--
-- Embedded in `flicker-widgets` (`include_str!`) and loaded as the `Widgets`
-- global via `load_widgets` (ScriptHost::set_lua_module). Each widget is split
-- into an `*_update` (hit-test + interaction, called from the script's `update`)
-- and an `*_draw` (emit commands, called from `draw`).
--
-- Values are NOT stored here: they live in the engine `Model` (two-way) — the
-- update returns the new value, the host applies it, and next frame `Model`
-- carries it for the draw. The only per-widget state the script must keep is
-- transient interaction (a slider's drag flag, a dropdown's open flag), passed
-- in via a `state` table keyed by a stable widget id.
--
-- Styles (colours/sizes) come from ui_theme.json — no hardcoded palette.

local W = {}

local function clamp(v, lo, hi)
  if v < lo then
    return lo
  elseif v > hi then
    return hi
  else
    return v
  end
end

local function point_in(px, py, x, y, w, h)
  return px >= x and px <= x + w and py >= y and py <= y + h
end

local function rgba(c)
  return c[1], c[2], c[3], c[4]
end

local function rect(cmds, x, y, w, h, c, layer)
  local r, g, b, a = rgba(c)
  cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = r, g = g, b = b, a = a, layer = layer }
end

-- `font` (optional) = the Prism face role: "display" (Cormorant, titles/names),
-- "label" (Cinzel, caps), or nil/"body" (EB Garamond, the default prose face).
local function label(cmds, x, y, str, size, c, align, layer, font)
  local r, g, b, a = rgba(c)
  cmds[#cmds + 1] =
    { kind = "text", x = x, y = y, text = str, size = size, align = align, font = font, r = r, g = g, b = b, a = a, layer = layer }
end

-- SLIDER -------------------------------------------------------------------
-- A horizontal track + draggable handle mapping x → [min, max]. Press anywhere
-- on the track to grab; drag while held; release to drop.
function W.slider_update(state, id, r, mx, my, clicked, down, value, min, max)
  if clicked and point_in(mx, my, r.x, r.y - 6, r.w, r.h + 12) then
    state[id] = true
  end
  if not down then
    state[id] = nil
  end
  if state[id] then
    value = min + clamp((mx - r.x) / r.w, 0, 1) * (max - min)
  end
  return value
end

-- `ticks` (optional): draw an N-division ruler behind the handle — N+1 marks
-- across the track, the two ends slightly taller. Older callers omit it and
-- get a plain slider. Tick colour is `s.tick` (falls back to the handle).
-- Prism material (all optional, gated on the style field so older callers are
-- unaffected): `s.fill_hi` = a 1px lit sheen along the top of the fill (the
-- carved sapphire/resource glow); `s.handle_hi` = a lit top edge on the handle.
function W.slider_draw(cmds, r, value, min, max, s, ticks)
  local t = clamp((value - min) / (max - min), 0, 1)
  rect(cmds, r.x, r.y, r.w, r.h, s.track)
  local fw = r.w * t
  rect(cmds, r.x, r.y, fw, r.h, s.fill)
  if s.fill_hi and fw > 0 then
    rect(cmds, r.x, r.y, fw, 1, s.fill_hi)
  end
  if ticks and ticks > 0 then
    local tc = s.tick or s.handle
    for i = 0, ticks do
      local edge = (i == 0 or i == ticks)
      local th = r.h + (edge and 12 or 6)
      rect(cmds, r.x + r.w * (i / ticks) - 1, r.y - (th - r.h) * 0.5, 2, th, tc)
    end
  end
  local hw = s.handle_w
  local hx = r.x + r.w * t - hw * 0.5
  rect(cmds, hx, r.y - 4, hw, r.h + 8, s.handle)
  if s.handle_hi then
    rect(cmds, hx, r.y - 4, hw, 1, s.handle_hi)
  end
end

-- STEPPER (numeric value box) ---------------------------------------------
-- A `[-] value [+]` box. Clicking a square end button steps by `step`,
-- clamped to [min, max]. (Keyboard text entry is a planned enhancement.)
function W.stepper_update(r, mx, my, clicked, value, step, min, max)
  if clicked then
    local bw = r.h
    if point_in(mx, my, r.x, r.y, bw, r.h) then
      value = value - step
    elseif point_in(mx, my, r.x + r.w - bw, r.y, bw, r.h) then
      value = value + step
    end
    value = clamp(value, min, max)
  end
  return value
end

function W.stepper_draw(cmds, r, value, s, fmt)
  local bw = r.h
  rect(cmds, r.x, r.y, r.w, r.h, s.box)
  rect(cmds, r.x, r.y, bw, r.h, s.btn)
  rect(cmds, r.x + r.w - bw, r.y, bw, r.h, s.btn)
  label(cmds, r.x + bw * 0.5, r.y + 2, "-", s.label_size, s.label, "center")
  label(cmds, r.x + r.w - bw * 0.5, r.y + 2, "+", s.label_size, s.label, "center")
  label(cmds, r.x + r.w * 0.5, r.y + 2, string.format(fmt or "%.0f", value), s.label_size, s.label, "center")
end

-- DROPDOWN -----------------------------------------------------------------
-- A header showing the current option; click to open the list, click an item
-- to select (returns its 1-based index), any click closes. The open list draws
-- at `layer = 1` (relative to the screen) so it covers whatever is beneath it.
function W.dropdown_update(state, id, r, mx, my, clicked, count, selected)
  if clicked then
    if state[id] then
      for i = 1, count do
        local iy = r.y + r.h * i
        if point_in(mx, my, r.x, iy, r.w, r.h) then
          selected = i
        end
      end
      state[id] = nil
    elseif point_in(mx, my, r.x, r.y, r.w, r.h) then
      state[id] = true
    end
  end
  return selected
end

function W.dropdown_draw(cmds, state, id, r, options, selected, s)
  rect(cmds, r.x, r.y, r.w, r.h, s.cell)
  label(cmds, r.x + 8, r.y + 3, options[selected], s.label_size, s.label, "left")
  if state[id] then
    for i = 1, #options do
      local iy = r.y + r.h * i
      local fill = (i == selected) and s.hot or s.cell
      rect(cmds, r.x, iy, r.w, r.h, fill, 1)
      label(cmds, r.x + 8, iy + 3, options[i], s.label_size, s.label, "left", 1)
    end
  end
end

-- BUTTON -------------------------------------------------------------------
-- A momentary push button: returns `true` on the frame it is clicked. Stateless
-- (no `state` table needed) — the caller treats the return as a one-shot action.
function W.button_update(r, mx, my, clicked)
  return clicked and point_in(mx, my, r.x, r.y, r.w, r.h)
end

-- Prism material (all optional, gated): `s.glow` = a soft sapphire halo drawn
-- behind (primary emphasis); `s.shadow` = a drop shadow under the slab;
-- `s.border` = a 1px frame; `s.hi` = a lit top edge just inside the frame.
function W.button_draw(cmds, r, text, s, hot)
  if s.glow then
    rect(cmds, r.x - 3, r.y - 3, r.w + 6, r.h + 6, s.glow)
  end
  if s.shadow then
    rect(cmds, r.x, r.y + 2, r.w, r.h, s.shadow)
  end
  local fill = hot and s.hot or s.cell
  rect(cmds, r.x, r.y, r.w, r.h, fill)
  if s.border then
    rect(cmds, r.x, r.y, r.w, 1, s.border)
    rect(cmds, r.x, r.y + r.h - 1, r.w, 1, s.border)
    rect(cmds, r.x, r.y, 1, r.h, s.border)
    rect(cmds, r.x + r.w - 1, r.y, 1, r.h, s.border)
  end
  if s.hi then
    rect(cmds, r.x + 1, r.y + 1, r.w - 2, 1, s.hi)
  end
  label(cmds, r.x + r.w * 0.5, r.y + (r.h - s.label_size) * 0.5, text, s.label_size, s.label, "center", nil, "label")
end

return W
