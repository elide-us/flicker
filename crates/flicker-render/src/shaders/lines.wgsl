// Immediate-mode line shader.
//
// Renders un-indexed line-list vertices with per-vertex color. Used
// by `Renderer::draw_bounding_box` and any other gizmo-style line
// drawing. Shares the mesh pipeline's camera uniform for the view-
// projection matrix; depth-tested against the 3D scene (LessEqual)
// but does not write depth so line drawing doesn't occlude other
// geometry that's submitted later in the same pass.

struct Camera {
    view_projection: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = camera.view_projection * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
