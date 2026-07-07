// Volumetric ground-fog raymarch — see pipeline_ground_fog.rs.
//
// A fullscreen pass: per pixel, reconstruct the world view ray and march it through a thin
// horizontal fog slab (y ∈ [bottom, top]), accumulating extinction from an animated fbm noise
// field that drifts with the wind. Because density is *integrated along the ray*, overlapping
// fog composites correctly (no billboard/quad layering artifacts) and edges fade continuously —
// nothing to spawn or wrap. Depth-aware: the ray stops at the nearest solid surface, so the fog
// is occluded by geometry in front of it. Cheap "over" composite (premultiplied alpha).

struct Fog {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>, // xyz = camera, w = edge-fade feather distance
    color: vec4<f32>,      // rgb = fog colour, w = density
    band: vec4<f32>,       // x = bottom y, y = top y, z = noise scale, w = coverage (0..1)
    wind: vec4<f32>,       // xy = drift velocity (xz), z = time, w = vertical falloff power
    bounds: vec4<f32>,     // (min_x, min_z, max_x, max_z) — localise the fog to this rect
};

@group(0) @binding(0) var<uniform> fog: Fog;
// The opaque pass's depth buffer (read-only) — clamps each ray at the nearest solid surface.
@group(0) @binding(1) var depth_tex: texture_depth_2d;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VertexOut;
    let xy = corners[vid];
    out.clip_position = vec4<f32>(xy, 0.0, 1.0);
    out.ndc = xy;
    return out;
}

// --- hash value-noise / fbm (same as volumetric.wgsl) -----------------------
fn hash13(p_in: vec3<f32>) -> f32 {
    var p3 = fract(p_in * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let c000 = hash13(i + vec3<f32>(0.0, 0.0, 0.0));
    let c100 = hash13(i + vec3<f32>(1.0, 0.0, 0.0));
    let c010 = hash13(i + vec3<f32>(0.0, 1.0, 0.0));
    let c110 = hash13(i + vec3<f32>(1.0, 1.0, 0.0));
    let c001 = hash13(i + vec3<f32>(0.0, 0.0, 1.0));
    let c101 = hash13(i + vec3<f32>(1.0, 0.0, 1.0));
    let c011 = hash13(i + vec3<f32>(0.0, 1.0, 1.0));
    let c111 = hash13(i + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(c000, c100, u.x);
    let x10 = mix(c010, c110, u.x);
    let x01 = mix(c001, c101, u.x);
    let x11 = mix(c011, c111, u.x);
    let y0 = mix(x00, x10, u.y);
    let y1 = mix(x01, x11, u.y);
    return mix(y0, y1, u.z);
}

fn fbm3(p_in: vec3<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var p = p_in;
    for (var o = 0; o < 4; o = o + 1) {
        v = v + a * vnoise(p);
        p = p * 2.02;
        a = a * 0.5;
    }
    return v;
}

// Horizontal localisation: 1 in the interior of the bounds rect, smoothly fading to 0
// across the `edge_fade` feather near/beyond its edges — so the fog stays over the field
// instead of extending to the horizon.
fn edge_fade(xz: vec2<f32>) -> f32 {
    let mn = fog.bounds.xy;
    let mx = fog.bounds.zw;
    let dist = min(min(xz.x - mn.x, mx.x - xz.x), min(xz.y - mn.y, mx.y - xz.y));
    return smoothstep(0.0, max(fog.camera_pos.w, 0.001), dist);
}

// Fog density at a world point: animated fbm × a vertical falloff (densest at the bottom)
// × the horizontal edge localisation.
fn fog_density(p: vec3<f32>) -> f32 {
    let bottom = fog.band.x;
    let top = fog.band.y;
    let scale = fog.band.z;
    let coverage = fog.band.w;
    let time = fog.wind.z;

    let h = clamp((p.y - bottom) / max(top - bottom, 0.001), 0.0, 1.0);
    let vfall = pow(1.0 - h, max(fog.wind.w, 0.25));

    // Drift the sample point by the wind so the fog streams across.
    let sp = (p - vec3<f32>(fog.wind.x * time, 0.0, fog.wind.y * time)) * scale;
    let n = fbm3(sp);
    let d = max(n - (1.0 - coverage), 0.0) / max(coverage, 0.02);
    return d * vfall * edge_fade(p.xz);
}

// t-range where the ray is inside the horizontal slab [bottom, top].
fn slab_t(ro: vec3<f32>, rd: vec3<f32>, bottom: f32, top: f32) -> vec2<f32> {
    if (abs(rd.y) < 1e-4) {
        if (ro.y >= bottom && ro.y <= top) {
            return vec2<f32>(0.0, 1e9);
        }
        return vec2<f32>(1.0, -1.0);
    }
    let ta = (bottom - ro.y) / rd.y;
    let tb = (top - ro.y) / rd.y;
    return vec2<f32>(min(ta, tb), max(ta, tb));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let far = fog.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let world = far.xyz / far.w;
    let ro = fog.camera_pos.xyz;
    let rd = normalize(world - ro);

    // Nearest opaque surface along this ray (from the depth buffer) → fog is occluded by it.
    let d = textureLoad(depth_tex, vec2<i32>(in.clip_position.xy), 0);
    var surf_dist = 1.0e9;
    if (d < 0.99999) {
        let surf = fog.inv_view_proj * vec4<f32>(in.ndc, d, 1.0);
        surf_dist = length(surf.xyz / surf.w - ro);
    }

    let bottom = fog.band.x;
    let top = fog.band.y;
    let sl = slab_t(ro, rd, bottom, top);
    let t0 = max(sl.x, 0.0);
    let t1 = min(sl.y, surf_dist);
    if (t1 <= t0) {
        return vec4<f32>(0.0);
    }

    let k = fog.color.w * 3.0;
    let steps = 28;
    let dt = (t1 - t0) / f32(steps);
    let jitter = hash13(vec3<f32>(in.ndc * 512.0, fog.wind.z));
    var t = t0 + dt * jitter;
    var trans = 1.0;
    var accum = vec3<f32>(0.0);
    let fog_col = fog.color.rgb;
    for (var s = 0; s < steps; s = s + 1) {
        let p = ro + rd * t;
        let dens = fog_density(p);
        if (dens > 0.001) {
            let sigma = dens * k;
            let dl = trans * (1.0 - exp(-sigma * dt));
            accum = accum + dl * fog_col;
            trans = trans * exp(-sigma * dt);
            if (trans < 0.004) {
                break;
            }
        }
        t = t + dt;
    }
    let alpha = 1.0 - trans;
    return vec4<f32>(accum, alpha); // premultiplied "over"
}
