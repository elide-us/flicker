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
//   * solid mode (`flags.x == 0.0`): Lambertian shading on a fixed
//     directional light, with the base color resolved from the packed
//     material — primary in low 12 bits, secondary in next 12, blend
//     factor in top 8 — by indexing a small color table and `mix`ing
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

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> per_draw: PerDraw;

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

        // Fallback for unknown materials (also the EMPTY=0 case).
        default: { return vec3<f32>(1.0, 0.0, 1.0); }
    }
}

// Resolve a packed material to a color. Primary in low 12 bits,
// secondary in next 12, blend in top 8. Linear interpolation between
// primary and secondary by blend / 255.
fn material_color(material: u32) -> vec3<f32> {
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

    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let lambert = max(dot(in.world_normal, light_dir), 0.0);
    let base = material_color(in.material);
    let shaded = base * (0.3 + 0.7 * lambert);
    return vec4<f32>(shaded, 1.0) * per_draw.tint;
}
