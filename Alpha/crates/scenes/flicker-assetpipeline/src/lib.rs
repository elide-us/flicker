//! flicker-assetpipeline — **Clayworks Bench**, the content workbench.
//!
//! The realized workflow (golden spec 4916D78B, node A3A3259C): open a folder of raw
//! sources → a step rail classifies what is in it → matches it to the canonical
//! skeleton → bakes the one self-describing `flicker.rig` → hot-reloads in-app. Every
//! stage is `flicker-content`'s; this crate hosts, it does not process.
//!
//! The canonical shape (Populous is the reference): `assetpipeline.scene.json` is the
//! tree, `assetpipeline.lua` lights the selected workflow's step slice, and the Rust is
//! split as `services` (the document + its pipeline services, UI-free), `ui` (the
//! roster the tree / Model / dispatcher share), `compose` (what the rig panels draw,
//! from the document), `meshes` (the GPU caches the panels' draw items come from),
//! `gizmo` (the bench's half of the shared 3D gadget: which joint is selected and what a
//! `GadgetDelta` means to the document) and `scene` (the thin behaviour).

mod compose;
mod gizmo;
mod meshes;
mod scene;
mod services;
#[cfg(test)]
mod tests;
mod ui;

pub use scene::scene;
