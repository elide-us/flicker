-- flicker-pocepochs — the LIFE-SUPPORTING CONDITIONS panel.
--
-- A read-only detector view (memory `habitability-detection-model`): each condition axis is
-- a gauge with a green "habitable band" in the middle and a marker showing where the planet
-- currently sits, read live from the Rust observer (`flicker_worldengine::habitability`). The
-- planet is **life-supporting** only when every axis is simultaneously in its band — detected,
-- never scripted. Axes whose driving process isn't simulated yet show "no signal" and light up
-- as those procedures land. Composed from the shared `Widgets` toolkit (gauge_draw); values
-- cross the boundary as flat Model scalars: per axis `a<i>_{name,v,lo,hi,lolab,hilab,in}`
-- (`v = -1` ⇒ no signal), plus `axes_total`/`axes_live`/`axes_in`/`life`.

local M = {}

-- Prism palette (this POC HUD reads no ui_elements.json, so inline literals that
-- mirror the shared theme.tokens: bronze structure, ink/dim text, Septisigil
-- green = Life/in-band, stamina amber = caution/out-of-band, sunk stone, sapphire rim).
local GOLD = { 0.722, 0.592, 0.353, 1.0 } -- bronze (headers / structural)
local TEXT = { 0.871, 0.847, 0.788, 1.0 } -- ink
local DIM = { 0.561, 0.541, 0.490, 1.0 }
local GREEN = { 0.184, 0.616, 0.357, 1.0 } -- Septisigil green (Life) — in-band
local AMBER = { 0.941, 0.804, 0.416, 1.0 } -- stamina amber — out-of-band caution
local PANEL_BG = { 0.055, 0.063, 0.086, 0.90 } -- sunk stone
local BORDER = { 0.227, 0.353, 0.627, 0.80 } -- sapphire rim

-- Gauge style: dark stone track, green habitable band, amber marker out-of-band /
-- green in-band, a lit rim sheen, and a grey wash over axes with no live signal yet.
local GAUGE = {
  track = { 0.078, 0.090, 0.122, 0.95 },
  band = { 0.184, 0.616, 0.357, 0.55 },
  marker = AMBER,
  marker_in = GREEN,
  no_signal = { 0.055, 0.063, 0.086, 0.72 },
  sheen = { 0.90, 0.94, 1.0, 0.10 },
  marker_w = 5,
}

local PAD = 12
local HEAD_H = 28
local ROW_H = 42
local BAR_H = 12
local FOOT_H = 50
local PANEL_W = 330

-- Read-only panel: no interaction, so update is a no-op (the contract requires both entry
-- points to exist). The gauges are driven entirely by the observer's Model each frame.
function M.update(mx, my, clicked, sw, sh, down)
  return {}
end

function M.draw(sw, sh)
  if not (Model and Widgets) then
    return {}
  end
  local cmds = {}
  local function rect(x, y, w, h, c)
    cmds[#cmds + 1] = { kind = "rect", x = x, y = y, w = w, h = h, r = c[1], g = c[2], b = c[3], a = c[4] }
  end
  local function text(x, y, t, sz, c, align)
    cmds[#cmds + 1] =
      { kind = "text", x = x, y = y, text = t, size = sz, align = align, r = c[1], g = c[2], b = c[3], a = c[4] }
  end

  local total = math.floor((Model.axes_total or 5) + 0.5)
  local panel_h = PAD + HEAD_H + total * ROW_H + FOOT_H + PAD
  -- Inset from the right/bottom edges so the panel (and its right-aligned values) clear the
  -- window edge.
  local px = sw - PANEL_W - 32
  local py = sh - panel_h - 20

  -- Backing panel + 1px frame.
  rect(px, py, PANEL_W, panel_h, PANEL_BG)
  rect(px, py, PANEL_W, 1, BORDER)
  rect(px, py + panel_h - 1, PANEL_W, 1, BORDER)
  rect(px, py, 1, panel_h, BORDER)
  rect(px + PANEL_W - 1, py, 1, panel_h, BORDER)

  text(px + PAD, py + PAD, "LIFE-SUPPORTING CONDITIONS", 15, GOLD)

  local bx = px + PAD
  local bw = PANEL_W - 2 * PAD
  local y = py + PAD + HEAD_H
  for i = 1, total do
    local name = Model["a" .. i .. "_name"] or ("axis " .. i)
    local v = Model["a" .. i .. "_v"]
    local lo = Model["a" .. i .. "_lo"] or 0.3
    local hi = Model["a" .. i .. "_hi"] or 0.7
    local live = v ~= nil and v >= 0

    -- Row header: axis name (left) + status (right).
    text(bx, y, name, 13, live and TEXT or DIM)
    if live then
      local inb = Model["a" .. i .. "_in"]
      text(bx + bw, y, inb and "in band" or "out of band", 11, inb and GREEN or AMBER, "right")
    else
      text(bx + bw, y, "no signal yet", 11, DIM, "right")
    end

    -- The gauge bar (green band + live marker; greyed when no signal).
    local r = { x = bx, y = y + 17, w = bw, h = BAR_H }
    Widgets.gauge_draw(cmds, r, live and v or nil, lo, hi, GAUGE)

    -- End captions (what the low / high ends mean).
    text(bx, y + 17 + BAR_H + 2, Model["a" .. i .. "_lolab"] or "", 10, DIM, "left")
    text(bx + bw, y + 17 + BAR_H + 2, Model["a" .. i .. "_hilab"] or "", 10, DIM, "right")

    y = y + ROW_H
  end

  -- Aggregate verdict: the light + a plain count. Green only when the full conjunction holds.
  local life = Model.life == true
  local live_n = math.floor((Model.axes_live or 0) + 0.5)
  local in_n = math.floor((Model.axes_in or 0) + 0.5)
  rect(bx, y + 6, 14, 14, life and GREEN or { 0.24, 0.10, 0.10, 1.0 })
  local verdict = life and "LIFE-SUPPORTING" or (in_n .. " / " .. total .. " axes in band")
  text(bx + 24, y + 6, verdict, 14, life and GREEN or TEXT)
  text(bx + bw, y + 8, live_n .. " / " .. total .. " observed", 11, DIM, "right")

  -- The current atmosphere "kind" (dominant gas), the outgassing readout.
  text(bx, y + 26, "air: " .. (Model.atm_kind or "—"), 12, DIM)

  return cmds
end

return M
