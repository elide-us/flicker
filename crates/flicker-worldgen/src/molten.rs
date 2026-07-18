//! **Epoch-2 molten convection** — the mantle heat engine that makes Epoch 2 a real
//! phase instead of an equatorial stretch, and gives Epoch 3 something physical to form
//! its plate seams *over*.
//!
//! The old version was a pure **zonal shear** (every cell pushed its composition
//! eastward, faster at the equator) — it banded material along the equator and drove
//! nothing downstream. This is a **material-driven convection** instead: buoyancy comes
//! from the composition (light columns want to **rise**, dense columns want to **sink**),
//! and a diffusion + buoyancy feedback lets that seed **self-organize into coherent
//! convection cells** — hot **upwelling** domains and cold **downwelling** domains. The
//! surface flow (hot → cold) advects the melt, concentrating dense material into the cold
//! sinks and depleting the hot rises, which sharpens the cells (the feedback). Nothing is
//! painted; the cell layout emerges from the material + the seed.
//!
//! The per-cell result is the [`heat`](HexState::heat) field — the map Epoch 3 will read:
//! downwelling (cold) → convergent/subduction seams, upwelling (hot) → divergent ridges.
//! The subsequent density differentiation (Epoch 2's `apply`) firms a crust over the
//! convected composition.
//!
//! Conserving: the melt only ever **transfers** between cells (double-buffered, read the
//! original grid), so `Σ composition` is invariant (tested). `heat` is a *derived*
//! potential, not a conserved quantity. Pass-based / geological cadence — a few steps
//! baked into the state, never a per-frame PDE.

use flicker_materials::Tables;
use flicker_worldstate::Composition;
use glam::Vec3;

use crate::cooling::cell_radiogenic_heat;
use crate::pipeline::EpochCtx;
use crate::state::HexState;

/// Neighbour-blend fraction per heat-diffusion pass — coarsens the buoyancy seed into
/// large-scale convection cells (higher ⇒ fewer, broader cells).
const HEAT_DIFFUSE: f32 = 0.3;
/// Buoyancy feedback strength: how hard `heat` is pulled toward the *current* material
/// buoyancy each step. The positive feedback (dense sinks cool → pull more → sharpen;
/// depleted rises heat → push more) is what makes the cells coherent rather than mushy.
const HEAT_FEEDBACK: f32 = 0.25;
/// Surface-flow advection gain: fraction of a cell's composition carried to its coldest
/// neighbour per step, scaled by the heat drop across that edge (steeper ⇒ faster flow).
const CONVECT_ADVECT: f32 = 0.5;
/// Courant cap: never carry more than this fraction of a cell downstream in one step.
const MAX_FRAC: f32 = 0.4;
/// Smoothing passes applied to the raw buoyancy seed before the loop, so the cells start
/// from cell-scale structure rather than per-hex composition noise.
const HEAT_SEED_SMOOTH: u32 = 3;
/// How strongly the per-cell **radiogenic** heat source shifts the convection heat map on
/// top of composition buoyancy. Buoyancy says light material rises (hot); radiogenics say
/// K/U-rich material generates its own heat — both drive upwelling, so they add. `0`
/// reproduces the old buoyancy-only heat map.
const RADIOGENIC_HEAT_WEIGHT: f32 = 0.6;

/// Composition-weighted mean density (g/cm³) of a cell's material — the buoyancy proxy
/// (dense sinks, light rises). `0` for an empty cell (treated as neutral upstream).
fn mean_density(comp: &Composition, tables: &Tables) -> f32 {
    let (mut m, mut w) = (0.0f64, 0.0f64);
    for (el, amount) in comp.iter() {
        if let Some(e) = tables.element_by_number(el) {
            m += amount * e.density_g_cm3 as f64;
            w += amount;
        }
    }
    if w > 0.0 {
        (m / w) as f32
    } else {
        0.0
    }
}

/// Per-cell buoyancy in `0..1` from the composition density, normalised across the
/// planet: **lightest → 1 (hot, wants to rise), densest → 0 (cold, wants to sink)**.
/// Empty cells read neutral (`0.5`). A flat planet (no density spread) is all neutral.
fn buoyancy_field(cells: &[HexState], tables: &Tables) -> Vec<f32> {
    let dens: Vec<f32> = cells.iter().map(|c| mean_density(&c.composition, tables)).collect();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &d in &dens {
        if d > 0.0 {
            lo = lo.min(d);
            hi = hi.max(d);
        }
    }
    if hi <= lo {
        return vec![0.5; cells.len()];
    }
    dens.iter()
        .map(|&d| if d <= 0.0 { 0.5 } else { 1.0 - (d - lo) / (hi - lo) })
        .collect()
}

/// The per-cell **heat source** that seeds convection: composition buoyancy PLUS the
/// radiogenic heat the cell's own K/U produce (normalized `0..1` across the planet and
/// scaled by [`RADIOGENIC_HEAT_WEIGHT`]). Heat and material distribution are now two reads
/// of the one molten layer — a radioactive-rich cell runs hot even when it is not the
/// lightest. With no radiogenic material anywhere, this is exactly [`buoyancy_field`].
fn heat_source_field(cells: &[HexState], tables: &Tables) -> Vec<f32> {
    let buoy = buoyancy_field(cells, tables);
    let radio: Vec<f32> = cells.iter().map(|c| cell_radiogenic_heat(&c.composition)).collect();
    let hi = radio.iter().copied().fold(0.0f32, f32::max);
    if hi <= 0.0 {
        return buoy; // no K/U anywhere → pure buoyancy, unchanged
    }
    buoy.iter()
        .zip(&radio)
        .map(|(&b, &r)| b + RADIOGENIC_HEAT_WEIGHT * (r / hi))
        .collect()
}

/// One neighbour-average diffusion pass over a scalar field (blend fraction `k`).
fn diffuse(field: &[f32], ctx: &EpochCtx, k: f32) -> Vec<f32> {
    let n = field.len();
    let mut out = field.to_vec();
    for i in 0..n {
        let (mut sum, mut cnt) = (0.0f32, 0.0f32);
        for &nb in &ctx.neighbors[i] {
            sum += field[nb as usize];
            cnt += 1.0;
        }
        if cnt > 0.0 {
            out[i] = field[i] + k * (sum / cnt - field[i]);
        }
    }
    out
}

/// Advect each cell's composition one step down the heat gradient (hot → its coldest
/// neighbour): the surface limb of convection, carrying dense material toward the cold
/// downwelling sinks. `vigor` scales the transport this step (the cooling clock's
/// fierce-early/gentle-late stir; `1.0` = the old uniform strength). Conserved (a
/// transfer read from the original grid, double-buffered).
fn advect_down_heat(cells: &mut [HexState], ctx: &EpochCtx, heat: &[f32], vigor: f32) {
    let n = cells.len();
    let mut next: Vec<Composition> = cells.iter().map(|c| c.composition.clone()).collect();
    for i in 0..n {
        // Coldest neighbour (steepest heat descent); none colder ⇒ i is a downwelling
        // sink (a local minimum) and simply accumulates.
        let mut coldest = None;
        let mut cold_h = heat[i];
        for &nb in &ctx.neighbors[i] {
            let j = nb as usize;
            if heat[j] < cold_h {
                cold_h = heat[j];
                coldest = Some(j);
            }
        }
        let Some(j) = coldest else { continue };
        let frac = ((heat[i] - cold_h) * CONVECT_ADVECT * vigor).clamp(0.0, MAX_FRAC) as f64;
        if frac <= 0.0 {
            continue;
        }
        for (el, amount) in cells[i].composition.iter() {
            let taken = next[i].remove(el, amount * frac);
            next[j].add(el, taken);
        }
    }
    for (i, c) in cells.iter_mut().enumerate() {
        c.composition = std::mem::take(&mut next[i]);
    }
}

/// Run the molten convection for `steps` passes at **uniform vigor** (the pre-cooling
/// behaviour): self-organize the buoyancy into convection cells, advect the melt within
/// them, and write the resulting [`heat`](HexState::heat) field (renormalised to `0..1`
/// for a full-contrast map). Conserves total element mass. The one-shot pipeline and the
/// tests use this; the cooling engine uses [`run_molten_convection_cooling`].
pub fn run_molten_convection(cells: &mut [HexState], ctx: &EpochCtx, steps: u32) {
    run_molten_convection_cooling(cells, ctx, steps, &[]);
}

/// As [`run_molten_convection`], but the stir strength each step is scaled by `vigor`
/// — the cooling clock's per-step convection vigor (`vigor[s]` for step `s`; a missing
/// or empty entry falls back to `1.0`, i.e. the uniform behaviour). Fierce while molten,
/// gentle as the planet cools; [`crate::cooling::convection_vigor`] builds the schedule
/// mean-preserving so the total stir over the default molten era matches the uniform
/// pass. Still conserves total element mass (only the transport rate changes).
pub fn run_molten_convection_cooling(
    cells: &mut [HexState],
    ctx: &EpochCtx,
    steps: u32,
    vigor: &[f32],
) {
    let n = cells.len();
    if n == 0 {
        return;
    }
    // Seed the heat from the material buoyancy, smoothed to cell scale.
    let mut heat = heat_source_field(cells, ctx.tables);
    for _ in 0..HEAT_SEED_SMOOTH {
        heat = diffuse(&heat, ctx, HEAT_DIFFUSE);
    }

    // `steps == 0` leaves the raw buoyancy seed (the scrubber's t=0, before convecting).
    for s in 0..steps {
        let v = vigor.get(s as usize).copied().unwrap_or(1.0);
        // Surface flow carries melt from the hot rises toward the cold sinks.
        advect_down_heat(cells, ctx, &heat, v);
        // Buoyancy feedback: re-read the (moved) material and pull heat toward it, so a
        // cold sink that gathered dense material cools further (deepens) and a depleted
        // rise heats further — the cells sharpen instead of blurring away.
        let buoy = heat_source_field(cells, ctx.tables);
        for (h, b) in heat.iter_mut().zip(buoy) {
            *h += HEAT_FEEDBACK * (b - *h);
        }
        // Diffuse to keep the cells coherent (and set their wavelength with the feedback).
        heat = diffuse(&heat, ctx, HEAT_DIFFUSE);
    }

    // Renormalise to 0..1 so the heat map keeps full contrast regardless of tuning.
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &h in &heat {
        lo = lo.min(h);
        hi = hi.max(h);
    }
    let span = (hi - lo).max(1e-6);
    for (i, c) in cells.iter_mut().enumerate() {
        c.heat = (heat[i] - lo) / span;
    }
}

/// Swap each cell's **mantle-layer composition** with its flat `composition` slot, so the
/// shared convection algorithm operates on the column mantle — the single material truth.
/// Call again to swap back; cells with no mantle layer are left as-is.
fn swap_mantle_comp(cells: &mut [HexState]) {
    for c in cells {
        if let Some(m) = c.column.mantle_mut() {
            std::mem::swap(&mut c.composition, &mut m.composition);
        }
    }
}

/// Copy each cell's convection heat (`HexState::heat`, the viewer's read) into its mantle
/// layer, so the layer carries the same heat the flow produced.
fn mirror_heat_to_mantle(cells: &mut [HexState]) {
    for c in cells {
        let h = c.heat;
        if let Some(m) = c.column.mantle_mut() {
            m.heat = h;
        }
    }
}

/// Set each cell's mantle-layer **motion** to the convection flow — the tangent direction the
/// melt drifts, down the heat gradient toward the cold downwellings (the same direction
/// [`advect_down_heat`] carries material), so the mantle layer carries a live motion field.
fn mirror_flow_to_mantle(cells: &mut [HexState], ctx: &EpochCtx) {
    let heat: Vec<f32> = cells.iter().map(|c| c.heat).collect();
    for i in 0..cells.len() {
        let pi = ctx.dirs[i];
        let mut flow = Vec3::ZERO;
        for &j in &ctx.neighbors[i] {
            let j = j as usize;
            let dh = heat[i] - heat[j]; // positive → neighbour j is colder → melt flows toward it
            if dh > 0.0 {
                let d = ctx.dirs[j] - pi;
                flow += dh * (d - pi * d.dot(pi)).normalize_or_zero();
            }
        }
        if let Some(m) = cells[i].column.mantle_mut() {
            m.motion = flow;
        }
    }
}

/// **Seed the convection heat field** from the column mantle's buoyancy, smoothed to cell
/// scale, written into the mantle layer and mirrored to `HexState::heat` for the viewer. The
/// tick-sim calls this once when convection activates; thereafter [`convection_step`] evolves
/// the persisted field one pass per tick.
pub fn seed_convection_heat(cells: &mut [HexState], ctx: &EpochCtx) {
    if cells.is_empty() {
        return;
    }
    swap_mantle_comp(cells);
    let mut heat = heat_source_field(cells, ctx.tables);
    for _ in 0..HEAT_SEED_SMOOTH {
        heat = diffuse(&heat, ctx, HEAT_DIFFUSE);
    }
    for (c, h) in cells.iter_mut().zip(heat) {
        c.heat = h;
    }
    swap_mantle_comp(cells);
    mirror_heat_to_mantle(cells);
}

/// **One convection pass** on the column's **mantle layer** — the single material truth. The
/// mantle composition is swapped into the flat convection slot, advected down its persisted
/// heat gradient at `vigor`, sharpened by the buoyancy + radiogenic feedback, then swapped
/// back — so the molten material actually moves between cells' mantles, conserved. The heat
/// is renormalised to `0..1`, written to `HexState::heat`, and mirrored into the mantle
/// layer. The tick-sim steps this each tick with `vigor` ∝ the live thermal state.
pub fn convection_step(cells: &mut [HexState], ctx: &EpochCtx, vigor: f32) {
    if cells.is_empty() {
        return;
    }
    swap_mantle_comp(cells);
    let mut heat: Vec<f32> = cells.iter().map(|c| c.heat).collect();
    advect_down_heat(cells, ctx, &heat, vigor);
    let buoy = buoyancy_field(cells, ctx.tables);
    for (h, b) in heat.iter_mut().zip(buoy) {
        *h += HEAT_FEEDBACK * (b - *h);
    }
    heat = diffuse(&heat, ctx, HEAT_DIFFUSE);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &h in &heat {
        lo = lo.min(h);
        hi = hi.max(h);
    }
    let span = (hi - lo).max(1e-6);
    for (i, c) in cells.iter_mut().enumerate() {
        c.heat = (heat[i] - lo) / span;
    }
    swap_mantle_comp(cells);
    mirror_heat_to_mantle(cells);
    mirror_flow_to_mantle(cells, ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use glam::Vec3;

    const O: flicker_materials::ElementId = 8;
    const FE: flicker_materials::ElementId = 26;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// An equatorial ring.
    fn ring(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let dirs = (0..n)
            .map(|k| {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors =
            (0..n).map(|k| vec![((k + n - 1) % n) as u32, ((k + 1) % n) as u32]).collect();
        (dirs, neighbors)
    }

    #[test]
    fn convection_conserves_mass_and_moves_material() {
        let t = tables();
        let (dirs, neighbors) = ring(8);
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let mut cells: Vec<HexState> = (0..8).map(|_| HexState::new(Composition::new())).collect();
        cells[0].composition.add(O, 100.0);
        cells[3].composition.add(FE, 40.0);
        let before: f64 = cells.iter().map(|c| c.composition.total()).sum();

        run_molten_convection(&mut cells, &ctx, 40);

        let after: f64 = cells.iter().map(|c| c.composition.total()).sum();
        assert!((after - before).abs() < 1e-9, "molten convection conserves mass: {after} vs {before}");
        // The melt actually flowed — the light oxygen source no longer holds all of it.
        assert!(cells[0].composition.amount(O) < 100.0, "material didn't convect off its source cell");
    }

    #[test]
    fn heat_is_cold_over_dense_material_and_hot_over_light() {
        let t = tables();
        let n = 16;
        let (dirs, neighbors) = ring(n);
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 7 };
        // Half the ring dense (iron), half light (silica) — a clear buoyancy contrast.
        let mut cells: Vec<HexState> = (0..n)
            .map(|i| {
                let el = if i < n / 2 { FE } else { 14u8 }; // Fe dense, Si light
                HexState::new(Composition::from_iter([(el, 1000.0)]))
            })
            .collect();

        run_molten_convection(&mut cells, &ctx, 20);

        // The heat field is written and normalised into range.
        assert!(cells.iter().all(|c| (0.0..=1.0).contains(&c.heat)));
        // Downwelling sits over the dense (iron) half, upwelling over the light (silica)
        // half: the mean heat is lower where the heavy material started.
        let dense_heat: f32 = cells[..n / 2].iter().map(|c| c.heat).sum::<f32>() / (n / 2) as f32;
        let light_heat: f32 = cells[n / 2..].iter().map(|c| c.heat).sum::<f32>() / (n / 2) as f32;
        assert!(
            dense_heat < light_heat,
            "convection should run cold over dense material, hot over light ({dense_heat} vs {light_heat})"
        );
    }

    #[test]
    fn deterministic_for_a_seed() {
        let t = tables();
        let (dirs, neighbors) = ring(12);
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 3 };
        let base: Vec<HexState> = (0..12)
            .map(|i| HexState::new(Composition::from_iter([(if i % 2 == 0 { FE } else { O }, 500.0)])))
            .collect();
        let mut a = base.clone();
        let mut b = base;
        run_molten_convection(&mut a, &ctx, 15);
        run_molten_convection(&mut b, &ctx, 15);
        assert_eq!(a, b, "molten convection must be deterministic");
    }

    #[test]
    fn radiogenics_reshape_the_heat_source() {
        let t = tables();
        // A real density spread (so buoyancy is non-trivial) with uranium concentrated in
        // the first three cells. The heat source must differ from pure buoyancy —
        // radiogenics are part of the molten heat map now, not just the global clock — and
        // every radiogenic cell reads at least as hot as buoyancy alone.
        let cells: Vec<HexState> = (0..8)
            .map(|i| {
                let base = if i % 2 == 0 { FE } else { 14u8 };
                let mut c = Composition::from_iter([(base, 1000.0)]);
                if i < 3 {
                    c.add(92u8, 40.0); // uranium
                }
                HexState::new(c)
            })
            .collect();
        let buoy = buoyancy_field(&cells, &t);
        let src = heat_source_field(&cells, &t);
        assert_ne!(buoy, src, "radiogenics must change the heat source field");
        for i in 0..3 {
            assert!(src[i] >= buoy[i], "radiogenic cell {i} must be ≥ its buoyancy-only heat");
        }
    }

    #[test]
    fn convection_stirs_the_column_mantle_and_leaves_the_flat_field_frozen() {
        let t = tables();
        let (dirs, neighbors) = ring(8);
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        // Seed the mantle via HexState::new (it builds the primordial column from the comp):
        // a dense-iron half and a light-silica half — the buoyancy contrast convection organizes.
        let mut cells: Vec<HexState> = (0..8)
            .map(|i| {
                let el = if i < 4 { FE } else { 14u8 };
                HexState::new(Composition::from_iter([(el, 1000.0)]))
            })
            .collect();
        let flat_before: Vec<Composition> = cells.iter().map(|c| c.composition.clone()).collect();
        let mantle_before: Vec<f64> =
            cells.iter().map(|c| c.column.mantle().unwrap().mass()).collect();

        seed_convection_heat(&mut cells, &ctx);
        for _ in 0..20 {
            convection_step(&mut cells, &ctx, 1.0);
        }

        // The MANTLE layer was stirred — material moved between cells' mantles…
        let mantle_after: Vec<f64> =
            cells.iter().map(|c| c.column.mantle().unwrap().mass()).collect();
        assert_ne!(mantle_before, mantle_after, "convection must move material between the mantles");
        // …conserved across the column mantles…
        let (m0, m1): (f64, f64) = (mantle_before.iter().sum(), mantle_after.iter().sum());
        assert!((m0 - m1).abs() < 1e-6, "mantle convection conserves mass ({m0} vs {m1})");
        // …while the flat legacy field is left frozen (the column is the truth)…
        for (c, before) in cells.iter().zip(&flat_before) {
            assert_eq!(&c.composition, before, "the flat composition must be untouched");
        }
        // …and the convection heat was mirrored into the mantle layer.
        assert!(cells.iter().any(|c| c.column.mantle().unwrap().heat > 0.0), "mantle heat was set");
    }
}
