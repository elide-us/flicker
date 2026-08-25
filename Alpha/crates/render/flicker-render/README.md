# flicker-render

The GPU renderer. It owns the wgpu device and window surface and **every draw pipeline**
behind one [`Renderer`] handle, records a frame's draw order and compositing in a per-frame
[`FrameGraph`], and executes the **stage / surface / pass** recipe system — the typed values
([`StageDef`], [`PassKind`], [`Attachments`], [`LightRig`], …) that a scene's JSON compiles
into. If you are drawing anything on screen — a sprite, a lit mesh, an offscreen picture, a
whole world — you go through this crate.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Cluster:** `render`. Sibling of `flicker-2d`.
- **Builds on:** `flicker-core`; `wgpu` (device, pipelines, shaders), `winit` ([`Window`]),
  `glam` (math — [`Mat4`]/[`Vec2`]/[`Vec3`] are re-exported so callers need not pin glam),
  `glyphon` (text), `image`, `bytemuck`.
- **Used by (direct dependents):** `flicker-app` (drives the frame: [`Renderer::tick`] →
  [`Renderer::begin_frame`] → draws → [`Renderer::end_frame`], and owns the one
  [`FrameGraph`] per frame), `flicker-widgets` (compiles scene JSON → [`StageDef`] in
  `flicker-widgets/src/stages.rs`, and its fillers draw stages), `flicker` (the core hub),
  `flicker-2d`, and the scene crates `flicker-loomforge` / `flicker-quartermaster` /
  `flicker-sablework`.
- **Reads from the content tree:** **Nothing.** This crate is serde-free and script-free —
  its inputs are typed Rust values, never files. JSON → [`StageDef`] compilation lives in
  `flicker-widgets/src/stages.rs`; humans author those stages per
  [`Alpha/content/sensorium/STAGES.md`](../../../content/sensorium/STAGES.md), and whole
  scenes per [`Alpha/content/sensorium/README.md`](../../../content/sensorium/README.md).

## Vocabulary (this crate owns these words)

- **Surface** — a set of named, format-typed images (colour(s) + depth) a stage renders into.
  The window's swapchain is just the **root surface**; an offscreen render target is another.
- **Stage** ([`StageDef`]) — the authored unit: a lighting rig + optional camera framing +
  content layers + an optional **pass recipe** + a refresh **rate**, rendering into one surface.
- **Pass** ([`PassDef`] / [`PassKind`]) — one whole-surface step drawn around the content
  (the sky behind, the fog over, the tonemap resolve). A pass declares what it **reads** and
  **writes**; nothing authors a pass *number*.
- **Recipe** — a stage's ordered pass list. Order is **derived** from reads/writes
  ([`StageDef::pass_order`], read-after-write, declaration order breaks ties), never authored.
  A stage with no passes gets the default recipe: one [`PassKind::Scene`].
- **Attachment** ([`Attachment`] / [`AttachmentFormat`]) — one named image a surface owns
  (`color`, `depth`, optional `hdr`), with a format and a resolution scale.
- **Rate** ([`Rate`]) — how often a surface re-renders: `Live` / `Poster` / `Hz(n)` / `Dirty`.
- **Bind** — a recipe field driven by per-frame simulation state: `"<field>_bind": "<key>"`
  names a key the scene publishes into [`StageInputs`]. **A bind REPLACES the authored value.**
- **Model / StageInputs** ([`StageInputs`]) — the per-frame `key → f32` table (plus typed
  `gaps` / `clock` / `dirty` channels) a scene publishes; the ONE channel between a simulation
  and its authored passes. This crate's equivalent of the UI "Model", but typed, not JSON.
- **Rig** ([`LightRig`]) — the frame's light list + ambient + sky/fog palette. **Driver**
  ([`Driver`]) — a per-light intensity modulation (flicker / pulse) evaluated on the stage clock.

Authoring the JSON that compiles into all of the above is
[`STAGES.md`](../../../content/sensorium/STAGES.md); this README documents the **Rust API** a
crate calls or consumes.

---

## Public API

### `Renderer` — the imperative device + draw handle

Created once with `Renderer::new(window: Arc<Window>) -> Result<Renderer>` (**async** — it
requests a wgpu adapter/device). Everything else is `&mut self` on the main thread.

**Per-frame lifecycle** (the runner's order):

| Item | For | The one thing to know |
|---|---|---|
| `tick(dt: Duration)` | Advance the per-surface clock | Call ONCE per frame, **before** `begin_frame` — not inside it (`render_to_texture` re-enters `begin_frame`). `Hz`/`Poster` surfaces measure liveness against this clock. |
| `begin_frame()` | Reset per-frame draw queues + effect flags | Uploaded mesh **storage** persists; only the per-frame draw queue clears. Does NOT advance the clock. |
| `end_frame() -> Result<()>` | Upload, encode, present | Acquires the swapchain, encodes the main pass, presents. Reports any **stray draws** (see Sharp edges). Recovers from a lost/outdated surface by reconfiguring. |

**2D draws** (pixel space, origin top-left; painter's order by `layer`):

| Item | For | The one thing to know |
|---|---|---|
| `draw_triangle(a,b,c,color)` | Solid triangle | Vertices in pixels. |
| `draw_sprite(tex,pos,size,color)` | Textured quad | `color` is a tint multiplied with the texel (`[1.0;4]` = none). |
| `draw_sprite_uv(…,uv)` | Atlas quad | `uv = [u0,v0,u1,v1]`; the batch groups by texture so an atlas costs one bind. [`FULL_TEXTURE`] is the whole image. |
| `draw_sprite_ex(…,uv,rotation,pivot)` | Rotated atlas quad | `rotation` radians; screen-y down ⇒ positive turns clockwise. Others delegate here with `rotation=0`. |
| `draw_ui_panel(pos,size,color,color2,grad,radius,border,border_color,feather)` | SDF rounded-rect panel | One draw: fill/gradient + border + soft feather. `grad`: `0`=solid, `1`=vertical, `2`=horizontal. |
| `draw_text(text,pos,size,color)` | Body-face text | `pos` is the top-left baseline. |
| `draw_text_role(text,pos,size,color,role,italic,bold,tracking,wrap)` | Styled text | `role` selects the face ([`FontRole`]); `tracking<0` = the role default; `wrap: Some(px)` wraps. |
| `measure_text(text,size)` / `measure_text_role(…)` | Layout before drawing | Shapes a throwaway buffer, no upload. Style must match the eventual draw. |
| `register_ui_font(bytes)` | Register a TTF/OTF face | Call once at startup; an unregistered role falls back to a system font. |
| `set_layer(f32)` / `layer() -> f32` | Ambient 2D layer | Higher draws on top; ties break by submission order. The depth buffer is never used for 2D. Reset to `0.0` each `begin_frame`. |
| `set_clip([x,y,w,h])` / `clear_clip()` | Scissor 2D draws | Captured per-draw, so it survives the per-layer sort. Reset each frame. |

**3D draws** (world space; need a camera set this frame):

| Item | For | The one thing to know |
|---|---|---|
| `set_camera(&Camera)` | The 3D view | Typically once per frame before any `draw_mesh`. |
| `upload_mesh(verts, MeshIndices) -> MeshHandle` / `free_mesh(h)` | Flat mesh storage | Persists across frames; index format ([`MeshIndices`]) is picked automatically. |
| `draw_mesh(h, model, MeshDrawOptions)` | Lambertian mesh | `options`: `tint`, `wireframe` (barycentric overlay — draw twice for fill+wires), `gloss` (limb sheen from the first non-`Dir` light). |
| `upload_textured_mesh(verts, idx) -> TexturedMeshHandle` / `free_textured_mesh(h)` | UV-mapped mesh storage | Separate storage/handle from `upload_mesh`; the flat and textured paths never cross. |
| `draw_textured_mesh(h, tex, model, opts)` | Albedo-textured mesh | Matte dielectric, no PBR maps. `opts.wireframe` ignored (fill-only). |
| `draw_textured_mesh_soft(…)` | Soft-alpha textured mesh | Albedo alpha blends (× tint alpha) instead of the hard cutout — clouds, decals. |
| `draw_textured_mesh_pbr(h, tex, PbrMaps, model, opts)` | PBR textured mesh | Samples [`PbrMaps`] (normal/rough/metal/AO); each `None` slot uses the pipeline default. Load maps with `load_texture_linear`. |
| `upload_skinned_mesh(verts, idx) -> SkinnedMeshHandle` / `free_skinned_mesh(h)` | GPU-skinned mesh storage | Uploaded once; the GPU deforms from a per-instance bone palette. |
| `draw_skinned_instanced(h, models, palettes, bone_count)` | N skinned instances, one draw | `palettes.len()` must equal `models.len() * bone_count`. **One skinned mesh per frame** — a second call replaces the queued draw. |
| `draw_bounding_box(min,max,color)` / `draw_lines(segs,color)` | Depth-tested world lines | Immediate-mode; depth-tested but do not write depth. |
| `draw_lines_overlay(segs,color)` | Lines over everything | Depth test disabled — a skeleton/gizmo overlay visible through geometry. |
| `draw_billboard(tex,pos,size,uv_min,uv_max,color)` | Camera-facing quad | Constant world size; depth-tested. |
| `draw_billboard_additive(…)` | Additive glow billboard | Adds (× alpha), writes no depth, so glows stack into a halo. |
| `set_material_palette(&[[f32;4]; MATERIAL_PALETTE_LEN])` | Voxel material colours | The pipeline boots all-magenta (loud-wrong); set once from the catalog. Persists. |
| `load_texture(px,w,h)` / `load_texture_linear(px,w,h) -> TextureHandle` | Upload a texture | `_linear` for non-colour data (normal/rough/metal/AO maps). |
| `update_texture(h, px) -> bool` | Replace a texture's pixels | `false` if the handle is stale. |

**Whole-surface effect setters** — normally raised by a recipe pass through
[`FrameGraph::surface`], not called by hand. Each is per-frame and a no-op without a camera:

| Item | For | The one thing to know |
|---|---|---|
| `set_scene(LightRig)` | The frame's lighting + atmosphere | The ONE door into the scene uniform. A content closure calling this REPLACES the stage's rig (last writer wins). |
| `scene_lighting() -> LightRig` | Read the live rig | A pass whose look follows the scene (unauthored fog colour) reads `fog_color` here. |
| `draw_sky()` | Procedural sky | Reads rig sky slots 0/1 + `sky_zenith`/`sky_horizon`. Drawn first, everything layers on top. |
| `set_depth_plan(Vec<DepthPass>)` | Order the overlay depth-samplers | The ONLY thing deciding whether the disk or the fog composites first. Built by [`depth_plan`] from the ordered recipe; unset keeps the legacy order. |
| `set_volumetric_disk(VolumetricDisk)` | Raymarched dust disk | Composited over the sky. |
| `set_ground_fog(GroundFog)` | Raymarched fog slab | Depth-aware; composited in the overlay pass. |
| `set_water(Water)` | Animated water MESH | A wave-displaced grid drawn in the opaque pass (occludes/occluded by terrain), sun specular from light slots 0/1. Grid uploaded once, reused. |
| `set_bloom(threshold,knee,intensity,radius)` | HDR bloom | Reads+writes `hdr`; **visual no-op unless the surface is HDR** (has an `hdr` attachment resolved by the tonemap). |
| `set_tonemap_grade(grade, grade_strength, exposure, AttachmentFormat)` | HDR → sRGB resolve | Routes lit passes into the `hdr` attachment and rolls off (ACES). Allocated lazily in the format `hdr` names. No-op without an HDR attachment. |

**Shadows** (producer/consumer, both scene-wired — see Sharp edges):

| Item | For | The one thing to know |
|---|---|---|
| `begin_shadow_view(light_view_proj)` | Enter a PRODUCER caster view | REPLACES the camera VP so lit passes write depth from the light. Same matrix [`LightRig::shadow_view_proj`] produces. Reset each `begin_frame`. |
| `set_shadow_source(source, light_view_proj, bias, light)` | Bind a shadow on a CONSUMER surface | Samples the producer target's depth. A surface that never calls it binds the `enabled=0` default (byte-identical no-shadow output). |

**Offscreen render targets** (the public RTT surface — declare draws via [`FrameGraph`], not
the `pub(crate)` `render_to_texture`):

| Item | For | The one thing to know |
|---|---|---|
| `create_render_target(w,h) -> RenderTargetHandle` | Allocate an offscreen surface | Its colour is registered in the texture store. |
| `target_texture(h) -> Option<TextureHandle>` | Sample a target's colour | `None` if freed — draw it as sprite/billboard/mesh. |
| `target_depth(h) -> Option<(&TextureView, u64)>` | A target's depth (for shadow consumers) | — |
| `free_render_target(h)` | Free target + its colour texture | Closes the append-only leak; safe the same frame it was last sampled. Handle + any `TextureHandle` from it go stale. |
| `resize_render_target(h,w,h2)` | Resize in place | Handle stays valid; re-fetch via `target_texture`. No-op if unchanged. |

**Window / display:** `window()`, `inner_size()`, `size()`, `resize(w,h)`, `monitor_size()`,
`video_mode_sizes()`, `is_fullscreen()`, `outer_position()` / `set_outer_position()`,
`set_windowed()`, `set_borderless_fullscreen()`, `set_exclusive_fullscreen() -> bool`. The
public field `clear_color: [f64;4]` is the swapchain clear (a root stage's authored `clear`
lands here).

### `FrameGraph` — the per-frame draw-order + compositing recorder

An ephemeral, **declare-only** recorder the scene manager builds ONCE per frame and
[`execute`](FrameGraph::execute)s exactly once, after every visible scene has declared into
it. `execute` runs four phases in order: every offscreen pass (dependency-ordered) → every
root element → screen composites → overlays.

| Item | For | The one thing to know |
|---|---|---|
| `FrameGraph::new()` | Start a frame's graph | Owned by the scene manager, one per frame. |
| `target(handle, clear, draw)` | Declare an offscreen sub-scene | Recipe-less; always [`Rate::Live`]. |
| `root(draw)` | Declare a screen-surface element | Full-window content; no target, no blit. Runs after every offscreen pass. |
| `overlay(draw)` | The screen surface's FINAL 2D | HUD replay + immediate 2D; runs last, after composites. |
| `surface(into, &StageDef, StageInputs, Rate, content)` | Declare a surface from its STAGE | **The recipe entry point.** Applies lighting/framing/clear, drives the rig once, and runs each [`PassDef`] in [`StageDef::pass_order`]. `into` = [`CompositeTarget::Target`] (offscreen) or [`CompositeTarget::Screen`] (root). |
| `composite_panel(src, into, Rect, layer, tint, Option<PanelFrame>, Option<Label>)` | Blit a target as a 2D panel | Emits panel→sprite→label at one layer. `into` a target records a render-order dependency. |
| `composite_billboard(src, into, world_pos, world_size, additive, tint)` | Blit a target as a world billboard | RTT-as-billboard; no pipeline change. |
| `set_base_layer(f32)` / `base_layer() -> f32` | Stamp/read a scene's depth band | `execute` restores each element's band before running it. |
| [`CompositeTarget`], [`Rect`], [`PanelFrame`], [`Label`] | Composite arguments | `CompositeTarget::{Screen, Target(h)}`; `Rect{pos,size}`; `PanelFrame` is a `draw_ui_panel` bundle + `inset`; `Label` is text over a panel. |

### The stage recipe — typed values (`stage.rs`)

Consumed by [`FrameGraph::surface`]; produced by `flicker-widgets`' parser. Author the JSON
per [`STAGES.md`](../../../content/sensorium/STAGES.md) — this is the **Rust-consumer** view.

| Item | What it is | The one thing to know |
|---|---|---|
| [`StageDef`] | The typed stage: `lighting`, `clear: Option`, `camera: Option`, `layers`, `attachments`, `passes`, `rate` | Every field defaults to its own type's (unlit, no clear, no framing, colour+depth, default recipe, live). `camera: None` = the scene owns the view (the globes). |
| `StageDef::recipe()` / `pass_order() -> (Vec<usize>, bool)` | The passes, and their dependency order | `pass_order` is Kahn over reads/writes; a cycle falls back to declaration order with `cyclic=true`. |
| `StageDef::layers_outside(&[..])` / `has_layer(kind)` | Which authored layer kinds a filler cannot draw | Names each undrawn kind once, so an unsupported layer warns, never silently no-ops. |
| `StageDef::CLEAR_UNAUTHORED` | Transparent clear for an unauthored offscreen surface | — |
| [`PassKind`] (+ `KINDS`, `kind()`) | The closed 9-kind pass roster | `scene`, `sky`, `volumetric_disk`, `ground_fog`, `tonemap_grade`, `composite`, `shadow_map`, `water_surface`, `bloom`. **Two are markers** — see Sharp edges. |
| [`PassDef`] (+ `new`, `default_reads`, `default_writes`) | One pass: `kind` + `reads` + `writes` | Reads/writes are the ONLY ordering info. Depth-samplers read `depth`; tonemap/bloom read `hdr`; scene writes `color`+`depth`. |
| [`TonemapGradePass`] / [`TonemapSlot`] | ACES resolve + colour grade (pass-owned) | `resolve(inputs) -> (tint, strength, exposure)`; `grade` tint is art (never bound), strength/exposure are bindable. |
| [`BloomPass`] | HDR bloom knobs (`threshold`/`knee`/`intensity`/`radius`) | The ONE roster kind with no binds and no per-frame input. |
| [`VolumetricPass`] / [`VolumetricSlot`] | Dust-disk pass + binds | `resolve(inputs) -> VolumetricDisk`; `gaps` come from `StageInputs`, never authored. |
| [`GroundFogPass`] / [`FogSlot`] | Fog pass + binds | `resolve(inputs, live_fog_color) -> GroundFog`; `floor` ADDS to the slab; unauthored `color` follows the live atmosphere. |
| [`WaterPass`] / [`WaterSlot`] | Animated-water pass + binds | `resolve(inputs) -> Water`; `sea_level`/`time` bindable; wave list packed to [`MAX_WAVE_SOURCES`]. No reflection source (mesh has a real specular). |
| [`ShadowMapPass`] | One kind, two roles by `from` | PRODUCER (`from: None`) writes `depth`; CONSUMER (`from: Some(surface)`) contributes an input binding (empty reads/writes, order-free). Scene-wired. |
| [`CompositePass`] | `from: <surface>` | The ordering edge for a cross-surface blit; the blit itself is `composite_panel`. Scene-wired. |
| [`Attachments`] / [`Attachment`] / [`AttachmentFormat`] | The images a surface owns | `AttachmentFormat::{Surface, Depth32, Rgba16f}`; `texture_format()` is the ONE format authority. `pixels(rect)` sizes off the **`color`** attachment's `scale` only (seam 81A1D5DC). Default = `color` + `depth`. Constants: `COLOR`/`DEPTH`/`HDR`, `NAMES`. |
| [`Rate`] (+ `NAMES`, `renders(drawn, since_last, dirty)`) | Refresh policy | `Live`/`Poster`/`Hz(f32)`/`Dirty`; a never-drawn surface always renders once. |
| [`StageInputs`] | The per-frame publish sink | `set(key, f32)` (bind sink), `gaps(..)`, `clock(..)`, `with_dirty(..)`; readers `get`/`keys`/`clock_seconds`/`is_dirty`. **A bound key nothing publishes leaves the authored value — see Sharp edges.** |
| [`DepthPass`] + [`depth_plan(&[&PassDef])`] | The overlay depth-sampler plan | `{Volumetric, GroundFog}`; the one builder [`FrameGraph::surface`] hands to `set_depth_plan`. Water is real geometry, so it is NOT in the plan though it reads depth. |
| [`StageLayer`] (+ `KINDS`, `kind()`) | One authored content layer | `Skinned`, `Ring`, `Grid`, `Shells`, `Shell`, `Graticule`, `Material`; a filler draws the ones it knows. |
| [`StageCamera`] (+ `camera()`) | Authored orbit framing | Default = the portrait framing; turns into a [`Camera`] looking at `target_y`. |
| [`ring_segments`] / [`grid_segments`] / [`grid_segments_xy`] | Pure line geometry | No GPU/state — build once, unit-test without a device. Degenerate inputs return empty, never panic. `_xy` is the Z-up sibling for editor space. |

### Scene data types (`mesh.rs`)

| Item | What it is | The one thing to know |
|---|---|---|
| [`Camera`] (+ `orbit`, `with_ortho_height`, `view`/`projection`/`view_projection`, `pick_ray`) | RH Y-up camera | `ortho_height: Some(h)` = orthographic (editor panels). `pick_ray` unprojects through the inverse VP — convention-proof for any camera. |
| [`ray_triangle(origin,dir,a,b,c) -> Option<f32>`] | Möller–Trumbore intersection | Front-face only, so a pick can't select a back-facing surface. |
| [`LightRig`] (+ `push`, `driven`, `sky_sun`/`sky_moon`, `shadow_view_proj`) | The frame's light list + atmosphere | ONE representation: a sun is `lights[0]` with `LightKind::Dir`. **Slots 0/1 ARE the sky slots** (a non-`Dir` or past-`count` slot reads black). `push` fills then refuses, loudly. |
| [`Light`] (+ `dir`, `point`, `radiance`) / [`LightKind`] | One light | `Dir`/`Point`/`Spot`; `radius <= 0` = no falloff (not a sentinel — the legacy point light). `radiance() = color * intensity`. |
| [`Driver`] (+ `gain(t)`) / [`DriverKind`] | Per-light intensity modulation | `Flicker` (only dims) / `Pulse`; deterministic in `(kind,speed,depth,seed,t)`; `depth==0` = literal `1.0` (bit-identical undriven). CPU-side, once per stage per frame. |
| [`MAX_LIGHTS`] | The rig ceiling (8) | Empty slots cost nothing per fragment. |
| [`MeshVertex`] / [`MeshHandle`] / [`MeshIndices`] / [`MeshDrawOptions`] | Flat-mesh types | `MeshVertex` = position+normal+material (28 bytes). `MeshDrawOptions`: `wireframe`/`tint`/`gloss`. |

### Effect params (re-exported from the pipeline modules)

| Item | What it is | The one thing to know |
|---|---|---|
| [`GroundFog`] | Ground-fog slab params | Band/colour/noise/wind; wrapped by [`GroundFogPass`]. |
| [`VolumetricDisk`] / [`MAX_VOLUMETRIC_BODIES`] | Dust-disk params | `formation`/`time`/`density` + carved `gaps`; up to 32 bodies. |
| [`Water`] / [`WaveSource`] / [`WaveKind`] / [`MAX_WAVE_SOURCES`] | Water params + wave roster | `WaveKind::{Radial, Directional}`; up to 6 sources (extras are a compile problem). Built by [`WaterPass::resolve`]. |
| [`PbrMaps`] / [`TexturedVertex`] / [`build_textured_verts`] | Textured-mesh inputs | `build_textured_verts` assembles a [`TexturedVertex`] buffer. |
| [`SkinnedVertex`] | GPU-skinning vertex | position/normal/uv + 4 joints/weights. |
| [`FontRole`] | Which registered face `draw_text_role` selects | — |

### Multi-view editor grid (`quad_grid.rs`)

The N-up viewport grid the asset-pipeline / controller-tester editors use — orbit cameras,
per-cell picking, and declaration into a [`FrameGraph`].

| Item | What it is |
|---|---|
| [`QuadGrid`] (+ `new`, `editor`, `views`, `set_viewport`/`viewport`, `camera`, `cell`/`cell_at`, `label_rect`/`label_hit`, `local_cursor`, `declare`/`declare_with`) | The grid of viewport cells over one render target. |
| [`ViewportFiller`] (+ `new`, `with_views`, `set_rect`/`rect`, `grid`/`grid_mut`, `orbit`/`orbit_mut`, `camera`/`cameras_with`/`camera_at`, `reset_framing`, `toggle_flip_at`, `apply_pointer`, `pick_ray_at`, `declare`/`declare_framed`) | A `QuadGrid` + per-cell [`Orbit`] state, seated in a rect. |
| [`Orbit`] (+ `dist`, `ortho_radius`, `zoom_by`, `camera`, `pan_by_view`, `orbit_by`, `apply_pointer`) | Per-cell orbit/zoom/pan camera state. |
| [`QuadView`] (+ `label_for`) / [`QuadStyle`] / [`ViewportLayout`] (+ `from_name`, `view_count`) | A cell's view definition / style / the layout roster. |
| [`EDITOR_QUADS`] / [`ORBIT_FOV_Y`] | The 4-up editor preset / the orbit FOV constant. |

### Handles & constants

| Item | What it is |
|---|---|
| [`RenderTargetHandle`] / [`TextureHandle`] / [`MeshHandle`] / [`TexturedMeshHandle`] / [`SkinnedMeshHandle`] | Opaque resource handles (persist across frames until freed). |
| [`FULL_TEXTURE`] | `[0,0,1,1]` — the whole-texture UV rect. |
| [`HDR_FORMAT`] | The `rgba16f` HDR intermediate format (defined THROUGH `AttachmentFormat::texture_format`, so it can't drift from the authored word). |
| [`TargetColor`] | `Srgb`/`Hdr` — which colour variant a lit pipeline renders into. *Informational:* the crate hides its pipelines behind `Renderer`, so a caller passes this only when driving a pipeline directly (which the public API does not expose). |
| [`MATERIAL_PALETTE_LEN`] | `256` — the material colour-palette length. |
| [`SkinnedMeshPipeline`] | The GPU-skinning pipeline. **Exported but not usable downstream** — see Findings (its constructor needs crate-private types). Use `Renderer::upload_skinned_mesh` / `draw_skinned_instanced` instead. |
| [`Mat4`] / [`Vec2`] / [`Vec3`] | Re-exported glam math. |

## Interactions

- **Signals / intents:** **None.** This crate has no input layer — it never sees signals,
  keys, or buttons. Input signals live in `flicker-input-core`; a scene wires them elsewhere
  and calls the draw API here.
- **The "Model" here is [`StageInputs`]** — typed, not JSON. A scene publishes `set(key, f32)`
  (plus `gaps`/`clock`/`dirty`); a recipe's `"<field>_bind": "<key>"` names one. **A bind
  REPLACES the authored field.** The owner of every key is the scene that publishes it; the
  recipe names it. A bound key nothing publishes is the crate's one silent seam (see below).
- **What it hands other crates:** the [`Renderer`] device+draw handle; [`RenderTargetHandle`]
  offscreen surfaces (sampleable as [`TextureHandle`]); the executor [`FrameGraph::surface`]
  that consumes a compiled [`StageDef`]; frame-graph composites between surfaces.
- **Threads / async:** `Renderer::new` is `async` (adapter/device request). Everything else
  is single-threaded, main-thread, synchronous. No workers, no channels.

## Gates

The contract tests a change must keep green (`cargo test -p flicker-render`):

- **Recipe ordering** (`stage.rs`): `pass_order_puts_writers_before_readers`,
  `the_default_recipe_is_one_scene_pass`, `encode_plan_matches_recipe_order`,
  `water_orders_after_scene_before_fog_and_is_not_a_depth_sampler`,
  `bloom_orders_after_hdr_writers_and_before_tonemap`.
- **Binds & sizing** (`stage.rs`): `inputs_replace_the_fields_a_recipe_binds`,
  `tonemap_binds_replace_the_authored_strength_and_exposure`, `attachments_pixels_honour_scale`.
- **Rate / liveness** (`stage.rs`): `rate_live_and_poster_reproduce_todays_liveness`,
  `a_poster_surface_renders_once_then_never`, `an_hz_surface_renders_on_the_clock`.
- **Layers & geometry** (`stage.rs`): `layers_outside_names_each_undrawn_kind_once`,
  `the_default_framing_is_the_portrait_and_looks_at_target_y`, `ring_closes_on_itself_…`,
  `grid_spans_the_extent_both_ways`, `xy_grid_is_flat_in_z_…`, `degenerate_ring_and_grid_are_empty_not_panics`.
- **Frame graph** (`frame_graph.rs`): `root_runs_after_every_offscreen_pass_and_before_screen_composites`,
  `overlay_runs_after_every_screen_composite`, `the_base_layer_of_each_step_is_the_band_it_was_declared_in`,
  `chain_renders_source_before_consumer`, `diamond_renders_root_first_and_sink_last`,
  `cycle_falls_back_without_panic`, `edge_to_undeclared_source_imposes_no_constraint`.
- **Lighting** (`mesh.rs`): `the_default_rig_is_the_legacy_trio_in_slot_order`,
  `the_roster_is_bounded_and_a_missing_key_light_is_black`, `drivers_are_deterministic_for_a_seed`.
- **Picking** (`mesh.rs`): `centre_pixel_ray_points_at_the_target`,
  `centre_ray_hits_a_triangle_at_the_target`, `back_faces_are_rejected`,
  `geometry_behind_the_ray_is_rejected`.
- Plus per-pipeline uniform/format gates across `pipeline_*.rs` (water: 6, bloom/shadow/sky/
  tonemap/volumetric/ground_fog/ui/mesh: the format & uniform-packing tests).

## Sharp edges

- **Two pass kinds are MARKERS.** [`PassKind::Composite`] and [`PassKind::ShadowMap`] are
  no-ops in the executor — they stand in a recipe only as the ordering edge and the fail-loud
  gate anchor. The real work is a companion `Renderer` call the SCENE must make:
  `composite_panel`/`composite_billboard` for a composite; `begin_shadow_view` (producer) +
  `set_shadow_source` (consumer) for a shadow. **Author one in JSON without the Rust wiring
  and it renders nothing** — the roster ([`PassKind::KINDS`]) can't tell these two from the
  seven data-complete kinds (banked seam 897E33F7).
- **A bound key nothing publishes falls to the authored value, silently.** A typo'd
  `"<field>_bind": "<key>"` where no `StageInputs::set(key, …)` matches leaves the authored
  number (a `*_bind::resolve` `get` returns `None` and the field is left alone) — it dims,
  it doesn't blank, so nothing errors. Only a per-scene gate an author must remember to write
  catches it (banked seam D5DC92D0; rule F17AF41E — a bind REPLACES, a number authored
  beside its bind is dead data and a loud parse problem).
- **`Attachment.scale` is honoured only on `color`.** [`Attachments::pixels`] sizes a surface
  off the `color` attachment's scale; a scale on `depth` or any other attachment is not
  consulted (banked seam 81A1D5DC). The parser separately rejects an `hdr` attachment whose
  scale ≠ 1.
- **A content closure calling `set_scene` REPLACES the stage's rig** — the rig is one value,
  last writer wins. A scene that owns its own lights composes them over `scene_lighting()`.
- **`FrameGraph` is declare-only.** Build one per frame, `execute` it exactly once (the scene
  manager does this). A scene's `render` only declares; it never executes a graph of its own.
- **A root (`Screen`) surface cannot poster.** It keeps no image, so a non-`Live` rate on a
  `FrameGraph::surface(Screen, …)` is warned and ignored — the root renders every frame.
- **`tick` advances the clock, not `begin_frame`.** `begin_frame` re-enters per offscreen
  pass; ticking there would over-count. Call `tick` once per frame before `begin_frame`.
- **Stray draws still render but are counted.** Any `draw_*` queued outside a declared pass
  lands in the main queues but is reported at `end_frame` (first stray frame, then every
  300th) — full-window content belongs in `FrameGraph::root`, an offscreen picture in
  `target`, a HUD in `overlay`.
- **HDR is opt-in.** `set_bloom` / `set_tonemap_grade` are visual no-ops unless the surface
  declares an `hdr` attachment (the compiler couples the attachment to the pass).
- **Sky / fog / disk / water need a camera** set this frame (they need the view ray) — no-op
  without `set_camera`.
- **2D never uses the depth buffer** — pure painter's order by `layer`, ties by submission order.
