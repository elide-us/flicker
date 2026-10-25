//! The guards a designer leans on: a bake or preview refuses only what it reaches,
//! and refuses it LOUD — non-finite numbers, lattice-breaking noise, broken documents
//! — while the band seats where it is baked and the shared maths stays shared.

use flicker_primitive::noise as primitive;

use super::*;
use crate::bake::{seat, seat_range};

/// A loose broken node — being drawn, never connected — does not stop the bake, and
/// `problems` names exactly it.
#[test]
fn a_loose_broken_node_does_not_block_the_bake() {
    let r = presets::granite();
    let mut g = MaterialGraph::from_recipe(&r);
    g.nodes.push(node("loose", NodeKind::Invert {}, &[]));
    let set = bake_graph(&g, 48).expect("a node nothing reads is not the bake's business");
    assert_eq!(differing_bytes(&set, &bake(&r, 48)), vec![]);
    assert_eq!(
        problems(&g),
        vec![GraphError::MissingInput {
            at: "loose.field".into()
        }]
    );

    // A loop between two loose cards and a loose NaN: the same — each listed once.
    let mut g = MaterialGraph::from_recipe(&r);
    g.nodes.push(node("p", NodeKind::Invert {}, &["q"]));
    g.nodes.push(node("q", NodeKind::Invert {}, &["p"]));
    g.nodes
        .push(node("nan", NodeKind::Constant { value: f64::NAN }, &[]));
    assert!(bake_graph(&g, 16).is_ok());
    let found = problems(&g);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.contains(&GraphError::NonFinite {
        node: "nan".into(),
        param: "value"
    }));
    assert!(found
        .iter()
        .any(|p| matches!(p, GraphError::Cycle { path } if path.len() == 3)));
}

/// A preview validates what it reaches and never the output: a node upstream of a
/// broken output port still previews, exactly as it would in the sound graph.
#[test]
fn a_preview_upstream_of_a_broken_output_port_still_works() {
    let sound = MaterialGraph::from_recipe(&presets::granite());
    let mut broken = sound.clone();
    broken.output.metalness = Scalar::Field("deleted".into());
    let bad_port = GraphError::UnknownNode {
        key: "deleted".into(),
        at: "output.metalness".into(),
    };
    assert_eq!(bake_graph(&broken, 16).unwrap_err(), bad_port);
    for key in ["ch1", "mix3", "rough"] {
        assert_eq!(
            field_of(&broken, key, 32).unwrap(),
            field_of(&sound, key, 32).unwrap(),
            "{key}"
        );
    }
    assert_eq!(problems(&broken), vec![bad_port]);
}

/// NaN and infinity fail LOUD: a non-finite parameter is refused before anything is
/// evaluated, and a node that evaluates to non-finite texels is refused by name —
/// never quantized into a silent black map.
#[test]
fn non_finite_numbers_are_refused_not_baked_black() {
    let reads = |key: &str| Output {
        height: Some(key.into()),
        ..Output::default()
    };
    let nan = graph(
        vec![node("k", NodeKind::Constant { value: f64::NAN }, &[])],
        reads("k"),
    );
    assert_eq!(
        bake_graph(&nan, 8).unwrap_err(),
        GraphError::NonFinite {
            node: "k".into(),
            param: "value"
        }
    );
    let mut ramp = ColorRamp::default();
    ramp.stops[1].color[2] = f32::NAN;
    for (kind, param) in [
        (NodeKind::Ramp { ramp }, "ramp"),
        (
            NodeKind::Palette {
                rgb: [0.5, f32::INFINITY, 0.5],
            },
            "rgb",
        ),
    ] {
        let g = graph(
            vec![
                node("k", NodeKind::Constant { value: 0.5 }, &[]),
                node("c", kind, &["k"][..if param == "rgb" { 0 } else { 1 }]),
            ],
            Output {
                base: Some("c".into()),
                ..Output::default()
            },
        );
        assert_eq!(
            bake_graph(&g, 8).unwrap_err(),
            GraphError::NonFinite {
                node: "c".into(),
                param
            }
        );
    }
    let pal = || node("pal", NodeKind::Palette { rgb: [1.0; 3] }, &[]);
    for (output, param) in [
        (
            Output {
                relief: f32::NAN,
                ..Output::default()
            },
            "relief",
        ),
        (
            Output {
                roughness: Scalar::Const(f32::INFINITY),
                ..Output::default()
            },
            "roughness",
        ),
        (
            Output {
                emit: Some(Emit {
                    color: "pal".into(),
                    mask: None,
                    strength: f32::NAN,
                }),
                ..Output::default()
            },
            "emit.strength",
        ),
    ] {
        assert_eq!(
            bake_graph(&graph(vec![pal()], output), 8).unwrap_err(),
            GraphError::NonFinite {
                node: "output".into(),
                param
            }
        );
    }

    // A warp of 1e306 tiles is a finite number — the document loads — but at 256²
    // the push overflows f64, and the node that made the NaNs is the one named.
    let wild = graph(
        vec![
            node("n", noise(NoiseKind::Fbm, 4, 1), &[]),
            node("w", NodeKind::Warp { amount: 1e306 }, &["n", "n"]),
            node(
                "lv",
                NodeKind::Levels {
                    lo: 0.0,
                    hi: 1.0,
                    gamma: 1.0,
                },
                &["w"],
            ),
        ],
        reads("lv"),
    );
    let text = serde_json::to_string(&wild).unwrap();
    let loaded = load_material_json(&text).expect("1e306 is finite; it loads");
    assert_eq!(
        bake_graph(&loaded, 256).unwrap_err(),
        GraphError::NonFiniteTexel { node: "w".into() }
    );
}

/// Noise parameters past the lattice's safe arithmetic are refused by name — the v1
/// rack panics inside `flicker_primitive` on the same numbers — and the bounds leave
/// every shipped voice, and a voice right at them, evaluating.
#[test]
fn noise_past_its_lattice_bounds_is_refused_not_panicked() {
    for (voice, param) in [
        (
            Channel {
                warp: 1e19,
                ..Channel::default()
            },
            "warp",
        ),
        (
            Channel {
                lacunarity: 64.0,
                octaves: 12,
                ..Channel::default()
            },
            "lacunarity",
        ),
        (
            Channel {
                scale: u32::MAX,
                ..Channel::default()
            },
            "scale",
        ),
    ] {
        let g = graph(
            vec![node("n", NodeKind::noise(&voice), &[])],
            Output {
                height: Some("n".into()),
                ..Output::default()
            },
        );
        let refused = GraphError::OutOfRange {
            node: "n".into(),
            param,
        };
        assert_eq!(bake_graph(&g, 16).unwrap_err(), refused);
        assert_eq!(problems(&g), vec![refused]);
    }
    for (what, r) in fidelity_corpus() {
        assert_eq!(problems(&MaterialGraph::from_recipe(&r)), vec![], "{what}");
    }
    // At the bounds themselves the lattice arithmetic still holds: no panic, finite.
    let edge = Channel {
        scale: MAX_NOISE_SCALE,
        warp: MAX_NOISE_WARP,
        octaves: 12,
        lacunarity: 4.0,
        ..Channel::default()
    };
    let g = graph(
        vec![node("n", NodeKind::noise(&edge), &[])],
        Output {
            height: Some("n".into()),
            ..Output::default()
        },
    );
    assert_eq!(problems(&g), vec![]);
    assert!(field_of(&g, "n", 16).unwrap().iter().all(|v| v.is_finite()));
}

/// Lit texels and the share of the tile they cover.
fn lit_share(band: &[f32]) -> f64 {
    band.iter().filter(|v| **v > 0.0).count() as f64 / band.len() as f64
}

/// A Band downstream of a re-rastering node seats against its input AS BAKED: the lit
/// texels are exactly those above the edge seated in that size's own range, and the
/// glow covers the same share of the surface as the field resolves — where a fixed 32²
/// seat measured a different field and drifted (measured: 1.9% → 3.2% lit behind this
/// blur, 30.9% where 45% is right behind this tile).
#[test]
fn a_band_behind_a_raster_node_seats_where_it_is_baked() {
    let tiled = graph(
        vec![
            node("n", noise(NoiseKind::Fbm, 4, 5), &[]),
            node("t", NodeKind::Tile { repeat: 16 }, &["n"]),
            node("b", band(0.6, 1.0, 1.0), &["t"]),
        ],
        Output::default(),
    );
    let mut shares = Vec::new();
    for n in [64u32, 256, 1024] {
        let input = field_of(&tiled, "t", n).unwrap();
        let lit = field_of(&tiled, "b", n).unwrap();
        let (lo, hi) = seat_range(&input).expect("the tiled noise has range");
        let edge = seat(lo, hi, 0.6);
        let exact = input
            .iter()
            .zip(&lit)
            .all(|(h, b)| (*h > edge) == (*b > 0.0));
        assert!(
            exact,
            "at {n}² the band is not seated in the field as baked"
        );
        shares.push(lit_share(&lit));
    }
    // 64² holds only 4×4 texels of each of the 16 repeats, so its share moves in 1/16
    // steps; from 256² on, the field is resolved and the glow holds its share.
    assert!((shares[0] - shares[2]).abs() <= 1.0 / 16.0, "{shares:?}");
    assert!((shares[1] - shares[2]).abs() <= 0.01, "{shares:?}");

    let blurred = graph(
        vec![
            node("n", noise(NoiseKind::Value, 16, 5), &[]),
            node("bl", NodeKind::Blur { radius: 0.015 }, &["n"]),
            node("b", band(0.95, 1.0, 1.0), &["bl"]),
        ],
        Output::default(),
    );
    let shares: Vec<f64> = [64u32, 100, 256, 1024]
        .iter()
        .map(|&n| lit_share(&field_of(&blurred, "b", n).unwrap()))
        .collect();
    let (lo, hi) = shares
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), s| (l.min(*s), h.max(*s)));
    assert!(
        lo > 0.0 && hi - lo <= 0.01,
        "the glow drifts with size: {shares:?}"
    );
}

/// A closed window at the default softness peaks at FULL light in its middle — the
/// ramps span half the window, not the headroom to the ceiling.
#[test]
fn a_closed_window_peaks_at_full_strength() {
    let peak = crate::output::band_window(0.35, 0.2, Some(0.5), 1.0);
    assert!(peak >= 0.99, "the band_window peak is {peak}");
    let g = graph(
        vec![
            node("ramp", gradient(Axis::U, 1), &[]),
            node("w", band(0.2, 0.3, 1.0), &["ramp"]),
        ],
        Output::default(),
    );
    let peak = field_of(&g, "w", 1024)
        .unwrap()
        .into_iter()
        .fold(0.0f32, f32::max);
    assert!(peak >= 0.99, "the window peaks at {peak}");
    // `soft` past 1 is held to 1 in a closed window — it used to cap the peak at 0.5.
    let band_window = crate::output::band_window;
    assert_eq!(
        band_window(0.35, 0.2, Some(0.5), 2.0),
        band_window(0.35, 0.2, Some(0.5), 1.0)
    );
    let g = graph(
        vec![
            node("ramp", gradient(Axis::U, 1), &[]),
            node("w", band(0.2, 0.3, 2.0), &["ramp"]),
        ],
        Output::default(),
    );
    let peak = field_of(&g, "w", 1024)
        .unwrap()
        .into_iter()
        .fold(0.0f32, f32::max);
    assert!(peak >= 0.99, "a soft-2 window peaks at {peak}");
}

/// A versionless document that is really a broken GRAPH reports the graph's error,
/// not the recipe parser's; one shaped like a recipe still reports as a recipe.
#[test]
fn a_versionless_broken_graph_reports_the_graph_error() {
    let mut doc = serde_json::to_value(every_kind_graph()).unwrap();
    doc.as_object_mut().unwrap().remove("version");
    doc["nodes"][0]["kind"] = "no_such_kind".into();
    match load_material_json(&doc.to_string()) {
        Err(LoadError::Invalid { version: 2, reason }) => {
            assert!(reason.contains("no_such_kind"), "{reason}")
        }
        other => panic!("expected the v2 error, got {other:?}"),
    }
    let mut v1 = serde_json::to_value(presets::basalt()).unwrap();
    v1.as_object_mut().unwrap().remove("out");
    assert!(matches!(
        load_material_json(&v1.to_string()),
        Err(LoadError::Invalid { version: 1, .. })
    ));
}

/// Unknown fields are refused rather than dropped, and anything but a JSON object is
/// not a material document.
#[test]
fn unknown_fields_and_non_objects_are_refused() {
    for text in ["[]", "42", "\"material\""] {
        assert!(
            matches!(load_material_json(text), Err(LoadError::Syntax(_))),
            "{text}"
        );
    }
    let mut doc = serde_json::to_value(every_kind_graph()).unwrap();
    doc["nodes"][3]["bogus_param"] = 5.into();
    match load_material_json(&doc.to_string()) {
        Err(LoadError::Invalid { version: 2, reason }) => {
            assert!(reason.contains("bogus_param"), "{reason}")
        }
        other => panic!("an unknown node field loaded: {other:?}"),
    }
    let mut doc = serde_json::to_value(every_kind_graph()).unwrap();
    doc["output"]["roughnes"] = 0.5.into();
    assert!(matches!(
        load_material_json(&doc.to_string()),
        Err(LoadError::Invalid { version: 2, .. })
    ));
    // Even the parameterless kind: a misspelt `"input"` on an Invert is refused, not
    // silently dropped along with the wiring it meant.
    let mut doc = serde_json::to_value(every_kind_graph()).unwrap();
    let invert = doc["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n["kind"] == "invert")
        .expect("the every-kind graph has an Invert");
    let fields = invert.as_object_mut().unwrap();
    fields.remove("inputs");
    fields.insert("input".into(), serde_json::json!(["gu"]));
    fields.insert("bogus".into(), 1.into());
    match load_material_json(&doc.to_string()) {
        Err(LoadError::Invalid { version: 2, reason }) => {
            assert!(
                reason.contains("input") || reason.contains("bogus"),
                "{reason}"
            )
        }
        other => panic!("a misspelt Invert loaded: {other:?}"),
    }
}

/// Relief past [`MAX_RELIEF`] is refused — 3·10³⁸ would overflow the normal into NaN —
/// and the bound is what the step formula says: at the largest rung's step, the
/// steepest slope [`MAX_RELIEF`] allows still makes a finite, fully tipped normal.
#[test]
fn relief_past_its_bound_is_refused() {
    for relief in [3e38f32, -3e38, MAX_RELIEF * 2.0] {
        let g = graph(
            vec![node("n", noise(NoiseKind::Fbm, 4, 1), &[])],
            Output {
                height: Some("n".into()),
                relief,
                ..Output::default()
            },
        );
        assert_eq!(
            bake_graph(&g, 16).unwrap_err(),
            GraphError::OutOfRange {
                node: "output".into(),
                param: "relief"
            },
            "relief {relief}"
        );
    }
    assert_eq!(MAX_RELIEF, 1_048_576.0, "2^20");
    let finest = crate::bake::relief_step(crate::BAKE_SIZES[crate::BAKE_SIZES.len() - 1].px);
    // A unit step between the neighbours of texel 1, at the finest step there is.
    let h = [0.0f32, 0.0, 1.0, 1.0].repeat(4);
    let steep = crate::bake::normal_texel(&h, 4, 1, 1, MAX_RELIEF, finest);
    assert_eq!(steep, [0, 128, 128, 255], "tipped all the way, and finite");
    for (_, r) in fidelity_corpus() {
        assert!(r.out.relief.abs() <= MAX_RELIEF);
    }
}

/// The batched previews are the one-at-a-time previews, in request order, from one
/// pass over the union.
#[test]
fn batched_previews_match_one_at_a_time() {
    let g = every_kind_graph();
    let of = |kind: ValueKind| -> Vec<&str> {
        g.nodes
            .iter()
            .filter(|n| n.kind.output() == kind)
            .map(|n| n.key.as_str())
            .collect()
    };
    let (fields, colours) = (of(ValueKind::Field), of(ValueKind::Colour));
    let batch = fields_of(&g, &fields, 32).unwrap();
    assert_eq!(batch.len(), fields.len());
    for (key, got) in fields.iter().zip(&batch) {
        assert_eq!(*got, field_of(&g, key, 32).unwrap(), "{key}");
    }
    let batch = colours_of(&g, &colours, 32).unwrap();
    for (key, got) in colours.iter().zip(&batch) {
        assert_eq!(*got, colour_of(&g, key, 32).unwrap(), "{key}");
    }
}

/// The Contrast node is the voice's own contrast (`noise::contrast`), re-centred on a
/// pivot; MixColor fades through the voice's own exact-endpoint fade.
#[test]
fn contrast_and_mix_are_the_voices_own_maths() {
    for (h, pivot, amount) in [
        (0.2f64, 0.5f64, 2.0f64),
        (0.7, 0.3, 1.5),
        (0.1, 0.8, -1.0),
        (0.9, 0.2, 4.0),
        (0.4, 0.6, 0.0),
    ] {
        let want = ((h - pivot) * amount + pivot).clamp(0.0, 1.0);
        let got = nodes::contrast(h, pivot, amount);
        assert!(
            (got - want).abs() < 1e-12,
            "{h} about {pivot} ×{amount}: {got}"
        );
    }
    for h in [0.1, 0.45, 0.8] {
        assert!((nodes::contrast(h, 0.5, 1.7) - primitive::contrast(h, 1.7)).abs() < 1e-12);
    }
    let (a, b) = ([0.1, 0.2, 0.3], [0.9, 0.8, 0.7]);
    assert_eq!(nodes::mix_color(a, b, 0.0), a);
    assert_eq!(nodes::mix_color(a, b, 1.0), b);
    let half = nodes::mix_color(a, b, 0.5);
    assert!((half[0] - 0.5).abs() < 1e-6, "{half:?}");
}
