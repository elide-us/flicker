-- Loomforge Bench — the scene's LOGIC (the SceneName.lua half of the pair;
-- bench migration 2026-08-19).
--
-- The behaviour publishes RAW runtime variables into `Model` each frame:
--   sel_tab                 -- the active page, a NUMBER 0..3 (an index is a
--                              number — 1B64FF03): SM · Pack · Creature · TAE
--   tool                    -- the active canvas tool's id ("tool_select" …)
--   has_edge                -- a transition is selected (the SM side rail swaps
--                              from the clip library to the edge inspector)
--   packkind_<i>_on         -- the four kind-filter toggles' states
--   skel_count / skel_<i>_on -- the skeleton-filter rows (refilled containers)
--   pack_visible / pack_sel -- the card grid's size + selection cursor
--   pack_open               -- the selected pack is the one already loaded
--   tae_has_event           -- an event is selected in the TAE inspector
--   tae_resp_<i>_on         -- the five response-mask toggles' states
--   …plus every readout/label bind the tree names (pre-formatted in Rust).
--
-- `derive()` owns the PRESENTATION selection only: page gates and every
-- lit/idle wash. Every value returned is a boolean gate or a dotted style path
-- into the scene's own style blocks.

local M = {}

local TABS = { "sm", "pack", "creature", "tae" }
local TOOLS = { "tool_select", "tool_add", "tool_link", "tool_delete" }
local RESPONSES = 5
local KINDS = 4

local ACTIVE = "loomforge.tab_active"
local IDLE = "loomforge.tab_idle"

local function n(key)
  return (Model and Model[key]) or 0
end

local function b(key)
  return (Model and Model[key]) or false
end

function M.derive()
  local out = {}

  -- The page gates + tab washes off the one cursor.
  local tab = n("sel_tab")
  for i, id in ipairs(TABS) do
    out["page_" .. id] = (tab == i - 1)
    out["tab_" .. id .. "_sty"] = (tab == i - 1) and ACTIVE or IDLE
  end

  -- The tool rail's washes off the active tool id.
  local tool = (Model and Model.tool) or "tool_select"
  for _, id in ipairs(TOOLS) do
    out[id .. "_sty"] = (tool == id) and ACTIVE or IDLE
  end

  -- The SM side rail: the clip library and the edge inspector swap.
  local edge = b("has_edge")
  out.rail_clips = not edge
  out.rail_edge = edge

  -- Filter toggles + response toggles wear the lit/idle pair.
  for i = 0, KINDS - 1 do
    out["packkind_" .. i .. "_sty"] = b("packkind_" .. i .. "_on") and ACTIVE or IDLE
  end
  for i = 0, n("skel_count") - 1 do
    out["skel_" .. i .. "_sty"] = b("skel_" .. i .. "_on") and ACTIVE or IDLE
  end
  for i = 0, RESPONSES - 1 do
    out["tae_resp_" .. i .. "_sty"] = b("tae_resp_" .. i .. "_on") and ACTIVE or IDLE
  end

  -- The card grid's selection wash + the Load button's primary/secondary swap.
  local sel = n("pack_sel")
  for i = 0, n("pack_visible") - 1 do
    out["packcard_" .. i .. "_sty"] = (i == sel) and ACTIVE or IDLE
  end
  out.pack_load_sty = b("pack_open") and "loomforge.button_secondary" or "loomforge.button_primary"

  -- The TAE inspector's empty-state gate.
  out.tae_no_event = not b("tae_has_event")

  return out
end

return M
