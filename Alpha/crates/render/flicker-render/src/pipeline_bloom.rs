//! HDR bloom post-effect pass.
//!
//! Bright HDR highlights (the water's sun glint, the sky's sun disc) GLOW. Three stages of
//! fullscreen-triangle draws — a **bright-pass** (soft-kneed threshold, full-res `hdr` into a
//! half-res buffer), a **separable Gaussian blur** (H then V, ping-ponging between two
//! half-res targets), and an **additive composite** (`hdr += bloom * intensity`) — run AFTER
//! everything that writes the LINEAR HDR (rgba16f) attachment and BEFORE the `tonemap_grade`
//! resolve reads it, so the extra radiance survives the ACES roll-off as a glow. See
//! `shaders/bloom.wgsl`.
//!
//! All four draws share ONE bind-group layout (a `Bloom` uniform + a source texture + a
//! filtering sampler) and ONE uniform buffer, differing only in the source they read and the
//! target they write:
//!
//! | draw       | source                | target        | blend        |
//! |------------|-----------------------|---------------|--------------|
//! | bright     | the surface `hdr`     | scratch **a** | overwrite    |
//! | blur H     | scratch **a**         | scratch **b** | overwrite    |
//! | blur V     | scratch **b**         | scratch **a** | overwrite    |
//! | composite  | scratch **a**         | the `hdr`     | ADD, COLOR   |
//!
//! The bright pass's read of the SURFACE hdr is cached per HDR id via [`Self::bind_bright`] —
//! the EXACT precedent the tonemap uses for its per-surface HDR bind (`pipeline_tonemap.rs`),
//! renewed on resize. The two internal scratch targets are renderer-owned (like the tonemap's
//! HDR attachment, never a scene-created target) and their bind groups are rebuilt whenever
//! the scratch is (re)allocated ([`Self::bind_scratch`]). Every target is [`crate::HDR_FORMAT`].

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

/// CPU-side mirror of the WGSL `Bloom` uniform. `vec4` lanes for trivial std140.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BloomUniform {
    /// `(1/w, 1/h, _, _)` of the half-res scratch — the blur's tap spacing.
    texel: [f32; 4],
    /// `(threshold, knee, intensity, radius)`.
    params: [f32; 4],
}

const BLOOM_UNIFORM_SIZE: u64 = std::mem::size_of::<BloomUniform>() as u64;

impl Default for BloomUniform {
    fn default() -> Self {
        Self::new(1, 1, 1.0, 0.5, 0.6, 1.0)
    }
}

impl BloomUniform {
    /// Build the uniform from the half-res scratch size (for the blur's `texel`) and the
    /// pass-owned art knobs.
    pub fn new(
        scratch_w: u32,
        scratch_h: u32,
        threshold: f32,
        knee: f32,
        intensity: f32,
        radius: f32,
    ) -> Self {
        let texel = |n: u32| if n > 0 { 1.0 / n as f32 } else { 0.0 };
        Self {
            texel: [texel(scratch_w), texel(scratch_h), 0.0, 0.0],
            params: [threshold, knee, intensity, radius],
        }
    }
}

/// The bloom pipeline: four fullscreen draws (bright, blur H, blur V, composite) over one
/// shared bind-group layout + uniform + sampler. The bright pass's read of the surface HDR is
/// cached per HDR id (`bright_src`); the two scratch bind groups (`bind_a`/`bind_b`) are
/// rebuilt on each scratch (re)allocation.
pub struct BloomPipeline {
    bright: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    /// One bind group per SURFACE HDR texture the bright pass has read, keyed by the
    /// renderer's HDR id; dropped through [`Self::forget`] when that texture goes away.
    bright_src: HashMap<u64, wgpu::BindGroup>,
    /// The HDR id the next `encode`'s bright pass reads — the surface being encoded.
    active: Option<u64>,
    /// Bind group over scratch **a** (the blur V + composite source), rebuilt on realloc.
    bind_a: Option<wgpu::BindGroup>,
    /// Bind group over scratch **b** (the blur H source), rebuilt on realloc.
    bind_b: Option<wgpu::BindGroup>,
}

impl BloomPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.bloom.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/bloom.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.bloom.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(BLOOM_UNIFORM_SIZE),
                    },
                    count: None,
                },
                // The source colour, sampled (linear filter) for the down/up-sample + blur taps.
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.bloom.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Every draw is a fullscreen triangle into an HDR_FORMAT target over the ONE layout;
        // only the fragment entry point and the blend differ. `blend = None` overwrites (the
        // bright + blur targets); the composite ADDS with a COLOR write-mask so the hdr alpha
        // (the tonemap's passthrough) is never touched.
        let make = |entry: &str, blend: Option<wgpu::BlendState>, mask: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.bloom.pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: entry,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: crate::HDR_FORMAT,
                        blend,
                        write_mask: mask,
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
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        // `out = src + dst` — the blurred bright buffer ADDS to the hdr already there.
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            // Alpha is write-masked off below, so this component is inert; keep it valid.
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let bright = make("fs_bright", None, wgpu::ColorWrites::ALL);
        let blur_h = make("fs_blur_h", None, wgpu::ColorWrites::ALL);
        let blur_v = make("fs_blur_v", None, wgpu::ColorWrites::ALL);
        let composite = make("fs_composite", Some(additive), wgpu::ColorWrites::COLOR);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flicker.bloom.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.bloom.uniform"),
            size: BLOOM_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            bright,
            blur_h,
            blur_v,
            composite,
            bind_group_layout,
            sampler,
            uniform_buf,
            bright_src: HashMap::new(),
            active: None,
            bind_a: None,
            bind_b: None,
        }
    }

    /// Select the SURFACE HDR the next `encode`'s bright pass reads — the HDR attachment of the
    /// surface being resolved, keyed by the renderer's HDR id (renewed on resize). Built once
    /// per HDR texture and cached, so a steady frame allocates nothing. The exact precedent the
    /// tonemap's `bind_hdr` sets.
    pub fn bind_bright(
        &mut self,
        device: &wgpu::Device,
        hdr_id: u64,
        hdr_view: &wgpu::TextureView,
    ) {
        // Build-then-insert (not `or_insert_with`): `make_bind` borrows `&self`, so it must
        // finish and hand back an owned bind group before `bright_src` takes the `&mut` borrow.
        if !self.bright_src.contains_key(&hdr_id) {
            let bind = self.make_bind(device, hdr_view);
            self.bright_src.insert(hdr_id, bind);
        }
        self.active = Some(hdr_id);
    }

    /// (Re)build the bind groups over the two half-res scratch targets — called by the renderer
    /// whenever it (re)allocates the scratch (a resize, or the first bloom frame), so a cached
    /// bind group can never outlive the texture it samples.
    pub fn bind_scratch(
        &mut self,
        device: &wgpu::Device,
        a_view: &wgpu::TextureView,
        b_view: &wgpu::TextureView,
    ) {
        self.bind_a = Some(self.make_bind(device, a_view));
        self.bind_b = Some(self.make_bind(device, b_view));
    }

    fn make_bind(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.bloom.bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Drop the bright bind group of a SURFACE HDR texture that no longer exists — a freed or
    /// resized target, or the window's HDR attachment after a resize. Called at every site the
    /// tonemap's `forget` is (the renderer owns the leak-prevention, exactly like the HDR
    /// attachment).
    pub fn forget(&mut self, hdr_id: u64) {
        self.bright_src.remove(&hdr_id);
        if self.active == Some(hdr_id) {
            self.active = None;
        }
    }

    /// Upload this frame's bloom params (threshold/knee/intensity/radius + the half-res texel),
    /// called from the renderer's ensure step once per bloom frame.
    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: BloomUniform) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Encode the three-stage bloom into `encoder`: bright (`hdr` -> `a`), blur H (`a` -> `b`),
    /// blur V (`b` -> `a`), composite (`a` -> `hdr`, additive). The renderer owns the two
    /// half-res scratch views (`a_view`/`b_view`) and the surface `hdr_view`; the bind groups
    /// referencing them live here. A no-op (leaving the hdr untouched) until a surface HDR is
    /// bound AND the scratch bind groups exist — so the tonemap that follows resolves the
    /// un-bloomed hdr rather than crashing.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        hdr_view: &wgpu::TextureView,
        a_view: &wgpu::TextureView,
        b_view: &wgpu::TextureView,
    ) {
        let (Some(id), Some(bind_a), Some(bind_b)) = (self.active, &self.bind_a, &self.bind_b)
        else {
            return;
        };
        let Some(bright_src) = self.bright_src.get(&id) else {
            return;
        };

        // The three overwrite passes reuse one helper: a depth-less fullscreen draw that clears
        // its target and runs one pipeline over one bind group.
        let mut overwrite = |target: &wgpu::TextureView,
                             pipeline: &wgpu::RenderPipeline,
                             bind: &wgpu::BindGroup,
                             label: &str| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        };
        overwrite(
            a_view,
            &self.bright,
            bright_src,
            "flicker.bloom.bright_pass",
        );
        overwrite(b_view, &self.blur_h, bind_a, "flicker.bloom.blur_h_pass");
        overwrite(a_view, &self.blur_v, bind_b, "flicker.bloom.blur_v_pass");

        // Composite: ADD the blurred bright buffer (scratch a) back into the hdr — Load, not
        // Clear, so the lit scene beneath it survives.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.bloom.composite_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: hdr_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.composite);
            pass.set_bind_group(0, bind_a, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// CPU mirror of the WGSL `soft_knee` (`shaders/bloom.wgsl`) — the SAME soft-knee bright
/// extraction, so the headless test proves the curve is sane while the text gate below proves
/// the shipped WGSL still carries the same expression (the mirror cannot silently diverge).
#[cfg(test)]
fn soft_knee(b: f32, threshold: f32, knee: f32) -> f32 {
    let soft = (b - threshold + knee).clamp(0.0, 2.0 * knee);
    let curve = soft * soft / (4.0 * knee + 1.0e-4);
    curve.max(b - threshold) / b.max(1.0e-4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The soft-knee bright extraction is sane** — the property the bright pass turns on:
    /// nothing below `threshold - knee` blooms, the ramp is monotonic and smooth through the
    /// knee, and far above the threshold the scale approaches `(b - threshold) / b` (a clean
    /// threshold subtraction). The mirror is Rust; the SHIPPED extraction is WGSL, so this also
    /// inspects the real channel — the `include_str!`'d shader — for the same expression and
    /// the four entry points, so editing `bloom.wgsl` breaks this gate instead of quietly
    /// diverging from the mirror (rules 8634C200 / 522E4A1D).
    #[test]
    fn bloom_wgsl_ships_bright_knee_blur_and_composite() {
        let (threshold, knee) = (1.0_f32, 0.5_f32);

        // Below the knee: no bloom.
        assert_eq!(
            soft_knee(0.2, threshold, knee),
            0.0,
            "dark pixels do not bloom"
        );
        assert_eq!(
            soft_knee(threshold - knee, threshold, knee),
            0.0,
            "the knee foot is exactly 0"
        );
        // Monotonic non-decreasing scale across the ramp and beyond.
        let mut prev = f32::NEG_INFINITY;
        let mut x = 0.0_f32;
        while x <= 8.0 {
            let s = soft_knee(x, threshold, knee);
            assert!(
                s >= prev - 1e-6,
                "soft-knee scale must be monotonic: {x} -> {s} (prev {prev})"
            );
            assert!(s >= 0.0, "the scale is never negative: {x} -> {s}");
            prev = s;
            x += 0.01;
        }
        // Far above the threshold the scale approaches the plain threshold subtraction.
        let bright = 8.0_f32;
        let scaled = bright * soft_knee(bright, threshold, knee);
        assert!(
            (scaled - (bright - threshold)).abs() < 1e-2,
            "well above the knee, bright*scale ~= b - threshold: {scaled} vs {}",
            bright - threshold
        );

        // ── THE REAL CHANNEL ──
        let wgsl = include_str!("shaders/bloom.wgsl");
        for token in [
            "fn vs_main",
            "fn fs_bright",
            "fn fs_blur_h",
            "fn fs_blur_v",
            "fn fs_composite",
        ] {
            assert!(
                wgsl.contains(token),
                "bloom.wgsl no longer defines `{token}`"
            );
        }
        // The bright-pass soft knee, verbatim from the mirror above.
        assert!(
            wgsl.contains("clamp(b - threshold + knee, 0.0, 2.0 * knee)")
                && wgsl.contains("soft * soft / (4.0 * knee + 1.0e-4)")
                && wgsl.contains("max(curve, b - threshold) / max(b, 1.0e-4)"),
            "bloom.wgsl no longer carries the soft-knee expression the CPU mirror asserts"
        );
        // The 9-tap Gaussian: the centre weight, the outermost weight, and separable taps.
        assert!(
            wgsl.contains("* 0.227027") && wgsl.contains("* 0.016216"),
            "bloom.wgsl no longer carries the 9-tap Gaussian weights"
        );
        assert!(
            wgsl.contains("blur9(in.uv, vec2<f32>(1.0, 0.0))")
                && wgsl.contains("blur9(in.uv, vec2<f32>(0.0, 1.0))"),
            "bloom.wgsl must blur separably (H then V)"
        );
        // The additive composite multiplies the blurred bright buffer by intensity.
        assert!(
            wgsl.contains("c * bloom.params.z"),
            "bloom.wgsl's composite must scale the bloom by intensity before the additive blend"
        );
    }

    /// **GATE (GPU-optional) — the bloom pipeline compiles AND draws** the full three-stage
    /// chain: bright (a real HDR source into half-res `a`), blur H (`a` -> `b`), blur V
    /// (`b` -> `a`), and the additive composite back into the HDR. A malformed `bloom.wgsl`, a
    /// layout mismatch, or a bad blend/write-mask fails HERE, not at app launch. Then it
    /// exercises `forget` — the leak-prevention channel — and asserts the bright cache drops the
    /// id. Skips cleanly with no GPU adapter. (The ground_fog / tonemap compile-test pattern,
    /// extended to issue the whole sequence.)
    #[test]
    fn bloom_pipeline_compiles_and_draws() {
        let Some((device, queue)) =
            crate::pipeline_mesh::tests::test_device("flicker.bloom_test.device")
        else {
            eprintln!("bloom_pipeline_compiles_and_draws: no GPU adapter — skipping");
            return;
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut pipeline = BloomPipeline::new(&device);
        pipeline.set_uniform(&queue, BloomUniform::new(8, 8, 1.0, 0.5, 0.6, 1.0));

        let make = |w: u32, h: u32, label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            (tex, view)
        };
        // Full-res surface hdr (16²) + two half-res scratch (8²).
        let (_hdr, hdr_view) = make(16, 16, "flicker.bloom_test.hdr");
        let (_a, a_view) = make(8, 8, "flicker.bloom_test.a");
        let (_b, b_view) = make(8, 8, "flicker.bloom_test.b");

        let hdr_id = 42;
        pipeline.bind_bright(&device, hdr_id, &hdr_view);
        pipeline.bind_scratch(&device, &a_view, &b_view);

        let mut enc = device.create_command_encoder(&Default::default());
        pipeline.encode(&mut enc, &hdr_view, &a_view, &b_view);
        queue.submit([enc.finish()]);
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "the bloom path failed validation: {err:?}");

        // The leak-prevention channel: forgetting the surface HDR id drops its bright bind and
        // clears `active`, so a re-encode is inert rather than sampling a dead texture.
        assert!(pipeline.active.is_some());
        pipeline.forget(hdr_id);
        assert!(
            pipeline.active.is_none(),
            "forget clears the active surface HDR"
        );
        assert!(
            !pipeline.bright_src.contains_key(&hdr_id),
            "forget drops the bright bind group"
        );
    }
}
