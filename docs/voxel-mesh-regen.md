# Spec — Cluster → Mesh regeneration + flat-field render wiring

Status: ready to implement. Scope is one slice: turn a contoured `Cluster`
into a renderable mesh and display it in the `voxel-cluster` example. This
is the stage after `contour()` in the pipeline:

```
primitive → contour to voxel (QEF) → [THIS SLICE: cluster → mesh → draw]
```

The contour stage (`flicker-voxel/src/{primitive,qef,contour}.rs`) is
already implemented and produces a `Cluster`. Nothing renders it yet, so
the example still draws only the empty bounding box. This slice closes that
gap for the **flat field** test input.

---

## 1. Goal & success criterion

Running `cargo run --release -p voxel-cluster` shows a **flat grey plane at
the cluster's half-height (y = 128)** filling the interior of the existing
wireframe box, flat-shaded, viewable/flyable with the existing controls.
`cargo test -p flicker-voxel` stays green, with new mesh tests added.

Non-goals (explicitly *not* this slice): curved/3D primitives, cross-cluster
seams/neighbor reads, LOD, the contour per-cell rework (see §9), octree
storage.

---

## 2. What exists to build against (verified APIs — do not guess)

**`flicker-voxel` (pure data, no graphics dep — keep it that way):**
- `contour(primitive: &dyn Primitive, material: Material) -> Cluster`
- `FlatField::at_half() -> FlatField` (implements `Primitive`)
- `Cluster`: `get(LocalCoord) -> Voxel`, `CLUSTER_DIM = 256` (u32),
  `override_count()`.
- `Voxel`: `.corner() -> CornerVector`, `.material() -> Material`.
- `CornerVector::to_components() -> [f32; 3]` (each axis in `[-0.5, 1.5]`).
- `Material`: `.raw() -> u32`, `Material::EMPTY`, `Material::new(p,s,b) -> Option<Material>`.
- `LocalCoord::new(u32,u32,u32) -> Option<LocalCoord>`.

**Owned-vertex formula** (the single most important fact): the surface
vertex carried by voxel `(x,y,z)` is, in cluster-local voxel units,
```
p(x,y,z) = [x, y, z] + voxel.corner().to_components()
```
A voxel's center is `(x+0.5, y+0.5, z+0.5)`; the default corner
`(0.5,0.5,0.5)` puts the owned point at the center.

**Solidity convention:** voxel is solid ⇔ `material() != Material::EMPTY`.

**`flicker-render` (via `flicker::render`):**
- `MeshVertex { position: [f32;3], normal: [f32;3], material: u32 }`
- `renderer.upload_mesh(&[MeshVertex], MeshIndices) -> MeshHandle` (call once)
- `MeshIndices::U32(&[u32])`
- `renderer.draw_mesh(handle, model: Mat4, MeshDrawOptions)` (per frame)
- `MeshDrawOptions { wireframe: bool, tint: [f32;4] }`, `::default()` =
  filled, no tint. The fill shader derives a base color by hashing
  `material` and applies Lambertian shading from the vertex normal.
- Exports available from `flicker::render`: `Mat4`, `Vec3`, `MeshVertex`,
  `MeshIndices`, `MeshDrawOptions`, `MeshHandle`, `Camera`, `Renderer`.
- `id.world_offset() -> [f32;3]` (cluster's world origin).

Reference for end-to-end mesh usage: `examples/mesh-smoke/src/main.rs`
(per-face duplicated vertices, `upload_mesh` in `init`, `draw_mesh` in
`render`).

---

## 3. Connectivity model — per-cell dual contouring

Vertices come from cells; quads come from sign-changing edges.

**Definitions** (axes a,b,c are a permutation of X,Y,Z; `e_a` is the unit
step along a):
- **Grid point / voxel** `(x,y,z)`, integer, `0 ≤ · < 256`. Solidity
  `s(x,y,z) = cluster.get(coord).material() != Material::EMPTY`. Any
  coordinate outside `[0,256)` is treated as **empty** (no neighbor cluster
  yet — this is what leaves cluster-border faces open for now).
- **Cell** `(x,y,z)`, integer, `0 ≤ · < 255` (i.e. `CLUSTER_DIM - 1` per
  axis). Its 8 corners are voxels `(x+i, y+j, z+k)` for `i,j,k ∈ {0,1}`.
- **Active cell:** its 8 corner solidities are not all equal.
- **Cell vertex:** `p(x,y,z)` of the cell's **min-corner voxel** `(x,y,z)`
  (the owned-vertex formula in §2). *This is the storage convention this
  slice assumes; see §9 for why it holds for the flat field and what must
  change before curved shapes.*
- **Active edge:** an axis-aligned edge between adjacent voxels `g` and
  `g + e_a` with `s(g) != s(g + e_a)`.

**Quad emission:** for each active edge along axis `a` at grid point `g`,
the four cells sharing that edge are the min-corners
```
(g_a, g_b,   g_c  )
(g_a, g_b-1, g_c  )
(g_a, g_b,   g_c-1)
(g_a, g_b-1, g_c-1)
```
(written in a,b,c order; substitute back into x,y,z). Gather those four
cells' vertices → one quad → two triangles. **If any of the four cell
coords is outside `[0,255)`, skip the edge** (cluster border).

**Iteration to find active edges without double-counting:** scan every
voxel `g` in `[0,256)³`; for each of the three **positive** axes
`a ∈ {+X,+Y,+Z}`, test the edge `g → g+e_a`. Only emit when
`g+e_a` is in range and solidity differs. Each interior edge is visited
exactly once this way.

**Winding / normal (robust, no hand-tabulated cases):**
1. Build the quad's four vertex positions in a fixed corner order
   (e.g. the four cells listed above, in that order).
2. Compute the geometric normal `n = normalize((v1 - v0) × (v2 - v0))`.
3. The surface should face solid→empty. The expected direction is
   `±e_a`: `+e_a` if `s(g)` is solid (and `g+e_a` empty), `-e_a` otherwise.
4. If `dot(n, expected) < 0`, reverse the quad's vertex order (and negate
   `n`).
5. Triangulate as `(v0,v1,v2)` and `(v0,v2,v3)`.

**Flat shading:** emit **four fresh vertices per quad** (no vertex sharing),
all carrying the oriented face normal `n`. This gives crisp flat facets and
a clean wireframe overlay (same rationale as `mesh-smoke`). Indices are
local to the growing vertex list.

**Material per quad:** use the material of the **solid** endpoint of the
active edge (`g` if solid, else `g+e_a`): `material.raw()` into
`MeshVertex.material`. (Flat field → all grey → uniform color.)

**Worked check (flat field).** Solid for voxel `y ≤ 127`, empty `y ≥ 128`.
Active edges are the `+Y` edges at every `(x,z)` between `(x,127,z)` solid
and `(x,128,z)` empty. For such an edge the four cells are
`(x,127,z), (x-1,127,z), (x,127,z-1), (x-1,127,z-1)`, whose min-corner
voxels each carry corner `(0.5,1.0,0.5)` → owned points
`(x±0.5, 128, z±0.5)`. Quad centered at `(x,128,z)`, normal `+Y`. Tiling
over all interior `(x,z)` yields one continuous flat plane at `y=128`;
the `x=0`/`z=0` border edges are skipped.

---

## 4. New module: `flicker-voxel/src/mesh.rs`

Keep `flicker-voxel` graphics-free: define a render-agnostic output type.

```rust
/// One flat-shaded mesh vertex in cluster-local voxel units. Field-for-
/// field convertible to flicker-render's MeshVertex (the example does the
/// mapping, so this crate stays graphics-free).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClusterVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

/// CPU mesh: vertices plus a u32 triangle-index list.
#[derive(Clone, Debug, Default)]
pub struct ClusterMesh {
    pub vertices: Vec<ClusterVertex>,
    pub indices: Vec<u32>,
}

impl ClusterMesh {
    pub fn is_empty(&self) -> bool { self.indices.is_empty() }
    pub fn triangle_count(&self) -> usize { self.indices.len() / 3 }
}

/// Regenerate a renderable mesh from a contoured cluster via per-cell
/// dual contouring (see docs/voxel-mesh-regen.md §3).
pub fn mesh(cluster: &Cluster) -> ClusterMesh { /* ... */ }
```

Implementation notes:
- Factor the quad builder into a private testable helper, e.g.
  `fn push_quad(out: &mut ClusterMesh, cells: [[i32;3];4], expected_normal: [f32;3], material: u32)`
  that does the orient-and-triangulate of §3. Unit-test this directly on
  synthetic inputs so winding is verified without a full cluster scan.
- A small inline vec3 helper set (`sub`, `cross`, `normalize`, `dot`) is
  fine; no `glam` dependency in this crate.
- Reading a cell vertex: look up `cluster.get(LocalCoord::new(...)?)`,
  apply the owned-vertex formula. Cells are only looked up after the
  in-range check, so `LocalCoord::new` will succeed.

Register in `lib.rs`: `mod mesh;` and
`pub use mesh::{mesh, ClusterMesh, ClusterVertex};`.

---

## 5. Example wiring: `examples/voxel-cluster/src/main.rs`

Minimal additions; keep the bounding box, HUD, and controls.

- Imports: add `Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex`
  to the `flicker::render` use; add `contour, FlatField` (and the mesh
  re-exports if needed) to the `flicker_voxel` use.
- Struct field: `mesh: Option<MeshHandle>`.
- `init`:
  1. `let material = Material::new(1, 1, 0).expect("grey");` (placeholder
     grey; the fill shader colors by material hash).
  2. `let cluster = contour(&FlatField::at_half(), material);`
  3. Insert that cluster into the `ClusterMap` (replacing the
     `Cluster::empty()` insert) so the bbox still draws and counts are
     right.
  4. `let cm = flicker_voxel::mesh(&cluster);`
  5. Map to render vertices:
     `let verts: Vec<MeshVertex> = cm.vertices.iter().map(|v| MeshVertex { position: v.position, normal: v.normal, material: v.material }).collect();`
  6. `self.mesh = Some(renderer.upload_mesh(&verts, MeshIndices::U32(&cm.indices)));`
  7. Keep the white-pixel texture line.
- `render`: after the bounding-box loop, draw the mesh at the cluster's
  world offset:
  ```rust
  if let Some(h) = self.mesh {
      let o = ClusterId::new(0,0,0,0).world_offset();
      let model = Mat4::from_translation(Vec3::new(o[0], o[1], o[2]));
      renderer.draw_mesh(h, model, MeshDrawOptions::default());
  }
  ```
  (Single cluster at origin → `model` is effectively identity, but write it
  via `world_offset` so multi-cluster is a trivial later change.)
- Update the HUD line that says "single bare cluster" to reflect that it's
  now a contoured flat field (cosmetic).

The existing spawn pose (`pos (128, 340, -180)`, pitch `-0.6`) already
frames the box; the plane at `y=128` sits inside it.

---

## 6. Tests (`flicker-voxel/src/mesh.rs`, `#[cfg(test)]`)

Keep them fast — avoid the full 8M-voxel flat fill in tests.

1. **`push_quad` winding** (synthetic, no cluster): give four coplanar
   corners and an expected normal; assert the emitted triangles' geometric
   normal has positive dot with the expected direction, and that a flipped
   expected direction reverses the order. Assert 4 vertices / 6 indices per
   quad and that all emitted normals equal the oriented face normal.
2. **Small cube** (`contour(&CubeField{n:3})` reused/!exposed from contour
   tests, or a local copy): `mesh()` returns a non-empty mesh; every index
   `< vertices.len()`; `vertices.len() % 4 == 0`; `indices.len() % 6 == 0`;
   every normal is unit length (±1e-3); the surface is closed-ish (no need
   to assert manifold — just sane counts).
3. **One-cell flat patch** (synthetic cluster, no full fill): build a
   `Cluster`, set a 2×2 block of `(x,127,z)` voxels solid with corner
   `(0.5,1.0,0.5)` and leave `y=128` empty, then `mesh()` it and assert at
   least one quad whose vertices all have `y ≈ 128` and normal `≈ (0,1,0)`.
   This verifies the flat result end-to-end cheaply.

(The genuine full-cluster flat field is verified visually by running the
example, not in the unit suite, because of the fill cost — see §8.)

---

## 7. `flicker-render` changes

None expected — the mesh pipeline already exists (`upload_mesh`,
`draw_mesh`, `MeshVertex`). If the build complains that any of the named
render exports aren't reachable from `flicker::render`, surface that rather
than working around it; `mesh-smoke` imports all of them, so they should be.

---

## 8. Performance expectations (call out, don't fix here)

`contour(FlatField)` writes every solid voxel — ~8.3M `HashMap` inserts for
the flat field — and `mesh()` scans ~255³ cells. Both are slow in debug.
**Run the example with `--release`.** This is the known `Cluster`
octree-storage TODO, not new debt; do not optimize it in this slice.

---

## 9. Design decision — per-cell vs per-solid-voxel (READ BEFORE CODING)

The mesh-regen above is **per-cell** dual contouring: a vertex per active
cell, stored at the cell's min-corner voxel. The current `contour.rs`
assigns a vertex per **solid surface voxel** (a solid voxel with an empty
face-neighbor), placed by QEF over its exposed faces.

For the **flat field these coincide exactly**: the active cells are the
`y=127` layer, their min-corner voxels are exactly the solid surface voxels
the contour populated, and the stored corner is the same `(0.5,1.0,0.5)`.
So per-cell `mesh()` renders the current contour output correctly **now** —
no contour change is needed for this slice.

They **diverge for curved/3D shapes** (spheres, pills, boxes): an active
cell whose min-corner voxel is *empty* (solid mass in the cell's `+`
corners) gets no stored vertex under the per-solid-voxel contour, so
per-cell `mesh()` would read a default/garbage corner there → cracks and
spikes.

**Follow-up task (NOT this slice), to do before curved primitives:** rework
`contour.rs` to iterate cells and store one QEF vertex per active cell at
its min-corner voxel (the `Primitive` interface and `qef.rs` stay; only the
iteration and the voxel a vertex is written to change). Track this so it's
done before the first sphere goes in. If you (the implementer) discover a
cleaner storage convention for cell vertices than "min-corner voxel,"
raise it rather than guessing — it touches the `+++ corner` storage model.

---

## 10. Checklist

- [ ] `flicker-voxel/src/mesh.rs` with `ClusterVertex`, `ClusterMesh`,
      `mesh()`, private `push_quad` helper.
- [ ] `lib.rs`: `mod mesh;` + `pub use mesh::{mesh, ClusterMesh, ClusterVertex};`.
- [ ] Tests 1–3 from §6; `cargo test -p flicker-voxel` green.
- [ ] `voxel-cluster/main.rs` wired per §5.
- [ ] `cargo run --release -p voxel-cluster` shows the flat grey plane at
      mid-height inside the box.
- [ ] Report: did per-cell `mesh()` need any contour change for the flat
      field? (Expected: no.) Any render-export drift?
