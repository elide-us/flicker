//! Body detection — find where bodies are likely to seed (the Phase-1 **hot spots**),
//! **across the whole disk**, in world coordinates.
//!
//! For each ring we take its angular **clump crests** (local density maxima). Then we cluster
//! crests that are co-located in *(radius-fraction, angle)* space — zoom- and scale-independent,
//! so it works the same for the tightly-packed inner metals and the sparse outer volatiles. A
//! cluster that merges several rings is a stronger column, but a lone ring with a strong crest is
//! still a valid seed (an isolated outer body). The hot spot's `strength` is its **peak
//! overdensity** (prominence over ambient).
//!
//! Output is **world-space** (`au`, `theta`, `strength`) — the renderer maps it to the screen.
//! Detection only — it marks candidate sites; the collapse (`crate::collapse`) seeds bodies from
//! the same conserved cloud the crests come from.

use std::f32::consts::{PI, TAU};

use crate::cloud::CloudField;
use crate::model::{CastParams, Ejecta};

/// Angular samples per ring when hunting clump crests.
const N_ANG: usize = 160;
/// A crest must clear `1 + MIN_PROM` over ambient to count as a lump.
const MIN_PROM: f32 = 0.06;
/// Cluster lumps within this fractional radius difference …
const SEP_FRAC: f32 = 0.16;
/// … and this angular distance (radians).
const SEP_ANG: f32 = 0.45;
/// Show hot spots whose peak prominence reaches this.
const MIN_PEAK: f32 = 0.10;
/// Cap the hot spots returned (strongest first).
pub const MAX_HOT_SPOTS: usize = 60;

/// An overdensity site in world coordinates: cast radius `au`, angle `theta` (radians, at the
/// current rotation), and `strength` (peak overdensity over ambient). The Phase-1 output — the
/// seed locations bodies aggregate from.
#[derive(Clone, Copy, Debug)]
pub struct HotSpot {
    pub au: f32,
    pub theta: f32,
    pub strength: f32,
}

/// Scan the cloud for candidate body sites, strongest (peak overdensity) first.
pub fn detect(ej: &Ejecta, cast: &CastParams, cloud: &CloudField, time: f32, anchor_au: f32) -> Vec<HotSpot> {
    // 1. Every ring's clump crests → lumps at (au, angle, prominence).
    let mut lumps: Vec<(f32, f32, f32)> = Vec::new();
    for (i, el) in ej.elements.iter().enumerate() {
        let au = cast.distance_au(el.atomic_mass);
        let rot = cloud.omega(au, anchor_au) * time;
        let mut d = [0.0_f32; N_ANG];
        for (j, slot) in d.iter_mut().enumerate() {
            *slot = cloud.density(i, j as f32 / N_ANG as f32 * TAU, rot);
        }
        for j in 0..N_ANG {
            let prev = d[(j + N_ANG - 1) % N_ANG];
            let next = d[(j + 1) % N_ANG];
            if d[j] > prev && d[j] >= next && d[j] > 1.0 + MIN_PROM {
                lumps.push((au, j as f32 / N_ANG as f32 * TAU, d[j] - 1.0));
            }
        }
    }

    // 2. Cluster strongest-first by (radius-fraction, angle). The seed lump is the peak.
    let mut order: Vec<usize> = (0..lumps.len()).collect();
    order.sort_by(|&a, &b| lumps[b].2.total_cmp(&lumps[a].2));
    let mut used = vec![false; lumps.len()];
    let mut out: Vec<HotSpot> = Vec::new();
    for &a in &order {
        if used[a] {
            continue;
        }
        used[a] = true;
        let (au_a, th_a, peak) = lumps[a];
        for (b, &(au_b, th_b, _)) in lumps.iter().enumerate() {
            if used[b] {
                continue;
            }
            if (au_a - au_b).abs() / au_a.max(0.01) < SEP_FRAC && ang_diff(th_a, th_b) < SEP_ANG {
                used[b] = true;
            }
        }
        if peak >= MIN_PEAK {
            out.push(HotSpot { au: au_a, theta: th_a, strength: peak });
        }
    }

    out.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    out.truncate(MAX_HOT_SPOTS);
    out
}

/// Smallest absolute angle between two directions (radians, `0..=π`).
fn ang_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > PI {
        TAU - d
    } else {
        d
    }
}
