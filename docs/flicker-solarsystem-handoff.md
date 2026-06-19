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

## A. The cinematic pass — ACTIVE WORK (start here)

The viewer plays the recorded formation back as a **cinematic**, not a data viz. Current state (all in
`scene.rs` + the engine pieces in §2):

- **Galactic star-field background** — the engine **sky pass**, enabled in `scene.rs::render` via
  `renderer.draw_sky()` + a `SceneLighting{}` with **sun *and* moon pushed below the horizon** (full night →
  pure Milky-Way band + procedural star field, no sun/moon discs) and a near-black sky gradient. This is the
  "best starfield" (same one voxel-cluster uses). It exists so the **dark dust occludes the stars into lanes**.
- **Volumetric dust cloud** (`crates/flicker-render/src/shaders/volumetric.wgsl`, driven by
  `scene.rs::set_disk_cloud` → `Renderer::set_volumetric_disk(VolumetricDisk{..})`): a full-screen raymarch of
  a flared annular disk, **domain-warped + billowing** (per-position scale-height) so it's *blobby*, not a
  pancake; tuned **dark** (occludes the star-field → dark lanes) with a warm glowing centre. **Sim-driven**:
  dissipates inside-out with `formation = t`, carves **annular gaps only at the giants' orbits** (NOT every
  embryo — 32 embryo gaps shred the whole cloud).
- **Star rendered *inside* the shader** so the dust **occludes it** (`inscatter += star_col*core*trans`); the
  in-scatter is **shadow-marched toward the star** (`shadow_to_star`) → **god rays** through the gaps. (There is
  no star billboard — that would draw on top and never be blocked.)
- **Camera** (`camera.rs` + `scene.rs::cinematic_pose(t)`): opens **edge-on just below the disk plane, well out**
  (looking *through* the cloud at the occluded star), then slowly **rises above and glides in**. A languid
  ~50 s pass (`speed 0.020`, slowed from a frantic 0.032); opens ~15 % tighter (`outer * 2.72`, was `3.2`).
  Once the system **settles (coast)** the cinematic doesn't stop — it keeps a **slow turntable orbit**
  (`COAST_ORBIT_RATE` × `coast_cam`, a wall-clock that runs *even while frozen*, so the locked beauty shot keeps
  turning around fixed bodies). **Dragging hands off to manual** orbit (`OrbitCam::update(.., active)` +
  `set_pose`); **reseed/`R` (and `[`/`]`) re-arm *and resume* the cinematic** — `rerun()` sets `play = true`, so a
  fresh roll always plays even if the previous system was frozen to lock its seed (that omission was why the
  cinematic looked "gone, just pan/zoom" after a freeze).
- **The formation never freezes.** When playback reaches `t = 1` the system crosses into a **Keplerian coast**
  (`scene.rs::kepler_advance` + `coast_year`/`coast_rate`): the settled survivors keep orbiting their conics
  forever at the same pace the formation ended on — no pause, no 150 Myr stop. The HUD clock climbs past 150 Myr
  and reads `coasting`. Pure playback continuation (no sim/physics change); the sim + Epoch-1 export are untouched.
- On top (additive billboards / lines, see §2): the bodies as glow dots, moons, collision flashes, **per-planet
  orbit ellipses**, **blue rings** on currently-habitable worlds, and the live HUD. All read `scene.rs::live`
  (the single per-frame body set — recorded snapshot while forming, Kepler-coasted while settled).

**Where the look lives (all blind-tuned — push these on user feedback):** in `volumetric.wgsl` — `STEPS` (44),
`shadow_to_star` taps (4), `fbm2/fbm3` octaves, `cloud` contrast (`pow(turb,1.7)*1.9`), `vbump` billow range,
the warp strength, the in-scatter `lit`/god-ray term, the `core` star profile. In `scene.rs::set_disk_cloud` —
`density` (2.2), `tint` (dark), `glow` (warm), `scale_height`, gap width. In `cinematic_pose` — pitch/distance/
yaw ramps; `speed`. **Watch performance:** the raymarch + per-step shadow taps are heavy and Claude is blind to
framerate — if it's choppy on the M5 Pro, cut `STEPS`/shadow taps/octaves first.

**Cinematic next features (in priority-ish order):**
1. **Depth-correct body occlusion** — the dust occludes the *star* but not the *planet/body billboards* (they
   draw after). For lanes crossing in front of bodies: make the depth buffer sampleable
   (`pipeline_mesh.rs::create_depth_view` + `| wgpu::TextureUsages::TEXTURE_BINDING`) and have `volumetric.wgsl`
   read it to bound rays / composite in 3D.
2. **3D Epoch-1 planet spheres at the end** — each final planet resolves into a lit, rotating,
   composition-coloured **sphere** (the browseable "starting points" the user cherry-picks). Engine is ready:
   `upload_mesh`/`draw_mesh`/`set_scene` + per-vertex RGB (`mesh.wgsl` `direct()` path); write a ~80-line
   standalone icosphere/UV-sphere builder (do **not** entangle flicker-world's hex globe), colour vertices from
   the body's `Composition` (blend of `MaterialClass::color`), light with a star `SceneLighting`.
3. **Brighter galactic-core bulge** behind the disk (the sky's Milky-Way band is faint at `0.05` — boosting it
   touches shared `sky.wgsl`, or add a separate bright-core element just for this scene).
4. **Click a world → enter Epoch 1** (the flow into `flicker-world` mode — affordance/hook only; the transition
   itself is slice C, below).

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
   into `SPACING_B`-Hill-radius feeding zones (now **`SPACING_B 10.0`**, widened from 8.5 — the upper end of the
   oligarchic range, for **fewer, chunkier embryos → fewer final bodies**); each zone's solids → one embryo of
   local composition. (For an even sparser system, the complementary knob is `sim.rs::ACCRETION_FOCUSING`, the
   N-body runaway-clearing reach — left at its physical 1.0.) Embryos
   are seeded **already dynamically excited** (e ≲ 0.12, the eccentricity a swarm reaches *entering* the
   giant-impact phase) so they cross and collide immediately — the slow mutual stirring that pumps e up takes
   far more orbits than the compressed run covers; collisions then damp e back down, as in reality.
   `promote_giants` is **gas-budget gated** (this is what keeps giants realistic — 0–4/system, was an
   unphysical 6–12). Candidate cores are past the snow line above the **critical core mass `CRIT_CORE 10 M⊕`**
   (Mizuno/Pollack, raised from 6). The disk holds a finite gas reservoir (`GAS_CAPTURE_EFFICIENCY 0.20` of
   its total gas, `disk_gas_mass`); cores are served **inner→outer** (nearest the snow line reach critical
   mass first, while gas remains) — each draws its envelope (modest below `RUNAWAY_CORE 12` → ice giant, large
   jittered above → gas giant, capped `ENVELOPE_CAP`) until the reservoir is **spent**, after which the
   remaining outer cores stay **bare** (failed cores → ice-giant cores / large icy bodies, à la Uranus/Neptune).
   Giant **count/kind/mass stay emergent** — driven now by disk mass + metallicity *and how much gas there was
   to share*: 0 in a light disk, a few in a heavy metal-rich one. **`GAS_CAPTURE_EFFICIENCY` is the main giant-
   count lever** (down → fewer). (Disk capped at ~15 AU: bodies past it complete only ~1–2 orbits in the
   compressed run, so they can't evolve.) *Open follow-up:* heavy disks leave **over-massive bare cores**
   (tens of M⊕) past the snow line — the analytic isolation mass runs high out there; tune via disk
   truncation / `SPACING_B` / a core-mass ceiling if they read too big.
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
   from a simplified `Q*_RD` law + debris), and **moon capture** (a small body passing slowly + grazing →
   bound as a `Moon`, bookkept on the planet, *not* merged into its mass/composition). Capture is
   **giant-aware**: a giant target binds over a much **wider approach cone** (`GIANT_B_GRAZE 0.35` vs the
   rock-only `B_GRAZE 0.7`) and a slightly larger mass ratio (`GIANT_MOON_MAX_RATIO 0.15` vs `MOON_MAX_RATIO
   0.12`) — deep well + circumplanetary drag — so **remote giants assemble satellite systems from passing
   dwarfs/icy bodies** instead of swallowing them (all giants form past the snow line, so "giant" = "remote").
   Composition layers core→envelope; collisions
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
- `scene.rs` — the **cinematic** playback render (the full picture is in **§A**): galactic sky background,
  the **volumetric** dust cloud (`set_disk_cloud`), the in-shader-occluded star + god rays, the choreographed
  `cinematic_pose(t)` camera, and on top — bodies/moons/flashes as `draw_billboard_additive` glow, **per-planet
  orbit ellipses** (`orbit_ellipse(&Body)`), **blue rings** on habitable worlds, the live HUD. **Everything is
  LIVE** off `current_bodies()` (the current snapshot): the protoplanet list, the planet/giant/dwarf counts,
  the rings and the export all update as the system evolves; `displayed()` = top-mass + *all* habitable so the
  rings always match listed rows. `camera.rs` — `OrbitCam` doubles as the cinematic camera (`set_pose` + an
  `active` flag for the manual handoff).
- Dep added: `flicker-materials.workspace = true`. **Engine additions (reusable, non-breaking):**
  (1) **additive, no-depth-write billboard pipeline** (`pipeline_billboard.rs` 2nd pipeline +
  `Renderer::draw_billboard_additive`); (2) **volumetric raymarch pass** — `pipeline_volumetric.rs` +
  `shaders/volumetric.wgsl` (fullscreen, `inv_view_proj` ray, marches a flared annular dust disk + fbm
  turbulence + inside-out dissipation + annular gaps; premultiplied "over"), `Renderer::set_volumetric_disk(VolumetricDisk{..})`,
  exported via `flicker::render`. Both have GPU shader-validation tests. Existing `draw_billboard`/sky untouched.

**Controls:** drag = take manual camera control · wheel zoom · **Space = play / freeze** · ←/→ scrub · ↑/↓ select ·
`[`/`]` dial supernova · R reseed · Esc. Formation spans a cosmetic ~150 Myr, then the system **coasts on**
(keeps orbiting past 150 Myr); camera is cinematic until you drag. ← rewinds out of the coast.

**Space is the freeze / Epoch-1 seed-lock.** Pausing (`!self.play`) holds the *current configuration* fixed —
all bodies + their live compositions at that instant become the committed Epoch-1 seed (the HUD reads
`❚❚ FROZEN · Epoch-1 seed locked` and the export panel `Epoch-1 SEED — LOCKED`, both in cyan). Nothing is
snapshotted separately: everything is already recalculated on the fly off `scene.rs::live`, so freezing just
stops the clock and the displayed/selected world's export *is* the seed. Space again resumes; ↑/↓ still re-pick
which frozen world is the seed.

## 3. Deferred (next slices)

- **Close the loop to Epoch 1 (slice C):** wire the export vector into `flicker-world`'s
  `Epoch1Params.abundance` (`crates/flicker-world/src/world.rs` `ABUNDANCE_DEFS`, `epoch1.rs`) —
  the function + HUD display exist; the cross-crate hand-off does not. (Note: `ABUNDANCE_DEFS` has
  14 symbols and no `Mg`/`Ni`; decide whether to extend it or fold those when wiring.)
- **Cinematic refinement + next features → see §A** (the active work: look-tuning knobs, depth-correct body
  occlusion, 3D Epoch-1 planet spheres, brighter galactic-core bulge, click-to-Epoch hook, perf).
- **Richer asteroid belts.** Belt objects exist (leftover small survivors, classified + rendered + counted)
  and emerge mainly in low-mass disks. A *giant-sculpted* main belt (resonances suppressing accretion in a
  specific annulus, leaving a ring of planetesimals just inside a giant) is not yet modelled — would need
  seeding a separate planetesimal population and/or resonance stirring. (The **gas-budget limit** on giants
  is now **implemented** — see §1.2 — giants are gas-rationed inner→outer, 0–4/system.)
- Background-threaded precompute (currently a synchronous ~150–300 ms hitch on enter/reseed/dial);
  outer disk beyond ~22 AU (evolves too slowly to simulate watchably); rogue-planet *capture* by other
  systems (the user's musing — a body ejected here arriving elsewhere); multi-star ignition.
  `materials[256]` mineralogy is **out of scope** (Epoch 2).

## 4. Verify

`source ~/.cargo/env && cargo build/clippy/test -p flicker-solarsystem -p flicker-render` — **25 tests
(solarsystem) + 4 (render, incl. GPU shader-validation for billboard/sky/volumetric), clippy clean**. The
engine additions are non-breaking: also build `flicker-render -p flicker-world` (0 errors) when touching the
render crate. Mg touched shared data: `cargo test -p flicker-materials -p flicker-worldgen -p flicker-world` green.

Visual (user): `cargo run -p flicker-solarsystem` — a slow cinematic pass: camera opens edge-on under the disk
plane looking through a dark volumetric dust cloud at a star occluded by it (god rays through the gaps),
against a Milky-Way star field; the cloud dissipates inside-out and gaps open at the giants as the system
forms; the camera rises and glides into the inner system; a few protoplanets settle, habitable ones blue-ringed.
Drag for manual camera, ↑/↓ select a world to see its Epoch-1 export, `[`/`]` dial the supernova, `R` reseed.
