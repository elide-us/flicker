//! `flicker-input-router` — the event bus + focus + consumer API.
//!
//! This crate formalizes the arbitration that lives hand-rolled in every scene
//! today (spec §4.1): the `hud_hit` / `chat_hit` / `active()==World` gate ladder
//! in `flicker-pocclusters` becomes a typed **event bus** with capture / bubble
//! and consume-or-pass. Per the input-contract design it is "the walker's
//! existing hit-test / focus / results formalized + scene-at-base — NOT a
//! parallel arbiter" (`19E72F39`).
//!
//! # Purity & dependency direction
//!
//! The router **core** is pure and **window-free**: it depends ONLY on
//! [`flicker_input_core`] and touches no platform, no `flicker-widgets`, and no
//! `flicker-scene`. The edge is one-way — frontend → router → core, never the
//! reverse (spec §2). The router does **not** resolve input or read devices; it
//! wraps the core [`Resolver`](flicker_input_core::Resolver)'s
//! [`Fired`](flicker_input_core::Fired) output into [`InputEvent`]s and routes
//! them.
//!
//! # The bus (spec §4.2)
//!
//! A per-frame ordered handler chain, highest input priority first (matching the
//! priority `pocclusters` encodes by statement order):
//!
//! ```text
//! [0] system/global   capture-only: quit, debug-console toggle, screenshot
//! [1] scene/mode root  the active scene (declares its base context)
//! [2] context/modal    pushed modals + EXCLUSIVE contexts (PauseMenu, TextEntry)
//! [3] UI panel tree    the walker (hit-test + focused element)   ← widgets adapter
//! [4] gameplay at base camera / world-pick / movement  ← always last, runs only on Pass
//! ```
//!
//! [`Router::dispatch`] runs two phases per event:
//!
//! 1. **capture** — top-down (`chain[0]` → `chain[N]`); the first
//!    [`Flow::Consumed`] stops that event (this is where the exclusive keyboard
//!    owner claims text + Enter/Esc so `Menu` never reaches the scene root).
//! 2. **target + bubble** — high priority → low (`chain[0]` → `chain[N]`); the
//!    first [`Flow::Consumed`] stops propagation, a [`Flow::Pass`] lets the next
//!    handler act, and the gameplay-at-base handler (last in chain) runs only
//!    when everything above it passed.
//!
//! # The seam (spec §5)
//!
//! The caller drives the per-frame order: **device** fills the snapshot →
//! **router** derives the active [`InputContext`](flicker_input_core::InputContext)
//! → **core** resolves `(prev, curr) × context → Vec<Fired>` → **router** wraps
//! each `Fired` into an [`InputEvent`] (via [`InputEvent::from_fired`]) and
//! [`Router::dispatch`]es. Handlers emit router-owned intents into
//! [`RouteCtx`]; after dispatch the consumer applies them with
//! [`apply_context_requests`] and reads the [`DispatchReport`]. Structural scene
//! transitions stay with the scene, which reads the report after dispatch — the
//! router never touches `flicker-scene`.
//!
//! # Module map
//!
//! - [`event`] — [`InputEvent`], [`Flow`].
//! - [`handler`] — the [`InputHandler`] trait.
//! - [`router`] — [`Router`], [`RouteCtx`], [`RouterRequest`], [`DispatchReport`],
//!   [`FocusChange`], [`apply_context_requests`].
//! - [`nav`] — [`Focusable`], [`NavDir`], [`nav`], [`tab`] (window-free
//!   directional-nav helpers, spec §4.4).

pub mod event;
pub mod handler;
pub mod nav;
pub mod router;

// ── Flat public surface (crate root) ──
pub use event::{Flow, InputEvent};
pub use handler::InputHandler;
pub use nav::{nav, nav_geometric, Focusable, NavDir};
pub use router::{
    apply_context_requests, DispatchReport, FocusChange, RouteCtx, Router, RouterRequest,
};
