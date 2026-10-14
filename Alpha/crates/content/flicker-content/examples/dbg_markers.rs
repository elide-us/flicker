fn main() {
    use flicker_content::baseline::*;
    use flicker_skeletal::format::*;
    let fmt = |v: Vec<String>| {
        v.iter()
            .map(|s| format!("\"{s}\","))
            .collect::<Vec<_>>()
            .join("\n                ")
    };
    println!(
        "HUMANOID\n                {}",
        fmt(markers_for(&SkeletonRecipe::humanoid()))
    );
    let mut horse = reference_recipe(Pattern::Quadruped);
    horse.trunk.tails = vec![TailKind::Long { bones: 8 }];
    println!("HORSE\n                {}", fmt(markers_for(&horse)));
}
