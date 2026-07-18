//! The reservoirs the accretion budget partitions into (spec §4.2). The planet is
//! an **open system**: `Delivered` adds mass from the outer system, `Escaped`
//! loses it to space, and both appear in the conservation invariant (§4.3).
//!
//! Every reservoir that holds conserved matter is a [`Composition`] — the same
//! element ledger a column layer uses. The ocean's chemistry scalars (DIC, pH,
//! alkalinity, salinity, dissolved O₂) and the residual assert of §4.5 arrive
//! with the hydrosphere (M3); here the ocean *is* its conserved element content,
//! and `mass_kg()` is derived from that — never an independent number that could
//! drift from the matter actually in it.

use flicker_materials::ElementId;
use flicker_worldstate::Composition;

/// The aggregated world ocean — **no geometry** (spec §4.5). Its element content
/// is the truth; mass is read off it.
#[derive(Clone, Debug, Default)]
pub struct Ocean {
    /// Conserved element mass held in the world ocean (H, O, dissolved C/Na/Cl…).
    /// Empty at t=0 — the ocean CONDENSES OUT of the infall/escape balance later
    /// (M3). `mass_kg() == 0` is a legal snowball / runaway-greenhouse state.
    pub contents: Composition,
}

impl Ocean {
    /// Ocean mass, kg — derived from the element content, never stored separately.
    pub fn mass_kg(&self) -> f64 {
        self.contents.total()
    }
}

/// The boundary + global reservoirs (spec §4.2). The **mantle is not here** — at
/// M1 it is the per-cell [`MantleField`](crate::mantle::MantleField), because it
/// must convect and differentiate cell by cell. The `core` is global (well-mixed
/// metal): differentiation (M1) sinks Fe/Ni/S into it, outgassing (M3) fills
/// `atmosphere`, the ocean condenses out (M3).
#[derive(Clone, Debug, Default)]
pub struct Reservoirs {
    /// The metallic core — well-mixed, global (empty until differentiation).
    pub core: Composition,
    /// Atmosphere: partial element mass per outgassed / delivered species.
    pub atmosphere: Composition,
    /// The world ocean (aggregated, tracked).
    pub ocean: Ocean,
    /// Running total delivered from the outer system — **adds** to the planet
    /// (a right-hand-side term of the invariant, not a store of present matter).
    pub delivered: Composition,
    /// Mass gone to space — never returns.
    pub escaped: Composition,
}

impl Reservoirs {
    /// Conserved mass of `element` in these reservoirs on the *present* side of the
    /// §4.3 invariant: core + atmosphere + ocean + **escaped**. Escaped mass has
    /// left the planet but is still accounted for (it existed); the per-cell mantle
    /// and the column stack are added by [`World`](crate::planet::World).
    /// `delivered` is deliberately excluded — it is the accreted-plus-delivered
    /// right-hand side, not present matter.
    pub fn conserved_mass(&self, element: ElementId) -> f64 {
        self.core.amount(element)
            + self.atmosphere.amount(element)
            + self.ocean.contents.amount(element)
            + self.escaped.amount(element)
    }
}
