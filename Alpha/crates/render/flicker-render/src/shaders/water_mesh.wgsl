// Animated water-surface MESH — see pipeline_water_mesh.rs.
//
// A PROJECTED-GRID ocean (Claes Johanson): the uploaded unit grid is read as SCREEN space and
// cast from the camera onto the sea plane, so the mesh always tiles exactly the visible water
// out to the horizon with screen-uniform density — dense near the camera, coarse far away (a
// built-in LOD at a FIXED vertex count). The VERTEX stage unprojects each grid vertex through
// the camera inverse (`water.inv_view_proj`) onto `y = sea_level`, then lifts that world point
// to `y = sea_level + Σ waves` and recomputes the surface normal ANALYTICALLY from the summed
// wave derivatives. ONE roster carries both wave kinds: RADIAL sources (rings from a centre —
// the near-island chop, attenuated hard toward flat with distance) and DIRECTIONAL ones (plane
// waves marching along a world direction, defined everywhere and attenuated far more gently, so
// the open-ocean band at the horizon keeps swelling instead of reading as glass);
// the FRAGMENT stage shades an
// ENVIRONMENT-LIT water body — the rig's analytic sky mirrored along the reflected view ray,
// Fresnel-blended over a shallow→deep ramp lit by the rig's ambient + the SKY SLOTS' diffuse —
// plus a REAL specular summed over those same two slots of the frame's light list (slot 0 = the
// sun, slot 1 = the moon), so the day sea carries a sun glint and the night sea a moon streak
// from ONE lobe and ONE knob set. The sky palette is the LIVE one, uploaded into this pass's
// own @group(1) uniform from the renderer's LightRig (the shared `Scene` carries no sky lanes,
// so water is where it has to ride); the ambient floor is read straight off the SHARED
// `scene.ambient` every lit mesh uses, never copied — so the sea follows the day/night cycle
// with nothing authored per time of day and no second spelling of the same number.
// Because it is real geometry it writes depth and is depth-tested (occludes and is occluded),
// and it composites premultiplied "over" the lit scene so shallow water is translucent.
//
// The shared frame prelude (struct Light / Scene / ShadowUniform / light_sample /
// shadow_factor) is PREPENDED from `shaders/frame_prelude.wgsl` at module build (compose_lit
// in pipeline_mesh.rs), exactly as the lit mesh shaders do — that is where `Scene`,
// `light_sample`, and `scene.camera_pos` / `scene.lights[i]` come from. Water reads the sky
// slots through it; it never calls `shadow_factor`, but the prelude declares it, so the shadow
// bindings below exist only to satisfy that reference (a disabled default is bound).

struct Camera {
    view_projection: mat4x4<f32>,
};

// One wave source: `a = (x, y, amplitude, k)`, `b = (omega, phase, kind, _)`, with
// `k = 2π/wavelength` and `omega = speed·k`. The leading XY lane carries the source's GEOMETRY,
// read two ways by the `kind` flag (`b.z`) — ONE lane, never a parallel array:
//   kind 0 = RADIAL      — `a.xy` is the world-XZ CENTRE the rings spread from; height at a
//                          point is `amplitude · sin(k·distance_to_center − omega·time + phase)`.
//   kind 1 = DIRECTIONAL — `a.xy` is the UNIT XZ direction the crests march along (normalized
//                          at parse); height is `amplitude · sin(k·dot(p.xz, dir) − omega·time
//                          + phase)`, a plane wave with no centre, so it is defined EVERYWHERE.
struct Wave {
    a: vec4<f32>,
    b: vec4<f32>,
};

struct Water {
    // Camera INVERSE view-projection — unprojects the screen-space grid onto the sea plane (the
    // projected-grid technique). The RE-projection uses the shared @group(0) camera, so this is
    // the only camera matrix water owns; the shared CameraUniform is untouched.
    inv_view_proj: mat4x4<f32>,
    // xyz = world camera position (the projected-grid ray origin). Carried here because the
    // shared @group(0) `scene` uniform is FRAGMENT-only in the frame layout, so the VERTEX stage
    // cannot read `scene.camera_pos` — the fog/volumetric passes carry it the same way.
    camera_pos: vec4<f32>,
    // (sea_level, shore_fade, spec_shininess, spec_strength)
    params0: vec4<f32>,
    // (time, normal_scale, wave_falloff, env_strength)
    params1: vec4<f32>,
    // rgb = shallow water colour (grazing view); w unused.
    shallow: vec4<f32>,
    // rgb = deep water colour (looking straight down); w unused.
    deep: vec4<f32>,
    // rgb = the LIVE rig's sky colour straight up; w unused. Uploaded from `LightRig::sky_zenith`
    // — the SAME field sky.wgsl gradients — so the sea mirrors the actual sky, not a copy of it.
    sky_zenith: vec4<f32>,
    // rgb = the LIVE rig's sky colour at the horizon band; w unused.
    //
    // There is deliberately NO `ambient` lane here. The ambient floor the body is lit by is
    // the SHARED frame one — `scene.ambient`, bound at @group(0) below and filled from the very
    // same `LightRig` — so carrying a second copy in this uniform would be a bit-for-bit
    // duplicate with its own drift risk. The sky palette DOES ride here because `Scene` has no
    // sky lanes: those two have no shared home to read from.
    sky_horizon: vec4<f32>,
    // x = how many of `waves` are live.
    counts: vec4<u32>,
    // The roster length is `MAX_WAVE_SOURCES` (pipeline_water_mesh.rs); the wgsl gate asserts
    // this literal still equals that constant, so the two can never drift apart.
    waves: array<Wave, 6>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> scene: Scene;

@group(1) @binding(0) var<uniform> water: Water;

// group(2): the shared shadow bindings — declared ONLY so the prelude's `shadow_factor`
// resolves (water never calls it). The renderer binds the 1×1 `enabled = 0` default here.
@group(2) @binding(0) var<uniform> shadow_uni: ShadowUniform;
@group(2) @binding(1) var shadow_tex: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

struct VertexIn {
    @location(0) position: vec3<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
};

// Screen-space overscan: the unit grid is read as NDC covering slightly MORE than the screen
// ([-1,1] × this), so the projected edges sit just off-screen and never reveal a grid seam.
const WATER_OVERSCAN: f32 = 1.06;

// How much of `wave_falloff` a DIRECTIONAL (ambient, open-ocean) source feels. Radial sources
// are flattened hard with distance so the projected grid's coarse far rows do not shimmer; a
// directional swell is authored LONG (wavelength ~120-200) so it has no sub-pixel detail to
// alias, and it is the only thing moving out at the horizon — flatten it at the same rate and
// the far sea goes back to being a dead glass mirror. 0.15 keeps the horizon band swelling
// while still letting `wave_falloff` govern BOTH kinds from one authored dial.
const WATER_AMBIENT_FALLOFF_SCALE: f32 = 0.15;

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let sea_level = water.params0.x;
    let ro = water.camera_pos.xyz; // ray origin = the camera (VS-visible copy; see the uniform)

    // PROJECTED GRID: read the uploaded unit grid ([0,1]²) as NDC (with overscan) and cast a ray
    // from the camera through it onto the sea plane. Unproject the far NDC corner through the
    // camera inverse to get the world-space view ray (the ground_fog idiom), then intersect it
    // with `y = sea_level`.
    let ndc = (in.position.xz * 2.0 - vec2<f32>(1.0)) * WATER_OVERSCAN;
    let far4 = water.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let far = far4.xyz / far4.w;
    let to_far = far - ro;
    let t_far = max(length(to_far), 1e-4);
    let rd = to_far / t_far; // = normalize(far - ro)

    // Intersect with the plane. A forward hit exists only when the ray crosses the plane AHEAD
    // of the camera (`t_plane > 0`); otherwise — looking at/above the horizon, or a ray parallel
    // to the sea — clamp to the camera's far distance so the vertex hugs the horizon line. World
    // Y is always taken from the plane below, so a clamped (horizon) vertex is never geometry
    // above the water, and the guarded divide never yields a NaN. (Camera BELOW the sea still
    // resolves: it hits when looking up, clamps to the horizon when looking down.)
    let denom = rd.y;
    let t_plane = (sea_level - ro.y) / denom;
    let hits = abs(denom) > 1e-4 && t_plane > 0.0;
    let t = select(t_far, min(t_plane, t_far), hits);
    let hit = ro + rd * t;
    let wx = hit.x;
    let wz = hit.z;

    // Distance attenuation, computed ONCE per vertex (both rates are per-vertex constants, not
    // per-source): fade the wave height and flatten the normal toward +Y with distance from the
    // camera, so the near-island chop reads as a near-flat mirror far away. This hides the
    // projected grid's far-field coarseness AND kills the shimmer/aliasing of sub-pixel wavelets
    // at range. The AMBIENT rate is the same law scaled by WATER_AMBIENT_FALLOFF_SCALE, so the
    // long directional swell survives out to the horizon — the far sea moves.
    let dist = length(hit.xz - ro.xz);
    let falloff = water.params1.z;
    let atten_radial = 1.0 / (1.0 + dist * falloff);
    let atten_ambient = 1.0 / (1.0 + dist * falloff * WATER_AMBIENT_FALLOFF_SCALE);

    // Navier-Stokes-inspired wave summation (the heightmap idiom with a −ω·t term): ONE loop
    // over ONE roster sums every source's height and, analytically, its XZ derivatives. The
    // kind flag picks the phase argument and the gradient DIRECTION; the sin/cos are evaluated
    // once, outside the branch, so a directional source costs no more than a radial one.
    let time = water.params1.x;
    var h = 0.0;
    var dhdx = 0.0;
    var dhdz = 0.0;
    let n = water.counts.x;
    for (var i = 0u; i < n; i = i + 1u) {
        let s = water.waves[i];
        let amp = s.a.z;
        let k = s.a.w;
        let omega = s.b.x;
        let phase = s.b.y;
        var arg: f32;
        var grad: vec2<f32>;
        var atten: f32;
        if (s.b.z > 0.5) {
            // DIRECTIONAL plane wave: phase runs along `dir`, so the argument is k·(p·dir) and
            // the gradient of `arg` in XZ is simply k·dir — no centre, defined everywhere.
            let dir = s.a.xy;
            arg = k * dot(vec2<f32>(wx, wz), dir) - omega * time + phase;
            grad = dir;
            atten = atten_ambient;
        } else {
            // RADIAL: phase runs with distance from the centre, so the gradient of `arg` is
            // k·(p − centre)/d — the outward radial unit vector.
            let dx = wx - s.a.x;
            let dz = wz - s.a.y;
            let d = max(sqrt(dx * dx + dz * dz), 1e-3);
            arg = k * d - omega * time + phase;
            grad = vec2<f32>(dx / d, dz / d);
            atten = atten_radial;
        }
        h = h + amp * sin(arg) * atten;
        // d/dx [amp·sin(arg)] = amp·k·cos(arg)·(∂arg/∂x)/k = amp·k·cos(arg)·grad.x; same for z.
        let c = amp * k * cos(arg) * atten;
        dhdx = dhdx + c * grad.x;
        dhdz = dhdz + c * grad.y;
    }

    let world = vec3<f32>(wx, sea_level + h, wz);
    // normal_scale exaggerates/dampens the SHADING slope without touching the geometry, so the
    // specular glint can be tuned independently of the wave height.
    let ns = water.params1.y;
    var out: VertexOut;
    out.clip_position = camera.view_projection * vec4<f32>(world, 1.0);
    out.world_position = world;
    out.world_normal = normalize(vec3<f32>(-dhdx * ns, 1.0, -dhdz * ns));
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let sea_level = water.params0.x;
    let shore_fade = water.params0.y;
    let spec_shininess = water.params0.z;
    let spec_strength = water.params0.w;

    let n = normalize(in.world_normal);
    let v = normalize(scene.camera_pos.xyz - in.world_position);
    let ndv = max(dot(n, v), 0.0);

    // Fresnel (Schlick): grazing angles reflect more, so the specular and the shallow tint
    // both strengthen toward the horizon — the source of the horizon glint.
    let f0 = 0.02;
    let fresnel = f0 + (1.0 - f0) * pow(clamp(1.0 - ndv, 0.0, 1.0), 5.0);

    // The frame's SKY SLOTS — slot 0 the sun, slot 1 the moon, the two the Celestial cycle
    // rewrites every frame (the slot-indexed contract `sky_sun()`/`sky_moon()` read) — sampled
    // through the prelude exactly as the lit meshes do. ONE loop over both accumulates the
    // diffuse that lights the BODY and the Blinn specular that glints off it: one sample per
    // slot, both terms. The slots may be BLACK or ABSENT — a black light contributes exactly 0
    // through `light_sample`, and `counts.x` bounds the loop the way sky.wgsl bounds its own
    // reads — so a rig with no moon is bit-identical to the sun-only path.
    let sky_slots = min(scene.counts.x, 2u);
    // The ambient floor is the SHARED frame one — the same `scene.ambient` every lit mesh
    // shades against, filled from the same LightRig — not a second copy in the water uniform.
    var lit = scene.ambient.rgb;
    var spec_sum = vec3<f32>(0.0);
    for (var i = 0u; i < sky_slots; i = i + 1u) {
        let li = scene.lights[i];
        let l = light_sample(li, in.world_position).xyz; // unit vector TOWARD the light
        let radiance = li.color_intensity.rgb * li.color_intensity.w;
        lit = lit + radiance * max(dot(n, l), 0.0);
        // A tight half-vector lobe — NO broad marble cap — so at grazing angles (V·H stretched)
        // near the horizon it becomes a long glinting highlight along the light's azimuth. By
        // day the moon's radiance is ~black and the sun owns the streak; at night the sun term
        // is black and the MOON's streak is what is left on the water.
        let half_vec = normalize(l + v);
        spec_sum = spec_sum + radiance * pow(max(dot(n, half_vec), 0.0), spec_shininess);
    }

    // Body colour by view angle: looking straight down (ndv→1) reads the DEEP colour, a grazing
    // view (ndv→0) the SHALLOW/surface colour. `shore_fade` sharpens the transition (art knob).
    // The ramp is then LIT by `lit` above — the ambient floor + both sky slots' diffuse, the same
    // `ambient + Σ radiance·N·L` the lit meshes shade with — so the sea is bright at noon, warm
    // at sunset, and faintly MOONLIT at night rather than pure black or its authored colour.
    let depth_frac = pow(clamp(ndv, 0.0, 1.0), max(shore_fade, 0.01) * 0.2);
    let body = mix(water.shallow.rgb, water.deep.rgb, depth_frac) * lit;
    // Deeper (opaque) water hides the terrain; shallow/grazing water lets it show through.
    // The FLOOR is the grazing-angle alpha, and it is deliberately HIGH: grazing is exactly
    // where the projected grid's far rows are coarsest and where the Fresnel mirror is
    // strongest, so a low floor let the seabed/terrain behind bleed through the reflection and
    // the two fought — the far water muddied and read as scrambled rather than as a surface.
    // At 0.88 the grazing sea is near-opaque (it still tints, it no longer shows the ground),
    // while a straight-down view stays at 1.0 exactly as before.
    let alpha = clamp(mix(0.88, 1.0, depth_frac), 0.0, 1.0);

    // ENVIRONMENT: the analytic sky along the REFLECTED view ray. `mix(horizon, zenith, …)`
    // under the same `pow(h, 0.5)` horizon compression sky.wgsl paints its gradient with, over
    // the same live `sky_zenith`/`sky_horizon` palette — so the water mirrors the ACTUAL sky
    // above it (including the sunset one) rather than a second, differently-shaped gradient.
    // Blended in by the FRESNEL term, so it strengthens toward the horizon exactly as a real
    // water surface does; `env_strength` is the art dial from full mirror to body-only.
    let refl = reflect(-v, n);
    let env = mix(water.sky_horizon.rgb, water.sky_zenith.rgb, pow(saturate(refl.y), 0.5));
    let env_strength = clamp(water.params1.w, 0.0, 1.0);
    let surface = mix(body, env, fresnel * env_strength);

    // The summed sky-slot specular, Fresnel-weighted under the ONE strength knob (the sun's
    // glint and the moon's streak are the same lobe — only their radiance differs, which is
    // exactly why the moon's is naturally subtle). Written HDR, so the >1 spec survives to the
    // tonemap/bloom.
    let spec = spec_sum * spec_strength * fresnel;

    // Premultiplied "over": the surface is premultiplied by alpha, the specular adds on top as
    // glow.
    let rgb = surface * alpha + spec;
    return vec4<f32>(rgb, alpha);
}
