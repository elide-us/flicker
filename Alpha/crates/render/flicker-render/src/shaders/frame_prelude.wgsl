// ---- frame prelude (shared: mesh / mesh_textured / skinned) ----
// The ONE text every lit shader shares. WGSL has no `#include`, so each lit pipeline
// PREPENDS this file to its body shader at module build (`compose_lit` in
// `pipeline_mesh.rs`); the gate `the_frame_prelude_is_one_text` proves this file carries
// the contract and that no body re-declares it. `lines.wgsl` reads no light and does NOT
// use this prelude.
//
// Mirrored CPU-side by `LightUniform` / `SceneUniform` in `pipeline_mesh.rs` and
// `ShadowUniform` in `pipeline_shadow.rs`; every member is a 16-byte `vec4` lane so std140
// alignment is trivially correct and `array<Light, 8>` needs no inter-element padding.
// Field ORDER is the contract with those Rust structs. No colour-grade lane rides here:
// the grade is pass-owned by tonemap_grade.wgsl, the ONE representation of it.
//
// The shadow bindings themselves (`shadow_uni` / `shadow_tex` / `shadow_samp`) are declared
// PER-SHADER, at each pipeline's own free group (mesh/skinned `@group(2)`, mesh_textured
// `@group(3)` — its `@group(2)` is the material set), exactly as the `@group(0)` camera /
// scene bindings are per-shader. `shadow_factor` references them by name (module-scope
// forward reference), so this one text serves all three whatever group each binds them at.
struct Light {
    color_intensity: vec4<f32>,  // rgb = colour, w = intensity (driver already applied)
    position_kind: vec4<f32>,    // xyz = world position, w = kind (0 dir, 1 point, 2 spot)
    direction_radius: vec4<f32>, // xyz = toward-light (dir) / cone axis (spot), w = radius
    cone: vec4<f32>,             // x = cos(inner), y = cos(outer); zw reserved
};

struct Scene {
    ambient: vec4<f32>,    // rgb = ambient floor; w unused
    camera_pos: vec4<f32>, // xyz = world camera position (fog distance + view vector)
    fog_color: vec4<f32>,  // rgb = fog colour; w = fog density
    counts: vec4<u32>,     // x = how many of `lights` are lit; yzw reserved
    lights: array<Light, 8>,
};

// The sun/light shadow map's params. `light_view_proj` is the ONE matrix the producer
// stage rendered the casters with; `params` = (bias, enabled, texel_size, light_index).
// `enabled = 0` (the default bound for every non-shadow surface) makes `shadow_factor`
// return exactly 1.0, so the lit output is byte-identical to the no-shadow path.
struct ShadowUniform {
    light_view_proj: mat4x4<f32>,
    params: vec4<f32>,
};

// One light's geometry at a world point: `xyz` = the unit vector TOWARD the light,
// `w` = attenuation (distance falloff × cone). A directional light returns the literal
// 1.0, and a point light with no authored radius returns the literal 1.0 too — which is
// what makes the list bit-for-bit identical to the sun/moon/point math it replaced.
fn light_sample(li: Light, wp: vec3<f32>) -> vec4<f32> {
    let kind = li.position_kind.w;
    if (kind < 0.5) {
        return vec4<f32>(li.direction_radius.xyz, 1.0);
    }
    let to_l = li.position_kind.xyz - wp;
    let l = to_l / max(length(to_l), 1e-4);
    var atten = 1.0;
    let r = li.direction_radius.w;
    if (r > 0.0) {
        // Karis (UE4, 2013) windowed inverse square: physical 1/d² inside the radius,
        // smoothly windowed to exactly zero at it, so a light has finite reach.
        let d2 = dot(to_l, to_l);
        let w = saturate(1.0 - (d2 * d2) / (r * r * r * r));
        atten = (w * w) / (d2 + 1.0);
    }
    if (kind > 1.5) {
        let cd = dot(li.direction_radius.xyz, -l);
        atten = atten * smoothstep(li.cone.y, li.cone.x, cd);
    }
    return vec4<f32>(l, atten);
}

// The shadow visibility of `world_pos` for the light the shadow is cast for: 1.0 = fully
// lit, 0.0 = fully occluded. Transforms the point by the light's view-projection, does the
// perspective divide + NDC→UV, biases the reference depth, and averages a 3×3 PCF kernel
// ("PCF3") of hardware comparisons. Guarded by `params.y` (enabled): a disabled shadow, a
// point behind the light, or a point outside the map all return 1.0, so nothing outside the
// shadow's footprint is darkened. Sampled with `textureSampleCompareLevel` (level 0, no
// derivatives) so it is valid inside the light loop's non-uniform control flow.
fn shadow_factor(world_pos: vec3<f32>) -> f32 {
    if (shadow_uni.params.y < 0.5) {
        return 1.0;
    }
    let clip = shadow_uni.light_view_proj * vec4<f32>(world_pos, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let reference = ndc.z - shadow_uni.params.x;
    let texel = shadow_uni.params.z;
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSampleCompareLevel(shadow_tex, shadow_samp, uv + offset, reference);
        }
    }
    return sum / 9.0;
}
// ---- end frame prelude ----
