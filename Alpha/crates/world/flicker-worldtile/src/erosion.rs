//! **Per-pixel erosion** — T7, where the maps stop being an interpolation and
//! start being a place.
//!
//! The migration (T6) lays every bed conformably: the honest shape, and a bland
//! one — the interpolation of what the aggregate knew. This module is what cuts
//! into it. Rain gathers downhill across ~3.5 million pixel-clusters, cuts what it
//! can, carries it, and puts it down where the water slackens — and how much it
//! cuts depends on **what it is cutting**, pixel by pixel, bed by bed. Differential
//! resistance is the entire mechanism: erode everything equally and, after
//! normalising, nothing has happened. This is the same law, in the same form, as
//! the aggregate stage (`flicker_poc_chemistry::surface::Erosion`) — the two
//! scales must not disagree about how a landscape forms.
//!
//! # Why this is the payoff
//!
//! A ridge of hard rock in a plain of soft sediment: the plain lowers, pass after
//! pass, and the ridge stands — not because anything raised it, but because
//! everything around it left. A river crossing a layer cake cuts through bed after
//! bed, and the walls of its canyon expose the whole stack. Both are *watched* in
//! the God Mode inspector, never asserted: the tests here pin the mechanism
//! (differential loss, conservation, determinism) and leave the scenery to the
//! maintainer's eye.
//!
//! # The accounting
//!
//! Load is carried in **kilograms**, because beds have different densities and a
//! metre of basalt is not a metre of loose sediment. Every pass reports what it
//! eroded, what it put back down, and what left across the rim — and
//! `Σ(before) = Σ(after) + exported` holds exactly, which is the same trial
//! balance the rest of the simulation answers to.
//!
//! What leaves the rim is **banked, not destroyed**: the caller gets it in the
//! [`PassReport`] and owns handing it to the neighbouring tile or the sea. In the
//! restrained prototype the report is the ledger; the reintegration commit that
//! folds it back into the aggregate world is the batch server's half (T8).

use std::collections::HashMap;

use crate::mask::{TileFrame, TILE_DIM};
use crate::materialize::{Tile, NEIGHBOURS_8};

/// What the erosion needs to know about one stratum: how heavy its rock is and how
/// well it stands up to water. Computed once per tile by the caller (the resistance
/// read lives in the sim crate's rock tier — `bed_resistance` — and the density in
/// its column classifiers; this crate stays pure arithmetic over maps).
#[derive(Clone, Copy, Debug)]
pub struct BedProps {
    /// kg/m³ — converts a metre cut off this bed into mass carried.
    pub density_kg_m3: f64,
    /// `0`..`1` — the outcrop model's input. The cut is DIVIDED by this, so soft
    /// ground goes first, which is the whole reason a landscape develops shape.
    pub resistance: f32,
}

/// The dials of one erosion pass. `Default` is the physics as written — the same
/// posture as the sim's `Levers`.
#[derive(Clone, Copy, Debug)]
pub struct ErosionParams {
    /// Metres cut per pass at unit flow, unit slope, unit resistance.
    pub rate: f64,
    /// How many pixels' worth of freshly-cut ground the stream can hold before it
    /// starts putting it down — dimensionless, because it scales the SAME
    /// `√flow · slope` term the cut uses. Small catchments cut; catchments larger
    /// than this deposit; the transition length is this number, whatever the rate
    /// dial says. (An absolute kg constant here was the first bug this module had:
    /// one pixel's cut overwhelmed it and the entire tile fell into the deposition
    /// regime — the water could never carry what the rain was cutting.)
    pub capacity_factor: f64,
    /// Rain per pixel per pass — the flow each pixel contributes to its catchment.
    pub rain: f64,
    /// Slope (rise over run) above which loose ground will not stand.
    pub talus_slope: f64,
    /// Fraction of the over-steep excess that slumps downhill per pass.
    pub talus_rate: f64,
    /// What pixel-born sediment is, once it lands: loose and soft. New beds laid by
    /// deposition take these props — until reconciliation lithifies them, which is
    /// the aggregate lifecycle's job at reintegration.
    pub sediment: BedProps,
}

impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            rate: 0.02,
            capacity_factor: 800.0,
            rain: 1.0,
            talus_slope: 0.9,
            talus_rate: 0.25,
            sediment: BedProps {
                density_kg_m3: 2200.0,
                resistance: 0.08,
            },
        }
    }
}

/// What one pass did — the ledger the caller reconciles.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassReport {
    /// Mass cut off beds this pass, kg.
    pub eroded_kg: f64,
    /// Mass laid back down as sediment this pass, kg.
    pub deposited_kg: f64,
    /// Mass carried out across the rim, kg — banked for the neighbour or the sea;
    /// the caller owns it now.
    pub exported_kg: f64,
}

/// The erosion engine for one tile. Owns its scratch buffers so a pass allocates
/// nothing — this runs millions of pixels per pass, on a background thread, at a
/// restrained cadence, and the buffers are most of a hundred megabytes.
pub struct Eroder {
    /// Composite height per pixel, m (strata sum; skirt heights live separately).
    height: Vec<f32>,
    /// Where each owned pixel drains.
    downhill: Vec<Drain>,
    /// The height drop to that drain, m (distance-corrected). Zero for a sink.
    drop_m: Vec<f32>,
    /// Owned pixels in draining order (highest first).
    order: Vec<u32>,
    /// Accumulated flow per pixel.
    flow: Vec<f32>,
    /// Sediment load arriving at each pixel, kg.
    load: Vec<f64>,
}

/// Where a pixel's water goes.
#[derive(Clone, Copy, PartialEq)]
enum Drain {
    /// Into a lower owned pixel.
    Into(u32),
    /// Across the rim — the skirt is lower; the water leaves the tile.
    Out,
    /// Nowhere: this pixel is its neighbourhood's low point. A basin floor.
    Sink,
}

impl Eroder {
    pub fn new() -> Self {
        let n = TILE_DIM as usize * TILE_DIM as usize;
        Self {
            height: vec![0.0; n],
            downhill: vec![Drain::Sink; n],
            drop_m: vec![0.0; n],
            order: Vec::new(),
            flow: vec![0.0; n],
            load: vec![0.0; n],
        }
    }

    /// **One pass of rain over the tile.** Route, gather, cut, carry, settle —
    /// then let over-steep ground slump. Deterministic: same tile, same props, same
    /// params → the same tile after, every time.
    pub fn pass(
        &mut self,
        tile: &mut Tile,
        props: &[BedProps],
        params: &ErosionParams,
    ) -> PassReport {
        let dim = TILE_DIM as usize;
        let px_m = tile.frame.pixel_span_m();
        let px_area = tile.frame.pixel_area_m2();
        let mut report = PassReport::default();

        // ── The ground as it stands. ──
        for i in 0..dim * dim {
            self.height[i] = 0.0;
        }
        for map in &tile.strata {
            for (h, &t) in self.height.iter_mut().zip(map.iter()) {
                *h += t;
            }
        }

        // ── Where each pixel drains: its steepest descent, rim and skirt included. ──
        for (x, y) in tile.mask.iter() {
            let i = idx(x, y);
            let here = self.height[i];
            let mut best = Drain::Sink;
            let mut best_drop = 0.0f32;
            for (dx, dy) in NEIGHBOURS_8 {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= dim as i64 || ny >= dim as i64 {
                    continue;
                }
                let (nx, ny) = (nx as u32, ny as u32);
                let dist = if dx != 0 && dy != 0 {
                    std::f32::consts::SQRT_2
                } else {
                    1.0
                };
                let there = if tile.mask.contains(nx, ny) {
                    Some(self.height[idx(nx, ny)])
                } else {
                    tile.skirt.get(&(nx as u16, ny as u16)).copied()
                };
                let Some(there) = there else { continue };
                let drop = (here - there) / dist;
                if drop > best_drop {
                    best_drop = drop;
                    best = if tile.mask.contains(nx, ny) {
                        Drain::Into(idx(nx, ny) as u32)
                    } else {
                        Drain::Out
                    };
                }
            }
            self.downhill[i] = best;
            self.drop_m[i] = best_drop;
        }

        // ── Draining order: highest first, so a trunk carries its whole catchment. ──
        self.order.clear();
        self.order
            .extend(tile.mask.iter().map(|(x, y)| idx(x, y) as u32));
        let height = &self.height;
        self.order.sort_unstable_by(|&a, &b| {
            height[b as usize]
                .partial_cmp(&height[a as usize])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b)) // total order → deterministic
        });

        // ── Gather the rain. ──
        for i in 0..dim * dim {
            self.flow[i] = 0.0;
            self.load[i] = 0.0;
        }
        for &i in &self.order {
            let i = i as usize;
            self.flow[i] += params.rain as f32;
            if let Drain::Into(to) = self.downhill[i] {
                let f = self.flow[i];
                self.flow[to as usize] += f;
            }
        }

        // ── Cut, carry, settle — in the same order the water runs. ──
        for oi in 0..self.order.len() {
            let i = self.order[oi] as usize;
            let mut carrying = std::mem::take(&mut self.load[i]);
            let flow = self.flow[i] as f64;

            // Rise over run to wherever the water goes next — measured against the
            // skirt when it leaves, so the rim is a hillside, not a guess.
            let slope = (self.drop_m[i] as f64 / px_m).max(0.0);

            // The water's carrying capacity here: slack water holds little. Scaled
            // by the same terms as the cut, so "how far a stream carries" is a
            // property of the network, not of the rate dial.
            let capacity = params.capacity_factor
                * params.rate
                * params.sediment.density_kg_m3
                * px_area
                * flow.sqrt()
                * slope;

            if capacity > carrying {
                // Room to cut. Stream power does the cutting; the exposed bed's
                // resistance decides how much of it lands — soft ground first.
                let want_m = params.rate * flow.sqrt() * slope;
                let cut = strip(
                    tile,
                    props,
                    params,
                    i,
                    want_m,
                    (capacity - carrying) / px_area,
                );
                report.eroded_kg += cut;
                carrying += cut;
            } else {
                // Overloaded: the excess settles here as new ground.
                let drop = carrying - capacity;
                let laid = deposit(
                    tile,
                    params,
                    i,
                    drop / (params.sediment.density_kg_m3 * px_area),
                ) * params.sediment.density_kg_m3
                    * px_area;
                report.deposited_kg += laid;
                carrying -= laid;
            }

            match self.downhill[i] {
                Drain::Into(to) => self.load[to as usize] += carrying,
                Drain::Out => report.exported_kg += carrying,
                Drain::Sink => {
                    // The water stops; everything it carried is this basin's floor.
                    let laid = deposit(
                        tile,
                        params,
                        i,
                        carrying / (params.sediment.density_kg_m3 * px_area),
                    ) * params.sediment.density_kg_m3
                        * px_area;
                    report.deposited_kg += laid;
                    // What the f32 floor could not absorb leaves with the books
                    // balanced rather than vanishing into rounding.
                    report.exported_kg += carrying - laid;
                }
            }
        }

        // ── Slopes that will not stand, slump. ──
        //
        // Water carves walls; talus is what keeps them at the angle loose ground
        // can hold. Highest-first again, and each move is mass off the top of the
        // high pixel onto its low neighbour — the same ledger as the water.
        for oi in 0..self.order.len() {
            let i = self.order[oi] as usize;
            if let Drain::Into(to) = self.downhill[i] {
                let to = to as usize;
                let rise = (self.height[i] - self.height[to]) as f64;
                let excess = rise - params.talus_slope * px_m;
                if excess > 0.0 {
                    let slump_m = excess * params.talus_rate;
                    let moved = strip(tile, props, params, i, slump_m, f64::MAX);
                    let laid = deposit(
                        tile,
                        params,
                        to,
                        moved / (params.sediment.density_kg_m3 * px_area),
                    ) * params.sediment.density_kg_m3
                        * px_area;
                    // Slumped ground is moved ground, not lost ground: it shows in
                    // the report's churn but nets to (almost) zero — the f32
                    // shortfall is booked as export so the ledger stays exact.
                    report.eroded_kg += moved;
                    report.deposited_kg += laid;
                    report.exported_kg += moved - laid;
                }
            }
        }

        report
    }
}

impl Default for Eroder {
    fn default() -> Self {
        Self::new()
    }
}

/// Cut up to `want_m` of thickness off the top of the stack at pixel `i`, bed by
/// bed, each bed resisting with its own strength — and never more mass than
/// `cap_kg_per_area · area`. Returns the mass taken.
///
/// The cascade is the canyon mechanism: when the cut goes deeper than the top bed,
/// the next one down is exposed — and if IT is harder, the cut stalls there, which
/// is how a resistant bed comes to hold a whole wall.
fn strip(
    tile: &mut Tile,
    props: &[BedProps],
    params: &ErosionParams,
    i: usize,
    want_m: f64,
    cap_m_at_unit_density: f64,
) -> f64 {
    let px_area = tile.frame.pixel_area_m2();
    let mut budget_m = want_m;
    let mut cap_m = cap_m_at_unit_density;
    let mut taken_kg = 0.0;
    for s in (0..tile.strata.len()).rev() {
        if budget_m <= 0.0 || cap_m <= 0.0 {
            break;
        }
        let t = tile.strata[s][i];
        if t <= 0.0 {
            continue;
        }
        let p = bed_props(props, params, s);
        let resist = f64::from(p.resistance.clamp(0.02, 1.0));
        // This bed yields its share of the budget, slowed by its own resistance.
        let cut = (budget_m / resist).min(t as f64).min(cap_m);
        if cut <= 0.0 {
            break;
        }
        // The ledger records what the MAP records. Thickness is f32 and the plan is
        // f64; book the f32 the map actually lost, or a hundred passes of rounding
        // become a leak the conservation test rightly refuses to excuse.
        let after = (t as f64 - cut) as f32;
        let actual = (t - after) as f64;
        tile.strata[s][i] = after;
        taken_kg += actual * p.density_kg_m3 * px_area;
        // The budget spent is the resisted amount — a hard bed consumes the whole
        // budget to give up a sliver, which is what "standing" means.
        budget_m -= actual * resist;
        cap_m -= actual;
    }
    taken_kg
}

/// Lay `add_m` of sediment on pixel `i`. Pixel-born ground goes on a pixel-born
/// bed: the first deposition anywhere on the tile starts the map, and from then on
/// the tile carries one more stratum than it migrated with — which is the stratum
/// lifecycle happening at pixel scale, and exactly what the reintegration commit
/// reconciles with the column later.
fn deposit(tile: &mut Tile, _params: &ErosionParams, i: usize, add_m: f64) -> f64 {
    if add_m <= 0.0 {
        return 0.0;
    }
    if tile.pixel_born == 0 {
        tile.strata
            .push(vec![0.0; TILE_DIM as usize * TILE_DIM as usize]);
        tile.pixel_born = 1;
    }
    let top = tile.strata.len() - 1;
    // Book what the map takes, not what was offered — the f32 counterpart of the
    // rule in `strip`.
    let before = tile.strata[top][i];
    let after = (before as f64 + add_m) as f32;
    tile.strata[top][i] = after;
    (after - before) as f64
}

/// The props of stratum `s`: a migrated bed reads the caller's table, a pixel-born
/// bed is sediment.
fn bed_props(props: &[BedProps], params: &ErosionParams, s: usize) -> BedProps {
    if s >= props.len() {
        params.sediment
    } else {
        props[s]
    }
}

/// Flat index of a pixel.
fn idx(x: u32, y: u32) -> usize {
    (y as usize) * TILE_DIM as usize + x as usize
}

impl TileFrame {
    /// The span of one pixel on the ground, m — 128 ft, the cluster the pixel is.
    pub fn pixel_span_m(&self) -> f64 {
        crate::mask::TILE_SPAN_M / TILE_DIM as f64
    }
}

/// Total mass standing on the tile, kg, bed by bed — the left side of the pass
/// ledger. Pixel-born beds weigh in at the sediment density.
pub fn tile_mass_kg(tile: &Tile, props: &[BedProps], params: &ErosionParams) -> f64 {
    let px_area = tile.frame.pixel_area_m2();
    tile.strata
        .iter()
        .enumerate()
        .map(|(s, map)| {
            let d = bed_props(props, params, s).density_kg_m3;
            map.iter().map(|&t| t as f64).sum::<f64>() * d * px_area
        })
        .sum()
}

/// Watchable fixtures for the two acceptance scenes. They construct honest tiles —
/// real masks, real frames, uniform skirts — with authored stacks, so the
/// *mechanism* can be watched doing its work in minutes instead of epochs. They
/// assert nothing; the tests pin invariants and the maintainer's eye judges the
/// scenery.
pub mod demo {
    use super::*;
    use crate::mask::HexMask;
    use glam::Vec3;

    /// A neutral frame + mask + skirt for a demo tile: a hex of span `radius_m`,
    /// its skirt at height zero so the rim drains outward.
    fn stage(radius_m: f64) -> (TileFrame, HexMask, HashMap<(u16, u16), f32>) {
        let centre = Vec3::Z;
        // The hexagon must subtend the TILE on this planet: across-flats =
        // `TILE_SPAN_M`, so the circumradius is that over √3 — in tangent-plane
        // units of the radius. Get this wrong and the demo is a toy hexagon lost
        // in the middle of an empty frame (the first cut of this fixture was
        // exactly that, 24× too small, and every probe landed outside the mask).
        let circum = (crate::mask::TILE_SPAN_M / 3f64.sqrt() / radius_m) as f32;
        let outline: Vec<Vec3> = (0..6)
            .map(|k| {
                let a = std::f32::consts::TAU * (k as f32 + 0.5) / 6.0;
                (Vec3::new(a.cos(), a.sin(), 0.0) * circum + centre).normalize()
            })
            .collect();
        let frame = TileFrame::new(0, centre, &outline, radius_m);
        let mask = HexMask::new(&frame, &outline);
        let mut skirt = HashMap::new();
        for (x, y) in mask.iter() {
            for (dx, dy) in NEIGHBOURS_8 {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= TILE_DIM as i64 || ny >= TILE_DIM as i64 {
                    continue;
                }
                let (nx, ny) = (nx as u32, ny as u32);
                if !mask.contains(nx, ny) {
                    skirt.entry((nx as u16, ny as u16)).or_insert(0.0f32);
                }
            }
        }
        (frame, mask, skirt)
    }

    /// **The ridge.** A dike of hard rock stands inside a plain of soft sediment,
    /// and the sediment is filled in AROUND it, up to one smooth sloping surface —
    /// so at the start there is no ridge to see. Rain lowers the plain; over the
    /// dike the cutting stalls a few metres down; the ridge *emerges by
    /// subtraction*. "What actually happened is we eroded away the ground that
    /// we're walking on, not the other way around."
    pub fn ridge(radius_m: f64) -> (Tile, Vec<BedProps>) {
        let (frame, mask, skirt) = stage(radius_m);
        let n = TILE_DIM as usize * TILE_DIM as usize;
        let mut soft = vec![0.0f32; n];
        let mut dike = vec![0.0f32; n];
        for (x, y) in mask.iter() {
            let i = super::idx(x, y);
            let (fx, fy) = (x as f32 / TILE_DIM as f32, y as f32 / TILE_DIM as f32);
            // One smooth surface: a ramp, so the water has a heading.
            let surface = 40.0 + 400.0 * fy;
            // The dike crosses it in an arc, rising to six metres under the ground.
            let band = (fy - 0.5 - 0.15 * (fx * 6.0).sin()).abs();
            if band < 0.02 {
                dike[i] = (surface - 6.0).max(0.0);
            }
            // The sediment is whatever fills the rest of the way up.
            soft[i] = surface - dike[i];
        }
        let tile = Tile {
            cell: 0,
            frame,
            mask,
            strata: vec![dike, soft],
            materials: vec![0, 1],
            skirt,
            pixel_born: 0,
        };
        let props = vec![
            BedProps {
                density_kg_m3: 2900.0,
                resistance: 0.95,
            }, // the dike stands
            BedProps {
                density_kg_m3: 2300.0,
                resistance: 0.10,
            }, // the plain goes
        ];
        (tile, props)
    }

    /// **The canyon.** A layer cake — alternating hard and soft beds — tilted so a
    /// trunk stream must form and cut down through the stack. The walls it leaves
    /// expose bed after bed: the Columbia-gorge walk, in miniature.
    pub fn canyon(radius_m: f64, beds: usize) -> (Tile, Vec<BedProps>) {
        let (frame, mask, skirt) = stage(radius_m);
        let n = TILE_DIM as usize * TILE_DIM as usize;
        let mut strata = Vec::new();
        let mut props = Vec::new();
        for b in 0..beds.max(2) {
            let mut map = vec![0.0f32; n];
            for (x, y) in mask.iter() {
                let i = super::idx(x, y);
                // Every bed drapes the same tilted plain — conformable, like the
                // migration leaves them — with a shallow valley seeded down the
                // middle so the trunk has a heading.
                let (fx, fy) = (x as f32 / TILE_DIM as f32, y as f32 / TILE_DIM as f32);
                let tilt = 30.0 * fy;
                let vee = 8.0 * (fx - 0.5).abs();
                map[i] = 20.0 + (tilt + vee) / beds as f32;
            }
            strata.push(map);
            let hard = b % 2 == 0;
            props.push(BedProps {
                density_kg_m3: if hard { 2800.0 } else { 2250.0 },
                resistance: if hard { 0.85 } else { 0.12 },
            });
        }
        let materials = (0..strata.len() as u8).collect();
        let tile = Tile {
            cell: 0,
            frame,
            mask,
            strata,
            materials,
            skirt,
            pixel_born: 0,
        };
        (tile, props)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small radius so demo tiles have visible slopes per pixel; passes stay
    /// fast. Stated through the span canon (a freq-1 planetoid is ~73 km)
    /// rather than as a stray Earth-radius literal.
    fn r() -> f64 {
        flicker_poc_chemistry::radius_for_freq(1)
    }

    /// **The trial balance, per pass.** Everything cut is carried, dropped, or
    /// exported: `Σ(before) = Σ(after) + exported`, exactly — the same ledger
    /// discipline as every stage on the aggregate side.
    #[test]
    fn a_pass_conserves_every_gram() {
        let (mut tile, props) = demo::ridge(r());
        let params = ErosionParams::default();
        let mut eroder = Eroder::new();
        for pass in 0..5 {
            let before = tile_mass_kg(&tile, &props, &params);
            let report = eroder.pass(&mut tile, &props, &params);
            let after = tile_mass_kg(&tile, &props, &params);
            let books = after + report.exported_kg;
            assert!(
                (books - before).abs() <= 1e-9 * before.max(1.0),
                "pass {pass}: {before:.6e} kg became {after:.6e} + {:.6e} exported (Δ {:.3e})",
                report.exported_kg,
                books - before
            );
        }
    }

    /// Erosion is a function of its inputs: the same tile eroded twice gives the
    /// same maps and the same books, byte for byte.
    #[test]
    fn erosion_is_deterministic() {
        let params = ErosionParams::default();
        let run = || {
            let (mut tile, props) = demo::canyon(r(), 6);
            let mut eroder = Eroder::new();
            let mut exported = 0.0;
            for _ in 0..3 {
                exported += eroder.pass(&mut tile, &props, &params).exported_kg;
            }
            (tile.strata, exported)
        };
        let (a, ea) = run();
        let (b, eb) = run();
        assert_eq!(ea, eb, "the books diverged");
        for (ma, mb) in a.iter().zip(&b) {
            assert_eq!(ma, mb, "the maps diverged");
        }
    }

    /// **The outcrop model at pixel scale**: the same cutting budget applied to the
    /// same pixel takes more off soft rock than hard. This is `strip` measured
    /// directly — the division by resistance with everything else pinned. (A
    /// whole-tile A/B on this is NOT controlled: softening a bed everywhere also
    /// changes every upstream cut, so the measured pixel can flip from a cutting to
    /// a depositing regime and the comparison measures the network, not the rock.)
    #[test]
    fn soft_ground_goes_first() {
        let (tile, _) = demo::ridge(r());
        let params = ErosionParams::default();
        let (x, y) = (TILE_DIM / 2, TILE_DIM / 2);
        let i = super::idx(x, y);
        let want_m = 0.5;

        let mut soft_tile = tile.clone();
        let soft_props = vec![
            BedProps {
                density_kg_m3: 2900.0,
                resistance: 0.95,
            },
            BedProps {
                density_kg_m3: 2300.0,
                resistance: 0.10,
            },
        ];
        let soft_kg = strip(&mut soft_tile, &soft_props, &params, i, want_m, f64::MAX);

        let mut hard_tile = tile.clone();
        let hard_props = vec![
            BedProps {
                density_kg_m3: 2900.0,
                resistance: 0.95,
            },
            BedProps {
                density_kg_m3: 2300.0,
                resistance: 0.90,
            },
        ];
        let hard_kg = strip(&mut hard_tile, &hard_props, &params, i, want_m, f64::MAX);

        assert!(
            soft_kg > hard_kg * 2.0,
            "the same budget took {soft_kg:.3e} kg off soft and {hard_kg:.3e} kg off hard"
        );
    }

    /// **The cascade** — cut past the bottom of a bed and the next one down is
    /// exposed. This is how a canyon wall comes to show the stack: the surface is
    /// not one map, it is whichever bed the water has reached.
    #[test]
    fn cutting_through_a_bed_exposes_the_one_beneath() {
        let (mut tile, props) = demo::canyon(r(), 4);
        let params = ErosionParams {
            rate: 0.4,
            ..Default::default()
        };
        let top = tile.strata.len() - 1;
        let exposed_before: usize = tile
            .mask
            .iter()
            .filter(|&(x, y)| tile.exposed_bed(x, y) == Some(top))
            .count();
        let mut eroder = Eroder::new();
        for _ in 0..30 {
            eroder.pass(&mut tile, &props, &params);
        }
        let exposed_after: usize = tile
            .mask
            .iter()
            .filter(|&(x, y)| tile.exposed_bed(x, y) == Some(top))
            .count();
        assert!(
            exposed_after < exposed_before,
            "thirty passes of hard rain exposed nothing beneath the top bed"
        );
    }

    /// Water leaves across the rim where the outside is lower — the skirt doing its
    /// job. A tile is a piece of a planet: its watershed does not end at its edge,
    /// and mass carried off it is banked for the neighbour, not lost.
    #[test]
    fn the_watershed_drains_across_the_rim() {
        let (mut tile, props) = demo::ridge(r());
        let params = ErosionParams::default();
        let mut exported = 0.0;
        let mut eroder = Eroder::new();
        for _ in 0..4 {
            exported += eroder.pass(&mut tile, &props, &params).exported_kg;
        }
        assert!(
            exported > 0.0,
            "a sloped tile with a low skirt exported nothing"
        );
    }

    /// Deposition grows a pixel-born bed — the stratum lifecycle at pixel scale.
    /// The tile carries one more map than it migrated with, flagged as such for the
    /// reintegration commit to reconcile.
    #[test]
    fn slack_water_lays_a_new_bed() {
        let (mut tile, props) = demo::canyon(r(), 4);
        let migrated = tile.strata.len();
        let params = ErosionParams::default();
        let mut eroder = Eroder::new();
        let mut deposited = 0.0;
        for _ in 0..6 {
            deposited += eroder.pass(&mut tile, &props, &params).deposited_kg;
        }
        assert!(deposited > 0.0, "nothing settled anywhere in six passes");
        assert_eq!(
            tile.strata.len(),
            migrated + 1,
            "pixel-born sediment is its own bed"
        );
        assert_eq!(tile.pixel_born, 1);
    }

    /// **The ridge, measured as a mechanism.** The dike line starts flush with the
    /// plain — the sediment was filled in around it. After rain, the ground over
    /// the dike stands above plain pixels *in the same band of the same slope* —
    /// same water, same gradient, different rock. Emergence by subtraction.
    /// (The scenery itself is the maintainer's to judge in the inspector.)
    #[test]
    fn the_plain_lowers_and_the_dike_does_not_go_with_it() {
        let (mut tile, props) = demo::ridge(r());
        // Hard rain, so the emergence shows within a test's patience; the same
        // physics at the restrained default is what the inspector watches.
        let params = ErosionParams {
            rate: 2.0,
            ..Default::default()
        };

        // Pixels over the dike, and plain pixels JUST beside the band — the same
        // latitudes, so the same catchment and the same slope regime.
        let (mut over_dike, mut beside) = (Vec::new(), Vec::new());
        for (x, y) in tile.mask.iter() {
            let (fx, fy) = (x as f32 / TILE_DIM as f32, y as f32 / TILE_DIM as f32);
            let band = (fy - 0.5 - 0.15 * (fx * 6.0).sin()).abs();
            if band < 0.015 {
                over_dike.push((x, y));
            } else if band > 0.03 && band < 0.05 {
                beside.push((x, y));
            }
        }
        assert!(
            over_dike.len() > 1000 && beside.len() > 1000,
            "the fixture has its bands"
        );
        let mean = |px: &[(u32, u32)], t: &Tile| {
            px.iter()
                .map(|&(x, y)| t.composite_m(x, y) as f64)
                .sum::<f64>()
                / px.len() as f64
        };
        let gap_before = mean(&over_dike, &tile) - mean(&beside, &tile);

        let mut eroder = Eroder::new();
        for _ in 0..25 {
            eroder.pass(&mut tile, &props, &params);
        }
        let gap_after = mean(&over_dike, &tile) - mean(&beside, &tile);
        assert!(
            gap_after > gap_before + 1.0,
            "the dike did not emerge: relief gap went {gap_before:.2} m → {gap_after:.2} m"
        );
    }
}

#[cfg(test)]
mod real_world {
    use super::*;
    use crate::materialize::materialize;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_poc_chemistry::{
        bed_resistance, budget::Budget, config::content_data_dir, density_kg_m3, formation_stages,
        scheduler::Scheduler, World,
    };
    use flicker_worldgrid::icosphere_with_outlines;
    use std::sync::Arc;

    /// **T6 and T7 end to end**: grow a world, walk one cell through the migration
    /// door, and rain on what comes out — with the beds carrying the rock tier's
    /// own reads, exactly as the God Mode path wires them. The books must balance
    /// and the rain must have done *something*; what the ground looks like after is
    /// the maintainer's window, not this test's.
    #[test]
    fn rain_on_a_real_tile_keeps_the_books() {
        let t = Arc::new(
            Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables"),
        );
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let (grid, outlines) = icosphere_with_outlines(6);
        let mut w = World::seed(grid, b, &t, 5);
        let mut s = Scheduler::new(formation_stages(Arc::clone(&t), &w, &Default::default()), 5);
        for _ in 0..150 {
            s.step(&mut w, 1.0, None);
        }
        crate::materialize::guarantee_stack(&mut w);
        let cell = w
            .columns
            .iter()
            .position(|c| c.layers.len() >= 2)
            .expect("the fixture stratified a column");
        let mut tile = materialize(&w, cell, crate::radius_for_freq(6), &outlines[cell]);
        let props: Vec<BedProps> = w.columns[cell]
            .layers
            .iter()
            .map(|bed| BedProps {
                density_kg_m3: density_kg_m3(bed),
                resistance: bed_resistance(&t, bed),
            })
            .collect();

        let params = ErosionParams::default();
        let before = tile_mass_kg(&tile, &props, &params);
        let mut eroder = Eroder::new();
        let mut exported = 0.0;
        let mut churned = 0.0;
        for _ in 0..6 {
            let r = eroder.pass(&mut tile, &props, &params);
            exported += r.exported_kg;
            churned += r.eroded_kg;
        }
        let after = tile_mass_kg(&tile, &props, &params);
        assert!(
            (after + exported - before).abs() <= 1e-9 * before,
            "the books: {before:.6e} → {after:.6e} + {exported:.6e} out"
        );
        assert!(
            churned > 0.0,
            "six passes of rain on a real tile moved nothing at all"
        );
    }
}
