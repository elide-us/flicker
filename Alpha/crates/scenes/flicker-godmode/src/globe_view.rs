//! **The globe as an instrument** — the forming planet rendered into the bench's
//! centre viewport instead of straight to the swapchain.
//!
//! The difference is not cosmetic. A scene-painted globe is a backdrop that the
//! panels float over; an [`rtt`] node is a piece of the surface, laid out by the
//! same walker that lays out everything else, so the bench can put instruments
//! beside the planet without either one guessing where the other ended up.
//!
//! The walker never fills the rect it reserves — it runs late, and offscreen
//! passes must run FIRST (they reset the shared per-frame draw queues). So the
//! contract is a hand-off: the walker publishes an [`RttSlot`] in `update`, and
//! the scene declares this pass against that rect at the top of `render`.
//!
use flicker::render::{
    Camera, CompositeTarget, FrameGraph, MeshDrawOptions, MeshHandle, Rect, RenderTargetHandle,
    Renderer, SceneLighting,
};
use glam::{Mat4, Vec3};

/// The `stages.<source>` block this view is authored by, and the `source` the
/// bench's `rtt` node names. One string, both sides.
pub const STAGE_SOURCE: &str = "godmode_globe";

/// Line geometry drawn over the globe, **grouped by colour** — one group is one
/// `draw_lines` call. Grouped rather than per-segment coloured because the line
/// pipeline tints a whole batch, and the grouping is meaningful anyway: one
/// group is one plate.
pub type Arrows = Vec<([f32; 4], Vec<(Vec3, Vec3)>)>;

/// The authored look of the globe stage: the light it is seen by and the
/// backdrop it sits on.
///
/// **No camera.** Every other stage in the catalog frames a fixed subject and
/// authors the framing with it; this one is flown by the maintainer, so the
/// scene's own orbit camera owns the view and this struct deliberately does not.
/// Read from `stages.<source>` rather than hardcoded, because a config nothing
/// reads is an authored name that resolves to nothing.
#[derive(Clone, Copy, Debug)]
pub struct GlobeStage {
    pub lighting: SceneLighting,
    /// Clear colour for the offscreen target. Transparent by default so the
    /// node's own panel supplies the backdrop.
    pub clear: [f64; 4],
}

impl Default for GlobeStage {
    fn default() -> Self {
        Self { lighting: SceneLighting::default(), clear: [0.0, 0.0, 0.0, 0.0] }
    }
}

impl GlobeStage {
    /// Parse `stages.<source>` out of the loaded `ui_elements.json`.
    ///
    /// Best-effort: anything missing keeps the default, because a malformed
    /// style file must not leave the bench with a black hole and no
    /// explanation. What it must never do is silently ignore a value that IS
    /// authored — hence the warnings.
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
        out
    }
}

/// The globe's offscreen target, sized to whatever rect the walker reserved.
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
    /// `arrows` are pre-grouped line segments (one group per colour) drawn INSIDE
    /// this pass — they must be, because the offscreen pass has its own frame
    /// queues: a `draw_lines` issued outside it would be thrown away by the next
    /// `begin_frame` and never reach the globe. Borrowed for the graph's lifetime
    /// rather than cloned; at a few thousand arrows the copy would be per-frame
    /// bandwidth spent on nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn render<'f>(
        &mut self,
        r: &mut Renderer,
        fg: &mut FrameGraph<'f>,
        rect: Rect,
        layer: f32,
        camera: Camera,
        stage: GlobeStage,
        core: Option<MeshHandle>,
        shells: &[MeshHandle],
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
        let meshes: Vec<MeshHandle> = core.into_iter().chain(shells.iter().copied()).collect();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles() -> serde_json::Value {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/sensorium/resources/ui_elements.json"
        ))
        .expect("ui_elements.json reads");
        serde_json::from_str(&raw).expect("ui_elements.json parses")
    }

    /// **The authored stage is actually READ.** The bench declares
    /// `stages.godmode_globe`, and a declaration nothing consumes is a name
    /// that resolves to nothing — which is how Sablework's lit view once
    /// shipped lit by a constant while its own `"lighting": "studio"` sat
    /// unused. So: the parsed stage must differ from the bare default, and it
    /// must emit light.
    #[test]
    fn the_authored_globe_stage_is_read() {
        let s = GlobeStage::from_styles(&styles(), STAGE_SOURCE);
        let bare = GlobeStage::default();
        assert!(
            s.lighting.sun_dir != bare.lighting.sun_dir
                || s.lighting.sun_color != bare.lighting.sun_color
                || s.lighting.ambient != bare.lighting.ambient,
            "stages.{STAGE_SOURCE} is authored but nothing in it reached the view"
        );
        assert!(
            s.lighting.sun_color.length_squared() > 0.0
                || s.lighting.ambient.length_squared() > 0.0,
            "the globe would render black"
        );
    }

    /// An unknown source must fall back LIT, not black: a typo in a style file
    /// should cost the authored look, never the picture.
    #[test]
    fn an_unknown_source_still_lights_the_globe() {
        let s = GlobeStage::from_styles(&styles(), "no_such_stage");
        assert!(
            s.lighting.sun_color.length_squared() > 0.0
                || s.lighting.ambient.length_squared() > 0.0,
            "the fallback must still light the globe"
        );
    }
}
