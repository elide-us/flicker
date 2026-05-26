# ClayEngine World Generation Pipeline — Layers 1-6 Spec (Revision 2)

## Overview

The world generator is a one-shot batch system that produces the geological substrate of a planet from initial conditions. It runs six epochs of simulation at the hex level (planet-scale macro state), with cluster-column-level detail materialized either eagerly during generation or lazily during play.

The generator runs offline. It is not interactive. Hex-level generation across the full planet is computationally cheap (thousands of hexes). Cluster-column-level materialization is expensive (millions of pixels per hex) and can be deferred.

The runtime engine never executes any layer 1-6 logic; it only reads the materialized output.

## World topology

A world is a one-dimensional array of hexes arranged in polar-symmetric rings.

- **Index 0** is the north pole — a single hex.
- **Indices 1-6** are the first ring around the north pole — 6 hexes.
- **Ring N** contains 6N hexes, arranged in increasing latitude bands.
- After **R rings** from the north pole, the array reaches the equator (the widest ring, with 6R hexes).
- The pattern then mirrors: subsequent rings shrink by 6 hexes each, back down to a single hex at the **south pole**, which is the last element of the array.

**Total hex count for a planet with R rings between pole and equator:** 2 + 6R² hexes.

| R (rings to equator) | Total hexes | Hex size at equator (mi diameter) | Use case |
|---|---|---|---|
| 1 | 8 | enormous | sanity test only |
| 5 | 152 | very large | small test world |
| 10 | 602 | ~50 mi | small playable planet |
| 20 | 2402 | ~25 mi | medium playable planet |
| 30 | 5402 | ~17 mi | full-scale playable planet |
| 50 | 15002 | ~10 mi | large-scale planet |

(Hex size assumes each hex is 2048 × 128 feet wide; smaller R means each hex covers more of the planet's surface area.)

Adjacency between hexes is computed from the array structure plus the ring topology. Each hex has 6 neighbors (with the pole hexes being special cases — the north pole borders all 6 hexes of ring 1, and similarly for the south pole).

The array structure is deterministic for a given R. Two worlds with the same R have the same topology, regardless of their content.

## Hex data structure

Each hex carries two scales of data:

### Hex-level state (the macro state)

A small per-hex state vector that captures the macro-scale geology of the hex. This is what the world generator's epoch pipeline produces and what the runtime reads to determine what kind of place each hex is.

| Field | Set by epoch | Type | Purpose |
|---|---|---|---|
| `element_composition` | 1 | Element ratios (sparse map) | Bulk composition of the hex's material column |
| `density_profile` | 2 | Depth-keyed density values | How material is layered vertically after differentiation |
| `plate_id` | 3 | Integer | Which tectonic plate this hex belongs to |
| `plate_age` | 3 | Float (millions of years) | When this crust was formed |
| `boundary_type` | 3 | Enum (interior/divergent/convergent/transform) | Hex's role in plate dynamics |
| `mean_surface_elevation` | 3 | Float (meters relative to reference) | Macroscale topography |
| `volcanic_activity` | 3 | Float (0-1) | Volcanism likelihood |
| `water_volume` | 4 | Float (cubic meters) | How much water is present at this hex |
| `atmosphere_composition` | 4 | Element ratios | Local atmospheric makeup |
| `temperature` | 4 | Float (°C average) | Surface temperature |
| `hydrothermal_signature` | 5 | Float (0-1) | Mineralization intensity |
| `vein_membership` | 5 | List of vein IDs | Which named ore veins pass through this hex |
| `surface_material_signature` | 6 | Element ratios | Surface composition after erosion |
| `dominant_biome` | 6 | Enum | Primary biome classification |
| `seed` | 1 | u64 | Deterministic seed for cluster-column materialization |

The hex-level state for the entire planet is generated up front, costs a few thousand vector entries × R-squared hexes, and is the authoritative output of layer 7. This data is small and gets fully materialized at generation time.

### Cluster-column-level state (the micro state)

The actual heightmap-stack textures that describe the cluster columns within each hex.

- A hex's content is described by a 2048 × 2048 pixel texture (or a stack of such textures, one per material layer).
- The hex is a hexagonal region within this rectangular texture — pixels outside the hexagonal bounds are unused.
- Each pixel within the hexagonal region represents the top of one cluster column.
- Each pixel covers 128 feet × 128 feet of ground area (one cluster's horizontal footprint).
- Per-pixel data includes top elevation, surface material identity, and any per-cluster metadata needed for runtime materialization.
- Multiple textures may be stacked to represent different material strata (bedrock surface, sediment surface, soil surface, water surface, ice surface) — the topmost layer present at each pixel is the visible ground.

Cluster-column-level state is *not* required to be materialized eagerly at generation time. It can be:

- **Eagerly materialized** during generation, producing the full per-hex texture stacks for every hex. Slower initial generation; no in-game latency.
- **Lazily materialized** on first hex visit during play, with caching for subsequent visits. Fast initial generation; one-time generation cost per hex on first visit.
- **Hybrid**: critical hexes (player spawn area, named locations) materialized eagerly, other hexes lazily.

The cluster-column materialization function takes `(hex_index, hex_level_state, pixel_coordinates) → cluster_column_data` and is deterministic. The same inputs always produce the same outputs, which means materialized data is regenerable if lost.

## Cross-hex structures

Some generation outputs span multiple hexes and are stored separately from the per-hex vectors:

- **Plates** — list of `{plate_id, hex_membership_set, motion_vector, type}`. Output of epoch 3.
- **Veins** — list of `{vein_id, hex_path, depth_profile, element, concentration_profile}`. Output of epoch 5. A vein spans multiple hexes with a defined geometric path and concentration variation.
- **Watersheds** — list of `{watershed_id, drainage_hexes, outlet_hex}`. Output of epoch 6. Drainage basins.
- **Climate bands** — implicit from latitude (array position) but may be stored as a derived structure for fast lookup.

## Hex adjacency and texture alignment

When the runtime renders the player's current view (at most three hex diameters of visible terrain), adjacent hexes' textures must align at shared edges for continuous terrain.

The convention: each hex has a defined orientation in world space derived from its array position and the planet's polar axis. Adjacent hexes share edge pixels at specific coordinates in their respective textures, and those edge pixels must agree on cluster-column elevation, material, and other properties.

The cluster-column materialization function must respect this constraint: hexes A and B sharing an edge produce identical pixel values at corresponding edge coordinates.

This is achievable because both hexes' edge-pixel materialization functions take the shared edge's hex-level inputs (plate properties of both hexes, water flow patterns across the edge, etc.) and apply the same deterministic computation, producing the same outputs at the boundary.

## Rendering scope

The runtime renders at most three hex diameters of terrain visible from the player position. Standing in the center of one hex shows that hex plus its six neighbors (seven hexes total), with the surrounding hexes only partially visible due to camera frustum.

This means:

- **The number of hexes actively materialized for rendering is small and constant** — never more than ~7 regardless of planet size.
- **There is no "planet-wide LOD" concept** because distant hexes are hidden by planetary curvature.
- **LOD within a hex is still needed** because a hex is ~50 miles across at full size, and within a single hex the player sees distance ranges from immediately adjacent to several thousand feet.
- **Streaming load is proportional to player motion across hex boundaries, not to distance traveled** — entering a new hex triggers materialization (if not already cached); moving within a hex requires no new hex-level materialization.

## Epoch specifications

The six epochs operate at the hex level. Each epoch reads the prior epoch's hex-level outputs and produces additional fields on the per-hex state vector and/or new entries in cross-hex structures.

Cluster-column-level detail is produced by a separate materialization pass after the hex-level generation completes (whether eagerly or lazily).

### Epoch 1: Initial element distribution

**Purpose:** Establish bulk composition per hex from initial planetary conditions.

**Operates at:** Hex level (one composition vector per hex).

**Inputs:** Initial element abundance ratios, planet seed, ring count.

**Processing:** Distribute total element mass across the hex array with smooth large-scale variation. Bias heavy elements toward equatorial hexes (proxy for being closer to the original accretion plane), volatile elements toward polar hexes. Apply correlated noise to introduce regional variation.

**Outputs:** Per-hex `element_composition`, `seed`.

### Epoch 2: Differentiation and crust formation

**Purpose:** Establish vertical density layering and crust thickness per hex.

**Operates at:** Hex level (one depth profile per hex).

**Inputs:** Epoch 1 outputs.

**Processing:** For each hex, sort composition by density to produce depth profile. Compute crust thickness from cooling rate (influenced by latitude — polar hexes cool faster). Mark hexes with thin crust as volcanic candidates.

**Outputs:** Per-hex `density_profile`, initial `volcanic_activity`.

### Epoch 3: Plate tectonic structuring

**Purpose:** Partition hexes into plates and establish macro-topography.

**Operates at:** Hex level (plate assignment per hex, plate metadata cross-hex).

**Inputs:** Epoch 2 outputs.

**Processing:** Generate plate boundaries using a hex-adjacency-aware partitioning (Voronoi-like over the hex grid using sparse seed hexes). Assign plate motion vectors. Classify each hex's boundary type based on neighbor plate memberships and motion vectors. Adjust `mean_surface_elevation` based on boundary type (convergent → mountains, divergent → rifts). Refine `volcanic_activity` based on boundary activity.

**Outputs:** Per-hex `plate_id`, `plate_age`, `boundary_type`, `mean_surface_elevation`, refined `volcanic_activity`. Cross-hex `plates` structure.

### Epoch 4: Hydrosphere and atmosphere formation

**Purpose:** Water condenses, oceans form in low-elevation hexes, atmosphere establishes.

**Operates at:** Hex level (per-hex water and atmosphere properties).

**Inputs:** Epoch 1, 3 outputs (water budget from composition, elevation from tectonics).

**Processing:** Compute global water budget. Fill lowest-elevation hexes with water (bathtub fill respecting hex adjacency). Compute atmospheric composition from integrated volcanic outgassing across all volcanic hexes. Establish temperature distribution from latitude (array position determines latitude band) and elevation. Compute baseline precipitation from temperature and water proximity.

**Outputs:** Per-hex `water_volume`, `atmosphere_composition`, `temperature`.

### Epoch 5: Mineralization and ore vein formation

**Purpose:** Establish ore concentrations and multi-hex vein structures.

**Operates at:** Hex level (per-hex hydrothermal signature, cross-hex vein paths).

**Inputs:** Epoch 3, 4 outputs.

**Processing:** Identify hydrothermal candidate zones (active plate boundaries with water proximity). For each candidate, generate a vein: choose target element from local composition, trace path through adjacent hexes following fault structure, assign concentration profile. Major veins span dozens of hexes; minor veins span a handful. Populate `vein_membership` for affected hexes.

**Outputs:** Per-hex `hydrothermal_signature`, `vein_membership`. Cross-hex `veins` structure.

### Epoch 6: Erosion, sedimentation, and biome assignment

**Purpose:** Apply erosion to refine elevation, assign biomes, finalize surface state.

**Operates at:** Hex level (per-hex surface state, watershed identification cross-hex).

**Inputs:** All prior epoch outputs.

**Processing:** Iterate erosion-deposition across hex adjacency: water flows from high to low elevation hexes, carrying sediment, depositing in basins. Refine `mean_surface_elevation` from erosion. Compute `surface_material_signature` from composition plus erosion history. Identify watersheds by tracing drainage paths. Assign `dominant_biome` from temperature, precipitation, elevation, surface material.

**Outputs:** Per-hex `surface_material_signature`, refined `mean_surface_elevation`, `dominant_biome`. Cross-hex `watersheds` structure.

## Sophistication phasing

Each epoch has a Phase 1 simple implementation and a Phase 2 sophisticated implementation. The interface contracts (input/output fields) stay constant; the implementations evolve.

Phase 1 across all six epochs produces a working planet within weeks. Phase 2 improvements happen over months to years, replacing kernels one at a time.

## Cluster-column materialization

After hex-level generation completes, cluster-column-level detail can be materialized.

**The materialization function:** `(hex_index, hex_state, vein_data, watershed_data, pixel_coords) → cluster_top_data`

For each pixel within a hex's texture:

1. Compute the pixel's world position (hex center plus offset within hex).
2. Sample interpolated values from hex-level state and from neighbors (for smooth boundary transitions).
3. Apply elevation noise within the hex bounded by hex-level mean elevation.
4. Apply vein contributions (if any vein passes through this pixel's vertical column, add appropriate material at appropriate depth).
5. Apply biome-driven surface details (forest cover, grass, exposed rock).
6. Produce the final pixel value for each material layer in the stack.

This function is deterministic given its inputs. The same hex with the same seed always produces the same texture.

## Storage

The world data blob contains:

- World metadata (R, seed, generation parameters, version).
- Per-hex state vectors for all hexes (the full layer 7 hex-level data).
- Cross-hex structures (plates, veins, watersheds).
- Optionally: materialized cluster-column textures for some or all hexes (if eager materialization was used or hexes have been visited and cached).
- Historical layer data (per-hex states at completion of each prior epoch, for reference).

This blob is stored through TheOracle as a world artifact.

## What this spec does NOT cover

- Layer 7 to layer 8 derivation (heat control map, GM dials). Separate spec.
- Layer 9 (ongoing surface simulation). Separate spec.
- Voxel materialization from cluster-column data at runtime. Separate spec.
- The specific element chemistry model (what elements, what reactions, what makes mud "mud"). Separate spec.
- Biology beyond biome assignment. Either part of epoch 6 sophistication or a separate epoch 6.5 — to be decided.

## What needs to be decided before implementation

- The number and identity of tracked elements (cardinality of `element_composition`).
- The specific algorithms for each epoch's Phase 1 kernel.
- The cluster-column materialization function in detail.
- The texture format and material layer stack count.
- The blob serialization format for Oracle storage.
- Whether to materialize cluster-column data eagerly, lazily, or hybrid.
