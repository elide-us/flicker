//! stage — the typed STAGE DEFINITION, plus the pure line geometry its layers need.
//!
//! A nested `surface` node names a stage SOURCE — `stages.<source>` in the loaded styles
//! (the shared `ui_stages.json` satellite, or the scene file that owns it). The source says
//! WHAT the offscreen sub-scene renders: the lighting it is seen by, the backdrop it sits
//! on, how it is framed, and an ordered list of content layers. The node says WHERE it
//! lands, and `FrameGraph` decides WHEN. [`StageDef`] is that WHAT as one typed value,
//! compiled from JSON by the ONE stage compiler in `flicker-widgets` (`stages.rs`) and
//! consumed by every filler — the loomforge doll rig, the globe, the Sablework lit sample.
//! (The three private parsers it replaced each knew a third of the vocabulary and
//! warned-and-skipped the rest.)
//!
//! Every layer KIND the engine draws is a [`StageLayer`] variant. A filler draws the kinds
//! it understands and NAMES the ones it does not ([`StageDef::layers_outside`]), so an
//! authored layer that nothing draws is a warning, never silence (rule 4BB12A75).
//!
//! **The recipe.** `layers` say what the CONTENT closure draws; [`StageDef::passes`] say
//! which of the engine's whole-surface passes run around it, what each writes, and what
//! each reads — the sky behind, the volumetric disk and the ground fog over. That is a
//! [`PassDef`] list over the surface's [`Attachments`], ordered by
//! [`StageDef::pass_order`] (read-after-write; declaration order is the tie-break), and
//! executed by [`FrameGraph::surface`](crate::FrameGraph::surface). A stage authoring no
//! passes gets the DEFAULT recipe — one [`PassKind::Scene`] — which is exactly where the
//! content closure has always run, so every stage written before recipes existed renders
//! unchanged. Per-frame numbers the simulation owns (a formation clock, a floor height,
//! the disk's gaps) arrive as [`StageInputs`] and REPLACE the authored field, so the
//! recipe compiles once at load and nothing re-parses JSON per frame.
//!
//! The line-geometry kinds (`ring`, `grid`) are PURE FUNCTIONS here — no GPU, no renderer
//! state — so a scene builds them once and can unit-test the result without a device.
//! Feed the output to [`Renderer::draw_lines`](crate::Renderer::draw_lines) (depth-tested,
//! under the character) or `draw_lines_overlay` (always visible).
//!
//! Salvaged from the retired `flicker-packeditor`, which drew the same gold ground ring
//! under every character on its field.

use glam::{Vec2, Vec3};

use crate::{Camera, GroundFog, LightRig, VolumetricDisk, Water, WaveSource, MAX_WAVE_SOURCES};

/// The authored ORBIT framing of a stage — its `camera` block. Absent on a stage whose
/// view belongs to the scene (the globes: the maintainer flies the planet, so the stage
/// authors only the light it is seen by and the backdrop it sits on).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    /// Height of the orbit target above the floor — a portrait looks at the chest, a
    /// material sample at the origin.
    pub target_y: f32,
}

impl Default for StageCamera {
    /// The portrait framing — the doll rig's framing before framing was authored data.
    fn default() -> Self {
        Self {
            yaw: 0.55,
            pitch: 0.18,
            dist: 2.6,
            target_y: 0.95,
        }
    }
}

impl StageCamera {
    /// The orbit camera this framing describes, looking at `(0, target_y, 0)`.
    pub fn camera(&self) -> Camera {
        Camera::orbit(
            Vec3::new(0.0, self.target_y, 0.0),
            self.dist,
            self.yaw,
            self.pitch,
        )
    }
}

/// One authored content layer of a stage (`stages.<source>.layers[]`), in the engine's
/// whole vocabulary. A filler draws the variants it knows.
#[derive(Clone, Debug, PartialEq)]
pub enum StageLayer {
    /// The posed character — the doll rig's skinned mesh.
    Skinned,
    /// The ground ring under a doll, lit in `color_active` when its slot is selected.
    Ring {
        radius: f32,
        y: f32,
        segments: usize,
        color: [f32; 4],
        color_active: [f32; 4],
    },
    /// The faint floor grid under a wider shot.
    Grid {
        spacing: f32,
        extent: f32,
        y: f32,
        color: [f32; 4],
    },
    /// The scene's OWN shell list draws here — the sparse, per-cell-coloured stack a
    /// simulation publishes, which no style file could author.
    Shells,
    /// A STATIC shell of a globe's tiling, authored whole: how far out it sits (a
    /// fraction of the globe radius), how far each tile's corners pull toward its centre,
    /// and the one colour it is drawn in. Two of these — a near-black full shell under
    /// an inset coloured one — are how a hex map gets outlines with no per-edge geometry.
    Shell {
        radius_scale: f32,
        inset: f32,
        color: [f32; 3],
    },
    /// The shared reference frame over a globe at this fraction of its radius.
    Graticule { radius_scale: f32 },
    /// The material sample — the lit body the Sablework bench dresses in its six maps.
    Material,
}

impl StageLayer {
    /// Every `draw` kind the engine knows, spelled as authored.
    pub const KINDS: &'static [&'static str] = &[
        "skinned",
        "ring",
        "grid",
        "shells",
        "shell",
        "graticule",
        "material",
    ];

    /// The authored `draw` spelling of this layer.
    pub fn kind(&self) -> &'static str {
        match self {
            StageLayer::Skinned => "skinned",
            StageLayer::Ring { .. } => "ring",
            StageLayer::Grid { .. } => "grid",
            StageLayer::Shells => "shells",
            StageLayer::Shell { .. } => "shell",
            StageLayer::Graticule { .. } => "graticule",
            StageLayer::Material => "material",
        }
    }
}

/// What one [`Attachment`] of a surface holds. `Surface` is the swapchain-format colour
/// every surface has had since there were surfaces; `Depth32` is its depth; `Rgba16f` is
/// the LINEAR HDR working colour a `tonemap_grade` resolves.
///
/// **This is the ONE authority on what a declared format IS to wgpu** —
/// [`Self::texture_format`] is the only mapping, and [`crate::HDR_FORMAT`] is defined
/// through it, so the format a scene file authors is the format the renderer allocates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentFormat {
    Surface,
    Depth32,
    Rgba16f,
}

impl AttachmentFormat {
    /// Every format a surface can declare, spelled as authored.
    pub const NAMES: &'static [&'static str] = &["surface", "depth32", "rgba16f"];

    /// The authored spelling of this format.
    pub fn name(self) -> &'static str {
        match self {
            AttachmentFormat::Surface => "surface",
            AttachmentFormat::Depth32 => "depth32",
            AttachmentFormat::Rgba16f => "rgba16f",
        }
    }

    /// The wgpu texture format this declared format IS. `surface` is whatever the
    /// swapchain was configured with (so it follows the platform), `depth32` is the one
    /// depth format the depth-sampling passes read, and `rgba16f` is the LINEAR HDR
    /// working colour. **The ONE mapping** — [`crate::HDR_FORMAT`] is defined through it
    /// and the HDR allocation takes its format from the stage's declared `hdr`
    /// attachment, so a declared format can never diverge from an allocated one.
    pub const fn texture_format(self, surface: wgpu::TextureFormat) -> wgpu::TextureFormat {
        match self {
            AttachmentFormat::Surface => surface,
            AttachmentFormat::Depth32 => crate::pipeline_mesh::DEPTH_FORMAT,
            AttachmentFormat::Rgba16f => wgpu::TextureFormat::Rgba16Float,
        }
    }

    /// The format that name spells, or `None` — the caller reports the name it read.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "surface" => Some(AttachmentFormat::Surface),
            "depth32" => Some(AttachmentFormat::Depth32),
            "rgba16f" => Some(AttachmentFormat::Rgba16f),
            _ => None,
        }
    }
}

/// One named image a surface's passes read and write: its format, and how big it is
/// relative to the surface's seated rect (`0.5` = half-resolution).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attachment {
    pub format: AttachmentFormat,
    pub scale: f32,
}

impl Default for Attachment {
    fn default() -> Self {
        Self {
            format: AttachmentFormat::Surface,
            scale: 1.0,
        }
    }
}

/// The images ONE surface owns, in authored order. A stage that authors no `attachments`
/// block gets [`Attachments::COLOR`] + [`Attachments::DEPTH`] — what every surface has
/// always had — so nothing written before attachments existed changes shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachments {
    entries: Vec<(String, Attachment)>,
}

impl Default for Attachments {
    fn default() -> Self {
        Self {
            entries: vec![
                (Self::COLOR.to_string(), Attachment::default()),
                (
                    Self::DEPTH.to_string(),
                    Attachment {
                        format: AttachmentFormat::Depth32,
                        scale: 1.0,
                    },
                ),
            ],
        }
    }
}

impl Attachments {
    /// The colour a surface composites from — the one every pass writes by default, and
    /// the one [`Self::pixels`] sizes the render target off.
    pub const COLOR: &'static str = "color";
    /// The depth the depth-sampling passes (volumetric disk, ground fog) read.
    pub const DEPTH: &'static str = "depth";
    /// The LINEAR HDR (rgba16f) colour the lit-3D passes write into when a surface goes
    /// HDR, and the [`PassKind::TonemapGrade`] pass reads to resolve back into
    /// [`Self::COLOR`]. A surface that declares no `hdr` attachment renders straight into
    /// `color` as before.
    pub const HDR: &'static str = "hdr";

    /// An EMPTY set — what the compiler fills from an authored `attachments` block, which
    /// REPLACES the default pair rather than adding to it.
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Declare (or redeclare) one named image.
    pub fn set(&mut self, name: &str, attachment: Attachment) -> &mut Self {
        match self.entries.iter_mut().find(|(n, _)| n == name) {
            Some((_, a)) => *a = attachment,
            None => self.entries.push((name.to_string(), attachment)),
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<Attachment> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| *a)
    }

    /// Every declared name, in authored order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _)| n.as_str())
    }

    /// Pixel size of a surface of this stage seated in `rect` (its rect in destination
    /// pixels), honouring `color`'s `scale`. **THE one sizing rule** — a render target is
    /// at least 1×1, and a rect that is NaN, negative or absurd yields 1 rather than a
    /// device-lost panic.
    pub fn pixels(&self, rect: Vec2) -> (u32, u32) {
        let scale = match self.get(Self::COLOR) {
            Some(a) if a.scale.is_finite() && a.scale > 0.0 => a.scale,
            _ => 1.0,
        };
        let px = |v: f32| {
            let n = (v * scale).round();
            if n >= 1.0 {
                n as u32
            } else {
                1
            }
        };
        (px(rect.x), px(rect.y))
    }
}

/// How often a surface re-renders. **The ONE representation of liveness** — N live
/// surfaces cost N GPU submits per frame and a pack-browser screen carries a dozen, so
/// this is authored data, not a renderer detail.
///
/// The decision is answered by [`Rate::renders`], driven once per frame at each surface's
/// [`FrameGraph::surface`](crate::FrameGraph::surface) target by the renderer's per-surface
/// clock ([`Renderer::surface_should_render`](crate::Renderer::surface_should_render)): every
/// rate — `Hz` and `Dirty` included — is live, so all four are authorable.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Rate {
    /// Re-render every frame.
    #[default]
    Live,
    /// Never re-render: the target keeps the last image drawn into it and still
    /// composites — the poster rule, which is why a still doll costs nothing.
    Poster,
    /// Re-render at most this many times a second.
    Hz(f32),
    /// Re-render when the content says it changed.
    Dirty,
}

impl Rate {
    /// The rates a surface can name with one word (`Hz` is `{"hz": N}`).
    pub const NAMES: &'static [&'static str] = &["live", "poster", "dirty"];

    /// Whether the surface re-renders this frame. `drawn` = anything has ever been drawn
    /// into its target (a never-drawn surface ALWAYS renders once, or it would composite
    /// garbage); `since_last` = seconds since its last render (`f32::INFINITY` from a
    /// caller with no clock); `dirty` = the content says it changed.
    pub fn renders(self, drawn: bool, since_last: f32, dirty: bool) -> bool {
        if !drawn {
            return true;
        }
        match self {
            Rate::Live => true,
            Rate::Poster => false,
            Rate::Hz(hz) if hz > 0.0 => since_last >= 1.0 / hz,
            Rate::Hz(_) => false,
            Rate::Dirty => dirty,
        }
    }
}

/// One PASS of a stage's recipe — a whole-surface step the engine draws around the
/// content. Mirrors [`StageLayer`]'s shape (a `KINDS` roster + [`PassKind::kind`]) rather
/// than a `dyn Pass` trait: the roster is closed, and a new kind should be a new arm the
/// compiler forces every reader to answer for.
#[derive(Clone, Debug, PartialEq)]
pub enum PassKind {
    /// The content closure — everything the scene itself draws (meshes, lines,
    /// billboards, the 2D layers). The default recipe is exactly this one pass.
    Scene,
    /// The procedural sky behind the scene. Its palette rides the stage's lighting
    /// preset, so it takes no params of its own.
    Sky,
    VolumetricDisk(Box<VolumetricPass>),
    GroundFog(Box<GroundFogPass>),
    /// The tonemap + colour-grade RESOLVE: reads the surface's `hdr` attachment and writes
    /// the tonemapped result into `color`. Derives LAST from reads/writes (it reads what
    /// the lit passes write), so a surface goes HDR by declaring the `hdr` attachment,
    /// repointing those passes' `writes` at it, and appending this pass.
    TonemapGrade(TonemapGradePass),
    Composite(CompositePass),
    /// A sun/light shadow map — ONE kind, two ROLES split by [`ShadowMapPass::from`]: a
    /// PRODUCER stage renders the casters from the light's view into a depth attachment;
    /// a CONSUMER pass in a room recipe names that surface and binds its depth so the lit
    /// passes sample it. See [`ShadowMapPass`].
    ShadowMap(ShadowMapPass),
    /// A REAL animated water MESH at a sea level — a wave-displaced grid drawn as depth-writing
    /// geometry in the opaque pass (it reads `depth`, so it derives after the `scene` that
    /// wrote it; it writes `hdr` before the tonemap). Occludes and is occluded by the terrain,
    /// shades a Fresnel body plus a real sun specular. Boxed — its authored fields (two colours
    /// + a wave list + binds) make it the largest pass. See [`WaterPass`].
    WaterSurface(Box<WaterPass>),
    /// The HDR bloom post-effect: reads the surface's `hdr` colour, extracts the bright parts
    /// above a soft-kneed threshold, blurs them, and adds the glow back into `hdr` — so it
    /// derives AFTER everything that writes `hdr` (it reads it) and BEFORE `tonemap_grade`
    /// (which reads the bloomed `hdr`). Pure art knobs and no binds — the ONE roster kind that
    /// takes no per-frame input at all. See [`BloomPass`].
    Bloom(BloomPass),
}

impl PassKind {
    /// Every `pass` kind the engine knows, spelled as authored.
    pub const KINDS: &'static [&'static str] = &[
        "scene",
        "sky",
        "volumetric_disk",
        "ground_fog",
        "tonemap_grade",
        "composite",
        "shadow_map",
        "water_surface",
        "bloom",
    ];

    /// The authored `pass` spelling of this kind.
    pub fn kind(&self) -> &'static str {
        match self {
            PassKind::Scene => "scene",
            PassKind::Sky => "sky",
            PassKind::VolumetricDisk(_) => "volumetric_disk",
            PassKind::GroundFog(_) => "ground_fog",
            PassKind::TonemapGrade(_) => "tonemap_grade",
            PassKind::Composite(_) => "composite",
            PassKind::ShadowMap(_) => "shadow_map",
            PassKind::WaterSurface(_) => "water_surface",
            PassKind::Bloom(_) => "bloom",
        }
    }
}

/// The fields of a `tonemap_grade` a per-frame input may drive — how far the resolve lerps
/// toward the authored tint, and the exposure before the curve. Mirrors [`FogSlot`] /
/// [`WaterSlot`]: a bind REPLACES the authored value.
///
/// This is what makes the grade FOLLOW the simulation: a day/night cycle publishes a
/// golden-hour warmth and the room's grade rides it, so the tint is authored art and the
/// strength is per-frame state — one representation of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TonemapSlot {
    GradeStrength,
    Exposure,
}

/// The authored tonemap + colour-grade pass. The grade rides HERE (pass-owned), not on the
/// scene uniform — the tonemap pipeline uploads it — so the grade finally has a reader and
/// there is ONE representation of it. Neutral defaults (no tint, unit exposure) are a pure
/// ACES resolve.
///
/// A recipe that authors NO binds resolves to exactly its authored numbers, so every
/// tonemap written before binds existed grades exactly as it did.
#[derive(Clone, Debug, PartialEq)]
pub struct TonemapGradePass {
    /// Linear-RGB grade tint the resolve lerps toward by `grade_strength`. Pure art — never
    /// bound (a per-frame COLOUR would need a second channel; the strength is the dial).
    pub grade: Vec3,
    /// Grade strength `0..1`. `0.0` = tonemap only (no tint). A `grade_strength_bind`
    /// REPLACES it.
    pub grade_strength: f32,
    /// Linear exposure multiply applied before the curve. `1.0` = neutral. An
    /// `exposure_bind` REPLACES it.
    pub exposure: f32,
    /// `(slot, input key)` — a bind REPLACES the authored field at apply time.
    pub binds: Vec<(TonemapSlot, String)>,
}

impl Default for TonemapGradePass {
    fn default() -> Self {
        Self {
            grade: Vec3::ZERO,
            grade_strength: 0.0,
            exposure: 1.0,
            binds: Vec::new(),
        }
    }
}

impl TonemapGradePass {
    /// The grade params this frame — `(tint, strength, exposure)`, the exact triple
    /// [`Renderer::set_tonemap_grade`](crate::Renderer::set_tonemap_grade) takes: the
    /// authored numbers with every bound field replaced by the published input. An input
    /// key nothing publishes leaves the authored number (the recipe's own gate is what
    /// fails loud on a bind that resolves to nothing), and an EMPTY bind list returns the
    /// authored triple unchanged — today's behaviour, bit for bit.
    pub fn resolve(&self, inputs: &StageInputs) -> (Vec3, f32, f32) {
        let mut grade_strength = self.grade_strength;
        let mut exposure = self.exposure;
        for (slot, key) in &self.binds {
            let Some(v) = inputs.get(key) else { continue };
            match slot {
                TonemapSlot::GradeStrength => grade_strength = v,
                TonemapSlot::Exposure => exposure = v,
            }
        }
        (self.grade, grade_strength, exposure)
    }
}

/// The authored HDR bloom pass — bright HDR highlights glow. Reads the surface's `hdr`
/// colour, extracts everything above a soft-kneed threshold into a half-res buffer, blurs it
/// separably, and adds the glow back into `hdr` BEFORE the `tonemap_grade` resolve. Pure art
/// knobs, no binds and no per-frame simulation inputs — the simplest roster kind. See
/// `pipeline_bloom.rs` for the three-stage encode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BloomPass {
    /// Linear-HDR brightness a pixel must exceed to bloom. On an HDR surface ~`1.0` blooms only
    /// the genuinely bright (post-exposure) highlights — the sun glint, the sun disc — while the
    /// lit terrain below 1.0 does not glow. Below `threshold - knee` a pixel contributes nothing.
    pub threshold: f32,
    /// Soft-knee half-width below the threshold: a smooth quadratic ramp so the bloom fades in
    /// rather than popping on at a hard edge. `0.0` = a hard cutoff at the threshold.
    pub knee: f32,
    /// How strongly the blurred bloom is added back into the scene (`hdr += bloom * intensity`).
    pub intensity: f32,
    /// Blur spread multiplier — scales the Gaussian tap spacing, so the glow widens without
    /// changing the tap count. `1.0` = the base 9-tap radius.
    pub radius: f32,
}

impl Default for BloomPass {
    /// A gentle default glow: only >1 highlights bloom, softly, at a modest strength.
    fn default() -> Self {
        Self {
            threshold: 1.0,
            knee: 0.5,
            intensity: 0.6,
            radius: 1.0,
        }
    }
}

/// The authored volumetric-disk pass: the disk as authored, plus the per-frame numbers a
/// simulation publishes into it.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumetricPass {
    pub disk: VolumetricDisk,
    /// `(field, input key)` — a bind REPLACES the authored field at apply time.
    pub binds: Vec<(VolumetricSlot, String)>,
}

/// The fields of a `volumetric_disk` a per-frame input may drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumetricSlot {
    Formation,
    Time,
    Density,
}

impl VolumetricPass {
    /// The disk this frame: the authored numbers, every bound field replaced by the
    /// published input, and the simulation's annular gaps (never authored data). An
    /// input key nothing publishes leaves the authored number — the recipe's own gate
    /// is what fails loud on a bind that resolves to nothing.
    pub fn resolve(&self, inputs: &StageInputs) -> VolumetricDisk {
        let mut disk = self.disk.clone();
        for (slot, key) in &self.binds {
            let Some(v) = inputs.get(key) else { continue };
            match slot {
                VolumetricSlot::Formation => disk.formation = v,
                VolumetricSlot::Time => disk.time = v,
                VolumetricSlot::Density => disk.density = v,
            }
        }
        disk.gaps = inputs.gap_list().to_vec();
        disk
    }
}

/// The authored ground-fog pass.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundFogPass {
    /// The slab as authored. Its own `color` is inert — [`Self::color`] is the authored
    /// colour and [`Self::resolve`] always writes it.
    pub fog: GroundFog,
    /// World Y added to `bottom`/`top` at apply time — a slab authored RELATIVE to a
    /// floor the simulation finds. `0.0` = absolute authoring.
    pub floor: f32,
    /// `None` = take the fog colour from the renderer's live
    /// [`LightRig::fog_color`], so a fog under a day/night cycle follows it without
    /// the scene reaching into the recipe.
    pub color: Option<Vec3>,
    /// `(field, input key)` — a bind REPLACES the authored field at apply time.
    pub binds: Vec<(FogSlot, String)>,
}

/// The fields of a `ground_fog` a per-frame input may drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FogSlot {
    Floor,
    Density,
    Time,
    Coverage,
}

impl GroundFogPass {
    /// The fog this frame: the authored slab, every bound field replaced by the published
    /// input, the (possibly bound) floor added to the slab, and the colour resolved
    /// against `live_fog_color` when the recipe authors none.
    pub fn resolve(&self, inputs: &StageInputs, live_fog_color: Vec3) -> GroundFog {
        let mut fog = self.fog;
        let mut floor = self.floor;
        for (slot, key) in &self.binds {
            let Some(v) = inputs.get(key) else { continue };
            match slot {
                FogSlot::Floor => floor = v,
                FogSlot::Density => fog.density = v,
                FogSlot::Time => fog.time = v,
                FogSlot::Coverage => fog.coverage = v,
            }
        }
        fog.bottom += floor;
        fog.top += floor;
        fog.color = self.color.unwrap_or(live_fog_color);
        fog
    }
}

/// The authored composite pass: another surface's colour, blitted into this one.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositePass {
    /// The surface whose colour is drawn.
    pub from: String,
}

/// The authored shadow-map pass — ONE kind, two ROLES split by [`Self::from`]:
///
/// * **PRODUCER** (`from: None`), on a shadow stage: renders the casters from the light's
///   view into the stage's depth attachment. `light` is the rig slot the shadow is cast
///   for (a `Dir` light), `extent` the half-size of the orthographic box fitted around the
///   field (world units), `bias` the depth bias the consumer samples with. Its
///   `default_writes` is `["depth"]` — the shadow depth it produces.
/// * **CONSUMER** (`from: Some(surface)`), in a room recipe: names the producer surface
///   whose depth the lit passes sample. It contributes an INPUT BINDING, not an attachment
///   read/write (both `default_reads`/`default_writes` are empty), so its position in the
///   derived order is free — the scene wires the surface + the ONE light-view-projection
///   matrix into the renderer (like a `composite`, whose real blit the scene records too).
///   `extent` is unused on the consumer (the producer fitted the box).
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowMapPass {
    /// The rig slot this shadow is cast for.
    pub light: u32,
    /// Depth bias (shadow-map units) subtracted when sampling, to fight self-shadow acne.
    pub bias: f32,
    /// Half-size of the orthographic caster box (world units) — PRODUCER only.
    pub extent: f32,
    /// `Some(surface)` = the CONSUMER role naming the producer surface; `None` = the
    /// PRODUCER role that renders the depth.
    pub from: Option<String>,
}

/// The fields of a `water_surface` a per-frame input may drive — the sea level (a flood
/// height the simulation may raise/lower) and the animation time (the scene clock that
/// scrolls the ripple normals). Mirrors [`FogSlot`]: a bind REPLACES the authored value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterSlot {
    SeaLevel,
    Time,
}

/// The authored water-surface pass — a REAL animated water MESH at `sea_level`, drawn as
/// depth-writing geometry in the opaque pass (not a screen-space plane).
///
/// A PROJECTED-GRID ocean: the vertex shader reads the water grid as SCREEN space and casts it
/// through the camera inverse onto the sea plane, so it always tiles the visible water out to
/// the horizon (dense near the camera, coarse far — a built-in LOD at a fixed vertex count),
/// then lifts each world point to `y = sea_level + Σ waves` (faded flat with distance by
/// `wave_falloff` — gently for the directional swell, hard for the radial chop) with its normal
/// recomputed analytically from the summed wave derivatives; the fragment shader shades a Fresnel
/// water body (lerping `shallow → deep` by view angle, sharpened by `shore_fade`) plus a REAL
/// specular summed over the frame's two SKY SLOTS, light 0 (the sun) and light 1 (the moon) —
/// the "glow" a mirror reflection was standing in for, by day and by night. Being real geometry
/// it occludes and is occluded by the terrain, and writes the surface's `hdr` colour so the
/// specular survives to the tonemap.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterPass {
    /// World Y of the water plane before the waves displace it. A `sea_level_bind` REPLACES it
    /// (a flood the simulation drives), like the fog's `floor`.
    pub sea_level: f32,
    /// Linear RGB seen at grazing view angles (the surface / shallow tint).
    pub shallow: Vec3,
    /// Linear RGB seen looking straight down (deep water).
    pub deep: Vec3,
    /// Sharpness of the shallow→deep view-angle transition (art knob).
    pub shore_fade: f32,
    /// Specular exponent — larger = a tighter, brighter glint. ONE knob for BOTH sky slots.
    pub spec_shininess: f32,
    /// Specular strength multiplier — the same one knob over both sky slots.
    pub spec_strength: f32,
    /// Multiplies the wave slope used for SHADING (not the geometry), so the glint choppiness
    /// tunes independently of the wave height.
    pub normal_scale: f32,
    /// Distance falloff `k` for the far-field flattening (`1 / (1 + dist·k)`): the RADIAL chop
    /// fades to a flat mirror toward the horizon, hiding the projected grid's far-field
    /// coarseness and killing sub-pixel wavelet shimmer. `0` = waves never attenuate.
    /// DIRECTIONAL sources feel the same law at a much gentler rate, so the open-ocean swell
    /// still reaches the horizon — one authored dial, two rates.
    pub wave_falloff: f32,
    /// How much of the live SKY the surface mirrors, `0..1` — the dial between the authored
    /// body colour and a full Fresnel reflection of the atmosphere.
    pub env_strength: f32,
    /// The wave sources summed into the surface — ONE roster of RADIAL and DIRECTIONAL kinds
    /// (`<= MAX_WAVE_SOURCES`; extras are a compile problem).
    pub waves: Vec<WaveSource>,
    /// `(slot, input key)` — a bind REPLACES the authored field at apply time.
    pub binds: Vec<(WaterSlot, String)>,
}

impl WaterPass {
    /// The water params this frame: the authored numbers with every bound field replaced by
    /// the published input, and the authored wave list packed into the fixed uniform roster.
    /// `time` is pure per-frame input (never an authored field, like the fog's clock-driven
    /// time) — unbound leaves the water still.
    pub fn resolve(&self, inputs: &StageInputs) -> Water {
        let mut sea_level = self.sea_level;
        let mut time = 0.0;
        for (slot, key) in &self.binds {
            let Some(v) = inputs.get(key) else { continue };
            match slot {
                WaterSlot::SeaLevel => sea_level = v,
                WaterSlot::Time => time = v,
            }
        }
        let mut waves = [WaveSource::default(); MAX_WAVE_SOURCES];
        let count = self.waves.len().min(MAX_WAVE_SOURCES);
        waves[..count].copy_from_slice(&self.waves[..count]);
        Water {
            sea_level,
            shallow: self.shallow,
            deep: self.deep,
            shore_fade: self.shore_fade,
            spec_shininess: self.spec_shininess,
            spec_strength: self.spec_strength,
            normal_scale: self.normal_scale,
            wave_falloff: self.wave_falloff,
            env_strength: self.env_strength,
            time,
            waves,
            wave_count: count as u32,
        }
    }
}

/// One step of a stage's recipe: what it does, what it READS, and what it WRITES. The
/// reads/writes are the ONLY ordering information — [`StageDef::pass_order`] derives the
/// order from them, so no file anywhere spells a pass number.
#[derive(Clone, Debug, PartialEq)]
pub struct PassDef {
    pub kind: PassKind,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}

impl PassDef {
    /// A pass with the reads and writes its kind has by default.
    pub fn new(kind: PassKind) -> Self {
        Self {
            reads: Self::default_reads(&kind),
            writes: Self::default_writes(&kind),
            kind,
        }
    }

    /// What a pass of this kind reads unless the recipe says otherwise: the
    /// depth-sampling passes read the depth; the tonemap reads the HDR colour; a composite
    /// reads the surface it names.
    pub fn default_reads(kind: &PassKind) -> Vec<String> {
        match kind {
            // A shadow_map reads no surface attachment: the PRODUCER writes depth, the
            // CONSUMER contributes an input binding the scene wires (not an attachment read).
            PassKind::Scene | PassKind::Sky | PassKind::ShadowMap(_) => Vec::new(),
            PassKind::VolumetricDisk(_) | PassKind::GroundFog(_) | PassKind::WaterSurface(_) => {
                vec![Attachments::DEPTH.to_string()]
            }
            // The tonemap resolves the HDR; bloom extracts its bright parts. BOTH reading `hdr`
            // is what orders them after everything the lit passes wrote into it.
            PassKind::TonemapGrade(_) | PassKind::Bloom(_) => vec![Attachments::HDR.to_string()],
            PassKind::Composite(c) => vec![c.from.clone()],
        }
    }

    /// What a pass of this kind writes unless the recipe says otherwise: the colour, and
    /// for the content pass the depth every later pass reads.
    pub fn default_writes(kind: &PassKind) -> Vec<String> {
        match kind {
            PassKind::Scene => vec![
                Attachments::COLOR.to_string(),
                Attachments::DEPTH.to_string(),
            ],
            // A shadow_map PRODUCER writes the depth it renders the casters into; the
            // CONSUMER writes nothing (it only binds a foreign surface's depth).
            PassKind::ShadowMap(s) => {
                if s.from.is_none() {
                    vec![Attachments::DEPTH.to_string()]
                } else {
                    Vec::new()
                }
            }
            // Water emits LINEAR radiance (its Fresnel reflection) and is tonemapped, so it
            // writes the `hdr` colour — the read the tonemap makes of `hdr` is what orders
            // the resolve after it, exactly as the lit passes do. Bloom ADDS its glow back into
            // `hdr` (in place), so it writes `hdr` too — which orders the tonemap after it.
            PassKind::WaterSurface(_) | PassKind::Bloom(_) => vec![Attachments::HDR.to_string()],
            _ => vec![Attachments::COLOR.to_string()],
        }
    }
}

/// Which depth-sampling pass runs, as one element of a frame's [`depth_plan`]. The
/// renderer's overlay pass walks the plan in order, so the recipe — not a fixed rule —
/// decides whether the disk or the fog composites first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthPass {
    Volumetric,
    GroundFog,
}

/// **The per-frame depth-pass plan** — the depth-sampling passes of an ALREADY-ORDERED
/// recipe (the output of [`StageDef::pass_order`]), in the order they execute.
///
/// The ONE builder: [`FrameGraph::surface`](crate::FrameGraph::surface) hands the result
/// to [`Renderer::set_depth_plan`](crate::Renderer::set_depth_plan) as it applies the
/// recipe, and `encode_passes` walks exactly that. A test that calls this is testing the
/// renderer's own plan, not a second derivation of it.
pub fn depth_plan(ordered: &[&PassDef]) -> Vec<DepthPass> {
    ordered
        .iter()
        .filter_map(|p| match p.kind {
            PassKind::VolumetricDisk(_) => Some(DepthPass::Volumetric),
            PassKind::GroundFog(_) => Some(DepthPass::GroundFog),
            // `water_surface` READS depth (so it orders after the scene), but it is REAL
            // geometry drawn in the opaque pass, not a read-only overlay depth-sampler — so it
            // is not one of these overlay passes.
            _ => None,
        })
        .collect()
}

/// The per-frame numbers a scene publishes into its stage's recipe — the ONE channel
/// between a simulation and its authored passes. `flicker-render` is serde-free and
/// script-free, so these are typed values keyed by the name the recipe binds, never JSON.
#[derive(Default, Clone, Debug)]
pub struct StageInputs {
    scalars: Vec<(String, f32)>,
    /// Annular gaps for a `volumetric_disk` — simulation output, never authored data.
    gaps: Vec<(f32, f32)>,
    /// The scene's own clock, in seconds, that the stage rig's light
    /// [`Driver`](crate::Driver)s are evaluated against. Typed like `gaps` — simulation
    /// output, never an authored key — so a fire's flicker is deterministic and
    /// seedable rather than tied to a wall clock.
    clock: f32,
    /// Whether the content changed this frame — the answer a [`Rate::Dirty`] surface's
    /// liveness turns on. Typed like `clock`: the simulation says whether it moved, the
    /// renderer's per-surface clock decides whether that means a re-render.
    dirty: bool,
}

impl StageInputs {
    /// Publish one number under the key a recipe's `*_bind` names.
    pub fn set(&mut self, key: &str, value: f32) -> &mut Self {
        match self.scalars.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.scalars.push((key.to_string(), value)),
        }
        self
    }

    /// Publish the annular gaps the forming bodies carve in a volumetric disk.
    pub fn gaps(&mut self, gaps: Vec<(f32, f32)>) -> &mut Self {
        self.gaps = gaps;
        self
    }

    /// Publish the scene's clock (seconds) that this stage's light drivers run on.
    /// Unset ⇒ `0.0`, which every driver evaluates deterministically — a scene that
    /// publishes no clock gets a still rig, never a wall-clock one.
    pub fn clock(&mut self, t: f32) -> &mut Self {
        self.clock = t;
        self
    }

    /// The clock this frame's light drivers are evaluated at — read by
    /// [`FrameGraph::surface`](crate::FrameGraph::surface), the ONE place a rig is driven.
    pub fn clock_seconds(&self) -> f32 {
        self.clock
    }

    /// Publish whether the content changed this frame — the signal a [`Rate::Dirty`]
    /// surface re-renders on. Unset ⇒ `false` (a `dirty` surface stays a poster until the
    /// scene says otherwise).
    pub fn with_dirty(&mut self, dirty: bool) -> &mut Self {
        self.dirty = dirty;
        self
    }

    /// Whether the scene marked the content changed this frame — read by
    /// [`FrameGraph::surface`](crate::FrameGraph::surface) as it hands the rate to the
    /// renderer's per-surface clock.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn get(&self, key: &str) -> Option<f32> {
        self.scalars.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    /// Every published key, so a scene's own gate can prove its recipe's binds resolve.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.scalars.iter().map(|(k, _)| k.as_str())
    }

    pub(crate) fn gap_list(&self) -> &[(f32, f32)] {
        &self.gaps
    }
}

/// The authored WHAT of one stage source: how it is lit, what it clears to, how it is
/// framed (if the stage owns its view), what it draws, in order — and the RECIPE of
/// engine passes that runs around the drawing.
///
/// Every field's default is its own type's — an unlit rig, no clear, no framing, nothing
/// drawn, the colour+depth pair, the default one-`scene` recipe, live — so the default is
/// DERIVED rather than a second spelling of the same list.
#[derive(Clone, Debug, Default)]
pub struct StageDef {
    pub lighting: LightRig,
    /// What the surface clears to, or `None` when the stage authors no `clear` — the
    /// absence is a DECISION, so it is typed rather than spelled as a colour. An
    /// offscreen surface with no `clear` is transparent ([`StageDef::CLEAR_UNAUTHORED`]),
    /// so the node's own panel supplies the backdrop; a ROOT surface with no `clear`
    /// leaves the window's ([`Renderer::clear_color`](crate::Renderer::clear_color))
    /// alone. Authored, it drives BOTH destinations — see [`crate::FrameGraph::surface`].
    pub clear: Option<[f64; 4]>,
    /// `None` = the scene's camera owns the view (the globes).
    pub camera: Option<StageCamera>,
    /// What is drawn, in authored order.
    pub layers: Vec<StageLayer>,
    /// The images this surface owns. Default: one colour and its depth.
    pub attachments: Attachments,
    /// The engine passes around the content, in AUTHORED order (the executed order is
    /// [`Self::pass_order`]). Empty = the default recipe, see [`Self::recipe`].
    pub passes: Vec<PassDef>,
    /// How often a surface of this stage re-renders, when its seat authors nothing.
    pub rate: Rate,
}

/// The recipe of a stage that authors none: ONE content pass. Every stage written before
/// recipes existed gets this, so the content closure runs exactly where it always ran.
fn default_recipe() -> &'static [PassDef] {
    static DEFAULT: std::sync::OnceLock<Vec<PassDef>> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(|| vec![PassDef::new(PassKind::Scene)])
}

impl StageDef {
    /// What an OFFSCREEN surface clears to when its stage authors no `clear`:
    /// transparent, so the node's own panel is the backdrop.
    pub const CLEAR_UNAUTHORED: [f64; 4] = [0.0; 4];

    /// The passes this stage runs: the authored ones, or the default recipe.
    pub fn recipe(&self) -> &[PassDef] {
        if self.passes.is_empty() {
            default_recipe()
        } else {
            &self.passes
        }
    }

    /// Indices into [`Self::recipe`] in DEPENDENCY order: a pass that reads an attachment
    /// runs after every pass that writes it, and declaration order breaks every tie (so
    /// two passes writing the same image keep the order they were authored in). Pure —
    /// the same Kahn ordering the frame graph uses across surfaces, one level down.
    ///
    /// Returns `(order, cyclic)`; a cycle (a pair of passes each reading what the other
    /// writes) falls back to declaration order with `cyclic = true`, so the executor can
    /// warn instead of dropping a pass.
    pub fn pass_order(&self) -> (Vec<usize>, bool) {
        let passes = self.recipe();
        let ids: Vec<u32> = (0..passes.len() as u32).collect();
        let mut deps: Vec<(u32, u32)> = Vec::new();
        for (i, consumer) in passes.iter().enumerate() {
            for (j, producer) in passes.iter().enumerate() {
                if i != j
                    && consumer
                        .reads
                        .iter()
                        .any(|r| producer.writes.iter().any(|w| w == r))
                {
                    deps.push((i as u32, j as u32));
                }
            }
        }
        crate::frame_graph::topo_order(&ids, &deps)
    }

    /// The kinds among this stage's layers that are NOT in `supported` — what a filler
    /// cannot draw, so it can say so once instead of drawing nothing in silence.
    pub fn layers_outside(&self, supported: &[&str]) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for layer in &self.layers {
            let kind = layer.kind();
            if !supported.contains(&kind) && !out.contains(&kind) {
                out.push(kind);
            }
        }
        out
    }

    /// Whether any layer is of the authored `draw` kind.
    pub fn has_layer(&self, kind: &str) -> bool {
        self.layers.iter().any(|l| l.kind() == kind)
    }
}

/// A usable authored dimension: finite and greater than zero. Authored JSON reaches
/// these helpers unvalidated, so every entry point guards on it.
fn positive(v: f32) -> bool {
    v.is_finite() && v > 0.0
}

/// Line-loop segments approximating a circle on the horizontal plane through
/// `center` — the ground ring under a staged character. `segments` sides; 24 is
/// what the pack editor used and it reads smooth down to portrait sizes.
///
/// A degenerate ring (fewer than 3 sides, or a non-finite / non-positive radius)
/// returns empty rather than panicking, so authored JSON can be passed straight
/// through without validation.
pub fn ring_segments(center: Vec3, radius: f32, segments: usize) -> Vec<(Vec3, Vec3)> {
    use std::f32::consts::TAU;
    if segments < 3 || !positive(radius) {
        return Vec::new();
    }
    let at = |k: usize| {
        let a = k as f32 / segments as f32 * TAU;
        Vec3::new(
            center.x + radius * a.cos(),
            center.y,
            center.z + radius * a.sin(),
        )
    };
    (0..segments).map(|i| (at(i), at(i + 1))).collect()
}

/// A square ground grid centred on the origin at height `y`: lines every `spacing`
/// metres out to `extent` metres each way. The `grid` stage layer — the faint floor
/// under a turntable shot.
///
/// Returns empty on non-finite or non-positive inputs, for the same reason as
/// [`ring_segments`].
pub fn grid_segments(spacing: f32, extent: f32, y: f32) -> Vec<(Vec3, Vec3)> {
    if !(positive(spacing) && positive(extent)) {
        return Vec::new();
    }
    let n = (extent / spacing).floor() as i32;
    let mut segs = Vec::with_capacity((2 * n as usize + 1) * 2);
    for i in -n..=n {
        let t = i as f32 * spacing;
        segs.push((Vec3::new(t, y, -extent), Vec3::new(t, y, extent)));
        segs.push((Vec3::new(-extent, y, t), Vec3::new(extent, y, t)));
    }
    segs
}

/// A square ground grid centred on the origin in the **XY plane** at height `z`: lines
/// every `spacing` units out to `extent` each way.
///
/// The Z-up sibling of [`grid_segments`]. Content is authored Z-up (ground = XY, +Z up)
/// and the asset-pipeline editor draws the RAW source at that reckoning, so its stage
/// floor lies in XY; the Y-up [`grid_segments`] is for runtime-space scenes, which see
/// content only after the loader's Z-up→Y-up conversion. Same guards as its sibling.
pub fn grid_segments_xy(spacing: f32, extent: f32, z: f32) -> Vec<(Vec3, Vec3)> {
    if !(positive(spacing) && positive(extent)) {
        return Vec::new();
    }
    let n = (extent / spacing).floor() as i32;
    let mut segs = Vec::with_capacity((2 * n as usize + 1) * 2);
    for i in -n..=n {
        let t = i as f32 * spacing;
        segs.push((Vec3::new(t, -extent, z), Vec3::new(t, extent, z)));
        segs.push((Vec3::new(-extent, t, z), Vec3::new(extent, t, z)));
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volumetric(binds: Vec<(VolumetricSlot, &str)>) -> PassDef {
        PassDef::new(PassKind::VolumetricDisk(Box::new(VolumetricPass {
            disk: VolumetricDisk::default(),
            binds: binds.into_iter().map(|(s, k)| (s, k.to_string())).collect(),
        })))
    }

    fn ground_fog(floor: f32, binds: Vec<(FogSlot, &str)>) -> PassDef {
        PassDef::new(PassKind::GroundFog(Box::new(GroundFogPass {
            fog: GroundFog {
                bottom: -2.0,
                top: 12.0,
                ..GroundFog::default()
            },
            floor,
            color: None,
            binds: binds.into_iter().map(|(s, k)| (s, k.to_string())).collect(),
        })))
    }

    fn water(binds: Vec<(WaterSlot, &str)>) -> PassDef {
        PassDef::new(PassKind::WaterSurface(Box::new(WaterPass {
            sea_level: 0.0,
            shallow: Vec3::ZERO,
            deep: Vec3::ZERO,
            shore_fade: 4.0,
            spec_shininess: 200.0,
            spec_strength: 1.0,
            normal_scale: 1.0,
            wave_falloff: 0.0,
            env_strength: 1.0,
            waves: Vec::new(),
            binds: binds.into_iter().map(|(s, k)| (s, k.to_string())).collect(),
        })))
    }

    /// **A `water_surface` derives AFTER `scene` and BEFORE `ground_fog`/`tonemap_grade`, and
    /// is NOT an overlay depth-sampler.** Water reads `depth` (so it follows the scene that
    /// writes it) and writes `hdr` (so the tonemap that reads `hdr` resolves after it) — the
    /// exact reads/writes that place it, authored deliberately out of order to prove the
    /// derivation. Unlike the disk/fog it is REAL geometry drawn in the opaque pass, so it is
    /// absent from the overlay [`depth_plan`] even though it reads depth; only the fog is. All
    /// without a GPU — the shipped plan builder, not a re-derivation.
    #[test]
    fn water_orders_after_scene_before_fog_and_is_not_a_depth_sampler() {
        let hdr_scene = || {
            let mut s = PassDef::new(PassKind::Scene);
            s.writes = vec![Attachments::HDR.to_string(), Attachments::DEPTH.to_string()];
            s
        };
        // Authored tonemap-FIRST, water-SECOND, fog-THIRD, scene-LAST — the derivation must
        // reorder to scene → water → ground_fog → tonemap (scene writes the depth water + fog
        // read and the hdr the tonemap reads; water precedes the fog by declaration order,
        // neither reading what the other writes — the real scene authors them in this order).
        let def = StageDef {
            passes: vec![
                PassDef::new(PassKind::TonemapGrade(TonemapGradePass::default())),
                water(Vec::new()),
                ground_fog(0.0, Vec::new()),
                hdr_scene(),
            ],
            ..StageDef::default()
        };
        let (order, cyclic) = def.pass_order();
        assert!(!cyclic);
        let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
        let pos = |k: &str| kinds.iter().position(|&x| x == k).unwrap();
        assert!(
            pos("scene") < pos("water_surface")
                && pos("water_surface") < pos("ground_fog")
                && pos("water_surface") < pos("tonemap_grade"),
            "water follows the scene (reads its depth) and precedes the fog/tonemap: {kinds:?}"
        );
        // Water is REAL geometry (opaque pass), so only the fog is an overlay depth-sampler.
        let ordered: Vec<&PassDef> = order.iter().map(|&i| &def.recipe()[i]).collect();
        assert_eq!(
            depth_plan(&ordered),
            [DepthPass::GroundFog],
            "the water is not an overlay depth-sampler; the fog is"
        );
    }

    /// **`bloom` derives AFTER every `hdr` writer and immediately BEFORE `tonemap_grade`.**
    /// Bloom reads `hdr` (so it follows sky/scene/water/fog, all of which write it in the room
    /// recipe) and writes `hdr` (so the tonemap that reads `hdr` resolves after it). Authored
    /// deliberately out of order — the tonemap and bloom FIRST — to prove the derivation from
    /// reads/writes, not declaration order, is what places it. This is the honest channel gate
    /// for the ordering the encoder relies on; it also proves no cycle even though bloom both
    /// reads and writes `hdr`. All without a GPU — the shipped `pass_order`.
    #[test]
    fn bloom_orders_after_hdr_writers_and_before_tonemap() {
        let hdr = || Attachments::HDR.to_string();
        let depth = || Attachments::DEPTH.to_string();
        // The room recipe repoints sky/scene/water/fog to write `hdr` (mirrors the scene JSON).
        let mut sky = PassDef::new(PassKind::Sky);
        sky.writes = vec![hdr()];
        let mut scene = PassDef::new(PassKind::Scene);
        scene.writes = vec![hdr(), depth()];
        let mut fog = ground_fog(0.0, Vec::new());
        fog.writes = vec![hdr()];
        // The order-free shadow_map CONSUMER (reads/writes empty) the real room recipe carries —
        // it must not wedge between bloom and the tonemap.
        let shadow = PassDef::new(PassKind::ShadowMap(ShadowMapPass {
            light: 0,
            bias: 0.0,
            extent: 0.0,
            from: Some("sun".to_string()),
        }));
        // water already defaults to writes:[hdr], reads:[depth]; bloom + tonemap default
        // reads:[hdr]; only bloom also writes:[hdr].
        let def = StageDef {
            passes: vec![
                PassDef::new(PassKind::TonemapGrade(TonemapGradePass::default())),
                PassDef::new(PassKind::Bloom(BloomPass::default())),
                sky,
                shadow,
                scene,
                water(Vec::new()),
                fog,
            ],
            ..StageDef::default()
        };
        let (order, cyclic) = def.pass_order();
        assert!(
            !cyclic,
            "bloom reading AND writing hdr must not form a cycle"
        );
        let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
        let pos = |k: &str| kinds.iter().position(|&x| x == k).unwrap();
        assert!(
            pos("bloom") > pos("sky")
                && pos("bloom") > pos("scene")
                && pos("bloom") > pos("water_surface")
                && pos("bloom") > pos("ground_fog"),
            "bloom derives after every hdr writer: {kinds:?}"
        );
        assert_eq!(
            pos("tonemap_grade"),
            pos("bloom") + 1,
            "bloom derives IMMEDIATELY before the tonemap resolve (which reads the bloomed hdr): {kinds:?}"
        );
        // Bloom is NOT an overlay depth-sampler (it reads hdr, not depth).
        let ordered: Vec<&PassDef> = order.iter().map(|&i| &def.recipe()[i]).collect();
        assert_eq!(
            depth_plan(&ordered),
            [DepthPass::GroundFog],
            "bloom is not a depth-sampler; only the fog is"
        );
    }

    /// **The default recipe is ONE content pass.** Every stage authored before recipes
    /// existed has an empty `passes` list, and this is what makes those stages render
    /// exactly as they did: the closure, where the closure always ran.
    #[test]
    fn the_default_recipe_is_one_scene_pass() {
        let def = StageDef::default();
        assert!(def.passes.is_empty(), "nothing is authored");
        assert_eq!(def.recipe().len(), 1);
        assert_eq!(def.recipe()[0].kind, PassKind::Scene);
        assert_eq!(def.recipe()[0].reads, Vec::<String>::new());
        assert_eq!(def.recipe()[0].writes, ["color", "depth"]);
        assert_eq!(def.pass_order(), (vec![0], false));
        // The default surface owns exactly the two images every surface has always had.
        assert_eq!(
            def.attachments.names().collect::<Vec<_>>(),
            [Attachments::COLOR, Attachments::DEPTH]
        );
        assert_eq!(def.rate, Rate::Live, "a surface renders unless told not to");
        // Every kind spells itself with a name the vocabulary lists.
        for kind in [
            PassKind::Scene,
            PassKind::Sky,
            volumetric(Vec::new()).kind,
            ground_fog(0.0, Vec::new()).kind,
            PassKind::TonemapGrade(TonemapGradePass::default()),
            PassKind::Composite(CompositePass {
                from: "side".to_string(),
            }),
        ] {
            assert!(PassKind::KINDS.contains(&kind.kind()), "{:?}", kind.kind());
        }
        // The tonemap READS the HDR colour and WRITES the surface colour — the reads/writes
        // that make `pass_order` derive it last (it reads what the lit passes write).
        let tm = PassDef::new(PassKind::TonemapGrade(TonemapGradePass::default()));
        assert_eq!(tm.reads, [Attachments::HDR]);
        assert_eq!(tm.writes, [Attachments::COLOR]);
    }

    /// **Order is derived from reads and writes**, never authored: the depth-sampling
    /// passes land after the content that wrote the depth, and passes that only WRITE the
    /// colour keep the order they were authored in (that is how the sky stays behind).
    #[test]
    fn pass_order_puts_writers_before_readers() {
        // Authored deliberately out of order: the disk reads a depth that is written
        // three entries later, and the sky is authored last.
        let def = StageDef {
            passes: vec![
                volumetric(vec![(VolumetricSlot::Formation, "dust_formation")]),
                ground_fog(0.0, Vec::new()),
                PassDef::new(PassKind::Scene),
                PassDef::new(PassKind::Sky),
            ],
            ..StageDef::default()
        };
        let (order, cyclic) = def.pass_order();
        assert!(!cyclic);
        let kinds: Vec<&str> = order.iter().map(|&i| def.recipe()[i].kind.kind()).collect();
        assert_eq!(
            kinds,
            ["scene", "sky", "volumetric_disk", "ground_fog"],
            "readers of `depth` follow its writer; the two colour-only writers keep \
             authored order among themselves"
        );

        // The authoring the CURRENT encoder can honour: sky, content, then the two
        // depth-samplers in the order it encodes them.
        let solar = StageDef {
            passes: vec![
                PassDef::new(PassKind::Sky),
                PassDef::new(PassKind::Scene),
                volumetric(Vec::new()),
            ],
            ..StageDef::default()
        };
        assert_eq!(solar.pass_order(), (vec![0, 1, 2], false));

        // A cycle — each pass reading what the other writes — must not hang or drop a
        // pass; it falls back to authored order, flagged.
        let mut a = PassDef::new(PassKind::Scene);
        let mut b = PassDef::new(PassKind::Sky);
        a.reads = vec!["glow".to_string()];
        a.writes = vec!["color".to_string()];
        b.reads = vec!["color".to_string()];
        b.writes = vec!["glow".to_string()];
        let tangled = StageDef {
            passes: vec![a, b],
            ..StageDef::default()
        };
        let (order, cyclic) = tangled.pass_order();
        assert!(cyclic && order.len() == 2);
    }

    /// **The encode plan matches the recipe order.** This calls the renderer's OWN plan
    /// builder — [`depth_plan`] over `pass_order`, the exact pair `FrameGraph::surface`
    /// runs before it hands the result to `Renderer::set_depth_plan` and `encode_passes`
    /// walks it — so the assertion is about the shipped plan, not a re-derivation of it.
    /// Includes a fog-BEFORE-disk recipe, the case the removed refusal used to forbid, and
    /// the tonemap resolving last; both without a GPU.
    #[test]
    fn encode_plan_matches_recipe_order() {
        // The one call pair the frame graph makes: derive the order, then build the plan.
        let plan = |def: &StageDef| -> Vec<DepthPass> {
            let (order, cyclic) = def.pass_order();
            assert!(!cyclic);
            let ordered: Vec<&PassDef> = order.iter().map(|&i| &def.recipe()[i]).collect();
            depth_plan(&ordered)
        };

        // Authored fog-FIRST (no read/write coupling, so declaration order is the tie-break):
        // the plan is [ground_fog, volumetric_disk], which the fixed encoder could not honour
        // and the refusal used to forbid — now it simply IS the order.
        let fog_first = StageDef {
            passes: vec![
                PassDef::new(PassKind::Scene),
                ground_fog(0.0, Vec::new()),
                volumetric(Vec::new()),
            ],
            ..StageDef::default()
        };
        assert_eq!(
            plan(&fog_first),
            [DepthPass::GroundFog, DepthPass::Volumetric]
        );

        // Authored disk-first is the other order — the plan tracks the recipe, not a fixed rule.
        let disk_first = StageDef {
            passes: vec![
                PassDef::new(PassKind::Scene),
                volumetric(Vec::new()),
                ground_fog(0.0, Vec::new()),
            ],
            ..StageDef::default()
        };
        assert_eq!(
            plan(&disk_first),
            [DepthPass::Volumetric, DepthPass::GroundFog]
        );

        // A tonemap reads `hdr`; the lit passes write it — so it derives LAST regardless of
        // where it is authored (here it is authored FIRST).
        let mut sky = PassDef::new(PassKind::Sky);
        sky.writes = vec![Attachments::HDR.to_string()];
        let mut scene = PassDef::new(PassKind::Scene);
        scene.writes = vec![Attachments::HDR.to_string(), Attachments::DEPTH.to_string()];
        let hdr = StageDef {
            passes: vec![
                PassDef::new(PassKind::TonemapGrade(TonemapGradePass::default())),
                sky,
                scene,
            ],
            ..StageDef::default()
        };
        let (order, cyclic) = hdr.pass_order();
        assert!(!cyclic);
        let kinds: Vec<&str> = order.iter().map(|&i| hdr.recipe()[i].kind.kind()).collect();
        assert_eq!(
            kinds.last(),
            Some(&"tonemap_grade"),
            "the tonemap resolves the HDR colour last: {kinds:?}"
        );
    }

    /// **`Rate` reproduces today's liveness exactly.** The whole codebase used to spell
    /// it `live || !drawn` at four sites; `Live`/`Poster` must answer the same, and a
    /// never-drawn surface must render whatever its rate says (or it composites garbage).
    #[test]
    fn rate_live_and_poster_reproduce_todays_liveness() {
        for (rate, drawn, since, dirty, expect) in [
            (Rate::Live, true, f32::INFINITY, false, true),
            (Rate::Live, false, f32::INFINITY, false, true),
            (Rate::Poster, true, f32::INFINITY, false, false),
            (Rate::Poster, false, 0.0, false, true), // the first frame always draws
            (Rate::Dirty, true, 0.0, true, true),
            (Rate::Dirty, true, 0.0, false, false),
            (Rate::Hz(2.0), true, 0.6, false, true), // 0.6s ≥ 1/2s
            (Rate::Hz(2.0), true, 0.4, false, false),
            (Rate::Hz(2.0), true, f32::INFINITY, false, true), // no clock → renders
            (Rate::Hz(0.0), true, f32::INFINITY, false, false), // never, not NaN-often
        ] {
            assert_eq!(
                rate.renders(drawn, since, dirty),
                expect,
                "{rate:?} drawn={drawn} since={since} dirty={dirty}"
            );
        }
        assert_eq!(Rate::default(), Rate::Live);
        for name in Rate::NAMES {
            assert!(["live", "poster", "dirty"].contains(name));
        }
    }

    /// A poster surface renders exactly ONCE — its never-drawn first frame — then never
    /// again, whatever the clock reads. This is the poster rule the per-surface clock now
    /// owns after the three hand-rolled `drawn` caches were deleted (S5d): the target keeps
    /// its last image and the composite still runs.
    #[test]
    fn a_poster_surface_renders_once_then_never() {
        // Frame 1: never drawn → renders, or it would composite garbage.
        assert!(Rate::Poster.renders(false, 0.0, false));
        // Every frame after — drawn, at ANY elapsed time — skips: the image stands.
        assert!(!Rate::Poster.renders(true, 0.0, false));
        assert!(!Rate::Poster.renders(true, 1000.0, false));
        assert!(!Rate::Poster.renders(true, f32::INFINITY, false));
    }

    /// An `hz` surface renders on the CLOCK: once its target holds an image it re-renders
    /// only after a full period has elapsed since its last render — the `since_last =
    /// frame_clock - last_render` the renderer's `surface_should_render` computes and feeds
    /// here — and rides the clock steadily after.
    #[test]
    fn an_hz_surface_renders_on_the_clock() {
        let hz = Rate::Hz(4.0); // one render every 0.25s
                                // First frame always draws (nothing to composite yet).
        assert!(hz.renders(false, 0.0, false));
        // Drawn, but less than a full period since the last render → hold the poster.
        assert!(!hz.renders(true, 0.10, false));
        assert!(!hz.renders(true, 0.24, false));
        // A full period (or more) elapsed → re-render.
        assert!(hz.renders(true, 0.25, false));
        assert!(hz.renders(true, 0.60, false));
    }

    /// **One sizing rule.** A surface's pixel size comes from its colour attachment's
    /// scale — the four hand-rolled `round().max(1)` blocks this replaced could not have
    /// honoured a half-resolution surface at all — and a degenerate rect yields 1×1
    /// rather than a zero-sized target.
    #[test]
    fn attachments_pixels_honour_scale() {
        let full = Attachments::default();
        assert_eq!(full.pixels(Vec2::new(320.4, 180.6)), (320, 181));

        let mut half = Attachments::empty();
        half.set(
            Attachments::COLOR,
            Attachment {
                format: AttachmentFormat::Surface,
                scale: 0.5,
            },
        )
        .set(
            Attachments::DEPTH,
            Attachment {
                format: AttachmentFormat::Depth32,
                scale: 0.5,
            },
        );
        assert_eq!(half.pixels(Vec2::new(320.0, 181.0)), (160, 91));
        assert_eq!(
            half.get(Attachments::DEPTH).map(|a| a.format),
            Some(AttachmentFormat::Depth32)
        );
        assert!(half.get("glow").is_none(), "only what is declared exists");

        // Degenerate rects and scales cost the picture, never the device.
        for rect in [
            Vec2::new(0.0, 0.0),
            Vec2::new(-8.0, 4.0),
            Vec2::new(f32::NAN, 4.0),
        ] {
            let (w, h) = full.pixels(rect);
            assert!(w >= 1 && h >= 1, "{rect:?} → {w}x{h}");
        }
        let mut bad = Attachments::empty();
        bad.set(
            Attachments::COLOR,
            Attachment {
                scale: f32::NAN,
                ..Attachment::default()
            },
        );
        assert_eq!(
            bad.pixels(Vec2::new(64.0, 64.0)),
            (64, 64),
            "unusable scale"
        );
        // A surface with no colour at all still sizes off its rect.
        assert_eq!(Attachments::empty().pixels(Vec2::new(64.0, 32.0)), (64, 32));
        for f in AttachmentFormat::NAMES {
            assert_eq!(AttachmentFormat::from_name(f).map(|a| a.name()), Some(*f));
        }
        assert!(AttachmentFormat::from_name("rgba32f").is_none());
    }

    /// A bind REPLACES the authored field, the fog's slab is authored relative to a floor
    /// the simulation publishes, and an unauthored fog colour follows the live atmosphere.
    /// The gaps are the simulation's alone — no recipe can author them.
    #[test]
    fn inputs_replace_the_fields_a_recipe_binds() {
        let mut inputs = StageInputs::default();
        inputs
            .set("dust_formation", 0.25)
            .set("dust_formation", 0.5) // a key published twice keeps the last word
            .set("fog_floor", 30.0)
            .gaps(vec![(1.0, 0.2), (2.0, 0.3)]);
        assert_eq!(inputs.get("dust_formation"), Some(0.5));
        assert_eq!(inputs.get("nothing_publishes_this"), None);
        assert_eq!(
            inputs.keys().collect::<Vec<_>>(),
            ["dust_formation", "fog_floor"]
        );

        let PassKind::VolumetricDisk(v) = volumetric(vec![
            (VolumetricSlot::Formation, "dust_formation"),
            (VolumetricSlot::Time, "never_published"),
        ])
        .kind
        else {
            unreachable!()
        };
        let disk = v.resolve(&inputs);
        assert_eq!(disk.formation, 0.5, "the bound field is replaced");
        assert_eq!(
            disk.time,
            VolumetricDisk::default().time,
            "an unpublished key leaves the authored number"
        );
        assert_eq!(disk.gaps, [(1.0, 0.2), (2.0, 0.3)], "gaps are sim output");

        let PassKind::GroundFog(f) = ground_fog(0.0, vec![(FogSlot::Floor, "fog_floor")]).kind
        else {
            unreachable!()
        };
        let fog = f.resolve(&inputs, Vec3::new(0.1, 0.2, 0.3));
        assert_eq!(
            (fog.bottom, fog.top),
            (28.0, 42.0),
            "the slab is authored relative to the published floor"
        );
        assert_eq!(fog.color, Vec3::new(0.1, 0.2, 0.3), "no authored colour");
        // An authored colour wins over the live atmosphere, and an absolute slab is a
        // floor of 0.
        let PassKind::GroundFog(mut f) = ground_fog(0.0, Vec::new()).kind else {
            unreachable!()
        };
        f.color = Some(Vec3::X);
        let fog = f.resolve(&StageInputs::default(), Vec3::ZERO);
        assert_eq!((fog.bottom, fog.top, fog.color), (-2.0, 12.0, Vec3::X));
    }

    /// **GATE — a `tonemap_grade` bind REPLACES the authored number, and NO binds is
    /// today's behaviour bit for bit.** This is what lets the grade FOLLOW the sun: the
    /// tint stays authored art while the strength rides a per-frame `grade_warmth` the
    /// scene publishes. The three facts the feature rests on: a bound slot takes the
    /// published value, an unpublished key leaves the authored number (so a typo dims
    /// rather than blanks), and an empty bind list returns the authored triple unchanged —
    /// the guarantee that every tonemap written before binds existed grades as it did.
    #[test]
    fn tonemap_binds_replace_the_authored_strength_and_exposure() {
        let mut inputs = StageInputs::default();
        inputs.set("grade_warmth", 0.31).set("stop", 1.4);

        let tinted = |binds: Vec<(TonemapSlot, &str)>| TonemapGradePass {
            grade: Vec3::new(1.18, 0.92, 0.68),
            grade_strength: 0.05,
            exposure: 1.0,
            binds: binds.into_iter().map(|(s, k)| (s, k.to_string())).collect(),
        };

        // Both slots bound → both replaced; the TINT is never bound, so it stands.
        let (grade, strength, exposure) = tinted(vec![
            (TonemapSlot::GradeStrength, "grade_warmth"),
            (TonemapSlot::Exposure, "stop"),
        ])
        .resolve(&inputs);
        assert_eq!(
            grade,
            Vec3::new(1.18, 0.92, 0.68),
            "the tint is authored art"
        );
        assert_eq!(strength, 0.31, "the bound strength is the published warmth");
        assert_eq!(exposure, 1.4, "the bound exposure is replaced too");

        // A key NOTHING publishes leaves the authored number (the recipe's gate is what
        // fails loud on it) — never a zero that would silently drop the grade.
        let (_, strength, exposure) =
            tinted(vec![(TonemapSlot::GradeStrength, "never_published")]).resolve(&inputs);
        assert_eq!((strength, exposure), (0.05, 1.0));

        // NO binds → the authored triple, unchanged. The solarbirth cinematic's static
        // grade is exactly this shape, and this is why it still resolves as it always did.
        assert_eq!(
            tinted(Vec::new()).resolve(&inputs),
            (Vec3::new(1.18, 0.92, 0.68), 0.05, 1.0)
        );
        // And the neutral default is still the pure ACES resolve.
        assert_eq!(
            TonemapGradePass::default().resolve(&inputs),
            (Vec3::ZERO, 0.0, 1.0)
        );
    }

    /// A filler names exactly the layer kinds it cannot draw — once each, in authored
    /// order — and an empty answer means "everything authored is drawn".
    #[test]
    fn layers_outside_names_each_undrawn_kind_once() {
        let def = StageDef {
            layers: vec![
                StageLayer::Skinned,
                StageLayer::Graticule { radius_scale: 1.0 },
                StageLayer::Shells,
                StageLayer::Graticule { radius_scale: 1.1 },
            ],
            ..StageDef::default()
        };
        assert_eq!(
            def.layers_outside(&["skinned", "ring", "grid"]),
            vec!["graticule", "shells"]
        );
        assert!(def
            .layers_outside(&["skinned", "graticule", "shells"])
            .is_empty());
        assert!(def.has_layer("shells") && !def.has_layer("material"));
        // Every variant spells itself with a name the vocabulary lists.
        for layer in &def.layers {
            assert!(StageLayer::KINDS.contains(&layer.kind()));
        }
    }

    /// The default framing is the portrait — and a framing turns into an orbit camera
    /// looking at the authored target height.
    #[test]
    fn the_default_framing_is_the_portrait_and_looks_at_target_y() {
        let cam = StageCamera::default();
        assert!((cam.dist - 2.6).abs() < 1e-6 && (cam.target_y - 0.95).abs() < 1e-6);
        let c = StageCamera {
            target_y: 1.5,
            ..cam
        }
        .camera();
        assert!(
            (c.target.y - 1.5).abs() < 1e-6,
            "the camera looks at target_y"
        );
        assert!(c.position.is_finite());
        assert!(
            StageDef::default().camera.is_none(),
            "a bare stage owns no view"
        );
    }

    /// The Z-up grid is the same lattice in the other plane: flat in Z, spanning X and Y.
    /// A scene that mixed the two conventions would draw its floor as a wall, so the plane
    /// each one lies in is the thing worth pinning.
    #[test]
    fn xy_grid_is_flat_in_z_and_spans_both_ground_axes() {
        let segs = grid_segments_xy(0.5, 3.0, -1.25);
        assert_eq!(segs.len(), 26, "13 lines each ground axis, interleaved");
        assert!(
            segs.iter().all(|(a, b)| a.z == -1.25 && b.z == -1.25),
            "every vertex sits on the given ground height"
        );
        for axis in [0usize, 1] {
            let c = |v: &Vec3| if axis == 0 { v.x } else { v.y };
            let min = segs.iter().map(|(a, _)| c(a)).fold(f32::MAX, f32::min);
            let max = segs.iter().map(|(a, _)| c(a)).fold(f32::MIN, f32::max);
            assert!(
                (min + 3.0).abs() < 1e-5 && (max - 3.0).abs() < 1e-5,
                "axis {axis} spans the extent"
            );
        }
        // The Y-up sibling is untouched by this: it is still flat in Y.
        assert!(grid_segments(0.5, 3.0, 0.0).iter().all(|(a, _)| a.y == 0.0));
        // Same degenerate guards.
        assert!(grid_segments_xy(0.0, 3.0, 0.0).is_empty());
        assert!(grid_segments_xy(0.5, f32::NAN, 0.0).is_empty());
    }

    #[test]
    fn ring_closes_on_itself_and_sits_at_one_height() {
        let segs = ring_segments(Vec3::new(1.0, 0.5, -2.0), 0.45, 24);
        assert_eq!(segs.len(), 24);
        // Every vertex is on the ring's plane and at the ring's radius.
        for (a, b) in &segs {
            for v in [a, b] {
                assert!((v.y - 0.5).abs() < 1e-5, "ring left its plane");
                let r = ((v.x - 1.0).powi(2) + (v.z + 2.0).powi(2)).sqrt();
                assert!((r - 0.45).abs() < 1e-4, "vertex off the radius: {r}");
            }
        }
        // The loop is continuous: each segment starts where the last ended, and the
        // last closes back onto the first.
        for w in segs.windows(2) {
            assert!((w[0].1 - w[1].0).length() < 1e-5, "ring has a gap");
        }
        assert!(
            (segs[23].1 - segs[0].0).length() < 1e-4,
            "ring never closed"
        );
    }

    #[test]
    fn degenerate_ring_and_grid_are_empty_not_panics() {
        assert!(ring_segments(Vec3::ZERO, 0.45, 2).is_empty());
        assert!(ring_segments(Vec3::ZERO, 0.0, 24).is_empty());
        assert!(ring_segments(Vec3::ZERO, f32::NAN, 24).is_empty());
        assert!(grid_segments(0.0, 6.0, 0.0).is_empty());
        assert!(grid_segments(0.5, -1.0, 0.0).is_empty());
        assert!(grid_segments(f32::INFINITY, 6.0, 0.0).is_empty());
    }

    #[test]
    fn grid_spans_the_extent_both_ways() {
        let segs = grid_segments(0.5, 3.0, 0.0);
        // 13 lines each axis (-3.0 .. 3.0 step 0.5), interleaved.
        assert_eq!(segs.len(), 26);
        assert!(segs.iter().all(|(a, b)| a.y == 0.0 && b.y == 0.0));
        let min_x = segs.iter().map(|(a, _)| a.x).fold(f32::MAX, f32::min);
        let max_x = segs.iter().map(|(a, _)| a.x).fold(f32::MIN, f32::max);
        assert!((min_x + 3.0).abs() < 1e-5 && (max_x - 3.0).abs() < 1e-5);
    }
}
