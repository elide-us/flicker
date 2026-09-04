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
    bench
        .authored_tree()
        .expect("the shipped scene file declares a tree")
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
    let mut styles = flicker::ui::load_shared_styles(bench.scene_styles_json.as_ref());
    crate::palette::inject_glow_styles(&mut styles, &bench.palette);
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        right_down: false,
        screen: Vec2::new(1920.0, 1080.0),
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    };
    run_ui(&tree, &bench.model(), &styles, &snap, &mut UiState::new()).commands
}

// ── drift gates ───────────────────────────────────────────────────────────────

/// The scene file itself parses — the manifest's copy and this crate's are the
/// same bytes, so this is the authoring gate.
#[test]
fn the_shipped_scene_file_parses_with_a_tree_and_styles() {
    let def = flicker::ui::SceneDef::parse("sablework", SW_SCENE).expect("scene file parses");
    assert_eq!(
        def.behaviour, "sablework",
        "the file names its own behaviour"
    );
    assert!(def.tree.is_some(), "the scene file declares a tree");
    assert!(
        def.styles.is_some(),
        "the scene file carries this scene's style blocks"
    );
}

/// The VOCABULARY gate. An unknown kind renders NOTHING (the walker
/// anchor-overlays its children and never reaches a draw arm), so a typo or a
/// name left behind by a rename would be an invisible hole nobody noticed until
/// they opened the bench.
#[test]
fn the_hud_names_only_known_kinds() {
    let tree = tree_of(&Sablework::shipped());
    let unknown = flicker::ui::unknown_kinds(&tree);
    assert!(
        unknown.is_empty(),
        "the console names unknown kinds: {unknown:?}"
    );
}

/// The STRINGS gate (tree channel): every display literal is a `$token`, so no
/// English ships unlocalisable.
#[test]
fn the_hud_ships_no_raw_display_literals() {
    let tree = tree_of(&Sablework::shipped());
    let raw = flicker::ui::raw_display_literals(&tree);
    assert!(
        raw.is_empty(),
        "the console ships raw display literals: {raw:?}"
    );
}

/// The STRINGS gate's blind side: copy published from RUST into the Model bypasses
/// the tree gate entirely, so the crate self-gates its own source.
#[test]
fn no_raw_display_copy_is_published_into_the_model() {
    for (file, src) in [
        ("lib.rs", include_str!("lib.rs")),
        ("route.rs", include_str!("route.rs")),
    ] {
        let flagged = strings::raw_model_publish_literals(src);
        assert!(
            flagged.is_empty(),
            "{file} publishes raw display copy: {flagged:?}"
        );
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
    assert!(
        unresolved.is_empty(),
        "tokens with no stringtable entry: {unresolved:?}"
    );
}

/// Every DECLARED result must have a handler. A signal that fires a name nothing
/// handles is the "authored name that fails to nothing" failure: the pad press
/// does nothing and looks like dead hardware. Two handler tiers exist: a rail
/// name is handled by the AUTHORED RAIL (the strip steps its own bind inside the
/// walker), a bench name by the dispatcher.
#[test]
fn every_declared_intent_reaches_a_handler() {
    // The rail names: the authored `tabs` strip claims them, so the walker's
    // strip-step channel is their handler — the scene never steps them.
    let tree = tree_of(&Sablework::shipped());
    let mut nodes = Vec::new();
    flatten(&tree, &mut nodes);
    let rail = nodes
        .iter()
        .find(|n| n.id == "sw_views")
        .expect("the view rail is authored");
    assert_eq!(
        rail.props.get("next_action"),
        Some(&Value::Text("map_next".into()))
    );
    assert_eq!(
        rail.props.get("prev_action"),
        Some(&Value::Text("map_prev".into()))
    );

    // The bench names: the dispatcher steps the patch ring itself (no rail
    // claims these), so the declared page intents land on a live arm.
    let mut bench = Sablework::shipped();
    let before = bench.patch;
    fire(&mut bench, "patch_next", true);
    assert_ne!(bench.patch, before, "patch_next reaches the dispatcher");
    fire(&mut bench, "patch_prev", true);
    assert_eq!(bench.patch, before, "patch_prev steps back");
}

/// THE ANTI-DOUBLE-STEP GATE (the skipped-tab bug, MCP 801B1B09): a name an
/// authored rail claims as its `next_action`/`prev_action` is stepped by the RAIL
/// inside the walker — the scene reads only the echoed index. A dispatcher arm
/// stepping the same name is a SECOND consumer: +2 per bumper press. This pins
/// the fix at both levels — behaviour (the fired name moves nothing scene-side;
/// the echoed bind is what moves the view) and source (no `is_on` on any
/// rail-claimed name anywhere in the dispatcher).
#[test]
fn the_scene_never_hand_steps_a_rail_owned_name() {
    // Behaviour: the fired rail name is a scene-side no-op; the echoed bind moves.
    let mut bench = Sablework::shipped();
    let before = bench.selected_map();
    fire(&mut bench, "map_next", true);
    assert_eq!(
        bench.selected_map(),
        before,
        "the scene does not step on the fired name"
    );
    fire(&mut bench, "sel_map", 2.0);
    assert_eq!(
        bench.sel_map, 2,
        "the echoed rail index is what the scene adopts"
    );

    // Source: no dispatcher arm reads any rail-claimed step name.
    let tree = tree_of(&Sablework::shipped());
    let mut nodes = Vec::new();
    flatten(&tree, &mut nodes);
    let mut rail_names = Vec::new();
    for n in &nodes {
        for key in ["next_action", "prev_action"] {
            if let Some(Value::Text(name)) = n.props.get(key) {
                rail_names.push(name.clone());
            }
        }
    }
    assert!(!rail_names.is_empty(), "the tree authors rail step names");
    let src = include_str!("route.rs");
    for name in rail_names {
        assert!(
            !src.contains(&format!("is_on(\"{name}\")")),
            "route.rs hand-steps rail-owned name `{name}` — the rail already steps it (+2 per press)"
        );
    }
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
    let options = tabs
        .children
        .iter()
        .filter(|k| k.component == "option")
        .count();
    assert_eq!(
        options, VIEW_COUNT,
        "the tabs must offer every view exactly once"
    );
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
            nodes
                .iter()
                .any(|n| n.visible_bind.as_deref() == Some(bind.as_str())),
            "no view cell is gated by {bind}"
        );
    }
    assert!(
        nodes
            .iter()
            .any(|n| n.visible_bind.as_deref() == Some("lit_shown")),
        "no view cell is gated by lit_shown"
    );
    // The lit cell carries the rtt the walker reserves for the offscreen pass.
    assert!(
        nodes
            .iter()
            .any(|n| n.component == "surface" && n.id == "sw_lit"),
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
        let sty = model
            .text(&format!("ch{n}_sty"))
            .expect("every row has a wash");
        if n == 3 {
            assert_eq!(
                sty, "sablework.row_sel",
                "the selected row must wear the wash"
            );
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

    assert!(
        fire(&mut bench, "ch1_source", true),
        "changing a source is an edit"
    );
    assert!(
        fire(&mut bench, "ch2_blend", true),
        "changing a blend is an edit"
    );
    assert!(
        fire(&mut bench, "relief", 0.9_f64),
        "an output knob is an edit"
    );
    assert!(fire(&mut bench, "reseed", true), "reseeding is an edit");

    assert!(
        !fire(&mut bench, "sel_map", 1.0_f64),
        "switching maps is a VIEW change"
    );
    assert!(
        !fire(&mut bench, "sel_map", 5.0_f64),
        "switching maps is a VIEW change"
    );
}

/// The bound view number lands inside the ring wherever a caller points it — a
/// wild number clamps rather than indexing past the views.
#[test]
fn the_view_number_clamps_into_the_ring() {
    let mut bench = Sablework::shipped();
    fire(&mut bench, "sel_map", 900.0_f64);
    assert!(bench.showing_lit(), "past the end lands on the last view");
    fire(&mut bench, "sel_map", -3.0_f64);
    assert_eq!(
        bench.selected_map(),
        MapKind::ALL[0],
        "below zero lands on the first"
    );
}

/// Re-writing a control with the value it already holds is NOT an edit. A slider
/// publishes its value every frame it is held, so treating that as a change would
/// re-bake continuously while the mouse sits still on a handle.
#[test]
fn rewriting_the_same_value_does_not_rebake() {
    let mut bench = Sablework::shipped();
    let relief = bench.recipe().out.relief as f64;
    assert!(
        !fire(&mut bench, "relief", relief),
        "an unchanged knob must not re-bake"
    );

    let scale = bench.recipe().channels[0].scale as f64;
    assert!(
        !fire(&mut bench, "ch1_scale", scale),
        "an unchanged slider must not re-bake"
    );
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
        assert!(
            (1..=64).contains(&scale),
            "scale {scale} out of range from {raw}"
        );
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
            assert!(
                (lo..=hi).contains(&v),
                "{key} = {v} from {wild}, outside {lo}..{hi}"
            );
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
    assert_eq!(
        unique.len(),
        NoiseKind::ALL.len(),
        "every source must be reachable: {seen:?}"
    );

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
    assert!(
        bench.recipe().channels[0].enabled,
        "the opening patch has voice 1 on"
    );
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
    assert_eq!(
        bench.recipe().id,
        first,
        "next must wrap around the library"
    );
    fire(&mut bench, "patch_prev", true);
    assert_eq!(
        bench.recipe().id,
        flicker_texture::presets::all()[n - 1].id,
        "prev wraps back"
    );
}

// ── the material binding + the commit controls ────────────────────────────────

/// The dropdown binds by OPTION INDEX: 0 = Unbound, i+1 = the i-th material. Unbound is
/// a real state, not an absence: a scratch surface whose identity is undecided.
#[test]
fn the_material_dropdown_binds_by_option_index() {
    let mut bench = Sablework::shipped();
    let count = bench.materials.len();
    assert!(count > 0, "the material index loaded");

    // Option 0 unbinds.
    fire(&mut bench, "sel_material", 0.0);
    assert_eq!(bench.recipe().material, None, "option 0 is Unbound");

    // Each material is reachable directly by its option index (position + 1).
    let mut seen = Vec::new();
    for i in 0..count {
        fire(&mut bench, "sel_material", (i + 1) as f64);
        seen.push(
            bench
                .recipe()
                .material
                .expect("bound to the picked material"),
        );
    }
    assert_eq!(seen.len(), count, "every material is reachable");
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), count, "each option binds a distinct material");

    // A value past the list clamps to Unbound rather than a wrong id or a panic.
    fire(&mut bench, "sel_material", (count + 5) as f64);
    assert_eq!(
        bench.recipe().material,
        None,
        "an option past the list is Unbound"
    );
}

/// The dropdown reads the binding as a specific NAME: every option's label binds its
/// material's name (never a `$token`), and the control echoes the bound material's option
/// index — 0 (Unbound) for None OR an id the index cannot resolve, so the two never render
/// a phantom option.
#[test]
fn the_dropdown_reads_the_binding_as_a_specific_name() {
    let bench = Sablework::shipped();
    let model = bench.model();
    for (i, (_, name)) in bench.materials.iter().enumerate() {
        let key = format!("mat_opt_{}", i + 1);
        match model.get(&key) {
            Some(Value::Text(s)) => {
                assert_eq!(s, name, "option {} binds the i-th material's name", i + 1);
                assert!(!s.starts_with('$'), "a material name is data, not a token");
            }
            other => panic!("option {} name not published as text: {other:?}", i + 1),
        }
    }

    let mut bench = Sablework::shipped();
    bench.set_material_by_option(1.0);
    assert_eq!(
        bench.model().number("sel_material"),
        Some(1.0),
        "the first material echoes option 1"
    );

    // An id the index does not carry falls to Unbound (0), not a phantom option.
    bench.recipe.material = Some(251);
    assert_eq!(
        bench.model().number("sel_material"),
        Some(0.0),
        "an unresolvable id reads as Unbound"
    );

    bench.set_material_by_option(0.0);
    assert_eq!(bench.recipe().material, None, "option 0 unbinds");
}

/// THE GATE, checked where a user could actually reach it: the size control must
/// only ever land on an OFFERED rung. 4K and 8K bake correctly but are off the
/// picker pending the engine-level memory budget, and a control that stepped onto
/// one would quietly commit a 1.8 GB bake.
#[test]
fn the_size_control_never_reaches_a_gated_rung() {
    let mut bench = Sablework::shipped();
    assert_eq!(
        bench.bake_rung().px,
        flicker_texture::BAKE_DEFAULT,
        "opens on the baseline"
    );
    for _ in 0..(flicker_texture::BAKE_SIZES.len() * 3) {
        fire(&mut bench, "bake_size", true);
        let rung = bench.bake_rung();
        assert!(
            rung.enabled,
            "the picker landed on gated rung {}",
            rung.label
        );
    }
}

/// Neither the binding nor the rung changes the IMAGE, so neither may ask for a
/// re-bake — the preview would render identical pixels at a real cost.
#[test]
fn binding_and_size_are_not_image_edits() {
    let mut bench = Sablework::shipped();
    assert!(
        !fire(&mut bench, "sel_material", 1.0),
        "binding a material is not an image edit"
    );
    assert!(
        !fire(&mut bench, "bake_size", true),
        "the bake rung is not an image edit"
    );
}

/// A rename edits the BOUND material's name and persists it STRICTLY by byte id — writing a
/// TEMP copy of the index (never the real content file). First keystroke replaces, then it
/// appends; an empty name is refused so a material is never blanked.
#[test]
fn the_material_rename_edits_a_bound_name_and_persists() {
    let dir = std::env::temp_dir().join("flicker_sw_rename_test");
    std::fs::create_dir_all(&dir).unwrap();
    let real = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/materials.json"
    );
    std::fs::copy(real, dir.join("materials.json")).unwrap();

    let mut bench = Sablework::shipped();
    bench.data_dir = dir.clone();
    assert!(!bench.materials.is_empty(), "the index loaded");

    // Bind the first material and open a rename on it.
    bench.set_material_by_option(1.0);
    let id = bench.recipe().material.expect("bound");
    bench.begin_rename();
    assert!(bench.is_renaming(), "a bound material opens a rename");

    // The field's edited text comes back on its bind (the preselect-replace is the
    // component's `select_all_on_enter`, tested in flicker-widgets).
    bench.set_rename_draft("Xy");
    assert_eq!(bench.rename_draft(), Some("Xy"), "the draft follows the field");

    // Commit persists: the in-memory index and the temp file both carry the new name.
    bench.commit_rename();
    assert!(!bench.is_renaming(), "commit closes the field");
    assert_eq!(
        bench.materials.iter().find(|(m, _)| *m == id).unwrap().1,
        "Xy",
        "the in-memory index updated at once"
    );
    let raw = std::fs::read_to_string(dir.join("materials.json")).unwrap();
    assert!(raw.contains("\"Xy\""), "the file persisted the new name");

    // An empty draft is refused — the field stays open, the material is not blanked.
    bench.begin_rename();
    bench.set_rename_draft("   ");
    bench.commit_rename();
    assert!(bench.is_renaming(), "an empty name keeps the field open");
    bench.cancel_rename();
    assert!(!bench.is_renaming(), "cancel closes without writing");
}

/// A rename needs a bound material — Unbound is not a material and has no byte to relabel.
#[test]
fn rename_needs_a_bound_material() {
    let mut bench = Sablework::shipped();
    bench.set_material_by_option(0.0); // Unbound
    bench.begin_rename();
    assert!(!bench.is_renaming(), "Unbound opens no rename");
}

/// The bound material's byte id is EXPLICIT in the model (#<id>), and Unbound shows no id —
/// the id is the u8 wire value, so the tool never hides which byte it is editing.
#[test]
fn the_byte_id_is_explicit_in_the_model() {
    let mut bench = Sablework::shipped();
    bench.set_material_by_option(0.0);
    match bench.model().get("material_id_label") {
        Some(Value::Text(s)) => assert_eq!(s, "—", "Unbound shows no id"),
        other => panic!("id label (unbound): {other:?}"),
    }
    bench.set_material_by_option(1.0);
    let id = bench.recipe().material.unwrap();
    match bench.model().get("material_id_label") {
        Some(Value::Text(s)) => assert_eq!(s, &format!("#{id}"), "bound shows #<byte>"),
        other => panic!("id label (bound): {other:?}"),
    }
}

/// A commit is a WORKER job, so the click must return immediately with the bench
/// still live — and a second click while one is in flight must not start a second.
#[test]
fn a_commit_starts_once_and_does_not_block() {
    let mut bench = Sablework::shipped();
    assert_eq!(bench.commit_state, CommitState::Idle);
    fire(&mut bench, "commit", true);
    assert_eq!(
        bench.commit_state,
        CommitState::Working,
        "the click returned without baking"
    );
    fire(&mut bench, "commit", true);
    assert_eq!(
        bench.commit_state,
        CommitState::Working,
        "a second click is not a second artifact"
    );
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
    assert!(
        !fire(&mut bench, "lit_body", true),
        "swapping the body is not an edit"
    );
    assert_ne!(bench.lit.body, body, "the body did not swap");
    assert!(
        !fire(&mut bench, "lit_spin", false),
        "stopping the spin is not an edit"
    );
    assert!(!bench.lit.spinning);
}

// ── the base-colour tint knobs ────────────────────────────────────────────────

/// The Hue and Saturation knobs are the bench's ONLY colour control: turning
/// either recolours the base ramp and re-bakes, and driving hue toward yellow with
/// saturation up makes the base map gold — red and green over blue, the colour the
/// rack's grey noise never had on its own.
#[test]
fn the_tint_knobs_recolour_the_base_map_gold() {
    let mut bench = Sablework::shipped();

    // A colour knob is a recipe edit (unlike selecting a voice or switching a view).
    assert!(
        fire(&mut bench, "tint_sat", 0.8_f64),
        "saturation is an edit"
    );
    assert!(fire(&mut bench, "tint_hue", 0.13_f64), "hue is an edit");

    // The knob reads back the ramp it wrote: the ramp is the one source of colour,
    // and the model publishes its hue for the slider to echo.
    assert!(
        (bench.model().number("tint_hue").unwrap() - 0.13).abs() < 0.02,
        "the hue knob echoes the ramp it set"
    );

    // Gold: a yellow hue at real saturation makes a base map whose red and green
    // dominate blue across the whole swatch.
    let set = flicker_texture::bake(bench.recipe(), 32);
    let base = set.get(MapKind::BaseColor).expect("the base map baked");
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for px in base.pixels.chunks(4) {
        r += px[0] as u64;
        g += px[1] as u64;
        b += px[2] as u64;
    }
    assert!(
        r > b && g > b,
        "gold base is red+green over blue (r{r} g{g} b{b})"
    );
}

/// Re-applying the ramp's current hue/saturation is not a change — a colour knob
/// released on the value it already holds must not re-bake, the same no-op-is-not-
/// an-edit contract the scalar knobs keep, checked on the tint channel too.
#[test]
fn re_tinting_the_same_colour_does_not_rebake() {
    let mut bench = Sablework::shipped();
    let (hue, sat) = bench.recipe().out.ramp.tint();
    assert!(
        !fire(&mut bench, "tint_hue", hue as f64),
        "the ramp's own hue is not an edit"
    );
    assert!(
        !fire(&mut bench, "tint_sat", sat as f64),
        "the ramp's own saturation is not an edit"
    );
}

// ── the flattened nav topology (plan 1A292918 T3) ─────────────────────────────

/// THE FLATTEN GATE (the bench's first nav gate). The rack container dissolved: the
/// six channels are DIRECT top-tier stops, so from the centre a single Left lands on
/// a channel and Up/Down walks them — no rack→voice→control nesting (enter depth ≤ 1).
/// Pins the stop roster, the depth invariant, and the geometric adjacency over the
/// real resolved layout via the router's own `nav_geometric`.
#[test]
fn the_flattened_nav_topology_is_the_authored_surface() {
    use flicker_input_router::{nav_geometric, Focusable, NavDir};
    use std::collections::HashSet;

    let bench = Sablework::shipped();
    let tree = tree_of(&bench);
    let mut all = Vec::new();
    flatten(&tree, &mut all);

    // Container discovery mirrors the walker: a node is a top-tier STOP when it has an
    // id, carries NO tab_group, and is CLAIMED by a member (or authors `pane: true`).
    let claimed: HashSet<&str> = all
        .iter()
        .filter(|n| !n.tab_group.is_empty())
        .map(|n| n.tab_group.as_str())
        .collect();
    let is_container = |n: &UiNode| {
        claimed.contains(n.id.as_str()) || matches!(n.props.get("pane"), Some(Value::Bool(true)))
    };
    let mut stops: Vec<&UiNode> = all
        .iter()
        .copied()
        .filter(|n| !n.id.is_empty() && n.tab_group.is_empty() && is_container(n))
        .collect();
    stops.sort_by_key(|n| n.nav_ordinal);

    // (a) The roster IS the flattened surface — the six channels as direct stops.
    let roster: Vec<(u32, &str)> = stops
        .iter()
        .map(|n| (n.nav_ordinal, n.id.as_str()))
        .collect();
    assert_eq!(
        roster,
        vec![
            (1, "sw_top"),
            (2, "sw_voice_1"),
            (3, "sw_voice_2"),
            (4, "sw_voice_3"),
            (5, "sw_voice_4"),
            (6, "sw_voice_5"),
            (7, "sw_voice_6"),
            (8, "sw_view"),
            (9, "sw_out"),
            (10, "sw_footer"),
            (11, "sw_nav"),
            // The materials-page container — claimed by the Rust-filled list rows. It is a
            // structural stop but is `visible_bind`-gated OFF on the bench page (page 0), so
            // the runtime walker never lands on it there; a degenerate rect keeps it out of
            // the geometric adjacency below.
            (12, "sw_mat_panel"),
        ],
        "the flattened top-tier stop roster"
    );
    assert!(
        !claimed.contains("sw_rack"),
        "the rack is pure layout now, not a nav stop"
    );

    // (b) ENTER DEPTH ≤ 1 + convention: no stop's interior is itself a container, and
    // every stop authors a non-zero ordinal.
    for stop in &stops {
        assert!(stop.nav_ordinal > 0, "{} authors a zero ordinal", stop.id);
        for n in &all {
            if n.tab_group == stop.id {
                assert!(
                    !claimed.contains(n.id.as_str()),
                    "enter depth > 1: container {} sits inside stop {}",
                    n.id,
                    stop.id
                );
            }
        }
    }

    // (c) GEOMETRIC adjacency over the RESOLVED layout at 1920×1080.
    let styles = flicker::ui::load_shared_styles(bench.scene_styles_json.as_ref());
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        right_down: false,
        screen: Vec2::new(1920.0, 1080.0),
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    };
    let frame = run_ui(&tree, &bench.model(), &styles, &snap, &mut UiState::new());
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
    // The cards sit in a 2-COLUMN grid (voice_1/2 top, 3/4 middle, 5/6 bottom), so nav is
    // grid-shaped: Left from the centre view lands on a channel, Right steps to the card
    // beside it, Down to the card below it.
    assert!(
        nav_geometric(&fo, "sw_view", NavDir::Left)
            .as_deref()
            .is_some_and(|s| s.starts_with("sw_voice_")),
        "Left from the centre view → a channel card, got {:?}",
        nav_geometric(&fo, "sw_view", NavDir::Left)
    );
    assert_eq!(
        nav_geometric(&fo, "sw_voice_1", NavDir::Right).as_deref(),
        Some("sw_voice_2"),
        "Right → the card beside it in the grid row"
    );
    assert_eq!(
        nav_geometric(&fo, "sw_voice_1", NavDir::Down).as_deref(),
        Some("sw_voice_3"),
        "Down → the card below it in the grid column"
    );
}

// ── the channel cards (B1) ────────────────────────────────────────────────────

/// Each channel card publishes a LIVE effect blurb: the current source's $token when
/// the voice sounds, or `$sw_fx_off` when it is silent.
#[test]
fn every_voice_card_publishes_a_live_description() {
    let bench = Sablework::shipped();
    let model = bench.model();
    for n in 1..=flicker_texture::CHANNEL_COUNT {
        let ch = &bench.recipe().channels[n - 1];
        // The blurb is two lines: a "status" ($sw_fx_) and a "comment" ($sw_fx2_), split so
        // neither runs off the narrow card.
        let (fx, fx2) = if ch.enabled {
            (
                format!("$sw_fx_{}", ch.source.id()),
                format!("$sw_fx2_{}", ch.source.id()),
            )
        } else {
            ("$sw_fx_off".to_string(), "$sw_fx2_off".to_string())
        };
        assert_eq!(
            model.text(&format!("ch{n}_fx")),
            Some(fx.as_str()),
            "voice {n} status line"
        );
        assert_eq!(
            model.text(&format!("ch{n}_fx2")),
            Some(fx2.as_str()),
            "voice {n} comment line"
        );
    }
}

/// The blurb FOLLOWS the source pill and the on-toggle — it describes the current
/// combination, not a static label.
#[test]
fn the_card_description_follows_source_and_enable() {
    let mut bench = Sablework::shipped();
    fire(&mut bench, "ch1_on", true); // ensure the voice sounds
    let before = bench.model().text("ch1_fx").unwrap().to_string();
    fire(&mut bench, "ch1_source", true); // step the source pill
    let after = bench.model().text("ch1_fx").unwrap().to_string();
    assert_ne!(before, after, "the blurb tracks the source pill");
    assert!(
        after.starts_with("$sw_fx_"),
        "still a localised fx token: {after}"
    );
    fire(&mut bench, "ch1_on", false);
    assert_eq!(
        bench.model().text("ch1_fx"),
        Some("$sw_fx_off"),
        "a silenced voice describes itself as off"
    );
}

/// The six cards FIT the rack at the reference resolution — none overflows the panel,
/// and each resolves to its authored card height (not collapsed).
#[test]
fn the_six_channel_cards_fit_the_rack() {
    let bench = Sablework::shipped();
    let tree = tree_of(&bench);
    let styles = flicker::ui::load_shared_styles(bench.scene_styles_json.as_ref());
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        right_down: false,
        screen: Vec2::new(1920.0, 1080.0),
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    };
    let frame = run_ui(&tree, &bench.model(), &styles, &snap, &mut UiState::new());
    let rack = frame.rect("sw_rack").expect("the rack resolves");
    let last = frame.rect("sw_voice_6").expect("the sixth card resolves");
    assert!(
        last.pos.y + last.size.y <= rack.pos.y + rack.size.y + 0.5,
        "card 6 (bottom {}) overflows the rack (bottom {})",
        last.pos.y + last.size.y,
        rack.pos.y + rack.size.y
    );
    for n in 1..=6 {
        let c = frame.rect(&format!("sw_voice_{n}")).expect("card resolves");
        assert!(c.size.y >= 100.0, "card {n} height {} collapsed", c.size.y);
    }
}

/// The card's contents FLOW inside the tile — the description wraps to the card width
/// (never bleeding into the neighbour) and the four sliders stack vertically, each a real
/// height. Pins the two flow bugs caught in-window: a `grow` text in a row overran its
/// box, and sliders in an overlay `stack` piled on top of each other. The fix was using
/// the components correctly — a direct-child wrap text (stretched to width) and a
/// vertical-flow `cell` for the slider column.
#[test]
fn the_card_contents_flow_inside_the_tile() {
    let bench = Sablework::shipped();
    let tree = tree_of(&bench);
    let styles = flicker::ui::load_shared_styles(bench.scene_styles_json.as_ref());
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        right_down: false,
        screen: Vec2::new(1920.0, 1080.0),
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    };
    let frame = run_ui(&tree, &bench.model(), &styles, &snap, &mut UiState::new());
    let card = frame.rect("sw_voice_1").expect("card resolves");

    // The description got a real, card-bounded width (so wrap has something to wrap to) and
    // does NOT spill past the card's right edge into the neighbouring tile.
    let desc = frame.rect("ch1_desc").expect("description resolves");
    assert!(
        desc.size.x > 100.0,
        "description width {} collapsed",
        desc.size.x
    );
    assert!(
        desc.pos.x + desc.size.x <= card.pos.x + card.size.x + 0.5,
        "description (right {}) overflows the card (right {})",
        desc.pos.x + desc.size.x,
        card.pos.x + card.size.x
    );

    // The four sliders stack vertically, each a real height, in order, inside the card.
    let mut prev_y = card.pos.y;
    for id in ["ch1_scale", "ch1_octaves", "ch1_warp", "ch1_amount"] {
        let s = frame.rect(id).unwrap_or_else(|| panic!("{id} resolves"));
        assert!(s.size.y >= 12.0, "{id} height {} collapsed", s.size.y);
        assert!(
            s.pos.y >= prev_y - 0.5,
            "{id} is not below the previous control"
        );
        assert!(
            s.pos.y + s.size.y <= card.pos.y + card.size.y + 0.5,
            "{id} overflows the card bottom"
        );
        prev_y = s.pos.y;
    }
}

// ── the emissive palette (C) ──────────────────────────────────────────────────

/// The shipped curated plastic set loads (ruled 2026-08-19): 8 entries, unique
/// ids, every component in range — and BLACK IS NOT A MEMBER (absence of light,
/// excluded by the same ruling). Pins the shipped file so the invariant can't
/// erode to an empty strip — or regrow a black swatch — unnoticed.
#[test]
fn the_glow_palette_loads_the_shipped_set() {
    let bench = Sablework::shipped();
    let pal = &bench.palette;
    assert_eq!(pal.len(), 8, "the curated 8-colour emissive palette loads");
    let mut ids: Vec<&str> = pal.iter().map(|c| c.id.as_str()).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "swatch ids are unique");
    for c in pal.iter() {
        assert!(
            c.rgb.iter().all(|&x| (0.0..=1.0).contains(&x)),
            "{}: rgb {:?} out of range",
            c.id,
            c.rgb
        );
        assert_ne!(
            c.rgb,
            [0.0, 0.0, 0.0],
            "{}: black is excluded from emissive",
            c.id
        );
    }
}

/// `nearest` is exact on a member and picks the closest off-palette colour.
#[test]
fn glow_nearest_picks_the_closest_entry() {
    let bench = Sablework::shipped();
    let pal = &bench.palette;
    let orange = pal.iter().position(|c| c.id == "orange").expect("orange");
    assert_eq!(
        pal.nearest(pal.get(orange).unwrap().rgb),
        Some(orange),
        "exact member"
    );
    assert_eq!(
        pal.nearest([0.95, 0.60, 0.10]),
        Some(orange),
        "near-orange snaps to orange"
    );
}

/// A swatch pick writes the EXACT palette colour into the glow and IS an image edit;
/// re-picking the same colour is a no-op (no re-bake) — the dispatcher's contract.
#[test]
fn a_glow_pick_writes_the_exact_colour_and_re_bakes_once() {
    let mut bench = Sablework::shipped();
    let want = bench.palette.get(3).unwrap().rgb;
    assert!(
        fire(&mut bench, "glow_pick_3", true),
        "a colour pick is an image edit"
    );
    assert_eq!(
        bench.recipe().out.emissive,
        want,
        "the exact palette colour is written"
    );
    assert!(
        !fire(&mut bench, "glow_pick_3", true),
        "re-picking the same colour must not re-bake"
    );
}

/// Roll SNAPS the random glow onto the palette — emissive stays bounded.
#[test]
fn roll_snaps_the_glow_onto_the_palette() {
    let mut bench = Sablework::shipped();
    let set: Vec<[f32; 3]> = bench.palette.iter().map(|c| c.rgb).collect();
    for _ in 0..8 {
        fire(&mut bench, "roll", true);
        assert!(
            set.contains(&bench.recipe().out.emissive),
            "a rolled glow {:?} is not a palette member",
            bench.recipe().out.emissive
        );
    }
}

/// The model echoes the selected swatch (nearest match) and publishes each id.
#[test]
fn the_model_echoes_the_selected_swatch() {
    let mut bench = Sablework::shipped();
    fire(&mut bench, "glow_pick_5", true);
    assert_eq!(
        bench.model().number("glow_sel"),
        Some(5.0),
        "the picked swatch echoes"
    );
    assert_eq!(bench.model().number("glow_count"), Some(8.0));
    let want_id = bench.palette.get(5).unwrap().id.clone();
    assert_eq!(
        bench.model().text("glow_sw6_id"),
        Some(want_id.as_str()),
        "each swatch id is published for the wash"
    );
}

/// FAIL-LOUD: every injected swatch style path resolves to a block. `style_bind` values
/// escape the cross-scene style-path gate, so the crate must cover the channel (8634C200).
#[test]
fn every_swatch_style_path_resolves() {
    let bench = Sablework::shipped();
    let mut styles = flicker::ui::load_shared_styles(bench.scene_styles_json.as_ref());
    crate::palette::inject_glow_styles(&mut styles, &bench.palette);
    for c in bench.palette.iter() {
        for suffix in ["", "_sel"] {
            let path = format!("sablework.glowsw.{}{}", c.id, suffix);
            let resolved = path.split('.').try_fold(&styles, |v, k| v.get(k));
            assert!(
                resolved.is_some_and(|v| v.is_object()),
                "swatch style path {path} resolves to nothing"
            );
        }
    }
}

/// Stepping patches loads each preset's glow VERBATIM — no snap on load, so a factory
/// patch keeps its authored emissive until the user picks a swatch.
#[test]
fn stepping_patches_keeps_authored_glow_no_snap_on_load() {
    let mut bench = Sablework::shipped();
    for _ in 0..flicker_texture::presets::all().len() {
        fire(&mut bench, "patch_next", true);
        let id = bench.recipe().id.clone();
        let authored = flicker_texture::presets::all()
            .into_iter()
            .find(|r| r.id == id)
            .unwrap()
            .out
            .emissive;
        assert_eq!(
            bench.recipe().out.emissive,
            authored,
            "patch {id} keeps its authored glow (no snap on load)"
        );
    }
}

/// The pair script derives the selection wash: the picked swatch wears `_sel`, the rest
/// their base block (mirrors the voice-row wash).
#[test]
fn the_selected_swatch_wears_the_wash() {
    let mut bench = Sablework::shipped();
    fire(&mut bench, "glow_pick_2", true);
    let model = bench.model();
    let id2 = bench.palette.get(2).unwrap().id.clone();
    let id0 = bench.palette.get(0).unwrap().id.clone();
    let sel = format!("sablework.glowsw.{id2}_sel");
    let base = format!("sablework.glowsw.{id0}");
    assert_eq!(
        model.text("glow_sw3_sty"),
        Some(sel.as_str()),
        "the picked swatch wears _sel"
    );
    assert_eq!(
        model.text("glow_sw1_sty"),
        Some(base.as_str()),
        "the rest wear the base"
    );
}

/// The swatch strip is Rust-filled from the palette: one `button` per entry, each firing
/// `glow_pick_<i>` in the sw_out ring.
#[test]
fn the_swatch_strip_is_filled_from_the_palette() {
    let bench = Sablework::shipped();
    let tree = tree_of(&bench);
    let mut all = Vec::new();
    flatten(&tree, &mut all);
    let strip = all
        .iter()
        .find(|n| n.id == "sw_glow_swatches")
        .expect("the swatch strip is authored");
    assert_eq!(
        strip.children.len(),
        bench.palette.len(),
        "one swatch per palette entry"
    );
    for (i, sw) in strip.children.iter().enumerate() {
        assert_eq!(sw.component, "button");
        assert_eq!(sw.id, format!("glow_pick_{i}"));
        let want_action = format!("glow_pick_{i}");
        assert_eq!(sw.action.as_deref(), Some(want_action.as_str()));
        assert_eq!(sw.tab_group, "sw_out");
        assert_eq!(sw.nav_ordinal, 11 + i as u32);
    }
}

// ── the materials page (B2, v1: browse + bind) ────────────────────────────────

/// The bench/materials page switch shows exactly one page and is a VIEW change (no
/// re-bake). The pair script gates the two bodies off `sel_page`.
#[test]
fn the_page_switch_shows_exactly_one_page() {
    let mut bench = Sablework::shipped();
    let m = bench.model();
    assert_eq!(
        m.get("page_bench_shown"),
        Some(&Value::Bool(true)),
        "opens on the bench"
    );
    assert_eq!(m.get("page_materials_shown"), Some(&Value::Bool(false)));

    assert!(
        !fire(&mut bench, "page_materials", true),
        "switching pages is a view change, not an image edit"
    );
    let m = bench.model();
    assert_eq!(m.get("page_bench_shown"), Some(&Value::Bool(false)));
    assert_eq!(m.get("page_materials_shown"), Some(&Value::Bool(true)));
    assert_eq!(bench.model().number("sel_page"), Some(1.0));

    fire(&mut bench, "page_bench", true);
    assert_eq!(
        bench.model().number("sel_page"),
        Some(0.0),
        "and back again"
    );
}

/// The materials LIST is Rust-filled — one `button` row per defined material, in the
/// materials-panel ring.
#[test]
fn the_materials_list_is_filled_from_the_index() {
    let bench = Sablework::shipped();
    let tree = tree_of(&bench);
    let mut all = Vec::new();
    flatten(&tree, &mut all);
    let list = all
        .iter()
        .find(|n| n.id == "sw_mat_list")
        .expect("the materials list is authored");
    assert_eq!(
        list.children.len(),
        bench.materials.len(),
        "one row per material"
    );
    for (i, row) in list.children.iter().enumerate() {
        assert_eq!(row.component, "button");
        assert_eq!(row.id, format!("mat_pick_{i}"));
        assert_eq!(row.tab_group, "sw_mat_panel");
    }
}

/// A list row binds the recipe to THAT material by id — the same recipe field the header
/// dropdown writes, and like it NOT an image edit.
#[test]
fn the_materials_list_binds_by_row() {
    let mut bench = Sablework::shipped();
    let count = bench.materials.len();
    assert!(count > 0, "the material index loaded");
    for i in 0..count {
        let want = bench.materials[i].0;
        assert!(
            !fire(&mut bench, &format!("mat_pick_{i}"), true),
            "binding a material is not an image edit"
        );
        assert_eq!(
            bench.recipe().material,
            Some(want),
            "row {i} binds its material"
        );
    }
}

/// The materials page draws no unresolved `$token` — the default frame gate never shows
/// this page, so it is walked here with the page switched on.
#[test]
fn the_materials_page_tokens_resolve() {
    load_strings();
    let mut bench = Sablework::shipped();
    fire(&mut bench, "page_materials", true);
    let tree = tree_of(&bench);
    let styles = flicker::ui::load_shared_styles(bench.scene_styles_json.as_ref());
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        right_down: false,
        screen: Vec2::new(1920.0, 1080.0),
        wheel: 0.0,
        exclusive: false,
        motion: Default::default(),
    };
    let cmds = run_ui(&tree, &bench.model(), &styles, &snap, &mut UiState::new()).commands;
    let unresolved: Vec<String> = cmds
        .iter()
        .filter_map(|c| match c {
            HudCommand::Text { text, .. } if text.starts_with('$') => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        unresolved.is_empty(),
        "materials page tokens: {unresolved:?}"
    );
}
