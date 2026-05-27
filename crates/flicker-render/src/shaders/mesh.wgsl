// 3D mesh shader.
//
// Vertex stage: transforms `position` from cluster-local through
// `model` and the camera's `view_projection` to clip space; carries
// world-space normal and material to the fragment stage. Barycentric
// coordinates are derived from `vertex_index % 3` so non-indexed (or
// non-shared-vertex) draws get correct `(1,0,0)/(0,1,0)/(0,0,1)`
// triplets per triangle. Shared-vertex indexed meshes get approximate
// barycentrics — fine for the smoke-check cube, which duplicates
// vertices per face; voxel meshes that share vertices will produce
// approximate wireframe edges (acceptable for debug visualization).
//
// Fragment stage:
//   * solid mode (`flags.x == 0.0`): Lambertian shading on a fixed
//     directional light, modulated by a material-hashed base color
//     and the `tint` uniform.
//   * wireframe mode (`flags.x == 1.0`): output the fixed wireframe
//     color where `min(bary)` is near zero (close to an edge); discard
//     everywhere else.

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
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) material: u32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) bary: vec3<f32>,
    @location(2) @interpolate(flat) material: u32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let world = per_draw.model * vec4<f32>(in.position, 1.0);
    out.clip_position = camera.view_projection * world;
    // Treat the model as rigid (no non-uniform scale); transform normals
    // by `model`'s upper-3x3 directly. For shears or non-uniform scale
    // we'd want the inverse-transpose — not needed by the voxel
    // pipeline at this phase.
    out.world_normal = normalize((per_draw.model * vec4<f32>(in.normal, 0.0)).xyz);
    // Barycentric coordinates from vertex_index modulo 3.
    let bary_id = in.vertex_index % 3u;
    if (bary_id == 0u) {
        out.bary = vec3<f32>(1.0, 0.0, 0.0);
    } else if (bary_id == 1u) {
        out.bary = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        out.bary = vec3<f32>(0.0, 0.0, 1.0);
    }
    out.material = in.material;
    return out;
}

// Hash a material id into an HSV-style base color. This is a debug-
// visualization stand-in; real material textures slot in here later.
fn material_color(material: u32) -> vec3<f32> {
    // Mix three prime-multiplied bit fields into a hue, then convert HSV
    // (with fixed saturation/value) to RGB.
    let h_raw = f32(material) * 0.6180339887;
    let hue = fract(h_raw);
    let sat = 0.55;
    let val = 0.85;
    let c = val * sat;
    let h6 = hue * 6.0;
    let x = c * (1.0 - abs(h6 % 2.0 - 1.0));
    var rgb: vec3<f32>;
    if (h6 < 1.0) {
        rgb = vec3<f32>(c, x, 0.0);
    } else if (h6 < 2.0) {
        rgb = vec3<f32>(x, c, 0.0);
    } else if (h6 < 3.0) {
        rgb = vec3<f32>(0.0, c, x);
    } else if (h6 < 4.0) {
        rgb = vec3<f32>(0.0, x, c);
    } else if (h6 < 5.0) {
        rgb = vec3<f32>(x, 0.0, c);
    } else {
        rgb = vec3<f32>(c, 0.0, x);
    }
    let m = val - c;
    return rgb + vec3<f32>(m);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if (per_draw.flags.x > 0.5) {
        // Wireframe mode: keep fragments near a triangle edge, discard
        // everything else. `fwidth(bary)` is the absolute screen-space
        // derivative of the barycentric — i.e., how much bary changes
        // per pixel. Dividing bary by that derivative converts the
        // "barycentric distance to edge" into screen-space pixels, so
        // the threshold below (`1.5`) is a roughly 1.5-pixel-wide
        // wireframe independent of triangle size or camera distance.
        // The `max(..., 0.00001)` guard avoids `inf` on degenerate
        // fragments where the derivative collapses to zero (clipped
        // by the near plane, etc.).
        let d = fwidth(in.bary);
        let edge = in.bary / max(d, vec3<f32>(0.00001));
        let min_edge = min(edge.x, min(edge.y, edge.z));
        if (min_edge > 1.5) {
            discard;
        }
        return vec4<f32>(0.2, 0.9, 0.4, 1.0);
    }

    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
    let lambert = max(dot(in.world_normal, light_dir), 0.0);
    let base = material_color(in.material);
    let shaded = base * (0.3 + 0.7 * lambert);
    return vec4<f32>(shaded, 1.0) * per_draw.tint;
}
