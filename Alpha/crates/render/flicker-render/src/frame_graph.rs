//! The frame graph — the single owner of render-target draw order and compositing.
//!
//! A [`FrameGraph`] is an **ephemeral, per-frame** driver an app builds inside its
//! `render()`: it records each offscreen target's self-contained sub-scene plus the
//! *composites* that draw one target's result into another (a UI panel, or a
//! world-space billboard). On [`FrameGraph::execute`] it topologically orders the
//! passes so every target is rendered before anything that samples it, drives the
//! existing [`Renderer::render_to_texture`] per offscreen target, and injects the
//! composite blits automatically — so callers stop hand-ordering targets and
//! hand-writing panel/sprite/label blits (the two footguns that produced the
//! "panels behind the HUD" and "free-a-target-mid-loop" bugs).
//!
//! It is NOT a field of [`Renderer`]: keeping it ephemeral lets its draw closures
//! borrow frame-local data (the app already pre-extracts its per-frame locals), and
//! sidesteps aliasing — `execute` moves each closure out and hands it the disjoint
//! `&mut Renderer`.
//!
//! **Contract:** the scene manager builds ONE `FrameGraph` per frame and calls
//! [`FrameGraph::execute`] on it EXACTLY ONCE, after every visible scene has declared
//! into it. A scene's `render` is declare-only — it records targets, roots, screen
//! composites, and its final 2D [`overlay`](FrameGraph::overlay); it never executes a
//! graph of its own. `execute` runs the four phases in order — every offscreen pass, then
//! every root element, then the screen composites, then the overlays — so the shared
//! per-frame draw queues are reset only by an offscreen pass, and only while those queues
//! are empty. A target's draw closure may use everything the main frame can, including the
//! depth-sampling passes (volumetric disk, ground fog): every surface carries its own
//! depth, and the renderer binds the pass to the depth of the surface it is encoding.
//!
//! **The root surface.** The screen is a surface too: a scene whose live content fills
//! the window (a world, a 2D game's play field, the boot widget) declares it with
//! [`FrameGraph::root`] instead of an offscreen target — no extra render target, no blit.
//! Roots run after every offscreen pass (never destroyed by a later one) and before the
//! screen composites (nested surfaces land over them). The screen surface's FINAL 2D — a
//! scene's HUD replay and any immediate 2D — is declared with [`FrameGraph::overlay`] and
//! runs in the last phase, AFTER the composites, reproducing the submission order the 2D
//! drawn after a per-scene `execute` used to have. ANY draw — 2D or 3D — queued OUTSIDE a
//! declared pass is counted and reported by the renderer at `end_frame` — the screen
//! surface is declared, not assumed.
//!
//! **Layer bands ride the graph.** A scene occupies a wide depth band (its stack position
//! × the scene stride). The manager stamps the band with [`FrameGraph::set_base_layer`]
//! before each scene declares, every declared element records the band that was current,
//! and `execute` restores that band before running the element — so a scene reads its band
//! back with [`FrameGraph::base_layer`] at declare time and the deferred draw lands in it.
//!
//! **A surface's RECIPE.** [`FrameGraph::target`] and [`FrameGraph::root`] are the
//! recipe-LESS primitives: a closure, a clear colour, and nothing said about what the
//! engine draws around it. [`FrameGraph::surface`] is the same two destinations driven by
//! a [`StageDef`] instead — the authored WHAT of the surface:
//!
//! - the stage's lighting is applied before anything draws, and its framing too when the
//!   stage authors one (a stage with no `camera` leaves the view to the content closure —
//!   that is what `camera: None` MEANS, and it is how the globes work);
//! - the stage's `clear` reaches whichever destination it was given: an offscreen target
//!   is cleared to it, and a ROOT surface's clear is the WINDOW's — written to
//!   [`Renderer::clear_color`], which `end_frame` clears the swapchain with. Absence is
//!   typed (`clear: None`) and means transparent offscreen / untouched on screen, so no
//!   destination silently drops an authored backdrop;
//! - then each [`PassDef`] of [`StageDef::recipe`] is applied in [`StageDef::pass_order`]
//!   — the content closure IS the [`PassKind::Scene`] pass, so a stage authoring no
//!   `passes` runs exactly the closure, exactly where `target`/`root` would have run it;
//! - the per-frame numbers a simulation owns arrive as [`StageInputs`] and REPLACE the
//!   fields the recipe binds them to, so the recipe itself is compiled once at load.
//!
//! Ordering is read-after-write over the surface's [`Attachments`](crate::Attachments),
//! with declaration order as the tie-break — the same Kahn ordering this module already uses ACROSS
//! surfaces, applied one level down. Today's encoder still fixes the order it encodes in
//! (sky, then content, then volumetric, then ground fog), which every derived order for
//! the current roster agrees with; a recipe that would need a different one is refused by
//! the stage compiler rather than silently re-ordered here.

use crate::{
    depth_plan, AttachmentFormat, Attachments, FontRole, PassDef, PassKind, Rate,
    RenderTargetHandle, Renderer, StageDef, StageInputs, Vec2, Vec3,
};

/// Where a composited render-target result is drawn.
pub enum CompositeTarget {
    /// The main swapchain frame (drawn last; injected into the main-frame queue).
    Screen,
    /// Another offscreen target — creates a "render `src` before this target" dependency.
    Target(RenderTargetHandle),
}

/// A rectangle in destination-target pixels (top-left origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

/// The backing frame drawn behind a composited panel — a [`Renderer::draw_ui_panel`]
/// argument bundle — plus `inset`: how far (pixels) the sampled render-target image is
/// inset inside the frame on every side.
#[derive(Clone, Copy)]
pub struct PanelFrame {
    pub fill: [f32; 4],
    pub fill2: [f32; 4],
    pub grad: f32,
    pub radius: f32,
    pub border: f32,
    pub border_color: [f32; 4],
    pub feather: f32,
    pub inset: f32,
}

/// A text label drawn over a composited panel, offset from the panel's top-left.
#[derive(Clone, Copy)]
pub struct Label<'f> {
    pub text: &'f str,
    pub offset: Vec2,
    pub size: f32,
    pub color: [f32; 4],
    pub role: FontRole,
}

/// One recorded composite. Private — built via [`FrameGraph::composite_panel`] /
/// [`FrameGraph::composite_billboard`].
enum Composite<'f> {
    Panel {
        src: RenderTargetHandle,
        into: CompositeTarget,
        rect: Rect,
        layer: f32,
        tint: [f32; 4],
        frame: Option<PanelFrame>,
        label: Option<Label<'f>>,
        base: f32,
    },
    Billboard {
        src: RenderTargetHandle,
        into: CompositeTarget,
        world_position: Vec3,
        world_size: Vec2,
        additive: bool,
        tint: [f32; 4],
        base: f32,
    },
}

impl Composite<'_> {
    fn source(&self) -> RenderTargetHandle {
        match self {
            Composite::Panel { src, .. } | Composite::Billboard { src, .. } => *src,
        }
    }
    fn destination(&self) -> &CompositeTarget {
        match self {
            Composite::Panel { into, .. } | Composite::Billboard { into, .. } => into,
        }
    }
    /// The layer band this composite was declared in — the scene's depth band, recorded
    /// so `execute` can restore it before a screen-bound composite runs.
    fn base(&self) -> f32 {
        match self {
            Composite::Panel { base, .. } | Composite::Billboard { base, .. } => *base,
        }
    }
}

/// ONE draw body: the `FnOnce(&mut Renderer)` closure an offscreen pass takes
/// ([`Renderer::render_to_texture`]), a root element or an overlay aims at the swapchain
/// ([`FrameGraph::root`] / [`FrameGraph::overlay`]), and a recipe carries as its content
/// pass — the same shape in all, boxed so the graph can hold a list of them.
type Draw<'f> = Box<dyn FnOnce(&mut Renderer) + 'f>;

/// One offscreen pass: a target + its clear colour + the sub-scene draw closure, plus the
/// layer band it was declared in and its liveness (how often it re-renders — the recipe-less
/// [`FrameGraph::target`] is always [`Rate::Live`]; a [`FrameGraph::surface`] carries the
/// seat's authored rate). `execute` asks [`Renderer::surface_should_render`] with these before
/// running the pass, so a poster / `hz` surface skips the render while its composite still runs.
struct TargetPass<'f> {
    target: RenderTargetHandle,
    clear: [f64; 4],
    base: f32,
    rate: Rate,
    dirty: bool,
    draw: Draw<'f>,
}

/// A swapchain element — a root or an overlay — paired with the layer band it was
/// declared in (see [`FrameGraph::set_base_layer`]).
struct Element<'f> {
    base: f32,
    draw: Draw<'f>,
}

/// The per-frame render-target draw-order + compositing recorder. See the module docs.
#[derive(Default)]
pub struct FrameGraph<'f> {
    passes: Vec<TargetPass<'f>>,
    composites: Vec<Composite<'f>>,
    /// The screen surface's own elements, drawn straight into the swapchain after every
    /// offscreen pass and before the screen composites (see [`Self::root`]).
    roots: Vec<Element<'f>>,
    /// The screen surface's FINAL 2D — HUD replay + any immediate 2D — drawn after the
    /// screen composites, reproducing today's post-`execute` submission order (see
    /// [`Self::overlay`]).
    overlays: Vec<Element<'f>>,
    /// The layer band the next declared element records — the scene's depth band, stamped
    /// by the manager with [`Self::set_base_layer`] before each scene declares.
    base: f32,
}

/// One step of an executed graph, in the order [`FrameGraph::execute`] runs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Offscreen pass `i` (an index into the declared passes, already dependency-ordered).
    Target(usize),
    /// Root element `i`, in declaration order.
    Root(usize),
    /// Screen-bound composite `i` (an index into the declared composites).
    Screen(usize),
    /// Overlay element `i`, in declaration order — the screen surface's final 2D.
    Overlay(usize),
}

/// THE order of a frame: every offscreen pass (dependency-ordered), then every root
/// element, then every screen composite, then every overlay. The overlay phase runs LAST
/// so a scene's HUD replay lands where its post-`execute` 2D used to — after the
/// composites, not before. Pure, so the contract is testable without a GPU.
fn schedule(
    pass_order: &[usize],
    roots: usize,
    screen_composites: &[usize],
    overlays: usize,
) -> Vec<Step> {
    pass_order
        .iter()
        .map(|&i| Step::Target(i))
        .chain((0..roots).map(Step::Root))
        .chain(screen_composites.iter().map(|&i| Step::Screen(i)))
        .chain((0..overlays).map(Step::Overlay))
        .collect()
}

impl<'f> FrameGraph<'f> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp the layer band the NEXT declared elements record — the scene's depth band
    /// (its stack position × the scene stride). The manager calls this before each scene
    /// declares; `execute` restores the band before running each element, so a deferred
    /// draw lands in the same band `renderer.set_layer` used to put an immediate one.
    pub fn set_base_layer(&mut self, layer: f32) {
        self.base = layer;
    }

    /// The layer band currently stamped — what a scene offsets its own sub-layers from at
    /// declare time (the value that was `renderer.layer()` before draws deferred).
    pub fn base_layer(&self) -> f32 {
        self.base
    }

    /// Declare an offscreen target's self-contained sub-scene. `draw` sets its camera /
    /// scene / geometry exactly as a [`Renderer::render_to_texture`] closure would. The
    /// recipe-less primitive is always [`Rate::Live`] — a caller that wants poster / `hz`
    /// liveness authors a [`Self::surface`] instead, the one place a rate is carried.
    pub fn target(
        &mut self,
        target: RenderTargetHandle,
        clear: [f64; 4],
        draw: impl FnOnce(&mut Renderer) + 'f,
    ) {
        self.passes.push(TargetPass {
            target,
            clear,
            base: self.base,
            rate: Rate::Live,
            dirty: false,
            draw: Box::new(draw),
        });
    }

    /// Declare an element of the ROOT surface — content drawn straight into the swapchain:
    /// a full-window world, a 2D game's play field, the boot widget. `draw` sets its camera /
    /// scene / geometry exactly as a [`Self::target`] closure would; there is no target and
    /// no composite, so a root element costs no blit. It runs after every offscreen pass
    /// (the shared draw queues are never reset under it) and before the screen composites
    /// (nested surfaces land over it). A scene may declare several; they run in order.
    pub fn root(&mut self, draw: impl FnOnce(&mut Renderer) + 'f) {
        self.roots.push(Element {
            base: self.base,
            draw: Box::new(draw),
        });
    }

    /// Declare the screen surface's FINAL 2D — a scene's HUD replay and any immediate 2D
    /// that used to run AFTER the scene's own `execute`. Overlays run in the LAST phase,
    /// after the screen composites, so their submission order matches exactly what drawing
    /// them post-`execute` produced (composites under the HUD, not over it). `draw` uses
    /// the ordinary 2D calls; the band it was declared in is restored before it runs, so a
    /// `render_hud` reads the right [`Renderer::layer`]. Wrapped in a declared pass like a
    /// root. A scene may declare several; they run in order.
    pub fn overlay(&mut self, draw: impl FnOnce(&mut Renderer) + 'f) {
        self.overlays.push(Element {
            base: self.base,
            draw: Box::new(draw),
        });
    }

    /// Declare a surface from its authored STAGE — the recipe entry point (see the module
    /// docs). `into` picks the destination the recipe-less primitives split by hand:
    /// `Target(h)` records an offscreen pass, `Screen` a root element. `content` is the
    /// scene's own drawing — the [`PassKind::Scene`] pass — and runs at the point the
    /// recipe puts it, which for a stage authoring no `passes` is "immediately", exactly
    /// as [`Self::target`]/[`Self::root`] would have run it.
    ///
    /// **`stage.clear` reaches BOTH destinations.** An offscreen pass clears its target
    /// to it (unauthored = [`StageDef::CLEAR_UNAUTHORED`], transparent); a ROOT surface's
    /// clear IS the window's, so it is written to [`Renderer::clear_color`] — the value
    /// `end_frame` clears the swapchain with — and an unauthored one leaves whatever the
    /// app set. A root stage's authored `clear` is never dropped on the floor.
    ///
    /// `inputs` are this frame's published numbers; a bind naming a key nothing published
    /// keeps the authored value (the scene's own gate is what fails loud on that).
    ///
    /// `rate` is the seat's authored liveness (the node's [`SurfaceSlot`](crate::SurfaceSlot)
    /// rate, else the stage's). For an offscreen `Target` it rides into the pass and
    /// [`Renderer::surface_should_render`] decides, once a frame against the per-surface clock,
    /// whether the sub-scene re-renders — a poster skips the render while its composite still
    /// runs. A `Screen` root has no target to keep an image in, so a non-`Live` rate is named
    /// and ignored (rule 4BB12A75); the root renders every frame. `Dirty` reads
    /// [`StageInputs::is_dirty`].
    pub fn surface(
        &mut self,
        into: CompositeTarget,
        stage: &StageDef,
        inputs: StageInputs,
        rate: Rate,
        content: impl FnOnce(&mut Renderer) + 'f,
    ) {
        let (order, cyclic) = stage.pass_order();
        if cyclic {
            let kinds: Vec<&str> = stage.recipe().iter().map(|p| p.kind.kind()).collect();
            tracing::warn!(
                "FrameGraph: pass cycle in the recipe {kinds:?}; falling back to authored order"
            );
        }
        // The recipe is OWNED by the closure: the graph outlives this call, and a
        // per-frame clone of a handful of compiled passes is cheaper than a borrow that
        // would pin the styles for the whole frame.
        let recipe: Vec<PassDef> = order.iter().map(|&i| stage.recipe()[i].clone()).collect();
        // THE per-frame plan, built once here from the already-ordered recipe by the one
        // pure builder the renderer's encoder walks — never re-derived downstream.
        let plan = depth_plan(&recipe.iter().collect::<Vec<_>>());
        // The format the surface's `hdr` attachment DECLARES, handed to the tonemap so the
        // allocation takes the authored word. A stage with no `hdr` attachment never
        // reaches the HDR path (the compiler couples the attachment to the pass), so the
        // fallback is only ever the neutral one.
        let hdr_format = stage
            .attachments
            .get(Attachments::HDR)
            .map(|a| a.format)
            .unwrap_or(AttachmentFormat::Rgba16f);
        // The authored rig, DRIVEN once for this stage this frame: every light with a
        // driver takes its gain from the scene's own clock. A rig with no drivers comes
        // back bit-for-bit identical, which is what makes this inert for every stage
        // that authors none.
        let lighting = stage.lighting.driven(inputs.clock_seconds());
        let camera = stage.camera.map(|c| c.camera());
        let clear = stage.clear;
        // Read the dirty signal before `inputs` moves into the content closure.
        let dirty = inputs.is_dirty();
        let draw = move |r: &mut Renderer| {
            // The stage's rig lands BEFORE the content closure runs, so a scene that owns
            // its own lights can compose over it (`r.scene_lighting()`). NOTE: a content
            // closure that calls `set_scene` REPLACES this rig rather than adding to it —
            // the rig is one value, and the last writer wins.
            r.set_scene(lighting);
            if let Some(cam) = camera {
                r.set_camera(&cam);
            }
            r.set_depth_plan(plan);
            let mut content: Option<Draw<'f>> = Some(Box::new(content));
            for pass in &recipe {
                apply_pass(r, pass, hdr_format, &inputs, &mut content);
            }
        };
        match into {
            // An offscreen pass carrying the seat's rate — the clock skips its render when
            // the poster / `hz` rule says so, while the composite (declared separately) runs.
            CompositeTarget::Target(target) => self.passes.push(TargetPass {
                target,
                clear: clear.unwrap_or(StageDef::CLEAR_UNAUTHORED),
                base: self.base,
                rate,
                dirty,
                draw: Box::new(draw),
            }),
            // The screen has no target to clear — `end_frame` clears it from
            // `Renderer::clear_color`, so THAT is where a root stage's authored clear
            // lands. Unauthored leaves the app's own colour standing. And the screen keeps
            // no image, so a non-live rate cannot poster it — name it rather than silently
            // ignore (rule 4BB12A75); the root renders every frame.
            CompositeTarget::Screen => {
                if rate != Rate::Live {
                    tracing::warn!(
                        "FrameGraph::surface: a root surface cannot poster — the screen keeps \
                         no image; rate {rate:?} ignored, the root renders every frame"
                    );
                }
                self.root(move |r: &mut Renderer| {
                    if let Some(c) = clear {
                        r.clear_color = c;
                    }
                    draw(r);
                })
            }
        }
    }

    /// Composite `src`'s result into `into` as a 2D panel element at `rect`/`layer`,
    /// tinted by `tint`, with an optional backing `frame` and `label`. If `into` is
    /// another target this records a "render `src` first" dependency.
    #[allow(clippy::too_many_arguments)]
    pub fn composite_panel(
        &mut self,
        src: RenderTargetHandle,
        into: CompositeTarget,
        rect: Rect,
        layer: f32,
        tint: [f32; 4],
        frame: Option<PanelFrame>,
        label: Option<Label<'f>>,
    ) {
        self.composites.push(Composite::Panel {
            src,
            into,
            rect,
            layer,
            tint,
            frame,
            label,
            base: self.base,
        });
    }

    /// Composite `src`'s result as a camera-facing world-space billboard in `into` — an
    /// RTT-as-billboard (needs no pipeline change; the target's colour texture already
    /// carries a billboard bind group).
    pub fn composite_billboard(
        &mut self,
        src: RenderTargetHandle,
        into: CompositeTarget,
        world_position: Vec3,
        world_size: Vec2,
        additive: bool,
        tint: [f32; 4],
    ) {
        self.composites.push(Composite::Billboard {
            src,
            into,
            world_position,
            world_size,
            additive,
            tint,
            base: self.base,
        });
    }

    /// Run the frame's four phases in order: every offscreen pass in dependency order
    /// (injecting the composites bound INTO each target right after its sub-scene), then
    /// every root element, then the screen-bound composites, then the overlays — the
    /// screen surface's final 2D. Called EXACTLY ONCE per frame, by the scene manager,
    /// after every visible scene has declared into this one graph. Before each step the
    /// band it was declared in is restored (see [`Self::set_base_layer`]), so a deferred
    /// draw lands where `renderer.set_layer(band)` used to put an immediate one; the
    /// renderer's layer is left as it was found afterward.
    pub fn execute(self, r: &mut Renderer) {
        let FrameGraph {
            passes,
            composites,
            roots,
            overlays,
            base: _,
        } = self;
        let restore_layer = r.layer();

        // Order the offscreen passes: an edge (dst, src) means "target dst composites
        // src, so src renders first". Screen destinations are the implicit final sink
        // and impose no ordering on the offscreen passes.
        let target_ids: Vec<u32> = passes.iter().map(|p| p.target.0).collect();
        let deps: Vec<(u32, u32)> = composites
            .iter()
            .filter_map(|c| match c.destination() {
                CompositeTarget::Target(dst) => Some((dst.0, c.source().0)),
                CompositeTarget::Screen => None,
            })
            .collect();
        let (order, cyclic) = topo_order(&target_ids, &deps);
        if cyclic {
            tracing::warn!(
                "FrameGraph: render-target composite cycle among {target_ids:?}; \
                 falling back to declaration order"
            );
        }

        let screen_composites: Vec<usize> = composites
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.destination(), CompositeTarget::Screen))
            .map(|(i, _)| i)
            .collect();

        // The band each element was declared in, in the index space `step_base` uses —
        // collected before the pass/root/overlay vectors are consumed into option slots.
        let pass_bases: Vec<f32> = passes.iter().map(|p| p.base).collect();
        let root_bases: Vec<f32> = roots.iter().map(|e| e.base).collect();
        let overlay_bases: Vec<f32> = overlays.iter().map(|e| e.base).collect();
        let composite_bases: Vec<f32> = composites.iter().map(|c| c.base()).collect();

        let mut passes: Vec<Option<TargetPass>> = passes.into_iter().map(Some).collect();
        let mut roots: Vec<Option<Element>> = roots.into_iter().map(Some).collect();
        let mut overlays: Vec<Option<Element>> = overlays.into_iter().map(Some).collect();
        for step in schedule(&order, roots.len(), &screen_composites, overlays.len()) {
            // Restore the band this step was declared in before it runs (an offscreen pass
            // resets the layer inside its sub-frame anyway; a root/overlay/composite needs
            // it so its 2D — a `render_hud` reading `renderer.layer()` — lands in its band).
            r.set_layer(step_base(
                step,
                &pass_bases,
                &root_bases,
                &composite_bases,
                &overlay_bases,
            ));
            match step {
                // An offscreen pass; inject the composites landing IN this target right
                // after its own draws (RTT samples another RTT).
                Step::Target(i) => {
                    let Some(TargetPass {
                        target,
                        clear,
                        rate,
                        dirty,
                        draw,
                        ..
                    }) = passes[i].take()
                    else {
                        continue;
                    };
                    // The per-surface clock decides whether this surface re-renders this
                    // frame; a poster (or off-period `hz`) surface keeps its last image and
                    // its composite — a separate step — still runs. `target` is always Live.
                    if !r.surface_should_render(target, rate, dirty) {
                        continue;
                    }
                    let composites = &composites;
                    r.render_to_texture(target, clear, move |r| {
                        draw(r);
                        for c in composites {
                            if matches!(c.destination(), CompositeTarget::Target(dst) if *dst == target)
                            {
                                emit_composite(r, c);
                            }
                        }
                    });
                }
                // A root element: straight into the main-frame queues, inside a declared
                // pass so the renderer's stray-3D gate knows it was meant.
                Step::Root(i) => {
                    let Some(Element { draw, .. }) = roots[i].take() else {
                        continue;
                    };
                    r.begin_pass();
                    draw(r);
                    r.end_pass();
                }
                // A screen-bound composite, injected into the main-frame queue inside a
                // declared pass — its panel/sprite/label draws are graph work, not the
                // immediate-mode strays the renderer's declared-surface gate names.
                Step::Screen(i) => {
                    r.begin_pass();
                    emit_composite(r, &composites[i]);
                    r.end_pass();
                }
                // An overlay: the screen surface's final 2D — HUD replay + immediate 2D —
                // inside a declared pass like a root, run after the composites so its
                // submission order matches the post-`execute` 2D it replaces.
                Step::Overlay(i) => {
                    let Some(Element { draw, .. }) = overlays[i].take() else {
                        continue;
                    };
                    r.begin_pass();
                    draw(r);
                    r.end_pass();
                }
            }
        }

        r.set_layer(restore_layer);
    }
}

/// The layer band a scheduled [`Step`] runs at — looked up from the per-element bands in
/// the same index space [`schedule`] walks (a `Target`/`Screen` index addresses the pass /
/// composite it names; a `Root`/`Overlay` index is its position in declaration order).
/// Pure, so "each step runs in the band it was declared in" is testable without a GPU.
fn step_base(
    step: Step,
    pass_bases: &[f32],
    root_bases: &[f32],
    composite_bases: &[f32],
    overlay_bases: &[f32],
) -> f32 {
    match step {
        Step::Target(i) => pass_bases[i],
        Step::Root(i) => root_bases[i],
        Step::Screen(i) => composite_bases[i],
        Step::Overlay(i) => overlay_bases[i],
    }
}

/// Apply ONE pass of a recipe to the renderer — "the pass trait", realized as the single
/// place a [`PassKind`] becomes renderer calls. The content closure is taken (an `FnOnce`
/// runs once); a recipe that names two `scene` passes therefore draws the content once
/// and the second is a no-op, which is the only sane reading of "draw this twice" without
/// re-recording the scene.
///
/// `Composite` is a no-op here: a composite between SURFACES is already recorded by
/// [`FrameGraph::composite_panel`] and injected by [`FrameGraph::execute`], so the pass
/// exists in the recipe as the ordering edge, not as a second blit.
fn apply_pass(
    r: &mut Renderer,
    pass: &PassDef,
    hdr_format: AttachmentFormat,
    inputs: &StageInputs,
    content: &mut Option<Draw<'_>>,
) {
    match &pass.kind {
        PassKind::Scene => {
            if let Some(draw) = content.take() {
                draw(r);
            }
        }
        PassKind::Sky => r.draw_sky(),
        PassKind::VolumetricDisk(v) => r.set_volumetric_disk(v.resolve(inputs)),
        PassKind::GroundFog(f) => {
            // An unauthored fog colour follows the renderer's LIVE atmosphere, so a fog
            // under a day/night cycle tracks it without the scene touching the recipe.
            let live = r.scene_lighting().fog_color;
            r.set_ground_fog(f.resolve(inputs, live));
        }
        // Raise the frame's HDR flag + grade; `encode_passes` then routes the lit passes
        // into the surface's HDR attachment and resolves it back to the sRGB colour last.
        // The strength/exposure RESOLVE against this frame's inputs first (like the fog and
        // the water), so a day/night cycle can drive the grade — a golden-hour warmth the
        // scene publishes — without the scene reaching into the recipe.
        PassKind::TonemapGrade(t) => {
            let (grade, grade_strength, exposure) = t.resolve(inputs);
            r.set_tonemap_grade(grade, grade_strength, exposure, hdr_format)
        }
        PassKind::Composite(_) => {}
        // A no-op here for BOTH roles, exactly like `Composite`: the runtime values a
        // shadow needs — the light-view-projection matrix, and the producer target the
        // consumer samples — are per-frame simulation output, not authored fields. The
        // scene wires them (`begin_shadow_view` on the producer's caster closure,
        // `set_shadow_source` on the consumer's), and this pass stands in the recipe as
        // the ordering marker + the fail-loud gate anchor, never as a second binding.
        PassKind::ShadowMap(_) => {}
        // The authored water params (incl. the wave roster) resolve here, like the fog; the
        // renderer builds the grid mesh lazily and draws it in the opaque pass. The animated
        // water is real geometry with a sun specular — no reflection source to wire.
        PassKind::WaterSurface(w) => r.set_water(w.resolve(inputs)),
        // Raise the frame's bloom params (pure art knobs, no binds); `encode_passes` then runs
        // the bright/blur/composite chain in the HDR path, after the lit passes wrote `hdr` and
        // before the tonemap resolves it — the slot its reads/writes derive.
        PassKind::Bloom(b) => r.set_bloom(b.threshold, b.knee, b.intensity, b.radius),
    }
}

/// Draw one composite into the renderer's current target, reusing the ordinary 2D /
/// billboard draw calls. A panel emits `draw_ui_panel → draw_sprite → draw_text` at a
/// single layer, so frame → image → label stack correctly under the existing
/// intra-layer paint order (ui → sprite → text) with no per-caller bookkeeping.
fn emit_composite(r: &mut Renderer, c: &Composite) {
    match c {
        Composite::Panel {
            src,
            rect,
            layer,
            tint,
            frame,
            label,
            ..
        } => {
            let Some(tex) = r.target_texture(*src) else {
                return; // target freed / never rendered — skip gracefully
            };
            r.set_layer(*layer);
            let inset = match frame {
                Some(f) => {
                    r.draw_ui_panel(
                        rect.pos,
                        rect.size,
                        f.fill,
                        f.fill2,
                        f.grad,
                        f.radius,
                        f.border,
                        f.border_color,
                        f.feather,
                    );
                    f.inset
                }
                None => 0.0,
            };
            r.draw_sprite(
                tex,
                rect.pos + Vec2::splat(inset),
                rect.size - Vec2::splat(inset * 2.0),
                *tint,
            );
            if let Some(l) = label {
                r.draw_text_role(
                    l.text,
                    rect.pos + l.offset,
                    l.size,
                    l.color,
                    l.role,
                    false,
                    false,
                    -1.0,
                    None,
                );
            }
        }
        Composite::Billboard {
            src,
            world_position,
            world_size,
            additive,
            tint,
            ..
        } => {
            let Some(tex) = r.target_texture(*src) else {
                return;
            };
            if *additive {
                r.draw_billboard_additive(
                    tex,
                    *world_position,
                    *world_size,
                    Vec2::ZERO,
                    Vec2::ONE,
                    *tint,
                );
            } else {
                r.draw_billboard(
                    tex,
                    *world_position,
                    *world_size,
                    Vec2::ZERO,
                    Vec2::ONE,
                    *tint,
                );
            }
        }
    }
}

/// Order the passes (by index into `targets`) so every target renders before any pass
/// that composites it. `deps` are `(dst, src)` handle-id pairs — "the pass targeting
/// `dst` composites `src`". Edges whose endpoint is not a declared pass this frame
/// (e.g. a persistent, already-rendered target) impose no constraint.
///
/// Kahn's algorithm seeded in **declaration order** → deterministic. Returns
/// `(order, cyclic)`; a cycle (physically impossible for real RTT — a target cannot
/// sample itself mid-render) is broken by appending the residual passes in declaration
/// order, with `cyclic = true` so the caller can warn. Pure — unit-tested below.
pub(crate) fn topo_order(targets: &[u32], deps: &[(u32, u32)]) -> (Vec<usize>, bool) {
    let n = targets.len();
    let index_of = |handle: u32| targets.iter().position(|&t| t == handle);

    let mut in_degree = vec![0usize; n];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(dst, src) in deps {
        if let (Some(di), Some(si)) = (index_of(dst), index_of(src)) {
            if di != si {
                successors[si].push(di);
                in_degree[di] += 1;
            }
        }
    }

    let mut order = Vec::with_capacity(n);
    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut head = 0;
    while head < queue.len() {
        let node = queue[head];
        head += 1;
        order.push(node);
        for &succ in &successors[node] {
            in_degree[succ] -= 1;
            if in_degree[succ] == 0 {
                queue.push(succ);
            }
        }
    }

    let cyclic = order.len() < n;
    if cyclic {
        // Deterministic fallback: append the passes still tangled in the cycle.
        for i in 0..n {
            if !order.contains(&i) {
                order.push(i);
            }
        }
    }
    (order, cyclic)
}

#[cfg(test)]
mod tests {
    use super::{schedule, step_base, topo_order, Step};

    #[test]
    fn root_runs_after_every_offscreen_pass_and_before_screen_composites() {
        // Three targets (already dependency-ordered 2,0,1), two root elements, two
        // screen composites, no overlays: the root can never be reset by a later
        // offscreen pass, and nested surfaces composite over it.
        let steps = schedule(&[2, 0, 1], 2, &[0, 3], 0);
        assert_eq!(
            steps,
            vec![
                Step::Target(2),
                Step::Target(0),
                Step::Target(1),
                Step::Root(0),
                Step::Root(1),
                Step::Screen(0),
                Step::Screen(3),
            ]
        );
    }

    #[test]
    fn overlay_runs_after_every_screen_composite() {
        // Two targets, one root, two screen composites, two overlays: the overlays (a
        // scene's HUD replay + immediate 2D) run in the LAST phase, after every screen
        // composite — the order-preservation proof that the HUD lands where the 2D drawn
        // after a per-scene `execute` used to, OVER the composites rather than under them.
        let steps = schedule(&[0, 1], 1, &[0, 1], 2);
        assert_eq!(
            steps,
            vec![
                Step::Target(0),
                Step::Target(1),
                Step::Root(0),
                Step::Screen(0),
                Step::Screen(1),
                Step::Overlay(0),
                Step::Overlay(1),
            ]
        );
        let last_screen = steps
            .iter()
            .rposition(|s| matches!(s, Step::Screen(_)))
            .unwrap();
        let first_overlay = steps
            .iter()
            .position(|s| matches!(s, Step::Overlay(_)))
            .unwrap();
        assert!(
            first_overlay > last_screen,
            "every overlay follows every composite"
        );
    }

    #[test]
    fn the_base_layer_of_each_step_is_the_band_it_was_declared_in() {
        // Two scenes declaring into ONE graph — scene 0 in band 0.0, scene 1 in band
        // 100.0 — each contributing a target, a root, a screen composite, and an overlay.
        // `execute` restores each step's declared band before running it, so a deferred
        // draw lands where `set_layer(band)` used to put an immediate one.
        let pass_bases = [0.0, 100.0];
        let root_bases = [0.0, 100.0];
        let composite_bases = [0.0, 100.0];
        let overlay_bases = [0.0, 100.0];
        let bands: Vec<f32> = schedule(&[0, 1], 2, &[0, 1], 2)
            .into_iter()
            .map(|s| {
                step_base(
                    s,
                    &pass_bases,
                    &root_bases,
                    &composite_bases,
                    &overlay_bases,
                )
            })
            .collect();
        // Target(0),Target(1), Root(0),Root(1), Screen(0),Screen(1), Overlay(0),Overlay(1)
        // → each element's own band, whichever phase it lands in.
        assert_eq!(bands, vec![0.0, 100.0, 0.0, 100.0, 0.0, 100.0, 0.0, 100.0]);
    }

    #[test]
    fn a_graph_with_only_a_root_is_just_the_root() {
        assert_eq!(schedule(&[], 1, &[], 0), vec![Step::Root(0)]);
        assert!(schedule(&[], 0, &[], 0).is_empty());
    }

    #[test]
    fn no_deps_keeps_declaration_order() {
        let (order, cyclic) = topo_order(&[10, 11, 12], &[]);
        assert_eq!(order, vec![0, 1, 2]);
        assert!(!cyclic);
    }

    #[test]
    fn chain_renders_source_before_consumer() {
        // C composites B, B composites A → A must render, then B, then C.
        let targets = [/*A*/ 1, /*B*/ 2, /*C*/ 3];
        let deps = [(3, 2), (2, 1)]; // (dst, src)
        let (order, cyclic) = topo_order(&targets, &deps);
        assert_eq!(order, vec![0, 1, 2], "A before B before C");
        assert!(!cyclic);
    }

    #[test]
    fn diamond_renders_root_first_and_sink_last() {
        // D composites B and C; B and C each composite A.
        let targets = [/*A*/ 1, /*B*/ 2, /*C*/ 3, /*D*/ 4];
        let deps = [(4, 2), (4, 3), (2, 1), (3, 1)];
        let (order, cyclic) = topo_order(&targets, &deps);
        assert!(!cyclic);
        let pos = |h: u32| order.iter().position(|&i| targets[i] == h).unwrap();
        assert!(pos(1) < pos(2) && pos(1) < pos(3), "A before B and C");
        assert!(pos(2) < pos(4) && pos(3) < pos(4), "B and C before D");
    }

    #[test]
    fn edge_to_undeclared_source_imposes_no_constraint() {
        // Target 2 composites a persistent target 99 that isn't a declared pass —
        // ordering is unconstrained, declaration order stands, no false cycle.
        let (order, cyclic) = topo_order(&[1, 2], &[(2, 99)]);
        assert_eq!(order, vec![0, 1]);
        assert!(!cyclic);
    }

    #[test]
    fn cycle_falls_back_without_panic() {
        // A composites B and B composites A — impossible for real RTT. Must not hang or
        // panic; returns all passes in declaration order flagged cyclic.
        let (order, cyclic) = topo_order(&[1, 2], &[(1, 2), (2, 1)]);
        assert!(cyclic);
        assert_eq!(order.len(), 2);
        assert!(order.contains(&0) && order.contains(&1));
    }
}
