//! **The V5 pipeline as one call** — composed → epochs → era → the FOREVER
//! LOOP — for every headless reader (the shape evaluator, the ground
//! renderer) so none of them spells the stages a second way. The WorldBench
//! runs the same stages interactively (its RUN and LOOP boxes); this is their
//! batch form.
//!
//! **The forever loop** (the mandate's "motion and chemistry and iteration"):
//! a static world planes under rain alone; a planet keeps its hills because
//! uplift answers erosion. Each loop step the era's relief is WRITTEN BACK
//! into the crust's parcels ([`Epochs::absorb`] — the eroded rock is gone
//! from the parcel that carried it, the sediment is part of it), the epochs
//! run [`LOOP_EPOCHS`] ticks carrying the eroded world (compression, drops,
//! impacts), and the era adopts the epoched world again for [`LOOP_CYCLES`]
//! cycles. Write back, move, adopt, cut — forever.

use crate::epochs::{EpochRecipe, Epochs};
use crate::evolve::Evolution;
use crate::map::HexMap;
use crate::recipe::{compose, Composed, Recipe};
use crate::seams::SeamField;

/// One loop step's epochs and era cycles.
pub const LOOP_EPOCHS: u32 = 10;
pub const LOOP_CYCLES: u32 = 10;

/// A world at the end of the stages run: its heights, sea and hardness, and
/// the era's loose sediment per tile (radius units) where the era ran.
pub struct Staged {
    pub world: Composed,
    pub sediment: Option<Vec<f32>>,
    /// What the stages reported.
    pub epochs: Option<crate::epochs::EpochStats>,
    pub era_ticks: u64,
    /// Eruptions and impacts over every epoch run.
    pub events: (u64, u64),
    pub loops: u32,
}

/// The era's world as the next stage reads it.
fn era_world(map: &HexMap, era: &Evolution) -> (Composed, Vec<f32>) {
    let frame = era.adopted().expect("adopted");
    let sediment = (0..map.len() as u32)
        .map(|t| era.sediment(t) * frame.scale)
        .collect();
    (
        Composed {
            heights: era.relief(),
            sea: era.relief_sea(),
            hardness: era.surface_hardnesses(),
        },
        sediment,
    )
}

/// Compose `recipe`, run `epochs` epochs of the stone ocean (at `epoch_recipe`),
/// then `erode` surface cycles of the era adopted over the result, then
/// `loops` steps of the forever loop.
pub fn stage(
    map: &HexMap,
    recipe: &Recipe,
    epochs: u32,
    epoch_recipe: &EpochRecipe,
    erode: u32,
    loops: u32,
) -> Staged {
    let mut world = compose(map, recipe);
    let mut stats = None;
    let mut crust_epochs: Option<Epochs> = None;
    if epochs > 0 || loops > 0 {
        let mut e = Epochs::new(map, recipe, &world, epoch_recipe);
        if epochs > 0 {
            stats = Some(e.run(map, epoch_recipe, epochs));
            world = Composed {
                heights: e.heights().to_vec(),
                sea: world.sea,
                hardness: e.hardness().to_vec(),
            };
        }
        crust_epochs = Some(e);
    }
    let mut sediment = None;
    let mut era_ticks = 0;
    let mut era: Option<(Evolution, SeamField, crate::crust::CrustField)> = None;
    if erode > 0 || loops > 0 {
        let seams = SeamField::new(map, recipe.heat.cells, recipe.heat.spots, recipe.heat.seed);
        let crust = crate::crust::CrustField::derive(map, &seams);
        let mut e = Evolution::new(map, &seams);
        e.adopt(map, &seams, &world);
        for _ in 0..erode {
            let sea = e.resolve_sea();
            e.tick(map, &seams, &crust, sea);
        }
        era_ticks = e.ticks();
        let (w, s) = era_world(map, &e);
        world = w;
        sediment = Some(s);
        era = Some((e, seams, crust));
    }
    for _ in 0..loops {
        let (era_e, seams, crust) = era.as_mut().expect("the loop runs an era");
        let crust_e = crust_epochs.as_mut().expect("the loop runs the epochs");
        // Write back, move, adopt, cut.
        crust_e.absorb(&era_e.relief(), &era_e.surface_hardnesses());
        stats = Some(crust_e.run(map, epoch_recipe, LOOP_EPOCHS));
        let moved = Composed {
            heights: crust_e.heights().to_vec(),
            sea: world.sea,
            hardness: crust_e.hardness().to_vec(),
        };
        era_e.adopt(map, seams, &moved);
        for _ in 0..LOOP_CYCLES {
            let sea = era_e.resolve_sea();
            era_e.tick(map, seams, crust, sea);
        }
        era_ticks += era_e.ticks();
        let (w, s) = era_world(map, era_e);
        world = w;
        sediment = Some(s);
    }
    Staged {
        world,
        sediment,
        epochs: stats,
        era_ticks,
        events: crust_epochs.as_ref().map_or((0, 0), Epochs::events_so_far),
        loops,
    }
}

// ── IS THE CUT HONEST ABOUT HARDNESS? ──
// The open question banked in HEX WORLD DIRECTION (BDF69451): on the globe
// the era reads as soft lowlands spreading, with no place visibly eroding
// differently by material. Measured here, not guessed — the land hexes the
// era adopted, binned by the hardness ledger they were adopted with, and per
// bin what the era did to their height. Judged against NO EROSION WITHOUT
// HARDNESS (23D867F8): an honest cut takes more off the soft bins than the
// hard; a cut blind to hardness takes the same off every bin.

/// One hardness bin of the adopted land and what the era did to it.
#[derive(Clone, Debug, PartialEq)]
pub struct CutBin {
    /// The bin's hardness range, `0..1`.
    pub lo: f32,
    pub hi: f32,
    pub count: usize,
    /// Mean height change, radius units (negative = lowered).
    pub mean_dh: f32,
    /// Mean of the lowering alone — what came off — radius units.
    pub mean_cut: f32,
    /// Share of the bin's hexes standing lower after.
    pub lowered: f32,
    /// Mean height above the sea the bin's hexes were adopted at, radius
    /// units — the confound: hard rock is the high rock, and the high rock
    /// is the steep rock.
    pub mean_rise: f32,
}

/// The bins' edges: the catalog's rocks fall one class to a bin — breccia
/// and shale (0.25, 0.15) | sandstone (0.5) | andesite and basalt (0.7,
/// 0.75) | granite and gneiss (0.8, 0.85) | quartzite (0.95).
pub const CUT_BIN_EDGES: [f32; 6] = [0.0, 0.3, 0.6, 0.78, 0.9, 1.0];

/// Bin the land of `before` (the world the era adopted) by its hardness and
/// read `after` (the era's relief, same units) against it.
pub fn hardness_cut_bins(before: &Composed, after: &[f32]) -> Vec<CutBin> {
    hardness_cut_bins_within(before, after, |_| true)
}

/// [`hardness_cut_bins`] over the land hexes `keep` admits (by index).
fn hardness_cut_bins_within(
    before: &Composed,
    after: &[f32],
    keep: impl Fn(usize) -> bool,
) -> Vec<CutBin> {
    let n_bins = CUT_BIN_EDGES.len() - 1;
    let mut sums = vec![(0.0f64, 0.0f64, 0usize, 0usize, 0.0f64); n_bins];
    for (i, (&h0, &h1)) in before.heights.iter().zip(after).enumerate() {
        if h0 <= before.sea || !keep(i) {
            continue;
        }
        let hard = before
            .hardness
            .get(i)
            .copied()
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        let b = CUT_BIN_EDGES
            .windows(2)
            .position(|w| hard >= w[0] && hard < w[1])
            .unwrap_or(n_bins - 1);
        let dh = f64::from(h1 - h0);
        let e = &mut sums[b];
        e.0 += dh;
        e.1 += (-dh).max(0.0);
        e.2 += 1;
        if dh < 0.0 {
            e.3 += 1;
        }
        e.4 += f64::from(h0 - before.sea);
    }
    sums.into_iter()
        .enumerate()
        .map(|(b, (sum, cut, n, low, rise))| CutBin {
            lo: CUT_BIN_EDGES[b],
            hi: CUT_BIN_EDGES[b + 1],
            count: n,
            mean_dh: if n == 0 { 0.0 } else { (sum / n as f64) as f32 },
            mean_cut: if n == 0 { 0.0 } else { (cut / n as f64) as f32 },
            lowered: if n == 0 { 0.0 } else { low as f32 / n as f32 },
            mean_rise: if n == 0 {
                0.0
            } else {
                (rise / n as f64) as f32
            },
        })
        .collect()
}

/// **The cut at matched relief**: the land split into three terciles by
/// the height it was adopted at, each binned by hardness on its own — so
/// soft and hard are compared at the same elevation and the confound (hard
/// rock stands high and steep) is held still. Per tercile: its height
/// range (radius units above the sea) and its bins.
pub fn hardness_cut_matched(before: &Composed, after: &[f32]) -> Vec<(f32, f32, Vec<CutBin>)> {
    let rise: Vec<f32> = before.heights.iter().map(|h| h - before.sea).collect();
    matched_by(before, after, &rise)
}

/// **The cut at matched SLOPE**: the land split into three terciles by the
/// steepest drop to a neighbour it was adopted with (radius units per
/// radian of arc), each binned by hardness on its own — the era's water
/// cut and its cliffs both answer to slope, so this is the reading that
/// isolates the grade.
pub fn hardness_cut_matched_slope(
    map: &HexMap,
    before: &Composed,
    after: &[f32],
) -> Vec<(f32, f32, Vec<CutBin>)> {
    let slope: Vec<f32> = (0..map.len() as u32)
        .map(|t| {
            let h = before.heights[t as usize];
            let d = map.direction(t);
            map.neighbours(t)
                .iter()
                .map(|nb| {
                    let dist = (map.direction(*nb) - d).length().max(1e-6);
                    (h - before.heights[*nb as usize]) / dist
                })
                .fold(0.0f32, f32::max)
        })
        .collect();
    matched_by(before, after, &slope)
}

/// **The cut per unit of slope**, per hardness bin: every land hex's
/// lowering divided by the steepest drop it was adopted with (radius units
/// per radian), averaged over the bin — the budget's slope term divided
/// out, so what is left to compare across bins is the grade's doing (and
/// the rain's). Per bin: its hardness range, its count, the mean cut per
/// slope.
pub fn hardness_cut_per_slope(
    map: &HexMap,
    before: &Composed,
    after: &[f32],
) -> Vec<(f32, f32, usize, f32)> {
    let n_bins = CUT_BIN_EDGES.len() - 1;
    let mut sums = vec![(0.0f64, 0usize); n_bins];
    for t in 0..map.len() as u32 {
        let i = t as usize;
        let h = before.heights[i];
        if h <= before.sea {
            continue;
        }
        let d = map.direction(t);
        let slope = map
            .neighbours(t)
            .iter()
            .map(|nb| {
                let dist = (map.direction(*nb) - d).length().max(1e-6);
                (h - before.heights[*nb as usize]) / dist
            })
            .fold(0.0f32, f32::max);
        if slope <= 1e-4 {
            continue;
        }
        let hard = before
            .hardness
            .get(i)
            .copied()
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        let b = CUT_BIN_EDGES
            .windows(2)
            .position(|w| hard >= w[0] && hard < w[1])
            .unwrap_or(n_bins - 1);
        let cut = (h - after[i]).max(0.0);
        sums[b].0 += f64::from(cut / slope);
        sums[b].1 += 1;
    }
    sums.into_iter()
        .enumerate()
        .map(|(b, (sum, n))| {
            (
                CUT_BIN_EDGES[b],
                CUT_BIN_EDGES[b + 1],
                n,
                if n == 0 { 0.0 } else { (sum / n as f64) as f32 },
            )
        })
        .collect()
}

/// [`hardness_cut_per_slope`] as one report line: the populated bins and
/// the softest's cut-per-slope over the hardest's.
pub fn hardness_cut_per_slope_report(bins: &[(f32, f32, usize, f32)]) -> String {
    let populated: Vec<_> = bins.iter().filter(|b| b.2 > 0).collect();
    let parts: Vec<String> = populated
        .iter()
        .map(|(lo, hi, n, per)| format!("{lo:.2}..{hi:.2}→{:.4} ({n})", per))
        .collect();
    let verdict = match (populated.first(), populated.last()) {
        (Some(s), Some(h)) if !std::ptr::eq(*s, *h) && h.3 > 0.0 => {
            let r = s.3 / h.3;
            if r > 1.0 {
                format!("soft/hard {r:.2}: steers")
            } else {
                format!("soft/hard {r:.2}: BLIND")
            }
        }
        _ => "one bin".to_string(),
    };
    format!("  cut per slope: {}  — {verdict}\n", parts.join("  "))
}

/// The land in three terciles of `key` (per tile), each binned by hardness.
fn matched_by(before: &Composed, after: &[f32], key: &[f32]) -> Vec<(f32, f32, Vec<CutBin>)> {
    let mut keys: Vec<f32> = before
        .heights
        .iter()
        .zip(key)
        .filter(|(h, _)| **h > before.sea)
        .map(|(_, k)| *k)
        .collect();
    if keys.len() < 3 {
        return Vec::new();
    }
    keys.sort_by(f32::total_cmp);
    let n = keys.len();
    let edges = [keys[0], keys[n / 3], keys[2 * n / 3], f32::INFINITY];
    edges
        .windows(2)
        .map(|w| {
            let (lo, hi) = (w[0], w[1]);
            let bins = hardness_cut_bins_within(before, after, |i| key[i] >= lo && key[i] < hi);
            (lo, hi.min(keys[n - 1]), bins)
        })
        .collect()
}

/// The softest populated bin's mean cut over the hardest's: above 1 the
/// hardness steers the cut; `None` with fewer than two populated bins.
pub fn soft_over_hard(bins: &[CutBin]) -> Option<f32> {
    let populated: Vec<&CutBin> = bins.iter().filter(|b| b.count > 0).collect();
    let (soft, hard) = (populated.first()?, populated.last()?);
    if std::ptr::eq(*soft, *hard) {
        return None;
    }
    Some(if hard.mean_cut > 0.0 {
        soft.mean_cut / hard.mean_cut
    } else {
        f32::INFINITY
    })
}

/// The bins as a report, one line each, heights in micro-radii (one µR is
/// about 6.4 m on an Earth), and the verdict line.
pub fn hardness_cut_report(bins: &[CutBin]) -> String {
    let mut s = String::new();
    for b in bins {
        s.push_str(&format!(
            "  hardness {:.2}..{:.2}: {:7} hexes  rise {:7.0} µR  Δh {:+9.2} µR  cut {:8.2} µR  lowered {:3.0}%\n",
            b.lo,
            b.hi,
            b.count,
            b.mean_rise * 1e6,
            b.mean_dh * 1e6,
            b.mean_cut * 1e6,
            b.lowered * 100.0
        ));
    }
    match soft_over_hard(bins) {
        Some(ratio) if ratio > 1.0 => s.push_str(&format!(
            "  soft/hard cut {ratio:.2}: hardness steers the cut\n"
        )),
        Some(ratio) => s.push_str(&format!(
            "  soft/hard cut {ratio:.2}: THE CUT IS BLIND TO HARDNESS\n"
        )),
        None => s.push_str("  one hardness bin populated: no verdict\n"),
    }
    s
}

/// [`hardness_cut_matched`] as report lines: per relief tercile, the
/// populated bins' mean cut and the soft/hard verdict at that relief.
pub fn hardness_cut_matched_report(key: &str, terciles: &[(f32, f32, Vec<CutBin>)]) -> String {
    let mut s = String::new();
    for (lo, hi, bins) in terciles {
        let cuts: Vec<String> = bins
            .iter()
            .filter(|b| b.count > 0)
            .map(|b| {
                format!(
                    "{:.2}..{:.2}→{:.0} µR ({})",
                    b.lo,
                    b.hi,
                    b.mean_cut * 1e6,
                    b.count
                )
            })
            .collect();
        let verdict = match soft_over_hard(bins) {
            Some(r) if r > 1.0 => format!("soft/hard {r:.2}: steers"),
            Some(r) => format!("soft/hard {r:.2}: BLIND"),
            None => "one bin".to_string(),
        };
        s.push_str(&format!(
            "  {key} {:6.0}..{:6.0} µR: {}  — {verdict}\n",
            lo * 1e6,
            hi * 1e6,
            cuts.join("  ")
        ));
    }
    s
}

#[cfg(test)]
mod hardness_honesty {
    use super::*;

    /// **The bins read the cut by hardness**: soft land lowered five times
    /// as much as hard reads as a soft/hard ratio of 5, the sea excluded;
    /// a cut that takes the same off every bin reads as blind.
    #[test]
    fn the_bins_read_the_cut_by_hardness() {
        let n = 12;
        let before = Composed {
            heights: (0..n).map(|i| if i == 0 { -0.01 } else { 0.1 }).collect(),
            sea: 0.0,
            hardness: (0..n)
                .map(|i| if i % 2 == 0 { 0.15 } else { 0.95 })
                .collect(),
        };
        let after: Vec<f32> = (0..n)
            .map(|i| {
                if i == 0 {
                    -0.5 // the sea floor — not land, not counted
                } else if i % 2 == 0 {
                    0.1 - 0.010
                } else {
                    0.1 - 0.002
                }
            })
            .collect();
        let bins = hardness_cut_bins(&before, &after);
        assert_eq!(bins.len(), CUT_BIN_EDGES.len() - 1);
        assert_eq!(bins[0].count, 5, "the soft land, the sea hex excluded");
        assert_eq!(bins[4].count, 6, "the root");
        assert!((bins[0].mean_cut - 0.010).abs() < 1e-6);
        assert!((bins[4].mean_cut - 0.002).abs() < 1e-6);
        assert_eq!(bins[0].lowered, 1.0);
        assert!(
            (bins[0].mean_rise - 0.1).abs() < 1e-6,
            "adopted a tenth of a radius up"
        );
        // At matched relief (every land hex at the same height here, so
        // one tercile holds them all and two are empty of the rest).
        let matched = hardness_cut_matched(&before, &after);
        assert_eq!(matched.len(), 3);
        let populated: Vec<_> = matched
            .iter()
            .filter(|(_, _, b)| b.iter().any(|x| x.count > 0))
            .collect();
        assert!(!populated.is_empty());
        assert!(hardness_cut_matched_report("relief", &matched).contains("steers"));
        let ratio = soft_over_hard(&bins).expect("two bins populated");
        assert!((ratio - 5.0).abs() < 1e-4, "soft over hard {ratio}");
        assert!(hardness_cut_report(&bins).contains("steers the cut"));
        // Blind: the same off every hex.
        let blind: Vec<f32> = before.heights.iter().map(|h| h - 0.005).collect();
        let bins = hardness_cut_bins(&before, &blind);
        assert!((soft_over_hard(&bins).unwrap() - 1.0).abs() < 1e-6);
        assert!(hardness_cut_report(&bins).contains("BLIND"));
    }
}
