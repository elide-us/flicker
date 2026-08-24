# flicker-input-core

The crate that owns **what the engine can be asked to do**. It defines the one
signal vocabulary (`ActionSignal` — 62 named intents such as `Confirm`, `NavUp`,
`Dodge`), the per-frame input snapshot every device fills, the binding/context
model that turns a physical control into a signal, and the deterministic resolver
that turns a snapshot into edges. It is the leaf of the input stack: it touches no
platform, spawns no threads, and depends on nothing else of ours — so every other
crate in the repo can name a signal without pulling in a window.

Everything downstream speaks **signal names**, never controls. This crate is the
catalog those names come from: a scene declares `"on_menu": "pause_open"`, the
settings key-bindings page derives its rows from `ActionSignal::rebindable()`, and
the stringtable carries a `$sig_*` display string for each — all from the single
enum in `src/signal.rs`.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

---

## Contents

1. [Where it sits](#where-it-sits)
2. [The frame, end to end](#the-frame-end-to-end)
3. [The signal catalog](#the-signal-catalog) — all 62, by group
4. [Naming a signal](#naming-a-signal)
5. [Public API](#public-api)
6. [Interactions](#interactions)
7. [Gates](#gates)
8. [Sharp edges](#sharp-edges)

---

## Where it sits

**Builds on:** nothing of ours. Only `glam` (vectors) and `serde` (the persisted
profile). No `winit`, no `gilrs`, no threads — that is the crate's defining
constraint, and the reason it can be a dependency of everything.

**Used by:**

| Crate | Takes |
|---|---|
| `flicker-input-device` | The types it fills — `InputState` (held state + edges), `AnalogCache`, the device enums it translates `winit`/`gilrs` values into |
| `flicker-input-router` | `Fired` → wrapped into its `InputEvent`; `InputContext` for the active-context gate |
| `flicker-app` | Owns the one production `Resolver` + `ContextualBindings` (the *pump*, `runner.rs`), and latches the analog sample into the snapshot |
| `flicker-shell` | `InputProfile` persistence, `RebindCapture`, and the settings key-bindings page derived from `ActionSignal::rebindable()` / `SignalGroup::ALL` |
| `flicker-widgets` | `ActionSignal::from_name` — resolves a scene file's `on_<signal>` prop into a signal (`intents.rs`) |
| `flicker-scene`, `flicker-globe`, `flicker-script`, every `scenes/*` crate | `ActionSignal` + `EventKind` to match on the events the pump hands them |
| `flicker` (umbrella) | Re-exported as `flicker::input_core` |

**Content files it reaches into:** none directly — it is pure Rust. It *names*
two content artefacts that other crates resolve:

| Path | What of it | If it is missing |
|---|---|---|
| `Alpha/content/data/stringtable.json` | Every `token()` this crate mints (`$sig_*`, `$siggroup_*`, `$key_*`, `$mbtn_*`, `$pad_*`, `$axis_*`, `$mouse_x`/`$mouse_y`) must exist as a key | The widgets stringtable draws the raw `$token`; the shell gate `every_catalog_token_resolves_in_the_shipped_table` fails |
| the app's per-user `settings.json` | The serialized `InputProfile` (the player's bindings) | `InputProfile::default()` is used; nothing errors |

---

## The frame, end to end

Six steps, one per frame, in this order. Only step 4 is this crate's resolver;
steps 1–3 and 5–6 are named here because a caller has to drive them.

```
1. device   winit/gamepad events  ->  InputState   (held state + ordered edges)
2. app      analog sample         ->  InputState::set_analog_latch
3. app      the active surface declares its InputContext  ->  push onto the stack
4. CORE     Resolver::resolve_frame(&bindings, &cfg, &snapshot, tick, &mut out)
                                  ->  Vec<Fired> { signal, kind, control }
5. router   Fired -> InputEvent -> the handler chain (capture, then bubble)
6. device   InputState::clear_frame_edges()   (held state survives; edges reset)
```

Vocabulary used above and throughout:

- **signal** — a named *intent* (`Confirm`, `Dodge`), never a control. The whole
  point of the crate.
- **binding** — one physical control (`InputBinding`) attached to a signal. Player
  data, not contract.
- **context** — which binding table is live (`World`, `Menu`, `Flying`, …). A
  stack, so a menu opening over the world pushes and closing pops.
- **profile** — the serializable unit that holds all of a player's context maps
  plus their analog tuning. What `settings.json` stores.
- **the pump** — `flicker-app`'s `InputPump`: the one place in the running app
  that owns a `Resolver` and a `ContextualBindings`. A scene does not own either;
  it reads the events the pump hands it.
- **the walker** — `flicker-widgets`' UI tree traversal, the consumer that turns a
  nav signal into focus movement.
- **the Model** — the per-frame key→value table the engine hands to a scene's Lua
  pair script.

Two things do **not** ride the edge stream and are queried directly instead —
because a camera needs a value every frame, not an event:

| Query | Returns | Integrate as |
|---|---|---|
| `ContextualBindings::signal_axis(sig, &snap, &cfg)` | `0.0..1.0` deflection of the signal's own direction | a **rate** — multiply by `dt` |
| `ContextualBindings::signal_pointer_delta(sig, &snap)` | pixels of gated mouse motion this frame | a **delta** — use as-is, no `dt` |
| `ContextualBindings::signal_held(sig, &snap, &cfg)` | `bool`, deadzone-aware | a stance |

Direction lives in the *signal*, not in the sign: `LookLeft` and `LookRight` each
report how far they themselves are deflected, and the caller subtracts. Multiple
bindings on one signal report the **largest**, never the sum, so binding a signal
twice cannot make it twice as fast.

---

## The signal catalog

All 62 variants of `ActionSignal`, grouped exactly as `ActionSignal::group()`
groups them. The **Scope** column is `ActionSignal::rebind_scope()`:

- **Player** — listed and rebindable on the settings key-bindings page. This is
  the set `ActionSignal::rebindable()` yields (29 signals).
- **Locked** — engine-owned; catalogued but not player-editable (26).
- **Reserved** — declared, deliberately not wired to anything yet (7).

**†** marks a signal with **no default binding in either shipped profile**
(`default` and `xbox_souls`). A `Player †` signal is bindable by the player on the
settings page. A `Locked †` or `Reserved †` signal cannot fire at all today —
declaring an intent on one is a silent no-op (see [Sharp edges](#sharp-edges)).

Add a variant and it must be named, labelled, grouped and scoped or the crate does
not compile — every match over `ActionSignal` is exhaustive with no `_` arm.

#### Movement — `$siggroup_movement`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `MoveForward` | Player | `$sig_move_forward` | Move Forward |
| `MoveBackward` | Player | `$sig_move_backward` | Move Backward |
| `StrafeLeft` | Player | `$sig_strafe_left` | Strafe Left |
| `StrafeRight` | Player | `$sig_strafe_right` | Strafe Right |
| `MoveUp` | Player | `$sig_move_up` | Move Up |
| `MoveDown` | Player | `$sig_move_down` | Move Down |

Digital only. The analog movement channel is `signal_axis` over these same names.

#### Camera — `$siggroup_camera`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `LookUp` | Locked | `$sig_look_up` | Look Up |
| `LookDown` | Locked | `$sig_look_down` | Look Down |
| `LookLeft` | Locked | `$sig_look_left` | Look Left |
| `LookRight` | Locked | `$sig_look_right` | Look Right |
| `ToggleMouseCapture` | Reserved † | `$sig_toggle_mouse_capture` | Toggle Mouse Capture |

`Look*` is the continuous channel: read it with `signal_axis` **and**
`signal_pointer_delta` and add both. `ToggleMouseCapture` is a mode switch a
full-screen surface reads to flip exclusive, cursor-locked camera control.

#### Combat / interaction — `$siggroup_combat`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `PrimaryAction` | Player | `$sig_primary_action` | Primary Action |
| `SecondaryAction` | Player | `$sig_secondary_action` | Secondary Action |
| `Jump` | Player | `$sig_jump` | Jump |
| `Sprint` | Player | `$sig_sprint` | Sprint |
| `Crouch` | Player | `$sig_crouch` | Crouch |
| `Interact` | Player | `$sig_interact` | Interact |
| `Reload` | Player | `$sig_reload` | Reload |

#### Souls combat — `$siggroup_souls`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `AttackLight` | Player | `$sig_attack_light` | Light Attack |
| `AttackHeavy` | Player | `$sig_attack_heavy` | Heavy Attack |
| `Defend` | Player | `$sig_defend` | Defend |
| `Special` | Player | `$sig_special` | Special |
| `Dodge` | Player | `$sig_dodge` | Dodge |
| `LockOn` | Player | `$sig_lock_on` | Lock On |
| `UseItem` | Player † | `$sig_use_item` | Use Item |
| `Kick` | Player † | `$sig_kick` | Kick |
| `CounterPerilous` | Player † | `$sig_counter_perilous` | Perilous Counter |
| `Grapple` | Player † | `$sig_grapple` | Grapple |

These are **intents, not abilities**: `Defend` means "the player wants to defend",
and what it resolves to (block, parry, deflect) is the equipped loadout's problem,
downstream in `flicker-mechanics`. That is the test for adding a variant here — if
the answer varies with equipment, it is data, not a new signal.

#### UI — `$siggroup_ui`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `Confirm` | Player | `$sig_confirm` | Confirm |
| `Cancel` | Player | `$sig_cancel` | Cancel |
| `Menu` | Player | `$sig_menu` | Menu |
| `Inventory` | Player † | `$sig_inventory` | Inventory |
| `Map` | Player | `$sig_map` | Map |

#### System — `$siggroup_system`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `Quit` | Player † | `$sig_quit` | Quit |

#### Navigation — `$siggroup_nav`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `ChordBegin` | Locked | `$sig_chord_begin` | Chord Begin |
| `Activate` | Reserved † | `$sig_activate` | Activate |
| `ItemSelect` | Reserved † | `$sig_item_select` | Item Select |
| `NavUp` | Locked | `$sig_nav_up` | Navigate Up |
| `NavDown` | Locked | `$sig_nav_down` | Navigate Down |
| `NavLeft` | Locked | `$sig_nav_left` | Navigate Left |
| `NavRight` | Locked | `$sig_nav_right` | Navigate Right |
| `TabNext` | Locked | `$sig_tab_next` | Next Tab |
| `TabPrev` | Locked | `$sig_tab_prev` | Previous Tab |
| `PageNext` | Locked | `$sig_page_next` | Next Page |
| `PagePrev` | Locked | `$sig_page_prev` | Previous Page |

`Tab*` and `Page*` are two **scales** of the same gesture, not synonyms: `Tab*`
cycles a section within a page, `Page*` cycles the whole page. A surface that
draws both rails needs them distinct. `ChordBegin` announces that the chord
modifier went down.

#### Editor navigation — `$siggroup_editor_nav`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `PanelNext` | Locked | `$sig_panel_next` | Next Panel |
| `PanelPrev` | Locked | `$sig_panel_prev` | Previous Panel |
| `ModeNext` | Reserved † | `$sig_mode_next` | Next View Mode |
| `ModePrev` | Reserved † | `$sig_mode_prev` | Previous View Mode |
| `ZoomIn` | Locked | `$sig_zoom_in` | Zoom In |
| `ZoomOut` | Locked | `$sig_zoom_out` | Zoom Out |

`Nav*` moves within the focused panel; `Panel*` moves *between* panels. They are
separate intents on purpose — a component answers `Nav*` only, so a panel move
never nudges a slider.

#### Editor verbs — `$siggroup_editor_verbs`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `Undo` | Locked † | `$sig_undo` | Undo |
| `Redo` | Locked † | `$sig_redo` | Redo |
| `Cut` | Locked † | `$sig_cut` | Cut |
| `Paste` | Locked † | `$sig_paste` | Paste |
| `Rename` | Locked † | `$sig_rename` | Rename |
| `CreateFolder` | Locked † | `$sig_create_folder` | New Folder |
| `ContextMenu` | Locked † | `$sig_context_menu` | Context Menu |
| `Yes` | Reserved † | `$sig_yes` | Yes |
| `No` | Reserved † | `$sig_no` | No |

Every row is `†`. `editor_chords()` builds a map that binds six of them, but no
shipped profile installs `InputContext::Chord`, so none of them reach a scene
today. Declaring `"on_undo"` compiles, loads, warns nothing, and does nothing.

#### Text terminals — `$siggroup_text`

| Signal | Scope | Token | Display |
|---|---|---|---|
| `SubmitText` | Locked † | `$sig_submit_text` | Submit Text |
| `CancelText` | Locked † | `$sig_cancel_text` | Cancel Text |

Dedicated, not a `Confirm`/`Cancel` overload — a text field commits without the
enclosing dialog also confirming. Both are `†`: `InputContext::TextEntry` ships
with an empty map, so a focused field reads `InputState::typed()` and
`InputState::backspace()` directly rather than these signals.

---

## Naming a signal

There is exactly **one** vocabulary: the serde variant name.
`ActionSignal::name()` returns it, `ActionSignal::from_name()` resolves it back,
and a persisted profile stores that same string. Three surfaces fold it
mechanically — learn the fold once and every name is predictable:

| Surface | Form | Example for `PageNext` |
|---|---|---|
| Rust, profile JSON | the variant name, exact | `PageNext` |
| a scene file's declared intent | `on_` + snake | `"on_page_next": "cat_pm_page_next"` |
| the stringtable / a node's `text` prop | `$sig_` + snake | `$sig_page_next` |

`from_name` is **exact** — no case folding. `"menu"` resolves to `None`,
`"Menu"` resolves. When `flicker-widgets` fails to resolve an `on_<…>` prop it
logs a warning and skips that prop; the rest of the tree is untouched.

Group headers fold the same way: `SignalGroup::token()` gives `$siggroup_nav`,
`$siggroup_editor_verbs`, and so on — one per group, all ten present in the
shipped stringtable.

---

## Public API

Everything below is re-exported from the crate root (`flicker_input_core::X`) as
well as from its module.

### The vocabulary — `signal`

| Item | For | The one thing to know |
|---|---|---|
| `ActionSignal` | The 62 intents | `Copy`; serde-stable variant names; **add only, never rename** — profiles on disk carry the string |
| `ActionSignal::ALL` | `&'static [ActionSignal]` | The count is `ALL.len()`, never a literal |
| `ActionSignal::name` / `from_name` | The one vocabulary, both directions | `from_name` scans `ALL` (no second table); exact match |
| `ActionSignal::label` | Terse HUD/inspector text | English, not localized — use `token()` for anything a player sees |
| `Display for ActionSignal` | Long English label | Same caveat as `label` |
| `ActionSignal::group` / `rebind_scope` | Catalog metadata | Exhaustive matches; a new variant will not compile until classified |
| `ActionSignal::token` | `$sig_<snake>` | Allocates a `String` per call — don't call it per frame |
| `ActionSignal::rebindable()` | Iterator over the `Player` set | The settings page's row source; iterate this, never a hand-list |
| `SignalGroup` (+ `ALL`, `token`) | The ten sections, in declaration order | Membership lives only in `group()` — this enum is just the section list |
| `RebindScope` | `Player` / `Locked` / `Reserved` | See the catalog note above for what each means |

### Physical controls — `device`

Pure symbols; `flicker-input-device` maps platform values onto them. Each carries
its own canonical catalog and token stem, so rebind capture and any derived UI
iterate the enum rather than a parallel list.

| Enum | Count (`::ALL`) | Token stem | Notes |
|---|---|---|---|
| `Key` | 103 | `$key_<snake>` | Letters, digits, F-keys, arrows, modifiers, nav/editing, punctuation, numpad |
| `MouseButton` | 5 | `$mbtn_<snake>` | Left, Right, Middle, Back, Forward |
| `GamepadButton` | 21 | `$pad_<snake>` | Xbox-position naming: `North` is the top face button, `West` the left |
| `GamepadAxis` | 6 | `$axis_<snake>` | Both sticks (X/Y) plus both triggers |

| Item | For |
|---|---|
| `AxisDirection` | Which half of an analog axis a binding watches — `Positive` / `Negative` |
| `DeadzoneShape` | `Circular` (magnitude-based, kills diagonal drift) or `PerAxis` |
| `Display` on all of the above | Terse English keycap text; localized text comes from the token |

The token fold reads the derived `Debug` name, which is pinned equal to the serde
name by a test — a manual `Debug` impl or a `serde(rename)` on these enums breaks
loudly rather than silently renaming a stringtable key.

### Bindings and maps — `binding`

| Item | For | The one thing to know |
|---|---|---|
| `InputBinding` | One physical control: `Key`, `MouseButton`, `GamepadButton`, `GamepadAxis {axis, direction}`, `MouseMotion {axis, direction, gate}` | `Copy` + `Hash`; the map key |
| `MouseAxis` (+ `token`) | `X` / `Y` for `MouseMotion` | |
| `InputBinding::is_down` | THE "is this control active" query | Deadzone/threshold-aware, **player 0 only**; always `false` for `MouseMotion` |
| `InputBinding::mouse_delta_axis` | The gated, directional motion delta | `0.0` for any non-`MouseMotion` binding, or when the gate button is not held |
| `InputBinding::edge_down` | The down-state this edge gives the control | `None` for gamepad bindings — pads are polled, not evented |
| `InputMap` | signal → bindings, plus a reverse index | **One input maps to exactly one signal**: re-binding a control silently removes it from its previous signal |
| `InputMap::empty` / `bind` / `unbind` / `clear_action` | Mutation | |
| `InputMap::bindings_for` / `action_for` / `bound_actions` | Lookup | `bindings_for` returns `&[]` for an unbound signal — never an error |
| `InputMap::action_pressed` | Is any binding for this signal down? | Prefer `ContextualBindings::signal_held` — it is context-aware and deadzone-aware |
| `InputMap::backfill_unbound_from` | Adopt another map's bindings for signals this one leaves **wholly** unbound | The migration primitive: a saved map freezes the defaults of its build, so a default added later would otherwise be unreachable forever. Never overwrites a bound signal |
| `InputMap::wasd_and_mouse` | The keyboard/mouse preset — the `World` map of the `default` profile | |
| `InputMap::xbox_souls` | The controller preset — the `World` map of the `xbox_souls` profile | Binds the *press* of each control; tap-vs-hold and chords are not here |
| `InputMap::flight_path` / `flying` | The two modes of one flight camera, for the same-named contexts | They differ **only** in what the left stick does; everything else is shared |
| `InputMap::esdf_and_mouse` / `gamepad_default` | Alternate presets | Not referenced by any profile or consumer — see [Sharp edges](#sharp-edges) |
| `Default for InputMap` | `wasd_and_mouse` | |
| `Activation` / `BindingDescriptor` | Per-binding press/toggle/hold + tap-vs-hold + modifier flags | **Declared, not implemented** — nothing reads them |

Serialization: `InputMap` round-trips through a flat `Vec<(ActionSignal,
Vec<InputBinding>)>`, because `InputBinding` is a non-unit enum and JSON cannot
use one as an object key. Load rebuilds both indices through `bind`, so the
one-input-one-signal invariant survives a round trip.

### Contexts and the profile — `context`

| Item | For | The one thing to know |
|---|---|---|
| `InputContext` | Which map is live | An **open newtype** over `u16`, not an enum: use `==`, and register your own with `InputContext::register(id)` at `id >= FIRST_CUSTOM` (9) |
| Built-ins | `World`(0) `Menu`(1) `Radial`(2) `TextEntry`(3) `Mounted`(4) `Flying`(5) `Vehicle`(6) `Chord`(7) `FlightPath`(8) | `Radial`, `Mounted`, `Vehicle` and `Chord` have no map in any shipped profile — they fall back to `World` |
| `InputContext::BUILTIN_NAMES` / `from_name` / `name` | The frozen name↔id registry | Profiles persist the **name**; the `u16` is runtime-assigned and not stable. A registered custom context has no name and never persists |
| `ContextualBindings` | The runtime maps + the active-context **stack** | `World` is always the base, so `active()` always answers; a context with no map of its own falls back to `World` |
| `::new` / `with` / `set_map` | Build-time / live replacement | `set_map` preserves the stack — this is how a live rebind reaches a running scene |
| `::active` / `push` / `pop` | Stack control | `pop` refuses to empty below `World` and returns `None` there |
| `::active_map` | The live `InputMap` | |
| `::signal_held` / `signal_axis` / `signal_pointer_delta` | The three continuous queries | See [the frame table](#the-frame-end-to-end) for how to integrate each |
| `::from_profile` / `to_profile` | Profile ↔ runtime | Maps only. `to_profile` writes `EventKind::Press` for every context — see [Sharp edges](#sharp-edges) |
| `InputProfile` | The persisted unit: `schema`, `name`, `contexts`, `controls`, `gamepad` | The thing `settings.json` stores |
| `InputProfile::default_profile` / `xbox_souls` / `by_name` | The two built-ins | `by_name` accepts `"default"` and `"xbox_souls"` only; anything else is `None` |
| `InputProfile::PRESET_NAMES` | `(id, $token)` rows for the settings **controller** selector | Controller configs only — `"default"` is deliberately absent, so this is *not* the list `by_name` accepts |
| `InputProfile::context_map` / `set_context_map` | Read/write one context's map by name | |
| `InputProfile::backfill_from_presets` | Fill the gaps a stale save leaves | **Call once at load, before anything reads the profile.** A profile whose `name` matches no built-in is treated as a custom layout and left exactly as saved |
| `ContextBindings` (+ `simple`) | One context as data: `map`, `default_event`, `signals` | `default_event` and `signals` are persisted and **never applied** — see [Sharp edges](#sharp-edges) |
| `SignalBinding` | A per-(control, context) event-kind override row | Same caveat; nothing constructs one |

The shipped profiles carry five contexts: `World`, `TextEntry` (empty by design —
a focused field owns the keyboard), `Menu`, `FlightPath`, `Flying`.

### The snapshot — `snapshot`

| Item | For | The one thing to know |
|---|---|---|
| `InputState` | The per-frame snapshot: held keys/buttons, mouse position + deltas, gamepads, the edge log, the analog latch | Cheap to clone (the resolver keeps last frame's) |
| `::key_down` / `mouse_button_down` / `gamepad` / `gamepad_connected` / `gamepads` | Held-state queries | |
| `::edges` | This frame's ordered transitions | The reason a press survives a long frame |
| `::pressed` / `released` / `mouse_pressed` | Edge queries derived from the log | True even for a press+release inside one frame — a `key_down` vs `prev` comparison silently drops that case |
| `::typed` / `backspace` | OS-committed text (post-IME) and the backspace edge | Empty/false except on text-entry frames |
| `::mouse_delta` / `mouse_wheel_delta` / `mouse_left_pressed` | Pointer deltas + the click edge | All reset by `clear_frame_edges` |
| `::analog_latch` / `set_analog_latch` | The coherent per-frame copy of the 120 Hz sample | `None` until a device fills it |
| `::input_active` | "Is this signal down" over a bare map, scanning **every** connected pad | The only multi-pad query in the crate; prefer `ContextualBindings::signal_held`, which is context- and deadzone-aware |
| Driver hooks — `set_key`, `set_mouse_button`, `push_edge`, `push_typed`, `flag_backspace`, `gamepad_mut`, `remove_gamepad`, `clear_frame_edges` | For `flicker-input-device` only | `push_edge` **only on an actual state change** — auto-repeat must not read as a fresh press |
| `InputEdge` | `Key { key, down }` / `Mouse { button, down }` | Evented controls only; gamepads are polled and carry no ordering |
| `GamepadState` | One pad: buttons, raw axes, its config | `left_stick()`/`right_stick()` return the **deadzoned, rescaled** vector; `axis_value()` is raw |
| `GamepadConfig` | `left_stick_deadzone` `right_stick_deadzone` `deadzone_shape` `trigger_threshold` | Defaults: 0.15 / 0.15 / `Circular` / 0.5 |
| `apply_deadzone` | The deadzone maths, standalone | Rescales so the deadzone edge maps to 0.0 and the extreme to 1.0 |

### Resolution — `resolve`

| Item | For | The one thing to know |
|---|---|---|
| `Resolver` | Owns previous-frame state + per-binding press times | The single home of edge state — a scene must not keep `*_prev` bools |
| `::resolve_frame(&bindings, &cfg, &curr, now, &mut out)` | `(prev, curr) × active context → Fired`s | Writes into a **caller-owned** buffer; reuse a cleared `Vec`, no per-frame alloc |
| `::held_ticks` | Ticks a control has been held | `None` when not pressed |
| `::reset` | Drop history (context reset, focus loss) | After a reset a still-held control reads as a fresh press |
| `TickTime` = `u64` | A monotonic tick supplied by the caller | Deliberately **not** `Instant` — resolution is deterministic and replayable |
| `EventKind` | `Press` `Release` `Hold` `Chord` | Only `Press` and `Release` are ever emitted |
| `Fired` | `{ signal, kind, control }` | `Copy`; the router wraps it into an `InputEvent` |

Evented controls (keyboard, mouse) resolve from the ordered edge log; polled
controls (gamepad) resolve from level comparison. Both paths run in the same call.

### The analog channel — `analog`

| Item | For | The one thing to know |
|---|---|---|
| `AnalogFrame` | One 120 Hz sample: both sticks, both triggers, `seq`, `captured` | `captured` is wall-clock and is used **only** for staleness — never sim-credited |
| `AnalogFrame::neutral` | Seed / disconnect fallback | |
| `AnalogCache` | Current + previous, single-threaded interior mutability | Read through `&self`; every read is an owned copy |
| `::sample` / `::previous` / `::is_stale` | Consumer surface | |
| `::push` | Sampler-only (`flicker-input-device`) | Game code never pushes |
| `AbstractControls` | Per-device look/move tuning: mouse + stick sensitivity, four invert flags, `move_speed`, `stick_deadzone` | Rides inside `InputProfile` |
| `::look_delta_mouse` / `look_delta_stick` | Raw delta → `(yaw, pitch)` radians | Positive pitch looks up; screen Y grows down, so the sign is already handled. The stick result is a **rate** — multiply by `dt` |

### The chord layer — `chord`

Hold a modifier and the controls under it mean editor verbs. It is implemented as
a **context**, not an event kind: while the modifier is down, `InputContext::Chord`
is on top of the stack, so its map is the active one and a member resolves to its
verb and to nothing else — suppression is structural, with no suppression logic.

| Item | For | The one thing to know |
|---|---|---|
| `ChordLayer` (+ `new`, `is_open`, `held_by`) | Keeps the layer in step with the modifier | |
| `ChordLayer::update(&mut bindings, &fired, &curr, &cfg) -> bool` | Call once per frame, right after `resolve_frame` | Closes on the modifier's **physical state**, not on a release edge — the chord map does not bind `ChordBegin`, so waiting for an edge would strand the layer open forever |
| `editor_chords()` | The default verb map | No chord member is a face button: holding the modifier commits that thumb |

No shipped profile installs `InputContext::Chord`, and nothing in the app calls
`ChordLayer::update` — see [Sharp edges](#sharp-edges).

### Rebind capture — `rebind`

| Item | For | The one thing to know |
|---|---|---|
| `RebindCapture` | Drives "press a key to bind" from a settings screen | |
| `::start(action, for_gamepad, &input)` | Arm | **Pass the live snapshot.** Anything held at this moment — above all the click that armed the field — is seeded as prior state, so only an actuation that *begins* after arming can capture |
| `::poll(&input, &mut map)` | Returns `Some((action, binding))` on capture | Binds **slot 0**, unbinding the control from any other signal first; capture then ends |
| `::unbind_current(&mut map)` | Drop slot 0 and end capture | The caller must detect the unbind key itself — it is otherwise a bindable key and `poll` would capture it |
| `::cancel` / `is_active` / `current_action` / `is_gamepad` | State | |
| `capture_input(...)` | The bare edge detector `poll` uses | Iterates the device `ALL` catalogs, so a control added to an enum is capturable immediately |

---

## Interactions

**Signals it answers:** none. This crate *defines* the vocabulary; it never
consumes it. Every "Signals it answers" section elsewhere in the repo should link
back to [the catalog above](#the-signal-catalog).

**What it hands other crates:**

| Handed | To | Shape |
|---|---|---|
| `Fired { signal, kind, control }` | `flicker-input-router` → the handler chain → scenes | Per-frame `Vec`, caller-owned |
| `InputState` | filled by `flicker-input-device`, read by everyone | One shared snapshot per frame |
| `AnalogCache` | filled by `flicker-input-device`, latched by `flicker-app` | 120 Hz, volatile |
| `ActionSignal` names | `flicker-widgets` (`on_<signal>` props), `flicker-shell` (settings rows) | Strings |
| `$sig_*` / `$siggroup_*` / device tokens | the widgets stringtable | `$`-sigil tokens resolved at draw |
| `InputProfile` | `flicker-shell` | Serialized into the per-user `settings.json` |

**The three channels a consumer reads a signal on** — all downstream of this
crate, listed so the catalog is usable:

1. **A declared intent in a scene file** — `"on_<snake_signal>": "<result name>"`
   on the scene's root node. The result name is then mirrored into the Model as
   `sig_<result>` for exactly one frame, so a Lua pair script can observe it as
   `Model.sig_<result>`. Owned by `flicker-widgets` (`intents.rs`) — see
   [`Alpha/content/sensorium/README.md`](../../../content/sensorium/README.md).
2. **A Rust handler in the router chain**, matching on `Fired.signal` /
   `Fired.kind`.
3. **A direct continuous query** — `signal_held` / `signal_axis` /
   `signal_pointer_delta` — for cameras and movement.

**Model keys:** this crate publishes none. The transient `sig_<result>` mirror is
a `flicker-widgets` key namespace and shares nothing but a prefix spelling with
this crate's `$sig_*` **stringtable** keys.

**Threads:** none. Everything here is single-threaded and allocation-conscious;
`AnalogCache` uses `Cell`, not a lock.

---

## Gates

68 tests, all green (`cargo test -p flicker-input-core`). The contract-bearing
ones, by name:

| Test | Breaks when |
|---|---|
| `signal::all_covers_every_variant_uniquely` | A variant is dropped from or duplicated in `ALL` |
| `signal::name_round_trips_all_and_matches_serde` | `name()` forks away from the persisted serde string — profiles on disk would stop resolving |
| `signal::rebindable_set_is_the_ruled_29` | The `Player` set changes size, or the Player/Locked/Reserved split stops covering `ALL` |
| `signal::every_signal_is_grouped_and_every_group_has_members` | A group gains no members (an empty section on a derived page) or a signal groups outside `SignalGroup::ALL` |
| `signal::tokens_are_the_snake_fold_of_the_one_vocabulary` | A token stops being `"$sig_" + snake(name())`, or two collide |
| `device::all_catalogs_are_complete_and_unique` | A `::ALL` count drifts (103 / 5 / 21 / 6) or gains a duplicate |
| `device::tokens_ride_the_serde_names` | A device enum's `Debug` name stops equalling its serde name — the token fold reads `Debug` |
| `device::face_button_labels_match_physical_position` | `North`/`West` labels get swapped again |
| `binding::input_map_rebind_enforces_one_action_per_input` | One control ends up bound to two signals |
| `binding::input_map_round_trips_through_json` | Persistence breaks (this is why the on-disk form is a flat pair list) |
| `binding::the_world_preset_covers_the_whole_bench_focus_tier` | A focus-tier signal loses its `World` binding — the dead-hardware gate |
| `binding::flight_camera_presets_bind_the_two_mode_contract` | The two flight modes stop differing only in the left stick |
| `binding::xbox_souls_binds_ruled_layout` | The controller layout is edited carelessly |
| `context::backfill_adopts_new_defaults_and_keeps_user_binds` | A stale save stops adopting new defaults, or starts overwriting a user's own binds |
| `context::builtin_context_names_round_trip` | A frozen context name changes — saved profiles would silently lose that context |
| `context::contextual_bindings_serde_skips_stack_and_keys_by_name` | The runtime stack starts persisting, or maps key by `u16` |
| `context::mouse_look_is_a_bound_signal_gated_on_rmb` | Mouse-look regresses to a raw poll instead of a bound signal |
| `resolve::press_and_release_inside_one_frame_both_fire` | A tap inside a long frame is swallowed |
| `resolve::every_tap_in_a_stalled_frame_arrives_in_order` | Only the last tap of a stalled frame survives |
| `resolve::edges_do_not_leak_into_the_next_frame` | This frame's edges re-fire next frame |
| `resolve::gamepad_controls_still_resolve_from_level_state` | Polled controls start needing an edge log they never have |
| `chord::releasing_the_modifier_closes_the_layer` | The chord layer strands open (the release has no edge to ride) |
| `chord::a_swallowed_release_cannot_strand_the_layer` | Alt-tab mid-hold wedges a bench in chord mode |
| `chord::no_chord_member_is_a_face_button` | A verb moves onto a face button, unreachable while the modifier is held |
| `rebind::the_arming_click_is_never_captured_as_the_binding` | Arming a rebind field binds the click that armed it |
| `rebind::unbind_current_drops_slot_zero_and_ends_capture` | Unbind stops ending capture, or removes the wrong slot |

Two more gates live in `flicker-shell` because they cross into content:
`every_catalog_token_resolves_in_the_shipped_table` (every token this crate mints
exists in `stringtable.json`) and `derived_keyboard_page_matches_the_rebindable_set`
(the settings page's rows *are* `rebindable()`, not a parallel list).

---

## Sharp edges

- **Binding the same control twice silently unbinds the first signal.**
  `InputMap::bind` enforces one-input-one-signal by removing the earlier owner —
  no error, no log. Three presets in this crate trip over it today
  (`binding.rs:328`, `:568`, `:800` bind `Quit` and then immediately bind the same
  control to `Menu`), which is why `Quit` has no binding in either shipped profile
  despite `wasd_and_mouse`'s own doc comment advertising one. When authoring a
  preset, order matters and the loser is silent.
- **A `†` signal fires for nobody, and nothing says so.** Declaring
  `"on_undo"` or `"on_mode_next"` in a scene file passes the name-resolution gate
  (the signal exists) and then does nothing, because no profile binds it. Four
  shipped scenes currently declare thirteen such intents. Check the `†` column
  before wiring a scene to a signal.
- **`ContextBindings::default_event` and `SignalBinding` are inert.** They
  persist, round-trip, and are never consulted by `Resolver::resolve_frame`. The
  `Menu` context is built with `EventKind::Release`, but menus fire on `Press`
  like everything else — and `ContextualBindings::to_profile` rewrites the field
  to `Press` on the next save. Do not rely on the reshape table.
- **`EventKind::Hold` and `EventKind::Chord` are never emitted.** `Resolver` emits
  `Press` and `Release` only. `held_ticks` is the raw material for `Hold`, but no
  one converts it. Likewise `Activation` / `BindingDescriptor` are declared and
  read by nothing.
- **The chord layer is complete and unwired.** `ChordLayer`, `editor_chords()` and
  `InputContext::Chord` all work and are tested, but no profile installs the Chord
  map and no crate calls `ChordLayer::update`. `ChordBegin` *is* bound, so the
  modifier fires a signal that opens nothing.
- **`is_down` reads player 0 only.** `InputBinding::is_down` — and therefore
  `signal_held`, `signal_axis` and the whole resolver — consults gamepad slot 0.
  `InputState::input_active` is the only query that scans every connected pad;
  nothing on the shipped path calls it yet.
- **`signal_axis` is already deadzoned.** It returns the snapshot's own rescaled
  magnitude. Applying your own deadzone on top carves a second, larger dead centre
  out of the stick.
- **`MouseMotion` never reports as "down".** It resolves only through
  `signal_pointer_delta`; `is_down` and `input_active` return `false` for it, and
  it produces no `Fired` events. A gated `MouseMotion` binding is also invisible to
  `binding_label`-style display, which has no single token for a compound binding.
- **Call `backfill_from_presets` exactly once, at load.** Without it, every default
  binding added after a player's `settings.json` was written is unreachable for
  that player, forever — the profile froze the defaults of its own build.
- **`PRESET_NAMES` is not the list `by_name` accepts.** It is the settings
  *controller* selector's roster and holds only `xbox_souls`; `by_name` also
  accepts `"default"`, which never appears in it.
- **`token()` allocates.** `ActionSignal::token` and the device `token()`s return
  `String`. Resolve them once at build time, not per frame.
- **Tools not yet on a shipped path.** This crate is a toolbox; several `pub` items
  are built toward the input spec and simply have no caller on the shipped path
  *yet*: the chord layer (`ChordLayer`, `editor_chords` — awaiting a profile that
  installs `InputContext::Chord` and drives `update`), `Activation` /
  `BindingDescriptor` (awaiting a resolver that reads them), `Resolver::held_ticks`
  (the raw material for `Hold`), the extension seam `InputContext::register` /
  `FIRST_CUSTOM`, the preset building blocks `InputMap::esdf_and_mouse` /
  `gamepad_default` (not on `PRESET_NAMES`' roster), `ContextualBindings::to_profile`
  and `AnalogCache::is_stale`. All compile and are tested; treat them as shelf
  stock, not dead weight. The exception is the superseded trio
  `InputState::input_active` / `InputMap::action_pressed` /
  `ContextualBindings::action_pressed` — prefer `ContextualBindings::signal_held`
  (context- and deadzone-aware), noting `input_active` is the one all-pads scan.
