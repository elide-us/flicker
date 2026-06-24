//! Body detection — find where bodies are likely to seed, **across the whole disk**.
//!
//! For each ring we take its angular **clump crests** (local density maxima). Then we
//! cluster crests that are co-located in *(radius-fraction, angle)* space — zoom- and
//! scale-independent, so it works the same for the tightly-packed inner metals and the
//! sparse outer volatiles. A cluster that merges several rings is a stronger column, but a
//! lone ring with a strong crest is still a valid seed (an isolated outer body). The
//! candidate's `strength` is its **peak overdensity** (prominence over ambient) — that's
//! what the formation threshold gates on, so "does a body form here" doesn't require a
//! crowd of rings, just a strong-enough clump.
//!
//! Detection only — it places candidate sites; accretion (`crate::accrete`) decides which
//! clear the threshold and grows them.

use std::f32::consts::{PI, TAU};

use flicker::render::Vec2;

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
/// Show candidates whose peak prominence reaches this (belts included; the *forming* cut
/// is the accretion threshold, higher than this).
const MIN_PEAK: f32 = 0.10;
/// Cap the candidates returned (strongest first).
pub const MAX_CANDIDATES: usize = 60;

/// An overdensity site: screen position + peak overdensity (`strength`). Informational —
/// the actual body collapse happens on the live mass field in `crate::accrete`.
pub struct Candidate {
    pub pos: Vec2,
    pub strength: f32,
}

/// Screen mapping the detector needs to place candidates.
pub struct View {
    pub center: Vec2,
    pub px_per_au: f32,
    pub view_radius_px: f32,
}

/// Scan the cloud for candidate body sites, strongest (peak overdensity) first.
pub fn detect(
    ej: &Ejecta,
    params: &CastParams,
    cloud: &CloudField,
    time: f32,
    anchor_au: f32,
    view: &View,
) -> Vec<Candidate> {
    // 1. Every ring's clump crests → lumps at (au, screen angle, prominence).
    let mut lumps: Vec<(f32, f32, f32)> = Vec::new();
    for (i, el) in ej.elements.iter().enumerate() {
        let au = params.distance_au(el.atomic_mass);
        let r_px = au * view.px_per_au;
        if r_px < 4.0 || r_px > view.view_radius_px * 1.7 {
            continue; // off the visible plate
        }
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
    let mut out: Vec<Candidate> = Vec::new();
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
            let rr = au_a * view.px_per_au;
            out.push(Candidate {
                pos: Vec2::new(view.center.x + rr * th_a.cos(), view.center.y + rr * th_a.sin()),
                strength: peak,
            });
        }
    }

    out.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    out.truncate(MAX_CANDIDATES);
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
