//! `ground_eval` — the headless V5 GROUND evaluator: one hex of the wave
//! planet at 2K, through the migration ([`flicker_worldtile::materialize`])
//! over the V5 source, written as a shaded PNG and measured. The judge's eyes
//! for the six-inch end, beside `resonance_eval`'s for the ball.
//!
//! ```text
//! cargo run --release -p flicker-worldtile --bin ground_eval -- --seed 49 --freq 96 \
//!     --epochs 300 --erode 200 --look 30,20 --out /tmp/v5
//! cargo run --release -p flicker-worldtile --bin ground_eval -- --recipe r.json --cell 1234
//! cargo run --release -p flicker-worldtile --bin ground_eval -- --seed 49 --cut 20
//! cargo run --release -p flicker-worldtile --bin ground_eval -- --seed 49 --cluster 1024,1024 --lod 0
//! cargo run --release -p flicker-worldtile --bin ground_eval -- --seed 49 --cut 20 --field
//! ```
//! `--recipe path.json` (a recipe `resonance_eval` wrote) or `--seed`/`--freq`;
//! `--epochs N` / `--erode N` run the stages first (`flicker_worldengine::stage`);
//! the hex is `--cell C`, or the land hex nearest `--look lon,lat` (degrees),
//! or the highest land hex; `--scale S` the metres lever (default
//! [`V5_RELIEF_SCALE`]). Writes `<out>/ground_<seed>_<cell>.png` (the 2K map
//! hill-shaded, height-tinted, corners black) and prints the hex's column
//! height, the pixels' mean and spread, the pixel-scale roughness, the seam
//! agreement with every neighbour, and the time. `--field` hands the hex to
//! the engine as its QUAD FIELD (one 128 ft quad a pixel at its LOD8 vector,
//! the drops between neighbours as walls — `flicker_voxel::QuadField`),
//! prints its counts and writes two pictures of the field's own meshes,
//! software-drawn: `field_<seed>_<cell>.png`, the whole hex from above, and
//! `field_<seed>_<cell>_close.png`, a few dozen quads around `--cluster` (or
//! the middle) from an oblique eye, where the drops read as lit walls.

use flicker_voxel::FieldRegion;
use flicker_worldengine::{stage, EpochRecipe, HexMap, Recipe, TileId};
use flicker_worldtile::{quad_field, ResonanceSource, TileSource, TILE_DIM, V5_RELIEF_SCALE};
use glam::{Vec3, Vec3A};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (mut freq, mut seed) = (96u32, Recipe::OPENING_SEED);
    let mut recipe_path: Option<String> = None;
    let (mut epochs, mut erode, mut loops) = (0u32, 0u32, 0u32);
    let mut cell: Option<u32> = None;
    let mut cut = 0u32;
    let mut cluster: Option<(u32, u32)> = None;
    let mut lod = 8u8;
    let mut field = false;
    let mut look: Option<(f32, f32)> = None;
    let mut scale = V5_RELIEF_SCALE;
    let mut out = std::path::PathBuf::from("/tmp/v5");
    while let Some(a) = args.next() {
        let mut next = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--freq" => freq = next()?.parse()?,
            "--seed" => seed = next()?.parse()?,
            "--recipe" => recipe_path = Some(next()?),
            "--epochs" => epochs = next()?.parse()?,
            "--erode" => erode = next()?.parse()?,
            "--loop" => loops = next()?.parse()?,
            "--cell" => cell = Some(next()?.parse()?),
            "--cut" => cut = next()?.parse()?,
            "--cluster" => {
                let s = next()?;
                let (a, b) = s.split_once(',').ok_or("--cluster x,y")?;
                cluster = Some((a.trim().parse()?, b.trim().parse()?));
            }
            "--lod" => lod = next()?.parse()?,
            "--field" => field = true,
            "--scale" => scale = next()?.parse()?,
            "--out" => out = next()?.into(),
            "--look" => {
                let s = next()?;
                let (a, b) = s.split_once(',').ok_or("--look lon,lat")?;
                look = Some((a.parse()?, b.parse()?));
            }
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    std::fs::create_dir_all(&out)?;
    let recipe: Recipe = match &recipe_path {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?,
        None => Recipe::default_at(freq, seed),
    };
    let map = HexMap::new(recipe.freq);
    let started = std::time::Instant::now();
    let staged = stage(&map, &recipe, epochs, &EpochRecipe::default(), erode, loops);
    let world = &staged.world;
    println!(
        "staged in {:.1}s: epochs {epochs}, era cycles {}",
        started.elapsed().as_secs_f32(),
        staged.era_ticks
    );
    // The hex: asked, or the land hex nearest the look, or the highest land.
    let cell = cell.unwrap_or_else(|| {
        let land = |t: TileId| world.heights[t as usize] > world.sea;
        match look {
            Some((lon, lat)) => {
                let (lon, lat) = (lon.to_radians(), lat.to_radians());
                let dir = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
                map.tiles()
                    .filter(|t| land(*t))
                    .max_by(|a, b| {
                        map.direction(*a)
                            .dot(dir)
                            .total_cmp(&map.direction(*b).dot(dir))
                    })
                    .unwrap_or(0)
            }
            None => map
                .tiles()
                .max_by(|a, b| world.heights[*a as usize].total_cmp(&world.heights[*b as usize]))
                .unwrap_or(0),
        }
    }) as usize;
    let src = ResonanceSource::new(&map, &recipe, world, staged.sediment.as_deref(), scale);
    let started = std::time::Instant::now();
    let mut tile = src.materialize(cell);
    let took = started.elapsed().as_secs_f32();
    if cut > 0 {
        // THE WATER: the pixel eroder over the hex, by the hex's hardness,
        // nothing under the sea line — a pass at a time, each one's ledger
        // and the ground's span printed, so a runaway shows where it starts.
        let started = std::time::Instant::now();
        for k in 1..=cut {
            let r = src.cut(&mut tile, 1);
            if k % 10 != 0 && k != cut && k != 1 {
                continue;
            }
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for (x, y) in tile.mask.iter() {
                let h = tile.composite_m(x, y);
                lo = lo.min(h);
                hi = hi.max(h);
            }
            println!(
                "cut {k}: eroded {:.4} Mt  deposited {:.4} Mt  exported {:.4} Mt  lakes {}  outlets {}  span {lo:.1}–{hi:.1} m  beds {}",
                r.eroded_kg * 1e-9,
                r.deposited_kg * 1e-9,
                r.exported_kg * 1e-9,
                r.lake_px,
                r.outlet_px,
                tile.strata.len(),
            );
        }
        println!(
            "cut {cut} passes in {:.1}s",
            started.elapsed().as_secs_f32()
        );
    }
    // Measure.
    let (mut lo, mut hi, mut sum, mut n) = (f32::MAX, f32::MIN, 0.0f64, 0usize);
    let (mut rough, mut pairs) = (0.0f64, 0usize);
    for (x, y) in tile.mask.iter() {
        let h = tile.composite_m(x, y);
        lo = lo.min(h);
        hi = hi.max(h);
        sum += f64::from(h);
        n += 1;
        if x + 1 < TILE_DIM && tile.mask.contains(x + 1, y) {
            rough += f64::from((tile.composite_m(x + 1, y) - h).abs());
            pairs += 1;
        }
    }
    let mean = sum / n.max(1) as f64;
    let column = src.thickness_m(cell);
    let sea = src.sea_level_m();
    println!(
        "hex {cell}: column {column:.1} m (sea {sea:.1} m)  pixels {n}  mean {mean:.1} m  span {lo:.1}–{hi:.1} m  roughness {:.3} m/px  beds {}  {took:.1}s",
        rough / pairs.max(1) as f64,
        tile.strata.len()
    );
    // THE LADDER under a pixel: the cluster's vectors at the asked LOD.
    if let Some((cx, cy)) = cluster {
        for level in [8u8, lod] {
            let v = src.cluster_lod(&tile, cx, cy, level);
            let (lo, hi) = v.span();
            let n = v.n();
            let mut rough = 0.0f64;
            for row in 0..n {
                for col in 1..n {
                    rough += f64::from(
                        (v.offsets_m[row * n + col] - v.offsets_m[row * n + col - 1]).abs(),
                    );
                }
            }
            println!(
                "cluster ({cx},{cy}) LOD{level}: {n}×{n} vectors over the LOD8 vector {:.4} m (pixel {:.4} m)  offsets {lo:+.4}–{hi:+.4} m  roughness {:.5} m/vector",
                v.base_m,
                tile.composite_m(cx, cy),
                rough / (n * (n - 1)).max(1) as f64
            );
        }
    }
    // The seam: both sides of every shared boundary read the same ground.
    let mine = src.relief_sampler(cell);
    let a = map.direction(cell as u32).as_dvec3().normalize();
    let mut worst = 0.0f64;
    for &j in map.neighbours(cell as u32) {
        let theirs = src.relief_sampler(j as usize);
        let b = map.direction(j).as_dvec3().normalize();
        for k in 0..9 {
            let t = 0.3 + 0.05 * k as f64;
            let p = (a * (1.0 - t) + b * t).normalize();
            worst = worst.max((mine(p) - theirs(p)).abs());
        }
    }
    println!("seam: worst disagreement with a neighbour {worst:.6} m");
    // The picture — the tile's own, the one the bench's TILE view shows.
    let (rgba, dim) = tile.shaded_rgba(1, sea, src.top_m());
    // The field wears the same picture, so it is built before the picture
    // is given to the PNG.
    let the_field = field.then(|| quad_field(&tile, &rgba));
    let png: image::RgbaImage =
        image::ImageBuffer::from_raw(dim, dim, rgba).ok_or("image buffer")?;
    let path = out.join(format!("ground_{}_{cell}.png", seed));
    png.save(&path)?;
    println!("→ {}", path.display());
    if let Some(f) = the_field {
        // THE FIELD: the hex as the engine's field of vectors, the surface
        // contoured between them drawn by a software rasterizer — the one
        // picture of the geometry the window draws, for eyes that have no
        // window.
        let started = std::time::Instant::now();
        let regions = f.surface_meshes(256);
        let (verts, idx) = regions.iter().fold((0usize, 0usize), |(v, i), r| {
            (v + r.mesh.vertices.len(), i + r.mesh.indices.len())
        });
        println!(
            "field: {} vectors (mask {}), {} quads, {} regions, {} vertices, {} indices, {:.0} MB, {:.1}s",
            f.quads.len(),
            tile.mask.count(),
            regions.iter().map(|r| r.quads).sum::<usize>(),
            regions.len(),
            verts,
            idx,
            (verts * 28 + idx * 4) as f64 * 1e-6,
            started.elapsed().as_secs_f32()
        );
        let (centre, radius) = f.framing();
        let above = raster(&regions, Vec3A::from(centre), radius, 0.0, 1024);
        let path = out.join(format!("field_{}_{cell}_above.png", seed));
        above.save(&path)?;
        println!("→ {}", path.display());
        let (cx, cy) = cluster.unwrap_or((TILE_DIM / 2, TILE_DIM / 2));
        let close_centre = f
            .at(cx, cy)
            .map(|q| Vec3A::from(f.vertex_position(q)))
            .unwrap_or(Vec3A::from(centre));
        let close = raster(
            &regions,
            close_centre,
            12.0 * flicker_voxel::CLUSTER_DIM as f32,
            35f32.to_radians(),
            1024,
        );
        let path = out.join(format!("field_{}_{cell}_close.png", seed));
        close.save(&path)?;
        println!("→ {}", path.display());
    }
    Ok(())
}

/// **A picture of the field's own meshes** — an orthographic eye over
/// `centre`, framing `radius` voxels across half the picture, from straight
/// above (`elevation` 0) or tilted that many radians toward the south-east;
/// every triangle of every region depth-tested and flat-lit by its own
/// normal under one light from the upper west, so the ground reads as the
/// tile's picture, its slopes lit or shaded by which way they face.
fn raster(
    regions: &[FieldRegion],
    centre: Vec3A,
    radius: f32,
    elevation: f32,
    size: u32,
) -> image::RgbaImage {
    // The eye's basis: `forward` looks down at the ground (tilted toward the
    // north from the south-east when elevated), `right` east-ish, `up` the
    // screen's vertical.
    let forward = if elevation == 0.0 {
        Vec3A::NEG_Y
    } else {
        Vec3A::new(
            -0.5 * elevation.sin(),
            -elevation.cos(),
            -0.5 * elevation.sin(),
        )
        .normalize()
    };
    let right = forward.cross(Vec3A::Y).normalize_or_zero();
    let right = if right.length_squared() < 0.5 {
        Vec3A::X
    } else {
        right
    };
    let up = right.cross(forward).normalize();
    let scale = size as f32 * 0.5 / radius;
    let light = Vec3A::new(-0.5, 0.7, -0.4).normalize();
    let n = (size as usize) * (size as usize);
    let mut depth = vec![f32::MAX; n];
    let mut colour = vec![[6u8, 8, 12]; n];
    let project = |p: [f32; 3]| {
        let d = Vec3A::from(p) - centre;
        (
            size as f32 * 0.5 + d.dot(right) * scale,
            size as f32 * 0.5 - d.dot(up) * scale,
            d.dot(forward),
        )
    };
    for r in regions {
        for tri in r.mesh.indices.as_chunks::<3>().0 {
            let v: Vec<_> = tri.iter().map(|&i| r.mesh.vertices[i as usize]).collect();
            let p: Vec<(f32, f32, f32)> = v.iter().map(|w| project(w.position)).collect();
            let (min_x, max_x) = (
                p.iter()
                    .map(|q| q.0)
                    .fold(f32::MAX, f32::min)
                    .floor()
                    .max(0.0),
                p.iter()
                    .map(|q| q.0)
                    .fold(f32::MIN, f32::max)
                    .ceil()
                    .min(size as f32 - 1.0),
            );
            let (min_y, max_y) = (
                p.iter()
                    .map(|q| q.1)
                    .fold(f32::MAX, f32::min)
                    .floor()
                    .max(0.0),
                p.iter()
                    .map(|q| q.1)
                    .fold(f32::MIN, f32::max)
                    .ceil()
                    .min(size as f32 - 1.0),
            );
            if min_x > max_x || min_y > max_y {
                continue;
            }
            // The face's own normal, turned toward the eye (what the FLAT
            // draw does), lit by the one light over an ambient floor.
            let a = Vec3A::from(v[0].position);
            let mut normal = (Vec3A::from(v[1].position) - a)
                .cross(Vec3A::from(v[2].position) - a)
                .normalize_or_zero();
            if normal.dot(forward) > 0.0 {
                normal = -normal;
            }
            let shade = 0.3 + 0.7 * normal.dot(light).max(0.0);
            let m = v[0].material;
            let rgb = [
                ((m & 0xFF) as f32 * shade) as u8,
                (((m >> 8) & 0xFF) as f32 * shade) as u8,
                (((m >> 16) & 0xFF) as f32 * shade) as u8,
            ];
            let area =
                (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-12 {
                continue;
            }
            let mut y = min_y;
            while y <= max_y {
                let mut x = min_x;
                while x <= max_x {
                    let (px, py) = (x + 0.5, y + 0.5);
                    let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                    let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                        let z = w0 * p[0].2 + w1 * p[1].2 + w2 * p[2].2;
                        let i = (y as usize) * size as usize + x as usize;
                        if z < depth[i] {
                            depth[i] = z;
                            colour[i] = rgb;
                        }
                    }
                    x += 1.0;
                }
                y += 1.0;
            }
        }
    }
    let mut out = image::RgbaImage::new(size, size);
    for (i, px) in out.pixels_mut().enumerate() {
        let c = colour[i];
        *px = image::Rgba([c[0], c[1], c[2], 255]);
    }
    out
}
