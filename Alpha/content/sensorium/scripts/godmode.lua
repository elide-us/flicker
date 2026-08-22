-- God Mode (godmode) — the scene's LOGIC (the SceneName.lua half of the pair;
-- bench migration 2026-08-19).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   playing, eroding, balanced (bools)          -- run state + the ledger audit
--   field_<action>_state                        -- "active" / "suggested" / "dim", one per view tab
--   proc_<n>_name / proc_<n>_state              -- stage id + "held" / "running" / "waiting"
--   procs_running / procs_waiting / procs_held  -- the chip's counts
--   a<n>_live / a<n>_in                         -- per condition axis (5)
--   axes_total / axes_live / axes_in_band, life_light / no_life
--   gate_stage / gate_opened / gate_my          -- the newest gate transition
--   ledger_total                                -- Σ mass, pre-formatted engine-side (fmt_mass)
-- plus RESOLVED WORD variables (localization lives in the stringtable, resolved
-- engine-side; this script only PICKS and COMPOSES): w_playing, w_paused,
-- w_play, w_pause, w_erode_on, w_erode_off, w_hold, w_release, w_held,
-- w_running, w_waiting, w_gates_chip, w_in_band, w_out_of_band, w_no_signal,
-- w_life_supporting, w_axes_in_band, w_observed, w_gate_opened, w_gate_shut,
-- w_balanced, w_broken.
--
-- The measurement readouts (stats / interior / crust / air / water / life
-- lines, the gate card's cause + progress, the ledger rows) stay composed at
-- the ENGINE's publish sites: they ride fmt_mass / fmt_pressure unit helpers
-- and are instrument readings, not state words. What this script owns is every
-- STATE-shaped read: which word, which glyph, which style path.

local M = {}

local AXES = 5
local PROC_ROWS = 24
local FIELDS = {
  "field_temperature", "field_differentiation", "field_plates", "field_seams",
  "field_elevation", "field_coast", "field_motion", "field_rain",
  "field_strata", "field_ore",
}

local function n(key)
  return (Model and Model[key]) or 0
end

local function w(key)
  return (Model and Model[key]) or ""
end

local function b(key)
  return (Model and Model[key]) or false
end

function M.derive()
  local out = {}

  -- The transport words + the run-state wash.
  local playing = b("playing")
  out.play_state = playing and w("w_playing") or w("w_paused")
  out.play_state_color = playing and "chemistry.playing.color" or "chemistry.paused.color"
  out.play_label = playing and w("w_pause") or w("w_play")
  out.erode_label = b("eroding") and w("w_erode_off") or w("w_erode_on")

  -- The view strip, in three brightnesses: the view you are IN is brightest, a
  -- view a RUNNING process says shows its work is normal, the rest go dim —
  -- the strip is a reading of the era.
  for _, action in ipairs(FIELDS) do
    local state = w(action .. "_state")
    if state == "active" then
      out[action .. "_style"] = "modal.buttons.variants.primary"
    elseif state == "suggested" then
      out[action .. "_style"] = "modal.buttons.variants.secondary"
    else
      out[action .. "_style"] = "modal.buttons.variants.ghost"
    end
  end

  -- The process console rows: mark glyph + stage + state word, and the
  -- ARM/RELEASE button label — all picked off one state word per row.
  for i = 1, PROC_ROWS do
    local state = w("proc_" .. i .. "_state")
    if state ~= "" then
      local name = w("proc_" .. i .. "_name")
      if state == "held" then
        out["proc_" .. i] = string.format("⊘ %-18s%s", name, w("w_held"))
        out["proc_" .. i .. "_color"] = "chemistry.held"
        out["hold_" .. i .. "_label"] = w("w_release")
      elseif state == "running" then
        out["proc_" .. i] = string.format("● %-18s%s", name, w("w_running"))
        out["proc_" .. i .. "_color"] = "chemistry.ok"
        out["hold_" .. i .. "_label"] = w("w_hold")
      else
        out["proc_" .. i] = string.format("○ %-18s%s", name, w("w_waiting"))
        out["proc_" .. i .. "_color"] = "chemistry.waiting"
        out["hold_" .. i .. "_label"] = w("w_hold")
      end
    end
  end

  -- The processes CHIP — the one line that stands in for the whole console on
  -- the default screen; a held gate is worth surfacing, so it brightens.
  local held = n("procs_held")
  local chip = string.format("⚙ %s  ·  %d %s  ·  %d %s",
    w("w_gates_chip"), n("procs_running"), w("w_running"), n("procs_waiting"), w("w_waiting"))
  if held > 0 then
    chip = chip .. string.format("  ·  %d %s", held, w("w_held"))
  end
  out.proc_summary = chip
  out.proc_chip_style = (held > 0) and "modal.buttons.variants.secondary"
    or "modal.buttons.variants.ghost"

  -- The newest gate transition — the header line and the pause card's headline
  -- share the pieces (the stage name is an identifier, not display copy).
  if w("gate_stage") ~= "" then
    local moved = b("gate_opened") and w("w_gate_opened") or w("w_gate_shut")
    out.gate = string.format("⏸ %s %s  ·  %.0f My", w("gate_stage"), moved, n("gate_my"))
    out.gate_color = b("gate_opened") and "chemistry.ok" or "chemistry.waiting"
    out.gate_headline = string.format("%s %s", w("gate_stage"), moved)
  end

  -- The conservation ledger's verdict line.
  out.ledger_status = string.format("Σ %s  ·  %s",
    w("ledger_total"), b("balanced") and w("w_balanced") or w("w_broken"))
  out.ledger_status_color = b("balanced") and "chemistry.ok" or "chemistry.bad"

  -- Per-axis status words + name washes off the live/in-band booleans.
  for i = 1, AXES do
    local live = b("a" .. i .. "_live")
    if live then
      if b("a" .. i .. "_in") then
        out["a" .. i .. "_status"] = w("w_in_band")
        out["a" .. i .. "_status_color"] = "chemistry.hab.status_in"
      else
        out["a" .. i .. "_status"] = w("w_out_of_band")
        out["a" .. i .. "_status_color"] = "chemistry.hab.status_out"
      end
    else
      out["a" .. i .. "_status"] = w("w_no_signal")
      out["a" .. i .. "_status_color"] = "chemistry.hab.status_dead"
    end
    out["a" .. i .. "_name_color"] = live and "chemistry.hab.name_live" or "chemistry.hab.name_dead"
  end

  -- The verdict footer.
  if b("life_light") then
    out.verdict = w("w_life_supporting")
    out.verdict_color = "chemistry.hab.verdict_life"
  else
    out.verdict = string.format("%d / %d %s", n("axes_in_band"), n("axes_total"), w("w_axes_in_band"))
    out.verdict_color = "chemistry.hab.verdict_count"
  end
  out.observed = string.format("%d / %d %s", n("axes_live"), n("axes_total"), w("w_observed"))

  return out
end

return M
