//! The [`Stage`] trait and its deterministic RNG (spec §7.1, §11).
//!
//! A stage is a pure step of the formation sim: it reads the cheap global
//! [`PlanetState`](crate::planet::PlanetState) and mutates the [`World`], moving
//! mass between reservoirs/columns (or debiting `escaped` / crediting
//! `delivered`) — never creating or destroying it. Its `is_live` predicate gates
//! on **chemistry, never on the tick number** (§7.2); the only clock-reading
//! exception is the exogenous module (§7.5/10), which is not part of M0.

use crate::planet::{PlanetState, World};

/// A deterministic per-stage RNG stream (SplitMix64). The whole run is seeded by
/// one `u64`, split into an independent stream per stage, so the same seed yields
/// the identical world (§11). No global RNG, no wall-clock entropy.
#[derive(Clone, Debug)]
pub struct StageRng {
    state: u64,
}

impl StageRng {
    /// A stream seeded directly.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The independent stream for stage `index` of a run seeded by `master`.
    pub fn for_stage(master: u64, index: usize) -> Self {
        let mut s = master ^ 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(index as u64 + 1);
        s = (s ^ (s >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        Self { state: s }
    }

    /// Next 64 random bits (SplitMix64).
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Next float in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// One step of the formation simulation.
pub trait Stage: Send + Sync {
    /// Stable name, used in `CellProgress` and the conservation-audit panic.
    fn name(&self) -> &'static str;

    /// Whether this stage runs given the current planet chemistry. Gates on
    /// state, **never on the tick number** (§7.2). Defaults to always-live.
    fn is_live(&self, _state: &PlanetState) -> bool {
        true
    }

    /// Advance the world by `dt_myr`. Must conserve mass — the audit after each
    /// stage panics naming this stage otherwise (§4.3).
    fn tick(&self, world: &mut World, dt_myr: f64, rng: &mut StageRng);
}
