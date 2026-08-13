//! **The globe as an instrument** — a planet rendered into a bench's viewport
//! region instead of straight to the swapchain.
//!
//! The difference is not cosmetic. A scene-painted globe is a backdrop that the
//! panels float over; an `rtt` node is a piece of the surface, laid out by the
//! same walker that lays out everything else, so a bench can put instruments
//! beside the planet without either one guessing where the other ended up.
//!
//! The walker never fills the rect it reserves — it runs late, and offscreen
//! passes must run FIRST (they reset the shared per-frame draw queues). So the
//! contract is a hand-off: the walker publishes an `RttSlot` in `update`, and
//! the scene declares this pass against that rect at the top of `render`.
//!
//! Lifted out of `flicker-godmode` when the Populous bench needed the same
//! offscreen plumbing. Rule DDD070C7: the second consumer moves the code, it
//! does not copy it — a forked RTT pass is two places for a target to leak.

use flicker::render::{
    Camera, CompositeTarget, FrameGraph, MeshDrawOptions, MeshHandle, Rect, RenderTargetHandle,
    Renderer, SceneLighting,
};
use glam::{Mat4, Vec3};

/// Line geometry drawn over the globe, **grouped by colour** — one group is one
/// `draw_lines` call. Grouped rather than per-segment coloured because the line
/// pipeline tints a whole batch, and the grouping is meaningful anyway: one
/// group is one plate.
pub type Arrows = Vec<([f32; 4], Vec<(Vec3, Vec3)>)>;

/// No lines over the globe — the empty `Arrows` a bench passes when its planet
/// carries no overlay. A `const` so a caller with nothing to draw does not
/// allocate a `Vec` per frame to say so.
pub const NO_ARROWS: &Arrows = &Vec::new();

/// One authored `layers[]` entry of a globe stage — WHAT the world is made of,
/// in draw order, exactly as the other stage sources author their rings and
/// grids (`flicker-loomforge/src/stage.rs`). A globe's layers are the same idea
/// at planetary scale: shells of the world's own tiling, and the reference
/// frame over them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StageLayer {
    /// The scene's own shell list draws here — the sparse, per-cell-coloured
    /// stack a simulation publishes, which no style file could author.
    Shells,
    /// A STATIC shell of the world's own tiling, authored whole: how far out it
    /// sits (a fraction of [`crate::RADIUS`]), how far each tile's corners pull
    /// toward its centre, and the one colour it is drawn in. Two of these — a
    /// near-black full shell under an inset coloured one — are how a hex map
    /// gets outlines with no per-edge geometry.
    Shell {
        radius_scale: f32,
        inset: f32,
        color: [f32; 3],
    },
    /// The shared reference frame ([`crate::graticule`]) at this fraction of the
    /// radius — authored rather than hardcoded so the clearance is a style
    /// decision like every other.
    Graticule { radius_scale: f32 },
}

/// The authored look of a globe stage: the light it is seen by, the backdrop it
/// sits on, and the layers it is drawn from.
///
/// **No camera.** Every other stage in the catalog frames a fixed subject and
/// authors the framing with it; a globe is flown by the maintainer, so the
/// scene's own orbit camera owns the view and this struct deliberately does not.
/// Read from `stages.<source>` rather than hardcoded, because a config nothing
/// reads is an authored name that resolves to nothing.
#[derive(Clone, Debug)]
pub struct GlobeStage {
    pub lighting: SceneLighting,
    /// Clear colour for the offscreen target. Transparent by default so the
    /// node's own panel supplies the backdrop.
    pub clear: [f64; 4],
    /// What is drawn, in authored order.
    pub layers: Vec<StageLayer>,
}

impl Default for GlobeStage {
    fn default() -> Self {
        Self {
            lighting: SceneLighting::default(),
            clear: [0.0, 0.0, 0.0, 0.0],
            // A stage that authors no layers still shows the scene's own
            // shells: the picture is never the thing a missing style costs.
            layers: vec![StageLayer::Shells],
        }
    }
}

impl GlobeStage {
    /// Parse `stages.<source>` out of the loaded `ui_theme.json`.
    ///
    /// Best-effort: anything missing keeps the default, because a malformed
    /// style file must not leave a bench with a black hole and no explanation.
    /// What it must never do is silently ignore a value that IS authored —
    /// hence the warnings.
    pub fn from_styles(styles: &serde_json::Value, source: &str) -> Self {
        let mut out = GlobeStage::default();
        let Some(stage) = styles.get("stages").and_then(|s| s.get(source)) else {
            tracing::warn!("stages.{source} is not authored — the globe uses defaults");
            return out;
        };
        // `lighting` NAMES a block in the shared `stages.lighting` table, the
        // same indirection every other stage source uses.
        if let Some(name) = stage.get("lighting").and_then(|v| v.as_str()) {
            match styles.get("stages").and_then(|s| s.get("lighting")).and_then(|l| l.get(name)) {
                Some(l) => {
                    let v3 = |k: &str| -> Option<Vec3> {
                        let a = l.get(k)?.as_array()?;
                        Some(Vec3::new(
                            a.first()?.as_f64()? as f32,
                            a.get(1)?.as_f64()? as f32,
                            a.get(2)?.as_f64()? as f32,
                        ))
                    };
                    if let Some(v) = v3("sun_dir") {
                        out.lighting.sun_dir = v.normalize_or_zero();
                    }
                    if let Some(v) = v3("sun") {
                        out.lighting.sun_color = v;
                    }
                    if let Some(v) = v3("moon_dir") {
                        out.lighting.moon_dir = v.normalize_or_zero();
                    }
                    if let Some(v) = v3("moon") {
                        out.lighting.moon_color = v;
                    }
                    if let Some(v) = v3("ambient") {
                        out.lighting.ambient = v;
                    }
                }
                None => tracing::warn!("stages.lighting.{name} is not authored"),
            }
        }
        // `clear` and `layers` are authored on every globe source in the shared
        // style file and were read by NOTHING until this pass — an authored name
        // that resolved to silence (rule 4BB12A75). `load_styles` has already
        // turned a `$token` into its four floats by the time the block gets here,
        // which is why a colour arrives as an array.
        if let Some(c) = jcolor(stage.get("clear")) {
            out.clear = c.map(f64::from);
        }
        if let Some(a) = stage.get("layers").and_then(|l| l.as_array()) {
            out.layers = a.iter().filter_map(parse_layer).collect();
        }
        out
    }
}

/// An already token-resolved rgba array, or `None` when the key is absent.
fn jcolor(v: Option<&serde_json::Value>) -> Option<[f32; 4]> {
    let a = v?.as_array()?;
    let mut out = [0.0f32; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = a.get(i).and_then(|c| c.as_f64()).unwrap_or(0.0) as f32;
    }
    Some(out)
}

fn jnum(v: &serde_json::Value, key: &str, default: f32) -> f32 {
    v.get(key)
        .and_then(|n| n.as_f64())
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .unwrap_or(default)
}

/// One `layers[]` entry → a [`StageLayer`]. Modelled on the shared stage parser
/// in `flicker-loomforge/src/stage.rs`, including its fail-loud warn: an unknown
/// `draw` kind is a typo an author must SEE, so it is named and skipped rather
/// than silently drawing nothing.
fn parse_layer(v: &serde_json::Value) -> Option<StageLayer> {
    match v.get("draw").and_then(|d| d.as_str())? {
        "shells" => Some(StageLayer::Shells),
        "shell" => {
            let c = jcolor(v.get("color")).unwrap_or([1.0; 4]);
            Some(StageLayer::Shell {
                radius_scale: jnum(v, "radius_scale", 1.0),
                inset: jnum(v, "inset", 0.0),
                color: [c[0], c[1], c[2]],
            })
        }
        "graticule" => Some(StageLayer::Graticule { radius_scale: jnum(v, "radius_scale", 1.0) }),
        other => {
            tracing::warn!("globe stage: unknown layer draw kind {other:?} — skipped");
            None
        }
    }
}

/// A globe's offscreen target, sized to whatever rect the walker reserved.
#[derive(Default)]
pub struct GlobeView {
    target: Option<RenderTargetHandle>,
    size: (u32, u32),
}

impl GlobeView {
    /// Declare this frame's offscreen pass and composite it into `rect`.
    ///
    /// The meshes stay the scene's — this module owns the TARGET, not the
    /// planet. `camera` is the maintainer's live orbit camera, which is why the
    /// stage authors no framing of its own.
    ///
    /// `arrows` are pre-grouped line segments (one group per colour) drawn
    /// INSIDE this pass — they must be, because the offscreen pass has its own
    /// frame queues: a `draw_lines` issued outside it would be thrown away by
    /// the next `begin_frame` and never reach the globe. Borrowed for the
    /// graph's lifetime rather than cloned; at a few thousand arrows the copy
    /// would be per-frame bandwidth spent on nothing. A bench with no overlay
    /// passes [`NO_ARROWS`].
    // Nine arguments, and they are nine independent facts about ONE draw: the
    // renderer, the graph, where, how deep, from what angle, lit how, of what,
    // with what over it. Bundling them into a struct would move the same list
    // one line up and cost every caller a name for it.
    #[allow(clippy::too_many_arguments)]
    pub fn render<'f>(
        &mut self,
        r: &mut Renderer,
        fg: &mut FrameGraph<'f>,
        rect: Rect,
        layer: f32,
        camera: Camera,
        stage: &GlobeStage,
        meshes: &[MeshHandle],
        arrows: &'f Arrows,
    ) {
        let w = (rect.size.x.round() as u32).max(1);
        let h = (rect.size.y.round() as u32).max(1);
        match self.target {
            Some(_) if self.size == (w, h) => {}
            Some(t) => {
                r.resize_render_target(t, w, h);
                self.size = (w, h);
            }
            None => {
                self.target = Some(r.create_render_target(w, h));
                self.size = (w, h);
            }
        }
        let Some(target) = self.target else { return };

        let lighting = stage.lighting;
        let opts = MeshDrawOptions::default();
        let meshes: Vec<MeshHandle> = meshes.to_vec();
        fg.target(target, stage.clear, move |r| {
            r.set_scene(lighting);
            r.set_camera(&camera);
            for h in meshes {
                r.draw_mesh(h, Mat4::IDENTITY, opts);
            }
            // Depth-tested against the shells, so a heading on the far side of
            // the world is hidden by the world — which is what makes the arrows
            // read as standing ON the globe rather than floating around it.
            for (color, segments) in arrows {
                r.draw_lines(segments, *color);
            }
        });
        // `frame: None` — the walker already drew the node's `rtt_holder` panel
        // on the 2D path, so a second frame here would double the chrome.
        fg.composite_panel(target, CompositeTarget::Screen, rect, layer, [1.0; 4], None, None);
    }

    /// Give the target back. A bench that leaves its viewport (scene `exit`)
    /// holds GPU memory for a picture nobody is looking at otherwise.
    pub fn free(&mut self, r: &mut Renderer) {
        if let Some(t) = self.target.take() {
            r.free_render_target(t);
            self.size = (0, 0);
        }
    }
}
