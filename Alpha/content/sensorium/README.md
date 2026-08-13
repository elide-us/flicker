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

> **Scope.** This is the usage guide. The design of record — the ratified
> five-line architecture, its history, the migration state — lives in the MCP
> memory bank, never here.

---

## Contents

1. [The 60-second model](#the-60-second-model)
2. [The scene file](#the-scene-file) — tree, anchors, exits, styles
3. [The pair script](#the-pair-script) — `derive`, `arrange`, `react`
4. [The theme, the style file, the stages](#the-theme-the-style-file-the-stages)
5. [Components](#components) — the Rust vocabulary and the catalog
6. [Input is signals](#input-is-signals)
7. [Strings](#strings)
8. [Adding a scene, end to end](#adding-a-scene-end-to-end)
9. [The gates your scene must pass](#the-gates-your-scene-must-pass)
10. [Sharp edges](#sharp-edges)

---

## The 60-second model

The engine draws **components** — fully-featured Rust controls (button, slider,
panel, popup_panel, paged_menu, …). You arrange them in a JSON **tree**, wire
their values to named **Model** keys, and write a small Lua file that owns the
scene's **logic**: it receives the raw runtime variables the engine publishes
and returns the derived values the components display.

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
id (never write an `id` key). Top-level keys:

```jsonc
{
  "behaviour": "clicktrainer",   // the Rust behaviour that plays this scene
  "boot": false,                  // exactly ONE scene in the folder says true
  "params": {},                   // that behaviour's own configuration
  "tree": { ... },                // the component tree (below)
  "exits": {                      // fired result name → where the kernel goes
    "next": { "to": "Main", "mode": "replace" }
  },
  "styles": { ... }               // THIS scene's layout blocks (below)
}
```

### The tree

A node names a `component` kind and nests `children`. Location is authored
here — anchors, sizes, pads, gaps:

```jsonc
{
  "component": "screen",
  "on_menu": "pause_open",              // input intent, declared as data
  "children": [
    {
      "component": "cell",
      "anchor": "top_left", "width": 300, "pad": 16, "gap": 10,
      "style": "clicktrainer.hud.panel",     // a dotted path into styles
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
- `action` fires a named result on activation — pad Confirm and mouse click are
  the same signal.
- `visible_bind` gates a subtree on a Model key. Declare everything, default
  hidden, reveal from Lua.
- `anchor` pins a node (`center`, `top_left`, …); nodes without one flow in
  their parent (`row` / `cell` / `stack` / `grid` / `list`).

### `styles` — this scene's layout blocks

Whatever style paths your tree names beyond the global defaults live **in your
scene file**, under `styles`. Colors are `$token` refs — the palette stays in
the theme:

```jsonc
"styles": {
  "clicktrainer": {
    "hud": { "panel": { "fill_top": "$stone3", "fill_bot": "$stone1",
                         "border": "$edge2", "radius": 4 } }
  },
  "modal": { ... }   // a scene may carry a shared-looking block it uses; the
                     // block still belongs to THIS scene, not to a shared file
}
```

A `theme` key inside `styles` is refused at parse — the palette cannot fork.

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

local function react_text(seconds)
  if seconds == nil or seconds < 0 then return "—" end
  return string.format("%.0f ms", seconds * 1000.0)
end

function M.derive()
  return {
    hits = string.format("%d", (Model and Model.hits) or 0),
    accuracy = string.format("%.0f%%", (Model and Model.accuracy_pct) or 0),
    react_last = react_text(Model and Model.react_last_s),
  }
end

return M
```

The catalog's script goes further: it **seeds** every demo control's initial
value (yielding once a committed value echoes back), lights the active nav
bookmark by returning a style path per bookmark, and gates the Paged Menu
card's tab rails off the bound page/tab — all scene logic, all in the Lua.

### `arrange()` — which components are on, and how they're configured

Each key is a node id from the scene's tree; each entry turns that component
on/off and may **override its props** — any key beyond the structural five
(`on`, `anchor`, `offset`, `resizable`, `movable`) lands on the node exactly
as the same key would in the scene JSON. Called on change, not per frame.

For a scene whose tree gates slices on a selection (pages, tabs), return which
gate keys are on — see `populous.lua`, the reference:

```lua
function M.arrange()
  local page, tab = (Model and Model.page) or 0, (Model and Model.tab) or 0
  return {
    ["shown_p0_t0"] = { on = (page == 0 and tab == 0) },
    ["shown_p0_t1"] = { on = (page == 0 and tab == 1) },
  }
end
```

For a scene that configures a component's features, the entry carries the
props — the splashes are the reference:

```lua
-- TegLogo.lua (shipped): the splash node's image + fade timeline (seconds).
function M.arrange()
  return {
    splash = {
      on = true,
      image = "package/sensorium/assets/elideus_productions_yellow.png",
      fade_in = 0.6,
      hold = 1.2,
      fade_out = 0.6,
    },
  }
end
```

### `react(sig)` — signals → intents

Given this frame's fired signal names, update your own remembered state and
return outbound intents (navigation the kernel routes, a game action). Called
only when something fired. The splashes route with it: the engine reports
`done` (timeline complete), `confirm` (click / A / Enter / Space), or `cancel`
(Esc / B), and the returned intent — `next` or `exit` — is fired as the
scene's result and routed by the scene FILE's `exits` map:

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

Non-pair scripts — anything not named for a scene — live in `scripts/shared/`
(today: `settings.lua` until settings becomes a scene, plus the dormant
benches' old HUD scripts, which dissolve into pairs as each bench migrates).

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

**`ui_stages.json` holds the RTT stage sources** — named offscreen sub-scenes
(lighting preset, camera, content layers) that a tree's `rtt` node composites
into its rect. The node says where; the stage says what.

All three merge into one root at load, then tokens resolve — so a dotted style
path behaves exactly as if everything were one file, while each file owns one
nature of thing.

---

## Components

Components are Rust — drawn, laid out, and hit-tested by the engine, fully
featured. You configure features from data (tree props) and logic (your Lua);
you never define a component in JSON or Lua.

**The live reference is the Component Catalog scene** (Developer realm): one
card per kind with every feature on. When you wonder what a `paged_menu` or a
`gauge` can do, launch it. The current interactive vocabulary (see
`RUST_COMPONENT_KINDS` for the authoritative list): button, panel, sprite,
rune_corners, tooltip, checkbox, toggle, radio, tile, pill_toggle, tabs,
select, slider, stepper, text_field, list, context_menu, gauge,
resource_gauge, stat_dot, action_slot, medallion, badge, splash, popup_panel,
paged_menu — plus the structural kinds (screen, cell, row, stack, grid, rtt,
text, option).

A new component = a new arm in the engine's `component.rs`, an entry in the
kinds list, and a catalog card. There is no other tier.

---

## Input is signals

Nothing binds to a key. Devices resolve to **signals** (Confirm, Cancel, Menu,
Nav*, …) in the central pump; a mouse click IS a Confirm at the pointer's
target. Your scene participates two ways:

- **Declare intents as data** on tree roots: `"on_menu": "pause_open"`,
  `"on_cancel": "back"` — the walker consumes the signal at the focused (or
  hit) component and fires your named result.
- **Consume results**, not devices: `action` names arrive in the frame's
  results; scene-level results route through your file's `exits`.

If you find yourself wanting a keycode, you are on the wrong layer.

---

## Strings

Every display string is a `$token` into the stringtable
(`content/data/stringtable.json`), resolved at draw. Raw display literals in a
tree or published from Rust fail the gates. Copy composed in your pair script
(a phase line, a countdown) is content — compose it from resolved pieces the
engine hands you or tokens you name.

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
5. Run the gates (below); the manifest test now covers your exits and
   behaviour binding automatically.

---

## The gates your scene must pass

- `ui_theme_json_is_the_theme_and_nothing_else` — you didn't touch the theme
  file except tokens.
- `no_component_block_lives_in_a_shared_file` — your blocks are in YOUR file.
- The manifest gate — your behaviour resolves; your exits point at real scenes.
- The strings gates — no raw display literals in the tree or the Model channel.
- The pair-script gate — your Lua loads and derives (write one per scene; see
  `the_pair_script_derives_the_display_strings` in Click Trainer).
- The `.tree.json` absence gate — that form is dead; don't reinvent it.

---

## Sharp edges

- **The pair is by name.** `Goto { id }` = the scene file's name. Case matters.
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
- **The shell furniture interim**: pause/confirm/settings ride
  `Main.scene.json`'s styles until they become scene pairs — don't copy that
  pattern for a new scene.
