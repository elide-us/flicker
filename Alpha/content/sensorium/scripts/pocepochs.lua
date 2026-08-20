-- Epoch Simulation (pocepochs) — the scene's LOGIC (the SceneName.lua half of
-- the pair; bench migration 2026-08-19).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   tick, my, temp_k, cells_n                    -- the sim clock + census
--   core_n, crust_n, ocean_n, atm_n              -- per-layer cell counts
--   playing (bool), view (NUMBER 0..2), cut, freq
--   axes_total / axes_live / axes_in_band, life / no_life, air_kind (resolved)
--   a<n>_v / a<n>_live / a<n>_in_band            -- per condition axis
--   a<n>_name / a<n>_lolab / a<n>_hilab          -- observer metadata, resolved
-- plus RESOLVED WORD variables (localization lives in the stringtable, resolved
-- engine-side; this script only COMPOSES): w_playing, w_paused, w_play, w_pause,
-- w_view, w_size, w_tick, w_cells, w_core, w_crust, w_ocean, w_atm, w_in_band,
-- w_out_of_band, w_no_signal, w_life_supporting, w_axes_in_band, w_observed,
-- w_air, w_view_0 / w_view_1 / w_view_2.
--
-- `derive()` turns those into the display strings the HUD binds — formatted
-- here in CONTENT, not in engine code.

local M = {}

local AXES = 5

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

  -- The readout line: clock · census · per-layer emergence counts.
  out.stats_val = string.format(
    "%s %d  ·  %.0f My  ·  T %.0f K  ·  %d %s  ·  %s %d · %s %d · %s %d · %s %d",
    w("w_tick"), n("tick"), n("my"), n("temp_k"), n("cells_n"), w("w_cells"),
    w("w_core"), n("core_n"), w("w_crust"), n("crust_n"),
    w("w_ocean"), n("ocean_n"), w("w_atm"), n("atm_n"))

  -- Transport words + the view line (the keyboard hint line died with the keys).
  local playing = b("playing")
  out.play_state = playing and w("w_playing") or w("w_paused")
  out.play_state_color = playing and "pocepochs.playing.color" or "pocepochs.paused.color"
  out.play_label = playing and w("w_pause") or w("w_play")
  local view = n("view")
  local view_word = w("w_view_" .. view)
  out.view_line = string.format("%s: %s", w("w_view"), view_word)
  out.view_label = view_word
  out.size_line = string.format("%s %d", w("w_size"), n("freq"))
  out.layers_view = (view == 2)

  -- Per-axis status words + name washes off the live/in-band booleans.
  for i = 1, AXES do
    local live = b("a" .. i .. "_live")
    if live then
      if b("a" .. i .. "_in_band") then
        out["a" .. i .. "_status"] = w("w_in_band")
        out["a" .. i .. "_status_color"] = "pocepochs.hab.status_in"
      else
        out["a" .. i .. "_status"] = w("w_out_of_band")
        out["a" .. i .. "_status_color"] = "pocepochs.hab.status_out"
      end
    else
      out["a" .. i .. "_status"] = w("w_no_signal")
      out["a" .. i .. "_status_color"] = "pocepochs.hab.status_dead"
    end
    out["a" .. i .. "_name_color"] = live and "pocepochs.hab.name_live" or "pocepochs.hab.name_dead"
  end

  -- The verdict footer.
  if b("life") then
    out.verdict = w("w_life_supporting")
    out.verdict_color = "pocepochs.hab.verdict_life"
  else
    out.verdict = string.format("%d / %d %s", n("axes_in_band"), n("axes_total"), w("w_axes_in_band"))
    out.verdict_color = "pocepochs.hab.verdict_count"
  end
  out.observed = string.format("%d / %d %s", n("axes_live"), n("axes_total"), w("w_observed"))
  out.air = string.format("%s %s", w("w_air"), w("air_kind"))

  return out
end

return M
