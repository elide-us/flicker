# Authoring a Prism screen

**A screen is a Lua table. Every reusable surface is one entry in
`ui_templates.json`. The Rust engine owns everything else — layout, each
control's draw and hit, caching, and routing.**

That split is what this system buys you: a new screen is *data*, a new surface is
*one JSON proto*, and neither needs a recompile. This guide is what a person
needs to author one without opening the walker.

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
8. [Writing a component](#writing-a-component) — a `draw_<kind>` arm
9. [Templates are data](#templates-are-data) — `ui_templates.json`
10. [Surfaces](#surfaces) — declaring what a screen shows
11. [Intents](#intents) — the UI *is* the input declaration
12. [Orchestrations](#orchestrations) — signal → surface, as data
13. [Workflows](#workflows) — an Orchestration with an ordinal
14. [Strings](#strings) — `$token`
15. [Where data lives](#where-data-lives)
16. [The drift gates](#the-drift-gates) — what a new screen must pass
17. [The mode launcher](#the-mode-launcher)
18. [Sanctioned exceptions](#sanctioned-exceptions)
19. [Sharp edges & guardrails](#sharp-edges--guardrails)

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
| `component` kind (`cell`, `text`, `button`) | the walker places it, then draws + hit-tests it in `component.rs` |
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
| **Surface** | any subtree of a screen gated by a `visible_bind`, declared in the scene's `Surfaces` list. |
| **Orchestration** | *"a layout that changes based on signals"* — the `fired name → surface op` table, as data. `OrchestrationRule` over a `Surfaces`. |
| **Workflow** | an Orchestration **with an ordinal**: ordered step surfaces + needs/yields gates + the document contract. Definitions are data (`ui_workflows.json`). |
| **Step** | one stop of a Workflow: an id, a `$token` rail title, its surface, and its `needs`/`yields` document contract. |
| **Template** | a whole surface as pure DATA: a parameterised tree in `ui_templates.json`. |
| **Component** | the logic owner, a configurable black box. *Interactive* components are `draw_<kind>` arms in `component.rs`; *structural* ones (`frame`, `card`, `option_grid`) are Rust composition builders. |
| **Primitive** | no logic: the layout resolver, the HudCommand renderers (panel SDF / rect / sprite / **text** / caret / clip), point-in-rect, measure_text. |

**Folded names.** These were renamed; the old ones do not parse:

| Retired | Use |
|---|---|
| `stage` / `StageSlot` | `rtt` |
| `panel` (as a synonym for a styled box) | `cell` (a cell carrying a `style`) — but see below: `panel` is now a **kind of its own** |
| `column` | `cell` |
| `page` | `screen` |
| `scroll` | `list` |
| `element` | survives **only** in the filename `ui_elements.json` |

**`panel` came back, and it is not the old name.** A `panel` is the FOCUSABLE
pane of a multi-panel view: it draws its own backdrop *and* its own focus rim
from the `panel` style block, choosing between them by the focus the walker
holds. A styled box that is not a pane is still a `cell`.

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
let script = ScriptHost::from_file(HUD_SCRIPT)?;
load_ui_json(&script, HUD_UI_ELEMENTS);            // the `UI` global for the script
let tree = expand(script.ui_tree()?.unwrap(), &builtin_templates());  // templates → nodes
self.intents = UiIntents::of(&tree);               // collects `on_menu`
self.tree = Some(tree);
self.script = Some(script);                        // RETAINED: it owns the data globals
```

`script` is kept alive deliberately — it owns the `UI` global and any data
globals the screen publishes. Drawing needs nothing from it: every control draws
in the engine.

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
let frame = run_ui(&tree, &model, &self.styles, &snap, &mut self.ui_state);

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

**What Rust does per frame, in order** (`run_ui`):

1. `resolve` — lay the tree into rects, skipping `visible_bind`-false subtrees.
2. hit pass — each candidate component answers a `HitVerdict`; verdicts fold
   into `results` and `UiState`.
3. `fold_typed` — this frame's keyboard text into the focused node's `bind`.
4. `echo_binds` — every placed control with a `bind` reports its effective value.
5. draw pass — each node whose fingerprint changed redraws; the rest replay from
   the cache. Commands are lifted onto the node's sub-layer.
6. `rtt` pass — each `rtt` node reserves an `RttSlot` for the scene's frame graph.

Reference screens to copy from, smallest first: `flicker-sablework`
(`sablework_console` — one proto, one `build_tree`, the smallest complete
example) · `flicker-assetpipeline` (`clayworks_bench` — the whole bench:
protos, surfaces, a [Workflow](#workflows)) · `flicker-quartermaster` (the
canonical reference, and the largest).

> **Do NOT copy the `hud_<scene>.lua` screens still in
> `content/sensorium/scripts/`.** They hand-compose their surface out of
> `Cell`/`Row`/`Stack` builders — the pre-convergence idiom. Composition is
> DATA: a scene configures a proto in `ui_templates.json` and emits one
> instance. The remaining legacy screens are tracked for migration, not for
> imitation.

---

## Bounded dispatch

This is the one thing to internalise, because it decides what your screen costs
and what your component may assume.

**A component redraws on change and hit-tests on input. Never per node per
frame.**

### Draw

Every node has a cache entry holding its emitted commands plus a fingerprint of
every input its draw reads:

- the node's rect, layer, enabled flag, clip;
- its resolved style block (by **content**, so a rebuilt or hot-reloaded styles
  tree invalidates correctly);
- the Model/results values under the keys it reads — its `bind`, every prop whose
  name ends in `_bind`, and `focus_group`;
- for children-as-data kinds (`tabs`, `pill_toggle`, `select`, `context_menu`),
  its children's props;
- the stringtable generation (a language switch invalidates exactly the text);
- pointer position, **only** for interactive components (a structural box never
  consults the cursor), and only rounded to whole pixels, and only while the node
  is hot or open.

Unchanged fingerprint ⇒ verbatim replay, nothing redrawn. A still frame is
`redraw_nodes == 0` — asserted by tests, observable through `UiFrame.stats`.

The naming convention is load-bearing: **a prop ending in `_bind` holds a Model
key**. Name a new one `foo_bind` and the cache covers it the day you author it.

### Hit

Every placed node's hit arm runs every frame, in the engine — recomputing a
verdict costs nothing, so `hud_hit` simply stays continuous while a pointer
rests on a control. That is also what lets an arm reach PAST its own node rect:
a `select`'s popup lies below its field, a `context_menu` may lay rows past its
height, and a captured `slider` keeps mapping the pointer once the drag leaves
the track — none of which the walker's geometry could have pre-filtered for.

(The bounded, candidate-gated dispatch this replaced existed to keep per-frame
crossings into the `ui/<kind>.lua` tier affordable. That tier is gone —
2026-08-10 — and its gate with it.)

### The escape hatch

A kind listed in `rust_hit_shape` needs no hit arm at all — the walker answers
the whole claim generically:

| `HitShape` | Walker behaviour | Used by |
|---|---|---|
| `Rect` | hover claims; click fires `action` and/or toggles a bool `bind` | `button`, `panel`, `tile`, `action_slot` |
| `None` | never claims, never interacts | `sprite`, `tooltip`, `rune_corners`, `gauge` |
| *(absent)* | run the kind's bespoke hit arm for a verdict | everything else |

The two are mutually exclusive: a control with a tight sub-rect region declares
neither shape and owns its arm (`rust_owns_hit`). Getting that wrong is a test
failure, not a silent widening.

---

## The four channels

The Lua↔Rust boundary is exactly four channels, and scalars only
(`bool` | `number` | `text`). `mlua` never leaves `flicker-script`.

| # | Channel | Direction | Carries |
|---|---|---|---|
| 1 | **Tree** | Lua → Rust, once | `M.tree()` → a `UiNode` tree, parsed and cached |
| 2 | **Model** | Rust → Lua, per frame | the `ValueMap` a node's `bind` / `*_bind` reads |
| 3 | **Results** | Rust ← walk | fired `action`s, edited `bind` values, `hud_hit`, drag payload |
| 4 | **Component props / commands** | Rust ↔ Rust | `draw_<kind>(rect, props, out)` and the hit arms → `HitVerdict` |

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
| `tabs` | the model's **number**, else the **first child's numeric `value`** |
| `pill_toggle` / `select` | the model's **number** if present, else nothing |
| `radio` | the model's **text** if present, else nothing |
| `text_field` | the model's text, default `""` |

So a scene may read its result keys unconditionally. Clicks always win: the hit
pass runs first and the echo only fills keys nothing wrote.

### AN INDEX IS A NUMBER

`tabs`, `pill_toggle` and `select` pick a position in an **ordered collection**,
so their `option` children carry a **numeric** `value` and the bind reports a
number — in the roster, in the node, on the bind, in `results`, and in the
scene's own field. Author `value = 0, 1, 2 …`, publish
`m.set(bind, index as f64)`, and read `results.number(bind)`.

A non-numeric option value is **not** accepted as an alternative spelling: the
component skips it, `results` stays empty, and a warning names the node and the
offending type. Accepting both representations is worse than picking the wrong
one — it entrenches the fork instead of fixing it.

`radio` is the exception, and the only one: it matches a **name**, not a
position, so its `value` is text. If what you are picking has a stable name
rather than an order, reach for `radio`; if it has an order, it is a number.

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

### Structural — 8 kinds (Rust primitives: the walker lays out *and* draws these)

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

### Interactive — 23 kinds (each is a `draw_<kind>` arm in `component.rs`)

| Kind | Purpose | Common props |
|---|---|---|
| `button` | clickable labelled slab (`hit_shape = "rect"`) | `id`, `action`, `label`\|`text_bind`, `size`, `label_size`, `style`, `style_bind`, `enabled_bind` |
| `tile` | grid slot that lights when filled (`hit_shape = "rect"`) | `label`, `bind`, `style`, `style_off` |
| `panel` | a focusable PANE: its own backdrop, and its own rim when the walker's panel cursor is on it. No bind, no action — see [multi-panel views](#multi-panel-views--the-recipe) | `id`, `tab_group`, `nav_ordinal`, `pad`, `style` (default `panel`) |
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
| `resource_gauge` | filled-fraction bar — health/mana/stamina/cast (`hit_shape = "none"`) | `bind` (0..1), `tone`, `label`, `readout`/`_bind`, `low`/`_bind`, `style` |
| `action_slot` | hotbar recess: rune, keybind tag, charges, cooldown veil (`hit_shape = "rect"`) | `action`, `rune`/`_bind`, `key`, `charges`/`_bind`, `cd`/`_bind`, `active`/`_bind`, `style` |
| `medallion` | circular portrait well in a named metal ring (`hit_shape = "none"`) | `size`, `ring`/`_bind`, `rune`/`_bind`, `rune_color`, `style` |
| `stat_dot` | one Septisigil gem-dot keyed by prism hue (`hit_shape = "none"`) | `hue`/`_bind`, `glow`, `style` |
| `badge` | small status chip | `label`, `tone`, `solid`, `label_size`, `style` |
| `tooltip` | hover/info bubble (`hit_shape = "none"`) | `rune`/`rune_bind`, `name`/`name_bind`, `meta`/`meta_bind`, `rune_color` |
| *(controller-input icons)* | **not a kind** — a `button` wearing an atlas glyph. Give it `glyph` + `glyph_style` and it draws the icon instead of a label; its activate flash is the ordinary button one, keyed on its `action`, so a click, a pad Confirm and a bound shoulder signal all light it | on a `button`: `glyph` (name, e.g. `lt`), `glyph_style` (normally `pad_glyphs` — atlas, cell map, colours), `glyph_size` |
| `sprite` | textured quad (`hit_shape = "none"`) | `tex`, `alpha`, `aspect` |
| `rune_corners` | the four carved corner glyphs (`hit_shape = "none"`) | `style`, `glyph_size`, `tl`/`tr`/`bl`/`br` |

> `list`'s *layout* (the offset shift and the viewport clip) is a walker
> primitive; its *draw* (backdrop + scrollbar) and *hit* (claim + wheel→offset)
> are `draw_list` + its hit arm. The walker hands it the measured `content_h`, so the bar can
> never disagree with the placement.

---

## Writing a component

One kind = one `draw_<kind>` arm = one geometry, all in
`flicker-widgets/src/component.rs`. Adding one is additive: no new file, no new
tier.

```rust
// 1. component.rs — the draw arm. You get the RESOLVED rect; never compute a
//    position from a parent.
fn draw_knob(r: Rect, props: &Json, out: &mut Vec<HudCommand>) {
    let s = &props["style"];
    push_panel(out, r, first_color(s, &["hover_fill", "fill"], PANEL), jnum(s, "radius", 3.0));
    push_text(out, r.x + r.w * 0.5, r.y, text_of(props, "label"), 14.0,
              first_color(s, &["label"], INK), TextAlign::Center, FontRole::Label, …);
}

// 2. wire it into the dispatch table in `draw_node`:
"knob" => draw_knob(r, &props, out),

// 3. answer its HIT — a trivial geometry in `rust_hit_shape`:
"knob" => Some(HitShape::Rect),
//    …or a bespoke arm in `hit_node` when the tight region is a sub-rect
//    (declare it in `rust_owns_hit`; the two are mutually exclusive).

// 4. add "knob" to RUST_COMPONENT_KINDS in lib.rs — that list IS the legal
//    vocabulary, so this is what makes `knob` a nameable kind.
```

Then add its style block to `ui_elements.json`. The roster gate
(`every_engine_component_is_legal_and_answers_its_hit_in_rust`) holds every kind
to steps 3 and 4 — a control that draws but never answers its hit is not
interactive at all, and that gate is what catches it.

Geometry must be **one function shared by draw and hit** (see `track_rect` for
the slider) — that is the whole point of one control living in one place.

### Colours

A component reads its colours from its **resolved style block** (already
`$token`-expanded rgba). The `const INK / PANEL / SAP / …` fallbacks at the
bottom of `component.rs` are the missing-key floor only, and each is a byte copy
of a named `theme.tokens` entry — pinned by
`component_consts_mirror_their_named_theme_tokens`, so a new one must name its
token (or be declared token-less) or the build fails.

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

Carets are **measured**, never estimated: emit `HudCommand::Text` with the
prefix and let the render bridge place the bar. Do not reintroduce
`chars × advance`.

### The hit verdict

A bespoke hit arm returns a `HitVerdict`. `click` is this frame's press edge,
**already gated** on the node's enabled state; `down` is the raw held state.

| Field | Effect |
|---|---|
| `hit` | pointer is in your **tight** region → claims `hud_hit` |
| `value` | new value for the node's `bind` |
| `activate` | fire the node's `action` |
| `activate_child = i` | fire child `i`'s `action` — **1-based**, matching `props.children[i]` |
| `capture = true/false` | grab pointer capture; release on button-up is the walker's generic rule |
| `open = true/false` | open/close this node's popup (`close` only applies if you still own it) |
| `focus = true` | take keyboard focus (needs an `id`; clearing is the walker's clicked-frame rule) |
| `group_focus` | write this node's `bind` into its `focus_group` key |

A non-scalar `value` is a contract error.

One caveat worth knowing: a hit reads the props from your node's **last real
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

Sixteen general-purpose protos live in
`Alpha/content/sensorium/resources/ui_templates.json` (the file also carries each
bench's own protos — `godmode_*`, `clayworks_*`, `pop_size_dial`, `stat_row` —
which are library for that bench, not vocabulary for a new screen):

| Template | Shape | Slots | Key props |
|---|---|---|---|
| `window` | framed window: title bar (+ close ✕) · body well · optional footer | `content`, `footer` | `title`, `subtitle`, `w`/`h` or `w_frac`/`h_frac`, `style` (path prefix), `title_h`, `title_pad`/`_pad_y`, `title_size`, `has_close`, `close_action`/`_style`/`_label`, `body_style`, `body_pad`, `footer_h`, `footer_pad`/`_pad_y`, `footer_gap`, `footer_style` |
| `tabbed_window` | `window` + a tab strip over the body well | `tabs` (`option` children), `content`, `footer` | everything `window` takes, plus `tab_bind` (default `tab`), `tab_h`, `tab_pad`, `tab_active`, `tab_idle` |
| `workbench` | full-screen bench: header · tab strip · body (viewport + rail) · footer | `header`, `tabs`, `viewport`, `rail`, `footer` | `style`, `w_frac`/`h_frac` (default `1`), `header_h`/`_pad`/`_gap`/`_style`, `tab_*`, `body_pad`/`_gap`, `footer_*`, `footer_btn_h` |
| `workflow` | `workbench` wired to the Workflow runtime + the discard dialog — see [Workflows](#workflows) | `header`, `rail`, `steps`, `inspector`, `footer_extra` | `id`, `on_menu`/`on_cancel`, `style`, `rail_h`/`_pad`/`_gap`/`_style`, `header_*`, `body_*`, `footer_*`, `btn_w`, `next_label`, `discard_title_size`, `overlay_style` |
| `popup_panel` | the gothic popup slab: title · subtitle · divider · items · footer | `items` | `id`, `panel_style`, `panel_w`/`_pad`/`_gap`, `layer`, `title`/`_size`/`_color`, `subtitle*`, `divider`, `items_gap`, `footer*` |
| `popup_menu` | scrim + `popup_panel`, anchorable | `items`, `muse` | everything `popup_panel` takes, plus `overlay_style`, `anchor`, `offset_x`/`offset_y` |
| `choice_dialog` | centred question modal with confirm/cancel built from props | `buttons` (replaces the prop-built stack) | `title`/`_size`/`_color`, `message*`, `subtitle_bind*`, `confirm_label`/`_action`/`_variant`, `cancel_label`/`_action`/`_variant`, `panel_w`/`_pad`, `btn_h`/`_gap`/`_label_size`, `overlay_style`, `divider_style` |
| `game_hud` | screen root with the five anchored HUD regions | `top_left`, `top_right`, `bottom`, `bottom_left`, `center` | `id`, `on_menu`/`on_cancel`, `edge_pad`/`edge_pad_neg`, `side_w`, `bar_h`, `stack_gap`, `slot_gap` |
| `unit_frame` | portrait medallion + name/subtitle + a gauge stack | `gauges` | `tone` (style variant), `pad`, `gap`, `portrait` (medallion size), `ring`, `rune`/`_bind`, `name`/`_bind`, `name_size`, `subtitle` |
| `command_card` | heading + a grid of `action_slot`s | `slots` | `style`, `pad`, `gap`, `heading`/`_size`/`_color`, `cols` (track string), `slot_gap` |
| `resource_readout` | one horizontal strip of readout items | `items` | `style`, `h`, `pad_x`, `gap` |
| `paged_menu` | controller-first page/tab CONTAINER: title · page rail (LT/RT, underlined) · tab rail (LB/RB, collapses when `tabs_shown` is off) · one content region · pad-glyph footer legend | `header`, `pages` (`option` children), `tabs` (`option` children), `content` | `eyebrow`, `title`/`title_size`, `page_bind` (default `page`), `tab_bind` (default `tab`), `tabs_shown` (visible-bind key), `pad`, `gap`, `header_h`, `rail_h`/`rail_gap`, `tab_h`/`tab_w`/`tab_gap`, `content_pad`, `legend_h` |
| `default_page` | a page root: the `screen` + its page chrome `frame`, declaring only the signals a page may legally own | `content` | `id`, `on_menu`, `on_page_next`/`on_page_prev`, `on_tab_next`/`on_tab_prev`, `runes`, `style` |
| `multi_view` | **the N-panel arrangement** — a row with ONE `panes` slot spliced as N siblings | `panes` | `gap` |
| `ui_panel` | one pane of a `multi_view`: the `panel` component around a `content` slot | `content` | `id`, `tab_group`, `nav_ordinal`, `title`, `pad`, `placeholder`, `style` |
| `rtt_panel` | the viewport pane: the SAME `panel` node holding one `rtt` (`<id>_rtt`) | — | `id`, `tab_group`, `nav_ordinal`, `source`, `aspect`, `live_bind`, `pad`, `style`, `rtt_style` |

`rtt_panel` defaults `rtt_style` to **`rtt_holder`** — the neutral template-tier
namespace for an offscreen-view slot's chrome. Do not point it back at a
per-bench block; that forks the palette.

Instanced by a shipped screen today: `window` (settings, assetpipeline) ·
`workbench` (assetpipeline) · `choice_dialog` (settings, menu, assetpipeline) ·
`popup_panel` / `popup_menu` (menu) · `default_page` / `paged_menu` /
`multi_view` / `ui_panel` / `rtt_panel` (Populous). The other six — `workflow`,
`tabbed_window`, `game_hud`, `unit_frame`, `command_card` and `resource_readout`
— are authored library with no screen instancing them yet, so there is no worked
example to copy: read the proto in `ui_templates.json` and check its props
against the table above.

### Multi-panel views — the recipe

A bench that shows several things side by side composes **exactly three** protos,
and its Rust never learns which pane is focused:

```rust
// One `multi_view`; its `panes` slot holds N siblings, in reading order.
let mut view = instance(MULTI_VIEW);
view.slots.insert("panes".into(), vec![
    pane(UI_PANEL,  "pop_left",  /* content: a slider instance   */),
    pane(RTT_PANEL, "pop_view",  /* source: "populous_globe"     */),
    pane(UI_PANEL,  "pop_right", /* content: three `stat_row`s   */),
]);
```

1. **`multi_view` is pure layout.** It has no pane count, no privileged kind and
   no focus param: one pane, two or ten is the same proto with a different slot.
2. **A pane's `id` IS its `tab_group`.** That single fact is what makes it a
   panel: the walker's panel cursor (the left stick / `PanelNext`/`PanelPrev`)
   cycles the groups, and the `panel` component draws its own rim from the focus
   the walker holds. A scene passes an id, a group and content — **never a style,
   never a rim, never a `focused` flag**.
3. **Ordinals order the inside of a pane.** The pane itself sits at `nav_ordinal`
   0 so cycling into a group lands ON the pane; its controls take 1, 2, 3 …, and
   the d-pad walks them.
4. **A viewport is a pane like any other.** `rtt_panel` is the identical `panel`
   node holding one `rtt`; read its rect back as
   `frame.rtt_rect("<id>_rtt")` and hand it to the offscreen pass.
5. **An empty pane is honest.** Leave the slot unfilled and the localized
   `$ui_pane_empty` placeholder shows — a well, not a void.

The retired shape, for recognition: a proto with fixed left/right slots around a
hardwired centre `rtt` (`tri_pane_rtt`), plus a scene-side pane enum, a
scene-chosen rim style and a scene-owned enter/exit mode. If you find that shape
anywhere, it is drift — the arrangement is `multi_view`, and focus is the
walker's.

Three stay **Rust builders** (their output is computed, not templatable — the
`frame` grid needs generated track strings, `option_grid` chunks rows):

| Builder | Slots | Key props |
|---|---|---|
| `frame` | `center`, `n`, `s`, `w`, `e`, `nw`, `ne`, `sw`, `se` | `style` (path prefix, default `settings`), `w`/`h` or `w_frac`/`h_frac`, `edge` (default **30** — the corner-rune clearance), `n_size`/`s_size`/`w_size`/`e_size`, `closable`, `close_action`/`_style`/`_label` |
| `card` | `content` | `title`, `subtitle`, `disabled`, `style` (default `menu.panel`), `pad`, `gap`, `header_gap`, `title_size`, `subtitle_size` |
| `option_grid` | `cards` | `cols` (default 4), `heading`/`_size`/`_color`, `subtitle*`, `hint*`, `gap`, `grid_gap`, `well_pad`, `well_style` |

`window` and `workbench` are themselves `{"template": "frame"}` nodes;
`tabbed_window` is a `window`, `workflow` is a `workbench` — protos compose
protos, up to `MAX_TEMPLATE_DEPTH = 8`. Only a template resolving *from within*
another template's output counts toward the bound; ordinary tree depth is free.

**`frame` is the docking container.** Nine named regions on a 3×3 border grid, with
per-edge track sizes — that is where a surface with a header, side panels, a
viewport and a footer comes from. `n`/`s` are the bands, `w`/`e` the side panels,
`center` the content, and the four corners the rune clearance. Reach for it before
composing a layout out of rows: a header is `n`, not the first row of `center`.
The Populous bench is the worked example, and it is three `frame`-family nodes
deep by design: `default_page`'s outer `frame` carries the page chrome (corner
runes on), the `paged_menu` docks into it, and a second `frame` inside that
(runes **off** — corners never stack) holds the page's own content, which is one
`multi_view`.

Note that `n` and `s` occupy the top-/bottom-**centre** cell only, flanked by the
corner cells. Widening `w`/`e` into real panels therefore widens the corners too,
and the bands read as sitting *between* the panels rather than spanning the frame.

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
| `orchestrate(&rules, &results)` | apply an [Orchestration](#orchestrations)'s `signal → op` table for this frame |
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
- **Declare only what you dispatch.** A declared name with no arm in the scene's
  result handling is dead hardware; a fired name that resolves to nothing is the
  failure the fail-loud rule exists for.
- **Some signals are never yours.** `Confirm`, `Cancel`, `Nav*`, `Panel*` and
  `ChordBegin` belong to the WALKER, on every screen. The walker consumes a
  declared intent and returns *before* the activation path, so declaring one of
  these does not add behaviour — it removes behaviour, statically. God Mode
  states the rule at the point of temptation:

  > **Declare only what you dispatch.** And note what is NOT here: `on_confirm`.
  > Confirm stays the walker's, because it is what ACTIVATES the focused control
  > — a bench full of buttons that stole Confirm for one of them would have a pad
  > that can move the focus ring and never press anything.

  Two gates hold the two channels this can travel:
  `no_proto_declares_a_walker_owned_signal` (the template file) and
  `no_scene_reads_a_device_or_names_a_pane_style` (every scene's own source).
  Both derive the vocabulary from `walker_owned(signal)` — the walker's own
  answer — so neither can drift from what the layer actually consumes.
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

## Orchestrations

**An Orchestration is a layout that changes based on signals.** That is the
whole construct, and the third leg of a stool you have already met: `Surfaces`
declares *what a screen can show*, intents and actions produce *what fired*, and
an Orchestration is the `fired name → surface op` table between them, as data.
`OnSignal(OpenInventory) → Show(inventory)` is a value, not a branch.

```rust
let rules = vec![
    OrchestrationRule::show("inventory_open",  "inventory"),
    OrchestrationRule::hide("inventory_close", "inventory"),
    OrchestrationRule::toggle("map_toggle",    "map"),
    OrchestrationRule::exclusive("pick_audio", "sec_audio"),
];

// per frame — after the scene folds its results, before publish:
self.surfaces.orchestrate(&rules, &results);
self.surfaces.publish(&mut model);
```

| Field | Is |
|---|---|
| `on` | any name truthy in this frame's results — a button `action`, a fired intent name, a `sig_*` mirror key |
| `op` | `SurfaceOp::Show` / `Hide` / `Toggle` / `Exclusive` (`Exclusive` = `set_exclusive`: show the target, hide its radio group) |
| `target` | a declared surface **id** — not its published Model key. An unknown id warns and no-ops |

Rules are applied in list order, so two rules driving one surface in the same
frame resolve last-one-wins. An unfired rule touches nothing; a fired rule that
lands on the state it already had costs nothing downstream — same Model value,
same fingerprint, same cached replay.

### The boundary (deliberate, banked)

An Orchestration owns **stateless** signal→surface reactions. A flow entangled
with scene state stays scene logic — settings' modal ladder resets scroll,
clears dirty flags and returns a `ModalFlow` value on the same flips, so
expressing it as rules would *add* code to preserve behaviour. The test is one
line: **if the reaction is "…and also do X to the scene", it is not a rule.**

Two things to know before you reach for it. Rules are Rust values — there is no
`ui_orchestrations.json`; the data-driven case of the family is the Workflow
below, whose definitions *are* content. And no shipped screen declares rules
yet: the Workflow runtime is the live proof, and the first greenfield consumer
is the adventurer inventory panel when it lands.

---

## Workflows

**A Workflow is an Orchestration with an ordinal.** The same signal→surface
machinery, plus a position in a line, a document contract, and two gates. It is
the spine of a bench: the asset pipeline runs on it, and a new bench gets its
step order, its rail, its Back guard and its forward gating from data.

Four pieces. The first is usually the only one you write.

| # | Piece | Lives in |
|---|---|---|
| 1 | the **definition** — the ordered steps | `Alpha/content/sensorium/resources/ui_workflows.json` |
| 2 | the **runtime** — ordinal, gates, publishing | `flicker-widgets` `workflow.rs` |
| 3 | the **document** — a `ValueMap` the scene rebuilds each frame | scene state |
| 4 | the **screen** — one subtree per step, gated by its surface key | your `hud_*.lua` |

### 1 — the definition

```json
"import_prop": {
  "title": "$ap_prop",
  "steps": [
    { "id": "task",    "title": "$wf_step_task",   "yields": ["source", "class"] },
    { "id": "conform", "title": "$wf_step_mount",  "needs": ["source"], "yields": ["fit"] },
    { "id": "review",  "title": "$wf_step_review", "needs": ["fit"],    "yields": ["committed"] }
  ]
}
```

| Field | Meaning |
|---|---|
| `title` (definition) | the dispatch-card / breadcrumb name, a `$token` |
| `id` | stable step identity — the stem of every key the runtime publishes for it |
| `title` (step) | the rail-chip label, a `$token`, published already resolved |
| `needs` | document keys that must be **present** to ENTER this step |
| `yields` | document keys this step produces — **enforced**, twice over (below) |
| `surface` | the `visible_bind` key gating the step's subtree (default: the NAMESPACED `wf_step_<id>`, *not* the bare id) |
| `context` | an `InputContext` name held while the step is shown, routed by `apply_contexts` |

**The surface key is namespaced.** A step's subtree gates on `wf_step_<id>` — not
on the bare `id`. Bare ids collided with sibling Model namespaces and with the
document keys they shadow (a step `attach` against a document key `attach`), so
the default is prefixed like every other workflow key. Write
`visible_bind = "wf_step_review"`, never `"review"`. The `surface` field still
overrides it verbatim, prefix and all — but reach for it only to point two steps
at one subtree, not to shorten the name.

**Branching is a second definition.** There is no conditional step, by ruling:
a required branch is another entry in this file, chosen up front at dispatch.
The asset pipeline's Task cards pick `import_character` or `import_prop`, and
"skip Attach for a non-character" is simply a definition without that step.

### 2 — the document is the pipe

Steps never talk to each other. Each reads and writes the bench's one document,
and `needs`/`yields` are its contract. The runtime probes **presence only** —
values are incidental, so a document is cheap to rebuild every frame:

```rust
fn wf_doc(&self) -> ValueMap {
    let mut d = ValueMap::new();
    let Some(src) = self.source.as_ref() else { return d };
    d.set("source", true);
    if src.rig.is_some()      { d.set("rig", true); }
    if !src.attach.is_empty() { d.set("attach", true); }
    d
}
```

`Next` into a step whose `needs` are unmet **warns with the missing keys and
refuses** — the author sees a log line naming the gap, never a blank page.

**Both halves of the contract are checked, and neither waits for a click.**

- **At construction**, `Workflow::new` warns for every `needs` key that no
  *earlier* step `yields`. That is not always a bug — a key the scene seeds
  before the workflow runs is legitimate — but it is always worth reading, because
  the other cause is a contract that can never be satisfied. You find out at
  startup, not at the hundredth click of a `Next` that refuses.
- **On leaving a step**, `handle` warns when the step you are walking off declared
  `yields` the document does not contain. The step that *lied* is named, at the
  moment it lies — not one step later as a mystery refusal against the step that
  merely believed it.

So a broken pipe surfaces at its source. Keep `yields` honest: it is load-bearing
now, and a stale entry is a warning on every run.

### 3 — driving it

```rust
// enter
let defs = workflows_from_json(UI_WORKFLOWS);        // include_str! the resource
let mut wf = Workflow::from_def(defs.get("import_character").expect("definition ships"));

// update, per frame
wf.handle(&results, &self.wf_doc());        // consume this frame's wf_* results
wf.publish(&self.wf_doc(), &mut model);     // surfaces + rail + footer keys
wf.apply_contexts(&mut self.route);         // step contexts (LIFO, exactly like Surfaces)
if authored_value_changed { wf.set_dirty(true); }   // arms the Back guard
```

The result vocabulary is fixed — these four names, no aliases:

| Result | Does |
|---|---|
| `wf_next` | advance through the needs gate. On the **last** step it is the bench's own finish action: the runtime warns and ignores it |
| `wf_back` | step back — **or**, when the step is dirty, raise the `wf_discard` dialog and hold the step |
| `wf_discard_yes` | close the dialog, clear dirty, step back |
| `wf_discard_no` | close the dialog, keep editing |

`set_dirty` is stage logic's to call, and so is undoing the document edits on
`wf_discard_yes` — the scene reacts to the same result. The runtime owns
progression and nothing else: **no IO**. File dialogs and content writes stay
in stage logic, which is what keeps the construct thin.

### 4 — what it publishes

| Key | Value |
|---|---|
| `wf_step_<id>` *(each step's surface key)* | one exclusive group (`wf_steps`); exactly the current step is `true` |
| `wf_discard` | the discard dialog's surface |
| `wf_step` | the current step id |
| `wf_step_i` / `wf_step_n` | 1-based position / total |
| `wf_can_next` | the next step's `needs` are all met — **`false` on the last step, by definition** |
| `wf_<id>_title` | that step's rail label, **already stringtable-resolved** — ride it with `text_bind` |
| `wf_<id>_style` | `workflow.chip.active` / `.visited` / `.todo` — ride it with `style_bind` |
| `wf_<id>_show` | rail **membership** — `true` for every step of the running definition; ride it with `visible_bind` |

`wf_<id>_*` is published only for the steps of the **running** definition, so
switching definitions stops publishing the dropped step's keys entirely. That is
what makes `wf_<id>_show` do real work: the character definition publishes
`wf_attach_show`, the prop definition never does, and the Attach chip leaves the
rail on its own. **The rail derives from `ui_workflows.json`** — there is no
hand-kept id list in any scene, and adding a step grows the rail without touching
Rust.

### 5 — the screen

Gate each step's subtree by its surface key, and give each one a chip:

```lua
-- one subtree per step, gated by the NAMESPACED surface key
Cell { visible_bind = "wf_step_review", children = { --[[ … ]] } }

-- the rail: one chip per step id, entirely on the runtime's published binds
local WF_STEPS = { "task", "conform", "attach", "review" }
for _, id in ipairs(WF_STEPS) do
  tabs[#tabs + 1] = Button {
    grow = 1,
    text_bind    = "wf_" .. id .. "_title",   -- pre-localized label
    style_bind   = "wf_" .. id .. "_style",   -- active / visited / todo
    visible_bind = "wf_" .. id .. "_show",    -- membership in the RUNNING definition
  }
end
```

All three binds come from the runtime; the scene publishes none of them. What the
scene still owns is the **chip roster** — `WF_STEPS` above. The tree is built
once, so there is no "repeat over the steps" channel: a rail is authored per step
id, over the union of ids across every definition that bench can dispatch. A
chip whose step is not in the running definition simply never gets its
`wf_<id>_show` key and stays hidden.

So adding a step to `ui_workflows.json` gates its subtree the moment you author
the matching `wf_step_<id>` subtree, and joins the rail as soon as its id is in
that union list. Chips carry no `action` by convention: the footer moves you.

**Wire the gate test.** `Workflow::ungated_steps(&tree)` returns every step whose
surface key no node in the tree gates on — a step that would flip its surface and
render nothing. It is the workflow half of the [drift gates](#the-drift-gates),
and a new bench wires it the same way:

```rust
let mut wf = AssetPipeline::new();
assert!(wf.wf.ungated_steps(&tree).is_empty(),
        "character workflow steps with no visible_bind: {:?}", wf.wf.ungated_steps(&tree));
wf.dispatch_workflow(Some(AssetClass::Prop));      // and again per dispatched definition
assert!(wf.wf.ungated_steps(&tree).is_empty(),
        "prop workflow steps with no visible_bind: {:?}", wf.wf.ungated_steps(&tree));
```

Assert it once **per definition the bench can dispatch** — the steps differ, so
one pass proves only one branch. That turns a definition/tree mismatch into a
build failure instead of a blank page. `flicker-assetpipeline`'s screen test is
the precedent to copy.

### The `workflow` proto

`{ template = "workflow" }` builds the standard shape — a `workbench` plus a
Back/Next footer wired to the runtime vocabulary, plus a `choice_dialog` already
gated on `wf_discard`:

| Slot | Lands in |
|---|---|
| `header` | the workbench header bar |
| `rail` | the workbench **tab strip** — your step chips |
| `steps` | the workbench **viewport** — your per-step subtrees |
| `inspector` | the workbench **rail** column |
| `footer_extra` | between the spacer and Next |

> Read that table twice: the proto's `rail` slot is the workbench's `tabs`, and
> the proto's `inspector` slot is the workbench's `rail`. The word means the
> chip strip one level up and the side column one level down.

The footer pair ships as `id`/`action` `wf_back` and `wf_next`, both in
`tab_group = "wf_footer"` for the controller floor, with Next on
`enabled_bind = "wf_can_next"`.

Three limits of the proto as it stands, because they decide whether you can use
it. Back carries no `enabled_bind`, so on step 1 — where `wf_back` is inert — the
button still looks live. Next's enable is the literal `wf_can_next`, not a
prop, and that key is `false` on the last step, so the proto's Next is disabled
exactly where a bench's finish action sits. And Next's caption is the
substitution prop `@next_label` (a static string). A bench that needs a live
finish button — the asset pipeline does, its last-step Next reads RESTART —
passes `next_label_bind` instead, and the proto's Next button reads that Model
key through `text_bind`.

---

## Where data lives

Three destinations, and the choice is not stylistic — it is the data-placement
law:

| Kind of data | Goes | Example |
|---|---|---|
| **Durable content** — assets, semi-permanent configuration, definitions | `Alpha/content/sensorium/resources/` | `ui_elements.json`, `ui_templates.json`, `ui_workflows.json` |
| **Ephemeral / transient / mutable** | `Alpha/content/data/`, shaped as **database-candidate records** (assume a DB backend is coming: row-shaped, keyed, no free-form blobs) | a bench's saved state, per-user picks |
| **Every display string** | `Alpha/content/data/stringtable.json` as a `$token` | `"wf_step_task": { "en-us": "Workflow" }` |

A workflow definition is durable content: it describes the shape of a flow, not
a run of it. The *document* a run produces is scene state and never lands in
`resources/`. And a `title` in a definition is a `$token`, never English — the
runtime resolves it at publish, so the rail is localized without the screen
knowing.

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
    let script = ScriptHost::from_file(HUD_SCRIPT).expect("hud_hello.lua loads");
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

**Two more run once for the whole workspace**, so a new bench inherits them
without wiring anything:

- `no_proto_declares_a_walker_owned_signal` — no proto anywhere offers a
  walker-owned `on_*` as a param.
- `no_scene_reads_a_device_or_names_a_pane_style` — no scene reads a gamepad
  directly (only the controller tester, whose subject IS the device), names the
  retired `tri_pane.*` pane palette, grows its own shell builder or orbit camera,
  or declares a walker-owned signal. The last clause carries a **closed list** of
  the benches not yet migrated: it fails when a new name appears *and* when a
  listed one migrates without shrinking the list.

**A workflow bench wires a third:** `wf.ungated_steps(&tree)`, once per definition
it can dispatch — every step whose surface key nothing in the tree gates on. It
is a method rather than a free function because it needs the running definition's
steps, not just the tree. See [Workflows §5](#5--the-screen).

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
| **`text` and styled-container backgrounds are walker primitives, not Components** | `text` is the most numerous kind in a real tree and container backdrops are the shared `draw_panel_bg`, so the walker draws both itself rather than routing them through a `draw_<kind>` arm. They are the one place a "control" has no component arm to find. |
| **`flicker-world`'s `world_ui.lua` + `widgets.lua`** | The last immediate-mode control surface: per-epoch structural slider rows plus a duration-weighted timeline scrubber with multi-key-bound geometry — no walker channel expresses it, so converting is a redesign. It is the ONE remaining consumer of the trimmed legacy `Widgets` global (slider/stepper/dropdown/button); `widgets.lua` dies with that conversion. |
| **`logo.lua` stays immediate** | Its timeline needs Model-driven texture switching plus per-frame fit-scale geometry. It returns no `M.tree()`. |
| **loomforge (and the floating chat panel) rebuild the tree every frame** | Load-bearing: the bench mutates its document mid-frame and its node ids encode *filtered* list positions read against post-mutation state, so a retained tree would need revision plumbing across every mutation funnel. This costs nothing: cache identity is **structural** (an id, else the parent key folded with kind + sibling index), not the address of a retained node, so a rebuilt-but-identical tree replays at `redraw_nodes == 0`. Its `UiIntents` are still collected **once** — root props are static even when the tree is not. |
| **Data-coloured scene panels** | God Mode's bulk-seed swatches and pocepochs' legend + element panel draw in the scene, because their per-row colours are DATA (`element_rgb`, which hashes a fallback for elements nobody has picked a colour for). The walker's colour channel is dotted style paths **by design** — one palette, one place. A per-datum colour has no path. **The bar is per-DATUM, not merely per-row:** God Mode's event log used to sit here and no longer does — its colours were per-KIND (born / died / merged / split / gate open / gate shut), a finite set, so they became ordinary style paths and the panel became the ordinary `godmode_events` proto. An exception that has stopped being true is drift with a comment on it. |

---

## Sharp edges & guardrails

### Silent failures — the ones to grep for first

The system fails loudly for a mistyped **component kind** (the `unknown_kinds`
gate), a mistyped **`on_<signal>`** (warn + skip), a component that answers
**neither** `rust_hit_shape` nor `rust_owns_hit` (the roster gate — a test
failure), and a missing **string token** (renders raw + warns). Everything
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
- **A rail chip the scene never authored never appears.** The runtime publishes
  `wf_<id>_show` for every step of the running definition, so membership follows
  the data — but nothing cross-checks a tree's chip roster against the
  definitions' step ids. Add a step to `ui_workflows.json` and its chip is still
  missing until you add the id to the rail's list. (Its *subtree* is covered:
  that mismatch is a build failure via `ungated_steps` — wire that gate.)
- **A workflow name that no definition matches keeps the running workflow**
  (warn), and a `ui_workflows.json` that fails to parse yields an **empty map**
  (warn) — every lookup misses at once rather than one loudly.

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
- **A flow is data before it is code.** Ordered stages ⇒ a definition in
  `ui_workflows.json`, not a `Step` enum and a `match`. A branch ⇒ a second
  definition, not a conditional. A signal that shows a panel ⇒ an
  `OrchestrationRule`, not an `if` in `update()`.
- **Enhance in place.** One walker (`flicker-widgets`), one parser
  (`flicker-script`), one style source (`ui_elements.json`), one template file
  (`ui_templates.json`), one workflow-definition file (`ui_workflows.json`), one
  stringtable. Extend them; never fork a parallel path.

---

*Walker: `Alpha/crates/frontend/flicker-widgets/src/component.rs` · templates:
`…/template.rs` + `Alpha/content/sensorium/resources/ui_templates.json` ·
surfaces + orchestration: `…/surfaces.rs` · workflows: `…/workflow.rs` +
`Alpha/content/sensorium/resources/ui_workflows.json` · intents: `…/intents.rs` ·
strings: `…/strings.rs` + `Alpha/content/data/stringtable.json` · node schema +
the Lua seam: `Alpha/crates/scripting/flicker-script/src/lib.rs` · components:
`…/component.rs` (`draw_<kind>` / `rust_hit_shape` / `rust_owns_hit`) ·
styles + palette:
`Alpha/content/sensorium/resources/ui_elements.json` · surface protos:
`Alpha/content/sensorium/resources/ui_templates.json` · a live workflow bench:
`Alpha/crates/scenes/flicker-assetpipeline/src/lib.rs` (`build_tree`) + its
`clayworks_bench` proto.*
