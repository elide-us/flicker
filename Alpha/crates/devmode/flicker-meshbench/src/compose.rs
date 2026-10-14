//! **What the rig panels draw** — composed from the document each frame.
//!
//! Line batches are pure data → data: the ground grid and the collision volumes
//! (depth-tested), the skeleton, the selected joint's ball and the attach-point markers
//! (overlay, drawn over the body). The gizmo's HANDLES are not here: they depend on the
//! panel's projection, so the scene draws them per panel from the gadget. Draw items come from the
//! bench's mesh caches ([`ViewMeshes`]) as handles the bench owns. The `model_view`
//! panels draw exactly these — handed over as their `ViewContext` — and the behaviour
//! decides what a click on them means (`gizmo`).

use flicker::render::{grid_segments_xy, Mat4, MeshDrawOptions, Renderer, Vec3};
use flicker_content::AssetClass;
use flicker_globe::Arrows;
use flicker_mechanics::{debug, Shape};
use flicker_rigview::{Draw, GadgetStyle, Projection};
use flicker_skeletal::pose::{global_transforms, sample_local_poses};

use crate::meshes::{BakePreview, BasePreview, ViewMeshes};
use crate::services::{side_of, ClipPreview, Document, Parsed, Side};
use crate::ui::Step;

/// Joint balls — the cyan the whole editor uses for the rig.
pub(crate) const JOINT: [f32; 4] = [0.35, 0.9, 1.0, 1.0];
/// Bone diamonds between joints.
pub(crate) const BONE: [f32; 4] = [0.62, 0.50, 0.95, 1.0];
/// The selected joint's ball (amber — the gizmo's own accent).
pub(crate) const GIZMO_SEL: [f32; 4] = [1.0, 0.8, 0.15, 1.0];
/// Attach-point markers: idle and selected.
pub(crate) const MARKER: [f32; 4] = [0.722, 0.592, 0.353, 0.85];
pub(crate) const MARKER_SEL: [f32; 4] = [0.435, 0.592, 1.0, 1.0];
/// The floor grid the perspective panel stands the subject on.
pub(crate) const GROUND: [f32; 4] = [0.55, 0.63, 0.75, 0.16];
/// Collision volumes.
pub(crate) const COLLISION: [f32; 4] = [0.25, 1.0, 0.45, 0.9];
/// The fitting body a prop is mounted against: a dim, cool clay.
pub(crate) const BODY_TINT: [f32; 4] = [0.40, 0.44, 0.52, 1.0];
/// The subject (a flat-shaded character mesh).
pub(crate) const SUBJECT_TINT: [f32; 4] = [0.80, 0.79, 0.77, 1.0];
/// The mounted piece: warm, so it reads against the body.
pub(crate) const PIECE_TINT: [f32; 4] = [1.0, 0.74, 0.40, 1.0];

/// Joint-ball sizing: a fraction of the bone's length, clamped to a fraction of the
/// subject's radius.
const BALL_LEN_FRAC: f32 = 0.14;
const BALL_MIN_FRAC: f32 = 0.006;
const BALL_MAX_FRAC: f32 = 0.035;
/// Bone diamond waist as a fraction of the bone's length.
const BONE_WAIST_FRAC: f32 = 0.12;
/// Attach marker cross half-size as a fraction of the subject's radius (selected ×1.6).
const MARKER_FRAC: f32 = 0.04;
/// What a bone outside a panel's isolation keeps of its alpha when the panel DIMS rather than
/// hides — the perspective picker keeps its context at a glance.
const DIM_ALPHA: f32 = 0.35;
/// The origin cross's arm as a fraction of the subject's radius.
const CENTRE_ARM_FRAC: f32 = 0.12;
/// Dash lengths for the median / floor lines and the other panels' cut planes, as fractions of
/// the subject's radius.
const PLANE_DASH_FRAC: f32 = 0.035;
const CUT_DASH_FRAC: f32 = 0.07;

/// A colour at a fraction of its alpha — the dimmed twin of a skeleton colour.
fn dimmed(c: [f32; 4]) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * DIM_ALPHA]
}

/// Which bones a panel draws — DRAW-ONLY: edits, picks and the mirror never consult it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoneFilter {
    /// One side's limbs: every bone whose name carries that side's suffix — the arm chain
    /// clavicle → fingers and the leg chain thigh → toes; trunk, head and tail are out.
    Limb(Side),
    /// A joint and everything under it — the hand and its fingers (SOLO).
    Subtree(usize),
}

/// A panel's isolation: the filter, the bones it may never hide (the selected joint and its
/// reach chain — nothing selected can vanish), and whether the rest is dimmed (the perspective
/// picker) or hidden (an orthographic panel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Isolation {
    pub(crate) filter: BoneFilter,
    pub(crate) keep: Vec<usize>,
    pub(crate) dim: bool,
}

/// The isolation a panel draws `doc` under: `filter`, keeping the selected joint and its
/// reach chain whatever the filter says.
pub(crate) fn isolation(doc: &Document, filter: BoneFilter, dim: bool) -> Isolation {
    let mut keep = Vec::new();
    if let Some(sel) = doc.bone_sel() {
        keep.push(sel);
        keep.extend(doc.reach_chain(sel).unwrap_or_default());
    }
    Isolation { filter, keep, dim }
}

/// Per-bone: does the panel draw it in full? (The mask is the ONE truth the skeleton batches
/// follow, so a gate on it is a gate on the drawing.)
pub(crate) fn bone_mask(p: &Parsed, iso: &Isolation) -> Vec<bool> {
    let n = p.model.bones.len();
    let mut mask: Vec<bool> = match iso.filter {
        BoneFilter::Limb(side) => p
            .model
            .bones
            .iter()
            .map(|b| side_of(&b.name) == Some(side))
            .collect(),
        BoneFilter::Subtree(root) => {
            let mut m = vec![false; n];
            for i in p.subtree(root) {
                m[i] = true;
            }
            m
        }
    };
    for &k in &iso.keep {
        if let Some(m) = mask.get_mut(k) {
            *m = true;
        }
    }
    mask
}

/// The subject's framing: centre, bounding radius and the feet plane (absolute z).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Framing {
    pub(crate) centre: Vec3,
    pub(crate) radius: f32,
    pub(crate) floor: f32,
    /// The subject's box half sizes about `centre` — the depth a panel's back cut spans.
    pub(crate) half: Vec3,
}

impl Framing {
    /// The frame with nothing loaded.
    pub(crate) fn neutral() -> Self {
        Self {
            centre: Vec3::ZERO,
            radius: 100.0,
            floor: -100.0,
            half: Vec3::splat(100.0),
        }
    }

    fn of_base(b: &BasePreview) -> Self {
        Self {
            centre: b.centre,
            radius: b.radius,
            floor: b.centre.z + b.floor,
            half: Vec3::splat(b.radius),
        }
    }
}

/// The document's own subject framing (the parsed model), else neutral.
pub(crate) fn framing(doc: &Document) -> Framing {
    doc.parsed()
        .map(|p| Framing {
            centre: p.centre,
            radius: p.radius,
            floor: p.centre.z + p.floor,
            half: p.half_extent,
        })
        .unwrap_or_else(Framing::neutral)
}

/// What to draw, from the view toggles (skeleton / base body / collision / wireframe, and the
/// animated preview's source maps).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Show {
    pub(crate) skeleton: bool,
    pub(crate) base: bool,
    pub(crate) collision: bool,
    pub(crate) wireframe: bool,
    /// The Preview step's body under its PBR maps rather than the neutral steel.
    pub(crate) pbr: bool,
}

/// One panel's line batches plus the framing it looks at.
#[derive(Clone, Debug)]
pub(crate) struct Composed {
    pub(crate) lines: Arrows,
    pub(crate) overlay: Arrows,
    pub(crate) framing: Framing,
}

impl Composed {
    fn empty(framing: Framing) -> Self {
        Self {
            lines: Vec::new(),
            overlay: Vec::new(),
            framing,
        }
    }

    /// The batches without the ground grid — what an orthographic panel draws.
    pub(crate) fn without_ground(&self) -> Self {
        Self {
            lines: self
                .lines
                .iter()
                .filter(|(c, _)| *c != GROUND)
                .cloned()
                .collect(),
            overlay: self.overlay.clone(),
            framing: self.framing,
        }
    }
}

/// The GADGET's colours, every one a `theme.tokens` entry out of the loaded styles — the gadget
/// deliberately has no `Default`, so this is where the bench names each of them (rule 790872EE:
/// colours come from the ONE palette, never an rgba literal in scene code).
///
/// Idle keeps the axes readable at rest as the three signal colours (X red, Y green, Z blue — the
/// convention the mechanics geometry itself tags with); the Aim → Locked → Modify walk runs through
/// the sapphire family the whole UI uses for "the thing under your pointer" and out to the editor's
/// selection amber while deltas flow; a refused axis wears the danger tone, drawn dead.
pub(crate) const GADGET_TOKENS: [&str; 7] = [
    "sig_red",
    "sig_green",
    "sig_blue",
    "rune_glow",
    "sapphire",
    "stam_hi",
    "danger_base",
];

/// The shipped palette, for the gates and the tests that assert against real colours.
#[cfg(test)]
pub(crate) fn theme() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../../content/sensorium/resources/ui_theme.json"
    ))
    .expect("the shipped theme parses")
}

pub(crate) fn gadget_style(styles: &serde_json::Value) -> GadgetStyle {
    let c = GADGET_TOKENS.map(|name| token(styles, name));
    GadgetStyle {
        idle: [c[0], c[1], c[2]],
        aimed: c[3],
        locked: c[4],
        modifying: c[5],
        refused: c[6],
    }
}

/// One `theme.tokens` colour out of the loaded styles. A name that is not in the palette falls back
/// to full white — a colour nothing else in the bench draws, so a typo is loud on screen rather
/// than invisible; `every_gadget_colour_is_a_theme_token` is the gate that keeps it from ever shipping.
fn token(styles: &serde_json::Value, name: &str) -> [f32; 4] {
    let mut out = [1.0; 4];
    if let Some(a) = styles["theme"]["tokens"][name].as_array() {
        for (i, c) in a.iter().take(4).enumerate() {
            out[i] = c.as_f64().unwrap_or(1.0) as f32;
        }
    }
    out
}

/// The floor grid under `f`, centred on the subject.
fn ground(f: &Framing) -> ([f32; 4], Vec<(Vec3, Vec3)>) {
    let mut segs = grid_segments_xy(f.radius * 0.25, f.radius * 2.5, f.floor);
    for (a, b) in &mut segs {
        a.x += f.centre.x;
        a.y += f.centre.y;
        b.x += f.centre.x;
        b.y += f.centre.y;
    }
    (GROUND, segs)
}

/// The skeleton's overlay batches in world space: bone diamonds, joint balls, and the
/// selected joint's larger amber ball. (The gizmo's HANDLES are no longer composed here —
/// they are per-panel now, drawn straight from `Gadget::handle_lines`.)
///
/// Under a `mask` a bone the panel does not show is skipped, or — when `dim` — drawn in the dimmed
/// colours: the same glyphs, a third of the alpha. The selected joint always draws in full.
fn skeleton(
    out: &mut Arrows,
    parents: &[i32],
    globals: &[Mat4],
    radius: f32,
    sel: Option<usize>,
    mask: Option<&[bool]>,
    dim: bool,
) {
    if globals.is_empty() {
        return;
    }
    let shown = |i: usize| mask.is_none_or(|m| m.get(i).copied().unwrap_or(true));
    let min_r = (radius * BALL_MIN_FRAC).max(0.2);
    let max_r = (radius * BALL_MAX_FRAC).max(min_r);
    let radii = debug::joint_ball_radii(parents, globals, BALL_LEN_FRAC, min_r, max_r);
    out.push((
        BONE,
        debug::bone_diamonds_where(Mat4::IDENTITY, parents, globals, BONE_WAIST_FRAC, shown),
    ));
    if dim && mask.is_some() {
        out.push((
            dimmed(BONE),
            debug::bone_diamonds_where(Mat4::IDENTITY, parents, globals, BONE_WAIST_FRAC, |i| {
                !shown(i)
            }),
        ));
    }
    let (mut balls, mut dim_balls) = (Vec::new(), Vec::new());
    for (i, g) in globals.iter().enumerate() {
        if Some(i) == sel {
            continue; // the selected joint draws amber below
        }
        let ball = debug::wireframe(&Shape::Sphere {
            center: g.w_axis.truncate(),
            radius: radii[i],
        });
        if shown(i) {
            balls.extend(ball);
        } else if dim {
            dim_balls.extend(ball);
        }
    }
    out.push((JOINT, balls));
    if !dim_balls.is_empty() {
        out.push((dimmed(JOINT), dim_balls));
    }
    if let Some((s, g)) = sel.and_then(|s| globals.get(s).map(|g| (s, g))) {
        out.push((
            GIZMO_SEL,
            debug::wireframe(&Shape::Sphere {
                center: g.w_axis.truncate(),
                radius: radii.get(s).copied().unwrap_or(0.5) * 1.4,
            }),
        ));
    }
}

/// The attach-point markers: a cross at each resolved point, the selected one blue and larger.
fn markers(out: &mut Arrows, doc: &Document, radius: f32) {
    let att_sel = doc.attach_sel();
    let half = (radius * MARKER_FRAC).max(0.5);
    let (mut marks, mut sel_marks) = (Vec::new(), Vec::new());
    for i in 0..doc.attach_rows().len() {
        let Some(w) = doc.attach_world(i) else {
            continue;
        };
        let selected = Some(i) == att_sel;
        let h = if selected { half * 1.6 } else { half };
        let cross = [
            (w - Vec3::X * h, w + Vec3::X * h),
            (w - Vec3::Y * h, w + Vec3::Y * h),
            (w - Vec3::Z * h, w + Vec3::Z * h),
        ];
        if selected {
            sel_marks.extend(cross);
        } else {
            marks.extend(cross);
        }
    }
    if !marks.is_empty() {
        out.push((MARKER, marks));
    }
    if !sel_marks.is_empty() {
        out.push((MARKER_SEL, sel_marks));
    }
}

/// Whether the open source is a prop (mounted against the fitting body) rather than a
/// character or a clip.
fn is_prop(doc: &Document) -> bool {
    doc.class() == Some(AssetClass::Prop)
}

/// The four rig panels' line batches for `step`. A prop on the Mount step is framed on the
/// fitting body (`base`) with the body's skeleton drawn; everything else is framed on the
/// parsed subject with its skeleton and the markers on Attach and Review. (The gizmo's
/// handles are added per PANEL by the scene — they depend on the projection.)
pub(crate) fn rig_lines(
    doc: &Document,
    show: Show,
    step: Step,
    base: Option<&BasePreview>,
) -> Composed {
    if let (true, Some(b)) = (is_prop(doc), base) {
        let mut out = Composed::empty(Framing::of_base(b));
        out.lines.push(ground(&out.framing));
        if show.skeleton {
            skeleton(
                &mut out.overlay,
                &b.parents,
                &b.globals,
                b.radius,
                None,
                None,
                false,
            );
        }
        return out;
    }
    let mut out = Composed::empty(framing(doc));
    out.lines.push(ground(&out.framing));
    let Some(p) = doc.parsed() else {
        return out;
    };
    if show.collision {
        let mut segs = Vec::new();
        for v in &p.collision {
            if let Some(g) = p.globals.get(v.bone) {
                segs.extend(debug::wireframe(&v.world(*g)));
            }
        }
        if !segs.is_empty() {
            out.lines.push((COLLISION, segs));
        }
    }
    if show.skeleton {
        let sel = (step == Step::Rig).then(|| doc.bone_sel()).flatten();
        skeleton(
            &mut out.overlay,
            &p.parents,
            &p.globals,
            p.radius,
            sel,
            None,
            false,
        );
    }
    if matches!(step, Step::Attach | Step::Review) {
        markers(&mut out.overlay, doc, p.radius);
    }
    out
}

/// ONE panel's skeleton batches under an isolation — what replaces the shared skeleton in a
/// panel that isolates (the Rig step's overlay is the skeleton alone, so the swap is whole).
pub(crate) fn skeleton_overlay(doc: &Document, iso: &Isolation) -> Arrows {
    let mut out = Arrows::new();
    let Some(p) = doc.parsed() else {
        return out;
    };
    let mask = bone_mask(p, iso);
    skeleton(
        &mut out,
        &p.parents,
        &p.globals,
        p.radius,
        doc.bone_sel(),
        Some(&mask),
        iso.dim,
    );
    out
}

/// The CENTRE marks' colours, theme tokens like the gadget's (rule 790872EE): the origin cross
/// wears the three signal colours the gadget's idle axes wear, the median / floor lines the ink
/// the panel labels wear, the other panels' cut planes the modify accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CentreStyle {
    pub(crate) axes: [[f32; 4]; 3],
    pub(crate) plane: [f32; 4],
    pub(crate) cut: [f32; 4],
}

pub(crate) const CENTRE_TOKENS: [&str; 5] = [
    "sig_red",
    "sig_green",
    "sig_blue",
    "ink_sapphire",
    "stam_hi",
];

/// The DARK backdrop the Display group's toggle swaps every panel to — the theme's `stage_black`
/// (the stages themselves author `stage_grey`, the cool grey the base models' black underclothing
/// reads against).
pub(crate) fn dark_backdrop(styles: &serde_json::Value) -> [f64; 4] {
    token(styles, "stage_black").map(f64::from)
}

pub(crate) fn centre_style(styles: &serde_json::Value) -> CentreStyle {
    let c = CENTRE_TOKENS.map(|name| token(styles, name));
    CentreStyle {
        axes: [c[0], c[1], c[2]],
        plane: c[3],
        cut: c[4],
    }
}

/// `a → b` as dashes of `dash` length with equal gaps (a dash of nothing is the whole line).
fn dashed(a: Vec3, b: Vec3, dash: f32) -> Vec<(Vec3, Vec3)> {
    let len = (b - a).length();
    if dash <= 0.0 || len <= dash * 2.0 {
        return vec![(a, b)];
    }
    let dir = (b - a) / len;
    let mut out = Vec::new();
    let mut t = 0.0;
    while t < len {
        let end = (t + dash).min(len);
        out.push((a + dir * t, a + dir * end));
        t += dash * 2.0;
    }
    out
}

/// The world/model centre a panel draws (Aaron 2026-09-08: "more clear visual of the center of
/// the model/world"): the origin's tri-colour axis cross in every panel; in an orthographic
/// panel that sees it edge-on, the median plane X = 0 as a dashed line through the subject's
/// box, and the floor as a dashed line where the panel sees IT edge-on. The import plants the
/// mesh on the plumb line and the root at the origin, so the world centre IS the model centre.
pub(crate) fn centre_marks(f: &Framing, projection: Projection, style: &CentreStyle) -> Arrows {
    let mut out = Arrows::new();
    let arm = (f.radius * CENTRE_ARM_FRAC).max(1.0);
    for (axis, colour) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().zip(style.axes) {
        out.push((colour, vec![(-axis * arm, axis * arm)]));
    }
    let Some(depth) = projection.depth_axis() else {
        return out;
    };
    let dash = (f.radius * PLANE_DASH_FRAC).max(0.5);
    let (c, r) = (f.centre, f.radius);
    let mut plane = Vec::new();
    // The median plane is edge-on wherever the panel does not look along X.
    if depth.x.abs() < 0.5 {
        let along = Vec3::X.cross(depth).normalize_or_zero();
        let (a, b) = if along.z.abs() > 0.5 {
            (Vec3::new(0.0, c.y, f.floor), Vec3::new(0.0, c.y, c.z + r))
        } else {
            (Vec3::new(0.0, c.y - r, c.z), Vec3::new(0.0, c.y + r, c.z))
        };
        plane.extend(dashed(a, b, dash));
    }
    // The floor is edge-on wherever the panel does not look along Z.
    if depth.z.abs() < 0.5 {
        let along = Vec3::Z.cross(depth).normalize_or_zero();
        let mid = Vec3::new(c.x, c.y, f.floor);
        plane.extend(dashed(mid - along * r, mid + along * r, dash));
    }
    if !plane.is_empty() {
        out.push((style.plane, plane));
    }
    out
}

/// The OTHER panels' live cut planes as this orthographic panel sees them: a plane whose normal
/// lies across this panel's view is edge-on here — a dashed line through the subject's box where
/// it crosses — so a depth set in TOP is watched in FRONT. A plane facing this panel (its own, or
/// a parallel one) is invisible edge-on and skipped; the perspective panel draws none.
pub(crate) fn cut_marks(
    f: &Framing,
    projection: Projection,
    planes: &[(Vec3, f32)],
    style: &CentreStyle,
) -> Arrows {
    let Some(depth) = projection.depth_axis() else {
        return Arrows::new();
    };
    let dash = (f.radius * CUT_DASH_FRAC).max(0.5);
    let mut segs = Vec::new();
    for &(n, d) in planes {
        if n.dot(depth).abs() > 0.5 {
            continue;
        }
        let along = depth.cross(n).normalize_or_zero();
        let anchor = f.centre - n * (n.dot(f.centre) - d);
        segs.extend(dashed(
            anchor - along * f.radius,
            anchor + along * f.radius,
            dash,
        ));
    }
    if segs.is_empty() {
        Arrows::new()
    } else {
        vec![(style.cut, segs)]
    }
}

/// The four rig panels' draw items for `step`: a character's mesh (its skinned pose on
/// the Rig step, the source otherwise) and wireframe; a prop's piece at its fit on the
/// fitting body (the body itself when `show.base`); nothing for a clip.
pub(crate) fn rig_draws(
    doc: &Document,
    meshes: &mut ViewMeshes,
    r: &mut Renderer,
    step: Step,
    show: Show,
) -> Vec<Draw> {
    let mut draws = Vec::new();
    match doc.class() {
        Some(AssetClass::Skin | AssetClass::Creature) | None => {
            let mesh = if step == Step::Rig {
                meshes
                    .skinned_mesh(doc, r)
                    .or_else(|| meshes.source_mesh(doc, r))
            } else {
                meshes.source_mesh(doc, r)
            };
            if let Some(m) = mesh {
                draws.push(m.draw(Mat4::IDENTITY, SUBJECT_TINT));
            }
            if show.wireframe || step == Step::Prep {
                if let Some(w) = meshes.wire_mesh(doc, r) {
                    draws.push(Draw::Mesh {
                        mesh: w,
                        world: Mat4::IDENTITY,
                        options: MeshDrawOptions {
                            wireframe: true,
                            ..Default::default()
                        },
                    });
                }
            }
        }
        Some(AssetClass::Prop) => {
            let piece = meshes.source_mesh(doc, r);
            let world = match (meshes.base(), doc.fit()) {
                (Some(b), Some(fit)) => b.socket_world(fit),
                _ => Mat4::IDENTITY,
            };
            if show.base {
                if let Some(body) = meshes.base_upload() {
                    draws.push(body.draw(Mat4::IDENTITY, BODY_TINT));
                }
            }
            if let Some(m) = piece {
                draws.push(m.draw(world, PIECE_TINT));
            }
        }
        Some(AssetClass::Animation) => {}
    }
    draws
}

/// The preview step's bake view: the bake's skeleton at this frame's pose over its ground.
pub(crate) fn bake_lines(bp: &BakePreview, globals: &[Mat4], show_skeleton: bool) -> Composed {
    let mut out = Composed::empty(Framing {
        centre: bp.centre,
        radius: bp.radius,
        floor: bp.floor,
        half: Vec3::splat(bp.radius),
    });
    out.lines.push(ground(&out.framing));
    if show_skeleton {
        skeleton(
            &mut out.overlay,
            &bp.parents,
            globals,
            bp.radius,
            None,
            None,
            false,
        );
    }
    out
}

/// The clip step's two views at `tick`: root motion, then in place — each framed on its
/// own extent.
pub(crate) fn clip_lines(cp: &ClipPreview, tick: f32) -> [Composed; 2] {
    let tick = (tick as u32).min(cp.duration.saturating_sub(1));
    let panel = |clip, centre: Vec3, radius: f32| {
        let locals = sample_local_poses(&cp.bones, clip, tick, false);
        let globals = global_transforms(&cp.bones, &locals);
        let mut out = Composed::empty(Framing {
            centre,
            radius,
            floor: cp.floor,
            half: Vec3::splat(radius),
        });
        out.lines.push(ground(&out.framing));
        skeleton(
            &mut out.overlay,
            &cp.parents,
            &globals,
            cp.radius,
            None,
            None,
            false,
        );
        out
    };
    [
        panel(&cp.rm, cp.rm_center, cp.rm_radius),
        panel(&cp.ip, cp.ip_center, cp.radius),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EVERY GADGET COLOUR IS A PALETTE TOKEN (rule 790872EE). `GadgetStyle` has no `Default`
    /// precisely so the consumer must name each one; this is the gate that the seven names it
    /// gives are real `theme.tokens` entries and actually resolve — a typo would otherwise ship
    /// as a white handle nobody notices until the bench is open.
    #[test]
    fn every_gadget_colour_is_a_theme_token() {
        let theme = theme();
        for name in GADGET_TOKENS {
            assert!(
                theme["theme"]["tokens"][name].is_array(),
                "the gadget names `{name}`, which is not in theme.tokens"
            );
        }
        // And the resolver reaches them: nothing falls back to the loud white.
        let style = gadget_style(&theme);
        for c in style.idle.iter().chain([
            &style.aimed,
            &style.locked,
            &style.modifying,
            &style.refused,
        ]) {
            assert_ne!(
                *c, [1.0; 4],
                "a gadget colour fell back instead of resolving"
            );
        }
        // The three axes stay distinguishable at rest — that is the whole point of three arrows.
        assert_ne!(style.idle[0], style.idle[1]);
        assert_ne!(style.idle[1], style.idle[2]);
    }

    /// EVERY CENTRE COLOUR IS A PALETTE TOKEN, the gadget's gate again.
    #[test]
    fn every_centre_colour_is_a_theme_token() {
        let theme = theme();
        for name in CENTRE_TOKENS {
            assert!(
                theme["theme"]["tokens"][name].is_array(),
                "the centre marks name `{name}`, which is not in theme.tokens"
            );
        }
        let style = centre_style(&theme);
        for c in style.axes.iter().chain([&style.plane, &style.cut]) {
            assert_ne!(
                *c, [1.0; 4],
                "a centre colour fell back instead of resolving"
            );
        }
    }

    fn masked(doc: &Document, filter: BoneFilter) -> Vec<bool> {
        bone_mask(doc.parsed().unwrap(), &isolation(doc, filter, false))
    }

    fn named(doc: &Document, name: &str) -> usize {
        doc.parsed()
            .unwrap()
            .bone_index(name)
            .unwrap_or_else(|| panic!("{name} in the canon"))
    }

    /// The RIGHT view's LIMB filter keeps every right-suffixed bone — clavicle to fingers, thigh
    /// to toes — and nothing of the trunk, head or left side.
    #[test]
    fn the_right_view_isolates_the_right_arm_and_leg() {
        let mut doc = crate::tests::synthetic_rigged_doc("compose_limb");
        // The selection is always kept, so put it on the side under test.
        assert!(doc.select_bone(named(&doc, "hand_r")));
        let m = masked(&doc, BoneFilter::Limb(Side::Right));
        for on in [
            "clavicle_r",
            "upperarm_r",
            "hand_r",
            "index_02_r",
            "thigh_r",
            "foot_r",
        ] {
            assert!(m[named(&doc, on)], "{on} is the right limb");
        }
        for off in [
            "pelvis",
            "spine_01",
            "neck_01",
            "head",
            "clavicle_l",
            "hand_l",
            "thigh_l",
        ] {
            assert!(!m[named(&doc, off)], "{off} is not");
        }
        assert!(doc.select_bone(named(&doc, "hand_l")));
        let m = masked(&doc, BoneFilter::Limb(Side::Left));
        assert!(m[named(&doc, "hand_l")] && !m[named(&doc, "hand_r")]);
    }

    /// The selected joint and its reach chain survive any filter — nothing selected can vanish.
    #[test]
    fn the_selected_joint_survives_the_filter() {
        let mut doc = crate::tests::synthetic_rigged_doc("compose_keep");
        let hand_l = named(&doc, "hand_l");
        assert!(doc.select_bone(hand_l));
        let m = masked(&doc, BoneFilter::Limb(Side::Right));
        assert!(m[hand_l], "the selected left hand shows in the right view");
        for k in doc.reach_chain(hand_l).unwrap() {
            assert!(m[k], "its reach chain shows too");
        }
        assert!(
            !m[named(&doc, "thigh_l")],
            "the rest of the left side stays hidden"
        );
    }

    /// SOLO keeps the selected joint's subtree only: the hand and its fingers, not the forearm.
    #[test]
    fn a_solo_view_keeps_the_subtree_only() {
        let mut doc = crate::tests::synthetic_rigged_doc("compose_solo");
        let hand = named(&doc, "hand_r");
        assert!(doc.select_bone(hand));
        let m = masked(&doc, BoneFilter::Subtree(hand));
        assert!(m[hand] && m[named(&doc, "index_01_r")] && m[named(&doc, "index_02_r")]);
        assert!(!m[named(&doc, "hand_l")] && !m[named(&doc, "pelvis")]);
        // The reach chain (forearm, upper arm) is kept, everything else above is not.
        for k in doc.reach_chain(hand).unwrap() {
            assert!(m[k]);
        }
        assert!(!m[named(&doc, "clavicle_r")]);
    }

    /// An orthographic panel HIDES what its filter drops; the perspective picker DIMS it: the same
    /// glyphs at a third of the alpha, and the full skeleton's segment count is preserved.
    #[test]
    fn a_dimmed_panel_draws_the_rest_at_a_third_of_the_alpha() {
        let mut doc = crate::tests::synthetic_rigged_doc("compose_dim");
        assert!(doc.select_bone(named(&doc, "hand_r")));
        let count = |a: &Arrows, c: [f32; 4]| {
            a.iter()
                .filter(|(k, _)| *k == c)
                .map(|(_, s)| s.len())
                .sum::<usize>()
        };
        let full = rig_lines(
            &doc,
            Show {
                skeleton: true,
                ..Default::default()
            },
            Step::Rig,
            None,
        )
        .overlay;
        let hidden = skeleton_overlay(&doc, &isolation(&doc, BoneFilter::Limb(Side::Right), false));
        let dim = skeleton_overlay(&doc, &isolation(&doc, BoneFilter::Limb(Side::Right), true));
        assert!(
            count(&hidden, JOINT) < count(&full, JOINT),
            "hiding drops joint balls"
        );
        assert!(count(&hidden, BONE) < count(&full, BONE), "and bone glyphs");
        assert_eq!(
            count(&hidden, dimmed(JOINT)),
            0,
            "a hiding panel draws no dim batch"
        );
        assert_eq!(
            count(&dim, JOINT) + count(&dim, dimmed(JOINT)),
            count(&full, JOINT),
            "dimming keeps every ball, bright or dim"
        );
        assert_eq!(
            count(&dim, BONE) + count(&dim, dimmed(BONE)),
            count(&full, BONE)
        );
        assert!(count(&dim, dimmed(JOINT)) > 0);
        assert_eq!(
            count(&dim, GIZMO_SEL),
            count(&full, GIZMO_SEL),
            "the selection stays amber"
        );
        assert!((dimmed(JOINT)[3] - JOINT[3] * DIM_ALPHA).abs() < 1e-6);
    }

    /// Every panel draws the origin's three-axis cross; FRONT and TOP see the median plane
    /// X = 0 edge-on as a line at x = 0; FRONT and LEFT see the floor as a line at the floor;
    /// the perspective panel draws the cross alone (it has the grid).
    #[test]
    fn every_ortho_panel_draws_the_origin_cross_and_its_median_line() {
        // Framed a little off the plumb line, so a line through the CENTRE and a line on the
        // MEDIAN PLANE cannot be confused.
        let f = Framing {
            centre: Vec3::new(7.0, 3.0, 90.0),
            radius: 90.0,
            floor: 0.0,
            half: Vec3::new(30.0, 15.0, 90.0),
        };
        let style = centre_style(&theme());
        for p in Projection::ALL {
            let marks = centre_marks(&f, p, &style);
            for (axis, colour) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().zip(style.axes) {
                let arm = marks
                    .iter()
                    .find(|(c, _)| *c == colour)
                    .map(|(_, s)| s[0])
                    .expect("an axis arm");
                assert!(
                    (arm.0 + arm.1).length() < 1e-4,
                    "{p:?}: the arm is centred on the origin"
                );
                assert!(
                    arm.1.normalize().dot(axis).abs() > 0.999,
                    "{p:?}: along {axis}"
                );
            }
            let plane: Vec<_> = marks
                .iter()
                .filter(|(c, _)| *c == style.plane)
                .flat_map(|(_, s)| s.iter().copied())
                .collect();
            let at_x0 = plane
                .iter()
                .filter(|(a, b)| a.x.abs() < 1e-3 && b.x.abs() < 1e-3)
                .count();
            let on_floor = plane
                .iter()
                .filter(|(a, b)| a.z.abs() < 1e-3 && b.z.abs() < 1e-3)
                .count();
            match p {
                Projection::Perspective => assert!(plane.is_empty(), "the picker keeps its grid"),
                Projection::Front => assert!(at_x0 > 0 && on_floor > 0, "FRONT: median + floor"),
                Projection::Top => assert!(at_x0 > 0 && on_floor == 0, "TOP: median only"),
                Projection::Left => assert!(at_x0 == 0 && on_floor > 0, "LEFT: floor only"),
            }
        }
    }

    /// A FRONT panel's cut plane (normal +Y at y = −20) is a line at y = −20 in TOP and LEFT, and
    /// nothing in FRONT itself or the perspective panel.
    #[test]
    fn the_cut_plane_is_a_line_in_the_other_orthos() {
        let f = Framing {
            centre: Vec3::new(0.0, 0.0, 90.0),
            radius: 90.0,
            floor: 0.0,
            half: Vec3::new(30.0, 15.0, 90.0),
        };
        let style = centre_style(&theme());
        let planes = [(Vec3::Y, -20.0)];
        for p in Projection::ALL {
            let marks = cut_marks(&f, p, &planes, &style);
            let segs: Vec<_> = marks.iter().flat_map(|(_, s)| s.iter().copied()).collect();
            match p {
                Projection::Top | Projection::Left => {
                    assert!(!segs.is_empty(), "{p:?} sees the plane edge-on");
                    assert!(
                        segs.iter()
                            .all(|(a, b)| (a.y + 20.0).abs() < 1e-3 && (b.y + 20.0).abs() < 1e-3),
                        "{p:?}: the line lies on y = −20"
                    );
                    let span = segs
                        .iter()
                        .map(|(a, b)| a.max(*b))
                        .fold(Vec3::splat(f32::MIN), Vec3::max)
                        - segs
                            .iter()
                            .map(|(a, b)| a.min(*b))
                            .fold(Vec3::splat(f32::MAX), Vec3::min);
                    assert!(
                        span.max_element() > 150.0,
                        "{p:?}: it crosses the subject's box"
                    );
                    assert!(marks.iter().all(|(c, _)| *c == style.cut));
                }
                Projection::Front | Projection::Perspective => {
                    assert!(segs.is_empty(), "{p:?} cannot see it edge-on")
                }
            }
        }
    }
}
