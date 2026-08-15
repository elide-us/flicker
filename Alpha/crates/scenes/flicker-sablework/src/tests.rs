//! The bench's drift gates and its dispatcher's contract.
//!
//! None of this needs a GPU: the authored tree, the Model, the pair script, the
//! styles and the dispatcher are all reachable without a device, which is the
//! point of keeping the instrument a pure function and the scene a thin face on
//! it. The gates pin the three authored artifacts (scene tree · pair script ·
//! Rust vocabulary) to each other, so a rename in one is loud in the others.

use flicker::render::Vec2;
use flicker::script::{HudCommand, Value, ValueMap};
use flicker::ui::{run_ui, strings, UiInput, UiState};
use flicker_texture::{BlendMode, MapKind, NoiseKind};

use super::*;

/// The shipped stringtable, so a walked frame carries resolved en-us text rather
/// than raw `$token`s.
fn load_strings() {
    let table = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    strings::load_str(&table, "en-us");
}

/// THE tree the scene walks — the AUTHORED tree off the shipped scene file, the
/// same one the runtime receives through the manifest.
fn tree_of(bench: &Sablework) -> UiNode {
    bench.authored_tree().expect("the shipped scene file declares a tree")
}

/// Every node of the tree, flattened — for the vocabulary gates.
fn flatten<'a>(node: &'a UiNode, out: &mut Vec<&'a UiNode>) {
    out.push(node);
    for kid in &node.children {
        flatten(kid, out);
    }
}

/// A walked frame over the bench's FULL model (raw + derived) — what the screen
/// actually draws.
fn draw() -> Vec<HudCommand> {
    load_strings();
    let bench = Sablework::shipped();
    let tree = tree_of(&bench);
    let styles =
        flicker::ui::load_styles_for(HUD_UI_THEME, bench.scene_styles_json.as_ref());
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        screen: Vec2::new(1920.0, 1080.0),
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    run_ui(&tree, &bench.model(), &styles, &snap, &mut UiState::new())
        .commands
}

// ── drift gates ───────────────────────────────────────────────────────────────

/// The scene file itself parses — the manifest's copy and this crate's are the
/// same bytes, so this is the authoring gate.
#[test]
fn the_shipped_scene_file_parses_with_a_tree_and_styles() {
    let def = flicker::ui::SceneDef::parse("sablework", SW_SCENE).expect("scene file parses");
    assert_eq!(def.behaviour, "sablework", "the file names its own behaviour");
    assert!(def.tree.is_some(), "the scene file declares a tree");
    assert!(def.styles.is_some(), "the scene file carries this scene's style blocks");
}

/// The VOCABULARY gate. An unknown kind renders NOTHING (the walker
/// anchor-overlays its children and never reaches a draw arm), so a typo or a
/// name left behind by a rename would be an invisible hole nobody noticed until
/// they opened the bench.
#[test]
fn the_hud_names_only_known_kinds() {
    let tree = tree_of(&Sablework::shipped());
    let unknown = flicker::ui::unknown_kinds(&tree);
    assert!(unknown.is_empty(), "the console names unknown kinds: {unknown:?}");
}

/// The STRINGS gate (tree channel): every display literal is a `$token`, so no
/// English ships unlocalisable.
#[test]
fn the_hud_ships_no_raw_display_literals() {
    let tree = tree_of(&Sablework::shipped());
    let raw = flicker::ui::raw_display_literals(&tree);
    assert!(raw.is_empty(), "the console ships raw display literals: {raw:?}");
}

/// The STRINGS gate's blind side: copy published from RUST into the Model bypasses
/// the tree gate entirely, so the crate self-gates its own source.
#[test]
fn no_raw_display_copy_is_published_into_the_model() {
    for (file, src) in
        [("lib.rs", include_str!("lib.rs")), ("route.rs", include_str!("route.rs"))]
    {
        let flagged = strings::raw_model_publish_literals(src);
        assert!(flagged.is_empty(), "{file} publishes raw display copy: {flagged:?}");
    }
}

/// Every `$token` the bench publishes or the tree names must EXIST in the
/// stringtable. A token that resolves to nothing is the "authored name that fails
/// to nothing" failure: the control renders blank and looks like a layout bug.
#[test]
fn every_token_the_bench_draws_resolves() {
    let unresolved: Vec<String> = draw()
        .iter()
        .filter_map(|c| match c {
            HudCommand::Text { text, .. } if text.starts_with('$') => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(unresolved.is_empty(), "tokens with no stringtable entry: {unresolved:?}");
}

/// Every DECLARED result must have a dispatcher arm. A signal that fires a name
/// nothing handles is the "authored name that fails to nothing" failure: the pad
/// press does nothing and looks like dead hardware.
#[test]
fn every_declared_intent_reaches_the_dispatcher() {
    let mut bench = Sablework::shipped();
    let before = bench.selected_map();
    fire(&mut bench, "map_next", true);
    assert_ne!(bench.selected_map(), before, "the bumpers do not step the map tabs");
    fire(&mut bench, "map_prev", true);
    assert_eq!(bench.selected_map(), before, "stepping back does not return");

    // The tabs wrap both ways, so a pad can reach every view without a dead end.
    // VIEW_COUNT, not MAP_IDS.len(): the LIT view is the eighth tab.
    for _ in 0..VIEW_COUNT {
        fire(&mut bench, "map_prev", true);
    }
    assert_eq!(bench.selected_map(), before, "the view tabs do not wrap");
}

// ── the three authored artifacts stay in lockstep ─────────────────────────────

/// The tabs are one vocabulary shared with `MapKind::ALL` plus the LIT view; the
/// authored option count must cover exactly that ring, or a bound number would
/// point at a view that does not exist.
#[test]
fn the_view_tabs_cover_every_map_and_the_lit_view() {
    assert_eq!(MAP_IDS.len(), MapKind::ALL.len());
    let tree = tree_of(&Sablework::shipped());
    let mut nodes = Vec::new();
    flatten(&tree, &mut nodes);

    let tabs = nodes
        .iter()
        .find(|n| n.component == "tabs" && n.bind.as_deref() == Some("sel_map"))
        .expect("the view selector binds sel_map");
    let options = tabs.children.iter().filter(|k| k.component == "option").count();
    assert_eq!(options, VIEW_COUNT, "the tabs must offer every view exactly once");
}

/// Every view cell the model can show exists in the authored tree, gated by the
/// visibility bind the pair script derives — the tree, the Lua and `MAP_IDS`
/// name the same cells.
#[test]
fn every_view_cell_exists_and_is_visibility_gated() {
    let tree = tree_of(&Sablework::shipped());
    let mut nodes = Vec::new();
    flatten(&tree, &mut nodes);

    for id in MAP_IDS {
        let bind = format!("{id}_shown");
        assert!(
            nodes.iter().any(|n| n.visible_bind.as_deref() == Some(bind.as_str())),
            "no view cell is gated by {bind}"
        );
    }
    assert!(
        nodes.iter().any(|n| n.visible_bind.as_deref() == Some("lit_shown")),
        "no view cell is gated by lit_shown"
    );
    // The lit cell carries the rtt the walker reserves for the offscreen pass.
    assert!(
        nodes.iter().any(|n| n.component == "rtt" && n.id == "sw_lit"),
        "the LIT cell holds no `sw_lit` rtt node"
    );
}

/// Exactly one view cell is shown at a time — the pair script's derive() must
/// gate one on and the rest off for every position of the bound number. Two
/// stacked sprites would draw one over the other and look like the wrong map.
#[test]
fn exactly_one_view_cell_is_shown() {
    let mut bench = Sablework::shipped();
    for view in 0..VIEW_COUNT {
        let mut results = ValueMap::default();
        results.set("sel_map", view as f64);
        route::apply(&mut bench, &results);
        let model = bench.model();
        let mut shown: Vec<String> = MAP_IDS
            .iter()
            .map(|id| format!("{id}_shown"))
            .chain(["lit_shown".to_string()])
            .filter(|key| matches!(model.get(key), Some(Value::Bool(true))))
            .collect();
        assert_eq!(shown.len(), 1, "view {view} showed {shown:?}");
        let key = shown.remove(0);
        // The flat maps name their own cell; the eighth position is the LIT cell.
        let expected = MAP_IDS
            .get(view)
            .map(|id| format!("{id}_shown"))
            .unwrap_or_else(|| "lit_shown".to_string());
        assert_eq!(key, expected);
    }
}

/// The rack-row washes ride the pair script too: the selected voice's row wears
/// the selection style, every other row rests.
#[test]
fn the_selected_voice_row_wears_the_wash() {
    let mut bench = Sablework::shipped();
    fire(&mut bench, "ch3_blend", true); // touching voice 3 selects it
    let model = bench.model();
    for n in 1..=flicker_texture::CHANNEL_COUNT {
        let sty = model.text(&format!("ch{n}_sty")).expect("every row has a wash");
        if n == 3 {
            assert_eq!(sty, "sablework.row_sel", "the selected row must wear the wash");
        } else {
            assert_eq!(sty, "sablework.row", "an unselected row must rest");
        }
    }
}

// ── the dispatcher's contract ─────────────────────────────────────────────────

fn fire(bench: &mut Sablework, key: &str, value: impl Into<Value>) -> bool {
    let mut results = ValueMap::default();
    results.set(key, value);
    route::apply(bench, &results)
}

/// An edit re-bakes; a VIEW change does not. This is what keeps a drag cheap —
/// switching maps and selecting voices are free.
#[test]
fn only_recipe_edits_ask_for_a_bake() {
    let mut bench = Sablework::shipped();

    assert!(fire(&mut bench, "ch1_source", true), "changing a source is an edit");
    assert!(fire(&mut bench, "ch2_blend", true), "changing a blend is an edit");
    assert!(fire(&mut bench, "relief", 0.9_f64), "an output knob is an edit");
    assert!(fire(&mut bench, "reseed", true), "reseeding is an edit");

    assert!(!fire(&mut bench, "sel_map", 1.0_f64), "switching maps is a VIEW change");
    assert!(!fire(&mut bench, "sel_map", 5.0_f64), "switching maps is a VIEW change");
}

/// The bound view number lands inside the ring wherever a caller points it — a
/// wild number clamps rather than indexing past the views.
#[test]
fn the_view_number_clamps_into_the_ring() {
    let mut bench = Sablework::shipped();
    fire(&mut bench, "sel_map", 900.0_f64);
    assert!(bench.showing_lit(), "past the end lands on the last view");
    fire(&mut bench, "sel_map", -3.0_f64);
    assert_eq!(bench.selected_map(), MapKind::ALL[0], "below zero lands on the first");
}

/// Re-writing a control with the value it already holds is NOT an edit. A slider
/// publishes its value every frame it is held, so treating that as a change would
/// re-bake continuously while the mouse sits still on a handle.
#[test]
fn rewriting_the_same_value_does_not_rebake() {
    let mut bench = Sablework::shipped();
    let relief = bench.recipe().out.relief as f64;
    assert!(!fire(&mut bench, "relief", relief), "an unchanged knob must not re-bake");

    let scale = bench.recipe().channels[0].scale as f64;
    assert!(!fire(&mut bench, "ch1_scale", scale), "an unchanged slider must not re-bake");
}

/// Touching a voice selects it, so the right-hand fine knobs always describe the
/// voice under the cursor.
#[test]
fn touching_a_voice_selects_it() {
    let mut bench = Sablework::shipped();
    fire(&mut bench, "ch4_blend", true);
    assert_eq!(bench.selected_voice(), 3);
    fire(&mut bench, "ch2_scale", 9.0_f64);
    assert_eq!(bench.selected_voice(), 1);
}

/// Scale IS the lattice period, so the dispatcher must land it on a whole number
/// of cells however the slider reports itself — a fractional period cannot close
/// a seam, and the resulting swatch would tear at its own edge.
#[test]
fn scale_is_always_a_whole_number_of_cells() {
    let mut bench = Sablework::shipped();
    for raw in [1.4_f64, 6.5, 12.7, 63.9, 900.0, -5.0] {
        fire(&mut bench, "ch1_scale", raw);
        let scale = bench.recipe().channels[0].scale;
        assert!((1..=64).contains(&scale), "scale {scale} out of range from {raw}");
    }
}

/// Every knob clamps into the range its field actually allows, whatever a caller
/// writes — the recipe has to stay valid because the baker trusts it.
#[test]
fn every_knob_clamps_to_a_valid_recipe() {
    let mut bench = Sablework::shipped();
    for (key, lo, hi) in [
        ("relief", 0.0_f32, 1.0_f32),
        ("roughness", 0.0, 1.0),
        ("roughness_mod", -1.0, 1.0),
        ("metalness", 0.0, 1.0),
        ("metalness_mod", -1.0, 1.0),
        ("ao", 0.0, 1.0),
    ] {
        for wild in [-99.0_f64, 99.0] {
            fire(&mut bench, key, wild);
            let out = &bench.recipe().out;
            let v = match key {
                "relief" => out.relief,
                "roughness" => out.roughness,
                "roughness_mod" => out.roughness_mod,
                "metalness" => out.metalness,
                "metalness_mod" => out.metalness_mod,
                _ => out.ao,
            };
            assert!((lo..=hi).contains(&v), "{key} = {v} from {wild}, outside {lo}..{hi}");
        }
    }
}

/// The pill buttons walk their whole enum and come back round, so every source
/// and every blend is reachable with one control.
#[test]
fn the_pills_step_through_every_option_and_wrap() {
    let mut bench = Sablework::shipped();
    let start = bench.recipe().channels[0].source;
    let mut seen = vec![start];
    for _ in 0..NoiseKind::ALL.len() {
        fire(&mut bench, "ch1_source", true);
        seen.push(bench.recipe().channels[0].source);
    }
    assert_eq!(seen[0], seen[NoiseKind::ALL.len()], "sources must wrap");
    let mut unique: Vec<_> = seen.clone();
    unique.sort_by_key(|k| k.id());
    unique.dedup();
    assert_eq!(unique.len(), NoiseKind::ALL.len(), "every source must be reachable: {seen:?}");

    let mut seen = vec![bench.recipe().channels[0].blend];
    for _ in 0..BlendMode::ALL.len() {
        fire(&mut bench, "ch1_blend", true);
        seen.push(bench.recipe().channels[0].blend);
    }
    assert_eq!(seen[0], seen[BlendMode::ALL.len()], "blends must wrap");
}

/// A checkbox must be able to turn something OFF. `is_on` alone cannot — it reads
/// "absent" and "false" the same way, so an unchecked box would never register.
#[test]
fn a_checkbox_can_turn_a_voice_off() {
    let mut bench = Sablework::shipped();
    assert!(bench.recipe().channels[0].enabled, "the opening patch has voice 1 on");
    assert!(fire(&mut bench, "ch1_on", false), "unchecking is an edit");
    assert!(!bench.recipe().channels[0].enabled, "voice 1 turned off");
    assert!(fire(&mut bench, "ch1_on", true));
    assert!(bench.recipe().channels[0].enabled, "voice 1 turned back on");
}

/// Stepping patches walks the whole factory library and wraps both ways.
#[test]
fn the_patch_buttons_walk_the_library_both_ways() {
    let n = flicker_texture::presets::all().len();
    let mut bench = Sablework::shipped();
    let first = bench.recipe().id.clone();
    for _ in 0..n {
        fire(&mut bench, "patch_next", true);
    }
    assert_eq!(bench.recipe().id, first, "next must wrap around the library");
    fire(&mut bench, "patch_prev", true);
    assert_eq!(bench.recipe().id, flicker_texture::presets::all()[n - 1].id, "prev wraps back");
}

// ── the material binding + the commit controls ────────────────────────────────

/// The picker walks unbound → every material in index order → unbound. Unbound is
/// a real state, not an absence: a scratch surface whose identity is undecided.
#[test]
fn the_material_picker_walks_the_index_and_returns_to_unbound() {
    let mut bench = Sablework::shipped();
    let count = bench.materials.len();
    assert!(count > 0, "the material index loaded");

    // Start from unbound whatever the opening patch was bound to.
    while bench.recipe().material.is_some() {
        fire(&mut bench, "material", true);
    }
    let mut seen = Vec::new();
    for _ in 0..count {
        fire(&mut bench, "material", true);
        seen.push(bench.recipe().material.expect("bound while walking"));
    }
    assert_eq!(seen.len(), count, "every material is reachable");
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), count, "the walk repeats nothing");

    fire(&mut bench, "material", true);
    assert_eq!(bench.recipe().material, None, "past the end returns to unbound");
}

/// Every binding must resolve to a NAME. A binding the index cannot resolve reads
/// as `#id` rather than as unbound — they are different problems and must not look
/// the same on the console.
#[test]
fn a_binding_always_reads_as_something_specific() {
    let mut bench = Sablework::shipped();
    while bench.recipe().material.is_none() {
        fire(&mut bench, "material", true);
    }
    let label = bench.material_label();
    assert!(!label.starts_with('$'), "a bound material shows its name, not a token");
    assert!(!label.is_empty());

    bench.recipe.material = Some(251); // not in the 20-row index
    assert!(bench.material_label().starts_with('#'), "an unresolvable id reads as broken");

    bench.recipe.material = None;
    assert_eq!(bench.material_label(), "$sw_unbound");
}

/// THE GATE, checked where a user could actually reach it: the size control must
/// only ever land on an OFFERED rung. 4K and 8K bake correctly but are off the
/// picker pending the engine-level memory budget, and a control that stepped onto
/// one would quietly commit a 1.8 GB bake.
#[test]
fn the_size_control_never_reaches_a_gated_rung() {
    let mut bench = Sablework::shipped();
    assert_eq!(bench.bake_rung().px, flicker_texture::BAKE_DEFAULT, "opens on the baseline");
    for _ in 0..(flicker_texture::BAKE_SIZES.len() * 3) {
        fire(&mut bench, "bake_size", true);
        let rung = bench.bake_rung();
        assert!(rung.enabled, "the picker landed on gated rung {}", rung.label);
    }
}

/// Neither the binding nor the rung changes the IMAGE, so neither may ask for a
/// re-bake — the preview would render identical pixels at a real cost.
#[test]
fn binding_and_size_are_not_image_edits() {
    let mut bench = Sablework::shipped();
    assert!(!fire(&mut bench, "material", true), "binding a material is not an image edit");
    assert!(!fire(&mut bench, "bake_size", true), "the bake rung is not an image edit");
}

/// A commit is a WORKER job, so the click must return immediately with the bench
/// still live — and a second click while one is in flight must not start a second.
#[test]
fn a_commit_starts_once_and_does_not_block() {
    let mut bench = Sablework::shipped();
    assert_eq!(bench.commit_state, CommitState::Idle);
    fire(&mut bench, "commit", true);
    assert_eq!(bench.commit_state, CommitState::Working, "the click returned without baking");
    fire(&mut bench, "commit", true);
    assert_eq!(bench.commit_state, CommitState::Working, "a second click is not a second artifact");
}

/// The status line always says something SPECIFIC. A blank status during a
/// several-hundred-millisecond bake reads as a dead button.
#[test]
fn the_commit_status_is_never_blank() {
    let mut bench = Sablework::shipped();
    for state in [
        CommitState::Idle,
        CommitState::Working,
        CommitState::Done("/tmp/staging/materials/Granite".into()),
        CommitState::Failed("permission denied".into()),
    ] {
        bench.commit_state = state.clone();
        let status = bench
            .hud_model()
            .text("commit_status")
            .expect("the status line is published")
            .to_string();
        assert!(!status.is_empty(), "{state:?} published a blank status");
    }
}

// ── the lit preview ────────────────────────────────────────────────────────────

/// The lit view's own controls change how you LOOK at the surface, never what it
/// is — so neither may trigger a re-bake.
#[test]
fn the_lit_controls_are_not_recipe_edits() {
    let mut bench = Sablework::shipped();
    let body = bench.lit.body;
    assert!(!fire(&mut bench, "lit_body", true), "swapping the body is not an edit");
    assert_ne!(bench.lit.body, body, "the body did not swap");
    assert!(!fire(&mut bench, "lit_spin", false), "stopping the spin is not an edit");
    assert!(!bench.lit.spinning);
}
