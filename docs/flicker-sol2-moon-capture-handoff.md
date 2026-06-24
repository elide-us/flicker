# flicker-sol2 — moon-capture phase handoff

Written 2026-06-24, end of the session that got the system-formation sim **working**. This doc is
the design + open questions for the **next** phase: moons, rings, and the (related) 3D question.
Read `docs/flicker-sol2-handoff.md` first for the full pipeline; this picks up from there.

> **Commit note:** the user said they'll commit the current (uncommitted) `examples/flicker-sol2`
> tree before the next session. Assume it's committed. State at handoff: 17 tests pass, clippy
> clean, ~2200 lines.

---

## UPDATE 2026-06-24 (session 2) — moon stability + retention + rings LANDED (all in `collapse.rs`/`scene.rs`)

The next-phase work below is **built**. 19 tests pass, clippy clean, conserved 1.0000, star 0.966.
The model stayed honest (no Hill *grab*; capture is still the force test). What changed:

**Slice 1 — moon stability (the "moons all collapse into the planet" fix).**
- **Collision radius ≠ accretion reach.** New `density(i)` (volume-additive over gas/ice/rock
  composition — derived, no table) + `phys_radius(i) = (3m/4πρ)^(1/3)`: a real, microscopic
  physical radius, distinct from the inflated `RADIUS_K·∛m` accretion reach. That gap is the room
  a moon needs.
- **Angular-momentum-aware merge** (`orbit_peri_apo`): within the reach, a *genuine satellite* is
  protected — merged only if its orbit's pericenter dips inside the collision radius (a real hit).
  Sibling and moon–moon pairs still contact-merge, so planet accretion + co-accretion are intact.
- **Satellite drag fix** (`dissipate`): softening-consistent circular speed (a true fixed point
  for tight orbits), and bodies orbiting a *planet* circularise (no sub-circular inward migration
  that fed moons to their death). The migration drag still applies to bodies orbiting the *star*.

**Retention through migration (`host_of`).** Diagnosis: moons were stripped because as a host
migrates inward, the closing star wins on instantaneous force, `primary_of` flips to the star, the
moon loses satellite protection, and the big reach eats it. Fix: `host_of` keeps a body a planet's
satellite if it's *bound* and within `HILL_FRAC` (0.5) of that planet's Hill radius, even when the
star out-pulls on raw force. **Retention of an already-bound moon, not a grab** — capture is still
the honest `primary_of` force test. Used by both `dissipate` and `merge`.
- **Result:** moons are now a stable fixed point in a settled system (test). In the real sim:
  healthy radial spread (~0.8–44 AU), star dominant, **~1–3 *settled* moons** per system. The
  early "24–44 moons" are transient pairs in a still-merging disk and consolidate legitimately.
  Moon abundance is emergently **low** — most infalling material is gravitationally focused into
  the planet (plunges) before the gas drag can circularise it. Raising it honestly needs **finer
  integration near planets** (adaptive substeps so the circumplanetary drag can circularise
  infalling material) / stronger circumplanetary drag — a separate slice, NOT done.
- **Rejected:** softening the gas-era migration (`DRAG_TARGET_FRAC` 0.95→0.98) to slow Hill-radius
  collapse — it retains a couple more moons mid-run but **triples the leftover-body count** (the
  known "too many bodies" issue) for ~no steady-state gain. Reverted; left at 0.95.

**Slice 2 — tidal shear → rings (`roche_radius`, `ring_mass`).** A *close, settled* satellite whose
**whole orbit** (apocenter, not just pericenter) sits inside its host's tidal/Roche radius is
shredded into a ring instead of held as a moon. The apocenter gate keeps eccentric **infalling
debris out of the rings** (that still accretes) — only genuinely close circularised satellites ring.
- **Density physics is emergent + real:** `roche = R·(2ρ_host/ρ_body)^(1/3)` — a low-density **icy**
  body shreds (wide ring), a dense **rock** barely does (stays a moon / merges). So ice giants ring,
  captured rocks survive.
- **Scale caveat (load-bearing):** the *true* Roche limit is **sub-softening** at real planetary
  radii, so a true-scale ring never resolves. We keep the density **ratio** but scale it to the
  sim's resolvable accretion radius via **`TIDAL_FRAC` (0.1)** — the same inflation principle as the
  accretion reach / `BODY_DRAW_BOOST`. `TIDAL_FRAC` is the **ring-prominence dial**: 0.1 → ~3–4
  ringed bodies/system, modest ring fractions, moons preserved; ~0.2 starts making ring-dominated
  blobs (infalling material rings). `ring_mass[i]` is the per-body ring tonnage (a subset of
  `mass[i]`, so total mass is conserved exactly); the renderer draws it as a translucent icy
  annulus and the status line shows `N ringed`.

**Star-moon fix (post first visual verify).** The user verified: system looked right *except the
**star was also capturing moons*** (small bodies hugging the central star + an inner spiral of
unmerged debris). Root cause: the satellite protection was applied symmetrically, so because the
star is everyone's dominant attractor, inner bodies became "the star's satellites" and were
shielded from merging into it (the committed sim had absorbed them within the star's reach). Fix +
the user's framing (the 3-body precedence shifts at the **L1 point**): a *planet* has a **bounded**
domain — its Hill/L1 sphere, set by the L1 point with the star — inside which satellites orbit and
are protected; the **star is the root dominant with no outer L1 boundary**, so close material is
ABSORBED, not hosted. Implemented as `a != 0 &&` on the satellite-protection branch in `merge`
(the star, always the heaviest in a pair ⇒ `a==0`, falls straight through to the contact merge).
Verified headless: **0 bodies inside the star's reach, 0 star-moons, planet moons preserved (~2),
clean inner edge ~0.9–1.5 AU**. Test: `the_star_absorbs_close_bodies_rather_than_hosting_moons`.

**New tuning dials:** `HILL_FRAC` (moon retention), `TIDAL_FRAC` (ring prominence), `RHO_*_GCC`
(class densities). The user verifies visually and may dial these.

**Visualization polish (same session, all `scene.rs` / `BodyType::color`).**
- **Distinct body palette** — six saturated hues (gold star / amber gas giant / blue ice giant /
  red rocky / cyan icy / stone asteroid), replacing the old near-identical pastels; drives discs,
  motion vectors, and the legend.
- **Curved motion vectors.** `draw_motion` now draws each body's vector as an **osculating-circle
  arc** (curvature from the star's gravity: `n̂` = the ⟂-to-velocity part of `a`, `R_c = v²/|a⊥|`),
  starting tangent to the true velocity and bowing around the star — for a circular orbit it lies
  on the orbit ring, for an eccentric/perturbed one it bends correctly, so it tracks the well's
  shape. Forward arc carries the arrowhead; the dotted trail is the mirror arc behind. Arc length
  scales with orbital range (`MOTION_ORBIT_FRAC`), sweep capped (`MOTION_ARC_MAX_SWEEP`), radial/
  degenerate cases fall back to a straight arrow. Curvature is star-only (dominant for every body
  that draws an arc; moons draw none). `Sim::orbit_host(i)` exposes host_of for the renderer.
- **Captured moons draw no vector** (absolute velocity mirrors the host; relative motion is
  sub-pixel at system zoom — the moon disc beside its planet reads as a satellite).

**User validation (2026-06-24):** *"a truly great result … every time the system gets to around
1.5 BY it is looking very much like our natural system."* The emergent systems land in the
Sol-like band on their own — the model + the moon/ring/viz work is confirmed good.

**Still open (NOT done):** denser moon systems (finer integration near planets); rendering moons as
discs near their planet (they draw via `draw_collapse`/`draw_motion` already, but no dedicated moon
disc/orbit); the 3D question (user chose **2D now**); body-consolidation pass for the high
gas-giant count.

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
