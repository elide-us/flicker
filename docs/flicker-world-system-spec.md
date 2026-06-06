# Flicker — World System Architecture & Unifying Model

**Status:** Vision / architecture spec. Authoritative on the *model* and the *decisions*; intentionally silent on implementation specifics (struct layouts, exact algorithms, numeric tuning) except where a number was an explicit design decision. Implementation is Claude Code's job; this document is the shape that work has to fit inside.

**Audience:** Claude Code, and any future contributor who needs to understand why the engine is built the way it is before touching it.

**Scope:** The world system (planet-scale generation and simulation), the water cycle (Rivulets), and the relationship between the simulation's data and the geometry the renderer draws. The existing voxel/cluster/contour/LOD-meshing work is *downstream* of everything described here and is referenced but not re-specified.

---

## 0. The Unifying Model

One principle governs every layer of this engine:

> **The data is the truth. The rendered world — geometry, meshes, voxels, water surfaces — is a downstream, regenerable representation of that data. Persist only what cannot be regenerated.**

This is the same thesis that runs through the rest of the project ("the data is the application, code is a render-only target"), pointed at a planet. Its consequences are concrete and load-bearing:

- **Shape is disposable.** Terrain geometry is a *function* of world data, evaluated on demand. It is never the asset. It can be thrown away and recomputed, and it routinely is.
- **Mass is conserved.** The thing the simulation actually owns and protects is material quantity, not contour. Conservation laws live on quantities, never on geometry.
- **Physics is free; meaning is expensive.** Because terrain, water, erosion, and procedural content are all derivable, they cost almost nothing to keep — you keep the compact source and regenerate the rest. The only things that must be durably stored are the things a model cannot recompute: what specific people did. Player claims, the things they built, NPCs, family history, secrets, character progression.

The engine's entire job is to make the first category (derivable world) so cheap that the project can afford to be precious about the second category (irreplaceable record). Every architectural decision below is an instance of this principle. When a decision seems to violate it (e.g. claims keeping exact shape), that's the signal that you've found something in the "meaning" category, and the exception is deliberate.

---

## 1. Scale Hierarchy

The world is composed top-down through nested scopes. Each scope up is a coarser aggregate; the finest scope (voxels) is a *render-time* materialization, not stored truth for most of the world.

- **World** — an array of hex maps. The persistent universe.
- **Hex map** — a single 2048-wide heightmap. One renderable world zone and the base unit of streaming/loading. Each valid pixel is one cluster column. The 30-degree corner pixels are ignored; pixel selection is explicit (no alpha blend), and each chosen pixel is exactly one heightmap variable in the world map.
- **Pixel / cluster column** — one heightmap pixel maps to one 256×256 voxel cluster footprint, covering 128 sq ft of world surface. A pixel is **not** a height value with metadata attached. See §4 — it is a material ledger.
- **Cluster** — the voxel shape-compression domain. This is the existing engine work: contour, QEF dual contouring, corner vectors, LOD strides, watertight seams. The cluster system is a *renderer* for the data this document describes.
- **Voxel** — the finest unit; a container of materials rendered through the materials system.

### The LOD8 closure

The cluster map left an unfilled slot: the LOD8 cluster vector was never solidified. **That slot is filled by world data.** The world heightmap pixel *is* the LOD8 cluster vector. It was never the cluster system's job to produce — it originates here, as the per-column macro state, and is consumed by the cluster mesher as its coarsest input.

This is the single seam between the world system and the voxel system. They are otherwise decoupled — which is correct and healthy. The macro world emits the LOD8 backdrop; the voxel system refines detail upward from it on demand. Same data-is-truth / render-target pattern, one scope up.

---

## 2. World Topology

The world is a one-dimensional, polar-symmetric array of hexes.

- **Index 0** is the north pole — a single hex.
- **Ring N** contains **6N** hexes.
- After **R** rings the array reaches the equator (the widest ring, 6R hexes), then mirrors: rings shrink back down to a single hex at the **south pole**, the last element.
- **Total hex count = 2 + 6R².** R is the world-size parameter; doubling R roughly quadruples the planet.

### Orientation and neighbors

All hexes share one canonical orientation (flat side "up" = toward the pole for that spiral). Neighbor resolution uses two formulas only:

1. **Within-spiral** — pure arithmetic on the array index plus ring topology. No stored orientation; it is computed from position.
2. **Equator fold-mirror** — the "fold the page in half" model: the world is a flat 2D hex map that spirals out from the north pole to the equator, then folds; a hex on the equator's edge has its cross-fold neighbor at the mirror position on the opposing spiral. This is the only genuinely tricky seam.

The poles are the special case (one hex bordering all 6 hexes of ring 1). Eliminating an explicit equator *band* (so the two outermost rings touch directly through the fold) remains an acceptable simplification if it makes the mirror math cleaner.

### Rendering extent

Rendering is hex-local and small: no more than ~3 hex-diameters total. Standing in the center of one hex renders that hex plus the six around it (~7 hexes), most of it at the highest LOD only near the camera. The planet is enormous; the rendered slice is tiny.

---

## 3. The Epoch Pipeline (World Generation)

World generation runs **offline**. The runtime never executes epoch logic; it only reads the materialized output. Macro (per-hex) generation is cheap (thousands of hexes); per-column materialization is the expensive part and is deferred (see §8).

Nine epochs, in three groups of three. Each group operates on different layers/aspects of the world map:

**Epochs 1–3 — Molten.** Start condition: the planet has cleared its band of matter after star formation. Navier-Stokes matter distribution, filtering, sorting by hardness and atomic attractiveness, heat distribution, compound formation. Includes matter-addition events (consumed co-orbitals; possible moon-forming impacts). Astrophysical inputs are the seed parameters: orbit, satellites, eccentricity, orbital offset, wobble.

**Epochs 4–6 — Geological.** Early life, sediment formation, cooling crust, continent compression, mountain ranges driven into material clusters from the molten epochs, weighted ocean plates, underlying hot zones, and dozens of major erosive cycles continuing to contour the map. Layered features (e.g. chalk-cliff strata) form here. These epochs use the desired starting conditions as *targets* that constrain the molten-epoch generator — i.e. generation is goal-directed, not purely forward-simulated.

**Epochs 7–9 — Real / persistent simulation.** This is the retained "real world" data — the only state the server keeps and shares. Heat, evaporation, atmospheric pressure, condensation, and erosive sediment distribution form a complete erosion cycle (harder materials erode less). These epochs produce:

- **Volcano drivers.**
- **Deep resource vein zones** — massive (planetary-scale, potentially spanning dozens of zones, effectively infinite depth), a server-lifetime supply once found. Generated through continued epoch passes; GM-placeable for gameplay reasons (e.g. exotic gold deliberately placed underwater as a challenge).
- **The three-layer column** (see §3.1).

### 3.1 The three-layer column ("the three dots")

Each cluster column resolves to three stacked layers:

- **Top (epoch 9) — game world ground.** The rendered terrain; what the bake demos work on. Material distribution is blended upward from below by the erosion simulation (e.g. silicon + iron brought together from lower layers yields the corresponding rock type up here — gameplay-logical material emergence).
- **Middle (layer 8) — resource / lava layer.** Resource veins and the underground adventure zone: players harvest materials and build (bunkers, etc.). Its shape is driven by the motion and zones below it.
- **Bottom — death zone.** Not player-travellable (you die/melt going too deep). The GM playground: GMs spawn hot zones here that produce procedural results in the layers above. Causation is bottom-up — GM control at depth → procedural emergence at the surface.

### 3.2 Source-of-truth inversion (important)

Everywhere else in the project the recipe is truth and the artifact is a lossless render. **The world inverts this, deliberately.** The generator is chaotic (Navier-Stokes plus many erosion cycles — not cheaply reproducible across hardware/threads), and the world is mutable after birth (GM edits, player edits, ongoing simulation). Therefore:

- **The baked epoch-7-9 state is the source of truth.** The seed + parameters are a *birth certificate* (how the planet was born), **not** a regeneration recipe (how to rebuild it). The world is not regenerable from seed after generation.
- **Epochs 1–6 are discardable scaffolding.** Once they have produced the epoch-7-9 starting state, they can be thrown away.

This is crystallization-with-provenance where the crystallization is *irreversible*: the rendered world becomes the truth, and provenance documents lineage rather than enabling rebuild.

---

## 4. The Material Ledger (what a pixel actually is)

Each heightmap pixel is a **material ledger** — a composition vector plus trait fields:

- Amounts of each material and compound present in that column.
- Trait fields: hardness, brittleness, water saturation, and the other dimensional properties that drive the simulation.

**Shape is a pure function of the ledger, evaluated at bake time.** The heightmap height is just the most visible scalar face of this balance sheet. The shape is the report; the ledger is the company.

### Conservation invariant

- The conserved truth is **mass per material**. Shape is regenerable noise.
- A player interaction is a **transaction against the vector**, not a recorded geometry. Digging 150 voxels of dirt means the column's dirt scalar drops by 150. The *hole* is not data. The next bake derives whatever shape that smaller quantity wants under current conditions — it will likely bear no resemblance to the player's original carve, and that is correct. We generate the truth for that column as it lives now.
- **The conservation requirement lives on the bookkeeping, not the geometry.** The erosion sweep moves material between columns and splits compounds by trait; that arithmetic must conserve and account honestly, or the world slowly leaks or breeds matter over geological time. Shape can differ every bake; mass cannot drift. This is the invariant that survives every simplification in this document.
- **Player edits and slow geology enter the truth through the same door.** A player digging 150 dirt and a river moving 150 dirt are the same kind of event to the world layer — a change to material quantities the next erosion sweep integrates and redistributes. There is no special code path for "player did it" vs "physics did it," because once shape is discarded they are identical at the level the truth is stored.

---

## 5. The Erosion Sweep (the centerpiece)

The erosion sweep is **the only system that evolves the truth**. It is not a maintenance/GC chore that happens to also do cleanup — it is the heart of the world simulation. Ledgers are inert between sweeps. The sweep:

- Takes the epoch-scale macro flows still in motion (continents splitting to form oceans, glaciers and rivers and geysers and volcanoes carving valleys, hot zones pushing material up) and **redistributes material across columns** according to the trait fields.
- Runs **slow** — on a geological cadence, never in the frame loop.

### Degeneration as write-back (the GC routine)

"Degeneration" is the reclamation routine that runs when players are **not** near. It is a **write-back / promotion**, not a discard and not a shape-preservation:

1. When the erosion sweep next reaches a column that a player modified, it **re-aggregates the material change back into world data** (e.g. the column now contains 150 less dirt) and rewrites the heightmap pixel.
2. The baked LOD0 data for that column is then **freed**.
3. The next time a player arrives, a fresh bake is generated that is volumetrically consistent with the updated ledger — same world, minus the dirt that was removed, plus whatever the erosion sweep did to it in the meantime.

**Only material aggregates are kept; CSG/shape history is discarded.** The player's specific excavation shape is gone the moment the sweep launders it into the ledger. This is what makes player edits and geology unify into a single species of change.

---

## 6. The Water Cycle — Rivulets

Water is hard, so it gets its own structure. Liquid is a voxel material type; water saturation is a trait field alongside hardness and brittleness. A surface "air" voxel accumulates moisture; past a saturation threshold it becomes water and joins a Rivulet.

### What a Rivulet is

A **Rivulet** is a flow-routing structure that doubles as a transport optimization. It stores the *route* water takes — not the water sitting in place. The payoff: moving sediment along a chain of arbitrary length collapses to **one operation** — copy from the origin data point to the tail pointer. Transport cost is O(1) regardless of chain length. This is the same instinct as the rest of the engine: don't simulate the volume, simulate the boundary — here the boundary is the endpoints of a flow.

Water sits as a **layer above the terrain heightmap** at the macro scale. **Lava and ice are tracked the same way** as parallel flow fields with different rate constants (lava: high viscosity, high threshold, freezes its own channel; ice: creeps rather than flows and carries its load frozen-in — ice may ultimately need its own structure rather than the rivulet abstraction; deferred, see §14).

### Structural requirements (where the naive version breaks)

- **The structure is a DAG of segments, not a plain linked list.** A singly-linked chain models a straight segment, but real drainage needs: **confluences** (tributaries join — a node with multiple parents, one child), **deltas/fans** (one parent, many children), and **sinks** (lakes / endorheic basins where flow stops and accumulates — a terminus with no downstream pointer). Linked-list is the right mental model only for the straight runs *between* explicit nodes. Terminus types are **outflow** (ocean / map edge) and **sink** (basin that holds volume). A traversal that assumes every node has a downstream pointer will fault on a basin — the most common terrain feature there is.
- **Maintenance, not transport, is the real cost.** The sweep that moves sediment is the same sweep that reshapes the terrain the rivulet was routed over — deposition aggrades the terminus, erosion incises the source, capture merges neighbors. The structure's topology is invalidated by its own output. Transport is O(1); **reroute / re-derivation is O(reroute)** and is what determines whether this scales.
- **Conservation lives or dies at the merge.** Every confluence is an addition that must balance. Merging two rivulets mid-sweep after both have executed their transport can double-count or drop the shared sediment — exactly the slow matter leak the ledger model forbids. **The merge must be transactional against the material vectors (reconcile quantities at the confluence), not against the flow geometry.** Same lesson as §4: account on the conserved quantity, derive the geometry.

### Water as a query and as geometry

Because water is a layer over the ledger and not placed voxels, "where is there water" is a cheap query over the rivulet structure: every node is wet, every sink is a body of water. Determinations of what is water, how deep, and where it runs are largely deterministic functions of saturation plus the flow field. More water in a column → deeper water → a higher water-surface value. The water surface is a **second heightmap stacked on the terrain heightmap**.

---

## 7. LOD & Materialization (the sparse pyramid)

The previous invariant — "cluster data must be baked to LOD0 on generation" — is **replaced**. It was a conservative crutch: a sufficient-but-overkill way to guarantee the real invariant.

### The decision: sparse, generate-on-approach

- **The resting state of the entire world is LOD8**, derived directly from the world heightmap data (easy).
- **Materialization is sparse and on demand.** Finer LODs, up to LOD0, are generated only as a player approaches — and only as close as the player actually gets. We do **not** pre-generate LOD0 everywhere within horizon distance. The contents of a cluster are derived from **world data**, not stored cluster data, so they can be generated on the spot whenever needed.
- **The separate all-LOD8 backdrop layer is absorbed.** It is no longer a special second structure — it is simply the default, unrefined state of every cluster nobody is standing near. One model: the world is uniformly LOD8 except for a small refined bubble that follows each active player.

### Two operations, not one

The LOD0-everywhere rule hid the fact that there are two fundamentally different operations:

- **Decimate** — stride down from stored fine data to a coarse view. Cheap, lossless from the stored truth. This is the consumer-side LOD filtering already implemented.
- **Generate** — synthesize finer detail that was never stored, from world data. This is the new operation sparse storage introduces, and it carries different guarantees.

### Coherence requirement (relocated)

Refinement must be **additive octave-stacking**: the heightmap is the base octave; each finer LOD *adds* one band of higher-frequency detail on top, never regenerates from scratch. This guarantees "stride down from LOD0" and "generate directly at LODn" produce the same answer by construction (LODn is LOD0 with the top octaves truncated) — the same band-limited principle that keeps a synthesizer's oscillators from aliasing.

**However:** because shape is disposable (§4), this requirement is *not* about geometry popping — a generated gully may legitimately differ each bake and nothing is lost. The requirement **relocates to the mass/material accounting**: the write-back (§5) makes generation recursive (a change becomes heightmap → heightmap regenerates the baseline). If the generator is not a clean band-limited pyramid in its *quantities*, every erosion sweep is a lossy re-encode and the world's material accounting drifts over geological time. **Coherence is a conservation property of the ledger, not a popping property of the contour.**

### The seam, and why it survives

The ≤1-LOD seam assert is untouched. Materialization follows concentric rings one level apart (LOD0 core stepping up through 1, 2, 3 … to meet the surrounding resting LOD), so the generated baseline satisfies the assert automatically.

### Timing drains the urgency

Radius doubles per LOD and detail halves, so LOD ring crossings are geologically far apart. A player crosses, say, the LOD7→LOD8 boundary once, slowly, after a long walk. Refinement events are rare and spread across enormous distances — not a per-frame churn at the camera. A coherence error at a boundary crossed once per kilometre is a "the seam should match when eventually crossed" property, not a real-time emergency.

### Three decoupled clocks

These never have to agree, which is why none can stall the others:

1. **Render LOD** — the drawn mesh coarsens by ring on its own terms.
2. **Cache warmth** — a hot/cold lazy structure holds cluster data warm after a player leaves, in case they double back. Threaded cluster regeneration is available to feed it.
3. **Erosion sweep** — the only thing that performs the actual write-back and free (§5).

"Player left," "data freed," and "mesh coarsened" are fully decoupled events. Reclamation is **lazy**, never instant-on-recede.

---

## 8. Cluster Fates & Persistence

Every cluster is exactly one of three fates, separated by **who owns the truth**:

1. **Regenerable-from-ledger** — the entire resting world. Free to discard; rebuildable from compact world data by the erosion ledger. This is *physics*. You do **not** need durable storage for the planet.
2. **Breathing-LOD0 cache** — the active player bubble. Materialized on approach, reclaimed lazily. This is *rendering*.
3. **Durable-LOD0 authority** — claims (§9) and, by analogy, the entire narrative layer (§11). The only data whose voxel/state contents are *truth, not derivation*. Must serialize, persist, survive restarts, and be backed up. **This is the only category that ever touches a backup**, because losing it means losing something a model cannot recompute — something with a player's name on it.

The persistence problem therefore splits cleanly: a vast regenerable world you can afford to lose, and a small precious record you must keep.

---

## 9. Claims

A claim is a player-owned region of limited, fixed size. **Decided rule: claims are always recorded as LOD0 clusters with perfect voxel bake fidelity, and are flatly exempt from the degeneration cycle.** No reclamation, no reconciliation, no shadow simulation, no feathering of shape — the geometry is simply kept. (Earlier lease/shadow/viewshed elaborations are superseded by this flat rule.)

Consequences:

- **Fixed size makes the budget structural, not behavioral.** Persistent full-fidelity storage = (claim count × claim size) — a number you can compute ahead of time, unlike the breathing bubble whose cost depends on where players wandered.
- **A claim is a fixed ring-structure, not a bare LOD0 patch.** A permanent LOD0 island dropped into LOD8 resting terrain is an 8-level jump that the ≤1-LOD assert forbids. So a claim carries the same concentric refinement rings outward that a player bubble does — LOD0 core stepping up to meet resting terrain — but **permanent rather than camera-following**. It is the player-bubble machinery with the camera nailed down.
- **Claims are the one place shape is authority.** Everywhere else a cluster answers "yes" to "regenerable from world data?"; a claim answers "no." Its bake is the truth.

Retention rationale (general principle, not a claim mechanism): modifications worth keeping are those within a player's horizon of their claim — the set where a continuity disruption would actually be witnessed. The claim itself is simply pinned LOD0; this rationale explains *why* claim-local fidelity matters.

---

## 10. Dungeons (the easy half)

Dungeons are the dungeon-maker half of the game, and they are easy for one structural reason: **a dungeon escapes the hardest constraint the world model is built around — it has no neighbors to reconcile.**

- Underground, sealed, entered through a portal. No seam, no LOD ring to blend outward, no erosion sweep laundering its contents, no surrounding world that evolves while the player is away. It is a **closed box** — the one geometry the architecture never has to reconcile. Bounded by **authored walls**, not computed horizon.
- **A dungeon does not derive from world data at all.** Surface terrain is a *function* (ledger in, contour out, regenerable, conserved). A dungeon is *placed*: a modular building system assembled from structured templates — pure authored content that happens to use the same voxel renderer. It is the **sampler** to the surface's **synthesizer**: same output format, opposite philosophy, and the easier engineering (assembling known-good pieces vs generating coherent novelty under conservation laws).

### Template streaming

Typical dungeon: ~20–30 template pieces (≤~100 max), ~2MB each → ~40–60MB over a full crawl. This is a non-event:

- **Templates are instanced** — a dungeon using 30 distinct pieces may place them hundreds of times. You pay per piece once and stamp it freely. You stream the dungeon's **vocabulary, not its size**; a sprawling dungeon and a small one drawn from the same pieces cost the same. (This is the same deduplication-by-shared-vocabulary idea as the kernel's reserved package instances and the material ledger's curated element set — the template library is the dungeon's periodic table.)
- **The tier system gates template access as the dungeon grows** — both a gameplay mechanic (so players don't stack all their templates at the entrance) and, structurally, a depth-bounded set of upcoming assets. Total bytes are not the constraint; this is a content/pacing system, not a bandwidth problem, and does not need further engineering now.

---

## 11. The Narrative / RPG Layer

Everything else — above-world towns and the player tapestry system, NPCs, town management, family-business continuity, magic, secrets, discoveries, achievements, unlocks, character evolution — is the actual game, in a different discipline from terrain.

Storage-wise it lands in the **durable-authority tier (§8)**, with claims: small but precious, irreplaceable, with a player's name on it, *cannot be regenerated from a model*. It is the inverse of terrain — terrain is enormous and must be cheap, so it's regenerable and conserves only quantities; narrative state is tiny and each byte is irreplaceable.

So the persistence split that organizes clusters organizes the whole game. **Two tiers, top to bottom: the vast regenerable world, and the small durable record of what specific people did in it.**

---

## 12. The Renderer as a Downstream Representation

The renderer never owns truth. It is the last stage of a chain ordered by how-true-each-thing-is:

1. **Conserved material & water volume in the ledger** — the truth. Slow, owns conservation (§4–6).
2. **Heightmaps (terrain + water) derived at bake** — shape. Regenerable; changes only on the erosion cadence (§5, §7).
3. **Surface mesh with a material** — render. Terrain mesh via the existing cluster/contour pipeline; **water as a second `IVoxelLayer` sampling the water scalar field, meshed exactly like terrain and drawn with a water material.** LOD-ringed identically. If you can draw the ground, the still water surface is nearly free.
4. **Optional flow animation** — garnish. Visible motion (a stream running, a waterfall) is **not** a surface scalar and cannot come from the heightmap model. It is faked: flowmap-driven scrolling normals on the static surface, foam decals at rivulet termini, particle emitters at falls. It **carries no state, conserves nothing, runs on the frame clock, scales with budget, and degrades to a perfectly still surface at zero cost.** A still, volumetrically-correct lake is a complete, shippable feature; the moving look is deferrable polish.

The critical line: **the simulation's model of water (rivulet bundles, saturation, conserved volume) and the renderer's model of water (a surface scalar per column) are different representations of the same truth, and the conversion is a reduction** — a bundle of strands collapses to a single depth value, the way the material ledger collapses to a terrain height. No live fluid solve ever runs in the hot path; the water surface is as static, up close, as the mountain, and re-bakes only on the erosion cadence.

---

## 13. Module / Crate Decomposition

Boundaries fall out along the seams above:

1. **world-topology** — the hex array, spiral indexing, the two neighbor formulas, pole/equator special cases, R-parameterization, 2 + 6R² sizing. Pure geometry/indexing; no physics, no materials. *Build first; everything addresses through it.*
2. **materials** (shared vocabulary) — the limited/curated periodic table plus hardness, atomic attractiveness, brittleness, heat behavior, compound rules. The load-bearing simplification (real chemistry is intractable; a curated set is computable *and* art-directable). Imported by both world-gen and the voxel renderer; the macro analog of the kernel's base extended types.
3. **world-gen** — the nine-epoch pipeline, planetary physics, erosion cycles, vein generation. Offline-heavy; depends on (1) and (2); the runtime links none of it.
4. **world-state** — the retained epoch-7-9 ledgers, the live slow-tick erosion sweep, the rivulet/lava/ice flow structures, GM controls (hot zones, vein placement overrides), the degeneration/write-back routine, and the invalidation channel that tells the bake system which columns went stale. Authority lives here.
5. **seam-to-voxel** (thin) — translates a hex pixel + column ledger into the LOD8 cluster vector the existing voxel engine consumes. Deliberately small; it is the plug for the socket the cluster map left open.

---

## 14. Decided vs Deferred

**Decided (treat as load-bearing):**

- Data-is-truth / shape-is-disposable / mass-is-conserved as the governing model.
- Polar hex array, ring N = 6N, total 2 + 6R², two neighbor formulas, fold-mirror equator.
- Nine-epoch offline pipeline; epoch-7-9 state is retained truth; seed is a birth certificate, not a regeneration recipe.
- Pixel = material ledger; shape is a bake-time function of it; conservation is on quantities.
- Erosion sweep is the only system that evolves truth; degeneration = material-aggregate write-back, shape discarded.
- Sparse LOD pyramid; resting world is LOD8; generate-on-approach up to LOD0; backdrop layer absorbed into the default state.
- Three cluster fates; only the durable-authority tier is backed up.
- Claims are fixed-size, always LOD0, durable, with permanent refinement rings.
- Dungeons are closed-box, template-placed (sampler), do not derive from world data; template streaming is a non-issue.
- Renderer (terrain and water) is a downstream reduction; water surface is a second heightmap layer.

**Deferred / not yet determined (do not invent these):**

- True 3D modification beyond 2D heightmap contouring (caves/overhangs as first-class world data rather than claim-local authored shape).
- The water renderer's concrete implementation (mesh/material specifics).
- Flow animation behavior and its cost tier.
- Whether ice needs a structure distinct from rivulets.
- Lease-end / shadow-sim reconciliation — **superseded**; claims are flat LOD0. Do not reintroduce.
- Eviction/backtracking optimizations for dungeon template streaming — not needed at current scope.

---

*This document captures the model as designed. It does not prescribe data structures, function signatures, or algorithms. Where a decision here constrains implementation, honor it; where this document is silent, that silence is intentional and the decision is open.*
