-- Logo splash, driven by ui_elements.json (`UI.logo`): a backdrop + a large
-- centred wordmark. The scene owns the timing / click-to-skip; this just draws.
-- Edit UI.logo to change the splash text / size / colour. Plain Lua.

local M = {}

function M.update(mx, my, clicked, sw, sh)
  return {}
end

function M.draw(sw, sh)
  if not UI then
    return {}
  end
  local lg = UI.logo
  local bg = lg.backdrop
  local c = lg.color
  return {
    { kind = "rect", x = 0, y = 0, w = sw, h = sh, r = bg[1], g = bg[2], b = bg[3], a = bg[4] },
    {
      kind = "text",
      x = sw * 0.5,
      y = sh * 0.5 - lg.size * 0.5,
      text = lg.text,
      size = lg.size,
      align = "center",
      r = c[1],
      g = c[2],
      b = c[3],
      a = c[4],
    },
  }
end

return M
