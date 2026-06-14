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
//     base color is resolved from the packed material — primary in low
//     12 bits, secondary in next 12, blend factor in top 8 — by indexing
//     a small color table and `mix`ing primary→secondary by `blend / 255`.
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
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> per_draw: PerDraw;
@group(0) @binding(2) var<uniform> scene: Scene;

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

// Look up a base color for a single material index. Demo palette —
// extend as new materials are added. The `default` arm returns magenta
// so EMPTY (=0) and unknown indices remain visible as "missing".
fn material_index_color(index: u32) -> vec3<f32> {
    switch index {
        // ---- water depth band ----
        case 1u: { return vec3<f32>(0.04, 0.10, 0.28); } // DEEP_WATER   navy
        case 2u: { return vec3<f32>(0.10, 0.25, 0.50); } // MID_WATER    blue
        case 3u: { return vec3<f32>(0.30, 0.55, 0.75); } // SHALLOW      cerulean
        case 4u: { return vec3<f32>(0.75, 0.85, 0.90); } // CREST        pale
        case 5u: { return vec3<f32>(0.95, 0.97, 0.98); } // FOAM         off-white

        // ---- cloud band ----
        case 6u: { return vec3<f32>(0.30, 0.32, 0.36); } // CLOUD_DARK   storm underbelly
        case 7u: { return vec3<f32>(0.65, 0.67, 0.70); } // CLOUD_MID    mid grey
        case 8u: { return vec3<f32>(0.92, 0.94, 0.96); } // CLOUD_LIGHT  sunlit crown

        // ---- atmospheric ----
        case 9u: { return vec3<f32>(0.80, 0.82, 0.86); } // CIRRUS       wispy pale

        // ---- matte test surface ----
        // Neutral mid-value stone, faintly warm. Deliberately desaturated and
        // ~0.45 so it reads Lambertian gradients, fog, and colour grading
        // cleanly without fighting them — the surface the lighting cycle is
        // assessed against. Used by the voxel-cluster field (`Material::new(10..)`).
        case 10u: { return vec3<f32>(0.46, 0.44, 0.42); } // STONE        matte neutral

        // ---- world-sim surface stack (demo) ----
        case 11u: { return vec3<f32>(0.95, 0.35, 0.10); } // LAVA         hot orange
        case 12u: { return vec3<f32>(0.80, 0.90, 1.00); } // ICE          pale icy blue
        case 13u: { return vec3<f32>(0.32, 0.42, 0.24); } // LAND         mossy ground

        // ---- upper atmosphere heatmaps (demo) ----
        case 14u: { return vec3<f32>(0.20, 0.95, 0.55); } // AURORA       thermosphere glow
        case 15u: { return vec3<f32>(0.55, 0.32, 0.88); } // UV           ozone absorption
        case 16u: { return vec3<f32>(0.02, 0.02, 0.06); } // VOID         night-side / space

        // ---- biomes (climate-classified land) ----
        case 17u: { return vec3<f32>(0.82, 0.72, 0.45); } // DESERT       sand
        case 18u: { return vec3<f32>(0.70, 0.68, 0.32); } // SAVANNA      dry grass
        case 19u: { return vec3<f32>(0.45, 0.65, 0.30); } // GRASSLAND    green
        case 20u: { return vec3<f32>(0.20, 0.45, 0.22); } // FOREST       forest green
        case 21u: { return vec3<f32>(0.10, 0.33, 0.18); } // RAINFOREST   deep green
        case 22u: { return vec3<f32>(0.26, 0.42, 0.34); } // TAIGA        cold conifer
        case 23u: { return vec3<f32>(0.56, 0.52, 0.46); } // TUNDRA       grey-brown

        // ---- world-gen rock hardness ramp (Epoch viz) ----
        case 24u: { return vec3<f32>(0.30, 0.25, 0.22); } // ROCK_SOFT    dark, weak (shale/clay)
        case 25u: { return vec3<f32>(0.82, 0.80, 0.75); } // ROCK_HARD    pale, resistant (granite)

        // ---- world-gen ore veins (Epoch 5) ----
        case 26u: { return vec3<f32>(0.62, 0.20, 0.14); } // ORE_IRON     hematite red
        case 27u: { return vec3<f32>(0.78, 0.46, 0.16); } // ORE_COPPER   coppery rust
        case 28u: { return vec3<f32>(0.93, 0.78, 0.28); } // ORE_GOLD     gold
        case 29u: { return vec3<f32>(0.80, 0.82, 0.85); } // ORE_SILVER   bright silver
        case 30u: { return vec3<f32>(0.58, 0.30, 0.66); } // ORE_OTHER    violet (rare metal)

        // Fallback for unknown materials (also the EMPTY=0 case).
        default: { return vec3<f32>(1.0, 0.0, 1.0); }
    }
}

// Resolve a packed material to a color. Two encodings:
//   * Direct RGB (escape): primary == 0xFFF marks a packed RGB666 colour in the
//     upper bits (R bits 12-17, G 18-23, B 24-29) — for continuous data maps the
//     palette can't express. No real palette entry uses index 0xFFF.
//   * Palette blend (default): primary in low 12 bits, secondary in next 12,
//     blend in top 8 — linear interpolation between two palette colours.
fn material_color(material: u32) -> vec3<f32> {
    if ((material & 0xFFFu) == 0xFFFu) {
        let r = f32((material >> 12u) & 0x3Fu) / 63.0;
        let g = f32((material >> 18u) & 0x3Fu) / 63.0;
        let b = f32((material >> 24u) & 0x3Fu) / 63.0;
        return vec3<f32>(r, g, b);
    }
    let primary = material & 0xFFFu;
    let secondary = (material >> 12u) & 0xFFFu;
    let blend = f32((material >> 24u) & 0xFFu) / 255.0;
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
    let shaded = base * (scene.ambient.rgb + sun + moon);
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
