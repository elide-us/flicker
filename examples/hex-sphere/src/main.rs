//! Headless test client for `flicker-worldgrid` (docs/hex-sphere-handoff.md).
//!
//! Builds the full icosahedral hex sphere, prints a verification report, and
//! writes a PLY mesh — each cell a polygon coloured by its shard, the twelve
//! pentagons picked out in red — so the topology can be eyeballed in any 3-D
//! viewer. Deliberately GPU-free so it runs on the underpowered test box.
//!
//!   cargo run -p hex-sphere -- [freq] [out.ply]

use std::fmt::Write as _;
use std::fs;

use flicker_worldgrid::icosphere_with_outlines;
use glam::Vec3;

fn main() {
    let mut args = std::env::args().skip(1);
    let freq: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(16);
    // Default into target/ (gitignored) so a bare run doesn't litter the repo.
    let out = args.next().unwrap_or_else(|| "target/hex-sphere.ply".to_string());

    let (s, outlines) = icosphere_with_outlines(freq);
    report(&s);
    let ply = build_ply(&s, &outlines);
    fs::write(&out, ply).unwrap_or_else(|e| panic!("writing {out}: {e}"));
    let abs = fs::canonicalize(&out).unwrap_or_else(|_| out.clone().into());
    println!("\nwrote PLY mesh → {}", abs.display());
    println!("open it in Blender / MeshLab; shards are colour-coded, pentagons red.");
}

/// Print the topology verification report.
fn report(s: &flicker_worldgrid::Sphere) {
    let n = s.len();
    println!("icosahedral hex sphere @ freq {}", s.freq);
    println!("  cells            {n}  (formula 10·freq²+2 = {})", 10 * s.freq * s.freq + 2);

    let pents: Vec<usize> = (0..n).filter(|&i| s.is_pentagon[i]).collect();
    println!("  pentagons        {}  (must be 12)", pents.len());

    let bad_degree = (0..n)
        .filter(|&i| s.neighbors[i].len() != if s.is_pentagon[i] { 5 } else { 6 })
        .count();
    println!("  wrong-degree     {bad_degree}  (must be 0: pentagons=5, hexes=6)");

    // Euler V - E + F = 2 for the closed dual polyhedron.
    let e: usize = s.neighbors.iter().map(|nb| nb.len()).sum::<usize>() / 2;
    let f = 2 * e / 3;
    println!("  V-E+F            {}  (must be 2)", n as i64 - e as i64 + f as i64);

    let mut counts = [0usize; 20];
    for &sh in &s.shard {
        counts[sh as usize] += 1;
    }
    let (smin, smax) = (counts.iter().min().unwrap(), counts.iter().max().unwrap());
    println!("  shards           20 faces, {smin}–{smax} cells each");

    let (mut lo, mut hi, mut sum) = (f32::MAX, 0.0f32, 0.0f32);
    for (i, &a) in s.area.iter().enumerate() {
        sum += a;
        if !s.is_pentagon[i] {
            lo = lo.min(a);
            hi = hi.max(a);
        }
    }
    println!("  hex area spread  {:.3}× (ISEA slice flattens toward 1.0)", hi / lo);
    println!("  total area       {sum:.3}  (sphere 4π = {:.3})", 4.0 * std::f32::consts::PI);

    println!("  the twelve pentagons (lat, lon):");
    for &i in &pents {
        let d = s.dirs[i];
        let lat = d.y.clamp(-1.0, 1.0).asin().to_degrees();
        let lon = d.z.atan2(d.x).to_degrees();
        println!("    shard {:>2}  ({lat:>6.1}°, {lon:>7.1}°)", s.shard[i]);
    }
}

/// Serialise the cells as an ASCII PLY: one polygon face per cell, RGB by shard,
/// pentagons forced red so the twelve defects stand out.
fn build_ply(s: &flicker_worldgrid::Sphere, outlines: &[Vec<Vec3>]) -> String {
    let nv: usize = outlines.iter().map(|o| o.len()).sum();
    let nf = outlines.len();

    let mut body = String::new();
    let mut faces = String::new();
    let mut base = 0usize;
    for (i, outline) in outlines.iter().enumerate() {
        for c in outline {
            let _ = writeln!(body, "{} {} {}", c.x, c.y, c.z);
        }
        let (r, g, b) = if s.is_pentagon[i] {
            (235u8, 40, 40)
        } else {
            shard_color(s.shard[i])
        };
        let _ = write!(faces, "{}", outline.len());
        for k in 0..outline.len() {
            let _ = write!(faces, " {}", base + k);
        }
        let _ = writeln!(faces, " {r} {g} {b}");
        base += outline.len();
    }

    let mut ply = String::new();
    ply.push_str("ply\nformat ascii 1.0\n");
    ply.push_str("comment flicker hex-sphere test client\n");
    let _ = writeln!(ply, "element vertex {nv}");
    ply.push_str("property float x\nproperty float y\nproperty float z\n");
    let _ = writeln!(ply, "element face {nf}");
    ply.push_str("property list uchar int vertex_indices\n");
    ply.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
    ply.push_str("end_header\n");
    ply.push_str(&body);
    ply.push_str(&faces);
    ply
}

/// A distinct hue per shard (HSV→RGB at fixed saturation/value).
fn shard_color(shard: u8) -> (u8, u8, u8) {
    let h = shard as f32 / 20.0 * 6.0;
    let (s, v) = (0.55f32, 0.95f32);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let q = |f: f32| ((f + m) * 255.0).round() as u8;
    (q(r), q(g), q(b))
}
