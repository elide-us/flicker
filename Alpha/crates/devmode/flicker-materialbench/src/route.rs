//! The dispatcher — the ONE place a signal becomes an edit.
//!
//! Both input channels arrive here as one `ValueMap` of results (a pointer click sets a
//! node id, a fired `ActionSignal` sets its intent name, so a pad and a mouse are
//! indistinguishable by the time an edit happens), and the graph canvas's gestures arrive
//! as its `CanvasEvents`. Every edit either asks for is ONE [`Designer`] mutation —
//! nothing else in the bench writes the document.
//!
//! [`apply`] and [`canvas`] return **true only when the image changed**: that answer
//! gates the re-bake. Hovering, selecting, switching views, zooming the lit sample, binding
//! a slot and every control echoing the value it was handed return false and cost no bake —
//! save a slot pick that OPENS the slot's committed look (`MaterialBench::bind_slot`), a
//! whole new document.
//!
//! # An edit, not an echo — against the Model the frame was DRAWN from
//!
//! Every control is compared against the value the Model handed it at the START of the
//! frame (`published`), and a SOURCE select's option index is read through the candidate
//! lists of that same moment (the private `Start`) — never against the document as an earlier
//! dispatch in the same frame left it. So a glow colour picked on a material with no glow
//! does not make the strength slider's resting echo look like an edit, and a pick that
//! moves the selection never writes the old node's values into the new one. A control the
//! Model handed NO value this frame (a glow control while there is no glow) can only be
//! echoing its resting default, and is never read.
//!
//! # A declined edit lets go of its control
//!
//! An edit the document refuses, or one that changes nothing, leaves the Model exactly
//! where it was — the one case the walker's local display ownership cannot clear by
//! itself — so every such path hands the bind back (`MaterialBench::decline`) and the
//! control shows the Model again on the next walk, instead of holding (and re-reporting,
//! every frame) the value the document declined.
//!
//! [`Designer`]: crate::designer::Designer

use flicker::script::{Value, ValueMap};
use flicker_canvas::{CanvasEvents, CanvasMode, Link};
use flicker_texture::{MaterialGraph, NodeKind};

use crate::designer::{
    self, Card, Designer, OutPort, Sink, WIRE_FROM_OUTPUT, WIRE_MISTYPED, WIRE_NO_FREE_PORT,
    WIRE_NO_PORT,
};
use crate::{
    Binding, Framing, MaterialBench, ASK_BIND_HERE, ASK_KEEP, ASK_REPLACE, LIT_ZOOM, VIEW_COUNT,
};

/// A control's result, when it differs from what the Model handed it this frame — the
/// one definition of "an edit, not an echo". A control the Model handed nothing is never
/// read: all it can report is its resting default.
fn changed(published: &ValueMap, results: &ValueMap, key: &str) -> Option<f64> {
    let v = results.get(key)?;
    if published.get(key)? == v {
        return None;
    }
    match v {
        Value::Number(n) => Some(*n),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// A checkbox / toggle's written-back state — distinct from [`ValueMap::is_on`], which
/// cannot tell "absent" from "present and false".
fn toggled(results: &ValueMap, key: &str) -> Option<bool> {
    match results.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// What this frame's SOURCE selects listed, captured before any edit lands: the selected
/// node's candidates per input port and every output port's, in [`OutPort::ALL`] order.
/// Option `1 + j` of a source select is entry `j` of ITS list here — the same lists the
/// Model labelled the options from.
struct Start {
    inputs: Vec<Vec<String>>,
    outputs: Vec<Vec<String>>,
}

impl Start {
    fn of(d: &Designer) -> Self {
        let owned = |keys: Vec<&str>| keys.into_iter().map(str::to_string).collect();
        let inputs = match d.selected() {
            Some((_, n)) => (0..n.kind.ports().len())
                .map(|port| {
                    owned(d.candidates(&Sink::Input {
                        node: n.key.clone(),
                        port,
                    }))
                })
                .collect(),
            None => Vec::new(),
        };
        let outputs = OutPort::ALL
            .iter()
            .map(|p| owned(d.candidates(&Sink::Output(*p))))
            .collect();
        Self { inputs, outputs }
    }
}

/// Apply this frame's control results against the Model they were drawn from. Returns
/// whether the IMAGE changed (and so whether a re-bake is owed).
pub fn apply(bench: &mut MaterialBench, published: &ValueMap, results: &ValueMap) -> bool {
    let start = Start::of(&bench.designer);

    // ── the selected node and the output stage, as the Model described them ──
    let mut edited = dials(bench, published, results);
    edited |= wires(bench, &start, published, results);
    edited |= output_stage(bench, &start, published, results);

    // ── the node list: picks, insertion, deletion, order ──
    let picked = (0..bench.designer.graph().nodes.len())
        .find(|i| results.is_on(&format!("nl_pick_{i}")))
        .map(|i| bench.designer.graph().nodes[i].key.clone());
    if let Some(key) = picked {
        // Picked from the LIST: its card may lie anywhere on the canvas — bring it into
        // view (a card picked on the canvas is in view already).
        bench.designer.select(Some(&key));
        bench.canvas.reveal(&key);
    }
    if results.is_on("node_insert") {
        // The insertion menu is the shared popup chrome; `update` opens it (it owns the
        // Transition) and its answer comes back here as an `insert_<kind>` name.
        bench.ask_insert = true;
    }
    for kind in NodeKind::catalog() {
        if results.is_on(&format!("insert_{}", kind.id())) {
            // The new card goes BESIDE the node it was inserted after, so the arrangement
            // the author is looking at stays put around it.
            let after = bench.designer.selected_key().map(str::to_string);
            if bench.designer.insert(kind) {
                edited = true;
                if let (Some(key), Some(after)) =
                    (bench.designer.selected_key().map(str::to_string), after)
                {
                    // …and in view: the author is about to work on it.
                    if bench.canvas.place_beside(&key, &after) {
                        bench.canvas.reveal(&key);
                    }
                }
            }
        }
    }
    if results.is_on("node_delete") {
        edited |= delete_selection(bench);
    }
    for (name, delta) in [("node_up", -1), ("node_down", 1)] {
        if results.is_on(name) {
            if let Some(key) = bench.designer.selected_key().map(str::to_string) {
                edited |= bench.designer.move_node(&key, delta);
            }
        }
    }

    // ── view-only ──
    // The tabs bind `sel_map`, a NUMBER. THE RAIL OWNS ITS OWN STEPPING: the strip authors
    // `next_action`/`prev_action` ("map_next"/"map_prev"), so a bumper, a gutter click and
    // a picked tab all step the rail's bind INSIDE the walker, and the only thing that
    // arrives here is the resulting index. Hand-stepping it here too was the skipped-tab
    // bug (MCP 801B1B09). The LIT view rides the same bound number.
    if let Some(v) = results.number("sel_map") {
        bench.sel_map = (v.max(0.0) as usize).min(VIEW_COUNT - 1);
    }
    if results.is_on("lit_body") {
        bench.sample.body = bench.sample.body.toggled();
    }
    if let Some(v) = toggled(results, "lit_spin") {
        bench.sample.spinning = v;
    }
    // The Lit view's ZOOM is the view's, never the document's — how close the sample is
    // drawn, so no bake — and a value the sample declines (it clamps to where the zoom
    // already is) hands the slider back to the Model.
    if let Some(v) = changed(published, results, LIT_ZOOM) {
        if !bench.sample.set_zoom(v as f32, bench.zoom.range()) {
            bench.decline(LIT_ZOOM);
        }
    }
    if let Some(v) = results.number("canvas_mode") {
        bench.canvas_mode = if v >= 0.5 {
            CanvasMode::Link
        } else {
            CanvasMode::Select
        };
    }
    if results.is_on("canvas_reset") {
        // Reset VIEW: the camera frames the whole graph again — legibly, so past the
        // floor on the selection; the author's arrangement of the cards is theirs and
        // stays.
        bench.fit_pending = Some(Framing::Selection);
    }
    // A list's scroll offset rides its bind: the walker writes it (wheel, clamped) and
    // reads it back off the Model, so the bench adopts what it echoes.
    if let Some(v) = results.number("nl_scroll") {
        bench.node_scroll = v;
    }
    if let Some(v) = results.number("mat_scroll") {
        bench.mat_scroll = v;
    }

    // ── the title bar: the slot binding, the pages, the rename, the rung, Commit ──
    // The binding and the rung are document and output settings, but neither changes the
    // IMAGE — a re-bake would show the same pixels — so neither asks for one. A slot whose
    // look is already COMMITTED asks first (`bind_slot`) and changes nothing yet: the
    // dropdown keeps showing the pick until the prompt is answered.
    if let Some(n) = changed(published, results, "sel_material") {
        match bench.set_material_by_option(n) {
            Binding::Unchanged => bench.decline("sel_material"),
            Binding::Rebound | Binding::Asked => {}
        }
    }
    // A prompt's answer, back through `modal_closed`: REPLACE makes the waiting document the
    // one in hand (a committed look opens, a preset or a Roll loads — a whole new image);
    // BIND HERE keeps the work and binds it to the picked slot; KEEP changes nothing and
    // hands the control that asked back to the Model.
    if results.is_on(ASK_REPLACE) {
        edited |= bench.replace_pending();
    }
    if results.is_on(ASK_BIND_HERE) {
        bench.bind_here();
    }
    if results.is_on(ASK_KEEP) {
        bench.keep_editing();
    }
    if results.is_on("page_bench") {
        bench.page = 0;
        bench.cancel_rename();
    }
    if results.is_on("page_materials") {
        bench.page = 1;
        bench.cancel_rename();
    }
    let row = (0..bench.materials.len()).find(|i| results.is_on(&format!("mat_pick_{i}")));
    if let Some(id) = row.and_then(|i| bench.materials.get(i)).map(|def| def.id) {
        // A re-bind, or a committed slot's prompt — neither changes the image yet.
        bench.bind_slot(Some(id));
    }
    if results.is_on("rename") {
        bench.begin_rename();
    }
    if results.is_on("rename_commit") {
        bench.commit_rename();
    }
    if results.is_on("rename_cancel") {
        bench.cancel_rename();
    }
    if results.is_on("bake_size") {
        bench.step_size();
    }
    if results.is_on("commit") {
        bench.start_commit();
    }

    // ── whole documents: presets, Roll, Reseed ──
    // Each is a LOAD, and a load over edits not yet loaded or committed asks first
    // (`load_or_ask` — the shared unsaved-changes prompt; its Discard loads, its Keep hands
    // the control back). The select shows the preset only while the document IS it (see
    // `MaterialBench::preset_option`), so re-picking the one an edit has moved away from
    // reloads it.
    if let Some(v) = changed(published, results, "sel_preset") {
        let pick = v.round();
        if pick >= 0.0 && (pick as usize) < bench.presets.len() {
            edited |= load_preset(bench, pick as usize, Some("sel_preset"));
        } else {
            bench.decline("sel_preset");
        }
    }
    let presets = bench.presets.len();
    if results.is_on("patch_next") {
        let next = bench.preset.map_or(0, |p| (p + 1) % presets);
        edited |= load_preset(bench, next, None);
    }
    if results.is_on("patch_prev") {
        let prev = bench.preset.map_or(0, |p| (p + presets - 1) % presets);
        edited |= load_preset(bench, prev, None);
    }
    // ROLL — a whole new material in one press, seeded off the current seed so the
    // sequence is reproducible rather than clock-driven (a roll you liked stays saveable).
    // The rack's random patch arrives as a graph (the rack IS a preset graph), its glow
    // snapped onto the curated palette (B46FFC37) — `random` itself stays unrestricted;
    // the restriction is the designer's.
    if results.is_on("roll") {
        let seed = bench
            .designer
            .graph()
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let mut rolled = MaterialGraph::from_recipe(&flicker_texture::random(seed));
        designer::snap_glow(&mut rolled, &bench.palette);
        edited |= bench.load_or_ask(rolled, None, None);
    }
    if results.is_on("reseed") {
        edited |= bench.designer.reseed();
    }

    edited
}

/// Load preset `i` — a whole new document, and a new workspace for it (the canvas's
/// camera and hand placements belonged to the last one) — or, over unsaved edits, ask
/// first; `control` is the select the pick came from. Whether the image changed now.
fn load_preset(bench: &mut MaterialBench, i: usize, control: Option<&'static str>) -> bool {
    match bench.presets.get(i).cloned() {
        Some(graph) => bench.load_or_ask(graph, Some(i), control),
        None => false,
    }
}

/// The selected wire if there is one, else the selected node.
fn delete_selection(bench: &mut MaterialBench) -> bool {
    if let Some(sink) = bench.designer.selected_edge().cloned() {
        return bench.designer.unwire(&sink);
    }
    match bench.designer.selected_key().map(str::to_string) {
        Some(key) => bench.designer.delete(&key),
        None => false,
    }
}

/// The selected node's dials: every published `node_<param>` whose result differs.
fn dials(bench: &mut MaterialBench, published: &ValueMap, results: &ValueMap) -> bool {
    let Some((_, node)) = bench.designer.selected() else {
        return false;
    };
    let (key, kind) = (node.key.clone(), node.kind.clone());
    let mut edited = false;
    for (name, _) in designer::knobs(&kind, &bench.palette) {
        let Some(v) = changed(published, results, &name) else {
            continue;
        };
        let param = name.trim_start_matches("node_");
        let ok = match param {
            "hue" => bench.designer.set_ramp_tint(&key, Some(v), None),
            "sat" => bench.designer.set_ramp_tint(&key, None, Some(v)),
            // A palette node takes a CURATED colour: option `i` is palette entry `i`; -1
            // (a colour off the palette, the select's placeholder) is a display state,
            // never a pick.
            "rgb" => {
                let pick = v.round();
                match (pick >= 0.0)
                    .then(|| bench.palette.get(pick as usize))
                    .flatten()
                    .map(|c| c.rgb)
                {
                    Some(rgb) => bench.designer.set_palette_rgb(&key, rgb),
                    None => false,
                }
            }
            _ => bench.designer.set_param(&key, param, v),
        };
        if !ok {
            bench.decline(&name);
        }
        edited |= ok;
    }
    edited
}

/// The selected node's input selects, `node_in<k>` per port.
fn wires(
    bench: &mut MaterialBench,
    start: &Start,
    published: &ValueMap,
    results: &ValueMap,
) -> bool {
    let Some((_, node)) = bench.designer.selected() else {
        return false;
    };
    let key = node.key.clone();
    let mut edited = false;
    for (port, candidates) in start.inputs.iter().enumerate() {
        let sink = Sink::Input {
            node: key.clone(),
            port,
        };
        let bind = format!("node_in{port}");
        edited |= pick_source(bench, published, results, &bind, &sink, candidates);
    }
    edited
}

/// A SOURCE select: option 0 unwires `sink`, option `1 + j` wires the `j`-th of the
/// candidates it listed. A refused wire leaves its notice for the status line; every
/// declined pick hands the select back to the Model.
fn pick_source(
    bench: &mut MaterialBench,
    published: &ValueMap,
    results: &ValueMap,
    bind: &str,
    sink: &Sink,
    candidates: &[String],
) -> bool {
    let Some(v) = changed(published, results, bind) else {
        return false;
    };
    let pick = v.round();
    let ok = if pick < 0.0 {
        false
    } else if pick == 0.0 {
        bench.designer.unwire(sink)
    } else {
        match candidates.get(pick as usize - 1) {
            Some(from) => wire(bench, from, sink),
            None => false,
        }
    };
    if !ok {
        bench.decline(bind);
    }
    ok
}

/// One wire through the one mutation, its refusal kept as the notice.
fn wire(bench: &mut MaterialBench, from: &str, sink: &Sink) -> bool {
    match bench.designer.wire(from, sink) {
        Ok(changed) => changed,
        Err(notice) => {
            bench.designer.refuse(notice);
            false
        }
    }
}

/// The output stage: its two colour/field ports, the relief, the three scalar ports
/// (a source select over a constant), and the glow's colour, mask and strength — every
/// one read against what the Model published, so the glow a colour pick turns on this
/// frame is not mistaken for the mask and strength controls having moved.
fn output_stage(
    bench: &mut MaterialBench,
    start: &Start,
    published: &ValueMap,
    results: &ValueMap,
) -> bool {
    let listed = |port: OutPort| start.outputs[port as usize].as_slice();
    let mut edited = false;
    for (bind, port) in [("out_base", OutPort::Base), ("out_height", OutPort::Height)] {
        edited |= pick_source(
            bench,
            published,
            results,
            bind,
            &Sink::Output(port),
            listed(port),
        );
    }
    if let Some(v) = changed(published, results, "out_relief") {
        let ok = bench.designer.set_relief(v);
        if !ok {
            bench.decline("out_relief");
        }
        edited |= ok;
    }
    for (port, src, level) in [
        (OutPort::Roughness, "out_rough_src", "out_rough"),
        (OutPort::Metalness, "out_metal_src", "out_metal"),
        (OutPort::Ao, "out_ao_src", "out_ao"),
    ] {
        // The constant is published (and its slider shown) only while no field drives
        // the port, so it is only read then.
        if let Some(v) = changed(published, results, level) {
            let ok = bench.designer.set_level(port, v);
            if !ok {
                bench.decline(level);
            }
            edited |= ok;
        }
        edited |= pick_source(
            bench,
            published,
            results,
            src,
            &Sink::Output(port),
            listed(port),
        );
    }
    // The glow's colour is the curated palette: 0 = no glow, `1 + i` = entry `i`
    // (written into the palette node the glow takes, or a fresh one), -1 = a colour off
    // the palette (the placeholder — a display state, never a pick).
    if let Some(v) = changed(published, results, "out_emit_color") {
        let pick = v.round();
        let ok = if pick == 0.0 {
            bench.designer.set_emit_colour(None)
        } else if pick > 0.0 {
            match bench.palette.get(pick as usize - 1).map(|c| c.rgb) {
                Some(rgb) => bench.designer.set_emit_colour(Some(rgb)),
                None => false,
            }
        } else {
            false
        };
        if !ok {
            bench.decline("out_emit_color");
        }
        edited |= ok;
    }
    edited |= pick_source(
        bench,
        published,
        results,
        "out_emit_mask",
        &Sink::Output(OutPort::EmitMask),
        listed(OutPort::EmitMask),
    );
    if let Some(v) = changed(published, results, "out_emit_strength") {
        let ok = bench.designer.set_emit_strength(v);
        if !ok {
            bench.decline("out_emit_strength");
        }
        edited |= ok;
    }
    edited
}

/// Apply this frame's graph-canvas gestures — the canvas reports WHAT was hit, this
/// decides what it MEANS: a card press selects its node (the output card deselects), an
/// edge press selects that wire, a press on empty canvas lets go of a wire, and a link
/// dragged card → port WIRES (see [`link`]). `release` is the port under the release
/// cursor (`GraphCanvas::port_at`), when there is one. Returns whether the image changed.
pub fn canvas(
    bench: &mut MaterialBench,
    events: &CanvasEvents,
    release: Option<(usize, usize)>,
) -> bool {
    if let Some(p) = events.pressed {
        match (p.card, p.edge) {
            (Some(c), _) => match bench.canvas_content.cards.get(c).copied() {
                Some(Card::Node(i)) => {
                    let key = bench.designer.graph().nodes.get(i).map(|n| n.key.clone());
                    bench.designer.select(key.as_deref());
                }
                Some(Card::Output) => bench.designer.select(None),
                None => {}
            },
            (None, Some(e)) => {
                let sink = bench
                    .canvas_content
                    .sinks
                    .get(e)
                    .and_then(Sink::from_shared);
                bench.designer.select_edge(sink);
            }
            (None, None) => bench.designer.select_edge(None),
        }
    }
    events.linked.is_some_and(|l| link(bench, l, release))
}

/// A completed link drag: the source card's node wired into the port the release landed
/// on — dropping ON a port is how a wire is replaced — or, released on a card's BODY,
/// into its first FREE port that takes the source's kind of signal. A body drop never
/// replaces a wire: when every such port is wired it is refused, as are a link out of the
/// output card, onto a card with no ports, or into a port of the other kind (each with
/// its status notice). A press and release on one card is a click, not a link.
pub fn link(bench: &mut MaterialBench, link: Link, release: Option<(usize, usize)>) -> bool {
    let Some(to) = link.to.or(release.map(|(card, _)| card)) else {
        return false;
    };
    if to == link.from {
        return false;
    }
    let cards = &bench.canvas_content.cards;
    let from = match cards.get(link.from) {
        Some(Card::Node(i)) => bench.designer.graph().nodes.get(*i).map(|n| n.key.clone()),
        Some(Card::Output) => {
            bench.designer.refuse(WIRE_FROM_OUTPUT);
            return false;
        }
        None => None,
    };
    let Some(from) = from else {
        return false;
    };
    let sinks: Vec<Sink> = match cards.get(to) {
        Some(Card::Node(i)) => match bench.designer.graph().nodes.get(*i) {
            Some(n) => (0..n.kind.ports().len())
                .map(|port| Sink::Input {
                    node: n.key.clone(),
                    port,
                })
                .collect(),
            None => return false,
        },
        Some(Card::Output) => OutPort::ALL.iter().map(|p| Sink::Output(*p)).collect(),
        None => return false,
    };
    if sinks.is_empty() {
        bench.designer.refuse(WIRE_NO_PORT);
        return false;
    }
    let makes = bench
        .designer
        .index_of(&from)
        .map(|i| bench.designer.graph().nodes[i].kind.output());
    let port = release.filter(|(card, _)| *card == to).map(|(_, k)| k);
    let sink = match port {
        Some(k) => sinks.get(k).cloned(),
        None => {
            let mut fitting = sinks
                .iter()
                .filter(|s| bench.designer.sink_kind(s) == makes)
                .peekable();
            if fitting.peek().is_none() {
                None
            } else {
                match fitting.find(|s| bench.designer.source_of(s).is_none()) {
                    Some(free) => Some(free.clone()),
                    None => {
                        bench.designer.refuse(WIRE_NO_FREE_PORT);
                        return false;
                    }
                }
            }
        }
    };
    match sink {
        Some(sink) => wire(bench, &from, &sink),
        None => {
            bench.designer.refuse(WIRE_MISTYPED);
            false
        }
    }
}
