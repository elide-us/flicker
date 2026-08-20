//! The **surface classifier** — the "(ledger) → drawn material" read of
//! unification ruling R7, v1 (materials-unification plan P2, 2026-08-19).
//!
//! A container's TRUTH is its ledger (compounds + elements, conserved); the
//! drawn material is a **derived narrowing** onto the ≤255-slot catalog — the
//! one face a voxel renders as. This read is pure and encodes no causal rule:
//! the sim (or the player) changes the ledger, and the caller re-runs this to
//! see what the container now *looks like* (extract the silicon from sand and
//! the surface re-classifies — nothing here decides that it should).
//!
//! v1 keys, in order:
//! 1. **Dominant compound** (by mass) → the material whose `represents`
//!    column claims it ([`Tables::material_representing`]); resolved through
//!    the compound catalog, uniqueness-gated at load.
//! 2. **Element-signature fallback** — the best `materials.json` `signature`
//!    match over the elements present (ported from the superseded
//!    `flicker-worldgen::classify::match_material`): the whole signature must
//!    be present; the most specific (longest) signature wins; the lower id
//!    breaks ties deterministically.
//! 3. Nothing matches (or the ledger is empty) → `None` — the mesh palette
//!    draws unclassified ground loud-magenta rather than guessing.
//!
//! CONDITIONS (temperature/pressure — ice vs water, molten vs solid) are the
//! R7 registry's v2 hook and deliberately NOT consulted here; the phase read
//! lives with the sim's condition fields, not in this composition-only pick.

use flicker_materials::{MaterialDef, MaterialId, Tables};

use crate::composition::Composition;
use crate::compound_ledger::CompoundLedger;

/// Classify a container's ledger to the catalog material that best represents
/// it — the drawn face. Pure; recompute after any ledger change.
pub fn classify_material(
    comp: &Composition,
    compounds: &CompoundLedger,
    tables: &Tables,
) -> Option<MaterialId> {
    // 1. The dominant formed compound names the material outright.
    if let Some(id) = compounds.dominant() {
        if let Some(def) = tables.compound_by_id(id) {
            if let Some(m) = tables.material_representing(&def.name) {
                return Some(m.id);
            }
        }
    }
    // 2. Fall back to the element-signature match over what is present.
    signature_match(comp, tables).map(|m| m.id)
}

/// Best-matching material by element signature: every signature element must
/// be present in `comp`; prefer the most specific (longest) signature; break
/// ties on the lower id. `None` when nothing fully matches.
fn signature_match<'t>(comp: &Composition, tables: &'t Tables) -> Option<&'t MaterialDef> {
    let present: Vec<&str> = comp
        .iter()
        .filter(|(_, a)| *a > 0.0)
        .filter_map(|(el, _)| tables.element_by_number(el).map(|e| e.symbol.as_str()))
        .collect();
    if present.is_empty() {
        return None;
    }
    let mut best: Option<&MaterialDef> = None;
    for m in tables.materials() {
        if m.signature.is_empty() {
            continue;
        }
        let full = m.signature.iter().all(|s| present.contains(&s.as_str()));
        if !full {
            continue;
        }
        let better = match best {
            None => true,
            Some(b) => {
                m.signature.len() > b.signature.len()
                    || (m.signature.len() == b.signature.len() && m.id < b.id)
            }
        };
        if better {
            best = Some(m);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::JsonTableSource;

    const SI: u8 = 14;
    const O: u8 = 8;
    const FE: u8 = 26;
    const AL: u8 = 13;
    const K: u8 = 19;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    fn mat_id(t: &Tables, name: &str) -> MaterialId {
        t.material_by_name(name).unwrap().id
    }

    /// The primary key: a dominant compound resolves through `represents` —
    /// including synonyms (Iron Oxide and Hematite both draw as Hematite).
    #[test]
    fn dominant_compound_picks_the_representing_material() {
        let t = tables();
        let coal = t.compound("Coal").unwrap().id;
        let hematite = t.compound("Hematite").unwrap().id;
        let iron_oxide = t.compound("Iron Oxide").unwrap().id;
        let water = t.compound("Water").unwrap().id;

        let mut led = CompoundLedger::new();
        led.add(coal, 900.0);
        led.add(hematite, 100.0);
        let comp = Composition::new();
        assert_eq!(
            classify_material(&comp, &led, &t),
            Some(mat_id(&t, "Coal")),
            "mostly coal draws as Coal"
        );

        let mut led = CompoundLedger::new();
        led.add(iron_oxide, 500.0);
        assert_eq!(
            classify_material(&comp, &led, &t),
            Some(mat_id(&t, "Hematite")),
            "the Iron Oxide synonym draws as Hematite"
        );

        let mut led = CompoundLedger::new();
        led.add(water, 1.0);
        assert_eq!(
            classify_material(&comp, &led, &t),
            Some(mat_id(&t, "Water")),
            "a water-dominated container draws as Water (slot kept by ruling)"
        );
    }

    /// The ledger is live: extract the dominant compound and the pick follows
    /// what remains — Aaron's sand→silicon shape, run on defined materials.
    #[test]
    fn reclassifying_after_extraction_follows_the_ledger() {
        let t = tables();
        let quartz = t.compound("Quartz").unwrap().id;
        let hematite = t.compound("Hematite").unwrap().id;
        let mut led = CompoundLedger::new();
        led.add(quartz, 800.0);
        led.add(hematite, 300.0);
        let comp = Composition::new();
        assert_eq!(
            classify_material(&comp, &led, &t),
            Some(mat_id(&t, "Sand")),
            "quartz-dominant draws as Sand"
        );
        // Extract the quartz; hematite now dominates the same container.
        led.remove(quartz, 700.0);
        assert_eq!(
            classify_material(&comp, &led, &t),
            Some(mat_id(&t, "Hematite")),
            "after extraction the SAME container re-classifies"
        );
    }

    /// A dominant compound nothing represents (a mantle mineral) falls back to
    /// the element signature of the composition.
    #[test]
    fn unrepresented_compound_falls_back_to_signature() {
        let t = tables();
        let olivine = t.compound("Olivine").unwrap().id;
        let mut led = CompoundLedger::new();
        led.add(olivine, 100.0);
        // Bedrock's full signature (Si O Fe Al) is present and is the most
        // specific full match for this composition.
        let comp = Composition::from_iter([(SI, 500.0), (O, 400.0), (FE, 80.0), (AL, 20.0)]);
        assert_eq!(
            classify_material(&comp, &led, &t),
            Some(mat_id(&t, "Bedrock")),
            "no representing material → signature fallback"
        );
    }

    /// Signature specificity and the deterministic tie-break: Si+O alone
    /// matches both Sandstone (12) and Sand (22) in full — the lower id wins;
    /// adding Al+K makes Granite (4 elements) the more specific pick.
    #[test]
    fn signature_prefers_specific_then_lower_id() {
        let t = tables();
        let led = CompoundLedger::new();
        let si_o = Composition::from_iter([(SI, 800.0), (O, 200.0)]);
        assert_eq!(
            classify_material(&si_o, &led, &t),
            Some(mat_id(&t, "Sandstone")),
            "equal-length full matches tie-break to the lower id"
        );
        let granite = Composition::from_iter([(SI, 600.0), (O, 300.0), (AL, 60.0), (K, 40.0)]);
        assert_eq!(
            classify_material(&granite, &led, &t),
            Some(mat_id(&t, "Granite")),
            "the longest fully-present signature wins"
        );
    }

    /// Empty ledgers classify to None — unclassified ground draws loud-wrong,
    /// it is never guessed.
    #[test]
    fn empty_ledger_is_unclassified() {
        let t = tables();
        assert_eq!(
            classify_material(&Composition::new(), &CompoundLedger::new(), &t),
            None
        );
    }
}
