# Handoff — `flicker-celestial` evolution / iteration (the per-body epoch lifecycle)

**Read this first for the next session.** It captures a load-bearing scope convergence and the
foundation laid for it. Prereqs: `docs/flicker-celestial-spec.md` (design of record),
`docs/flicker-celestial-data-model-handoff.md` (the model + cloud + viewer rendering that
preceded this), `CLAUDE.md` §5 (the epoch pipeline). Honour the working memories:
`less-code-every-calculation-counts`, `gas-giant-is-liquefied-air`, `worldgen-timeline-system`.

---

## 0. The convergence (the big realization — internalize this)

The world-gen **epochs are not a separate per-planet pipeline; they are the per-body evolution
stages**, and the **solar-system sim is the iteration engine + environmental context the
one-shot epoch pipeline always lacked.** Epoch 1 (composition from aggregation bands), 2
(stratification), 3 (tectonics) — and later 4–6 — run **per body, iterated, on a mega-year
clock**. The celestial sim supplies what a standalone planet bake never had: the **star as a
heat source** (luminosity / distance² → insolation → the temperature that gates molten /
liquid-water / frozen), per-body composition, and **event onsets** (impacts, biological
innovations, extinctions) placed on the MYR timeline.

This unifies four threads into one system: the epoch pipeline (`flicker-worldgen`), the celestial
sim, spec §6 (evolution) + §6d (the time-scale dial), and the existing
`worldgen-timeline-system` (durations + onsets).

### Emergent stages, NO terminations (a locked decision)
A body advances to the next stage **only when its data physically supports it** — there are
deliberately **no per-type hardcoded terminations**. A gas giant simply never satisfies the
requirements past stratification; a hot inner world never cools past molten; an HZ rocky world
keeps qualifying all the way to life. The endpoint is an *emergent property of composition +
insolation*. **Bonus property:** because most bodies stall early, the deep/expensive stages
(tectonics, hydrosphere, life) only ever run on the few worlds that earn them — **the same
emergence that makes it realistic also bounds the compute.** The termination is the budget.

---

## 1. What landed this session (foundation, library-only, tested)

All in `crates/flicker-celestial` — additive, no viewer changes, 39 tests + clippy clean.

- **`model::world::HexWorld`** — a body's surface as a grid of conserved composition: the
  **planet-scale macro-voxel (§8)**, the world-storage foundation. `{ freq, cells:
  Vec<flicker_worldstate::Composition> }`, one element-composition per icosphere cell. Storage
  only — **topology lives elsewhere** (epochs are topology-agnostic). The load-bearing property
  is **conservation**: `transfer(from, to, element, amount)` is the conservation-safe transport
  primitive (the advection / density-sort steps build on it); `total()` is invariant under
  stepping. Serializable, so an evolved planet's state can be captured for the game.
- **`evolution`** (skeleton) — the iteration engine's anchor:
  - `Stage` — the epoch sequence (Aggregation → Stratification → Tectonics → Hydrosphere →
    Mineralization → Biosphere), `next()` for the chain. Advancing is gate-driven/emergent, not
    type-terminated.
  - `BodyEvolution { world, stage, age_myr }` — the per-body lifecycle state; `step(dt_myr)` is
    the seam the sim/worker call (currently advances age only — the per-stage transforms and the
    emergent gates are the next slices).

**Deliberately NOT built** (kept skeletal on purpose, per "less code"): the per-stage transforms,
the advance gates, the worker wiring, the interpolated display, materialisation of a body into a
`HexWorld`, the viewer reading from `HexWorld`. Those are the slices below.

---

## 2. The runtime architecture (how it runs, once built)

```
celestial sim (slow MYR clock, decoupled from the cinematic camera clock)
   └─ per planet: BodyEvolution { HexWorld, Stage, age }
        └─ step submitted to flicker-worker  (pure CPU: produce the next HexWorld)
             └─ main thread polls completed steps, double-buffers the last two states
                  └─ display INTERPOLATES per-cell colour between them as the clock advances
                       (geometry fixed → only colour blends → continuous morph, no snap)
```

Pinned design points (decided across the session — don't relitigate):
- **Reuse `flicker-worker` as-is** (the voxel-cluster pattern: `submit(closure)` + result channel
  + a generation counter to drop stale results). The only new bookkeeping is caller-side: one
  in-flight step per planet. **Do not invent a new worker system.** Workers do pure CPU (step the
  grid / build the colour array); the main thread owns all GPU upload.
- **Two clocks**: the cinematic camera pass keeps its current pace/path (it's already striking);
  the *system simulation* runs slow (§6d dial), spending the freed wall-clock on steps.
- **Blend = interpolate, never swap.** Hold the two latest completed states + a fraction `f` of
  clock progress between them; each frame the planet = `lerp(Sₖ, Sₖ₊₁, f)`; the worker computes
  Sₖ₊₂ ahead, so there's always a target and the sim is never blocked. Only **per-cell colour**
  interpolates (the icosphere geometry is fixed). Cheapest impls, in order of preference for "no
  free frames": small+frequent steps + brief crossfade (viewer-local, no engine change) → CPU
  colour-lerp at a coarse viewer-LOD → a shader `mix(colourA, colourB, f)` uniform (zero
  per-frame rebuild, but touches the *shared* mesh pipeline — treat as an optimization).
- **One step framework, dispatched by composition** (the consistency rule that ran through the
  whole viewer cleanup): `step(grid, dt, ctx) -> grid'` picks density-sort for solids vs zonal
  advection for gas. One function, two rules — never parallel code paths.

---

## 3. The slice ladder (build in this order; each is shippable + visible)

1. **Render the viewer from a `HexWorld`. ✅ LANDED.** `worldglobe::materialize_solid` /
   `materialize_gas` build a body's `HexWorld` (Epoch-1 spread for solids / the gas swirl seeder
   for gas), and `worldglobe::globe_mesh(&HexWorld)` derives the disposable mesh, colouring each
   cell from the **stored** grid (no more inline `seed_hex(dir)` formula). `scene.rs`'s
   `globe_cache` now holds `(MeshHandle, HexWorld)` per body — the stored material truth the step
   will rewrite. No behaviour change on screen; now there's a real conserved grid to step.
   `globe_mesh(&world)` is the canonical "re-derive the mesh from the grid" call slice 2/3 reuses.
2. **Two-state double-buffer + interpolated display. ✅ LANDED.** `scene::Globe { prev, next,
   mesh, shown }` holds two `HexWorld`s per body; `worldglobe::globe_mesh_blend(a, b, f)` colours
   each cell `lerp(colour(a[i]), colour(b[i]), f)` — geometry is identical between states, only the
   per-cell colour blends, so the surface morphs continuously (no geometry change, no hard snap).
   The blend is **quantised** (`BLEND_QUANTA`): the mesh is re-derived/re-uploaded only when its
   level changes, so most frames just redraw the cached mesh (honours "no free frames"). Proven
   with two *static* states — `next` is a proof second materialisation (solid: a re-rolled province
   seed `^ EVO_PROOF_SEED`; gas: a shifted band count), driven by a cinematic-clock ping-pong
   (`blend = ½ − ½cos(anim_time·BLEND_RATE)`, frozen while paused). **Slice 3 swaps in:** make
   `next` the worker's freshly-stepped grid, drive `blend` monotonically from the MYR clock, and
   `next`→`prev` when a step completes (the `Globe` already has the buffers + crossfade for it).
3. **Worker-driven `step`, gas first.** — **TRANSFORM LANDED (library-only); worker + viewer feed
   NEXT.** The conserved gas advection now exists in `evolution`:
   - `evolution::advect_zonal(world, dt_myr, ctx) -> HexWorld` — **real** zonal differential-rotation
     transport via `HexWorld::transfer` (equator faster than poles; every cell sheds a Courant-capped
     fraction of *all* its elements to its eastward neighbour, chromophores riding along). Exactly
     conserved (`total()` invariant — read amounts from the original grid, one outflow per cell;
     proven over 50 steps). Pure, so the worker can run it off-thread.
   - `evolution::step_world(world, dt, ctx)` — the single entry point, **dispatched by composition**:
     gaseous (H+He > ½ mass) → `advect_zonal`; solid → *holds* (Epoch-2 density sort is slice 4 — a
     body simply doesn't run a transform its data doesn't call for). `BodyEvolution::step(dt, ctx)`
     transports + ages.
   - `evolution::StepCtx { dirs, neighbors }` — topology passed in by the caller's icosphere, keeping
     `flicker-celestial` topology-agnostic (the viewer already builds `Sphere { dirs, neighbors }`).
   - 5 new tests (44 total): conservation over 50 steps, eastward-only transport, equator-shears-
     faster-than-pole, polar cells inert, gas-vs-solid dispatch.

   **Visible half — ✅ LANDED (synchronous; worker deferred).** The viewer now *simulates* gas
   giants on screen:
   - `worldglobe::step_grid(world, dt)` bridges the grid's icosphere (`dirs`/`neighbors`) to
     `evolution::step_world` — the viewer's topology↔HexWorld seam.
   - `scene::Globe` advances **forward**: `next = step_grid(prev)`; the displayed colour crossfades
     `prev`→`next`, and when it completes `next` becomes `prev` and the next state is stepped in.
     Because the colour at blend 1 *is* the new `prev`, steps are seamless (no snap). Pace from
     `EVO_RATE`/`EVO_STEP_MYR` on a per-frame `evo_dt` derived from `anim_time` (so it freezes with
     playback). The slice-2 `EVO_PROOF_SEED` placeholder + ping-pong are gone.
   - **Dispatch shows through:** only gas giants evolve (real conserved advection — the swirl is
     transported, drifting east, equator faster); **solids hold static** (`evolves=false`), by
     design, until their density-sort transform (slice 4). Moons are solid → static.
   - The painted `gas_seed_hex` is kept as the **initial condition S₀** (not re-painted per state) —
     a seeded initial swirl is consistent with `gas-giant-is-liquefied-air`; only the *evolution* had
     to become real, and it has.

   **Deferred (not blocking):** (a) move `step_grid` onto `flicker-worker` (it's pure; cheap enough
   inline now — the mesh rebuild, not the step, dominates); (b) optionally re-home S₀ to *blobby
   noise* so belts form purely from advection rather than starting pre-banded — a look-tuning call to
   make with the user watching, since the long-run attractor of pure zonal advection is clean
   latitudinal bands (longitudinal structure smears out; no meridional transport / turbulence yet).
4. **Solid stratification = `Epoch2`, carrying `HexState`. ✅ STEP 1 LANDED.**

   **Scope decision (the user's, locked):** the **system sim evolves every body through Epoch 3**,
   then **pegs**. Gas/ice/molten worlds reach their emergent final state and stall; rocky/HZ worlds
   get rough geography (E2 differentiation + E3 plates). A selected **HZ** planet's Epoch-3 state is
   the **starting point for the single-planet sim** (`flicker-world`), which runs Epochs 4–9
   (hydrosphere → veins → life). So the mineable veins (E5) live in the *single-planet* sim — the
   system only lays the plate/fault structure they later follow. The rocky stages **reuse the
   `flicker-worldgen` epoch transforms** (parameterised by their `duration`/maturity knob), not a
   reinvented rule (the convergence).

   **Carrier moved `HexWorld` → `Vec<HexState>`** (the epochs' working type; `HexState.composition`
   is the Epoch-1 bulk). Orchestration lives in `flicker-solarsystem` (it has both
   `flicker-worldgen` + `flicker-celestial`); `flicker-celestial` stays lean (model + the gas
   advection, which now runs on `HexState.composition`). What landed:
   - `worldglobe::materialize_solid`/`materialize_gas` → `Vec<HexState>`; `globe_mesh_blend` colours
     from `HexState::surface()` (the differentiated crust once it exists, else the bulk).
   - `worldglobe::step_solid(prev, tables, freq, seed, settle)` → `Epoch2` at a crystallisation
     `settle` ∈ [0,1] (mapped to `duration` = `settle·NOMINAL_DURATION`). Bulk untouched; only the
     derived crust changes → re-running with a growing `settle` matures the differentiation (iron
     drains out of the surface, conserved in the bulk below). `step_gas` advects the composition.
   - `scene::Globe` now holds `Vec<HexState>` + `freq` + `seed` + `age_myr`; **every body evolves** —
     solid differentiates (settle grows with age, completes at `DIFFERENTIATION_MYR`, then holds),
     gas advects. Test `differentiation_drains_iron_from_the_surface_but_conserves_the_bulk`.

   **Rendering perf (load-bearing — don't regress):** an evolving globe must NOT rebuild the
   icosphere or render at the full `hex_freq`. `worldglobe::Topo` caches the icosphere (`dirs` +
   `neighbors` + `outlines`) **once** per body (in the `Globe`), reused for every step + mesh
   rebuild; and the evolving display is capped to a coarse **`VIEWER_EVO_FREQ` (32)** — the full hex
   budget (Earth ≈ 100) is for materialization, not the viewer. (Without these, rebuilding a 100k-cell
   icosphere per crossfade-step per body dropped the frame rate to ~1 fps — a slow-frame→bigger-step
   feedback loop.) The per-rebuild cost is now just a colour pass + a small upload, only on blend-level
   change.

   **Rendering model (the user's art direction, locked + landed):** *data is cheap, drawing is the
   cost.* The simulation iterates per-cell math freely; the **drawing is art-directed and capped** —
   it must never push geometry to the GPU per frame. So:
   - **Flat-shaded hexes, no crossfade.** `worldglobe::globe_mesh(state, topo)` builds one flat
     state's mesh; the evolution is shown by **swapping** to the next state's mesh, not blending.
     (`globe_mesh_blend`/`lerp3` removed.)
   - **Cached icosphere.** `worldglobe::Topo` builds `icosphere_with_outlines` **once** per body and
     reuses it for every step + mesh build — the per-frame icosphere rebuild (which craters frame
     rate at Earth's ~100k cells) is gone.
   - **Art cadence.** `scene::Globe` holds a flat state for `STEP_INTERVAL` real seconds, then takes
     one cheap sim step + one mesh swap (≤1 upload/frame, none between steps). At `EVO_COMPLETE_MYR`
     the body **pegs** (static — the pegged system is the in-game night sky). This killed the
     ~1-frame/3-s stall.
   - **Hex budget corrected** (`flicker-celestial::hex`): `freq` is now strictly **∝ radius** (Earth
     100 → Mercury ~38; the old two-anchor line was inconsistent). Gas giants are **no longer a
     static 48** — `hex_freq_for_giant` = half a solid's count at that size (≈2× hex size, coarse;
     never rendered as a detailed world). Body display sizes halved (~2× too large).

   **Next:** the **procedural readiness gate** the user specified — iterate each epoch *for many
   cycles* and advance when **converged** (per-cycle change below threshold, cap ~100) **and
   eligible** (data supports the next epoch; gas/molten/ice stall) — replacing the fixed
   `EVO_COMPLETE_MYR` peg. Then **`Epoch3`** (tectonics) as the next stage. See the
   `system-sim-epoch3-cutoff` + `celestial-evolution-simulate-not-paint` memories.
5. **The emergent advance gate** (`can_advance(world, ctx) -> bool` per stage, driven by
   insolation + composition) and the MYR clock + §6d per-phase step sizing.
6. Later: Epochs 3–6 as further stages; event onsets on the timeline; reconcile with
   `flicker-worldgen`'s epoch transforms (reuse them as the stage steppers rather than
   reimplementing — they consume `EpochCtx { dirs, neighbors, seed }`, which the icosphere
   already provides, incl. `Sphere.neighbors` the viewer currently ignores).

---

## 4. Invariants / decisions to honour

- **Conservation** every step: material *moves* between cells, never created/destroyed
  (`HexWorld::transfer` + `Composition`'s add/remove enforce it). `total()` is the check.
- **Determinism per seed**: steps must be reproducible regardless of worker timing, or freeze /
  Epoch-1-seed-lock isn't reproducible. Freezing pauses the *evolution* clock; the seed-lock
  captures the *current evolved* grid.
- **Emergent, not terminated** (§0): no per-type stops; gate on the data.
- **Lean**: one `step`, the generic worker, colour-only blend, reuse the epoch transforms +
  timeline/onset system + celestial model. The genuinely new code is the per-body lifecycle
  driver + making the epoch transforms *iterable* (some are one-shot bakes today).
- **`flicker-celestial` stays GPU-free**; the viewer consumes it.

---

## 5. Where things live

- Storage + lifecycle: `crates/flicker-celestial/src/model/world.rs`, `.../evolution/mod.rs`.
- Hex budget (freq from radius; giants pinned 48): `crates/flicker-celestial/src/hex.rs`.
- The viewer that will consume it: `examples/flicker-solarsystem/` (`scene.rs` render loop,
  `worldglobe.rs` build/seed, `material.rs` chemistry). Currently renders globes from per-cell
  formulas + the enriched element palette / pigment-weighted composed colour.
- Epoch transforms to reuse as steppers: `crates/flicker-worldgen/` (`epoch1..6.rs`).
- Topology (dirs + **neighbors**): `crates/flicker-worldgrid/` (`icosphere_with_outlines`).
