//! The Clayworks DOCUMENT and its services — UI-free.
//!
//! [`Document`] is the ONE working document the bench edits: the opened source folder, the
//! parsed model, the conform result and everything the human authored on top of it. Every
//! service here drives one of `flicker-content`'s stages against that document (scan → parse
//! → classify → conform / mount / retarget → bake → commit into STAGING); the scene is a thin
//! behaviour that reads the accessors and calls the services, and the viewport tier draws.
//!
//! **This crate hosts; it does not process.** Every stage is `flicker-content`'s
//! (`scan_folder` → `parse_fbx` → `rename_to_canonical` → `conform_to_canonical` →
//! `bake_rig`). Adding processing logic *here* would fork a pipeline that already exists —
//! the document's job is to drive it and hold its reports.
//!
//! # Its output is STAGED, not shipped
//!
//! Export writes into `content/staging/` via `flicker_content::roots`, never straight
//! into the package the game reads. "I imported an asset" and "the asset ships" are two
//! events now; the Quartermaster promotes the second.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use flicker::render::{Mat4, MeshVertex, Vec2, Vec3};
use flicker::ui::strings;
use flicker_content::{
    bake_rig, bake_skin, classify_asset, conform_to_canonical, decimate_to, default_reference,
    fit_baseline_to_mesh, garment_socket, parse_fbx, rename_to_canonical, reorient_to_canonical,
    scale_mesh_to_stature, scan_folder, write_garment, write_prop, write_rig, AssetClass,
    AssetReport, ConformMode, ConformOutput, Fit, Kind, PropKind, RawModel, RenameReport, Scan,
};
use flicker_mechanics::{autofit_capsules_from, Volume};
// The clip preview's playable form: the retargeter's in-memory output decoded through the
// SAME resolve path `load_dirs` uses, then sampled per frame — no disk round-trip.
use flicker_skeletal::format::{resolve_clips, rig_bones, Bone as SkelBone, ResolvedClip, RigFile};
use flicker_skeletal::pose::global_transforms;

/// The three WORKFLOW names the Task page's cards dispatch between — branching lives
/// BETWEEN definitions, never inside one (the prop rail simply HAS no character-only
/// Attach step). The scene reads [`Document::workflow`] to pick its page; the document
/// only records which one the declared class selected.
///
/// `task` is the ENTRY page of ALL THREE — a workflow-selection card grid (Import
/// Character / Accessory / Prop / Animation). The user DECLARES the workflow there rather
/// than the tool guessing it; choosing a card opens the folder dialog, and ingest → parse
/// → classify → conform all run inline (see [`Document::open`]), so the asset lands
/// DIRECTLY on the rig-edit view.
pub(crate) const WF_CHARACTER: &str = "import_character";
pub(crate) const WF_PROP: &str = "import_prop";
pub(crate) const WF_ANIMATION: &str = "import_animation";

/// The shared idle the bake preview plays, under the package root — the same clip the
/// Controller Tester's pack opens on, so the smoke test judges against the real thing.
pub(crate) const BAKE_PREVIEW_CLIP: &str = "retarget/clips/locomotion/In-Place/idle_neutral.json";

/// The canonical rig's bone count — the BAKED figure, which is what `flicker.rig` carries and
/// what every other part of the engine quotes. Canon value; the
/// `reference_rig_still_has_the_canonical_bone_count` test asserts it against the reference file
/// itself, so this cannot drift away from the content it describes.
// THE canon count — read from the authored baseline table, never restated.
pub(crate) const REFERENCE_BONES: usize = flicker_content::baseline::CANON_BONES;

/// What a CONFORMED model carries, one short of the canonical count: `root` is synthesized at
/// bone 0 by `bake_rig`, not by conform, so a 65-bone conform result is complete. Deriving it
/// here keeps the two figures from being independently maintained.
pub(crate) const CONFORMED_BONES: usize = REFERENCE_BONES - 1;

/// The WORKING MODEL — the one skeleton the document owns, from Analyze onward.
///
/// Conform mutates `model` in place (rename → derive → reorient → infer) and the viewport
/// frames are re-derived from it; there is deliberately no second copy of the skeleton to drift
/// against. `verts`/`tris` are measured once at parse and unchanged by conform.
pub(crate) struct Parsed {
    pub(crate) model: RawModel,
    pub(crate) verts: usize,
    pub(crate) tris: usize,
    /// Rest-pose world frames + parent topology, for the viewport skeleton. Cached — rebuilt
    /// when the model or an authored offset changes, never per frame.
    pub(crate) globals: Vec<Mat4>,
    pub(crate) parents: Vec<i32>,
    /// Bounding centre. The quad cameras all target the ORIGIN, which in Z-up ground reckoning is
    /// the asset's FEET — so the viewport draws everything offset by `-centre` to frame the asset.
    pub(crate) centre: Vec3,
    /// Half-extent about `centre`, to frame the orthographic views.
    pub(crate) radius: f32,
    /// The asset's feet plane in RECENTRED space (negative) — where the stage floor is drawn.
    pub(crate) floor: f32,
    /// Auto-fit collision volumes (per-bone capsules + leaf-bone spheres), rebuilt with the pose so
    /// the `Collision` overlay shows the coverage the rig currently produces. Empty for a bone-less
    /// prop. The SAME `flicker-mechanics` auto-fit the paperdoll and the runtime bridge use.
    pub(crate) collision: Vec<Volume>,
}

impl Parsed {
    pub(crate) fn new(model: RawModel) -> Self {
        let verts = model.vertices.len();
        let tris = model.indices.len() / 3;
        let mut p = Self {
            model,
            verts,
            tris,
            globals: Vec::new(),
            parents: Vec::new(),
            centre: Vec3::ZERO,
            radius: 1.0,
            floor: 0.0,
            collision: Vec::new(),
        };
        p.rebuild(&[]);
        p
    }

    pub(crate) fn bones(&self) -> usize {
        self.model.bones.len()
    }

    /// Re-derive the world frames, applying the authored per-bone offsets on top of the
    /// conformed rest pose. `offsets` is empty until the Conform stage authors any.
    pub(crate) fn rebuild(&mut self, offsets: &[BoneOffset]) {
        let (globals, parents) = rest_globals(&self.model, offsets);
        let (centre, radius, floor) = model_bounds(&self.model, &globals);
        self.centre = centre;
        self.radius = radius;
        self.floor = floor;
        // Auto-fit the collision coverage from the SAME topology + rest frames the overlay draws, so
        // toggling `Collision` shows the capsules/spheres this pose would produce. Rebuilt with the
        // pose (cheap) rather than once, so an authored bone offset moves its volume too.
        self.collision = autofit_capsules_from(&parents, &globals);
        self.globals = globals;
        self.parents = parents;
    }

    /// Index of a bone by canonical name — how an attach point finds its parent.
    pub(crate) fn bone_index(&self, name: &str) -> Option<usize> {
        self.model.bones.iter().position(|b| b.name == name)
    }
}

/// One bone's authored correction, applied on top of the conform result. This is the
/// AUTHORED data; the posed skeleton is derived from it, so "Reset bone" is just zeroing it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct BoneOffset {
    /// Translation in source units (cm), parent-relative — the same space as `RawBone::translation`.
    pub(crate) t: [f32; 3],
    /// Roll about the bone's own X axis, in degrees.
    pub(crate) roll: f32,
}

impl BoneOffset {
    fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// How a bone came to be in the conformed rig — what colours its row in the bone map.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MapState {
    /// Carried over from the source and renamed to a canonical name.
    Ok,
    /// Placed by a derive pass whose result is worth a human's eye (hip / shoulder / ankle).
    Review,
    /// Not in the source at all — inferred from the reference rig.
    Auto,
}

impl MapState {
    /// Row tag `$token` — resolved where the bone row is composed.
    pub(crate) fn tag(self) -> &'static str {
        match self {
            MapState::Ok => "$ap_tag_mapped",
            MapState::Review => "$ap_tag_review",
            MapState::Auto => "$ap_tag_auto",
        }
    }
}

/// The conform result plus what the human authored on top of it.
pub(crate) struct Rig {
    pub(crate) rename: RenameReport,
    pub(crate) out: ConformOutput,
    /// Per-bone provenance, parallel to the working model's bones.
    pub(crate) map: Vec<MapState>,
    /// Per-bone authored corrections, parallel to the working model's bones.
    pub(crate) offsets: Vec<BoneOffset>,
    /// Selected row in the bone map.
    pub(crate) sel: usize,
}

impl Rig {
    fn counts(&self) -> (usize, usize, usize) {
        let n = |s: MapState| self.map.iter().filter(|m| **m == s).count();
        (n(MapState::Ok), n(MapState::Review), n(MapState::Auto))
    }
}

/// One authored attach point: a named socket at an offset from a real canonical bone.
///
/// The parent bones are all canonical (`hand_r`, `thigh_l`, `spine_02`, …), so a point is fully
/// defined against the conformed skeleton. Persisting the SET of them is what `flicker.rig` cannot
/// carry yet — its `attach` block is a single mount describing how one asset hangs off a socket,
/// not a list of sockets a character offers. Review reports that gap rather than papering over it.
pub(crate) struct AttachPoint {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) parent: &'static str,
    pub(crate) offset: [f32; 3],
    /// The parent's index in the working model, resolved ONCE when the rig gains canonical names.
    /// Looking it up by name per frame would be 6 points × 65 string compares every frame, in a
    /// panel that only changes when conform runs.
    pub(crate) bone: Option<usize>,
}

/// The six points the design specifies, in rail order. Labels are `$token`s,
/// resolved where the attach rows are composed (the Model-channel strings gate).
pub(crate) const ATTACH_POINTS: [(&str, &str, &str); 6] = [
    ("hand_r", "$ap_grip_hand_r", "hand_r"),
    ("hand_l", "$ap_grip_hand_l", "hand_l"),
    ("holster_r", "$ap_holster_hip_r", "thigh_r"),
    ("holster_l", "$ap_holster_hip_l", "thigh_l"),
    ("scabbard", "$ap_scabbard_back", "spine_02"),
    ("belt", "$ap_belt_waist", "pelvis"),
];

/// Candidate mount sockets a PROP or GARMENT can hang from — the body bones the fit stage offers
/// as its picker (a non-character asset mounts to ONE socket, unlike the character's six points).
/// Curated to the common canonical bones + the dedicated `Weapon_R/L` sockets; the choice is
/// validated against the loaded base body at bake time, so a missing bone surfaces as a commit
/// error rather than a silent mis-mount.
pub(crate) const SOCKETS: &[(&str, &str)] = &[
    // (canonical bone, display-label `$token` — resolved where the picker rows compose)
    ("hand_r", "$ap_hand_r"),
    ("hand_l", "$ap_hand_l"),
    ("Weapon_R", "$ap_weapon_socket_r"),
    ("Weapon_L", "$ap_weapon_socket_l"),
    ("spine_02", "$ap_chest"),
    ("spine_03", "$ap_upper_chest"),
    ("pelvis", "$ap_pelvis"),
    ("neck_01", "$ap_neck"),
    ("head", "$ap_head"),
    ("clavicle_l", "$ap_shoulder_l"),
    ("thigh_r", "$ap_thigh_r"),
    ("thigh_l", "$ap_thigh_l"),
    ("calf_l", "$ap_shin_l"),
    ("calf_r", "$ap_shin_r"),
    ("foot_l", "$ap_foot_l"),
    ("foot_r", "$ap_foot_r"),
    ("lowerarm_l", "$ap_forearm_l"),
    ("lowerarm_r", "$ap_forearm_r"),
];

/// A prop/garment's authored placement — the human-in-the-loop fit the Attach stage tunes for a
/// NON-character asset (Skin uses the six attach points + per-bone offsets instead). `socket`
/// indexes [`SOCKETS`]; `rot` is euler degrees; `scale` is PER-AXIS and `uniform` is scale-all
/// (the paperdoll fit gadget's X/Y/Z + scale-all, which the rig format already carried). Baked into
/// the rig's `attach` block (prop) or the skin transform (garment) at Commit — what the user
/// approved is what ships.
#[derive(Clone, Copy)]
pub(crate) struct PropFit {
    pub(crate) socket: usize,
    pub(crate) offset: [f32; 3],
    pub(crate) rot: [f32; 3],
    pub(crate) scale: [f32; 3],
    pub(crate) uniform: f32,
}

impl Default for PropFit {
    fn default() -> Self {
        Self {
            socket: 0,
            offset: [0.0; 3],
            rot: [0.0; 3],
            scale: [1.0; 3],
            uniform: 1.0,
        }
    }
}

impl PropFit {
    pub(crate) fn socket_name(&self) -> &'static str {
        SOCKETS
            .get(self.socket)
            .map(|(id, _)| *id)
            .unwrap_or("pelvis")
    }
}

/// The Animation workflow's WORKING STATE — the active BVH retargeted onto the reference
/// skeleton IN MEMORY, both variants resolved and playable, plus the exact emitted JSON so
/// Commit writes precisely what the preview showed. Built by `prepare_clip` (idempotent,
/// the sibling of `analyze`/`conform`); a pick clears it to re-run.
pub(crate) struct ClipPreview {
    /// The reference skeleton the clips were baked onto (decoded from the emitted clip).
    pub(crate) bones: Vec<SkelBone>,
    /// Per-bone parent indices, in `bones` order — the overlay helpers' shape.
    pub(crate) parents: Vec<i32>,
    pub(crate) ip: ResolvedClip,
    pub(crate) rm: ResolvedClip,
    /// Ticks — both variants share it (same source frames, same 60 Hz canon).
    pub(crate) duration: u32,
    /// Rest-pose framing: half-extent, ground height, and centre.
    pub(crate) radius: f32,
    pub(crate) floor: f32,
    pub(crate) ip_center: Vec3,
    /// RootMotion framing widened to the pelvis's planar TRAVEL, so the walk stays in shot.
    pub(crate) rm_center: Vec3,
    pub(crate) rm_radius: f32,
    /// The retargeter's verbatim output — what Commit writes (no re-run, no drift).
    pub(crate) variants: flicker_content::retarget::ClipVariants,
}

impl ClipPreview {
    /// Decode the retargeter's in-memory output into a playable preview: parse both clip
    /// JSONs, take the embedded reference skeleton, resolve the tracks through the SAME
    /// path `load_dirs` uses, and derive the two panels' framing.
    fn resolve(variants: flicker_content::retarget::ClipVariants) -> Result<Self, String> {
        let ip_file: RigFile = serde_json::from_value(variants.in_place.clone())
            .map_err(|e| format!("in-place clip: {e}"))?;
        let rm_file: RigFile = serde_json::from_value(variants.root_motion.clone())
            .map_err(|e| format!("root-motion clip: {e}"))?;
        let bones = rig_bones(&ip_file);
        if bones.is_empty() {
            return Err("clip carries no skeleton".into());
        }
        let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
        let ip = resolve_clips(&ip_file, &bones, false)
            .pop()
            .ok_or("in-place clip resolved empty")?;
        let rm = resolve_clips(&rm_file, &bones, false)
            .pop()
            .ok_or("root-motion clip resolved empty")?;
        let duration = ip.duration_ticks.max(rm.duration_ticks).max(1);

        // Rest framing from the skeleton's own joint extent — a clip has no mesh.
        let rest_locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        let rest = global_transforms(&bones, &rest_locals);
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for g in &rest {
            let p = g.w_axis.truncate();
            min = min.min(p);
            max = max.max(p);
        }
        let radius = ((max - min).length() * 0.5).max(1.0);
        let ip_center = (min + max) * 0.5;

        // The RootMotion panel frames the TRAVEL: the pelvis carries the planar
        // translation (In-Place pins exactly that), so its key extent widens the shot.
        let (mut tmin, mut tmax) = (Vec2::ZERO, Vec2::ZERO);
        if let Some(pi) = bones.iter().position(|b| b.name == "pelvis") {
            if let Some(tr) = rm.tracks.iter().find(|t| t.bone == pi) {
                for k in &tr.keys {
                    let p = Vec2::new(k.translation[0], k.translation[1]);
                    tmin = tmin.min(p);
                    tmax = tmax.max(p);
                }
            }
        }
        let travel = (tmax + tmin) * 0.5;
        Ok(Self {
            rm_center: ip_center + Vec3::new(travel.x, travel.y, 0.0),
            rm_radius: radius + (tmax - tmin).length() * 0.5,
            bones,
            parents,
            ip,
            rm,
            duration,
            radius,
            floor: min.z,
            ip_center,
            variants,
        })
    }
}

/// The loaded source folder — what Load produced, plus what each later stage added.
pub(crate) struct Source {
    pub(crate) dir: PathBuf,
    pub(crate) scan: Scan,
    /// The riggable mesh chosen to rig.
    pub(crate) fbx: PathBuf,
    /// EVERY riggable mesh the scan found — a weapon set is four or five pieces, an outfit folder is
    /// tops/pants/gloves/shoes — plus which one is selected. The Load stage offers the choice rather
    /// than refusing the folder; only a single-mesh folder skips straight past it.
    pub(crate) candidates: Vec<PathBuf>,
    pub(crate) candidate_sel: usize,
    pub(crate) textures: usize,
    pub(crate) parsed: Option<Parsed>,
    /// What Classify detected, and the override the user may have applied over it.
    pub(crate) report: Option<AssetReport>,
    pub(crate) class: Option<AssetClass>,
    pub(crate) prop: PropKind,
    /// What Conform produced — `None` until the stage runs.
    pub(crate) rig: Option<Rig>,
    /// Where a re-opened rig came from ("staging" / "package"), `None` on the vendor-FBX
    /// path — surfaced so a staged reload is never silent about its source (Aaron
    /// 2026-08-20: the promoted fit lives in PACKAGE after Quartermaster's move-only
    /// promote empties staging, and a silent fallthrough read as lost work).
    pub(crate) reopened: Option<&'static str>,
    /// Authored attach points (always the six; `parent` resolves against the conformed rig).
    pub(crate) attach: Vec<AttachPoint>,
    /// Selected attach point.
    pub(crate) attach_sel: usize,
    /// The prop/garment mount fit — socket + offset/rotation/scale — authored in the Attach stage
    /// for a non-character asset. Unused by the Skin path (which uses `attach` + bone offsets).
    pub(crate) fit: PropFit,
    /// Where Commit wrote the rig, once it has.
    pub(crate) committed: Option<PathBuf>,
    /// Set when a stage failed, surfaced instead of a fabricated result.
    pub(crate) error: Option<String>,
    /// The Animation workflow's retargeted, playable preview — `None` until
    /// `prepare_clip` runs (and for every other class, always).
    pub(crate) clip: Option<ClipPreview>,
}

impl Source {
    /// The asset name the pipeline would bake under — the source folder's own name.
    pub(crate) fn asset_name(&self) -> &str {
        self.dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("asset")
    }

    pub(crate) fn file_name(&self) -> &str {
        self.fbx.file_name().and_then(|s| s.to_str()).unwrap_or("")
    }

    /// The effective classification: the user's override if they made one, else what was detected.
    pub(crate) fn class(&self) -> Option<AssetClass> {
        self.class.or(self.report.as_ref().map(|r| r.class))
    }

    /// World position of an attach point — its parent bone's conformed frame plus the authored
    /// offset. `None` while the point has no parent bone (before conform runs, the source carries
    /// vendor names, so nothing resolves).
    fn attach_world(&self, i: usize) -> Option<Vec3> {
        let p = self.attach.get(i)?;
        let g = self.parsed.as_ref()?.globals.get(p.bone?)?;
        Some(g.w_axis.truncate() + Vec3::from_array(p.offset))
    }

    /// Bind every attach point to its parent bone. Called when the working model's names change
    /// — i.e. once, after conform.
    fn resolve_attach(&mut self) {
        let Some(parsed) = self.parsed.as_ref() else {
            return;
        };
        for p in &mut self.attach {
            p.bone = parsed.bone_index(p.parent);
        }
    }
}

/// The Prep step's working set for one raw source: the source mesh verbatim (the 100% every
/// target is measured against — the working `Parsed` is overwritten by the prepped mesh, so
/// this is the only pristine copy), and the mesh at the target last applied, cached so a
/// stature change re-scales without re-collapsing a 100K mesh.
pub(crate) struct PrepCache {
    key: (PathBuf, usize),
    source: RawModel,
    pub(crate) source_tris: usize,
    /// The triangle target last APPLIED (the source count after RESET).
    applied: usize,
    /// The source collapsed to `applied`, unscaled.
    decimated: RawModel,
}

/// The bench's document: the opened source and everything authored over it, plus the
/// declared-on-Task preferences the services read. UI-free — the scene publishes it
/// through the accessors below and the viewport tier reads its frames.
pub(crate) struct Document {
    pub(crate) source: Option<Source>,
    /// The class the user DECLARED on the Task page — the class Load stamps onto the source
    /// instead of auto-detecting it. `None` before a card is chosen (and on the character/default
    /// path, which `workflow_for(None)` already treats as a character).
    pub(crate) pending_class: Option<AssetClass>,
    /// The Prop SUB-TYPE the user DECLARED on the Import panel — the clothing-vs-static fork that
    /// decides the bake path (`Clothing` → `write_garment`; anything else → `write_prop`). Set by
    /// the Accessory/Prop cards; `None` (→ `PropKind::Accessory` default) for Character/Animation.
    pub(crate) pending_prop: Option<PropKind>,
    /// Which workflow the declared class dispatched to — one of [`WF_CHARACTER`] /
    /// [`WF_PROP`] / [`WF_ANIMATION`]; the character rail before any card is chosen.
    pub(crate) workflow: &'static str,
    /// Task-page toggle (Aaron 2026-08-20): before parsing the chosen folder's FBX, look for
    /// this asset's already-STAGED rig (`staging/characters/<name>/<name>.json`) and re-open
    /// THAT instead — the re-process loop, so an already-fitted body can be adjusted further
    /// and re-committed without redoing the joint work from the vendor source.
    pub(crate) prefer_staged: bool,
    /// Import DIAGNOSTIC (Task page): stage this vendor rig EXACTLY as provided — skip the joint
    /// derivation and the reorient in [`conform_to_canonical`] (`ConformMode::AsProvided`), keeping
    /// Meshy's own skeleton and skin, and only completing the bone set. Off by default; the standard
    /// canonical conform runs unless a human opts into this to test the raw rig against the clips.
    pub(crate) as_provided: bool,
    /// The side-by-side pick — what Commit keeps (ruled: one, the other, or both).
    pub(crate) variant_rm: bool,
    pub(crate) variant_ip: bool,
    /// Symmetry: an ortho reposition of a left/right joint mirrors to its `_l`/`_r` twin. Default on;
    /// a toggle turns it off to move one joint alone.
    pub(crate) mirror_joints: bool,
    /// PREP stage: the target stature (cm) a raw mesh is resized to before rigging (a live
    /// slider), and the decimation TARGET triangle count as typed (digits only; Aaron
    /// 2026-09-03: a count, not a percent — applied on the APPLY button, never live).
    pub(crate) stature_cm: f32,
    pub(crate) decimate_target: String,
    /// The Prep cache for the current source — the pristine boneless mesh plus the decimation
    /// last APPLIED to it, keyed by the source identity (folder + picked candidate) so it is
    /// cut once per piece. Boneless (raw) meshes only; a mesh that arrives rigged is game-ready
    /// and Prep leaves it untouched.
    pub(crate) prep: Option<PrepCache>,
    /// Bumped on every authored-offset write (a Conform slider or a gizmo drag) — the skinned-mesh
    /// cache key, so the live re-skin re-uploads exactly when the pose changes and never otherwise.
    pub(crate) pose_gen: u64,
    /// Bumped when the working MESH GEOMETRY changes (Prep decimation / stature scale), so the rest
    /// preview re-uploads exactly then — offset edits (which bump `pose_gen`) leave the rest mesh be.
    pub(crate) mesh_gen: u64,
}

impl Document {
    /// An empty document: nothing open, the character workflow, every preference at its
    /// default (variants both picked, mirror on, the canonical stature).
    pub(crate) fn new() -> Self {
        Self {
            source: None,
            pending_class: None,
            pending_prop: None,
            workflow: WF_CHARACTER,
            prefer_staged: false,
            as_provided: false,
            variant_rm: true,
            variant_ip: true,
            mirror_joints: true,
            stature_cm: flicker_content::baseline::STATURE,
            decimate_target: String::new(),
            prep: None,
            pose_gen: 0,
            mesh_gen: 0,
        }
    }

    /// The six attach points, authored fresh for a newly loaded asset.
    fn new_attach() -> Vec<AttachPoint> {
        ATTACH_POINTS
            .iter()
            .map(|(id, label, parent)| AttachPoint {
                id,
                label,
                parent,
                offset: [0.0; 3],
                bone: None,
            })
            .collect()
    }

    /// The native open-folder dialog — the Load step's ONE dialog seam. `None` = cancelled.
    #[cfg(not(test))]
    pub(crate) fn pick_folder() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Open asset source folder")
            .pick_folder()
    }

    /// The headless test build has no OS dialog to block on: every pick is CANCELLED, so
    /// what a test exercises is the cancel path (`load_folder` stays put); the folder itself
    /// is handed to [`Self::open`] directly.
    #[cfg(test)]
    pub(crate) fn pick_folder() -> Option<PathBuf> {
        None
    }

    /// Ingest a folder that has already been chosen. Split from the dialog so the whole wizard
    /// downstream of it is exercisable without a GUI.
    pub(crate) fn open(&mut self, dir: PathBuf) {
        match scan_folder(&dir) {
            Ok(scan) => {
                let textures = scan.of_kind(Kind::Texture).count();
                // EVERY riggable mesh, not only an unambiguous one: a weapon set holds four or five
                // pieces and an outfit folder holds tops/pants/gloves/shoes, so the document OFFERS
                // the choice (the Load picker) instead of refusing the folder. The first is
                // pre-selected so the wizard is never stuck. The ANIMATION workflow's candidates
                // are the folder's BVH clips instead — same picker, different kind.
                let (candidates, error): (Vec<PathBuf>, Option<String>) = if self.pending_class
                    == Some(AssetClass::Animation)
                {
                    let c: Vec<PathBuf> = scan.of_kind(Kind::Bvh).map(|e| e.path.clone()).collect();
                    let e = c
                        .is_empty()
                        .then(|| format!("No BVH clips in {}", dir.display()));
                    (c, e)
                } else {
                    let c: Vec<PathBuf> = scan.candidates().map(|e| e.path.clone()).collect();
                    let e = c
                        .is_empty()
                        .then(|| format!("No riggable mesh in {}", dir.display()));
                    (c, e)
                };
                let fbx = candidates.first().cloned().unwrap_or_default();
                tracing::info!(
                    "scanned {}: {} entries, {} riggable, {textures} textures",
                    dir.display(),
                    scan.entries.len(),
                    scan.riggable.len()
                );
                let ok = error.is_none();
                self.source = Some(Source {
                    dir,
                    scan,
                    fbx,
                    candidates,
                    candidate_sel: 0,
                    textures,
                    parsed: None,
                    report: None,
                    // The class is DECLARED on the Task page, not guessed here — stamped from the
                    // workflow the user chose so the whole flow is intent-driven, not auto-detected.
                    class: self.pending_class,
                    // The sub-type is DECLARED on the Import panel (Accessory → garment/worn, Prop →
                    // static) exactly like the class — not guessed. `None` keeps the historical default.
                    prop: self.pending_prop.unwrap_or(PropKind::Accessory),
                    rig: None,
                    reopened: None,
                    attach: Self::new_attach(),
                    attach_sel: 0,
                    fit: PropFit::default(),
                    committed: None,
                    error,
                    clip: None,
                });
                // The DISPATCH: the class declared on the Task page picks WHICH workflow runs
                // (character rail vs the attach-less prop rail) — then the asset lands DIRECTLY
                // on the rig-edit view, single- or multi-mesh alike, with zero extra clicks. A
                // scan error lands there too, surfaced as the "Blocked:" line rather than a dead
                // Load page. When the folder holds several riggable meshes the rig stage shows
                // the inline piece picker; the first is pre-selected so the view is never empty.
                self.dispatch_workflow(Self::workflow_for(self.pending_class));
                if ok {
                    // The Task page's staged-reload preference: adopt the asset's already-staged
                    // rig when one exists, and `analyze`/`conform` below become no-ops (both
                    // early-return once their outputs are present). Absence falls through to the
                    // vendor-FBX path unchanged.
                    if self.prefer_staged {
                        self.adopt_staged();
                    }
                    self.analyze();
                    // conform() is idempotent and early-returns for Prop/Animation, so a model gets
                    // its rig here and a prop/animation simply lands on its own Conform role page
                    // (Mount / Clips) — the source-generic dispatch. A RAW (boneless) mesh is not
                    // rigged here: its install waits for the Rig step's own `conform`, after Prep.
                    self.run_conform(false);
                }
            }
            Err(e) => tracing::error!("scan failed: {e}"),
        }
    }

    /// Re-open the asset's already-PROCESSED rig instead of the vendor FBX — the Task page's
    /// "prefer staged" toggle (Aaron 2026-08-20: the re-process loop, so an already-fitted
    /// body is adjusted further instead of redoing the joint work from the source). Character
    /// path only. Searches STAGING first (in-progress work wins), then PACKAGE — because the
    /// Quartermaster's promote is MOVE-only, a promoted fit's ONE copy lives in package and
    /// staging is empty, and a staging-only search silently fell through to a fresh un-fitted
    /// conform (Aaron hit exactly this). The processed file is the bake's own output, so it
    /// loads ALREADY-CONFORMED: `rig` is pre-filled (every bone-map row Ok, zero authored
    /// offsets), which makes the `conform()` that `open` runs next a no-op — re-running the
    /// derive passes would move the human's fitted joints. Nothing processed anywhere is
    /// normal (a first import) and falls through to the FBX path.
    pub(crate) fn adopt_staged(&mut self) {
        let package_characters = flicker_content::roots().package().join("characters");
        for (root, origin) in [
            (characters_dir(), "staging"),
            (package_characters, "package"),
        ] {
            if self.adopt_staged_from(&root, origin) {
                return;
            }
        }
    }

    /// [`Self::adopt_staged`] against one explicit root — so the reload path is exercisable
    /// against a scratch directory instead of the live trees, like `commit_to`. Returns whether
    /// the rig was adopted; `origin` is surfaced as the provenance line.
    pub(crate) fn adopt_staged_from(&mut self, root: &Path, origin: &'static str) -> bool {
        if matches!(
            self.pending_class,
            Some(AssetClass::Prop | AssetClass::Animation)
        ) {
            return false;
        }
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let name = src.asset_name().to_string();
        let path = root.join(&name).join(format!("{name}.json"));
        let mut model = match flicker_content::load_rig_raw(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::info!("no {origin} rig to re-open for {name}: {e:#}");
                return false;
            }
        };
        if model.bones.is_empty() {
            tracing::warn!("{origin} {name} carries no skeleton — falling through");
            return false;
        }
        // Chain repair on the way in: a rig staged BEFORE the splice fix carries the broken
        // chain in its bake (the golem's head hung off `neck_01`, dangling `neck_02`). The
        // splice preserves every fitted joint's world frame — only the composition is fixed —
        // so reloading is also how an existing fit is healed without redoing it.
        match flicker_content::splice_canonical_chain(&mut model, &default_reference()) {
            Ok(spliced) if !spliced.is_empty() => {
                tracing::info!("staged {name}: spliced {spliced:?} onto the canonical chain");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("staged {name}: chain check skipped: {e:#}"),
        }
        let n = model.bones.len();
        let parsed = Parsed::new(model);
        src.report = Some(classify_asset(&src.scan, Some(parsed.bones())));
        src.parsed = Some(parsed);
        src.rig = Some(Rig {
            rename: RenameReport::default(),
            out: ConformOutput::default(),
            map: vec![MapState::Ok; n],
            offsets: vec![BoneOffset::default(); n],
            sel: 0,
        });
        src.reopened = Some(origin);
        src.resolve_attach();
        src.error = None;
        tracing::info!("re-opened {origin} rig {name}: {n} bones");
        true
    }

    /// ANALYZE — parse the chosen FBX and measure it. Synchronous today, so a large
    /// source hitches one frame; folding the stages onto `flicker-worker::WorkerPool` is
    /// FDD Layer B and deliberately not started here.
    pub(crate) fn analyze(&mut self) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        // An Animation source's candidates are BVH files, not FBX meshes — its stage
        // runner is `prepare_clip`, the sibling of this one, so there is nothing to parse.
        if src.class() == Some(AssetClass::Animation) {
            return;
        }
        if src.parsed.is_some() || src.fbx.as_os_str().is_empty() {
            return;
        }
        match parse_fbx(&src.fbx) {
            Ok(model) => {
                tracing::info!(
                    "parsed {}: {} bones, {} verts",
                    src.fbx.display(),
                    model.bones.len(),
                    model.vertices.len()
                );
                let parsed = Parsed::new(model);
                // Classify sharpens the moment the skeleton is known — the bone count is its
                // deciding signal, so it is derived here rather than re-guessed per frame.
                let report = classify_asset(&src.scan, Some(parsed.bones()));
                // Seed the fit's STARTING socket from what was detected, so the Attach stage opens
                // on a sensible mount the user then confirms or moves — a weapon at the hand, a
                // garment at its body region, an accessory at the chest.
                let name = src
                    .dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let start = match report.class {
                    AssetClass::Prop if report.prop == PropKind::Clothing => garment_socket(&name),
                    AssetClass::Prop if report.prop == PropKind::Weapon => "hand_r",
                    AssetClass::Prop => "spine_02",
                    _ => "hand_r",
                };
                src.fit.socket = SOCKETS.iter().position(|(id, _)| *id == start).unwrap_or(0);
                src.report = Some(report);
                src.parsed = Some(parsed);
                src.error = None;
            }
            Err(e) => src.error = Some(format!("Parse failed: {e}")),
        }
    }

    /// CONFORM — rename to canonical names, then run the full conform against the reference rig,
    /// and read the per-bone provenance straight out of the reports. Runs once when the Rig
    /// stage is reached; the sliders then author on top of its result. On a RAW (boneless)
    /// mesh this is where the canon is installed — the stage is entered only after Prep, so
    /// it rigs the decimated, stature-scaled geometry.
    pub(crate) fn conform(&mut self) {
        self.run_conform(true);
    }

    /// The conform stage proper. `prep_done` is the one piece of wizard state it consumes: a
    /// RAW (boneless) mesh is rigged only once the user has passed the Prep step — installing
    /// on the un-prepped mesh would rig the wrong scale and triangle count — so [`Self::conform`]
    /// (the stage entry) passes `true` and [`Self::open`]'s inline run passes `false`. A mesh
    /// that arrives with a skeleton ignores it.
    fn run_conform(&mut self, prep_done: bool) {
        // Read the import mode BEFORE borrowing `source`: the canonical path corrects the vendor
        // rig onto the reference; as-provided stages it untouched (Aaron's raw-rig diagnostic).
        let mode = if self.as_provided {
            ConformMode::AsProvided
        } else {
            ConformMode::Canonical
        };
        // The raw-mesh rig path needs the target stature (set on Prep). Read before borrowing
        // `source`.
        let stature = self.stature_cm;
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if src.rig.is_some() {
            return;
        }
        // Conform is the CHARACTER path — it maps a biped skeleton onto the canonical
        // reference. A Prop or Animation has no such skeleton, so the wizard routes by the
        // confirmed class rather than forcing every asset through it: running it on a
        // skeleton-less mesh is exactly the misleading "no skeleton" failure. Their bake
        // paths (prop fit / clip retarget) are their own stages, and the stage says so.
        // (An unclassified asset falls through to the character path, as it always has.)
        if matches!(src.class(), Some(AssetClass::Prop | AssetClass::Animation)) {
            return;
        }
        let Some(parsed) = src.parsed.as_mut() else {
            return;
        };
        // RAW MESH (no skeleton): install the authored canon scaled to the target stature and bake
        // fresh skin — the boneless rig path (Aaron 2026-08-22). Deferred until the Prep stage is
        // done so it rigs the decimated, stature-scaled geometry. The bind IS the authored canon by
        // construction (uniform stature scale, no mesh-fit, no rolled-back pose_mesh_to_canon).
        if parsed.model.bones.is_empty() {
            if !prep_done {
                return;
            }
            scale_mesh_to_stature(&mut parsed.model, stature);
            fit_baseline_to_mesh(&mut parsed.model, stature);
            // Rough skin first so the HIP fit reads flesh by OWNERSHIP: a bare Z-band at hip height
            // catches the A-posed hands (they hang there), but the weight test excludes them because
            // they belong to the hand bones. Then re-skin from the fitted hips.
            bake_skin(&mut parsed.model);
            let _ = flicker_content::derive_hip_placement(&mut parsed.model);
            bake_skin(&mut parsed.model);
            let n = parsed.model.bones.len();
            parsed.rebuild(&[]);
            tracing::info!("installed canon on raw mesh: {n} bones at {stature}cm, skinned");
            src.rig = Some(Rig {
                rename: RenameReport::default(),
                out: ConformOutput::default(),
                map: vec![MapState::Ok; n],
                offsets: vec![BoneOffset::default(); n],
                sel: 0,
            });
            src.resolve_attach();
            src.error = None;
            return;
        }
        let rename = rename_to_canonical(&mut parsed.model);
        match conform_to_canonical(&mut parsed.model, &default_reference(), mode) {
            Ok(out) => {
                let map = bone_map_states(&parsed.model, &out);
                let n = parsed.model.bones.len();
                parsed.rebuild(&[]);
                tracing::info!(
                    "conformed {}: {} bones, {} inferred, {} renamed, {} unmapped",
                    src.fbx.display(),
                    n,
                    out.infer.added.len(),
                    rename.renamed,
                    rename.unmapped.len()
                );
                src.rig = Some(Rig {
                    rename,
                    out,
                    map,
                    offsets: vec![BoneOffset::default(); n],
                    sel: 0,
                });
                // The bones now carry canonical names, so the attach points can bind to them.
                src.resolve_attach();
                src.error = None;
            }
            Err(e) => src.error = Some(format!("Conform failed: {e}")),
        }
    }

    /// CLIP — the Animation workflow's stage runner, the sibling of `analyze`/`conform`:
    /// retarget the ACTIVE BVH onto the reference skeleton IN MEMORY and resolve both
    /// variants for playback. Nothing touches disk until Commit. Idempotent (a pick
    /// clears `clip` to re-run); a failure surfaces as the error, never invents.
    pub(crate) fn prepare_clip(&mut self) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if src.clip.is_some()
            || src.class() != Some(AssetClass::Animation)
            || src.fbx.as_os_str().is_empty()
        {
            return;
        }
        let built = flicker_content::retarget::build_variants(&src.fbx, &default_reference())
            .map_err(|e| e.to_string())
            .and_then(ClipPreview::resolve);
        match built {
            Ok(cp) => {
                tracing::info!(
                    "retargeted {}: {} ticks, {} bones",
                    src.fbx.display(),
                    cp.duration,
                    cp.bones.len()
                );
                src.clip = Some(cp);
                src.error = None;
            }
            Err(e) => src.error = Some(format!("Clip retarget failed: {e}")),
        }
    }

    /// The character model EXACTLY as Commit bakes it: the working model cloned, the
    /// authored offsets applied, and every joint's frame translated onto the canon.
    ///
    /// THE ONE BAKE PATH — `commit_to` writes it and the Preview page plays it, so the
    /// preview can never drift from the export. The frame translation is the invariant's
    /// output gate (shared clips play absolute rotations in canonical frames): positions
    /// ship exactly as placed — Meshy's and the human's fitted joints alike — and only
    /// each bone's frame is rewritten, which is what lets the as-provided editing view
    /// stay vendor-faithful in the bench yet still produce a playable body. Idempotent on
    /// an already-canonical rig with no authored offsets; when joints WERE dragged, the
    /// limb frames re-align to the final joint layout.
    pub(crate) fn character_bake_model(&self) -> Result<RawModel, String> {
        let src = self.source.as_ref().ok_or("no source is open")?;
        let parsed = src.parsed.as_ref().ok_or("nothing is parsed")?;
        let mut model = parsed.model.clone();
        if let Some(rig) = src.rig.as_ref() {
            apply_offsets(&mut model, &rig.offsets);
        }
        reorient_to_canonical(&mut model, &default_reference())
            .map_err(|e| format!("Canonical frame translation failed: {e}"))?;
        Ok(model)
    }

    /// The headless half of the bake preview: bake the character exactly as Commit writes
    /// it, and resolve the SHARED idle onto the baked bones. Split from the GPU upload so
    /// tests judge the smoke test without a renderer.
    pub(crate) fn bake_preview_parts(
        &self,
    ) -> Result<(RigFile, Vec<SkelBone>, ResolvedClip), String> {
        let name = self
            .source
            .as_ref()
            .map(|s| s.asset_name().to_string())
            .ok_or("no source is open")?;
        let model = self.character_bake_model()?;
        let rig_file = bake_rig(&model, &name);
        let bones = rig_bones(&rig_file);
        if bones.is_empty() {
            return Err("the bake produced no skeleton".into());
        }
        let idle = flicker_content::roots().package().join(BAKE_PREVIEW_CLIP);
        let text = flicker_content::package::read_text(&idle)
            .map_err(|e| format!("shared idle {}: {e}", idle.display()))?;
        let file: RigFile = serde_json::from_str(&text).map_err(|e| format!("shared idle: {e}"))?;
        let clip = resolve_clips(&file, &bones, false)
            .pop()
            .ok_or("the shared idle resolved empty")?;
        if clip.tracks.is_empty() {
            return Err(
                "the shared idle resolved onto NO bones — names diverged from canon".into(),
            );
        }
        Ok((rig_file, bones, clip))
    }

    /// COMMIT — bake the conformed model and write `flicker.rig` into STAGING
    /// ([`Self::commit_root`]). The authored bone offsets are baked in by re-deriving the
    /// model first, so what eventually ships is exactly what the viewport showed.
    ///
    /// This writes the bench's OUTPUT; it does not publish. The asset reaches the tree the game
    /// loads from only when the Content Manager promotes it out of staging. Tests bake against
    /// a scratch root through [`Self::commit_to`], never the engine's live content tree.
    pub(crate) fn commit(&mut self) {
        let root = self.commit_root();
        self.commit_to(&root);
    }

    /// Where THIS source's commit lands, by what it is: clip variants → the shared
    /// retarget library; ENVIRONMENT props → their own `staging/props/` tier (a tree is
    /// not a character); characters, garments and worn accessories → `staging/characters/`.
    /// Every root is STAGING — the Quartermaster's promote pass is the only door into
    /// `package/`, the one tree the engine loads content from.
    pub(crate) fn commit_root(&self) -> PathBuf {
        let class = self.source.as_ref().and_then(|s| s.class());
        let prop = self.source.as_ref().map(|s| s.prop);
        match (class, prop) {
            (Some(AssetClass::Animation), _) => clips_dir(),
            (Some(AssetClass::Prop), Some(PropKind::Environment)) => props_dir(),
            _ => characters_dir(),
        }
    }

    /// The commit itself, against an explicit root — so the write path is exercisable against a
    /// scratch directory instead of the engine's live content tree. Dispatches by CLASS: a Skin
    /// bakes the conformed character (offsets applied), a Prop bakes a static mesh, a clothing Prop
    /// bakes a garment SKINNED onto the base body. `flicker-content` owns every bake; this only
    /// routes and records the outcome.
    pub(crate) fn commit_to(&mut self, root: &Path) {
        // The ANIMATION path first: it has no parsed FBX — its input is the retargeted
        // preview, its output the PICKED clip variants (ruled: one, the other, or both).
        if self.source.as_ref().and_then(|s| s.class()) == Some(AssetClass::Animation) {
            self.commit_clip_to(root);
            return;
        }
        // Export must never be a SILENT no-op (QA 2026-08-03: "doesn't always end up
        // producing an object in the staging folder" — this early-return was why): with
        // nothing parsed there is nothing to bake, and the refusal lands where every
        // other stage failure does, in the error line.
        {
            let Some(src) = self.source.as_mut() else {
                return;
            };
            if src.parsed.is_none() {
                src.error = Some(
                    "Nothing is parsed — the source never loaded, so there is nothing to commit."
                        .to_string(),
                );
                return;
            }
        }
        // Read everything under a shared borrow, then drop it before the write + the mutable
        // outcome record (so the borrow checker stays happy across the class dispatch).
        let (class, prop, name, model_result, has_rig, fit, fbx, mounts) = {
            let Some(src) = self.source.as_ref() else {
                return;
            };
            let Some(parsed) = src.parsed.as_ref() else {
                return;
            };
            // A CHARACTER bakes through the ONE shared path (`character_bake_model`) — the
            // same model the Preview page plays, so the preview can never drift from the
            // export. A prop/garment ships the parse as-is (no offsets, no frame gate).
            let model_result =
                if matches!(src.class(), Some(AssetClass::Skin) | None) && src.rig.is_some() {
                    self.character_bake_model()
                } else {
                    Ok(parsed.model.clone())
                };
            // The human-authored placement the Attach stage tuned — what Commit bakes in.
            let fit = Fit {
                socket: src.fit.socket_name().to_string(),
                offset: src.fit.offset,
                rot_deg: src.fit.rot,
                scale: src.fit.scale,
                uniform: src.fit.uniform,
            };
            // The character's authored attach POINTS (the Attach stage's output) — handed
            // to the bake so the six tuned placements SHIP in the rig's `attach_points`
            // block instead of being discarded at export (the audited third-step gap).
            let mounts: Vec<flicker_content::MountPoint> = src
                .attach
                .iter()
                .map(|p| flicker_content::MountPoint {
                    id: p.id.to_string(),
                    bone: p.parent.to_string(),
                    offset: p.offset,
                })
                .collect();
            // The mesh file this came from — the prop/garment bakes read its FOLDER for the vendor's
            // texture maps, and its NAME tells one set piece's maps from another's.
            (
                src.class(),
                src.prop,
                src.asset_name().to_string(),
                model_result,
                src.rig.is_some(),
                fit,
                src.fbx.clone(),
                mounts,
            )
        };
        let model = match model_result {
            Ok(m) => m,
            Err(e) => {
                if let Some(s) = self.source.as_mut() {
                    s.error = Some(e);
                }
                return;
            }
        };

        let dir = root.join(&name);
        let out = dir.join(format!("{name}.json"));
        let result: std::result::Result<(), String> = std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))
            .and_then(|()| match class {
                // Clothing is a garment: a mesh SKINNED onto the base, its fit baked into the verts.
                Some(AssetClass::Prop) if prop == PropKind::Clothing => {
                    write_garment(&model, &fbx, &name, &out, &fit).map_err(|e| e.to_string())
                }
                // Any other prop is a rigid static mesh; the authored fit is written into its attach.
                Some(AssetClass::Prop) => {
                    // POC flat-colour (bake_prop) flows through the headless `import_prop` example,
                    // not the bench UI yet — textured props pass None here (unchanged behaviour).
                    write_prop(&model, &fbx, &name, &out, &fit, None).map_err(|e| e.to_string())
                }
                // Animation never reaches this dispatch — routed to `commit_clip_to` above.
                Some(AssetClass::Animation) => unreachable!("animation commits via commit_clip_to"),
                // Character: requires the conform to have produced a rig.
                _ => {
                    if has_rig {
                        write_rig(&model, &fbx, &name, &out, &mounts).map_err(|e| e.to_string())
                    } else {
                        Err("Conform has not run — nothing to commit.".to_string())
                    }
                }
            });

        let Some(src) = self.source.as_mut() else {
            return;
        };
        match result {
            Ok(()) => {
                tracing::info!("committed {}", out.display());
                src.committed = Some(out);
                src.error = None;
            }
            Err(e) => src.error = Some(e),
        }
    }

    /// The Animation commit: write the PICKED variants of the previewed clip under
    /// `<root>/<Set>/{In-Place,RootMotion}/<stem>.json` — the retargeter's VERBATIM
    /// output, so what lands in staging is exactly what the side-by-side showed.
    /// Root-parameterized for scratch-dir tests, like the character path.
    pub(crate) fn commit_clip_to(&mut self, root: &Path) {
        let (ip, rm) = (self.variant_ip, self.variant_rm);
        let outcome = {
            let Some(src) = self.source.as_ref() else {
                return;
            };
            match src.clip.as_ref() {
                None => Err("Clip retarget has not run — nothing to commit.".to_string()),
                Some(_) if !ip && !rm => {
                    Err("Pick at least one variant (Root Motion / In-Place) to commit.".to_string())
                }
                Some(cp) => flicker_content::retarget::write_variants(
                    &cp.variants,
                    &root.join(src.asset_name()),
                    ip,
                    rm,
                )
                .map_err(|e| e.to_string()),
            }
        };
        let Some(src) = self.source.as_mut() else {
            return;
        };
        match outcome {
            Ok(paths) => {
                tracing::info!(
                    "committed {} clip variant(s) for {}",
                    paths.len(),
                    src.asset_name()
                );
                src.committed = paths.into_iter().next();
                src.error = None;
            }
            Err(e) => src.error = Some(e),
        }
    }

    /// The engine-requirement checks the Review stage reports — each computed from real state, so
    /// a red line is a real blocker and not a placeholder.
    pub(crate) fn requirements(&self) -> Vec<(bool, String)> {
        let Some(src) = self.source.as_ref() else {
            return Vec::new();
        };
        let verts = src.parsed.as_ref().map(|p| p.verts).unwrap_or(0);
        // Prop / garment / animation carry their OWN requirement set — the character skeleton/attach
        // checks below do not apply to them.
        // Every requirement's static copy is a `$token`; the live counts compose around
        // the resolved text (the sanctioned composed-string shape).
        let r = |t: &str| strings::resolve(t).into_owned();
        match src.class() {
            Some(AssetClass::Prop) if src.prop == PropKind::Clothing => {
                return vec![
                    (
                        verts > 0,
                        format!(
                            "{} ({verts} {})",
                            r("$ap_req_garment_mesh_present"),
                            r("$ap_verts")
                        ),
                    ),
                    (true, r("$ap_req_skins_onto_the_canonical_base")),
                ];
            }
            Some(AssetClass::Prop) => {
                return vec![
                    (
                        verts > 0,
                        format!(
                            "{} ({verts} {})",
                            r("$ap_req_prop_mesh_present"),
                            r("$ap_verts")
                        ),
                    ),
                    (true, r("$ap_req_socket_fit_authored_in_the_paperdoll")),
                ];
            }
            Some(AssetClass::Animation) => {
                return vec![
                    (
                        src.clip.is_some(),
                        r("$ap_req_clip_retargets_onto_the_reference"),
                    ),
                    (
                        self.variant_ip || self.variant_rm,
                        r("$ap_req_at_least_one_variant_selected"),
                    ),
                ];
            }
            _ => {}
        }
        // Character (Skin / unclassified): reported in BAKED terms — the +1 is the root `bake_rig`
        // synthesizes — so the figure here is the one the shipped rig will carry.
        let conformed = src.parsed.as_ref().map(|p| p.bones()).unwrap_or(0);
        let baked = if conformed == 0 { 0 } else { conformed + 1 };
        let mut out = vec![(
            conformed == CONFORMED_BONES,
            format!(
                "{} ({baked} / {REFERENCE_BONES} {})",
                r("$ap_req_skeleton_conforms"),
                r("$ap_bones")
            ),
        )];
        match src.rig.as_ref() {
            None => out.push((false, r("$ap_req_conform_has_not_run"))),
            Some(rig) => {
                let (_, review, _) = rig.counts();
                out.push((
                    rig.rename.unmapped.is_empty(),
                    if rig.rename.unmapped.is_empty() {
                        format!(
                            "{} ({review} {})",
                            r("$ap_req_all_bones_mapped_or_reviewed"),
                            r("$ap_flagged")
                        )
                    } else {
                        format!(
                            "{} {}",
                            rig.rename.unmapped.len(),
                            r("$ap_req_source_bones_unmapped")
                        )
                    },
                ));
            }
        }
        let resolved = (0..src.attach.len())
            .filter(|i| self.attach_resolved(*i))
            .count();
        out.push((
            resolved == src.attach.len(),
            format!(
                "{} ({resolved} / {})",
                r("$ap_req_attach_points_on_valid_parents"),
                src.attach.len()
            ),
        ));
        out.push((
            src.textures > 0,
            format!(
                "{} ({} {})",
                r("$ap_req_textures_masks_resolved"),
                src.textures,
                r("$ap_found")
            ),
        ));
        out
    }

    /// Whether an attach point's parent bone exists in the conformed rig.
    pub(crate) fn attach_resolved(&self, i: usize) -> bool {
        self.source
            .as_ref()
            .and_then(|s| s.attach.get(i))
            .is_some_and(|p| p.bone.is_some())
    }

    /// Write a WORLD-space translation delta onto the selected bone's [`BoneOffset::t`], converting
    /// it to the bone's PARENT-local frame first — that is the space `t` lives in (folded into the
    /// pose by [`rest_globals`], so the sliders and the gizmo share one value). `recentre` is
    /// translation-only, so the world delta needs no un-recentring. Re-derives the pose and bumps
    /// `pose_gen` so the mesh re-skins live.
    pub(crate) fn apply_gizmo_delta(&mut self, sel: usize, globals: &[Mat4], world_delta: Vec3) {
        // Parent's current posed rotation/scale takes the world delta into parent-local (root = world).
        let parent_inv = self
            .source
            .as_ref()
            .and_then(|s| s.parsed.as_ref())
            .and_then(|p| p.model.bones.get(sel))
            .and_then(|b| usize::try_from(b.parent).ok())
            .and_then(|pi| globals.get(pi))
            .map(|g| glam::Mat3::from_mat4(*g).inverse())
            .unwrap_or(glam::Mat3::IDENTITY);
        let dt = parent_inv * world_delta;
        if !dt.is_finite() {
            return;
        }
        let Some(src) = self.source.as_mut() else {
            return;
        };
        let Some(slot) = src.rig.as_mut().and_then(|r| r.offsets.get_mut(sel)) else {
            return;
        };
        slot.t[0] += dt.x;
        slot.t[1] += dt.y;
        slot.t[2] += dt.z;
        let offsets = src
            .rig
            .as_ref()
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        if let Some(parsed) = src.parsed.as_mut() {
            parsed.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    /// ORTHO reposition (the tool's core): move the bone's REST position and RE-BIND, so only the
    /// SKELETON moves — the mesh stays put. Unlike [`Self::apply_gizmo_delta`] (Perspective, which
    /// DEFORMS to preview the skin), this edits the rest local translation and re-derives every
    /// `inverse_bind` to the new rest, keeping the palette identity at rest. Mirrors to the
    /// symmetric `_l`/`_r` joint when `mirror_joints` is on. Bake Skin afterwards to re-weight to
    /// the corrected skeleton. A PERMANENT conform edit — the scene arms its discard guard from
    /// the `pose_gen` bump.
    pub(crate) fn reposition_bone(&mut self, sel: usize, globals: &[Mat4], world_delta: Vec3) {
        let mirror = self.mirror_joints.then(|| self.mirror_of(sel)).flatten();
        let offsets = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) else {
            return;
        };
        let dt = parent_local_delta(globals, &p.model, sel, world_delta);
        if !dt.is_finite() {
            return;
        }
        for k in 0..3 {
            p.model.bones[sel].translation[k] += dt[k];
        }
        if let Some(m) = mirror {
            // Mirror across the body's symmetry plane (world X): negate the X of the move.
            let mworld = Vec3::new(-world_delta.x, world_delta.y, world_delta.z);
            let mdt = parent_local_delta(globals, &p.model, m, mworld);
            if mdt.is_finite() {
                for k in 0..3 {
                    p.model.bones[m].translation[k] += mdt[k];
                }
            }
        }
        // Re-bind: inverse_bind = (new rest world)⁻¹ for every bone → the mesh is unchanged at rest;
        // only the skeleton has moved. Bake Skin then re-weights to the corrected bones.
        let (rest, _) = rest_globals(&p.model, &[]);
        for (b, g) in p.model.bones.iter_mut().zip(&rest) {
            b.inverse_bind = g.inverse().to_cols_array();
        }
        p.rebuild(&offsets);
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    /// The symmetric bone of `sel` (its `_l`/`_r` twin), if the rig has one.
    pub(crate) fn mirror_of(&self, sel: usize) -> Option<usize> {
        let p = self.source.as_ref()?.parsed.as_ref()?;
        let mname = mirror_name(&p.model.bones.get(sel)?.name)?;
        p.model.bones.iter().position(|b| b.name == mname)
    }

    /// Re-bake the character's skin WEIGHTS from the repositioned skeleton (replacing the source's
    /// auto-skin), then re-skin the view. The rest pose is untouched, so the mesh does not move — only
    /// its deformation changes, which the Perspective panel shows. A conform edit the scene's
    /// discard guard reads off the `pose_gen` bump.
    pub(crate) fn bake_skin_now(&mut self) {
        let offsets = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        if let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) {
            bake_skin(&mut p.model);
            p.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    /// Restore the focused joint's [`BoneOffset`] to `offset` (its pre-drag value) — the spring-back
    /// that ends a Perspective deform TEST, snapping the joint back to its rest position.
    pub(crate) fn restore_offset(&mut self, sel: usize, offset: BoneOffset) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if let Some(slot) = src.rig.as_mut().and_then(|r| r.offsets.get_mut(sel)) {
            *slot = offset;
        }
        let offsets = src
            .rig
            .as_ref()
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        if let Some(p) = src.parsed.as_mut() {
            p.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    /// Return to the rig view to import the NEXT piece. THE loop a weapon set or an outfit needs:
    /// pick → rig → bake → pick the next. Keeps the open folder and its candidate list; drops
    /// everything derived from the piece just committed, so the next one starts clean rather than
    /// inheriting the last one's rig or fit. The piece picker is right there on the rig stage for
    /// choosing which mesh comes next; the scene restarts its own page walk.
    pub(crate) fn start_next_piece(&mut self) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        src.parsed = None;
        src.report = None;
        // Keep the DECLARED workflow: the next piece in a set is the same class the user chose on
        // the Workflow page, not a re-detection.
        src.class = self.pending_class;
        src.rig = None;
        src.committed = None;
        src.error = None;
        src.fit = PropFit::default();
        src.clip = None;
    }

    /// The workflow DEFINITION a declared class dispatches to — the launcher-field pattern: a
    /// Character (and the unclassified default) walks [`WF_CHARACTER`]; a Prop / Accessory
    /// walks [`WF_PROP`], which simply HAS no character-only Attach step; an Animation walks
    /// [`WF_ANIMATION`].
    pub(crate) fn workflow_for(class: Option<AssetClass>) -> &'static str {
        match class {
            Some(AssetClass::Prop) => WF_PROP,
            Some(AssetClass::Animation) => WF_ANIMATION,
            Some(AssetClass::Skin) | None => WF_CHARACTER,
        }
    }

    /// Record which workflow runs (one of the `WF_*` names). The document only records the
    /// name; the scene walks the pages. [`Self::open`] re-derives it from the declared class
    /// through [`Self::workflow_for`], so a folder opened headlessly dispatches too.
    pub(crate) fn dispatch_workflow(&mut self, workflow: &'static str) {
        self.workflow = workflow;
    }

    /// PREP — cache the pristine source once for a raw (boneless) mesh, at 100% (the target
    /// field reads the source count), then prep the working geometry. Keyed by source identity
    /// so it runs once per piece; a mesh that already ships a skeleton is game-ready and skipped
    /// (Prep is the raw-mesh conditioning stage).
    pub(crate) fn ensure_prep_source(&mut self) {
        // The source identity + whether the working mesh currently carries a skeleton, as owned
        // values so the immutable borrow ends before the mutable rebuild below.
        let Some((has_bones, key)) = self.source.as_ref().and_then(|s| {
            s.parsed
                .as_ref()
                .map(|p| (!p.model.bones.is_empty(), (s.dir.clone(), s.candidate_sel)))
        }) else {
            return;
        };
        let built = self.prep.as_ref().is_some_and(|c| c.key == key);
        if !built {
            // A mesh that arrives rigged is game-ready — no cache, no decimation.
            if has_bones {
                return;
            }
            let source = match self.source.as_ref().and_then(|s| s.parsed.as_ref()) {
                Some(p) => p.model.clone(),
                None => return,
            };
            let source_tris = source.indices.len() / 3;
            tracing::info!("prep: cached the raw source at {source_tris} triangles");
            self.decimate_target = source_tris.to_string();
            self.prep = Some(PrepCache {
                key,
                decimated: source.clone(),
                source,
                source_tris,
                applied: source_tris,
            });
            self.rebuild_prepped_model();
        } else if has_bones {
            // Re-entered Prep after conforming a raw mesh: revert the working mesh to the boneless
            // prepped geometry so the controls act again (the rig re-installs on the next Conform).
            self.rebuild_prepped_model();
        }
    }

    /// The triangle target a typed field resolves to against a source of `source_tris`: digits
    /// only, empty or zero falls back to the source (100%), and nothing above the source is
    /// asked for (decimation only removes).
    pub(crate) fn prep_target(text: &str, source_tris: usize) -> usize {
        text.parse::<usize>()
            .ok()
            .filter(|t| *t > 0)
            .map_or(source_tris, |t| t.min(source_tris))
    }

    /// APPLY on the Prep step: collapse the pristine source to the typed triangle target and
    /// rebuild the working mesh. Returns whether the applied target changed (the scene arms its
    /// discard guard). The field is re-published as the resolved target, so a clamped or empty
    /// entry shows what was actually applied.
    pub(crate) fn apply_decimate_target(&mut self) -> bool {
        let Some(cache) = self.prep.as_mut() else {
            return false;
        };
        let target = Self::prep_target(&self.decimate_target, cache.source_tris);
        self.decimate_target = target.to_string();
        if target == cache.applied {
            return false;
        }
        cache.decimated = if target >= cache.source_tris {
            cache.source.clone()
        } else {
            decimate_to(&cache.source, target)
        };
        cache.applied = target;
        tracing::info!(
            "prep: decimated {} → {} triangles (target {target})",
            cache.source_tris,
            cache.decimated.indices.len() / 3
        );
        self.rebuild_prepped_model();
        true
    }

    /// RESET on the Prep step: back to 100% — the field reads the source count and the working
    /// mesh is the source again. Returns whether anything changed.
    pub(crate) fn reset_decimate_target(&mut self) -> bool {
        let Some(cache) = self.prep.as_mut() else {
            return false;
        };
        self.decimate_target = cache.source_tris.to_string();
        if cache.applied == cache.source_tris {
            return false;
        }
        cache.applied = cache.source_tris;
        cache.decimated = cache.source.clone();
        self.rebuild_prepped_model();
        true
    }

    /// Rebuild the working model from the cached (applied) decimation + target stature, and
    /// invalidate any conform result so the rig re-installs from the new geometry. Boneless meshes
    /// only (the Prep cache is absent for a mesh that arrived rigged, so this is a no-op there).
    /// The height slider's write is `stature_cm` then this.
    pub(crate) fn rebuild_prepped_model(&mut self) {
        let stature = self.stature_cm;
        let Some(cache) = self.prep.as_ref() else {
            return;
        };
        let mut model = cache.decimated.clone();
        scale_mesh_to_stature(&mut model, stature);
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if let Some(parsed) = src.parsed.as_mut() {
            *parsed = Parsed::new(model);
        }
        src.rig = None; // geometry changed — the rig re-installs on entering Conform
        self.pose_gen = self.pose_gen.wrapping_add(1);
        self.mesh_gen = self.mesh_gen.wrapping_add(1);
    }

    /// A height as BOTH metric and imperial: "170 cm · 5′7″" (the unit is metric; the imperial is
    /// shown alongside for reading).
    pub(crate) fn height_readout(cm: f32) -> String {
        let total_in = (cm / 2.54).max(0.0);
        let mut feet = (total_in / 12.0).floor() as i32;
        let mut inches = (total_in - feet as f32 * 12.0).round() as i32;
        if inches >= 12 {
            feet += 1;
            inches = 0;
        }
        format!("{cm:.0} cm · {feet}′{inches}″")
    }

    /// The Prep readout: a raw mesh shows the working triangle count against the source's; a mesh
    /// that arrives rigged shows that it is game-ready and skipped.
    pub(crate) fn prep_status(&self) -> String {
        let Some(parsed) = self.source.as_ref().and_then(|s| s.parsed.as_ref()) else {
            return String::new();
        };
        if !parsed.model.bones.is_empty() {
            return strings::resolve("$ap_prep_rigged").into_owned();
        }
        match self.prep.as_ref() {
            Some(cache) => format!(
                "{} / {} {}",
                parsed.tris,
                cache.source_tris,
                strings::resolve("$ap_triangles"),
            ),
            None => String::new(),
        }
    }

    /// World position of the currently-authored attach point `i` — see [`Source::attach_world`].
    pub(crate) fn attach_world(&self, i: usize) -> Option<Vec3> {
        self.source.as_ref()?.attach_world(i)
    }

    // ── Accessors: the facts a thin scene publishes, without reaching into the internals. ──

    /// The asset name the pipeline bakes under — the source folder's own name.
    pub(crate) fn asset_name(&self) -> Option<&str> {
        self.source.as_ref().map(Source::asset_name)
    }

    /// The picked source file's name (the FBX, or the BVH on the animation path).
    pub(crate) fn file_name(&self) -> Option<&str> {
        self.source.as_ref().map(Source::file_name)
    }

    /// What conform did to the skeleton, for the status line: how many bones were
    /// inferred from the reference and how many limbs were re-aligned. `None` before
    /// conform ran.
    pub(crate) fn rig_summary(&self) -> Option<String> {
        let rig = self.source.as_ref()?.rig.as_ref()?;
        let r = |t: &str| strings::resolve(t).into_owned();
        Some(format!(
            "{} {} · {} {}",
            rig.out.infer.added.len(),
            r("$ap_inferred"),
            rig.out.reorient.limbs_aligned,
            r("$ap_limbs_aligned")
        ))
    }

    /// The effective class: the declared override, else what Classify detected.
    pub(crate) fn class(&self) -> Option<AssetClass> {
        self.source.as_ref().and_then(Source::class)
    }

    /// The working model with its cached rest frames and bounds — what the view tier
    /// composes its skeleton, collision and framing from. `None` before a parse.
    pub(crate) fn parsed(&self) -> Option<&Parsed> {
        self.source.as_ref()?.parsed.as_ref()
    }

    /// Triangles in the WORKING mesh (after Prep), `None` before a parse.
    pub(crate) fn tri_count(&self) -> Option<usize> {
        self.parsed().map(|p| p.tris)
    }

    /// Vertices in the working mesh, `None` before a parse.
    pub(crate) fn vert_count(&self) -> Option<usize> {
        self.parsed().map(|p| p.verts)
    }

    /// Bones in the working skeleton (0 for a raw mesh), `None` before a parse.
    pub(crate) fn bone_count(&self) -> Option<usize> {
        self.parsed().map(Parsed::bones)
    }

    /// The bone map: every working bone's canonical name with its provenance, in skeleton
    /// order. Empty before conform.
    pub(crate) fn bone_rows(&self) -> Vec<(String, MapState)> {
        let Some(src) = self.source.as_ref() else {
            return Vec::new();
        };
        let (Some(parsed), Some(rig)) = (src.parsed.as_ref(), src.rig.as_ref()) else {
            return Vec::new();
        };
        parsed
            .model
            .bones
            .iter()
            .zip(&rig.map)
            .map(|(b, state)| (b.name.clone(), *state))
            .collect()
    }

    /// The selected bone-map row, once a rig exists.
    pub(crate) fn bone_sel(&self) -> Option<usize> {
        self.source.as_ref()?.rig.as_ref().map(|r| r.sel)
    }

    /// Select a bone by index (a pick in the rig view). `false` without a rig or past its end.
    pub(crate) fn select_bone(&mut self, i: usize) -> bool {
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let bones = src.parsed.as_ref().map(|p| p.globals.len()).unwrap_or(0);
        let Some(rig) = src.rig.as_mut().filter(|_| i < bones) else {
            return false;
        };
        rig.sel = i;
        true
    }

    /// Select a bone by canonical name. `false` when the rig has no such bone (or no rig).
    pub(crate) fn select_bone_named(&mut self, name: &str) -> bool {
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let Some(i) = src.parsed.as_ref().and_then(|p| p.bone_index(name)) else {
            return false;
        };
        let Some(rig) = src.rig.as_mut() else {
            return false;
        };
        rig.sel = i;
        true
    }

    /// The selected bone's authored offset.
    pub(crate) fn selected_offset(&self) -> Option<BoneOffset> {
        let rig = self.source.as_ref()?.rig.as_ref()?;
        rig.offsets.get(rig.sel).copied()
    }

    /// Author the selected bone's offset — the slider path. The skeleton is re-derived ONLY
    /// when the value actually changed (controls report every frame), and the live skin
    /// re-uploads off the `pose_gen` bump exactly as a gizmo drag does — the two write the
    /// same offset. Zeroing it is "Reset bone".
    pub(crate) fn set_selected_offset(&mut self, off: BoneOffset) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        let Some(rig) = src.rig.as_mut() else {
            return;
        };
        let Some(slot) = rig.offsets.get_mut(rig.sel) else {
            return;
        };
        if *slot == off {
            return;
        }
        *slot = off;
        // The authored pose changed — re-derive the frames once, here, not per frame.
        let offsets = rig.offsets.clone();
        if let Some(parsed) = src.parsed.as_mut() {
            parsed.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    /// Every riggable mesh (or BVH clip) the opened folder offers, as `(file stem, file
    /// name)` — a weapon set is four or five pieces, an outfit is tops/pants/gloves/shoes.
    pub(crate) fn candidate_rows(&self) -> Vec<(String, String)> {
        let Some(src) = self.source.as_ref() else {
            return Vec::new();
        };
        src.candidates
            .iter()
            .map(|p| {
                let part = |s: Option<&std::ffi::OsStr>| {
                    s.and_then(|s| s.to_str()).unwrap_or("").to_string()
                };
                (part(p.file_stem()), part(p.file_name()))
            })
            .collect()
    }

    /// Pick the piece to import by file stem. Choosing a DIFFERENT piece invalidates everything
    /// derived from the previous one, so the wizard can never carry a stale parse/conform forward;
    /// re-picking the current one is a no-op. `false` when no candidate has that stem.
    pub(crate) fn select_candidate(&mut self, stem: &str) -> bool {
        let pending = self.pending_class;
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let Some(idx) = src
            .candidates
            .iter()
            .position(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem))
        else {
            return false;
        };
        if idx != src.candidate_sel {
            src.candidate_sel = idx;
            src.fbx = src.candidates[idx].clone();
            src.parsed = None;
            src.report = None;
            // Preserve the declared workflow across a pick — only the derived state is stale.
            src.class = pending;
            src.rig = None;
            src.committed = None;
            src.error = None;
            // The clip preview derives from the ACTIVE pick; the conform-step runner
            // (`prepare_clip`) rebuilds it next frame, like analyze/conform.
            src.clip = None;
        }
        true
    }

    /// The picked piece's file stem — the picker's bound value. `None` with nothing open
    /// (or a folder with nothing to pick).
    pub(crate) fn selected_candidate(&self) -> Option<&str> {
        let src = self.source.as_ref()?;
        src.candidates
            .get(src.candidate_sel)?
            .file_stem()
            .and_then(|s| s.to_str())
    }

    /// The mount sockets a prop/garment can hang from, as `(canonical bone, label $token)`.
    pub(crate) fn socket_rows(&self) -> Vec<(String, String)> {
        SOCKETS
            .iter()
            .map(|(id, label)| (id.to_string(), label.to_string()))
            .collect()
    }

    /// Mount the piece to a socket by canonical bone name. `false` for an unknown socket or
    /// with nothing open.
    pub(crate) fn select_socket(&mut self, id: &str) -> bool {
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let Some(idx) = SOCKETS.iter().position(|(s, _)| *s == id) else {
            return false;
        };
        src.fit.socket = idx;
        true
    }

    /// The character's six attach points, as `(id, label $token)` in rail order.
    pub(crate) fn attach_rows(&self) -> Vec<(String, String)> {
        let Some(src) = self.source.as_ref() else {
            return Vec::new();
        };
        src.attach
            .iter()
            .map(|p| (p.id.to_string(), p.label.to_string()))
            .collect()
    }

    /// Select an attach point by id. `false` for an unknown id or with nothing open.
    pub(crate) fn select_attach(&mut self, id: &str) -> bool {
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let Some(i) = src.attach.iter().position(|p| p.id == id) else {
            return false;
        };
        src.attach_sel = i;
        true
    }

    /// The selected attach point's index (rail order), once a folder is open.
    pub(crate) fn attach_sel(&self) -> Option<usize> {
        self.source.as_ref().map(|s| s.attach_sel)
    }

    /// The selected attach point's authored offset from its parent bone (cm).
    pub(crate) fn attach_offset(&self) -> Option<[f32; 3]> {
        let src = self.source.as_ref()?;
        src.attach.get(src.attach_sel).map(|p| p.offset)
    }

    /// Author the selected attach point's offset — the three Attach sliders.
    pub(crate) fn set_attach_offset(&mut self, o: [f32; 3]) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        if let Some(p) = src.attach.get_mut(src.attach_sel) {
            p.offset = o;
        }
    }

    /// The prop/garment mount fit, once a folder is open.
    pub(crate) fn fit(&self) -> Option<&PropFit> {
        self.source.as_ref().map(|s| &s.fit)
    }

    /// The prop/garment mount fit for authoring — the fit sliders write here.
    pub(crate) fn fit_mut(&mut self) -> Option<&mut PropFit> {
        self.source.as_mut().map(|s| &mut s.fit)
    }

    /// Whether Commit has written this piece. The multi-piece "next piece" offer is this
    /// AND `candidate_rows().len() > 1`.
    pub(crate) fn has_committed(&self) -> bool {
        self.source.as_ref().is_some_and(|s| s.committed.is_some())
    }

    /// The retargeted clip's real facts — file, length, and the variant pick Commit will
    /// honour — as one readout line. `None` until `prepare_clip` built the preview.
    pub(crate) fn clip_summary(&self) -> Option<String> {
        let src = self.source.as_ref()?;
        let cp = src.clip.as_ref()?;
        let r = |t: &str| strings::resolve(t).into_owned();
        let secs = cp.duration as f32 / cp.ip.tick_rate_hz.max(1) as f32;
        let mark = |on: bool| if on { "[x]" } else { "[ ]" };
        Some(format!(
            "{} {} · {} {} · {secs:.1}s · {} {} {} · {} {}",
            r("$ap_clip"),
            src.file_name(),
            r("$ap_duration"),
            cp.duration,
            r("$ap_variants"),
            mark(self.variant_rm),
            r("$ap_root_motion"),
            mark(self.variant_ip),
            r("$ap_in_place"),
        ))
    }

    /// The last stage failure, surfaced instead of a fabricated result.
    pub(crate) fn error(&self) -> Option<&str> {
        self.source.as_ref()?.error.as_deref()
    }
}

/// The user-facing name for an asset class. `AssetClass::id()` ("skin"/"prop"/"animation") is a
/// stable serialization token and must never reach the UI; this is the DISPLAY string, kept separate
/// so the id cannot leak into a panel and so localization has a single place to hook. Skin reads as
/// "Character" — the word the user chose on the workflow card, not the internal skin term.
pub(crate) fn class_label(class: Option<AssetClass>) -> Cow<'static, str> {
    strings::resolve(match class {
        Some(AssetClass::Skin) => "$ap_character",
        Some(AssetClass::Prop) => "$ap_prop",
        Some(AssetClass::Animation) => "$ap_animation",
        None => "$ap_unclassified",
    })
}

/// CPU linear-blend skin of the source mesh into deformed [`MeshVertex`]es (source space), through
/// the posed `globals` and each bone's `inverse_bind`. Mirrors `flicker-skeletal::skin` but over the
/// raw [`RawModel`] the pipeline holds — no format conversion. The palette is `globals[b] *
/// inverse_bind[b]`; at the conform rest pose that is the identity (so the bind mesh is reproduced),
/// and an authored offset moves the bone's `globals` entry, deforming the vertices it weights.
pub(crate) fn skin_source_verts(model: &RawModel, globals: &[Mat4]) -> Vec<MeshVertex> {
    let palette: Vec<Mat4> = model
        .bones
        .iter()
        .enumerate()
        .map(|(b, bone)| {
            globals.get(b).copied().unwrap_or(Mat4::IDENTITY)
                * Mat4::from_cols_array(&bone.inverse_bind)
        })
        .collect();
    model
        .vertices
        .iter()
        .map(|v| {
            let p = Vec3::from_array(v.p).extend(1.0);
            let n = Vec3::from_array(v.n);
            let mut pos = Vec3::ZERO;
            let mut nrm = Vec3::ZERO;
            let mut any = false;
            for k in 0..4 {
                let w = v.weights[k];
                if w == 0.0 {
                    continue;
                }
                any = true;
                let m = palette
                    .get(v.joints[k] as usize)
                    .copied()
                    .unwrap_or(Mat4::IDENTITY);
                pos += w * (m * p).truncate();
                nrm += w * (glam::Mat3::from_mat4(m) * n);
            }
            let position = if any { pos.to_array() } else { v.p };
            let normal = if nrm.length_squared() > 1e-12 {
                nrm.normalize().to_array()
            } else {
                v.n
            };
            MeshVertex {
                position,
                normal,
                material: 0,
            }
        })
        .collect()
}

/// Rest-pose world frames + parent topology from a parsed model, with the authored
/// `offsets` folded in. Bones are stored as local TRS relative to their parent, so a single
/// forward pass composes them; parents always precede children in an FBX skeleton.
///
/// An offset is parent-relative translation plus a roll about the bone's own X axis — the same
/// space the source bone is stored in, so an offset of zero reproduces the conform exactly.
pub(crate) fn rest_globals(model: &RawModel, offsets: &[BoneOffset]) -> (Vec<Mat4>, Vec<i32>) {
    let mut globals: Vec<Mat4> = Vec::with_capacity(model.bones.len());
    let mut parents: Vec<i32> = Vec::with_capacity(model.bones.len());
    for (i, b) in model.bones.iter().enumerate() {
        let o = offsets.get(i).copied().unwrap_or_default();
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from_array(b.scale),
            glam::Quat::from_array(b.rotation) * glam::Quat::from_rotation_x(o.roll.to_radians()),
            Vec3::from_array(b.translation) + Vec3::from_array(o.t),
        );
        let world = match usize::try_from(b.parent) {
            Ok(p) if p < globals.len() => globals[p] * local,
            _ => local,
        };
        globals.push(world);
        parents.push(b.parent);
    }
    (globals, parents)
}

/// Fold the authored offsets into a model's bones, so a bake carries what the viewport showed.
/// The same arithmetic as `rest_globals` applies, one level down: local TRS, parent-relative.
pub(crate) fn apply_offsets(model: &mut RawModel, offsets: &[BoneOffset]) {
    for (b, o) in model.bones.iter_mut().zip(offsets) {
        if o.is_zero() {
            continue;
        }
        for k in 0..3 {
            b.translation[k] += o.t[k];
        }
        let q =
            glam::Quat::from_array(b.rotation) * glam::Quat::from_rotation_x(o.roll.to_radians());
        b.rotation = q.to_array();
    }
}

/// A WORLD-space translation delta expressed in a bone's PARENT-local frame (root = world), using the
/// pre-edit `globals` for the parent's rotation — the space `RawBone::translation` / `BoneOffset::t`
/// live in.
pub(crate) fn parent_local_delta(
    globals: &[Mat4],
    model: &RawModel,
    sel: usize,
    world_delta: Vec3,
) -> Vec3 {
    let parent_inv = model
        .bones
        .get(sel)
        .and_then(|b| usize::try_from(b.parent).ok())
        .and_then(|pi| globals.get(pi))
        .map(|g| glam::Mat3::from_mat4(*g).inverse())
        .unwrap_or(glam::Mat3::IDENTITY);
    parent_inv * world_delta
}

/// The symmetric bone NAME for a left/right bone (`thigh_l`↔`thigh_r`), or `None` for a centre bone.
pub(crate) fn mirror_name(name: &str) -> Option<String> {
    const PAIRS: [(&str, &str); 6] = [
        ("_l", "_r"),
        ("_r", "_l"),
        ("_L", "_R"),
        ("_R", "_L"),
        (".L", ".R"),
        (".R", ".L"),
    ];
    for (a, b) in PAIRS {
        if let Some(stem) = name.strip_suffix(a) {
            return Some(format!("{stem}{b}"));
        }
    }
    None
}

/// Per-bone provenance, read straight out of the conform reports — the bone map's colour key has
/// exactly one source of truth.
///
/// `InferReport.added` names the bones the reference contributed (auto); the hip / shoulder /
/// ankle derives moved joints whose placement is worth a human's eye (review); everything else
/// came from the source and was renamed (ok).
pub(crate) fn bone_map_states(model: &RawModel, out: &ConformOutput) -> Vec<MapState> {
    // The derive passes report per-side placements, not bone names, so a side that was actually
    // placed marks its own joints.
    let mut review: Vec<&str> = Vec::new();
    let mut mark = |placed: bool, names: &[&'static str]| {
        if placed {
            review.extend_from_slice(names);
        }
    };
    mark(out.hip.left.is_some(), &["thigh_l"]);
    mark(out.hip.right.is_some(), &["thigh_r"]);
    mark(out.shoulder.left.is_some(), &["clavicle_l", "upperarm_l"]);
    mark(out.shoulder.right.is_some(), &["clavicle_r", "upperarm_r"]);
    mark(out.ankle.left.is_some(), &["foot_l", "ball_l"]);
    mark(out.ankle.right.is_some(), &["foot_r", "ball_r"]);

    model
        .bones
        .iter()
        .map(|b| {
            if out.infer.added.iter().any(|a| a == &b.name) {
                MapState::Auto
            } else if review.contains(&b.name.as_str()) {
                MapState::Review
            } else {
                MapState::Ok
            }
        })
        .collect()
}

/// Where a committed rig lands — **STAGING**, not the shipped package.
///
/// A commit here is the pipeline's OUTPUT, not a publish: the Content Manager bench reviews what
/// lands in staging and promotes it into `package/` (recording it in the package manifest). So a
/// fresh commit is deliberately NOT visible to the running game until it is promoted.
///
/// The root comes from the executable's `content.json` via [`flicker_content::roots`] rather than a
/// climb out of this crate's source dir, so the tree can move without touching this bench.
pub(crate) fn characters_dir() -> PathBuf {
    flicker_content::roots().staging().join("characters")
}

/// Where committed CLIP VARIANTS land — the shared retarget library's STAGING tier,
/// mirroring the package layout the paperdoll reads (`retarget/clips/<set>/…`), reached
/// by promotion exactly like [`characters_dir`]'s rigs.
pub(crate) fn clips_dir() -> PathBuf {
    flicker_content::roots()
        .staging()
        .join("retarget")
        .join("clips")
}

/// Where committed ENVIRONMENT props land — their own staging tier (`staging/props/`):
/// a tree filed under `characters/` would read as classified nonsense in the
/// Quartermaster. Promoted into the package like everything else.
pub(crate) fn props_dir() -> PathBuf {
    flicker_content::roots().staging().join("props")
}

/// The asset's bounding CENTRE and half-extent — the framing the viewport needs.
///
/// Measured from the MESH when there is one (a prop carries no skeleton at all) and from the bone
/// frames otherwise. Everything is then drawn offset by `-centre`, because the quad cameras all
/// target the ORIGIN and in Z-up ground reckoning the origin is the asset's FEET (a character
/// stands 0..170 in +Z) — targeting it framed the feet with the body sticking out of shot.
pub(crate) fn model_bounds(model: &RawModel, globals: &[Mat4]) -> (Vec3, f32, f32) {
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let mut any = false;
    for v in &model.vertices {
        let p = Vec3::from(v.p);
        lo = lo.min(p);
        hi = hi.max(p);
        any = true;
    }
    if !any {
        for g in globals {
            let p = g.w_axis.truncate();
            lo = lo.min(p);
            hi = hi.max(p);
            any = true;
        }
    }
    if !any {
        return (Vec3::ZERO, 1.0, 0.0);
    }
    let centre = (lo + hi) * 0.5;
    // The floor is reported ALREADY RECENTRED (`lo.z - centre.z`, so it is negative), because
    // every caller draws through the same `-centre` offset — handing back the raw `lo.z` would
    // make each one re-derive the shift and eventually one of them would forget.
    (
        centre,
        ((hi - lo).max_element() * 0.5).max(1.0),
        lo.z - centre.z,
    )
}
