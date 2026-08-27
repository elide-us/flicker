//! Emit a prop VARIATION SET descriptor (`flicker.propset`) into the content tree.
//!
//!   cargo run -p flicker-content --example build_propset -- <out_dir> <SetName> <prop:weight>...
//!
//! e.g. the grass field — three weighted variants (shorter grass more common), landing as
//! `<out_dir>/GrassField.set.json.gz`:
//!   cargo run -p flicker-content --example build_propset -- \
//!     Alpha/content/staging/props/GrassField GrassField Grass-Tall:1 Grass-Medium:1.5 Grass-Short:2

use std::path::Path;

use flicker_content::{PropSet, PropVariant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: build_propset <out_dir> <SetName> <prop:weight>...");
        std::process::exit(2);
    }
    let out_dir = Path::new(&args[1]);
    let name = &args[2];

    let mut variants = Vec::new();
    for spec in &args[3..] {
        let (prop, weight) = spec.rsplit_once(':').unwrap_or((spec.as_str(), "1"));
        variants.push(PropVariant {
            prop: prop.to_string(),
            weight: weight
                .parse()
                .map_err(|e| anyhow::anyhow!("bad weight in '{spec}': {e}"))?,
        });
    }

    let set = PropSet::new(name.clone(), variants);
    let out = out_dir.join(format!("{name}.set.json"));
    set.write(&out)?;
    println!(
        "wrote {}.gz — {} variants, total weight {:.2}",
        out.display(),
        set.variants.len(),
        set.total_weight(),
    );
    Ok(())
}
