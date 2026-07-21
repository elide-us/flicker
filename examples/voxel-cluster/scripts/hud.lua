-- In-game HUD, driven by Lua + `ui_elements.json` (the `UI.voxel` section) and the
-- engine `Model` (live values published each frame). Layout / colours / labels
-- live in the JSON; this script owns only behaviour: formatting the stat strings
-- from the Model, and the checkbox click-state the engine queries. Edit
-- ui_elements.json to move / restyle the HUD — no recompile. Plain Lua.

local M = {}

-- Persistent checkbox state, keyed by each checkbox's `key` (the names the
-- engine queries). Starts empty → nil reads as off; toggled on click.
local checked = {}

-- Transient widget interaction state (slider drag / dropdown open), keyed by a
-- stable widget id — see widgets.lua. Values themselves live in the Model.
local widget_state = {}

-- The interactive controls' widget rects, from UI.voxel.controls (label on the
-- left, widget at `widget_x`).
local function control_rects(sh)
  local c = UI.voxel.controls
  local rh = c.row_h
  -- Bottom-left, stacked upward, so the (now larger) checkbox table above never
  -- covers it. `margin_b` leaves room below the locomotion row for its dropdown
  -- to open on-screen.
  local loco_y = sh - c.margin_b
  local sens_y = loco_y - rh
  local speed_y = sens_y - rh
  return c, {
    speed = { x = c.widget_x, y = speed_y, w = c.speed.w, h = c.speed.h },
    sens = { x = c.widget_x, y = sens_y, w = c.sens.w, h = c.sens.h },
    loco = { x = c.widget_x, y = loco_y, w = c.locomotion.w, h = c.locomotion.h },
  }
end

local function checkbox_items()
  return UI.voxel.checkboxes.items
end

-- Day/night cycle panel (lower-right). Lays out a 3-unit-wide grid, two rows:
-- row 1 = Sun (1 unit) + Moon (2 units); row 2 = Year (full 3-unit span).
-- Anchored to the bottom-right via the screen size so it tracks resolution.
-- Three rows: Sun + Moon, then Year (full span), then Speed (full span).
-- Returns the config plus the four slider rects, the backdrop panel rect, and
-- the label anchor points.
local function lighting_rects(sw, sh)
  local L = UI.voxel.lighting
  local unit, gap, pad, th = L.unit, L.gap, L.pad, L.track_h
  local total_w = unit * 3 + gap
  local panel_w = total_w + pad * 2
  local label_h = L.label_size + 6
  -- Vertical stack inside the content box (offsets from the content top).
  local row1_label_dy = L.header_h
  local row1_track_dy = row1_label_dy + label_h
  local row2_label_dy = row1_track_dy + th + L.row_gap
  local row2_track_dy = row2_label_dy + label_h
  local row3_label_dy = row2_track_dy + th + L.row_gap
  local row3_track_dy = row3_label_dy + label_h
  local row4_label_dy = row3_track_dy + th + L.row_gap
  local row4_track_dy = row4_label_dy + label_h
  local row5_label_dy = row4_track_dy + th + L.row_gap
  local row5_track_dy = row5_label_dy + label_h
  local row6_label_dy = row5_track_dy + th + L.row_gap
  local row6_track_dy = row6_label_dy + label_h
  local panel_h = row6_track_dy + th + pad * 2
  local px = sw - L.margin_x - panel_w
  local py = sh - L.margin_y - panel_h
  local cx, cy = px + pad, py + pad
  local sun = { x = cx, y = cy + row1_track_dy, w = unit, h = th }
  local moon = { x = cx + unit + gap, y = cy + row1_track_dy, w = unit * 2, h = th }
  local year = { x = cx, y = cy + row2_track_dy, w = total_w, h = th }
  local speed = { x = cx, y = cy + row3_track_dy, w = total_w, h = th }
  local fog = { x = cx, y = cy + row4_track_dy, w = total_w, h = th }
  local lat = { x = cx, y = cy + row5_track_dy, w = total_w, h = th }
  local epoch = { x = cx, y = cy + row6_track_dy, w = total_w, h = th }
  local panel = { x = px, y = py, w = panel_w, h = panel_h }
  local labels = {
    header = { x = cx, y = cy },
    sun = { x = sun.x, y = cy + row1_label_dy },
    moon = { x = moon.x, y = cy + row1_label_dy },
    year = { x = year.x, y = cy + row2_label_dy },
    speed = { x = speed.x, y = cy + row3_label_dy },
    fog = { x = fog.x, y = cy + row4_label_dy },
    lat = { x = lat.x, y = cy + row5_label_dy },
    epoch = { x = epoch.x, y = cy + row6_label_dy },
  }
  return L, sun, moon, year, speed, fog, lat, epoch, panel, labels
end

local MONTHS = { "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec" }
local MOON_PHASES = { "New", "Waxing", "Full", "Waning" }

local function fmt_clock(t)
  local hh = math.floor(t) % 24
  local mm = math.floor((t - math.floor(t)) * 60)
  return string.format("%02d:%02d", hh, mm)
end

local function fmt_month(m)
  return MONTHS[math.floor(m) % 12 + 1]
end

local function fmt_moon(w)
  return string.format("%s · wk %.1f", MOON_PHASES[math.floor(w) % 4 + 1], w)
end

local function fmt_speed(v)
  if v <= 0 then
    return "paused"
  end
  return string.format("%.0f min/s", v)
end

local function fmt_fog(v)
  if v <= 0 then
    return "clear"
  end
  return string.format("%.0f%%", v * 100)
end

local function fmt_lat(v)
  if v >= 89.5 then
    return "north pole"
  elseif v <= 0.5 then
    return "equator"
  end
  return string.format("%.0f°N", v)
end

local function fmt_epoch(v)
  return string.format("yr %.1f", v)
end

-- The i-th checkbox's clickable square, from the JSON geometry.
local function box_rect(i)
  local cb = UI.voxel.checkboxes
  return { x = cb.origin[1], y = cb.origin[2] + (i - 1) * cb.row_h, w = cb.box, h = cb.box }
end

local function point_in(px, py, r)
  return px >= r.x and px <= r.x + r.w and py >= r.y and py <= r.y + r.h
end

function M.update(mx, my, clicked, sw, sh, down)
  if not UI then
    return {}
  end
  if clicked then
    for i, item in ipairs(checkbox_items()) do
      if point_in(mx, my, box_rect(i)) then
        checked[item.key] = not checked[item.key]
      end
    end
  end
  local states = {}
  for _, item in ipairs(checkbox_items()) do
    states[item.key] = checked[item.key] or false
  end

  -- Interactive controls (slider / stepper / dropdown), wired two-way: the
  -- current value comes from the Model, the new value goes back in `states`,
  -- and the host applies it (so next frame the Model reflects it).
  if Model and Widgets then
    local c, r = control_rects(sh)
    states.move_speed = Widgets.slider_update(
      widget_state,
      "speed",
      r.speed,
      mx,
      my,
      clicked,
      down,
      Model.move_speed or 0,
      c.speed.min,
      c.speed.max
    )
    states.look_sens = Widgets.stepper_update(
      r.sens,
      mx,
      my,
      clicked,
      Model.look_sens or 0,
      c.sens.step,
      c.sens.min,
      c.sens.max
    )
    local current = (Model.walk and 2) or 1
    states.locomotion =
      Widgets.dropdown_update(widget_state, "loco", r.loco, mx, my, clicked, #c.locomotion.options, current)

    -- Day/night cycle sliders (lower-right): sun (time of day), moon (phase),
    -- year (season). Wide tracks + absolute drag, so a click/touch sets the
    -- handle where the cursor is — fine control, no big jumps.
    if UI.voxel.lighting then
      local L, sun, moon, year, speed, fog, lat, epoch, panel = lighting_rects(sw, sh)
      states.time_of_day =
        Widgets.slider_update(widget_state, "sun", sun, mx, my, clicked, down, Model.time_of_day or 12, L.sun.min, L.sun.max)
      states.moon_phase =
        Widgets.slider_update(widget_state, "moon", moon, mx, my, clicked, down, Model.moon_phase or 0, L.moon.min, L.moon.max)
      states.year_month =
        Widgets.slider_update(widget_state, "year", year, mx, my, clicked, down, Model.year_month or 6, L.year.min, L.year.max)
      -- Speed (sim min / real sec); 0 = paused. Distinct id from the move-speed
      -- slider's "speed" so their drag flags don't collide.
      states.sim_speed = Widgets.slider_update(
        widget_state,
        "cycle_speed",
        speed,
        mx,
        my,
        clicked,
        down,
        Model.sim_speed or 0,
        L.speed.min,
        L.speed.max
      )
      states.fog =
        Widgets.slider_update(widget_state, "fog", fog, mx, my, clicked, down, Model.fog or 0, L.fog.min, L.fog.max)
      states.latitude = Widgets.slider_update(
        widget_state,
        "latitude",
        lat,
        mx,
        my,
        clicked,
        down,
        Model.latitude or 0,
        L.latitude.min,
        L.latitude.max
      )
      states.epoch = Widgets.slider_update(
        widget_state,
        "epoch",
        epoch,
        mx,
        my,
        clicked,
        down,
        Model.epoch or 0,
        L.epoch.min,
        L.epoch.max
      )
      -- Tell the engine the pointer is over this panel so it suppresses the
      -- world-pick on the same press (the static top-left HUD guard in Rust
      -- doesn't know this screen-relative, lower-right panel).
      states.lighting_capture = point_in(mx, my, panel)
    end
  end
  return states
end

-- Stat readouts: each named line's style (y / size / colour) comes from
-- UI.voxel.stats.<id>; the text is formatted here from the engine Model.
local function stats(cmds)
  if not Model then
    return
  end
  local s = UI.voxel.stats
  local function line(spec, text)
    local c = spec.color
    cmds[#cmds + 1] =
      { kind = "text", x = s.x, y = spec.y, text = text, size = spec.size, r = c[1], g = c[2], b = c[3], a = c[4] }
  end

  local controls
  if Model.walk then
    controls = "walk — WASD on surface, gravity, right-drag look"
  else
    controls = "fly — WASD move, R/F up/down, right-drag look"
  end
  line(s.title, string.format("voxel cluster — %.0f×%.0f field — %s", Model.field_dim, Model.field_dim, controls))
  line(
    s.pos,
    string.format(
      "pos: (%.0f, %.0f, %.0f)  yaw: %.2f  pitch: %.2f",
      Model.pos_x,
      Model.pos_y,
      Model.pos_z,
      Model.yaw,
      Model.pitch
    )
  )
  line(s.clusters, string.format("clusters: %.0f   extent: %.0f³ voxels each", Model.cluster_count, Model.cluster_dim))
  line(
    s.config,
    string.format(
      "config — speed: %.0f  sens: %.4f  invert-Y: %s  invert-X: %s",
      Model.move_speed,
      Model.look_sens,
      tostring(Model.invert_y),
      tostring(Model.invert_x)
    )
  )
  line(
    s.diag,
    string.format("corner arrows stored: %.0f   nav clusters (rings 0–2): %.0f", Model.corner_arrows, Model.nav_count)
  )
  line(s.escape, "press Escape to quit")

  local pick
  if Model.has_pick then
    pick = string.format(
      "pick: (%.0f, %.0f, %.0f, lod %.0f) p = (%.0f, %.0f, %.0f)",
      Model.pick_cx,
      Model.pick_cy,
      Model.pick_cz,
      Model.pick_lod,
      Model.pick_px,
      Model.pick_py,
      Model.pick_pz
    )
  else
    pick = "pick: (none — left-click a face)"
  end
  line(s.pick, pick)

  if Model.walk then
    local ground = "—"
    if Model.has_ground then
      ground = string.format("%.0f", Model.ground_y)
    end
    local grounded = Model.grounded and "grounded" or "airborne"
    line(s.walk, string.format("walk: %s   ground y: %s   vy: %+.1f", grounded, ground, Model.vy))
  end
end

-- The feature-toggle checkbox panel, from UI.voxel.checkboxes.
local function checkboxes(cmds)
  local cb = UI.voxel.checkboxes
  local h = cb.header
  cmds[#cmds + 1] = {
    kind = "text",
    x = cb.origin[1],
    y = cb.origin[2] - 24,
    text = h.text,
    size = h.size,
    r = h.color[1],
    g = h.color[2],
    b = h.color[3],
    a = h.color[4],
  }
  for i, item in ipairs(checkbox_items()) do
    local r = box_rect(i)
    local bc = cb.border_color
    cmds[#cmds + 1] =
      { kind = "rect", x = r.x, y = r.y, w = r.w, h = r.h, r = bc[1], g = bc[2], b = bc[3], a = bc[4] }
    local fill = checked[item.key] and cb.fill_on or cb.fill_off
    cmds[#cmds + 1] = {
      kind = "rect",
      x = r.x + 2,
      y = r.y + 2,
      w = r.w - 4,
      h = r.h - 4,
      r = fill[1],
      g = fill[2],
      b = fill[3],
      a = fill[4],
    }
    local lc = cb.label_color
    cmds[#cmds + 1] = {
      kind = "text",
      x = r.x + r.w + 10,
      y = r.y + 1,
      text = item.label,
      size = cb.label_size,
      r = lc[1],
      g = lc[2],
      b = lc[3],
      a = lc[4],
    }
  end
end

-- Interactive controls: a move-speed slider, a sensitivity stepper, and a
-- locomotion dropdown — drawn from the Widgets toolkit, styled from JSON, with
-- live values from the Model.
local function controls(cmds, sh)
  if not (Model and Widgets) then
    return
  end
  local c, r = control_rects(sh)
  local lc = c.label_color
  local function lbl(text, y)
    cmds[#cmds + 1] = {
      kind = "text",
      x = c.label_x,
      y = y,
      text = text,
      size = c.label_size,
      r = lc[1],
      g = lc[2],
      b = lc[3],
      a = lc[4],
    }
  end

  lbl(c.speed.label, r.speed.y)
  Widgets.slider_draw(cmds, r.speed, Model.move_speed or 0, c.speed.min, c.speed.max, c.slider_style)
  lbl(c.sens.label, r.sens.y)
  Widgets.stepper_draw(cmds, r.sens, Model.look_sens or 0, c.stepper_style, c.sens.fmt)
  lbl(c.locomotion.label, r.loco.y)
  local current = (Model.walk and 2) or 1
  Widgets.dropdown_draw(cmds, widget_state, "loco", r.loco, c.locomotion.options, current, c.dropdown_style)
end

-- Day/night cycle panel: a backdrop, a header, and three ruler sliders (Sun /
-- Moon / Year) with a live value readout, anchored to the lower-right corner.
-- Values come from the Model; the host applies the returned values next frame.
local function lighting(cmds, sw, sh)
  if not (Model and Widgets and UI.voxel.lighting) then
    return
  end
  local L, sun, moon, year, speed, fog, lat, epoch, panel, labels = lighting_rects(sw, sh)
  local st = L.style

  local bg = L.backdrop
  cmds[#cmds + 1] =
    { kind = "rect", x = panel.x, y = panel.y, w = panel.w, h = panel.h, r = bg[1], g = bg[2], b = bg[3], a = bg[4] }

  local h = L.header
  cmds[#cmds + 1] = {
    kind = "text",
    x = labels.header.x,
    y = labels.header.y,
    text = h.text,
    size = h.size,
    r = h.color[1],
    g = h.color[2],
    b = h.color[3],
    a = h.color[4],
  }

  -- One labelled, ruled slider per cycle control. The name sits at the track's
  -- left edge; the formatted value is centred over the track.
  local function row(anchor, name, value, fmt, r, spec)
    local lc, vc = L.label_color, L.value_color
    cmds[#cmds + 1] = {
      kind = "text",
      x = anchor.x,
      y = anchor.y,
      text = name,
      size = L.label_size,
      r = lc[1],
      g = lc[2],
      b = lc[3],
      a = lc[4],
    }
    cmds[#cmds + 1] = {
      kind = "text",
      x = r.x + r.w * 0.5,
      y = anchor.y,
      text = fmt(value),
      size = L.label_size,
      align = "center",
      r = vc[1],
      g = vc[2],
      b = vc[3],
      a = vc[4],
    }
    Widgets.slider_draw(cmds, r, value, spec.min, spec.max, st, spec.ticks)
  end

  row(labels.sun, L.sun.label, Model.time_of_day or 12, fmt_clock, sun, L.sun)
  row(labels.moon, L.moon.label, Model.moon_phase or 0, fmt_moon, moon, L.moon)
  row(labels.year, L.year.label, Model.year_month or 6, fmt_month, year, L.year)
  row(labels.speed, L.speed.label, Model.sim_speed or 0, fmt_speed, speed, L.speed)
  row(labels.fog, L.fog.label, Model.fog or 0, fmt_fog, fog, L.fog)
  row(labels.lat, L.latitude.label, Model.latitude or 0, fmt_lat, lat, L.latitude)
  row(labels.epoch, L.epoch.label, Model.epoch or 0, fmt_epoch, epoch, L.epoch)
end

function M.draw(sw, sh)
  if not UI then
    return {}
  end
  local cmds = {}
  stats(cmds)
  checkboxes(cmds)
  controls(cmds, sh)
  lighting(cmds, sw, sh)
  return cmds
end

return M
