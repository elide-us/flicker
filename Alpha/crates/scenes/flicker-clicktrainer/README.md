# flicker-clicktrainer

The aim-trainer bench, and the reference **2D scene crate**: a sprite game (a target box
and its lifetime bar) drawn on the screen surface, with a declarative vector HUD floating
over it, and the pointer routed correctly between the two. It is the smallest complete
example of a *scene crate* — a library that supplies one `Scene` behaviour, paired with an
authored scene file and a Lua pair script, launched by name from the `prism-alpha` roster.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

**Vocabulary used below** (each is a flicker word, not a general one): a **scene file** is
the authored `*.scene.json` that declares the component **tree**; its **pair script** is the
same-named `.lua` that turns raw numbers into display strings; the **Model** is the per-frame
key→value table the engine hands to Lua and to the tree's binds; the **walker** is the Rust
pass that lays out, hit-tests and draws the tree; a **signal** is a device-independent input
verb (`Menu`, `PrimaryAction`); an **intent** is a signal binding declared as data on the
scene file's root; a **result** is a name fired into the frame's results map; an **exit** is
an authored destination a fired result routes to; a **surface** is the drawing ground the
scene's own 2D/3D element occupies under the UI.

## Where it sits

- **Builds on:** `flicker` (the umbrella: `Scene`/`Transition`, `Renderer`/`FrameGraph`,
  `ScriptHost`/`ValueMap`, and the walker entry points `run_ui`/`render_hud`) ·
  `flicker-input-core` (the `ActionSignal` vocabulary — catalog in
  [`../../input/flicker-input-core/README.md`](../../input/flicker-input-core/README.md)) ·
  `flicker-input-router` (`InputHandler`/`Router`/`Flow`) · `flicker-shell` (`PauseScene`,
  `Theme`, `input_profile`) · `fastrand` · `serde_json` · `tracing`.
- **Used by:** `prism-alpha` only, and only through [`scene`](#public-api). Its roster entry
  is in [`../../../prism-alpha/src/main.rs`](../../../prism-alpha/src/main.rs) (`roster()`,
  id `"clicktrainer"`, title `"Click Trainer"`, realm `REALM_ADVENTURER`).
- **Reads from the content tree:**

| Path | When | If missing |
|---|---|---|
| [`content/sensorium/scenes/clicktrainer.scene.json`](../../../content/sensorium/scenes/clicktrainer.scene.json) | at launch, by the kernel — the parsed `SceneDef` is handed to `scene()` | no `tree` ⇒ `tracing::error!` and **the game still plays with no HUD** |
| [`content/sensorium/scripts/clicktrainer.lua`](../../../content/sensorium/scripts/clicktrainer.lua) | compiled in via `include_str!` at `src/lib.rs:52`, loaded in `ClickTrainer::new` | load error ⇒ `tracing::error!` and the HUD binds raw numbers, not display strings |
| [`content/sensorium/resources/ui_theme.json`](../../../content/sensorium/resources/ui_theme.json) | `enter`, via `load_shared_styles` | the scene's `$token` colours fall back to compiled defaults, silently |
| [`content/data/stringtable.json`](../../../content/data/stringtable.json) | per draw — the ten `$ct_*` tokens the tree names | the raw token text draws |

To change what the HUD looks like or says, edit the scene file — see
[`../../../content/sensorium/README.md`](../../../content/sensorium/README.md) for the
authoring format. This file does not re-teach it.

## Public API

Three items, all reachable from `lib.rs`.

| Item | For | The one thing to know |
|---|---|---|
| `pub fn scene(def: &SceneDef) -> Box<dyn Scene>` | the roster factory — the only intended entry point | The `SceneDef` is the *parsed scene file*; the kernel resolves it from the manifest when a menu row fires `Goto { id: "clicktrainer" }`. |
| `pub struct ClickTrainer` | the `Scene` implementation | All frame state lives here. Nothing outside the crate constructs it today — see the single-host note below. |
| `pub fn ClickTrainer::new(def: &SceneDef) -> Self` | the unboxed constructor | Loads the pair script and clones the authored tree, but places **no target**: `enter` does that, and needs the `Renderer` for the screen size. |

`ClickTrainer` and `ClickTrainer::new` currently have no caller outside this crate —
`prism-alpha` registers `scene` only, and there is no binary target. They are the seam a
second host would use, kept deliberately; the actual drift is their doc comment at
`src/lib.rs:61`, which still names a `main` and a "paperdoll launcher" that do not exist.

The `Scene` trait methods it implements are `enter`, `update`, `render`. It leaves
`input_context`, `route`, `is_overlay`, `pointer_captured` and `exit` at their defaults.

**Tuning — compiled, not authored.** These are private `const`s in `src/lib.rs`; the scene
file's `params` block is empty and is never read, so changing difficulty means a rebuild.

| Const | Value | Meaning |
|---|---|---|
| `TARGET_START_SIZE` | `90.0` | target edge in px for the first hit |
| `TARGET_MIN_SIZE` | `34.0` | floor of the shrink ramp |
| `TARGET_SHRINK_PER_HIT` | `1.5` | px of edge lost per hit |
| `TARGET_LIFETIME` | `1.15` | seconds before a target times out (a miss) |
| `HUD_RESERVE_W` / `HUD_RESERVE_H` | `320.0` / `340.0` | top-left px region targets re-roll out of, so nothing spawns under the panel |

## Interactions

### Signals it answers

Signals only — never keys or buttons; what produces a signal is profile data, out of scope
here. The chain in `update` is three layers, highest priority first:

```
[ROOT] RootHandler    src/route.rs  — declares World; consumes nothing
[1]    WalkerHandler  flicker-widgets — the HUD panel: pointer-consume + declared intents
[2]    GameplayBase   src/route.rs  — a click that bubbled past the HUD
```

| Signal | Channel | Effect |
|---|---|---|
| `Menu` | **declared intent** — `"on_menu": "pause_open"` on the scene file's root node; consumed at layer 1, drained by `walker.take_fired()` | `src/lib.rs:344` returns `Transition::Push(PauseScene)`. Both edges are consumed, so it never reaches gameplay. |
| `PrimaryAction` (Press) | layer 1 while the pointer is over the panel (`hud_hit`) ⇒ **consumed**; otherwise falls to `GameplayBase` | Over the panel: the walker fires the hit node's `action`. Over the play field: `src/lib.rs:364` scores the **current pointer position** as a hit or a miss. |
| `SecondaryAction` | layer 1, consumed while `hud_hit`; unhandled otherwise | Nothing. |
| `Confirm` · `Cancel` · `NavUp` · `NavDown` · `NavLeft` · `NavRight` · `PanelNext` · `PanelPrev` · `ChordBegin` | subscribed by layer 1, but it holds **no focusable tree** here, so they pass through and no layer below answers them | **Nothing.** There is no non-pointer path to RESET — see Sharp edges. |

The pair script's `react(sig)` channel is **not used**: `clicktrainer.lua` defines only
`derive()`, and the crate never calls `ScriptHost::react`.

### Results and exits it fires

| Result | Produced by | Consumed by |
|---|---|---|
| `hud_hit` | the walker, every frame the pointer is inside the styled `stats_panel` cell or the RESET button | `src/lib.rs:314` — folded into the walker layer's pointer-consume for this frame |
| `reset` | the `button` node's `"action": "reset"` in the scene file | `src/lib.rs:315` calls `ClickTrainer::reset()` — zeroes both counters, all three reaction times, and the shrink ramp |
| `pause_open` | the declared `Menu` intent | `src/lib.rs:344` → `Transition::Push` |

**Exits: none.** The scene file's `"exits": {}` is empty, and the crate does not implement
`Scene::route`, so an authored exit here would be inert. The only stack move this scene ever
makes is the pause `Push`.

### Model keys

Two hops. Rust publishes **raw** values into the script; `derive()` returns **display**
values; the merged map is what the tree's binds read.

| Key | Type | Published by | Read by |
|---|---|---|---|
| `hits` · `misses` | Number → **then overwritten as Text** | `src/lib.rs:225-226`, then `clicktrainer.lua:25-26` | the script sees the *number*, the tree's `text_bind` sees the *string* |
| `accuracy_pct` | Number, `0..100` | `src/lib.rs:227` | `clicktrainer.lua:23` only — nothing binds it |
| `stat_last_s` · `stat_best_s` · `stat_avg_s` | Number, seconds; **`-1.0` means "no data yet"** | `src/lib.rs:228-230` | `clicktrainer.lua:13-18` only — nothing binds them |
| `accuracy` | Text, e.g. `"83%"` | `clicktrainer.lua:27` | tree `text_bind` |
| `stat_last` · `stat_best` · `stat_avg` | Text, e.g. `"210 ms"` or `"—"` | `clicktrainer.lua:28-30` | tree `text_bind` |
| `sig_pause_open` | Bool `true`, transient | `UiIntents::mirror_into` at `src/lib.rs:246` | **nothing** — it is published for scripts that want to observe the intent; this one does not |

The tree binds exactly six keys, all `text_bind`, all Text: `hits`, `misses`, `accuracy`,
`stat_last`, `stat_best`, `stat_avg`. There is no `bind`, `visible_bind`, `enabled_bind` or
`style_bind` in this scene. A `text_bind` naming a key the Model does not carry draws an
empty string with no warning, so adding a row means editing all three files.

### Style paths the tree names

All resolve inside the scene file's own `styles.clicktrainer` block, merged over the shared
theme root. Colours are `$token` refs into `ui_theme.json`
(`$stone3` `$stone1` `$edge2` `$ink_bright` `$dim_soft` `$bronze` `$dim` `$rune_glow_hi`
`$faint` — all nine verified present).

`clicktrainer.panel` · `clicktrainer.divider` · `clicktrainer.title.color` ·
`clicktrainer.subtitle.color` · `clicktrainer.label_color` · `clicktrainer.value_color` ·
`clicktrainer.accent` · `clicktrainer.hint.color`

Every **other** key in that block — `margin`, `row_h`, `label_size`, `value_size`, the
`rows` array, `reset.*`, `title.text`, `title.size`, `subtitle.text`, `subtitle.size`,
`hint.text`, `hint.size`, `hint.gap_top` — is named by nothing and reaches nothing. See
Sharp edges before editing them.

### What it hands the shell

- One `FrameGraph` per frame whose `root` pass draws the two sprites — the screen surface's
  2D element, below the UI (`src/lib.rs:407-418`).
- The walker's `HudCommand` list, blitted by `render_hud` after the graph executes.
- `Transition::Push(PauseScene)` on `pause_open`, built from a `Theme` created once in
  `enter` and the `"World"` context map taken from `flicker_shell::input_profile()`
  (falling back to `InputMap::wasd_and_mouse` if the profile has no `"World"` entry).
- The window title, set once in `enter`.

No threads, no workers, no async.

## Gates

`cargo test -p flicker-clicktrainer` — 7 tests, all green.

| Test | What it holds |
|---|---|
| `tests::the_pair_script_derives_the_display_strings` | Builds the scene from the **real** scene file and runs the real `hud_model()` path: all six display keys must come back as Text, `hits` = `"0"`, `accuracy` = `"100%"`, `stat_last` = `"—"`. A `derive()` that throws fails the build instead of shipping numbers. |
| `tests::tree_is_well_formed_and_declares_the_pause_intent` | Every component kind in the scene file is one the engine knows; every display literal is a `$token`; no raw display copy is published into the Model from `lib.rs`; the root declares `Menu → "pause_open"`. |
| `tests::hud_routes_clicks_and_draws` | The click-routing contract through the real tree: a pointer on the panel sets `hud_hit`, one on the play field does not, and a click on RESET fires `reset` **and** `hud_hit` — never a game miss. |
| `route::tests::hud_hit_swallows_the_click_before_the_base` | The defining behaviour, structurally: with `hud_hit` the walker layer consumes `PrimaryAction`; without it the click reaches `GameplayBase`. |
| `route::tests::dispatch_fires_the_declared_pause_intent` | `Menu` fires `pause_open` at layer 1, and the root has no `Menu` arm. |
| `route::tests::root_declares_world_and_consumes_nothing` | `RootHandler` declares `World` and passes every signal. |
| `route::tests::gameplay_base_records_the_click` | Only a `PrimaryAction` **Press** is a game click; a Release is not. |

Two gates in `prism-alpha/src/main.rs` also cover this crate:
`roster_holds_the_migrated_benches` pins `"clicktrainer"` second in the Adventurer realm, and
`every_authored_scene_resolves_and_every_bench_is_authored` binds the roster id to the scene
file's existence.

## Sharp edges

- **RESET is pointer-only.** `update` builds its walker layer with `WalkerHandler::hud(…)`
  and never calls `.with_nav(…)`, and the scene file declares no `tab_group` anywhere — so
  the layer holds no focusable tree, `Confirm`/`Nav*`/`Panel*` pass straight through, and
  the RESET button cannot be reached or pressed by anything but a pointer. Every other
  migrated bench calls `.with_nav`.
- **`hud_hit` is implied by having a style, not declared.** The panel absorbs the pointer
  because it is a `cell` that *carries a `style` prop* and contains the cursor. Delete
  `"style": "clicktrainer.panel"` to make the panel transparent and the panel stops
  swallowing clicks — every RESET press would then also score as a game miss. Nothing warns.
- **`-1.0` is the null.** "No hit yet" crosses the Rust→Lua boundary as the number `-1.0`
  (`src/lib.rs:218` maps `INFINITY` to it; `clicktrainer.lua:14` reads `< 0`). It is a bare
  literal on both sides, named nowhere.
- **`hits` and `misses` are a number in the script and a string in the tree.** The derived
  Text overwrites the raw Number under the same key. The four other raw keys carry a `_pct`
  or `_s` suffix and do not collide; these two were never renamed.
- **A typo'd style path resolves to nothing, silently.** Dotted paths walk the styles tree
  and a missing segment yields null (`flicker-widgets/src/component.rs:8786`), after which
  every reader falls back to a compiled default. A misspelled `clicktrainer.panl` draws a
  plausible-looking unstyled box.
- **Half the `styles.clicktrainer` block is inert, and one entry disagrees with reality.**
  It predates the declarative tree; the tree now carries its own `width`, `pad`, `size` and
  `text_size`. `reset.h` says `36`, but the button is authored `"size_class": "md"`, which
  is a compiled `32.0` (`flicker-widgets/src/component.rs:4603`). Editing these keys does
  nothing.
- **The HUD keep-out is a compiled constant.** `HUD_RESERVE_W/H` (320×340) is hand-matched to
  the panel's current anchor, offset and height. Move the panel, widen it, or add a row and
  targets start spawning underneath it — where they are drawn over and cannot be clicked
  (the HUD consumes the pointer), so they always time out as misses. Nothing warns.
- **`sig_pause_open` arrives late.** The mirror is published on the first frame *after* the
  intent fires — which, because the pause overlay freezes this scene, means the first frame
  after the overlay closes.
- **`PrimaryAction` is scored at the pointer wherever it comes from.** The hit test is
  per-frame against `input.mouse_position`, not per-event, so a `PrimaryAction` press from a
  non-pointer device scores against wherever the pointer happens to be resting.
- **`enter` must run before `update`.** `src/lib.rs:345` does `self.ui_theme.expect(…)`; the
  theme is built in `enter`.
- **Losing the pair script is a soft failure.** If `clicktrainer.lua` fails to load the game
  still plays and the HUD still draws — with empty stat values, because the tree binds the
  derived keys and only the raw ones exist. The gate above is what keeps that off the screen.
