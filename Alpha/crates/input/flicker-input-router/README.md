# flicker-input-router

The **one event bus** for input. It takes the resolved signals a frame produced
and routes them through an ordered chain of handlers — each layer either *consumes*
a signal (stops it) or *passes* it down — so the arbitration every scene used to
hand-roll (the `hud_hit` / `chat_hit` / `active()==World` gate ladder) is now a
single typed dispatcher. It also carries the window-free focus and directional-nav
helpers a UI layer needs. It is pure and depends only on
[`flicker-input-core`](../flicker-input-core/README.md); it reads no devices, owns
no window, spawns no threads.

**This crate routes *signals*, not *intents*.** A signal already *is* the intent —
a click is a `Confirm` at whatever the pointer hit, a `Menu` press is the intent to
open the menu (rule 37722F91). There is no second "intent router" and no
signal→intent mapping table to look for: a handler simply *subscribes* to the
signals it cares about, and the bus routes each signal to the first subscribed
handler that consumes it. If you are hunting for where a key becomes an action,
stop — that resolution already happened upstream in `flicker-input-core`, and the
router never sees a key.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

---

## Contents

1. [Vocabulary](#vocabulary)
2. [Where it sits](#where-it-sits)
3. [The frame, end to end](#the-frame-end-to-end)
4. [Public API](#public-api)
5. [Interactions](#interactions)
6. [Gates](#gates)
7. [Sharp edges](#sharp-edges)

---

## Vocabulary

Terms this README leans on (flicker-specific; not general programming):

- **signal** — the semantic *what* of an input (`Confirm`, `Menu`, `NavDown`,
  `PanelNext`, `SubmitText`…). The one vocabulary, owned and named by
  [`flicker-input-core`](../flicker-input-core/README.md#the-signal-catalog). The
  router is signal-*agnostic*: it carries whatever signals the core defines and
  defines none of its own.
- **context** (`InputContext`) — the mode/vehicle a frame's input resolves under
  (`World` base, `Menu`, `TextEntry`, `FlightPath`, `Radial`…). Owned by the core
  registry; the router only names it in requests and events.
- **handler / layer** — one `InputHandler` in the chain (system, scene-root, modal,
  UI-panel, gameplay-base). "Layer" is its position in the chain.
- **chain** — the per-frame ordered `[&mut dyn InputHandler]`, highest input
  priority first, that a scene hands to `Router::dispatch`.
- **walker** — `flicker-widgets`' UI-panel handler: it hit-tests the scene's UI
  tree, owns the focused element, and is the `InputHandler` most scenes drop in for
  the UI tier. It is the router's biggest single consumer, not part of this crate.
- **focus** — the id of the currently highlighted UI node, shared by pointer and
  pad. The router *decides* focus changes; the focus store itself lives in the
  walker (`flicker-widgets`).
- **the pump / runner** — the central signal loop in `flicker-app` (`runner.rs`)
  that fills the snapshot, resolves it, and builds the `InputEvent`s the router
  routes. "The caller" below is usually the pump plus the active scene.

---

## Where it sits

**Builds on:** [`flicker-input-core`](../flicker-input-core/README.md) only —
`Fired` (a resolved signal edge), `ActionSignal`, `EventKind`, `InputContext`,
`InputState` (the raw held-state snapshot), and `ContextualBindings` (the
active-context stack). Nothing else of ours; no `winit`, no `glam`, no threads.
`Focusable.rect` is a plain `[f32; 4]` precisely so no vector dep is needed.

**Used by:**

| Crate | Takes |
|---|---|
| `flicker-widgets` | Implements `InputHandler` for the walker; consumes `Focusable` / `nav` / `nav_geometric` for directional nav, and `apply_context_requests` / `FocusChange` / `RouterRequest` to reconcile focus + context |
| `flicker-app` (the pump) | `InputEvent` (built via `from_fired`), `RouteCtx`, and `apply_context_requests` to drain a frame's context requests into the shared `ContextualBindings` (`runner.rs`) |
| `flicker-scene` | `SceneInput` (its `FrameInput` alias) carries the frame's `events: &[InputEvent]` + a `RouteCtx` for scenes to route |
| `flicker-shell` | `Router` + `InputHandler` (splash skip + shell scenes), `apply_context_requests` |
| `flicker-globe` | Implements `InputHandler` for `GlobeWorld` (the world/camera tier) |
| every `scenes/*` crate | `Router::dispatch` + `InputHandler` + `Flow` + `RouteCtx`; some read `DispatchReport` (`consumed_by` / `passed`) to turn a consumed signal into a scene `Transition` |
| `flicker` (umbrella) | Re-exported as `flicker::input_router` |

**Content files it reaches into:** none. Pure Rust; reads no scene files, no theme,
no stringtable. Signal *names* it routes are authored elsewhere (a scene's
`on_<signal>` prop) and resolved by `flicker-widgets`, not here.

---

## The frame, end to end

The router is one station in a pipeline the **pump** drives; it does not resolve
input or read devices. Per frame:

1. **device** fills the raw snapshot (`InputState`) — pointer, held buttons, analog.
2. **the pump** derives the active `InputContext` (see the sharp edge below on
   *where* that comes from), syncs the `ContextualBindings` stack to it, and asks
   **core** to resolve `(prev, curr) × context → Vec<Fired>`.
3. **the pump** wraps each `Fired` into an [`InputEvent`](#public-api) via
   `InputEvent::from_fired` — signal + kind + the active context + a borrow of the
   raw snapshot. (The physical control is *dropped here* — nothing past the bus can
   ask "which key?".)
4. **the scene** builds its handler `chain` and calls
   `Router::dispatch(events, &mut chain, &mut route)`.
5. Handlers push router-owned intents (`RouterRequest`) into the `RouteCtx` and the
   dispatch returns a [`DispatchReport`](#public-api).
6. **the caller** reconciles with `apply_context_requests` (context push/pop applied
   to the bindings; the `FocusChange` returned for the caller to write through the
   walker) and reads the report (e.g. `consumed_by(ROOT, Menu)` → a scene
   `Transition`).

`Router::dispatch` runs **two phases per event**:

```text
chain: [0] system/global   capture-only: quit, debug-console, screenshot
       [1] scene/mode root  the active scene
       [2] context/modal    pushed modals + exclusive contexts (PauseMenu, TextEntry)
       [3] UI panel tree    the walker (hit-test + focused element)
       [4] gameplay at base camera / world-pick / movement  — runs only on all-pass
```

- **capture** — top-down (`chain[0]`→`chain[N]`); the first `Flow::Consumed` claims
  the event before any lower handler's `handle` runs. This is where an exclusive
  keyboard owner grabs text + Enter/Esc so `Menu` never reaches the scene root.
- **target + bubble** — top-down again; the first `Flow::Consumed` stops
  propagation, a `Flow::Pass` falls through, and the gameplay-base handler (last)
  runs only when everything above it passed.

A handler is **skipped entirely for any signal it does not subscribe to** — in both
phases — so it can never even be *asked* about a signal it does not own. That is the
signal-subscription model: a context watches the stream and takes only what is
relevant; it never eats all input.

---

## Public API

Everything is re-exported flat from the crate root. Grouped by concern:

### The bus

| Item | What it is for | The one thing to know |
|---|---|---|
| `Router` | The dispatcher. A unit struct — it holds no state | Stateless; all per-frame state lives in the handlers, the `RouteCtx`, and the returned report |
| `Router::dispatch(events, chain, rc) -> DispatchReport` | Route a frame's events through the chain, two phases each | `chain` is `&mut [&mut dyn InputHandler]`, highest priority first; you own the ordering |
| `InputEvent<'a>` | One routable event: `signal`, `kind`, `context`, and a borrow of the raw `InputState` (`raw`) | `raw` is the pointer/analog source a UI handler hit-tests against; the event is `Copy` |
| `InputEvent::new(signal, kind, context, raw)` | Assemble an event from parts (tests, hand-built frames) | — |
| `InputEvent::from_fired(fired, context, raw)` | Wrap a core `Fired` with the active context — the pump's half of the seam | Copies `signal` + `kind`; the physical `control` stays in the resolver's domain and is **not** carried onto the bus |
| `Flow` | A handler's verdict: `Consumed` (stop) / `Pass` (let the next act) | `Consumed` == today's `hud_hit == true` |

### The handler contract

`InputHandler` is the trait each layer implements. Only `handle` is required.

| Method | Default | What it does |
|---|---|---|
| `subscribes(&self, signal) -> bool` | `true` (everything) | The signals this layer is willing to consume. The dispatcher offers `capture`/`handle` **only** for subscribed signals; an unsubscribed signal passes straight through this layer. Override to a narrow set to stop eating input you do not own (67DEE93A) |
| `capture(&mut self, ev, rc) -> Flow` | `Flow::Pass` | Top-down first-refusal pass. Return `Consumed` to claim the event before any lower `handle` runs (system/global + exclusive keyboard owners override this) |
| `handle(&mut self, ev, rc) -> Flow` | *required* | Target + bubble pass. `Consumed` stops propagation; `Pass` falls through |
| `declares_context(&self) -> Option<InputContext>` | `None` | *Intended* to tell the caller which context this layer owns. **Not wired to any caller today — see Sharp edges #1.** |

### Router-owned intents & reconciliation

The router carries **only** context + focus changes — never a `flicker-scene`
dependency. Structural scene transitions stay with the scene (it reads the report).

| Item | What it is for | The one thing to know |
|---|---|---|
| `RouterRequest` | An intent a handler emits during dispatch: `PushContext(ctx)` / `PopContext` / `SetFocus(id)` / `ClearFocus` | Context requests apply in emission order; focus requests are last-wins |
| `RouteCtx` | The scratch queue handlers push requests into for one dispatch | `new()` + `push_context` / `pop_context` / `set_focus` / `clear_focus`; drain it after dispatch, then clear it |
| `apply_context_requests(bindings, requests) -> Option<FocusChange>` | Reconcile a frame's requests after dispatch | Applies `PushContext`/`PopContext` to the `ContextualBindings`; **returns** the focus decision rather than applying it (the focus store lives in the walker, not here) |
| `FocusChange` | The resolved focus decision: `Set(id)` / `Clear` | You must write it through the walker adapter yourself, or focus never moves |

### The frame report

| Item | What it is for | The one thing to know |
|---|---|---|
| `DispatchReport` | What each signal's routing did this frame | Fields are private; query it, don't construct it (except `default()` for an empty one) |
| `DispatchReport::consumed_by(layer, signal) -> bool` | Did that layer consume that signal (either phase)? | `layer` is the **positional index** into your chain — keep a `const ROOT: usize = 0` etc. (see Sharp edges #2) |
| `DispatchReport::passed(signal) -> bool` | Did the signal fall through the whole chain unconsumed? | Both queries are existential over the frame's events, not per-event |

### Directional nav (window-free)

Pure functions over a flat `Focusable` list — no window, no walker — so they
unit-test in isolation. The output id is written into the walker's focus store, so
pointer + d-pad share one focus identity.

| Item | What it is for | The one thing to know |
|---|---|---|
| `Focusable` | A flattened focusable node: `id`, `group` (the `tab_group`), `ordinal` (`nav_ordinal`), `rect` (`[x,y,w,h]`) | `group`/`ordinal` are Lua-authored props; `rect` is patched in per frame by the walker |
| `NavDir` | `Up` / `Down` / `Left` / `Right`. `Down`/`Right` step forward by ordinal; `Up`/`Left` step back | — |
| `nav(items, current, dir) -> Option<String>` | Ordinal ring **within the current item's group** (wrapping) | With no `current` (or an unknown id) it enters at the group extreme by `(ordinal, group, id)`; empty list → `None` |
| `nav_geometric(stops, current, dir) -> Option<String>` | Geometric nearest-in-direction over resolved `rect`s (banded tier, then unbanded fallback) | Deterministic, total, **no wrap at the edge** (returns `None` — wrapping is the ring's job). Degenerate/unknown `current` → `None`, so the caller falls back to `nav` |

---

## Interactions

- **Signals it captures** — *all of them, and none by itself.* The router is a
  signal-agnostic bus; it carries every `ActionSignal` the core defines
  (`Confirm`, `Cancel`, `Menu`, `NavUp`/`NavDown`/`NavLeft`/`NavRight`,
  `PanelNext`/`PanelPrev`, `TabNext`/`TabPrev`, `Quit`, `SubmitText`,
  `PrimaryAction`, …; the full catalog is
  [`flicker-input-core`](../flicker-input-core/README.md#the-signal-catalog)). Which
  signals are actually consumed is a *per-handler* decision via `subscribes()`; the
  router names no signal in its own code. It never sees a key, button, or axis —
  only resolved signals (rule 37722F91 / DFE3E44E).
- **Results / intents it fires** — `RouterRequest` (`PushContext` / `PopContext` /
  `SetFocus` / `ClearFocus`) into the `RouteCtx`, reconciled by
  `apply_context_requests`. That is the router's *entire* output vocabulary.
  Anything scene-shaped — a `Transition`, an `exits` lookup, a fired result name —
  is **not** the router's: the scene reads the `DispatchReport` after dispatch and
  decides that itself.
- **Model keys** — none. The router touches no Model (the per-frame key→value table
  the engine hands to Lua); it neither publishes nor binds.
- **What it hands other crates** — the `DispatchReport` (queried with `consumed_by`
  / `passed`) and the `Option<FocusChange>` returned from `apply_context_requests`.
  Both are plain values; the caller acts on them.
- **Threads / workers / async** — none. `Router::dispatch` is a synchronous,
  single-pass function over the frame's events; there is no runtime, no queue that
  outlives the call, and no background work.

---

## Gates

The contract is pinned by 20 tests (`cargo test -p flicker-input-router`, 20/0).

**The bus (`router.rs`):**

| Test | What it locks |
|---|---|
| `capture_stops_on_first_consumed` | The capture phase halts at the first `Consumed`; the lower layer's `handle` never runs |
| `a_layer_only_consumes_signals_it_subscribes_to` | The subscription model (67DEE93A): a Confirm-only scene layer never eats a Nav, a nav-only focus layer never eats a Confirm; the unsubscribed layer is skipped before it is even asked |
| `handle_runs_high_to_low_and_pass_falls_through` | All captures run first (top-down), then `handle` top-down; a `Pass` falls through, a `Consumed` stops |
| `base_handler_runs_only_on_all_pass` | The gameplay-base (last layer) runs only when everything above passed; a higher `Consumed` skips it; an all-pass reports `passed` |
| `requests_push_pop_in_emission_order_and_focus_last_wins` | Context push/pop apply in emission order; the later `SetFocus` wins |
| `set_focus_wins_when_emitted_last` / `clear_focus_wins_when_emitted_last` | Focus is strictly last-wins in either direction |
| `no_focus_requests_returns_none` | A frame with only context requests returns no `FocusChange` |
| `dispatch_collects_requests_then_apply_reconciles_them` | The full seam: a handler emits during dispatch, the caller drains it into the bindings afterward |

**Directional nav (`nav.rs`):**

| Test | What it locks |
|---|---|
| `nav_moves_by_ordinal_with_wrap` | Ordinal stepping + wrap; `Right`==`Down`, `Left`==`Up` |
| `nav_stays_within_current_group` | Nav never leaves the active `tab_group` |
| `nav_no_current_enters_at_extreme` / `nav_unknown_current_is_treated_as_no_current` | Entry with no/unknown focus lands at the deterministic extreme |
| `nav_empty_is_none` | Empty list → `None` |
| `geometric_moves_to_the_banded_neighbour` | The stop that sits that way on screen wins |
| `geometric_does_not_wrap_at_the_edge` | A directional press at the edge does nothing (no wrap) |
| `geometric_banded_beats_a_closer_unbanded_centre` | An overlapping (banded) neighbour beats a geometrically-closer non-overlapping one |
| `geometric_ties_break_by_ordinal_then_id` | Equal-distance peers fall to the authored `(ordinal, id)` order |
| `geometric_falls_back_to_unbanded_when_nothing_bands` | Diagonal-only reachability still resolves |
| `geometric_degenerate_or_unknown_current_is_none` | No anchor rect / unknown id → `None` (caller falls back to the ordinal ring) |

---

## Sharp edges

1. **`declares_context()` is not wired to anything.** The trait method's own doc
   says it is "read by the caller to derive the active context," but **no production
   caller reads it** — `Router::dispatch` never consults it, and the pump derives
   the active context from `Scene::input_context()` (in `flicker-scene`) instead.
   The four scene crates that override it (`clicktrainer`, `jiggle`, `solarbirth`,
   `pocclusters`) only read it back in their *own tests*. If you override it
   expecting the context to switch, nothing happens and nothing errors —
   `solarbirth`'s root declares `FlightPath` while its `Scene::input_context()` is
   the real authority. **Today the authoritative context seam is
   `Scene::input_context()`, not this method.** (Tracked: MCP incident `2C7E6042`;
   it is unfinished wiring, not a dead method — see Findings.)
2. **Layer identity is a bare positional `usize`.** `consumed_by(layer, signal)`
   takes the index of the layer in the chain you built. There is no named-layer API,
   so an off-by-one just returns `false` — silently, no panic. Every scene manages
   this with `const ROOT: usize = 0;` (etc.) beside its chain; follow that
   convention or a report query lies to you.
3. **The subscription default is the greedy one.** `subscribes()` defaults to
   `true` (consume everything). That is deliberate backward-compat — a handler that
   gates imperatively inside `handle` is unchanged — but a nav-only focus layer that
   *forgets* to override it will happily eat `Confirm`. If a layer has a narrow job,
   declare its set.
4. **`apply_context_requests` returns the focus change, it does not apply it.** The
   focus store lives in `flicker-widgets`, so if you drop the returned
   `Option<FocusChange>`, context will move but focus never will — silently.
5. **`from_fired` throws away the physical control.** Only `signal` + `kind` reach
   the bus. This is the whole point (nothing is wired to a key), but it means a
   handler cannot ask "was this the keyboard or the pad?" — by design.
6. **`DispatchReport` queries are per-frame, not per-event.** `consumed_by` /
   `passed` are true if *any* event carrying that signal did so this frame. If you
   need per-event outcomes, inspect during dispatch.
