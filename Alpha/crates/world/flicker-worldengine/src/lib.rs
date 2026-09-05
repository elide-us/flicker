//! flicker-worldengine — **the planet's home**: where a world is simulated, and
//! the format it is stored in.
//!
//! # The live planet: [`Evolution`] + [`PlanetEpoch`]
//!
//! [`Evolution`] is the ONE live planet driver — the ten-[`Phase`] tick that
//! grows a world forward on a [`HexMap`]: volcanism injects material, hot rock
//! spreads, plate boundaries uplift and subduct, plates carry their rock, water
//! and ice and flora settle over it. It moved here from the Populous Bench
//! (2026-08-28) so the sim lives in an engine crate any scene can reach; the
//! bench now only hosts it. Its static context is rolled once and handed in:
//! [`SeamField`] (the convection cells and plumes), [`CrustField`] (the vents
//! and upwell zones derived from them) and [`PlateField`].
//!
//! [`PlanetEpoch`] is that world's file — `.epoch` v2, the format [`Evolution`]
//! captures into and restores from. It stores the RECIPE that regenerates the
//! static context (freq, seed, cells, spots) plus the era's path-dependent
//! per-hex ledger; everything derivable is re-derived on restore. Tick and
//! capture are one driver's two halves, not two unbridged ones.
//!
//! ```no_run
//! use flicker_worldengine::{CrustField, Evolution, HexMap, SeamField};
//! use flicker_worldengine::{DEFAULT_CELLS, DEFAULT_SPOTS};
//! let map = HexMap::new(96);                       // the standard world
//! let seams = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 0xC0FFEE);
//! let crust = CrustField::derive(&map, &seams);
//! let mut era = Evolution::new(&map, &seams);
//! let sea = era.resolve_sea();
//! era.tick(&map, &seams, &crust, sea);             // one tick of the ten phases
//! let planet = era.capture(&map, &seams, "my world");  // a .epoch v2, ready to save
//! # let _ = planet;
//! ```
//!
//! # LEGACY — the frozen v1 toolbox
//!
//! [`engine`] ([`WorldEngine`]: the nine-epoch batch over the
//! [`flicker_worldgen`] chemistry kernels, with the immutable [`EpochSnapshot`]
//! cache, the forward-regenerative replay and the v1 [`EpochFile`] capture),
//! [`sim`] ([`Simulation`]: an independent E1/E2 tick over the same kernels),
//! [`habitability`], [`config`], [`levers`], [`nodes`], [`snapshot`], and the v1
//! [`EpochFile`] half of [`epochfile`] are FROZEN. They have zero live
//! consumers — the scenes that drove them (God Mode, the epoch viewer) were
//! retired in favour of the Populous Bench — and they are kept deliberately as
//! engine inventory under the standing ruling that merely-unused engine features
//! are not dead code. Removing them is the owner's later call, not a cleanup.
//!
//! ```no_run
//! use flicker_worldengine::WorldEngine;
//! let mut engine = WorldEngine::from_repo().unwrap();  // Earth-like defaults
//! engine.set_freq(6);
//! let hydrosphere = engine.snapshot(4);                // computes epochs 1..=4
//! println!("cells: {}", hydrosphere.len());
//! engine.set_lever("e3_mountain_uplift", 1.2);         // invalidates epochs 3..=9
//! let world = engine.capture("my world");              // a v1 .epoch, ready to save
//! # let _ = world;
//! ```

// ── THE LIVE PLANET ────────────────────────────────────────────────────────
pub mod crust;
pub mod epochfile;
pub mod evolve;
pub mod map;
pub mod plates;
pub mod seams;

// ── THE FROZEN v1 TOOLBOX ──────────────────────────────────────────────────
pub mod config;
pub mod engine;
pub mod habitability;
pub mod levers;
pub mod nodes;
pub mod sim;
pub mod snapshot;

// The live driver's surface: the map a planet stands on, the three static
// fields rolled onto it, and the era that evolves them.
pub use crust::{CrustField, UPWELL_HEAT};
pub use evolve::{
    vein_index_of, vein_kinds, Evolution, Phase, VeinKind, VeinNode, AIR_LAYERS, BOOTSTRAP_TICKS,
    CHANNEL_LIVE, DECK_ALT, GREEN_COVER, ICE_SOLID, MARINE_HARD_CAP, META_HARD_CAP, PHASES,
};
pub use map::{diameter_mi, HexMap, TileId, DEFAULT_FREQ, MAX_FREQ, MIN_FREQ, TILE_MI};
pub use plates::{PlateField, DEFAULT_PLATES, MAX_PLATES, MIN_PLATES};
pub use seams::{
    SeamField, DEFAULT_CELLS, DEFAULT_SPOTS, MAX_CELLS, MAX_SPOTS, MIN_CELLS, MIN_SPOTS,
};

// The `.epoch` format — v2 (`PlanetEpoch`, what `Evolution` captures into) and
// the frozen v1 half (`EpochFile`, what `WorldEngine` captured into).
pub use epochfile::{
    EpochFile, EpochFileError, PlanetEpoch, PlanetEra, PlanetLedger, PlanetRecipe, VeinBody,
    EPOCH_FORMAT, EPOCH_VERSION, LEGACY_EPOCH_VERSION,
};

// LEGACY (v1 toolbox). The freq constants the v1 batch runs at stay at
// `config::{MIN_FREQ, MAX_FREQ, DEFAULT_FREQ}` — the crate root's are the live
// map's dial.
pub use config::{
    build_epoch1, build_transforms, mutate_epoch, next_seed, seed_chain, WorldConfig, DEFAULT_SEED,
    WORLD_EPOCHS,
};
pub use engine::WorldEngine;
pub use habitability::{observe, Axis, Habitability};
pub use levers::{
    repo_content_dir, AbundanceDef, GeneratorError, GeneratorParams, GeneratorParamsSource,
    JsonGeneratorSource, LeverDef,
};
pub use sim::{Simulation, World, MY_PER_TICK};
pub use snapshot::{masses_agree, EpochSnapshot, Provenance};

// LEGACY. Re-export the per-cell state the v1 API exposes (`World.cells`,
// `EpochSnapshot.cells`) so viewers can name it without depending on
// `flicker-worldgen` directly. Its `Phase` (the v1 matter phase) stays at
// `flicker_worldgen::Phase` — the crate root's `Phase` is the live era's.
pub use flicker_worldgen::{classify, HexState, Layer, LayerClass, LayerKind, LayerLedger};
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
            assert!(
                masses_agree(m, seed_mass, 1e-9),
                "epoch {epoch} drifted pre-delivery ({m} vs {seed_mass})"
            );
        }
        // Epoch 4 delivers water from the outer system — element mass jumps up.
        let e4 = e.snapshot(4).conserved_mass();
        assert!(
            e4 > seed_mass,
            "E4 water delivery should add mass ({e4} vs {seed_mass})"
        );
        // Epochs 4-9: no further additions or losses — conserved among themselves.
        for epoch in 4..=WORLD_EPOCHS {
            let m = e.snapshot(epoch).conserved_mass();
            assert!(
                masses_agree(m, e4, 1e-9),
                "epoch {epoch} drifted post-delivery ({m} vs {e4})"
            );
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
        assert!(
            snap.cells.iter().any(|c| !c.compounds.is_empty()),
            "no compounds formed"
        );
        // Water is present as a compound (delivered + accounted).
        let water_id = tables.compound("Water").expect("Water compound").id;
        assert!(
            snap.cells
                .iter()
                .any(|c| c.compounds.amount(water_id) > 0.0),
            "no water compound"
        );
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
        assert!(
            e.peek(2).is_some(),
            "epoch 2 should stay cached after a late edit"
        );
        assert!(
            e.peek(6).is_none(),
            "epoch 6 should be invalidated by its own lever"
        );

        let e2_after = e.snapshot(2).clone();
        let e6_after = e.snapshot(6).clone();
        assert_eq!(e2_before.cells, e2_after.cells, "an E6 edit changed E2");
        assert_ne!(
            e6_before.cells, e6_after.cells,
            "the E6 edit didn't change E6"
        );
    }

    #[test]
    fn editing_an_early_lever_invalidates_forward() {
        let mut e = engine();
        e.snapshot(WORLD_EPOCHS); // fill everything
        e.set_lever("e3_plates", 3.0);
        assert!(
            e.peek(2).is_some(),
            "epoch 2 (before the edit) stays cached"
        );
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
        assert_ne!(
            e3_before.cells,
            e.snapshot(3).cells,
            "the E3 reseed didn't change E3"
        );
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
        assert_eq!(
            before, after,
            "reseeding Epoch 3 reshuffled the material-derived plate layout"
        );
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
            !e.tables()
                .harvestable_compounds()
                .any(|c| c.name == "Water"),
            "water should not be a mineable ore"
        );
    }

    #[test]
    fn the_cooling_clock_spans_every_epoch_as_one_axis() {
        let mut e = engine();
        // Epoch 1 is the molten seed — it advances no cooling steps; the clock starts at E2.
        assert_eq!(
            e.epoch_cool_steps(1),
            0,
            "the seed layer advances no cooling steps"
        );
        // Per-epoch boundaries are cumulative and strictly increasing across E2..=E9, and
        // each epoch's start is the previous epoch's end — one contiguous axis.
        let mut prev = e.cool_step_before(2);
        for ep in 3..=WORLD_EPOCHS {
            let b = e.cool_step_before(ep);
            assert!(
                b > prev,
                "cooling boundary must advance at E{ep} ({b} vs {prev})"
            );
            assert_eq!(
                b,
                e.cool_step_end(ep - 1),
                "start of E{ep} == end of E{}",
                ep - 1
            );
            prev = b;
        }
        assert_eq!(
            e.cooling_total_steps(),
            e.cool_step_end(WORLD_EPOCHS),
            "span == end of E9"
        );
        // The tectonics onset lands within the molten+tectonic span (default: at the boundary).
        if let Some(onset) = e.tectonics_onset_step() {
            assert!(
                onset <= e.cool_step_before(4),
                "tectonics onset sits in the molten+tectonic span"
            );
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
