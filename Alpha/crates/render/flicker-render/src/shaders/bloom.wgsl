// HDR bloom — bright-pass + separable Gaussian blur + additive composite.
//
// See pipeline_bloom.rs. A chain of fullscreen-triangle draws run AFTER everything that
// writes the LINEAR HDR (rgba16f) attachment and BEFORE the `tonemap_grade` resolve reads
// it — derived from reads/writes (bloom reads `hdr`, writes `hdr`; the tonemap reads `hdr`):
//
//   1. fs_bright    — full-res hdr -> half-res: keep the parts above a SOFT-KNEED threshold
//                     (a smooth ramp so the bloom does not pop on/off at a hard edge).
//   2. fs_blur_h    — 9-tap Gaussian along X (half-res -> half-res).
//   3. fs_blur_v    — 9-tap Gaussian along Y (half-res -> half-res).
//   4. fs_composite — add the blurred bright buffer back into the full-res hdr (additive
//                     blend), scaled by `intensity`, so the sun glint / sun disc GLOW.
//
// The bright/blur passes overwrite their target; the composite ADDS (One, One) with a colour
// write-mask only, so the hdr alpha the tonemap passes through is left intact.

struct Bloom {
    // (1/w, 1/h) of the texture the blur SAMPLES (the half-res scratch); zw unused.
    texel: vec4<f32>,
    // (threshold, knee, intensity, radius).
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> bloom: Bloom;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOut {
    // The same fullscreen triangle the sky / fog / tonemap passes use, plus a [0, 1] uv so the
    // bright/composite can up/down-sample between resolutions. Self-consistent: the read uv
    // and the write position share ONE mapping, so any flip cancels through the chain.
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VertexOut;
    let c = corners[vid];
    out.clip_position = vec4<f32>(c, 0.0, 1.0);
    out.uv = c * 0.5 + 0.5;
    return out;
}

// SOFT-KNEE bright extraction: below `threshold - knee` returns 0; through the knee a smooth
// quadratic ramp; above `threshold + knee` it is exactly `b - threshold` (the hard
// `max(b - threshold, 0)` with a soft shoulder). Returned as a SCALE applied to the colour so
// the hue is preserved. Mirrored on the CPU by `soft_knee` in pipeline_bloom.rs.
fn soft_knee(b: f32, threshold: f32, knee: f32) -> f32 {
    let soft = clamp(b - threshold + knee, 0.0, 2.0 * knee);
    let curve = soft * soft / (4.0 * knee + 1.0e-4);
    return max(curve, b - threshold) / max(b, 1.0e-4);
}

@fragment
fn fs_bright(in: VertexOut) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src_tex, src_smp, in.uv, 0.0).rgb;
    let b = max(c.r, max(c.g, c.b)); // brightness = max channel
    let scale = max(soft_knee(b, bloom.params.x, bloom.params.y), 0.0);
    return vec4<f32>(c * scale, 1.0);
}

// One separable 9-tap Gaussian pass along `axis` (Rastergrid weights, sum ~ 1). `radius`
// scales the tap spacing so the spread tunes in data without changing the tap count.
fn blur9(uv: vec2<f32>, axis: vec2<f32>) -> vec3<f32> {
    let d = axis * bloom.texel.xy * bloom.params.w;
    var sum = textureSampleLevel(src_tex, src_smp, uv, 0.0).rgb * 0.227027;
    sum += textureSampleLevel(src_tex, src_smp, uv + d * 1.0, 0.0).rgb * 0.1945946;
    sum += textureSampleLevel(src_tex, src_smp, uv - d * 1.0, 0.0).rgb * 0.1945946;
    sum += textureSampleLevel(src_tex, src_smp, uv + d * 2.0, 0.0).rgb * 0.1216216;
    sum += textureSampleLevel(src_tex, src_smp, uv - d * 2.0, 0.0).rgb * 0.1216216;
    sum += textureSampleLevel(src_tex, src_smp, uv + d * 3.0, 0.0).rgb * 0.054054;
    sum += textureSampleLevel(src_tex, src_smp, uv - d * 3.0, 0.0).rgb * 0.054054;
    sum += textureSampleLevel(src_tex, src_smp, uv + d * 4.0, 0.0).rgb * 0.016216;
    sum += textureSampleLevel(src_tex, src_smp, uv - d * 4.0, 0.0).rgb * 0.016216;
    return sum;
}

@fragment
fn fs_blur_h(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(blur9(in.uv, vec2<f32>(1.0, 0.0)), 1.0);
}

@fragment
fn fs_blur_v(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(blur9(in.uv, vec2<f32>(0.0, 1.0)), 1.0);
}

@fragment
fn fs_composite(in: VertexOut) -> @location(0) vec4<f32> {
    // Additive (One, One) into the hdr — emit bloom * intensity. Alpha is masked off at the
    // pipeline (write_mask = COLOR), so the hdr alpha survives for the tonemap's passthrough.
    let c = textureSampleLevel(src_tex, src_smp, in.uv, 0.0).rgb;
    return vec4<f32>(c * bloom.params.z, 1.0);
}
