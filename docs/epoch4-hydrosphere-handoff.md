# Handoff — Epoch 4 hydrosphere: oceans from the H/O budget

> **Status: landed (2026-06-13).** Epoch 4's oceans are now **composition-driven**
> and **visible forming**. Dial Epoch 1's hydrogen or oxygen and the ocean volume in
> Epoch 4 responds; step Epoch 3 → Epoch 4 and the seas fill the basins. Continues
> the E1→E2→E3 continuity arc (see [`epoch3-isostasy-handoff.md`] and
> `planet-evolution-sim` memory). User-confirmed E3 was the right direction;
> this applies the same "promote earlier data, make it dominant/visible" pattern to E4.

## What the user asked for
1. **See the formation of oceans** at Epoch 4 (not a static climate map).
2. **Ocean volume derives from the H/O endowment** — lower H *or* O at Epoch 1 →
   less ocean at Epoch 4. "Focus on realistic processes."

## What landed

### 1. Water endowment from the element budget (`crates/flicker-worldgen/src/epoch4.rs`)
- `water_endowment(prev, tables)` — water is **H₂O**, so it needs hydrogen **and**
  **free oxygen**: the oxygen left once the rock-forming cations take theirs.
  `oxide_oxygen_per_atom` charges each non-volatile, non-O element `valence/2`
  oxygen (a cation of charge *v* balances *v*/2 of O²⁻); silica-/iron-rich crust
  locks most of the oxygen, an oxygen-rich mix leaves a surplus.
- **Mass-action (product), not limiting-reagent (min).** `water ∝ (H/2) × free_O`.
  *Why not min:* Epoch 1 renormalizes every hex to a fixed total mass, so lowering
  O's share **raises hydrogen's share**; with a limiting-reagent `min`, water is
  hydrogen-limited at the default mix, so dialing O *down* actually nudged ocean
  *up* (the H-bump won). A product makes **both** reactants always contribute, so
  lowering H or O both drain the sea — defensible as law-of-mass-action kinetics
  (rate ∝ [H]²[O]). Measured response at the default calibration (freq-32 world):
  - H 0.14→0.07 → submerged **0.62 → 0.44**; H×3 → floods.
  - O 46→35→30→25→20 → **0.62 → 0.60 → 0.59 → 0.56 → 0.52** (smooth, no cliff).

### 2. Oceans fill by **volume**, not by a fraction dial
- `fill_to_volume(elevs, volume)` binary-searches the sea level that holds the
  endowment's water on the Epoch-3 terrain (`Σ max(0, level − elevation) = volume`).
  Little water pools in the deepest basins; more spreads onto the shelves — physical.
- The old `ocean_fraction` (a target submerged %) is **gone**, replaced by
  `hydration` (default 1.0, range 0..3): an outgassing/delivery efficiency that
  scales how much of the endowment reaches the surface. Submerged fraction is now an
  **output**. Renamed everywhere: `Epoch4` field, `PARAM_DEFS`, `world.rs`,
  `ui_elements.json` (label "Hydration").
- `WATER_FILL_GAIN = 0.1` calibrates the default Earth-like mix to ~62% ocean.
  **If the default coverage drifts, re-check this** — guarded by
  `world::ocean_volume_follows_the_hydrogen_and_oxygen_budget`.

### 3. Oceans become *visible* at Epoch 4
- `natural_view(3)` switched Temperature → **Elevation** (`scene.rs`), so stepping
  into Epoch 4 stays on the relief map and you watch the water arrive. Temperature
  is one `V`/tab away.
- `relief_color` (`color.rs`) now draws ocean **only where water stands**
  (`water_depth > 0`), not wherever `elevation < 0`. So Epoch 3 is a **dry**
  tectonic world (basins are bare low ground; the substrate-following continents the
  user praised are still the high ground) and Epoch 4 is when the seas *form*. Dry
  land ramps from the deepest ground when there's no sea yet, else from the
  coastline — `Ranges.max_depth` distinguishes the two. (The Terrain view already
  keyed off `water_depth`, so the two views are now consistent.)

## Data caveats / model notes
- Elemental densities in `data/materials/periodic_table.json` are **gas-phase at
  STP** (O = 0.00143 g/cm³), so they're useless for "crust mean density" — the
  oxide-demand model uses `valence_electrons` (the book's gameplay valence), not
  density. `Fe` is counted as an oxygen sink (valence 2) even though Epoch 2 sinks it
  to the core — an iron-rich world reads drier, which is a reasonable coupling.
- The default mix sits a little oxygen-rich of the silica balance (free O ≈ 2× the
  hydrogen supply per hex), so **hydrogen is the punchier lever** and oxygen the
  gentler one. If the user wants oxygen to bite harder, raise the oxide demand
  weighting or shift the default mix toward the balance.

## Tests (all headless, green)
- `epoch4::free_oxygen_gates_the_water_budget`, `more_hydrogen_makes_more_ocean`,
  `less_oxygen_dries_the_world`, `fill_to_volume_holds_the_requested_water`,
  `ocean_fills_the_low_basins_first` (+ the existing temp/precip/prebiotic tests,
  reworked onto a `epoch4_at_sea` helper that sets the coastline independent of the
  fill gain).
- `world::ocean_volume_follows_the_hydrogen_and_oxygen_budget` — default coverage in
  a sane band, and H-down / O-down drain while H-up floods (the calibration guard).

## Open / next (user-driven — don't presume)
- **Atmosphere has no spatial character** and no view — it's well-mixed. Outgassing
  concentrated near volcanic provinces, or a CO₂/greenhouse field feeding back into
  temperature, would give it something to see and guide.
- Prebiotic cradles could read more clearly along the warm-shallow-volcanic shores.
- Oxygen-lever punch (see caveat) if the user wants it stronger.
