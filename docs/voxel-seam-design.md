# Voxel seam handling — integrated cell-baking design

## Scope (revised)

**No secondary seam baking routine. No separate seam mesh data
structures. No new geometry objects.** Seam alignment is a property
of how cells bake their own boundary vertices during the normal
contour pass. Two adjacent clusters produce boundary vertices that
land at the same world positions because both compute them from the
same voxel data through the same canonical arithmetic.

The current `emit_seam` pass in `contour.rs` violates this — it
produces a *separate* seam strip with its own vertices and triangles
appended to the mesh buffer. It should be deleted in the next pass
and its responsibility folded into the main interior-cell loop.

The value we take from VoxelFarm is **the vertex-alignment
invariant**, not their architecture. They run a secondary seam-baking
pipeline (`computeCellSeams`, `createSeam`, `CSeamCellData`) because
their cells live in an octree and their meshes ship through a stream
of separate per-cell buffers; the seam mesh is a separate buffer
because everything is. Our `CellMesh` is a single buffer per cluster,
so we don't need (or want) the secondary plumbing — but we still need
the alignment rule.

## The alignment rule, end-to-end

For two adjacent clusters A and B, sharing a face on the seam plane:

1. A and B each have an interior cell grid. With no neighbor handling,
   A's cells end at `cx = cell_dim - 1`, leaving a one-cell gap from
   A's boundary face. Same for B's `-X` side. This is the gap that
   produces the visible crack.

2. With neighbor handling, A's cell grid **extends by one cell row on
   each face that has a neighbor**: `cx` ranges `0 ..= cell_dim`. The
   extra row of cells at `cx = cell_dim` straddles the seam — its X+0
   corners are A's last voxel column, its X+1 corners are B's first
   voxel column (read across the seam through the `NeighborContext`).

3. The extended cell's vertex is computed by the same centroid logic
   as any other cell. It lands on the seam plane (world X ≈
   `CLUSTER_DIM`). Critically, **B's contour pass also extends its
   `-X` boundary**, producing the *same* extended cells (mirrored as
   B's `cx = -1`, which we'd actually iterate as `cx = 0` of a
   prepended row). Because A and B compute centroids from the same
   8 corner positions in a shared world frame in canonical iteration
   order, **the two computations produce byte-identical world
   positions**.

4. Face emission similarly extends. Voxel-grid edges that cross the
   seam (X-axis edge from `(CLUSTER_DIM - 1, vy, vz)` to `(0, vy, vz)`
   in B's frame) now have 4 surrounding cells fully defined — two from
   the original interior, two from the extended boundary row — so the
   quad emits cleanly. The triangles on each side share world-position
   vertices, even though their local index buffers don't.

5. Combined-mesh closure: when A's mesh and B's mesh are concatenated
   and B's vertices are translated by `+CLUSTER_DIM` on the seam axis,
   each seam world-position has two vertex instances (one from A, one
   from B) with byte-identical positions. The boundary triangles from
   A's cells reference A's instance; from B's cells reference B's
   instance. Each edge is shared by exactly 2 triangles per side,
   so 4 in the combined mesh — but those are 2 distinct logical edges
   (one in A's index space, one in B's), each in 2 triangles. The
   `edge_use_counts` test, which keys edges by vertex index pairs,
   sees them as separate edges and the histogram is `{2: ∀edges}`.

That last point is the key: **closure under combined-mesh is achieved
by both clusters emitting their own copy of the boundary triangles**,
at byte-equal world positions. There's no "single source of truth" for
each seam triangle — both sides own their copies — but because the
copies are byte-equal in space, they overlap perfectly and no crack
is visible.

This sidesteps the contradiction I documented earlier (closure vs.
byte-equality). Under "fine side emits a separate seam strip,"
either-or. Under "both sides extend their own cell grid by one row
across each shared face," both invariants hold simultaneously.

## What changes in `contour_cluster_lod_with_neighbors`

The Phase 3 contour pass has a cell loop that iterates `cx ∈ 0..cell_dim`.
The interior is unchanged. What needs to change:

1. **Extended iteration range per face.** Pseudocode:
   ```rust
   let extend_neg_x = neighbors.neg_x.is_some();
   let extend_pos_x = neighbors.pos_x.is_some();
   let cx_lo = if extend_neg_x { -1 } else { 0 };
   let cx_hi = if extend_pos_x { cell_dim as i32 + 1 } else { cell_dim as i32 };
   for cx in cx_lo..cx_hi { ... }
   ```
   And similarly for Y, Z. A cell at `cx = -1` straddles the `-X`
   seam; `cx = cell_dim` straddles the `+X` seam; corner cells like
   `(-1, -1, 0)` straddle two seams; `(-1, -1, -1)` three. All handled
   uniformly by the same cell-baking code.

2. **Corner read goes through `NeighborContext`.** A helper:
   ```rust
   fn read_corner(
       cluster: &Cluster,
       neighbors: &NeighborContext,
       vx: i32, vy: i32, vz: i32,
   ) -> Voxel {
       // (vx, vy, vz) may be out of [0, CLUSTER_DIM); route to the
       // appropriate neighbor cluster.
   }
   ```
   When `vx < 0`, read from `neighbors.neg_x`'s voxel at
   `(CLUSTER_DIM + vx, vy, vz)`. When `vx >= CLUSTER_DIM`, read from
   `neighbors.pos_x`'s voxel at `(vx - CLUSTER_DIM, vy, vz)`. And so
   on for Y, Z. Edges and corners of the cluster (two or three of the
   coordinates out of range) cascade to neighbor-of-neighbor, which
   we don't have — those configurations gracefully fall back to base
   (or skip emission).

3. **Cell vertex computation unchanged.** The existing centroid +
   gradient-normal + closest-corner-material logic in the Phase 3
   inner loop already works correctly when corners come from
   neighbors; it just needs `read_corner` instead of `cluster.get` on
   the boundary positions. Critical: the canonical iteration order
   over `(k, j, i)` must be preserved so two adjacent clusters compute
   byte-equal world positions on their shared boundary cells.

4. **Face emission similarly extends.** The three axis-edge loops in
   Pass 2 grow their ranges by 1 on each side where a neighbor exists.
   Edges crossing into neighbor space sample their classifications
   through the same `read_corner` helper.

5. **Bounds update unchanged.** Boundary cell positions fall just
   outside `[0, CLUSTER_DIM]` (e.g., `(CLUSTER_DIM, ?, ?)` for `+X`
   boundary cells) and are naturally included in `bounds_min`/
   `bounds_max`.

6. **Delete `emit_seam` entirely.** Its job is now done by the
   extended interior loop. Delete the `Face` struct, the dispatch
   loop in `contour_cluster_lod_with_neighbors`, and the entire
   `emit_seam` function. Net code reduction.

## LOD differences

The above closes equal-LOD seams cleanly. For different-LOD seams the
interaction is more delicate, but the same principle applies:

- When `neighbors.pos_x = Some((&b, b_lod))` and `b_lod.stride() >
  self_lod.stride()`, self's boundary row at `cx = cell_dim` reads
  the neighbor at the **coarser stride**. This means self's boundary
  cells are coarsened along the in-face axes (Y, Z) to match B's
  resolution. Self's interior is unchanged; only the boundary row
  coarsens.

- B's boundary row at its `-X` side (its `cx = -1`) reads A at A's
  finer stride. But because B is already iterating at coarse stride,
  the natural sampling is one A voxel per B sample point — which is
  precisely the coarse-aligned subset of A's voxels. Both sides end
  up reading the same 8 voxels per boundary cell.

- T-junctions remain at the interior of the fine side, where fine
  cells terminate on a coarse boundary cell. The fine side's
  boundary row is coarsened, so it has fewer vertices than its
  interior. The fine interior's vertices that would have connected
  to the now-missing fine-stride boundary cells are left with edges
  that "fan into" the coarse cell's vertex. Visible cracks are
  possible but mild.

This is acceptable for Phase 4. A later phase can add fan-triangle
emission for the fine side's transition row if cracks become a
visible problem.

## Test consequences

- **Six face count tests**: change shape. There's no `emit_seam`-
  emitted strip to count. Instead, count triangles at the seam
  plane: cells in the extended boundary row produce surface there.
  The exact count depends on the fixture; for a `solid_uniform`/
  `Cluster::empty()` pair on `+X`, the seam plane has a full surface
  spanning `(CLUSTER_DIM - 1)²` cells = 255² × 2 triangles. Expected
  counts in the tests adjust accordingly.

- **`coarse_to_fine_seam_no_cracks_in_combined_mesh`** activates and
  passes. Both clusters emit boundary triangles; world positions
  byte-align; combined edge histogram is all 2s (each cluster's
  copy is its own edge in the index space).

- **`seam_vertices_byte_equal_under_symmetry`** activates and passes
  for equal-LOD pairs. For mixed-LOD pairs, the test compares the
  coarse cluster's boundary vertex against the fine cluster's
  coarsened-boundary-row vertex; both compute via the same canonical
  helper, byte-equal.

- **`three_way_lod_chain`** activates. Each cluster reads its
  immediate neighbors through `NeighborContext`; the chain closes
  pairwise.

- **Phase 1–3 tests**: byte-identical. The extended cell loop only
  activates when a neighbor is present; with all `None`, the
  iteration ranges are unchanged from Phase 3.

## Implementation order

1. Delete `emit_seam`, `Face`, and the dispatch loop. Crate
   temporarily loses seam handling; closure tests stay `#[ignore]`.

2. Add `read_corner(cluster, neighbors, vx, vy, vz)` helper that
   routes out-of-range coordinates to neighbors and falls back to
   self's base for out-of-cascade.

3. Thread `neighbors: &NeighborContext` into the inner cell loop;
   replace `cluster.get(LocalCoord::new(vx, vy, vz))` with
   `read_corner(cluster, neighbors, vx, vy, vz)`.

4. Extend the cell-grid iteration range per face when a neighbor
   exists.

5. Extend the three axis-edge face-emission loops similarly.

6. Update the six per-face quad-count tests with new expected
   values.

7. Activate `coarse_to_fine_seam_no_cracks_in_combined_mesh`,
   `seam_vertices_byte_equal_under_symmetry`, and
   `three_way_lod_chain`.

## What's deferred

- T-junction fan triangles on the fine side of a mixed-LOD seam.
- Edge and corner neighbors (when two or three of the cluster's
  coordinates are out of range simultaneously). Phase 4 stays on
  face neighbors only.
- The world-gen rule that constrains adjacent clusters to ±1 LOD
  level. Belongs to the layer above `flicker-voxel`.

## Token-honest summary

Implementing the above end-to-end is ~300 lines of code change plus
test adjustments. Within reach but past this session's budget.
The doc above is the precise handoff: the convention, the helper
shape, the loop bounds, the test edits.
