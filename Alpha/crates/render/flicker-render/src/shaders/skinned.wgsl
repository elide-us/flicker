// Instanced, GPU-skinned mesh shader — Slice 1 of the animation field viewer.
//
// One static mesh, N instances, ONE draw call. Each instance carries its own
// bone-matrix palette (a slice of the shared `palettes` storage buffer, at
// `palette_offset`) plus a model transform. The vertex shader looks the instance
// up by `@builtin(instance_index)`, skins position + normal from that instance's
// palette (4-influence linear blend), applies the model transform, then the
// camera. The fragment is a simple Lambert over neutral steel, driven by the frame's
// LIGHT LIST — this slice proves the skinning + instancing path; texturing/PBR is a
// later slice (reuse the material path from mesh_textured.wgsl).
//
// Storage buffers are read in the VERTEX stage — requires the adapter's
// VERTEX_STORAGE downlevel capability (native Metal / Vulkan / D3D12 have it;
// the headless test in pipeline_skinned.rs validates it against Limits::default()).

struct Camera { view_projection: mat4x4<f32>, };

// The frame prelude (struct Light / Scene / ShadowUniform / light_sample / shadow_factor)
// is PREPENDED from `shaders/frame_prelude.wgsl` at module build — the ONE shared text, not
// a copy pasted here. See that file and `compose_lit` in `pipeline_mesh.rs`.

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

// The sun/light shadow map (group 2 is free for this pipeline). The prelude's
// `shadow_factor` reads these by name; a non-shadow surface binds a default with
// `enabled = 0`, so it returns 1.0 and this shader is byte-identical to the no-shadow path.
@group(2) @binding(0) var<uniform> shadow_uni: ShadowUniform;
@group(2) @binding(1) var shadow_tex: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

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
    // Ambient-seeded, exactly as the sun+moon sum this replaced. Point and spot lights
    // now REACH the skinned pass (they used to be silently dropped here); no shipped
    // stage pairs a `skinned` layer with a non-directional rig — gated scene-side.
    var diffuse = scene.ambient.rgb;
    for (var i = 0u; i < scene.counts.x; i = i + 1u) {
        let li = scene.lights[i];
        let s = light_sample(li, in.world_position);
        let radiance = li.color_intensity.rgb * li.color_intensity.w;
        // Shadow darkens only the light this map is cast for; vis = 1.0 exactly otherwise
        // (and for every surface with no shadow bound), so the term is bit-identical then.
        var vis = 1.0;
        if (shadow_uni.params.y > 0.5 && u32(shadow_uni.params.w) == i) {
            vis = shadow_factor(in.world_position);
        }
        diffuse = diffuse + radiance * (max(dot(n, s.xyz), 0.0) * s.w) * vis;
    }
    let shaded = base * diffuse;
    return vec4<f32>(shaded, 1.0);
}
