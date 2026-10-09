//! The GOLEM STAGE — the reference body standing in the tester's centre panel,
//! playing locomotion off the SAME dispatched signals the right-hand panels print.
//!
//! This is the bus inspector's proof-of-life: a fired `ActionSignal` is not just a
//! row in the CONSUMED panel, it is MOTION. Signals the gameplay-base layer consumes
//! (World context only — push Menu/Radial/TextEntry and the golem honestly stops
//! responding, because the bus really is routing elsewhere) fold into the skeletal
//! runtime's [`state::Inputs`] locomotion contract; the analog left stick rides in
//! beside them from the latch. The golem's pack is the recovered KATANAMI state machine
//! (2026-09-07): a real transition graph over the Katanami clip library — move / run /
//! crouch / jump / attack / hit / die — so `StateMachine::advance` consumes the `Inputs`
//! and drives itself. A states-only pack (the seeded golem draft) has no edges; for that
//! case a small pure mapper still picks the state the inputs mean and forces it.
//!
//! Drawing reuses the proven shared seams end to end: `sample_local_poses` (+
//! `blend_local_poses` across the machine's blend window) → `global_transforms` →
//! `skin::palette` → `skin::skin` → one upload per frame (freed next frame), exactly the
//! paperdoll's path for one live character. The body draws in its OWN SKIN when the rig
//! ships its maps (Aaron 2026-09-07: "I'd like to see the golem skinned as well") — the
//! textured PBR path the Clayworks preview uses, tangents built per frame from the
//! skinned corners — and falls back to flat clay when it has none.
//!
//! A CREATURE — a body on the Quadruped, Bird or Bat pattern (Aaron 2026-09-08: the tester
//! lists `package/creatures/` and `staging/creatures/` too) — has no clip library: its
//! pattern's default pack names GENERATED gaits ([`state::ClipSource`], G5), and while the
//! machine sits in one the stage steps the gait generator (`flicker-mechanics::gait`) over the
//! rig's rest frames on a flat floor, along the controller's heading at the speed the gait
//! means, then skins those frames through the same seams. A gait nothing generates yet is
//! said in the caption and the log, never hidden.

use std::path::{Path, PathBuf};
use std::time::Duration;

use flicker::render::{
    build_textured_verts, grid_segments, Camera, LightRig, Mat4, MeshDrawOptions, MeshHandle,
    MeshIndices, MeshVertex, PbrMaps, Rect, Renderer, TextureHandle, TexturedMeshHandle, Vec2,
    Vec3,
};
use flicker::ui::strings;
use flicker_input_core::{ActionSignal, AnalogFrame, EventKind};
use flicker_input_router::{DispatchReport, InputEvent};
use flicker_mechanics::gait::{kind_of, speed_for, FlatFloor, Locomotion, LocomotionFamily};
use flicker_skeletal::format::{self, Bone, Model, Pattern};
use flicker_skeletal::state::{self, Gait, GeneratedGait, StateMachine};
use flicker_skeletal::{pose, skin};
use serde::Deserialize;

/// The reference body — the ultra golem (GolemBaseV2, 2026-09-07: the raw Meshy mesh sized
/// to 170 cm, collapsed, and rigged on the authored canon), addressed the same way the other
/// scenes address package content.
fn golem_dir() -> PathBuf {
    flicker_core::roots::roots()
        .package()
        .join("characters/GolemBaseV2")
}
/// A PATTERN's default controller pack — `package/skeletons/<Pattern>/<Pattern>.pack.json`
/// (Aaron 2026-09-08: the pack is per controller, not per body; the humanoid's is the recovered
/// Katanami graph, re-pointed at the baked Katanami library). A body that ships its own pack
/// beside itself overrides it; a pattern without a pack yet falls back to the humanoid's so the
/// stage always has a graph.
fn pattern_pack(pattern: Pattern) -> PathBuf {
    let name = pattern.name();
    flicker_core::roots::roots()
        .package()
        .join("skeletons")
        .join(name)
        .join(format!("{name}.pack.json"))
}

/// Every BODY the stage can show, tier by tier, each tier sorted: the promoted characters
/// (`package/characters/<Name>/<Name>.json`, Aaron 2026-09-07), then the CREATURES — promoted
/// (`package/creatures/`) and staged (`staging/creatures/`; Aaron 2026-09-08: the tester lists
/// creatures too and drives their generated gaits). A creature folder that is an old STATIC
/// bake — a rig with no bones, the staged Horse — has nothing to drive and is skipped, not
/// crashed on. (The skeleton-only pattern references live in `package/skeletons/`, not here.)
fn bodies() -> Vec<PathBuf> {
    let roots = flicker_core::roots::roots();
    let tiers = [
        (roots.package().join("characters"), false),
        (roots.package().join("creatures"), true),
        (roots.staging().join("creatures"), true),
    ];
    let mut out = Vec::new();
    for (root, creatures) in tiers {
        let mut tier: Vec<PathBuf> = std::fs::read_dir(&root)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_dir())
                    .filter(|p| {
                        let rig = own_rig(p);
                        (rig.exists() || rig.with_extension("json.gz").exists())
                            && (!creatures || rig_has_bones(&rig))
                    })
                    .collect()
            })
            .unwrap_or_default();
        tier.sort();
        out.extend(tier);
    }
    out
}

/// A body folder's own rig by its LOGICAL path — `<Name>/<Name>.json`; the shared reader is
/// gz-transparent, so the packaged `.json.gz` answers to it too.
fn own_rig(body: &Path) -> PathBuf {
    let name = body
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    body.join(format!("{name}.json"))
}

/// Whether a rig on disk carries a skeleton — a head parse of the bone list alone, so listing
/// a tier never decodes a mesh. A file that is not a rig has no bones either.
fn rig_has_bones(rig: &Path) -> bool {
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        skeleton: Skeleton,
    }
    #[derive(Deserialize, Default)]
    struct Skeleton {
        #[serde(default)]
        bones: Vec<serde::de::IgnoredAny>,
    }
    flicker_core::compression::read_text(rig)
        .ok()
        .and_then(|text| serde_json::from_str::<Head>(&text).ok())
        .is_some_and(|head| !head.skeleton.bones.is_empty())
}

/// A body's own pack when it ships one, else its PATTERN's default pack, else the humanoid's.
fn pack_for(body: &Path) -> PathBuf {
    let name = body
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let own = body.join(format!("{name}.pack.json"));
    if own.exists() || body.join(format!("{name}.pack.json.gz")).exists() {
        return own;
    }
    let pattern = flicker_skeletal::format::rig_pattern(&body.join(format!("{name}.json")));
    let by_pattern = pattern_pack(pattern);
    if by_pattern.exists() || by_pattern.with_extension("json.gz").exists() {
        by_pattern
    } else {
        pattern_pack(Pattern::Humanoid)
    }
}
/// The clip libraries the golem's states resolve against, by canonical bone name: the
/// Katanami set (what the pack references) and the Motifect locomotion set beside it. A body
/// whose recipe names a preset gets each library's `@<Preset>` bake instead — the loader's
/// swap, so a digitigrade golem plays the Katanami set re-baked onto its own rest.
fn clip_dirs() -> [PathBuf; 2] {
    let clips = flicker_core::roots::roots()
        .package()
        .join("retarget/clips");
    [clips.join("katanami"), clips.join("locomotion")]
}

/// The demo handler chain's gameplay-base slot (system 0 ▸ scene-root 1 ▸ modal 2 ▸
/// gameplay 3) — the layer whose consumed signals become motion.
const GAMEPLAY_BASE: usize = 3;

/// The flat clay tint — the fallback for a body that ships no base-colour map (the same
/// recessive reference-body reading the Clayworks viewport gives an untextured golem).
const CLAY: [f32; 4] = [0.62, 0.66, 0.74, 1.0];

/// The body's decoded maps, loaded once from the rig's material on the first drawn frame.
#[derive(Clone, Copy)]
struct Skin {
    albedo: TextureHandle,
    maps: PbrMaps,
}

/// Decode the rig's material maps beside the body (`BaseColor` sRGB; `Normal` /
/// `Roughness` / `Metallic` linear). No base colour, or one that fails to decode, means no
/// skin — the stage draws clay and says why in the log.
fn load_skin(r: &mut Renderer, model: &Model, dir: &Path) -> Option<Skin> {
    let material = model.mesh.materials.first()?;
    let mut load = |name: &str, srgb: bool| -> Option<TextureHandle> {
        if name.is_empty() {
            return None;
        }
        let path = dir.join(name);
        match image::open(&path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                Some(if srgb {
                    r.load_texture(rgba.as_raw(), w, h)
                } else {
                    r.load_texture_linear(rgba.as_raw(), w, h)
                })
            }
            Err(e) => {
                tracing::warn!(map = %path.display(), "golem stage: map failed ({e})");
                None
            }
        }
    };
    let albedo = load(&material.base_color, true)?;
    let maps = PbrMaps {
        normal: load(&material.normal, false),
        roughness: load(&material.roughness, false),
        metalness: load(&material.metalness, false),
        ..Default::default()
    };
    Some(Skin { albedo, maps })
}
/// The stage floor lattice, matching the editor viewports' faint ground.
const GROUND: [f32; 4] = [0.55, 0.63, 0.75, 0.16];

/// Analog thresholds: below `DEAD` the stick is noise; above `RUN` full tilt means run.
const DEAD: f32 = 0.25;
const RUN_TILT: f32 = 0.65;

/// Movement signals held down right now, folded from the bus's press/release edges.
#[derive(Default, Clone, Copy)]
struct Held {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    sprint: bool,
}

pub struct GolemStage {
    /// The promoted bodies the stage can show and which one is up — the picker.
    bodies: Vec<PathBuf>,
    body: usize,
    model: Option<Model>,
    machine: Option<StateMachine>,
    /// The gait generator for a body whose pack names GENERATED gaits (a creature pattern);
    /// `None` for a body whose states all sample clips, or whose rig has no limb to plant.
    gait: Option<GaitDrive>,
    /// Last frame's uploaded body, freed before each new upload (one live handle per path).
    mesh: Option<MeshHandle>,
    textured: Option<TexturedMeshHandle>,
    /// The body's maps, decoded on the first drawn frame; `skin_tried` stops a body that
    /// has none from being probed every frame.
    skin: Option<Skin>,
    skin_tried: bool,
    held: Held,
    /// Crouch is a TOGGLE (the ruled LS-click semantics), flipped on the press edge.
    crouched: bool,
    /// Jump press edge, consumed by the next frame's `Inputs`.
    jump: bool,
    /// Attack press edge (light attack), consumed by the next frame's `Inputs`.
    attack: bool,
    /// Rest-pose framing (world space): centre, half-extent, ground height.
    center: Vec3,
    radius: f32,
    floor: f32,
    /// Load failure, shown on the stage instead of an empty hole — fail loud.
    pub error: Option<String>,
}

impl GolemStage {
    pub fn new() -> Self {
        let bodies = bodies();
        let body = bodies.iter().position(|b| *b == golem_dir()).unwrap_or(0);
        Self {
            bodies,
            body,
            model: None,
            machine: None,
            gait: None,
            mesh: None,
            textured: None,
            skin: None,
            skin_tried: false,
            held: Held::default(),
            crouched: false,
            jump: false,
            attack: false,
            center: Vec3::ZERO,
            radius: 100.0,
            floor: 0.0,
            error: None,
        }
    }

    /// Load the reference body + the clip libraries + its pack. IO only — uploads are
    /// per-frame. A missing content tree records an error the stage displays.
    pub fn load(&mut self) {
        let body = self.body_dir();
        let load = || -> Result<(Model, StateMachine, Option<GaitDrive>), String> {
            let pack = pack_for(&body);
            let def = state::load_pack(&pack).map_err(|e| e.to_string())?;
            // The clip libraries load only when the pack names a clip: a creature pattern's
            // pack is generated gaits end to end, so its body loads alone — nothing to
            // resolve, no `@<Pattern>` bake to look for.
            let names_a_clip = def.states.iter().any(|s| s.clip.library().is_some());
            let names_a_gait = def.states.iter().any(|s| s.clip.generated().is_some());
            let [katanami, locomotion] = clip_dirs();
            let dirs: Vec<&Path> = if names_a_clip {
                vec![&body, &katanami, &locomotion]
            } else {
                vec![&body]
            };
            let model = format::load_dirs(&dirs).map_err(|e| e.to_string())?;
            let refs: Vec<state::ClipRef> = model
                .clips
                .iter()
                .map(|c| state::ClipRef {
                    name: &c.name,
                    duration_ticks: c.duration_ticks,
                })
                .collect();
            let machine = StateMachine::build(&def, &refs).map_err(|e| e.to_string())?;
            let gait = names_a_gait.then(|| GaitDrive::new(&model.bones)).flatten();
            if names_a_gait && gait.is_none() {
                // The contract's loud failure: a pack that asks for generated gaits over a
                // rig with nothing to plant — the body will only ever hold its rest.
                tracing::warn!(
                    pack = %pack.display(),
                    "golem stage: the pack names generated gaits but the rig has no leg or \
                     foreleg chain to plant — generated states hold the rest pose"
                );
            }
            Ok((model, machine, gait))
        };
        match load() {
            Ok((model, machine, gait)) => {
                // Rest framing: pose the bind, skin it, and measure the world-space
                // result — the same numbers the camera and ground will draw against.
                let locals: Vec<Mat4> = model.bones.iter().map(|b| b.local).collect();
                let globals = pose::global_transforms(&model.bones, &locals);
                let palette = skin::palette(&model.bones, &globals);
                let rest = skin::skin(&model.mesh, &palette);
                let mut min = Vec3::splat(f32::MAX);
                let mut max = Vec3::splat(f32::MIN);
                for v in &rest {
                    let p = model.world.transform_point3(Vec3::from(v.position));
                    min = min.min(p);
                    max = max.max(p);
                }
                if rest.is_empty() {
                    min = Vec3::ZERO;
                    max = Vec3::ONE;
                }
                // ENGINE SPACE IS Y-UP: `model.world` reorients the Z-up rig, so the
                // measured bounds here are already engine-oriented — the ground is
                // the Y floor. (The first cut read Z as up and laid the golem on its
                // back; the orientation guard test below pins Y-tall now.)
                self.center = (min + max) * 0.5;
                self.radius = ((max - min).length() * 0.5).max(1.0);
                self.floor = min.y;
                tracing::info!(
                    bones = model.bones.len(),
                    clips = model.clips.len(),
                    "golem stage: reference body loaded"
                );
                self.model = Some(model);
                self.machine = Some(machine);
                self.gait = gait;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Fold this frame's BUS OUTCOME into locomotion inputs and advance the machine.
    /// Only signals the gameplay-base layer actually CONSUMED move the body — under a
    /// modal context the golem stands down, which is the routing made visible.
    pub fn drive(
        &mut self,
        dt: Duration,
        events: &[InputEvent],
        report: &DispatchReport,
        latch: Option<&AnalogFrame>,
    ) {
        for ev in events {
            if !report.consumed_by(GAMEPLAY_BASE, ev.signal) {
                continue;
            }
            let down = match ev.kind {
                EventKind::Press | EventKind::Hold | EventKind::Chord => true,
                EventKind::Release => false,
            };
            match ev.signal {
                ActionSignal::MoveForward => self.held.forward = down,
                ActionSignal::MoveBackward => self.held.back = down,
                ActionSignal::StrafeLeft => self.held.left = down,
                ActionSignal::StrafeRight => self.held.right = down,
                ActionSignal::Sprint => self.held.sprint = down,
                ActionSignal::Crouch if ev.kind == EventKind::Press => {
                    self.crouched = !self.crouched;
                }
                ActionSignal::Jump if ev.kind == EventKind::Press => self.jump = true,
                ActionSignal::AttackLight if ev.kind == EventKind::Press => self.attack = true,
                _ => {}
            }
        }

        // The analog channel rides in beside the digital edges (spec: Move* analog
        // lives on the latch). Stick direction augments; full tilt means run.
        let stick = latch.map(|f| f.left_stick).unwrap_or(Vec2::ZERO);
        let mut inputs = fold_inputs(self.held, self.crouched, self.jump, stick);
        inputs.attack = self.attack;
        self.jump = false;
        self.attack = false;

        if let Some(m) = self.machine.as_mut() {
            // A real graph (the Katanami pack) drives itself from the inputs. A states-only
            // pack has no edges, so the scene picks the state those inputs mean; a mid-flight
            // Jump is left to finish (`next` → Idle).
            if !m.has_graph() {
                let desired = desired_state(&inputs);
                if m.current_state_name() != "Jump"
                    && m.current_state_name() != desired
                    && !m.force_state_by_name(desired)
                {
                    tracing::warn!("golem stage: pack has no state `{desired}`");
                }
            }
            m.advance(dt.as_secs_f32(), &inputs);
            // A generated state has no clip: step the gait generator for it under this
            // frame's heading and the speed its gait means.
            if let (Some(wanted), Some(gait)) = (m.current_generated(), self.gait.as_mut()) {
                gait.step(wanted, &inputs, dt.as_secs_f32());
            }
        }
    }

    /// The picked body's folder (the golem when nothing is promoted).
    fn body_dir(&self) -> PathBuf {
        self.bodies
            .get(self.body)
            .cloned()
            .unwrap_or_else(golem_dir)
    }

    /// The picked body's name, for the caption.
    pub fn body_name(&self) -> String {
        self.body_dir()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string()
    }

    /// The promoted bodies the picker steps through.
    #[cfg(test)]
    pub fn bodies(&self) -> &[PathBuf] {
        &self.bodies
    }

    /// Step the body pick by `delta` (clamped at the ends — a linear rail never wraps) and
    /// load it: its own pack or the golem's, its libraries by recipe. Returns whether the
    /// pick moved.
    pub fn step_body(&mut self, delta: i32) -> bool {
        let n = self.bodies.len() as i32;
        if n == 0 {
            return false;
        }
        let want = (self.body as i32 + delta).clamp(0, n - 1) as usize;
        if want == self.body {
            return false;
        }
        self.body = want;
        self.skin = None;
        self.skin_tried = false;
        self.load();
        true
    }

    /// The machine's live caption for the stage footer: `BODY · STATE · clip · tick/len`; for
    /// a generated state `BODY · STATE · generated <gait> · <driven gait> <speed> cm/s`, or —
    /// for a gait nothing generates yet — that it has no generator (said, not hidden).
    pub fn caption(&self) -> Option<String> {
        let m = self.machine.as_ref()?;
        let model = self.model.as_ref()?;
        let head = format!("{}  \u{00b7}  {}", self.body_name(), m.current_state_name());
        if let Some(wanted) = m.current_generated() {
            let generated = strings::resolve("$ctt_generated");
            return Some(match &self.gait {
                Some(gait) if gait.undriven.is_none() => format!(
                    "{head}  \u{00b7}  {generated} {}  \u{00b7}  {:?} {:.0} cm/s",
                    wanted.gait.name(),
                    gait.loco.gait,
                    gait.speed
                ),
                _ => format!(
                    "{head}  \u{00b7}  {generated} {}  \u{00b7}  {}",
                    wanted.gait.name(),
                    strings::resolve("$ctt_no_generator")
                ),
            });
        }
        let clip = model
            .clips
            .get(m.current_clip())
            .map(|c| c.name.as_str())
            .unwrap_or("?");
        Some(format!(
            "{head}  \u{00b7}  {clip}  \u{00b7}  {}/{}",
            m.current_tick(),
            m.current_duration()
        ))
    }

    /// Draw the 3D stage (camera + ground + the skinned body). Called FIRST in the
    /// scene's render — the UI chrome then frames the centre `view` rect around it.
    pub fn render(&mut self, r: &mut Renderer, view: Rect) {
        // One live body upload: free last frame's before this frame's replaces it.
        if let Some(h) = self.mesh.take() {
            r.free_mesh(h);
        }
        if let Some(h) = self.textured.take() {
            r.free_textured_mesh(h);
        }
        let (Some(model), Some(machine)) = (self.model.as_ref(), self.machine.as_ref()) else {
            return;
        };
        // A generated state is posed by the driver, not a clip — and the driven body
        // TRAVELS, so the camera and the ground follow it.
        let driven = machine.current_generated().and(self.gait.as_ref());
        let travel = driven
            .map(|g| model.world.transform_vector3(g.loco.position))
            .unwrap_or(Vec3::ZERO);

        // Aim the camera so the body sits in the CENTRE PANEL, not the window centre:
        // shift the look target opposite the panel's horizontal offset from the
        // window's middle (a small, exact correction — the camera is full-frame).
        let size = r.size();
        let panel_cx = view.pos.x + view.size.x * 0.5;
        let frac = ((size.x * 0.5 - panel_cx) / size.x).clamp(-0.25, 0.25);
        let target = self.center + travel + Vec3::new(frac * self.radius * 2.6, 0.0, 0.0);
        let dist = self.radius * 2.6;
        let (yaw, pitch) = (0.55_f32, 0.32_f32); // a gentle 3/4 view, slightly above
                                                 // Y-up engine space: orbit in the XZ plane, lift along +Y.
        let eye = target
            + Vec3::new(
                yaw.sin() * pitch.cos() * dist,
                pitch.sin() * dist,
                yaw.cos() * pitch.cos() * dist,
            );
        r.set_camera(&Camera {
            position: eye,
            target,
            up: Vec3::Y,
            fov_y_radians: 50.0_f32.to_radians(),
            near: 1.0,
            far: dist * 10.0,
            ortho_height: None,
        });
        r.set_scene(LightRig::default());

        // Ground lattice at the body's feet (the Y-up stage helper), carried along under a
        // travelling body by whole cells so it reads as one fixed floor walked over.
        let spacing = self.radius * 0.25;
        let carry = Vec3::new(
            (travel.x / spacing).round() * spacing,
            0.0,
            (travel.z / spacing).round() * spacing,
        );
        let ground: Vec<(Vec3, Vec3)> = grid_segments(spacing, self.radius * 2.5, self.floor)
            .into_iter()
            .map(|(a, b)| (a + carry, b + carry))
            .collect();
        r.draw_lines(&ground, GROUND);

        // Pose → blend → globals → palette → skin → one upload (textured, or flat clay). A
        // generated state takes the driver's world frames straight: no clip, no blend.
        let globals = match driven {
            Some(gait) => gait.globals.clone(),
            None => {
                let sample = |ci: usize, tick: u32| {
                    model
                        .clips
                        .get(ci)
                        .map(|c| pose::sample_local_poses(&model.bones, c, tick, model.retarget))
                        .unwrap_or_else(|| model.bones.iter().map(|b| b.local).collect())
                };
                let incoming = sample(machine.current_clip(), machine.current_tick());
                let locals = match machine.blend() {
                    Some(b) => {
                        let outgoing = sample(b.from_clip, b.from_tick);
                        pose::blend_local_poses(&outgoing, &incoming, b.weight)
                    }
                    None => incoming,
                };
                pose::global_transforms(&model.bones, &locals)
            }
        };
        let palette = skin::palette(&model.bones, &globals);
        let skinned = skin::skin(&model.mesh, &palette);
        if skinned.is_empty() {
            return;
        }
        if !self.skin_tried {
            self.skin_tried = true;
            let dir = self
                .bodies
                .get(self.body)
                .cloned()
                .unwrap_or_else(golem_dir);
            self.skin = load_skin(r, model, &dir);
        }
        match self.skin {
            Some(skin) => {
                // The rig mesh is one vertex per corner with sequential indices — the shape
                // the tangent builder wants.
                let bind = &model.mesh.vertices;
                let verts = build_textured_verts(
                    0..skinned.len(),
                    |i| skinned[i].position,
                    |i| skinned[i].normal,
                    |i| bind[i].uv,
                );
                let handle = r.upload_textured_mesh(&verts, MeshIndices::U32(&model.mesh.indices));
                r.draw_textured_mesh_pbr(
                    handle,
                    skin.albedo,
                    skin.maps,
                    model.world,
                    MeshDrawOptions::default(),
                );
                self.textured = Some(handle);
            }
            None => {
                let verts: Vec<MeshVertex> = skinned
                    .iter()
                    .map(|v| MeshVertex {
                        position: v.position,
                        normal: v.normal,
                        material: 0,
                    })
                    .collect();
                let handle = r.upload_mesh(&verts, MeshIndices::U32(&model.mesh.indices));
                r.draw_mesh(
                    handle,
                    model.world,
                    MeshDrawOptions {
                        tint: CLAY,
                        ..Default::default()
                    },
                );
                self.mesh = Some(handle);
            }
        }
    }
}

/// Digital edges + the crouch toggle + the jump edge + the analog stick, folded into
/// the skeletal runtime's locomotion contract. Pure — the mapping's unit under test.
fn fold_inputs(held: Held, crouched: bool, jump: bool, stick: Vec2) -> state::Inputs {
    let (sx, sy) = (stick.x, stick.y);
    let stick_live = stick.length() > DEAD;
    let forward = held.forward || (stick_live && sy > sx.abs());
    let back = held.back || (stick_live && -sy > sx.abs());
    let left = held.left || (stick_live && -sx >= sy.abs());
    let right = held.right || (stick_live && sx >= sy.abs());
    state::Inputs {
        move_: forward || back || left || right,
        left,
        right,
        back,
        run: held.sprint || stick.length() > RUN_TILT,
        crouch: crouched,
        jump,
        ..Default::default()
    }
}

/// The pack state those inputs mean, against the seeded golem pack's state names.
/// Jump outranks everything; crouch keeps its own family; direction picks the
/// strafe/back variants; run picks the Run family over Walk.
fn desired_state(i: &state::Inputs) -> &'static str {
    if i.jump {
        return "Jump";
    }
    if i.crouch {
        return if !i.move_ {
            "Crouch"
        } else if i.back {
            "Crouch_Move_B"
        } else {
            "Crouch_Move"
        };
    }
    if !i.move_ {
        return "Idle";
    }
    match (i.back, i.left, i.right, i.run) {
        (true, _, _, false) => "Walk_B",
        (true, _, _, true) => "Run_B",
        (_, true, _, false) => "Walk_L",
        (_, true, _, true) => "Run_L",
        (_, _, true, false) => "Walk_R",
        (_, _, true, true) => "Run_R",
        (_, _, _, true) => "Run",
        _ => "Walk",
    }
}

/// THE GENERATED-GAIT DRIVE. A body whose pack names generated gaits (a creature pattern —
/// quadruped, bird, bat) has no clip to sample: while the machine sits in such a state the
/// stage steps the gait generator (`flicker-mechanics::gait::Locomotion`, G2) over the rig's
/// rest frames instead — on a flat floor at the feet's rest height, along the controller's
/// heading, at the speed the state's gait MEANS. The driver keeps the phase clock and the
/// foot contacts from frame to frame; the machine keeps which gait is wanted.
struct GaitDrive {
    loco: Locomotion,
    parents: Vec<i32>,
    /// The rig's rest world frames — what the driver places rigidly before it plants the feet.
    rest: Vec<Mat4>,
    /// This frame's posed world frames (rig space) — what `render` skins in a generated state.
    globals: Vec<Mat4>,
    floor: FlatFloor,
    /// The wanted gait nothing generates yet (`fly` / `glide` / `perch` until G4): the body
    /// stands, and the caption and the log say why.
    undriven: Option<Gait>,
    /// The speed (cm/s) the last step drove at.
    speed: f32,
}

impl GaitDrive {
    /// Read the rig's limbs off its rest frames; `None` when nothing plants (no leg or
    /// foreleg chain on the canon names).
    fn new(bones: &[Bone]) -> Option<Self> {
        let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
        let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
        let locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        let rest = pose::global_transforms(bones, &locals);
        let loco = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
        if loco.feet.is_empty() {
            return None;
        }
        // The floor is where the feet rest: the lowest effector at rest.
        let height = loco
            .feet
            .iter()
            .map(|f| rest[f.limb.effector].w_axis.z)
            .fold(f32::INFINITY, f32::min);
        Some(Self {
            globals: rest.clone(),
            loco,
            parents,
            rest,
            floor: FlatFloor { height },
            undriven: None,
            speed: 0.0,
        })
    }

    /// Step the generator for the machine's generated state under this frame's inputs: the
    /// state's family, the speed its gait means while a direction is held (a standstill
    /// otherwise), the controller's heading.
    fn step(&mut self, wanted: GeneratedGait, inputs: &state::Inputs, dt: f32) {
        self.loco.family = LocomotionFamily::from(wanted.family);
        let speed = match kind_of(wanted.gait) {
            Some(kind) => {
                self.undriven = None;
                if inputs.move_ {
                    speed_for(kind, self.loco.leg_len)
                } else {
                    0.0
                }
            }
            None => {
                if self.undriven != Some(wanted.gait) {
                    tracing::warn!(
                        gait = wanted.gait.name(),
                        "golem stage: no generator for this gait yet — the body stands"
                    );
                    self.undriven = Some(wanted.gait);
                }
                0.0
            }
        };
        self.speed = speed;
        let heading = heading_of(inputs, self.loco.heading);
        self.loco.step(
            &mut self.globals,
            &self.parents,
            &self.rest,
            heading,
            speed,
            &self.floor,
            dt,
        );
    }
}

/// The controller's heading in rig space — the canon faces −Y with +X to its LEFT: forward
/// and back run along the facing, a strafe turns the body to walk that way (a quadruped does
/// not sidestep). No direction held keeps the last heading.
fn heading_of(i: &state::Inputs, last: Vec3) -> Vec3 {
    let forward = -Vec3::Y;
    let right = -Vec3::X;
    let fx = f32::from(i.right) - f32::from(i.left);
    let fy = if i.back {
        -1.0
    } else if i.move_ && !i.left && !i.right {
        1.0
    } else {
        0.0
    };
    (forward * fy + right * fx).try_normalize().unwrap_or(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_mechanics::gait::GaitKind;
    use flicker_skeletal::state::GaitFamily;

    fn inputs(held: Held, crouched: bool, jump: bool, stick: Vec2) -> state::Inputs {
        fold_inputs(held, crouched, jump, stick)
    }

    /// The signal→state table: every family and direction, digital and analog.
    #[test]
    fn signals_map_to_the_packs_state_vocabulary() {
        let h = Held::default();
        assert_eq!(desired_state(&inputs(h, false, false, Vec2::ZERO)), "Idle");
        assert_eq!(
            desired_state(&inputs(
                Held { forward: true, ..h },
                false,
                false,
                Vec2::ZERO
            )),
            "Walk"
        );
        assert_eq!(
            desired_state(&inputs(
                Held {
                    forward: true,
                    sprint: true,
                    ..h
                },
                false,
                false,
                Vec2::ZERO
            )),
            "Run"
        );
        assert_eq!(
            desired_state(&inputs(Held { back: true, ..h }, false, false, Vec2::ZERO)),
            "Walk_B"
        );
        assert_eq!(
            desired_state(&inputs(
                Held {
                    left: true,
                    sprint: true,
                    ..h
                },
                false,
                false,
                Vec2::ZERO
            )),
            "Run_L"
        );
        assert_eq!(
            desired_state(&inputs(Held { right: true, ..h }, false, false, Vec2::ZERO)),
            "Walk_R"
        );
        // Crouch family — idle, forward, backward.
        assert_eq!(desired_state(&inputs(h, true, false, Vec2::ZERO)), "Crouch");
        assert_eq!(
            desired_state(&inputs(
                Held { forward: true, ..h },
                true,
                false,
                Vec2::ZERO
            )),
            "Crouch_Move"
        );
        assert_eq!(
            desired_state(&inputs(Held { back: true, ..h }, true, false, Vec2::ZERO)),
            "Crouch_Move_B"
        );
        // Jump outranks everything.
        assert_eq!(
            desired_state(&inputs(Held { forward: true, ..h }, true, true, Vec2::ZERO)),
            "Jump"
        );
    }

    /// The analog channel: a gentle tilt walks, full tilt runs, direction follows
    /// the dominant axis, and the dead zone stays still.
    #[test]
    fn the_stick_walks_then_runs_by_tilt() {
        let h = Held::default();
        assert_eq!(
            desired_state(&inputs(h, false, false, Vec2::new(0.05, 0.1))),
            "Idle"
        );
        assert_eq!(
            desired_state(&inputs(h, false, false, Vec2::new(0.0, 0.45))),
            "Walk"
        );
        assert_eq!(
            desired_state(&inputs(h, false, false, Vec2::new(0.0, 0.95))),
            "Run"
        );
        assert_eq!(
            desired_state(&inputs(h, false, false, Vec2::new(-0.5, 0.1))),
            "Walk_L"
        );
        assert_eq!(
            desired_state(&inputs(h, false, false, Vec2::new(0.9, -0.1))),
            "Run_R"
        );
        assert_eq!(
            desired_state(&inputs(h, false, false, Vec2::new(0.0, -0.5))),
            "Walk_B"
        );
    }

    /// The REAL content round-trip: load the reference body + the clip libraries + the
    /// Katanami pack, drive signal-shaped frames through the GRAPH, and watch the machine
    /// move. Skips when the content tree is absent, like every real-data test.
    /// THE BODY PICKER (Aaron 2026-09-07): the stage lists every promoted body, opens on the
    /// golem, and stepping the pick loads the next body with a pack (its own or the golem's)
    /// and its libraries. Real content; skips without it.
    #[test]
    fn the_picker_lists_promoted_bodies_and_steps_between_them() {
        if !golem_dir().exists() {
            eprintln!("skipping: no promoted golem");
            return;
        }
        let mut stage = GolemStage::new();
        assert!(stage.bodies().contains(&golem_dir()), "the golem is listed");
        assert_eq!(
            stage.body_name(),
            "GolemBaseV2",
            "the stage opens on the golem"
        );
        let roots = flicker_core::roots::roots();
        let tiers = [
            roots.package().join("characters"),
            roots.package().join("creatures"),
            roots.staging().join("creatures"),
        ];
        assert!(
            stage
                .bodies()
                .iter()
                .all(|b| tiers.iter().any(|t| b.starts_with(t))),
            "bodies come from the character and creature tiers (the pattern references live in skeletons/)"
        );
        // A listed creature has bones; an old static bake (the staged Horse, 0 bones) is
        // skipped, not crashed on.
        for b in stage.bodies() {
            if !b.starts_with(&tiers[0]) {
                assert!(
                    rig_has_bones(&own_rig(b)),
                    "{}: a listed creature carries a skeleton",
                    b.display()
                );
            }
        }
        let horse = tiers[2].join("Horse");
        if horse.is_dir() && !rig_has_bones(&own_rig(&horse)) {
            assert!(
                !stage.bodies().contains(&horse),
                "the boneless Horse bake is not listed"
            );
        }
        stage.load();
        assert!(stage.error.is_none(), "{:?}", stage.error);
        let before = stage.body_name();
        if stage.step_body(1) || stage.step_body(-1) {
            assert_ne!(stage.body_name(), before, "the pick moved");
            assert!(
                stage.error.is_none(),
                "the picked body loads: {:?}",
                stage.error
            );
            assert!(
                stage
                    .caption()
                    .is_some_and(|c| c.starts_with(&stage.body_name())),
                "the caption names the body"
            );
        }
    }

    /// THE PACK RESOLUTION GATE (G5): a body on the Quadruped pattern resolves to the
    /// pattern's default pack, whose Walk state is a GENERATED walk — no clip, the
    /// generator's — and every state of it is generated; the graph builds against NO
    /// library and drives itself off the same inputs.
    #[test]
    fn a_quadruped_body_resolves_to_a_pack_whose_walk_is_generated() {
        let reference = flicker_core::roots::roots()
            .package()
            .join("skeletons/Quadruped");
        if !own_rig(&reference).with_extension("json.gz").exists() {
            eprintln!("skipping: no Quadruped reference");
            return;
        }
        // The reference folder is shaped like a body (`<Name>/<Name>.json`), so it resolves
        // exactly the way a promoted quadruped will.
        let pack = pack_for(&reference);
        assert_eq!(pack, pattern_pack(Pattern::Quadruped));
        let def = state::load_pack(&pack).expect("the Quadruped default pack parses");
        let walk = def
            .states
            .iter()
            .find(|s| s.name == "Walk")
            .expect("a Walk state");
        assert_eq!(
            walk.clip.generated(),
            Some(GeneratedGait {
                gait: Gait::Walk,
                family: GaitFamily::Walker
            })
        );
        assert!(
            def.states.iter().all(|s| s.clip.generated().is_some()),
            "every state is generated"
        );
        let mut sm = StateMachine::build(&def, &[]).expect("builds with no clips at all");
        assert!(sm.warnings().is_empty(), "{:?}", sm.warnings());
        assert!(sm.has_graph());
        sm.advance(
            1.0 / 60.0,
            &state::Inputs {
                move_: true,
                ..Default::default()
            },
        );
        assert_eq!(sm.current_state_name(), "Walk", "move walks");
    }

    /// THE GENERATED WALK GATE (G5): stepping the Quadruped reference's generated walk for 60
    /// frames moves the root along the heading and keeps every hoof on or above the floor;
    /// a standstill stands; a gait nothing generates yet is said, not hidden.
    #[test]
    fn the_generated_walk_moves_the_root_and_keeps_every_hoof_on_the_floor() {
        let rig = own_rig(
            &flicker_core::roots::roots()
                .package()
                .join("skeletons/Quadruped"),
        );
        let Ok(text) = flicker_core::compression::read_text(&rig) else {
            eprintln!("skipping: no Quadruped reference");
            return;
        };
        let file: format::RigFile = serde_json::from_str(&text).expect("the reference parses");
        let bones = format::rig_bones(&file);
        let mut drive = GaitDrive::new(&bones).expect("four limbs to plant");
        assert_eq!(drive.loco.feet.len(), 4);
        let floor = drive.floor.height;
        let walk = GeneratedGait {
            gait: Gait::Walk,
            family: GaitFamily::Walker,
        };
        let forward = state::Inputs {
            move_: true,
            ..Default::default()
        };
        for _ in 0..60 {
            drive.step(walk, &forward, 1.0 / 60.0);
            assert_eq!(
                drive.loco.gait,
                GaitKind::Walk,
                "the speed the walk means walks"
            );
            for foot in &drive.loco.feet {
                let hoof = drive.globals[foot.limb.effector].w_axis.z;
                assert!(
                    hoof >= floor - 0.5,
                    "a hoof went through the floor: z = {hoof}, floor = {floor}"
                );
            }
        }
        let root = drive.globals[0].w_axis.truncate();
        assert!(root.length() > 50.0, "the root travelled: {root}");
        assert!(
            drive.loco.position.y < -50.0,
            "along the facing (−Y): {}",
            drive.loco.position
        );
        assert!(drive.undriven.is_none());
        assert!(drive.speed > 0.0);

        let before = drive.loco.position;
        drive.step(walk, &state::Inputs::default(), 1.0 / 60.0);
        assert_eq!(drive.loco.gait, GaitKind::Stand, "no direction held stands");
        assert_eq!(drive.loco.position, before);

        let fly = GeneratedGait {
            gait: Gait::Fly,
            family: GaitFamily::Walker,
        };
        drive.step(fly, &forward, 1.0 / 60.0);
        assert_eq!(drive.undriven, Some(Gait::Fly), "no generator yet — said");
        assert_eq!(drive.speed, 0.0);
    }

    #[test]
    fn the_golem_loads_and_signals_move_the_machine() {
        let mut stage = GolemStage::new();
        if !golem_dir().exists() {
            eprintln!("skipping: no content tree");
            return;
        }
        stage.load();
        assert!(stage.error.is_none(), "golem stage load: {:?}", stage.error);

        // THE ORIENTATION GUARD (QA 2026-08-04: "the golem is lying on its back"):
        // engine space is Y-UP, so the world-transformed rest body must be TALLEST
        // along Y — a Z-up misread lays it down, and this catches it headless.
        {
            let model = stage.model.as_ref().unwrap();
            let locals: Vec<Mat4> = model.bones.iter().map(|b| b.local).collect();
            let globals = pose::global_transforms(&model.bones, &locals);
            let palette = skin::palette(&model.bones, &globals);
            let rest = skin::skin(&model.mesh, &palette);
            let mut min = Vec3::splat(f32::MAX);
            let mut max = Vec3::splat(f32::MIN);
            for v in &rest {
                let p = model.world.transform_point3(Vec3::from(v.position));
                min = min.min(p);
                max = max.max(p);
            }
            let d = max - min;
            assert!(
                d.y > d.x && d.y > d.z,
                "the body must stand along +Y in engine space, got extents {d:?}"
            );
            assert!(
                (stage.floor - min.y).abs() < 1e-3,
                "the stage floor is the Y ground, got floor={} min.y={}",
                stage.floor,
                min.y
            );
        }

        // THE ANIMATED-POSE GUARD (2026-08-20 — the gap the bind guard above missed):
        // a rig NOT conformed to the Humanoid reference STANDS at its bind (so the check
        // above passes) but LIES ON ITS SIDE the moment a clip plays, because the retarget
        // rebases translation only — the bind's bone frames then fight the shared clips'
        // absolute rotations (GolemBase_Low shipped this way: pelvis frame 90° off,
        // extents X≈2.0 Y≈0.65). So the Idle pose the pack opens on must ALSO be tallest
        // along Y — sampled exactly as `render()` does, world transform included.
        {
            let model = stage.model.as_ref().unwrap();
            let machine = stage.machine.as_ref().unwrap();
            assert_eq!(
                machine.current_state_name(),
                "Idle",
                "the pack opens on Idle"
            );
            let locals = pose::sample_local_poses(
                &model.bones,
                &model.clips[machine.current_clip()],
                machine.current_tick(),
                model.retarget,
            );
            let globals = pose::global_transforms(&model.bones, &locals);
            let palette = skin::palette(&model.bones, &globals);
            let posed = skin::skin(&model.mesh, &palette);
            let mut min = Vec3::splat(f32::MAX);
            let mut max = Vec3::splat(f32::MIN);
            for v in &posed {
                let p = model.world.transform_point3(Vec3::from(v.position));
                min = min.min(p);
                max = max.max(p);
            }
            let d = max - min;
            assert!(
                d.y > d.x && d.y > d.z,
                "the ANIMATED Idle body must stand along +Y — a rig not conformed to \
                 the Humanoid reference animates on its side (bind frames fight the shared \
                 clips); got extents {d:?}"
            );
        }

        let m = stage.machine.as_mut().unwrap();
        assert_eq!(m.current_state_name(), "Idle", "the pack opens on Idle");
        assert!(
            m.warnings().is_empty(),
            "every state's clip resolves against the loaded libraries: {:?}",
            m.warnings()
        );
        assert!(
            m.has_graph(),
            "the Katanami pack is a real transition graph"
        );

        // Held forward + run: the GRAPH takes Idle → Walk → Run on its own edges.
        let inputs = state::Inputs {
            move_: true,
            run: true,
            ..Default::default()
        };
        for _ in 0..30 {
            m.advance(1.0 / 60.0, &inputs);
            if m.current_state_name() == "Run" {
                break;
            }
        }
        assert_eq!(
            m.current_state_name(),
            "Run",
            "move + run reaches Run via the graph"
        );
        let model = stage.model.as_ref().unwrap();
        let clip = &model.clips[stage.machine.as_ref().unwrap().current_clip()];
        assert_eq!(
            clip.name, "Run_nonWeapon",
            "Run plays the Katanami clip by name"
        );
        assert!(
            !clip.tracks.is_empty(),
            "the Katanami clip resolved onto the golem's canonical bones"
        );

        // An attack edge from Idle: the graph enters Attack_1 and its clip plays.
        let m = stage.machine.as_mut().unwrap();
        let idle = state::Inputs::default();
        for _ in 0..600 {
            m.advance(1.0 / 60.0, &idle);
            if m.current_state_name() == "Idle" {
                break;
            }
        }
        assert_eq!(
            m.current_state_name(),
            "Idle",
            "releasing the stick returns to Idle"
        );
        let attack = state::Inputs {
            attack: true,
            ..Default::default()
        };
        m.advance(1.0 / 60.0, &attack);
        assert_eq!(
            m.current_state_name(),
            "Attack_1",
            "an attack edge enters Attack_1"
        );
    }
}
