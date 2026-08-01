-- ui.context_menu — a standalone floating context menu (DS `ContextMenu`) — the
-- reusable form of the popup that `select` draws inline. The NODE RECT IS THE MENU;
-- its items ride as CHILD data rows (`props.children`), each carrying a `label` (or
-- `text`), an optional right-aligned `hint` keybind, `active`/`disabled` bools, a
-- `divider` flag (a hairline separator in place of a row), and an `action` the
-- WALKER fires on a row click — the verdict names the picked row (`activate_child`,
-- 1-based) and the walker fires that child's action, because the children are data
-- of the menu node, so the menu is the dispatch surface. Reuses the settings
-- dropdown's menu block shape (top/bot/border/radius/row_h/label_size/label/sel_bg/
-- sel_label/hover_bg, plus divider/disabled/hint), so it matches the `select` popup
-- exactly. Rows stack from the top of the rect at `row_h` and may run PAST the
-- authored rect bottom — a live one stays clickable there (the walker's click-edge
-- candidate escape for children-as-data kinds), though only the authored rect
-- claims the pointer. To float the menu above siblings, put a `layer` prop on the
-- node — run_ui lifts the whole subtree (a select must lift its popup internally
-- because popup and field share one node; a context_menu owns its node, so no
-- internal lift is needed).
--
-- The COMPONENT interface: M.draw(cmds, rect, props) +
-- M.hit(mx, my, rect, props, click, down) → verdict (see ui.checkbox).
-- props: children (rows as above, each optionally carrying its `action` name),
--   style (the resolved menu block), mx, my (pointer, for the hover wash), layer.

local core = require("ui.core")

local M = {}

local INK = { 0.871, 0.847, 0.788, 1.0 }
local DIM = { 0.561, 0.541, 0.49, 1.0 }
local STONE = { 0.055, 0.063, 0.086, 1.0 }
local SAP = { 0.141, 0.247, 0.471, 1.0 }
local CLEAR = { 0.0, 0.0, 0.0, 0.0 }

-- The `i`-th (1-based) item row: a full-width, `row_h`-tall band stacked from the
-- top of the menu rect — THE geometry, shared by draw and hit so they can never
-- disagree (mirrors the select popup's inline row math).
local function menu_row(r, row_h, i)
  return { x = r.x, y = r.y + (i - 1) * row_h, w = r.w, h = row_h }
end

function M.draw(cmds, r, props)
  local st = props.style or {}
  local layer = props.layer
  local kids = props.children or {}

  -- Menu backdrop: top→bot fill, rounded, optional hairline border (mirrors the
  -- select popup's menu panel).
  local mtop = core.first_color(st, { "top" }, STONE)
  local mbot = core.first_color(st, { "bot" }, mtop)
  local mborder = core.first_color(st, { "border" }, CLEAR)
  core.panel(cmds, r, {
    fill = mtop,
    fill2 = mbot,
    radius = core.jnum(st, "radius", 3),
    border = (mborder[4] > 0) and 1 or 0,
    border_color = mborder,
    layer = layer,
  })

  local row_h = core.jnum(st, "row_h", 30)
  local msz = core.jnum(st, "label_size", 15)
  for i = 1, #kids do
    local c = kids[i]
    local row = menu_row(r, row_h, i)
    if c.divider == true then
      -- A divider draws a centred hairline instead of a label row.
      core.rect(cmds, {
        x = r.x + 8,
        y = row.y + math.floor(row.h * 0.5),
        w = math.max(r.w - 16, 0),
        h = 1,
      }, core.first_color(st, { "divider", "border" }, DIM), layer)
    else
      local disabled = c.disabled == true
      local active = c.active == true
      -- Row wash (inset 4px like the select popup): an active row takes the
      -- selection fill; a hovered *live* row takes the hover fill.
      if active then
        core.rect(cmds, { x = r.x + 4, y = row.y, w = math.max(r.w - 8, 0), h = row_h },
          core.first_color(st, { "sel_bg" }, SAP), layer)
      elseif not disabled and core.point_in(props.mx, props.my, row) then
        core.rect(cmds, { x = r.x + 4, y = row.y, w = math.max(r.w - 8, 0), h = row_h },
          core.first_color(st, { "hover_bg" }, STONE), layer)
      end
      -- Label (left): disabled dims, active uses the selected-label colour.
      local lc
      if disabled then
        lc = core.first_color(st, { "disabled", "label" }, DIM)
      elseif active then
        lc = core.first_color(st, { "sel_label", "label" }, INK)
      else
        lc = core.first_color(st, { "label" }, INK)
      end
      core.text(cmds, r.x + 14, row.y + (row_h - msz) * 0.5, c.label or c.text or "", msz, lc,
        "left", "body", layer)
      -- Optional right-aligned keybind hint, always dim.
      if type(c.hint) == "string" then
        core.text(cmds, r.x + r.w - 14, row.y + (row_h - msz) * 0.5, c.hint, msz,
          core.first_color(st, { "hint" }, DIM), "right", "body", layer)
      end
    end
  end
end

-- The whole authored rect claims the pointer (a gap, a divider, or a disabled row
-- never picks through to the scene behind); rows laid PAST the rect bottom do not
-- claim, but a click on a live one still activates it — the verdict names the
-- 1-based row and the walker fires that child's `action`. Dividers and disabled
-- rows are inert for interaction.
function M.hit(mx, my, r, props, click)
  local v = { hit = core.point_in(mx, my, r) }
  if click then
    local st = props.style or {}
    local kids = props.children or {}
    local row_h = core.jnum(st, "row_h", 30)
    for i = 1, #kids do
      local c = kids[i]
      if not (c.divider == true or c.disabled == true)
        and core.point_in(mx, my, menu_row(r, row_h, i)) then
        v.activate_child = i
      end
    end
  end
  return v
end

return M
