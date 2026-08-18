//! Textured-quad ("sprite") pipeline.
//!
//! Each [`Renderer::draw_sprite`](crate::Renderer::draw_sprite) call appends six
//! vertices for one quad tagged with a `layer` (the painter's-order sort key
//! shared by every 2D pipeline). Pixel coordinates are converted to NDC at
//! vertex-push time. In `prepare` the quads are stably sorted by layer
//! (submission order preserved within a layer) and coalesced into [`Run`]s that
//! break on a layer *or* texture change, so quads sharing a texture within a
//! layer still render in one draw call. The renderer draws each layer's runs
//! interleaved with the triangle/text bands of the same layer
//! (`Renderer::end_frame`) — 2D ordering is pure CPU painter's order, the depth
//! buffer is never used (mirrors DirectXTK's `DepthNone` `SpriteBatch` default).

use bytemuck::{Pod, Zeroable};
use glam::Vec2;

use crate::pipeline_mesh::DEPTH_FORMAT;
use crate::texture::TextureHandle;

/// Rotate `p` about `pivot` by `radians`, in PIXEL space (before pixel→NDC).
/// Rotating here rather than in NDC keeps the sprite square: `to_ndc` scales x
/// and y by different amounts (the screen aspect), so a rotation applied after
/// it would shear. Screen y is down, so a positive angle turns CLOCKWISE on
/// screen (the DirectXTK `SpriteBatch` convention); the `flicker-2d` `Sprite`
/// wrapper negates if it wants counter-clockwise-positive.
#[inline]
fn rotate_px(p: Vec2, pivot: Vec2, radians: f32) -> Vec2 {
    let d = p - pivot;
    let (sin, cos) = radians.sin_cos();
    pivot + Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

pub struct SpritePipeline {
    pipeline: wgpu::RenderPipeline,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: u64,
    /// Submitted quads, in submission order, each tagged with its layer.
    quads: Vec<Quad>,
    /// Scratch built in `prepare`: `quads` flattened to vertices, ordered by
    /// layer. Retained across frames to avoid reallocating.
    upload: Vec<Vertex>,
    /// One run per contiguous block of quads sharing a layer *and* texture.
    runs: Vec<Run>,
    /// Distinct layers present this frame, ascending.
    layer_set: Vec<f32>,
    /// Framebuffer size cached from the latest `push` — the scissor's full-frame reset.
    screen: Vec2,
}

/// One quad awaiting draw: its six vertices, texture, and sort layer.
struct Quad {
    layer: f32,
    clip: Option<[f32; 4]>,
    texture: TextureHandle,
    verts: [Vertex; 6],
}

struct Run {
    layer: f32,
    clip: Option<[f32; 4]>,
    texture: TextureHandle,
    vertex_offset: u32,
    vertex_count: u32,
}

impl SpritePipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("flicker.sprite.bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flicker.sprite.sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.sprite.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sprite.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.sprite.pipeline_layout"),
            bind_group_layouts: &[&texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.sprite.pipeline"),
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
            // 2D overlay — share the depth attachment with the 3D pipeline
            // but neither write nor test depth so sprites always layer on
            // top of the 3D scene.
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
            label: Some("flicker.sprite.vbo"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            texture_bind_group_layout,
            sampler,
            vertex_buffer,
            vertex_buffer_capacity: initial_capacity,
            quads: Vec::new(),
            upload: Vec::new(),
            runs: Vec::new(),
            layer_set: Vec::new(),
            screen: Vec2::ZERO,
        }
    }

    pub fn clear(&mut self) {
        self.quads.clear();
        self.upload.clear();
        self.runs.clear();
        self.layer_set.clear();
    }

    /// Push one quad at `position` (top-left in pixels) with the given pixel `size`,
    /// multiplied by `color` (RGBA in 0..1) in the fragment shader, sorting at `layer`.
    ///
    /// `uv` is the SOURCE sub-rectangle in normalized texture space, `[u0, v0, u1, v1]`
    /// with the origin top-left — `[0.0, 0.0, 1.0, 1.0]` for the whole texture. This is
    /// what lets many small images share one texture (an ATLAS) and so one draw call:
    /// the quads batch by `texture`, and an atlas makes that one bind for the lot.
    ///
    /// `rotation` (radians) spins the quad about `pivot` (in the same top-left
    /// screen-pixel space as `position`); `pivot = position + size * 0.5` spins
    /// about the sprite's centre. `rotation == 0.0` is the axis-aligned fast path
    /// (`pivot` ignored) — every non-rotated caller is byte-identical to before.
    /// The rotation is baked into the six vertex positions here, so `prepare`,
    /// batching, and `render_layer` are untouched: a rotated quad still coalesces
    /// by `(layer, texture, clip)` like any other.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        screen: Vec2,
        texture: TextureHandle,
        position: Vec2,
        size: Vec2,
        color: [f32; 4],
        layer: f32,
        clip: Option<[f32; 4]>,
        uv: [f32; 4],
        rotation: f32,
        pivot: Vec2,
    ) {
        self.screen = screen;
        let to_ndc =
            |p: Vec2| -> [f32; 2] { [(p.x / screen.x) * 2.0 - 1.0, 1.0 - (p.y / screen.y) * 2.0] };

        let tl = position;
        let tr = position + Vec2::new(size.x, 0.0);
        let bl = position + Vec2::new(0.0, size.y);
        let br = position + size;

        // Rotate the corners in pixel space before pixel→NDC. A zero angle skips
        // the trig entirely, so the common axis-aligned case is unchanged.
        let (tl, tr, bl, br) = if rotation == 0.0 {
            (tl, tr, bl, br)
        } else {
            (
                rotate_px(tl, pivot, rotation),
                rotate_px(tr, pivot, rotation),
                rotate_px(bl, pivot, rotation),
                rotate_px(br, pivot, rotation),
            )
        };

        let [u0, v0, u1, v1] = uv;

        self.quads.push(Quad {
            layer,
            clip,
            texture,
            verts: [
                Vertex {
                    position: to_ndc(tl),
                    uv: [u0, v0],
                    color,
                },
                Vertex {
                    position: to_ndc(bl),
                    uv: [u0, v1],
                    color,
                },
                Vertex {
                    position: to_ndc(br),
                    uv: [u1, v1],
                    color,
                },
                Vertex {
                    position: to_ndc(tl),
                    uv: [u0, v0],
                    color,
                },
                Vertex {
                    position: to_ndc(br),
                    uv: [u1, v1],
                    color,
                },
                Vertex {
                    position: to_ndc(tr),
                    uv: [u1, v0],
                    color,
                },
            ],
        });
    }

    /// The distinct layers present this frame, ascending.
    pub fn layers(&self) -> impl Iterator<Item = f32> + '_ {
        self.layer_set.iter().copied()
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.upload.clear();
        self.runs.clear();
        self.layer_set.clear();
        if self.quads.is_empty() {
            return;
        }

        // Stable-sort by layer so quads group into ascending bands while
        // preserving submission order (hence blend order) within a layer.
        let mut order: Vec<usize> = (0..self.quads.len()).collect();
        order.sort_by(|&a, &b| self.quads[a].layer.total_cmp(&self.quads[b].layer));

        for &i in &order {
            let quad = &self.quads[i];
            let vertex_offset = self.upload.len() as u32;
            self.upload.extend_from_slice(&quad.verts);

            // Coalesce with the previous run only when both layer and texture
            // match; a layer change always starts a new run (and a new band).
            match self.runs.last_mut() {
                Some(run)
                    if run.layer == quad.layer
                        && run.texture == quad.texture
                        && run.clip == quad.clip =>
                {
                    run.vertex_count += 6;
                }
                _ => {
                    if self.layer_set.last() != Some(&quad.layer) {
                        self.layer_set.push(quad.layer);
                    }
                    self.runs.push(Run {
                        layer: quad.layer,
                        clip: quad.clip,
                        texture: quad.texture,
                        vertex_offset,
                        vertex_count: 6,
                    });
                }
            }
        }

        let needed = (self.upload.len() * std::mem::size_of::<Vertex>()) as u64;
        if needed > self.vertex_buffer_capacity {
            let new_capacity = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.sprite.vbo"),
                size: new_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_buffer_capacity = new_capacity;
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.upload));
    }

    /// Draw only the quads submitted at `layer` (no-op if none).
    pub fn render_layer<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        layer: f32,
        textures: &'a [Option<crate::texture::LoadedTexture>],
    ) {
        if self.runs.iter().all(|r| r.layer != layer) {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        let bytes = (self.upload.len() * std::mem::size_of::<Vertex>()) as u64;
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..bytes));
        for run in self.runs.iter().filter(|r| r.layer == layer) {
            let Some(tex) = textures
                .get(run.texture.0 as usize)
                .and_then(|t| t.as_ref())
            else {
                continue;
            };
            pass.set_bind_group(0, &tex.bind_group, &[]);
            crate::pipeline_ui::set_scissor(pass, self.screen, run.clip);
            let start = run.vertex_offset;
            let end = start + run.vertex_count;
            pass.draw(start..end, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rotate_px;
    use glam::Vec2;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-4
    }

    #[test]
    fn zero_angle_is_identity() {
        let p = Vec2::new(7.0, 3.0);
        // Any pivot, zero angle → the point is unmoved (matches push's fast path).
        assert!(close(rotate_px(p, Vec2::new(100.0, 100.0), 0.0), p));
    }

    #[test]
    fn quarter_turn_about_centre_walks_corners() {
        // A 2×2 quad at origin, centre pivot (1,1). A +90° turn (y-down = clockwise
        // on screen) carries each corner onto the next corner's slot.
        let (c, quarter) = (Vec2::new(1.0, 1.0), std::f32::consts::FRAC_PI_2);
        let tl = Vec2::new(0.0, 0.0);
        let tr = Vec2::new(2.0, 0.0);
        assert!(close(rotate_px(tl, c, quarter), tr)); // top-left lands where top-right was
    }

    #[test]
    fn full_turn_returns_to_start() {
        let p = Vec2::new(5.0, -2.0);
        let c = Vec2::new(1.0, 1.0);
        assert!(close(rotate_px(p, c, std::f32::consts::TAU), p));
    }
}
