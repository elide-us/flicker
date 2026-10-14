// Textured 3D mesh shader — the albedo-textured, PBR-lit sibling of mesh.wgsl.
//
// Vertex stage: transforms position through model + camera to clip space; carries
// world-space normal + tangent + position and the UV to the fragment stage.
// Fragment stage: `shade_material` (material.wgsl, prepended at module build — the material
// bind group at group 2 and the whole PBR path live there, shared with the skinned textured
// shader): samples albedo (sRGB) for base colour and the PBR map set (LINEAR), a tangent-space
// normal map (perturbs the world normal via a TBN built from the interpolated normal + tangent),
// roughness, metalness, AO, and self-illumination (`Emit`).
// Lighting keeps the SAME light-list Lambertian diffuse as mesh.wgsl, then:
//   * diffuse is scaled by (1 - metalness) and by AO (metal has no diffuse; AO darkens);
//   * a pragmatic GGX-lite specular is added per light — its lobe sharpens with
//     smoothness (1 - roughness) and its colour is white for dielectrics, tinted by
//     albedo for metals (Fresnel-ish F0 = mix(0.04, albedo, metalness)). "Good-enough
//     reflective steel," stable and not blown-out — NOT full Cook-Torrance.
// Alpha-tests to cut fully-transparent texels (hair cards) so alpha reads as a cutout.

struct Camera {
    view_projection: mat4x4<f32>,
};

struct PerDraw {
    model: mat4x4<f32>,
    tint: vec4<f32>,
    flags: vec4<f32>, // flags.y = gloss (sheen strength), flags.z = soft-alpha blend.
};

// The frame prelude (struct Light / Scene / ShadowUniform / light_sample / shadow_factor)
// is PREPENDED from `shaders/frame_prelude.wgsl` at module build — the ONE shared text, not
// a copy pasted here. See that file and `compose_lit` in `pipeline_mesh.rs`.

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> scene: Scene;

@group(1) @binding(0) var<uniform> per_draw: PerDraw;

// The sun/light shadow map at group 3 (group 2 is the material set above). The prelude's
// `shadow_factor` reads these by name; a non-shadow surface binds a default with
// `enabled = 0`, so it returns 1.0 and this shader is byte-identical to the no-shadow path.
@group(3) @binding(0) var<uniform> shadow_uni: ShadowUniform;
@group(3) @binding(1) var shadow_tex: texture_depth_2d;
@group(3) @binding(2) var shadow_samp: sampler_comparison;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>, // xyz = tangent, w = handedness (+1/-1)
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world_tangent: vec3<f32>,
    @location(4) tangent_w: f32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let world = per_draw.model * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_projection * world;
    out.world_position = world.xyz;
    out.world_normal = normalize((per_draw.model * vec4<f32>(in.normal, 0.0)).xyz);
    out.world_tangent = (per_draw.model * vec4<f32>(in.tangent.xyz, 0.0)).xyz;
    out.tangent_w = in.tangent.w;
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // `flags.z` selects SOFT-ALPHA blend mode (clouds / ground decals); the default (0) is a
    // cutout that drops fully-transparent texels (hair-card edges).
    let soft = per_draw.flags.z > 0.5;
    let shaded = shade_material(
        in.uv,
        in.world_normal,
        in.world_position,
        in.world_tangent,
        in.tangent_w,
        per_draw.tint,
        per_draw.flags.y,
        soft,
    );
    if (!soft && shaded.alpha < 0.5) {
        discard;
    }
    return shaded.color;
}
