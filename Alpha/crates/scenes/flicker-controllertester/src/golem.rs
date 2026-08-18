//! The GOLEM STAGE — the reference body standing in the tester's centre panel,
//! playing locomotion off the SAME dispatched signals the right-hand panels print.
//!
//! This is the bus inspector's proof-of-life: a fired `ActionSignal` is not just a
//! row in the CONSUMED panel, it is MOTION. Signals the gameplay-base layer consumes
//! (World context only — push Menu/Radial/TextEntry and the golem honestly stops
//! responding, because the bus really is routing elsewhere) fold into the skeletal
//! runtime's [`state::Inputs`] locomotion contract; the analog left stick rides in
//! beside them from the latch. A small pure mapper picks the pack state those inputs
//! mean TODAY (the seeded golem pack is states-only); when the pack grows its own
//! transition graph in Loomforge, `StateMachine::advance` consumes the SAME `Inputs`
//! and the mapper simply stops disagreeing with it.
//!
//! Drawing reuses the proven shared seams end to end: `sample_local_poses` (+
//! `blend_local_poses` across the machine's blend window) → `global_transforms` →
//! `skin::palette` → `skin::skin` → one flat clay upload per frame (freed next
//! frame), exactly the paperdoll's path for one live character.

use std::path::PathBuf;
use std::time::Duration;

use flicker::render::{
    grid_segments, Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Rect,
    Renderer, SceneLighting, Vec2, Vec3,
};
use flicker_input_core::{ActionSignal, AnalogFrame, EventKind};
use flicker_input_router::{DispatchReport, InputEvent};
use flicker_skeletal::format::{self, Model};
use flicker_skeletal::state::{self, StateMachine};
use flicker_skeletal::{pose, skin};

/// The reference body — the ONE character the package ships (the content sweep's
/// invariant), addressed the same way the other scenes address package content.
fn golem_dir() -> PathBuf {
    flicker_core::roots::roots()
        .package()
        .join("characters/GolemBase_Low")
}
/// The shared clip library the golem plays, resolved by canonical bone name.
fn clips_dir() -> PathBuf {
    flicker_core::roots::roots()
        .package()
        .join("retarget/clips/locomotion")
}

/// The demo handler chain's gameplay-base slot (system 0 ▸ scene-root 1 ▸ modal 2 ▸
/// gameplay 3) — the layer whose consumed signals become motion.
const GAMEPLAY_BASE: usize = 3;

/// The flat clay tint — the same recessive reference-body reading the Clayworks
/// viewport gives the golem (untextured draw; the stage is about MOTION, not maps).
const CLAY: [f32; 4] = [0.62, 0.66, 0.74, 1.0];
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
    model: Option<Model>,
    machine: Option<StateMachine>,
    /// Last frame's uploaded skin, freed before each new upload (one live handle).
    mesh: Option<MeshHandle>,
    held: Held,
    /// Crouch is a TOGGLE (the ruled LS-click semantics), flipped on the press edge.
    crouched: bool,
    /// Jump press edge, consumed by the next frame's `Inputs`.
    jump: bool,
    /// Rest-pose framing (world space): centre, half-extent, ground height.
    center: Vec3,
    radius: f32,
    floor: f32,
    /// Load failure, shown on the stage instead of an empty hole — fail loud.
    pub error: Option<String>,
}

impl GolemStage {
    pub fn new() -> Self {
        Self {
            model: None,
            machine: None,
            mesh: None,
            held: Held::default(),
            crouched: false,
            jump: false,
            center: Vec3::ZERO,
            radius: 100.0,
            floor: 0.0,
            error: None,
        }
    }

    /// Load the reference body + the shared clips + its pack. IO only — uploads are
    /// per-frame. A missing content tree records an error the stage displays.
    pub fn load(&mut self) {
        let load = || -> Result<(Model, StateMachine), String> {
            let golem = golem_dir();
            let clips = clips_dir();
            let model = format::load_dirs(&[&golem, &clips]).map_err(|e| e.to_string())?;
            let def = state::load_pack(&golem.join("GolemBase_Low.pack.json"))
                .map_err(|e| e.to_string())?;
            let refs: Vec<state::ClipRef> = model
                .clips
                .iter()
                .map(|c| state::ClipRef {
                    name: &c.name,
                    duration_ticks: c.duration_ticks,
                })
                .collect();
            let machine = StateMachine::build(&def, &refs).map_err(|e| e.to_string())?;
            Ok((model, machine))
        };
        match load() {
            Ok((model, machine)) => {
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
                _ => {}
            }
        }

        // The analog channel rides in beside the digital edges (spec: Move* analog
        // lives on the latch). Stick direction augments; full tilt means run.
        let stick = latch.map(|f| f.left_stick).unwrap_or(Vec2::ZERO);
        let inputs = fold_inputs(self.held, self.crouched, self.jump, stick);
        self.jump = false;

        if let Some(m) = self.machine.as_mut() {
            // The seeded pack is states-only, so the scene picks the state those
            // inputs mean; a mid-flight Jump is left to finish (`next` → Idle).
            let desired = desired_state(&inputs);
            if m.current_state_name() != "Jump"
                && m.current_state_name() != desired
                && !m.force_state_by_name(desired)
            {
                tracing::warn!("golem stage: pack has no state `{desired}`");
            }
            m.advance(dt.as_secs_f32(), &inputs);
        }
    }

    /// The machine's live caption: `STATE · clip · tick/len` for the stage footer.
    pub fn caption(&self) -> Option<String> {
        let m = self.machine.as_ref()?;
        let model = self.model.as_ref()?;
        let clip = model
            .clips
            .get(m.current_clip())
            .map(|c| c.name.as_str())
            .unwrap_or("?");
        Some(format!(
            "{}  \u{00b7}  {}  \u{00b7}  {}/{}",
            m.current_state_name(),
            clip,
            m.current_tick(),
            m.current_duration()
        ))
    }

    /// Draw the 3D stage (camera + ground + the skinned body). Called FIRST in the
    /// scene's render — the UI chrome then frames the centre `view` rect around it.
    pub fn render(&mut self, r: &mut Renderer, view: Rect) {
        // One live skin upload: free last frame's before this frame's replaces it.
        if let Some(h) = self.mesh.take() {
            r.free_mesh(h);
        }
        let (Some(model), Some(machine)) = (self.model.as_ref(), self.machine.as_ref()) else {
            return;
        };

        // Aim the camera so the body sits in the CENTRE PANEL, not the window centre:
        // shift the look target opposite the panel's horizontal offset from the
        // window's middle (a small, exact correction — the camera is full-frame).
        let size = r.size();
        let panel_cx = view.pos.x + view.size.x * 0.5;
        let frac = ((size.x * 0.5 - panel_cx) / size.x).clamp(-0.25, 0.25);
        let target = self.center + Vec3::new(frac * self.radius * 2.6, 0.0, 0.0);
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
        r.set_scene(SceneLighting::default());

        // Ground lattice at the body's feet (the Y-up stage helper).
        let ground = grid_segments(self.radius * 0.25, self.radius * 2.5, self.floor);
        r.draw_lines(&ground, GROUND);

        // Pose → blend → globals → palette → skin → one flat clay upload.
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
        let globals = pose::global_transforms(&model.bones, &locals);
        let palette = skin::palette(&model.bones, &globals);
        let skinned = skin::skin(&model.mesh, &palette);
        if skinned.is_empty() {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The REAL content round-trip: load the reference body + shared clips + the
    /// seeded pack, drive one signal-shaped frame, and watch the machine move.
    /// Skips when the content tree is absent, like every real-data test.
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

        let m = stage.machine.as_mut().unwrap();
        assert_eq!(m.current_state_name(), "Idle", "the pack opens on Idle");

        // A held forward + run: the mapper forces Run, advance plays it.
        let inputs = state::Inputs {
            move_: true,
            run: true,
            ..Default::default()
        };
        assert!(m.force_state_by_name(desired_state(&inputs)));
        m.advance(0.10, &inputs);
        assert_eq!(m.current_state_name(), "Run");
        let model = stage.model.as_ref().unwrap();
        let clip = &model.clips[stage.machine.as_ref().unwrap().current_clip()];
        assert_eq!(clip.name, "run_jog", "Run plays the pack's clip by name");
        assert!(
            !clip.tracks.is_empty(),
            "the shared clip resolved onto the golem's canonical bones"
        );
    }
}
