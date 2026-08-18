//! The **Stage** — the live-animating character doll composited into the bench's UI.
//!
//! A stage is a panel whose fill is a render-to-texture sub-scene: something says WHERE
//! (a walker `stage` node reserving a [`RttSlot`], or the scene-owned canvas placing a
//! card), the shared `stages` block in `ui_theme.json` says WHAT, and `FrameGraph`
//! decides WHEN. This module owns the WHAT→GPU half: parsing the authored sources,
//! posing the doll, and declaring the offscreen passes.
//!
//! **Liveness IS the poster cache.** A slot that is not `live` simply skips its
//! `FrameGraph::target` pass — its render target keeps the last image drawn into it and
//! still composites, so a still doll costs zero GPU work per frame and needs no separate
//! poster texture. Only the selected card and the pointed-at clip row animate, which is
//! what makes a screen carrying a dozen dolls affordable (N live targets = N submits).
//!
//! Parsing and framing are pure functions over JSON — unit-tested without a device; only
//! [`StageRig::load`] and [`StageRig::stage`] touch the renderer.

use std::collections::HashMap;

use flicker::render::{
    grid_segments, ring_segments, Camera, CompositeTarget, FrameGraph, Mat4, MeshIndices, Rect,
    RenderTargetHandle, Renderer, SceneLighting, SkinnedMeshHandle, SkinnedVertex, Vec3,
};
use flicker_skeletal::format::{Bone, Model, ResolvedClip, Vertex};
use flicker_skeletal::{pose, skin};
use serde_json::Value as Json;

/// One authored draw layer of a stage source (`stages.<source>.layers[]`).
#[derive(Clone, Debug, PartialEq)]
pub enum Layer {
    /// The posed character. The JSON names `model_bind`/`pose_bind`, but an editor drives
    /// its own dolls: the pose comes from this scene's document, not the walker's model
    /// map, so those keys are declarations of intent rather than lookups here.
    Skinned,
    /// The ground ring under the doll — lit differently when the slot is active.
    Ring {
        radius: f32,
        y: f32,
        segments: usize,
        color: [f32; 4],
        color_active: [f32; 4],
    },
    /// The faint floor grid under a turntable shot.
    Grid {
        spacing: f32,
        extent: f32,
        y: f32,
        color: [f32; 4],
    },
}

/// A named sub-scene: how to light it, where to put the camera, and what to draw.
#[derive(Clone, Debug)]
pub struct Source {
    pub lighting: SceneLighting,
    pub clear: [f64; 4],
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target_y: f32,
    pub layers: Vec<Layer>,
}

/// What the scene wants staged in one slot this frame.
///
/// `rect` is the IMAGE rect — the walker's `RttSlot` already inset it inside the node's
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
    /// Re-render this frame. `false` reuses the target's last image (the poster).
    pub live: bool,
    /// Clip index to pose with; `None` → the rest pose.
    pub clip: Option<usize>,
    /// Play-head, seconds. The clip loops on its own duration.
    pub time: f32,
    /// Selected / hovered — lights the ground ring in its active colour.
    pub active: bool,
}

/// One slot's offscreen target, plus whether anything has been drawn into it yet (a
/// never-drawn slot must render once even when it is not live, or it would blit garbage).
struct Slot {
    target: RenderTargetHandle,
    w: u32,
    h: u32,
    drawn: bool,
}

/// The GPU-side stage rig: the doll's skinned mesh, the authored sources, and one render
/// target per slot id.
#[derive(Default)]
pub struct StageRig {
    sources: HashMap<String, Source>,
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

    /// Parse the authored sources and upload the doll's skinned mesh. Call from `enter`,
    /// once the styles are resolved and the document's model is loaded.
    pub fn load(&mut self, r: &mut Renderer, model: &Model, styles: &Json) {
        self.sources = parse_sources(styles);
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

    /// Declare this frame's stage passes and composites on `fg`.
    ///
    /// Must be followed by `fg.execute(r)` as the FIRST thing in `render()` — offscreen
    /// passes reset the shared draw queues, so any main-frame geometry queued before it
    /// would be dropped.
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
            let w = (req.rect.size.x.round() as u32).max(1);
            let h = (req.rect.size.y.round() as u32).max(1);

            let slot = match slots.get_mut(&req.id) {
                Some(s) => {
                    // A resized slot's old image is the wrong shape — redraw it.
                    if s.w != w || s.h != h {
                        r.resize_render_target(s.target, w, h);
                        s.w = w;
                        s.h = h;
                        s.drawn = false;
                    }
                    s
                }
                None => slots.entry(req.id.clone()).or_insert(Slot {
                    target: r.create_render_target(w, h),
                    w,
                    h,
                    drawn: false,
                }),
            };

            if req.live || !slot.drawn {
                // Everything the pass needs is OWNED here, so the closure borrows nothing
                // and can outlive this loop iteration.
                let palette = palette_for(
                    &model.bones,
                    req.clip.and_then(|i| model.clips.get(i)),
                    req.time,
                    model.retarget,
                );
                let cam = Camera::orbit(
                    Vec3::new(0.0, src.target_y, 0.0),
                    src.dist,
                    src.yaw,
                    src.pitch,
                );
                let lighting = src.lighting;
                let lines = line_layers(&src.layers, req.active);
                let (ground, mesh, bones) = (*ground, *mesh, *bone_count);
                fg.target(slot.target, src.clear, move |r| {
                    r.set_scene(lighting);
                    r.set_camera(&cam);
                    // Line layers are depth-tested against the doll, so the authored
                    // order costs nothing to honour and a ring still reads under the feet.
                    for (segs, color) in &lines {
                        r.draw_lines(segs, *color);
                    }
                    if let Some(m) = mesh {
                        r.draw_skinned_instanced(m, &[ground], &palette, bones);
                    }
                });
                slot.drawn = true;
            }

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

/// The line geometry of a source's non-character layers, in authored order.
fn line_layers(layers: &[Layer], active: bool) -> Vec<LineLayer> {
    layers
        .iter()
        .filter_map(|l| match l {
            Layer::Skinned => None,
            Layer::Ring {
                radius,
                y,
                segments,
                color,
                color_active,
            } => Some((
                ring_segments(Vec3::new(0.0, *y, 0.0), *radius, *segments),
                if active { *color_active } else { *color },
            )),
            Layer::Grid {
                spacing,
                extent,
                y,
                color,
            } => Some((grid_segments(*spacing, *extent, *y), *color)),
        })
        .collect()
}

// ── Parsing the authored `stages` block ──────────────────────────────────────

/// Every named source in `stages`, keyed by name. `lighting` holds the shared presets
/// and `_`-prefixed keys are comments, so neither is a source.
pub fn parse_sources(styles: &Json) -> HashMap<String, Source> {
    let Some(stages) = styles.get("stages").and_then(Json::as_object) else {
        return HashMap::new();
    };
    let presets = stages.get("lighting");
    stages
        .iter()
        .filter(|(k, _)| !k.starts_with('_') && k.as_str() != "lighting")
        .map(|(name, v)| (name.clone(), parse_source(v, presets)))
        .collect()
}

fn parse_source(v: &Json, presets: Option<&Json>) -> Source {
    let cam = v.get("camera");
    Source {
        lighting: parse_lighting(presets, v.get("lighting").and_then(Json::as_str)),
        clear: jcolor(v, "clear", [0.0; 4]).map(|c| c as f64),
        yaw: jnum(cam, "yaw", 0.55),
        pitch: jnum(cam, "pitch", 0.18),
        dist: jnum(cam, "dist", 2.6),
        target_y: jnum(cam, "target_y", 0.95),
        layers: v
            .get("layers")
            .and_then(Json::as_array)
            .map(|a| a.iter().filter_map(parse_layer).collect())
            .unwrap_or_default(),
    }
}

fn parse_layer(v: &Json) -> Option<Layer> {
    match v.get("draw").and_then(Json::as_str)? {
        "skinned" => Some(Layer::Skinned),
        "ring" => Some(Layer::Ring {
            radius: jnum(Some(v), "radius", 0.45),
            y: jnum(Some(v), "y", 0.0),
            segments: jnum(Some(v), "segments", 24.0).max(0.0) as usize,
            color: jcolor(v, "color", [0.72, 0.59, 0.35, 1.0]),
            // A source may omit the active colour; then the ring simply never lights.
            color_active: jcolor(
                v,
                "color_active",
                jcolor(v, "color", [0.72, 0.59, 0.35, 1.0]),
            ),
        }),
        "grid" => Some(Layer::Grid {
            spacing: jnum(Some(v), "spacing", 0.5),
            extent: jnum(Some(v), "extent", 6.0),
            y: jnum(Some(v), "y", 0.0),
            color: jcolor(v, "color", [0.55, 0.63, 0.75, 0.09]),
        }),
        other => {
            tracing::warn!("loomforge stage: unknown layer draw kind {other:?} — skipped");
            None
        }
    }
}

fn parse_lighting(presets: Option<&Json>, name: Option<&str>) -> SceneLighting {
    let d = SceneLighting::default();
    let Some(p) = presets.zip(name).and_then(|(p, n)| p.get(n)) else {
        return d;
    };
    let p = Some(p);
    SceneLighting {
        sun_dir: jvec3(p, "sun_dir", d.sun_dir).normalize_or_zero(),
        sun_color: jvec3(p, "sun", d.sun_color),
        moon_dir: jvec3(p, "moon_dir", d.moon_dir).normalize_or_zero(),
        moon_color: jvec3(p, "moon", d.moon_color),
        ambient: jvec3(p, "ambient", d.ambient),
        sky_zenith: jvec3(p, "sky_zenith", d.sky_zenith),
        sky_horizon: jvec3(p, "sky_horizon", d.sky_horizon),
        ..d
    }
}

fn jnum(v: Option<&Json>, key: &str, default: f32) -> f32 {
    v.and_then(|v| v.get(key))
        .and_then(Json::as_f64)
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .unwrap_or(default)
}

fn jvec3(v: Option<&Json>, key: &str, default: Vec3) -> Vec3 {
    let Some(a) = v.and_then(|v| v.get(key)).and_then(Json::as_array) else {
        return default;
    };
    if a.len() < 3 {
        return default;
    }
    let c = |i: usize| a[i].as_f64().unwrap_or(0.0) as f32;
    let out = Vec3::new(c(0), c(1), c(2));
    if out.is_finite() {
        out
    } else {
        default
    }
}

/// An rgba from an already token-resolved colour array (`resolve_tokens` has turned
/// `$bronze` into four floats by the time the styles reach here).
fn jcolor(v: &Json, key: &str, default: [f32; 4]) -> [f32; 4] {
    let Some(a) = v.get(key).and_then(Json::as_array) else {
        return default;
    };
    let mut out = default;
    for (i, c) in a.iter().take(4).enumerate() {
        out[i] = c.as_f64().unwrap_or(default[i] as f64) as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_skeletal::format::{Bone, Vertex};

    /// The REAL shared `stages` block — the parser must understand what is actually
    /// authored, not a fixture that drifts from it.
    fn real_styles() -> Json {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/sensorium/resources/ui_theme.json"
        );
        flicker::ui::load_styles(path)
    }

    #[test]
    fn parses_the_shared_stage_sources() {
        let styles = real_styles();
        let sources = parse_sources(&styles);
        assert!(
            sources.contains_key("portrait"),
            "the Loomforge doll source must exist"
        );
        assert!(
            sources.contains_key("turntable"),
            "the TAE preview source must exist"
        );
        assert!(
            !sources.contains_key("lighting"),
            "the preset table is not a source"
        );
        assert!(
            !sources.keys().any(|k| k.starts_with('_')),
            "comments are not sources"
        );

        let p = &sources["portrait"];
        assert!(p.dist > 0.0 && p.target_y > 0.0, "portrait is framed");
        // studio lighting: a lit sun, unlike the `night` preset.
        assert!(p.lighting.sun_color.length() > 0.1, "studio preset is lit");
        assert!(
            p.layers.contains(&Layer::Skinned),
            "the doll itself must be a layer"
        );
        assert!(
            p.layers.iter().any(|l| matches!(l, Layer::Ring { .. })),
            "the portrait stands on a ground ring"
        );
        // The turntable is the wider shot over a floor grid.
        let t = &sources["turntable"];
        assert!(
            t.dist > p.dist,
            "the turntable frames wider than the portrait"
        );
        assert!(t.layers.iter().any(|l| matches!(l, Layer::Grid { .. })));
    }

    /// Token refs are resolved to rgba before the parser sees them, and an active ring
    /// swaps colour — the one piece of per-slot state the geometry carries.
    #[test]
    fn ring_lights_when_active() {
        let sources = parse_sources(&real_styles());
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

    /// Authored JSON reaches the parser unvalidated; a malformed source must degrade to
    /// defaults rather than panic or emit non-finite camera/geometry values.
    #[test]
    fn malformed_sources_degrade_to_defaults() {
        let styles: Json = serde_json::json!({
            "stages": {
                "_comment": "not a source",
                "broken": {
                    "lighting": "no_such_preset",
                    "camera": { "dist": "nonsense", "yaw": null },
                    "layers": [
                        { "draw": "ring", "radius": -1.0, "segments": 24 },
                        { "draw": "unheard_of" },
                        { "no_draw_key": true }
                    ]
                }
            }
        });
        let sources = parse_sources(&styles);
        let b = &sources["broken"];
        assert!(b.dist.is_finite() && b.dist > 0.0, "bad dist falls back");
        assert!(b.yaw.is_finite());
        assert_eq!(
            b.layers.len(),
            1,
            "only the ring parsed; unknown kinds dropped"
        );
        // A negative radius yields no geometry rather than a panic (guarded upstream in
        // flicker-render's ring_segments).
        assert!(line_layers(&b.layers, false)[0].0.is_empty());
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
