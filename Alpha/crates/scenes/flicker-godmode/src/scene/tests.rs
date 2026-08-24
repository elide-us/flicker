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
    assert!(
        (c - 0.1).abs() < 1e-9,
        "sqrt(100/1e4) = 0.1 passes through untouched: {c}"
    );
}

/// **A rebirth clears the gate acknowledgement.** The sim clears its gate log
/// on Reset and on a forge, so the scene's read-high-water must clear WITH it —
/// otherwise the second run's first gate (same stage, same `at_myr`) reads as
/// already-acknowledged and the pause summary never shows again: the run stops
/// and says nothing (the second-run silence Aaron hit in-window).
#[test]
fn a_rebirth_clears_the_gate_acknowledgement() {
    let mut scene = GodModeScene::shipped();
    scene.snap = Some(fixture_snapshot());

    // The maintainer read a gate at 500 My, then pressed RESET.
    scene.gate_ack = 500.0;
    let cmds = scene.apply_results(&ValueMap::new().with("reset", true));
    assert!(
        matches!(cmds.as_slice(), [SimCommand::Reset]),
        "reset asks the sim to rebirth"
    );
    assert_eq!(
        scene.gate_ack,
        f64::NEG_INFINITY,
        "reset clears the read high-water"
    );

    // Same story through the transport's RESEED (= the Starter's FORGE).
    scene.gate_ack = 500.0;
    let cmds = scene.apply_results(&ValueMap::new().with("reseed", true));
    assert!(
        matches!(cmds.as_slice(), [SimCommand::Reseed(_)]),
        "reseed forges a new world with a fresh seed"
    );
    assert_eq!(
        scene.gate_ack,
        f64::NEG_INFINITY,
        "a forged world has no read gates"
    );
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
        // them. The authored console carries PROCESS_ROWS rows against the 22
        // shipped stages — the spare rows ride `proc_<n>_shown = false` in the
        // app, but a row that IS backed must always hold, and that is the
        // contract under test.
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
        let mut scene = GodModeScene::shipped();
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
    let mut scene = GodModeScene::shipped();
    scene.snap = Some(fixture_snapshot());
    // The fixture's levers ARE the defaults, so 1.0 is exactly the echo.
    let echo = LEVERS
        .iter()
        .fold(ValueMap::new(), |m, &(key, _, _)| m.with(key, 1.0));
    assert!(
        scene.apply_results(&echo).is_empty(),
        "an unmoved rack is silent"
    );

    // And the rate dial, the same way.
    let mut scene = GodModeScene::shipped();
    scene.snap = Some(fixture_snapshot());
    let at_rest = scene.snap.as_ref().expect("primed").rate_hz as f64;
    assert!(
        scene
            .apply_results(&ValueMap::new().with("rate", at_rest))
            .is_empty(),
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

    let mut scene = GodModeScene::shipped();
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
    assert!(
        m.is_on("life_light"),
        "a life-supporting world lights the lamp"
    );
    assert!(
        m.text("life").is_some_and(|s| !s.is_empty()),
        "and the life line is still its own text: {:?}",
        m.get("life")
    );
}

/// **The pixel stage is gated on life** — the design's era gate: the aggregate
/// era ends, and per-pixel rain begins, only once the five-axis light says the
/// world can sustain it. RAIN ON on a dead world does nothing (and the button
/// is disabled through `rain_allowed`); RAIN OFF is always allowed, so a world
/// that slides back out of band can still have its rain shut off.
#[test]
fn rain_waits_for_the_life_light() {
    let mut scene = GodModeScene::shipped();
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
    assert!(
        matches!(cmds.as_slice(), [SimCommand::ErodeToggle]),
        "the light admits the rain"
    );
    assert!(scene.eroding);

    // The world slides out of band mid-rain: shutting it OFF is still allowed.
    let mut snap = fixture_snapshot();
    snap.habitability.life_supporting = false;
    scene.snap = Some(snap);
    let cmds = scene.apply_results(&ValueMap::new().with("erode", true));
    assert!(
        matches!(cmds.as_slice(), [SimCommand::ErodeToggle]),
        "off is always reachable"
    );
    assert!(!scene.eroding);
}

// ── The view roster ──────────────────────────────────────────────────────────

/// Read `processes.json` the way the sim thread does, from the same directory.
fn shipped_processes() -> Vec<ProcessDef> {
    flicker_poc_chemistry::load_processes(&flicker_poc_chemistry::content_data_dir())
}

/// **The ten views are ten views everywhere.**
///
/// The roster is spread over places that can each drift on their own: the
/// `Field` enum, `FIELD_ACTIONS`, and the stringtable. Drift is silent in every
/// direction — a `Field` no button reaches can only be found by cycling, an
/// unresolved token renders as its own name. So they are pinned to each other
/// here.
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
        assert!(
            !seen.contains(&f),
            "cycle() closes early at {f:?}: {seen:?}"
        );
        seen.push(f);
        f = f.cycle();
    }
    assert_eq!(f, FIELD_ACTIONS[0].1, "cycle() closes the ring");
    for &(_, field, _, _) in FIELD_ACTIONS.iter() {
        assert!(
            seen.contains(&field),
            "{field:?} has a button but cycle() never reaches it"
        );
    }

    // 2. Every label token resolves. An unresolved token renders as the token,
    //    so this is the difference between a button saying MOTION and one
    //    saying $chem_field_motion.
    for &(action, _, token, _) in FIELD_ACTIONS.iter() {
        let text = flicker::ui::strings::resolve(token);
        assert_ne!(
            text, token,
            "'{action}' label {token} is missing from the stringtable"
        );
        assert!(
            !text.is_empty(),
            "'{action}' label {token} resolves to nothing"
        );
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
            FIELD_ACTIONS
                .iter()
                .map(|&(_, f, _, _)| f.view_name())
                .collect::<Vec<_>>(),
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
        assert_eq!(
            coast_class(kind, elev, sea),
            want,
            "{kind:?} at {elev} m under a {sea} m sea"
        );
    }
    // Exactly at sea level is dry land: the sea covers what is BELOW it.
    assert_eq!(coast_class(Continental, sea, sea), SHELF_LAND);

    // The classes are visually distinct, and the coastline reads brighter than
    // the ground it edges — an outline without a shader.
    let colors: Vec<[f32; 3]> = [
        SHELF_NONE,
        SHELF_LAND,
        SHELF_SHELF,
        SHELF_BED,
        SHELF_EXPOSED,
    ]
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
    assert_eq!(
        rain_color(0.0, 0.0),
        rain_color(0.0, 5.0),
        "no rain reads the same either way"
    );
    let dry = rain_color(0.0, 5.0);
    let wet = rain_color(5.0, 5.0);
    assert!(
        wet.iter().sum::<f32>() > dry.iter().sum::<f32>(),
        "rain is brighter than none"
    );
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
    assert!(
        motion_arrows(&dirs, &still, true, |_| true).is_empty(),
        "a still world draws nothing"
    );

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
    assert!(
        shaft(0.9) > shaft(0.3) * 2.0,
        "the arrow grows with the step it is part-way through"
    );

    // Two plates, two colours — and a cutaway hides what it cuts.
    let mixed: Vec<CellView> = (0..n)
        .map(|i| moving_cell(if i % 2 == 0 { 1 } else { 2 }, 0.6))
        .collect();
    assert_eq!(
        motion_arrows(&dirs, &mixed, true, |_| true).len(),
        2,
        "a colour per plate"
    );
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

    let mut scene = GodModeScene::shipped();
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
        assert!(
            strip || rows,
            "{field:?} explains itself with rows or a scale"
        );

        if strip {
            // Every chip resolves, and the ends are labelled.
            for k in 0..LEGEND_STRIP {
                let path = m.text(&format!("legend_g{k}")).unwrap_or_default();
                assert!(
                    resolves(path),
                    "{field:?} strip chip {k} path {path:?} resolves"
                );
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
    // someone hand-edits a legend colour into ui_theme.json instead, this is
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
    assert_eq!(
        chip("legend.coast_shelf"),
        coast_color(SHELF_SHELF),
        "shelf chip = shelf paint"
    );
    assert_eq!(
        chip("legend.heat_0"),
        temp_color(0.0),
        "cold end of the heat strip"
    );
    assert_eq!(
        chip(&format!("legend.heat_{}", LEGEND_STRIP - 1)),
        temp_color(1.0),
        "hot end of the heat strip"
    );
    assert_eq!(
        chip("legend.seam_conv"),
        seam_color(2),
        "convergent chip = seam paint"
    );
}

// ── The scene pair (five-line split) ─────────────────────────────────────────

/// **The shipped scene file authors this bench.** The tree parses, the screen
/// declares the pause + view-cycle intents, the globe has its `rtt` slot, the
/// gauge rows refilled from the observer's bands, and every component kind and
/// display literal in the file is legal — the same def the manifest hands the
/// runtime, exercised without an app.
#[test]
fn the_shipped_scene_authors_the_bench() {
    let scene = GodModeScene::shipped();
    let tree = scene.authored.as_ref().expect("the def declares a tree");

    // The declared intents (S9): pause + the pad's view cycle.
    for (intent, name) in [
        ("on_menu", "pause_open"),
        ("on_tab_next", "field_next"),
        ("on_tab_prev", "field_prev"),
    ] {
        assert_eq!(
            tree.props.get(intent),
            Some(&Value::Text(name.into())),
            "the screen declares {intent} = {name}"
        );
    }

    fn find<'a>(n: &'a UiNode, id: &str) -> Option<&'a UiNode> {
        if n.id == id {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, id))
    }
    let globe = find(tree, GLOBE_SLOT).expect("the globe's rtt slot is authored");
    assert_eq!(
        globe.component, "surface",
        "the walker reserves the globe's rect"
    );
    let rows = find(tree, "gm_hab_rows").expect("the gauge-row refill container is authored");
    assert_eq!(
        rows.children.len(),
        flicker_poc_chemistry::habitability::BANDS.len(),
        "one refilled row per condition axis, bands from the observer"
    );

    // The component vocabulary + the display-literal law, over the whole tree.
    assert!(
        flicker::ui::unknown_kinds(tree).is_empty(),
        "unknown component kinds: {:?}",
        flicker::ui::unknown_kinds(tree)
    );
    assert!(
        flicker::ui::raw_display_literals(tree).is_empty(),
        "raw display literals (labels must ride $tokens): {:?}",
        flicker::ui::raw_display_literals(tree)
    );

    // …and the same law at the Rust publish seam: no raw display copy enters
    // the Model from scene.rs (state words and dotted paths are wiring).
    let flags = strings::raw_model_publish_literals(include_str!("../scene.rs"));
    assert!(flags.is_empty(), "raw Model-publish copy: {flags:?}");
}

/// **The declared pause intent fires through the AUTHORED tree** — the walker
/// layer consumes the Menu press and fires the screen's `on_menu` name; the
/// scene maps it onto the pause push. (Replaces the dead route.rs chain test:
/// the pump owns the chain now, and the walker is its only scene layer.)
#[test]
fn dispatch_fires_the_declared_pause_intent() {
    use flicker::ui::WalkerHandler;
    use flicker_input_core::{ActionSignal, EventKind, InputContext, InputState};
    use flicker_input_router::{InputEvent, InputHandler, RouteCtx, Router};

    let scene = GodModeScene::shipped();
    let raw = InputState::new();
    let events = [InputEvent::new(
        ActionSignal::Menu,
        EventKind::Press,
        InputContext::World,
        &raw,
    )];
    let mut ui = UiState::new();
    let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&scene.ui_intents);
    let mut rc = RouteCtx::new();
    {
        let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
        Router::dispatch(&events, &mut chain, &mut rc);
    }
    assert_eq!(
        walker.take_fired(),
        vec!["pause_open".to_string()],
        "the screen's declared on_menu fires as a result name"
    );
}

/// **The pair script derives the state words over the raw publish.** The raw
/// Model carries state (`playing`, `proc_<n>_state`, `a<n>_live`…); godmode.lua
/// turns it into the words, glyphs and style paths the tree binds — so the
/// walked surface only works if the pair actually met.
#[test]
fn the_pair_script_derives_the_state_words() {
    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");

    let mut scene = GodModeScene::shipped();
    scene.snap = Some(fixture_snapshot());
    let m = scene.model();

    for key in [
        "play_state",
        "play_label",
        "proc_1",
        "proc_summary",
        "verdict",
        "ledger_status",
    ] {
        assert!(
            m.text(key)
                .is_some_and(|s| !s.is_empty() && !s.starts_with('$')),
            "derive() must yield display TEXT for '{key}': {:?}",
            m.get(key)
        );
    }
    // The fixture opens on Temperature: its tab is the ACTIVE variant, and a
    // fixture with every stage running marks the console rows with the run glyph.
    assert_eq!(
        m.text("field_temperature_style"),
        Some("modal.buttons.variants.primary"),
        "the active view's tab is primary"
    );
    assert!(
        m.text("proc_1").is_some_and(|s| s.contains('\u{25cf}')),
        "a running stage carries the run glyph: {:?}",
        m.get("proc_1")
    );
}

/// **GATE — God Mode does not double-light its globe.** The bench's shells used to
/// carry a half-lambert wrap baked into their vertex COLOUR
/// (`dirs[i].dot(Vec3::new(0.4, 0.7, 0.55).normalize()) * 0.5 + 0.5`), applied on top of
/// the real `studio` shading its own stage already asks for. Its light direction sat
/// about 7° from `studio`'s sun, so the hack was squaring the same key light — a hard,
/// doubled terminator instead of a correctly-soft one.
///
/// A shell colour is now a function of the CELL (its temperature, its rock, its gas),
/// never of the cell's DIRECTION: the terminator across the globe has exactly ONE
/// source, the stage rig (`the_authored_globe_stage_is_read` gates that it is lit).
/// This reads the module's own source because the colour closures are built inside
/// `render`, behind a `Renderer` no headless test can hold — the same
/// `include_str!("../scene.rs")` seam the publish-literal gate above uses.
#[test]
fn godmode_does_not_double_light() {
    let src = include_str!("../scene.rs");
    for banned in [
        // The hack's own light vector, and the closure that applied it.
        "Vec3::new(0.4, 0.7, 0.55)",
        "let lit =",
        "lit(i,",
        // Any resurrection of a per-cell lambert baked into a colour.
        "dot(light)",
    ] {
        assert!(
            !src.contains(banned),
            "scene.rs lights its shell colours itself (`{banned}`) on top of the stage \
             rig — the globe would take its terminator from two lights at once"
        );
    }
    // And the direction table is used for GEOMETRY only: no colour closure reads it.
    assert!(
        !src.contains("dirs[i].dot("),
        "a shell colour that reads the cell's direction is a second light"
    );
}
