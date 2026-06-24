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

## Locked 2026-06-23 — the mass source and the collapse framing

The open question in the model above (where per-element *amounts* come from) is **resolved**, and it
dissolves the four-attempt failure. Long-form: MCP memory decision **"flicker-sol2 mass source LOCKED"**.

- **Two dials, derived — never a table, never asked of the user.** The cloud's per-element *tonnage*
  is derived from two starting values:
  - **Mass** — the total origin mass (the existing `explosion` / supernova-size dial): how much material.
  - **Metallicity** — the fraction of that mass that is *metals* (everything heavier than He; Sun ≈ 1.4%):
    the gas-to-rock balance. Low Z → big star, little planet stuff; high Z → rockier, metal-rich systems.
  These two "control the shape and volume of the mass ejection" (user). Bias toward Sol-like through them
  (e.g. Z ≈ solar), **never** by clamping the physics.
- **Per-element split = the cosmic abundance curve.** The measured solar-system composition *is* the
  integrated output of real supernova nucleosynthesis: steep decline with atomic number, even-odd zigzag,
  **iron peak** at Fe/Ni, post-iron **cliff** (U ≈ 12 dex below H). Known for all 27 Prism elements.
  Amount = abundance(Z), renormalised so total = Mass and metals-share = Metallicity; spread in **angle**
  by the existing cloud field and placed in **radius** by the existing cast model. The confirmed Stage-1
  view is untouched — this is a conserved-mass data layer beneath it. (WHERE = f(atomic mass) [cast];
  HOW MUCH = f(atomic number) [abundance].)
- **Formation = inward gravitational COLLAPSE on the existing cast cloud.** The cast throws H/He to the
  *outer* rings, but the star is H/He and forms at the *centre*: so the cast cloud is the initial debris
  snapshot, and gravity **re-collapses it inward** — conserved mass *moves* toward the centre of mass; the
  dominant central accumulation becomes the **star** (a 2nd/3rd dominant lump = a 2nd/3rd sun, emergent,
  never clamped). This collapse runs **on the cloud we already have** — it is **not** a separate
  condensation disk (that was failed attempt #1). "Disk" kept being reached for because a real collapse
  *makes* a disk; the fix is to run the collapse on the cast cloud, not replace it.
- **Corrected scenario:** a massive star dies (supernova), flings its processed metals + H/He envelope
  outward (the cast), enriching a cloud that is still mostly H/He; that cloud collapses and a **new** star
  ignites at the centre (not the old one reigniting); the leftover debris is its planets. One
  self-contained event per seed.

**Built 2026-06-23 — mass layer + collapse core (slices 1–2).** `mass.rs`: the two dials →
per-element conserved tonnage via the cosmic abundance curve (`Σ == Mass`, `Σ metals == Z·Mass`),
with an in-app readout (`9`/`0` Mass, `7`/`8` Metallicity). `collapse.rs`: the cloud is sampled into
**motes** (parcels of conserved matter, mass weighted by the cloud clumps), given the re-collapse
initial velocity (inward fallback + low spin), and they fall under direct-sum gravity (softened,
symplectic-Euler substeps) and **merge on contact** (inelastic, conserving mass + momentum +
composition). Ignite with `Enter`; `R` clears back to the distribution view. 16 tests pass
(conservation, merging, a dominant body emerges, no blow-up); clippy clean. Tuning constants:
`SPIN_FRAC`, `INFALL_FRAC`, `RADIUS_K`, `SIM_YEARS_PER_SEC`.

**Gas drag added + tuned to Sol-like (2026-06-23, user asked to bias toward a single central star).**
The first vacuum run formed a binary that, around ~60 yr, slingshotted ~90% of the system into deep
space. The honest missing physics was dissipation: the bodies move through the gas cloud, which drags
on them. `dissipate()` bleeds kinetic energy (`v *= 1 − drag·dt`), fading over `GAS_TAU` as the gas
disperses — so orbits decay inward (mass concentrates into one central star) and nothing reaches escape
speed (the system stays bound). Not a clamp; it's the disk gas the vacuum N-body lacked. Tuned
constants: `SPIN_FRAC=0.18`, `DRAG=0.6`, `GAS_TAU=35`, `INFALL_FRAC=0.30`, `RADIUS_K=0.8`.

Headless across 4 seeds at the defaults (~260 yr): a **single central star (97–100%, at/near the
centre)**, **3–5 substantial planets** (a gas giant + Neptune-mass bodies) plus smaller bodies (23–57
total), **~100% of the mass bound** within 60 AU, conserved to 1.00000. No binaries, no wholesale
ejection. (Sol itself is 99.86% star, so a star this dominant is realistic; the planets are correctly
tiny fractions.)

**Still open (next threads):** body–body **interactions** beyond merge (capture-as-moon, Roche
shred-to-ring, explicit ejection), **recursion** (moons forming around planets by the same
aggregation), emergent **type labels** (star/giant/planet/moon/asteroid/comet, ice/rock/water) read off
the result, the **radial architecture** the user wants (rocky inner → gas giants → ice giants at the
reaches, ice moons), the **habitability** gate (temp + pressure), and **composition export** for the
chosen HZ world. The planet bodies are tiny in mass (realistic) so the renderer may want to exaggerate
their drawn size for visibility.

**Added 2026-06-23 (later):** emergent **body typing** (`BodyType` via `classify()`: star / gas giant /
ice giant / rocky / icy / asteroid, from mass + composition — gas = H/He, ice = C/N/O, rock = the rest)
now drives each body's colour, a system-makeup readout, and a type legend; plus a **`Tab` = new system**
control (reseed the cloud + re-ignite) to flip through emergent systems. Verified across seeds: defaults
give a central Star + a hydrogen GasGiant + Neptune-mass rocky bodies + asteroids; the ice types exist in
code but don't emerge yet (volatiles mostly merge into the star — the radial ice architecture is still
ahead). **Next, for the orbital *consistency* the user flagged (erratic orbits): the body interaction
layer — capture-as-moon, Roche shred-to-ring, damping close encounters — plus recursion (moons) and the
radial rocky→gas→ice sort.**

**Connected to the stage-2 disk (2026-06-23, the key re-architecture — user caught the disconnect before
it became "failure #5").** The collapse was a *parallel* model: it re-sampled the cast into an even
angular **grid** (visible spokes) with an *invented* spin/infall, ignoring the cloud's clumps and
rotation. The fix makes the collapse run on the **same stage-2 disk** the rings view shows:
- **Seeded from the disturbances** — parcels are drawn from each ring's angular-density **CDF**
  (`hash3`/`rand01` + `partition_point`), so they cluster in the clumps (the dots), not on a grid. A
  reseed now gives a genuinely different system. (Headless: initial angular CV ≈ 0.16, i.e. clumpy, not 0.)
- **Motion = the disk's rotation** — `SPIN_FRAC` raised to 0.85 (near-circular orbital motion in the
  shear's sense), `INFALL_FRAC` dropped to 0.05; the inward collapse comes from the gas drag, not an
  invented plunge. Coherent angular motion from t=0, no radial sloshing.
- **One model** — the Mass/Metallicity tonnage (`mass.rs`) × the cloud clumps/rotation (`cloud.rs`) ×
  the cast radius *are* the stage-2 disk; igniting only switches on gravity. (Stages: 1 = the supernova
  cast/dispersal; 2 = the rotating clumpy disk it settled into — gravity must start from 2.)

Headless across seeds: still a single central star (96–100%), bound (100% < 60 AU), conserved (1.0000),
with varied body counts (19–44) per seed. 17 tests pass; clippy clean. Clumpiness tracks the confirmed
view's clump dial (0.6), so it's faithful, not exaggerated.

## Star-extraction model (2026-06-23 — the decided model, user-corrected through two interrupts)

Simulating the H/He collapsing inward to a central star fought the cast geometry (H/He is cast
*outward*) and produced endless instability (rotation damped to rest → radial sloshing; a binary that
slingshotted everything out; a drifting star). **The decided fix is to cheat:** extract the star's share
of the gas straight into a pinned central star, and run the collapse only on *the rest of the cloud*.

- **Extract** `STAR_GAS_FRAC` (0.98) of the gas (H/He) → **body 0, the star**: placed at the centre,
  **pinned** (it gravitates but never moves — skipped in `integrate`/`dissipate`, kept fixed in `merge`).
  Dominant (~97%), so no drift and no slingshot ejection.
- **The disk = the rest of the cloud** — the *leftover* gas (so gas giants can still form — do NOT extract
  all the gas) + all the metals. Seeded from the clump CDF (clustered at the dots), orbiting the star at
  the circular speed. Conserved: `star + Σ disk == total tonnage`.
- **Accretion:** gas drag circularises the disk and (while gas is present, `DRAG_TARGET_FRAC` 0.95
  sub-Keplerian) migrates parcels inward so they cross and **merge** into planets; a perpetual `DRAG_FLOOR`
  keeps the settled orbits stable. Types emerge from composition: leftover gas → gas giants, C/N/O → ice
  giants, metals → rocky.
- Removed the old `is_gas_body` gas/solid dynamics split, `recenter_momentum`, `GAS_SPIN`/`SOLID_SPIN`
  (replaced by `STAR_GAS_FRAC` + `DISK_SPIN`).

Headless (3 seeds, ~500–1500 yr): **star 97% pinned dead-centre, 0 drift, 100% bound, conserved 1.0000**,
~35–40 orbiting bodies — gas giants + ice giants + a few rocky. 17 tests pass; clippy clean. **Open tuning
(user to dial visually): the gas-giant *count* is high — the leftover-gas parcels don't consolidate into a
few big giants; needs a consolidation pass.** `STAR_GAS_FRAC` (gas-giant size / star dominance),
`DRAG_TARGET_FRAC` (migration vs body count), `MOTES_PER_EL`, `DRAG`/`GAS_TAU` are the dials.

> The two key user corrections this session (both via interrupt): (1) **don't be literal — don't extract
> ALL the gas**; leave gas in the cloud so gas/ice giants can form. (2) The star extraction is the
> "cheat" that is *meant* to be there; the rest is "roughly estimating what the rest of the cloud does."

**Working + committed (2026-06-24).** The sim runs and stabilises cleanly (user ran ~5K turns).
Viz landed: per-body **motion vectors** (`draw_motion` — forward arrow + dotted trail, replaced the
meaningless radius circles) and a **gravity-well overlay** (`G` toggle, `well.rs` — equipotential
contours + sphere-of-influence rings, computed as a real field so a tilted camera later turns it into
the funnel). The dead `,`/`.` gradient dial was removed; planets draw bigger. **Next phase = moon
capture (co-accretion), 3D the open question → `docs/flicker-sol2-moon-capture-handoff.md`.**

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
