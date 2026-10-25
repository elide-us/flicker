//! The gates the graph holds itself to — the rack's own (seamless, deterministic,
//! opaque, round-trips) plus the one that makes it v2 of THIS crate rather than a
//! second synthesizer: a converted recipe bakes to the recipe's bytes.

use super::eval::Plane;
use super::nodes;
use super::*;
use crate::bake::{bake, MapKind, MapSet};
use crate::channel::CHANNEL_COUNT;
use crate::presets;
use crate::random::random;

mod guards;

/// A node wired to `inputs` in port order.
fn node(key: &str, kind: NodeKind, inputs: &[&str]) -> Node {
    Node {
        key: key.into(),
        kind,
        inputs: inputs.iter().map(|k| Some((*k).to_string())).collect(),
    }
}

/// A two-input node with an optional mask on its third port.
fn masked(key: &str, kind: NodeKind, a: &str, b: &str, mask: Option<&str>) -> Node {
    Node {
        key: key.into(),
        kind,
        inputs: vec![Some(a.into()), Some(b.into()), mask.map(Into::into)],
    }
}

fn noise(source: NoiseKind, scale: u32, salt: u64) -> NodeKind {
    NodeKind::noise(&Channel {
        source,
        scale,
        salt,
        warp: 0.3,
        ..Channel::default()
    })
}

// Positional constructors, so a graph reads as a table rather than a page of braces.
fn gradient(axis: Axis, repeat: u32) -> NodeKind {
    NodeKind::Gradient { axis, repeat }
}
fn shape(pattern: Pattern, cells: u32, mortar: f64) -> NodeKind {
    NodeKind::Shape {
        pattern,
        cells,
        mortar,
    }
}
fn blend(mode: BlendMode, amount: f64) -> NodeKind {
    NodeKind::Blend { mode, amount }
}
fn band(at: f32, width: f32, soft: f32) -> NodeKind {
    NodeKind::Band { at, width, soft }
}
fn hard_step(at: f64) -> NodeKind {
    NodeKind::Threshold { at, soft: 0.0 }
}

fn graph(nodes: Vec<Node>, output: Output) -> MaterialGraph {
    MaterialGraph {
        version: GRAPH_VERSION,
        id: "test".into(),
        name: "Test".into(),
        material: None,
        seed: 0x5EED,
        nodes,
        output,
    }
}

/// A hand-built graph that uses EVERY node kind, most of them reaching the output.
/// The two linear gradients repeat ONCE, so the seam is the only place a sawtooth
/// could tear — the periodicity gate would see it there and nowhere else.
fn every_kind_graph() -> MaterialGraph {
    let (lo, hi, gamma) = (0.2, 0.8, 1.4);
    let (pivot, amount) = (0.4, 1.6);
    let nodes = vec![
        node("n1", noise(NoiseKind::Fbm, 4, 1), &[]),
        node("n2", noise(NoiseKind::Worley, 6, 2), &[]),
        node("c", NodeKind::Constant { value: 0.3 }, &[]),
        node("gu", gradient(Axis::U, 1), &[]),
        node("gv", gradient(Axis::V, 1), &[]),
        node("gr", gradient(Axis::Radial, 3), &[]),
        node("tiles", shape(Pattern::Tiles, 4, 0.1), &[]),
        node("bricks", shape(Pattern::Bricks, 3, 0.12), &[]),
        node("hex", shape(Pattern::Hex, 5, 0.1), &[]),
        node("dots", shape(Pattern::Dots, 6, 0.3), &[]),
        node("lv", NodeKind::Levels { lo, hi, gamma }, &["n1"]),
        node("ct", NodeKind::Contrast { pivot, amount }, &["n2"]),
        node("iv", NodeKind::Invert {}, &["gu"]),
        node("bl", NodeKind::Blur { radius: 0.03 }, &["n2"]),
        node("th", NodeKind::Threshold { at: 0.5, soft: 0.1 }, &["gr"]),
        node("bd", band(0.6, 1.0, 0.5), &["n1"]),
        node("win", band(0.2, 0.3, 0.2), &["n2"]),
        node("cv", NodeKind::Curvature { radius: 0.05 }, &["n1"]),
        node("wp", NodeKind::Warp { amount: 0.04 }, &["tiles", "n1"]),
        node("tl", NodeKind::Tile { repeat: 3 }, &["n2"]),
        node(
            "md",
            NodeKind::Modulate {
                base: 0.6,
                amount: 0.25,
            },
            &["n1"],
        ),
        node("oc", NodeKind::Occlusion { strength: 0.8 }, &["n1"]),
        masked("b1", blend(BlendMode::Overlay, 0.7), "n1", "wp", Some("th")),
        node(
            "rp",
            NodeKind::Ramp {
                ramp: ColorRamp::default(),
            },
            &["b1"],
        ),
        node(
            "pl",
            NodeKind::Palette {
                rgb: [0.2, 0.6, 1.0],
            },
            &[],
        ),
        masked(
            "mx",
            NodeKind::MixColor { amount: 0.8 },
            "rp",
            "pl",
            Some("bd"),
        ),
    ];
    graph(
        nodes,
        Output {
            base: Some("mx".into()),
            height: Some("b1".into()),
            relief: 0.6,
            roughness: Scalar::Field("md".into()),
            metalness: Scalar::Const(0.1),
            ao: Scalar::Field("oc".into()),
            emit: Some(Emit {
                color: "pl".into(),
                mask: Some("bd".into()),
                strength: 0.9,
            }),
        },
    )
}

/// The v1 corpus the fidelity gate runs over: every factory patch, the named rolls,
/// and the cases that exercise the conversion's edges — a hand-set glow on every
/// patch (the Band and its seating), an empty rack (the `zero` bus), a first voice
/// that is not an open `Base` (folding into `zero`), a glow over a field with no
/// range (the dropped claim), and a busy six-voice rack.
fn fidelity_corpus() -> Vec<(String, TextureRecipe)> {
    let mut cases: Vec<(String, TextureRecipe)> = presets::all()
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect();
    for seed in [1u64, 2, 3, 7, 42] {
        cases.push((format!("random({seed})"), random(seed)));
    }
    for mut r in presets::all() {
        r.out.emissive_strength = 0.8;
        r.out.emissive_band = 0.6;
        cases.push((format!("{} + hand-set glow", r.id), r));
    }

    let empty = TextureRecipe {
        channels: [Channel::default(); CHANNEL_COUNT],
        ..TextureRecipe::default()
    };
    cases.push(("empty rack".into(), empty));

    let mut first_adds = presets::basalt();
    first_adds.channels[0].blend = BlendMode::Add;
    first_adds.channels[0].amount = 0.7;
    first_adds.out.emissive_strength = 1.0;
    cases.push(("first voice Add 0.7".into(), first_adds));

    let mut late_start = presets::sandstone();
    late_start.channels[0].enabled = false;
    cases.push(("first slot silent".into(), late_start));

    let mut flat_glow = TextureRecipe::default();
    flat_glow.channels[0].contrast = 0.0; // a flat 0.5 field: no range to seat in
    flat_glow.out.emissive_strength = 1.0;
    cases.push(("glow over a flat field".into(), flat_glow));

    let mut busy = TextureRecipe::default();
    for (i, kind) in NoiseKind::ALL.iter().enumerate() {
        busy.channels[i] = Channel {
            enabled: true,
            source: *kind,
            scale: 3 + i as u32,
            warp: 0.3,
            salt: i as u64 * 17,
            blend: BlendMode::ALL[i % BlendMode::ALL.len()],
            amount: 0.6,
            ..Channel::default()
        };
    }
    busy.out.metalness_mod = 0.4;
    busy.out.emissive_strength = 0.7;
    busy.out.emissive_band = 0.3;
    cases.push(("busy rack".into(), busy));
    cases
}

/// How many bytes of each map differ, by kind — empty when the sets are identical.
fn differing_bytes(ours: &MapSet, theirs: &MapSet) -> Vec<(MapKind, usize)> {
    MapKind::ALL
        .iter()
        .filter_map(|&kind| {
            let a = &ours.get(kind)?.pixels;
            let b = &theirs.get(kind)?.pixels;
            let differ =
                a.iter().zip(b).filter(|(x, y)| x != y).count() + a.len().abs_diff(b.len());
            (differ > 0).then_some((kind, differ))
        })
        .collect()
}

/// THE v2 GATE: the rack is a preset graph, not an approximation of one. Every
/// factory patch and the named rolls, converted and baked as a graph, produce the
/// SAME BYTES as `bake` for all seven maps — because both paths call the same blend,
/// ramp, modulation, gradient, ring and seating functions.
#[test]
fn a_converted_recipe_bakes_byte_identically_to_v1() {
    let mut glowing = 0;
    for (what, r) in fidelity_corpus() {
        let g = MaterialGraph::from_recipe(&r);
        let ours = bake_graph(&g, 64).unwrap_or_else(|e| panic!("{what}: {e}"));
        let theirs = bake(&r, 64);
        assert_eq!(
            differing_bytes(&ours, &theirs),
            vec![],
            "{what}: the graph bake drifted from v1"
        );
        assert_eq!(ours, theirs, "{what}: map sets differ");
        let emit = &theirs.get(MapKind::Emit).unwrap().pixels;
        if emit.chunks(4).any(|p| p[..3] != [0, 0, 0]) {
            glowing += 1;
        }
    }
    // The corpus must actually exercise the glow path, or the Band's fidelity is
    // asserted over black maps only.
    assert!(glowing >= 5, "only {glowing} corpus bakes glow");
}

/// The same identity at every size, including the seating size itself (32, where the
/// Band measures its own input) and the degenerate ones.
#[test]
fn the_identity_holds_at_every_size() {
    let mut glowing_granite = presets::granite();
    glowing_granite.out.emissive_strength = 1.0;
    let cases = [glowing_granite, random(42), presets::hematite()];
    for r in &cases {
        let g = MaterialGraph::from_recipe(r);
        for size in [0u32, 1, 2, 7, 31, 32, 33, 100, 128] {
            let ours = bake_graph(&g, size).expect("converted graph bakes");
            assert_eq!(
                differing_bytes(&ours, &bake(r, size)),
                vec![],
                "{} at {size}²",
                r.id
            );
        }
    }
}

/// The conversion's shape is the documented one: `chN`/`mixN` keyed by rack slot, the
/// first open `Base` voice standing as the bus, `zero` only where `mix` needs it, and
/// every silent voice carried WHOLE — `chN` and its fold `mixN` dangling off the bus
/// as it stands at its slot, read by nothing: nothing lost, nothing baked.
#[test]
fn the_rack_converts_to_its_documented_preset_graph() {
    let granite = presets::granite();
    let g = MaterialGraph::from_recipe(&granite);
    let keys: Vec<&str> = g.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "ch1", "ch2", "mix2", "ch3", "mix3", "ch4", "mix4", "ch5", "mix5", "ch6", "mix6",
            "ramp", "rough", "metal", "ao", "band", "glow"
        ]
    );
    assert_eq!(g.output.height.as_deref(), Some("mix3"));
    // The silent voices are there, as the voices they were — each fold dangling off
    // the live bus `mix3` — and nothing reads them.
    let read = |key: &str| {
        g.nodes
            .iter()
            .any(|n| n.inputs.contains(&Some(key.to_string())))
    };
    for slot in 3..6 {
        let (voice, fold) = (format!("ch{}", slot + 1), format!("mix{}", slot + 1));
        let ch = &granite.channels[slot];
        let find = |key: &str| g.nodes.iter().find(|n| n.key == key).unwrap();
        assert_eq!(find(&voice).kind, NodeKind::noise(ch));
        assert_eq!(
            find(&fold).kind,
            NodeKind::Blend {
                mode: ch.blend,
                amount: ch.amount
            }
        );
        assert_eq!(
            find(&fold).inputs,
            [Some("mix3".into()), Some(voice.clone())]
        );
        assert!(!read(&fold), "{fold} must dangle");
    }
    assert_eq!(problems(&g), vec![], "a converted patch is sound");

    // Silent BEFORE the bus has opened: the dangling fold takes the rack's empty bus,
    // and the live fold — opened by the next voice — is untouched, bytes and all.
    let mut late = presets::granite();
    late.channels[0].enabled = false;
    late.channels[1].blend = BlendMode::Base;
    late.channels[1].amount = 1.0;
    let lg = MaterialGraph::from_recipe(&late);
    let keys: Vec<&str> = lg.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(&keys[..5], ["zero", "ch1", "mix1", "ch2", "ch3"]);
    assert_eq!(problems(&lg), vec![]);
    assert_eq!(
        differing_bytes(&bake_graph(&lg, 32).unwrap(), &bake(&late, 32)),
        vec![]
    );

    // Hematite's bed is Base at 0.6: it folds into a zero bus, as in `mix`.
    let h = MaterialGraph::from_recipe(&presets::hematite());
    let keys: Vec<&str> = h.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(&keys[..5], ["zero", "ch1", "mix1", "ch2", "mix2"]);

    // An empty rack is the zero bus alone.
    let empty = TextureRecipe {
        channels: [Channel::default(); CHANNEL_COUNT],
        ..TextureRecipe::default()
    };
    assert_eq!(
        MaterialGraph::from_recipe(&empty).output.height.as_deref(),
        Some("zero")
    );
}

/// Every graph a recipe converts to is a SOUND one — nothing about it fails loud.
#[test]
fn converted_graphs_are_sound_and_complete() {
    for (what, r) in fidelity_corpus() {
        let set = bake_graph(&MaterialGraph::from_recipe(&r), 16)
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(set.maps.len(), MapKind::ALL.len(), "{what}");
        for m in &set.maps {
            assert_eq!(m.pixels.len(), 16 * 16 * 4, "{what} {:?}", m.kind);
            assert!(
                m.pixels.chunks(4).all(|p| p[3] == 255),
                "{what} {:?} not opaque",
                m.kind
            );
        }
    }
}

/// The seam criterion of `no_baked_map_has_a_seam`, on one channel of a raster: the
/// step across the wrap is no worse than the worst ordinary step between neighbours.
fn assert_seamless(what: &str, n: usize, px: impl Fn(usize, usize) -> f32) {
    let mut interior = 0.0f32;
    for y in 0..n {
        for x in 0..n - 1 {
            interior = interior.max((px(x, y) - px(x + 1, y)).abs());
            interior = interior.max((px(y, x) - px(y, x + 1)).abs());
        }
    }
    for k in 0..n {
        let x_seam = (px(n - 1, k) - px(0, k)).abs();
        let y_seam = (px(k, n - 1) - px(k, 0)).abs();
        assert!(
            x_seam <= interior && y_seam <= interior,
            "{what}: seam steps {x_seam}/{y_seam} at {k}, worse than the interior's {interior}"
        );
    }
}

/// Every node kind is periodic on the tile: each node of the every-kind graph,
/// evaluated alone at 64², meets itself across both seams no worse than it meets
/// its own neighbours.
#[test]
fn every_node_kind_is_periodic_on_the_tile() {
    let g = every_kind_graph();
    let n = 64usize;
    for nd in &g.nodes {
        let what = format!("{} ({})", nd.key, nd.kind.id());
        match nd.kind.output() {
            ValueKind::Field => {
                let f = field_of(&g, &nd.key, n as u32).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert!(f.iter().all(|v| v.is_finite()), "{what}: non-finite texel");
                assert_seamless(&what, n, |x, y| f[y * n + x]);
            }
            ValueKind::Colour => {
                let c = colour_of(&g, &nd.key, n as u32).unwrap_or_else(|e| panic!("{what}: {e}"));
                for ch in [0, 1, 2] {
                    assert_seamless(&format!("{what}[{ch}]"), n, |x, y| c[y * n + x][ch]);
                }
            }
        }
    }
    // And the baked maps, through the output, as `bake` is held to.
    let set = bake_graph(&g, n as u32).expect("every-kind graph bakes");
    for m in &set.maps {
        for ch in 0..3 {
            assert_seamless(&format!("{:?}[{ch}]", m.kind), n, |x, y| {
                m.pixels[(y * n + x) * 4 + ch] as f32
            });
        }
    }
}

/// The closed-form generators are periodic as FUNCTIONS, not merely as rasters —
/// `f(u, v) = f(u + 1, v)` — as `the_mixed_field_is_periodic_on_the_tile` holds the
/// rack to.
#[test]
fn closed_form_generators_are_periodic_functions() {
    let points = [(0.0, 0.0), (0.3, 0.7), (0.9, 0.1), (0.5, 0.5), (0.13, 0.61)];
    let shifts = [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (-1.0, 2.0)];
    let check = |what: &str, f: &dyn Fn(f64, f64) -> f64| {
        for (u, v) in points {
            for (du, dv) in shifts {
                let (a, b) = (f(u, v), f(u + du, v + dv));
                assert!(
                    (a - b).abs() < 1e-9,
                    "{what} not periodic at ({u},{v}) + ({du},{dv}): {a} vs {b}"
                );
            }
        }
    };
    for axis in [Axis::U, Axis::V, Axis::Radial] {
        for repeat in [0, 1, 3] {
            check(&format!("gradient {axis:?}×{repeat}"), &|u, v| {
                nodes::gradient(axis, repeat, u, v)
            });
        }
    }
    for pattern in [Pattern::Tiles, Pattern::Bricks, Pattern::Hex, Pattern::Dots] {
        for cells in [1, 3, 4, 7] {
            check(&format!("shape {pattern:?}×{cells}"), &|u, v| {
                nodes::shape(pattern, cells, 0.15, u, v)
            });
        }
    }
}

/// Circularly shift an `n × n` raster by `(dx, dy)` texels.
fn shifted<T: Copy>(src: &[T], n: usize, dx: usize, dy: usize) -> Vec<T> {
    (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            src[((y + n - dy) % n) * n + (x + n - dx) % n]
        })
        .collect()
}

/// The neighbourhood nodes WRAP: shifting the input around the torus shifts the
/// output the same way. A clamped border breaks this at every edge texel — which is
/// exactly the seam this crate exists to never make.
#[test]
fn neighbourhood_nodes_wrap_the_tile() {
    let n = 64u32;
    let side = n as usize;
    let (dx, dy) = (13, 29);
    let voice = Channel {
        source: NoiseKind::Fbm,
        scale: 5,
        ..Channel::default()
    };
    let src: Vec<f64> = crate::bake::sample_grid(n, |u, v| voice.eval(u, v, 77));
    let src32: Vec<f32> = src.iter().map(|v| *v as f32).collect();
    let moved = shifted(&src, side, dx, dy);
    let moved32 = shifted(&src32, side, dx, dy);
    let close = |what: &str, a: &[f64], b: &[f64]| {
        let worst = a
            .iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max);
        assert!(
            worst < 1e-9,
            "{what} does not wrap: worst texel off by {worst}"
        );
    };

    close(
        "blur",
        &nodes::blur(&moved, n, 0.07),
        &shifted(&nodes::blur(&src, n, 0.07), side, dx, dy),
    );
    close(
        "curvature",
        &nodes::curvature(&moved32, n, 0.06),
        &shifted(&nodes::curvature(&src32, n, 0.06), side, dx, dy),
    );
    close(
        "occlusion",
        &nodes::occlusion(&moved32, n, 1.0),
        &shifted(&nodes::occlusion(&src32, n, 1.0), side, dx, dy),
    );
    let by = Plane::Raster(src.iter().map(|v| v * 0.8).collect());
    let by_moved = Plane::Raster(shifted(&by.texels(side * side), side, dx, dy));
    close(
        "warp",
        &nodes::warp(&moved, &by_moved, n, 0.09),
        &shifted(&nodes::warp(&src, &by, n, 0.09), side, dx, dy),
    );
}

/// Determinism is the contract the whole crate rests on: same graph, same bytes.
#[test]
fn baking_a_graph_is_deterministic() {
    let g = every_kind_graph();
    assert_eq!(bake_graph(&g, 48).unwrap(), bake_graph(&g, 48).unwrap());
    assert_eq!(
        bake_graph(&g, 48).unwrap(),
        bake_graph(&g.clone(), 48).unwrap()
    );
    for r in presets::all() {
        assert_eq!(
            MaterialGraph::from_recipe(&r),
            MaterialGraph::from_recipe(&r)
        );
    }
    let rolled = MaterialGraph::from_recipe(&random(9));
    assert_eq!(
        bake_graph(&rolled, 40).unwrap(),
        bake_graph(&rolled, 40).unwrap()
    );
}

/// The seven maps come out complete, sized and opaque from a hand-built graph too.
#[test]
fn a_hand_built_graph_bakes_a_complete_opaque_set() {
    let set = bake_graph(&every_kind_graph(), 24).expect("bakes");
    assert_eq!(set.size, 24);
    for kind in MapKind::ALL {
        let m = set.get(kind).unwrap_or_else(|| panic!("{kind:?} missing"));
        assert_eq!(m.pixels.len(), 24 * 24 * 4, "{kind:?}");
        assert!(m.pixels.chunks(4).all(|p| p[3] == 255), "{kind:?} alpha");
    }
    // An output with nothing wired is still a whole, honest set: neutral grey, flat,
    // dark emit — not an error and not black.
    let bare = graph(vec![], Output::default());
    let set = bake_graph(&bare, 8).expect("an unwired output bakes");
    assert!(set
        .get(MapKind::BaseColor)
        .unwrap()
        .pixels
        .chunks(4)
        .all(|p| p == [128, 128, 128, 255]));
    assert!(set
        .get(MapKind::Normal)
        .unwrap()
        .pixels
        .chunks(4)
        .all(|p| p == [128, 128, 255, 255]));
    assert!(set
        .get(MapKind::Emit)
        .unwrap()
        .pixels
        .chunks(4)
        .all(|p| p == [0, 0, 0, 255]));
}

/// THE ORDER A READER SEES IS THE ORDER THE BAKE RUNS: `eval_order` walks the wires with
/// the evaluator's own walk — every node once, each after everything that feeds it, loose
/// nodes in list order — and stays total on a graph that cannot bake: a loop is cut where
/// the walk meets it, a dangling wire is skipped.
#[test]
fn eval_order_is_dependencies_first_and_total() {
    let g = MaterialGraph::from_recipe(&presets::granite());
    let order = g.eval_order();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        (0..g.nodes.len()).collect::<Vec<_>>(),
        "every node exactly once"
    );
    let at: std::collections::HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(pos, &i)| (g.nodes[i].key.as_str(), pos))
        .collect();
    for (pos, &i) in order.iter().enumerate() {
        for key in g.nodes[i].inputs.iter().flatten() {
            assert!(
                at[key.as_str()] < pos,
                "{} is fed by {key}, which must come before it",
                g.nodes[i].key
            );
        }
    }
    // A loop (a ⇄ b), a dangling wire (c) and a loose node (d): total, each once, the loop
    // cut where the walk met it.
    let broken = graph(
        vec![
            node("a", NodeKind::Invert {}, &["b"]),
            node("b", NodeKind::Blur { radius: 0.1 }, &["a"]),
            node("c", NodeKind::Invert {}, &["gone"]),
            node("d", NodeKind::Constant { value: 0.5 }, &[]),
        ],
        Output::default(),
    );
    assert_eq!(broken.eval_order(), vec![1, 0, 2, 3]);
    assert!(graph(Vec::new(), Output::default()).eval_order().is_empty());
}

/// `Output::sources` is the wiring AS WRITTEN, position for position with the one port
/// table — a constant scalar is no wire, and a fresh output wires nothing.
#[test]
fn output_sources_follow_the_one_port_table() {
    let out = Output {
        base: Some("base".into()),
        height: Some("height".into()),
        roughness: Scalar::Field("roughness".into()),
        metalness: Scalar::Const(0.0),
        ao: Scalar::Field("ao".into()),
        emit: Some(Emit {
            color: "emit.color".into(),
            mask: Some("emit.mask".into()),
            strength: 1.0,
        }),
        ..Output::default()
    };
    for (port, source) in OUTPUT_PORTS.iter().zip(out.sources()) {
        if port.name == "metalness" {
            assert_eq!(source, None, "a constant scalar is no wire");
        } else {
            assert_eq!(source, Some(port.name), "{} reads its own wire", port.name);
        }
    }
    assert_eq!(Output::default().sources(), [None; OUTPUT_PORTS.len()]);
}

/// THE GLOW PREDICATE: every one of the eight authored glows glows; the same graph at
/// strength 0, or with its Emit removed, does not; nor does a converted rack recipe
/// (granite has no Emit).
#[test]
fn a_graph_glows_exactly_when_its_emit_is_wired_above_zero() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/glow_materials.json"
    ))
    .expect("the authored glows read");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("the glows parse");
    let glows: Vec<MaterialGraph> = doc["graphs"]
        .as_array()
        .expect("a `graphs` list")
        .iter()
        .map(|g| load_material_json(&g.to_string()).expect("a glow loads"))
        .collect();
    assert_eq!(glows.len(), 8, "the eight curated glows");
    for g in &glows {
        assert!(g.glows(), "{} glows", g.id);
        let mut dark = g.clone();
        dark.output.emit.as_mut().expect("an Emit").strength = 0.0;
        assert!(!dark.glows(), "{} at strength 0 does not", g.id);
        let mut bare = g.clone();
        bare.output.emit = None;
        assert!(!bare.glows(), "{} with no Emit does not", g.id);
    }
    assert!(
        !MaterialGraph::from_recipe(&presets::granite()).glows(),
        "converted granite has no glow"
    );
}

/// A graph that cannot evaluate returns WHY — never a panic, never a black map.
#[test]
fn a_cycle_is_an_error() {
    let looped = graph(
        vec![
            node("a", NodeKind::Invert {}, &["b"]),
            node("b", NodeKind::Blur { radius: 0.1 }, &["a"]),
        ],
        Output {
            height: Some("a".into()),
            ..Output::default()
        },
    );
    match bake_graph(&looped, 16) {
        Err(GraphError::Cycle { path }) => {
            assert!(path.len() >= 3, "{path:?}");
            assert_eq!(path.first(), path.last(), "the loop closes: {path:?}");
            assert!(path.contains(&"a".into()) && path.contains(&"b".into()));
        }
        other => panic!("expected a cycle, got {other:?}"),
    }
    // A preview of a node ON the loop refuses it; one OFF it does not care, and the
    // designer hears about the loop from `problems`.
    let mut with_bystander = looped.clone();
    with_bystander
        .nodes
        .push(node("c", NodeKind::Constant { value: 0.5 }, &[]));
    assert!(matches!(
        field_of(&with_bystander, "b", 8),
        Err(GraphError::Cycle { .. })
    ));
    assert!(field_of(&with_bystander, "c", 8).is_ok());
    assert!(matches!(
        problems(&with_bystander).as_slice(),
        [GraphError::Cycle { .. }]
    ));
    // A node fed by itself.
    let selfish = graph(
        vec![node("s", NodeKind::Invert {}, &["s"])],
        Output {
            height: Some("s".into()),
            ..Output::default()
        },
    );
    assert_eq!(
        bake_graph(&selfish, 8).unwrap_err(),
        GraphError::Cycle {
            path: vec!["s".into(), "s".into()]
        }
    );
}

#[test]
fn an_unknown_key_is_an_error() {
    let dangling = graph(
        vec![node("inv", NodeKind::Invert {}, &["nope"])],
        Output {
            height: Some("inv".into()),
            ..Output::default()
        },
    );
    let err = bake_graph(&dangling, 8).unwrap_err();
    assert_eq!(
        err,
        GraphError::UnknownNode {
            key: "nope".into(),
            at: "inv.field".into()
        }
    );
    assert!(err.to_string().contains("nope") && err.to_string().contains("inv.field"));

    let bad_output = graph(
        vec![node("k", NodeKind::Constant { value: 1.0 }, &[])],
        Output {
            roughness: Scalar::Field("missing".into()),
            ..Output::default()
        },
    );
    assert_eq!(
        bake_graph(&bad_output, 8).unwrap_err(),
        GraphError::UnknownNode {
            key: "missing".into(),
            at: "output.roughness".into()
        }
    );
    assert!(matches!(
        field_of(&every_kind_graph(), "no-such-node", 8),
        Err(GraphError::UnknownNode { .. })
    ));
}

#[test]
fn a_port_type_mismatch_is_an_error() {
    let pal = || node("pal", NodeKind::Palette { rgb: [1.0; 3] }, &[]);
    let field_n = || node("f", NodeKind::Constant { value: 0.5 }, &[]);

    // A colour into a field port.
    let g = graph(
        vec![pal(), node("inv", NodeKind::Invert {}, &["pal"])],
        Output {
            height: Some("inv".into()),
            ..Output::default()
        },
    );
    let err = bake_graph(&g, 8).unwrap_err();
    assert_eq!(
        err,
        GraphError::PortType {
            at: "inv.field".into(),
            wants: ValueKind::Field,
            source: "pal".into(),
            found: ValueKind::Colour
        }
    );
    assert!(err.to_string().contains("colour"), "{err}");

    // A field into a colour port.
    let g = graph(
        vec![
            pal(),
            field_n(),
            masked("mx", NodeKind::MixColor { amount: 0.5 }, "pal", "f", None),
        ],
        Output {
            base: Some("mx".into()),
            ..Output::default()
        },
    );
    assert!(matches!(
        bake_graph(&g, 8),
        Err(GraphError::PortType { at, .. }) if at == "mx.b"
    ));

    // The output's ports are typed as well.
    for (output, at) in [
        (
            Output {
                height: Some("pal".into()),
                ..Output::default()
            },
            "output.height",
        ),
        (
            Output {
                base: Some("f".into()),
                ..Output::default()
            },
            "output.base",
        ),
        (
            Output {
                emit: Some(Emit {
                    color: "pal".into(),
                    mask: Some("pal".into()),
                    strength: 1.0,
                }),
                ..Output::default()
            },
            "output.emit.mask",
        ),
    ] {
        let g = graph(vec![pal(), field_n()], output);
        assert!(
            matches!(bake_graph(&g, 8), Err(GraphError::PortType { at: a, .. }) if a == at),
            "{at} accepted the wrong type"
        );
    }

    // A field preview of a colour node is the same mistake.
    assert!(matches!(
        field_of(&every_kind_graph(), "pl", 8),
        Err(GraphError::PortType { .. })
    ));
    assert!(matches!(
        colour_of(&every_kind_graph(), "n1", 8),
        Err(GraphError::PortType { .. })
    ));
}

/// THE ONE OUTPUT TABLE: every port the validator types is typed by [`OUTPUT_PORTS`] —
/// wire each port the WRONG kind and the error names `output.<the table's name>` and wants
/// the table's kind. A designer reads the same table (its output card is laid out by it),
/// so the two cannot disagree about what a port takes.
#[test]
fn every_output_port_is_typed_by_the_one_table() {
    let pal = || node("pal", NodeKind::Palette { rgb: [1.0; 3] }, &[]);
    let f = || node("f", NodeKind::Constant { value: 0.5 }, &[]);
    assert_eq!(Output::ports().len(), 7);
    assert_eq!(Output::ports(), &OUTPUT_PORTS);
    for (i, port) in Output::ports().iter().enumerate() {
        // The wrong kind for this port: a colour where it takes a field, and the reverse.
        let wrong = match port.kind {
            ValueKind::Field => "pal",
            ValueKind::Colour => "f",
        };
        let mut out = Output::default();
        match i {
            0 => out.base = Some(wrong.into()),
            1 => out.height = Some(wrong.into()),
            2 => out.roughness = Scalar::Field(wrong.into()),
            3 => out.metalness = Scalar::Field(wrong.into()),
            4 => out.ao = Scalar::Field(wrong.into()),
            5 => {
                out.emit = Some(Emit {
                    color: wrong.into(),
                    mask: None,
                    strength: 1.0,
                })
            }
            _ => {
                out.emit = Some(Emit {
                    color: "pal".into(),
                    mask: Some(wrong.into()),
                    strength: 1.0,
                })
            }
        }
        let g = graph(vec![pal(), f()], out);
        let at = format!("output.{}", port.name);
        assert!(
            problems(&g).iter().any(|p| matches!(
                p,
                GraphError::PortType { at: a, wants, .. } if *a == at && *wants == port.kind
            )),
            "{at} is not typed by the table: {:?}",
            problems(&g)
        );
    }
}

#[test]
fn a_missing_input_is_an_error() {
    let k = || node("k", NodeKind::Constant { value: 0.5 }, &[]);
    // A blend with its `b` unwired.
    let g = graph(
        vec![
            k(),
            Node {
                key: "mix".into(),
                kind: blend(BlendMode::Add, 1.0),
                inputs: vec![Some("k".into()), None],
            },
        ],
        Output {
            height: Some("mix".into()),
            ..Output::default()
        },
    );
    assert_eq!(
        bake_graph(&g, 8).unwrap_err(),
        GraphError::MissingInput { at: "mix.b".into() }
    );
    // A filter with no inputs at all, OFF the output's path: the bake does not read it
    // and does not fail on it; `problems` names it.
    let g = graph(
        vec![k(), node("lonely", NodeKind::Invert {}, &[])],
        Output::default(),
    );
    assert!(bake_graph(&g, 8).is_ok());
    assert_eq!(
        problems(&g),
        vec![GraphError::MissingInput {
            at: "lonely.field".into()
        }]
    );
    // A mask is the one optional port: unwired is fine.
    let g = graph(
        vec![k(), masked("m", blend(BlendMode::Max, 0.5), "k", "k", None)],
        Output {
            height: Some("m".into()),
            ..Output::default()
        },
    );
    assert!(bake_graph(&g, 8).is_ok());
}

#[test]
fn duplicate_keys_and_extra_inputs_are_errors() {
    let g = graph(
        vec![
            node("k", NodeKind::Constant { value: 0.1 }, &[]),
            node("k", NodeKind::Constant { value: 0.2 }, &[]),
        ],
        Output::default(),
    );
    assert_eq!(
        bake_graph(&g, 8).unwrap_err(),
        GraphError::DuplicateKey { key: "k".into() }
    );
    let g = graph(
        vec![
            node("k", NodeKind::Constant { value: 0.1 }, &[]),
            node("inv", NodeKind::Invert {}, &["k", "k"]),
        ],
        Output {
            height: Some("inv".into()),
            ..Output::default()
        },
    );
    assert_eq!(
        bake_graph(&g, 8).unwrap_err(),
        GraphError::UnknownPort {
            node: "inv".into(),
            index: 1
        }
    );
}

/// The document IS the artifact: a lossy field would silently change a surface the
/// next time it opened. Every converted patch and the every-kind graph survive a
/// JSON round trip byte-identically.
#[test]
fn graphs_round_trip_through_json_byte_identically() {
    let mut graphs: Vec<MaterialGraph> = fidelity_corpus()
        .iter()
        .map(|(_, r)| MaterialGraph::from_recipe(r))
        .collect();
    graphs.push(every_kind_graph());
    for g in graphs {
        for text in [
            serde_json::to_string_pretty(&g).unwrap(),
            serde_json::to_string(&g).unwrap(),
        ] {
            let back: MaterialGraph =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}\n{text}", g.id));
            assert_eq!(back, g, "{}: value drifted", g.id);
            let again = if text.contains('\n') {
                serde_json::to_string_pretty(&back).unwrap()
            } else {
                serde_json::to_string(&back).unwrap()
            };
            assert_eq!(again, text, "{}: bytes drifted", g.id);
        }
    }
}

/// The v2 document is versioned and reads as data: params inline on the node, the
/// kind as its tag, a constant scalar as a bare number.
#[test]
fn the_v2_document_is_versioned_and_flat() {
    let g = MaterialGraph::from_recipe(&presets::granite());
    let v = serde_json::to_value(&g).unwrap();
    assert_eq!(v["version"], 2);
    let first = &v["nodes"][0];
    assert_eq!(first["key"], "ch1");
    assert_eq!(first["kind"], "noise");
    assert_eq!(first["source"], "fbm");
    assert_eq!(v["output"]["roughness"], "rough");
    let mut bare = g.clone();
    bare.output.metalness = Scalar::Const(0.25);
    assert_eq!(
        serde_json::to_value(&bare).unwrap()["output"]["metalness"],
        0.25
    );
    // Every kind's tag is its id, so a style or label keyed by `id()` names the
    // same thing the file does.
    for kind in NodeKind::catalog() {
        let tagged = serde_json::to_value(node("x", kind.clone(), &[])).unwrap();
        assert_eq!(tagged["kind"], kind.id());
    }
}

/// The catalog offers every kind once, and the every-kind graph really uses them all.
#[test]
fn the_catalog_and_the_test_graph_cover_every_kind() {
    let catalog: Vec<&str> = NodeKind::catalog().iter().map(NodeKind::id).collect();
    let mut unique = catalog.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), catalog.len(), "duplicate kind in the catalog");
    let mut used: Vec<&str> = every_kind_graph()
        .nodes
        .iter()
        .map(|n| n.kind.id())
        .collect();
    used.sort_unstable();
    used.dedup();
    assert_eq!(used, unique, "the every-kind graph misses a kind");
    // A noise node is a voice and back.
    let voice = presets::granite().channels[1];
    assert_eq!(
        NodeKind::noise(&voice)
            .voice()
            .map(|c| (c.source, c.scale, c.salt, c.contrast)),
        Some((voice.source, voice.scale, voice.salt, voice.contrast))
    );
}

/// `load_material_json` reads a v1 recipe — written by the v1 serializer, the form
/// every committed bundle is in — as the graph it converts to, and it bakes as the
/// recipe always did.
#[test]
fn load_reads_a_v1_recipe_document() {
    for (what, r) in fidelity_corpus().into_iter().take(9) {
        let v1 = serde_json::to_string_pretty(&r).unwrap();
        assert!(!v1.contains("\"version\""), "v1 has no version field");
        let g = load_material_json(&v1).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(g, MaterialGraph::from_recipe(&r), "{what}");
        assert_eq!(
            differing_bytes(&bake_graph(&g, 32).unwrap(), &bake(&r, 32)),
            vec![],
            "{what}"
        );
    }
    // A recipe explicitly marked version 1 reads the same way.
    let mut doc = serde_json::to_value(presets::basalt()).unwrap();
    doc["version"] = 1.into();
    assert_eq!(
        load_material_json(&doc.to_string()).unwrap(),
        MaterialGraph::from_recipe(&presets::basalt())
    );
}

#[test]
fn load_reads_a_v2_graph_document() {
    for g in [every_kind_graph(), MaterialGraph::from_recipe(&random(3))] {
        let text = serde_json::to_string_pretty(&g).unwrap();
        assert_eq!(load_material_json(&text).unwrap(), g);
        // Tolerant of a v2 document that omits its version.
        let mut doc = serde_json::to_value(&g).unwrap();
        doc.as_object_mut().unwrap().remove("version");
        assert_eq!(load_material_json(&doc.to_string()).unwrap(), g);
    }
}

#[test]
fn load_refuses_what_it_cannot_read() {
    assert!(matches!(
        load_material_json("not json"),
        Err(LoadError::Syntax(_))
    ));
    assert!(matches!(
        load_material_json("[1, 2]"),
        Err(LoadError::Syntax(_))
    ));
    let mut doc = serde_json::to_value(every_kind_graph()).unwrap();
    doc["version"] = 3.into();
    assert_eq!(
        load_material_json(&doc.to_string()).unwrap_err(),
        LoadError::UnsupportedVersion("3".into())
    );
    doc["version"] = "two".into();
    assert!(matches!(
        load_material_json(&doc.to_string()),
        Err(LoadError::UnsupportedVersion(_))
    ));
    assert!(matches!(
        load_material_json(r#"{"version": 2, "id": "x"}"#),
        Err(LoadError::Invalid { version: 2, .. })
    ));
    assert!(matches!(
        load_material_json("{}"),
        Err(LoadError::Invalid { version: 1, .. })
    ));
}

/// A layer is a blend with a mask, and a glow is a colour × mask: each acts ONLY
/// inside its mask. Counted, not eyeballed — every texel inside changes, none outside.
#[test]
fn a_masked_blend_and_a_masked_emit_act_only_inside_their_masks() {
    let n = 64usize;
    let g = graph(
        vec![
            node("across", gradient(Axis::U, 1), &[]),
            node("middle", hard_step(0.5), &["across"]),
            node("down", gradient(Axis::V, 1), &[]),
            node("rows", hard_step(0.5), &["down"]),
            node("bed", NodeKind::Constant { value: 0.2 }, &[]),
            node("grain", noise(NoiseKind::Worley, 8, 3), &[]),
            masked(
                "layer",
                blend(BlendMode::Base, 1.0),
                "bed",
                "grain",
                Some("middle"),
            ),
            node(
                "glow",
                NodeKind::Palette {
                    rgb: [1.0, 0.4, 0.1],
                },
                &[],
            ),
        ],
        Output {
            height: Some("layer".into()),
            emit: Some(Emit {
                color: "glow".into(),
                mask: Some("rows".into()),
                strength: 1.0,
            }),
            ..Output::default()
        },
    );
    let field = |key: &str| field_of(&g, key, n as u32).expect(key);
    let (middle, rows, layer, grain) = (
        field("middle"),
        field("rows"),
        field("layer"),
        field("grain"),
    );
    assert!(
        middle.iter().chain(&rows).all(|m| *m == 0.0 || *m == 1.0),
        "hard masks"
    );

    // The blend: the grain where the mask is open, the bed untouched where it is shut.
    let inside = middle.iter().filter(|m| **m == 1.0).count();
    let outside = n * n - inside;
    assert!(
        inside > n * n / 4 && outside > n * n / 4,
        "mask splits the tile: {inside}/{outside}"
    );
    let layered_inside = (0..n * n)
        .filter(|&i| middle[i] == 1.0 && layer[i] == grain[i])
        .count();
    let bed_outside = (0..n * n)
        .filter(|&i| middle[i] == 0.0 && layer[i] == 0.2f64 as f32)
        .count();
    assert_eq!(
        layered_inside, inside,
        "the layer must cover all of its mask"
    );
    assert_eq!(bed_outside, outside, "the layer leaked outside its mask");

    // The glow: lit exactly where its mask is open, black everywhere else.
    let set = bake_graph(&g, n as u32).expect("masked graph bakes");
    let emit = &set.get(MapKind::Emit).unwrap().pixels;
    let lit = |i: usize| emit[i * 4..i * 4 + 3] != [0, 0, 0];
    let glow_inside = rows.iter().filter(|m| **m == 1.0).count();
    let lit_inside = (0..n * n).filter(|&i| rows[i] == 1.0 && lit(i)).count();
    let lit_outside = (0..n * n).filter(|&i| rows[i] == 0.0 && lit(i)).count();
    assert!(
        glow_inside > n * n / 4,
        "the glow mask is not trivial: {glow_inside}"
    );
    assert_eq!(lit_inside, glow_inside, "every texel inside the mask glows");
    assert_eq!(lit_outside, 0, "{lit_outside} texels glow outside the mask");
    // In its own colour: palette × 1 × 1.
    let first = (0..n * n).find(|&i| rows[i] == 1.0).unwrap();
    assert_eq!(&emit[first * 4..first * 4 + 4], [255, 102, 26, 255]);
}

/// A Band under width 1 CLOSES: a soft window onto the field, seated in its measured
/// range — lit through the window, dark above it, and the open band (v1's) unchanged.
#[test]
fn a_closed_band_is_a_soft_window() {
    let n = 64usize;
    let g = graph(
        vec![
            node("ramp", gradient(Axis::U, 1), &[]),
            node("open", band(0.0, 1.0, 0.05), &["ramp"]),
            node("window", band(0.0, 0.5, 0.05), &["ramp"]),
        ],
        Output::default(),
    );
    let ramp = field_of(&g, "ramp", n as u32).unwrap();
    let open = field_of(&g, "open", n as u32).unwrap();
    let window = field_of(&g, "window", n as u32).unwrap();
    // The ramp spans 0..1, so the window opens at 0 and closes halfway up.
    let (mut inside, mut above) = (0, 0);
    for i in 0..n * n {
        let h = ramp[i];
        if (0.06..=0.44).contains(&h) {
            inside += 1;
            assert_eq!(window[i], 1.0, "dark inside the window at {h}");
        }
        if h > 0.5 {
            above += 1;
            assert_eq!(window[i], 0.0, "lit above the window at {h}");
            assert_eq!(open[i], 1.0, "the open band must stay lit at {h}");
        }
    }
    assert!(
        inside > n * n / 4 && above > n * n / 4,
        "{inside} / {above}"
    );
}
