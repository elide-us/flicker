//! Headless pixel-readback test for the 2D layering model.
//!
//! Renders the three 2D pipelines to a 64×64 offscreen target through the same
//! ascending-layer loop as [`Renderer::end_frame`](crate::Renderer) and reads
//! the pixels back, asserting that a higher layer covers a lower one *across*
//! pipelines — including the original bug: a higher-layer **sprite** must cover
//! lower-layer **text** (which the old "all text last" order could not).
//!
//! Requires a GPU adapter; on a machine without one the test skips (a no-op
//! pass) rather than failing, mirroring how `wgpu` headless tests behave in
//! CI. 64 px wide × 4 bytes = 256-byte rows, already the `COPY_BYTES_PER_ROW`
//! alignment, so the readback needs no row padding.

use glam::Vec2;

use crate::pipeline_mesh::create_depth_view;
use crate::pipeline_sprite::SpritePipeline;
use crate::pipeline_text::TextPipeline;
use crate::pipeline_triangle::TrianglePipeline;
use crate::texture::LoadedTexture;
use crate::TextureHandle;

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let row = (SIZE * 4) as usize;
    let o = y as usize * row + x as usize * 4;
    [data[o], data[o + 1], data[o + 2], data[o + 3]]
}

#[test]
fn higher_layer_covers_lower_across_pipelines() {
    let instance = wgpu::Instance::default();
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
    else {
        eprintln!("layering_test: no GPU adapter available — skipping");
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("flicker.layering_test.device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .expect("request device");

    // Offscreen color target + the shared depth attachment the 2D pipelines
    // expect (they bind it but never test/write it).
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("flicker.layering_test.color"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let (_depth, depth_view) = create_depth_view(&device, SIZE, SIZE);

    let mut triangle = TrianglePipeline::new(&device, FORMAT);
    let mut sprite = SpritePipeline::new(&device, FORMAT);
    let mut text = TextPipeline::new(&device, &queue, FORMAT);

    // A 1×1 white texture so sprite tints render as flat colors.
    let white = LoadedTexture::from_rgba8(
        &device,
        &queue,
        &sprite.sampler,
        &sprite.texture_bind_group_layout,
        &[255, 255, 255, 255],
        1,
        1,
    );
    let textures = vec![Some(white)];
    let tex = TextureHandle(0);
    let screen = Vec2::new(SIZE as f32, SIZE as f32);

    // Submit in *layer-inverted* order so the result can only be right if the
    // layer — not submission order — drives z:
    //   - BLUE full-screen sprite at layer 1   (submitted FIRST, must end on top)
    //   - GREEN full-screen sprite at layer 0
    //   - BLACK text at layer 0                 (the "menu text" the bug leaked)
    //   - RED triangle at layer 2 over the corner (cross-pipeline, on top of all)
    sprite.push(screen, tex, Vec2::ZERO, screen, [0.0, 0.0, 1.0, 1.0], 1.0, None);
    sprite.push(screen, tex, Vec2::ZERO, screen, [0.0, 1.0, 0.0, 1.0], 0.0, None);
    text.push("WWWWWW", 2.0, 26.0, 24.0, [0.0, 0.0, 0.0, 1.0], 0.0, crate::FontRole::Body, false, false, -1.0, None, None);
    triangle.push(
        screen,
        Vec2::new(0.0, 0.0),
        Vec2::new(20.0, 0.0),
        Vec2::new(0.0, 20.0),
        [1.0, 0.0, 0.0, 1.0],
        2.0,
    );

    triangle.prepare(&device, &queue);
    sprite.prepare(&device, &queue);
    text.prepare(&device, &queue, SIZE, SIZE)
        .expect("text prepare");

    // The union of layers, ascending — exactly what Renderer::end_frame walks.
    let mut layers: Vec<f32> = Vec::new();
    layers.extend(triangle.layers());
    layers.extend(sprite.layers());
    layers.extend(text.layers());
    layers.sort_by(f32::total_cmp);
    layers.dedup();
    assert_eq!(layers, vec![0.0, 1.0, 2.0], "all three layers present");

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("flicker.layering_test.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Magenta clear: any uncovered pixel is conspicuous.
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 1.0,
                        g: 0.0,
                        b: 1.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        for &layer in &layers {
            triangle.render_layer(&mut pass, layer);
            sprite.render_layer(&mut pass, layer, &textures);
            text.render_layer(&mut pass, layer).expect("text render");
        }
    }

    // Read the target back through a tightly-packed buffer (256-byte rows).
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("flicker.layering_test.readback"),
        size: (SIZE * SIZE * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::Maintain::Wait);
    rx.recv().expect("map channel").expect("map readback");
    let data = slice.get_mapped_range().to_vec();

    let is = |p: [u8; 4], r: u8, g: u8, b: u8| {
        let near = |a: u8, b: u8| (a as i16 - b as i16).abs() <= 8;
        near(p[0], r) && near(p[1], g) && near(p[2], b)
    };

    // Center: where black text was drawn at layer 0 — must now be BLUE (the
    // layer-1 sprite covered it). This is the modal-bleed-through bug, fixed.
    let center = pixel(&data, 32, 38);
    assert!(
        is(center, 0, 0, 255),
        "layer-1 sprite must cover layer-0 text; got {center:?}"
    );

    // A plain layer-1 region (outside the triangle): BLUE on top of layer-0 green.
    let mid = pixel(&data, 50, 50);
    assert!(
        is(mid, 0, 0, 255),
        "layer-1 blue must cover layer-0 green; got {mid:?}"
    );

    // The corner under the layer-2 triangle: RED covers the layer-1 sprite —
    // cross-pipeline (triangle vs sprite) ordering by layer.
    let corner = pixel(&data, 3, 3);
    assert!(
        is(corner, 255, 0, 0),
        "layer-2 triangle must cover layer-1 sprite; got {corner:?}"
    );

    // Nothing left uncovered (no magenta clear showing through).
    for &(x, y) in &[(1u32, 1u32), (63, 0), (0, 63), (63, 63), (32, 32)] {
        let p = pixel(&data, x, y);
        assert!(!is(p, 255, 0, 255), "uncovered pixel at ({x},{y}): {p:?}");
    }

    drop(data);
    readback.unmap();
}
