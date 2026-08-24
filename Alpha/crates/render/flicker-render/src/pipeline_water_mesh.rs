//! Animated water-surface MESH pipeline.
//!
//! A REAL triangle grid (not a screen-space plane), drawn in the opaque pass with the shared
//! `@group(0)` [`FrameBindGroup`] (camera + the frame's light list). The vertex stage lifts
//! each grid vertex to `y = sea_level + Σ waves` — ONE roster summing RADIAL sources (rings
//! from a centre, the near-island chop) and DIRECTIONAL ones (plane waves marching along a
//! world direction, the open-ocean swell that keeps the horizon band moving) — and recomputes
//! the surface normal ANALYTICALLY from the summed wave derivatives; the fragment stage shades
//! an ENVIRONMENT-LIT water body — the rig's analytic sky mirrored along the reflected view ray,
//! Fresnel-blended over a shallow→deep body that is itself lit by the rig's ambient + the SKY
//! SLOTS — plus a REAL specular over those same two slots, `scene.lights[0]` (the sun) and
//! `scene.lights[1]` (the moon) the Celestial cycle writes, so the day sea carries a sun glint
//! and the night sea a moon streak from one lobe and one knob set.
//! Because the sky palette is the LIVE one (handed in at upload from the renderer's rig into
//! this pass's own `@group(1)`, since the shared `Scene` carries no sky lanes) and the ambient
//! floor is read straight off that shared `scene.ambient` rather than copied, the sea breathes
//! with the day/night cycle: aquamarine at noon, gold at sunset, near-black at night, with
//! nothing authored per time of day and one spelling of the ambient. See
//! `shaders/water_mesh.wgsl`.
//!
//! Because it is geometry it **writes and tests depth** (`CompareFunction::Less`,
//! `depth_write_enabled: true`) — it occludes and is occluded by the terrain, unlike the old
//! flat read-only pass — and composites premultiplied "over" the lit scene, so shallow water
//! is translucent. It writes the surface's `hdr` colour, so the `>1` specular survives to the
//! tonemap and bloom. `cull_mode: None` keeps the wavy surface visible from below at the shore.
//!
//! The grid itself is a UNIT grid ([0,1]² in XZ) built ONCE ([`water_grid`]) and uploaded
//! through the normal mesh store as a [`MeshHandle`]; the vertex shader reads it as a
//! SCREEN-space grid and PROJECTS it through the camera inverse onto the sea plane (the
//! projected-grid technique — always tiling the visible water to the horizon, dense near the
//! camera), then overwrites Y + normal, so the uploaded positions are placeholders.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use crate::mesh::{LightRig, MeshHandle, MeshVertex};
use crate::pipeline_mesh::{compose_lit, FrameBindGroup, LoadedMesh, DEPTH_FORMAT};
use crate::pipeline_shadow::ShadowBind;

/// How many wave sources one water surface sums. A fixed roster — empty slots (amplitude 0)
/// cost a `sin`/`cos` each and contribute nothing, and the loop is `count`-bounded so unused
/// slots are skipped. Three radial sources around the island perimeter plus two directional
/// open-ocean swells is the demo, so the roster holds six.
pub const MAX_WAVE_SOURCES: usize = 6;

/// Side count of the water grid — an `N × N` cell grid, `(N+1)²` vertices. This is now a
/// SCREEN grid (the VS projects it onto the sea plane), so the cost is FIXED regardless of how
/// far the ocean reaches; 256 keeps the near-camera water finely tessellated for the glint while
/// the far rows fall on the horizon (and flatten via `wave_falloff`). Art/perf knob.
pub const WATER_GRID_N: u32 = 256;

/// The GEOMETRY of a wave source — the ONE field that differs between the two kinds, so a
/// source can never carry both a centre and a direction (the "one representation, never accept
/// both" law made unrepresentable rather than validated).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaveKind {
    /// RINGS spreading from a world-XZ point: the phase argument is `k · distance(p.xz, center)`,
    /// so the crests curve and the amplitude reads as belonging to *that* place. The near-island
    /// chop.
    Radial {
        /// World XZ the wave radiates from.
        center: Vec2,
    },
    /// A PLANE wave marching along a UNIT XZ direction: the phase argument is `k · dot(p.xz, dir)`,
    /// which is defined *everywhere* (no centre to be far from), so this is the open-ocean swell
    /// that keeps the horizon band moving. Author it LONG (`wavelength` ~120–200) so the far field
    /// swells instead of shimmering.
    Directional {
        /// Unit XZ direction the crests travel along. Normalized at parse — `k · dot(p, dir)` is
        /// only a wavelength when `dir` is unit.
        dir: Vec2,
    },
}

impl Default for WaveKind {
    fn default() -> Self {
        Self::Radial { center: Vec2::ZERO }
    }
}

/// One wave source. Height at a world point `p` is `amplitude · sin(arg − omega·time + phase)`
/// where `arg` is the [`WaveKind`]'s phase argument — a radial sibling of the terrain heightmap's
/// `A·sin(k·x − …)` summation with a `−ω·t` term added, or the literal plane-wave form of it.
/// ONE roster holds both kinds; the shader sums them in ONE loop.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct WaveSource {
    /// Radial (a centre) or directional (a direction) — see [`WaveKind`].
    pub kind: WaveKind,
    /// Peak height contribution (world units). An `amplitude` of 0 is an inert source, so a
    /// padded slot is a no-op.
    pub amplitude: f32,
    /// Angular wavenumber `2π / wavelength` (radians per world unit).
    pub k: f32,
    /// Angular frequency `speed · k` (radians per second) — the `−ω·t` scroll.
    pub omega: f32,
    /// Phase offset (radians), so two sources are not in lockstep.
    pub phase: f32,
}

/// Per-frame parameters for the water surface — the public API input (the renderer turns this
/// into the GPU uniform). The resolved output of
/// [`WaterPass::resolve`](crate::WaterPass::resolve). Distances / heights are world units;
/// colours are linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Water {
    /// World Y the flat water plane sits at (before the waves displace it).
    pub sea_level: f32,
    /// Linear RGB seen at grazing view angles (the surface / shallow tint).
    pub shallow: Vec3,
    /// Linear RGB seen looking straight down (deep water).
    pub deep: Vec3,
    /// Sharpness of the shallow→deep view-angle transition (art knob).
    pub shore_fade: f32,
    /// Specular exponent — larger = tighter, brighter glint. ONE knob for BOTH sky slots (the
    /// sun's daytime glint and the moon's night streak are the same lobe, separated only by
    /// how much radiance each slot carries).
    pub spec_shininess: f32,
    /// Specular strength multiplier — the same one knob over both sky slots.
    pub spec_strength: f32,
    /// Multiplies the wave slope used for SHADING (not the geometry), so the glint choppiness
    /// tunes independently of the wave height.
    pub normal_scale: f32,
    /// Distance falloff `k` for the far-field flattening: a RADIAL source's height + slope are
    /// scaled by `1 / (1 + dist·k)`, so the near-island chop fades to a flat mirror toward the
    /// horizon (kills projected-grid coarseness + sub-pixel shimmer). `0` = waves never
    /// attenuate. DIRECTIONAL sources use the same law at a much gentler rate
    /// (`WATER_AMBIENT_FALLOFF_SCALE` in `water_mesh.wgsl`) so the open ocean keeps swelling
    /// out to the horizon — one dial, two rates.
    pub wave_falloff: f32,
    /// How much of the SKY the surface mirrors, `0..1`, scaling the Fresnel-weighted
    /// environment term. `1.0` = the full Fresnel reflection (a real water surface); `0.0` =
    /// the body colour alone (the pre-environment look). The dial between "aquamarine sea"
    /// and "mirror".
    pub env_strength: f32,
    /// Animation clock (seconds) — pure per-frame input; `0` = still water.
    pub time: f32,
    /// The wave sources; only the first `wave_count` are summed.
    pub waves: [WaveSource; MAX_WAVE_SOURCES],
    /// How many of `waves` are live (`<= MAX_WAVE_SOURCES`).
    pub wave_count: u32,
}

impl Default for Water {
    fn default() -> Self {
        Self {
            sea_level: 0.0,
            shallow: Vec3::new(0.10, 0.30, 0.34),
            deep: Vec3::new(0.02, 0.06, 0.11),
            shore_fade: 4.0,
            spec_shininess: 200.0,
            spec_strength: 1.0,
            normal_scale: 1.0,
            wave_falloff: 0.0015,
            // A full Fresnel sky reflection — what real water does, and what makes the sea
            // read as the sky's colour at grazing angles (blue at noon, gold at sunset).
            env_strength: 1.0,
            time: 0.0,
            waves: [WaveSource::default(); MAX_WAVE_SOURCES],
            wave_count: 0,
        }
    }
}

/// CPU-side mirror of one WGSL `Wave`. The leading XY lane carries the source's GEOMETRY —
/// the `center` of a radial source or the unit `dir` of a directional one, ONE lane read two
/// ways — and `b.z` is the kind flag that says which: `a = (x, y, amplitude, k)`,
/// `b = (omega, phase, kind, _)` with `kind` `0` = radial, `1` = directional.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
struct WaveUniform {
    a: [f32; 4],
    b: [f32; 4],
}

/// CPU-side mirror of the WGSL `Water` uniform. `vec4` lanes for trivial std140.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct WaterMeshUniform {
    /// Camera inverse view-projection — the projected grid unprojects the screen-space vertices
    /// through this onto the sea plane. (The RE-projection uses the shared `@group(0)` camera.)
    inv_view_proj: [[f32; 4]; 4],
    /// `(x, y, z, _)` world camera position — the VS ray origin. The shared `scene` uniform is
    /// FRAGMENT-only in the frame layout, so the VS cannot read `scene.camera_pos`; it reads this
    /// copy instead (the fog/volumetric uniforms carry `camera_pos` the same way).
    camera_pos: [f32; 4],
    /// `(sea_level, shore_fade, spec_shininess, spec_strength)`.
    params0: [f32; 4],
    /// `(time, normal_scale, wave_falloff, env_strength)`.
    params1: [f32; 4],
    /// `(r, g, b, _)` shallow.
    shallow: [f32; 4],
    /// `(r, g, b, _)` deep.
    deep: [f32; 4],
    /// `(r, g, b, _)` — the LIVE rig's procedural-sky colour straight up. The FS mixes it
    /// with `sky_horizon` along the reflected view ray, so the sea mirrors the same analytic
    /// sky the sky pass paints (see `LightRig::sky_zenith`).
    sky_zenith: [f32; 4],
    /// `(r, g, b, _)` — the LIVE rig's sky colour at the horizon band.
    ///
    /// There is deliberately NO `ambient` lane after this one. The ambient floor the water body
    /// is lit by is the SHARED frame one — [`SceneUniform::ambient`], filled from the same
    /// [`LightRig`] by `rig_to_uniform` — and the water FS is already bound to that `Scene` at
    /// `@group(0)`, so it reads `scene.ambient.rgb` directly. A copy here would be a bit-for-bit
    /// duplicate of a number the shader can already see, i.e. a second spelling that can drift.
    /// The sky palette above has no such shared home (`Scene` carries no sky lanes), which is
    /// exactly why those two lanes stay.
    sky_horizon: [f32; 4],
    /// `(wave_count, _, _, _)`.
    counts: [u32; 4],
    waves: [WaveUniform; MAX_WAVE_SOURCES],
}

const WATER_UNIFORM_SIZE: u64 = std::mem::size_of::<WaterMeshUniform>() as u64;

// std140 layout gate: the leading `mat4x4` is four 16-byte lanes, every other header member is
// one, and each `Wave` is two of them, so the array needs no inter-element padding. Pinned
// offsets keep the Rust order in step with the WGSL `struct Water` (whose field order the wgsl
// gate below asserts as text).
const _: () = assert!(std::mem::size_of::<WaveUniform>() == 32);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, inv_view_proj) == 0);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, camera_pos) == 64);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, params0) == 80);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, params1) == 96);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, shallow) == 112);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, deep) == 128);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, sky_zenith) == 144);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, sky_horizon) == 160);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, counts) == 176);
const _: () = assert!(std::mem::offset_of!(WaterMeshUniform, waves) == 192);
const _: () = assert!(WATER_UNIFORM_SIZE == 192 + 32 * MAX_WAVE_SOURCES as u64);

impl Default for WaterMeshUniform {
    fn default() -> Self {
        Self::from_params(
            &Water::default(),
            Mat4::IDENTITY,
            Vec3::ZERO,
            &LightRig::default(),
        )
    }
}

impl WaterMeshUniform {
    /// Build the GPU uniform from the per-frame params + the camera's inverse view-projection and
    /// world position (the projected grid casts a ray from `camera_pos` through the unprojected
    /// screen vertex onto the sea plane; the RE-projection uses the shared `@group(0)` camera,
    /// and the fragment shader reads the sun out of the shared `Scene` uniform). Extends the
    /// fog/volumetric `from_params(_, inv_view_proj, camera_pos)` signature with the LIVE
    /// [`LightRig`].
    ///
    /// The rig's `sky_zenith` / `sky_horizon` ride along because the water is ENVIRONMENT-lit:
    /// the FS mirrors that sky palette along the reflected view ray. They ride HERE because the
    /// shared `Scene` has no sky lanes to read them from; the ambient floor, which `Scene` DOES
    /// carry, is read there instead of copied. A celestial cycle rewrites the rig every frame,
    /// so the sea breathes with the day for free — the fog's "take the renderer's LIVE colour"
    /// idiom, one pass along. `@group(0)` is untouched.
    pub fn from_params(p: &Water, inv_view_proj: Mat4, camera_pos: Vec3, rig: &LightRig) -> Self {
        let mut waves = [WaveUniform::default(); MAX_WAVE_SOURCES];
        let count = (p.wave_count as usize).min(MAX_WAVE_SOURCES);
        for (dst, src) in waves.iter_mut().zip(p.waves.iter()).take(count) {
            // The kind picks WHICH vector rides the leading lane and sets the flag the shader
            // branches on — one packing, no parallel array.
            let (xy, kind) = match src.kind {
                WaveKind::Radial { center } => (center, 0.0),
                WaveKind::Directional { dir } => (dir, 1.0),
            };
            *dst = WaveUniform {
                a: [xy.x, xy.y, src.amplitude, src.k],
                b: [src.omega, src.phase, kind, 0.0],
            };
        }
        Self {
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 0.0],
            params0: [p.sea_level, p.shore_fade, p.spec_shininess, p.spec_strength],
            params1: [p.time, p.normal_scale, p.wave_falloff, p.env_strength],
            shallow: [p.shallow.x, p.shallow.y, p.shallow.z, 0.0],
            deep: [p.deep.x, p.deep.y, p.deep.z, 0.0],
            sky_zenith: [rig.sky_zenith.x, rig.sky_zenith.y, rig.sky_zenith.z, 0.0],
            sky_horizon: [rig.sky_horizon.x, rig.sky_horizon.y, rig.sky_horizon.z, 0.0],
            counts: [count as u32, 0, 0, 0],
            waves,
        }
    }
}

/// Build the water grid: an `n × n` cell UNIT grid in XZ (`[0,1]²`), `(n+1)²` vertices, at
/// `y = 0` with `+Y` normals. Both Y and the normal are PLACEHOLDERS — the vertex shader reads
/// the XZ as SCREEN space, projects it through the camera inverse onto the sea plane, and
/// overwrites Y (`sea_level + waves`) and the normal (analytic). Uploaded ONCE via
/// `Renderer::upload_mesh`; the same handle is drawn every frame.
pub fn water_grid(n: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    let n = n.max(1);
    let side = n + 1;
    let mut vertices = Vec::with_capacity((side * side) as usize);
    for z in 0..side {
        for x in 0..side {
            vertices.push(MeshVertex {
                position: [x as f32 / n as f32, 0.0, z as f32 / n as f32],
                normal: [0.0, 1.0, 0.0],
                material: 0,
            });
        }
    }
    let mut indices = Vec::with_capacity((n * n * 6) as usize);
    for z in 0..n {
        for x in 0..n {
            let i0 = z * side + x;
            let i1 = i0 + 1;
            let i2 = i0 + side;
            let i3 = i2 + 1;
            // Two CCW triangles per cell (cull_mode is None, so winding is not load-bearing).
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }
    (vertices, indices)
}

/// The water-mesh pipeline. `@group(0)` = the shared frame group (camera + light list);
/// `@group(1)` = the water uniform (a static bind — no per-id texture cache, unlike the fog);
/// `@group(2)` = the shared shadow bind (a disabled default, present only to satisfy the
/// prelude's `shadow_factor`). Baked for both colour formats (swapchain + [`crate::HDR_FORMAT`]);
/// [`Self::render`] selects by [`crate::TargetColor`].
pub struct WaterMeshPipeline {
    pipeline: [wgpu::RenderPipeline; 2],
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl WaterMeshPipeline {
    pub fn new(
        device: &wgpu::Device,
        frame: &FrameBindGroup,
        shadow: &ShadowBind,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.water_mesh.shader"),
            source: wgpu::ShaderSource::Wgsl(
                compose_lit(include_str!("shaders/water_mesh.wgsl")).into(),
            ),
        });

        let water_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.water_mesh.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(WATER_UNIFORM_SIZE),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.water_mesh.pipeline_layout"),
            // @group(2) = the shared shadow bind (disabled default) — the prelude references it.
            bind_group_layouts: &[frame.layout(), &water_layout, shadow.layout()],
            push_constant_ranges: &[],
        });

        // The grid uploads as `MeshVertex`; the shader reads only the position (location 0),
        // so the layout declares just that over the full `MeshVertex` stride.
        let vertex_attrs = [wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }];
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vertex_attrs,
        };

        let make = |fmt: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.water_mesh.pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: std::slice::from_ref(&vertex_layout),
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: fmt,
                        // Premultiplied "over": `out = src.rgb + dst·(1−src.a)`.
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    // A wavy surface is seen from below at the shore — draw both faces.
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                // REAL geometry: occludes (writes depth) and is occluded (tests `Less`).
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipeline = [make(surface_format), make(crate::HDR_FORMAT)];

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.water_mesh.uniform"),
            size: WATER_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.water_mesh.bind_group"),
            layout: &water_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            uniform_buf,
            bind_group,
        }
    }

    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: WaterMeshUniform) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Draw the water grid `handle` into the current colour + depth. `frame` is the renderer's
    /// ONE per-frame group (camera + lights) at slot 0; `shadow` supplies the disabled default
    /// at slot 2; `target` selects the colour-format variant.
    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frame: &'a FrameBindGroup,
        shadow: &'a ShadowBind,
        meshes: &'a [Option<LoadedMesh>],
        handle: MeshHandle,
        target: crate::TargetColor,
    ) {
        let Some(mesh) = meshes.get(handle.0 as usize).and_then(|s| s.as_ref()) else {
            return; // the grid was not uploaded / was freed
        };
        pass.set_pipeline(&self.pipeline[target as usize]);
        pass.set_bind_group(0, frame.bind_group(), &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.set_bind_group(2, shadow.active_bind_group(), &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer().slice(..));
        pass.set_index_buffer(mesh.tri_index_buffer().slice(..), mesh.tri_index_format());
        pass.draw_indexed(0..mesh.tri_index_count(), 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CPU mirror of the shader's wave summation — height + analytic XZ derivatives at a
    /// world point, over BOTH wave kinds. The WGSL `vs_main` loop is the source of truth; this
    /// exists ONLY to assert that math holds (a source-distance / plane-wave height, a unit
    /// normal), never as a second renderer. Distance attenuation is the shader's alone (it needs
    /// the camera) and is deliberately NOT mirrored here.
    fn sample_waves(waves: &[WaveSource], x: f32, z: f32, t: f32) -> (f32, f32, f32) {
        let mut h = 0.0;
        let mut dhdx = 0.0;
        let mut dhdz = 0.0;
        for s in waves {
            // The kind picks the phase argument and the direction its gradient runs along —
            // the ONE branch the shader takes, mirrored.
            let (arg_pos, grad) = match s.kind {
                WaveKind::Radial { center } => {
                    let dx = x - center.x;
                    let dz = z - center.y;
                    let d = (dx * dx + dz * dz).sqrt().max(1e-3);
                    (d, Vec2::new(dx / d, dz / d))
                }
                WaveKind::Directional { dir } => (Vec2::new(x, z).dot(dir), dir),
            };
            let arg = s.k * arg_pos - s.omega * t + s.phase;
            h += s.amplitude * arg.sin();
            let c = s.amplitude * s.k * arg.cos();
            dhdx += c * grad.x;
            dhdz += c * grad.y;
        }
        (h, dhdx, dhdz)
    }

    /// CPU mirror of the VS projected-grid step (the WGSL `vs_main` is the source of truth; this
    /// exists ONLY to assert the projection holds — a screen-centre vertex lands ON the sea
    /// plane, a horizon vertex clamps to the camera's far distance — never as a second renderer).
    /// Returns the world XZ the grid vertex maps to; the caller's Y is always `sea_level + waves`.
    fn project_grid_to_sea(
        inv_view_proj: Mat4,
        camera_pos: Vec3,
        sea_level: f32,
        ndc: Vec2,
    ) -> Vec2 {
        let far4 = inv_view_proj * glam::Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let far = far4.truncate() / far4.w;
        let to_far = far - camera_pos;
        let t_far = to_far.length().max(1e-4);
        let rd = to_far / t_far;
        let denom = rd.y;
        let t_plane = (sea_level - camera_pos.y) / denom;
        let hits = denom.abs() > 1e-4 && t_plane > 0.0;
        let t = if hits { t_plane.min(t_far) } else { t_far };
        let hit = camera_pos + rd * t;
        Vec2::new(hit.x, hit.z)
    }

    /// **GATE — the projected grid maps screen space onto the sea plane** (the CPU mirror of the
    /// VS calculus, reusing the SAME `Mat4` the pipeline uploads). A screen-centre vertex
    /// unprojects to a world XZ that reprojects back to the screen centre — i.e. it lands ON
    /// `y = sea_level` — and a top-row vertex (looking above the horizon) clamps to a far
    /// horizontal distance with no NaN. This is the load-bearing new mechanism; if the
    /// unproject / plane-intersect / horizon-clamp drifts, water stops reaching the horizon.
    #[test]
    fn the_projected_grid_maps_screen_to_the_sea_plane() {
        // The camera the pipeline builds: `perspective_rh · look_at_rh` (wgpu z∈[0,1]). Placed
        // above the sea, looking down and forward at it.
        let eye = Vec3::new(384.0, 220.0, 384.0);
        let target = Vec3::new(384.0, 120.0, 760.0);
        let aspect = 16.0 / 9.0;
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);
        let proj = Mat4::perspective_rh(60f32.to_radians(), aspect, 0.5, 5000.0);
        let vp = proj * view;
        let inv = vp.inverse();
        let sea = 120.0;
        let eye_xz = Vec2::new(eye.x, eye.z);

        // A screen-CENTRE vertex unprojects ONTO the sea plane: reprojecting the world hit (with
        // Y = sea_level) lands back at the screen centre, and the hit is finite (no NaN).
        let hit_c = project_grid_to_sea(inv, eye, sea, Vec2::ZERO);
        assert!(
            hit_c.is_finite(),
            "centre hit is finite (no NaN): {hit_c:?}"
        );
        let clip = vp * glam::Vec4::new(hit_c.x, sea, hit_c.y, 1.0);
        let ndc_back = Vec2::new(clip.x / clip.w, clip.y / clip.w);
        assert!(
            clip.w > 0.0 && ndc_back.length() < 1e-3,
            "the centre grid vertex lands ON the sea plane (reprojects to screen centre): {ndc_back:?}"
        );

        // A TOP-ROW vertex (overscanned above the screen top → above the horizon) has NO forward
        // plane hit, so it clamps to a far HORIZONTAL distance — finite, and far beyond the
        // centre hit (it hugs the horizon line), never a NaN or geometry above the water.
        let hit_top = project_grid_to_sea(inv, eye, sea, Vec2::new(0.0, 1.06));
        assert!(
            hit_top.is_finite(),
            "the horizon-clamped vertex is finite (no NaN): {hit_top:?}"
        );
        let d_center = (hit_c - eye_xz).length();
        let d_top = (hit_top - eye_xz).length();
        assert!(
            d_top > d_center * 4.0,
            "the top row hugs the far horizon (much farther than the centre hit): \
             top={d_top}, centre={d_center}"
        );
    }

    /// **GATE — the grid builds a well-formed unit mesh.** `(n+1)²` vertices, every one on the
    /// `y = 0` placeholder plane with a `+Y` placeholder normal (the VS lifts + re-normals),
    /// `n²·6` indices, and every index in range. A malformed grid would draw garbage or panic
    /// the upload; this catches it CPU-side, no GPU.
    #[test]
    fn the_water_grid_is_a_planar_unit_mesh() {
        let n = 8u32;
        let (verts, idx) = water_grid(n);
        assert_eq!(
            verts.len(),
            ((n + 1) * (n + 1)) as usize,
            "vertex count is (n+1)²"
        );
        assert_eq!(idx.len(), (n * n * 6) as usize, "two triangles per cell");
        for v in &verts {
            assert_eq!(
                v.position[1], 0.0,
                "pre-displacement Y is the 0 placeholder"
            );
            assert_eq!(v.normal, [0.0, 1.0, 0.0], "planar +Y placeholder normal");
            assert!((0.0..=1.0).contains(&v.position[0]) && (0.0..=1.0).contains(&v.position[2]));
        }
        let max = verts.len() as u32;
        assert!(idx.iter().all(|&i| i < max), "every index is in range");
    }

    /// **GATE — the wave summation is the height + a unit normal it claims, for BOTH kinds.**
    /// At a known distance from a RADIAL source the height is `amplitude·sin(k·d − ω·t + phase)`;
    /// for a DIRECTIONAL source it is the plane wave `amplitude·sin(k·(p·dir) − ω·t + phase)`
    /// evaluated at a point with no centre to be near, and its XZ gradient is
    /// `amplitude·k·cos(arg)·dir` — the simpler derivative the shader's directional branch
    /// relies on. The analytic normal `normalize(-dhdx, 1, -dhdz)` is unit for any slope. All
    /// three are checked against the CPU mirror of the shader's ONE loop.
    #[test]
    fn the_wave_sum_is_a_height_and_a_unit_normal() {
        let center = Vec2::new(300.0, 384.0);
        let radial = WaveSource {
            kind: WaveKind::Radial { center },
            amplitude: 1.5,
            k: std::f32::consts::TAU / 60.0,
            omega: 0.8,
            phase: 0.3,
        };
        // A point exactly `d` from the centre along +X.
        let d = 45.0f32;
        let (x, z, t) = (center.x + d, center.y, 2.0);
        let (h, dhdx, dhdz) = sample_waves(&[radial], x, z, t);
        let want = radial.amplitude * (radial.k * d - radial.omega * t + radial.phase).sin();
        assert!((h - want).abs() < 1e-4, "radial height h={h} want={want}");
        let normal = Vec3::new(-dhdx, 1.0, -dhdz).normalize();
        assert!(
            (normal.length() - 1.0).abs() < 1e-5,
            "the analytic normal is unit"
        );
        assert!(normal.y > 0.0, "the surface faces up");

        // A DIRECTIONAL (ambient) source: a long swell marching along a unit XZ direction. Its
        // height is the plane wave at THIS point — no centre anywhere in it — and its gradient
        // is `amp·k·cos(arg)·dir`, which is what lets the open ocean keep moving at any range.
        let dir = Vec2::new(0.6, 0.8); // already unit; the parser normalizes authored dirs
        let ambient = WaveSource {
            kind: WaveKind::Directional { dir },
            amplitude: 0.45,
            k: std::f32::consts::TAU / 160.0,
            omega: 0.5,
            phase: 1.1,
        };
        let (hx, hz) = (1024.0f32, -512.0f32); // far from every centre in this test
        let (dh, ddx, ddz) = sample_waves(&[ambient], hx, hz, t);
        let arg = ambient.k * Vec2::new(hx, hz).dot(dir) - ambient.omega * t + ambient.phase;
        let want_h = ambient.amplitude * arg.sin();
        assert!(
            (dh - want_h).abs() < 1e-4,
            "directional height is the plane wave amp·sin(k·(p·dir) − ω·t + φ): {dh} vs {want_h}"
        );
        let c = ambient.amplitude * ambient.k * arg.cos();
        assert!(
            (ddx - c * dir.x).abs() < 1e-6 && (ddz - c * dir.y).abs() < 1e-6,
            "the directional derivative is amp·k·cos(arg)·dir: ({ddx}, {ddz}) vs \
             ({}, {})",
            c * dir.x,
            c * dir.y
        );
        let dn = Vec3::new(-ddx, 1.0, -ddz).normalize();
        assert!(
            (dn.length() - 1.0).abs() < 1e-5 && dn.y > 0.0,
            "the directional analytic normal is unit and faces up"
        );

        // ONE roster, ONE summed loop: the mixed sum is exactly the two evaluated separately —
        // the property that lets a scene author radial chop and ambient swell side by side.
        let (mh, mdx, mdz) = sample_waves(&[radial, ambient], x, z, t);
        let (ah, adx, adz) = sample_waves(&[ambient], x, z, t);
        assert!(
            (mh - (h + ah)).abs() < 1e-4
                && (mdx - (dhdx + adx)).abs() < 1e-4
                && (mdz - (dhdz + adz)).abs() < 1e-4,
            "the mixed roster sums both kinds in one loop"
        );
    }

    /// **GATE — the shipped `water_mesh.wgsl` is ENVIRONMENT-lit** (the real channel — the
    /// shipped text, not a Rust re-derivation). The body used to be a flat authored colour ramp,
    /// which read wrong the moment the sun moved: the same aquamarine at midnight as at noon.
    /// Four things fix that and each is asserted here — the water uniform CARRIES the live sky
    /// palette, the FS mirrors the sky along the REFLECTED view ray under the same
    /// horizon compression `sky.wgsl` paints with, that mirror is blended in by the FRESNEL term
    /// (so it strengthens toward the horizon) under the `env_strength` dial, and the body ramp is
    /// multiplied by an `ambient + sun·N·L` diffuse. Delete any one and the sea stops breathing
    /// with the cycle.
    ///
    /// The horizon-compression check is a **TWO-END** gate: it reads BOTH shipped shaders and
    /// asserts each carries its half of the same gradient law. Asserting only the water side let
    /// somebody retune `sky.wgsl`'s exponent and leave the sea mirroring a sky that no longer
    /// exists, with every test still green — the drift the gate is supposed to catch travels
    /// through the OTHER file (rule 8634C200). Now retuning either end breaks it, which is the
    /// prompt to retune both.
    #[test]
    fn water_mesh_wgsl_is_environment_lit_by_the_live_sky() {
        let wgsl = include_str!("shaders/water_mesh.wgsl");
        // The uniform carries the LIVE rig's SKY palette — and only that. The ambient floor is
        // NOT duplicated here: `Scene` already carries it, so the FS reads it there (below).
        assert!(
            wgsl.contains("sky_zenith: vec4<f32>") && wgsl.contains("sky_horizon: vec4<f32>"),
            "the water uniform must carry the live sky palette"
        );
        assert!(
            !wgsl.contains("    ambient: vec4<f32>,"),
            "the water uniform must NOT carry its own `ambient` lane — that is a bit-copy of \
             `Scene.ambient`, which this shader is already bound to (rule 405F7034: one \
             spelling of a number, not two that can drift)"
        );
        // The REFLECTED view ray, and the analytic sky along it — the same
        // `mix(horizon, zenith, pow(h, 0.5))` gradient sky.wgsl paints, so the sea mirrors the
        // ACTUAL sky (sunset included) rather than a second gradient of its own.
        assert!(
            wgsl.contains("reflect(-v, n)")
                && wgsl.contains(
                    "mix(water.sky_horizon.rgb, water.sky_zenith.rgb, pow(saturate(refl.y), 0.5))"
                ),
            "the FS must mirror the analytic sky along the reflected view ray"
        );
        // ── THE OTHER END ── the sky pass's own gradient, in the shipped `sky.wgsl`: the same
        // `pow(h, 0.5)` compression over the same horizon→zenith mix. Retune the exponent or
        // flip the mix on EITHER side and this fails, so the mirror and the sky are retuned
        // together or not at all.
        let sky = include_str!("shaders/sky.wgsl");
        assert!(
            sky.contains("let grad = pow(h, 0.5);")
                && sky.contains("mix(sky.horizon.rgb, sky.zenith.rgb, grad)"),
            "sky.wgsl must still paint `mix(horizon, zenith, pow(h, 0.5))` — the water's \
             environment mirror above is a copy of THIS law, and a gate on only one end lets \
             the two drift apart silently"
        );
        // Fresnel-weighted, under the `env_strength` art dial.
        assert!(
            wgsl.contains("mix(body, env, fresnel * env_strength)"),
            "the environment must blend in by the Fresnel term, dialled by env_strength"
        );
        // The body ramp is LIT (the SHARED frame ambient floor + BOTH sky slots' diffuse,
        // accumulated in the one slot loop), so it darkens at night — faintly moonlit, not black.
        assert!(
            wgsl.contains("var lit = scene.ambient.rgb;")
                && wgsl.contains("lit = lit + radiance * max(dot(n, l), 0.0);")
                && wgsl.contains("water.deep.rgb, depth_frac) * lit"),
            "the shallow→deep body must be lit by the SHARED scene ambient + the sky slots' \
             diffuse"
        );
    }

    /// **GATE — the shipped `water_mesh.wgsl` carries the mechanism it claims** (the real
    /// channel — assert the shipped text, not a Rust re-derivation). The projected grid, the two
    /// wave kinds and their attenuations, the analytic normal, and the specular summed over BOTH
    /// sky slots are what this pass exists to do; deleting any breaks this gate.
    #[test]
    fn water_mesh_wgsl_ships_waves_normal_and_sky_slot_specular() {
        let wgsl = include_str!("shaders/water_mesh.wgsl");
        // The PROJECTED GRID: unproject the screen-space grid through the camera inverse, then
        // intersect the view ray with the sea plane (`y = sea_level`) — this is what makes the
        // ocean reach the horizon instead of spanning a fixed field box.
        assert!(
            wgsl.contains("water.inv_view_proj * vec4<f32>(ndc")
                && wgsl.contains("(sea_level - ro.y) / denom"),
            "the VS must unproject the screen grid and intersect the sea plane"
        );
        // The far-field flattening, at TWO rates off the ONE authored dial: the radial chop
        // flattens at `1/(1 + dist·wave_falloff)`, the directional swell at a fraction of that
        // rate — delete the second and the horizon band is a dead glass mirror again.
        assert!(
            wgsl.contains("let dist = length(hit.xz - ro.xz);")
                && wgsl.contains("let atten_radial = 1.0 / (1.0 + dist * falloff);")
                && wgsl.contains(
                    "let atten_ambient = 1.0 / (1.0 + dist * falloff * \
                     WATER_AMBIENT_FALLOFF_SCALE);"
                ),
            "the VS must attenuate radial waves hard and directional waves GENTLY with distance"
        );
        // The vertex displacement: `y = sea_level + Σ amplitude·sin(arg − ω·t + …)` over ONE
        // roster, with the RADIAL argument (k·distance) and the DIRECTIONAL one (k·(p·dir)).
        assert!(
            wgsl.contains("k * d - omega * time")
                && wgsl.contains("k * dot(vec2<f32>(wx, wz), dir) - omega * time")
                && wgsl.contains("amp * sin(arg)"),
            "the VS must sum BOTH radial and directional (plane) waves scrolled by -omega*time"
        );
        // The analytic normal from the summed derivatives — one `grad` per kind (the radial
        // unit vector, or the directional `dir` itself).
        assert!(
            wgsl.contains("amp * k * cos(arg)")
                && wgsl.contains("grad = vec2<f32>(dx / d, dz / d);")
                && wgsl.contains("grad = dir;")
                && wgsl.contains("vec3<f32>(-dhdx"),
            "the VS must build the normal from the analytic wave derivatives of both kinds"
        );
        // The roster length in the shader is the SAME constant Rust packs against — a bare
        // literal here is exactly how a layout drifts silently into garbage waves.
        assert!(
            wgsl.contains(&format!("array<Wave, {MAX_WAVE_SOURCES}>")),
            "the WGSL wave roster must be MAX_WAVE_SOURCES ({MAX_WAVE_SOURCES}) long"
        );
        // The REAL specular over BOTH SKY SLOTS: one Blinn lobe, one knob set, summed over
        // `scene.lights[0]` (sun) and `scene.lights[1]` (moon) under a count-bounded loop, so
        // the day sea glints and the night sea carries a moon streak.
        assert!(
            wgsl.contains("let sky_slots = min(scene.counts.x, 2u);")
                && wgsl.contains("for (var i = 0u; i < sky_slots; i = i + 1u)")
                && wgsl.contains("let li = scene.lights[i];")
                && wgsl.contains("light_sample(li")
                && wgsl.contains("spec_sum = spec_sum + radiance * pow(max(dot(n, half_vec), 0.0), spec_shininess);"),
            "the FS must sum one specular lobe over BOTH sky slots (sun 0 + moon 1)"
        );
    }

    /// **GATE (GPU-optional) — the water-mesh pipeline compiles AND draws** a real grid into an
    /// offscreen HDR colour + depth, binding the shared frame group, the water uniform, and the
    /// shadow default. A malformed `water_mesh.wgsl`, a layout mismatch, or a bad vertex layout
    /// fails HERE, not at app launch. Skips cleanly with no GPU adapter. (The ground_fog /
    /// tonemap compile-test pattern, extended to actually issue the indexed draw.)
    #[test]
    fn water_mesh_pipeline_compiles_and_draws() {
        let Some((device, queue)) =
            crate::pipeline_mesh::tests::test_device("flicker.water_mesh_test.device")
        else {
            eprintln!("water_mesh_pipeline_compiles_and_draws: no GPU adapter — skipping");
            return;
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let frame = FrameBindGroup::new(&device);
        let shadow = ShadowBind::new(&device);
        let pipeline = WaterMeshPipeline::new(&device, &frame, &shadow, fmt);
        pipeline.set_uniform(&queue, WaterMeshUniform::default());

        // Upload a tiny grid the way the renderer does (through the mesh pipeline's uploader).
        let mesh_pipeline = crate::pipeline_mesh::MeshPipeline::new(
            &device,
            &frame,
            &shadow,
            fmt,
            device.limits().min_uniform_buffer_offset_alignment,
        );
        let (verts, idx) = water_grid(4);
        let loaded = mesh_pipeline.upload(&device, &verts, crate::mesh::MeshIndices::U32(&idx));
        let meshes = [Some(loaded)];

        let make_tex = |f, usage, label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 16,
                    height: 16,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: f,
                usage,
                view_formats: &[],
            })
        };
        let color = make_tex(
            fmt,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            "flicker.water_mesh_test.color",
        );
        let depth = make_tex(
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            "flicker.water_mesh_test.depth",
        );
        let cview = color.create_view(&Default::default());
        let dview = depth.create_view(&Default::default());
        frame.set_camera_matrix(&queue, glam::Mat4::IDENTITY);
        frame.set_scene_uniform(&queue, crate::pipeline_mesh::SceneUniform::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.water_mesh_test.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cview,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dview,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pipeline.render(
                &mut pass,
                &frame,
                &shadow,
                &meshes,
                MeshHandle(0),
                crate::TargetColor::Srgb,
            );
        }
        queue.submit([enc.finish()]);
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "the water-mesh path failed validation: {err:?}"
        );
    }
}
