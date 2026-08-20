// 3D mesh shader.
//
// Vertex stage: transforms `position` from cluster-local through
// `model` and the camera's `view_projection` to clip space; carries
// world-space normal, world-space position, and the packed per-voxel
// `material` index to the fragment stage. `material` is flat-
// interpolated — each triangle takes its provoking vertex's value, so
// shading is uniform across a triangle and the smooth gradients in the
// scene come from adjacent voxels carrying adjacent primary/secondary/
// blend values, not from per-pixel interpolation of indices.
//
// Fragment stage:
//   * solid mode (`flags.x == 0.0`): Lambertian shading from two
//     directional lights (sun + moon) over a flat ambient, all driven
//     by the frame-global `Scene` uniform (`Renderer::set_scene`); the
//     base color is resolved from the packed material — primary id in
//     bits 0-7, secondary id in 8-15, blend factor in 16-23 (bit 31 =
//     direct-RGB escape) — by indexing a small color table and `mix`ing
//     primary→secondary by `blend / 255`.
//   * wireframe mode (`flags.x == 1.0`): emit the fixed wireframe
//     color directly. The renderer enters this branch only when it
//     issues a line-list draw against a separately built edge index
//     buffer (see `pipeline_mesh.rs`), so every fragment is genuinely
//     on a triangle edge. No per-vertex barycentric trick is needed.

struct Camera {
    view_projection: mat4x4<f32>,
};

struct PerDraw {
    model: mat4x4<f32>,
    tint: vec4<f32>,
    // flags.x: 1.0 = wireframe mode, 0.0 = filled. Other components reserved.
    flags: vec4<f32>,
};

// Frame-global lighting / atmosphere. One uniform carries the whole
// day/night cycle: two directional lights (sun + moon), a flat ambient,
// plus fog + colour-grade fields used by later slices. Laid out as
// `vec4`s so std140 alignment is trivially correct (each field is 16-byte
// aligned, no implicit padding); the `.w` lanes pack the two scalars.
// Mirrored CPU-side by `SceneUniform` in `pipeline_mesh.rs`.
struct Scene {
    sun_dir: vec4<f32>,     // xyz = direction toward the sun (normalized); w unused
    sun_color: vec4<f32>,   // rgb = sun radiance; w unused
    moon_dir: vec4<f32>,    // xyz = direction toward the moon (normalized); w unused
    moon_color: vec4<f32>,  // rgb = moon radiance; w unused
    ambient: vec4<f32>,     // rgb = ambient floor; w unused
    camera_pos: vec4<f32>,  // xyz = world camera position (fog distance, later)
    fog_color: vec4<f32>,   // rgb = fog colour; w = fog density (later)
    grade: vec4<f32>,       // rgb = colour-grade tint; w = grade strength (later)
    point_pos: vec4<f32>,   // xyz = point-light world position (e.g. a star); w unused
    point_color: vec4<f32>, // rgb = point-light radiance; black = off
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> per_draw: PerDraw;
@group(0) @binding(2) var<uniform> scene: Scene;
// One colour per material-catalog slot (materials.json id = index). Booted
// all-magenta by the pipeline; `Renderer::set_material_palette` uploads the
// catalog colours, so an undefined id stays visibly "missing".
@group(0) @binding(3) var<uniform> palette: array<vec4<f32>, 256>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) material: u32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) @interpolate(flat) material: u32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let world = per_draw.model * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_projection * world;
    out.world_position = world.xyz;
    // Treat the model as rigid (no non-uniform scale); transform normals
    // by `model`'s upper-3x3 directly. For shears or non-uniform scale
    // we'd want the inverse-transpose — not needed by the voxel
    // pipeline at this phase.
    out.world_normal = normalize((per_draw.model * vec4<f32>(in.normal, 0.0)).xyz);
    out.material = in.material;
    return out;
}

// Look up a base color for a single material id — one read from the
// material-catalog palette (materials.json colours, uploaded by
// `Renderer::set_material_palette`). Undefined ids (and an unset palette)
// read the boot magenta, so "missing" stays visible.
fn material_index_color(index: u32) -> vec3<f32> {
    return palette[index & 0xFFu].rgb;
}

// Resolve a packed material to a color. Two encodings (the u8 catalog layout,
// 2026-08-19 — matches flicker-voxel `Material`):
//   * Direct RGB (escape): bit 31 set marks an RGB888 colour in bits 0-23
//     (R 0-7, G 8-15, B 16-23) — for continuous data maps the palette can't
//     express. Costs no material id; catalog words never set the top byte.
//   * Palette blend (default): primary id in bits 0-7, secondary id in 8-15,
//     blend in 16-23 — linear interpolation between two palette colours.
fn material_color(material: u32) -> vec3<f32> {
    if ((material & 0x80000000u) != 0u) {
        let r = f32(material & 0xFFu) / 255.0;
        let g = f32((material >> 8u) & 0xFFu) / 255.0;
        let b = f32((material >> 16u) & 0xFFu) / 255.0;
        return vec3<f32>(r, g, b);
    }
    let primary = material & 0xFFu;
    let secondary = (material >> 8u) & 0xFFu;
    let blend = f32((material >> 16u) & 0xFFu) / 255.0;
    let c_primary = material_index_color(primary);
    let c_secondary = material_index_color(secondary);
    return mix(c_primary, c_secondary, blend);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if (per_draw.flags.x > 0.5) {
        // Wireframe mode: line-list draws reach this branch and every
        // fragment is on a triangle edge by construction.
        return vec4<f32>(0.2, 0.9, 0.4, 1.0);
    }

    let base = material_color(in.material);
    // Two directional lights (sun + moon) with per-light colour, over a
    // flat ambient floor. Each light contributes a matte Lambertian term;
    // below-horizon lights fade by carrying a near-zero colour from the
    // example-side day-arc math, so no explicit night branch is needed.
    let sun = scene.sun_color.rgb * max(dot(in.world_normal, scene.sun_dir.xyz), 0.0);
    let moon = scene.moon_color.rgb * max(dot(in.world_normal, scene.moon_dir.xyz), 0.0);
    // Point light (e.g. a central star): lit from each fragment's own direction to the light,
    // so bodies at different world positions get correct, individual day/night terminators.
    let to_point = scene.point_pos.xyz - in.world_position;
    let point_dir = to_point / max(length(to_point), 1e-4);
    let point = scene.point_color.rgb * max(dot(in.world_normal, point_dir), 0.0);
    // Liquid/icy **sheen** (per-draw gloss = `flags.y`). NOT a mirror specular — a tight hot-spot
    // reads as a marble, wrong at planet scale. Instead the wet cue is a soft **limb sheen**:
    // brightest where the view grazes the surface (Fresnel, the planet's lit edge), with only a
    // faint, broad sunward lift — an ocean/atmosphere look, no bright reflection dot. Lit side
    // only, scaled by gloss; matte surfaces (gloss 0) skip it and read exactly as before.
    let gloss = per_draw.flags.y;
    var sheen = vec3<f32>(0.0);
    if (gloss > 0.001) {
        let view_dir = normalize(scene.camera_pos.xyz - in.world_position);
        let ndl = max(dot(in.world_normal, point_dir), 0.0);
        let ndv = max(dot(in.world_normal, view_dir), 0.0);
        let fresnel = pow(1.0 - ndv, 3.0); // grazing-angle limb brightening
        let half_vec = normalize(point_dir + view_dir);
        let broad = pow(max(dot(in.world_normal, half_vec), 0.0), 3.0); // very broad, faint
        sheen = scene.point_color.rgb * (0.35 * fresnel + 0.10 * broad) * ndl * gloss;
    }
    let shaded = base * (scene.ambient.rgb + sun + moon + point) + sheen;
    let lit = vec4<f32>(shaded, 1.0) * per_draw.tint;

    // Distance fog (forward): exponential by view distance, blending the lit
    // surface toward `fog_color` — driven example-side to the sky-horizon
    // colour, so far terrain melts into the sky rather than a flat grey wall.
    // `fog_color.w` is the density (0 ⇒ no fog); the camera position rides in
    // the Scene uniform. Keeps the HUD/2D crisp — fog lives only in the 3D pass.
    let dist = length(in.world_position - scene.camera_pos.xyz);
    let fog = 1.0 - exp(-scene.fog_color.w * dist);
    let rgb = mix(lit.rgb, scene.fog_color.rgb, fog);
    return vec4<f32>(rgb, lit.a);
}
