//! `resonance_eval` — the headless V5 SHAPE evaluator: compose a field recipe,
//! measure it, de-project it to a flat contour map. No GPU, no window.
//!
//! ```text
//! cargo run -p flicker-worldengine --bin resonance_eval -- --seed 42 --out /tmp/v5
//! cargo run -p flicker-worldengine --bin resonance_eval -- --recipe r.json --out /tmp/v5
//! cargo run -p flicker-worldengine --bin resonance_eval -- --sweep 24 --freq 48 --out /tmp/v5
//! ```
//! Options: `--freq N` (map size, default 48 for speed), `--seed N`, `--recipe
//! path.json` (overrides seed/freq), `--set key=value` (dial overrides:
//! `sea`, `wave.amp`, `wave.lobe`, `wave.seed`, `wave.mask`, `wave.mask.on`,
//! `wave.on`, the stack's `res.spread`, `res.seam`, `res.seed`,
//! `res.mask`, `res.on`, `res.list=f1,f2,…` (rebuild the slots at those
//! frequencies on the spectral law), a slot's `s<i>.freq`, `s<i>.amp`,
//! `s<i>.axes`, `s<i>.on`, `s<i>.mask=off|region|seam`, `heat.cells`, `heat.spots`,
//! `heat.seed`, `heat.on`, `emit.on`, `emit.radius`, `emit.arms`,
//! `emit.rings`, `emit.amp`), `--sweep N`
//! (score N seeds, render the best), `--out dir`, `--size WxH`, `--contour C`,
//! `--globe PX` (also render the shaded ball, PX square), `--look lon,lat`,
//! `--hardness` (also render the hardness channel as a grey map), and
//! `--epochs N` (run N epochs of plate motion, compression and compaction
//! first; `--rate R` tiles per tick, `--rift T` the rift threshold), and
//! `--erode N` (then run N surface cycles of the era — water, air, the
//! hardness-selective cut — adopted over that world; prints the sea's drift,
//! the land share and the land's TEXTURE ratio against the adopted relief),
//! `--impacts R` (strikes per thousand epochs), and `--loop N` (N steps of
//! the FOREVER LOOP after the stages: write the era back into the crust, move
//! `LOOP_EPOCHS`, adopt, cut `LOOP_CYCLES` — through `stage`, which prints
//! its summary).
//! Writes `<out>/map_<seed>.png`, `<out>/recipe_<seed>.json`,
//! `<out>/metrics_<seed>.json` and prints the metrics.
//!
//! **`--seed-sweep N`** — THE WINDOW IS THE JUDGE: N seeds from `--seed` up,
//! each composed and run `--epochs` epochs (300 when none is given), rendered
//! onto ONE CONTACT SHEET (`<out>/sheet_<seed>_<N>.png`): per seed, the globe
//! at epoch zero, the globe after the epochs, and the flat map after the
//! epochs, the seed written in the corner — so the agent sees many worlds at
//! once, as the bench would show them (the same tint, the same hill-shade),
//! and the bench's opening seed is chosen by looking, never by one lucky
//! world. `--sheet-cols C` seeds per row (4), `--sheet-cell PX` the globe
//! size (256).

use flicker_worldengine::recipe::{
    compose, metrics, render_equirect, render_globe, render_hardness, score, Composed, MaskKind,
    Recipe, StackRecipe,
};
use flicker_worldengine::{CrustField, EpochRecipe, Epochs, Evolution, HexMap, SeamField};
use glam::Vec3;

/// The spectral law a `res.list=` rides: relief at frequency 1, pink fall-off.
const LIST_RELIEF: f32 = 0.03;
const LIST_TILT: f32 = 1.0;

fn set(r: &mut Recipe, key: &str, val: &str) -> Result<(), String> {
    let f = || val.parse::<f32>().map_err(|e| format!("{key}: {e}"));
    let u = || val.parse::<u32>().map_err(|e| format!("{key}: {e}"));
    let u64v = || val.parse::<u64>().map_err(|e| format!("{key}: {e}"));
    let b = || match val {
        "1" | "true" | "on" => Ok(true),
        "0" | "false" | "off" => Ok(false),
        _ => Err(format!("{key}: not a bool")),
    };
    match key {
        "sea" => r.sea_percent = f()?,
        "wave.amp" => r.wave.amp = f()?,
        "wave.lobe" => r.wave.lobe = f()?,
        // The resonance STACK's dials and its list.
        "res.spread" => r.stack.spread = f()?,
        "res.seam" => r.stack.seam = f()?,
        "res.seed" => r.stack.seed = u64v()?,
        "res.mask" => r.stack.mask_seed = u64v()?,
        "res.on" => r.stack.on = b()?,
        // `res.list=2,5,31` rebuilds the slots at those frequencies (all on,
        // region-masked, heights on the spectral law at LIST_RELIEF/LIST_TILT)
        // on the stack's current dials and rolls.
        "res.list" => {
            let freqs: Vec<f32> = val
                .split(',')
                .map(|t| t.trim().parse::<f32>().map_err(|e| format!("{key}: {e}")))
                .collect::<Result<_, _>>()?;
            let dials = r.stack.clone();
            let mut stack =
                StackRecipe::over(&freqs, dials.seed, dials.mask_seed, LIST_RELIEF, LIST_TILT);
            stack.spread = dials.spread;
            stack.seam = dials.seam;
            stack.on = dials.on;
            r.stack = stack;
        }
        "wave.seed" => r.wave.seed = u64v()?,
        "wave.mask" => r.wave.mask.seed = u64v()?,
        "wave.mask.on" => r.wave.mask.on = b()?,
        "wave.on" => r.wave.on = b()?,
        "heat.cells" => r.heat.cells = u()?,
        "heat.spots" => r.heat.spots = u()?,
        "heat.seed" => r.heat.seed = u64v()?,
        "heat.on" => r.heat.on = b()?,
        "emit.on" => r.heat.emitters_on = b()?,
        "emit.radius" => r.heat.emitters.radius_tiles = f()?,
        "emit.arms" => r.heat.emitters.arms = u()?,
        "emit.rings" => r.heat.emitters.rings = f()?,
        "emit.amp" => r.heat.emitters.amp = f()?,
        "bubbles.on" => r.bubbles.on = b()?,
        "bubbles.amp" => r.bubbles.amp = f()?,
        "bubbles.largest" => r.bubbles.largest = f()?,
        "bubbles.octaves" => r.bubbles.octaves = u()?,
        "bubbles.tilt" => r.bubbles.tilt = f()?,
        "bubbles.seed" => r.bubbles.seed = u64v()?,
        k if k.starts_with('s') => {
            let (idx, rest) = k[1..]
                .split_once('.')
                .ok_or_else(|| format!("{key}: expected s<i>.<field>"))?;
            let i: usize = idx.parse().map_err(|_| format!("{key}: bad index"))?;
            let slot = r
                .stack
                .slots
                .get_mut(i)
                .ok_or_else(|| format!("{key}: no slot {i}"))?;
            match rest {
                "freq" => slot.frequency = f()?,
                "amp" => slot.amp = f()?,
                "axes" => slot.axes = u()?,
                "on" => slot.on = b()?,
                "mask" => {
                    slot.mask = match val {
                        "off" | "0" | "false" => MaskKind::Off,
                        "region" | "1" | "true" => MaskKind::Region,
                        "seam" | "2" => MaskKind::Seam,
                        _ => return Err(format!("{key}: off | region | seam")),
                    }
                }
                _ => return Err(format!("{key}: unknown slot field")),
            }
        }
        _ => return Err(format!("unknown key `{key}`")),
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (mut freq, mut seed, mut sweep) = (48u32, 42u64, 0usize);
    let mut recipe_path: Option<String> = None;
    let mut out = std::path::PathBuf::from("/tmp/v5");
    let (mut w, mut h, mut contour) = (1024u32, 512u32, 0.01f32);
    let (mut globe, mut look) = (0u32, (30.0f32, 20.0f32));
    let (mut epochs, mut rate, mut rift) = (
        0u32,
        EpochRecipe::default().rate,
        EpochRecipe::default().rift,
    );
    let mut hardness = false;
    let mut erode = 0u32;
    let mut loops = 0u32;
    let mut impacts = EpochRecipe::default().impacts;
    let (mut seed_sweep, mut sheet_cols, mut sheet_cell) = (0usize, 4usize, 256u32);
    let mut sets: Vec<(String, String)> = Vec::new();
    while let Some(a) = args.next() {
        let mut next = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--freq" => freq = next()?.parse()?,
            "--seed" => seed = next()?.parse()?,
            "--sweep" => sweep = next()?.parse()?,
            "--recipe" => recipe_path = Some(next()?),
            "--out" => out = next()?.into(),
            "--contour" => contour = next()?.parse()?,
            "--globe" => globe = next()?.parse()?,
            "--epochs" => epochs = next()?.parse()?,
            "--rate" => rate = next()?.parse()?,
            "--rift" => rift = next()?.parse()?,
            "--hardness" => hardness = true,
            "--erode" => erode = next()?.parse()?,
            "--loop" => loops = next()?.parse()?,
            "--impacts" => impacts = next()?.parse()?,
            "--seed-sweep" => seed_sweep = next()?.parse()?,
            "--sheet-cols" => sheet_cols = next()?.parse::<usize>()?.max(1),
            "--sheet-cell" => sheet_cell = next()?.parse::<u32>()?.max(32),
            "--look" => {
                let s = next()?;
                let (a, b) = s.split_once(',').ok_or("--look lon,lat")?;
                look = (a.parse()?, b.parse()?);
            }
            "--size" => {
                let s = next()?;
                let (a, b) = s.split_once('x').ok_or("--size WxH")?;
                w = a.parse()?;
                h = b.parse()?;
            }
            "--set" => {
                let s = next()?;
                let (k, v) = s.split_once('=').ok_or("--set key=value")?;
                sets.push((k.to_string(), v.to_string()));
            }
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    std::fs::create_dir_all(&out)?;
    let base: Recipe = match &recipe_path {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?,
        None => Recipe::default_at(freq, seed),
    };
    let map = HexMap::new(base.freq);
    let apply = |mut r: Recipe| -> Result<Recipe, String> {
        for (k, v) in &sets {
            set(&mut r, k, v)?;
        }
        Ok(r)
    };
    let epoch_recipe = EpochRecipe {
        rate,
        rift,
        impacts,
        ..EpochRecipe::default()
    };

    if seed_sweep > 0 {
        let epochs = if epochs == 0 { 300 } else { epochs };
        let (lon, lat) = (look.0.to_radians(), look.1.to_radians());
        let dir = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
        let cell = sheet_cell;
        let rows = seed_sweep.div_ceil(sheet_cols);
        let (sw, sh) = (cell * 2 * sheet_cols as u32, cell * 2 * rows as u32);
        let mut sheet = vec![0u8; (sw * sh * 3) as usize];
        for k in 0..seed_sweep {
            let s = seed.wrapping_add(k as u64);
            let r = apply(Recipe::default_at(base.freq, s))?;
            let c0 = compose(&map, &r);
            let m0 = metrics(&map, &c0);
            let mut e = Epochs::new(&map, &r, &c0, &epoch_recipe);
            let last = e.run(&map, &epoch_recipe, epochs);
            let c1 = Composed {
                heights: e.heights().to_vec(),
                sea: c0.sea,
                hardness: e.hardness().to_vec(),
            };
            let m1 = metrics(&map, &c1);
            println!(
                "seed {s:>6}  @0: score {:5.2} land {:4.1}% conts {:2} isl {:3}  @{epochs}: score {:5.2} land {:4.1}% conts {:2} isl {:3} ridges@edge {:4.2} rafts {} largest {:4.2}",
                score(&m0), m0.land_share * 100.0, m0.continents, m0.islands,
                score(&m1), m1.land_share * 100.0, m1.continents, m1.islands, m1.ridges_on_edges, last.rafts, m1.largest_land_share
            );
            let (col, row) = ((k % sheet_cols) as u32, (k / sheet_cols) as u32);
            let (x0, y0) = (col * cell * 2, row * cell * 2);
            blit(
                &mut sheet,
                sw,
                x0,
                y0,
                &render_globe(&map, &c0, cell, dir),
                cell,
                cell,
            );
            blit(
                &mut sheet,
                sw,
                x0 + cell,
                y0,
                &render_globe(&map, &c1, cell, dir),
                cell,
                cell,
            );
            blit(
                &mut sheet,
                sw,
                x0,
                y0 + cell,
                &render_equirect(&map, &c1, cell * 2, cell, contour),
                cell * 2,
                cell,
            );
            draw_digits(&mut sheet, sw, x0 + 4, y0 + 4, &s.to_string(), 3);
        }
        let png: image::RgbImage =
            image::ImageBuffer::from_raw(sw, sh, sheet).ok_or("sheet buffer")?;
        let path = out.join(format!("sheet_{seed}_{seed_sweep}.png"));
        png.save(&path)?;
        println!("sheet: {}", path.display());
        return Ok(());
    }

    let seeds: Vec<u64> = if sweep > 0 {
        (0..sweep as u64).map(|k| seed.wrapping_add(k)).collect()
    } else {
        vec![seed]
    };
    let mut best: Option<(f32, u64)> = None;
    for s in &seeds {
        let r = if recipe_path.is_some() && sweep == 0 {
            apply(base.clone())?
        } else {
            let mut r = Recipe::default_at(base.freq, *s);
            if recipe_path.is_some() {
                // A sweep over a recipe: keep its dials, re-roll every field.
                let fresh = Recipe::default_at(base.freq, *s);
                r = base.clone();
                r.wave.seed = fresh.wave.seed;
                r.heat.seed = fresh.heat.seed;
                r.stack.seed = fresh.stack.seed;
            }
            apply(r)?
        };
        let mut c = compose(&map, &r);
        if loops > 0 {
            // THE FOREVER LOOP — the stages as one call, then the loop.
            let staged = flicker_worldengine::stage(&map, &r, epochs, &epoch_recipe, erode, loops);
            let (eruptions, strikes) = staged.events;
            println!(
                "loop {loops} × ({} epochs + {} cycles) after {epochs} epochs + {erode} cycles: era cycles {}  eruptions {eruptions}  impacts {strikes}",
                flicker_worldengine::LOOP_EPOCHS,
                flicker_worldengine::LOOP_CYCLES,
                staged.era_ticks
            );
            c = staged.world;
        } else if epochs > 0 {
            // The epochs: the stone ocean moving for `epochs` ticks before the
            // reading — rafts from the land masses, floor flowing with the heat.
            let er = epoch_recipe;
            let mut e = Epochs::new(&map, &r, &c, &er);
            let last = e.run(&map, &er, epochs);
            let (born, stacked) = e.census(&map);
            let (eruptions, strikes) = e.events_so_far();
            println!(
                "epochs {epochs}: rafts {}  consumed {}  young {}  welds {}  rifts {}  crust {:.4} = up {:.4} + root {:.4}  tallest {:.4}  parcels {}  born {born}  stacked-tiles {stacked}  abyss {:.4}  eruptions {eruptions}  impacts {strikes}",
                last.rafts, last.consumed, last.opening, last.welds, last.rifts, last.consumed_crust, last.thickened, last.rooted, last.uplifted, e.parcels(), e.abyss()
            );
            c = Composed {
                heights: e.heights().to_vec(),
                sea: c.sea,
                hardness: e.hardness().to_vec(),
            };
        }
        if erode > 0 && loops == 0 {
            // THE ERA over the wave planet: the surface procedures adopted
            // over the world as it stands (composed, or epoched), the cut
            // selective by the hardness ledger.
            let seams = SeamField::new(&map, r.heat.cells, r.heat.spots, r.heat.seed);
            let crust = CrustField::derive(&map, &seams);
            let mut era = Evolution::new(&map, &seams);
            era.adopt(&map, &seams, &c);
            let started = std::time::Instant::now();
            for _ in 0..erode {
                let sea = era.resolve_sea();
                era.tick(&map, &seams, &crust, sea);
            }
            let per = started.elapsed().as_secs_f32() * 1000.0 / erode as f32;
            let eroded = Composed {
                heights: era.relief(),
                sea: era.relief_sea(),
                hardness: era.surface_hardnesses(),
            };
            let land_before = metrics(&map, &c).land_share;
            let land_after = metrics(&map, &eroded).land_share;
            let lost: f32 = c
                .heights
                .iter()
                .zip(&eroded.heights)
                .filter(|(h, _)| **h > c.sea)
                .map(|(h, e)| (h - e).max(0.0))
                .sum::<f32>()
                / c.heights.iter().filter(|h| **h > c.sea).count().max(1) as f32;
            // The sea floor's texture too — the drowned waves must not plane.
            let rough = |h: &[f32], under: bool| {
                let mut sum = 0.0f32;
                let mut k = 0usize;
                for t in 0..map.len() as u32 {
                    if (c.heights[t as usize] <= c.sea) != under {
                        continue;
                    }
                    for nb in map.neighbours(t) {
                        sum += (h[t as usize] - h[*nb as usize]).abs();
                        k += 1;
                    }
                }
                sum / k.max(1) as f32
            };
            let floor_texture = rough(&eroded.heights, true) / rough(&c.heights, true).max(1e-9);
            println!(
                "erode {erode}: sea {:+.4} → {:+.4}  land {:.1}% → {:.1}%  texture land {:.3} floor {:.3}  mean land loss {:.5}  changed {}  {per:.1} ms/cycle",
                c.sea, eroded.sea, land_before * 100.0, land_after * 100.0,
                era.texture_ratio(&map).unwrap_or(1.0), floor_texture, lost, era.changed_tiles()
            );
            // IS THE CUT HONEST ABOUT HARDNESS? The adopted land binned by
            // the ledger it was adopted with, what the era took off each bin
            // — the open question of HEX WORLD DIRECTION, measured.
            print!(
                "hardness honesty over {erode} cycles:\n{}at matched relief:\n{}at matched slope:\n{}{}",
                flicker_worldengine::hardness_cut_report(
                    &flicker_worldengine::hardness_cut_bins(&c, &eroded.heights)
                ),
                flicker_worldengine::hardness_cut_matched_report(
                    "relief",
                    &flicker_worldengine::hardness_cut_matched(&c, &eroded.heights)
                ),
                flicker_worldengine::hardness_cut_matched_report(
                    "slope",
                    &flicker_worldengine::hardness_cut_matched_slope(&map, &c, &eroded.heights)
                ),
                flicker_worldengine::hardness_cut_per_slope_report(
                    &flicker_worldengine::hardness_cut_per_slope(&map, &c, &eroded.heights)
                )
            );
            c = eroded;
        }
        {
            // The relief's span against the sea: the floor's percentiles and
            // the summit — a spike a stacked column raised shows here as a
            // max far above the 99.5th percentile.
            let mut sorted: Vec<f32> = c.heights.to_vec();
            sorted.sort_by(f32::total_cmp);
            let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q).round() as usize] - c.sea;
            let towers = sorted
                .iter()
                .filter(|h| **h - c.sea > 3.0 * at(0.995))
                .count();
            println!(
                "span vs sea: min {:+.4}  p0.5 {:+.4}  p5 {:+.4}  p50 {:+.4}  p95 {:+.4}  p99.5 {:+.4}  max {:+.4}  towers>3xp99.5 {}",
                at(0.0), at(0.005), at(0.05), at(0.5), at(0.95), at(0.995), at(1.0), towers
            );
        }
        let m = metrics(&map, &c);
        let sc = score(&m);
        println!(
            "seed {s:>6}  score {sc:6.2}  land {:5.1}%  continents {:2}  islands {:3}  largest {:4.2}  coast {:4.2}  ridges@edge {:4.2}  seabed {:4.2}  basins {:2}",
            m.land_share * 100.0,
            m.continents,
            m.islands,
            m.largest_land_share,
            m.coast_per_land,
            m.ridges_on_edges,
            m.seabed_relief,
            m.basins
        );
        if best.is_none_or(|(b, _)| sc < b) {
            best = Some((sc, *s));
        }
        if sweep == 0 || Some((sc, *s)) == best {
            let img = render_equirect(&map, &c, w, h, contour);
            let png: image::RgbImage =
                image::ImageBuffer::from_raw(w, h, img).ok_or("image buffer")?;
            png.save(out.join(format!("map_{s}.png")))?;
            if globe > 0 {
                let (lon, lat) = (look.0.to_radians(), look.1.to_radians());
                let dir = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
                let img = render_globe(&map, &c, globe, dir);
                let png: image::RgbImage =
                    image::ImageBuffer::from_raw(globe, globe, img).ok_or("globe buffer")?;
                png.save(out.join(format!("globe_{s}.png")))?;
            }
            if hardness {
                // The hardness channel, de-projected: soft dark → hard bright,
                // the shoreline drawn so the land reads against the floor.
                let img = render_hardness(&map, &c, w, h);
                let png: image::RgbImage =
                    image::ImageBuffer::from_raw(w, h, img).ok_or("hardness buffer")?;
                png.save(out.join(format!("hard_{s}.png")))?;
                let mut sorted = c.hardness.clone();
                sorted.sort_by(f32::total_cmp);
                let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q).round() as usize];
                println!(
                    "hardness: min {:.2}  p5 {:.2}  p50 {:.2}  p95 {:.2}  max {:.2}",
                    at(0.0),
                    at(0.05),
                    at(0.5),
                    at(0.95),
                    at(1.0)
                );
            }
            std::fs::write(
                out.join(format!("recipe_{s}.json")),
                serde_json::to_string_pretty(&r)?,
            )?;
            std::fs::write(
                out.join(format!("metrics_{s}.json")),
                serde_json::to_string_pretty(&m)?,
            )?;
        }
    }
    if let Some((sc, s)) = best {
        println!(
            "best: seed {s} score {sc:.2} → {}",
            out.join(format!("map_{s}.png")).display()
        );
    }
    Ok(())
}

/// Paste an RGB image `w × h` into the sheet at `(x0, y0)`.
fn blit(sheet: &mut [u8], sheet_w: u32, x0: u32, y0: u32, img: &[u8], w: u32, h: u32) {
    for y in 0..h {
        let src = ((y * w) * 3) as usize;
        let dst = (((y0 + y) * sheet_w + x0) * 3) as usize;
        let n = (w * 3) as usize;
        if dst + n <= sheet.len() && src + n <= img.len() {
            sheet[dst..dst + n].copy_from_slice(&img[src..src + n]);
        }
    }
}

/// A 3×5 digit font, one row per byte's low three bits.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Write `text`'s digits in white on a black box at `(x0, y0)`, each pixel
/// of the font `scale` pixels wide.
fn draw_digits(sheet: &mut [u8], sheet_w: u32, x0: u32, y0: u32, text: &str, scale: u32) {
    let sheet_h = sheet.len() as u32 / 3 / sheet_w.max(1);
    let mut put = |x: u32, y: u32, v: u8| {
        if x < sheet_w && y < sheet_h {
            let i = ((y * sheet_w + x) * 3) as usize;
            sheet[i] = v;
            sheet[i + 1] = v;
            sheet[i + 2] = v;
        }
    };
    let glyph_w = 4 * scale;
    let box_w = text.len() as u32 * glyph_w + scale;
    let box_h = 7 * scale;
    for y in 0..box_h {
        for x in 0..box_w {
            put(x0 + x, y0 + y, 0);
        }
    }
    for (k, ch) in text.chars().enumerate() {
        let Some(d) = ch.to_digit(10) else { continue };
        let glyph = DIGITS[d as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3u32 {
                if bits & (0b100 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            put(
                                x0 + scale + k as u32 * glyph_w + col * scale + dx,
                                y0 + scale + row as u32 * scale + dy,
                                255,
                            );
                        }
                    }
                }
            }
        }
    }
}
