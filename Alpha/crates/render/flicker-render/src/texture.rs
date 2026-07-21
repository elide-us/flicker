//! Texture storage owned by the renderer.
//!
//! Callers receive opaque [`TextureHandle`] values when they upload pixels; the
//! actual `wgpu::Texture`, view, and per-texture bind group live inside the
//! [`crate::Renderer`] and are looked up by index.

use wgpu::util::DeviceExt;

/// Opaque handle to a texture stored on the renderer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureHandle(pub(crate) u32);

/// Internal record kept per loaded texture.
pub(crate) struct LoadedTexture {
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    /// Bind group for the billboard pipeline's texture layout, built by
    /// `Renderer::load_texture` so any texture can be used as a billboard
    /// atlas. `None` until set.
    pub billboard_bind_group: Option<wgpu::BindGroup>,
    /// Bind group for the textured-mesh pipeline's texture layout (linear
    /// sampler), built by `Renderer::load_texture`. `None` until set.
    pub mesh_bind_group: Option<wgpu::BindGroup>,
    #[allow(dead_code)]
    pub size: (u32, u32),
}

impl LoadedTexture {
    /// Wrap an **already-created** texture + view (e.g. an offscreen render target's
    /// colour attachment) as a sampleable texture: builds the sprite-pipeline bind group
    /// so it can be drawn like any uploaded texture. The billboard / mesh bind groups are
    /// added by [`crate::Renderer`] in `register_texture` (as for uploaded textures).
    pub fn from_view(
        device: &wgpu::Device,
        sampler: &wgpu::Sampler,
        layout: &wgpu::BindGroupLayout,
        texture: wgpu::Texture,
        view: wgpu::TextureView,
        size: (u32, u32),
    ) -> Self {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.render_target.bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        Self {
            texture,
            view,
            bind_group,
            billboard_bind_group: None,
            mesh_bind_group: None,
            size,
        }
    }

    /// Upload an RGBA8 **sRGB** pixel buffer (colour data — albedo/sprites) to the GPU
    /// and build a bind group tied to the provided sprite-pipeline layout.
    pub fn from_rgba8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        layout: &wgpu::BindGroupLayout,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Self {
        Self::from_rgba8_format(
            device,
            queue,
            sampler,
            layout,
            pixels,
            width,
            height,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        )
    }

    /// Upload an RGBA8 **linear** pixel buffer (non-colour data — normal / roughness /
    /// metalness / AO maps) to the GPU. Same bind-group wiring as [`Self::from_rgba8`];
    /// only the texture format differs (no sRGB decode), so the raw bytes are sampled
    /// as-is — mandatory for tangent-space normals and scalar PBR maps.
    pub fn from_rgba8_linear(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        layout: &wgpu::BindGroupLayout,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Self {
        Self::from_rgba8_format(
            device,
            queue,
            sampler,
            layout,
            pixels,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
        )
    }

    /// Shared upload path parameterised by texture format (sRGB colour vs linear data).
    #[allow(clippy::too_many_arguments)]
    fn from_rgba8_format(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        layout: &wgpu::BindGroupLayout,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        assert_eq!(
            pixels.len() as u32,
            width * height * 4,
            "RGBA8 pixel buffer length must equal width * height * 4"
        );

        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("flicker.sprite.texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            pixels,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.sprite.bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        Self {
            texture,
            view,
            bind_group,
            billboard_bind_group: None,
            mesh_bind_group: None,
            size: (width, height),
        }
    }
}
