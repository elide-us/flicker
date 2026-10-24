//! Resolving and validating a graph — exactly the part a request reaches.
//!
//! A bake or a preview validates the subgraph it will evaluate and nothing else, so a
//! card being drawn, a loop between two loose cards or a deleted key on an unread
//! port never blanks the designer's view; [`problems`] lists the whole graph's.

use std::collections::{HashMap, HashSet};

use super::{GraphError, MaterialGraph, Scalar, ValueKind, MAX_RELIEF, OUTPUT_PORTS};

// ───────────────────────────── resolving the wires ─────────────────────────────

/// Key → node. Keys are what every wire names, so a duplicate is refused outright,
/// wherever it sits.
pub(super) fn key_index(graph: &MaterialGraph) -> Result<HashMap<&str, usize>, GraphError> {
    let mut index = HashMap::with_capacity(graph.nodes.len());
    for (i, node) in graph.nodes.iter().enumerate() {
        if index.insert(node.key.as_str(), i).is_some() {
            return Err(GraphError::DuplicateKey {
                key: node.key.clone(),
            });
        }
    }
    Ok(index)
}

/// Resolve a wire: the key must name a node, and that node must make `wants`.
pub(super) fn resolve(
    graph: &MaterialGraph,
    index: &HashMap<&str, usize>,
    key: &str,
    wants: ValueKind,
    at: impl FnOnce() -> String,
) -> Result<usize, GraphError> {
    let Some(&i) = index.get(key) else {
        return Err(GraphError::UnknownNode {
            key: key.into(),
            at: at(),
        });
    };
    let found = graph.nodes[i].kind.output();
    if found != wants {
        return Err(GraphError::PortType {
            at: at(),
            wants,
            source: key.into(),
            found,
        });
    }
    Ok(i)
}

/// Node `i`, inspected: where each of its ports is wired from (`None` when unwired or
/// broken), and everything wrong with it — its wiring, then its parameters.
fn inspect(
    graph: &MaterialGraph,
    index: &HashMap<&str, usize>,
    i: usize,
) -> (Vec<Option<usize>>, Vec<GraphError>) {
    let node = &graph.nodes[i];
    let ports = node.kind.ports();
    let mut problems = Vec::new();
    if node.inputs.len() > ports.len() {
        problems.push(GraphError::UnknownPort {
            node: node.key.clone(),
            index: ports.len(),
        });
    }
    let mut wires = Vec::with_capacity(ports.len());
    for (p, port) in ports.iter().enumerate() {
        let at = || format!("{}.{}", node.key, port.name);
        wires.push(match node.inputs.get(p).and_then(Option::as_deref) {
            Some(key) => match resolve(graph, index, key, port.kind, at) {
                Ok(src) => Some(src),
                Err(e) => {
                    problems.push(e);
                    None
                }
            },
            None => {
                if port.required {
                    problems.push(GraphError::MissingInput { at: at() });
                }
                None
            }
        });
    }
    problems.extend(node.kind.param_problems(&node.key));
    (wires, problems)
}

/// A scalar output port, resolved: a constant, or the index of the field driving it.
#[derive(Clone, Copy)]
pub(super) enum Source {
    Const(f32),
    Node(usize),
}

/// The output node's ports, resolved to node indices.
pub(super) struct Ports {
    pub(super) base: Option<usize>,
    pub(super) height: Option<usize>,
    pub(super) roughness: Source,
    pub(super) metalness: Source,
    pub(super) ao: Source,
    /// (colour, mask, strength)
    pub(super) emit: Option<(usize, Option<usize>, f32)>,
}

/// The output node, inspected: its ports resolved (a broken port resolves to
/// nothing), and everything wrong with it.
pub(super) fn inspect_output(
    graph: &MaterialGraph,
    index: &HashMap<&str, usize>,
) -> (Ports, Vec<GraphError>) {
    let out = &graph.output;
    let mut problems = Vec::new();
    // What each port takes, and the name its errors carry, come from the ONE table.
    let [base_p, height_p, rough_p, metal_p, ao_p, color_p, mask_p] = OUTPUT_PORTS;
    let wire = |key: &str, wants: ValueKind, port: &str, problems: &mut Vec<GraphError>| {
        resolve(graph, index, key, wants, || format!("output.{port}"))
            .map_err(|e| problems.push(e))
            .ok()
    };
    let finite = |v: f32, param: &'static str, problems: &mut Vec<GraphError>| {
        if !v.is_finite() {
            problems.push(GraphError::NonFinite {
                node: "output".into(),
                param,
            });
        }
    };
    let base = out
        .base
        .as_deref()
        .and_then(|k| wire(k, base_p.kind, base_p.name, &mut problems));
    let height = out
        .height
        .as_deref()
        .and_then(|k| wire(k, height_p.kind, height_p.name, &mut problems));
    finite(out.relief, "relief", &mut problems);
    if out.relief.abs() > MAX_RELIEF {
        problems.push(GraphError::OutOfRange {
            node: "output".into(),
            param: "relief",
        });
    }
    let level = |s: &Scalar, port: super::Port, problems: &mut Vec<GraphError>| match s {
        Scalar::Const(v) => {
            finite(*v, port.name, problems);
            Source::Const(*v)
        }
        Scalar::Field(key) => {
            wire(key, port.kind, port.name, problems).map_or(Source::Const(0.0), Source::Node)
        }
    };
    let roughness = level(&out.roughness, rough_p, &mut problems);
    let metalness = level(&out.metalness, metal_p, &mut problems);
    let ao = level(&out.ao, ao_p, &mut problems);
    let emit = match &out.emit {
        None => None,
        Some(e) => {
            let color = wire(&e.color, color_p.kind, color_p.name, &mut problems);
            let mask = e
                .mask
                .as_deref()
                .map(|k| wire(k, mask_p.kind, mask_p.name, &mut problems));
            if !e.strength.is_finite() {
                problems.push(GraphError::NonFinite {
                    node: "output".into(),
                    param: "emit.strength",
                });
            }
            // A broken wire was reported above; it resolves to nothing here.
            color.map(|c| (c, mask.flatten(), e.strength))
        }
    };
    let ports = Ports {
        base,
        height,
        roughness,
        metalness,
        ao,
        emit,
    };
    (ports, problems)
}

/// Per node, one entry per port: the node feeding it (`None` unwired or unresolved).
pub(super) type Wires = Vec<Vec<Option<usize>>>;

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Unseen,
    Open,
    Done,
}

/// Depth-first over the wires from `roots`, dependencies first — iterative, so a long
/// chain cannot blow the stack. `open` resolves a node's wires the first time the walk
/// reaches it (and may refuse it); `looped` hears each loop as the path around it —
/// each node fed by the next, the last the first again — and decides whether the walk
/// goes on. Returns the order and every reached node's wires.
///
/// THE one walk: the evaluator plans with it ([`Plan::reaching`]), [`problems`] finds
/// loops with it, and [`MaterialGraph::eval_order`] orders a designer's cards with it —
/// so the order a reader sees is the order the bake runs.
fn walk(
    count: usize,
    roots: impl IntoIterator<Item = usize>,
    mut open: impl FnMut(usize) -> Result<Vec<Option<usize>>, GraphError>,
    mut looped: impl FnMut(Vec<usize>) -> Result<(), GraphError>,
) -> Result<(Vec<usize>, Wires), GraphError> {
    let mut mark = vec![Mark::Unseen; count];
    let mut wires: Wires = vec![Vec::new(); count];
    let mut order = Vec::new();
    for root in roots {
        if mark[root] != Mark::Unseen {
            continue;
        }
        wires[root] = open(root)?;
        mark[root] = Mark::Open;
        let mut stack = vec![(root, 0usize)];
        while let Some(top) = stack.last_mut() {
            let (node, port) = *top;
            let Some(&wire) = wires[node].get(port) else {
                mark[node] = Mark::Done;
                order.push(node);
                stack.pop();
                continue;
            };
            top.1 += 1;
            let Some(dep) = wire else { continue };
            match mark[dep] {
                Mark::Unseen => {
                    wires[dep] = open(dep)?;
                    mark[dep] = Mark::Open;
                    stack.push((dep, 0));
                }
                Mark::Open => {
                    let from = stack.iter().position(|(k, _)| *k == dep).unwrap_or(0);
                    let mut path: Vec<usize> = stack[from..].iter().map(|(k, _)| *k).collect();
                    path.push(dep);
                    looped(path)?;
                }
                Mark::Done => {}
            }
        }
    }
    Ok((order, wires))
}

impl MaterialGraph {
    /// The nodes in EVALUATION order — dependencies first, ties (and loose nodes) in
    /// node-list order — by the same depth-first `walk` the evaluator plans with, rooted
    /// at EVERY node so a loose one has its place too. Lenient where the evaluator is
    /// strict, so a graph in any state still orders (a card being drawn, a deleted key, a
    /// loop): a wire to a missing key is skipped, a duplicate key resolves to its FIRST
    /// node, and a loop is cut where the walk meets it. Every node appears exactly once.
    /// What a designer or a viewer lays its cards out by.
    pub fn eval_order(&self) -> Vec<usize> {
        let mut index = HashMap::with_capacity(self.nodes.len());
        for (i, node) in self.nodes.iter().enumerate() {
            index.entry(node.key.as_str()).or_insert(i);
        }
        // Neither callback refuses, so the walk cannot fail.
        walk(
            self.nodes.len(),
            0..self.nodes.len(),
            |i| {
                Ok(self.nodes[i]
                    .inputs
                    .iter()
                    .map(|k| k.as_deref().and_then(|k| index.get(k).copied()))
                    .collect())
            },
            |_| Ok(()),
        )
        .map(|(order, _)| order)
        .unwrap_or_default()
    }
}

fn keys(graph: &MaterialGraph, path: &[usize]) -> Vec<String> {
    path.iter().map(|&i| graph.nodes[i].key.clone()).collect()
}

/// Everything wrong with the graph — the WHOLE graph, loose nodes included: every
/// duplicate key, every broken wire, missing input, unknown port, non-finite or
/// out-of-range parameter and loop, and the output's own. Empty when the graph is
/// sound.
///
/// A bake and a preview only refuse what they REACH, so a node being drawn, a loop
/// between two loose cards or a deleted key on an unread port never blanks the
/// designer's view; this is where the designer lists them instead.
pub fn problems(graph: &MaterialGraph) -> Vec<GraphError> {
    let mut found = Vec::new();
    let mut index = HashMap::with_capacity(graph.nodes.len());
    let mut doubled = HashSet::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        if index.contains_key(node.key.as_str()) {
            if doubled.insert(node.key.as_str()) {
                found.push(GraphError::DuplicateKey {
                    key: node.key.clone(),
                });
            }
        } else {
            index.insert(node.key.as_str(), i);
        }
    }
    let mut wired = Vec::with_capacity(graph.nodes.len());
    for i in 0..graph.nodes.len() {
        let (wires, problems) = inspect(graph, &index, i);
        found.extend(problems);
        wired.push(wires);
    }
    found.extend(inspect_output(graph, &index).1);
    // Every node is a root, so a loop among loose nodes is found too; each back edge
    // is one loop. Neither callback refuses, so the walk cannot fail.
    let mut loops = Vec::new();
    let _ = walk(
        graph.nodes.len(),
        0..graph.nodes.len(),
        |i| Ok(wired[i].clone()),
        |path| {
            loops.push(GraphError::Cycle {
                path: keys(graph, &path),
            });
            Ok(())
        },
    );
    found.extend(loops);
    found
}

/// The subgraph a request reaches, proven evaluable — every key resolved, every wire
/// typed, every required port wired, every parameter finite and in range, no loop —
/// and the order to evaluate it in.
pub(super) struct Plan<'g> {
    pub(super) graph: &'g MaterialGraph,
    /// Per REACHED node, one entry per port: the node feeding it.
    pub(super) wires: Wires,
    /// The reached nodes, dependencies first.
    pub(super) order: Vec<usize>,
}

impl<'g> Plan<'g> {
    /// Resolve, validate and order exactly what `roots` reach. A loose node elsewhere
    /// — unwired, looped, out of range — is the designer's to fix ([`problems`] lists
    /// it), never a reason this request fails.
    pub(super) fn reaching(
        graph: &'g MaterialGraph,
        index: &HashMap<&str, usize>,
        roots: &[usize],
    ) -> Result<Self, GraphError> {
        let (order, wires) = walk(
            graph.nodes.len(),
            roots.iter().copied(),
            |i| {
                let (wires, problems) = inspect(graph, index, i);
                problems.into_iter().next().map_or(Ok(wires), Err)
            },
            |path| {
                Err(GraphError::Cycle {
                    path: keys(graph, &path),
                })
            },
        )?;
        Ok(Self {
            graph,
            wires,
            order,
        })
    }
}
