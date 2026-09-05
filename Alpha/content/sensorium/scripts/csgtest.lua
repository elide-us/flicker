-- CSG Test — the scene's LOGIC (the SceneName.lua half of the pair). The
-- proving ground for the new contouring engine and the buildout home for the
-- voxel editing tools; forked from pocclusters.lua (migration P6 2026-08-19).
--
-- The engine publishes RAW runtime variables into `Model` each frame:
--   cam_x/cam_y/cam_z, yaw_deg, pitch_deg        -- camera pose
--   mesh_count, cluster_dim, field_dim           -- grid facts
--   arrow_count, nav_count                       -- diagnostics
--   move_speed, vy, grounded, ground_y_s         -- locomotion (ground_y_s is
--                                                   pre-formatted, "—" = none)
--   has_pick, pick_cx/cy/cz, pick_lod, pick_vx/vy/vz  -- selection
--   insp_cx/cy/cz, insp_kx/ky/kz, insp_lod       -- inspected cell + cluster
--   c0_lx .. c7_wz                               -- 8 corners × 6 coords
--   cc_*                                         -- celestial numbers (their
--                                                   *_val readouts arrive
--                                                   pre-formatted from Rust's
--                                                   celestial::fmt_* — clock,
--                                                   phase, month names)
-- plus RESOLVED WORD variables (localization lives in the stringtable, resolved
-- engine-side; this script only COMPOSES): mode_tag, w_cluster_field, w_yaw,
-- w_pitch, w_clusters, w_voxels, w_mode, w_arrows, w_nav, w_cluster, w_lod,
-- w_voxel, w_state, w_ground_y, w_cell, w_corners.
--
-- `derive()` turns those into the display strings the HUD components bind —
-- formatted here in CONTENT, not in engine code.

local M = {}

local function n(key)
  return (Model and Model[key]) or 0
end

local function w(key)
  return (Model and Model[key]) or ""
end

function M.derive()
  local out = {}

  out.title_line = string.format("%s %d×%d — %s",
    w("w_cluster_field"), n("field_dim"), n("field_dim"), w("mode_tag"))

  out.cam_val = string.format("(%.1f, %.1f, %.1f) · %s %.0f° %s %.0f°",
    n("cam_x"), n("cam_y"), n("cam_z"), w("w_yaw"), n("yaw_deg"), w("w_pitch"), n("pitch_deg"))

  out.grid_val = string.format("%d %s · %d³ %s",
    n("mesh_count"), w("w_clusters"), n("cluster_dim"), w("w_voxels"))

  out.move_val = string.format("%s · %.0f u/s", w("w_mode"), n("move_speed"))

  out.diag_val = string.format("%s %d · %s %d",
    w("w_arrows"), n("arrow_count"), w("w_nav"), n("nav_count"))

  out.speed_val = string.format("%.0f u/s", n("move_speed"))

  if Model and Model.has_pick then
    out.pick_val = string.format("%s (%d, %d, %d) %s %d · %s (%d, %d, %d)",
      w("w_cluster"), n("pick_cx"), n("pick_cy"), n("pick_cz"),
      w("w_lod"), n("pick_lod"),
      w("w_voxel"), n("pick_vx"), n("pick_vy"), n("pick_vz"))

    out.insp_title = string.format("%s (%d, %d, %d)",
      w("w_cell"), n("insp_cx"), n("insp_cy"), n("insp_cz"))
    out.insp_sub = string.format("%s (%d, %d, %d) %s %d · 8 %s",
      w("w_cluster"), n("insp_kx"), n("insp_ky"), n("insp_kz"),
      w("w_lod"), n("insp_lod"), w("w_corners"))

    -- One row per cube corner: the name encodes the corner's octant signs
    -- (bit 0 = X, 1 = Y, 2 = Z), the six coords render fixed-point.
    for i = 0, 7 do
      local function sign(bit)
        if math.floor(i / 2 ^ bit) % 2 == 1 then
          return "+"
        end
        return "-"
      end
      out["insp_c" .. i .. "_name"] = string.format("c%d %s%s%s", i, sign(0), sign(1), sign(2))
      for _, ax in ipairs({ "lx", "ly", "lz", "wx", "wy", "wz" }) do
        out["insp_c" .. i .. "_" .. ax] = string.format("%.2f", n("c" .. i .. "_" .. ax))
      end
    end
  else
    out.pick_val = ""
  end

  if Model and Model.walk then
    out.walk_val = string.format("%s · %s %s · vy %+.1f",
      w("w_state"), w("w_ground_y"), tostring((Model and Model.ground_y_s) or "—"), n("vy"))
  else
    out.walk_val = ""
  end

  return out
end

return M
