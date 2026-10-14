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
//! `sea`, `wave.amp`, `wave.lobe`, `wave.plateau`, `wave.mask`, `wave.mask.on`,
//! `ridge.on`, `ridge.level`, `ridge.width`, `ridge.amp`, `h<i>.amp`,
//! `h<i>.axes`, `h<i>.mask`, `h<i>.on`, `heat.cells`, `heat.spots`,
//! `emit.on`, `emit.radius`, `emit.arms`, `emit.rings`, `emit.amp`), `--sweep N`
//! (score N seeds, render the best), `--out dir`, `--size WxH`, `--contour C`.
//! Writes `<out>/map_<seed>.png`, `<out>/recipe_<seed>.json`,
//! `<out>/metrics_<seed>.json` and prints the metrics.

use flicker_worldengine::recipe::{compose, metrics, render_equirect, score, Recipe};
use flicker_worldengine::HexMap;

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
        "wave.plateau" => r.wave.plateau = f()?,
        "ridge.on" => r.wave.ridge.on = b()?,
        "ridge.level" => r.wave.ridge.level = f()?,
        "ridge.width" => r.wave.ridge.width = f()?,
        "ridge.amp" => r.wave.ridge.amp = f()?,
        "coast.on" => r.coast.on = b()?,
        "coast.level" => r.coast.level = f()?,
        "coast.width" => r.coast.width = f()?,
        "coast.amp" => r.coast.amp = f()?,
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
        k if k.starts_with('h') => {
            let (idx, rest) = k[1..]
                .split_once('.')
                .ok_or_else(|| format!("{key}: expected h<i>.<field>"))?;
            let i: usize = idx.parse().map_err(|_| format!("{key}: bad index"))?;
            let h = r
                .harmonics
                .get_mut(i)
                .ok_or_else(|| format!("{key}: no harmonic {i}"))?;
            match rest {
                "amp" => h.amp = f()?,
                "axes" => h.axes = u()?,
                "ratio" => h.ratio = f()?,
                "seed" => h.seed = u64v()?,
                "mask" => h.mask.seed = u64v()?,
                "mask.on" => h.mask.on = b()?,
                "on" => h.on = b()?,
                _ => return Err(format!("{key}: unknown harmonic field")),
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
                for (h, f) in r.harmonics.iter_mut().zip(&fresh.harmonics) {
                    h.seed = f.seed;
                }
            }
            apply(r)?
        };
        let c = compose(&map, &r);
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
