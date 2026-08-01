# Authoring a Prism screen

**A screen is a Lua table. Every control's behaviour is one `ui/<kind>.lua` module.
Every reusable surface is one entry in `ui_templates.json`. The Rust walker owns
layout, caching, dispatch, and routing — and nothing else.**

That split is what this system buys you: a new screen is *data*, a new control is
*one Lua file*, a new surface is *one JSON proto*, and none of the three needs a
recompile of the other two. This guide is what a person needs to author one
without opening the walker.

> **Scope.** This is the *usage* guide. The design of record — the ratified
> vocabulary, the ownership split, the convergence history — lives in the MCP
> memory bank (start at the Prism UI unified spec), never here.

---

## Contents

1. [The 60-second model](#the-60-second-model)
2. [Vocabulary](#vocabulary) — the locked names
3. [The simplest screen, end to end](#the-simplest-screen-end-to-end)
4. [Bounded dispatch](#bounded-dispatch) — the one subtle thing
5. [The four channels](#the-four-channels)
6. [Layout](#layout)
7. [Component catalog](#component-catalog)
8. [Writing a component](#writing-a-component) — `ui/<kind>.lua`
9. [Templates are data](#templates-are-data) — `ui_templates.json`
10. [Surfaces](#surfaces) — declaring what a screen shows
11. [Intents](#intents) — the UI *is* the input declaration
12. [Strings](#strings) — `$token`
13. [The drift gates](#the-drift-gates) — what a new screen must pass
14. [The mode launcher](#the-mode-launcher)
15. [Sanctioned exceptions](#sanctioned-exceptions)
16. [Sharp edges & guardrails](#sharp-edges--guardrails)

---

## The 60-second model

A node is a Lua table with a `component` kind. Nesting under `children` builds
the tree. The scene's Lua returns that tree **once**; Rust walks the cached tree
every frame.

```lua
Screen {
  id = "hello", on_menu = "pause_open",
  children = {
    Cell {
      anchor = "center", width = 380, pad = 28, gap = 14, style = "menu.panel",
      children = {
        Text   { text = "$hello_title", text_size = 28, align = "center", font = "display",
                 color = "menu.title" },
        Text   { text_bind = "status", text_size = 14, color = "menu.caption" },
        Button { id = "begin", action = "start_game", label = "$hello_begin", size = 46,
                 style = "modal.buttons.variants.primary" },
      },
    },
  },
}
```

Four things are happening, and they are the whole system:

| In the table | What owns it |
|---|---|
| `component` kind (`cell`, `text`, `button`) | the walker places it; a Lua module in `ui/<kind>.lua` draws + hit-tests it |
| `style = "menu.panel"` / `color = "menu.title"` | a **dotted path** into `ui_elements.json`; colours resolve from `theme.tokens` at draw |
| `$hello_title` | a **stringtable token**; the active locale's text resolves at draw |
| `text_bind` / `action` / `on_menu` | **name channels** into the scene's Rust Model + results |

You never write a pixel coordinate, a colour, or an English string.

---

## Vocabulary

One name per concept. These are locked; a synonym in new code is a defect.

| Term | What it is |
|---|---|
| **Screen** | the root full-screen container a Scene shows — composes RTTs + the UI layer. The `screen` component kind. |
| **RTT** | an offscreen viewport with its own camera/lighting, driven by the FrameGraph. The `rtt` component kind; the walker *reserves* its rect (`UiFrame.rtts`) and the scene's frame graph fills it. |
| **Scene** | the `flicker-scene` lifecycle unit — `enter`/`update`/`render`/`exit` + `Transition`. Owns the Model and reads the results. |
| **Template** | a whole surface as pure DATA: a parameterised tree in `ui_templates.json`. |
| **Component** | the logic owner, a configurable black box. *Interactive* components live in `ui/<kind>.lua`; *structural* ones (`frame`, `card`, `option_grid`) are Rust composition builders. |
| **Primitive** | no logic: the layout resolver, the HudCommand renderers (panel SDF / rect / sprite / **text** / caret / clip), point-in-rect, measure_text. |

**Folded names.** These were renamed; the old ones do not parse:

| Retired | Use |
|---|---|
| `stage` / `StageSlot` | `rtt` |
| `panel` | `cell` (a cell carrying a `style`) |
| `column` | `cell` |
| `page` | `screen` |
| `scroll` | `list` |
| `element` | survives **only** in the filename `ui_elements.json` |

The one deliberate exception: the top-level **`stages`** section of
`ui_elements.json` keeps its name — it is sub-scene *lighting config* keyed by an
`rtt` node's `source`, not a component kind.

---

## The simplest screen, end to end

Everything below is real, working shape. Four pieces.

### 1 — register the scene

`prism-alpha/src/main.rs` (or any shell client):

```rust
SceneEntry::new("hello", "Hello Bench", "primary", || Box::new(HelloScene::new()))
    .with_realm(REALM_DEVELOPER)                       // which mode page lists it
    .with_info(SceneInfo::new(                         // omit → a plain popup button
        "Hello Bench", "Tool", "Demo",
        "The smallest walker screen.", "Clay 0.1 · Demo",
    )),
```

### 2 — the screen tree

`Alpha/content/sensorium/scripts/hud_hello.lua`:

```lua
local M = {}

local function tag(kind) return function(t) t.component = kind; return t end end
local Screen, Cell, Text, Button = tag("screen"), tag("cell"), tag("text"), tag("button")

function M.tree()
  return Screen {
    id = "hello",
    on_menu = "pause_open",                     -- the input DECLARATION
    children = {
      { template = "card",                      -- a template instance
        anchor = "center", width = 420,
        title = "$hello_title",
        slots = {
          content = {
            Text   { text_bind = "tick", text_size = 16, color = "menu.caption" },
            Button { id = "poke", action = "poke", label = "$hello_poke", size = 44,
                     style = "modal.buttons.variants.primary" },
          },
        },
      },
    },
  }
end

return M
```

Three tokens go into `Alpha/content/data/stringtable.json`:

```json
"hello_title": { "en-us": "HELLO" },
"hello_poke":  { "en-us": "POKE" }
```

### 3 — build it once, in `enter`

```rust
self.styles = load_styles(HUD_UI_ELEMENTS);
let script = ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES)?;
load_ui_json(&script, HUD_UI_ELEMENTS);            // the `UI` global for the script
let tree = expand(script.ui_tree()?.unwrap(), &builtin_templates());  // templates → nodes
self.intents = UiIntents::of(&tree);               // collects `on_menu`
self.tree = Some(tree);
self.script = Some(script);                        // RETAINED: it is also the component library
```

`script` is kept alive deliberately — the same VM that built the tree serves as
the `ComponentLibrary` the walker dispatches each node's draw/hit into.

### 4 — walk it, every frame

```rust
let mut model = ValueMap::new().with("tick", format!("{:.1}s", self.t));
UiIntents::mirror_into(&mut model, &self.fired_sigs);   // the sig_* mirror

let snap = UiInput {
    mouse: input.mouse_position,
    clicked: input.mouse_left_pressed,
    down: input.mouse_left,
    screen: renderer.size(),
    typed: String::new(), backspace: false,
    wheel: input.mouse_wheel_delta,
};
let lib = self.script.as_ref().map(|h| h as &dyn ComponentLibrary);
let frame = run_ui_with(&tree, &model, &self.styles, &snap, &mut self.ui_state, lib);

self.commands = frame.commands;                          // → render_hud in render()
let over_hud  = frame.results.is_on("hud_hit");          // UI ate the pointer
if frame.results.is_on("poke") { self.t = 0.0; }         // the button's action

// input bus: the declared on_menu arrives as a fired result name
let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
    .with_intents(&self.intents);
{
    let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
    Router::dispatch(&events, &mut chain, &mut self.route);
}
self.fired_sigs = walker.take_fired();
if self.fired_sigs.iter().any(|n| n == "pause_open") {
    return Transition::Push(Box::new(PauseScene::new(/* … */)));
}
```

**What Rust does per frame, in order** (`run_ui_impl`):

1. `resolve` — lay the tree into rects, skipping `visible_bind`-false subtrees.
2. hit pass — bounded dispatch into each candidate component's `M.hit`; verdicts
   fold into `results` and `UiState`.
3. `fold_typed` — this frame's keyboard text into the focused node's `bind`.
4. `echo_binds` — every placed control with a `bind` reports its effective value.
5. draw pass — each node whose fingerprint changed redraws; the rest replay from
   the cache. Commands are lifted onto the node's sub-layer.
6. `rtt` pass — each `rtt` node reserves an `RttSlot` for the scene's frame graph.

Reference screens to copy from, smallest first:
`hud_solarbirth.lua` (97 L, text only) · `hud_clicktrainer.lua` (126 L, + one
button) · `hud_pocepochs.lua` (202 L, + gauges) · `settings.lua` (405 L, every
control) · `menu.lua` (284 L, templates + launcher).

---

## Bounded dispatch

This is the one thing to internalise, because it decides what your screen costs
and what your component may assume.

**Lua is entered draw-on-change and hit-on-input. Never per node per frame.**

### Draw

Every node — Lua-drawn *and* Rust-drawn — has a cache entry holding its emitted
commands plus a fingerprint of every input its draw reads:

- the node's rect, layer, enabled flag, clip;
- its resolved style block (by **content**, so a rebuilt or hot-reloaded styles
  tree invalidates correctly);
- the Model/results values under the keys it reads — its `bind`, every prop whose
  name ends in `_bind`, and `focus_group`;
- for children-as-data kinds (`tabs`, `pill_toggle`, `select`, `context_menu`),
  its children's props;
- the stringtable generation (a language switch invalidates exactly the text);
- pointer position, **only** for Lua-dispatched kinds, and only rounded to whole
  pixels, and only while the node is hot or open.

Unchanged fingerprint ⇒ verbatim replay, zero crossings. A still frame is
`lua_draws == 0` and `redraw_nodes == 0` — asserted by tests, observable through
`UiFrame.stats` (`UiStats { lua_draws, lua_hits, redraw_nodes, nodes }`).

The naming convention is load-bearing: **a prop ending in `_bind` holds a Model
key**. Name a new one `foo_bind` and the cache covers it the day you author it.

### Hit

Lua is entered for a hit only when *both*:

- the frame is **input-active** — a click edge, a release edge, actual
  pointer/screen movement, a wheel tick, or a held drag; and
- the node is a **candidate** — pointer within its rect + 8px slop, **or** it
  owns the open popup (a `select`'s menu lies below its rect), **or** it holds a
  pointer capture (a slider drag off the track), **or**, on a click edge only, it
  is a children-as-data kind (a `context_menu` may lay rows past its rect).

Idle frames replay a per-node memo so `hud_hit` stays continuous at zero cost.

### The escape hatch

A component that declares `M.hit_shape` never crosses at all:

| `hit_shape` | Walker behaviour | Used by |
|---|---|---|
| `"rect"` | hover claims; click fires `action` and/or toggles a bool `bind` | `button`, `tile` |
| `"none"` | never claims, never interacts | `sprite`, `tooltip`, `rune_corners`, `gauge` |
| *(absent)* | dispatch `M.hit` for a verdict | everything else |

`hit_shape` is probed **once** at library build. A typo there is a build error.

---

## The four channels

The Lua↔Rust boundary is exactly four channels, and scalars only
(`bool` | `number` | `text`). `mlua` never leaves `flicker-script`.

| # | Channel | Direction | Carries |
|---|---|---|---|
| 1 | **Tree** | Lua → Rust, once | `M.tree()` → a `UiNode` tree, parsed and cached |
| 2 | **Model** | Rust → Lua, per frame | the `ValueMap` a node's `bind` / `*_bind` reads |
| 3 | **Results** | Rust ← walk | fired `action`s, edited `bind` values, `hud_hit`, drag payload |
| 4 | **Component props / commands** | Rust ↔ Lua, bounded | `M.draw(cmds, rect, props)` and `M.hit(...)` → `HitVerdict` |

Per-node wiring props:

| Prop | On | Does |
|---|---|---|
| `bind` | inputs | two-way: reads the Model key, writes edits into results |
| `action` | buttons, `context_menu` rows | fires an event name into results (`results.is_on("id")`) |
| `text_bind` | `text`, `button` | caption comes from a Model key's pre-formatted string |
| `visible_bind` | any node | the node + subtree are placed only when the key is truthy |
| `enabled_bind` | any node | draws dim & inert; the click edge is pre-gated on it |
| `style_bind` | any styled node | a Model key holding a **dotted style path** — restyle by state |
| `color` / `color_bind` | `text` | dotted colour path (text's channel), literal or from a key |
| `live` / `live_bind` | `rtt` | whether the slot renders a fresh target this frame |
| `focus_group` | `slider`, focusable rows | a shared Model key naming the member that holds focus |
| `tab_group` / `nav_ordinal` | any focusable | directional-nav group + order (d-pad / arrows) |
| `drag_kind` / `drag_id` | **any** node | makes it a drag source; the payload rides results |

**Rust owns both ends of every name.** The Lua names a wire; the scene's `model()`
must publish it and its `update()` must read it back. Nothing checks the spelling
— see [Sharp edges](#sharp-edges--guardrails).

### The bind echo (a contract, not a convenience)

Every placed control with a `bind` reports its effective value **every frame**,
whether or not it was touched, with its kind's own absent-value default:

| Kind | Idle echo |
|---|---|
| `checkbox` / `toggle` / `tile` | the model's bool, default `false` |
| `slider` / `stepper` | the model's number, default the node's `min` |
| `list` | the offset, default `0` |
| `tabs` | the model's text, else the **first child's `value`** |
| `radio` / `pill_toggle` / `select` | the model's text if present, else nothing |
| `text_field` | the model's text, default `""` |

So a scene may read its result keys unconditionally. Clicks always win: the hit
pass runs first and the echo only fills keys nothing wrote.

---

## Layout

### `size` is main-axis length

A `row` flows left-to-right; a `cell` flows top-to-bottom. A child's `size` is its
length **along the parent's flow axis** — width in a row, height in a cell.

```lua
Row  { size = 60, children = { Button { size = 120 }, Button { size = 120 } } }  -- 120px WIDE
Cell {           children = { Button { size = 46  }, Button { size = 46  } } }  -- 46px TALL
```

`grow = n` shares the leftover main-axis space by weight. `Stack { grow = 1 }` is
the idiomatic spacer.

| Field | Effect |
|---|---|
| `pad` / `pad_x` / `pad_y` | inset all sides / horizontal only / vertical only (per-axis wins) |
| `gap` | space between children along the main axis |
| `align` | **container** prop for the CROSS axis: `stretch` (default) / `start` / `center` / `end` |
| `width` / `height` | fixed pixel box (anchored or measured) |
| `width_frac` / `height_frac` | fraction of the parent rect (`1.0` = full) |
| `aspect` | lock width to height × ratio (keeps a sprite square) |
| `layer` | sub-layer, **accumulated down the subtree** — how a popup sits over a scrim |

A bare `text` with no `size` reserves `text_size + leading` (leading defaults to
10). That is why no template does row-height arithmetic any more.

### Anchoring

Inside a `screen` or `stack`, children do not flow — each is placed by its own
`anchor` (`top_left`, `top`, `top_right`, `left`, `center`, `right`,
`bottom_left`, `bottom`, `bottom_right`) plus `offset = { dx, dy }`.

### `grid`

`grid` is the 2-D generalisation. Tracks are a CSS-ish string; children place
themselves or auto-flow:

```lua
Grid { cols = "30 1fr 30", rows = "52 1fr 58", col_gap = 0, row_gap = 0,
       children = { Cell { col = 1, row = 1, col_span = 1 } } }
```

Track tokens: `auto` · `<px>` · `<n>fr`. An unparseable token degrades to `auto`
so a typo never shifts every later column.

---

## Component catalog

Every kind a tree may legally name. Anything else is a typo the
[`unknown_kinds` gate](#the-drift-gates) turns into a test failure.

### Structural (Rust primitives — the walker lays out *and* draws these)

| Kind | Purpose | Common props |
|---|---|---|
| `screen` | overlay root; children placed by `anchor` | `id`, `on_<signal>` |
| `cell` | THE box — flows children top-to-bottom; with a `style` it draws a background | `size`, `grow`, `pad*`, `gap`, `align`, `style` |
| `row` | flows children left-to-right | same as `cell` |
| `stack` | overlays children by anchor; also the `grow` spacer | `grow`, `size` |
| `grid` | 2-D track grid | `cols`, `rows`, `col_gap`, `row_gap`; children: `col`, `row`, `col_span`, `row_span` |
| `rtt` | reserves an offscreen viewport rect for the frame graph | `id`, `source`, `style`, `inset`, `live`/`live_bind`, `tint` |
| `text` | one line | `text` \| `text_bind`, `prefix`, `text_size`, `color`/`color_bind`, `font` (`body`/`display`/`label`/`rune`), `align`, `italic`, `bold`, `tracking`, `wrap` |
| `option` | **data only**, never drawn — a child entry of `select`/`pill_toggle`/`tabs` | `value`, `label` |

### Interactive (each is `ui/<kind>.lua`)

| Kind | Purpose | Common props |
|---|---|---|
| `button` | clickable labelled slab (`hit_shape = "rect"`) | `id`, `action`, `label`\|`text_bind`, `size`, `label_size`, `style`, `style_bind`, `enabled_bind` |
| `tile` | grid slot that lights when filled (`hit_shape = "rect"`) | `label`, `bind`, `style`, `style_off` |
| `checkbox` | labelled tick box | `bind`, `label`, `label_size`, `label_x`, `style` |
| `toggle` | on/off pill | `bind`, `size`, `style`, `enabled_bind` |
| `radio` | one-of-many (matches `value`) | `bind`, `value`, `label`, `style` |
| `slider` | drag a value in a range | `bind`, `min`, `max`, `size`, `label_w`, `value_w`, `slider_h`, `decimals`, `plus`, `suffix`, `focus_group`, `style` |
| `stepper` | −/+ around a number | `bind`, `min`, `max`, `step`, `label`, `style` |
| `pill_toggle` | segmented control; `option` children | `bind`, `size`, `style`, `enabled_bind` |
| `tabs` | tab strip; `option` children | `bind`, `tab_active`, `tab_idle` |
| `select` | dropdown; `option` children. Its popup lies **below** the node rect | `bind`, `size`, `style`, `enabled_bind` |
| `context_menu` | right-click action list; rows are children carrying `action` | `style`, children with `label`/`hint`/`action` |
| `list` | clipped, wheel-scrollable column | `bind` (offset), `scroll_speed`, `gutter`, `pad`, `grow` |
| `text_field` | single-line editable text | `id` (focus is held by id), `bind`, `placeholder`, `text_pad`, `style` |
| `gauge` | read-only band gauge with a marker (`hit_shape = "none"`) | `bind`, `lo`, `hi`, `style` |
| `badge` | small status chip | `label`, `tone`, `solid`, `label_size`, `style` |
| `tooltip` | hover/info bubble (`hit_shape = "none"`) | `rune`/`rune_bind`, `name`/`name_bind`, `meta`/`meta_bind`, `rune_color` |
| `sprite` | textured quad (`hit_shape = "none"`) | `tex`, `alpha`, `aspect` |
| `rune_corners` | the four carved corner glyphs (`hit_shape = "none"`) | `style`, `glyph_size`, `tl`/`tr`/`bl`/`br` |

> `list`'s *layout* (the offset shift and the viewport clip) is a walker
> primitive; its *draw* (backdrop + scrollbar) and *hit* (claim + wheel→offset)
> are `ui/list.lua`. The walker hands it the measured `content_h`, so the bar can
> never disagree with the placement.

---

## Writing a component

One kind = one file = one geometry. Adding one is additive: no enum, no walker
edit.

```lua
-- Alpha/content/sensorium/scripts/ui/knob.lua
local core = require("ui.core")
local knob = {}

-- Optional: declare a trivial shape and the walker answers in Rust, zero crossings.
-- knob.hit_shape = "rect"   -- or "none"

function knob.draw(cmds, r, props)
  local s = props.style or {}
  core.panel(cmds, r, {
    fill   = core.first_color(s, { "hover_fill", "fill" }, { 0.1, 0.1, 0.1, 1 }),
    radius = core.jnum(s, "radius", 3),
    layer  = props.layer,
  })
  core.text(cmds, r.x + r.w * 0.5, r.y, props.label, 14,
            core.first_color(s, { "label" }, { 1, 1, 1, 1 }), "center", "label", props.layer)
end

function knob.hit(mx, my, r, props, click, down)
  local over = core.point_in(mx, my, r)
  local v = { hit = over }
  if over and click then v.value = not (props.bind_value == true); v.activate = true end
  return v
end

return knob
```

Then register it in `UI_COMPONENT_MODULES` (`flicker-widgets/src/lib.rs`) — one
line, `("ui.knob", include_str!(".../ui/knob.lua"))` — and add its style block to
`ui_elements.json`. Registration is what makes `knob` a legal kind.

### `M.draw(cmds, rect, props)`

Emit plain-data HudCommands into `cmds`. You get the **resolved** rect; you never
compute a position from a parent. Emitters in `ui.core`:

| Emitter | Draws |
|---|---|
| `core.panel(cmds, r, s)` | the SDF rounded-rect: fill + 2-stop gradient + border + feather |
| `core.panel_bg(cmds, r, s, layer)` | a container backdrop from a style block's standard keys |
| `core.rect(cmds, r, c, layer)` | flat tinted rect (1px rules, ticks) |
| `core.sprite(cmds, r, tex, alpha, layer)` | textured quad |
| `core.text(cmds, x, y, str, size, c, align, font, layer)` | one line in a face role |
| `core.caret(cmds, x, y, w, h, prefix, size, c, layer, font, max_x)` | a **measured** caret — the render bridge shapes `prefix` and places the bar after it |

Helpers: `core.point_in(px, py, r)` · `core.first_color(s, keys, dflt)` (the alias
chain — `hover_top` → `hot` → `fill_top` → `cell` → `fill`) · `core.jnum(s, k,
dflt)` · `core.fmt_val(v, props)` · `core.rgba(c)`.

`core.caret` exists because caret position **must** be measured, never estimated
from character count. Do not reintroduce `chars × advance`.

### What arrives in `props`

Your own node props, plus these walker-owned fields:

| Prop | Meaning |
|---|---|
| `style` | your resolved style block (already `$token`-expanded rgba) |
| `style_off`, `tab_active`, `tab_idle` | the other named style paths, also resolved to blocks |
| `label` | the node's display text (`text_bind` → `text` → `label`, prefixed, stringtable-resolved) |
| `hot` | pointer over the rect, or this node holds keyboard focus |
| `focused` | keyboard focus by `id`; for a `focus_group` member, the group key currently holds this `bind` |
| `enabled` | the `enabled_bind` verdict |
| `layer` | always `0` — the walker lifts your whole command range afterwards |
| `mx`, `my` | pointer, for sub-region hover |
| `gap`, `pad_x`, `pad_y` | the node's own layout metrics |
| `bind_value` | the effective bound value (results override model); **absent** when unset |
| `open` | this node owns the open popup |
| `captured` | this node holds pointer capture (hit calls only) |
| `wheel` | this frame's wheel tick (hit calls only; never cached, never fingerprinted) |
| `content_h` | `list` only — the walker-measured content height |
| `children` | segmented controls: each child's props, plus its `action` |

**Display-text props resolve through the stringtable before they reach you.**
The list is fixed: `label`, `text`, `title`, `subtitle`, `footer`, `placeholder`,
`hint`, `name`, `meta`, `prefix` — on the node *and* on its data children. Bind
values and user text (a chat buffer) never resolve.

### `M.hit(mx, my, rect, props, click, down)` → `HitVerdict`

`click` is this frame's press edge, **already gated** on the node's enabled state.
`down` is the raw held state. Return `true`/`nil`/a table:

| Field | Effect |
|---|---|
| `hit` | pointer is in your **tight** region → claims `hud_hit` (+ the idle memo) |
| `value` | new value for the node's `bind` |
| `activate` | fire the node's `action` |
| `activate_child = i` | fire child `i`'s `action` — **1-based**, matching `props.children[i]` |
| `capture = true/false` | grab pointer capture; release on button-up is the walker's generic rule |
| `open = true/false` | open/close this node's popup (`close` only applies if you still own it) |
| `focus = true` | take keyboard focus (needs an `id`; clearing is the walker's clicked-frame rule) |
| `group_focus` | write this node's `bind` into its `focus_group` key |

A non-scalar `value` is a contract error. Geometry must be **one function shared
by `draw` and `hit`** (see `track_rect` in `ui/slider.lua`) — that is the whole
point of one control living in one file.

One caveat worth knowing: `M.hit` reads the props from your node's **last real
draw**, with only `bind_value`/`open`/`captured`/`wheel` patched live. On the
exact frame a geometry prop changes, hit lags draw by one frame — deliberate: the
user clicked what they *saw*.

---

## Templates are data

A template is a whole surface, parameterised. Instantiate one by putting
`template = "<name>"` on a node, its parameters as sibling fields, and its named
`slots` filled with your content:

```lua
{ template = "window",
  title = "$set_settings", w = 1180, h = 726, style = "settings",
  close_action = "settings_close", footer_h = 58,
  slots = {
    content = { --[[ your body nodes ]] },
    footer  = { --[[ your button nodes ]] },
  },
}
```

`expand(tree, &builtin_templates())` resolves every template node **once**, before
the tree is cached — zero per-frame cost. A template-free tree passes through
unchanged.

### The protos

Six live in `Alpha/content/sensorium/resources/ui_templates.json`:

| Template | Shape | Slots | Key props |
|---|---|---|---|
| `window` | framed window: title bar (+ close ✕) · body well · optional footer | `content`, `footer` | `title`, `subtitle`, `w`/`h` or `w_frac`/`h_frac`, `style` (path prefix), `title_h`, `title_pad`/`_pad_y`, `title_size`, `has_close`, `close_action`/`_style`/`_label`, `body_style`, `body_pad`, `footer_h`, `footer_pad`/`_pad_y`, `footer_gap`, `footer_style` |
| `workbench` | full-screen bench: header · tab strip · body (viewport + rail) · footer | `header`, `tabs`, `viewport`, `rail`, `footer` | `style`, `w_frac`/`h_frac`, `header_h`/`_pad`/`_gap`/`_style`, `tab_*`, `body_pad`/`_gap`, `footer_*`, `footer_btn_h` |
| `popup_panel` | the gothic popup slab: title · subtitle · divider · items · footer | `items` | `id`, `panel_style`, `panel_w`/`_pad`/`_gap`, `layer`, `title`/`_size`/`_color`, `subtitle*`, `divider`, `items_gap`, `footer*` |
| `popup_menu` | scrim + `popup_panel`, anchorable | `items`, `muse` | everything `popup_panel` takes, plus `overlay_style`, `anchor`, `offset_x`/`offset_y` |
| `choice_dialog` | centred question modal with confirm/cancel built from props | `buttons` (replaces the prop-built stack) | `title`/`_size`/`_color`, `message*`, `subtitle_bind*`, `confirm_label`/`_action`/`_variant`, `cancel_label`/`_action`/`_variant`, `panel_w`/`_pad`, `btn_h`/`_gap`/`_label_size`, `overlay_style`, `divider_style` |
| `side_by_side_rtt` / `quad_rtt_view` | one or two framed RTT viewports | — | `source`/`left_source`/`right_source`, `style`, `gap`, `live`/`live_bind`, `tint`, `width`/`height`, `quad_id` |

Three stay **Rust builders** (their output is computed, not templatable — the
`frame` grid needs generated track strings, `option_grid` chunks rows):

| Builder | Slots | Key props |
|---|---|---|
| `frame` | `center`, `n`, `s`, `w`, `e`, `nw`, `ne`, `sw`, `se` | `style` (path prefix, default `settings`), `w`/`h` or `w_frac`/`h_frac`, `edge` (default **30** — the corner-rune clearance), `n_size`/`s_size`/`w_size`/`e_size`, `closable`, `close_action`/`_style`/`_label` |
| `card` | `content` | `title`, `subtitle`, `disabled`, `style` (default `menu.panel`), `pad`, `gap`, `header_gap`, `title_size`, `subtitle_size` |
| `option_grid` | `cards` | `cols` (default 4), `heading`/`_size`/`_color`, `subtitle*`, `hint*`, `gap`, `grid_gap`, `well_pad`, `well_style` |

`window` and `workbench` are themselves `{"template": "frame"}` nodes — protos
compose protos, up to `MAX_TEMPLATE_DEPTH = 8`.

### The proto schema

A proto is a `UiNode` tree as JSON with four substitution forms, applied
**JSON-level, before any parse**:

| Form | Meaning |
|---|---|
| `"@name"` / `"@name=default"` | the whole value, **natively typed**. Prop absent + no default ⇒ the **key is removed** (or the array element dropped). A default is typed by shape: `true`/`false` → bool, numeric → number, else string; `=` alone → `""`. |
| `"…@{name}…"` / `"@{name=default}"` | string **interpolation** — e.g. `"modal.buttons.variants.@{confirm_variant=primary}"`. Any referenced prop absent with no default removes the **whole** value. |
| `"when": "@name"` / `"when": "!@name"` | gate the node. Truthy = present, not `false`, not empty text (0 *is* truthy — presence is the signal). Passing strips the `when` key. |
| `{ "component": "slot", "name": "x", "children": [ … ] }` | splice the instance's `x` slot; its own `children` are the **fallback** when the slot is empty |

Plus `"when_filled": true` on any node: drop it entirely unless some slot beneath
it produced *instance* (not fallback) content — that is `window`'s
omit-the-footer-when-empty.

Two **pseudo-props** are injected into the substitution context because the
parser consumes them structurally and a proto could never see them otherwise:
`@anchor` (the instance's anchor, as its name) and `@id` (always present, possibly
empty — the data twin of a builder's `id_prefix`, used for `"@{id}_left"` child
ids). A real prop of either name wins.

`$token` strings pass through **untouched** — the stringtable resolves them at
draw, never here.

### Instance placement

A template node's own `anchor` / `offset` / `size` / `grow` / `width` / `height` /
`id` / `visible_bind` overlay onto the built root **only where the root left them
default**. So `{ template = "card", anchor = "center", width = 400 }` pins and
sizes the instance, while a template that sets its own layout keeps it.

---

## Surfaces

A screen declares, as data, which of its subtrees are shown — and drives them
through one helper instead of scattering `m.set("x_visible", …)` through
`update()`.

```rust
fn hello_surfaces() -> Surfaces {
    Surfaces::new(vec![
        Surface::new("sec_video").group("sections").on(),   // radio group, starts shown
        Surface::new("sec_audio").group("sections"),
        Surface::new("inspector").key("has_pick"),          // publishes under a legacy key
        Surface::new("confirm_close").context("Menu"),      // holds an InputContext while up
    ])
}
```

| Call | Effect |
|---|---|
| `set(id, on)` / `show` / `hide` / `toggle` | state |
| `set_exclusive(id)` | show `id`, hide every other member of **its group** (other groups untouched) |
| `is_on(id)` | read |
| `publish(&mut model)` | **the one visibility write per frame** — writes exactly the Model keys the tree's `visible_bind`s read |
| `apply_surface_contexts(&mut route)` | fold this frame's flips into router `PushContext` / `PopContext` |
| `visibility_diff()` | the raw diff, for tests and tooling |

There is deliberately **no second visibility path**: a surface toggled by the
helper is indistinguishable from a key set by hand, and republishing an unchanged
state leaves every fingerprint — and therefore the draw cache — untouched.

Contexts un-push **LIFO**. Hiding a surface while a later-shown context surface is
still up cannot remove from the middle of the router's stack, so it *pops-to* its
own context (taking the newer ones with it) and warns. Declare-and-hide in LIFO
order and that path never fires.

`apply_surface_contexts` and `visibility_diff` share one baseline and each advances
it — use exactly **one** of the two per frame.

**Not a surface:** a pushed scene. Pause / settings / confirm are stack scenes
(the scene manager updates only the top, which is what makes a pause actually
pause). A dialog popping up *inside* one screen is a surface.

---

## Intents

"Did the user intend to open the menu" is **screen data**. A screen's root node
declares it:

```lua
Screen { id = "hello", on_menu = "pause_open", on_cancel = "hello_close", … }
```

- The prop suffix is the signal's stable name in snake_case: `on_attack_light`
  names `AttackLight`. There is no second name table — the suffix folds onto the
  serde variant names.
- **Only the root declares.** A child's `on_*` props stay ordinary component props.
- An unknown suffix, a non-string value, or an empty name is **warned and
  skipped** — the vocabulary-gate philosophy: a typo is a log line, never a silent
  dead binding.

Wiring, per frame:

```rust
let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
    .with_nav(tree)                 // optional: tab_group / nav_ordinal traversal
    .with_intents(&self.intents);
Router::dispatch(&events, &mut chain, &mut self.route);
for name in walker.take_fired() {
    results.set(name.as_str(), true);   // folds in exactly like a click
    self.fired_sigs.push(name);
}
```

- A declared binding **owns** its signal: `on_cancel` takes precedence over the
  walker's built-in back-out, and both edges are consumed so it never leaks to
  gameplay.
- The pointer gate still runs first — a click the HUD owns is swallowed, never
  re-fired as an intent.
- **The `sig_*` mirror:** each fired name is republished into the next Model as
  `sig_<name> = true`, for scripts to observe. It is **transient by contract** —
  exactly one publish, then dropped. A script that needs the fact longer latches
  it itself. Absent keys read falsy in Lua, so no `false` is ever written.

Two shapes you will meet:

- `on_menu = "pause_open"` — every game screen. The scene maps the fired name onto
  `Transition::Push(PauseScene…)`.
- `on_cancel = "menu_back"` — a tier-2 launcher page, wired to the same result the
  BACK button fires, so Escape and the button are one code path.

A layer **above** the walker that consumes a signal still starves the intent,
structurally: pocclusters' chat swallows `Menu` while it owns the keyboard.
TextEntry's one-way hand-off stays deliberately outside the intent map.

---

## Strings

Every UI display string is a token in `Alpha/content/data/stringtable.json`:

```json
{ "menu_quit": { "en-us": "QUIT" } }
```

| Rule | Behaviour |
|---|---|
| `$token` | resolves to the active locale's text |
| `$$` | escapes a literal `$` (`"$$5.00"` → `"$5.00"`) |
| a **miss** | renders the token **raw** — visible on screen, greppable — and warns once |
| bad JSON on reload | keeps the previous table; a bad edit never blanks the UI |
| locale | `GameSettings.language` (empty ⇒ `en-us`), per-token fallback to `en-us` |

Resolution happens at the **draw boundary only**, over the fixed
`DISPLAY_STR_PROPS` list (`label`, `text`, `title`, `subtitle`, `footer`,
`placeholder`, `hint`, `name`, `meta`, `prefix`), on a node and on its data
children. Never on a `bind` value or user text.

A (re)load bumps a generation counter that every node fingerprint folds in — so a
language switch redraws exactly the nodes showing text.

**Composed strings** have no format language, on purpose. Two options: make the
whole composed string a token, or pre-format it in Rust and ride a Model bind.
The launcher's `"#scenes scenes available"` is the one gate-exempt case, with an
in-file comment saying so.

The `$` sigil is shared with the palette and disambiguated by domain: a **colour**
prop resolves against `theme.tokens` in `ui_elements.json`; a **text** prop
resolves against the stringtable.

---

## The drift gates

Two pure functions turn the two classic silent failures into build failures. A
new screen wires both into its own test — this is the step that makes a screen
*shipped*.

```rust
#[test]
fn the_hello_screen_is_clean() {
    let script = ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES)
        .expect("hud_hello.lua loads");
    load_ui_json(&script, HUD_UI_ELEMENTS);
    // publish any data globals `tree()` reads (ROSTER, HAB, MENU, …) first
    let tree = script.ui_tree().expect("tree builds").expect("script exposes tree()");
    let tree = expand(tree, &builtin_templates());

    assert!(flicker::ui::unknown_kinds(&tree).is_empty(),
            "hud_hello names unknown kinds: {:?}", flicker::ui::unknown_kinds(&tree));
    assert!(flicker::ui::raw_display_literals(&tree).is_empty(),
            "hud_hello ships raw display literals: {:?}", flicker::ui::raw_display_literals(&tree));
}
```

**`unknown_kinds(tree)`** — every component kind the engine does not know, plus
any `template` node that never expanded (reported as `template:<name>`). Without
it, a stale kind anchor-overlays its children and draws nothing: an invisible
hole.

**`raw_display_literals(tree)`** — every non-`$token` display literal. Exempt, and
these are the only exemptions: `$`-prefixed values, empty strings, a prop whose
node also carries its `<prop>_bind` twin, single glyphs (`✕`, `·`, `‹`), literals
with no alphabetic character, and pure `%`-format strings (`"%d"`, `"%.2f%%"`).

Build the tree the test walks **the same way the scene does** — same globals, same
`expand`. A gate that walks a fixture instead of the real tree proves nothing
about what ships.

---

## The mode launcher

The launcher is two tiers of ordinary stack scenes; `menu.lua` stays entirely
realm-agnostic.

**Root** — `EXPLORE THE WORLD` (Adventurer) · `BUILD THE WORLD` (DM) ·
`DEVELOPER MODE` · then any realm-less entry (Click Trainer) · `SETTINGS` · `QUIT`.

A mode button fires `mode_<realm>` → `Transition::Push(MenuScene::for_mode(realm))`.
The root stays frozen beneath, so Pop restores it — view *and* focus — without
re-entering.

**Membership is a tag list**, because tools are shared across modes:

```rust
SceneEntry::new(id, label, variant, factory)
    .with_realm(REALM_DEVELOPER)      // repeatable
    .with_realm(REALM_ADVENTURER)
    .with_info(SceneInfo::new(name, mode, region, desc, meta))
```

| `realms` | `info` | Where it renders |
|---|---|---|
| empty | `None` | a plain launch button on the **root** popup |
| tagged | `Some` | a **panel row** on that realm's tier-2 page |
| tagged | `None` | a plain launch button on that realm's tier-2 popup |

`SceneInfo::mode` is a pure display string — it does **not** decide placement.
`realms` does.

**Back** is one path: the `menu_back` button and `on_cancel = "menu_back"` on the
tier-2 root both produce the same result name, and the scene has one
`Transition::Pop` arm. Pause → MAIN MENU uses `ReplaceRoot`, which unwinds
everything.

Three page-level fields the shell publishes into `MENU`, which is all `menu.lua`
knows about tiers: `mode` (non-empty ⇒ this is a tier-2 page ⇒ declare
`on_cancel`), `note` (a `$token` riding the popup footer — the DM page's
under-construction line), `panel_head` (`false` drops the scene panel's header
block, so the Adventurer page shows exactly its entry).

---

## Sanctioned exceptions

Five places deliberately do not follow the rules above. Each is a decision, not
drift — do not "fix" them without a ruling, and do not copy them into new work.

| Exception | Why |
|---|---|
| **`text` and styled-container backgrounds stay Rust primitives** | There is no `ui/text.lua` by ruling. `text` is the most numerous kind in a real tree, and container backdrops are the shared `draw_panel_bg`. A Lua component draws the same backdrop through `core.panel_bg`, so both paths emit identical commands. |
| **`flicker-world`'s `world_ui.lua` + `widgets.lua`** | The last immediate-mode control surface: per-epoch structural slider rows plus a duration-weighted timeline scrubber with multi-key-bound geometry — no walker channel expresses it, so converting is a redesign. It is the ONE remaining consumer of the trimmed legacy `Widgets` global (slider/stepper/dropdown/button); `widgets.lua` dies with that conversion. |
| **`logo.lua` stays immediate** | Its timeline needs Model-driven texture switching plus per-frame fit-scale geometry. It returns no `M.tree()`. |
| **loomforge (and the floating chat panel) rebuild the tree every frame** | Load-bearing: the bench mutates its document mid-frame and its node ids encode *filtered* list positions read against post-mutation state, so a retained tree would need revision plumbing across every mutation funnel. This costs nothing: cache identity is **structural** (an id, else the parent key folded with kind + sibling index), not the address of a retained node, so a rebuilt-but-identical tree replays at `redraw_nodes == 0`. Its `UiIntents` are still collected **once** — root props are static even when the tree is not. |
| **Data-coloured scene panels** | poc-chemistry's seed swatches + event log and pocepochs' legend + element panel draw in the scene, because their per-row colours are DATA (`element_rgb`). The walker's colour channel is dotted style paths **by design** — one palette, one place. A per-datum colour has no path. |

---

## Sharp edges & guardrails

### Silent failures — the ones to grep for first

The system fails loudly for a mistyped **component kind** (the `unknown_kinds`
gate), a mistyped **`on_<signal>`** (warn + skip), a mistyped **`hit_shape`**
(build error), and a missing **string token** (renders raw + warns). Everything
below is still silent. When a control "does nothing", check these in order:

- **A `bind` / `action` / `text_bind` name is a plain string with no compiler
  check.** A typo'd `bind` still echoes a default under the typo'd key, so the
  control renders and moves nothing. Grep the scene's `.rs` for the key.
- **A `visible_bind` key that no `Surfaces` declaration (or `model()`) publishes
  makes the subtree permanently invisible**, with no warning. Nothing cross-checks
  the tree's `visible_bind` names against the declaration.
- **A misspelled slot name in a template instance silently drops that content.**
  `slots = { contents = … }` for a proto that splices `content` renders an empty
  region and logs nothing.
- **A misspelled `@prop` inside a proto silently removes the key it was the value
  of** — that is the documented "absent with no default ⇒ removal" rule doing its
  job, and it looks identical to a typo.
- **A dotted `style` path that resolves to nothing draws unstyled.** Copy paths
  from an existing scene or from `ui_elements.json`.
- **A typo'd `template` name expands to an empty `screen`** (warn-only), and
  `unknown_kinds` cannot see it — the node is gone by then. (Forgetting `expand`
  entirely *is* caught: the gate reports `template:<name>`.)

### Guardrails

- **Never round-trip `ui_elements.json`.** It is hand-formatted with hundreds of
  significant floats; a serialize/parse cycle once produced a 2600-line spurious
  diff. Add a block as an exact-string insertion.
- **No inline rgba anywhere.** Every colour is a `$token` under `theme.tokens`,
  referenced by a style block, referenced by a node's dotted path. A template
  builder or proto that touches a colour has forked the palette.
- **Never write display copy in a tree.** It goes in the stringtable as a token,
  or it is pre-formatted in Rust and rides a Model bind.
- **Build and `expand` the tree once, cache it, then walk the cache.** New screen
  = data. New surface = a JSON proto. New control = one Lua file. Only a genuinely
  new *primitive* touches the walker.
- **Geometry lives once.** A component's `draw` and `hit` share one geometry
  function. Two copies is the duplication the whole per-control migration removed.
- **Enhance in place.** One walker (`flicker-widgets`), one parser
  (`flicker-script`), one style source (`ui_elements.json`), one template file
  (`ui_templates.json`), one stringtable. Extend them; never fork a parallel path.

---

*Walker: `Alpha/crates/frontend/flicker-widgets/src/component.rs` · templates:
`…/template.rs` + `Alpha/content/sensorium/resources/ui_templates.json` ·
surfaces: `…/surfaces.rs` · intents: `…/intents.rs` · strings: `…/strings.rs` +
`Alpha/content/data/stringtable.json` · node schema + the Lua seam:
`Alpha/crates/scripting/flicker-script/src/lib.rs` · components:
`Alpha/content/sensorium/scripts/ui/*.lua` · styles + palette:
`Alpha/content/sensorium/resources/ui_elements.json` · screens:
`Alpha/content/sensorium/scripts/*.lua`.*
