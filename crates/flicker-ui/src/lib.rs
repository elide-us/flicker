//! flicker-ui — a thin **re-export** of [`flicker_widgets`].
//!
//! The UI widget toolkit was promoted into the Alpha frontend cluster
//! (`Alpha/crates/frontend/flicker-widgets`); this crate re-exports it so existing
//! consumers that import `flicker_ui::…` keep working unchanged. New code may depend
//! on `flicker-widgets` directly. See `docs/ui-widget-catalog-handoff.md`.

pub use flicker_widgets::*;
