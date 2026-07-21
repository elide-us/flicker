// Camera-facing billboard shader.
//
// Each vertex carries a per-billboard center (`world_position`), the
// quad half-extents in world units (`world_size`), a normalised corner
// offset in [-0.5, 0.5]^2 (`corner_offset`), a UV, and a tint. The
// vertex shader orients the quad from the camera-space right/up basis
// supplied in the camera uniform, so the quad is always perpendicular
// to the view direction and stays the same world-space size regardless
// of view angle.
//
// Depth-tested (`LessEqual`) and depth-writing — billboards interact
// correctly with surrounding 3D meshes.

struct Camera {
    view_projection: mat4x4<f32>,
    camera_right_ws: vec4<f32>,
    camera_up_ws: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct VertexIn {
    @location(0) corner_offset: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) world_size: vec2<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let right = camera.camera_right_ws.xyz * (in.corner_offset.x * in.world_size.x);
    let up = camera.camera_up_ws.xyz * (in.corner_offset.y * in.world_size.y);
    let world_pos = in.world_position + right + up;
    var out: VertexOut;
    out.clip_position = camera.view_projection * vec4<f32>(world_pos, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let sample = textureSample(atlas_tex, atlas_samp, in.uv);
    let result = sample * in.color;
    // Discard fully-transparent fragments so depth isn't written for them;
    // this keeps cutout-style glyphs from punching through nearby meshes.
    if (result.a < 0.01) {
        discard;
    }
    return result;
}
