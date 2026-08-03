//! Factory patches — the instrument's starting library.
//!
//! A synthesizer ships with patches for the same reason this does: a rack of
//! neutral sliders teaches you nothing about what the instrument can do, and the
//! fastest way to learn a knob is to turn it on a sound you already recognize.
//! Each patch here is bound to a real row of the 256-material index, so it also
//! answers the pipeline's actual question — *what does material 10 look like?*
//!
//! These are **recipes, not images**: settings that a reader can dial to, not
//! baked output standing in for a generator. Each is a few hundred bytes and
//! rebuilds at any resolution.

use flicker_materials::MaterialId;

use crate::channel::{BlendMode, Channel, NoiseKind, CHANNEL_COUNT};
use crate::output::{ColorRamp, OutputStage, RampStop};
use crate::recipe::TextureRecipe;

/// Build a recipe from a sparse channel list — the rest of the rack stays silent.
fn patch(
    id: &str,
    name: &str,
    material: MaterialId,
    seed: u64,
    voices: &[Channel],
    out: OutputStage,
) -> TextureRecipe {
    let mut channels = [Channel::default(); CHANNEL_COUNT];
    for (slot, voice) in channels.iter_mut().zip(voices) {
        *slot = *voice;
    }
    TextureRecipe {
        id: id.into(),
        name: name.into(),
        material: Some(material),
        seed,
        channels,
        out,
    }
}

fn ramp(stops: &[(f32, [f32; 3])]) -> ColorRamp {
    ColorRamp { stops: stops.iter().map(|(at, color)| RampStop { at: *at, color: *color }).collect() }
}

/// **Granite** (material 10) — a coarse plutonic rock: a mottled feldspar bed
/// with dark mineral speckle scattered through it.
///
/// The speckle is a high-scale `Worley` on `Min`, which carves isolated dark
/// grains rather than a uniform dirtying — the difference between rock and noise.
pub fn granite() -> TextureRecipe {
    patch(
        "granite",
        "Granite",
        10,
        0x6721_A19E,
        &[
            Channel {
                enabled: true,
                source: NoiseKind::Fbm,
                scale: 5,
                octaves: 5,
                contrast: 1.3,
                ..Channel::default()
            },
            Channel {
                enabled: true,
                source: NoiseKind::Worley,
                scale: 26,
                salt: 0x9A1,
                contrast: 2.2,
                blend: BlendMode::Min,
                amount: 0.55,
                ..Channel::default()
            },
            Channel {
                enabled: true,
                source: NoiseKind::Value,
                scale: 48,
                salt: 0x3C7,
                blend: BlendMode::Overlay,
                amount: 0.3,
                ..Channel::default()
            },
        ],
        OutputStage {
            ramp: ramp(&[
                (0.0, [0.20, 0.18, 0.19]),
                (0.45, [0.55, 0.50, 0.48]),
                (0.75, [0.78, 0.73, 0.70]),
                (1.0, [0.88, 0.85, 0.83]),
            ]),
            relief: 0.35,
            roughness: 0.72,
            roughness_mod: 0.12,
            metalness: 0.0,
            metalness_mod: 0.0,
            ao: 0.45,
        },
    )
}

/// **Sandstone** (material 12) — bedded sediment: near-horizontal strata bent by
/// a strong domain warp, with grain over the top.
///
/// This is the patch that shows what `warp` is for: the same `Stripe` source with
/// warp at zero is ruled lines, and at 0.6 it is rock.
pub fn sandstone() -> TextureRecipe {
    patch(
        "sandstone",
        "Sandstone",
        12,
        0x5AD5_70E1,
        &[
            Channel {
                enabled: true,
                source: NoiseKind::Stripe,
                scale: 9,
                warp: 0.55,
                salt: 0x11,
                contrast: 0.85,
                ..Channel::default()
            },
            Channel {
                enabled: true,
                source: NoiseKind::Fbm,
                scale: 4,
                octaves: 4,
                salt: 0x77,
                blend: BlendMode::Overlay,
                amount: 0.45,
                ..Channel::default()
            },
            Channel {
                enabled: true,
                source: NoiseKind::Value,
                scale: 64,
                salt: 0xB3,
                blend: BlendMode::Overlay,
                amount: 0.22,
                ..Channel::default()
            },
        ],
        OutputStage {
            ramp: ramp(&[
                (0.0, [0.45, 0.34, 0.20]),
                (0.4, [0.72, 0.60, 0.39]),
                (0.8, [0.86, 0.76, 0.55]),
                (1.0, [0.93, 0.87, 0.71]),
            ]),
            relief: 0.55,
            roughness: 0.86,
            roughness_mod: 0.08,
            metalness: 0.0,
            metalness_mod: 0.0,
            ao: 0.6,
        },
    )
}

/// **Basalt** (material 11) — a chilled flow: shrinkage-fractured plates with a
/// vesicular pitting inside them.
///
/// `Worley` inverted is the plate interior; the fracture network is what is left
/// between the cells.
pub fn basalt() -> TextureRecipe {
    patch(
        "basalt",
        "Basalt",
        11,
        0x8A5A_17C0,
        &[
            Channel {
                enabled: true,
                source: NoiseKind::Worley,
                scale: 7,
                salt: 0x2F,
                contrast: 1.8,
                ..Channel::default()
            },
            Channel {
                enabled: true,
                source: NoiseKind::Billow,
                scale: 22,
                octaves: 3,
                salt: 0x5E,
                blend: BlendMode::Mul,
                amount: 0.5,
                ..Channel::default()
            },
            Channel {
                enabled: true,
                source: NoiseKind::Fbm,
                scale: 3,
                octaves: 4,
                salt: 0xC1,
                blend: BlendMode::Overlay,
                amount: 0.35,
                ..Channel::default()
            },
        ],
        OutputStage {
            ramp: ramp(&[
                (0.0, [0.04, 0.04, 0.05]),
                (0.35, [0.14, 0.14, 0.16]),
                (0.8, [0.28, 0.28, 0.31]),
                (1.0, [0.38, 0.38, 0.41]),
            ]),
            relief: 0.8,
            roughness: 0.65,
            roughness_mod: -0.2,
            metalness: 0.0,
            metalness_mod: 0.0,
            ao: 0.85,
        },
    )
}

/// **Hematite ore** (material 40) — metallic iron filaments threading a dull host
/// rock.
///
/// The only patch with `metalness_mod`: the ridged vein network drives metalness
/// as well as colour, so the veins read as metal and the rock around them does
/// not. That is the whole reason metalness is modulated by the field rather than
/// being a single number.
pub fn hematite() -> TextureRecipe {
    patch(
        "hematite",
        "Hematite",
        40,
        0xFE00_002A,
        &[
            Channel {
                enabled: true,
                source: NoiseKind::Fbm,
                scale: 4,
                octaves: 4,
                contrast: 0.9,
                amount: 0.6,
                ..Channel::default()
            },
            Channel {
                enabled: true,
                source: NoiseKind::Ridged,
                scale: 6,
                octaves: 4,
                warp: 0.35,
                salt: 0xFE,
                contrast: 2.6,
                blend: BlendMode::Max,
                amount: 0.9,
                ..Channel::default()
            },
        ],
        OutputStage {
            ramp: ramp(&[
                (0.0, [0.17, 0.15, 0.14]),
                (0.55, [0.34, 0.20, 0.16]),
                (0.85, [0.62, 0.24, 0.18]),
                (1.0, [0.80, 0.42, 0.34]),
            ]),
            relief: 0.5,
            roughness: 0.7,
            roughness_mod: -0.45,
            metalness: 0.15,
            metalness_mod: 0.7,
            ao: 0.55,
        },
    )
}

/// Every factory patch, in library order.
pub fn all() -> Vec<TextureRecipe> {
    vec![granite(), sandstone(), basalt(), hematite()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::{bake, MapKind};

    /// Ids are the file stems a bake writes under, so a collision would have one
    /// patch silently overwrite another in `staging/`.
    #[test]
    fn patch_ids_and_material_bindings_are_unique() {
        let all = all();
        let mut ids: Vec<_> = all.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len(), "duplicate patch id");

        let mut mats: Vec<_> = all.iter().filter_map(|r| r.material).collect();
        mats.sort_unstable();
        mats.dedup();
        assert_eq!(mats.len(), all.len(), "two patches claim one material");
    }

    /// Every patch must bind to a material that actually exists in the index —
    /// a binding to a phantom id would fail to resolve at the point terrain asks
    /// for it, far from here.
    ///
    /// (The tables are reached by a manifest-relative climb because this is a
    /// TEST FIXTURE. The crate itself never reads a path: a caller hands it a
    /// recipe, and where the material index lives is the app's business.)
    #[test]
    fn every_binding_resolves_in_the_material_index() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/data");
        let tables =
            flicker_materials::Tables::from_source(&flicker_materials::JsonTableSource::new(dir))
                .expect("material tables load");
        for r in all() {
            let id = r.material.expect("factory patches are bound");
            assert!(
                tables.material(id).is_some(),
                "patch '{}' binds material {id}, which is not in the index",
                r.id
            );
        }
    }

    /// A patch has to actually make a surface: a field that never moves is a flat
    /// colour, which would mean the channels are cancelling each other out.
    #[test]
    fn every_patch_produces_a_varied_surface() {
        for r in all() {
            let set = bake(&r, 64);
            let base = set.get(MapKind::BaseColor).unwrap();
            let lo = base.pixels.chunks(4).map(|p| p[0]).min().unwrap();
            let hi = base.pixels.chunks(4).map(|p| p[0]).max().unwrap();
            assert!(hi as i32 - lo as i32 > 24, "patch '{}' is nearly flat ({lo}..{hi})", r.id);
        }
    }
}
