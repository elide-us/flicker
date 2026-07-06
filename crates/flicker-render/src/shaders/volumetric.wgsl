// Volumetric protoplanetary-disk raymarch — see pipeline_volumetric.rs.
//
// A fullscreen pass: per pixel, reconstruct the world view ray and march it through a 3D
// dust-density field (a flared, **domain-warped, billowing** disk — blobby, not a pancake),
// accumulating extinction + scattered starlight. The central star is rendered **inside** the
// shader so the dust **occludes** it, and the in-scatter is shadowed toward the star, so light
// streams through the gaps as **god rays**. Cinematic over realistic.

struct Disk {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    geom: vec4<f32>,   // inner, outer, snow_line, scale_height
    state: vec4<f32>,  // density, formation, time, body_count
    tint: vec4<f32>,   // dark dust tint (rgb)
    glow: vec4<f32>,   // warm star-lit colour (rgb)
    gaps: array<vec4<f32>, 32>, // (orbital_radius, gap_width, _, _) — annular gaps
};

@group(0) @binding(0) var<uniform> disk: Disk;
// The opaque pass's depth buffer (read-only). Lets the cloud stop each ray at the nearest solid
// body — so bodies sit *inside* the dust (front dust hazes them, back dust is occluded) and a body
// in front of the star occludes it, instead of the cloud/star always drawing behind everything.
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

// --- hash value-noise / fbm -------------------------------------------------
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

fn fbm2(p_in: vec3<f32>) -> f32 {
    return 0.6 * vnoise(p_in) + 0.3 * vnoise(p_in * 2.03);
}

fn fbm3(p_in: vec3<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var p = p_in;
    for (var o = 0; o < 3; o = o + 1) {
        v = v + a * vnoise(p);
        p = p * 2.02;
        a = a * 0.5;
    }
    return v;
}

fn rot_y(p: vec3<f32>, ang: f32) -> vec3<f32> {
    let c = cos(ang);
    let s = sin(ang);
    return vec3<f32>(c * p.x + s * p.z, p.y, -s * p.x + c * p.z);
}

// Shared radial/dissipation/gap profile (no turbulence) — the disk's smooth backbone.
fn radial_profile(r: f32) -> f32 {
    let inner = disk.geom.x;
    let outer = disk.geom.y;
    let snow = disk.geom.z;
    let formation = disk.state.y;
    if (r > outer || r < inner * 0.25) {
        return 0.0;
    }
    let ramp_in = smoothstep(inner * 0.4, inner * 1.5, r);
    let falloff = pow(inner / max(r, inner * 0.5), 0.45);
    let fade_out = 1.0 - smoothstep(outer * 0.7, outer, r);
    var radial = ramp_in * falloff * fade_out;
    let snow_d = (r - snow) / (0.5 * snow);
    radial = radial * (1.0 + 0.8 * exp(-snow_d * snow_d));
    // Inside-out dissipation.
    let r_clear = inner + (outer - inner) * pow(formation, 0.7);
    radial = radial * smoothstep(r_clear * 0.5, r_clear * 1.15, r);
    radial = radial * (1.0 - smoothstep(0.78, 1.0, formation));
    // Annular gaps at the giants' orbits.
    let n = i32(disk.state.w);
    for (var i = 0; i < n; i = i + 1) {
        let g = disk.gaps[i];
        let gd = (r - g.x) / max(g.y, 0.05);
        radial = radial * (1.0 - 0.9 * exp(-gd * gd));
    }
    return max(radial, 0.0);
}

// Cheap density for shadow marches: backbone × a smooth vertical Gaussian (no turbulence).
fn density_coarse(p: vec3<f32>) -> f32 {
    let r = length(p.xz);
    let radial = radial_profile(r);
    if (radial <= 0.0) {
        return 0.0;
    }
    let h = max(disk.geom.w * r, 0.02);
    let yz = p.y / h;
    return radial * exp(-yz * yz);
}

// Full density: domain-warped, billowing, high-contrast — blobby clouds, not a pancake.
fn density(p: vec3<f32>) -> f32 {
    let inner = disk.geom.x;
    let sh = disk.geom.w;
    let time = disk.state.z;
    let r = length(p.xz);
    let radial = radial_profile(r);
    if (radial <= 0.0) {
        return 0.0;
    }

    // Swirl (inner faster) + domain warp + slow time ripple → organic, bubbling structure.
    let ang = time * 0.5 * pow(inner / max(r, inner), 0.5);
    let s = 0.75 / max(inner, 0.1);
    let np0 = rot_y(p, ang) * s + vec3<f32>(0.0, time * 0.25, 0.0);
    let warp = (fbm2(np0 + vec3<f32>(4.7, 1.3, 8.2)) - 0.5) * 2.4;
    let turb = fbm3(np0 + vec3<f32>(warp, 0.0, warp));
    let cloud = pow(clamp(turb, 0.0, 1.0), 1.7) * 1.9 + 0.04; // wispy filaments + voids

    // Billowing thickness: vary the scale height by a coarse noise so the disk bulges out of
    // its plane into blobs — these are the chunks that occlude the star edge-on.
    let vbump = mix(0.45, 1.8, fbm2(np0 * 0.6 + vec3<f32>(2.0, 5.0, 1.0)));
    let h = max(sh * r * vbump, 0.02);
    let yz = p.y / h;
    let vert = exp(-yz * yz);

    return max(radial * vert * cloud, 0.0);
}

// Transmittance from `p` toward the star (origin) — short coarse march → god-ray shadows.
fn shadow_to_star(p: vec3<f32>, k: f32) -> f32 {
    let dist = length(p);
    if (dist < 1e-3) {
        return 1.0;
    }
    let dir = -p / dist;
    let span = min(dist, disk.geom.y * 0.6);
    let sd = span / 4.0;
    var tau = 0.0;
    for (var i = 1; i <= 4; i = i + 1) {
        tau = tau + density_coarse(p + dir * (sd * f32(i))) * k * sd;
    }
    return exp(-tau);
}

fn slab_range(ro: vec3<f32>, rd: vec3<f32>, hmax: f32) -> vec2<f32> {
    if (abs(rd.y) < 1e-4) {
        if (abs(ro.y) < hmax) {
            return vec2<f32>(0.0, 1e9);
        }
        return vec2<f32>(1.0, -1.0);
    }
    let t1 = (hmax - ro.y) / rd.y;
    let t2 = (-hmax - ro.y) / rd.y;
    return vec2<f32>(min(t1, t2), max(t1, t2));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let far = disk.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let world = far.xyz / far.w;
    let ro = disk.camera_pos.xyz;
    let rd = normalize(world - ro);

    // Distance to the nearest opaque body along this ray, from the depth buffer. `d >= 1` is the
    // cleared far plane (no body) → treat as infinite. Reconstruct the surface's world position
    // from (ndc, d) and measure along the ray, so the cloud can clamp to it.
    let d = textureLoad(depth_tex, vec2<i32>(in.clip_position.xy), 0);
    var surf_dist = 1.0e9;
    if (d < 0.99999) {
        let surf = disk.inv_view_proj * vec4<f32>(in.ndc, d, 1.0);
        surf_dist = length(surf.xyz / surf.w - ro);
    }

    let inner = disk.geom.x;
    let outer = disk.geom.y;
    let sh = disk.geom.w;
    let hmax = sh * outer * 4.0; // a bit taller — the billows bulge out of the plane
    let k = disk.state.x * 2.6;

    var trans = 1.0;
    var inscatter = vec3<f32>(0.0);

    let sl = slab_range(ro, rd, hmax);
    let t0 = max(sl.x, 0.0);
    // Clamp the march to the nearest body: dust behind a body is occluded by it.
    let t1 = min(min(sl.y, outer * 4.0), surf_dist);
    if (t1 > t0) {
        let STEPS = 44;
        let dt = (t1 - t0) / f32(STEPS);
        let jitter = hash13(vec3<f32>(in.ndc * 512.0, disk.state.z));
        var t = t0 + dt * jitter;
        for (var step = 0; step < STEPS; step = step + 1) {
            let p = ro + rd * t;
            let dens = density(p);
            if (dens > 0.001) {
                let r = max(length(p.xz), inner * 0.5);
                let warmth = clamp(inner * 1.6 / r, 0.0, 1.0);
                let scatter = mix(disk.tint.rgb, disk.glow.rgb, warmth);
                // Shadow toward the star → the in-scatter breaks into shafts (god rays).
                let shadow = shadow_to_star(p, k);
                let lit = scatter * (0.05 + 2.3 * pow(inner / r, 1.3)) * (0.22 + 1.0 * shadow);
                let sigma = dens * k;
                let dl = trans * (1.0 - exp(-sigma * dt));
                inscatter = inscatter + dl * lit;
                trans = trans * exp(-sigma * dt);
            }
            t = t + dt;
            if (trans < 0.003) {
                break;
            }
        }
    }

    // The central star (at the origin), rendered here so the dust **occludes** it: a hot bright
    // core + bloom, dimmed by `trans` (the dust this ray passed through). A solid body in front of
    // the star also occludes it — so a planet transiting the star eclipses it.
    let star_dir = normalize(-ro);
    let align = max(dot(rd, star_dir), 0.0);
    let core = pow(align, 7000.0) * 2.6 + pow(align, 220.0) * 0.5 + pow(align, 26.0) * 0.10;
    let star_col = vec3<f32>(1.0, 0.93, 0.80);
    let star_vis = select(1.0, 0.0, surf_dist < length(ro)); // body nearer than the star → eclipse
    inscatter = inscatter + star_col * core * trans * star_vis;

    let alpha = 1.0 - trans;
    return vec4<f32>(inscatter, alpha);
}
