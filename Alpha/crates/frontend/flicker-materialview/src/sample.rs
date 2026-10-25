//! The **material sample** — a material as a surface, not as six flat images.
//!
//! Roughness, metalness and normal relief barely read in a flat swatch: they are all
//! statements about how light LEAVES a surface, so judging them needs light moving across
//! one. This is the view that makes an output stage dialable instead of guesswork.
//!
//! # One sphere, one plane, every sample
//!
//! The geometry is the same for every material, so it is uploaded ONCE
//! ([`SampleBodies`]) and lent to every [`MaterialSample`] at render — a page of a hundred
//! spheres is a hundred draws of one mesh, not a hundred meshes. A sample owns only what
//! is per seat: its offscreen target, its seat, and its turntable.
//!
//! # A turntable, not a camera
//!
//! The sample spins under a fixed light rather than the camera orbiting it. That is the
//! right control for a swatch (you want the highlight to travel across the surface), it
//! needs no camera class, and it means this is not another copy of the orbit camera.
//!
//! # Zoom: how close, never from where
//!
//! A host may draw its sample closer — [`MaterialSample::zoom`], `1.0` being the stage's own
//! framing. The zoom divides the authored camera's DISTANCE and touches nothing else: `render`
//! hands the view the consumer's stage with `camera.dist / zoom` (the stage itself, borrowed
//! untouched, at `1.0`). The angle it is seen from, the height it looks at and the light all
//! stay the stage's authored data, because a turntable lacks exactly one degree of freedom —
//! how close — and a second orbit camera here would be a second FRAMING SOURCE competing with
//! the stage's (the engine's interactive `Orbit` is for a body a user flies around; this view
//! is authored). It is a magnification rather than a raw distance so `2.0` reads the way a
//! slider labelled Zoom reads — half the distance, about twice the size — and it steps
//! LINEARLY ([`MaterialSample::zoom_by`]) so a wheel notch and the slider's step are the same
//! increment of the same number. The range and the step are the HOST's authored slider
//! ([`ZoomDial`]), never this crate's.
//!
//! # What it draws is the consumer's authored stage
//!
//! The look — lighting, clear, framing, the HDR resolve and its bloom — is the
//! [`StageDef`] the consumer names, compiled by the one stage compiler; the `material`
//! layer IS the sample body. A stage that authors no `material` layer draws nothing and
//! says so, because a declaration nothing consumes is how this view once shipped lit by a
//! constant while its own `"lighting": "studio"` sat unused.

use std::borrow::Cow;
use std::ops::RangeInclusive;
use std::time::Duration;

use flicker::render::{
    build_textured_verts, FrameGraph, Mat4, MeshDrawOptions, MeshIndices, Rate, Rect, Renderer,
    StageCamera, StageDef, StageInputs, TexturedMeshHandle, Vec2,
};
use flicker::script::{UiNode, Value};
use flicker::ui::SurfaceSlot;
use flicker_globe::view::Seat;
use flicker_globe::GlobeView;

use crate::maps::SampleMaps;

/// The one layer kind a material sample draws: the sample body. A consumer's stage that
/// authors anything else is named once, never drawn as nothing in silence.
pub const SAMPLE_LAYERS: &[&str] = &["material"];
/// Turns per second of the turntable. Slow: the point is to watch a highlight travel, and
/// a fast spin reads as motion rather than as surface.
const SPIN_RATE: f32 = 0.15;

/// Which body the material is shown on.
///
/// Both are needed and neither is redundant: a sphere sweeps every surface angle past the
/// light at once, which is how you judge roughness and metalness; a plane shows the
/// texture undistorted, which is how you judge the pattern and the seam.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Body {
    #[default]
    Sphere,
    Plane,
}

impl Body {
    pub fn toggled(self) -> Self {
        match self {
            Body::Sphere => Body::Plane,
            Body::Plane => Body::Sphere,
        }
    }

    /// Stable id — the `$token` suffix a consumer's label resolves through.
    pub fn id(self) -> &'static str {
        match self {
            Body::Sphere => "sphere",
            Body::Plane => "plane",
        }
    }

    /// The body's label as a stringtable token — `$mtl_body_<id>`, the ONE spelling every
    /// host's Sphere/Plane toggle shows (the MaterialBench's Lit view, the Material
    /// Inspector).
    pub fn token(self) -> String {
        format!("$mtl_body_{}", self.id())
    }
}

/// One sample body's geometry: parallel position / normal / UV arrays, one entry per
/// vertex of a triangle soup. Named because the bare tuple is what `build_textured_verts`
/// consumes and it appears at every producer.
type Soup = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>);

/// A UV sphere as a triangle SOUP (three sequential vertices per triangle).
///
/// Soup rather than indexed geometry because [`build_textured_verts`] assigns one tangent
/// per consecutive triple; sharing vertices across triangles would average tangents across
/// the UV seam and put a visible crease down the sphere exactly where the texture wraps.
fn sphere_soup(sectors: usize, stacks: usize) -> Soup {
    let (mut pos, mut nrm, mut uv) = (Vec::new(), Vec::new(), Vec::new());
    let at = |i: usize, j: usize| -> ([f32; 3], [f32; 2]) {
        let u = i as f32 / sectors as f32;
        let v = j as f32 / stacks as f32;
        let (phi, theta) = (u * std::f32::consts::TAU, v * std::f32::consts::PI);
        let p = [
            theta.sin() * phi.cos(),
            theta.cos(),
            theta.sin() * phi.sin(),
        ];
        // `1 - v` so the texture's top row lands at the sphere's north pole, the same
        // top-origin convention the rig format uses.
        (p, [u, 1.0 - v])
    };
    for j in 0..stacks {
        for i in 0..sectors {
            let (a, auv) = at(i, j);
            let (b, buv) = at(i + 1, j);
            let (c, cuv) = at(i + 1, j + 1);
            let (d, duv) = at(i, j + 1);
            for (p, t) in [(a, auv), (b, buv), (c, cuv), (a, auv), (c, cuv), (d, duv)] {
                pos.push(p);
                // A unit sphere's position IS its normal.
                nrm.push(p);
                uv.push(t);
            }
        }
    }
    (pos, nrm, uv)
}

/// A unit plane standing upright, as a triangle soup.
fn plane_soup() -> Soup {
    let s = 1.15;
    let quad = [
        ([-s, -s], [0.0, 1.0]),
        ([s, -s], [1.0, 1.0]),
        ([s, s], [1.0, 0.0]),
        ([-s, -s], [0.0, 1.0]),
        ([s, s], [1.0, 0.0]),
        ([-s, s], [0.0, 0.0]),
    ];
    let (mut pos, mut nrm, mut uv) = (Vec::new(), Vec::new(), Vec::new());
    for ([x, y], t) in quad {
        pos.push([x, y, 0.0]);
        nrm.push([0.0, 0.0, 1.0]);
        uv.push(t);
    }
    (pos, nrm, uv)
}

/// The ONE sphere and ONE plane every sample on a screen is drawn with.
///
/// The host owns them: it builds them when it first has a [`Renderer`] (lazily, on the
/// first render that shows a sample — an explicit init step is a wiring order the next
/// caller can forget, and forgetting it renders NOTHING) and gives them back with
/// [`SampleBodies::release`] on scene exit.
pub struct SampleBodies {
    sphere: TexturedMeshHandle,
    plane: TexturedMeshHandle,
}

impl SampleBodies {
    /// Upload both bodies.
    pub fn new(r: &mut Renderer) -> Self {
        let mut upload = |(pos, nrm, uv): Soup| {
            let verts = build_textured_verts(0..pos.len(), |i| pos[i], |i| nrm[i], |i| uv[i]);
            let indices: Vec<u32> = (0..verts.len() as u32).collect();
            r.upload_textured_mesh(&verts, MeshIndices::U32(&indices))
        };
        Self {
            sphere: upload(sphere_soup(48, 24)),
            plane: upload(plane_soup()),
        }
    }

    /// The mesh `body` is drawn with.
    pub fn mesh(&self, body: Body) -> TexturedMeshHandle {
        match body {
            Body::Sphere => self.sphere,
            Body::Plane => self.plane,
        }
    }

    /// Give both meshes back (scene `exit`). Consumes the set: a freed body set cannot be
    /// drawn, and cannot be freed twice — the renderer reuses a freed slot, so a second
    /// free would take the mesh of whatever was uploaded into it since.
    pub fn release(self, r: &mut Renderer) {
        r.free_textured_mesh(self.sphere);
        r.free_textured_mesh(self.plane);
    }
}

/// What a declared pass drew — the record each render is compared against to tell a
/// `dirty`-rate seat whether its image changed. Generic over the maps only so the decision
/// tests without a renderer; a sample records its [`SampleMaps`] (albedo, the five bound
/// PBR handles and the gain). It records HANDLES, not pixels: a texture rewritten in place
/// under the same handle is invisible to it, which is what [`MaterialSample::mark_dirty`] is
/// for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn<M> {
    body: Body,
    spinning: bool,
    maps: M,
    /// The seat's SIZE — the image's shape — and not its position: moving a seat moves only
    /// the composite, which runs every frame anyway (the Doll refits on a shape change, the
    /// same rule).
    size: Vec2,
    /// The zoom the pass was framed at — a closer framing is a new image. Recorded on EVERY
    /// seat, a poster's included: no host zooms a poster today, but a record that skipped a
    /// field the picture depends on would keep a stale image the day one does.
    zoom: f32,
}

/// Whether this frame's pass is dirty — the `dirty` a [`Rate::Dirty`] seat re-renders on.
/// True when the host `marked` it (an in-place texture rewrite —
/// [`MaterialSample::mark_dirty`]), when nothing has been drawn since the sample was
/// (re)seated or released, when anything RECORDED changed — the body, the turntable, the
/// map handles, the gain, the seat's size, the zoom — and every frame the turntable turns
/// (its angle is new each frame). It compares handles, never pixels: an in-place rewrite is
/// dirty only when marked.
fn changed<M: PartialEq>(last: Option<&Drawn<M>>, now: &Drawn<M>, marked: bool) -> bool {
    marked || now.spinning || last != Some(now)
}

/// What a stage must author for a material sample to draw it as its consumer meant — THE
/// check: every sample runs it once against its stage (the backstop), and a host seating
/// many samples under one stage runs it ONCE itself, so a broken stage is named once rather
/// than once per sample. Names, each as `stages.<source>`: layers the sample cannot draw, no
/// `material` layer (the sample is not drawn), no `camera` (the sample is framed by whatever
/// set the camera last — the framing is the stage's, applied by the frame graph from the
/// definition, so an unauthored one is a picture nobody chose). Empty when the stage is
/// drawable as authored.
pub fn sample_stage_problems(stage: &StageDef, source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let undrawn = stage.layers_outside(SAMPLE_LAYERS);
    if !undrawn.is_empty() {
        problems.push(format!(
            "stages.{source} authors {undrawn:?} layers the material sample does not draw"
        ));
    }
    if !stage.has_layer("material") {
        problems.push(format!(
            "stages.{source} authors no `material` layer — the material sample is not drawn"
        ));
    }
    if stage.camera.is_none() {
        problems.push(format!(
            "stages.{source} authors no `camera` — the material sample is framed by whatever \
             set the camera last"
        ));
    }
    problems
}

/// How a seat's authored rate is DECLARED — the rate handed to the per-surface clock, and
/// whether the turntable turns.
///
/// **A `poster` is declared [`Rate::Dirty`], its turntable parked.** A poster keeps its image
/// until its dirty channel ([`changed`]) fires — new map handles, gain, body or seat shape, or
/// the host's mark — and then re-renders exactly once. A literal [`Rate::Poster`] would keep the
/// FIRST image forever, so a poster whose maps arrive after its first frame (decoded on a
/// worker, uploaded when they land) would show its stand-in for good; this is the Doll's rule
/// (a still doll asks for `Dirty`). Parked, because a poster shows ONE frame — a turning one
/// would be dirty every frame, a live surface by another name. Every other rate is declared
/// as authored, the turntable as set.
fn liveness(rate: Rate, spinning: bool) -> (Rate, bool) {
    match rate {
        Rate::Poster => (Rate::Dirty, false),
        other => (other, spinning),
    }
}

/// The stage as a sample at `zoom` is framed under it — THE stage `render` hands the view.
/// At `1.0` it is the consumer's stage itself, borrowed untouched (no copy on the frame
/// thread for the framing every poster and unzoomed seat draws at); otherwise a copy whose
/// camera stands `dist / zoom` from its target, every other authored fact — the angle, the
/// target height, the light, the recipe, the gain — as authored. A stage that authors no
/// camera has no framing to scale, so it, too, is handed through as it is (the stage check
/// names the missing camera).
fn framed(stage: &StageDef, zoom: f32) -> Cow<'_, StageDef> {
    match stage.camera {
        Some(camera) if zoom != 1.0 => {
            let mut zoomed = stage.clone();
            zoomed.camera = Some(StageCamera {
                dist: camera.dist / zoom,
                ..camera
            });
            Cow::Owned(zoomed)
        }
        _ => Cow::Borrowed(stage),
    }
}

/// A host's ZOOM DIAL: the range a sample's [`MaterialSample::zoom`] may take and how far one
/// wheel notch moves it — read off the host's zoom SLIDER exactly as its tree authors it, the
/// slider's own `min`, `max` and `step`. Those are the numbers the walker clamps that slider
/// to, snaps its drag to and steps its pad nudge by, so reading them here makes the tree the
/// ONE source of the range: the slider, the pad and the wheel move one number between one
/// pair of bounds by one increment, and no param or constant repeats them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomDial {
    /// The farthest the sample may be drawn — at most `1.0`, the stage's own framing.
    pub min: f32,
    /// The closest — at least `1.0`.
    pub max: f32,
    /// One wheel notch's move — the slider's own step.
    pub step: f32,
}

impl ZoomDial {
    /// Zoom OFF: the stage's authored framing and nothing else — a `1.0..=1.0` range and a
    /// notch that moves nothing. What a host draws when its slider will not read, after
    /// saying why ([`Self::authored`]).
    pub const OFF: Self = Self {
        min: 1.0,
        max: 1.0,
        step: 0.0,
    };

    /// The range, in the shape [`MaterialSample::zoom_by`] and [`MaterialSample::set_zoom`]
    /// take it.
    pub fn range(&self) -> RangeInclusive<f32> {
        self.min..=self.max
    }

    /// The dial the control `id` of `tree` authors (`file` names the scene file the error
    /// points at). FAIL LOUD, never a silent default: a tree with no such node, or one whose
    /// `min`, `max` or `step` is missing or not a finite number, or whose numbers make no zoom
    /// — `min` must lie in `(0, 1]` (the zoom DIVIDES the camera's distance, so 0 would put
    /// the camera at infinity, and every sample opens on the stage's own framing, `1.0`, so
    /// the range must hold it), `max` at least `1`, `step` above 0 (a notch that moves nothing
    /// is a dead wheel) — is ONE error naming the node and every key at fault, and the dial is
    /// [`Self::OFF`].
    pub fn authored(tree: &UiNode, id: &str, file: &str) -> Self {
        let Some(node) = find(tree, id) else {
            tracing::error!(
                "{file} authors no `{id}` — the sample's zoom is off: the stage's framing only"
            );
            return Self::OFF;
        };
        let read = |key: &str| -> Result<f32, String> {
            match node.props.get(key) {
                Some(Value::Number(n)) if (*n as f32).is_finite() => Ok(*n as f32),
                Some(other) => Err(format!("`{key}` is {other:?}, not a finite number")),
                None => Err(format!("`{key}` is missing")),
            }
        };
        let problems: Vec<String> = match (read("min"), read("max"), read("step")) {
            (Ok(min), Ok(max), Ok(step)) => {
                let mut problems = Vec::new();
                if !(min > 0.0 && min <= 1.0) {
                    problems.push(format!(
                        "`min` {min} is outside (0, 1] — the zoom divides the camera's \
                         distance, and the sample opens on the stage's framing (1)"
                    ));
                }
                if max < 1.0 {
                    problems.push(format!(
                        "`max` {max} is below 1 — the sample opens on the stage's framing (1)"
                    ));
                }
                if step <= 0.0 {
                    problems.push(format!(
                        "`step` {step} is not above 0 — a wheel notch would move nothing"
                    ));
                }
                if problems.is_empty() {
                    return Self { min, max, step };
                }
                problems
            }
            (min, max, step) => [min.err(), max.err(), step.err()]
                .into_iter()
                .flatten()
                .collect(),
        };
        tracing::error!(
            "{file}: the zoom control `{id}` will not read — {} — the sample's zoom is off: the \
             stage's framing only",
            problems.join("; ")
        );
        Self::OFF
    }
}

/// The node `id` names in `tree`, depth first.
fn find<'t>(tree: &'t UiNode, id: &str) -> Option<&'t UiNode> {
    if tree.id == id {
        return Some(tree);
    }
    tree.children.iter().find_map(|kid| find(kid, id))
}

/// One lit material sample seated in one surface: its offscreen target, its seat and its
/// turntable. The geometry is the host's ([`SampleBodies`]); the maps are the caller's
/// ([`SampleMaps`]), handed in each render.
pub struct MaterialSample {
    /// The `stages.<source>` this sample is drawn under — named in every warning about it.
    source: String,
    /// The offscreen target and its size — the engine's ONE surface-filler target
    /// lifecycle ([`GlobeView::render_pass`]), the same one the rig view and its doll use.
    view: GlobeView,
    /// Where and how this frame's image lands; `None` declares nothing.
    seat: Option<Seat>,
    /// Turntable angle, radians. Advanced by `dt` so the spin is frame-rate independent.
    spin: f32,
    pub spinning: bool,
    pub body: Body,
    /// How close the sample is drawn: the stage's authored camera distance is divided by it
    /// at render (`1.0` = the stage's own framing, `2.0` = from half the distance). It is the
    /// VIEW's, never the document's — a host keeps it across body toggles and material loads.
    /// Moved through [`Self::zoom_by`] / [`Self::set_zoom`], which keep it finite and inside
    /// the host's authored range ([`ZoomDial`]); a zoom of 0 or less would put the camera at
    /// or behind infinity.
    pub zoom: f32,
    /// What the last declared pass drew; `None` until the first render after a (re)seat or
    /// a release, so that render always publishes `dirty`.
    drawn: Option<Drawn<SampleMaps>>,
    /// The host rewrote a texture this sample wears IN PLACE since the last declared pass
    /// ([`Self::mark_dirty`]) — the record compares handles, so only the host can say so.
    /// Consumed by the next pass the sample declares.
    marked: bool,
    /// The stage's undrawn layers were named once; say it once, not per frame.
    layers_checked: bool,
}

impl MaterialSample {
    /// An unseated sample drawn under `stages.<source>` — a spinning sphere with no target
    /// yet. The source is the consumer's (it compiles the stage); the sample names it in its
    /// warnings so an authoring slip points at the block to fix.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            view: GlobeView::default(),
            seat: None,
            spin: 0.0,
            spinning: true,
            body: Body::default(),
            zoom: 1.0,
            drawn: None,
            marked: false,
            layers_checked: false,
        }
    }

    /// THE WHEEL LAW, one for every host that zooms a sample: `notches` (positive = closer,
    /// the wheel's sign) times `step`, ADDED — linear, so a notch is exactly one slider step
    /// and the wheel walks the very values the slider snaps to — clamped to `range`. Returns
    /// whether the zoom changed (a notch past either end changes nothing). A non-finite notch
    /// or step, or a range [`Self::set_zoom`] refuses, is refused: nothing changes and it
    /// answers `false`.
    pub fn zoom_by(&mut self, notches: f32, step: f32, range: RangeInclusive<f32>) -> bool {
        if !(notches.is_finite() && step.is_finite()) {
            return false;
        }
        self.set_zoom(self.zoom + notches * step, range)
    }

    /// Set the zoom outright — a slider's committed value — clamped to `range`. Returns whether
    /// it changed, so a host hands a control that changed nothing back to its Model. Refused
    /// (`false`, nothing changes) for a non-finite zoom, or a range that is not a finite
    /// `0 < lo <= hi`: `f32::clamp` would panic on a reversed or NaN range, and a zoom of 0 or
    /// less would stand the camera at or past infinity — so whatever range a host hands in,
    /// the zoom these laws leave is finite and positive.
    pub fn set_zoom(&mut self, zoom: f32, range: RangeInclusive<f32>) -> bool {
        let (lo, hi) = range.into_inner();
        if !(zoom.is_finite() && lo.is_finite() && hi.is_finite() && 0.0 < lo && lo <= hi) {
            return false;
        }
        let zoom = zoom.clamp(lo, hi);
        let changed = zoom != self.zoom;
        self.zoom = zoom;
        changed
    }

    /// Seat the sample in the `surface` slot the walker reserved for it — its rect, the
    /// node's sub-layer and tint, and its authored rate (a `poster` keeps its image until
    /// its dirty channel fires).
    /// `None` unseats: an unseated sample declares nothing and keeps its target for its
    /// return, and its first render back publishes `dirty`.
    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.seat = slot.map(Seat::from);
        if self.seat.is_none() {
            self.drawn = None;
        }
    }

    /// Seat the sample at a rect the HOST laid out, with the rate it decides — a sample on
    /// a card a canvas placed, which has no `surface` node of its own to reserve it.
    pub fn seat_at(&mut self, rect: Rect, layer: f32, tint: [f32; 4], rate: Rate) {
        self.seat = Some(Seat {
            rect,
            layer,
            tint,
            rate,
        });
    }

    /// Force the next declared pass DIRTY — the hook for a host that rewrites a texture this
    /// sample wears IN PLACE (`Renderer::update_texture` — the MaterialBench rewrites its seven
    /// preview maps this way). The dirty channel compares map HANDLES, gain, body and seat
    /// size, never pixels, so a rewrite under the same handle changes the picture without
    /// dirtying a `dirty` or `poster` seat: call this with every in-place rewrite. Held until
    /// the next pass the sample declares, which consumes it. (A `live` seat re-renders every
    /// frame regardless.)
    pub fn mark_dirty(&mut self) {
        self.marked = true;
    }

    /// Take the host's mark — `true` once after [`Self::mark_dirty`], then `false` until the
    /// next mark. Called by each pass the sample declares.
    fn take_mark(&mut self) -> bool {
        std::mem::take(&mut self.marked)
    }

    /// What a pass declared NOW records: this sample's body and zoom, the seat's `size`, the
    /// `maps` it wears and whether its turntable turns as declared (`spinning`, after
    /// `liveness`). THE constructor of the record `render` compares — generic over the maps so
    /// a test builds one without a renderer, the way the dirty decision itself is tested.
    fn record<M>(&self, maps: M, size: Vec2, spinning: bool) -> Drawn<M> {
        Drawn {
            body: self.body,
            spinning,
            maps,
            size,
            zoom: self.zoom,
        }
    }

    /// Advance the turntable — unless it is parked: stopped, or seated as a `poster`, which
    /// shows one frame (`liveness`).
    pub fn tick(&mut self, dt: Duration) {
        let rate = self.seat.map_or(Rate::Live, |s| s.rate);
        if liveness(rate, self.spinning).1 {
            self.spin = (self.spin + dt.as_secs_f32() * SPIN_RATE * std::f32::consts::TAU)
                .rem_euclid(std::f32::consts::TAU);
        }
    }

    /// Declare this frame's offscreen pass and composite it into the seat — at the seat's
    /// sub-layer above `base_layer`, with its tint, and honouring its rate (a `dirty` seat
    /// re-renders when its record changed — body, turntable, map handles, gain, seat size or
    /// zoom — or when the host marked an in-place texture rewrite ([`Self::mark_dirty`]),
    /// which a record of handles cannot see; a `poster` keeps its image until then, declared
    /// `dirty` with its turntable parked — see `liveness`). Unseated, or under a stage that
    /// authors no `material` layer, it declares nothing (and a mark waits for the next pass).
    ///
    /// `bodies` is the host's shared geometry, `maps` the material to wear, `stage` the
    /// consumer's compiled stage — its lighting, framing, HDR resolve and bloom are applied
    /// by the frame graph from the definition, its camera stood `dist / zoom` from its target
    /// (`framed`: the stage itself at zoom `1.0`).
    pub fn render(
        &mut self,
        r: &mut Renderer,
        fg: &mut FrameGraph<'_>,
        base_layer: f32,
        bodies: &SampleBodies,
        maps: &SampleMaps,
        stage: &StageDef,
    ) {
        let Some(seat) = self.seat else { return };
        // The BACKSTOP: each sample names its stage's problems once. A host seating many
        // samples under one stage checks it ONCE itself with the same predicate and seats
        // none on a broken stage, so a broken stage is named once, not once per sample.
        if !self.layers_checked {
            self.layers_checked = true;
            for problem in sample_stage_problems(stage, &self.source) {
                tracing::warn!("{problem}");
            }
        }
        if !stage.has_layer("material") {
            return;
        }
        // The seat's rate as DECLARED: a poster becomes `dirty` with its turntable parked.
        let (rate, turning) = liveness(seat.rate, self.spinning);
        // The `dirty` a `Rate::Dirty` seat re-renders on: did anything recorded change since
        // the last pass, or did the host mark an in-place rewrite? (A live seat renders
        // regardless.)
        let now = self.record(*maps, seat.rect.size, turning);
        let marked = self.take_mark();
        let mut inputs = StageInputs::default();
        inputs.with_dirty(changed(self.drawn.as_ref(), &now, marked));
        self.drawn = Some(now);

        let mesh = bodies.mesh(self.body);
        let SampleMaps {
            albedo,
            pbr,
            emissive_gain,
        } = *maps;
        // The SAMPLE turns, not the camera: the light stays put and the highlight travels
        // across the surface, which is the whole point of the view. Liveness is the seat's
        // `rate`, driven by the renderer's per-surface clock — a poster keeps its image until
        // its dirty channel says otherwise (the composite still runs); a `live` sample turns
        // every frame.
        let model = Mat4::from_rotation_y(self.spin);
        let options = MeshDrawOptions {
            emissive_gain,
            ..MeshDrawOptions::default()
        };
        // The zoom moves the stage's camera closer along its own line of sight — the frame
        // graph frames the pass from the definition it is handed, so the definition is what
        // carries it.
        let stage = framed(stage, self.zoom);
        self.view.render_pass(
            r,
            fg,
            Seat { rate, ..seat },
            base_layer,
            &stage,
            inputs,
            None,
            move |r| r.draw_textured_mesh_pbr(mesh, albedo, pbr, model, options),
        );
    }

    /// Give the target back and unseat (scene `exit`, or a page that dropped the sample) —
    /// a host that leaves holds GPU memory for a picture nobody is looking at otherwise.
    /// The next seated render draws into a fresh target, and publishes `dirty`.
    pub fn release(&mut self, r: &mut Renderer) {
        self.view.free(r);
        self.seat = None;
        self.drawn = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both bodies must be triangle SOUP — a whole number of triangles, one tangent per
    /// triple. A shared-vertex mesh would average tangents across the UV seam and crease
    /// the sphere exactly where the texture wraps.
    #[test]
    fn both_bodies_are_whole_triangle_soups() {
        for (pos, nrm, uv) in [sphere_soup(12, 6), plane_soup()] {
            assert_eq!(pos.len(), nrm.len());
            assert_eq!(pos.len(), uv.len());
            assert!(
                pos.len() >= 3 && pos.len() % 3 == 0,
                "{} vertices is not soup",
                pos.len()
            );
        }
    }

    /// The sphere is a UNIT sphere with outward normals — the pipeline lights it from the
    /// vertex normal, so an inward one would render inside-out.
    #[test]
    fn the_sphere_is_unit_with_outward_normals() {
        let (pos, nrm, uv) = sphere_soup(16, 8);
        for (p, n) in pos.iter().zip(&nrm) {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "radius {len}");
            let dot = p[0] * n[0] + p[1] * n[1] + p[2] * n[2];
            assert!(dot > 0.0, "normal points inward");
        }
        for [u, v] in uv {
            assert!(
                (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v),
                "uv {u},{v} off-sheet"
            );
        }
    }

    /// The turntable wraps and is frame-rate independent — the same elapsed time in one
    /// step or many must land on the same angle, or the spin speed would depend on how the
    /// frames happened to fall.
    #[test]
    fn the_turntable_wraps_and_ignores_frame_rate() {
        let mut a = MaterialSample::new("probe");
        a.tick(Duration::from_secs_f32(1.0));
        let mut b = MaterialSample::new("probe");
        for _ in 0..60 {
            b.tick(Duration::from_secs_f32(1.0 / 60.0));
        }
        assert!((a.spin - b.spin).abs() < 1e-3, "{} vs {}", a.spin, b.spin);
        assert!(
            a.spin < std::f32::consts::TAU,
            "the angle must stay wrapped"
        );

        // Stopped means stopped.
        let before = a.spin;
        a.spinning = false;
        a.tick(Duration::from_secs_f32(1.0));
        assert_eq!(a.spin, before);
    }

    #[test]
    fn the_body_toggle_round_trips() {
        let b = Body::default();
        assert_eq!(b.toggled().toggled(), b);
        assert_ne!(Body::Sphere.id(), Body::Plane.id());
    }

    /// THE STAGE CHECK, as one function: a framed stage drawing only the `material` layer
    /// passes clean; a stage with no camera, no `material` layer, or a layer the sample cannot
    /// draw has each named, against the source the caller passes — the words every sample's
    /// backstop warns, and a many-sample host's once-only check reports.
    #[test]
    fn the_stage_check_names_each_thing_a_sample_cannot_draw() {
        use flicker::render::{StageCamera, StageLayer};
        let good = StageDef {
            layers: vec![StageLayer::Material],
            camera: Some(StageCamera::default()),
            ..StageDef::default()
        };
        assert!(sample_stage_problems(&good, "probe").is_empty());

        let unframed = StageDef {
            camera: None,
            ..good.clone()
        };
        let p = sample_stage_problems(&unframed, "probe");
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("stages.probe") && p[0].contains("`camera`"));

        let wrong = StageDef {
            layers: vec![StageLayer::Shells],
            ..good.clone()
        };
        let p = sample_stage_problems(&wrong, "probe").join("\n");
        assert!(
            p.contains("[\"shells\"] layers") && p.contains("no `material` layer"),
            "{p}"
        );
    }

    /// THE DIRTY DECISION a `Rate::Dirty` seat re-renders on: the first pass after a
    /// (re)seat or release always draws; a still sample showing what it last drew does not;
    /// a turning one draws every frame; and ANY change to what the pass RECORDS — the body,
    /// the turntable starting or stopping, the map handles or gain, the seat's shape, the
    /// zoom — draws once more. The maps stand in as numbers: the decision is the same for any
    /// comparable map set, and this needs no renderer. (Pixels rewritten under the same
    /// handles are the host's to mark —
    /// `a_marked_sample_is_dirty_once_though_nothing_recorded_changed`.)
    #[test]
    fn the_sample_is_dirty_exactly_when_its_record_changes() {
        let still = Drawn {
            body: Body::Sphere,
            spinning: false,
            maps: 1_u32,
            size: Vec2::new(200.0, 150.0),
            zoom: 1.0,
        };
        assert!(
            changed(None, &still, false),
            "nothing drawn yet: the first pass draws"
        );
        assert!(
            !changed(Some(&still), &still, false),
            "unchanged and still: no draw"
        );

        let turning = Drawn {
            spinning: true,
            ..still
        };
        assert!(
            changed(Some(&turning), &turning, false),
            "a turning sample is a new image every frame"
        );
        assert!(
            changed(Some(&turning), &still, false),
            "stopping the turntable draws the frame it stopped on"
        );

        for (what, now) in [
            (
                "the body",
                Drawn {
                    body: Body::Plane,
                    ..still
                },
            ),
            ("the maps", Drawn { maps: 2, ..still }),
            (
                "the seat's shape",
                Drawn {
                    size: Vec2::new(96.0, 96.0),
                    ..still
                },
            ),
            ("the zoom", Drawn { zoom: 2.5, ..still }),
        ] {
            assert!(
                changed(Some(&still), &now, false),
                "a change to {what} draws"
            );
            assert!(
                !changed(Some(&now), &now, false),
                "…once: {what} unchanged after"
            );
        }
    }

    /// **THE HOST'S MARK.** The record holds map HANDLES, so a texture a host rewrites in
    /// place (`update_texture`, same handle) changes the picture with nothing recorded
    /// changing: `changed` sees no difference, and only the host's mark dirties the pass.
    /// `mark_dirty` sets the mark, the next declared pass takes it (`take_mark`, the very
    /// call `render` makes) and it is gone after — one re-render per rewrite, for a poster
    /// seat as for a `dirty` one.
    #[test]
    fn a_marked_sample_is_dirty_once_though_nothing_recorded_changed() {
        let still = Drawn {
            body: Body::Sphere,
            spinning: false,
            maps: 1_u32,
            size: Vec2::new(96.0, 96.0),
            zoom: 1.0,
        };
        let mut s = MaterialSample::new("probe");
        assert!(!s.take_mark(), "a fresh sample carries no mark");
        assert!(
            !changed(Some(&still), &still, s.take_mark()),
            "same handles, no mark: the record sees no change"
        );

        s.mark_dirty();
        let (poster, _) = liveness(Rate::Poster, false);
        assert!(
            poster.renders(true, 0.0, changed(Some(&still), &still, s.take_mark())),
            "an in-place rewrite, marked, re-renders a poster"
        );
        assert!(
            !poster.renders(true, 0.0, changed(Some(&still), &still, s.take_mark())),
            "…once: the pass took the mark"
        );

        s.mark_dirty();
        s.mark_dirty();
        assert!(
            changed(Some(&still), &still, s.take_mark()),
            "two rewrites before a pass: one dirty pass"
        );
        assert!(!s.take_mark(), "and no second one");
    }

    /// **THE POSTER RULE.** An authored `poster` is declared `Rate::Dirty` with its
    /// turntable parked, so it re-renders exactly when its dirty channel fires — new map
    /// handles arriving off a worker after its first frame — and never otherwise; a literal
    /// `Rate::Poster` would keep that first image (the stand-in) forever. Every other rate is
    /// declared as authored, so the bench's `live` Lit view is untouched.
    #[test]
    fn a_poster_is_declared_dirty_with_its_turntable_parked() {
        assert_eq!(liveness(Rate::Poster, true), (Rate::Dirty, false));
        assert_eq!(liveness(Rate::Poster, false), (Rate::Dirty, false));
        assert_eq!(liveness(Rate::Live, true), (Rate::Live, true), "the bench");
        assert_eq!(liveness(Rate::Dirty, true), (Rate::Dirty, true));
        assert_eq!(liveness(Rate::Hz(20.0), false), (Rate::Hz(20.0), false));

        // A poster seat frame by frame, as the per-surface clock sees it.
        let (rate, turning) = liveness(Rate::Poster, true);
        let first = Drawn {
            body: Body::Sphere,
            spinning: turning,
            maps: 1_u32,
            size: Vec2::new(96.0, 96.0),
            zoom: 1.0,
        };
        assert!(
            rate.renders(false, 0.0, changed(None, &first, false)),
            "the first frame draws"
        );
        assert!(
            !rate.renders(true, 0.0, changed(Some(&first), &first, false)),
            "an unchanged poster keeps its image — the turntable switch on or not"
        );
        let arrived = Drawn { maps: 2, ..first };
        assert!(
            rate.renders(true, 0.0, changed(Some(&first), &arrived, false)),
            "its maps arriving re-render it"
        );
        assert!(
            !rate.renders(true, 0.0, changed(Some(&arrived), &arrived, false)),
            "…once"
        );
        assert!(
            !Rate::Poster.renders(true, 0.0, changed(Some(&first), &arrived, false)),
            "the literal rate would never have shown the late maps"
        );

        // Seated as a poster, the turntable is parked: ticking turns nothing.
        let rect = Rect {
            pos: Vec2::ZERO,
            size: Vec2::new(96.0, 96.0),
        };
        let mut s = MaterialSample::new("probe");
        s.seat_at(rect, 0.0, [1.0; 4], Rate::Poster);
        assert!(s.spinning, "the sample's own switch is untouched");
        s.tick(Duration::from_secs_f32(1.0));
        assert_eq!(s.spin, 0.0, "a poster shows one frame");
        s.seat_at(rect, 0.0, [1.0; 4], Rate::Live);
        s.tick(Duration::from_secs_f32(1.0));
        assert!(s.spin > 0.0, "a live seat turns");
    }

    /// A probe stage framed the way a lit host frames its sample.
    fn framed_probe() -> StageDef {
        use flicker::render::StageLayer;
        StageDef {
            layers: vec![StageLayer::Material],
            camera: Some(StageCamera {
                yaw: 0.6,
                pitch: 0.28,
                dist: 2.6,
                target_y: 0.1,
            }),
            emissive_gain: 4.0,
            ..StageDef::default()
        }
    }

    /// **THE STAGE A ZOOMED SAMPLE HANDS ITS VIEW.** `render` hands `GlobeView::render_pass`
    /// its stage through `framed` and nothing else; a render needs a GPU, so this observes that
    /// handoff the way the dirty tests observe `changed`. The authored camera stands
    /// `dist / zoom` from its target — closer past 1, farther below it — and every other
    /// authored fact (the angle, the target height, the layers, the gain) is the stage's own;
    /// the stage itself is never written.
    #[test]
    fn a_zoomed_sample_hands_its_view_the_stage_at_dist_over_zoom() {
        let stage = framed_probe();
        let authored = stage.camera.expect("the probe is framed");
        for zoom in [2.0_f32, 4.0, 1.25, 0.5] {
            let seen = framed(&stage, zoom);
            assert!(matches!(seen, Cow::Owned(_)), "zoom {zoom}: a patched copy");
            let camera = seen.camera.expect("still framed");
            assert_eq!(camera.dist, authored.dist / zoom, "zoom {zoom}");
            assert_eq!(
                StageCamera {
                    dist: authored.dist,
                    ..camera
                },
                authored,
                "zoom {zoom}: only the distance moved"
            );
            assert_eq!(seen.layers, stage.layers, "zoom {zoom}: the layers");
            assert_eq!(
                seen.emissive_gain, stage.emissive_gain,
                "zoom {zoom}: the gain"
            );
        }
        assert_eq!(stage.camera, Some(authored), "the stage was never written");
    }

    /// **ZOOM 1 IS THE STAGE ITSELF.** At the stage's own framing the view is handed the very
    /// stage the consumer compiled — borrowed, no copy on the frame thread — and a stage that
    /// authors no camera has no framing to scale, so it is handed through at any zoom.
    #[test]
    fn a_unit_zoom_hands_the_view_the_authored_stage_untouched() {
        let stage = framed_probe();
        assert!(
            matches!(framed(&stage, 1.0), Cow::Borrowed(s) if std::ptr::eq(s, &stage)),
            "the very stage, not a copy"
        );
        let unframed = StageDef {
            camera: None,
            ..framed_probe()
        };
        let seen = framed(&unframed, 2.0);
        assert!(
            matches!(seen, Cow::Borrowed(s) if std::ptr::eq(s, &unframed)),
            "no camera, nothing to scale"
        );
        assert!(seen.camera.is_none());
    }

    /// **THE WHEEL LAW** (`zoom_by`, every host's): a notch ADDS one step — linear, so the
    /// wheel walks the very values the slider snaps to — with the wheel's sign (positive =
    /// closer), clamped at either end, and a notch past an end, no notch or a zero step
    /// changes nothing and says so.
    #[test]
    fn the_wheel_law_steps_linearly_and_stops_at_either_end() {
        let mut s = MaterialSample::new("probe");
        assert_eq!(s.zoom, 1.0, "a sample opens on the stage's own framing");
        let range = || 1.0..=4.0;
        assert!(s.zoom_by(1.0, 0.25, range()));
        assert_eq!(s.zoom, 1.25, "one notch, one step");
        assert!(s.zoom_by(3.0, 0.25, range()));
        assert_eq!(s.zoom, 2.0, "three notches, three steps");
        assert!(s.zoom_by(-2.0, 0.25, range()));
        assert_eq!(s.zoom, 1.5, "a negative notch backs off");
        assert!(s.zoom_by(100.0, 0.25, range()));
        assert_eq!(s.zoom, 4.0, "clamped at the closest");
        assert!(!s.zoom_by(1.0, 0.25, range()), "past the end: no change");
        assert!(s.zoom_by(-100.0, 0.25, range()));
        assert_eq!(s.zoom, 1.0, "clamped at the farthest");
        assert!(!s.zoom_by(0.0, 0.25, range()), "no notch: no change");
        assert!(!s.zoom_by(1.0, 0.0, range()), "a zero step: no change");
        assert_eq!(s.zoom, 1.0);
    }

    /// **THE SLIDER LAW** (`set_zoom`): a committed value lands clamped into the range, and
    /// the answer says whether the zoom MOVED — the same value again, or one that clamps to
    /// where the zoom already is, is no change (a host hands that control back to its Model).
    #[test]
    fn the_slider_law_clamps_and_says_whether_the_zoom_moved() {
        let mut s = MaterialSample::new("probe");
        let range = || 1.0..=4.0;
        assert!(s.set_zoom(2.5, range()));
        assert_eq!(s.zoom, 2.5);
        assert!(!s.set_zoom(2.5, range()), "the same value: no change");
        assert!(s.set_zoom(9.0, range()));
        assert_eq!(s.zoom, 4.0, "clamped at the top");
        assert!(s.set_zoom(0.1, range()));
        assert_eq!(s.zoom, 1.0, "clamped at the bottom");
        assert!(
            !s.set_zoom(-3.0, range()),
            "clamps to where it already is: no change"
        );
    }

    /// **BOTH LAWS REFUSE WHAT IS NOT A FINITE, POSITIVE ZOOM**: a NaN or infinite notch,
    /// step, value or range bound — a notch times a step that overflows to infinity included —
    /// a reversed range (on which `f32::clamp` would panic), and a range reaching 0 or below (a
    /// camera at or past infinity) change nothing and answer `false`.
    #[test]
    fn both_zoom_laws_refuse_what_is_not_a_finite_zoom() {
        let mut s = MaterialSample::new("probe");
        assert!(s.set_zoom(2.0, 1.0..=4.0));
        let refused = [
            ("a NaN notch", s.zoom_by(f32::NAN, 0.25, 1.0..=4.0)),
            (
                "an infinite notch",
                s.zoom_by(f32::INFINITY, 0.25, 1.0..=4.0),
            ),
            ("a NaN step", s.zoom_by(1.0, f32::NAN, 1.0..=4.0)),
            ("an infinite step", s.zoom_by(1.0, f32::INFINITY, 1.0..=4.0)),
            (
                "a notch that overflows",
                s.zoom_by(f32::MAX, f32::MAX, 1.0..=4.0),
            ),
            ("a reversed range", s.zoom_by(1.0, 0.25, 4.0..=1.0)),
            ("a NaN value", s.set_zoom(f32::NAN, 1.0..=4.0)),
            ("an infinite value", s.set_zoom(f32::INFINITY, 1.0..=4.0)),
            (
                "a negative infinity",
                s.set_zoom(f32::NEG_INFINITY, 1.0..=4.0),
            ),
            ("a NaN bound", s.set_zoom(2.5, f32::NAN..=4.0)),
            ("an infinite bound", s.set_zoom(2.5, 1.0..=f32::INFINITY)),
            ("a reversed range", s.set_zoom(2.5, 4.0..=1.0)),
            ("a range reaching 0", s.set_zoom(0.0, 0.0..=4.0)),
            ("a range below 0", s.zoom_by(-100.0, 0.25, -1.0..=4.0)),
        ];
        for (what, moved) in refused {
            assert!(!moved, "{what} was taken");
        }
        assert_eq!(s.zoom, 2.0, "nothing moved the zoom");
    }

    /// **A ZOOM DIRTIES THE NEXT PASS.** The record a sample makes (`record`, the one `render`
    /// compares) carries the sample's OWN zoom, so moving the zoom makes the next pass dirty —
    /// once — and leaving it alone does not.
    #[test]
    fn a_zoom_change_dirties_the_next_pass_once() {
        let mut s = MaterialSample::new("probe");
        let size = Vec2::new(200.0, 150.0);
        let before = s.record(1_u32, size, false);
        assert!(
            !changed(Some(&before), &s.record(1_u32, size, false), false),
            "nothing moved: clean"
        );
        assert!(s.set_zoom(2.0, 1.0..=4.0));
        let after = s.record(1_u32, size, false);
        assert_eq!(after.zoom, 2.0, "the record carries the sample's zoom");
        assert!(
            changed(Some(&before), &after, false),
            "a zoom dirties the next pass"
        );
        assert!(
            !changed(Some(&after), &s.record(1_u32, size, false), false),
            "…once"
        );
    }

    /// **A POSTER RECORDS ITS ZOOM like any seat.** No host zooms a poster (the catalog's
    /// spheres never receive one), but the record is honest: a poster-seated sample whose zoom
    /// moved re-renders exactly once, and keeps that image after.
    #[test]
    fn a_poster_seat_records_its_zoom_too() {
        let rect = Rect {
            pos: Vec2::ZERO,
            size: Vec2::new(96.0, 96.0),
        };
        let mut s = MaterialSample::new("probe");
        s.seat_at(rect, 0.0, [1.0; 4], Rate::Poster);
        let (rate, turning) = liveness(Rate::Poster, s.spinning);
        let first = s.record(1_u32, rect.size, turning);
        assert!(s.set_zoom(2.0, 1.0..=4.0));
        let closer = s.record(1_u32, rect.size, turning);
        assert!(
            rate.renders(true, 0.0, changed(Some(&first), &closer, false)),
            "a zoomed poster re-renders"
        );
        assert!(
            !rate.renders(true, 0.0, changed(Some(&closer), &closer, false)),
            "…once"
        );
    }

    /// A probe tree holding `control` two levels down, as a host's tree would.
    fn tree_holding(control: serde_json::Value) -> UiNode {
        flicker::script::parse_ui_json(&serde_json::json!({
            "component": "cell",
            "children": [{"component": "row", "children": [control]}]
        }))
        .expect("the probe tree parses")
    }

    /// **THE DIAL IS THE AUTHORED SLIDER**: its range and step are the slider's own `min`,
    /// `max` and `step`, found by id wherever the tree holds it.
    #[test]
    fn the_zoom_dial_is_the_authored_sliders_own_range_and_step() {
        let tree = tree_holding(serde_json::json!({
            "component": "slider", "id": "probe_zoom", "bind": "probe_zoom",
            "min": 0.5, "max": 3.0, "step": 0.5
        }));
        let dial = ZoomDial::authored(&tree, "probe_zoom", "probe.scene.json");
        assert_eq!(
            dial,
            ZoomDial {
                min: 0.5,
                max: 3.0,
                step: 0.5
            }
        );
        assert_eq!(dial.range(), 0.5..=3.0);
    }

    /// **A DIAL THAT WILL NOT READ IS OFF** (and says so — the host's gate counts the error):
    /// an absent control, a missing or non-numeric key, a range that cannot hold the stage's
    /// own framing or would divide by 0, a step that moves nothing — each is `OFF`, the
    /// stage's framing only, which no wheel or slider moves.
    #[test]
    fn a_zoom_dial_that_will_not_read_is_off() {
        use serde_json::{json, Value as J};
        fn slider(min: J, max: J, step: J) -> J {
            json!({
                "component": "slider", "id": "probe_zoom",
                "min": min, "max": max, "step": step
            })
        }
        for (case, control) in [
            ("absent", json!({"component": "text", "id": "elsewhere"})),
            (
                "no step",
                json!({"component": "slider", "id": "probe_zoom", "min": 1, "max": 4}),
            ),
            ("a text max", slider(json!(1), json!("4"), json!(0.25))),
            ("a min of 0", slider(json!(0), json!(4), json!(0.25))),
            (
                "a min past the framing",
                slider(json!(1.5), json!(4), json!(0.25)),
            ),
            (
                "a max short of the framing",
                slider(json!(0.5), json!(0.75), json!(0.25)),
            ),
            ("a zero step", slider(json!(1), json!(4), json!(0))),
            ("a negative step", slider(json!(1), json!(4), json!(-0.25))),
            (
                "an f32 overflow",
                slider(json!(1), json!(1e300), json!(0.25)),
            ),
        ] {
            let tree = tree_holding(control);
            assert_eq!(
                ZoomDial::authored(&tree, "probe_zoom", "probe.scene.json"),
                ZoomDial::OFF,
                "{case}"
            );
        }
        let (off, mut s) = (ZoomDial::OFF, MaterialSample::new("probe"));
        assert!(!s.zoom_by(1.0, off.step, off.range()), "OFF: the wheel");
        assert!(!s.set_zoom(3.0, off.range()), "OFF: the slider");
        assert_eq!(s.zoom, 1.0, "the stage's framing only");
    }
}
