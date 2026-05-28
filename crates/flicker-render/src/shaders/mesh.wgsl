// 3D mesh shader.
//
// Vertex stage: transforms `position` from cluster-local through
// `model` and the camera's `view_projection` to clip space; carries
// world-space normal and world-space position to the fragment stage.
//
// Fragment stage:
//   * solid mode (`flags.x == 0.0`): Lambertian shading on a fixed
//     directional light, modulated by a procedural "missing texture"
//     magenta/black world-space checker and the `tint` uniform. The
//     checker reveals per-voxel geometry on flat shaded surfaces where
//     Lambertian alone would be uniform.
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
    return out;
}

// Procedural "missing texture" checker: 1-voxel-wide squares of
// magenta and black, keyed off the fragment's world position. The
// checker exists in 3D so it tiles correctly on any axis-aligned
// face. This is a debug-visualization stand-in until real materials
// land — adjacent voxels on the same face show alternating colors,
// making per-cell geometry visible without relying on shading
// variation.
fn material_color(world_position: vec3<f32>) -> vec3<f32> {
    // Floor each axis to a 1-voxel pitch, XOR the parities together.
    let cell = vec3<i32>(floor(world_position));
    let parity = (cell.x ^ cell.y ^ cell.z) & 1;
    if (parity == 0) {
        return vec3<f32>(1.0, 0.0, 1.0);  // magenta
    } else {
        return vec3<f32>(0.0, 0.0, 0.0);  // black
    }
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
    let base = material_color(in.world_position);
    let shaded = base * (0.3 + 0.7 * lambert);
    return vec4<f32>(shaded, 1.0) * per_draw.tint;
}
