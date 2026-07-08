//! State-graph extraction + **grouped scene layout** for the field viewer.
//!
//! Turns the authored `flicker.pack` + the model's clip library into laid-out [`Node`]s
//! (one character per node) organised into labelled [`GroupBox`]es on the floor:
//! * **Movement** — the connected locomotion states (idle/walk/run/strafe/crouch), placed
//!   by a force-directed relaxation so transition-linked clusters weight together;
//! * one box per **triggered sequence** — **Jump** (Start→Loop→End), **Attack**,
//!   **Reactions** (Dame/Death) — laid out in a small grid (they're reached by an event,
//!   not part of the movement weave);
//! * the **waiting table** — every clip *no state uses* — split into **In-Place** and
//!   **RootMotion** groups.
//!
//! Boxes are arranged left→right, each vertically centred, and the whole scene is centred
//! on the origin. [`edges`](Graph::edges) still carries every state transition (for the
//! click-to-reveal selection). Deterministic (seeded, fixed iterations, no RNG).

use std::collections::{HashMap, HashSet};

use glam::Vec2;

use flicker_skeletal::format::Model;
use flicker_skeletal::state::StateMachineDef;

/// One character in the field.
pub struct Node {
    /// Index into [`Model::clips`], or `usize::MAX` if the state's clip is missing.
    pub clip_index: usize,
    /// Layout position on the floor plane (engine metres, XZ; centred on the origin).
    pub pos: Vec2,
    /// `true` = a state in the graph; `false` = an unwired waiting clip.
    #[allow(dead_code)]
    pub connected: bool,
    /// State name (connected) or clip name (waiting) — HUD + selection.
    pub label: String,
}

/// What a [`GroupBox`] holds — drives its outline colour.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    /// The outer box for a whole `.pack` file — encloses its action sub-boxes.
    Pack,
    /// The connected locomotion graph (a sub-box inside the pack).
    Movement,
    /// A triggered sequence — jump / attack / reactions (a sub-box inside the pack).
    Triggered,
    /// Waiting clips, in-place variants.
    WaitingInPlace,
    /// Waiting clips, root-motion variants.
    WaitingRoot,
}

/// A labelled region on the floor grouping a set of nodes.
pub struct GroupBox {
    pub label: String,
    pub min: Vec2,
    pub max: Vec2,
    pub kind: BoxKind,
}

/// The laid-out scene.
pub struct Graph {
    pub nodes: Vec<Node>,
    /// Unique undirected edges as index pairs into [`Self::nodes`] (connected nodes only).
    pub edges: Vec<(usize, usize)>,
    /// Directed exit targets per connected node (its transition + `next` destinations) —
    /// for the "arrows to the states you can reach from here" highlight.
    pub out_edges: Vec<Vec<usize>>,
    /// Targets of any-state edges (hit → Dame, die → Death) — reachable from *every* state,
    /// so they're added to the active node's exit arrows.
    pub any_targets: Vec<usize>,
    pub connected_count: usize,
    pub orphan_count: usize,
    /// Floor rectangle containing everything (XZ, centred on the origin).
    pub floor_min: Vec2,
    pub floor_max: Vec2,
    /// Labelled group boxes (graph groups + waiting groups), centred with everything else.
    pub boxes: Vec<GroupBox>,
}

/// Cluster a state belongs to — ported from `tools/gen_state_diagram.py`'s `group_of`.
fn group_of(name: &str) -> &'static str {
    if name == "Idle" {
        "core"
    } else if name.starts_with("Walk") {
        "walk"
    } else if name.starts_with("Run") {
        "run"
    } else if name.starts_with("Crouch") {
        "crouch"
    } else if name.starts_with("Jump") {
        "jump"
    } else if name.starts_with("Attack") || name.starts_with("Short_Attack") {
        "combat"
    } else if name.starts_with("Dame") || name.starts_with("Death") {
        "react"
    } else {
        "misc"
    }
}

/// Locomotion groups that merge into the one force-directed **Movement** box.
fn is_movement(group: &str) -> bool {
    matches!(group, "core" | "walk" | "run" | "crouch" | "misc")
}

impl Graph {
    /// Build the laid-out scene. `unit` is the character bounding radius (metres);
    /// `pack_name` labels the enclosing pack box (the `.pack` file this graph came from).
    pub fn build(model: &Model, pack: Option<&StateMachineDef>, pack_name: &str, unit: f32) -> Graph {
        let clip_index: HashMap<&str, usize> = model
            .clips
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.as_str(), i))
            .collect();

        // ── connected nodes = states (positions set during layout) ──
        let mut nodes: Vec<Node> = Vec::new();
        let mut used: HashSet<&str> = HashSet::new();
        let mut state_row: HashMap<&str, usize> = HashMap::new();
        let mut group_by_node: Vec<&'static str> = Vec::new();

        if let Some(def) = pack {
            for s in &def.states {
                used.insert(s.clip.as_str());
                let idx = nodes.len();
                state_row.insert(s.name.as_str(), idx);
                group_by_node.push(group_of(&s.name));
                nodes.push(Node {
                    clip_index: clip_index.get(s.clip.as_str()).copied().unwrap_or(usize::MAX),
                    pos: Vec2::ZERO,
                    connected: true,
                    label: s.name.clone(),
                });
            }
        }
        let connected_count = nodes.len();

        // ── edges (all transitions, for click-to-reveal selection) ──
        let mut edge_set: HashSet<(usize, usize)> = HashSet::new();
        if let Some(def) = pack {
            let add = |from: &str, to: &str, set: &mut HashSet<(usize, usize)>| {
                if let (Some(&a), Some(&b)) = (state_row.get(from), state_row.get(to)) {
                    if a != b {
                        set.insert((a.min(b), a.max(b)));
                    }
                }
            };
            for s in &def.states {
                for t in &s.transitions {
                    add(&s.name, &t.to, &mut edge_set);
                }
                if let Some(n) = &s.next {
                    add(&s.name, n, &mut edge_set);
                }
            }
        }
        let mut edges: Vec<(usize, usize)> = edge_set.into_iter().collect();
        edges.sort_unstable();

        // Directed exit targets per state (transitions + `next`) + the any-state targets.
        let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); connected_count];
        let mut any_targets: Vec<usize> = Vec::new();
        if let Some(def) = pack {
            for s in &def.states {
                let Some(&from) = state_row.get(s.name.as_str()) else {
                    continue;
                };
                for t in &s.transitions {
                    if let Some(&to) = state_row.get(t.to.as_str()) {
                        if to != from {
                            out_edges[from].push(to);
                        }
                    }
                }
                if let Some(n) = &s.next {
                    if let Some(&to) = state_row.get(n.as_str()) {
                        if to != from {
                            out_edges[from].push(to);
                        }
                    }
                }
            }
            for a in &def.any {
                if let Some(&to) = state_row.get(a.to.as_str()) {
                    any_targets.push(to);
                }
            }
        }
        for e in &mut out_edges {
            e.sort_unstable();
            e.dedup();
        }
        any_targets.sort_unstable();
        any_targets.dedup();

        // ── partition connected nodes into their boxes ──
        let mut movement = Vec::new();
        let mut jump = Vec::new();
        let mut combat = Vec::new();
        let mut react = Vec::new();
        for (i, &g) in group_by_node.iter().enumerate() {
            match g {
                g if is_movement(g) => movement.push(i),
                "jump" => jump.push(i),
                "combat" => combat.push(i),
                "react" => react.push(i),
                _ => movement.push(i),
            }
        }

        // ── waiting clips (unused by any state) split In-Place vs RootMotion ──
        let mut inplace = Vec::new();
        let mut root = Vec::new();
        for (i, c) in model.clips.iter().enumerate() {
            if used.contains(c.name.as_str()) {
                continue;
            }
            let idx = nodes.len();
            nodes.push(Node {
                clip_index: i,
                pos: Vec2::ZERO,
                connected: false,
                label: c.name.clone(),
            });
            if c.name.starts_with("RM/") {
                root.push(idx);
            } else {
                inplace.push(idx);
            }
        }
        let orphan_count = inplace.len() + root.len();

        // ── lay the boxes out left→right, each vertically centred ──
        let space = unit * 2.6;
        let pad = unit * 1.3;
        let inner_gap = unit * 1.6; // between sub-boxes *inside* the pack
        let gap = unit * 6.0; // between the pack box and the waiting table
        let mut boxes: Vec<GroupBox> = Vec::new();

        // The pack's action sub-boxes, laid in a compact row (all belong to ONE .pack).
        let mut cursor = 0.0f32;
        let mut graph_min = Vec2::splat(f32::INFINITY);
        let mut graph_max = Vec2::splat(f32::NEG_INFINITY);
        let mut sub_boxes: Vec<GroupBox> = Vec::new();

        if !movement.is_empty() {
            let (mn, mx) = place_movement(&mut nodes, &movement, &edges, cursor, unit, space, pad);
            graph_min = graph_min.min(mn);
            graph_max = graph_max.max(mx);
            sub_boxes.push(GroupBox { label: "Movement".into(), min: mn, max: mx, kind: BoxKind::Movement });
            cursor = mx.x + inner_gap;
        }
        for (label, members) in [("Jump", &jump), ("Attack", &combat), ("Reactions", &react)] {
            if members.is_empty() {
                continue;
            }
            let (mn, mx) = place_grid(&mut nodes, members, cursor, space, pad, 6);
            graph_min = graph_min.min(mn);
            graph_max = graph_max.max(mx);
            sub_boxes.push(GroupBox { label: label.into(), min: mn, max: mx, kind: BoxKind::Triggered });
            cursor = mx.x + inner_gap;
        }

        // The enclosing pack box (drawn first so the sub-boxes read inside it).
        if !sub_boxes.is_empty() {
            let ppad = unit * 1.8;
            let pmin = graph_min - Vec2::splat(ppad);
            let pmax = graph_max + Vec2::splat(ppad);
            boxes.push(GroupBox { label: pack_name.to_string(), min: pmin, max: pmax, kind: BoxKind::Pack });
            boxes.extend(sub_boxes);
            cursor = pmax.x + gap; // waiting table starts to the right of the pack box
        } else {
            cursor += gap;
        }

        // The waiting table (clips no state uses) — separate from the pack.
        for (label, members, kind) in [
            ("In-Place", &inplace, BoxKind::WaitingInPlace),
            ("RootMotion", &root, BoxKind::WaitingRoot),
        ] {
            if members.is_empty() {
                continue;
            }
            let (mn, mx) = place_grid(&mut nodes, members, cursor, space, pad, 10);
            boxes.push(GroupBox { label: label.into(), min: mn, max: mx, kind });
            cursor = mx.x + gap;
        }

        // ── floor = union of boxes + margin, centred on the origin ──
        let margin = unit * 2.5;
        let mut fmin = Vec2::splat(f32::INFINITY);
        let mut fmax = Vec2::splat(f32::NEG_INFINITY);
        for b in &boxes {
            fmin = fmin.min(b.min);
            fmax = fmax.max(b.max);
        }
        if !fmin.is_finite() {
            fmin = Vec2::ZERO;
            fmax = Vec2::ZERO;
        }
        fmin -= Vec2::splat(margin);
        fmax += Vec2::splat(margin);
        let shift = (fmin + fmax) * 0.5;

        for n in &mut nodes {
            n.pos -= shift;
        }
        for b in &mut boxes {
            b.min -= shift;
            b.max -= shift;
        }

        Graph {
            nodes,
            edges,
            out_edges,
            any_targets,
            connected_count,
            orphan_count,
            floor_min: fmin - shift,
            floor_max: fmax - shift,
            boxes,
        }
    }
}

/// Place `members` in a grid whose min-corner starts at `x0` (X), vertically centred on
/// `z = 0`. Returns the padded box rect `(min, max)`.
fn place_grid(
    nodes: &mut [Node],
    members: &[usize],
    x0: f32,
    space: f32,
    pad: f32,
    max_cols: usize,
) -> (Vec2, Vec2) {
    let n = members.len().max(1);
    let cols = ((n as f32).sqrt().ceil() as usize).clamp(1, max_cols.max(1));
    let rows = n.div_ceil(cols);
    let w = cols as f32 * space + 2.0 * pad;
    let h = rows as f32 * space + 2.0 * pad;
    let z0 = -h * 0.5;
    for (k, &idx) in members.iter().enumerate() {
        let (c, r) = (k % cols, k / cols);
        nodes[idx].pos = Vec2::new(
            x0 + pad + c as f32 * space + space * 0.5,
            z0 + pad + r as f32 * space + space * 0.5,
        );
    }
    (Vec2::new(x0, z0), Vec2::new(x0 + w, z0 + h))
}

/// Place the movement states with a force-directed relaxation, then fit them into a square
/// box whose min-corner starts at `x0`, vertically centred on `z = 0`.
fn place_movement(
    nodes: &mut [Node],
    members: &[usize],
    edges: &[(usize, usize)],
    x0: f32,
    unit: f32,
    space: f32,
    pad: f32,
) -> (Vec2, Vec2) {
    let n = members.len();
    let side = (n as f32).sqrt().ceil().max(2.0) * space * 1.35;

    // Local edges (both endpoints in the movement set), remapped to 0..n.
    let local: HashMap<usize, usize> = members.iter().enumerate().map(|(i, &g)| (g, i)).collect();
    let local_edges: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|&(a, b)| match (local.get(&a), local.get(&b)) {
            (Some(&la), Some(&lb)) => Some((la, lb)),
            _ => None,
        })
        .collect();
    // Golden-angle spiral seeds (deterministic).
    let seeds: Vec<Vec2> = (0..n)
        .map(|i| {
            let a = i as f32 * 2.399_963_2;
            Vec2::new(a.cos() * side * 0.3, a.sin() * side * 0.3)
        })
        .collect();
    let movable = vec![true; n];
    let relaxed = force_directed(n, &local_edges, &seeds, &movable, unit * 4.5, 400);

    // Fit into side·0.85, centred in the box.
    let mut mn = Vec2::splat(f32::INFINITY);
    let mut mx = Vec2::splat(f32::NEG_INFINITY);
    for p in &relaxed {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    let span = (mx.x - mn.x).max(mx.y - mn.y).max(1e-3);
    let scale = side * 0.85 / span;
    let center = (mn + mx) * 0.5;
    let box_center = Vec2::new(x0 + pad + side * 0.5, 0.0);
    for (i, &idx) in members.iter().enumerate() {
        nodes[idx].pos = (relaxed[i] - center) * scale + box_center;
    }
    (
        Vec2::new(x0, -side * 0.5 - pad),
        Vec2::new(x0 + side + 2.0 * pad, side * 0.5 + pad),
    )
}

/// Fruchterman–Reingold force-directed layout. `movable[i] == false` pins node `i`.
fn force_directed(
    n: usize,
    edges: &[(usize, usize)],
    seed: &[Vec2],
    movable: &[bool],
    k: f32,
    iters: usize,
) -> Vec<Vec2> {
    let mut pos = seed.to_vec();
    let mut temp = k * 2.0;
    let cool = temp / (iters as f32 + 1.0);
    for _ in 0..iters {
        let mut disp = vec![Vec2::ZERO; n];
        for i in 0..n {
            for j in (i + 1)..n {
                let d = pos[i] - pos[j];
                let dist = d.length().max(0.01);
                let force = (k * k) / dist * (d / dist);
                disp[i] += force;
                disp[j] -= force;
            }
        }
        for &(a, b) in edges {
            let d = pos[a] - pos[b];
            let dist = d.length().max(0.01);
            let force = (dist * dist) / k * (d / dist);
            disp[a] -= force;
            disp[b] += force;
        }
        for i in 0..n {
            if !movable[i] {
                continue;
            }
            let dl = disp[i].length().max(0.01);
            pos[i] += (disp[i] / dl) * dl.min(temp);
        }
        temp = (temp - cool).max(0.0);
    }
    pos
}
