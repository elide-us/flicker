# flicker-system

The **star-system formation simulation**, as a headless (GPU-free) library. Feed it one
config struct describing a supernova; it casts that supernova's elements outward by atomic
weight into a clumpy, spinning cloud, then ignites a gravitational collapse in which a star
and its planets, moons and rings **emerge** — conserved to the gram — and hands you back a
running snapshot plus the exact material composition of the selected habitable world, ready
to seed a single-world planet sim. Nothing is drawn and nothing is selected: body types and
counts are read off the result, never chosen. A caller (a viewer, a constraint search, or
the downstream planet sim) drives the whole thing through one handle, [`System`](src/system.rs).

> Design of record — why it is shaped this way, the physics decisions, the tuning history,
> and the abandoned predecessors — lives in the project's MCP memory, not here. This file
> documents how to use the crate.

## Vocabulary

flicker/crate-specific words, defined once so the rest of this file reads cleanly:

- **cast** — how far the supernova flings an element. A single characteristic *distance*
  per element, a decreasing function of atomic weight: light elements reach far, heavy ones
  fall short. (`CastParams`, `model.rs`.)
- **ejecta** — the flung element set (`Ejecta`): every Prism element with its identity and a
  display colour, sorted lightest → heaviest.
- **cloud** — the ejecta's *azimuthal structure* (`CloudField`): each element's ring carries
  over-/under-dense arcs (clumps) plus a slight non-circular meander, and the whole field
  rotates under differential shear. It says how matter *clumps*, not how much there is.
- **mass layer** — the conserved *per-element tonnage* (`CloudMass`): the absolute amount of
  each element, derived from two dials (total mass, metallicity) shaped by the real cosmic
  abundance curve. `Σ tonnage == total`, exactly.
- **hot spot** — a Phase-1 overdensity seed site (`HotSpot`): where the cloud is lumpy
  enough that bodies will later collect. Detection only; a marker, not a body.
- **mote / parcel** — one "potential body" sampled from the cloud at ignition. Motes orbit,
  drift, and merge into the handful of bodies that survive.
- **ignite** — start Phase 2: sample the cloud into motes and begin the collapse.
- **the cheat** — the H/He is cast *outward*, so a central star can't form from it by
  gravity alone. Instead the sim extracts most of the gas straight into a **pinned central
  star** (always body index `0`, fixed at the origin). That dominant central mass is what
  makes every other orbit stable.
- **Epoch-3 handoff** — the Phase-2 → Phase-3 data contract (`Epoch3Handoff`): the chosen
  habitable world's exact conserved composition plus the context (mass, orbit, star mass,
  moons, ring) the planet sim needs.
- **playable world** — a body that currently passes a narrow physical gate: rocky, in the
  star's habitable zone, in an Earth-ish mass band. Re-evaluated every step (emergent, never
  selected); the Epoch-3 handoff is taken from the most massive one.

**Units** throughout: distance in **AU**, mass in **M☉** (solar masses; `EARTH_PER_SUN`
converts to Earth masses for display), time in **years** (so `G = 4π²`).

## Where it sits

- **Builds on:**
  - `glam` — `Vec2` for body positions/velocities. That is the only math dependency.
  - [`flicker-materials`](../../Alpha/crates/content/flicker-materials) — the authoritative
    28-element Prism periodic table (symbols, atomic masses). Loaded at runtime by
    [`load_tables`](src/model.rs).
- **Reads from the content tree:** `Alpha/content/data/` (the Prism material JSON —
  `periodic_table.json` et al.) at runtime, via `load_tables()`. The path is baked at
  **compile time** relative to this crate's manifest (`CARGO_MANIFEST_DIR/../../Alpha/content/data`)
  and `load_tables()` **panics** if that directory is absent — see Sharp edges.
- **Used by:** nothing in-tree today. The viewer this crate was extracted from
  (`examples/flicker-sol2`) has been removed, and the Phase-3 single-world planet sim that
  consumes the Epoch-3 handoff does not yet exist here. This is a spec-ward tool — the API
  is built toward the pipeline, ahead of its consumers — not dead code.

### Not to be confused with (three "celestial" crates, different jobs)

| Crate | Job | Relationship |
|---|---|---|
| **flicker-system** (this crate) | **Formation dynamics** — *creates* a star system from a supernova cloud by emergent gravitational collapse, conserved to the gram. Output is a live, evolving `SystemState`. | — |
| [`flicker-orrery`](../../Alpha/crates/world/flicker-orrery) | **Presentation layout** — a *fixed* canon roster of the eight ruled Prism worlds placed on tilted-ellipse orbits over time, for the intro cinematic and the heliocentric sky. Forms nothing. | **Not an overlap.** Different stage and different data: `flicker-system`'s `BodyType` is *emergent* (read off mass + composition); `flicker-orrery`'s `BodyKind` is a *canon-locked* class of a named world. Distinct concepts, no shared representation. |
| `flicker-celestial` | An **abandoned** unification-refactor attempt (a `model`/`formation`/`evolution` crate meant to consolidate the celestial POCs). | This crate's `lib.rs` disclaims it explicitly. It has no consumers. Do not build on it; extend `flicker-system` (formation) or `flicker-orrery` (layout). |

## Driving it

Two loops, from the same handle:

**Phase-1 distribution view** — the cloud before anything collapses:

```rust
let sys = System::new(SystemConfig::default());   // loads Prism tables, derives cloud + mass layer
let spots = sys.hot_spots(t_years);               // overdensity seed sites at sim-clock t
// read sys.ejecta(), sys.cloud(), sys.cloud_mass(), sys.anchor_au() to draw the rings
```

**Full formation** — ignite and step to a settled system:

```rust
let mut sys = System::new(SystemConfig::default());
sys.ignite();                                     // Phase 2 begins (star extracted, disk seeded)
loop {
    sys.step(dt_years);                            // advance the collapse
    let snap = sys.state().unwrap();              // per-body read model for a UI / search
    if let Some(seed) = sys.epoch3_handoff() { /* the chosen world's material */ }
}
```

`SystemConfig::default()` is the canonical Sol-like system. Change a dial then call
`sync_distribution()` (cheap re-derive of Phase 1) or `reseed(seed)` for a genuinely
different cloud; `clear()` drops a running collapse back to the distribution view.

## Public API

### The facade — `System` (`src/system.rs`)

| Item | What it is for | The one thing to know |
|---|---|---|
| `System::new(cfg) -> System` | Build from a config; loads tables, derives the cloud + mass layer | Not yet ignited — Phase 1 only until you call `ignite`. |
| `ignite(&mut self)` | Start Phase 2: extract the star, seed the disk from the cloud | Re-callable; each call rebuilds the collapse from the current config. |
| `step(&mut self, dt_years)` | Advance the running collapse | No-op before `ignite`. Internally substepped for stability. |
| `clear(&mut self)` | Drop the collapse, back to the distribution view | Leaves the config untouched. |
| `sync_distribution(&mut self)` | Re-derive Phase 1 after changing `cast`/`mass`/`clump` | Cheap. Leaves any running collapse alone. |
| `reseed(&mut self, seed)` | Roll a new cloud clump pattern (a different system) | Updates `config.seed`; pair with `ignite` for a whole new system. |
| `anchor_au(&self) -> f32` | The shear anchor — geometric-mean cast radius over the element set | Feeds `hot_spots`; also the radius the cloud's shear is anchored to. |
| `hot_spots(&self, time) -> Vec<HotSpot>` | Phase-1 overdensity sites at sim-clock `time` | Strongest first, capped at `MAX_HOT_SPOTS`. Pure read; no ignition needed. |
| `state(&self) -> Option<SystemState>` | The per-body read model of the live collapse | `None` before ignition. Recomputed each call. |
| `epoch3_handoff(&self) -> Option<Epoch3Handoff>` | The chosen habitable world's material payload | `None` until a **playable** world exists — see the three None-causes in Sharp edges. |
| `ejecta` · `config` · `config_mut` · `cloud` · `cloud_mass` · `sim` · `is_ignited` | Read (and `config_mut`) access to the owned pipeline state | `sim()` is `None` before ignition. |

### The input contract — `SystemConfig` + `Tuning` (`src/config.rs`)

`SystemConfig` is every lever in one struct (`Clone + Copy`); `Default` reproduces the
confirmed Sol-like regime.

| Field | Meaning |
|---|---|
| `cast: CastParams` | The cast model — explosion reach + atomic-weight falloff. |
| `mass: MassParams` | Total cloud tonnage + metallicity (gas-to-metal balance). |
| `clump: f32` | Cloud lumpiness (0 → smooth rings; higher → stronger clumps / hot spots). |
| `seed: u32` | Reproducible cloud seed. |
| `motes_per_el: usize` | Motes sampled per element at ignition (collapse resolution / body count). |
| `tuning: Tuning` | The collapse physics + body-typing + playability levers (below). |

`Tuning` (all `Copy`, all read off `Sim::tuning` at runtime) — the dials meant to be
watched and adjusted; true physical constants (`G`, unit conversions, the integration
substep) are deliberately *not* here:

| Group | Fields |
|---|---|
| Gravity / integration | `softening` (AU, finite gravity at tiny separations), `radius_k` (accretion-reach coefficient, `reach = radius_k·m^⅓`) |
| The cheat + disk spin | `star_gas_frac` (fraction of gas extracted into the pinned star), `disk_spin` (fraction of circular speed disk parcels start at) |
| Gas drag | `drag`, `drag_target_frac` (sub-circular → inward migration), `gas_tau` (dispersal timescale), `drag_floor` (perpetual floor keeping settled orbits circular) |
| Densities (g/cc) | `rho_gas_gcc`, `rho_ice_gcc`, `rho_rock_gcc` — drive the physical/collision + Roche radii |
| Satellites / rings | `hill_frac` (Hill-fraction a moon is retained within), `tidal_frac` (Roche scale → shredding into rings) |
| Body-type thresholds (M☉) | `star_mass`, `giant_mass`, `planet_mass` — the mass cuts `classify` reads |
| Playability gate | `playable_mass_min`, `playable_mass_max` (mass band), `hz_inner_frac`, `hz_outer_frac` (habitable-zone edges × √luminosity) |

Constants: `DEFAULT_SEED` (`0xC10D_5EED`), `DEFAULT_MOTES_PER_EL` (`12`), `DEFAULT_CLUMP` (`0.6`).

### Phase 1 — cast, mass, cloud, detect

| Item | What it is for | The one thing to know |
|---|---|---|
| `CastParams { explosion, falloff }` | The two cast dials | `explosion` (0..1) scales the lightest element's reach 8→80 AU; `falloff` is the atomic-weight exponent (`0.5` = energy equipartition). |
| `CastParams::reach_au()` / `distance_au(atomic_mass)` | Lightest-element reach / one element's cast distance | Pure functions of the live params — distance is recomputed, never stored. |
| `load_tables() -> flicker_materials::Tables` | Load the Prism periodic table from `Alpha/content/data` | Panics if the repo content dir is missing (Sharp edges). |
| `Ejecta { elements: Vec<ElementCast> }` + `from_tables(&Tables)` | The element set, sorted by atomic mass ascending | Index `0` = lightest (H) = outermost ring; index order is the shared axis for cloud, mass, and legend. |
| `ElementCast { symbol, name, number, atomic_mass, color }` | One element's identity + display tint | `color` is a grounded per-element hue; unmapped elements fall back to neutral (Sharp edges). |
| `MassParams { total, metallicity }` | The two mass dials | `total` in M☉; `metallicity` is the metals (Z>2) fraction (Sun ≈ 0.014). |
| `CloudMass { tonnage: Vec<f32> }` + `derive(&ej, &p)` | The conserved per-element tonnage | Parallel to `ejecta.elements`. `total()` ≈ `MassParams::total`; `metals(&ej)` = the Z fraction exactly. |
| `EARTH_PER_SUN` (`332_946.0`) | M☉ → Earth-mass display factor | Presentation only. |
| `CloudField` + `new(n, seed, strength)` · `reseed(seed)` | The rotating clump/wobble structure | `strength` = lumpiness; `density(i,θ,rot)` (~1, >1 in clumps), `wobble(i,θ,rot)` (fractional radial meander), `omega(r_au, anchor_au)` (shear rate). |
| `detect(ej, cast, cloud, time, anchor_au) -> Vec<HotSpot>` | Scan the cloud for seed sites | Strongest first, capped at `MAX_HOT_SPOTS` (`60`). `System::hot_spots` wraps this. |
| `HotSpot { au, theta, strength }` | One overdensity site, world-space | `strength` = peak overdensity over ambient. |

### Phase 2 — the collapse: `Sim` + `BodyType` (`src/collapse.rs`)

`Sim` is the running collapse in struct-of-arrays form; a merged-away mote is flagged
`!alive` (indices stay stable within a step). Built by `from_cloud(ej, cast, cloud, cm,
per_el, tuning)` — normally via `System::ignite`, not directly.

| Item | What it is for | The one thing to know |
|---|---|---|
| `Sim::from_cloud(…)` | Extract the star into body `0`, seed the disk from the cloud's clumps | Conserved: star + disk == the whole cloud. |
| `step(&mut self, dt)` | One collapse step: gravity (softened direct sum, substepped) → drag → merge | Mass conserved across the whole step. |
| `classify(i) -> BodyType` | Read off what body `i` became, from mass + composition | Display only — the sim never branches on it. |
| `is_playable(i) -> bool` | The playable-world gate for body `i` | Rocky + in the HZ + in the mass band. `false` for the star (index 0) and dead bodies. |
| `orbit_host(i) -> usize` | The body `i` orbits — `0` (star) for a planet, else the planet it is a moon of | Force-dominant attractor + Hill-retention; no reach-in grab. |
| `radius_au(i)` · `live_count()` · `largest_mass()` · `total_mass()` · `init_total()` | Drawn/merge radius; counts; the emergent star's mass; conservation check | `total_mass()` should equal `init_total()` every step. |
| pub fields `n_el, pos, vel, mass, comp, alive, ring_mass, el_numbers, time, tuning` | The raw arrays, for a renderer/search that needs them | `comp[i*n_el + e]` = element `e`'s mass in body `i`. `ring_mass[i]` is a *subset* of `mass[i]` (tidally-shredded debris drawn as a disc). |
| `BodyType` — `Star · GasGiant · IceGiant · RockyPlanet · IcyBody · Asteroid` | The emergent body class | `color() -> [f32;3]`, `label() -> &str`. |

### The output contracts (`src/system.rs`)

| Item | Meaning |
|---|---|
| `SystemState { time, star_mass, total_mass, init_total, bodies }` | Snapshot of a running collapse. `star_mass` is the heaviest live body; `total_mass` should equal `init_total`. `bodies` is star-first. |
| `BodySnapshot { index, pos, vel, mass, ring_mass, radius_au, kind, host, playable }` + `is_star()` / `is_moon()` | One live body. `index` is its stable per-frame id (and what `host` refers to); `host == 0` ⇒ orbits the star (a planet), else a moon of body `host`. |
| `ElementMass { symbol, number, mass_msun }` | One element's conserved mass in a composition (the handoff payload unit). |
| `Epoch3Handoff { composition, total_mass_msun, orbit_radius_au, star_mass_msun, moons, has_ring }` | The Phase-2 → Phase-3 contract for the chosen habitable world: its exact per-element composition plus the context (mass, orbit, star mass, moon count, ring) the planet sim needs. |

## Interactions

**None in the flicker sense** — `flicker-system` is a headless simulation library. It
captures no input signals, publishes and binds no Model keys, renders nothing, spawns no
threads, and writes nothing to the content tree. What it *hands* a consumer is data:
`SystemState` and `Vec<HotSpot>` for a viewer to draw, `Epoch3Handoff` for the downstream
planet sim, and read access (`cloud()`, `cloud_mass()`, `ejecta()`, `sim()`) to the live
pipeline state. Its only content-tree touch is the runtime **read** of `Alpha/content/data`
in `load_tables()`.

## Gates

`source ~/.cargo/env && cargo test -p flicker-system` — **20 pass, 1 ignored** (see the
last row; it is the gate over this crate's headline output):

| Test | What it enforces |
|---|---|
| `model::loads_all_28_elements` | The Prism table loads all 28 elements, sorted H → U. |
| `model::heavier_elements_cast_shorter` | Cast distance decreases with atomic weight. |
| `model::explosion_size_scales_every_distance` | `explosion` scales all distances but preserves element ratios. |
| `mass::total_tonnage_matches_the_mass_dial` | `Σ tonnage == MassParams::total`. |
| `mass::metals_hold_exactly_the_metallicity_fraction` | `Σ metals == metallicity · total`. |
| `mass::hydrogen_dominates_and_uranium_is_a_trace` | Abundance shape: H ≫ U, U present but a deep trace. |
| `mass::iron_peak_outweighs_its_neighbours` | The iron peak (Fe > Cr, Co, Ti). |
| `mass::raising_metallicity_shifts_mass_from_gas_to_metals` | Higher Z → more metals, less H. |
| `cloud::zero_strength_is_flat` | `strength = 0` → uniform density, no wobble. |
| `cloud::density_varies_around_the_ring_when_lumpy` | Non-uniform density when lumpy. |
| `cloud::inner_rings_shear_faster_than_outer` | Keplerian-style differential shear. |
| `cloud::shear_survives_across_visible_rings` | The clamp doesn't flatten adjacent rings to one rate. |
| `collapse::collapse_conserves_total_mass` | Mass conserved across gravity + merge over 300 steps. |
| `collapse::motes_merge_into_fewer_bodies` | The collapse coalesces motes. |
| `collapse::a_dominant_central_star_emerges` | A star holding >80% of the cloud grows. |
| `collapse::the_system_stays_bound` | >95% of mass stays bound near the star (no wholesale ejection). |
| `collapse::a_moon_stays_in_orbit_around_its_planet` | A moon survives (not swallowed) and keeps orbiting its planet. |
| `collapse::an_icy_satellite_inside_roche_shreds_into_a_ring` | An icy satellite inside the Roche zone → ring; mass conserved. |
| `collapse::the_star_absorbs_close_bodies_rather_than_hosting_moons` | The star absorbs close bodies (no "moon of the star"). |
| `collapse::velocities_and_positions_stay_finite` | No numeric blow-up. |
| `collapse::playable_worlds_emerge_and_obey_the_gate` **(#[ignore]'d)** | That some emergent system yields a **playable** world and every flagged world obeys the gate. **Disabled on x86_64** — the seed set births no playable world there; runs only with `cargo test -- --ignored` on Apple Silicon. See Sharp edges. |

## Sharp edges

- **The headline output is unverified on x86_64.** The one gate proving a playable world can
  emerge (and therefore that `epoch3_handoff()` ever returns `Some`) is `#[ignore]`'d because
  the default seed set produces no playable world on x86_64 CI. On that architecture the
  Epoch-3 handoff is `None` for the default seeds and no green test warns you. Run on Apple
  Silicon (`cargo test -- --ignored`) to exercise it, or supply your own seed.
- **`epoch3_handoff()` returning `None` means one of three things**, indistinguishably: not
  ignited yet; ignited but no playable world has emerged *yet* (keep stepping); or this
  system will never yield one (bad/unlucky seed, or the x86_64 case above). There is no
  status telling them apart — check `is_ignited()` and how long you have stepped.
- **Body `0` is always the pinned central star** — injected by "the cheat", fixed at the
  origin, and the heaviest body. `is_playable(0)` is always `false`; `host == 0` marks a
  planet, not a moon.
- **`load_tables()` hard-panics on a compile-time-relative path.** The content dir is baked
  as `CARGO_MANIFEST_DIR/../../Alpha/content/data`; move the crate out of the repo layout and
  it panics. Fine in-repo, a landmine for reuse elsewhere.
- **Lithium (element 3) is untabulated.** It exists in the 28-element table but is absent from
  both the abundance table (falls to a trace floor) and the colour table (falls to neutral
  grey) — silently, with no warning. Any element you add without a mapping does the same. (The
  source comment "for the 27 Prism elements" reflects the 27 that *are* tabulated, not the
  canon count of 28.)
- **`SystemState.star_mass` (heaviest live body) and `Epoch3Handoff.star_mass_msun`
  (`mass[0]`) are computed two ways** but agree because body 0 is both pinned and dominant.
- **`ring_mass[i]` is part of `mass[i]`, not extra.** It records how much of a body's mass is
  an orbiting ring so a renderer draws it as a disc; it never breaks conservation.
- **`hot_spots` / `detect` are markers, not bodies.** They mark where the cloud is lumpy;
  they do not correspond one-to-one to the bodies the collapse produces.
