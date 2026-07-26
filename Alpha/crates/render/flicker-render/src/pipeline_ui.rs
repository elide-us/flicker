//! Vector "UI panel" pipeline — rounded-rect + 2-stop gradient + border in one
//! SDF draw.
//!
//! The flat Prism chrome (menu / pause / settings panels, the sapphire / stone /
//! rust button kit, field wells, dropdown fields) is a *shape*, not a texture:
//! each [`Renderer::draw_ui_panel`](crate::Renderer::draw_ui_panel) call appends
//! one quad whose fragment shader (`shaders/ui.wgsl`) evaluates a signed-distance
//! rounded box, fills it with a solid or 2-stop linear gradient, and rings it
//! with an optional border — anti-aliased via screen-space derivatives, with an
//! extra `feather` for soft drop shadows. This retires the CPU-baked panel /
//! button textures and the stacked-1px-rect border / shadow fakes: one command,
//! one draw call per panel run.
//!
//! Structurally identical to [`pipeline_triangle`](crate::pipeline_triangle) — a
//! bind-group-free, vertex-fed 2D pipeline whose quads are stably sorted by
//! `layer` into per-layer [`Band`]s and drawn interleaved with the triangle /
//! sprite / text bands of the same layer (`Renderer::encode_passes`), so 2D
//! ordering stays pure CPU painter's order and the depth buffer is never used
//! (mirrors DirectXTK's `DepthNone` `SpriteBatch` default). Every per-panel
//! parameter (half-size, radius, border, both gradient stops, border colour) is
//! replicated across the six vertices — exactly how the sprite pipeline
//! replicates its per-quad colour — since there is no uniform / push-constant
//! channel on the 2D pipelines.

use bytemuck::{Pod, Zeroable};
use glam::Vec2;

use crate::pipeline_mesh::DEPTH_FORMAT;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2], // NDC
    local: [f32; 2],    // px within the rect: (0,0)..(w,h)
    extent: [f32; 2],   // half size in px (named `extent`, not `half`: `half`
    // is a reserved Metal type name and breaks the WGSL→MSL translation)
    params: [f32; 4], // radius, border, grad, feather
    fill0: [f32; 4],
    fill1: [f32; 4],
    bcolor: [f32; 4],
}

impl Vertex {
    const ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4,
        4 => Float32x4, 5 => Float32x4, 6 => Float32x4
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

/// One panel awaiting draw: its six vertices plus the layer they sort on.
struct Panel {
    layer: f32,
    clip: Option<[f32; 4]>,
    verts: [Vertex; 6],
}

/// A contiguous block of one layer's vertices within the uploaded buffer.
struct Band {
    layer: f32,
    clip: Option<[f32; 4]>,
    vertex_start: u32,
    vertex_count: u32,
}

/// Per-frame UI-panel pipeline state.
pub struct UiPipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: u64,
    /// Framebuffer size cached from the latest `push` — the scissor's full-frame reset.
    screen: Vec2,
    /// Submitted panels, in submission order, tagged with their layer.
    panels: Vec<Panel>,
    /// Scratch built in `prepare`: `panels` flattened into vertices, ordered by
    /// layer. Retained across frames to avoid reallocating.
    upload: Vec<Vertex>,
    /// One band per distinct layer, ascending — indexes into `upload`.
    bands: Vec<Band>,
}

impl UiPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.ui.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/ui.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.ui.pipeline_layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.ui.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // 2D overlay — share the depth attachment but neither write nor
            // test depth, so panels always layer on top of the 3D scene.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let initial_capacity = (std::mem::size_of::<Vertex>() * 6 * 64) as u64;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.ui.vbo"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vertex_buffer,
            vertex_buffer_capacity: initial_capacity,
            screen: Vec2::ZERO,
            panels: Vec::new(),
            upload: Vec::new(),
            bands: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.panels.clear();
        self.upload.clear();
        self.bands.clear();
    }

    /// Push one panel at `position` (top-left in pixels) of the given pixel
    /// `size`. `fill0`/`fill1` are the gradient stops (equal for a solid fill);
    /// `grad` selects the axis (0 solid, 1 vertical, 2 horizontal); `radius` is
    /// the corner radius and `border` the border thickness (both px, 0 to
    /// disable); `feather` softens the outer edge (px, for drop shadows).
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        screen: Vec2,
        position: Vec2,
        size: Vec2,
        fill0: [f32; 4],
        fill1: [f32; 4],
        grad: f32,
        radius: f32,
        border: f32,
        border_color: [f32; 4],
        feather: f32,
        layer: f32,
        clip: Option<[f32; 4]>,
    ) {
        self.screen = screen;
        let to_ndc =
            |p: Vec2| -> [f32; 2] { [(p.x / screen.x) * 2.0 - 1.0, 1.0 - (p.y / screen.y) * 2.0] };
        let extent = [size.x * 0.5, size.y * 0.5];
        let params = [radius, border, grad, feather];
        let mk = |ndc: [f32; 2], local: [f32; 2]| Vertex {
            position: ndc,
            local,
            extent,
            params,
            fill0,
            fill1,
            bcolor: border_color,
        };

        let tl = position;
        let tr = position + Vec2::new(size.x, 0.0);
        let bl = position + Vec2::new(0.0, size.y);
        let br = position + size;

        self.panels.push(Panel {
            layer,
            clip,
            verts: [
                mk(to_ndc(tl), [0.0, 0.0]),
                mk(to_ndc(bl), [0.0, size.y]),
                mk(to_ndc(br), [size.x, size.y]),
                mk(to_ndc(tl), [0.0, 0.0]),
                mk(to_ndc(br), [size.x, size.y]),
                mk(to_ndc(tr), [size.x, 0.0]),
            ],
        });
    }

    /// The distinct layers present this frame, ascending. Used by the renderer
    /// to drive interleaved per-layer drawing across the 2D pipelines.
    pub fn layers(&self) -> impl Iterator<Item = f32> + '_ {
        self.bands.iter().map(|b| b.layer)
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.upload.clear();
        self.bands.clear();
        if self.panels.is_empty() {
            return;
        }

        // Stable-sort by layer so panels group into ascending bands while
        // preserving submission order (hence blend order) within a layer.
        let mut order: Vec<usize> = (0..self.panels.len()).collect();
        order.sort_by(|&a, &b| self.panels[a].layer.total_cmp(&self.panels[b].layer));

        for &i in &order {
            self.upload.extend_from_slice(&self.panels[i].verts);
        }

        let mut idx = 0;
        let mut start = 0u32;
        while idx < order.len() {
            let layer = self.panels[order[idx]].layer;
            let clip = self.panels[order[idx]].clip;
            let mut count = 0u32;
            // Split a layer's run into sub-bands at each clip change, so a scroll
            // region's panels draw under their own scissor while the rest stay full.
            while idx < order.len()
                && self.panels[order[idx]].layer == layer
                && self.panels[order[idx]].clip == clip
            {
                count += 6;
                idx += 1;
            }
            self.bands.push(Band {
                layer,
                clip,
                vertex_start: start,
                vertex_count: count,
            });
            start += count;
        }

        let needed = (self.upload.len() * std::mem::size_of::<Vertex>()) as u64;
        if needed > self.vertex_buffer_capacity {
            let new_capacity = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.ui.vbo"),
                size: new_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_buffer_capacity = new_capacity;
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.upload));
    }

    /// Draw the panels submitted at `layer` (no-op if none). A layer may hold
    /// several bands when a scroll region set a clip mid-layer; each band draws
    /// under its own scissor (full frame when unclipped), so a clip never leaks
    /// onto the next.
    pub fn render_layer<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, layer: f32) {
        let bytes = (self.upload.len() * std::mem::size_of::<Vertex>()) as u64;
        let mut bound = false;
        for band in self.bands.iter().filter(|b| b.layer == layer) {
            if !bound {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..bytes));
                bound = true;
            }
            set_scissor(pass, self.screen, band.clip);
            pass.draw(
                band.vertex_start..band.vertex_start + band.vertex_count,
                0..1,
            );
        }
    }
}

/// Apply a 2D scissor: `clip` (px x,y,w,h) clamped to the framebuffer, or the full
/// frame when `None`. Every clipped 2D pipeline sets this per band so a scroll
/// region's clip masks only its own draws and resets for everything after.
pub(crate) fn set_scissor(pass: &mut wgpu::RenderPass, screen: Vec2, clip: Option<[f32; 4]>) {
    let (sw, sh) = (screen.x.max(0.0), screen.y.max(0.0));
    let (x, y, w, h) = match clip {
        Some([cx, cy, cw, ch]) => {
            let x0 = cx.clamp(0.0, sw);
            let y0 = cy.clamp(0.0, sh);
            let x1 = (cx + cw).clamp(0.0, sw);
            let y1 = (cy + ch).clamp(0.0, sh);
            (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
        }
        None => (0.0, 0.0, sw, sh),
    };
    pass.set_scissor_rect(x as u32, y as u32, w as u32, h as u32);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the UI pipeline under a validation error scope — this compiles
    /// `ui.wgsl` and checks it against the vertex layout, so a malformed shader
    /// (or a vertex-attribute mismatch) fails here rather than at app launch.
    /// Also pushes a gradient panel and prepares it, exercising the whole vertex
    /// path (both stops, border colour, radius). Skips without a GPU.
    #[test]
    fn ui_pipeline_compiles_shader() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            eprintln!("ui_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("flicker.ui_test.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut pipeline = UiPipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.push(
            Vec2::new(1280.0, 720.0),
            Vec2::new(40.0, 60.0),
            Vec2::new(300.0, 120.0),
            [0.10, 0.20, 0.40, 1.0], // stop 0
            [0.05, 0.10, 0.20, 1.0], // stop 1
            1.0,                     // vertical gradient
            14.0,                    // radius
            1.0,                     // border
            [0.30, 0.40, 0.60, 1.0], // border colour
            0.0,                     // feather
            0.0,                     // layer
            None,                    // clip
        );
        pipeline.prepare(&device, &queue);
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "ui.wgsl failed validation: {err:?}");
    }
}
