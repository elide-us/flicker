# flicker-script

The **Luau host** — the one and only place the engine and end-user Lua meet. It embeds a Luau
VM (via `mlua`) and runs a screen's **pair script**: the `SceneName.lua` half of the five-line
UI architecture, which owns a scene's runtime logic while Rust owns its structure and drawing.
Every value that crosses this seam is plain data — a named scalar, a draw command, an input
snapshot — **never** an engine handle, GPU resource, or borrow. `mlua` is confined to this
crate; no other workspace crate depends on it or touches the VM.

Vocabulary this README uses (all flicker terms):
- **Pair script** — the `<scene>.lua` that partners a `<scene>.scene.json`. The JSON declares
  the component tree + anchors; the Lua supplies logic. This crate loads and runs it.
- **Model** — the per-frame `name → value` table the engine publishes to the script (the `Model`
  Lua global). How a script reads live engine data (fps, a slider's current value, a phase name).
- **Signal** — an abstract input event (`Confirm`, `Cancel`, `Menu`, `NavUp`, …); the catalog is
  [`flicker-input-core`](../../input/flicker-input-core/README.md). Nothing wires to a key.
- **Intent** — a named result a script fires *outward* (a navigation target, a game action) for
  the kernel/shell to route. A signal a script *answers* is an intent expressed as its firing.
- **Arrangement** — which components are on, where they sit, and their behavioural flags, decided
  by the script's `arrange()` and applied over the *static* JSON tree (the script never rebuilds
  structure).
- **Walker** — the Rust component walker in `flicker-widgets` that lays out, draws, hit-tests,
  and resolves binds on a `UiNode` tree. It is the primary consumer of this crate's data types.

> Design of record — why it is shaped this way, decisions, and history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:** `flicker-input-core` (the `InputState` snapshot fed to the legacy `update` path),
  `mlua` (the Luau VM — confined here), `serde` / `serde_json` (JSON marshalling for
  `set_global_json` and `parse_ui_json`).
- **Used by:** `flicker-widgets` (direct dep — the walker consumes `UiNode` / `ValueMap` / `Value`
  / `HudCommand` and drives `set_global_json`); `flicker` (core) re-exports the whole crate as
  **`flicker::script`**, which is how the shell and every scene crate import it
  (`use flicker::script::{ScriptHost, ValueMap, UiNode, …}`); `flicker-shell` (runs the full
  `arrange` → `apply_props` → `react` → `derive` lifecycle for menu / splash / loading / settings /
  shared modals); every scene crate (`flicker-clicktrainer`, `-populous`, `-quartermaster`,
  `-sablework`, `-solarbirth`, `-pocclusters`, `-componentcatalog`, and the rest — each builds a
  `ScriptHost` from its embedded `*.lua` and folds `derive()`).
- **Reads from the content tree:** only through `ScriptHost::from_file(path)`, when a caller
  supplies one. Today the scene crates embed their `<scene>.lua` as a compiled-in string constant
  and call `ScriptHost::new` instead. The authored pair scripts and scene files themselves live
  under `Alpha/content/sensorium/` — see **[the Sensorium authoring guide](../../../content/sensorium/README.md)**
  for how to write them; this README documents the Rust host they run on.

## The frame lifecycle — the one thing to internalise

The modern (pair-script) path runs three hooks with a **strict ordering** the caller must honour:

1. `set_model(&raw)` — publish the scene's raw runtime variables to the `Model` global.
2. `derive()` — the script reads `Model`, returns *derived* values (display strings, per-component
   styles, visibility gates). **Must run after `set_model`**, because it reads what step 1 published.
3. Fold the derived map into the frame Model (`for (k, v) in derived.entries() { m.set(k, v) }`),
   then hand `m` to the walker, which resolves every node's `bind` / `visible_bind` against it.

`arrange()` and `react()` are **on-change**, not per-frame: `arrange()` when a panel opens or the
layout is reconfigured; `react(sig)` only when a signal fires. Calling them every frame is a
performance bug, not a correctness one.

## Public API

### `ScriptHost` — load a module, then drive its channels

| Item | What it is for | The one thing to know |
|---|---|---|
| `ScriptHost::new(source, chunk_name)` | Load + evaluate a module from a source string. | Runs the module validator (`check_contract`) at load, so a malformed script fails **loud at load**, not mid-frame. `chunk_name` labels errors. |
| `ScriptHost::from_file(path)` | Same, reading the source off disk. | The only place the crate itself touches the content tree; path doubles as the chunk name. |
| `ScriptHost::new_with_modules(source, chunk_name, &[(name, source)])` | Like `new`, but first installs a minimal `require` so modules can `require("name")` each other. | Legacy per-file composition primitive; **no shipped scene uses it today** (its consumer, the `ui/<kind>.lua` tier, was deleted 2026-08-10). Retained as a toolbox capability, pinned by `require_composes_per_file_modules`. |
| `set_model(&ValueMap)` | Publish the per-frame **Model** (`Model` global). | Call **once per frame, before** `derive`/`update`/`draw`. Replaces the previous frame's `Model`. |
| `derive() -> Option<ValueMap>` | **Modern hook.** Run `derive()`; return derived Model values. | `None` when the module has no `derive`. Reads the `Model` set by `set_model` — order matters. |
| `arrange() -> Option<Arrangement>` | **Modern hook.** Run `arrange()`; return per-component on/anchor/offset/flags + prop overrides, keyed by id. | `None` when the module has no `arrange`. **On-change only.** |
| `react(&ValueMap) -> Option<ValueMap>` | **Modern hook.** Run `react(sig)` with the fired signals; return outbound intents. | `None` when the module has no `react`. **On signal only.** Signal-name-agnostic (see Interactions). |
| `ui_tree() -> Option<UiNode>` | **Mid path.** Run the optional `tree()` builder; parse the returned table into a `UiNode` tree. | `None` for a module with no `tree`. The engine caches the result and re-calls only on a structural change. |
| `update(&InputState, sw, sh) -> ValueMap` | **Legacy immediate path.** Call Lua `update(mx, my, clicked, sw, sh, down)`; return the results map. | Reads the mouse snapshot directly (position, click *edge* `clicked`, *held* `down`). `down` is the 6th arg — older 5-arg scripts still work. **No shipped scene uses this path today.** |
| `draw(sw, sh) -> Vec<HudCommand>` | **Legacy immediate path.** Call Lua `draw(sw, sh)`; parse its command tables. | Unknown command kinds are skipped with a `tracing::warn`, not a hard error. **No shipped scene uses this path today.** |
| `set_texture_ids(&[(name, id)])` | Expose engine textures to the script as the `Textures` global (`name → id`). | For `HudCommand::Sprite` by name on the legacy path. Call once after load (again if the set changes). |
| `set_global_json(name, &Value)` | Marshal a static JSON tree into the global `name` (objects → tables, arrays → 1-indexed, null → nil). | The **layout/config** inbound channel — e.g. `set_global_json("UI", ui_theme_json)`. Live consumer: `flicker-widgets`. Call once at load (again to hot-reload). |
| `set_lua_module(name, source, chunk_name)` | Evaluate a shared Lua library and bind it to the global `name` (e.g. a `Widgets` toolkit). | Code, not data, but confined to the VM — does not widen the data boundary. Call once at load. |

### The pair-script hook contract (what a human authors)

A pair script returns a module table (`local M = {} … return M`) exposing one or more hooks:

| Lua hook | Rust method | Engine calls it | Receives | Returns |
|---|---|---|---|---|
| `M.derive()` | `derive` | each frame, after `set_model` | reads the `Model` global | `{ key = value }` — derived Model entries (bool/number/text) |
| `M.arrange()` | `arrange` | on change | — | `{ id = { on, anchor, offset, resizable, movable, ...props } }` |
| `M.react(sig)` | `react` | when a signal fires | `sig` = table of fired signal names | `{ intent = value }` — outbound intents |
| `M.tree()` | `ui_tree` | on structural change | — | a nested node table → `UiNode` tree (mid path) |
| `M.update(...)` + `M.draw(...)` | `update` / `draw` | each frame (legacy) | mouse snapshot; `Model` / `Textures` globals | results map / draw-command list |

**The module validator (`check_contract`, load-time gate).** A module must expose **at least one of**
`arrange` / `react` / `derive` (modern), **or** `tree` (mid), **or** `update` **and** `draw` (legacy —
both required). A module exposing none of these fails loud at load:
`"script must expose `arrange`/`react`/`derive`, or `tree`, or `update` + `draw`"`. This gate is the
fix for a real in-window regression: the validator once predated `derive()` and rejected every
new pair script, so benches silently fell back to raw numbers (MCP incident 1CD45785). See Sharp
edges for the residual seam.

### Contract data types

| Item | What it is | The one thing to know |
|---|---|---|
| `Value` | The only currency crossing the boundary: `Bool` / `Number(f64)` / `Text`. | Deserializes untagged (`true` / `3.5` / `"x"`). `From` impls for `bool`, `f64`, `f32`, `i64`, `u32`, `usize`, `&str`, `String`. All Lua numbers marshal through `f64`. |
| `ValueMap` | A `name → Value` map — both the inbound Model and the outbound results/intents. | Build with `new` / `with` / `set` / `extend`; query with `get` / `is_on` / `number` / `text` / `entries`. Names are defined by whichever side fills it. Typed getters do **not** coerce (`number("label")` on a text value → `None`); a missing name → `is_on` false / `number`/`text` `None`. |
| `UiNode` | One placed component instance — a node of the tree the walker lays out. | `component` is required (alias `type`). Placement: `anchor` + `offset`, or flow (`size`/`grow`). Behaviour: `bind`, `action`, `visible_bind` (alias `visible`), `enabled_bind` (alias `enabled`), `nav_ordinal` + `tab_group`. Every non-structural scalar key lands in `props`. |
| `Arrangement` / `ComponentArrange` | The `arrange()` result, keyed by component id. A component **absent from the map is off**. | Two apply-halves: `to_model()` flattens on/placement/flags into Model binds; `apply_props(&mut tree)` writes each entry's scalar prop overrides onto the id-matched node. Both apply over the static tree — Lua never rebuilds structure. |
| `HudCommand` | A draw command: `Rect` / `Sprite` / `Text` / `TextCaret` / `Panel` / `Clip`. | Coordinates in HUD pixels (origin top-left); colours RGBA `0.0..=1.0`; `layer` is painter's order relative to the scene's base. **`Clip` is walker-injected only** — a Lua `draw()` cannot emit it (see the draw-kind catalog). |
| `TextAlign` / `FontRole` / `UiAnchor` | Plain-data enums carried across the seam. | The crate has no renderer/font dep, so these are *data* — the render bridge maps `FontRole` → a concrete font family, `UiAnchor` → a screen corner. |
| `ScriptError` | `Io { path, source }` (file read failed) / `Lua(mlua::Error)` (VM error or wrong-shape return). | Every host method returns `Result<_, ScriptError>`. |

### Free functions

| Item | What it is | The one thing to know |
|---|---|---|
| `parse_ui_json(&serde_json::Value) -> Result<UiNode, String>` | The **one** reader that turns a scene/arrangement JSON object into a `UiNode`. | Shares `UI_STRUCTURAL_KEYS` with the Lua `tree()` parser, so a data-authored tree and a Lua-authored tree cannot drift. Rejects `template` / `slots` / missing `component` **loud** (the template tier was removed, 201F4F51). |
| `UiAnchor::from_name(&str) -> Option<UiAnchor>` | The single anchor-name → enum mapping (`"top_left"`, `"center"`, …). | Shared by the tree parser and the `arrange()` per-id anchor override, so the two paths resolve anchors identically. Unknown/absent → `None` (the node flows). |

## Interactions

- **Signals it answers / intents it fires.** `react(sig)` is **signal-name-agnostic**: it marshals
  whatever `ValueMap` of fired-signal names the caller hands in, and returns whatever intent map the
  script builds — it neither knows nor matches keys or buttons. The signal vocabulary is the
  caller's: the shell passes names like `"confirm"` / `"cancel"` / `"done"` (its own constants), which
  originate from the `ActionSignal` catalog in
  [`flicker-input-core`](../../input/flicker-input-core/README.md). This crate does **no** key/button
  matching anywhere — signals in, intents out, both as named data.
- **Model keys — published.** `set_model` writes the `Model` global; `derive()` returns more keys the
  caller folds in via `entries()`. Owner of these names: the scene's pair script + its scene crate.
  `arrange().to_model()` publishes, per component id: **`<id>`** (visibility — matches a node's
  `visible_bind`), **`<id>_anchor`**, **`<id>_off_x`**, **`<id>_off_y`**, **`<id>_resizable`**,
  **`<id>_movable`**. Owner: the `Arrangement`.
- **Model keys — bound.** A `UiNode`'s `bind` / `visible_bind` / `enabled_bind` are resolved against
  the frame Model by the **walker** (`flicker-widgets`), not by this crate. `apply_props` writes
  `arrange()` prop overrides straight onto the matching tree node.
- **What it hands other crates.** `Vec<HudCommand>` (to the render bridge in `flicker-widgets`),
  `UiNode` trees (to the walker), `ValueMap`s (results / intents / derived Model) to the shell and
  scene crates.
- **Globals it exposes to Lua.** `Model` (`set_model`), `Textures` (`set_texture_ids`), any named
  JSON tree (`set_global_json`, e.g. `UI`), any named library (`set_lua_module`), and `require`
  (only under `new_with_modules`).
- **Threads / workers / async.** None — every host call is synchronous, per-frame, on the caller's
  thread. (The workspace enables mlua's `async` feature, but this crate uses no async host calls.)

## The security boundary — what Lua may and may not touch

The Lua layer is **end-user-editable and runs on a client in the enemy's hands** (rule 69E82FE7), so
the boundary is load-bearing:

- **May:** read the data channels (`Model`, `Textures`, JSON globals), run pure Lua (`math`, `string`),
  keep its own remembered state between calls, and **return plain scalars** — an arrangement, a derived
  Model, intents, draw commands. Structure and behaviour it configures must be *knobs on hardened Rust
  components*, never new logic or structure.
- **May not:** receive or return an engine handle, GPU resource, or borrow — the type system permits
  only `Value` (bool/number/text) across the seam, so nothing else *can* cross. The VM is Luau, which
  ships no `io` / `package` / stock `require` and only a neutered `os`.
- **Enforced by:** the data-only type contract (the real, load-bearing property) and per-`ScriptHost`
  VM isolation — a script's blast radius is its own frame outputs, which the engine re-reads by type.
- **Not (yet) enforced:** the crate does **not** call mlua's `Lua::sandbox(true)` or otherwise freeze
  globals / drop bare `load`; it relies on Luau's defaults. The "freeze globals read-only, drop `load`"
  hardening was explicitly deferred (MCP decision DD9F3836) and is still absent — see Finding in the
  handoff, not a knob you configure here.

## Gates

The crate's contracts are pinned by these tests (`cargo test -p flicker-script`, 20 passing):

- `an_empty_script_is_still_refused` / `missing_entry_points_fail_to_load` — the module validator
  refuses a hook-less module at load.
- `tree_only_module_satisfies_contract` / `arrange_and_react_marshal_the_modern_contract` — a `tree`-only
  and an `arrange`/`react`-only module each satisfy the contract; `react` state persists across calls
  and a `launch` signal produces the nav intent.
- `arrange_entry_props_configure_the_component` — an `arrange()` entry's non-structural scalars become
  component props and `apply_props` lands them on the id-matched node.
- `arrangement_flattens_to_the_run_ui_model` — `to_model()` emits `<id>` visibility + `<id>_*` placement/flag binds.
- `model_round_trip` / `value_map_typed_accessors` — `Value` survives the boundary in both directions,
  types preserved; typed getters don't coerce.
- `set_global_json_marshals_nested_tree` — nested objects + 1-indexed arrays marshal faithfully.
- `ui_tree_parses_nested_component_tree` / `ui_tree_absent_on_legacy_module` — the `tree()` parse
  (structural reads, props sweep, bindings, anchor, offset, recursion) and the `None` case.
- `nav_props_parse_from_lua_with_defaults` / `nav_props_parse_from_json_with_defaults` — `nav_ordinal`
  + `tab_group` parse on both paths, defaults hold, and a **float-shaped ordinal** (`1.0`) reads as `1`.
- `a_template_or_slots_key_is_a_loud_load_error` — both parse paths reject `template` / `slots` loud.
- `draw_returns_rect_and_text` / `sprite_layer_and_align_parse` / `panel_command_parses` — the draw-kind
  parser and its colour/uv/gradient defaults.
- `click_inside_toggles_state` / `unknown_toggle_is_off` — the legacy `update` hit-test + result map.
- `require_composes_per_file_modules` — `new_with_modules` resolves peer `require`s to a cached singleton.

## Author catalogs (the magic strings)

- **`draw()` command kinds** (the `kind` field): `"rect"`, `"sprite"`, `"text"`, `"caret"`
  (→ `TextCaret`), `"panel"`. An unknown kind is dropped with a `tracing::warn`. **`Clip` has no Lua
  kind** — it is injected by the walker, so a script cannot emit it.
- **Node structural keys** (everything else on a node becomes a prop): `id`, `component` (alias `type`),
  `children`, `anchor`, `offset`, `size`, `grow`, `width`, `height`, `gap`, `pad`, `pad_x`, `pad_y`,
  `bind`, `action`, `visible`/`visible_bind`, `enabled`/`enabled_bind`, `nav_ordinal`, `tab_group`.
- **`arrange()` entry structural keys** (everything else is a component prop override): `on`, `anchor`,
  `offset`, `resizable`, `movable`.
- **Anchor names** (`UiAnchor::from_name`): `top_left`, `top`, `top_right`, `left`, `center`, `right`,
  `bottom_left`, `bottom`, `bottom_right`.
- **`align` strings** (`Text`): `"center"`, `"right"`; anything else → left. **`font` strings**:
  `"display"`, `"label"`, `"rune"`; anything else → body.

## Sharp edges

- **The name channel is unvalidated.** `set_model` / `derive` / `arrange` produce, and the walker
  consumes, flat `name → value` maps. This crate never checks that a produced key matches a bind or
  that a bind matches a produced key. A typo (`derive` returns `"acc"`, the tree binds `"accuracy"`)
  is **silent**: the bind resolves to nothing and renders as nothing. The loud-fail for authored names
  lives at the *parse* boundary (`template`/`slots`/missing-`component` reject) and downstream in the
  walker — not in the ValueMap channel. See the Sensorium README for the bind-resolution failure modes.
- **A mis-authored hook name fails loud here but can be swallowed upstream.** `check_contract` returns
  an `Err` for a module exposing no known hook (e.g. a typo'd `derive`). Most scene crates match that
  `Err`, log a `tracing::error`, and continue with `script = None` — which is **invisible in the GPU
  window** and shows raw/blank binds. Per-bench pair-script regression gates exist to catch this at
  build time (the 1CD45785 fix); a new wired scene must ship the same gate.
- **`derive`/`update`/`react`/`arrange` skip unsupported return types with a warning.** A hook that
  returns a table or `nil` for a key gets a `tracing::warn` and the key is dropped (not a frame
  failure). The warning is invisible in-window, so the symptom is a missing value, not an error.
- **Ordering is load-bearing and implicit in the API.** `set_model` must precede `derive`; `arrange` /
  `react` are on-change, not per-frame. Nothing in the type signatures enforces this.
- **Two placement representations, deliberately unified.** A node's anchor exists as the static
  `UiNode.anchor` *and* as an `arrange()` `<id>_anchor` override bind; both resolve through the single
  `UiAnchor::from_name`, so they cannot drift. This is by design, not a fork.
- **`Number` is always `f64`.** Integer-looking Model values marshal through `f64`; read them with
  `number()` and cast at the use site.
