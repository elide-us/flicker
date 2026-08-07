//! Two families, both against the REAL thing.
//!
//! * **The drift gates** walk the tree `build_tree` actually builds — expanded, so
//!   no assertion is made over a template node whose contents were never opened —
//!   and assert the authored vocabulary, the localized copy on both channels, the
//!   declared intents, and the absence of a per-scene HUD script.
//! * **The rules** drive [`sim::Game`] on exact seeds: separation, fuel, the
//!   approach, the departure gate, and every refusal the command panel can earn.

use flicker::render::Vec2;
use flicker::script::{ScriptHost, UiNode};
use flicker::ui::{raw_display_literals, strings, unknown_kinds, UiIntents, UI_COMPONENT_MODULES};
use flicker_input_core::ActionSignal;

use super::*;

const SCREEN: Vec2 = Vec2::new(1920.0, 1080.0);

/// A scene with its content loaded, exactly as `enter` leaves it minus the GPU.
fn console() -> Atc {
    let mut a = Atc::with_seed(7);
    a.ui_styles = load_styles(HUD_UI_ELEMENTS);
    a.layout = Layout::from_styles(&a.ui_styles);
    a
}

/// The stringtable, loaded once so `$token` resolution in the Model is the real
/// thing. The table is process-wide, so every test that needs it calls this.
fn load_stringtable() {
    static PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/data/stringtable.json");
    let json = std::fs::read_to_string(PATH).expect("stringtable.json");
    strings::load_str(&json, "en-us");
}

// ── the drift gates ──────────────────────────────────────────────────────────

/// The console is a DATA proto, not a per-scene HUD script. Sablework regressed on
/// exactly this and only an absence gate catches it.
#[test]
fn the_scene_ships_no_hud_script() {
    let stale = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/sensorium/scripts/hud_atc.lua"
    );
    assert!(
        !std::path::Path::new(stale).exists(),
        "composition is DATA: the console lives in ui_templates.json, never in a hud_atc.lua"
    );
}

/// The vocabulary gate over the REAL tree: every kind is one the engine knows, and
/// — because `unknown_kinds` reports an unresolved proto as `template:<name>` —
/// nothing survived un-expanded.
#[test]
fn the_tree_is_well_formed_and_fully_expanded() {
    let tree = console().build_tree(SCREEN);
    assert!(unknown_kinds(&tree).is_empty(), "unknown kinds: {:?}", unknown_kinds(&tree));
    assert!(!has_template(&tree), "every template instance expanded");
}

fn has_template(n: &UiNode) -> bool {
    n.template.is_some()
        || n.children.iter().any(has_template)
        || n.slots.values().flatten().any(has_template)
}

/// The strings gate on BOTH channels: every display literal in the tree is a
/// `$token`, and the crate self-gates the Model publish seam the tree walk is
/// structurally blind to.
#[test]
fn every_display_string_is_localized() {
    let tree = console().build_tree(SCREEN);
    let raw = raw_display_literals(&tree);
    assert!(raw.is_empty(), "raw display literals in the console: {raw:?}");

    let flags = strings::raw_model_publish_literals(include_str!("lib.rs"));
    assert!(flags.is_empty(), "raw display copy published into the Model: {flags:?}");
}

/// Every `$token` the console names resolves. A missing token renders raw, which
/// is visible but only to whoever happens to look at that screen.
#[test]
fn every_token_the_console_names_is_in_the_stringtable() {
    load_stringtable();
    let mut missing = Vec::new();
    let mut seen = |s: &str| {
        if s.starts_with('$') && !s.starts_with("$$") && strings::resolve(s) == s {
            missing.push(s.to_string());
        }
    };
    fn walk(n: &UiNode, f: &mut impl FnMut(&str)) {
        for (key, v) in &n.props {
            if let flicker::script::Value::Text(s) = v {
                if key != "style" && key != "style_bind" {
                    f(s);
                }
            }
        }
        n.children.iter().for_each(|c| walk(c, f));
        n.slots.values().flatten().for_each(|c| walk(c, f));
    }
    walk(&console().build_tree(SCREEN), &mut seen);
    // The Rust side's own tokens ride the same table.
    for t in [
        "$atc_hint_pick",
        "$atc_score_landed",
        "$atc_score_departed",
        "$atc_score_sweeps",
    ] {
        seen(t);
    }
    for r in [
        Reject::NoSuchFlight,
        Reject::OnTheGround,
        Reject::Airborne,
        Reject::WrongDirection,
        Reject::OutOfRange,
    ] {
        seen(reject_token(r));
    }
    for e in [
        Event::Entered('A'),
        Event::Ready('A'),
        Event::Landed('A'),
        Event::Departed('A'),
        Event::LowFuel('A'),
        Event::WentAround('A'),
    ] {
        seen(event_token(e));
    }
    for v in [
        Verdict::Conflict('A', 'B'),
        Verdict::OutOfFuel('A'),
        Verdict::WrongExit('A'),
        Verdict::WrongAltitude('A'),
        Verdict::Cleared,
    ] {
        let (a, b) = verdict_tokens(v);
        seen(a);
        seen(b);
    }
    assert!(missing.is_empty(), "tokens with no stringtable entry: {missing:?}");
}

/// The screen declares exactly what it dispatches — a bound signal with no arm is
/// dead hardware — and deliberately does NOT declare `Confirm`, because that would
/// displace the walker's "activate the focused control" and leave a pad user unable
/// to press DISPATCH.
#[test]
fn the_screen_declares_only_what_it_dispatches() {
    let intents = UiIntents::of(&console().build_tree(SCREEN));
    assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));
    assert_eq!(intents.result_for(ActionSignal::Cancel), Some("clear_cmd"));
    assert_eq!(intents.result_for(ActionSignal::TabNext), Some("flight_next"));
    assert_eq!(intents.result_for(ActionSignal::TabPrev), Some("flight_prev"));
    assert_eq!(
        intents.result_for(ActionSignal::Confirm),
        None,
        "Confirm stays the walker's — declaring it would kill pad activation"
    );

    // …and every declared name reaches an arm that visibly does something — a name
    // with no arm is dead hardware, and a silent no-op is exactly how that ships.
    let fire = |c: &mut Atc, name: &str| {
        let mut r = ValueMap::new();
        r.set(name.to_string(), true);
        c.apply_results(&r);
    };
    let mut c = console();
    fire(&mut c, "flight_next");
    assert_eq!(c.sel_flight, Some(c.roster()[0]), "flight_next addresses a flight");
    fire(&mut c, "flight_prev");
    assert_eq!(c.sel_flight, Some(*c.roster().last().unwrap()), "flight_prev walks back");

    c.sel_verb = Some(Verb::TurnLeft);
    c.sel_value = Some(90);
    fire(&mut c, "clear_cmd");
    assert_eq!((c.sel_verb, c.sel_value), (None, None), "clear_cmd empties the panel");

    c.log.push(String::new());
    fire(&mut c, "restart");
    assert!(c.log().is_empty() && c.game.sweep == 0, "restart deals a fresh session");
}

/// The console really draws, through the real component library — a broken proto
/// or a missing style path would leave the surface empty.
#[test]
fn the_console_draws() {
    load_stringtable();
    let c = console();
    let tree = c.build_tree(SCREEN);
    let host = ScriptHost::library(UI_COMPONENT_MODULES).expect("component library");
    let mut state = UiState::new();
    let snap = UiInput {
        mouse: Vec2::new(-50.0, -50.0),
        clicked: false,
        down: false,
        screen: SCREEN,
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    let frame =
        run_ui_with(&tree, &c.hud_model(), &c.ui_styles, &snap, &mut state, Some(&host));
    assert!(!frame.commands.is_empty(), "the console draws");
    let says = |needle: &str| {
        frame
            .commands
            .iter()
            .any(|cmd| matches!(cmd, HudCommand::Text { text, .. } if text.contains(needle)))
    };
    assert!(says("FLIGHT STATUS"), "the strip bay is titled");
    assert!(says("DISPATCH"), "the command panel offers the send button");
    assert!(says("A"), "the first aircraft has a strip");
}

/// The command card is tall enough for everything in it. `cmd_h` is a fixed height
/// with a flexible strip bay beneath, so content that outgrows it does not push the
/// bay down — it draws straight over it, which looks like a corrupted layout rather
/// than like an overflow. The bottom log row is the last thing in the card, so its
/// baseline is the measurement that matters.
#[test]
fn the_command_card_holds_its_contents() {
    load_stringtable();
    let mut c = console();
    c.game.aircraft = vec![flying('A', sim::Kind::Prop, 5.0, 6.0, 90)];
    // A distinctive value in the LAST log row, so it can be found in the draw list.
    c.log = (0..LOG_ROWS).map(|i| format!("{i}{i}{i}{i}{i}")).collect();
    let last = c.log[LOG_ROWS - 1].clone();

    let tree = c.build_tree(SCREEN);
    let host = ScriptHost::library(UI_COMPONENT_MODULES).expect("component library");
    let mut state = UiState::new();
    let snap = UiInput {
        mouse: Vec2::new(-50.0, -50.0),
        clicked: false,
        down: false,
        screen: SCREEN,
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    let frame = run_ui_with(&tree, &c.hud_model(), &c.ui_styles, &snap, &mut state, Some(&host));
    let bottom = frame
        .commands
        .iter()
        .find_map(|cmd| match cmd {
            HudCommand::Text { text, y, size, .. } if *text == last => Some(y + size),
            _ => None,
        })
        .expect("the last transmission row draws");
    let inner_bottom = c.layout.margin + c.layout.cmd_h - c.layout.pad;
    assert!(
        bottom <= inner_bottom,
        "the command card overflows: the last log row ends at {bottom:.0}, the card at {inner_bottom:.0} — raise atc.layout.cmd_h"
    );
}

/// THE interaction the whole game is played through, end to end against the real
/// `ui.select` component: clicking the FLIGHT field opens its popup, clicking a row
/// writes that designation back as the `cmd_flight` result, and the scene's
/// dispatcher turns that into the addressed flight. Nothing here knows a pixel
/// coordinate — the field and the row are FOUND, so the gate survives a re-layout.
#[test]
fn the_flight_dropdown_opens_and_picks() {
    load_stringtable();
    let mut c = console();
    c.game.aircraft = vec![
        flying('A', sim::Kind::Prop, 5.0, 6.0, 90),
        flying('B', sim::Kind::Jet, 7.0, 6.0, 90),
    ];
    let tree = c.build_tree(SCREEN);
    let model = c.hud_model();
    let host = ScriptHost::library(UI_COMPONENT_MODULES).expect("component library");
    // Down the middle of the command rail, where every control spans.
    let x = SCREEN.x - c.layout.margin - c.layout.rail_w * 0.5;
    let at = |y: f32, clicked: bool| UiInput {
        mouse: Vec2::new(x, y),
        clicked,
        down: clicked,
        screen: SCREEN,
        typed: String::new(),
        backspace: false,
        wheel: 0.0,
    };
    // How many times a designation is DRAWN. Closed, 'A' appears once — its strip.
    // With the FLIGHT popup open it appears twice, because one of the rows is it.
    let draws_of = |f: &flicker::ui::UiFrame, needle: &str| {
        f.commands
            .iter()
            .filter(|c| matches!(c, HudCommand::Text { text, .. } if text == needle))
            .count()
    };
    let idle = at(-50.0, false);
    let mut base_state = UiState::new();
    let closed = run_ui_with(&tree, &model, &c.ui_styles, &idle, &mut base_state, Some(&host));
    assert_eq!(draws_of(&closed, "A"), 1, "closed, flight A is drawn once — on its strip");

    // Find the field by what a click on it DOES, not by where it is: the frame after
    // it draws the roster a second time, in the open popup.
    let open_at = |y: f32| {
        let mut st = UiState::new();
        run_ui_with(&tree, &model, &c.ui_styles, &at(y, true), &mut st, Some(&host));
        let after = run_ui_with(&tree, &model, &c.ui_styles, &idle, &mut st, Some(&host));
        (st, draws_of(&after, "A") == 2)
    };
    let field_y = (0..500)
        .map(|s| c.layout.margin + s as f32)
        .find(|y| open_at(*y).1)
        .expect("a click down the rail opens the FLIGHT dropdown");

    // Now find the popup row that writes 'B'. Each probe re-opens, because any click
    // closes the popup — which is itself the behaviour being relied on.
    let picked = (1..240)
        .map(|s| field_y + s as f32)
        .find_map(|y| {
            let (mut st, _) = open_at(field_y);
            let f = run_ui_with(&tree, &model, &c.ui_styles, &at(y, true), &mut st, Some(&host));
            (f.results.text("cmd_flight") == Some("B")).then(|| (f.results.clone(), st))
        });
    let (results, after) = picked.expect("one of the popup rows is flight B");
    let closed_again = run_ui_with(&tree, &model, &c.ui_styles, &idle, &mut { after }, Some(&host));
    assert_eq!(draws_of(&closed_again, "A"), 1, "picking a row closes the popup");

    c.apply_results(&results);
    assert_eq!(c.selected(), Some('B'), "the pick addresses the flight");
}

/// The geometry is ONE source: the bezel hugs a SQUARE sector grid, sits inside the
/// room the rail leaves, and is the same rect the scene paints the radar into — so
/// the frame and the picture cannot land in different places, at any window size.
#[test]
fn the_bezel_hugs_the_grid_inside_the_room_the_rail_leaves() {
    let c = console();
    let l = &c.layout;
    // The layout came from the file, not from the loud stand-in.
    assert!(l.margin > 0.0 && l.rail_w > 200.0, "atc.layout was read: {l:?}");

    for screen in [SCREEN, Vec2::new(1280.0, 720.0), Vec2::new(2560.0, 1440.0)] {
        let s = Scope::new(screen, l);
        let room_w = screen.x - l.rail_w - l.gap - l.margin * 2.0;
        assert!(s.w <= room_w + 0.01, "{screen:?}: the bezel fits beside the rail");
        assert!(s.h <= screen.y - l.margin * 2.0 + 0.01, "{screen:?}: …and inside the window");
        assert!(s.x >= l.margin - 0.01 && s.y >= l.margin - 0.01, "{screen:?}: margins hold");
        // Square sectors, filling the bezel exactly once the pad is taken off.
        assert!((s.w - (s.cell * sim::COLS + l.pad * 2.0)).abs() < 0.01, "{screen:?}");
        assert!((s.h - (s.cell * sim::ROWS + l.pad * 2.0)).abs() < 0.01, "{screen:?}");
        // One of the two axes is snug against the room — nothing is wasted.
        let snug = (s.w - room_w).abs() < 0.01 || (s.h - (screen.y - l.margin * 2.0)).abs() < 0.01;
        assert!(snug, "{screen:?}: the grid is as large as the room allows");

        let (gx, gy) = s.sector(s.px(4.0, 7.0));
        assert!((gx - 4.0).abs() < 0.001 && (gy - 7.0).abs() < 0.001, "{screen:?}: px round-trips");
        // Sector (0,0) and (COLS,ROWS) are the bezel's inner corners.
        assert!((s.px(0.0, 0.0).x - (s.x + l.pad)).abs() < 0.01, "{screen:?}");
        assert!((s.px(sim::COLS, sim::ROWS).y - (s.y + s.h - l.pad)).abs() < 0.01, "{screen:?}");
    }
}

/// Every ink the radar draws with resolves. `style_color` answers MAGENTA for a
/// path that names nothing, so a typo is loud on the scope rather than plausible —
/// and this gate makes it loud at build time instead.
#[test]
fn every_scope_ink_resolves() {
    let styles = load_styles(HUD_UI_ELEMENTS);
    const MAGENTA: [f32; 4] = [1.0, 0.0, 1.0, 1.0];
    for key in [
        "face", "grid", "grid_edge", "corridor", "airport", "approach", "marker", "marker_off",
        "hold", "blip", "blip_ground", "blip_low", "blip_sel", "conflict", "tag", "leader",
    ] {
        let path = format!("atc.scope.{key}");
        assert_ne!(style_color(&styles, &path), MAGENTA, "{path} names nothing");
    }
    // …and the gate itself is honest about a path that really is missing.
    assert_eq!(style_color(&styles, "atc.scope.not_a_key"), MAGENTA);
}

// ── the command panel ────────────────────────────────────────────────────────

/// Picking a verb whose value means something else drops the stale value, so a
/// heading can never be sent as an altitude.
#[test]
fn changing_the_verb_drops_the_stale_value() {
    let mut c = console();
    let id = c.game.aircraft[0].id;
    c.sel_flight = Some(id);

    let mut r = ValueMap::new();
    r.set("cmd_verb".to_string(), Verb::TurnLeft.id().to_string());
    r.set("cmd_value".to_string(), "090".to_string());
    c.apply_results(&r);
    assert_eq!(c.sel_value, Some(90), "a heading in the heading domain sticks");

    let mut r = ValueMap::new();
    r.set("cmd_verb".to_string(), Verb::Descend.id().to_string());
    c.apply_results(&r);
    assert_eq!(c.sel_value, None, "the heading did not survive into DH");
    assert_eq!(c.pending(), None, "…and DISPATCH has nothing to send");
}

/// A value outside the picked verb's domain is refused at the panel, before it can
/// reach the sim.
#[test]
fn the_value_domain_is_enforced() {
    let mut c = console();
    c.sel_verb = Some(Verb::Ascend);
    let mut r = ValueMap::new();
    r.set("cmd_value".to_string(), "90".to_string()); // a heading, not an altitude
    c.apply_results(&r);
    assert_eq!(c.sel_value, None);
    assert_eq!(c.value_domain().len(), 10, "ten thousand feet, in thousands");
}

/// The dropdown option lists are the live data: one row per aircraft, and a value
/// list that follows the verb.
#[test]
fn the_option_lists_follow_the_session() {
    let mut c = console();
    assert_eq!(c.flight_options().len(), c.game.aircraft.len());
    assert!(c.value_options().is_empty(), "no verb picked, no values");
    c.sel_verb = Some(Verb::TurnRight);
    assert_eq!(c.value_options().len(), 36, "every heading a controller can say");
    c.sel_verb = Some(Verb::HoldA);
    assert!(c.value_options().is_empty(), "a hold carries no value");
}

/// DISPATCH sends the canonical transmission, logs it, and disarms itself — the
/// panel is never one careless Confirm away from repeating a command.
#[test]
fn dispatch_sends_logs_and_disarms() {
    load_stringtable();
    let mut c = console();
    c.game.aircraft = vec![flying('A', sim::Kind::Prop, 5.0, 6.0, 360)];
    c.sel_flight = Some('A');
    c.sel_verb = Some(Verb::TurnLeft);
    c.sel_value = Some(90);
    assert_eq!(c.pending().map(|(_, cmd)| cmd.code()), Some("TL09".to_string()));

    c.dispatch();
    assert_eq!(c.log().first().map(String::as_str), Some("ATL09"));
    assert_eq!(c.game.find('A').unwrap().target_heading, 90);
    assert_eq!(c.pending(), None, "the verb and value cleared");
}

/// A refused transmission changes nothing and says why, in the player's language.
#[test]
fn a_refusal_logs_the_reason() {
    load_stringtable();
    let mut c = console();
    // 'Z' is the last of the roster and cannot be up yet.
    c.sel_flight = Some('Z');
    c.sel_verb = Some(Verb::Takeoff);
    c.dispatch();
    let line = c.log().first().cloned().unwrap_or_default();
    assert!(line.starts_with('Z'), "the refusal names the flight: {line}");
    assert!(line.contains("no such flight"), "…and the reason, resolved: {line}");
}

/// The bumpers walk the live roster and wrap, so a pad reaches every flight.
#[test]
fn the_bumpers_walk_the_roster() {
    let mut c = console();
    let roster = c.roster();
    c.step_flight(true);
    assert_eq!(c.sel_flight, Some(roster[0]));
    c.step_flight(false);
    assert_eq!(c.sel_flight, Some(*roster.last().unwrap()), "stepping back wraps");
}

/// A click on the radar picks the blip under it, and only if there IS one there.
#[test]
fn a_click_on_a_blip_addresses_it() {
    let mut c = console();
    c.scope = Scope::new(SCREEN, &c.layout);
    let a = c.game.aircraft[0].clone();
    c.pick_at(c.scope.px(a.x, a.y));
    assert_eq!(c.sel_flight, Some(a.id));
    c.sel_flight = None;
    c.pick_at(c.scope.px(a.x + 3.0, a.y + 3.0));
    assert_eq!(c.sel_flight, None, "empty sky picks nothing");
}

// ── the rules ────────────────────────────────────────────────────────────────

/// A turn always ENDS on the commanded heading, whichever way round it was sent,
/// so a heading readout is only ever a number the controller asked for.
#[test]
fn a_turn_lands_exactly_on_the_commanded_heading() {
    // Held on a hold so the aircraft cannot end the session by wandering out mid-turn
    // — this test is about the turn and nothing else.
    for (from, to, right) in [(360, 90, true), (360, 90, false), (130, 310, true), (50, 50, false)] {
        let mut g = solo(1);
        let mut a = flying('A', sim::Kind::Prop, 5.0, 6.0, from);
        a.plan = sim::Plan::Land(0);
        g.aircraft = vec![a];
        let cmd = if right { Cmd::TurnRight(to) } else { Cmd::TurnLeft(to) };
        g.command('A', cmd).expect("a vector is always accepted in the air");
        for _ in 0..8 {
            g.tick();
            if g.find('A').is_none_or(|a| a.heading == to) {
                break;
            }
        }
        assert_eq!(
            g.find('A').map(|a| a.heading),
            Some(to),
            "{from} → {to} the {} way",
            if right { "right" } else { "left" }
        );
        assert!(g.verdict.is_none(), "the turn stayed inside the area");
    }
}

/// Speed is the airframe: a jet covers a sector a sweep and a prop half of one —
/// which is the paper spec's ½-grid increment, arrived at rather than asserted.
#[test]
fn a_jet_covers_twice_the_ground_of_a_prop() {
    let mut g = solo(2);
    g.aircraft.clear();
    for (kind, x) in [(sim::Kind::Jet, 2.0), (sim::Kind::Prop, 8.0)] {
        g.aircraft.push(flying(if kind == sim::Kind::Jet { 'J' } else { 'P' }, kind, x, 6.0, 180));
    }
    g.tick();
    let moved = |id: char| g.find(id).map(|a| a.y - 6.0).unwrap_or_default();
    assert!((moved('J') - 1.0).abs() < 0.001);
    assert!((moved('P') - 0.5).abs() < 0.001);
}

/// Separation is horizontal AND vertical: co-altitude traffic a sector apart is
/// fine, and only closing both dimensions ends the session.
#[test]
fn separation_needs_both_dimensions() {
    // Same altitude, well clear horizontally → no verdict.
    let mut g = solo(4);
    g.aircraft = vec![flying('A', sim::Kind::Prop, 2.0, 2.0, 90), flying('B', sim::Kind::Prop, 8.0, 8.0, 270)];
    g.tick();
    assert!(g.verdict.is_none(), "six sectors apart is not a close encounter");

    // Stacked a thousand feet apart, on top of one another → still clear.
    let mut g = solo(4);
    let mut low = flying('A', sim::Kind::Prop, 5.0, 5.0, 90);
    let mut high = flying('B', sim::Kind::Prop, 5.0, 5.0, 90);
    low.alt = 5000;
    low.target_alt = 5000;
    high.alt = 6000;
    high.target_alt = 6000;
    g.aircraft = vec![low, high];
    g.tick();
    assert!(g.verdict.is_none(), "a thousand feet of stack is legal separation");

    // Co-altitude and on top of one another → the game is over.
    let mut g = solo(4);
    g.aircraft = vec![flying('A', sim::Kind::Prop, 5.0, 5.0, 90), flying('B', sim::Kind::Prop, 5.2, 5.0, 90)];
    g.tick();
    assert!(matches!(g.verdict, Some(Verdict::Conflict(..))), "got {:?}", g.verdict);
}

/// An aircraft on the ground is not flying: two of them at the same field are not
/// a close encounter.
#[test]
fn traffic_on_the_ground_cannot_lose_separation() {
    let mut g = solo(5);
    let mut a = flying('A', sim::Kind::Prop, 3.4, 3.2, 130);
    let mut b = flying('B', sim::Kind::Prop, 3.4, 3.2, 130);
    a.phase = Phase::Ready;
    b.phase = Phase::Ready;
    a.base = Some(0);
    b.base = Some(0);
    g.aircraft = vec![a, b];
    g.tick();
    assert!(g.verdict.is_none());
}

/// The approach: aligned with the runway and at pattern altitude at the outer
/// marker is a landing; either one wrong is a go-around and the aircraft flies on.
#[test]
fn the_approach_is_alignment_and_altitude_at_the_marker() {
    for (alt, heading_off, lands) in [(sim::FIELD_ALT, 0, true), (sim::FIELD_ALT, 90, false), (3000, 0, false)] {
        let mut g = solo(6);
        let hdg = g.landing_heading(0);
        let (mx, my) = g.outer_marker(0);
        let (dx, dy) = sim::track(hdg);
        // A prop half a sector short of the marker, tracking through it.
        let mut a = flying('A', sim::Kind::Prop, mx - dx * 0.5, my - dy * 0.5, sim::wrap(hdg + heading_off));
        a.target_heading = a.heading;
        a.alt = alt;
        a.target_alt = alt;
        a.plan = sim::Plan::Land(0);
        g.aircraft = vec![a];
        let events = g.tick();
        let landed = events.iter().any(|e| matches!(e, Event::Landed('A')));
        assert_eq!(landed, lands, "alt {alt}, {heading_off}° off");
        if lands {
            assert_eq!(g.landed, 1);
            assert!(g.aircraft.is_empty(), "a landing leaves the scope");
        } else {
            assert!(events.iter().any(|e| matches!(e, Event::WentAround('A'))));
            assert_eq!(g.aircraft.len(), 1, "a go-around stays up");
        }
    }
}

/// Leaving the area is the assigned corridor at ten thousand feet — anything else
/// is one of the two departure losses.
#[test]
fn the_departure_gate_is_the_corridor_and_the_altitude() {
    let zmp = CORRIDORS.iter().position(|c| c.id == "ZMP").expect("ZMP");
    let zdv = CORRIDORS.iter().position(|c| c.id == "ZDV").expect("ZDV");

    // Right corridor, right altitude.
    let mut g = solo(8);
    let c = &CORRIDORS[zmp];
    let mut a = flying('A', sim::Kind::Jet, c.x - 0.4, c.y, c.exit_heading);
    a.plan = sim::Plan::Depart(zmp);
    g.aircraft = vec![a];
    assert!(g.tick().iter().any(|e| matches!(e, Event::Departed('A'))));
    assert_eq!(g.departed, 1);
    // A clean departure never LOSES. On a solo deal it also empties the scope, which
    // is the win — so the only verdict allowed here is that one.
    assert!(g.verdict.is_none_or(Verdict::is_win), "got {:?}", g.verdict);

    // Right corridor, wrong altitude.
    let mut g = solo(8);
    let mut a = flying('A', sim::Kind::Jet, c.x - 0.4, c.y, c.exit_heading);
    a.plan = sim::Plan::Depart(zmp);
    a.alt = 4000;
    a.target_alt = 4000;
    g.aircraft = vec![a];
    g.tick();
    assert!(matches!(g.verdict, Some(Verdict::WrongAltitude('A'))), "got {:?}", g.verdict);

    // Wrong corridor entirely.
    let mut g = solo(8);
    let mut a = flying('A', sim::Kind::Jet, c.x - 0.4, c.y, c.exit_heading);
    a.plan = sim::Plan::Depart(zdv);
    g.aircraft = vec![a];
    g.tick();
    assert!(matches!(g.verdict, Some(Verdict::WrongExit('A'))), "got {:?}", g.verdict);
}

/// Fifteen minutes of fuel, a call at ten, and the session ends when it runs out.
#[test]
fn fuel_calls_at_ten_minutes_and_ends_the_session_at_fifteen() {
    let mut g = solo(9);
    g.aircraft = vec![holding('A')];
    let mut called = false;
    for _ in 0..sim::FUEL_TICKS {
        called |= g.tick().iter().any(|e| matches!(e, Event::LowFuel('A')));
        if g.verdict.is_some() {
            break;
        }
    }
    assert!(called, "the pilot calls low on fuel");
    assert!(matches!(g.verdict, Some(Verdict::OutOfFuel('A'))), "got {:?}", g.verdict);
    assert_eq!(g.sweep, u64::from(sim::FUEL_TICKS), "fifteen minutes, to the sweep");
}

/// A hold parks an aircraft on its fix instead of letting it wander off the scope.
#[test]
fn a_hold_keeps_an_aircraft_on_its_fix() {
    let mut g = solo(10);
    let mut a = flying('A', sim::Kind::Prop, HOLDS[0].x, HOLDS[0].y - 2.0, 180);
    a.plan = sim::Plan::Land(0);
    g.aircraft = vec![a];
    g.command('A', Cmd::HoldAt(0)).expect("a hold is accepted in the air");
    for _ in 0..12 {
        g.tick();
        assert!(g.verdict.is_none(), "a held aircraft never leaves the area");
    }
    let a = g.find('A').expect("still up");
    let d = (a.x - HOLDS[0].x).hypot(a.y - HOLDS[0].y);
    assert!(d < 2.0, "it is orbiting the fix, {d:.2} sectors out");
}

/// A take-off is a `CT` and nothing else, and it leaves the field at pattern
/// altitude on the runway in use.
#[test]
fn cleared_for_takeoff_is_the_only_way_off_the_ground() {
    let mut g = solo(11);
    let mut a = flying('A', sim::Kind::Jet, AIRPORTS[0].x, AIRPORTS[0].y, 130);
    a.phase = Phase::Ready;
    a.alt = 0;
    a.target_alt = 0;
    a.base = Some(0);
    a.plan = sim::Plan::Depart(0);
    g.aircraft = vec![a];

    assert_eq!(g.command('A', Cmd::TurnLeft(90)), Err(Reject::OnTheGround));
    assert_eq!(g.command('A', Cmd::DescendHold(0)), Err(Reject::OnTheGround));
    let expected = g.landing_heading(0);
    assert_eq!(g.command('A', Cmd::ClearedTakeoff), Ok("ACT".to_string()));
    let a = g.find('A').expect("rolling");
    assert_eq!((a.phase, a.alt, a.heading), (Phase::Airborne, sim::FIELD_ALT, expected));
    assert_eq!(g.command('A', Cmd::ClearedTakeoff), Err(Reject::Airborne), "once is enough");
}

/// A climb is up and a descent is down: the panel cannot smuggle one past as the
/// other, and neither may leave the ten-thousand-foot band.
#[test]
fn altitude_commands_hold_their_direction_and_their_band() {
    let mut g = solo(12);
    g.aircraft = vec![flying('A', sim::Kind::Prop, 5.0, 5.0, 90)]; // at 10,000
    assert_eq!(g.command('A', Cmd::AscendHold(sim::ENTRY_ALT)), Err(Reject::WrongDirection));
    assert_eq!(g.command('A', Cmd::DescendHold(sim::ENTRY_ALT)), Err(Reject::WrongDirection));
    assert_eq!(g.command('A', Cmd::AscendHold(12_000)), Err(Reject::OutOfRange));
    assert_eq!(g.command('A', Cmd::TurnLeft(95)), Err(Reject::OutOfRange), "headings are tens");
    assert_eq!(g.command('B', Cmd::TurnLeft(90)), Err(Reject::NoSuchFlight));
    assert_eq!(g.command('A', Cmd::DescendHold(3000)), Ok("ADH03".to_string()));
    for _ in 0..7 {
        g.tick();
    }
    assert_eq!(g.find('A').map(|a| a.alt), Some(3000), "it stopped where it was told");
}

/// A session deals the whole roster and only the whole roster, and the wind picks
/// a real runway direction at each field.
#[test]
fn a_session_deals_the_whole_roster() {
    let mut g = Game::new(13);
    for (i, dir) in g.landing_dir.iter().enumerate() {
        assert!(*dir < 2, "field {i} landing direction is one of the two runway ends");
        assert!(AIRPORTS[i].axis.contains(&g.landing_heading(i)));
    }
    let mut seen: Vec<char> = g.aircraft.iter().map(|a| a.id).collect();
    // Fly a long session with no commands; it will end badly, which is the point —
    // what is asserted is that designations are unique and bounded by the roster.
    for _ in 0..400 {
        for e in g.tick() {
            if matches!(e, Event::Entered(_) | Event::Ready(_)) {
                seen.push(e.flight());
            }
        }
        if g.verdict.is_some() {
            break;
        }
    }
    assert!(seen.len() <= sim::ROSTER, "never more than the roster: {}", seen.len());
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), seen.len(), "designations are unique: {seen:?}");
    assert!(seen.iter().all(char::is_ascii_uppercase));
}

// ── fixtures ─────────────────────────────────────────────────────────────────

/// A session whose roster is exhausted the moment it is dealt, so a rule under
/// test is never interrupted by fresh traffic checking in.
fn solo(seed: u64) -> Game {
    Game::with_roster(seed, 1)
}

/// An airborne aircraft at ten thousand feet, steady on `heading`.
fn flying(id: char, kind: sim::Kind, x: f32, y: f32, heading: i32) -> Aircraft {
    Aircraft {
        id,
        kind,
        x,
        y,
        heading,
        target_heading: heading,
        turn_right: true,
        alt: sim::ENTRY_ALT,
        target_alt: sim::ENTRY_ALT,
        plan: sim::Plan::Depart(0),
        phase: Phase::Airborne,
        fuel: sim::FUEL_TICKS,
        entry: Some(0),
        base: None,
    }
}

/// One already orbiting hold A, so a fuel test never ends by wandering off.
fn holding(id: char) -> Aircraft {
    let mut a = flying(id, sim::Kind::Prop, HOLDS[0].x, HOLDS[0].y, 90);
    a.phase = Phase::Holding(0);
    a.plan = sim::Plan::Land(0);
    a
}
