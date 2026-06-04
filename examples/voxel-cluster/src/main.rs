//! voxel-cluster: a 3×3 cluster field contoured from the split-scene
//! world. Each cluster contours its own region against the shared
//! global primitive; meshing closes the four internal seams (and the
//! interior cluster's all four faces) via the low-side-owns convention
//! in `flicker_voxel::mesh`.
//!
//! Pipeline: 3×3 `ClusterId`s → `contour` per cluster → `ClusterMap`
//! → per-cluster `NeighborContext` → `mesh` → upload one mesh handle
//! per cluster, drawn at its `world_offset()`. The cluster boundary is
//! drawn as a white wireframe box; two debug toggles let the user
//! inspect the meshes interactively (see controls below).
//!
//! Camera controls (rebindable via `Bindings`):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch.
//!   * Escape: quit.
//!
//! Debug toggles are driven by a scripted HUD (`scripts/hud.lua`,
//! loaded at startup via `flicker-script`) — six clickable
//! checkboxes the Lua side owns, replacing the old `1`/`2`/`\` key
//! handling:
//!   * Wireframe overlay on top of the solid mesh.
//!   * Corner-vector arrows — for every stored voxel whose
//!     `CornerVector` differs from the default, draw a line from the
//!     voxel's grid coord to the decoded corner tip. Visualizes where
//!     the contour's QEF placed each active cell's dual vertex.
//!   * Center cluster LOD — flips the centre cluster of the 3×3 field
//!     between LOD 0 and LOD 1, exercising the cross-LOD seam.
//!   * Navmesh wireframe — the LOD2 walkable surface drawn magenta as
//!     floor-to-floor links between walkable-adjacent columns.
//!   * Camera-driven LOD — each cluster's LOD follows its distance from
//!     the camera (smoothed to the mesher's ±1 adjacency invariant),
//!     re-meshing on a swap. Supersedes the manual centre-LOD toggle.
//!   * LOD billboards — a digit per cluster, on the navmesh surface at
//!     the cluster centre, showing that cluster's current LOD.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, App, Bindings, ControlConfig, InputState};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, TextureHandle,
    Vec2, Vec3,
};
use flicker::script::{HudCommand, ScriptHost};
use flicker_voxel::{
    cluster_center_world, contour, in_nav_rings, BakedCluster, Cluster, ClusterId, ClusterMap,
    ClusterNav, CornerVector, FaceDir, LocalCoord, Lod, Material, NeighborContext, Scene,
    CLUSTER_DIM, NAV_DIM,
};

/// Side length of the cluster field, in clusters. A 3×3 row in XZ
/// gives one fully-interior cluster (all four lateral neighbors
/// present), which is what actually exercises seam tangent stitching
/// on every face simultaneously.
const FIELD_DIM: u16 = 3;

struct VoxelCluster {
    /// The cluster map — populated with `FIELD_DIM × FIELD_DIM`
    /// clusters at LOD 0, each contoured against the shared world
    /// primitive at its own world offset.
    map: ClusterMap,

    /// A 1×1 white pixel uploaded once at `init`. The sprite shader
    /// multiplies it by a tint, so this is the "solid colored quad"
    /// primitive used to draw the scripted HUD's checkbox rectangles.
    white: Option<TextureHandle>,

    /// One mesh handle per cluster, paired with the cluster's id so
    /// `render` can draw each at its world offset.
    meshes: Vec<(ClusterId, MeshHandle)>,

    /// LOD2 walkable surface per cluster, derived on the load path
    /// alongside the mesh and gated to rings 0–2 (§4.6). Nothing
    /// consumes it yet — the walking camera is follow-up work — so it
    /// is retained here purely to keep nav on the existing load path,
    /// ready to drop onto the ring scheduler's worker queue unchanged
    /// when that lands.
    navs: Vec<(ClusterId, ClusterNav)>,

    /// First-person camera state.
    position: Vec3,
    yaw: f32,
    pitch: f32,
    /// Last cursor position while right-dragging, so we can compute a
    /// per-frame delta. `None` when right is not held.
    last_look_cursor: Option<Vec2>,

    bindings: Bindings,
    config: ControlConfig,

    /// The scripted HUD. Owns the three debug-toggle checkboxes; the
    /// fields below are refreshed from it each frame. `None` only if
    /// the script failed to load (the example still runs without it).
    script: Option<ScriptHost>,

    /// Wireframe-overlay second pass on top of the solid mesh. Mirrors
    /// the script's `"wireframe"` checkbox, refreshed each `update`.
    wireframe_on: bool,
    /// Draw corner-vector arrows (precomputed in `init`). Mirrors the
    /// script's `"corner_arrows"` checkbox.
    corner_arrows_on: bool,
    /// State of the script's `"center_lod"` checkbox on the previous
    /// frame, so we rebuild the field only on the toggle edge.
    prev_center_lod_on: bool,
    /// Draw the LOD2 navmesh as a magenta wireframe. Mirrors the
    /// script's `"navmesh"` checkbox, refreshed each `update`.
    navmesh_on: bool,
    /// Cached line segments: from each stored voxel's world grid coord
    /// to its decoded `CornerVector` tip, across all clusters in the
    /// field.
    corner_arrows: Vec<(Vec3, Vec3)>,
    /// Cached navmesh wireframe: one world-space segment per pair of
    /// walkable-linked adjacent columns, across all clusters. Rebuilt in
    /// [`Self::rebuild`] alongside the per-cluster navs (`floor_at` +
    /// `linked`); drawn each frame when `navmesh_on`.
    navmesh_segments: Vec<(Vec3, Vec3)>,

    /// Mirrors the script's `"camera_lod"` checkbox: when on, camera
    /// distance drives each cluster's LOD (see `target_lod_for_cluster`),
    /// superseding the manual centre-LOD toggle.
    camera_lod_on: bool,
    /// Mirrors the script's `"lod_billboards"` checkbox: draw a per-cluster
    /// LOD-digit billboard on the navmesh surface at the cluster centre.
    lod_billboards_on: bool,
    /// Currently-applied per-cluster LOD for the 3×3 field, already smoothed
    /// to the mesher's ±1 cross-LOD adjacency invariant. `rebuild` meshes to
    /// this and the billboards display it; recomputed each `update`.
    lod_field: [[u8; FIELD_DIM as usize]; FIELD_DIM as usize],
    /// Digit-glyph atlas (digits 0–7) for the LOD billboards, uploaded once
    /// in `init`.
    digit_atlas: Option<TextureHandle>,

    /// LOD level of the centre cluster of the 3×3 field. Toggled
    /// between 0 (uniform with neighbours) and 1 (coarser than its
    /// four lateral neighbours) by the HUD's centre-LOD checkbox.
    /// Other clusters stay at LOD 0.
    center_lod_level: u8,
    /// Set by `update` when the centre-LOD checkbox flips; consumed at
    /// the top of `render` (which has `&mut Renderer`) to re-contour +
    /// re-mesh the whole field.
    needs_rebuild: bool,

    /// CPU-side world-space triangle data for ray-casting against the
    /// rendered mesh. One entry per cluster: `(id, world vertex
    /// positions, u32 indices)`. Populated alongside [`Self::meshes`]
    /// in [`Self::rebuild`] — the uploaded `MeshHandle` alone isn't
    /// enough for picking. Brute-force ray–triangle is fine at 9
    /// clusters / a few hundred thousand triangles.
    pick_meshes: Vec<(ClusterId, Vec<Vec3>, Vec<u32>)>,

    /// Most recent pick: `(owning cluster, dual-cell center in
    /// cluster-local grid coords)`. `None` until the first hit; sticky
    /// thereafter.
    selection: Option<(ClusterId, [i32; 3])>,

    should_quit: bool,
}

impl Default for VoxelCluster {
    fn default() -> Self {
        // Camera gets its real pose in `init`. The placeholders here
        // just satisfy the Default bound; nothing renders before init.
        Self {
            map: ClusterMap::new(),
            white: None,
            meshes: Vec::new(),
            navs: Vec::new(),
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            last_look_cursor: None,
            bindings: Bindings::wasd(),
            config: ControlConfig::default(),
            script: None,
            wireframe_on: false,
            corner_arrows_on: false,
            prev_center_lod_on: false,
            navmesh_on: false,
            corner_arrows: Vec::new(),
            navmesh_segments: Vec::new(),
            camera_lod_on: false,
            lod_billboards_on: false,
            lod_field: [[0u8; FIELD_DIM as usize]; FIELD_DIM as usize],
            digit_atlas: None,
            center_lod_level: 0,
            needs_rebuild: false,
            pick_meshes: Vec::new(),
            selection: None,
            should_quit: false,
        }
    }
}

impl VoxelCluster {
    /// Build the example around an already-loaded HUD script. All other
    /// state takes its placeholder values from [`Default`]; the camera
    /// gets its real pose in [`App::init`].
    fn new(script: ScriptHost) -> Self {
        Self {
            script: Some(script),
            ..Self::default()
        }
    }

    /// Unit vector pointing where the camera is looking, derived from
    /// yaw/pitch. Right-handed Y-up.
    fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos())
    }

    /// Horizontal "right" relative to the camera's facing (ignores
    /// pitch so strafing stays in the world XZ plane).
    fn move_right(&self) -> Vec3 {
        let f = self.forward();
        let flat = Vec3::new(f.x, 0.0, f.z).normalize_or_zero();
        flat.cross(Vec3::Y).normalize_or_zero()
    }

    /// Horizontal forward (ignores pitch so WASD doesn't pitch into
    /// the ground or sky).
    fn move_forward(&self) -> Vec3 {
        let f = self.forward();
        Vec3::new(f.x, 0.0, f.z).normalize_or_zero()
    }
}

/// Field-of-view used by [`VoxelCluster::render`]; mirrored here so the
/// picking ray uses the exact same vertical FOV as the projection.
const PICK_FOV_Y_RADIANS: f32 = 60.0_f32 * std::f32::consts::PI / 180.0;

/// Bounding rect of the scripted HUD's checkbox panel, in HUD pixels
/// (origin top-left). Mirrors `scripts/hud.lua`'s `ORIGIN_X`/`ORIGIN_Y`
/// /`ROW_H`/`BOX` plus the row of label text on the right. Generous on
/// purpose: a click anywhere inside this rect is treated as the HUD's,
/// not a world pick. Keep in sync with the Lua side if the layout
/// moves.
const HUD_PANEL_X0: f32 = 12.0;
const HUD_PANEL_Y0: f32 = 152.0;
const HUD_PANEL_X1: f32 = 280.0;
// Six checkbox rows: the Lua panel's last box bottom is
// `ORIGIN_Y + 5*ROW_H + BOX = 180 + 130 + 18 = 328`; pad to ~10px.
const HUD_PANEL_Y1: f32 = 338.0;

impl VoxelCluster {
    /// `true` when the cursor sits on the scripted HUD's checkbox panel
    /// — the script already consumes the click there (toggling a
    /// checkbox), so we must not also fire a world pick on the same
    /// press edge.
    fn cursor_on_hud(cursor: Vec2) -> bool {
        cursor.x >= HUD_PANEL_X0
            && cursor.x <= HUD_PANEL_X1
            && cursor.y >= HUD_PANEL_Y0
            && cursor.y <= HUD_PANEL_Y1
    }

    /// Origin + direction of the picking ray for screen-space cursor
    /// `cursor` on a viewport of pixel size `viewport`.
    ///
    /// Camera basis built to **match** the renderer's view matrix
    /// (`glam::Mat4::look_at_rh` in [`flicker_render::Camera::view`]):
    ///   * `r = f.cross(Y)` — same as `look_at_rh`'s internal right
    ///     vector (`forward × up`). For the example's yaw-0/face-+Z
    ///     pose this resolves to `(-1, 0, 0)`, which means world `+X`
    ///     lands on the **left** half of the screen — counter-intuitive
    ///     until you note that the camera looks *into* `+Z`. Build
    ///     the picking ray with this same convention so screen-left
    ///     clicks fire rays toward world `+X` and screen-right clicks
    ///     toward world `-X`. (Implementing `Y.cross(f)` instead
    ///     produces the inverse and visibly mirrors the pick.)
    ///   * `u = r.cross(f)` — recomputed up; for a pitched-down camera
    ///     this leans `+Y` toward `+Z` (top of view tilts forward),
    ///     matching head-tilt intuition.
    ///
    /// NDC x runs left→right with the pixel x; NDC y runs bottom→top
    /// because the mouse origin is top-left.
    fn build_pick_ray(&self, cursor: Vec2, viewport: Vec2) -> (Vec3, Vec3) {
        let f = self.forward();
        let r = f.cross(Vec3::Y).normalize_or_zero();
        let u = r.cross(f).normalize_or_zero();
        let aspect = viewport.x / viewport.y;
        let t = (PICK_FOV_Y_RADIANS * 0.5).tan();
        // +0.5 so the ray passes through the pixel's centre, not its
        // top-left corner — matters at low resolutions, harmless at
        // high.
        let ndc_x = 2.0 * (cursor.x + 0.5) / viewport.x - 1.0;
        let ndc_y = 1.0 - 2.0 * (cursor.y + 0.5) / viewport.y;
        let dir = (f + r * (ndc_x * aspect * t) + u * (ndc_y * t)).normalize_or_zero();
        (self.position, dir)
    }

    /// Ray–triangle intersection (Möller–Trumbore). Returns the
    /// parametric `t` along `(origin, dir)` for the front-face hit, or
    /// `None` if the ray misses, hits the back face within numerical
    /// tolerance, or lands behind the origin. The mesh emits oriented
    /// quads, so back-face culling here matches what the renderer
    /// shows.
    fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
        let edge1 = b - a;
        let edge2 = c - a;
        let h = dir.cross(edge2);
        let det = edge1.dot(h);
        // Back-face / parallel-ray rejection. Positive det = ray
        // hits the front face (CCW winding when viewed from
        // `origin`); negative det = back face.
        if det <= 1e-7 {
            return None;
        }
        let inv_det = 1.0 / det;
        let s = origin - a;
        let bu = inv_det * s.dot(h);
        if !(0.0..=1.0).contains(&bu) {
            return None;
        }
        let q = s.cross(edge1);
        let bv = inv_det * dir.dot(q);
        if bv < 0.0 || bu + bv > 1.0 {
            return None;
        }
        let t = inv_det * edge2.dot(q);
        if t > 1e-4 {
            Some(t)
        } else {
            None
        }
    }

    /// Walk every retained triangle and return the nearest front-face
    /// hit as `(ClusterId, world point)`. Brute force over the field's
    /// ~9 cluster meshes: fine for 9 × a few hundred thousand
    /// triangles at click rate. Add spatial acceleration when the
    /// field grows.
    fn try_pick(&self, cursor: Vec2, viewport: Vec2) -> Option<(ClusterId, Vec3)> {
        let (origin, dir) = self.build_pick_ray(cursor, viewport);
        let mut best: Option<(f32, ClusterId)> = None;
        for (id, verts, indices) in &self.pick_meshes {
            for tri in indices.chunks_exact(3) {
                let a = verts[tri[0] as usize];
                let b = verts[tri[1] as usize];
                let c = verts[tri[2] as usize];
                if let Some(t) = Self::ray_triangle(origin, dir, a, b, c) {
                    if best.map_or(true, |(bt, _)| t < bt) {
                        best = Some((t, *id));
                    }
                }
            }
        }
        best.map(|(t, id)| (id, origin + dir * t))
    }

    /// Snap a world-space hit point to the nearest dual-cell center
    /// (lattice grid point) inside the owning cluster: subtract the
    /// cluster's world offset, round each axis, clamp to `[0,
    /// CLUSTER_DIM]`. The range is `[0, 256]` (inclusive both ends) so
    /// hits exactly on a face plane resolve to the boundary grid
    /// point.
    fn hit_to_local_p(id: ClusterId, hit_world: Vec3) -> [i32; 3] {
        let off = id.world_offset();
        let dim = CLUSTER_DIM as i32;
        let snap = |w: f32, o: f32| (w - o).round().clamp(0.0, dim as f32) as i32;
        [
            snap(hit_world.x, off[0]),
            snap(hit_world.y, off[1]),
            snap(hit_world.z, off[2]),
        ]
    }
}

// ---------- virtual-voxel viz layer (Stage 2 of the inspector) ----------
//
// A "virtual voxel" is the dual cell centred on a grid point `p` — a
// cube whose 8 corners are the dual vertices of the 8 primal cells
// meeting at `p`. Each corner is owned by a different voxel: the one
// occupying that octant around `p`. We *read* each owner's stored
// corner to place its octant; if the owner's storage is empty (no
// override, or out of cluster bounds), the display defaults to the
// owner's cell-centre, which puts the corner at `p ± 0.5` — the clean
// unit lattice cube. The "weird shapes" appear at the surface where
// stored corners pull the cube off-lattice.
//
// `Cluster::get` returns truth: stored override or `base`. Display
// defaults belong here, in the viz layer, not on `Cluster`. (A prior
// bug put LOD-stride filtering inside `get`; it returned `base` for
// real data and cost a brutal debug detour. Never again — two
// near-identical accessors where one lies by design is exactly the
// trap.) Hence the free function `display_corner` and explicit
// `VirtualVoxelCorner` struct that records both translations.

/// Returns the owner voxel's stored corner if `m` lies inside the
/// cluster's `[0, CLUSTER_DIM)³` range, else [`CornerVector::DEFAULT`].
/// The in-range path goes through [`Cluster::get`] — the storage's one
/// source of truth — and reads `.corner()`. The OOB path is the
/// *display* default; v1 does not chase cross-cluster owners.
///
/// Deliberately a free function (not a `Cluster` method) — fabricated
/// defaults on display-time queries must never be confused with the
/// truthful in-storage accessor.
fn display_corner(cluster: &Cluster, m: [i32; 3]) -> CornerVector {
    let dim = CLUSTER_DIM as i32;
    if (0..dim).contains(&m[0]) && (0..dim).contains(&m[1]) && (0..dim).contains(&m[2]) {
        let coord = LocalCoord::new(m[0] as u32, m[1] as u32, m[2] as u32).expect("range checked");
        cluster.get(coord).corner()
    } else {
        CornerVector::DEFAULT
    }
}

/// One corner of a virtual voxel, with both coordinate frames kept
/// explicit. `owner_relative` is the same value the owning voxel
/// stores (the provenance/write-back handle, *if* editing ever
/// arrives); `self_relative` is the same world point expressed from
/// the dual cell's centre `p` — what the renderer and the math reason
/// in. Same point, two frames; storing both keeps the bookkeeping
/// visible while debugging.
#[derive(Copy, Clone, Debug)]
struct VirtualVoxelCorner {
    /// Owning voxel's min-corner in cluster-local grid coords:
    /// `m = p + (bx - 1, by - 1, bz - 1)` per the octant mapping.
    owner_local: [i32; 3],
    /// `V.to_components()`: corner offset from `m` (the owner's own
    /// min corner), range `[-0.5, 1.5]`. The owner's default value of
    /// `(0.5, 0.5, 0.5)` is its cell centre.
    owner_relative: [f32; 3],
    /// `(m - p) + V.to_components()`: same world point expressed from
    /// `p`. For default owners this collapses to `(bx - 0.5, by - 0.5,
    /// bz - 0.5)` → corner at `p ± 0.5`, the clean lattice cube.
    self_relative: [f32; 3],
    /// Absolute world-space corner position (`cluster_origin + m +
    /// V`). The renderer consumes this; picking uses it for hit
    /// outlines.
    world: Vec3,
}

/// A virtual voxel: dual cell centred on a grid point `p` inside the
/// given cluster, with its 8 corners' provenance and translations.
#[derive(Clone, Debug)]
struct VirtualVoxel {
    /// Owning cluster (so callers can format the selection consistently
    /// and so a later edit pass can find the storage).
    cluster: ClusterId,
    /// Cluster-local grid coord of the dual cell's centre.
    center_local: [i32; 3],
    /// Corners indexed by `o ∈ 0..8`, bits `(bx, by, bz) = (o & 1,
    /// (o >> 1) & 1, (o >> 2) & 1)`. Bit `1` = the `+` side of `p` on
    /// that axis; bit `0` = the `-` side. (The `---` corner `o = 0`
    /// borrows the `p - (1, 1, 1)` voxel's `+++` reach — same world
    /// point, two frames.)
    corners: [VirtualVoxelCorner; 8],
}

impl VirtualVoxel {
    /// Build the virtual voxel at lattice point `p` in `cluster` whose
    /// world offset is `cluster_origin`. Eight reads from
    /// `display_corner`, one per octant.
    fn build(cluster_id: ClusterId, cluster: &Cluster, p: [i32; 3]) -> Self {
        let off = cluster_id.world_offset();
        let cluster_origin = Vec3::new(off[0], off[1], off[2]);
        let mut corners = [VirtualVoxelCorner {
            owner_local: [0; 3],
            owner_relative: [0.0; 3],
            self_relative: [0.0; 3],
            world: Vec3::ZERO,
        }; 8];
        for o in 0..8 {
            let bx = (o & 1) as i32;
            let by = ((o >> 1) & 1) as i32;
            let bz = ((o >> 2) & 1) as i32;
            let m = [p[0] + bx - 1, p[1] + by - 1, p[2] + bz - 1];
            let v = display_corner(cluster, m).to_components();
            // self-relative = (m - p) + V = (bx - 1, by - 1, bz - 1) + V.
            // For the default V = (0.5, 0.5, 0.5) this becomes
            // (bx - 0.5, by - 0.5, bz - 0.5) → the corner sits at
            // p ± 0.5 (clean axis-aligned unit cube around p).
            let self_relative = [
                (m[0] - p[0]) as f32 + v[0],
                (m[1] - p[1]) as f32 + v[1],
                (m[2] - p[2]) as f32 + v[2],
            ];
            let world = cluster_origin
                + Vec3::new(m[0] as f32 + v[0], m[1] as f32 + v[1], m[2] as f32 + v[2]);
            corners[o] = VirtualVoxelCorner {
                owner_local: m,
                owner_relative: v,
                self_relative,
                world,
            };
        }
        Self {
            cluster: cluster_id,
            center_local: p,
            corners,
        }
    }
}

/// The 12 cube edges as pairs of octant indices `(o0, o1)`, ordered by
/// the axis the edge runs along (X = bit 0, Y = bit 1, Z = bit 2).
/// Every pair differs in exactly one bit: that's the axis-aligned cube
/// topology. The dual cell's mesh is just `corners[o0].world →
/// corners[o1].world` for each entry.
const CUBE_EDGES: [(usize, usize); 12] = [
    // X-axis edges (toggle bit 0).
    (0b000, 0b001),
    (0b010, 0b011),
    (0b100, 0b101),
    (0b110, 0b111),
    // Y-axis edges (toggle bit 1).
    (0b000, 0b010),
    (0b001, 0b011),
    (0b100, 0b110),
    (0b101, 0b111),
    // Z-axis edges (toggle bit 2).
    (0b000, 0b100),
    (0b001, 0b101),
    (0b010, 0b110),
    (0b011, 0b111),
];

impl VoxelCluster {
    /// Build the [`VirtualVoxel`] for the current selection, if any.
    /// Cheap — eight `cluster.get` reads per call — so callers
    /// recompute every frame instead of caching.
    fn current_virtual_voxel(&self) -> Option<VirtualVoxel> {
        let (id, p) = self.selection?;
        let cluster = self.map.get(id)?;
        Some(VirtualVoxel::build(id, cluster, p))
    }
}

impl VoxelCluster {
    /// Rebuild every cluster's contour and mesh from scratch. Called
    /// once at init and again on every `\` toggle. Cheaper than tracking
    /// dirty bits here — the whole 9-cluster rebuild costs a couple
    /// seconds and a `\` press is a deliberate debug action.
    fn rebuild(&mut self, renderer: &mut Renderer) {
        let material = Material::new(1, 1, 0).expect("grey material is in-range");

        // Per-cluster LOD comes from the smoothed `lod_field` (driven by the
        // camera when "camera_lod" is on, else the manual centre-LOD toggle —
        // see `update`). Copied out so the closure doesn't borrow `self`.
        let lod_field = self.lod_field;
        let lod_for = |x: u16, z: u16| -> u8 { lod_field[x as usize][z as usize] };

        self.map = ClusterMap::new();
        self.meshes.clear();
        // CPU triangles for picking are regenerated alongside the
        // uploaded meshes below; clear the stale entries first.
        self.pick_meshes.clear();
        // A rebuild discards every triangle the previous selection's
        // pick was anchored to, so the selection no longer references
        // anything coherent — drop it.
        self.selection = None;

        let ids: Vec<ClusterId> = (0..FIELD_DIM)
            .flat_map(|x| (0..FIELD_DIM).map(move |z| ClusterId::new(lod_for(x, z), x, 0, z)))
            .collect();

        // Source of voxel data per cluster, in priority order:
        //   1. Bake on disk (LOD-0 only) — fast startup, no procedural
        //      re-evaluation.
        //   2. Contour from the primitive — fallback when the bake is
        //      missing, stale, or this rebuild requested a non-zero
        //      LOD that the LOD-0 bake can't satisfy.
        //
        // The centre-LOD HUD checkbox still drives a re-contour today
        // because the contour itself encodes LOD into the cluster
        // (see `crates/flicker-voxel/src/contour.rs` — per-voxel
        // expansion is sized to the LOD's cell footprint). When the
        // mesh refactor lands and stride becomes a render-time
        // parameter, the bake will satisfy every rebuild.
        let bake_dir = bake_dir_path();
        let all_lod_zero = ids.iter().all(|id| id.lod() == 0);
        let mut loaded_from_bake = false;
        if all_lod_zero {
            if let Some(loaded) = try_load_bake_field(&bake_dir, &ids) {
                tracing::info!(
                    "loaded {} clusters from bake at {}",
                    loaded.len(),
                    bake_dir.display()
                );
                for (id, cluster) in loaded {
                    self.map.insert(id, cluster);
                }
                loaded_from_bake = true;
            }
        }
        if !loaded_from_bake {
            // Contour every cluster at its LOD. Contour is per-cluster
            // and oblivious to neighbour LODs; cross-LOD seam handling
            // lives entirely in mesh.
            for id in &ids {
                let scene = Scene::world_at(id.world_offset());
                self.map.insert(*id, contour(&scene, material, *id));
            }
        }

        // Build per-cluster neighbor contexts and mesh each. The
        // neighbor's stored LOD is what mesh uses to drive cross-LOD
        // stride adjustments at the boundary layer.
        let mut new_meshes: Vec<(ClusterId, MeshHandle)> = Vec::new();
        // LOD2 nav surfaces, derived on this same load path (§4.7) and
        // gated to rings 0–2 (§4.6). The camera position at rebuild time
        // is the ring origin; in this 3×3 field every cluster falls
        // inside ring 2 so all of them get nav.
        let mut new_navs: Vec<(ClusterId, ClusterNav)> = Vec::new();
        // Magenta navmesh wireframe segments, accumulated across clusters
        // as each nav is derived; drawn when the HUD's navmesh toggle is on.
        let mut new_navmesh_segments: Vec<(Vec3, Vec3)> = Vec::new();
        let camera = [self.position.x, self.position.y, self.position.z];
        let mut total_walkable_cols = 0_usize;
        // Watertight diagnostic — accumulated across all clusters, logged
        // at the end of rebuild. `total_unshared` mixes real gaps with
        // legitimate world-boundary edges; `total_over_shared` is always
        // a bug.
        let mut total_edges = 0_usize;
        let mut total_unshared = 0_usize;
        let mut total_over_shared = 0_usize;
        let mut sample_gaps: Vec<(ClusterId, [f32; 3], [f32; 3])> = Vec::new();
        for id in &ids {
            let x = id.x();
            let z = id.z();
            let nb = |xx: u16, zz: u16| -> Option<(&Cluster, Lod)> {
                let lod = lod_for(xx, zz);
                let cid = ClusterId::new(lod, xx, 0, zz);
                self.map
                    .get(cid)
                    .map(|c| (c, Lod::new(lod).expect("valid lod")))
            };
            let neg_x = if x > 0 { nb(x - 1, z) } else { None };
            let pos_x = if x + 1 < FIELD_DIM {
                nb(x + 1, z)
            } else {
                None
            };
            let neg_z = if z > 0 { nb(x, z - 1) } else { None };
            let pos_z = if z + 1 < FIELD_DIM {
                nb(x, z + 1)
            } else {
                None
            };
            let neighbors = NeighborContext {
                neg_x,
                pos_x,
                neg_z,
                pos_z,
                ..NeighborContext::none()
            };

            let cluster = self.map.get(*id).expect("just inserted");
            let self_lod = Lod::new(id.lod()).expect("valid lod");
            let cm = flicker_voxel::mesh(cluster, &neighbors, self_lod);

            // Derive the LOD2 walkable surface from the state field
            // (never the mesh) for clusters inside rings 0–2. compute_nav
            // is pure and runs after meshing only to sit on the existing
            // load path; it does not read `cm`. Nothing consumes the
            // result yet — it is stashed for the walking-camera follow-up.
            if in_nav_rings(camera, cluster_center_world(*id)) {
                let nav = ClusterNav::compute_nav(cluster, &neighbors);
                total_walkable_cols += (0..NAV_DIM as u8)
                    .flat_map(|x| (0..NAV_DIM as u8).map(move |z| (x, z)))
                    .filter(|&(x, z)| nav.floor_at(x, z).is_some())
                    .count();
                append_navmesh_segments(&nav, *id, cluster, &neighbors, &mut new_navmesh_segments);
                new_navs.push((*id, nav));
            }

            // Run the watertight check before upload (we need the
            // CPU-side ClusterMesh and its position data).
            let hist = cm.edge_use_histogram();
            total_edges += hist.len();
            let cluster_off = id.world_offset();
            for (&(va, vb), &uses) in &hist {
                match uses {
                    0 => {}
                    1 => {
                        total_unshared += 1;
                        if sample_gaps.len() < 8 {
                            let pa = cm.vertices[va as usize].position;
                            let pb = cm.vertices[vb as usize].position;
                            let wa = [
                                pa[0] + cluster_off[0],
                                pa[1] + cluster_off[1],
                                pa[2] + cluster_off[2],
                            ];
                            let wb = [
                                pb[0] + cluster_off[0],
                                pb[1] + cluster_off[1],
                                pb[2] + cluster_off[2],
                            ];
                            sample_gaps.push((*id, wa, wb));
                        }
                    }
                    2 => {}
                    _ => total_over_shared += 1,
                }
            }

            let verts: Vec<MeshVertex> = cm
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.position,
                    normal: v.normal,
                    material: v.material,
                })
                .collect();
            let handle = renderer.upload_mesh(&verts, MeshIndices::U32(&cm.indices));
            new_meshes.push((*id, handle));

            // Snapshot world-space triangles for CPU ray-casting. The
            // GPU buffer above is opaque to us; the picker needs raw
            // positions in world coords (the same frame the camera ray
            // lives in), so we apply the cluster's world offset here
            // and stash positions + indices.
            let pick_verts: Vec<Vec3> = cm
                .vertices
                .iter()
                .map(|v| {
                    Vec3::new(
                        v.position[0] + cluster_off[0],
                        v.position[1] + cluster_off[1],
                        v.position[2] + cluster_off[2],
                    )
                })
                .collect();
            self.pick_meshes.push((*id, pick_verts, cm.indices.clone()));
        }
        self.meshes = new_meshes;
        self.navs = new_navs;
        self.navmesh_segments = new_navmesh_segments;

        tracing::info!(
            "rebuild: {} clusters, {} edges total, {} unshared (gaps + world-boundary), {} over-shared",
            ids.len(),
            total_edges,
            total_unshared,
            total_over_shared,
        );
        tracing::info!(
            "nav: derived LOD2 surfaces for {} of {} clusters (rings 0–2), {} walkable columns total",
            self.navs.len(),
            ids.len(),
            total_walkable_cols,
        );
        for (id, a, b) in &sample_gaps {
            tracing::info!(
                "  unshared edge in cluster ({}, {}, {}): ({:.2}, {:.2}, {:.2}) → ({:.2}, {:.2}, {:.2})",
                id.x(), id.y(), id.z(),
                a[0], a[1], a[2],
                b[0], b[1], b[2],
            );
        }

        // Corner-vector arrows: across the whole field, every stored
        // voxel with a non-default corner contributes one segment.
        let mut arrows: Vec<(Vec3, Vec3)> = Vec::new();
        for id in &ids {
            let off = id.world_offset();
            let origin_world = Vec3::new(off[0], off[1], off[2]);
            let cluster = self.map.get(*id).expect("just inserted");
            for (coord, voxel) in cluster.overrides() {
                if voxel.corner() == CornerVector::DEFAULT {
                    continue;
                }
                let base =
                    origin_world + Vec3::new(coord.x() as f32, coord.y() as f32, coord.z() as f32);
                let [dx, dy, dz] = voxel.corner().to_components();
                let tip = base + Vec3::new(dx, dy, dz);
                arrows.push((base, tip));
            }
        }
        self.corner_arrows = arrows;
    }
}

/// Voxel-space lift applied to navmesh wireframe segments so the magenta
/// grid hovers just clear of the grey surface mesh instead of z-fighting
/// it. 1 voxel = 6 inches; tune up if it reads as buried.
const NAVMESH_VIZ_LIFT: f32 = 1.0;

/// World-space position of nav column `(x, z)` at floor index `floor` in
/// the cluster whose local origin is `origin`: the LOD2 sample position
/// horizontally (`origin + sample * stride`), the floor height
/// vertically (`origin_y + floor * stride`), lifted clear of the mesh.
///
/// `x`/`z` may be `NAV_DIM` (64) — the sample one step past a `+X`/`+Z`
/// boundary, which lands exactly on the neighbour's near edge in *this*
/// cluster's own frame. That is how a seam segment's far endpoint is
/// placed without ever consulting the neighbour's origin.
fn nav_column_point(origin: Vec3, x: u8, z: u8, floor: u8) -> Vec3 {
    // LOD2 sample stride in voxels: CLUSTER_DIM / NAV_DIM = 256 / 64 = 4.
    let stride = CLUSTER_DIM as f32 / NAV_DIM as f32;
    origin
        + Vec3::new(
            x as f32 * stride,
            floor as f32 * stride + NAVMESH_VIZ_LIFT,
            z as f32 * stride,
        )
}

/// Append this cluster's complete navmesh wireframe to `out`: one
/// segment per pair of walkable-linked 4-adjacent columns, interior
/// **and** across cluster boundaries. Only each column's `+X` and `+Z`
/// neighbour is visited, so every link is emitted exactly once — the
/// same low-side-owns convention the mesher uses for seam quads.
///
/// Boundary links are read through `neighbors`, this cluster's own
/// references to its neighbours' state fields, via
/// [`ClusterNav::linked_across`] / [`ClusterNav::floor_across`] — exactly
/// how the mesher reads neighbour voxels while meshing. The far endpoint
/// is placed at the sample one past the boundary (`NAV_DIM`) in *this*
/// cluster's frame, which lands on the neighbour's near edge. So the seam
/// is generated correctly in this single pass, never stitched together
/// from independently-built per-cluster navs.
fn append_navmesh_segments(
    nav: &ClusterNav,
    id: ClusterId,
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    out: &mut Vec<(Vec3, Vec3)>,
) {
    let off = id.world_offset();
    let origin = Vec3::new(off[0], off[1], off[2]);
    let dim = NAV_DIM as u8; // 64
    let last = dim - 1; // 63
    for x in 0..dim {
        for z in 0..dim {
            let Some(f) = nav.floor_at(x, z) else {
                continue;
            };
            let here = nav_column_point(origin, x, z, f);

            // Interior +X / +Z links. `linked` is true only when both
            // columns have a floor, so the neighbour lookup is infallible.
            if x + 1 < dim && nav.linked((x, z), (x + 1, z)) {
                let fx = nav.floor_at(x + 1, z).expect("linked implies a floor");
                out.push((here, nav_column_point(origin, x + 1, z, fx)));
            }
            if z + 1 < dim && nav.linked((x, z), (x, z + 1)) {
                let fz = nav.floor_at(x, z + 1).expect("linked implies a floor");
                out.push((here, nav_column_point(origin, x, z + 1, fz)));
            }

            // Seam +X / +Z links at the cluster's far edge: read the
            // neighbour column through our references and place its
            // endpoint at the virtual sample `dim` in our own frame.
            if x == last && nav.linked_across(cluster, neighbors, (x, z), FaceDir::PosX) {
                let nf = nav
                    .floor_across(cluster, neighbors, (x, z), FaceDir::PosX)
                    .expect("linked_across implies a neighbour floor");
                out.push((here, nav_column_point(origin, dim, z, nf)));
            }
            if z == last && nav.linked_across(cluster, neighbors, (x, z), FaceDir::PosZ) {
                let nf = nav
                    .floor_across(cluster, neighbors, (x, z), FaceDir::PosZ)
                    .expect("linked_across implies a neighbour floor");
                out.push((here, nav_column_point(origin, x, dim, nf)));
            }
        }
    }
}

// ===== Camera-driven LOD policy =====

/// Base distance (voxel units) for the per-cluster LOD policy: a cluster
/// `2^n` of these from the camera gets LOD `n` (clamped 0–7). Tuned small
/// (~half a cluster edge) so the 9-cluster field shows visible swaps as the
/// camera moves — the field is too small to exercise a wide LOD range
/// otherwise. A bigger field (more clusters) is the follow-up that exercises
/// the policy properly.
const LOD_BASE_DISTANCE: f32 = 128.0;

/// Per-cluster target LOD from the camera's world-space distance to the
/// cluster centre. Lower = finer. `clamp(floor(log2(dist / base)), 0, 7)`,
/// reusing `flicker_voxel::cluster_center_world` for the centre.
fn target_lod_for_cluster(camera: Vec3, id: ClusterId) -> u8 {
    let c = cluster_center_world(id);
    let distance = (camera - Vec3::new(c[0], c[1], c[2])).length().max(1.0);
    let raw = (distance / LOD_BASE_DISTANCE).log2().floor() as i32;
    raw.clamp(0, Lod::MAX.level() as i32) as u8
}

/// Relax a 3×3 per-cluster LOD field so no 4-adjacent pair differs by more
/// than one level — the mesher's locked cross-LOD adjacency invariant
/// (`flicker_voxel::mesh` panics otherwise). Iterates to a fixed point,
/// raising (coarsening) the finer side of any over-steep pair. Values only
/// increase and are bounded by the field's max, so it always converges.
fn smooth_lod_field(field: &mut [[u8; FIELD_DIM as usize]; FIELD_DIM as usize]) {
    let dim = FIELD_DIM as usize;
    loop {
        let mut changed = false;
        for x in 0..dim {
            for z in 0..dim {
                for (nx, nz) in [(x + 1, z), (x, z + 1)] {
                    if nx >= dim || nz >= dim {
                        continue;
                    }
                    let here = field[x][z];
                    let there = field[nx][nz];
                    if here.abs_diff(there) > 1 {
                        // Raise the finer (lower-LOD) side to one below the
                        // coarser side, closing the gap to exactly 1.
                        if here < there {
                            field[x][z] = there - 1;
                        } else {
                            field[nx][nz] = here - 1;
                        }
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

// ===== LOD-digit billboard atlas (world-space, on the navmesh surface) =====

/// World-space edge length (voxels) of the LOD-digit billboards.
const BILLBOARD_SIZE: f32 = 32.0;

// Tiny 5×7 bitmap font for digits 0–7, baked into a texture atlas.
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const DIGITS: [[u8; GLYPH_H]; 8] = [
    // 0
    [
        0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
    ],
    // 1
    [
        0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
    ],
    // 2
    [
        0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
    ],
    // 3
    [
        0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
    ],
    // 4
    [
        0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
    ],
    // 5
    [
        0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
    ],
    // 6
    [
        0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
    ],
    // 7
    [
        0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
    ],
];

const CELL_W: usize = 32;
const CELL_H: usize = 40;
const ATLAS_W: usize = CELL_W * 8;
const ATLAS_H: usize = CELL_H;

/// Build the RGBA8 digit atlas: 8 cells (digits 0–7) side by side, each a
/// scaled-up white glyph on transparent black.
fn build_digit_atlas() -> Vec<u8> {
    const SCALE: usize = 4;
    let glyph_px_w = GLYPH_W * SCALE;
    let glyph_px_h = GLYPH_H * SCALE;
    let margin_x = (CELL_W - glyph_px_w) / 2;
    let margin_y = (CELL_H - glyph_px_h) / 2;

    let mut pixels = vec![0u8; ATLAS_W * ATLAS_H * 4];
    for (digit_idx, rows) in DIGITS.iter().enumerate() {
        let cell_x0 = digit_idx * CELL_W + margin_x;
        let cell_y0 = margin_y;
        for (row_idx, &row_bits) in rows.iter().enumerate() {
            for col in 0..GLYPH_W {
                let bit = (row_bits >> (GLYPH_W - 1 - col)) & 1;
                if bit == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let px = cell_x0 + col * SCALE + dx;
                        let py = cell_y0 + row_idx * SCALE + dy;
                        let i = (py * ATLAS_W + px) * 4;
                        pixels[i] = 0xff;
                        pixels[i + 1] = 0xff;
                        pixels[i + 2] = 0xff;
                        pixels[i + 3] = 0xff;
                    }
                }
            }
        }
    }
    pixels
}

/// UV sub-rect of the atlas for digit `d` (0–7).
fn digit_uv_rect(d: u8) -> (Vec2, Vec2) {
    let cell_w_uv = 1.0 / 8.0;
    let u0 = d as f32 * cell_w_uv;
    let u1 = u0 + cell_w_uv;
    (Vec2::new(u0, 0.0), Vec2::new(u1, 1.0))
}

impl App for VoxelCluster {
    fn init(&mut self, renderer: &mut Renderer) {
        self.rebuild(renderer);

        // Frame the whole field from outside its -Z face, angled down.
        let field_extent = FIELD_DIM as f32 * CLUSTER_DIM as f32;
        let center_x = field_extent * 0.5;
        self.position = Vec3::new(center_x, field_extent * 1.1, -field_extent * 0.5);
        self.yaw = 0.0; // face +Z, into the field.
        self.pitch = -0.55; // look down at it.

        // 1×1 white pixel — tinted to build solid colored HUD quads.
        // Retained sprite-UI capability; no active widgets yet.
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));

        // Digit-glyph atlas for the LOD billboards (digits 0–7).
        self.digit_atlas =
            Some(renderer.load_texture(&build_digit_atlas(), ATLAS_W as u32, ATLAS_H as u32));
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) {
        if input.action_active(&self.bindings, Action::Quit) {
            self.should_quit = true;
            return;
        }
        let dt_s = dt.as_secs_f32();

        // Debug toggles now live in the HUD script: feed it the mouse +
        // click edge, and mirror the checkbox states it reports back.
        // (`as_ref().map(..)` releases the `self.script` borrow before we
        // write the other fields below.)
        if let Some(result) = self.script.as_ref().map(|s| s.update(input)) {
            match result {
                Ok(toggles) => {
                    self.wireframe_on = toggles.is_on("wireframe");
                    self.corner_arrows_on = toggles.is_on("corner_arrows");
                    self.navmesh_on = toggles.is_on("navmesh");
                    self.camera_lod_on = toggles.is_on("camera_lod");
                    self.lod_billboards_on = toggles.is_on("lod_billboards");

                    // Track the manual centre-LOD toggle (used when
                    // camera-driven LOD is off).
                    let center_on = toggles.is_on("center_lod");
                    if center_on != self.prev_center_lod_on {
                        self.center_lod_level = if center_on { 1 } else { 0 };
                    }
                    self.prev_center_lod_on = center_on;

                    // Decide the desired per-cluster LOD field: camera-driven
                    // (smoothed to the mesher's ±1 adjacency invariant) when
                    // enabled, else the manual centre-LOD field. A change
                    // triggers a synchronous re-mesh in `render`; the swap
                    // hitch is the cost of synchronous re-contour — the async
                    // worker queue that removes it is the next slice.
                    let mut desired = [[0u8; FIELD_DIM as usize]; FIELD_DIM as usize];
                    if self.camera_lod_on {
                        for x in 0..FIELD_DIM {
                            for z in 0..FIELD_DIM {
                                desired[x as usize][z as usize] = target_lod_for_cluster(
                                    self.position,
                                    ClusterId::new(0, x, 0, z),
                                );
                            }
                        }
                        smooth_lod_field(&mut desired);
                    } else {
                        desired[1][1] = self.center_lod_level;
                    }
                    if desired != self.lod_field {
                        self.lod_field = desired;
                        self.needs_rebuild = true;
                    }
                }
                Err(e) => tracing::error!("HUD script update failed: {e}"),
            }
        }

        // Left-click → world pick (inspector). The HUD script consumes
        // the press edge for its own checkbox toggling above, but it
        // does so without telling us — so we re-check the press edge
        // here and gate it on "cursor outside the HUD panel" to avoid
        // double-firing on a checkbox click. Right-drag is for look;
        // left-click is for pick. No conflict.
        if input.mouse_left_pressed && !Self::cursor_on_hud(input.mouse_position) {
            if let Some((id, hit_world)) = self.try_pick(input.mouse_position, renderer.size()) {
                let p = Self::hit_to_local_p(id, hit_world);
                tracing::info!(
                    "pick: cluster ({}, {}, {}, lod {}) → world ({:.2}, {:.2}, {:.2}) → p ({}, {}, {})",
                    id.x(), id.y(), id.z(), id.lod(),
                    hit_world.x, hit_world.y, hit_world.z,
                    p[0], p[1], p[2],
                );
                self.selection = Some((id, p));
            }
        }

        // Look: right-drag, with invert/sensitivity applied by config.
        if input.mouse_right {
            if let Some(prev) = self.last_look_cursor {
                let (dyaw, dpitch) = self.config.look_delta(input.mouse_position - prev);
                self.yaw -= dyaw;
                self.pitch = (self.pitch + dpitch).clamp(-1.5, 1.5);
            }
            self.last_look_cursor = Some(input.mouse_position);
        } else {
            self.last_look_cursor = None;
        }

        // Movement: query actions, not keys.
        let mut motion = Vec3::ZERO;
        if input.action_active(&self.bindings, Action::MoveForward) {
            motion += self.move_forward();
        }
        if input.action_active(&self.bindings, Action::MoveBackward) {
            motion -= self.move_forward();
        }
        if input.action_active(&self.bindings, Action::StrafeRight) {
            motion += self.move_right();
        }
        if input.action_active(&self.bindings, Action::StrafeLeft) {
            motion -= self.move_right();
        }
        if input.action_active(&self.bindings, Action::MoveUp) {
            motion += Vec3::Y;
        }
        if input.action_active(&self.bindings, Action::MoveDown) {
            motion -= Vec3::Y;
        }
        if motion.length_squared() > 0.0 {
            self.position += motion.normalize() * self.config.move_speed * dt_s;
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if self.needs_rebuild {
            self.needs_rebuild = false;
            self.rebuild(renderer);
        }
        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 10000.0,
        });

        // Draw each cluster's extent as a white wireframe box.
        for (id, _cluster) in self.map.iter() {
            let offset = id.world_offset();
            let min = Vec3::new(offset[0], offset[1], offset[2]);
            let max = min + Vec3::splat(CLUSTER_DIM as f32);
            renderer.draw_bounding_box(min, max, [1.0, 1.0, 1.0, 1.0]);
        }

        // Draw each cluster's mesh at its world offset.
        for (id, handle) in &self.meshes {
            let o = id.world_offset();
            let model = Mat4::from_translation(Vec3::new(o[0], o[1], o[2]));
            renderer.draw_mesh(*handle, model, MeshDrawOptions::default());
            if self.wireframe_on {
                renderer.draw_mesh(
                    *handle,
                    model,
                    MeshDrawOptions {
                        wireframe: true,
                        ..Default::default()
                    },
                );
            }
        }

        // Corner-vector arrows: every stored voxel with a non-default
        // corner contributes one segment from its grid coord to the
        // decoded tip. Orange so it reads against the grey mesh.
        if self.corner_arrows_on && !self.corner_arrows.is_empty() {
            renderer.draw_lines(&self.corner_arrows, [1.0, 0.6, 0.15, 1.0]);
        }

        // Navmesh wireframe: the LOD2 walkable surface as floor-to-floor
        // links between walkable-adjacent columns. Magenta so it reads
        // against both the grey mesh and the orange corner arrows.
        if self.navmesh_on && !self.navmesh_segments.is_empty() {
            renderer.draw_lines(&self.navmesh_segments, [1.0, 0.0, 1.0, 1.0]);
        }

        // LOD billboards: a digit per cluster, sitting on the navmesh surface
        // at the cluster centre, showing that cluster's current LOD. World-
        // space and depth-tested, so terrain in front occludes them.
        if self.lod_billboards_on {
            if let Some(atlas) = self.digit_atlas {
                let half_col = (NAV_DIM / 2) as u8; // centre column of the 64² nav grid
                let stride = CLUSTER_DIM as f32 / NAV_DIM as f32; // 4 voxels / LOD2 cell
                let half_edge = CLUSTER_DIM as f32 * 0.5; // cluster-centre offset
                for (id, nav) in &self.navs {
                    let off = id.world_offset();
                    // Surface height from the navmesh centre column; fall back
                    // to the cluster's volume centre if that column has no floor.
                    let surface_y = match nav.floor_at(half_col, half_col) {
                        Some(f) => off[1] + f as f32 * stride,
                        None => off[1] + half_edge,
                    };
                    // Lift the centre by half the quad height so the
                    // billboard's bottom edge rests on the surface rather than
                    // its midline (which buries the lower half in the mesh).
                    let pos = Vec3::new(
                        off[0] + half_edge,
                        surface_y + BILLBOARD_SIZE * 0.5,
                        off[2] + half_edge,
                    );
                    let lod = self.lod_field[id.x() as usize][id.z() as usize];
                    let (uv_min, uv_max) = digit_uv_rect(lod);
                    renderer.draw_billboard(
                        atlas,
                        pos,
                        Vec2::splat(BILLBOARD_SIZE),
                        uv_min,
                        uv_max,
                        [1.0, 0.95, 0.4, 1.0],
                    );
                }
            }
        }

        // HUD text.
        renderer.draw_text(
            &format!(
                "voxel cluster — {}×{} field — WASD move, R/F up/down, right-drag look",
                FIELD_DIM, FIELD_DIM
            ),
            Vec2::new(16.0, 16.0),
            22.0,
            [1.0, 1.0, 1.0, 1.0],
        );
        renderer.draw_text(
            &format!(
                "pos: ({:.0}, {:.0}, {:.0})  yaw: {:.2}  pitch: {:.2}",
                self.position.x, self.position.y, self.position.z, self.yaw, self.pitch
            ),
            Vec2::new(16.0, 44.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "clusters: {}   extent: {}³ voxels each",
                self.map.len(),
                CLUSTER_DIM
            ),
            Vec2::new(16.0, 64.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "config — speed: {:.0}  sens: {:.4}  invert-Y: {}  invert-X: {}",
                self.config.move_speed,
                self.config.look_sensitivity,
                self.config.invert_pitch,
                self.config.invert_yaw,
            ),
            Vec2::new(16.0, 84.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        // Diagnostics the checkboxes don't convey on their own.
        renderer.draw_text(
            &format!(
                "corner arrows stored: {}   centre LOD: {}  (other clusters: LOD 0)   nav clusters (rings 0–2): {}",
                self.corner_arrows.len(),
                self.center_lod_level,
                self.navs.len(),
            ),
            Vec2::new(16.0, 104.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            "press Escape to quit",
            Vec2::new(16.0, 124.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        // Current pick — the inspector's selection state. `None` until
        // the first left-click lands on a meshed face.
        let pick_line = match self.selection {
            Some((id, p)) => format!(
                "pick: ({}, {}, {}, lod {}) p = ({}, {}, {})",
                id.x(),
                id.y(),
                id.z(),
                id.lod(),
                p[0],
                p[1],
                p[2]
            ),
            None => "pick: (none — left-click a face)".to_string(),
        };
        renderer.draw_text(
            &pick_line,
            Vec2::new(16.0, 144.0),
            16.0,
            [0.95, 0.85, 0.60, 1.0],
        );

        // Virtual-voxel inspector: 12-edge wireframe of the dual cell
        // at the selected lattice point, plus a per-corner readout of
        // both translation frames (owner-relative `V` as the owner
        // voxel stores it, and self-relative — same world point
        // expressed from `p`). Inspect-only; both frames are kept so
        // the bookkeeping is visible while debugging.
        if let Some(vv) = self.current_virtual_voxel() {
            let mut segments: Vec<(Vec3, Vec3)> = Vec::with_capacity(12);
            for &(o0, o1) in &CUBE_EDGES {
                segments.push((vv.corners[o0].world, vv.corners[o1].world));
            }
            // Darker than the bright-white cluster bounding box so the
            // dual cell reads as a distinct overlay rather than a
            // sub-box of the cluster.
            renderer.draw_lines(&segments, [0.7, 0.7, 0.75, 1.0]);

            // Per-corner readout. Anchored to the right side of the
            // screen so it doesn't fight the left-column diagnostics
            // or the scripted HUD's checkbox panel. The leading
            // (-/+ -/+ -/+) tag is the octant bit pattern decoded
            // back into per-axis sign, matching the brief's table.
            let surface_w = renderer.size().x;
            let panel_w = 620.0;
            let panel_x = (surface_w - panel_w - 16.0).max(16.0);
            renderer.draw_text(
                &format!(
                    "virt voxel  cluster ({}, {}, {}, lod {})  p = ({}, {}, {})",
                    vv.cluster.x(),
                    vv.cluster.y(),
                    vv.cluster.z(),
                    vv.cluster.lod(),
                    vv.center_local[0],
                    vv.center_local[1],
                    vv.center_local[2]
                ),
                Vec2::new(panel_x, 16.0),
                16.0,
                [0.95, 0.85, 0.60, 1.0],
            );
            for o in 0..8 {
                let c = &vv.corners[o];
                let bx = (o & 1) as i32;
                let by = ((o >> 1) & 1) as i32;
                let bz = ((o >> 2) & 1) as i32;
                let tag = |b: i32| if b == 1 { '+' } else { '-' };
                let line = format!(
                    "o={} ({}{}{})  m=({:>3},{:>3},{:>3})  V=({:+.3},{:+.3},{:+.3})  self=({:+.3},{:+.3},{:+.3})",
                    o,
                    tag(bx), tag(by), tag(bz),
                    c.owner_local[0], c.owner_local[1], c.owner_local[2],
                    c.owner_relative[0], c.owner_relative[1], c.owner_relative[2],
                    c.self_relative[0], c.self_relative[1], c.self_relative[2],
                );
                renderer.draw_text(
                    &line,
                    Vec2::new(panel_x, 40.0 + (o as f32) * 18.0),
                    13.0,
                    [0.82, 0.86, 0.92, 1.0],
                );
            }
        }

        // The scripted HUD: the Lua side returns plain draw commands,
        // which we turn into sprite (rect) and text calls. Rects are
        // drawn with the 1×1 white texture tinted by the command color.
        if let (Some(script), Some(white)) = (self.script.as_ref(), self.white) {
            match script.draw() {
                Ok(commands) => {
                    for command in commands {
                        match command {
                            HudCommand::Rect { x, y, w, h, color } => {
                                renderer.draw_sprite(
                                    white,
                                    Vec2::new(x, y),
                                    Vec2::new(w, h),
                                    color,
                                );
                            }
                            HudCommand::Text {
                                x,
                                y,
                                text,
                                size,
                                color,
                            } => {
                                renderer.draw_text(&text, Vec2::new(x, y), size, color);
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("HUD script draw failed: {e}"),
            }
        }
    }
}

/// Directory the example reads bake files from on startup and writes
/// bake files to in `--bake` mode. Resolved against this crate's
/// source dir so `cargo run` finds it from any working directory.
fn bake_dir_path() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/bake"))
}

/// Filename for a freshly-written cluster bake. The on-disk file
/// stores LOD-0 data only (LOD is a render concern) and is gzip-
/// compressed compact JSON, so the canonical extension is
/// `.json.gz`. The cluster's spatial address is the rest of the name
/// — `lod` is omitted because it's always `0` here.
fn bake_filename(x: u16, y: u16, z: u16) -> String {
    format!("cluster_{x}_{y}_{z}.json.gz")
}

/// Legacy filename for uncompressed cluster bakes — what `--bake`
/// wrote before the gzip wiring landed. Used as a fallback by the
/// loader so existing hand-decompressed or pre-gzip bakes still
/// load without a forced re-bake.
fn bake_filename_legacy(x: u16, y: u16, z: u16) -> String {
    format!("cluster_{x}_{y}_{z}.json")
}

/// Try to load every cluster in `ids` from `dir`. Returns `Some(vec)`
/// only if **all** loads succeed; partial loads fall back to
/// contour-from-primitive (no point starting up with a half-baked
/// field). Reads are best-effort: any error path logs at warn level
/// and yields `None`. The compressed name (`.json.gz`) is tried
/// first; if it isn't present, the legacy uncompressed name
/// (`.json`) is tried as a fallback.
fn try_load_bake_field(
    dir: &std::path::Path,
    ids: &[ClusterId],
) -> Option<Vec<(ClusterId, Cluster)>> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let compressed_path = dir.join(bake_filename(id.x(), id.y(), id.z()));
        let legacy_path = dir.join(bake_filename_legacy(id.x(), id.y(), id.z()));
        let (path, bytes) = if let Ok(bytes) = std::fs::read(&compressed_path) {
            (compressed_path, bytes)
        } else if let Ok(bytes) = std::fs::read(&legacy_path) {
            (legacy_path, bytes)
        } else {
            return None; // neither file present → fall back to contour
        };
        let baked = match BakedCluster::from_bytes(&bytes) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("bake load failed for {}: {e}", path.display());
                return None;
            }
        };
        // The bake's on-disk id should match what we asked for. If it
        // doesn't, the file was hand-edited or copied incorrectly —
        // refuse to trust it.
        if baked.id.bits() != id.bits() {
            tracing::warn!(
                "bake at {} carries id {:?}, expected {:?}; skipping bake load",
                path.display(),
                baked.id,
                id,
            );
            return None;
        }
        out.push((baked.id, baked.cluster));
    }
    Some(out)
}

/// Bake every cluster in the 3×3 field to disk at LOD 0, then exit.
/// Triggered by `--bake` on the command line; the demo's renderer
/// never spins up in this mode. Files are written
/// `bake/cluster_{x}_{y}_{z}.json.gz` (compact JSON, gzip-
/// compressed). To inspect a file, `gunzip -c cluster_*.json.gz |
/// jq .` — round-trips through `BakedCluster::from_bytes` either
/// way.
fn run_bake_mode() -> Result<()> {
    let dir = bake_dir_path();
    std::fs::create_dir_all(&dir)?;
    let material = Material::new(1, 1, 0).expect("grey material is in-range");
    let mut written = 0_usize;
    let mut total_bytes = 0_u64;
    for x in 0..FIELD_DIM {
        for z in 0..FIELD_DIM {
            let id = ClusterId::new(0, x, 0, z);
            let scene = Scene::world_at(id.world_offset());
            let cluster = contour(&scene, material, id);
            let baked = BakedCluster::from_cluster(id, cluster);
            // Compact JSON, gzipped — the dense state field's 4 MB of
            // packed bytes and the long runs of identical material
            // bytes both compress 5–10× under default gzip. A
            // typical 3×3 demo cluster lands around 10 MB on disk.
            let bytes = baked
                .to_disk_bytes()
                .map_err(|e| anyhow::anyhow!("serialize cluster ({x}, 0, {z}): {e}"))?;
            let path = dir.join(bake_filename(id.x(), id.y(), id.z()));
            std::fs::write(&path, &bytes)?;
            tracing::info!("wrote {} ({} bytes)", path.display(), bytes.len());
            written += 1;
            total_bytes += bytes.len() as u64;
        }
    }
    tracing::info!(
        "baked {written} clusters ({} MB total) to {}",
        total_bytes / (1024 * 1024),
        dir.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "voxel_cluster=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    // Hand-parse argv — at most one flag, no need for a CLI crate.
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--bake" => return run_bake_mode(),
            "--help" | "-h" => {
                println!("voxel-cluster — flicker voxel demo");
                println!("Usage:");
                println!("  voxel-cluster           run the demo (loads bake/ if present, else contours)");
                println!("  voxel-cluster --bake    contour the 3×3 field and write bake/cluster_*.json, then exit");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    // Load the HUD script at startup. The path is resolved against this
    // crate's source dir, so the example finds it regardless of the
    // working directory `cargo run` is launched from; editing the .lua
    // file then takes effect on the next run, no recompile needed.
    let script_path = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/hud.lua");
    // `ScriptError` carries an `mlua::Error`, which is not `Send`, so it
    // can't auto-convert into `anyhow::Error`; format it to a message.
    let script = ScriptHost::from_file(script_path)
        .map_err(|e| anyhow::anyhow!("failed to load HUD script: {e}"))?;
    tracing::info!("loaded HUD script from {script_path}");

    run(VoxelCluster::new(script))?;
    Ok(())
}
