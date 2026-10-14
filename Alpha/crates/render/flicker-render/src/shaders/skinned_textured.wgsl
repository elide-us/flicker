// Textured, PBR-lit GPU skinning — skinned.wgsl's instanced vertex stage (position, normal AND
// the bind-pose tangent skinned by the instance's palette) over material.wgsl's fragment path
// (prepended at module build, with the frame prelude). The "later slice" skinned.wgsl's header
// named: the animated preview showing its maps (Aaron, 2026-10-08).
//
// Bind groups: 0 = the frame (camera + lights), 1 = palettes + instances (vertex storage),
// 2 = the material (material.wgsl), 3 = the shadow bind.

struct Camera { view_projection: mat4x4<f32>, };

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

@group(3) @binding(0) var<uniform> shadow_uni: ShadowUniform;
@group(3) @binding(1) var shadow_tex: texture_depth_2d;
@group(3) @binding(2) var shadow_samp: sampler_comparison;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @location(5) tangent: vec4<f32>,
    @builtin(instance_index) instance: u32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world_tangent: vec3<f32>,
    @location(4) tangent_w: f32,
};

// Accumulate one bone influence over position, normal and tangent (unrolled per component).
fn accum(
    base: u32, joint: u32, weight: f32,
    position: vec3<f32>, normal: vec3<f32>, tangent: vec3<f32>,
    p: ptr<function, vec3<f32>>, n: ptr<function, vec3<f32>>, t: ptr<function, vec3<f32>>,
) {
    if (weight == 0.0) { return; }
    let m = palettes[base + joint];
    *p = *p + weight * (m * vec4<f32>(position, 1.0)).xyz;
    let linear = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
    *n = *n + weight * (linear * normal);
    *t = *t + weight * (linear * tangent);
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let inst = instances[in.instance];
    let base = inst.palette_offset;

    var pos = vec3<f32>(0.0);
    var nrm = vec3<f32>(0.0);
    var tan = vec3<f32>(0.0);
    accum(base, in.joints.x, in.weights.x, in.position, in.normal, in.tangent.xyz, &pos, &nrm, &tan);
    accum(base, in.joints.y, in.weights.y, in.position, in.normal, in.tangent.xyz, &pos, &nrm, &tan);
    accum(base, in.joints.z, in.weights.z, in.position, in.normal, in.tangent.xyz, &pos, &nrm, &tan);
    accum(base, in.joints.w, in.weights.w, in.position, in.normal, in.tangent.xyz, &pos, &nrm, &tan);

    let world = inst.model * vec4<f32>(pos, 1.0);
    var out: VertexOut;
    out.clip_position = camera.view_projection * world;
    out.world_position = world.xyz;
    out.world_normal = normalize((inst.model * vec4<f32>(nrm, 0.0)).xyz);
    out.world_tangent = (inst.model * vec4<f32>(tan, 0.0)).xyz;
    out.tangent_w = in.tangent.w;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // No per-draw tint or sheen on a skinned body; its albedo's alpha is a cutout.
    let shaded = shade_material(
        in.uv,
        in.world_normal,
        in.world_position,
        in.world_tangent,
        in.tangent_w,
        vec4<f32>(1.0, 1.0, 1.0, 1.0),
        0.0,
        false,
    );
    if (shaded.alpha < 0.5) {
        discard;
    }
    return shaded.color;
}
