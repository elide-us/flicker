//! The God Mode bench's gates and rules.
//!
//! A CHILD module of `scene` (not a sibling): these drive the dispatcher and
//! read scene state directly, which only a child can see.

use super::*;

/// **The veil is never a lid.** Five saturated shells — the magma-era burst
/// that turned the readout into a white ball — must squeeze down until
/// two-thirds of the surface still shows through, without losing the
/// between-gas ratios that make the stack a read; and a light sky must pass
/// through untouched.
#[test]
fn the_air_veil_never_closes_into_a_lid() {
    // The defect world, roughly: every column absurdly past saturation.
    let burst: Vec<(u16, f32)> = vec![(92, 3.4e8), (2, 1.9e7), (93, 4.2e5), (91, 8.4e3)];
    let cover = veil_coverages(&burst);
    let see_through: f64 = cover.iter().map(|c| 1.0 - c).product();
    let floor = (-MAX_STACK_TAU).exp(); // ≈ 0.497 — the cap's own guarantee
    assert!(
        see_through >= floor - 1e-9,
        "the ceiling holds through the densest sky: {see_through:.3} vs e^-τmax {floor:.3}"
    );
    assert!(cover.iter().all(|&c| c > 0.0), "and every gas still shows");
    assert!(
        cover.windows(2).all(|p| p[0] >= p[1] - 1e-12),
        "the heavier column still reads denser: {cover:?}"
    );

    // A trace sky is below the ceiling and must not be touched at all.
    let trace: Vec<(u16, f32)> = vec![(91, 100.0)];
    let c = veil_coverages(&trace)[0];
    assert!((c - 0.1).abs() < 1e-9, "sqrt(100/1e4) = 0.1 passes through untouched: {c}");
}

/// Collect every `action` a button in the built tree declares — what the
/// surface promises the maintainer can press.
fn declared_actions(n: &UiNode, out: &mut Vec<String>) {
    if let Some(a) = n.action.as_ref() {
        out.push(a.clone());
    }
    for c in &n.children {
        declared_actions(c, out);
    }
}

/// Everything a dispatcher arm may move that is not a sim command — the
/// scene's own observable view state, as one owned comparable value.
type ViewState = (bool, bool, bool, bool, bool, Field, u64, u32, f64, Vec<f64>);
fn view_state(s: &GodModeScene) -> ViewState {
    (
        s.gates_open,
        s.starter_open,
        s.seed_shown,
        s.cut,
        s.air,
        s.field,
        s.seed,
        s.pending_freq,
        s.gate_ack,
        s.pending_scales.clone(),
    )
}

/// **Every button that looks alive IS alive.**
///
/// A control whose action name nothing dispatches is a dead button that
/// still draws, still highlights, and still does nothing when pressed —
/// which is exactly how CARRY ON shipped. Presence gates cannot see it: the
/// node is there, the label resolves, the tree walks green. So this drives
/// the ONE dispatcher directly with each declared action and demands the
/// bench answer — by moving its own view state, or by asking the sim for
/// something.
#[test]
fn every_declared_action_moves_the_bench() {
    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    let mut actions = Vec::new();
    declared_actions(&GodModeScene::new().build_tree(), &mut actions);
    actions.sort();
    actions.dedup();
    assert!(!actions.is_empty(), "the bench declares buttons at all");

    for action in actions {
        // Each action is tried from TWO opposite benches. A control is
        // idempotent from the state it puts you in — `gates_close` on a closed
        // console, `field_ore` on a bench already showing ore — so asking only
        // "did it move from the default" would call those dead when they are
        // simply already there. The claim that matters is that SOME reachable
        // state answers it.
        let moved = [false, true].into_iter().any(|opened| {
            let mut scene = GodModeScene::new();
            // Primed with a snapshot so the arms that read one (holds,
            // levers, resume, rate) have something to read.
            scene.snap = Some(fixture_snapshot());
            scene.gates_open = opened;
            scene.starter_open = opened;
            scene.seed_shown = opened;
            scene.field = if opened { Field::Ore } else { Field::Temperature };
            // A topology to face, so INSPECT has a cell to look inside. Without
            // one `facing_cell` is honestly `None` and the arm cannot act —
            // which is a bare scene, not a dead button.
            scene.dirs = vec![Vec3::X, Vec3::Y, Vec3::Z];
            let before = view_state(&scene);
            let cmds = scene.apply_results(&ValueMap::new().with(action.clone(), true));
            !cmds.is_empty() || view_state(&scene) != before
        });
        assert!(
            moved,
            "'{action}' is declared on a button but moves nothing from any state — \
             a dead control that still looks alive"
        );
    }
}

/// **A rebirth clears the gate acknowledgement.** The sim clears its gate log
/// on Reset and on a forge, so the scene's read-high-water must clear WITH it —
/// otherwise the second run's first gate (same stage, same `at_myr`) reads as
/// already-acknowledged and the pause summary never shows again: the run stops
/// and says nothing (the second-run silence Aaron hit in-window).
#[test]
fn a_rebirth_clears_the_gate_acknowledgement() {
    let mut scene = GodModeScene::new();
    scene.snap = Some(fixture_snapshot());

    // The maintainer read a gate at 500 My, then pressed RESET.
    scene.gate_ack = 500.0;
    let cmds = scene.apply_results(&ValueMap::new().with("reset", true));
    assert!(matches!(cmds.as_slice(), [SimCommand::Reset]), "reset asks the sim to rebirth");
    assert_eq!(scene.gate_ack, f64::NEG_INFINITY, "reset clears the read high-water");

    // Same story through the transport's RESEED (= the Starter's FORGE).
    scene.gate_ack = 500.0;
    let cmds = scene.apply_results(&ValueMap::new().with("reseed", true));
    assert!(
        matches!(cmds.as_slice(), [SimCommand::Reseed(_)]),
        "reseed forges a new world with a fresh seed"
    );
    assert_eq!(scene.gate_ack, f64::NEG_INFINITY, "a forged world has no read gates");
}

/// A snapshot with enough in it that the dispatcher's snapshot-reading arms
/// can act: one process to hold, levers to diff against, a gate to ack.
fn fixture_snapshot() -> Snapshot {
    use flicker_poc_chemistry::habitability::Habitability;
    Snapshot {
        gen: 1,
        tick: 1,
        tick_myr: 100.0,
        playing: false,
        rate_hz: 30.0,
        swept_cells: 0,
        state: Default::default(),
        cells: Vec::new(),
        plate_count: 0,
        recent_events: Vec::new(),
        // Every console row backed, so `hold_<n>` is answerable for all of
        // them. The proto carries PROCESS_ROWS rows against 17 real stages
        // — the spare rows ride `proc_<n>_shown = false` in the app, but a
        // row that IS backed must always hold, and that is the contract
        // under test.
        processes: (0..PROCESS_ROWS)
            .map(|_| flicker_poc_chemistry::ProcessState {
                name: "Outgassing",
                held: false,
                ready: true,
            })
            .collect(),
        levers: Default::default(),
        life: Default::default(),
        tissue_kg: 0.0,
        coal_kg: 0.0,
        oils_kg: 0.0,
        water_sea_kg: 1.4e21,
        water_sky_kg: 2.0e18,
        water_life_kg: 0.0,
        water_stone_kg: 0.0,
        air_shells: Vec::new(),
        habitability: Habitability {
            // Life-supporting, so the arms the era gate guards (RAIN ON) can
            // act from the fixture — the gate's own test builds a dead world.
            axes: Vec::new(),
            life_supporting: true,
            axes_in_band: 5,
            axes_live: 5,
        },
        gate_events: vec![crate::sim_thread::GateEvent {
            at_myr: 100.0,
            stage: "Outgassing",
            opened: true,
        }],
    }
}

/// **Each lever moves its OWN field, and nothing else.**
///
/// A rack of fourteen sliders driven by one table is exactly where a
/// copy-paste row silently points two controls at the same field: both would
/// still slide, both would still echo, and the bench would quietly be lying
/// about one of them. So every lever is dialled to twice the physics-as-written
/// and the resulting command is checked field by field against the default.
#[test]
fn each_lever_moves_exactly_its_own_field() {
    let base = Levers::default();
    for &(key, get, _) in LEVERS {
        let mut scene = GodModeScene::new();
        scene.snap = Some(fixture_snapshot());

        let cmds = scene.apply_results(&ValueMap::new().with(key, 2.0));
        let [SimCommand::SetLevers(next)] = cmds.as_slice() else {
            panic!("'{key}' should send exactly one SetLevers, got {cmds:?}");
        };
        assert!(
            (get(next) - 2.0 * get(&base)).abs() <= 1e-6 * get(&base).abs().max(1e-9),
            "'{key}' set its own field to {} (wanted 2x {})",
            get(next),
            get(&base)
        );
        // Every OTHER lever is untouched.
        for &(other, other_get, _) in LEVERS {
            if other == key {
                continue;
            }
            assert!(
                (other_get(next) - other_get(&base)).abs()
                    <= 1e-6 * other_get(&base).abs().max(1e-9),
                "'{key}' also moved '{other}' — two controls, one field"
            );
        }
    }
}

/// **A dial that has not moved sends nothing.** Every `SetLevers` rebuilds the
/// pipeline, so a slider echoing its own value back must not be mistaken for
/// the maintainer turning it — otherwise merely LOOKING at the bench rebuilds
/// the world sixty times a second.
#[test]
fn a_lever_at_its_echo_sends_nothing() {
    let mut scene = GodModeScene::new();
    scene.snap = Some(fixture_snapshot());
    // The fixture's levers ARE the defaults, so 1.0 is exactly the echo.
    let echo = LEVERS.iter().fold(ValueMap::new(), |m, &(key, _, _)| m.with(key, 1.0));
    assert!(scene.apply_results(&echo).is_empty(), "an unmoved rack is silent");

    // And the rate dial, the same way.
    let mut scene = GodModeScene::new();
    scene.snap = Some(fixture_snapshot());
    let at_rest = scene.snap.as_ref().expect("primed").rate_hz as f64;
    assert!(
        scene.apply_results(&ValueMap::new().with("rate", at_rest)).is_empty(),
        "an unmoved rate dial is silent"
    );
    let moved = scene.apply_results(&ValueMap::new().with("rate", at_rest + 20.0));
    assert!(
        matches!(moved.as_slice(), [SimCommand::SetRate(_)]),
        "and a turned one is heard: {moved:?}"
    );
}

/// **The verdict lamp answers, and it does not eat the life line.**
///
/// Regression: the lamp's `visible_bind` is `life_light`, but the publish
/// set `life` — which is the readout's life-line TEXT bind. One name
/// collision cost both readings: the lamp never lit, and a life-supporting
/// world overwrote its own biosphere line with the bool `true`. So this
/// asserts the two keys stay separate at the SOURCE (the publish), which is
/// where the collision was; a fixture-only test would have watched it
/// happen.
#[test]
fn the_verdict_lamp_lights_without_eating_the_life_line() {
    use flicker_poc_chemistry::habitability::Habitability;

    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    let mut scene = GodModeScene::new();
    scene.snap = Some(Snapshot {
        gen: 1,
        tick: 7,
        tick_myr: 700.0,
        playing: false,
        rate_hz: 30.0,
        swept_cells: 0,
        state: Default::default(),
        cells: Vec::new(),
        plate_count: 0,
        recent_events: Vec::new(),
        processes: Vec::new(),
        levers: Default::default(),
        life: Default::default(),
        tissue_kg: 0.0,
        coal_kg: 0.0,
        oils_kg: 0.0,
        water_sea_kg: 1.4e21,
        water_sky_kg: 2.0e18,
        water_life_kg: 0.0,
        water_stone_kg: 0.0,
        air_shells: Vec::new(),
        // The verdict this test exists for.
        habitability: Habitability {
            axes: Vec::new(),
            life_supporting: true,
            axes_in_band: 5,
            axes_live: 5,
        },
        gate_events: Vec::new(),
    });

    let m = scene.hud_model();
    assert!(m.is_on("life_light"), "a life-supporting world lights the lamp");
    assert!(
        m.text("life").is_some_and(|s| !s.is_empty()),
        "and the life line is still its own text: {:?}",
        m.get("life")
    );
}

/// **The bench has to land in its regions — at BOTH reference resolutions.**
///
/// Presence is not placement. The gates above all pass over a surface that has
/// collapsed into one corner, and this bench has a specific way to fail: the
/// instrument column, the globe and the lever rack share one row, so any of
/// them mis-sized eats the others. So this asserts geometry — instruments left,
/// levers right, transport in the header band, chip in the footer band, and the
/// globe's reserved rect actually between the columns and actually big.
///
/// Run at 1280×720 as well as 1920×1080 because the columns are the thing most
/// likely to break on a short screen, and a bench that only works on the
/// developer's monitor is a bench with a hidden requirement.
#[test]
fn the_bench_lands_its_regions_at_both_resolutions() {
    use flicker::ui::run_ui;

    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    let scene = GodModeScene::new();
    let tree = scene.build_tree();
    let styles = load_styles(HUD_UI_ELEMENTS);
    let chip_line = "\u{2699} 5 running  ·  12 waiting";

    for (screen_w, screen_h) in [(1920.0f32, 1080.0f32), (1280.0f32, 720.0f32)] {
        let snap = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            screen: Vec2::new(screen_w, screen_h),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let model = ValueMap::new().with("loaded", true).with("proc_summary", chip_line);
        let frame = run_ui(&tree, &model, &styles, &snap, &mut UiState::new());
        let cmds = &frame.commands;
        let at = |s: &str| -> (f32, f32) {
            cmds.iter()
                .find_map(|c| match c {
                    HudCommand::Text { text, x, y, .. } if text == s => Some((*x, *y)),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("`{s}` never rendered at {screen_w}x{screen_h}"))
        };

        // Instruments read down the LEFT.
        let (ledger_x, _) = at("CONSERVATION LEDGER");
        let (hab_x, _) = at("LIFE-SUPPORTING CONDITIONS");
        assert!(
            ledger_x < screen_w / 3.0 && hab_x < screen_w / 3.0,
            "instruments belong in the left column (ledger {ledger_x}, hab {hab_x}) \
             at {screen_w}x{screen_h}"
        );

        // Controls read down the RIGHT.
        let (rack_x, _) = at("BOUNDARY INPUTS");
        assert!(
            rack_x > screen_w / 2.0,
            "the lever rack belongs in the right rail (x {rack_x}) at {screen_w}x{screen_h}"
        );

        // Transport across the TOP, chip along the BOTTOM.
        let (_, title_y) = at("GOD MODE · WORLD FORGE");
        assert!(title_y < 90.0, "the title rides the header band (y {title_y})");
        let (_, chip_y) = at(chip_line);
        assert!(
            chip_y > screen_h - 120.0,
            "the processes chip rides the footer band (y {chip_y}) at {screen_w}x{screen_h}"
        );

        // The globe is reserved BETWEEN the columns, and it is big.
        let slot = frame
            .rtts
            .iter()
            .find(|s| s.id == "gm_globe")
            .unwrap_or_else(|| panic!("the globe reserved no slot at {screen_w}x{screen_h}"));
        assert!(
            slot.x > ledger_x && slot.x + slot.w < rack_x,
            "the globe sits between the columns (x {} w {} vs ledger {ledger_x} rack {rack_x})",
            slot.x,
            slot.w
        );
        let floor = if screen_w > 1500.0 { 400.0 } else { 300.0 };
        assert!(
            slot.w > floor && slot.h > floor,
            "the globe viewport is {}x{} at {screen_w}x{screen_h} — the planet needs room",
            slot.w,
            slot.h
        );

        // **Every view button is on screen, in roster order.** A strip that grew
        // from seven buttons to ten is exactly where a row quietly runs off the
        // edge of the narrower bench — and a view you cannot reach is a view you
        // do not have, however well the ramp behind it works.
        let mut last_x = f32::MIN;
        for &(action, _, token, _) in FIELD_ACTIONS.iter() {
            let label = flicker::ui::strings::resolve(token).into_owned();
            let (x, _) = at(&label);
            assert!(
                x >= 0.0 && x < screen_w,
                "the '{action}' tab ({label}) sits at x {x} on a {screen_w}-wide bench — \
                 the strip has outgrown the screen"
            );
            assert!(
                x > last_x,
                "the strip must read in roster order: {label} at {x} follows something at {last_x}"
            );
            last_x = x;
        }

        // The popups stay CLOSED: the bench does not carry everything at once.
        assert!(
            !cmds.iter().any(
                |c| matches!(c, HudCommand::Text { text, .. } if text == "SIMULATION GATES")
            ),
            "the gate console must be CLOSED by default at {screen_w}x{screen_h}"
        );
    }
}

/// **The buttons must actually work when CLICKED.** The pause summary's
/// CARRY ON was dead in the window while every test passed, because the
/// scene read only `hud_hit` from the HUD frame and threw the click
/// results away — the keyboard path worked, the mouse path fired into a
/// struct nobody read. So this drives the REAL path: render the popup,
/// find the button on screen, put the mouse there, click, and assert the
/// action lands in the frame's results — the map the dispatcher now reads.
#[test]
fn clicking_the_pause_summary_buttons_fires_their_actions() {
    use flicker::ui::run_ui;

    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    let scene = GodModeScene::new();
    let tree = scene.build_tree();
    let styles = load_styles(HUD_UI_ELEMENTS);
    let screen = Vec2::new(1920.0, 1080.0);
    let model = ValueMap::new()
        .with("loaded", true)
        .with("gate_pause_shown", true)
        .with("gate_headline", "Volcanism") // strings-gate-exempt: a stage NAME is an identifier, not display copy
        .with("gate_color", "chemistry.ok");

    // Pass 1: find where the button's label landed.
    let idle = UiInput {
        mouse: Vec2::new(-9.0, -9.0),
        clicked: false,
        down: false,
        screen,
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    let mut state = UiState::new();
    let cmds = run_ui(&tree, &model, &styles, &idle, &mut state).commands;
    let (bx, by) = cmds
        .iter()
        .find_map(|c| match c {
            HudCommand::Text { text, x, y, .. } if text == "CARRY ON" => Some((*x, *y)),
            _ => None,
        })
        .expect("the resume button renders");

    // Pass 2: click it. Aim slightly into the glyphs — the label's x/y is
    // its draw origin, which sits inside the button's hit rect.
    let click = UiInput {
        mouse: Vec2::new(bx + 4.0, by + 4.0),
        clicked: true,
        down: true,
        screen,
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    let results = run_ui(&tree, &model, &styles, &click, &mut state).results;
    assert!(
        results.is_on("gate_resume"),
        "a click on CARRY ON must fire `gate_resume` into the frame results"
    );
}

#[test]
fn hud_tree_is_well_formed_and_gates_its_states() {
    use flicker::ui::run_ui;
    use flicker_input_core::ActionSignal;

    // The HUD's display copy is `$token`s now (S10 strings gate); load the
    // shipped table so the walked commands carry the resolved en-us text.
    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    // GATE 4 — ABSENCE. The per-scene HUD script is GONE and must stay gone;
    // Sablework regressed once and only an absence check catches that.
    let legacy = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/sensorium/scripts/hud_chemistry.lua"
    );
    assert!(
        !std::path::Path::new(legacy).exists(),
        "hud_chemistry.lua is back — composition belongs in ui_templates.json"
    );

    let scene = GodModeScene::new();
    let tree = scene.build_tree();

    // GATE 1 — every template instance RESOLVED. An unexpanded proto would
    // draw a bare box in the app while a naive test walked past it.
    fn unresolved(n: &UiNode, out: &mut Vec<String>) {
        if let Some(t) = n.template.as_ref() {
            out.push(t.clone());
        }
        for c in &n.children {
            unresolved(c, out);
        }
    }
    let mut left = Vec::new();
    unresolved(&tree, &mut left);
    assert!(left.is_empty(), "template instances survived expansion: {left:?}");

    // GATE 2 — vocabulary. `unknown_kinds` reports an unresolved proto as
    // `template:<name>`, so this catches a misnamed proto too.
    assert!(
        flicker::ui::unknown_kinds(&tree).is_empty(),
        "the bench names unknown kinds: {:?}",
        flicker::ui::unknown_kinds(&tree)
    );
    // GATE 3 — the strings gate (S10): every display literal is a `$token`.
    assert!(
        flicker::ui::raw_display_literals(&tree).is_empty(),
        "the bench ships raw display literals: {:?}",
        flicker::ui::raw_display_literals(&tree)
    );
    // The MODEL-CHANNEL strings gate (S10's blind side): display copy published
    // from Rust into the Model bypasses the tree gate above, so the crate
    // self-gates its OWN source — every `.set`/`.with` value must be a resolved
    // `$token`, a data shape, or carry an explicit `strings-gate-exempt` reason.
    let flags = strings::raw_model_publish_literals(include_str!("../scene.rs"));
    assert!(flags.is_empty(), "raw display copy published into the Model: {flags:?}");
    // The input DECLARATION rides the tree as data — and every signal it
    // declares is one the scene actually dispatches (declare only what you
    // dispatch; a bound signal with no arm is dead hardware).
    let intents = UiIntents::of(&tree);
    assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));
    assert_eq!(intents.result_for(ActionSignal::Cancel), Some("gate_resume"));
    assert_eq!(intents.result_for(ActionSignal::TabNext), Some("field_next"));
    assert_eq!(intents.result_for(ActionSignal::TabPrev), Some("field_prev"));
    // And the one that must NOT be declared. Confirm is what ACTIVATES the
    // focused control; a bench that claimed it for its own result would have a
    // pad that can move the focus ring and never press anything.
    assert_eq!(
        intents.result_for(ActionSignal::Confirm),
        None,
        "Confirm belongs to the walker — declaring it kills pad activation"
    );

    let styles = load_styles(HUD_UI_ELEMENTS);
    let snap = UiInput {
        mouse: Vec2::new(-9.0, -9.0),
        clicked: false,
        down: false,
        screen: Vec2::new(1920.0, 1080.0),
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    let has = |cmds: &[HudCommand], s: &str| {
        cmds.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
    };

    // Loading state: the banner shows, the readout does not.
    let loading = ValueMap::new().with("loading", true);
    let cmds = run_ui(&tree, &loading, &styles, &snap, &mut UiState::new()).commands;
    assert!(has(&cmds, "GENERATING PLANET…"), "loading banner renders");
    assert!(!has(&cmds, "FLICKER · CHEMISTRY SIM (M2 · LAYER STACK)"), "readout gated off");

    // Loaded state: readout + ledger lines ride their binds. The fixture's
    // display words ride the SAME stringtable tokens `hud_model` resolves
    // (Model-channel strings gate); the numbers compose around them.
    let r = |t: &str| strings::resolve(t).into_owned();
    let row = |t: &str, pct: f64| format!("{:<11}{pct:>6.2}%", r(t));
    // Composed exactly as `hud_model` composes it — the stage name is an
    // identifier (the string the conservation audit would name), the word
    // rides its token. Bound once so the fixture and its assertion cannot
    // drift apart.
    let gate_line = format!("\u{23f8} {} {}  ·  412 My", "Volcanism", r("$chem_gate_opened"));
    let loaded = ValueMap::new()
        .with("loaded", true)
        .with("stats", format!("{} 42  ·  84 My  ·  92162 {}", r("$chem_tick"), r("$chem_cells")))
        .with("interior", format!("{} 31.0%  ·  {} 88%", r("$chem_core"), r("$chem_differentiated")))
        .with("play_state", r("$chem_playing"))
        .with("play_state_color", "chemistry.playing.color")
        .with("hints", format!("{}{}  {}", r("$chem_hints_head"), "heat", r("$chem_hints_tail")))
        .with("crust", format!("{} 1.2%", r("$chem_crust")))
        .with(
            "air_line",
            format!(
                "{} CO2  ·  {} 12.0 bar  ·  {} +140 K",
                r("$chem_air"),
                r("$chem_pco2"),
                r("$chem_greenhouse")
            ),
        )
        .with("ledger_status", format!("Σ {}  ·  {}", "5.972e24 kg", r("$chem_balanced")))
        .with("ledger_status_color", "chemistry.ok")
        .with("ledger_1", row("$chem_ledger_mantle", 68.0))
        .with("ledger_2", row("$chem_ledger_core", 31.0))
        .with("ledger_3", row("$chem_ledger_crust", 0.5))
        .with("ledger_4", row("$chem_ledger_atmosphere", 0.3))
        .with("ledger_5", row("$chem_ledger_ocean", 0.1))
        .with("ledger_6", row("$chem_ledger_escaped", 0.1))
        .with("a1_name", r("$chem_ax_interior"))
        .with("a1_name_color", "pocepochs.hab.name_live")
        .with("a1_v", 0.25)
        .with("a1_lolab", r("$chem_axlo_dead"))
        .with("a1_hilab", r("$chem_axhi_magma"))
        .with("a1_status", r("$chem_in_band"))
        .with("a1_status_color", "pocepochs.hab.status_in")
        .with("gate_shown", true)
        .with("gate_color", "chemistry.ok")
        .with("gate", gate_line.clone())
        .with("no_life", true)
        .with("verdict", format!("1 / 5 {}", r("$chem_axes_in_band")))
        .with("verdict_color", "pocepochs.hab.verdict_count")
        .with("observed", format!("5 / 5 {}", r("$chem_observed")));
    let cmds = run_ui(&tree, &loaded, &styles, &snap, &mut UiState::new()).commands;
    assert!(!has(&cmds, "GENERATING PLANET…"), "loading banner gated off");
    assert!(has(&cmds, "GOD MODE · WORLD FORGE"), "title renders");
    assert!(has(&cmds, "PLAYING"), "state word rides its bind");
    assert!(has(&cmds, "HEAT"), "the field tabs render");
    assert!(has(&cmds, "BOUNDARY INPUTS"), "the lever rack renders");
    assert!(has(&cmds, "WORLD EVENTS"), "the event bank renders");
    assert!(has(&cmds, "TILE INSPECTOR"), "the tile inspector renders");
    assert!(has(&cmds, "CONSERVATION LEDGER"), "ledger panel renders");
    assert!(has(&cmds, "Mantle      68.00%"), "ledger rows ride their binds");
    assert!(has(&cmds, "Air CO2  ·  pCO₂ 12.0 bar  ·  greenhouse +140 K"), "air line rides its bind");
    assert!(has(&cmds, "LIFE-SUPPORTING CONDITIONS"), "hab panel renders");
    assert!(has(&cmds, "Interior"), "axis name rides its bind");
    assert!(has(&cmds, "1 / 5 axes in band"), "verdict counts the coincidence");
    assert!(has(&cmds, &gate_line), "the gate-transition line says why the run stopped");

    // ── THE PAUSE SUMMARY. Hidden while the run is going; it appears only
    //    when the world has crossed one of its own thresholds and stopped
    //    there, and it says which one, why, and what it means. ──
    assert!(!has(&cmds, "THE WORLD CHANGED"), "no summary while the run is live");

    let why = r("$chem_why_volcanism_open");
    let paused = loaded
        .clone()
        .with("gate_pause_shown", true)
        // The stage name is an IDENTIFIER (the string the conservation
        // audit would name), so it rides as an argument, exactly as
        // `hud_model` composes it — never inline where it would read as copy.
        .with("gate_headline", format!("{} {}", "Volcanism", r("$chem_gate_opened")))
        .with("gate_why", why.clone())
        .with("gate_cause", format!("{} 412 My", r("$chem_gate_at")))
        .with("gate_effect", r("$chem_gate_effect"));
    let cmds = run_ui(&tree, &paused, &styles, &snap, &mut UiState::new()).commands;
    assert!(has(&cmds, "THE WORLD CHANGED"), "the summary appears on a gate pause");
    assert!(
        has(&cmds, &format!("{} {}", "Volcanism", r("$chem_gate_opened"))),
        "it names the gate that moved"
    );
    assert!(has(&cmds, &why), "and says what that MEANS, not just that it happened");
    assert!(has(&cmds, "CARRY ON"), "with the way out declared as a control");
    assert!(has(&cmds, "GATES…"), "and the door to the controls beside it");

    // ── THE GATE CONSOLE. A popup, not a fixture of the screen: rows ride
    //    the SAME proc binds the chip summarises, and every visible row
    //    carries its HOLD/RELEASE control. ──
    // Deliberately built on `loaded` — NOT on the paused fixture. The
    // console must be reachable from a RUNNING world (G, or the chip): the
    // first build was only ever test-rendered over pause state, so nobody
    // could say from the tests whether the pause was load-bearing, and in
    // the window the pause popup's GATES… button became the only door
    // anyone found.
    let console = loaded
        .clone()
        .with("gates_open", true)
        .with("proc_1_shown", true)
        // The stage name is an IDENTIFIER, composed exactly as `hud_model`
        // composes the row — never inline where it would read as copy.
        .with(
            "proc_1",
            format!("\u{25cf} {:<18}{}", "RadiogenicDecay", r("$chem_running")),
        )
        .with("proc_1_color", "chemistry.ok")
        .with("hold_1_label", r("$chem_hold"));
    let console = console.with("water_coverage", 0.75).with("water_infall", 1.0);
    let cmds = run_ui(&tree, &console, &styles, &snap, &mut UiState::new()).commands;
    assert!(has(&cmds, "SIMULATION GATES"), "the console opens on its bind, UNPAUSED");
    assert!(
        !has(&cmds, "THE WORLD CHANGED"),
        "and needs no pause summary on screen to do it"
    );
    assert!(has(&cmds, "INFALL"), "the delivery dial rides beside coverage");
    assert!(
        has(&cmds, &format!("\u{25cf} {:<18}{}", "RadiogenicDecay", r("$chem_running"))),
        "a stage row rides the shared proc binds"
    );
    assert!(has(&cmds, "HOLD"), "and carries its HOLD/RELEASE control");
    assert!(has(&cmds, "WATER"), "the coverage lever rides in the console");
    assert!(has(&cmds, "CLOSE"), "with the way back out");

    // ── THE STARTER. New-world knobs: element endowments (symbols are
    //    catalog DATA riding binds) + the planet's size — and the FORGE
    //    button, without which none of it touches anything. ──
    let starter = console
        .clone()
        .with("starter_open", true)
        .with("seed_el_1_shown", true)
        .with("seed_el_1_label", "H") // strings-gate-exempt: an element SYMBOL is data
        .with("seed_el_1", 1.0)
        .with("seed_freq", 96.0)
        .with("seed_cells", format!("92162 {}", r("$chem_cells")));
    let cmds = run_ui(&tree, &starter, &styles, &snap, &mut UiState::new()).commands;
    assert!(has(&cmds, "THE STARTER"), "the starter opens on its bind");
    assert!(has(&cmds, "MERCURY"), "the preset row stages its input bundles");
    assert!(has(&cmds, "EUROPA"), "from iron ball to ice moon");
    assert!(has(&cmds, "H"), "an endowment row names its element");
    assert!(has(&cmds, "SIZE"), "the size control is back");
    assert!(has(&cmds, "92162 cells"), "and says what the size means");
    assert!(has(&cmds, "FORGE WORLD"), "nothing moves except through the forge");
}

/// **The pixel stage is gated on life** — the design's era gate: the aggregate
/// era ends, and per-pixel rain begins, only once the five-axis light says the
/// world can sustain it. RAIN ON on a dead world does nothing (and the button
/// is disabled through `rain_allowed`); RAIN OFF is always allowed, so a world
/// that slides back out of band can still have its rain shut off.
#[test]
fn rain_waits_for_the_life_light() {
    let mut scene = GodModeScene::new();
    let mut snap = fixture_snapshot();
    snap.habitability.life_supporting = false;
    snap.habitability.axes_in_band = 0;
    scene.snap = Some(snap);

    // Dead world: RAIN ON refuses — no command, no mirror flip.
    let cmds = scene.apply_results(&ValueMap::new().with("erode", true));
    assert!(cmds.is_empty(), "rain must wait for the light");
    assert!(!scene.eroding, "the mirror did not flip");

    // The light turns: the same click starts the rain.
    scene.snap = Some(fixture_snapshot());
    let cmds = scene.apply_results(&ValueMap::new().with("erode", true));
    assert!(matches!(cmds.as_slice(), [SimCommand::ErodeToggle]), "the light admits the rain");
    assert!(scene.eroding);

    // The world slides out of band mid-rain: shutting it OFF is still allowed.
    let mut snap = fixture_snapshot();
    snap.habitability.life_supporting = false;
    scene.snap = Some(snap);
    let cmds = scene.apply_results(&ValueMap::new().with("erode", true));
    assert!(matches!(cmds.as_slice(), [SimCommand::ErodeToggle]), "off is always reachable");
    assert!(!scene.eroding);
}

// ── The view roster ──────────────────────────────────────────────────────────

/// Read `processes.json` the way the sim thread does, from the same directory.
fn shipped_processes() -> Vec<ProcessDef> {
    flicker_poc_chemistry::load_processes(&flicker_poc_chemistry::content_data_dir())
}

/// **The ten views are ten views everywhere.**
///
/// The roster is spread over four places that can each drift on their own: the
/// `Field` enum, `FIELD_ACTIONS`, the authored button strip, and the
/// stringtable. Drift is silent in every direction — a button whose action no
/// arm reads is dead but still draws, a `Field` no button reaches can only be
/// found by cycling, an unresolved token renders as its own name. So all four
/// are pinned to each other here.
#[test]
fn the_view_roster_agrees_with_itself() {
    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    // 1. Every row is distinct, and `cycle` reaches all of them — the tab keys
    //    and the buttons must not disagree about how many views exist.
    let mut seen = Vec::new();
    let mut f = FIELD_ACTIONS[0].1;
    for _ in 0..FIELD_ACTIONS.len() {
        assert!(!seen.contains(&f), "cycle() closes early at {f:?}: {seen:?}");
        seen.push(f);
        f = f.cycle();
    }
    assert_eq!(f, FIELD_ACTIONS[0].1, "cycle() closes the ring");
    for &(_, field, _, _) in FIELD_ACTIONS.iter() {
        assert!(seen.contains(&field), "{field:?} has a button but cycle() never reaches it");
    }

    // 2. Every action is on a real button, and every field button is in the
    //    roster — declared and dispatched are the same set.
    let mut declared = Vec::new();
    declared_actions(&GodModeScene::new().build_tree(), &mut declared);
    for &(action, _, _, _) in FIELD_ACTIONS.iter() {
        assert!(
            declared.iter().any(|d| d == action),
            "'{action}' is in the roster but no button declares it"
        );
    }
    for action in declared.iter().filter(|a| a.starts_with("field_")) {
        assert!(
            FIELD_ACTIONS.iter().any(|&(a, _, _, _)| a == action),
            "a button declares '{action}', which the roster does not have"
        );
    }

    // 3. Every label token resolves. An unresolved token renders as the token,
    //    so this is the difference between a button saying MOTION and one
    //    saying $chem_field_motion.
    for &(action, _, token, _) in FIELD_ACTIONS.iter() {
        let text = flicker::ui::strings::resolve(token);
        assert_ne!(text, token, "'{action}' label {token} is missing from the stringtable");
        assert!(!text.is_empty(), "'{action}' label {token} resolves to nothing");
    }
}

/// **Every view a process names is a view the bench has.** `processes.json` is
/// maintainer-editable, so a typo here is content that looks authored and does
/// nothing: the instrument never lights, the pause card never offers it, and
/// there is no error anywhere to explain why.
#[test]
fn every_authored_view_names_a_real_one() {
    let defs = shipped_processes();
    for p in &defs {
        if p.view.is_empty() {
            continue; // "nothing to see here" is a legal, and often honest, answer
        }
        assert!(
            Field::from_view(&p.view).is_some(),
            "processes.json: '{}' names view '{}', which is not one of {:?}",
            p.runs,
            p.view,
            FIELD_ACTIONS.iter().map(|&(_, f, _, _)| f.view_name()).collect::<Vec<_>>(),
        );
    }
    // And the file is actually USING the mechanism — if every entry were blank
    // this test would pass while the strip stayed uniformly dim forever.
    assert!(
        defs.iter().filter(|p| !p.view.is_empty()).count() >= 5,
        "the pipeline should point at the instruments that show it working"
    );
}

/// **The four grounds.** The pair this exists for is CONTINENTAL-under-water
/// (a shelf) against OCEANIC-under-water (deep floor): every other view paints
/// those the same, which is a large part of why a drowned world reads as one
/// featureless blue ball.
#[test]
fn the_coast_view_separates_the_four_grounds() {
    use crate::sim_thread::{coast_class, SHELF_NONE};
    use flicker_poc_chemistry::CrustKind::*;
    let sea = 100.0;
    let table = [
        (Undifferentiated, 0.0, SHELF_NONE),
        (Undifferentiated, 500.0, SHELF_NONE), // no crust is no crust, wet or dry
        (Continental, 500.0, SHELF_LAND),
        (Continental, 50.0, SHELF_SHELF),
        (Oceanic, 50.0, SHELF_BED),
        (Oceanic, 500.0, SHELF_EXPOSED),
    ];
    for (kind, elev, want) in table {
        assert_eq!(coast_class(kind, elev, sea), want, "{kind:?} at {elev} m under a {sea} m sea");
    }
    // Exactly at sea level is dry land: the sea covers what is BELOW it.
    assert_eq!(coast_class(Continental, sea, sea), SHELF_LAND);

    // The classes are visually distinct, and the coastline reads brighter than
    // the ground it edges — an outline without a shader.
    let colors: Vec<[f32; 3]> =
        [SHELF_NONE, SHELF_LAND, SHELF_SHELF, SHELF_BED, SHELF_EXPOSED]
            .iter()
            .map(|&c| coast_color(c))
            .collect();
    for (i, a) in colors.iter().enumerate() {
        for b in colors.iter().skip(i + 1) {
            let d: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum();
            assert!(d > 0.15, "two grounds paint alike: {a:?} vs {b:?}");
        }
    }
    let lum = |c: [f32; 3]| c[0] + c[1] + c[2];
    for c in [SHELF_LAND, SHELF_SHELF, SHELF_BED, SHELF_EXPOSED] {
        assert!(
            lum(coast_color(c | SHELF_EDGE)) > lum(coast_color(c)),
            "the coastline must stand out from its own ground"
        );
    }
}

/// **The rain map tells the truth about a dry world.** A ramp that stretches to
/// whatever the frame's maximum happens to be would paint a desert as though it
/// had weather — which is exactly the failure the heat view had.
#[test]
fn the_rain_view_stays_dark_on_a_dry_world() {
    assert_eq!(rain_color(0.0, 0.0), rain_color(0.0, 5.0), "no rain reads the same either way");
    let dry = rain_color(0.0, 5.0);
    let wet = rain_color(5.0, 5.0);
    assert!(wet.iter().sum::<f32>() > dry.iter().sum::<f32>(), "rain is brighter than none");
    // Monotone: more rain is never darker.
    let mid = rain_color(1.0, 5.0);
    assert!(mid.iter().sum::<f32>() >= dry.iter().sum::<f32>());
    assert!(wet.iter().sum::<f32>() >= mid.iter().sum::<f32>());
}

/// A cell going `step` of the way toward its next hex, heading east.
fn moving_cell(plate: u32, step: f32) -> CellView {
    CellView {
        temp_k: 1000.0,
        differentiation: 0.0,
        beds: BED_OCEANIC,
        plate,
        seam: 0,
        strata: 1,
        elevation_m: 0.0,
        ore: 0.0,
        coast: SHELF_BED,
        rain: 0.0,
        motion_dir: Vec3::Z,
        motion_step: step,
    }
}

/// **The arrows say what they mean.** A heading is drawn only where the ground
/// is actually going somewhere, it grows with how far along its step the column
/// has come, and it is grouped by plate so a raft reads as one body.
#[test]
fn motion_arrows_draw_only_what_is_moving() {
    // A ring of cells around the equator, all drifting east on one plate.
    let n = 600;
    let dirs: Vec<Vec3> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            Vec3::new(a.cos(), 0.0, a.sin())
        })
        .collect();

    // Nothing moving: nothing drawn. (A still world with arrows all over it
    // would be the readout inventing motion.)
    let still: Vec<CellView> = (0..n).map(|_| moving_cell(1, 0.0)).collect();
    assert!(motion_arrows(&dirs, &still, true, |_| true).is_empty(), "a still world draws nothing");

    // Moving: arrows appear, in whole three-segment arrows (shaft + two barbs).
    let moving: Vec<CellView> = (0..n).map(|_| moving_cell(1, 0.5)).collect();
    let groups = motion_arrows(&dirs, &moving, true, |_| true);
    let segments: usize = groups.iter().map(|(_, s)| s.len()).sum();
    assert!(segments > 0, "a drifting world draws headings");
    assert_eq!(segments % 3, 0, "every arrow is a shaft and two barbs");
    assert_eq!(groups.len(), 1, "one plate, one colour group");

    // A longer step is a longer arrow — the read is PROGRESS, so it has to show.
    let shaft = |step: f32| {
        let cells: Vec<CellView> = (0..n).map(|_| moving_cell(1, step)).collect();
        motion_arrows(&dirs, &cells, true, |_| true)
            .first()
            .map(|(_, s)| (s[0].1 - s[0].0).length())
            .unwrap_or(0.0)
    };
    assert!(shaft(0.9) > shaft(0.3) * 2.0, "the arrow grows with the step it is part-way through");

    // Two plates, two colours — and a cutaway hides what it cuts.
    let mixed: Vec<CellView> =
        (0..n).map(|i| moving_cell(if i % 2 == 0 { 1 } else { 2 }, 0.6)).collect();
    assert_eq!(motion_arrows(&dirs, &mixed, true, |_| true).len(), 2, "a colour per plate");
    assert!(
        motion_arrows(&dirs, &mixed, true, |_| false).is_empty(),
        "the cutaway takes the headings with the ground"
    );
}

/// **Every view explains its own colours.** The legend was Aaron's direct ask
/// ("all of the displays need to have legends that are appropriate, either a
/// bullet list of tile colors or a scale for what the range means") — so every
/// Field must publish a title plus rows or a ramp strip, every swatch path must
/// resolve into the GENERATED `legend.*` style block, and that block must carry
/// the same colours the globe is painted with, because it is generated from the
/// same functions.
#[test]
fn every_view_explains_its_colours() {
    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    let mut scene = GodModeScene::new();
    scene.snap = Some(fixture_snapshot());
    scene.inject_legend_styles();

    // A dotted legend path resolves to an RGBA array in the generated block.
    let styles = scene.ui_styles.clone();
    let resolves = |path: &str| -> bool {
        let mut node = &styles;
        for seg in path.split('.') {
            match node.get(seg) {
                Some(v) => node = v,
                None => return false,
            }
        }
        node.as_array().is_some_and(|a| a.len() == 4)
    };

    for &(_, field, _, _) in FIELD_ACTIONS.iter() {
        scene.field = field;
        let mut m = ValueMap::new();
        scene.legend_model(&mut m);

        assert!(m.is_on("legend_shown"), "{field:?} shows a legend at all");

        let strip = m.is_on("legend_strip_shown");
        let rows = m.is_on("legend_r1_shown");
        assert!(strip || rows, "{field:?} explains itself with rows or a scale");

        if strip {
            // Every chip resolves, and the ends are labelled.
            for k in 0..LEGEND_STRIP {
                let path = m.text(&format!("legend_g{k}")).unwrap_or_default();
                assert!(resolves(path), "{field:?} strip chip {k} path {path:?} resolves");
            }
            for end in ["legend_lo", "legend_hi"] {
                let label = m.text(end).unwrap_or_default();
                assert!(
                    !label.is_empty() && !label.starts_with('$'),
                    "{field:?} {end} labels the scale: {label:?}"
                );
            }
        }
        // Every visible row has a resolving swatch and resolved copy.
        for n in 1..=6 {
            if !m.is_on(&format!("legend_r{n}_shown")) {
                continue;
            }
            let path = m.text(&format!("legend_r{n}_c")).unwrap_or_default();
            assert!(resolves(path), "{field:?} row {n} swatch {path:?} resolves");
            let label = m.text(&format!("legend_r{n}")).unwrap_or_default();
            assert!(
                !label.is_empty() && !label.starts_with('$'),
                "{field:?} row {n} label resolves: {label:?}"
            );
        }
    }

    // The generated block IS the paint: a sampled check on both shapes. If
    // someone hand-edits a legend colour into ui_elements.json instead, this is
    // the test that says the sphere and the card no longer agree.
    let chip = |path: &str| -> [f32; 3] {
        let mut node = &styles;
        for seg in path.split('.') {
            node = node.get(seg).expect("path resolves");
        }
        let a = node.as_array().expect("rgba");
        [
            a[0].as_f64().unwrap() as f32,
            a[1].as_f64().unwrap() as f32,
            a[2].as_f64().unwrap() as f32,
        ]
    };
    assert_eq!(chip("legend.coast_shelf"), coast_color(SHELF_SHELF), "shelf chip = shelf paint");
    assert_eq!(chip("legend.heat_0"), temp_color(0.0), "cold end of the heat strip");
    assert_eq!(
        chip(&format!("legend.heat_{}", LEGEND_STRIP - 1)),
        temp_color(1.0),
        "hot end of the heat strip"
    );
    assert_eq!(chip("legend.seam_conv"), seam_color(2), "convergent chip = seam paint");
}
