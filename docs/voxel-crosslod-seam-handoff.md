# Handoff — multi-cluster + LOD meshing, cross-LOD seams (Fork3, from QEF Stable)

> **New-session handoff.** The repo was rolled back to the **STABLE QEF
> contouring** point on branch **Fork3** after an earlier multi-cluster/LOD
> attempt (B1–B3) went down a wrong path on cross-LOD seams. This doc is the
> starting context: what's here, the approach we've converged on, the trap to
> avoid, and a staged plan. Read it before writing meshing code.

## Working conventions (how this project runs)
- **Delete-don't-patch.** This is greenfield. When an approach is wrong, remove
  it cleanly; don't layer fixes on a broken model.
- **Incremental with visible checkpoints.** Each step must build, pass
  `cargo test -p flicker-voxel`, and produce a *renderable* result the author can
  eyeball in `cargo run --release -p voxel-cluster`. Stop at checkpoints; don't
  one-shot a large speculative triangulation.
- **Flag real forks** in the doc/PR rather than silently inventing architecture;
  be decisive on mechanics, surface genuine design choices for the author.
- **No bench-racing the spec.** If a construction resists a clean single pass,
  land the verifiable subset and report exactly what remains.

## Current state at this rollback (verified by reading the source)
Working and stable:
- `contour(primitive: &dyn Primitive, material: Material) -> Cluster` — per-cell
  dual contouring, **single cluster, LOD 0**. Each active cell's QEF dual vertex
  is stored on its **min-corner voxel** as a `CornerVector`; `material` records
  grid-point solidity independently (an empty voxel can carry an active cell's
  vertex). `QEF_LAMBDA = 0.01`. Origin-based sampling.
- `mesh(cluster: &Cluster) -> ClusterMesh` — per-cell DC connectivity, **single
  cluster, LOD 0**. For each sign-changing axis edge, the 4 surrounding cells →
  one quad; `push_quad` orients winding by the solid→empty expected normal;
  vertices duplicated per quad (flat-shaded). `CELL_DIM = 255`. Coords outside
  `[0,256)` are treated as empty → cluster borders are open.
- Substrate present but **unused by mesh/contour yet**:
  - `Lod` (`neighbor.rs`): levels 0–7, `stride()=2^L`, `sample_dim()=256>>L`,
    `cell_dim()=sample_dim-1`. **Sample sets are nested** (LOD `L+1` ⊂ LOD `L`) —
    this nesting is what makes cross-LOD alignment possible; rely on it.
  - `NeighborContext` (6 faces, each `Option<(&Cluster, Lod)>`), `FaceDir`,
    `read_corner(cluster, neighbors, vx, vy, vz) -> Voxel` (routes a single-axis
    OOB read to the matching face neighbor; **currently ignores the neighbor's
    LOD** — the `Some((src, _))` — and does not snap; that's fine for uniform LOD
    and must be addressed for cross-LOD reads, see §3).
  - `ClusterId` (packed `[LOD:4][x:10][y:8][z:10]`, `world_offset()=[x,y,z]*256`),
    `ClusterMap` (`HashMap<ClusterId,Cluster>`).
- `neighbor.rs` module doc already states the governing principle: *"boundary
  continuity comes from reading the neighbor's authoritative data directly, not
  from materializing redundant halo slabs."* Keep that.

Not written (all of it rolled back): multi-cluster mesh, LOD-aware contour/mesh,
any cross-LOD seam handling.

Existing docs and how to treat them:
- **`docs/voxel-seam-design.md` — AUTHORITATIVE for uniform-LOD seams.** The
  alignment-invariant approach: each cluster extends its boundary cell row by one
  and computes the straddling cell's vertex from shared corner data read across
  the seam, so adjacent clusters produce **byte-identical** seam vertices and the
  surfaces meet with no crack — *no separate seam mesh, no bridging*. Follow it
  for §1.
- **`docs/voxel-multicluster-lod-seams.md` — SUPERSEDED for cross-LOD.** Its
  cross-LOD section describes the *bridging* approach this handoff rejects (see
  "The trap" below). Ignore its cross-LOD/T-junction sections; its LOD-aware
  contour/mesh mechanics (stride sampling) are still fine as reference for §2.
- `docs/voxel-mesh-regen.md` — the per-cell DC mesh spec; still accurate for the
  single-cluster base.

## The geometry, in plain terms (so the approach is obvious)
Two clusters share a plane. Each independently places **one surface vertex per
active cell**, solved from its own local data.

- **Uniform LOD:** both sample the plane at the same spacing. If both compute
  their boundary vertices from the *same shared corner data in the same world
  frame with the same arithmetic*, they land on **identical** positions → every
  triangle edge on one side has a twin on the other → watertight. That's
  `voxel-seam-design.md`. No bridging needed; alignment is a consequence of
  shared inputs.

- **Cross-LOD:** the fine side samples the plane twice as densely as the coarse
  side. Now the two boundary vertex rows have **different spacing and don't line
  up**. The killer is the **T-junction**: a fine vertex sits in the middle of a
  coarse edge; the long coarse edge passes through a point the fine side treats as
  a corner, and as the two surfaces curve apart a thin triangular crack opens.
  Watertight still requires *shared edges* — every edge endpoint matched on both
  sides.

## The approach (chosen): match cadence at the boundary, fan inside the owner
**Do not bridge across the seam.** Restore the alignment invariant by making the
two sides **agree on the seam cadence**, then build the resolution transition
*inside the fine cluster from its own cells*.

1. **Mesh the seam itself at the coarse cadence, on both sides.** The coarse
   cluster does this naturally. The **fine cluster meshes its one boundary cell
   row at the coarse neighbor's (lower) cadence** — i.e. for the row of cells
   touching the seam, it places boundary vertices at the coarse spacing, computed
   from the same shared corner data the coarse side uses (nested sample sets make
   the coarse samples a subset of the fine ones, so this is exact). Result: both
   clusters again produce **identical seam vertices** → the seam is watertight by
   the same alignment invariant as the uniform case. The only cross-cluster read
   is *positions to match* (read the neighbor's boundary corner data), never a
   reach-across to *build triangles from*.

2. **Build the 2→1 transition fan inside the fine cluster.** The fine cluster's
   boundary row is now at coarse cadence (vertices every 2 units); its first
   *interior* row is at full fine cadence (every 1 unit). The strip between them —
   the fan that reconciles two fine vertices to one coarse vertex — is built
   **entirely from the fine cluster's own cells**, where it has full connectivity
   and consistent vertex depths. Nothing reaches across the seam to triangulate.
   The fine interior stays full detail; only its last row tapers to the neighbor.

This is Transvoxel's transition cells, reframed as *"the fine side down-samples its
seam row to the neighbor's cadence and tapers internally"* — which fits our model
(each cluster owns its own geometry) and avoids the lookup-table machinery.

**Ownership rule:** the **finer** side adapts (down-samples its boundary row) when
adjacent to a coarser neighbor. The coarser side meshes normally. Equal LOD →
`voxel-seam-design.md` unchanged.

## The trap (what the rejected attempt did — do not repeat)
The B3 attempt **bridged across the seam**: the coarse side reached over and tried
to triangulate between its single coarse vertex and the fine neighbor's scattered
vertices read across. Three failures fall out of bridging, all observed:
- **No connectivity.** It read loose neighbor *vertices*, never the neighbor's
  *surface connectivity*, so it could not reproduce the neighbor's real edges — it
  invented a different surface ("not finding a coherent surface on the neighbor
  side").
- **Depth mismatch → folded triangles.** On a slope the coarse vertex and the fine
  vertices sit at very different positions along the seam normal; bridging
  triangles between them fold over each other (overlapping pairs).
- **Per-footprint coverage gaps.** Fanning a coarse apex to only the active fine
  cells in each coarse box caught ~1–2 of 4 cells on a slope → thin single
  triangles covering ~20% of the gap.

Root lesson: **bridging forces one side to reconstruct the other side's mesh from
raw vertex soup.** The fix is the opposite — make the seam vertices *coincide* (so
no reconstruction is needed) and keep all transition geometry *inside the owning
cluster*.

## Staged plan (checkpoint at each step)
**§1 — Multi-cluster, uniform LOD.** Implement `voxel-seam-design.md`:
`mesh(cluster, neighbors: &NeighborContext, lod: Lod)` (carry `lod` from the start
even though §1 is LOD-0, so §2/§3 don't re-thread it), boundary cell row extended
by one, straddling-cell vertices computed from shared data via `read_corner`,
emitted in owner-local coords (the example translates each cluster by its world
offset). **Checkpoint:** a 3×3 uniform-LOD field is watertight at interior joins;
world edges open; single-cluster output unchanged.

**§2 — LOD-aware contour + mesh (single cluster).** Add `Lod` to `contour` and the
mesh sample iteration: sample at `stride`, store/decode the corner in cell units
(`corner = (v − origin)/stride`; decode `origin + corner·stride`). LOD 0 must stay
byte-identical. **Checkpoint:** a single cluster at LOD 2–3 renders coarser but
recognizable and watertight; LOD 0 unchanged. (Reference the stride mechanics in
`voxel-multicluster-lod-seams.md` §B1 — mechanics only, not its seam approach.)

**§3 — Cross-LOD seam (the new approach above).** Two parts:
- **Correct cross-LOD reads.** When reading a neighbor stored at a *different* LOD,
  snap the read to the neighbor's stride and decode the fetched corner in the
  **neighbor's frame** (its stride) shifted by the owner↔neighbor offset on the
  out-of-range axis. Re-derive this cleanly and small — it's a position transform,
  shaped for "read positions to match," not "read cells to bridge."
- **Fine side down-samples its seam row + internal taper** per the approach.
  **Checkpoint:** a mixed-LOD field (one cluster one level coarser than its
  neighbors) is watertight at the cross-LOD joins — verify up close in wireframe
  (no overlapping triangles, no T-gaps) and by flying underneath (no see-through).

Keep adjacency to **≤1 LOD level between neighbors** (standard; bounds the
transition to 2→1). Assert it.

## Fallback worth knowing (don't build unless asked)
If guaranteed-watertight-for-traversal is needed *before* the seam work is elegant,
**skirts** — a short vertical curtain hung down from each cluster's boundary edge —
are the dead-simple, can't-leak option many shipping voxel engines use. They waste
a little geometry and can poke through on convex edges, but they never crack. It's
a pragmatic floor, not the goal.

## First task for this session
Start at **§1** (multi-cluster uniform-LOD seams per `voxel-seam-design.md`),
landing the checkpoint before touching LOD. Confirm the §1 plan (or flag any
divergence from `voxel-seam-design.md`) before writing code.
