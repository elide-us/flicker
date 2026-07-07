//! flicker-animation — skeletal-animation model viewer POC.
//!
//! Loads the `flicker.rig` JSON (rig + clips) emitted by the `fbximport` converter,
//! samples the selected clip at the current tick on the CPU (the authoritative pose
//! layer), and renders:
//!  * SLICE 1 — the bone wireframe (parent→child segments via the lines pipeline);
//!  * SLICE 2/3 — the CPU-skinned mesh, one draw per material submesh, textured with
//!    each submesh's albedo (via `flicker-render`'s textured mesh pipeline) or flat
//!    gray where no texture maps. Per-frame CPU skin + re-upload.
//!
//! Controls: drag = rotate · wheel = zoom · Space = play/pause · ←/→ = step tick ·
//! ↑/↓ = cycle clip · PgUp/PgDn = raise/lower · M = mesh · T = textures · B = skeleton
//! · R = reset · Esc = quit.
//!
//! The user runs the window (`cargo run -p flicker-animation`) and verifies; this
//! crate never launches it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, App, InputState, Key};
use flicker::render::{
    Camera, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, PbrMaps, Renderer, TextureHandle,
    TexturedMeshHandle, TexturedVertex, Vec2, Vec3,
};
use glam::Mat4;

use flicker_skeletal::{format, pose, skin, state};

use format::Model;
use state::{Inputs, StateMachine};

/// Which subsystem drives clip selection + playback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    /// The animation state machine owns the pose (gameplay input → states/events).
    Graph,
    /// Manual clip browser (the original POC controls: cycle clip, step tick).
    Manual,
}

/// A submesh's GPU upload for the current frame — textured (albedo) or flat (gray).
enum SubGpu {
    Textured(TexturedMeshHandle),
    Flat(MeshHandle),
}

/// A static prop submesh, uploaded once — textured (albedo + PBR maps) or flat (colour).
enum PropPart {
    Textured(TexturedMeshHandle, TextureHandle, PbrMaps),
    Flat(MeshHandle),
}

/// Neutral "stone" material id — the fallback flat colour for untextured submeshes
/// that carry no explicit colour.
const FLAT_GRAY_MATERIAL: u32 = 10;

/// Steel-ish flat colour for the (untextured) katana prop.
const KATANA_COLOR: [f32; 3] = [0.55, 0.57, 0.62];

/// Pack an RGB colour (0..1) into the mesh pipeline's direct-RGB material escape:
/// the low 12 bits set to `0xFFF` mark a packed RGB666 in the upper bits (see
/// `flicker-render` `mesh.wgsl` `material_color`), so an exact flat colour renders
/// through the existing lit-mesh pipeline with no tint fudging.
fn pack_rgb666(r: f32, g: f32, b: f32) -> u32 {
    let q = |c: f32| ((c.clamp(0.0, 1.0) * 63.0).round() as u32) & 0x3F;
    0xFFF | (q(r) << 12) | (q(g) << 18) | (q(b) << 24)
}

/// Build a `TexturedVertex` list for a contiguous, **non-deduplicated** triangle range
/// (each 3 sequential vertices form one triangle — the converter emits geometry that
/// way). Computes a per-triangle tangent from the 3 positions + UVs (standard
/// `dP/dUV` solve) and assigns it, orthonormalized against each corner's normal, to all
/// three corners — no cross-vertex averaging needed. `w` carries the handedness sign so
/// the shader can reconstruct the bitangent. Positions/normals come from `pn` (skinned
/// or bind geometry); UVs from `uvs`. Both are indexed by absolute vertex index `j`.
fn build_textured_verts(
    range: std::ops::Range<usize>,
    pos: impl Fn(usize) -> [f32; 3],
    nrm: impl Fn(usize) -> [f32; 3],
    uv: impl Fn(usize) -> [f32; 2],
) -> Vec<TexturedVertex> {
    let count = range.len();
    let mut out: Vec<TexturedVertex> = range
        .map(|j| TexturedVertex {
            position: pos(j),
            normal: nrm(j),
            uv: uv(j),
            // Placeholder; overwritten per-triangle below.
            tangent: [1.0, 0.0, 0.0, 1.0],
        })
        .collect();

    // Each consecutive triple is one triangle (local indices 3k, 3k+1, 3k+2).
    let tris = count / 3;
    for tk in 0..tris {
        let i0 = tk * 3;
        let i1 = i0 + 1;
        let i2 = i0 + 2;
        let p0 = Vec3::from(out[i0].position);
        let p1 = Vec3::from(out[i1].position);
        let p2 = Vec3::from(out[i2].position);
        let uv0 = Vec2::from(out[i0].uv);
        let uv1 = Vec2::from(out[i1].uv);
        let uv2 = Vec2::from(out[i2].uv);

        let e1 = p1 - p0;
        let e2 = p2 - p0;
        let d1 = uv1 - uv0;
        let d2 = uv2 - uv0;
        let det = d1.x * d2.y - d2.x * d1.y;

        // Tangent = normalize(dP/dU). Degenerate UVs (det≈0) fall back to an arbitrary
        // basis so the TBN stays finite (the shader re-orthonormalizes anyway).
        let (tangent, sign) = if det.abs() > 1e-8 {
            let r = 1.0 / det;
            let t = (e1 * d2.y - e2 * d1.y) * r;
            let bt = (e2 * d1.x - e1 * d2.x) * r;
            // Handedness: +1 if the geometric bitangent agrees with N×T, else -1.
            let n = (Vec3::from(out[i0].normal)
                + Vec3::from(out[i1].normal)
                + Vec3::from(out[i2].normal))
            .normalize_or_zero();
            let sign = if n.cross(t).dot(bt) < 0.0 { -1.0 } else { 1.0 };
            let t = t.normalize_or_zero();
            let t = if t.length_squared() < 1e-12 {
                Vec3::X
            } else {
                t
            };
            (t, sign)
        } else {
            (Vec3::X, 1.0)
        };
        for li in [i0, i1, i2] {
            out[li].tangent = [tangent.x, tangent.y, tangent.z, sign];
        }
    }
    out
}

/// Orbit camera mirrored from `flicker-world`'s `OrbitCam` (drag rotates, wheel
/// zooms). Looks at the origin — the model is centred there by `Model::world`.
struct OrbitCam {
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
    prev_mouse: Vec2,
    dragging: bool,
}

impl OrbitCam {
    fn new(radius: f32) -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.35,
            distance: radius * 3.0,
            min_distance: radius * 1.2,
            max_distance: radius * 9.0,
            prev_mouse: Vec2::ZERO,
            dragging: false,
        }
    }

    fn update(&mut self, input: &InputState) {
        let mouse = input.mouse_position;
        if input.mouse_left {
            if self.dragging {
                let delta = mouse - self.prev_mouse;
                self.yaw -= delta.x * 0.01;
                self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.4, 1.4);
            }
            self.dragging = true;
        } else {
            self.dragging = false;
        }
        self.prev_mouse = mouse;

        let wheel = input.mouse_wheel_delta;
        if wheel != 0.0 {
            self.distance =
                (self.distance * (1.0 - wheel * 0.1)).clamp(self.min_distance, self.max_distance);
        }
    }

    fn camera(&self) -> Camera {
        Camera::orbit(Vec3::ZERO, self.distance, self.yaw, self.pitch)
    }
}

struct Viewer {
    model: Model,
    cam: OrbitCam,
    clip_index: usize,
    /// Playback position in ticks (float; integer part selects the keyframe).
    play_head: f32,
    playing: bool,
    /// Vertical reframing offset (engine metres) added to the world transform, so
    /// the camera can look higher/lower up the model. Persists across clips.
    world_y: f32,
    /// Effective submeshes `(material, start, count)` — the JSON's, or a single
    /// whole-mesh fallback. Index ranges are also vertex ranges (sequential indices).
    sub: Vec<(usize, usize, usize)>,
    /// Per-submesh GPU upload for this frame (freed + replaced each frame).
    sub_gpu: Vec<Option<SubGpu>>,
    /// Albedo textures keyed by PNG basename (loaded once in `init`, deduped).
    textures: HashMap<String, TextureHandle>,
    /// Assets dir, for loading textures in `init`.
    assets: PathBuf,
    show_mesh: bool,
    show_skeleton: bool,
    show_textures: bool,
    /// Skin variant: 0 = Color_1, 1 = Color_2, 2 = Color_3 (1/2/3 keys). Swaps the
    /// albedo atlas only; PBR maps stay Color_1 (the variants are recolours).
    skin: usize,
    // Katana prop attached to the character's Weapon_R socket.
    /// Katana mesh loaded in `main`, uploaded to GPU in `init` (then cleared).
    katana_mesh: Option<format::Mesh>,
    /// Per-submesh GPU parts for the katana (uploaded once; static geometry).
    katana_parts: Vec<PropPart>,
    /// Index of the `Weapon_R` socket bone in the character skeleton, if present.
    weapon_bone: Option<usize>,
    katana_equipped: bool,
    // ── Animation state machine (combat spine, slice 1) ──
    /// The state graph, if a `flicker.pack` loaded. Drives clip + tick in Graph mode.
    sm: Option<StateMachine>,
    /// Which subsystem drives playback (Graph if a pack loaded, else Manual).
    mode: ViewMode,
    /// Window events open at the state machine's current tick (HUD).
    hud_active: Vec<String>,
    /// Ring of the most recent fired timeline events (HUD).
    hud_events: Vec<String>,
    /// Whether transition crossfades are applied in Graph mode (`L` toggles — an A/B
    /// against the old hard-cut). The state machine tracks the blend regardless; this
    /// only controls whether the viewer interpolates the outgoing pose.
    blend_enabled: bool,
    // Edge-detection for one-shot controls (InputState exposes level state for keys).
    prev_space: bool,
    prev_up: bool,
    prev_down: bool,
    prev_left: bool,
    prev_right: bool,
    prev_r: bool,
    prev_m: bool,
    prev_b: bool,
    prev_t: bool,
    prev_k: bool,
    prev_g: bool,
    prev_f: bool,
    prev_h: bool,
    prev_x: bool,
    prev_l: bool,
    should_quit: bool,
}

impl Viewer {
    fn new(model: Model, assets: PathBuf, katana_mesh: Option<format::Mesh>) -> Self {
        let cam = OrbitCam::new(model.orbit_radius);
        let has_mesh = !model.mesh.vertices.is_empty();
        let weapon_bone = model.bones.iter().position(|b| b.name == "Weapon_R");
        // Effective submeshes: the JSON's material groups, or one whole-mesh fallback
        // (material usize::MAX → no material → renders flat).
        let sub: Vec<(usize, usize, usize)> = if !model.mesh.submeshes.is_empty() {
            model
                .mesh
                .submeshes
                .iter()
                .map(|s| (s.material, s.start, s.count))
                .collect()
        } else if has_mesh {
            vec![(usize::MAX, 0, model.mesh.indices.len())]
        } else {
            Vec::new()
        };
        let sub_gpu = (0..sub.len()).map(|_| None).collect();

        // Build the animation state machine from the content pack (if present). States
        // reference clips by NAME; resolve them against the loaded model's clip list
        // (mirroring how the rig loader resolves clip tracks to bones by name).
        let sm = match state::load_pack(&assets.join("Katanami.pack.json")) {
            Ok(def) => {
                let refs: Vec<state::ClipRef> = model
                    .clips
                    .iter()
                    .map(|c| state::ClipRef {
                        name: &c.name,
                        duration_ticks: c.duration_ticks,
                    })
                    .collect();
                match StateMachine::build(&def, &refs) {
                    Ok(sm) => {
                        for w in sm.warnings() {
                            tracing::warn!("state pack: {w}");
                        }
                        tracing::info!(
                            "state machine ready — starting in '{}'",
                            sm.current_state_name()
                        );
                        Some(sm)
                    }
                    Err(e) => {
                        tracing::warn!("state pack failed to build ({e}); manual mode only");
                        None
                    }
                }
            }
            Err(e) => {
                tracing::info!("no state pack ({e}); manual clip browser only");
                None
            }
        };
        // Default to graph-driven playback when a pack loaded; else the manual browser.
        let mode = if sm.is_some() {
            ViewMode::Graph
        } else {
            ViewMode::Manual
        };

        Self {
            model,
            cam,
            sm,
            mode,
            hud_active: Vec::new(),
            hud_events: Vec::new(),
            blend_enabled: true,
            clip_index: 0,
            play_head: 0.0,
            playing: true,
            world_y: 0.0,
            sub,
            sub_gpu,
            textures: HashMap::new(),
            assets,
            // Show the skinned mesh when there is one; otherwise fall back to bones.
            show_mesh: has_mesh,
            show_skeleton: !has_mesh,
            show_textures: true,
            skin: 0,
            katana_mesh,
            katana_parts: Vec::new(),
            weapon_bone,
            katana_equipped: true,
            prev_space: false,
            prev_up: false,
            prev_down: false,
            prev_left: false,
            prev_right: false,
            prev_r: false,
            prev_m: false,
            prev_b: false,
            prev_t: false,
            prev_k: false,
            prev_g: false,
            prev_f: false,
            prev_h: false,
            prev_x: false,
            prev_l: false,
            should_quit: false,
        }
    }

    fn current_clip(&self) -> Option<&format::ResolvedClip> {
        self.model.clips.get(self.clip_index)
    }

    fn duration(&self) -> u32 {
        self.current_clip()
            .map(|c| c.duration_ticks.max(1))
            .unwrap_or(1)
    }

    fn cycle_clip(&mut self, delta: i32) {
        if self.model.clips.is_empty() {
            return;
        }
        let n = self.model.clips.len() as i32;
        self.clip_index = (self.clip_index as i32 + delta).rem_euclid(n) as usize;
        self.play_head = 0.0;
    }

    fn step(&mut self, delta: i32) {
        self.playing = false;
        let dur = self.duration() as i32;
        let cur = self.play_head.floor() as i32;
        self.play_head = (cur + delta).rem_euclid(dur) as f32;
    }

    /// Resolve a material's PBR map basenames to loaded texture handles (each `None`
    /// when the map is absent or failed to load → the pipeline default). `textures-off`
    /// suppresses the whole set (matte look).
    fn resolve_maps(&self, mat: &format::Material) -> PbrMaps {
        if !self.show_textures {
            return PbrMaps::default();
        }
        let get = |name: &str| {
            if name.is_empty() {
                None
            } else {
                self.textures.get(name).copied()
            }
        };
        PbrMaps {
            normal: get(&mat.normal),
            roughness: get(&mat.roughness),
            metalness: get(&mat.metalness),
            ao: get(&mat.ao),
        }
    }

    /// Resolve a base-color name to the current skin variant's texture: substitute the
    /// `Katanami_` prefix for `Katanami2_`/`Katanami3_` (Color_2/3), falling back to the
    /// original (Color_1) when that variant PNG isn't present (e.g. Color_2 has no hair).
    fn variant_albedo(&self, base: &str) -> Option<TextureHandle> {
        if base.is_empty() {
            return None;
        }
        let name = match self.skin {
            1 => base.replacen("Katanami_", "Katanami2_", 1),
            2 => base.replacen("Katanami_", "Katanami3_", 1),
            _ => base.to_string(),
        };
        self.textures
            .get(&name)
            .or_else(|| self.textures.get(base))
            .copied()
    }

    /// Sample a clip's LOCAL bone poses at `tick`, or the rest pose when the clip
    /// index is missing (an unresolved state, or no clips at all).
    fn sample_locals(&self, clip_idx: usize, tick: u32) -> Vec<Mat4> {
        match self.model.clips.get(clip_idx) {
            Some(clip) => {
                let t = tick.min(clip.duration_ticks.saturating_sub(1));
                pose::sample_local_poses(&self.model.bones, clip, t)
            }
            None => self.model.bones.iter().map(|b| b.local).collect(),
        }
    }

    /// Parent→child bone segments in engine space (`world` maps source space to
    /// engine space, including the vertical reframing offset).
    fn bone_segments(&self, world: Mat4, globals: &[Mat4]) -> Vec<(Vec3, Vec3)> {
        let mut segs = Vec::with_capacity(self.model.bones.len());
        for (i, bone) in self.model.bones.iter().enumerate() {
            if bone.parent < 0 {
                continue;
            }
            let a = world.transform_point3(globals[bone.parent as usize].w_axis.truncate());
            let b = world.transform_point3(globals[i].w_axis.truncate());
            segs.push((a, b));
        }
        segs
    }
}

impl App for Viewer {
    fn init(&mut self, renderer: &mut Renderer) {
        // Load every material's textures once (deduped). Albedo (base_color) is sRGB
        // COLOUR data; the PBR maps (normal/roughness/metalness/ao) are LINEAR data and
        // must upload via `load_texture_linear` (no sRGB decode) or normals/scalars would
        // be gamma-shifted. A missing/failed texture just leaves that slot at the
        // pipeline default. Includes the katana's overridden maps (its materials were
        // rewritten to the Hair atlas in `main`).
        let mut srgb: Vec<String> = Vec::new();
        let mut linear: Vec<String> = Vec::new();
        {
            let mut mats: Vec<&format::Material> =
                self.model.mesh.materials.iter().collect();
            if let Some(mesh) = &self.katana_mesh {
                mats.extend(mesh.materials.iter());
            }
            for m in mats {
                if !m.base_color.is_empty() {
                    srgb.push(m.base_color.clone());
                }
                for map in [&m.normal, &m.roughness, &m.metalness, &m.ao] {
                    if !map.is_empty() {
                        linear.push(map.clone());
                    }
                }
            }
        }
        // Skin-variant albedos (1/2/3 keys) — loaded if present; a missing variant
        // (e.g. Color_2 has no hair atlas) falls back to Color_1 at resolve time.
        for v in [
            "Katanami2_Body_BaseColor.png",
            "Katanami3_Body_BaseColor.png",
            "Katanami3_Hair_BaseColor.png",
        ] {
            srgb.push(v.to_string());
        }
        srgb.sort();
        srgb.dedup();
        linear.sort();
        linear.dedup();
        for (name, is_srgb) in srgb
            .iter()
            .map(|n| (n, true))
            .chain(linear.iter().map(|n| (n, false)))
        {
            if self.textures.contains_key(name) {
                continue;
            }
            let path = self.assets.join(name);
            match image::open(&path) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let handle = if is_srgb {
                        renderer.load_texture(rgba.as_raw(), w, h)
                    } else {
                        renderer.load_texture_linear(rgba.as_raw(), w, h)
                    };
                    self.textures.insert(name.clone(), handle);
                    tracing::info!(
                        "loaded texture {name} ({w}x{h}, {})",
                        if is_srgb { "sRGB" } else { "linear" }
                    );
                }
                Err(e) => tracing::warn!("texture {name} failed ({e}); slot uses default"),
            }
        }
        // Upload the katana prop once (static geometry). Each submesh is textured with
        // its albedo + PBR maps (already loaded above; overridden to the Hair atlas in
        // `main`, so the blade gets a steel metal/rough response) or flat steel where no
        // texture maps. Drawn each frame at the Weapon_R socket transform.
        if let Some(mesh) = self.katana_mesh.take() {
            let steel = pack_rgb666(KATANA_COLOR[0], KATANA_COLOR[1], KATANA_COLOR[2]);
            let ranges: Vec<(usize, usize, usize)> = if mesh.submeshes.is_empty() {
                vec![(usize::MAX, 0, mesh.indices.len())]
            } else {
                mesh.submeshes
                    .iter()
                    .map(|s| (s.material, s.start, s.count))
                    .collect()
            };
            for (mat, start, count) in ranges {
                if count == 0 || start + count > mesh.vertices.len() {
                    continue;
                }
                let material = mesh.materials.get(mat);
                let base = material.map(|m| m.base_color.as_str()).unwrap_or("");
                let tex = if base.is_empty() {
                    None
                } else {
                    self.textures.get(base).copied()
                };
                let indices: Vec<u32> = (0..count as u32).collect();
                let part = match tex {
                    Some(th) => {
                        let maps = material.map(|m| self.resolve_maps(m)).unwrap_or_default();
                        let verts = build_textured_verts(
                            start..start + count,
                            |j| mesh.vertices[j].p,
                            |j| mesh.vertices[j].n,
                            |j| mesh.vertices[j].uv,
                        );
                        let h = renderer.upload_textured_mesh(&verts, MeshIndices::U32(&indices));
                        PropPart::Textured(h, th, maps)
                    }
                    None => {
                        let verts: Vec<MeshVertex> = (start..start + count)
                            .map(|j| MeshVertex {
                                position: mesh.vertices[j].p,
                                normal: mesh.vertices[j].n,
                                material: steel,
                            })
                            .collect();
                        PropPart::Flat(renderer.upload_mesh(&verts, MeshIndices::U32(&indices)))
                    }
                };
                self.katana_parts.push(part);
            }
            tracing::info!(
                "katana prop: {} parts, attach bone Weapon_R = {:?}",
                self.katana_parts.len(),
                self.weapon_bone
            );
        }

        tracing::info!(
            "flicker-animation: {} bones, {} clips, {} submeshes, {} textures",
            self.model.bones.len(),
            self.model.clips.len(),
            self.sub.len(),
            self.textures.len(),
        );
    }

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) {
        if input.key_down(Key::Escape) {
            self.should_quit = true;
        }
        self.cam.update(input);

        // ── Edge-detect the one-shot keys once; some are reinterpreted per mode
        // (Space = play/pause in Manual, jump in Graph; R = reset play-head vs. reset
        // the state machine). ──
        let space = input.key_down(Key::Space);
        let space_edge = space && !self.prev_space;
        self.prev_space = space;
        let up = input.key_down(Key::Up);
        let up_edge = up && !self.prev_up;
        self.prev_up = up;
        let down = input.key_down(Key::Down);
        let down_edge = down && !self.prev_down;
        self.prev_down = down;
        let left = input.key_down(Key::Left);
        let left_edge = left && !self.prev_left;
        self.prev_left = left;
        let right = input.key_down(Key::Right);
        let right_edge = right && !self.prev_right;
        self.prev_right = right;
        let r = input.key_down(Key::R);
        let r_edge = r && !self.prev_r;
        self.prev_r = r;
        let f = input.key_down(Key::F);
        let f_edge = f && !self.prev_f;
        self.prev_f = f;
        let h = input.key_down(Key::H);
        let h_edge = h && !self.prev_h;
        self.prev_h = h;
        let x = input.key_down(Key::X);
        let x_edge = x && !self.prev_x;
        self.prev_x = x;

        // Mode toggle (only meaningful when a state pack loaded).
        let g = input.key_down(Key::G);
        if g && !self.prev_g && self.sm.is_some() {
            self.mode = match self.mode {
                ViewMode::Graph => ViewMode::Manual,
                ViewMode::Manual => ViewMode::Graph,
            };
        }
        self.prev_g = g;

        // ── Shared view controls (both modes) ──
        // Vertical reframing (held, smooth). Nudges the model up/down relative to the
        // camera target (which sits at the origin). Speed scales with model size.
        let vstep = self.model.orbit_radius * dt.as_secs_f32();
        if input.key_down(Key::PageUp) {
            self.world_y += vstep;
        }
        if input.key_down(Key::PageDown) {
            self.world_y -= vstep;
        }
        let m = input.key_down(Key::M);
        if m && !self.prev_m {
            self.show_mesh = !self.show_mesh;
        }
        self.prev_m = m;
        let b = input.key_down(Key::B);
        if b && !self.prev_b {
            self.show_skeleton = !self.show_skeleton;
        }
        self.prev_b = b;
        let t = input.key_down(Key::T);
        if t && !self.prev_t {
            self.show_textures = !self.show_textures;
        }
        self.prev_t = t;
        let k = input.key_down(Key::K);
        if k && !self.prev_k {
            self.katana_equipped = !self.katana_equipped;
        }
        self.prev_k = k;
        // Toggle transition crossfading (Graph mode) — an A/B against the old hard-cut.
        let l = input.key_down(Key::L);
        if l && !self.prev_l {
            self.blend_enabled = !self.blend_enabled;
        }
        self.prev_l = l;
        // Skin variant (Color_1/2/3) — direct selection, no edge-detect needed.
        if input.key_down(Key::Digit1) {
            self.skin = 0;
        } else if input.key_down(Key::Digit2) {
            self.skin = 1;
        } else if input.key_down(Key::Digit3) {
            self.skin = 2;
        }

        // ── Mode-specific playback ──
        match self.mode {
            ViewMode::Graph => {
                // Gameplay controls → state-machine inputs: movement modifiers are held;
                // jump/attack/hit/die are edges. (H = simulate taking a hit, X = die —
                // debug drivers for the any-state transitions.) WASD drives directional
                // locomotion: W forward, A/S/D left/back/right (any of them = moving).
                let w = input.key_down(Key::W);
                let a = input.key_down(Key::A);
                let s = input.key_down(Key::S);
                let d = input.key_down(Key::D);
                let inputs = Inputs {
                    move_: w || a || s || d,
                    left: a,
                    right: d,
                    back: s,
                    run: input.key_down(Key::LeftShift) || input.key_down(Key::RightShift),
                    crouch: input.key_down(Key::C),
                    jump: space_edge,
                    attack: f_edge,
                    hit: h_edge,
                    die: x_edge,
                };
                // Advance the machine (owned report ends the &mut sm borrow before we
                // touch the HUD buffers).
                let report = self.sm.as_mut().map(|sm| {
                    if r_edge {
                        sm.reset();
                    }
                    sm.advance(dt.as_secs_f32(), &inputs)
                });
                if let Some(report) = report {
                    self.hud_active = report
                        .active
                        .iter()
                        .map(|w| {
                            if w.label.is_empty() {
                                w.kind.tag().to_string()
                            } else {
                                format!("{} {}", w.kind.tag(), w.label)
                            }
                        })
                        .collect();
                    for e in &report.fired {
                        self.hud_events.push(if e.label.is_empty() {
                            format!("{}@{}", e.kind.tag(), e.tick)
                        } else {
                            format!("{} {}@{}", e.kind.tag(), e.label, e.tick)
                        });
                    }
                    let n = self.hud_events.len();
                    if n > 6 {
                        self.hud_events.drain(0..n - 6);
                    }
                }
            }
            ViewMode::Manual => {
                if space_edge {
                    self.playing = !self.playing;
                }
                if up_edge {
                    self.cycle_clip(-1);
                }
                if down_edge {
                    self.cycle_clip(1);
                }
                if left_edge {
                    self.step(-1);
                }
                if right_edge {
                    self.step(1);
                }
                if r_edge {
                    self.play_head = 0.0;
                }
                if self.playing {
                    if let Some(clip) = self.current_clip() {
                        let dur = clip.duration_ticks.max(1) as f32;
                        self.play_head += dt.as_secs_f32() * clip.tick_rate_hz as f32;
                        self.play_head = self.play_head.rem_euclid(dur);
                    }
                }
            }
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        renderer.set_camera(&self.cam.camera());

        // Choose the clip + tick to sample: the state machine in Graph mode, else the
        // manual browser. A missing clip (unresolved state, or no clips) → rest pose.
        let (clip_idx, tick) = match (self.mode, &self.sm) {
            (ViewMode::Graph, Some(sm)) => (sm.current_clip(), sm.current_tick()),
            _ => (
                self.clip_index,
                (self.play_head.floor() as u32).min(self.duration().saturating_sub(1)),
            ),
        };
        // Sample the incoming pose. If a Graph transition is crossfading (and blending
        // is enabled), sample the outgoing pose too and blend the LOCAL transforms
        // before forward kinematics. `blend_weight` is kept for the HUD readout.
        let incoming = self.sample_locals(clip_idx, tick);
        let active_blend = if self.blend_enabled && self.mode == ViewMode::Graph {
            self.sm.as_ref().and_then(|s| s.blend())
        } else {
            None
        };
        let blend_weight = active_blend.map(|b| b.weight);
        let locals = match active_blend {
            Some(b) => {
                let outgoing = self.sample_locals(b.from_clip, b.from_tick);
                pose::blend_local_poses(&outgoing, &incoming, b.weight)
            }
            None => incoming,
        };
        let globals = pose::global_transforms(&self.model.bones, &locals);

        let world = Mat4::from_translation(Vec3::new(0.0, self.world_y, 0.0)) * self.model.world;

        // Slice 2/3: CPU-skinned mesh, one draw per material submesh. Skin all
        // vertices once, then per submesh build textured (albedo) or flat (gray) GPU
        // vertices and re-upload (freeing last frame's buffers first). Index ranges
        // are vertex ranges — the converter emits a non-deduped, sequential list.
        let mesh_drawn = self.show_mesh && !self.model.mesh.vertices.is_empty();
        if mesh_drawn {
            let palette = skin::palette(&self.model.bones, &globals);
            let skinned = skin::skin(&self.model.mesh, &palette);
            for si in 0..self.sub.len() {
                if let Some(prev) = self.sub_gpu[si].take() {
                    match prev {
                        SubGpu::Textured(h) => renderer.free_textured_mesh(h),
                        SubGpu::Flat(h) => renderer.free_mesh(h),
                    }
                }
                let (mat, start, count) = self.sub[si];
                if count == 0 || start + count > skinned.len() {
                    continue;
                }
                // Resolve this submesh's albedo (empty / missing / textures-off → flat)
                // and its PBR map set (normal/roughness/metalness/ao → pipeline default
                // when absent).
                let material = self.model.mesh.materials.get(mat);
                let base = material.map(|m| m.base_color.as_str()).unwrap_or("");
                let tex = if self.show_textures {
                    self.variant_albedo(base)
                } else {
                    None
                };
                let indices: Vec<u32> = (0..count as u32).collect();
                match tex {
                    Some(th) => {
                        let maps = material.map(|m| self.resolve_maps(m)).unwrap_or_default();
                        // Skinned positions/normals + static UVs; per-triangle tangents.
                        let verts = build_textured_verts(
                            start..start + count,
                            |j| skinned[j].position,
                            |j| skinned[j].normal,
                            |j| self.model.mesh.vertices[j].uv,
                        );
                        let h = renderer.upload_textured_mesh(&verts, MeshIndices::U32(&indices));
                        renderer.draw_textured_mesh_pbr(
                            h,
                            th,
                            maps,
                            world,
                            MeshDrawOptions::default(),
                        );
                        self.sub_gpu[si] = Some(SubGpu::Textured(h));
                    }
                    None => {
                        // Untextured: use the material's flat colour if it has one,
                        // else neutral gray.
                        let mat_id = self
                            .model
                            .mesh
                            .materials
                            .get(mat)
                            .filter(|m| m.color.len() >= 3)
                            .map(|m| pack_rgb666(m.color[0], m.color[1], m.color[2]))
                            .unwrap_or(FLAT_GRAY_MATERIAL);
                        let verts: Vec<MeshVertex> = (start..start + count)
                            .map(|j| MeshVertex {
                                position: skinned[j].position,
                                normal: skinned[j].normal,
                                material: mat_id,
                            })
                            .collect();
                        let h = renderer.upload_mesh(&verts, MeshIndices::U32(&indices));
                        renderer.draw_mesh(h, world, MeshDrawOptions::default());
                        self.sub_gpu[si] = Some(SubGpu::Flat(h));
                    }
                }
            }
        }

        // Katana prop: rigid, drawn at the Weapon_R socket's animated global transform
        // (× the world matrix, same as the character). Grip offset is identity for now
        // (tune if the blade sits wrong in the hand).
        if self.katana_equipped && !self.katana_parts.is_empty() {
            if let Some(wb) = self.weapon_bone {
                let katana_model = world * globals[wb];
                for part in &self.katana_parts {
                    match part {
                        PropPart::Textured(h, tex, maps) => {
                            // Suppress the PBR maps when textures are toggled off (matte).
                            let maps = if self.show_textures {
                                *maps
                            } else {
                                PbrMaps::default()
                            };
                            renderer.draw_textured_mesh_pbr(
                                *h,
                                *tex,
                                maps,
                                katana_model,
                                MeshDrawOptions::default(),
                            )
                        }
                        PropPart::Flat(h) => {
                            renderer.draw_mesh(*h, katana_model, MeshDrawOptions::default())
                        }
                    }
                }
            }
        }

        // Skeleton wireframe (Slice 1 view): an overlay when toggled on, or the sole
        // view when the mesh is hidden/absent.
        if self.show_skeleton || !mesh_drawn {
            let segments = self.bone_segments(world, &globals);
            renderer.draw_lines(&segments, [0.35, 0.9, 1.0, 1.0]);
        }

        // ── HUD ──
        let (clip_name, dur) = self
            .model
            .clips
            .get(clip_idx)
            .map(|c| (c.name.as_str(), c.duration_ticks))
            .unwrap_or(("<rest pose>", 0));
        match self.mode {
            ViewMode::Graph => {
                let state_name = self
                    .sm
                    .as_ref()
                    .map(|s| s.current_state_name())
                    .unwrap_or("—");
                renderer.draw_text(
                    &format!("[GRAPH] {state_name}   clip {clip_name}   tick {tick}/{dur}"),
                    Vec2::new(16.0, 16.0),
                    20.0,
                    [1.0, 1.0, 1.0, 1.0],
                );
                renderer.draw_text(
                    "WASD move · Shift run · C crouch · Space jump · F attack · H hit · X die · R reset · G manual · L blend · M/T/B/K/1-3 view · drag/wheel cam · Esc",
                    Vec2::new(16.0, 42.0),
                    14.0,
                    [0.80, 0.86, 0.95, 1.0],
                );
            }
            ViewMode::Manual => {
                let play = if self.playing { "PLAY" } else { "PAUSE" };
                renderer.draw_text(
                    &format!(
                        "[MANUAL] clip [{}/{}] {clip_name}   tick {tick}/{dur}   {play}",
                        self.clip_index + 1,
                        self.model.clips.len(),
                    ),
                    Vec2::new(16.0, 16.0),
                    20.0,
                    [1.0, 1.0, 1.0, 1.0],
                );
                renderer.draw_text(
                    "Space play/pause · <-/-> step · Up/Down clip · G graph · PgUp/PgDn raise/lower · M/T/B/K/1-3 view · R reset · drag/wheel cam · Esc",
                    Vec2::new(16.0, 42.0),
                    14.0,
                    [0.80, 0.86, 0.95, 1.0],
                );
            }
        }
        renderer.draw_text(
            &format!(
                "{} bones · {} verts · {} submeshes · mesh {} · tex {} · skin Color_{} · skeleton {} · weapon {}",
                self.model.bones.len(),
                self.model.mesh.vertices.len(),
                self.sub.len(),
                if self.show_mesh { "on" } else { "off" },
                if self.show_textures { "on" } else { "off" },
                self.skin + 1,
                if self.show_skeleton { "on" } else { "off" },
                if self.katana_equipped { "on" } else { "off" },
            ),
            Vec2::new(16.0, 62.0),
            14.0,
            [0.70, 0.78, 0.90, 1.0],
        );
        // Graph-mode timeline readout: TAE windows open now + recently fired events +
        // the crossfade state (off / on / on with the live weight%).
        if self.mode == ViewMode::Graph {
            let windows = if self.hud_active.is_empty() {
                "—".to_string()
            } else {
                self.hud_active.join("  ")
            };
            let events = self.hud_events.join("  ");
            let blend = match (self.blend_enabled, blend_weight) {
                (false, _) => "off".to_string(),
                (true, Some(w)) => format!("on {:.0}%", w * 100.0),
                (true, None) => "on".to_string(),
            };
            renderer.draw_text(
                &format!("windows: {windows}     events: {events}     blend: {blend}"),
                Vec2::new(16.0, 82.0),
                14.0,
                [0.95, 0.82, 0.55, 1.0],
            );
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "flicker_animation=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    let assets = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));
    let model = format::load_dir(&assets)?;
    tracing::info!(
        "loaded rig: {} bones, {} clips, mesh {} verts (source {}/{}, transform {})",
        model.bones.len(),
        model.clips.len(),
        model.mesh.vertices.len(),
        model.source.source_axis,
        model.source.source_unit,
        model.source.applied_transform,
    );
    for clip in &model.clips {
        if !clip.unresolved.is_empty() {
            tracing::warn!(
                "clip '{}': {} track bone(s) not in rig skeleton (first few: {:?})",
                clip.name,
                clip.unresolved.len(),
                &clip.unresolved[..clip.unresolved.len().min(5)],
            );
        }
    }

    // Katana prop (static mesh) — attached to the Weapon_R socket in the viewer.
    let katana_mesh = match format::load_mesh(&assets.join("Mesh_Katana.json")) {
        Ok(mut m) => {
            // The FBX diffuse ref for this katana points at the Body atlas (kimono
            // florals — wrong). Its "Mat_Hair_Color_1" material actually rides the Hair
            // atlas, same as the character's hair (whose FBX ref was also wrong). Override
            // the albedo AND the whole PBR map set to the Hair atlas so the blade gets the
            // Hair normal/roughness/metalness/AO — the steel metal/rough response.
            for mat in &mut m.materials {
                mat.base_color = "Katanami_Hair_BaseColor.png".to_string();
                mat.normal = "Katanami_Hair_Normal.png".to_string();
                mat.roughness = "Katanami_Hair_Roughness.png".to_string();
                mat.metalness = "Katanami_Hair_Metalness.png".to_string();
                mat.ao = "Katanami_Hair_AO.png".to_string();
            }
            tracing::info!(
                "loaded katana prop: {} verts (albedo + PBR maps overridden to Hair atlas)",
                m.vertices.len()
            );
            Some(m)
        }
        Err(e) => {
            tracing::warn!("katana prop not loaded ({e}); running without weapon");
            None
        }
    };

    run(Viewer::new(model, assets, katana_mesh))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets_dir() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
    }

    fn has_fixtures(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            })
            .unwrap_or(false)
    }

    /// The authoritative-layer validation: the rig loads, and every clip resolves
    /// its tracks against the skeleton. Skips cleanly on a fresh checkout with no
    /// converted fixtures in `assets/`.
    #[test]
    fn rig_loads_and_clips_resolve() {
        let dir = assets_dir();
        if !has_fixtures(&dir) {
            eprintln!("skipping: no .json fixtures in {}", dir.display());
            return;
        }
        let model = format::load_dir(&dir).expect("load rig");
        assert!(model.bones.len() >= 90, "expected ~94 bones, got {}", model.bones.len());
        for clip in &model.clips {
            assert!(!clip.tracks.is_empty(), "clip {} resolved no tracks", clip.name);
            for tr in &clip.tracks {
                assert!(tr.bone < model.bones.len());
            }
        }
    }

    /// The recursive `clips/` adoption: the full structured library loads (not the
    /// flat 13), In-Place clips keep bare stems, RootMotion clips are `RM/…`, and a
    /// same-stem clip in both trees coexists disambiguated.
    #[test]
    fn full_clip_library_loads_with_rm_namespacing() {
        let dir = assets_dir();
        if !has_fixtures(&dir) {
            return;
        }
        let model = format::load_dir(&dir).expect("load rig");
        assert!(
            model.clips.len() >= 80,
            "expected the full structured library (~91), got {}",
            model.clips.len()
        );
        let names: std::collections::HashSet<&str> =
            model.clips.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains("Idle_nonWeapon"), "In-Place bare stem present");
        assert!(names.contains("Attack_3"), "new combo clip present");
        assert!(names.contains("Strafe_Front"), "strafe set present");
        assert!(names.contains("RM/Slide"), "root-motion clip namespaced");
        assert!(names.contains("RM/PickUp"), "root-motion clip namespaced");
        assert!(
            names.contains("Run_nonWeapon") && names.contains("RM/Run_nonWeapon"),
            "the same-stem In-Place and RootMotion clips coexist"
        );
    }

    /// The authored pack still resolves every clip it references against the newly
    /// structured library (it references bare In-Place stems, which are preserved).
    #[test]
    fn pack_resolves_against_the_loaded_library() {
        let dir = assets_dir();
        if !has_fixtures(&dir) {
            return;
        }
        let model = format::load_dir(&dir).expect("load rig");
        let Ok(def) = state::load_pack(&dir.join("Katanami.pack.json")) else {
            return;
        };
        let refs: Vec<state::ClipRef> = model
            .clips
            .iter()
            .map(|c| state::ClipRef {
                name: &c.name,
                duration_ticks: c.duration_ticks,
            })
            .collect();
        let sm = StateMachine::build(&def, &refs).expect("build state machine");
        let unresolved: Vec<&String> = sm
            .warnings()
            .iter()
            .filter(|w| w.contains("unknown clip"))
            .collect();
        assert!(unresolved.is_empty(), "pack references clips missing from the library: {unresolved:?}");
    }

    /// Sampling + forward kinematics never produces NaN/inf global transforms.
    #[test]
    fn pose_sampling_is_finite() {
        let dir = assets_dir();
        let Ok(model) = format::load_dir(&dir) else {
            return;
        };
        let Some(clip) = model.clips.first() else {
            return;
        };
        let dur = clip.duration_ticks.max(1);
        for &tick in &[0, dur / 2, dur.saturating_sub(1)] {
            let locals = pose::sample_local_poses(&model.bones, clip, tick);
            let globals = pose::global_transforms(&model.bones, &locals);
            assert_eq!(globals.len(), model.bones.len());
            for g in &globals {
                assert!(
                    g.w_axis.truncate().is_finite(),
                    "non-finite global translation at tick {tick}"
                );
            }
        }
    }

    /// At the rest/bind pose, CPU skinning must REPRODUCE the original mesh
    /// (`global × inverse_bind` collapses to the mesh bind transform). This is the
    /// guard that catches a wrong bind-matrix convention — a transposed decode
    /// leaves values finite but blows the mesh up, so a finiteness-only check
    /// wouldn't catch it; comparing to the bind mesh does.
    #[test]
    fn skinning_rest_matches_bind() {
        let dir = assets_dir();
        let Ok(model) = format::load_dir(&dir) else {
            return;
        };
        if model.mesh.vertices.is_empty() {
            return;
        }
        let rest: Vec<Mat4> = model.bones.iter().map(|b| b.local).collect();
        let globals = pose::global_transforms(&model.bones, &rest);
        let palette = skin::palette(&model.bones, &globals);
        let verts = skin::skin(&model.mesh, &palette);
        assert_eq!(verts.len(), model.mesh.vertices.len());
        let mut max_err = 0.0f32;
        for (sv, ov) in verts.iter().zip(&model.mesh.vertices) {
            assert!(sv.position.iter().all(|c| c.is_finite()));
            let d = ((sv.position[0] - ov.p[0]).powi(2)
                + (sv.position[1] - ov.p[1]).powi(2)
                + (sv.position[2] - ov.p[2]).powi(2))
            .sqrt();
            max_err = max_err.max(d);
        }
        assert!(
            max_err < 1.0,
            "rest-pose skinning must match the bind mesh; max err {max_err} (cm) — \
             likely a bind-matrix convention/decode bug"
        );
    }
}
