//! The **Stage** — the live-animating character doll composited into the bench's UI.
//!
//! A stage is a panel whose fill is a render-to-texture sub-scene: something says WHERE
//! (a walker `surface` node reserving a [`SurfaceSlot`], or the scene-owned canvas placing a
//! card), the authored `stages.<source>` block says WHAT (compiled by the ONE stage
//! compiler, `flicker::ui::stage_defs`), and `FrameGraph` decides WHEN. This module owns
//! the WHAT→GPU half for the doll: posing it and declaring the offscreen passes.
//!
//! **Liveness is the seat's [`Rate`], driven by the renderer's per-surface clock.** A slot
//! declares its `FrameGraph::surface` every frame; the clock skips the RENDER of one whose
//! rate is not `Live` (its target keeps the last image and still composites), so a still
//! doll costs zero GPU submits and needs no separate poster texture or hand-rolled cache.
//! Only the selected card and the pointed-at clip row animate, which is what makes a screen
//! carrying a dozen dolls affordable (N live targets = N submits).
//!
//! Framing and line geometry are pure functions — unit-tested without a device; only
//! [`StageRig::load`] and [`StageRig::stage`] touch the renderer.
//!
//! [`SurfaceSlot`]: flicker::ui::SurfaceSlot

use std::collections::HashMap;

use flicker::render::{
    grid_segments, ring_segments, CompositeTarget, FrameGraph, Mat4, MeshIndices, Rate, Rect,
    RenderTargetHandle, Renderer, SkinnedMeshHandle, SkinnedVertex, StageCamera, StageDef,
    StageInputs, StageLayer, Vec3,
};
use flicker_skeletal::format::{Bone, Model, ResolvedClip, Vertex};
use flicker_skeletal::{pose, skin};
use serde_json::Value as Json;

/// The layer kinds the doll rig draws. A stage authoring any other kind is told so at
/// load — once — rather than drawing nothing in silence.
const DOLL_LAYERS: &[&str] = &["skinned", "ring", "grid"];

/// What the scene wants staged in one slot this frame.
///
/// `rect` is the IMAGE rect — the walker's `SurfaceSlot` already inset it inside the node's
/// frame, and the backdrop panel is drawn by whoever owns the WHERE, so the graph only
/// blits the doll over it.
#[derive(Clone)]
pub struct StageReq {
    /// Stable per-slot key — the render target is cached under this.
    pub id: String,
    /// Which `stages.<source>` to render.
    pub source: String,
    pub rect: Rect,
    pub layer: f32,
    pub tint: [f32; 4],
    /// How often this slot re-renders. Anything but [`Rate::Live`] reuses the target's
    /// last image (the poster).
    pub rate: Rate,
    /// Clip index to pose with; `None` → the rest pose.
    pub clip: Option<usize>,
    /// Play-head, seconds. The clip loops on its own duration.
    pub time: f32,
    /// Selected / hovered — lights the ground ring in its active colour.
    pub active: bool,
}

/// One slot's offscreen target and the size it was last built at. Liveness — a never-drawn
/// slot rendering once, a poster keeping its image — is the renderer's per-surface clock's
/// job now (the `rate` handed to `FrameGraph::surface`), not a flag carried here.
struct Slot {
    target: RenderTargetHandle,
    w: u32,
    h: u32,
}

/// The GPU-side stage rig: the doll's skinned mesh, the compiled sources, and one render
/// target per slot id.
#[derive(Default)]
pub struct StageRig {
    sources: HashMap<String, StageDef>,
    mesh: Option<SkinnedMeshHandle>,
    bone_count: u32,
    /// Rest-pose ground offset. `Model::world` centres the rig on the origin, but the
    /// authored cameras and rings are metric with the feet at y = 0, so the doll is
    /// dropped onto the floor before it is drawn.
    ground: Mat4,
    slots: HashMap<String, Slot>,
}

impl StageRig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile the authored sources and upload the doll's skinned mesh. Call from `enter`,
    /// once the styles are resolved and the document's model is loaded.
    pub fn load(&mut self, r: &mut Renderer, model: &Model, styles: &Json) {
        self.sources = flicker::ui::stage_defs(styles);
        for (name, def) in &mut self.sources {
            let undrawn = def.layers_outside(DOLL_LAYERS);
            if !undrawn.is_empty() {
                tracing::warn!(
                    "loomforge stage: `{name}` authors {undrawn:?} layers the doll rig does not draw"
                );
            }
            // The framing is applied by the frame graph from the DEFINITION, so the doll
            // rig's "an unframed stage is a portrait" policy is applied to the definition
            // — once, at load — rather than re-decided in every frame's draw closure.
            if def.camera.is_none() {
                tracing::warn!(
                    "loomforge stage: `{name}` authors no camera — the doll takes the portrait framing"
                );
                def.camera = Some(StageCamera::default());
            }
        }
        self.bone_count = model.bones.len() as u32;
        self.ground = ground_transform(model.world, &model.mesh.vertices);

        if model.mesh.vertices.is_empty() {
            tracing::warn!("loomforge stage: the rig has no mesh — dolls will be empty");
            return;
        }
        let verts: Vec<SkinnedVertex> = model
            .mesh
            .vertices
            .iter()
            .map(|v| SkinnedVertex {
                position: v.p,
                normal: v.n,
                uv: v.uv,
                joints: v.joints,
                weights: v.weights,
            })
            .collect();
        // The converter emits a non-deduped sequential list when indices are absent.
        let indices: Vec<u32> = if model.mesh.indices.is_empty() {
            (0..verts.len() as u32).collect()
        } else {
            model.mesh.indices.clone()
        };
        self.mesh = Some(r.upload_skinned_mesh(&verts, MeshIndices::U32(&indices)));
        tracing::info!(
            sources = self.sources.len(),
            bones = self.bone_count,
            verts = verts.len(),
            "loomforge stage: doll uploaded"
        );
    }

    /// Whether a source name resolves — lets the scene skip building requests for a
    /// stage the JSON does not define.
    pub fn has_source(&self, name: &str) -> bool {
        self.sources.contains_key(name)
    }

    /// How many render targets the cache is holding — the scene prunes off this rather
    /// than rebuilding a keep-set every frame.
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Declare this frame's stage passes and composites on `fg` — declare-only; the
    /// manager owns the one graph and executes it once, running every offscreen pass
    /// before the scene's overlay chrome.
    ///
    /// `base_layer` is the scene's own layer at draw time; a request's `layer` is relative
    /// to it, because the tree is built in `update` before the base layer is known.
    pub fn stage(
        &mut self,
        r: &mut Renderer,
        fg: &mut FrameGraph<'_>,
        model: &Model,
        base_layer: f32,
        reqs: &[StageReq],
    ) {
        // Split the borrows: the target cache is mutated while the sources are read.
        let Self {
            sources,
            mesh,
            bone_count,
            ground,
            slots,
        } = self;
        for req in reqs {
            let Some(src) = sources.get(&req.source) else {
                continue;
            };
            let (w, h) = src.attachments.pixels(req.rect.size);

            let slot = match slots.get_mut(&req.id) {
                Some(s) => {
                    // A resized slot's old image is the wrong shape — the resize rebuilds a
                    // fresh (never-drawn) target, so the clock renders it once again.
                    if s.w != w || s.h != h {
                        r.resize_render_target(s.target, w, h);
                        s.w = w;
                        s.h = h;
                    }
                    s
                }
                None => slots.entry(req.id.clone()).or_insert(Slot {
                    target: r.create_render_target(w, h),
                    w,
                    h,
                }),
            };

            // Everything the pass needs is OWNED here, so the closure borrows nothing and
            // can outlive this loop iteration. Liveness is the seat's `rate`, driven by the
            // renderer's per-surface clock: a poster doll's pass is skipped (its composite
            // below still runs), so a screen of still dolls costs no GPU submits.
            let palette = palette_for(
                &model.bones,
                req.clip.and_then(|i| model.clips.get(i)),
                req.time,
                model.retarget,
            );
            let lines = line_layers(&src.layers, req.active);
            let (ground, mesh, bones) = (*ground, *mesh, *bone_count);
            // The stage's lighting and framing are applied by the graph from the
            // definition; the doll publishes no per-frame inputs.
            fg.surface(
                CompositeTarget::Target(slot.target),
                src,
                StageInputs::default(),
                req.rate,
                move |r| {
                    // Line layers are depth-tested against the doll, so the authored
                    // order costs nothing to honour and a ring still reads under the
                    // feet.
                    for (segs, color) in &lines {
                        r.draw_lines(segs, *color);
                    }
                    if let Some(m) = mesh {
                        r.draw_skinned_instanced(m, &[ground], &palette, bones);
                    }
                },
            );

            // `frame: None` — the backdrop panel was already drawn by whoever placed the
            // slot, keeping every panel in the codebase on one code path.
            fg.composite_panel(
                slot.target,
                CompositeTarget::Screen,
                req.rect,
                base_layer + req.layer,
                req.tint,
                None,
                None,
            );
        }
    }

    /// Drop the targets of slots that no longer appear on screen — switching tabs would
    /// otherwise leak a target per doll the previous page showed.
    pub fn retain_slots(&mut self, r: &mut Renderer, keep: &dyn Fn(&str) -> bool) {
        self.slots.retain(|id, slot| {
            if keep(id) {
                return true;
            }
            r.free_render_target(slot.target);
            false
        });
    }
}

/// The doll's bone palette for one clip at one play-head. Salvaged from the retired pack
/// editor's `rebuild_palettes` — CPU posing is cheap; the GPU does the vertex skinning.
fn palette_for(
    bones: &[Bone],
    clip: Option<&ResolvedClip>,
    time: f32,
    retarget: bool,
) -> Vec<Mat4> {
    let locals = match clip {
        Some(c) => {
            let tick = if c.duration_ticks > 0 {
                let ticks = time * c.tick_rate_hz as f32;
                (ticks.floor() as i64).rem_euclid(c.duration_ticks as i64) as u32
            } else {
                0
            };
            pose::sample_local_poses(bones, c, tick, retarget)
        }
        None => bones.iter().map(|b| b.local).collect(),
    };
    let globals = pose::global_transforms(bones, &locals);
    skin::palette(bones, &globals)
}

/// Drop the rig onto the floor: `Model::world` centres it on the origin, the authored
/// stages are metric with the feet at y = 0.
fn ground_transform(world: Mat4, vertices: &[Vertex]) -> Mat4 {
    let feet = vertices
        .iter()
        .map(|v| world.transform_point3(Vec3::from(v.p)).y)
        .fold(f32::INFINITY, f32::min);
    let drop = if feet.is_finite() { -feet } else { 0.0 };
    Mat4::from_translation(Vec3::new(0.0, drop, 0.0)) * world
}

/// One line layer's segments plus the colour to draw them in.
type LineLayer = (Vec<(Vec3, Vec3)>, [f32; 4]);

/// The line geometry of a source's non-character layers, in authored order. Kinds the
/// doll rig does not draw were named at load; here they simply contribute no lines.
fn line_layers(layers: &[StageLayer], active: bool) -> Vec<LineLayer> {
    layers
        .iter()
        .filter_map(|l| match l {
            StageLayer::Ring {
                radius,
                y,
                segments,
                color,
                color_active,
            } => Some((
                ring_segments(Vec3::new(0.0, *y, 0.0), *radius, *segments),
                if active { *color_active } else { *color },
            )),
            StageLayer::Grid {
                spacing,
                extent,
                y,
                color,
            } => Some((grid_segments(*spacing, *extent, *y), *color)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_skeletal::format::{Bone, Vertex};

    /// The REAL styles root for this bench — the shared theme trio with the shipped
    /// loomforge.scene.json's own blocks (its `stages` section holds the doll source)
    /// merged over it, exactly as the runtime builds them.
    fn real_styles() -> Json {
        let def = flicker::ui::SceneDef::parse(
            "loomforge",
            include_str!("../../../../content/sensorium/scenes/loomforge.scene.json"),
        )
        .expect("the shipped loomforge.scene.json parses");
        flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        )
    }

    /// The doll's source is authored, lit, framed, and made of exactly the layer kinds
    /// the rig draws — so nothing it authors is a name that resolves to nothing.
    #[test]
    fn the_doll_source_is_authored_lit_and_framed() {
        let sources = flicker::ui::stage_defs(&real_styles());
        let p = sources
            .get("portrait")
            .expect("the Loomforge doll source must exist");
        assert!(
            !sources.contains_key("lighting") && !sources.keys().any(|k| k.starts_with('_')),
            "the preset table and comments are not sources"
        );
        let cam = p.camera.expect("the portrait frames its subject");
        assert!(cam.dist > 0.0 && cam.target_y > 0.0, "portrait is framed");
        // studio lighting: a lit sun, unlike the `night` preset.
        assert!(
            p.lighting.sky_sun().color.length() > 0.1,
            "studio preset is lit"
        );
        assert!(
            p.layers.contains(&StageLayer::Skinned),
            "the doll itself must be a layer"
        );
        assert!(
            p.layers
                .iter()
                .any(|l| matches!(l, StageLayer::Ring { .. })),
            "the portrait stands on a ground ring"
        );
        assert!(
            p.layers_outside(DOLL_LAYERS).is_empty(),
            "every authored layer is one the doll rig draws"
        );
    }

    /// Token refs are resolved to rgba before the compiler sees them, and an active ring
    /// swaps colour — the one piece of per-slot state the geometry carries.
    #[test]
    fn ring_lights_when_active() {
        let sources = flicker::ui::stage_defs(&real_styles());
        let layers = &sources["portrait"].layers;
        let idle = line_layers(layers, false);
        let lit = line_layers(layers, true);
        assert_eq!(
            idle.len(),
            lit.len(),
            "activity changes colour, not geometry"
        );
        assert!(!idle.is_empty(), "the ring produced segments");
        assert_ne!(idle[0].1, lit[0].1, "an active ring is a different colour");
        // And the colours are real rgba, not an unresolved `$token` left as a string.
        assert!(idle[0].1.iter().all(|c| c.is_finite()));
    }

    /// A layer the rig does not draw contributes no lines, and a degenerate ring yields
    /// no geometry rather than a panic (guarded in flicker-render's `ring_segments`).
    #[test]
    fn undrawn_and_degenerate_layers_yield_no_lines() {
        let layers = vec![
            StageLayer::Graticule { radius_scale: 1.0 },
            StageLayer::Ring {
                radius: -1.0,
                y: 0.0,
                segments: 24,
                color: [1.0; 4],
                color_active: [1.0; 4],
            },
        ];
        let lines = line_layers(&layers, false);
        assert_eq!(lines.len(), 1, "only the ring is a line layer");
        assert!(lines[0].0.is_empty(), "a negative radius is no ring");
    }

    fn bone(name: &str) -> Bone {
        Bone {
            name: name.into(),
            parent: -1,
            local: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }
    }

    fn vert(y: f32) -> Vertex {
        Vertex {
            p: [0.0, y, 0.0],
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    /// The rig is centred on the origin by `Model::world`, but the authored stages put
    /// the feet at y = 0 — so the ground transform must drop the doll by its lowest
    /// vertex, otherwise it floats above (or sinks through) its own ring.
    #[test]
    fn ground_transform_puts_the_feet_on_the_floor() {
        let g = ground_transform(Mat4::IDENTITY, &[vert(-0.9), vert(0.9)]);
        let lowest = g.transform_point3(Vec3::new(0.0, -0.9, 0.0));
        assert!(
            lowest.y.abs() < 1e-5,
            "lowest vertex must land on y = 0, got {}",
            lowest.y
        );
        // The whole rig shifts together — the top rises by the same drop, it is not scaled.
        let top = g.transform_point3(Vec3::new(0.0, 0.9, 0.0));
        assert!((top.y - 1.8).abs() < 1e-5, "the doll keeps its height");
        // An empty mesh must not produce a NaN transform.
        assert!(ground_transform(Mat4::IDENTITY, &[]).is_finite());
    }

    /// A rest-pose palette is one matrix per bone and all finite — the shape
    /// `draw_skinned_instanced` requires (`palettes.len() == models.len() * bone_count`).
    #[test]
    fn rest_palette_matches_the_bone_count() {
        let bones = [bone("root"), bone("spine"), bone("head")];
        let p = palette_for(&bones, None, 0.0, false);
        assert_eq!(p.len(), 3, "one matrix per bone");
        assert!(p.iter().all(|m| m.is_finite()));
        // A missing clip falls back to the rest pose rather than panicking — the
        // request carries an index the caller already failed to resolve.
        assert_eq!(palette_for(&bones, None, 1.5, false).len(), 3);
    }
}
