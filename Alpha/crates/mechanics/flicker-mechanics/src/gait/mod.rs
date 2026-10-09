//! THE GAIT GENERATOR — G2 of the gait generator / IK solver design (37704D6B, Aaron's ruling
//! CD36B9BE): procedural locomotion for the modular skeletons that have no motion library — the
//! quadrupeds, the sprawlers, the hoppers, the climbers, later the fliers. Pure glam math over a
//! skeleton's WORLD frames, renderer- and signal-agnostic like [`crate::ik`], which it drives.
//!
//! - [`pattern`] — the GAIT TABLE: each gait's period, duty factor, per-foot phase offsets,
//!   stride, step height and body bob, and the Froude-number selection of a gait from speed.
//! - [`surface`] — [`SurfaceQuery`](surface::SurfaceQuery), what a foot lands on: a flat floor,
//!   a height function (the island), and (G3) mesh supports for trunks and branches.
//! - [`planner`] — the FOOT PLANNER: a planted foot stays on its contact while the body moves
//!   over it; a swinging foot arcs to a landing predicted from the body's velocity and snapped to
//!   the support.
//! - [`body`] — the BODY SOLVER: the trunk stands over its stance contacts, pitched and rolled to
//!   the plane through them, its up blended toward the support normal by `cling` (a climber).
//! - [`locomotion`] — the DRIVER: limbs read off the skeleton ([`crate::ik::limbs_of`]), the
//!   phase clock, the rigid placement of the body frame and one [`crate::ik::plant`] per foot.
//! - [`flight`] — the FLAP CYCLE (G4): wings beat about the shoulder with a lag down the chain,
//!   feather groups sweep on the upstroke, a bat's digits fold; glide and the perch fold.
//! - [`mesh`] — mesh and capsule supports (G3): trunks and branches a climber plants on.

pub mod body;
pub mod flight;
pub mod locomotion;
pub mod mesh;
pub mod pattern;
pub mod planner;
pub mod surface;

pub use body::{BodyFrame, BodySolver};
pub use flight::{FeatherGroup, FlapCycle, Wing, Wings};
pub use locomotion::Locomotion;
pub use mesh::{ray_capsule, ray_triangle, CapsuleSupport, Composite, MeshSupport};
pub use pattern::{
    kind_of, pattern, select, speed_for, GaitKind, GaitPattern, LocomotionFamily, FOOT_SLOTS,
    GRAVITY_CM,
};
pub use planner::{FootPlanner, FootState};
pub use surface::{Contact, FlatFloor, HeightFn, SupportKind, SurfaceQuery};
