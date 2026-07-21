//! flicker-scene: a small stack-based scene manager that drives the engine's
//! top-level flow — logo → menu → loading → game — with modal overlays such as
//! a pause menu.
//!
//! # Model
//!
//! The manager owns a **stack** of [`Scene`]s and itself implements
//! [`flicker_app::App`], so a game runs one with
//! `flicker_app::run(SceneManager::new(initial))` — no other wiring.
//!
//! Each frame:
//! * **update** routes to the **top** scene only; everything beneath is frozen
//!   (this is what makes a pushed pause menu actually pause the game). The top
//!   scene returns a [`Transition`] to reshape the stack.
//! * **render** draws the **visible slice**: the topmost opaque scene plus any
//!   [overlay](Scene::is_overlay) scenes above it, bottom-up — so an overlay
//!   (e.g. a pause modal) draws over the still-visible game beneath it.
//!
//! Structural changes ([`Push`](Transition::Push) / [`Pop`](Transition::Pop) /
//! [`Replace`](Transition::Replace)) are applied at the top of `render`,
//! because [`Scene::enter`] / [`Scene::exit`] need `&mut Renderer` (to upload /
//! free GPU resources) while `update` only borrows it immutably.
//!
//! # Transitions
//!
//! * [`Transition::Replace`] — swap the top scene (logo → menu → loading →
//!   game): exit the old, enter the new.
//! * [`Transition::ReplaceRoot`] — unwind the *whole* stack (exiting each,
//!   top-down) and start over at a new scene (pause → main menu). `Replace`
//!   only swaps the top, which would orphan the scenes beneath it.
//! * [`Transition::Push`] — overlay a scene, freezing the one below (game →
//!   pause): enter the new; the one below is untouched.
//! * [`Transition::Pop`] — remove the top, revealing the one below (pause →
//!   game): exit the popped scene; the revealed one is *not* re-entered.
//! * [`Transition::Quit`] — exit the application.
//! * [`Transition::None`] — stay put.

use std::time::Duration;

use flicker_app::{App, InputState};
use flicker_render::Renderer;

/// One screen / mode of the application — a logo, a menu, the game, a pause
/// modal, and so on. Scenes are owned by the [`SceneManager`] on a stack.
pub trait Scene {
    /// Called once when the scene becomes active (on its [`Transition::Push`]
    /// or [`Transition::Replace`], or as the manager's initial scene). Upload
    /// textures / build state here. Default: no-op.
    fn enter(&mut self, _renderer: &mut Renderer) {}

    /// Advance one frame and return a [`Transition`]. Only the top scene is
    /// updated; scenes beneath an overlay are frozen.
    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition;

    /// Draw the scene. Overlays draw over whatever is already on screen.
    fn render(&mut self, renderer: &mut Renderer);

    /// Called once when the scene is removed ([`Pop`](Transition::Pop) /
    /// `Replace` / `Quit`). Free GPU resources here. Default: no-op.
    fn exit(&mut self, _renderer: &mut Renderer) {}

    /// `true` if the scene below should stay visible beneath this one — a
    /// modal/overlay such as a pause menu. Default `false` (an opaque scene
    /// that fully covers the screen).
    fn is_overlay(&self) -> bool {
        false
    }
}

/// A stack reshape requested by a scene's [`update`](Scene::update).
pub enum Transition {
    /// Stay on the current scene.
    None,
    /// Replace the top scene with a new one (e.g. menu → loading).
    Replace(Box<dyn Scene>),
    /// Unwind the entire stack (exiting each scene, top-down) and start over at
    /// a new scene (e.g. pause → main menu). Unlike [`Replace`](Transition::Replace),
    /// which swaps only the top, this frees every scene beneath it too.
    ReplaceRoot(Box<dyn Scene>),
    /// Overlay a scene, freezing the one below (e.g. game → pause).
    Push(Box<dyn Scene>),
    /// Remove the top scene, revealing the one below.
    Pop,
    /// Exit the application.
    Quit,
}

/// Owns the scene stack and drives it as a [`flicker_app::App`].
pub struct SceneManager {
    stack: Vec<Box<dyn Scene>>,
    pending: Transition,
    quit: bool,
}

impl SceneManager {
    /// Create a manager starting on `initial`; its [`Scene::enter`] fires from
    /// [`App::init`].
    #[must_use]
    pub fn new(initial: Box<dyn Scene>) -> Self {
        Self {
            stack: vec![initial],
            pending: Transition::None,
            quit: false,
        }
    }

    /// Apply the transition stored by the last `update`. Runs in `render`
    /// because `enter`/`exit` need `&mut Renderer`.
    fn apply_pending(&mut self, renderer: &mut Renderer) {
        match std::mem::replace(&mut self.pending, Transition::None) {
            Transition::None | Transition::Quit => {}
            Transition::Replace(mut next) => {
                if let Some(mut top) = self.stack.pop() {
                    top.exit(renderer);
                }
                next.enter(renderer);
                self.stack.push(next);
            }
            Transition::ReplaceRoot(mut next) => {
                // Unwind the whole stack top-down (each scene frees its GPU
                // resources in `exit`), then start fresh at `next` — the
                // "return to main menu" primitive. `Replace` would swap only the
                // top and leave the frozen game scene(s) beneath it stranded.
                while let Some(mut top) = self.stack.pop() {
                    top.exit(renderer);
                }
                next.enter(renderer);
                self.stack.push(next);
            }
            Transition::Push(mut next) => {
                next.enter(renderer);
                self.stack.push(next);
            }
            Transition::Pop => {
                if let Some(mut top) = self.stack.pop() {
                    top.exit(renderer);
                }
                if self.stack.is_empty() {
                    self.quit = true; // popped the last scene → nothing left to run
                }
            }
        }
    }

    fn visible_start(&self) -> usize {
        let overlays: Vec<bool> = self.stack.iter().map(|s| s.is_overlay()).collect();
        visible_start_in(&overlays)
    }
}

/// The depth band each stacked scene occupies: its stack position × this stride
/// becomes the scene's base 2D layer. Must exceed the largest *relative* layer any
/// single scene uses internally — a redesigned modal spans ~2 sub-layers
/// (background/Muse vs. popup + button labels), an open dropdown adds one more, and
/// an in-scene HUD reaches ~10 — so scenes never interleave and an overlay's
/// vector panels *and* text cleanly cover the scene beneath it.
const SCENE_LAYER_STRIDE: f32 = 100.0;

/// Index of the lowest scene that must be drawn this frame: the topmost opaque
/// (non-overlay) scene, since overlays draw over whatever is beneath them.
/// Pure helper, split out for unit testing.
fn visible_start_in(overlays: &[bool]) -> usize {
    (0..overlays.len()).rposition(|i| !overlays[i]).unwrap_or(0)
}

impl App for SceneManager {
    fn init(&mut self, renderer: &mut Renderer) {
        if let Some(top) = self.stack.last_mut() {
            top.enter(renderer);
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) {
        if let Some(top) = self.stack.last_mut() {
            let transition = top.update(dt, input, renderer);
            if matches!(transition, Transition::Quit) {
                self.quit = true;
            }
            self.pending = transition;
        }
    }

    fn should_quit(&self) -> bool {
        self.quit || self.stack.is_empty()
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // Structural changes need &mut Renderer (enter/exit), so apply them
        // here, then draw the visible slice bottom-up.
        self.apply_pending(renderer);
        let start = self.visible_start();
        for (offset, scene) in self.stack[start..].iter_mut().enumerate() {
            // Each scene occupies a wide DEPTH BAND (its stack position ×
            // SCENE_LAYER_STRIDE), not a single layer — so a scene's internal
            // sub-layers (a modal's background/Muse vs. its popup + labels, an open
            // dropdown over its rows) never collide with the next scene's, and an
            // overlay's panels *and* text cleanly cover the scene beneath it. A
            // scene offsets small relative layers from `renderer.layer()` within
            // its band (see render_hud's `base + layer`).
            renderer.set_layer((start + offset) as f32 * SCENE_LAYER_STRIDE);
            scene.render(renderer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::visible_start_in;

    #[test]
    fn visibility_slice() {
        assert_eq!(visible_start_in(&[]), 0);
        assert_eq!(visible_start_in(&[false]), 0); // one full scene
        assert_eq!(visible_start_in(&[false, true]), 0); // game + overlay → draw both
        assert_eq!(visible_start_in(&[false, true, true]), 0); // game + two overlays
        assert_eq!(visible_start_in(&[false, false]), 1); // two opaque → only the top
        assert_eq!(visible_start_in(&[false, false, true]), 1); // opaque, opaque, overlay
        assert_eq!(visible_start_in(&[true]), 0); // overlay with nothing under it
    }
}
