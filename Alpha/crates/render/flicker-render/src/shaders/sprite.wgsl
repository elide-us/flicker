struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@group(0) @binding(0) var sprite_tex: texture_2d<f32>;
@group(0) @binding(1) var sprite_samp: sampler;

// The 2D UI colours arrive as sRGB values (theme.tokens = hex/255). The surface
// is an sRGB target, so decode the *tint* to linear before the sRGB store — the
// sampled texture is already linear (the sampler decodes an sRGB texture on read),
// so it is NOT decoded again. Without this a raw token (e.g. #1b1f28) is treated
// as linear and brightened toward white. White tint (1,1,1) is a fixed point, so
// textured sprites drawn untinted (logos, the Muse, baked chrome) are unchanged.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(lo, hi, c > vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let tint = vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
    return textureSample(sprite_tex, sprite_samp, in.uv) * tint;
}
