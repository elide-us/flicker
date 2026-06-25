//! `flicker-system` — the **star-system formation simulation** (Phase 1 + Phase 2), as a
//! headless library with a clean API.
//!
//! A supernova casts its elements outward by atomic weight ([`model`]) into a clumpy, sheared
//! cloud ([`cloud`]) carrying a conserved per-element mass layer ([`mass`]). **Phase 1** reads the
//! overdensity **hot spots** out of that cloud ([`detect`]). **Phase 2** ignites a gravitational
//! collapse ([`collapse`]): the gas extracts into a pinned central star and the rest accretes into
//! planets, moons and rings — emergent, conserved to the gram. The selected habitable-zone world's
//! exact composition is the **Epoch-3 handoff** that seeds the single-world planet sim (Phase 3).
//!
//! Drive it all through [`System`]: feed a [`SystemConfig`] (every lever), read a [`SystemState`]
//! and an [`Epoch3Handoff`]. No GPU, no engine deps — the viewer (`examples/flicker-sol2`) and the
//! planet sim consume this crate. (Extracted from that example; NOT `flicker-celestial`, abandoned.)

mod cloud;
mod collapse;
mod config;
mod detect;
mod mass;
mod model;
mod system;

// ── public API ──────────────────────────────────────────────────────────────────────
pub use cloud::CloudField;
pub use collapse::{BodyType, Sim};
pub use config::{SystemConfig, Tuning, DEFAULT_CLUMP, DEFAULT_MOTES_PER_EL, DEFAULT_SEED};
pub use detect::{detect, HotSpot, MAX_HOT_SPOTS};
pub use mass::{CloudMass, MassParams, EARTH_PER_SUN};
pub use model::{load_tables, CastParams, Ejecta, ElementCast};
pub use system::{BodySnapshot, Epoch3Handoff, ElementMass, System, SystemState};
