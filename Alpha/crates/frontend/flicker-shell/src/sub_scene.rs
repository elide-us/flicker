//! SUB SCENES — a nested `surface`'s scene IS a complete scene.
//!
//! Aaron (2026-09-09): *"The whole purpose of a surface is to be a complete unit … surfaces
//! are complete scene rendering objects. In the case of a scene that contains a scene, the
//! sub scene is managed by the root scene, and inherits context from intent."* A key panel,
//! a lock-picking puzzle, a 2D game on a terminal in a 3D world, the four panels of a model
//! view — all one mechanism.
//!
//! [`SubScene`] is that mechanism's host side. The parent scene's walker reserves a `surface`
//! node exactly as it always has (a [`SurfaceSlot`] with a rect, a layer, a tint, a rate);
//! the parent SEATS a sub scene in that slot, and each frame:
//!
//! * **update** — the child sees the frame's input addressed at ITS surface: the pointer
//!   sample the walker's barrier handed the slot ([`SurfacePointer`], local coordinates),
//!   the slot's size as its whole screen, and the discrete signals only while the slot is
//!   the FOCUSED pane (contract A8C9F02B §4d: nested surfaces require focus — that is the
//!   "context from intent"). Inside, the child runs its own walker: its UI claims first,
//!   its root surface takes the rest, exactly as a top-level scene.
//! * **render** — the child declares into the shared frame graph as if it were on top of
//!   the stack; [`FrameGraph::sub_scene`] lands everything it says about "the screen" in this
//!   surface's render target, and the host composites that target where the slot was seated.
//!   The child's chrome is INSIDE the texture: no layer fight with the composite (the
//!   2D encoder's panels-under-sprites order is what made overlay chrome on a parent
//!   vanish — incident 09E5A30F).
//! * **enter / exit** ride the host's: a scene seated for the first time enters on its
//!   first render (that is where `&mut Renderer` is), and the host frees its target and
//!   exits it when the host exits.
//!
//! The child is any [`Scene`] — a concrete type the host constructs (so the host keeps
//! TYPED access to it through [`SubScene::scene_mut`]: the model view's document handles,
//! draw items and picks flow through that seam) or a roster-resolved `Box<dyn Scene>` (a
//! gallery card). The child never learns it is a sub scene.

use std::time::Duration;

use flicker::input_core::{InputContext, InputState};
use flicker::render::{
    CompositeTarget, FrameGraph, Rate, Rect, RenderTargetHandle, Renderer, Vec2,
};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::ui::{SurfacePointer, SurfaceSlot};

/// Where the walker seated a sub scene this frame — the slot's image rect and the
/// composite facts (layer, tint, liveness) the host replays when it composites.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seat {
    rect: Rect,
    layer: f32,
    tint: [f32; 4],
    rate: Rate,
}

impl From<&SurfaceSlot> for Seat {
    fn from(s: &SurfaceSlot) -> Self {
        Seat {
            rect: Rect {
                pos: Vec2::new(s.x, s.y),
                size: Vec2::new(s.w, s.h),
            },
            layer: s.layer,
            tint: s.tint,
            rate: s.rate,
        }
    }
}

/// A sub scene — a scene played inside one of another scene's surfaces. See the module docs.
pub struct SubScene<S: Scene> {
    scene: S,
    seat: Option<Seat>,
    /// The surface's render target — created on first render, resized to the seat, freed
    /// on [`exit`](Self::exit).
    target: Option<RenderTargetHandle>,
    size: (u32, u32),
    entered: bool,
}

impl<S: Scene> SubScene<S> {
    /// Wrap `scene` for hosting. It is not entered until its first seated render.
    pub fn new(scene: S) -> Self {
        Self {
            scene,
            seat: None,
            target: None,
            size: (0, 0),
            entered: false,
        }
    }

    /// The sub scene — the host's TYPED channel into it (context in, results out).
    pub fn scene(&self) -> &S {
        &self.scene
    }

    /// See [`scene`](Self::scene).
    pub fn scene_mut(&mut self) -> &mut S {
        &mut self.scene
    }

    /// Seat the scene in the slot the walker reserved this frame, or unseat it (`None`
    /// — the surface is off screen this frame: no update, no render, no cost).
    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.seat = slot.map(Seat::from);
    }

    /// Whether the scene has a seat this frame.
    pub fn seated(&self) -> bool {
        self.seat.is_some()
    }

    /// The seated rect in the HOST's pixels, if seated.
    pub fn rect(&self) -> Option<Rect> {
        self.seat.map(|s| s.rect)
    }

    /// Advance the sub scene one frame, addressing the input at its surface.
    ///
    /// `pointer` is the walker's sample for the slot (`frame.surface_pointer(id)`) —
    /// `None` while the cursor is elsewhere or a UI node over the surface claimed it.
    /// `focused` says whether the slot is the focused pane: the discrete signals reach the
    /// child only then. The child's [`Transition`] comes back to the host, which decides
    /// what a sub scene may do to the stack (a bench ignores it; a gallery might
    /// honour a `Quit` as "close this game").
    ///
    /// Not yet entered (never rendered) or unseated → no update, [`Transition::None`].
    pub fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        pointer: Option<&SurfacePointer>,
        signals: &mut SceneInput,
        focused: bool,
        renderer: &Renderer,
    ) -> Transition {
        let Some(seat) = self.seat else {
            return Transition::None;
        };
        if !self.entered {
            return Transition::None;
        }
        let local = local_input(input, pointer);
        let mut view = signals.for_surface(seat.rect.size, focused);
        self.scene.update(dt, &local, &mut view, renderer)
    }

    /// The sub scene's input context, while its surface is focused — a sub-scene text
    /// field owns the keyboard exactly as a top-level one would. The host folds this into
    /// its own [`Scene::input_context`].
    pub fn input_context(&self, focused: bool) -> Option<InputContext> {
        if focused {
            self.scene.input_context()
        } else {
            None
        }
    }

    /// Declare the sub scene into the frame's graph, inside this surface's target, and
    /// composite the target where the slot was seated (at `base_layer` + the slot's layer,
    /// with the slot's tint). Enters the scene on its first seated render. Unseated →
    /// declares nothing.
    pub fn render<'f>(
        &'f mut self,
        renderer: &mut Renderer,
        fg: &mut FrameGraph<'f>,
        base_layer: f32,
    ) {
        let Some(seat) = self.seat else {
            return;
        };
        let SubScene {
            scene,
            target,
            size,
            entered,
            ..
        } = self;
        if !*entered {
            scene.enter(renderer);
            *entered = true;
        }
        let want = (
            seat.rect.size.x.round().max(1.0) as u32,
            seat.rect.size.y.round().max(1.0) as u32,
        );
        let handle = match *target {
            Some(t) if *size == want => t,
            Some(t) => {
                renderer.resize_render_target(t, want.0, want.1);
                *size = want;
                t
            }
            None => {
                let t = renderer.create_render_target(want.0, want.1);
                *target = Some(t);
                *size = want;
                t
            }
        };
        fg.sub_scene(handle, seat.rate, |fg| scene.render(renderer, fg));
        fg.composite_panel(
            handle,
            CompositeTarget::Screen,
            seat.rect,
            base_layer + seat.layer,
            seat.tint,
            None,
            None,
        );
    }

    /// Exit the sub scene (if it was ever entered) and give its target back. A host
    /// calls this from its own `exit` — a render target is an index into the renderer's
    /// pool, so dropping the wrapper reclaims nothing (rule 728E682F).
    pub fn exit(&mut self, renderer: &mut Renderer) {
        if self.entered {
            self.scene.exit(renderer);
            self.entered = false;
        }
        if let Some(t) = self.target.take() {
            renderer.free_render_target(t);
            self.size = (0, 0);
        }
        self.seat = None;
    }
}

/// The frame's device state addressed at ONE surface: the pointer fields rewritten from
/// the walker's sample (local coordinates; buttons/wheel/motion as the barrier handed
/// them), everything else — keyboard, gamepads, edges — shared as-is. No sample means
/// the cursor is not on this surface: it reads as parked off-screen with nothing held,
/// so a sub-scene walker hovers nothing and a sub-scene camera sees no drag.
fn local_input(input: &InputState, pointer: Option<&SurfacePointer>) -> InputState {
    let mut local = input.clone();
    match pointer {
        Some(p) => {
            local.mouse_position = p.local;
            local.mouse_left = p.left;
            local.mouse_right = p.right;
            local.mouse_left_pressed = p.pressed && p.left;
            local.mouse_wheel_delta = p.wheel;
            local.mouse_delta = p.delta;
        }
        None => {
            local.mouse_position = Vec2::new(-1.0, -1.0);
            local.mouse_left = false;
            local.mouse_right = false;
            local.mouse_left_pressed = false;
            local.mouse_wheel_delta = 0.0;
            local.mouse_delta = Vec2::ZERO;
        }
    }
    local
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(local: Vec2, left: bool, pressed: bool) -> SurfacePointer {
        SurfacePointer {
            id: "panel".into(),
            root: false,
            cursor: local + Vec2::new(100.0, 50.0),
            local,
            delta: Vec2::new(3.0, -2.0),
            left,
            right: false,
            pressed,
            wheel: 1.5,
            captured: left,
            rect: Rect {
                pos: Vec2::new(100.0, 50.0),
                size: Vec2::new(200.0, 120.0),
            },
        }
    }

    #[test]
    fn a_sample_addresses_the_pointer_at_the_surface_in_local_coordinates() {
        let mut input = InputState::new();
        input.mouse_position = Vec2::new(140.0, 80.0);
        input.mouse_left = true;
        input.mouse_left_pressed = true;
        let local = local_input(&input, Some(&sample(Vec2::new(40.0, 30.0), true, true)));
        assert_eq!(local.mouse_position, Vec2::new(40.0, 30.0));
        assert!(local.mouse_left && local.mouse_left_pressed);
        assert_eq!(local.mouse_wheel_delta, 1.5);
        assert_eq!(local.mouse_delta, Vec2::new(3.0, -2.0));
    }

    #[test]
    fn a_press_edge_is_only_a_press_when_the_left_button_landed_it() {
        let input = InputState::new();
        // The barrier reports `pressed` for EITHER button's edge; a sub-scene walker's
        // `clicked` is the LEFT one.
        let right_only = SurfacePointer {
            right: true,
            ..sample(Vec2::ZERO, false, true)
        };
        assert!(!local_input(&input, Some(&right_only)).mouse_left_pressed);
        assert!(local_input(&input, Some(&right_only)).mouse_right);
    }

    #[test]
    fn no_sample_parks_the_pointer_off_the_surface_with_nothing_held() {
        let mut input = InputState::new();
        input.mouse_position = Vec2::new(140.0, 80.0);
        input.mouse_left = true;
        input.mouse_wheel_delta = 2.0;
        let local = local_input(&input, None);
        assert_eq!(local.mouse_position, Vec2::new(-1.0, -1.0));
        assert!(!local.mouse_left && !local.mouse_left_pressed);
        assert_eq!(local.mouse_wheel_delta, 0.0);
    }

    #[test]
    fn a_seat_comes_from_the_slot_and_leaves_with_it() {
        struct Blank;
        impl Scene for Blank {
            fn update(
                &mut self,
                _: Duration,
                _: &InputState,
                _: &mut SceneInput,
                _: &Renderer,
            ) -> Transition {
                Transition::None
            }
            fn render<'f>(&'f mut self, _: &mut Renderer, _: &mut FrameGraph<'f>) {}
        }
        let mut sub = SubScene::new(Blank);
        assert!(!sub.seated() && sub.rect().is_none());
        let slot = SurfaceSlot {
            id: "panel".into(),
            source: String::new(),
            scene: "demo".into(),
            params: Default::default(),
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 200.0,
            layer: 2.0,
            rate: Rate::Live,
            tint: [1.0; 4],
            layout: flicker::render::ViewportLayout::Single,
        };
        sub.seat(Some(&slot));
        assert_eq!(
            sub.rect(),
            Some(Rect {
                pos: Vec2::new(10.0, 20.0),
                size: Vec2::new(300.0, 200.0),
            })
        );
        sub.seat(None);
        assert!(!sub.seated());
    }
}
