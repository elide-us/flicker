# Prompt — virtual-voxel inspector (click-to-select + dual-cell wireframe)

> **For a fresh Claude Code session on branch Fork3.** Adds an interactive
> inspector to the `voxel-cluster` example: left-click a meshed face, and the
> engine draws the wireframe of the **virtual voxel** (dual cell) at that
> lattice point — its true deformed shape, built from the eight surrounding
> voxels' stored vectors. Inspect-only; no editing. Baseline:
> `cargo test -p flicker-voxel` → green; the 3×3 field renders with working
> cross-LOD seams.

## Working conventions (unchanged)
Delete-don't-patch; incremental with a visible checkpoint per stage; flag real
forks; keep `cargo test -p flicker-voxel` green. The author runs this session;
deliver in stages and stop at each checkpoint for a render check
(`cargo run --release -p voxel-cluster`).

## The architectural rule this feature exists to respect
`cluster.get` returns **truth** — the stored voxel or `base`, never a fabricated
or filtered value. (A prior bug put LOD-stride filtering inside `get`; it
returned `base` for real data and cost an enormous debugging detour. Never
again.) This inspector needs *fabricated* data — undefined corners default to a
clean lattice position for display — so that fabrication lives in a **viz-layer
function**, never on `Cluster`. Do **not** add `cluster.get_voxel` or any second
accessor on `Cluster` with different honesty semantics; two near-identical
accessors where one lies by design is exactly the trap that caused the bug.

## Concept: the virtual voxel is a dual cell centered on a lattice point
The voxel field stores, per active cell, one dual vertex on the cell's
min-corner voxel as a `CornerVector` (offset from that voxel's min corner,
decodable range `[-0.5, 1.5]`, default `(0.5,0.5,0.5)` = cell center). The
*surface* is the network of those dual vertices.

A **virtual voxel** is the dual cell centered on a grid point `p`: a cube whose
8 corners are the dual vertices of the 8 primal cells meeting at `p`. Each corner
is **owned by a different voxel** — the one occupying that octant around `p` —
and we read the owner's stored vector to place the corner. Because each owner's
default vertex is its own cell center, the undeformed dual cell is the
axis-aligned unit cube `[p-0.5, p+0.5]³`, centered on `p`. Active (surface)
cells pull their corner off-lattice → the deformed "weird shapes" we want to see.

### Octant → owner mapping (load-bearing — implement exactly)
For dual-cell center `p = (px,py,pz)` in cluster-local grid coords, index the 8
corners by `o ∈ 0..8` with bits `(bx,by,bz) = (o&1, (o>>1)&1, (o>>2)&1)`, where
bit `1` = the `+` side of `p` on that axis and bit `0` = the `-` side.

| quantity | formula |
|---|---|
| owner min-corner `m` | `m = p + (bx-1, by-1, bz-1)` → owners range over `p + {-1,0}³` |
| owner's stored vector `V` | `display_corner(cluster, m)` (truth, or `DEFAULT` if undefined/OOB) |
| **owner-relative** translation | `V.to_components()` — offset from the owner's *own* min corner `m` |
| world corner position | `cluster_origin + m + V.to_components()` |
| **self-relative** translation | `world_corner - cluster_origin - p` = `(m - p) + V.to_components()` |

Sanity check of the model: the `---` corner (`o=0`, bits `0,0,0`) has owner
`m = p-(1,1,1)`; its default vector `(0.5,0.5,0.5)` is that voxel's own `+++`
reach (toward `p`), landing the corner at `p-0.5`. That is "the `---` corner
borrows the `p-(1,1,1)` voxel's `+++` vector" — same world point, two frames.

For a default owner, `self-relative` is `bit - 0.5` per axis (`-0.5` on the `-`
side, `+0.5` on the `+` side) → corner at `p ± 0.5`, the clean lattice cube.

### Maintain both translations
The per-corner data structure keeps **both** the owner-relative vector (the value
as the owning voxel stores it, the provenance/write-back handle) **and** the
self-relative vector (the same world point expressed from `p`, what we render and
reason in). They are the same world position in two coordinate frames; store both.

Suggested shape (viz layer, not in `flicker-voxel` storage types):
```
struct VirtualVoxelCorner {
    owner_local: [i32; 3],   // m, the owning voxel's min corner (cluster-local)
    owner_relative: [f32; 3],// V.to_components(): offset from owner min corner
    self_relative: [f32; 3], // offset from p (the dual-cell center)
    world: [f32; 3],         // absolute world position (for rendering / picking)
}
struct VirtualVoxel { center_local: [i32;3], cluster: ClusterId, corners: [VirtualVoxelCorner; 8] }
```

### The viz accessor (fabrication lives here)
```
// Returns the owner's stored corner if in range, else the display default.
// In range → cluster.get(coord).corner() (TRUTH). Out of [0,256) on any axis,
// or undefined → CornerVector::DEFAULT. This is a display fabrication and is
// deliberately NOT a Cluster method.
fn display_corner(cluster: &Cluster, m: [i32;3]) -> CornerVector
```
Cross-cluster owners (when `p` sits on a cluster boundary, some `m` fall into a
neighbor at `-1` or `256`) → **default for now** (documented limitation; reading
the neighbor across is a later refinement, not v1). This matches "undefined
emits default values."

## Picking pipeline
1. **Retain CPU mesh for ray-casting.** The example currently uploads each
   `ClusterMesh` and keeps only the `MeshHandle`, discarding triangles — there's
   nothing to ray-cast. In `rebuild`, also store per-cluster **world-space**
   triangle data for picking: the vertex positions (offset by `world_offset()`)
   and the `u32` indices. Keep it beside `meshes`.
2. **Left-click edge, with HUD precedence.** Detect a left-button press edge
   (mirror how `ScriptHost::update` derives its `clicked` signal — check what
   `InputState` exposes; add a tracked previous-frame bool if there's no edge
   flag). If the click is inside the HUD panel rect (it drives a checkbox), do
   **not** world-pick — HUD consumes it. Right-drag stays look; left-click is
   pick, no conflict.
3. **Build the picking ray from the camera + viewport.** Needs viewport
   dimensions — use the renderer's surface size (it already computes aspect for
   projection; if no public accessor, add a minimal `surface_size()`).
   Right-handed, Y-up, matching `forward()`:
   ```
   f = forward();  r = f.cross(Vec3::Y).normalize();  u = r.cross(f).normalize();
   aspect = w / h;  t = (fov_y * 0.5).tan();
   ndc_x = 2.0*(mx + 0.5)/w - 1.0;   ndc_y = 1.0 - 2.0*(my + 0.5)/h;  // mouse origin top-left
   dir = (f + ndc_x*aspect*t*r + ndc_y*t*u).normalize();   origin = camera.position;
   ```
   Validate the basis handedness: a center-screen click (`ndc≈0`) must produce a
   ray ≈ `forward()` and pick what's straight ahead. Fix the `r`/`u` signs if not.
4. **Ray–triangle (Möller–Trumbore)** over all retained world-space triangles;
   take the nearest positive hit. Brute force across the 9 clusters is fine.
5. **Hit → dual-cell center `p`.** `hit_world` → owning cluster via
   `floor(hit_world / CLUSTER_DIM)` per axis (match to a `ClusterId` in the map);
   `p = round(hit_world - cluster_origin)` per axis, clamped to `[0, 256]`. `p` is
   the grid point whose dual cell `[p-0.5,p+0.5]³` contains the hit. Store the
   selection `(ClusterId, p)` in app state; it persists until the next pick.

## Rendering
Build the `VirtualVoxel` for the current selection each frame (cheap — 8 reads),
then draw its 12 cube edges via the existing `renderer.draw_lines(&segments,
color)` (the `corner_arrows` path already uses this call). Edges connect octant
corners differing in exactly one bit. Color: a **darker white** to distinguish
from the bright-white cluster box — e.g. `[0.7, 0.7, 0.75, 1.0]`. Segments use
each corner's `world` position.

HUD text (reuse `renderer.draw_text`): show the selection — `p`, its `ClusterId`,
and for one corner (or all 8 if compact) both translations, so the dual-frame
bookkeeping is visible while debugging. Optional: also draw the picked triangle's
outline in a third tint to confirm the ray hit.

## Staging (stop at each checkpoint)
**Stage 1 — pick → readout.** Retain CPU triangles; left-click edge with HUD
precedence; ray build + ray–triangle; compute `(ClusterId, p)`; print/HUD-show it.
No wireframe yet. **Checkpoint:** clicking various faces prints a stable, sensible
`p` that tracks where you clicked (center-screen ray validated against `forward`).
This de-risks the picking math before any geometry depends on it.

**Stage 2 — dual-cell wireframe.** Add `display_corner`, the `VirtualVoxel`
builder (both translations), and the 12-edge `draw_lines` render in darker-white.
**Checkpoint:** the selected virtual voxel draws as an axis-aligned unit cube on
flat/empty regions and as a visibly deformed cube where it straddles the surface;
it sits centered on the clicked lattice point; HUD shows owner-relative vs
self-relative for its corners.

## Out of scope (v1)
- **Editing/placing voxels** — this is inspect-only. (Write-back is why we keep
  owner-relative provenance, but no mutation now.)
- **Cross-cluster owner reads** — boundary owners default; reading the neighbor
  across is later.
- **Provenance-exact quad selection** — `round`-to-grid-point picks the dual cell
  containing the hit; storing per-triangle source-cell IDs in the mesh for exact
  quad mapping is a later option, not needed here.
- **LOD > 0 inspection** — the inspector reads true per-voxel data at stride 1
  (now that `get` tells the truth); coarse-LOD virtual-voxel display is later.

## Files
- `examples/voxel-cluster/src/main.rs` — retained pick meshes, input edge + HUD
  precedence, ray build, ray–triangle, selection state, `display_corner`, the
  `VirtualVoxel` builder, wireframe + HUD draw.
- Possibly `flicker-render` — a `surface_size()` accessor if none is public
  (renderer only; do not touch `flicker-voxel` storage).
- No changes to `cluster.rs` / `get` — fabrication stays in the example's viz layer.
