# flicker-mechanics

The **geometry substrate of the mechanics cluster** — the workspace cluster that owns
game-mechanics runtime (collision now, combat resolution later), as distinct from a
low-level physics engine. Today it is pure geometry: bone-bound collision **shapes** and
overlap **queries**, a kinematic **drop-to-ground** settle, and the renderer-agnostic
**gizmo + rig-overlay** line geometry the import editor draws. It is `glam`-only except for
one module (`bridge`) that reads the `flicker.rig` `collision` section. Everything it
returns is plain math (shapes, contacts, `(Vec3, Vec3)` line pairs) that a caller feeds to
its own pipeline.

**What this crate is NOT (yet).** Its cluster is named for combat, and it is the substrate
combat will stand on — but **no combat resolution lives here**: no stats, no damage, no
hit-registry, no ability resolution. The design of record (see *The signal→ability boundary*
below) places that resolution here in later slices as new modules beside these; the modules
that exist today implement only the geometry those steps will consume. A reader looking for
"where a swing becomes damage" will not find it in this crate yet.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:** `glam` (all vector/matrix math; units are centimetres, world is **Z-up**);
  `flicker-skeletal` — only `bridge` touches it, to read the `flicker.rig` `collision`
  section (`format::Collision` / `CollisionVolume` / `CollisionShape` / `CollisionRole`) and
  the skeleton's `Bone` list. `flicker-skeletal` is `glam`-only, so there is no cycle.
- **Used by:**
  - `flicker-assetpipeline` (the import / rig editor) — `autofit_capsules_from`,
    `gizmo_segments` / `pick_handle` / `drag_plane`, the whole `debug::` overlay, and
    `Shape` / `Volume` / `closest_point_ray_segment`.
  - `flicker-loomforge` references this crate in prose only ("still the geometry layer;
    hitbox↔hurtbox resolution is a later slice") — it is not a code consumer.
- **Reads from the content tree:** nothing directly. It consumes an *already-parsed*
  `flicker.rig` `collision` section handed in by the caller (via `flicker-skeletal`); it
  never opens a file. If the `collision` section is absent, `format::Collision` defaults to
  empty and `volumes_from_format` returns no volumes.

## The signal→ability boundary — what resolves here vs what stays intent

This is the seam most likely to mislead a reader, so state it plainly:

- A **signal** (a.k.a. *intent*) is the semantic WHAT the player asked for — `Confirm`,
  `Defend`, `Kick`. Signals are owned by `flicker-input-core` (`ActionSignal`); see
  `Alpha/crates/input/flicker-input-core/README.md`. **This crate never sees a signal.** It
  is below the input and scene layers entirely — it captures nothing and subscribes to
  nothing.
- An **ability** is what an intent *resolves to* given the player's loadout — the specific
  parry a rapier's `Defend` becomes, the effect a slotted item applies. Per the contract
  stated in `flicker-input-core`'s `signal.rs` (resolved abilities are "a
  `flicker-mechanics` concern, resolved downstream, server-authoritative"), **ability
  resolution is designed to live in this crate**, downstream of the signal.
- **Today that resolver does not exist.** Nothing in this crate turns an intent into an
  ability. What exists is the geometry the resolved ability's attack windows will test
  against: a `Role::Hitbox` `Volume` (a bone-bound capsule a TAE `HitboxActive` window will
  switch on — TAE = the animation's timeline of tick-windows, owned by `flicker-skeletal`)
  and `overlap` / `penetration` to test it against a target's `Role::Physics` hurtbox
  (the damageable volume).

So: **intents stay upstream in `flicker-input-core`; abilities are meant to resolve here but
have not been built; only the collision geometry that resolution will use is present.**

## Security posture — a contract point, not just design

The client binary is in the hands of an adversary and combat authority is the server's
(the design of record is in MCP). Two consequences bind any caller of this crate:

- **This crate is compiled Rust logic on the trusted side of the untrusted-Lua boundary.**
  It exposes no authored/editable seam — no Lua touches it, no scene string selects its
  behaviour. Keep it that way: mechanics logic that decides an outcome belongs in compiled
  Rust here, never on the end-user-editable Lua orchestration layer.
- **A client-side overlap is a PREDICTION, never an authority.** `overlap` / `penetration`
  are the same deterministic geometry whether run on the client (to predict and to draw
  feedback) or on the server (to resolve). Because they are pure functions of their inputs,
  the two agree — but a `true` computed on the client is advisory. **Never gate a real
  outcome (damage, loot, death) on a client-side result**; the authoritative resolution
  re-runs server-side. This crate gives you geometry, not a verdict.

## Public API

Everything below is reachable from `lib.rs`. The `bridge`, `collision`, `drop`, `gizmo`
items are re-exported at the crate root (`flicker_mechanics::overlap`); the `debug` items
are reached by their module path (`flicker_mechanics::debug::wireframe`) — `debug` is a
`pub mod` but is not re-exported at the root.

### Collision shapes (`collision`)

| Item | What it is for | The one thing to know |
|---|---|---|
| `Shape` | The one collision primitive: `Sphere{center,radius}` · `Capsule{a,b,radius}` · `Obb{center,half_extents,rotation}` | A sphere is a zero-length capsule — one segment-vs-segment routine drives every sphere/capsule pair. Lengths are cm, in whatever frame the shape lives in. |
| `Shape::transformed(m: Mat4)` | Place a bone-local shape into the world by a bone's global pose | Radius/half-extent scale by the matrix's **mean axis length** — exact for the uniform scale a bone pose carries, approximate under non-uniform scale. |
| `Shape::translated(v: Vec3)` | Shift a shape, no rotate/scale | Used by the drop settle; keeps orientation. |
| `Shape::bounding_radius()` | Broad-phase cull radius about the shape's centre | — |
| `Shape::center()` | Centre point (capsule midpoint, box centre) | — |
| `Shape::lowest_z()` | Lowest support point along world −Z (Z-up) | The seam that rests a shape on a ground plane; a box projects its half-extents through its rotation. |
| `Role` | What a volume is FOR: `Physics` (persistent occupancy + the damageable hurtbox) · `Hitbox` (transient, TAE-gated attack box) · `Attach` (mount point) | Metadata only. **The overlap queries do NOT read it** — see Sharp edges. Mirrors `flicker_skeletal::format::CollisionRole`. |
| `Volume { shape, bone, role }`, `Volume::new` | A bone-local `Shape` + the bone index it rides + its `Role` — the runtime collision unit | `bone` is an index into the caller's pose palette. |
| `Volume::world(bone_global: Mat4)` | The volume's shape in world space, given its bone's global matrix | The volume follows the pose because it is expressed in the bone's frame. |

### Collision queries (`collision`)

| Item | What it is for | The one thing to know |
|---|---|---|
| `overlap(a: &Shape, b: &Shape) -> bool` | Yes/no: do two shapes overlap | Convenience for `penetration(a,b).is_some()`. |
| `penetration(a, b) -> Option<Contact>` | Overlap with contact detail, or `None` if disjoint | Covers all pairs (seg-seg; capsule/sphere-vs-box iterative; box-box via separating-axis + minimum-translation). |
| `Contact { depth, normal, point }` | A confirmed overlap | `normal` is unit and points **from `b` toward `a`** — the direction to push `a` to resolve. |
| `closest_point_ray_segment(o, d, a, b) -> (Vec3, Vec3)` | Closest points between a ray (t ≥ 0) and a segment | Returns `(point on ray, point on segment)`; their distance is the ray's miss distance to a bone. Powers gizmo picking and bone picking. |
| `fit_capsule(points) -> Shape` | Fit a capsule down a point cloud's longest AABB axis | For a rigid **prop** (no skeleton to fit per-bone). Squat or empty cloud → a `Sphere`. |
| `upright_capsule(points, radius_frac) -> Shape` | A vertical (world-Z) character-controller pill | Radius comes from the **height**, not the arm span — a swinging limb never changes the collision radius. `radius_frac` ≈ 0.12–0.16 reads as humanoid. |

### The `flicker.rig` → runtime bridge (`bridge`)

The only module that touches the asset format. Bone references resolve by **name**
(share-by-name, the format contract).

| Item | What it is for | The one thing to know |
|---|---|---|
| `shape_from_format(&CollisionShape) -> Shape` | Wire shape → runtime `Shape` | — |
| `role_from_format(CollisionRole) -> Role` | Wire role → runtime `Role` | — |
| `volumes_from_format(&Collision, &[Bone]) -> Vec<Volume>` | Convert an asset's authored `collision` section into runtime volumes, resolving each bone name to an index | A volume whose bone name is **not** in the skeleton is **silently dropped** — no warn, no error. See Finding 1 / Sharp edges. |
| `autofit_capsules(&[Bone]) -> Vec<Volume>` | The default collision set the import editor presents for hand-tuning: one capsule per bone (origin → farthest child), leaf → small sphere | Skips the synthetic `root` at the feet; **tags every volume `Physics`** — the editor re-tags hitboxes. Radius = `clamp(len·0.15, 1..15)` cm. |
| `autofit_capsules_from(&[i32], &[Mat4]) -> Vec<Volume>` | The **same** algorithm from plain topology (`parents`) + rest-pose `globals`, for a caller holding posed globals rather than `format::Bone` | One algorithm, two input adapters; a gate asserts byte-identical output. This is the entry point the editor overlay uses. |

### Drop-to-ground (`drop`)

A kinematic settle (fall straight down, rest on a plane) — **not** a rigid-body solver: no
tumbling, bouncing, or friction.

| Item | What it is for | The one thing to know |
|---|---|---|
| `GRAVITY_CM_S2: f32` | Standard gravity in rig units: `-981.0` cm/s² along −Z | — |
| `FallingItem { shape, vel_z, resting }`, `::new` | A dropped item falling onto a horizontal ground plane; `shape` is in **world** space | — |
| `FallingItem::step(ground_z, dt, gravity)` | Advance one timestep; snap onto the ground and stop when the lowest point would cross it | Idempotent once `resting`. |
| `FallingItem::settle(ground_z, dt, gravity, max_steps) -> u32` | Run the fall to rest at a fixed step, capped so a bad call can't spin forever | Returns steps taken. |
| `settle_offset(&Shape, ground_z) -> f32` | The vertical offset that places a shape exactly resting on the plane | `shape.translated((0,0,offset))` sits on the ground. |

### Transform gizmo (`gizmo`) — editor manipulator, renderer-agnostic

Pure geometry + picking for a Blender/Maya-style TRS manipulator. Operates on a plain
`origin` + `basis` (world axes as `Mat3` columns), so it works on any rig; the caller draws
the returned segments through its own line pipeline.

| Item | What it is for | The one thing to know |
|---|---|---|
| `GizmoMode` | `Translate` (fully implemented) · `Rotate` · `Scale` | Geometry + picking branch on this. |
| `Axis` (`X`/`Y`/`Z`) + `Axis::ALL` / `unit` / `color` / `hover_color` | The three axes + their standard R=X / G=Y / B=Z colours | `hover_color` brightens toward white. |
| `gizmo_segments(origin, basis, mode, size, hover) -> Vec<(Vec3,Vec3,[f32;4])>` | The handle line segments, each carrying its own RGBA | Translate = shaft + V arrowhead per axis; Rotate = a ring per axis; Scale = shaft + end box. |
| `pick_handle(ray_origin, ray_dir, origin, basis, mode, size, max_dist) -> Option<Axis>` | The nearest axis handle a cursor ray hits within `max_dist` | Rays are `(origin, dir)` to match `Camera::pick_ray`. |
| `drag_translate(axis, basis, origin, ray_prev, ray_now) -> Vec3` | Axis-constrained **world** translation delta between two cursor rays | `Vec3::ZERO` when a ray runs near-parallel to the axis — the caller holds the previous value that frame. World delta; the caller converts to parent-local. |
| `drag_plane(normal, origin, ray_prev, ray_now) -> Vec3` | Free **planar** world delta in the plane through `origin` — for dragging in an orthographic view | `Vec3::ZERO` when a ray runs near-parallel to the plane. |

### Debug / overlay geometry (`debug::`) — reached by module path

Turns shapes and posed skeletons into `(Vec3, Vec3)` line pairs. Skeleton functions take
plain topology (`parents[i]` = parent index, `< 0` = root) + posed bone **global** matrices,
so the module stays `glam`-only. `world` maps rig space (Z-up/cm) into engine draw space.

| Item | What it is for | The one thing to know |
|---|---|---|
| `debug::wireframe(&Shape)` | A shape as wireframe: sphere = 3 rings; capsule = rings + longitudinals + dome ribs + axis; box = 12 edges | The collision-volume overlay. |
| `debug::joint_segments(world, parents, globals)` | Parent→child joint lines | Skips the root. |
| `debug::frame_axis_segments(world, parents, globals)` | Per-bone frame +X, scaled toward the child — the rig-orientation diagnostic | An off-limb bone (an arm "hunch") shows its axis crossing the joint line. Root + leaf bones skipped. |
| `debug::bone_diamonds(world, parents, globals, waist_frac)` | Octahedral "bone" glyphs (the DCC skeleton look), 12 segments each | A bone hanging off a ground root draws as a single line, not a diamond. |
| `debug::joint_ball_radii(parents, globals, frac, min_r, max_r)` | Per-joint ball radius scaled to bone length, then clamped | Extremity joints read smaller than hips/spine. |

## Interactions

- **Signals it captures — none.** This crate sits below the input and scene layers; it is
  called by scenes, editors, and (in future) the combat resolver, and never subscribes to a
  signal. See *The signal→ability boundary* above.
- **Results / intents it fires — none.** It returns values; it routes nothing.
- **Model keys — none.** It publishes and binds no runtime Model variables (*Model* = the
  per-frame key→value table the engine hands the Lua layer; this crate does not touch it).
- **What it hands other crates:** runtime `Volume`s (from an asset or auto-fit),
  `Contact`s / booleans (overlap), and coloured or plain `(Vec3, Vec3[, RGBA])` line
  segments for a caller's own line pipeline. No handles, no worker jobs, no async — every
  function is synchronous and pure.

## Gates

`cargo test -p flicker-mechanics` — **37 tests**, one module block each. Named contracts a
change must keep green:

- **`collision`** — `sphere_sphere_overlap_and_gap`, `sphere_vs_capsule_uses_nearest_point_on_segment`,
  `capsule_capsule_crossing_and_parallel`, `sphere_vs_box`, `box_vs_box_sat_axis_and_edge`,
  `capsule_vs_box` (every shape pair resolves correctly); `volume_follows_its_bone_pose`;
  `lowest_z_support_point`; `fit_capsule_to_an_elongated_cloud`,
  `fit_capsule_squat_or_empty_is_a_sphere`, `upright_capsule_is_a_vertical_pill_ignoring_arm_span`
  (the fit heuristics); the three `ray_segment_*` picking cases.
- **`bridge`** — `format_shape_and_role_convert`; `volumes_resolve_bone_names_and_drop_unknowns`
  (this test **asserts the silent drop** — see Finding 1); `autofit_makes_a_capsule_per_bone_and_skips_the_root`;
  `autofit_from_globals_matches_autofit_from_bones` (the two entry points are the same algorithm).
- **`drop`** — `settle_offset_rests_the_shape_on_the_plane`, `falling_item_settles_on_the_ground`,
  `an_item_at_ground_height_snaps_and_rests`, `resting_item_is_idempotent`.
- **`gizmo`** — `translate_segments_count_and_axis_colours`, `hover_brightens_only_the_hovered_axis`,
  `rotate_and_scale_geometry_present`, `pick_hits_the_intended_axis`, `pick_misses_past_the_threshold`,
  `pick_chooses_the_nearest_of_two_candidate_axes`, `drag_delta_is_axis_aligned_and_correct_magnitude`,
  `drag_parallel_ray_is_a_no_op`, `drag_respects_a_rotated_basis`, `drag_plane_moves_in_the_view_plane_and_locks_the_view_axis`.
- **`debug`** — `wireframe_shapes_produce_segments_on_their_surface`, `joint_segments_link_parent_to_child_and_skip_the_root`,
  `frame_axis_lies_on_the_limb_when_aligned_and_crosses_when_off`, `bone_diamonds_draw_root_child_as_a_line_and_others_as_diamonds`,
  `joint_ball_radii_scale_with_bone_length_and_clamp`.

## Sharp edges

- **Units are centimetres; the world is Z-up.** `lowest_z`, `GRAVITY_CM_S2` (−981), and the
  drop settle all assume −Z is down. A shape carries no unit tag — the caller keeps the
  contract.
- **`overlap` / `penetration` are role-agnostic.** They test raw geometry and ignore
  `Role` entirely — a hitbox will report overlap against a hitbox. **Filtering to
  hitbox↔hurtbox (or excluding self) is the caller's job**, because the combat resolver that
  would own that filter is not built yet.
- **`volumes_from_format` fails silently on a bad bone name** (see Finding 1). A typo'd
  `bone` in an authored `collision` volume vanishes with no signal.
- **A client-side hit is not authoritative** — see *Security posture*. Draw feedback from it;
  never settle an outcome on it.
- **`transformed` uses mean-axis scale** for radii/half-extents — exact only for uniform
  scale (which a bone pose carries). A deliberately non-uniform matrix distorts a radius.
- **The drop is straight-down only** — no tumble, bounce, or lateral motion. A resting
  `FallingItem` is idempotent; step it forever and it stays put. True dynamics
  (ragdoll/destruction) are a separate, not-yet-present rigid-body middleware.
- **`fit_capsule` / `upright_capsule` degrade to a sphere** for squat or empty input rather
  than erroring — a caller expecting a capsule must match on the result.
- **Available but not yet wired into a shipped scene** (spec-ward tools, not defects): the
  `drop` module (item drop-to-ground is designed but no scene uses it yet), and the whole
  `Role::Hitbox` / hurtbox path (it awaits the combat resolver). These are catalogued here so
  the next author can find them, not flagged for removal.
