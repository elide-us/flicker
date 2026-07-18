//! **Pre-hydraulic weathering** — the erosion that runs *before* Epoch 6's full
//! hydraulic sim, so relief doesn't grow unchecked over deep time.
//!
//! The tectonic epochs *build* relief every step (drift piles crust, orogeny lifts
//! arcs, hotspot plumes stack chains) but nothing wore it back down until the Epoch-6
//! water cycle — so a long timeline let the sharpest features (hotspot streaks, swept
//! boundary ridges) run away into spikes. Real planets weather from the start: a thick
//! early **proto-atmosphere**, freeze–thaw, and gravity mass-wasting grind the steepest
//! slopes down long before oceans exist. This adds that missing process as a *gentle*
//! pass the engine runs in Epochs 3–5, so weathering **keeps pace** with the tectonic
//! growth instead of arriving all at once at Epoch 6.
//!
//! The mechanism is Epoch 6's own **thermal creep** (slope-limited relaxation toward a
//! talus angle) dialled down and made **hardness-aware**: soft rock sheds fast, hard
//! rock resists and stands as ridges (the same erodibility basis Epoch 6 uses). It
//! moves the *derived* elevation only — like [`run_orogeny`](crate::run_orogeny) and the
//! hotspot chains, relief here is a derived quantity, so there is no conserved element
//! mass to leak; Epoch 6 still owns the material-conserving sediment conveyor. The pair
//! exchange is symmetric, so the elevation field is conserved (peaks lose exactly what
//! the neighbouring lows gain — nothing is planed off the whole planet).

use crate::epoch6::solid_hardness;
use crate::pipeline::EpochCtx;
use crate::state::HexState;

/// Weather the relief for `steps` passes of hardness-weighted, talus-limited creep.
///
/// Each pass relaxes every neighbour pair whose slope exceeds `talus` toward it, moving
/// `rate` of the above-talus excess (scaled by the **higher** hex's erodibility — the
/// peak is what weathers). `steps` scales with elapsed time, so more deep time = more
/// smoothing (the mitigation against runaway tectonic spikes). A no-op for `steps == 0`,
/// so the Epoch-3 iteration scrubber shows pristine relief at t=0 and progressively
/// weathered relief as it advances.
pub fn run_protoatmospheric_erosion(
    cells: &mut [HexState],
    ctx: &EpochCtx,
    talus: f32,
    rate: f32,
    steps: u32,
) {
    let n = cells.len();
    if n == 0 || steps == 0 {
        return;
    }
    // Per-hex erodibility from the surface hardness (Epoch 6's basis): soft rock sheds
    // fast (→ 1.0), hard rock resists (→ 0.2) and stands as a ridge.
    let erod: Vec<f32> = cells
        .iter()
        .map(|s| (1.0 - solid_hardness(ctx, s.surface()) / 10.0).clamp(0.2, 1.0))
        .collect();
    let mut elev: Vec<f32> = cells.iter().map(|s| s.elevation).collect();
    for _ in 0..steps {
        // Read a snapshot, write live — each undirected pair handled once (nb > i),
        // symmetric so the elevation sum is conserved.
        let snap = elev.clone();
        for i in 0..n {
            for &nb in &ctx.neighbors[i] {
                let j = nb as usize;
                if j <= i {
                    continue;
                }
                let diff = snap[i] - snap[j];
                if diff.abs() > talus {
                    // The higher hex is the one weathering; its erodibility sets the pace.
                    let ehi = if diff > 0.0 { erod[i] } else { erod[j] };
                    let mv = (diff.abs() - talus) * rate * ehi * 0.5 * diff.signum();
                    elev[i] -= mv;
                    elev[j] += mv;
                }
            }
        }
        for e in elev.iter_mut() {
            *e = e.clamp(-1.0, 1.5);
        }
    }
    for (i, s) in cells.iter_mut().enumerate() {
        s.elevation = elev[i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldstate::Composition;
    use glam::Vec3;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// A ring of `n` hexes, each neighbouring its two ring-mates.
    fn ring(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let dirs = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors =
            (0..n).map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32]).collect();
        (dirs, neighbors)
    }

    /// Weathering knocks a sharp spike down and conserves the elevation field (what the
    /// peak loses, its neighbours gain — nothing is planed off the whole planet).
    #[test]
    fn weathering_softens_a_spike_and_conserves_the_field() {
        let t = tables();
        let n = 24;
        let (dirs, neighbors) = ring(n);
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 0 };
        // A single tall spike on an otherwise flat crust (uniform soft silica).
        let mut cells: Vec<HexState> = (0..n)
            .map(|_| {
                let mut s = HexState::new(Composition::new());
                s.crust = Composition::from_iter([(14u8, 1000.0)]);
                s
            })
            .collect();
        cells[n / 2].elevation = 1.0;
        let peak0 = cells[n / 2].elevation;
        let sum0: f32 = cells.iter().map(|s| s.elevation).sum();

        run_protoatmospheric_erosion(&mut cells, &ctx, 0.08, 0.25, 20);

        assert!(cells[n / 2].elevation < peak0 - 0.05, "the spike wasn't weathered down");
        let sum1: f32 = cells.iter().map(|s| s.elevation).sum();
        assert!((sum1 - sum0).abs() < 1e-3, "weathering didn't conserve the elevation field");
        // steps = 0 is a no-op (the scrubber's pristine t=0).
        let mut pristine = vec![HexState::new(Composition::new()); n];
        pristine[0].elevation = 1.0;
        let before = pristine.clone();
        run_protoatmospheric_erosion(&mut pristine, &ctx, 0.08, 0.25, 0);
        assert_eq!(before, pristine, "steps=0 should not weather");
    }

    /// Hardness differentiates the weathering: a **soft** spike wears down more than an
    /// identically-shaped **hard** one, so hard rock stands as ridges.
    #[test]
    fn hard_rock_resists_more_than_soft() {
        let t = tables();
        let n = 24;
        let (dirs, neighbors) = ring(n);
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 0 };
        // Two worlds, identical spike; one built of a soft former, one of a hard former.
        let (soft, hard) = {
            // Pick the softest and hardest solid elements the tables carry.
            let mut solids: Vec<(u8, f32)> = (1u8..=99)
                .filter_map(|z| {
                    let e = t.element_by_number(z)?;
                    (e.state != flicker_materials::PhysicalState::Gas).then_some((z, e.hardness))
                })
                .collect();
            solids.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            (solids.first().unwrap().0, solids.last().unwrap().0)
        };
        let build = |el: u8| {
            let mut cells: Vec<HexState> = (0..n)
                .map(|_| {
                    let mut s = HexState::new(Composition::new());
                    s.crust = Composition::from_iter([(el, 1000.0)]);
                    s
                })
                .collect();
            cells[n / 2].elevation = 1.0;
            let mut cells2 = cells.clone();
            run_protoatmospheric_erosion(&mut cells2, &ctx, 0.08, 0.25, 20);
            cells2[n / 2].elevation
        };
        assert!(
            build(soft) < build(hard) - 1e-4,
            "soft rock should weather more than hard ({} vs {})",
            build(soft),
            build(hard)
        );
    }
}
