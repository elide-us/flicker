# flicker-sol2 — handoff

`examples/flicker-sol2` (`cargo run -p flicker-sol2`). **Currently: the ejecta-cloud
*distribution* viewer only.** Built on flicker-render (2D) + flicker-scene + flicker-app;
consumes flicker-materials (the 27-element Prism table).

## What it is now

A 2D top-down view of the supernova's **cast material distribution**: one translucent colour
ring per Prism element at a distance set by its **atomic weight** (heavier = nearer the star,
lighter = farther). The cloud is **differentially sheared, clumpy, and meandering**
(`cloud.rs`), and **overdensity dots** mark where matter concentrates (`detect.rs`, toggle `B`).
A focus deck (hover or ←/→) highlights one element's radial density gradient at a time.

**This view is confirmed right** (the user: *"that is legit the right view … it LOOKS right"*).
It is the distribution only — there is no formation/aggregation step in the tree.

**Files:** `main.rs` · `model.rs` (cast-by-weight ring positions + element palette) ·
`cloud.rs` (shear/clump field) · `detect.rs` (overdensity dots) · `draw.rs` (2D helpers) ·
`scene.rs` (the viewer + distribution sliders). ~1096 lines; builds clean; 7 tests.

**Controls:** `[`/`]` explosion (cast reach) · ↑/↓ falloff (atomic-weight→distance steepness) ·
`,`/`.` gradient · `;`/`'` clump · ←/→ or hover focus an element · wheel/`-`/`=` zoom ·
Space pause · N reclump · B dots · R reset · Esc.

## What was removed, and why (2026-06-23)

The **formation simulation was deleted** after four separate failed attempts. `disk.rs`,
`accrete.rs`, `subdisk.rs`, `aggregate.rs`, `field.rs`, and all the post-`A` Stage-2 code in
`scene.rs` are gone. The four attempts, in order:
1. **Condensation disk** — rebuilt the system from *solar abundances* + temperature, ignoring
   the cast. 2. **Hill-grid core accretion** — placed bodies on a gravity-math grid, discarding
   the overdensity dots. 3. **Circumplanetary sub-disk moons** — the same condensation pattern
   one level down. 4. **Conserved aggregate field** — a whole 4th parallel system.

**Root cause (the load-bearing lesson):** every attempt **built a new parallel system instead
of deriving from the starting values that already exist.** (E.g. it asked the user to supply
per-element amounts when the disk already defined them.) **EVERYTHING IS DERIVED FROM THE
STARTING VALUES.** A rebuild must derive from the existing inputs — the Prism table, the cloud
distribution, the cast params, the seed — in the existing pipeline, not bolt on new tables.

## The model for an eventual rebuild (design of record)

Recorded in MCP memory (project `flicker`): spec **"flicker-sol2 Stage-2 = body-aggregation-
from-a-cloud sim"** and decision **"flicker-sol2 formation sim ROLLED BACK"**. The shape the
user actually wants (confirmed, but NOT yet correctly built):

- The cloud is conserved matter (real per-element mass, **derived from the existing
  distribution / starting values** — do not invent amounts).
- Its **overdensity dots are *potentials*** — each seeds a body. **NOT all potentials become
  planets**, and bodies are **not** placed on a grid.
- Each body **gathers real mass** within its reach — mass is **moved** out of the cloud into the
  body, element by element, conserved to the gram (so a planet's composition is *exact and
  known*). Reach grows with mass (runaway is emergent).
- Bodies **interact**: comparable masses **merge**; a much smaller body near a bigger one is
  **captured as a moon**; one inside the Roche limit **shreds into a ring**; strong encounters
  **eject** or fling to **extreme/tilted orbits**.
- **Recursion:** a planet's captured satellites run the same aggregation one level down.
- **What a body *is* (comet/asteroid/planet/moon/giant/ring, ice/water/rock) is emergent** —
  read off the result, never selected.
- The point: the selected HZ world has an **exact, known elemental composition** for downstream
  world-gen (e.g. U placed across 1 AU so HZ worlds contain uranium).

## Guardrails for the rebuild (do NOT re-learn these the hard way)

- **A dominant body / a second sun is a VALID outcome, not a bug.** Multi-star (binary/triple)
  systems are *common*; Sol-like singles are *uncommon*. When the aggregation makes one body
  eat the system, that is the physics working — **do not clamp reach, normalize the cloud mass,
  or fall back to Hill-spacing to suppress it.** That "fix" reflex was the root of all four
  failures. The **star(s) emerge** from the aggregation (the dominant lump(s) are the sun(s); a
  seed yields 1/2/3+); "the cloud is heavier than the star" is a regime, not an error.
- **Bias toward Sol-like via the STARTING VALUES, never the physics.** Tune the initial cloud
  mass + element distribution toward the narrow Sol-like band, using the *known* material makeup
  of our own planets as guardrails; accept that most seeds won't land there (that mirrors
  reality).
- **Habitability is a narrow physical gate** — liquid water needs temperature *and* pressure
  (~1 atm), not just orbital distance.
- (Full statement: MCP memory invariant "a giant eating the system / a 2nd sun is a VALID
  emergent outcome …".)
