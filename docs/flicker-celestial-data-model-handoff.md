# Handoff — `flicker-celestial` data model (refactor Slice 1)

**Status:** **LANDED.** The new `flicker-celestial` lib crate exists and its `model`
module — the data classes the whole refactor sits on — is built and unit-tested
(21 tests, clippy clean). This is **§10 steps 1–2** of `docs/flicker-celestial-spec.md`
("Data models first … *nothing else until the model is clean*"). Formation (§5),
evolution (§6), the hex-data abstraction (§8) and re-homing the viewers (§10.6) are
**not** started — they consume this model.

Read first: `docs/flicker-celestial-spec.md` (the design of record), then this doc,
then the code under `crates/flicker-celestial/src/`.

---

## 1. What landed

A new workspace crate `crates/flicker-celestial` (lib, **no GPU dep**; added to
`[workspace.dependencies]`, **not** the umbrella — the umbrella only re-exports the
*engine* crates, matching `flicker-worldgen`/`worldstate`). Layout:

```
crates/flicker-celestial/src/
├── lib.rs              crate docs + re-exports
├── units.rs           AU·yr·M☉ orbital convention (G=4π²) + SI/CGS conversions
├── model/
│   ├── mod.rs
│   ├── condensation.rs CondensationClass (5) + ClassComposition (the physics view)
│   ├── cloud.rs        Cloud / CloudRing — the material reservoir (pure accounting)
│   ├── body.rs         Body (the recursive node + the four fields) + BodyKind
│   ├── satellite.rs    Satellite enum + Disc (ring↔belt continuum)
│   └── system.rs       System (tree wrapper) + Classification (IAU, derived)
└── formation/
    ├── mod.rs          materialize_cloud (analytic field → conserved Cloud)
    └── disk.rs         Nebula + the analytic disk field (ported from the POC)
```

### The node: `Body`
A recursive tree node. **Stored:** `pos`/`vel` (kinematic state **in the parent's
frame** — heliocentric for a planet, planetocentric for a moon; root star at rest at
origin), `kind: BodyKind` (intrinsic formation type: `Star`/`Protoplanet`/`Giant`/
`Debris`), the two compositions (below), and `satellites: Vec<Satellite>`.

**The four physical fields (spec §4a)** — composition is stored; gravity/density/
pressure are first-class *derivations*:
- **Composition** — see §2 (the two-composition decision).
- **Density** — `density_g_cm3()`, from the class breakdown's material densities.
- **Gravity** — `surface_gravity_si()` (`G·M/R²`, SI m/s², for surface expression);
  orbital/relational gravity uses the AU/yr/M☉ system via `mu(parent_mass)` /
  `orbital_elements(parent_mass)`.
- **Pressure** — `central_pressure_gpa()` (uniform-sphere hydrostatic estimate
  `3GM²/8πR⁴`; the driver for the gas-giant "fluid interior" contract, spec §7).

Orbital math is **μ-parametrised** (`mu`, `is_bound`, `orbital_elements`, `period`
all take `parent_mass`) so the *same* code serves every level of the tree — this is
the generalisation of the formation sim's `M_STAR`-hardcoded math to the recursion.

### The reservoir: `Cloud` (+ `CloudRing`) — pure accounting
The system holds the material; a body grows by **absorbing** it (the user's
clarification — see §2). `Cloud` is the conserved reservoir that makes that
accounting real: a set of concentric `CloudRing`s, each a `ClassComposition`.
**It is not a visual object** — the dust cloud's *look* is the renderer's artful
volumetric pass; this is "pure data accounting, only the formed bodies are
visualised" (user). Rings (radial bins) are used because the *accounting* needs
radial resolution (condensation class is set by disk radius; feeding zones are
radial bands), not for any look. Transfer primitive: `Cloud::draw_band(inner,
outer, mass)` removes material proportionally from the overlapping rings and returns
it (the *removed-from-cloud* half); `Body::absorb(&drawn)` adds it (the
*inserted-into-body* half). Conservation is exact — tested.

The **materialisation** (turning the analytic disk field into rings) and the
**feeding policy** (which body draws how much) are formation concerns (spec §5) that
will populate and drive this reservoir; the data class supplies the conserved
substrate + transfer they build on.

### Formation: the materialisation (Slice 2 — LANDED)
`formation::materialize_cloud(nebula, n_rings) -> Cloud` turns the analytic disk
field into the conserved reservoir — **the user's "key point": statistical → actual
distribution, so body growth is accounted correctly.** `formation::disk` ports the
POC's physics onto the model: `Nebula` (supernova_size → `sigma_1au` + metallicity),
`solid_surface_density(r)` (`r^-3/2` with the snow-line ice jump), `composition_fractions(r)`
(condensation sequence), `annulus_solid_mass`, `class_composition_at`. The
materialisation splits `[DISK_INNER, DISK_OUTER]` into `n_rings` annuli, each given the
solid mass it actually holds, classed by its midpoint composition. **Total cloud mass =
the disk's integrated solids** (tested, <1% of the analytic integral); inner rings are
dry rock+metal, outer rings ice-dominated. Gas is a **separate** reservoir
(`Nebula::disk_gas_mass`) — an envelope captured by giants, not a solid in the cloud.

This replaces the POC's broken accounting (embryos *integrated out of* an analytic
field that never lost the mass). Now: materialise → bodies `absorb` from it
(`Cloud::draw_band` → `Body::absorb`) → conserved, with a remainder. An end-to-end
conservation test proves `cloud_total + body = before`. 31 tests, clippy clean.

**Deliberately deferred: body SEEDING + the consumption model.** *How* bodies are
seeded and *how much* of the cloud they consume is the one genuinely creative call
(spec §5/§11): a full sweep leaves no remainder; a *partial*/probabilistic consumption
leaves belts/rings/comets. That decision is the user's to drive, so seeding is the next
step — the materialisation + transfer primitive are the foundation it builds on.

### The tree edge: `Satellite` (+ `Disc`)
The spec names four satellite *classifications* (body, ring, belt, comet) but says to
**model the continuums, not the labels** — so there are only **two variants**:
- `Satellite::Body(Body)` — a discrete child (moon, submoon, **or comet**; comet is a
  body with a high-eccentricity orbit, not a separate type).
- `Satellite::Disc(Disc)` — an annulus of material (**ring or belt**: one structure on
  a surface-density continuum; `Disc::class()` reads ring vs belt, `Disc::gaps`
  carries resonance/shepherd-carved lanes).

The four classifications are recovered as **derived tags**: `Classification`
(moon/comet/planet/dwarf from the orbit + tree) and `DiscClass` (ring/belt from
surface density).

### The wrapper: `System`
`System { star: Body }` + tree utilities: `for_each_body(|body, parent_mass, depth|)`
(hands each body the parent mass the orbital math needs), `total_mass`,
`count_bodies`, `solar_bodies`, `classify_solar_body`. IAU planet-vs-dwarf clearing
(`cleared_neighborhood`) and `classify(...)` are ported from the formation sim and
generalised to "siblings of a shared parent".

---

## 2. The load-bearing decision: **store BOTH compositions; one drives the other; the cloud drives the body**

This was the one genuine fork (surfaced to and decided by the user). The concrete
problem: the spec (§4a) wants `Body.composition` to be the conserved **element**
vector (`flicker_worldstate::Composition`) *and* wants density/radius/pressure
derived from it — but those conflict, because the Prism periodic table stores
**elemental** densities (oxygen is a *gas* at 0.0014 g/cm³), while a planet's oxygen
is bound in silicate **rock** (~3 g/cm³). A bulk density off the raw element vector is
physically wrong. The reverse map (element → condensation class) is also ambiguous (O
lives in silicate **and** ice **and** carbon), so the rock-vs-ice info density needs
can't be recovered once flattened to elements.

**Decision (user): a `Body` carries both.**
- `classes: ClassComposition` — the **condensation-class** breakdown (Metal/Silicate/
  Carbon/Ice/Gas, each with a *material* bulk density). The **physics truth** the four
  fields derive from.
- `composition: flicker_worldstate::Composition` — the conserved **element** vector
  (keyed by atomic number). The ledger / Epoch-1 currency.

**They are not two independent copies "kept in sync" — one drives the other** (user's
correction). A body **grows by absorbing classed material**; every deposit/absorption
adds class mass and, with it, the element projection of that mass
(`ClassComposition::to_element_composition`, "class drives elements"). All material
enters through `Body::absorb` / `Body::deposit` / `Body::strip` (collision peeling,
outermost-first); `pos`/`vel`/`kind` are poked freely, the compositions never
directly — so they can't disagree. A test asserts the totals track through deposits
and a strip.

**The cloud→body axis (user's reframe).** The deeper model: the **system** (cloud)
holds the material; the **body** (planet) aggregates it *by absorbing the cloud* —
and that absorption *is* the band-clearing. The accounting is a conservation
transfer — **removed from cloud, inserted into body** — not a sync. So the two body
compositions are legitimate because both accumulate from the *same real transfers*
out of the cloud (whose rings carry classed material by condensation radius). See the
`Cloud` reservoir (§1) — `worldstate::Composition::remove → add` is the transfer
primitive at the element level; `ClassComposition::take_mass` / `Cloud::draw_band` at
the class level.

The class→element makeup (`CondensationClass::element_makeup`) is ported from the
formation sim's `CLASS_ELEMENTS` but **keyed by `ElementId` (atomic number)** — so the
bridge needs no symbol round-trip and the crate threads **no `Tables`** through any
derivation. (A test still asserts every makeup element is a Prism element.)

> **The key gap the user flagged.** Today the old `disk.rs` is a purely *analytic*
> field (`solid_surface_density(r) × composition_fractions(r)`); embryos are seeded by
> *integrating it over a feeding zone*, so nothing the cloud actually *loses* — there
> is no conserved reservoir, hence belts/leftovers/conservation aren't real (only the
> giants' gas budget is tracked). **The key next step is materialising that
> statistical field into the actual, conserved `Cloud`** so body growth is accounted
> correctly (and partial clearing can leave a remainder → belts, spec §5). Formation
> deposits *by class* (the condensation sequence), so the element vector fills
> correctly and `to_epoch1_abundance` becomes "normalise `composition`".

---

## 3. Units (`units.rs`)

Orbital mechanics in **AU · year · solar-mass**, `G = 4π²` (1 AU around 1 M☉ = 1 yr) —
the formation sim's convention, `f64` throughout. The **central star's mass is NOT a
constant** (it is `System::star_mass()`); only the unit conversions are fixed
(`M_SUN_G`, `M_SUN_KG`, `AU_CM`, `AU_M`, `G_SI`, `M_EARTH`). Physical surface
quantities report in SI/CGS with explicit suffixes (`_si` m/s², g/cm³, GPa).

---

## 4. Validation

`cargo test -p flicker-celestial` → **21 passing**; `cargo clippy -p flicker-celestial
--all-targets` clean. Coverage: composition conservation + class↔element projection +
strip ordering; Earth-like radius/gravity/pressure land near reality; iron world
denser/smaller than ice; circular/eccentric/hyperbolic orbital elements; disc
surface-density continuum + gaps; tree walk parent-mass threading; IAU planet-vs-dwarf,
giant, comet, moon classification. (`flicker-celestial` is purely additive — nothing
depends on it yet, so the rest of the workspace is unaffected.)

---

## 5. Deferred / next (consumers of this model — do NOT build until directed)

- **Hex-data abstraction (spec §8, step 3)** — generic "hexes of data"; pin the
  hex-budget constants (Mercury `freq 48` / Earth `100`; `HEX_FREQ_GIANT = 48`);
  extract `flicker-world`'s `globe`/`color` into a reusable lib (kill the
  `worldglobe.rs` duplication).
- **Formation block (spec §5, step 4)** — **materialisation DONE** (§"Formation"
  above: `formation::materialize_cloud`). **Next: body seeding + the consumption
  model** — the one creative call (spec §11): seed bodies (generatively?) that
  `absorb` from the cloud, with *partial* consumption leaving a conserved remainder →
  belts/rings/comets, and gas giants that *capture* many moons (drawing the gas from
  `Nebula::disk_gas_mass`). The full-sweep-vs-partial decision is the user's to drive.
  Then the N-body / collision physics (`flicker-solarsystem/{sim,collide}.rs`).
- **Evolution block (spec §6, step 5)** — the hierarchical order-of-magnitude N-body +
  coin-flip-on-equality + capture; the universal time-scale dial (§6d).
- **Re-home the viewers (step 6)** — point `flicker-solarsystem` and
  `flicker-world/celestial.rs` at this model; delete the example's private
  `body`/`material`/`sim`/`collide`/`habitability`.
- **Discard (step 7)** — `examples/hex-map` + `docs/hex-map-handoff.md`; retire
  `hex-world` after extracting `layers.rs`.

### Viewer (`flicker-solarsystem`) — baked "fake fixed light" removed from `planet.rs`
The slice-2 surface bake (`planet.rs`) modulated each body's **albedo by a brightness
term** — a world-fixed gradient that read as a *fixed light not matching the star* (it
fought the point-light terminator). Both instances now removed so the engine star **point
light is the sole source of light/dark** (matching the settled-world hex globes):
- `gas_surface`: dropped `shade = 0.78 + 0.22*bands` — bands are now warm/cool *albedo* only.
- `rocky_surface`: dropped the fbm brightness gradient `base*(1 + 0.45*land)` (its dominant
  octave is sub-one-cycle → a smooth world-fixed gradient = the "above-right fake light" the
  user flagged); `fbm` removed. Kept the *albedo* features (ice caps, metallic cast). The
  procedural continents went with the gradient — re-introduce as albedo **colour** (or via
  the hex-globe path) if wanted. **User-confirmed fixed** (terminators now track the star).
- **Gas giants migrated to the hex globe** (spec §7 "solid ball of air"). Before, only
  settled *non-gaseous* worlds resolved to a hex globe (`!d.gaseous && self.coasting`); gas
  giants always fell through to the procedural swirl-sphere — the "not migrated to the hex
  model" the user flagged. Now a settled giant renders as `worldglobe::GasGlobe`: the same
  flicker-world icosphere geometry as the rocky globes, but cells coloured by the gas
  composition + swirling bands (reusing `planet::gas_surface`). Geometry is built **once**
  (`GasGlobe::new`, cached on the scene); only the per-cell colour is recomputed per frame
  (`GasGlobe::shade`) so the bands keep swirling without rebuilding the icosphere. Lit by the
  star point light like everything else. Viewer render-LOD = `GLOBE_FREQ` (9), same as rocky
  (the spec's `freq 48` pin is the *data* budget, not this viewer LOD). NOTE: settled rocky/
  icy/dwarf worlds were already on the hex globe; only gas giants were the gap. (Blind change;
  the current default seed has **no** gas giants — dial a heavier disk `[`/`]` or reseed `R`
  to get one to verify.)
- **Hex count now scales with body size** (spec §8 — pinned as an invariant in
  `flicker-celestial::hex`). New `hex_freq_for_radius(r_au)` (line through **Mercury≈48 /
  Earth≈100**, clamped `[12,100]`) + `HEX_FREQ_GIANT = 48`. The viewer was rendering every
  globe at a flat `GLOBE_FREQ = 9`; now each `Draw` carries a `hex_freq`: a **solid** world
  scales it with its real `physical_radius`, a **gas giant** is pinned/capped at 48 (the
  "scale only the giants" rule — a giant renders *large* but stays *coarse*). The cache key is
  unaffected (freq is a function of composition→radius, which the key already hashes).
  **PERF NOTE (user is the judge):** this is a big cell-count jump — freq 9 = 812 cells, but
  48 = 23 042 and 100 = 100 002. Rocky globes are cached (one-time build hitch when a world
  first resolves); the gas globe is rebuilt per frame at 48 (heavier with many giants). If it
  hitches, the spec sanctions a coarser *viewer LOD* (a stride/divisor over these data-budget
  freqs) — the constants stay the accurate data, the viewer just renders fewer.

### Open knobs left as documented placeholders (confirm/tune when formation feeds them)
- `RING_BELT_SURFACE_DENSITY` (M☉/AU² ring↔belt threshold) — placeholder.
- `COMET_MIN_ECCENTRICITY` (0.4) — comet classification threshold — placeholder.
- `central_pressure_gpa` is a uniform-sphere estimate; a depth-resolved profile is a
  later refinement.
- Serde is **not** derived on the spatial types yet (glam's `serde` feature is off);
  add when "capture the celestial state for the game" is built. `Composition` is
  already serde.
