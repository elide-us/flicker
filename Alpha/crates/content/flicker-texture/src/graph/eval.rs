//! Evaluating a graph: the validated subgraph a request reaches ([`super::check`]),
//! one raster pass per node at the bake size — each output map encoded the moment its
//! inputs are final, through the SAME texel encoders [`bake`](crate::bake()) uses.

use std::borrow::Cow;
use std::collections::HashMap;

use super::check::{inspect_output, key_index, resolve, Plan, Source};
use super::nodes;
use super::{GraphError, MaterialGraph, NodeKind, ValueKind};
use crate::bake::{self, MapKind, MapSet, SEAT_SIZE};
use crate::output::{self, NEUTRAL};

/// A node's value over the tile, row-major. `Uniform` is a constant that never
/// needed a raster — a `Constant`, a `Palette`, and anything pointwise over only
/// those — so a flat input costs one number rather than a megabyte.
#[derive(Clone, Debug)]
pub(crate) enum Plane<T> {
    Uniform(T),
    Raster(Vec<T>),
}

impl<T: Copy> Plane<T> {
    /// The value at texel `i`.
    pub(crate) fn at(&self, i: usize) -> T {
        match self {
            Plane::Uniform(v) => *v,
            Plane::Raster(texels) => texels[i],
        }
    }

    fn is_uniform(&self) -> bool {
        matches!(self, Plane::Uniform(_))
    }

    /// Whether every texel passes `ok`.
    fn all(&self, ok: impl Fn(T) -> bool) -> bool {
        match self {
            Plane::Uniform(v) => ok(*v),
            Plane::Raster(texels) => texels.iter().all(|v| ok(*v)),
        }
    }

    /// A pointwise node over this plane: uniform in, uniform out.
    fn map<U>(&self, f: impl Fn(T) -> U) -> Plane<U> {
        match self {
            Plane::Uniform(v) => Plane::Uniform(f(*v)),
            Plane::Raster(texels) => Plane::Raster(texels.iter().map(|v| f(*v)).collect()),
        }
    }

    /// Every texel — what a neighbourhood node reads. Borrowed when the plane is
    /// already a raster; only a uniform plane is expanded.
    pub(crate) fn texels(&self, count: usize) -> Cow<'_, [T]>
    where
        T: Clone,
    {
        match self {
            Plane::Uniform(v) => Cow::Owned(vec![*v; count]),
            Plane::Raster(texels) => Cow::Borrowed(texels),
        }
    }
}

/// A field read at map precision: `f32`, as the output stage reads its height buffer.
fn f32_raster(field: &Plane<f64>, count: usize) -> Vec<f32> {
    match field {
        Plane::Uniform(v) => vec![*v as f32; count],
        Plane::Raster(texels) => texels.iter().map(|v| *v as f32).collect(),
    }
}

/// Texels in an `n × n` tile.
fn cells(n: u32) -> usize {
    (n as usize) * (n as usize)
}

/// What a node evaluated to.
#[derive(Clone, Debug)]
enum Value {
    Field(Plane<f64>),
    Colour(Plane<[f32; 3]>),
}

impl Value {
    /// Whether every texel is a finite number at MAP precision (`f32`) — what it is
    /// about to be quantized at, where a NaN would become a silent black.
    fn is_finite(&self) -> bool {
        match self {
            Value::Field(f) => f.all(|v| (v as f32).is_finite()),
            Value::Colour(c) => c.all(|rgb| rgb.iter().all(|v| v.is_finite())),
        }
    }
}

/// Where a Band opens and — when its width is under 1 — closes, seated in its
/// input's measured range by the bake's own seating.
#[derive(Clone, Copy)]
struct Seat {
    edge: f32,
    close: Option<f32>,
}

impl Seat {
    /// `None` when the input has no range to seat in: the band lights nowhere.
    fn of(measured: &[f32], at: f32, width: f32) -> Option<Self> {
        bake::seat_range(measured).map(|(lo, hi)| {
            let edge = bake::seat(lo, hi, at);
            Seat {
                edge,
                close: (width < 1.0).then(|| edge + (hi - lo) * width.max(0.0)),
            }
        })
    }
}

/// Every node's value during a run — `None` until computed, and again once dropped.
type Slots = Vec<Option<Value>>;

/// What a run hands each value to the moment it is computed; returns the pins it has
/// finished with.
type Visit<'v> = dyn FnMut(usize, &[Option<Value>]) -> Result<Vec<usize>, GraphError> + 'v;

/// One map of the output waiting on the nodes that feed it.
#[derive(Clone, Copy)]
enum Job {
    /// BaseColor ← a colour node.
    Base(usize),
    /// Normal and Height ← the height field.
    Surface(usize),
    /// Roughness / Metallic / AO (`MapKind::ALL` slot 2, 3, 4) ← a field.
    Level(usize, usize),
    /// Emit ← a colour, an optional mask, a strength.
    Emit(usize, Option<usize>, f32),
}

impl Job {
    fn needs(self) -> impl Iterator<Item = usize> {
        match self {
            Job::Base(i) | Job::Surface(i) | Job::Level(_, i) => [Some(i), None],
            Job::Emit(color, mask, _) => [Some(color), mask],
        }
        .into_iter()
        .flatten()
    }
}

impl<'g> Plan<'g> {
    /// The node wired into port `p` of node `i`, if any.
    fn source(&self, i: usize, p: usize) -> Option<usize> {
        self.wires[i].get(p).copied().flatten()
    }

    /// Whether node `i`, or anything feeding it, re-rasters ([`NodeKind::is_raster`]).
    fn rastered(&self, i: usize) -> bool {
        let mut seen = vec![false; self.wires.len()];
        let mut stack = vec![i];
        while let Some(j) = stack.pop() {
            if std::mem::replace(&mut seen[j], true) {
                continue;
            }
            if self.graph.nodes[j].kind.is_raster() {
                return true;
            }
            stack.extend(self.wires[j].iter().flatten());
        }
        false
    }

    /// Evaluate at `n × n` every node the PINNED nodes reach, dependencies first,
    /// handing each value to `visit` the moment it is computed; `visit` returns the
    /// pins it has finished with. A value is dropped as soon as every consumer has read
    /// it and its pins are gone, so peak memory is the graph's live frontier rather
    /// than all of it. A `guard`ed run (a bake) refuses any node whose texels are not
    /// finite at map precision — one scan per evaluated node.
    fn run(
        &self,
        n: u32,
        mut pins: Vec<usize>,
        guard: bool,
        visit: &mut Visit<'_>,
    ) -> Result<Slots, GraphError> {
        let count = self.wires.len();
        let mut needed = vec![false; count];
        let mut reach: Vec<usize> = (0..count).filter(|&i| pins[i] > 0).collect();
        while let Some(i) = reach.pop() {
            if !needed[i] {
                needed[i] = true;
                reach.extend(self.wires[i].iter().flatten());
            }
        }

        // A Band seats against its input's measured range. Over generators and
        // pointwise nodes that input is the same field at every size, so it is
        // measured once at SEAT_SIZE — the bake's own seating, the same band at every
        // size. Downstream of a re-rastering node it is a different field at each
        // size, so it is measured where it is baked: inline, from the input in hand.
        let mut seats = HashMap::new();
        if n != SEAT_SIZE {
            let bands: Vec<(usize, usize, f32, f32)> = (0..count)
                .filter(|&i| needed[i])
                .filter_map(|i| match self.graph.nodes[i].kind {
                    NodeKind::Band { at, width, .. } => self
                        .source(i, 0)
                        .filter(|&src| !self.rastered(src))
                        .map(|src| (i, src, at, width)),
                    _ => None,
                })
                .collect();
            if !bands.is_empty() {
                let mut measure = vec![0usize; count];
                for band in &bands {
                    measure[band.1] += 1;
                }
                let measured = self.run(SEAT_SIZE, measure, guard, &mut |_, _| Ok(Vec::new()))?;
                for (band, src, at, width) in bands {
                    let field = f32_raster(self.field(&measured, src)?, cells(SEAT_SIZE));
                    seats.insert(band, Seat::of(&field, at, width));
                }
            }
        }

        let mut consumers = vec![0usize; count];
        for i in (0..count).filter(|&i| needed[i]) {
            for &j in self.wires[i].iter().flatten() {
                consumers[j] += 1;
            }
        }
        let mut slots: Slots = (0..count).map(|_| None).collect();
        for &i in self.order.iter().filter(|&&i| needed[i]) {
            let value = self.compute(i, n, &slots, seats.get(&i).copied())?;
            if guard && !value.is_finite() {
                return Err(GraphError::NonFiniteTexel {
                    node: self.graph.nodes[i].key.clone(),
                });
            }
            slots[i] = Some(value);
            let released = visit(i, &slots)?;
            for &j in &released {
                pins[j] = pins[j].saturating_sub(1);
            }
            for &j in self.wires[i].iter().flatten() {
                consumers[j] -= 1;
            }
            let settled = self.wires[i].iter().flatten().copied();
            for j in settled.chain(released).chain([i]) {
                if consumers[j] == 0 && pins[j] == 0 {
                    slots[j] = None;
                }
            }
        }
        Ok(slots)
    }

    /// Node `i`'s value. Absent only if the evaluator itself is wrong — reported, not
    /// panicked on.
    fn value<'s>(&self, slots: &'s [Option<Value>], i: usize) -> Result<&'s Value, GraphError> {
        slots[i].as_ref().ok_or_else(|| GraphError::MissingInput {
            at: self.graph.nodes[i].key.clone(),
        })
    }

    fn mistyped(&self, i: usize, wants: ValueKind) -> GraphError {
        let node = &self.graph.nodes[i];
        GraphError::PortType {
            at: "evaluation".into(),
            wants,
            source: node.key.clone(),
            found: node.kind.output(),
        }
    }

    fn field<'s>(
        &self,
        slots: &'s [Option<Value>],
        i: usize,
    ) -> Result<&'s Plane<f64>, GraphError> {
        match self.value(slots, i)? {
            Value::Field(f) => Ok(f),
            Value::Colour(_) => Err(self.mistyped(i, ValueKind::Field)),
        }
    }

    fn colour<'s>(
        &self,
        slots: &'s [Option<Value>],
        i: usize,
    ) -> Result<&'s Plane<[f32; 3]>, GraphError> {
        match self.value(slots, i)? {
            Value::Colour(c) => Ok(c),
            Value::Field(_) => Err(self.mistyped(i, ValueKind::Colour)),
        }
    }

    fn input_field<'s>(
        &self,
        slots: &'s [Option<Value>],
        i: usize,
        p: usize,
    ) -> Result<&'s Plane<f64>, GraphError> {
        match self.source(i, p) {
            Some(src) => self.field(slots, src),
            None => Err(self.unwired(i, p)),
        }
    }

    fn input_colour<'s>(
        &self,
        slots: &'s [Option<Value>],
        i: usize,
        p: usize,
    ) -> Result<&'s Plane<[f32; 3]>, GraphError> {
        match self.source(i, p) {
            Some(src) => self.colour(slots, src),
            None => Err(self.unwired(i, p)),
        }
    }

    /// An optional field port (a mask): `None` when unwired.
    fn input_mask<'s>(
        &self,
        slots: &'s [Option<Value>],
        i: usize,
        p: usize,
    ) -> Result<Option<&'s Plane<f64>>, GraphError> {
        self.source(i, p)
            .map(|src| self.field(slots, src))
            .transpose()
    }

    fn unwired(&self, i: usize, p: usize) -> GraphError {
        let node = &self.graph.nodes[i];
        let port = node.kind.ports().get(p).map_or("?", |port| port.name);
        GraphError::MissingInput {
            at: format!("{}.{port}", node.key),
        }
    }

    /// Evaluate node `i` at `n × n` from its (already evaluated) inputs. `seat` is a
    /// Band's seating measured beforehand at SEAT_SIZE; `None` seats it inline.
    fn compute(
        &self,
        i: usize,
        n: u32,
        slots: &[Option<Value>],
        seat: Option<Option<Seat>>,
    ) -> Result<Value, GraphError> {
        let count = cells(n);
        let field = |p: usize| self.input_field(slots, i, p);
        let raster = |texels: Vec<f64>| Value::Field(Plane::Raster(texels));
        let kind = &self.graph.nodes[i].kind;
        Ok(match kind {
            NodeKind::Noise { .. } => {
                let seed = self.graph.seed;
                let voice = kind.voice().unwrap_or_default();
                raster(bake::sample_grid(n, |u, v| voice.eval(u, v, seed)))
            }
            NodeKind::Constant { value } => Value::Field(Plane::Uniform(*value)),
            NodeKind::Gradient { axis, repeat } => raster(bake::sample_grid(n, |u, v| {
                nodes::gradient(*axis, *repeat, u, v)
            })),
            NodeKind::Shape {
                pattern,
                cells: across,
                mortar,
            } => raster(bake::sample_grid(n, |u, v| {
                nodes::shape(*pattern, *across, *mortar, u, v)
            })),
            NodeKind::Levels { lo, hi, gamma } => {
                Value::Field(field(0)?.map(|h| nodes::levels(h, *lo, *hi, *gamma)))
            }
            NodeKind::Contrast { pivot, amount } => {
                Value::Field(field(0)?.map(|h| nodes::contrast(h, *pivot, *amount)))
            }
            NodeKind::Invert {} => Value::Field(field(0)?.map(|h| 1.0 - h)),
            NodeKind::Blur { radius } => raster(nodes::blur(&field(0)?.texels(count), n, *radius)),
            NodeKind::Threshold { at, soft } => {
                Value::Field(field(0)?.map(|h| nodes::threshold(h, *at, *soft)))
            }
            NodeKind::Band { at, width, soft } => {
                let src = field(0)?;
                let seat = match seat {
                    Some(seat) => seat,
                    None => Seat::of(&f32_raster(src, count), *at, *width),
                };
                Value::Field(match seat {
                    Some(s) => {
                        src.map(|h| output::band_window(h as f32, s.edge, s.close, *soft) as f64)
                    }
                    None => Plane::Uniform(0.0),
                })
            }
            NodeKind::Curvature { radius } => {
                raster(nodes::curvature(&f32_raster(field(0)?, count), n, *radius))
            }
            NodeKind::Warp { amount } => {
                raster(nodes::warp(&field(0)?.texels(count), field(1)?, n, *amount))
            }
            NodeKind::Tile { repeat } => Value::Field(nodes::tile(field(0)?, n, *repeat)),
            NodeKind::Modulate { base, amount } => {
                Value::Field(field(0)?.map(|h| output::modulate(*base, *amount, h as f32) as f64))
            }
            NodeKind::Occlusion { strength } => raster(nodes::occlusion(
                &f32_raster(field(0)?, count),
                n,
                *strength,
            )),
            NodeKind::Blend { mode, amount } => {
                let (a, b) = (field(0)?, field(1)?);
                let mask = self.input_mask(slots, i, 2)?;
                // Unmasked, the amount is passed through untouched — the exact fold the
                // rack's `mix` makes; a mask scales it texel by texel.
                let one = |t: usize| {
                    let amt = mask.map_or(*amount, |m| *amount * m.at(t));
                    mode.apply(a.at(t), b.at(t), amt)
                };
                Value::Field(uniform_or_raster(
                    a.is_uniform() && b.is_uniform() && mask.is_none_or(Plane::is_uniform),
                    count,
                    one,
                ))
            }
            NodeKind::Ramp { ramp } => Value::Colour(field(0)?.map(|h| ramp.sample(h as f32))),
            NodeKind::Palette { rgb } => Value::Colour(Plane::Uniform(*rgb)),
            NodeKind::MixColor { amount } => {
                let a = self.input_colour(slots, i, 0)?;
                let b = self.input_colour(slots, i, 1)?;
                let mask = self.input_mask(slots, i, 2)?;
                let one = |t: usize| {
                    let k = mask.map_or(*amount as f64, |m| *amount as f64 * m.at(t));
                    nodes::mix_color(a.at(t), b.at(t), k)
                };
                Value::Colour(uniform_or_raster(
                    a.is_uniform() && b.is_uniform() && mask.is_none_or(Plane::is_uniform),
                    count,
                    one,
                ))
            }
        })
    }

    /// Encode one output map from its now-final inputs, with the texel encoders `bake`
    /// writes its own maps with.
    fn encode(
        &self,
        job: Job,
        n: u32,
        relief: f32,
        slots: &[Option<Value>],
        maps: &mut [Option<Vec<u8>>; 7],
    ) -> Result<(), GraphError> {
        let count = cells(n);
        match job {
            Job::Base(i) => {
                let c = self.colour(slots, i)?;
                maps[0] = Some(encode(count, |t| bake::color_texel(c.at(t))));
            }
            Job::Surface(i) => {
                let (normal, height) =
                    surface(&f32_raster(self.field(slots, i)?, count), n, relief);
                maps[1] = Some(normal);
                maps[5] = Some(height);
            }
            Job::Level(slot, i) => {
                let f = self.field(slots, i)?;
                maps[slot] = Some(encode(count, |t| bake::scalar_texel(f.at(t) as f32)));
            }
            Job::Emit(color, mask, strength) => {
                let c = self.colour(slots, color)?;
                let m = mask.map(|m| self.field(slots, m)).transpose()?;
                // colour × mask × strength: the output stage's `emissive_at` is this same
                // product, band × strength, so a converted glow lands on the same bytes.
                maps[6] = Some(encode(count, |t| {
                    let e = m.map_or(1.0, |m| m.at(t) as f32).clamp(0.0, 1.0) * strength;
                    bake::emit_texel(c.at(t), e)
                }));
            }
        }
        Ok(())
    }
}

/// A pointwise node over several inputs: uniform when every input is.
fn uniform_or_raster<T>(uniform: bool, count: usize, one: impl Fn(usize) -> T) -> Plane<T> {
    if uniform {
        Plane::Uniform(one(0))
    } else {
        Plane::Raster((0..count).map(one).collect())
    }
}

/// `texel(i)` for every texel, row-major, as map bytes.
fn encode(count: usize, texel: impl Fn(usize) -> [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count * 4);
    for i in 0..count {
        bytes.extend_from_slice(&texel(i));
    }
    bytes
}

/// The Normal and Height maps of a height field. The one map pair that needs the
/// whole field at once — the slope reads its neighbours — and it reads it at `f32`,
/// as `bake` reads its height buffer, because that is the precision the shared
/// gradient → normal is defined on.
fn surface(height: &[f32], n: u32, relief: f32) -> (Vec<u8>, Vec<u8>) {
    let step = bake::relief_step(n);
    let mut normal = Vec::with_capacity(height.len() * 4);
    for y in 0..n as i64 {
        for x in 0..n as i64 {
            normal.extend_from_slice(&bake::normal_texel(height, n, x, y, relief, step));
        }
    }
    (
        normal,
        encode(height.len(), |i| bake::scalar_texel(height[i])),
    )
}

/// Bake a graph into the full map set at `size × size` — the same seven
/// [`MapKind`]s, encoded exactly as [`bake`](crate::bake()) encodes them:
///
/// - **BaseColor** from the `base` colour (unwired: the neutral grey);
/// - **Normal** from the `height` field's wrapped gradient at `relief`;
/// - **Roughness / Metallic / AO** from their scalars or fields;
/// - **Height** as the field itself (unwired: flat zero);
/// - **Emit** = colour × mask × strength (black when unwired).
///
/// Only what the output's wired ports reach is validated and evaluated; a broken node
/// nothing reads does not stop the bake (see [`problems`](super::problems)). Each map
/// is encoded the moment its inputs are final and their planes dropped, so the bake
/// holds the seven maps' bytes plus the graph's live frontier, not every node at once.
/// A node whose texels are not finite at map precision is refused with
/// [`GraphError::NonFiniteTexel`], naming it — one scan per evaluated node — rather
/// than quantized to a silent black.
pub fn bake_graph(graph: &MaterialGraph, size: u32) -> Result<MapSet, GraphError> {
    let n = size.max(1);
    let count = cells(n);
    let index = key_index(graph)?;
    let (ports, problems) = inspect_output(graph, &index);
    if let Some(e) = problems.into_iter().next() {
        return Err(e);
    }
    let relief = graph.output.relief;

    // Ports with nothing to wait for are encoded now; the rest become jobs that run
    // the moment their nodes are final.
    let mut maps: [Option<Vec<u8>>; 7] = Default::default();
    let mut jobs = Vec::new();
    match ports.base {
        Some(i) => jobs.push(Job::Base(i)),
        None => maps[0] = Some(encode(count, |_| bake::color_texel(NEUTRAL))),
    }
    match ports.height {
        Some(i) => jobs.push(Job::Surface(i)),
        None => {
            let (normal, height) = surface(&vec![0.0; count], n, relief);
            maps[1] = Some(normal);
            maps[5] = Some(height);
        }
    }
    for (slot, level) in [(2, ports.roughness), (3, ports.metalness), (4, ports.ao)] {
        match level {
            Source::Node(i) => jobs.push(Job::Level(slot, i)),
            Source::Const(v) => maps[slot] = Some(encode(count, |_| bake::scalar_texel(v))),
        }
    }
    match ports.emit {
        Some((color, mask, strength)) => {
            jobs.push(Job::Emit(color, mask, strength.clamp(0.0, 1.0)))
        }
        None => maps[6] = Some(encode(count, |_| bake::emit_texel([0.0; 3], 0.0))),
    }

    let roots: Vec<usize> = jobs.iter().flat_map(|job| job.needs()).collect();
    let plan = Plan::reaching(graph, &index, &roots)?;
    let mut pins = vec![0usize; graph.nodes.len()];
    for &root in &roots {
        pins[root] += 1;
    }
    let mut done = vec![false; jobs.len()];
    plan.run(n, pins, true, &mut |i, slots| {
        let mut released = Vec::new();
        for (k, &job) in jobs.iter().enumerate() {
            let ready =
                !done[k] && job.needs().any(|j| j == i) && job.needs().all(|j| slots[j].is_some());
            if ready {
                plan.encode(job, n, relief, slots, &mut maps)?;
                done[k] = true;
                released.extend(job.needs());
            }
        }
        Ok(released)
    })?;

    let mut bytes: [Vec<u8>; 7] = Default::default();
    for (slot, map) in maps.into_iter().enumerate() {
        bytes[slot] = map.ok_or_else(|| GraphError::MissingInput {
            at: format!("output.{}", MapKind::ALL[slot].role()),
        })?;
    }
    Ok(bake::map_set(n, bytes))
}

/// Requested preview nodes, evaluated: the plan, the nodes in request order, and
/// their values.
struct Previewed<'g> {
    plan: Plan<'g>,
    roots: Vec<usize>,
    slots: Slots,
}

/// Evaluate the requested preview nodes — one plan, one pass over the UNION of what
/// they reach. Only that union is validated; the output is not consulted at all.
fn previews<'g>(
    graph: &'g MaterialGraph,
    keys: &[&str],
    wants: ValueKind,
    n: u32,
) -> Result<Previewed<'g>, GraphError> {
    let index = key_index(graph)?;
    let roots = keys
        .iter()
        .map(|key| resolve(graph, &index, key, wants, || "preview".into()))
        .collect::<Result<Vec<_>, _>>()?;
    let plan = Plan::reaching(graph, &index, &roots)?;
    let mut pins = vec![0usize; graph.nodes.len()];
    for &root in &roots {
        pins[root] += 1;
    }
    let slots = plan.run(n, pins, false, &mut |_, _| Ok(Vec::new()))?;
    Ok(Previewed { plan, roots, slots })
}

/// FIELD nodes evaluated at `size × size` in one pass, in request order, row-major
/// `f32` — the designer's per-node previews. Only what the keys reach is validated
/// (the output is not), a key naming a colour node is a [`GraphError::PortType`] (use
/// [`colours_of`]), and values come back raw: a preview shows a non-finite texel where
/// a bake refuses it.
pub fn fields_of(
    graph: &MaterialGraph,
    keys: &[&str],
    size: u32,
) -> Result<Vec<Vec<f32>>, GraphError> {
    let n = size.max(1);
    let p = previews(graph, keys, ValueKind::Field, n)?;
    p.roots
        .iter()
        .map(|&i| Ok(f32_raster(p.plan.field(&p.slots, i)?, cells(n))))
        .collect()
}

/// COLOUR nodes evaluated at `size × size` in one pass, in request order, row-major
/// linear RGB — the previews [`fields_of`] gives field nodes.
pub fn colours_of(
    graph: &MaterialGraph,
    keys: &[&str],
    size: u32,
) -> Result<Vec<Vec<[f32; 3]>>, GraphError> {
    let n = size.max(1);
    let p = previews(graph, keys, ValueKind::Colour, n)?;
    p.roots
        .iter()
        .map(|&i| Ok(p.plan.colour(&p.slots, i)?.texels(cells(n)).into_owned()))
        .collect()
}

/// One FIELD node's preview — [`fields_of`] for a single key.
pub fn field_of(graph: &MaterialGraph, key: &str, size: u32) -> Result<Vec<f32>, GraphError> {
    let mut one = fields_of(graph, &[key], size)?;
    one.pop().ok_or_else(|| GraphError::UnknownNode {
        key: key.into(),
        at: "preview".into(),
    })
}

/// One COLOUR node's preview — [`colours_of`] for a single key.
pub fn colour_of(graph: &MaterialGraph, key: &str, size: u32) -> Result<Vec<[f32; 3]>, GraphError> {
    let mut one = colours_of(graph, &[key], size)?;
    one.pop().ok_or_else(|| GraphError::UnknownNode {
        key: key.into(),
        at: "preview".into(),
    })
}
