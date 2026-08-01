-- ui.stepper — a −[value]+ numeric box: an optional left label, then a field with two
-- square end buttons (− / +) painted over it and the formatted value centred. This
-- module owns the stepper's WHOLE definition — draw AND hit: the whole row claims,
-- but only the two square end cells step (± `step`, clamped to [min, max]).
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: bind_value (number), min, max, step, label, label_w, field_h, decimals /
--   plus / suffix (value format), layer, style { field / box, btn, label,
--   value_color, label_size, value_size }.

local core = require("ui.core")

local stepper = {}

local INK = { 0.871, 0.847, 0.788, 1.0 }
local PANEL = { 0.078, 0.09, 0.122, 1.0 }
local STONE = { 0.055, 0.063, 0.086, 1.0 }

-- The field (inset past the optional label column, vertically centred) and its two
-- square end buttons — THE geometry, shared by draw and hit so they can never disagree.
local function cells(r, props)
  local label_w = core.jnum(props, "label_w", 0)
  local fh = core.jnum(props, "field_h", r.h)
  local field = { x = r.x + label_w, y = r.y + (r.h - fh) * 0.5, w = math.max(r.w - label_w, 0), h = fh }
  local bw = field.h
  local minus = { x = field.x, y = field.y, w = bw, h = field.h }
  local plus = { x = field.x + field.w - bw, y = field.y, w = bw, h = field.h }
  return field, minus, plus
end

function stepper.draw(cmds, r, props)
  local s = props.style or {}
  local layer = props.layer
  local min = core.jnum(props, "min", 0)
  local value = props.bind_value or min

  local field, minus, plus = cells(r, props)

  local lsz = core.jnum(s, "label_size", 13)
  local label_col = core.first_color(s, { "label" }, INK)

  -- Optional row label in the reserved left column (mirrors the slider).
  if props.label and props.label ~= "" then
    core.text(cmds, r.x, r.y + (r.h - lsz) * 0.5, props.label, lsz, label_col, "left", "body", layer)
  end

  -- Field background, then the two square end buttons painted over it.
  core.rect(cmds, field, core.first_color(s, { "field", "box" }, PANEL), layer)
  local btn_col = core.first_color(s, { "btn" }, STONE)
  core.rect(cmds, minus, btn_col, layer)
  core.rect(cmds, plus, btn_col, layer)

  -- Glyphs on the end buttons, the value centred in the field.
  core.text(cmds, minus.x + minus.w * 0.5, minus.y + (minus.h - lsz) * 0.5, "-", lsz, label_col, "center", "label", layer)
  core.text(cmds, plus.x + plus.w * 0.5, plus.y + (plus.h - lsz) * 0.5, "+", lsz, label_col, "center", "label", layer)
  local vsz = core.jnum(s, "value_size", lsz)
  core.text(cmds, field.x + field.w * 0.5, field.y + (field.h - vsz) * 0.5, core.fmt_val(value, props), vsz,
    core.first_color(s, { "value_color", "label" }, label_col), "center", "body", layer)
end

-- The whole row claims; only the two square end cells STEP: − steps down, + steps
-- up, by `step` (default 1), clamped to [min, max] from the CURRENT bound value.
-- A click on the value field between them changes nothing (the echo still reports).
function stepper.hit(mx, my, r, props, click)
  local v = { hit = core.point_in(mx, my, r) }
  if click then
    local _, minus, plus = cells(r, props)
    local min = core.jnum(props, "min", 0)
    local max = core.jnum(props, "max", 1)
    local step = core.jnum(props, "step", 1)
    local cur = props.bind_value or min
    if core.point_in(mx, my, minus) then
      v.value = math.max(math.min(cur - step, max), min)
    elseif core.point_in(mx, my, plus) then
      v.value = math.max(math.min(cur + step, max), min)
    end
  end
  return v
end

return stepper
