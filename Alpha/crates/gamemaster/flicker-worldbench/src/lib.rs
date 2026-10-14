//! **WorldBench V5** — planet generation's fifth generation: the hex world
//! shaped by RESONANCE FIELDS.
//!
//! The lineage (Aaron 2026-10-11): V1 was two flat hex maps back to back (the
//! corners broke it); V2/V3 put hex columns on the icosahedral sphere and worked
//! out the chemistry ledger; V4 is the Populous bench — the hex-stack ledger and
//! the `Evolution` era that pushes material around it, which gives realistic
//! continents, motion and erosion. What V4 never had is a procedural answer to
//! SHAPE below the hex. V5's answer is one family of continuous fields over the
//! sphere — waves with their own wavelengths, frequencies and sources, overlaid
//! additively — that the ledger's conserved quantities are DISTRIBUTED by, the
//! same functions evaluated from the whole planet down to a single cluster.
//!
//! This crate is the SCREEN for that work and nothing else: the fields live in
//! [`flicker_worldengine::resonance`] so any scene can reach them, the globe is
//! the shared [`flicker_globe::GlobeWorld`], and the bench frame is Populous's
//! (header rails, panel | RTT | panel, nav footer) authored afresh in
//! `worldbench.scene.json` — Populous itself is untouched. It is a GAME MASTER
//! scene: a world is authored here.
//!
//! The first expression is the FULL PLANET WAVE — six pentagons crest above the
//! mean, six trough below — drawn as the tile shell EXTRUDED by the wave, under a
//! translucent sea sphere whose level the SEA dial sets: 0% at the lowest tile,
//! 98% at the highest, the last two percent drowning the planet.

mod scene;
pub mod ui;

pub use scene::{scene, WorldBench};
