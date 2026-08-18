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
//! # Lifecycle phase
//!
//! The manager is also the application **kernel**: it tracks a [`Phase`]
//! (`Starting → Loading → Running ⇄ Paused → Unloading → Stopping`) — the app-state
//! authority. Today the phase is DERIVED from the stack and only OBSERVED (read via
//! [`SceneManager::phase`]); nothing gates on it yet, so runtime behaviour is
//! unchanged. It is the seam later steps use to stream resources, freeze the
//! simulation while paused, and shut down.
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

use flicker_app::{App, CursorImage};
pub use flicker_app::{FrameInput, InputEvent, RouteCtx};
use flicker_input_core::{InputContext, InputState};

/// The resolved discrete input a scene receives each frame — the central event pump's
/// [`FrameInput`], surfaced under the scene vocabulary.
///
/// This is the seam that DECOUPLES input from the scene (Input Standardization,
/// 2026-08-10): the RUNNER owns the pipeline — device snapshot → `Resolver` → active
/// [`InputContext`] — and threads the already-resolved signal `events` + the `route`
/// scratch down through the [`SceneManager`] to the active scene. A scene NEVER
/// re-resolves and never owns a `Resolver`; it composes its handler chain (the UI
/// walker + its own gameplay base) and routes `signals.events` through it. Context
/// requests the chain pushes into `route` are reconciled by the runner against the ONE
/// shared context stack after the frame.
pub type SceneInput<'a> = FrameInput<'a>;

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
    ///
    /// `signals` carries the frame's resolved input (see [`SceneInput`]) — a scene
    /// routes `signals.events` through its handler chain rather than owning a
    /// resolver. `input` remains for the pointer + analog channel (the discrete bus
    /// is `signals.events`; the analog split is a later stage).
    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition;

    /// The [`InputContext`] this scene's active surface owns — the runner resolves the
    /// pump's events for it. `None` (default) = the `World` base. A menu returns
    /// `Menu`; gameplay leaves it at the base. Only the TOP scene's is consulted.
    fn input_context(&self) -> Option<InputContext> {
        None
    }

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

    /// Where a fired result NAME routes this scene — consulted by the kernel when the
    /// scene returns [`Transition::Fire`]. Default `None`: the scene declares no
    /// destination for that result, and the kernel logs and stays put. A scene backed by
    /// a scene file returns its file's `exits` lookup here, so the chain lives in DATA
    /// and the scene names an INTENT rather than constructing its successor.
    fn route(&self, _result: &str) -> Option<Transition> {
        None
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
    /// Go to the scene REGISTERED UNDER `id`, letting the manager resolve and build
    /// it — the id-addressed form of `Replace` / `ReplaceRoot` / `Push`.
    ///
    /// This is what takes the scene chain out of Rust. The variants above all carry a
    /// `Box<dyn Scene>`, which means the departing scene must name its successor's
    /// concrete TYPE and construct it: `Transition::Replace(Box::new(MenuScene::new()))`
    /// hard-wires "logo is followed by menu" into the logo's source. Naming an id
    /// instead lets the successor be authored — a splash, the main menu and a bench
    /// launch are then the SAME mechanism, and the chain lives in the roster rather
    /// than in constructor calls.
    ///
    /// An id with no registered scene is a loud no-op (logged, stack untouched) — a
    /// typo must not silently strand the player on a splash screen.
    Goto {
        /// The registered scene id, e.g. `"ce_logo"` / `"main_menu"`.
        id: String,
        /// Which stack reshape to perform once the id resolves.
        mode: GotoMode,
    },
    /// Fire a named RESULT and let the KERNEL decide where it goes — it consults the
    /// active scene's [`route`](Scene::route) (the scene file's authored `exits`) and
    /// enacts the destination. This is how a scene names an INTENT (`"done"`, `"quit"`)
    /// with zero knowledge of its successor; contrast the `Box`-carrying moves above,
    /// which force the departing scene to construct its own successor.
    Fire(String),
}

/// How a [`Transition::Goto`] reshapes the stack once its id resolves — the three
/// forward moves, named so a roster can spell them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GotoMode {
    /// Swap the top scene (a splash advancing to the next splash).
    Replace,
    /// Unwind the whole stack and start over (returning to the main menu).
    ReplaceRoot,
    /// Overlay, freezing the scene below (a pause menu).
    Push,
}

/// Resolves a scene id to a fresh instance — the manager's link to whatever owns the
/// roster.
///
/// The manager deliberately does NOT own the registry: scene ids and their factories
/// are a CLIENT concern (`flicker-shell` holds `SceneEntry { id, factory, … }`), and a
/// crate this low must not depend on the app above it. Injecting the lookup keeps the
/// mechanism here and the roster there.
pub type SceneResolver = Box<dyn Fn(&str) -> Option<Box<dyn Scene>>>;

/// The application kernel's lifecycle phase — the app-state authority (Aaron 2026-08-10).
///
/// The kernel is always in exactly ONE phase. Today it is DERIVED from the flow the
/// stack already drives and merely OBSERVED (nothing gates on it yet), so runtime
/// behaviour is unchanged; it is the seam later steps use to stream resources
/// (`Loading`/`Unloading`), freeze the simulation (`Paused`), and shut down
/// (`Stopping`). Read it with [`SceneManager::phase`]. All six states are enumerated
/// now (Aaron's map); `Loading`/`Unloading` are ENTERED by the resource-lifecycle step,
/// so today's synchronous transitions settle straight to `Running`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before the first scene is live — window, renderer, and kernel coming up.
    Starting,
    /// Bringing a scene's resources in (its [`Scene::enter`]).
    Loading,
    /// The active scene is live: updating and rendering each frame.
    Running,
    /// The active scene is suspended beneath an overlay — e.g. a pause menu.
    Paused,
    /// Releasing a departing scene's resources (its [`Scene::exit`]).
    Unloading,
    /// Shutting down — no scene left to run.
    Stopping,
}

/// Owns the scene stack and drives it as a [`flicker_app::App`].
pub struct SceneManager {
    stack: Vec<Box<dyn Scene>>,
    pending: Transition,
    quit: bool,
    /// The kernel's current lifecycle [`Phase`]. Starts at [`Phase::Starting`]; the
    /// first [`Scene::enter`] (from [`App::init`]) settles it to [`Phase::Running`].
    phase: Phase,
    cursor: Option<CursorImage>,
    /// Resolves [`Transition::Goto`] ids. `None` = no roster wired, so id-addressed
    /// transitions cannot be served (they log and do nothing).
    resolver: Option<SceneResolver>,
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
            phase: Phase::Starting,
            cursor: None,
            resolver: None,
        }
    }

    /// Start on the scene registered under `entry`, resolving it through `resolver` —
    /// the fully id-addressed boot.
    ///
    /// `SceneManager::new` takes a constructed scene, which makes the ENTRY POINT a
    /// compiled-in decision the same way `Replace(Box::new(..))` made the chain one.
    /// Here the first scene is named, not built, so the whole chain — entry included —
    /// is a roster concern.
    ///
    /// Returns `None` when `entry` is not registered; that is a fatal boot
    /// misconfiguration and the caller should say so loudly rather than limp on.
    #[must_use]
    pub fn from_roster(entry: &str, resolver: SceneResolver) -> Option<Self> {
        let initial = resolver(entry)?;
        Some(Self {
            stack: vec![initial],
            pending: Transition::None,
            quit: false,
            phase: Phase::Starting,
            cursor: None,
            resolver: Some(resolver),
        })
    }

    /// Wire a roster onto a manager built with [`new`](Self::new), so its scenes can
    /// use [`Transition::Goto`].
    #[must_use]
    pub fn with_resolver(mut self, resolver: SceneResolver) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Attach a custom hardware cursor for the runner to register at window
    /// creation (e.g. the shell's themed pointer). Appearance only — cursor
    /// visibility is owned by the input-modality wiring, not the manager.
    #[must_use]
    pub fn with_cursor(mut self, cursor: Option<CursorImage>) -> Self {
        self.cursor = cursor;
        self
    }

    /// Apply the transition stored by the last `update`. Runs in `render`
    /// because `enter`/`exit` need `&mut Renderer`.
    fn apply_pending(&mut self, renderer: &mut Renderer) {
        let pending = std::mem::replace(&mut self.pending, Transition::None);
        // Resolve any INDIRECTION — a fired result (`Fire`) or an id-addressed `Goto` —
        // into a concrete stack move BEFORE the stack is touched, so the arms below stay
        // the only place the stack changes shape.
        let pending = self.resolve_indirection(pending);
        match pending {
            Transition::None | Transition::Quit => {}
            // Resolved above into a concrete move; unreachable here.
            Transition::Fire(_) | Transition::Goto { .. } => {}
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
        // Come to rest in the phase the reshaped stack now implies (Running / Paused /
        // Stopping). A `None`/`Quit` transition re-settles harmlessly to the same phase.
        self.settle_phase();
    }

    /// Resolve an indirect [`Transition`] — a fired result or an id-addressed `Goto` —
    /// down to a concrete stack move, BEFORE the stack is touched. `Fire(name)` asks the
    /// ACTIVE scene where its result routes ([`Scene::route`], backed by the scene file's
    /// authored exits): the scene names an intent, the KERNEL decides. `Goto{id}` builds
    /// the named scene through the roster. The two can chain (a fired result routes to a
    /// `Goto`), so this loops until it reaches a concrete move; an unrouted fire or a
    /// missing id is a loud no-op.
    fn resolve_indirection(&self, mut pending: Transition) -> Transition {
        loop {
            pending = match pending {
                Transition::Fire(name) => match self.stack.last().and_then(|s| s.route(&name)) {
                    Some(next) => next,
                    None => {
                        tracing::warn!(
                            "the active scene fired result '{name}', but its file routes \
                             it nowhere — ignored; add an `exits` entry for it"
                        );
                        return Transition::None;
                    }
                },
                Transition::Goto { id, mode } => match self.resolve(&id) {
                    Some(next) => {
                        return match mode {
                            GotoMode::Replace => Transition::Replace(next),
                            GotoMode::ReplaceRoot => Transition::ReplaceRoot(next),
                            GotoMode::Push => Transition::Push(next),
                        };
                    }
                    // Fail LOUD and stay put. Silently doing nothing would look like a
                    // hung splash screen; advancing to some fallback would hide the typo.
                    None => {
                        tracing::error!(
                            "scene id '{id}' is not in the roster — transition ignored; \
                             register it, or fix the id the departing scene named"
                        );
                        return Transition::None;
                    }
                },
                concrete => return concrete,
            };
        }
    }

    /// Build the scene registered under `id`, if a roster is wired and knows it.
    fn resolve(&self, id: &str) -> Option<Box<dyn Scene>> {
        match &self.resolver {
            Some(r) => r(id),
            None => {
                tracing::error!("scene id '{id}' requested but no roster is wired");
                None
            }
        }
    }

    fn visible_start(&self) -> usize {
        let overlays: Vec<bool> = self.stack.iter().map(|s| s.is_overlay()).collect();
        visible_start_in(&overlays)
    }

    /// The kernel's current lifecycle [`Phase`] — the app-state authority. Observed
    /// today (nothing gates on it yet); the seam later steps drive resource streaming,
    /// pause, and shutdown from.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Settle the phase to what the stack now implies, once a frame's transition has been
    /// applied: `Stopping` when nothing is left to run, `Paused` when the top scene is an
    /// overlay (the scene beneath it is frozen), else `Running`.
    fn settle_phase(&mut self) {
        let top_overlay = self.stack.last().map(|s| s.is_overlay());
        self.phase = settled_phase(self.quit, top_overlay);
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

/// The settled [`Phase`] implied by the stack after a transition — factored out (like
/// [`visible_start_in`]) so the kernel's phase logic is unit-testable without a GPU
/// [`Renderer`]. `top_overlay` is the top scene's [`Scene::is_overlay`], or `None` when
/// the stack is empty. A requested exit (`quit`) outranks everything: it is `Stopping`
/// whatever is on the stack.
fn settled_phase(quit: bool, top_overlay: Option<bool>) -> Phase {
    match (quit, top_overlay) {
        (true, _) => Phase::Stopping,
        (false, None) => Phase::Stopping, // empty stack → nothing left to run
        (false, Some(true)) => Phase::Paused, // an overlay on top freezes the scene beneath
        (false, Some(false)) => Phase::Running,
    }
}

impl App for SceneManager {
    fn init(&mut self, renderer: &mut Renderer) {
        if let Some(top) = self.stack.last_mut() {
            top.enter(renderer);
        }
        // The first scene is now live: Starting → Running (settle_phase derives it).
        self.settle_phase();
    }

    fn cursor(&self) -> Option<CursorImage> {
        self.cursor.clone()
    }

    /// The runner resolves the pump's events for THIS — the top scene's context.
    fn active_context(&self) -> Option<InputContext> {
        self.stack.last().and_then(|s| s.input_context())
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut FrameInput,
        renderer: &Renderer,
    ) {
        if let Some(top) = self.stack.last_mut() {
            // Forward the runner's resolved signals to the top scene — the manager is
            // pure plumbing here; the pipeline (Resolver + shared context stack) lives
            // in the runner. Scenes not yet converted ignore `signals` and still
            // resolve internally, so behaviour is unchanged until each is migrated.
            let transition = top.update(dt, input, signals, renderer);
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

    /// `with_cursor` feeds `App::cursor` verbatim; a bare manager keeps `None`
    /// (the platform arrow).
    #[test]
    fn cursor_passthrough() {
        use flicker_app::{App, CursorImage};
        struct Stub;
        impl super::Scene for Stub {
            fn update(
                &mut self,
                _: std::time::Duration,
                _: &flicker_input_core::InputState,
                _: &mut super::SceneInput,
                _: &flicker_render::Renderer,
            ) -> super::Transition {
                super::Transition::None
            }
            fn render(&mut self, _: &mut flicker_render::Renderer) {}
        }
        let img = CursorImage {
            rgba: vec![0xff; 4],
            width: 1,
            height: 1,
            hotspot: (0, 0),
        };
        let with = super::SceneManager::new(Box::new(Stub)).with_cursor(Some(img));
        assert_eq!(
            with.cursor().map(|c| (c.width, c.height, c.hotspot)),
            Some((1, 1, (0, 0)))
        );
        assert!(super::SceneManager::new(Box::new(Stub)).cursor().is_none());
    }

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

    /// The kernel's phase is a pure function of "is it quitting" + "what is on top",
    /// and a freshly built kernel is [`Phase::Starting`] until its first scene enters.
    /// (The `enter` / `apply_pending` wiring needs a GPU `Renderer`, so it is exercised
    /// in-window; the derivation carries the logic and is tested here.)
    #[test]
    fn kernel_phase_machine() {
        use super::{settled_phase, Phase, Scene, SceneInput, SceneManager, Transition};

        assert_eq!(settled_phase(false, Some(false)), Phase::Running); // one full scene
        assert_eq!(settled_phase(false, Some(true)), Phase::Paused); // overlay → frozen beneath
        assert_eq!(settled_phase(false, None), Phase::Stopping); // empty stack → nothing to run
        assert_eq!(settled_phase(true, Some(false)), Phase::Stopping); // quit outranks a live scene
        assert_eq!(settled_phase(true, Some(true)), Phase::Stopping); // quit outranks an overlay

        struct Stub;
        impl Scene for Stub {
            fn update(
                &mut self,
                _: std::time::Duration,
                _: &flicker_input_core::InputState,
                _: &mut SceneInput,
                _: &flicker_render::Renderer,
            ) -> Transition {
                Transition::None
            }
            fn render(&mut self, _: &mut flicker_render::Renderer) {}
        }
        assert_eq!(SceneManager::new(Box::new(Stub)).phase(), Phase::Starting);
    }

    /// A fired result routes through the ACTIVE scene's [`Scene::route`] (its file exits)
    /// to a concrete move; a result the scene routes nowhere is a loud no-op. Exercises
    /// the whole `Fire → route → Goto → resolve` chain without a GPU.
    #[test]
    fn fire_routes_through_the_active_scenes_exits() {
        use super::{GotoMode, Scene, SceneInput, SceneManager, Transition};

        /// Routes ANY fired result to a `Replace`-`Goto` of the scene id `"next"`.
        struct Router;
        impl Scene for Router {
            fn update(
                &mut self,
                _: std::time::Duration,
                _: &flicker_input_core::InputState,
                _: &mut SceneInput,
                _: &flicker_render::Renderer,
            ) -> Transition {
                Transition::None
            }
            fn render(&mut self, _: &mut flicker_render::Renderer) {}
            fn route(&self, _result: &str) -> Option<Transition> {
                Some(Transition::Goto {
                    id: "next".into(),
                    mode: GotoMode::Replace,
                })
            }
        }
        /// Routes nothing (the default [`Scene::route`]).
        struct Plain;
        impl Scene for Plain {
            fn update(
                &mut self,
                _: std::time::Duration,
                _: &flicker_input_core::InputState,
                _: &mut SceneInput,
                _: &flicker_render::Renderer,
            ) -> Transition {
                Transition::None
            }
            fn render(&mut self, _: &mut flicker_render::Renderer) {}
        }

        let mgr = SceneManager::new(Box::new(Router)).with_resolver(Box::new(
            |id: &str| -> Option<Box<dyn Scene>> {
                (id == "next").then(|| Box::new(Plain) as Box<dyn Scene>)
            },
        ));
        assert!(
            matches!(
                mgr.resolve_indirection(Transition::Fire("done".into())),
                Transition::Replace(_)
            ),
            "a fired result routes through the scene's exits into a concrete move"
        );

        let plain = SceneManager::new(Box::new(Plain));
        assert!(
            matches!(
                plain.resolve_indirection(Transition::Fire("nowhere".into())),
                Transition::None
            ),
            "a result the scene routes nowhere is dropped (loud no-op)"
        );
    }
}
