// Textured 3D mesh shader — the albedo-textured, PBR-lit sibling of mesh.wgsl.
//
// Vertex stage: transforms position through model + camera to clip space; carries
// world-space normal + tangent + position and the UV to the fragment stage.
// Fragment stage: samples albedo (sRGB) for base colour and the PBR map set (LINEAR) —
// all in one combined material bind group (group 1): a tangent-space normal map
// (perturbs the world normal via a TBN built from the interpolated normal + tangent),
// roughness, metalness, AO, and self-illumination (`Emit`).
// Lighting keeps the SAME sun/moon/point Lambertian diffuse as mesh.wgsl, then:
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
    flags: vec4<f32>, // flags.y = gloss (sheen strength); rest reserved.
};

// Frame-global lighting / atmosphere — identical layout to mesh.wgsl's `Scene`
// (mirrored CPU-side by `SceneUniform`, shared here via the same uniform buffer shape).
struct Scene {
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    moon_dir: vec4<f32>,
    moon_color: vec4<f32>,
    ambient: vec4<f32>,
    camera_pos: vec4<f32>,
    fog_color: vec4<f32>,   // rgb = fog colour; w = density
    grade: vec4<f32>,       // rgb = grade tint; w = strength (reserved)
    point_pos: vec4<f32>,
    point_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> per_draw: PerDraw;
@group(0) @binding(2) var<uniform> scene: Scene;

// Combined material group: 5 textures + one shared sampler. Packed into a single bind
// group to stay within the default `max_bind_groups` limit of 4 (group 0 = uniforms).
@group(1) @binding(0) var albedo_tex: texture_2d<f32>;
@group(1) @binding(1) var normal_tex: texture_2d<f32>;
@group(1) @binding(2) var rough_tex: texture_2d<f32>;
@group(1) @binding(3) var metal_tex: texture_2d<f32>;
@group(1) @binding(4) var ao_tex: texture_2d<f32>;
// Self-illumination. sRGB colour data per the content standard's `Emit` map, so it
// is a COLOUR (a rune glows blue while the metal beside it stays dark), not a
// scalar mask. Default 1x1 BLACK ⇒ a draw that omits it emits nothing.
@group(1) @binding(5) var emit_tex: texture_2d<f32>;
@group(1) @binding(6) var mat_sampler: sampler;

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

// One light's contribution: Lambertian diffuse + a smoothness-sharpened Blinn-Phong-ish
// specular. `spec_color` is the light-scaled F0 (white dielectric / albedo-tinted metal),
// `shininess` maps roughness→lobe width.
fn light_contrib(
    n: vec3<f32>,
    l: vec3<f32>,
    v: vec3<f32>,
    light_color: vec3<f32>,
    spec_color: vec3<f32>,
    shininess: f32,
) -> vec3<f32> {
    let ndl = max(dot(n, l), 0.0);
    if (ndl <= 0.0) {
        return vec3<f32>(0.0);
    }
    let half_vec = normalize(l + v);
    let ndh = max(dot(n, half_vec), 0.0);
    // Normalized-ish Blinn-Phong lobe; the (shininess+... ) factor keeps energy roughly
    // bounded so sharp highlights don't blow out.
    let spec = pow(ndh, shininess) * (shininess + 2.0) / 8.0;
    return light_color * ndl * (spec_color * spec);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(albedo_tex, mat_sampler, in.uv);
    // `flags.z` selects SOFT-ALPHA blend mode (clouds / ground decals): blend by the
    // texture's alpha. The default (0) is a cutout that drops fully-transparent texels
    // (hair-card edges). Opaque albedo (a==1) is unaffected either way.
    let soft = per_draw.flags.z > 0.5;
    if (!soft && texel.a < 0.5) {
        discard;
    }
    let base = texel.rgb;

    // --- Sample the PBR maps (linear). Defaults (draws omitting a map) are flat
    // normal (0.5,0.5,1) / rough=1 / metal=0 / ao=1, so an albedo-only draw is a matte
    // dielectric. ---
    let rough = clamp(textureSample(rough_tex, mat_sampler, in.uv).r, 0.04, 1.0);
    let metal = clamp(textureSample(metal_tex, mat_sampler, in.uv).r, 0.0, 1.0);
    let ao = textureSample(ao_tex, mat_sampler, in.uv).r;
    let emit = textureSample(emit_tex, mat_sampler, in.uv).rgb;

    // --- Build the perturbed world normal from the tangent-space normal map. ---
    var geo_n = normalize(in.world_normal);
    // Re-orthonormalize the tangent against the (interpolated) normal (Gram-Schmidt).
    var t = in.world_tangent - geo_n * dot(geo_n, in.world_tangent);
    let t_len = length(t);
    var n = geo_n;
    if (t_len > 1e-5) {
        t = t / t_len;
        let b = cross(geo_n, t) * in.tangent_w;
        let tn = textureSample(normal_tex, mat_sampler, in.uv).xyz * 2.0 - 1.0;
        // TBN * tangent-space normal → world space.
        n = normalize(tn.x * t + tn.y * b + tn.z * geo_n);
    }

    let view_dir = normalize(scene.camera_pos.xyz - in.world_position);

    // Diffuse Lambertian from the three lights (same as mesh.wgsl).
    let sun_d = scene.sun_color.rgb * max(dot(n, scene.sun_dir.xyz), 0.0);
    let moon_d = scene.moon_color.rgb * max(dot(n, scene.moon_dir.xyz), 0.0);
    let to_point = scene.point_pos.xyz - in.world_position;
    let point_dir = to_point / max(length(to_point), 1e-4);
    let point_d = scene.point_color.rgb * max(dot(n, point_dir), 0.0);

    // Metal has (almost) no diffuse; AO attenuates the ambient + diffuse floor.
    let diffuse_amt = (1.0 - metal);
    let ambient = scene.ambient.rgb * ao;
    let diffuse = base * (ambient + (sun_d + moon_d + point_d) * diffuse_amt);

    // Pragmatic specular. F0 = 0.04 for dielectric, albedo for metal. Smoothness →
    // shininess exponent (rougher = broader/dimmer).
    let f0 = mix(vec3<f32>(0.04), base, metal);
    let smoothness = 1.0 - rough;
    let shininess = exp2(1.0 + smoothness * 10.0); // ~2 (rough) .. ~2048 (mirror)
    var spec = vec3<f32>(0.0);
    spec = spec + light_contrib(n, scene.sun_dir.xyz, view_dir, scene.sun_color.rgb, f0, shininess);
    spec = spec + light_contrib(n, scene.moon_dir.xyz, view_dir, scene.moon_color.rgb, f0, shininess);
    spec = spec + light_contrib(n, point_dir, view_dir, scene.point_color.rgb, f0, shininess);
    // A small ambient specular so metal reads reflective even away from a direct highlight.
    spec = spec + f0 * ambient * smoothness;

    // Existing gloss sheen term (flags.y), preserved for callers that used it. Kept
    // subtle and additive on top of the PBR specular.
    let gloss = per_draw.flags.y;
    var sheen = vec3<f32>(0.0);
    if (gloss > 0.001) {
        let ndv = max(dot(n, view_dir), 0.0);
        let fresnel = pow(1.0 - ndv, 3.0);
        let half_vec = normalize(point_dir + view_dir);
        let broad = pow(max(dot(n, half_vec), 0.0), 3.0);
        let ndl = max(dot(n, point_dir), 0.0);
        sheen = scene.point_color.rgb * (0.35 * fresnel + 0.10 * broad) * ndl * gloss;
    }

    let shaded = diffuse + spec + sheen;
    // EMISSION is added AFTER the tint and BEFORE fog. After the tint because a
    // glow is the surface's own light — dimming the object must not dim what it
    // emits — and before fog because distance still swallows a glow like any other
    // radiance. Never multiplied by AO or a light term: nothing shadows a light.
    let lit = vec4<f32>(shaded, 1.0) * per_draw.tint + vec4<f32>(emit, 0.0);

    let dist = length(in.world_position - scene.camera_pos.xyz);
    let fog = 1.0 - exp(-scene.fog_color.w * dist);
    let rgb = mix(lit.rgb, scene.fog_color.rgb, fog);
    // Soft mode blends by texture alpha × tint alpha; cutout/opaque mode uses tint alpha.
    let out_a = select(lit.a, texel.a * per_draw.tint.a, soft);
    return vec4<f32>(rgb, out_a);
}
