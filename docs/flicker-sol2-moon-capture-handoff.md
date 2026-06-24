# flicker-sol2 — moon-capture phase handoff

Written 2026-06-24, end of the session that got the system-formation sim **working**. This doc is
the design + open questions for the **next** phase: moons, rings, and the (related) 3D question.
Read `docs/flicker-sol2-handoff.md` first for the full pipeline; this picks up from there.

> **Commit note:** the user said they'll commit the current (uncommitted) `examples/flicker-sol2`
> tree before the next session. Assume it's committed. State at handoff: 17 tests pass, clippy
> clean, ~2200 lines.

---

## Where the sim stands (the thing moons build on)

The **star-extraction model** (decided this session, see the main handoff "Star-extraction model"):
- Press **Enter** to ignite, **Tab** for a new system.
- Body 0 = a **pinned central star** = `STAR_GAS_FRAC` (0.98) of the cloud's gas (H/He). It
  gravitates but never moves (skipped in `integrate`/`dissipate`, kept fixed in `merge`). Dominant
  (~97%) ⇒ no drift, no ejection.
- The **disk** = the rest of the cloud (leftover gas + all metals), seeded from the clump CDF
  (clusters at the dots), orbiting the star, accreting (merge on contact) into planets. Types are
  read off composition: leftover gas → gas giants, C/N/O → ice giants, metals → rocky.
- Conserved to the float: `star + Σ disk == total tonnage`.
- The user ran ~5K turns of an earlier build and it **stabilised cleanly** — dominant star, planets
  on clean orbits, mass conserved. Confirmed "really close."

**Visualisation (done this session):**
- `draw_motion` (scene.rs): each body's **velocity vector** — a forward arrow + a dotted trail,
  tinted by type. Replaced the old radius-circle "orbits," which assumed circular orbits and were
  meaningless. Lengths via `MOTION_SCALE` / `MOTION_MIN_PX` / `MOTION_MAX_PX`.
- `well.rs` + **`G`** toggle: a gravity-well overlay — equipotential **contours** (marching squares
  on `ln(−Φ)` over a 64² grid) + each planet's **sphere-of-influence ring** (the real force crossover
  `r_orbit·√(m/M★)`). Computed as a real field so a future tilted camera turns it into the funnel.
  - Open polish: planet dimples may read as subtle warps (the star's well is ~1000× deeper); the SOI
    rings carry the "moon territory" read. `GRID`/`LEVELS` in `well.rs` are the dials; it recomputes
    each frame while toggled (toggle off if perf drags).

**Tuning constants (collapse.rs):** `STAR_GAS_FRAC` 0.98, `DISK_SPIN` 1.0, `DRAG` 0.6,
`DRAG_TARGET_FRAC` 0.95, `GAS_TAU` 35, `DRAG_FLOOR` 0.02, `RADIUS_K` 0.8, `SOFTENING` 0.05,
`MAX_DT` 0.02; `MOTES_PER_EL` 24, `SIM_YEARS_PER_SEC` 6 (scene.rs). Open tuning the user flagged:
**gas-giant count is high** (~25; leftover-gas parcels don't consolidate into a few big giants) —
a body-consolidation pass + the inner-rocky/outer-gas radial sort are still pending.

---

## Moon capture — the design (THE next phase)

### The load-bearing finding (do NOT re-learn this)

**Real gravity does not capture flybys.** Verified headless: with honest N-body + the disk drag,
**zero moons** emerge, every seed. Gravitational capture needs energy bled off *during* the
encounter; a gentle disk drag can't bind a body screaming past a planet — it leaves on a hyperbola.
Reaching in with a **Hill-range grab would fake it** — the user explicitly rejected that
("we're not just using Hill ranges to arbitrarily grab things… keep it real, no shortcuts").

**Real moons form the other way — co-accretion / the circumplanetary disk.** As a planet pulls in
its feeding zone, that infalling material has a spread of **angular momentum around the planet**.
Material aimed at the planet falls in. Material with enough sideways motion to clear the planet's
surface can't fall in — it settles into orbit (the circumplanetary disk) and accretes into moons.
The same aggregation, one level down. It falls out of the inputs; no grab.

### What's already built (the honest foundation)

`dissipate` now circularises each body toward its **real dominant attractor**, not always the star:
`primary_of(i, p)` returns the heavier body exerting the largest `G·m/r²` on `i` (the star always
qualifies; a planet wins only when it genuinely out-pulls the star — which IS the real condition for
`i` to be that planet's satellite). **No Hill formula.** So the moment a moon exists, it circularises
around *its planet*, not the star. This is the right base; it just doesn't *create* moons yet.

### What's missing (build this)

The accretion is **angular-momentum-blind**: a body merges the instant it's within a planet's reach,
so the planet swallows its would-be moons. Make the merge honest about impact parameter:

1. **Real collision radius** separate from the inflated accretion reach (`RADIUS_K`) we use for
   planet growth. Moons orbit *outside* the collision radius.
2. **Angular-momentum-aware merge:** when a lighter body comes within a heavier one's reach, compute
   its actual orbit around it (from real relative pos + vel). **Pericenter clears the surface →** it's
   orbiting, not hitting → it stays in orbit (a moon; the dominant-attractor drag circularises it).
   **Pericenter below the surface →** real collision → merge.
3. **Finer integration** for the tight circumplanetary orbits — the user signed off on "calc can be
   the limiter, keep it real." Likely adaptive substeps when bodies are close, and/or smaller
   `SOFTENING` (it currently floors orbit tightness at ~0.05 AU).
4. **Rings = the same test, one step further:** material that settles *inside* the planet's **Roche
   limit** (`r_Roche = R_planet·(2·ρ_planet/ρ_body)^(1/3)`, a real formula from densities) is tidally
   shredded into a disc instead of forming a moon. Draw it as a ring around the planet.
5. **Recursion** is mostly free: moons of the same planet merge among themselves via the existing
   contact merge (aggregation one level down). The new work is rendering moons orbiting their planet
   and typing them.

### Render (when moons exist)

- Draw moons as small discs near their planet; a moon's motion vector already works (`draw_motion`).
- A faint orbit/ring around a planet for its moons; the Roche-shred ring as a disc.
- Possibly a `BodyType::Moon` (read off "primary is a planet") for the makeup line / colour.

---

## The 3D question (raised this session, NOT yet decided)

The user pointed out the sim is **2D** (`Vec2` pos/vel, planar gravity). Precise consequence:
**eccentricity already works in 2D** (an ellipse is planar; our orbits are near-circular only because
the gas drag circularises them) — what 2D *cannot* do is **inclination** / vertical disk structure /
true 3D accretion geometry. Real planetary systems are nearly coplanar, so 2D captures most planar
structure, but true 3D accretion wants the third axis.

**Path if/when we go 3D** (it pairs naturally with moons and with the camera-unlock):
- **Sim:** `Vec2 → Vec3` through `collapse.rs` (`r²` gains `dz²`); seed a *thin* disk (small vertical
  scale height, not a perfect plane) so there's something to incline; the drag also **settles bodies
  toward the midplane** (vertical damping) as real disk gas does.
- **View:** do *not* port to the engine's 3D mesh pipeline — project each `Vec3` to the screen through
  a small camera transform and keep drawing discs with the 2D pipeline. Add pitch/yaw to that transform
  and **that's the camera unlock** the user wants (tilt/orbit the system). The well's `Φ` field then
  stands up into the rubber-sheet funnel when tilted.
- **Decision for next session:** do moons in the current 2D first, or move to 3D first (so moons form
  with real inclinations)? Either order works; 3D-first is more honest but a bigger lift.

---

## Files

`examples/flicker-sol2/src/`: `model.rs` (cast + Prism table + element colours), `mass.rs` (two-dial
tonnage via cosmic abundance), `cloud.rs` (clumpy sheared rings), `detect.rs` (overdensity dots),
`collapse.rs` (**the sim** — star extraction, seeding, gravity, drag/`primary_of`, merge, typing),
`well.rs` (gravity-well overlay), `draw.rs` (2D primitives incl. `dotted`), `scene.rs` (the viewer +
all dials + `draw_motion`), `main.rs`.

## To sync to MCP memory next session (server was disconnected this session)

Update `project: flicker`:
- The session_summary (key 9EF5B29F) and the "mass source LOCKED" decision (C79F2F14) are current
  for the model; **add**: the sim is built + working + committed; motion-vector + gravity-well viz
  landed; `primary_of` dominant-attractor in place; **next = moon co-accretion** (this doc); the **2D
  vs 3D** open question. The local memory `flicker-sol2-mass-source-collapse` has the same in brief.
