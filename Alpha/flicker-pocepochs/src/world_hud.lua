-- flicker-pocepochs HUD: the bottom **heat-loss timeline** for the tick simulation.
--
-- The playhead maps to a sim tick; the log curve is the cooling clock T=(1-k)^tick; the
-- gold ticks are the **emergent tipping points** (drawn where the evaluations actually
-- fired), and the caption shows the current phase + thermal state. The Epoch-1-style
-- planet-size slider stays (a change rebuilds the sim). State crosses as plain numbers:
-- in  → `Model.playhead` (0..1), `Model.tick`/`tick_total`, `Model.cool_k`/`cool_temp`,
--       `Model.phase`, `Model.tip_hard`/`tip_tect` (tipping ticks, −1 = not yet), `planet_*`;
-- out → `playhead`, `planet_freq`, `ui_capture`.

local M = {}

-- Transient drag state, keyed by widget id (the toolkit reads/writes state[id]).
local state = {}

local MARGIN_X = 28
local MARGIN_BOTTOM = 26
local HEIGHT = 30 -- the draggable track height
local CURVE_H = 26 -- the heat-loss curve strip, above the track

local TRACK = { 0.16, 0.17, 0.20, 0.95 }
local FILL = { 0.42, 0.36, 0.24, 0.95 }
local PLAYHEAD = { 0.98, 0.90, 0.66, 1.0 }
local ONSET = { 0.96, 0.78, 0.40, 1.0 }
local CURVE_COLOR = { 0.42, 0.30, 0.22, 0.9 }
local CURVE_NOW = { 0.98, 0.62, 0.32, 1.0 }
local LABEL = { 0.86, 0.88, 0.93, 1.0 }
local DIM = { 0.62, 0.65, 0.70, 1.0 }
local WARN = { 0.93, 0.72, 0.36, 1.0 }
local PANEL_BG = { 0.05, 0.06, 0.08, 0.82 }

-- Planet-size slider (planet size = grid frequency; the scene maps it to a sim rebuild).
local PSIZE = { x = 28, y = 150, w = 250, h = 15, label_size = 14, warn_size = 11 }
local SLIDER_STYLE = {
  track = TRACK,
  fill = FILL,
  handle = { 0.96, 0.86, 0.62, 1.0 },
  handle_w = 8,
}

local function bar_rect(sw, sh)
  return { x = MARGIN_X, y = sh - MARGIN_BOTTOM - HEIGHT, w = sw - 2 * MARGIN_X, h = HEIGHT }
end

local function point_in(px, py, x, y, w, h)
  return px >= x and px <= x + w and py >= y and py <= y + h
end

function M.update(mx, my, clicked, sw, sh, down)
  if not (Model and Widgets) then
    return {}
  end
  local r = bar_rect(sw, sh)
  -- The draggable playhead over the whole timeline (0..1 = tick 0..tick_total).
  local playhead = Widgets.timeline_update(state, "timeline", r, mx, my, clicked, down, Model.playhead or 0)
  local capture = state["timeline"] == true
    or point_in(mx, my, r.x, r.y - CURVE_H - 22, r.w, r.h + CURVE_H + 32)
  local out = { playhead = playhead }

  -- Planet-size slider — a change rebuilds the sim (fresh topology + molten seed).
  local pr = { x = PSIZE.x, y = PSIZE.y, w = PSIZE.w, h = PSIZE.h }
  out.planet_freq = Widgets.slider_update(state, "psize", pr, mx, my, clicked, down,
    Model.planet_freq or 12, Model.planet_min or 12, Model.planet_max or 96)
  if state["psize"] == true or point_in(mx, my, pr.x, pr.y - 6, pr.w, pr.h + 12) then
    capture = true
  end

  out.ui_capture = capture
  return out
end

function M.draw(sw, sh)
  if not (Model and Widgets) then
    return {}
  end
  local cmds = {}
  local r = bar_rect(sw, sh)
  local function rect(x, y, w, h, c)
    cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = c[1], g = c[2], b = c[3], a = c[4] }
  end
  local function text(x, y, t, sz, c)
    cmds[#cmds + 1] =
      { kind = "text", x = x, y = y, text = t, size = sz, r = c[1], g = c[2], b = c[3], a = c[4] }
  end

  local total = math.max(1, math.floor((Model.tick_total or 1) + 0.5))
  local tick = math.floor((Model.tick or 0) + 0.5)
  local k = Model.cool_k or 0.0
  local temp = Model.cool_temp or 1.0
  local phase = Model.phase or ""
  local tip_hard = math.floor((Model.tip_hard or -1) + 0.5)
  local tip_tect = math.floor((Model.tip_tect or -1) + 0.5)
  local cy = r.y - CURVE_H - 4 -- curve strip top
  local function tx(tk)
    return r.x + (tk / total) * r.w
  end

  -- Backing panel behind the caption + curve + track.
  rect(r.x - 10, cy - 18, r.w + 20, (r.y + r.h) - (cy - 18) + 10, PANEL_BG)

  -- Caption: the emergent phase + the thermal state + the tick.
  text(r.x, cy - 16, string.format(
    "PLANET SIM — %s   ·   heat T=%.2f (%.0f%% cooled)   ·   tick %d / %d",
    phase, temp, (1.0 - temp) * 100.0, tick, total), 12, LABEL)

  -- The logarithmic heat-loss curve (T = (1-k)^tick): hot molten left, cooling right, the
  -- current tick lit.
  local stride = math.max(1, math.floor(total / 160))
  local bw = math.max(1, r.w / total * stride - 1)
  local s = 0
  while s <= total do
    local t = (1.0 - k) ^ s
    local bh = math.max(1, t * CURVE_H)
    local col = CURVE_COLOR
    if math.abs(s - tick) <= stride then
      col = CURVE_NOW
    end
    rect(tx(s), cy + CURVE_H - bh, bw, bh, col)
    s = s + stride
  end

  -- The timeline track + fill-to-playhead + the playhead.
  rect(r.x, r.y, r.w, r.h, TRACK)
  local ph = Model.playhead or 0
  rect(r.x, r.y, ph * r.w, r.h, FILL)
  rect(r.x + ph * r.w - 1, r.y - 2, 3, r.h + 4, PLAYHEAD)

  -- Emergent tipping markers: gold ticks (curve → track) + a label, at the tick each
  -- evaluation fired (−1 = not yet, so not drawn).
  local function marker(tk, lbl)
    if tk < 0 then
      return
    end
    rect(tx(tk) - 1, cy, 2, CURVE_H + r.h + 4, ONSET)
    text(math.min(tx(tk) + 4, r.x + r.w - 90), r.y + r.h + 2, lbl, 10, ONSET)
  end
  marker(tip_hard, "crust hardening")
  marker(tip_tect, "tectonics")

  -- Planet-size slider: label, track, the full-Earth cap, and the ½-Earth perf note.
  local pr = { x = PSIZE.x, y = PSIZE.y, w = PSIZE.w, h = PSIZE.h }
  text(pr.x, pr.y - 18, "Planet size:  " .. (Model.psize_label or ""), PSIZE.label_size, LABEL)
  Widgets.slider_draw(cmds, pr, Model.planet_freq or 12, Model.planet_min or 12, Model.planet_max or 96, SLIDER_STYLE)
  text(pr.x + pr.w + 10, pr.y + 1, "full Earth (max)", PSIZE.warn_size, DIM)
  if Model.psize_over_half then
    text(pr.x, pr.y + 20, "Above ½ Earth is a live-batch, performance-hit scenario.", PSIZE.warn_size, WARN)
  end

  return cmds
end

return M
