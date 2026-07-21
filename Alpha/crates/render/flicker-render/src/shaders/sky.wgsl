// Procedural sky — a fullscreen pass that fakes atmospheric scattering.
//
// Drawn first in the main pass (before the 3D mesh), covering the whole
// screen with a view-direction-dependent gradient: a horizon→zenith band
// that brightens and warms toward the sun (a faked wide Rayleigh wash + a
// tight Mie forward-scatter core) and a cooler, dimmer glow toward the moon
// at night. No skybox, no texture — the entire look comes from the `Sky`
// uniform the renderer fills from the day/night cycle each frame.
//
// The vertex stage emits a single oversized triangle (no vertex buffer); the
// fragment stage reconstructs the world-space view ray from the inverse
// view-projection and the camera position, then shades it. Because the same
// inverse view-projection drives both this and the mesh transform, the sun's
// glow lands in the sky exactly where the mesh shading says the sun is.
//
// On top of the gradient + glows it draws two celestial discs: a flat bright
// sun disc and a moon disc with an analytic phase terminator (the visible
// sphere point's normal lit by the sun). The moon is composited last and sized
// equal to the sun, so when the sliders bring them into alignment the moon's
// (then-dark) silhouette eclipses the sun, leaving a corona ring — "The Advent".

struct Sky {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,   // xyz world camera position; w unused
    sun_dir: vec4<f32>,      // xyz toward the sun (normalized); w unused
    sun_color: vec4<f32>,    // sun radiance — fades to ~0 below horizon / at night
    moon_dir: vec4<f32>,     // xyz toward the moon (normalized); w unused
    moon_color: vec4<f32>,   // moon radiance — cool, dim, peaks at full moon
    zenith: vec4<f32>,       // sky colour straight up; w unused
    horizon: vec4<f32>,      // sky colour at the horizon band; w unused
    star_rotation: mat4x4<f32>, // world→celestial: rotates the star field with time + latitude
};

// Hash a 3D cell to [0,1) (Dave Hoskins, hash13). Deterministic, no texture.
fn hash13(p3in: vec3<f32>) -> f32 {
    var p3 = fract(p3in * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Procedural star field over a celestial direction. Bins the sphere into a fine
// cell grid; a fraction of cells hold a star (more inside the Milky Way band,
// via `density`), drawn as a soft round point with a hashed brightness.
fn star_field(cel: vec3<f32>, density: f32) -> f32 {
    let scale = 240.0;
    let p = cel * scale;
    let cell = floor(p);
    let present = hash13(cell);
    let threshold = 0.018 + density * 0.10;
    if (present > threshold) {
        return 0.0;
    }
    // Sub-cell position + brightness, each from a decorrelated hash.
    let sp = vec3<f32>(hash13(cell + 11.5), hash13(cell + 23.7), hash13(cell + 37.1));
    let d = length(fract(p) - sp);
    let core = smoothstep(0.18, 0.0, d);
    let bright = 0.25 + 0.75 * hash13(cell + 5.3);
    return core * bright;
}

@group(0) @binding(0) var<uniform> sky: Sky;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOut {
    // Fullscreen triangle: (-1,-1), (3,-1), (-1,3) — its inscribed quad
    // covers the whole [-1,1] viewport with a single primitive.
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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Reconstruct the world-space view ray: unproject this pixel at the far
    // plane (clip z = 1.0 under wgpu's [0,1] depth) and subtract the eye.
    let far = sky.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let world = far.xyz / far.w;
    let dir = normalize(world - sky.camera_pos.xyz);

    let up = clamp(dir.y, -1.0, 1.0); // -1 down .. +1 straight up
    let h = clamp(up, 0.0, 1.0);

    // Vertical gradient, compressed toward the horizon so the bright band is
    // thin (the longest scattering path) and the zenith dominates overhead.
    let grad = pow(h, 0.5);
    var col = mix(sky.horizon.rgb, sky.zenith.rgb, grad);

    // Below the horizon, sink to a dark ground haze (mostly hidden by terrain).
    let below = clamp(-up * 4.0, 0.0, 1.0);
    col = mix(col, sky.horizon.rgb * 0.30, below);

    // Night sky: a star scattering + a Milky Way band, rotated into the
    // sky-fixed frame so they wheel with time of day and tilt with latitude.
    // They rise as the sun sets (`night`) and are painted over by the
    // sun/moon glow + discs drawn afterwards.
    let night = 1.0 - smoothstep(-0.12, 0.06, sky.sun_dir.y);
    if (night > 0.002) {
        let cel = normalize((sky.star_rotation * vec4<f32>(dir, 0.0)).xyz);
        // Galactic plane: a fixed great circle in celestial space; the band is
        // the region near-perpendicular to its pole.
        let g_pole = normalize(vec3<f32>(0.20, 0.46, 0.86));
        let band = 1.0 - smoothstep(0.0, 0.30, abs(dot(cel, g_pole)));
        // Faint dusty nebulosity along the band, mottled by a coarse hash.
        let milky = pow(band, 2.0) * (0.5 + 0.5 * hash13(floor(cel * 60.0)));
        col += vec3<f32>(0.34, 0.40, 0.56) * (milky * 0.05 * night);
        // Star scatter — denser inside the Milky Way band.
        let stars = star_field(cel, band);
        col += vec3<f32>(0.85, 0.90, 1.0) * (stars * night);
    }

    // Faked sun scattering: a wide, dim Rayleigh wash (reads as sky glow, not
    // body) + a tight Mie core that hugs the disc, so the sun's *body* reads at
    // its disc size instead of ballooning to ~2× — both tinted by the cycle's
    // sun colour, so the glow warms at dawn/dusk and vanishes once it sets.
    let sd = max(dot(dir, sky.sun_dir.xyz), 0.0);
    let sun_glow = pow(sd, 8.0) * 0.30 + pow(sd, 1600.0) * 1.0;
    col += sky.sun_color.rgb * sun_glow;

    // Cooler, dimmer moon glow — visible mainly at night, when the sun colour
    // has faded to black and the moon colour carries the phase brightness.
    let md = max(dot(dir, sky.moon_dir.xyz), 0.0);
    let moon_glow = pow(md, 24.0) * 0.3 + pow(md, 900.0) * 1.1;
    col += sky.moon_color.rgb * moon_glow;

    // --- Celestial discs ---------------------------------------------------
    // A sun disc and a phase-shaded moon disc, the moon painted last so it
    // eclipses the sun when they align. The moon is a touch larger than the sun
    // (as in a real total eclipse) so it cleanly swallows it, and so the two
    // bodies read as comparable rather than the sun's bloom dwarfing the moon.
    let sun_r = 0.038;   // sun angular radius (rad) — stylized, ~10× real
    let moon_r = 0.047; // moon a hair larger (matches the sun's bloomed body)
    let edge = 0.004;   // soft-edge width for an anti-aliased rim
    let sr = sin(moon_r);

    let sun_ang = acos(clamp(dot(dir, sky.sun_dir.xyz), -1.0, 1.0));
    let moon_ang = acos(clamp(dot(dir, sky.moon_dir.xyz), -1.0, 1.0));

    // Eclipse strength = how much the moon covers the sun (disc overlap), gated
    // to the sun being up. Computed once here: it keeps the daytime moon solid
    // while it's eclipsing and drives the corona below.
    let align = max(dot(sky.sun_dir.xyz, sky.moon_dir.xyz), 0.0);
    let separation = acos(clamp(align, -1.0, 1.0));
    let coverage = 1.0 - smoothstep(moon_r - sun_r, moon_r + sun_r, separation);
    let eclipse = coverage * smoothstep(-0.02, 0.02, sky.sun_dir.y);

    // Sun disc: a flat bright disc tinted by the cycle's sun colour, so it
    // warms at dusk and is simply gone at night. Fades out below the horizon.
    let sun_mask = (1.0 - smoothstep(sun_r - edge, sun_r + edge, sun_ang))
        * smoothstep(-0.02, 0.02, sky.sun_dir.y);
    let sun_disc = sky.sun_color.rgb * 3.5;

    // Moon disc: reconstruct the visible sphere point's outward normal and
    // light it by the sun for a real phase terminator. Cool-pale where lit,
    // near-black where shadowed — at new moon the whole near face is dark,
    // which is exactly the silhouette that eclipses the sun. A stable tangent
    // frame (swapping the up reference near the zenith) spans the disc.
    let mc = sky.moon_dir.xyz;
    let m_up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(mc.y) > 0.99);
    let m_right = normalize(cross(m_up, mc));
    let m_top = cross(mc, m_right);
    let mu = dot(dir, m_right) / sr;
    let mv = dot(dir, m_top) / sr;
    let mz = sqrt(max(0.0, 1.0 - mu * mu - mv * mv));
    let m_normal = m_right * mu + m_top * mv - mc * mz; // outward; −mc at the centre
    let lit = smoothstep(-0.08, 0.08, dot(m_normal, sky.sun_dir.xyz));
    let moon_disc = mix(vec3<f32>(0.02, 0.025, 0.045), vec3<f32>(0.60, 0.65, 0.74), lit);
    let moon_geom = (1.0 - smoothstep(moon_r - edge, moon_r + edge, moon_ang))
        * smoothstep(-0.05, 0.02, mc.y);
    // Wash the shadow limb into the local sky so the unlit side never reads as
    // a hard disc whose black mismatches the sky behind it. Only the lit sliver
    // stays solid. Exception: where the moon covers the sun it must stay opaque
    // to eclipse it (`1 - sun_mask`).
    let wash = (1.0 - lit) * (1.0 - sun_mask);
    // The daytime moon is faint — the bright sky overwhelms it — so fade the
    // whole disc as the day brightens. Held solid while it's eclipsing the sun
    // (`1 - eclipse`) and returns in full at night.
    let day_hide = smoothstep(0.0, 0.25, sky.sun_dir.y) * (1.0 - eclipse);
    let moon_mask = moon_geom * (1.0 - wash) * (1.0 - day_hide * 0.85);

    // Sun first, then the moon over it — the moon's silhouette occludes the sun.
    col = mix(col, sun_disc, sun_mask);
    col = mix(col, moon_disc, moon_mask);

    // Eclipse corona: a bright ring hugging just outside the moon's rim — the
    // dramatic part. Uses the `eclipse` coverage from above (the same the
    // terrain darkens by, so sky and ground peak together). A fixed white base
    // keeps the ring blazing once `sun_color` has dimmed out under the moon.
    let outside = smoothstep(moon_r - edge, moon_r + edge, moon_ang);
    let rim = exp(-pow((moon_ang - moon_r) / 0.012, 2.0)) * outside;
    col += (sky.sun_color.rgb + vec3<f32>(0.6)) * (rim * eclipse * 1.8);

    return vec4<f32>(col, 1.0);
}
