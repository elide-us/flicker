//! The DESIGNER's document — the [`MaterialGraph`] being authored, what is selected in
//! it, and the ONE set of mutations the dispatcher ([`crate::route`]) turns signals into.
//!
//! Nothing else in the bench writes the graph. Every edit is a method here; each returns
//! whether the document actually CHANGED (re-writing the value already held is not an
//! edit, so a control echoing its value never re-bakes), and each change re-runs
//! [`problems`], so the designer's problems list can never go stale. An edit that cannot
//! be made leaves the document untouched and records a NOTICE — a stringtable token the
//! status line shows — so a refused wire says why instead of silently doing nothing.
//!
//! The publishing helpers sit beside the mutations they mirror: [`knobs`] is the selected
//! node's parameters exactly as the Model carries them, and the dispatcher compares each
//! control's result against that same function — one definition of "what the control was
//! handed", so an echo can never be mistaken for an edit.

use std::collections::HashSet;

use flicker::script::Value;
use flicker_materialview::{graph_view, GlowPalette};
use flicker_texture::graph::{Axis, Pattern, Port, ValueKind};
use flicker_texture::{
    problems, BlendMode, Emit, GraphError, MaterialGraph, Node, NodeKind, NoiseKind, Output, Scalar,
};
use serde_json::Value as Json;

/// The notice a wire leaves when its port takes the other kind of signal (a colour into
/// a field port, or the reverse).
pub const WIRE_MISTYPED: &str = "$mtl_wire_mistyped";
/// The notice a wire leaves when it would close a loop — a node fed, however far
/// upstream, by itself.
pub const WIRE_LOOPS: &str = "$mtl_wire_loops";
/// The notice a link dragged OUT of the output card leaves: the output feeds nothing.
pub const WIRE_FROM_OUTPUT: &str = "$mtl_wire_from_output";
/// The notice a glow mask leaves when there is no glow to mask yet.
pub const WIRE_NO_GLOW: &str = "$mtl_wire_no_glow";
/// The notice a link dropped on a card with no input ports leaves (a generator).
pub const WIRE_NO_PORT: &str = "$mtl_wire_no_port";
/// The notice a link dropped on a card's BODY leaves when every port that takes its kind
/// of signal is already wired — a body drop never replaces a wire (drop on the port).
pub const WIRE_NO_FREE_PORT: &str = "$mtl_wire_no_free_port";

/// Parameters that are not dials. A noise node's `salt` separates it from its
/// neighbours under the graph seed: it is set when the node is made (and the whole graph
/// re-rolls through Reseed's seed), never dialled.
pub const NOT_KNOBS: [&str; 1] = ["salt"];

// The canvas's cards — what each stands for, and the Output pseudo-card's key (one the
// designer never generates as a node key, so a hand placement never collides with one) —
// are the engine's ONE projection of a graph onto cards (`flicker_materialview::graph_view`,
// which the Material Inspector draws through too), re-exported under the names the bench
// has always used.
pub use flicker_materialview::graph_view::{Card, OUTPUT_CARD};

/// Every [`Axis`], in menu order — the gradient node's axis select lists exactly these.
pub const AXES: [Axis; 3] = [Axis::U, Axis::V, Axis::Radial];
/// Every [`Pattern`], in menu order — the shape node's pattern select lists exactly these.
pub const PATTERNS: [Pattern; 4] = [Pattern::Tiles, Pattern::Bricks, Pattern::Hex, Pattern::Dots];

/// An axis's serialized id. Exhaustive on purpose: a new [`Axis`] variant fails to
/// compile here until [`AXES`] (and the scene's select) learn it.
pub fn axis_id(a: Axis) -> &'static str {
    match a {
        Axis::U => "u",
        Axis::V => "v",
        Axis::Radial => "radial",
    }
}

/// A pattern's serialized id — exhaustive for the same reason as [`axis_id`].
pub fn pattern_id(p: Pattern) -> &'static str {
    match p {
        Pattern::Tiles => "tiles",
        Pattern::Bricks => "bricks",
        Pattern::Hex => "hex",
        Pattern::Dots => "dots",
    }
}

/// The option ids an ENUM parameter's select lists, in option order — `None` for a
/// parameter that is not an enum. The select's bound number is an index into this list.
pub fn enum_ids(param: &str) -> Option<Vec<&'static str>> {
    match param {
        "source" => Some(NoiseKind::ALL.iter().map(|k| k.id()).collect()),
        "mode" => Some(BlendMode::ALL.iter().map(|m| m.id()).collect()),
        "axis" => Some(AXES.iter().map(|a| axis_id(*a)).collect()),
        "pattern" => Some(PATTERNS.iter().map(|p| pattern_id(*p)).collect()),
        _ => None,
    }
}

/// One of the output node's seven ports — the output card's, in port order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutPort {
    Base,
    Height,
    Roughness,
    Metalness,
    Ao,
    EmitColor,
    EmitMask,
}

impl OutPort {
    /// Every port, in card order.
    pub const ALL: [OutPort; 7] = [
        OutPort::Base,
        OutPort::Height,
        OutPort::Roughness,
        OutPort::Metalness,
        OutPort::Ao,
        OutPort::EmitColor,
        OutPort::EmitMask,
    ];

    /// The port's row in flicker-texture's ONE output table ([`Output::ports`]) — the
    /// validator resolves the output through the same rows, so the designer cannot offer
    /// a port a type the evaluator would refuse. [`OutPort::ALL`] is that table's order.
    pub fn table(self) -> Port {
        Output::ports()[self as usize]
    }

    /// What the port takes.
    pub fn takes(self) -> ValueKind {
        self.table().kind
    }

    /// Which of the three scalar ports this is (`roughness`, `metalness`, `ao`).
    fn level_slot(self) -> Option<usize> {
        match self {
            OutPort::Roughness => Some(0),
            OutPort::Metalness => Some(1),
            OutPort::Ao => Some(2),
            _ => None,
        }
    }
}

/// The constant a scalar output port rests at when nothing has driven it yet — the crate's
/// own fresh output.
fn fresh_level(slot: usize) -> f32 {
    let fresh = Output::default();
    match [fresh.roughness, fresh.metalness, fresh.ao].get(slot) {
        Some(Scalar::Const(c)) => *c,
        _ => 0.0,
    }
}

/// Where a wire lands: one input port of a node, or one of the output's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sink {
    Input { node: String, port: usize },
    Output(OutPort),
}

impl Sink {
    /// The wire as the shared graph view names it ([`graph_view::Sink`]) — an output port
    /// by its row in the ONE output table, which [`OutPort::ALL`]'s order IS.
    pub fn shared(&self) -> graph_view::Sink {
        match self {
            Sink::Input { node, port } => graph_view::Sink::Input {
                node: node.clone(),
                port: *port,
            },
            Sink::Output(p) => graph_view::Sink::Output(*p as usize),
        }
    }

    /// A wire the shared view drew, in the designer's terms — `None` for an output row the
    /// table does not have.
    pub fn from_shared(sink: &graph_view::Sink) -> Option<Self> {
        Some(match sink {
            graph_view::Sink::Input { node, port } => Sink::Input {
                node: node.clone(),
                port: *port,
            },
            graph_view::Sink::Output(row) => Sink::Output(*OutPort::ALL.get(*row)?),
        })
    }
}

/// A wire the shared view drew IS a designer sink when the two name the same port — an
/// output port by its row in the one table — so a drawn wire is asked about in the
/// designer's own terms.
impl PartialEq<Sink> for graph_view::Sink {
    fn eq(&self, other: &Sink) -> bool {
        *self == other.shared()
    }
}

/// The document being designed and the designer's selection in it.
pub struct Designer {
    graph: MaterialGraph,
    /// The selected node, by key.
    sel: Option<String>,
    /// The selected wire, by where it lands (a sink has at most one source).
    sel_edge: Option<Sink>,
    /// Everything wrong with the whole graph — re-run after every edit.
    problems: Vec<GraphError>,
    /// Why the last edit was refused, as a stringtable token, until the next edit lands.
    notice: Option<&'static str>,
    /// The specifics the notice names, as DATA beside its token (a path, a loader's
    /// reason) — empty for a notice that needs none. Cleared with the notice.
    notice_detail: String,
    /// Bumped by every edit, selection and refusal — what a view derived from the
    /// document (the canvas content) compares to know it is stale.
    rev: u64,
    /// The document is exactly as it was loaded — so re-picking the preset it came from
    /// is no edit, and after the first edit it is.
    pristine: bool,
    /// GRAPH mutations this session — bumped by every edit of the mutation set and by
    /// nothing else (a selection, a refusal, a slot binding change no map).
    mutations: u64,
    /// [`Self::mutations`] as of the last load or SUCCESSFUL commit — the work up to there
    /// is safe (in the document it came from, or staged); past it is unsaved.
    saved: u64,
    /// Every key this session has held, loaded or made — a fresh key never reuses one,
    /// so nothing (a hand placement, a remembered wire) can attach to the wrong node.
    seen: HashSet<String>,
    /// The constant each scalar port (roughness, metalness, ao) held before a field was
    /// wired into it — what it returns to when that field is unwired.
    resting: [f32; 3],
    /// The strength a NEW glow starts at — authored (`params.new_glow_strength`), so a
    /// glow colour picked on a material with no glow is visible at once.
    glow_strength: f32,
}

impl Designer {
    /// A designer over `graph`, with its first node selected. A glow the designer starts
    /// takes `glow_strength`.
    pub fn new(graph: MaterialGraph, glow_strength: f32) -> Self {
        let mut d = Self {
            graph,
            sel: None,
            sel_edge: None,
            problems: Vec::new(),
            notice: None,
            notice_detail: String::new(),
            rev: 0,
            pristine: true,
            mutations: 0,
            saved: 0,
            seen: HashSet::new(),
            resting: [0.0; 3],
            glow_strength,
        };
        d.loaded();
        d
    }

    // ── reading ──────────────────────────────────────────────────────────────────

    pub fn graph(&self) -> &MaterialGraph {
        &self.graph
    }

    pub fn problems(&self) -> &[GraphError] {
        &self.problems
    }

    pub fn notice(&self) -> Option<&'static str> {
        self.notice
    }

    /// What the notice names, as data — the status line's detail beside its token.
    pub fn notice_detail(&self) -> &str {
        &self.notice_detail
    }

    /// The document's revision — it moves on every edit, selection and refusal.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Whether the document is exactly as it was last loaded — what the preset select
    /// echoes (a slot binding counts here: the document is no longer the preset).
    pub fn pristine(&self) -> bool {
        self.pristine
    }

    /// Whether the GRAPH has been edited since it was last loaded or committed — what the
    /// unsaved-changes guard asks before a committed look replaces the document. Only a
    /// mutation of the graph counts: binding or re-binding a slot does not (the maps bake
    /// the same, and the next commit lands it), nor does selecting or a refused edit.
    pub fn unsaved(&self) -> bool {
        self.mutations != self.saved
    }

    /// The graph's mutation count — what a commit captures as it starts, so its success can
    /// mark exactly the work it staged as saved ([`Self::saved_through`]).
    pub fn mutations(&self) -> u64 {
        self.mutations
    }

    /// A commit of the document as it stood at `mutations` STAGED: the work up to there is
    /// saved. An edit made while the commit baked stays unsaved; a load since then is not
    /// undone.
    pub fn saved_through(&mut self, mutations: u64) {
        self.saved = self.saved.max(mutations);
    }

    pub fn selected_key(&self) -> Option<&str> {
        self.sel.as_deref()
    }

    pub fn selected_edge(&self) -> Option<&Sink> {
        self.sel_edge.as_ref()
    }

    /// The node at `key`, by position in the node list.
    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.graph.nodes.iter().position(|n| n.key == key)
    }

    /// The selected node, with its position.
    pub fn selected(&self) -> Option<(usize, &Node)> {
        let i = self.index_of(self.sel.as_deref()?)?;
        Some((i, &self.graph.nodes[i]))
    }

    /// The nodes that make a `kind` value, in node-list order.
    pub fn family(&self, kind: ValueKind) -> impl Iterator<Item = &Node> {
        self.graph
            .nodes
            .iter()
            .filter(move |n| n.kind.output() == kind)
    }

    /// The nodes `sink` may be wired from, in node-list order — what its SOURCE select
    /// offers past its "none". Only what the graph would ACCEPT: the port's kind of signal,
    /// and for a node's own port never the node itself nor anything it already feeds
    /// (either would close a loop). A refusal is then something no pick can reach.
    pub fn candidates(&self, sink: &Sink) -> Vec<&str> {
        let Some(kind) = self.sink_kind(sink) else {
            return Vec::new();
        };
        let target = match sink {
            Sink::Input { node, .. } => Some(node.as_str()),
            Sink::Output(_) => None,
        };
        self.family(kind)
            .filter(|n| target.is_none_or(|t| n.key != t && !self.reads(&n.key, t)))
            .map(|n| n.key.as_str())
            .collect()
    }

    /// The type a sink takes, if the sink exists.
    pub fn sink_kind(&self, sink: &Sink) -> Option<ValueKind> {
        match sink {
            Sink::Output(p) => Some(p.takes()),
            Sink::Input { node, port } => {
                let i = self.index_of(node)?;
                self.graph.nodes[i].kind.ports().get(*port).map(|p| p.kind)
            }
        }
    }

    /// The key wired into `sink`, if any (whether or not it resolves).
    pub fn source_of(&self, sink: &Sink) -> Option<&str> {
        match sink {
            Sink::Input { node, port } => {
                let i = self.index_of(node)?;
                self.graph.nodes[i].inputs.get(*port)?.as_deref()
            }
            // The output's wiring as written, by its row in the ONE port table — the
            // reading the shared graph view draws its wires from (`Output::sources`).
            Sink::Output(p) => self.graph.output.sources()[*p as usize],
        }
    }

    /// The option a SOURCE select shows for `sink`: `1 + j` when the `j`-th of its
    /// [`candidates`](Self::candidates) drives it, `0` (the authored "none" / "constant")
    /// when nothing does — or when what is wired there is no candidate (a deleted or
    /// mistyped key, which the problems list names). The Model publishes this and the
    /// dispatcher compares against it, so the two never disagree about what an echo is.
    pub fn source_option(&self, sink: &Sink) -> f64 {
        let Some(src) = self.source_of(sink) else {
            return 0.0;
        };
        self.candidates(sink)
            .iter()
            .position(|k| *k == src)
            .map_or(0.0, |j| (j + 1) as f64)
    }

    /// The option the glow-colour select shows: `0` when there is no glow, `1 + i` when
    /// the glow takes a palette node holding palette entry `i` exactly, and `-1` (the
    /// select's placeholder) when it takes any other colour.
    pub fn emit_option(&self, palette: &GlowPalette) -> f64 {
        let Some(emit) = self.graph.output.emit.as_ref() else {
            return 0.0;
        };
        match self
            .index_of(&emit.color)
            .map(|i| &self.graph.nodes[i].kind)
        {
            Some(NodeKind::Palette { rgb }) => {
                palette_index(palette, *rgb).map_or(-1.0, |i| (i + 1) as f64)
            }
            _ => -1.0,
        }
    }

    /// How many sinks read `key` — node inputs and output ports.
    fn readers(&self, key: &str) -> usize {
        let inputs = self
            .graph
            .nodes
            .iter()
            .flat_map(|n| n.inputs.iter().flatten())
            .filter(|k| *k == key)
            .count();
        let outputs = OutPort::ALL
            .iter()
            .filter(|p| self.source_of(&Sink::Output(**p)) == Some(key))
            .count();
        inputs + outputs
    }

    /// One of the three scalar output ports.
    pub fn level(&self, port: OutPort) -> Option<&Scalar> {
        let out = &self.graph.output;
        match port {
            OutPort::Roughness => Some(&out.roughness),
            OutPort::Metalness => Some(&out.metalness),
            OutPort::Ao => Some(&out.ao),
            _ => None,
        }
    }

    // ── selection ────────────────────────────────────────────────────────────────

    /// Select a node by key (`None` deselects). Selecting a node lets go of a wire.
    pub fn select(&mut self, key: Option<&str>) {
        self.sel = key
            .filter(|k| self.index_of(k).is_some())
            .map(str::to_string);
        self.sel_edge = None;
        self.rev += 1;
    }

    /// Select a wire by where it lands (`None` lets go of it).
    pub fn select_edge(&mut self, sink: Option<Sink>) {
        self.sel_edge = sink.filter(|s| self.source_of(s).is_some());
        self.rev += 1;
    }

    /// Record why an edit was refused — the status line says so until the next edit.
    pub fn refuse(&mut self, notice: &'static str) {
        self.refuse_with(notice, String::new());
    }

    /// Record a refusal with its specifics as DATA beside the token (S4's token + detail
    /// convention): the status line shows both until the next edit.
    pub fn refuse_with(&mut self, notice: &'static str, detail: String) {
        self.notice = Some(notice);
        self.notice_detail = detail;
        self.rev += 1;
    }

    /// Bind (or unbind) the material slot this graph is the appearance of. Not an IMAGE
    /// edit — the maps bake the same — so the caller owes no re-bake; it is still the
    /// document's, and lands in the committed bundle.
    pub fn bind(&mut self, material: Option<flicker_materials::MaterialId>) -> bool {
        if self.graph.material == material {
            return false;
        }
        self.graph.material = material;
        self.pristine = false;
        true
    }

    // ── the mutations ────────────────────────────────────────────────────────────

    /// Replace the whole document (a preset, a roll) and select its first node.
    pub fn load(&mut self, graph: MaterialGraph) {
        self.graph = graph;
        self.loaded();
    }

    /// Insert a fresh node of `kind` right after the selected one (at the end when none
    /// is), under a new key, and select it. A noise node takes a salt no other noise node
    /// in the graph has, so it is a new voice rather than an echo of one.
    pub fn insert(&mut self, kind: NodeKind) -> bool {
        let key = self.fresh_key(kind.id());
        let kind = match kind {
            NodeKind::Noise {
                source,
                scale,
                octaves,
                lacunarity,
                gain,
                warp,
                contrast,
                invert,
                ..
            } => NodeKind::Noise {
                source,
                scale,
                octaves,
                lacunarity,
                gain,
                warp,
                salt: self.fresh_salt(),
                contrast,
                invert,
            },
            other => other,
        };
        let at = self
            .selected()
            .map_or(self.graph.nodes.len(), |(i, _)| i + 1);
        self.graph.nodes.insert(
            at,
            Node {
                key: key.clone(),
                kind,
                inputs: Vec::new(),
            },
        );
        self.sel = Some(key);
        self.sel_edge = None;
        self.edited();
        true
    }

    /// Delete the node `key` and every wire it fed — its consumers' ports fall unwired,
    /// an output port it drove falls back to resting (a scalar to the crate's fresh
    /// constant, the glow to none). The selection moves to its neighbour.
    pub fn delete(&mut self, key: &str) -> bool {
        let Some(i) = self.index_of(key) else {
            return false;
        };
        self.graph.nodes.remove(i);
        for node in &mut self.graph.nodes {
            for input in &mut node.inputs {
                if input.as_deref() == Some(key) {
                    *input = None;
                }
            }
            trim(&mut node.inputs);
        }
        for port in OutPort::ALL {
            let sink = Sink::Output(port);
            if self.source_of(&sink) == Some(key) {
                self.clear(&sink);
            }
        }
        self.sel = self
            .graph
            .nodes
            .get(i.min(self.graph.nodes.len().saturating_sub(1)))
            .map(|n| n.key.clone());
        self.sel_edge = None;
        self.edited();
        true
    }

    /// Move the node `key` `delta` places along the node list (its AUTHORING order — the
    /// wires, not this list, decide evaluation). False at either end.
    pub fn move_node(&mut self, key: &str, delta: isize) -> bool {
        let Some(i) = self.index_of(key) else {
            return false;
        };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.graph.nodes.len() || delta == 0 {
            return false;
        }
        self.graph.nodes.swap(i, j as usize);
        self.edited();
        true
    }

    /// Wire the node `from` into `sink`. Refused — with the notice to show — when the
    /// types differ, when the wire would close a loop, or when a glow mask has no glow to
    /// mask. `Ok(false)` when the wire is already there (or names nothing).
    pub fn wire(&mut self, from: &str, sink: &Sink) -> Result<bool, &'static str> {
        let (Some(fi), Some(wants)) = (self.index_of(from), self.sink_kind(sink)) else {
            return Ok(false);
        };
        if self.graph.nodes[fi].kind.output() != wants {
            return Err(WIRE_MISTYPED);
        }
        if self.source_of(sink) == Some(from) {
            return Ok(false);
        }
        match sink {
            Sink::Input { node, port } => {
                if node == from || self.reads(from, node) {
                    return Err(WIRE_LOOPS);
                }
                let Some(ti) = self.index_of(node) else {
                    return Ok(false);
                };
                let inputs = &mut self.graph.nodes[ti].inputs;
                if inputs.len() <= *port {
                    inputs.resize(*port + 1, None);
                }
                inputs[*port] = Some(from.to_string());
            }
            Sink::Output(port) => {
                if let (Some(slot), Some(Scalar::Const(c))) = (port.level_slot(), self.level(*port))
                {
                    // Remember the constant the field replaces — unwiring returns to it.
                    self.resting[slot] = *c;
                }
                let new_glow = self.glow_strength;
                let out = &mut self.graph.output;
                let key = from.to_string();
                match port {
                    OutPort::Base => out.base = Some(key),
                    OutPort::Height => out.height = Some(key),
                    OutPort::Roughness => out.roughness = Scalar::Field(key),
                    OutPort::Metalness => out.metalness = Scalar::Field(key),
                    OutPort::Ao => out.ao = Scalar::Field(key),
                    OutPort::EmitColor => {
                        let (mask, strength) = out
                            .emit
                            .take()
                            .map_or((None, new_glow), |e| (e.mask, e.strength));
                        out.emit = Some(Emit {
                            color: key,
                            mask,
                            strength,
                        });
                    }
                    OutPort::EmitMask => match out.emit.as_mut() {
                        Some(e) => e.mask = Some(key),
                        None => return Err(WIRE_NO_GLOW),
                    },
                }
            }
        }
        self.edited();
        Ok(true)
    }

    /// Unwire `sink`. False when nothing was wired there.
    pub fn unwire(&mut self, sink: &Sink) -> bool {
        if self.source_of(sink).is_none() {
            return false;
        }
        self.clear(sink);
        if self.sel_edge.as_ref() == Some(sink) {
            self.sel_edge = None;
        }
        self.edited();
        true
    }

    /// Set one numeric, boolean or enum parameter of the node `key`. Integers round, an
    /// enum takes its option INDEX, a non-finite value is refused, and a value the node
    /// already holds is no edit. Writes through the node's own serialized form, so every
    /// kind's every parameter is reachable by name with no per-kind table to drift.
    pub fn set_param(&mut self, key: &str, param: &str, value: f64) -> bool {
        if !value.is_finite() || NOT_KNOBS.contains(&param) {
            return false;
        }
        let Some(i) = self.index_of(key) else {
            return false;
        };
        let Some(kind) = with_param(&self.graph.nodes[i].kind, param, value) else {
            return false;
        };
        if kind == self.graph.nodes[i].kind {
            return false;
        }
        self.graph.nodes[i].kind = kind;
        self.edited();
        true
    }

    /// Re-tint a ramp node: its representative hue and/or saturation, swept over every
    /// stop with each stop's brightness kept ([`flicker_texture::ColorRamp::recolor`]).
    /// The tint it already wears is no edit.
    pub fn set_ramp_tint(&mut self, key: &str, hue: Option<f64>, sat: Option<f64>) -> bool {
        let Some(i) = self.index_of(key) else {
            return false;
        };
        let NodeKind::Ramp { ramp } = &mut self.graph.nodes[i].kind else {
            return false;
        };
        if hue.is_some_and(|h| !h.is_finite()) || sat.is_some_and(|s| !s.is_finite()) {
            return false;
        }
        let (cur_hue, cur_sat) = ramp.tint();
        let hue = hue.map_or(cur_hue, |h| (h as f32).rem_euclid(1.0));
        let sat = sat.map_or(cur_sat, |s| (s as f32).clamp(0.0, 1.0));
        if (hue - cur_hue).abs() <= 1e-4 && (sat - cur_sat).abs() <= 1e-4 {
            return false;
        }
        // "Changed" means the STOPS changed: a hue on a grey ramp recolours nothing, and
        // reporting it as an edit would re-bake on every frame the control holds it.
        let mut tinted = ramp.clone();
        tinted.recolor(hue, sat);
        if tinted == *ramp {
            return false;
        }
        *ramp = tinted;
        self.edited();
        true
    }

    /// Set a palette node's colour.
    pub fn set_palette_rgb(&mut self, key: &str, colour: [f32; 3]) -> bool {
        let Some(i) = self.index_of(key) else {
            return false;
        };
        let NodeKind::Palette { rgb } = &mut self.graph.nodes[i].kind else {
            return false;
        };
        if *rgb == colour {
            return false;
        }
        *rgb = colour;
        self.edited();
        true
    }

    /// The output's relief.
    pub fn set_relief(&mut self, v: f64) -> bool {
        let v = v as f32;
        if !v.is_finite() || self.graph.output.relief == v {
            return false;
        }
        self.graph.output.relief = v;
        self.edited();
        true
    }

    /// A scalar output port's CONSTANT — only while no field drives it.
    pub fn set_level(&mut self, port: OutPort, v: f64) -> bool {
        let v = v as f32;
        if !v.is_finite() {
            return false;
        }
        let out = &mut self.graph.output;
        let slot = match port {
            OutPort::Roughness => &mut out.roughness,
            OutPort::Metalness => &mut out.metalness,
            OutPort::Ao => &mut out.ao,
            _ => return false,
        };
        match slot {
            Scalar::Const(c) if *c != v => *c = v,
            _ => return false,
        }
        self.edited();
        true
    }

    /// The glow's strength — only while there is a glow.
    pub fn set_emit_strength(&mut self, v: f64) -> bool {
        let v = v as f32;
        match self.graph.output.emit.as_mut() {
            Some(e) if v.is_finite() && e.strength != v => e.strength = v,
            _ => return false,
        }
        self.edited();
        true
    }

    /// The glow's colour, from the curated palette (the DESIGNER's restriction — the
    /// graph stores free RGB): `None` turns the glow off; a colour is written into the
    /// palette node the glow already takes, or a new palette node the glow is wired to.
    pub fn set_emit_colour(&mut self, colour: Option<[f32; 3]>) -> bool {
        let Some(colour) = colour else {
            if self.graph.output.emit.is_none() {
                return false;
            }
            self.graph.output.emit = None;
            self.edited();
            return true;
        };
        // The palette node the glow takes is rewritten in place only when the glow is its
        // ONE reader — recolouring a palette node something else also reads (a base colour)
        // would recolour that too, so the glow gets a fresh node of its own instead.
        let current = self
            .graph
            .output
            .emit
            .as_ref()
            .and_then(|e| self.index_of(&e.color))
            .filter(|&i| matches!(self.graph.nodes[i].kind, NodeKind::Palette { .. }));
        if let Some(i) = current {
            let key = self.graph.nodes[i].key.clone();
            if self.readers(&key) == 1 {
                return self.set_palette_rgb(&key, colour);
            }
            if matches!(self.graph.nodes[i].kind, NodeKind::Palette { rgb } if rgb == colour) {
                return false;
            }
        }
        let key = self.fresh_key("glow");
        self.graph.nodes.push(Node {
            key: key.clone(),
            kind: NodeKind::Palette { rgb: colour },
            inputs: Vec::new(),
        });
        // The new node is wired straight in, through the same mutation a dragged link
        // makes (a palette node always takes a colour port, so it cannot be refused).
        if !matches!(self.wire(&key, &Sink::Output(OutPort::EmitColor)), Ok(true)) {
            self.edited();
        }
        true
    }

    /// Re-roll every noise node at once: step the graph seed (each node's own salt still
    /// separates it from its neighbours) — the same seed step the rack's Reseed took, so a
    /// reseeded patch is the same instrument giving a different performance.
    pub fn reseed(&mut self) -> bool {
        self.graph.seed = self
            .graph
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.edited();
        true
    }

    // ── internals ────────────────────────────────────────────────────────────────

    /// After any edit: the problems list re-runs, a stale selection lets go, and the
    /// last refusal is answered.
    fn edited(&mut self) {
        self.problems = problems(&self.graph);
        self.notice = None;
        self.notice_detail.clear();
        self.rev += 1;
        self.pristine = false;
        self.mutations += 1;
        if self
            .sel
            .as_deref()
            .is_some_and(|k| self.index_of(k).is_none())
        {
            self.sel = None;
        }
        if self
            .sel_edge
            .as_ref()
            .is_some_and(|s| self.source_of(s).is_none())
        {
            self.sel_edge = None;
        }
    }

    fn loaded(&mut self) {
        self.sel = self.graph.nodes.first().map(|n| n.key.clone());
        self.sel_edge = None;
        self.seen
            .extend(self.graph.nodes.iter().map(|n| n.key.clone()));
        for port in [OutPort::Roughness, OutPort::Metalness, OutPort::Ao] {
            if let Some(slot) = port.level_slot() {
                self.resting[slot] = match self.level(port) {
                    Some(Scalar::Const(c)) => *c,
                    _ => fresh_level(slot),
                };
            }
        }
        self.edited();
        self.pristine = true;
        // A freshly loaded document holds nothing unsaved.
        self.saved = self.mutations;
    }

    /// Clear whatever drives `sink` (no checks — callers decide it is wired).
    fn clear(&mut self, sink: &Sink) {
        match sink {
            Sink::Input { node, port } => {
                if let Some(i) = self.index_of(node) {
                    let inputs = &mut self.graph.nodes[i].inputs;
                    if let Some(slot) = inputs.get_mut(*port) {
                        *slot = None;
                    }
                    trim(inputs);
                }
            }
            Sink::Output(port) => {
                let out = &mut self.graph.output;
                match port {
                    OutPort::Base => out.base = None,
                    OutPort::Height => out.height = None,
                    OutPort::Roughness | OutPort::Metalness | OutPort::Ao => {
                        // Back to the constant it held before the field was wired in.
                        if let Some(slot) = port.level_slot() {
                            let rest = Scalar::Const(self.resting[slot]);
                            match port {
                                OutPort::Roughness => out.roughness = rest,
                                OutPort::Metalness => out.metalness = rest,
                                _ => out.ao = rest,
                            }
                        }
                    }
                    OutPort::EmitColor => out.emit = None,
                    OutPort::EmitMask => {
                        if let Some(e) = out.emit.as_mut() {
                            e.mask = None;
                        }
                    }
                }
            }
        }
    }

    /// Whether `node` reads `dep`, however far upstream.
    fn reads(&self, node: &str, dep: &str) -> bool {
        let mut seen = vec![false; self.graph.nodes.len()];
        let mut stack: Vec<usize> = self.index_of(node).into_iter().collect();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            for k in self.graph.nodes[i].inputs.iter().flatten() {
                if k == dep {
                    return true;
                }
                if let Some(j) = self.index_of(k) {
                    stack.push(j);
                }
            }
        }
        false
    }

    /// `stem1`, `stem2`, … — the first key this SESSION has never held (a deleted node's
    /// key is not handed to the next one, so nothing remembered about it can attach to a
    /// stranger). Recorded as seen.
    fn fresh_key(&mut self, stem: &str) -> String {
        let key = (1..)
            .map(|n| format!("{stem}{n}"))
            .find(|k| !self.seen.contains(k) && self.index_of(k).is_none())
            .expect("an unbounded counter always finds a free key");
        self.seen.insert(key.clone());
        key
    }

    /// One past the largest salt any noise node holds.
    fn fresh_salt(&self) -> u64 {
        self.graph
            .nodes
            .iter()
            .filter_map(|n| match n.kind {
                NodeKind::Noise { salt, .. } => Some(salt),
                _ => None,
            })
            .max()
            .map_or(1, |s| s.wrapping_add(1))
    }
}

/// Drop trailing unwired ports, so a node's input list stays as short as its wiring.
fn trim(inputs: &mut Vec<Option<String>>) {
    while inputs.last().is_some_and(Option::is_none) {
        inputs.pop();
    }
}

/// `kind` with one parameter rewritten through its serialized form — `None` when the
/// kind has no such parameter, or the value cannot be one (a non-finite number, an enum
/// index past its list).
fn with_param(kind: &NodeKind, param: &str, value: f64) -> Option<NodeKind> {
    let mut doc = serde_json::to_value(kind).ok()?;
    let slot = doc.as_object_mut()?.get_mut(param)?;
    *slot = match slot {
        Json::Bool(_) => Json::Bool(value != 0.0),
        Json::Number(n) if n.is_u64() => Json::from(value.round().max(0.0) as u64),
        Json::Number(_) => Json::from(value),
        Json::String(_) => {
            let ids = enum_ids(param)?;
            let i = value.round();
            if i < 0.0 {
                return None;
            }
            Json::String(ids.get(i as usize)?.to_string())
        }
        _ => return None,
    };
    let kind: NodeKind = serde_json::from_value(doc).ok()?;
    // A finite f64 can still overflow an f32 parameter into infinity (1e39 in a band's
    // `at`): what does not come back as a finite number is refused, not stored.
    let back = serde_json::to_value(&kind).ok()?;
    match back.get(param) {
        Some(Json::Number(n)) if n.as_f64().is_some_and(f64::is_finite) => Some(kind),
        Some(Json::Bool(_) | Json::String(_)) => Some(kind),
        _ => None,
    }
}

/// The palette entry a colour IS, exactly — `None` for a colour off the curated palette.
pub fn palette_index(palette: &GlowPalette, rgb: [f32; 3]) -> Option<usize> {
    palette.iter().position(|c| c.rgb == rgb)
}

/// A refused or failed graph, as the status line speaks it: a stringtable TOKEN per kind
/// of error (the copy is localized) and the specifics — the node, port, parameter or loop
/// it names — as raw data beside it. Never the evaluator's English.
pub fn error_parts(e: &GraphError) -> (&'static str, String) {
    match e {
        GraphError::DuplicateKey { key } => ("$mtl_err_duplicate_key", key.clone()),
        GraphError::UnknownNode { key, at } => ("$mtl_err_unknown_node", format!("{at} ← {key}")),
        GraphError::UnknownPort { node, index } => {
            ("$mtl_err_unknown_port", format!("{node} · {index}"))
        }
        GraphError::MissingInput { at } => ("$mtl_err_missing_input", at.clone()),
        GraphError::PortType { at, source, .. } => {
            ("$mtl_err_port_type", format!("{at} ← {source}"))
        }
        GraphError::Cycle { path } => ("$mtl_err_cycle", path.join(" ← ")),
        GraphError::NonFinite { node, param } => ("$mtl_err_non_finite", format!("{node}.{param}")),
        GraphError::OutOfRange { node, param } => {
            ("$mtl_err_out_of_range", format!("{node}.{param}"))
        }
        GraphError::NonFiniteTexel { node } => ("$mtl_err_non_finite_texel", node.clone()),
    }
}

/// A node's dials exactly as the Model carries them — `node_<param>` for every numeric,
/// boolean and enum parameter (an enum as its option index), a ramp as its tint
/// (`node_hue`, `node_sat`), a palette node as its palette entry (`node_rgb`, -1 off it).
/// [`NOT_KNOBS`] are left out. The dispatcher compares results against this same list,
/// so what was published is, by construction, what an echo looks like.
pub fn knobs(kind: &NodeKind, palette: &GlowPalette) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    match kind {
        NodeKind::Ramp { ramp } => {
            let (hue, sat) = ramp.tint();
            out.push(("node_hue".to_string(), Value::Number(hue as f64)));
            out.push(("node_sat".to_string(), Value::Number(sat as f64)));
            return out;
        }
        NodeKind::Palette { rgb } => {
            // The curated entry it IS (its option), or -1 — the select's placeholder.
            let option = palette_index(palette, *rgb).map_or(-1.0, |i| i as f64);
            out.push(("node_rgb".to_string(), Value::Number(option)));
            return out;
        }
        _ => {}
    }
    let Ok(Json::Object(fields)) = serde_json::to_value(kind) else {
        return out;
    };
    for (name, v) in fields {
        if name == "kind" || NOT_KNOBS.contains(&name.as_str()) {
            continue;
        }
        let value = match v {
            Json::Bool(b) => Value::Bool(b),
            Json::Number(n) => match n.as_f64() {
                Some(f) => Value::Number(f),
                None => continue,
            },
            Json::String(s) => {
                match enum_ids(&name).and_then(|ids| ids.iter().position(|id| *id == s)) {
                    Some(i) => Value::Number(i as f64),
                    None => continue,
                }
            }
            _ => continue,
        };
        out.push((format!("node_{name}"), value));
    }
    out
}

/// Snap the glow's colour onto the nearest palette entry (Roll's rule: a rolled glow stays
/// inside the curated set — B46FFC37). A no-op when the glow is not a palette node.
pub fn snap_glow(graph: &mut MaterialGraph, palette: &GlowPalette) {
    let Some(key) = graph.output.emit.as_ref().map(|e| e.color.clone()) else {
        return;
    };
    let Some(node) = graph.nodes.iter_mut().find(|n| n.key == key) else {
        return;
    };
    if let NodeKind::Palette { rgb } = &mut node.kind {
        if let Some(c) = palette.nearest(*rgb).and_then(|i| palette.get(i)) {
            *rgb = c.rgb;
        }
    }
}
