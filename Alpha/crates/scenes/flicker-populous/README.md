# flicker-populous

The **Populous Bench** — the Game Master screen where a *world* is authored, built up
one planetary layer at a time from nothing but a hex tiling of a sphere. It is the final
base-planet iteration: rather than simulating a planet's history and waiting, you dial
each layer in directly and watch it change.

It is also the project's **reference scene** — composition-is-data, the `paged_menu`
control, `arrange()` gating slices by selection. If you are building a new bench, copy
this one's shape.

> Design of record — why it is shaped this way, the derivation models behind each layer,
> the decisions and the history — lives in the project's MCP memory, not here. This file
> is a guideline for using and extending the bench. Start from spec
> `DF5C03D0-EE03-4BCD-8778-CB2B986AB2EA` (the hex-stack ledger) and decisions
> `CC104DA7…` (seams + hex page), `37D3554A…` (crust layer), `E3893654…` (volcanism).

## What it shows

Two pages. The World page's tabs after the first are **layer tabs** — one planetary
layer each, painted over the same globe:

| Page · tab | The centre pane shows | The left pane offers |
|---|---|---|
| **World · Map** | the planet: grey tiles, near-black under-shell, the seams reading as outlines | the **size** dial — and the right pane shows the world's numbers |
| **World · Seams** *(molten)* | the same planet painted with the **molten heat field**: cool convection-cell interiors, hot seams between them, white-hot **hot spots** (mantle plumes) wherever they stand | the **cells** and **spots** dials, and a **randomize** button |
| **World · Crust** *(deep crust)* | the same planet painted **bedrock brown** with **red lava** at the vents, which come out as lumpy **volcanic fields** — dense chains with clear bedrock between them | (placeholder) |
| **Hex · Stack** | the centre cell alone, as a stack of true CSG columns: a thin **molten** cell below, a thick **bedrock** cell above — red when that cell is a vent | (placeholder) |

Both layer tabs carry a gold **reticle** on the cell the camera faces. That cell is the
one the Hex page inspects — so you aim on a layer tab, then page across to Hex.

The world is **three objects, one each**, shared by every page, in a chain: the tiling
(`HexMap`), the molten heat field derived over it (`SeamField`), and the crust's vents
derived from *that* (`CrustField`). Each is re-derived, never edited, and nothing
downstream has a control of its own — the lava moves only because the heat under it
moved. How each derivation works is in MCP, not here.

## Using it

Turn the planet with the look/zoom **signals** while the centre pane is *entered*
(Confirm locks into a pane, Cancel backs out) — merely highlighting it is not enough.
`Menu` opens the pause overlay. The page and tab rails step from the shoulder signals, a
click, or a pad Confirm on a pill; all three converge on the same result.

To inspect a particular column: go to a layer tab, turn the planet until the reticle
rings the cell you want, then page to **Hex**. Aiming at a lava dot on the Crust tab is
what makes the stack's bedrock cell render red.

Every control writes the **one** shared world, whichever tab it is shown on. A size
change on the Map tab is the size every other tab renders; a re-roll on the Seams tab
moves the vents on the Crust tab and the colours in the stack.

## Where it sits

- **Builds on:** [`flicker-worldgrid`](../../world/flicker-worldgrid/README.md) (the
  icosphere topology) · [`flicker-globe`](../../frontend/flicker-globe/README.md) (the
  shared hex-globe component — both centre-pane views are one) · `flicker` core (the
  scene/walker/Model/script host) · [`flicker-shell`](../../frontend/flicker-shell/README.md)
  (theme, pause overlay) ·
  [`flicker-input-core`](../../input/flicker-input-core/README.md) /
  [`flicker-input-router`](../../input/flicker-input-router/README.md) (the signal
  catalog and dispatch chain) · `fastrand` (the one random source — every roll is a kept
  seed, so a world is reproducible).
- **Used by:** `prism-alpha` only — registered as behaviour `"populous"` in the Game
  Master realm (`Alpha/prism-alpha/src/main.rs:38`).
- **Reads from the content tree:**
  | Path | If missing |
  |---|---|
  | [`sensorium/scenes/populous.scene.json`](../../../content/sensorium/scenes/populous.scene.json) | `new` panics — fail loud |
  | [`sensorium/scripts/populous.lua`](../../../content/sensorium/scripts/populous.lua) | build fails (`include_str!`) |
  | `sensorium/resources/ui_theme.json` / `ui_style.json` | falls back to shared styles |
  | `data/stringtable.json` | a `$token` that resolves to nothing draws as nothing |

## The authored surface

This is the part a human actually reaches from outside — the names the scene file, the
pair script and the stringtable must agree on. The Rust types (`HexMap`, `SeamField`,
`CrustField`, `PopulousBench`) document themselves in source; only the bindings below
are contract.

**Model keys the scene publishes** (every one has a node that binds it):

| Key | Kind | Read by |
|---|---|---|
| `page` / `tab` | number | the page rail, and **both** tab rails (`bind`) |
| `paged_tabs_shown` | bool | the `paged_menu`, collapsing the tab band |
| `pop_freq` | number, two-way | the **size** dial (48–120) |
| `pop_cells` | number, two-way | the **cells** dial (2–12) |
| `pop_spots` | number, two-way | the **spots** dial (0–12) |
| `pop_hexes` / `pop_diameter` / `pop_tile` | pre-formatted string | the three readout rows (`text_bind`) |

Formatting happens in Rust and rides a bind — a node carries a `$token` caption or a
bind name, never a composed number.

**Gate keys `populous.lua`'s `arrange()` publishes** (bound by `visible_bind`):

| Key | Gates |
|---|---|
| `shown_page0` / `shown_page1` | that page's tab rail **and** its centre-pane viewport |
| `shown_p0_t0` / `shown_p0_t1` / `shown_p0_t2` / `shown_p1_t0` | the Map / Seams / Crust / Stack slices inside each pane |

**Actions and signals.** One action: `pop_seams_randomize` (the randomize button).
Signals are declared `on_<signal>` on the screen root — `Menu → pause_open`,
`PageNext`/`PagePrev` and `TabNext`/`TabPrev` → the rails' own step results. The rails
consume their four; the scene answers `pause_open`. Continuous `Look*`/`Zoom*` are read
from the signal source, never a device, and only while the centre pane is entered.

`Confirm`, `Cancel`, `Nav*`, `Panel*` and `ChordBegin` are the **walker's** on every
Prism screen. Declaring one here is a failing build — it once statically killed every
button on this screen.

**Node ids and stage sources:** panes `pop_left` / `pop_view` / `pop_right` · viewports
`pop_view_rtt` (stage `populous_globe`) and `pop_hex_rtt` (stage `populous_hex`) · rails
`paged_pages`, `paged_tabs`, `paged_tabs_p1`.

**Stringtable tokens:** `$pop_page_world` `$pop_page_hex` `$pop_tab_map` `$pop_tab_seams`
`$pop_tab_crust` `$pop_tab_stack` `$pop_size` `$pop_cells` `$pop_spots`
`$pop_seams_randomize` `$pop_stat_hexes` `$pop_stat_diameter` `$pop_stat_tile`
`$ui_pane_empty`. Theme colours: `$stage_void` `$world_seam` `$world_tile`.

## Extending it

**Adding a tab** touches four files, and nothing binds them together — a mismatch fails
to *nothing* (a dark rail, a dark pane, a blank page), so do all four:

1. **`src/ui.rs`** — a row in `PAGES`. If it paints a layer, it also needs an arm in
   `world_view()` matching its `id`; that match is a plain string compare that falls
   through to the authored look, so a typo shows the plain grey planet with no warning.
2. **`populous.scene.json`** — one rail `option`, plus a `visible_bind`-gated cell in
   **each of the three panes**. A pane with nothing to show still needs its gated
   `$ui_pane_empty` placeholder, or it keeps showing the previous tab's content.
3. **`populous.lua`** — one `arrange()` line for the new key. **Easiest to forget,
   silent when you do.**
4. **`stringtable.json`** — the `$token` the label names.

**Adding a control** is shorter but has one extra step: the bind name in `ui.rs`, the
node in the scene file, its `$token`, an arm in `apply_results`, a publisher in
`model()` — **and** the component count in
`the_bench_is_exactly_the_catalog_and_nothing_else`, which fails the build until you
update it. That is the anti-drift mechanism working, but it means a content-only edit is
never content-only.

**Adding a layer** is a tab plus a field type, its re-derivation wired into every place
the field above it changes (see Sharp edges), and a cell in the stack view.

## Gates

`source ~/.cargo/env && cargo test -p flicker-populous` — **35 tests**, all green:
5 in `map.rs` (the tiling's laws), 4 in `seams.rs` and 2 in `crust.rs` (each layer's
field is the shape it claims), 24 in `scene.rs` (the surface, the rails, the dispatch,
the shared world, the signals). They are named for what they hold; read the names.

Four carry contracts a change must not quietly break:

- `the_planet_size_is_one_world_shared_by_every_tab` — forking the world per tab fails.
- `every_slice_shares_the_panes_and_is_gated_apart` — one tree, one three-pane
  arrangement, every slice on its own gate key.
- `arrange_lights_the_selected_tabs_slice` — the Lua lights exactly one page and one
  slice for every selection.
- `the_screen_declares_only_what_it_owns_and_every_one_has_an_arm` — the declared
  signals, and not one the walker owns.

## Sharp edges

- **The derivation chain is manual, in order, and a stale link fails SILENTLY.** Neither
  field holds a reference to what it came from, so after rebuilding the map you must
  re-derive the heat field **and then** the crust — in that order, and again whenever
  the heat field alone changes. Skip a step and the reads are *plausible*, not wrong-
  looking: a stale heat field reads cold, a stale crust reads bedrock. `resize` and the
  three seam arms in `apply_results` are the places that do it correctly; copy one.
  Every new layer adds a link.
- **A `visible_bind` naming a key `arrange()` never publishes fails to nothing.** At the
  page tier that blanks a whole page. The gates assert today's six key names by hand;
  they do not cross-check that every gate key in the tree has a publisher, so a seventh
  is unguarded by construction.
- **The centre cell is chosen on a layer tab, and only there.** The Map tab and the Hex
  page leave it wherever it was — at start-up, whatever the opening camera faced. The
  Hex page shows no indication of *which* cell it is displaying, because its side panes
  are still placeholders.
- **Every layer paint is a colour override on the authored shell, not a shell of its
  own.** Radii and insets stay whatever `stages.populous_globe` says, so re-authoring
  the stage changes every layer view too — and removing its second `shell` layer would
  leave them all with nothing to paint.
- **Repaint is latched, not per-frame.** The 92k-tile mesh is rebuilt only when the view
  actually changes or the data moves. Add a control that changes how the shell should
  look, and republish explicitly — nothing polls.
- **The stack's proportions are provisional** and its inter-cell gap is load-bearing:
  two closed columns sharing a face z-fight without it. The view is framed wide
  specifically to leave room for the layers still to come, which is why the stack sits
  small and low — that is the intended picture, not a framing bug.
- **The echo contract.** Every two-way bind echoes its resting value each frame;
  `apply_results` acts only on a *changed* value. The page arm resets the tab as a side
  effect, so its echo must never re-fire. Guard any arm you add the same way.
- **The viewport is square and entered-only,** and both centre views share that one
  pane — entering it hands the camera to whichever page is selected.
- **The readouts stay in Rust.** `populous.lua` implements `arrange()` and nothing else;
  the three stat values are pre-formatted in `model()` and ride `text_bind`. Moving them
  into a Lua `derive()` is a tracked, not-yet-landed pass.
- **Textures are registered in ID order** (white 0, muse 1, pad_glyphs 2) so a component
  added later draws without re-plumbing the atlas.

## Related

- [`../../../content/sensorium/README.md`](../../../content/sensorium/README.md) — how to
  author scene files and pair scripts: the format, the catalogs, the gates.
- [`../../frontend/flicker-widgets/README.md`](../../frontend/flicker-widgets/README.md) —
  the component kinds this scene names and the knobs they carry.
- [`../../frontend/flicker-globe/README.md`](../../frontend/flicker-globe/README.md) —
  the globe component both centre-pane views are.
- [`../flicker-clicktrainer/README.md`](../flicker-clicktrainer/README.md) — the sibling
  reference scene for the pure-2D chain (no windowed viewport).
