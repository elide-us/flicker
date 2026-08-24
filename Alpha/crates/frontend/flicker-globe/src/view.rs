//! **The globe as an instrument** — a planet rendered into a bench's viewport
//! region instead of straight to the swapchain.
//!
//! The difference is not cosmetic. A scene-painted globe is a backdrop that the
//! panels float over; a nested `surface` node is a piece of the screen, laid out by the
//! same walker that lays out everything else, so a bench can put instruments
//! beside the planet without either one guessing where the other ended up.
//!
//! The walker never fills the rect it reserves — it runs late (its HUD is the screen
//! surface's final 2D, an overlay). So the contract is a hand-off: the walker publishes
//! a `SurfaceSlot` in `update`, and the scene declares this pass against that rect in
//! `render`, into the frame's shared graph.
//!
//! What the globe is LIT by, cleared to and made of is the authored stage
//! ([`StageDef`], compiled by the one stage compiler in `flicker-widgets`); the
//! layer kinds a globe draws are `shells` / `shell` / `graticule` — see
//! [`GLOBE_LAYERS`]. A globe stage authors NO camera: the maintainer flies the
//! planet, so the scene's own orbit camera owns the view.
//!
//! Lifted out of `flicker-godmode` when the Populous bench needed the same
//! offscreen plumbing. Rule DDD070C7: the second consumer moves the code, it
//! does not copy it — a forked RTT pass is two places for a target to leak.

use flicker::render::{
    Camera, CompositeTarget, FrameGraph, MeshDrawOptions, MeshHandle, Rate, Rect,
    RenderTargetHandle, Renderer, StageDef, StageInputs,
};
use flicker::ui::SurfaceSlot;
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

/// The stage layer kinds a globe draws: the scene's published shells, an authored
/// static shell, and the shared reference frame. Anything else a globe stage
/// authors is named at construction rather than drawn as nothing.
pub const GLOBE_LAYERS: &[&str] = &["shells", "shell", "graticule"];

/// Where and how a nested globe surface is seated this frame — the walker's
/// [`SurfaceSlot`] reduced to what the view honours: the image rect, the node's
/// sub-layer, its composite tint, and its [`Rate`] (a surface authored `poster` keeps
/// its last image and skips the pass — the poster rule).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seat {
    pub rect: Rect,
    pub layer: f32,
    pub tint: [f32; 4],
    pub rate: Rate,
}

impl From<&SurfaceSlot> for Seat {
    fn from(s: &SurfaceSlot) -> Self {
        Seat {
            rect: Rect {
                pos: glam::Vec2::new(s.x, s.y),
                size: glam::Vec2::new(s.w, s.h),
            },
            layer: s.layer,
            tint: s.tint,
            rate: s.rate,
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
    /// INSIDE this pass's content closure — that is where the surface's own
    /// geometry goes. Borrowed for the graph's lifetime rather than cloned; at a
    /// few thousand arrows the copy would be per-frame bandwidth spent on nothing.
    /// A bench with no overlay passes [`NO_ARROWS`].
    // Nine arguments, and they are nine independent facts about ONE draw: the
    // renderer, the graph, where, how deep, from what angle, lit how, of what,
    // with what over it. Bundling them into a struct would move the same list
    // one line up and cost every caller a name for it.
    #[allow(clippy::too_many_arguments)]
    pub fn render<'f>(
        &mut self,
        r: &mut Renderer,
        fg: &mut FrameGraph<'f>,
        seat: Seat,
        base_layer: f32,
        camera: Camera,
        stage: &StageDef,
        meshes: &[MeshHandle],
        arrows: &'f Arrows,
    ) {
        let rect = seat.rect;
        let (w, h) = stage.attachments.pixels(rect.size);
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

        // Liveness is the seat's rate, driven by the renderer's per-surface clock: a poster
        // skips the pass and composites the image it already holds, a never-drawn target
        // renders once. The composite below runs regardless — the poster rule.
        fg.surface(
            CompositeTarget::Target(target),
            stage,
            StageInputs::default(),
            seat.rate,
            Self::draw_pass(camera, meshes, arrows),
        );
        // `frame: None` — the walker already drew the node's holder panel on the 2D
        // path, so a second frame here would double the chrome. The composite lands at
        // the node's own sub-layer above the scene's base, with the node's tint.
        fg.composite_panel(
            target,
            CompositeTarget::Screen,
            rect,
            base_layer + seat.layer,
            seat.tint,
            None,
            None,
        );
    }

    /// Declare the globe as an element of the ROOT surface — straight into the
    /// swapchain, no target, no composite, no blit. The full-window planet (Epoch
    /// Simulation) draws this way; a windowed globe goes through [`Self::render`].
    /// Needs no view state, which is the point: a root globe allocates nothing.
    pub fn render_root<'f>(
        fg: &mut FrameGraph<'f>,
        camera: Camera,
        stage: &StageDef,
        meshes: &[MeshHandle],
        arrows: &'f Arrows,
    ) {
        fg.surface(
            CompositeTarget::Screen,
            stage,
            StageInputs::default(),
            // A root globe fills the window and renders every frame; the stage's rate is
            // passed for completeness (a non-live one is named and ignored by the graph —
            // the screen keeps no image to poster).
            stage.rate,
            Self::draw_pass(camera, meshes, arrows),
        );
    }

    /// The ONE draw body both surfaces share: the lit shells, then the arrows. The
    /// stage's LIGHTING is applied by the frame graph from the definition; the camera
    /// is not, because a globe stage authors no framing — the maintainer flies the
    /// planet, and that live orbit camera is what this sets.
    fn draw_pass<'f>(
        camera: Camera,
        meshes: &[MeshHandle],
        arrows: &'f Arrows,
    ) -> impl FnOnce(&mut Renderer) + 'f {
        let opts = MeshDrawOptions::default();
        let meshes: Vec<MeshHandle> = meshes.to_vec();
        move |r| {
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
        }
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
