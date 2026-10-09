//! Bake a conformed [`RawModel`] to the canonical `flicker.rig` — the last stage of the in-app
//! pipeline (WS-F slice 4). Emits the SAME JSON the Blender exporter produces, via the canonical
//! `flicker-skeletal` format types (their `Serialize` derive is additive), so there is one schema.
//!
//! Two things happen here that the conform deliberately left out, because they are bake concerns:
//!   * a synthesized identity **`root`** bone at index 0 (Meshy rigs have none; the engine + baked
//!     clips expect `root` at the feet as bone 0, `pelvis` parented to it) — every existing parent
//!     index and every vertex joint index shifts +1;
//!   * the source header (`Z_up` / `cm` / `applied_transform: none`) the loader reads to orient the
//!     rig into engine space, and `retarget: true` (rotation-only playback for a retargeted body).

use anyhow::{bail, Context, Result};
use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use flicker_skeletal::format::{
    ArmKind, Attach, BoneRaw, Cloth, Collision, Material, Mesh, RigFile, Skeleton, SkeletonRecipe,
    Source, Submesh, TrunkSpec, Vertex,
};

use crate::fbx::{RawBone, RawModel, RawVertex};
use crate::flesh::Flesh;
use crate::regions::{build_cloth, split_garment};
use crate::shape::{Body, ShapeGraph};

const IDENTITY16: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// The translation that puts the model's TRUNK on the origin's plumb line (X and Y only — the
/// ground stays where it is), `None` for a model without a pelvis. A biped stands its pelvis on
/// the line; a QUADRUPED — read off the rest, its spine running along the body rather than up —
/// stands the middle of its back there (pelvis to `spine_03`), so the root sits under the body
/// and not at its tail end.
fn pelvis_shift(model: &RawModel) -> Option<Vec3> {
    let i = model.bones.iter().position(|b| b.name == "pelvis")?;
    let world = crate::conform::model_world_frames(model);
    let p = world.get(i)?.w_axis.truncate();
    let centre = model
        .bones
        .iter()
        .position(|b| b.name == "spine_03")
        .and_then(|j| world.get(j))
        .map(|g| g.w_axis.truncate())
        .filter(|s| {
            let d = *s - p;
            d.y.abs() > d.z.abs() // the spine runs along the body: a quadruped
        })
        .map_or(p, |s| (p + s) * 0.5);
    Some(Vec3::new(-centre.x, -centre.y, 0.0))
}

/// Carry the whole model by `shift`: every vertex, every top-level bone's rest translation, and
/// every bone's inverse bind (`world⁻¹ · T(−shift)` — rest skinning stays the identity).
pub(crate) fn recentre(model: &mut RawModel, shift: Vec3) {
    for v in &mut model.vertices {
        v.p = (Vec3::from(v.p) + shift).to_array();
    }
    // A region's chain anchors are BIND-space points on the mesh — they ride the carry, or the
    // whole comb hangs a body-width away from the cloth it drives.
    for c in model.regions.iter_mut().flat_map(|r| r.chains.iter_mut()) {
        c.anchor = (Vec3::from(c.anchor) + shift).to_array();
    }
    let back = Mat4::from_translation(-shift);
    for b in &mut model.bones {
        if b.parent < 0 {
            b.translation = (Vec3::from(b.translation) + shift).to_array();
        }
        b.inverse_bind = (Mat4::from_cols_array(&b.inverse_bind) * back).to_cols_array();
    }
}

/// Assemble a [`RigFile`] from a conformed model. `source_name` is the asset stem (the exporter's
/// `--out` basename, e.g. `"PrismHumanBaseA"`). A single flat material/submesh is emitted when the
/// model carries no material info; textured materials are wired in the texture slice.
pub fn bake_rig(model: &RawModel, source_name: &str) -> RigFile {
    // THE ROOT SITS UNDER THE PELVIS (Aaron 2026-09-07: "the root motion bone is wrong"): the
    // synthesized root is the origin, so the whole body is carried across until the pelvis
    // stands on the origin's plumb line — a mesh whose bounding box a tail skewed off-centre
    // no longer animates beside its own position. A canon body already has its pelvis on the
    // line and bakes untouched.
    let recentred;
    let model = match pelvis_shift(model) {
        Some(shift) if shift.length_squared() > 1e-8 => {
            let mut m = model.clone();
            recentre(&mut m, shift);
            recentred = m;
            &recentred
        }
        _ => model,
    };
    // Synthesized identity root at bone 0; every real bone's parent shifts +1 (a former root → 0).
    let mut bones = Vec::with_capacity(model.bones.len() + 1);
    bones.push(BoneRaw {
        name: "root".to_string(),
        parent: -1,
        local: IDENTITY16,
        inverse_bind: IDENTITY16,
    });
    for b in &model.bones {
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from(b.scale),
            Quat::from_array(b.rotation),
            Vec3::from(b.translation),
        )
        .to_cols_array();
        bones.push(BoneRaw {
            name: b.name.clone(),
            parent: if b.parent < 0 { 0 } else { b.parent + 1 },
            local,
            inverse_bind: b.inverse_bind,
        });
    }

    // Vertices: the +1 root shift moves every joint index up by one (0-weight pads included — inert).
    // EXCEPT when the source carried no bones at all: then the synthesized `root` is the ONLY bone,
    // and shifting would point every vertex at a non-existent bone 1. Such a mesh belongs in
    // [`bake_prop`], but the character path must still emit a VALID file if one reaches it.
    let shift = if model.bones.is_empty() { 0 } else { 1 };
    let vertices: Vec<Vertex> = model
        .vertices
        .iter()
        .map(|v| Vertex {
            p: v.p,
            n: v.n,
            uv: v.uv,
            joints: [
                v.joints[0] + shift,
                v.joints[1] + shift,
                v.joints[2] + shift,
                v.joints[3] + shift,
            ],
            weights: v.weights,
        })
        .collect();
    let indices = model.indices.clone();

    // One flat submesh/material for now (untextured → the loader renders it neutral).
    let materials = vec![Material {
        name: "material_0".to_string(),
        slot: "material_0".to_string(),
        ..Default::default()
    }];
    let submeshes = vec![Submesh {
        material: 0,
        start: 0,
        count: indices.len(),
    }];
    // The BODY CAPSULES, read off this body's own flesh — what the runtime cloth hangs off so a
    // coat's falls do not pass through the thighs (spec 6C46CAB9, on the rig's existing
    // `collision` contract). Empty for a boneless mesh; old rigs simply carry none.
    let capsules = crate::capsules::bone_capsules(model, &bones);
    // THE BODY'S OWN REGIONS GET THEIR COMBS HERE — a mane, a tail's fall, an ear tagged on the
    // body (by hand in the Regions panel, or handed off by the bind: ruling 7881216F) shipped
    // with its membership and no chains, so nothing at runtime ever swung it. The chains are
    // DERIVED, measured off the mesh as it is baked ([`build_cloth`]); a region from an older
    // rig that carries binds and no authored membership keeps exactly what it carries.
    let mut regions = model.regions.clone();
    for r in regions.iter_mut().filter(|r| !r.verts.is_empty()) {
        build_cloth(model, r);
    }

    RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: source_name.to_string(),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: "none".to_string(),
            ..Default::default()
        },
        skeleton: Skeleton { bones },
        mesh: Mesh {
            vertices,
            indices,
            submeshes,
            materials,
            // The model's TAGGED REGIONS are the rig's cloth: membership, tag, comb and binds
            // (spec 0A81088E). Empty for an untagged body.
            cloth: Cloth { regions },
            ..Default::default()
        },
        clips: Vec::new(),
        attach: Default::default(),
        attach_points: Vec::new(),
        collision: Collision { volumes: capsules },
        retarget: true,
        skeleton_recipe: None,
    }
}

/// Bake a bone-less static **PROP** mesh (a weapon, an accessory, a raw garment before it is
/// skinned) to `flicker.rig`. A prop is the OPPOSITE of the character bake:
///   * it is NEVER skinned and carries **no skeleton** (`skeleton.bones: []`) — so there is no
///     synthesized `root` and **no `+1` joint shift**; the vertices, whose skin is the inert
///     `([0;4], [0.0;4])` `parse_fbx` yields for an unrigged mesh, pass through verbatim;
///   * `retarget` is **false** — a rigid prop plays no retargeted clips;
///   * the socket it hangs from and its fit (offset/rotate/scale) are NOT baked here — they are
///     authored in the editor and folded into the `attach` block from `fits.json` at load time
///     (`flicker-paperdoll::write_inline_attach`), which is why `export_prop` also omits them.
///
/// Byte-shape parity with the Python `io_scene_flicker_rig.py::export_prop`.
/// Recompute the mesh's skin WEIGHTS from the current skeleton — DISCARDING the source rig's
/// vendor auto-skin. This is the system's whole point (Aaron 2026-08-20): a vendor (Meshy)
/// weights the mesh against ITS OWN skeleton, which rides the FRONT of the body — so once the
/// joints are re-placed on the body's true spinal axis, the vendor weights are wrong by
/// construction (the 2026-08-20 golem audit measured that bleed: forearms owning belly flesh,
/// clavicles owning a third of the neck). Each vertex binds to its nearest bone SEGMENTS by
/// inverse-square distance, top-4, pruned and normalised.
///
/// Segment defaults, hardened by that audit:
///   * a bone's tail is the MEAN of its children's heads, not its FIRST child — first-child
///     tails ran `pelvis→thigh_l` and `spine_03→clavicle_l` (both sideways-LEFT in canonical
///     order), skewing the whole torso's weighting toward one side; the mean puts a
///     multi-child bone's segment back on the chain (pelvis→up-the-spine, hand→out-the-palm);
///   * non-deform bones never own flesh: `root` (whose segment spans the entire lower core)
///     and the `Weapon_L`/`Weapon_R` mount sockets are excluded from candidacy;
///   * a bone must sit INSIDE the flesh it owns: a candidate whose nearest point lies outward
///     of the vertex along its normal is air, not anatomy — the A-posed forearms hang in
///     FRONT of the belly, and by raw distance they claimed a third of it (measured on the
///     golem); the normal test rejects them while keeping every bone genuinely under the skin.
///     The test is only trusted when it leaves a PLAUSIBLE bone (2026-09-03, ElfBaseA): if the
///     nearest surviving bone is both twice as far as the nearest bone of all and more than
///     15 cm out, the normals are lying for this vertex — an inward-facing inner shell, or a
///     bone grazing a curved surface at a marginal angle — and the vertex falls back to plain
///     distance. Before that guard, a rejected heel bone handed the toes to finger bones a
///     metre away, which posed as long triangles trailing the feet;
///   * influences below 2% after normalisation are pruned and the rest renormalised, so a
///     distant bone never keeps a token grip on flesh it has no business moving.
///
/// Two more, from the 2026-09-07 stray triangles on GolemBaseV2 (the ultra body under the
/// Katanami set):
///   * the mesh arrives one vertex per CORNER (the `parse_fbx` / `decimate_to` convention), so one
///     point on the skin used to be scored up to a dozen times from a dozen slightly different
///     wedge normals — and two corners that disagreed on a bone tore apart the moment that bone
///     moved (1,575 of the golem's 31,002 positions had corners bound to different bones). Each
///     POSITION is scored once, from the mean of its corners' normals, and every corner takes the
///     same answer;
///   * the raw inverse-square pick is a hard switch at the equidistant surface between two bones
///     (calf→foot at the ankle, spine→upperarm at the armpit), which stretched the triangles
///     straddling it into spikes. The weights are diffused over the mesh's own edges afterwards
///     — a Laplacian blend, `SMOOTH_RADIUS_CM` wide — so a joint's transition is a band, not a
///     step. Measured off-GPU on the golem over six Katanami poses: 7,842 stretched triangles
///     → 363. (A softer falloff constant instead was measured WORSE: it only widens the bleed.)
///
/// And one from the 2026-09-28 stance normaliser (the 13 declined strides of F4A8D976):
///   * A LIMB'S SKIN IS ITS TUBE'S FLESH (spec 04803E0C's graph, read off this body's own flesh by
///     [`Tubes::read`]). A bone the fit laid down a limb of the shape graph is SEATED in that limb,
///     and its flesh is the limb's tube and nothing else: a vertex belongs to a limb's bones only
///     where [`ShapeGraph::limb_membership`] says it is that limb's flesh, blending to the trunk's
///     bones across a JUNCTION BAND about one limb radius either side of where the limb's bones
///     begin; a vertex outside every seated tube takes the trunk's bones only (every bone seated in
///     no tube — the spine, the head, a module nothing matched). Plain distance put the belly, the
///     rump and the other side's paw on a thigh whose head sits at the skin of the barrel, and a
///     re-pose of that thigh across a 14–28 cm stride dragged them through the floor. The Laplacian
///     below runs as before and the partition is then put back ([`keep_to_the_tubes`]), so it
///     blends a knee inside a limb and a hip inside the trunk but never smears limb weight back
///     across the junction. A body with no graph, or with no bone seated in any limb of it, binds
///     exactly as it always did.
///
/// Run AFTER the bones are repositioned inside the mesh: the rest pose and `inverse_bind` are
/// untouched, so this changes only how the mesh DEFORMS when posed, never where it sits at rest.
///
/// This door READS the body itself; the raw-mesh rig sequence and the bench's re-bake bind on the
/// body the fit already read ([`bind`]).
pub fn bake_skin(model: &mut RawModel) {
    let body = Body::read(model);
    bind(model, Some(&body));
}

/// THE BIND ITSELF — [`bake_skin`] on a body ALREADY READ off this mesh (the fit's own, handed on
/// by `conform::rig_raw_mesh` and kept by the bench: the bind asks its questions of the graph the
/// fit laid the joints down, and never thins the mesh a second time), or with `None` the plain
/// distance bind every body without a seated limb gets, which is what a gate measures the tube
/// bind against.
///
/// THIS IS THE SKIN A BODY IS RIGGED AND UN-POSED ON: a limb's bones carry their own TUBE and
/// nothing of the trunk, so [`square_stance`] can swing a raised limb across a whole stride
/// without dragging the belly through the floor. It is not the skin a body MOVES in —
/// [`bind_for_motion`], which the bake path puts on last.
pub fn bind(model: &mut RawModel, body: Option<&Body>) {
    bind_on(model, body, None);
}

/// THE SKIN A BODY MOVES IN — [`bind`], and each standing limb's bones carry their GIRDLE as
/// well as their tube ([`Girdles`]): the haunch a buried femur lies under, the shoulder over a
/// scapula. The LAST step of the one bake path, after the un-poses ([`square_stance`],
/// [`face_forward`]) have run on the tube skin.
///
/// `seating` is how the body sat in its limbs AS POSED ([`Seating::read`], taken before the
/// un-poses): a limb's tube is the flesh the un-pose carried, and stays exactly that — a
/// squared limb is not read a second time off a mesh where its foot now stands against its
/// twin's. Only the girdles are read where the body now STANDS: round the bones as they now
/// lie, through `flesh` — this mesh's field as it stands, `None` to read it here (an un-pose
/// moved it).
///
/// A body with no girdle — no seating, an upright trunk, no limb to the ground — keeps the
/// skin it has, bit for bit.
pub fn bind_for_motion(model: &mut RawModel, seating: Option<&Seating>, flesh: Option<&Flesh>) {
    let Some(seating) = seating.filter(|s| {
        !s.girdled.is_empty()
            && s.member.len() == model.vertices.len()
            && s.seat.len() == model.bones.len()
    }) else {
        return;
    };
    let read;
    let flesh = match flesh {
        Some(f) => f,
        None => {
            read = Flesh::build_body(model);
            &read
        }
    };
    bind_on(model, None, Some((seating, flesh)));
}

/// HOW A BODY SITS IN ITS LIMBS, as the bind reads it off the body AS POSED ([`Tubes::read`]):
/// the limb each bone is seated in, the limb each vertex is flesh of, and which limbs carry a
/// girdle. Bone for bone and vertex for vertex, so it stays true of the model through an
/// un-pose, which moves both and reorders neither.
pub struct Seating {
    /// Per bone: the graph limb it is seated in, `None` for a bone of the trunk.
    seat: Vec<Option<usize>>,
    /// Per VERTEX: the limb whose tube it is flesh of, and its share
    /// ([`ShapeGraph::limb_membership`]).
    member: Vec<Option<(usize, f32)>>,
    /// The limbs that carry a GIRDLE ([`Tubes::girdled`]): the graph limb, the side of the
    /// plane it hangs on (±1), how thick its tube is where it begins (cm).
    girdled: Vec<(usize, f32, f32)>,
    /// Per VERTEX: the one bone it is RIGID with, where it is flesh of a bare end
    /// ([`rigid_ends`]) — a horn, an antler, an ear — and how many times BROADER than it is
    /// thin the flesh is there ([`cross_section`]).
    rigid: Vec<Option<(usize, f32, u32)>>,
    plane_x: f32,
    cell: f32,
}

impl Seating {
    /// `None` when the mesh has no graph or no bone is seated in any limb of it.
    pub fn read(model: &RawModel, body: &Body) -> Option<Seating> {
        let (heads, tails) = bone_segments(model);
        let (corner, positions) = crate::fbx::weld_by_position(&model.vertices);
        Self::on(model, body, &heads, &tails, &corner, &positions)
    }

    /// [`Self::read`] on a weld already made.
    fn on(
        model: &RawModel,
        body: &Body,
        heads: &[Vec3],
        tails: &[Vec3],
        corner: &[u32],
        positions: &[Vec3],
    ) -> Option<Seating> {
        let deform = deforming(model);
        let tubes = Tubes::read(model, heads, tails, &deform, body)?;
        let at: Vec<Option<(usize, f32)>> = positions
            .iter()
            .map(|&p| tubes.graph.limb_membership(p, &tubes.begin))
            .collect();
        let rigid = rigid_ends(&tubes, &body.flesh, model, heads, tails, &deform, positions);
        Some(Seating {
            member: corner.iter().map(|&c| at[c as usize]).collect(),
            rigid: corner.iter().map(|&c| rigid[c as usize]).collect(),
            girdled: tubes.girdled(heads),
            plane_x: tubes.graph.plane_x,
            cell: tubes.graph.cell,
            seat: tubes.seat,
        })
    }
}

/// A BONELESS APPENDAGE of a bound body, as the bind read it ([`rigid_ends`]): flesh that is
/// rigid with one bone — a horn, an antler, an ear, a tusk, the fall of hair past a tail's last
/// bone.
#[derive(Debug, Clone, PartialEq)]
pub struct Appendage {
    /// The bone it grows from (an index into the model's bones).
    pub bone: usize,
    /// Its vertices.
    pub verts: Vec<u32>,
    /// How many times BROADER than it is thin its flesh typically is ([`cross_section`]):
    /// about 1 for a round tube (a horn), several for a sheet (an ear) — the median of
    /// [`Appendage::flats`].
    pub flat: f32,
    /// That reading at every position of it, sorted: what a pair of twins is read as one by.
    pub flats: Vec<f32>,
    /// The share of it that is its largest CONNECTED piece of mesh: 1 for a horn, which is one
    /// shell; a fall of hair or a fan of feathers is many cards lying along each other, none
    /// of them much of it.
    pub whole: f32,
    /// Its TWIN across the body's plane, by index among the appendages read together: the one
    /// on the same bone, or a bone of the same chain, that mirrors it (the other horn, the
    /// other ear), or the one nearest its size on the twin bone (the other wing's feathers) —
    /// each the other's nearest. A pair is read as ONE (`regions::appendage_regions`), and
    /// hangs from ONE bone — the further down the chain of the two — so two horns never
    /// arrive one soft and one rigid, or one on the head and one on the neck, on readings
    /// either side of the line.
    pub twin: Option<usize>,
}

/// Two appendages MIRROR each other when their centroids reflect across the plane within this
/// share of their own spread (plus two cells for the grid).
const TWIN_SPREAD: f32 = 0.5;

/// A piece under this share of the body's welded positions is a sliver, not an appendage
/// anyone would tag.
const MIN_APPENDAGE: f32 = 0.001;
/// Two positions of one bone within this many cells of each other LIE AGAINST each other.
const APPENDAGE_GAP_CELLS: f32 = 2.0;
/// Two pieces of one bone are ONE appendage when this share of either's positions lies against
/// the other: a fall of hair past a tail's last bone arrives as dozens of separate cards (35 on
/// one source) that run side by side their whole length, and it is one fall; a horn and the ear
/// beside it touch at their roots alone, and stay two.
const APPENDAGE_ALONG: f32 = 0.5;

impl Seating {
    /// THE APPENDAGES this seating holds rigid, each as ONE piece: the positions rigid with one
    /// bone that share a triangle edge, and the pieces of them that lie along each other
    /// ([`APPENDAGE_ALONG`]). `model` is the mesh the seating was read off (vertex for vertex);
    /// a seating of another mesh answers with none.
    pub fn appendages(&self, model: &RawModel) -> Vec<Appendage> {
        if self.rigid.len() != model.vertices.len() {
            return Vec::new();
        }
        let (corner, positions) = crate::fbx::weld_by_position(&model.vertices);
        let mut at: Vec<Option<(usize, f32, u32)>> = vec![None; positions.len()];
        for (&c, r) in corner.iter().zip(&self.rigid) {
            at[c as usize] = *r;
        }
        let mut group: Vec<u32> = (0..positions.len() as u32).collect();
        fn find(group: &mut [u32], mut x: u32) -> u32 {
            while group[x as usize] != x {
                group[x as usize] = group[group[x as usize] as usize];
                x = group[x as usize];
            }
            x
        }
        fn join(group: &mut [u32], a: u32, b: u32) {
            let (ra, rb) = (find(group, a), find(group, b));
            group[ra.max(rb) as usize] = ra.min(rb);
        }
        // SHELLS: the positions of one bone joined across the mesh's own edges.
        for t in model.indices.as_chunks::<3>().0 {
            let w = t.map(|i| corner[i as usize]);
            for (a, b) in [(w[0], w[1]), (w[1], w[2]), (w[2], w[0])] {
                let same = match (at[a as usize], at[b as usize]) {
                    (Some(x), Some(y)) => x.0 == y.0,
                    _ => false,
                };
                if same {
                    join(&mut group, a, b);
                }
            }
        }
        let rigid: Vec<u32> = (0..positions.len() as u32)
            .filter(|&pi| at[pi as usize].is_some())
            .collect();
        let bone_of = |pi: u32| at[pi as usize].map_or(usize::MAX, |r| r.0);
        let shell: Vec<u32> = (0..positions.len() as u32)
            .map(|pi| find(&mut group, pi))
            .collect();
        let mut shell_size: HashMap<u32, usize> = HashMap::new();
        for &pi in &rigid {
            *shell_size.entry(shell[pi as usize]).or_default() += 1;
        }
        // ...and the shells on ONE RUN of one bone — the flesh round one bare subtree of the
        // shape graph ([`rigid_ends`]): a fall of hair is one fall however many cards it is cut
        // into, and so is a fan of feathers.
        let mut first: HashMap<(usize, u32), u32> = HashMap::new();
        for &pi in &rigid {
            let Some((bone, _, run)) = at[pi as usize] else {
                continue;
            };
            match first.entry((bone, run)) {
                std::collections::hash_map::Entry::Occupied(o) => join(&mut group, *o.get(), pi),
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(pi);
                }
            }
        }
        let shell_group: Vec<u32> = (0..positions.len() as u32)
            .map(|pi| find(&mut group, pi))
            .collect();
        // Which OTHER pieces of its bone each position lies against ([`APPENDAGE_GAP_CELLS`]).
        // A cell all of one piece (the flag) has nothing to tell that piece.
        let gap = APPENDAGE_GAP_CELLS * self.cell;
        let cell_of = |p: Vec3| (p / gap).floor().as_ivec3().to_array();
        let mut cells: HashMap<(usize, [i32; 3]), (bool, Vec<u32>)> = HashMap::new();
        for &pi in &rigid {
            let cell = cells
                .entry((bone_of(pi), cell_of(positions[pi as usize])))
                .or_default();
            cell.0 |= cell
                .1
                .first()
                .is_some_and(|&o| shell_group[o as usize] != shell_group[pi as usize]);
            cell.1.push(pi);
        }
        let mut against: Vec<(u32, u32)> = Vec::new();
        let mut seen: Vec<u32> = Vec::new();
        for &pi in &rigid {
            let (p, mine) = (positions[pi as usize], shell_group[pi as usize]);
            let home = cell_of(p);
            seen.clear();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let near = [home[0] + dx, home[1] + dy, home[2] + dz];
                        let Some((mixed, there)) = cells.get(&(bone_of(pi), near)) else {
                            continue;
                        };
                        if !mixed && shell_group[there[0] as usize] == mine {
                            continue;
                        }
                        for &o in there {
                            let other = shell_group[o as usize];
                            if other != mine
                                && !seen.contains(&other)
                                && positions[o as usize].distance(p) <= gap
                            {
                                seen.push(other);
                            }
                        }
                    }
                }
            }
            against.extend(seen.iter().map(|&other| (pi, other)));
        }
        // Join the pieces that lie along each other, until none does.
        loop {
            let mut size: HashMap<u32, usize> = HashMap::new();
            for &pi in &rigid {
                *size.entry(find(&mut group, pi)).or_default() += 1;
            }
            let mut along: HashMap<(u32, u32), usize> = HashMap::new();
            let mut k = 0;
            while k < against.len() {
                let pi = against[k].0;
                let mine = find(&mut group, pi);
                seen.clear();
                while k < against.len() && against[k].0 == pi {
                    let other = find(&mut group, against[k].1);
                    if other != mine && !seen.contains(&other) {
                        seen.push(other);
                    }
                    k += 1;
                }
                for &other in &seen {
                    *along.entry((mine, other)).or_default() += 1;
                }
            }
            let mut joins: Vec<(u32, u32)> = along
                .into_iter()
                .filter(|((a, _), n)| *n as f32 >= APPENDAGE_ALONG * size[a] as f32)
                .map(|(pair, _)| pair)
                .collect();
            if joins.is_empty() {
                break;
            }
            joins.sort_unstable();
            for (a, b) in joins {
                join(&mut group, a, b);
            }
        }
        // Per appendage: its bone, how flat the flesh is at every position of it, and the size
        // of its largest shell.
        let least = (MIN_APPENDAGE * positions.len() as f32).ceil() as usize;
        let mut pieces: HashMap<u32, (usize, Vec<f32>, usize)> = HashMap::new();
        for &pi in &rigid {
            let Some((bone, flat, _)) = at[pi as usize] else {
                continue;
            };
            let piece = pieces
                .entry(find(&mut group, pi))
                .or_insert((bone, Vec::new(), 0));
            piece.1.push(flat);
            piece.2 = piece.2.max(shell_size[&shell[pi as usize]]);
        }
        let mut roots: Vec<u32> = pieces
            .iter()
            .filter(|(_, piece)| piece.1.len() >= least)
            .map(|(root, _)| *root)
            .collect();
        roots.sort_unstable();
        let mut out: Vec<Appendage> = roots
            .into_iter()
            .map(|root| {
                let (bone, mut flat, largest) = pieces.remove(&root).expect("a root of the map");
                flat.sort_by(f32::total_cmp);
                let verts: Vec<u32> = (0..model.vertices.len() as u32)
                    .filter(|&v| {
                        at[corner[v as usize] as usize].is_some()
                            && find(&mut group, corner[v as usize]) == root
                    })
                    .collect();
                Appendage {
                    bone,
                    whole: largest as f32 / flat.len() as f32,
                    flat: flat[flat.len() / 2],
                    flats: flat,
                    verts,
                    twin: None,
                }
            })
            .collect();
        // TWINS across the plane ([`Appendage::twin`]).
        let centroid: Vec<Vec3> = out
            .iter()
            .map(|a| {
                a.verts
                    .iter()
                    .map(|&v| Vec3::from_array(model.vertices[v as usize].p))
                    .sum::<Vec3>()
                    / a.verts.len().max(1) as f32
            })
            .collect();
        let spread: Vec<f32> = out
            .iter()
            .zip(&centroid)
            .map(|(a, c)| {
                (a.verts
                    .iter()
                    .map(|&v| Vec3::from_array(model.vertices[v as usize].p).distance_squared(*c))
                    .sum::<f32>()
                    / a.verts.len().max(1) as f32)
                    .sqrt()
            })
            .collect();
        let name = |i: usize| model.bones[out[i].bone].name.as_str();
        let parent = |i: usize| usize::try_from(model.bones[i].parent).ok();
        let descends = |mut i: usize, from: usize| {
            while let Some(p) = parent(i) {
                if p == from {
                    return true;
                }
                i = p;
            }
            false
        };
        let one_chain = |i: usize, j: usize| {
            let (a, b) = (out[i].bone, out[j].bone);
            a == b || descends(a, b) || descends(b, a)
        };
        let mirrored = |i: usize, j: usize| {
            let tol = TWIN_SPREAD * spread[i].max(spread[j]) + 2.0 * self.cell;
            let (a, b) = (centroid[i], centroid[j]);
            one_chain(i, j)
                && (a.x + b.x - 2.0 * self.plane_x).abs() <= tol
                && (a.y - b.y).abs() <= tol
                && (a.z - b.z).abs() <= tol
        };
        let kin = |i: usize, j: usize| {
            i != j && (mirrored(i, j) || twin_name(name(i)).as_deref() == Some(name(j)))
        };
        let nearest = |i: usize| {
            let size = out[i].verts.len().max(1) as f32;
            (0..out.len()).filter(|&j| kin(i, j)).min_by(|&x, &y| {
                let ratio = |j: usize| {
                    let other = out[j].verts.len().max(1) as f32;
                    (size / other).max(other / size)
                };
                ratio(x).total_cmp(&ratio(y)).then(x.cmp(&y))
            })
        };
        let twins: Vec<Option<usize>> = (0..out.len())
            .map(|i| nearest(i).filter(|&j| nearest(j) == Some(i)))
            .collect();
        // A pair on two bones of one chain hangs from the one further down it.
        let bones: Vec<usize> = (0..out.len())
            .map(|i| match twins[i] {
                Some(j) if descends(out[j].bone, out[i].bone) => out[j].bone,
                _ => out[i].bone,
            })
            .collect();
        for ((a, twin), bone) in out.iter_mut().zip(twins).zip(bones) {
            a.twin = twin;
            a.bone = bone;
        }
        out
    }
}

/// The bind behind both doors: `body` to read the seating off (the tube skin), or the seating
/// `kept` from before an un-pose with the standing flesh its girdles are read through.
fn bind_on(model: &mut RawModel, body: Option<&Body>, kept: Option<(&Seating, &Flesh)>) {
    let n = model.bones.len();
    if n == 0 || model.vertices.is_empty() {
        return;
    }
    let (heads, tails) = bone_segments(model);
    let deform = deforming(model);
    /// An influence this weak after normalisation is noise — prune it and renormalise.
    const MIN_INFLUENCE: f32 = 0.02;
    // One score per POSITION (see above): weld the corners by position and average their normals.
    let (corner, positions) = crate::fbx::weld_by_position(&model.vertices);
    let mut normals: Vec<Vec3> = vec![Vec3::ZERO; positions.len()];
    for (v, &c) in model.vertices.iter().zip(&corner) {
        normals[c as usize] += Vec3::from_array(v.n);
    }

    // THE TUBES: which limb of this body's shape each bone is seated in, and which limb's flesh
    // each position is. `None` — no graph, or no bone seated in any limb of it — binds as before.
    let read = body.and_then(|b| Seating::on(model, b, &heads, &tails, &corner, &positions));
    let seating = kept.map(|k| k.0).or(read.as_ref());
    let mut member: Vec<Option<(usize, f32)>> = vec![None; positions.len()];
    let mut rigid: Vec<Option<usize>> = vec![None; positions.len()];
    if let Some(s) = seating {
        for ((&c, m), r) in corner.iter().zip(&s.member).zip(&s.rigid) {
            member[c as usize] = *m;
            rigid[c as usize] = r.map(|(bone, ..)| bone);
        }
    }
    // THE GIRDLES: trunk flesh a limb's buried bones lie under is that limb's too. Each limb's
    // share is read per position, then DIFFUSED across the mesh like any weight — where a
    // limb's bones and the trunk's lie a hand apart (a scapula's top under the withers, a hip
    // beside the pelvis) the bare ramp is two centimetres wide, and a swing tears the skin
    // across it (measured: 18 → 1047 stretched triangles on a woolly shoulder). A position
    // already in a tube keeps its tube; its own limb's share only ever grows.
    if let Some(g) = kept.map(|(s, flesh)| Girdles::of(s, flesh, &heads, &tails)) {
        let k = g.limbs.len();
        let mut shares = vec![0.0f32; positions.len() * k];
        for (row, &p) in shares.chunks_mut(k).zip(&positions) {
            g.shares(p, row);
        }
        // As wide as the thickest limb where it begins: one diffusion spreads a weight
        // [`SMOOTH_RADIUS_CM`], and spreads add in quadrature.
        let widest = g.limbs.iter().map(|l| l.3).fold(0.0_f32, f32::max);
        let rounds = ((widest / SMOOTH_RADIUS_CM).powi(2).round() as usize).clamp(1, 6);
        let free = vec![false; positions.len()];
        for _ in 0..rounds {
            smooth_weights(&mut shares, k, &positions, &corner, &model.indices, &free);
        }
        for (m, row) in member.iter_mut().zip(shares.chunks(k)) {
            let Some((at, share)) = row
                .iter()
                .copied()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|best| best.1 > GIRDLE_LEAST)
            else {
                continue;
            };
            let (l, share) = (g.limbs[at].0, share.min(1.0));
            *m = match *m {
                None => Some((l, share)),
                Some((own, s)) if own == l => Some((l, s.max(share))),
                other => other,
            };
        }
    }
    let seat: Vec<Option<usize>> = seating.map_or_else(|| vec![None; n], |s| s.seat.clone());

    // Per-position influences, dense over the bones (the smoothing pass below diffuses them).
    // A position of a seated limb scores that limb's bones for its share and the trunk's for the
    // rest; every other position scores the trunk's bones, which on a body with no seated limb
    // is every bone — the bind as it always was.
    let mut weights = vec![0.0f32; positions.len() * n];
    // The flesh's DEPTH at every position — its distance to the nearest bone it scored
    // against: what the motion skin's blend is as wide as ([`settle_weights`]).
    let mut depth = vec![f32::INFINITY; positions.len()];
    for (pi, (&p, nsum)) in positions.iter().zip(&normals).enumerate() {
        let normal = nsum.normalize_or_zero();
        let bones = Bones {
            heads: &heads,
            tails: &tails,
        };
        let row = &mut weights[pi * n..(pi + 1) * n];
        let trunk = |i: usize| deform[i] && seat[i].is_none();
        depth[pi] = match member[pi] {
            None => bones.score(p, normal, trunk, 1.0, row),
            Some((l, share)) => {
                let own = bones.score(p, normal, |i| deform[i] && seat[i] == Some(l), share, row);
                own.min(bones.score(p, normal, trunk, 1.0 - share, row))
            }
        };
        // A part with no bone to take it (a body that is all limb) hands its share back.
        let sum: f32 = row.iter().sum();
        if sum <= 0.0 {
            depth[pi] = bones.score(p, normal, |i| deform[i], 1.0, row);
        } else if (sum - 1.0).abs() > 1e-6 {
            row.iter_mut().for_each(|w| *w /= sum);
        }
    }
    // ── TAGGED REGIONS ARE PINNED (spec 0A81088E). A hair fall, a mane, a garment's hanging
    // panel is driven by ONE bone at 100 % — its anchor — and it stays out of the Laplacian
    // entirely: neither smoothed, nor a source for its neighbours. Diffusing across the seam is
    // exactly how hair smears onto the leg it hangs beside (the tail-hair swamp, 2D31782B), and
    // the pin is what the cloth sim then swings off. A region whose anchor bone is not in this
    // skeleton is left alone — it scores like any other flesh.
    let mut pinned = vec![false; positions.len()];
    // ── BARE ENDS ARE RIGID ([`rigid_ends`]): a horn, an antler, an ear is one bone's, whole.
    // Unlike a tagged region it LENDS that weight to the flesh it grows out of — it goes into
    // the Laplacian whole and is made whole again after it — so the skin round its base
    // blends into it over the same band as any joint, and a horn does not hinge on a crease.
    // A region tagged over one still wins.
    let whole = |weights: &mut [f32], pinned: &mut [bool], pin: bool| {
        for (pi, bone) in rigid.iter().enumerate() {
            if let Some(b) = bone.filter(|&b| b < n && !pinned[pi]) {
                let row = &mut weights[pi * n..(pi + 1) * n];
                row.fill(0.0);
                row[b] = 1.0;
                pinned[pi] = pin;
            }
        }
    };
    whole(&mut weights, &mut pinned, false);
    for r in &model.regions {
        let Some(b) = model.bones.iter().position(|x| x.name == r.anchor_bone) else {
            tracing::warn!(
                "bake_skin: region '{}' anchor bone '{}' is not in the skeleton; not pinned",
                r.name,
                r.anchor_bone
            );
            continue;
        };
        for &v in &r.verts {
            let Some(&c) = corner.get(v as usize) else {
                continue;
            };
            let row = &mut weights[c as usize * n..(c as usize + 1) * n];
            row.fill(0.0);
            row[b] = 1.0;
            pinned[c as usize] = true;
        }
    }
    // THE SKIN A BODY IS UN-POSED ON blends every joint a hand's width; THE SKIN IT MOVES IN
    // is settled to equilibrium, its blends as wide as the flesh is thick ([`settle_weights`]).
    match kept {
        None => smooth_weights(
            &mut weights,
            n,
            &positions,
            &corner,
            &model.indices,
            &pinned,
        ),
        Some((s, _)) => settle_weights(
            &mut weights,
            n,
            &positions,
            &corner,
            &model.indices,
            &pinned,
            &depth,
            s.cell,
        ),
    }
    whole(&mut weights, &mut pinned, true);
    if seating.is_some() {
        keep_to_the_tubes(&mut weights, n, &member, &seat, &pinned);
    }

    let sets: Vec<([u32; 4], [f32; 4])> = (0..positions.len())
        .map(|pi| top_four(&weights[pi * n..(pi + 1) * n], MIN_INFLUENCE))
        .collect();
    for (v, &c) in model.vertices.iter_mut().zip(&corner) {
        (v.joints, v.weights) = sets[c as usize];
    }
}

/// Every bone's BODY as the bind reads it: the segment from its joint to the MEAN of its children's
/// joints — a leaf is a point. The mean keeps a multi-child bone's segment on the chain (a pelvis
/// runs up its spine, not sideways down one thigh; see [`bake_skin`]).
pub(crate) fn bone_segments(model: &RawModel) -> (Vec<Vec3>, Vec<Vec3>) {
    let n = model.bones.len();
    let heads: Vec<Vec3> = rest_world_frames(model)
        .iter()
        .map(|g| g.w_axis.truncate())
        .collect();
    let mut child_sum = vec![Vec3::ZERO; n];
    let mut child_count = vec![0u32; n];
    for (i, b) in model.bones.iter().enumerate() {
        if let Ok(p) = usize::try_from(b.parent) {
            if p < n {
                child_sum[p] += heads[i];
                child_count[p] += 1;
            }
        }
    }
    let tails = (0..n)
        .map(|i| {
            if child_count[i] > 0 {
                child_sum[i] / child_count[i] as f32
            } else {
                heads[i]
            }
        })
        .collect();
    (heads, tails)
}

/// Which bones may own flesh at all: the bake-synthetic `root` and the weapon mount sockets are
/// attachment frames, not deformers.
fn deforming(model: &RawModel) -> Vec<bool> {
    model
        .bones
        .iter()
        .map(|b| !matches!(b.name.as_str(), "root" | "Weapon_L" | "Weapon_R"))
        .collect()
}

/// How far outward (cosine vs the vertex normal) a bone's nearest point may sit before it counts
/// as OUTSIDE the flesh. 0 would reject bones lying tangentially along the surface (a clavicle
/// beside the neck); 1 would reject nothing. 0.25 keeps under-the-skin and alongside-the-skin
/// bones while cutting the in-the-air limb in front of the torso.
const OUTSIDE_COS: f32 = 0.25;
/// The normal test is DISTRUSTED for a vertex when its nearest inside bone sits further than BOTH
/// this multiple of the nearest bone overall AND [`FAR_INSIDE_CM`] — the A-posed forearm in front
/// of the belly (6 cm out, spine 13 cm in) stays rejected, while a misread heel bone (7 cm, next
/// inside bone at 25 cm+) is no longer overruled.
const FAR_INSIDE_RATIO: f32 = 2.0;
/// See [`FAR_INSIDE_RATIO`]; the absolute floor keeps a close call between two nearby bones on the
/// normal test's side.
const FAR_INSIDE_CM: f32 = 15.0;

/// The bones as [`bake_skin`] scores flesh against them: each one's segment, joint to tail.
struct Bones<'a> {
    heads: &'a [Vec3],
    tails: &'a [Vec3],
}

impl Bones<'_> {
    /// ONE POSITION'S RAW INFLUENCES over the bones `allowed` admits, normalised and ADDED into
    /// `row` at `scale` — the bind's own reading (see [`bake_skin`]): every admitted bone by the
    /// distance to its segment, the normal test with its distrust guard, the nearest four by
    /// inverse square. Nothing is added when `allowed` admits no bone. Hands back the flesh's
    /// DEPTH there — the distance to the nearest admitted bone (infinite when none).
    fn score(
        &self,
        p: Vec3,
        normal: Vec3,
        allowed: impl Fn(usize) -> bool,
        scale: f32,
        row: &mut [f32],
    ) -> f32 {
        if scale <= 0.0 {
            return f32::INFINITY;
        }
        let inside = |cp: Vec3| -> bool {
            let to_bone = cp - p;
            let len = to_bone.length();
            // A bone point ON the vertex, or a degenerate normal, can't be judged — allow it.
            len < 1e-4 || normal.dot(to_bone) / len <= OUTSIDE_COS
        };
        // Every admitted bone by distance, each tagged with the normal test's verdict.
        let mut all: Vec<(usize, f32, bool)> = (0..self.heads.len())
            .filter(|&i| allowed(i))
            .map(|i| {
                let cp = closest_point_segment(p, self.heads[i], self.tails[i]);
                (i, (p - cp).length(), inside(cp))
            })
            .collect();
        all.sort_by(|a, b| a.1.total_cmp(&b.1));
        let nearest = all.first().map_or(f32::INFINITY, |c| c.1);
        let nearest_inside = all.iter().find(|c| c.2).map_or(f32::INFINITY, |c| c.1);
        // Trust the normal test only while it leaves a plausible bone. No inside candidate at
        // all (a sliver, junk normals), or the survivors implausibly far behind the nearest bone
        // (an inward-facing shell, a heel bone grazing the skin at a marginal angle) — plain
        // distance is still better than flesh bound a metre away.
        let distrust = nearest_inside > (nearest * FAR_INSIDE_RATIO).max(FAR_INSIDE_CM);
        let picked: Vec<(usize, f32)> = all
            .iter()
            .filter(|c| distrust || c.2)
            .take(4)
            .map(|c| (c.0, 1.0 / (c.1 * c.1 + 1e-3)))
            .collect();
        let sum: f32 = picked.iter().map(|c| c.1).sum();
        if sum > 0.0 {
            for (i, w) in picked {
                row[i] += scale * (w / sum);
            }
        }
        nearest
    }
}

/// THE TUBES A SKIN BIND HONOURS — this body's own [`ShapeGraph`], the limb of it every bone is
/// SEATED in, and where each seated limb's flesh begins.
struct Tubes<'a> {
    graph: &'a ShapeGraph,
    /// Per bone: the graph limb it is seated in, `None` for a bone of the trunk.
    seat: Vec<Option<usize>>,
    /// Per graph limb: where its own flesh begins, as arc along its lead
    /// ([`ShapeGraph::limb_membership`]'s `begin`) — `None` for a limb no bone is seated in.
    begin: Vec<Option<f32>>,
}

impl<'a> Tubes<'a> {
    /// READ THE TUBES OFF THIS BODY: its body-masked [`Flesh`] and the [`ShapeGraph`] thinned from
    /// it — `body`, the read the fit itself made (`conform::fit_baseline_to_mesh`) off this mesh.
    ///
    /// A limb's OWN run starts where its lead clears every ball of the core it hangs off
    /// ([`clear_of_core`]): before that the lead is inside the trunk. A bone is SEATED in a limb
    /// when it was LAID down that limb — the fit puts a chain's joints on the limb's own path, so
    /// the far end of such a bone sits inside that run's balls and its middle inside the lead's
    /// ([`on_limb`]), even where the lead is still inside the core — or, off every path (a finger,
    /// a toe, a jaw), when the middle of it is that limb's flesh rather than the trunk's. A bone the grid cannot read at all — thinner than a cell, or hanging in the
    /// air — follows its parent. Only deforming bones are seated. A module nothing matched keeps
    /// its composed rest, rarely on any limb's path, and stays with the trunk as it always was.
    ///
    /// A seated limb's flesh BEGINS where its root-most seated bone's joint projects onto its lead
    /// — the SOCKET the fit placed it by — and never inside the core: the junction band is centred
    /// on the socket, not on the junction deep in the trunk.
    ///
    /// `None` when the mesh has no graph or no bone is seated in any limb of it.
    fn read(
        model: &RawModel,
        heads: &[Vec3],
        tails: &[Vec3],
        deform: &[bool],
        body: &'a Body,
    ) -> Option<Tubes<'a>> {
        let (flesh, graph) = (&body.flesh, body.graph.as_ref()?);
        let own: Vec<f32> = graph
            .limbs
            .iter()
            .map(|l| clear_of_core(graph, l))
            .collect();
        let own_runs: Vec<Option<f32>> = own.iter().copied().map(Some).collect();
        let mut seat: Vec<Option<usize>> = vec![None; heads.len()];
        for (i, b) in model.bones.iter().enumerate() {
            if !deform[i] {
                continue;
            }
            let mid = 0.5 * (heads[i] + tails[i]);
            // LAID: the far end on the limb's own run, the middle anywhere along its lead — a
            // raised thigh buried in a woolly or crouched body runs down the lead while it is
            // still inside the core, and is the limb's bone all the same.
            let laid = (0..graph.limbs.len())
                .filter_map(|l| {
                    let limb = &graph.limbs[l];
                    on_limb(limb, own[l], tails[i])?;
                    Some((l, on_limb(limb, 0.0, mid)?))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(l, _)| l);
            seat[i] = if laid.is_some() {
                laid
            } else if flesh.contains(mid) {
                graph
                    .limb_membership(mid, &own_runs)
                    .filter(|&(_, share)| share >= 0.5)
                    .map(|(l, _)| l)
            } else {
                usize::try_from(b.parent)
                    .ok()
                    .filter(|&p| p < i)
                    .and_then(|p| seat[p])
            };
        }
        // A JUNCTION is the trunk's. A bone whose children go into DIFFERENT parts of the body —
        // one down a limb, one into the trunk or another limb — is where those parts meet, however
        // thin the flesh round it reads: a pelvis whose hindquarters thin into the legs' own flesh
        // still carries the spine, both thighs and the tail, and seated in one leg it would leave
        // the other side of the rump with nothing to follow.
        let n = heads.len();
        let parent_of = |i: usize| {
            usize::try_from(model.bones[i].parent)
                .ok()
                .filter(|&p| p < n)
        };
        let starts_a_chain: Vec<bool> = {
            let mut has_kids = vec![false; n];
            for i in 0..n {
                if let Some(p) = parent_of(i) {
                    has_kids[p] = true;
                }
            }
            has_kids
        };
        // Only children that START A CHAIN count as branches — a twist helper or a leaf rides the
        // segment it hangs on and is no fork.
        let mut parts: Vec<Vec<Option<usize>>> = vec![Vec::new(); n];
        for i in 0..n {
            if let Some(p) = parent_of(i) {
                if deform[i] && starts_a_chain[i] && !parts[p].contains(&seat[i]) {
                    parts[p].push(seat[i]);
                }
            }
        }
        for i in 0..n {
            if parts[i].len() > 1 {
                seat[i] = None;
            }
        }
        let parents: Vec<Option<usize>> = (0..n).map(parent_of).collect();
        let chained: Vec<bool> = (0..n).map(|i| deform[i] && starts_a_chain[i]).collect();
        settle_chains(
            &mut seat,
            &parents,
            &chained,
            deform,
            &|l| stands(graph, l),
            &|l, i| heads[i].distance(graph.limbs[l].tip()),
        );
        // AN APPENDAGE SEATS NO BONE. A limb that does not stand and whose lead runs on BARE
        // ([`bare_end`]) past its bones for more than [`APPENDAGE_SHARE`] of its own length is
        // not those bones' limb — it grows out of the flesh they carry. A pair of antlers leaves
        // a thick-necked body as two tubes off the trunk's front end, the neck and head bones
        // lie along the stem of one of them, and seated there they owned that one antler and
        // nothing else: the skull itself was left to the trunk's bones (measured on three
        // horned sources: no weight at all on the head bone within 7 cm of it). Unseated, they
        // are the trunk's again and the skull is theirs by the plain reading.
        let boned = |c: Vec3, r: f32| holds(c, r, heads, tails, deform).next().is_some();
        for (l, limb) in graph.limbs.iter().enumerate() {
            if limb.sheet || stands(graph, l) || !seat.contains(&Some(l)) {
                continue;
            }
            let Some(run) = bare_end(graph, &limb.lead, &limb.lead_r, &boned) else {
                continue;
            };
            let bare = run.boneless;
            if bare >= BARE_RADII * run.boneless_thick
                && bare > APPENDAGE_SHARE * (limb.arc - own[l])
            {
                for s in seat.iter_mut().filter(|s| **s == Some(l)) {
                    *s = None;
                }
            }
        }
        let mut begin: Vec<Option<f32>> = vec![None; graph.limbs.len()];
        for (i, s) in seat.iter().enumerate() {
            let Some(l) = *s else {
                continue;
            };
            let at = arc_on(&graph.limbs[l].lead, heads[i]).max(own[l]);
            begin[l] = Some(begin[l].map_or(at, |b| b.min(at)));
        }
        if begin.iter().all(Option::is_none) {
            return None;
        }
        tracing::debug!(
            "bake_skin: {} bone(s) seated in {} limb(s) of {}",
            seat.iter().flatten().count(),
            begin.iter().flatten().count(),
            graph.summary()
        );
        Some(Tubes { graph, seat, begin })
    }

    /// THE LIMBS THAT CARRY A GIRDLE ([`Girdles`]), each with the side of the plane it hangs on
    /// (±1) and how thick its tube is where its own flesh begins (cm): the limbs that STAND
    /// ([`stands`]) under a LYING core, with bones seated in them, off the midline.
    fn girdled(&self, heads: &[Vec3]) -> Vec<(usize, f32, f32)> {
        let graph = self.graph;
        let mut limbs = Vec::new();
        for (l, limb) in graph.limbs.iter().enumerate() {
            let lying = graph.cores.get(limb.core).is_some_and(|c| !c.upright);
            if !(stands(graph, l) && lying) {
                continue;
            }
            let (mut x, mut bones) = (0.0_f32, 0usize);
            for (i, _) in self.seat.iter().enumerate().filter(|s| *s.1 == Some(l)) {
                x += heads[i].x;
                bones += 1;
            }
            if bones == 0 {
                continue;
            }
            let off = x / bones as f32 - graph.plane_x;
            if off.abs() < graph.cell {
                continue;
            }
            // How thick the limb is where its own flesh begins: its lead's radius there.
            let (mut arc, mut thick) = (0.0_f32, graph.cell);
            for (k, (c, r)) in limb.lead.iter().zip(&limb.lead_r).enumerate() {
                if k > 0 {
                    arc += limb.lead[k - 1].distance(*c);
                }
                if arc >= self.begin[l].unwrap_or(0.0) {
                    thick = r.max(graph.cell);
                    break;
                }
            }
            limbs.push((l, off.signum(), thick));
        }
        limbs
    }
}

/// SETTLE WHICH CHAIN EACH STANDING LIMB CARRIES, after every bone's own read ([`Tubes::read`]).
/// `seat` is per bone, `parents` its parent, `chained` whether it deforms and starts a chain
/// (has children — a twist helper or a leaf rides the segment it hangs on), `stands` whether a
/// limb stands ([`stands`]), `to_tip` how far a bone's joint is from a limb's tip.
///
/// A STANDING LIMB'S CHAIN SEATS AS ONE. A bone whose chain goes on only into a limb that
/// stands is that limb's bone WHATEVER ITS OWN READ SAID. The trunk, where it lies inside the
/// body: a crouched squirrel's thigh, tucked into its haunch, is where the graph's limb has not
/// yet begun, and left with the trunk it owns the haunch and swings it through the floor when
/// the leg is squared; so is a quadruped's shoulder bone over its foreleg. Or another limb
/// altogether, where the shape gave the flesh round it a tube of its own: a hump over the
/// withers, a beard, the bulge of an upper arm thinned apart from the forearm under it
/// (measured on the 17 hoofed sources: 6 of 68 leg chains were split across two limbs or had
/// lost their root to one, and 2 more had lost it to the trunk under a raised foot). A junction
/// — children in different parts — is nobody's, and an arm that never reaches the ground keeps
/// its root with the trunk: a biped's shoulder shrugs the flesh round it. Children before
/// parents, so a chain seats root-ward in one pass.
///
/// A LEAF is the end of its parent's chain — a twist helper riding a thigh, an eye in a head —
/// and sits where its parent sits.
///
/// ONE STANDING LIMB, ONE CHAIN. A tail that hangs against a hind leg reads as that leg's flesh
/// and its bones as laid down the leg (4 of the 17 hoofed sources), and a leg that carries a
/// tail's bones takes the tail's root with it at every stride. The limb is the chain that
/// reaches its TIP: a bone seated in it that does not hang, through bones seated in it, off the
/// same root as the bone nearest the tip is the trunk's.
fn settle_chains(
    seat: &mut [Option<usize>],
    parents: &[Option<usize>],
    chained: &[bool],
    deform: &[bool],
    stands: &dyn Fn(usize) -> bool,
    to_tip: &dyn Fn(usize, usize) -> f32,
) {
    let n = seat.len();
    for i in (0..n).rev().filter(|&i| chained[i]) {
        let mut chain = (0..n)
            .filter(|&c| parents[c] == Some(i) && chained[c])
            .map(|c| seat[c]);
        if let Some(Some(l)) = chain.next() {
            if chain.all(|s| s == Some(l)) && stands(l) {
                seat[i] = Some(l);
            }
        }
    }
    for i in 0..n {
        if deform[i] && !chained[i] {
            if let Some(p) = parents[i].filter(|&p| seat[p].is_some()) {
                seat[i] = seat[p];
            }
        }
    }
    let limbs = seat.iter().flatten().max().map_or(0, |l| l + 1);
    for l in (0..limbs).filter(|&l| stands(l)) {
        let root_of = |seat: &[Option<usize>], mut i: usize| {
            while let Some(p) = parents[i].filter(|&p| seat[p] == Some(l)) {
                i = p;
            }
            i
        };
        let Some(root) = (0..n)
            .filter(|&i| seat[i] == Some(l))
            .min_by(|&a, &b| to_tip(l, a).total_cmp(&to_tip(l, b)))
            .map(|i| root_of(seat, i))
        else {
            continue;
        };
        let strays: Vec<usize> = (0..n)
            .filter(|&i| seat[i] == Some(l) && root_of(seat, i) != root)
            .collect();
        for i in strays {
            seat[i] = None;
        }
    }
}

/// A bare end is an APPENDAGE — a horn, an antler, an ear, a tusk — when it is at least this
/// many times as long as it is thick where it begins. A muzzle past its jaw, a hoof past its
/// last joint, the crown of a skull are bare too, and are not: they are the end of the flesh
/// their bones carry.
const BARE_RADII: f32 = 4.0;
/// A limb whose lead is an appendage for more than this share of its own run seats no bone
/// ([`Tubes::read`]).
const APPENDAGE_SHARE: f32 = 0.5;
/// The balls at a bare run's ROOT its bone is read against ([`rigid_ends`]).
const BARE_ROOT: usize = 3;

/// The deforming bones whose own segment passes through the medial ball at `c` of radius `r`
/// (let out by [`LAID_SLACK`]) — the bones that ball's flesh has in it.
fn holds<'a>(
    c: Vec3,
    r: f32,
    heads: &'a [Vec3],
    tails: &'a [Vec3],
    deform: &'a [bool],
) -> impl Iterator<Item = usize> + 'a {
    (0..heads.len()).filter(move |&i| {
        deform[i] && c.distance(closest_point_segment(c, heads[i], tails[i])) <= LAID_SLACK * r
    })
}

/// THE BARE END OF A TUBE: the run of a path's balls past the mass its bones hold out to its
/// tip, once the path is clear of every core — flesh no bone was laid down. A BONE HOLDS ITS
/// MASS: from the last ball a bone's segment passes through (`boned`), the hold runs on along
/// the path while the flesh stays at least half ([`crate::shape::CORE_FRACTION`]) as thick as
/// that ball, for up to [`BARE_RADII`] of it — the crown of a skull whose head joint sits at
/// its base is the head's, not the horn's that grows out of it (read from the last boned ball
/// alone, one horn of a real source took the poll between the horns with it, 17 000 vertices
/// against 9 000, and read FLAT for it, 2026-10-08); the hair past a tail's last bone, as thin
/// as the dock but far longer than four of it, is still bare. Whether a run makes an
/// APPENDAGE ([`BARE_RADII`]) is the caller's call: [`rigid_ends`] reads the run past the
/// held mass (and admits the MIRROR of one); [`Tubes::read`]'s unseat reads the run from the
/// last boned ball, mass and all — how much of a limb its bones were laid in.
fn bare_end(
    graph: &ShapeGraph,
    path: &[Vec3],
    radii: &[f32],
    boned: &dyn Fn(Vec3, f32) -> bool,
) -> Option<BareRun> {
    let in_a_core = |p: &Vec3| {
        graph
            .cores
            .iter()
            .flat_map(|k| k.path.iter().zip(&k.radii))
            .any(|(c, r)| p.distance(*c) <= *r)
    };
    let clear = path.iter().position(|p| !in_a_core(p))?;
    let n = path.len().min(radii.len());
    let last = (0..n).rev().find(|&k| boned(path[k], radii[k]));
    let held = last.map(|k| {
        let mass = radii[k];
        let (mut end, mut travelled) = (k, 0.0);
        for j in k + 1..n {
            travelled += path[j - 1].distance(path[j]);
            if radii[j] < crate::shape::CORE_FRACTION * mass || travelled > BARE_RADII * mass {
                break;
            }
            end = j;
        }
        end
    });
    let run = |start: usize| {
        let thick = radii.get(start)?.max(graph.cell);
        let length: f32 = path[start.saturating_sub(1)..]
            .windows(2)
            .map(|w| w[0].distance(w[1]))
            .sum();
        Some((start, length, thick))
    };
    let (start, length, thick) = run(held.map_or(clear, |k| (k + 1).max(clear)))?;
    let (_, boneless, boneless_thick) = run(last.map_or(clear, |k| (k + 1).max(clear)))?;
    Some(BareRun {
        start,
        length,
        thick,
        last,
        held,
        boneless,
        boneless_thick,
    })
}

/// A path's bare run ([`bare_end`]).
#[derive(Clone, Copy, Debug)]
struct BareRun {
    /// Its first ball — past the mass the bones hold.
    start: usize,
    /// Its length from there (cm) and the flesh's thickness where it begins.
    length: f32,
    thick: f32,
    /// The last ball a bone's segment passes through, if any, and the last ball its hold
    /// runs on to through the mass: the run begins past the latter, and grows out of that
    /// mass when the hold ran on at all.
    last: Option<usize>,
    held: Option<usize>,
    /// The run read from that ball instead, mass and all: its length and thickness.
    boneless: f32,
    boneless_thick: f32,
}

/// A bare run too short to be an appendage on its own is one when a run accepted as one
/// MIRRORS it across the body's plane — its balls' centroid reflected within this share of
/// the longer run's length (plus two cells) of the other's, and it at least half as long: a
/// pair of ears is two ears, however the grid thinned each side (one side of a real source
/// read 16 cm and the other 10, against a 13-cm bar).
const MIRROR_SHARE: f32 = 0.25;

/// How many directions round a path [`cross_section`] reads the flesh in.
const CROSS_READS: usize = 8;
/// Flesh this many times thicker than the part being read is another part's.
const CROSS_THICKEN: f32 = 2.0;

/// HOW FLAT THE FLESH IS ACROSS A PATH at `at`: its extent through `at` in each of
/// [`CROSS_READS`] directions square to the path (`along`), broadest over thinnest — about 1
/// for a round tube, several for a sheet. Read off the flesh itself, as the graph's own sheet
/// test is: a plate thins to one curve or to a comb of them as its thickness falls, and the
/// distance from a skin to its nearest curve says nothing of which it was.
fn cross_section(flesh: &Flesh, at: Vec3, along: Vec3) -> f32 {
    let Some(axis) = along.try_normalize() else {
        return 1.0;
    };
    let (u, v) = axis.any_orthonormal_pair();
    let step = 0.5 * flesh.cell();
    // How far THIS part's flesh runs from `at` one way, a half-cell at a time: to the skin, or
    // to where the flesh thickens past [`CROSS_THICKEN`] of what it is here — a horn curled
    // against a cheek is not as broad as the head it lies on.
    let own = CROSS_THICKEN * flesh.radius_at(at).max(flesh.cell());
    let reach = |dir: Vec3| {
        (1..400)
            .map(|k| k as f32 * step)
            .take_while(|d| {
                let q = at + dir * *d;
                flesh.contains(q) && flesh.radius_at(q) <= own
            })
            .last()
            .unwrap_or(0.0)
    };
    let (mut thin, mut broad) = (f32::INFINITY, 0.0_f32);
    for k in 0..CROSS_READS {
        let turn = std::f32::consts::PI * k as f32 / CROSS_READS as f32;
        let dir = u * turn.cos() + v * turn.sin();
        let extent = reach(dir) + reach(-dir) + step;
        thin = thin.min(extent);
        broad = broad.max(extent);
    }
    broad / thin.max(step)
}

/// BARE ENDS ARE RIGID (Aaron on the horned bodies, 2026-10-04: *"the horns pick up weight
/// from the bones … we need to ensure the horns don't get skewed from the weighting, there are
/// artifacts visually of solid planes"*). A horn, an antler, an ear has no bone of its own, and
/// read by distance its flesh is shared out among whatever bones lie nearest — a tine over the
/// neck to the neck, one over the withers to the spine, one by the face to an eye — so every
/// pose that turns those bones against each other shears it into sheets (measured: no vertex
/// of one body's antlers wholly on one bone, eight bones sharing them).
///
/// Stated on the shape graph and the skeleton alone: every path of every limb that does not
/// stand (and is no boned sheet — a wing) has its BARE END ([`bare_end`]), and the flesh of an appendage's
/// (in a limb with a chain of its own, only one past the END of that chain) — every position
/// whose nearest medial ball, of all the body's, is one of that run's — goes WHOLLY to ONE
/// bone: the one it grows from. That is the bone whose segment comes nearest the run's own
/// balls, or the bone its chain runs on into that comes as near (a head, over the neck bone
/// that ends at the head's own joint); never a bone of another limb that stands (a tail that
/// falls beside a hind leg is not the leg's); a leaf among siblings counts as their parent
/// (an eye, a jaw are the head's).
///
/// Per position: the bone it is rigid with, how many times broader than thin the flesh is there
/// ([`cross_section`]) and which RUN it is on — one id per connected bare subtree of a limb's
/// paths, so an antler's tines are their beam's and a fall of hair's lobes are one fall's, while
/// two horns whose paths part on the boned head are two; `None` for every position of no
/// appendage.
fn rigid_ends(
    tubes: &Tubes,
    flesh: &Flesh,
    model: &RawModel,
    heads: &[Vec3],
    tails: &[Vec3],
    deform: &[bool],
    positions: &[Vec3],
) -> Vec<Option<(usize, f32, u32)>> {
    let (graph, seat) = (tubes.graph, tubes.seat.as_slice());
    let n = heads.len();
    let parent = |i: usize| {
        usize::try_from(model.bones[i].parent)
            .ok()
            .filter(|&p| p < n)
    };
    let mut kids = vec![0usize; n];
    for p in (0..n).filter_map(parent) {
        kids[p] += 1;
    }
    let kids = &kids;
    // A leaf among siblings rides its parent; every other bone stands for itself.
    let stands_for = |i: usize| match parent(i) {
        Some(p) if kids[i] == 0 && kids[p] > 1 && deform[p] => p,
        _ => i,
    };
    let to = |i: usize, c: Vec3| c.distance(closest_point_segment(c, heads[i], tails[i]));
    let boned = |c: Vec3, r: f32| holds(c, r, heads, tails, deform).next().is_some();
    let key = |c: Vec3| c.to_array().map(f32::to_bits);
    // Every ball of an appendage's bare end, with the bone it is rigid with, how flat the flesh
    // is across it and the run it is on: the path that first claimed it, and two paths that
    // share a bare ball are one run.
    let mut bare: HashMap<[u32; 3], (Vec3, f32, usize, f32, u32)> = HashMap::new();
    // A sheet with bones laid down it is a wing, and the wing's to fold; a sheet with none is
    // an ear.
    let paths: Vec<(usize, &Vec<Vec3>, &Vec<f32>)> = graph
        .limbs
        .iter()
        .enumerate()
        .filter(|(l, limb)| !(limb.sheet && seat.contains(&Some(*l))))
        .flat_map(|(l, limb)| {
            std::iter::once((&limb.lead, &limb.lead_r))
                .chain(limb.fan.iter().map(|(p, r)| (p, r)))
                .map(move |(path, radii)| (l, path, radii))
        })
        .collect();
    let mut same: Vec<u32> = (0..paths.len() as u32).collect();
    fn run_of(same: &mut [u32], mut x: u32) -> u32 {
        while same[x as usize] != x {
            same[x as usize] = same[same[x as usize] as usize];
            x = same[x as usize];
        }
        x
    }
    // Every path's bare run, and which are APPENDAGES: long enough for their thickness, or
    // the mirror of one that is. A limb that stands is its own chain's from socket to sole —
    // its toes, its feathering and whatever hangs against it are bound as its flesh always was.
    let runs: Vec<Option<BareRun>> = paths
        .iter()
        .map(|&(l, path, radii)| {
            if stands(graph, l) {
                None
            } else {
                bare_end(graph, path, radii, &boned)
            }
        })
        .collect();
    let centroid = |run: usize| {
        let start = runs[run]?.start;
        let balls = &paths[run].1[start..];
        Some(balls.iter().copied().sum::<Vec3>() / balls.len().max(1) as f32)
    };
    let long_enough: Vec<bool> = runs
        .iter()
        .map(|r| r.is_some_and(|r| r.length >= BARE_RADII * r.thick))
        .collect();
    let mirrored = |i: usize| {
        let (Some(ri), Some(ci)) = (runs[i], centroid(i)) else {
            return false;
        };
        (0..runs.len()).filter(|&j| long_enough[j]).any(|j| {
            let (Some(rj), Some(cj)) = (runs[j], centroid(j)) else {
                return false;
            };
            let (len_i, len_j) = (ri.length, rj.length);
            let tol = MIRROR_SHARE * len_i.max(len_j) + 2.0 * graph.cell;
            len_i >= 0.5 * len_j
                && (ci.x + cj.x - 2.0 * graph.plane_x).abs() <= tol
                && (ci.y - cj.y).abs() <= tol
                && (ci.z - cj.z).abs() <= tol
        })
    };
    let appendage: Vec<bool> = (0..runs.len())
        .map(|i| long_enough[i] || mirrored(i))
        .collect();
    for (run, &(l, path, radii)) in paths.iter().enumerate() {
        let Some(BareRun {
            start, last, held, ..
        }) = runs[run].filter(|_| appendage[run])
        else {
            continue;
        };
        // The bone it grows from: the one whose segment comes nearest the run's ROOT, its
        // first balls ([`BARE_ROOT`]) — of the bones that HOLD the mass it grows out of, when
        // it grows out of one (the hold ran on past the last ball a segment passes through:
        // a tusk out of the head's face, a horn out of its crown), else of every bone of the
        // body. Never a bone of a limb that STANDS on its own (a tail's fall beside a hind
        // leg is not the leg's, however near it hangs), unless the run is that limb's own.
        // Read against every bone, a tusk went to the trunk's chain it hangs beside instead
        // of the head it grows from.
        let grows = |i: usize| deform[i] && seat[i].is_none_or(|s| s == l || !stands(graph, s));
        let mass = last.zip(held).filter(|(k, h)| h > k).map(|(k, _)| k);
        let holders: Vec<usize> = mass
            .map(|k| {
                (0..n)
                    .filter(|&i| {
                        grows(i) && holds(path[k], radii[k], heads, tails, deform).any(|b| b == i)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let from = |i: usize| grows(i) && (holders.is_empty() || holders.contains(&i));
        let gap = |i: usize| {
            path[start..]
                .iter()
                .zip(&radii[start..])
                .take(BARE_ROOT)
                .map(|(c, r)| to(i, *c) - r)
                .fold(f32::INFINITY, f32::min)
        };
        let Some(mut bone) = (0..n)
            .filter(|&i| from(i))
            .min_by(|&a, &b| gap(a).total_cmp(&gap(b)))
        else {
            continue;
        };
        // ...and on down its chain while the one bone the chain CARRIES ON INTO — the child
        // its own segment ends at — comes as near (within a cell): a head, over the neck bone
        // that ends at the head's own joint; never a chain hung off it beside the run, as a
        // trunk's is beside a tusk — the head's segment ends among its children, not in it.
        let carried_on = |b: usize| {
            (0..n).filter(move |&c| {
                parent(c) == Some(b) && kids[c] > 0 && tails[b].distance(heads[c]) <= graph.cell
            })
        };
        loop {
            let mut on = carried_on(bone).filter(|&c| grows(c));
            match (on.next(), on.next()) {
                (Some(next), None) if gap(next) <= gap(bone) + graph.cell => bone = next,
                _ => break,
            }
        }
        // A limb with a chain of its own is bare only PAST THE END of that chain: a horn on
        // the head a neck's bones end in, the tuft past a tail's last bone. Flesh beside a bone
        // the chain runs on from — the feathers along a folded wing's forearm — is that
        // chain's to bend.
        if seat.contains(&Some(l)) && carried_on(bone).next().is_some() {
            continue;
        }
        let bone = stands_for(bone);
        if let Some(BareRun { length, thick, .. }) = runs[run] {
            tracing::debug!(
                "bare run {run} of limb {l}: balls {start}..{}, {length:.0} cm, {thick:.1} thick, on {}{}",
                path.len().min(radii.len()),
                model.bones[bone].name,
                if long_enough[run] { "" } else { " (its mirror's)" }
            );
        }
        for k in start..path.len().min(radii.len()) {
            let along = path[(k + 1).min(path.len() - 1)] - path[k.saturating_sub(1)];
            match bare.entry(key(path[k])) {
                std::collections::hash_map::Entry::Occupied(shared) => {
                    let (a, b) = (
                        run_of(&mut same, shared.get().4),
                        run_of(&mut same, run as u32),
                    );
                    same[a.max(b) as usize] = a.min(b);
                }
                std::collections::hash_map::Entry::Vacant(own) => {
                    own.insert((
                        path[k],
                        radii[k],
                        bone,
                        cross_section(flesh, path[k], along),
                        run as u32,
                    ));
                }
            }
        }
    }
    if bare.is_empty() {
        return vec![None; positions.len()];
    }
    for ball in bare.values_mut() {
        ball.4 = run_of(&mut same, ball.4);
    }
    // Every other ball of the body: the cores', and every path's that is no appendage's.
    let mut rest: HashMap<[u32; 3], (Vec3, f32)> = HashMap::new();
    let cores = graph.cores.iter().flat_map(|k| k.path.iter().zip(&k.radii));
    let limbs = graph.limbs.iter().flat_map(|l| {
        std::iter::once((&l.lead, &l.lead_r))
            .chain(l.fan.iter().map(|(p, r)| (p, r)))
            .flat_map(|(p, r)| p.iter().zip(r))
    });
    for (c, r) in cores.chain(limbs) {
        if !bare.contains_key(&key(*c)) {
            rest.insert(key(*c), (*c, *r));
        }
    }
    // In a fixed order: two balls a position is equally near must answer alike every run.
    let mut bare: Vec<_> = bare.into_iter().collect();
    bare.sort_unstable_by_key(|b| b.0);
    let bare: Vec<(Vec3, f32, usize, f32, u32)> = bare.into_iter().map(|b| b.1).collect();
    let rest: Vec<(Vec3, f32)> = rest.into_values().collect();
    let out: Vec<Option<(usize, f32, u32)>> = positions
        .iter()
        .map(|&p| {
            let (own, bone, flat, run) = bare
                .iter()
                .map(|(c, r, bone, flat, run)| (p.distance(*c) - r, *bone, *flat, *run))
                .min_by(|a, b| a.0.total_cmp(&b.0))?;
            rest.iter()
                .all(|(c, r)| p.distance(*c) - r > own)
                .then_some((bone, flat, run))
        })
        .collect();
    out
}

/// A LIMB STANDS when it reaches the ground or its twin across the plane does: a foot raised
/// mid-stride is a pose, not another kind of limb (the reading `conform` makes of a pair) — and
/// the raised limb is the one the stance normaliser swings.
fn stands(graph: &ShapeGraph, l: usize) -> bool {
    let grounded = |k: usize| graph.limbs.get(k).is_some_and(|limb| limb.grounded);
    grounded(l)
        || graph
            .pairs
            .iter()
            .any(|p| (p.l == l && grounded(p.r)) || (p.r == l && grounded(p.l)))
}

/// THE GIRDLES — the trunk flesh a limb's BURIED bones carry (Aaron on the Elk, 2026-10-02: *"the
/// blending and weighting on the mesh only touches the tubular portions of the mesh, there's zero
/// motion in the torso or shoulders"*). A limb under a LYING trunk does not begin where its tube
/// leaves the body: a femur runs from a hip under the croup forward through the haunch, a scapula
/// and a humerus lie across the ribs, and the flesh over them is the limb's — it is what moves
/// when the leg does. The tube bind gave that flesh to the spine (rightly, for un-posing a
/// stride: [`bind`]), and a walk then swung four tubes under a statue. [`bind_for_motion`].
///
/// Stated on shape and chain alone: a limb's girdle is a SLEEVE of the trunk's flesh round that
/// limb's own bones, as thick as the limb is where it leaves the body — a BELL about each bone,
/// whole on the bone itself and falling smoothly to nothing at [`GIRDLE_SLEEVE_RADII`] of the
/// tube's radii from it ((1 − (d/R)²)²: 0.79 of the flesh one radius out, 0.31 at two) — and
/// only on the limb's own side of its bones: none on the midline (a spine's ridge, a sternum, a
/// tail's root are the trunk's), all of it from the bones' own depth outward. So the sleeve is
/// where the bones are, and a bone a human drags onto the bulge he can see takes its sleeve
/// with it. The bell has no plateau on purpose: the first cut held the flesh WHOLE within one
/// radius, and a ball of shoulder turned rigidly with the scapula (Aaron, 2026-10-08: *"a
/// clear sphere around the joints that is rotating directly in line with the joint rotation …
/// almost a 1:1 quaternion rotation"*) — the flesh over a buried bone is shared with the
/// trunk's bones all the way in. (Measured and dropped: "nearer the limb's bones than the
/// trunk's". A trunk's bones run down its core's centreline, a deep body's back is a core
/// radius from them, and a scapula's top then owned the whole hump over it — 70 % of a boar
/// followed its legs.)
///
/// And only flesh the limb's bones can SEE ([`Self::sees`]): the straight line from it to the
/// nearest of them stays inside the body. Distance alone hands a scapula the tine of an antler that reaches
/// back over the shoulder, a beard and a trunk-nose hanging in front of the forelegs, an ear —
/// each nearer a limb's bone than any of the trunk's, and none of them the limb's flesh.
///
/// Only limbs that STAND ([`stands`]) under a LYING core, and only limbs off the midline:
/// an upright trunk's legs begin where it ends and its arms hang from shoulders the trunk
/// shrugs (the humanoid canon's skin is its own, validated against its clip libraries).
struct Girdles<'a> {
    flesh: &'a Flesh,
    heads: &'a [Vec3],
    tails: &'a [Vec3],
    /// Per girdled limb: the graph limb, the side of the plane it hangs on (±1), its bones,
    /// and how thick its tube is where it begins (cm).
    limbs: Vec<(usize, f32, Vec<usize>, f32)>,
    plane_x: f32,
    cell: f32,
}

/// A girdle's sleeve fades to nothing this many of its limb's own radii (the tube's, where its
/// flesh begins) from the limb's bones — a bell, whole only on the bone itself.
const GIRDLE_SLEEVE_RADII: f32 = 3.0;
/// A share under this is no girdle at all — the tail of the diffusion, not a limb's flesh.
const GIRDLE_LEAST: f32 = 0.02;

impl<'a> Girdles<'a> {
    /// The girdles `seating` names, round its limbs' bones as they now lie (`heads`, `tails`)
    /// and through the flesh as it now stands.
    fn of(seating: &Seating, flesh: &'a Flesh, heads: &'a [Vec3], tails: &'a [Vec3]) -> Self {
        let limbs = seating
            .girdled
            .iter()
            .map(|&(l, side, thick)| {
                let bones = (0..heads.len())
                    .filter(|&i| seating.seat[i] == Some(l))
                    .collect();
                (l, side, bones, thick)
            })
            .collect();
        Girdles {
            flesh,
            heads,
            tails,
            limbs,
            plane_x: seating.plane_x,
            cell: seating.cell,
        }
    }

    /// HOW WELL THE BONE POINT `at` SEES `p`: 1 where the straight line between them stays inside
    /// the flesh for nine tenths of its length (read a cell at a time, from a cell in — a vertex
    /// sits ON the skin), 0 where half of it is in the air, a ramp between. A ramp and not a
    /// test: over wool and shag one tuft's line clips the air and its neighbour's does not, and
    /// a yes/no there is a whole limb's swing between two vertices of one triangle.
    fn sees(&self, p: Vec3, at: Vec3) -> f32 {
        let steps = (p.distance(at) / self.cell).ceil() as usize;
        if steps < 3 {
            return 1.0;
        }
        let inside = (1..steps)
            .filter(|&q| self.flesh.contains(p.lerp(at, q as f32 / steps as f32)))
            .count();
        ((inside as f32 / (steps - 1) as f32 - 0.5) / 0.4).clamp(0.0, 1.0)
    }

    /// Each girdled limb's share of `p` (in [`Self::limbs`]' order), written into `row`.
    fn shares(&self, p: Vec3, row: &mut [f32]) {
        let nearest = |bones: &[usize]| {
            bones
                .iter()
                .map(|&i| closest_point_segment(p, self.heads[i], self.tails[i]))
                .map(|c| (p.distance(c), c))
                .min_by(|a, b| a.0.total_cmp(&b.0))
        };
        for (share, (_, side, bones, thick)) in row.iter_mut().zip(&self.limbs) {
            let Some((limb, at)) = nearest(bones) else {
                continue;
            };
            let rho = limb / (GIRDLE_SLEEVE_RADII * thick).max(self.cell);
            if rho >= 1.0 {
                continue;
            }
            let ramp = (1.0 - rho * rho).powi(2);
            let depth = (side * (at.x - self.plane_x)).max(self.cell);
            let out = (side * (p.x - self.plane_x) / depth).clamp(0.0, 1.0);
            *share = ramp * out * self.sees(p, at);
        }
    }
}

/// How far past its own inscribed radius a joint may sit and still be ON a limb's path — a
/// socket two sides of a pair share sits between their two tubes' starts (`conform::pair_socket`),
/// and a limb's tip is traced a radius on past the last point thinning left (`conform::fit_to_graph`).
const LAID_SLACK: f32 = 1.25;

/// WHERE A LIMB'S OWN RUN STARTS as the graph reads it: the arc along its lead of the first point
/// outside every ball of the core it hangs off. The lead starts at a junction inside the trunk, and
/// everything before this point is trunk flesh however thin the class split called it.
fn clear_of_core(graph: &ShapeGraph, limb: &crate::shape::Limb) -> f32 {
    let Some(core) = graph.cores.get(limb.core) else {
        return 0.0;
    };
    let mut arc = 0.0;
    for (k, p) in limb.lead.iter().enumerate() {
        if k > 0 {
            arc += limb.lead[k - 1].distance(*p);
        }
        if core
            .path
            .iter()
            .zip(&core.radii)
            .all(|(c, r)| p.distance(*c) > *r)
        {
            return arc;
        }
    }
    arc
}

/// Is `p` ON `limb`'s own run — inside one of its balls past `from` (arc along the lead, the fan cut
/// at the same arc), each let out by [`LAID_SLACK`]? `Some` with how deep inside the nearest one
/// (negative is inside), `None` when it is on none of them.
fn on_limb(limb: &crate::shape::Limb, from: f32, p: Vec3) -> Option<f32> {
    let mut best: Option<f32> = None;
    for (path, radii) in
        std::iter::once((&limb.lead, &limb.lead_r)).chain(limb.fan.iter().map(|(a, b)| (a, b)))
    {
        let mut arc = 0.0;
        for (k, (c, r)) in path.iter().zip(radii).enumerate() {
            if k > 0 {
                arc += path[k - 1].distance(*c);
            }
            let d = p.distance(*c) - LAID_SLACK * r;
            if arc >= from && d <= 0.0 && best.is_none_or(|b| d < b) {
                best = Some(d);
            }
        }
    }
    best
}

/// Where along `path` the point on it nearest `p` lies, in cm of arc from its start.
fn arc_on(path: &[Vec3], p: Vec3) -> f32 {
    let (mut best, mut at, mut run) = (f32::INFINITY, 0.0, 0.0);
    for w in path.windows(2) {
        let q = closest_point_segment(p, w[0], w[1]);
        let d = p.distance_squared(q);
        if d < best {
            best = d;
            at = run + w[0].distance(q);
        }
        run += w[0].distance(w[1]);
    }
    at
}

/// PUT THE PARTITION BACK after the Laplacian: every position keeps its OWN limb's share on that
/// limb's bones and the rest on the trunk's, and no other limb's bone keeps anything. The blend
/// the smoothing made INSIDE each part stands (a knee inside a leg, a hip inside the trunk); what
/// it carried ACROSS a junction — limb weight smeared onto the belly, trunk weight into the foot,
/// one leg's weight onto its twin where their flesh touches — is taken back out. A pinned region
/// keeps its pin.
fn keep_to_the_tubes(
    weights: &mut [f32],
    n: usize,
    member: &[Option<(usize, f32)>],
    seat: &[Option<usize>],
    pinned: &[bool],
) {
    for (pi, m) in member.iter().enumerate() {
        if pinned[pi] {
            continue;
        }
        let row = &mut weights[pi * n..(pi + 1) * n];
        let (own, share) = m.map_or((None, 0.0), |(l, s)| (Some(l), s));
        let (mut limb, mut trunk) = (0.0_f32, 0.0_f32);
        for (b, w) in row.iter_mut().enumerate() {
            match seat[b] {
                None => trunk += *w,
                s if s == own => limb += *w,
                Some(_) => *w = 0.0,
            }
        }
        // A part left with nothing to carry its share hands it to the other.
        let (want_limb, want_trunk) = match (limb > 0.0, trunk > 0.0) {
            (true, true) => (share, 1.0 - share),
            (true, false) => (1.0, 0.0),
            (false, true) => (0.0, 1.0),
            (false, false) => continue,
        };
        let scale = |want: f32, have: f32| if have > 0.0 { want / have } else { 0.0 };
        let (fl, ft) = (scale(want_limb, limb), scale(want_trunk, trunk));
        for (b, w) in row.iter_mut().enumerate() {
            *w *= if seat[b].is_none() { ft } else { fl };
        }
    }
}

/// How wide a joint's weight transition is blended along the mesh, in cm. The diffusion runs
/// `(radius / mean edge length)²` passes (a random walk of that many steps spreads about that
/// far), capped at [`SMOOTH_MAX_PASSES`]; a coarse mesh whose edges exceed the radius gets none.
const SMOOTH_RADIUS_CM: f32 = 5.0;
/// The cap on diffusion passes — a fine mesh would ask for more, and the blend at the cap is
/// already wider than a knee.
const SMOOTH_MAX_PASSES: usize = 24;

/// Laplacian-diffuse per-position bone weights (`n` per position, dense) across the mesh's own
/// edges: each pass replaces a position's weights with the mean of itself and its neighbours'
/// average, so the hard inverse-square switch between two bones becomes a band
/// [`SMOOTH_RADIUS_CM`] wide. Edges come from the triangle list mapped through `corner`
/// (corner → position); a degenerate or edge-less mesh is left untouched. An edge touching a
/// PINNED position (a tagged region's vertex) is dropped, which takes those positions out of the
/// diffusion in both directions at once — they are neither smoothed nor a source.
fn smooth_weights(
    weights: &mut [f32],
    n: usize,
    positions: &[Vec3],
    corner: &[u32],
    indices: &[u32],
    pinned: &[bool],
) {
    let Some(mesh) = MeshEdges::read(positions, corner, indices, pinned) else {
        return;
    };
    let MeshEdges {
        edges,
        carry,
        degree,
        mean_edge,
    } = &mesh;
    let passes = ((SMOOTH_RADIUS_CM / mean_edge).powi(2).round() as usize).min(SMOOTH_MAX_PASSES);
    if passes == 0 {
        return;
    }
    let mut acc = vec![0.0f32; weights.len()];
    for _ in 0..passes {
        acc.fill(0.0);
        for (&(a, b), &c) in edges.iter().zip(carry) {
            let (a, b) = (a as usize * n, b as usize * n);
            for k in 0..n {
                acc[a + k] += c * (weights[b + k] - weights[a + k]);
                acc[b + k] += c * (weights[a + k] - weights[b + k]);
            }
        }
        for (pi, &d) in degree.iter().enumerate() {
            if d == 0 {
                continue;
            }
            let step = 0.5 / d as f32;
            for k in 0..n {
                let i = pi * n + k;
                weights[i] += acc[i] * step;
            }
        }
    }
}

/// The mesh's edges at WELD level, as the diffusions walk them: each edge once, with the share
/// of a full transfer it carries and every position's degree. Edges come from the triangle
/// list mapped through `corner` (corner → position); an edge touching a PINNED position (a
/// tagged region's vertex) is dropped, which takes those positions out of a diffusion in both
/// directions at once — neither smoothed nor a source. `None` for a degenerate or edge-less
/// mesh.
///
/// Each edge carries `(mean edge / its length)²` of a full transfer, capped at 1: a diffusion's
/// pass count is set from the MEAN edge, but a decimated mesh keeps its detail dense (face,
/// hands) and its flat stretches coarse (a forearm at 2–3 cm edges), and an uncorrected pass
/// over a 3 cm edge diffuses nine times as far as one over a 1 cm edge — GolemBaseV2's forearm
/// took hand and finger weight 10 cm up from the wrist that way. Scaled per edge, a blend is
/// the same width in centimetres everywhere on the mesh.
struct MeshEdges {
    edges: Vec<(u32, u32)>,
    carry: Vec<f32>,
    degree: Vec<u32>,
    mean_edge: f32,
}

impl MeshEdges {
    fn read(positions: &[Vec3], corner: &[u32], indices: &[u32], pinned: &[bool]) -> Option<Self> {
        let mut edges: Vec<(u32, u32)> = Vec::with_capacity(indices.len());
        for t in indices.as_chunks::<3>().0 {
            let (a, b, c) = (
                corner[t[0] as usize],
                corner[t[1] as usize],
                corner[t[2] as usize],
            );
            for (x, y) in [(a, b), (b, c), (c, a)] {
                if x != y && !pinned[x as usize] && !pinned[y as usize] {
                    edges.push((x.min(y), x.max(y)));
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();
        if edges.is_empty() {
            return None;
        }
        let mean_edge = edges
            .iter()
            .map(|&(a, b)| positions[a as usize].distance(positions[b as usize]))
            .sum::<f32>()
            / edges.len() as f32;
        if mean_edge <= 1e-6 {
            return None;
        }
        let mut degree = vec![0u32; positions.len()];
        let mut carry: Vec<f32> = Vec::with_capacity(edges.len());
        for &(a, b) in &edges {
            degree[a as usize] += 1;
            degree[b as usize] += 1;
            let d = positions[a as usize]
                .distance(positions[b as usize])
                .max(1e-3);
            carry.push((mean_edge / d).powi(2).min(1.0));
        }
        Some(Self {
            edges,
            carry,
            degree,
            mean_edge,
        })
    }
}

/// How firmly the bind's own reading holds at a position of the skin a body moves in, per unit
/// of the flesh's depth there: the settle's blend has a decay length of `depth / √HEAT`, so at
/// 1 a joint's blend is as wide as the flesh is thick there — a hand's width on a hand, a
/// span on a haunch.
const HEAT: f32 = 1.0;
/// The settle's iteration cap and the change under which it is done.
const HEAT_PASSES: usize = 160;
const HEAT_TOLERANCE: f32 = 1e-3;

/// THE SKIN A BODY MOVES IN IS SETTLED TO EQUILIBRIUM — bone heat (Baran & Popović), the
/// distributed weighting Aaron asked for in place of the hand's-width band: every bone's
/// weight field `w` over the mesh is the one that balances, at every position, its diffusion
/// along the mesh's edges against the bind's own reading `p` there,
/// `Σⱼ cⱼ (w(v) − w(j)) + H(v) (w(v) − p(v)) = 0`, with `H(v) = HEAT · (ē / depth(v))²` —
/// `depth` the flesh's depth at the position (its distance to the nearest bone, never under a
/// cell) and `ē` the mean edge. Close to a bone the reading stands as read; far from any it is
/// what diffuses in along the mesh from where it does; and the blend at a joint comes out as
/// wide as the flesh is thick there ([`HEAT`]) — several bones share every position between
/// two joints instead of the nearest taking it whole. Jacobi-iterated over the dense rows to
/// [`HEAT_TOLERANCE`] or [`HEAT_PASSES`]; pinned positions and positions with no edge keep
/// their reading. Where the mesh is finer than a flesh's depth the equilibrium is what the
/// hand's-width diffusion approximated; where it is coarse it is the same band.
#[allow(clippy::too_many_arguments)]
fn settle_weights(
    weights: &mut [f32],
    n: usize,
    positions: &[Vec3],
    corner: &[u32],
    indices: &[u32],
    pinned: &[bool],
    depth: &[f32],
    cell: f32,
) {
    let Some(mesh) = MeshEdges::read(positions, corner, indices, pinned) else {
        return;
    };
    let MeshEdges {
        edges,
        carry,
        mean_edge,
        ..
    } = &mesh;
    let mut carried = vec![0.0f32; positions.len()];
    for (&(a, b), &c) in edges.iter().zip(carry) {
        carried[a as usize] += c;
        carried[b as usize] += c;
    }
    let hold: Vec<f32> = depth
        .iter()
        .map(|&d| HEAT * (mean_edge / d.max(cell).max(1e-3)).powi(2))
        .collect();
    let source = weights.to_vec();
    let mut next = vec![0.0f32; weights.len()];
    for _ in 0..HEAT_PASSES {
        next.fill(0.0);
        for (&(a, b), &c) in edges.iter().zip(carry) {
            let (a, b) = (a as usize * n, b as usize * n);
            for k in 0..n {
                next[a + k] += c * weights[b + k];
                next[b + k] += c * weights[a + k];
            }
        }
        let mut worst = 0.0f32;
        for (pi, &c) in carried.iter().enumerate() {
            if c <= 0.0 || pinned[pi] {
                continue;
            }
            let h = hold[pi];
            let scale = 1.0 / (c + h);
            for k in 0..n {
                let i = pi * n + k;
                let w = (next[i] + h * source[i]) * scale;
                worst = worst.max((w - weights[i]).abs());
                weights[i] = w;
            }
        }
        if worst < HEAT_TOLERANCE {
            break;
        }
    }
}

/// The four strongest influences of a dense weight row, normalised, with anything under
/// `min_influence` pruned and the rest renormalised (the strongest survivor is always ≥ 0.25 of
/// the pre-prune sum, so the renormalisation basis can't vanish). Unused slots repeat the
/// strongest joint at weight 0 — inert.
fn top_four(row: &[f32], min_influence: f32) -> ([u32; 4], [f32; 4]) {
    let mut best: Vec<(usize, f32)> = row
        .iter()
        .enumerate()
        .filter(|(_, &w)| w > 0.0)
        .map(|(i, &w)| (i, w))
        .collect();
    best.sort_by(|a, b| b.1.total_cmp(&a.1));
    best.truncate(4);
    let sum: f32 = best.iter().map(|c| c.1).sum();
    let mut joints = [best.first().map_or(0, |c| c.0 as u32); 4];
    let mut weights = [0.0f32; 4];
    let mut kept = 0.0;
    for (k, &(bi, w)) in best.iter().enumerate() {
        let w = if sum > 0.0 { w / sum } else { 0.0 };
        if w >= min_influence {
            joints[k] = bi as u32;
            weights[k] = w;
            kept += w;
        }
    }
    if kept > 0.0 {
        for w in &mut weights {
            *w /= kept;
        }
    }
    (joints, weights)
}

/// Load a baked `flicker.rig` back into the editor's [`RawModel`] — the INVERSE of [`bake_rig`],
/// so an already-staged (or promoted) character can be re-opened, adjusted further, and
/// re-committed without starting over from the vendor FBX. Gz-transparent like every package
/// read, so it accepts the `.json` path whether the file at rest is loose or `.json.gz`.
///
/// Undoes the two bake concerns: the synthesized identity `root` at bone 0 is stripped (its
/// exact signature — name, no parent, identity local and bind — is required, so a rig that
/// legitimately authored a root is passed through untouched), and every parent index and
/// vertex joint shifts back down by one.
pub fn load_rig_raw(path: &Path) -> Result<RawModel> {
    let text = crate::package::read_text(path)
        .with_context(|| format!("reading staged rig {}", path.display()))?;
    let rig: RigFile = serde_json::from_str(&text)
        .with_context(|| format!("parsing staged rig {}", path.display()))?;
    Ok(rig_to_raw(&rig))
}

/// The pure conversion under [`load_rig_raw`] — see there for the contract.
fn rig_to_raw(rig: &RigFile) -> RawModel {
    let synthesized_root = rig.skeleton.bones.first().is_some_and(|b| {
        b.name == "root" && b.parent == -1 && b.local == IDENTITY16 && b.inverse_bind == IDENTITY16
    });
    let (skip, shift) = if synthesized_root { (1, 1u32) } else { (0, 0) };
    let bones: Vec<RawBone> = rig
        .skeleton
        .bones
        .iter()
        .skip(skip)
        .map(|b| {
            let (scale, rotation, translation) =
                Mat4::from_cols_array(&b.local).to_scale_rotation_translation();
            RawBone {
                name: b.name.clone(),
                parent: if b.parent < skip as i32 {
                    -1
                } else {
                    b.parent - skip as i32
                },
                translation: translation.to_array(),
                rotation: rotation.to_array(),
                scale: scale.to_array(),
                inverse_bind: b.inverse_bind,
            }
        })
        .collect();
    let vertices: Vec<RawVertex> = rig
        .mesh
        .vertices
        .iter()
        .map(|v| RawVertex {
            p: v.p,
            n: v.n,
            uv: v.uv,
            joints: [
                v.joints[0].saturating_sub(shift),
                v.joints[1].saturating_sub(shift),
                v.joints[2].saturating_sub(shift),
                v.joints[3].saturating_sub(shift),
            ],
            weights: v.weights,
        })
        .collect();
    RawModel {
        regions: Vec::new(),
        vertices,
        indices: rig.mesh.indices.clone(),
        bones,
    }
}

/// Rest WORLD frame per bone, composed from the stored local TRS (parents precede children).
pub(crate) fn rest_world_frames(model: &RawModel) -> Vec<Mat4> {
    let mut g: Vec<Mat4> = Vec::with_capacity(model.bones.len());
    for b in &model.bones {
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from_array(b.scale),
            Quat::from_array(b.rotation),
            Vec3::from_array(b.translation),
        );
        let world = match usize::try_from(b.parent) {
            Ok(p) if p < g.len() => g[p] * local,
            _ => local,
        };
        g.push(world);
    }
    g
}

/// Closest point on the segment `a`–`b` to point `p`.
pub(crate) fn closest_point_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-12 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    a + ab * t
}

/// Which side a limb pair is SQUARED FROM (Aaron's ruling FEFDA2B2: *"I should be able to select
/// which side to mirror, there's no guarantee it will be one specific side."*). `Auto` takes the
/// PLANTED — lower — limb of each pair as the source, whichever side that is; `Left`/`Right` force
/// that side as the source for EVERY pair regardless of which foot is up. It rides with the body
/// like the Prep facing knob (69F4B20D).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StanceSource {
    #[default]
    Auto,
    Left,
    Right,
}

/// What [`square_stance`] squared: per limb, its ROOT bone's name and how far its FOOT stood above
/// its twin's ([`Foot`] — the flesh under the ground joint, never the joint), in cm. Empty when the
/// body already stands square.
#[derive(Debug, Clone, Default)]
pub struct StanceReport {
    pub squared: Vec<(String, f32)>,
    /// The limbs it DECLINED to square, with how far under the body's own floor the skin follow
    /// would have dragged the mesh (cm). A limb here is left exactly as it was posed.
    pub declined: Vec<(String, f32)>,
}

/// How far above its twin a FOOT must stand to count as RAISED when the pair is STANDING ON
/// NEITHER SIDE: this fraction of the model's height, never less than the noise floor. A mid-stride
/// lift is 10 cm+ on a metre of horse, and a pair with no foot on the floor is not telling us about
/// a stride at all. A pair with a foot ON THE FLOOR is never gated by it ([`noise_floor`]).
const RAISED_FRACTION: f32 = 0.03;
/// The authoring wobble under THE NOISE FLOOR, in cm ([`noise_floor`]) — a wobble, and a doll a few
/// cm tall, stay out of the normaliser.
const RAISED_MIN_CM: f32 = 2.0;
/// How high above the MESH's lowest point a standing pair's lower FOOT may stand and still be ON
/// THE GROUND at all. A hoof, a ball and a toe stand in the bottom fifth of a body even when theirs
/// is the raised limb; a pair whose feet are both higher is not standing on anything (a body the
/// fit could not reach down, a composed rest in the air) and is none of the normaliser's business.
pub(crate) const GROUND_BAND: f32 = 0.2;

/// THE NOISE FLOOR of a body whose mesh spans `lo`..`hi`, in cm: [`RAISED_MIN_CM`] of authoring
/// wobble, or ONE CELL of the flesh grid that body is read at ([`crate::flesh::cell_for`]),
/// whichever is larger — a height finer than the grid could resolve is not a pose, it is the grid.
/// A FOOT AT OR UNDER IT IS ON THE FLOOR, ONE OVER IT IS RAISED: what the stance normaliser squares
/// by, and the band every harness judges what it left against, so a 4 m body is read at its own
/// 3 cm grain and a doll at the 2 cm floor.
pub(crate) fn noise_floor(lo: Vec3, hi: Vec3) -> f32 {
    RAISED_MIN_CM.max(crate::flesh::cell_for(
        (hi - lo).max_element(),
        crate::flesh::DEFAULT_CELLS,
    ))
}

/// How a bone CARRIES its flesh when it moves: its OLD segment taken exactly onto its NEW one —
/// stretched along itself to the new length (`stretch` along the old `axis`), turned about the old
/// joint, and set down where the joint now belongs. A bone that keeps its length is a rigid move.
///
/// The stretch is what keeps a re-posed limb's flesh on its own bones. The two sides of a pair are
/// laid down their OWN tubes, so a raised limb's bones need not be its twin's lengths (measured: a
/// Wolf's raised forepaw 31.5 cm from wrist to toes, its planted twin's 18.5), and a move that
/// only turned and set down the bone carried its flesh at the OLD length along the new direction —
/// 13 cm past the joint the next bone now starts at, straight through the floor. Taken end to end,
/// both ends of every segment land on their new joints, and two bones blending across a joint
/// agree on where that joint went.
#[derive(Clone, Copy)]
struct Carry {
    from: Vec3,
    to: Vec3,
    rot: Quat,
    /// The OLD segment's unit direction (zero for a leaf, which has no length to stretch).
    axis: Vec3,
    /// New length over old.
    stretch: f32,
    /// A SQUARED limb's twin, as the offset its reflection is hung by ([`mirrored_targets`]): the
    /// skin this bone carries is held to that twin's reflected flesh ([`Silhouette::hold`]).
    /// `None` for a move with no twin to wear (a turned head).
    twin: Option<Vec3>,
    /// A squared limb's SET-DOWN ([`Settle`]), when its flesh would hang over its twin's ground.
    settle: Option<Settle>,
    /// A turned head's REACH ([`Ahead`]): the move carries only the flesh ahead of the neck's root.
    ahead: Option<Ahead>,
}

/// THE FLESH A TURNED NECK CARRIES — what lies AHEAD of its root along the neck: `share` of a
/// point is 0 behind the root (the plane through `from` across `dir`), 1 from `ramp` ahead of it,
/// and a straight ramp between. The un-turn swings the neck and head about that root, and a body
/// vertex a metre behind it holding a tenth of a neck bone's weight (a bind's long tail) would be
/// swung a tenth of the way round a metre-long lever — the Wolf's back moved up to 9.6 cm. The
/// body behind the neck is not the neck's flesh, whatever small share a bind gave it.
#[derive(Clone, Copy)]
struct Ahead {
    from: Vec3,
    dir: Vec3,
    ramp: f32,
}

impl Ahead {
    fn share(&self, p: Vec3) -> f32 {
        ((p - self.from).dot(self.dir) / self.ramp.max(1e-3)).clamp(0.0, 1.0)
    }
}

/// A squared limb's SKIN SET DOWN onto the ground its twin stands on — the skin, not the bones:
/// lowered by `by` at the limb's sole (`sole`, its lowest flesh once carried and held), by nothing
/// at its socket's height (`top`) and in proportion between, and by the share of each vertex the
/// limb carries. The joints stay exactly the twin's reflection, so a squared body read again is
/// square, and the flesh stretches down its own limb by at most `by`.
#[derive(Clone, Copy)]
struct Settle {
    top: f32,
    sole: f32,
    by: f32,
}

impl Carry {
    /// A move that keeps the bone's length: turn about the old joint, set down at the new one.
    fn turn(from: Vec3, to: Vec3, rot: Quat) -> Self {
        Carry {
            from,
            to,
            rot,
            axis: Vec3::ZERO,
            stretch: 1.0,
            twin: None,
            settle: None,
            ahead: None,
        }
    }

    fn at(&self, p: Vec3) -> Vec3 {
        let d = p - self.from;
        self.to + self.rot * (d + self.axis * ((self.stretch - 1.0) * d.dot(self.axis)))
    }
}

/// SQUARE THE STANCE — the bake-time normaliser for MID-STRIDE sources (ruling 42AB9BA8, the
/// assessment 38EA5048, amended by FEFDA2B2). Every Meshy creature is generated with one paw or
/// hoof raised, so a rest pose taken from the mesh is a stride: rest previews, posters, mirror
/// operations and every authored clip start from it. This puts the body back on both feet.
///
/// For each STANDING PAIR ([`standing_pairs`]: the `_l`/`_r` limb roots of a module the recipe
/// authors standing, each with its GROUND joint) whose lower FOOT stands in the bottom
/// [`GROUND_BAND`] of the body:
///   1. every foot is read by its FLESH ([`Foot`]: the lowest skin of the limb's own tube under its
///      ground joint), never by the joint — the fit lays a planted foot's joint anywhere up to 8 cm
///      inside it and a raised one's at its very tip. A foot at or under the [`noise_floor`] is ON
///      THE FLOOR, one over it RAISED. A pair with one foot on the floor is STANDING on that limb,
///      and its raised twin is squared whatever the size of the raise; a pair with both feet on the
///      floor is square already. A pair standing on neither side is not evidence of a stride and
///      keeps [`RAISED_FRACTION`] of the model's height. Under `source`, Left/Right name the source
///      outright and the other side is the one that moves.
///   2. every bone of the moving limb (root through the ground joint, twists and toes included) is
///      put at its twin's world position REFLECTED across X = 0 — the same reflection the bench's
///      `mirror_to_twin` uses — with the reflected chain HUNG ON THE LIMB'S OWN SOCKET, which does
///      not move ([`mirrored_targets`]: the socket is a body measurement, not a pose).
///   3. the skin FOLLOWS ([`followed`]): each vertex moves by the weighted blend of its bones'
///      [`Carry`]s (a bone's takes its old segment exactly onto its new one; bones outside the
///      moving limb contribute identity), normals by the same blend; the flesh the limb carries
///      WHOLLY is held to its twin's reflected flesh ([`Silhouette::hold`]) and, where its foot
///      would hang over the ground its twin's foot stands on, set down onto it ([`Settle`], the same
///      [`Foot`]s). Positions only — no weight changes any of this. The skin a limb carries is its
///      TUBE's flesh ([`bake_skin`]), so squaring a stride drags no belly or tail with it.
///   4. the new rest positions are written back through the conform's own
///      [`crate::conform::write_world_frames`], which rebuilds each local and `inverse_bind`, so
///      the promoted body STANDS SQUARE with a SQUARE BIND and whatever frame convention it came in
///      with (identity on a composed body, 4D0CE655).
///
/// ONLY A STANDING MODULE IS SQUARED — a wing and a humanoid or hanging arm are never a stance,
/// matched or not: a bird's drooping wingtip near the floor is not a raised foot, and squaring it
/// re-poses a whole membrane and moves the limb's junction with the body, which the next read of
/// that mesh then fails to pair.
///
/// Runs inside the ONE bake path (56091EDF) after the skin is bound and before the rig is written,
/// so Preview and Commit see the same stance — never in the Rig step, where the human is placing
/// joints on the mesh AS POSED. A symmetric body is left bit-for-bit alone. This door READS the
/// body itself; the import and the bench square on the body their fit already read
/// ([`square_stance_on`]).
pub fn square_stance(
    model: &mut RawModel,
    source: StanceSource,
    recipe: &SkeletonRecipe,
) -> StanceReport {
    let body = Body::read(model);
    square_stance_on(model, source, recipe, Some(&body))
}

/// [`square_stance`] on `body`, a read ALREADY MADE off this mesh as posed — the fit's own, handed
/// on by `conform::rig_raw_mesh` and kept by the bench beside the model it describes, so an import
/// thins its mesh once however many times it is baked. `None` is a body with no flesh to read: its
/// feet are its joints and nothing holds or settles its skin.
pub fn square_stance_on(
    model: &mut RawModel,
    source: StanceSource,
    recipe: &SkeletonRecipe,
    body: Option<&Body>,
) -> StanceReport {
    let mut report = StanceReport::default();
    let n = model.bones.len();
    if n == 0 {
        return report;
    }
    let world = crate::conform::model_world_frames(model);
    let pos: Vec<Vec3> = world.iter().map(|g| g.w_axis.truncate()).collect();
    let kids = children(model);
    // THE BODY'S BOX IS ITS MESH'S — a rigged body is inside its mesh; a skeleton on its own still
    // answers. NOT the two together: a module the fit left UNMATCHED keeps its composed rest and
    // is prompted on the rail (spec 04803E0C §3), which can leave a hoof dangling tens of
    // centimetres UNDER the belly — the Hippo's hind pair sits 32 cm below its own mesh. Fold that
    // in and the floor drops beneath the body, every real foot reads as raised, and
    // [`GROUND_BAND`] throws a standing front pair out as if it stood on nothing.
    let empty = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let mesh = model
        .vertices
        .iter()
        .map(|v| Vec3::from_array(v.p))
        .fold(empty, |(l, h), p| (l.min(p), h.max(p)));
    let (lo, hi) = if mesh.0.z.is_finite() {
        mesh
    } else {
        pos.iter().fold(empty, |(l, h), p| (l.min(*p), h.max(*p)))
    };
    let (low, height) = (lo.z, hi.z - lo.z);
    if !height.is_finite() || height <= 0.0 {
        return report;
    }
    let noise = noise_floor(lo, hi);
    let by_name: HashMap<&str, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let twin_of =
        |i: usize| twin_name(&model.bones[i].name).and_then(|t| by_name.get(&*t).copied());
    let posed: Vec<f32> = model.vertices.iter().map(|v| v.p[2]).collect();

    let mut moves: Vec<Option<Carry>> = (0..n).map(|_| None).collect();
    // The body's own flesh as posed, looked at once and only when a pair is to be squared.
    let mut silhouette: Option<Option<Silhouette>> = None;
    for pair in standing_pairs(model, recipe, &pos) {
        let [(limb_l, ground_l), (limb_r, ground_r)] = &pair.sides;
        let (left, right) = (limb_l[0], limb_r[0]);
        // THE FEET, BY THEIR FLESH — the one reading of "on the floor" and "raised".
        let feet = [
            Foot::of(model, body, pos[*ground_l]),
            Foot::of(model, body, pos[*ground_r]),
        ];
        let floor = [feet[0].floor(model) - low, feet[1].floor(model) - low];
        // A PAIR ON THE GROUND, or none of the normaliser's business.
        if floor[0].min(floor[1]) > GROUND_BAND * height {
            continue;
        }
        let lift = floor[0] - floor[1];
        // THE FLOOR EXISTS TO IGNORE NOISE, NOT RAISES. A pair with one foot ON THE FLOOR is a body
        // STANDING on that limb, so a twin whose foot is off it is a pose whatever its size; two
        // feet on the floor are square already, however their joints were laid. A pair standing
        // on neither side is not evidence of a stride, and keeps the height fraction.
        // Auto reads the pose; Left/Right name the source and the OTHER side moves regardless.
        let raised = match (source, floor.map(|f| f <= noise)) {
            (StanceSource::Auto, [true, true]) => continue,
            (StanceSource::Auto, [true, false]) => right,
            (StanceSource::Auto, [false, true]) => left,
            (StanceSource::Auto, _) if lift.abs() <= (RAISED_FRACTION * height).max(noise) => {
                continue;
            }
            (StanceSource::Auto, _) if lift > 0.0 => left,
            (StanceSource::Auto, _) => right,
            (StanceSource::Left, _) => right,
            (StanceSource::Right, _) => left,
        };
        let (limb, (ours, theirs)) = if raised == left {
            (limb_l, (&feet[0], &feet[1]))
        } else {
            (limb_r, (&feet[1], &feet[0]))
        };
        // Every bone that moves needs a twin to be reflected from, or the chain would part.
        let Some(target) = mirrored_targets(limb, &pos, twin_of) else {
            tracing::warn!(
                "square_stance: {} has a bone with no twin — left as posed",
                model.bones[raised].name
            );
            continue;
        };
        // Already its twin's reflection — a squared pair under a FORCED source. Touch nothing.
        if limb.iter().all(|i| pos[*i].distance(target.0[i]) <= 1e-4) {
            continue;
        }
        let mut pending = limb_moves(limb, &target, &kids, &pos);
        let name = model.bones[raised].name.clone();
        let hold = silhouette
            .get_or_insert_with(|| body.and_then(|b| Silhouette::of(model, &b.flesh)))
            .as_ref();
        // THE SQUARED FOOT STANDS WHERE ITS TWIN'S STANDS. The fit lays each side down its own
        // tube, and a RAISED end's ground joint can sit at the very end of its flesh while its
        // planted twin's sits inside its own with the sole beneath it (the Horse's forehooves:
        // the raised one's flesh ends at its joint, the planted one's 1.8 cm under it). Carried
        // onto its twin's joints, such a limb hangs over the ground its twin stands on — the hold
        // keeps a squared limb from going UNDER that ground, and this is its other half: its SKIN
        // is set down onto it ([`Settle`]), the squared foot's sole as the move carries it against
        // its twin's as posed. Never its joints: a squared pair's joints are each other's
        // reflection, which is what makes squaring a squared body a no-op. It is set down exactly
        // when the move would leave it RAISED — over the noise floor, the one line that decided it
        // was raised — so a squared foot never stands off the floor by the reading that squared
        // it (the Squirrel's hind paw landed 2.1 cm up, 1.6 over its twin's sole); a foot the move
        // lands on the floor is not touched, and a foot with no flesh to read has no sole.
        if hold.is_some() {
            let mut carried = posed.clone();
            for (i, p, _) in followed(model, &pending, hold) {
                carried[i] = p.z;
            }
            let top = target.0[&limb[0]].z;
            if let (Some(sole), Some(ground)) =
                (ours.floor_at(|i| carried[i]), theirs.floor_at(|i| posed[i]))
            {
                if sole - low > noise && sole > ground && top > sole {
                    let settle = Settle {
                        top,
                        sole,
                        by: sole - ground,
                    };
                    for m in pending.iter_mut().flatten() {
                        m.settle = Some(settle);
                    }
                }
            }
        }
        // A SQUARED BODY STILL STANDS ON THE FLOOR IT WAS STANDING ON. The skin follows this move
        // bone by bone and is held to the twin's flesh ([`followed`]); whatever a limb's bones
        // still carry beyond that — on a body bound before the tube bind, the belly, the tail's
        // fall, the other side's paw — would be dragged along with it. Where that puts flesh
        // under the ground the body was grounded on, the normaliser has not squared the body, it
        // has broken it: the mesh then hangs below its own feet, and since the floor every foot is
        // measured from is the MESH's lowest point (77D298EB) every hoof on the body reads as if it
        // were in the air — the Ram 33.7 cm under z = 0 on a 13.8 cm lift, the Lizard 40.6
        // (incident 7CF34E04). Leave that pair AS POSED and say so (4BB12A75: fail loud); the rail
        // still has it to place.
        let sunk = sink_below(model, &pending, low, hold);
        if sunk > noise {
            tracing::warn!(
                "square_stance: squaring {name} would sink this body {sunk:.1} cm through its own \
                 floor — left as posed ({:.1} cm of lift)",
                lift.abs()
            );
            report.declined.push((name, sunk));
            continue;
        }
        for (slot, m) in moves.iter_mut().zip(&pending) {
            if m.is_some() {
                *slot = *m;
            }
        }
        tracing::info!(
            "square_stance: {name} ({}) squared from {} ({:.1} cm of lift, {} bones)",
            pair.module,
            model.bones[if raised == left { right } else { left }].name,
            lift.abs(),
            limb.len()
        );
        report.squared.push((name, lift.abs()));
    }
    if report.squared.is_empty() {
        return report;
    }

    follow_and_rebind(model, &world, &moves, silhouette.flatten().as_ref());
    report
}

/// Every bone's children, by index.
fn children(model: &RawModel) -> Vec<Vec<usize>> {
    let n = model.bones.len();
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, b) in model.bones.iter().enumerate() {
        if let Some(p) = usize::try_from(b.parent).ok().filter(|&p| p < n) {
            kids[p].push(i);
        }
    }
    kids
}

/// ONE STANDING PAIR of a body — both sides of a module the recipe authors standing, each as its
/// whole limb (root first) and its GROUND joint ([`limb_and_ground`]), left side first.
pub(crate) struct StandingPair {
    /// The module the pair is ([`crate::baseline::module_id`]'s spelling, as the matcher and the
    /// rail carry it).
    pub module: String,
    pub sides: [(Vec<usize>, usize); 2],
}

/// THE STANDING PAIRS OF A BODY: every `_l`/`_r` pair of LIMB ROOTS — a sided bone whose parent
/// carries no side (`thigh_l` under `pelvis`, `clavicle_l` under `spine_03` on a quadruped) — of a
/// module `recipe` authors standing ([`standing_bones`]). `pos` is every bone's world position.
///
/// THE ONE LIST OF GROUND JOINTS. The stance normaliser squares these pairs and the harness judges
/// these feet, and nothing else on a body is ever a ground joint: a wing's digit drooping to the
/// floor, a hand hanging by a knee, is no foot — by the recipe's own topology, never by a bone's
/// name or its height.
pub(crate) fn standing_pairs(
    model: &RawModel,
    recipe: &SkeletonRecipe,
    pos: &[Vec3],
) -> Vec<StandingPair> {
    let n = model.bones.len();
    let standing = standing_bones(recipe);
    let kids = children(model);
    let by_name: HashMap<&str, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let is_root = |i: usize| {
        side_of(&model.bones[i].name).is_some()
            && usize::try_from(model.bones[i].parent)
                .ok()
                .filter(|&p| p < n)
                .is_some_and(|p| side_of(&model.bones[p].name).is_none())
    };
    let mut out = Vec::new();
    for left in (0..n).filter(|&i| side_of(&model.bones[i].name) == Some(true) && is_root(i)) {
        let Some(right) = twin_name(&model.bones[left].name)
            .and_then(|t| by_name.get(t.as_str()).copied())
            .filter(|&r| is_root(r))
        else {
            continue;
        };
        let (Some(module), Some(_)) = (
            standing.get(&model.bones[left].name),
            standing.get(&model.bones[right].name),
        ) else {
            continue;
        };
        out.push(StandingPair {
            module: module.clone(),
            sides: [
                limb_and_ground(left, &kids, pos),
                limb_and_ground(right, &kids, pos),
            ],
        });
    }
    out
}

/// THE BONES OF THE MODULES A RECIPE AUTHORS STANDING — every leg, and a foreleg whose chain ends
/// in a ground contact ([`ArmKind::Ungulate`]) — by name, as the composer names them, each with the
/// id of the module it is ([`crate::baseline::module_id`]): the modules the matcher lets take only a
/// pair that reaches the floor (`conform::match_recipe`). The walk claims each module's instance
/// prefix in the composer's own order (trunk, head, arms, legs, tails, then each mount's trunk),
/// because that order is what decides which instance is which.
fn standing_bones(recipe: &SkeletonRecipe) -> HashMap<String, String> {
    use crate::baseline::{module_id, next_prefix};
    fn walk(
        spec: &TrunkSpec,
        counts: &mut HashMap<&'static str, usize>,
        out: &mut std::collections::HashSet<String>,
    ) {
        next_prefix(counts, "trunk");
        if spec.head {
            next_prefix(counts, "head");
        }
        for kind in &spec.arms {
            let p = next_prefix(counts, "arm");
            if *kind == ArmKind::Ungulate {
                out.insert(module_id("arm", &p));
            }
        }
        for _ in &spec.legs {
            out.insert(module_id("leg", &next_prefix(counts, "leg")));
        }
        for _ in &spec.tails {
            next_prefix(counts, "tail");
        }
        for m in &spec.mounts {
            walk(&m.trunk, counts, out);
        }
    }
    let mut modules = std::collections::HashSet::new();
    walk(&recipe.trunk, &mut HashMap::new(), &mut modules);
    let Ok((bones, of)) = crate::baseline::compose_with_modules(recipe, crate::baseline::STATURE)
    else {
        return HashMap::new();
    };
    bones
        .into_iter()
        .zip(of)
        .filter(|(_, m)| modules.contains(m))
        .map(|(b, m)| (b.name, m))
        .collect()
}

/// HOW EVERY BONE OF A LIMB CARRIES ITS FLESH onto its `target` — one slot per bone of the body,
/// `None` outside the limb. A bone's segment runs to its children INSIDE the limb, meaned as
/// [`bake_skin`] means them (so a foot with several toes keeps one chain), and its [`Carry`] takes
/// the old segment exactly onto the new one.
fn limb_moves(
    limb: &[usize],
    (target, twin): &(HashMap<usize, Vec3>, Vec3),
    kids: &[Vec<usize>],
    pos: &[Vec3],
) -> Vec<Option<Carry>> {
    let mut moves: Vec<Option<Carry>> = vec![None; pos.len()];
    for &i in limb {
        let to = target[&i];
        let ends: Vec<usize> = kids[i]
            .iter()
            .copied()
            .filter(|c| target.contains_key(c))
            .collect();
        let carry = if ends.is_empty() {
            // A LEAF — the ground joint itself — has no segment of its own to turn by, and its
            // flesh is the end of its parent's: it turns WITH its parent. Left unturned, a raised
            // hoof, paw or toe keeps the angle it was raised at while the bones above it swing
            // down, and lands tipped through the floor.
            let parent = limb
                .iter()
                .copied()
                .find(|&p| kids[p].contains(&i))
                .and_then(|p| moves[p]);
            Carry {
                twin: Some(*twin),
                ..Carry::turn(pos[i], to, parent.map_or(Quat::IDENTITY, |m| m.rot))
            }
        } else {
            let k = ends.len() as f32;
            let old = ends.iter().map(|&c| pos[c]).sum::<Vec3>() / k - pos[i];
            let new = ends.iter().map(|&c| target[&c]).sum::<Vec3>() / k - to;
            match (old.try_normalize(), new.try_normalize()) {
                (Some(a), Some(b)) => Carry {
                    from: pos[i],
                    to,
                    rot: Quat::from_rotation_arc(a, b),
                    axis: a,
                    stretch: new.length() / old.length(),
                    twin: Some(*twin),
                    settle: None,
                    ahead: None,
                },
                _ => Carry {
                    twin: Some(*twin),
                    ..Carry::turn(pos[i], to, Quat::IDENTITY)
                },
            }
        };
        moves[i] = Some(carry);
    }
    moves
}

/// HOW FAR UNDER `low` THE SKIN WOULD GO if `moves` were followed, in cm (`0` when nothing drops
/// below it) — exactly what [`follow_and_rebind`] applies ([`followed`]), run over the positions
/// alone, so a move can be judged before the body is committed to it.
fn sink_below(
    model: &RawModel,
    moves: &[Option<Carry>],
    low: f32,
    hold: Option<&Silhouette>,
) -> f32 {
    let worst = followed(model, moves, hold)
        .iter()
        .map(|&(_, p, _)| p.z)
        .fold(low, f32::min);
    (low - worst).max(0.0)
}

/// A STANDING LIMB'S FOOT — the flesh its ground joint stands on. THE ONE READING of "on the floor"
/// and "raised": the stance normaliser's raise test, its [`Settle`] and the harness's verdict all
/// take a foot's height from here ([`Foot::of`], [`Foot::floor`]), never from the joint.
///
/// The fit lays a limb's ground joint at the far end of its graph limb, which is not the sole: a
/// planted foot's joint can sit 3 to 8 cm up inside it (the Wolf's and the Elephant's forefeet,
/// soles on the floor) and a raised one's at the very tip of its flesh (the Horse's raised
/// forehoof), so joints compared with joints call a planted foot raised and miss a raise that is
/// really there (the BlackBear's forepaw: joints 2.2 / 6.7 cm, soles 0.4 / 4.1).
///
/// THE FOOT IS THE SKIN OF THE LIMB'S OWN TUBE WITHIN ONE RADIUS OF ITS GROUND JOINT. The tube is
/// the graph limb whose flesh holds the joint ([`ShapeGraph::limb_membership`], every limb's own
/// run — [`clear_of_core`] on — weighed against the cores and against each other); its RADIUS is
/// how thick that tube is, the median inscribed radius of its own run (the ball nearest a joint at
/// a cap has shrunk to a cell, and a round tube's skin stands a whole radius off its axis). The
/// joint must stand in the body's flesh or within one radius of it for the tube to speak for it at
/// all — never "within one radius of the lead": thinning stops a radius short of a cap and the fit
/// traces the ground joint on down the flesh to its end, up to a whole pastern past the lead's last
/// ball (the Horse's hind hooves, 12 cm). The foot's skin is that tube's flesh within one radius of
/// the joint's plumb line (and the cell the grid read it at), from one radius over the joint all the
/// way down: the sole is the lowest of it, whatever height inside the foot the joint was laid at,
/// because down is the one direction the world supplies. Only the tube's own flesh near the joint
/// counts — the twin's hoof, a tail lying under a raised foot, the snout or the belly the tube's
/// flesh runs on into a hand's breadth away are never this foot (measured: read over the whole
/// tube, a Boar's raised forehoof took its "sole" from flesh 22 cm ahead of it).
///
/// A joint no tube holds within one radius (a module nothing matched, its composed rest dangling
/// off the body) and a body with no flesh to read — a point set, or flesh under two cells thick, the
/// test [`Silhouette::of`] refuses such a mesh by — have no foot: such a limb stands where its
/// joint stands.
pub(crate) struct Foot {
    /// The foot's skin, by vertex.
    skin: Vec<usize>,
    /// The ground joint's own height — where a foot no flesh speaks for stands.
    joint: f32,
}

impl Foot {
    /// The foot under the ground joint at `joint`, read off `body` — this mesh as posed.
    pub(crate) fn of(model: &RawModel, body: Option<&Body>, joint: Vec3) -> Foot {
        let none = Foot {
            skin: Vec::new(),
            joint: joint.z,
        };
        let Some((flesh, graph)) = body.and_then(|b| Some((&b.flesh, b.graph.as_ref()?))) else {
            return none;
        };
        if graph.max_radius < 2.0 * graph.cell {
            return none;
        }
        // Every limb weighed by its OWN run: a joint is in a tube or in the body, never in the
        // stretch of a lead that is still inside the trunk.
        let own: Vec<Option<f32>> = graph
            .limbs
            .iter()
            .map(|l| Some(clear_of_core(graph, l)))
            .collect();
        let tube = |p: Vec3| {
            graph
                .limb_membership(p, &own)
                .filter(|&(_, share)| share >= 0.5)
                .map(|(l, _)| l)
        };
        let Some(l) = tube(joint) else {
            return none;
        };
        // How thick the tube is: the median of its own run's inscribed radii.
        let limb = &graph.limbs[l];
        let from = own[l].unwrap_or(0.0);
        let mut arc = 0.0_f32;
        let mut radii: Vec<f32> = Vec::new();
        for (k, (c, r)) in limb.lead.iter().zip(&limb.lead_r).enumerate() {
            if k > 0 {
                arc += limb.lead[k - 1].distance(*c);
            }
            if arc >= from {
                radii.push(*r);
            }
        }
        radii.sort_by(f32::total_cmp);
        let Some(&r) = radii.get(radii.len() / 2) else {
            return none;
        };
        if flesh.distance_outside(joint) > r + graph.cell {
            return none;
        }
        let reach = r + graph.cell;
        let skin = model
            .vertices
            .iter()
            .enumerate()
            .filter(|(_, v)| {
                let p = Vec3::from_array(v.p);
                p.z <= joint.z + r && (p - joint).truncate().length() <= reach && tube(p) == Some(l)
            })
            .map(|(i, _)| i)
            .collect();
        Foot {
            skin,
            joint: joint.z,
        }
    }

    /// HOW HIGH THE FOOT STANDS as posed (world z) — the lowest of its skin, or where its joint
    /// stands when no flesh speaks for it. At or under the [`noise_floor`] over the body's lowest
    /// point it is ON THE FLOOR, over it RAISED.
    pub(crate) fn floor(&self, model: &RawModel) -> f32 {
        self.floor_at(|i| model.vertices[i].p[2])
            .unwrap_or(self.joint)
    }

    /// The lowest of the foot's skin with each vertex's height read by `z` — as posed, or where a
    /// move carries it. `None` for a foot with no skin, which has no sole to measure.
    fn floor_at(&self, z: impl Fn(usize) -> f32) -> Option<f32> {
        self.skin.iter().map(|&i| z(i)).reduce(f32::min)
    }
}

/// WHERE THE SKIN GOES when it follows `moves`: `(vertex, position, normal)` for every vertex that
/// carries weight on a moved bone. Each takes the weighted blend of its bones' [`Carry`]s, as
/// DELTAS — so a bone that is not moving contributes exactly identity and a vertex outside the
/// moved chain never shifts — and normals the same blend of the turns. Positions only: no weight
/// changes here.
///
/// A vertex a SQUARED limb carries WHOLLY — its tube's own flesh, every weight on the moved chain —
/// is then HELD to its twin's reflected flesh ([`Silhouette::hold`]): a squared limb takes its
/// twin's pose, and the skin it carries there stays inside the flesh that pose is. Its own bones
/// cannot promise that on their own: the two sides of a pair are laid down their own tubes and
/// their joints need not sit at the same place in their flesh (the Ram's raised ball joint 7.9 cm
/// under its hock, its planted twin's 21.7), so a hoof carried onto its twin's joints can land
/// with its flesh through the floor its twin stands on. Flesh the limb only PARTLY carries is the
/// junction's and moves by its weights alone — so a bind that hands the limb trunk flesh is still
/// caught by [`sink_below`], never quietly repaired. Last, a limb that would hang over its twin's
/// ground has its skin set down onto it ([`Settle`]).
fn followed(
    model: &RawModel,
    moves: &[Option<Carry>],
    hold: Option<&Silhouette>,
) -> Vec<(usize, Vec3, Vec3)> {
    let mut out = Vec::new();
    for (i, v) in model.vertices.iter().enumerate() {
        let (p, nrm) = (Vec3::from_array(v.p), Vec3::from_array(v.n));
        let (mut dp, mut dn, mut carried) = (Vec3::ZERO, Vec3::ZERO, 0.0_f32);
        // The moved bone this vertex hangs on most — whose twin, if any, it is held to.
        let mut most: Option<(f32, Carry)> = None;
        for k in 0..4 {
            let w = v.weights[k];
            let Some(m) = moves.get(v.joints[k] as usize).and_then(Option::as_ref) else {
                continue;
            };
            let w = w * m.ahead.map_or(1.0, |a| a.share(p));
            if w <= 0.0 {
                continue;
            }
            dp += w * (m.at(p) - p);
            dn += w * (m.rot * nrm - nrm);
            carried += w;
            if most.is_none_or(|(b, _)| w > b) {
                most = Some((w, *m));
            }
        }
        let Some((_, m)) = most else {
            continue;
        };
        let mut to = p + dp;
        if let (Some(twin), Some(h), true) = (m.twin, hold, carried >= WHOLLY) {
            to = h.hold(p, to, twin);
        }
        if let Some(s) = m.settle {
            to.z -= s.by * ((s.top - to.z) / (s.top - s.sole)).clamp(0.0, 1.0) * carried;
        }
        out.push((i, to, (nrm + dn).normalize_or_zero()));
    }
    out
}

/// THE FLESH A SQUARED LIMB MAY WEAR — the body's own flesh as posed (the body's read, [`Body`]),
/// and the mesh's box the grid was built over. Looked at once per [`square_stance`], only when a
/// pair is to be squared.
struct Silhouette<'a> {
    flesh: &'a Flesh,
    lo: Vec3,
    hi: Vec3,
}

impl<'a> Silhouette<'a> {
    /// `None` when the mesh has no body to measure — flesh under two cells thick is a sheet or a
    /// cloud, whose "silhouette" is the voxel grid talking about itself (the same test the trunk
    /// alignment refuses such a mesh by, 4FF16605).
    fn of(model: &RawModel, flesh: &'a Flesh) -> Option<Self> {
        let body = flesh.core()?;
        if body.radius < 2.0 * flesh.cell() {
            return None;
        }
        let (lo, hi) = crate::conform::bbox(model);
        Some(Silhouette { flesh, lo, hi })
    }

    /// HOLD a squared limb's vertex to its twin's reflected flesh — "any vertex the follow leaves
    /// outside the flesh's posed silhouette is pushed back along `distance_outside`'s gradient".
    /// A squared limb's posed silhouette IS its twin's, reflected across the offset the chain was
    /// hung by (`twin`, [`mirrored_targets`]): `to`, reflected onto the twin's side, is judged
    /// against the body's own flesh there, and wherever it lies clearly outside it — more than
    /// [`HOLD_CELLS`] grid cells, past what the voxel field can resolve and past the blend a
    /// joint makes of two bones' turns — it is walked back in down the distance field's
    /// gradient; and never below the floor that flesh stands on, the one edge of it known
    /// exactly. Only a vertex that was ON the skin before the move (`from`) is held: a point
    /// floating off the body is nothing the flesh can speak for.
    fn hold(&self, from: Vec3, to: Vec3, twin: Vec3) -> Vec3 {
        let f = self.flesh;
        let cell = f.cell();
        if f.distance_outside(from) > cell {
            return to;
        }
        let across = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        let mut q = across(to - twin);
        for _ in 0..HOLD_STEPS {
            // The grid covers the mesh's box (padded), so a point past it is first brought back to
            // the box: the way in from under the floor is up.
            let c = q.clamp(self.lo, self.hi);
            let d = f.distance_outside(c);
            if d + q.distance(c) <= HOLD_CELLS * cell {
                break;
            }
            let slope =
                |a: Vec3| f.distance_outside(c + a * cell) - f.distance_outside(c - a * cell);
            let out = Vec3::new(slope(Vec3::X), slope(Vec3::Y), slope(Vec3::Z));
            let Some(out) = out.try_normalize() else {
                q = c;
                break;
            };
            q = c - out * d;
        }
        // The one edge of that silhouette known finer than a cell is its FLOOR: the twin stands on
        // the ground the body stood on, the lowest point of the mesh.
        q.z = q.z.max(self.lo.z);
        across(q) + twin
    }
}

/// A vertex whose weights on the moved chain add up to this is carried WHOLLY by it — the four
/// renormalised influences of [`top_four`] sum to one up to rounding.
const WHOLLY: f32 = 0.999;

/// How far outside its twin's reflected flesh, in grid cells, a held vertex must lie before
/// [`Silhouette::hold`] walks it back: a cell is what the voxel field can resolve, and a second
/// covers the blend a joint makes of two bones' turns — neither is a violation to repair.
const HOLD_CELLS: f32 = 2.0;

/// How many gradient steps [`Silhouette::hold`] takes to walk a vertex back onto its twin's flesh
/// — the chamfer field is exact along its steps, so one step lands within a cell or two and the
/// rest only settle it.
const HOLD_STEPS: usize = 4;

/// THE SKIN FOLLOWS the moves ([`followed`]), and the body RE-BINDS at its new rest — the write
/// path [`square_stance`] and [`face_forward`] share, because squaring a stride and un-turning a
/// head are the same two moves over a different set of bones. The re-bind writes each moved
/// bone's world frame as its OWN basis at its new translation, through the conform's
/// [`crate::conform::write_world_frames`], so a composed body's identity frames stay identity
/// (4D0CE655) and a vendor rig keeps the basis it came in with.
fn follow_and_rebind(
    model: &mut RawModel,
    world: &[Mat4],
    moves: &[Option<Carry>],
    hold: Option<&Silhouette>,
) {
    for (i, p, n) in followed(model, moves, hold) {
        let v = &mut model.vertices[i];
        v.p = p.to_array();
        v.n = n.to_array();
    }
    let moved: Vec<Mat4> = world
        .iter()
        .zip(moves)
        .map(|(g, m)| match m {
            Some(m) => Mat4::from_cols(g.x_axis, g.y_axis, g.z_axis, m.to.extend(1.0)),
            None => *g,
        })
        .collect();
    crate::conform::write_world_frames(&mut model.bones, &moved);
}

/// What [`face_forward`] measured: the head's yaw off the canon forward in degrees (positive =
/// turned to the BODY's left, the way the generated birds ship), and whether that was enough to
/// un-turn it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FaceReport {
    pub yaw_deg: f32,
    pub turned: bool,
}

/// How far off forward a head may look and still be called straight. The birds ship at 45°
/// (5147C59D); a few degrees is an authoring wobble that is not worth re-binding a body over.
const FACE_TOLERANCE_DEG: f32 = 5.0;

/// FACE FORWARD — the bake-time un-pose for a source whose HEAD IS TURNED. Every generated bird is
/// posed "body facing camera, head facing 45 degrees to the body's left" (5147C59D), so a rest pose
/// taken from the mesh looks sideways: the parked P4 question (164AE2F3), answered the way the
/// stance normaliser answered the mid-stride leg.
///
/// The head's FACING is the eyes' midpoint ahead of the head joint, or — no eyes — the jaw hung in
/// front of it. Its yaw about Z is measured against −Y, the canon forward. Past
/// [`FACE_TOLERANCE_DEG`] the NECK CHAIN (`neck_01` through `head` and every head child, whatever a
/// composed trunk prefixes them with) is rotated about Z at the neck's root until the head looks
/// down −Y, the skin following by the same per-bone rigid blend [`square_stance`] uses — for the
/// flesh AHEAD of the neck's root only ([`Ahead`]): the body behind it is not the neck's, whatever
/// small share of a neck bone a bind handed it — and the body re-binding at the new rest
/// ([`follow_and_rebind`]). The chain turns RIGIDLY, so no neck bone changes length.
///
/// Runs in the ONE bake path (56091EDF) right after [`square_stance`], on the bound skin and before
/// the rig is written, BY DEFAULT (A79A6131: turned heads are common across the generated sources;
/// the Prep box and `import_folder --no-face` are the opt-out). The turn it reads is the one the
/// fit laid: `conform::fit_to_graph` follows the neck to the head and turns the face with it. A
/// head that already faces forward is left bit-for-bit alone.
pub fn face_forward(model: &mut RawModel) -> FaceReport {
    let n = model.bones.len();
    let Some(head) = model.bones.iter().position(|b| b.name == "head") else {
        return FaceReport::default(); // a headless body (a quadruped trunk) has nothing to turn
    };
    let world = crate::conform::model_world_frames(model);
    let pos: Vec<Vec3> = world.iter().map(|g| g.w_axis.truncate()).collect();
    let at = |name: &str| {
        model
            .bones
            .iter()
            .position(|b| b.name == name)
            .map(|i| pos[i])
    };
    let facing = match (at("eye_l"), at("eye_r"), at("jaw")) {
        (Some(l), Some(r), _) => (l + r) * 0.5 - pos[head],
        (_, _, Some(j)) => j - pos[head], // the jaw hangs FORWARD of the head joint
        _ => {
            tracing::warn!("face_forward: `head` carries neither eyes nor a jaw — left as posed");
            return FaceReport::default();
        }
    };
    if facing.x.hypot(facing.y) < 1e-3 {
        tracing::warn!(
            "face_forward: the head's face sits straight above its joint — left as posed"
        );
        return FaceReport::default();
    }
    // The right-handed angle about +Z from the canon forward (−Y) to the facing.
    let yaw = facing.x.atan2(-facing.y);
    let report = FaceReport {
        yaw_deg: yaw.to_degrees(),
        turned: yaw.abs() > FACE_TOLERANCE_DEG.to_radians(),
    };
    if !report.turned {
        return report;
    }
    // The pivot: the root of the neck chain under the head — a composed rider's `trunk2_neck_01` is
    // one too. A head sitting straight on a trunk with no neck turns on its own joint.
    let mut pivot = head;
    while let Ok(p) = usize::try_from(model.bones[pivot].parent) {
        if p < n && model.bones[p].name.contains("neck") {
            pivot = p;
        } else {
            break;
        }
    }
    let (rot, o) = (Quat::from_rotation_z(-yaw), pos[pivot]);
    // Only the flesh AHEAD of the neck's root turns with it ([`Ahead`]), fading in over the
    // neck's first bone. A head that turns on its own joint (no neck) has no root to be behind.
    let ahead = (pivot != head).then(|| Ahead {
        from: o,
        dir: (pos[head] - o).normalize_or_zero(),
        ramp: (0..n)
            .filter(|&c| model.bones[c].parent == pivot as i32)
            .map(|c| pos[c].distance(o))
            .fold(0.0_f32, f32::max),
    });
    let mut moves: Vec<Option<Carry>> = (0..n).map(|_| None).collect();
    let mut stack = vec![pivot];
    while let Some(i) = stack.pop() {
        moves[i] = Some(Carry {
            ahead,
            ..Carry::turn(pos[i], o + rot * (pos[i] - o), rot)
        });
        stack.extend((0..n).filter(|&c| model.bones[c].parent == i as i32));
    }
    tracing::info!(
        "face_forward: {} was {:.1}° off forward — the neck turned at {}",
        model.bones[head].name,
        report.yaw_deg,
        model.bones[pivot].name
    );
    follow_and_rebind(model, &world, &moves, None);
    report
}

/// `Some(true)` for an `_l` bone, `Some(false)` for an `_r` one, `None` for a midline bone.
pub(crate) fn side_of(name: &str) -> Option<bool> {
    match name.rsplit_once('_') {
        Some((_, "l")) => Some(true),
        Some((_, "r")) => Some(false),
        _ => None,
    }
}

/// The name of `name`'s `_l`/`_r` twin.
pub(crate) fn twin_name(name: &str) -> Option<String> {
    let (stem, side) = name.rsplit_once('_')?;
    match side {
        "l" => Some(format!("{stem}_r")),
        "r" => Some(format!("{stem}_l")),
        _ => None,
    }
}

/// A limb root's whole subtree (parents before children) and its GROUND joint — the deepest
/// descendant in the hierarchy, the lowest of them when several tie (hoof, forehoof, ball, toe).
pub(crate) fn limb_and_ground(
    root: usize,
    kids: &[Vec<usize>],
    pos: &[Vec3],
) -> (Vec<usize>, usize) {
    let (mut limb, mut depth) = (vec![root], vec![0u32]);
    let mut i = 0;
    while i < limb.len() {
        let (b, d) = (limb[i], depth[i]);
        i += 1;
        for &c in &kids[b] {
            limb.push(c);
            depth.push(d + 1);
        }
    }
    let ground = limb
        .iter()
        .zip(&depth)
        .max_by(|a, b| a.1.cmp(b.1).then(pos[*b.0].z.total_cmp(&pos[*a.0].z)))
        .map_or(root, |(&i, _)| i);
    (limb, ground)
}

/// Each limb bone's target: its twin's world position REFLECTED across X = 0 — the same reflection
/// the bench's `mirror_to_twin` uses — with the WHOLE reflected chain shifted so that the limb's
/// own SOCKET (`limb[0]`, the root [`limb_and_ground`] was seeded with) lands exactly where it
/// already is. `None` when any bone of the limb has no twin to reflect from.
///
/// **THE SOCKET IS THE BODY, NOT THE POSE.** A stride is the rotations BELOW the shoulder or the
/// hip; where that shoulder SITS is a measurement the fit took off the mesh's own tube (spec
/// 04803E0C §3), and the two sides of a pair do not have to be each other's mirror — a mid-stride
/// source carries the swinging limb's socket forward of its twin's. Reflecting the socket ONTO its
/// twin moves the BODY: [`follow_and_rebind`] then carries every vertex weighted to that socket,
/// and on a quadruped that is a wide skirt of TRUNK flesh around the shoulder or the hip. Measured
/// across the 46-body sweep it punched the mesh through the floor it was standing on — the Ram
/// 33.7 cm under z = 0 on a 13.8 cm lift, the ElkBull 28.3, the Lizard 18.3 — and since the floor
/// every ground joint is measured from is the MESH's own lowest point (77D298EB), a body whose
/// flesh hangs below its feet reads as if every hoof were in the air.
///
/// Anchored here, the socket only ROTATES (its chain swings from its own lead onto its twin's
/// reflected one), which is what re-posing a limb means, and the trunk keeps its shape.
fn mirrored_targets(
    limb: &[usize],
    pos: &[Vec3],
    twin_of: impl Fn(usize) -> Option<usize>,
) -> Option<(HashMap<usize, Vec3>, Vec3)> {
    let mirror = |i: usize| {
        let t = pos[twin_of(i)?];
        Some(Vec3::new(-t.x, t.y, t.z))
    };
    let root = *limb.first()?;
    let anchor = pos[root] - mirror(root)?;
    let targets = limb
        .iter()
        .map(|&i| Some((i, mirror(i)? + anchor)))
        .collect::<Option<HashMap<usize, Vec3>>>()?;
    Some((targets, anchor))
}

pub fn bake_prop(model: &RawModel, source_name: &str, flat_color: Option<[f32; 3]>) -> RigFile {
    let vertices: Vec<Vertex> = model
        .vertices
        .iter()
        .map(|v| Vertex {
            p: v.p,
            n: v.n,
            uv: v.uv,
            joints: v.joints,
            weights: v.weights,
        })
        .collect();
    let indices = model.indices.clone();
    // One flat submesh/material — the same placeholder the character bake emits, over which
    // [`write_prop`] wires the source folder's maps. Left un-textured (a bake straight from
    // `bake_prop`, with no folder to read) the paperdoll renders the prop as flat steel.
    //
    // POC ONLY — NOT PERMANENT. `flat_color`, when present, is the FBX material's base colour baked
    // straight into the rig's per-material `color`, so an untextured flat-shaded prop (Synty
    // foliage) keeps its look through this path. This DELIBERATELY CONFLICTS with the
    // Materials-Unification project (render_class / 255-slot draw vocabulary), which is the durable
    // home for prop colour — remove this once prop colour is sourced through the materials system
    // instead of the rig. Absent `flat_color`, behaviour is unchanged (empty `color` = placeholder).
    let color = flat_color.map_or_else(Vec::new, |c| vec![c[0], c[1], c[2]]);
    let materials = vec![Material {
        name: "material_0".to_string(),
        slot: "material_0".to_string(),
        color,
        ..Default::default()
    }];
    let submeshes = vec![Submesh {
        material: 0,
        start: 0,
        count: indices.len(),
    }];

    RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: source_name.to_string(),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: "none".to_string(),
            ..Default::default()
        },
        skeleton: Skeleton { bones: Vec::new() },
        mesh: Mesh {
            vertices,
            indices,
            submeshes,
            materials,
            ..Default::default()
        },
        clips: Vec::new(),
        attach: Default::default(),
        attach_points: Vec::new(),
        collision: Default::default(),
        retarget: false,
        skeleton_recipe: None,
    }
}

/// Bake a raw garment mesh SKINNED onto a body — the outfit-overlay path (WS-F slice 8, porting
/// `tools/skin_outfit.py`). Unlike a prop (drawn rigid, fit applied live at draw), a garment is
/// **skinned by the body's palette and its fit is BAKED into the vertex positions**, then drawn at
/// the plain `world` matrix. So this:
///   1. rebuilds the socket's placement transform EXACTLY as the engine's `PieceFit::matrix`
///      (`flicker-paperdoll`): `world = rest_global[socket] · from_quat(rot) · SRT(scale·uniform,
///      user_rot, offset)`, where `rot` cancels the socket's rest rotation (its `inverse_bind`
///      rotation part), so `offset` acts along world axes at rest;
///   2. bakes every garment vertex through `world` (positions; inverse-transpose for normals);
///   3. transfers skin from the NEAREST body vertex (its `joints`+`weights`, already root-shifted
///      and normalised) — a garment has no skeleton of its own;
///   4. emits the body's FULL `skeleton.bones` verbatim, so `load_outfit`'s by-name remap onto the
///      base is the identity, and marks `applied_transform:"baked-fit"`, `retarget:false`.
///
/// `fit.rotate` is consumed as a QUATERNION (the engine's `user_rot`), not re-Eulerised.
///
/// `hang_cm` is how far off the body a vertex must stand to read as CLOTH — the ONE measurement
/// the region split makes (spec 0A81088E). A caller with no knob passes
/// [`DEFAULT_HANG_CM`](crate::regions::DEFAULT_HANG_CM); the bench passes the human's.
pub fn bake_garment(
    garment: &RawModel,
    source_name: &str,
    body: &RigFile,
    socket: &str,
    fit: &Attach,
    hang_cm: f32,
) -> Result<RigFile> {
    let socket_idx = body
        .skeleton
        .bones
        .iter()
        .position(|b| b.name == socket)
        .with_context(|| format!("socket bone {socket:?} is not in the body skeleton"))?;
    if body.mesh.vertices.is_empty() {
        bail!("the body rig carries no mesh — nothing to transfer skin weights from");
    }

    // The socket placement — SHARED with the editor's viewport preview via `attach_world`, so what
    // the user positions on screen is byte-for-byte what bakes here (no fit-math drift, gotcha #4).
    let world = attach_world(&body.skeleton.bones[socket_idx].inverse_bind, fit);
    let normal_mat = Mat3::from_mat4(world).inverse().transpose();

    let body_verts = &body.mesh.vertices;
    // Spatial hash over the body once, so a dense base does not turn the transfer quadratic.
    let grid = VertexGrid::build(body_verts);
    let vertices: Vec<Vertex> = garment
        .vertices
        .iter()
        .map(|v| {
            let p = world.transform_point3(Vec3::from(v.p));
            let n = (normal_mat * Vec3::from(v.n)).normalize_or_zero();
            let nearest = grid.nearest(p, body_verts);
            Vertex {
                p: p.to_array(),
                n: n.to_array(),
                uv: v.uv,
                joints: body_verts[nearest].joints,
                weights: body_verts[nearest].weights,
            }
        })
        .collect();

    let indices = garment.indices.clone();

    // ── THE GARMENT SPLITS ITSELF (spec 0A81088E), *unless it was already tagged by hand*. Now
    // that the piece is placed on the body and carries its skin, what hangs more than `hang_cm`
    // clear of the body's flesh is cloth, and every connected panel of it becomes a region with
    // its own comb of chains. A body-hugging piece (a glove, a boot) yields none, which is a
    // no-op. AUTHORED ROWS WIN: a garment that arrives carrying `regions` was tagged in the
    // bench's Regions panel (T2), and re-splitting here would throw the human's tags, chain
    // counts and stiffnesses away at Commit — so the split runs only on an UNTAGGED piece.
    let body_raw = rig_to_raw(body);
    // `rig_to_raw` drops the synthesized root and shifts every joint index down with it; the
    // transferred weights above are still in the RIG's numbering, so they take the same step.
    let root = u32::from(body_raw.bones.len() < body.skeleton.bones.len());
    let fitted = RawModel {
        vertices: vertices
            .iter()
            .map(|v| RawVertex {
                p: v.p,
                n: v.n,
                uv: v.uv,
                joints: v.joints.map(|j| j.saturating_sub(root)),
                weights: v.weights,
            })
            .collect(),
        indices: indices.clone(),
        bones: body_raw.bones.clone(),
        regions: Vec::new(),
    };
    let mut regions = if garment.regions.is_empty() {
        split_garment(&fitted, &Flesh::build_body(&body_raw), hang_cm)
    } else {
        garment.regions.clone()
    };
    // The comb is rebuilt either way: an authored row carries its tag, anchor, chain count and
    // stiffness but no chains — those are measured from the PLACED geometry, here.
    for r in &mut regions {
        build_cloth(&fitted, r);
        tracing::info!(
            "bake_garment: region {} → {} chains, {} verts on {}",
            r.name,
            r.chains.len(),
            r.verts.len(),
            r.anchor_bone
        );
    }

    let materials = vec![Material {
        name: "material_0".to_string(),
        slot: "material_0".to_string(),
        ..Default::default()
    }];
    let submeshes = vec![Submesh {
        material: 0,
        start: 0,
        count: indices.len(),
    }];

    Ok(RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: format!("{source_name} (skinned to body)"),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: "baked-fit".to_string(),
            ..Default::default()
        },
        skeleton: Skeleton {
            bones: body.skeleton.bones.clone(),
        },
        mesh: Mesh {
            vertices,
            indices,
            submeshes,
            materials,
            // The panels the garment split found, with their combs (spec 0A81088E).
            cloth: Cloth { regions },
            ..Default::default()
        },
        clips: Vec::new(),
        attach: Default::default(),
        attach_points: Vec::new(),
        collision: Default::default(),
        retarget: false,
        skeleton_recipe: None,
    })
}

/// The world placement an [`Attach`] resolves to on a socket bone, given the bone's `inverse_bind`
/// (= inverse rest world): `rest_global · from_quat(rot) · SRT(scale·uniform, user_rot, offset)`,
/// where `rot` cancels the socket's rest rotation so `offset` acts along world axes. This IS the
/// engine's `PieceFit::matrix` (flicker-paperdoll); the garment bake AND the editor's viewport
/// preview both call it, so the placement the user approves on screen is exactly what bakes.
pub fn attach_world(socket_inverse_bind: &[f32; 16], attach: &Attach) -> Mat4 {
    let inv_bind = Mat4::from_cols_array(socket_inverse_bind);
    let rest_global = inv_bind.inverse();
    let rot = Quat::from_mat4(&inv_bind).normalize();
    rest_global
        * Mat4::from_quat(rot)
        * Mat4::from_scale_rotation_translation(
            Vec3::from(attach.scale) * attach.uniform,
            Quat::from_array(attach.rotate).normalize(),
            Vec3::from(attach.offset),
        )
}

/// A uniform spatial hash over the body vertices for the weight transfer.
///
/// `skin_outfit.py` used exact brute force deliberately — a garment hangs loose, so a *capped*
/// search can silently grab the wrong side of a fold. This keeps that EXACTNESS (it returns the
/// same vertex brute force would) while dropping the cost: the expanding-shell scan stops only once
/// the nearest cell boundary is farther than the best hit so far, which is a sound bound. Needed
/// because a dense base (GolemBase carries ~285k verts vs the human base's ~12.5k) turns
/// O(garment · body) into ~70 s per piece.
struct VertexGrid {
    cell: f32,
    buckets: HashMap<(i32, i32, i32), Vec<u32>>,
}

impl VertexGrid {
    fn build(verts: &[Vertex]) -> Self {
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for v in verts {
            let p = Vec3::from(v.p);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        // Aim for ~1 vertex per cell: side ≈ extent / cbrt(n).
        let extent = (hi - lo).max_element().max(1.0);
        let cell = (extent / (verts.len() as f32).cbrt().max(1.0)).max(0.25);
        let mut buckets: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, v) in verts.iter().enumerate() {
            buckets
                .entry(cell_key(Vec3::from(v.p), cell))
                .or_default()
                .push(i as u32);
        }
        Self { cell, buckets }
    }

    /// The index of the body vertex closest to `p` — identical to a brute-force argmin.
    fn nearest(&self, p: Vec3, verts: &[Vertex]) -> usize {
        // A few shells cover any query sitting on or near the body. A vertex far outside the grid
        // (a garment not yet fitted) would otherwise walk hundreds of empty shells, so past this we
        // just do the exact scan — bounded, and never worse than brute force.
        const MAX_SHELLS: i32 = 6;
        let c = cell_key(p, self.cell);
        let (mut best, mut best_d) = (0usize, f32::INFINITY);
        for r in 0..=MAX_SHELLS {
            self.scan_shell(c, r, p, verts, &mut best, &mut best_d);
            // Anything unscanned sits at Chebyshev distance ≥ r+1 cells, i.e. at least `r * cell`
            // away — so once that bound exceeds the best hit, no closer vertex can exist.
            let bound = r as f32 * self.cell;
            if best_d.is_finite() && bound * bound >= best_d {
                return best;
            }
        }
        let (mut b, mut bd) = (0usize, f32::INFINITY);
        for (i, v) in verts.iter().enumerate() {
            let d = (Vec3::from(v.p) - p).length_squared();
            if d < bd {
                bd = d;
                b = i;
            }
        }
        b
    }

    /// Visit only the SURFACE cells at Chebyshev radius `r` — O(r²) per shell, where iterating the
    /// whole cube and skipping the interior would be O(r³) (and O(r⁴) across the expansion).
    fn scan_shell(
        &self,
        c: (i32, i32, i32),
        r: i32,
        p: Vec3,
        verts: &[Vertex],
        best: &mut usize,
        best_d: &mut f32,
    ) {
        let mut visit = |k: (i32, i32, i32)| {
            let Some(b) = self.buckets.get(&k) else {
                return;
            };
            for &i in b {
                let d = (Vec3::from(verts[i as usize].p) - p).length_squared();
                if d < *best_d {
                    *best_d = d;
                    *best = i as usize;
                }
            }
        };
        if r == 0 {
            visit(c);
            return;
        }
        for dy in -r..=r {
            for dz in -r..=r {
                visit((c.0 - r, c.1 + dy, c.2 + dz));
                visit((c.0 + r, c.1 + dy, c.2 + dz));
            }
        }
        for dx in -r + 1..=r - 1 {
            for dz in -r..=r {
                visit((c.0 + dx, c.1 - r, c.2 + dz));
                visit((c.0 + dx, c.1 + r, c.2 + dz));
            }
        }
        for dx in -r + 1..=r - 1 {
            for dy in -r + 1..=r - 1 {
                visit((c.0 + dx, c.1 + dy, c.2 - r));
                visit((c.0 + dx, c.1 + dy, c.2 + r));
            }
        }
    }
}

pub(crate) fn cell_key(p: Vec3, cell: f32) -> (i32, i32, i32) {
    (
        (p.x / cell).floor() as i32,
        (p.y / cell).floor() as i32,
        (p.z / cell).floor() as i32,
    )
}

/// Bake `model` (a character) and write the `flicker.rig` JSON to `out`.
///
/// `source_fbx` is the mesh file the model was parsed from — its folder holds the vendor's texture
/// maps, which are brought along by [`wire_source_textures`] exactly as [`write_prop`] and
/// [`write_garment`] do. Without it a character committed through the EDITOR shipped untextured
/// while the CLI [`import_folder`](crate::pipeline::import_folder) path wired its maps — the same
/// gap props carried until they were routed through here.
/// `mounts` are the character's authored attach POINTS (the editor's Attach stage) —
/// folded into the rig's `attach_points` block so the six placements the user tuned
/// actually ship instead of being discarded at export.
pub fn write_rig(
    model: &RawModel,
    source_fbx: &Path,
    source_name: &str,
    out: &Path,
    mounts: &[MountPoint],
    recipe: Option<&SkeletonRecipe>,
) -> Result<()> {
    let mut rig = bake_rig(model, source_name);
    rig.skeleton_recipe = recipe.cloned();
    rig.attach_points = mounts
        .iter()
        .map(|m| flicker_skeletal::format::AttachPoint {
            id: m.id.clone(),
            bone: m.bone.clone(),
            offset: m.offset,
        })
        .collect();
    wire_source_textures(source_fbx, source_name, out, &mut rig)?;
    write_rig_file(&rig, out)
}

/// The six attach points every CHARACTER ships with, in rail order — `(id, canonical bone)`.
/// The ids are what gameplay binds to (a weapon mounts by attach point, 99E083C9): the Clayworks
/// Attach stage labels and tunes them; the headless import ships them untuned. ONE table — the
/// bench composes its rail from it.
pub const DEFAULT_MOUNTS: [(&str, &str); 6] = [
    ("hand_r", "hand_r"),
    ("hand_l", "hand_l"),
    ("holster_r", "thigh_r"),
    ("holster_l", "thigh_l"),
    ("scabbard", "spine_02"),
    ("belt", "pelvis"),
];

/// [`DEFAULT_MOUNTS`] as untuned mount points (zero offsets) — what a character bakes with when
/// no Attach stage has tuned them.
pub fn default_mounts() -> Vec<MountPoint> {
    DEFAULT_MOUNTS
        .iter()
        .map(|(id, bone)| MountPoint {
            id: (*id).to_string(),
            bone: (*bone).to_string(),
            offset: [0.0; 3],
        })
        .collect()
}

/// One authored CHARACTER attach point the editor hands the bake — where a prop will
/// mount on this rig (grip, holster, scabbard, belt …). Editor-facing, like [`Fit`]:
/// the bench never touches the `flicker-skeletal` types.
#[derive(Debug, Clone)]
pub struct MountPoint {
    /// Stable id (`hand_r`, `belt`, …) — what gameplay binds to.
    pub id: String,
    /// Canonical bone name the point rides (the Attach stage's parent).
    pub bone: String,
    pub offset: [f32; 3],
}

/// Serialize an already-baked [`RigFile`] to `out` — the shared writer the character, prop and
/// garment bakes all funnel through, so there is one JSON-write path (and one place that owns the
/// error context). Emits the gz-at-rest form (`<out>.gz`) via the shared seam
/// ([`crate::package::write_text`]); readers address the rig by its logical `out` path.
pub fn write_rig_file(rig: &RigFile, out: &Path) -> Result<()> {
    let json = serde_json::to_string(rig).context("serializing the rig")?;
    crate::package::write_text(out, &json).with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

/// A human-authored placement for a prop or garment — the socket bone it mounts to, plus the
/// offset / rotation (euler degrees, applied XYZ) / per-axis scale + scale-all the fit stage tunes.
/// The EDITOR authors this visually and hands it to the bake; it is converted to the format
/// [`Attach`] here so callers never touch the `flicker-skeletal` types. This is the "human in the
/// loop" contract: the tool infers a starting fit, the user corrects it, and the bake honours it.
#[derive(Debug, Clone)]
pub struct Fit {
    pub socket: String,
    pub offset: [f32; 3],
    pub rot_deg: [f32; 3],
    /// PER-AXIS scale of the raw vendor mesh — the X / Y / Z sliders, for a piece that needs
    /// reshaping (a blade lengthened without being thickened).
    pub scale: [f32; 3],
    /// SCALE-ALL, multiplied onto every axis — resize without reshaping. Kept separate from
    /// `scale` so "make it 10% bigger" stays one slider instead of three kept in sync.
    pub uniform: f32,
}

impl Default for Fit {
    fn default() -> Self {
        Self {
            socket: String::new(),
            offset: [0.0; 3],
            rot_deg: [0.0; 3],
            scale: [1.0; 3],
            uniform: 1.0,
        }
    }
}

impl Fit {
    /// The format `Attach` this fit bakes to. Rotation is euler-XYZ degrees → quaternion (the
    /// engine's `user_rot`); per-axis `scale` and scale-all `uniform` map STRAIGHT across, because
    /// the format already carries both and `attach_world` already applies `scale · uniform` — so
    /// widening the editor needed no format change.
    ///
    /// Every factor is floored at 0.001: the sliders can reach zero, and a zero (or negative) axis
    /// collapses the mesh into a plane the bake cannot recover from.
    pub fn to_attach(&self) -> Attach {
        Attach {
            socket: self.socket.clone(),
            offset: self.offset,
            rotate: Quat::from_euler(
                EulerRot::XYZ,
                self.rot_deg[0].to_radians(),
                self.rot_deg[1].to_radians(),
                self.rot_deg[2].to_radians(),
            )
            .to_array(),
            scale: self.scale.map(|s| s.max(0.001)),
            uniform: self.uniform.max(0.001),
        }
    }
}

/// Bake a static prop and write it, folding the editor's authored [`Fit`] into the rig's `attach`
/// block so the paperdoll loads it PRE-FITTED (the socket + offset/rotation/scale the user tuned in
/// the viewport). Editor-facing: [`RawModel`] + paths + `Fit`, never the `flicker-skeletal` types.
///
/// `source_fbx` is the mesh file the model was parsed from — its folder holds the vendor's texture
/// maps, which are brought along by [`wire_source_textures`] exactly as the character import does.
pub fn write_prop(
    model: &RawModel,
    source_fbx: &Path,
    source_name: &str,
    out: &Path,
    fit: &Fit,
    flat_color: Option<[f32; 3]>,
) -> Result<()> {
    let mut rig = bake_prop(model, source_name, flat_color);
    rig.attach = fit.to_attach();
    wire_source_textures(source_fbx, source_name, out, &mut rig)?;
    write_rig_file(&rig, out)
}

/// Bake a garment SKINNED onto the canonical base body using the editor's authored [`Fit`], and
/// write it. The fit's socket + placement position the raw mesh into body bind space BEFORE the
/// nearest-vertex weight transfer, so what the user approved in the viewport is exactly what bakes.
/// Skins onto [`fitting_base`] — the clay Golem fitting body, not the conform canon.
///
/// `source_fbx` carries the garment's own texture maps along, as for [`write_prop`] — a garment
/// takes the BODY's skin weights but keeps its OWN material. `hang_cm` is the region split's one
/// measurement (see [`bake_garment`]); a model that already carries authored `regions` keeps them
/// and the hang goes unread.
pub fn write_garment(
    model: &RawModel,
    source_fbx: &Path,
    source_name: &str,
    out: &Path,
    fit: &Fit,
    hang_cm: f32,
) -> Result<()> {
    let body_path = fitting_base();
    let body_text = crate::package::read_text(&body_path).with_context(|| {
        format!(
            "reading the fitting body rig {} to skin the garment onto",
            body_path.display()
        )
    })?;
    let body: RigFile = serde_json::from_str(&body_text).context("parsing the base body rig")?;
    let mut rig = bake_garment(
        model,
        source_name,
        &body,
        &fit.socket,
        &fit.to_attach(),
        hang_cm,
    )?;
    wire_source_textures(source_fbx, source_name, out, &mut rig)?;
    write_rig_file(&rig, out)
}

/// Bring the source folder's texture maps along with a prop / garment bake — the SAME wiring the
/// character import runs ([`crate::pipeline::wire_textures`]), reached through the same
/// [`scan`](crate::scan) that classified the folder in the first place. The maps are copied beside
/// the rig as `<AssetName>_<Map>.png` and the material points at those basenames; a folder with no
/// maps simply wires none. Returns what was written.
fn wire_source_textures(
    source_fbx: &Path,
    source_name: &str,
    out: &Path,
    rig: &mut RigFile,
) -> Result<Vec<String>> {
    let folder = crate::pipeline::dir_of(source_fbx);
    let scan = crate::scan::scan_folder(folder)
        .with_context(|| format!("scanning {} for the asset's texture maps", folder.display()))?;
    let out_dir = crate::pipeline::dir_of(out);
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    crate::pipeline::wire_textures(&scan, source_fbx, out_dir, source_name, rig)
}

/// The BODY a garment skins onto, and the body the editor's prop/garment preview mounts against.
///
/// DISTINCT from [`default_reference`](crate::conform), which is the CONFORM canon — the 66-bone
/// skeleton every character is mapped onto. This is the *fitting* body: the clay Golem built for
/// hanging outfits and props on.
///
/// Prefers **`GolemBase_Low`** — the game-ready ~3.3k-tri cut (1.7 MB) — over the 95k-tri authoring
/// mesh (49.6 MB): fitting only needs the body's SHAPE, and the dense one costs 29× the load and
/// weight-transfer work for no fitting benefit. Falls back to the HQ Golem, then the canonical human
/// base, so any tree still bakes.
pub fn fitting_base() -> std::path::PathBuf {
    // PROMOTED content wins: the package tree is searched before staging, so an in-progress
    // import can't silently shadow the blessed fitting body — the same precedence the gz seam
    // applies between an at-rest file and a stale raw twin. Staging is still searched, so a
    // freshly imported base works before it has been promoted.
    let roots = crate::roots::roots();
    for characters in [
        roots.package().join("characters"),
        roots.staging().join("characters"),
    ] {
        // The golem IS the fitting body: GolemBaseV2 since the 2026-09-06 source reset retired
        // the `_Low` bases (GolemBase_Low left the package on 2026-09-07); the older names
        // still answer on a tree that has them.
        for candidate in ["GolemBaseV2", "GolemBase_Low", "GolemBase"] {
            let p = characters.join(candidate).join(format!("{candidate}.json"));
            if crate::package::file_exists(&p) {
                return p;
            }
        }
    }
    crate::conform::default_reference()
}

/// A reasonable STARTING socket bone for a garment, inferred from its name — mirrors
/// `skin_outfit.py`'s `PIECE_SOCKET`, defaulting to the upper spine for an unrecognised torso
/// piece. Only seeds the editor's socket picker; the user then confirms or changes it.
pub fn garment_socket(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("pant")
        || n.contains("hem")
        || n.contains("skirt")
        || n.contains("belt")
        || n.contains("leg")
    {
        "pelvis"
    } else if n.contains("boot") || n.contains("shoe") || n.contains("foot") {
        "calf_l"
    } else if n.contains("glove")
        || n.contains("gauntlet")
        || n.contains("hand")
        || n.contains("bracer")
    {
        "lowerarm_l"
    } else if n.contains("hood") || n.contains("hat") || n.contains("helm") || n.contains("mask") {
        "head"
    } else {
        "spine_02"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conform::{conform_to_canonical, default_reference, ConformMode};
    use crate::fbx::parse_fbx;
    use crate::rig::rename_to_canonical;

    /// A minimal torso-and-legs skeleton for the skin tests: world heads via parent-relative
    /// translations, identity rotations — the same shape a conformed model carries.
    fn skin_fixture() -> RawModel {
        let bone = |name: &str, parent: i32, t: [f32; 3]| RawBone {
            name: name.to_string(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: IDENTITY16,
        };
        let vert = |p: [f32; 3]| RawVertex {
            p,
            n: [0.0, -1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [0.0; 4],
        };
        RawModel {
            regions: Vec::new(),
            bones: vec![
                bone("root", -1, [0.0, 0.0, 0.0]),      // 0 — masked from flesh
                bone("pelvis", 0, [0.0, 0.0, 95.0]),    // 1
                bone("thigh_l", 1, [8.67, 0.0, -5.0]),  // 2
                bone("thigh_r", 1, [-8.67, 0.0, -5.0]), // 3
                bone("spine_01", 1, [0.0, 0.0, 15.0]),  // 4
                bone("calf_l", 2, [0.0, 0.0, -42.0]),   // 5
                bone("calf_r", 3, [0.0, 0.0, -42.0]),   // 6
                bone("neck_01", 4, [0.0, 0.0, 30.0]),   // 7
            ],
            vertices: vec![
                vert([0.0, -9.0, 100.0]), // front belly — torso flesh
                vert([0.0, -14.0, 2.0]),  // between the feet — nearest the root segment
            ],
            indices: vec![0, 1, 0],
        }
    }

    /// THE SKIN DEFAULTS GUARD (2026-08-20 audit): a multi-child bone's segment runs to the
    /// MEAN of its children (first-child ran `pelvis→thigh_l`, leaning the torso weighting
    /// left), the synthetic `root` never owns flesh (its segment spans the whole lower core),
    /// and noise influences are pruned. All three are what "discard the vendor skin and bake
    /// from OUR skeleton" depends on to come out clean.
    #[test]
    fn bake_skin_keeps_flesh_on_the_chain_and_off_the_root() {
        let mut m = skin_fixture();
        bake_skin(&mut m);
        let nm: Vec<&str> = m.bones.iter().map(|b| b.name.as_str()).collect();
        let share = |v: &RawVertex, name: &str| -> f32 {
            (0..4)
                .filter(|&k| nm[v.joints[k] as usize] == name)
                .map(|k| v.weights[k])
                .sum()
        };
        for v in &m.vertices {
            assert_eq!(share(v, "root"), 0.0, "the root must never own flesh");
            let sum: f32 = v.weights.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-4,
                "weights renormalise after pruning"
            );
            for k in 0..4 {
                assert!(
                    v.weights[k] == 0.0 || v.weights[k] >= 0.02,
                    "influences below the prune floor must not survive"
                );
            }
        }
        // The belly vert is TORSO flesh: with mean-of-children tails the pelvis segment runs up
        // the spine (not sideways down thigh_l), so pelvis outweighs either thigh, and the
        // torso family in total dominates the legs.
        let belly = &m.vertices[0];
        assert!(
            share(belly, "pelvis") > share(belly, "thigh_l"),
            "pelvis must outweigh a thigh on front-belly flesh"
        );
        let torso = share(belly, "pelvis") + share(belly, "spine_01") + share(belly, "neck_01");
        let legs = share(belly, "thigh_l")
            + share(belly, "thigh_r")
            + share(belly, "calf_l")
            + share(belly, "calf_r");
        assert!(
            torso > legs,
            "belly flesh belongs to the torso chain, got torso={torso} legs={legs}"
        );
        // Mean tails are symmetric, so a midline vert weights both sides identically.
        assert!(
            (share(belly, "thigh_l") - share(belly, "thigh_r")).abs() < 1e-4,
            "midline flesh must weight left and right identically"
        );
        // The floor vert sits nearest the (masked) root segment — it must fall to the LEGS
        // (thighs reach the calf heads in this footless fixture, so the four leg bones split
        // it), never to the root frame.
        let floor = &m.vertices[1];
        let floor_legs = share(floor, "thigh_l")
            + share(floor, "thigh_r")
            + share(floor, "calf_l")
            + share(floor, "calf_r");
        assert!(
            floor_legs > 0.99,
            "floor flesh falls to the legs once root is masked, got {floor_legs}"
        );
    }

    /// A torso-and-foot fixture for the normal-test guard: an A-posed forearm segment 6 cm
    /// in FRONT of the belly (the case the test exists for), a heel whose normal grazes the
    /// foot bone at a marginal angle, and a chest vertex on an inward-facing inner shell.
    fn normal_guard_fixture() -> RawModel {
        let bone = |name: &str, parent: i32, t: [f32; 3]| RawBone {
            name: name.to_string(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: IDENTITY16,
        };
        let vert = |p: [f32; 3], n: [f32; 3]| RawVertex {
            p,
            n,
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [0.0; 4],
        };
        RawModel {
            regions: Vec::new(),
            bones: vec![
                bone("root", -1, [0.0, 0.0, 0.0]),
                bone("pelvis", 0, [0.0, 0.0, 100.0]), // segment (0,0,100)→(0,0,120)
                bone("spine_01", 1, [0.0, 0.0, 20.0]), // segment (0,0,120)→(0,0,140)
                bone("neck_01", 2, [0.0, 0.0, 20.0]), // point (0,0,140)
                bone("lowerarm_l", 0, [20.0, -14.0, 110.0]), // segment across the belly front
                bone("hand_l", 4, [-40.0, 0.0, 0.0]), // point (-20,-14,110)
                bone("calf_l", 0, [8.0, 0.0, 30.0]),  // point
                bone("foot_l", 0, [8.0, 0.0, 8.0]),   // segment (8,0,8)→(8,-12,2)
                bone("ball_l", 7, [0.0, -12.0, -6.0]), // point
            ],
            vertices: vec![
                // Belly skin, facing forward: forearm 6 cm OUT in front, pelvis 8 cm under.
                vert([0.0, -8.0, 110.0], [0.0, -1.0, 0.0]),
                // Heel, side-facing with a forward-up tilt: the foot bone 7 cm away reads
                // marginally outside (cos ≈ 0.43), so do ball and calf; the only "inside"
                // bone is the pelvis a metre up.
                vert([8.0, 5.0, 3.0], [0.9, -0.3, 0.3]),
                // Chest on an inward-facing inner shell: spine 10 cm away but "outside" by
                // the inverted normal; the nearest inside bone is the forearm 25 cm away.
                vert([0.0, -10.0, 135.0], [0.0, 1.0, 0.0]),
            ],
            indices: vec![0, 1, 2],
        }
    }

    /// THE NORMAL-TEST GUARD (2026-09-03, ElfBaseA stray polygons): the outside-the-flesh test
    /// still rejects a forearm hanging in front of the belly, but is overruled when it would
    /// leave only an implausibly far bone — a heel misread at a marginal angle no longer binds
    /// to the pelvis, and an inward-facing chest shell no longer follows the arm.
    #[test]
    fn bake_skin_distrusts_the_normal_test_when_only_far_bones_survive() {
        let mut m = normal_guard_fixture();
        bake_skin(&mut m);
        let nm: Vec<&str> = m.bones.iter().map(|b| b.name.as_str()).collect();
        let dominant = |v: &RawVertex| -> &str {
            let k = (0..4)
                .max_by(|&a, &b| v.weights[a].total_cmp(&v.weights[b]))
                .unwrap();
            nm[v.joints[k] as usize]
        };
        assert_eq!(
            dominant(&m.vertices[0]),
            "pelvis",
            "a forearm in the air in front of the belly must still lose to the spine"
        );
        assert_eq!(
            dominant(&m.vertices[1]),
            "foot_l",
            "a heel whose foot bone grazes the normal test must not bind a metre away"
        );
        assert_eq!(
            dominant(&m.vertices[2]),
            "spine_01",
            "an inward-facing chest shell must bind to the spine under it, not the arm"
        );
    }

    /// [`rig_to_raw`] is the exact inverse of [`bake_rig`]: the synthesized root strips, the
    /// +1 joint/parent shift undoes, and a second bake reproduces the first file verbatim —
    /// the round trip the staged-reload workflow rides.
    #[test]
    fn staged_rig_round_trips_through_load() {
        let mut original = skin_fixture();
        bake_skin(&mut original);
        let baked = bake_rig(&original, "Fixture");
        let reloaded = rig_to_raw(&baked);
        assert_eq!(
            reloaded.bones.len(),
            original.bones.len(),
            "root strips back off"
        );
        for (a, b) in original.bones.iter().zip(&reloaded.bones) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.parent, b.parent);
            assert_eq!(a.inverse_bind, b.inverse_bind);
            for i in 0..3 {
                assert!((a.translation[i] - b.translation[i]).abs() < 1e-5);
                assert!((a.scale[i] - b.scale[i]).abs() < 1e-5);
            }
        }
        for (a, b) in original.vertices.iter().zip(&reloaded.vertices) {
            assert_eq!(a.joints, b.joints);
            assert_eq!(a.weights, b.weights);
            assert_eq!(a.p, b.p);
        }
        let rebaked = bake_rig(&reloaded, "Fixture");
        assert_eq!(
            serde_json::to_string(&baked.skeleton).unwrap(),
            serde_json::to_string(&rebaked.skeleton).unwrap(),
            "bake ∘ load ∘ bake is identity on the skeleton"
        );
        assert_eq!(
            serde_json::to_string(&baked.mesh).unwrap(),
            serde_json::to_string(&rebaked.mesh).unwrap(),
            "bake ∘ load ∘ bake is identity on the mesh"
        );
    }

    /// REAL-DATA GUARD (skips without the content tree): re-deriving the conformed golem's
    /// skin from its own centred skeleton keeps the TORSO CORE on the torso chain, never
    /// weights a mount frame, and stays left-right symmetric — the three hardened defaults,
    /// pinned against the real body. (Honest-band note, 2026-08-20: the band is |x| < 16 —
    /// the solid torso — because the A-posed forearms hang at belly HEIGHT, and a wider band
    /// measures the arm's own flesh as "belly".)
    #[test]
    fn golem_rebake_keeps_the_torso_core_on_the_chain() {
        let path = crate::roots::roots()
            .package()
            .join("characters/GolemBase_Low/GolemBase_Low.json");
        if !crate::package::file_exists(&path) {
            eprintln!("skipping: no content tree");
            return;
        }
        let mut m = load_rig_raw(&path).expect("staged golem loads");
        bake_skin(&mut m);
        let nm: Vec<&str> = m.bones.iter().map(|b| b.name.as_str()).collect();
        let mut torso = 0.0f32;
        let mut band_total = 0.0f32;
        let mut n = 0;
        let (mut thigh_l, mut thigh_r) = (0.0f32, 0.0f32);
        for v in &m.vertices {
            for k in 0..4 {
                let b = nm[v.joints[k] as usize];
                let w = v.weights[k];
                assert!(
                    w == 0.0 || (b != "root" && b != "Weapon_L" && b != "Weapon_R"),
                    "a mount frame must never own flesh, {b} got {w}"
                );
                if b == "thigh_l" {
                    thigh_l += w;
                }
                if b == "thigh_r" {
                    thigh_r += w;
                }
            }
            // The solid-torso front-belly core (Z-up cm, −Y forward, |x| inside the trunk).
            if v.p[0].abs() >= 16.0 || v.p[2] < 95.0 || v.p[2] >= 115.0 || v.p[1] >= -2.0 {
                continue;
            }
            n += 1;
            for k in 0..4 {
                let b = nm[v.joints[k] as usize];
                band_total += v.weights[k];
                if b.starts_with("pelvis") || b.starts_with("spine") || b.starts_with("thigh") {
                    torso += v.weights[k];
                }
            }
        }
        assert!(n > 100, "the belly core samples real flesh, got {n} verts");
        assert!(
            torso / band_total.max(1e-6) > 0.75,
            "the belly core belongs to the torso chain: {torso:.1}/{band_total:.1} over {n} verts"
        );
        let (lo, hi) = (thigh_l.min(thigh_r), thigh_l.max(thigh_r));
        assert!(
            lo / hi.max(1e-6) > 0.85,
            "mean-of-children tails weight the sides evenly, got thigh_l={thigh_l:.1} thigh_r={thigh_r:.1}"
        );
    }

    /// A dense two-bone strip — 1 cm cells, 0.2 cm wide, along +X: `a` owns 0–10, `b` 10–20,
    /// `c` is a point at 20 — emitted one vertex per CORNER, the way every parsed or decimated
    /// mesh arrives.
    fn strip_fixture() -> RawModel {
        let bone = |name: &str, parent: i32, t: [f32; 3]| RawBone {
            name: name.to_string(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: IDENTITY16,
        };
        let corner = |x: f32, y: f32| RawVertex {
            p: [x, y, 0.0],
            n: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [0.0; 4],
        };
        let mut vertices = Vec::new();
        for i in 0..20 {
            let (x0, x1) = (i as f32, i as f32 + 1.0);
            for tri in [
                [(x0, -0.1), (x1, -0.1), (x1, 0.1)],
                [(x0, -0.1), (x1, 0.1), (x0, 0.1)],
            ] {
                for (x, y) in tri {
                    vertices.push(corner(x, y));
                }
            }
        }
        let indices = (0..vertices.len() as u32).collect();
        RawModel {
            regions: Vec::new(),
            vertices,
            indices,
            bones: vec![
                bone("a", -1, [0.0, 0.0, 0.0]),
                bone("b", 0, [10.0, 0.0, 0.0]),
                bone("c", 1, [10.0, 0.0, 0.0]),
            ],
        }
    }

    /// THE REGION PIN (spec 0A81088E): a tagged region's vertices take their anchor bone at
    /// 100 % and stay OUT of the Laplacian — neither smoothed (the pin would erode toward the
    /// bones its neighbours own) nor a source (the anchor would smear back across the seam onto
    /// the body, which IS the tail-hair swamp, 2D31782B). The strip's NEAR end is tagged onto the
    /// far bone `c`, which owns nothing within 10 cm of it, so any smear is unmistakable.
    #[test]
    fn a_tagged_region_pins_to_its_anchor_and_stays_out_of_the_smoothing() {
        const C: u32 = 2; // bone `c`, the far end of the strip
        let mut model = strip_fixture();
        let tagged: Vec<u32> = model
            .vertices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.p[0] <= 5.0)
            .map(|(i, _)| i as u32)
            .collect();
        assert!(!tagged.is_empty(), "the strip has a near end");
        model.regions = vec![flicker_skeletal::format::ClothRegion {
            name: "cloth_01".to_string(),
            anchor_bone: "c".to_string(),
            tag: flicker_skeletal::format::RegionTag::Cloth,
            verts: tagged.clone(),
            chain_count: 1,
            params: Default::default(),
            chains: Vec::new(),
            binds: Vec::new(),
        }];
        let mut plain = strip_fixture();
        bake_skin(&mut plain);
        bake_skin(&mut model);
        let anchor = |m: &RawModel, i: usize| {
            let v = &m.vertices[i];
            v.joints
                .iter()
                .zip(&v.weights)
                .filter(|(&j, _)| j == C)
                .map(|(_, &w)| w)
                .sum::<f32>()
        };
        // PINNED: the whole weight on the anchor, un-eroded by the smoothing that follows.
        for &v in &tagged {
            assert_eq!(model.vertices[v as usize].joints[0], C, "vertex {v} pinned");
            assert_eq!(model.vertices[v as usize].weights[0], 1.0, "vertex {v}");
        }
        // NOT A SOURCE: within a smoothing radius of the seam the anchor owns nothing — in the
        // untagged bake AND in the pinned one. A pin that diffused would paint the band.
        for i in 0..model.vertices.len() {
            if tagged.contains(&(i as u32)) || model.vertices[i].p[0] > 10.0 {
                continue;
            }
            assert_eq!(anchor(&plain, i), 0.0, "untagged: `c` owns no vertex {i}");
            assert_eq!(
                anchor(&model, i),
                0.0,
                "vertex {i} at {:?} took anchor weight across the seam",
                model.vertices[i].p
            );
        }
    }

    /// THE STRAY-TRIANGLE GUARD (2026-09-07, GolemBaseV2 under the Katanami set): every corner of
    /// a position carries the SAME weights (a corner-per-vertex mesh must never tear at a bone),
    /// and the transition between two bones is a BAND, not a step — the raw inverse-square pick
    /// flips `a`→`b` between adjacent 1 cm columns (a 0.5 jump at x = 9→10→11), which is exactly
    /// the spike a moving bone pulls out of the mesh. Far from the joint ownership is untouched.
    #[test]
    fn bake_skin_scores_a_position_once_and_blends_the_joint() {
        let mut m = strip_fixture();
        bake_skin(&mut m);
        let share = |v: &RawVertex, bone: u32| -> f32 {
            (0..4)
                .filter(|&k| v.joints[k] == bone)
                .map(|k| v.weights[k])
                .sum()
        };
        let mut by_position: HashMap<[i64; 2], ([u32; 4], [f32; 4])> = HashMap::new();
        let mut a_at: HashMap<i64, f32> = HashMap::new();
        for v in &m.vertices {
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "weights normalise, got {sum}");
            let key = [
                (v.p[0] * 100.0).round() as i64,
                (v.p[1] * 100.0).round() as i64,
            ];
            let set = (v.joints, v.weights);
            let first = *by_position.entry(key).or_insert(set);
            assert_eq!(
                first, set,
                "every corner of one position takes the same weights, differs at {:?}",
                v.p
            );
            a_at.insert(key[0], share(v, 0));
        }
        for x in 0..=3 {
            assert!(
                a_at[&(x * 100)] > 0.9,
                "far from the joint `a` keeps its flesh, at x={x} got {}",
                a_at[&(x * 100)]
            );
        }
        for x in 17..=20 {
            assert!(
                a_at[&(x * 100)] < 0.1,
                "past the joint `a` owns nothing, at x={x} got {}",
                a_at[&(x * 100)]
            );
        }
        for x in 0..20 {
            let (here, next) = (a_at[&(x * 100)], a_at[&((x + 1) * 100)]);
            assert!(
                (here - next).abs() < 0.25,
                "the a→b transition is a band, not a step: {here:.2} → {next:.2} at x={x}"
            );
        }
    }

    /// THE STRAY-TRIANGLE GATE on the promoted content (real package; skips without it): the
    /// ultra golem, CPU-skinned through the same `pose` → `palette` → `skin` path the engine
    /// draws, sheds no spikes under the Katanami set — no more than 0.2% of its triangles may
    /// stretch past 2.5× their rest edge (and 8 cm) at any sampled pose. Before the per-position
    /// bake + blend that number was 2.7% on Attack_1 (2,718 of 62,000) and 0.26% standing still.
    #[test]
    fn the_promoted_golem_sheds_no_stray_triangles_under_the_katanami_set() {
        let package = crate::roots::roots().package();
        let golem = package.join("characters/GolemBaseV2");
        let clips = package.join("retarget/clips/katanami");
        if !crate::package::file_exists(&golem.join("GolemBaseV2.json")) || !clips.is_dir() {
            eprintln!("skipping: no promoted GolemBaseV2 / katanami library");
            return;
        }
        use flicker_skeletal::{pose, skin};
        let model = flicker_skeletal::format::load_dirs(&[&golem, &clips])
            .expect("the golem + the katanami library load");
        let mesh = &model.mesh;
        let tris: Vec<[usize; 3]> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
            .collect();
        let longest = |at: &dyn Fn(usize) -> Vec3, t: &[usize; 3]| -> f32 {
            let (a, b, c) = (at(t[0]), at(t[1]), at(t[2]));
            a.distance(b).max(b.distance(c)).max(c.distance(a))
        };
        let rest: Vec<f32> = tris
            .iter()
            .map(|t| longest(&|i| Vec3::from(mesh.vertices[i].p), t))
            .collect();
        for (name, tick) in [
            ("Idle_nonWeapon", 100u32),
            ("Walk_nonWeapon", 20),
            ("Run_nonWeapon", 20),
            ("Jump_Start", 15),
            ("Attack_1", 25),
        ] {
            let clip = model
                .clips
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("katanami clip {name} in the library"));
            let locals = pose::sample_local_poses(&model.bones, clip, tick, model.retarget);
            let globals = pose::global_transforms(&model.bones, &locals);
            let palette = skin::palette(&model.bones, &globals);
            let posed = skin::skin(mesh, &palette);
            let stray = tris
                .iter()
                .zip(&rest)
                .filter(|(t, &r)| {
                    let l = longest(&|i| Vec3::from(posed[i].position), t);
                    l > 8.0 && l > 2.5 * r.max(0.05)
                })
                .count();
            assert!(
                stray * 500 <= tris.len(),
                "{name} tick {tick}: {stray} of {} triangles stretched past 2.5× their rest edge — the skin bake is shedding spikes again",
                tris.len()
            );
        }
    }

    fn find_character() -> Option<std::path::PathBuf> {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/source/PrismHumanBaseA");
        std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| {
                p.to_string_lossy().contains("Character_output")
                    && p.extension().map(|e| e == "fbx").unwrap_or(false)
            })
    }

    /// A real static-PROP FBX (a weapon) — no skeleton. Skips the prop tests when the content tree
    /// is absent, the same guard the character tests use.
    fn find_prop() -> Option<std::path::PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismWeaps/MuseEpicSet");
        std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().map(|e| e == "fbx").unwrap_or(false))
    }

    /// A one-vertex stand-in, so the write paths can be exercised without a multi-megabyte parse.
    fn tiny_model() -> RawModel {
        RawModel {
            regions: Vec::new(),
            vertices: vec![RawVertex {
                p: [0.0, 0.0, 0.0],
                n: [0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
                joints: [0; 4],
                weights: [0.0; 4],
            }],
            indices: vec![0],
            bones: Vec::new(),
        }
    }

    /// The PROP bake CARRIES ITS TEXTURES — the bug this closes. A prop used to land as a lone
    /// `.json` with the flat placeholder material while its maps stayed behind in the source folder,
    /// so the paperdoll drew it untextured. It now runs the SAME wiring the character import does
    /// (`pipeline::wire_textures`): the maps are copied beside the rig as `<AssetName>_<Map>.png` and
    /// the material points at those basenames.
    ///
    /// Hermetic — a synthetic TWO-piece folder, the shape a Meshy weapon set / outfit really arrives
    /// in, which also pins the two ways this can go quietly wrong: a piece must take ITS OWN maps and
    /// not a sibling's, and the packed `…_metallic_roughness` must not displace the dedicated
    /// `…_metallic`.
    #[test]
    fn write_prop_copies_the_source_maps_beside_the_rig() {
        let root = std::env::temp_dir().join("flicker_content_prop_textures");
        let (src, out_dir) = (root.join("source"), root.join("out"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&src).unwrap();
        let fbx = src.join("Piece_A_texture.fbx");
        for (name, body) in [
            ("Piece_A_texture.fbx", "fbx"),
            ("Piece_A_texture.png", "A-base"),
            ("Piece_A_texture_metallic.png", "A-metal"),
            ("Piece_A_texture_roughness.png", "A-rough"),
            ("Piece_A_texture_normal.png", "A-normal"),
            ("Piece_A_texture_metallic_roughness.png", "A-packed"),
            ("Piece_A_texture_emission.png", "A-emit"),
            // The sibling piece in the same folder — its maps must NOT be picked up.
            ("Piece_B_texture.fbx", "fbx"),
            ("Piece_B_texture.png", "B-base"),
            ("Piece_B_texture_metallic.png", "B-metal"),
        ] {
            std::fs::write(src.join(name), body).unwrap();
        }

        let out = out_dir.join("PieceA.json");
        write_prop(&tiny_model(), &fbx, "PieceA", &out, &Fit::default(), None)
            .expect("the prop writes");

        let rig: RigFile =
            serde_json::from_str(&crate::package::read_text(&out).expect("the rig was written"))
                .unwrap();
        let m = rig.mesh.materials.first().expect("the prop has a material");
        assert_eq!(m.base_color, "PieceA_BaseColor.png", "albedo wired");
        assert_eq!(m.metalness, "PieceA_Metallic.png");
        assert_eq!(m.roughness, "PieceA_Roughness.png");
        assert_eq!(m.normal, "PieceA_Normal.png");
        assert_eq!(
            m.name, "PieceA",
            "named for the asset, as the character import names it"
        );
        assert_eq!(m.slot, "PieceA");

        // The BYTES say which file actually landed: this piece's maps, and the dedicated metallic —
        // not the sibling's albedo, not the packed `metallic_roughness`.
        let beside = |n: &str| {
            std::fs::read_to_string(out_dir.join(n))
                .unwrap_or_else(|e| panic!("{n} must sit beside the rig: {e}"))
        };
        assert_eq!(
            beside("PieceA_BaseColor.png"),
            "A-base",
            "this piece's albedo, not the sibling's"
        );
        assert_eq!(
            beside("PieceA_Metallic.png"),
            "A-metal",
            "the dedicated metallic, not the packed map"
        );
        assert_eq!(beside("PieceA_Roughness.png"), "A-rough");
        assert_eq!(beside("PieceA_Normal.png"), "A-normal");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The GARMENT half of the same fix: a garment takes the BODY's skin weights but keeps its OWN
    /// material, so its source maps travel with it exactly as a prop's do. Needs the fitting body
    /// (`fitting_base`) to skin onto; skips cleanly without it.
    #[test]
    fn write_garment_copies_the_source_maps_beside_the_rig() {
        let body_path = fitting_base();
        let Ok(body_text) = crate::package::read_text(&body_path) else {
            eprintln!("skipping: no fitting body at {}", body_path.display());
            return;
        };
        let body: RigFile = serde_json::from_str(&body_text).expect("the fitting body parses");
        let socket = ["spine_02", "spine_01", "pelvis"]
            .into_iter()
            .find(|s| body.skeleton.bones.iter().any(|b| b.name == *s));
        let (Some(socket), false) = (socket, body.mesh.vertices.is_empty()) else {
            eprintln!("skipping: the fitting body has no torso socket / no mesh");
            return;
        };

        let root = std::env::temp_dir().join("flicker_content_garment_textures");
        let (src, out_dir) = (root.join("source"), root.join("out"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&src).unwrap();
        let fbx = src.join("Top_Duster_texture.fbx");
        for (name, bytes) in [
            ("Top_Duster_texture.fbx", "fbx"),
            ("Top_Duster_texture.png", "duster-base"),
            ("Top_Duster_texture_roughness.png", "duster-rough"),
        ] {
            std::fs::write(src.join(name), bytes).unwrap();
        }

        let out = out_dir.join("Duster.json");
        let fit = Fit {
            socket: socket.to_string(),
            ..Default::default()
        };
        write_garment(
            &tiny_model(),
            &fbx,
            "Duster",
            &out,
            &fit,
            crate::regions::DEFAULT_HANG_CM,
        )
        .expect("the garment writes");

        let rig: RigFile =
            serde_json::from_str(&crate::package::read_text(&out).expect("the rig was written"))
                .unwrap();
        let m = rig
            .mesh
            .materials
            .first()
            .expect("the garment has a material");
        assert_eq!(m.base_color, "Duster_BaseColor.png");
        assert_eq!(m.roughness, "Duster_Roughness.png");
        assert_eq!(
            std::fs::read_to_string(out_dir.join("Duster_BaseColor.png"))
                .expect("copied beside the rig"),
            "duster-base"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same wiring against the REAL folder that showed the bug: the hair prop the user imported
    /// carries five Meshy maps, and its bake must reference + copy the four the material has slots
    /// for (the emission map has no renderer behind it, so it is deliberately left alone). Uses a
    /// stand-in mesh — what is under test is the folder→material wiring, not the 22 MB FBX parse
    /// (`bake_prop_has_export_prop_shape` covers that). Skips when the content tree is absent.
    #[test]
    fn write_prop_wires_the_real_hair_props_maps() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismHumanBaseHairColor");
        let Some(fbx) = std::fs::read_dir(&src).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().map(|e| e == "fbx").unwrap_or(false))
        }) else {
            eprintln!("skipping: no PrismHumanBaseHairColor source");
            return;
        };
        let dir = std::env::temp_dir().join("flicker_content_hair_prop_textures");
        let _ = std::fs::remove_dir_all(&dir);
        let name = "PrismHumanBaseHairColor";
        let out = dir.join(format!("{name}.json"));
        write_prop(&tiny_model(), &fbx, name, &out, &Fit::default(), None)
            .expect("the hair prop writes");

        let rig: RigFile =
            serde_json::from_str(&crate::package::read_text(&out).expect("the rig was written"))
                .unwrap();
        let m = rig.mesh.materials.first().expect("the prop has a material");
        for (slot, got) in [
            ("BaseColor", &m.base_color),
            ("Metallic", &m.metalness),
            ("Roughness", &m.roughness),
            ("Normal", &m.normal),
        ] {
            let want = format!("{name}_{slot}.png");
            assert_eq!(
                got.as_str(),
                want,
                "{slot} referenced by the content-standard basename"
            );
            let copied = dir.join(&want);
            assert!(copied.exists(), "{want} copied beside the rig");
            assert!(
                std::fs::metadata(&copied).unwrap().len() > 0,
                "{want} is not empty"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The PROP bake: a bone-less weapon FBX parses (proving the `parse_fbx` no-skeleton relaxation)
    /// and bakes to the `export_prop` shape — an EMPTY skeleton, the mesh verbatim with its inert
    /// zero skin, `retarget:false`, and crucially NO synthesized root / NO `+1` joint shift. The
    /// Python oracle `Dagger.json` has the same shape.
    #[test]
    fn bake_prop_has_export_prop_shape() {
        let Some(fbx) = find_prop() else {
            eprintln!("skipping: no PrismWeaps source");
            return;
        };
        // The relaxation in `parse_fbx`: a prop FBX with no skeleton parses instead of bailing.
        let model = parse_fbx(&fbx).expect("a bone-less prop FBX parses");
        assert!(model.bones.is_empty(), "a weapon carries no skeleton");
        assert!(!model.vertices.is_empty(), "but it has geometry");

        let rig = bake_prop(&model, "Dagger", None);
        assert!(
            rig.skeleton.bones.is_empty(),
            "a prop bakes with NO skeleton (no synthesized root)"
        );
        assert!(!rig.retarget, "a rigid prop does not retarget");
        assert_eq!(
            rig.mesh.vertices.len(),
            model.vertices.len(),
            "every vertex carried through"
        );
        assert_eq!(
            rig.mesh.indices.len(),
            rig.mesh.vertices.len(),
            "sequential-triple indices"
        );
        assert_eq!(rig.mesh.submeshes.len(), 1);
        assert_eq!(
            rig.mesh.submeshes[0].count,
            rig.mesh.indices.len(),
            "one submesh spans the mesh"
        );
        // The skin is inert and UNSHIFTED — there is no root to shift onto, so `[0;4]` stays `[0;4]`
        // (the character bake's `+1` shift would point these at a non-existent bone 1).
        assert!(
            rig.mesh
                .vertices
                .iter()
                .all(|v| v.joints == [0, 0, 0, 0] && v.weights == [0.0; 4]),
            "an unrigged prop keeps its zero skin verbatim"
        );
        assert_eq!(rig.source.source_axis, "Z_up");
        assert_eq!(rig.source.source_unit, "cm");

        // Round-trips through the real deserializer the paperdoll loads with.
        let json = serde_json::to_string(&rig).unwrap();
        let back: RigFile = serde_json::from_str(&json).unwrap();
        assert!(back.skeleton.bones.is_empty());
        assert_eq!(back.mesh.vertices.len(), rig.mesh.vertices.len());

        // The Python oracle, when present, has the SAME shape — proving the target, not just
        // internal consistency.
        let oracle = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/package/characters/musefit/Dagger.json");
        if let Ok(text) = crate::package::read_text(&oracle) {
            let d: RigFile = serde_json::from_str(&text).expect("oracle Dagger.json parses");
            assert!(d.skeleton.bones.is_empty(), "oracle prop has no skeleton");
            assert!(!d.retarget, "oracle prop is retarget:false");
        }
    }

    /// THE GARMENT BAKE'S REGIONS (spec 0A81088E, T2's owed item 1). Three things on one synthetic
    /// slab body and one plate hanging 20 cm clear of it:
    ///   a) an UNTAGGED garment splits itself at the hang it is handed;
    ///   b) the HANG IS THE KNOB — opened past the gap, nothing reads as cloth;
    ///   c) an AUTHORED row (the bench's Regions panel) SURVIVES: the split does not run at all,
    ///      the human's name/tag/chain count stand, and the comb is still built from the placed
    ///      geometry. Before this, Commit re-split every garment at the default and threw the
    ///      tagging away.
    #[test]
    fn an_authored_garment_keeps_its_rows_and_the_hang_is_the_knob() {
        use crate::flesh::fixtures::{box_mesh, merge};
        use flicker_skeletal::format::{ClothRegion, RegionTag};

        // The socket's rest frame is the IDENTITY, so a garment authored in world coordinates
        // bakes exactly where it was authored and the geometry below reads literally. The body is
        // a torso slab plus a shelf out in front of it, so the gap between them lies INSIDE the
        // body's own grid — a point past that grid hangs by definition (`distance_outside`) and
        // would make the hang unmeasurable.
        let body_mesh = merge(vec![
            box_mesh(-20.0, 20.0, -10.0, 10.0, 0.0, 100.0),
            box_mesh(-20.0, 20.0, 40.0, 50.0, 0.0, 100.0),
        ]);
        let body = RigFile {
            skeleton_recipe: None,
            format: "flicker.rig".to_string(),
            version: 1,
            retarget: true,
            source: Source::default(),
            skeleton: Skeleton {
                bones: vec![
                    BoneRaw {
                        name: "root".into(),
                        parent: -1,
                        local: IDENTITY16,
                        inverse_bind: IDENTITY16,
                    },
                    BoneRaw {
                        name: "chest".into(),
                        parent: 0,
                        local: IDENTITY16,
                        inverse_bind: IDENTITY16,
                    },
                ],
            },
            mesh: Mesh {
                vertices: body_mesh
                    .vertices
                    .iter()
                    .map(|v| Vertex {
                        p: v.p,
                        n: v.n,
                        uv: v.uv,
                        joints: [1, 0, 0, 0],
                        weights: [1.0, 0.0, 0.0, 0.0],
                    })
                    .collect(),
                indices: body_mesh.indices.clone(),
                submeshes: vec![Submesh {
                    material: 0,
                    start: 0,
                    count: body_mesh.indices.len(),
                }],
                materials: vec![Material {
                    name: "m".into(),
                    slot: "m".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            clips: Vec::new(),
            attach: Default::default(),
            attach_points: Vec::new(),
            collision: Default::default(),
        };
        // A plate in the gap, 5–9 cm off the torso's +Y face — the nearest flesh there is.
        let plate = box_mesh(-15.0, 15.0, 15.0, 19.0, 40.0, 80.0);
        let fit = Attach {
            socket: "chest".into(),
            offset: [0.0; 3],
            rotate: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            uniform: 1.0,
        };
        let bake = |g: &RawModel, hang: f32| {
            bake_garment(g, "Plate", &body, "chest", &fit, hang)
                .expect("garment bakes")
                .mesh
                .cloth
                .regions
        };

        // (a) untagged, at the bake's own default: the plate hangs clear, so it splits.
        let split = bake(&plate, crate::regions::DEFAULT_HANG_CM);
        assert!(
            !split.is_empty(),
            "a plate 20 cm off the body reads as cloth at the default hang"
        );
        assert!(
            split.iter().all(|r| !r.verts.is_empty()),
            "a split row owns vertices"
        );

        // (b) the hang IS the knob: opened past the gap, nothing reads as cloth.
        assert!(
            bake(&plate, 12.0).is_empty(),
            "a hang wider than the gap takes none of the piece"
        );

        // (c) AUTHORED rows win — at a hang that would otherwise find nothing, so the only way
        // a row can come out is by having been carried, never re-derived.
        let mut tagged = plate.clone();
        tagged.regions = vec![ClothRegion {
            name: "cape_by_hand".into(),
            anchor_bone: "chest".into(),
            tag: RegionTag::Mane,
            verts: (0..plate.vertices.len() as u32).collect(),
            chain_count: 4,
            params: Default::default(),
            chains: Vec::new(),
            binds: Vec::new(),
        }];
        let kept = bake(&tagged, 12.0);
        assert_eq!(kept.len(), 1, "exactly the authored row, nothing re-split");
        assert_eq!(kept[0].name, "cape_by_hand", "the human's name stands");
        assert_eq!(kept[0].tag, RegionTag::Mane, "and the human's tag");
        assert_eq!(kept[0].chain_count, 4, "and the human's chain count");
        assert_eq!(kept[0].anchor_bone, "chest");
        assert!(
            !kept[0].chains.is_empty(),
            "the comb is still measured from the PLACED geometry"
        );
    }

    /// A synthetic 2-bone body + a one-vertex garment pins the garment bake's two load-bearing
    /// steps without any real asset (so it always runs): the vertex is placed through the SOCKET's
    /// rest frame, and it takes the NEAREST body vertex's skin — the chest-bound one, not the
    /// root-bound one at the origin.
    #[test]
    fn garment_skins_onto_the_nearest_body_vertex() {
        // chest bone rest world = translate +100 z, so its inverse_bind translates −100 z.
        let ib_chest = Mat4::from_translation(Vec3::new(0.0, 0.0, -100.0)).to_cols_array();
        let body = RigFile {
            skeleton_recipe: None,
            format: "flicker.rig".to_string(),
            version: 1,
            retarget: true,
            source: Source::default(),
            skeleton: Skeleton {
                bones: vec![
                    BoneRaw {
                        name: "root".into(),
                        parent: -1,
                        local: IDENTITY16,
                        inverse_bind: IDENTITY16,
                    },
                    BoneRaw {
                        name: "chest".into(),
                        parent: 0,
                        local: IDENTITY16,
                        inverse_bind: ib_chest,
                    },
                ],
            },
            mesh: Mesh {
                vertices: vec![
                    // chest-bound, AT the chest rest position — the nearest to the baked garment vert.
                    Vertex {
                        p: [0.0, 0.0, 100.0],
                        n: [0.0, 0.0, 1.0],
                        uv: [0.0, 0.0],
                        joints: [1, 0, 0, 0],
                        weights: [1.0, 0.0, 0.0, 0.0],
                    },
                    // root-bound, at the origin — farther away, must NOT be chosen.
                    Vertex {
                        p: [0.0, 0.0, 0.0],
                        n: [0.0, 0.0, 1.0],
                        uv: [0.0, 0.0],
                        joints: [0, 0, 0, 0],
                        weights: [1.0, 0.0, 0.0, 0.0],
                    },
                ],
                indices: vec![0, 1, 0],
                submeshes: vec![Submesh {
                    material: 0,
                    start: 0,
                    count: 3,
                }],
                materials: vec![Material {
                    name: "m".into(),
                    slot: "m".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            clips: Vec::new(),
            attach: Default::default(),
            attach_points: Vec::new(),
            collision: Default::default(),
        };
        let garment = RawModel {
            regions: Vec::new(),
            vertices: vec![RawVertex {
                p: [0.0, 0.0, 0.0],
                n: [0.0, 0.0, 1.0],
                uv: [0.3, 0.4],
                joints: [0; 4],
                weights: [0.0; 4],
            }],
            indices: vec![0],
            bones: Vec::new(),
        };
        let fit = Attach {
            socket: "chest".into(),
            offset: [0.0; 3],
            rotate: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            uniform: 1.0,
        };
        let rig = bake_garment(
            &garment,
            "TestGarment",
            &body,
            "chest",
            &fit,
            crate::regions::DEFAULT_HANG_CM,
        )
        .expect("garment bakes");

        // Carries the body's FULL skeleton (so load_outfit's by-name remap is the identity).
        assert_eq!(rig.skeleton.bones.len(), 2);
        assert_eq!(rig.skeleton.bones[1].name, "chest");
        assert!(!rig.retarget, "a garment does not retarget");
        assert_eq!(rig.source.applied_transform, "baked-fit");

        let v = &rig.mesh.vertices[0];
        assert!(
            (Vec3::from(v.p) - Vec3::new(0.0, 0.0, 100.0)).length() < 1e-3,
            "placed at the socket rest frame, got {:?}",
            v.p
        );
        assert_eq!(
            v.joints,
            [1, 0, 0, 0],
            "took the chest-bound body vertex's joints"
        );
        assert_eq!(v.weights, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(v.uv, [0.3, 0.4], "the garment keeps its own UV");
    }

    /// The spatial grid must be EXACT — it has to return what a brute-force argmin would, or a
    /// garment silently skins to the wrong side of a fold (exactly why `skin_outfit.py` refused a
    /// capped search). Checked against a brute-force oracle over a scattered, human-scale body.
    #[test]
    fn vertex_grid_matches_brute_force() {
        let verts: Vec<Vertex> = (0..600u32)
            .map(|i| {
                let f = i as f32;
                Vertex {
                    p: [
                        (f * 7.3) % 60.0 - 30.0,
                        (f * 3.1) % 90.0 - 45.0,
                        (f * 11.7) % 180.0,
                    ],
                    n: [0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                    joints: [i % 60, 0, 0, 0],
                    weights: [1.0, 0.0, 0.0, 0.0],
                }
            })
            .collect();
        let grid = VertexGrid::build(&verts);
        let brute = |p: Vec3| {
            let (mut b, mut bd) = (0usize, f32::INFINITY);
            for (i, v) in verts.iter().enumerate() {
                let d = (Vec3::from(v.p) - p).length_squared();
                if d < bd {
                    bd = d;
                    b = i;
                }
            }
            b
        };
        for k in 0..250u32 {
            let f = k as f32;
            let p = Vec3::new(
                (f * 5.7) % 70.0 - 35.0,
                (f * 2.3) % 100.0 - 50.0,
                (f * 13.1) % 200.0 - 10.0,
            );
            let (g, b) = (grid.nearest(p, &verts), brute(p));
            // Ties are legitimate, so compare the DISTANCE — what the transfer actually depends on.
            let dg = (Vec3::from(verts[g].p) - p).length_squared();
            let db = (Vec3::from(verts[b].p) - p).length_squared();
            assert!(
                (dg - db).abs() < 1e-4,
                "grid picked a farther vertex at {p}: {dg} vs {db}"
            );
        }
    }

    /// END-TO-END on REAL assets: parse a raw Muse fit FBX, skin it onto a baked body, and confirm
    /// a valid outfit overlay — the body's full skeleton, every vertex re-skinned to real body
    /// bones with normalised weights. `#[ignore]` (exact nearest-vertex over ~12k body × ~44k
    /// garment verts is heavy in debug); run explicitly:
    ///   cargo test -p flicker-content -- --ignored bake_garment_real
    #[test]
    #[ignore]
    fn bake_garment_real_skins_a_muse_fit_onto_the_body() {
        // The FITTING body (the clay Golem when present) — the same one `write_garment` skins onto.
        let body_path = fitting_base();
        let Ok(body_text) = crate::package::read_text(&body_path) else {
            eprintln!("skipping: no baked body at {}", body_path.display());
            return;
        };
        let body: RigFile = serde_json::from_str(&body_text).expect("baked body parses");
        if body.mesh.vertices.is_empty() || body.skeleton.bones.is_empty() {
            eprintln!("skipping: body carries no skinned mesh");
            return;
        }
        let fit_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/source/PrismFits/Muse001");
        let Some(fbx) = std::fs::read_dir(&fit_dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().map(|e| e == "fbx").unwrap_or(false))
        }) else {
            eprintln!("skipping: no Muse fit source");
            return;
        };
        let garment = parse_fbx(&fbx).expect("the raw garment FBX parses");
        assert!(
            garment.bones.is_empty(),
            "a raw Meshy garment carries no skeleton"
        );

        let socket = ["spine_02", "spine_01", "pelvis"]
            .into_iter()
            .find(|s| body.skeleton.bones.iter().any(|b| b.name == *s))
            .expect("the body has a torso socket");
        let fit = Attach {
            socket: socket.into(),
            offset: [0.0; 3],
            rotate: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            uniform: 1.0,
        };
        let rig = bake_garment(
            &garment,
            "Muse001",
            &body,
            socket,
            &fit,
            crate::regions::DEFAULT_HANG_CM,
        )
        .expect("garment bakes");

        assert_eq!(
            rig.skeleton.bones.len(),
            body.skeleton.bones.len(),
            "carries the body's full skeleton"
        );
        assert_eq!(
            rig.mesh.vertices.len(),
            garment.vertices.len(),
            "every garment vertex skinned"
        );
        assert_eq!(rig.source.applied_transform, "baked-fit");
        let nb = body.skeleton.bones.len() as u32;
        for v in &rig.mesh.vertices {
            assert!(
                v.joints.iter().all(|&j| j < nb),
                "joints index the body skeleton"
            );
            let w: f32 = v.weights.iter().sum();
            assert!(
                (w - 1.0).abs() < 0.05 || w == 0.0,
                "transferred weights normalised, got {w}"
            );
            assert!(Vec3::from(v.p).is_finite() && Vec3::from(v.n).is_finite());
        }
        // Round-trips through the real deserializer the paperdoll loads with.
        let json = serde_json::to_string(&rig).unwrap();
        let _back: RigFile = serde_json::from_str(&json).unwrap();
        eprintln!(
            "baked {} garment verts onto {} body bones",
            rig.mesh.vertices.len(),
            nb
        );
    }

    /// The bake of the conformed female base yields the oracle's shape: 66 bones with a synthesized
    /// identity `root` at 0, `pelvis` parented to it, the face group under `head`, and the mesh
    /// carried through with joints shifted for the root.
    #[test]
    fn bake_of_conform_has_oracle_shape() {
        let (Some(fbx), reference) = (find_character(), default_reference()) else {
            eprintln!("skipping: no source / reference");
            return;
        };
        if !reference.exists() {
            return;
        }
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        conform_to_canonical(&mut model, &reference, ConformMode::Canonical).unwrap();
        let rig = bake_rig(&model, "PrismHumanBaseA");

        assert_eq!(
            rig.skeleton.bones.len(),
            66,
            "65 conformed + synthesized root"
        );
        assert_eq!(rig.skeleton.bones[0].name, "root");
        assert_eq!(rig.skeleton.bones[0].parent, -1);
        assert_eq!(rig.skeleton.bones[0].local, IDENTITY16, "root is identity");
        let by_name = |n: &str| rig.skeleton.bones.iter().position(|b| b.name == n);
        let (pelvis, head) = (by_name("pelvis").unwrap(), by_name("head").unwrap());
        assert_eq!(
            rig.skeleton.bones[pelvis].parent, 0,
            "pelvis parents to root"
        );
        for n in ["jaw", "eye_l", "eye_r"] {
            assert_eq!(
                rig.skeleton.bones[by_name(n).unwrap()].parent as usize,
                head,
                "{n} under head"
            );
        }
        // Every parent index is valid, and the tree is topological (parent precedes child).
        for (i, b) in rig.skeleton.bones.iter().enumerate() {
            assert!(
                b.parent == -1 || (b.parent >= 0 && (b.parent as usize) < i),
                "bone {i} '{}' parent {} precedes it",
                b.name,
                b.parent
            );
        }
        assert_eq!(rig.mesh.vertices.len(), model.vertices.len());
        assert_eq!(rig.mesh.submeshes.len(), 1);
        assert_eq!(rig.mesh.submeshes[0].count, rig.mesh.indices.len());
        // Joints are valid bone indices (shifted into 1..=65, weighted ones real).
        let nb = rig.skeleton.bones.len() as u32;
        assert!(rig
            .mesh
            .vertices
            .iter()
            .all(|v| v.joints.iter().all(|&j| j < nb)));
    }

    /// END-TO-END real pipeline: parse → rename → conform → bake → WRITE the flicker.rig → reload it
    /// through the real `flicker_skeletal::format::load_dir`. Confirms the in-app bake produces a file
    /// the engine loader actually accepts (66 bones, mesh present), and reports its size. `#[ignore]`d
    /// because it writes a large JSON; run explicitly:
    ///   `cargo test -p flicker-content -- --ignored bakes_and_reloads`
    #[test]
    #[ignore]
    fn bakes_and_reloads_through_the_engine_loader() {
        let (Some(fbx), reference) = (find_character(), default_reference()) else {
            return;
        };
        if !reference.exists() {
            return;
        }
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        conform_to_canonical(&mut model, &reference, ConformMode::Canonical).unwrap();

        let dir = std::env::temp_dir().join("flicker_content_bake_e2e");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("PrismHumanBaseA.json");
        write_rig(&model, &fbx, "PrismHumanBaseA", &out, &[], None).unwrap();
        let bytes = std::fs::metadata(&out).unwrap().len();
        eprintln!("wrote {} ({:.1} MB)", out.display(), bytes as f64 / 1.0e6);

        let loaded =
            flicker_skeletal::format::load_dir(&dir).expect("engine loader accepts the bake");
        eprintln!(
            "engine loaded {} bones, {} verts",
            loaded.bones.len(),
            loaded.mesh.vertices.len()
        );
        assert_eq!(loaded.bones.len(), 66, "engine sees 66 bones");
        assert_eq!(
            loaded.mesh.vertices.len(),
            model.vertices.len(),
            "mesh carried through"
        );
        assert!(
            loaded.bones.iter().any(|b| b.name == "root"),
            "root present"
        );
        // `write_rig` is the EDITOR's commit path (the CLI `import_folder` wires its own). It must
        // bring the source's maps along, or a character committed from the bench ships UNTEXTURED —
        // exactly the gap props carried until they were routed through the same call. Guarded here
        // because that gap was rediscovered once already.
        assert_eq!(
            loaded.mesh.materials.first().map(|m| m.base_color.as_str()),
            Some("PrismHumanBaseA_BaseColor.png"),
            "the committed character's material must reference its copied albedo"
        );
        assert!(
            dir.join("PrismHumanBaseA_BaseColor.png").exists(),
            "and the map itself is written beside the rig"
        );
        // Provenance too, not only the material: `source.textures` is the manifest a validator reads
        // to answer "does this asset carry its maps?" — it must list what was actually copied.
        assert!(
            loaded
                .source
                .textures
                .iter()
                .any(|t| t == "PrismHumanBaseA_BaseColor.png"),
            "the copied maps are recorded in the rig's source provenance, got {:?}",
            loaded.source.textures
        );
        // Whole dir: the bake now leaves ~57 MB of copied maps beside the rig, not just the JSON.
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The canonical types serialize and the JSON round-trips through the real `flicker-skeletal`
    /// deserializer — proving the additive `Serialize` derive and the root synthesis on a tiny model.
    #[test]
    fn serialized_rig_round_trips_through_the_loader() {
        let model = RawModel {
            regions: Vec::new(),
            vertices: vec![RawVertex {
                p: [1.0, 2.0, 3.0],
                n: [0.0, 0.0, 1.0],
                uv: [0.25, 0.5],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            }],
            indices: vec![0],
            bones: vec![
                RawBone {
                    name: "pelvis".into(),
                    parent: -1,
                    translation: [0.0, 0.0, 95.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                    inverse_bind: IDENTITY16,
                },
                RawBone {
                    name: "spine_01".into(),
                    parent: 0,
                    translation: [0.0, 0.0, 10.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                    inverse_bind: IDENTITY16,
                },
            ],
        };
        let rig = bake_rig(&model, "tiny");
        let json = serde_json::to_string(&rig).unwrap();
        let back: RigFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.skeleton.bones.len(), 3, "root + pelvis + spine_01");
        assert_eq!(back.skeleton.bones[0].name, "root");
        assert_eq!(back.skeleton.bones[1].name, "pelvis");
        assert_eq!(
            back.skeleton.bones[1].parent, 0,
            "pelvis re-parented onto root"
        );
        assert_eq!(
            back.skeleton.bones[2].parent, 1,
            "spine_01 parent shifted +1"
        );
        assert_eq!(
            back.mesh.vertices[0].joints,
            [1, 1, 1, 1],
            "joints shifted for the root"
        );
        assert!(back.retarget);
        assert_eq!(back.source.source_axis, "Z_up");
    }

    /// THE ROOT UNDER A QUADRUPED'S BACK (P3): a trunk whose spine runs along the body bakes
    /// carried across so the MIDDLE of its back — pelvis to spine_03 — stands over the root,
    /// not its tail end.
    #[test]
    fn the_baked_root_sits_under_a_quadrupeds_back() {
        let bone = |name: &str, parent: i32, t: [f32; 3], world: Vec3| RawBone {
            name: name.into(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            inverse_bind: Mat4::from_translation(world).inverse().to_cols_array(),
        };
        let vert = |p: [f32; 3], joint: u32| RawVertex {
            p,
            n: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let model = RawModel {
            regions: Vec::new(),
            vertices: vec![vert([10.0, 40.0, 90.0], 0), vert([10.0, -60.0, 100.0], 1)],
            indices: vec![0, 1, 0],
            bones: vec![
                bone(
                    "pelvis",
                    -1,
                    [10.0, 40.0, 90.0],
                    Vec3::new(10.0, 40.0, 90.0),
                ),
                bone(
                    "spine_03",
                    0,
                    [0.0, -100.0, 10.0],
                    Vec3::new(10.0, -60.0, 100.0),
                ),
            ],
        };
        let rig = bake_rig(&model, "quadruped");
        let bones = flicker_skeletal::format::rig_bones(&rig);
        let world = flicker_skeletal::pose::global_transforms(
            &bones,
            &bones.iter().map(|b| b.local).collect::<Vec<_>>(),
        );
        let pelvis = world[1].w_axis.truncate();
        let spine_03 = world[2].w_axis.truncate();
        assert!(
            (pelvis.x.abs() < 1e-4) && ((pelvis.y - 50.0).abs() < 1e-4),
            "the pelvis sits half a body behind the root: {pelvis}"
        );
        assert!(
            (spine_03.y + 50.0).abs() < 1e-4 && (spine_03.z - 100.0).abs() < 1e-4,
            "the withers sit half a body ahead: {spine_03}"
        );
        assert_eq!(
            rig.mesh.vertices[0].p,
            [0.0, 50.0, 90.0],
            "the mesh came across"
        );
    }

    /// THE ROOT UNDER THE PELVIS (Aaron 2026-09-07): a body whose pelvis stands off the origin's
    /// plumb line bakes carried across so the pelvis is over the root, every vertex with it, and
    /// the rest skin still the identity; a body already on the line bakes untouched.
    #[test]
    fn the_baked_root_sits_under_the_pelvis() {
        let bone = |name: &str, parent: i32, t: [f32; 3], world: Vec3| RawBone {
            name: name.into(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            inverse_bind: Mat4::from_translation(world).inverse().to_cols_array(),
        };
        let vert = |p: [f32; 3], joint: u32| RawVertex {
            p,
            n: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            joints: [joint, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let model = RawModel {
            regions: Vec::new(),
            vertices: vec![vert([10.0, 5.0, 90.0], 0), vert([10.0, 5.0, 120.0], 1)],
            indices: vec![0, 1, 0],
            bones: vec![
                bone("pelvis", -1, [10.0, 5.0, 90.0], Vec3::new(10.0, 5.0, 90.0)),
                bone("spine_01", 0, [0.0, 0.0, 30.0], Vec3::new(10.0, 5.0, 120.0)),
            ],
        };
        let rig = bake_rig(&model, "offcentre");
        let bones = flicker_skeletal::format::rig_bones(&rig);
        let world = flicker_skeletal::pose::global_transforms(
            &bones,
            &bones.iter().map(|b| b.local).collect::<Vec<_>>(),
        );
        let pelvis = world[1].w_axis.truncate();
        assert!(
            pelvis.x.abs() < 1e-4 && pelvis.y.abs() < 1e-4 && (pelvis.z - 90.0).abs() < 1e-4,
            "the pelvis stands over the root: {pelvis}"
        );
        assert_eq!(
            rig.mesh.vertices[0].p,
            [0.0, 0.0, 90.0],
            "the mesh came across"
        );
        for (b, w) in bones.iter().zip(&world) {
            let palette = *w * b.inverse_bind;
            assert!(
                (palette - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-4),
                "{}: rest skinning stays the identity",
                b.name
            );
        }
        // A body on the line is untouched — bit for bit.
        let centred = RawModel {
            regions: Vec::new(),
            vertices: vec![vert([0.0, 0.0, 90.0], 0)],
            indices: vec![0, 0, 0],
            bones: vec![bone(
                "pelvis",
                -1,
                [0.0, 0.0, 90.0],
                Vec3::new(0.0, 0.0, 90.0),
            )],
        };
        let rig = bake_rig(&centred, "centred");
        assert_eq!(rig.mesh.vertices[0].p, [0.0, 0.0, 90.0]);
    }

    // ── THE STANCE NORMALISER (ruling 42AB9BA8, amended FEFDA2B2) ────────────────────────────

    /// A synthetic MID-STRIDE biped: a box body on two legs, plus a pair of ARMS whose ground
    /// joints hang far above the ground. `knee_l`/`foot_l` pose the LEFT leg and `foot_r` plants
    /// the right one (its knee runs straight down at x = −10), so passing (10, 0, 45) /
    /// (10, 0, 5) / (−10, 0, 5) builds the SQUARED body and anything else a stride; `top` is the
    /// body's highest vertex, which is what [`RAISED_FRACTION`] is a fraction OF. The lowest
    /// vertex sits at z = 0, so a foot's own z IS its height off the floor. A handful of skinned
    /// verts per bone, weights hand-set (the normaliser must not touch them).
    fn stance_model(knee_l: Vec3, foot_l: Vec3, foot_r: Vec3, top: f32) -> RawModel {
        let w: [(&str, i32, Vec3); 13] = [
            ("root", -1, Vec3::new(0.0, 0.0, 0.0)),
            ("pelvis", 0, Vec3::new(0.0, 0.0, 90.0)),
            ("thigh_l", 1, Vec3::new(10.0, 0.0, 85.0)),
            ("thigh_r", 1, Vec3::new(-10.0, 0.0, 85.0)),
            ("calf_l", 2, knee_l),
            ("calf_r", 3, Vec3::new(-10.0, 0.0, 45.0)),
            ("foot_l", 4, foot_l),
            ("foot_r", 5, foot_r),
            ("spine_01", 1, Vec3::new(0.0, 0.0, 100.0)),
            ("clavicle_l", 8, Vec3::new(8.0, 0.0, 115.0)),
            ("clavicle_r", 8, Vec3::new(-8.0, 0.0, 115.0)),
            ("hand_l", 9, Vec3::new(20.0, 0.0, 80.0)),
            ("hand_r", 10, Vec3::new(-20.0, 0.0, 60.0)),
        ];
        let bones = w
            .iter()
            .map(|&(name, parent, world)| {
                let p = usize::try_from(parent).ok().map_or(Vec3::ZERO, |i| w[i].2);
                RawBone {
                    name: name.to_string(),
                    parent,
                    translation: (world - p).to_array(),
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                    inverse_bind: Mat4::from_translation(world).inverse().to_cols_array(),
                }
            })
            .collect();
        let v = |p: Vec3, skin: &[(usize, f32)]| {
            let (mut joints, mut weights) = ([0u32; 4], [0.0f32; 4]);
            for (k, &(j, wt)) in skin.iter().enumerate() {
                joints[k] = j as u32;
                weights[k] = wt;
            }
            RawVertex {
                p: p.to_array(),
                n: [0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                joints,
                weights,
            }
        };
        let mid_thigh = (w[2].2 + knee_l) * 0.5;
        RawModel {
            regions: Vec::new(),
            vertices: vec![
                v(mid_thigh, &[(2, 1.0)]),                  // 0 — all thigh_l
                v(mid_thigh, &[(2, 0.5), (1, 0.5)]),        // 1 — half thigh_l, half pelvis
                v(foot_l, &[(6, 1.0)]), // 2 — all foot_l, AT the raised ground joint
                v(w[7].2, &[(7, 1.0)]), // 3 — all foot_r, the planted side
                v(Vec3::new(0.0, 0.0, top), &[(1, 1.0)]), // 4 — the top of the body
                v(Vec3::new(-10.0, 0.0, 0.0), &[(7, 1.0)]), // 5 — the ground under the planted foot
                v(w[11].2, &[(11, 1.0)]), // 6 — the left hand
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            bones,
        }
    }

    fn mid_stride_model() -> RawModel {
        // The knee bent forward and the foot 10 cm above its twin's — one stride.
        stance_model(
            Vec3::new(10.0, 10.0, 50.0),
            Vec3::new(10.0, 5.0, 15.0),
            Vec3::new(-10.0, 0.0, 5.0),
            120.0,
        )
    }

    fn world_of(model: &RawModel, name: &str) -> Vec3 {
        let i = model.bones.iter().position(|b| b.name == name).unwrap();
        crate::conform::model_world_frames(model)[i]
            .w_axis
            .truncate()
    }

    /// SQUARE THE STANCE (ruling 42AB9BA8): a mid-stride body is promoted standing on both feet —
    /// the raised limb put at the planted one's reflection, the SKIN carried into it by the
    /// weighted blend of the bones' rigid motions, and the whole thing re-bound square. The planted
    /// side, the weights and the arms are none of its business.
    #[test]
    fn square_stance_puts_the_raised_leg_at_its_twins_reflection_and_the_skin_follows() {
        let before = mid_stride_model();
        let mut m = mid_stride_model();
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(
            report.squared.len(),
            1,
            "one limb squared, not {:?}",
            report.squared
        );
        assert_eq!(report.squared[0].0, "thigh_l");
        assert!(
            (report.squared[0].1 - 10.0).abs() < 1e-3,
            "the lift is 10 cm, got {}",
            report.squared[0].1
        );
        // Both ground joints at one height, and every raised bone at its twin's reflection.
        let foot_l = world_of(&m, "foot_l");
        let foot_r = world_of(&m, "foot_r");
        assert!(
            (foot_l.z - foot_r.z).abs() < 1e-3,
            "the feet stand level: {} vs {}",
            foot_l.z,
            foot_r.z
        );
        for (l, r) in [
            ("thigh_l", "thigh_r"),
            ("calf_l", "calf_r"),
            ("foot_l", "foot_r"),
        ] {
            let (a, b) = (world_of(&m, l), world_of(&m, r));
            assert!(
                a.distance(Vec3::new(-b.x, b.y, b.z)) < 1e-3,
                "{l} is {r}'s reflection: {a} vs {b}"
            );
        }
        // The PLANTED side never moved, and neither did the arms (ground joints far off the floor).
        for name in ["thigh_r", "calf_r", "foot_r", "pelvis", "hand_l", "hand_r"] {
            assert!(
                world_of(&m, name).distance(world_of(&before, name)) < 1e-6,
                "{name} stays where it was"
            );
        }
        // The skin. A vert bound wholly to the raised foot lands at the mirrored foot.
        let hoof = Vec3::from_array(m.vertices[2].p);
        assert!(
            hoof.distance(Vec3::new(10.0, 0.0, 5.0)) < 1e-3,
            "the hoof vert lands at the mirrored hoof, got {hoof}"
        );
        // A vert half thigh, half pelvis moves HALF what the same vert bound wholly to the thigh
        // does — the pelvis is outside the limb and contributes identity.
        let start = Vec3::from_array(before.vertices[0].p);
        let full = Vec3::from_array(m.vertices[0].p) - start;
        let half = Vec3::from_array(m.vertices[1].p) - start;
        assert!(
            full.length() > 1.0,
            "the thigh really moves its flesh: {full}"
        );
        assert!(
            half.distance(full * 0.5) < 1e-3,
            "half the weight, half the motion: {half} vs {}",
            full * 0.5
        );
        // Flesh on the planted side, on the body and on the arm is untouched; so are ALL weights.
        for i in [3usize, 4, 5, 6] {
            assert_eq!(
                m.vertices[i].p, before.vertices[i].p,
                "vertex {i} is not the normaliser's business"
            );
        }
        for (a, b) in m.vertices.iter().zip(&before.vertices) {
            assert_eq!(
                (a.joints, a.weights),
                (b.joints, b.weights),
                "weights stand"
            );
        }
        // RE-BOUND SQUARE: rest-pose skinning is the identity at the new stance.
        let rest = crate::conform::model_world_frames(&m);
        for (b, g) in m.bones.iter().zip(&rest) {
            let palette = *g * Mat4::from_cols_array(&b.inverse_bind);
            assert!(
                (palette - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-4),
                "{}: the bind is square",
                b.name
            );
        }
    }

    /// A body that already stands square is left BIT FOR BIT alone — under every source, because a
    /// forced side that is already its twin's reflection has nothing to say.
    #[test]
    fn square_stance_leaves_a_square_body_exactly_as_it_found_it() {
        let squared_body = || {
            stance_model(
                Vec3::new(10.0, 0.0, 45.0),
                Vec3::new(10.0, 0.0, 5.0),
                Vec3::new(-10.0, 0.0, 5.0),
                120.0,
            )
        };
        let square = squared_body();
        for source in [StanceSource::Auto, StanceSource::Left, StanceSource::Right] {
            let mut m = squared_body();
            let report = square_stance(&mut m, source, &SkeletonRecipe::humanoid());
            assert!(
                report.squared.is_empty(),
                "{source:?}: nothing to square, got {:?}",
                report.squared
            );
            assert_eq!(
                format!("{m:?}"),
                format!("{square:?}"),
                "{source:?}: the model is untouched"
            );
        }
    }

    /// A body STANDING ON ONE SIDE is squared off a raise the HEIGHT FRACTION would have ignored.
    /// The sweep 45C8EBAB left six bodies mid-stride — Hippo 3.1 cm, Elephant 4.0, Boar 4.6,
    /// Rabbit 4.8 — because each raise was under [`RAISED_FRACTION`] of the body's height (5.1 cm
    /// on a 170 cm body). Once a ground joint is ON THE FLOOR, only noise is ignored.
    #[test]
    fn a_hoof_up_over_a_grounded_twin_is_squared_under_the_height_fraction() {
        // 170 cm body: the old rule wanted 5.1 cm of lift before it would look. This hoof has 4,
        // and its twin is on the floor.
        let mut m = stance_model(
            Vec3::new(10.0, 0.0, 45.0),
            Vec3::new(10.0, 0.0, 4.0),
            Vec3::new(-10.0, 0.0, 0.0),
            170.0,
        );
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(
            report.squared.len(),
            1,
            "the raised side is squared, got {:?}",
            report.squared
        );
        assert_eq!(report.squared[0].0, "thigh_l");
        assert!(
            (report.squared[0].1 - 4.0).abs() < 1e-3,
            "4 cm of lift, got {}",
            report.squared[0].1
        );
        let (l, r) = (world_of(&m, "foot_l"), world_of(&m, "foot_r"));
        assert!(
            l.z.abs() < 1e-3 && r.z.abs() < 1e-3,
            "both feet are on the floor now: {l} / {r}"
        );
        // The ARMS hang far above the ground band and are still none of the normaliser's business.
        for n in ["clavicle_l", "clavicle_r", "hand_l", "hand_r"] {
            let posed = world_of(
                &stance_model(
                    Vec3::new(10.0, 0.0, 45.0),
                    Vec3::new(10.0, 0.0, 4.0),
                    Vec3::new(-10.0, 0.0, 0.0),
                    170.0,
                ),
                n,
            );
            assert!(world_of(&m, n).distance(posed) < 1e-6, "{n} is untouched");
        }
    }

    /// THE FLOOR IS THE MESH'S, NOT THE SKELETON'S. A module the fit left UNMATCHED keeps its
    /// composed rest and is prompted on the rail (spec 04803E0C §3), so a body can carry a joint
    /// dangling far UNDER its own mesh — the Hippo's hind hooves sit 32 cm below its belly. Taking
    /// that as the ground would put every real ground joint tens of centimetres "up" and no pair
    /// would ever read as standing.
    #[test]
    fn a_joint_dangling_under_the_mesh_is_not_the_floor() {
        let mut m = stance_model(
            Vec3::new(10.0, 0.0, 45.0),
            Vec3::new(10.0, 0.0, 4.0),
            Vec3::new(-10.0, 0.0, 0.0),
            170.0,
        );
        // A MIDLINE joint (never a limb root, so it adds no pair) 30 cm below the mesh.
        let pelvis = m.bones.iter().position(|b| b.name == "pelvis").unwrap();
        let hang = Vec3::new(0.0, -20.0, -30.0);
        m.bones.push(RawBone {
            name: "tail_01".to_string(),
            parent: i32::try_from(pelvis).unwrap(),
            translation: (hang - Vec3::new(0.0, 0.0, 90.0)).to_array(),
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: Mat4::from_translation(hang).inverse().to_cols_array(),
        });
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(
            report.squared.len(),
            1,
            "the raised side is still squared off the MESH's floor, got {:?}",
            report.squared
        );
        assert!(
            world_of(&m, "foot_l").z.abs() < 1e-3,
            "the raised foot is down: {}",
            world_of(&m, "foot_l").z
        );
    }

    /// A MID-STRIDE body whose two SOCKETS are not each other's mirror — the swinging limb's hip
    /// carried forward of its twin's, which is what the fit reads off the mesh's own tubes once
    /// each limb is laid down its own (4FF16605) — and a slab of TRUNK flesh bound to that hip.
    /// `hip_l` places the left socket; everything else is [`stance_model`]'s biped.
    fn strided_socket_model(hip_l: Vec3, foot_l: Vec3, foot_r: Vec3) -> RawModel {
        let mut m = stance_model(Vec3::new(hip_l.x, hip_l.y, 45.0), foot_l, foot_r, 170.0);
        // Move the LEFT socket off its twin's reflection. Its children keep the world positions
        // they were built at, so only the socket's own local offset changes.
        let name_at = |n: &str| m.bones.iter().position(|b| b.name == n).unwrap();
        let hip = name_at("thigh_l");
        let pelvis = name_at("pelvis");
        let calf = name_at("calf_l");
        let parent = Vec3::from_array(m.bones[pelvis].translation)
            + Vec3::from_array(m.bones[0].translation);
        m.bones[hip].translation = (hip_l - parent).to_array();
        m.bones[hip].inverse_bind = Mat4::from_translation(hip_l).inverse().to_cols_array();
        m.bones[calf].translation = (Vec3::new(hip_l.x, hip_l.y, 45.0) - hip_l).to_array();
        // A SLAB OF TRUNK bound to the socket — the shoulder/hip skirt of a real body, reaching
        // 40 cm away from it, and the deepest mesh vertex sits on the floor at z = 0.
        let seed = m.vertices[0];
        let slab = |p: Vec3| {
            let mut v = seed;
            v.p = p.to_array();
            v.joints = [u32::try_from(hip).unwrap(), 0, 0, 0];
            v.weights = [1.0, 0.0, 0.0, 0.0];
            v
        };
        let (a, b) = (
            slab(hip_l + Vec3::new(0.0, -40.0, 0.0)),
            slab(hip_l + Vec3::new(0.0, 0.0, 35.0)),
        );
        m.vertices.push(a);
        m.vertices.push(b);
        m
    }

    /// THE SOCKET IS THE BODY, NOT THE POSE — and squaring a stride must never push the mesh
    /// through the floor it is standing on. Reflecting the RAISED limb's socket onto its twin's
    /// moved the socket itself, and [`follow_and_rebind`] carried every vertex weighted to it: on
    /// the 46-body sweep that is a skirt of TRUNK flesh, and the Ram's mesh ended 33.7 cm UNDER
    /// z = 0 on a 13.8 cm lift, the ElkBull's 28.3, the Lizard's 18.3 (incident 7CF34E04). Since
    /// the floor every ground joint is measured from is the MESH's own lowest point, a body whose
    /// flesh hangs below its feet reads as if every hoof were in the air.
    #[test]
    fn squaring_a_stride_moves_the_limb_and_not_the_socket_it_hangs_on() {
        let strided = || {
            strided_socket_model(
                Vec3::new(10.0, 25.0, 85.0), // the left hip 25 cm forward of the right one
                Vec3::new(10.0, 20.0, 12.0), // ... and its foot 12 cm up
                Vec3::new(-10.0, 0.0, 0.0),  // the right foot planted
            )
        };
        let before = strided();
        let mut m = strided();
        let floor = before
            .vertices
            .iter()
            .map(|v| v.p[2])
            .fold(f32::INFINITY, f32::min);
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(
            report.squared.len(),
            1,
            "the raised side is squared, got {:?}",
            report.squared
        );

        // THE SOCKET DID NOT MOVE — the body is where the fit measured it.
        let hip = world_of(&m, "thigh_l");
        assert!(
            hip.distance(world_of(&before, "thigh_l")) < 1e-3,
            "the socket stays at {}, went to {hip}",
            world_of(&before, "thigh_l")
        );
        // ... AND THE FLESH HANGING ON IT DID NOT GO THROUGH THE FLOOR.
        let sank = m
            .vertices
            .iter()
            .map(|v| v.p[2])
            .fold(f32::INFINITY, f32::min);
        assert!(
            sank >= floor - 1e-3,
            "the mesh keeps its floor: {floor} before, {sank} after"
        );
        // THE POSE IS STILL SQUARED: the chain below the socket takes its twin's shape, so the
        // two feet stand level — the sockets are at one height, so nothing is owed to asymmetry.
        let (foot_l, foot_r) = (world_of(&m, "foot_l"), world_of(&m, "foot_r"));
        assert!(
            (foot_l.z - foot_r.z).abs() < 1e-3,
            "the feet stand level: {} vs {}",
            foot_l.z,
            foot_r.z
        );
        assert!(
            foot_l.z.abs() < 1e-3,
            "the raised foot is down on the floor, got {}",
            foot_l.z
        );
        // The planted side and the body are still none of its business.
        for name in ["thigh_r", "calf_r", "foot_r", "pelvis", "hand_l", "hand_r"] {
            assert!(
                world_of(&m, name).distance(world_of(&before, name)) < 1e-6,
                "{name} stays where it was"
            );
        }
    }

    /// A SQUARE THAT WOULD PUT THE BODY UNDER ITS OWN FLOOR IS DECLINED, and said so. The skin
    /// follow carries whatever the moving bones hold weight over, and on a real source a limb
    /// re-posed across a long stride drags the belly, the tail's fall or the other side's paw with
    /// it: the Lizard's mesh ended 40.6 cm below the ground it was standing on (7CF34E04). A body
    /// whose flesh hangs beneath its feet has no floor left to measure a hoof against, so the
    /// normaliser leaves that limb as posed — the rail still has it.
    #[test]
    fn a_square_that_would_drag_the_mesh_under_the_floor_is_declined() {
        let mut m = strided_socket_model(
            Vec3::new(10.0, 25.0, 85.0),
            Vec3::new(10.0, 20.0, 12.0),
            Vec3::new(-10.0, 0.0, 0.0),
        );
        // Flesh ON THE FLOOR bound wholly to the RAISED foot — squaring drags it 12 cm under.
        let foot = m.bones.iter().position(|b| b.name == "foot_l").unwrap();
        let mut sole = m.vertices[0];
        sole.p = [10.0, 20.0, 0.0];
        sole.joints = [u32::try_from(foot).unwrap(), 0, 0, 0];
        sole.weights = [1.0, 0.0, 0.0, 0.0];
        m.vertices.push(sole);
        let before = m.clone();
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert!(
            report.squared.is_empty(),
            "nothing is squared, got {:?}",
            report.squared
        );
        assert_eq!(report.declined.len(), 1, "the limb is declined, loudly");
        assert_eq!(report.declined[0].0, "thigh_l");
        assert!(
            (report.declined[0].1 - 12.0).abs() < 0.5,
            "it says how far under: {} cm",
            report.declined[0].1
        );
        assert_eq!(
            format!("{m:?}"),
            format!("{before:?}"),
            "a declined body is left exactly as it was posed"
        );
    }

    /// … AND THE NOISE FLOOR STILL HOLDS. A centimetre between two feet that are both on the
    /// floor is an authoring wobble, not a stride, whichever side is lower.
    #[test]
    fn a_centimetre_between_two_grounded_feet_is_noise_and_nothing_moves() {
        let wobbly = || {
            stance_model(
                Vec3::new(10.0, 0.0, 45.0),
                Vec3::new(10.0, 0.0, 1.0),
                Vec3::new(-10.0, 0.0, 0.0),
                170.0,
            )
        };
        let before = wobbly();
        let mut m = wobbly();
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert!(
            report.squared.is_empty(),
            "a centimetre is noise, got {:?}",
            report.squared
        );
        assert_eq!(
            format!("{m:?}"),
            format!("{before:?}"),
            "the model is untouched"
        );
    }

    /// THE SOURCE SIDE IS SELECTABLE (Aaron FEFDA2B2: *"there's no guarantee it will be one
    /// specific side"*). `Left`/`Right` name the side to mirror FROM outright: forcing the RAISED
    /// side as the source lifts the planted limb onto it — the opposite of what `Auto` reads off
    /// the pose, which is the whole point of the override.
    #[test]
    fn a_forced_stance_source_squares_the_other_side_whatever_the_pose_says() {
        // The LEFT leg is the raised one. Forcing LEFT as the source moves the PLANTED right limb.
        let mut m = mid_stride_model();
        let report = square_stance(&mut m, StanceSource::Left, &SkeletonRecipe::humanoid());
        assert_eq!(
            report
                .squared
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            ["thigh_r"],
            "the right limb is the one that moves when the LEFT is the source"
        );
        let raised = mid_stride_model();
        for (l, r) in [
            ("thigh_l", "thigh_r"),
            ("calf_l", "calf_r"),
            ("foot_l", "foot_r"),
        ] {
            assert!(
                world_of(&m, l).distance(world_of(&raised, l)) < 1e-6,
                "{l} is the source and stays exactly as posed"
            );
            let (a, b) = (world_of(&m, l), world_of(&m, r));
            assert!(
                b.distance(Vec3::new(-a.x, a.y, a.z)) < 1e-3,
                "{r} is now {l}'s reflection: {b} vs {a}"
            );
        }
        // Forcing RIGHT — the planted side here — agrees with what Auto reads off the pose.
        let (mut forced, mut auto) = (mid_stride_model(), mid_stride_model());
        square_stance(
            &mut forced,
            StanceSource::Right,
            &SkeletonRecipe::humanoid(),
        );
        square_stance(&mut auto, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(format!("{forced:?}"), format!("{auto:?}"));
    }

    /// The centreline of [`strided_body`]'s tail, from inside the rump to the floor.
    const STRIDED_TAIL: [[f32; 3]; 4] = [
        [0.0, 16.0, 52.0],
        [0.0, 24.0, 46.0],
        [0.0, 30.0, 28.0],
        [0.0, 30.0, 2.0],
    ];

    /// A STRIDED BODY WITH FLESH — the stride of [`strided_socket_model`] built as CLOSED MESHES the
    /// voxel flesh and the shape graph can read (the fixtures above are hand-weighted point sets
    /// with no flesh at all): a barrel on two legs, the LEFT one swung back with its foot `lift` cm
    /// up, and a TAIL hanging from the rump to the floor behind them. The skeleton is what the fit
    /// lays on such a body: `pelvis` in the barrel, each leg's chain down its own tube from a SOCKET
    /// where the tube leaves the barrel, the two sockets each other's reflection
    /// (`conform::pair_socket`) and both legs' bones the same length, so squaring is a pure re-pose.
    /// The tail carries NO bones — a module nothing matched (the Ram's `tail:`, F4A8D976), whose
    /// flesh the plain distance bind hands to the nearest bones, which are the raised foot's. Each
    /// foot's flesh reaches `soles` cm on straight down under its joint (`[left, right]`; `0` ends
    /// the flesh at the joint): where a fit reads a planted end's ground joint inside its flesh
    /// and a raised one's at its tip.
    fn strided_body(lift: f32, soles: [f32; 2]) -> RawModel {
        use crate::flesh::fixtures::{merge, subdivided_box, tube};
        let (lo, hi) = (Vec3::new(-16.0, -20.0, 40.0), Vec3::new(16.0, 20.0, 70.0));
        let mut barrel = subdivided_box(lo, hi, 16);
        // Outward normals: a box face's own axis, a tube's straight out from its centreline.
        for v in &mut barrel.vertices {
            let p = Vec3::from_array(v.p).to_array();
            let (l, h) = (lo.to_array(), hi.to_array());
            let n = [0, 1, 2].map(|a| {
                if (p[a] - l[a]).abs() < 1e-3 {
                    -1.0
                } else if (p[a] - h[a]).abs() < 1e-3 {
                    1.0
                } else {
                    0.0
                }
            });
            v.n = Vec3::from_array(n).normalize_or_zero().to_array();
        }
        let radial = |mut m: RawModel, line: &[Vec3]| {
            for v in &mut m.vertices {
                let p = Vec3::from_array(v.p);
                let q = line
                    .windows(2)
                    .map(|w| closest_point_segment(p, w[0], w[1]))
                    .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
                    .expect("a line");
                v.n = (p - q).normalize_or_zero().to_array();
            }
            m
        };
        // The PLANTED right leg straight down; the LEFT the same two 20 cm bones, the thigh swung
        // 30° back and the calf on back until the foot is `lift` up.
        let (hip_r, knee_r, foot_r) = (
            Vec3::new(-9.0, 0.0, 40.0),
            Vec3::new(-9.0, 0.0, 20.0),
            Vec3::new(-9.0, 0.0, 0.0),
        );
        let hip_l = Vec3::new(9.0, 0.0, 40.0);
        let (s, c) = 30f32.to_radians().sin_cos();
        let knee_l = hip_l + 20.0 * Vec3::new(0.0, s, -c);
        let drop = knee_l.z - lift;
        let foot_l = knee_l + Vec3::new(0.0, (400.0 - drop * drop).max(0.0).sqrt(), -drop);
        let leg = |hip: Vec3, knee: Vec3, foot: Vec3, sole: f32| {
            let mut line = vec![hip + Vec3::new(0.0, 0.0, 8.0), hip, knee, foot];
            let mut radii = vec![5.0, 5.0, 4.5, 4.0];
            if sole > 0.0 {
                line.push(foot - Vec3::new(0.0, 0.0, sole));
                radii.push(4.0);
            }
            radial(tube(&line, &radii), &line)
        };
        let tail_line = STRIDED_TAIL.map(Vec3::from_array);
        let tail = radial(tube(&tail_line, &[4.0, 3.5, 3.0, 2.5]), &tail_line);
        let mut m = merge(vec![
            barrel,
            leg(hip_r, knee_r, foot_r, soles[1]),
            leg(hip_l, knee_l, foot_l, soles[0]),
            tail,
        ]);
        let bones: [(&str, i32, Vec3); 8] = [
            ("pelvis", -1, Vec3::new(0.0, 0.0, 55.0)),
            ("spine_01", 0, Vec3::new(0.0, -12.0, 57.0)),
            ("thigh_l", 0, hip_l),
            ("calf_l", 2, knee_l),
            ("foot_l", 3, foot_l),
            ("thigh_r", 0, hip_r),
            ("calf_r", 5, knee_r),
            ("foot_r", 6, foot_r),
        ];
        m.bones = bones
            .iter()
            .map(|&(name, parent, world)| {
                let p = usize::try_from(parent)
                    .ok()
                    .map_or(Vec3::ZERO, |i| bones[i].2);
                RawBone {
                    name: name.to_string(),
                    parent,
                    translation: (world - p).to_array(),
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                    inverse_bind: Mat4::from_translation(world).inverse().to_cols_array(),
                }
            })
            .collect();
        m
    }

    /// The lowest point of a model's mesh — its floor.
    fn floor_of(m: &RawModel) -> f32 {
        m.vertices
            .iter()
            .map(|v| v.p[2])
            .fold(f32::INFINITY, f32::min)
    }

    /// How much of a vertex's weight sits on bones whose names `pick` admits.
    fn weight_on(m: &RawModel, v: &RawVertex, pick: impl Fn(&str) -> bool) -> f32 {
        (0..4)
            .filter(|&k| pick(&m.bones[v.joints[k] as usize].name))
            .map(|k| v.weights[k])
            .sum()
    }

    /// THE SEATING OF A STANDING LIMB ([`settle_chains`], from the 17 hoofed sources of
    /// 2026-10-02). A leg whose thigh reads in a hump's tube, a raised foreleg whose shoulder
    /// bones read in the trunk, and a tail laid down the leg it hangs against: each standing
    /// limb ends up with its whole chain and nothing else, junctions stay nobody's, and a limb
    /// that does not stand keeps its root with the trunk.
    #[test]
    fn a_standing_limbs_chain_seats_as_one_and_carries_no_other() {
        // 0 pelvis ─ 1 thigh ─ 2 calf ─ 3 foot ─ 4 hoof
        //          ─ 5 tail_01 ─ 6 tail_02 ─ 7 tail_03
        //          ─ 8 spine ─ 9 clavicle ─ 10 upperarm ─ 11 forearm ─ 12 forehoof
        //                    ─ 13 wing_root ─ 14 wing_mid ─ 15 wing_tip
        let parents: Vec<Option<usize>> = [
            None,
            Some(0),
            Some(1),
            Some(2),
            Some(3),
            Some(0),
            Some(5),
            Some(6),
            Some(0),
            Some(8),
            Some(9),
            Some(10),
            Some(11),
            Some(8),
            Some(13),
            Some(14),
        ]
        .to_vec();
        let n = parents.len();
        let chained: Vec<bool> = (0..n).map(|i| parents.contains(&Some(i))).collect();
        let deform = vec![true; n];
        // Each bone's OWN read: limb 0 the hind leg, 1 the raised foreleg, 2 a hump, 3 a wing.
        let read: Vec<Option<usize>> = [
            None,    // pelvis
            Some(2), // thigh — in the hump's tube
            Some(0),
            Some(0),
            None,    // hoof — too thin to read; a leaf
            None,    // tail_01
            Some(0), // tail_02 — laid down the leg it hangs against
            Some(0),
            None, // spine
            None, // clavicle — inside the body
            None, // upperarm — inside the body
            Some(1),
            Some(1),
            None, // wing_root — inside the body
            Some(3),
            Some(3),
        ]
        .to_vec();
        let to_tip = |l: usize, i: usize| (i as f32 - [4.0, 12.0, 1.0, 15.0][l]).abs();
        let settled = |stands: &dyn Fn(usize) -> bool| {
            let mut seat = read.clone();
            settle_chains(&mut seat, &parents, &chained, &deform, stands, &to_tip);
            seat
        };
        // The leg stands; the raised foreleg stands by its twin; the hump and the wing do not.
        let seat = settled(&|l| l < 2);
        assert_eq!(
            seat[1..5],
            [Some(0); 4],
            "the leg is one chain, thigh to hoof"
        );
        assert_eq!(
            seat[5..8],
            [None; 3],
            "the tail against the leg is the trunk's"
        );
        assert_eq!(
            seat[9..13],
            [Some(1); 4],
            "a raised leg carries its own shoulder"
        );
        assert_eq!(
            (seat[0], seat[8]),
            (None, None),
            "a junction is nobody's: the pelvis, the spine"
        );
        assert_eq!(
            seat[13..],
            [None, Some(3), Some(3)],
            "a limb that does not stand keeps its root with the trunk"
        );
        // With no twin on the ground the foreleg is an arm, and its shoulder the trunk's.
        let seat = settled(&|l| l == 0);
        assert_eq!(seat[9..13], [None, None, Some(1), Some(1)]);
    }

    /// AN APPENDAGE SEATS NO BONE. A barrel on four legs with a thin neck and head: the neck is a
    /// limb of its shape and the neck and head bones are seated in it. Give the head a pair of
    /// antlers longer than the neck, and that limb's lead runs on bare past its bones for more
    /// than half of its own length — it is more antler than neck — so nothing is seated in it:
    /// the neck and head bones are the trunk's, and the flesh round them is theirs by the plain
    /// reading (three horned sources had their skulls bound to the spine while the head bone,
    /// seated in one antler, carried that antler alone).
    #[test]
    fn a_limb_that_is_mostly_antler_seats_no_bone() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let seats = |antlers: bool| {
            let mut parts = vec![box_mesh(-14.0, 14.0, -40.0, 40.0, 60.0, 92.0)];
            for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                let (x, y) = (sx * 10.0_f32, sy * 32.0_f32);
                parts.push(tube(
                    &[Vec3::new(x, y, 0.0), Vec3::new(x, y, 66.0)],
                    &[4.0, 4.0],
                ));
            }
            parts.push(tube(
                &[
                    Vec3::new(0.0, -34.0, 82.0),
                    Vec3::new(0.0, -54.0, 86.0),
                    Vec3::new(0.0, -90.0, 86.0),
                ],
                &[5.0, 5.0, 5.0],
            ));
            if antlers {
                for sx in [1.0_f32, -1.0] {
                    parts.push(tube(
                        &[
                            Vec3::new(sx * 2.0, -78.0, 89.0),
                            Vec3::new(sx * 24.0, -70.0, 150.0),
                        ],
                        &[2.2, 1.8],
                    ));
                }
            }
            let mut model = merge(parts);
            let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
            let report = crate::conform::fit_baseline_to_mesh(&mut model, 110.0, &recipe)
                .expect("the fit runs");
            let body = report.body.expect("the fit hands back its read");
            let (heads, tails) = bone_segments(&model);
            let tubes = Tubes::read(&model, &heads, &tails, &deforming(&model), &body)
                .expect("the legs are seated");
            ["neck_02", "head", "thigh_l"].map(|n| {
                tubes.seat[model
                    .bones
                    .iter()
                    .position(|b| b.name == n)
                    .unwrap_or_else(|| panic!("no `{n}`"))]
            })
        };
        let [neck, head, thigh] = seats(false);
        assert!(
            neck.is_some() && neck == head,
            "a bare-headed neck is its bones' limb: {neck:?} {head:?}"
        );
        let [neck, head, antlered_thigh] = seats(true);
        assert_eq!(
            (neck, head),
            (None, None),
            "a limb that is mostly antler seats no bone"
        );
        assert!(
            thigh.is_some() && antlered_thigh.is_some(),
            "a leg is its bones' limb either way"
        );
    }

    /// A LIMB'S SKIN IS ITS TUBE'S FLESH (the 13 declined strides of F4A8D976). The plain distance
    /// bind hands the belly round a thigh's socket to the thigh — its head sits at the barrel's skin
    /// — and a bone-less tail's fall to the raised foot hanging beside it; squaring that stride then
    /// drags the tail through the floor, and the square is DECLINED. Bound to its tubes, the belly
    /// and the tail are the trunk's, the square goes through, nothing goes under the floor, and the
    /// squared leg's own flesh rides its chain RIGIDLY — a vertex on one bone keeps its distance to
    /// that bone's joint and to the next joint along the chain.
    #[test]
    fn a_stride_bound_to_its_tubes_squares_without_dragging_the_body_through_the_floor() {
        let body = strided_body(12.0, [0.0, 0.0]);
        let floor = floor_of(&body);
        let leg = |b: &str| side_of(b) == Some(true);
        // The belly forward of the raised leg's socket, past the junction band — three of the
        // leg's radii (5 cm) and more off it: trunk flesh.
        let belly = |v: &RawVertex| {
            (v.p[2] - 40.0).abs() < 1e-3
                && (v.p[0] - 9.0).abs() <= 4.0
                && (-18.0..=-15.0).contains(&v.p[1])
        };
        // ...and the belly at the CREASE, one radius off the socket: the junction band.
        let crease = |v: &RawVertex| {
            (v.p[2] - 40.0).abs() < 1e-3
                && (v.p[0] - 9.0).abs() <= 1.0
                && (v.p[1] + 5.0).abs() < 1.0
        };
        // The tail's fall behind the barrel.
        let tail = |v: &RawVertex| v.p[0].abs() <= 4.5 && v.p[1] > 21.0;
        assert!(body.vertices.iter().any(belly) && body.vertices.iter().any(tail));

        // THE PLAIN DISTANCE BIND — what every body bound with before.
        let mut plain = body.clone();
        bind(&mut plain, None);
        let most =
            |m: &RawModel, pick: &dyn Fn(&RawVertex) -> bool, bones: &dyn Fn(&str) -> bool| {
                m.vertices
                    .iter()
                    .filter(|v| pick(v))
                    .map(|v| weight_on(m, v, bones))
                    .fold(0.0_f32, f32::max)
            };
        let on_thigh = |b: &str| b == "thigh_l";
        assert!(
            most(&plain, &belly, &on_thigh) > 0.3,
            "the plain bind hands the belly to the thigh: {}",
            most(&plain, &belly, &on_thigh)
        );
        assert!(
            most(&plain, &tail, &leg) > 0.1,
            "the plain bind hands the tail's fall to the raised leg: {}",
            most(&plain, &tail, &leg)
        );
        let mut dragged = plain.clone();
        let r = square_stance(
            &mut dragged,
            StanceSource::Auto,
            &SkeletonRecipe::humanoid(),
        );
        assert!(
            r.squared.is_empty() && r.declined.iter().any(|(b, _)| b == "thigh_l"),
            "on the plain bind the tail's fall drags the square under the floor and it is declined: {r:?}"
        );

        // THE TUBE BIND.
        let mut m = body.clone();
        bake_skin(&mut m);
        for v in m.vertices.iter().filter(|v| belly(v) || tail(v)) {
            assert_eq!(
                weight_on(&m, v, leg),
                0.0,
                "{:?} is trunk flesh and carries no leg weight",
                v.p
            );
        }
        // At the crease the leg and the trunk SHARE the flesh — the junction band.
        let band = most(&m, &crease, &leg);
        assert!(
            (0.3..=0.7).contains(&band),
            "the crease blends leg into trunk: {band}"
        );
        // A radius and more past its socket, the leg's flesh is the leg's alone — wherever it
        // does not lie against the tail's (the raised foot hangs 3 cm off it, and there the two
        // share their seam).
        let off_the_tail = |v: &RawVertex| {
            let p = Vec3::from_array(v.p);
            STRIDED_TAIL
                .map(Vec3::from_array)
                .windows(2)
                .map(|w| p.distance(closest_point_segment(p, w[0], w[1])))
                .fold(f32::INFINITY, f32::min)
                > 8.0
        };
        for v in m
            .vertices
            .iter()
            .filter(|v| v.p[0] > 4.0 && v.p[2] < 34.0 && off_the_tail(v))
        {
            assert!(
                (weight_on(&m, v, leg) - 1.0).abs() < 1e-5,
                "{:?} is the leg's own flesh",
                v.p
            );
        }
        let before = m.clone();
        // The raise is its FOOT's: the lowest of the raised leg's own flesh (its cap tilts with the
        // swung calf, so it hangs below the 12 cm its joint is up), over the planted sole.
        let raised_sole = m
            .vertices
            .iter()
            .filter(|v| v.p[0] > 4.0 && v.p[2] < 34.0 && off_the_tail(v))
            .map(|v| v.p[2])
            .fold(f32::INFINITY, f32::min);
        assert!(
            raised_sole < 12.0 - 1.0,
            "the raised foot's sole hangs under its joint: {raised_sole}"
        );
        let r = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(r.squared.len(), 1, "the raised leg is squared: {r:?}");
        assert_eq!(r.squared[0].0, "thigh_l");
        assert!(
            (r.squared[0].1 - (raised_sole - floor)).abs() < 1e-3,
            "the lift is the raised foot's sole, {raised_sole} over the floor: {r:?}"
        );
        assert!(
            r.declined.is_empty(),
            "nothing is declined: {:?}",
            r.declined
        );
        let after = floor_of(&m);
        assert!(
            after >= floor - 1e-3,
            "no vertex goes under the floor: {floor} before, {after} after"
        );
        let (foot_l, foot_r) = (world_of(&m, "foot_l"), world_of(&m, "foot_r"));
        assert!(
            foot_l.z.abs() < 1e-3 && foot_r.z.abs() < 1e-3,
            "both feet stand on the floor: {foot_l} / {foot_r}"
        );
        // RIGID: the pair's bones are the same length, so squaring TURNS each bone of the leg and
        // stretches none — and every vertex the leg carries wholly moved by exactly the blend of
        // those turns, re-derived here from the joints alone (a bone's old segment turned onto its
        // new one; the ground joint, a leaf, turns with the bone above it), and by nothing else.
        let joint = |mm: &RawModel, b: &str| world_of(mm, b);
        for (a, b) in [("thigh_l", "calf_l"), ("calf_l", "foot_l")] {
            let (l0, l1) = (
                joint(&before, a).distance(joint(&before, b)),
                joint(&m, a).distance(joint(&m, b)),
            );
            assert!(
                (l0 - l1).abs() < 1e-3,
                "{a}→{b} keeps its length: {l0} vs {l1}"
            );
        }
        let motion = |bone: &str, p: Vec3| -> Vec3 {
            let (head, tail) = match bone {
                "thigh_l" => ("thigh_l", "calf_l"),
                _ => ("calf_l", "foot_l"),
            };
            let turn = Quat::from_rotation_arc(
                (joint(&before, tail) - joint(&before, head)).normalize(),
                (joint(&m, tail) - joint(&m, head)).normalize(),
            );
            joint(&m, bone) + turn * (p - joint(&before, bone))
        };
        let mut rigid = 0;
        for (a, b) in before.vertices.iter().zip(&m.vertices) {
            if (weight_on(&before, a, leg) - 1.0).abs() > 1e-5 {
                continue;
            }
            let p = Vec3::from_array(a.p);
            let want: Vec3 = (0..4)
                .filter(|&k| a.weights[k] > 0.0)
                .map(|k| a.weights[k] * motion(&before.bones[a.joints[k] as usize].name, p))
                .sum();
            assert!(
                want.distance(Vec3::from_array(b.p)) < 1e-3,
                "{:?} rode its chain rigidly: {want} expected, {:?} got",
                a.p,
                b.p
            );
            rigid += 1;
        }
        assert!(
            rigid > 300,
            "the rigid check reads real flesh: {rigid} vertices"
        );
        // Everything that is not the squared leg's is exactly where it was.
        for (a, b) in before.vertices.iter().zip(&m.vertices) {
            if weight_on(&before, a, leg) == 0.0 {
                assert_eq!(a.p, b.p, "{:?} is not the square's business", a.p);
            }
        }
    }

    /// A BODY WITH NO BONE IN ANY TUBE BINDS EXACTLY AS BEFORE — the barrel, legs and tail of
    /// [`strided_body`] under a trunk-only skeleton (a recipe whose limbs matched nothing): the tube
    /// bind has nothing to honour and is the plain distance bind, bit for bit.
    #[test]
    fn a_body_with_no_bone_in_any_tube_binds_exactly_as_before() {
        let mut body = strided_body(12.0, [0.0, 0.0]);
        body.bones.truncate(2); // `pelvis` and `spine_01`, both in the barrel
        let (mut plain, mut tubed) = (body.clone(), body);
        bind(&mut plain, None);
        bake_skin(&mut tubed);
        assert_eq!(
            format!("{:?}", tubed.vertices),
            format!("{:?}", plain.vertices)
        );
    }

    /// A SQUARED FOOT STANDS WHERE ITS TWIN STANDS. The planted foot's flesh reaches 4 cm under its
    /// joint and the raised one's ends at its own, so carried onto its twin's joints the squared
    /// foot would hang 4 cm over the ground its twin stands on (the Horse's forehoof, 2.2 cm): its
    /// SKIN is set down onto that ground, its JOINTS stay exactly its twin's reflection — so the
    /// squared body, squared again, is left bit for bit. A float within the noise floor is no
    /// float: the stride of
    /// [`a_stride_bound_to_its_tubes_squares_without_dragging_the_body_through_the_floor`] rides
    /// its chain rigidly.
    #[test]
    fn a_squared_foot_is_set_down_onto_the_ground_its_twin_stands_on() {
        const SOLE: f32 = 4.0;
        let mut m = strided_body(12.0, [0.0, SOLE]);
        bake_skin(&mut m);
        let floor = floor_of(&m);
        assert!(
            (floor + SOLE).abs() < 1e-3,
            "the planted sole is the floor: {floor}"
        );
        let lowest = |m: &RawModel, left: bool| {
            m.vertices
                .iter()
                .filter(|v| weight_on(m, v, |b| side_of(b) == Some(left)) >= WHOLLY)
                .map(|v| v.p[2])
                .fold(f32::INFINITY, f32::min)
        };
        assert!(
            lowest(&m, true) > 5.0,
            "the raised foot is up: {}",
            lowest(&m, true)
        );
        let r = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(r.squared.len(), 1, "the raised leg is squared: {r:?}");
        assert!(
            r.declined.is_empty(),
            "nothing is declined: {:?}",
            r.declined
        );
        let (ours, theirs) = (lowest(&m, true), lowest(&m, false));
        assert!(
            (ours - theirs).abs() < 1e-3,
            "the squared foot stands on its twin's ground: {ours} vs {theirs}"
        );
        assert!(floor_of(&m) >= floor - 1e-3, "nothing goes under the floor");
        for (l, r) in [
            ("thigh_l", "thigh_r"),
            ("calf_l", "calf_r"),
            ("foot_l", "foot_r"),
        ] {
            let (a, b) = (world_of(&m, l), world_of(&m, r));
            assert!(
                a.distance(Vec3::new(-b.x, b.y, b.z)) < 1e-3,
                "{l} is {r}'s reflection: {a} vs {b}"
            );
        }
        // SQUARED IS SQUARE: the joints read level, so a second pass has nothing to do.
        let once = m.clone();
        let again = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert!(
            again.squared.is_empty() && again.declined.is_empty(),
            "{again:?}"
        );
        assert_eq!(format!("{once:?}"), format!("{m:?}"));
    }

    /// A PLANTED FOOT IS ON THE FLOOR WHEREVER ITS JOINT SITS. The fit lays a ground joint at the
    /// far end of its graph limb, and a planted foot's can land well up inside it (the Wolf's
    /// forefeet: joints 3.3 / 6.6 cm, soles 0.0 / 0.0; the Elephant's 8.1 / 4.0). Read by their
    /// joints this pair is a 6 cm stride with one side on the floor, and the old normaliser squared
    /// it; read by their FEET ([`Foot`]) both stand on the floor, and nothing moves.
    #[test]
    fn a_planted_foot_is_on_the_floor_wherever_its_joint_sits() {
        const UP: f32 = 6.0;
        // The left leg swung back until its joint is 6 cm up, its flesh carried on down to the
        // floor under it; the right leg straight down, its joint on its sole.
        let mut m = strided_body(UP, [UP, 0.0]);
        bake_skin(&mut m);
        let floor = floor_of(&m);
        let (l, r) = (world_of(&m, "foot_l"), world_of(&m, "foot_r"));
        assert!(
            (l.z - floor - UP).abs() < 1e-3 && (r.z - floor).abs() < 1e-3,
            "by the joints this is a {UP} cm stride: {l} / {r}"
        );
        let body = Body::read(&m);
        let (lo, hi) = crate::conform::bbox(&m);
        for (side, joint) in [("left", l), ("right", r)] {
            let foot = Foot::of(&m, Some(&body), joint).floor(&m) - floor;
            assert!(
                foot.abs() <= noise_floor(lo, hi),
                "the {side} foot stands on the floor: {foot} cm"
            );
        }
        let before = m.clone();
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert!(
            report.squared.is_empty() && report.declined.is_empty(),
            "two feet on the floor are square already: {report:?}"
        );
        assert_eq!(format!("{m:?}"), format!("{before:?}"), "nothing moves");
    }

    /// A RAISED FOOT IS READ BY ITS FLESH, AND SQUARED. The BlackBear's forepaw: joints 2.2 / 6.7 cm
    /// but soles 0.4 / 4.1 — a real raise its joints hide, because the planted joint sits inside its
    /// paw, over the old 2 cm "on the floor" line, so the pair read as standing on nothing and the
    /// height fraction swallowed the raise. Here the planted joint stands 3 cm up inside its foot
    /// and the raised foot's own flesh ends over 4 cm above the floor at its joint, which is only
    /// 1 cm over its twin's: by the feet one stands and one is raised; squared, the raised foot is
    /// set down onto its twin's ground and both feet read on the floor.
    #[test]
    fn a_raised_foot_is_read_by_its_flesh_and_squared() {
        const UNDER: f32 = 3.0; // the planted foot's flesh under its joint
        let mut m = strided_body(1.0, [0.0, UNDER]);
        bake_skin(&mut m);
        let floor = floor_of(&m);
        let (l, r) = (world_of(&m, "foot_l"), world_of(&m, "foot_r"));
        assert!(
            (r.z - floor - UNDER).abs() < 1e-3 && (l.z - r.z - 1.0).abs() < 1e-3,
            "the joints: planted {UNDER} cm up, the raised one 1 cm over it: {l} / {r}"
        );
        // The raised leg's own lowest skin, straight off the fixture: where its tube ends.
        let raised_sole = m
            .vertices
            .iter()
            .filter(|v| v.p[0] > 4.5 && v.p[2] < 34.0)
            .map(|v| v.p[2])
            .fold(f32::INFINITY, f32::min)
            - floor;
        let body = Body::read(&m);
        let (lo, hi) = crate::conform::bbox(&m);
        let noise = noise_floor(lo, hi);
        let foot = |mm: &RawModel, b: &Body, j: Vec3| Foot::of(mm, Some(b), j).floor(mm) - floor;
        let (raised, planted) = (foot(&m, &body, l), foot(&m, &body, r));
        assert!(
            raised_sole >= 4.0 && (raised - raised_sole).abs() < 1e-3,
            "the raised foot reads its own flesh, {raised_sole} cm up: {raised}"
        );
        assert!(
            planted.abs() <= noise,
            "its twin's is on the floor: {planted}"
        );
        let report = square_stance(&mut m, StanceSource::Auto, &SkeletonRecipe::humanoid());
        assert_eq!(
            report
                .squared
                .iter()
                .map(|(b, _)| b.as_str())
                .collect::<Vec<_>>(),
            ["thigh_l"],
            "the raised side is squared: {report:?}"
        );
        assert!(
            (report.squared[0].1 - (raised - planted)).abs() < 1e-3,
            "by its foot's lift: {report:?}"
        );
        assert!(floor_of(&m) >= floor - 1e-3, "nothing goes under the floor");
        let (l, r) = (world_of(&m, "foot_l"), world_of(&m, "foot_r"));
        let stood = Body::read(&m);
        for (side, joint) in [("left", l), ("right", r)] {
            let f = foot(&m, &stood, joint);
            assert!(
                f.abs() <= noise,
                "the {side} foot now stands on the floor: {f}"
            );
        }
    }

    /// A WING IS NEVER A FOOT, however near the floor its digit droops. The ground joints are the
    /// recipe's STANDING pairs ([`standing_pairs`]) and nothing else: the Bat's harness judged its
    /// wing digit 11 cm "off the floor" as a failed foot (59D4921F). A bat composed on its own
    /// recipe, its left wing lowered until the digit all but touches the floor and its right held
    /// up — by its joints a textbook raised pair — has one standing pair, its legs, and the
    /// normaliser leaves both wings exactly as posed.
    #[test]
    fn a_wing_digit_on_the_floor_is_never_a_ground_joint() {
        use crate::baseline::{compose_with_modules, reference_recipe, Pattern, STATURE};
        let recipe = reference_recipe(Pattern::Bat);
        let mut m = RawModel {
            regions: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            bones: Vec::new(),
        };
        crate::conform::install_skeleton(&mut m, &recipe, STATURE).expect("the bat composes");
        let (bones, of) = compose_with_modules(&recipe, STATURE).expect("the bat composes");
        let modules: HashMap<String, String> = bones.into_iter().map(|b| b.name).zip(of).collect();
        let module = |name: &str| modules.get(name).cloned();
        // The left wing: the root of the arm module on the left, lowered whole until its deepest
        // joint is half a centimetre over the floor the feet stand on.
        let mut world = crate::conform::model_world_frames(&m);
        let pos = |w: &[Mat4]| w.iter().map(|g| g.w_axis.truncate()).collect::<Vec<Vec3>>();
        let kids = children(&m);
        let wing = (0..m.bones.len())
            .find(|&i| {
                side_of(&m.bones[i].name) == Some(true)
                    && module(&m.bones[i].name).is_some_and(|md| md.starts_with("arm:"))
                    && usize::try_from(m.bones[i].parent)
                        .is_ok_and(|p| side_of(&m.bones[p].name).is_none())
            })
            .expect("the bat has a left wing");
        let (limb, digit) = limb_and_ground(wing, &kids, &pos(&world));
        let floor = pos(&world)
            .iter()
            .map(|p| p.z)
            .fold(f32::INFINITY, f32::min);
        let drop = pos(&world)[digit].z - (floor + 0.5);
        for &i in &limb {
            world[i].w_axis.z -= drop;
        }
        crate::conform::write_world_frames(&mut m.bones, &world);
        let at = pos(&crate::conform::model_world_frames(&m));
        let twin = twin_name(&m.bones[digit].name)
            .and_then(|t| m.bones.iter().position(|b| b.name == t))
            .expect("the digit has a twin");
        assert!(
            at[digit].z - floor < 1.0 && at[twin].z - at[digit].z > 10.0,
            "one digit on the floor, its twin well up: {} / {}",
            at[digit].z,
            at[twin].z
        );
        let pairs = standing_pairs(&m, &recipe, &at);
        assert_eq!(
            pairs.iter().map(|p| p.module.as_str()).collect::<Vec<_>>(),
            ["leg:"],
            "the bat stands on its legs alone"
        );
        for p in &pairs {
            for (l, _) in &p.sides {
                assert!(
                    !l.contains(&digit) && !l.contains(&twin),
                    "no wing bone is a foot"
                );
            }
        }
        let before = m.clone();
        let report = square_stance(&mut m, StanceSource::Auto, &recipe);
        assert!(
            report.squared.is_empty() && report.declined.is_empty(),
            "the wings are no stance: {report:?}"
        );
        assert_eq!(
            format!("{m:?}"),
            format!("{before:?}"),
            "and are left as posed"
        );
    }

    /// ONLY A PAIR THE RECIPE AUTHORS STANDING IS A STANCE — the matcher's gravity reading
    /// (0E38BE60), on the normaliser's side: every leg, and a foreleg whose chain ends in a ground
    /// contact ([`ArmKind::Ungulate`]). A wing or a hanging arm is never squared, however near the
    /// floor it droops: squaring the Vulture's and the Bat's wings re-posed whole membranes and
    /// lost their pairs on the next read. One trunk carrying forelegs AND wings stands on the
    /// first and not the second; a bird and a bat stand on their legs alone; a biped never on its
    /// arms.
    #[test]
    fn only_the_pairs_a_recipe_authors_standing_are_a_stance() {
        use crate::baseline::{
            compose_with_modules, module_id, reference_recipe, Pattern, STATURE,
        };
        let mut winged = reference_recipe(Pattern::Quadruped);
        winged.trunk.arms = vec![ArmKind::Ungulate, ArmKind::Bat];
        for (recipe, arms_stand) in [
            (winged, vec![module_id("arm", "")]),
            (reference_recipe(Pattern::Bird), vec![]),
            (reference_recipe(Pattern::Bat), vec![]),
            (SkeletonRecipe::humanoid(), vec![]),
        ] {
            let standing = standing_bones(&recipe);
            let (bones, of) = compose_with_modules(&recipe, STATURE).expect("the recipe composes");
            assert!(
                of.iter().any(|m| m.starts_with("arm:")),
                "the recipe has arms to leave out"
            );
            for (b, m) in bones.iter().zip(&of) {
                let want = m.starts_with("leg:") || arms_stand.contains(m);
                assert_eq!(standing.contains_key(&b.name), want, "{} ({m})", b.name);
            }
        }
    }

    // ── THE REPOSE ON THE REAL BODIES (diagnostic, ignored) ──────────────────────────────────

    /// THE TUBE BIND ON THE REAL BODIES. `FLICKER_REPOSE_SWEEP=<dir>` walks every
    /// `<dir>/<Name>/<Name>.json(.gz)` a headless import baked and squares each body TWICE from the
    /// same skeleton and mesh: on the skin the import shipped, and after a fresh [`bake_skin`] —
    /// printing what each squared and declined, the mesh's floor after, which limbs the bind
    /// seated, and for a limb still declined the vertices that would sink with what drags them.
    /// One `REPOSE` line per body.
    #[test]
    #[ignore]
    fn diagnose_the_repose_on_the_real_bodies() {
        let Ok(dir) = std::env::var("FLICKER_REPOSE_SWEEP") else {
            eprintln!("skipping: set FLICKER_REPOSE_SWEEP=<dir of <Name>/<Name>.json(.gz)>");
            return;
        };
        let mut rigs: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&dir)
            .expect("the sweep folder reads")
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let plain = e.path().join(format!("{name}.json"));
                let gz = e.path().join(format!("{name}.json.gz"));
                plain
                    .is_file()
                    .then(|| (name.clone(), plain))
                    .or_else(|| gz.is_file().then_some((name, gz)))
            })
            .collect();
        rigs.sort();
        assert!(!rigs.is_empty(), "no <Name>/<Name>.json under {dir}");
        for (name, rig) in rigs {
            repose_one(&name, &rig);
        }
    }

    fn repose_one(name: &str, rig: &std::path::Path) {
        let model = load_rig_raw(rig).expect("the rig loads");
        // The rig's own recipe, as the shape sweep reads it (a creature import carries one).
        let recipe: SkeletonRecipe = crate::package::read_text(rig)
            .ok()
            .and_then(|t| serde_json::from_str::<RigFile>(&t).ok())
            .and_then(|f| f.skeleton_recipe)
            .unwrap_or_else(|| {
                crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped)
            });
        let low = |m: &RawModel| {
            m.vertices
                .iter()
                .map(|v| v.p[2])
                .fold(f32::INFINITY, f32::min)
        };
        let floor = low(&model);
        let said = |r: &StanceReport| {
            let s: Vec<String> = r
                .squared
                .iter()
                .map(|(b, d)| format!("{b}({d:.1})"))
                .collect();
            let d: Vec<String> = r
                .declined
                .iter()
                .map(|(b, d)| format!("{b}[{d:.1}]"))
                .collect();
            format!("sq {} dec {}", s.join(","), d.join(","))
        };
        // A — THE SKIN THE IMPORT SHIPPED.
        let mut shipped = model.clone();
        let ra = square_stance(&mut shipped, StanceSource::Auto, &recipe);
        // B — A FRESH TUBE BIND, same skeleton, same mesh.
        let mut bound = model.clone();
        let t0 = std::time::Instant::now();
        let body = Body::read(&bound);
        bind(&mut bound, Some(&body));
        let bind_ms = t0.elapsed().as_millis();
        let (heads, tails) = bone_segments(&bound);
        let tubes = Tubes::read(&bound, &heads, &tails, &deforming(&bound), &body);
        println!(
            "\n### {name}: {} verts, {} bones, floor {floor:.1}, bind {bind_ms} ms",
            model.vertices.len(),
            model.bones.len()
        );
        if let Some(t) = &tubes {
            println!("  GRAPH {}", t.graph.summary());
            print!("{}", t.graph.detail());
            for (l, b) in t.begin.iter().enumerate() {
                let Some(b) = b else { continue };
                let limb = &t.graph.limbs[l];
                let seated: Vec<&str> = (0..bound.bones.len())
                    .filter(|&i| t.seat[i] == Some(l))
                    .map(|i| bound.bones[i].name.as_str())
                    .collect();
                println!(
                    "  limb {l} side {:+.0} arc {:.0} begin {b:.1} r {:.1} tip {:.0?}: {}",
                    limb.side,
                    limb.arc,
                    limb.lead_r[limb.lead.len().min(limb.lead_r.len()) / 2],
                    limb.tip().to_array(),
                    seated.join(" ")
                );
            }
            let unseated: Vec<&str> = (0..bound.bones.len())
                .filter(|&i| t.seat[i].is_none() && side_of(&bound.bones[i].name).is_some())
                .map(|i| bound.bones[i].name.as_str())
                .collect();
            println!("  sided bones left with the trunk: {}", unseated.join(" "));
        } else {
            println!("  NO TUBES — binds as before");
        }
        let mut squared = bound.clone();
        let rb = square_stance(&mut squared, StanceSource::Auto, &recipe);
        let after = low(&squared);
        // The squared pairs' ground joints, both sides, off the floor the body started on.
        let world = crate::conform::model_world_frames(&squared);
        let pos: Vec<Vec3> = world.iter().map(|g| g.w_axis.truncate()).collect();
        let n = squared.bones.len();
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, b) in squared.bones.iter().enumerate() {
            if let Ok(p) = usize::try_from(b.parent) {
                if p < n {
                    kids[p].push(i);
                }
            }
        }
        let by_name = |s: &str| squared.bones.iter().position(|b| b.name == s);
        for (root, _) in &rb.squared {
            for r in [Some(root.clone()), twin_name(root)].into_iter().flatten() {
                if let Some(i) = by_name(&r) {
                    let g = limb_and_ground(i, &kids, &pos).1;
                    println!(
                        "  ground {:>12} {:6.1} cm",
                        squared.bones[g].name,
                        pos[g].z - floor
                    );
                }
            }
        }
        // EACH STANDING FOOT as the shipped rig stands (before any square here): its FOOT
        // ([`Foot`], the one reading), the lowest vertex whose strongest bone is in its chain
        // (the skin's own view of the same sole), and its ground joint — all off the floor.
        {
            let wpos: Vec<Vec3> = crate::conform::model_world_frames(&model)
                .iter()
                .map(|g| g.w_axis.truncate())
                .collect();
            let body = Body::read(&model);
            let mut line = String::from("  FEET");
            for pair in standing_pairs(&model, &recipe, &wpos) {
                for (limb, ground) in &pair.sides {
                    let sole = model
                        .vertices
                        .iter()
                        .filter(|v| {
                            let k = (0..4)
                                .max_by(|&a, &c| v.weights[a].total_cmp(&v.weights[c]))
                                .unwrap_or(0);
                            limb.contains(&(v.joints[k] as usize))
                        })
                        .map(|v| v.p[2])
                        .fold(f32::INFINITY, f32::min);
                    line.push_str(&format!(
                        " | {} foot {:.1} sole {:.1} joint {:.1} [{}]",
                        model.bones[limb[0]].name,
                        Foot::of(&model, Some(&body), wpos[*ground]).floor(&model) - floor,
                        sole - floor,
                        wpos[*ground].z - floor,
                        foot_detail(&model, &body, wpos[*ground])
                    ));
                }
            }
            println!("{line}");
        }
        // Whatever a SQUARE left under the floor, and what carries it.
        let mut under: Vec<(f32, usize)> = squared
            .vertices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.p[2] < floor - 0.5)
            .map(|(i, v)| (v.p[2], i))
            .collect();
        under.sort_by(|a, b| a.0.total_cmp(&b.0));
        for &(z, vi) in under.iter().take(6) {
            let v = &squared.vertices[vi];
            let skin: Vec<String> = (0..4)
                .filter(|&k| v.weights[k] > 0.0)
                .map(|k| {
                    format!(
                        "{}:{:.2}",
                        squared.bones[v.joints[k] as usize].name, v.weights[k]
                    )
                })
                .collect();
            let was = Vec3::from_array(bound.vertices[vi].p);
            println!(
                "  UNDER z {z:6.1} (was {:.1},{:.1},{:.1}) {}",
                was.x,
                was.y,
                was.z,
                skin.join(" ")
            );
        }
        // For every limb still declined: the vertices its square would sink, and why.
        for (root, sunk) in &rb.declined {
            let world = crate::conform::model_world_frames(&bound);
            let pos: Vec<Vec3> = world.iter().map(|g| g.w_axis.truncate()).collect();
            let by = |s: &str| bound.bones.iter().position(|b| b.name == s);
            let twin_of = |i: usize| twin_name(&bound.bones[i].name).and_then(|t| by(&t));
            let Some(i) = by(root) else { continue };
            let limb = limb_and_ground(i, &kids, &pos).0;
            let Some(target) = mirrored_targets(&limb, &pos, twin_of) else {
                continue;
            };
            let moves = limb_moves(&limb, &target, &kids, &pos);
            for &b in &limb {
                let Some(m) = moves[b] else { continue };
                println!(
                    "    bone {:>18} ({:6.1},{:6.1},{:6.1}) -> ({:6.1},{:6.1},{:6.1}) turn {:5.1}°",
                    bound.bones[b].name,
                    m.from.x,
                    m.from.y,
                    m.from.z,
                    m.to.x,
                    m.to.y,
                    m.to.z,
                    m.rot.angle_between(Quat::IDENTITY).to_degrees()
                );
            }
            let mut worst: Vec<(f32, usize)> = bound
                .vertices
                .iter()
                .enumerate()
                .filter_map(|(vi, v)| {
                    let p = Vec3::from_array(v.p);
                    let dz: f32 = (0..4)
                        .filter_map(|k| {
                            let m = moves.get(v.joints[k] as usize)?.as_ref()?;
                            Some(v.weights[k] * (m.at(p).z - p.z))
                        })
                        .sum();
                    (p.z + dz < floor - 0.5).then_some((p.z + dz, vi))
                })
                .collect();
            worst.sort_by(|a, b| a.0.total_cmp(&b.0));
            println!(
                "  DECLINED {root} sinks {sunk:.1} cm — {} vertices under the floor",
                worst.len()
            );
            for &(z, vi) in worst.iter().step_by((worst.len() / 12).max(1)).take(12) {
                let v = &bound.vertices[vi];
                let p = Vec3::from_array(v.p);
                let skin: Vec<String> = (0..4)
                    .filter(|&k| v.weights[k] > 0.0)
                    .map(|k| {
                        format!(
                            "{}:{:.2}",
                            bound.bones[v.joints[k] as usize].name, v.weights[k]
                        )
                    })
                    .collect();
                let member = tubes
                    .as_ref()
                    .and_then(|t| t.graph.limb_membership(p, &t.begin))
                    .map_or("trunk".to_string(), |(l, s)| format!("limb {l} {s:.2}"));
                // What the membership weighed: the nearest core ball, and each seated limb's own.
                let why = tubes.as_ref().map_or(String::new(), |t| {
                    let power = |c: &Vec3, r: &f32| p.distance(*c) - r;
                    let core = t
                        .graph
                        .cores
                        .iter()
                        .flat_map(|k| k.path.iter().zip(&k.radii))
                        .map(|(c, r)| power(c, r))
                        .fold(f32::INFINITY, f32::min);
                    let mut out = format!("core {core:.1}");
                    for (l, b) in t.begin.iter().enumerate() {
                        let Some(b) = b else { continue };
                        let limb = &t.graph.limbs[l];
                        let (mut own, mut pre, mut arc) = (f32::INFINITY, f32::INFINITY, 0.0);
                        for (k, (c, r)) in limb.lead.iter().zip(&limb.lead_r).enumerate() {
                            if k > 0 {
                                arc += limb.lead[k - 1].distance(*c);
                            }
                            if arc >= *b {
                                own = own.min(power(c, r));
                            } else {
                                pre = pre.min(power(c, r));
                            }
                        }
                        if own < 60.0 || pre < 60.0 {
                            out.push_str(&format!(" | L{l} own {own:.1} pre {pre:.1}"));
                        }
                    }
                    out
                });
                println!(
                    "    at ({:6.1},{:6.1},{:6.1}) -> z {z:6.1}  {member:<12} {}  [{why}]",
                    p.x,
                    p.y,
                    p.z,
                    skin.join(" ")
                );
            }
        }
        println!(
            "REPOSE\t{name}\tSHIPPED {}\tTUBE {}\tfloor {floor:.1} -> {after:.1}\tbind {bind_ms} ms\tseated {}",
            said(&ra),
            said(&rb),
            tubes.as_ref().map_or(0, |t| t.begin.iter().flatten().count())
        );
    }

    /// What [`Foot::of`] read for the ground joint at `joint`, step by step: the tube that holds it
    /// and its share, how far off that tube's flesh the joint is against the radius it is read
    /// within, and how much skin the foot found.
    fn foot_detail(model: &RawModel, body: &Body, joint: Vec3) -> String {
        let Some(graph) = body.graph.as_ref() else {
            return "no graph".into();
        };
        let own: Vec<Option<f32>> = graph
            .limbs
            .iter()
            .map(|l| Some(clear_of_core(graph, l)))
            .collect();
        let Some((l, share)) = graph.limb_membership(joint, &own) else {
            return "no tube".into();
        };
        let limb = &graph.limbs[l];
        let from = own[l].unwrap_or(0.0);
        let mut arc = 0.0;
        let mut near: Option<(f32, f32)> = None;
        let mut radii: Vec<f32> = Vec::new();
        for (k, (c, r)) in limb.lead.iter().zip(&limb.lead_r).enumerate() {
            if k > 0 {
                arc += limb.lead[k - 1].distance(*c);
            }
            if arc >= from {
                radii.push(*r);
                let off = joint.distance(*c) - r;
                if near.is_none_or(|(o, _)| off < o) {
                    near = Some((off, *r));
                }
            }
        }
        radii.sort_by(f32::total_cmp);
        let tube_r = radii.get(radii.len() / 2).copied().unwrap_or(f32::NAN);
        let foot = Foot::of(model, Some(body), joint);
        let low = foot
            .skin
            .iter()
            .map(|&i| Vec3::from_array(model.vertices[i].p))
            .min_by(|a, b| a.z.total_cmp(&b.z))
            .map_or(String::new(), |p| {
                let d = p - joint;
                format!(
                    " lowest at d({:.1},{:.1},{:.1}) h {:.1}",
                    d.x,
                    d.y,
                    d.z,
                    d.truncate().length()
                )
            });
        format!(
            "limb {l} share {share:.2} off-lead {:.1} off-flesh {:.1} tip-r {:.1} tube-r {tube_r:.1} \
             skin {}{low}",
            near.map_or(f32::NAN, |n| n.0),
            body.flesh.distance_outside(joint),
            near.map_or(f32::NAN, |n| n.1),
            foot.skin.len()
        )
    }

    /// THE TUBE BIND UNDER ANIMATION (diagnostic, ignored). `FLICKER_TUBE_STRAY=<rig .json(.gz)>`
    /// re-binds that rig two ways on the same skeleton — the plain distance bind and the tube bind —
    /// and CPU-skins each through the Katanami set exactly as the promoted-golem gate does, counting
    /// the triangles stretched past 2.5x their rest edge (and 8 cm) per clip. The variants are
    /// written beside the rig under `stray/`.
    #[test]
    #[ignore]
    fn diagnose_the_tube_bind_under_the_katanami_set() {
        let Ok(rig) = std::env::var("FLICKER_TUBE_STRAY") else {
            eprintln!("skipping: set FLICKER_TUBE_STRAY=<rig>");
            return;
        };
        let rig = std::path::PathBuf::from(rig);
        let clips = crate::roots::roots()
            .package()
            .join("retarget/clips/katanami");
        let name = rig
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| {
                n.trim_end_matches(".gz")
                    .trim_end_matches(".json")
                    .to_string()
            })
            .expect("a rig file name");
        let raw = load_rig_raw(&rig).expect("the rig loads");
        use flicker_skeletal::{pose, skin};
        for (tag, tubes) in [("plain", false), ("tube", true)] {
            let mut m = raw.clone();
            if tubes {
                bake_skin(&mut m);
            } else {
                bind(&mut m, None);
            }
            let dir = rig
                .parent()
                .expect("a folder")
                .join("stray")
                .join(tag)
                .join(&name);
            std::fs::create_dir_all(&dir).expect("the variant folder");
            write_rig_file(&bake_rig(&m, &name), &dir.join(format!("{name}.json")))
                .expect("the variant writes");
            let model = flicker_skeletal::format::load_dirs(&[&dir, &clips])
                .expect("the variant + the katanami library load");
            let mesh = &model.mesh;
            let tris: Vec<[usize; 3]> = mesh
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
                .collect();
            let longest = |at: &dyn Fn(usize) -> Vec3, t: &[usize; 3]| -> f32 {
                let (a, b, c) = (at(t[0]), at(t[1]), at(t[2]));
                a.distance(b).max(b.distance(c)).max(c.distance(a))
            };
            let rest: Vec<f32> = tris
                .iter()
                .map(|t| longest(&|i| Vec3::from(mesh.vertices[i].p), t))
                .collect();
            let mut line = format!("STRAY\t{name}\t{tag}");
            for (clip_name, tick) in [
                ("Idle_nonWeapon", 100u32),
                ("Walk_nonWeapon", 20),
                ("Run_nonWeapon", 20),
                ("Jump_Start", 15),
                ("Attack_1", 25),
            ] {
                let Some(clip) = model.clips.iter().find(|c| c.name == clip_name) else {
                    line.push_str(&format!("\t{clip_name} -"));
                    continue;
                };
                let locals = pose::sample_local_poses(&model.bones, clip, tick, model.retarget);
                let globals = pose::global_transforms(&model.bones, &locals);
                let palette = skin::palette(&model.bones, &globals);
                let posed = skin::skin(mesh, &palette);
                let stray = tris
                    .iter()
                    .zip(&rest)
                    .filter(|(t, &r)| {
                        let l = longest(&|i| Vec3::from(posed[i].position), t);
                        l > 8.0 && l > 2.5 * r.max(0.05)
                    })
                    .count();
                line.push_str(&format!("\t{clip_name} {stray}"));
            }
            println!("{line}\tof {}", tris.len());
        }
    }

    // ── FACE FORWARD — the head-turn un-pose (164AE2F3, the birds' 45°) ──────────────────────

    /// A synthetic head on a neck: pelvis → spine_01 → neck_01 → neck_02 → head → (jaw, eyes),
    /// the whole neck chain yawed `deg` about Z at `neck_01` — positive turning the face to the
    /// BODY'S LEFT, the pose the generated birds ship in. `eyes` drops the eye pair, so the jaw
    /// fallback is exercised on the same body. One vert wholly on the head, one on the pelvis.
    fn head_model(deg: f32, eyes: bool) -> RawModel {
        let pivot = Vec3::new(0.0, 0.0, 140.0);
        let turn = Quat::from_rotation_z(deg.to_radians());
        let yawed = |p: Vec3| pivot + turn * (p - pivot);
        let mut w: Vec<(&str, i32, Vec3)> = vec![
            ("root", -1, Vec3::new(0.0, 0.0, 0.0)),
            ("pelvis", 0, Vec3::new(0.0, 0.0, 95.0)),
            ("spine_01", 1, Vec3::new(0.0, 0.0, 110.0)),
            ("neck_01", 2, pivot),
            ("neck_02", 3, yawed(Vec3::new(0.0, 0.0, 148.0))),
            ("head", 4, yawed(Vec3::new(0.0, 0.0, 155.0))),
            ("jaw", 5, yawed(Vec3::new(0.0, -8.0, 152.0))),
        ];
        if eyes {
            w.push(("eye_l", 5, yawed(Vec3::new(3.0, -9.0, 160.0))));
            w.push(("eye_r", 5, yawed(Vec3::new(-3.0, -9.0, 160.0))));
        }
        let bones = w
            .iter()
            .map(|&(name, parent, world)| {
                let p = usize::try_from(parent).ok().map_or(Vec3::ZERO, |i| w[i].2);
                RawBone {
                    name: name.to_string(),
                    parent,
                    translation: (world - p).to_array(),
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                    inverse_bind: Mat4::from_translation(world).inverse().to_cols_array(),
                }
            })
            .collect();
        let v = |p: Vec3, j: u32| RawVertex {
            p: p.to_array(),
            n: [0.0, -1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [j, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        RawModel {
            regions: Vec::new(),
            vertices: vec![
                v(yawed(Vec3::new(0.0, -6.0, 158.0)), 5), // 0 — the face, wholly on the head
                v(Vec3::new(0.0, 0.0, 95.0), 1),          // 1 — the pelvis, none of its business
            ],
            indices: vec![0, 1, 0],
            bones,
        }
    }

    /// FACE FORWARD (164AE2F3): a head bound 45° off the body is un-turned at the neck until it
    /// looks down the canon forward (−Y), the skin carried into it by the same per-bone rigid
    /// blend the stance normaliser uses, and the body re-bound looking straight ahead.
    #[test]
    fn face_forward_unturns_a_head_yawed_to_the_bodys_left() {
        let before = head_model(45.0, true);
        let mut m = head_model(45.0, true);
        let report = face_forward(&mut m);
        assert!(
            (report.yaw_deg - 45.0).abs() < 1e-3 && report.turned,
            "45° to the body's left, got {report:?}"
        );
        // The face looks STRAIGHT AHEAD: the eyes' midpoint is on the plumb line, in front of the
        // head joint (−Y is forward).
        let mid = (world_of(&m, "eye_l") + world_of(&m, "eye_r")) * 0.5;
        let head = world_of(&m, "head");
        assert!(mid.x.abs() < 1e-3, "the eyes sit on the midline, got {mid}");
        assert!(
            mid.y < head.y,
            "and ahead of the head joint: {mid} vs {head}"
        );
        // The SKIN followed: a vert wholly on the head lands at its rotated position.
        let pivot = world_of(&before, "neck_01");
        let turn = Quat::from_rotation_z(-45f32.to_radians());
        let want = pivot + turn * (Vec3::from_array(before.vertices[0].p) - pivot);
        let got = Vec3::from_array(m.vertices[0].p);
        assert!(
            got.distance(want) < 1e-3,
            "the face vert turns with the head: {got} vs {want}"
        );
        assert!(
            got.distance(Vec3::from_array(before.vertices[0].p)) > 1.0,
            "and it really moved"
        );
        // The PELVIS is untouched — neither its vert nor its joint.
        assert_eq!(m.vertices[1].p, before.vertices[1].p);
        for name in ["root", "pelvis", "spine_01", "neck_01"] {
            assert!(
                world_of(&m, name).distance(world_of(&before, name)) < 1e-6,
                "{name} stays where it was"
            );
        }
        // The chain turned RIGIDLY: no neck bone changed length.
        for (a, b) in [("neck_01", "neck_02"), ("neck_02", "head"), ("head", "jaw")] {
            let (was, now) = (
                world_of(&before, a).distance(world_of(&before, b)),
                world_of(&m, a).distance(world_of(&m, b)),
            );
            assert!((was - now).abs() < 1e-3, "{a}→{b}: {was} became {now}");
        }
        // RE-BOUND facing forward: rest-pose skinning is the identity again.
        let rest = crate::conform::model_world_frames(&m);
        for (b, g) in m.bones.iter().zip(&rest) {
            let palette = *g * Mat4::from_cols_array(&b.inverse_bind);
            assert!(
                (palette - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-4),
                "{}: the bind faces forward",
                b.name
            );
        }
    }

    /// A head with NO EYES still has a jaw hung in front of it, and that is enough to measure the
    /// turn from — the fallback is a real channel, not a comment (8634C200).
    #[test]
    fn a_head_with_no_eyes_is_measured_off_its_jaw() {
        let mut m = head_model(45.0, false);
        let report = face_forward(&mut m);
        assert!(
            (report.yaw_deg - 45.0).abs() < 1e-3 && report.turned,
            "the jaw reads the same turn, got {report:?}"
        );
        let (jaw, head) = (world_of(&m, "jaw"), world_of(&m, "head"));
        assert!(
            jaw.x.abs() < 1e-3 && jaw.y < head.y,
            "the jaw hangs straight ahead again, got {jaw}"
        );
    }

    /// THE BODY BEHIND THE NECK STAYS PUT ([`Ahead`]). A bind hands a little of a neck bone's
    /// weight to flesh a long way down the body — on the Wolf a tenth of `neck_02` rode its back a
    /// metre behind the neck — and the un-turn swings that flesh round the neck's root on a lever as
    /// long as the distance. A vert far behind the root, holding a tenth of the head, does not move;
    /// the face vert still turns all the way.
    #[test]
    fn face_forward_leaves_the_body_behind_the_neck_where_it_is() {
        let mut m = head_model(45.0, true);
        let head = m.bones.iter().position(|b| b.name == "head").unwrap() as u32;
        let back = Vec3::new(0.0, 90.0, 120.0);
        m.vertices.push(RawVertex {
            p: back.to_array(),
            n: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            joints: [2, head, 0, 0],
            weights: [0.9, 0.1, 0.0, 0.0],
        });
        let before = m.clone();
        assert!(face_forward(&mut m).turned, "the head is turned");
        assert_eq!(
            m.vertices[2].p,
            back.to_array(),
            "the back holding a tenth of the head stays where it was"
        );
        assert!(
            Vec3::from_array(m.vertices[0].p).distance(Vec3::from_array(before.vertices[0].p))
                > 1.0,
            "while the face turns"
        );
    }

    /// A head that already faces forward is left BIT FOR BIT alone — and says so.
    #[test]
    fn a_head_that_already_faces_forward_is_left_exactly_as_it_was() {
        let straight = head_model(0.0, true);
        let mut m = head_model(0.0, true);
        let report = face_forward(&mut m);
        assert!(
            report.yaw_deg.abs() < 1e-3 && !report.turned,
            "nothing to turn, got {report:?}"
        );
        assert_eq!(format!("{m:?}"), format!("{straight:?}"));
        // And a wobble inside the tolerance is a wobble, not a turn.
        let mut small = head_model(3.0, true);
        assert!(!face_forward(&mut small).turned, "3° is not a head turn");
    }
}
