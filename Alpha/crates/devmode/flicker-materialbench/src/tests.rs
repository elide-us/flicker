//! The designer's drift gates, its dispatcher's contract, and its layout.
//!
//! None of this needs a GPU: the authored tree, the frame tree, the Model, the pair
//! script, the styles, the dispatcher and the canvas mapping are all reachable without a
//! device. The gates pin the authored artifacts (scene tree · pair script · the graph's
//! own vocabulary — `NodeKind::catalog()`, `MapKind::ALL`, the enums) to each other, so a
//! rename in one is loud in the others.

use flicker::render::Vec2;
use flicker::script::{HudCommand, Value, ValueMap};
use flicker::ui::{run_ui, strings, UiFrame, UiInput, UiState};
use flicker_canvas::{CanvasEvents, Link, Press};
use flicker_texture::graph::ValueKind;
use flicker_texture::{load_material_json, MapKind, NodeKind, Scalar};

use super::*;
use crate::designer::{Card, OutPort, Sink};

// ── helpers ───────────────────────────────────────────────────────────────────

/// The shipped stringtable, so a walked frame carries resolved en-us text.
fn load_strings() {
    let table = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    strings::load_str(&table, "en-us");
}

/// Every node of a tree, flattened.
fn flatten<'a>(node: &'a UiNode, out: &mut Vec<&'a UiNode>) {
    out.push(node);
    for kid in &node.children {
        flatten(kid, out);
    }
}

fn nodes_of(tree: &UiNode) -> Vec<&UiNode> {
    let mut out = Vec::new();
    flatten(tree, &mut out);
    out
}

fn find<'a>(tree: &'a UiNode, id: &str) -> Option<&'a UiNode> {
    nodes_of(tree).into_iter().find(|n| n.id == id)
}

/// The AUTHORED tree, as the scene file wrote it.
fn authored(bench: &MaterialBench) -> UiNode {
    bench
        .authored_tree()
        .expect("the shipped scene file declares a tree")
}

/// The FRAME tree — the authored one with its rows expanded and its families filled.
fn frame_tree(bench: &mut MaterialBench) -> UiNode {
    bench.frame_tree().expect("the frame tree derives")
}

fn snap() -> UiInput {
    UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        right_down: false,
        screen: Vec2::new(1920.0, 1080.0),
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    }
}

fn styles(bench: &MaterialBench) -> serde_json::Value {
    flicker::ui::load_shared_styles(bench.scene_styles_json.as_ref())
}

/// One walked frame over the bench's FULL model (raw + derived) at 1920×1080 — what the
/// screen actually lays out and draws.
fn walk(bench: &mut MaterialBench) -> UiFrame {
    let tree = frame_tree(bench);
    let styles = styles(bench);
    run_ui(&tree, &bench.model(), &styles, &snap(), &mut UiState::new())
}

/// Every drawn text carrying a `$` ANYWHERE — a whole token that did not resolve, or one
/// composed into the middle of a line (which the draw boundary never resolves).
fn unresolved_tokens(commands: &[HudCommand]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|c| match c {
            HudCommand::Text { text, .. } if text.contains('$') => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Fire one result through the dispatcher, against the Model the bench publishes now —
/// the frame's controls were drawn from it.
fn fire(bench: &mut MaterialBench, key: &str, value: impl Into<Value>) -> bool {
    let published = bench.model();
    let mut results = ValueMap::default();
    results.set(key, value);
    route::apply(bench, &published, &results)
}

/// Every kind of graph error, once — what the status line must be able to speak.
fn every_graph_error() -> Vec<flicker_texture::GraphError> {
    use flicker_texture::GraphError as E;
    vec![
        E::DuplicateKey { key: "k".into() },
        E::UnknownNode {
            key: "k".into(),
            at: "output.height".into(),
        },
        E::UnknownPort {
            node: "n".into(),
            index: 3,
        },
        E::MissingInput {
            at: "n.field".into(),
        },
        E::PortType {
            at: "n.field".into(),
            wants: ValueKind::Field,
            source: "s".into(),
            found: ValueKind::Colour,
        },
        E::Cycle {
            path: vec!["a".into(), "b".into(), "a".into()],
        },
        E::NonFinite {
            node: "n".into(),
            param: "gain",
        },
        E::OutOfRange {
            node: "n".into(),
            param: "scale",
        },
        E::NonFiniteTexel { node: "n".into() },
    ]
}

/// A slot the shipped index lists no row for — bound all the same (a document may name
/// one), which the dropdown must show as its placeholder.
fn unlisted_slot(bench: &MaterialBench) -> flicker_materials::MaterialId {
    (1..=flicker_materials::MaterialId::MAX)
        .find(|&id| bench.materials.iter().all(|def| def.id != id))
        .expect("the index leaves a slot free")
}

/// A bench on the authored NEW default — one noise driving the height through the
/// stone ramp: `[noise (field), ramp (colour ← noise)]`, base ← ramp, height ← noise.
fn on_new() -> MaterialBench {
    let mut bench = MaterialBench::shipped();
    assert!(
        fire(&mut bench, "sel_preset", 0.0),
        "loading New is an edit"
    );
    assert_eq!(bench.preset, Some(0));
    bench
}

/// The v1 rack's GRANITE as the graph it converts to (`MaterialGraph::from_recipe`, the
/// shape a Roll arrives in) — the fixture the canvas-geometry tests were measured on: a
/// GLOWING graph of noise voices `ch0…` with an `ao` node at its edge. The shipped preset
/// library is authored data Aaron tunes, so a test that needs one particular SHAPE of graph
/// holds its own rather than borrowing a preset's.
fn rack_granite() -> MaterialGraph {
    MaterialGraph::from_recipe(&flicker_texture::presets::granite())
}

/// Insert a node of kind `id` through the dispatcher (the popup's answer) and return its
/// key — it is selected.
fn insert(bench: &mut MaterialBench, id: &str) -> String {
    assert!(
        fire(bench, &format!("insert_{id}"), true),
        "inserting {id} is an edit"
    );
    bench
        .designer
        .selected_key()
        .expect("the new node is selected")
        .to_string()
}

fn select(bench: &mut MaterialBench, key: &str) {
    let i = bench.designer.index_of(key).expect("the node exists");
    fire(bench, &format!("nl_pick_{i}"), true);
    assert_eq!(bench.designer.selected_key(), Some(key));
}

/// The designer's invariants after any mutation: the problems list is the graph's own
/// (`problems()` re-ran), and the document round-trips through the loader every reader
/// uses — it is still a valid v2 graph.
fn consistent(bench: &MaterialBench) {
    assert_eq!(
        bench.designer.problems(),
        flicker_texture::problems(bench.graph()).as_slice(),
        "the problems list went stale"
    );
    let text = serde_json::to_string(bench.graph()).expect("the graph serializes");
    assert_eq!(
        &load_material_json(&text).expect("the graph loads back"),
        bench.graph(),
        "the graph survives its own document"
    );
}

/// The card index of `key` on the canvas (after a sync).
fn card(bench: &mut MaterialBench, key: &str) -> usize {
    bench.sync_canvas();
    bench
        .canvas_content
        .keys
        .iter()
        .position(|k| k == key)
        .unwrap_or_else(|| panic!("no card for {key}"))
}

fn output_card(bench: &mut MaterialBench) -> usize {
    bench.sync_canvas();
    bench
        .canvas_content
        .cards
        .iter()
        .position(|c| *c == Card::Output)
        .expect("the output card is always laid out")
}

fn link(bench: &mut MaterialBench, from: usize, to: usize, port: Option<usize>) -> bool {
    let events = CanvasEvents {
        pressed: None,
        linked: Some(Link { from, to: Some(to) }),
    };
    route::canvas(bench, &events, port.map(|k| (to, k)))
}

fn press(bench: &mut MaterialBench, card: Option<usize>, edge: Option<usize>) -> bool {
    let events = CanvasEvents {
        pressed: Some(Press {
            card,
            edge,
            local: Vec2::ZERO,
        }),
        linked: None,
    };
    route::canvas(bench, &events, None)
}

// ── the scene file ────────────────────────────────────────────────────────────

/// The scene file itself parses — the manifest's copy and this crate's are the same
/// bytes, so this is the authoring gate.
#[test]
fn the_shipped_scene_file_parses_with_a_tree_and_styles() {
    let def = flicker::ui::SceneDef::parse("materialbench", SW_SCENE).expect("scene file parses");
    assert_eq!(def.behaviour, "materialbench");
    assert!(def.tree.is_some(), "the scene file declares a tree");
    assert!(def.styles.is_some(), "it carries this scene's style blocks");
    assert!(
        def.params.contains_key(NEW_MATERIAL_PARAM),
        "it authors the NEW-material default as data"
    );
}

/// THE VOCABULARY gate, on the authored tree AND the walked one: an unknown kind renders
/// nothing, so a typo would be an invisible hole. Standard kinds only.
#[test]
fn the_designer_names_only_known_kinds() {
    let mut bench = MaterialBench::shipped();
    for tree in [authored(&bench), frame_tree(&mut bench)] {
        let unknown = flicker::ui::unknown_kinds(&tree);
        assert!(
            unknown.is_empty(),
            "the designer names unknown kinds: {unknown:?}"
        );
    }
}

/// The STRINGS gate (tree channel), on both trees: every display literal is a `$token`.
#[test]
fn the_designer_ships_no_raw_display_literals() {
    let mut bench = MaterialBench::shipped();
    for tree in [authored(&bench), frame_tree(&mut bench)] {
        let raw = flicker::ui::raw_display_literals(&tree);
        assert!(raw.is_empty(), "raw display literals: {raw:?}");
    }
}

/// Every `$token` the scene authors, the bench composes, or the dispatcher leaves as a
/// notice EXISTS in the stringtable — a token that resolves to nothing draws blank (or
/// raw) and looks like a layout bug. Option labels are not drawn until their menu opens,
/// so they are checked here rather than through a frame.
#[test]
fn every_token_the_designer_names_exists() {
    load_strings();
    let bench = MaterialBench::shipped();
    let mut tokens: Vec<String> = Vec::new();
    fn collect(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) if s.starts_with('$') && !s.starts_with("$$") => {
                // A `$token` reference — not a `$stone3` palette token in a style block,
                // which this only walks the TREE for.
                out.push(s.clone());
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| collect(x, out)),
            _ => {}
        }
    }
    let doc: serde_json::Value = serde_json::from_str(SW_SCENE).expect("scene parses");
    collect(&doc["tree"], &mut tokens);
    for kind in NodeKind::catalog() {
        tokens.push(format!("$mtl_kind_{}", kind.id()));
    }
    for c in bench.palette.iter() {
        tokens.push(format!("$mtl_pal_{}", c.id));
    }
    for e in every_graph_error() {
        tokens.push(designer::error_parts(&e).0.to_string());
    }
    for p in OutPort::ALL {
        tokens.push(format!("$mtl_outport_{}", p.table().name.replace('.', "_")));
    }
    tokens.extend(
        [
            designer::WIRE_MISTYPED,
            designer::WIRE_LOOPS,
            designer::WIRE_FROM_OUTPUT,
            designer::WIRE_NO_GLOW,
            designer::WIRE_NO_PORT,
            designer::WIRE_NO_FREE_PORT,
            "$mtl_commit_io_failed",
            "$mtl_output",
            "$mtl_insert_title",
            "$mtl_cancel",
            "$mtl_unbound",
            "$mtl_commit_idle",
            "$mtl_commit_working",
            "$mtl_body_sphere",
            "$mtl_body_plane",
            commit::Refusal::Unbound.token(),
            commit::Refusal::SlotClosed.token(),
            COMMITTED_UNREADABLE,
            NO_EMIT_WARNING,
            // The committed-look prompt's own copy.
            "$mtl_committed_title",
            "$mtl_committed_body",
            "$mtl_open_committed",
            "$mtl_open_committed_discard",
            "$mtl_bind_here",
            // The shared unsaved-changes preset a load over unsaved work raises.
            "$modal_unsaved_title",
            "$modal_unsaved_body",
            "$modal_lbl_discard",
            "$modal_lbl_keep_editing",
        ]
        .map(str::to_string),
    );
    let missing: Vec<&String> = tokens
        .iter()
        .filter(|t| strings::resolve(t).as_ref() == t.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "tokens with no stringtable row: {missing:?}"
    );
}

/// Every token a walked frame DRAWS resolves — with every node kind's cell shown in turn,
/// so all nineteen parameter cells are drawn at least once.
#[test]
fn every_drawn_token_resolves_in_every_parameter_cell() {
    load_strings();
    let mut bench = on_new();
    let mut unresolved = unresolved_tokens(&walk(&mut bench).commands);
    for kind in NodeKind::catalog() {
        insert(&mut bench, kind.id());
        unresolved.extend(unresolved_tokens(&walk(&mut bench).commands));
    }
    assert!(unresolved.is_empty(), "unresolved tokens: {unresolved:?}");
}

/// Every DECLARED result reaches a handler: the rail names are the authored rail's to
/// step, the page names step the preset library in the dispatcher.
#[test]
fn every_declared_intent_reaches_a_handler() {
    let bench = MaterialBench::shipped();
    let tree = authored(&bench);
    let rail = find(&tree, "mtl_views").expect("the view rail is authored");
    assert_eq!(
        rail.props.get("next_action"),
        Some(&Value::Text("map_next".into()))
    );
    assert_eq!(
        rail.props.get("prev_action"),
        Some(&Value::Text("map_prev".into()))
    );
    let mut bench = MaterialBench::shipped();
    let before = bench.preset;
    assert!(
        fire(&mut bench, "patch_next", true),
        "patch_next loads a preset"
    );
    assert_ne!(bench.preset, before, "patch_next reaches the dispatcher");
    fire(&mut bench, "patch_prev", true);
    assert_eq!(bench.preset, before, "patch_prev steps back");
}

// ── the three authored artifacts stay in lockstep ─────────────────────────────

/// The view tabs are one vocabulary with `MapKind::ALL` plus the LIT view.
#[test]
fn the_view_tabs_cover_every_map_and_the_lit_view() {
    assert_eq!(MAP_IDS.len(), MapKind::ALL.len());
    let tree = authored(&MaterialBench::shipped());
    let tabs = nodes_of(&tree)
        .into_iter()
        .find(|n| n.component == "tabs" && n.bind.as_deref() == Some("sel_map"))
        .expect("the view selector binds sel_map");
    let options = tabs
        .children
        .iter()
        .filter(|k| k.component == "option")
        .count();
    assert_eq!(
        options, VIEW_COUNT,
        "the tabs offer every view exactly once"
    );
}

/// Every view cell exists, gated by the visibility bind the pair script derives, and
/// each flat map's sprite names ITS preview texture — after the theme's own textures in
/// the HUD's table, so the footer's glyph atlas (`pad_glyphs.tex`) is the theme's and
/// never a swatch (the old table put the previews first and indexed the roughness map).
#[test]
fn every_view_cell_names_its_preview_slot_after_the_theme() {
    let bench = MaterialBench::shipped();
    let tree = authored(&bench);
    let all = nodes_of(&tree);
    for (i, id) in MAP_IDS.iter().enumerate() {
        let bind = format!("{id}_shown");
        let cell = all
            .iter()
            .find(|n| n.visible_bind.as_deref() == Some(bind.as_str()))
            .unwrap_or_else(|| panic!("no view cell is gated by {bind}"));
        let sprite = cell
            .children
            .iter()
            .find(|k| k.component == "sprite")
            .unwrap_or_else(|| panic!("{bind}'s cell holds no sprite"));
        assert_eq!(
            sprite.props.get("tex"),
            Some(&Value::Number((MAP_TEX_BASE + i) as f64)),
            "{id} names preview slot {}",
            MAP_TEX_BASE + i
        );
    }
    assert!(all
        .iter()
        .any(|n| n.visible_bind.as_deref() == Some("lit_shown")));
    assert!(all
        .iter()
        .any(|n| n.component == "surface" && n.id == LIT_SLOT));
    let glyphs = styles(&bench)["pad_glyphs"]["tex"].as_f64();
    let atlas = Theme::TEXTURE_NAMES
        .iter()
        .position(|n| *n == "pad_glyphs")
        .expect("the theme names its glyph atlas");
    assert_eq!(
        glyphs,
        Some(atlas as f64),
        "pad_glyphs.tex indexes the theme's atlas"
    );
}

/// Exactly one view cell is shown for every position of the bound number.
#[test]
fn exactly_one_view_cell_is_shown() {
    let mut bench = MaterialBench::shipped();
    for view in 0..VIEW_COUNT {
        fire(&mut bench, "sel_map", view as f64);
        let model = bench.model();
        let shown: Vec<String> = MAP_IDS
            .iter()
            .map(|id| format!("{id}_shown"))
            .chain(["lit_shown".to_string()])
            .filter(|key| matches!(model.get(key), Some(Value::Bool(true))))
            .collect();
        let expected = MAP_IDS
            .get(view)
            .map(|id| format!("{id}_shown"))
            .unwrap_or_else(|| "lit_shown".to_string());
        assert_eq!(shown, vec![expected], "view {view}");
    }
}

/// THE PARAMETER-CELL DRIFT GATE, pinned to `NodeKind::catalog()`: one authored cell per
/// kind (`cell_<kind>`, gated by `cell_<kind>_shown`); inside it, a control for EVERY dial
/// the kind has — a slider for each number, a select for each enum (listing exactly the
/// enum, in order), a toggle for each flag, the ramp's tint knobs, the palette's curated
/// select — a SOURCE select per input port (option 0 authored, the rest repeated from that
/// port's candidates, `rows_from: in<k>`), and nothing bound to a dial the kind does not
/// have. Every slider's authored
/// range holds the kind's fresh value, so a fresh node never shows off its own rail.
#[test]
fn one_parameter_cell_per_node_kind_with_a_control_for_every_dial() {
    let bench = MaterialBench::shipped();
    let tree = authored(&bench);
    let catalog = NodeKind::catalog();
    let cells: Vec<&UiNode> = nodes_of(&tree)
        .into_iter()
        .filter(|n| n.id.starts_with("cell_"))
        .collect();
    assert_eq!(
        cells.len(),
        catalog.len(),
        "one parameter cell per node kind"
    );
    for kind in &catalog {
        let id = kind.id();
        let cell = find(&tree, &format!("cell_{id}"))
            .unwrap_or_else(|| panic!("no parameter cell for {id}"));
        assert_eq!(
            cell.visible_bind.as_deref(),
            Some(format!("cell_{id}_shown").as_str()),
            "{id}'s cell is gated by the pair script"
        );
        let controls: Vec<&UiNode> = nodes_of(cell)
            .into_iter()
            .filter(|n| n.bind.is_some())
            .collect();
        let control = |bind: &str| {
            controls
                .iter()
                .find(|n| n.bind.as_deref() == Some(bind))
                .copied()
                .unwrap_or_else(|| panic!("{id}: no control bound `{bind}`"))
        };
        let knobs = designer::knobs(kind, &bench.palette);
        for (bind, value) in &knobs {
            let c = control(bind);
            let param = bind.trim_start_matches("node_");
            match (param, value) {
                ("rgb", _) => {
                    assert_eq!(c.component, "select", "{id}.rgb is the curated palette");
                    assert_eq!(c.props.get("rows_from"), Some(&Value::Text("glow".into())));
                    // The palette and NOTHING else — no "custom" option that picks
                    // nothing: a colour off it is the placeholder, a display state.
                    assert_eq!(c.children.len(), 1, "{id}.rgb: the prototype alone");
                    assert_eq!(c.children[0].props.get("value"), Some(&Value::Number(0.0)));
                    assert!(c.props.contains_key("placeholder"));
                }
                (_, Value::Bool(_)) => assert_eq!(c.component, "toggle", "{bind} is a toggle"),
                (p, Value::Number(v)) => match designer::enum_ids(p) {
                    Some(ids) => {
                        assert_eq!(c.component, "select", "{bind} is a select");
                        let labels: Vec<&Value> = c
                            .children
                            .iter()
                            .filter_map(|o| o.props.get("label"))
                            .collect();
                        assert_eq!(labels.len(), ids.len(), "{bind} lists the whole enum");
                        let values: Vec<f64> = c
                            .children
                            .iter()
                            .filter_map(|o| match o.props.get("value") {
                                Some(Value::Number(n)) => Some(*n),
                                _ => None,
                            })
                            .collect();
                        assert_eq!(
                            values,
                            (0..ids.len()).map(|i| i as f64).collect::<Vec<_>>(),
                            "{bind}: an option's value is its index"
                        );
                        for (label, id_) in labels.iter().zip(&ids) {
                            let Value::Text(t) = label else {
                                panic!("{bind}: an option label is text")
                            };
                            assert!(
                                t.ends_with(&format!("_{id_}")),
                                "{bind}: option `{t}` is out of the enum's order (`{id_}`)"
                            );
                        }
                    }
                    None => {
                        assert_eq!(c.component, "slider", "{bind} is a slider");
                        let num = |k: &str| match c.props.get(k) {
                            Some(Value::Number(n)) => *n,
                            other => panic!("{bind}: `{k}` is {other:?}"),
                        };
                        let (lo, hi) = (num("min"), num("max"));
                        assert!(lo < hi, "{bind}: an empty range");
                        assert!(
                            (lo..=hi).contains(v),
                            "{id}: the fresh {param} {v} is off its slider's {lo}..{hi}"
                        );
                    }
                },
                other => panic!("{bind}: unexpected dial {other:?}"),
            }
        }
        for (k, port) in kind.ports().iter().enumerate() {
            let c = control(&format!("node_in{k}"));
            assert_eq!(
                c.component, "select",
                "{id}.{} is a source select",
                port.name
            );
            assert_eq!(
                c.props.get("rows_from"),
                Some(&Value::Text(format!("in{k}"))),
                "{id}.{} lists what port {k} may be wired from",
                port.name,
            );
            assert_eq!(
                c.tab_group, "mtl_params",
                "{id}.{} is in the params ring",
                port.name
            );
        }
        // Nothing in the cell binds a dial or a port the kind does not have.
        for c in &controls {
            let bind = c.bind.as_deref().unwrap_or_default();
            let known = knobs.iter().any(|(k, _)| k == bind)
                || (0..kind.ports().len()).any(|k| bind == format!("node_in{k}"));
            assert!(
                known,
                "{id}'s cell binds `{bind}`, which the kind does not have"
            );
        }
    }
    // Every float the evaluator checks for finiteness is reachable by some dial: the
    // serialized numbers (sliders), the ramp (its tint) and the palette (the curated
    // select) — `NOT_KNOBS` (the salt) is the one exception, and it is an integer.
    for kind in &catalog {
        let doc = serde_json::to_value(kind).expect("a kind serializes");
        for (field, v) in doc.as_object().expect("a kind is an object") {
            if field == "kind" || designer::NOT_KNOBS.contains(&field.as_str()) {
                continue;
            }
            let covered = match (field.as_str(), v) {
                ("ramp", _) => true,
                ("rgb", _) => true,
                _ => designer::knobs(kind, &bench.palette)
                    .iter()
                    .any(|(k, _)| k == &format!("node_{field}")),
            };
            assert!(covered, "{}.{field} has no dial", kind.id());
        }
    }
}

/// The OUTPUT STAGE cell: a source select per colour/field port, the relief, each scalar
/// port as a source select over a constant slider (shown only while constant), and the
/// glow — its colour over the curated palette, its mask, its strength.
#[test]
fn the_output_stage_cell_is_complete() {
    let tree = authored(&MaterialBench::shipped());
    let cell = find(&tree, "mtl_output_stage").expect("the output stage is authored");
    let control = |bind: &str| {
        nodes_of(cell)
            .into_iter()
            .find(|n| n.bind.as_deref() == Some(bind))
            .unwrap_or_else(|| panic!("no output control bound `{bind}`"))
    };
    for (bind, family) in [
        ("out_base", "colour"),
        ("out_height", "field"),
        ("out_rough_src", "field"),
        ("out_metal_src", "field"),
        ("out_ao_src", "field"),
        ("out_emit_mask", "field"),
        ("out_emit_color", "glow"),
    ] {
        let c = control(bind);
        assert_eq!(c.component, "select", "{bind}");
        assert_eq!(
            c.props.get("rows_from"),
            Some(&Value::Text(family.into())),
            "{bind}"
        );
    }
    for (bind, gate) in [
        ("out_relief", None),
        ("out_rough", Some("out_rough_const")),
        ("out_metal", Some("out_metal_const")),
        ("out_ao", Some("out_ao_const")),
        ("out_emit_strength", None),
    ] {
        let c = control(bind);
        assert_eq!(c.component, "slider", "{bind}");
        assert_eq!(c.visible_bind.as_deref(), gate, "{bind}'s gate");
    }
}

/// THE OPTION ROWS: every select that repeats its options (`rows_from`) is authored as
/// its leading choices (values 0, 1, …) then ONE prototype option LAST, valued on from
/// them and labelled `<source>_opt{row}`; the frame tree — the walker's own repeater, no
/// bench filler — holds the leading choices then one option per row of the source,
/// valued on contiguously, and every label they bind is in the Model.
#[test]
fn every_option_select_repeats_its_row_source() {
    let mut bench = on_new();
    insert(&mut bench, "blend"); // a node with three input ports, selected
    let tree = authored(&bench);
    let frame = frame_tree(&mut bench);
    let model = bench.model();
    let mut seen = 0;
    for n in nodes_of(&tree) {
        if n.component != "select" {
            continue;
        }
        let Some(Value::Text(source)) = n.props.get("rows_from") else {
            continue;
        };
        seen += 1;
        let (proto, leading) = n.children.split_last().expect("a prototype option");
        let label = match source.as_str() {
            "materials" => "material",
            other => other,
        };
        for (i, o) in leading.iter().enumerate() {
            assert_eq!(
                o.props.get("value"),
                Some(&Value::Number(i as f64)),
                "{}",
                n.id
            );
        }
        assert_eq!(proto.component, "option", "{}", n.id);
        assert_eq!(
            proto.props.get("value"),
            Some(&Value::Number(leading.len() as f64)),
            "{}: the prototype is valued on from the leading choices",
            n.id
        );
        assert_eq!(
            proto.props.get("label_bind"),
            Some(&Value::Text(format!("{label}_opt{{row}}"))),
            "{}",
            n.id
        );
        let rows = bench.rows(source).expect("a row source the bench answers");
        let built = find(&frame, &n.id).expect("the frame keeps every select");
        assert_eq!(
            built.children.len(),
            leading.len() + rows.len(),
            "{}: {} leading + {} rows",
            n.id,
            leading.len(),
            rows.len()
        );
        for (j, opt) in built.children.iter().enumerate().skip(leading.len()) {
            let row = j - leading.len();
            assert_eq!(opt.component, "option");
            assert_eq!(opt.props.get("value"), Some(&Value::Number(j as f64)));
            let key = format!("{label}_opt{row}");
            assert_eq!(opt.props.get("label_bind"), Some(&Value::Text(key.clone())));
            assert!(
                model.text(&key).is_some(),
                "{}: {key} is not published",
                n.id
            );
        }
    }
    assert_eq!(seen, 29, "every data-driven select repeats a row source");
}

/// THE NODE LIST is data: one row per node, in the node list's order, each picking its
/// own node and labelled `key · Kind`; a new node is a new row, never new tree.
#[test]
fn the_node_list_is_one_row_per_node() {
    let mut bench = on_new();
    let rows = |bench: &mut MaterialBench| {
        let frame = frame_tree(bench);
        find(&frame, "mtl_node_list")
            .expect("the node list is authored")
            .children
            .clone()
    };
    let before = rows(&mut bench);
    assert_eq!(before.len(), bench.graph().nodes.len());
    for (i, row) in before.iter().enumerate() {
        assert_eq!(row.component, "button");
        assert_eq!(row.id, format!("nl_{i}"));
        assert_eq!(row.action.as_deref(), Some(format!("nl_pick_{i}").as_str()));
        assert_eq!(row.tab_group, "mtl_nodes");
    }
    insert(&mut bench, "blend");
    assert_eq!(rows(&mut bench).len(), before.len() + 1, "a node is a row");
    let model = bench.model();
    for (i, n) in bench.graph().nodes.iter().enumerate() {
        let label = model
            .text(&format!("nl{i}_label"))
            .expect("every row's label is published");
        assert!(label.starts_with(&n.key), "row {i} names its node: {label}");
    }
}

/// THE SWATCH STRIP: one swatch per palette entry, each a sprite over the 1×1 texture
/// slot the Model publishes for it (`glow<i>_tex`, after the theme's and the previews').
#[test]
fn the_swatch_strip_is_one_sprite_per_palette_colour() {
    let mut bench = MaterialBench::shipped();
    let frame = frame_tree(&mut bench);
    let strip = find(&frame, "mtl_glow_swatches").expect("the strip is authored");
    assert_eq!(strip.children.len(), bench.palette.len());
    let model = bench.model();
    for (i, sw) in strip.children.iter().enumerate() {
        let sprite = sw
            .children
            .iter()
            .find(|k| k.component == "sprite")
            .expect("a swatch is a sprite");
        assert_eq!(
            sprite.props.get("tex_bind"),
            Some(&Value::Text(format!("glow{i}_tex")))
        );
        assert_eq!(
            model.number(&format!("glow{i}_tex")),
            Some((SWATCH_TEX_BASE + i) as f64)
        );
    }
    assert_eq!(swatch_texel([1.0, 0.5, 0.0]), [255, 128, 0, 255]);
}

/// The materials list is data too: one row per defined material.
#[test]
fn the_materials_list_is_one_row_per_material() {
    let mut bench = MaterialBench::shipped();
    let frame = frame_tree(&mut bench);
    let list = find(&frame, "mtl_mat_list").expect("the materials list is authored");
    assert_eq!(list.children.len(), bench.materials.len());
    for (i, row) in list.children.iter().enumerate() {
        assert_eq!(row.component, "button");
        assert_eq!(row.id, format!("mat_pick_{i}"));
        assert_eq!(row.tab_group, "mtl_mat_panel");
    }
}

// ── the pair script's derive() ────────────────────────────────────────────────

/// The node-list wash follows `sel_node_index`: the selected row wears `node_row_sel`,
/// every other row rests.
#[test]
fn the_selected_node_row_wears_the_wash() {
    let mut bench = MaterialBench::shipped();
    let n = bench.graph().nodes.len();
    for pick in [n - 1, 0, n / 2] {
        fire(&mut bench, &format!("nl_pick_{pick}"), true);
        let model = bench.model();
        assert_eq!(model.number("sel_node_index"), Some(pick as f64));
        for i in 0..n {
            let sty = model
                .text(&format!("nl{i}_sty"))
                .expect("every row has a wash");
            let want = if i == pick {
                "materialbench.node_row_sel"
            } else {
                "materialbench.node_row"
            };
            assert_eq!(sty, want, "row {i} with {pick} picked");
        }
    }
}

/// The cell gates follow `sel_kind`: for every node kind, exactly ONE parameter cell is
/// shown and it is that kind's; with nothing selected, none is (and the hint is).
#[test]
fn exactly_one_parameter_cell_shows_and_it_is_the_selected_kind() {
    let mut bench = on_new();
    let gates = |model: &ValueMap| -> Vec<String> {
        NodeKind::catalog()
            .iter()
            .map(|k| format!("cell_{}_shown", k.id()))
            .filter(|g| matches!(model.get(g), Some(Value::Bool(true))))
            .collect()
    };
    for kind in NodeKind::catalog() {
        insert(&mut bench, kind.id());
        let model = bench.model();
        assert_eq!(model.text("sel_kind"), Some(kind.id()));
        assert_eq!(gates(&model), vec![format!("cell_{}_shown", kind.id())]);
        assert_eq!(model.get("no_node_shown"), Some(&Value::Bool(false)));
    }
    let out = output_card(&mut bench);
    press(&mut bench, Some(out), None);
    let model = bench.model();
    assert!(gates(&model).is_empty(), "the output card selects no node");
    assert_eq!(model.get("no_node_shown"), Some(&Value::Bool(true)));
}

/// A scalar port's constant slider shows only while no field drives it.
#[test]
fn the_constant_sliders_show_only_while_no_field_drives_the_port() {
    let mut bench = on_new();
    let model = bench.model();
    assert_eq!(model.get("out_rough_const"), Some(&Value::Bool(true)));
    assert_eq!(model.number("out_rough"), Some(0.75));
    // Drive roughness from the noise (option 1 + its place in the field family).
    assert!(fire(&mut bench, "out_rough_src", 1.0));
    let model = bench.model();
    assert_eq!(model.get("out_rough_const"), Some(&Value::Bool(false)));
    assert_eq!(
        model.number("out_rough"),
        None,
        "no constant while a field drives it"
    );
    assert!(fire(&mut bench, "out_rough_src", 0.0), "back to a constant");
    assert_eq!(
        bench.model().get("out_rough_const"),
        Some(&Value::Bool(true))
    );
}

/// FAIL-LOUD on the style channel the cross-scene gate cannot see: every dotted path the
/// pair script derives (row washes, swatch washes, page tabs) resolves to a block.
#[test]
fn every_style_path_the_pair_script_derives_resolves() {
    let mut bench = MaterialBench::shipped();
    let styles = styles(&bench);
    for page in ["page_bench", "page_materials"] {
        fire(&mut bench, page, true);
        let model = bench.model();
        let mut checked = 0;
        for (key, v) in model.entries() {
            let Value::Text(path) = v else { continue };
            if !key.ends_with("_sty") {
                continue;
            }
            let resolved = path.split('.').try_fold(&styles, |v, k| v.get(k));
            assert!(
                resolved.is_some_and(|v| v.is_object()),
                "{key} = `{path}` resolves to nothing"
            );
            checked += 1;
        }
        assert!(checked > 10, "the washes are derived ({checked})");
    }
}

/// The selected swatch wears the wash — the one the glow wears.
#[test]
fn the_selected_swatch_wears_the_wash() {
    let mut bench = on_new();
    assert!(
        fire(&mut bench, "out_emit_color", 3.0),
        "pick palette entry 2"
    );
    let model = bench.model();
    assert_eq!(model.number("out_emit_color"), Some(3.0));
    for i in 0..bench.palette.len() {
        let want = if i == 2 {
            "materialbench.swatch_sel"
        } else {
            "materialbench.swatch"
        };
        assert_eq!(
            model.text(&format!("glow{i}_sty")),
            Some(want),
            "swatch {i}"
        );
    }
}

/// The page switch shows exactly one page and is a VIEW change.
#[test]
fn the_page_switch_shows_exactly_one_page() {
    let mut bench = MaterialBench::shipped();
    let m = bench.model();
    assert_eq!(m.get("page_bench_shown"), Some(&Value::Bool(true)));
    assert_eq!(m.get("page_materials_shown"), Some(&Value::Bool(false)));
    assert!(!fire(&mut bench, "page_materials", true), "a view change");
    let m = bench.model();
    assert_eq!(m.get("page_bench_shown"), Some(&Value::Bool(false)));
    assert_eq!(m.get("page_materials_shown"), Some(&Value::Bool(true)));
    fire(&mut bench, "page_bench", true);
    assert_eq!(bench.model().number("sel_page"), Some(0.0));
}

// ── the dispatcher's contract ─────────────────────────────────────────────────

/// AN ECHO IS NEVER AN EDIT. The walker echoes every visible control's value every
/// frame, so feeding the WHOLE published Model back as results must change nothing and
/// ask for no bake — for every preset, with every kind of node selected.
#[test]
fn an_echo_is_never_an_edit() {
    let mut bench = MaterialBench::shipped();
    for p in 0..bench.presets.len() {
        fire(&mut bench, "sel_preset", p as f64);
        let before = bench.graph().clone();
        for i in 0..bench.graph().nodes.len() {
            fire(&mut bench, &format!("nl_pick_{i}"), true);
            let echo = bench.model();
            assert!(
                !route::apply(&mut bench, &echo, &echo),
                "preset {p}, node {i}: the echo re-baked"
            );
            // …and what the WALKER echoes for every control it lays out.
            let frame = walk(&mut bench);
            assert!(
                !route::apply(&mut bench, &echo, &frame.results),
                "preset {p}, node {i}: the walker's echo re-baked"
            );
            assert_eq!(
                bench.graph(),
                &before,
                "preset {p}, node {i}: the echo edited"
            );
        }
    }
    // A fresh node of every kind too.
    let mut bench = on_new();
    for kind in NodeKind::catalog() {
        insert(&mut bench, kind.id());
        let before = bench.graph().clone();
        let echo = bench.model();
        assert!(
            !route::apply(&mut bench, &echo, &echo),
            "{}: echo",
            kind.id()
        );
        let frame = walk(&mut bench);
        assert!(
            !route::apply(&mut bench, &echo, &frame.results),
            "{}: the walker's echo",
            kind.id()
        );
        assert_eq!(bench.graph(), &before);
    }
}

/// The frame order: a pick that moves the selection in the same frame the old node's
/// dials echo NEVER writes the old node's values into the new one.
#[test]
fn a_selection_change_never_writes_the_old_nodes_dials_into_the_new_one() {
    let mut bench = on_new();
    let a = insert(&mut bench, "levels");
    fire(&mut bench, "node_gamma", 3.0);
    let b = insert(&mut bench, "levels");
    select(&mut bench, &a);
    // The frame: the Model describes `a` (gamma 3); the user picks `b`'s row.
    let published = bench.model();
    let mut results = published.clone();
    let bi = bench.designer.index_of(&b).unwrap();
    results.set(format!("nl_pick_{bi}"), true);
    route::apply(&mut bench, &published, &results);
    assert_eq!(bench.designer.selected_key(), Some(b.as_str()));
    let gamma = |bench: &MaterialBench, key: &str| match bench.graph().nodes
        [bench.designer.index_of(key).unwrap()]
    .kind
    {
        NodeKind::Levels { gamma, .. } => gamma,
        _ => unreachable!(),
    };
    assert_eq!(gamma(&bench, &a), 3.0);
    assert_eq!(gamma(&bench, &b), 1.0, "b kept its own gamma");
}

/// Only document edits re-bake; a view change, a pick, a binding never do.
#[test]
fn only_graph_edits_ask_for_a_bake() {
    let mut bench = on_new();
    assert!(
        fire(&mut bench, "out_relief", 2.0),
        "an output knob is an edit"
    );
    assert!(fire(&mut bench, "reseed", true), "reseeding is an edit");
    assert!(fire(&mut bench, "node_scale", 9.0), "a dial is an edit");
    assert!(
        !fire(&mut bench, "sel_map", 1.0),
        "switching maps is a view change"
    );
    assert!(
        !fire(&mut bench, "nl_pick_1", true),
        "picking a node is a view change"
    );
    assert!(
        !fire(&mut bench, "canvas_mode", 1.0),
        "the canvas tool is a view change"
    );
    assert!(
        !fire(&mut bench, "sel_material", 1.0),
        "binding is not an image edit"
    );
    assert!(
        !fire(&mut bench, "bake_size", true),
        "the rung is not an image edit"
    );
}

/// The bound view number clamps into the ring.
#[test]
fn the_view_number_clamps_into_the_ring() {
    let mut bench = MaterialBench::shipped();
    fire(&mut bench, "sel_map", 900.0);
    assert!(bench.showing_lit());
    fire(&mut bench, "sel_map", -3.0);
    assert_eq!(bench.selected_map(), MapKind::ALL[0]);
}

/// INSERT: the popup's answer inserts a fresh node of that kind after the selected one,
/// under a new key, selected — for every kind in the menu. Noise nodes take fresh salts.
#[test]
fn insert_adds_a_selected_node_of_every_kind() {
    let mut bench = on_new();
    for kind in NodeKind::catalog() {
        let before = bench.graph().nodes.len();
        let at = bench.designer.selected().map(|(i, _)| i + 1).unwrap();
        let key = insert(&mut bench, kind.id());
        assert_eq!(bench.graph().nodes.len(), before + 1);
        assert_eq!(
            bench.designer.index_of(&key),
            Some(at),
            "inserted after the selection"
        );
        assert_eq!(bench.graph().nodes[at].kind.id(), kind.id());
        consistent(&bench);
    }
    let salts: Vec<u64> = bench
        .graph()
        .nodes
        .iter()
        .filter_map(|n| match n.kind {
            NodeKind::Noise { salt, .. } => Some(salt),
            _ => None,
        })
        .collect();
    let mut unique = salts.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        salts.len(),
        "every noise node has its own salt"
    );
}

/// The Insert BUTTON asks for the shared popup menu; its pick comes back through
/// `modal_closed` to the ONE dispatcher; a cancel inserts nothing.
#[test]
fn the_insert_button_opens_the_menu_and_its_pick_inserts() {
    let mut bench = on_new();
    assert!(
        !fire(&mut bench, "node_insert", true),
        "asking is not an edit"
    );
    assert!(bench.ask_insert, "the menu is asked for");
    let before = bench.graph().nodes.len();
    bench.modal_closed("popup_menu", "insert_cancel", None);
    assert_eq!(bench.graph().nodes.len(), before, "cancel inserts nothing");
    bench.modal_closed("popup_menu", "insert_warp", None);
    assert_eq!(bench.graph().nodes.len(), before + 1);
    assert_eq!(
        bench.designer.selected().map(|(_, n)| n.kind.id()),
        Some("warp")
    );
}

/// DELETE takes the node and every wire it fed; an output port it drove rests.
#[test]
fn delete_removes_the_node_and_every_wire_it_fed() {
    let mut bench = on_new();
    select(&mut bench, "noise");
    assert!(fire(&mut bench, "node_delete", true));
    let g = bench.graph();
    assert!(bench.designer.index_of("noise").is_none());
    assert_eq!(g.output.height, None, "the height it drove is unwired");
    let ramp = &g.nodes[bench.designer.index_of("ramp").unwrap()];
    assert!(
        ramp.inputs.iter().all(Option::is_none),
        "the ramp lost its input"
    );
    assert_eq!(
        bench.designer.selected_key(),
        Some("ramp"),
        "the selection moves to the neighbour"
    );
    consistent(&bench);
    assert!(
        bench
            .designer
            .problems()
            .iter()
            .any(|p| p.to_string().contains("ramp.field")),
        "the ramp's missing input is listed"
    );
}

/// WIRE / UNWIRE through a node's source selects keep the problems list in step.
#[test]
fn the_source_selects_wire_and_unwire() {
    let mut bench = on_new();
    let key = insert(&mut bench, "levels");
    assert!(
        bench
            .designer
            .problems()
            .iter()
            .any(|p| p.to_string().contains(&format!("{key}.field"))),
        "a fresh filter's input is a listed problem"
    );
    let sink = Sink::Input {
        node: key.clone(),
        port: 0,
    };
    let noise = bench
        .designer
        .candidates(&sink)
        .iter()
        .position(|k| *k == "noise")
        .expect("the noise is on offer");
    assert!(fire(&mut bench, "node_in0", (noise + 1) as f64), "wired");
    assert_eq!(
        bench.designer.source_of(&Sink::Input {
            node: key.clone(),
            port: 0
        }),
        Some("noise")
    );
    assert!(bench.designer.problems().is_empty(), "nothing is wrong now");
    consistent(&bench);
    assert_eq!(
        bench.model().number("node_in0"),
        Some((noise + 1) as f64),
        "the select echoes the source"
    );
    assert!(fire(&mut bench, "node_in0", 0.0), "unwired");
    assert!(!bench.designer.problems().is_empty());
    consistent(&bench);
}

/// MAJOR 3: a SOURCE select offers only what the graph would ACCEPT — never the node
/// itself, never anything it feeds — so a loop cannot be picked at all; the options the
/// frame tree holds and the labels the Model publishes are exactly those candidates. A
/// pick past them is declined (no edit). The canvas CAN drag a loop: refused with its
/// notice, the graph untouched, and the next edit answers the notice.
#[test]
fn a_wire_that_would_loop_is_never_offered_and_refused_on_the_canvas() {
    let mut bench = on_new();
    let a = insert(&mut bench, "levels");
    assert!(
        fire(&mut bench, "node_in0", 1.0),
        "a ← noise (its only candidate)"
    );
    let b = insert(&mut bench, "levels");
    let offered_b = bench.designer.candidates(&Sink::Input {
        node: b.clone(),
        port: 0,
    });
    let ai = offered_b.iter().position(|k| *k == a).expect("a feeds b");
    assert!(fire(&mut bench, "node_in0", (ai + 1) as f64), "b ← a");
    select(&mut bench, &a);
    let offered: Vec<String> = bench
        .designer
        .candidates(&Sink::Input {
            node: a.clone(),
            port: 0,
        })
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        !offered.contains(&a),
        "a node is never offered its own output"
    );
    assert!(!offered.contains(&b), "nor anything it feeds");
    assert_eq!(offered, vec!["noise".to_string()]);
    let frame = frame_tree(&mut bench);
    let sel = find(&frame, "levels_in0").expect("the levels cell's input select");
    assert_eq!(
        sel.children.len(),
        1 + offered.len(),
        "None + the candidates"
    );
    let model = bench.model();
    assert!(model.text("in0_opt0").unwrap().starts_with("noise"));
    assert_eq!(model.text("in0_opt1"), None, "nothing else is labelled");
    let before = bench.graph().clone();
    assert!(
        !fire(&mut bench, "node_in0", 2.0),
        "a pick past the offer is declined"
    );
    assert_eq!(bench.graph(), &before);
    let (from, to) = (card(&mut bench, &b), card(&mut bench, &a));
    assert!(!link(&mut bench, from, to, Some(0)), "b → a would loop");
    assert_eq!(bench.graph(), &before, "a refused wire edits nothing");
    assert_eq!(bench.status_line().0, designer::WIRE_LOOPS);
    fire(&mut bench, "out_relief", 1.25);
    assert_ne!(
        bench.status_line().0,
        designer::WIRE_LOOPS,
        "the next edit answers it"
    );
}

/// SET_PARAM writes the dial's value through the node's own form: integers round, a
/// non-finite value is refused, and re-writing the value held is no edit.
#[test]
fn set_param_rounds_integers_refuses_non_finite_and_ignores_echoes() {
    let mut bench = on_new();
    select(&mut bench, "noise");
    assert!(fire(&mut bench, "node_scale", 7.6));
    let scale = |bench: &MaterialBench| match bench.graph().nodes[0].kind {
        NodeKind::Noise { scale, .. } => scale,
        _ => unreachable!(),
    };
    assert_eq!(
        scale(&bench),
        8,
        "a lattice period is a whole number of cells"
    );
    assert!(
        !fire(&mut bench, "node_scale", 8.0),
        "the value held is no edit"
    );
    assert!(
        !fire(&mut bench, "node_lacunarity", f64::NAN),
        "NaN is refused"
    );
    assert!(
        !fire(&mut bench, "node_gain", f64::INFINITY),
        "inf is refused"
    );
    assert!(fire(&mut bench, "node_invert", true));
    assert!(
        fire(&mut bench, "node_source", 4.0),
        "an enum takes its index"
    );
    assert!(matches!(
        bench.graph().nodes[0].kind,
        NodeKind::Noise {
            source: flicker_texture::NoiseKind::Worley,
            invert: true,
            ..
        }
    ));
    assert!(
        !fire(&mut bench, "node_source", 99.0),
        "past the enum is refused"
    );
    consistent(&bench);
}

/// Every enum select's option index lands on the variant it names (both directions).
#[test]
fn every_enum_index_round_trips_through_the_node() {
    let mut bench = on_new();
    for (kind, param) in [
        ("noise", "source"),
        ("blend", "mode"),
        ("gradient", "axis"),
        ("shape", "pattern"),
    ] {
        insert(&mut bench, kind);
        let ids = designer::enum_ids(param).unwrap();
        for i in (0..ids.len()).rev() {
            fire(&mut bench, &format!("node_{param}"), i as f64);
            assert_eq!(
                bench.model().number(&format!("node_{param}")),
                Some(i as f64),
                "{kind}.{param} option {i}"
            );
            let (_, n) = bench.designer.selected().unwrap();
            let doc = serde_json::to_value(&n.kind).unwrap();
            assert_eq!(
                doc[param], ids[i],
                "{kind}.{param} option {i} is `{}`",
                ids[i]
            );
        }
    }
    for a in designer::AXES {
        assert_eq!(serde_json::to_value(a).unwrap(), designer::axis_id(a));
    }
    for p in designer::PATTERNS {
        assert_eq!(serde_json::to_value(p).unwrap(), designer::pattern_id(p));
    }
}

/// OUTPUT_SET: the relief, the scalar ports (source and constant), the base and height
/// sources — each an edit through the one mutation set, each keeping the graph sound.
#[test]
fn the_output_stage_controls_edit_the_output() {
    let mut bench = on_new();
    assert!(fire(&mut bench, "out_relief", 1.5));
    assert_eq!(bench.graph().output.relief, 1.5);
    assert!(fire(&mut bench, "out_metal", 0.4));
    assert_eq!(bench.graph().output.metalness, Scalar::Const(0.4));
    assert!(
        fire(&mut bench, "out_ao_src", 1.0),
        "drive AO from the noise"
    );
    assert_eq!(bench.graph().output.ao, Scalar::Field("noise".into()));
    assert!(fire(&mut bench, "out_ao_src", 0.0), "back to the constant");
    assert!(matches!(bench.graph().output.ao, Scalar::Const(_)));
    assert!(fire(&mut bench, "out_height", 0.0), "unwire the height");
    assert_eq!(bench.graph().output.height, None);
    assert!(fire(&mut bench, "out_base", 0.0), "unwire the base colour");
    assert_eq!(bench.graph().output.base, None);
    consistent(&bench);
    // An unwired base bakes the neutral grey and an unwired height a flat surface —
    // visible, never black, never an error.
    flicker_texture::bake_graph(bench.graph(), 16).expect("an unwired output still bakes");
}

/// THE GLOW: a palette pick writes the EXACT palette colour into the palette node the
/// glow takes (creating it, wired, when there is none) and re-bakes once; option 0 turns
/// the glow off; the mask and strength edit only while there is a glow.
#[test]
fn a_glow_pick_writes_the_exact_palette_colour_into_a_palette_node() {
    let mut bench = on_new();
    assert!(bench.graph().output.emit.is_none(), "New has no glow");
    assert!(
        !fire(&mut bench, "out_emit_strength", 0.5),
        "no glow, no strength"
    );
    let want = bench.palette.get(4).unwrap().rgb;
    assert!(
        fire(&mut bench, "out_emit_color", 5.0),
        "a pick is an image edit"
    );
    let emit = bench.graph().output.emit.clone().expect("the glow exists");
    let node = &bench.graph().nodes[bench.designer.index_of(&emit.color).unwrap()];
    assert_eq!(
        node.kind,
        NodeKind::Palette { rgb: want },
        "the exact colour"
    );
    assert!(
        !fire(&mut bench, "out_emit_color", 5.0),
        "the same pick is no edit"
    );
    let nodes = bench.graph().nodes.len();
    assert!(fire(&mut bench, "out_emit_color", 1.0), "a second colour");
    assert_eq!(
        bench.graph().nodes.len(),
        nodes,
        "the palette node is rewritten, not doubled"
    );
    assert!(fire(&mut bench, "out_emit_strength", 0.8));
    assert!(fire(&mut bench, "out_emit_mask", 1.0), "mask by the noise");
    assert_eq!(
        bench.graph().output.emit.as_ref().unwrap().mask.as_deref(),
        Some("noise")
    );
    consistent(&bench);
    assert!(
        fire(&mut bench, "out_emit_color", 0.0),
        "option 0 turns it off"
    );
    assert!(bench.graph().output.emit.is_none());
    consistent(&bench);
}

/// The model echoes the glow's palette option and publishes each swatch's name.
#[test]
fn the_model_echoes_the_glow_palette_option() {
    load_strings();
    let mut bench = on_new();
    assert_eq!(bench.model().number("out_emit_color"), Some(0.0), "no glow");
    fire(&mut bench, "out_emit_color", 6.0);
    assert_eq!(bench.model().number("out_emit_color"), Some(6.0));
    assert_eq!(bench.model().number("glow_count"), Some(8.0));
    // A glow off the palette shows the placeholder (-1), never a wrong entry.
    let key = bench.graph().output.emit.as_ref().unwrap().color.clone();
    assert!(bench.designer.set_palette_rgb(&key, [0.3, 0.2, 0.1]));
    assert_eq!(bench.model().number("out_emit_color"), Some(-1.0));
    assert!(
        !fire(&mut bench, "out_emit_color", -1.0),
        "the placeholder is never a pick"
    );
    assert_eq!(bench.model().text("glow_opt0"), Some("Red"));
}

/// UP / DOWN move the selected node in the list (its authoring order); never past the
/// ends.
#[test]
fn up_and_down_move_the_selected_node() {
    let mut bench = on_new();
    select(&mut bench, "ramp");
    assert!(fire(&mut bench, "node_up", true));
    assert_eq!(bench.designer.index_of("ramp"), Some(0));
    assert!(!fire(&mut bench, "node_up", true), "not past the top");
    assert!(fire(&mut bench, "node_down", true));
    assert!(!fire(&mut bench, "node_down", true), "not past the bottom");
    assert_eq!(bench.designer.index_of("ramp"), Some(1));
    consistent(&bench);
}

/// Presets: the library is New + every graph of the factory library; the select and the
/// page bumpers walk it, wrapping both ways.
#[test]
fn the_preset_ring_walks_the_library_both_ways() {
    let n = factory_graphs().len() + 1;
    let mut bench = MaterialBench::shipped();
    assert_eq!(bench.presets.len(), n);
    assert_eq!(
        bench.preset,
        Some(1),
        "opens on the factory library's first entry"
    );
    assert_eq!(bench.graph().id, factory_graphs()[0].id);
    for _ in 0..n {
        fire(&mut bench, "patch_next", true);
    }
    assert_eq!(bench.preset, Some(1), "next wraps around the library");
    fire(&mut bench, "patch_prev", true);
    fire(&mut bench, "patch_prev", true);
    assert_eq!(bench.preset, Some(n - 1), "prev wraps back");
    assert!(fire(&mut bench, "sel_preset", 0.0));
    assert_eq!(bench.graph(), &bench.presets[0], "the select loads New");
}

/// EVERY PRESET IS AUTHORED, SOUND, AND BAKES — New is the scene's AUTHORED default (not a
/// fallback), and the rest IS the factory library, loaded VERBATIM and in file order (no
/// conversion, no snap). Each factory entry is a v2 graph bound to a slot whose index row it
/// is named as, a slot the ONE slot law accepts for that row's class, no id or slot authored
/// twice, and a look that GLOWS wherever its row is emissive (what Commit would otherwise
/// warn about). Every preset has no problems and bakes.
#[test]
fn every_preset_is_authored_sound_and_bakes() {
    use flicker_materials::{slot_accepts, RenderClass};
    let bench = MaterialBench::shipped();
    let doc: serde_json::Value = serde_json::from_str(SW_SCENE).unwrap();
    let authored: flicker_texture::MaterialGraph =
        serde_json::from_value(doc["params"][NEW_MATERIAL_PARAM].clone())
            .expect("the New default is a v2 graph document");
    assert_eq!(
        bench.presets[0], authored,
        "New is the AUTHORED default, not a fallback"
    );
    assert_eq!(authored.version, flicker_texture::graph::GRAPH_VERSION);
    for (i, graph) in bench.presets.iter().enumerate() {
        assert!(
            flicker_texture::problems(graph).is_empty(),
            "preset {i} ({}): {:?}",
            graph.id,
            flicker_texture::problems(graph)
        );
        flicker_texture::bake_graph(graph, 32)
            .unwrap_or_else(|e| panic!("preset {i} ({}) bakes: {e}", graph.id));
    }
    let factory = factory_graphs();
    assert!(!factory.is_empty(), "the factory library ships entries");
    assert_eq!(
        bench.presets[1..],
        factory[..],
        "the library is the factory file, verbatim and in file order"
    );
    let mut ids = std::collections::HashSet::new();
    let mut slots = std::collections::HashSet::new();
    for graph in &factory {
        assert_eq!(
            graph.version,
            flicker_texture::graph::GRAPH_VERSION,
            "{}: a v2 document",
            graph.id
        );
        let slot = graph
            .material
            .unwrap_or_else(|| panic!("{}: bound to a slot", graph.id));
        let row = bench
            .materials
            .iter()
            .find(|d| d.id == slot)
            .unwrap_or_else(|| panic!("{}: the index holds a row at {slot}", graph.id));
        assert_eq!(graph.name, row.name, "slot {slot}: the graph names its row");
        assert!(
            slot_accepts(slot, bench.class_of(Some(slot))),
            "slot {slot}: the slot law accepts its row's class"
        );
        assert!(ids.insert(graph.id.as_str()), "{}: an id twice", graph.id);
        assert!(slots.insert(slot), "slot {slot}: bound twice");
        if row.render_class == Some(RenderClass::Emissive) {
            assert!(graph.glows(), "{}: an emissive row's look glows", graph.id);
        }
    }
}

/// A BROKEN LIBRARY IS LOUD AND NEVER LEAVES THE BENCH WITHOUT A DOCUMENT: a missing
/// factory file, or ONE entry in it that is no material document, empties the library (all
/// or nothing — skipping the bad entry would hide it) and the select offers New alone, which
/// the bench then opens on (`1.min(len - 1)` is 0). A broken New opens the library's first
/// entry; with no library either, a blank graph that has no problems and bakes.
#[test]
fn a_broken_factory_library_leaves_new_alone() {
    let def = flicker::ui::SceneDef::parse("materialbench", crate::SW_SCENE).unwrap();
    let shipped = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/factory_materials.json"
    ))
    .expect("factory_materials.json reads");
    let data = staging_scratch("factory_data");
    std::fs::create_dir_all(&data).unwrap();
    let file = data.join(FACTORY_MATERIALS_FILE);
    let opens = |presets: &[MaterialGraph]| 1.min(presets.len() - 1);

    let lib = preset_library(&def.params, &data);
    assert_eq!(lib.len(), 1, "a missing library leaves New alone");
    assert_eq!(opens(&lib), 0, "and the bench opens on New");

    let mut doc: serde_json::Value = serde_json::from_str(&shipped).unwrap();
    doc["graphs"][1]["note"] = "a note inside an entry".into();
    std::fs::write(&file, doc.to_string()).unwrap();
    let lib = preset_library(&def.params, &data);
    assert_eq!(lib.len(), 1, "one malformed entry empties the library");

    std::fs::write(&file, &shipped).unwrap();
    let lib = preset_library(&def.params, &data);
    assert_eq!(lib[1..], factory_graphs()[..], "the file, read whole");
    assert_eq!(opens(&lib), 1, "the bench opens on its first entry");

    let mut broken = def.params.clone();
    broken.insert(NEW_MATERIAL_PARAM.into(), serde_json::json!({ "nodes": 3 }));
    let lib = preset_library(&broken, &data);
    assert_eq!(lib[0], lib[1], "a broken New opens the first factory entry");

    std::fs::remove_file(&file).unwrap();
    let lib = preset_library(&broken, &data);
    assert_eq!(lib.len(), 1, "nothing authored loaded");
    let blank = &lib[0];
    assert!(
        blank.nodes.is_empty() && blank.material.is_none(),
        "{blank:?}"
    );
    assert!(flicker_texture::problems(blank).is_empty(), "{blank:?}");
    flicker_texture::bake_graph(blank, 16).expect("the blank graph bakes");
}

/// ROLL snaps the glow onto the palette (B46FFC37) and is reproducible from the seed.
#[test]
fn roll_snaps_the_glow_onto_the_palette() {
    let mut bench = MaterialBench::shipped();
    let set: Vec<[f32; 3]> = bench.palette.iter().map(|c| c.rgb).collect();
    for _ in 0..8 {
        assert!(fire(&mut bench, "roll", true));
        assert_eq!(bench.preset, None, "a roll is no preset");
        let emit = bench
            .graph()
            .output
            .emit
            .clone()
            .expect("a roll glows (or could)");
        let node = &bench.graph().nodes[bench.designer.index_of(&emit.color).unwrap()];
        let NodeKind::Palette { rgb } = node.kind else {
            panic!("a rolled glow is a palette node")
        };
        assert!(
            set.contains(&rgb),
            "a rolled glow {rgb:?} is off the palette"
        );
        assert!(bench.designer.problems().is_empty());
        assert_eq!(bench.model().number("sel_preset"), Some(-1.0));
    }
}

/// Loading a preset keeps its authored graph VERBATIM — every factory entry picked in the
/// select IS the document, nothing snapped, converted or re-bound on load (Roll is the load
/// that snaps — `roll_snaps_the_glow_onto_the_palette`); an entry that glows still holds its
/// authored rgb, exactly, in the Palette node its emit's colour names. From New, so every
/// entry — the one the bench opens on included — is a real pick, never an echo.
#[test]
fn loading_presets_keeps_their_authored_graph_verbatim() {
    let mut bench = on_new();
    for (authored, i) in factory_graphs().iter().zip(1..) {
        assert!(
            fire(&mut bench, "sel_preset", i as f64),
            "{}: the pick loads",
            authored.id
        );
        assert_eq!(bench.preset, Some(i), "{}", authored.id);
        assert_eq!(bench.graph(), authored, "{}: loaded verbatim", authored.id);
        let Some(emit) = &authored.output.emit else {
            continue;
        };
        let rgb = |g: &MaterialGraph| match g.nodes.iter().find(|n| n.key == emit.color) {
            Some(Node {
                kind: NodeKind::Palette { rgb },
                ..
            }) => Some(*rgb),
            _ => None,
        };
        let want =
            rgb(authored).unwrap_or_else(|| panic!("{}: the glow is a Palette node", authored.id));
        assert_eq!(
            rgb(bench.graph()),
            Some(want),
            "{}: the authored glow colour, exactly",
            authored.id
        );
    }
}

/// RESEED steps the graph seed and keeps every node's salt — the same instrument giving
/// a different performance.
#[test]
fn reseed_steps_the_seed_and_keeps_every_salt() {
    let mut bench = MaterialBench::shipped();
    let before = bench.graph().clone();
    assert!(fire(&mut bench, "reseed", true));
    assert_ne!(bench.graph().seed, before.seed);
    assert_eq!(
        bench.graph().nodes,
        before.nodes,
        "every node (and salt) kept"
    );
    assert_ne!(
        flicker_texture::bake_graph(bench.graph(), 16).unwrap(),
        flicker_texture::bake_graph(&before, 16).unwrap(),
        "a new seed is a new performance"
    );
}

/// The ramp node's tint knobs recolour its stops and the base map goes gold.
#[test]
fn the_ramp_tint_recolours_the_base_map_gold() {
    let mut bench = on_new();
    select(&mut bench, "ramp");
    assert!(fire(&mut bench, "node_sat", 0.8), "saturation is an edit");
    assert!(fire(&mut bench, "node_hue", 0.13), "hue is an edit");
    assert!((bench.model().number("node_hue").unwrap() - 0.13).abs() < 0.02);
    let set = flicker_texture::bake_graph(bench.graph(), 32).unwrap();
    let base = set.get(MapKind::BaseColor).unwrap();
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for px in base.pixels.chunks(4) {
        r += px[0] as u64;
        g += px[1] as u64;
        b += px[2] as u64;
    }
    assert!(
        r > b && g > b,
        "gold is red+green over blue (r{r} g{g} b{b})"
    );
    let tint = bench.model();
    assert!(
        !fire(&mut bench, "node_hue", tint.number("node_hue").unwrap()),
        "the tint it wears is no edit"
    );
}

// ── the graph canvas ──────────────────────────────────────────────────────────

/// The canvas content IS the document: one card per node in EVALUATION order
/// (dependencies first) plus the output card last, an edge per resolving wire.
#[test]
fn the_canvas_cards_are_the_nodes_in_evaluation_order_then_the_output() {
    let mut bench = on_new();
    select(&mut bench, "ramp");
    fire(&mut bench, "node_up", true); // the list says ramp, noise — the wires say noise first
    bench.sync_canvas();
    let c = &bench.canvas_content;
    assert_eq!(c.keys, vec!["noise", "ramp", designer::OUTPUT_CARD]);
    assert_eq!(c.ports, vec![0, 1, OutPort::ALL.len() as u8]);
    assert_eq!(c.cards.last(), Some(&Card::Output));
    let wired: Vec<(usize, usize, Sink)> = c
        .edges
        .iter()
        .zip(&c.sinks)
        .map(|(e, s)| (e.from, e.to, Sink::from_shared(s).expect("a designer sink")))
        .collect();
    assert!(wired.contains(&(
        0,
        1,
        Sink::Input {
            node: "ramp".into(),
            port: 0
        }
    )));
    assert!(wired.contains(&(1, 2, Sink::Output(OutPort::Base))));
    assert!(wired.contains(&(0, 2, Sink::Output(OutPort::Height))));
    assert_eq!(wired.len(), 3);
}

/// THE CANVAS MAPPING: a Link between COMPATIBLE ports wires — onto the port the
/// release landed on, or (released on a card's body) its first free port of that kind.
#[test]
fn a_link_between_compatible_ports_wires() {
    let mut bench = on_new();
    let levels = insert(&mut bench, "levels");
    let (from, to) = (card(&mut bench, "noise"), card(&mut bench, &levels));
    assert!(
        link(&mut bench, from, to, Some(0)),
        "noise → levels.field wires"
    );
    assert_eq!(
        bench.designer.source_of(&Sink::Input {
            node: levels.clone(),
            port: 0
        }),
        Some("noise")
    );
    consistent(&bench);
    // Released on a blend's BODY: its first free field port (a), then the next (b).
    let blend = insert(&mut bench, "blend");
    let (n, l, b) = (
        card(&mut bench, "noise"),
        card(&mut bench, &levels),
        card(&mut bench, &blend),
    );
    assert!(link(&mut bench, n, b, None));
    assert!(link(&mut bench, l, b, None));
    let src = |bench: &MaterialBench, port| {
        bench
            .designer
            .source_of(&Sink::Input {
                node: blend.clone(),
                port,
            })
            .map(str::to_string)
    };
    assert_eq!(src(&bench, 0).as_deref(), Some("noise"));
    assert_eq!(src(&bench, 1).as_deref(), Some(levels.as_str()));
    // Onto the OUTPUT card's height port.
    let out = output_card(&mut bench);
    let l = card(&mut bench, &levels);
    assert!(link(&mut bench, l, out, Some(1)));
    assert_eq!(
        bench.graph().output.height.as_deref(),
        Some(levels.as_str())
    );
    consistent(&bench);
}

/// …and between INCOMPATIBLE ports it is REFUSED, the graph untouched and the status
/// line saying why. So is a link out of the output, onto a card with no ports, and one
/// that would loop.
#[test]
fn an_incompatible_link_is_refused_with_a_status_message() {
    let mut bench = on_new();
    let levels = insert(&mut bench, "levels");
    let before = bench.graph().clone();
    let (ramp, to) = (card(&mut bench, "ramp"), card(&mut bench, &levels));
    assert!(
        !link(&mut bench, ramp, to, Some(0)),
        "a colour into a field port"
    );
    assert_eq!(bench.graph(), &before);
    assert_eq!(bench.status_line().0, designer::WIRE_MISTYPED);
    assert_eq!(bench.model().get("status_shown"), Some(&Value::Bool(true)));

    let (out, l) = (output_card(&mut bench), card(&mut bench, &levels));
    assert!(!link(&mut bench, out, l, Some(0)));
    assert_eq!(bench.status_line().0, designer::WIRE_FROM_OUTPUT);

    let (l, noise) = (card(&mut bench, &levels), card(&mut bench, "noise"));
    assert!(!link(&mut bench, l, noise, None), "noise takes no input");
    assert_eq!(bench.status_line().0, designer::WIRE_NO_PORT);

    // levels ← noise, then noise… has no port; levels → levels is a click, not a link.
    let l = card(&mut bench, &levels);
    assert!(
        !link(&mut bench, l, l, Some(0)),
        "a press and release on one card"
    );
    assert_eq!(bench.graph(), &before, "nothing was wired by any of it");

    // The glow mask before there is a glow.
    let (n, out) = (card(&mut bench, "noise"), output_card(&mut bench));
    assert!(!link(&mut bench, n, out, Some(6)));
    assert_eq!(bench.status_line().0, designer::WIRE_NO_GLOW);
}

/// A card pick selects its node; an EDGE pick selects that wire, and Delete then UNWIRES
/// it (rather than deleting a node).
#[test]
fn a_card_pick_selects_and_an_edge_pick_then_delete_unwires() {
    let mut bench = on_new();
    let ramp = card(&mut bench, "ramp");
    press(&mut bench, Some(ramp), None);
    assert_eq!(bench.designer.selected_key(), Some("ramp"));
    bench.sync_canvas();
    let edge = bench
        .canvas_content
        .sinks
        .iter()
        .position(|s| *s == Sink::Output(OutPort::Height))
        .expect("the height wire is drawn");
    press(&mut bench, None, Some(edge));
    assert_eq!(
        bench.designer.selected_edge(),
        Some(&Sink::Output(OutPort::Height))
    );
    let nodes = bench.graph().nodes.len();
    assert!(fire(&mut bench, "node_delete", true));
    assert_eq!(bench.graph().output.height, None, "the wire is gone");
    assert_eq!(bench.graph().nodes.len(), nodes, "no node was deleted");
    assert_eq!(bench.designer.selected_edge(), None);
    consistent(&bench);
}

/// The canvas INK is the scene's: every `CanvasStyle` slot resolves from this scene's
/// `materialbench.canvas` block (a `$token` each) — none is left unset, none is Rust's.
#[test]
fn every_canvas_ink_resolves_from_the_scene() {
    let bench = MaterialBench::shipped();
    let style = canvas_style(&styles(&bench), CANVAS_INK);
    for (slot, ink) in [
        ("bg", style.bg),
        ("edge", style.edge),
        ("edge_lit", style.edge_lit),
        ("card_fill_top", style.card_fill_top),
        ("card_fill_bot", style.card_fill_bot),
        ("card_border", style.card_border),
        ("card_border_selected", style.card_border_selected),
        ("label", style.label),
        ("label_selected", style.label_selected),
        ("meta", style.meta),
        ("icon_top", style.icon_top),
        ("icon_bot", style.icon_bot),
        ("icon_border", style.icon_border),
        ("port", style.port),
        ("link", style.link),
    ] {
        assert!(ink[3] > 0.0, "canvas ink `{slot}` is unset: {ink:?}");
    }
}

/// THE CANVAS ON ITS SEAT: driven and drawn on the rect the walker reserved, it paints
/// one card per node plus the output card, every command inside a scissor on that seat
/// (a panned card must never paint over the panels around it).
#[test]
fn the_canvas_draws_every_card_inside_a_scissor_on_its_seat() {
    load_strings();
    let mut bench = on_new();
    bench.canvas_style = canvas_style(&styles(&bench), CANVAS_INK);
    let frame = walk(&mut bench);
    let slot = frame
        .surface(CANVAS_SLOT)
        .cloned()
        .expect("the walker reserves the canvas");
    bench.hud_commands.clear();
    assert!(
        !bench.drive_canvas(Some(&slot), None, false, false),
        "no pointer, no edit"
    );
    bench.draw_canvas(Some(&slot));
    let cmds = &bench.hud_commands;
    assert!(
        matches!(cmds.first(), Some(HudCommand::Clip { rect: Some(r) }) if *r == [slot.x, slot.y, slot.w, slot.h]),
        "the run opens with the seat's scissor"
    );
    assert!(matches!(cmds.last(), Some(HudCommand::Clip { rect: None })));
    let titles: Vec<&str> = cmds
        .iter()
        .filter_map(|c| match c {
            HudCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    for key in ["noise", "ramp", "Output"] {
        assert!(titles.contains(&key), "no `{key}` card: {titles:?}");
    }
    let edges = cmds
        .iter()
        .filter(|c| matches!(c, HudCommand::Line { .. }))
        .count();
    assert_eq!(edges, 3, "ramp ← noise, base ← ramp, height ← noise");
}

// ── the material binding + the commit ─────────────────────────────────────────

/// The dropdown binds by OPTION INDEX: 0 = Unbound, i+1 = the i-th material.
#[test]
fn the_material_dropdown_binds_by_option_index() {
    let mut bench = MaterialBench::shipped();
    let count = bench.materials.len();
    assert!(count > 0, "the material index loaded");
    fire(&mut bench, "sel_material", 0.0);
    assert_eq!(bench.graph().material, None, "option 0 is Unbound");
    let mut seen = Vec::new();
    for i in 0..count {
        fire(&mut bench, "sel_material", (i + 1) as f64);
        seen.push(bench.graph().material.expect("bound"));
    }
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), count, "each option binds a distinct material");
    let last = bench.graph().material;
    fire(&mut bench, "sel_material", (count + 5) as f64);
    assert_eq!(bench.graph().material, last, "past the list binds nothing");
}

/// Every material option's label binds that material's NAME, and the control echoes the
/// bound option index — -1 (the placeholder) for a slot the index does not list.
#[test]
fn the_dropdown_reads_the_binding_as_a_specific_name() {
    let mut bench = MaterialBench::shipped();
    let model = bench.model();
    for (i, def) in bench.materials.iter().enumerate() {
        assert_eq!(
            model.text(&format!("material_opt{i}")),
            Some(def.name.as_str()),
            "option {} binds the i-th material's name",
            i + 1
        );
    }
    bench.set_material_by_option(1.0);
    assert_eq!(bench.model().number("sel_material"), Some(1.0));
    let unlisted = unlisted_slot(&bench);
    bench.designer.bind(Some(unlisted));
    assert_eq!(bench.model().number("sel_material"), Some(-1.0));
}

/// A materials-list row binds the graph to THAT material — not an image edit.
#[test]
fn the_materials_list_binds_by_row() {
    let mut bench = MaterialBench::shipped();
    for i in 0..bench.materials.len() {
        let want = bench.materials[i].id;
        assert!(!fire(&mut bench, &format!("mat_pick_{i}"), true));
        assert_eq!(bench.graph().material, Some(want), "row {i}");
    }
}

/// The size control only ever lands on an OFFERED rung.
#[test]
fn the_size_control_never_reaches_a_gated_rung() {
    let mut bench = MaterialBench::shipped();
    assert_eq!(bench.bake_rung().px, flicker_texture::BAKE_DEFAULT);
    for _ in 0..(flicker_texture::BAKE_SIZES.len() * 3) {
        fire(&mut bench, "bake_size", true);
        assert!(bench.bake_rung().enabled);
    }
}

/// A rename edits the BOUND material's name and persists it by byte id — into a TEMP
/// copy of the index, never the real content file.
#[test]
fn the_material_rename_edits_a_bound_name_and_persists() {
    let dir = std::env::temp_dir().join(format!("flicker_mtl_rename_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let real = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/materials.json"
    );
    std::fs::copy(real, dir.join("materials.json")).unwrap();
    let mut bench = MaterialBench::shipped();
    bench.data_dir = dir.clone();
    bench.set_material_by_option(1.0);
    let id = bench.graph().material.expect("bound");
    bench.begin_rename();
    assert!(bench.is_renaming());
    bench.set_rename_draft("Xy");
    bench.commit_rename();
    assert!(!bench.is_renaming());
    assert_eq!(
        bench
            .materials
            .iter()
            .find(|def| def.id == id)
            .unwrap()
            .name,
        "Xy"
    );
    let raw = std::fs::read_to_string(dir.join("materials.json")).unwrap();
    assert!(raw.contains("\"Xy\""));
    bench.begin_rename();
    bench.set_rename_draft("   ");
    bench.commit_rename();
    assert!(bench.is_renaming(), "an empty name keeps the field open");
    bench.cancel_rename();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Unbound is not a material and has no byte to relabel.
#[test]
fn rename_needs_a_bound_material() {
    let mut bench = MaterialBench::shipped();
    bench.set_material_by_option(0.0);
    bench.begin_rename();
    assert!(!bench.is_renaming());
}

/// The bound byte id is explicit in the model.
#[test]
fn the_byte_id_is_explicit_in_the_model() {
    let mut bench = MaterialBench::shipped();
    bench.set_material_by_option(0.0);
    assert_eq!(bench.model().text("material_id_label"), Some("—"));
    bench.set_material_by_option(1.0);
    let id = bench.graph().material.unwrap();
    assert_eq!(
        bench.model().text("material_id_label"),
        Some(format!("#{id}").as_str())
    );
}

/// A scratch staging root, unique per process and call (no `Drop` cleanup: a commit is
/// a worker job still writing after the test returns).
fn staging_scratch(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "flicker_mtl_staging_{name}_{}_{seq}",
        std::process::id()
    ))
}

/// A commit is a WORKER job: the click returns at once, a second one is not a second
/// artifact, and it writes where it was aimed — and what lands is the v2 graph.
#[test]
fn a_commit_starts_once_and_lands_the_graph() {
    let mut bench = MaterialBench::shipped();
    assert_eq!(
        bench.model().get("material_bound"),
        Some(&Value::Bool(true))
    );
    let target = staging_scratch("dispatch");
    assert_ne!(target, flicker_content::roots().staging());
    bench.set_staging_root(target.clone());
    let smallest = flicker_texture::size::offered()
        .map(|r| r.px)
        .min()
        .unwrap();
    bench.size_rung = flicker_texture::size::offered()
        .position(|r| r.px == smallest)
        .unwrap();
    fire(&mut bench, "commit", true);
    assert_eq!(bench.commit_state, CommitState::Working);
    fire(&mut bench, "commit", true);
    assert_eq!(
        bench.commit_state,
        CommitState::Working,
        "not a second artifact"
    );
    let state = bench
        .commit_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the commit reports back");
    let CommitState::Done(dir) = state else {
        panic!("the commit failed: {state:?}")
    };
    let slot = bench.graph().material.unwrap();
    assert_eq!(
        std::path::PathBuf::from(&dir),
        flicker_content::bundle_dir(&target, slot)
    );
    let text = flicker_content::package::read_text(&std::path::Path::new(&dir).join(format!(
        "{}.texture.json",
        flicker_materials::slot_dir(slot)
    )))
    .expect("the document landed");
    assert_eq!(&load_material_json(&text).unwrap(), bench.graph());
}

// ── the emissive glows: authored as data, committed, opened again ────────────────

/// The authored factory library — the content data dir's `factory_materials.json`, read
/// through the list reader the bench's preset library and the headless commit
/// (`examples/commit_library.rs`) use.
fn factory_graphs() -> Vec<MaterialGraph> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/factory_materials.json"
    ))
    .expect("factory_materials.json reads");
    commit::graphs_in(&text).expect("every factory entry is a material document")
}

/// The authored glow graphs — the content data dir's `glow_materials.json`, read through
/// the list reader the headless commit (`examples/commit_library.rs`) uses.
fn glow_graphs() -> Vec<MaterialGraph> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/glow_materials.json"
    ))
    .expect("glow_materials.json reads");
    commit::graphs_in(&text).expect("every authored glow is a material document")
}

/// THE EIGHT GLOWS, as authored: one graph per emissive slot in palette order, each bound
/// to its slot, named as that slot's index row (an EMISSIVE row — the slot law holds), its
/// glow a Palette node holding EXACTLY that slot's curated colour (the designer's Glow
/// select shows it as that entry) — which is also the row's colour — at the strength the
/// designer starts a glow at (`params.new_glow_strength`), and no problems anywhere in the
/// graph.
#[test]
fn the_glow_materials_are_eight_curated_emissive_graphs() {
    use flicker_materials::{slot_accepts, RenderClass, EMISSIVE_FIRST};
    let bench = MaterialBench::shipped();
    let graphs = glow_graphs();
    assert_eq!(
        graphs.len(),
        bench.palette.len(),
        "one glow per curated colour"
    );
    assert_eq!(
        graphs.len(),
        usize::from(u8::MAX - EMISSIVE_FIRST) + 1,
        "and one per emissive slot"
    );
    let def = flicker::ui::SceneDef::parse("materialbench", crate::SW_SCENE).unwrap();
    let strength = new_glow_strength(&def.params);
    for (i, graph) in graphs.iter().enumerate() {
        let slot = EMISSIVE_FIRST + i as u8;
        assert_eq!(
            graph.material,
            Some(slot),
            "{}: bound to its slot",
            graph.name
        );
        let row = bench
            .materials
            .iter()
            .find(|d| d.id == slot)
            .unwrap_or_else(|| panic!("the index holds a row at {slot}"));
        assert_eq!(graph.name, row.name, "slot {slot}: the graph names its row");
        assert_eq!(row.render_class, Some(RenderClass::Emissive), "slot {slot}");
        assert!(
            slot_accepts(slot, bench.class_of(Some(slot))),
            "slot {slot}"
        );
        assert!(
            flicker_texture::problems(graph).is_empty(),
            "{}: {:?}",
            graph.name,
            flicker_texture::problems(graph)
        );
        let emit = graph.output.emit.as_ref().expect("a glow emits");
        assert_eq!(
            emit.strength, strength,
            "{}: the designer's new-glow strength",
            graph.name
        );
        assert!(
            emit.mask.is_some(),
            "{}: the glow is masked, not a wash",
            graph.name
        );
        let colour = bench.palette.get(i).expect("a curated colour").rgb;
        let d = designer::Designer::new(graph.clone(), strength);
        assert_eq!(
            d.emit_option(&bench.palette),
            (i + 1) as f64,
            "{}: the Glow select shows palette entry {i}",
            graph.name
        );
        assert_eq!(
            row.color, colour,
            "slot {slot}: the row wears the curated colour"
        );
    }
}

/// A COMMITTED GLOW CLEARS THE BLOOM at the bench's authored gain: every glow's baked
/// Emit map is PATTERNED (dark where its mask is off) and burns a full texel — 255, the
/// sRGB endpoint, exactly 1.0 linear — somewhere, which the lit stage multiplies by its
/// authored `emissive_gain`; that product must clear the stage's own bloom threshold AND
/// its knee, so the soft-knee bright pass keeps it whole and the halo spreads.
#[test]
fn a_committed_glow_clears_the_bloom_at_the_authored_gain() {
    use flicker::render::PassKind;
    let def = flicker::ui::SceneDef::parse("materialbench", crate::SW_SCENE).unwrap();
    let styles = flicker::ui::load_styles_for(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/sensorium/resources/ui_theme.json"
        ),
        def.styles.as_ref(),
    );
    let stage = flicker::ui::stage_def(&styles, LIT_STAGE_SOURCE).expect("the lit stage");
    let bloom = stage
        .recipe()
        .iter()
        .find_map(|p| match &p.kind {
            PassKind::Bloom(b) => Some(*b),
            _ => None,
        })
        .expect("the lit stage blooms");
    // The sRGB EOTF at a byte — the Emit map is uploaded as colour.
    let linear = |b: u8| {
        let c = f32::from(b) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    for graph in glow_graphs() {
        let set = flicker_texture::bake_graph(&graph, 64).expect("a glow bakes");
        let emit = set.get(MapKind::Emit).expect("an Emit map");
        let brightest: Vec<u8> = emit
            .pixels
            .chunks(4)
            .map(|p| p[0].max(p[1]).max(p[2]))
            .collect();
        let peak = *brightest.iter().max().unwrap();
        let dark = brightest.iter().filter(|&&b| b == 0).count();
        assert_eq!(peak, 255, "{}: the veins burn a full texel", graph.name);
        assert!(
            dark * 4 > brightest.len(),
            "{}: the glow is a pattern, not a wash ({dark} of {} dark)",
            graph.name,
            brightest.len()
        );
        let radiance = linear(peak) * stage.emissive_gain;
        assert!(
            radiance > bloom.threshold + bloom.knee,
            "{}: a full Emit texel at gain {} is {radiance}, under the bloom's {} + {}",
            graph.name,
            stage.emissive_gain,
            bloom.threshold,
            bloom.knee
        );
    }
}

/// The option the material dropdown offers for `slot`.
fn option_of(bench: &MaterialBench, slot: u8) -> f64 {
    bench
        .materials
        .iter()
        .position(|d| d.id == slot)
        .map(|i| (i + 1) as f64)
        .expect("the slot is listed")
}

/// A scratch staging tree holding the first authored glow (Glow Red, slot 248), and a bench
/// aimed at it — nothing promoted. Returns the bench, the tree and the staged graph.
fn bench_over_a_staged_glow(name: &str) -> (MaterialBench, std::path::PathBuf, MaterialGraph) {
    let staging = staging_scratch(name);
    let red = glow_graphs()[0].clone();
    let mut bench = MaterialBench::shipped();
    bench.set_staging_root(staging.clone());
    commit::commit(&red, bench.class_of(red.material), 16, &staging).expect("the glow stages");
    (bench, staging, red)
}

/// THE COMMITTED-LOOK PROMPT, all three answers × a clean and an UNSAVED document, through
/// the dispatcher: a pick of a slot with a committed look ALWAYS asks and changes nothing
/// (its OPEN reads as a discard exactly when the work is unsaved); OPEN makes the committed
/// look the document (an image edit, a new workspace); BIND HERE keeps the work — edits and
/// all — and binds it to the picked slot (no image edit; the next Commit replaces the
/// look); KEEP changes nothing and leaves the Model on the slot the work was bound to.
#[test]
fn a_committed_slot_always_asks_open_bind_here_or_keep() {
    use flicker_materials::EMISSIVE_FIRST;
    for unsaved in [false, true] {
        for answer in [ASK_REPLACE, ASK_BIND_HERE, ASK_KEEP] {
            let case = format!("{answer}, unsaved {unsaved}");
            let (mut bench, staging, red) = bench_over_a_staged_glow("ask3");
            if unsaved {
                assert!(fire(&mut bench, "reseed", true), "{case}: an edit");
            }
            assert_eq!(bench.designer.unsaved(), unsaved, "{case}");
            let before = bench.graph().clone();
            let was = bench.model().number("sel_material");
            bench.fit_pending = None;

            let pick = option_of(&bench, EMISSIVE_FIRST);
            assert!(
                !fire(&mut bench, "sel_material", pick),
                "{case}: asking is no edit"
            );
            assert!(bench.prompt_armed, "{case}: the prompt is armed");
            assert_eq!(
                bench.pending.as_ref().map(|p| p.kind),
                Some(PendingKind::Committed {
                    slot: EMISSIVE_FIRST
                }),
                "{case}"
            );
            assert_eq!(
                bench.graph(),
                &before,
                "{case}: nothing changes before the answer"
            );
            let open = MaterialBench::committed_answers(bench.designer.unsaved())[0].0;
            assert_eq!(
                open,
                if unsaved {
                    "$mtl_open_committed_discard"
                } else {
                    "$mtl_open_committed"
                },
                "{case}: OPEN says it discards exactly when there is work to lose"
            );

            bench.prompt_armed = false; // `update` took the arm and pushed the prompt
            bench.modal_closed(PROMPT_TREE, answer, None);
            assert!(bench.pending.is_none(), "{case}: answered");
            match answer {
                ASK_REPLACE => {
                    assert_eq!(bench.graph(), &red, "{case}: the committed look opened");
                    assert!(!bench.designer.unsaved(), "{case}: a fresh load");
                    assert_eq!(bench.preset, None, "{case}: no preset");
                    assert_eq!(bench.fit_pending, Some(Framing::Output), "{case}");
                }
                ASK_BIND_HERE => {
                    assert_eq!(bench.graph().nodes, before.nodes, "{case}: the work stays");
                    assert_eq!(bench.graph().seed, before.seed, "{case}");
                    assert_eq!(bench.graph().material, Some(EMISSIVE_FIRST), "{case}");
                    assert_eq!(
                        bench.designer.unsaved(),
                        unsaved,
                        "{case}: binding is no edit"
                    );
                    assert_eq!(bench.model().number("sel_material"), Some(pick), "{case}");
                    assert_eq!(bench.fit_pending, None, "{case}: the same workspace");
                }
                _ => {
                    assert_eq!(bench.graph(), &before, "{case}: nothing changed");
                    assert_eq!(bench.model().number("sel_material"), was, "{case}");
                }
            }
            let _ = std::fs::remove_dir_all(&staging);
        }
    }
}

/// A slot with NO committed look keeps today's behaviour — a plain re-bind, no prompt, no
/// image edit, no edit at all — whether the work is clean or unsaved; re-picking the bound
/// slot changes nothing; and any close of a prompt it did not name keeps the work.
#[test]
fn a_slot_with_nothing_committed_rebinds_without_asking() {
    use flicker_materials::EMISSIVE_FIRST;
    let (mut bench, staging, _) = bench_over_a_staged_glow("plain");
    for unsaved in [false, true] {
        if unsaved {
            assert!(fire(&mut bench, "reseed", true));
        }
        let held = bench.graph().nodes.clone();
        let slot = EMISSIVE_FIRST + 1 + u8::from(unsaved);
        let opt = option_of(&bench, slot);
        assert!(!fire(&mut bench, "sel_material", opt));
        assert!(!bench.prompt_armed && bench.pending.is_none(), "no prompt");
        assert_eq!(bench.graph().material, Some(slot), "re-bound");
        assert_eq!(bench.graph().nodes, held, "the work in hand");
        assert_eq!(
            bench.designer.unsaved(),
            unsaved,
            "a binding is not an edit"
        );
    }
    let bound = bench.model().number("sel_material").unwrap();
    assert!(!fire(&mut bench, "sel_material", bound));
    assert!(!bench.prompt_armed, "the bound slot asks nothing");

    // A back-out the prompt did not name keeps the work.
    let held = bench.graph().clone();
    let opt = option_of(&bench, EMISSIVE_FIRST);
    assert!(!fire(&mut bench, "sel_material", opt));
    bench.prompt_armed = false;
    bench.modal_closed(PROMPT_TREE, "cancelled", None);
    assert_eq!(bench.graph(), &held, "never a silent discard");
    assert!(bench.pending.is_none());
    let _ = std::fs::remove_dir_all(&staging);
}

/// A committed document that will not read or load — here a PROMOTED one, in `package/` —
/// is named in neutral words on the status line, its PATH and the LOADER's reason beside
/// the token as data (`status_detail`), and the pick still binds: nothing opens, nothing is
/// asked.
#[test]
fn an_unreadable_committed_document_is_named_with_its_path_and_reason() {
    use flicker_materials::EMISSIVE_FIRST;
    let tree = staging_scratch("unreadable");
    let (staging, package) = (tree.join("staging"), tree.join("package"));
    let mut bench = MaterialBench::shipped();
    bench.set_staging_root(staging);
    bench.set_package_root(package.clone());
    let slot = EMISSIVE_FIRST + 2;
    let doc = flicker_content::bundle_dir(&package, slot).join(format!(
        "{}.texture.json",
        flicker_materials::slot_dir(slot)
    ));
    flicker_content::package::write_text(&doc, "{\"nodes\": 7}").unwrap();
    let before = bench.graph().nodes.clone();
    let pick = option_of(&bench, slot);
    assert!(!fire(&mut bench, "sel_material", pick));
    assert_eq!(bench.graph().material, Some(slot), "the pick still binds");
    assert_eq!(bench.graph().nodes, before, "nothing opened");
    assert!(
        !bench.prompt_armed,
        "nothing to ask about: nothing would open"
    );
    let (token, detail) = bench.status_line();
    assert_eq!(token, COMMITTED_UNREADABLE);
    assert!(
        detail.contains(&flicker_materials::slot_dir(slot)) && detail.contains("texture.json"),
        "the detail names the document: {detail}"
    );
    assert!(detail.contains("v2"), "and the loader's reason: {detail}");
    let model = bench.model();
    assert_eq!(model.text("status_line"), Some(COMMITTED_UNREADABLE));
    assert_eq!(model.text("status_detail"), Some(detail.as_str()));
    let _ = std::fs::remove_dir_all(&tree);
}

/// The ERROR events `f` emits on THIS thread, each rendered as its `name=value` fields — a
/// scoped subscriber (none is installed globally, and a worker's events are its own
/// thread's), so a test can hold a fail-loud path to exactly the errors it should raise.
fn errors_during<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    use std::fmt::Write as _;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata};

    struct Errors(Arc<Mutex<Vec<String>>>);
    struct Fields(String);
    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            let _ = write!(self.0, "{}={value:?} ", field.name());
        }
    }
    impl tracing::Subscriber for Errors {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() == Level::ERROR
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0.lock().unwrap().push(fields.0);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let out = tracing::subscriber::with_default(Errors(Arc::clone(&seen)), f);
    let errors = seen.lock().unwrap().clone();
    (out, errors)
}

/// A scratch `(staging, package)` pair no other test shares (nothing exists until written).
fn scratch_roots(name: &str) -> (PathBuf, PathBuf) {
    let tree = staging_scratch(name);
    (tree.join("staging"), tree.join("package"))
}

/// A FRESH bench on the shipped scene over `roots` — through the one constructor both seams
/// share — and the ERROR events its construction raised.
fn fresh_over((staging, package): &(PathBuf, PathBuf)) -> (MaterialBench, Vec<String>) {
    let def = SceneDef::parse("materialbench", SW_SCENE).unwrap();
    errors_during(|| {
        MaterialBench::from_parts(
            def.tree,
            def.styles,
            &def.params,
            (staging.clone(), package.clone()),
        )
    })
}

/// THE OPENING SLOT'S STAGED LOOK OPENS. Re-picking the slot already bound asks nothing, so
/// a bench that opened on the factory entry over a slot holding a COMMITTED look would never
/// offer that look, and its first Commit would replace it unasked. A fresh bench opens on
/// the look instead — the committed-look prompt's OPEN, taken, since nothing is unsaved at
/// startup — and a look that is not the entry opens as a CUSTOM document: no preset, not
/// unsaved, bound to the slot, no notice, nothing logged. Over empty roots: the entry.
#[test]
fn a_fresh_bench_opens_on_its_slots_staged_look() {
    let entry = &factory_graphs()[0];
    let slot = entry.material.expect("the first factory entry is bound");
    let (bench, errors) = fresh_over(&scratch_roots("open_empty"));
    assert_eq!(bench.graph(), entry, "nothing committed: the entry");
    assert_eq!(bench.preset, Some(1));
    assert!(errors.is_empty(), "{errors:?}");

    let roots = scratch_roots("open_staged");
    let look = rack_granite();
    assert_eq!(
        look.material,
        Some(slot),
        "the fixture binds the entry's slot"
    );
    assert_ne!(&look, entry, "and is another look");
    commit::commit(&look, bench.class_of(Some(slot)), 16, &roots.0).expect("the look stages");
    let (bench, errors) = fresh_over(&roots);
    assert_eq!(bench.graph(), &look, "the staged look opened");
    assert_eq!(bench.preset, None, "a committed look is no preset");
    assert_eq!(bench.model().number("sel_preset"), Some(-1.0));
    assert!(!bench.designer.unsaved(), "nothing is unsaved at startup");
    assert_eq!(bench.graph().material, Some(slot));
    assert_eq!(bench.designer.notice(), None);
    assert!(errors.is_empty(), "{errors:?}");
    let _ = std::fs::remove_dir_all(roots.0.parent().unwrap());
}

/// A committed look that IS the factory entry — the library staged or promoted
/// byte-for-byte as the file has it; here a PROMOTED one, from `package/` — opens as the
/// PRESET it is: the select reads it, and the bumpers step on from it.
#[test]
fn a_committed_look_that_is_the_entry_opens_as_the_preset() {
    let entry = &factory_graphs()[0];
    let slot = entry.material.expect("the first factory entry is bound");
    let roots = scratch_roots("open_promoted");
    let class = MaterialBench::shipped().class_of(Some(slot));
    commit::commit(entry, class, 16, &roots.1).expect("the entry commits");
    let (bench, errors) = fresh_over(&roots);
    assert_eq!(bench.graph(), entry, "the promoted look opened");
    assert_eq!(bench.preset, Some(1), "and it is the preset");
    assert_eq!(bench.model().number("sel_preset"), Some(1.0));
    assert!(errors.is_empty(), "{errors:?}");
    let _ = std::fs::remove_dir_all(roots.0.parent().unwrap());
}

/// An opening look whose committed document will not load is ONE error naming the slot,
/// both roots and the document — and the factory entry opens, as the preset, the status
/// line saying why ([`COMMITTED_UNREADABLE`], the reason beside it), as a slot pick does.
#[test]
fn an_unreadable_opening_look_is_one_error_and_the_entry_opens() {
    let entry = &factory_graphs()[0];
    let slot = entry.material.expect("the first factory entry is bound");
    let roots = scratch_roots("open_unreadable");
    let doc = flicker_content::bundle_dir(&roots.0, slot).join(format!(
        "{}.texture.json",
        flicker_materials::slot_dir(slot)
    ));
    flicker_content::package::write_text(&doc, "not a material document").unwrap();
    let (bench, errors) = fresh_over(&roots);
    assert_eq!(bench.graph(), entry, "the factory entry opens");
    assert_eq!(bench.preset, Some(1));
    let (token, detail) = bench.status_line();
    assert_eq!(token, COMMITTED_UNREADABLE);
    assert!(detail.contains("texture.json"), "the reason: {detail}");
    assert_eq!(errors.len(), 1, "ONE error: {errors:?}");
    let named = |root: &std::path::Path| errors[0].contains(&root.display().to_string());
    assert!(
        errors[0].contains(&format!("slot={slot}"))
            && named(&roots.0)
            && named(&roots.1)
            && errors[0].contains("texture.json"),
        "the error names the slot, both roots and the document: {}",
        errors[0]
    );
    let _ = std::fs::remove_dir_all(roots.0.parent().unwrap());
}

/// EVERY WHOLE-DOCUMENT LOAD asks over unsaved work: the preset select, the patch
/// bumpers and Roll — the shared unsaved-changes prompt (its Keep also the back-out). Over
/// a clean document each loads at once; over an edited one each changes nothing until
/// DISCARD loads it (the preset it names, or the roll) and KEEP leaves the work.
#[test]
fn presets_patches_and_roll_ask_over_unsaved_work() {
    let mut bench = MaterialBench::shipped();
    assert!(
        fire(&mut bench, "sel_preset", 2.0),
        "clean: a pick loads at once"
    );
    assert_eq!(bench.preset, Some(2));
    assert!(
        fire(&mut bench, "patch_next", true),
        "clean: so does a bumper"
    );
    assert_eq!(bench.preset, Some(3));
    assert!(!bench.prompt_armed);

    for (key, value, want) in [
        ("sel_preset", Value::Number(1.0), Some(1)),
        // From preset 1 (the pick above), the bumper steps back to 0 — New.
        ("patch_prev", Value::Bool(true), Some(0)),
        ("roll", Value::Bool(true), None),
    ] {
        for answer in [ASK_KEEP, ASK_REPLACE] {
            let case = format!("{key} → {answer}");
            assert!(fire(&mut bench, "reseed", true), "{case}: an edit");
            let edited = bench.graph().clone();
            let preset = bench.preset;
            assert!(
                !fire(&mut bench, key, value.clone()),
                "{case}: asking is no edit"
            );
            assert!(bench.prompt_armed, "{case}: the prompt is armed");
            assert!(
                matches!(
                    bench.pending.as_ref().map(|p| p.kind),
                    Some(PendingKind::Load { .. })
                ),
                "{case}"
            );
            assert_eq!(
                bench.graph(),
                &edited,
                "{case}: nothing changes before the answer"
            );
            assert_eq!(
                bench.prompt().map(|p| p.cancel_result().to_string()),
                Some(ASK_KEEP.to_string()),
                "{case}: backing out keeps the work"
            );
            bench.prompt_armed = false;
            bench.modal_closed(PROMPT_TREE, answer, None);
            assert!(bench.pending.is_none(), "{case}");
            if answer == ASK_KEEP {
                assert_eq!(bench.graph(), &edited, "{case}: the work stays");
                assert_eq!(bench.preset, preset, "{case}");
            } else {
                assert_ne!(bench.graph(), &edited, "{case}: the load replaced it");
                assert_eq!(bench.preset, want, "{case}");
                assert!(!bench.designer.unsaved(), "{case}: a fresh load");
                if let Some(i) = want {
                    assert_eq!(bench.graph(), &bench.presets[i], "{case}");
                }
            }
        }
    }
}

/// A SUCCESSFUL COMMIT SAVES THE WORK: edit, commit, and once the commit lands a load no
/// longer asks — and the committed-look prompt's OPEN no longer reads as a discard. The
/// commit saves exactly what it staged: an edit made while it baked stays unsaved. A REFUSED
/// commit saves nothing.
#[test]
fn a_landed_commit_saves_the_work_it_staged() {
    let (mut bench, staging, _) = bench_over_a_staged_glow("saved");
    let smallest = flicker_texture::size::offered()
        .map(|r| r.px)
        .min()
        .unwrap();
    bench.size_rung = flicker_texture::size::offered()
        .position(|r| r.px == smallest)
        .unwrap();
    let land = |bench: &mut MaterialBench| {
        let state = bench
            .commit_rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the commit reports back");
        bench.commit_landed(state);
    };

    assert!(fire(&mut bench, "reseed", true));
    assert!(bench.designer.unsaved());
    assert!(
        !fire(&mut bench, "commit", true),
        "a commit is no image edit"
    );
    land(&mut bench);
    assert!(
        matches!(bench.commit_state, CommitState::Done(_)),
        "{:?}",
        bench.commit_state
    );
    assert!(!bench.designer.unsaved(), "what was committed is saved");
    assert_eq!(
        MaterialBench::committed_answers(bench.designer.unsaved())[0].0,
        "$mtl_open_committed",
        "nothing to lose: OPEN is a plain open"
    );
    assert!(
        fire(&mut bench, "roll", true),
        "a load over committed work loads at once"
    );
    assert!(!bench.prompt_armed);

    // An edit made WHILE the commit bakes stays unsaved: the next load asks.
    assert!(fire(&mut bench, "reseed", true));
    assert!(!fire(&mut bench, "commit", true));
    assert!(fire(&mut bench, "reseed", true), "edited during the bake");
    land(&mut bench);
    assert!(
        bench.designer.unsaved(),
        "the later edit is not what was staged"
    );
    assert!(!fire(&mut bench, "roll", true));
    assert!(bench.prompt_armed, "so the load asks");
    bench.prompt_armed = false;
    bench.modal_closed(PROMPT_TREE, ASK_KEEP, None);

    // A refused commit (Air holds nothing) saves nothing.
    bench.designer.bind(Some(0));
    assert!(!fire(&mut bench, "commit", true));
    land(&mut bench);
    assert!(
        matches!(bench.commit_state, CommitState::Failed { .. }),
        "{:?}",
        bench.commit_state
    );
    assert!(bench.designer.unsaved(), "a refusal saves nothing");
    let _ = std::fs::remove_dir_all(&staging);
}

/// AN EMISSIVE MATERIAL'S LOOK THAT DOES NOT GLOW commits — the slot law weighs the row,
/// not the look — but the landed Commit leaves a WARNING on the status line (its token, the
/// bundle's folder beside it as data), never a refusal. The warning speaks of the STAGED
/// look, so the next graph edit retires it (a view toggle does not). A glowing look, or a
/// look for a slot whose material is not emissive, warns nothing.
#[test]
fn a_dull_emissive_look_commits_with_a_warning() {
    use flicker_materials::EMISSIVE_FIRST;
    load_strings();
    let staging = staging_scratch("dull");
    let mut bench = MaterialBench::shipped();
    bench.set_staging_root(staging.clone());
    let smallest = flicker_texture::size::offered()
        .map(|r| r.px)
        .min()
        .unwrap();
    bench.size_rung = flicker_texture::size::offered()
        .position(|r| r.px == smallest)
        .unwrap();
    let land = |bench: &mut MaterialBench| {
        let state = bench
            .commit_rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the commit reports back");
        bench.commit_landed(state);
    };

    // The first factory patch does not glow; bound to a non-emissive slot it warns nothing.
    assert!(!bench.graph().glows(), "the patch does not glow");
    assert!(!fire(&mut bench, "commit", true));
    land(&mut bench);
    assert_eq!(bench.status_line(), (String::new(), String::new()));

    // Bound to an emissive slot (nothing committed there: a plain re-bind), it warns.
    let opt = option_of(&bench, EMISSIVE_FIRST);
    assert!(!fire(&mut bench, "sel_material", opt));
    assert!(!fire(&mut bench, "commit", true));
    land(&mut bench);
    let CommitState::Done(dir) = bench.commit_state.clone() else {
        panic!("a dull look is NOT refused: {:?}", bench.commit_state)
    };
    assert_eq!(
        bench.status_line(),
        (NO_EMIT_WARNING.to_string(), dir.clone())
    );
    assert_eq!(bench.model().text("status_line"), Some(NO_EMIT_WARNING));
    assert!(
        strings::resolve(NO_EMIT_WARNING).contains("staged look"),
        "the copy speaks of the STAGED look"
    );

    // It speaks of the STAGED look, so the next graph edit retires it — on the status line
    // at once, and from the bench at the frame's settle; a view toggle is no edit.
    assert!(
        !fire(&mut bench, "lit_body", true),
        "a view toggle is no edit…"
    );
    assert_eq!(bench.status_line().0, NO_EMIT_WARNING, "…and keeps it");
    assert!(fire(&mut bench, "reseed", true), "an edit");
    assert_eq!(
        bench.status_line(),
        (String::new(), String::new()),
        "the edit cleared it"
    );
    bench.retire_stale_warning();
    assert!(bench.commit_warning.is_none(), "gone, not just hidden");

    // A glowing look in the same slot: committed clean, the warning gone.
    let mut glow = glow_graphs()[0].clone();
    glow.material = Some(EMISSIVE_FIRST);
    assert!(glow.glows());
    bench.designer.load(glow);
    assert!(!fire(&mut bench, "commit", true));
    land(&mut bench);
    assert_eq!(bench.status_line(), (String::new(), String::new()));
    let _ = std::fs::remove_dir_all(&staging);
}

/// A PROMOTED look still opens: a promotion MOVES the bundle from `staging/` to
/// `package/`, so a slot whose bundle lives only in the package tree opens from there (its
/// pick asks; OPEN opens it) — and a slot committed in BOTH opens the staged (newer) one.
/// Both trees are the bench's injected roots (the content roots service at runtime), never
/// a spelled path.
#[test]
fn a_promoted_look_opens_from_the_package_tree() {
    use flicker_materials::EMISSIVE_FIRST;
    let tree = staging_scratch("promoted");
    let (staging, package) = (tree.join("staging"), tree.join("package"));
    let glows = glow_graphs();
    let (red, green) = (&glows[0], &glows[1]);
    let mut bench = MaterialBench::shipped();
    bench.set_staging_root(staging.clone());
    bench.set_package_root(package.clone());
    commit::commit(red, bench.class_of(red.material), 16, &package).expect("red is promoted");
    commit::commit(green, bench.class_of(green.material), 16, &package).expect("green too");
    let mut restaged = green.clone();
    restaged.seed ^= 0x5EED;
    commit::commit(&restaged, bench.class_of(restaged.material), 16, &staging)
        .expect("a newer green is staged over it");

    let (first, second) = (
        option_of(&bench, EMISSIVE_FIRST),
        option_of(&bench, EMISSIVE_FIRST + 1),
    );
    for (pick, want) in [(first, red), (second, &restaged)] {
        assert!(!fire(&mut bench, "sel_material", pick), "the pick asks");
        bench.prompt_armed = false;
        bench.modal_closed(PROMPT_TREE, ASK_REPLACE, None);
        assert_eq!(bench.graph(), want);
    }
    let _ = std::fs::remove_dir_all(&tree);
}

/// THE PROMPTS ARE THE SHARED `choice_dialog`, answering in the bench's own action names:
/// the committed-look prompt's three slots in order — OPEN (its label the discard form
/// exactly when the work is unsaved), BIND HERE, KEEP EDITING — every label a stringtable
/// row; and every prompt's back-out (Esc / pad-B) is the SAFE answer, Keep editing.
#[test]
fn the_prompts_answer_in_the_benchs_own_names_and_back_out_safely() {
    load_strings();
    assert_eq!(PROMPT_TREE, "choice_dialog");
    for unsaved in [false, true] {
        let answers = MaterialBench::committed_answers(unsaved);
        assert_eq!(
            answers.map(|(_, action)| action),
            [ASK_REPLACE, ASK_BIND_HERE, ASK_KEEP]
        );
        assert_eq!(answers[1].0, "$mtl_bind_here");
        assert_eq!(answers[2].0, "$modal_lbl_keep_editing");
        for (label, _) in answers {
            assert_ne!(strings::resolve(label).as_ref(), label, "{label} has a row");
        }
        assert_eq!(
            MaterialBench::committed_prompt(unsaved).cancel_result(),
            ASK_KEEP
        );
    }
    assert_ne!(
        MaterialBench::committed_answers(false)[0].0,
        MaterialBench::committed_answers(true)[0].0,
        "the discard form is its own label"
    );
}

/// KEEP HANDS THE DROPDOWN BACK, on the REAL channel: a dropdown row picked through the
/// walker for a slot with a committed look arms the prompt and opens nothing — the select
/// HOLDS the pick while the prompt is up (the walker's local display ownership; the Model
/// still names the bound slot) — and answering Keep editing releases it, so the next walk
/// shows and echoes the Model again instead of re-reporting a pick the document declined.
#[test]
fn keeping_the_work_hands_the_dropdown_back_to_the_model() {
    load_strings();
    let staging = staging_scratch("keep");
    let mut bench = live(MaterialBench::shipped());
    bench.set_staging_root(staging.clone());
    // Every slot the dropdown's opening rows can land on holds a committed look, so the
    // row the click lands on is one the pick ASKS about.
    let mut doc = bench.graph().clone();
    for def in bench.materials.iter().take(120) {
        doc.material = Some(def.id);
        let _ = commit::commit(&doc, def.render_class, 4, &staging);
    }
    assert!(fire(&mut bench, "reseed", true), "unsaved work");
    let before = bench.graph().clone();
    let bound = bench.model().number("sel_material");

    let f = step(&mut bench, &snap());
    let field = f.rect("material_select").expect("the dropdown is laid out");
    click(&mut bench, centre(field));
    let f = step(&mut bench, &snap());
    let [_, my, _, mh] = f.popup.expect("the dropdown opened");
    click(&mut bench, Vec2::new(centre(field).x, my + mh * 0.75));
    assert!(
        bench.prompt_armed,
        "the pick armed the committed-look prompt"
    );
    assert!(bench.pending.is_some());
    assert_eq!(bench.graph(), &before, "and opened nothing");

    // The select holds the pick while the prompt is up (the frozen frame's view of it).
    let tree = frame_tree(&mut bench);
    let model = bench.model();
    let held = bench
        .walk(&tree, &model, &snap())
        .results
        .number("sel_material");
    assert_ne!(
        held, bound,
        "the select still shows the pick the prompt is about"
    );

    bench.prompt_armed = false; // `update` took the arm and pushed the prompt
    bench.modal_closed(PROMPT_TREE, ASK_KEEP, None);
    let model = bench.model();
    let after = bench
        .walk(&tree, &model, &snap())
        .results
        .number("sel_material");
    assert_eq!(after, bound, "Keep handed the dropdown back to the Model");
    assert_eq!(bench.graph(), &before, "and the work is exactly as it was");
    assert!(bench.pending.is_none());
    let _ = std::fs::remove_dir_all(&staging);
}

/// KEEP HANDS THE PRESET SELECT BACK too, on the REAL channel: over unsaved work a preset
/// row picked through the walker arms the unsaved-changes prompt and loads nothing; Keep
/// editing releases the select, so the next walk echoes the Model again.
#[test]
fn keeping_the_work_hands_the_preset_select_back_to_the_model() {
    load_strings();
    let mut bench = live(MaterialBench::shipped());
    assert!(fire(&mut bench, "reseed", true), "unsaved work");
    let before = bench.graph().clone();
    let shown = bench.model().number("sel_preset");
    assert_eq!(shown, Some(-1.0), "edited, the select shows no preset");

    let f = step(&mut bench, &snap());
    let field = f
        .rect("preset_select")
        .expect("the preset select is laid out");
    click(&mut bench, centre(field));
    let f = step(&mut bench, &snap());
    let [_, my, _, mh] = f.popup.expect("the preset menu opened");
    let rows = bench.presets.len() as f32;
    click(&mut bench, Vec2::new(centre(field).x, my + mh * 2.5 / rows));
    assert!(
        bench.prompt_armed,
        "the pick armed the unsaved-changes prompt"
    );
    assert!(matches!(
        bench.pending.as_ref().map(|p| p.kind),
        Some(PendingKind::Load {
            control: Some("sel_preset"),
            ..
        })
    ));
    assert_eq!(bench.graph(), &before, "and loaded nothing");
    let tree = frame_tree(&mut bench);
    let model = bench.model();
    let held = bench
        .walk(&tree, &model, &snap())
        .results
        .number("sel_preset");
    assert_ne!(
        held, shown,
        "the select holds the pick while the prompt is up"
    );

    bench.prompt_armed = false;
    bench.modal_closed(PROMPT_TREE, ASK_KEEP, None);
    let model = bench.model();
    let after = bench
        .walk(&tree, &model, &snap())
        .results
        .number("sel_preset");
    assert_eq!(after, shown, "Keep handed the select back to the Model");
    assert_eq!(bench.graph(), &before);
}

/// WHAT COUNTS AS UNSAVED: every graph mutation of the designer's set marks the document
/// edited; selecting, a refusal and binding or re-binding a slot never do (`pristine` — the
/// preset select's echo — still turns on a binding, as S4 defined it); a load clears it; and
/// a commit's success clears exactly the mutations it captured.
#[test]
fn only_graph_edits_leave_the_document_unsaved() {
    let mut d = designer::Designer::new(MaterialBench::shipped().presets[1].clone(), 1.0);
    assert!(!d.unsaved() && d.pristine(), "a fresh load");
    let first = d.graph().nodes[0].key.clone();
    d.select(Some(first.as_str()));
    d.select_edge(None);
    d.refuse(designer::WIRE_LOOPS);
    assert!(d.bind(Some(12)), "a re-bind");
    assert!(
        !d.unsaved(),
        "selecting, refusing and binding are not edits"
    );
    assert!(!d.pristine(), "but the document is no longer the preset");
    assert!(d.bind(None));
    assert!(!d.unsaved());

    assert!(d.reseed());
    assert!(d.unsaved(), "a mutation is an edit");
    let at = d.mutations();
    assert!(d.insert(NodeKind::Invert {}));
    d.saved_through(at);
    assert!(d.unsaved(), "the insert came after what was committed");
    d.saved_through(d.mutations());
    assert!(!d.unsaved(), "a commit of it all saves it all");

    assert!(d.reseed());
    d.load(d.graph().clone());
    assert!(!d.unsaved() && d.pristine(), "a load clears it");
}

/// The status line always says something specific.
#[test]
fn the_commit_status_is_never_blank() {
    let mut bench = MaterialBench::shipped();
    for state in [
        CommitState::Idle,
        CommitState::Working,
        CommitState::Done("/tmp/staging/materials/010".into()),
        CommitState::Failed {
            token: "$mtl_commit_io_failed",
            detail: "permission denied".into(),
        },
    ] {
        bench.commit_state = state.clone();
        let status = bench.model().text("commit_status").unwrap().to_string();
        assert!(!status.is_empty(), "{state:?} published a blank status");
    }
}

/// A REFUSED commit publishes a stringtable TOKEN, never English from Rust.
#[test]
fn a_refused_commit_publishes_a_token_not_copy() {
    let mut bench = MaterialBench::shipped();
    bench.set_staging_root(staging_scratch("refused"));
    bench.designer.bind(Some(0));
    bench.start_commit();
    let state = bench
        .commit_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the refusal comes back");
    bench.commit_state = state;
    assert_eq!(
        bench.model().text("commit_status"),
        Some(commit::Refusal::SlotClosed.token())
    );
}

/// A graph that cannot bake says WHY in the status line (its GraphError) — never a
/// silent black swatch — and the swatch keeps the last image that baked.
#[test]
fn a_graph_that_cannot_bake_says_why_in_the_status_line() {
    let mut bench = on_new();
    // A deliberately broken document: the height names a node that is not there.
    let mut broken = bench.graph().clone();
    broken.output.height = Some("nowhere".into());
    bench.designer.load(broken);
    bench.request_bake();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while bench.bake_error.is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
        bench.collect_bakes();
    }
    let (token, detail) = bench.status_line();
    assert_eq!(
        token, "$mtl_err_unknown_node",
        "the token for its kind of error"
    );
    assert!(
        detail.contains("nowhere"),
        "the status names the bad wire as data: {detail:?}"
    );
    let model = bench.model();
    assert_eq!(model.get("status_shown"), Some(&Value::Bool(true)));
    assert_eq!(model.text("status_line"), Some(token.as_str()));
    assert_eq!(model.text("status_detail"), Some(detail.as_str()));
    assert!(model.number("problem_count").unwrap() >= 1.0);
}

// ── the lit preview ────────────────────────────────────────────────────────────

/// The lit view's own controls change how you LOOK, never what it is.
#[test]
fn the_lit_controls_are_not_graph_edits() {
    let mut bench = MaterialBench::shipped();
    let body = bench.sample.body;
    assert!(!fire(&mut bench, "lit_body", true));
    assert_ne!(bench.sample.body, body);
    assert!(!fire(&mut bench, "lit_spin", false));
    assert!(!bench.sample.spinning);
}

/// THE AUTHORED STAGE IS ACTUALLY READ, and it lights — and frames — the sample, with
/// the `bloom` that makes a glow read as a glow ordered before the tonemap resolve.
#[test]
fn the_authored_stage_is_read_and_lights_the_sample() {
    use flicker::render::{PassKind, Vec3};
    let def = flicker::ui::SceneDef::parse("materialbench", crate::SW_SCENE).unwrap();
    let styles = flicker::ui::load_styles_for(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/sensorium/resources/ui_theme.json"
        ),
        def.styles.as_ref(),
    );
    let stage = flicker::ui::stage_def(&styles, LIT_STAGE_SOURCE)
        .unwrap_or_else(|| panic!("stages.{LIT_STAGE_SOURCE} is not authored"));
    let lum = |v: Vec3| v.x + v.y + v.z;
    assert!(
        lum(stage.lighting.sky_sun().color) > 0.1,
        "the stage has no sun"
    );
    assert!(lum(stage.lighting.ambient) > 0.0);
    assert!(stage.lighting.sky_sun().direction.length() > 0.5);
    assert!(stage.camera.expect("the lit stage frames its sample").dist > 0.0);
    assert!(stage.has_layer("material"));
    assert!(stage
        .layers_outside(flicker_materialview::SAMPLE_LAYERS)
        .is_empty());
    let recipe = stage.recipe();
    let at = |want: fn(&PassKind) -> bool| recipe.iter().position(|p| want(&p.kind));
    let bloom = at(|k| matches!(k, PassKind::Bloom(_))).expect("a bloom pass");
    let tonemap = at(|k| matches!(k, PassKind::TonemapGrade(_))).expect("an HDR resolve");
    let (order, cyclic) = stage.pass_order();
    assert!(!cyclic);
    let rank = |i: usize| order.iter().position(|&o| o == i).unwrap();
    assert!(rank(bloom) < rank(tonemap));

    // THE EMISSIVE GAIN is AUTHORED here (not the compiler's unity default), finite, past
    // unity — and the SAME number the Material Catalog's `mc_sample` authors, so a
    // material glows alike in the bench and the catalog.
    let raw: serde_json::Value = serde_json::from_str(crate::SW_SCENE).unwrap();
    let authored = raw["stages"][LIT_STAGE_SOURCE]["emissive_gain"]
        .as_f64()
        .unwrap_or_else(|| panic!("stages.{LIT_STAGE_SOURCE} authors no emissive_gain"));
    assert_eq!(stage.emissive_gain as f64, authored, "the compiler read it");
    assert!(
        stage.emissive_gain.is_finite() && stage.emissive_gain > 1.0,
        "a glow needs a gain past unity to clear the bloom: {}",
        stage.emissive_gain
    );
    let catalog: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../content/sensorium/scenes/materialcatalog.scene.json"
    ))
    .expect("the catalog's scene file parses");
    assert_eq!(
        Some(authored),
        catalog["stages"]["mc_sample"]["emissive_gain"].as_f64(),
        "the bench and the catalog burn a glow at the same gain"
    );
}

// ── the layout ────────────────────────────────────────────────────────────────

/// THE DESIGNER FITS the reference screen: the three columns resolve side by side with
/// real extent; the canvas and the swatch well are real panes; a node row is a real row;
/// and for EVERY node kind, its parameter cell and the output stage below it fit inside
/// the parameter column with every control a real height.
#[test]
fn the_designer_fits_the_reference_screen() {
    let mut bench = on_new();
    let frame = walk(&mut bench);
    let rect = |frame: &UiFrame, id: &str| {
        frame
            .rect(id)
            .unwrap_or_else(|| panic!("{id} does not resolve"))
    };
    let (nodes, view, params) = (
        rect(&frame, "mtl_nodes"),
        rect(&frame, "mtl_view"),
        rect(&frame, "mtl_params"),
    );
    assert!(nodes.pos.x + nodes.size.x <= view.pos.x, "nodes | view");
    assert!(view.pos.x + view.size.x <= params.pos.x, "view | params");
    assert!(
        params.pos.x + params.size.x <= 1920.0,
        "the params column is on screen"
    );
    let canvas = rect(&frame, CANVAS_SLOT);
    assert!(
        canvas.size.x > 600.0 && canvas.size.y > 250.0,
        "the canvas is a real pane: {:?}",
        canvas.size
    );
    assert!(
        frame.surface(CANVAS_SLOT).is_some(),
        "the walker reserves the canvas"
    );
    let well = rect(&frame, "mtl_swatch_well");
    assert!(
        well.pos.y >= canvas.pos.y + canvas.size.y,
        "the swatch sits below the canvas"
    );
    assert!(
        rect(&frame, "nl_0").size.y >= 20.0,
        "a node row is a real row"
    );
    let bottom = params.pos.y + params.size.y + 0.5;
    for kind in NodeKind::catalog() {
        insert(&mut bench, kind.id());
        let frame = walk(&mut bench);
        let cell = rect(&frame, &format!("cell_{}", kind.id()));
        let out = rect(&frame, "mtl_output_stage");
        assert!(cell.size.y > 20.0, "{}: the cell collapsed", kind.id());
        assert!(
            out.pos.y + out.size.y <= bottom,
            "{}: the output stage (bottom {}) overflows the column (bottom {bottom})",
            kind.id(),
            out.pos.y + out.size.y
        );
        let tree = frame_tree(&mut bench);
        let cell_node = find(&tree, &format!("cell_{}", kind.id())).unwrap();
        for n in nodes_of(cell_node) {
            if n.tab_group.is_empty() || n.id.is_empty() {
                continue;
            }
            let r = rect(&frame, &n.id);
            assert!(r.size.y >= 12.0, "{}: {} collapsed", kind.id(), n.id);
            assert!(
                r.pos.y + r.size.y <= cell.pos.y + cell.size.y + 0.5,
                "{} overflows",
                n.id
            );
        }
    }
}

/// THE NAV TOPOLOGY: the top-tier stops are the designer's panes, in authored order,
/// each entered at depth ≤ 1, and the stick moves geometrically between the columns.
#[test]
fn the_nav_topology_is_the_designer_surface() {
    use flicker_input_router::{nav_geometric, Focusable, NavDir};
    use std::collections::HashSet;

    let mut bench = MaterialBench::shipped();
    let tree = frame_tree(&mut bench);
    let all = nodes_of(&tree);
    let claimed: HashSet<&str> = all
        .iter()
        .filter(|n| !n.tab_group.is_empty())
        .map(|n| n.tab_group.as_str())
        .collect();
    let mut stops: Vec<&UiNode> = all
        .iter()
        .copied()
        .filter(|n| !n.id.is_empty() && n.tab_group.is_empty() && claimed.contains(n.id.as_str()))
        .collect();
    stops.sort_by_key(|n| n.nav_ordinal);
    let roster: Vec<(u32, &str)> = stops
        .iter()
        .map(|n| (n.nav_ordinal, n.id.as_str()))
        .collect();
    assert_eq!(
        roster,
        vec![
            (1, "mtl_top"),
            (2, "mtl_nodes"),
            (3, "mtl_view"),
            (4, "mtl_params"),
            (5, "mtl_footer"),
            (6, "mtl_nav"),
            (7, "mtl_mat_panel"),
        ]
    );
    for stop in &stops {
        for n in &all {
            if n.tab_group == stop.id {
                assert!(
                    !claimed.contains(n.id.as_str()),
                    "enter depth > 1 inside {}",
                    stop.id
                );
            }
        }
    }
    let frame = walk(&mut bench);
    let fo: Vec<Focusable> = stops
        .iter()
        .map(|n| Focusable {
            id: n.id.clone(),
            group: String::new(),
            ordinal: n.nav_ordinal,
            rect: frame
                .rect(&n.id)
                .map(|r| [r.pos.x, r.pos.y, r.size.x, r.size.y])
                .unwrap_or([0.0; 4]),
        })
        .collect();
    assert_eq!(
        nav_geometric(&fo, "mtl_nodes", NavDir::Right).as_deref(),
        Some("mtl_view")
    );
    assert_eq!(
        nav_geometric(&fo, "mtl_view", NavDir::Right).as_deref(),
        Some("mtl_params")
    );
    assert_eq!(
        nav_geometric(&fo, "mtl_params", NavDir::Left).as_deref(),
        Some("mtl_view")
    );
}

// ── the fix-up contracts: declined edits, the frame's start, the canvas's seat ──

/// The seat the walker reserves for the canvas on the reference screen.
fn canvas_seat(bench: &mut MaterialBench) -> SurfaceSlot {
    walk(bench)
        .surface(CANVAS_SLOT)
        .cloned()
        .expect("the walker reserves the canvas")
}

/// A walker sample on the canvas at `at`, with the left button `left` and a press edge
/// `pressed` — what the barrier hands a hot (or captured) canvas.
fn canvas_pointer(
    slot: &SurfaceSlot,
    at: Vec2,
    left: bool,
    pressed: bool,
    wheel: f32,
) -> SurfacePointer {
    SurfacePointer {
        id: CANVAS_SLOT.into(),
        root: false,
        cursor: at,
        local: at - Vec2::new(slot.x, slot.y),
        delta: Vec2::ZERO,
        left,
        right: false,
        pressed,
        wheel,
        captured: left,
        rect: Rect {
            pos: Vec2::new(slot.x, slot.y),
            size: Vec2::new(slot.w, slot.h),
        },
    }
}

/// MAJOR 2: "changed" means the STOPS changed — a hue on a grey ramp recolours nothing,
/// so it is no edit (it used to report one on every frame the control held it).
#[test]
fn a_hue_on_a_grey_ramp_is_no_edit() {
    let mut bench = on_new();
    select(&mut bench, "ramp");
    assert!(
        fire(&mut bench, "node_sat", 0.0),
        "greying the ramp is an edit"
    );
    let grey = bench.graph().clone();
    assert!(
        !fire(&mut bench, "node_hue", 0.6),
        "a hue on greys changes nothing"
    );
    assert_eq!(bench.graph(), &grey);
    assert!(!bench.designer.set_ramp_tint("ramp", Some(0.3), None));
}

/// MAJOR 2, the audit: every mutation reports CHANGED only when the document changed —
/// re-writing what it already holds is `false`, and leaves it exactly as it was.
#[test]
fn every_mutation_reports_no_change_for_what_it_holds() {
    let mut bench = on_new();
    let rgb = bench.palette.get(0).unwrap().rgb;
    let d = &mut bench.designer;
    let before = d.graph().clone();
    let relief = d.graph().output.relief as f64;
    assert!(!d.set_relief(relief));
    let Scalar::Const(rough) = d.graph().output.roughness else {
        panic!("New's roughness is a constant")
    };
    assert!(!d.set_level(OutPort::Roughness, rough as f64));
    assert!(!d.set_emit_strength(0.5), "no glow to strengthen");
    assert!(!d.set_emit_colour(None), "no glow to turn off");
    let (hue, sat) = match &d.graph().nodes[d.index_of("ramp").unwrap()].kind {
        NodeKind::Ramp { ramp } => ramp.tint(),
        _ => unreachable!(),
    };
    assert!(!d.set_ramp_tint("ramp", Some(hue as f64), Some(sat as f64)));
    let material = d.graph().material;
    assert!(!d.bind(material));
    assert!(!d.move_node("noise", 0));
    assert!(
        !d.unwire(&Sink::Output(OutPort::Metalness)),
        "nothing drives it"
    );
    assert_eq!(
        d.wire("noise", &Sink::Output(OutPort::Height)),
        Ok(false),
        "already wired"
    );
    assert!(!d.delete("nobody"));
    assert_eq!(d.graph(), &before, "none of it touched the document");
    assert!(d.set_emit_colour(Some(rgb)));
    assert!(!d.set_emit_colour(Some(rgb)), "the colour it wears");
    let glow = d.graph().output.emit.clone().unwrap();
    assert!(!d.set_palette_rgb(&glow.color, rgb));
    assert!(!d.set_emit_strength(glow.strength as f64));
}

/// MAJOR 3: a control whose edit the document DECLINED is handed back to the Model —
/// through the real walker: the hue slider on a grey ramp is pressed (the walker takes
/// local ownership of what the hand set), the dispatcher declines it, and the next walk
/// shows and echoes the Model's hue again instead of holding the declined value forever.
#[test]
fn a_declined_edit_hands_the_control_back_to_the_model() {
    load_strings();
    let mut bench = on_new();
    select(&mut bench, "ramp");
    assert!(fire(&mut bench, "node_sat", 0.0));
    let tree = frame_tree(&mut bench);
    let styles = styles(&bench);
    let model = bench.model();
    let hue = model
        .number("node_hue")
        .expect("the ramp's hue is published");
    let laid = run_ui(&tree, &model, &styles, &snap(), &mut bench.ui_state);
    let r = laid.rect("ramp_hue").expect("the hue slider is laid out");
    let value_w = match find(&tree, "ramp_hue").and_then(|n| n.props.get("value_w")) {
        Some(Value::Number(w)) => *w as f32,
        _ => 0.0,
    };
    let at = Vec2::new(
        r.pos.x + (r.size.x - value_w) * 0.7,
        r.pos.y + r.size.y * 0.5,
    );
    let input = |down: bool, clicked: bool| UiInput {
        mouse: at,
        clicked,
        down,
        ..snap()
    };
    run_ui(
        &tree,
        &model,
        &styles,
        &input(true, true),
        &mut bench.ui_state,
    );
    let up = run_ui(
        &tree,
        &model,
        &styles,
        &input(false, false),
        &mut bench.ui_state,
    );
    let pressed = up.results.number("node_hue").expect("the slider reports");
    assert!(
        (pressed - hue).abs() > 0.05,
        "the press moved the slider ({pressed} vs {hue})"
    );
    assert!(
        !route::apply(&mut bench, &model, &up.results),
        "a hue on greys is declined"
    );
    let after = run_ui(&tree, &bench.model(), &styles, &snap(), &mut bench.ui_state);
    assert_eq!(
        after.results.number("node_hue"),
        Some(hue),
        "the control is back on the Model's hue"
    );
}

/// MAJOR 4: a glow colour picked on a material with NO glow starts at the AUTHORED
/// strength (`params.new_glow_strength`): the strength slider's resting echo in that same
/// frame (it had no value to show) is not an edit, and neither is the mask's.
#[test]
fn a_new_glow_starts_at_the_authored_strength() {
    let mut bench = on_new();
    assert!(bench.graph().output.emit.is_none());
    let doc: serde_json::Value = serde_json::from_str(SW_SCENE).unwrap();
    let authored = doc["params"][NEW_GLOW_STRENGTH_PARAM]
        .as_f64()
        .expect("the scene authors the new glow's strength") as f32;
    // The frame as the walker builds it — every control's echo — plus the one pick, and
    // the disabled strength slider resting at its floor.
    let published = bench.model();
    let mut results = walk(&mut bench).results;
    results.set("out_emit_color", 3.0);
    results.set("out_emit_strength", 0.0);
    assert!(route::apply(&mut bench, &published, &results));
    let emit = bench.graph().output.emit.clone().expect("the glow exists");
    assert_eq!(
        emit.strength, authored,
        "the authored start, not a resting echo"
    );
    assert_eq!(emit.mask, None, "the mask select's echo wired nothing");
}

// ── the REAL channel: the bench's own walker state, the frame's Model, the walker's
//    own surface sample and popup answer — exactly what `update` runs ─────────────────

/// A bench ready to walk: its merged styles loaded and compiled, as `enter` does.
fn live(mut bench: MaterialBench) -> MaterialBench {
    bench.adopt_styles();
    bench
}

/// One frame on the REAL channel, as `update` runs it (less the signal router): the walk
/// on the bench's own UI state against the frame's Model, then the settle — dispatch
/// against that Model, the canvas driven with the walker's own sample and popup answer,
/// the canvas drawn.
fn step(bench: &mut MaterialBench, input: &UiInput) -> UiFrame {
    let tree = frame_tree(bench);
    let model = bench.model();
    let frame = bench.walk(&tree, &model, input);
    bench.settle(&model, &frame, &frame.results, false);
    frame
}

/// The pointer at `mouse`: a press edge, the button held, a wheel notch.
fn pointer_at(mouse: Vec2, clicked: bool, down: bool, wheel: f32) -> UiInput {
    UiInput {
        mouse,
        clicked,
        down,
        wheel,
        ..snap()
    }
}

/// A full click (press frame, release frame) at `mouse`; the press frame.
fn click(bench: &mut MaterialBench, mouse: Vec2) -> UiFrame {
    let pressed = step(bench, &pointer_at(mouse, true, true, 0.0));
    step(bench, &pointer_at(mouse, false, false, 0.0));
    pressed
}

fn centre(r: Rect) -> Vec2 {
    r.pos + r.size * 0.5
}

fn inside(outer: Rect, r: Rect) -> bool {
    r.pos.x >= outer.pos.x - 0.5
        && r.pos.y >= outer.pos.y - 0.5
        && r.pos.x + r.size.x <= outer.pos.x + outer.size.x + 0.5
        && r.pos.y + r.size.y <= outer.pos.y + outer.size.y + 0.5
}

/// BLOCKER (round 3), on the real channel: the panel around the canvas claims the
/// pointer (`hud_hit` is true all over the canvas) and that must NOT occlude it — a wheel
/// notch over the bare canvas zooms, a press on a card selects it; but a click on an open
/// header dropdown's row lying over the canvas picks the row and never reaches the canvas
/// (no link starts on the card right under it).
#[test]
fn the_canvas_takes_the_pointer_over_its_bare_seat_and_never_through_a_menu() {
    load_strings();
    let mut bench = live(on_new());
    step(&mut bench, &snap());
    let ramp = card(&mut bench, "ramp");
    let at = centre(bench.canvas.cards()[ramp]);
    let zoom = bench.canvas.zoom();
    let f = step(&mut bench, &pointer_at(at, false, false, 1.0));
    assert!(
        f.results.is_on("hud_hit"),
        "the canvas's panel claims the pointer"
    );
    assert_ne!(
        bench.canvas.zoom(),
        zoom,
        "a wheel notch over the bare canvas zooms"
    );
    select(&mut bench, "noise");
    step(&mut bench, &snap());
    let card_ix = card(&mut bench, "ramp");
    let at = centre(bench.canvas.cards()[card_ix]);
    click(&mut bench, at);
    assert_eq!(
        bench.designer.selected_key(),
        Some("ramp"),
        "a press on a card over the bare canvas selects it"
    );

    // The material dropdown, open, its menu hanging over the canvas.
    let f = step(&mut bench, &snap());
    let field = f.rect("material_select").expect("the dropdown is laid out");
    let seat = f.rect(CANVAS_SLOT).expect("the canvas is laid out");
    click(&mut bench, centre(field));
    let f = step(&mut bench, &snap());
    let [mx, my, mw, mh] = f.popup.expect("the dropdown opened");
    let x = centre(field).x;
    let (top, bottom) = (my.max(seat.pos.y), (my + mh).min(seat.pos.y + seat.size.y));
    assert!(
        x >= seat.pos.x
            && x <= seat.pos.x + seat.size.x
            && x >= mx
            && x <= mx + mw
            && top + 40.0 < bottom,
        "the menu hangs over the canvas"
    );
    let y = (top + bottom) * 0.5;
    // A card right under that point, and the Link tool armed: a press that reached the
    // canvas there would start a link.
    let m = *bench.canvas.metrics();
    let local = bench
        .canvas
        .view()
        .to_local(bench.canvas.area(), Vec2::new(x, y))
        - m.card * 0.5;
    bench.canvas.place("ramp", local);
    bench.canvas_mode = CanvasMode::Link;
    step(&mut bench, &snap());
    let ramp = card(&mut bench, "ramp");
    assert!(
        bench.canvas.cards()[ramp].pos.x <= x,
        "the ramp card lies under the menu row"
    );
    let selected = bench.designer.selected_key().map(str::to_string);
    let f = step(&mut bench, &pointer_at(Vec2::new(x, y), true, true, 0.0));
    assert!(f.popup_hit, "the menu took the click");
    assert_eq!(
        bench.canvas.linking(),
        None,
        "no link started under the menu"
    );
    step(&mut bench, &pointer_at(Vec2::new(x, y), false, false, 0.0));
    assert!(
        bench.graph().material.is_some(),
        "the row under the click was picked"
    );
    assert_eq!(bench.designer.selected_key().map(str::to_string), selected);
}

/// BLOCKER (round 3), on the real scene: an open dropdown's rows are the dropdown's. With
/// the output stage at its tallest (every scalar port a constant, two more nodes in the
/// list) the glow-mask select opens UPWARD over the selected noise node's sliders: each of
/// its rows, clicked, picks that row and moves nothing else in the document; and the
/// material dropdown's rows lying over the view tabs never switch the view.
#[test]
fn an_open_dropdowns_rows_never_click_what_lies_under_them() {
    load_strings();
    let mut bench = live(MaterialBench::shipped());
    assert!(
        bench.load_or_ask(rack_granite(), None, None),
        "Granite loads"
    );
    for src in ["out_rough_src", "out_metal_src", "out_ao_src"] {
        fire(&mut bench, src, 0.0);
    }
    insert(&mut bench, "levels");
    insert(&mut bench, "blur");
    select(&mut bench, "ch1");
    assert!(
        bench.graph().output.emit.is_some(),
        "Granite glows, so its mask is live"
    );
    let f = step(&mut bench, &snap());
    let field = f
        .rect("out_emit_mask")
        .expect("the mask select is laid out");
    let candidates: Vec<Option<String>> = std::iter::once(None)
        .chain(
            bench
                .designer
                .candidates(&Sink::Output(OutPort::EmitMask))
                .into_iter()
                .map(|k| Some(k.to_string())),
        )
        .collect();
    let mut over_a_control = 0;
    for (k, want) in candidates.iter().enumerate() {
        click(&mut bench, centre(field));
        let f = step(&mut bench, &snap());
        let [_, my, _, mh] = f.popup.expect("the mask menu opened");
        assert!(my + mh <= field.pos.y, "it opens upward, over the column");
        let y = my + mh * (k as f32 + 0.5) / candidates.len() as f32;
        let x = centre(field).x;
        over_a_control += f
            .rects
            .iter()
            .filter(|(id, r)| {
                id.starts_with("noise_")
                    && x >= r[0]
                    && x <= r[0] + r[2]
                    && y >= r[1]
                    && y <= r[1] + r[3]
            })
            .count();
        let before = bench.graph().clone();
        click(&mut bench, Vec2::new(x, y));
        let after = bench.graph().clone();
        assert_eq!(
            after.output.emit.as_ref().and_then(|e| e.mask.clone()),
            *want,
            "row {k} picks its own candidate"
        );
        let (mut a, mut b) = (before, after);
        for g in [&mut a, &mut b] {
            if let Some(e) = g.output.emit.as_mut() {
                e.mask = None;
            }
        }
        assert_eq!(a, b, "row {k}: nothing under it moved");
    }
    assert!(
        over_a_control > 0,
        "some rows lay over the noise node's controls"
    );

    // The material dropdown over the view tabs.
    let f = step(&mut bench, &snap());
    let field = f.rect("material_select").expect("the dropdown is laid out");
    let tabs = f.rect("mtl_views").expect("the view tabs are laid out");
    click(&mut bench, centre(field));
    let f = step(&mut bench, &snap());
    let [_, my, _, mh] = f.popup.expect("the dropdown opened");
    let y = centre(tabs).y;
    assert!(y > my && y < my + mh, "a row lies over the tabs");
    let view = bench.sel_map;
    click(&mut bench, Vec2::new(centre(field).x, y));
    assert_eq!(
        bench.sel_map, view,
        "the view did not switch under the menu"
    );
    assert!(bench.graph().material.is_some(), "the row was picked");
}

/// MINOR 3 (round 3), and the final polish: a loaded graph opens LEGIBLY — never zoomed
/// out past the authored floor — and framed on its OUTPUT card, the result being what the
/// author reads first: Granite, which fits whole only far below the floor, opens at the
/// floor with the Output card on the seat; a small graph is still framed whole.
#[test]
fn a_loaded_graph_opens_at_a_readable_zoom_on_its_output() {
    load_strings();
    let doc: serde_json::Value = serde_json::from_str(SW_SCENE).unwrap();
    let floor = doc["params"][CANVAS_FIT_FLOOR_PARAM]
        .as_f64()
        .expect("the scene authors the canvas's readable floor") as f32;
    let mut bench = live(MaterialBench::shipped());
    let f = step(&mut bench, &snap());
    let seat = f.rect(CANVAS_SLOT).unwrap();
    assert!(
        (bench.canvas.zoom() - floor).abs() < 1e-4,
        "Granite opened at {} (floor {floor})",
        bench.canvas.zoom()
    );
    let out = output_card(&mut bench);
    let r = bench.canvas.cards()[out];
    assert!(inside(seat, r), "the Output card is on the seat");
    let mid = centre(seat);
    assert!(
        (centre(r) - mid).length() < 1.0,
        "and framed in the middle of it"
    );
    fire(&mut bench, "sel_preset", 0.0);
    step(&mut bench, &snap());
    assert!(
        bench.canvas.cards().iter().all(|r| inside(seat, *r)),
        "New is framed whole"
    );
}

/// The final polish: a node picked in the node LIST is brought into view — its card may
/// lie anywhere on the canvas (Granite at the floor shows only part of it) — by a pan,
/// never a zoom.
#[test]
fn a_node_picked_in_the_list_is_brought_into_view() {
    load_strings();
    let mut bench = live(MaterialBench::shipped());
    let f = step(&mut bench, &snap());
    let seat = f.rect(CANVAS_SLOT).unwrap();
    let off_seat: Vec<String> = bench
        .canvas_content
        .keys
        .iter()
        .zip(bench.canvas.cards())
        .filter(|(_, r)| !inside(seat, **r))
        .map(|(k, _)| k.clone())
        .collect();
    let key = off_seat
        .first()
        .expect("at the floor, part of Granite lies off the seat")
        .clone();
    let zoom = bench.canvas.zoom();
    select(&mut bench, &key);
    step(&mut bench, &snap());
    let card_ix = card(&mut bench, &key);
    let r = bench.canvas.cards()[card_ix];
    assert!(inside(seat, r), "{key}'s card was brought onto the seat");
    assert_eq!(bench.canvas.zoom(), zoom, "by a pan, never a zoom");
}

/// MINOR 3 (round 3): a node inserted after the selection lands ON THE SEAT — beside its
/// anchor where there is room, in view either way — whether the anchor sits mid-graph,
/// at an edge, or in a small graph.
#[test]
fn an_inserted_card_lands_on_the_seat() {
    load_strings();
    let new = MaterialBench::shipped().presets[0].clone();
    for (graph, anchor) in [
        (rack_granite(), "ch1"),
        (rack_granite(), "ao"),
        (new, "noise"),
    ] {
        let preset = graph.id.clone();
        let mut bench = live(MaterialBench::shipped());
        assert!(bench.load_or_ask(graph, None, None), "{preset} loads");
        step(&mut bench, &snap());
        select(&mut bench, anchor);
        let key = insert(&mut bench, "levels");
        let f = step(&mut bench, &snap());
        let seat = f.rect(CANVAS_SLOT).unwrap();
        let card_ix = card(&mut bench, &key);
        let r = bench.canvas.cards()[card_ix];
        assert!(
            inside(seat, r),
            "preset {preset}, after {anchor}: {key} at {:?}+{:?} is off the seat {:?}",
            r.pos,
            r.size,
            seat
        );
    }
}

/// MINOR 7: the dropdown's ECHO never rebinds — a graph bound to a slot the index does
/// not list shows the placeholder, its echo leaves it bound, and Unbound is still a real
/// pick from there.
#[test]
fn an_unlisted_binding_survives_the_dropdowns_echo() {
    let mut bench = MaterialBench::shipped();
    let unlisted = unlisted_slot(&bench);
    bench.designer.bind(Some(unlisted));
    let published = bench.model();
    assert_eq!(published.number("sel_material"), Some(-1.0));
    let echo = walk(&mut bench).results;
    assert_eq!(
        echo.number("sel_material"),
        Some(-1.0),
        "the walker echoes it"
    );
    route::apply(&mut bench, &published, &echo);
    assert_eq!(
        bench.graph().material,
        Some(unlisted),
        "the echo unbound the graph"
    );
    fire(&mut bench, "sel_material", 0.0);
    assert_eq!(bench.graph().material, None, "Unbound is a pick");
}

/// MINOR 8: nothing the bench PUBLISHES carries a `$` except as a whole token the draw
/// boundary resolves — the recipe line's "Unbound" is resolved copy, never `$mtl_unbound`
/// mid-line — and a walked frame draws no `$` anywhere.
#[test]
fn no_token_is_composed_into_published_text() {
    load_strings();
    let mut bench = on_new();
    bench.designer.bind(None);
    let model = bench.model();
    let line = model.text("recipe_line").unwrap();
    assert!(!line.contains('$'), "{line}");
    assert!(line.contains(strings::resolve("$mtl_unbound").as_ref()));
    for (key, v) in model.entries() {
        let Value::Text(t) = v else { continue };
        if t.contains('$') {
            assert!(
                t.starts_with('$')
                    && !t.contains(char::is_whitespace)
                    && strings::resolve(t).as_ref() != t.as_str(),
                "{key} = {t:?} composes a token into text"
            );
        }
    }
    assert!(unresolved_tokens(&walk(&mut bench).commands).is_empty());
}

/// MINOR 9: the status line speaks a stringtable TOKEN per kind of graph error — never
/// the evaluator's English — with the node, port or parameter it names beside it as raw
/// data; a walked frame with a problem draws the localized copy and the data, no `$`.
#[test]
fn the_status_line_speaks_tokens_never_rust_english() {
    load_strings();
    for e in every_graph_error() {
        let (token, _) = designer::error_parts(&e);
        assert!(token.starts_with("$mtl_err_"), "{token}");
        assert_ne!(
            strings::resolve(token).as_ref(),
            token,
            "{token} has no row"
        );
    }
    let mut bench = on_new();
    insert(&mut bench, "levels"); // an unwired filter: a problem
    let problem = bench
        .designer
        .problems()
        .first()
        .cloned()
        .expect("a problem");
    let (token, detail) = bench.status_line();
    let (want_token, want_detail) = designer::error_parts(&problem);
    assert_eq!(
        (token.as_str(), detail.as_str()),
        (want_token, want_detail.as_str())
    );
    let drawn: Vec<String> = walk(&mut bench)
        .commands
        .iter()
        .filter_map(|c| match c {
            HudCommand::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let copy = strings::resolve(&token).into_owned();
    assert!(drawn.contains(&copy), "the localized copy is drawn");
    assert!(drawn.contains(&detail), "its specifics are drawn as data");
    let english = problem.to_string();
    assert!(
        !drawn.iter().any(|t| t.contains(&english)),
        "the evaluator's English reached the screen"
    );
    assert!(!drawn.iter().any(|t| t.contains('$')));
}

/// MINOR 10: a link dropped on a card's BODY never replaces a wire — with every port
/// that takes its kind of signal already wired it is refused with its notice; dropped ON
/// a port it rewires that port (how a wire is replaced on purpose).
#[test]
fn a_body_drop_never_replaces_a_wire() {
    let mut bench = on_new();
    let levels = insert(&mut bench, "levels");
    assert!(fire(&mut bench, "node_in0", 1.0), "levels ← noise");
    let blend = insert(&mut bench, "blend");
    for port in 0..3 {
        let (n, b) = (card(&mut bench, "noise"), card(&mut bench, &blend));
        assert!(
            link(&mut bench, n, b, Some(port)),
            "noise → blend port {port}"
        );
    }
    let before = bench.graph().clone();
    let (l, b) = (card(&mut bench, &levels), card(&mut bench, &blend));
    assert!(!link(&mut bench, l, b, None), "every field port is wired");
    assert_eq!(bench.graph(), &before, "no wire was replaced");
    assert_eq!(bench.status_line().0, designer::WIRE_NO_FREE_PORT);
    assert!(
        link(&mut bench, l, b, Some(1)),
        "onto a port, it rewires it"
    );
    assert_eq!(
        bench.designer.source_of(&Sink::Input {
            node: blend.clone(),
            port: 1
        }),
        Some(levels.as_str())
    );
}

/// MINOR 11: the arrangement stays put — every card is pinned where it first lay, a node
/// inserted after the selection lands BESIDE it with every other card where it was, a
/// deleted node's placement goes, and a fresh key is never one this session has used.
#[test]
fn inserting_a_node_keeps_every_card_where_it_was() {
    load_strings();
    let mut bench = on_new();
    let slot = canvas_seat(&mut bench);
    bench.drive_canvas(Some(&slot), None, false, false);
    let keys = bench.canvas_content.keys.clone();
    let before: Vec<Option<Vec2>> = keys.iter().map(|k| bench.canvas.placement(k)).collect();
    assert!(
        before.iter().all(Option::is_some),
        "every card is pinned on its first layout"
    );
    select(&mut bench, "noise");
    let fresh = insert(&mut bench, "levels");
    bench.drive_canvas(Some(&slot), None, false, false);
    for (k, p) in keys.iter().zip(&before) {
        assert_eq!(
            bench.canvas.placement(k),
            *p,
            "{k} moved when a node was inserted"
        );
    }
    let m = *bench.canvas.metrics();
    let anchor = bench.canvas.placement("noise").unwrap();
    let at = bench
        .canvas
        .placement(&fresh)
        .expect("the new card is placed");
    assert_eq!(
        at.x,
        anchor.x + m.card.x + m.gap.x,
        "beside the node it was inserted after"
    );
    assert!(at.y >= anchor.y);
    assert!(fire(&mut bench, "node_delete", true));
    bench.sync_canvas();
    assert_eq!(
        bench.canvas.placement(&fresh),
        None,
        "a deleted card's placement is pruned"
    );
    select(&mut bench, "noise");
    let again = insert(&mut bench, "levels");
    assert_ne!(again, fresh, "a fresh key is never one this session used");
}

/// MINOR 12: a link released OUTSIDE the canvas's seat is abandoned — even over a card a
/// pan pushed past the seat's edge (clipped from view, so no drop target), and on a
/// release frame the walker hands no sample for at all.
#[test]
fn a_link_released_outside_the_seat_is_abandoned() {
    load_strings();
    let mut bench = on_new();
    let levels = insert(&mut bench, "levels");
    let slot = canvas_seat(&mut bench);
    bench.canvas_mode = CanvasMode::Link;
    bench.drive_canvas(Some(&slot), None, false, false);
    bench.canvas.place(&levels, Vec2::new(-4000.0, 0.0));
    bench.drive_canvas(Some(&slot), None, false, false);
    let li = card(&mut bench, &levels);
    let off = bench.canvas.cards()[li];
    let off_at = off.pos + off.size * 0.5;
    assert!(off_at.x < slot.x, "the levels card lies off the seat");
    let ni = card(&mut bench, "noise");
    let n = bench.canvas.cards()[ni];
    let on_noise = n.pos + n.size * 0.5;
    let before = bench.graph().clone();
    for released in [Some(off_at), None] {
        let press = canvas_pointer(&slot, on_noise, true, true, 0.0);
        bench.drive_canvas(Some(&slot), Some(&press), false, false);
        let drag = canvas_pointer(&slot, off_at, true, false, 0.0);
        bench.drive_canvas(Some(&slot), Some(&drag), false, false);
        let up = released.map(|at| canvas_pointer(&slot, at, false, false, 0.0));
        assert!(!bench.drive_canvas(Some(&slot), up.as_ref(), false, false));
        assert_eq!(bench.graph(), &before, "nothing was wired off the seat");
    }
    // Released ON the seat, over the card, the same drag wires.
    bench.canvas.place(
        &levels,
        bench.canvas.placement("noise").unwrap() + Vec2::new(0.0, 200.0),
    );
    bench.drive_canvas(Some(&slot), None, false, false);
    let li = card(&mut bench, &levels);
    let l = bench.canvas.cards()[li];
    let on_levels = l.pos + l.size * 0.25;
    bench.drive_canvas(
        Some(&slot),
        Some(&canvas_pointer(&slot, on_noise, true, true, 0.0)),
        false,
        false,
    );
    bench.drive_canvas(
        Some(&slot),
        Some(&canvas_pointer(&slot, on_levels, true, false, 0.0)),
        false,
        false,
    );
    assert!(bench.drive_canvas(
        Some(&slot),
        Some(&canvas_pointer(&slot, on_levels, false, false, 0.0)),
        false,
        false
    ));
    assert_eq!(
        bench.designer.source_of(&Sink::Input {
            node: levels.clone(),
            port: 0
        }),
        Some("noise")
    );
}

/// MINOR 13: an edge ends on its SINK PORT's dot, so two wires between the same two
/// cards are two lines, each the one picked at its own port; the output card names its
/// ports from the one table, a node card names none.
#[test]
fn two_wires_between_the_same_cards_pick_apart_at_their_ports() {
    load_strings();
    let mut bench = on_new();
    let blend = insert(&mut bench, "blend");
    for port in 0..2 {
        let (n, b) = (card(&mut bench, "noise"), card(&mut bench, &blend));
        assert!(link(&mut bench, n, b, Some(port)));
    }
    let slot = canvas_seat(&mut bench);
    bench.drive_canvas(Some(&slot), None, false, false);
    let b = card(&mut bench, &blend);
    for port in 0..2 {
        let edge = bench
            .canvas_content
            .sinks
            .iter()
            .position(|s| {
                *s == Sink::Input {
                    node: blend.clone(),
                    port,
                }
            })
            .expect("the wire is drawn");
        let canvas = &bench.canvas;
        let dot = with_nodes(&bench.canvas_content, |nodes| {
            canvas.port_center(b, port, nodes)
        })
        .expect("the port has a dot");
        assert_eq!(
            canvas.edge_at(dot),
            Some(edge),
            "port {port} picks its own wire"
        );
    }
    let out = output_card(&mut bench);
    let labels = &bench.canvas_content.port_labels;
    assert_eq!(labels[out].len(), OutPort::ALL.len());
    assert!(labels[out]
        .iter()
        .all(|l| !l.is_empty() && !l.contains('$')));
    assert!(
        labels[b].is_empty(),
        "a node card's ports are named in its cell"
    );
}

/// MINOR 14: a loaded graph is FRAMED on the seat (every card inside it), and Reset view
/// frames it again after a pan — keeping the author's arrangement.
#[test]
fn a_loaded_graph_is_framed_and_reset_view_frames_it_again() {
    load_strings();
    let mut bench = on_new();
    for kind in ["levels", "blend", "ramp", "warp", "tile", "invert"] {
        insert(&mut bench, kind);
    }
    let slot = canvas_seat(&mut bench);
    let seat = Rect {
        pos: Vec2::new(slot.x, slot.y),
        size: Vec2::new(slot.w, slot.h),
    };
    let framed = |bench: &MaterialBench| {
        bench.canvas.cards().iter().all(|c| {
            c.pos.x >= seat.pos.x - 0.5
                && c.pos.y >= seat.pos.y - 0.5
                && c.pos.x + c.size.x <= seat.pos.x + seat.size.x + 0.5
                && c.pos.y + c.size.y <= seat.pos.y + seat.size.y + 0.5
        })
    };
    fire(&mut bench, "canvas_reset", true);
    bench.drive_canvas(Some(&slot), None, false, false);
    assert!(framed(&bench), "Reset view frames every card");
    // A middle-drag pans most of a seat's width.
    let (a, z) = (
        Vec2::new(seat.pos.x + 4.0, seat.pos.y + 4.0),
        Vec2::new(seat.pos.x + seat.size.x - 4.0, seat.pos.y + 4.0),
    );
    bench.drive_canvas(
        Some(&slot),
        Some(&canvas_pointer(&slot, a, false, false, 0.0)),
        true,
        false,
    );
    bench.drive_canvas(
        Some(&slot),
        Some(&canvas_pointer(&slot, z, false, false, 0.0)),
        true,
        false,
    );
    bench.draw_canvas(Some(&slot)); // the frame's draw lays the panned view out
    assert!(!framed(&bench), "the pan pushed cards off the seat");
    let keys = bench.canvas_content.keys.clone();
    let arranged: Vec<Option<Vec2>> = keys.iter().map(|k| bench.canvas.placement(k)).collect();
    assert!(!fire(&mut bench, "canvas_reset", true), "a view change");
    bench.drive_canvas(Some(&slot), None, false, false);
    assert!(framed(&bench), "Reset view frames it again");
    let kept: Vec<Option<Vec2>> = keys.iter().map(|k| bench.canvas.placement(k)).collect();
    assert_eq!(kept, arranged, "the arrangement is the author's and stays");
    // A load is a new workspace, framed on its first seated frame.
    fire(&mut bench, "sel_preset", 0.0);
    bench.drive_canvas(Some(&slot), None, false, false);
    assert!(framed(&bench), "a loaded graph is framed");
}

/// MINOR 15: the output card's ports ARE flicker-texture's one output table — the kind
/// the designer offers each port and the order the card draws them read `Output::ports()`,
/// the table the validator checks every output wire against.
#[test]
fn the_output_ports_are_the_one_table() {
    let table = flicker_texture::Output::ports();
    assert_eq!(OutPort::ALL.len(), table.len());
    for (p, row) in OutPort::ALL.iter().zip(table) {
        assert_eq!(p.table(), *row);
        assert_eq!(p.takes(), row.kind);
    }
    let bench = on_new();
    for p in OutPort::ALL {
        for key in bench.designer.candidates(&Sink::Output(p)) {
            let i = bench.designer.index_of(key).unwrap();
            assert_eq!(
                bench.graph().nodes[i].kind.output(),
                p.takes(),
                "{key} offered to {}",
                p.table().name
            );
        }
    }
}

/// MINOR 16: a glow colour never recolours a palette node something ELSE reads — the
/// glow gets a fresh palette node of its own and the base colour keeps its colour; a glow
/// that is its node's only reader is rewritten in place.
#[test]
fn a_glow_colour_never_recolours_a_shared_palette_node() {
    let mut bench = on_new();
    let pal = insert(&mut bench, "palette");
    let first = bench.palette.get(0).unwrap().rgb;
    assert!(bench.designer.set_palette_rgb(&pal, first));
    let base = bench
        .designer
        .candidates(&Sink::Output(OutPort::Base))
        .iter()
        .position(|k| *k == pal)
        .unwrap();
    assert!(
        fire(&mut bench, "out_base", (base + 1) as f64),
        "base ← palette"
    );
    assert_eq!(
        bench.designer.wire(&pal, &Sink::Output(OutPort::EmitColor)),
        Ok(true),
        "and the glow reads it too"
    );
    let nodes = bench.graph().nodes.len();
    let other = bench.palette.get(3).unwrap().rgb;
    assert!(fire(&mut bench, "out_emit_color", 4.0), "palette entry 3");
    let g = bench.graph();
    assert_eq!(
        g.nodes[bench.designer.index_of(&pal).unwrap()].kind,
        NodeKind::Palette { rgb: first },
        "the base colour kept its colour"
    );
    assert_eq!(g.output.base.as_deref(), Some(pal.as_str()));
    let glow = g.output.emit.as_ref().unwrap().color.clone();
    assert_ne!(glow, pal, "the glow has a node of its own");
    assert_eq!(
        g.nodes[bench.designer.index_of(&glow).unwrap()].kind,
        NodeKind::Palette { rgb: other }
    );
    assert_eq!(g.nodes.len(), nodes + 1);
    let nodes = g.nodes.len();
    assert!(fire(&mut bench, "out_emit_color", 2.0));
    assert_eq!(
        bench.graph().nodes.len(),
        nodes,
        "its own node, rewritten in place"
    );
    consistent(&bench);
}

/// MINOR 17: a value that is finite as an f64 but not as the node's f32 (1e39 in a band's
/// `at`) is REFUSED, the node untouched — the document never stores an infinity.
#[test]
fn an_f32_overflow_is_refused() {
    let mut bench = on_new();
    let band = insert(&mut bench, "band");
    let before = bench.graph().clone();
    assert!(!fire(&mut bench, "node_at", 1e39));
    assert!(!bench.designer.set_param(&band, "at", 1e39));
    assert!(!bench.designer.set_param(&band, "at", -1e39));
    assert_eq!(bench.graph(), &before);
    assert!(
        bench.designer.set_param(&band, "at", 0.25),
        "a finite value still lands"
    );
    consistent(&bench);
}

/// MINOR 18: the preset select shows the preset only while the document IS it — after
/// the first edit it shows the placeholder, so picking that preset again reloads it; the
/// bumpers still step from the one loaded last.
#[test]
fn an_edited_preset_reloads_when_picked_again() {
    let mut bench = MaterialBench::shipped();
    assert!(fire(&mut bench, "sel_preset", 2.0));
    assert_eq!(bench.model().number("sel_preset"), Some(2.0));
    assert!(fire(&mut bench, "reseed", true));
    assert_eq!(
        bench.model().number("sel_preset"),
        Some(-1.0),
        "edited, it is no longer the preset"
    );
    // Over that unsaved edit the re-pick asks first; its Discard reloads.
    assert!(
        !fire(&mut bench, "sel_preset", 2.0),
        "it asks over the edit"
    );
    bench.prompt_armed = false;
    bench.modal_closed(PROMPT_TREE, ASK_REPLACE, None);
    assert_eq!(bench.graph(), &bench.presets[2], "picking it again reloads");
    assert_eq!(bench.model().number("sel_preset"), Some(2.0));
    fire(&mut bench, "reseed", true);
    fire(&mut bench, "patch_next", true);
    bench.prompt_armed = false;
    bench.modal_closed(PROMPT_TREE, ASK_REPLACE, None);
    assert_eq!(bench.preset, Some(3), "the bumpers step from the last load");
}

/// MINOR 18: unwiring a scalar port returns it to the constant it held before the field
/// was wired in — not the crate's fresh default — whether unwired by its select or by
/// deleting the field that drove it.
#[test]
fn unwiring_a_scalar_port_restores_its_constant() {
    let mut bench = on_new();
    assert!(fire(&mut bench, "out_rough", 0.3));
    assert!(fire(&mut bench, "out_rough_src", 1.0), "roughness ← noise");
    assert!(fire(&mut bench, "out_rough_src", 0.0), "back to a constant");
    assert_eq!(bench.graph().output.roughness, Scalar::Const(0.3));
    assert!(fire(&mut bench, "out_rough", 0.6));
    assert!(fire(&mut bench, "out_rough_src", 1.0));
    select(&mut bench, "noise");
    assert!(fire(&mut bench, "node_delete", true));
    assert_eq!(bench.graph().output.roughness, Scalar::Const(0.6));
}

// ── the Lit view's zoom (Aaron 2026-10-23: "zoom in on the sphere view") ────────

/// A bench on the LIT view — its styles loaded, one frame walked on the real channel, so the
/// sample is seated and the zoom slider laid out — and that frame.
fn on_the_lit_view() -> (MaterialBench, UiFrame) {
    load_strings();
    let mut bench = live(MaterialBench::shipped());
    fire(&mut bench, "sel_map", MAP_IDS.len() as f64);
    assert!(bench.showing_lit(), "the Lit view shows");
    let frame = step(&mut bench, &snap());
    (bench, frame)
}

/// The zoom slider's numbers as the RAW scene file authors them.
fn authored_zoom() -> ZoomDial {
    fn node<'v>(v: &'v serde_json::Value, id: &str) -> Option<&'v serde_json::Value> {
        if v.get("id").and_then(serde_json::Value::as_str) == Some(id) {
            return Some(v);
        }
        v.get("children")?
            .as_array()?
            .iter()
            .find_map(|kid| node(kid, id))
    }
    let doc: serde_json::Value = serde_json::from_str(SW_SCENE).expect("the scene parses");
    let slider = node(&doc["tree"], LIT_ZOOM).expect("the scene authors the zoom slider");
    let num = |key: &str| {
        slider[key]
            .as_f64()
            .unwrap_or_else(|| panic!("the zoom slider authors `{key}`")) as f32
    };
    ZoomDial {
        min: num("min"),
        max: num("max"),
        step: num("step"),
    }
}

/// THE SLIDER ZOOMS THE LIT SAMPLE, through the real channel: the bench's dial IS the
/// authored slider's own range and step (no param repeats them); the slider is laid out with
/// the Lit view, its caption resolved; a press on its rail's far end and the release — the
/// walker's drag, committed on release — zoom the sample to the top of the range, a press
/// mid-rail to the middle (snapped to the step), and the Model echoes each. An idle frame's
/// echo is never an edit, a value past either end clamps, and no zoom is an image edit (no
/// bake is asked for).
#[test]
fn the_zoom_slider_moves_the_lit_zoom_and_the_model_echoes_it() {
    let (mut bench, _) = on_the_lit_view();
    let dial = authored_zoom();
    assert_eq!(bench.zoom, dial, "the dial is the authored slider's own");
    assert_eq!(
        bench.sample.zoom, 1.0,
        "the sample opens on the stage's framing"
    );
    let frame = walk(&mut bench);
    let slider = frame
        .rect(LIT_ZOOM)
        .expect("the zoom slider is laid out with the Lit view");
    let seat = frame.rect(LIT_SLOT).expect("the lit sample is seated");
    assert!(
        slider.size.x > 100.0 && slider.size.y >= 28.0,
        "a real slider: {slider:?}"
    );
    assert!(
        slider.pos.y >= seat.pos.y + seat.size.y,
        "it sits under the sample"
    );
    assert!(unresolved_tokens(&frame.commands).is_empty());
    let caption = strings::resolve("$mtl_lit_zoom").into_owned();
    assert!(
        frame
            .commands
            .iter()
            .any(|c| matches!(c, HudCommand::Text { text, .. } if *text == caption)),
        "the slider draws its caption `{caption}`"
    );

    let generation = bench.generation;
    // The rail is the slider less its readout column (`value_w` 46), its caption above it.
    let rail_y = slider.pos.y + slider.size.y * 0.75;
    let rail_end = slider.pos.x + slider.size.x - 46.0 - 1.0;
    click(&mut bench, Vec2::new(rail_end, rail_y));
    assert_eq!(
        bench.sample.zoom, dial.max,
        "the far end is the closest framing"
    );
    assert_eq!(bench.model().number(LIT_ZOOM), Some(f64::from(dial.max)));
    let mid = slider.pos.x + (slider.size.x - 46.0) * 0.5;
    click(&mut bench, Vec2::new(mid, rail_y));
    let middle = dial.min + (dial.max - dial.min) * 0.5;
    assert_eq!(bench.sample.zoom, middle, "mid-rail, snapped to the step");
    for _ in 0..3 {
        step(&mut bench, &snap());
    }
    assert_eq!(
        bench.sample.zoom, middle,
        "the slider's echo is never an edit"
    );
    assert_eq!(bench.model().number(LIT_ZOOM), Some(f64::from(middle)));

    assert!(!fire(&mut bench, LIT_ZOOM, 99.0), "a zoom is no image edit");
    assert_eq!(bench.sample.zoom, dial.max, "clamped at the top");
    assert!(!fire(&mut bench, LIT_ZOOM, -5.0));
    assert_eq!(bench.sample.zoom, dial.min, "clamped at the bottom");
    assert_eq!(bench.generation, generation, "no zoom asked for a bake");
}

/// The Model hands the zoom slider a NUMBER — the one representation of the zoom — on every
/// page and view, so the slider never falls back to its resting default.
#[test]
fn the_lit_zoom_is_published_as_a_number() {
    let mut bench = MaterialBench::shipped();
    for view in [0, MAP_IDS.len()] {
        fire(&mut bench, "sel_map", view as f64);
        assert_eq!(
            bench.model().get(LIT_ZOOM),
            Some(&Value::Number(f64::from(bench.sample.zoom))),
            "view {view}"
        );
    }
}

/// THE WHEEL OVER THE LIT SEAT ZOOMS, through the real channel: one notch is one AUTHORED
/// step (the slider's own), notches add, the zoom stops at either end of the authored range,
/// the slider follows through the next frame's Model (and its echo never undoes a notch),
/// and no notch asks for a bake.
#[test]
fn a_wheel_notch_over_the_lit_seat_zooms_by_the_authored_step_and_clamps() {
    let (mut bench, frame) = on_the_lit_view();
    let dial = authored_zoom();
    let at = centre(frame.rect(LIT_SLOT).expect("the lit sample is seated"));
    let generation = bench.generation;
    let notch = |bench: &mut MaterialBench, wheel: f32| {
        step(bench, &pointer_at(at, false, false, wheel));
    };
    notch(&mut bench, 1.0);
    assert_eq!(bench.sample.zoom, 1.0 + dial.step, "one notch, one step");
    notch(&mut bench, 2.0);
    assert_eq!(bench.sample.zoom, 1.0 + 3.0 * dial.step, "notches add");
    notch(&mut bench, 0.0);
    assert_eq!(
        bench.model().number(LIT_ZOOM),
        Some(f64::from(1.0 + 3.0 * dial.step)),
        "the slider follows through the Model"
    );
    assert_eq!(
        bench.sample.zoom,
        1.0 + 3.0 * dial.step,
        "and its echo is no edit"
    );
    for _ in 0..64 {
        notch(&mut bench, 1.0);
    }
    assert_eq!(bench.sample.zoom, dial.max, "clamped at the closest");
    notch(&mut bench, -1.0);
    assert_eq!(bench.sample.zoom, dial.max - dial.step, "and back one step");
    for _ in 0..64 {
        notch(&mut bench, -1.0);
    }
    assert_eq!(bench.sample.zoom, dial.min, "clamped at the farthest");
    assert_eq!(bench.generation, generation, "no notch asked for a bake");
}

/// THE WHEEL IS THE SEAT'S ALONE, on the real channel: over the graph canvas a notch zooms
/// the graph and never the sample; over the zoom slider beside the seat it zooms nothing;
/// under an open dropdown whose menu hangs over the seat the menu owns the pointer and no
/// notch reaches the sample; and a frame whose click an open menu spent (`popup_hit`) gives
/// the seat's own notch to nobody — where the same notch, no menu, zooms.
#[test]
fn the_lit_zoom_wheel_never_reaches_through_a_menu_or_off_its_seat() {
    let (mut bench, frame) = on_the_lit_view();
    let seat = frame.rect(LIT_SLOT).expect("the lit sample is seated");
    let canvas = frame.rect(CANVAS_SLOT).expect("the canvas is seated");
    let graph = bench.canvas.zoom();
    step(&mut bench, &pointer_at(centre(canvas), false, false, 1.0));
    assert_ne!(bench.canvas.zoom(), graph, "the canvas took its own notch");
    assert_eq!(bench.sample.zoom, 1.0, "…and the sample none");
    let slider = frame.rect(LIT_ZOOM).expect("the zoom slider is laid out");
    step(&mut bench, &pointer_at(centre(slider), false, false, 1.0));
    assert_eq!(bench.sample.zoom, 1.0, "a notch beside the seat");

    // The material dropdown, open: its menu hangs over the seat.
    let field = frame
        .rect("material_select")
        .expect("the dropdown is laid out");
    click(&mut bench, centre(field));
    let f = step(&mut bench, &snap());
    let [mx, my, mw, mh] = f.popup.expect("the dropdown opened");
    let (left, right) = (mx.max(seat.pos.x), (mx + mw).min(seat.pos.x + seat.size.x));
    let (top, bottom) = (my.max(seat.pos.y), (my + mh).min(seat.pos.y + seat.size.y));
    assert!(
        left + 20.0 < right && top + 20.0 < bottom,
        "the menu {:?} hangs over the seat {seat:?}",
        [mx, my, mw, mh]
    );
    let under = Vec2::new((left + right) * 0.5, (top + bottom) * 0.5);
    let f = step(&mut bench, &pointer_at(under, false, false, 1.0));
    assert!(f.popup_hit, "the menu owns the pointer");
    assert_eq!(
        bench.sample.zoom, 1.0,
        "no notch reached the sample through the menu"
    );

    // The walker's own sample for the seat, a notch on it — on a frame an open menu spent.
    let slot = bench.lit_slot.clone().expect("the lit sample is seated");
    let over = SurfacePointer {
        id: LIT_SLOT.into(),
        root: false,
        cursor: centre(seat),
        local: centre(seat) - seat.pos,
        delta: Vec2::ZERO,
        left: false,
        right: false,
        pressed: false,
        wheel: 1.0,
        captured: false,
        rect: graph_view::seat_of(&slot),
    };
    bench.zoom_lit(Some(&over), true);
    assert_eq!(bench.sample.zoom, 1.0, "a frame the menu spent is nobody's");
    bench.zoom_lit(Some(&over), false);
    assert_eq!(
        bench.sample.zoom,
        1.0 + bench.zoom.step,
        "the same notch, no menu: it zooms"
    );
}

/// THE ZOOM IS THE VIEW'S, never the document's: the body toggle keeps it, and so does every
/// whole-document load — a preset, a Roll, and a slot's committed look opening.
#[test]
fn the_lit_zoom_survives_the_body_toggle_and_every_load() {
    use flicker_materials::EMISSIVE_FIRST;
    let mut bench = MaterialBench::shipped();
    assert!(!fire(&mut bench, LIT_ZOOM, 3.0), "a zoom is no image edit");
    assert_eq!(bench.sample.zoom, 3.0);
    fire(&mut bench, "lit_body", true);
    assert_eq!(bench.sample.body, flicker_materialview::Body::Plane);
    assert_eq!(bench.sample.zoom, 3.0, "the body toggle keeps it");
    assert!(fire(&mut bench, "sel_preset", 0.0), "New loads");
    assert_eq!(bench.sample.zoom, 3.0, "a preset load keeps it");
    assert!(fire(&mut bench, "roll", true), "a Roll loads");
    assert_eq!(bench.sample.zoom, 3.0, "a Roll keeps it");

    let (mut bench, staging, red) = bench_over_a_staged_glow("zoom_keeps");
    fire(&mut bench, LIT_ZOOM, 2.0);
    let pick = option_of(&bench, EMISSIVE_FIRST);
    fire(&mut bench, "sel_material", pick);
    bench.prompt_armed = false; // `update` took the arm and pushed the prompt
    bench.modal_closed(PROMPT_TREE, ASK_REPLACE, None);
    assert_eq!(bench.graph(), &red, "the slot's committed look opened");
    assert_eq!(bench.sample.zoom, 2.0, "a slot load keeps it");
    let _ = std::fs::remove_dir_all(&staging);
}

/// A ZOOM SLIDER THAT WILL NOT READ IS LOUD: a key that is not a number, a key missing, or
/// the slider gone is ONE error naming the slider (and the key), and the zoom is OFF — the
/// stage's own framing, which neither the wheel nor the slider moves. Never a silent default.
#[test]
fn a_zoom_slider_that_will_not_read_is_one_error_and_the_zoom_is_off() {
    fn edit(v: &mut serde_json::Value, f: &mut dyn FnMut(&mut serde_json::Value)) {
        f(v);
        if let Some(kids) = v.get_mut("children").and_then(|c| c.as_array_mut()) {
            for kid in kids {
                edit(kid, f);
            }
        }
    }
    let is_zoom =
        |v: &serde_json::Value| v.get("id").and_then(serde_json::Value::as_str) == Some(LIT_ZOOM);
    type Break = fn(&mut serde_json::Value);
    let cases: [(&str, Option<&str>, Break); 3] = [
        ("a step that is text", Some("step"), |v| {
            v["step"] = serde_json::json!("fast");
        }),
        ("no max", Some("max"), |v| {
            v.as_object_mut().unwrap().remove("max");
        }),
        ("the slider gone", None, |_| {}),
    ];
    for (case, key, broken) in cases {
        let mut doc: serde_json::Value = serde_json::from_str(SW_SCENE).unwrap();
        edit(&mut doc["tree"], &mut |v| {
            if is_zoom(v) {
                broken(v);
            }
            if key.is_none() {
                if let Some(kids) = v.get_mut("children").and_then(|c| c.as_array_mut()) {
                    kids.retain(|k| !is_zoom(k));
                }
            }
        });
        let def = SceneDef::from_json("materialbench", &doc).expect("the edited scene parses");
        let roots = scratch_roots("zoom_loud");
        let (bench, errors) = errors_during(|| {
            MaterialBench::from_parts(def.tree, def.styles, &def.params, roots.clone())
        });
        let named: Vec<&String> = errors.iter().filter(|e| e.contains(LIT_ZOOM)).collect();
        assert_eq!(
            named.len(),
            1,
            "{case}: ONE error names the slider: {errors:?}"
        );
        if let Some(key) = key {
            assert!(
                named[0].contains(&format!("`{key}`")),
                "{case}: and the key: {}",
                named[0]
            );
        }
        assert_eq!(bench.zoom, ZoomDial::OFF, "{case}: the zoom is off");
        let mut sample = MaterialSample::new(LIT_STAGE_SOURCE);
        assert!(
            !sample.zoom_by(1.0, bench.zoom.step, bench.zoom.range())
                && !sample.set_zoom(3.0, bench.zoom.range()),
            "{case}: nothing moves an OFF zoom"
        );
        let _ = std::fs::remove_dir_all(roots.0.parent().unwrap());
    }
}

/// DEVELOPMENT-TIER GATES (ruling 977B4D38): source-reading handoff conditions.
mod gates {
    use super::*;

    const SOURCES: [(&str, &str); 3] = [
        ("lib.rs", include_str!("lib.rs")),
        ("route.rs", include_str!("route.rs")),
        ("designer.rs", include_str!("designer.rs")),
    ];

    /// The STRINGS gate's blind side: copy published from RUST into the Model.
    #[test]
    fn no_raw_display_copy_is_published_into_the_model() {
        for (file, src) in SOURCES {
            let flagged = strings::raw_model_publish_literals(src);
            assert!(flagged.is_empty(), "{file} publishes raw copy: {flagged:?}");
        }
    }

    /// THE BENCH COMPOSES NO UI NODE (five-line law; the option filler is RETIRED):
    /// every option, row and swatch is expanded by the walker's own repeater
    /// (`instantiate_rows`, `rows_from` in the scene), so no shipped source of this crate
    /// spells a node into being. Two rules over lib.rs, route.rs, designer.rs and
    /// commit.rs (comments and `use` lines aside):
    ///
    /// * the TOKEN `UiNode` appears only as a TYPE that holds or borrows a tree the walker
    ///   made — `Option<UiNode>`, `Vec<UiNode>`, `&UiNode`, `&mut UiNode` — never as a
    ///   struct literal, a constructor path, a turbofish, an annotated binding
    ///   (`: UiNode =`) or on a line that deserializes (`from_value(` / `from_str(` …);
    /// * no line edits a node by any other spelling — its props, kind, id or visibility,
    ///   or its children (pushed, inserted, extended, spliced, resized, truncated,
    ///   retained, drained, cloned over, assigned, indexed or taken) — nor parses a tree
    ///   from JSON or Lua, builds a Rust panel, or parses a scene other than the one
    ///   shipped-scene parse; and the scene names no bench-side option family.
    #[test]
    fn the_bench_composes_no_ui_node() {
        const SHIPPED: [(&str, &str); 4] = [
            ("lib.rs", include_str!("lib.rs")),
            ("route.rs", include_str!("route.rs")),
            ("designer.rs", include_str!("designer.rs")),
            ("commit.rs", include_str!("commit.rs")),
        ];
        const HELD: [&str; 4] = ["Option<UiNode>", "Vec<UiNode>", "&mut UiNode", "&UiNode"];
        const DESERIALIZE: [&str; 4] = ["from_value(", "from_str(", "from_slice(", "from_reader("];
        const EDITS: [&str; 26] = [
            "fn fill_options",
            "options_from",
            "OPTIONS_FROM",
            ".props.insert(",
            ".component =",
            ".visible_bind =",
            ".id =",
            ".children.push(",
            ".children.insert(",
            ".children.extend(",
            ".children.append(",
            ".children.splice(",
            ".children.resize(",
            ".children.truncate(",
            ".children.retain(",
            ".children.drain(",
            ".children.clone_from(",
            ".children =",
            "children: vec!",
            "children[",
            "parse_ui_json",
            ".ui_tree(",
            "apply_props(",
            "chat_panel(",
            "UiNode::",
            "::<UiNode>",
        ];
        let mut scene_parses = 0;
        for (file, src) in SHIPPED {
            for (n, line) in src.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//")
                    || code.starts_with("use ")
                    || code.starts_with("pub use ")
                {
                    continue;
                }
                let at = format!("{file}:{}", n + 1);
                if line.contains("UiNode") {
                    let rest = HELD
                        .iter()
                        .fold(line.to_string(), |l, held| l.replace(held, ""));
                    assert!(
                        !rest.contains("UiNode"),
                        "{at} names `UiNode` outside a type that holds a tree: {line}"
                    );
                    assert!(
                        !DESERIALIZE.iter().any(|d| line.contains(d)),
                        "{at} deserializes a node: {line}"
                    );
                }
                for needle in EDITS {
                    assert!(
                        !line.contains(needle),
                        "{at} composes UI (`{needle}`): {line}"
                    );
                }
                assert!(
                    !(line.contains("mem::take(&mut") && line.contains("children")),
                    "{at} takes a node's children: {line}"
                );
                scene_parses += line.matches("SceneDef::parse(").count();
            }
        }
        assert_eq!(scene_parses, 1, "the one shipped-scene parse, and no other");
        assert!(
            !SW_SCENE.contains("options_from"),
            "the scene still names a bench-side option family"
        );
    }

    /// Rust never publishes a style path — washes are the pair script's (`derive()`),
    /// so no `.set(` here carries a `materialbench.` path.
    #[test]
    fn rust_never_publishes_a_style_path() {
        for (file, src) in SOURCES {
            for (n, line) in src.lines().enumerate() {
                if line.contains(".set(") {
                    assert!(
                        !line.contains("\"materialbench."),
                        "{file}:{} publishes a style path: {line}",
                        n + 1
                    );
                }
            }
        }
    }

    /// THE RACK IS GONE and nothing injects a style: the six-voice controls, the voice
    /// cursor and the injected glow styles survive nowhere in this crate's shipped code
    /// or its scene; a `TextureRecipe` appears only at the preset / Roll boundary.
    #[test]
    fn the_rack_ui_and_the_injected_styles_are_gone() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        assert!(
            !dir.join("palette.rs").exists(),
            "palette.rs (the injected styles) is gone"
        );
        for (file, src) in SOURCES {
            for needle in [
                "inject_glow_styles",
                "inject_",
                "sel_ch",
                "CHANNEL_COUNT",
                "TextureRecipe",
                "find_by_id_mut",
            ] {
                assert!(!src.contains(needle), "{file} still holds `{needle}`");
            }
        }
        let scene = SW_SCENE;
        for needle in ["\"ch1_", "\"sel_ch\"", "glowsw", "\"sel_lacunarity\""] {
            assert!(!scene.contains(needle), "the scene still holds {needle}");
        }
    }

    /// THE ANTI-DOUBLE-STEP GATE (MCP 801B1B09): a rail-claimed step name is stepped by
    /// the RAIL; the dispatcher never reads one.
    #[test]
    fn the_scene_never_hand_steps_a_rail_owned_name() {
        let mut bench = MaterialBench::shipped();
        let before = bench.selected_map();
        fire(&mut bench, "map_next", true);
        assert_eq!(bench.selected_map(), before);
        fire(&mut bench, "sel_map", 2.0);
        assert_eq!(bench.sel_map, 2);
        let tree = authored(&MaterialBench::shipped());
        let mut rail_names = Vec::new();
        for n in nodes_of(&tree) {
            for key in ["next_action", "prev_action"] {
                if let Some(Value::Text(name)) = n.props.get(key) {
                    rail_names.push(name.clone());
                }
            }
        }
        assert!(!rail_names.is_empty());
        let src = include_str!("route.rs");
        for name in rail_names {
            assert!(
                !src.contains(&format!("is_on(\"{name}\")")),
                "route.rs hand-steps rail-owned name `{name}`"
            );
        }
    }

    /// **The extraction gate**: the lit sample lives in `flicker-materialview`; this
    /// crate owns no render target, mesh or stage pass, and gives back what it holds in
    /// `exit` (incident ACD0F45D, rule 728E682F).
    #[test]
    fn no_scene_owned_sample_or_render_target_survives_in_this_crate() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        assert!(!dir.join("lit.rs").exists());
        let banned = [
            "LitPreview",
            "fn sphere_soup",
            "fn plane_soup",
            "build_textured_verts",
            "upload_textured_mesh",
            "free_textured_mesh",
            "draw_textured_mesh_pbr",
            "create_render_target",
            "free_render_target",
            "resize_render_target",
            "composite_panel",
            "fg.surface(",
            "CompositeTarget",
        ];
        for entry in std::fs::read_dir(&dir).expect("src/ is readable") {
            let path = entry.expect("a readable dir entry").path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("tests.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path)
                .expect("a readable source file")
                .replace("\r\n", "\n");
            let shipped = src
                .split_once("#[cfg(test)]\nmod tests {")
                .map(|(before, _)| before)
                .unwrap_or(&src);
            for needle in banned {
                assert!(
                    !shipped.contains(needle),
                    "{}: `{needle}` belongs to flicker-materialview",
                    path.display()
                );
            }
        }
        let lib = std::fs::read_to_string(dir.join("lib.rs")).unwrap();
        assert!(lib.contains("fn exit(&mut self, renderer: &mut Renderer)"));
        assert!(
            lib.contains("self.sample.release(renderer)")
                && lib.contains("self.bodies.take()")
                && lib.contains("bodies.release(renderer)"),
            "exit releases the sample's target and the shared bodies"
        );
        assert!(
            lib.contains("renderer.free_texture(t)"),
            "exit gives back the preview and swatch textures"
        );
    }
}
