// Instanced, GPU-skinned mesh shader — Slice 1 of the animation field viewer.
//
// One static mesh, N instances, ONE draw call. Each instance carries its own
// bone-matrix palette (a slice of the shared `palettes` storage buffer, at
// `palette_offset`) plus a model transform. The vertex shader looks the instance
// up by `@builtin(instance_index)`, skins position + normal from that instance's
// palette (4-influence linear blend), applies the model transform, then the
// camera. The fragment is a simple two-light Lambert over neutral steel — this
// slice proves the skinning + instancing path; texturing/PBR is a later slice
// (reuse the material path from mesh_textured.wgsl).
//
// Storage buffers are read in the VERTEX stage — requires the adapter's
// VERTEX_STORAGE downlevel capability (native Metal / Vulkan / D3D12 have it;
// the headless test in pipeline_skinned.rs validates it against Limits::default()).

struct Camera { view_projection: mat4x4<f32>, };

// Mirrors `SceneUniform` (pipeline_mesh.rs) — 10 vec4s, std140-trivial.
struct Scene {
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    moon_dir: vec4<f32>,
    moon_color: vec4<f32>,
    ambient: vec4<f32>,
    camera_pos: vec4<f32>,
    fog_color: vec4<f32>,
    grade: vec4<f32>,
    point_pos: vec4<f32>,
    point_color: vec4<f32>,
};

struct Instance {
    model: mat4x4<f32>,
    palette_offset: u32,
    bone_count: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> scene: Scene;
@group(1) @binding(0) var<storage, read> palettes: array<mat4x4<f32>>;
@group(1) @binding(1) var<storage, read> instances: array<Instance>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @builtin(instance_index) instance: u32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
};

// Accumulate one bone influence (unrolled per component, so no dynamic vector
// indexing). `base` is the instance's palette offset; `joint` a bone index.
fn accum(
    base: u32, joint: u32, weight: f32,
    position: vec3<f32>, normal: vec3<f32>,
    p: ptr<function, vec3<f32>>, n: ptr<function, vec3<f32>>,
) {
    if (weight == 0.0) { return; }
    let m = palettes[base + joint];
    *p = *p + weight * (m * vec4<f32>(position, 1.0)).xyz;
    let linear = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
    *n = *n + weight * (linear * normal);
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let inst = instances[in.instance];
    let base = inst.palette_offset;

    var pos = vec3<f32>(0.0);
    var nrm = vec3<f32>(0.0);
    accum(base, in.joints.x, in.weights.x, in.position, in.normal, &pos, &nrm);
    accum(base, in.joints.y, in.weights.y, in.position, in.normal, &pos, &nrm);
    accum(base, in.joints.z, in.weights.z, in.position, in.normal, &pos, &nrm);
    accum(base, in.joints.w, in.weights.w, in.position, in.normal, &pos, &nrm);

    let world = inst.model * vec4<f32>(pos, 1.0);
    var out: VertexOut;
    out.clip_position = camera.view_projection * world;
    out.world_position = world.xyz;
    out.world_normal = normalize((inst.model * vec4<f32>(nrm, 0.0)).xyz);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let base = vec3<f32>(0.55, 0.57, 0.62); // neutral steel — reads the pose via lighting
    let n = normalize(in.world_normal);
    let sun = scene.sun_color.rgb * max(dot(n, scene.sun_dir.xyz), 0.0);
    let moon = scene.moon_color.rgb * max(dot(n, scene.moon_dir.xyz), 0.0);
    let shaded = base * (scene.ambient.rgb + sun + moon);
    return vec4<f32>(shaded, 1.0);
}
