// THE PBR MATERIAL — the one fragment path every textured surface shades through, skinned or
// not: the combined material bind group (group 2: albedo / normal / roughness / metalness / AO /
// emit + the shared sampler) and `shade_material`, which samples it and lights it over the
// frame's LIGHT LIST exactly as mesh_textured.wgsl always has (its math, unchanged, moved here).
// PREPENDED to a shader by `compose_material` (pipeline_mesh_textured.rs), after the frame
// prelude, the way the prelude itself is — the ONE shared text, not a copy pasted into each
// shader. The shader keeps its own vertex stage, per-draw bindings and shadow bind (group 3)
// and calls `shade_material` from `fs_main`.

// Combined material group: 6 textures + one shared sampler. Packed into a single bind
// group to stay within the default `max_bind_groups` limit of 4 (0 = frame, 1 = per-draw).
@group(2) @binding(0) var albedo_tex: texture_2d<f32>;
@group(2) @binding(1) var normal_tex: texture_2d<f32>;
@group(2) @binding(2) var rough_tex: texture_2d<f32>;
@group(2) @binding(3) var metal_tex: texture_2d<f32>;
@group(2) @binding(4) var ao_tex: texture_2d<f32>;
// Self-illumination. sRGB colour data per the content standard's `Emit` map, so it
// is a COLOUR (a rune glows blue while the metal beside it stays dark), not a
// scalar mask. Default 1x1 BLACK ⇒ a draw that omits it emits nothing.
@group(2) @binding(5) var emit_tex: texture_2d<f32>;
@group(2) @binding(6) var mat_sampler: sampler;

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

/// What `shade_material` hands back: the lit colour (its alpha the blend's), and the albedo
/// texel's own alpha for the caller's cutout test.
struct Shaded {
    color: vec4<f32>,
    alpha: f32,
};

fn shade_material(
    uv: vec2<f32>,
    world_normal: vec3<f32>,
    world_position: vec3<f32>,
    world_tangent: vec3<f32>,
    tangent_w: f32,
    tint: vec4<f32>,
    gloss: f32,
    soft: bool,
) -> Shaded {
    let texel = textureSample(albedo_tex, mat_sampler, uv);
    // `soft` is the SOFT-ALPHA blend mode (clouds / ground decals): blend by the texture's
    // alpha. Otherwise the caller CUTS OUT fully-transparent texels (hair-card edges) on the
    // `alpha` handed back — after every sample, so no derivative lands in non-uniform control
    // flow. Opaque albedo (a==1) is unaffected either way.
    let base = texel.rgb;

    // --- Sample the PBR maps (linear). Defaults (draws omitting a map) are flat
    // normal (0.5,0.5,1) / rough=1 / metal=0 / ao=1, so an albedo-only draw is a matte
    // dielectric. ---
    let rough = clamp(textureSample(rough_tex, mat_sampler, uv).r, 0.04, 1.0);
    let metal = clamp(textureSample(metal_tex, mat_sampler, uv).r, 0.0, 1.0);
    let ao = textureSample(ao_tex, mat_sampler, uv).r;
    let emit = textureSample(emit_tex, mat_sampler, uv).rgb;

    // --- Build the perturbed world normal from the tangent-space normal map. ---
    var geo_n = normalize(world_normal);
    // Re-orthonormalize the tangent against the (interpolated) normal (Gram-Schmidt).
    var t = world_tangent - geo_n * dot(geo_n, world_tangent);
    let t_len = length(t);
    var n = geo_n;
    if (t_len > 1e-5) {
        t = t / t_len;
        let b = cross(geo_n, t) * tangent_w;
        let tn = textureSample(normal_tex, mat_sampler, uv).xyz * 2.0 - 1.0;
        // TBN * tangent-space normal → world space.
        n = normalize(tn.x * t + tn.y * b + tn.z * geo_n);
    }

    let view_dir = normalize(scene.camera_pos.xyz - world_position);

    // Pragmatic specular. F0 = 0.04 for dielectric, albedo for metal. Smoothness →
    // shininess exponent (rougher = broader/dimmer).
    let f0 = mix(vec3<f32>(0.04), base, metal);
    let smoothness = 1.0 - rough;
    let shininess = exp2(1.0 + smoothness * 10.0); // ~2 (rough) .. ~2048 (mirror)
    
    // ONE pass over the frame's LIGHT LIST, accumulating diffuse and specular. BOTH
    // accumulators are seeded with zero and the ambient stays OUTSIDE — which is what
    // keeps every sum's order, and so its exact f32 result, identical to the
    // sun→moon→point math this replaced. Every `textureSample` is already done ABOVE
    // this loop: sampling inside it would put a derivative in non-uniform control flow.
    var direct = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    var sheen = vec3<f32>(0.0);
    var sheen_taken = false;
    for (var i = 0u; i < scene.counts.x; i = i + 1u) {
        let li = scene.lights[i];
        let s = light_sample(li, world_position);
        let radiance = li.color_intensity.rgb * li.color_intensity.w;
        let ndl = max(dot(n, s.xyz), 0.0);
        // Shadow darkens BOTH diffuse and specular for the one light this map is cast for;
        // vis = 1.0 exactly otherwise (and for every non-shadow surface), so both sums are
        // bit-identical then.
        var vis = 1.0;
        if (shadow_uni.params.y > 0.5 && u32(shadow_uni.params.w) == i) {
            vis = shadow_factor(world_position);
        }
        direct = direct + radiance * (ndl * s.w) * vis;
        spec = spec + light_contrib(n, s.xyz, view_dir, radiance, f0, shininess) * s.w * vis;
        // The gloss sheen (flags.y) follows the FIRST non-directional light — subtle and
        // additive on top of the PBR specular.
        if (gloss > 0.001 && !sheen_taken && li.position_kind.w >= 0.5) {
            sheen_taken = true;
            let ndv = max(dot(n, view_dir), 0.0);
            let fresnel = pow(1.0 - ndv, 3.0);
            let half_vec = normalize(s.xyz + view_dir);
            let broad = pow(max(dot(n, half_vec), 0.0), 3.0);
            sheen = radiance * (0.35 * fresnel + 0.10 * broad) * ndl * gloss;
        }
    }

    // Metal has (almost) no diffuse; AO attenuates the ambient + diffuse floor.
    let diffuse_amt = (1.0 - metal);
    let ambient = scene.ambient.rgb * ao;
    let diffuse = base * (ambient + direct * diffuse_amt);
    // A small ambient specular so metal reads reflective even away from a direct highlight.
    spec = spec + f0 * ambient * smoothness;

    let shaded = diffuse + spec + sheen;
    // EMISSION is added AFTER the tint and BEFORE fog. After the tint because a
    // glow is the surface's own light — dimming the object must not dim what it
    // emits — and before fog because distance still swallows a glow like any other
    // radiance. Never multiplied by AO or a light term: nothing shadows a light.
    let lit = vec4<f32>(shaded, 1.0) * tint + vec4<f32>(emit, 0.0);

    let dist = length(world_position - scene.camera_pos.xyz);
    let fog = 1.0 - exp(-scene.fog_color.w * dist);
    let rgb = mix(lit.rgb, scene.fog_color.rgb, fog);
    // Soft mode blends by texture alpha × tint alpha; cutout/opaque mode uses tint alpha.
    let out_a = select(lit.a, texel.a * tint.a, soft);
    var out: Shaded;
    out.color = vec4<f32>(rgb, out_a);
    out.alpha = texel.a;
    return out;
}
