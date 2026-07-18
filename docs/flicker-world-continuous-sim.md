# flicker-world — The Continuous, Cooling-Driven Planet Simulation

The architecture the epoch model is converging on (design locked with the user
2026-07-08). It supersedes the *execution model* of the discrete 9-epoch hand-off:
the epochs become **onset markers on one evolving state**, driven by a single master
clock — **heat loss**. Read alongside `docs/flicker-world-epoch-redesign.md` (the
manifesto it refines) and `docs/flicker-world-e3-tectonics-handoff.md` (the E1–E3
physics that feed in).

> **One line:** a planet cools from molten over deep time; **processes turn on when the
> cooling state crosses their conditions and then keep running**, layering on top of one
> another, so the scrubber is a *heat-loss timeline* and every planet's history is
> generated (different onsets each seed), not scheduled.

---

## 1. The master clock is heat loss

- The planet starts **molten** and radiates its heat budget away (accretion + radiogenic
  heat leaving). This absolute **thermal state** is the master variable — every onset
  keys off it.
- **The scrubber IS the heat-loss timeline.** It is **logarithmic** — fast, cumulative
  action early (rapid cooling, vigorous convection, quick differentiation) that slows
  toward a geological crawl as the planet cools. Scrubbing = moving through cooling.
- Cooling rate is **composition-modulated** (radiogenic elements slow it, size/insulation
  matter), so two recipes reach each threshold at *different* times — the source of
  planet-to-planet variety.

## 2. Processes accumulate — nothing is handed off

Convection is **permanent**; it runs from the first molten tick until the planet is dead.
Every later process turns **on** when its condition is met and then **also ticks forever**,
layering on. No epoch "ends" and passes to the next — they stack and co-evolve (the way the
old hex-map computed its layers together, now on a long-term iterative basis).

**Convection unifies old-E2 + old-E3.** One convection flow: while the crust is mushy it
**stirs the melt** (old Epoch 2); once the lid has **cooled + firmed** into rigid crust,
that same flow **moves it as plates** (old Epoch 3). "Tectonics begins" is not a new sim —
it is convection starting to grip a rigid lid. Plate **consolidation** (small clusters
merging into a few large plates ringed by persistent convergent arcs — "ring of fire")
is just the conveyor finally running long enough.

## 3. The two authored injections (the only "fakes")

Everything else is simulated from the state. Two inputs are injected from outside:

1. **Starting materials** — the E1 seed: the **dry** bulk composition (Prism elements,
   absolute masses) scattered spatially with accretion-disk-clearing concentration zones.
   No water, no compounds — those are later chemistry.
2. **Water** — delivered from the **outer system** (late veneer / cometary), injected at
   the appropriate cooling point. Not made by the planet; added when the surface is ready
   to hold it.

## 4. Tagged events (condition-triggered)

Each fires when the cooling state (+ chemistry) crosses its threshold, and adds effects:

| Event | Trigger (from the cooling state) | Adds |
|---|---|---|
| **Tectonics onset** | crust cooled below solidus **and** firmed over a coherent-lid share | convection moves the rigid lid as plates; ridges/trenches/subduction |
| **Outgassing** (may be several) | interior degasses as it cools past volatile-release points | builds the early atmosphere from the interior's volatiles |
| **Water delivery** (injection) | surface cooled below boiling | oceans condense; the hydrosphere / water cycle turns on |
| **Moon-forming collision** | a tagged major-impact event | **axial tilt** → turns on **seasonal** temperature calculation |
| **Chemistry onsets** | temperature / reactant windows (carbonates, organics, …) | compounds layer on (the material-pipeline stage-2 chemistry, condition-gated) |

Stagnant-lid / dead worlds (never firm a mobile crust, or fully cool before life) are
**valid outcomes**, not failures.

## 5. Guardrails: Earth-like, but unique every time

Reverse-engineered thresholds/ranges (which event does what, and the windows they fire in)
are tuned so outcomes **land Earth-like** — the manifesto's "play Earth backwards to design
the levers." Within those guardrails the sim runs **forward and emergent**, so every seed
grows an interesting, unique planet. Guardrails constrain the *envelope*, not the result.

## 6. What this changes vs. today

- The 9 fixed epochs → **onset markers on one cooling timeline** (the `.epoch` snapshots
  can still be captured at markers for the cache/replay, but the truth is the continuous
  state).
- `e*_duration` levers (fixed step budgets) → **thresholds/rates** on the cooling clock.
- The per-epoch iteration scrubbers → **one logarithmic heat-loss scrubber** with generated
  onset markers.
- Feeds forward to the **heightmap detail tier** + **atmospheric cycles** (the future
  Group-III work) as just more layers that turn on.

## 7. Build order (foundation first)

1. **Cooling model** — an absolute planetary thermal state that decays (the heat-loss
   clock), composition-modulated. *Everything keys off this — build it first.*
2. **Continuous convection tick** keyed to the thermal state (vigor ∝ heat; differentiation
   rate ∝ how molten).
3. **Tectonics onset** — detect cooling + firmness crossing the rigid-lid point; the same
   convection flow begins moving the rigid crust as plates.
4. Then, as separate layers: **water-delivery** onset, **outgassing** events, the
   **moon/tilt/seasons** event, **chemistry** onsets, and the **logarithmic scrubber** UI.

Salvage: the E1 scatter, the material-driven convection + `heat` field, `convection_flow`,
the heat-seeded partition, the conveyor + subduction, isostasy/pile, weathering, the water
cycle, veins, erosion/biomes — all kept; they become **condition-gated layers on the
cooling clock** instead of sequential epochs.
