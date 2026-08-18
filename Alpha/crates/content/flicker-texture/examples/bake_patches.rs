//! Bake every factory patch to PNG — the instrument run for real, headless.
//!
//! ```text
//! cargo run -p flicker-texture --example bake_patches -- <out_dir> [size]   # size defaults to 2K
//! ```
//!
//! Writes `<Name>_<Map>.png` per the content standard's map-role naming, checks
//! each swatch's seam, and prints what it wrote. This is the offline face of the
//! same [`flicker_texture::bake`] the bench calls every frame — if a surface
//! looks wrong in the window, it is wrong here too, without a GPU in the way.

use flicker_texture::{bake, presets, BAKE_DEFAULT};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let size: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(BAKE_DEFAULT);
    std::fs::create_dir_all(&dir)?;

    for recipe in presets::all() {
        let set = bake(&recipe, size);
        // The seam check, on the bytes that are about to be written: the worst
        // step across the wrap must not exceed the worst step anywhere inside.
        let mut worst = None;
        for map in &set.maps {
            let px = |x: u32, y: u32| {
                let i = ((y * size + x) * 4) as usize;
                &map.pixels[i..i + 3]
            };
            let step = |a: &[u8], b: &[u8]| -> i32 {
                a.iter()
                    .zip(b)
                    .map(|(p, q)| (*p as i32 - *q as i32).abs())
                    .max()
                    .unwrap_or(0)
            };
            let interior = (0..size)
                .flat_map(|y| (0..size - 1).map(move |x| (x, y)))
                .map(|(x, y)| step(px(x, y), px(x + 1, y)))
                .max()
                .unwrap_or(0);
            let seam = (0..size)
                .map(|y| step(px(size - 1, y), px(0, y)))
                .max()
                .unwrap_or(0);
            if seam > interior {
                worst = Some(format!("{:?} seam {seam} > interior {interior}", map.kind));
            }

            let name = format!("{}_{}.png", recipe.name, map.kind.role());
            image::save_buffer(
                dir.join(&name),
                &map.pixels,
                size,
                size,
                image::ColorType::Rgba8,
            )?;
        }
        match worst {
            Some(bad) => println!("{:<10} {size}²  ✗ {bad}", recipe.name),
            None => println!(
                "{:<10} {size}²  seamless  {} maps  material {:?}",
                recipe.name,
                set.maps.len(),
                recipe.material
            ),
        }
    }
    println!("\nwrote to {}", dir.display());
    Ok(())
}
