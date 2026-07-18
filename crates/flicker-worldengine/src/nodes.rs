//! The **harvestable-ore vein guarantee**.
//!
//! Not every element needs a node — that would put a silly "hydrogen vein" on the
//! map. A voxel is a *container* of elements + compounds + water, and most
//! materials are **refined diffusely** from bulk voxels wherever they're
//! concentrated (a high-Si region yields Si when you mine and refine it — along
//! with everything else in the container; matter is conserved). What genuinely
//! needs a **vein** is the curated set of **mineable ores/gems** — the
//! send-minions-to-mine-it ore bodies (Hematite, Chalcopyrite, Native Gold,
//! Diamond, …), flagged `harvestable` in the compound catalog.
//!
//! Those ores are **compounds**, and the compound former ([`flicker_worldgen`]'s
//! chemistry pass) already concentrates each at the epoch whose mechanism causes
//! it — sulfides + natives on the hydrothermal veins (E5), carbonates in warm seas
//! and organics as coal (E6), salts as evaporites (E4) — conserved, formed *from*
//! the cell's free elements and capped so the element mass locked in compounds
//! never exceeds the ledger.
//!
//! [`ensure_ore_veins`] is the gameplay backstop over that physics: after all the
//! formation epochs, any harvestable ore that never reached a mineable
//! concentration anywhere is **force-formed** at the cell best able to make it
//! (its scarcest constituent richest) — a small, conserved seam, marked as a vein.
//! Deterministic; only the compound ledger + `vein_*` markers change, and the
//! conserved element ledger is never exceeded.

use std::collections::HashSet;

use flicker_materials::{ElementId, Tables};
use flicker_worldgen::{locked_element_mass, HexState};

/// Vein strength stamped on a force-formed ore seam (a modest, harvestable
/// concentration; a natural vein that's already stronger is left alone).
const FORCED_NODE_STRENGTH: f32 = 0.5;
/// A compound counts as "concentrated into a mineable vein" at/above this mass.
const MIN_ORE_MASS: f64 = 1.0;
/// Target mass of a force-formed seam (capped by what the cell can actually form).
const FORCED_ORE_MASS: f64 = 50.0;

/// Guarantee every **harvestable** ore in `tables` reaches a mineable vein somewhere.
/// Returns how many veins had to be forced (0 once the epoch physics already covers
/// the harvestable catalog).
pub fn ensure_ore_veins(cells: &mut [HexState], tables: &Tables) -> usize {
    // Ores already concentrated to a mineable seam anywhere (formed by the physics).
    let mut present: HashSet<u16> = HashSet::new();
    for c in cells.iter() {
        for (cid, amount) in c.compounds.iter() {
            if amount >= MIN_ORE_MASS {
                present.insert(cid);
            }
        }
    }

    let mut forced = 0;
    for ore in tables.harvestable_compounds() {
        if present.contains(&ore.id) {
            continue;
        }
        let fractions = tables.compound_mass_fractions(ore);
        if fractions.is_empty() {
            continue; // no parseable constituents — can't be formed from elements
        }
        // Place the seam where the ore can form most: the cell whose stoichiometric
        // ceiling (least-available free constituent ÷ its fraction) is highest.
        let mut best: Option<(usize, f64)> = None;
        for (i, c) in cells.iter().enumerate() {
            let locked = locked_element_mass(c, tables);
            let mut ceiling = f64::INFINITY;
            for &(el, fr) in &fractions {
                if fr > 0.0 {
                    let free =
                        (c.composition.amount(el) - locked.get(&el).copied().unwrap_or(0.0)).max(0.0);
                    ceiling = ceiling.min(free / fr);
                }
            }
            if ceiling.is_finite() && ceiling > best.map(|(_, v)| v).unwrap_or(0.0) {
                best = Some((i, ceiling));
            }
        }
        let Some((i, ceiling)) = best else { continue };
        let amount = ceiling.min(FORCED_ORE_MASS);
        if amount < MIN_ORE_MASS {
            continue; // the world genuinely can't form even a trace — skip
        }
        // Conserved: the former's cap logic guarantees `amount` fits the free
        // elements; adding to the compound ledger locks that element mass.
        cells[i].compounds.add(ore.id, amount);
        if let Some(metal) = vein_marker(ore.extracted(), &fractions, tables) {
            if FORCED_NODE_STRENGTH >= cells[i].vein_strength {
                cells[i].vein_element = Some(metal);
                cells[i].vein_strength = cells[i].vein_strength.max(FORCED_NODE_STRENGTH);
            }
        }
        forced += 1;
    }
    forced
}

/// The element to mark a vein hex with: the ore's extracted target if it names one,
/// otherwise its densest constituent (the "metal" of the seam).
fn vein_marker(
    extracted: Option<&str>,
    fractions: &[(ElementId, f64)],
    tables: &Tables,
) -> Option<ElementId> {
    if let Some(sym) = extracted {
        if let Some(e) = tables.element(sym) {
            return Some(e.number);
        }
    }
    fractions.iter().map(|&(el, _)| el).max_by(|&a, &b| {
        let da = tables.element_by_number(a).map(|e| e.density_g_cm3).unwrap_or(0.0);
        let db = tables.element_by_number(b).map(|e| e.density_g_cm3).unwrap_or(0.0);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    })
}
