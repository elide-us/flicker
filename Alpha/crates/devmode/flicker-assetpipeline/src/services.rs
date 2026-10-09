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
use flicker_content::baseline::{describe, load_presets, markers_by_module, skeletons_dir, Preset};
use flicker_content::{
    bake_rig, classify_asset, conform_to_canonical, decimate_to, default_reference, face_to_rig,
    fill_chain, garment_socket, measure_facing, mirror_mesh, parse_fbx, rename_to_canonical,
    reorient_to_canonical, rig_raw_mesh, scale_mesh_to_stature, scan_folder, write_garment,
    write_prop, write_rig, AssetClass, AssetReport, Body, ConformMode, ConformOutput, Fit, Flesh,
    Kind, PropKind, RawModel, RenameReport, Scan, ShapeMatch, StanceSource,
};
use flicker_mechanics::{autofit_capsules_from, Volume};
use flicker_skeletal::format::{ClothRegion, RegionTag, SkeletonRecipe};
// The clip preview's playable form: the retargeter's in-memory output decoded through the
// SAME resolve path `load_dirs` uses, then sampled per frame — no disk round-trip.
use flicker_skeletal::format::{
    resolve_clips, rig_bones, Bone as SkelBone, Pattern, ResolvedClip, RigFile,
};
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
pub(crate) const WF_CREATURE: &str = "import_creature";

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
#[cfg(test)]
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
    /// The bounding box's half sizes about `centre`, per axis — the depth a panel's back cut spans
    /// (a body is far shallower than it is tall, so the sphere radius would waste the slider).
    pub(crate) half_extent: Vec3,
    /// The asset's feet plane in RECENTRED space (negative) — where the stage floor is drawn.
    pub(crate) floor: f32,
    /// Auto-fit collision volumes (per-bone capsules + leaf-bone spheres), rebuilt with the pose so
    /// the `Collision` overlay shows the coverage the rig currently produces. Empty for a bone-less
    /// prop. The SAME `flicker-mechanics` auto-fit the paperdoll and the runtime bridge use.
    pub(crate) collision: Vec<Volume>,
    /// THE BODY AS READ — this mesh's FLESH field (spec 76EB9552: voxel occupancy + inscribed
    /// radius, the thing that knows where the middle of the body is) and the SHAPE GRAPH thinned
    /// from it. The raw-mesh fit hands its own read over ([`Self::read_by_fit`], DD7A59A9), so the
    /// Rig step's ortho depth and Infer, the Bake-skin button and every Preview and Commit ask
    /// their questions of the one read the fit laid the joints down on; anything else reads it on
    /// FIRST USE ([`Self::body`]) and keeps it, never per frame — the rest frames move over it all
    /// day without the geometry changing.
    ///
    /// It lives on `Parsed` BESIDE the model it describes, so every site that REPLACES the model
    /// with a fresh `Parsed` (Analyze, the re-opened staged rig, Prep's re-apply) drops the stale
    /// read structurally rather than by remembering to. The sites that change this model's
    /// geometry IN PLACE — `rig_raw_mesh`, `conform_to_canonical`'s reorient,
    /// `scale_mesh_to_stature` and a region write (a tagged region is masked out of the flesh) —
    /// drop it through [`Self::geometry_changed`], and say so.
    body: std::cell::OnceCell<Body>,
    /// THIS MODEL'S SKIN IS THE BENCH'S OWN BIND — the raw-mesh fit's ([`Self::read_by_fit`]) or
    /// a Bake-skin press — and not a vendor's. The bake path finishes such a skin with the one a
    /// body MOVES in (`flicker_content::bake::bind_for_motion`: haunches and shoulders on the
    /// limbs under them); a vendor's weights are never re-bound behind its back.
    pub(crate) own_skin: bool,
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
            half_extent: Vec3::ONE,
            floor: 0.0,
            collision: Vec::new(),
            body: std::cell::OnceCell::new(),
            own_skin: false,
        };
        p.rebuild(&[]);
        p
    }

    pub(crate) fn bones(&self) -> usize {
        self.model.bones.len()
    }

    /// The mesh's BODY — the fit's own read, else read on first use and kept (see the field's
    /// note). The geometry is what it is read from, so an authored offset or a repositioned joint
    /// — which move frames, not vertices — leave it valid.
    pub(crate) fn body(&self) -> &Body {
        self.body.get_or_init(|| Body::read(&self.model))
    }

    /// The body's [`Flesh`] field.
    pub(crate) fn flesh(&self) -> &Flesh {
        &self.body().flesh
    }

    /// THE THIN PART around `seed`, as this model's own vertices (spec 0A81088E T2's grow-from-
    /// click). Reads the SAME kept field the ortho depth does, so the first grow pays for it and
    /// every later one is free — and because that field is the BODY's (already-tagged regions
    /// masked out), a part that is spoken for is not flesh to grow through twice.
    pub(crate) fn grow_thin(&self, seed: Vec3) -> Vec<u32> {
        self.flesh().grow_thin(&self.model, seed)
    }

    /// The model's geometry changed under this `Parsed` — drop what was read from the old
    /// vertices. (Replacing the whole `Parsed` does this by construction; this is for the
    /// in-place mutations.)
    pub(crate) fn geometry_changed(&mut self) {
        self.body = std::cell::OnceCell::new();
    }

    /// THE FIT'S OWN READ of the geometry it just rigged (`rig_raw_mesh` sized the mesh, then read
    /// it): kept as this model's body, so nothing downstream thins the mesh a second time. `None`
    /// (a mesh too small to read) leaves the body to be read on first use.
    pub(crate) fn read_by_fit(&mut self, body: Option<Body>) {
        self.geometry_changed();
        if let Some(b) = body {
            let _ = self.body.set(b);
        }
        // The fit bound this mesh itself (`rig_raw_mesh` ends in the bind).
        self.own_skin = true;
    }

    /// Re-derive the world frames, applying the authored per-bone offsets on top of the
    /// conformed rest pose. `offsets` is empty until the Conform stage authors any.
    pub(crate) fn rebuild(&mut self, offsets: &[BoneOffset]) {
        let (globals, parents) = rest_globals(&self.model, offsets);
        let (centre, radius, floor, half_extent) = model_bounds(&self.model, &globals);
        self.centre = centre;
        self.radius = radius;
        self.half_extent = half_extent;
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

    /// `root` and every bone under it, parents before children — the hand and its fingers
    /// when `root` is the hand. What a SOLO view draws and what MIRROR → reflects.
    pub(crate) fn subtree(&self, root: usize) -> Vec<usize> {
        let n = self.parents.len();
        if root >= n {
            return Vec::new();
        }
        let mut out = vec![root];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i];
            out.extend(
                self.parents
                    .iter()
                    .enumerate()
                    .filter(|(_, &p)| p == cur as i32)
                    .map(|(c, _)| c),
            );
            i += 1;
        }
        out
    }
}

/// One bone's authored correction, applied on top of the conform result. This is the
/// AUTHORED data; the posed skeleton is derived from it, so "Reset bone" is just resetting it.
///
/// The three fields are the three things the gadget can do to a joint: `t` is Translate,
/// `roll` is Rotate (the bone's own X axis is the only rotation axis a limb chain wants),
/// and `scale` is Scale — the multiplier folded onto `RawBone::scale`, which is why the
/// identity is ONE and not zero and why `Default` is written out rather than derived.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoneOffset {
    /// Translation in source units (cm), parent-relative — the same space as `RawBone::translation`.
    pub(crate) t: [f32; 3],
    /// Roll about the bone's own X axis, in degrees.
    pub(crate) roll: f32,
    /// Per-axis multiplier on the bone's own local scale — identity is `[1.0; 3]`.
    pub(crate) scale: [f32; 3],
}

impl Default for BoneOffset {
    fn default() -> Self {
        Self {
            t: [0.0; 3],
            roll: 0.0,
            scale: [1.0; 3],
        }
    }
}

impl BoneOffset {
    /// Does this offset change nothing? (The "Reset bone" target, and the fold's skip.)
    fn is_identity(&self) -> bool {
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

/// WHAT ONE REGION KNOB WRITES (spec 0A81088E T2) — the four edits the Regions panel offers,
/// each landing on `model.regions[i]` through the ONE seam [`Document::edit_region`]. The two
/// steppers carry their direction (−1 / +1), the chain count arrives as TYPED text and the
/// stiffness as the slider's committed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RegionEdit {
    Tag(i32),
    Anchor(i32),
    Chains(String),
    Stiffness(f32),
}

/// THE REGION TAG VOCABULARY, once: the tag, the stem a hand-authored region is named from, and
/// its stringtable token. The stepper walks this order, the rows read the token and the names come
/// off the stem — one table, so a new tag cannot land in two of the three (1B64FF03).
pub(crate) const REGION_TAGS: [(RegionTag, &str, &str); 6] = [
    (RegionTag::Cloth, "cloth", "$ap_region_cloth"),
    (RegionTag::Hair, "hair", "$ap_region_hair"),
    (RegionTag::Mane, "mane", "$ap_region_mane"),
    (RegionTag::Tail, "tail", "$ap_region_tail"),
    (RegionTag::Pendant, "pendant", "$ap_region_pendant"),
    (RegionTag::Appendage, "appendage", "$ap_region_appendage"),
];

/// A tag's row in [`REGION_TAGS`] (`Cloth`'s, for a tag the table somehow lacks — the default the
/// contract itself declares).
pub(crate) fn region_tag(tag: RegionTag) -> (RegionTag, &'static str, &'static str) {
    REGION_TAGS
        .iter()
        .copied()
        .find(|t| t.0 == tag)
        .unwrap_or(REGION_TAGS[0])
}

/// One step of a STEPPER over `len` names, wrapping — the region's tag and anchor are rings, not
/// the skeleton pick's clamped list: there is no "first" tag to stop at.
fn step_index(at: usize, step: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let n = len as i32;
    (((at as i32 + step) % n + n) % n) as usize
}

/// How many UNMATCHED modules the MATCH STATUS line names before it trails off. The caption is
/// ONE wrapped line in the Rig cell, and the rail itself walks every prompt one at a time — the
/// line says how much is left and where it starts, not the whole list (a seven-trunk monster's
/// would be a paragraph).
const MATCH_NAMED: usize = 4;

/// A comb wider than this is not a garment panel; the field is typed, so it needs a ceiling.
const MAX_CHAINS: u32 = 32;
/// The jiggle stiffness's top — the sensitive band is 0.01–0.02 (duster analysis EC30FD2E), and
/// past a tenth the chain is rigid anyway.
const MAX_STIFFNESS: f32 = 0.1;

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

/// How the MARKERS RAIL's three step buttons move (spec FF40E825).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkerStep {
    /// ACCEPT — on to the next joint that still wants a human. The rail's ONLY forward move: a
    /// drag's release never makes it (incident 9715303C).
    NextUnplaced,
    /// SKIP — one place along the list, placed or not ("not this one").
    Skip,
    /// BACK — one place back ("let me have another look at that").
    Back,
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
    /// THE MARKERS RAIL's PLACED signal (spec FF40E825, the thing the Fill verb lacked —
    /// 20CA9653): which joints a HUMAN has put where he wants them. Parallel to the model's
    /// bones, and it lives HERE beside `offsets` for the same reason: a re-composed or
    /// re-conformed skeleton builds a fresh `Rig`, so the flags go with the bones they described
    /// structurally rather than by anyone remembering to clear them. Never set by Infer.
    pub(crate) placed: Vec<bool>,
    /// Where the markers rail stands — an index into [`Document::markers`].
    pub(crate) marker: usize,
    /// WHAT THE SHAPE GRAPH MATCHED on this body — the `FitReport::shape` the fit produced
    /// (spec 04803E0C, S2 431D08DF), kept with the rig it produced and cleared with it, exactly
    /// like `placed` above: a re-prepped or re-conformed body builds a FRESH `Rig`, so the rail
    /// resets to the new match structurally rather than by anyone remembering to.
    ///
    /// `None` when no fit ran at all (a vendor rig conformed onto the canon, a staged rig
    /// re-opened) or when the mesh had no readable shape — nothing matched, so the rail prompts
    /// the WHOLE depth-first walk, which is what it has always done.
    pub(crate) shape: Option<ShapeMatch>,
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
    /// Why the last Commit wrote NOTHING, until one succeeds — the Review page's own line
    /// (Aaron 2026-09-07: "it is unclear if anything happens, some kind of message needs to
    /// be provided").
    pub(crate) commit_error: Option<String>,
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
    /// PREP stage FACING (2026-09-11): quarter-turns (×90° about the vertical Z) applied to a
    /// RAW source mesh before it is rigged, so a body authored broadside (Meshy generates a
    /// quadruped from a side profile — its nose runs along X while the canon rig faces −Y) is
    /// turned to face the rig. THE HUMAN'S turns, on top of [`Document::facing_yaw`], which has
    /// already squared the body: what is left for him is the 180° when a source comes out
    /// tail-first. Opens at 0 and is cycled by the Facing control. Rides only with a raw mesh in
    /// Prep; a committed rig has it baked in.
    pub(crate) facing_quarters: u8,
    /// THE MEASURED FACING (degrees of yaw about Z): what `flicker_content::measure_facing` read
    /// off the SOURCE'S OWN FLESH when it was cached — the turn that lays the body's long
    /// horizontal axis on the rig's forward. Measured, because the bounding box is a bull's
    /// horns and a ewe's wool, and because seven of the seventeen hoofed sources are yawed
    /// 30–55° off the axis, which no whole quarter-turn squares (sweep 2026-09-21).
    pub(crate) facing_yaw: f32,
    /// PREP stage SQUARE FROM (Aaron's ruling FEFDA2B2): which side the bake-time stance
    /// normaliser mirrors a MID-STRIDE body from — `Auto` takes the planted limb of each pair,
    /// `Left`/`Right` name the source outright ("there's no guarantee it will be one specific
    /// side"). Rides with the body like the facing knob above, and is read by the ONE bake path,
    /// so Preview and Commit square the same way.
    pub(crate) stance_source: StanceSource,
    /// PREP stage MIRROR FROM (697DEC55 + FEFDA2B2): which half of a LOPSIDED source the mesh
    /// mirror KEEPS and reflects — `None` is "Off", the default, because a mirror destroys
    /// everything one-sided that is not tagged. A SOURCE-SHAPE fix like the facing knob beside it,
    /// so it is applied in [`Self::rebuild_prepped_model`] (after the turn, so X = 0 really is the
    /// body's median plane) and the skeleton is fitted to the mirrored body.
    pub(crate) mirror_keep: Option<Side>,
    /// PREP stage FACE FORWARD (164AE2F3): un-turn a head bound off −Y. ON BY DEFAULT (Aaron
    /// 2026-09-28, A79A6131: turned heads are common across the generated sources, not only the
    /// birds' 45°) — the fit lays the neck along the path the shape took and turns the face with
    /// it, and a head within 5° of forward is left alone; the checkbox is the opt-out. Rides with
    /// the body like the knobs above and is read by the ONE bake path (right after
    /// `square_stance`), so Preview and Commit un-turn the same head.
    pub(crate) face_forward: bool,
    /// PREP stage HANG (cm) (spec 0A81088E): how far off the body a vertex must stand to read as
    /// CLOTH — the region split's one measurement. Rides with the body like the two knobs above
    /// and opens at the bake's own [`flicker_content::DEFAULT_HANG_CM`], so pressing SPLIT without
    /// touching it reproduces exactly what `bake_garment` does.
    pub(crate) hang_cm: f32,
    /// The SKELETON pick (the modular skeleton system, 2026-09-07): the shipped presets and
    /// which one a raw mesh is composed onto at Conform. Humanoid first, always present.
    pub(crate) presets: Vec<Preset>,
    pub(crate) preset: usize,
    /// The MODULE edits on top of the pick (P2c S4): the recipe the Prep controls composed from
    /// the picked preset as a starting point, `None` while the pick stands unedited. Rides with
    /// the body — written into the rig on commit, adopted back on re-open.
    pub(crate) recipe_edit: Option<SkeletonRecipe>,
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
    /// THE REGIONS' generation (spec 0A81088E T2): bumped by the ONE seam every region write goes
    /// through ([`Self::edit_region`] and the four verbs). The panels' highlight re-uploads off it,
    /// and it is what says the bake must run again — the regions decide the skin pin, the flesh
    /// mask and the cloth the rig carries, so nothing measured from the old membership may stand.
    pub(crate) region_gen: u64,
    /// Which region's row the list has picked — the row whose knobs the panel shows and whose
    /// vertices the panels highlight.
    region_sel: Option<usize>,
    /// GROW FROM CLICK is armed: the next press in the PERSPECTIVE panel is a seed, not a joint
    /// pick. One shot — the verb disarms whether or not the ray found the mesh.
    pub(crate) grow_armed: bool,
    /// A perspective REACH test is posing the view ([`Self::pose_reach`]): the working frames are
    /// a bent chain, not the rest, until [`Self::clear_pose`] springs them back.
    pose_live: bool,
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
            facing_quarters: 0,
            facing_yaw: 0.0,
            stance_source: StanceSource::default(),
            mirror_keep: None,
            face_forward: true,
            hang_cm: flicker_content::DEFAULT_HANG_CM,
            presets: load_presets(&skeletons_dir()),
            preset: 0,
            recipe_edit: None,
            prep: None,
            pose_gen: 0,
            mesh_gen: 0,
            region_gen: 0,
            region_sel: None,
            grow_armed: false,
            pose_live: false,
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
    ///
    /// File selection is the OPERATING SYSTEM's dialog through the public `rfd` crate
    /// (Aaron, ruling of 2026-09-04, AAD0DC4B): NSOpenPanel on macOS, IFileDialog on
    /// Windows, GTK3 / the XDG desktop portal on Linux. The in-engine `file_browser`
    /// modal that briefly replaced it is reverted — it was not needed and did not work.
    #[cfg(not(test))]
    pub(crate) fn pick_folder() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Open asset source folder")
            .pick_folder()
    }

    /// The headless test build has no OS dialog to block on — this is the INJECTION
    /// SEAM that keeps the Source step gateable: a test arms [`stub_pick`] with the
    /// folder the dialog would have returned (or leaves it empty for the cancel path),
    /// and every pick consumes it exactly once, so a gate can prove the arm CALLS the
    /// seam and that what it returns is what gets opened. No test ever opens a real
    /// dialog.
    #[cfg(test)]
    pub(crate) fn pick_folder() -> Option<PathBuf> {
        PICK_STUB.with(|c| c.borrow_mut().take())
    }

    /// Ingest a folder that has already been chosen. Split from the dialog so the whole wizard
    /// downstream of it is exercisable without a GUI.
    pub(crate) fn open(&mut self, dir: PathBuf) {
        // Every open starts Prep CLEAN: a cache left by an earlier open of the same folder
        // would pass `ensure_prep_source`'s "re-entered Prep" test on a re-opened staged rig
        // and rebuild the working mesh from the stale decimation at whatever height was stored
        // last — throwing the adopted skeleton away (Aaron 2026-09-07).
        self.prep = None;
        self.recipe_edit = None;
        self.facing_quarters = 0;
        self.facing_yaw = 0.0;
        self.stance_source = StanceSource::default();
        self.mirror_keep = None;
        self.face_forward = true;
        self.preset = self.default_pick();
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
                    commit_error: None,
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
                    // Either Task-page box brings the rigged version back: "Import as rigged"
                    // (`prefer_staged`) by name, and "as provided" too — on a boneless source
                    // the only rig anyone provided is the one committed last time (Aaron
                    // 2026-09-07: "If I have checked the 'import as rigged' I should load the
                    // version that is in staging").
                    if self.prefer_staged || self.as_provided {
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
        // A creature lives in its own tier (`creatures/`), a character in `characters/`.
        let creature = self.pending_class == Some(AssetClass::Creature);
        let package = flicker_content::roots().package();
        let (staging, package) = if creature {
            (creatures_dir(), package.join("creatures"))
        } else {
            (characters_dir(), package.join("characters"))
        };
        for (root, origin) in [(staging, "staging"), (package, "package")] {
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
        let Some(name) = self.source.as_ref().map(|s| s.asset_name().to_string()) else {
            return false;
        };
        let path = root.join(&name).join(format!("{name}.json"));
        // The staged rig's own SKELETON RECIPE becomes the document's (the modular skeleton,
        // 2026-09-07; modules 2026-09-08): the pick follows its preset name, and any MODULE
        // edits it carries beyond that preset ride along as the working edit — so the
        // requirements count ITS bones and a re-commit writes the same recipe back, not
        // whatever the Prep stepper happened to rest on.
        if let Some(staged) = staged_recipe(&path) {
            if let Some(i) = staged
                .preset
                .as_deref()
                .and_then(|want| self.presets.iter().position(|p| p.name == want))
            {
                self.preset = i;
            }
            let picked = self.picked_recipe();
            self.recipe_edit = (staged.trunk != picked.trunk).then_some(SkeletonRecipe {
                trunk: staged.trunk,
                preset: picked.preset.or(staged.preset),
            });
        }
        let recipe = self.recipe();
        let Some(src) = self.source.as_mut() else {
            return false;
        };
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
        // The staged body IS the decimated, sized mesh Prep produced last time: read its height
        // and triangle count back into the Prep fields, so the bench shows the quality and size
        // it was committed at instead of the defaults (Aaron 2026-09-07).
        let (lo, hi) = model
            .vertices
            .iter()
            .map(|v| v.p[2])
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), z| {
                (lo.min(z), hi.max(z))
            });
        let height = if hi > lo { hi - lo } else { 0.0 };
        let tris = model.indices.len() / 3;
        let parsed = Parsed::new(model);
        // THE MATCH, READ-ONLY (incident 118CEA35): the body is read once — the one read every
        // later bake asks of this mesh (DD7A59A9) — and the recipe matched to its shape graph, so
        // the rail asks only for the modules the shape could not answer, exactly as after a fresh
        // fit. Nothing moves: the committed joints are the work being kept, and the match is a
        // reading of the mesh, not of them.
        let shape = parsed
            .body()
            .graph
            .as_ref()
            .map(|graph| flicker_content::match_recipe(graph, &recipe));
        src.report = Some(classify_asset(&src.scan, Some(parsed.bones())));
        src.parsed = Some(parsed);
        src.rig = Some(Rig {
            rename: RenameReport::default(),
            out: ConformOutput::default(),
            map: vec![MapState::Ok; n],
            offsets: vec![BoneOffset::default(); n],
            sel: 0,
            placed: vec![false; n],
            marker: 0,
            shape,
        });
        src.reopened = Some(origin);
        src.resolve_attach();
        src.error = None;
        if height > 0.0 {
            self.stature_cm = height.round();
        }
        if tris > 0 {
            self.decimate_target = tris.to_string();
        }
        tracing::info!("re-opened {origin} rig {name}: {n} bones, {height:.0} cm, {tris} tris");
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
        // The raw-mesh rig path needs the target stature and the skeleton pick (both set on
        // Prep). Read before borrowing `source`.
        let stature = self.stature_cm;
        let recipe = self.recipe();
        let skeleton = self.skeleton_name();
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
        // (An unclassified asset falls through to the character path, as it always has.) A
        // CREATURE is a character on a non-humanoid recipe (the modular skeleton, 2026-09-08):
        // it takes the same path on its Prep pick — a quadruped by default.
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
            // The ONE raw-mesh rig sequence (scale → fit arms, legs, tail → skin; the weight-
            // ownership hip fit + re-skin only when the legs found no leg tubes) — shared with
            // the headless `import_folder`, so a bench bake and a CLI bake are the same bake.
            let fit = match rig_raw_mesh(&mut parsed.model, stature, &recipe) {
                Ok(fit) => fit,
                Err(e) => {
                    src.error = Some(format!("Skeleton `{skeleton}` cannot be installed: {e}"));
                    return;
                }
            };
            // It scaled the mesh to the stature and then READ it: that read is the body every
            // later question about this mesh is asked of (DD7A59A9).
            parsed.read_by_fit(fit.body);
            // THE HAND-OFF (ruling 7881216F): every boneless appendage the bind found — a horn,
            // an antler, an ear — arrives as a row of the Regions panel, rigid or (flat) soft,
            // for the human to flip. Read off the fit's own body, which stays the body: these
            // regions are what that read found, not a change to it.
            let found = flicker_content::bake::Seating::read(&parsed.model, parsed.body())
                .map(|s| s.appendages(&parsed.model))
                .unwrap_or_default();
            let proposed = flicker_content::appendage_regions(&parsed.model, &found);
            for r in &proposed {
                tracing::info!(
                    "appendage: {} on {} — {} vertices, {} chain(s)",
                    r.name,
                    r.anchor_bone,
                    r.verts.len(),
                    r.chain_count
                );
            }
            parsed.model.regions.extend(proposed);
            let n = parsed.model.bones.len();
            parsed.rebuild(&[]);
            // WHAT MATCHED WHAT (S2 431D08DF). The rail prompts the modules the matcher left
            // UNMATCHED, so the match has to reach the document: `rig_raw_mesh` — the ONE raw-mesh
            // rig sequence, shared with the headless import so a bench bake and a CLI bake are the
            // same bake — hands back the fit's own `FitReport`, and its match IS the answer the fit
            // acted on. Nothing here thins the mesh a second time to re-derive it (0F0208AC).
            let shape = fit.shape;
            if let Some(m) = shape.as_ref() {
                for warn in &m.warnings {
                    tracing::warn!("fit: {warn}");
                }
                tracing::info!(
                    "shape match: {} module(s) matched, {} left for the rail",
                    m.matched.len(),
                    m.unmatched.len()
                );
            }
            tracing::info!(
                "installed skeleton `{skeleton}` on raw mesh: {n} bones at {stature}cm, skinned"
            );
            src.rig = Some(Rig {
                rename: RenameReport::default(),
                out: ConformOutput::default(),
                map: vec![MapState::Ok; n],
                offsets: vec![BoneOffset::default(); n],
                sel: 0,
                placed: vec![false; n],
                marker: 0,
                shape,
            });
            src.resolve_attach();
            src.error = None;
            self.region_gen = self.region_gen.wrapping_add(1);
            return;
        }
        let rename = rename_to_canonical(&mut parsed.model);
        match conform_to_canonical(&mut parsed.model, &default_reference(), mode) {
            Ok(out) => {
                let map = bone_map_states(&parsed.model, &out);
                let n = parsed.model.bones.len();
                parsed.geometry_changed(); // the reorient turned the mesh
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
                    placed: vec![false; n],
                    marker: 0,
                    // A VENDOR RIG corrected onto the canon: the shape graph never ran (the
                    // skeleton arrived with the mesh), so there is no match and the rail asks
                    // for the whole walk.
                    shape: None,
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
        // A COMPOSED body (a raw mesh rigged on the Prep pick) and a RE-OPENED bake keep the
        // IDENTITY frames their pattern skeleton and clip libraries were baked with; only a
        // fresh VENDOR rig is turned onto the canonical frames by the conform (Aaron
        // 2026-09-07: the reorient had turned the lizardman's foot frame 44° off its clips).
        // SQUARE THE STANCE before the frames are settled: a mid-stride source is bound on the
        // pose the human rigged, and the normaliser puts the raised limb at the planted one's
        // reflection and carries the skin into it (ruling 42AB9BA8). Inside the ONE bake path, so
        // the Preview page plays the squared body and Commit writes exactly that — never in the
        // Rig step, where the human is placing joints on the mesh AS POSED.
        // Its feet are read off the body the fit already read, kept beside the model (DD7A59A9):
        // an authored offset moves frames, never vertices, so that read still describes this mesh.
        let recipe = self.rig_recipe(&model);
        // How the body sits in its limbs AS POSED, taken before an un-pose moves one — the skin
        // it moves in is put on last, on this seating. Only for a skin the bench bound itself.
        let seating = parsed
            .own_skin
            .then(|| flicker_content::bake::Seating::read(&model, parsed.body()))
            .flatten();
        let stance = flicker_content::square_stance_on(
            &mut model,
            self.stance_source,
            &recipe,
            Some(parsed.body()),
        );
        let mut moved = !stance.squared.is_empty();
        for (limb, lift) in stance.squared {
            tracing::info!("square_stance: {limb} squared ({lift:.1} cm of lift)");
        }
        for (limb, sunk) in stance.declined {
            tracing::warn!(
                "square_stance: {limb} left as posed — squaring it would sink the mesh {sunk:.1} cm \
                 through its own floor"
            );
        }
        // FACE FORWARD, right after it and on the same bound skin (164AE2F3): a head bound off −Y
        // (the generated birds' 45°) is un-turned about the neck chain, the skin following the same
        // rigid blend. Inside the ONE bake path, so Preview == Commit here too.
        if self.face_forward {
            let r = flicker_content::face_forward(&mut model);
            tracing::info!(
                "face_forward: the head was {:.1}° off forward ({})",
                r.yaw_deg,
                if r.turned { "turned" } else { "left as posed" }
            );
            moved |= r.turned;
        }
        // THE SKIN IT MOVES IN, last (Aaron on the Elk, 2026-10-02: the walk moved four tubes
        // under a statue): the working skin and both un-poses are the TUBE skin, and what the
        // Preview plays and Commit writes carries each standing limb's girdle on that limb's
        // bones — round those bones as they now lie (a joint the human moved takes its flesh
        // with it), through the flesh as it now stands: the kept read unless an un-pose moved
        // it.
        let as_posed = (!moved).then(|| parsed.flesh());
        flicker_content::bake::bind_for_motion(&mut model, seating.as_ref(), as_posed);
        if self.prep.is_some() || src.reopened.is_some() {
            flicker_content::straighten_frames(&mut model);
        } else {
            reorient_to_canonical(&mut model, &default_reference())
                .map_err(|e| format!("Canonical frame translation failed: {e}"))?;
        }
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
        // The picked skeleton's PATTERN bake when one is shipped (`locomotion@<Pattern>`, the
        // P2c library rule), else the shared humanoid idle — so a lizardman previews on the
        // basis its clips were baked for.
        let package = flicker_content::roots().package();
        let by_preset = package.join(format!(
            "retarget/clips/locomotion@{}/In-Place/idle_neutral.json",
            self.recipe().pattern().name()
        ));
        let text = match flicker_content::package::read_text(&by_preset) {
            Ok(text) => text,
            // A CREATURE pattern has no biped basis to fall back on: the preview gets the
            // trackless REST clip, and the bench's bake preview walks it under the gait
            // generator (`meshes::GeneratedWalk`, G6 of the gait/IK design) rather than
            // bending a biped idle onto four legs or a pair of wings.
            Err(_)
                if matches!(
                    self.recipe().pattern(),
                    Pattern::Quadruped | Pattern::Bird | Pattern::Bat
                ) =>
            {
                return Ok((
                    rig_file,
                    bones,
                    ResolvedClip {
                        name: "rest".to_string(),
                        tick_rate_hz: 60,
                        duration_ticks: 60,
                        tracks: Vec::new(),
                        unresolved: Vec::new(),
                    },
                ));
            }
            Err(_) => {
                let idle = package.join(BAKE_PREVIEW_CLIP);
                flicker_content::package::read_text(&idle)
                    .map_err(|e| format!("shared idle {}: {e}", idle.display()))?
            }
        };
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
            (Some(AssetClass::Creature), _) => creatures_dir(),
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
                    s.commit_error = Some(e.clone());
                    s.error = Some(e);
                }
                return;
            }
        };

        // What the rig is made of: the picked skeleton when it composes to exactly these bones
        // (a raw mesh composed on the Prep pick, or a staged rig whose pick was re-adopted on
        // re-open — a staged lizard used to be written back as a Humanoid, 2026-09-07); a
        // vendor rig conformed onto the canonical reference IS the humanoid recipe.
        let recipe = self.rig_recipe(&model);
        let dir = root.join(&name);
        let out = dir.join(format!("{name}.json"));
        let result: std::result::Result<(), String> = std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))
            .and_then(|()| match class {
                // Clothing is a garment: a mesh SKINNED onto the base, its fit baked into the verts.
                Some(AssetClass::Prop) if prop == PropKind::Clothing => {
                    write_garment(&model, &fbx, &name, &out, &fit, self.hang_cm)
                        .map_err(|e| e.to_string())
                }
                // Any other prop is a rigid static mesh; the authored fit is written into its attach.
                Some(AssetClass::Prop) => {
                    // POC flat-colour (bake_prop) flows through the headless `import_prop` example,
                    // not the bench UI yet — textured props pass None here (unchanged behaviour).
                    write_prop(&model, &fbx, &name, &out, &fit, None).map_err(|e| e.to_string())
                }
                // A creature rigged on its recipe bakes a rig like a character (2026-09-08);
                // one with NO rig yet — Conform never ran — ships the prepped (sized,
                // collapsed) mesh as a static bake, the same file shape as an environment
                // prop, so it can be looked at now and rigged later.
                Some(AssetClass::Creature) => {
                    if has_rig {
                        write_rig(&model, &fbx, &name, &out, &mounts, Some(&recipe))
                            .map_err(|e| e.to_string())
                    } else {
                        write_prop(&model, &fbx, &name, &out, &Fit::default(), None)
                            .map_err(|e| e.to_string())
                    }
                }
                // Animation never reaches this dispatch — routed to `commit_clip_to` above.
                Some(AssetClass::Animation) => unreachable!("animation commits via commit_clip_to"),
                // Character: requires the conform to have produced a rig.
                _ => {
                    if has_rig {
                        write_rig(&model, &fbx, &name, &out, &mounts, Some(&recipe))
                            .map_err(|e| e.to_string())
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
                src.commit_error = None;
                src.error = None;
            }
            Err(e) => {
                src.commit_error = Some(e.clone());
                src.error = Some(e);
            }
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
        // synthesizes — so the figure here is the one the shipped rig will carry. The count it
        // must match is the PICKED skeleton's (the modular skeleton, 2026-09-07 — a lizardman
        // composes to 75, and "75 / 67 BLOCKED" on a rig that commits fine cost Aaron a
        // rig he thought was lost), which is the canon's 67 for a vendor rig conformed to it.
        let conformed = src.parsed.as_ref().map(|p| p.bones()).unwrap_or(0);
        let baked = if conformed == 0 { 0 } else { conformed + 1 };
        let expected = self.recipe_bones();
        let mut out = vec![(
            baked == expected,
            format!(
                "{} ({baked} / {expected} {})",
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
        let Some(model) = self.source.as_ref().and_then(|s| s.parsed.as_ref()) else {
            return;
        };
        let dt = parent_local_delta(globals, &model.model, sel, world_delta);
        if !dt.is_finite() {
            return;
        }
        self.edit_offset(sel, |o| {
            for k in 0..3 {
                o.t[k] += dt[k];
            }
        });
    }

    /// ROTATE the selected bone's authored offset by the gadget's world-space `delta`.
    ///
    /// [`BoneOffset`] carries ONE rotation — `roll`, about the bone's own X axis, which is the axis
    /// a limb chain actually wants and the one the `off_roll` dial writes. So the world delta is
    /// projected onto that axis (the swing-twist twist angle) and only its twist reaches the offset:
    /// the dial and the gadget keep writing ONE value instead of two rotations that could disagree.
    /// A ring turned about an axis the bone cannot express contributes nothing, which is the honest
    /// answer rather than a silent second rotation channel.
    pub(crate) fn apply_gizmo_rotate(&mut self, sel: usize, globals: &[Mat4], delta: glam::Quat) {
        let Some(axis) = globals
            .get(sel)
            .map(|g| g.x_axis.truncate())
            .and_then(|a| a.try_normalize())
        else {
            return;
        };
        let twist = 2.0 * delta.xyz().dot(axis).atan2(delta.w);
        if !twist.is_finite() || twist == 0.0 {
            return;
        }
        self.edit_offset(sel, |o| o.roll += twist.to_degrees());
    }

    /// SCALE the selected bone's authored offset by the gadget's per-axis `factors` (multiplicative,
    /// in the bone's local frame — the space [`BoneOffset::scale`] multiplies). Floored at
    /// [`flicker_mechanics::MIN_SCALE`], so a drag through the pivot pins rather than inverting the
    /// bone (a negative scale is a reflection, and mirroring is `flip`'s guarded job).
    pub(crate) fn apply_gizmo_scale(&mut self, sel: usize, factors: Vec3) {
        if !factors.is_finite() {
            return;
        }
        self.edit_offset(sel, |o| {
            for k in 0..3 {
                o.scale[k] = (o.scale[k] * factors[k]).max(flicker_mechanics::MIN_SCALE);
            }
        });
    }

    /// MIRROR the selected bone's authored offset onto its `_l`/`_r` twin, through the gadget's
    /// guarded reflection `m` (invariant C670523A: the caller's validator has already refused a
    /// joint with no twin, so reaching here means one exists). The authored translation is
    /// parent-local, so it goes to world, through the reflection, and back into the TWIN's parent
    /// frame; the roll negates because a reflection reverses handedness, and the scale carries
    /// across unchanged (its magnitude is the same on both sides).
    ///
    /// `false` when there is no twin — the same answer [`Self::mirror_of`] gives, so a caller that
    /// skipped the validator still cannot write invalid geometry.
    pub(crate) fn mirror_offset(&mut self, sel: usize, globals: &[Mat4], m: Mat4) -> bool {
        let Some(twin) = self.mirror_of(sel) else {
            return false;
        };
        let Some(off) = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .and_then(|r| r.offsets.get(sel))
            .copied()
        else {
            return false;
        };
        let Some(model) = self.source.as_ref().and_then(|s| s.parsed.as_ref()) else {
            return false;
        };
        let world = parent_basis(globals, &model.model, sel) * Vec3::from_array(off.t);
        let reflected = glam::Mat3::from_mat4(m) * world;
        let t = parent_basis(globals, &model.model, twin).inverse() * reflected;
        if !t.is_finite() {
            return false;
        }
        self.edit_offset(twin, |o| {
            *o = BoneOffset {
                t: t.to_array(),
                roll: -off.roll,
                scale: off.scale,
            };
        });
        true
    }

    /// Edit one bone's authored offset in place, then re-derive the pose once and bump `pose_gen`
    /// so the live skin re-uploads — the shared tail of every authored-offset write.
    fn edit_offset(&mut self, sel: usize, edit: impl FnOnce(&mut BoneOffset)) {
        let Some(src) = self.source.as_mut() else {
            return;
        };
        let Some(slot) = src.rig.as_mut().and_then(|r| r.offsets.get_mut(sel)) else {
            return;
        };
        let before = *slot;
        edit(slot);
        if *slot == before {
            return;
        }
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
        // THE PLACED SIGNAL (spec FF40E825): this is the HAND's own path — the gadget emits a
        // delta only when the drag really moved (so a click that travelled nowhere still marks
        // nothing, incident E4C6CED5) — and the mirrored twin was placed by the same hand. The
        // release's depth resolve rides this same drag and needs no second mark; Infer writes
        // through `reposition_bone_with` directly and so never marks.
        self.mark_placed(sel);
        if let Some(m) = mirror {
            self.mark_placed(m);
        }
        self.reposition_bone_with(sel, globals, world_delta, mirror);
    }

    /// Record that a human put joint `i` where he wants it. A no-op without a rig.
    fn mark_placed(&mut self, i: usize) {
        if let Some(f) = self
            .source
            .as_mut()
            .and_then(|s| s.rig.as_mut())
            .and_then(|r| r.placed.get_mut(i))
        {
            *f = true;
        }
    }

    /// THE GUIDED RIG's depth read (spec 76EB9552, ruling F9F728CA): put the joints an
    /// orthographic drag just moved where the MESH's own run says, along the one axis that panel
    /// cannot show — [`Flesh::limb_depth`], which answers by what the run IS. A limb run gives its
    /// midpoint; a body run gives the symmetry plane for a midline joint in the LEFT view, or the
    /// midpoint of the first limb run down the joint→child bone (the hip over its own leg hole);
    /// anything else keeps the depth the hand gave it.
    ///
    /// `depth` is the panel's own view direction ([`flicker_rigview::Projection::depth_axis`]), so
    /// the hidden axis is read from the picture and never re-derived here. The human supplies the
    /// two axes he can see; the mesh supplies the third — Aaron's "the app determines the correct
    /// depth in the mesh to be at the middle of the body mass".
    ///
    /// Called on the RELEASE edge of a drag that actually moved something (commit-on-release
    /// B694F6B1; a click that travelled nowhere still places nothing — incident E4C6CED5).
    /// Mid-drag the joint rides at its own depth, exactly as the press table says.
    ///
    /// The selected joint and, when symmetry mirrored the drag onto it, its twin are each
    /// resolved in their OWN column: a LEFT-view column through a knee crosses both legs, and the
    /// run nearest the joint's own coordinate is its leg. A joint the rule cannot justify — an
    /// EMPTY column (dragged clean off the silhouette), a spine joint in the FRONT view, a
    /// clavicle buried in the chest — stays exactly where the hand put it.
    /// Everything moves through [`Self::reposition_bone_with`], the one offset writer.
    ///
    /// Returns how many joints moved.
    pub(crate) fn resolve_drag_depth(&mut self, sel: usize, depth: Vec3) -> usize {
        let a = depth.abs();
        let axis = if a.x >= a.y.max(a.z) {
            0
        } else if a.y >= a.z {
            1
        } else {
            2
        };
        let mirror = self.mirror_joints.then(|| self.mirror_of(sel)).flatten();
        let mut moved = 0;
        for joint in [Some(sel), mirror].into_iter().flatten() {
            // A joint with no twin is a MIDLINE joint — the symmetry plane is its own answer.
            let midline = self.mirror_of(joint).is_none();
            let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) else {
                break;
            };
            // The joint's CURRENT rest world position: resolving the selected joint may have
            // carried this one's parent, so the twin is read after, never before.
            let Some(pos) = p.globals.get(joint).map(|g| g.w_axis.truncate()) else {
                continue;
            };
            let child = limb_child(p, joint);
            let Some(mid) = p.flesh().limb_depth(pos, child, axis, midline) else {
                continue; // no run the rule can justify — the hand's depth stands
            };
            let globals = p.globals.clone();
            let mut delta = Vec3::ZERO;
            delta[axis] = mid - pos[axis];
            if delta.length() <= 1e-4 {
                continue;
            }
            self.reposition_bone_with(joint, &globals, delta, None);
            moved += 1;
        }
        moved
    }

    /// MIRROR →: put every twin of the selected joint's SUBTREE where the mirror image of its
    /// partner is — the partner's rest world position reflected across the model's median
    /// plane (world X = 0: the import plants the mesh on the plumb line and the root at the
    /// origin). Parents first, each twin moved from where it now is, through the same
    /// reposition path an orthographic drag uses (re-bound, permanent, `pose_gen` bump), and
    /// never mirrored back — the partner is the truth and stays put. Returns how many twins
    /// moved; a subtree with no twins moves nothing. The reset Aaron asked for (2026-09-08):
    /// a delta-mirrored drag keeps whatever asymmetry the two sides already had, this removes it.
    pub(crate) fn mirror_to_twin(&mut self, sel: usize) -> usize {
        let Some(p) = self.parsed() else {
            return 0;
        };
        let pairs: Vec<(usize, Vec3)> = p
            .subtree(sel)
            .into_iter()
            .filter_map(|i| {
                let twin = self.mirror_of(i)?;
                let w = p.globals.get(i)?.w_axis.truncate();
                Some((twin, Vec3::new(-w.x, w.y, w.z)))
            })
            .collect();
        let mut moved = 0;
        for (twin, target) in pairs {
            // The twin's CURRENT frames: its parent may have just been moved above it.
            let Some(p) = self.parsed() else {
                break;
            };
            let globals = p.globals.clone();
            let Some(now) = globals.get(twin).map(|g| g.w_axis.truncate()) else {
                continue;
            };
            let delta = target - now;
            if delta.length() <= 1e-4 {
                continue;
            }
            self.reposition_bone_with(twin, &globals, delta, None);
            self.mark_placed(twin); // a human verb putting a joint where he wants it
            moved += 1;
        }
        moved
    }

    /// [`Self::reposition_bone`] with the mirror decided by the caller: `Some(twin)` moves the
    /// twin by the X-negated delta as well, `None` moves the one bone only.
    fn reposition_bone_with(
        &mut self,
        sel: usize,
        globals: &[Mat4],
        world_delta: Vec3,
        mirror: Option<usize>,
    ) {
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
            // On the body already read: moving joints never makes a read stale (DD7A59A9).
            let body = p.body.get_or_init(|| Body::read(&p.model));
            flicker_content::bake::bind(&mut p.model, Some(body));
            p.own_skin = true;
            p.rebuild(&offsets);
        }
        self.pose_gen = self.pose_gen.wrapping_add(1);
    }

    // ── THE MARKERS RAIL (spec FF40E825, ruling 42AB9BA8) ───────────────────────────────────
    //
    // The Rig step's OPENING PHASE: the bench prompts for the joints that truly require a human
    // eyeball, one at a time, and INFER derives the rest. It is a rail over the EXISTING tools —
    // it selects a joint and frames it, and the free ortho drag underneath is what places it. It
    // moves nothing itself, on press or otherwise (incident E4C6CED5).
    //
    // WHICH joints it asks for is the SHAPE MATCHER's answer, not a list (rule 513E5F78; S2
    // 431D08DF): the modules the graph found a partner for are placed by the fit, and the rail
    // walks only the ones it did not — depth-first, parents before children (6234D0F6).

    /// WHAT THE SHAPE GRAPH MADE OF THIS BODY, if a fit ran and read one (S2 431D08DF).
    pub(crate) fn shape_match(&self) -> Option<&ShapeMatch> {
        self.source.as_ref()?.rig.as_ref()?.shape.as_ref()
    }

    /// ONE WALK of the rail's prompts for the working body: every joint [`markers_by_module`]
    /// derives from the document's recipe that the working rig actually CARRIES, in the
    /// depth-first order (Aaron's fetlock amendment 6234D0F6 — parents before children, each
    /// chain in bone order, twists and fan children riding Infer), each tagged
    /// `(module id, joint, matched)`.
    ///
    /// The carried-bones filter is why the walk is read here rather than taken whole: a rig
    /// conformed from a vendor skeleton can be missing a module the recipe names, and a prompt
    /// for a joint that is not there would be a rail the human cannot step off (4BB12A75).
    ///
    /// `matched` is [`ShapeMatch::marker_order`]'s own test — a module the matcher left OUT of
    /// `unmatched` found its partner in the mesh — read here for BOTH halves at once, because the
    /// bench needs the unmatched half (the rail's list) and the matched half (the joints Infer
    /// must hold) off a single walk. `the_rail_is_the_matchers_unmatched_walk` gates the two
    /// against `marker_order` itself, so the test cannot drift out of step with the matcher's.
    ///
    /// With NO match — no fit ran (a vendor rig corrected onto the canon, a staged rig re-opened)
    /// or the mesh had no readable shape at all — every row reads UNMATCHED, so the rail is the
    /// whole walk, which is what it has always been.
    fn marker_rows(&self) -> Vec<(String, String, bool)> {
        let Some(p) = self.parsed() else {
            return Vec::new();
        };
        let shape = self.shape_match();
        markers_by_module(&self.recipe())
            .into_iter()
            .filter(|(_, joint)| p.model.bones.iter().any(|b| &b.name == joint))
            .map(|(id, joint)| {
                let matched = shape.is_some_and(|m| !m.unmatched.contains(&id));
                (id, joint, matched)
            })
            .collect()
    }

    /// THE RAIL'S LIST — the prompts of the modules the shape matcher left UNMATCHED, in the
    /// depth-first walk order (rule 513E5F78: what does not match is exactly what the human is
    /// asked for). A MATCHED module is PLACED, not prompted: its joints stay selectable and
    /// draggable like any other, they are simply not on the rail.
    pub(crate) fn markers(&self) -> Vec<String> {
        self.marker_rows()
            .into_iter()
            .filter(|(_, _, matched)| !matched)
            .map(|(_, joint, _)| joint)
            .collect()
    }

    /// The other half of the same walk: the joints the MATCHER placed. Infer holds these exactly
    /// as it holds the ones the human placed — `fit_to_graph` laid the chain down the partner the
    /// match chose, and re-deriving it from the flesh would throw that answer away.
    fn matched_markers(&self) -> Vec<String> {
        self.marker_rows()
            .into_iter()
            .filter(|(_, _, matched)| *matched)
            .map(|(_, joint, _)| joint)
            .collect()
    }

    /// THE MATCH STATUS — one read-only line in the Rig cell saying what the shape graph made of
    /// this body: how many of the recipe's modules found a partner in the mesh, and which ones
    /// did not, named by the joint the rail will ask for first in each.
    ///
    /// The whole sentence is the stringtable's and the COUNTS AND NAMES are substituted in as
    /// data (D5ED9ACF; a bone is never translated), the same wiring the `Place: {}` caption has.
    /// Empty when there is no match to report — no fit ran, or the mesh had no readable shape.
    pub(crate) fn marker_match_caption(&self) -> String {
        let Some(m) = self.shape_match() else {
            return String::new();
        };
        let total = m.matched.len() + m.unmatched.len();
        // The FIRST prompt of each unmatched module, in the walk's own order — the joint the rail
        // opens on for that module, which is what names it to a human.
        let mut named: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (id, joint, matched) in self.marker_rows() {
            if !matched && !seen.contains(&id) {
                seen.push(id);
                named.push(joint);
            }
        }
        if named.is_empty() {
            return strings::resolve("$ap_marker_match_all")
                .replace("{n}", &m.matched.len().to_string())
                .replace("{total}", &total.to_string());
        }
        let more = named.len() > MATCH_NAMED;
        named.truncate(MATCH_NAMED);
        let names = named.join(", ") + if more { " \u{2026}" } else { "" };
        strings::resolve("$ap_marker_match")
            .replace("{n}", &m.matched.len().to_string())
            .replace("{total}", &total.to_string())
            .replace("{names}", &names)
    }

    /// Where the rail stands, clamped to the list (a re-composed body is a shorter list).
    pub(crate) fn marker(&self) -> usize {
        let n = self.markers().len();
        self.source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map_or(0, |r| r.marker)
            .min(n.saturating_sub(1))
    }

    /// The joint the rail is asking for — the caption's one piece of data.
    pub(crate) fn marker_name(&self) -> Option<String> {
        self.markers().get(self.marker()).cloned()
    }

    /// The rail's caption, pre-formatted like every other readout: the whole sentence out of the
    /// stringtable with the JOINT NAME substituted in as data. Empty when there is no rail (a
    /// boneless document, or a body whose recipe prompts for nothing).
    pub(crate) fn marker_caption(&self) -> String {
        match self.marker_name() {
            Some(name) => strings::resolve("$ap_marker_place").replace("{}", &name),
            None => String::new(),
        }
    }

    /// Has the human placed the joint named `name`?
    fn marker_placed(&self, name: &str) -> bool {
        self.parsed()
            .and_then(|p| p.model.bones.iter().position(|b| b.name == name))
            .and_then(|b| self.placed().get(b).copied())
            .unwrap_or(false)
    }

    /// Which joints a HUMAN has placed, parallel to the model's bones (empty without a rig).
    pub(crate) fn placed(&self) -> &[bool] {
        self.source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map_or(&[][..], |r| r.placed.as_slice())
    }

    /// Move the rail — the ONLY thing that does (incident 9715303C: a drag places the joint, the
    /// human ACCEPTS it; a release never walks the rail on). `ACCEPT` goes on to the next joint
    /// that still wants a human and marks nothing itself: the drag already marked the joint it
    /// moved (and its twin), so a joint accepted without ever being moved stays UNPLACED — Infer
    /// derives it, and a later ACCEPT comes back round to it. `SKIP` and `BACK` step the list one
    /// place either way, so every marker stays reachable however it was left. Returns whether the
    /// rail moved (the scene selects and frames it then, and only then).
    pub(crate) fn step_marker(&mut self, step: MarkerStep) -> bool {
        let markers = self.markers();
        let n = markers.len();
        if n == 0 {
            return false;
        }
        let at = self.marker();
        let want = match step {
            MarkerStep::Back => (at + n - 1) % n,
            MarkerStep::Skip => (at + 1) % n,
            // The next UNPLACED marker, wrapping; if every one is placed, stay put.
            MarkerStep::NextUnplaced => (1..=n)
                .map(|k| (at + k) % n)
                .find(|&i| !self.marker_placed(&markers[i]))
                .unwrap_or(at),
        };
        self.set_marker(want)
    }

    fn set_marker(&mut self, want: usize) -> bool {
        let Some(rig) = self.source.as_mut().and_then(|s| s.rig.as_mut()) else {
            return false;
        };
        let moved = rig.marker != want;
        rig.marker = want;
        moved
    }

    /// INFER (ruling 42AB9BA8: a BUTTON, never automatic — there is no undo in the Rig step).
    /// Derive every joint the human did NOT place from the ones he did and the mesh's own flesh:
    /// an unplaced BRANCH POINT or LEAF goes to the medial point of the flesh near it
    /// ([`Flesh::centre_near`]); every single-child RUN between two anchors is laid down the
    /// re-centred profile at its current arc fractions, nudged to the NARROWINGS
    /// ([`fill_chain`]) — the verb that has had no caller since slice 2 (3995EF9E).
    ///
    /// The targets all come from ONE snapshot of the rest frames, then are applied ROOT→TIP with
    /// the frames recomputed after each write (a reposition carries its subtree, so a child read
    /// from the snapshot must be measured against where its parent has just put it). PLACED
    /// JOINTS ARE NEVER TOUCHED — nor are the joints of a MODULE THE SHAPE MATCHER PLACED (S2
    /// 431D08DF: the rail does not prompt those either) — and nothing Infer writes is marked
    /// placed, so a second Infer after nudging one end re-derives only the joints neither the
    /// human nor the fit has spoken for. The ROOT is the ground origin the body stands on and is
    /// never inferred.
    ///
    /// Symmetry is held OFF for the duration: Infer answers per joint out of the joint's own
    /// flesh, and a mirrored write could otherwise move a twin the human had placed.
    ///
    /// Auto-depth's limb rule runs on every inferred joint afterwards (ruling F9F728CA) — the
    /// same `Flesh::limb_depth` read an ortho release makes, on X, where a midline joint takes
    /// the symmetry plane and a limb root takes its own limb's midpoint.
    ///
    /// Returns how many joints were inferred.
    pub(crate) fn infer_markers(&mut self) -> usize {
        let placed = self.placed().to_vec();
        // THE MATCHER'S JOINTS ARE HELD LIKE THE HUMAN'S (S2 431D08DF). A module that found its
        // partner in the mesh had its chain laid down that partner by `fit_to_graph`, at the
        // narrowings the match chose and with its ground joint extended onto the floor. Deriving
        // it again from the flesh would throw that answer away — and the rail does not prompt
        // these either, so the two halves of the walk stay the one split: unmatched is the
        // human's work, matched is the fit's, and Infer fills only what neither has answered.
        let matched = self.matched_markers();
        let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) else {
            return 0;
        };
        let n = p.model.bones.len();
        if n == 0 || placed.len() != n {
            return 0;
        }
        // `placed` keeps its own meaning throughout (it is the HUMAN's signal, and only a hand
        // sets it); `held` is the union Infer may not move.
        let mut held = placed.clone();
        for name in &matched {
            if let Some(i) = p.model.bones.iter().position(|b| &b.name == name) {
                held[i] = true;
            }
        }
        let rest: Vec<Vec3> = p.globals.iter().map(|g| g.w_axis.truncate()).collect();
        let parents: Vec<i32> = p.model.bones.iter().map(|b| b.parent).collect();
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, parent) in parents.iter().enumerate() {
            if let Ok(k) = usize::try_from(*parent) {
                if k < n {
                    kids[k].push(i);
                }
            }
        }
        // The medial search's reach: a twentieth of the body's height, the bound the spec states.
        let reach = 0.05 * 2.0 * p.half_extent.z.abs().max(1.0);
        // An ANCHOR is a joint the run-filler may not move: the root, one the human placed or the
        // matcher placed, and every branch point or leaf (which Infer answers on its own, below).
        let anchor = |i: usize| i == 0 || held[i] || kids[i].len() != 1;
        let flesh = p.flesh();
        let mut target: Vec<Option<Vec3>> = vec![None; n];
        for i in 0..n {
            if !anchor(i) || i == 0 || held[i] {
                continue;
            }
            target[i] = Some(flesh.centre_near(rest[i], reach));
        }
        for a in 0..n {
            if !anchor(a) {
                continue;
            }
            for &first in &kids[a] {
                let mut chain = vec![a];
                let mut cur = first;
                loop {
                    chain.push(cur);
                    if anchor(cur) {
                        break;
                    }
                    cur = kids[cur][0];
                }
                if chain.len() < 3 {
                    continue; // neighbours: nothing unplaced between them
                }
                let fixed: Vec<bool> = chain.iter().map(|&i| anchor(i)).collect();
                let mut pts: Vec<Vec3> = chain
                    .iter()
                    .map(|&i| target[i].unwrap_or(rest[i]))
                    .collect();
                fill_chain(flesh, &mut pts, &fixed);
                for (k, &i) in chain.iter().enumerate() {
                    if !fixed[k] {
                        target[i] = Some(pts[k]);
                    }
                }
            }
        }
        let depth_of = |mut i: usize| {
            let mut d = 0;
            while let Ok(k) = usize::try_from(parents[i]) {
                if k >= n || d > n {
                    break;
                }
                i = k;
                d += 1;
            }
            d
        };
        // A HELD joint is written too — PINNED to the snapshot. A reposition carries its whole
        // subtree, so an inferred parent would otherwise drag a joint the human (or the matcher)
        // had placed off the spot it was put on; holding it at its snapshot position is what
        // "placed joints untouched" actually costs.
        let mut order: Vec<usize> = (0..n).filter(|&i| target[i].is_some() || held[i]).collect();
        order.sort_by_key(|&i| depth_of(i));

        let symmetry = self.mirror_joints;
        self.mirror_joints = false;
        let mut inferred = 0;
        for i in order {
            let want = target[i].unwrap_or(rest[i]);
            let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) else {
                break;
            };
            let globals = p.globals.clone();
            let Some(now) = globals.get(i).map(|g| g.w_axis.truncate()) else {
                continue;
            };
            let delta = want - now;
            if delta.length() > 1e-4 {
                self.reposition_bone_with(i, &globals, delta, None);
            }
            if target[i].is_some() {
                self.resolve_drag_depth(i, Vec3::X);
                inferred += 1;
            }
        }
        self.mirror_joints = symmetry;
        tracing::info!("infer_markers: {inferred} joint(s) derived from the flesh");
        inferred
    }

    // ── THE REGION TAGGER (spec 0A81088E T2) ────────────────────────────────────────────────
    //
    // The model's `regions` are ONE list on the working `RawModel` — the same field the skin
    // pin, the flesh mask and `bake_rig`'s cloth block already read (T1). Everything the panel
    // does lands there through [`Self::write_regions`], and nothing else in this file writes it.

    /// The Regions list's rows: `(name, label)`, the label reading name · tag · anchor · chains ·
    /// stiffness — the whole row as the spec states it, pre-formatted like every other readout.
    pub(crate) fn region_rows(&self) -> Vec<(String, String)> {
        self.regions()
            .iter()
            .map(|r| {
                let label = format!(
                    "{}  {}  {}  ×{}  {:.3}",
                    r.name,
                    strings::resolve(region_tag(r.tag).2),
                    r.anchor_bone,
                    r.chain_count,
                    r.params.stiffness
                );
                (r.name.clone(), label)
            })
            .collect()
    }

    /// The working model's regions (empty without a parse).
    pub(crate) fn regions(&self) -> &[ClothRegion] {
        self.parsed()
            .map(|p| p.model.regions.as_slice())
            .unwrap_or(&[])
    }

    /// The picked row, clamped to the list — a removal never leaves a stale index behind.
    pub(crate) fn region_sel(&self) -> Option<usize> {
        let n = self.regions().len();
        self.region_sel.filter(|&i| i < n).or((n > 0).then_some(0))
    }

    /// The picked region's vertices — what the panels highlight.
    pub(crate) fn region_verts(&self) -> &[u32] {
        self.region_sel()
            .and_then(|i| self.regions().get(i))
            .map(|r| r.verts.as_slice())
            .unwrap_or(&[])
    }

    pub(crate) fn select_region(&mut self, name: &str) -> bool {
        let Some(i) = self.regions().iter().position(|r| r.name == name) else {
            return false;
        };
        let changed = self.region_sel != Some(i);
        self.region_sel = Some(i);
        changed
    }

    /// THE ONE SEAM every region write goes through: replace the list, bump the generation and
    /// drop what was measured from the OLD membership. `Flesh::build_body` masks tagged vertices
    /// out, so a region that just appeared changes what the body IS — the cached field has to go
    /// with it (the same call the three in-place geometry mutations make).
    fn write_regions(&mut self, regions: Vec<ClothRegion>) {
        if let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) {
            p.model.regions = regions;
            p.geometry_changed();
        }
        self.region_gen = self.region_gen.wrapping_add(1);
    }

    /// Edit one region's row. Every knob on the panel — the two steppers, the chain count and the
    /// stiffness slider — writes `model.regions[i]` through here and nowhere else.
    pub(crate) fn edit_region(&mut self, i: usize, edit: RegionEdit) -> bool {
        let bones: Vec<String> = self
            .parsed()
            .map(|p| p.model.bones.iter().map(|b| b.name.clone()).collect())
            .unwrap_or_default();
        let mut regions = self.regions().to_vec();
        let Some(r) = regions.get_mut(i) else {
            return false;
        };
        let before = (
            r.tag,
            r.anchor_bone.clone(),
            r.chain_count,
            r.params.stiffness,
        );
        match edit {
            RegionEdit::Tag(step) => {
                let at = REGION_TAGS.iter().position(|t| t.0 == r.tag).unwrap_or(0);
                r.tag = REGION_TAGS[step_index(at, step, REGION_TAGS.len())].0;
            }
            RegionEdit::Anchor(step) => {
                if !bones.is_empty() {
                    let at = bones.iter().position(|b| *b == r.anchor_bone).unwrap_or(0);
                    r.anchor_bone = bones[step_index(at, step, bones.len())].clone();
                }
            }
            // The chain count is TYPED like the decimate target — digits, and 0 is meaningful
            // (the region skins rigidly to its anchor), so an empty field is simply no edit.
            RegionEdit::Chains(text) => match text.trim().parse::<u32>() {
                Ok(n) => r.chain_count = n.min(MAX_CHAINS),
                Err(_) => return false,
            },
            RegionEdit::Stiffness(v) => r.params.stiffness = v.clamp(0.0, MAX_STIFFNESS),
        }
        if before
            == (
                r.tag,
                r.anchor_bone.clone(),
                r.chain_count,
                r.params.stiffness,
            )
        {
            return false;
        }
        self.write_regions(regions);
        true
    }

    /// Drop a region — its vertices go back to being the body.
    pub(crate) fn remove_region(&mut self, i: usize) -> bool {
        let mut regions = self.regions().to_vec();
        if i >= regions.len() {
            return false;
        }
        regions.remove(i);
        self.write_regions(regions);
        true
    }

    /// SPLIT GARMENT: run the hang-distance split against the FITTING BODY at the Prep page's
    /// [`Self::hang_cm`], with the piece carried onto the body by the mount fit's own placement.
    ///
    /// The body and the placement are `bake_garment`'s own — `load_rig_raw(fitting_base())` and
    /// `attach_world` off the socket's inverse bind — so what the panel shows is what the bake
    /// measures, with the hang as the one thing the human may move. Replaces the list, as the verb
    /// says; `Err` names why nothing could be measured, for the status line.
    pub(crate) fn split_regions(&mut self) -> Result<usize, String> {
        let model = self
            .parsed()
            .map(|p| p.model.clone())
            .ok_or("nothing is parsed")?;
        let body = flicker_content::load_rig_raw(&flicker_content::fitting_base())
            .map_err(|e| format!("reading the fitting body: {e}"))?;
        if body.vertices.is_empty() {
            return Err("the fitting body carries no mesh to measure against".to_string());
        }
        let fit = self.fit().copied().unwrap_or_default();
        let socket = fit.socket_name();
        let placement = body
            .bones
            .iter()
            .find(|b| b.name == socket)
            .map(|b| {
                flicker_content::attach_world(
                    &b.inverse_bind,
                    &Fit {
                        socket: socket.to_string(),
                        offset: fit.offset,
                        rot_deg: fit.rot,
                        scale: fit.scale,
                        uniform: fit.uniform,
                    }
                    .to_attach(),
                )
            })
            .ok_or_else(|| format!("the fitting body has no {socket} socket"))?;
        let regions = flicker_content::split_worn(&model, placement, &body, self.hang_cm);
        let found = regions.len();
        self.region_sel = (found > 0).then_some(0);
        self.write_regions(regions);
        Ok(found)
    }

    /// GROW FROM CLICK, second half: `seed` is where the armed press's ray met the mesh. The thin
    /// part around it becomes a new HAIR region appended to the list and selected. The arm is
    /// spent by the caller either way.
    pub(crate) fn grow_region(&mut self, seed: Vec3) -> bool {
        let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) else {
            return false;
        };
        let verts = p.grow_thin(seed);
        self.append_region(RegionTag::Hair, verts)
    }

    /// SELECT CULLED: whatever the ortho panels' cut planes hide becomes a new CLOTH region.
    pub(crate) fn select_culled(&mut self, planes: &[(Vec3, f32)]) -> bool {
        let Some(model) = self.parsed().map(|p| &p.model) else {
            return false;
        };
        let verts = flicker_content::verts_beyond(model, planes);
        self.append_region(RegionTag::Cloth, verts)
    }

    /// Append a hand-authored region and select it. A selection too small to be a panel is no
    /// region at all — the same floor the auto-split applies.
    fn append_region(&mut self, tag: RegionTag, verts: Vec<u32>) -> bool {
        if verts.is_empty() {
            return false;
        }
        let Some(model) = self.parsed().map(|p| p.model.clone()) else {
            return false;
        };
        let mut regions = model.regions.clone();
        let name = format!("{}_{:02}", region_tag(tag).1, regions.len() + 1);
        regions.push(flicker_content::region_from(&model, name, tag, verts));
        self.region_sel = Some(regions.len() - 1);
        self.write_regions(regions);
        true
    }

    /// Restore the focused joint's [`BoneOffset`] to `offset` (its pre-drag value) — the spring-back
    /// that ends a Perspective deform TEST, snapping the joint back to its rest position.
    pub(crate) fn restore_offset(&mut self, sel: usize, offset: BoneOffset) {
        self.edit_offset(sel, |o| *o = offset);
    }

    /// The chain a perspective drag of joint `sel` BENDS — its parent and grandparent, nearest
    /// first, stopping short of the trunk's anchors (`pelvis`, `root`): a hand bends the forearm
    /// and the upper arm, a knee swings the thigh, a toe folds the foot then the calf. `None`
    /// when nothing above the joint may bend (a hip, the first spine link): that joint's test
    /// falls back to the plain translate deform.
    pub(crate) fn reach_chain(&self, sel: usize) -> Option<Vec<usize>> {
        let p = self.parsed()?;
        let anchor = |i: usize| {
            p.model
                .bones
                .get(i)
                .is_some_and(|b| matches!(b.name.as_str(), "root" | "pelvis"))
        };
        let mut chain = Vec::with_capacity(2);
        let mut cur = sel;
        while chain.len() < 2 {
            let Some(parent) = p.parents.get(cur).and_then(|&q| usize::try_from(q).ok()) else {
                break;
            };
            if anchor(parent) {
                break;
            }
            chain.push(parent);
            cur = parent;
        }
        (!chain.is_empty()).then_some(chain)
    }

    /// The perspective deform TEST, IK-style (Aaron 2026-09-07: "drags should deform the mesh IK
    /// style"): from the REST pose, bend `sel`'s [`Self::reach_chain`] so the joint reaches
    /// `target` — the way an animation moves a joint, by turning the bones above it, never by
    /// stretching — then pose the view's frames and bump `pose_gen` so the skin follows live.
    /// Nothing is authored: [`Self::clear_pose`] springs the rest back. `false` (and nothing
    /// posed) when the joint has no chain to bend.
    pub(crate) fn pose_reach(&mut self, sel: usize, target: Vec3) -> bool {
        let Some(chain) = self.reach_chain(sel) else {
            return false;
        };
        let offsets = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) else {
            return false;
        };
        let (mut globals, parents) = rest_globals(&p.model, &offsets);
        // The ONE CCD (flicker-mechanics::ik, moved out of this bench 2026-09-08): a dozen
        // sweeps, or within half a millimetre.
        flicker_mechanics::ik::ccd(&mut globals, &parents, &chain, sel, target, 12, 0.05);
        p.globals = globals;
        self.pose_live = true;
        self.pose_gen = self.pose_gen.wrapping_add(1);
        true
    }

    /// Spring a [`Self::pose_reach`] test back to the rest pose (a no-op when none is live).
    pub(crate) fn clear_pose(&mut self) {
        if !self.pose_live {
            return;
        }
        self.pose_live = false;
        let offsets = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        if let Some(p) = self.source.as_mut().and_then(|s| s.parsed.as_mut()) {
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
            Some(AssetClass::Creature) => WF_CREATURE,
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
        let Some((has_bones, reopened, key)) = self.source.as_ref().and_then(|s| {
            s.parsed.as_ref().map(|p| {
                (
                    !p.model.bones.is_empty(),
                    s.reopened.is_some(),
                    (s.dir.clone(), s.candidate_sel),
                )
            })
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
            // THE MEASURED FACING (69F4B20D, swept 2026-09-21): the yaw that squares the body
            // itself onto the rig's forward, read off the flesh once, here, where the source is
            // cached — never off the bounding box, which is a bull's horns and a ewe's wool. The
            // human's Turn 90° rides on top of it and opens at none.
            self.facing_yaw = measure_facing(&Flesh::build_body(&source));
            self.facing_quarters = 0;
            tracing::info!("prep: the body lies {:.1}° off the rig", self.facing_yaw);
            self.prep = Some(PrepCache {
                key,
                decimated: source.clone(),
                source,
                source_tris,
                applied: source_tris,
            });
            self.rebuild_prepped_model();
        } else if has_bones && !reopened {
            // Re-entered Prep after conforming a raw mesh: revert the working mesh to the boneless
            // prepped geometry so the controls act again (the rig re-installs on the next Conform).
            // A RE-OPENED staged rig is never reverted: its skeleton is the work being kept.
            self.rebuild_prepped_model();
        }
    }

    /// APPLY the typed height (Aaron 2026-09-07: "there should just be a button to apply it"): a
    /// raw mesh rebuilds from its Prep cache at the new stature; an already-rigged body (a
    /// re-opened staged rig) resizes as ONE thing — mesh, joints and binds together — so the
    /// rig survives the resize. Returns whether anything was resized.
    pub(crate) fn apply_stature(&mut self) -> bool {
        if self.prep.is_some() {
            self.rebuild_prepped_model();
            return true;
        }
        let stature = self.stature_cm;
        let offsets = self
            .source
            .as_ref()
            .and_then(|s| s.rig.as_ref())
            .map(|r| r.offsets.clone())
            .unwrap_or_default();
        let Some(src) = self.source.as_mut() else {
            return false;
        };
        let Some(parsed) = src.parsed.as_mut() else {
            return false;
        };
        if parsed.model.bones.is_empty() || stature <= 0.0 {
            return false;
        }
        let rep = scale_mesh_to_stature(&mut parsed.model, stature);
        if rep.scale == 0.0 {
            return false;
        }
        parsed.geometry_changed(); // the mesh is a different size
        parsed.rebuild(&offsets);
        src.resolve_attach();
        self.pose_gen = self.pose_gen.wrapping_add(1);
        self.mesh_gen = self.mesh_gen.wrapping_add(1);
        true
    }

    /// TURN the raw mesh a quarter-turn about the vertical (the Prep Facing control): a body
    /// authored broadside is faced onto the rig. A no-op unless a raw mesh is in Prep (a
    /// committed rig has its facing baked in). Returns whether it turned.
    pub(crate) fn turn_facing(&mut self) -> bool {
        if self.prep.is_none() {
            return false;
        }
        self.facing_quarters = (self.facing_quarters + 1) % 4;
        self.rebuild_prepped_model();
        true
    }

    /// MIRROR FROM: keep and reflect one half of a lopsided source. A source-shape change like the
    /// facing turn, so it rebuilds the prepped mesh from the pristine cache — pressing Off puts the
    /// lopsided original back rather than leaving a half-mirrored body behind. Returns whether it
    /// changed.
    pub(crate) fn set_mirror_keep(&mut self, keep: Option<Side>) -> bool {
        if self.mirror_keep == keep {
            return false;
        }
        self.mirror_keep = keep;
        self.rebuild_prepped_model();
        true
    }

    /// The Facing readout: the current turn in degrees.
    pub(crate) fn facing_readout(&self) -> String {
        format!("{}\u{00b0}", self.facing_quarters as u32 * 90)
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

    /// The skeleton the Prep step has picked — what a raw mesh is composed onto at Conform.
    pub(crate) fn recipe(&self) -> SkeletonRecipe {
        self.recipe_edit
            .clone()
            .unwrap_or_else(|| self.picked_recipe())
    }

    /// The pick a fresh open starts on: a CREATURE starts on the shipped `Quadruped` recipe
    /// (its four-legged basis, 2026-09-08), everything else on Humanoid (index 0). A re-opened
    /// staged rig then adopts its own recipe over this.
    fn default_pick(&self) -> usize {
        if self.pending_class == Some(AssetClass::Creature) {
            self.presets
                .iter()
                .position(|p| p.name == "Quadruped")
                .unwrap_or(0)
        } else {
            0
        }
    }

    /// The picked preset's recipe as shipped — the starting point the module edits sit on.
    fn picked_recipe(&self) -> SkeletonRecipe {
        self.presets
            .get(self.preset)
            .map_or_else(SkeletonRecipe::humanoid, |p| p.recipe.clone())
    }

    /// EDIT A MODULE of the working recipe (the Prep controls): `edit` changes the trunk of the
    /// current recipe; an edit that lands back on the pick's own trunk clears the edit. A changed
    /// recipe rebuilds the prepped mesh so the rig re-installs on the next Conform, exactly as a
    /// changed pick does. Returns whether anything changed.
    pub(crate) fn edit_recipe(
        &mut self,
        edit: impl FnOnce(&mut flicker_skeletal::format::TrunkSpec),
    ) -> bool {
        let before = self.recipe();
        let mut next = before.clone();
        edit(&mut next.trunk);
        if next == before {
            return false;
        }
        let picked = self.picked_recipe();
        self.recipe_edit = (next.trunk != picked.trunk).then_some(SkeletonRecipe {
            trunk: next.trunk,
            preset: picked.preset,
        });
        self.rebuild_prepped_model();
        true
    }

    /// How many bones the picked skeleton composes to, root included — the count a baked rig
    /// on that pick carries (the canon's 67 for Humanoid).
    pub(crate) fn recipe_bones(&self) -> usize {
        flicker_content::baseline::compose(&self.recipe(), self.stature_cm)
            .map_or(REFERENCE_BONES, |b| b.len())
    }

    /// Does the picked skeleton compose to exactly the bones this model carries (+ the root the
    /// bake adds)? The test that tells a rig composed on the pick from a vendor rig conformed to
    /// the canon — what a commit writes as the rig's recipe hangs on it.
    pub(crate) fn recipe_fits(&self, model: &RawModel) -> bool {
        self.recipe_bones() == model.bones.len() + 1
    }

    /// WHAT `model` IS MADE OF: the picked skeleton when it composes to exactly these bones, else
    /// the humanoid recipe a vendor rig conformed onto the canon IS. The one reading a commit
    /// writes as the rig's recipe and the bake squares the stance by.
    pub(crate) fn rig_recipe(&self, model: &RawModel) -> SkeletonRecipe {
        if self.recipe_fits(model) {
            self.recipe()
        } else {
            SkeletonRecipe::humanoid()
        }
    }

    /// The picked preset's name, for the Prep readout — marked when the modules were edited
    /// on top of it.
    pub(crate) fn skeleton_name(&self) -> String {
        let name = self
            .presets
            .get(self.preset)
            .map_or_else(|| "Humanoid".to_string(), |p| p.name.clone());
        if self.recipe_edit.is_some() {
            format!("{name} (custom)")
        } else {
            name
        }
    }

    /// What the pick composes to — bone count and the parts that differ from the canon.
    pub(crate) fn skeleton_summary(&self) -> String {
        describe(&self.recipe())
    }

    /// Step the Prep step's skeleton pick by `delta`; clamps at the ends (a linear rail never
    /// wraps). A changed pick rebuilds the prepped mesh so the rig re-installs on the next
    /// Conform. Returns whether the pick moved.
    pub(crate) fn step_preset(&mut self, delta: i32) -> bool {
        let n = self.presets.len() as i32;
        if n == 0 {
            return false;
        }
        let want = (self.preset as i32 + delta).clamp(0, n - 1) as usize;
        if want == self.preset {
            return false;
        }
        self.preset = want;
        self.recipe_edit = None; // a new starting point
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
        let facing = self.facing_quarters;
        let yaw = self.facing_yaw;
        let mirror = self.mirror_keep;
        let Some(cache) = self.prep.as_ref() else {
            return;
        };
        let mut model = cache.decimated.clone();
        scale_mesh_to_stature(&mut model, stature);
        // Turn the raw mesh to face the rig (−Y) before it is rigged and baked, through the ONE
        // facing verb `flicker_content::face_to_rig` the headless `import_folder` also calls
        // (69F4B20D). Exact 90° steps, so four turns return the geometry bit-for-bit.
        face_to_rig(&mut model, yaw, facing);
        // MIRROR THE MESH (697DEC55, the ratified order 42AB9BA8) — AFTER the turn, because the
        // mirror cuts on world X = 0 and only a mesh already faced onto the rig has its median
        // plane there. A source-shape fix, so it lands here with the other two and the skeleton
        // is fitted to the symmetric body, exactly as the headless `import_folder` does it.
        if let Some(keep) = mirror {
            let r = mirror_mesh(&mut model, keep);
            tracing::info!(
                "prep: mirrored from the {keep:?} half — {} dropped, {} added, {} region(s) kept",
                r.dropped,
                r.added,
                r.kept_regions
            );
        }
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
        let src = self.source.as_ref()?;
        let rig = src.rig.as_ref()?;
        let r = |t: &str| strings::resolve(t).into_owned();
        // A RE-OPENED staged rig: its joints are as they were committed; nothing was fitted here,
        // and the line says so rather than letting a human read the old work as a fresh fit
        // (incident 118CEA35).
        if let Some(origin) = src.reopened {
            return Some(r("$ap_rig_reopened").replace("{origin}", origin));
        }
        // A FITTED rig: what the fit matched, in the pick's own name — never the vendor-rig
        // conform's "0 inferred · 0 limbs aligned", which announced a finished fit as nothing done.
        if let Some(m) = rig.shape.as_ref() {
            let total = m.matched.len() + m.unmatched.len();
            return Some(
                r("$ap_rig_fitted")
                    .replace("{pattern}", self.recipe().pattern().name())
                    .replace("{n}", &m.matched.len().to_string())
                    .replace("{total}", &total.to_string())
                    .replace("{bones}", &self.bone_count().unwrap_or(0).to_string()),
            );
        }
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

    /// The Review page's one line about the last Commit: where it wrote and what (relative
    /// to the content root, with the baked bone count), why it wrote nothing, or empty
    /// before any attempt.
    pub(crate) fn commit_note(&self) -> String {
        let Some(src) = self.source.as_ref() else {
            return String::new();
        };
        let r = |t: &str| strings::resolve(t).into_owned();
        if let Some(e) = src.commit_error.as_ref() {
            return format!("{} {e}", r("$ap_export_failed"));
        }
        let Some(out) = src.committed.as_ref() else {
            return String::new();
        };
        let content = flicker_content::roots().root().to_path_buf();
        let shown = out
            .parent()
            .and_then(|d| d.strip_prefix(&content).ok())
            .map_or_else(|| out.display().to_string(), |p| p.display().to_string());
        let bones = src.parsed.as_ref().map_or(0, |p| p.bones() + 1);
        format!(
            "{} → {shown} · {bones} {}",
            r("$ap_exported"),
            r("$ap_bones")
        )
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
        Some(AssetClass::Creature) => "$ap_creature",
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
/// The recipe a staged/promoted rig declares (`skeleton_recipe`, modules and preset name), read off the rig file
/// beside its raw load — `None` for a rig without a recipe (a vendor rig conformed to canon).
fn staged_recipe(path: &Path) -> Option<SkeletonRecipe> {
    let text = flicker_content::package::read_text(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    serde_json::from_value(v.get("skeleton_recipe")?.clone()).ok()
}

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
            Vec3::from_array(b.scale) * Vec3::from_array(o.scale),
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
        if o.is_identity() {
            continue;
        }
        for k in 0..3 {
            b.translation[k] += o.t[k];
            b.scale[k] *= o.scale[k];
        }
        let q =
            glam::Quat::from_array(b.rotation) * glam::Quat::from_rotation_x(o.roll.to_radians());
        b.rotation = q.to_array();
    }
}

/// The rotation/scale part of a bone's PARENT frame in world space (identity at the root) — the
/// basis `RawBone::translation` and [`BoneOffset::t`] are expressed in.
/// THE LIMB a joint roots, as a rest world position: its LONGEST child bone, twist bones excluded
/// (a twist rides its parent's own axis and points nowhere the flesh can be read along). The
/// direction [`Flesh::limb_depth`] walks out of the body along — thigh for a hip, upperarm for a
/// clavicle. `None` for a leaf, which then keeps whatever depth the hand gave it.
fn limb_child(p: &Parsed, joint: usize) -> Option<Vec3> {
    let at = p.globals.get(joint)?.w_axis.truncate();
    let joint = i32::try_from(joint).ok()?;
    p.model
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| b.parent == joint && !b.name.to_ascii_lowercase().contains("twist"))
        .filter_map(|(i, _)| p.globals.get(i).map(|g| g.w_axis.truncate()))
        .max_by(|a, b| a.distance_squared(at).total_cmp(&b.distance_squared(at)))
}

pub(crate) fn parent_basis(globals: &[Mat4], model: &RawModel, sel: usize) -> glam::Mat3 {
    model
        .bones
        .get(sel)
        .and_then(|b| usize::try_from(b.parent).ok())
        .and_then(|pi| globals.get(pi))
        .map(|g| glam::Mat3::from_mat4(*g))
        .unwrap_or(glam::Mat3::IDENTITY)
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
    parent_basis(globals, model, sel).inverse() * world_delta
}

/// The left/right suffix table — the ONE spelling of sidedness the mirror and the isolation
/// filters share.
const SIDE_SUFFIXES: [(&str, &str, Side); 3] = [
    ("_l", "_r", Side::Left),
    ("_L", "_R", Side::Left),
    (".L", ".R", Side::Left),
];

/// Which side of the body a sided bone is on — `flicker_content::Side`, the ONE spelling of
/// sidedness (1B64FF03). The bench had its own copy of this two-variant enum until the Prep
/// mirror control needed the mirror verb's; there is one now, re-exported here so every
/// `crate::services::Side` path in the bench still reads off the module that uses it most.
pub(crate) use flicker_content::Side;

/// The other side. (A free fn, not an inherent `impl`: the type is another crate's.)
pub(crate) fn opposite(side: Side) -> Side {
    match side {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    }
}

/// The side a bone's name puts it on (`thigh_l` → Left, `Weapon_R` → Right), or `None` for a
/// centre bone (`spine_01`).
pub(crate) fn side_of(name: &str) -> Option<Side> {
    SIDE_SUFFIXES.iter().find_map(|(l, r, side)| {
        if name.ends_with(l) {
            Some(*side)
        } else if name.ends_with(r) {
            Some(opposite(*side))
        } else {
            None
        }
    })
}

/// The symmetric bone NAME for a left/right bone (`thigh_l`↔`thigh_r`), or `None` for a centre bone.
pub(crate) fn mirror_name(name: &str) -> Option<String> {
    SIDE_SUFFIXES.iter().find_map(|(l, r, _)| {
        if let Some(stem) = name.strip_suffix(l) {
            Some(format!("{stem}{r}"))
        } else {
            name.strip_suffix(r).map(|stem| format!("{stem}{l}"))
        }
    })
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

/// Where committed CREATURES land — unrigged non-humanoid bodies, their own staging tier
/// (`staging/creatures/`) until the creature rig families give them a canon.
pub(crate) fn creatures_dir() -> PathBuf {
    flicker_content::roots().staging().join("creatures")
}

/// The asset's bounding CENTRE and half-extent — the framing the viewport needs.
///
/// Measured from the MESH when there is one (a prop carries no skeleton at all) and from the bone
/// frames otherwise. Everything is then drawn offset by `-centre`, because the quad cameras all
/// target the ORIGIN and in Z-up ground reckoning the origin is the asset's FEET (a character
/// stands 0..170 in +Z) — targeting it framed the feet with the body sticking out of shot.
pub(crate) fn model_bounds(model: &RawModel, globals: &[Mat4]) -> (Vec3, f32, f32, Vec3) {
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
        return (Vec3::ZERO, 1.0, 0.0, Vec3::ONE);
    }
    let centre = (lo + hi) * 0.5;
    // The floor is reported ALREADY RECENTRED (`lo.z - centre.z`, so it is negative), because
    // every caller draws through the same `-centre` offset — handing back the raw `lo.z` would
    // make each one re-derive the shift and eventually one of them would forget.
    (
        centre,
        ((hi - lo).max_element() * 0.5).max(1.0),
        lo.z - centre.z,
        ((hi - lo) * 0.5).max(Vec3::splat(0.01)),
    )
}

// The `#[cfg(test)]` dialog seam's answer, armed by `stub_pick` and consumed once by
// `Document::pick_folder`. Thread-local, so parallel tests never see each other's arm.
#[cfg(test)]
thread_local! {
    static PICK_STUB: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Arm the headless dialog seam: the next [`Document::pick_folder`] returns `dir`.
#[cfg(test)]
pub(crate) fn stub_pick(dir: PathBuf) {
    PICK_STUB.with(|c| *c.borrow_mut() = Some(dir));
}
