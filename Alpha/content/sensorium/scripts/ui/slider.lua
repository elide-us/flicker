-- ui.slider — a labelled value track: an optional left label, a track with a fill up to
-- the value + a handle, and an optional right value readout. When FOCUSED (its
-- `focus_group` currently holds this bind) the label/track/fill recolour. This module
-- owns the slider's WHOLE definition — draw AND hit: the whole row claims + focuses,
-- the padded grab band (track ±6px) captures a drag, and a captured drag maps the
-- pointer over the track into the bound value while the button stays down.
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (number), min, max, label, label_w, value_w, slider_h, focused,
--   captured (the walker holds this node's pointer capture), decimals / plus / suffix
--   (value format), layer, style {
--   track, fill, fill_hi?, handle, focus_track, focus_fill, focus_label, value_color,
--   label_size, value_size, handle_w }.

local core = require("ui.core")

local slider = {}

local INK = { 0.871, 0.847, 0.788, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local SAP = { 0.141, 0.247, 0.471, 1.0 }
local STONE = { 0.055, 0.063, 0.086, 1.0 }
local RUNE = { 0.435, 0.592, 1.0, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- The track: inset past the label/value columns, `slider_h` tall, vertically
-- centred — THE geometry, shared by draw and hit so they can never disagree.
local function track_rect(r, props)
  local label_w = core.jnum(props, "label_w", 0)
  local value_w = core.jnum(props, "value_w", 0)
  local sh = core.jnum(props, "slider_h", r.h)
  return {
    x = r.x + label_w,
    y = r.y + (r.h - sh) * 0.5,
    w = math.max(r.w - label_w - value_w, 0),
    h = sh,
  }
end

function slider.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local min = core.jnum(props, "min", 0)
  local max = core.jnum(props, "max", 1)
  local value = props.bind_value or min
  local focused = props.focused == true

  local track = track_rect(r, props)

  -- Row label (left column).
  local lsz = core.jnum(s, "label_size", 13)
  if props.label and props.label ~= "" then
    local lc = focused and core.first_color(s, { "focus_label" }, RUNE) or INK
    core.text(cmds, r.x, r.y + (r.h - lsz) * 0.5, props.label, lsz, lc, "left", "body", layer)
  end

  -- Track + fill + handle.
  local track_col = focused and core.first_color(s, { "focus_track" }, STONE)
    or core.first_color(s, { "track" }, STONE)
  local fill_col = focused and core.first_color(s, { "focus_fill" }, RUNE)
    or core.first_color(s, { "fill" }, SAP)
  core.rect(cmds, track, track_col, layer)
  local t = math.max(math.min((value - min) / (max - min), 1), 0)
  local fw = track.w * t
  core.rect(cmds, { x = track.x, y = track.y, w = fw, h = track.h }, fill_col, layer)
  local fill_hi = core.first_color(s, { "fill_hi" }, CLEAR)
  if fill_hi[4] > 0 and fw > 0 then
    core.rect(cmds, { x = track.x, y = track.y, w = fw, h = 1 }, fill_hi, layer)
  end
  local hw = core.jnum(s, "handle_w", 9)
  core.rect(cmds, { x = track.x + track.w * t - hw * 0.5, y = track.y - 4, w = hw, h = track.h + 8 },
    core.first_color(s, { "handle" }, SAP), layer)

  -- Value readout (right column).
  if core.jnum(props, "value_w", 0) > 0 then
    local vsz = core.jnum(s, "value_size", 12)
    core.text(cmds, r.x + r.w, r.y + (r.h - vsz) * 0.5, core.fmt_val(value, props), vsz,
      core.first_color(s, { "value_color" }, DIM), "right", "body", layer)
  end
end

-- The whole row claims + (on click) grabs group focus; only the padded GRAB band
-- (track ±6px — a press just above/below the thin track still grabs) captures.
-- While captured and held, the pointer maps over the track into the bound value —
-- even after it leaves the row (the walker keeps a captured node dispatching).
function slider.hit(mx, my, r, props, click, down)
  local track = track_rect(r, props)
  local over = core.point_in(mx, my, r)
  local v = { hit = over }
  if over and click then
    v.group_focus = true
    local grab = { x = track.x, y = track.y - 6, w = track.w, h = track.h + 12 }
    if core.point_in(mx, my, grab) then
      v.capture = true
    end
  end
  if down and (props.captured == true or v.capture == true) then
    local min = core.jnum(props, "min", 0)
    local max = core.jnum(props, "max", 1)
    local t = math.max(math.min((mx - track.x) / track.w, 1), 0)
    v.value = min + t * (max - min)
    v.hit = true
  end
  return v
end

return slider
