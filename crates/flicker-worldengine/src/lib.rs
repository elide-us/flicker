//! flicker-worldengine — the **forward-regenerative world-generation engine**.
//!
//! The GPU-free facade over the [`flicker_worldgen`] physics kernels: it turns a
//! bulk-composition recipe + a seed + per-epoch levers into a nine-epoch planet,
//! evolving it forward one immutable [`EpochSnapshot`] at a time and caching each
//! so the timeline can scrub and "play god" edits regenerate only forward. It is
//! the planet-generation analogue of `flicker-system` (the star-system sim);
//! a viewer drives it and renders the snapshots, and a caller growing the "seven
//! Home worlds" is just seven [`WorldEngine`]s with different recipes.
//!
//! See `docs/flicker-world-epoch-redesign.md` for the full design.
//!
//! ```no_run
//! use flicker_worldengine::WorldEngine;
//! let mut engine = WorldEngine::from_repo().unwrap();  // Earth-like defaults
//! engine.set_freq(6);
//! let hydrosphere = engine.snapshot(4);                // computes epochs 1..=4
//! println!("cells: {}", hydrosphere.len());
//! engine.set_lever("e3_mountain_uplift", 1.2);         // invalidates epochs 3..=9
//! let world = engine.capture("my world");              // a .epoch, ready to save
//! # let _ = world;
//! ```

pub mod config;
pub mod engine;
pub mod epochfile;
pub mod habitability;
pub mod levers;
pub mod nodes;
pub mod sim;
pub mod snapshot;

pub use config::{
    build_epoch1, build_transforms, mutate_epoch, next_seed, seed_chain, WorldConfig, DEFAULT_FREQ,
    DEFAULT_SEED, MAX_FREQ, MIN_FREQ, WORLD_EPOCHS,
};
pub use engine::WorldEngine;
pub use epochfile::{EpochFile, EpochFileError, EPOCH_FORMAT, EPOCH_VERSION};
pub use habitability::{observe, Axis, Habitability};
pub use levers::{
    repo_content_dir, AbundanceDef, GeneratorError, GeneratorParams, GeneratorParamsSource,
    JsonGeneratorSource, LeverDef,
};
pub use sim::{Simulation, World, MY_PER_TICK};
pub use snapshot::{masses_agree, EpochSnapshot, Provenance};

// Re-export the per-cell state the public API exposes (`World.cells`, `EpochSnapshot.cells`)
// so viewers can name it without depending on `flicker-worldgen` directly.
pub use flicker_worldgen::{
    classify, HexState, Layer, LayerClass, LayerKind, LayerLedger, Phase,
};
// Re-export the material vocabulary type the viewer needs to name for classification reads.
pub use flicker_materials::Tables;
// Re-export the thermal helpers (Kelvin ↔ normalized) the viewer reads for the heat view.
pub use flicker_worldgen::cooling;

#[cfg(test)]
mod tests {
    use super::*;

    /// A cheap engine for tests (freq 6 ≈ 362 cells).
    fn engine() -> WorldEngine {
        let mut e = WorldEngine::from_repo().expect("engine from repo content");
        e.set_freq(6);
        e
    }

    #[test]
    fn bulk_mass_is_conserved_modulo_water_delivery() {
        let mut e = engine();
        let seed_mass = e.snapshot(1).conserved_mass();
        assert!(seed_mass > 0.0, "seed layer has no mass");
        // Epochs 1-3 form compounds without adding element mass — exactly conserved.
        for epoch in 1..=3 {
            let m = e.snapshot(epoch).conserved_mass();
            assert!(masses_agree(m, seed_mass, 1e-9), "epoch {epoch} drifted pre-delivery ({m} vs {seed_mass})");
        }
        // Epoch 4 delivers water from the outer system — element mass jumps up.
        let e4 = e.snapshot(4).conserved_mass();
        assert!(e4 > seed_mass, "E4 water delivery should add mass ({e4} vs {seed_mass})");
        // Epochs 4-9: no further additions or losses — conserved among themselves.
        for epoch in 4..=WORLD_EPOCHS {
            let m = e.snapshot(epoch).conserved_mass();
            assert!(masses_agree(m, e4, 1e-9), "epoch {epoch} drifted post-delivery ({m} vs {e4})");
        }
    }

    #[test]
    fn compounds_form_and_stay_bounded_by_the_elements() {
        use flicker_worldgen::locked_element_mass;
        let mut e = engine();
        e.snapshot(WORLD_EPOCHS); // realise the chain
        let tables = e.tables();
        let snap = e.peek(6).expect("epoch 6 computed");
        // Real chemistry happened.
        assert!(snap.cells.iter().any(|c| !c.compounds.is_empty()), "no compounds formed");
        // Water is present as a compound (delivered + accounted).
        let water_id = tables.compound("Water").expect("Water compound").id;
        assert!(snap.cells.iter().any(|c| c.compounds.amount(water_id) > 0.0), "no water compound");
        // The second-ledger invariant: no cell locks more of any element into
        // compounds than its element ledger holds.
        for c in &snap.cells {
            for (el, m) in locked_element_mass(c, tables) {
                assert!(
                    m <= c.composition.amount(el) + 1e-6,
                    "element {el} over-locked: {m} > {}",
                    c.composition.amount(el)
                );
            }
        }
    }

    #[test]
    fn editing_a_late_lever_freezes_earlier_epochs() {
        let mut e = engine();
        // Materialise the whole chain, then snapshot Epoch 2 and Epoch 6.
        let e2_before = e.snapshot(2).clone();
        let e6_before = e.snapshot(6).clone();

        // Edit an Epoch-6 lever: epochs 1..=5 stay frozen (still cached), 6..=9 drop.
        e.set_lever("e6_erosion_rate", 0.05);
        assert!(e.peek(2).is_some(), "epoch 2 should stay cached after a late edit");
        assert!(e.peek(6).is_none(), "epoch 6 should be invalidated by its own lever");

        let e2_after = e.snapshot(2).clone();
        let e6_after = e.snapshot(6).clone();
        assert_eq!(e2_before.cells, e2_after.cells, "an E6 edit changed E2");
        assert_ne!(e6_before.cells, e6_after.cells, "the E6 edit didn't change E6");
    }

    #[test]
    fn editing_an_early_lever_invalidates_forward() {
        let mut e = engine();
        e.snapshot(WORLD_EPOCHS); // fill everything
        e.set_lever("e3_plates", 3.0);
        assert!(e.peek(2).is_some(), "epoch 2 (before the edit) stays cached");
        assert!(e.peek(3).is_none(), "epoch 3 invalidated");
        assert!(e.peek(9).is_none(), "epoch 9 (after the edit) invalidated");
    }

    #[test]
    fn reseeding_an_epoch_leaves_upstream_identical() {
        let mut e = engine();
        let e2 = e.snapshot(2).clone();
        let e3_before = e.snapshot(3).clone();
        e.reseed(3);
        assert_eq!(e2.cells, e.snapshot(2).cells, "an E3 reseed changed E2");
        assert_ne!(e3_before.cells, e.snapshot(3).cells, "the E3 reseed didn't change E3");
    }

    #[test]
    fn reseeding_epoch3_keeps_the_material_derived_plate_layout() {
        // The Epoch-3 plate partition is a pure function of the frozen Epoch-2 crust,
        // so reseeding Epoch 3 must NOT reshuffle the plates — only the deformation
        // magnitudes (uplift/rift/land-share) jitter, never the layout.
        let mut e = engine();
        let before: Vec<u16> = e.snapshot(3).cells.iter().map(|c| c.plate).collect();
        e.reseed(3);
        let after: Vec<u16> = e.snapshot(3).cells.iter().map(|c| c.plate).collect();
        assert_eq!(before, after, "reseeding Epoch 3 reshuffled the material-derived plate layout");
    }

    #[test]
    fn every_harvestable_ore_forms_somewhere() {
        use std::collections::HashSet;
        let mut e = engine();
        // Formation is complete at E6; the ore-vein guarantee ran there.
        let world = e.snapshot(6);
        // Ores concentrated to a mineable seam anywhere (≥ the guarantee's MIN).
        let mut mined: HashSet<u16> = HashSet::new();
        for c in &world.cells {
            for (cid, amount) in c.compounds.iter() {
                if amount >= 1.0 {
                    mined.insert(cid);
                }
            }
        }
        for ore in e.tables().harvestable_compounds() {
            // Only ores with parseable constituents can be formed (Garnet/Turquoise
            // carry no formula and aren't marked harvestable).
            if e.tables().compound_mass_fractions(ore).is_empty() {
                continue;
            }
            assert!(
                mined.contains(&ore.id),
                "harvestable ore {} reaches no mineable vein in the world",
                ore.name
            );
        }
        // And the wrong rule is gone: a non-harvestable element (hydrogen) gets no
        // silly forced vein.
        assert!(
            !e.tables().harvestable_compounds().any(|c| c.name == "Water"),
            "water should not be a mineable ore"
        );
    }

    #[test]
    fn the_cooling_clock_spans_every_epoch_as_one_axis() {
        let mut e = engine();
        // Epoch 1 is the molten seed — it advances no cooling steps; the clock starts at E2.
        assert_eq!(e.epoch_cool_steps(1), 0, "the seed layer advances no cooling steps");
        // Per-epoch boundaries are cumulative and strictly increasing across E2..=E9, and
        // each epoch's start is the previous epoch's end — one contiguous axis.
        let mut prev = e.cool_step_before(2);
        for ep in 3..=WORLD_EPOCHS {
            let b = e.cool_step_before(ep);
            assert!(b > prev, "cooling boundary must advance at E{ep} ({b} vs {prev})");
            assert_eq!(b, e.cool_step_end(ep - 1), "start of E{ep} == end of E{}", ep - 1);
            prev = b;
        }
        assert_eq!(e.cooling_total_steps(), e.cool_step_end(WORLD_EPOCHS), "span == end of E9");
        // The tectonics onset lands within the molten+tectonic span (default: at the boundary).
        if let Some(onset) = e.tectonics_onset_step() {
            assert!(onset <= e.cool_step_before(4), "tectonics onset sits in the molten+tectonic span");
        }
    }

    #[test]
    fn epochs_seven_to_nine_are_present_stubs() {
        let mut e = engine();
        let e6 = e.snapshot(6).clone();
        for epoch in 7..=WORLD_EPOCHS {
            let s = e.snapshot(epoch);
            assert_eq!(s.epoch as usize, epoch);
            assert_eq!(s.len(), e6.len(), "stub epoch {epoch} must span the planet");
        }
    }
}
