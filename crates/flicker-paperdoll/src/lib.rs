//! flicker-paperdoll — the skeletal-animation "paperdoll" viewer (a character +
//! rig + clip library, dressed up and played back). Promoted from the old
//! `examples/flicker-animation` POC into Alpha as a flicker-shell client.
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
//! ↑/↓ = cycle clip · P = clip previewer (loops a raw clip; ↑/↓ select) · PgUp/PgDn = raise/lower ·
//! M = mesh · T = textures · B = skeleton (cyan joints + orange bone-frame axes) · R = reset · Esc = quit.
//!
//! A rigging-POC PACKAGE — library only, no binary; the scene runs inside the unified
//! `prism-alpha` launcher (`cargo run -p prism-alpha`). The user runs the window and
//! verifies; this crate never launches it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use flicker_input_core::{AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState, Key};
use flicker::render::{
    build_textured_verts, Camera, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, PbrMaps,
    QuadGrid, Renderer, SceneLighting, TextureHandle, TexturedMeshHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, Transition};
use flicker::script::{ComponentLibrary, HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    load_styles, load_ui_json, render_hud, run_ui_with, Surface, Surfaces, UiInput, UiIntents,
    UiState, WalkerHandler, UI_COMPONENT_MODULES,
};
// The HUD renders through `run_ui_with` (Lua component dispatch); `run_ui` and the
// strings module (fixture tokens + the Model-channel gate scan) are test-only.
#[cfg(test)]
use flicker::ui::{run_ui, strings};
use glam::Mat4;

use flicker_input_core::{ActionSignal, Fired, Resolver};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_skeletal::state::{Inputs, StateMachine};
use flicker_skeletal::{cloth, format, jiggle, pose, skin, state};
use flicker_mechanics as mechanics;

mod route;
use route::{GameplayBase, RootHandler};

/// The pendant hangs from the neck on a jiggle chain (secondary motion) instead of riding
/// rigidly. Rig-local cm, z-up, forward = −Y (the toes point −Y). The drape leans forward +
/// down so the medallion settles in FRONT of the chest, not straight down into it.
const NECKLACE_SEGMENTS: usize = 5;
/// Neck → upper-sternum-front, rig-local: forward (−Y) and down (−Z). Tuned so the medallion
/// TOP settles ≈ (0, −12, 134) — upper sternum, centred — with the medallion hanging below.
/// The neck itself never rotates in the clips; the swing is spine-sway carrying its position
/// (≈15° in Walk, ≈1° in Idle) — so it visibly swings while WALKING, near-static in Idle/pose.
const NECKLACE_DRAPE: Vec3 = Vec3::new(0.0, -13.0, -8.0);
/// Medallion size at the chain tip (its untouched fit is body-scale; jewellery is ~cm).
const NECKLACE_PENDANT_CM: f32 = 11.0;
const NECKLACE_CORD_COLOR: [f32; 4] = [0.16, 0.14, 0.11, 1.0];

/// Jiggle dials tuned for a light pendant cord: lighter-than-real gravity so the drape holds
/// its forward lean, moderate stiffness to keep the shape, brisk damping so it settles.
fn necklace_params() -> jiggle::JiggleParams {
    jiggle::JiggleParams {
        gravity: Vec3::new(0.0, 0.0, -450.0),
        // Looser than the default so the spine-sway anchor motion swings the tip visibly.
        stiffness: 0.2,
        damping: 0.9,
        ..Default::default()
    }
}

/// The longest bounding-box axis of a mesh (cm) — for sizing a piece from its own extent.
fn mesh_longest_axis(mesh: &format::Mesh) -> f32 {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for v in &mesh.vertices {
        for i in 0..3 {
            lo[i] = lo[i].min(v.p[i]);
            hi[i] = hi[i].max(v.p[i]);
        }
    }
    (0..3).map(|i| hi[i] - lo[i]).fold(0.0_f32, f32::max)
}

/// Draw a prop's uploaded parts at `model` — shared by the rigid-prop loop and the necklace
/// pendant, which draw the SAME parts at different transforms.
fn draw_prop_parts(renderer: &mut Renderer, parts: &[PropPart], model: Mat4, show_textures: bool) {
    for part in parts {
        match part {
            PropPart::Textured(h, tex, maps) => {
                let maps = if show_textures { *maps } else { PbrMaps::default() };
                renderer.draw_textured_mesh_pbr(*h, *tex, maps, model, MeshDrawOptions::default());
            }
            PropPart::Flat(h) => renderer.draw_mesh(*h, model, MeshDrawOptions::default()),
        }
    }
}

use format::Model;

/// This client's in-scene HUD script (behaviour) and the shared UI-element library
/// (layout, the `UI.paperdoll` section) — the same split every flicker client uses.
const HUD_SCRIPT_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Alpha/content/sensorium/scripts/hud_paperdoll.lua"
);
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/sensorium/resources/ui_elements.json");

/// A submesh's GPU upload for the current frame — textured (albedo) or flat (gray).
enum SubGpu {
    Textured(TexturedMeshHandle),
    Flat(MeshHandle),
}

/// A base-body submesh uploaded for the editor grid: a textured draw (albedo + PBR maps) or a
/// flat-coloured draw. Built once per frame, drawn into all four RTT panels, then freed.
enum PanelSub {
    Textured { h: TexturedMeshHandle, tex: TextureHandle, maps: PbrMaps },
    Flat(MeshHandle),
}

/// Scene lighting for the editor panels — a warm key + cool fill over a soft ambient, so the
/// character reads with form (the renderer's default is flatter). Mirrors the packeditor portrait.
fn pip_scene() -> SceneLighting {
    SceneLighting {
        sun_dir: Vec3::new(0.4, 0.8, 0.5).normalize(),
        sun_color: Vec3::splat(0.85),
        moon_dir: Vec3::new(-0.5, 0.3, -0.4).normalize(),
        moon_color: Vec3::new(0.20, 0.22, 0.30),
        ambient: Vec3::splat(0.35),
        ..SceneLighting::default()
    }
}

/// Resolve a material's PBR map names against a texture table. Free-standing so the prop
/// upload can call it while `Viewer::textures` is moved out for the borrow.
fn resolve_maps_in(textures: &HashMap<String, TextureHandle>, mat: &format::Material) -> PbrMaps {
    let get = |name: &str| {
        if name.is_empty() {
            None
        } else {
            textures.get(name).copied()
        }
    };
    PbrMaps {
        normal: get(&mat.normal),
        roughness: get(&mat.roughness),
        metalness: get(&mat.metalness),
        ao: get(&mat.ao),
        // Self-illumination. `format::Material` has carried `emit` since the
        // content standard added it; the renderer only gained the binding now, so
        // this is the last link — a character material naming an Emit map glows.
        emit: get(&mat.emit),
    }
}

/// A static prop submesh, uploaded once — textured (albedo + PBR maps) or flat (colour).
enum PropPart {
    Textured(TexturedMeshHandle, TextureHandle, PbrMaps),
    Flat(MeshHandle),
}

/// How a slot's piece rides the body.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// A skinned garment sharing the base skeleton — drawn over the body by the same
    /// per-frame pose palette (`format::load_outfit` remaps its joints by NAME).
    ///
    /// **Currently unconstructed, deliberately.** Every piece is `Prop` during the fit phase
    /// because the vendor ships them unrigged (`skeleton.bones = []`), and this path would
    /// bind such a mesh wholesale to `root`. It is the path a garment RETURNS to once it has
    /// been bound to the armature using its recorded fit — produced by tools/skin_outfit.py,
    /// which bakes the fit into the mesh and transfers the body's weights. Live for the six
    /// garments (chest/legs/gloves/boots).
    Garment,
    /// A rigid prop parented to one socket bone, drawn at that bone's animated global
    /// transform. The named bone must exist in the canonical rig.
    Prop(&'static str),
}

/// One equip slot in the inventory bar. `file` resolves under the shared content tree
/// (`Alpha/content/package/characters/<dir>/<file>`); a slot whose file is absent stays EMPTY —
/// its cell draws dim and inert — so the bar shows the full slot set before the art
/// lands and each piece lights up as it arrives, with no code change.
struct SlotDef {
    /// The Model/result key shared with `hud_paperdoll.lua` (`slot_<key>`).
    key: &'static str,
    dir: &'static str,
    file: &'static str,
    fit: Fit,
    /// The opposite half of a mirrored pair, if any. Adjusting either half applies the
    /// MIRRORED fit to the other (see `mirror_fit`) — the two are the same garment, so
    /// fitting each independently is duplicated work that also drifts out of symmetry.
    mirror: Option<&'static str>,
}

/// The equip slots, in bar order. Sheaths are deliberately absent: they have no cell
/// because they are always worn (there are no sheath/unsheath animations yet) — see
/// `SHEATHS`.
///
/// **Everything is `Fit::Prop` during the fit phase**, garments included. They arrive from
/// the vendor UNRIGGED (`skeleton.bones = []`), so `Fit::Garment`/`load_outfit` would bind
/// them wholesale to `root` — rigid, ignoring the body. Attaching each to a bone is what
/// lets it be eyeballed into place first; binding comes after the fit is recorded.
///
/// The bone is the "piece we think it's attached to" (user, 2026-07-16) — a starting guess,
/// not a claim. **Boots and gloves are symmetric PAIRS in one mesh**, so no single limb bone
/// is right for them; they hang off a central bone until they're split or skinned.
const SLOTS: &[SlotDef] = &[
    // Hair rides the head as a rigid prop (fittable via the gadget); like cloth it will want
    // a jiggle chain — a swinging ponytail — later.
    SlotDef { key: "hair", dir: "musefit", file: "Hair-Ponytail.json", fit: Fit::Prop("head") , mirror: None },
    SlotDef { key: "neck", dir: "musefit", file: "Neck-Pendant.json", fit: Fit::Prop("neck_01") , mirror: None },
    SlotDef { key: "chest", dir: "musefit", file: "Corset-Duster.skinned.json", fit: Fit::Garment, mirror: None },
    SlotDef { key: "legs", dir: "musefit", file: "Hem-Pants.skinned.json", fit: Fit::Garment, mirror: None },
    // Gloves and boots ship as symmetric PAIRS in one mesh; the converter splits them by
    // loose parts (`split: "lr"` in the manifest) so each half can follow its own limb. An
    // unsplit pair is unwearable at any transform — one bone cannot drive two feet.
    SlotDef { key: "glove_l", dir: "musefit", file: "Hand-Gloves-L.skinned.json", fit: Fit::Garment, mirror: None },
    SlotDef { key: "glove_r", dir: "musefit", file: "Hand-Gloves-R.skinned.json", fit: Fit::Garment, mirror: None },
    SlotDef { key: "boot_l", dir: "musefit", file: "Foot-Boots-L.skinned.json", fit: Fit::Garment, mirror: None },
    SlotDef { key: "boot_r", dir: "musefit", file: "Foot-Boots-R.skinned.json", fit: Fit::Garment, mirror: None },
    SlotDef { key: "rhand", dir: "musefit", file: "Katana.json", fit: Fit::Prop("Weapon_R") , mirror: None },
    SlotDef { key: "lhand", dir: "musefit", file: "Dagger.json", fit: Fit::Prop("Weapon_L") , mirror: None },
];

/// A piece's placement onto the body: uniform scale + an offset in its socket bone's frame.
/// This is the AUTHORED data the vendor export cannot supply — every asset arrives
/// normalised to a ~1.899 longest axis about the origin, so a katana and a pendant are the
/// same size and nothing about real scale/position survives. Eyeballed in-window, then
/// recorded to content.
#[derive(Clone, Copy)]
struct PieceFit {
    /// Overall size. The effective scale is `uniform * scale`, so this sets how big the
    /// piece IS while `scale` stays a multiplier around 1.0 — resizing the whole piece
    /// doesn't disturb a stretch you already dialled in, and vice versa.
    uniform: f32,
    /// PER-AXIS scale, in the piece's own axes — which are WORLD axes at rest, because
    /// `rot` cancels the socket's rest rotation. So x thickens laterally, y through the
    /// body's depth, z lengthens vertically, whichever bone it hangs off. Non-uniform by
    /// design: a garment generated separately from the body is rarely the same build, so
    /// one uniform scale can line it up but never make it fit (user, 2026-07-16: "the base
    /// model is thicker than this outfit, if I can thicken it, that would be perfect").
    scale: Vec3,
    offset: Vec3,
    /// USER rotation as a QUATERNION, accumulated from ±X/±Y/±Z nudges — about WORLD axes at
    /// rest, because `rot` has already cancelled the socket's rest tilt. A quaternion, not Euler
    /// angles, so nudging past 90° never gimbal-folds and a reset is an exact IDENTITY (D.6).
    /// This is what aims a weapon down the hand: the vendor's blade runs along its own +X with no
    /// relation to the grip (user, 2026-07-16: "the sword and dagger are both pointed
    /// straight into the hands"). Separate from `rot` so a rig fix can still re-derive the
    /// upright correction without discarding an authored aim.
    user_rot: glam::Quat,
    /// Rotation applied INSIDE the socket's frame. Defaults to the inverse of the bone's
    /// REST rotation, which is what keeps a piece upright: pieces are authored world-upright
    /// (z-up), but a canonical bone's frame is not — UE bones run X-down-bone — so parenting
    /// a piece straight to one tips it 90° (observed on every piece except the boots, which
    /// hang off the world-aligned `root`). Cancelling only the REST rotation leaves the piece
    /// still rotating WITH the bone under animation, which is what a worn item must do.
    rot: glam::Quat,
}

/// How long the Record confirmation stays on screen.
const RECORD_NOTE_SECS: f32 = 4.0;

/// The piece's longest axis lands at this fraction of the body's height. Purely a STARTING
/// size so the piece is visible and clickable — it is not a claim about the real object
/// (0.5 × 170cm ≈ 85cm reads about right for a katana or a duster, and far too big for a
/// boot; that is what the fit gadget is for).
const DEFAULT_FIT_FRACTION: f32 = 0.5;

impl PieceFit {
    /// Default placement: centred on its socket bone, upright, scaled so the piece's longest
    /// axis is `DEFAULT_FIT_FRACTION` of body height. Inferred from the ~1.899 normalisation
    /// — the one consistent number the vendor gives us (user's call, 2026-07-16).
    ///
    /// `socket` is the bone the piece hangs off, if any; its rest rotation is cancelled so
    /// the piece starts world-upright (see `rot`).
    fn default_for(mesh: &format::Mesh, body_height: f32, socket: Option<&format::Bone>) -> Self {
        let longest = mesh
            .vertices
            .iter()
            .fold([f32::MAX; 3], |mn, v| std::array::from_fn(|i| mn[i].min(v.p[i])))
            .iter()
            .zip(mesh.vertices.iter().fold([f32::MIN; 3], |mx, v| {
                std::array::from_fn(|i| mx[i].max(v.p[i]))
            }))
            .map(|(mn, mx)| mx - mn)
            .fold(0.0_f32, f32::max);
        let s = if longest > 1e-6 {
            body_height * DEFAULT_FIT_FRACTION / longest
        } else {
            1.0
        };
        // The inferred default is the OVERALL size; per-axis starts neutral at 1.0.
        let uniform = s;
        let scale = Vec3::ONE;
        // `inverse_bind` IS the inverse of the bone's rest world matrix, so its rotation part
        // is exactly the inverse rest rotation we want — no need to re-derive it.
        let rot = socket
            .map(|b| glam::Quat::from_mat4(&b.inverse_bind).normalize())
            .unwrap_or(glam::Quat::IDENTITY);
        Self { uniform, scale, offset: Vec3::ZERO, user_rot: glam::Quat::IDENTITY, rot }
    }

    /// This fit REFLECTED across the body's midline (x = 0) — the fit its mirrored partner
    /// should wear. Exact because `rot` puts everything in world axes: `offset.x` negates,
    /// scale is untouched (a reflection does not resize), and rotation keeps PITCH while YAW
    /// and ROLL negate (reflection reverses handedness, so the component about the mirror
    /// axis survives and the other two invert).
    ///
    /// `rot` is carried through unchanged but callers must NOT copy it — it belongs to the
    /// partner's own socket and is re-derived from the rig.
    fn mirrored(&self) -> Self {
        Self {
            uniform: self.uniform,
            scale: self.scale,
            offset: Vec3::new(-self.offset.x, self.offset.y, self.offset.z),
            // Reflect the quaternion across x = 0: keep PITCH (x) and w, negate YAW (y) and
            // ROLL (z). Derived from axis-angle: the mirror sends axis a→(ax,−ay,−az), angle→−θ.
            user_rot: glam::Quat::from_xyzw(self.user_rot.x, -self.user_rot.y, -self.user_rot.z, self.user_rot.w),
            rot: self.rot,
        }
    }

    /// `rot` first (cancelling the socket's rest tilt), then the piece's own scale/offset.
    ///
    /// Because `rot` cancels the bone's rest rotation, `offset` ends up along **world axes at
    /// rest** — x lateral, y depth, z up — not bone axes. That is deliberate: the torso bones
    /// are tilted 90°, so a bone-space offset would be unusable by eye. Nudging +x moves the
    /// piece to the body's left regardless of which bone it hangs off.
    fn matrix(&self) -> Mat4 {
        Mat4::from_quat(self.rot)
            * Mat4::from_scale_rotation_translation(self.scale * self.uniform, self.user_rot, self.offset)
    }
}

/// The recorded fits, keyed by ASSET name (`Corset-Duster`) — the fit belongs to the piece,
/// not to whichever slot happens to hold it. Written next to the pieces so it travels with
/// them, and read back at startup to override the inferred default.
const FITS_FILE: &str = "fits.json";

/// The gadget's rows `(key, group)` from the shared UI library. Empty on a bad/missing
/// file — the gadget then simply has no keyboard navigation rather than the viewer failing.
fn load_fit_rows() -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(HUD_UI_ELEMENTS) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v["paperdoll"]["fit"]["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    Some((r["key"].as_str()?.to_string(), r["group"].as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read `<chars>/<dir>/fits.json`. Absent/garbled → empty, so a body with no recorded fits
/// simply falls back to defaults rather than failing to load.
/// One recorded placement as it appears in `fits.json`. Plain arrays (not `Vec3`) because
/// this IS the file's shape; `PieceFit` is the runtime form.
#[derive(Clone, Copy, Default)]
struct RecordedFit {
    uniform: f32,
    scale: [f32; 3],
    offset: [f32; 3],
    rotate: glam::Quat,
}

impl RecordedFit {
    /// Build from a piece's INLINE `attach` section — the folded-in fit (WS-C C-γ). `None` when
    /// the section is empty (no `socket` recorded), so the caller falls back to the legacy shared
    /// `fits.json` sidecar. The upright socket correction is never carried here (re-derived from
    /// the rig at load), exactly like the sidecar path.
    fn from_attach(a: &format::Attach) -> Option<Self> {
        if a.socket.is_empty() {
            return None;
        }
        Some(Self {
            uniform: a.uniform,
            scale: a.scale,
            offset: a.offset,
            rotate: glam::Quat::from_xyzw(a.rotate[0], a.rotate[1], a.rotate[2], a.rotate[3]).normalize(),
        })
    }
}

/// Insert-or-replace ONLY the top-level `attach` section in a piece's own self-describing file
/// (WS-C C-γ — the fold of the shared `fits.json`). A read-modify-write that preserves everything
/// else (the piece's multi-MB mesh); a compact write keeps these single-line files a clean 1-line
/// diff instead of pretty-printing the whole mesh. Factored out of `record_fits` so the mesh-
/// preservation guarantee is unit-testable before the Record button mutates real content.
fn write_inline_attach(path: &Path, attach: serde_json::Value) -> Result<()> {
    // Gz-transparent read-modify-write (flicker-core::compression): the piece is
    // addressed by its LOGICAL path and re-emitted in its gz-at-rest form.
    let text = flicker_core::compression::read_text(path)
        .with_context(|| format!("reading piece {}", path.display()))?;
    let mut doc: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing piece {}", path.display()))?;
    doc.as_object_mut()
        .with_context(|| format!("piece {} is not a JSON object", path.display()))?
        .insert("attach".to_string(), attach);
    flicker_core::compression::write_text(path, &serde_json::to_string(&doc)?)
        .with_context(|| format!("writing piece {}", path.display()))?;
    Ok(())
}

fn load_fits(path: &Path) -> HashMap<String, RecordedFit> {
    let Ok(text) = flicker_core::compression::read_text(path) else {
        return HashMap::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        tracing::warn!("fits {} is not valid JSON; using defaults", path.display());
        return HashMap::new();
    };
    let mut out = HashMap::new();
    if let Some(map) = v.get("fits").and_then(|f| f.as_object()) {
        for (k, f) in map {
            let arr = |v: Option<&serde_json::Value>, dflt: f32| -> [f32; 3] {
                match v {
                    // A bare number = a uniform scale from an older record: splat it rather
                    // than silently dropping the fit.
                    Some(serde_json::Value::Number(n)) => {
                        [n.as_f64().unwrap_or(dflt as f64) as f32; 3]
                    }
                    Some(serde_json::Value::Array(a)) => std::array::from_fn(|i| {
                        a.get(i).and_then(|x| x.as_f64()).unwrap_or(dflt as f64) as f32
                    }),
                    _ => [dflt; 3],
                }
            };
            // Rotation: 4 floats = the quaternion we write now (D.6); 3 floats = a LEGACY
            // Euler-XYZ-degrees record (pre-D.6) → convert once so old fits keep their aim;
            // absent/garbled → IDENTITY.
            let rot = |v: Option<&serde_json::Value>| -> glam::Quat {
                match v.and_then(|x| x.as_array()) {
                    Some(a) if a.len() >= 4 => {
                        let g = |i: usize| a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                        glam::Quat::from_xyzw(g(0), g(1), g(2), g(3)).normalize()
                    }
                    Some(a) if a.len() == 3 => {
                        let g = |i: usize| {
                            (a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32).to_radians()
                        };
                        glam::Quat::from_euler(glam::EulerRot::XYZ, g(0), g(1), g(2))
                    }
                    _ => glam::Quat::IDENTITY,
                }
            };
            // `uniform` absent = 1.0: an older record stored the overall size in `scale`,
            // and `uniform * scale` reproduces it exactly.
            let uniform = f.get("uniform").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32;
            out.insert(
                k.clone(),
                RecordedFit {
                    uniform,
                    scale: arr(f.get("scale"), 1.0),
                    offset: arr(f.get("offset"), 0.0),
                    rotate: rot(f.get("rotate")),
                },
            );
        }
    }
    out
}

/// Always-worn props with no inventory cell (user, 2026-07-16: "the sheathes will
/// remain where they are"). Same loading path as a slot's prop; simply never toggled.
const SHEATHS: &[SlotDef] = &[
    SlotDef { key: "katana_sheath", dir: "musefit", file: "Katana-Sheath.json", fit: Fit::Prop("pelvis") , mirror: None },
    SlotDef { key: "dagger_sheath", dir: "musefit", file: "Dagger-Sheath.json", fit: Fit::Prop("pelvis") , mirror: None },
];

/// A slot's loaded piece. `None` on `Slot::content` = nothing to wear (dim, inert cell).
enum SlotContent {
    Garment(OutfitLayer),
    /// A rigid item at a socket bone. The mesh is RETAINED after the GPU upload (it is not
    /// taken) because click-to-select ray-casts against its triangles — brute force at click
    /// rate, the same tradeoff `flicker-pocclusters` makes. `uploaded` guards the one-shot
    /// GPU upload now that the mesh's presence no longer can.
    Prop { mesh: format::Mesh, parts: Vec<PropPart>, bone: usize, uploaded: bool },
}

/// One runtime slot: its definition, whatever loaded for it, whether it is worn, and its
/// placement onto the body.
struct Slot {
    def: &'static SlotDef,
    content: Option<SlotContent>,
    on: bool,
    fit: PieceFit,
}

impl Slot {
    /// Load a slot's piece. A missing file (the normal case until the art arrives) or a
    /// socket the rig lacks leaves the slot empty rather than failing the viewer.
    fn load(
        def: &'static SlotDef,
        chars: &Path,
        bones: &[format::Bone],
        on: bool,
        body_height: f32,
        fits: &HashMap<String, RecordedFit>,
    ) -> Self {
        let path = chars.join(def.dir).join(def.file);
        // Load the piece AND its inline `attach` mount (WS-C C-γ): a piece carries its own fit in
        // its one self-describing file, so we no longer depend on the shared `fits.json` sidecar.
        let (content, attach) = if !flicker_core::compression::file_exists(&path) {
            tracing::info!("slot '{}': no piece at {} — cell stays empty", def.key, path.display());
            (None, format::Attach::default())
        } else {
            match def.fit {
                Fit::Garment => match format::load_outfit_with_attach(&path, bones) {
                    Ok((mesh, attach)) => {
                        tracing::info!("slot '{}': garment {} verts", def.key, mesh.vertices.len());
                        (Some(SlotContent::Garment(OutfitLayer::new(mesh, bones))), attach)
                    }
                    Err(e) => {
                        tracing::warn!("slot '{}': garment failed ({e}); cell stays empty", def.key);
                        (None, format::Attach::default())
                    }
                },
                Fit::Prop(socket) => match bones.iter().position(|b| b.name == socket) {
                    None => {
                        tracing::warn!("slot '{}': rig has no '{socket}' socket; cell stays empty", def.key);
                        (None, format::Attach::default())
                    }
                    Some(bone) => match format::load_mesh_with_attach(&path) {
                        Ok((mesh, attach)) => {
                            tracing::info!("slot '{}': prop {} verts @ {socket}", def.key, mesh.vertices.len());
                            (Some(SlotContent::Prop { mesh, parts: Vec::new(), bone, uploaded: false }), attach)
                        }
                        Err(e) => {
                            tracing::warn!("slot '{}': prop failed ({e}); cell stays empty", def.key);
                            (None, format::Attach::default())
                        }
                    },
                },
            }
        };
        let on = on && content.is_some();
        let mut fit = match &content {
            Some(SlotContent::Garment(g)) => PieceFit::default_for(&g.mesh, body_height, None),
            Some(SlotContent::Prop { mesh: m, bone, .. }) => {
                PieceFit::default_for(m, body_height, bones.get(*bone))
            }
            _ => PieceFit {
                uniform: 1.0,
                scale: Vec3::ONE,
                offset: Vec3::ZERO,
                user_rot: glam::Quat::IDENTITY,
                rot: glam::Quat::IDENTITY,
            },
        };
        // A RECORDED fit overrides the inferred default — the whole point of recording. Prefer the
        // piece's INLINE `attach` (WS-C C-γ: the fit folded into its own self-describing file); fall
        // back to the legacy shared `musefit/fits.json` sidecar for pieces not yet re-recorded. The
        // upright correction is always re-derived from the rig, never stored: it is a property of
        // the socket, not of the piece, so a rig change must not be overridden by a stale file.
        let asset = def.file.trim_end_matches(".json");
        let recorded = RecordedFit::from_attach(&attach).or_else(|| fits.get(asset).copied());
        if let Some(r) = recorded {
            fit.uniform = r.uniform;
            fit.scale = Vec3::from(r.scale);
            fit.offset = Vec3::from(r.offset);
            fit.user_rot = r.rotate;
            let src = if attach.socket.is_empty() { "fits.json" } else { "inline attach" };
            tracing::info!(
                "slot '{}': recorded fit ({src}) — size {:.2}, stretch {:?}, offset {:?}, rot {:?}",
                def.key, r.uniform, r.scale, r.offset, r.rotate
            );
        } else if content.is_some() {
            tracing::info!("slot '{}': default fit — size {:.2} (no recorded fit)", def.key, fit.uniform);
        }
        Self { def, content, on, fit }
    }

    fn loaded(&self) -> bool {
        self.content.is_some()
    }
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

/// Effective submeshes `(material, start, count)` for a mesh — the JSON's material
/// groups, or one whole-mesh fallback (`usize::MAX` → no material → renders flat).
/// Index ranges are also vertex ranges (the converter emits sequential indices).
fn submeshes_of(mesh: &format::Mesh) -> Vec<(usize, usize, usize)> {
    if !mesh.submeshes.is_empty() {
        mesh.submeshes
            .iter()
            .map(|s| (s.material, s.start, s.count))
            .collect()
    } else if !mesh.vertices.is_empty() {
        vec![(usize::MAX, 0, mesh.indices.len())]
    } else {
        Vec::new()
    }
}

/// A clothing outfit drawn OVER the base body. It's a partial skinned mesh that shares
/// the base skeleton (its joints were remapped into base-index space at load, see
/// `format::load_outfit`), so it skins with the SAME per-frame palette as the base and
/// registers perfectly. Its own submeshes + per-frame GPU buffers, independent of the
/// base body's.
struct OutfitLayer {
    mesh: format::Mesh,
    sub: Vec<(usize, usize, usize)>,
    sub_gpu: Vec<Option<SubGpu>>,
    /// Dynamic secondary motion for this garment's dangly regions (sleeves/hem). Built from
    /// `mesh.cloth`; empty (a no-op) when the garment carries no cloth metadata.
    cloth: cloth::ClothSim,
}

impl OutfitLayer {
    fn new(mesh: format::Mesh, bones: &[format::Bone]) -> Self {
        let sub = submeshes_of(&mesh);
        let sub_gpu = (0..sub.len()).map(|_| None).collect();
        let cloth = cloth::ClothSim::build(&mesh.cloth, &mesh.vertices, bones);
        Self { mesh, sub, sub_gpu, cloth }
    }
}

struct Viewer {
    model: Model,
    cam: OrbitCam,
    /// The pack's runtime state machine (drives clip + tick + crossfade when animating).
    /// `None` = no `.pack` loaded, in which case Animate has nothing to play.
    sm: Option<StateMachine>,
    /// The pack's state names in authored order, for cycling the viewed animation with
    /// ↑/↓ (`force_state_by_name`). Read from the pack def at load — no graph layout needed.
    sm_states: Vec<String>,
    /// Which `sm_states` entry ↑/↓ last selected, so the cycle is stable across frames.
    state_idx: usize,
    /// Set by the HUD's Attack button for one frame, fed to the state machine as
    /// `Inputs.attack` (→ Attack_1). Movement is read live from WASD/Shift/C/Space each
    /// frame and needs no field.
    attack_pending: bool,
    /// The pendant's jiggle chain — the first user of the runtime solver. Built lazily from
    /// the neck's pose when the neck slot is worn, dropped when it isn't. `necklace_bone` is
    /// the `neck_01` index it hangs from; `necklace_pendant_scale` sizes the medallion at the
    /// tip. `frame_dt` carries the last update's dt into `render`, where the chain is stepped
    /// (the shell runs update→render 1:1).
    necklace: Option<jiggle::JiggleChain>,
    necklace_bone: Option<usize>,
    necklace_pendant_scale: f32,
    /// The neck's REST world rotation. The drape (and the medallion's facing) are defined at
    /// bind; each frame the driver twist = `current_neck_rot × inv(rest)` rotates them, so the
    /// necklace follows the upper body when the shoulders/spine twist (else a world-fixed drape
    /// leaves the pendant off-centre on a turned chest — user, 2026-07-17).
    necklace_neck_rest_rot: glam::Quat,
    frame_dt: f32,
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
    /// Debug overlay: draw the auto-fit collision volumes (WS-D). Toggled from the HUD.
    show_collision: bool,
    /// Auto-fit collision volume per bone (WS-D D4b) — capsules in each bone's LOCAL frame,
    /// posed by the current `globals` for the overlay + (later) combat queries.
    collision_volumes: Vec<mechanics::Volume>,
    /// The "whole-unit" character pill: ONE upright capsule bounding the body, for simple non-IK
    /// terrain/movement collision. Computed once from the bind mesh (feet→head), fixed in the
    /// character's root frame (it does not deform with the pose), drawn in the overlay.
    body_pill: mechanics::Shape,
    /// The bone the user clicked to select in the rig view (index into `model.bones`), for the
    /// joint editor (Slice 2). Highlighted amber in the skeleton overlay + read out in the HUD stats.
    selected_bone: Option<usize>,
    /// The 2×2 editor viewport (Perspective TL, Top TR, Side BL, Front BR) — the shared
    /// `flicker-render` grid, which owns the offscreen targets, the per-view cameras and the
    /// composite. `None` until `setup` runs, which needs the renderer to create the targets.
    grid: Option<QuadGrid>,
    /// The Animation/Pose toggle (Lua). `true` = the state machine drives the pose (real
    /// runtime animation, blended); `false` = A-pose (bind), the fitting reference. Idle is
    /// an authored stance, so bind — not Idle — is the honest pose to line a piece up in.
    animate: bool,
    /// Apply transition crossfades while animating (the "Blend" toggle — A/B against a
    /// hard cut). The state machine tracks the blend regardless; this only decides whether
    /// the viewer interpolates the outgoing pose.
    blend_enabled: bool,
    /// The slot the fit gadget is editing (index into `slots`); set by clicking a piece in
    /// the scene, cleared by clicking empty space.
    editing: Option<usize>,
    /// Which fit row ←/→ nudges, set by clicking that row. Empty = none focused. The
    /// KEYBOARD never reaches the script, so the engine owns the nudge and ↑/↓ navigation;
    /// the script only reports which row was clicked.
    fit_focus: String,
    /// Transient confirmation of the last Record, and how long it stays up. Writing to disk
    /// is otherwise invisible from inside the window — the log line is behind the app.
    record_note: String,
    record_note_t: f32,
    /// The gadget's rows as `(key, group)`, read once from `UI.paperdoll.fit.rows` — the
    /// SAME list `hud.lua` lays out, so panel order and keyboard navigation cannot drift.
    fit_rows: Vec<(String, String)>,
    /// Content-tree root (`.../characters`), for writing `musefit/fits.json` on Record.
    chars: PathBuf,
    /// Last frame's globals, so a click can pick against the pose the user actually SAW.
    /// `update` runs before `render`, so without this a pick would test the previous pose's
    /// geometry against this frame's cursor.
    last_globals: Vec<Mat4>,
    /// Last frame's world matrix, paired with `last_globals`.
    last_world: Mat4,
    /// The equip slots (`SLOTS`), in inventory-bar order — each owns its loaded piece and
    /// its worn flag. A garment is a partial skinned mesh sharing the base skeleton (its
    /// joints remapped by NAME at load, so it skins with the SAME per-frame palette); a
    /// prop is rigid, drawn at its socket bone's animated transform. Slots whose file is
    /// absent stay empty and their cell draws dim + inert.
    slots: Vec<Slot>,
    /// Always-worn props with no inventory cell (`SHEATHS`).
    sheaths: Vec<Slot>,
    /// The Lua HUD (`hud_paperdoll.lua` + `UI.paperdoll`): inventory bar, view toggles, and
    /// the stat readout. Replaces the POC keypress toggles + `draw_text` overlay.
    script: Option<ScriptHost>,
    /// The HUD's component tree, parsed ONCE from the script's `tree()` at load
    /// (step 4's lazy build-once — re-pulled only if the screen restructures, which
    /// paperdoll's static HUD never does). The walker runs this every frame.
    ui_tree: Option<UiNode>,
    /// The screen's declarative signal bindings (S9), collected from the parsed
    /// tree's ROOT `on_<signal>` props at the same load point (`on_menu =
    /// "pause_open"`). The walker layer consumes a declared signal; `update`
    /// maps the fired name onto its pause push.
    ui_intents: UiIntents,
    /// Intent names fired last frame — republished ONCE into the next HUD Model
    /// as the transient `sig_<name>` mirror (S9), then dropped.
    fired_sigs: Vec<String>,
    /// Retained walker interaction state (slider drag capture) across frames.
    ui_state: UiState,
    /// The screen's declared surface set (S8): the fit gadget panel, derived from
    /// the piece selection each frame and published under the tree's pre-existing
    /// `fit_active` gate. (`animate` is NOT here — it is a two-way checkbox bind
    /// that some rows also gate on, a control's value, not a managed surface.)
    surfaces: Surfaces,
    /// The resolved `ui_elements.json` the walker resolves node `style` paths against.
    ui_styles: serde_json::Value,
    /// This frame's HUD draw commands — the walker builds them in `update`, `render`
    /// blits them (one walk per frame; the script never sees a renderer handle).
    hud_commands: Vec<HudCommand>,
    /// 1×1 white texture backing `render_hud`'s rect fills.
    hud_white: Option<TextureHandle>,
    // Edge-detection for one-shot INPUT (InputState exposes level state for keys). Only
    // real input lives here now — playback and mode keys. The display toggles moved to
    // the Lua HUD, and the state machine's gameplay drivers read the BUS (S9b): jump is
    // the resolver's Jump edge, so the old `prev_space` latch is gone.
    prev_up: bool,
    prev_down: bool,
    prev_left: bool,
    prev_right: bool,
    prev_r: bool,
    /// DEBUG (D.4 test toggle): the inactive base body — base B (male) when A is shown — loaded
    /// alongside base A so `X` hot-swaps the whole rig+mesh+clips to confirm the male body skins
    /// + animates. `None` if `PrismHumanBaseB` is absent; `body_b` tracks which is live.
    alt_model: Option<Model>,
    body_b: bool,
    prev_x: bool,
    /// Debug clip previewer (`P`): when on, play `model.clips[preview_clip]` ON LOOP, bypassing
    /// the state machine, so a raw retargeted clip (e.g. `walk_forward`) can be watched to
    /// validate the retarget. ↑/↓ select the clip; `preview_time` accumulates real seconds,
    /// wrapped to the clip's tick range at its `tick_rate_hz`. Off = normal Animate/Pose.
    preview: bool,
    preview_clip: usize,
    preview_time: f32,
    prev_p: bool,
    /// Shell pause plumbing: the per-context action maps (World only — the viewer has no
    /// in-world text entry) and the gothic theme built once for the pause overlay we push.
    bindings: ContextualBindings,
    /// Gamepad config for the resolver + `signal_held` polls (default; no per-scene tuning).
    gamepad_config: GamepadConfig,
    /// The input seam (spec §9): a stateful edge [`Resolver`] (replaces the raw `menu_prev`
    /// latch), a REUSED `Fired` scratch buffer (no per-frame alloc — RT-7), the router's
    /// request queue, and a monotonic frame `tick` that is the `Resolver`'s `TickTime`
    /// (NOT wall-clock — spec §3.2a).
    resolver: Resolver,
    ev: Vec<Fired>,
    route: RouteCtx,
    tick: u64,
    ui_theme: Option<Theme>,
}

impl Viewer {
    fn new(
        model: Model,
        alt_model: Option<Model>,
        assets: PathBuf,
        chars: &Path,
        sm: Option<StateMachine>,
        sm_states: Vec<String>,
    ) -> Self {
        let cam = OrbitCam::new(model.orbit_radius);
        let has_mesh = !model.mesh.vertices.is_empty();
        // Body height in the rig's own units — the reference every piece's default fit is
        // sized against (the vendor's ~1.899 normalisation carries no real scale).
        let body_height = {
            let (mn, mx) = model.mesh.vertices.iter().fold((f32::MAX, f32::MIN), |(a, b), v| {
                (a.min(v.p[2]), b.max(v.p[2]))
            });
            if mx > mn { mx - mn } else { 170.0 }
        };
        // Equip slots + the always-worn sheaths, loaded from the content tree. Each slot
        // resolves independently: a piece whose file is absent simply leaves that cell
        // empty, so the bar shows the full slot set and pieces light up as they land.
        // The katana starts worn (it was the POC's default); everything else starts off.
        // Recorded fits travel with the pieces (musefit/fits.json) and override the
        // inferred defaults.
        let fits = load_fits(&chars.join("musefit").join(FITS_FILE));
        if !fits.is_empty() {
            tracing::info!("loaded {} recorded fit(s)", fits.len());
        }
        let slots: Vec<Slot> = SLOTS
            .iter()
            .map(|d| Slot::load(d, chars, &model.bones, d.key == "rhand" || d.key == "hair", body_height, &fits))
            .collect();
        let sheaths: Vec<Slot> = SHEATHS
            .iter()
            .map(|d| Slot::load(d, chars, &model.bones, true, body_height, &fits))
            .collect();
        // Effective submeshes for the base body.
        let sub = submeshes_of(&model.mesh);
        let sub_gpu = (0..sub.len()).map(|_| None).collect();

        // Necklace setup — computed before the struct literal moves `model`/`slots`.
        let necklace_bone = model.bones.iter().position(|b| b.name == "neck_01");
        // Rest world rotation from the neck's inverse_bind (= inv(rest_global)), so no FK.
        let necklace_neck_rest_rot = necklace_bone
            .map(|i| glam::Quat::from_mat4(&model.bones[i].inverse_bind).inverse())
            .unwrap_or(glam::Quat::IDENTITY);
        let necklace_pendant_scale = {
            let longest = slots
                .iter()
                .find(|s| s.def.key == "neck")
                .and_then(|s| match &s.content {
                    Some(SlotContent::Prop { mesh, .. }) => Some(mesh_longest_axis(mesh)),
                    _ => None,
                })
                .unwrap_or(1.9);
            NECKLACE_PENDANT_CM / longest.max(1e-3)
        };

        // Auto-fit a default collision volume per bone (WS-D D4b) — computed before the literal
        // moves `model`. Drives the debug overlay now; the source for combat queries later.
        let collision_volumes = mechanics::autofit_capsules(&model.bones);
        // The whole-unit movement pill: one upright capsule bounding the bind mesh (feet→head), for
        // simple non-IK terrain collision. Fixed in the root frame (computed from the rest mesh, so
        // it never deforms with the pose). 0.14 × height ≈ a humanoid radius.
        let body_pill =
            mechanics::upright_capsule(model.mesh.vertices.iter().map(|v| Vec3::from(v.p)), 0.14);

        Self {
            model,
            cam,
            sm,
            sm_states,
            state_idx: 0,
            attack_pending: false,
            necklace: None,
            necklace_bone,
            necklace_pendant_scale,
            necklace_neck_rest_rot,
            frame_dt: 1.0 / 60.0,
            world_y: 0.0,
            sub,
            sub_gpu,
            textures: HashMap::new(),
            assets,
            // Show the skinned mesh when there is one; otherwise fall back to bones.
            show_mesh: has_mesh,
            show_skeleton: !has_mesh,
            show_textures: true,
            show_collision: false,
            collision_volumes,
            body_pill,
            selected_bone: None,
            grid: None,
            // Start ANIMATED so the model is seen moving, not stuck in bind (user,
            // 2026-07-16: "see the model with animations attached and not get the A pose").
            // Toggle to Pose for fitting.
            animate: true,
            blend_enabled: true,
            editing: None,
            fit_focus: String::new(),
            fit_rows: load_fit_rows(),
            record_note: String::new(),
            record_note_t: 0.0,
            chars: chars.to_path_buf(),
            last_globals: Vec::new(),
            last_world: Mat4::IDENTITY,
            slots,
            sheaths,
            script: None,
            ui_tree: None,
            ui_intents: UiIntents::default(),
            fired_sigs: Vec::new(),
            ui_state: UiState::new(),
            // The Screen declaration (S8): one managed surface, publishing under
            // the tree's `fit_active` gate.
            surfaces: Surfaces::new(vec![Surface::new("fit_gadget").key("fit_active")]),
            ui_styles: serde_json::Value::Null,
            hud_commands: Vec::new(),
            hud_white: None,
            prev_up: false,
            prev_down: false,
            prev_left: false,
            prev_right: false,
            prev_r: false,
            alt_model,
            body_b: false,
            prev_x: false,
            preview: false,
            preview_clip: 0,
            preview_time: 0.0,
            prev_p: false,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            ui_theme: None,
        }
    }

    /// Engine values published to the HUD each frame (the Lua `Model` global). Per slot it
    /// publishes `slot_<key>_loaded` (is there a piece at all — the cell draws lit, or dim
    /// and inert) and `slot_<key>_on` (is it worn). `stats` is the readout that used to be
    /// the POC's `draw_text` line.
    fn hud_model(&self) -> ValueMap {
        let worn = self.slots.iter().filter(|s| s.on).count();
        let mut m = ValueMap::new()
            .with("show_mesh", self.show_mesh)
            .with("show_textures", self.show_textures)
            .with("show_skeleton", self.show_skeleton)
            .with("show_collision", self.show_collision)
            .with("animate", self.animate)
            .with("blend", self.blend_enabled)
            // Fit gadget: the `fit_gadget` surface (published from the Screen
            // declaration at the end of this fn) gates the whole panel, so with
            // nothing selected the script draws no gadget at all rather than
            // editing a phantom slot.
            .with(
                "fit_name",
                self.editing
                    .and_then(|i| self.slots.get(i))
                    .map(|s| s.def.file.trim_end_matches(".json").to_string())
                    .unwrap_or_default(),
            )
            .with(
                "fit_bone",
                self.editing
                    .and_then(|i| self.slots.get(i))
                    .map(|s| match s.def.fit {
                        Fit::Prop(b) => b.to_string(),
                        Fit::Garment => "(skinned)".to_string(),
                    })
                    .unwrap_or_default(),
            )
            .with("fit_focus", self.fit_focus.clone())
            .with(
                "fit_status",
                if self.record_note_t > 0.0 { self.record_note.clone() } else { String::new() },
            )
            .with("fit_u", self.editing.and_then(|i| self.slot_at(i)).map_or(1.0, |s| s.fit.uniform))
            .with("fit_sx", self.editing.and_then(|i| self.slot_at(i)).map_or(1.0, |s| s.fit.scale.x))
            .with("fit_sy", self.editing.and_then(|i| self.slot_at(i)).map_or(1.0, |s| s.fit.scale.y))
            .with("fit_sz", self.editing.and_then(|i| self.slot_at(i)).map_or(1.0, |s| s.fit.scale.z))
            // Rotation rows READ OUT the quaternion's Euler decomposition (degrees) for the
            // slider display only; authoring is nudge-only (D.6), so these are informational.
            .with("fit_rx", self.editing.and_then(|i| self.slot_at(i)).map_or(0.0, |s| s.fit.user_rot.to_euler(glam::EulerRot::XYZ).0.to_degrees()))
            .with("fit_ry", self.editing.and_then(|i| self.slot_at(i)).map_or(0.0, |s| s.fit.user_rot.to_euler(glam::EulerRot::XYZ).1.to_degrees()))
            .with("fit_rz", self.editing.and_then(|i| self.slot_at(i)).map_or(0.0, |s| s.fit.user_rot.to_euler(glam::EulerRot::XYZ).2.to_degrees()))
            .with("fit_x", self.editing.and_then(|i| self.slot_at(i)).map_or(0.0, |s| s.fit.offset.x))
            .with("fit_y", self.editing.and_then(|i| self.slot_at(i)).map_or(0.0, |s| s.fit.offset.y))
            .with("fit_z", self.editing.and_then(|i| self.slot_at(i)).map_or(0.0, |s| s.fit.offset.z))
            .with(
                "stats",
                format!(
                    "{} · {} bones · worn {}/{}{}",
                    if self.preview {
                        match self.model.clips.get(self.preview_clip) {
                            Some(c) => format!(
                                "PREVIEW {}/{}: {} (P off \u{00B7} \u{2191}\u{2193} clip)",
                                self.preview_clip + 1,
                                self.model.clips.len(),
                                c.name,
                            ),
                            None => "PREVIEW (no clips)".to_string(),
                        }
                    } else if self.animate {
                        match self.sm.as_ref() {
                            Some(sm) => format!(
                                "{} (WASD \u{00B7} Shift run \u{00B7} C crouch \u{00B7} Space jump \u{00B7} \u{2191}\u{2193} state)",
                                sm.current_state_name(),
                            ),
                            None => "no pack".to_string(),
                        }
                    } else {
                        "A-pose".to_string()
                    },
                    self.model.bones.len(),
                    worn,
                    self.slots.len(),
                    self.selected_bone
                        .and_then(|i| self.model.bones.get(i))
                        .map(|b| {
                            let t = b.local.w_axis.truncate();
                            format!(" · bone {} @ ({:.1}, {:.1}, {:.1})", b.name, t.x, t.y, t.z)
                        })
                        .unwrap_or_default(),
                ),
            );
        for s in &self.slots {
            m.set(format!("slot_{}_loaded", s.def.key), s.loaded());
            m.set(format!("slot_{}_on", s.def.key), s.on);
            // The inventory cell binds `slot_<key>` for BOTH its current value and its
            // write-back, so `apply_hud`'s unconditional read has the value every frame.
            m.set(format!("slot_{}", s.def.key), s.on);
        }
        // The declared surface's gate (`fit_active`) — published from the Screen
        // declaration, synced from the selection in `update`.
        self.surfaces.publish(&mut m);
        // The transient `sig_<name>` mirror (S9): intent names fired last frame
        // ride exactly this ONE publish for scripts to observe (`update` clears
        // them right after the walk).
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    /// Nudge the focused fit row by one fine step. Returns `true` if it consumed the press —
    /// the caller must then NOT also step the playhead, since ←/→ drive both.
    ///
    /// The arrows exist because a trackpad drag over a ~120px track spans the slider's whole
    /// range, which is far too coarse to fit a piece by eye (user, 2026-07-16). Hold Shift
    /// for the coarse step.
    fn nudge_fit(&mut self, dir: f32, coarse: bool) -> bool {
        let Some(i) = self.editing else { return false };
        if self.fit_focus.is_empty() {
            return false;
        }
        // Steps come from the same JSON the sliders' ranges do, so the gadget stays tunable
        // without a recompile.
        let ui: serde_json::Value = match std::fs::read_to_string(HUD_UI_ELEMENTS)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
        {
            Some(v) => v,
            None => return false,
        };
        let f = &ui["paperdoll"]["fit"];
        let Some((_, group)) = self.fit_rows.iter().find(|(k, _)| *k == self.fit_focus) else {
            return false;
        };
        let group = group.clone();
        let key = if coarse { "nudge_coarse" } else { "nudge" };
        let step = f[&group][key].as_f64().unwrap_or(0.1) as f32 * dir;
        let (lo, hi) = (
            f[&group]["min"].as_f64().unwrap_or(0.0) as f32,
            f[&group]["max"].as_f64().unwrap_or(1.0) as f32,
        );
        // `slot_at_mut` borrows all of self, so take the key first.
        let focus = self.fit_focus.clone();
        let Some(s) = self.slot_at_mut(i) else { return false };
        match focus.as_str() {
            "fit_u" => s.fit.uniform = (s.fit.uniform + step).clamp(lo, hi),
            "fit_sx" => s.fit.scale.x = (s.fit.scale.x + step).clamp(lo, hi),
            "fit_sy" => s.fit.scale.y = (s.fit.scale.y + step).clamp(lo, hi),
            "fit_sz" => s.fit.scale.z = (s.fit.scale.z + step).clamp(lo, hi),
            // Rotation nudges COMPOSE a world-axis quaternion (D.6): no gimbal fold, no clamp —
            // a prop can spin through a full turn on any axis. `step` is in degrees.
            "fit_rx" => s.fit.user_rot = (glam::Quat::from_axis_angle(Vec3::X, step.to_radians()) * s.fit.user_rot).normalize(),
            "fit_ry" => s.fit.user_rot = (glam::Quat::from_axis_angle(Vec3::Y, step.to_radians()) * s.fit.user_rot).normalize(),
            "fit_rz" => s.fit.user_rot = (glam::Quat::from_axis_angle(Vec3::Z, step.to_radians()) * s.fit.user_rot).normalize(),
            "fit_x" => s.fit.offset.x = (s.fit.offset.x + step).clamp(lo, hi),
            "fit_y" => s.fit.offset.y = (s.fit.offset.y + step).clamp(lo, hi),
            "fit_z" => s.fit.offset.z = (s.fit.offset.z + step).clamp(lo, hi),
            _ => return false,
        }
        self.mirror_fit(i);
        true
    }

    /// DEBUG (D.4 test): hot-swap the base body A ↔ B. Both bodies loaded the SAME anim+retarget
    /// clip dirs, so their clip lists match by name/order and the whole `Model` (rig+mesh+clips)
    /// swaps in one move — only `sub`/`sub_gpu` need rebuilding. Outfits/fit stay sized for base A
    /// (this is a body-verify toggle, not a re-fit), and base B borrows base A's `texture_0.png`
    /// (same basename) — geometry + animation are what this checks. Toggling leaks the previous
    /// frame's `sub_gpu` handles once; negligible for a debug tool.
    fn toggle_body(&mut self) {
        if self.alt_model.is_none() {
            tracing::info!("body toggle: no base B loaded (PrismHumanBaseB absent)");
            return;
        }
        let mut alt = self.alt_model.take().unwrap();
        std::mem::swap(&mut self.model, &mut alt);
        self.alt_model = Some(alt);
        self.body_b = !self.body_b;
        self.sub = submeshes_of(&self.model.mesh);
        self.sub_gpu = (0..self.sub.len()).map(|_| None).collect();
        // Keep the previewer pointed at the walk on the new body, if a preview is active.
        if self.preview && !self.model.clips.is_empty() {
            if let Some(i) = self.model.clips.iter().position(|c| c.name == "walk_forward") {
                self.preview_clip = i;
            }
            self.preview_clip = self.preview_clip.min(self.model.clips.len() - 1);
        }
        tracing::info!(
            "body → {} ({} verts, {} bones)",
            if self.body_b { "PrismHumanBaseB (male)" } else { "PrismHumanBaseA (female)" },
            self.model.mesh.vertices.len(),
            self.model.bones.len(),
        );
    }

    /// Move focus to the next/previous gadget row (↑/↓), so a fit can be dialled in without
    /// going back to the mouse for every axis. Returns `true` if it consumed the press.
    /// Wraps, and with nothing focused ↓ takes the first row / ↑ the last.
    fn cycle_fit_focus(&mut self, dir: i32) -> bool {
        if self.editing.is_none() || self.fit_rows.is_empty() {
            return false;
        }
        let n = self.fit_rows.len() as i32;
        let cur = self.fit_rows.iter().position(|(k, _)| *k == self.fit_focus);
        let next = match cur {
            Some(i) => (i as i32 + dir).rem_euclid(n),
            None if dir > 0 => 0,
            None => n - 1,
        };
        self.fit_focus = self.fit_rows[next as usize].0.clone();
        true
    }

    /// Mirror the piece at `i` onto its opposite half, if it has one. Both halves are the
    /// same garment, so fitting them separately is duplicated work that drifts out of
    /// symmetry (user, 2026-07-16: "if I move one mirror the other").
    ///
    /// The mirror is across the body's midline (x = 0), and it is exact because the fit
    /// cancels each socket's rest rotation — offsets and rotations are therefore in WORLD
    /// axes, and the two limb bones are themselves mirrored. The split halves are likewise
    /// already mirrored in their own mesh space (+0.46 vs -0.46), which is what makes
    /// negating x correct rather than coincidental:
    ///   * `offset.x` NEGATES; y (depth) and z (up) are unchanged.
    ///   * `scale`/`uniform` are unchanged — a reflection does not resize.
    ///   * rotation: PITCH survives, YAW and ROLL negate. Reflection reverses handedness, so
    ///     a rotation about the mirror axis maps to itself while the other two invert.
    ///   * `rot` (the auto upright correction) is NEVER copied — it is a property of the
    ///     partner's OWN socket and is re-derived from the rig.
    fn mirror_fit(&mut self, i: usize) {
        let Some(src) = self.slot_at(i) else { return };
        let Some(partner) = src.def.mirror else { return };
        let f = src.fit;
        let Some(j) = self
            .slots
            .iter()
            .position(|s| s.def.key == partner)
            .or_else(|| self.sheaths.iter().position(|s| s.def.key == partner).map(|k| k + self.slots.len()))
        else {
            return;
        };
        let m = f.mirrored();
        if let Some(p) = self.slot_at_mut(j) {
            p.fit.uniform = m.uniform;
            p.fit.scale = m.scale;
            p.fit.offset = m.offset;
            p.fit.user_rot = m.user_rot;
            // p.fit.rot is deliberately untouched: the partner's own socket correction.
        }
    }

    /// Apply the HUD's reported state back onto the engine. A slot that has no piece can
    /// never be worn, regardless of what the script reports.
    fn apply_hud(&mut self, res: &ValueMap) {
        for s in &mut self.slots {
            if s.content.is_some() {
                s.on = res.is_on(&format!("slot_{}", s.def.key));
            }
        }
        self.show_mesh = res.is_on("show_mesh");
        self.show_textures = res.is_on("show_textures");
        self.show_skeleton = res.is_on("show_skeleton");
        self.show_collision = res.is_on("show_collision");
        self.animate = res.is_on("animate");
        self.blend_enabled = res.is_on("blend");
        // One-frame attack pulse; consumed by the next state-machine advance.
        if res.is_on("attack") {
            self.attack_pending = true;
        }

        // Fit gadget → the edited slot. Only the selected slot is touched, and only while a
        // selection exists, so the sliders can never write onto the wrong piece.
        if let Some(i) = self.editing {
            if let Some(s) = self.slot_at_mut(i) {
                if let Some(v) = res.number("fit_u") {
                    s.fit.uniform = (v as f32).max(0.01);
                }
                if let Some(v) = res.number("fit_sx") {
                    s.fit.scale.x = (v as f32).max(0.01);
                }
                if let Some(v) = res.number("fit_sy") {
                    s.fit.scale.y = (v as f32).max(0.01);
                }
                if let Some(v) = res.number("fit_sz") {
                    s.fit.scale.z = (v as f32).max(0.01);
                }
                // Rotation is authored by nudges ONLY (D.6) — never absolute-set from the HUD's
                // Euler read-out, which would re-introduce the gimbal fold. Scale/offset below
                // stay absolute.
                if let Some(v) = res.number("fit_x") {
                    s.fit.offset.x = v as f32;
                }
                if let Some(v) = res.number("fit_y") {
                    s.fit.offset.y = v as f32;
                }
                if let Some(v) = res.number("fit_z") {
                    s.fit.offset.z = v as f32;
                }
            }
            self.mirror_fit(i);
        }
        if let Some(Value::Text(f)) = res.get("fit_focus") {
            self.fit_focus = f.clone();
        }
        if res.is_on("fit_record") {
            let chars = self.chars.clone();
            // Confirm IN THE WINDOW: a silent write is indistinguishable from a broken
            // button, and a failed one even more so.
            self.record_note = match self.record_fits(&chars) {
                Ok(n) => {
                    tracing::info!("Record: wrote {n} fit(s)");
                    format!("Saved {n} pieces -> inline attach")
                }
                Err(e) => {
                    tracing::error!("Record failed: {e}");
                    format!("SAVE FAILED: {e}")
                }
            };
            self.record_note_t = RECORD_NOTE_SECS;
        }
    }

    /// The piece at a combined index: `slots` first, then `sheaths`. Sheaths have no
    /// inventory cell (they are always worn), so clicking them in the scene is the ONLY way
    /// to fit them — they must share the selection address space or stay unreachable.
    fn slot_at(&self, i: usize) -> Option<&Slot> {
        let n = self.slots.len();
        if i < n {
            self.slots.get(i)
        } else {
            self.sheaths.get(i - n)
        }
    }

    fn slot_at_mut(&mut self, i: usize) -> Option<&mut Slot> {
        let n = self.slots.len();
        if i < n {
            self.slots.get_mut(i)
        } else {
            self.sheaths.get_mut(i - n)
        }
    }

    /// Click-to-select: the WORN piece whose geometry is nearest the cursor, or `None` for a
    /// click on empty space (which deselects). Uses the shared `Camera::pick_ray` +
    /// `ray_triangle` (`flicker-render`) rather than a fourth hand-rolled copy.
    ///
    /// The ray is transformed into each piece's LOCAL space by inverting its model matrix,
    /// so the raw mesh triangles can be tested untransformed — one matrix inverse per piece
    /// instead of transforming every vertex. Brute force over ~8-15k triangles per piece at
    /// CLICK rate (not per frame), matching the pocclusters precedent.
    fn pick_slot(&self, cursor: Vec2, viewport: Vec2, world: Mat4, globals: &[Mat4]) -> Option<usize> {
        let (o, d) = self.cam.camera().pick_ray(cursor, viewport)?;
        let mut best: Option<(usize, f32)> = None;
        // Worn slots AND the always-worn sheaths — same index space as `slot_at`.
        let all = self.slots.iter().chain(self.sheaths.iter());
        for (i, slot) in all.enumerate() {
            if !(slot.on || (i >= self.slots.len() && slot.loaded())) {
                continue;
            }
            let Some(SlotContent::Prop { mesh, bone, .. }) = &slot.content else {
                continue;
            };
            let model = world * globals[*bone] * slot.fit.matrix();
            let inv = model.inverse();
            let (lo, ld) = (inv.transform_point3(o), inv.transform_vector3(d));
            for tri in mesh.indices.chunks_exact(3) {
                let (a, b, c) = (
                    Vec3::from(mesh.vertices[tri[0] as usize].p),
                    Vec3::from(mesh.vertices[tri[1] as usize].p),
                    Vec3::from(mesh.vertices[tri[2] as usize].p),
                );
                if let Some(t) = flicker::render::ray_triangle(lo, ld, a, b, c) {
                    // `t` is in the piece's local space; compare in a single space by
                    // measuring the hit back in world.
                    let hit = model.transform_point3(lo + ld * t);
                    let dist = (hit - o).length();
                    if best.is_none_or(|(_, bd)| dist < bd) {
                        best = Some((i, dist));
                    }
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Write every loaded piece's fit to `musefit/fits.json`. The upright correction is NOT
    /// written — it is re-derived from the rig each load, so a rig fix can't be shadowed by
    /// a stale record.
    fn record_fits(&self, chars: &Path) -> Result<usize> {
        let mut n = 0;
        for s in self.slots.iter().chain(self.sheaths.iter()) {
            if s.content.is_none() {
                continue;
            }
            // `offset` is WORLD axes at rest (x lateral, y depth, z up — NOT bone axes); `scale`
            // × `uniform` size the raw ~1.899-normalised vendor mesh; `rotate` is a quaternion.
            // The socket's upright correction is deliberately NOT written — it is re-derived from
            // the rig each load, so a rig fix can never be shadowed by a stale record.
            let attach = serde_json::json!({
                "socket": s.def.key,
                "offset": [s.fit.offset.x, s.fit.offset.y, s.fit.offset.z],
                "rotate": [s.fit.user_rot.x, s.fit.user_rot.y, s.fit.user_rot.z, s.fit.user_rot.w],
                "scale": [s.fit.scale.x, s.fit.scale.y, s.fit.scale.z],
                "uniform": s.fit.uniform,
            });
            // Fold the fit INLINE into the piece's own self-describing file (WS-C C-γ).
            write_inline_attach(&chars.join(s.def.dir).join(s.def.file), attach)?;
            n += 1;
        }
        tracing::info!("recorded {n} inline attach fit(s) into their piece files");
        Ok(n)
    }




    /// Resolve a material's PBR map basenames to loaded texture handles (each `None`
    /// when the map is absent or failed to load → the pipeline default). `textures-off`
    /// suppresses the whole set (matte look).
    fn resolve_maps(&self, mat: &format::Material) -> PbrMaps {
        if !self.show_textures {
            return PbrMaps::default();
        }
        resolve_maps_in(&self.textures, mat)
    }

    /// Resolve a material's base-color name to its texture.
    ///
    /// The old 1/2/3 skin-variant swap (`Katanami_` → `Katanami2_`/`Katanami3_`) is GONE —
    /// it was a Katanami-model artifact, and the base body is a Meshy character with a
    /// single albedo. Removed 2026-07-16 (user: "old katanami model artifacts ... and also
    /// dead code").
    fn variant_albedo(&self, base: &str) -> Option<TextureHandle> {
        if base.is_empty() {
            return None;
        }
        self.textures.get(base).copied()
    }

    /// Sample a clip's LOCAL bone poses at `tick`, or the rest pose when the clip
    /// index is missing (an unresolved state, or no clips at all).
    /// Force the state machine to the next/previous authored state (↑/↓ while animating),
    /// so every animation can be previewed without game inputs. Wraps.
    fn cycle_state(&mut self, delta: i32) {
        if self.sm_states.is_empty() {
            return;
        }
        let n = self.sm_states.len() as i32;
        self.state_idx = (self.state_idx as i32 + delta).rem_euclid(n) as usize;
        let name = self.sm_states[self.state_idx].clone();
        if let Some(sm) = self.sm.as_mut() {
            sm.force_state_by_name(&name);
        }
    }

    /// Looped play-head tick for the clip previewer: real time × the clip's tick rate, wrapped
    /// to its length, so a raw clip repeats indefinitely for inspection.
    fn preview_tick(&self) -> u32 {
        match self.model.clips.get(self.preview_clip) {
            Some(c) if c.duration_ticks > 0 => {
                let rate = c.tick_rate_hz.max(1) as f32;
                ((self.preview_time * rate) as u32) % c.duration_ticks
            }
            _ => 0,
        }
    }

    fn sample_locals(&self, clip_idx: usize, tick: u32) -> Vec<Mat4> {
        match self.model.clips.get(clip_idx) {
            Some(clip) => {
                let t = tick.min(clip.duration_ticks.saturating_sub(1));
                pose::sample_local_poses(&self.model.bones, clip, t, self.model.retarget)
            }
            None => self.model.bones.iter().map(|b| b.local).collect(),
        }
    }

    /// Parent→child bone segments in engine space (`world` maps source space to
    /// engine space, including the vertical reframing offset).
    /// Click-to-select the BONE whose joint segment (parent→child) is nearest the cursor ray — the
    /// rig/joint editor's pick (Slice 2). Returns the bone index, or `None` if the rig has no
    /// selectable segments. The synthetic root (no parent) has no segment and is skipped. Picks the
    /// globally-nearest bone (no distance threshold yet — a screen-space cutoff can refine it).
    fn pick_bone(&self, cursor: Vec2, viewport: Vec2, world: Mat4, globals: &[Mat4]) -> Option<usize> {
        let (o, d) = self.cam.camera().pick_ray(cursor, viewport)?;
        let mut best: Option<(usize, f32)> = None;
        for (i, bone) in self.model.bones.iter().enumerate() {
            if bone.parent < 0 {
                continue;
            }
            let a = world.transform_point3(globals[bone.parent as usize].w_axis.truncate());
            let b = world.transform_point3(globals[i].w_axis.truncate());
            let (pr, ps) = mechanics::closest_point_ray_segment(o, d, a, b);
            let dist = (pr - ps).length();
            if best.is_none_or(|(_, bd)| dist < bd) {
                best = Some((i, dist));
            }
        }
        best.map(|(i, _)| i)
    }

    /// The selected bone's world-space segment (parent→child), for highlighting it in the panels.
    fn selected_bone_seg(&self, world: Mat4, globals: &[Mat4]) -> Option<(Vec3, Vec3)> {
        let i = self.selected_bone?;
        let bone = self.model.bones.get(i)?;
        if bone.parent < 0 {
            return None;
        }
        let a = world.transform_point3(globals[bone.parent as usize].w_axis.truncate());
        let b = world.transform_point3(globals[i].w_axis.truncate());
        Some((a, b))
    }

    fn bone_segments(&self, world: Mat4, globals: &[Mat4]) -> Vec<(Vec3, Vec3)> {
        // Rig-view geometry lives in the shared overlay lib (flicker-mechanics); the scene just
        // supplies the topology + posed globals.
        let parents: Vec<i32> = self.model.bones.iter().map(|b| b.parent).collect();
        mechanics::debug::joint_segments(world, &parents, globals)
    }

    /// The auto-fit collision volumes as wireframe line segments in engine space — each volume's
    /// shape placed by its bone's current pose (`world * globals[bone]`), for the debug overlay.
    fn collision_segments(&self, world: Mat4, globals: &[Mat4]) -> Vec<(Vec3, Vec3)> {
        let mut segs = Vec::new();
        // The whole-unit movement pill: bounds the body, fixed in the root frame (drawn at `world`).
        segs.extend(mechanics::debug::wireframe(&self.body_pill.transformed(world)));
        // Player: the auto-fit capsule per bone, placed by the bone's current pose.
        for vol in &self.collision_volumes {
            let Some(bone_global) = globals.get(vol.bone) else {
                continue;
            };
            let shape = vol.world(world * *bone_global);
            segs.extend(mechanics::debug::wireframe(&shape));
        }
        // Items: fit a capsule to each WORN ITEM's mesh (the weapons + their scabbards) and place it
        // at the prop's own world transform. Rigid props carry no skeleton, so the fit is over the
        // mesh. Cosmetic props (hair, pendant) and body-following garments are EXCLUDED — an auto-fit
        // over the ponytail was the big stray cylinder round the head/shoulders.
        for (i, slot) in self.slots.iter().chain(self.sheaths.iter()).enumerate() {
            let worn = slot.on || (i >= self.slots.len() && slot.loaded());
            if !worn || !matches!(slot.def.key, "rhand" | "lhand" | "katana_sheath" | "dagger_sheath") {
                continue;
            }
            let Some(SlotContent::Prop { mesh, bone, .. }) = &slot.content else {
                continue;
            };
            let model = world * globals[*bone] * slot.fit.matrix();
            let shape =
                mechanics::fit_capsule(mesh.vertices.iter().map(|v| Vec3::from(v.p))).transformed(model);
            segs.extend(mechanics::debug::wireframe(&shape));
        }
        segs
    }

    /// Per-bone down-bone axis (each bone's local +X in world space), as line segments.
    /// The joint wireframe (`bone_segments`) only shows joint POSITIONS — it can't reveal a
    /// bone's rest-frame ORIENTATION, which is what the mesh actually skins to and what the
    /// retarget aligns to each body's limbs. Drawn from each joint along its frame X, signed
    /// toward the child and scaled to the bone length, so for a correctly limb-aligned rig
    /// the axis lies ON the joint segment; a bone whose frame is off its limb (the arm
    /// "hunch") shows its axis visibly crossing the joint line. Leaf bones (no child) are
    /// skipped — there's no limb to compare against.
    fn bone_axis_segments(&self, world: Mat4, globals: &[Mat4]) -> Vec<(Vec3, Vec3)> {
        let parents: Vec<i32> = self.model.bones.iter().map(|b| b.parent).collect();
        mechanics::debug::frame_axis_segments(world, &parents, globals)
    }
}

impl Scene for Viewer {
    fn enter(&mut self, renderer: &mut Renderer) {
        // Gothic theme for the shell pause overlay we push on Esc.
        self.ui_theme = Some(Theme::build(renderer));
        // Load every material's textures once (deduped), including the katana's
        // overridden maps (rewritten to the Hair atlas in `main`).
        // Gather every texture to load as (basename, is_srgb, source_dir). Each body
        // loads its maps from ITS OWN asset dir, so a swapped-in body (the Muse) resolves
        // its material references beside its own mesh: the Katanami bundle (`self.assets`)
        // for the Katanami body + katana, and the sibling `musefit/` dir for the Muse001
        // outfit, whose material points at the Meshy bakes (`Baked_BaseColor.png`, …). Albedo
        // (base_color) is sRGB COLOUR data; the PBR maps (normal/roughness/metalness/ao)
        // are LINEAR (uploaded via `load_texture_linear`, no sRGB decode). A missing/failed
        // texture just leaves that slot at the pipeline default.
        let muse_dir = self
            .assets
            .parent()
            .map(|p| p.join("musefit"))
            .unwrap_or_else(|| self.assets.clone());
        let mut wanted: Vec<(String, bool, PathBuf)> = Vec::new();
        {
            let mut add = |mats: &[format::Material], dir: &Path| {
                for m in mats {
                    if !m.base_color.is_empty() {
                        wanted.push((m.base_color.clone(), true, dir.to_path_buf()));
                    }
                    for map in [&m.normal, &m.roughness, &m.metalness, &m.ao] {
                        if !map.is_empty() {
                            wanted.push((map.clone(), false, dir.to_path_buf()));
                        }
                    }
                }
            };
            add(&self.model.mesh.materials, &self.assets);
            // Every slot piece's textures, resolved from the dir its file came from.
            for s in self.slots.iter().chain(self.sheaths.iter()) {
                let dir = if s.def.dir == "katanami" {
                    self.assets.clone()
                } else {
                    muse_dir.clone()
                };
                match &s.content {
                    Some(SlotContent::Garment(g)) => add(&g.mesh.materials, &dir),
                    Some(SlotContent::Prop { mesh: m, .. }) => add(&m.materials, &dir),
                    _ => {}
                }
            }
        }
        // Dedup by basename (first source dir wins). Basenames don't collide across the
        // Muse (`Baked_*`) set + the base body's own maps, so the flat basename-keyed `textures`
        // map stays unambiguous. (The old Katanami skin-variant (1/2/3) loads were removed with
        // the legacy strip — they looked in the base dir, which never had them: dead no-ops.)
        wanted.sort_by(|a, b| a.0.cmp(&b.0));
        wanted.dedup_by(|a, b| a.0 == b.0);
        for (name, is_srgb, dir) in &wanted {
            if self.textures.contains_key(name) {
                continue;
            }
            let path = dir.join(name);
            match image::open(&path) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let handle = if *is_srgb {
                        renderer.load_texture(rgba.as_raw(), w, h)
                    } else {
                        renderer.load_texture_linear(rgba.as_raw(), w, h)
                    };
                    self.textures.insert(name.clone(), handle);
                    tracing::info!(
                        "loaded texture {name} ({w}x{h}, {})",
                        if *is_srgb { "sRGB" } else { "linear" }
                    );
                }
                Err(e) => tracing::warn!("texture {name} failed ({e}); slot uses default"),
            }
        }
        // Upload every prop once (static geometry) — each slot's weapon/pendant plus the
        // always-worn sheaths. Each submesh is textured with its albedo + PBR maps (loaded
        // above) or flat steel where it has no maps. Drawn each frame at its socket bone's
        // animated transform. Taking the mesh leaves `mesh: None` — the upload is one-shot.
        let steel = pack_rgb666(KATANA_COLOR[0], KATANA_COLOR[1], KATANA_COLOR[2]);
        let textures = std::mem::take(&mut self.textures);
        let mut uploaded: Vec<(&str, usize)> = Vec::new();
        for s in self.slots.iter_mut().chain(self.sheaths.iter_mut()) {
            let Some(SlotContent::Prop { mesh: m, parts, uploaded: done, .. }) = &mut s.content
            else {
                continue;
            };
            if *done {
                continue;
            }
            *done = true;
            let ranges: Vec<(usize, usize, usize)> = if m.submeshes.is_empty() {
                vec![(usize::MAX, 0, m.indices.len())]
            } else {
                m.submeshes.iter().map(|s| (s.material, s.start, s.count)).collect()
            };
            for (mat, start, count) in ranges {
                if count == 0 || start + count > m.vertices.len() {
                    continue;
                }
                let material = m.materials.get(mat);
                let base = material.map(|x| x.base_color.as_str()).unwrap_or("");
                let tex = if base.is_empty() {
                    None
                } else {
                    textures.get(base).copied()
                };
                let indices: Vec<u32> = (0..count as u32).collect();
                let part = match tex {
                    Some(th) => {
                        let maps = material.map(|x| resolve_maps_in(&textures, x)).unwrap_or_default();
                        let verts = build_textured_verts(
                            start..start + count,
                            |j| m.vertices[j].p,
                            |j| m.vertices[j].n,
                            |j| m.vertices[j].uv,
                        );
                        let h = renderer.upload_textured_mesh(&verts, MeshIndices::U32(&indices));
                        PropPart::Textured(h, th, maps)
                    }
                    None => {
                        let verts: Vec<MeshVertex> = (start..start + count)
                            .map(|j| MeshVertex {
                                position: m.vertices[j].p,
                                normal: m.vertices[j].n,
                                material: steel,
                            })
                            .collect();
                        PropPart::Flat(renderer.upload_mesh(&verts, MeshIndices::U32(&indices)))
                    }
                };
                parts.push(part);
            }
            uploaded.push((s.def.key, parts.len()));
        }
        self.textures = textures;
        for (key, n) in uploaded {
            tracing::info!("prop '{key}': {n} parts uploaded");
        }
        // 1×1 white pixel — `render_hud` tints it to build the HUD's solid quads.
        self.hud_white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));

        // Editor 2×2 grid: the shared `flicker-render` viewport grid creates one offscreen target
        // per view, sized to its screen cell, and composites each back as a framed, labelled panel.
        self.grid = Some(QuadGrid::editor(renderer));

        // The in-scene Lua HUD: inventory bar + view toggles + stat readout. A failure
        // here leaves `script: None` and the scene simply renders without a HUD.
        // The Prism-token-resolved styles the component walker resolves node `style`
        // paths against (the same tree Lua reads via the `UI` global).
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);
        self.script = match ScriptHost::from_file_with_modules(HUD_SCRIPT_PATH, UI_COMPONENT_MODULES)
        {
            Ok(s) => {
                load_ui_json(&s, HUD_UI_ELEMENTS); // layout constants (`UI.paperdoll`)
                // Build the component tree ONCE (step 4 lazy build-once): the walker
                // redraws this cached tree every frame with fresh Model bindings.
                match s.ui_tree() {
                    Ok(Some(tree)) => {
                        // The screen's declarative bindings (S9), read off the
                        // root once — cached exactly like the tree.
                        self.ui_intents = UiIntents::of(&tree);
                        self.ui_tree = Some(tree);
                    }
                    Ok(None) => tracing::error!("HUD script exposes no `tree()` — no HUD"),
                    Err(e) => tracing::error!("HUD tree build failed ({e}); no HUD"),
                }
                tracing::info!("loaded HUD script from {HUD_SCRIPT_PATH}");
                Some(s)
            }
            Err(e) => {
                tracing::error!("HUD script failed to load ({e}); scene runs without a HUD");
                None
            }
        };

        tracing::info!(
            "flicker-paperdoll: {} bones, {} clips, {} submeshes, {} textures",
            self.model.bones.len(),
            self.model.clips.len(),
            self.sub.len(),
            self.textures.len(),
        );
    }

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        // NOTE Esc/Menu → pause is no longer edge-detected here: the resolver owns the
        // press edge and the `RootHandler` consumes `Menu` in the dispatch below, turning
        // it into `Transition::Push(PauseScene)`. The orbit camera is likewise NOT updated
        // here — it runs AFTER the HUD + dispatch below, gated on `over_hud` (the walker's
        // canonical pointer-consume), so dragging a slider cannot also spin the view.

        // ── Edge-detect the one-shot keys once; some are reinterpreted per mode
        // (Space = play/pause in Manual, jump in Graph; R = reset play-head vs. reset
        // the state machine). ──
        // NOTE Space is NOT edge-detected here any more (S9b): the resolver owns the
        // Jump press edge, recorded by the gameplay base in the dispatch below.
        let up = input.key_down(Key::Up);
        let mut up_edge = up && !self.prev_up;
        self.prev_up = up;
        let down = input.key_down(Key::Down);
        let mut down_edge = down && !self.prev_down;
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
        let p = input.key_down(Key::P);
        let p_edge = p && !self.prev_p;
        self.prev_p = p;
        // DEBUG (D.4 test): X hot-swaps the base A ↔ base B body.
        let x = input.key_down(Key::X);
        if x && !self.prev_x {
            self.toggle_body();
        }
        self.prev_x = x;


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
        // ── The Lua HUD owns every display toggle (hud_paperdoll.lua + `UI.paperdoll`): the
        // inventory bar, mesh/textures/skeleton/blend, and the skin variant. This replaced
        // the POC's M/B/T/K/L and 1-4 keys. Publish the engine's values, run the script,
        // apply what it hands back — two-way, so the engine stays the single source of
        // truth and the script holds no duplicate state. Genuine INPUT stays on the
        // keyboard below (camera, playback, mode, and the state machine's drivers).
        self.record_note_t = (self.record_note_t - dt.as_secs_f32()).max(0.0);
        self.frame_dt = dt.as_secs_f32(); // carried into render, where the necklace is stepped
        let screen = _renderer.size();
        let mut over_hud = false;
        if self.ui_tree.is_some() {
            // The component walker owns layout + draw + hit-test. Build the model, walk
            // the cached tree → this frame's draw commands + interaction results. No
            // per-frame Lua: the tree is the one parsed at load.
            // Surface state derived from scene state, synced before the Model is built.
            self.surfaces.set("fit_gadget", self.editing.is_some());
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                screen,
                typed: String::new(),
                backspace: false,
                wheel: input.mouse_wheel_delta,
            };
            let frame = {
                // Disjoint field borrows: `ui_tree`/`ui_styles`/`script` read, `ui_state` mutated.
                let tree = self.ui_tree.as_ref().unwrap();
                let lib = self.script.as_ref().map(|h| h as &dyn ComponentLibrary);
                run_ui_with(tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib)
            };
            // `hud_hit` = cursor over any UI region, OR a slider drag in progress
            // (captures until release). That click must not ALSO pick the scene behind
            // it, nor orbit the camera (user, 2026-07-16: dragging a slider spun the view).
            over_hud = frame.results.is_on("hud_hit");
            self.apply_hud(&frame.results);
            self.hud_commands = frame.commands;
        }

        // ── The input seam (spec §9): ONE resolve + ONE dispatch replaces the raw
        // `menu_prev` edge and the `mouse_left_pressed && !over_hud` ray-pick gate.
        //   [ROOT]  RootHandler   — consumes Menu (Esc) → the scene pushes Pause
        //   [1]     WalkerHandler — consumes the pointer while `over_hud` (HUD-owned)
        //   [2]     GameplayBase  — a click that bubbled past the HUD → the ray-pick
        // `ev` is the REUSED `Fired` buffer; the `InputEvent` list is a short-lived local
        // (it borrows this frame's snapshot, so it cannot be a field — RT-7 holds because
        // steady-state frames resolve zero edges and allocate nothing).
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver
            .resolve_frame(&self.bindings, &self.gamepad_config, input, self.tick, &mut self.ev);
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        let mut gameplay = GameplayBase::default();
        {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut gameplay];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // Surface context wiring (S9): declared-surface flips become Push/Pop
        // context requests (no paperdoll surface carries a context today — the
        // seam is standard, the call a live no-op), then the standard reconcile:
        // context requests into the bindings, the focus decision THROUGH the
        // walker (one id for mouse + gamepad, spec §4.2a).
        self.surfaces.apply_surface_contexts(&mut self.route);
        let focus_change = apply_context_requests(&mut self.bindings, &self.route.requests);
        walker.apply_focus(focus_change);
        // The screen's fired intents (S9), drained once: acted on below and queued
        // for the one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();
        self.route.requests.clear();

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto the
        // shell pause push — the root's hardcoded Menu arm is gone. The scene
        // manager freezes us while the overlay is up.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.ui_theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                self.bindings.active_map(),
                &AbstractControls::default(),
                &self.gamepad_config,
            )));
        }

        // Gameplay base (Pass-only): a `PrimaryAction` press that bubbled past the HUD
        // walker runs the ray-pick — a bone in the rig view, a worn piece otherwise (the
        // typed form of the old `mouse_left_pressed && !over_hud` gate). It picks against
        // LAST frame's posed geometry, since `update` runs before `render`.
        if gameplay.pick && !self.last_globals.is_empty() {
            let world = self.last_world;
            let globals = std::mem::take(&mut self.last_globals);
            if self.show_skeleton {
                // Rig view: a click selects a BONE (the joint editor, Slice 2). The interactive
                // perspective is grid view 0, so map the click into that cell — the grid owns
                // the layout, so the pick ray matches exactly what that panel displays.
                let cell = self.grid.as_ref().map(|g| (g.local_cursor(0, input.mouse_position, screen), g.cell(0, screen).size));
                if let Some((Some(local), viewport)) = cell {
                    self.selected_bone = self.pick_bone(local, viewport, world, &globals);
                    match self.selected_bone {
                        Some(i) => tracing::info!(
                            "selected bone '{}' (local {:?})",
                            self.model.bones[i].name,
                            self.model.bones[i].local.w_axis.truncate(),
                        ),
                        None => tracing::info!("bone selection cleared"),
                    }
                }
            } else {
                // Normal view: a click selects a WORN piece for fitting (existing behaviour).
                self.editing = self.pick_slot(input.mouse_position, screen, world, &globals);
                match self.editing.and_then(|i| self.slot_at(i)) {
                    Some(s) => tracing::info!("selected '{}' for fitting", s.def.key),
                    None => tracing::info!("selection cleared"),
                }
            }
            self.last_globals = globals;
        }

        // The camera gets the mouse only if the HUD didn't claim it. `over_hud` is the
        // walker's canonical "pointer consumed" verdict (the router mechanism, spec §9),
        // reused to gate the POLLED left-drag orbit: the pick above is gated structurally
        // by the walker, but a polled drag needs this explicit check so a slider drag
        // never also spins the model (user, 2026-07-16).
        if !over_hud {
            self.cam.update(input);
        }

        // A FOCUSED fit row takes ←/→ before playback does — both want the arrows, and
        // while you are fitting a piece the nudge is what you mean. Consuming the edge stops
        // the play-head stepping underneath the gadget.
        let coarse = input.key_down(Key::LeftShift) || input.key_down(Key::RightShift);
        // ←/→ nudge the focused fit row (no other consumer now the manual browser is gone).
        if left_edge {
            self.nudge_fit(-1.0, coarse);
        }
        if right_edge {
            self.nudge_fit(1.0, coarse);
        }
        // ↑/↓ walk the gadget's rows while a piece is selected — otherwise they cycle clips.
        // Same precedence as ←/→: while fitting, the gadget is what you mean.
        if up_edge && self.cycle_fit_focus(-1) {
            up_edge = false;
        }
        if down_edge && self.cycle_fit_focus(1) {
            down_edge = false;
        }

        // ── Debug clip previewer (P) ──
        // Loop a RAW clip straight from `model.clips`, bypassing the state machine, so a
        // retargeted clip (e.g. `walk_forward`) can be watched on repeat to check foot plant /
        // drift. ↑/↓ select the clip (consumed here so they don't also cycle states); toggling
        // off returns to normal Animate/Pose. A debug tool, deliberately additive.
        if p_edge {
            self.preview = !self.preview;
            self.preview_time = 0.0;
            if self.preview && !self.model.clips.is_empty() {
                // Jump straight to the in-place walk on entry, if present, so the retargeted
                // clip we care about is right there; otherwise keep the current index.
                if let Some(i) = self.model.clips.iter().position(|c| c.name == "walk_forward") {
                    self.preview_clip = i;
                }
                self.preview_clip = self.preview_clip.min(self.model.clips.len() - 1);
            }
            match self.model.clips.get(self.preview_clip) {
                Some(c) => tracing::info!(
                    "clip previewer {}: '{}'",
                    if self.preview { "ON" } else { "OFF" },
                    c.name,
                ),
                None => tracing::info!(
                    "clip previewer {}",
                    if self.preview { "ON" } else { "OFF" }
                ),
            }
        }
        // ↑/↓ pick a RAW clip from `model.clips` and play it ON REPEAT (auto-entering the
        // preview cycle) — every clip, including the retargeted ones the Katanami state machine
        // can't reach, is watchable looping. A gameplay key (below) hands control back to the
        // state machine; `P` jumps straight to walk_forward. The arrows only fall through to the
        // state-cycle below when there are NO clips to preview.
        if !self.model.clips.is_empty() {
            let n = self.model.clips.len();
            if up_edge {
                self.preview = true;
                self.preview_clip = (self.preview_clip + n - 1) % n;
                self.preview_time = 0.0;
                up_edge = false;
                tracing::info!("preview clip '{}'", self.model.clips[self.preview_clip].name);
            }
            if down_edge {
                self.preview = true;
                self.preview_clip = (self.preview_clip + 1) % n;
                self.preview_time = 0.0;
                down_edge = false;
                tracing::info!("preview clip '{}'", self.model.clips[self.preview_clip].name);
            }
        }
        if self.preview {
            self.preview_time += dt.as_secs_f32();
        }

        // ── Animation playback (the pack's state machine) ──
        // WASD/Shift/C/Space drive the runtime state machine (gameplay preview); using any of
        // them drops out of the ↑/↓ clip-preview cycle. R resets the state machine. ←/→ stay
        // free for the fit gadget above. Inert while in Pose mode.
        if self.animate {
            // ↑/↓ normally loop a raw clip (handled above); they only reach this state-cycle
            // when there are no clips at all to preview.
            if up_edge {
                self.cycle_state(-1);
            }
            if down_edge {
                self.cycle_state(1);
            }
            // Basic locomotion drives the state machine, mirroring packeditor's hookup —
            // and the whole `Inputs` feed reads the BUS now (S9b), not raw keys:
            //  * WASD/d-pad move via `signal_held(Move*)` (as before);
            //  * run = `signal_held(Sprint)` — the default preset binds LShift AND
            //    RShift additively, so both physical keys sprint exactly as the old
            //    raw read did (Shift also still doubles as the fit-nudge "coarse"
            //    modifier above, unchanged);
            //  * crouch = `signal_held(Crouch)` — C as before (bound additively in
            //    the preset), plus the preset's Ctrl;
            //  * jump = the resolver's `Jump` press edge (Space), recorded by the
            //    gameplay base in the dispatch above — the raw `space_edge` latch died;
            //  * attack = the HUD's ATTACK button OR an `AttackLight` press off the
            //    bus — the FIRST souls-intent consumer (a souls profile's LMB/RB
            //    lands here; the HUD button keeps working).
            // Same physical keys, same feel; only the read path changed.
            let (w, a, s, d, run, crouch) = {
                let cfg = &self.gamepad_config;
                (
                    self.bindings.signal_held(ActionSignal::MoveForward, input, cfg),
                    self.bindings.signal_held(ActionSignal::StrafeLeft, input, cfg),
                    self.bindings.signal_held(ActionSignal::MoveBackward, input, cfg),
                    self.bindings.signal_held(ActionSignal::StrafeRight, input, cfg),
                    self.bindings.signal_held(ActionSignal::Sprint, input, cfg),
                    self.bindings.signal_held(ActionSignal::Crouch, input, cfg),
                )
            };
            let attack = self.attack_pending || gameplay.attack;
            // Any gameplay input returns from the ↑/↓ clip-preview cycle to the state machine.
            if w || a || s || d || gameplay.jump || crouch || attack {
                self.preview = false;
            }
            let inputs = Inputs {
                move_: w || a || s || d,
                left: a,
                right: d,
                back: s,
                run,
                crouch,
                jump: gameplay.jump,
                attack,
                hit: false,
                die: false,
            };
            self.attack_pending = false;
            if let Some(sm) = self.sm.as_mut() {
                if r_edge {
                    sm.reset();
                }
                sm.advance(dt.as_secs_f32(), &inputs);
            }
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // POSE selection. Animate → the state machine's current clip+tick, crossfaded with
        // the outgoing pose while a transition ramps (unless Blend is off). Pose, or a
        // pack-less body, → A-pose (bind): each bone's own rest local, the honest fitting
        // reference (Idle is an authored stance, already moved away from bind).
        let locals = if self.preview {
            // Debug previewer takes precedence: loop the selected raw clip (see update()).
            let tick = self.preview_tick();
            self.sample_locals(self.preview_clip, tick)
        } else {
            match (self.animate, self.sm.as_ref()) {
                (true, Some(sm)) => {
                    let incoming = self.sample_locals(sm.current_clip(), sm.current_tick());
                    match (self.blend_enabled, sm.blend()) {
                        (true, Some(b)) => {
                            let outgoing = self.sample_locals(b.from_clip, b.from_tick);
                            pose::blend_local_poses(&outgoing, &incoming, b.weight)
                        }
                        _ => incoming,
                    }
                }
                _ => self.model.bones.iter().map(|b| b.local).collect(),
            }
        };
        let globals = pose::global_transforms(&self.model.bones, &locals);
        // Cache the pose being drawn so a click next frame picks against what the user SAW
        // (update runs before render, so the click has no globals of its own).
        self.last_globals = globals.clone();
        // Skinning palette (base skeleton pose) — shared by the base body AND the outfit
        // overlay, so the outfit skins with the same pose and registers exactly.
        let palette = skin::palette(&self.model.bones, &globals);

        let world = Mat4::from_translation(Vec3::new(0.0, self.world_y, 0.0)) * self.model.world;
        self.last_world = world; // paired with `last_globals` for next frame's pick

        // ── Editor RTT panels: render the character (skinned mesh) + skeleton from each view into
        // its offscreen target and composite it into a screen quadrant, all through the frame graph
        // BEFORE the main-frame draws. Its offscreen passes reset the per-frame draw queues, so the
        // main-view `set_camera` must come AFTER `fg.execute`. The base body is skinned + uploaded
        // ONCE here and drawn into all four panels (freed after execute); the skeleton is drawn as
        // an OVERLAY so it reads through the mesh.
        if let Some(grid) = self.grid.as_ref() {
            let base_layer = renderer.layer();
            let joints = self.bone_segments(world, &globals);
            let axes = self.bone_axis_segments(world, &globals);
            let sel = self.selected_bone_seg(world, &globals);
            // Violet for the bone SEGMENTS — matching the Clayworks rig view's bone/joint
            // hue split (bones violet, joint balls cyan) so the two tools read alike.
            const BONE: [f32; 4] = [0.62, 0.50, 0.95, 1.0];
            const AXIS: [f32; 4] = [1.0, 0.55, 0.1, 1.0];
            const SELECTED: [f32; 4] = [1.0, 0.8, 0.15, 1.0];

            // Skin the base body once + upload each submesh (textured / flat), to draw into every
            // panel. This is the mesh the grid was missing — it used to draw only into the hidden
            // main frame. Honors the Mesh / Textures toggles (`show_mesh` / `show_textures`).
            let mut panel_subs: Vec<PanelSub> = Vec::new();
            if self.show_mesh && !self.model.mesh.vertices.is_empty() {
                let skinned = skin::skin(&self.model.mesh, &palette);
                for &(mat, start, count) in &self.sub {
                    if count == 0 || start + count > skinned.len() {
                        continue;
                    }
                    let material = self.model.mesh.materials.get(mat);
                    let base = material.map(|m| m.base_color.as_str()).unwrap_or("");
                    let tex = if self.show_textures { self.variant_albedo(base) } else { None };
                    let indices: Vec<u32> = (0..count as u32).collect();
                    match tex {
                        Some(th) => {
                            let maps = material.map(|m| self.resolve_maps(m)).unwrap_or_default();
                            let verts = build_textured_verts(
                                start..start + count,
                                |j| skinned[j].position,
                                |j| skinned[j].normal,
                                |j| self.model.mesh.vertices[j].uv,
                            );
                            let h = renderer.upload_textured_mesh(&verts, MeshIndices::U32(&indices));
                            panel_subs.push(PanelSub::Textured { h, tex: th, maps });
                        }
                        None => {
                            let mat_id = material
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
                            panel_subs.push(PanelSub::Flat(h));
                        }
                    }
                }
            }

            // Bones over the mesh only when the Skeleton toggle is on, or when there's no mesh to
            // show (mirrors the main-frame rule at the skeleton overlay) — so a textured body reads
            // clean by default and `Skeleton` reveals the rig.
            let draw_bones = self.show_skeleton || panel_subs.is_empty();

            // The grid renders each view into its offscreen target and composites it into its
            // screen cell (framed + labelled) one 2D layer above the backdrop. Target ordering and
            // the frame/portrait/label blit are centralized in `flicker-render` — no hand-ordering,
            // no free-a-target-mid-loop hazard, and the panel unit is emitted together so it can no
            // longer fall behind the HUD (both former bugs stay structurally impossible). The base
            // body is skinned/uploaded once above and drawn into every view; freed after the pass.
            let subs = &panel_subs;
            let jref = &joints;
            let aref = &axes;
            grid.render(
                renderer,
                base_layer + 1.0,
                self.model.orbit_radius,
                &self.cam.camera(),
                |r, _view| {
                    r.set_scene(pip_scene());
                    for sub in subs {
                        match sub {
                            PanelSub::Textured { h, tex, maps } => {
                                r.draw_textured_mesh_pbr(*h, *tex, *maps, world, MeshDrawOptions::default());
                            }
                            PanelSub::Flat(h) => r.draw_mesh(*h, world, MeshDrawOptions::default()),
                        }
                    }
                    if draw_bones {
                        r.draw_lines_overlay(jref, BONE);
                        r.draw_lines_overlay(aref, AXIS);
                        if let Some(seg) = sel {
                            r.draw_lines_overlay(&[seg], SELECTED);
                        }
                    }
                },
            );

            // Free the per-panel meshes AFTER the graph runs (freeing earlier would blank panels).
            for sub in panel_subs {
                match sub {
                    PanelSub::Textured { h, .. } => renderer.free_textured_mesh(h),
                    PanelSub::Flat(h) => renderer.free_mesh(h),
                }
            }
        }

        // Main-view camera (after the panel RTT passes, which reset the draw queues).
        renderer.set_camera(&self.cam.camera());

        // Slice 2/3: CPU-skinned mesh, one draw per material submesh. Skin all
        // vertices once, then per submesh build textured (albedo) or flat (gray) GPU
        // vertices and re-upload (freeing last frame's buffers first). Index ranges
        // are vertex ranges — the converter emits a non-deduped, sequential list.
        let mesh_drawn = self.show_mesh && !self.model.mesh.vertices.is_empty();
        if mesh_drawn {
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

        // Outfit overlay: the Muse drawn OVER the base body, skinned by the SAME palette
        // (its joints were remapped into base-skeleton space at load). Taken out of `self`
        // for the frame so the per-submesh GPU juggling doesn't alias the `&self`
        // texture/material lookups, then put back. Same textured/flat split as the base.
        let mut slots = std::mem::take(&mut self.slots);
        for slot in slots.iter_mut().filter(|s| s.on) {
            let Some(SlotContent::Garment(outfit)) = &mut slot.content else {
                continue;
            };
            if self.show_mesh {
                // Rigid skin, then swing the dangly cloth regions on top (a no-op when the
                // garment has no cloth metadata) — overwrites only the bound cloth verts.
                let mut skinned = skin::skin(&outfit.mesh, &palette);
                outfit.cloth.update(&palette, self.frame_dt, &mut skinned);
                for si in 0..outfit.sub.len() {
                    if let Some(prev) = outfit.sub_gpu[si].take() {
                        match prev {
                            SubGpu::Textured(h) => renderer.free_textured_mesh(h),
                            SubGpu::Flat(h) => renderer.free_mesh(h),
                        }
                    }
                    let (mat, start, count) = outfit.sub[si];
                    if count == 0 || start + count > skinned.len() {
                        continue;
                    }
                    let material = outfit.mesh.materials.get(mat);
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
                            let verts = build_textured_verts(
                                start..start + count,
                                |j| skinned[j].position,
                                |j| skinned[j].normal,
                                |j| outfit.mesh.vertices[j].uv,
                            );
                            let h = renderer.upload_textured_mesh(&verts, MeshIndices::U32(&indices));
                            renderer.draw_textured_mesh_pbr(
                                h,
                                th,
                                maps,
                                world,
                                MeshDrawOptions::default(),
                            );
                            outfit.sub_gpu[si] = Some(SubGpu::Textured(h));
                        }
                        None => {
                            let mat_id = material
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
                            outfit.sub_gpu[si] = Some(SubGpu::Flat(h));
                        }
                    }
                }
            }
        }
        self.slots = slots;

        // Props: rigid, drawn at their socket bone's animated global transform (× the
        // world matrix, same as the character). Grip offset is identity for now (tune if a
        // blade sits wrong in the hand). Worn slots plus the always-worn sheaths.
        for slot in self
            .slots
            .iter()
            .filter(|s| s.on)
            .chain(self.sheaths.iter().filter(|s| s.loaded()))
        {
            // The neck pendant is drawn via the necklace jiggle chain below, not rigidly.
            if slot.def.key == "neck" {
                continue;
            }
            let Some(SlotContent::Prop { parts, bone, .. }) = &slot.content else {
                continue;
            };
            // socket's animated transform × the piece's authored fit (scale + offset). The
            // fit is what makes the piece visible at all: the vendor normalises every asset
            // to a ~1.899 longest axis, which is ~1% of body height — a speck at the origin.
            let prop_model = world * globals[*bone] * slot.fit.matrix();
            draw_prop_parts(renderer, parts, prop_model, self.show_textures);
        }

        // ── Necklace: the pendant hangs from the neck on a jiggle chain and swings. The chain
        // works in rig-local space (same as `globals`), so `world` is applied only at draw —
        // gravity −Z is "down" because the body stands z-up here. Built lazily from the neck's
        // pose the first frame it's worn; dropped (to re-settle fresh) when unworn.
        if let Some(ni) = self.necklace_bone {
            let neck_slot = self.slots.iter().position(|s| s.def.key == "neck" && s.on);
            match neck_slot {
                Some(nsi) => {
                    let neck_pos = globals[ni].w_axis.truncate();
                    if self.necklace.is_none() {
                        let seg = NECKLACE_DRAPE.length() / NECKLACE_SEGMENTS as f32;
                        self.necklace = Some(jiggle::JiggleChain::new(
                            neck_pos,
                            NECKLACE_DRAPE.normalize_or_zero(),
                            seg,
                            NECKLACE_SEGMENTS,
                            necklace_params(),
                        ));
                    }
                    // The driver twist: how much the neck's world rotation has turned since
                    // bind. Rotating the drape by it makes the necklace follow the upper body
                    // when the shoulders/spine twist, instead of staying world-forward.
                    let neck_rot = glam::Quat::from_mat4(&globals[ni]);
                    let twist = neck_rot * self.necklace_neck_rest_rot.inverse();
                    // Step, then COPY the points so the mutable chain borrow ends before we
                    // touch `self.slots` for the pendant's orientation + parts.
                    let chain = self.necklace.as_mut().expect("just built");
                    chain.step(neck_pos, twist, self.frame_dt);
                    let pts: Vec<Vec3> = chain.positions().to_vec();
                    let tip = *pts.last().expect("chain has points");

                    // The cord: line segments between the chain points (in world space).
                    let segs: Vec<(Vec3, Vec3)> = pts
                        .windows(2)
                        .map(|w| (world.transform_point3(w[0]), world.transform_point3(w[1])))
                        .collect();
                    renderer.draw_lines(&segs, NECKLACE_CORD_COLOR);

                    // The medallion HANGS from its top at the cord tip and tilts with the
                    // cord, so the swing reads as rotation, not just translation. Build the
                    // frame explicitly: tall axis (local Z) up along the cord, disc face
                    // (local Y normal) forward (−Y), so it always faces the viewer.
                    let hang = (pts[pts.len() - 1] - pts[pts.len() - 2]).normalize_or_zero();
                    let up = -hang; // toward the neck
                    // Face the chest's (twisted) forward, not world-forward, so the medallion
                    // turns with the body.
                    let fwd = twist * Vec3::NEG_Y;
                    let face = (fwd - up * fwd.dot(up)).try_normalize().unwrap_or(Vec3::NEG_Y);
                    let right = up.cross(face).normalize_or_zero();
                    let rot = glam::Mat3::from_cols(right, face, up);
                    // Cord attaches at the pendant's TOP → its centre is half a height further
                    // down the cord.
                    let center = tip + hang * (NECKLACE_PENDANT_CM * 0.5);
                    let model = world
                        * Mat4::from_translation(center)
                        * Mat4::from_mat3(rot)
                        * Mat4::from_scale(Vec3::splat(self.necklace_pendant_scale));
                    if let Some(SlotContent::Prop { parts, .. }) = &self.slots[nsi].content {
                        draw_prop_parts(renderer, parts, model, self.show_textures);
                    }
                }
                None => self.necklace = None,
            }
        }

        // Skeleton view (B): the parent→child joint wireframe (cyan) PLUS each bone's frame
        // axis (orange). Both are laid OVER the mesh (depth-ignoring) when it's shown so the
        // rig reads through it; a plain depth-tested wireframe when the mesh is hidden. The
        // frame-axis pass repurposes what used to be a redundant second skeleton copy — it is
        // the only thing that reveals bone ORIENTATION (e.g. whether the retarget aligned the
        // arm bones to this body's limbs), which the joint-position wireframe alone cannot.
        if self.show_skeleton || !mesh_drawn {
            let joints = self.bone_segments(world, &globals);
            let axes = self.bone_axis_segments(world, &globals);
            // Violet bone segments — the Clayworks rig-view hue split (bones violet,
            // joint balls cyan), kept in step so the two tools read alike.
            const BONE: [f32; 4] = [0.62, 0.50, 0.95, 1.0]; // violet
            const AXIS: [f32; 4] = [1.0, 0.55, 0.1, 1.0]; // orange
            if mesh_drawn {
                renderer.draw_lines_overlay(&joints, BONE);
                renderer.draw_lines_overlay(&axes, AXIS);
            } else {
                renderer.draw_lines(&joints, BONE);
                renderer.draw_lines(&axes, AXIS);
            }
            // Highlight the SELECTED bone's segment (Slice 2) in amber, always over the top.
            if let Some(i) = self.selected_bone {
                if let Some(bone) = self.model.bones.get(i) {
                    if bone.parent >= 0 {
                        let a = world.transform_point3(globals[bone.parent as usize].w_axis.truncate());
                        let b = world.transform_point3(globals[i].w_axis.truncate());
                        const SELECTED: [f32; 4] = [1.0, 0.8, 0.15, 1.0]; // amber
                        renderer.draw_lines_overlay(&[(a, b)], SELECTED);
                    }
                }
            }
        }

        // Collision-volume debug overlay (WS-D D6): the auto-fit capsules/spheres, wireframed on the
        // posed bones, so the collision coverage is visible on the character.
        if self.show_collision {
            let segs = self.collision_segments(world, &globals);
            const COLLISION: [f32; 4] = [0.25, 1.0, 0.45, 0.9]; // green
            if mesh_drawn {
                renderer.draw_lines_overlay(&segs, COLLISION);
            } else {
                renderer.draw_lines(&segs, COLLISION);
            }
        }

        // ── HUD ── (all readouts live in the Lua HUD; see `hud_model`)
        // The stat readout that used to live here as a `draw_text` line is now published
        // as `Model.stats` and drawn by the Lua HUD (see `hud_model`), alongside the
        // inventory bar and the view toggles — rendered at the end of this method.
        // Editor 2×2 grid backdrop: an opaque fullscreen fill UNDER the quad composites (emitted a
        // layer up by the frame graph above), so the covered main scene doesn't show through the
        // 2px quad seams. (Skipping the covered main-scene draw entirely is a perf follow-up.)
        if self.grid.is_some() {
            let screen = renderer.size();
            renderer.draw_ui_panel(
                Vec2::ZERO, screen, [0.03, 0.03, 0.04, 1.0], [0.03, 0.03, 0.04, 1.0], 0.0, 0.0, 0.0,
                [0.0; 4], 0.0,
            );
        }

        // ── The Lua HUD, last so it lays over the scene: the inventory bar, the view
        // toggles, and the stat readout. `update` already published the Model and applied
        // the script's reported state; this only turns its plain-data commands into draw
        // calls (the strict boundary — the script never sees a renderer handle).
        if let Some(white) = self.hud_white {
            // The walker already produced this frame's commands in `update`; here we
            // only blit them (the strict boundary — the script never sees a renderer).
            //
            // Lift the WHOLE HUD above the RTT-panel composites. The frame graph emits each panel
            // (frame → portrait → label) at `base + 1`; the HUD sits at `base + 2` so its own
            // ui-panels (view checkboxes, inventory cells) are never buried under a panel's sprite
            // (within a layer the paint order is ui-panels → sprites → text).
            let base = renderer.layer();
            renderer.set_layer(base + 2.0);
            render_hud(renderer, &self.hud_commands, white, &[]);
            renderer.set_layer(base);
        }
    }
}

/// Build the viewer scene: the BASE body rig + mesh + textures come from `base_dir`,
/// the clip library + state `.pack` are the base's OWN retargeted Motifect locomotion set (the
/// retired Katanami library is no longer loaded — legacy strip, resolved by shared bone names).
/// The Muse outfit overlay loads from the sibling `muse/` dir. Panics with a clear
/// message if the base assets can't be read — the point of this viewer is to play the
/// *real* exported data.
fn build_viewer(base_dir: &Path) -> Viewer {
    // The unified base (PrismHumanBaseA) + its retargeted Motifect LOCOMOTION library
    // (Alpha/content/package/retarget/clips/locomotion — In-Place / RootMotion, e.g. `walk_forward`,
    // cyclable with the up/down control). The retired Katanami clip library is NO LONGER loaded
    // (legacy strip, 2026-07-21): the base owns its own clips + pack, so the ↑/↓ preview and the SM
    // see only the current data. Only the locomotion library is loaded — the other per-library
    // trees (`retarget/clips/<lib>/`: combat / daily_life / …) would pull ~800 MB and collide on
    // shared stems.
    // Prefer a per-body clip library `retarget/clips/<base>/locomotion` when present — clips
    // retargeted onto THIS body's own bind (in-app `flicker-content::retarget`), e.g. HumanBaseA's
    // flat-foot re-bake. It lives OUTSIDE the character dir (so `load_dirs`' base-dir recursion
    // doesn't double-load it). Falls back to the shared `retarget/clips/locomotion`.
    let base_name = base_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let content_root = base_dir.parent().and_then(|p| p.parent());
    let retarget_dir = content_root
        .map(|c| c.join(format!("retarget/clips/{base_name}/locomotion")))
        .filter(|d| d.is_dir())
        .or_else(|| content_root.map(|c| c.join("retarget/clips/locomotion")))
        .unwrap_or_else(|| base_dir.join("retarget/clips/locomotion"));
    let model = format::load_dirs(&[base_dir, retarget_dir.as_path()])
        .expect("load flicker.rig assets");
    tracing::info!(
        "loaded rig: {} bones, {} clips, mesh {} verts (base {}, source {}/{})",
        model.bones.len(),
        model.clips.len(),
        model.mesh.vertices.len(),
        base_dir.display(),
        model.source.source_axis,
        model.source.source_unit,
    );
    for clip in &model.clips {
        if !clip.unresolved.is_empty() {
            tracing::debug!(
                "clip '{}': {} track bone(s) not in base skeleton (dropped jiggle bones, first few: {:?})",
                clip.name,
                clip.unresolved.len(),
                &clip.unresolved[..clip.unresolved.len().min(5)],
            );
        }
    }

    // The pack's runtime state machine drives the Animate view. States reference clips by
    // NAME, resolved against the model's clip list (same as the rig loader resolves tracks).
    // Missing pack → Animate simply shows bind; only a bad `initial` is fatal at build.
    // The base owns its pack by naming convention (`<base>/<Base>.pack.json` — the golem
    // baseline pack drives the shared Motifect clips). A base with no pack of its own falls
    // through to `load_pack` erroring → Animate shows bind.
    let pack_path = base_dir
        .file_name()
        .map(|n| base_dir.join(format!("{}.pack.json", n.to_string_lossy())))
        .unwrap_or_else(|| base_dir.join("pack.json"));
    let (sm, sm_states) = match state::load_pack(&pack_path) {
        Ok(def) => {
            let names: Vec<String> = def.states.iter().map(|s| s.name.clone()).collect();
            let refs: Vec<state::ClipRef> = model
                .clips
                .iter()
                .map(|c| state::ClipRef { name: &c.name, duration_ticks: c.duration_ticks })
                .collect();
            match StateMachine::build(&def, &refs) {
                Ok(sm) => {
                    for w in sm.warnings() {
                        tracing::warn!("state pack: {w}");
                    }
                    tracing::info!("state machine ready ({} states), starting in '{}'",
                        names.len(), sm.current_state_name());
                    (Some(sm), names)
                }
                Err(e) => {
                    tracing::warn!("state pack failed to build ({e}); Animate shows bind");
                    (None, Vec::new())
                }
            }
        }
        Err(e) => {
            tracing::info!("no state pack ({e}); Animate shows bind");
            (None, Vec::new())
        }
    };

    // Equip pieces are resolved per-slot from the shared characters tree (see `SLOTS` /
    // `Slot::load`), so a piece appears in its cell the moment its file lands — no code
    // change. `base_dir` is <chars>/PrismHumanBaseA, so its parent is the tree root.
    let chars = base_dir.parent().unwrap_or(base_dir).to_path_buf();
    // DEBUG (D.4 test): load base B (male) alongside base A so `X` hot-swaps the body. Both use
    // the SAME retarget clip dir → identical clip lists (by name/order), so the whole Model swaps
    // cleanly and the retargeted clips drive base B's own bind (the D.3 fix).
    let alt_model = base_dir
        .parent()
        .map(|c| c.join("PrismHumanBaseB"))
        .filter(|d| d.exists())
        .and_then(|d| format::load_dirs(&[d.as_path(), retarget_dir.as_path()]).ok());
    if alt_model.is_some() {
        tracing::info!("base B (male) loaded alongside base A — press X to toggle the body");
    }
    Viewer::new(model, alt_model, base_dir.to_path_buf(), &chars, sm, sm_states)
}

/// Build the paperdoll rigging viewer as a boxed `Scene` for the shell's scene
/// registry — it loads the base rig / clips / pack from the shared content tree.
///
/// Content lives under `Alpha/content/package/characters/`. The BASE body is the
/// REFERENCE — GolemBase_Low (canon 66 names + topology, retarget=true), playing the
/// shared retarget clip library rotation-only with its own seeded baseline pack.
/// Finger/twist tracks stay unresolved (no source in the Motifect sets).
pub fn scene() -> Box<dyn Scene> {
    // THE REFERENCE BODY (Aaron, 2026-08-04): the golem IS the reference skeleton —
    // canon 66 names + topology, near-flat foot bind. It ships no clips or pack of its
    // own; the loader falls through to the shared `retarget/clips/locomotion` library,
    // which resolves on it by canonical bone name.
    let base_dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Alpha/content/package/characters/GolemBase_Low"
    ));
    Box::new(build_viewer(&base_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_skeletal::state::{self, StateMachine};

    fn assets_dir() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/package/characters/katanami"))
    }

    fn has_fixtures(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| {
                        e.path()
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.ends_with(".json") || n.ends_with(".json.gz"))
                    })
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

    /// The Prism locomotion pack (the base's OWN `PrismHumanBaseA.pack.json`) resolves every
    /// state's clip against the RETARGETED Motifect library by bare In-Place stem, so WASD
    /// locomotion has real animation to play and never silently sits in bind. Also proves
    /// `load_dirs` tolerates the `.pack.json` beside the rig in `base_dir` — the exact load
    /// the viewer does at startup. Skips on a checkout with no converted fixtures.
    #[test]
    fn prism_pack_resolves_against_the_retargeted_library() {
        let base = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Alpha/content/package/characters/PrismHumanBaseA"
        ));
        let retarget = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Alpha/content/package/retarget/clips/locomotion"
        ));
        let pack_path = base.join("PrismHumanBaseA.pack.json");
        if !flicker_core::compression::file_exists(&pack_path) || !retarget.join("In-Place").exists() {
            return; // no converted fixtures on this checkout
        }
        let model = format::load_dirs(&[base.as_path(), retarget.as_path()])
            .expect("load base A rig + retargeted clips (pack.json beside the rig must be tolerated)");
        let def = state::load_pack(&pack_path).expect("load Prism pack");
        let refs: Vec<state::ClipRef> = model
            .clips
            .iter()
            .map(|c| state::ClipRef { name: &c.name, duration_ticks: c.duration_ticks })
            .collect();
        let sm = StateMachine::build(&def, &refs).expect("build Prism state machine");
        let unresolved: Vec<&String> =
            sm.warnings().iter().filter(|w| w.contains("unknown clip")).collect();
        assert!(unresolved.is_empty(), "Prism pack references missing clips: {unresolved:?}");
        assert_eq!(sm.current_state_name(), "Idle", "Prism locomotion starts in Idle");
    }

    /// Animate mode must actually MOVE the rig — the whole point is to leave bind. Advancing
    /// the state machine over a few frames must change the sampled pose away from the rest
    /// locals, and forcing a different state must produce a different pose again. If either
    /// held still, the viewer would silently sit in bind while claiming to animate.
    #[test]
    fn state_machine_drives_the_pose_away_from_bind() {
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
            .map(|c| state::ClipRef { name: &c.name, duration_ticks: c.duration_ticks })
            .collect();
        let mut sm = StateMachine::build(&def, &refs).expect("build");

        let sample = |sm: &StateMachine| -> Vec<Mat4> {
            let clip = sm.current_clip();
            let tick = sm.current_tick();
            match model.clips.get(clip) {
                Some(c) => pose::sample_local_poses(&model.bones, c, tick, model.retarget),
                None => model.bones.iter().map(|b| b.local).collect(),
            }
        };
        let bind: Vec<Mat4> = model.bones.iter().map(|b| b.local).collect();
        let differs = |a: &[Mat4], b: &[Mat4]| {
            a.iter().zip(b).any(|(x, y)| {
                x.to_cols_array().iter().zip(y.to_cols_array()).any(|(p, q)| (p - q).abs() > 1e-3)
            })
        };

        // Idle over a second of frames must move off bind (Idle is an authored stance).
        for _ in 0..30 {
            sm.advance(1.0 / 30.0, &Inputs::default());
        }
        assert!(differs(&sample(&sm), &bind), "animating must move the rig off bind");

        // Forcing another state must give a different pose than the one we were in.
        let before = sample(&sm);
        let other = def.states.iter().map(|s| s.name.as_str()).find(|n| *n != sm.current_state_name());
        if let Some(name) = other {
            assert!(sm.force_state_by_name(name), "force_state_by_name must accept a real state");
            sm.advance(1.0 / 30.0, &Inputs::default());
            assert!(differs(&sample(&sm), &before), "cycling to another state must change the pose");
        }
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
            let locals = pose::sample_local_poses(&model.bones, clip, tick, model.retarget);
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

    /// A skinned garment (`tools/skin_outfit.py` output) must load via `load_outfit`, index
    /// the body skeleton, and — at bind — reproduce its baked positions (identity palette).
    /// The whole point is "the garment deforms WITH the body"; a bad joint remap or a decode
    /// bug would make it skin to garbage or vanish, with no error anywhere. Skips cleanly if
    /// the .skinned.json build artifact isn't present.
    #[test]
    fn skinned_garment_loads_and_binds_to_the_body() {
        let base_dir =
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/package/characters/PrismHumanBaseA"));
        let garment = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Alpha/content/package/characters/musefit/Corset-Duster.skinned.json"
        ));
        if !flicker_core::compression::file_exists(&garment) {
            eprintln!("skipping: {} not generated", garment.display());
            return;
        }
        let Ok(model) = format::load_dir(&base_dir) else { return };
        let mesh = format::load_outfit(&garment, &model.bones).expect("load_outfit skinned garment");
        assert!(!mesh.vertices.is_empty(), "skinned garment has geometry");
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.joints.iter().all(|&j| (j as usize) < model.bones.len())),
            "every garment joint must index the body skeleton after remap"
        );
        // Bind pose: palette is identity, so skinning must reproduce the baked rest verts —
        // i.e. the garment sits exactly where it was fitted, then deforms from there.
        let rest: Vec<Mat4> = model.bones.iter().map(|b| b.local).collect();
        let globals = pose::global_transforms(&model.bones, &rest);
        let palette = skin::palette(&model.bones, &globals);
        let skinned = skin::skin(&mesh, &palette);
        let max_err = skinned
            .iter()
            .zip(&mesh.vertices)
            .map(|(s, v)| {
                assert!(s.position.iter().all(|c| c.is_finite()), "no NaN in skinned garment");
                ((s.position[0] - v.p[0]).powi(2)
                    + (s.position[1] - v.p[1]).powi(2)
                    + (s.position[2] - v.p[2]).powi(2))
                .sqrt()
            })
            .fold(0.0_f32, f32::max);
        assert!(
            max_err < 1.0,
            "skinned garment at bind must match its baked positions; max err {max_err} cm — \
             a joint-remap or weight-transfer bug"
        );
    }

    /// A model shaped like `Viewer::hud_model` — every key the HUD reads.
    fn hud_test_model(loaded: bool, on: bool) -> ValueMap {
        let mut m = ValueMap::new()
            .with("show_mesh", true)
            .with("show_textures", true)
            .with("show_skeleton", false)
            .with("blend", true)
            .with("skin", 0_u32)
            // Sample stats copy, composed around its tokens exactly as display
            // words must be (Model-channel strings gate).
            .with(
                "stats",
                format!("63 {} · 115221 {}", strings::resolve("$pd_bones"), strings::resolve("$pd_verts")),
            );
        for d in SLOTS {
            m.set(format!("slot_{}_loaded", d.key), loaded);
            m.set(format!("slot_{}_on", d.key), on);
        }
        m
    }

    fn host() -> ScriptHost {
        let h = ScriptHost::from_file(HUD_SCRIPT_PATH).expect("load hud.lua");
        load_ui_json(&h, HUD_UI_ELEMENTS);
        h
    }

    /// Run the HUD's declared component tree through the walker — the engine's
    /// per-frame path (layout + hit-test + draw) — for asserting interaction results.
    fn run_hud(
        host: &ScriptHost,
        model: &ValueMap,
        input: &InputState,
        w: f32,
        h: f32,
    ) -> flicker::ui::UiFrame {
        let tree = host.ui_tree().expect("ui_tree parses").expect("hud.lua exposes tree()");
        // Vocabulary gate: an unknown component kind draws nothing at all, so a name left
        // behind by a rename would only show up by eye in the window.
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "hud_paperdoll.lua names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "hud_paperdoll.lua ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        // The MODEL-CHANNEL strings gate (S10's blind side): display copy published
        // from Rust into the Model bypasses the tree gate above, so the crate
        // self-gates its OWN source — every `.set`/`.with` value must be a resolved
        // `$token`, a data shape, or carry an explicit `strings-gate-exempt` reason.
        let flags = strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(flags.is_empty(), "raw display copy published into the Model: {flags:?}");
        let styles = load_styles(HUD_UI_ELEMENTS);
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen: Vec2::new(w, h),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        run_ui(&tree, model, &styles, &snap, &mut UiState::new())
    }

    fn ui() -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(HUD_UI_ELEMENTS).unwrap()).unwrap()
    }

    /// A selected piece, so the fit gadget's path is actually exercised.
    fn fit_model(active: bool) -> ValueMap {
        hud_test_model(true, true)
            .with("fit_active", active)
            .with("fit_name", "Corset-Duster")
            .with("fit_bone", "spine_02")
            // uniform carries the overall size; the per-axis rows are a multiplier at 1.0
            .with("fit_u", 44.79_f32)
            .with("fit_rx", 0.0_f32)
            .with("fit_ry", 0.0_f32)
            .with("fit_rz", 0.0_f32)
            .with("fit_sx", 1.0_f32)
            .with("fit_sy", 1.0_f32)
            .with("fit_sz", 1.0_f32)
            .with("fit_x", 0.0_f32)
            .with("fit_y", 0.0_f32)
            .with("fit_z", 0.0_f32)
    }

    /// WS-C C-γ safety: the Record read-modify-write folds `attach` INTO a real piece file while
    /// preserving its (multi-MB) mesh, and the result reloads to a runtime fit — with a second
    /// record replacing (not duplicating) the section. Guards content integrity before the Record
    /// button mutates real content; skips if the sample asset isn't present.
    #[test]
    fn record_folds_attach_inline_without_disturbing_the_mesh() {
        let src = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Alpha/content/package/characters/musefit/Dagger.json"
        ));
        if !flicker_core::compression::file_exists(&src) {
            eprintln!("skipping: {} not present", src.display());
            return;
        }
        let tmp = std::env::temp_dir().join("flicker_paperdoll_ws_c_record_roundtrip.json");
        std::fs::write(&tmp, flicker_core::compression::read_bytes(&src).expect("read sample piece"))
            .expect("copy sample piece");

        let before = format::load_mesh(&tmp).expect("baseline load").vertices.len();
        assert!(before > 0, "sample piece should have geometry");

        // Exactly the Record path: build the attach json, fold it in.
        let attach = serde_json::json!({
            "socket": "lhand", "offset": [4.6_f32, -13.0, 4.1],
            "rotate": [0.0_f32, 0.0, 0.0, 1.0], "scale": [1.0_f32, 1.0, 1.0], "uniform": 37.76_f32,
        });
        write_inline_attach(&tmp, attach).expect("fold attach inline");

        let (mesh, at) = format::load_mesh_with_attach(&tmp).expect("reload folded piece");
        assert_eq!(mesh.vertices.len(), before, "record must not disturb the mesh");
        assert_eq!(at.socket, "lhand");
        assert!((at.uniform - 37.76).abs() < 1e-3);
        assert!(RecordedFit::from_attach(&at).is_some(), "inline attach must yield a recorded fit");

        // Idempotent: re-recording REPLACES (not duplicates) attach; mesh still intact.
        let attach2 = serde_json::json!({
            "socket": "lhand", "offset": [0.0_f32, 0.0, 0.0], "rotate": [0.0_f32, 0.0, 0.0, 1.0],
            "scale": [1.0_f32, 1.0, 1.0], "uniform": 5.0_f32,
        });
        write_inline_attach(&tmp, attach2).expect("second fold");
        let (mesh2, at2) = format::load_mesh_with_attach(&tmp).expect("reload again");
        assert_eq!(mesh2.vertices.len(), before, "re-record must not disturb the mesh");
        assert!((at2.uniform - 5.0).abs() < 1e-3, "re-record replaces the attach");

        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(flicker_core::compression::gz_sibling(&tmp));
    }

    /// With nothing selected the gadget must not run at all — no phantom slot to edit.
    #[test]
    fn fit_gadget_is_absent_until_a_piece_is_selected() {
        let h = host();
        // Unselected: the fit column is gated by `fit_active`, so its sliders are never
        // placed and report nothing.
        let res = run_hud(&h, &fit_model(false), &InputState::new(), 1920.0, 1080.0).results;
        assert!(res.get("fit_u").is_none(), "no gadget values while unselected");
        assert!(!res.is_on("fit_record"), "record cannot fire while unselected");
        // Selected: the sliders ARE placed and report their bound values.
        let res = run_hud(&h, &fit_model(true), &InputState::new(), 1920.0, 1080.0).results;
        assert!(res.get("fit_u").is_some(), "gadget sliders report once a piece is selected");
    }

    /// Selected: the gadget runs, reports its values back, and draws.
    #[test]
    fn fit_gadget_reports_values_when_selected() {
        let h = host();
        let frame = run_hud(&h, &fit_model(true), &InputState::new(), 1920.0, 1080.0);
        // Every row the SHARED list declares must round-trip — a new row in the JSON that
        // the tree silently ignores would be a dead control.
        for row in ui()["paperdoll"]["fit"]["rows"].as_array().unwrap() {
            let k = row["key"].as_str().unwrap();
            assert!(frame.results.get(k).is_some(), "gadget must report the declared row '{k}'");
        }
        assert!(
            (frame.results.number("fit_u").unwrap() - 44.79).abs() < 0.01,
            "an untouched slider must report the Model's value back unchanged, not snap"
        );
        assert!(!frame.commands.is_empty(), "gadget draws a panel + rows");
    }

    /// `hud_hit` is the guard that stops a click on the HUD ALSO picking the scene behind
    /// it. If it ever reports false over a panel, clicking a slider would silently reselect
    /// whatever is behind the gadget.
    #[test]
    fn hud_hit_is_true_over_panels_and_false_over_open_scene() {
        let h = host();
        let (w, hgt) = (1920.0_f32, 1080.0_f32);
        let u = ui();
        let f = &u["paperdoll"]["fit"];
        let (m, fw) = (f["margin"].as_f64().unwrap() as f32, f["w"].as_f64().unwrap() as f32);

        let mut over = InputState::new();
        over.mouse_position = Vec2::new(w - m - fw * 0.5, m + 20.0); // inside the fit panel
        assert!(
            run_hud(&h, &fit_model(true), &over, w, hgt).results.is_on("hud_hit"),
            "cursor over the fit panel"
        );

        let mut open = InputState::new();
        open.mouse_position = Vec2::new(w * 0.5, hgt * 0.35); // open scene, above the bar
        assert!(
            !run_hud(&h, &fit_model(true), &open, w, hgt).results.is_on("hud_hit"),
            "cursor over open scene"
        );
    }

    /// The record button fires only on a click that lands on it.
    #[test]
    fn record_button_fires_only_when_clicked() {
        let h = host();
        let (w, hgt) = (1920.0_f32, 1080.0_f32);
        let u = ui();
        let f = &u["paperdoll"]["fit"];
        let g = |k: &str| f[k].as_f64().unwrap() as f32;
        // Mirror hud.lua's `fit_rects`: the RECORD button sits below the four slider rows.
        let y0 = g("margin") + g("pad") + g("header_size") + 6.0;
        // Derive from the SHARED row list rather than hardcoding a count — adding a row to
        // the JSON must not silently break this test's aim.
        let n_rows = f["rows"].as_array().unwrap().len() as f32;
        let rec_y = y0 + n_rows * g("row_h");
        let rec_h = f["record"]["h"].as_f64().unwrap() as f32;
        let mut click = InputState::new();
        click.mouse_position = Vec2::new(w - g("margin") - g("w") * 0.5, rec_y + rec_h * 0.5);
        click.mouse_left_pressed = true;
        assert!(
            run_hud(&h, &fit_model(true), &click, w, hgt).results.is_on("fit_record"),
            "click on RECORD fires"
        );

        let mut hover = click.clone();
        hover.mouse_left_pressed = false;
        assert!(
            !run_hud(&h, &fit_model(true), &hover, w, hgt).results.is_on("fit_record"),
            "hover must not fire"
        );
    }

    /// Clicking a slider must FOCUS it (that focus is what ←/→ then nudge), and focus must
    /// PERSIST across frames — otherwise the arrows would do nothing the moment the cursor
    /// moved off the track.
    #[test]
    fn clicking_a_slider_focuses_it_and_focus_persists() {
        let h = host();
        let (w, hgt) = (1920.0_f32, 1080.0_f32);
        let u = ui();
        let f = &u["paperdoll"]["fit"];
        let g = |k: &str| f[k].as_f64().unwrap() as f32;
        // Mirror hud.lua's `fit_rects` for the 2nd row (fit_x).
        let y0 = g("margin") + g("pad") + g("header_size") + 6.0;
        let sx = w - g("margin") - g("w") + g("pad") + g("label_w");
        let sw_ = g("w") - g("pad") * 2.0 - g("label_w") - g("value_w");
        let row_y = y0 + g("row_h") + (g("row_h") - g("slider_h")) * 0.5; // row 2 = fit_sx

        let focus_of = |res: &ValueMap| {
            res.get("fit_focus")
                .and_then(|v| match v {
                    Value::Text(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        };

        // Click in the slider's PADDED margin, not dead centre. `slider_update` grabs over
        // (r.y-6, r.h+12) while the track is only 12px tall, so this is where a real click
        // most often lands — and where focus used to silently fail to set while the drag
        // still worked. A centre-only test passes over the bug entirely.
        let mut click = InputState::new();
        click.mouse_position = Vec2::new(sx + sw_ * 0.5, row_y - 4.0);
        click.mouse_left_pressed = true;
        assert_eq!(
            focus_of(&run_hud(&h, &fit_model(true), &click, w, hgt).results),
            "fit_sx",
            "a click in the slider's padded grab margin must focus it — the drag region and \
             the focus region MUST agree, or the row drags without ever lighting up"
        );

        // ...and dead centre, obviously.
        click.mouse_position = Vec2::new(sx + sw_ * 0.5, row_y + g("slider_h") * 0.5);
        assert_eq!(
            focus_of(&run_hud(&h, &fit_model(true), &click, w, hgt).results),
            "fit_sx",
            "clicking a slider focuses it"
        );

        // Focus persists: the engine echoes it back on the Model, cursor now elsewhere.
        let mut away = InputState::new();
        away.mouse_position = Vec2::new(w * 0.5, hgt * 0.3);
        let res = run_hud(&h, &fit_model(true).with("fit_focus", "fit_sx"), &away, w, hgt).results;
        assert_eq!(
            res.get("fit_focus").and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("fit_sx"),
            "focus must survive the cursor leaving the track"
        );
    }

    /// Clicking a row's LABEL must focus it WITHOUT touching its value. Focusing via the
    /// track also drags it — so selecting a row used to destroy the value you had just set,
    /// which is the whole reason the label is a hit target.
    #[test]
    fn clicking_a_row_label_focuses_without_changing_its_value() {
        let h = host();
        let (w, hgt) = (1920.0_f32, 1080.0_f32);
        let u = ui();
        let f = &u["paperdoll"]["fit"];
        let g = |k: &str| f[k].as_f64().unwrap() as f32;
        let y0 = g("margin") + g("pad") + g("header_size") + 6.0;
        // Row 3 (fit_sy / "Thick"), in its LABEL column — left of the track.
        let label_x = w - g("margin") - g("w") + g("pad") + 4.0;
        let row_y = y0 + 2.0 * g("row_h") + g("row_h") * 0.5;

        let mut click = InputState::new();
        click.mouse_position = Vec2::new(label_x, row_y);
        click.mouse_left_pressed = true;
        let res = run_hud(&h, &fit_model(true), &click, w, hgt).results;
        assert_eq!(
            res.get("fit_focus").and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("fit_sy"),
            "clicking a row's label must focus that row"
        );
        assert!(
            (res.number("fit_sy").unwrap() - 1.0).abs() < 1e-6,
            "focusing via the label must NOT move the value — it was 1.0 and must stay 1.0; \
             if this drifts, selecting a row is destroying the fit you just dialled in"
        );
    }

    /// The mirror must be a true REFLECTION of the piece's placement, not a sign-flip that
    /// merely looks plausible. Asserted geometrically: transforming a point by the mirrored
    /// fit must equal mirroring the point transformed by the original — `M·fit·M`, with
    /// `M = diag(-1,1,1)`. Sign conventions here are very easy to get subtly wrong (a yaw
    /// that doesn't invert reads as "nearly right" until a glove is on backwards).
    #[test]
    fn mirrored_fit_is_a_true_reflection_across_the_midline() {
        // rot = IDENTITY isolates the user-controlled part; the socket correction belongs to
        // the partner's own bone and is never mirrored.
        let f = PieceFit {
            uniform: 12.0,
            scale: Vec3::new(1.3, 0.8, 1.1),
            offset: Vec3::new(7.0, -3.0, 11.0),
            user_rot: glam::Quat::from_euler(glam::EulerRot::XYZ, 20f32.to_radians(), 35f32.to_radians(), (-50f32).to_radians()),
            rot: glam::Quat::IDENTITY,
        };
        let m = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)); // reflect across x = 0
        let expect = m * f.matrix() * m;
        let got = f.mirrored().matrix();
        for (a, b) in expect.to_cols_array().iter().zip(got.to_cols_array().iter()) {
            assert!(
                (a - b).abs() < 1e-4,
                "mirrored fit must equal M·fit·M (a true reflection)\n  expect {:?}\n  got    {:?}",
                expect.to_cols_array(),
                got.to_cols_array()
            );
        }
    }

    /// Mirroring twice must return the original — otherwise adjusting one half, then the
    /// other, would walk the pair out of symmetry.
    #[test]
    fn mirroring_twice_is_the_identity() {
        let f = PieceFit {
            uniform: 3.5,
            scale: Vec3::new(1.0, 2.0, 0.5),
            offset: Vec3::new(-4.0, 5.0, 6.0),
            user_rot: glam::Quat::from_euler(glam::EulerRot::XYZ, 10f32.to_radians(), (-20f32).to_radians(), 30f32.to_radians()),
            rot: glam::Quat::IDENTITY,
        };
        let b = f.mirrored().mirrored();
        assert!((b.offset - f.offset).length() < 1e-6, "offset must round-trip");
        assert!(b.user_rot.abs_diff_eq(f.user_rot, 1e-6), "rotation must round-trip");
        assert!((b.scale - f.scale).length() < 1e-6, "scale must round-trip");
        assert!((b.uniform - f.uniform).abs() < 1e-6, "size must round-trip");
    }

    /// A reflection must not resize. If scale ever mirrored, one glove would grow.
    #[test]
    fn mirroring_never_changes_size() {
        let f = PieceFit {
            uniform: 44.79,
            scale: Vec3::new(1.4, 0.7, 2.0),
            offset: Vec3::ZERO,
            user_rot: glam::Quat::IDENTITY,
            rot: glam::Quat::IDENTITY,
        };
        let m = f.mirrored();
        assert_eq!(m.scale, f.scale, "a reflection does not resize");
        assert_eq!(m.uniform, f.uniform, "a reflection does not resize");
    }

    /// The Attack button fires a one-shot `attack` only on a click that lands on it, and
    /// ONLY while animating (Attack_1 is a runtime state — the button is meaningless in Pose
    /// mode). A stray fire, or one in Pose mode, would trigger an animation the user didn't ask
    /// for.
    #[test]
    fn attack_button_fires_only_on_click_and_only_while_animating() {
        let h = host();
        let (w, hgt) = (1920.0_f32, 1080.0_f32);
        let u = ui();
        let v = &u["paperdoll"]["view"];
        let g = |k: &str| v[k].as_f64().unwrap() as f32;
        let a = &v["attack"];
        let n_items = v["items"].as_array().unwrap().len() as f32;
        let atk_y = g("top") + n_items * g("row_h") + a["gap_y"].as_f64().unwrap() as f32;
        let center = Vec2::new(
            g("margin") + a["w"].as_f64().unwrap() as f32 * 0.5,
            atk_y + a["h"].as_f64().unwrap() as f32 * 0.5,
        );

        // Animating + a click on the button → fires.
        let mut click = InputState::new();
        click.mouse_position = center;
        click.mouse_left_pressed = true;
        assert!(
            run_hud(&h, &hud_test_model(true, true).with("animate", true), &click, w, hgt)
                .results
                .is_on("attack"),
            "click on Attack fires while animating"
        );

        // Same click, but Pose mode → the button isn't even present, so no fire.
        assert!(
            !run_hud(&h, &hud_test_model(true, true).with("animate", false), &click, w, hgt)
                .results
                .is_on("attack"),
            "Attack must not fire in Pose mode"
        );

        // Hover (no click) while animating → no fire.
        let mut hover = InputState::new();
        hover.mouse_position = center;
        assert!(
            !run_hud(&h, &hud_test_model(true, true).with("animate", true), &hover, w, hgt)
                .results
                .is_on("attack"),
            "hover must not fire"
        );
    }

    /// The Record confirmation must be DRAWN when the engine publishes one, and absent when
    /// it doesn't. A silent write is indistinguishable from a dead button — and a silent
    /// FAILED write is worse, since you'd carry on believing the fit was saved.
    #[test]
    fn record_confirmation_is_drawn_only_while_the_engine_publishes_one() {
        let h = host();
        let (w, hgt) = (1920.0_f32, 1080.0_f32);
        let has_text = |cmds: &[flicker::script::HudCommand], needle: &str| {
            cmds.iter().any(|c| match c {
                flicker::script::HudCommand::Text { text, .. } => text.contains(needle),
                _ => false,
            })
        };

        let status = |msg: &str| {
            run_hud(&h, &fit_model(true).with("fit_status", msg), &InputState::new(), w, hgt).commands
        };
        assert!(has_text(&status("Saved 9 pieces -> inline attach"), "Saved 9 pieces"),
            "the confirmation must be drawn");

        // Blank (the engine expires it) → gone, so a stale "saved" can't be misread as fresh.
        assert!(!has_text(&status(""), "Saved"), "an expired confirmation must disappear");

        // Failures must surface too, not just successes.
        assert!(has_text(&status("SAVE FAILED: permission denied"), "SAVE FAILED"),
            "a failed write must be visible in-window");
    }

    /// A slider drag must CAPTURE the mouse for its whole duration — including after the
    /// cursor leaves the panel. `hud_hit` is what stops the orbit camera taking the same
    /// drag; if it went false mid-drag the view would spin while adjusting a fit.
    #[test]
    fn a_slider_drag_captures_the_mouse_even_off_panel() {
        let h = host();
        let (w, hgt) = (1920.0_f32, 1080.0_f32);
        let u = ui();
        let f = &u["paperdoll"]["fit"];
        let g = |k: &str| f[k].as_f64().unwrap() as f32;
        let y0 = g("margin") + g("pad") + g("header_size") + 6.0;
        let sx = w - g("margin") - g("w") + g("pad") + g("label_w");
        let sw_ = g("w") - g("pad") * 2.0 - g("label_w") - g("value_w");
        let row_y = y0 + (g("row_h") - g("slider_h")) * 0.5;

        // One retained interaction state across the whole gesture (press → drag → release),
        // so the drag capture persists — exactly how the engine holds `ui_state` per frame.
        let tree = h.ui_tree().unwrap().unwrap();
        let styles = load_styles(HUD_UI_ELEMENTS);
        let model = fit_model(true);
        let mut state = UiState::new();
        let mut captures = |mx: f32, my: f32, clicked: bool, down: bool| {
            let snap = UiInput { mouse: Vec2::new(mx, my), clicked, down, screen: Vec2::new(w, hgt), typed: String::new(), backspace: false, wheel: 0.0 };
            run_ui(&tree, &model, &styles, &snap, &mut state).results.is_on("hud_hit")
        };

        // Press on the Size slider captures.
        assert!(
            captures(sx + sw_ * 0.5, row_y + g("slider_h") * 0.5, true, true),
            "press on a slider captures"
        );
        // Still HELD, cursor dragged far away over open scene — capture must persist.
        assert!(
            captures(w * 0.3, hgt * 0.5, false, true),
            "a drag that leaves the panel MUST still capture — else the camera spins mid-adjust"
        );
        // Released over open scene — capture ends, the camera gets the mouse back.
        assert!(
            !captures(w * 0.3, hgt * 0.5, false, false),
            "capture must END on release, or the camera would never work again"
        );
    }

    /// The nudge steps must be FINE. A trackpad drag spans the whole range, so if the arrow
    /// step were a meaningful fraction of it, the arrows would be no better than the drag.
    #[test]
    fn nudge_steps_are_fine_relative_to_the_slider_range() {
        let u = ui();
        let f = &u["paperdoll"]["fit"];
        for group in ["uniform", "scale", "offset", "rotate"] {
            let g = &f[group];
            let (lo, hi) = (g["min"].as_f64().unwrap(), g["max"].as_f64().unwrap());
            let n = g["nudge"].as_f64().unwrap();
            let coarse = g["nudge_coarse"].as_f64().unwrap();
            let frac = n / (hi - lo);
            assert!(
                frac < 0.005,
                "'{group}' nudge {n} is {:.2}% of its range — not fine enough to beat a drag",
                frac * 100.0
            );
            assert!(coarse > n, "'{group}' coarse step must exceed the fine step");
        }
    }

    /// The HUD script must load and run. Without this a Lua error is INVISIBLE: `init`
    /// logs it and leaves `script: None`, so the scene just renders with no HUD at all
    /// and every build/clippy run still passes.
    #[test]
    fn hud_script_runs_with_model() {
        let host = host();
        let frame = run_hud(&host, &hud_test_model(true, false), &InputState::new(), 1920.0, 1080.0);
        // Every slot reports back, so the engine can never miss one.
        for d in SLOTS {
            assert!(
                frame.results.get(&format!("slot_{}", d.key)).is_some(),
                "slot '{}' must report its state back",
                d.key
            );
        }
        assert!(!frame.commands.is_empty(), "hud emits panel + cell + toggle commands");
    }

    /// Clicking a loaded cell equips it; clicking an EMPTY cell must not — a slot with no
    /// piece is inert, so a stray click can't equip something that doesn't exist.
    #[test]
    fn inventory_cell_toggles_only_when_a_piece_is_loaded() {
        let host = host();
        let (w, h) = (1920.0_f32, 1080.0_f32);

        // Centre of the first inventory cell, from the SAME layout the walker resolves:
        // a bottom-anchored column [header, bar]; the bar (a padded row of cells) sits
        // flush with the column bottom, itself offset up by `margin_b`.
        let ui: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(HUD_UI_ELEMENTS).unwrap()).unwrap();
        let inv = &ui["paperdoll"]["inventory"];
        let num = |k: &str| inv[k].as_f64().unwrap() as f32;
        let (slot, gap, pad, mb) = (num("slot"), num("gap"), num("pad"), num("margin_b"));
        let n = SLOTS.len() as f32;
        let bar_w = n * slot + (n - 1.0) * gap + pad * 2.0; // the row includes its own pad
        let col_x = (w - bar_w) * 0.5;
        let cx = col_x + pad + slot * 0.5;
        let cy = (h - mb) - (pad * 2.0 + slot) + pad + slot * 0.5;

        let mut click = InputState::new();
        click.mouse_position = Vec2::new(cx, cy);
        click.mouse_left_pressed = true;

        let first = format!("slot_{}", SLOTS[0].key);
        let res = run_hud(&host, &hud_test_model(true, false), &click, w, h).results;
        assert!(res.is_on(&first), "clicking a LOADED cell equips it");

        let res = run_hud(&host, &hud_test_model(false, false), &click, w, h).results;
        assert!(!res.is_on(&first), "clicking an EMPTY cell must stay unequipped");
    }
}
