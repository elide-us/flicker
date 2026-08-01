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
//! **Contract:** call [`FrameGraph::execute`] once, first thing in `render()`, before
//! queuing any main-frame draws — the offscreen passes reset the shared per-frame
//! draw queues, so a later `execute` would drop main-frame geometry. (This is the one
//! centralized version of the old scattered "render RTTs before the main view" rule.)
//! A target's draw closure must not request a volumetric disk (offscreen volumetrics
//! sample the main depth buffer and are unsupported — same limit as `render_to_texture`).

use crate::{FontRole, RenderTargetHandle, Renderer, Vec2, Vec3};

/// Where a composited render-target result is drawn.
pub enum CompositeTarget {
    /// The main swapchain frame (drawn last; injected into the main-frame queue).
    Screen,
    /// Another offscreen target — creates a "render `src` before this target" dependency.
    Target(RenderTargetHandle),
}

/// A rectangle in destination-target pixels (top-left origin).
#[derive(Clone, Copy)]
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
    },
    Billboard {
        src: RenderTargetHandle,
        into: CompositeTarget,
        world_position: Vec3,
        world_size: Vec2,
        additive: bool,
        tint: [f32; 4],
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
}

/// One offscreen pass: a target + its clear colour + the sub-scene draw closure (the
/// same `FnOnce(&mut Renderer)` body [`Renderer::render_to_texture`] takes).
struct TargetPass<'f> {
    target: RenderTargetHandle,
    clear: [f64; 4],
    draw: Box<dyn FnOnce(&mut Renderer) + 'f>,
}

/// The per-frame render-target draw-order + compositing recorder. See the module docs.
#[derive(Default)]
pub struct FrameGraph<'f> {
    passes: Vec<TargetPass<'f>>,
    composites: Vec<Composite<'f>>,
}

impl<'f> FrameGraph<'f> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare an offscreen target's self-contained sub-scene. `draw` sets its camera /
    /// scene / geometry exactly as a [`Renderer::render_to_texture`] closure would.
    pub fn target(
        &mut self,
        target: RenderTargetHandle,
        clear: [f64; 4],
        draw: impl FnOnce(&mut Renderer) + 'f,
    ) {
        self.passes.push(TargetPass {
            target,
            clear,
            draw: Box::new(draw),
        });
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
        });
    }

    /// Render every offscreen pass in dependency order — injecting the composites bound
    /// INTO each target right after its sub-scene — then inject the screen-bound
    /// composites into the main-frame queue (rendered last by `end_frame`). Layer state
    /// is left as it was found, so the app's subsequent main-frame draws are unaffected.
    pub fn execute(self, r: &mut Renderer) {
        let FrameGraph { passes, composites } = self;
        let base_layer = r.layer();

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

        // Run each offscreen pass; inject the composites landing IN this target right
        // after its own draws (RTT samples another RTT).
        let mut passes: Vec<Option<TargetPass>> = passes.into_iter().map(Some).collect();
        for i in order {
            let Some(TargetPass {
                target,
                clear,
                draw,
            }) = passes[i].take()
            else {
                continue;
            };
            let composites = &composites;
            r.render_to_texture(target, clear, move |r| {
                draw(r);
                for c in composites {
                    if matches!(c.destination(), CompositeTarget::Target(dst) if *dst == target) {
                        emit_composite(r, c);
                    }
                }
            });
        }

        // Screen-bound composites, injected into the main-frame queue.
        for c in &composites {
            if matches!(c.destination(), CompositeTarget::Screen) {
                emit_composite(r, c);
            }
        }

        r.set_layer(base_layer);
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
                r.draw_billboard_additive(tex, *world_position, *world_size, Vec2::ZERO, Vec2::ONE, *tint);
            } else {
                r.draw_billboard(tex, *world_position, *world_size, Vec2::ZERO, Vec2::ONE, *tint);
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
fn topo_order(targets: &[u32], deps: &[(u32, u32)]) -> (Vec<usize>, bool) {
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
    use super::topo_order;

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
