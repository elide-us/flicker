# Spec & refactor handoff — `flicker-celestial` (unified celestial simulation crate)

**Status:** DESIGN / SPEC. No code written yet for `flicker-celestial`. This document captures the
design requirements (from the user, verbatim intent preserved) for a **new session** to: spec the
crate cleanly, define interfaces + use-cases, **refactor useful code in**, and **discard defunct
branches**. The work to date has been POC "careening" across several purposes; this is the step
back to **unify the celestial systems into one crate** and **extract logic into abstractions away
from the data**.

Read first: this doc, then `docs/flicker-solarsystem-handoff.md` (the current best formation +
rendering work to harvest), `docs/clayengine_world_generation_spec_v2.md` (epochs), `CLAUDE.md`
(§2 invariants, §4 crate map, §5 epochs).

---

## 0. The two load-bearing objectives (word them carefully)

Everything in this crate serves **two** equally-weighted objectives — the user chose these words
deliberately:

1. **Effective visualizations** — the data must drive renders that *read well* (a gas giant must
   look like a swirling ball of atmosphere; rings must read as rings; orbits/belts/comet paths must
   be legible). "Effective" — not merely pretty: the visual must communicate the system truthfully.
2. **Accurate data** — the same data must be *physically faithful enough* to feed the game systems
   (gravity, density, pressure, composition; conserved masses; real orbital relationships).

These are co-equal. A model that is accurate but unrenderable fails; a render that lies about the
data fails. Where art and reality diverge, follow the established rule (memory
`sim-reality-needs-procedural-drivers`): **art may lie for beauty (camera, glow, dust); simulation
reality — bodies, satellites, rings, composition, surfaces — must emerge from physical drivers and
be conditional, never blanket-applied.**

---

## 1. Why this refactor

- **Celestial logic is scattered** across ≥4 implementations that grew from different POCs and must
  be unified (§2 inventory): the formation sim (`examples/flicker-solarsystem`), the day/night sun
  model (`crates/flicker-world/src/celestial.rs`), the original day/night sim in
  `examples/voxel-cluster`, and the sky/lighting in `flicker-render`.
- **Logic is fused to data and to specific apps.** The formation sim's orbital/capture/collision
  logic lives inside one example's `sim.rs`/`collide.rs`/`disk.rs`. We want **data models** that
  multiple projects compose, and **functional blocks** (formation, evolution, rendering) that
  consume those models — logic *away from* data.
- **Two generation/erosion models exist.** The bulk of the effort is the **epoch system**
  (`flicker-worldgen`, epochs 1–6, coming along nicely). **Atmospheric / water-cycle** systems are
  less developed but have POCs of value (`examples/hex-world/src/layers.rs` — a conserved vertical
  water-cycle prototype) and captured ideas for **bridging the generative sim with the water-cycle
  sim** (`docs/water-cycle-handoff.md`, `docs/epoch-data-audit-handoff.md`).

---

## 2. Keep / Discard / Re-home inventory (grounded in the current tree)

### KEEP — proven, isolated, harvest into the new design
- **Voxel cluster** (`examples/voxel-cluster`, `crates/flicker-voxel`) — well-isolated, **tested,
  verified to perform the cluster process correctly**, storing **~2 MB of data per cluster**. The
  **micro-scale** of "composition theory" (a voxel is a container for a portion of a cluster's
  material distribution — CLAUDE.md §2). Leave as-is; the hex model (§8) is its **macro-scale
  sibling** at planet scale.
- **Epoch system** (`flicker-worldgen` epochs 1–6, `flicker-worldgrid` topology, `flicker-worldstate`
  ledger, `flicker-materials` vocabulary). The generation engine. Coming along nicely.
- **flicker-world hex composition** (`crates/flicker-world/src/{globe.rs,color.rs}`) — the
  hex-sphere mesh + per-cell composition coloring. To be **abstracted** into a generic "hexes of
  data" model (§8). NOTE: `flicker-world` is currently a **binary, not a library** — the refactor
  should extract its reusable `globe`/`color`/topology into a lib (this crate or a shared one) so
  `flicker-solarsystem` etc. stop reimplementing it (see `worldglobe.rs` duplication note in
  `flicker-solarsystem-handoff.md`).
- **flicker-solarsystem formation + rendering** (`examples/flicker-solarsystem`) — harvest the good
  ideas: supernova→disk initial conditions, condensation-by-radius composition, **gas-budget-gated
  giants**, **moon capture**, the **Epoch-1 composed hex globes** (`worldglobe.rs`), the engine
  **star point light** (now in `flicker-render`), the **comet camera**. The *logic* (formation,
  capture, collisions, orbital mechanics) is what migrates into `flicker-celestial`.

### DISCARD — defunct, causes confusion about "what is right"
- **`examples/hex-map`** — the **flat two-map / bent-rings / σ-zipper "weird rings"** precursor to
  modern flicker-world (`gadget.rs`, `snap_map.rs`, `snap_segment.rs`, `map_structure.rs`,
  `topology.rs`, `geom.rs`, `terrain.rs`, `text.rs`). The user explicitly flagged this as an
  **important cleanup**: it predates the ISEA hex-sphere and confuses what the right hex model is.
  Delete the crate **and** `docs/hex-map-handoff.md` (mark superseded). *(Note: `examples/hex-map/src/geom.rs`
  was once cited as a copy-source for flat within-hex math — confirm nothing live depends on it before deleting.)*
- **The polar-cap defect-concentration sketch** (CLAUDE.md §3) — never resurrect.
- `flicker-worldsim` — already removed (not in the crate list); keep it gone.

### RE-HOME — stranded but valuable, extract then retire the husk
- **`examples/hex-world/src/layers.rs`** — a working **vertical water-cycle prototype** (conserved
  to <0.1% over 300 ticks), stranded on the old flat topology. **Capture it** (user confirmed). Two
  distinct things come out of `hex-world`/Epoch-6, and they land in different places (§6b):
  - **Erosion** → a **later-stage evolution pattern** in `flicker-celestial::evolution` (§6b/§6c) — it
    is just a later stage of the system, run at its own time scale (§6d).
  - **Water cycle** → an **ongoing runtime system**, used **continuously** (not only at initial gen) —
    seeded from Epoch-4 output, run on the icosphere with cross-hex halo exchange
    (`docs/water-cycle-handoff.md`). Candidate home: a water/atmos sub-module the live sim ticks
    (CLAUDE.md §1 live-sim/GC). It is the start of the **generative ↔ water-cycle bridge**.
  - Then retire the rest of `hex-world` (its explorer is superseded by `flicker-world`).

---

## 3. The new crate: `flicker-celestial`

**Scope.** Owns the **data models** (§4) that describe a star system as a tree of bodies and
satellites with physical fields, plus the **functional blocks** that operate on them:
**formation** (cloud → system, §5) and **evolution** (procedural processing of system state over
time, §6). It does **NOT** render — rendering consumes the data (§9). It is **data + logic**, no GPU.

**Layering (logic away from data).**
```
flicker-celestial
├── model/      data only: Body, Satellite (tree), System, fields (gravity/density/pressure/composition)
├── formation/  cloud → initial System (fuzzy, partial consumption) — a transform on the model
├── evolution/  System(t) → System(t+1): the hierarchical N-body approximation, capture, decay
└── (consumers, elsewhere): renderers, flicker-world epoch sim, game systems
```
Dependencies down only: `model` knows nothing of `formation`/`evolution`; those consume `model`.
Reuses `flicker-materials` (vocabulary), `flicker-worldstate` (`Composition`/ledger). No render dep.

**Use-cases (who calls it):**
- The **solar-system viewer** (today `flicker-solarsystem`) — `formation::form(nebula) -> System`,
  `evolution::step(&mut System, dt)`, then render the `System`.
- **flicker-world / the epoch sim** — consumes a selected `Body` (its composition + fields) as the
  Epoch-1 blueprint, and the surrounding `System` as the celestial context (day/night, seasons,
  eclipses — the day/night model in `celestial.rs` becomes a *consumer/derivation* of the System).
- **Game systems** (future) — the live celestial state captured at "start the epoch sim from this
  body" (the next major integration noted in `flicker-solarsystem-handoff.md`).

---

## 4. Data models (the heart — design these first)

### 4a. `Body` — a physical object with the four fields
A body carries, at minimum, **composition AND gravity AND density AND pressure** (the user was
explicit: gravity/density/pressure *in addition to* composition — required for both objectives §0):

- **Composition** — element-mass distribution (`flicker-worldstate::Composition`, conserved). The
  *what it's made of*. Drives surface color, class (rocky/icy/gas), Epoch-1 seeding.
- **Gravity** — derived from mass (and used relationally between bodies). Drives orbits, capture,
  Roche limits, and surface expression (e.g. band count on gas giants scales with gravity).
- **Density** — bulk / differentiated density (from composition + pressure). Distinguishes
  iron worlds from ice worlds; sets radius from mass.
- **Pressure** — depth/altitude-dependent state driver. The reason a gas giant's interior is
  "effectively a water world" yet its visible surface is atmosphere (§7). Drives phase/material
  expression (composition theory: what a voxel/hex *expresses* = distribution + local conditions).

Derived/bookkept: mass, physical radius, temperature (insolation + internal), orbital elements,
classification (star / planet / dwarf / giant / belt-object / comet).

### 4b. `Satellite` — the recursive tree
A **System** is a **tree**: the **star is the root node**; bodies orbit the star; **bodies may have
satellites**; **moons may have their own (smaller) moons**. The same `Body`+`Satellite` node is the
**one abstract piece that composes systems** at every level — recursion all the way down.

**Satellite classifications** (a body may have **any combination**):
- **Body** (child) — a sub-body on a bound orbit (planet of a star; moon of a planet; submoon of a
  moon). Itself a full `Body` node → recursion.
- **Ring** — a disc of fine material in the body's equatorial-ish plane. **May have gaps** (Cassini
  divisions, shepherd-moon/resonance carved). **May be a Belt** if the material density is
  appropriate (a ring of denser/larger planetesimals reads as a belt).
- **Belt** — a populated annulus of small bodies/planetesimals (asteroid belt, Kuiper belt). A ring
  and a belt are two points on a **density continuum**, not distinct types — model accordingly.
- **Comet** — a high-eccentricity small body on a sweeping orbit (and the eventual source of the
  **motion-line trails** flagged but not built — `flicker-solarsystem-handoff.md`).

**Procedural relationships (these are reality, must have drivers — §0):**
- **Rings ↔ bodies are coupled.** *Rings form bodies* (ring material accretes into moonlets beyond
  the Roche limit) and *bodies form rings* (a moon wandering inside the Roche limit is tidally
  shredded into a ring — already prototyped in `flicker-solarsystem::ring_spec`). The model should
  make this a **two-way procedural relation**, not two unrelated features.
- **Ring gaps** ← resonances / shepherd satellites.
- **Ring ⇄ belt** ← material density crosses a threshold.

### 4c. Node abstraction (compose systems)
One node type (`Body` with an optional set of `Satellite`s, each of which may *be* a `Body`) +
the classification tags = the whole compositional vocabulary. Keep the node **abstract**: formation
and evolution build/transform trees of nodes; renderers and game systems walk them.

---

## 5. Formation — less aggressive consumption (a transform: cloud → System)

**Starting state (keep as-is):** a **random-sized supernova** seeds a nebula with materials
**distributed in a physically normal manner** (condensation-by-radius, snow line, metallicity), and
**bodies form from the detritus**. This is the agreed floor — "no reasonable further simplification
than forming the solar system from debris of a supernova" (user, prior session).

**The change — be fuzzier, consume less:**
- Today we **determine the bodies at the start** (≈ random based on the cloud's available radius)
  and **aggressively consume everything** into bodies. Stop doing that.
- **Fuzzy cloud → body decision.** What turns into a body should be **probabilistic / partial**, not
  a clean tiling. *This is the one genuinely creative/fuzzy step* (the rest is deterministic physics,
  §6) — needs a good model (candidate levers: local surface-density vs a stochastic threshold; only
  a fraction of a feeding zone collapses; leftover stays as cloud/belt).
- **Partial clearing → belts.** A forming body **clears out a portion of cloud** and **leaves the
  rest as a belt/ring**, rather than sweeping its whole band. Belts and rings are first-class
  outputs of formation, not just leftovers.
- **Smaller bodies keep forming** from remaining cloud (a population, not a fixed initial set).
- **Gas giants capture, not just sweep.** A giant should **not gather everything in its gravity
  band at first**; **smaller bodies on similar orbits get captured** as moons. **Big driver: the
  user wants gas giants with *a lot* of moons and isn't seeing it** in the current sim. The fix is
  to extract capture into the evolution model (§6) and let giants accumulate satellites over frames,
  *plus* seed some co-orbital small bodies that become moons.
- **Conserve the populations.** Do not fully consume rings, belts, bodies, *and* comets into planets
  — a finished system should still have all four classes present where physical.

---

## 6. Evolution simulation — a *consumer* of System data

**Principle:** the **evolution simulation is a consumer of system data**; system state is **evolved
through procedural processing of systems**. It is *logic over the model* (§3 layering), not a place
where data lives. The "simple calculations" currently inlined in `sim.rs` move here as clean blocks.

### 6a. The hierarchical, order-of-magnitude N-body approximation (the "three-body trick")
We are effectively asked to handle the three-body problem; the user supplied the trick to make it
tractable and deterministic. **Apply gravity hierarchically and by order of magnitude, per frame:**

1. **Inner before outer** (order-of-magnitude rule): inner bodies are resolved/applied **before**
   outer bodies.
2. **Parent before child**: a parent body's influence is applied **before** its children's.
3. **Compute in frames** (discrete steps), resolving in that hierarchical order.
4. **Equality → flip a coin.** A state of exact equality (a balanced tug — a **Lagrange point**)
   should be **rare**; the continuous math almost never lands on an exact zero. When it does, **flip
   a coin** to break the tie. (Don't special-case Lagrange physics; just break exact ties randomly.)
5. **This determines body capture** — who captures whom resolves from the hierarchical order +
   tie-break, deterministically per seed/frame.

This replaces the softened mutual-gravity N-body integrator in `flicker-solarsystem/sim.rs` with a
**hierarchical pass** that is cheaper, deterministic, and *designed* to produce capture (→ giants
with many moons, §5). The only fuzzy input is the initial cloud→body decision (§5); everything
downstream is this deterministic hierarchy.

### 6b. What evolution produces
- **Capture / loss** of satellites (bodies, comets) between parents — per the hierarchy.
- **Ring ⇄ body ⇄ belt** transitions (§4b) on their own cadences.
- **Surface evolution — later stages.** The **erosion simulation belongs here too** (user
  confirmed): it is just a **later stage of the same system's evolution** — port the Epoch-6 /
  `hex-world` erosion into evolution as a late-stage pattern. **Distinct from the water cycle**,
  which is **ongoing** — used **continuously at runtime**, not only in the initial generation. So
  evolution spans early (capture/clearing) → mid (tectonics, mineralization, erosion) → **ongoing**
  (the water cycle, GC) — each at its own time scale (§6d).
- **Slow decay / GC** consistent with the runtime model (CLAUDE.md §1: static gen → slow live sim →
  GC). Evolution is batch/geological cadence, not per-frame physics for gameplay.

### 6c. Progressive formation patterns (abstract)
Define formation/evolution as **progressive patterns** in the abstract: a small set of
named transforms (e.g. *condense*, *clear-to-belt*, *capture*, *shred-to-ring*, *accrete-from-ring*,
*eject*, *erode*, *water-cycle-tick*) that each take a System (sub)tree and return a transformed one.
Formation (§5) and evolution (§6) are then *sequences/loops* of these patterns over the tree —
reusable, testable blocks, logic fully separated from the node data. **Each pattern declares its
native time scale** (§6d).

### 6d. Time scale — the universal simulation-speed dial (cross-cutting; user-emphasised)
A **universal time-scale dial** governs how fast each phase advances, tuned to **the demands of the
batch (how much state to churn) and the demands of the detail (how fine the result must be)**.
Phases live at wildly different temporal scales, so the dial moves per phase — the **time-step is a
per-phase function, not a constant**:

- **Solar-system formation:** **millions of years** per step — coarse, few bodies, no fine detail →
  run **fast** (huge steps).
- **Tectonics / mineralization / erosion** (mid/late evolution): **slower** — finer surface state.
- **Biome / life evolution:** **thousands of years** per step — fine detail → **slow** time scale.
- **Ongoing runtime (water cycle, GC):** geological **batch cadence**, continuous (§6b, CLAUDE.md §1).

This dial is **the** knob that lets one engine serve *forming a solar system* (Myr/step) and
*evolving a biome* (kyr/step) without re-architecting. It **subsumes** §6a's "compute in frames" (a
frame = one dial-sized step) and the cosmetic time-rescale the solarsystem viewer already fakes
(`RUN_MYR`, `coast_rate`). Make it **explicit and shared**: each pattern (§6c) declares its native
time scale; the runtime picks the dial setting from the batch + detail demands of the moment.

---

## 7. Gas giants — "a solid ball of air" (the visualization contract)

From an outside viewer we see **only the atmosphere**, so render a gas giant as a **"solid" surface
whose material *is* gas** — effectively a solid ball of air. Pressure makes the interior behave like
a fluid/water-world, but **gas motion is far more violent than on rocky worlds**: the **material
system must mark gas/volatile worlds as rotationally volatile**, so the **hex tiles strongly express
swirl** (banded, turbulent, animated). This is a **material-system property** (a per-material/per-
class "volatility / rotational expression"), not a one-off render hack — so both objectives (§0)
hold: the *data* says "gas, high volatility"; the *renderer* expresses that as strong swirl.
(Currently stubbed as the procedural swirl-sphere in `flicker-solarsystem/planet.rs`; the real
version is a gas hex globe whose cells express swirl.) Because it's pure gas with no fine surface,
a giant is **kept rough — pinned to the Mercury hex count (`freq 48`) regardless of its real size**
(§8), rendered large but coarse.

---

## 8. Hex data abstraction — planet-scale macro voxels

**Abstract flicker-world's hex composition into a generic "hexes of data"** structure — cells that
carry data (composition + the §4a fields) and are **rendered as materials**. This is the
**macro-scale version of voxels** from composition theory (CLAUDE.md §2): a **voxel** is a container
for a portion of a *cluster's* material distribution (micro, ~128 ft); a **hex** is the same idea at
**planet scale** (a hex is a container for a portion of a *planet's* material distribution). One
abstraction, two scales.

**Hex count — set by the constant tile size; pin as "potential invariant" / "beta global":**
- **"Grid" = the icosphere `freq`** (the flicker-world HUD control; `cells = 10·freq² + 2`).
- **The 49.6-mi tile is fixed, so `freq` scales with the body's radius** (constant tile size →
  cell count ∝ surface area, `freq ∝ radius`; CLAUDE.md §2). Two **anchor points** from the user:
  **Mercury ≈ `freq 48`** (~23 042 cells), **Earth ≈ `freq 100`** (~100 002 cells). So a solid
  world's hex resolution is the *real* per-tile count for its size — **accurate data**, not a render
  preference. (The app caps the control at 48; rocky worlds bigger than Mercury want higher.)
- **Gas giants are PINNED at the Mercury count — `freq 48` — regardless of their (huge) size.** They
  are **pure-gas simulations** with no fine surface to resolve, so we **keep them rough**: render at
  the giant's true large size but with only ~Mercury's hex budget (coarse per-area — fine for a
  swirling ball). Do **not** scale a giant's freq up to its radius (a Jupiter at the 49.6-mi tile
  would be an absurd cell count, and the detail is wasted on monotonous gas). This is the §7
  "solid ball of air" rule made concrete: `HEX_FREQ_GIANT = 48`, fixed.
- **Constants/rule to pin:** rocky/icy `freq = hex_freq_for_radius(r)` anchored at **Mercury 48 /
  Earth 100** (`freq ∝ radius`); **`HEX_FREQ_GIANT = 48`** (fixed). A "potential invariant" / "beta
  global" in the spirit of `clayengine`'s `CLUSTER_DIM`/`MAX_LOD`. Home: `clayengine` or a
  `flicker-celestial` constants module. Confirm the radius→freq curve before pinning.
- **Separate concern — viewer render LOD.** The above is the **data** budget (game-accurate). The
  **solar-system viewer** drawing many planets at once can't render every one at full data res
  (~1 M cells/frame); it should render a **coarser LOD** (a stride over the hex data, à la the voxel
  LOD), independent of the stored hex count. (Today's `flicker-solarsystem` globe uses a flat
  placeholder `GLOBE_FREQ = 9`.)

---

## 9. Renderer responsibilities (consumes System + Body data)

Rendering is **separate** and **consumes** the model:
- **Orbits, rings, belts, comet paths** — **circles, planar lines, and projections** of these paths
  are drawn by the **renderer from the system/body data** (eccentric ellipses from orbital elements,
  ring annuli from ring data + gaps, belt scatter from belt data, comet trails from comet paths).
  The data model describes them; the renderer projects them.
- **Composed globes** (rocky hex worlds, gas swirling balls) — built from the hex-data abstraction
  (§8) + composition, lit by the **star point light** (already in `flicker-render`).
- Keep the **art-vs-reality** split (§0): camera (the comet camera), glow, and dust are art;
  the bodies/rings/belts/orbits are reality projected from data.

The current `flicker-solarsystem/scene.rs` already does much of this (orbit ellipses, ring meshes,
the globe cache, the point light) — that rendering logic stays in the *viewer*, consuming the new
`flicker-celestial` model instead of the example's ad-hoc `Body`/`sim` types.

---

## 10. Refactor / migration plan (sequencing for the new session)

1. **Create `flicker-celestial`** (lib, no GPU dep). Workspace member + `[workspace.dependencies]`.
2. **Data models first (§4).** `Body` (composition + gravity + density + pressure), `Satellite`
   enum (`Body | Ring | Belt | Comet`), `System` tree, classification. Unit-test the tree + field
   derivations. *Nothing else until the model is clean.*
3. **Hex-data abstraction (§8).** Generic hexes-of-data; pin the hex-budget constants; extract
   flicker-world's `globe`/`color` into a reusable lib (kill the `worldglobe.rs` duplication).
4. **Formation block (§5).** Port the supernova/disk/condensation seeding; replace aggressive
   consumption with the fuzzy/partial model; emit belts/rings/comets, not just planets.
5. **Evolution block (§6).** Implement the hierarchical order-of-magnitude N-body + coin-flip +
   capture, replacing the softened integrator. Tune until **gas giants accumulate many moons**.
6. **Re-home the viewer.** Point `flicker-solarsystem` (and `flicker-world`'s celestial context) at
   `flicker-celestial`; delete the example's private `body/disk/sim/collide/habitability`.
7. **Discard defunct branches (§2).** Delete `examples/hex-map` + `docs/hex-map-handoff.md`; retire
   `examples/hex-world` after extracting `layers.rs`.
8. **Water-cycle bridge (§2 re-home).** Promote `hex-world/layers.rs` to a real module seeded from
   Epoch 4; this is the start of bridging the generative sim ↔ atmospheric/water-cycle sim.

Sequence 1–2 are the foundation; 3–6 bring functional blocks in; 7–8 are cleanup + the next bridge.

---

## 11. Open / creative questions (flag for the design session)

- **The fuzzy cloud→body decision (§5)** is the one place to be creative — the rest is deterministic
  (§6). Needs a model that yields varied, *partial* consumption (belts + rings + comets survive).
- **Gas-giant moon abundance (§5/§6)** — validate the new capture model actually produces
  moon-rich giants (the current driver of dissatisfaction).
- **Hex-budget constants (§8)** — confirm the **radius→freq curve** (anchored Mercury `48` / Earth
  `100`, from the 49.6-mi tile), **`HEX_FREQ_GIANT = 48`** (giants pinned rough), and a separate
  viewer **render-LOD** (coarser than the stored data); pin as invariants.
- **The universal time-scale dial (§6d)** — design the shared API: each pattern declares a native
  time scale; the runtime picks the dial from batch + detail demands (Myr for formation → kyr for
  biomes → ongoing for the water cycle). This is the knob that lets one engine serve every phase.
- **Generative ↔ water-cycle bridge (§2/§6b)** — erosion → a late evolution stage; the water cycle →
  an **ongoing** runtime system seeded from Epoch-4 (`layers.rs`); what's left per
  `docs/water-cycle-handoff.md` / `epoch-data-audit-handoff.md`.
- **flicker-world lib extraction** — decide the home for the shared hex globe/color (this crate vs a
  dedicated render-helper) so nothing reimplements it.

---

## 12. Relationship to existing crates & docs

- **Reuses:** `flicker-materials` (vocabulary), `flicker-worldstate` (`Composition`/ledger),
  `flicker-worldgrid` (icosphere topology), `flicker-worldgen` (epochs — the *consumer* of a body's
  composition as Epoch-1 blueprint), `clayengine` (constants home).
- **Feeds:** the viewer(s) and the epoch sim (a selected `Body` → Epoch-1 blueprint; the `System` →
  celestial context).
- **Supersedes / absorbs:** the ad-hoc celestial logic in `flicker-solarsystem` (harvest),
  `flicker-world/celestial.rs` (becomes a consumer/derivation), `voxel-cluster`'s day/night sim
  (origin of the celestial model — `celestial-import-flicker-world` memory).
- **Docs to read:** `flicker-solarsystem-handoff.md`, `clayengine_world_generation_spec_v2.md`,
  `material-model-handoff.md`, `water-cycle-handoff.md`, `hex-sphere-handoff.md`,
  `flicker-world-handoff.md`. **Mark superseded:** `hex-map-handoff.md` (discard with the crate).
</content>
