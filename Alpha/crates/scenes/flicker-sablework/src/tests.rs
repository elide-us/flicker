//! The bench's drift gates and its dispatcher's contract.
//!
//! None of this needs a GPU: the tree, the Model, the styles and the dispatcher
//! are all reachable without a device, which is the point of keeping the
//! instrument a pure function and the scene a thin face on it.

use flicker::render::Vec2;
use flicker::script::{HudCommand, Value, ValueMap};
use flicker::ui::{load_styles, run_ui_with, strings, UiInput, UiState};
use flicker_texture::{BlendMode, MapKind, NoiseKind, CHANNEL_COUNT};

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

/// THE tree the scene walks. `build_tree` expands internally, so there is exactly
/// one seam and a gate cannot end up inspecting a different tree than the app draws.
fn tree_of(bench: &Sablework) -> UiNode {
    bench.build_tree(Vec2::new(1920.0, 1080.0))
}

/// A walked frame over the bench's real Model — what the screen actually draws.
fn draw() -> Vec<HudCommand> {
    load_strings();
    let bench = Sablework::new();
    let tree = tree_of(&bench);
    let styles = load_styles(HUD_UI_ELEMENTS);
    let host = bench.ui_host.as_ref().expect("component library loaded");
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        screen: Vec2::new(1920.0, 1080.0),
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    run_ui_with(&tree, &bench.hud_model(), &styles, &snap, &mut UiState::new(), Some(host))
        .commands
}

// ── drift gates ───────────────────────────────────────────────────────────────

/// The VOCABULARY gate. An unknown kind renders NOTHING (the walker
/// anchor-overlays its children and never reaches a draw arm), so a typo or a
/// name left behind by a rename would be an invisible hole nobody noticed until
/// they opened the bench.
#[test]
fn the_hud_names_only_known_kinds() {
    let tree = tree_of(&Sablework::new());
    let unknown = flicker::ui::unknown_kinds(&tree);
    assert!(unknown.is_empty(), "the console names unknown kinds: {unknown:?}");
}

/// The STRINGS gate (tree channel): every display literal is a `$token`, so no
/// English ships unlocalisable.
#[test]
fn the_hud_ships_no_raw_display_literals() {
    let tree = tree_of(&Sablework::new());
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

/// The bench draws its three columns — the shape check that catches a tree that
/// parses but lays out to nothing.
#[test]
fn the_console_draws() {
    let commands = draw();
    assert!(!commands.is_empty(), "the bench draws its panels + controls");
    let text = |needle: &str| {
        commands
            .iter()
            .any(|c| matches!(c, HudCommand::Text { text, .. } if text.contains(needle)))
    };
    assert!(text("SABLEWORK"), "the title renders");
    assert!(text("Channel Rack"), "the rack section header renders");
    assert!(text("Output Stage"), "the output-stage header renders");
    assert!(text("CH1") && text("CH6"), "every voice of the rack renders");
    assert!(text("Roughness"), "the output knobs render");
    assert!(text("Height"), "the map selector renders");
}

/// COMPOSITION IS DATA. The console is a proto in `ui_templates.json` that the scene
/// CONFIGURES — it does not build a surface, and there is no per-scene HUD script.
///
/// Pinned because the regression is silent: hand-composed chrome looks fine in
/// isolation and only reads as wrong beside another bench, and by then the parallel
/// idiom is load-bearing. The two halves asserted here are (a) the scene emits a
/// template INSTANCE, and (b) nothing unexpanded survives `build_tree`.
#[test]
fn the_console_is_a_data_proto_the_scene_only_configures() {
    let bench = Sablework::new();

    // (a) A raw screen carries exactly one child, and it is a template instance.
    let mut raw = UiNode { component: "screen".into(), ..Default::default() };
    raw.children = vec![UiNode { template: Some(CONSOLE_TEMPLATE.into()), ..Default::default() }];
    let expanded = expand(raw, &builtin_templates());
    assert!(
        !expanded.children.is_empty(),
        "`{CONSOLE_TEMPLATE}` is not registered in ui_templates.json"
    );

    // (b) The tree the app draws has no unexpanded proto left in it.
    fn unexpanded(n: &UiNode, out: &mut Vec<String>) {
        if let Some(t) = &n.template {
            out.push(t.clone());
        }
        for c in n.children.iter().chain(n.slots.values().flatten()) {
            unexpanded(c, out);
        }
    }
    let mut left = Vec::new();
    unexpanded(&tree_of(&bench), &mut left);
    assert!(left.is_empty(), "unexpanded template nodes reached the screen: {left:?}");

    // And there is no HUD script to regress into.
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/sensorium/scripts/hud_sablework.lua"
    );
    assert!(
        !std::path::Path::new(script).exists(),
        "a per-scene HUD script reappeared — composition belongs in ui_templates.json"
    );
}

/// The screen DECLARES its input as data, so a pad press, a key and a click are the
/// same event by the time the dispatcher sees them. A bench that read raw keys
/// instead would be unreachable from a controller — the floor, not an alternative.
#[test]
fn the_screen_declares_its_input_as_data() {
    let tree = tree_of(&Sablework::new());
    for signal in ["on_menu", "on_confirm", "on_cancel", "on_tab_next", "on_tab_prev"] {
        assert!(tree.props.contains_key(signal), "the screen does not declare {signal}");
    }
    // And the walker can collect them — the seam that puts them on the Router.
    assert!(!UiIntents::of(&tree).is_empty(), "declared intents were not collected");
}

/// Every DECLARED result must have a dispatcher arm. A signal that fires a name
/// nothing handles is the "authored name that fails to nothing" failure: the pad
/// press does nothing and looks like dead hardware.
#[test]
fn every_declared_intent_reaches_the_dispatcher() {
    let mut bench = Sablework::new();
    let before = bench.selected_map();
    fire(&mut bench, "map_next", true);
    assert_ne!(bench.selected_map(), before, "the bumpers do not step the map tabs");
    fire(&mut bench, "map_prev", true);
    assert_eq!(bench.selected_map(), before, "stepping back does not return");

    // The tabs wrap both ways, so a pad can reach every view without a dead end.
    // VIEW_COUNT, not MAP_IDS.len(): the LIT view is a seventh tab.
    for _ in 0..VIEW_COUNT {
        fire(&mut bench, "map_prev", true);
    }
    assert_eq!(bench.selected_map(), before, "the view tabs do not wrap");
}

/// Every workbench SLOT the bench fills must actually reach the screen. A slot
/// name the template does not have is silently dropped — the content simply never
/// draws, with no error anywhere.
#[test]
fn every_workbench_slot_reaches_the_screen() {
    load_strings();
    let commands = draw();
    let text = |needle: &str| {
        commands
            .iter()
            .any(|c| matches!(c, HudCommand::Text { text, .. } if text.contains(needle)))
    };
    assert!(text("SABLEWORK"), "header slot");
    assert!(text("Base") && text("Height"), "tabs slot (the map selector)");
    assert!(text("Channel Rack"), "viewport slot (the rack)");
    assert!(text("Output Stage"), "rail slot (the output stage)");
    assert!(text("Commit") && text("Ready"), "footer slot (actions + staging status)");
}

// ── the console's vocabulary must match the instrument's ──────────────────────

/// The Lua rack draws a FIXED number of voices. If `CHANNEL_COUNT` grew and the
/// tree did not, the extra voices would be silently unreachable — audible in the
/// image, invisible on the console.
#[test]
fn the_rack_draws_every_voice_the_instrument_has() {
    load_strings();
    let commands = draw();
    for n in 1..=CHANNEL_COUNT {
        let label = format!("CH{n}");
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, HudCommand::Text { text, .. } if text == &label)),
            "voice {n} of {CHANNEL_COUNT} has no row in hud_sablework.lua"
        );
    }
}

/// The map buttons are one vocabulary shared with the tree AND with
/// `MapKind::ALL`; a mismatch would point a button at the wrong texture.
#[test]
fn the_map_selector_covers_every_map() {
    assert_eq!(MAP_IDS.len(), MapKind::ALL.len());
    let bench = Sablework::new();
    let model = bench.hud_model();
    for id in MAP_IDS {
        assert!(model.get(&format!("{id}_shown")).is_some(), "{id} has no visibility bind");
        assert!(model.get(&format!("{id}_style")).is_some(), "{id} has no style bind");
    }
}

/// Exactly one map is shown at a time — six stacked sprites with two visible
/// would draw one over the other and look like the wrong map.
#[test]
fn exactly_one_map_is_shown() {
    let mut bench = Sablework::new();
    for picked in MAP_IDS {
        let mut results = ValueMap::default();
        results.set(picked, true);
        route::apply(&mut bench, &results);
        let model = bench.hud_model();
        let shown: Vec<&str> = MAP_IDS
            .iter()
            .copied()
            .filter(|id| matches!(model.get(&format!("{id}_shown")), Some(Value::Bool(true))))
            .collect();
        assert_eq!(shown, [picked], "selecting {picked} showed {shown:?}");
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
    let mut bench = Sablework::new();

    assert!(fire(&mut bench, "ch1_source", true), "changing a source is an edit");
    assert!(fire(&mut bench, "ch2_blend", true), "changing a blend is an edit");
    assert!(fire(&mut bench, "relief", 0.9_f64), "an output knob is an edit");
    assert!(fire(&mut bench, "reseed", true), "reseeding is an edit");

    assert!(!fire(&mut bench, "map_normal", true), "switching maps is a VIEW change");
    assert!(!fire(&mut bench, "map_height", true), "switching maps is a VIEW change");
}

/// Re-writing a control with the value it already holds is NOT an edit. A slider
/// publishes its value every frame it is held, so treating that as a change would
/// re-bake continuously while the mouse sits still on a handle.
#[test]
fn rewriting_the_same_value_does_not_rebake() {
    let mut bench = Sablework::new();
    let relief = bench.recipe().out.relief as f64;
    assert!(!fire(&mut bench, "relief", relief), "an unchanged knob must not re-bake");

    let scale = bench.recipe().channels[0].scale as f64;
    assert!(!fire(&mut bench, "ch1_scale", scale), "an unchanged slider must not re-bake");
}

/// Touching a voice selects it, so the right-hand fine knobs always describe the
/// voice under the cursor.
#[test]
fn touching_a_voice_selects_it() {
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
    assert!(!fire(&mut bench, "material", true), "binding a material is not an image edit");
    assert!(!fire(&mut bench, "bake_size", true), "the bake rung is not an image edit");
}

/// A commit is a WORKER job, so the click must return immediately with the bench
/// still live — and a second click while one is in flight must not start a second.
#[test]
fn a_commit_starts_once_and_does_not_block() {
    let mut bench = Sablework::new();
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
    let mut bench = Sablework::new();
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

/// THE SWATCH ACTUALLY DRAWS — the gate whose absence let the preview ship blank.
///
/// Every other gate passed while the swatch resolved to **0x0**: the tree was
/// valid, the vocabulary was known, the binds were published, and one sprite was
/// correctly emitted — at zero size, because a `stack` OVERLAYS its children
/// rather than stretching them and the sprite carried no explicit fill. The image
/// is the entire point of this bench, so its rect is a first-class assertion.
#[test]
fn the_swatch_draws_at_a_real_size() {
    load_strings();
    let sprites: Vec<(u32, f32, f32)> = draw()
        .iter()
        .filter_map(|c| match c {
            HudCommand::Sprite { tex, w, h, .. } => Some((*tex, *w, *h)),
            _ => None,
        })
        .collect();

    // Exactly one: six maps are stacked in the same well, gated by `visible_bind`,
    // and two visible would paint over each other.
    assert_eq!(sprites.len(), 1, "expected one visible map, got {sprites:?}");
    let (tex, w, h) = sprites[0];
    assert_eq!(tex, 0, "the opening map is BaseColor, texture 0");
    assert!(w > 100.0 && h > 100.0, "the swatch resolved to {w}x{h} — it will not be visible");
    assert!((w - h).abs() < 1.0, "the swatch is {w}x{h}; a non-square tiles as rectangles");
}

/// Selecting a map must move the SPRITE, not just the button highlight — the two
/// are separate binds, so a mismatch would light the right tab over the wrong image.
#[test]
fn switching_maps_switches_the_drawn_texture() {
    load_strings();
    let styles = load_styles(HUD_UI_ELEMENTS);
    let mut bench = Sablework::new();
    for (want, id) in MAP_IDS.iter().enumerate() {
        let mut results = ValueMap::default();
        results.set(*id, true);
        route::apply(&mut bench, &results);

        let tree = tree_of(&bench);
        let host = bench.ui_host.as_ref().expect("component library");
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let frame =
            run_ui_with(&tree, &bench.hud_model(), &styles, &snap, &mut UiState::new(), Some(host));
        let drawn: Vec<u32> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Sprite { tex, w, .. } if *w > 1.0 => Some(*tex),
                _ => None,
            })
            .collect();
        assert_eq!(drawn, [want as u32], "selecting {id} drew {drawn:?}");
    }
}

// ── the lit preview ────────────────────────────────────────────────────────────

/// The LIT view is the seventh tab, and selecting it swaps the flat swatch for the
/// rendered sample: the `rtt` node is reserved and NO sprite draws.
///
/// Both halves matter. A reserved rect with a sprite still drawing would paint the
/// flat map over the lit one; a sprite gone with no rect reserved would be a blank
/// panel, which is exactly how the swatch shipped invisible the first time.
#[test]
fn the_lit_tab_reserves_a_sub_scene_instead_of_a_sprite() {
    load_strings();
    let styles = load_styles(HUD_UI_ELEMENTS);
    let mut bench = Sablework::new();
    fire(&mut bench, LIT_ID, true);
    assert!(bench.showing_lit(), "the lit tab did not select");

    let tree = tree_of(&bench);
    let host = bench.ui_host.as_ref().expect("component library");
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        screen: Vec2::new(1920.0, 1080.0),
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    let frame =
        run_ui_with(&tree, &bench.hud_model(), &styles, &snap, &mut UiState::new(), Some(host));

    let slot = frame
        .rtts
        .iter()
        .find(|s| s.id == "sw_lit")
        .expect("the lit view reserved no sub-scene slot");
    assert_eq!(slot.source, "sablework_lit", "the slot names no stage source");
    assert!(slot.w > 100.0 && slot.h > 100.0, "the lit slot is {}x{}", slot.w, slot.h);

    let sprites = frame
        .commands
        .iter()
        .filter(|c| matches!(c, HudCommand::Sprite { w, .. } if *w > 1.0))
        .count();
    assert_eq!(sprites, 0, "a flat map still draws over the lit sample");
}

/// A flat tab reserves NO sub-scene — otherwise the offscreen pass would render
/// every frame to composite behind an opaque swatch, for nothing.
#[test]
fn a_flat_tab_costs_no_sub_scene() {
    load_strings();
    let styles = load_styles(HUD_UI_ELEMENTS);
    let mut bench = Sablework::new();
    fire(&mut bench, "map_normal", true);
    let tree = tree_of(&bench);
    let host = bench.ui_host.as_ref().expect("component library");
    let snap = UiInput {
        mouse: Vec2::new(-1.0, -1.0),
        clicked: false,
        down: false,
        screen: Vec2::new(1920.0, 1080.0),
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    let frame =
        run_ui_with(&tree, &bench.hud_model(), &styles, &snap, &mut UiState::new(), Some(host));
    assert!(
        frame.rtts.iter().all(|s| s.id != "sw_lit"),
        "the lit sub-scene renders while a flat map is showing"
    );
}

/// The lit view's own controls change how you LOOK at the surface, never what it
/// is — so neither may trigger a re-bake.
#[test]
fn the_lit_controls_are_not_recipe_edits() {
    let mut bench = Sablework::new();
    let body = bench.lit.body;
    assert!(!fire(&mut bench, "lit_body", true), "swapping the body is not an edit");
    assert_ne!(bench.lit.body, body, "the body did not swap");
    assert!(!fire(&mut bench, "lit_spin", false), "stopping the spin is not an edit");
    assert!(!bench.lit.spinning);
}

/// NO CONTROL DRAWS AN EMPTY LABEL.
///
/// The gate that would have caught 14 blank boxes shipping. `label_bind` was
/// listed among the walker's binds but never actually READ, so every button
/// carrying one drew `""` — and the token gate passed straight over it, because
/// an empty string does not start with `$`. Presence-of-text was asserted;
/// non-emptiness was not.
#[test]
fn no_control_draws_an_empty_label() {
    load_strings();
    let blank = draw()
        .iter()
        .filter(|c| matches!(c, HudCommand::Text { text, .. } if text.trim().is_empty()))
        .count();
    assert_eq!(blank, 0, "{blank} controls drew an empty label — a bound label that resolved to nothing");
}

/// EVERY BUTTON CAN FIRE.
///
/// The walker fires a node's `action`; its `id` is only the FOCUS id. A button
/// carrying an id alone is a dead control that still draws, highlights and
/// hovers — which is exactly how every button on this bench shipped inert while
/// the sliders worked.
#[test]
fn every_button_can_fire() {
    fn walk<'a>(n: &'a UiNode, out: &mut Vec<&'a UiNode>) {
        if n.component == "button" {
            out.push(n);
        }
        for c in n.children.iter().chain(n.slots.values().flatten()) {
            walk(c, out);
        }
    }
    let tree = tree_of(&Sablework::new());
    let mut buttons = Vec::new();
    walk(&tree, &mut buttons);
    assert!(buttons.len() >= 20, "only {} buttons found — the tree did not expand", buttons.len());

    let dead: Vec<String> = buttons
        .iter()
        .filter(|b| b.action.as_deref().unwrap_or("").is_empty())
        .map(|b| b.id.clone())
        .collect();
    assert!(dead.is_empty(), "buttons that fire nothing: {dead:?}");
}

/// Every action a button fires must reach the dispatcher and DO something — the
/// other half of the same failure. A button wired to a name nothing handles is
/// just as dead as one with no action at all.
#[test]
fn every_button_action_is_handled() {
    fn walk<'a>(n: &'a UiNode, out: &mut Vec<String>) {
        if n.component == "button" {
            if let Some(a) = n.action.as_deref() {
                out.push(a.to_string());
            }
        }
        for c in n.children.iter().chain(n.slots.values().flatten()) {
            walk(c, out);
        }
    }
    let mut actions = Vec::new();
    walk(&tree_of(&Sablework::new()), &mut actions);
    actions.sort();
    actions.dedup();

    // Every piece of state a button is allowed to move. Incomplete here means a
    // WORKING control reads as dead — which this list already did once, for the
    // bake rung.
    let observe = |b: &Sablework| {
        (
            b.recipe().clone(),
            b.selected_map(),
            b.selected_voice(),
            b.lit.body,
            b.lit.spinning,
            b.commit_state.clone(),
            b.bake_rung().px,
        )
    };
    for action in actions {
        // Fired from TWO starting views, because a selection is idempotent: picking
        // the tab already showing changes nothing and is correct. The action has to
        // move something from at least one reachable state.
        let moved = ["map_base", "map_metal"].iter().any(|from| {
            let mut bench = Sablework::new();
            fire(&mut bench, from, true);
            let before = observe(&bench);
            let edited = fire(&mut bench, &action, true);
            edited || observe(&bench) != before
        });
        assert!(moved, "`{action}` fires but changes nothing from any starting state");
    }
}
