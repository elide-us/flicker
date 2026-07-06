//! Slice 2 — the world-gen epoch chain runs **unmodified** on the real
//! icosahedral topology produced by `flicker-worldgrid`, including the
//! 5-neighbour pentagon defect, and yields a continuous planet (neighbours
//! blend, not per-hex noise).
//!
//! This is the proof behind docs/hex-sphere-handoff.md §3: the pipeline consumes
//! `EpochCtx { dirs, neighbors }` and never assumes degree 6, so swapping the toy
//! `ring(n)` for a pentagon patch needs no pipeline change — this test only uses
//! the public API.

use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::{
    six_epoch_stack, Boundary, Epoch1, Epoch1Params, Epoch6, EpochCtx, EPOCHS,
};
use flicker_worldgrid::pentagon_patch;

fn tables() -> Tables {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
    Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
}

/// Mean |Δ| of `vals` across adjacency edges (each undirected edge counted once).
fn neighbor_mean_diff(neighbors: &[Vec<u32>], vals: &[f64]) -> f64 {
    let (mut sum, mut count) = (0.0, 0u64);
    for (i, nbrs) in neighbors.iter().enumerate() {
        for &j in nbrs {
            if j as usize > i {
                sum += (vals[i] - vals[j as usize]).abs();
                count += 1;
            }
        }
    }
    sum / count.max(1) as f64
}

/// Mean |Δ| of `vals` over all unordered cell pairs — the global spread.
fn global_mean_diff(vals: &[f64]) -> f64 {
    let (mut sum, mut count) = (0.0, 0u64);
    for i in 0..vals.len() {
        for j in (i + 1)..vals.len() {
            sum += (vals[i] - vals[j]).abs();
            count += 1;
        }
    }
    sum / count.max(1) as f64
}

#[test]
fn epoch_stack_runs_on_a_pentagon_patch() {
    let t = tables();
    let e1 = Epoch1::new(&t, Epoch1Params::default(), 7);
    let patch = pentagon_patch(6);
    let n = patch.len();
    let pent = patch.center as usize;
    assert_eq!(patch.neighbors[pent].len(), 5, "the centre is the pentagon defect");

    // Feed the grid straight into the existing context — no special-casing.
    let ctx = EpochCtx {
        tables: &t,
        dirs: &patch.dirs,
        neighbors: &patch.neighbors,
        seed: 7,
    };
    let stack = six_epoch_stack(&e1, &ctx);

    // --- runs unmodified: same public API, every layer covers every cell ---
    assert_eq!(stack.len(), EPOCHS);
    for layer in &stack {
        assert_eq!(layer.len(), n, "every layer covers every cell");
    }

    // --- each real epoch did its work on the icosahedral topology (the
    //     neighbour-driven epochs 3 & 6 thus handled the 5-neighbour cell) ---
    assert!(stack[1].iter().all(|s| !s.crust.is_empty()), "Epoch 2 made no crust");
    assert!(stack[2].iter().any(|s| s.elevation != 0.0), "Epoch 3 wrote no elevation");
    assert!(
        stack[2].iter().any(|s| s.boundary != Boundary::Interior),
        "Epoch 3 found no plate boundary"
    );
    assert!(stack[3].iter().any(|s| s.water_depth > 0.0), "Epoch 4 made no ocean");
    assert!(
        stack[4].iter().any(|s| s.hydrothermal > 0.0),
        "Epoch 5 made no hydrothermal activity"
    );
    assert!(
        stack[5].iter().any(|s| s.flow > Epoch6::default().rain),
        "Epoch 6 routed no drainage"
    );

    // --- nothing went non-finite anywhere (no NaN leaking from the defect) ---
    for (e, layer) in stack.iter().enumerate() {
        for (i, s) in layer.iter().enumerate() {
            assert!(
                s.elevation.is_finite()
                    && s.water_depth.is_finite()
                    && s.flow.is_finite()
                    && s.temperature.is_finite()
                    && s.precipitation.is_finite()
                    && s.prebiotic.is_finite()
                    && s.biomass.is_finite()
                    && s.organics.is_finite()
                    && s.deposits.total().is_finite()
                    && s.atmosphere.total().is_finite()
                    && s.hydrothermal.is_finite()
                    && s.composition.total().is_finite(),
                "non-finite field at epoch {e}, cell {i}"
            );
        }
    }

    // --- the pentagon flows through as an ordinary cell (sane final values) ---
    let ground = &stack[EPOCHS - 1][pent];
    assert!(ground.composition.total() > 0.0, "pentagon lost its mass");
    let (lo, hi) = stack[EPOCHS - 1]
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), s| {
            (lo.min(s.elevation), hi.max(s.elevation))
        });
    assert!(
        ground.elevation >= lo && ground.elevation <= hi,
        "pentagon elevation {} outside the field band [{lo}, {hi}]",
        ground.elevation
    );

    // --- continuity: a coherent field blends across neighbours, not noise.
    //     Epoch 1 Fe is latitude-biased + regional fBm — smooth over the sphere,
    //     so adjacent cells differ markedly less than cells picked at random. ---
    let fe: Vec<f64> = stack[0].iter().map(|s| s.composition.amount(26)).collect();
    let nd = neighbor_mean_diff(&patch.neighbors, &fe);
    let gd = global_mean_diff(&fe);
    assert!(gd > 0.0, "Fe field is flat — nothing to test");
    assert!(
        nd < 0.7 * gd,
        "Fe not continuous across neighbours: neighbour Δ {nd:.3} vs global Δ {gd:.3}"
    );

    println!(
        "pentagon patch: {n} cells, neighbour Fe Δ {nd:.1} vs global {gd:.1} (ratio {:.2})",
        nd / gd
    );
}
