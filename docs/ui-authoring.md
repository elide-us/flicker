# Authoring Prism UI

**Scenes are tables. Templates are named arrangements. Pieces are the parts.
You compose; the walker renders.**

This is the guide for building a screen in flicker — a menu, a settings panel, a
HUD, an editor bench. You write a small Lua file that *describes* a tree of
elements. A Rust engine (the "walker") owns all the hard parts — layout,
drawing, hit-testing — so your file never does pixel math and never touches a
colour.

Three tiers, smallest to largest:

| Tier | What it is | Where it lives | You touch it… |
|------|------------|----------------|---------------|
| **Piece** | one element the walker can draw (`button`, `slider`, `text`) | a `match` arm in `component.rs` | rarely (adding a new kind) |
| **Template** | a named arrangement of pieces (`window`, `card`, `popup_menu`) | a builder fn in `template.rs` | sometimes (adding a reusable layout) |
| **Scene** | a whole screen — a tree of pieces + templates | a `.lua` file in `content/scripts/` | **all the time** |

Most authoring is the bottom row: **you write a `.lua` scene** that nests
pieces and drops in templates. You only go up a tier to add a *new* reusable
part. This guide covers all three, but start with the scene.

---

## Contents

1. [The 60-second model](#the-60-second-model)
2. [Your first scene](#your-first-scene)
3. [Size & position](#size--position) — the one subtle thing
4. [Wiring: how data flows](#wiring-how-data-flows)
5. [Styling](#styling)
6. [Template catalog](#template-catalog)
7. [Piece catalog](#piece-catalog)
8. [Worked example: `settings.lua`](#worked-example-settingslua)
9. [Adding a new piece](#adding-a-new-piece)
10. [Adding a new template](#adding-a-new-template)
11. [Sharp edges & guardrails](#sharp-edges--guardrails)

---

## The 60-second model

A **node** is a Lua table with a `component` (its kind) and some fields. Nesting
tables under `children` builds the tree. That's the whole idea:

```lua
Panel {
  style = "menu.panel", pad = 24, gap = 12,
  children = {
    Text   { text = "Greetings", size = 30, color = "menu.title", font = "display" },
    Button { label = "BEGIN", action = "start_game", size = 44 },
  },
}
```

- **`component`** — the kind of element. You don't write `component = "panel"`
  by hand; you use a one-line helper (`Panel`, `Text`, `Button`) that tags the
  table for you. More on that below.
- **fields** — everything else: `style`, `pad`, `size`, `label`, `action`,
  `children`, … Scalars only (numbers, strings, bools). Lists of sub-elements
  ride `children`, never a field.
- **the walker** — a Rust function `run_ui(tree, model, styles, input, state)`
  walks this tree every frame: it lays each node out, draws it, and reports
  which nodes were clicked. You never call it directly for a normal scene; the
  shell does.

You describe *what*; the walker decides *where* and *how*.

---

## Your first scene

A scene is a Lua module that returns a table with a `M.tree()` function. Here is
a complete, working one:

```lua
-- content/scripts/hello.lua
local M = {}

-- Tag a plain table with its component kind. This is the standard preamble;
-- copy it into every scene.
local function tag(k) return function(t) t.component = k; return t end end
local Page   = tag("page")
local Panel  = tag("panel")
local Text   = tag("text")
local Button = tag("button")

function M.tree()
  return Page {
    id = "hello",
    children = {
      Panel {
        anchor = "center", width = 380, pad = 28, gap = 14,
        style = "menu.panel",
        children = {
          Text {
            text = "Greetings, Adventurer",
            size = 40, text_size = 28,
            color = "menu.title", font = "display", align = "center",
          },
          Button {
            id = "start", action = "start_game", label = "BEGIN",
            size = 46, style = "modal.buttons.variants.primary",
          },
        },
      },
    },
  }
end

return M
```

That is a real scene. It centres a stone panel with a title and one button.

**What happens to it.** The shell loads your file once, builds the tree once
(expanding any templates — see below), and caches it. Then every frame it walks
the cached tree. Your button's `action = "start_game"` shows up in the frame's
results when clicked, and the scene's Rust code reads it:

```rust
let frame = run_ui(&tree, &model, &styles, &input, &mut state);
if frame.results.is_on("start_game") {
    // …transition to the game…
}
```

You will spend ~all your time in the Lua. The Rust side is small and usually
already written for your scene type.

---

## Size & position

This is the one part worth internalising. Everything else is naming.

### The main axis and `size`

A **row** lays its children out left-to-right. A **column** (and `panel`) lays
them top-to-bottom. The direction a container flows is its **main axis**.

**`size` is a child's length along its parent's main axis.**

- In a **row**, `size` is the child's **width** (height fills the row).
- In a **column**, `size` is the child's **height** (width fills the column).

```lua
Row { size = 60, children = {              -- this row is 60px tall
  Button { size = 120 },                   -- 120px WIDE (main axis = x)
  Button { size = 120 },
}}
Column { children = {
  Button { size = 46 },                    -- 46px TALL (main axis = y)
  Button { size = 46 },
}}
```

If that feels odd at first: `size` means "how much room I take in the line I'm
part of." One value, whichever way the line runs.

### Growing to fill

A child with **`grow = 1`** takes an equal share of whatever main-axis space is
left after the fixed-size siblings. Weights are relative:

```lua
Row { children = {
  Panel { size = 200 },        -- fixed 200px
  Panel { grow = 1 },          -- takes the rest
}}
Row { children = {
  Panel { grow = 1 },          -- these two split the leftover…
  Panel { grow = 2 },          -- …1 : 2, so the second is twice as wide
}}
```

A grow-1 stack is also the idiom for a **spacer** that pushes things apart:

```lua
Row { children = {
  Text { text = "Title" },
  Stack { grow = 1 },          -- eats the middle
  Button { label = "×", size = 40 },   -- shoved to the right edge
}}
```

### Padding & gaps

| Field | Effect |
|-------|--------|
| `pad` | inset on all four sides |
| `pad_x` | left+right inset (overrides `pad` horizontally) |
| `pad_y` | top+bottom inset (overrides `pad` vertically) |
| `gap` | space between children along the main axis |

`pad_x`/`pad_y` exist for bars: a title bar wants a wide horizontal inset but
must keep its full height, so it sets `pad_x = 30, pad_y = 8` rather than a
uniform `pad = 30` that would eat the bar. (A uniform `pad` bigger than half the
bar's height collapses it — use the per-axis fields there.)

### Anchoring (overlays)

Inside a `page` or `stack`, children don't flow — each is placed by its own
`anchor` (plus an optional `offset = { dx, dy }`). This is how you float a modal
over a backdrop:

| `anchor` | position |
|----------|----------|
| `top_left`, `top`, `top_right` | top edge |
| `left`, `center`, `right` | vertical middle |
| `bottom_left`, `bottom`, `bottom_right` | bottom edge |

```lua
Page { children = {
  Panel { anchor = "top_left", width_frac = 1.0, height_frac = 1.0,   -- full-screen scrim
          style = "screens.pause" },
  Panel { anchor = "center", width = 420, pad = 32,                    -- centred dialog
          style = "modal.panel", children = { --[[ … ]] } },
}}
```

### Fixed and fractional dimensions

| Field | Meaning |
|-------|---------|
| `width` / `height` | fixed pixel dimension for an anchored/measured box |
| `width_frac` / `height_frac` | fraction of the parent (e.g. `1.0` = full) |
| `aspect` | lock width to height × ratio (keeps an image square) |

That's the whole layout model: **flow with `size`/`grow`/`gap`/`pad`, or anchor
with `anchor`/`offset`/`width`/`height`.**

---

## Wiring: how data flows

A scene is not a static picture — controls read and write live values. Wiring
rides a handful of named **channels**. Every channel's value is a **string key**
into the scene's *Model* (published by the scene's Rust `model()` each frame) or
its *results* (read back after the walk).

| Channel | On what | Does |
|---------|---------|------|
| `bind` | inputs (slider, toggle, select…) | two-way: reads the Model key for the current value, writes edits back into results |
| `action` | buttons | fires an event into results when clicked; read it with `results.is_on("id")` |
| `text_bind` | text, buttons | the caption comes from a Model key's pre-formatted string (instead of a literal `text`/`label`) |
| `visible_bind` | any node | the node (and subtree) is shown only when the Model key is truthy |
| `enabled_bind` | inputs | the control draws dim & inert when the key is false |
| `style_bind` | any styled node | a Model key holds the *dotted style path* — lets a node restyle by state (active vs idle tab) |
| `color` / `color_bind` | text | a dotted colour path (text's escape hatch, since colours can't ride scalar props); `color_bind` reads the path from a Model key |

**The contract.** A `bind = "video_vsync"` only works if the scene's Rust
`model()` publishes a `video_vsync` value, and the scene reads
`results.get("video_vsync")` back to apply the edit. The Lua names the wire; the
Rust owns the value on both ends. Keep the names in sync — they are plain
strings and nothing checks them for you.

```lua
Toggle { bind = "video_vsync" }               -- reads + writes model["video_vsync"]
Button { action = "settings_apply" }          -- results.is_on("settings_apply")
Text   { text_bind = "bind_jump" }            -- shows model["bind_jump"], e.g. "SPACE"
Column { visible_bind = "sec_video" }         -- whole section gated by model["sec_video"]
```

---

## Styling

**You never write a colour in a scene.** You point at a **dotted style path**
into `content/resources/ui_elements.json`, and the walker resolves it — including
the `$token` palette — at draw time.

```lua
Button { style = "modal.buttons.variants.primary" }
Panel  { style = "menu.panel" }
Text   { color = "settings.row.name_color" }   -- text uses `color`, not `style`
```

- **Path convention:** `<scene>.<block>.<leaf>` — e.g. `settings.titlebar.close`,
  `menu.panel`, `modal.buttons.variants.danger`.
- **Palette:** every colour in `ui_elements.json` is a `$token` (e.g.
  `"$bronze"`, `"$rune_glow"`) defined once under `theme.tokens`. Style blocks
  reference tokens; scenes reference style blocks. Colour is single-sourced.
- **Reusable blocks you'll reach for a lot:**

  | Path | Use |
  |------|-----|
  | `modal.buttons.variants.primary` / `.secondary` / `.danger` | the three button looks |
  | `modal.panel` | a floating dialog panel |
  | `menu.panel`, `menu.title`, `menu.caption` | menu surfaces & text |
  | `screens.pause` | a full-screen dim scrim |
  | `settings.controls.*` | slider / toggle / select / segment control skins |

**Do not hand-edit `ui_elements.json` by round-tripping it** — it is
hand-formatted with hundreds of significant floats. Add a new block as an
exact-string text insertion, or point at an existing one. (More in
[Sharp edges](#sharp-edges--guardrails).)

---

## Template catalog

A template is a named arrangement you drop into a scene as a table with
`template = "<name>"`, its parameters as fields, and its named **slots** filled
with your content. The builder expands it into pieces once, before the scene is
cached.

```lua
{
  template = "window",
  title = "Settings", w = 1180, h = 726, style = "settings",
  slots = {
    content = { --[[ your body nodes ]] },
    footer  = { --[[ your button nodes ]] },
  },
}
```

| Template | What it is | Slots | Key params |
|----------|-----------|-------|------------|
| **`window`** | carved-stone framed window: title bar (with close/minimize), content well, optional footer, rune corners | `content`, `footer` | `title`, `title_size`, `w`, `h`, `style` (path prefix), `title_h`, `title_pad`/`title_pad_y`, `btn_w`, `has_close`, `close_action`, `close_style`, `has_min`, `min_action`, `footer_h`, `footer_pad`/`footer_pad_y`, `footer_gap`, `body_pad`, `body_style` |
| **`workbench`** | full-screen editor bench: header bar · tab strip · work area (viewport + rail) · footer bar | `header`, `tabs`, `viewport`, `rail`, `footer` | `header_h`/`_pad`/`_gap`/`_style`, `tab_h`/…/`_style`, `body_pad`/`_gap`, `footer_h`/…/`_style`, `footer_btn_h` |
| **`card`** | a titled slab: optional title/subtitle header over content | `content` | `title`, `subtitle`, `disabled` (dims header), `style` (default `menu.panel`), `pad`, `gap`, `header_gap`, `title_size`, `subtitle_size` |
| **`popup_menu`** | modal scrim + centred (or left-hero) popup with title/subtitle/divider and a button stack | `items`, `muse` (backdrop sprite) | `layout` (`center`/`left`), `title`/`title_size`/`title_color`, `subtitle`/…, `divider` (bool), `footer`, `overlay_style`, `panel_style`, `panel_w`/`_pad`/`_gap`, `items_gap` |
| **`choice_dialog`** | centred question modal with a small set of action buttons (built from props as pure data) | `buttons` (optional; replaces the prop-built stack) | `title`, `message`, `subtitle_bind` (live line), `confirm_label`/`_action`/`_variant`, `cancel_label`/`_action`/`_variant`, `panel_w`, `btn_h`, `btn_gap` |
| **`quad_rtt_view`** | one framed render-to-texture holder (a 2×2 editor viewport tiles into it) | — | `source`, `style` (default `assetpipeline.holder`), `quad_id`, `width`/`height`, `live`/`live_bind`, `tint` |
| **`side_by_side_rtt`** | two framed RTT viewports side by side (A/B, in-place vs root-motion) | — | `left_source`, `right_source`, `style`, `gap`, `left_live_bind`, `right_live_bind` |

**Placement.** A template node can carry its own `anchor`/`size`/`width`/… —
these overlay onto the builder's root *only where the builder left them default*.
So `{ template = "card", anchor = "center", width = 400 }` pins and sizes the
instance.

---

## Piece catalog

Pieces are the leaf kinds you compose (and the containers that hold them). Give
each the fields it needs; the walker draws it. Common fields shown — the full
set for any piece is its arm in `component.rs`.

### Containers & layout

| Kind | Purpose | Common fields |
|------|---------|---------------|
| `page` | overlay root; children placed by `anchor` | `id`, `children` |
| `column` | flows children top-to-bottom | `size`, `grow`, `pad`/`pad_x`/`pad_y`, `gap`, `style`, `children` |
| `row` | flows children left-to-right | same as column |
| `panel` | a **styled** column (draws a background) | `style`, + column fields |
| `stack` | overlays children (each by `anchor`); also the go-to `grow` spacer | `grow`, `size`, `children` |
| `scroll` | a clipped, wheel-scrollable column | `bind` (offset key), `wheel` (delta key), `scroll_speed`, `gutter`, `pad`, `grow`, `children` |
| `stage` | a render-to-texture holder (reserves a rect for the frame graph) | `id`, `style`, `source`, `live`/`live_bind`, `tint` |

### Text & buttons

| Kind | Purpose | Common fields |
|------|---------|---------------|
| `text` | one line of text | `text` \| `text_bind`, `size` (box height), `text_size` (glyph), `color` \| `color_bind`, `font` (`body`/`display`/`label`/`rune`), `align` (`left`/`center`/`right`), `italic`, `bold`, `tracking` |
| `button` | a clickable, labelled box | `id`, `action`, `label` \| `text_bind`, `size` (main-axis length), `label_size`, `style`, `style_bind`, `enabled_bind` |

### Inputs

| Kind | Purpose | Common fields |
|------|---------|---------------|
| `toggle` | on/off switch | `bind`, `size`, `style`, `enabled_bind` |
| `checkbox` | labelled tick box | `bind`, `label`, `label_size`, `label_x`, `style` |
| `radio` | one-of-many (matches `value`) | `bind`, `value`, `label`, `style` |
| `slider` | drag a value in a range | `bind`, `min`, `max`, `size`, `value_w`, `slider_h`, `decimals`, `suffix`, `label`, `style`, `enabled_bind` |
| `stepper` | −/+ around a number | `bind`, `min`, `max`, `step`, `label`, `style` |
| `select` | dropdown; options are `option` children | `bind`, `size`, `style`, `children` (`Opt{value,label}`), `enabled_bind` |
| `pill_toggle` | segmented control; `option` children | `bind`, `size`, `style`, `children`, `enabled_bind` |
| `tabs` | tab strip; `option` children | `bind`, `tab_active`, `tab_idle`, `children` |
| `text_field` | single-line editable text | `bind`, `placeholder`, `label_size`, `text_pad`, `style` |
| `option` | **data only** — an entry for select/pill/tabs (never drawn) | `value`, `label` |

### Surfaces & chrome

| Kind | Purpose | Common fields |
|------|---------|---------------|
| `badge` | a small status chip | `label`, `tone`, `solid`, `label_size`, `style` |
| `tooltip` | a hover/info bubble | `text` \| `text_bind`, `rune_color`, `style` |
| `rune_corners` | the four carved corner glyphs (overlay) | `style`, `glyph_size`, `tl`/`tr`/`bl`/`br` (glyph overrides) |
| `context_menu` | a right-click / popup action list | `style`, `children` |
| `cell` | a grid slot that lights when filled | `label`, `style`, `style_off`, `bind`, `enabled_bind` |

### Media

| Kind | Purpose | Common fields |
|------|---------|---------------|
| `sprite` | blit a texture | `tex` (id), `alpha`, `size`, `anchor` |

---

## Worked example: `settings.lua`

The settings screen uses every idea above. Here is its shape, trimmed.

**1 — the preamble** (tag helpers, one per kind you use):

```lua
local Page, Column, Row = tag("page"), tag("column"), tag("row")
local Text, Button, Select, Toggle, Slider = tag("text"), tag("button"), … 
local Opt = tag("option")   -- data child of select / pill
```

**2 — small builders** turn data rows into control pieces. One function maps a
row's `kind` to the right input piece:

```lua
local function control_node(r, wired)
  local key = BINDS[r.id] or ("pv_" .. r.id)   -- Model key: wired name, or preview
  local off = (not wired) and "off" or nil     -- unwired rows point enabled_bind at
  if r.kind == "toggle" then                   --   "off" (always false) → inert
    return Toggle { bind = key, size = 56, style = "settings.controls.toggle", enabled_bind = off }
  elseif r.kind == "slider" then
    return Slider { bind = key, size = 210, min = r.min, max = r.max,
                    style = "settings.controls.slider", enabled_bind = off }
  elseif r.kind == "dropdown" then
    return Select { bind = key, size = 210, style = "settings.controls",
                    enabled_bind = off, children = options_of(r) }
  end
end
```

Note the pattern: **unimplemented rows aren't deleted — they're tagged inert**
(`enabled_bind = "off"`) and wear a `PREVIEW` badge, so the layout target stays
intact.

**3 — the frame is a template.** The whole window (title bar, rune corners,
content well, footer) is the `window` template. The scene only fills the two
slots:

```lua
return Page {
  id = "settings",
  children = {
    Panel { anchor = "top_left", width_frac = 1.0, height_frac = 1.0, style = "screens.pause" },
    {
      template = "window",
      title = "Settings", w = 1180, h = 726, style = "settings",
      title_h = 52, title_pad = 30,
      close_action = "settings_close",
      footer_h = 58, footer_pad = 34,
      slots = {
        content = {
          Row { grow = 1, children = {
            nav_rail(),                                        -- left categories
            Column { grow = 1, pad = 24, gap = 12, children = {
              content_header(),
              content_scroll(),                                -- the scrolling body
            }},
          }},
        },
        footer = footer_children(),                            -- Restore · Apply · Save
      },
    },
  },
}
```

**4 — the scrolling body** is a `scroll` piece bound to an offset the Rust side
owns; sections gate on `visible_bind`:

```lua
{ component = "scroll", bind = "scroll_off", wheel = "wheel", scroll_speed = 46,
  grow = 1, pad = 6, children = {
    video_section(),                                    -- Column { visible_bind = "sec_video" }
    audio_section(),                                    -- Column { visible_bind = "sec_audio" }
    Column { visible_bind = "sec_input", children = { keyboard_tab(), mouse_tab() } },
} }
```

The Rust scene publishes `sec_video`/`sec_audio`/… gates, the `scroll_off`
value, and every control's bind in `model()`, and reads the results back in
`update()` to apply changes. The Lua never computes a coordinate.

---

## Adding a new piece

A piece is one `component` string the walker handles. Adding one is **additive**
— no enum, no new file. Everything is in
`Alpha/crates/frontend/flicker-widgets/src/component.rs`. Six touch points:

1. **Props** arrive automatically (`parse_ui_node`). Read them with
   `ptext` / `pnum` / `pbool`. A *list* prop (options) must ride **child nodes**,
   not a scalar field.
2. **Layout** (`resolve`) — a leaf needs nothing. If your leaf carries child
   *data* (like `select`'s options), add its kind to the no-descend guard so
   `resolve` doesn't try to place those children.
3. **Measure** (`measure`) — add a case only if the intrinsic size differs from
   the `size`/`width`/`height` fallback.
4. **Hit** (`hit_node`) — set `hud_hit`, write `bind`/`action` into results,
   report the current value each frame. Mirror the `checkbox` arm.
5. **Draw** (`draw_node`) — resolve the style block, read colours with
   `first_color(st, &["key"], DEFAULT)`. **No inline rgba** — every colour comes
   from a `$token` via the style block.
6. **Test** — a geometry/interaction unit test, mirroring the existing ones.

Then add a style block for it in `ui_elements.json` and you can use the new kind
from any scene.

---

## Adding a new template

A template is a Rust builder that composes pieces. In
`Alpha/crates/frontend/flicker-widgets/src/template.rs`:

1. **Write the builder** — signature
   `fn my_template(ctx: &BuildCtx, p: &Props, slots: &mut Slots) -> UiNode`.
   Read scalar params with `p_num`/`p_text`/`p_bool`; pull a slot's nodes with
   `take_slot(slots, "name")`; build the subtree with the `elem`/`with_style`/
   `with_num`/`with_text` helpers.
2. **Register it** — one line in `builtin_templates()`:
   `m.insert("my_template", my_template as TemplateFn);`
3. **Purity rule** — a builder reads params and moves slots. It emits nodes whose
   `style` names a **dotted path**; it **never touches a colour**. That's what
   keeps a template from forking the palette.

```rust
fn card(_ctx: &BuildCtx, p: &Props, slots: &mut Slots) -> UiNode {
    let mut panel = with_style(elem("panel"), Some(p_text(p, "style").unwrap_or("menu.panel")));
    panel.pad = p_num(p, "pad").unwrap_or(16.0) as f32;
    panel.children = take_slot(slots, "content");   // move the scene's nodes in
    panel
}
```

Templates expand **once**, before the scene is cached
(`expand(tree, &builtin_templates())`), so there is zero per-frame cost. A tree
with no `template` nodes passes through `expand` unchanged.

---

## Sharp edges & guardrails

Honest list of the things that will bite, and the rules that keep the system
sound.

**The genuinely simple parts** — composing nested tables, dropping in templates
with slots, wiring by name. If you've read this far you already know them.

**The sharp edges:**

- **`size` = main-axis length.** It's width in a row, height in a column. This
  is the one concept to hold in your head. ([Size & position](#size--position))
- **Bind names are strings with no compiler check.** `bind = "video_vsync"` in
  Lua must match a key the scene's Rust `model()` publishes and reads back. A
  typo fails silently (the control just does nothing). Grep the scene's `.rs`
  for the key when a control won't move.
- **Style paths are strings too.** `style = "settings.controls.slidr"` won't
  error — it resolves to nothing and the piece draws unstyled. Copy paths from an
  existing scene or from `ui_elements.json`.
- **Colours only live in text via `color`.** Other pieces get colour through
  their `style` block; text is the exception because a colour can't ride a scalar
  prop cleanly.

**The guardrails (don't cross these):**

- **Never round-trip `ui_elements.json`.** It's hand-formatted with hundreds of
  significant floats; a serialize/parse cycle once produced a 2600-line spurious
  diff. Add a new style block as an *exact-string text insertion*; never let a
  tool reformat the file.
- **No inline rgba anywhere.** Every colour is a `$token` under `theme.tokens`,
  referenced by a style block, referenced by a scene. One palette, one place.
- **Don't rebuild the tree per frame.** Build (and `expand`) once, cache, then
  walk the cache. New scene data = data only; new template or piece = Rust.
- **Enhance in place.** There is one walker (`flicker-widgets`) and one parser
  (`flicker-script`). Extend them; don't fork a parallel UI path.

---

*The walker: `Alpha/crates/frontend/flicker-widgets/src/component.rs` ·
templates: `…/template.rs` · the node schema:
`Alpha/crates/scripting/flicker-script/src/lib.rs` · styles:
`Alpha/content/resources/ui_elements.json` · example scenes:
`Alpha/content/scripts/*.lua`.*
