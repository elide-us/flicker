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
//   * solid mode (`flags.x == 0.0`): Lambertian shading from the frame's
//     LIGHT LIST over a flat ambient, all driven by the frame-global
//     `Scene` uniform (`Renderer::set_scene`); the
//     base color is resolved from the packed material — primary id in
//     bits 0-7, secondary id in 8-15, blend factor in 16-23 (bit 31 =
//     direct-RGB escape) — by indexing a small color table and `mix`ing
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
    // flags.x: 1.0 = wireframe mode, 0.0 = filled. flags.y: gloss (sheen strength).
    flags: vec4<f32>,
};

// The frame prelude (struct Light / Scene / ShadowUniform / light_sample / shadow_factor)
// is PREPENDED from `shaders/frame_prelude.wgsl` at module build — the ONE shared text, not
// a copy pasted here. See that file and `compose_lit` in `pipeline_mesh.rs`.

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> scene: Scene;

@group(1) @binding(0) var<uniform> per_draw: PerDraw;
// One colour per material-catalog slot (materials.json id = index). Booted
// all-magenta by the pipeline; `Renderer::set_material_palette` uploads the
// catalog colours, so an undefined id stays visibly "missing".
@group(1) @binding(1) var<uniform> palette: array<vec4<f32>, 256>;

// The sun/light shadow map (group 2 is free for this pipeline). The prelude's
// `shadow_factor` reads these by name; the default bound for a non-shadow surface has
// `enabled = 0`, so it returns 1.0 and this shader is byte-identical to the no-shadow path.
@group(2) @binding(0) var<uniform> shadow_uni: ShadowUniform;
@group(2) @binding(1) var shadow_tex: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

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

// Look up a base color for a single material id — one read from the
// material-catalog palette (materials.json colours, uploaded by
// `Renderer::set_material_palette`). Undefined ids (and an unset palette)
// read the boot magenta, so "missing" stays visible.
fn material_index_color(index: u32) -> vec3<f32> {
    return palette[index & 0xFFu].rgb;
}

// Resolve a packed material to a color. Two encodings (the u8 catalog layout,
// 2026-08-19 — matches flicker-voxel `Material`):
//   * Direct RGB (escape): bit 31 set marks an RGB888 colour in bits 0-23
//     (R 0-7, G 8-15, B 16-23) — for continuous data maps the palette can't
//     express. Costs no material id; catalog words never set the top byte.
//   * Palette blend (default): primary id in bits 0-7, secondary id in 8-15,
//     blend in 16-23 — linear interpolation between two palette colours.
fn material_color(material: u32) -> vec3<f32> {
    if ((material & 0x80000000u) != 0u) {
        let r = f32(material & 0xFFu) / 255.0;
        let g = f32((material >> 8u) & 0xFFu) / 255.0;
        let b = f32((material >> 16u) & 0xFFu) / 255.0;
        return vec3<f32>(r, g, b);
    }
    let primary = material & 0xFFu;
    let secondary = (material >> 8u) & 0xFFu;
    let blend = f32((material >> 16u) & 0xFFu) / 255.0;
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
    // EMISSIVE direct colour (bit 30 inside the bit-31 direct escape): the
    // surface GLOWS — no Lambert, no sheen, slightly over unit so it reads
    // hot against every lit neighbour (and survives an HDR roll-off as a
    // hot spot where a stage tonemaps).
    if ((in.material & 0xC0000000u) == 0xC0000000u) {
        let glow = vec4<f32>(base * 1.35, 1.0) * per_draw.tint;
        let gdist = length(in.world_position - scene.camera_pos.xyz);
        let gfog = 1.0 - exp(-scene.fog_color.w * gdist);
        return vec4<f32>(mix(glow.rgb, scene.fog_color.rgb, gfog), glow.a);
    }
    // The frame's LIGHT LIST over a flat ambient floor. Each light contributes a matte
    // Lambertian term; a light below the horizon fades by carrying a near-zero colour
    // from the scene-side day-arc math, so no explicit night branch is needed. The
    // accumulator is SEEDED with ambient, which is what keeps the sum's order — and so
    // its exact f32 result — identical to the ambient+sun+moon+point it replaced.
    // Liquid/icy **sheen** (per-draw gloss = `flags.y`) follows the FIRST non-directional
    // light. NOT a mirror specular — a tight hot-spot reads as a marble, wrong at planet
    // scale. Instead the wet cue is a soft **limb sheen**: brightest where the view grazes
    // the surface (Fresnel, the planet's lit edge), with only a faint, broad sunward lift —
    // an ocean/atmosphere look, no bright reflection dot. Lit side only, scaled by gloss;
    // matte surfaces (gloss 0) skip it and read exactly as before.
    let gloss = per_draw.flags.y;
    var diffuse = scene.ambient.rgb;
    var sheen = vec3<f32>(0.0);
    var sheen_taken = false;
    for (var i = 0u; i < scene.counts.x; i = i + 1u) {
        let li = scene.lights[i];
        let s = light_sample(li, in.world_position);
        let radiance = li.color_intensity.rgb * li.color_intensity.w;
        let ndl = max(dot(in.world_normal, s.xyz), 0.0);
        // Shadow: darken ONLY the light this map is cast for; every other light, and the
        // whole surface when no shadow is bound (enabled = 0), keeps vis = 1.0 exactly, so
        // the term is bit-identical to the unshadowed `radiance * (ndl * s.w)`.
        var vis = 1.0;
        if (shadow_uni.params.y > 0.5 && u32(shadow_uni.params.w) == i) {
            vis = shadow_factor(in.world_position);
        }
        diffuse = diffuse + radiance * (ndl * s.w) * vis;
        if (gloss > 0.001 && !sheen_taken && li.position_kind.w >= 0.5) {
            sheen_taken = true;
            let view_dir = normalize(scene.camera_pos.xyz - in.world_position);
            let ndv = max(dot(in.world_normal, view_dir), 0.0);
            let fresnel = pow(1.0 - ndv, 3.0); // grazing-angle limb brightening
            let half_vec = normalize(s.xyz + view_dir);
            let broad = pow(max(dot(in.world_normal, half_vec), 0.0), 3.0); // very broad, faint
            sheen = radiance * (0.35 * fresnel + 0.10 * broad) * ndl * gloss;
        }
    }
    let shaded = base * diffuse + sheen;
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
