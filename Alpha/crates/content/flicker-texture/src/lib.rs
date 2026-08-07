//! flicker-texture — the procedural texture **synthesizer**.
//!
//! Stage ③ of the material pipeline (`Elements → Compounds → **Materials**`): the
//! in-game surface appearance. Elements and compounds answer *what a place is
//! made of*; this answers *what that looks like*.
//!
//! # The instrument
//!
//! It is a synthesizer, deliberately and all the way down:
//!
//! ```text
//!   6 channels ──▶ mix bus ──▶ output stage ──▶ PBR map set
//!   (oscillators)  (blend)     (filters)        (the sound)
//! ```
//!
//! - A [`Channel`] is one voice — a tileable noise source with its own scale,
//!   octaves, warp and shaping, folded into the bus by a [`BlendMode`] at an
//!   amount.
//! - The rack is a **fixed six**, folded **ordinally**. No routing matrix, no
//!   node graph: the console is a strip of sliders you can see all of at once.
//! - The [`OutputStage`] projects the one mixed field into every map — colour
//!   through a ramp, relief from the gradient, roughness and metalness by
//!   modulation, occlusion from the neighbourhood. Every map is a consequence of
//!   the same field, which is why turning one knob moves the whole surface
//!   coherently, the way a real material behaves.
//!
//! # Two properties everything else rests on
//!
//! **Seamless.** The lattice wraps on the tile, and the baker's neighbourhood
//! operations wrap with it. A swatch abuts itself with no ridge — the difference
//! between a texture you can put on terrain and one you cannot.
//!
//! **Deterministic.** `(recipe, size)` is the entire input. No clock, no globals,
//! no filesystem. The same recipe rebuilds byte-identically forever, at any
//! resolution, on any machine — so the saved artifact is the small [`TextureRecipe`]
//! and the megabytes of image are a rebuildable consequence.
//!
//! # What this crate does not do
//!
//! It has no graphics and no IO: it hands back CPU pixel buffers and lets the
//! caller decide whether they become a GPU texture, a PNG in `staging/`, or a
//! terrain lookup. It also does not *classify* — it names which material a
//! recipe is the appearance of, and the composition → material classifier that
//! would choose that binding automatically lives elsewhere.

pub mod bake;
pub mod channel;
pub mod output;
pub mod presets;
pub mod random;
pub mod recipe;
pub mod size;

pub use bake::{bake, field, Map, MapKind, MapSet};
pub use channel::{mix, BlendMode, Channel, NoiseKind, CHANNEL_COUNT};
pub use output::{ColorRamp, OutputStage, RampStop};
pub use random::random;
pub use recipe::TextureRecipe;
pub use size::{offered, rung, BakeSize, BAKE_DEFAULT, BAKE_SIZES, PREVIEW_SIZE};
