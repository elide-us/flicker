//! The material GRAPH — flicker-texture v2.
//!
//! The rack folds six voices into ONE field and projects every map off it. The
//! graph lifts that limit without leaving the instrument's maths: a node is a
//! signal (a generator, a filter, a blend, a colour), a wire carries a whole field,
//! a layer is a [`NodeKind::Blend`] with a mask port, and ONE [`Output`] gives each
//! of the seven maps its own chain — emission included, as a colour × mask × strength
//! like any other output.
//!
//! It keeps the crate's two laws. **Seamless:** every generator is periodic on the
//! tile and every neighbourhood node (blur, curvature, occlusion, warp) wraps.
//! **Deterministic:** `(graph, size)` is the whole input, and a graph is a small
//! document whose maps are a rebuildable consequence.
//!
//! And it keeps the rack. [`MaterialGraph::from_recipe`] converts a v1
//! [`TextureRecipe`] LOSSLESSLY — the voices become noise nodes folded through blend
//! nodes, the output stage becomes the nodes that do what it did — and
//! [`bake_graph`] bakes the conversion byte-identically to [`bake`](crate::bake()),
//! because the two paths call the same blend, ramp, gradient, ring-cavity and
//! band-seating functions rather than two copies of them.
//!
//! # Precision
//!
//! A field is evaluated at `f64`, the precision the rack's bus folds at, so a
//! converted rack folds bit for bit. The nodes that ARE the v1 output stage's maths
//! (`Ramp`, `Modulate`, `Band`, `Occlusion`) read their input at `f32` — the
//! precision that stage reads its height buffer at — and the maps are `f32`
//! quantized to bytes, as before. Previews ([`field_of`], [`colour_of`]) hand back
//! `f32`.

use serde::{Deserialize, Serialize};

use flicker_materials::MaterialId;

use crate::channel::{BlendMode, Channel, NoiseKind};
use crate::output::{ColorRamp, OutputStage};
use crate::recipe::TextureRecipe;

mod check;
mod convert;
mod eval;
mod nodes;
#[cfg(test)]
mod tests;

pub use check::problems;
pub use eval::{bake_graph, colour_of, colours_of, field_of, fields_of};

/// The document version a [`MaterialGraph`] serializes as. v1 is the versionless
/// [`TextureRecipe`]; [`load_material_json`] reads both.
pub const GRAPH_VERSION: u32 = 2;

/// The most cells a `Noise` node may put across the tile. A cell finer than a texel
/// of the largest bake is already noise of noise; past this the node is refused with
/// [`GraphError::OutOfRange`] rather than sampled.
pub const MAX_NOISE_SCALE: u32 = 1 << 16;

/// The furthest a `Noise` node's domain warp may push, in cells.
pub const MAX_NOISE_WARP: f64 = 65_536.0;

/// The furthest lattice cell a `Noise` node may sample: its coordinate span (cells
/// across, plus half its warp, plus the lattice's +1 neighbour) times its finest
/// octave's frequency, `lacunarity^(octaves−1)` (at least ×2, the warp field's own).
///
/// The lattice of `flicker_primitive::noise` indexes cells as `i64` — it adds the +1
/// neighbour and multiplies the period by the octave step — so an unbounded reach
/// (a lacunarity of 64 over twelve octaves; a warp of 10¹⁹) overflows there and
/// panics. 2⁴⁰ sits 2²³ below that arithmetic, and 2¹² below `f64`'s exact-integer
/// range, so nothing a validated node samples can overflow.
pub const MAX_NOISE_REACH: f64 = 1_099_511_627_776.0;

/// The steepest slope a normal is built from, `2³²`: its square plus the other
/// axis's plus one stays under `2⁶⁶`, some `2⁶²` short of `f32::MAX` (≈ `2¹²⁸`).
const MAX_SLOPE: f32 = 4_294_967_296.0;

/// The most relief the output may ask of its height field (either sign), derived
/// rather than chosen. The baker's slope is `Δh · relief / step` with
/// `step = 2 / n` (`relief_step`); a height field's step between neighbours is at most
/// 1 (every node that varies stays within 0..1); and the finest step is the largest
/// bake rung's ([`BAKE_SIZES`](crate::BAKE_SIZES), 8K: `2⁻¹²`). So this is the relief
/// whose steepest slope at 8K is `MAX_SLOPE`: `2³² · 2⁻¹² = 2²⁰`. Past it the
/// normal's `dx² + dy² + 1` can overflow `f32` — flat normals at 10²⁰, NaN at 3·10³⁸ —
/// so the output refuses it with [`GraphError::OutOfRange`].
pub const MAX_RELIEF: f32 = MAX_SLOPE
    * crate::bake::relief_step(crate::size::BAKE_SIZES[crate::size::BAKE_SIZES.len() - 1].px);

fn graph_version() -> u32 {
    GRAPH_VERSION
}

/// A material: a seed, a set of keyed nodes, and the output they feed.
///
/// The authored artifact, exactly as the recipe was: a few hundred bytes of JSON
/// that bake to megabytes of maps at any resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialGraph {
    /// The document version — [`GRAPH_VERSION`]. Defaulted when absent, so a v2
    /// document that omits it still reads.
    #[serde(default = "graph_version")]
    pub version: u32,
    /// Stable id — the recipe's id carried over (`"granite"`), or a scratch name.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Which of the 256 materials this is the appearance of, if any.
    #[serde(default)]
    pub material: Option<MaterialId>,
    /// The one knob that re-rolls every noise node at once (each node's own `salt`
    /// separates it from its neighbours, as a voice's does).
    pub seed: u64,
    /// The nodes, in authoring order. Evaluation order is the wires', not this list's.
    pub nodes: Vec<Node>,
    /// Where the nodes land: the seven maps.
    pub output: Output,
}

/// One node: an author-visible key, what it does, and what feeds it.
///
/// An unknown field is refused, not ignored: everything but `key` and `inputs` is
/// handed to the kind, and every kind — each a variant WITH fields, even the
/// parameterless `Invert {}` — denies the fields it does not declare. The one
/// exception is inside a `Ramp`: its stops are the v1 [`RampStop`](crate::RampStop),
/// which tolerates unknown keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Stable and UNIQUE within the graph — wires and the output name nodes by it,
    /// and the designer places cards by it.
    pub key: String,
    /// What the node does, with its parameters inline (`"kind": "blend", "mode": …`).
    #[serde(flatten)]
    pub kind: NodeKind,
    /// Port → the key of the node feeding it, in [`NodeKind::ports`] order. `None`
    /// (or a short list) leaves a port unwired — legal only for an optional port.
    #[serde(default)]
    pub inputs: Vec<Option<String>>,
}

/// Which way a [`NodeKind::Gradient`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Across the tile.
    U,
    /// Down the tile.
    V,
    /// Out from each cell's centre.
    Radial,
}

/// The closed-form patterns a [`NodeKind::Shape`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// A square grid of tiles.
    Tiles,
    /// Running bond: courses of 2:1 bricks, every other course shifted half a brick.
    Bricks,
    /// A honeycomb of (very nearly regular) hexagons.
    Hex,
    /// A grid of round dots.
    Dots,
}

/// What a node does — the vocabulary of the graph.
///
/// GENERATORS take no input and make a field (or, `Palette`, a colour). FILTERS take
/// one field. `Warp` takes a field and the field it is pushed by. `Blend` and
/// `MixColor` take two inputs and an optional MASK. The ports of each kind are
/// [`NodeKind::ports`]; what it makes is [`NodeKind::output`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeKind {
    /// A tileable noise source — the rack's voice without its fold: a [`Channel`]'s
    /// source and shaping, evaluated by [`Channel::eval`] itself. Bounded by
    /// [`MAX_NOISE_SCALE`], [`MAX_NOISE_WARP`] and [`MAX_NOISE_REACH`].
    Noise {
        source: NoiseKind,
        /// Cells across the tile, and the lattice period.
        scale: u32,
        octaves: u32,
        lacunarity: f64,
        gain: f64,
        /// Domain warp, in cells.
        warp: f64,
        salt: u64,
        contrast: f64,
        invert: bool,
    },
    /// One value everywhere.
    Constant { value: f64 },
    /// A mirrored ramp `repeat` times across the tile (0 at the cell edges, 1 at the
    /// middle) — mirrored because a sawtooth would tear at the seam.
    Gradient { axis: Axis, repeat: u32 },
    /// A tileable pattern of `cells` across: 0 in the mortar, rising across a bevel to
    /// 1 on the face. `mortar` is the gap's width as a fraction of a cell.
    Shape {
        pattern: Pattern,
        cells: u32,
        mortar: f64,
    },
    /// Remap `[lo, hi]` onto `[0, 1]` (clamped), then a midtone `gamma` (`> 1`
    /// brightens). `hi` below `lo` inverts; `hi == lo` is a hard step.
    Levels { lo: f64, hi: f64, gamma: f64 },
    /// Push the field away from (`amount > 1`) or toward (`< 1`) `pivot`.
    Contrast { pivot: f64, amount: f64 },
    /// `1 − field`. Written `Invert {}`, not a unit variant: serde only denies
    /// unknown fields on a variant that has fields to deny against, and a misspelt
    /// `"input"` must be refused here like anywhere else.
    Invert {},
    /// A box blur of `radius` — a fraction of the TILE, so the same blur at every bake
    /// size — wrapping across the seam.
    Blur { radius: f64 },
    /// A step at `at`, softened into a linear ramp `soft` wide (`0` is hard).
    Threshold { at: f64, soft: f64 },
    /// WHERE a field lights — the output stage's emissive band as a node, and a soft
    /// window when it closes. A field with no range at all lights nowhere.
    ///
    /// - `at` is the opening edge, a FRACTION of the way up the input's own measured
    ///   range, seated by `bake`'s own seating. Over generators and pointwise nodes the
    ///   input is the same field at every size, so it is measured once at 32² and the
    ///   band sits in the same place at every size. Behind a re-rastering node (Blur,
    ///   Curvature, Tile, Warp, Occlusion) the field differs with size, so it is
    ///   measured at the BAKE size, where it is baked.
    /// - `width` is how far above `at` the window reaches, as a fraction of that range.
    ///   `1` or more never closes: the open v1 band, lit all the way up.
    /// - `soft` shapes the ramps. Open, it scales the fade-in from the edge toward the
    ///   field's ceiling — `1` fades all the way up, as the output stage does. Closed,
    ///   the ramps span `soft` × half the window, held to `(0, 1]`: at `1` they meet in
    ///   the middle and the light peaks at full; softer could only dim it. Toward `0`
    ///   either hardens into steps.
    Band { at: f32, width: f32, soft: f32 },
    /// An edge/cavity measure: `0.5` where the field is level with the ring `radius`
    /// (a fraction of the tile) around it, brighter on crests and edges, darker in
    /// cavities. Wraps.
    Curvature { radius: f64 },
    /// Push `field` by `amount` (a fraction of the tile) in the direction the `by`
    /// field points (`0..1` = once round the compass). Wraps.
    Warp { amount: f64 },
    /// The input repeated `repeat` times across the tile, POINT-SAMPLED: texel `x` is
    /// input texel `x·repeat mod n`. Exact at each size, and resolution-dependent by
    /// nature — a 16× tile at 64² keeps 4×4 texels of each repeat — so what reads it
    /// (a `Band`'s lit share, say) moves with the bake size until the repeat resolves.
    Tile { repeat: u32 },
    /// A level `base` swung `amount` either side of the field's midpoint, clamped —
    /// the output stage's roughness/metalness modulation as a node.
    Modulate { base: f32, amount: f32 },
    /// Ring-cavity ambient occlusion of the field at `strength` — the output stage's
    /// occlusion as a node: `1` open, darker where a texel sits below the ring a
    /// twelfth of the tile around it. Wraps.
    Occlusion { strength: f32 },
    /// Fold `b` into `a` by `mode` at `amount` — [`BlendMode::apply`], with the
    /// amount scaled by the MASK where one is wired. A layer is a blend with a mask.
    Blend { mode: BlendMode, amount: f64 },
    /// Field → colour through a [`ColorRamp`].
    Ramp { ramp: ColorRamp },
    /// One colour everywhere — linear RGB. (The designer offers the curated glow
    /// palette; the graph stores the colour, as the recipe stores its glow.)
    Palette { rgb: [f32; 3] },
    /// Cross-fade colour `a` toward `b` by `amount`, scaled by the mask where wired.
    MixColor { amount: f32 },
}

/// Whether a value is a scalar FIELD or a COLOUR field — the type of a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// One scalar per texel.
    Field,
    /// One linear-RGB colour per texel.
    Colour,
}

impl std::fmt::Display for ValueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ValueKind::Field => "field",
            ValueKind::Colour => "colour",
        })
    }
}

/// One input port of a node kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Port {
    /// The port's name — what an error points at, and the label on a card.
    pub name: &'static str,
    /// What the port takes.
    pub kind: ValueKind,
    /// Whether the node cannot evaluate without it. Only masks are optional.
    pub required: bool,
}

const fn port(name: &'static str, kind: ValueKind, required: bool) -> Port {
    Port {
        name,
        kind,
        required,
    }
}

const NO_PORTS: &[Port] = &[];
const FIELD_PORT: &[Port] = &[port("field", ValueKind::Field, true)];
const WARP_PORTS: &[Port] = &[
    port("field", ValueKind::Field, true),
    port("by", ValueKind::Field, true),
];
const BLEND_PORTS: &[Port] = &[
    port("a", ValueKind::Field, true),
    port("b", ValueKind::Field, true),
    port("mask", ValueKind::Field, false),
];
const MIX_PORTS: &[Port] = &[
    port("a", ValueKind::Colour, true),
    port("b", ValueKind::Colour, true),
    port("mask", ValueKind::Field, false),
];

impl NodeKind {
    /// The serialized tag — the `"kind"` a node is written under, and a stable id
    /// for styles and labels.
    pub fn id(&self) -> &'static str {
        match self {
            NodeKind::Noise { .. } => "noise",
            NodeKind::Constant { .. } => "constant",
            NodeKind::Gradient { .. } => "gradient",
            NodeKind::Shape { .. } => "shape",
            NodeKind::Levels { .. } => "levels",
            NodeKind::Contrast { .. } => "contrast",
            NodeKind::Invert {} => "invert",
            NodeKind::Blur { .. } => "blur",
            NodeKind::Threshold { .. } => "threshold",
            NodeKind::Band { .. } => "band",
            NodeKind::Curvature { .. } => "curvature",
            NodeKind::Warp { .. } => "warp",
            NodeKind::Tile { .. } => "tile",
            NodeKind::Modulate { .. } => "modulate",
            NodeKind::Occlusion { .. } => "occlusion",
            NodeKind::Blend { .. } => "blend",
            NodeKind::Ramp { .. } => "ramp",
            NodeKind::Palette { .. } => "palette",
            NodeKind::MixColor { .. } => "mix_color",
        }
    }

    /// The input ports, in the order [`Node::inputs`] lists them.
    pub fn ports(&self) -> &'static [Port] {
        match self {
            NodeKind::Noise { .. }
            | NodeKind::Constant { .. }
            | NodeKind::Gradient { .. }
            | NodeKind::Shape { .. }
            | NodeKind::Palette { .. } => NO_PORTS,
            NodeKind::Warp { .. } => WARP_PORTS,
            NodeKind::Blend { .. } => BLEND_PORTS,
            NodeKind::MixColor { .. } => MIX_PORTS,
            NodeKind::Levels { .. }
            | NodeKind::Contrast { .. }
            | NodeKind::Invert {}
            | NodeKind::Blur { .. }
            | NodeKind::Threshold { .. }
            | NodeKind::Band { .. }
            | NodeKind::Curvature { .. }
            | NodeKind::Tile { .. }
            | NodeKind::Modulate { .. }
            | NodeKind::Occlusion { .. }
            | NodeKind::Ramp { .. } => FIELD_PORT,
        }
    }

    /// What the node makes.
    pub fn output(&self) -> ValueKind {
        match self {
            NodeKind::Ramp { .. } | NodeKind::Palette { .. } | NodeKind::MixColor { .. } => {
                ValueKind::Colour
            }
            _ => ValueKind::Field,
        }
    }

    /// Whether the node RE-RASTERS: reads its input's texels rather than its value at a
    /// point (a neighbourhood, a resample). Such a node's output at one size is not
    /// its output at another sampled more finely — blur and curvature round their
    /// radius to whole texels, a tile point-samples, a warp interpolates — which is
    /// what a `Band` downstream of one has to seat against.
    pub fn is_raster(&self) -> bool {
        matches!(
            self,
            NodeKind::Blur { .. }
                | NodeKind::Curvature { .. }
                | NodeKind::Tile { .. }
                | NodeKind::Warp { .. }
                | NodeKind::Occlusion { .. }
        )
    }

    /// Every float parameter, by name — what the finiteness check walks. A ramp's
    /// stops and a palette's channels report under the one parameter that holds them.
    fn float_params(&self) -> Vec<(&'static str, f64)> {
        let wide = |v: f32| v as f64;
        match self {
            NodeKind::Noise {
                lacunarity,
                gain,
                warp,
                contrast,
                ..
            } => vec![
                ("lacunarity", *lacunarity),
                ("gain", *gain),
                ("warp", *warp),
                ("contrast", *contrast),
            ],
            NodeKind::Constant { value } => vec![("value", *value)],
            NodeKind::Gradient { .. } | NodeKind::Invert {} | NodeKind::Tile { .. } => vec![],
            NodeKind::Shape { mortar, .. } => vec![("mortar", *mortar)],
            NodeKind::Levels { lo, hi, gamma } => {
                vec![("lo", *lo), ("hi", *hi), ("gamma", *gamma)]
            }
            NodeKind::Contrast { pivot, amount } => vec![("pivot", *pivot), ("amount", *amount)],
            NodeKind::Blur { radius } | NodeKind::Curvature { radius } => {
                vec![("radius", *radius)]
            }
            NodeKind::Threshold { at, soft } => vec![("at", *at), ("soft", *soft)],
            NodeKind::Band { at, width, soft } => vec![
                ("at", wide(*at)),
                ("width", wide(*width)),
                ("soft", wide(*soft)),
            ],
            NodeKind::Warp { amount } | NodeKind::Blend { amount, .. } => {
                vec![("amount", *amount)]
            }
            NodeKind::Modulate { base, amount } => {
                vec![("base", wide(*base)), ("amount", wide(*amount))]
            }
            NodeKind::Occlusion { strength } => vec![("strength", wide(*strength))],
            NodeKind::Ramp { ramp } => ramp
                .stops
                .iter()
                .flat_map(|stop| [stop.at, stop.color[0], stop.color[1], stop.color[2]])
                .map(|v| ("ramp", wide(v)))
                .collect(),
            NodeKind::Palette { rgb } => rgb.iter().map(|v| ("rgb", wide(*v))).collect(),
            NodeKind::MixColor { amount } => vec![("amount", wide(*amount))],
        }
    }

    /// Everything wrong with the node's own parameters: each non-finite float (once
    /// per parameter), and a noise node past its bounds.
    pub(crate) fn param_problems(&self, key: &str) -> Vec<GraphError> {
        let mut problems: Vec<GraphError> = Vec::new();
        for (param, v) in self.float_params() {
            let seen = problems
                .iter()
                .any(|p| matches!(p, GraphError::NonFinite { param: q, .. } if *q == param));
            if !v.is_finite() && !seen {
                problems.push(GraphError::NonFinite {
                    node: key.into(),
                    param,
                });
            }
        }
        if let NodeKind::Noise {
            scale,
            octaves,
            lacunarity,
            warp,
            ..
        } = *self
        {
            // Bounds are only meaningful over finite numbers; a NaN was reported above.
            if problems.is_empty() {
                let out = |param| GraphError::OutOfRange {
                    node: key.into(),
                    param,
                };
                if scale > MAX_NOISE_SCALE {
                    problems.push(out("scale"));
                }
                if warp.abs() > MAX_NOISE_WARP {
                    problems.push(out("warp"));
                }
                // With the span in bounds, a reach past the lattice's is the octaves'
                // doing — the lacunarity they compound.
                let finest = lacunarity
                    .max(1.0)
                    .powi(octaves.clamp(1, 12) as i32 - 1)
                    .max(2.0);
                let reach = (scale.max(1) as f64 + 0.5 * warp.abs() + 1.0) * finest;
                if problems.is_empty() && reach > MAX_NOISE_REACH {
                    problems.push(out("lacunarity"));
                }
            }
        }
        problems
    }

    /// A noise generator speaking `channel`'s voice — its source and shaping, without
    /// the fold (`enabled`, `blend` and `amount` belong to the rack, and in a graph
    /// to the [`NodeKind::Blend`] that takes it).
    pub fn noise(channel: &Channel) -> Self {
        NodeKind::Noise {
            source: channel.source,
            scale: channel.scale,
            octaves: channel.octaves,
            lacunarity: channel.lacunarity,
            gain: channel.gain,
            warp: channel.warp,
            salt: channel.salt,
            contrast: channel.contrast,
            invert: channel.invert,
        }
    }

    /// The rack voice a `Noise` node speaks — the inverse of [`NodeKind::noise`], as
    /// an enabled channel at `Base`, fully open (the fold is never the voice's own).
    /// `None` for every other kind.
    pub fn voice(&self) -> Option<Channel> {
        match *self {
            NodeKind::Noise {
                source,
                scale,
                octaves,
                lacunarity,
                gain,
                warp,
                salt,
                contrast,
                invert,
            } => Some(Channel {
                enabled: true,
                source,
                scale,
                octaves,
                lacunarity,
                gain,
                warp,
                salt,
                contrast,
                invert,
                blend: BlendMode::Base,
                amount: 1.0,
            }),
            _ => None,
        }
    }

    /// One of every kind at a neutral setting, in menu order (generators, filters,
    /// blend, colour) — the designer's insertion list. Where the v1 instrument has a
    /// default (a fresh voice, the fresh output stage) the kind takes it.
    pub fn catalog() -> Vec<NodeKind> {
        let voice = Channel::default();
        let stage = OutputStage::default();
        vec![
            NodeKind::noise(&voice),
            NodeKind::Constant { value: 0.5 },
            NodeKind::Gradient {
                axis: Axis::U,
                repeat: 1,
            },
            NodeKind::Shape {
                pattern: Pattern::Tiles,
                cells: voice.scale,
                mortar: 0.1,
            },
            NodeKind::Levels {
                lo: 0.0,
                hi: 1.0,
                gamma: 1.0,
            },
            NodeKind::Contrast {
                pivot: 0.5,
                amount: voice.contrast,
            },
            NodeKind::Invert {},
            NodeKind::Blur { radius: 0.02 },
            NodeKind::Threshold { at: 0.5, soft: 0.1 },
            NodeKind::Band {
                at: stage.emissive_band,
                width: 1.0,
                soft: 1.0,
            },
            NodeKind::Curvature { radius: 1.0 / 12.0 },
            NodeKind::Warp { amount: 0.05 },
            NodeKind::Tile { repeat: 2 },
            NodeKind::Modulate {
                base: stage.roughness,
                amount: stage.roughness_mod,
            },
            NodeKind::Occlusion { strength: stage.ao },
            NodeKind::Blend {
                mode: voice.blend,
                amount: voice.amount,
            },
            NodeKind::Ramp { ramp: stage.ramp },
            NodeKind::Palette {
                rgb: stage.emissive,
            },
            NodeKind::MixColor { amount: 0.5 },
        ]
    }
}

/// A scalar output port: one value everywhere, or a field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    /// One value over the whole map (`"roughness": 0.7`).
    Const(f32),
    /// The key of the field node that drives the map (`"roughness": "rough"`).
    Field(String),
}

/// The emission port: a colour, where it shows, and how bright.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Emit {
    /// The key of the COLOUR node the glow takes.
    pub color: String,
    /// The key of the FIELD that says where it glows; unwired glows everywhere.
    #[serde(default)]
    pub mask: Option<String>,
    /// `0..=1`. The texel is colour × mask × strength; brightness past the surface's
    /// own white is the renderer's emissive gain, not this.
    pub strength: f32,
}

/// The one output node: seven ports, one per map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// COLOUR → BaseColor. Unwired bakes the crate's neutral grey (visible, not black).
    #[serde(default)]
    pub base: Option<String>,
    /// FIELD → Height, and the Normal map's slope. Unwired is a flat zero field.
    #[serde(default)]
    pub height: Option<String>,
    /// How far the height's gradient tips the normal (`0` = flat).
    pub relief: f32,
    pub roughness: Scalar,
    pub metalness: Scalar,
    /// The AO map. Drive it by an `Occlusion` node for ring-cavity AO, or by any field.
    pub ao: Scalar,
    /// Unwired writes a black Emit map, which the shader adds as nothing.
    #[serde(default)]
    pub emit: Option<Emit>,
}

/// The output node's seven ports, in card order — the ONE table of what each takes. The
/// validator resolves the output through it (its errors name a port `output.<name>`), and
/// a designer lays its output card out by it. Each may be left unwired — the map rests —
/// except `emit.color`, which an emission cannot do without.
pub const OUTPUT_PORTS: [Port; 7] = [
    port("base", ValueKind::Colour, false),
    port("height", ValueKind::Field, false),
    port("roughness", ValueKind::Field, false),
    port("metalness", ValueKind::Field, false),
    port("ao", ValueKind::Field, false),
    port("emit.color", ValueKind::Colour, true),
    port("emit.mask", ValueKind::Field, false),
];

impl MaterialGraph {
    /// Whether the graph GLOWS at all: an Emit wired at a strength above zero. THE one
    /// predicate — what the MaterialBench's Commit weighs an EMISSIVE material's look by,
    /// and what the PackageBench's review asks of a staged document: a look that does not
    /// glow is legal (the slot law weighs the index row, not the look) but warned.
    #[must_use]
    pub fn glows(&self) -> bool {
        self.output.emit.as_ref().is_some_and(|e| e.strength > 0.0)
    }
}

impl Output {
    /// The output's ports, in card order — [`OUTPUT_PORTS`].
    pub fn ports() -> &'static [Port] {
        &OUTPUT_PORTS
    }

    /// The key WRITTEN into each port, in [`OUTPUT_PORTS`] order — `None` for a port left
    /// unwired (a scalar port resting at its constant, no glow, no mask). Whether the key
    /// resolves is the validator's question ([`problems`]); this is the wiring as written,
    /// the shape a designer or a viewer draws the output card's wires from.
    pub fn sources(&self) -> [Option<&str>; OUTPUT_PORTS.len()] {
        fn field(s: &Scalar) -> Option<&str> {
            match s {
                Scalar::Field(key) => Some(key.as_str()),
                Scalar::Const(_) => None,
            }
        }
        [
            self.base.as_deref(),
            self.height.as_deref(),
            field(&self.roughness),
            field(&self.metalness),
            field(&self.ao),
            self.emit.as_ref().map(|e| e.color.as_str()),
            self.emit.as_ref().and_then(|e| e.mask.as_deref()),
        ]
    }
}

impl Default for Output {
    /// Nothing wired, and the fresh output stage's finish — matte, non-metallic, open.
    fn default() -> Self {
        let stage = OutputStage::default();
        Self {
            base: None,
            height: None,
            relief: stage.relief,
            roughness: Scalar::Const(stage.roughness),
            metalness: Scalar::Const(stage.metalness),
            ao: Scalar::Const(1.0),
            emit: None,
        }
    }
}

/// Why a graph cannot evaluate. Returned — never a panic, never a silent black map.
///
/// `at` names where the bad name was written: `"node.port"`, or `"output.<port>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// Two nodes share a key, so a wire to it would be ambiguous.
    DuplicateKey { key: String },
    /// A wire names a key that no node has.
    UnknownNode { key: String, at: String },
    /// A node lists an input past its kind's last port.
    UnknownPort { node: String, index: usize },
    /// A required port is unwired.
    MissingInput { at: String },
    /// A wire joins a port to a node of the wrong type (a colour into a field port).
    PortType {
        at: String,
        wants: ValueKind,
        source: String,
        found: ValueKind,
    },
    /// The wires loop. Each key feeds the one before it; the last repeats the first.
    Cycle { path: Vec<String> },
    /// A float parameter is NaN or infinite (`node` is `"output"` for the output's own).
    NonFinite { node: String, param: &'static str },
    /// A parameter is past the bound its arithmetic is safe to — see
    /// [`MAX_NOISE_SCALE`], [`MAX_NOISE_WARP`], [`MAX_NOISE_REACH`], [`MAX_RELIEF`].
    OutOfRange { node: String, param: &'static str },
    /// A node evaluated to a texel that is not finite at map precision — it would
    /// quantize to a silent black (or white) rather than to a surface.
    NonFiniteTexel { node: String },
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "material graph: ")?;
        match self {
            GraphError::DuplicateKey { key } => write!(f, "two nodes share the key '{key}'"),
            GraphError::UnknownNode { key, at } => {
                write!(f, "{at} names '{key}', which is not a node in the graph")
            }
            GraphError::UnknownPort { node, index } => write!(
                f,
                "node '{node}' lists an input at port {index}, past its last port"
            ),
            GraphError::MissingInput { at } => write!(f, "{at} is not connected"),
            GraphError::PortType {
                at,
                wants,
                source,
                found,
            } => write!(f, "{at} takes a {wants}, but '{source}' makes a {found}"),
            GraphError::Cycle { path } => write!(f, "the wires loop: {}", path.join(" ← ")),
            GraphError::NonFinite { node, param } => {
                write!(f, "{node}.{param} is not a finite number")
            }
            GraphError::OutOfRange { node, param } => {
                write!(
                    f,
                    "{node}.{param} is past the range its arithmetic is safe to"
                )
            }
            GraphError::NonFiniteTexel { node } => {
                write!(f, "node '{node}' evaluates to texels that are not finite")
            }
        }
    }
}

impl std::error::Error for GraphError {}

/// Why a material document did not load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Not JSON, or JSON that is not an object (an array, a number…).
    Syntax(String),
    /// A `version` this build cannot read — newer than it knows, or not a number.
    UnsupportedVersion(String),
    /// A document of a known version that does not match that version's schema.
    Invalid { version: u32, reason: String },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Syntax(why) => write!(f, "material document is not readable: {why}"),
            LoadError::UnsupportedVersion(v) => {
                write!(
                    f,
                    "material document version {v} is not one this build reads"
                )
            }
            LoadError::Invalid { version, reason } => {
                write!(f, "material document (v{version}) is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Read a material document: a v2 graph (`"version": 2`), or a v1 recipe — the
/// versionless [`TextureRecipe`] form (or one marked `"version": 1`) — converted by
/// [`MaterialGraph::from_recipe`]. Parsing only: the caller owns the file, and an
/// unsound graph loads and then refuses to bake with a [`GraphError`] that names
/// the bad wire, so a designer can open it and fix it ([`problems`] lists them all).
pub fn load_material_json(text: &str) -> Result<MaterialGraph, LoadError> {
    // An object first — anything else is not a material document at all — read just
    // far enough to dispatch on: the version, and whether it is shaped like a graph.
    let doc: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(text).map_err(|e| LoadError::Syntax(e.to_string()))?;
    let invalid = |version: u32, e: serde_json::Error| LoadError::Invalid {
        version,
        reason: e.to_string(),
    };
    let version = match doc.get("version") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| LoadError::UnsupportedVersion(v.to_string()))?,
        ),
    };
    // The typed reads parse the TEXT again, not the map, so every number takes the
    // one parse path a direct read would.
    match version {
        Some(GRAPH_VERSION) => serde_json::from_str(text).map_err(|e| invalid(GRAPH_VERSION, e)),
        Some(1) => serde_json::from_str::<TextureRecipe>(text)
            .map(|r| MaterialGraph::from_recipe(&r))
            .map_err(|e| invalid(1, e)),
        // Versionless is v1 by definition — unless it is a v2 graph that merely omits
        // the field, which the graph's own serde default tolerates. When it is
        // neither, the error that explains it is the one for the form it LOOKS like.
        None => match serde_json::from_str::<TextureRecipe>(text) {
            Ok(r) => Ok(MaterialGraph::from_recipe(&r)),
            Err(v1) => serde_json::from_str::<MaterialGraph>(text).map_err(|v2| {
                if doc.contains_key("nodes") || doc.contains_key("output") {
                    invalid(GRAPH_VERSION, v2)
                } else {
                    invalid(1, v1)
                }
            }),
        },
        Some(v) => Err(LoadError::UnsupportedVersion(v.to_string())),
    }
}
