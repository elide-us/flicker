//! flicker-skeletal — the CPU-authoritative skeletal-animation runtime.
//!
//! GPU-free, viewer-agnostic. A renderer (the `flicker-animation` example today,
//! `flicker-csg` later) turns this crate's posed bone transforms and skinned
//! vertices into draw calls; nothing here touches wgpu.
//!
//! The layers, bottom-up:
//! - [`format`] — the `flicker.rig` JSON contract (the shared seam with the C++
//!   `fbximport` converter) + [`format::load_dir`]: parse the rig + clips, decode
//!   the row-major/row-vector matrices, resolve clip tracks to bones **by name**.
//! - [`pose`] — sample a clip at an integer tick to per-bone LOCAL transforms, then
//!   accumulate down the hierarchy into GLOBAL (model-space) transforms. This is the
//!   authoritative pose layer; hitboxes and skinning both read it.
//! - [`skin`] — 4-influence CPU linear-blend skinning (`global × inverse_bind`).
//! - [`state`] — the animation/combat **state machine** + **TAE event timeline**,
//!   authored as a separate `flicker.pack` JSON so `flicker.rig` stays a purely
//!   mechanical FBX-derived atom.
//!
//! Extracted verbatim from `examples/flicker-animation` (the POC that proved these
//! out). See docs/flicker-animation-handoff.md and
//! docs/flicker-combat-animation-handoff.md.

pub mod format;
pub mod pose;
pub mod skin;
pub mod state;
