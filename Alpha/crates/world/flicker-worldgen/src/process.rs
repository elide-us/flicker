//! **The per-hex process pipeline — the tick engine.**
//!
//! Each tick iterates every hex in the world; for each hex it runs the same ordered
//! **process collection**. Every process takes the *same input* (a hex's layer column, via
//! [`Ctx`]) and produces the *same output* (a set of [`Effect`]s). A process first **gates**
//! on applicability — "is this appropriate for this hex's current material state?" — then
//! **computes** its effects without mutating anything.
//!
//! The loop is two-pass, so it is order-independent and cross-hex effects settle cleanly:
//! 1. **Compute** — read the frozen current state; every process on every applicable hex
//!    appends its effects to one queue. Nothing changes yet.
//! 2. **Apply** — drain the queue into the state. Mass a process pushed to a *neighbour* lands
//!    now, so next tick that hex has more mass → more pressure → a different temperature. The
//!    cascade is emergent.
//!
//! Effects, not results: a process applies a physical effect (adjust temperature, sink dense
//! metal, freeze a skin, degas a volatile, push buoyant mass poleward); the planet's history
//! *emerges*. No process aims at an outcome.

use flicker_materials::{ElementId, Tables};
use flicker_worldstate::Composition;

use crate::chemistry::{formable_mass, water_element_split};
use crate::cooling::{
    cell_radiogenic_heat, normalized, BASE_COOLING_K, MAX_RADIOGENIC_SLOWDOWN, RADIOGENIC_HALF,
    T_CONDENSE, T_SOLIDUS, T_SPACE,
};
use crate::layer::LayerKind;
use crate::state::HexState;

const H: ElementId = 1;
const O: ElementId = 8;
/// Elements this dense (g/cm³) sink to the core (siderophiles).
const CORE_DENSITY_MIN: f32 = 6.0;

// ── the effect + context types every process shares ──────────────────────────────

/// A change a process wants to make this tick — drained into the state in the apply pass.
#[derive(Clone, Debug)]
pub enum Effect {
    /// Move `amount` element mass from one layer to another (same hex or a neighbour) —
    /// **conserved**: the apply removes only what the source holds and adds exactly that.
    Transfer {
        from_hex: usize,
        from: LayerKind,
        element: ElementId,
        to_hex: usize,
        to: LayerKind,
        amount: f64,
    },
    /// Add external mass to a layer (the authored water veneer — the one non-conserved input).
    Deliver {
        hex: usize,
        kind: LayerKind,
        element: ElementId,
        amount: f64,
    },
    /// Record (`+`) or release (`−`) a compound in a layer — the named-species accounting.
    Compound {
        hex: usize,
        kind: LayerKind,
        compound: u16,
        amount: f64,
    },
    /// Set a hex's temperature (K) — the temperature-adjustment result.
    SetTemperature { hex: usize, kelvin: f32 },
}

/// The read-only view a process computes against: the hex under consideration plus the whole
/// (frozen) world for neighbour reads, the vocabulary, and this tick's per-hex water ration.
pub struct Ctx<'a> {
    pub hex: usize,
    pub cells: &'a [HexState],
    pub neighbors: &'a [Vec<u32>],
    pub tables: &'a Tables,
    /// Water mass delivered to THIS hex this tick (rationed from the planet budget by the caller).
    pub delivery: f64,
}

impl Ctx<'_> {
    fn me(&self) -> &HexState {
        &self.cells[self.hex]
    }
    fn mantle_mass(&self, el: ElementId) -> f64 {
        self.me()
            .column
            .mantle()
            .map(|m| m.composition.amount(el))
            .unwrap_or(0.0)
    }
}

/// A world-gen process: the same shape for all — gate, then compute effects.
pub trait Process {
    fn name(&self) -> &'static str;
    /// Is this process appropriate for the hex's current material state?
    fn applies(&self, ctx: &Ctx) -> bool;
    /// Compute the effects (pure — reads `ctx`, mutates nothing).
    fn compute(&self, ctx: &Ctx, out: &mut Vec<Effect>);
}

// ── the orchestrator (compute all → apply all) ───────────────────────────────────

/// The canonical process collection, in application order.
pub fn processes() -> Vec<Box<dyn Process>> {
    vec![
        Box::new(Temperature),
        Box::new(Convection),
        Box::new(CoreDifferentiation),
        Box::new(CrustFreezing),
        Box::new(Outgassing),
        Box::new(Hydrosphere),
    ]
}

/// **One tick.** Compute every applicable process's effects over the frozen state (pass 1),
/// then apply them all (pass 2). `delivery_total` is the planet's water delivery for this tick,
/// split evenly across the hexes. Returns the total delivered mass (external, for accounting).
pub fn run_tick(
    cells: &mut [HexState],
    neighbors: &[Vec<u32>],
    tables: &Tables,
    procs: &[Box<dyn Process>],
    delivery_total: f64,
) -> f64 {
    if cells.is_empty() {
        return 0.0;
    }
    let per_cell = delivery_total / cells.len() as f64;

    // Pass 1 — compute (read-only over the frozen state).
    let mut effects: Vec<Effect> = Vec::new();
    for hex in 0..cells.len() {
        let ctx = Ctx {
            hex,
            cells,
            neighbors,
            tables,
            delivery: per_cell,
        };
        for p in procs {
            if p.applies(&ctx) {
                p.compute(&ctx, &mut effects);
            }
        }
    }

    // Pass 2 — apply (mutate).
    let mut delivered = 0.0;
    for e in effects {
        delivered += apply(cells, e);
    }
    // Keep the drawn thicknesses consistent with the mass each column now holds.
    for c in cells.iter_mut() {
        c.column.set_thickness_by_mass_share();
    }
    delivered
}

/// Apply one effect to the state; returns any externally-delivered mass (for the ledger).
fn apply(cells: &mut [HexState], e: Effect) -> f64 {
    match e {
        Effect::Transfer {
            from_hex,
            from,
            element,
            to_hex,
            to,
            amount,
        } => {
            let removed = cells[from_hex]
                .column
                .find(from)
                .map(|i| {
                    cells[from_hex].column.layers_mut()[i]
                        .composition
                        .remove(element, amount)
                })
                .unwrap_or(0.0);
            if removed > 0.0 {
                let heat = cells[to_hex].temperature;
                let ti = cells[to_hex].column.ensure(to, heat);
                cells[to_hex].column.layers_mut()[ti]
                    .composition
                    .add(element, removed);
            }
            0.0
        }
        Effect::Deliver {
            hex,
            kind,
            element,
            amount,
        } => {
            if amount <= 0.0 {
                return 0.0;
            }
            let heat = cells[hex].temperature;
            let i = cells[hex].column.ensure(kind, heat);
            cells[hex].column.layers_mut()[i]
                .composition
                .add(element, amount);
            amount
        }
        Effect::Compound {
            hex,
            kind,
            compound,
            amount,
        } => {
            let heat = cells[hex].temperature;
            let idx = if amount >= 0.0 {
                cells[hex].column.ensure(kind, heat)
            } else {
                match cells[hex].column.find(kind) {
                    Some(i) => i,
                    None => return 0.0,
                }
            };
            let ledger = &mut cells[hex].column.layers_mut()[idx].compounds;
            if amount >= 0.0 {
                ledger.add(compound, amount);
            } else {
                ledger.remove(compound, -amount);
            }
            0.0
        }
        Effect::SetTemperature { hex, kelvin } => {
            cells[hex].temperature = kelvin;
            0.0
        }
    }
}

// ── the processes ─────────────────────────────────────────────────────────────────

/// Per-hex **temperature adjustment** — the heat balance. Today: radiative loss toward space,
/// slowed by the hex's own radiogenic (U/K) content, so a radioactive hex holds its heat
/// longer. (The fuller balance — pressure/compression heating from accreted overburden, latent
/// heat of phase change, inter-layer conduction — are further terms on this same effect.)
struct Temperature;

/// Per-hex Newtonian cooling coefficient from its radiogenic content (more U/K ⇒ slower).
fn hex_cooling_k(comp: &Composition) -> f32 {
    let idx = cell_radiogenic_heat(comp);
    let slowdown = MAX_RADIOGENIC_SLOWDOWN * idx / (idx + RADIOGENIC_HALF);
    (BASE_COOLING_K * (1.0 - slowdown)).max(0.0)
}

impl Process for Temperature {
    fn name(&self) -> &'static str {
        "temperature"
    }
    fn applies(&self, _ctx: &Ctx) -> bool {
        true // every hex adjusts its temperature every tick
    }
    fn compute(&self, ctx: &Ctx, out: &mut Vec<Effect>) {
        let hex = ctx.me();
        // The radiogenic brake reads the hex's LIVE column (every stratum merged), not the
        // frozen flat `composition` mirror. So K/U that has convected away to a neighbour
        // (the hex holds less → cools faster) or sunk to the core (relocated *within* the
        // hex → same total → cooling unchanged) governs the rate correctly. Sourcing this
        // from the conserved column is the one truth; the flat mirror is no longer read.
        let k = hex_cooling_k(&hex.column.total_composition());
        let next = hex.temperature - k * (hex.temperature - T_SPACE);
        out.push(Effect::SetTemperature {
            hex: ctx.hex,
            kelvin: next,
        });
    }
}

/// **Core differentiation** — while the mantle is molten, its siderophile (dense) elements sink
/// to the core. Rate scales with how molten the hex is.
struct CoreDifferentiation;

impl Process for CoreDifferentiation {
    fn name(&self) -> &'static str {
        "core-differentiation"
    }
    fn applies(&self, ctx: &Ctx) -> bool {
        // Molten, with a mantle that still holds something dense.
        ctx.me().temperature > T_SOLIDUS
            && ctx.me().column.mantle().is_some_and(|m| {
                m.composition.iter().any(|(el, a)| {
                    a > 0.0
                        && ctx
                            .tables
                            .element_by_number(el)
                            .is_some_and(|e| e.density_g_cm3 >= CORE_DENSITY_MIN)
                })
            })
    }
    fn compute(&self, ctx: &Ctx, out: &mut Vec<Effect>) {
        let Some(mantle) = ctx.me().column.mantle() else {
            return;
        };
        let rate = 0.08 * normalized(ctx.me().temperature) as f64;
        for (el, amount) in mantle.composition.iter() {
            let dense = ctx
                .tables
                .element_by_number(el)
                .is_some_and(|e| e.density_g_cm3 >= CORE_DENSITY_MIN);
            if dense && amount > 0.0 {
                out.push(Effect::Transfer {
                    from_hex: ctx.hex,
                    from: LayerKind::Mantle,
                    element: el,
                    to_hex: ctx.hex,
                    to: LayerKind::Core,
                    amount: amount * rate,
                });
            }
        }
    }
}

/// Felsic melt affinity per element — the fraction drawn up into the freezing crust.
fn crust_affinity(e: ElementId) -> f64 {
    match e {
        8 => 0.55,  // O
        14 => 0.70, // Si
        13 => 0.70, // Al
        11 => 0.90, // Na
        19 => 0.95, // K
        3 => 0.92,  // Li
        20 => 0.30, // Ca
        22 => 0.30, // Ti
        12 => 0.08, // Mg — stays
        26 => 0.15, // Fe — stays
        _ => 0.30,
    }
}

/// **Crust freezing** — once the hex has cooled below the silicate solidus, felsic melt freezes
/// out of the mantle into a crust skin, saturating to a thin layer.
struct CrustFreezing;
const CRUST_GEN_RATE: f64 = 0.03;
const CRUST_SAT_FRAC: f64 = 0.01;

impl Process for CrustFreezing {
    fn name(&self) -> &'static str {
        "crust-freezing"
    }
    fn applies(&self, ctx: &Ctx) -> bool {
        ctx.me().temperature < T_SOLIDUS && ctx.me().column.mantle().is_some()
    }
    fn compute(&self, ctx: &Ctx, out: &mut Vec<Effect>) {
        let col = &ctx.me().column;
        let total = col.total_mass();
        if total <= 0.0 {
            return;
        }
        let crust_mass = col
            .find(LayerKind::Crust)
            .map(|i| col.layers()[i].mass())
            .unwrap_or(0.0);
        let fill = (1.0 - crust_mass / (CRUST_SAT_FRAC * total)).max(0.0);
        if fill <= 0.0 {
            return;
        }
        let step = (CRUST_GEN_RATE * fill).clamp(0.0, 1.0);
        let Some(mantle) = col.mantle() else {
            return;
        };
        for (el, amount) in mantle.composition.iter() {
            let want = step * crust_affinity(el) * amount;
            if want > 0.0 {
                out.push(Effect::Transfer {
                    from_hex: ctx.hex,
                    from: LayerKind::Mantle,
                    element: el,
                    to_hex: ctx.hex,
                    to: LayerKind::Crust,
                    amount: want,
                });
            }
        }
    }
}

/// One outgassing gas species + its release-temperature floor (K); hotter species escape first.
struct GasSpecies {
    compound: &'static str,
    floor: f32,
}
const OUTGAS_SERIES: [GasSpecies; 5] = [
    GasSpecies {
        compound: "Sulfur Dioxide",
        floor: 1050.0,
    },
    GasSpecies {
        compound: "Water",
        floor: 800.0,
    },
    GasSpecies {
        compound: "Carbon Dioxide",
        floor: 670.0,
    },
    GasSpecies {
        compound: "Hydrogen Chloride",
        floor: 570.0,
    },
    GasSpecies {
        compound: "Nitrogen",
        floor: 100.0,
    },
];
const OUTGAS_RATE: f64 = 0.03;

/// **Outgassing** — the interior degasses volatiles into the atmosphere as real gas compounds,
/// distilling by temperature (hot species early → N₂ residual). Conserved mantle → atmosphere.
struct Outgassing;

impl Process for Outgassing {
    fn name(&self) -> &'static str {
        "outgassing"
    }
    fn applies(&self, ctx: &Ctx) -> bool {
        ctx.me().column.mantle().is_some()
            && OUTGAS_SERIES
                .iter()
                .any(|g| ctx.me().temperature >= g.floor)
    }
    fn compute(&self, ctx: &Ctx, out: &mut Vec<Effect>) {
        let temp = ctx.me().temperature;
        let vigor = normalized(temp) as f64;
        for gas in &OUTGAS_SERIES {
            if temp < gas.floor {
                continue;
            }
            let Some(compound) = ctx.tables.compound(gas.compound) else {
                continue;
            };
            let fractions = ctx.tables.compound_mass_fractions(compound);
            if fractions.is_empty() {
                continue;
            }
            let formable = formable_mass(|el| ctx.mantle_mass(el), &fractions);
            let amount = formable * OUTGAS_RATE * vigor;
            if amount <= 0.0 {
                continue;
            }
            for &(el, fr) in &fractions {
                out.push(Effect::Transfer {
                    from_hex: ctx.hex,
                    from: LayerKind::Mantle,
                    element: el,
                    to_hex: ctx.hex,
                    to: LayerKind::Atmosphere,
                    amount: amount * fr,
                });
            }
            out.push(Effect::Compound {
                hex: ctx.hex,
                kind: LayerKind::Atmosphere,
                compound: compound.id,
                amount,
            });
        }
    }
}

/// **Hydrosphere** — delivered water enters as vapour or liquid by the surface conditions, and
/// water already present changes phase across the condensation point (only the water moves; the
/// other gases stay in the air). Water is tracked as both its H/O elements and the H₂O compound.
struct Hydrosphere;
const PHASE_RATE: f64 = 0.2;

impl Process for Hydrosphere {
    fn name(&self) -> &'static str {
        "hydrosphere"
    }
    fn applies(&self, ctx: &Ctx) -> bool {
        // Applies if water is arriving, or any is present to change phase.
        ctx.delivery > 0.0
            || ctx.me().column.find(LayerKind::Ocean).is_some()
            || ctx.me().column.find(LayerKind::Atmosphere).is_some()
    }
    fn compute(&self, ctx: &Ctx, out: &mut Vec<Effect>) {
        let Some((water_id, hf, of)) = water_element_split(ctx.tables) else {
            return;
        };
        let temp = ctx.me().temperature;

        // Newly delivered water joins whichever phase the surface supports.
        if ctx.delivery > 0.0 {
            let kind = if temp < T_CONDENSE {
                LayerKind::Ocean
            } else {
                LayerKind::Atmosphere
            };
            out.push(Effect::Deliver {
                hex: ctx.hex,
                kind,
                element: H,
                amount: ctx.delivery * hf,
            });
            out.push(Effect::Deliver {
                hex: ctx.hex,
                kind,
                element: O,
                amount: ctx.delivery * of,
            });
            out.push(Effect::Compound {
                hex: ctx.hex,
                kind,
                compound: water_id,
                amount: ctx.delivery,
            });
        }

        // Water already present changes phase across the condensation point.
        let (from, to) = if temp < T_CONDENSE {
            (LayerKind::Atmosphere, LayerKind::Ocean) // condensation
        } else {
            (LayerKind::Ocean, LayerKind::Atmosphere) // evaporation
        };
        if let Some(i) = ctx.me().column.find(from) {
            let water = ctx.me().column.layers()[i].compounds.amount(water_id);
            let move_w = water * PHASE_RATE;
            if move_w > 0.0 {
                for (el, frac) in [(H, hf), (O, of)] {
                    out.push(Effect::Transfer {
                        from_hex: ctx.hex,
                        from,
                        element: el,
                        to_hex: ctx.hex,
                        to,
                        amount: move_w * frac,
                    });
                }
                out.push(Effect::Compound {
                    hex: ctx.hex,
                    kind: from,
                    compound: water_id,
                    amount: -move_w,
                });
                out.push(Effect::Compound {
                    hex: ctx.hex,
                    kind: to,
                    compound: water_id,
                    amount: move_w,
                });
            }
        }
    }
}

/// **Convection** — a molten hex is buoyant and stirs: it pushes a slice of its mantle mass
/// toward its **coldest neighbour** (a cross-hex transfer). Over many ticks this moves real
/// material around the planet; the receiving hex gains mass (→ pressure → temperature next
/// tick). Only fires while molten and only downhill in temperature, so it settles as the
/// planet cools.
struct Convection;
const CONVECT_FRAC: f64 = 0.02;

impl Process for Convection {
    fn name(&self) -> &'static str {
        "convection"
    }
    fn applies(&self, ctx: &Ctx) -> bool {
        ctx.me().temperature > T_SOLIDUS && ctx.me().column.mantle().is_some()
    }
    fn compute(&self, ctx: &Ctx, out: &mut Vec<Effect>) {
        // Find the coldest neighbour; push mantle mass toward it if it is colder than us.
        let my_t = ctx.me().temperature;
        let mut coldest: Option<(usize, f32)> = None;
        for &nb in &ctx.neighbors[ctx.hex] {
            let t = ctx.cells[nb as usize].temperature;
            if coldest.map(|(_, ct)| t < ct).unwrap_or(true) {
                coldest = Some((nb as usize, t));
            }
        }
        let Some((to_hex, to_t)) = coldest else {
            return;
        };
        if to_t >= my_t {
            return; // nothing colder to sink toward
        }
        let Some(mantle) = ctx.me().column.mantle() else {
            return;
        };
        // A slice of the mantle, scaled by how molten we are and the temperature contrast.
        let contrast = ((my_t - to_t) / my_t).clamp(0.0, 1.0) as f64;
        let rate = CONVECT_FRAC * normalized(my_t) as f64 * contrast;
        for (el, amount) in mantle.composition.iter() {
            if amount > 0.0 {
                out.push(Effect::Transfer {
                    from_hex: ctx.hex,
                    from: LayerKind::Mantle,
                    element: el,
                    to_hex,
                    to: LayerKind::Mantle,
                    amount: amount * rate,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldstate::Composition;

    fn tables() -> Tables {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// A tiny ring of `n` hexes, each seeded molten with the given bulk, wired as a cycle.
    fn world(n: usize, bulk: Composition, temp: f32) -> (Vec<HexState>, Vec<Vec<u32>>) {
        let cells: Vec<HexState> = (0..n)
            .map(|_| {
                let mut h = HexState::new(bulk.clone());
                h.temperature = temp;
                h
            })
            .collect();
        let neighbors: Vec<Vec<u32>> = (0..n)
            .map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32])
            .collect();
        (cells, neighbors)
    }

    /// The load-bearing invariant: the two-pass loop conserves element mass (delivery aside).
    #[test]
    fn the_tick_conserves_element_mass() {
        let t = tables();
        let bulk =
            Composition::from_iter([(26u8, 3000.0), (14u8, 1500.0), (8u8, 3000.0), (6u8, 200.0)]);
        let (mut cells, neighbors) = world(7, bulk, 1900.0);
        let procs = processes();
        let mass = |c: &[HexState]| -> f64 { c.iter().map(|h| h.column.total_mass()).sum() };
        let before = mass(&cells);
        let mut delivered = 0.0;
        for _ in 0..30 {
            // Cool the world a touch each tick by hand so phases are exercised, and deliver a
            // little water — the only external mass.
            delivered += run_tick(&mut cells, &neighbors, &t, &procs, 50.0);
        }
        let after = mass(&cells);
        assert!(
            (after - (before + delivered)).abs() <= 1e-6 * (before + delivered),
            "element mass conserved modulo delivery: {before} + {delivered} vs {after}"
        );
    }

    /// Finding-1 fix: the cooling brake reads the hex's LIVE column, not the frozen flat
    /// `composition` mirror. Radiogenic mass that migrates out of the column must speed the
    /// hex's cooling, while the stale mirror would never notice it leave.
    #[test]
    fn cooling_reads_the_live_column_not_the_frozen_flat_composition() {
        // Si (inert) + K (a radiogenic cooling brake, Z=19).
        let bulk = Composition::from_iter([(14u8, 1000.0), (19u8, 200.0)]);
        let mut h = HexState::new(bulk);
        let k_before = hex_cooling_k(&h.column.total_composition());
        // Drain the potassium from the column, as a cross-hex convection export would,
        // leaving the frozen flat `composition` mirror untouched.
        let mi = h
            .column
            .find(LayerKind::Mantle)
            .expect("primordial mantle present");
        let removed = h.column.layers_mut()[mi].composition.remove(19u8, 200.0);
        assert_eq!(removed, 200.0, "the K actually left the column");
        let k_after = hex_cooling_k(&h.column.total_composition());
        assert!(
            k_after > k_before,
            "losing radiogenic K must cool faster ({k_after} > {k_before})"
        );
        // The stale flat mirror still shows the K — the OLD read would report the slow rate.
        assert_eq!(
            hex_cooling_k(&h.composition),
            k_before,
            "the flat mirror never noticed the K leave"
        );
    }

    /// Cross-hex effects actually move mass: a hot hex next to a cold one sheds mantle into it.
    #[test]
    fn convection_moves_mass_between_hexes() {
        let t = tables();
        let bulk = Composition::from_iter([(14u8, 1000.0), (8u8, 1000.0)]);
        let (mut cells, neighbors) = world(3, bulk, 1900.0);
        cells[1].temperature = 900.0; // a cold neighbour to sink toward
        let procs = processes();
        let before_cold = cells[1].column.total_mass();
        run_tick(&mut cells, &neighbors, &t, &procs, 0.0);
        assert!(
            cells[1].column.total_mass() > before_cold,
            "the cold hex received convected mantle mass from its hot neighbours"
        );
    }
}
