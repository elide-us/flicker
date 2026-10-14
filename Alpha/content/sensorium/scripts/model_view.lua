-- Model View — the model view scene's Lua ORCHESTRATION layer.
--
-- The panel's whole component tree is authored as DATA (scenes/model_view.scene.json)
-- and its picture is the rig view the Rust behaviour declares as this scene's root stage.
-- This script only turns published KNOBS into visibility and display copy — the Lua layer is
-- untrusted and end-user-editable (security law 69E82FE7), so it holds no structure, no
-- per-frame data and no decision the engine relies on.
--
-- What the engine publishes into `Model` each frame:
--   projection  -- "persp" | "top" | "side" | "front" (the host's per-instance param)
--   flipped     -- whether an orthographic panel views from the opposite side
--   chrome      -- the host's param: may this panel show the isolate row?
--   label       -- the host's param: may this panel show its corner label?
--   limb, cull, cull_at  -- the isolate controls' committed values, echoed back
--
-- `arrange()` gates the two chrome slices; `derive()` writes the corner label's caption.

local M = {}

-- The corner label's copy, per projection: { near side, flipped side }. Stringtable
-- tokens, so a panel's caption localizes like every other display string. A projection
-- absent from this table is the PERSPECTIVE picker: no opposite side, so no caption and
-- no flip control.
local LABEL = {
  top   = { "$mv_top",   "$mv_bottom" },
  side  = { "$mv_left",  "$mv_right"  },
  front = { "$mv_front", "$mv_back"   },
}

local function projection()
  return (Model and Model.projection) or "persp"
end

-- The caption for the side this panel currently shows; empty on the perspective picker.
function M.derive()
  local sides = LABEL[projection()]
  if not sides then
    return { view_label = "" }
  end
  local flipped = (Model and Model.flipped) == true
  return { view_label = flipped and sides[2] or sides[1] }
end

-- The chrome gates. Both are the host's param AND an orthographic projection: the
-- perspective picker never cuts (it is the panel you pick in) and has no side to flip to,
-- so neither control would do anything there.
function M.arrange()
  local ortho = LABEL[projection()] ~= nil
  return {
    chrome_on = { on = ortho and (Model and Model.chrome) == true },
    label_on  = { on = ortho and (Model and Model.label) == true },
  }
end

return M
