# Spec — Multi-cluster world + LOD-aware seam meshing (one algorithm)

Status: ready to implement. **Part A** is a small warm-up (split scene + two
debug toggles, single cluster) with a checkpoint. **Part B** is the real work:
one mesh algorithm that writes seam panels across cluster joins by reading
neighbor context, where a coarse↔fine join is just the T-junction special case
of the same edge match — no second pass.

This builds directly on the existing substrate (all already present, verified):
`ClusterId` (packed `[LOD:4][x:10][y:8][z:10]`, `world_offset()`), `ClusterMap`
(`HashMap<ClusterId,Cluster>`), `Lod` (`stride`/`sample_dim`/`cell_dim`, levels
0–7), `NeighborContext` (6 `Option<(&Cluster, Lod)>` faces), and `read_corner`
(routes a single-axis-out-of-range voxel read to the matching face neighbor).

---

## Part A — split scene + debug toggles (single cluster, checkpoint)

### A1. Wave field lower, primitives upper
The primitive `Scene` is currently a union of `Sdf` parts (min of distances).
Add the heightmap as one more part and raise the shapes.

- `impl Sdf for HeightField`:
  `fn distance(&self, p) -> f32 { p[1] - self.height_bilinear(p[0], p[2]) }`
  where `height_bilinear` bilinearly samples the existing cached 256² column
  grid (continuous and cache-fast — do NOT call `world_height_seeded` per sample;
  that rebuilds the wave field and makes the union's 16 M `is_solid` calls
  unusably slow). Out-of-cache `(x,z)` clamps to the edge for now (single
  cluster).
- `Scene::world()`: union of `HeightField::from_default_seed([0,0,0])` (fills the
  lower band, y≈64–192) plus the six gallery shapes with centers raised into the
  upper cluster (e.g. y≈200, sized so they clear the terrain). Keep `gallery()`
  too; `world()` is the new default the example contours.
- Union `is_solid`/`edge_hermite` are unchanged (min-distance sign + `sdf_hermite`
  finite-difference normal). Terrain normals will be bilinear-faceted; that's
  fine — shading is deferred to texturing.

### A2. Two debug toggles (both OFF by default)
In `examples/voxel-cluster/src/main.rs`, add two bound keys (e.g. `1` = wireframe,
`2` = centroid) that flip `bool` fields, both starting `false`.

- **Wireframe** (`on` → also draw the mesh with `MeshDrawOptions { wireframe:
  true, .. }` as an overlay pass, exactly like `mesh-smoke`'s second draw).
- **Centroid** — *I don't have the deleted prior implementation; this is my
  interpretation, correct it if it meant something else.* Draw a small marker at
  each cell's dual vertex (the QEF-placed point the contour stored). Simplest:
  have `mesh()` optionally also return the unique cell-vertex positions, and draw
  a tiny axis cross at each via the existing line/`draw_bounding_box` primitive
  (or a dedicated `draw_points` if you add one). Keep it behind the toggle; perf
  isn't a concern when off.
- Show toggle state in the HUD line.

**CHECKPOINT A:** `cargo run --release -p voxel-cluster` shows terrain in the
lower cluster with the six shapes floating above; `1`/`2` toggle the overlays;
both default off. `cargo test -p flicker-voxel` green.

---

## Part B — multi-cluster + LOD-aware seam meshing

### B0. The model (read before coding)
- **Each cluster contours one extra cell layer on its +X/+Y/+Z faces.** Today
  the contour places cell vertices for cells `0..cell_dim`. Extend it to also
  place the `+`-face seam cells (min-corner voxel at index `sample_dim-1` along
  the `+` axis), whose `+`-corner sample sits one step past the cluster (voxel
  coord `256` at LOD 0). The contour evaluates its own primitive there (the
  primitive is global), so no neighbor data and no ordering dependency at contour
  time. That seam cell's vertex is stored on its in-cluster min-corner voxel.
- **The mesh stops skipping the `+`-boundary edge.** The edge loop already steps
  only in `+` axis directions from `g ≥ 0`, so every join edge is visited exactly
  once — by the cluster on its low side. For that edge, the far voxel's solidity
  is read through `read_corner(cluster, neighbors, …)` (the neighbor's
  authoritative stored data) instead of being treated as empty. The four cells
  around the edge are the owner's own seam cells (now stored), so their vertices
  resolve locally. **Ownership is automatic; there is no second pass and no
  explicit owner bookkeeping.**
- **World-boundary faces** (neighbor `None`) stay open, as today.
- **LOD is per-cluster** (carried by `ClusterId.lod()` / the `Lod` in
  `NeighborContext`). The same edge match runs at any LOD; coarse↔fine is the
  T-junction special case (B3).

### B1. LOD-aware contour (single cluster)
Add an LOD parameter: `pub fn contour(primitive: &dyn Primitive, material: Material, lod: Lod) -> Cluster`
(LOD 0 reproduces today's behavior).

- Stride `s = lod.stride()`, sample grid points at voxel coords `0, s, 2s, …,
  (sample_dim-1)·s`. Cells indexed `0..sample_dim-1`, each spanning `s` voxels;
  classify corners and gather edge Hermite via the primitive at the **strided**
  voxel coords (e.g. cell `(ci,cj,ck)` corner `(i,j,k)` → voxel
  `((ci+i)·s, (cj+j)·s, (ck+k)·s)`).
- **Stride-scaled corner storage.** A LOD-`L` cell is `s` voxels wide, which
  exceeds the corner-vector's `[-0.5,1.5]` *voxel* range. Store the offset in
  **cell units**: `corner = (vertex_world - min_corner_voxel) / s`, range
  `[-0.5,1.5]` covers the cell. Store on the min-corner sample voxel
  `(ci·s, cj·s, ck·s)`. (LOD 0: `s=1`, identical to today.)
- Include the `+`-face seam layer per B0: iterate the cell whose `+`-corner is at
  sample index `sample_dim` (voxel `256`), evaluating the primitive there.
- Solidity is still stored densely on solid sample voxels (octree TODO unchanged).

**CHECKPOINT B1:** contour a single cluster at LOD 2–3 and mesh it (mesh must
decode the stride-scaled corner: `vertex = min_corner_voxel + corner·s`). The
shape renders coarser but recognizable; `cargo test` green (add a LOD round-trip
test: a LOD-`L` flat field's cell vertex decodes back onto the plane).

### B2. Multi-cluster, uniform-LOD seams
- `mesh` gains neighbor context and the cluster's own placement:
  `pub fn mesh(cluster: &Cluster, neighbors: &NeighborContext, lod: Lod) -> ClusterMesh`,
  emitting vertices in **world space** (`+ cluster_world_offset`). Pass the
  offset in, or have the caller translate; pick one and be consistent.
- Replace the "skip if `g+1 ≥ dim`" / "skip if any of 4 cells out of range" guards
  on `+` faces with neighbor-resolving reads: far-voxel solidity via
  `read_corner`; the 4 seam-cell vertices are the owner's own (B1 seam layer).
  Decode every cell vertex with the stride scale and add the cluster's world
  offset so seam quads join correctly to the neighbor's world-space surface.
- Example: build a small world in the `ClusterMap` (the module doc anticipates a
  3×3 field), all LOD 0, terrain from the shared global heightmap so adjacent
  clusters agree at joins by construction. Assemble each cluster's
  `NeighborContext` from the map (by `ClusterId` arithmetic) and draw each
  cluster's mesh at its `world_offset()`.

**CHECKPOINT B2:** the 3×3 terrain field renders **watertight** — the one-voxel
border gaps from the single-cluster era are gone at interior joins; outer world
edges remain open. This is the bulk of the payoff and must be solid before B3.

### B3. Cross-LOD T-junction fan
- Fix `read_corner` to honor the neighbor's `Lod`: when routing a read into a
  neighbor, snap the wrapped coordinate to the neighbor's stride sample (it
  currently uses the raw coord and ignores the `Lod` in the tuple).
- When an owner's `+` neighbor is **finer**, the owner's single coarse boundary
  vertex spans several of the neighbor's finer boundary cells along the shared
  edge. Emit a **fan**: read the neighbor's finer boundary vertices along that
  edge (via `read_corner` at the neighbor's stride, in world space) and triangulate
  from the coarse owner vertex to the ordered fine vertices, keeping winding
  consistent with the normal-orientation rule already in `push_quad`. When the
  neighbor is **coarser**, the owner is finer and (by the low-side ownership rule)
  the *neighbor* owns the join — the finer side contributes nothing there, so no
  crack. This asymmetry is what makes "same algorithm, special case" hold.
- Example: drop one cluster in the field to LOD 1 or 2 next to LOD-0 neighbors.

**CHECKPOINT B3:** the coarse↔fine adjacency is watertight (no cracks along the
T-junction); flipping which cluster is coarse still seals.

### Deferred (NOT this slice): adaptive LOD selection
Choosing *which* LOD each cluster/region gets — the octree collapse driven by the
QEF residual (flat→coarse, curved→fine) — is the *policy* that feeds this
*mechanism*. It's the natural next slice once B1–B3 are solid. This spec wires
LOD as a manual per-cluster parameter so the mechanism can be exercised and
verified first.

---

## Tests (`flicker-voxel`, cheap)
- LOD: `Lod` stride/dim relationships already covered; add a contour LOD
  round-trip (flat field at LOD `L` → cell vertex decodes onto the plane).
- Seam: a synthetic 2-cluster pair sharing a flat field across the join — assert
  the seam emits quads bridging the boundary (no gap) and that flipping which
  side is the neighbor doesn't double-emit.
- `read_corner` LOD snap: a unit test that a read into a LOD-`L` neighbor returns
  the stride-snapped sample.
- Full multi-cluster meshing is verified by running the example, not in the unit
  suite.

## Performance
Per-cluster contour is unchanged in character (dense solid fill dominates; octree
TODO). LOD `L` clusters are far cheaper (`(256/2^L)³` cells). Build the field once
at startup; run `--release`.

## Checklist
- [ ] A1 split scene (`HeightField: Sdf` bilinear, `Scene::world()`); A2 two
      toggles (off by default); **CHECKPOINT A**.
- [ ] B1 LOD-aware contour (stride sampling, stride-scaled corners, `+`-seam
      layer); mesh decodes stride scale; **CHECKPOINT B1**.
- [ ] B2 `mesh(cluster, neighbors, lod)` world-space + neighbor-resolved `+`
      boundaries; multi-cluster example; **CHECKPOINT B2** (watertight uniform-LOD
      field).
- [ ] B3 `read_corner` LOD snap + cross-LOD fan; **CHECKPOINT B3** (watertight
      coarse↔fine).
- [ ] Confirm the two flagged decisions (centroid-toggle meaning; adaptive-LOD
      deferral) before running.
