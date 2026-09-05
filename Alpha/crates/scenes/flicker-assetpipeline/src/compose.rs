//! **What the rig panels draw** — composed from the document each frame.
//!
//! Line batches are pure data → data: the ground grid and the collision volumes
//! (depth-tested), the skeleton, the selected joint's ball and the attach-point markers
//! (overlay, drawn over the body). The gizmo's HANDLES are not here: they depend on the
//! panel's projection, so the scene draws them per panel from the gadget. Draw items come from the
//! bench's mesh caches ([`ViewMeshes`]) as handles the bench owns. The
//! [`flicker_rigview::RigView`] panels draw exactly these, and the behaviour decides
//! what a click on them means (`gizmo`).

use flicker::render::{grid_segments_xy, Mat4, MeshDrawOptions, Renderer, Vec3};
use flicker_content::AssetClass;
use flicker_globe::Arrows;
use flicker_mechanics::{debug, Shape};
use flicker_rigview::{Draw, GadgetStyle};
use flicker_skeletal::pose::{global_transforms, sample_local_poses};

use crate::meshes::{BakePreview, BasePreview, ViewMeshes};
use crate::services::{ClipPreview, Document};
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

/// The subject's framing: centre, bounding radius and the feet plane (absolute z).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Framing {
    pub(crate) centre: Vec3,
    pub(crate) radius: f32,
    pub(crate) floor: f32,
}

impl Framing {
    /// The frame with nothing loaded.
    pub(crate) fn neutral() -> Self {
        Self {
            centre: Vec3::ZERO,
            radius: 100.0,
            floor: -100.0,
        }
    }

    fn of_base(b: &BasePreview) -> Self {
        Self {
            centre: b.centre,
            radius: b.radius,
            floor: b.centre.z + b.floor,
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
        })
        .unwrap_or_else(Framing::neutral)
}

/// What to draw, from the view toggles (skeleton / base body / collision / wireframe).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Show {
    pub(crate) skeleton: bool,
    pub(crate) base: bool,
    pub(crate) collision: bool,
    pub(crate) wireframe: bool,
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
fn skeleton(out: &mut Arrows, parents: &[i32], globals: &[Mat4], radius: f32, sel: Option<usize>) {
    if globals.is_empty() {
        return;
    }
    let min_r = (radius * BALL_MIN_FRAC).max(0.2);
    let max_r = (radius * BALL_MAX_FRAC).max(min_r);
    let radii = debug::joint_ball_radii(parents, globals, BALL_LEN_FRAC, min_r, max_r);
    out.push((
        BONE,
        debug::bone_diamonds(Mat4::IDENTITY, parents, globals, BONE_WAIST_FRAC),
    ));
    let mut balls = Vec::new();
    for (i, g) in globals.iter().enumerate() {
        if Some(i) == sel {
            continue; // the selected joint draws amber below
        }
        balls.extend(debug::wireframe(&Shape::Sphere {
            center: g.w_axis.truncate(),
            radius: radii[i],
        }));
    }
    out.push((JOINT, balls));
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
            skeleton(&mut out.overlay, &b.parents, &b.globals, b.radius, None);
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
        skeleton(&mut out.overlay, &p.parents, &p.globals, p.radius, sel);
    }
    if matches!(step, Step::Attach | Step::Review) {
        markers(&mut out.overlay, doc, p.radius);
    }
    out
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
        Some(AssetClass::Skin) | None => {
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
    });
    out.lines.push(ground(&out.framing));
    if show_skeleton {
        skeleton(&mut out.overlay, &bp.parents, globals, bp.radius, None);
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
        });
        out.lines.push(ground(&out.framing));
        skeleton(&mut out.overlay, &cp.parents, &globals, cp.radius, None);
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
}
