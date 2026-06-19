# Handoff — `flicker-solarsystem` (solar-system formation sim → the feed into Epoch 1)

**Status:** The **formation simulation is complete and the user loves the results** (§1) — emergent,
mostly-non-habitable, diverse solar systems whose protoplanets carry a real composition for Epoch 1.
The **active work is the CINEMATIC visual pass** (§A): a volumetric raymarched dust cloud, a galactic
star-field background, in-shader star occlusion + god rays, and a slow Star-Trek-titles camera. It's in
a **great place and being iteratively refined** — the user dials the look by eye and tells Claude what to
push. *Continue refining the cinematic pass.* The sim itself is stable; don't re-tune it (see the EMERGENT
warning in §1).

New session: read this, then **`scene.rs`** + **`crates/flicker-render/src/shaders/volumetric.wgsl`** +
`pipeline_volumetric.rs` (the active cinematic surface), and the sim files
`examples/flicker-solarsystem/src/{material,body,disk,collide,sim,habitability}.rs` as reference.

**Audience:** Claude Code (impl), Elideus (review). **Verification is visual** (`cargo run -p
flicker-solarsystem`); Claude keeps `cargo build/clippy/test` green. Per `user-verifies-app-themselves`.
The cinematic look is **all blind-tuned** — Claude can't see the framebuffer; every change is a hypothesis
the user confirms. Aesthetic north star (user's words): **Star Trek, not The Expanse/Star Wars** — fancy
utopian sci-fi, beautiful even when dark; *"never realistic, always cinematic — lie about the data, make
it artful."* The *data* (sim) is correct; the *visuals* are allowed to lie for beauty.

---

## 0. Prism alignment (a hard constraint — verified with the user)

The sim's chemistry stays **strictly inside Prism's limited periodic table**
(`data/materials/periodic_table.json`, loaded via `flicker_materials::Tables`). It tracks
**element masses over the Prism set** grouped into 5 coarse **condensation classes** (a physics
grouping, not mineralogy) — **no `materials[256]` classification** (that's Epoch 2's job). Output
is an element-abundance vector, the currency Epoch 1 reads.

**Magnesium was added to the canonical table this session** (user-directed): `Mg` #12, taking
Prism to **27 elements** (design ceiling 30). One ripple fixed: the count assertion at
`crates/flicker-materials/src/tables.rs:193` (26 → 27). Mg is now a major silicate component.

## 1. The model (what gets simulated) — units AU · yr · M☉, `G = 4π²`

0. **Nebula / supernova (the master initial condition, `disk.rs::Nebula`).** Each system is seeded by a
   **supernova of a drawn size** (`supernova_size ∈ [0,1]`) that sets its **disk surface density**
   (`sigma_1au`, log-uniform `[SIGMA_MIN 3, SIGMA_MAX 36]` g/cm²) **and metallicity** (`Z_MIN 0.5..Z_MAX
   2.2`). **Metallicity scales the *solids* available** (`solid_sigma() = sigma_1au · metallicity`) — so
   metal-rich disks grow bigger cores and thus more giants (the real correlation, working through core
   mass, not a threshold fudge). Main diversity lever: light/metal-poor disks stay barren, heavy/metal-
   rich ones turn giant-dominated. Drawn random per system (`random_supernova`); the viewer **dials it**
   (`[`/`]`).
1. **Disk → solids (analytic, `disk.rs`).** `solid_surface_density(r, σ)` = `Σ ∝ r^-3/2` with the
   **snow-line jump** (`ICE_BOOST`, past `SNOW_LINE = 2.7`). `composition_fractions(r)` gives class
   fractions from the condensation sequence: metal/silicate/carbon refractories inner; **ice** past the
   snow line; plus a small **inner-disk hydration floor** (`HYDRATION_FLOOR`, hydrated silicates — a
   real, debated water source) ramping up toward the snow line.
2. **Embryos + giants (analytic seeding, `disk.rs::seed_embryos`).** Tile `[DISK_INNER 0.3, DISK_OUTER 15]`
   into `SPACING_B`-Hill-radius feeding zones; each zone's solids → one embryo of local composition. Embryos
   are seeded **already dynamically excited** (e ≲ 0.12, the eccentricity a swarm reaches *entering* the
   giant-impact phase) so they cross and collide immediately — the slow mutual stirring that pumps e up takes
   far more orbits than the compressed run covers; collisions then damp e back down, as in reality.
   `promote_giants` gives an H/He envelope to **every** past-snow-line core above a **fixed** gas-capture
   threshold (`CRIT_CORE 6 M⊕`) — **no count cap**: modest envelope below `RUNAWAY_CORE 12` (→ ice giant),
   large jittered envelope above (→ gas giant). Giant **count/kind/mass are emergent** (0 in a light disk,
   several in a heavy metal-rich one). (Disk capped at ~15 AU: bodies past it complete only ~1–2 orbits in
   the compressed run, so they can't evolve.)
3. **N-body giant-impact phase (`sim.rs::run`).** Velocity-Verlet (leapfrog), star + mutual gravity,
   softened. A collision fires when the separation drops below the **gravitationally-focused capture
   distance**: the inflated geometric reach (`body.rs::COLLISION_INFLATION = 1200`, the time-compression
   trick — why "Myr" is a **cosmetic** rescale) times the **Safronov factor** `√(1 + ACCRETION_FOCUSING·
   (v_esc/v_rel)²)` (clamped, capped at `MAX_CAPTURE_AU`). **`ACCRETION_FOCUSING` is the "how much gravity
   bodies exert on their bands" lever** — it makes massive, slow-passing bodies sweep up a far larger reach
   (runaway growth → cleared bands → fewer, larger planets). Each collision → `collide.rs::resolve`.
   Unbound-and-far bodies are **ejected** (rogue planets), star-grazers **consumed**. Runs once on enter/
   reseed/dial, decimated into a `Timeline`; scene plays it back. ~150–300 ms/run on the M5.
4. **Collisions (`collide.rs`, Leinhardt-Stewart-flavoured).** From approach speed `v_inf` vs mutual
   escape `v_esc` and impact parameter `b`: **merge** (gentle), **hit-and-run** (fast + grazing,
   comparable sizes — two bodies bounce apart), **erosion/disruption** (head-on — largest remnant
   from a simplified `Q*_RD` law + debris), and **moon capture** (a body `< MOON_MAX_RATIO` of the
   target, passing slowly + grazing → bound as a `Moon`, bookkept on the planet, *not* merged into its
   mass/composition; mostly happens to giants — realistic). Composition layers core→envelope; collisions
   **strip outermost-first** (→ iron-rich / desiccated remnants); merges carry moons. Mass + momentum
   conserved exactly. Moons ride with the largest remnant.
5. **Classify, verdict + export (`body.rs`, `habitability.rs`, `material.rs`).** Survivors are labelled the
   **IAU way**: *giant*, or a *planet* that **cleared its orbital band** (`body::cleared_neighborhood` —
   outweighs everything else within ±15 % of its semi-major axis), or a *dwarf/belt object* that didn't.
   This is why a low-mass disk's crowded outer swarm reads as a Kuiper-like belt, not 20 "planets" (avg
   ~9 planets/system emerges from it). **Habitability verdict** (a pure classifier, never feeds back): in HZ
   (`0.95–1.37`) ∧ `0.3–3 M⊕` ∧ low `e` ∧ rocky ∧ has water → **playable**, else the reasons.
   `Composition::to_epoch1_abundance(&Tables)` → symbol-keyed mass-% over Prism.

**EVERYTHING is EMERGENT, not targeted (a correction the user made twice — internalise it).** Do *not*
tune the generator to hit any outcome rate (habitability OR giant count OR anything). Both prior mistakes
were hardcoded outcomes: a `SIGMA_1AU` picked to vary the playable count, and a `MAX_GIANTS = 3` cap.
Both are gone. Diversity now flows from the **`Nebula`** (supernova → disk mass + metallicity): light/
metal-poor disks → small planets → **barren**; heavy/metal-rich → many giants, dynamically violent;
Goldilocks → habitable worlds. Giant count/kind/mass, belt objects, ejections (rogue planets), and
habitability are all just *what falls out*. All physical parameters are set from science/observation
(cited at each const). If a future outcome looks wrong, fix the **physics/inputs**, never add a target.

**Observed emergent behaviour (reference only — don't optimise it):** across seeds, **~9 planets/system**
on average (range ~2–16); giant count 0 (light/metal-poor → often barren) to ~7 (heavy metal-rich);
giant masses span ~10 M⊕ ice giants to multi-Jupiter gas giants; the inner/HZ zone clears to ~1 HZ body;
low-mass disks leave a Kuiper-like **dwarf/belt** swarm; some outer bodies eject as **rogue planets**;
~few % of *planets* / roughly half of Sun-like *systems* have a potentially-habitable world. Caveat on the per-system rate: we model **only Sun-like stars** (the favourable case) and "HZ
rocky + water" is *potential* habitability, not confirmed life — the extra real-world filters (stable
Gyr climate, magnetic field, tectonics, tidal locking, atmosphere, sterilising events, and that most
stars aren't Sun-like) are unmodelled and are where the rest of reality's barrenness lives. Add those
for more realism — **never** a target multiplier.

## 2. Files (`examples/flicker-solarsystem/src/`)

- `astro.rs` — constants/units (`G`, `M_STAR`, mass/length conversions).
- `material.rs` — `MaterialClass{Metal,Silicate,Carbon,Ice,Gas}`, `CLASS_ELEMENTS` (Prism-element
  makeup, validated ⊆ table), `Composition` (per-class mass; merge/strip/density/water/export),
  `load_tables()`.
- `body.rs` — `Body{pos,vel,mass,comp,kind,moons}`, `Moon{mass,comp}`, physical/collision radii, orbital
  elements, `is_bound`, `cleared_neighborhood` (IAU planet-vs-dwarf classification).
- `disk.rs` — `Nebula` (supernova→sigma+metallicity, `solid_sigma`), `solid_surface_density`,
  `composition_fractions/_at`, `seed_embryos`/`promote_giants`, dust field, `disc_texture`/`ring_texture`, `Rng`.
- `collide.rs` — `resolve(a,b) -> Outcome` (regime incl. `Capture` + bodies + site).
- `sim.rs` — `run(seed, supernova_size) -> Timeline`; integrator, **gravitationally-focused** collision
  detection (`ACCRETION_FOCUSING` lever), ejection passes. `Snapshot.bodies` holds **full `Body` clones**
  so the scene runs the live verdict/classification/export off the *current* moment (the last snapshot is
  the final state — there's no separate `finals`). `Timeline.nebula` = initial conditions.
- `habitability.rs` — `assess(&Body) -> Verdict`.
- `scene.rs` — **CINEMATIC** playback render. All glow goes through `draw_billboard_additive` (no depth
  write → no artifacts; additive → bloom): a layered **star bloom**, a dense (~18k) **glowing dust nebula**
  that dissipates by ~⅘, the bodies (planets/giants/moons/belt) as glow, collision flashes, **blue rings**
  on habitable worlds. **Per-planet orbit ellipses** (`orbit_ellipse(&Body)` from the state vector, incl.
  eccentric ones). **Choreographed camera** (`cinematic_pose(t)` + `camera.rs`): rises from below the disk
  plane through the dust to above it and glides into the inner/HZ region as the cloud clears; **dragging
  hands off to manual orbit** (`OrbitCam::update(.., active)`/`set_pose`), reseed re-arms. **Everything LIVE**
  off `current_bodies()` (list/rings/counts/export); `displayed()` = top-mass + *all* habitable so rings
  match rows. HUD as before.
  The disk is now a real **volumetric raymarched dust cloud** (`set_disk_cloud` → `Renderer::set_volumetric_disk`),
  not sprites — driven by the sim (dissipates inside-out with `formation=t`, carves **annular gaps** at the
  **giants'** orbits via Hill radius — only giants, else 32 embryo-gaps shred the cloud). Sprite field deleted.
  A **deep-space galactic background** is drawn behind it via the existing **sky pass** (`renderer.draw_sky()`
  + a `SceneLighting` with sun *and* moon pushed below the horizon → pure Milky-Way-band + star field, no discs);
  the dust composites over it, so **dense dust occludes the stars into dark lanes** (the galactic-core look) —
  the dust is tuned *dark* (occluding) with a warm glowing centre, not bright/white.
- Dep added: `flicker-materials.workspace = true`. **Engine additions (reusable, non-breaking):**
  (1) **additive, no-depth-write billboard pipeline** (`pipeline_billboard.rs` 2nd pipeline +
  `Renderer::draw_billboard_additive`); (2) **volumetric raymarch pass** — `pipeline_volumetric.rs` +
  `shaders/volumetric.wgsl` (fullscreen, `inv_view_proj` ray, marches a flared annular dust disk + fbm
  turbulence + inside-out dissipation + annular gaps; premultiplied "over"), `Renderer::set_volumetric_disk(VolumetricDisk{..})`,
  exported via `flicker::render`. Both have GPU shader-validation tests. Existing `draw_billboard`/sky untouched.

**Controls:** drag = take manual camera control · wheel zoom · Space play/pause · ←/→ scrub · ↑/↓ select ·
`[`/`]` dial supernova · R reseed · Esc. Cosmetic span ~150 Myr; camera is cinematic until you drag.

## 3. Deferred (next slices)

- **Close the loop to Epoch 1 (slice C):** wire the export vector into `flicker-world`'s
  `Epoch1Params.abundance` (`crates/flicker-world/src/world.rs` `ABUNDANCE_DEFS`, `epoch1.rs`) —
  the function + HUD display exist; the cross-crate hand-off does not. (Note: `ABUNDANCE_DEFS` has
  14 symbols and no `Mg`/`Ni`; decide whether to extend it or fold those when wiring.)
- **Cinematic, next steps:** The cloud is now **blobby/billowing** (domain-warped noise + per-position
  scale-height variation, `density()` in `volumetric.wgsl`), the **star is rendered inside the shader** so
  the dust **occludes** it (`inscatter += star_col*core*trans`), and the in-scatter is **shadowed toward the
  star** → **god rays** through the gaps (`shadow_to_star`). Camera opens edge-on just below the plane, well
  out, and the pass is slowed (`speed 0.032`, ~30 s). Remaining: the dust still doesn't occlude the
  **planet/body** billboards (only the star) — that needs sampleable depth (`create_depth_view` +
  `| TEXTURE_BINDING`); a brighter **galactic-core bulge** (sky's Milky-Way band is faint `0.05` — shared
  `sky.wgsl`); **perf** — the raymarch + shadow taps are blind to framerate (reduce `STEPS`/shadow taps/fbm
  octaves if choppy on the M5 Pro). All look constants still **blind-tuned** — expect color/shape iteration.
  (2) **3D Epoch-1 planet spheres at the end** — lit/rotating/composition-coloured (the browseable "starting
  points"); engine ready (`upload_mesh`/`draw_mesh`/`set_scene` + per-vertex RGB; ~80-line sphere builder).
  (3) Explicit **god-ray** streaks. (4) **Click a world → enter Epoch 1** (flow into `flicker-world` — hook only).
- **Richer asteroid belts.** Belt objects exist (leftover small survivors, classified + rendered + counted)
  and emerge mainly in low-mass disks. A *giant-sculpted* main belt (resonances suppressing accretion in a
  specific annulus, leaving a ring of planetesimals just inside a giant) is not yet modelled — would need
  seeding a separate planetesimal population and/or resonance stirring. Likewise a possible **gas-budget
  limit** on giants (finite disk gas → caps how many runaway giants can form) if counts read too high.
- Background-threaded precompute (currently a synchronous ~150–300 ms hitch on enter/reseed/dial);
  outer disk beyond ~22 AU (evolves too slowly to simulate watchably); rogue-planet *capture* by other
  systems (the user's musing — a body ejected here arriving elsewhere); multi-star ignition.
  `materials[256]` mineralogy is **out of scope** (Epoch 2).

## 4. Verify

`source ~/.cargo/env && cargo build/clippy/test -p flicker-solarsystem` — 21 tests, clippy clean.
Mg touched shared data: `cargo test -p flicker-materials -p flicker-worldgen -p flicker-world` green.
Visual (user): `cargo run -p flicker-solarsystem` — embryos orbit, collide (flashes), merge/scatter,
some eject; settles to a few protoplanets; HUD lists each with mass/orbit/water% and a
playable/not-playable verdict (most *not*), plus the selected survivor's element-export vector.
