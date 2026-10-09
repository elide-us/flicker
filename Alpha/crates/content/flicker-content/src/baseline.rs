//! THE AUTHORED BASELINE SKELETON — the `Humanoid` reference, historically `GolemBaseSkeleton` (Aaron's ruling, 2026-08-04).
//!
//! The reference skeleton is a GENERATED ARTIFACT, authored from first principles as
//! DATA — never derived from any vendor mesh. This closes the Katanami lineage for
//! good: the canon (67 names + parent topology + conventions) was always ours; the
//! BIND (where joints rest) is now ours too — a perfect A-pose at a stated stature,
//! exactly symmetric BY CONSTRUCTION (the left side is authored, the right side is
//! mirrored), soles flat on the ground, shoulders level, spine plumb.
//!
//! Segment heights and lengths follow the Drillis–Contini anthropometric fractions
//! of stature (the standard proportional body model), scaled by [`STATURE`]. Every
//! number is a fraction times one knob, so retuning the baseline is editing data and
//! regenerating — `cargo run -p flicker-content --example bake_baseline`.
//!
//! Conventions (identical to every other `flicker.rig` this crate bakes): Z-up, cm,
//! the character FACES -Y, LEFT is +X, root at the ground origin. Bone locals are
//! PURE TRANSLATIONS (identity rest rotations): a bone's direction is implied by its
//! children's positions, clips supply rotations wholesale, and the retarget rebase
//! reads exactly these rest translations.
//!
//! Since 2026-09-07 the baseline is one RECIPE of the modular skeleton system: [`compose`]
//! turns any `SkeletonRecipe` (a tree of trunk / head / arm / leg / tail modules, unbounded)
//! into its authored rest, and the humanoid recipe composes to exactly this skeleton — the
//! gate below pins it to the packaged reference bone for bone. [`TOPOLOGY`] keeps the
//! humanoid's names + parents in the packaged reference's historical order; bone ORDER
//! within a rig is not canonical (names + topology are), and a freshly composed rig lists
//! its bones depth-first by module.

use anyhow::Result;
use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use flicker_skeletal::format::Pattern;
use flicker_skeletal::format::{
    ArmKind, BoneRaw, LegKind, Orientation, RigFile, Skeleton, SkeletonRecipe, Socket, Source,
    TailKind, TrunkSpec,
};

/// The ruled base height (Aaron, 2026-08-04): 170 cm stature. Everything scales off it.
pub const STATURE: f32 = 170.0;

/// The canonical bone-count — derived from the one topology table below, and the
/// single number every consumer (the Clayworks requirements, the loader tests)
/// reads. 67 since 2026-08-04: `neck_02` joined the chain (Aaron: "at least one
/// more so it can be SIX SEVEN").
pub const CANON_BONES: usize = TOPOLOGY.len();

/// The HUMANOID canon topology — names + parents, in the packaged reference's
/// historical order (parents precede children). THE canon statement for the humanoid
/// recipe; the golem and every conformed humanoid carry exactly this arrangement, and
/// `compose(&SkeletonRecipe::humanoid())` reproduces it (the gate below).
pub const TOPOLOGY: [(&str, &str); 67] = [
    ("root", "-"),
    ("pelvis", "root"),
    ("thigh_l", "pelvis"),
    ("thigh_r", "pelvis"),
    ("spine_01", "pelvis"),
    ("calf_l", "thigh_l"),
    ("calf_r", "thigh_r"),
    ("spine_02", "spine_01"),
    ("foot_l", "calf_l"),
    ("foot_r", "calf_r"),
    ("spine_03", "spine_02"),
    ("ball_l", "foot_l"),
    ("ball_r", "foot_r"),
    ("clavicle_l", "spine_03"),
    ("clavicle_r", "spine_03"),
    ("neck_01", "spine_03"),
    ("upperarm_l", "clavicle_l"),
    ("upperarm_r", "clavicle_r"),
    ("neck_02", "neck_01"),
    ("head", "neck_02"),
    ("lowerarm_l", "upperarm_l"),
    ("lowerarm_r", "upperarm_r"),
    ("hand_l", "lowerarm_l"),
    ("hand_r", "lowerarm_r"),
    ("Weapon_L", "hand_l"),
    ("index_01_l", "hand_l"),
    ("index_02_l", "index_01_l"),
    ("index_03_l", "index_02_l"),
    ("middle_01_l", "hand_l"),
    ("middle_02_l", "middle_01_l"),
    ("middle_03_l", "middle_02_l"),
    ("pinky_01_l", "hand_l"),
    ("pinky_02_l", "pinky_01_l"),
    ("pinky_03_l", "pinky_02_l"),
    ("ring_01_l", "hand_l"),
    ("ring_02_l", "ring_01_l"),
    ("ring_03_l", "ring_02_l"),
    ("thumb_01_l", "hand_l"),
    ("thumb_02_l", "thumb_01_l"),
    ("thumb_03_l", "thumb_02_l"),
    ("lowerarm_twist_01_l", "lowerarm_l"),
    ("upperarm_twist_01_l", "upperarm_l"),
    ("Weapon_R", "hand_r"),
    ("index_01_r", "hand_r"),
    ("index_02_r", "index_01_r"),
    ("index_03_r", "index_02_r"),
    ("middle_01_r", "hand_r"),
    ("middle_02_r", "middle_01_r"),
    ("middle_03_r", "middle_02_r"),
    ("pinky_01_r", "hand_r"),
    ("pinky_02_r", "pinky_01_r"),
    ("pinky_03_r", "pinky_02_r"),
    ("ring_01_r", "hand_r"),
    ("ring_02_r", "ring_01_r"),
    ("ring_03_r", "ring_02_r"),
    ("thumb_01_r", "hand_r"),
    ("thumb_02_r", "thumb_01_r"),
    ("thumb_03_r", "thumb_02_r"),
    ("lowerarm_twist_01_r", "lowerarm_r"),
    ("upperarm_twist_01_r", "upperarm_r"),
    ("calf_twist_01_l", "calf_l"),
    ("thigh_twist_01_l", "thigh_l"),
    ("calf_twist_01_r", "calf_r"),
    ("thigh_twist_01_r", "thigh_r"),
    ("jaw", "head"),
    ("eye_l", "head"),
    ("eye_r", "head"),
];

/// The A-pose arm's droop below horizontal, in degrees. 42° is the classic relaxed
/// A — high enough that armpits and sleeves skin cleanly, low enough that shoulder
/// deformation stays neutral.
const A_POSE_DEG: f32 = 42.0;

/// One authored rest bone: its name, its parent's name (`-` for the root) and its world
/// rest position. What [`compose`] emits; every consumer turns it into locals/binds.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthoredBone {
    pub name: String,
    pub parent: String,
    pub position: Vec3,
}

/// COMPOSE a recipe into its authored rest skeleton at `stature` cm — THE one place bones come
/// from (the modular skeleton system, Aaron 2026-09-07). Walks the recipe tree depth-first:
/// the trunk chain, its head, its shoulder pairs (left then right), its hip pairs, its tails,
/// then each mounted child trunk the same way. Parents always precede children. Every module
/// authors its rest RELATIVE TO ITS SOCKET as fractions of the trunk's knob (`stature ×
/// trunk.scale`), so chaining composes; the first instance of every kind keeps the canon names
/// and later instances take the numbered per-kind prefix (see `SkeletonRecipe`).
///
/// `compose(&SkeletonRecipe::humanoid(), STATURE)` IS the `Humanoid` reference (GolemBaseSkeleton) — the gate below pins
/// it to the packaged reference bone for bone. A variant that is not authored yet fails LOUD.
pub fn compose(recipe: &SkeletonRecipe, stature: f32) -> Result<Vec<AuthoredBone>> {
    let mut out = Composer {
        bones: vec![AuthoredBone {
            name: "root".to_string(),
            parent: "-".to_string(),
            position: Vec3::ZERO,
        }],
        modules: vec![String::new()],
        module: String::new(),
        counts: HashMap::new(),
    };
    let pelvis = Vec3::new(0.0, 0.0, 0.560 * stature * recipe.trunk.scale);
    out.trunk(&recipe.trunk, stature, "root", pelvis)?;
    let mut seen = std::collections::HashSet::new();
    for b in &out.bones {
        anyhow::ensure!(
            seen.insert(b.name.as_str()),
            "recipe composes `{}` twice",
            b.name
        );
    }
    Ok(out.bones)
}

/// [`compose`], with the MODULE each bone came from beside it (parallel to the bones, `""` for the
/// root). The rail walks this: a bone's module is the only structural way to tell a trunk's own
/// spine from the limb roots hanging off it.
pub fn compose_with_modules(
    recipe: &SkeletonRecipe,
    stature: f32,
) -> Result<(Vec<AuthoredBone>, Vec<String>)> {
    let mut out = Composer {
        bones: vec![AuthoredBone {
            name: "root".to_string(),
            parent: "-".to_string(),
            position: Vec3::ZERO,
        }],
        modules: vec![String::new()],
        module: String::new(),
        counts: HashMap::new(),
    };
    let pelvis = Vec3::new(0.0, 0.0, 0.560 * stature * recipe.trunk.scale);
    out.trunk(&recipe.trunk, stature, "root", pelvis)?;
    Ok((out.bones, out.modules))
}

/// The depth-first walk's state: the bones so far and the per-kind instance counters.
struct Composer {
    bones: Vec<AuthoredBone>,
    /// The module each bone came from, parallel to `bones` — [`module_id`]'s spelling. Recorded
    /// as the walk goes because the MARKERS RAIL walks the composed skeleton itself now, and a
    /// bone's module is what says where one module's chain ends and the next begins.
    modules: Vec<String>,
    module: String,
    counts: HashMap<&'static str, usize>,
}

/// Emit `_l` for the left side (x > 0) and `_r` for the mirrored right (x negated).
const SIDES: [(&str, f32); 2] = [("l", 1.0), ("r", -1.0)];

/// A quadruped's croup (its `pelvis`) rides this far above the hip socket the leg modules are
/// authored for (0.560h), putting the back at 0.88h of the withers height.
const QUAD_CROUP_LIFT: f32 = 0.32;
/// The withers height as a fraction of the quadruped's stature knob — the knob IS the withers.
/// Public because it is also the rule the other way round: [`crate::conform::align_trunk`] reads
/// a mesh's withers and DIVIDES by this to get the stature to compose that body at.
pub const QUAD_WITHERS: f32 = 0.92;
/// A biped's `pelvis` above its CROTCH — the height its two legs part — as a fraction of the
/// stature. Drillis–Contini, the fractions the whole canon is built on, put the crotch at 0.485
/// of stature and the hip centre at the 0.560 the composed `pelvis` sits at, so a body whose legs
/// part where the canon's do keeps the canon's pelvis exactly ([`crate::conform::align_trunk`]).
pub const BIPED_PELVIS_OVER_CROTCH: f32 = 0.560 - 0.485;

/// The KNEE where a thigh of `thigh_len` from `hip` and a shin of `calf_len` from `ankle` meet —
/// two circles in the leg's YZ plane, taken on the FORWARD side (the Z-shaped bent-knee stance).
/// A pair that cannot reach straightens along the hip→ankle line.
fn knee_between(hip: Vec3, ankle: Vec3, thigh_len: f32, calf_len: f32) -> Vec3 {
    let d = ankle.distance(hip).max(1e-3);
    let u = (ankle - hip) / d;
    let along = ((thigh_len * thigh_len - calf_len * calf_len + d * d) / (2.0 * d)).clamp(0.0, d);
    let out = (thigh_len * thigh_len - along * along).max(0.0).sqrt();
    // Perpendicular to hip→ankle in the leg's YZ plane, pointing forward (−Y).
    let mut forward = Vec3::new(0.0, -u.z, u.y);
    if forward.y > 0.0 {
        forward = -forward;
    }
    hip + u * along + forward * out
}

/// THE NAMING RULE for repeated modules (spec C658F114, Aaron's amendment): the FIRST instance of
/// every kind keeps the canon names, every later one takes a numbered per-kind prefix. One place,
/// because two walks read it — `compose`, which emits the bones, and [`markers_for`], which names
/// the ones the rail prompts for. A second copy here would be a rail that prompts for joints the
/// skeleton does not have.
pub(crate) fn next_prefix(counts: &mut HashMap<&'static str, usize>, kind: &'static str) -> String {
    let k = counts.entry(kind).or_insert(0);
    *k += 1;
    if *k == 1 {
        String::new()
    } else {
        format!("{kind}{k}_")
    }
}

impl Composer {
    /// The name prefix for the next instance of `kind`: none for the first, `kind<k>_` after.
    fn prefix(&mut self, kind: &'static str) -> String {
        let p = next_prefix(&mut self.counts, kind);
        // THE ONE place a module begins: every module function claims its prefix here first, so
        // recording the module here tags every bone it goes on to push.
        self.module = module_id(kind, &p);
        p
    }

    fn push(&mut self, name: String, parent: &str, position: Vec3) {
        self.bones.push(AuthoredBone {
            name,
            parent: parent.to_string(),
            position,
        });
        self.modules.push(self.module.clone());
    }

    /// A trunk at `pelvis` (world), hung from `socket`, with everything it carries.
    fn trunk(&mut self, spec: &TrunkSpec, stature: f32, socket: &str, pelvis: Vec3) -> Result<()> {
        let h = stature * spec.scale;
        let p = self.prefix("trunk");
        let quadruped = spec.orientation == Orientation::Quadruped;
        // ── the chain: pelvis → spine → neck ──
        // A BIPED's is plumb above the pelvis (Drillis–Contini). A QUADRUPED's lies ALONG THE
        // BACK (P3, spec C658F114; `h` = the withers height): the pelvis is the croup at the
        // rear (0.88h), the spine runs forward (−Y) dipping a little and rising to the withers
        // `length·h` ahead (0.92h), the neck climbs from there and the head is carried forward
        // above it. The hip pairs hang from a socket 0.32h BELOW the croup — the height every
        // leg module is authored for — so the same legs reach the same ground.
        let pelvis = if quadruped {
            pelvis + Vec3::new(0.0, 0.0, QUAD_CROUP_LIFT * h)
        } else {
            pelvis
        };
        let l = spec.length.max(0.2) * h;
        let chain: [(&str, Vec3); 6] = if quadruped {
            [
                ("pelvis", Vec3::ZERO),
                ("spine_01", Vec3::new(0.0, -l / 3.0, -0.020 * h)),
                ("spine_02", Vec3::new(0.0, -2.0 * l / 3.0, -0.020 * h)),
                ("spine_03", Vec3::new(0.0, -l, 0.040 * h)),
                ("neck_01", Vec3::new(0.0, -l - 0.12 * h, 0.140 * h)),
                ("neck_02", Vec3::new(0.0, -l - 0.22 * h, 0.240 * h)),
            ]
        } else {
            [
                ("pelvis", Vec3::ZERO),
                ("spine_01", Vec3::new(0.0, 0.0, 0.055 * h)),
                ("spine_02", Vec3::new(0.0, 0.0, 0.112 * h)),
                ("spine_03", Vec3::new(0.0, 0.0, 0.170 * h)),
                ("neck_01", Vec3::new(0.0, 0.0, 0.285 * h)),
                ("neck_02", Vec3::new(0.0, 0.0, 0.298 * h)),
            ]
        };
        // A NECK mount REPLACES this trunk's neck (Aaron's ruling 2026-09-12): the chain stops at
        // `spine_03` and the mounted trunk's own pelvis — the SUB-PELVIS the upper torso bends on —
        // stands where `neck_01` would have. Only a HEADLESS trunk has a neck to give up.
        let necks = spec
            .mounts
            .iter()
            .filter(|m| m.socket == Socket::Neck)
            .count();
        anyhow::ensure!(
            necks <= 1,
            "a trunk has ONE neck: {necks} `Neck` mounts on the same trunk"
        );
        anyhow::ensure!(
            necks == 0 || !spec.head,
            "a `Neck` mount replaces the neck of a HEADLESS trunk, and this trunk has a head — \
             mount the child on `Withers` or `Spine(n)`, or author the trunk `head: false`"
        );
        let mut parent = socket.to_string();
        for (base, offset) in chain
            .into_iter()
            .take(if necks == 1 { 4 } else { chain.len() })
        {
            let name = format!("{p}{base}");
            self.push(name.clone(), &parent, pelvis + offset);
            parent = name;
        }
        let at = |base: &str| format!("{p}{base}");
        let spine_03 = pelvis + chain[3].1;
        let neck_02 = pelvis + chain[5].1;
        if spec.head {
            let (head_name, head) = self.head(h, &at("neck_02"), neck_02, spec.orientation);
            if spec.proboscis > 0 {
                self.proboscis(spec.proboscis, h, &head_name, head, spec.orientation)?;
            }
        }
        for (i, kind) in spec.arms.iter().enumerate() {
            // Later pairs step back (+Y) along the trunk — a biped's also up (wings mount
            // behind the arms); a quadruped's along its level back.
            let step = if quadruped {
                Vec3::new(0.0, 0.15 * h, 0.0)
            } else {
                Vec3::new(0.0, 0.06 * h, 0.03 * h)
            };
            let socket = spine_03 + step * i as f32;
            self.arm_pair(*kind, h, &at("spine_03"), socket)?;
        }
        for (i, kind) in spec.legs.iter().enumerate() {
            // Later pairs step along the trunk: a biped's back (+Y), a quadruped's forward. A
            // quadruped's hip socket hangs below the croup at the leg modules' hip height.
            let step = if quadruped {
                Vec3::new(0.0, -0.15 * h, 0.0)
            } else {
                Vec3::new(0.0, 0.12 * h, 0.0)
            };
            let drop = if quadruped {
                Vec3::new(0.0, 0.0, -QUAD_CROUP_LIFT * h)
            } else {
                Vec3::ZERO
            };
            let socket = pelvis + drop + step * i as f32;
            self.leg_pair(*kind, h, &at("pelvis"), socket, spec.orientation)?;
        }
        for kind in &spec.tails {
            self.tail(*kind, h, &at("pelvis"), pelvis)?;
        }
        for mount in &spec.mounts {
            let (bone, offset) = match (mount.socket, quadruped) {
                (Socket::Withers, false) => ("spine_03", Vec3::new(0.0, -0.03 * h, 0.10 * h)),
                (Socket::Withers, true) => ("spine_03", Vec3::new(0.0, 0.0, 0.06 * h)),
                (Socket::Rear, false) => ("pelvis", Vec3::new(0.0, 0.12 * h, 0.0)),
                (Socket::Rear, true) => ("pelvis", Vec3::new(0.0, 0.06 * h, 0.0)),
                (Socket::Spine(n), _) => (
                    match n {
                        1 => "spine_01",
                        2 => "spine_02",
                        _ => "spine_03",
                    },
                    Vec3::new(0.0, 0.0, 0.05 * h),
                ),
                // The NECK ROOT: the child's pelvis takes the place of the neck that was not
                // composed — `neck_01`'s offset, read off `spine_03`, whatever the orientation.
                (Socket::Neck, _) => ("spine_03", chain[4].1 - chain[3].1),
            };
            let bone_pos = self
                .bones
                .iter()
                .find(|b| b.name == at(bone))
                .map(|b| b.position)
                .expect("the trunk chain was just emitted");
            self.trunk(&mount.trunk, stature, &at(bone), bone_pos + offset)?;
        }
        Ok(())
    }

    /// The head on `neck_02`: head, jaw, and the two eyes (the face rides forward, −Y). A
    /// biped's head sits up on the neck; a quadruped's is carried forward off it, the muzzle
    /// ahead and below, the eyes ahead and above.
    fn head(
        &mut self,
        h: f32,
        neck_02_name: &str,
        neck_02: Vec3,
        orientation: Orientation,
    ) -> (String, Vec3) {
        let p = self.prefix("head");
        let quadruped = orientation == Orientation::Quadruped;
        let head = neck_02
            + if quadruped {
                Vec3::new(0.0, -0.08 * h, -0.02 * h)
            } else {
                Vec3::new(0.0, 0.0, 0.012 * h)
            };
        let head_name = format!("{p}head");
        self.push(head_name.clone(), neck_02_name, head);
        let jaw = if quadruped {
            Vec3::new(0.0, -0.08 * h, -0.08 * h)
        } else {
            Vec3::new(0.0, -0.026 * h, 0.015 * h)
        };
        self.push(format!("{p}jaw"), &head_name, head + jaw);
        for (side, sign) in SIDES {
            let eye = if quadruped {
                Vec3::new(0.035 * h * sign, -0.07 * h, 0.01 * h)
            } else {
                Vec3::new(0.019 * h * sign, -0.047 * h, 0.066 * h)
            };
            self.push(format!("{p}eye_{side}"), &head_name, head + eye);
        }
        (head_name, head)
    }

    /// A PROBOSCIS off `head_name` at `head` (world): `bones` articulated bones from the face
    /// down — rooted ahead of and below the head joint, where a muzzle ends, and hanging from
    /// there, a quadruped's straight down and a biped's forward and down. Instance k ≥ 2 numbers
    /// its bones `proboscis<k>_NN`. The composed rest is a plausible default only: the fit lays
    /// the chain along the appendage's own path (`conform`).
    fn proboscis(
        &mut self,
        bones: u8,
        h: f32,
        head_name: &str,
        head: Vec3,
        orientation: Orientation,
    ) -> Result<()> {
        let p = self.prefix("proboscis");
        let stem = if p.is_empty() {
            "proboscis".to_string()
        } else {
            p.trim_end_matches('_').to_string()
        };
        anyhow::ensure!(
            (1..=32).contains(&bones),
            "a proboscis of {bones} bones is outside the authored range (1–32)"
        );
        let (root, step) = if orientation == Orientation::Quadruped {
            (
                head + Vec3::new(0.0, -0.10 * h, -0.08 * h),
                Vec3::new(0.0, -0.01 * h, -0.05 * h),
            )
        } else {
            (
                head + Vec3::new(0.0, -0.05 * h, 0.0),
                Vec3::new(0.0, -0.02 * h, -0.04 * h),
            )
        };
        let mut parent = head_name.to_string();
        let mut at = root;
        for i in 1..=bones {
            let name = format!("{stem}_{i:02}");
            self.push(name.clone(), &parent, at);
            parent = name;
            at += step;
        }
        Ok(())
    }

    /// One shoulder pair of `kind` hung from `spine_03_name` at `socket` (world, midline).
    fn arm_pair(&mut self, kind: ArmKind, h: f32, spine_03_name: &str, socket: Vec3) -> Result<()> {
        if kind == ArmKind::Hanging {
            return self.hanging_arm_pair(h, spine_03_name, socket);
        }
        if kind == ArmKind::Ungulate {
            return self.ungulate_foreleg_pair(h, spine_03_name, socket);
        }
        if kind == ArmKind::Bird {
            return self.bird_wing_pair(h, spine_03_name, socket);
        }
        if kind == ArmKind::Bat {
            return self.bat_wing_pair(h, spine_03_name, socket);
        }
        anyhow::ensure!(kind == ArmKind::Humanoid, "{kind:?} arms are not authored");
        let p = self.prefix("arm");
        let (s, c) = A_POSE_DEG.to_radians().sin_cos();
        for (side, sign) in SIDES {
            let n = |base: &str| format!("{p}{base}_{side}");
            let dir = Vec3::new(c * sign, 0.0, -s); // down-and-out in the XZ plane
            let clavicle = socket + Vec3::new(0.015 * h * sign, 0.0, 0.088 * h);
            let shoulder = socket + Vec3::new(0.129 * h * sign, 0.0, 0.088 * h);
            let elbow = shoulder + dir * (0.186 * h);
            let wrist = elbow + dir * (0.146 * h);
            self.push(n("clavicle"), spine_03_name, clavicle);
            self.push(n("upperarm"), &n("clavicle"), shoulder);
            self.push(
                n("upperarm_twist_01"),
                &n("upperarm"),
                (shoulder + elbow) * 0.5,
            );
            self.push(n("lowerarm"), &n("upperarm"), elbow);
            self.push(
                n("lowerarm_twist_01"),
                &n("lowerarm"),
                (elbow + wrist) * 0.5,
            );
            self.push(n("hand"), &n("lowerarm"), wrist);
            // The grip point, mid-palm — the Weapon socket keeps its capitalised canon name.
            self.push(
                format!("{p}Weapon_{}", side.to_uppercase()),
                &n("hand"),
                wrist + dir * (0.054 * h),
            );
            // Fingers: roots fan across Y at the knuckle line, segments continue the arm
            // line (3 phalanges, tapering). The thumb sits forward and shorter.
            let knuckle = wrist + dir * (0.049 * h);
            let mut finger = |name: &str, root: Vec3, len: f32| {
                let joints = [root, root + dir * len, root + dir * (len * 1.8)];
                let mut parent = n("hand");
                for (i, v) in joints.into_iter().enumerate() {
                    let bone = n(&format!("{name}_{:02}", i + 1));
                    self.push(bone.clone(), &parent, v);
                    parent = bone;
                }
            };
            for (name, y_off, len) in [
                ("index", -0.009 * h, 0.020 * h),
                ("middle", 0.0, 0.022 * h),
                ("pinky", 0.018 * h, 0.016 * h),
                ("ring", 0.009 * h, 0.020 * h),
            ] {
                finger(name, knuckle + Vec3::new(0.0, y_off, 0.0), len);
            }
            finger(
                "thumb",
                wrist + dir * (0.022 * h) + Vec3::new(0.0, -0.024 * h, 0.0),
                0.018 * h,
            );
        }
        Ok(())
    }

    /// The canon arm chain HANGING at the sides — every joint an offset from the `spine_03`
    /// socket in fractions of stature, measured off Aaron's hand-rigged LizardBaseA
    /// (2026-09-07, FDA5BE46): the shoulder out and a little back, the upper arm 20° out and
    /// 8° back, the forearm 17° out and 5° forward, the hand and fingers straight down. The
    /// same bones as the humanoid arm, so every clip resolves by name; only the rest differs,
    /// which is the whole point — a clip library baked on this pattern moves hanging arms the
    /// way the humanoid library moves A-posed ones.
    fn hanging_arm_pair(&mut self, h: f32, spine_03_name: &str, socket: Vec3) -> Result<()> {
        let p = self.prefix("arm");
        let at = |sign: f32, x: f32, y: f32, z: f32| socket + Vec3::new(x * h * sign, y * h, z * h);
        for (side, sign) in SIDES {
            let n = |base: &str| format!("{p}{base}_{side}");
            let clavicle = at(sign, 0.020, 0.020, 0.041);
            let shoulder = at(sign, 0.078, 0.045, 0.039);
            let elbow = at(sign, 0.131, 0.066, -0.110);
            let wrist = at(sign, 0.166, 0.055, -0.229);
            self.push(n("clavicle"), spine_03_name, clavicle);
            self.push(n("upperarm"), &n("clavicle"), shoulder);
            self.push(
                n("upperarm_twist_01"),
                &n("upperarm"),
                (shoulder + elbow) * 0.5,
            );
            self.push(n("lowerarm"), &n("upperarm"), elbow);
            self.push(
                n("lowerarm_twist_01"),
                &n("lowerarm"),
                (elbow + wrist) * 0.5,
            );
            self.push(n("hand"), &n("lowerarm"), wrist);
            // The grip point, mid-palm, straight below the wrist.
            let off = |x: f32, y: f32, z: f32| Vec3::new(x * h * sign, y * h, z * h);
            self.push(
                format!("{p}Weapon_{}", side.to_uppercase()),
                &n("hand"),
                wrist + off(0.008, -0.003, -0.049),
            );
            // Fingers: roots fan across Y at the knuckle line below the wrist, segments run
            // straight DOWN (3 phalanges, tapering); the thumb sits forward and higher.
            let mut finger = |name: &str, root: Vec3, len: f32| {
                let joints = [
                    root,
                    root + Vec3::new(0.0, 0.0, -len),
                    root + Vec3::new(0.0, 0.0, -len * 1.75),
                ];
                let mut parent = n("hand");
                for (i, v) in joints.into_iter().enumerate() {
                    let bone = n(&format!("{name}_{:02}", i + 1));
                    self.push(bone.clone(), &parent, v);
                    parent = bone;
                }
            };
            for (name, x, y, z, len) in [
                ("index", 0.013, -0.030, -0.064, 0.026),
                ("middle", 0.016, -0.014, -0.066, 0.029),
                ("ring", 0.015, 0.001, -0.064, 0.026),
                ("pinky", 0.008, 0.016, -0.062, 0.021),
            ] {
                finger(name, wrist + off(x, y, z), len * h);
            }
            finger("thumb", wrist + off(-0.010, -0.035, -0.027), 0.023 * h);
        }
        Ok(())
    }

    /// THE BIRD WING (P4, spec C658F114) hung from the shoulder `socket`, SPREAD WIDE — the
    /// pose the Meshy birds ship in, and the wing's own A-pose: clavicle → `upperarm`
    /// (shoulder) → `lowerarm` (elbow, out along the span) → `hand` (wrist) → `wing_tip`
    /// at 0.8h out; `wing_feathers_01` (the secondaries) hangs back off the forearm and
    /// `wing_feathers_02` (the primaries) back off the hand — the two bones a fold/spread or a
    /// flap turns. `h` = the standing height.
    fn bird_wing_pair(&mut self, h: f32, spine_03_name: &str, socket: Vec3) -> Result<()> {
        let p = self.prefix("arm");
        for (side, sign) in SIDES {
            let n = |base: &str| format!("{p}{base}_{side}");
            let at = |x: f32, y: f32, z: f32| socket + Vec3::new(x * h * sign, y * h, z * h);
            let chain = [
                ("clavicle", at(0.03, 0.0, 0.02)),
                ("upperarm", at(0.08, 0.0, 0.03)),
                ("lowerarm", at(0.30, 0.02, 0.06)),
                ("hand", at(0.52, 0.03, 0.07)),
                ("wing_tip", at(0.80, 0.05, 0.06)),
            ];
            let mut parent = spine_03_name.to_string();
            for (base, pos) in chain {
                let name = n(base);
                self.push(name.clone(), &parent, pos);
                parent = name;
            }
            self.push(n("wing_feathers_01"), &n("lowerarm"), at(0.36, 0.16, 0.02));
            self.push(n("wing_feathers_02"), &n("hand"), at(0.62, 0.15, 0.02));
        }
        Ok(())
    }

    /// THE BAT WING (P4, spec C658F114) hung from the shoulder `socket`, spread: the arm chain to
    /// the wrist, a clawed `wing_thumb` forward, and four membrane digits `wing_digit_1..4`
    /// (three segments each) fanning from the wrist — the first along the span to the tip, the
    /// fourth trailing back along the body. `h` = the standing (hanging) height.
    fn bat_wing_pair(&mut self, h: f32, spine_03_name: &str, socket: Vec3) -> Result<()> {
        let p = self.prefix("arm");
        for (side, sign) in SIDES {
            let n = |base: &str| format!("{p}{base}_{side}");
            let at = |x: f32, y: f32, z: f32| socket + Vec3::new(x * h * sign, y * h, z * h);
            let chain = [
                ("clavicle", at(0.03, 0.0, 0.02)),
                ("upperarm", at(0.10, 0.0, 0.02)),
                ("lowerarm", at(0.30, 0.02, 0.04)),
                ("hand", at(0.45, 0.03, 0.05)),
            ];
            let mut parent = spine_03_name.to_string();
            for (base, pos) in chain {
                let name = n(base);
                self.push(name.clone(), &parent, pos);
                parent = name;
            }
            let wrist = at(0.45, 0.03, 0.05);
            self.push(n("wing_thumb"), &n("hand"), at(0.48, -0.03, 0.06));
            // Digit tips fan from along the span (1) to back along the body (4).
            let tips = [
                at(0.85, 0.02, 0.04),
                at(0.80, 0.12, 0.02),
                at(0.70, 0.22, 0.0),
                at(0.55, 0.30, -0.02),
            ];
            for (d, tip) in tips.into_iter().enumerate() {
                let mut parent = n("hand");
                for seg in 1..=3 {
                    let t = seg as f32 / 3.0;
                    let name = n(&format!("wing_digit_{}_{seg:02}", d + 1));
                    self.push(name.clone(), &parent, wrist + (tip - wrist) * t);
                    parent = name;
                }
            }
        }
        Ok(())
    }

    /// THE QUADRUPED FORELEG (P3b, spec C658F114) hung from the withers `socket` (`h` = the
    /// withers height): the scapula (`clavicle`) high on the shoulder, the shoulder joint
    /// (`upperarm`) forward and well down, the elbow (`lowerarm`) back under it, the long
    /// forearm to the carpus (`hand`), the cannon to the fetlock (`foredigit`, 0.12h — where the
    /// hoofed sources' own fetlocks stand, 0.12–0.16h; first authored at 0.08h, a coronet's
    /// height, which is where the fit then looked for it), and the ground contact (`forehoof`)
    /// under it — the joint an IK foot-plant drives. Horse proportions; the same chain serves a
    /// paw (a shorter, more sloped digit is the human's placement). No twists.
    fn ungulate_foreleg_pair(&mut self, h: f32, spine_03_name: &str, socket: Vec3) -> Result<()> {
        let p = self.prefix("arm");
        for (side, sign) in SIDES {
            let n = |base: &str| format!("{p}{base}_{side}");
            let at = |x: f32, y: f32, z: f32| socket + Vec3::new(x * h * sign, y * h, z * h);
            let chain = [
                ("clavicle", at(0.06, 0.02, -0.06)),
                ("upperarm", at(0.09, -0.10, -0.28)),
                ("lowerarm", at(0.09, -0.04, -0.42)),
                ("hand", at(0.09, -0.06, -0.64)),
                ("foredigit", at(0.09, -0.07, -0.80)),
                ("forehoof", at(0.09, -0.08, -QUAD_WITHERS)),
            ];
            let mut parent = spine_03_name.to_string();
            for (base, pos) in chain {
                let name = n(base);
                self.push(name.clone(), &parent, pos);
                parent = name;
            }
        }
        Ok(())
    }

    /// One hip pair of `kind` hung from `pelvis_name` at `socket` (world, midline), on a trunk of
    /// `orientation` (the hoofed leg hangs from a quadruped's hip or a biped's).
    fn leg_pair(
        &mut self,
        kind: LegKind,
        h: f32,
        pelvis_name: &str,
        socket: Vec3,
        orientation: Orientation,
    ) -> Result<()> {
        if kind == LegKind::Unguligrade {
            return self.unguligrade_leg_pair(h, pelvis_name, socket, orientation);
        }
        if let LegKind::Bird { heel } = kind {
            return self.bird_leg_pair(heel, h, pelvis_name, socket);
        }
        let p = self.prefix("leg");
        for (side, sign) in SIDES {
            let n = |base: &str| format!("{p}{base}_{side}");
            let hip_x = 0.051 * h * sign; // femoral head offset from the midline
            let thigh = socket + Vec3::new(hip_x, 0.0, -0.030 * h);
            // Ball of the foot: forward along -Y, riding just above the sole — a toe walker's
            // where its recipe puts it (under the hip for the lizardman).
            let ball_y = match kind {
                LegKind::ToeWalker { toe, .. } => toe * h,
                _ => -0.071 * h,
            };
            let ball = socket + Vec3::new(hip_x, ball_y, -0.548 * h);
            let (calf, foot) = match kind {
                // A vertical column, ankle slightly back, sole flat on the ground.
                LegKind::Plantigrade => (
                    socket + Vec3::new(hip_x, 0.0, -0.275 * h),
                    socket + Vec3::new(hip_x, 0.015 * h, -0.521 * h),
                ),
                // The heel RAISED to `heel` of stature, set back behind the hip line; the same
                // thigh and calf LENGTHS as the plantigrade leg, so the knee is where two
                // circles meet — taken on the forward side, the Z-shaped digitigrade stance
                // (cats, dogs, catman, werewolf, lizardman, heeled shoes). The metatarsus runs
                // from the raised ankle down to the toe on the ground.
                LegKind::Digitigrade { heel } => {
                    anyhow::ensure!(
                        (0.02..=0.45).contains(&heel),
                        "a digitigrade heel of {heel} of stature is off the body (0.02–0.45)"
                    );
                    let ankle = socket + Vec3::new(hip_x, 0.05 * h, -(0.560 - heel) * h);
                    let thigh_len = 0.245 * h;
                    let calf_len = (0.246f32.powi(2) + 0.015f32.powi(2)).sqrt() * h;
                    (knee_between(thigh, ankle, thigh_len, calf_len), ankle)
                }
                // A LONG-FOOT TOE WALKER (Aaron 2026-09-07 — lizardman, faun, the cat peoples):
                // the heel raised to `heel` and set BACK `setback` behind the hip line, a shorter
                // thigh and shin meeting at a knee only mildly bent, and a long metatarsus from
                // the raised heel down-forward to a ball at `toe`. The numbers come off the
                // hand-rigged LizardBaseA (FDA5BE46): every bone direction within 2° of it.
                LegKind::ToeWalker {
                    heel,
                    setback,
                    thigh: thigh_frac,
                    calf: calf_frac,
                    ..
                } => {
                    anyhow::ensure!(
                        (0.02..=0.45).contains(&heel),
                        "a toe walker's heel of {heel} of stature is off the body (0.02–0.45)"
                    );
                    anyhow::ensure!(
                        thigh_frac >= 0.05 && calf_frac >= 0.05,
                        "a toe walker's thigh ({thigh_frac}) and shin ({calf_frac}) must each be at least 0.05 of stature"
                    );
                    let ankle = socket + Vec3::new(hip_x, setback * h, -(0.560 - heel) * h);
                    let (thigh_len, calf_len) = (thigh_frac * h, calf_frac * h);
                    anyhow::ensure!(
                        ankle.distance(thigh) <= thigh_len + calf_len + 1e-3,
                        "a toe walker's thigh ({thigh_frac}) and shin ({calf_frac}) cannot reach a heel at {heel} set back {setback}"
                    );
                    (knee_between(thigh, ankle, thigh_len, calf_len), ankle)
                }
                LegKind::Unguligrade => unreachable!("composed by unguligrade_leg_pair"),
                LegKind::Bird { .. } => unreachable!("composed by bird_leg_pair"),
            };
            self.push(n("thigh"), pelvis_name, thigh);
            self.push(n("thigh_twist_01"), &n("thigh"), (thigh + calf) * 0.5);
            self.push(n("calf"), &n("thigh"), calf);
            self.push(n("calf_twist_01"), &n("calf"), (calf + foot) * 0.5);
            self.push(n("foot"), &n("calf"), foot);
            self.push(n("ball"), &n("foot"), ball);
        }
        Ok(())
    }

    /// THE BIRD LEG (P4) hung from the hip `socket`: a short `thigh` inside the body, the `calf`
    /// (knee) forward, the ankle (`foot` — the visible backward "knee") raised to `heel` of
    /// stature and set back, the tarsometatarsus down to the toe base (`ball`) on the ground,
    /// three forward toes and the hind toe (`hallux`) on the ground — the joints a perch IK
    /// curls round a branch. No twists.
    fn bird_leg_pair(&mut self, heel: f32, h: f32, pelvis_name: &str, socket: Vec3) -> Result<()> {
        anyhow::ensure!(
            (0.05..=0.45).contains(&heel),
            "a bird's ankle at {heel} of stature is off the body (0.05–0.45)"
        );
        let p = self.prefix("leg");
        for (side, sign) in SIDES {
            let n = |base: &str| format!("{p}{base}_{side}");
            let at = |x: f32, y: f32, z: f32| socket + Vec3::new(x * h * sign, y * h, z * h);
            let ball = at(0.06, -0.02, -0.56);
            let chain = [
                ("thigh", at(0.06, 0.0, -0.03)),
                ("calf", at(0.06, -0.06, -0.18)),
                ("foot", at(0.06, 0.04, -(0.56 - heel))),
                ("ball", ball),
            ];
            let mut parent = pelvis_name.to_string();
            for (base, pos) in chain {
                let name = n(base);
                self.push(name.clone(), &parent, pos);
                parent = name;
            }
            self.push(
                n("toe_01"),
                &n("ball"),
                ball + Vec3::new(0.0, -0.08 * h, 0.0),
            );
            self.push(
                n("toe_02"),
                &n("ball"),
                ball + Vec3::new(0.04 * h * sign, -0.06 * h, 0.0),
            );
            self.push(
                n("toe_03"),
                &n("ball"),
                ball + Vec3::new(-0.04 * h * sign, -0.06 * h, 0.0),
            );
            self.push(
                n("hallux"),
                &n("ball"),
                ball + Vec3::new(0.0, 0.05 * h, 0.0),
            );
        }
        Ok(())
    }

    /// THE HOOFED HIND LEG (P3b, spec C658F114), authored to the anatomy Aaron stated on the Elk
    /// (incident BAD0D72C): offsets from the hip `socket` S the quadruped trunk hangs every leg
    /// module from (0.56h, `h` = the withers height). The hip joint (`thigh`) sits HIGH AT THE
    /// REAR, just under the croup (S + 0.22h = 0.78h); the femur runs FORWARD and down to the
    /// stifle (`calf`, the true knee, along the belly: 0.14h ahead, at 0.56h); the tibia runs BACK
    /// to the hock (`foot` — the common "knee", the ankle: 0.38h, behind the hip line); the cannon
    /// straight down to the fetlock (`ball`, 0.13h — where the hoofed sources' own fetlocks
    /// stand; 0.10h until 2026-09-30); the pastern to the `hoof` — a toe, on the ground, the
    /// joint an IK foot-plant drives. Femur ≈ 0.26h, tibia 0.27h, cannon 0.25h, pastern 0.13h.
    /// No twists.
    ///
    /// A PLUMB trunk (the hoofed biped, `Pattern::Unguligrade`) offers its pelvis as S and the hip
    /// every biped leg hangs from (S − 0.03h): the same leg hangs from there, scaled about the
    /// ground under S so its hoof still meets it.
    fn unguligrade_leg_pair(
        &mut self,
        h: f32,
        pelvis_name: &str,
        socket: Vec3,
        orientation: Orientation,
    ) -> Result<()> {
        // (joint, along the body: + back / − forward, up) off S in `h`; the hoof 0.56h under S.
        const CHAIN: [(&str, f32, f32); 5] = [
            ("thigh", 0.0, 0.22),
            ("calf", -0.14, 0.0),
            ("foot", 0.06, -0.18),
            ("ball", 0.05, -0.43),
            ("hoof", 0.035, -0.56),
        ];
        let hip = match orientation {
            Orientation::Quadruped => CHAIN[0].2,
            Orientation::Biped => -0.03,
        };
        let k = (0.56 + hip) / (0.56 + CHAIN[0].2);
        let p = self.prefix("leg");
        for (side, sign) in SIDES {
            let mut parent = pelvis_name.to_string();
            for (base, y, z) in CHAIN {
                let name = format!("{p}{base}_{side}");
                let at = Vec3::new(0.09 * sign, k * y, k * (z + 0.56) - 0.56);
                self.push(name.clone(), &parent, socket + at * h);
                parent = name;
            }
        }
        Ok(())
    }

    /// One tail off `pelvis_name` at `pelvis` (world): its root behind and a little below the
    /// pelvis, then either one bone, a chain of articulated bones, or one bone with a chain of
    /// physics-driven hair bones. Tail instance k ≥ 2 numbers its bones `tail<k>_NN`.
    fn tail(&mut self, kind: TailKind, h: f32, pelvis_name: &str, pelvis: Vec3) -> Result<()> {
        let p = self.prefix("tail");
        let stem = if p.is_empty() {
            "tail".to_string()
        } else {
            p.trim_end_matches('_').to_string()
        };
        let root = pelvis + Vec3::new(0.0, 0.08 * h, -0.03 * h);
        let (articulated, hair) = match kind {
            TailKind::Short => (1u8, 0u8),
            TailKind::ShortHair { bones } => (1, bones),
            TailKind::Long { bones } => (bones, 0),
        };
        anyhow::ensure!(
            (1..=32).contains(&articulated) && hair <= 32,
            "{kind:?} is outside the authored range (1–32 articulated, ≤ 32 hair bones)"
        );
        let mut parent = pelvis_name.to_string();
        let mut at = root;
        for i in 1..=articulated {
            let name = format!("{stem}_{i:02}");
            self.push(name.clone(), &parent, at);
            parent = name;
            at += Vec3::new(0.0, 0.06 * h, -0.02 * h);
        }
        let mut at = root + Vec3::new(0.0, 0.04 * h, -0.02 * h);
        for i in 1..=hair {
            let name = format!("{stem}_hair_{i:02}");
            self.push(name.clone(), &parent, at);
            parent = name;
            at += Vec3::new(0.0, 0.04 * h, -0.02 * h);
        }
        Ok(())
    }
}

/// THE MARKERS RAIL's prompted joints for `recipe`, in the order the rail walks them (spec
/// FF40E825, ruling 42AB9BA8) — the joints Aaron says "truly require a human eyeball", and the
/// only ones the human is asked to place before Infer derives the rest.
///
/// DERIVED FROM THE RECIPE'S MODULES, never an authored list: a body's prompts are exactly what
/// its own legs, arms, head and tails imply, so a new recipe needs no content file and a chained
/// or mounted trunk gets its prompts with its own prefixes for free. The prefixes come out of
/// [`next_prefix`] walked in `compose`'s own order (head → proboscis → arms → legs → tails → mounts), so every
/// name here IS a bone of `compose(recipe)` — the gate asserts exactly that.
///
/// Per module: a LEG gives its hip (`thigh`), its hock/ankle (`foot`) and its ground joint (`ball`,
/// or the `hoof` a hoofed leg stands on); an ARM gives the shoulder and the wrist, plus the ground
/// contact of a foreleg and the tip of a wing; a trunk with a head gives the `head`; a tail gives
/// its first bone.
///
/// ORDER: **THE ROOT FIRST.** Per trunk: its `pelvis`, then a quadruped trunk's `spine_03` (the
/// withers — the other end of a body that lies along the ground), then the head, then the first
/// tail bone, and only then the limb chains outward — legs before arms, in module order, root-most
/// first within a module, LEFT before RIGHT. A mounted trunk follows its parent's whole list.
///
/// The rail used to open on the THIGH (incident D81498B7, Aaron in the window on the Horse:
/// *"it's asking me to place the thigh first, but the pelvis center needs to be placed first …
/// the whole thing is all fucked up because the pelvis isn't even inside the mesh"*). A drag
/// carries the joint's whole subtree, so the pelvis is the one prompt that moves everything: asked
/// first it is a placement, asked last it undoes every answer before it.
pub fn markers_for(recipe: &SkeletonRecipe) -> Vec<String> {
    markers_by_module(recipe)
        .into_iter()
        .map(|(_, m)| m)
        .collect()
}

/// [`markers_for`], with each prompt tagged by the MODULE it belongs to — `"<kind>:<prefix>"`,
/// the kind and the instance prefix [`Composer`] numbers that module's bones with (`"leg:"`,
/// `"arm:arm2_"`, `"trunk:trunk2_"`). The shape matcher (`conform::match_recipe`) walks the same
/// recipe with the same counters and reports what it matched by the same id, so the rail can ask
/// for the UNMATCHED modules first without a second naming convention to keep in step.
pub fn markers_by_module(recipe: &SkeletonRecipe) -> Vec<(String, String)> {
    let Ok((bones, modules)) = compose_with_modules(recipe, STATURE) else {
        return Vec::new();
    };
    let index: HashMap<&str, usize> = bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let parent: Vec<Option<usize>> = bones
        .iter()
        .map(|b| index.get(b.parent.as_str()).copied())
        .collect();
    // How many children a bone has IN EACH MODULE — the one structural thing that tells a FAN
    // (fingers, toes, a bird's feather groups, a bat's digits, a head's jaw and eyes) from a
    // chain carrying on or from the limb roots hanging off a pelvis. A pelvis has four children,
    // but only ONE of them is its own module's spine; a hand has six and all six are the arm's.
    let mut siblings: HashMap<(usize, &str), usize> = HashMap::new();
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            *siblings.entry((*p, modules[i].as_str())).or_insert(0) += 1;
        }
    }
    // A bone RIDES INFER when it is a twist helper, or when it is one of three or more siblings
    // of its own module — and so does everything under it (a finger's second and third phalanx
    // are no more asked for than the first).
    let mut ride = vec![false; bones.len()];
    for i in 0..bones.len() {
        let fan = parent[i]
            .and_then(|p| siblings.get(&(p, modules[i].as_str())).copied())
            .is_some_and(|n| n >= 3);
        let inherited = parent[i].is_some_and(|p| ride[p]);
        ride[i] = inherited || fan || bones[i].name.contains("_twist_");
    }
    // THE MODULE ORDER of the rail, then every bone of each module in the order the composer
    // emitted it — which is already parents before children, root-most first, LEFT before RIGHT.
    let mut walk = Rail::default();
    walk.trunk(&recipe.trunk);
    let mut out = Vec::new();
    for id in walk.out {
        for (i, b) in bones.iter().enumerate() {
            if modules[i] == id && !ride[i] {
                out.push((id.clone(), b.name.clone()));
            }
        }
    }
    out
}

/// THE RAIL'S MODULE ORDER. The recipe walked with the SAME per-kind counters [`Composer`]
/// numbers its bones with — the prefixes are claimed in the composer's order (trunk, head, arms,
/// legs, tails, mounts), because that walk is what decides which instance is which — but they are
/// ASKED FOR in the rail's own order: the root trunk, its head, the legs a body stands on, then
/// its arms, then its tails, and a mounted trunk's whole list after its parent's.
#[derive(Default)]
struct Rail {
    out: Vec<String>,
    counts: HashMap<&'static str, usize>,
}

impl Rail {
    fn trunk(&mut self, spec: &TrunkSpec) {
        let p = next_prefix(&mut self.counts, "trunk");
        let head = spec.head.then(|| next_prefix(&mut self.counts, "head"));
        let proboscis =
            (spec.head && spec.proboscis > 0).then(|| next_prefix(&mut self.counts, "proboscis"));
        let arms: Vec<String> = spec
            .arms
            .iter()
            .map(|_| next_prefix(&mut self.counts, "arm"))
            .collect();
        let legs: Vec<String> = spec
            .legs
            .iter()
            .map(|_| next_prefix(&mut self.counts, "leg"))
            .collect();
        let tails: Vec<String> = spec
            .tails
            .iter()
            .map(|_| next_prefix(&mut self.counts, "tail"))
            .collect();
        self.out.push(module_id("trunk", &p));
        if let Some(h) = head {
            self.out.push(module_id("head", &h));
        }
        if let Some(n) = proboscis {
            self.out.push(module_id("proboscis", &n));
        }
        for k in &legs {
            self.out.push(module_id("leg", k));
        }
        for k in &arms {
            self.out.push(module_id("arm", k));
        }
        for k in &tails {
            self.out.push(module_id("tail", k));
        }
        for mount in &spec.mounts {
            self.trunk(&mount.trunk);
        }
    }
}

/// THE ONE SPELLING of a module's identity — its kind and the instance prefix its bones carry.
/// Written here because three walks have to agree on it: [`Composer`] (which emits the bones),
/// [`markers_by_module`] (which names the prompts) and `conform::match_recipe` (which says which
/// module took which piece of the mesh).
pub fn module_id(kind: &str, prefix: &str) -> String {
    format!("{kind}:{prefix}")
}

/// A named recipe on disk — `package/skeletons/<Pattern>/<Name>.recipe.json`, the authored
/// SOURCE in the folder of the PATTERN it animates on, beside that pattern's reference rig
/// (Aaron 2026-09-08: the recipes live with the skeleton rigs, in their own root) — what the
/// Clayworks Prep step steps through.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub recipe: SkeletonRecipe,
}

/// The shipped presets, Humanoid first and always present, the rest by file name. A file
/// that fails to parse is reported on the log and skipped — the bench must never lose the
/// humanoid because a hand-edited preset has a typo.
pub fn load_presets(dir: &Path) -> Vec<Preset> {
    let mut out = vec![Preset {
        name: "Humanoid".to_string(),
        recipe: SkeletonRecipe {
            preset: Some("Humanoid".to_string()),
            ..SkeletonRecipe::humanoid()
        },
    }];
    // One folder per PATTERN under the skeletons root: every `*.recipe.json` one level down
    // (`<Pattern>/<Name>.recipe.json`), whatever the folder is called.
    let is_recipe = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".recipe.json"))
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir())
                .flat_map(|d| {
                    std::fs::read_dir(d)
                        .into_iter()
                        .flatten()
                        .filter_map(|e| e.ok().map(|e| e.path()))
                })
                .filter(|p| is_recipe(p))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    for path in files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".recipe.json"))
            .unwrap_or_default()
            .to_string();
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<SkeletonRecipe>(&s).map_err(|e| e.to_string()))
        {
            Ok(mut recipe) => {
                recipe.preset = Some(name.clone());
                if let Some(existing) = out.iter_mut().find(|p| p.name == name) {
                    existing.recipe = recipe;
                } else {
                    out.push(Preset { name, recipe });
                }
            }
            Err(e) => tracing::warn!(preset = %path.display(), "skeleton preset skipped: {e}"),
        }
    }
    out
}

/// THE SKELETONS ROOT — `package/skeletons/`, one folder per [`Pattern`] (Aaron 2026-09-08:
/// "we should have done it that way to start"): `<Pattern>/<Pattern>.json.gz` is the pattern's
/// reference rig (its animation basis, the retarget target), and every `<Name>.recipe.json`
/// beside it is a shipped recipe that animates on that pattern. Bodies live in
/// `package/characters/` and carry their recipe INSIDE their rig; a library re-baked on a
/// pattern's rest is `package/retarget/clips/<library>@<Pattern>/`.
pub fn skeletons_dir() -> PathBuf {
    crate::roots::roots().package().join("skeletons")
}

/// A pattern's folder under the skeletons root — the ONE place the naming lives.
pub fn pattern_dir(pattern: Pattern) -> PathBuf {
    skeletons_dir().join(pattern.name())
}

/// The recipe a pattern's REFERENCE rig is composed from: the humanoid canon with the pattern's
/// leg module. The toe walker keeps the hanging arms Aaron measured off his lizard (556DF849) —
/// the rest its libraries were baked on; every other module is the canon's. A body of any
/// recipe on the pattern animates on this rest (arm directions re-aim at load).
pub fn reference_recipe(pattern: Pattern) -> SkeletonRecipe {
    let mut recipe = SkeletonRecipe::humanoid();
    recipe.trunk.legs = vec![pattern.leg()];
    recipe.trunk.arms = vec![pattern.arm()];
    // The quadruped's reference: the trunk along the back, ungulate forelegs at the withers,
    // unguligrade hind legs at the hips — a hoofed stance (P3b). Paws and other hind-leg
    // kinds are the body's own recipe. A bird stands on a plumb trunk with its wings spread;
    // a bat likewise.
    if pattern == Pattern::Quadruped {
        recipe.trunk.orientation = Orientation::Quadruped;
    }
    recipe
}

/// Write a pattern's reference rig into a skeletons root as `<Pattern>/<Pattern>.json` (gz at
/// rest via the shared seam) — `bake_baseline --pattern <Name>`. Fails loud for a pattern whose
/// modules are not authored yet (Unguligrade until P3).
pub fn emit_pattern(skeletons_root: &Path, pattern: Pattern, stature: f32) -> Result<PathBuf> {
    let rig = emit_skeleton(
        skeletons_root,
        &reference_recipe(pattern),
        stature,
        pattern.name(),
    )?;
    emit_pattern_pack(skeletons_root, pattern)?;
    Ok(rig)
}

use flicker_skeletal::state::{
    ClipSource, Gait, GaitFamily, GeneratedGait, PackFile, StateDef, StateMachineDef,
    TransitionDef, Trigger,
};

/// A creature pattern's DEFAULT CONTROLLER — `skeletons/<Pattern>/<Pattern>.pack.json`, every
/// state a GENERATED gait ([`ClipSource::Generated`]; gait generator / IK design 37704D6B, G5):
/// the quadrupeds, birds and bats have no motion library, the generator is their basis. `None`
/// for the humanoid family — its default is the recovered Katanami graph, hand-authored beside
/// the reference — and for the biped variants, which resolve to it.
///
/// The graphs speak the humanoid pack's signal vocabulary — `move` / `run` / `crouch` / `jump`
/// — so the one set of controller signals drives every body on the stage:
/// - **Quadruped** — Idle (a stand) → Walk on `move` → Trot on `run` → Run (the gallop) on
///   `jump` while trotting; the held `crouch` is Climb — the walk with cling, the climber
///   family — so a branch walker can be driven before its surfaces exist (G3).
/// - **Bird** — Idle → Walk on `move` → Hop on `run` (a hopper's fast gait); `jump` takes off
///   into Fly, `crouch` glides, `jump` again perches, `move` off the perch walks.
/// - **Bat** — Idle (a climber's stand) → Fly on `jump` → Glide on `crouch` → Perch on `jump`;
///   `move` off the perch drops back to Idle.
///
/// Fly, Glide and Perch have no generator until the flap cycle (G4): a consumer says so.
pub fn default_pack(pattern: Pattern) -> Option<PackFile> {
    use GaitFamily::{Climber, Hopper, Walker};
    use Trigger::{Crouch, CrouchStop, Jump, Move, MoveStop, Run, RunStop};
    let states = match pattern {
        Pattern::Quadruped => vec![
            gait_state(
                "Idle",
                Gait::Stand,
                Walker,
                &[("Climb", Crouch, 4), ("Walk", Move, 1)],
            ),
            gait_state(
                "Walk",
                Gait::Walk,
                Walker,
                &[
                    ("Climb", Crouch, 4),
                    ("Trot", Run, 3),
                    ("Idle", MoveStop, 1),
                ],
            ),
            gait_state(
                "Trot",
                Gait::Trot,
                Walker,
                &[
                    ("Run", Jump, 5),
                    ("Idle", MoveStop, 4),
                    ("Walk", RunStop, 3),
                ],
            ),
            gait_state(
                "Run",
                Gait::Gallop,
                Walker,
                &[("Idle", MoveStop, 4), ("Walk", RunStop, 3)],
            ),
            gait_state("Climb", Gait::Climb, Climber, &[("Idle", CrouchStop, 4)]),
        ],
        Pattern::Bird => vec![
            gait_state(
                "Idle",
                Gait::Stand,
                Walker,
                &[("Fly", Jump, 5), ("Walk", Move, 1)],
            ),
            gait_state(
                "Walk",
                Gait::Walk,
                Walker,
                &[("Fly", Jump, 5), ("Hop", Run, 3), ("Idle", MoveStop, 1)],
            ),
            gait_state(
                "Hop",
                Gait::Hop,
                Hopper,
                &[
                    ("Fly", Jump, 5),
                    ("Idle", MoveStop, 4),
                    ("Walk", RunStop, 3),
                ],
            ),
            gait_state(
                "Fly",
                Gait::Fly,
                Walker,
                &[("Perch", Jump, 5), ("Glide", Crouch, 4)],
            ),
            gait_state(
                "Glide",
                Gait::Glide,
                Walker,
                &[("Perch", Jump, 5), ("Fly", CrouchStop, 4)],
            ),
            gait_state(
                "Perch",
                Gait::Perch,
                Walker,
                &[("Fly", Jump, 5), ("Walk", Move, 1)],
            ),
        ],
        Pattern::Bat => vec![
            gait_state("Idle", Gait::Stand, Climber, &[("Fly", Jump, 5)]),
            gait_state(
                "Fly",
                Gait::Fly,
                Climber,
                &[("Perch", Jump, 5), ("Glide", Crouch, 4)],
            ),
            gait_state(
                "Glide",
                Gait::Glide,
                Climber,
                &[("Perch", Jump, 5), ("Fly", CrouchStop, 4)],
            ),
            gait_state(
                "Perch",
                Gait::Perch,
                Climber,
                &[("Fly", Jump, 5), ("Idle", Move, 1)],
            ),
        ],
        Pattern::Humanoid | Pattern::Digitigrade | Pattern::ToeWalker | Pattern::Unguligrade => {
            return None;
        }
    };
    let name = pattern.name();
    Some(PackFile {
        format: "flicker.pack".into(),
        version: 1,
        note: format!(
            "The {name} pattern's DEFAULT CONTROLLER, written by flicker-content \
             baseline::emit_pattern_pack (gait generator G5) — regenerate with `cargo run -p \
             flicker-content --example bake_baseline -- --pattern {name}`, never by hand. Every \
             state is a GENERATED gait (no clip library exists for these bodies): the runtime \
             steps the gait generator for the state's gait at the speed that means it. Signals \
             as the humanoid pack: move / run / crouch / jump. A body's own pack beside its rig \
             overrides this."
        ),
        state_machine: StateMachineDef {
            initial: "Idle".into(),
            default_blend_ticks: 0,
            tick_rate_hz: 60,
            any: Vec::new(),
            states,
        },
    })
}

/// One generated state with its outgoing edges `(to, on, priority)`.
fn gait_state(
    name: &str,
    gait: Gait,
    family: GaitFamily,
    edges: &[(&str, Trigger, i32)],
) -> StateDef {
    StateDef {
        name: name.into(),
        clip: ClipSource::Generated(GeneratedGait { gait, family }),
        looping: true,
        next: None,
        root_motion: false,
        stamina_cost: 0.0,
        blocking: false,
        guard_angle: None,
        transitions: edges
            .iter()
            .map(|&(to, on, priority)| TransitionDef {
                to: to.into(),
                on,
                window: None,
                priority,
                blend_ticks: None,
                on_incoming: None,
            })
            .collect(),
        events: Vec::new(),
    }
}

/// Write a pattern's default controller pack beside its reference rig —
/// `<Pattern>/<Pattern>.pack.json` (gz at rest via the shared seam) — for the patterns that
/// have one ([`default_pack`]); `Ok(None)` for the rest. [`emit_pattern`] calls it, so a bake
/// ships the pattern's folder complete.
pub fn emit_pattern_pack(skeletons_root: &Path, pattern: Pattern) -> Result<Option<PathBuf>> {
    let Some(pack) = default_pack(pattern) else {
        return Ok(None);
    };
    let dir = skeletons_root.join(pattern.name());
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{}.pack.json", pattern.name()));
    flicker_skeletal::state::write_pack(&out, &pack)?;
    Ok(Some(out))
}

/// One line for a panel: what the recipe composes to — bone count and the parts that differ
/// from the humanoid canon (a variant that does not compose says why instead).
pub fn describe(recipe: &SkeletonRecipe) -> String {
    fn walk(trunk: &TrunkSpec, parts: &mut Vec<String>, trunks: &mut usize) {
        *trunks += 1;
        if trunk.orientation == Orientation::Quadruped {
            parts.push("quadruped".to_string());
        }
        if !trunk.head {
            parts.push("headless".to_string());
        }
        for arm in &trunk.arms {
            match arm {
                ArmKind::Humanoid => {}
                ArmKind::Hanging => parts.push("hanging arms".to_string()),
                ArmKind::Bat => parts.push("bat wings".to_string()),
                ArmKind::Bird => parts.push("bird wings".to_string()),
                ArmKind::Ungulate => parts.push("forelegs".to_string()),
            }
        }
        if trunk.arms.len() > 1 {
            parts.push(format!("{} shoulder pairs", trunk.arms.len()));
        }
        for leg in &trunk.legs {
            match leg {
                LegKind::Plantigrade => {}
                LegKind::Digitigrade { heel } => {
                    parts.push(format!("digitigrade legs (heel {:.0}%)", heel * 100.0))
                }
                LegKind::ToeWalker { heel, .. } => parts.push(format!(
                    "toe-walker legs (long foot, heel {:.0}%)",
                    heel * 100.0
                )),
                LegKind::Unguligrade => parts.push("hooves".to_string()),
                LegKind::Bird { heel } => {
                    parts.push(format!("bird legs (ankle {:.0}%)", heel * 100.0))
                }
            }
        }
        if trunk.head && trunk.proboscis > 0 {
            parts.push(format!("proboscis ({})", trunk.proboscis));
        }
        for tail in &trunk.tails {
            parts.push(match tail {
                TailKind::Short => "short tail".to_string(),
                TailKind::ShortHair { bones } => format!("hair tail ({bones})"),
                TailKind::Long { bones } => format!("long tail ({bones})"),
            });
        }
        for m in &trunk.mounts {
            if m.socket == Socket::Neck {
                // The mounted trunk's pelvis replaced this trunk's neck (ruling 2026-09-12).
                parts.push("neck mount".to_string());
            }
            walk(&m.trunk, parts, trunks);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    let mut trunks = 0;
    walk(&recipe.trunk, &mut parts, &mut trunks);
    if trunks > 1 {
        parts.insert(0, format!("{trunks} trunks"));
    }
    let head = match compose(recipe, STATURE) {
        Ok(bones) => format!("{} bones", bones.len()),
        Err(e) => return format!("does not compose: {e}"),
    };
    if parts.is_empty() {
        format!("{head} · the humanoid canon")
    } else {
        format!("{head} · {}", parts.join(" · "))
    }
}

/// Every canon bone's world rest position — the humanoid recipe composed at [`STATURE`].
pub fn world_positions() -> HashMap<String, Vec3> {
    compose(&SkeletonRecipe::humanoid(), STATURE)
        .expect("the humanoid recipe composes")
        .into_iter()
        .map(|b| (b.name, b.position))
        .collect()
}

/// Assemble a composed recipe as a skeleton-only `flicker.rig`: translation-only locals
/// (identity rest rotations), inverse binds straight from the world rests, empty mesh/clips
/// — a reference is nobody's body.
pub fn skeleton_rig(recipe: &SkeletonRecipe, stature: f32, name: &str) -> Result<RigFile> {
    let authored = compose(recipe, stature)?;
    let index: HashMap<&str, usize> = authored
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let bones: Vec<BoneRaw> = authored
        .iter()
        .map(|b| {
            let parent = index.get(b.parent.as_str()).copied();
            let local_t = match parent {
                Some(pi) => b.position - authored[pi].position,
                None => b.position,
            };
            BoneRaw {
                name: b.name.clone(),
                parent: parent.map_or(-1, |pi| pi as i32),
                local: Mat4::from_translation(local_t).to_cols_array(),
                inverse_bind: Mat4::from_translation(-b.position).to_cols_array(),
            }
        })
        .collect();
    Ok(RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: name.to_string(),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: format!(
                "authored baseline: composed recipe, A-pose ({A_POSE_DEG}\u{00b0}) at {stature} cm, no source mesh"
            ),
            ..Default::default()
        },
        skeleton: Skeleton { bones },
        mesh: Default::default(),
        clips: Vec::new(),
        attach: Default::default(),
        attach_points: Vec::new(),
        collision: Default::default(),
        retarget: true,
        skeleton_recipe: None,
    })
}

/// The humanoid canon as a skeleton-only `flicker.rig` — the packaged `Humanoid` reference
/// (historically `GolemBaseSkeleton`; the bones are the same, bit for bit).
pub fn golem_base_skeleton() -> RigFile {
    skeleton_rig(
        &SkeletonRecipe::humanoid(),
        STATURE,
        Pattern::Humanoid.name(),
    )
    .expect("the humanoid recipe composes")
}

/// Write the humanoid reference into a skeletons root as `Humanoid/Humanoid.json` (gz at rest
/// via the shared seam). Returns the logical path.
pub fn emit(skeletons_root: &Path) -> Result<PathBuf> {
    emit_pattern(skeletons_root, Pattern::Humanoid, STATURE)
}

/// Write ANY recipe's skeleton-only rig into a root as `<name>/<name>.json` — the retarget
/// target for baking a clip library onto that recipe's rest.
pub fn emit_skeleton(
    characters_root: &Path,
    recipe: &SkeletonRecipe,
    stature: f32,
    name: &str,
) -> Result<PathBuf> {
    let dir = characters_root.join(name);
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{name}.json"));
    let mut rig = skeleton_rig(recipe, stature, name)?;
    rig.skeleton_recipe = Some(recipe.clone());
    crate::bake::write_rig_file(&rig, &out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_skeletal::format::{pattern_of, Mount};

    /// THE CREATURE PATTERNS' DEFAULT CONTROLLERS (G5): Quadruped, Bird and Bat each ship a
    /// pack whose EVERY state is a generated gait — the states the design names, opening on
    /// Idle, every edge resolving, no edge that can never fire — building against no clip
    /// library at all; the humanoid family ships none here (its default is the recovered
    /// Katanami graph, hand-authored).
    #[test]
    fn the_creature_patterns_default_to_generated_packs() {
        use flicker_skeletal::state::StateMachine;
        for (pattern, expected) in [
            (
                Pattern::Quadruped,
                vec!["Idle", "Walk", "Trot", "Run", "Climb"],
            ),
            (
                Pattern::Bird,
                vec!["Idle", "Walk", "Hop", "Fly", "Glide", "Perch"],
            ),
            (Pattern::Bat, vec!["Idle", "Fly", "Glide", "Perch"]),
        ] {
            let pack =
                default_pack(pattern).unwrap_or_else(|| panic!("{pattern:?} ships a default pack"));
            assert_eq!(pack.format, "flicker.pack");
            let sm = &pack.state_machine;
            assert_eq!(sm.initial, "Idle");
            let names: Vec<&str> = sm.states.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(names, expected, "{pattern:?}");
            assert!(
                sm.states.iter().all(|s| s.clip.generated().is_some()),
                "{pattern:?}: every state is generated"
            );
            let machine = StateMachine::build(sm, &[]).expect("builds with no clip library");
            assert!(
                machine.warnings().is_empty(),
                "{pattern:?}: {:?}",
                machine.warnings()
            );
            assert!(machine.has_graph(), "{pattern:?}: a real graph");
            if pattern != Pattern::Bat {
                let walk = sm.states.iter().find(|s| s.name == "Walk").unwrap();
                assert_eq!(walk.clip.generated().map(|g| g.gait), Some(Gait::Walk));
            }
        }
        for pattern in [
            Pattern::Humanoid,
            Pattern::Digitigrade,
            Pattern::ToeWalker,
            Pattern::Unguligrade,
        ] {
            assert!(
                default_pack(pattern).is_none(),
                "{pattern:?} resolves to the humanoid's hand-authored pack"
            );
        }
    }

    /// The SHIPPED default packs are the pipeline's: what `bake_baseline --pattern <P>` wrote
    /// parses back equal to [`default_pack`] — a pack edited by hand, or written by an older
    /// writer, drifts here. Skips without the content tree, like every real-data gate.
    #[test]
    fn the_shipped_default_packs_are_the_pipelines() {
        for pattern in [Pattern::Quadruped, Pattern::Bird, Pattern::Bat] {
            let path = pattern_dir(pattern).join(format!("{}.pack.json", pattern.name()));
            if !path.exists() && !path.with_extension("json.gz").exists() {
                eprintln!("skipping: no {}", path.display());
                continue;
            }
            let shipped =
                flicker_skeletal::state::read_pack(&path).expect("the shipped pack parses");
            let expected = default_pack(pattern).unwrap();
            assert_eq!(
                serde_json::to_value(&shipped).unwrap(),
                serde_json::to_value(&expected).unwrap(),
                "{}: drifted from the writer — re-run bake_baseline --pattern {}",
                path.display(),
                pattern.name()
            );
        }
    }

    /// THE BASELINE LINT — the gates that make "the skeleton is right" a property,
    /// not an eyeball: canon shape, exact mirror symmetry, level shoulders, flat
    /// soles, plumb spine, the ruled stature, and segment sanity.
    #[test]
    fn the_baseline_is_symmetric_level_flat_and_at_stature() {
        let rig = golem_base_skeleton();
        assert_eq!(
            rig.skeleton.bones.len(),
            CANON_BONES,
            "the canon count holds"
        );

        // Hierarchy order: every parent precedes its child (loaders assume it).
        for (i, b) in rig.skeleton.bones.iter().enumerate() {
            assert!(
                b.parent < i as i32,
                "bone {i} `{}` precedes its parent",
                b.name
            );
        }
        // Topology matches the canon table: the same names, the same parents (bone ORDER
        // within a rig is not canonical — a composed rig lists depth-first by module).
        let parent_of: HashMap<&str, &str> = rig
            .skeleton
            .bones
            .iter()
            .map(|b| {
                let p = if b.parent < 0 {
                    "-"
                } else {
                    rig.skeleton.bones[b.parent as usize].name.as_str()
                };
                (b.name.as_str(), p)
            })
            .collect();
        assert_eq!(parent_of.len(), TOPOLOGY.len(), "no bone is composed twice");
        for (name, parent) in TOPOLOGY {
            let p = parent_of
                .get(name)
                .unwrap_or_else(|| panic!("canon bone `{name}` is not composed"));
            assert_eq!(*p, parent, "`{name}` parents `{p}`, canon says `{parent}`");
        }

        let pos = world_positions();
        // EXACT mirror symmetry: every left/right pair, the Weapon grips included.
        let mut pairs = 0;
        for (name, _) in TOPOLOGY {
            let twin = if let Some(stem) = name.strip_suffix("_l") {
                format!("{stem}_r")
            } else if name == "Weapon_L" {
                "Weapon_R".to_string()
            } else {
                continue;
            };
            let (l, r) = (pos[name], pos[twin.as_str()]);
            assert!(
                (l.x + r.x).abs() < 1e-4 && (l.y - r.y).abs() < 1e-4 && (l.z - r.z).abs() < 1e-4,
                "`{name}` and `{twin}` are not exact mirrors: {l:?} vs {r:?}"
            );
            pairs += 1;
        }
        assert_eq!(
            pairs, 29,
            "every one of the 29 left/right pairs was checked"
        );

        // Level shoulders — the defect that started this. And a plumb spine.
        assert!((pos["upperarm_l"].z - pos["upperarm_r"].z).abs() < 1e-4);
        assert!(
            (pos["upperarm_l"].z - 0.818 * STATURE).abs() < 1e-3,
            "shoulders at 0.818·H"
        );
        for n in [
            "root", "pelvis", "spine_01", "spine_02", "spine_03", "neck_01", "neck_02", "head",
        ] {
            assert!(
                pos[n].x.abs() < 1e-4 && pos[n].y.abs() < 1e-4,
                "`{n}` off the plumb line"
            );
        }

        // Flat soles: root on the ground, balls riding just above it, ankles low.
        assert_eq!(pos["root"], Vec3::ZERO);
        for n in ["ball_l", "ball_r"] {
            assert!(
                pos[n].z > 0.0 && pos[n].z < 0.02 * STATURE,
                "`{n}` sole not flat: {:?}",
                pos[n]
            );
        }
        assert!(pos["foot_l"].z < 0.05 * STATURE, "ankle rides low");

        // The ruled stature: the eyes (the highest joints) sit just under H.
        let top = pos.values().map(|v| v.z).fold(f32::MIN, f32::max);
        assert!(
            (pos["eye_l"].z - top).abs() < 1e-4,
            "eyes are the highest joints"
        );
        assert!(
            top < STATURE && top > 0.90 * STATURE,
            "stature respected: top {top} vs {STATURE}"
        );

        // The A-pose: the whole arm line droops A_POSE_DEG below horizontal.
        let (s, e, w) = (pos["upperarm_l"], pos["lowerarm_l"], pos["hand_l"]);
        for (a, b) in [(s, e), (e, w)] {
            let d = b - a;
            let ang = (-d.z).atan2(d.x).to_degrees();
            assert!(
                (ang - A_POSE_DEG).abs() < 0.1,
                "arm segment at {ang}°, authored {A_POSE_DEG}°"
            );
        }
        // Twists sit exactly mid-segment.
        assert!((pos["upperarm_twist_01_l"] - (s + e) * 0.5).length() < 1e-4);
        assert!((pos["calf_twist_01_l"] - (pos["calf_l"] + pos["foot_l"]) * 0.5).length() < 1e-4);

        // Every non-root bone has a real segment (no zero-length locals except the
        // deliberate riders on their parents' frames).
        for b in &rig.skeleton.bones {
            if b.parent >= 0 {
                let t = Vec3::new(b.local[12], b.local[13], b.local[14]);
                assert!(t.length() > 0.1, "`{}` collapses onto its parent", b.name);
            }
        }
    }

    /// THE BIT-EXACT GATE (the modular skeleton's first law, 2026-09-07): the humanoid recipe
    /// composes to the PACKAGED `Humanoid` reference (the historic GolemBaseSkeleton) bone for bone —
    /// same world rest positions — so every promoted rig and every baked clip library stands.
    /// Real content; skips without it.
    #[test]
    fn the_humanoid_recipe_composes_to_the_packaged_reference() {
        let path = pattern_dir(Pattern::Humanoid).join("Humanoid.json");
        if !crate::package::file_exists(&path) {
            eprintln!("skipping: no packaged reference");
            return;
        }
        let text = crate::package::read_text(&path).expect("the reference reads");
        let reference: RigFile = serde_json::from_str(&text).expect("the reference parses");
        let mut world = vec![Vec3::ZERO; reference.skeleton.bones.len()];
        let mut expected: HashMap<String, (String, Vec3)> = HashMap::new();
        for (i, b) in reference.skeleton.bones.iter().enumerate() {
            let local = Mat4::from_cols_array(&b.local).w_axis.truncate();
            let parent = if b.parent < 0 {
                world[i] = local;
                "-".to_string()
            } else {
                world[i] = world[b.parent as usize] + local;
                reference.skeleton.bones[b.parent as usize].name.clone()
            };
            expected.insert(b.name.clone(), (parent, world[i]));
        }
        let composed = compose(&SkeletonRecipe::humanoid(), STATURE).expect("composes");
        assert_eq!(composed.len(), expected.len(), "the same bone count");
        for b in &composed {
            let (parent, pos) = expected
                .get(&b.name)
                .unwrap_or_else(|| panic!("`{}` is not in the packaged reference", b.name));
            assert_eq!(&b.parent, parent, "`{}` parents differ", b.name);
            assert!(
                (b.position - *pos).length() < 1e-3,
                "`{}` rests at {:?}, the reference at {:?}",
                b.name,
                b.position,
                pos
            );
        }
    }

    /// A QUADRUPED TRUNK LIES ALONG THE BODY (P3, spec C658F114): the pelvis at the rear, the
    /// spine forward (−Y) and rising to the withers `length·h` ahead, the neck climbing, the head
    /// carried forward above the withers with the muzzle ahead of the eyes; the hip pairs hang
    /// from the socket the leg modules are authored for (the hoofed hip joint high at the rear,
    /// just under the croup) so the feet still meet the ground; the shoulders hang at the
    /// withers; a tail runs back behind the pelvis; a withers mount sits above spine_03. Every
    /// canon name is present, so the bench's tools work unchanged.
    #[test]
    fn a_quadruped_trunk_lies_along_the_body_with_its_head_forward() {
        let mut recipe = reference_recipe(Pattern::Quadruped);
        recipe.trunk.tails = vec![TailKind::Long { bones: 6 }];
        recipe.trunk.mounts = vec![Mount {
            socket: Socket::Withers,
            trunk: TrunkSpec {
                scale: 0.6,
                ..TrunkSpec::default()
            },
        }];
        let h = STATURE;
        let bones = compose(&recipe, h).expect("a quadruped trunk composes");
        let at = |name: &str| {
            bones
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("{name} composed"))
                .position
        };
        let pelvis = at("pelvis");
        let spine_03 = at("spine_03");
        let neck_02 = at("neck_02");
        let head = at("head");
        assert!(
            (pelvis.z - (0.560 + QUAD_CROUP_LIFT) * h).abs() < 1e-3,
            "the croup rides above the hip socket: {}",
            pelvis.z
        );
        assert!(
            (at("thigh_l").z - 0.78 * h).abs() < 0.02 * h && at("thigh_l").z < pelvis.z,
            "the hip joint sits at the rear just under the croup (BAD0D72C): {}",
            at("thigh_l").z
        );
        assert!(
            (spine_03.z - QUAD_WITHERS * h).abs() < 1e-3,
            "the withers stand at the knob: {}",
            spine_03.z
        );
        assert!(
            (pelvis.y - spine_03.y - TrunkSpec::DEFAULT_LENGTH * h).abs() < 1e-3,
            "the withers lie `length·h` forward of the pelvis: {}",
            pelvis.y - spine_03.y
        );
        assert!(
            spine_03.z > pelvis.z && neck_02.z > spine_03.z && head.z > spine_03.z,
            "the spine rises to the withers, the neck climbs, the head is carried above them"
        );
        assert!(
            head.y < neck_02.y && neck_02.y < spine_03.y,
            "forward is −Y all the way to the head"
        );
        assert!(
            at("jaw").y < head.y && at("jaw").z < head.z,
            "the muzzle is ahead and below"
        );
        assert!(at("eye_l").y < head.y && at("eye_l").x > 0.0 && at("eye_r").x < 0.0);
        // The canon limbs hang where a quadruped's do: hind legs at the pelvis, forelegs at
        // the withers.
        assert!((at("thigh_l").y - pelvis.y).abs() < 0.01 * h && at("thigh_l").x > 0.0);
        assert!(
            (at("clavicle_l").y - spine_03.y).abs() < 0.12 * h,
            "the shoulder pair hangs at the withers"
        );
        assert!(
            at("hoof_l").z.abs() < 1e-3 && at("forehoof_l").z.abs() < 1e-3,
            "hooves and forehooves meet the ground: {} {}",
            at("hoof_l").z,
            at("forehoof_l").z
        );
        assert!(
            at("tail_01").y > pelvis.y && at("tail_06").y > at("tail_01").y,
            "the tail runs back"
        );
        assert!(
            at("trunk2_pelvis").z > spine_03.z
                && (at("trunk2_pelvis").y - spine_03.y).abs() < 0.01 * h,
            "the rider trunk mounts above the withers"
        );
        for name in [
            "root",
            "pelvis",
            "spine_01",
            "spine_02",
            "spine_03",
            "neck_01",
            "neck_02",
            "head",
            "jaw",
            "eye_l",
            "eye_r",
            "clavicle_r",
            "hand_r",
            "thigh_r",
            "foot_r",
        ] {
            assert!(bones.iter().any(|b| b.name == name), "{name} is composed");
        }
        assert_eq!(pattern_of(&recipe), Pattern::Quadruped);
        assert!(describe(&recipe).contains("quadruped"));
        // The humanoid is untouched by the branch (the bit-exact gate stands beside this one).
        assert_eq!(compose(&SkeletonRecipe::humanoid(), h).unwrap().len(), 67);
    }

    /// THE HOOFED LIMBS (P3b; the hind leg authored to Aaron's anatomy, BAD0D72C): the
    /// unguligrade hind leg runs from a hip high at the rear just under the croup (0.78h) FORWARD
    /// to the stifle (0.56h), BACK to the hock (0.38h), straight down the cannon to the fetlock
    /// (0.13h) and the hoof on the ground, no twists — femur, tibia, cannon, pastern ≈ 0.26,
    /// 0.27, 0.25, 0.13h; the ungulate foreleg runs from a scapula high at the withers through
    /// shoulder, elbow, carpus and fetlock (0.12h) to a forehoof on the ground; both mirror
    /// across X; a
    /// hoofed BIPED (Unguligrade pattern) hangs the same leg from its own hip under a plumb trunk
    /// and still reaches the ground.
    #[test]
    fn hoofed_limbs_run_from_the_trunk_to_the_ground() {
        let h = STATURE;
        let bones = compose(&reference_recipe(Pattern::Quadruped), h).expect("composes");
        let at = |name: &str| {
            bones
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("{name} composed"))
                .position
        };
        let has = |name: &str| bones.iter().any(|b| b.name == name);
        // Hind leg.
        let (hip, stifle, hock, fetlock, hoof) = (
            at("thigh_l"),
            at("calf_l"),
            at("foot_l"),
            at("ball_l"),
            at("hoof_l"),
        );
        assert!(hip.z > stifle.z && stifle.z > hock.z && hock.z > fetlock.z && fetlock.z > hoof.z);
        assert!(stifle.y < hip.y, "the stifle is forward of the hip");
        assert!(hock.y > stifle.y, "the hock is back of the stifle");
        assert!(hoof.z.abs() < 1e-3, "the hoof is on the ground");
        let croup = at("pelvis");
        assert!(
            hip.z < croup.z && croup.z - hip.z < 0.12 * h && (hip.y - croup.y).abs() < 0.01 * h,
            "the hip joint is at the rear, just under the croup: {hip} under {croup}"
        );
        for (joint, p, frac) in [
            ("hip", hip, 0.78),
            ("stifle", stifle, 0.56),
            ("hock", hock, 0.38),
            ("fetlock", fetlock, 0.13),
        ] {
            assert!(
                (p.z - frac * h).abs() < 1e-3 * h,
                "the {joint} at {frac}h: {:.3}h",
                p.z / h
            );
        }
        let bones_of = |j: [Vec3; 5]| [0, 1, 2, 3].map(|i| j[i].distance(j[i + 1]));
        let hind = bones_of([hip, stifle, hock, fetlock, hoof]);
        for (bone, len, frac) in [
            ("femur", hind[0], 0.26),
            ("tibia", hind[1], 0.27),
            ("cannon", hind[2], 0.25),
            ("pastern", hind[3], 0.13),
        ] {
            assert!(
                (len - frac * h).abs() < 0.01 * h,
                "the {bone} is ≈ {frac}h: {:.3}h",
                len / h
            );
        }
        assert!(
            (hock.y - fetlock.y).abs() < (hock.z - fetlock.z) * 0.1,
            "the cannon runs straight down"
        );

        assert!(
            !has("thigh_twist_01_l") && !has("calf_twist_01_l"),
            "no twists on a hoofed leg"
        );
        // Foreleg.
        let (scap, shoulder, elbow, carpus, fore_fetlock, forehoof) = (
            at("clavicle_l"),
            at("upperarm_l"),
            at("lowerarm_l"),
            at("hand_l"),
            at("foredigit_l"),
            at("forehoof_l"),
        );
        let withers = at("spine_03");
        assert!(
            (scap.z - withers.z).abs() < 0.10 * h,
            "the scapula rides high at the withers"
        );
        assert!(
            scap.z > shoulder.z
                && shoulder.z > elbow.z
                && elbow.z > carpus.z
                && carpus.z > fore_fetlock.z
                && fore_fetlock.z > forehoof.z
        );
        assert!(
            shoulder.y < scap.y,
            "the shoulder joint is forward of the scapula"
        );
        assert!(forehoof.z.abs() < 1e-3, "the forehoof is on the ground");
        assert!(
            (fore_fetlock.z - 0.12 * h).abs() < 1e-3 * h,
            "the fore fetlock at 0.12h: {fore_fetlock}"
        );
        assert!(
            !has("middle_01_l") && !has("Weapon_L"),
            "no fingers, no grip on a foreleg"
        );
        for name in ["hoof", "forehoof", "foredigit", "thigh", "clavicle"] {
            let (l, r) = (at(&format!("{name}_l")), at(&format!("{name}_r")));
            assert!(
                (l.x + r.x).abs() < 1e-3 && (l.y - r.y).abs() < 1e-3 && (l.z - r.z).abs() < 1e-3,
                "{name} mirrors"
            );
        }
        // The hoofed biped: the same leg, hung from the biped hip under its pelvis.
        let biped =
            compose(&reference_recipe(Pattern::Unguligrade), h).expect("a hoofed biped composes");
        let bat = |name: &str| {
            biped
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("the biped's {name}"))
                .position
        };
        let joints = ["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"].map(bat);
        assert!(
            joints[4].z.abs() < 1e-3,
            "the biped's hoof is on the ground"
        );
        assert!(
            (joints[0].z - (bat("pelvis").z - 0.03 * h)).abs() < 1e-3,
            "the biped's hip is every biped leg's, under its pelvis: {}",
            joints[0]
        );
        assert!(
            joints[1].y < joints[0].y && joints[2].y > joints[1].y,
            "the same zigzag: stifle forward, hock back"
        );
        let scale = bones_of(joints)[0] / hind[0];
        for (b, q) in bones_of(joints).iter().zip(hind) {
            assert!(
                (b / q - scale).abs() < 1e-3,
                "the biped's bones keep the leg's proportions: {b} vs {q}"
            );
        }
        assert!(
            biped.iter().any(|b| b.name == "middle_01_l"),
            "with its humanoid arms"
        );
    }

    /// THE WINGS AND THE BIRD LEG (P4): a bird's wing runs shoulder → elbow → wrist → tip
    /// SPREAD to 0.8h out along X with its two feather groups hanging back off the forearm and
    /// the hand; a bat's wing ends in a forward thumb and four three-segment digits fanning from
    /// the wrist, the first along the span and the fourth trailing back; a bird's leg drops from
    /// a short thigh to a forward knee, a raised backward ankle and a toe base on the ground with
    /// three forward toes and a hind toe; both fliers mirror and their references compose.
    #[test]
    fn wings_spread_and_bird_legs_perch_on_the_ground() {
        let h = STATURE;
        let bird = compose(&reference_recipe(Pattern::Bird), h).expect("a bird composes");
        let at = |bones: &[AuthoredBone], name: &str| {
            bones
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("{name} composed"))
                .position
        };
        let shoulder = at(&bird, "upperarm_l");
        let elbow = at(&bird, "lowerarm_l");
        let wrist = at(&bird, "hand_l");
        let tip = at(&bird, "wing_tip_l");
        assert!(
            shoulder.x < elbow.x && elbow.x < wrist.x && wrist.x < tip.x,
            "out along the span"
        );
        assert!(
            (tip.x - 0.80 * h).abs() < 0.01 * h,
            "spread wide: {}",
            tip.x
        );
        assert!(
            at(&bird, "wing_feathers_01_l").y > elbow.y
                && at(&bird, "wing_feathers_02_l").y > wrist.y,
            "the feather groups hang back"
        );
        assert!(
            (at(&bird, "wing_tip_l").x + at(&bird, "wing_tip_r").x).abs() < 1e-3,
            "mirrors"
        );
        let (thigh, knee, ankle, ball) = (
            at(&bird, "thigh_l"),
            at(&bird, "calf_l"),
            at(&bird, "foot_l"),
            at(&bird, "ball_l"),
        );
        assert!(knee.y < thigh.y, "the knee is forward");
        assert!(
            ankle.y > knee.y && (ankle.z - 0.25 * h).abs() < 1e-3,
            "the ankle is back and raised to the knob"
        );
        assert!(ball.z.abs() < 1e-3, "the toe base is on the ground");
        for toe in ["toe_01_l", "toe_02_l", "toe_03_l", "hallux_l"] {
            assert!(at(&bird, toe).z.abs() < 1e-3, "{toe} is on the ground");
        }
        assert!(
            at(&bird, "toe_01_l").y < ball.y && at(&bird, "hallux_l").y > ball.y,
            "toes forward, the hallux back"
        );
        assert!(
            !bird.iter().any(|b| b.name == "middle_01_l"),
            "no fingers on a wing"
        );
        assert_eq!(pattern_of(&reference_recipe(Pattern::Bird)), Pattern::Bird);

        let bat = compose(&reference_recipe(Pattern::Bat), h).expect("a bat composes");
        let wrist = at(&bat, "hand_l");
        assert!(
            at(&bat, "wing_thumb_l").y < wrist.y,
            "the thumb claws forward"
        );
        let d1 = at(&bat, "wing_digit_1_03_l");
        let d4 = at(&bat, "wing_digit_4_03_l");
        assert!(
            d1.x > d4.x && d4.y > d1.y,
            "digit 1 along the span, digit 4 trailing back"
        );
        for d in 1..=4 {
            let a = at(&bat, &format!("wing_digit_{d}_01_l"));
            let b = at(&bat, &format!("wing_digit_{d}_02_l"));
            let c = at(&bat, &format!("wing_digit_{d}_03_l"));
            assert!(
                (b - a).length() > 0.0 && ((c - b) - (b - a)).length() < 1e-3,
                "three even segments"
            );
        }
        assert!((at(&bat, "wing_digit_1_03_l").x + at(&bat, "wing_digit_1_03_r").x).abs() < 1e-3);
        assert_eq!(pattern_of(&reference_recipe(Pattern::Bat)), Pattern::Bat);
        assert!(describe(&reference_recipe(Pattern::Bird)).contains("bird"));
    }

    /// DOUBLE SHOULDERS AND DRAGONS (Aaron 2026-09-08): a winged gargoyle carries humanoid arms
    /// and a second, bat-wing pair (`arm2_…`) mounted behind and above them; a winged headless
    /// the same with no head; a naga a humanoid upper body on a serpent tail and no legs; the
    /// three dragon layouts — sprawler (forelegs + wings), wyvern (wings as forelegs) and four
    /// legs + wings — compose on the quadruped trunk with the wing pair behind the forelegs,
    /// and every one is the pattern its first pair or trunk decides.
    #[test]
    fn winged_bipeds_the_naga_and_the_dragons_compose() {
        let h = STATURE;
        let at = |bones: &[AuthoredBone], name: &str| {
            bones
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("{name} composed"))
                .position
        };
        let mut gargoyle = SkeletonRecipe::humanoid();
        gargoyle.trunk.arms = vec![ArmKind::Humanoid, ArmKind::Bat];
        gargoyle.trunk.legs = vec![LegKind::Digitigrade { heel: 0.15 }];
        gargoyle.trunk.tails = vec![TailKind::Short];
        let g = compose(&gargoyle, h).expect("a winged gargoyle composes");
        assert!(
            g.iter().any(|b| b.name == "hand_l")
                && g.iter().any(|b| b.name == "arm2_wing_digit_1_03_l")
        );
        let (arm, wing) = (at(&g, "clavicle_l"), at(&g, "arm2_clavicle_l"));
        assert!(
            wing.y > arm.y && wing.z > at(&g, "spine_02").z,
            "the wings mount behind the arms, high on the back"
        );
        assert_eq!(
            pattern_of(&gargoyle),
            Pattern::Digitigrade,
            "the first pair decides"
        );

        let mut headless = gargoyle.clone();
        headless.trunk.head = false;
        headless.trunk.legs = vec![LegKind::Plantigrade];
        let hl = compose(&headless, h).expect("a winged headless composes");
        assert!(
            !hl.iter().any(|b| b.name == "head")
                && hl.iter().any(|b| b.name == "arm2_wing_thumb_r")
        );
        assert_eq!(pattern_of(&headless), Pattern::Humanoid);

        let mut naga = SkeletonRecipe::humanoid();
        naga.trunk.legs.clear();
        naga.trunk.tails = vec![TailKind::Long { bones: 12 }];
        let n = compose(&naga, h).expect("a naga composes");
        assert!(!n.iter().any(|b| b.name == "thigh_l") && n.iter().any(|b| b.name == "tail_12"));
        assert_eq!(
            pattern_of(&naga),
            Pattern::Humanoid,
            "no legs is a humanoid basis"
        );

        let dragon = |arms: Vec<ArmKind>, legs: LegKind, length: f32| {
            let mut r = SkeletonRecipe::humanoid();
            r.trunk.orientation = Orientation::Quadruped;
            r.trunk.arms = arms;
            r.trunk.legs = vec![legs];
            r.trunk.tails = vec![TailKind::Long { bones: 10 }];
            r.trunk.length = length;
            r
        };
        let sprawler = dragon(
            vec![ArmKind::Ungulate, ArmKind::Bat],
            LegKind::Plantigrade,
            0.9,
        );
        let wyvern = dragon(vec![ArmKind::Bat], LegKind::Digitigrade { heel: 0.25 }, 0.6);
        let four = dragon(
            vec![ArmKind::Ungulate, ArmKind::Bat],
            LegKind::Digitigrade { heel: 0.25 },
            0.7,
        );
        for (name, r) in [
            ("sprawler", &sprawler),
            ("wyvern", &wyvern),
            ("four legs + wings", &four),
        ] {
            let bones = compose(r, h).unwrap_or_else(|e| panic!("the {name} dragon composes: {e}"));
            assert_eq!(
                pattern_of(r),
                Pattern::Quadruped,
                "{name}: the trunk decides"
            );
            assert!(
                bones.iter().any(|b| b.name == "tail_10"),
                "{name}: a long tail"
            );
            let wings = if r.trunk.arms.len() == 2 { "arm2_" } else { "" };
            assert!(
                bones
                    .iter()
                    .any(|b| b.name == format!("{wings}wing_digit_1_03_l")),
                "{name}: wings"
            );
            if r.trunk.arms.len() == 2 {
                assert!(
                    bones.iter().any(|b| b.name == "forehoof_l"),
                    "{name}: forelegs"
                );
                assert!(
                    at(&bones, "arm2_clavicle_l").y > at(&bones, "clavicle_l").y,
                    "{name}: the wings mount behind the forelegs"
                );
            }
        }
    }

    /// THE UNBOUNDED-TREE GATE (Aaron: "seven torsos chained together with fourteen arms"):
    /// seven trunks chained withers-to-pelvis, two arm pairs each, legs on the base only,
    /// compose with unique names, parents before children, and the numbered per-kind
    /// prefixes exactly where the naming rule puts them.
    #[test]
    fn a_seven_trunk_boss_composes_without_limits() {
        let mut trunk = TrunkSpec {
            arms: vec![ArmKind::Humanoid, ArmKind::Humanoid],
            legs: Vec::new(),
            scale: 0.6,
            ..TrunkSpec::default()
        };
        for _ in 0..6 {
            let child = trunk.clone();
            trunk = TrunkSpec {
                arms: vec![ArmKind::Humanoid, ArmKind::Humanoid],
                legs: Vec::new(),
                scale: 0.6,
                mounts: vec![flicker_skeletal::format::Mount {
                    socket: Socket::Withers,
                    trunk: child,
                }],
                ..TrunkSpec::default()
            };
        }
        trunk.legs = vec![LegKind::Plantigrade];
        trunk.scale = 1.0;
        let recipe = SkeletonRecipe {
            trunk,
            preset: None,
        };
        let bones = compose(&recipe, STATURE).expect("the boss composes");
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (i, b) in bones.iter().enumerate() {
            assert!(
                seen.insert(&b.name, i).is_none(),
                "`{}` composed twice",
                b.name
            );
            if b.parent != "-" {
                assert!(
                    seen[b.parent.as_str()] < i,
                    "`{}` is listed before its parent `{}`",
                    b.name,
                    b.parent
                );
            }
        }
        for name in [
            "pelvis",
            "trunk2_pelvis",
            "trunk7_spine_03",
            "clavicle_l",
            "arm2_clavicle_l",
            "arm14_hand_l",
            "arm14_Weapon_R",
            "head",
            "head7_eye_r",
            "thigh_l",
        ] {
            assert!(seen.contains_key(name), "the boss lacks `{name}`");
        }
        assert!(
            !seen.contains_key("leg2_thigh_l"),
            "only the base trunk has legs"
        );
        // 1 root + 7 × (6 trunk + 4 head) + 14 pairs × 2 sides × 22 arm bones (clavicle,
        // upperarm + twist, lowerarm + twist, hand, Weapon, 5 × 3 fingers) + 2 × 6 leg bones.
        assert_eq!(bones.len(), 1 + 7 * 10 + 14 * 2 * 22 + 12);
        assert!(
            seen["trunk7_spine_03"] > seen["trunk6_spine_03"],
            "trunks number in depth-first order"
        );
    }

    /// Aaron's LION-CENTAUR BOSS (00D9CD1F) — the shipped `Quadruped/LionCentaur` preset: a
    /// headless Quadruped lion (ungulate paws, digitigrade hind legs, a long tail) carrying a
    /// Biped rider with Humanoid arms and Bird wings, on whichever socket.
    fn lion_centaur(socket: Socket) -> SkeletonRecipe {
        SkeletonRecipe {
            trunk: TrunkSpec {
                orientation: Orientation::Quadruped,
                head: false,
                proboscis: 0,
                arms: vec![ArmKind::Ungulate],
                legs: vec![LegKind::Digitigrade { heel: 0.25 }],
                tails: vec![TailKind::Long { bones: 8 }],
                mounts: vec![Mount {
                    socket,
                    trunk: TrunkSpec {
                        arms: vec![ArmKind::Humanoid, ArmKind::Bird],
                        legs: Vec::new(),
                        scale: 0.75,
                        ..TrunkSpec::default()
                    },
                }],
                scale: 1.0,
                length: 0.65,
            },
            preset: None,
        }
    }

    /// THE NECK SOCKET (Aaron's ruling 2026-09-12): a `Neck` mount REPLACES a headless trunk's
    /// neck — the lion's chain stops at `spine_03`, no `neck_01`/`neck_02` is composed, and the
    /// rider's own `pelvis` is the SUB-PELVIS standing exactly where `neck_01` would have,
    /// parented to `spine_03`. The rider keeps its own neck and head. 107 bones on the withers,
    /// 105 here — the two neck bones are what the sub-pelvis replaced.
    #[test]
    fn a_neck_mount_replaces_a_headless_trunks_neck_with_the_riders_pelvis() {
        let withers =
            compose(&lion_centaur(Socket::Withers), STATURE).expect("the withers boss composes");
        let bones = compose(&lion_centaur(Socket::Neck), STATURE).expect("the neck boss composes");
        let pos = |bones: &[AuthoredBone], name: &str| {
            bones
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("`{name}` composed"))
                .position
        };
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (i, b) in bones.iter().enumerate() {
            assert!(
                seen.insert(&b.name, i).is_none(),
                "`{}` composed twice",
                b.name
            );
            if b.parent != "-" {
                assert!(
                    seen[b.parent.as_str()] < i,
                    "`{}` is listed before its parent `{}`",
                    b.name,
                    b.parent
                );
            }
        }
        assert!(
            !seen.contains_key("neck_01") && !seen.contains_key("neck_02"),
            "the lion's neck is GONE — the rider's pelvis took its place"
        );
        assert!(
            seen.contains_key("trunk2_neck_01") && seen.contains_key("head"),
            "the rider keeps its own neck and head"
        );
        let sub_pelvis = bones
            .iter()
            .find(|b| b.name == "trunk2_pelvis")
            .expect("the sub-pelvis composed");
        assert_eq!(
            sub_pelvis.parent, "spine_03",
            "the sub-pelvis hangs off the withers bone"
        );
        assert!(
            (sub_pelvis.position - pos(&withers, "neck_01")).length() < 1e-4,
            "the sub-pelvis stands at the neck root: {:?} vs {:?}",
            sub_pelvis.position,
            pos(&withers, "neck_01")
        );
        assert_eq!(
            withers.len(),
            107,
            "a WITHERS mount keeps the lion's two neck bones"
        );
        assert_eq!(bones.len(), 105);
        assert!(
            describe(&lion_centaur(Socket::Neck)).contains("neck mount"),
            "describe names the neck mount: {}",
            describe(&lion_centaur(Socket::Neck))
        );
        // The SHIPPED preset is exactly this recipe (real content; skips without it).
        let dir = skeletons_dir();
        if dir.is_dir() {
            let preset = load_presets(&dir)
                .into_iter()
                .find(|p| p.name == "LionCentaur")
                .expect("the LionCentaur preset ships");
            assert_eq!(preset.recipe.trunk, lion_centaur(Socket::Neck).trunk);
            assert_eq!(preset.recipe.pattern(), Pattern::Quadruped);
        }
    }

    /// THE MARKERS RAIL IS A WALK OF THE COMPOSED SKELETON, not a list of joints picked per
    /// module (Aaron in the window, 2026-09-21: the rail walked a hoofed leg hip → stifle → hock →
    /// HOOF and SKIPPED THE FETLOCK, because the old rule hand-picked three joints a leg "needs an
    /// eye on" — anatomy sneaking back into the one place the spec forbids it, rule 513E5F78).
    ///
    /// Every bone of the composed rest is prompted, parents before children, each chain in bone
    /// order, with exactly TWO structural exclusions and no name picks at all: `*_twist_*` helper
    /// bones, and FAN CHILDREN — a bone that is one of three or more children of its parent IN ITS
    /// OWN MODULE (a hand's fingers, a foot's toes, a bird's feather groups, a bat's digits, a
    /// head's jaw and eyes) together with everything under it, which ride the INFER button.
    /// Counting siblings PER MODULE is what keeps a pelvis's four children (a spine, two thighs
    /// and a tail) from reading as a fan.
    ///
    /// ORDER: the root trunk (pelvis, spine, neck), its head, the legs a body stands on, then its
    /// arms, then its tails, then a mounted trunk's whole list — left before right within a
    /// module, root-most first (incident D81498B7).
    #[test]
    fn the_markers_rail_walks_every_joint_of_the_composed_skeleton() {
        // A humanoid arm is clavicle → upperarm → lowerarm → hand: no fingers, no twists.
        assert_eq!(
            markers_for(&SkeletonRecipe::humanoid()),
            [
                "pelvis",
                "spine_01",
                "spine_02",
                "spine_03",
                "neck_01",
                "neck_02",
                "head",
                "thigh_l",
                "calf_l",
                "foot_l",
                "ball_l",
                "thigh_r",
                "calf_r",
                "foot_r",
                "ball_r",
                "clavicle_l",
                "upperarm_l",
                "lowerarm_l",
                "hand_l",
                "clavicle_r",
                "upperarm_r",
                "lowerarm_r",
                "hand_r",
            ]
        );

        // A HOOFED HIND LEG is thigh → calf → foot → BALL → hoof, the fetlock among them.
        let mut horse = reference_recipe(Pattern::Quadruped);
        horse.trunk.tails = vec![TailKind::Long { bones: 8 }];
        assert_eq!(
            markers_for(&horse),
            [
                "pelvis",
                "spine_01",
                "spine_02",
                "spine_03",
                "neck_01",
                "neck_02",
                "head",
                "thigh_l",
                "calf_l",
                "foot_l",
                "ball_l",
                "hoof_l",
                "thigh_r",
                "calf_r",
                "foot_r",
                "ball_r",
                "hoof_r",
                "clavicle_l",
                "upperarm_l",
                "lowerarm_l",
                "hand_l",
                "foredigit_l",
                "forehoof_l",
                "clavicle_r",
                "upperarm_r",
                "lowerarm_r",
                "hand_r",
                "foredigit_r",
                "forehoof_r",
                "tail_01",
                "tail_02",
                "tail_03",
                "tail_04",
                "tail_05",
                "tail_06",
                "tail_07",
                "tail_08",
            ]
        );

        // The boss: a headless quadruped lion (ungulate forepaws, digitigrade hind legs, a long
        // tail) carrying a biped rider with humanoid arms AND bird wings. The lion's whole walk
        // comes first and the rider's follows it, the rider's own `trunk2_pelvis` opening its
        // half; the rider's two arm pairs are the SECOND and THIRD arms composed, hence `arm2_`
        // and `arm3_`. The lion is HEADLESS, so the one `head` in the list is the rider's.
        assert_eq!(
            markers_for(&lion_centaur(Socket::Neck)),
            [
                "pelvis",
                "spine_01",
                "spine_02",
                "spine_03",
                "thigh_l",
                "calf_l",
                "foot_l",
                "ball_l",
                "thigh_r",
                "calf_r",
                "foot_r",
                "ball_r",
                "clavicle_l",
                "upperarm_l",
                "lowerarm_l",
                "hand_l",
                "foredigit_l",
                "forehoof_l",
                "clavicle_r",
                "upperarm_r",
                "lowerarm_r",
                "hand_r",
                "foredigit_r",
                "forehoof_r",
                "tail_01",
                "tail_02",
                "tail_03",
                "tail_04",
                "tail_05",
                "tail_06",
                "tail_07",
                "tail_08",
                "trunk2_pelvis",
                "trunk2_spine_01",
                "trunk2_spine_02",
                "trunk2_spine_03",
                "trunk2_neck_01",
                "trunk2_neck_02",
                "head",
                "arm2_clavicle_l",
                "arm2_upperarm_l",
                "arm2_lowerarm_l",
                "arm2_hand_l",
                "arm2_clavicle_r",
                "arm2_upperarm_r",
                "arm2_lowerarm_r",
                "arm2_hand_r",
                "arm3_clavicle_l",
                "arm3_upperarm_l",
                "arm3_lowerarm_l",
                "arm3_hand_l",
                "arm3_wing_tip_l",
                "arm3_wing_feathers_01_l",
                "arm3_wing_feathers_02_l",
                "arm3_clavicle_r",
                "arm3_upperarm_r",
                "arm3_lowerarm_r",
                "arm3_hand_r",
                "arm3_wing_tip_r",
                "arm3_wing_feathers_01_r",
                "arm3_wing_feathers_02_r",
            ]
        );
    }

    /// AND EVERY PROMPTED NAME IS A REAL BONE — the rail names joints the human must select and
    /// frame, so a name that resolved to nothing would be a dead prompt (4BB12A75). Checked across
    /// every shipped pattern's reference recipe, a tailed and a wing-carrying variant, and the
    /// two-trunk boss, against the bones `compose` actually emits.
    #[test]
    fn every_prompted_marker_is_a_bone_the_recipe_composes() {
        let mut bat = SkeletonRecipe::humanoid();
        bat.trunk.arms = vec![ArmKind::Bat];
        let mut tailed = SkeletonRecipe::humanoid();
        tailed.trunk.tails = vec![TailKind::ShortHair { bones: 4 }, TailKind::Short];
        let mut recipes: Vec<SkeletonRecipe> =
            Pattern::ALL.iter().map(|p| reference_recipe(*p)).collect();
        recipes.push(bat);
        recipes.push(tailed);
        recipes.push(lion_centaur(Socket::Neck));
        recipes.push(lion_centaur(Socket::Withers));
        for recipe in &recipes {
            let bones = compose(recipe, STATURE).expect("the recipe composes");
            let markers = markers_for(recipe);
            assert!(!markers.is_empty(), "every body prompts for something");
            for name in &markers {
                assert!(
                    bones.iter().any(|b| &b.name == name),
                    "the rail prompts for `{name}`, which {:?} does not compose",
                    recipe.pattern()
                );
            }
            let mut seen = std::collections::HashSet::new();
            for name in &markers {
                assert!(seen.insert(name.clone()), "`{name}` is prompted twice");
            }
        }
    }

    /// A `Neck` mount on a HEADED trunk — or two on one trunk — FAILS LOUD: a headed trunk has a
    /// neck of its own (mount on `Withers`), and a trunk has only one neck to give up.
    #[test]
    fn a_neck_mount_on_a_headed_trunk_fails_loud() {
        let mut headed = lion_centaur(Socket::Neck);
        headed.trunk.head = true;
        let e = compose(&headed, STATURE)
            .expect_err("a headed trunk refuses a neck mount")
            .to_string();
        assert!(e.contains("HEADLESS"), "the error says why: {e}");
        let mut twice = lion_centaur(Socket::Neck);
        let again = twice.trunk.mounts[0].clone();
        twice.trunk.mounts.push(again);
        let e = compose(&twice, STATURE)
            .expect_err("one neck per trunk")
            .to_string();
        assert!(e.contains("ONE neck"), "the error says why: {e}");
    }

    /// DIGITIGRADE LEGS (Aaron: the cat and dog peoples, catman, werewolf, lizardman, heeled
    /// shoes, the golem's heel): the ankle rises to the heel knob, the toe stays on the ground,
    /// the thigh and calf keep the plantigrade LENGTHS, and the knee lands forward of the
    /// hip→ankle line — the Z-shaped stance. Every other bone is the humanoid canon.
    #[test]
    fn digitigrade_legs_raise_the_heel_and_keep_their_lengths() {
        let mut recipe = SkeletonRecipe::humanoid();
        recipe.trunk.legs = vec![LegKind::Digitigrade { heel: 0.15 }];
        let pos: HashMap<String, Vec3> = compose(&recipe, STATURE)
            .expect("composes")
            .into_iter()
            .map(|b| (b.name, b.position))
            .collect();
        let canon = world_positions();
        assert_eq!(pos.len(), canon.len(), "the same bones as the humanoid");
        assert!(
            (pos["foot_l"].z - 0.15 * STATURE).abs() < 1e-3,
            "ankle at the heel knob"
        );
        assert!(
            (pos["ball_l"].z - canon["ball_l"].z).abs() < 1e-3,
            "toe on the ground"
        );
        let len = |a: &str, b: &str, p: &HashMap<String, Vec3>| p[a].distance(p[b]);
        assert!((len("thigh_l", "calf_l", &pos) - len("thigh_l", "calf_l", &canon)).abs() < 1e-2);
        assert!((len("calf_l", "foot_l", &pos) - len("calf_l", "foot_l", &canon)).abs() < 1e-2);
        assert!(
            pos["calf_l"].y < pos["foot_l"].y,
            "the knee sits forward of the ankle"
        );
        assert!(
            pos["foot_l"].y > canon["foot_l"].y,
            "the ankle sits back behind the hip line"
        );
        for n in ["pelvis", "spine_03", "head", "hand_l", "clavicle_r"] {
            assert!((pos[n] - canon[n]).length() < 1e-4, "`{n}` is untouched");
        }
        let off = SkeletonRecipe {
            trunk: TrunkSpec {
                legs: vec![LegKind::Digitigrade { heel: 0.9 }],
                ..TrunkSpec::default()
            },
            preset: None,
        };
        assert!(
            compose(&off, STATURE).is_err(),
            "a heel off the body fails loud"
        );
    }

    /// HANGING ARMS (Aaron 2026-09-07, measured off his hand-rigged lizardman): the same bones
    /// as the humanoid arm, the chain hanging at the sides — upper arm and forearm DIRECTIONS
    /// within 2° of the measured rig, wrist below elbow below shoulder, every finger below the
    /// wrist, mirror-symmetric, every other bone the canon.
    #[test]
    fn hanging_arms_carry_the_lizardman_pattern() {
        let mut recipe = SkeletonRecipe::humanoid();
        recipe.trunk.arms = vec![ArmKind::Hanging];
        let pos: HashMap<String, Vec3> = compose(&recipe, STATURE)
            .expect("composes")
            .into_iter()
            .map(|b| (b.name, b.position))
            .collect();
        let canon = world_positions();
        assert_eq!(pos.len(), canon.len(), "the same bones as the humanoid");
        let dir = |a: &str, b: &str| (pos[b] - pos[a]).normalize();
        let upper = dir("upperarm_l", "lowerarm_l");
        let fore = dir("lowerarm_l", "hand_l");
        let deg = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(
            deg(upper, Vec3::new(0.33, 0.13, -0.94).normalize()) < 2.0,
            "upper arm {upper}"
        );
        assert!(
            deg(fore, Vec3::new(0.29, -0.09, -0.95).normalize()) < 2.0,
            "forearm {fore}"
        );
        assert!(pos["hand_l"].z < pos["lowerarm_l"].z && pos["lowerarm_l"].z < pos["upperarm_l"].z);
        for f in ["index", "middle", "ring", "pinky", "thumb"] {
            for i in 1..=3 {
                let j = pos[&format!("{f}_0{i}_l")];
                assert!(j.z < pos["hand_l"].z, "{f}_0{i} hangs below the wrist: {j}");
            }
        }
        assert!(
            (pos["middle_03_l"].z - pos["middle_01_l"].z).abs() > 0.03 * STATURE,
            "the fingers have length"
        );
        for (l, r) in [
            ("hand_l", "hand_r"),
            ("lowerarm_l", "lowerarm_r"),
            ("index_02_l", "index_02_r"),
        ] {
            let m = Vec3::new(-pos[r].x, pos[r].y, pos[r].z);
            assert!((pos[l] - m).length() < 1e-4, "{l} mirrors {r}");
        }
        for n in ["pelvis", "spine_03", "head", "thigh_l", "foot_r"] {
            assert!((pos[n] - canon[n]).length() < 1e-4, "`{n}` is untouched");
        }
    }

    /// TOE-WALKER LEGS (Aaron 2026-09-07, the numbers measured off his hand-rigged lizardman):
    /// the heel high and set back, a shorter thigh and shin meeting at a mildly bent knee, a long
    /// foot to a ball under the hip — every bone DIRECTION within 2° of the measured leg, the
    /// ball on the ground, every other bone the canon; a leg that cannot reach its heel fails loud.
    #[test]
    fn toe_walker_legs_carry_the_long_foot_pattern() {
        let kind = LegKind::ToeWalker {
            heel: 0.15,
            setback: 0.075,
            toe: 0.0,
            thigh: 0.19,
            calf: 0.20,
        };
        let mut recipe = SkeletonRecipe::humanoid();
        recipe.trunk.legs = vec![kind];
        let pos: HashMap<String, Vec3> = compose(&recipe, STATURE)
            .expect("composes")
            .into_iter()
            .map(|b| (b.name, b.position))
            .collect();
        let canon = world_positions();
        assert_eq!(pos.len(), canon.len(), "the same bones as the humanoid");
        // Bone directions from straight down, positive = BACK (+Y): measured 2.8 / 17.4 / −30.4.
        let dir = |a: &str, b: &str| {
            let v = pos[b] - pos[a];
            v.y.atan2(-v.z).to_degrees()
        };
        assert!(
            (dir("thigh_l", "calf_l") - 4.3).abs() < 1.0,
            "thigh {}",
            dir("thigh_l", "calf_l")
        );
        assert!(
            (dir("calf_l", "foot_l") - 17.7).abs() < 1.0,
            "shin {}",
            dir("calf_l", "foot_l")
        );
        assert!(
            (dir("foot_l", "ball_l") + 28.5).abs() < 1.0,
            "foot {}",
            dir("foot_l", "ball_l")
        );
        assert!(
            (pos["foot_l"].z - 0.15 * STATURE).abs() < 1e-3,
            "heel at 0.15"
        );
        assert!(
            (pos["foot_l"].y - 0.075 * STATURE).abs() < 1e-3,
            "heel set back 0.075"
        );
        assert!(
            (pos["ball_l"].z - canon["ball_l"].z).abs() < 1e-3 && pos["ball_l"].y.abs() < 1e-3,
            "the ball on the ground under the hip: {}",
            pos["ball_l"]
        );
        let len = |a: &str, b: &str| pos[a].distance(pos[b]);
        assert!((len("thigh_l", "calf_l") - 0.19 * STATURE).abs() < 1e-2);
        assert!((len("calf_l", "foot_l") - 0.20 * STATURE).abs() < 1e-2);
        for n in ["pelvis", "spine_03", "head", "hand_l", "clavicle_r"] {
            assert!((pos[n] - canon[n]).length() < 1e-4, "`{n}` is untouched");
        }
        assert!(
            (pos["thigh_l"].x + pos["thigh_r"].x).abs() < 1e-4
                && (pos["ball_l"] - Vec3::new(-pos["ball_r"].x, pos["ball_r"].y, pos["ball_r"].z))
                    .length()
                    < 1e-4,
            "mirror symmetric"
        );
        let short = SkeletonRecipe {
            trunk: TrunkSpec {
                legs: vec![LegKind::ToeWalker {
                    heel: 0.05,
                    setback: 0.30,
                    toe: 0.0,
                    thigh: 0.10,
                    calf: 0.10,
                }],
                ..TrunkSpec::default()
            },
            preset: None,
        };
        assert!(
            compose(&short, STATURE).is_err(),
            "a leg that cannot reach its heel fails loud"
        );
    }

    /// TAILS: a long tail chains its bones off the pelvis, a hair tail hangs its physics chain
    /// off its one bone, a second tail numbers itself `tail2_…`, and a headless trunk has no
    /// head bones at all.
    /// A PROBOSCIS (ruling 7881216F) chains off the head's front and hangs down from there; a
    /// second instance numbers its bones; the rail prompts it right after the head; a headless
    /// trunk composes none however many it asks for; and the describer names it.
    #[test]
    fn a_proboscis_chains_off_the_head_and_is_prompted_after_it() {
        let mut recipe = reference_recipe(Pattern::Quadruped);
        recipe.trunk.proboscis = 8;
        let bones = compose(&recipe, STATURE).expect("composes");
        let by_name: HashMap<&str, &AuthoredBone> =
            bones.iter().map(|b| (b.name.as_str(), b)).collect();
        assert_eq!(by_name["proboscis_01"].parent, "head");
        for k in 2..=8 {
            assert_eq!(
                by_name[format!("proboscis_{k:02}").as_str()].parent,
                format!("proboscis_{:02}", k - 1)
            );
        }
        let head = by_name["head"].position;
        let root = by_name["proboscis_01"].position;
        let tip = by_name["proboscis_08"].position;
        assert!(
            root.y < head.y && root.z < head.z,
            "the root hangs ahead of and below the head: {root} vs {head}"
        );
        assert!(
            tip.z < root.z - 20.0 && tip.x == 0.0,
            "and the chain hangs down the midline: {tip}"
        );
        let markers = markers_for(&recipe);
        let after_head = markers.iter().position(|m| m == "head").unwrap() + 1;
        assert_eq!(
            &markers[after_head..after_head + 8],
            (1..=8)
                .map(|k| format!("proboscis_{k:02}"))
                .collect::<Vec<_>>()
                .as_slice(),
            "the rail asks for the proboscis right after the head"
        );
        assert!(
            describe(&recipe).contains("proboscis (8)"),
            "{}",
            describe(&recipe)
        );

        let mut headless = recipe.clone();
        headless.trunk.head = false;
        assert!(
            compose(&headless, STATURE)
                .expect("composes")
                .iter()
                .all(|b| !b.name.starts_with("proboscis")),
            "no head, no proboscis"
        );
    }

    #[test]
    fn tails_chain_off_the_pelvis_and_a_headless_trunk_has_no_head() {
        let mut recipe = SkeletonRecipe::humanoid();
        recipe.trunk.head = false;
        recipe.trunk.tails = vec![
            TailKind::Long { bones: 8 },
            TailKind::ShortHair { bones: 4 },
        ];
        let bones = compose(&recipe, STATURE).expect("composes");
        let parent: HashMap<&str, &str> = bones
            .iter()
            .map(|b| (b.name.as_str(), b.parent.as_str()))
            .collect();
        assert_eq!(parent["tail_01"], "pelvis");
        for i in 2..=8 {
            assert_eq!(
                parent[format!("tail_{i:02}").as_str()],
                format!("tail_{:02}", i - 1)
            );
        }
        assert!(!parent.contains_key("tail_09"));
        assert_eq!(parent["tail2_01"], "pelvis");
        assert_eq!(parent["tail2_hair_01"], "tail2_01");
        assert_eq!(parent["tail2_hair_04"], "tail2_hair_03");
        assert!(!parent.contains_key("tail2_02"));
        for n in ["head", "jaw", "eye_l", "eye_r"] {
            assert!(!parent.contains_key(n), "headless: `{n}` must not exist");
        }
        assert!(parent.contains_key("neck_02"), "the neck stays");
        let by_name: HashMap<&str, Vec3> = bones
            .iter()
            .map(|b| (b.name.as_str(), b.position))
            .collect();
        assert!(
            by_name["tail_08"].y > by_name["tail_01"].y,
            "the tail runs back (+Y)"
        );
        assert_eq!(bones.len(), 67 - 4 + 8 + 5);
    }

    /// Every SHIPPED preset composes, the Humanoid preset IS the humanoid recipe, and a recipe
    /// survives the JSON round trip the presets ride on (real content; skips without it).
    /// EVERY RECIPE LIVES IN ITS PATTERN'S FOLDER (Aaron 2026-09-08): a shipped recipe sits in
    /// `skeletons/<Pattern>/` for the pattern it animates on, and that pattern's reference rig is
    /// baked there — a pattern's basis and the recipes on it never part. Real content; skips
    /// without it.
    #[test]
    fn every_recipe_lives_in_its_patterns_folder() {
        let dir = skeletons_dir();
        if !dir.is_dir() {
            eprintln!("skipping the shipped presets: no content tree");
            return;
        }
        let presets = load_presets(&dir);
        assert!(presets.len() > 1, "the content tree ships presets");
        for p in &presets {
            let home = pattern_dir(p.recipe.pattern());
            let recipe = home.join(format!("{}.recipe.json", p.name));
            assert!(
                recipe.is_file(),
                "`{}` lives at {}",
                p.name,
                recipe.display()
            );
            let rig = p.recipe.pattern().name();
            assert!(
                home.join(format!("{rig}.json.gz")).is_file()
                    || home.join(format!("{rig}.json")).is_file(),
                "`{}`'s pattern reference `{rig}` is baked beside it in {}",
                p.name,
                home.display()
            );
        }
        // Every authored pattern's reference is baked; a pattern still owed says so by name.
        for pattern in Pattern::ALL {
            let composes = compose(&reference_recipe(pattern), STATURE).is_ok();
            let home = pattern_dir(pattern);
            let baked = home.join(format!("{}.json.gz", pattern.name())).is_file();
            assert_eq!(
                baked,
                composes,
                "pattern `{}`: composes = {composes}, baked = {baked}",
                pattern.name()
            );
        }
        // The reference recipes carry the canon everywhere but the legs (and the toe walker's arms).
        let d = reference_recipe(Pattern::Digitigrade);
        assert_eq!(d.trunk.arms, vec![ArmKind::Humanoid]);
        assert!(d.trunk.tails.is_empty() && d.trunk.head);
        assert_eq!(
            reference_recipe(Pattern::ToeWalker).trunk.arms,
            vec![ArmKind::Hanging]
        );
        assert_eq!(
            reference_recipe(Pattern::Humanoid),
            SkeletonRecipe::humanoid()
        );
    }

    #[test]
    fn every_shipped_preset_composes() {
        let dir = skeletons_dir();
        let presets = load_presets(&dir);
        assert_eq!(presets[0].name, "Humanoid");
        assert_eq!(presets[0].recipe.trunk, SkeletonRecipe::humanoid().trunk);
        assert_eq!(presets[0].recipe.preset.as_deref(), Some("Humanoid"));
        if !dir.is_dir() {
            eprintln!("skipping the shipped presets: no content tree");
            return;
        }
        assert!(
            presets.len() > 1,
            "the content tree ships presets in {}",
            dir.display()
        );
        for p in &presets {
            let bones = compose(&p.recipe, STATURE)
                .unwrap_or_else(|e| panic!("preset `{}` does not compose: {e}", p.name));
            // A whole body, not a stub: a hoofed quadruped is 33 bones (no fingers, no
            // twists), the humanoid 67.
            assert!(bones.len() >= 30, "preset `{}` is a body", p.name);
            let text = serde_json::to_string(&p.recipe).unwrap();
            let back: SkeletonRecipe = serde_json::from_str(&text).unwrap();
            assert_eq!(back, p.recipe, "preset `{}` round-trips", p.name);
            assert!(
                !describe(&p.recipe).starts_with("does not"),
                "`{}` describes",
                p.name
            );
        }
    }

    /// The emitted file round-trips through the ENGINE loader and the RETARGETER's
    /// target reader — the two consumers the reference exists for.
    #[test]
    fn the_baseline_emits_loads_and_targets() {
        let dir = std::env::temp_dir().join("flicker_baseline_emit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = emit(&dir).expect("emit baseline");
        let model = flicker_skeletal::format::load_dir(out.parent().unwrap())
            .expect("engine loads the baseline");
        assert_eq!(model.bones.len(), CANON_BONES);
        assert!(
            model.mesh.vertices.is_empty(),
            "skeleton-only — the reference is nobody's body"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
