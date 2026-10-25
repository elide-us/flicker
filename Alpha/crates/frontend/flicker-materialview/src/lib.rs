//! **flicker-materialview** — the lit material SAMPLE, an engine surface filler.
//!
//! A [`MaterialSample`] draws one material — albedo, normal, roughness, metalness, AO and
//! Emit — on a turning unit sphere (or an upright plane, [`Body`]) under a fixed light,
//! into the `surface` slot the walker reserved for it, and composites the image there. It
//! is the 3D-filler shape of `flicker_rigview::Doll`: [`seat`](MaterialSample::seat) it
//! from the walker's `SurfaceSlot` (or [`seat_at`](MaterialSample::seat_at) a rect the
//! host laid out), [`render`](MaterialSample::render) it into the frame graph,
//! [`release`](MaterialSample::release) its target on exit. Its offscreen target goes
//! through the engine's ONE surface-filler lifecycle (`flicker_globe::GlobeView`), so a
//! sample's target is created, resized, composited and freed exactly as a doll's is.
//!
//! # The contract
//!
//! - **Geometry is shared.** The host builds ONE [`SampleBodies`] (a sphere and a plane)
//!   when it first has a renderer and lends it to every sample it draws; it gives the
//!   meshes back with [`SampleBodies::release`] on exit, after each sample's own release.
//! - **The look is the consumer's authored stage.** Lighting, clear, framing (`camera`),
//!   the HDR attachment and its `tonemap_grade` resolve, and the `bloom` that makes a glow
//!   read as a glow are all the `stages.<source>` block the consumer names and compiles.
//!   That stage MUST author the `material` layer ([`SAMPLE_LAYERS`]): it IS the sample
//!   body. A stage without it draws nothing and says so once per sample; a stage authoring a
//!   layer the sample cannot draw, or no camera, is named once per sample too. That check is
//!   one function, [`sample_stage_problems`], so a host seating many samples under one stage
//!   runs it ONCE itself and names a broken stage once (the Material Catalog does).
//! - **Liveness is the seat's rate.** A `live` seat turns every frame; a `dirty` seat
//!   re-renders when its dirty channel fires — the sample compares the body, turntable, map
//!   HANDLES, gain and seat size against its last pass and publishes `dirty` on any
//!   difference (and every frame it turns). It compares handles, never pixels: a host that
//!   rewrites a texture IN PLACE under the same handle (`Renderer::update_texture` — the
//!   MaterialBench's preview maps) must call [`MaterialSample::mark_dirty`], or a non-live
//!   seat keeps the old picture. A `poster` seat keeps its image until that same channel
//!   fires — it is declared `dirty` with its turntable parked, so a poster whose maps arrive
//!   after its first frame re-renders exactly once, when they land (the Doll's rule). The
//!   composite runs every frame whatever the rate.
//! - **Zoom is the host's view, a divisor on the stage's distance.** [`MaterialSample::zoom`]
//!   (`1.0` = the stage's own framing) stands the authored camera `dist / zoom` from its
//!   target and changes nothing else; it is recorded like the body, so a zoom is a new image.
//!   Every host moves it by the ONE pair of laws — [`MaterialSample::zoom_by`] for the wheel
//!   (linear: a notch is one step), [`MaterialSample::set_zoom`] for a slider — inside the
//!   [`ZoomDial`] its own tree's zoom slider authors.
//! - **The maps are the caller's, per render** ([`SampleMaps`]): textures a host already
//!   holds ([`SampleMaps::of_set`] — borrowed, never freed here), a promoted bundle's maps
//!   ([`decode_bundle_maps`] on a worker, then [`upload_decoded`] → an [`OwnedMaps`]), or a
//!   flat PBR read off the material's index row ([`FlatMaterial::of`] through a shared
//!   [`FlatTextures`] cache). What this crate uploads is freed exactly once, by a consuming
//!   `release` ([`OwnedMaps::release`], [`FlatTextures::release`]) on the host's exit.
//!
//! # The emissive gain
//!
//! [`SampleMaps::emissive_gain`] is the draw-time multiplier on the Emit map
//! (`MeshDrawOptions::emissive_gain`, applied in the one PBR material text). An 8-bit Emit
//! texel caps at 1.0, so at the default gain of `1.0` a glow never outshines the surface's
//! own white; above it the glow is real HDR radiance — on the stage's `rgba16f` attachment,
//! where its `bloom` pass spreads it into a halo before `tonemap_grade` rolls it off. The
//! Emit texel itself stays the curated palette colour ([`GlowPalette`]); the gain is how
//! bright it burns, never what hue.

pub mod graph_view;
mod maps;
mod palette;
mod sample;

pub use maps::{
    decode_bundle_maps, decode_bundle_maps_with, decode_map, map_path, upload_decoded, DecodedMaps,
    FlatMaterial, FlatTextures, OwnedMaps, SampleMaps, Texel,
};
pub use palette::{GlowColor, GlowPalette};
pub use sample::{
    sample_stage_problems, Body, MaterialSample, SampleBodies, ZoomDial, SAMPLE_LAYERS,
};
