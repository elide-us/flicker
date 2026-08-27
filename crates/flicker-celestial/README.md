# flicker-celestial

A GPU-free library of **celestial mechanics and planet-formation data models**: a star
system as a recursive tree of bodies and material discs, each body carrying a conserved
composition from which its density, radius, gravity and pressure derive; the analytic
protoplanetary disk and its materialisation into a conserved material cloud; and a per-body
surface-evolution stepper. Pure data + logic — no rendering, no input, no window.

> ⚠️ **Status: superseded reference POC — do not build on it.** This crate is **not the
> design of record** and **nothing in the workspace consumes it** (it is a `crates/`
> reference crate, not part of the shipping `Alpha/` build). It remains a
> `[workspace.dependencies]` entry so it keeps compiling, but the live system-formation /
> celestial lineage moved on — see `flicker-system` (whose `Cargo.toml` notes *"NOT
> flicker-celestial"*) and the sky/orrery consumers `flicker-orrery` + `flicker-pocclusters`.
> Read this crate as history and as a source of reusable celestial math, not as a foundation
> to extend. The code is clean and green (see [Gates](#gates)); the "next slice" language in
> the source doc-comments is aspirational and did not land.

> Design of record — why it is shaped this way, why it was set aside, decisions and history —
> lives in the project's MCP memory, not here. This file documents how to use the crate as it
> stands.

A quick vocabulary, since these are project words:

- **Body** — one physical object (star, protoplanet, gas giant, debris) and a node in the
  system tree; owns its satellites.
- **Satellite** — a tree edge under a body: either a child **Body** (moon / submoon / comet)
  or a **Disc** (ring / belt).
- **System** — a whole star system = its root **Body** (the star) plus everything hanging off it.
- **Composition** — a conserved mass distribution. This crate keeps two views of it per body:
  the **condensation-class** breakdown (`ClassComposition` — the physics truth) and the
  **element** vector (`flicker_worldstate::Composition` — the ledger). See
  [the one concept](#the-one-concept-two-compositions-one-truth).
- **Cloud** — the system's leftover material reservoir, as pure accounting (never rendered).
- **HexWorld** — a body's surface stored as a grid of per-cell composition (the planet-scale
  "macro-voxel"); what evolution steps over.

## Where it sits

- **Builds on:**
  - `flicker-worldstate` — the conserved element `Composition` (keyed by atomic number) that
    `Body`, `HexWorld` and the class→element projection are expressed in.
  - `flicker-materials` — `ElementId` (atomic number) and the Prism element tables; the
    per-class element makeup is asserted against those tables in a test.
  - `glam` — `DVec3` (f64) for orbital state; `Vec3` (f32) for evolution topology.
  - `serde` — only `HexWorld` derives it (so an evolved planet's grid can be captured).
- **Used by:** nothing (see the status banner). It is a standalone reference library.
- **Reads from the content tree:** nothing at runtime. One test (`every_makeup_element_is_in_prism`)
  loads `Alpha/content/data` through `flicker-materials` to validate the hardcoded element makeup;
  the library itself performs no file I/O.

## The one concept: two compositions, one truth

Every `Body` carries its material **twice**, and this is the crate's single load-bearing idea:

- `Body::classes` — a `ClassComposition`: mass per **condensation class** (Metal, Silicate,
  Carbon, Ice, Gas). This is the **physics truth**. Density, radius, gravity and pressure all
  derive from it, because each class has a correct *material* bulk density (rock ≈ 3.3, iron
  ≈ 7.9 g/cm³).
- `Body::composition` — a `flicker_worldstate::Composition`: mass per **element** (by atomic
  number). This is the **ledger** (the same currency world-gen / Epoch-1 consume).

They are **not two independent copies**. The class breakdown *drives* the element vector:
every mutator (`deposit`, `deposit_composition`, `absorb`, `strip`) re-projects classes →
elements (`ClassComposition::to_element_composition`), so the two can never disagree and
`classes.total() == composition.total()` always. The direction matters: class→element is
exact, but element→class is ambiguous (a planet's oxygen is rock-bound at ~3 g/cm³, not the
free-gas 0.0014 g/cm³ the periodic table lists), which is exactly why both are kept rather
than one recovered from the other. **Only ever change a body's material through those four
mutators** — writing `classes` or `composition` directly would break the invariant.

## Public API

Re-exported at the crate root (`flicker_celestial::…`): `Body`, `BodyKind`, `Satellite`,
`Disc`, `DiscClass`, `DiscGap`, `System`, `Classification`, `Cloud`, `CloudRing`,
`ClassComposition`, `CondensationClass`, `HexWorld`, `BodyEvolution`, `Stage`. Everything else
is reachable through its module (`model::`, `formation::`, `units::`, `hex::`).

### Model — the data (`model::`)

| Item | What it is | The one thing to know |
|---|---|---|
| `Body` | A system-tree node: `pos`/`vel`, `kind`, the two compositions, `satellites` | `pos`/`vel` are in the **parent's frame** (heliocentric for a planet, planetocentric for a moon); the root star is at the origin, at rest |
| `BodyKind` | `Star` / `Protoplanet` / `Giant` / `Debris` — what a body *physically is* (formation outcome) | Intrinsic and stored; the IAU label is separate and *derived* (`Classification`) |
| `Body::deposit` / `deposit_composition` / `absorb` / `strip` | The only ways to change a body's material | Each keeps the two compositions in sync; `strip` peels **outermost-first** (gas → ice → … → metal core) |
| `Body::mass` / `density_g_cm3` / `physical_radius` / `surface_gravity_si` / `central_pressure_gpa` | The "four fields" (composition + the three derived) | Radius/density/pressure use **M☉ + CGS/SI**; return `0.0` for an empty/unresolvable body. `surface_gravity_si` is m/s², `central_pressure_gpa` a uniform-sphere *estimate* |
| `Body::mu` / `specific_energy` / `is_bound` / `orbital_elements` / `period` / `orbital_radius` | Two-body orbital state about a parent | All take `parent_mass` (M☉) — the crate never stores it. `orbital_elements` returns **`(a, e)`** only; `period` is `None` when unbound |
| `Satellite` | `Body(Body)` or `Disc(Disc)` — the tree edge | Two variants model two continuums: moon/submoon/**comet** are all `Body` (comet is a derived orbit tag, not a variant); ring/**belt** are both `Disc` |
| `Disc`, `DiscClass`, `DiscGap` | A material annulus (ring or belt) with optional cleared `gaps` | Ring vs belt is read off **surface density** (`Disc::class`), a continuum — not stored |
| `System` | The star as root + tree walks | `for_each_body(|body, parent_mass, depth|)` hands each body the parent mass the orbital methods need — the intended way to compute orbits for the whole tree |
| `Classification` | `Star`/`GasGiant`/`Planet`/`DwarfPlanet`/`Moon`/`Comet`/`Debris` — the *contextual* IAU label | **Derived from the tree, never stored** (the same body reads as `Planet` alone and `DwarfPlanet` in a crowd) — via `model::classify` / `model::cleared_neighborhood` |
| `CondensationClass` | The 5 classes, ordered densest→least (`ALL`) | `density()`, `label()`, `color()`, `element_makeup()`, `index()` — the class knobs. `ALL` order **is** the core→envelope layering and the strip order |
| `ClassComposition` | Mass per class (`[f64; 5]`), the physics view | `take_mass` / `take_fraction` / `strip_outermost` are **conserved draws** (removed + remainder = original); `to_element_composition` is the exact class→element bridge |
| `Cloud`, `CloudRing` | The system's material reservoir as concentric rings | `draw_band(inner, outer, amount)` is the *remove-from-cloud* half of accretion → hand the result to `Body::absorb`; draws **clamp to what's there** (never over-draw) |
| `HexWorld` | A body's surface = `Vec<Composition>` at an icosphere `freq` | Storage only — **topology lives elsewhere**; `transfer(from, to, el, amt)` is the conserved transport primitive; the only serde-capable type |

### Formation — nebula → conserved cloud (`formation::`)

| Item | What it is | The one thing to know |
|---|---|---|
| `Nebula` | A system's initial conditions from one dial, `supernova_size ∈ [0,1]` | `new(seed, size)` sets `sigma_1au` + `metallicity`; `solid_sigma()` is the solids actually available; `disk_gas_mass()` is a **separate** gas reservoir (giant envelopes, not a solid in the cloud) |
| `materialize_cloud(nebula, n_rings)` / `materialize_cloud_default` | Discretise the analytic disk into a conserved `Cloud` | This is the crate's "key point": it turns the *statistical* field into the *actual* distribution, so body growth can be conservation-accounted. Total cloud mass ≈ integrated disk solids (tested < 1%) |
| `solid_surface_density(r, sigma_1au)`, `annulus_solid_mass(r0, r1, sigma_1au)`, `composition_fractions(r)`, `class_composition_at(r, mass)` | The analytic disk physics (all in **AU**) | `r^-3/2` power law with the **snow-line jump** (~2.7 AU); inner = dry rock+metal, outer = ice-dominated |
| `DISK_INNER` (0.3), `DISK_OUTER` (15.0), `SNOW_LINE` (2.7), `DEFAULT_CLOUD_RINGS` (64) | The disk extent + accounting granularity | AU; `DEFAULT_CLOUD_RINGS` is accounting resolution, **not** a visual parameter |
| `random_supernova(seed)`, `Rng` | Deterministic splitmix64 RNG + a default supernova size | Same seed → same system |

> **Not built (and it did not land):** body **seeding** and the cloud **consumption model** —
> *how* bodies form and *how much* cloud each sweeps up — were the deliberately-deferred
> creative call. The transfer primitive they would have used (`Cloud::draw_band` → `Body::absorb`)
> exists and is tested; the seeding step above it does not.

### Evolution — step a body's surface (`evolution::`)

| Item | What it is | The one thing to know |
|---|---|---|
| `BodyEvolution` | Per-body sim state: `world` (grid), `stage`, `age_myr` | `step(dt_myr, ctx)` transports material and ages the body; **total mass is invariant** by construction |
| `Stage` | `Aggregation → Stratification → Tectonics → Hydrosphere → Mineralization → Biosphere` | The world-gen epochs as per-body stages; `next()` walks them. **The gate that promotes a stage did not land** — `stage` never advances on its own |
| `step_world(world, dt_myr, ctx)` / `advect_zonal` | The pure stepper, **dispatched by composition** | A gas-dominated world (H+He > ½ mass) runs zonal advection; a **solid world holds unchanged** (its density-sort step did not land) — so on a rocky world `step_world` is a no-op |
| `StepCtx { dirs, neighbors }` | The topology a step reads (parallel to the grid's cells) | Supplied by the **caller's** icosphere — the crate stays topology-agnostic; a step never owns the mesh |

### Units & hex budget (`units::`, `hex::`)

| Item | What it is | The one thing to know |
|---|---|---|
| `units::G` (4π²) | Gravitational constant in **AU · yr · M☉** | The orbital-math constant; `G·M☉ = 4π²` so a 1 AU / 1 M☉ orbit takes exactly 1 year |
| `units::{M_SUN_G, M_SUN_KG, M_EARTH, AU_CM, AU_M, G_SI, EARTH_GRAVITY_SI}` | Unit conversions | Orbital math is AU/yr/M☉; physical fields switch to CGS/SI (`_si` suffixes). Mass is **M☉ everywhere** |
| `units::earth_masses(m_sun)` | M☉ → M⊕ display helper | Reporting only |
| `hex::hex_freq_for_radius(r_au)` / `hex_freq_for_giant(r_au)` | A body's macro-voxel resolution from its radius | `freq ∝ radius`, anchored Earth = 100, clamped `[12, 100]`; a giant uses **half** a solid's count |

## Interactions

None. This is a pure, GPU-free library: no input **signals**, no per-frame **Model**, no
runtime content-tree reads, no threads. It exposes plain types and functions; a consumer
(a renderer, a sim driver, a worker) owns the frame loop, the mesh topology, and any I/O.
The `step_world` / `advect_zonal` functions are deliberately **pure** so a caller can run them
off-thread on the last completed grid.

## Gates

44 tests, all green (`cargo test -p flicker-celestial`). The contract-defining ones:

- **Conservation is the spine.** `model::body::…::deposit_and_strip_keep_the_two_compositions_in_sync`,
  `mass_is_the_composition_total`; `condensation::…::element_projection_conserves_class_mass`,
  `merge_conserves_mass`, `strip_removes_outermost_first`; `cloud::…::drawing_into_a_body_conserves_mass`,
  `a_draw_larger_than_the_band_takes_only_what_is_there`; `formation::…::a_body_absorbing_from_the_cloud_conserves_total_mass`,
  `materialised_cloud_conserves_the_disk_solids`; `world::…::transfer_moves_material_and_conserves_total`;
  `advect::…::advection_conserves_total_mass_over_many_steps`; `evolution::…::fresh_body_starts_aggregated_and_ages_conserving_mass`.
- **Physics lands near reality.** `body::…::earth_radius_and_gravity_land_near_reality`,
  `iron_world_is_denser_than_an_ice_world`, `circular_orbit_recovers_its_radius_and_period`,
  `unbound_body_reads_as_unbound`.
- **Classification from the tree.** `system::…::lone_planet_clears_its_lane_but_a_crowd_does_not`,
  `giant_classifies_as_a_giant_and_clears_by_definition`, `eccentric_small_body_reads_as_a_comet`,
  `a_bound_child_of_a_planet_is_a_moon`, `tree_walk_counts_bodies_and_passes_parent_mass`.
- **Disk shape / disc continuum.** `disk::…::composition_is_dry_inside_and_icy_outside`,
  `surface_density_jumps_across_the_snow_line`, `supernova_sets_disk_mass_and_metallicity`;
  `satellite::…::class_follows_the_density_threshold`, `gaps_reduce_area_and_raise_surface_density`.
- **Evolution dispatch + transport.** `evolution::…::dispatch_advects_gas_but_holds_solid`,
  `advect::…::material_moves_downstream_eastward_not_upstream`, `equator_shears_faster_than_high_latitude`,
  `poles_do_not_advect`.
- **Element contract.** `condensation::…::every_makeup_element_is_in_prism` — every atomic number
  a class emits must be a Prism element (guards the coupling below).
- **Persistence.** `world::…::serde_round_trip_captures_state`.

## Sharp edges

- **Nothing advances a `Stage`.** `Stage::next` exists and `BodyEvolution::step` ages the body,
  but the emergent gate that would promote `stage` did not land — `stage` stays where you set it.
- **`step_world` is a no-op on solid worlds.** Only gas-dominated worlds (H+He > ½ total mass)
  transport; a rocky world's density-sort step did not land, so `step_world` returns a clone.
  This is *intended* emergent behaviour ("a body doesn't run a transform its data doesn't call
  for"), not a bug — but it surprises if you expect a rocky planet to evolve.
- **Mass unit is M☉ everywhere.** `ClassComposition::volume_cm3` (and thus every density/radius/
  gravity/pressure derivation) bakes in `M_SUN_G`; feeding it non-solar masses yields wrong
  volumes. `central_pressure_gpa` is a uniform-sphere *estimate* (Earth ≈ 360 GPa), not a
  depth-resolved profile.
- **Empty / degenerate inputs return zero, silently.** `physical_radius`, `density_g_cm3`,
  `surface_gravity_si`, `central_pressure_gpa` return `0.0` for a massless body; `Disc` with
  `outer ≤ inner` reports `0.0` area/surface-density; `HexWorld::transfer` is a no-op for equal
  or out-of-range indices. These are conservation-safe, but a caller must check, not assume.
- **`orbital_elements` returns only `(a, e)`** — two of the six elements. Unbound orbits give
  `a < 0`, `e ≥ 1`; a degenerate state gives `(0, 0)`.
- **Only `HexWorld` is serde-capable.** `Body`, `System`, `Cloud`, `ClassComposition`, `Disc`
  etc. do **not** derive serde — you cannot persist a whole system, only an evolved grid.
- **The per-class `element_makeup` is a second reference to Prism's element set** (hardcoded
  atomic numbers). It is guarded by `every_makeup_element_is_in_prism`, but if Prism's element
  table changes, update the makeup and keep that test green.
- **Source doc-comments point at deleted files.** Several modules cite `docs/flicker-celestial-*.md`
  and "spec §N" sections; that `docs/` corpus was migrated into MCP and deleted. Ignore those
  pointers — the design of record is in MCP (see the banner).
