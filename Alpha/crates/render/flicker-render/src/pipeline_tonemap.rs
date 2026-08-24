//! Tonemap + colour-grade RESOLVE pass.
//!
//! A single fullscreen-triangle draw (like the sky) that reads the LINEAR HDR (rgba16f)
//! attachment the lit-3D passes wrote and resolves it into the surface's sRGB `color`
//! attachment, in this order: exposure, the optional colour-grade tint, then the
//! ACES-fitted filmic curve (Narkowicz 2015) LAST, plus alpha passthrough. Exposure and
//! the grade are linear-HDR operations; the curve's clamp is the final word, so a tint
//! component above 1 cannot re-open the clip. See `shaders/tonemap_grade.wgsl`.
//!
//! It runs LAST for a surface that declares an `hdr` attachment (the recipe's
//! `tonemap_grade` pass, ordered after everything that writes `hdr`), so the sRGB
//! `color` the compositor samples is the tonemapped image. A surface with no `hdr`
//! attachment never invokes it — the byte-identical pre-HDR path.
//!
//! The HDR texture bind group is built on demand and cached per HDR id via
//! [`bind_hdr`](TonemapGradePipeline::bind_hdr) — the EXACT precedent the depth-sampling
//! passes use for their per-surface depth texture (`pipeline_ground_fog.rs`), because
//! every surface (the window, each offscreen target) carries its own HDR attachment and
//! a resize renews its id. The colour target is the swapchain (sRGB) format only.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// CPU-side mirror of the WGSL `Grade` uniform. `vec4` lanes for trivial std140.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GradeUniform {
    /// `(r, g, b, _)` — the grade tint the grade lerps toward.
    tint: [f32; 4],
    /// `(exposure, grade_strength, _, _)`.
    params: [f32; 4],
}

const GRADE_UNIFORM_SIZE: u64 = std::mem::size_of::<GradeUniform>() as u64;

impl Default for GradeUniform {
    /// The neutral resolve: unit exposure, no grade tint — pure ACES.
    fn default() -> Self {
        Self::new(Vec3::ZERO, 0.0, 1.0)
    }
}

impl GradeUniform {
    /// Build the uniform from the pass-owned grade params (tint, strength, exposure).
    pub fn new(tint: Vec3, grade_strength: f32, exposure: f32) -> Self {
        Self {
            tint: [tint.x, tint.y, tint.z, 0.0],
            params: [exposure, grade_strength, 0.0, 0.0],
        }
    }
}

/// The tonemap + grade pipeline. A uniform + the HDR colour texture of the SURFACE being
/// resolved, no vertex buffer (the fullscreen triangle is generated from
/// `@builtin(vertex_index)`). Every surface has its own HDR attachment (the window's,
/// recreated on resize; each offscreen target's), so the bind groups are built on demand
/// and cached per HDR id via [`bind_hdr`](Self::bind_hdr) — the renderer selects the one
/// for the pass it is encoding.
pub struct TonemapGradePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// One bind group per HDR texture the pass has sampled, keyed by the renderer's HDR
    /// id; dropped through [`forget`](Self::forget) when that texture goes away.
    bind_groups: HashMap<u64, wgpu::BindGroup>,
    /// The HDR id the next `render` samples — the surface currently being encoded.
    active: Option<u64>,
    uniform_buf: wgpu::Buffer,
}

impl TonemapGradePipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.tonemap_grade.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/tonemap_grade.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.tonemap_grade.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(GRADE_UNIFORM_SIZE),
                    },
                    count: None,
                },
                // The HDR colour attachment, read 1:1 with `textureLoad` (no sampler).
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.tonemap_grade.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.tonemap_grade.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    // Resolves into the sRGB surface — a full-frame opaque overwrite.
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            // A full-frame resolve — no depth attachment (the tonemap pass carries none).
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.tonemap_grade.uniform"),
            size: GRADE_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            bind_groups: HashMap::new(),
            active: None,
            uniform_buf,
        }
    }

    /// Select the HDR texture the next `render` resolves: the HDR attachment of the SURFACE
    /// being drawn — the window's, or an offscreen target's own — keyed by the renderer's
    /// HDR id. Built once per HDR texture and cached, so a steady frame allocates nothing.
    pub fn bind_hdr(&mut self, device: &wgpu::Device, hdr_id: u64, hdr_view: &wgpu::TextureView) {
        self.bind_groups.entry(hdr_id).or_insert_with(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("flicker.tonemap_grade.bind_group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(hdr_view),
                    },
                ],
            })
        });
        self.active = Some(hdr_id);
    }

    /// Drop the bind group of an HDR texture that no longer exists — a freed or resized
    /// target, or the window's HDR attachment after a resize.
    pub fn forget(&mut self, hdr_id: u64) {
        self.bind_groups.remove(&hdr_id);
        if self.active == Some(hdr_id) {
            self.active = None;
        }
    }

    /// Upload this frame's grade params (called from the renderer's `prepare_frame` when a
    /// `tonemap_grade` pass set them).
    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: GradeUniform) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let Some(bind_group) = self.active.and_then(|id| self.bind_groups.get(&id)) else {
            return; // no surface HDR bound yet
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// CPU mirror of the WGSL `aces` operator (`shaders/tonemap_grade.wgsl`) — the SAME
/// Narkowicz 2015 fit, per channel. The shader and this share no code (WGSL vs Rust), so
/// this is the headless proof that the operator itself is sane; the GPU compile test below
/// proves the shader parses against the layout, and the window is the visual check.
#[cfg(test)]
fn aces_cpu(x: f32) -> f32 {
    let (a, b, c, d, e) = (2.51_f32, 0.03, 2.43, 0.59, 0.14);
    ((x * (a * x + b)) / (x * (c * x + d) + e)).clamp(0.0, 1.0)
}

/// CPU mirror of the whole `fs_main` colour math, per channel — exposure, then the grade
/// tint lerp, then ACES. The ORDER is the part worth mirroring: the curve is last, so its
/// clamp is what the resolve ends on.
#[cfg(test)]
fn resolve_cpu(hdr: f32, exposure: f32, tint: f32, strength: f32) -> f32 {
    let c = hdr * exposure;
    let graded = c * (1.0 - strength) + (c * tint) * strength;
    aces_cpu(graded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The ACES operator is sane** — the property S3a decouples from the content flip:
    /// black stays black, it is monotonic non-decreasing, it never leaves `[0, 1]`, a
    /// mid-white (1.0) rolls off BELOW 1.0 (the filmic shoulder, not a passthrough), and a
    /// large input saturates toward 1.0 instead of clipping to a flat wall.
    ///
    /// The mirror above is Rust; the SHIPPED operator is WGSL, so this also inspects the
    /// real channel — the `include_str!`'d shader source the pipeline compiles — for the
    /// five Narkowicz coefficients and the clamp. Editing `tonemap_grade.wgsl`'s curve
    /// therefore breaks this gate instead of quietly diverging from the mirror.
    #[test]
    fn aces_operator_is_sane() {
        // ── THE REAL CHANNEL ──
        let wgsl = include_str!("shaders/tonemap_grade.wgsl");
        for constant in ["2.51", "0.03", "2.43", "0.59", "0.14"] {
            assert!(
                wgsl.contains(constant),
                "tonemap_grade.wgsl no longer carries the Narkowicz coefficient {constant} \
                 the CPU mirror below asserts against"
            );
        }
        assert!(
            wgsl.contains("clamp("),
            "tonemap_grade.wgsl's aces must still clamp into [0, 1] — without it the \
             resolve can hand the sRGB store an out-of-range value"
        );
        // ── THE GRADE RIDES BEFORE THE CLAMP ──
        // Order is the contract: exposure and the tint are linear-HDR ops, ACES is last.
        let (aces_at, grade_at) = (
            wgsl.find("c = aces(c)")
                .expect("the shader applies the curve"),
            wgsl.find("c = mix(c, c * grade.tint.rgb")
                .expect("the shader applies the grade tint"),
        );
        assert!(
            grade_at < aces_at,
            "the grade tint must be applied BEFORE the ACES clamp, or a tint component \
             above 1 re-opens the clip the curve exists to close"
        );

        assert_eq!(aces_cpu(0.0), 0.0, "black stays black");
        assert!(
            aces_cpu(1.0) < 1.0,
            "mid-white rolls off below 1.0 (filmic shoulder)"
        );
        assert!(
            aces_cpu(1.0) > 0.7,
            "…but is not crushed — ~0.8 for a unit input"
        );

        let mut prev = f32::NEG_INFINITY;
        let mut x = 0.0_f32;
        while x <= 32.0 {
            let y = aces_cpu(x);
            assert!((0.0..=1.0).contains(&y), "aces stays in [0,1]: {x} -> {y}");
            assert!(
                y >= prev - 1e-6,
                "aces must be monotonic: {x} -> {y} (prev {prev})"
            );
            prev = y;
            x += 0.01;
        }
        assert!(
            aces_cpu(1.0e4) > 0.99,
            "bright highlights approach 1.0, no hard clip"
        );

        // The whole resolve, in shipped order: solarbirth's warm tint (1.06 red) at full
        // strength over a bright input still lands inside [0, 1], because the curve — and
        // its clamp — runs after the grade. Grading after the curve would multiply an
        // already-resolved ~0.99 by 1.06 and hand the sRGB store 1.05.
        for hdr in [0.0_f32, 0.25, 1.0, 8.0, 1.0e4] {
            let y = resolve_cpu(hdr, 1.0, 1.06, 1.0);
            assert!(
                (0.0..=1.0).contains(&y),
                "a tint > 1 must not re-open the clip: {hdr} -> {y}"
            );
        }
        // A zero-strength grade IS the pure ACES resolve, whatever the tint says.
        assert_eq!(resolve_cpu(1.0, 1.0, 1.06, 0.0), aces_cpu(1.0));
        // …and the tint still does something: a warm cast at full strength lifts red.
        assert!(resolve_cpu(0.25, 1.0, 1.06, 1.0) > resolve_cpu(0.25, 1.0, 1.06, 0.0));
    }

    /// Build the pipeline under a validation error scope — this compiles
    /// `tonemap_grade.wgsl` and checks it against the bind-group layout, so a malformed
    /// shader fails here rather than at app launch. Skips cleanly with no GPU adapter.
    #[test]
    fn tonemap_grade_pipeline_compiles_shader() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            eprintln!("tonemap_grade_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("flicker.tonemap_grade_test.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = TonemapGradePipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.set_uniform(&queue, GradeUniform::default());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "tonemap_grade.wgsl failed validation: {err:?}"
        );
    }
}
