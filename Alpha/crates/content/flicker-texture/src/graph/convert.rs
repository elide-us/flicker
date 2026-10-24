//! v1 → v2: the rack as a preset graph.
//!
//! The conversion is LOSSLESS in the sense the crate cares about — the converted
//! graph bakes byte-identically to the recipe — and it gets there by construction,
//! not by tuning: every node it emits evaluates through the very function the rack
//! and its output stage call (`Channel::eval`, `BlendMode::apply`, `ColorRamp::sample`,
//! the modulation, the ring-cavity occlusion, the band and its seating).

use super::{Emit, MaterialGraph, Node, NodeKind, Output, Scalar, GRAPH_VERSION};
use crate::channel::BlendMode;
use crate::recipe::TextureRecipe;

fn node(key: &str, kind: NodeKind, inputs: &[&str]) -> Node {
    Node {
        key: key.into(),
        kind,
        inputs: inputs.iter().map(|k| Some((*k).to_string())).collect(),
    }
}

impl MaterialGraph {
    /// The graph a v1 recipe IS.
    ///
    /// - Every channel `N` (its rack slot, 1-based) becomes a `Noise` node `chN`, in
    ///   slot order.
    /// - A SILENT channel keeps its whole voice: `chN`, and a DANGLING `mixN` with its
    ///   mode and amount, fed `(the bus as it stands at slot N, chN)` — exactly the
    ///   fold it would make if it sounded — that nothing downstream reads. A bake only
    ///   evaluates what the output reaches, so the dangling pair costs nothing and
    ///   moves no byte. RE-ENABLING the voice in the designer is rewiring `mixN` into
    ///   the fold: whatever reads the bus `mixN` reads (the next sounding voice's
    ///   `mix`, or — past the last — the output's height and the stage nodes) reads
    ///   `mixN` instead. Silent before any voice has opened the bus, its bus is
    ///   `zero` — the rack's empty bus.
    /// - The rack folds in order through `Blend` nodes `mixN` (the bus below, the
    ///   voice, unmasked) with each voice's mode and amount. `mix` starts its bus at
    ///   ZERO and folds even the first voice into it, so the first voice stands as the
    ///   bus by itself only where that fold is the identity — `Base`, fully open;
    ///   otherwise a `Constant` 0 node `zero` is the bus it folds into, exactly as in
    ///   `mix` (so a first voice at `Base` 0.6 is 0.6 of itself, as it always was). An
    ///   empty rack is `zero` alone.
    /// - The final bus is the output's `height`, at the stage's `relief`.
    /// - `ramp` (`Ramp`) is the base colour; `rough` and `metal` (`Modulate`) are the
    ///   stage's roughness and metalness modulation; `ao` (`Occlusion`) its ring
    ///   cavity; `band` (`Band`, open and seated like `emissive_band`) × `glow`
    ///   (`Palette`, the stage's glow colour) × `emissive_strength` is the emission —
    ///   kept even at strength 0, so the authored glow survives the trip into the
    ///   designer.
    pub fn from_recipe(recipe: &TextureRecipe) -> Self {
        let mut nodes = Vec::new();
        // `mix` folds even the first sounding voice into a zero bus; it stands as the
        // bus itself only where that fold is the identity.
        let opens = recipe
            .channels
            .iter()
            .find(|ch| ch.enabled)
            .is_some_and(|ch| ch.blend == BlendMode::Base && ch.amount >= 1.0);
        let zero = || node("zero", NodeKind::Constant { value: 0.0 }, &[]);
        let mut bus = None;
        if !opens {
            nodes.push(zero());
            bus = Some("zero".to_string());
        }
        for (slot, ch) in recipe.channels.iter().enumerate() {
            let voice = format!("ch{}", slot + 1);
            let mix = format!("mix{}", slot + 1);
            let fold = NodeKind::Blend {
                mode: ch.blend,
                amount: ch.amount,
            };
            if !ch.enabled {
                // Silent: the voice and its fold, dangling off the bus as it stands.
                let below = match &bus {
                    Some(below) => below.clone(),
                    None => {
                        if !nodes.iter().any(|n: &Node| n.key == "zero") {
                            nodes.push(zero());
                        }
                        "zero".to_string()
                    }
                };
                nodes.push(node(&voice, NodeKind::noise(ch), &[]));
                nodes.push(node(&mix, fold, &[&below, &voice]));
                continue;
            }
            nodes.push(node(&voice, NodeKind::noise(ch), &[]));
            bus = Some(match bus {
                None => voice,
                Some(below) => {
                    nodes.push(node(&mix, fold, &[&below, &voice]));
                    mix
                }
            });
        }
        // Never empty: a rack with no sounding voice did not open, so `zero` is the bus.
        let bus = bus.unwrap_or_else(|| "zero".to_string());

        let out = &recipe.out;
        let stage: [(&str, NodeKind); 5] = [
            (
                "ramp",
                NodeKind::Ramp {
                    ramp: out.ramp.clone(),
                },
            ),
            (
                "rough",
                NodeKind::Modulate {
                    base: out.roughness,
                    amount: out.roughness_mod,
                },
            ),
            (
                "metal",
                NodeKind::Modulate {
                    base: out.metalness,
                    amount: out.metalness_mod,
                },
            ),
            ("ao", NodeKind::Occlusion { strength: out.ao }),
            (
                "band",
                NodeKind::Band {
                    at: out.emissive_band,
                    width: 1.0,
                    soft: 1.0,
                },
            ),
        ];
        for (key, kind) in stage {
            nodes.push(node(key, kind, &[&bus]));
        }
        nodes.push(node("glow", NodeKind::Palette { rgb: out.emissive }, &[]));

        MaterialGraph {
            version: GRAPH_VERSION,
            id: recipe.id.clone(),
            name: recipe.name.clone(),
            material: recipe.material,
            seed: recipe.seed,
            nodes,
            output: Output {
                base: Some("ramp".into()),
                height: Some(bus),
                relief: out.relief,
                roughness: Scalar::Field("rough".into()),
                metalness: Scalar::Field("metal".into()),
                ao: Scalar::Field("ao".into()),
                emit: Some(Emit {
                    color: "glow".into(),
                    mask: Some("band".into()),
                    strength: out.emissive_strength,
                }),
            },
        }
    }
}
