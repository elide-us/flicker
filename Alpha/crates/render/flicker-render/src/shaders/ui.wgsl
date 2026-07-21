// UI panel shader: a signed-distance rounded box, filled with a solid or 2-stop
// linear gradient and ringed with an optional border. Anti-aliased with
// screen-space derivatives; an extra `feather` widens the edge for soft drop
// shadows. All per-panel params ride on the vertices (no uniform/push-constant
// channel on the 2D pipelines), replicated across the quad — only `local`
// (the pixel coordinate within the rect) varies per corner.
//
// NB: the half-size field is named `extent`, not `half` — `half` is a reserved
// type name in Metal Shading Language, so a `half` field breaks the WGSL→MSL
// translation (caught by `ui_pipeline_compiles_shader`).

struct VertexIn {
    @location(0) position: vec2<f32>, // NDC
    @location(1) local:    vec2<f32>, // px within the rect: (0,0)..(w,h)
    @location(2) extent:   vec2<f32>, // half size in px
    @location(3) params:   vec4<f32>, // radius, border, grad, feather (px / mode)
    @location(4) fill0:    vec4<f32>, // gradient stop 0 / solid fill
    @location(5) fill1:    vec4<f32>, // gradient stop 1
    @location(6) bcolor:   vec4<f32>, // border colour
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local:  vec2<f32>,
    @location(1) extent: vec2<f32>,
    @location(2) params: vec4<f32>,
    @location(3) fill0:  vec4<f32>,
    @location(4) fill1:  vec4<f32>,
    @location(5) bcolor: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.local  = in.local;
    out.extent = in.extent;
    out.params = in.params;
    out.fill0  = in.fill0;
    out.fill1  = in.fill1;
    out.bcolor = in.bcolor;
    return out;
}

// Signed distance to a rounded box centred at the origin with half-extents `b`
// and corner radius `r` (negative inside, 0 on the edge). Standard IQ form.
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

// The panel colours arrive as sRGB values (the `theme.tokens` are hex/255, the
// same numbers the design's CSS uses). The surface is an sRGB target, so the GPU
// re-encodes the shader's *linear* output on store — without this decode a token
// like #1b1f28 would be treated as linear and brightened toward white. Gradients
// are mixed in sRGB space (matching CSS) and decoded once, here at the output.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(lo, hi, c > vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let radius  = in.params.x;
    let border  = in.params.y;
    let grad    = in.params.z;
    let feather = in.params.w;

    // Centred pixel coordinate; clamp radius so corners never invert on a
    // thin rect (radius > the smaller half-extent).
    let p = in.local - in.extent;
    let r = min(radius, min(in.extent.x, in.extent.y));
    let d = sd_round_box(p, in.extent, r);

    // Fill: 0 = solid (fill0), 1 = vertical gradient, 2 = horizontal.
    var t = 0.0;
    if (grad == 1.0) {
        t = clamp(in.local.y / max(2.0 * in.extent.y, 1.0), 0.0, 1.0);
    } else if (grad == 2.0) {
        t = clamp(in.local.x / max(2.0 * in.extent.x, 1.0), 0.0, 1.0);
    }
    let fill = mix(in.fill0, in.fill1, t);

    // Border band: border colour where -border < d < 0, fill inside that.
    // The band boundary stays crisp (fwidth) even when the outer edge is
    // feathered for a shadow.
    var col = fill;
    if (border > 0.0) {
        let fcov = clamp(0.5 - (d + border) / max(fwidth(d), 1e-4), 0.0, 1.0);
        col = mix(in.bcolor, fill, fcov);
    }

    // Outer edge coverage (straight alpha — do NOT premultiply rgb). Decode the
    // sRGB fill to linear so the sRGB target re-encodes it to the intended colour.
    let aa = max(fwidth(d), 1e-4) + max(feather, 0.0);
    let cov = clamp(0.5 - d / aa, 0.0, 1.0);
    return vec4<f32>(srgb_to_linear(col.rgb), col.a * cov);
}
