//! flicker-mechanics — the mechanics cluster (golden-spec WS-D, collision node CDDBC150).
//!
//! Game-mechanics runtime that sits WITH combat + animation, NOT a low-level physics layer
//! (memory B5E4CFC6): TAE events open hitboxes → hitbox↔hurtbox overlap resolves hits →
//! capsule-vs-world sweeps move things. The rigid-body SOLVER stays thin Rapier/Avian middleware,
//! added only for true dynamics (ragdolls / destruction) — not here.
//!
//! This crate begins with the pure GEOMETRY layer: bone-bound collision primitives + overlap
//! queries ([`collision`]). Combat resolution (hitbox↔hurtbox on TAE windows), item drop-to-ground,
//! the oriented-box shape, the `flicker.rig` `collision` section → runtime bridge, and debug-draw
//! all layer on top in later slices — they build on this, they don't change it.

pub mod bridge;
pub mod collision;
pub mod debug;
pub mod drop;

pub use bridge::{
    autofit_capsules, autofit_capsules_from, role_from_format, shape_from_format,
    volumes_from_format,
};
pub use collision::{
    closest_point_ray_segment, fit_capsule, overlap, penetration, upright_capsule, Contact, Role,
    Shape, Volume,
};
pub use drop::{settle_offset, FallingItem, GRAVITY_CM_S2};
