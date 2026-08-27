# Authoring a Prism scene

**Five things, five homes. Everything you author lives in exactly one of them:**

| You are writing… | It goes in… |
|---|---|
| The scene tree — which components, where they sit (anchors, sizes) | `scenes/SceneName.scene.json` |
| The scene's logic — component behaviour, runtime variables, derived values | `scripts/SceneName.lua` |
| Colors | `resources/ui_theme.json` (`theme.tokens` — the ONE palette) |
| Global default weights & effects | `resources/ui_style.json` |
| A component itself — its drawing, layout, hit-testing, features | The Rust engine (`flicker-widgets`) — not authorable here |

A scene is a **pair**: `SceneName.scene.json` + `SceneName.lua`, matched by
name, one each per scene. Both are human content — plain files, no recompile,
no abstraction between them. If two scenes want similar logic, each carries its
own copy; a scene's files describe *that scene*, nothing else.

Two folders sit beside those five: `scenes/shared/` + `scripts/shared/` (the
modal furniture every scene can raise — see [Shared modals](#shared-modals)),
and `resources/ui_stages.json` (the shared **stage** library — see
[The theme, the style file, the stages](#the-theme-the-style-file-the-stages)).

> **Scope.** This is the usage guide — how to author against the system. The
> design of record (why it is shaped this way, decisions, history, migration
> state) lives in the project's MCP memory bank, never here.

---

## Contents

1. [The 60-second model](#the-60-second-model)
2. [The scene file](#the-scene-file) — tree, anchors, exits, styles
3. [Binds — the one general rule](#binds--the-one-general-rule)
4. [The pair script](#the-pair-script) — `derive`, `arrange`, `react`
5. [The theme, the style file, the stages](#the-theme-the-style-file-the-stages)
6. [Components](#components) — the Rust vocabulary and the catalog
7. [Input is signals](#input-is-signals)
8. [Shared modals](#shared-modals)
9. [Strings](#strings)
10. [Adding a scene, end to end](#adding-a-scene-end-to-end)
11. [The gates your scene must pass](#the-gates-your-scene-must-pass)
12. [Sharp edges](#sharp-edges)

---

## The 60-second model

The engine draws **components** — fully-featured Rust controls (button, slider,
panel, popup_panel, paged_menu, …). You arrange them in a JSON **tree**, wire
their values to named **Model** keys (the per-frame key→value table the engine
hands to Lua), and write a small Lua file that owns the scene's **logic**: it
receives the raw runtime variables the engine publishes and returns the derived
values the components display.

Colors never appear in your files as numbers: every color is a `$token`
reference into the one palette (`ui_theme.json` → `theme.tokens`). The engine
resolves tokens at load, merges your scene's style blocks over the shared
defaults, and walks your cached tree every frame.

```
scenes/clicktrainer.scene.json   ← the tree: a stats panel, six text rows, RESET
scripts/clicktrainer.lua         ← the logic: raw hits/misses → "83%", "210 ms"
```

---

## The scene file

`scenes/SceneName.scene.json` — the file IS the scene; its NAME is the scene's
id (never write an `id` key; the parser refuses one and says so). Top-level
keys, and these eight only — anything else is a load error naming the offender:

```jsonc
{
  "_comment": "free-form note; `_`-prefixed keys are comments everywhere",
  "behaviour": "clicktrainer",   // the Rust behaviour that plays this scene
  "boot": false,                  // exactly ONE scene in the folder says true
  "params": {},                   // that behaviour's own configuration
  "tree": { ... },                // the component tree (below)
  "exits": {                      // fired result name → where the kernel goes
    "next": { "to": "Main", "mode": "replace" }
  },
  "styles": { ... },              // THIS scene's layout blocks (below)
  "stages": { ... }               // THIS scene's stage sources (below)
}
```

A **result** is a name your scene fires (a button's `action`, a declared
intent, a value your pair script returns). `exits` is the routing table for the
ones that leave the scene; `mode` is optional and is one of:

| `mode` | The kernel… |
|---|---|
| `replace` (default) | swaps this scene for the target |
| `replace_root` | swaps it and clears the stack beneath |
| `push` | keeps this scene underneath and stacks the target on top |

`params` is opaque to the loader — only the behaviour knows its keys, so only
the behaviour can report a missing one.

### The tree

A node names a `component` kind and nests `children`. Location is authored
here — anchors, sizes, pads, gaps:

```jsonc
{
  "component": "surface",
  "on_menu": "pause_open",              // input intent, declared as data
  "children": [
    {
      "component": "cell",
      "anchor": "top_left", "width": 300, "pad": 16, "gap": 10,
      "style": "clicktrainer.panel",         // a dotted path into styles
      "children": [
        { "component": "text", "text": "$ct_hits", "text_size": 13 },
        { "component": "text", "text_bind": "hits", "text_size": 13 },
        { "component": "button", "id": "reset", "action": "reset",
          "label": "$ct_reset", "style": "modal.buttons.variants.secondary" }
      ]
    }
  ]
}
```

- `text` shows fixed copy via a `$token`; `text_bind` shows a Model value.
- `bind` on a control (slider, checkbox…) two-way binds its value to a Model key.
- `action` fires a named result on activation — a Confirm at the pointer and a
  Confirm on the focused node are the same thing.
- `visible_bind` gates a subtree on a Model key. Declare everything, default
  hidden, reveal from Lua.
- `anchor` pins a node (`center`, `top_left`, …); nodes without one flow in
  their parent (`row` / `cell` / `stack` / `grid` / `list`).
- `runes: true` wears the corner-rune decoration; `runes_style` names an
  override block. It is a FLAG on any node, not a component kind.

### `styles` — this scene's layout blocks

Whatever style paths your tree names beyond the global defaults live **in your
scene file**, under `styles`. Colors are `$token` refs — the palette stays in
the theme:

```jsonc
"styles": {
  "clicktrainer": {
    "panel": { "fill_top": "$stone3", "fill_bot": "$stone1",
               "border": "$edge2", "radius": 4, "border_w": 1 }
  },
  "modal": { ... }   // a scene may carry a shared-looking block it uses; the
                     // block still belongs to THIS scene, not to a shared file
}
```

A `theme` key inside `styles` is refused at parse — the palette cannot fork. So
is a `stages` key inside `styles`: stages ride the scene file's own top-level
`stages` section, and the loader folds them together for you.

---

## Binds — the one general rule

**Any prop takes a `_bind` twin.** `"<prop>_bind": "model_key"` sets `<prop>`
from the Model each frame, for *every* prop a component reads — not just the
named ten. `fade_in_bind`, `size_bind`, `inner_bind` all work; the mechanism is
generic, so a new component prop is bindable the day it exists.

Ten stems are the ones the walker gives special meaning, and they are the ones
worth memorising:

| Bind | Sets |
|---|---|
| `text_bind` / `label_bind` | the node's displayed copy (bound copy is exempt from the raw-literal gate) |
| `visible_bind` | whether the subtree is walked at all |
| `enabled_bind` | whether the control accepts input |
| `style_bind` | the dotted style path (the Model holds the path string) |
| `color_bind` | a dotted colour path (the Model holds the path string) |
| `live_bind` | a `surface` node's liveness (see [stages](#the-theme-the-style-file-the-stages)) |
| `rune_bind` / `name_bind` / `meta_bind` | a tooltip's three text fields |

Eleven stems are the walker's own and refuse a bind: `hot`, `pressed`,
`enabled`, `focused`, `open`, `captured`, `wheel`, `label`, `style`, `layer`,
`content_h`. Authoring e.g. `hot_bind` logs a warning and is skipped rather
than silently overwritten.

`bind` (no suffix) is the different one: a control's **two-way** value channel.
A slider or checkbox reads its value from that Model key and writes the
committed value back to it.

---

## The pair script

`scripts/SceneName.lua` is the scene's control surface. **Every pair script is
a module** — a `local M = {}` table returning up to three hooks, all optional
(at least one must be present):

### `derive()` — logic → display

The engine publishes the scene's **raw runtime variables** into the global
`Model` table, then calls `derive()`. You return the **derived** values the
components bind — display strings, per-component style paths, visibility gates.
This is where the scene's component logic lives.

```lua
-- clicktrainer.lua (shipped)
local M = {}

local function stat_text(seconds)
  if seconds == nil or seconds < 0 then return "—" end
  return string.format("%.0f ms", seconds * 1000.0)
end

function M.derive()
  return {
    hits = string.format("%d", (Model and Model.hits) or 0),
    accuracy = string.format("%.0f%%", (Model and Model.accuracy_pct) or 0),
    stat_last = stat_text(Model and Model.stat_last_s),
  }
end

return M
```

`componentcatalog.lua` goes further: it seeds every demo control's initial
value (yielding once a committed value echoes back), lights the active nav
bookmark by returning a style path per bookmark, and gates the Paged Menu
card's tab rails off the bound page/tab — all scene logic, all in the Lua.

### `arrange()` — which components are on, and how they're configured

Each key is a node id from the scene's tree; each entry turns that component
on/off and may **override its props** — any key beyond the structural five
(`on`, `anchor`, `offset`, `resizable`, `movable`) lands on the node exactly
as the same key would in the scene JSON. Called on change, not per frame.

Two shapes cover almost everything. Gating slices on a selection (pages, tabs)
— `populous.lua` is the reference:

```lua
function M.arrange()
  local page, tab = (Model and Model.page) or 0, (Model and Model.tab) or 0
  return {
    ["shown_p0_t0"] = { on = (page == 0 and tab == 0) },
    ["shown_p0_t1"] = { on = (page == 0 and tab == 1) },
  }
end
```

…and configuring a component's features — `TegLogo.lua`, whose node id
`splash` is a `sprite` in presenting mode:

```lua
function M.arrange()
  return {
    splash = { on = true, fade_in = 0.6, hold = 1.2, fade_out = 0.6,
               image = "package/sensorium/assets/elideus_productions_yellow.png" },
  }
end
```

### `react(sig)` — signals → intents

Given this frame's fired signal names, update your own remembered state and
return outbound intents (navigation the kernel routes, a game action). Called
only when something fired. The splashes route with it: the engine reports
`done` (timeline complete), `confirm`, or `cancel`, and the returned intent —
`next` or `exit` — is fired as the scene's result and routed by the scene
FILE's `exits` map:

```lua
-- TegLogo.lua (shipped): done/confirm advance, cancel backs out.
function M.react(sig)
  if sig.cancel then return { exit = true } end
  if sig.done or sig.confirm then return { next = true } end
  return {}
end
```

**What Lua never does:** draw, hit-test, lay out, or touch per-frame streams.
A slider's value flows engine↔component directly; your script sees selections
and state, on change. Copy you compose in Lua is content — yours to word.

Non-pair scripts — anything not named for a scene — live in `scripts/shared/`:
the pair scripts of the shared modals (`pause.lua`, `confirm.lua`,
`settings.lua`) plus `hud_paperdoll.lua`.

---

## The theme, the style file, the stages

**`ui_theme.json` is the theme node and nothing else.** `theme.tokens` maps
token names to rgba — the single source of every color in the app. Add or
adjust a token here; reference it as `"$name"` anywhere else. A build gate
fails if any other key appears in this file.

**`ui_style.json` holds truly-global default weights and effects** — and only
those. It is empty today: component defaults are compiled into the drawing
code, and scene-specific values ride scene files. Never put a per-component or
per-scene block here; a gate enforces that too.

### Stages

A **stage source** is a named offscreen sub-scene — lighting, a clear colour, a
camera, content layers, and the recipe of engine passes around them — that a
tree's nested `surface` node composites into its rect. **The node says WHERE;
the stage says WHAT.**

**A stage one scene uses lives in that scene's file**, under the top-level
`stages` section. **`ui_stages.json` is the shared stage LIBRARY** — what more
than one scene draws from, which today is the `lighting` presets every stage
names: `studio`, `night`, `deep_space`, `hearth`. A scene NAMES a preset; it
never authors one, and it may not reuse a library source name. Everything merges into
one root at load — theme, satellites, then the scene's own `styles` over them
and its `stages` into the shared block — then tokens resolve, so a dotted style
path or a stage name behaves as if everything were one file.

One compiler reads every stage (`flicker::ui::stage_def`). Every authoring
problem is reported by name — at build time through the gates below, at runtime
as a warning with the same words. A bad value still degrades to its default: a
malformed stage costs the authored look, never the picture.

**The `surface` node** (where a stage lands):

| Prop | Meaning |
|---|---|
| `source` | the `stages.<name>` to render here. Optional — a surface the behaviour fills itself authors none. A name with no stage warns loudly and the slot is still reserved. |
| `layout` | `single` (default) / `pair` / `quad` — how many camera panes the behaviour tiles. An unknown name warns and the slot is skipped. |
| `inset` | pixels inset from the node's rect (may also sit in the shared panel style). |
| `rate` | `live` / `poster` / `dirty` / `{"hz": N}` — read by the same parser a stage's `rate` uses, so node and stage spell it identically. |
| `live` / `live_bind` | boolean sugar for Live/Poster when you don't author `rate`. |
| `tint` | a dotted colour path. |

**The stage source** itself:

```jsonc
"stages": {
  "solarbirth_sky": {
    "lighting": "deep_space",              // a preset NAME from the library
    "clear": [0.0, 0.0, 0.0, 1.0],         // rgba or a "$token"; absent = default
    "camera": { "kind": "orbit", "yaw": …, "pitch": …, "dist": …, "target_y": … },
    "layers":  [ { "draw": "shells" }, … ],// content layers (below)
    "attachments": { "color": { "format": "surface", "scale": 1.0 },
                     "depth": { "format": "depth32" } },
    "passes": [ { "pass": "sky" },
                { "pass": "scene" },
                { "pass": "volumetric_disk", "reads": ["depth"], "writes": ["color"],
                  "inner": 3.0, "outer": 21.7, "formation_bind": "dust_formation" } ],
    "rate": "live"
  }
}
```

`attachments`, `passes` and `rate` are all optional; a stage without them is
one `scene` pass into colour+depth, live. The vocabularies:

| Key | Legal values |
|---|---|
| `layers[].draw` | `skinned` · `ring` · `grid` · `shells` · `shell` · `graticule` · `material` |
| `attachments.<n>.format` | `surface` (the swapchain's) · `depth32` · `rgba16f` (linear HDR) |
| `passes[].pass` | `scene` · `sky` · `volumetric_disk` · `ground_fog` · `tonemap_grade` · `water_surface` · `bloom` · `composite`\* · `shadow_map`\* |
| `rate` | `live` · `poster` · `dirty` · `{"hz": N}` |

\* `composite` and `shadow_map` are ordering **markers** — they render nothing on
their own; the scene's Rust behaviour wires their runtime (see
[`STAGES.md`](STAGES.md#passes--the-roster)). The other seven render from data alone.

Not every filler draws every layer kind — a behaviour warns at load naming the
kinds it was handed and cannot draw.

**Three rules worth internalising about a recipe:**

1. **Never author draw order.** Order is derived from what each pass `reads`
   and `writes`. A pass that reads `depth` runs after the pass that wrote it.
   Declaration order is only the tie-break between independent passes. Names in
   `reads`/`writes` must be keys of this stage's own `attachments`.
2. **A `*_bind` REPLACES the field it names** — no multiply, no offset. The key
   is a plain Model-style string the scene publishes each frame. Authoring a
   number *and* binding the same slot is a compile problem, because the number
   would be dead data.
3. **A `passes` list with no `scene` pass is a problem** — the content would be
   silently dropped, so the compiler refuses instead.

> **The full stage reference is the companion guide:**
> [`STAGES.md`](STAGES.md). Read it for the whole pass roster with every pass's
> keys and defaults, the two lighting-rig forms (the legacy `sun/moon/point`
> trio and the general `lights[]` array with falloff and drivers), HDR and the
> tonemap, the sun-shadow producer/consumer pattern, water, and worked examples
> (Solar Birth, the Prism Test Room). This section is the orientation; that file
> is the catalog.

---

## Components

Components are Rust — drawn, laid out, and hit-tested by the engine, fully
featured. You configure features from data (tree props) and logic (your Lua);
you never define a component in JSON or Lua.

**The live reference is the Component Catalog scene** (Developer realm): one
card per kind with every feature on. When you wonder what a `paged_menu` or a
`gauge` can do, launch it.

The **interactive** vocabulary (authoritative list: `RUST_COMPONENT_KINDS` in
`flicker-widgets/src/lib.rs`) — button, panel, sprite, tooltip, checkbox,
toggle, radio, tile, pill_toggle, tabs, select, slider, stepper, text_field,
list, context_menu, gauge, resource_gauge, stat_dot, action_slot, medallion,
badge, popup_panel, paged_menu, nav_footer.

The **structural** kinds (`STRUCTURAL_KINDS`, same file) — surface, cell, row,
stack, grid, text, option. `option` is pure data a strip/menu/legend reads out
of its own children.

Two names a stale example may still use are not kinds: a splash is a `sprite`
in its presenting mode (backdrop + contain-fit + fade timeline), and corner
runes are the `runes: true` flag. A kind outside the two lists draws NOTHING —
the walker anchor-overlays its children and the draw arm falls through — which
is why every scene carries a vocabulary gate.

`nav_footer` is the bench-standard footer band: a left LEGEND of `option`
children (glyph/keycap + help label) and the scene's authored button cluster
right-aligned. Stateless — its buttons fire the same result names the screen's
declared intents fire, so there is one activation channel, not two.

A new component = a new arm in `component.rs`, an entry in
`RUST_COMPONENT_KINDS`, and a catalog card. There is no other tier.

> **Sprites, colour, and the raster engine beneath the components** have their
> own companion guide: [`RASTER_AND_SPRITES.md`](RASTER_AND_SPRITES.md). Read it
> for how `draw_sprite`/`draw_sprite_uv` and the `sprite` widget work, how the
> one palette resolves, atlas UV, and the ClayEngine `Sprite`/`SpriteStrip` port
> for 2D games. (The `sprite` component here is presentation-only — no atlas, no
> animation; those live in the `flicker-2d` layer.)

---

## Input is signals

Nothing binds to a key. Devices resolve to **signals** — named intents like
`Confirm`, `Cancel`, `Menu`, `Interact`, `NavUp` — in the central pump, and a
pointer press at a component's target IS a `Confirm` there. Which device
produces which signal is the player's binding profile, never your business.
Your scene participates three ways:

### 1. Declare an intent as data

On the tree's **ROOT node only**: `"on_menu": "pause_open"`. The prop is
`on_` + the signal's name folded to snake_case (`on_attack_light` names
`AttackLight`); the walker consumes that signal and fires your named result.
An unknown suffix or a non-string value is warned and skipped.

**Nine signals are walker-owned and mean one thing on every screen** — do not
declare them: `NavUp`, `NavDown`, `NavLeft`, `NavRight`, `PanelNext`,
`PanelPrev`, `Confirm`, `ChordBegin`, and `Cancel`. A declared intent *beats*
the walker's default, so declaring one of these takes focus movement or
activation away from the whole screen. (`Cancel` is the one modals override
deliberately — see [Shared modals](#shared-modals) — and nothing else should.)

### 2. Consume results, not devices

`action` names arrive in the frame's results; scene-level results route through
your file's `exits`.

### 3. Show the player which control does it

Any node may carry `"signal": "<SignalName>"` (PascalCase — the spelling `on_`
folds onto) and the engine draws a **device-adaptive control face**: the pad
glyph on a controller, the keycap of the bound key on keyboard/mouse. `tooltip`
uses it for its leading slot; `option` children of a `nav_footer` use it for the
legend. `solarbirth.scene.json` is the worked example.

The face has a **partner requirement**: the scene's behaviour must publish
`bind_<SignalName>`, `glyph_<SignalName>` and `input_device` into the Model —
one Rust call does all three (`publish_signal_bindings`). You cannot turn the
face on from data alone, and nothing tells you so: without the publish the
layout still reserves the affordance slot and draws nothing into it.

If you find yourself wanting a keycode, you are on the wrong layer.

---

## Shared modals

`scenes/shared/` holds the overlays every scene can raise — `pause`,
`confirm`, `settings` — each a normal pair (`shared/<name>.scene.json` +
`scripts/shared/<name>.lua`). They are authored ONCE and referenced, never
copied into a scene.

They differ from a roster scene in three ways — the whole reason they get their
own folder:

- **No `behaviour` key, and no roster entry.** The manifest indexes top-level
  scene files only and skips directories, so these are compiled into the shell
  and played by a Rust shell scene (`PauseScene`, `ConfirmDisplayScene`,
  `UnifiedSettingsScene`), not by a registered behaviour.
- **They declare `on_cancel`** — the one deliberate override of a walker-owned
  signal, because a modal's Cancel must mean *close this modal*. Pause folds it
  to `resume`; confirm declares none at all (Keep/Revert/timeout only).
- **Their chrome styles ride `Main.scene.json`** (`modal`, `screens.pause`,
  `screens.confirm`), resolved at runtime — a shared tree references the
  carrier's blocks rather than carrying its own.

`shared/pause.lua` is the intended authoring example: its `arrange()` lights
the `show_settings` / `show_main_menu` / `show_quit` gates the tree's optional
buttons hang on, so varying the menu is flipping one boolean. Resume stays
ungated — the always-present way out.

`shared/settings.scene.json` is the exception to "the tree is what you see":
its per-section containers are authored EMPTY and filled by hardened Rust from
a row schema. The untrusted Lua composes no structure there.

---

## Strings

Every display string is a `$token` into the stringtable
(`content/data/stringtable.json`), resolved at draw. Raw display literals in a
tree or published from Rust fail the gates. Copy composed in your pair script
(a phase line, a countdown) is content — compose it from resolved pieces the
engine hands you or tokens you name. Copy arriving through `text_bind` /
`label_bind` is exempt: that is the channel a runtime name travels.

---

## Adding a scene, end to end

1. **Write the pair**: `scenes/myscene.scene.json` (behaviour, tree, exits,
   styles) + `scripts/myscene.lua` (derive/arrange/react as needed).
2. **Give it a behaviour**: either a shell builtin plays it, or the client
   registers a roster entry whose id matches the file's `behaviour` — the
   factory receives your parsed def (`def.tree`, `def.styles`) and the launcher
   gets its realm + panel row from the same entry.
3. **Load through the def** — never a private copy of the tree or styles:
   `load_styles_for(theme_path, def.styles.as_ref())`, tree from `def.tree`.
4. **Wire the pair script** exactly like Click Trainer: publish raw variables
   with `set_model`, fold `derive()`'s output into the frame Model.
5. **Write the scene's own gates** — they are per-scene-crate, not inherited.
   Copy Click Trainer's: `unknown_kinds(&tree)` empty,
   `raw_display_literals(&tree)` empty, and
   `the_pair_script_derives_the_display_strings`. Nothing else checks a new
   scene's vocabulary for you.

---

## The gates your scene must pass

Shared gates, in `flicker-widgets`:

| Gate | What it holds |
|---|---|
| `ui_theme_json_is_the_theme_and_nothing_else` | you didn't touch the theme file except tokens |
| `no_component_block_lives_in_a_shared_file` | your blocks are in YOUR file |
| `load_styles_merges_satellites_and_refuses_a_palette_fork` | the satellites merge; no second palette |
| `no_shipped_scene_names_the_template_tier` | no `template` / `slots` key at any depth |
| `no_shipped_scene_authors_a_retired_surface_kind` | `screen` / `rtt` / `viewport` are all `surface` now |
| `no_scene_reads_a_device_or_names_a_pane_style` | no scene reaches past the input map for a device |
| `every_shipped_stage_compiles_clean` | every library preset + every scene's stages, zero problems |
| `every_surface_source_in_a_shipped_scene_resolves` | every `surface` node's `source` names a real stage |
| `a_stage_in_the_shared_library_is_shared` | a library source is named by ≥2 scenes |
| `a_scenes_stages_merge_into_the_shared_block_and_never_shadow_the_library` | your stage never shadows a library name |
| `every_lit3d_shipped_stage_resolves_through_exactly_one_tonemap` | an HDR surface resolves once |

Plus, in your own scene crate: the vocabulary gate (`unknown_kinds` empty), the
strings gate (`raw_display_literals` empty), the pair-script gate (see
`the_pair_script_derives_the_display_strings` in Click Trainer), and the
manifest gate — your behaviour resolves and your exits point at real scenes.
The shell's `the_shipped_screens_name_only_kinds_the_engine_knows` covers the
shared modals.

---

## Sharp edges

- **The pair is by name.** A scene id is the scene file's name. Case matters.
- **A script must return a module exposing a hook** (`derive`, `arrange`,
  `react`, `tree`, `update`+`draw`) — or the host refuses it at load
  (fail-fast, not mid-frame).
- **`Model` may be nil-sparse** in Lua — read defensively:
  `(Model and Model.key) or default`.
- **Derive returns scalars** (bool / number / string). Tables other than the
  return itself are not marshaled.
- **On change, not per frame.** `arrange()`/`react()` run on change; keep
  `derive()` cheap — it runs with the frame.
- **Declare all, default hidden.** Gated panels exist in the tree from the
  start; Lua reveals them. A component missing from the tree cannot be shown.
- **One palette.** A `theme` key anywhere outside `ui_theme.json` is refused,
  loudly. Add tokens instead.
- **Intents live on the ROOT node only.** An `on_<signal>` on a nested node is
  never collected and never warned about — it simply does nothing.
- **A `signal:` name is never checked.** Misspell it, or author it in a scene
  whose behaviour never publishes the binding, and the affordance slot is still
  reserved and drawn empty — a permanent gap in the legend, with no warning.
  Copy the spelling from a working scene rather than typing it.
- **`sig_<name>` is transient.** A fired intent name is mirrored into the Model
  for exactly one publish and then dropped. Latch it in Lua if you need it
  longer.
- **Some node ids are load-bearing.** A `splash` behaviour finds its image node
  by `id == "splash"`; the settings filler finds its row containers by
  `video_rows` / `audio_rows` / `kb_rows` / `mouse_rows` / `controller_rows`.
  Rename one and the scene loads and shows nothing there.
