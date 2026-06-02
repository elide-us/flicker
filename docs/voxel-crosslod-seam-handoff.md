# Cross-LOD seam handoff (Fork3, post-§3b iteration)

You're picking up work on flicker's voxel pipeline. Repo at
/Users/elideus/Repos/flicker, branch Fork3. Relevant files:
crates/flicker-voxel/src/{contour,mesh,neighbor}.rs and
examples/voxel-cluster/src/main.rs (3×3 cluster field; `\` toggles the
centre cluster between LOD 0 and LOD 1).

## What works (don't touch)

- §1: multi-cluster, uniform LOD, low-side ownership for seams. Tests
  guard this in row_of_three_clusters_along_x_seam_closed.
- §2: LOD-aware contour and mesh for a single cluster. Storage stride
  matches LOD; cell corners stored in cell-units [0, 1]; decode is
  `origin + corner * stride`. LOD 0 stays byte-identical to §1.
- §3a: ±1 LOD adjacency assertion in mesh.
- Vertex weld (ClusterMesh::weld()): byte-equal positions collapse to
  one index with averaged normals. Runs at the end of mesh().
- ClusterMesh::edge_use_histogram() diagnostic; the example logs
  per-cluster unshared/over-shared counts at rebuild.

The current cross-LOD code (§3b v2) — `emit_neg_seam_takeover`,
`plus_neighbor_skip`, and the cross-LOD snap in `cell_vertex` — is
WRONG and needs to be torn out. Don't try to extend it; it's based on
the wrong ownership model.

## The correct cross-LOD rule

LOD `L` reads every `2^L`-th voxel — standard dual contour at that
stride. That's the per-cluster baseline. Nothing else applies in the
interior.

The edge-alignment override, per face, before meshing that face's
boundary layer:

  effective_boundary_stride = 2^min(self_lod, neighbor_lod)

The boundary cell layer on that face — one cell row deep — is contoured
and meshed at `effective_boundary_stride`, regardless of which side of
the seam self is on. The override is symmetric: lower LOD wins the edge
alignment, both sides apply the same rule independently against their
own neighbors, both arrive at the same stride at the shared face.

This single rule covers every case:
  - When LODs match (any pair where self_lod == neighbor_lod), min ==
    self_lod, the override is a no-op, and behavior is exactly §1.
  - When LODs differ, the higher-LOD side adapts its boundary layer to
    the lower-LOD side's stride. The lower-LOD side keeps its own
    stride (which equals the override). Both sides end up sampling the
    seam at the same cadence, so the boundary vertices align by
    construction.

The polygon shape on the higher-LOD (coarser-stride) side may span N
samples along the seam and 1 cell deep into self — that's fine, it's
one polygon with a long edge that happens to have N intermediate
vertices. No T-junctions are possible because both sides emit at the
same cadence at the seam; no transition fans needed.

Cross-cluster reads: when self's boundary layer samples a position that
falls outside its own storage (the −X/−Y/−Z faces — the "borrow"
direction), `read_corner` already handles the wrap to the neighbor's
voxel data. With the boundary now iterated at the lower stride, the
read just lands on the neighbor's data at that finer stride — which
exists because the lower-LOD neighbor has data at every position of its
own stride. The §3a decode-with-neighbor's-stride is the right rule;
no snapping needed.

NO transition fans, NO transition rectangles with extra T-points, NO
ownership flips. The handoff doc voxel-crosslod-seam-handoff.md
proposes a fan-based approach — that document is SUPERSEDED. The
override-stride model above is the actual rule.

## What to do

1. Revert §3b v2 entirely from mesh.rs:
     - Remove `emit_neg_seam_takeover` and its post-loop call.
     - Remove `plus_neighbor_skip` and its guard in the main loop.
     - Remove the `s_n.max(stride)` snap in `cell_vertex`; restore the
       §3a behavior — cross-cluster reads decode with the neighbor's
       stride directly, no snap.

2. Implement the per-face boundary-stride override.
     - At the start of mesh(), compute per_face_stride[6] = 2^min(self,
       neighbor) for each face that has a neighbor; self's stride
       otherwise.
     - The main iteration loop already samples at self.stride. The
       boundary layer (the single sample row immediately adjacent to
       each face) needs to iterate at that face's `per_face_stride`
       instead. When `per_face_stride == self.stride` this is a no-op.
     - When `per_face_stride < self.stride`, the override is active.
       Iterate the in-axis directions of the boundary row at the
       smaller stride. The 4-cell gather for those edges reads finer
       cells — for cross-cluster cells (−sides), `read_corner` already
       routes to the neighbor's data at the finer stride. For the
       in-self half of the gather (when self is the coarser side), the
       cells at intermediate fine positions don't exist in self's own
       storage; this is the part that needs careful design — see the
       open question below.

3. Validate against the diagnostic. At centre LOD 1 with all neighbors
   at LOD 0, the rebuild log should show unshared/over-shared close to
   the uniform-LOD baseline (15,408 / 218 at LOD 0). The current state
   shows 64,065 / 266 — the 48,657 extra unshared are the T-junctions
   the §3b v2 code creates because both sides emit at different cadences
   at the seam. After the override fix, that delta should be near zero.

## Open question to flag with the author before coding

When the coarser side iterates its boundary layer at finer stride, the
4-cell gather for an edge in that layer has cells at intermediate
positions that don't exist in self's coarse-stride storage. Two
candidates for what to do, both consistent with the rule but with
different mechanics:

  (a) Read everything in the boundary layer's gather from the finer
      neighbor via cross-cluster reads, including the cells that are
      geometrically "in" self's territory. The neighbor has data at
      every fine position; the coarse side just reads it. The coarse
      side's own coarse cells in that row aren't used for boundary
      emissions; they're still used for the interior emissions one row
      in.

  (b) Have contour pre-populate the coarse side's boundary slots at
      finer stride when adjacent to a finer neighbor. Adds a neighbor-
      LOD-aware contour pass. More like a Transvoxel approach.

Surface this to the author before implementing; the project author has
been explicit they don't want trapdoor cleverness, and (a) is the
simpler reading of "we read voxels from our own cluster at the lower
stride — except where the data lives in the neighbor, which is fine."