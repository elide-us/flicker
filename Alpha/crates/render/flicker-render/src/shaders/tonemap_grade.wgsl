// Tonemap + colour-grade RESOLVE — see pipeline_tonemap.rs.
//
// A fullscreen pass that reads the LINEAR HDR (rgba16f) attachment the lit-3D passes
// wrote and writes the resolved result into the surface's sRGB `color` attachment
// (whose store applies the OETF). The output is LINEAR — the curve maps scene radiance
// [0, ∞) into [0, 1] with a smooth highlight shoulder instead of a hard clip. Alpha
// passes straight through, so a transparent-cleared offscreen globe / portrait keeps
// its cut-out.
//
// THE ORDER IS THE CONTRACT: exposure → grade tint → ACES, and ACES is LAST.
// Both exposure and the grade are LINEAR HDR operations, so they belong on the scene
// radiance while it still has headroom; the ACES fit is the one operator that maps that
// range into [0, 1] and its clamp is the final word. Grading AFTER the curve would let a
// tint component above 1 (a warm cast of 1.06) multiply an already-resolved value back
// out of range and re-open the clip the curve exists to close.
//
// The HDR texture is the SAME size as the destination colour, so each pixel is resolved
// 1:1 by a `textureLoad` at the fragment's own framebuffer coordinate — no sampler, no
// UV flip to get wrong.

struct Grade {
    // rgb = grade tint (a warm/cool cast the grade lerps toward); a unused.
    tint: vec4<f32>,
    // x = exposure (linear multiply), y = grade strength (0 = tonemap only), zw unused.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> grade: Grade;
@group(0) @binding(1) var hdr_tex: texture_2d<f32>;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOut {
    // The same fullscreen triangle the sky / fog passes use.
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VertexOut;
    out.clip_position = vec4<f32>(corners[vid], 0.0, 1.0);
    return out;
}

// ACES filmic tonemap, Narkowicz 2015 fit. Input/output linear; maps [0, ∞) → [0, 1]
// with a slight toe and a smooth shoulder (highlights roll off, never hard-clip).
fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.clip_position.xy);
    let hdr = textureLoad(hdr_tex, px, 0);
    var c = hdr.rgb * grade.params.x; // exposure — linear HDR
    c = mix(c, c * grade.tint.rgb, grade.params.y); // grade: tint lerp, still linear HDR
    c = aces(c); // filmic curve LAST — its clamp is the final word
    return vec4<f32>(c, hdr.a); // alpha passthrough
}
