# Handoff — The Material Model & the Rivulet Sediment Simulation

> A standalone handoff for a fresh context. It captures the world-material data
> model and the simulation that runs on it, worked out over a design thread.
> Re-verify code/data anchors (names drift). Builds on and concretizes
> `docs/flicker-world-system-spec.md` §4 (material ledger), §5 (erosion sweep),
> §6 (Rivulets); `docs/clayengine_world_generation_spec_v2.md` (the epoch
> pipeline); and the canonical materials reference in
> `~/Repos/elide-us/Prism/BookIII.md`. Supersedes nothing; sits *upstream* of
> `docs/water-cycle-handoff.md` (which is the vertical band/water-cycle POC —
> right physics, wrong execution model; see §4).

---

## 0. Why we're here (the pivot)

We had built a runtime water cycle (`examples/hex-world`, Steps 1–3) and a Lua UI
with World/Local views. Then we stepped back: the data model showed **shape**
(stacked heightmaps) and **influence** (heatmaps) but had **no material
composition** under it — `ground` was a bare scalar. So the cycle "showed there
is data" but wasn't the right model, and three things were premature:

- Bridging to voxels (a lower container fed by the *wrong* data).
- Adding more to the cycle (cross-hex halo, etc.) before it carries anything.

**The through-line that organizes everything below:** material model
(composition + traits) → enables **Rivulets** to **move sediment** → which is
the *entire point* of the water cycle. Water cycle is meaningless without moving
sediment. So we build the material foundation first (epoch 1–6 territory: how a
planet gets made of stuff), then the erosion/rivulet engine that moves it.

---

## 1. The data model — four tiers

From vocabulary down to render:

1. **Reference / metadata tables** — static JSON, authored, never simulated
   (`data/materials/*.json`; see §2).
2. **Aggregate ledger** — the per-pixel sim truth: a **composition vector**
   (element → absolute amount), derived aggregate traits, and effect state.
   **Surface-only, sparse** (§3, §4).
3. **Sparse voxel data** — low-level detail, materialized only inside the active
   player bubble; each voxel's contents derive from its cluster's aggregate +
   local variation. (The "join" target.)
4. **Render material** — `voxel composition → classify() → one of 256` material
   ids. Regenerable, never stored as truth.

**The join:** voxel (③) → reads → cluster aggregate (②) → classified by →
material signature (①) → render material (④). The **aggregate is the hinge**;
the simulation only ever touches ②.

---

## 2. The vocabulary (built — `data/materials/`)

- **`periodic_table.json`** — 26 elements from Prism BookIII. *Book fields*
  (name, number, symbol, category, valence_electrons, uses — verbatim; valence
  is the book's gameplay value, not IUPAC). *Physical fields* (atomic_mass,
  density_g_cm3, melting_point_c, state — real-world; epoch 1 distributes by
  mass/abundance, epoch 2 differentiates by **density**). *Trait fields*
  (hardness, brittleness, water_capacity — proposed starting values).
- **`materials.json`** — the **256-index material table**; 20 resolved so far,
  rest reserved to grow. A material is **not** an element — it's what an
  aggregate composition **classifies to** (granite, sandstone, limestone, shale,
  dirt, clay, sand, gravel, peat/bog, hematite/copper/gold ore, coal, water, ice,
  lava, oil). Each row: `category`, `signature` (defining elements, most→least),
  `hardness`, `brittleness`, `water_capacity`, `viscosity`, `density_g_cm3`,
  `color`, and `extracted_element` for ores.

**Composition is absolute amounts** (e.g. `Fe 7000, Si 8000, C 20000`), not
fractions.

**The four traits and what they drive:**
- `hardness` (0–10) — erosion resistance; sets how easily a rivulet lifts it.
- `brittleness` (0–1) — fracture → **sediment generation** (loose carryable mass).
- `water_capacity` (0–1) — porosity; how much water a material holds (capacity;
  the *current* saturation is per-cluster sim state, not here).
- `viscosity` (0–1) — flow-effect motion rate **per pass**: water ≈ 0.05 (moves
  every pass), oil/lava mid, ice ≈ 0.95 (creeps). Solids are static (move only
  via erosion).

A cluster's effective trait = composition-weighted blend of element traits;
formed **materials override** with authoritative values.

**Future plumbing (designed-for, not built):** a `TableSource` abstraction loads
these tables — JSON file today, **flicker-net → web service → DB** later. The sim
asks the source; it never hardcodes a path. (`flicker-net` is an empty stub now.)
Likely a `materials` crate owns the vocabulary + loader; the epoch pipeline a
`world-gen` crate. A `compounds.json` (BookIII's chemistry: Fe₂O₃→Fe etc.) is
still to transcribe — mostly a crafting/extraction reference; the world sim works
element→material directly.

---

## 3. Aggregation (256³ voxels → cluster ledger)

A cluster = a 256³ voxel cube (a 128-ft cube). Each voxel is a container of an
element distribution. The **cluster aggregate = Σ over its voxels = the conserved
total mass-per-element** in that cube. Directionality is the architecture:

- **Down (materialize):** aggregate → distribute mass across voxels + local noise
  → each voxel classifies to a material. Feeds the low-level render.
- **Up (write-back):** a player edits voxels → re-aggregate the cube → the ledger
  updates. Same door as geology (spec §4).

The sim never iterates voxels — one composition vector per cluster keeps per-pixel
accounting tractable.

---

## 4. Simulation semantics — the big reframe

This is **not a live/continuous simulation.** It is **pass-based accounting**:

- A hex is updated **once per pass** (the rolling sweep over the planetary array —
  see `docs/water-cycle-handoff.md` "execution model" + the spec §5 erosion
  sweep). Each pass produces a **new static snapshot**.
- **Rendering is a *read*** of the current state, never a live solve. When a
  player walks into a hex, we tell them what the sim currently says it looks like.
  When they leave, their edits **reconcile back on the next pass**. "It's not
  live, in truth — we know what's water, but it doesn't change live as people are
  in the world." The slowness is the only reason a "water simulation" is
  affordable at world scale.
- **The ledger is surface-only.** We calculate the **surface**; everything below
  is **100% solid material** (the column's bulk composition), materialized
  **sparsely as players dig/reveal** in any direction. A pixel's sim data is its
  surface composition + bulk + effect state — not a 256-deep vertical profile.
- **Water / ice / lava are *effects*, not layers** — persistent state with
  **viscosity-gated per-pass motion** (water moves every pass; ice/lava slower).

**Consequence for existing code:** the `LayerStack` water cycle (hex-world Steps
2–3) has the **right physics** (evaporate → lift → condense → runoff) but the
**wrong execution model** (it ticks *continuously per frame* on *bare scalars*).
It must be re-homed: **one step per pass**, operating on the **composition +
saturation ledger**, render = snapshot read. The physics is reusable; the
invocation and substrate change.

---

## 5. Rivulets — the sediment conveyor (the centerpiece)

> Water is a **separate system**; this is where Rivulets play out. Its **primary
> purpose is to MOVE SEDIMENT** and reshape terrain. Water is just what we draw
> along the route.

**Structure:** a Rivulet is a **linked list of water voxels** — a conveyor belt
for mass.

- **A water voxel is a data flag, not motion.** A non-solid surface voxel that
  crosses a **saturation threshold** (from rain) *becomes* a water voxel; the
  engine draws it as water. It does not actively move.
- **One rivulet defines ONE voxel.** Water **depth is stacked rivulets** — enough
  saturation for two water voxels = two rivulets, which the renderer reads as
  2-high water.
- **Formation searches neighbour clusters** to **join an existing rivulet or
  start a new one**. (This is also how cross-hex/cross-cluster coupling happens —
  the rivulet *is* the horizontal "halo" the earlier handoff parked.)
- **Aggregation:** rivers and lakes are **aggregated rivulets**; promote them to
  coarser "pipes" for accounting. Everything ultimately drains toward the ocean.
- **Ocean = an abstract infinite source/sink.** We don't track its volume:
  evaporation comes *from* it, water drains *back* to it. Keep it consistent as
  the water cycle's source/sink.

**Transport = erosion (the point):**
- The rivulet erodes **easily-sedimented composition** from its **source
  cluster** (e.g. silicon) — *erodibility* derives from the traits (low hardness
  + high brittleness).
- Moves it **O(1) to the tail** (copy origin → tail pointer; spec §6).
- **Deposits** at the tail — adding mass to the tail aggregate, which can
  **form NEW SOLID VOXELS** once it accumulates enough (materialize → classify
  via `materials.json`).
- Conserved at confluences: reconcile **quantities**, not geometry (spec §6).

**Dynamics (snake-like, pass-based):**
- **Terminus types:** *outflow* (ocean / map edge), *sink* (lake/basin holds
  volume), and **evaporative** — heat at the tail evaporates it away, at which
  point we **drop whatever that rivulet was carrying at that cluster** (deposit
  the sediment) and the list **shortens** by one.
- **Losing the head** (rain stops) doesn't end it — we still trace the rivulet
  down the hill. It **moves like a snake**: rolling head-down toward a lower
  neighbour voxel while drying at the tail. Because the sim is slow/pass-based we
  do this **only when the hex is batch-processed**, never in real time.

**Player interaction:** capturing a water voxel yields a **bucket of water +
whatever sediment that voxel carried**. *How much sediment* is TBD — a fraction
of the linked-list's length? a fraction of the per-pass sediment-movement calc?

---

## 6. Open questions (TBD — flagged, not guessed)

- **Carry parameters** — what/how much sediment a rivulet voxel holds and moves
  per pass; capacity vs. flow.
- **Start/end determination** — how a rivulet's source and terminus are chosen
  (saturation gradient? downhill trace? basin detection?).
- **Termination logic** — ocean outflow vs. sink accumulation vs. evaporative
  recession, and the deposit rules for each.
- **Erodibility formula** — the exact function of hardness/brittleness/saturation
  → mass shed per element per pass.
- **Rivulet maintenance** — reroute/merge/capture cost as the terrain the rivulet
  carved invalidates its own path (spec §6: *this*, not transport, is the real
  cost and what determines whether it scales).
- **Player-capture sediment amount** — the bucket question above.
- **Material classifier** — composition → one of 256 (nearest-signature vs.
  explicit threshold rules; probably rules author the signatures).
- **Ledger granularity & sparse storage** — surface representation, how
  "default → materialize on touch → write-back" is stored.
- **Compound rules / `compounds.json`** — element→compound formation (heat +
  attractiveness) if/when the sim needs explicit compounds vs. element→material.

---

## 7. Where this sits in the bigger picture

- **Epoch 1–6 (planet formation / material distribution)** produces the
  foundational composition maps this model runs on — the gap "from zero to a
  planet that looks like a planet." Epoch 1 (seed per-hex composition) is the
  first runnable kernel.
- **This material model is the substrate; Rivulets are the erosion engine**; the
  pass-based sweep is the clock.
- **Deferred (do not build yet):** the bake-to-voxel bridge (`impl Primitive for
  LayerStack` → `voxel-cluster`); the full epoch pipeline beyond epoch 1; the
  crafting compound catalog. The cross-hex halo is now subsumed by rivulets
  joining across neighbour clusters.

---

## 8. Next concrete steps

> **Status** (implementation progress + decisions: `docs/material-model-impl-handoff.md`):
> steps 3–5 are **built and well surpassed** — crates `flicker-materials`,
> `flicker-worldstate`, `flicker-worldgen` exist, and the epoch pipeline now runs
> **Epochs 1–4 with real formation physics** (composition → differentiation →
> plate tectonics + orogeny → hydrosphere) with per-cell hardness/terrain fields,
> all rendered in the `hex-world` stack viz. Epochs 5–6 then **step 6 (re-home the
> water cycle)** are the remaining arc.

1. **This doc locks the model** (the "define the boundaries" deliverable). ✅
2. **`compounds.json`** (world-forming subset) if/when needed. *(deferred)*
3. **`TableSource` loader** reading `data/materials/*.json` (the JSON-now /
   network-later seam). ✅ — `flicker-materials`.
4. **Aggregate ledger schema** (②): per-pixel `{ composition: {El: amount},
   bulk_composition, surface_material, effects: { water_saturation, ice, lava } }`,
   sparse. ✅ — `flicker-worldstate`.
5. **Epoch 1** — seed per-hex composition from the tables; render a dominant-
   element/material tint per hex to *see and verify* the distribution. ✅ —
   `flicker-worldgen` + `hex-world` stack viz. (Per-cell **materialization** —
   resolving each hex toward its 2048² detail — is the next refinement.)
6. **Re-home the water cycle** onto the ledger + pass-based stepping (§4), then
   build the **Rivulet** structure on top (§5). *(pending — the sim is kept and
   the tier-② substrate now exists)*

---

## 9. Decided vs deferred

**Decided (load-bearing):**
- Pixel = material ledger (composition vector + traits); shape derived; mass
  conserved (spec §4). Composition = absolute element amounts.
- Surface-only ledger; below = 100% solid bulk; sparse materialization on reveal.
- Pass-based accounting, render = read; **not** a live simulation.
- Water/ice/lava = effects with viscosity-gated per-pass motion, not layers.
- 256-material index, limited resolved set + room to grow; materials = classified
  blends, not elements; ores map to an extracted element.
- Four traits: hardness, brittleness, water_capacity, viscosity.
- Rivulet = linked list of water voxels (1 voxel each; depth = stacked); forms on
  saturation; joins/creates across neighbour clusters; conveyor that erodes →
  O(1) transports → deposits → can form new solid voxels; aggregates into
  rivers/lakes; drains to an abstract infinite ocean source/sink.
- **The purpose is sediment movement / terrain change**; water is rendered, not
  the point.
- Vocabulary fed from JSON tables now (`data/materials/`), `flicker-net` → DB
  later via a `TableSource` seam.

**Deferred / TBD:** everything in §6, plus the bake bridge, the full epoch
pipeline, and the crafting compound catalog.
