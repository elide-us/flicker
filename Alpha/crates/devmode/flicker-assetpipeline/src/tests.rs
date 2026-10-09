//! The services' tests — the pipeline exercised against the real content tree (skipping
//! when it is absent, like `flicker-content`'s own real-data tests) and headlessly against
//! scratch roots. No UI: the scene's own tests live with the scene.

use std::path::{Path, PathBuf};

use flicker::render::{Mat4, Vec3};
use flicker::ui::strings;
use flicker_content::baseline::markers_for;
use flicker_content::{
    default_reference, AssetClass, Fit, Matched, PropKind, RawModel, RawVertex, ShapeMatch,
};
use flicker_skeletal::format::{LegKind, Pattern, RegionTag, TailKind};
use flicker_skeletal::pose::{global_transforms, sample_local_poses};

use crate::meshes::{BasePreview, BASE_MESH_BUDGET};
use crate::services::{
    class_label, model_bounds, rest_globals, side_of, BoneOffset, Document, MapState, Parsed,
    RegionEdit, Side, ATTACH_POINTS, CONFORMED_BONES, REFERENCE_BONES, SOCKETS, WF_ANIMATION,
    WF_CHARACTER, WF_CREATURE, WF_PROP,
};

/// The real source folder the whole pipeline is developed against. Every test that needs a
/// genuine skeleton goes through this, and SKIPS when the content tree is absent — the same
/// guard `flicker-content`'s own real-data tests use.
fn real_source() -> Option<PathBuf> {
    let dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/source/PrismHumanBaseA"
    ));
    dir.exists().then_some(dir)
}

/// Load the shipped stringtable (en-us) into the process-wide table, so tests asserting
/// resolved copy read FINAL text. Safe across parallel test threads — every caller loads the
/// same content, so the shared table never changes under an assertion.
pub(crate) fn load_shipped_strings() {
    let strings = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/data/stringtable.json"
    ))
    .expect("stringtable reads");
    flicker::ui::strings::load_str(&strings, "en-us");
}

/// A document with the real CHARACTER asset loaded, parsed and conformed — exactly where
/// `open` lands it. `open` dispatches the character workflow and runs ingest → parse →
/// conform inline (exactly as clicking the Import Character card does), so the derived
/// state exists just as the scene would find it.
fn at_conform() -> Option<Document> {
    let dir = real_source()?;
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Skin);
    doc.open(dir);
    assert!(doc.source.is_some(), "the real source folder scanned");
    assert_eq!(
        doc.workflow, WF_CHARACTER,
        "a character dispatches the character workflow"
    );
    Some(doc)
}

/// A document with the real asset loaded and PARSED but NOT conformed. Callers re-route the
/// class (to a prop) or drive the picker from here, so the inline conform `open` runs is
/// dropped to leave a clean parse.
fn parsed() -> Option<Document> {
    let mut doc = at_conform()?;
    doc.source.as_mut().unwrap().rig = None;
    Some(doc)
}

/// The restored `Collision` overlay must have geometry to draw: a real character's auto-fit
/// yields per-bone capsules (the "boxes") AND at least one leaf-bone sphere (the "joint balls"),
/// every one indexing a real bone so the overlay can place it. Skips without the content tree.
#[test]
fn collision_overlay_has_capsules_and_joint_balls() {
    use flicker_mechanics::collision::Shape;
    let Some(doc) = parsed() else { return };
    let parsed = doc.source.as_ref().unwrap().parsed.as_ref().unwrap();
    assert!(
        !parsed.collision.is_empty(),
        "auto-fit produced collision volumes for the character"
    );
    let capsules = parsed
        .collision
        .iter()
        .filter(|v| matches!(v.shape, Shape::Capsule { .. }))
        .count();
    let spheres = parsed
        .collision
        .iter()
        .filter(|v| matches!(v.shape, Shape::Sphere { .. }))
        .count();
    assert!(
        capsules > 0,
        "bones with children fit capsules (the collision boxes)"
    );
    assert!(
        spheres > 0,
        "leaf bones (fingertips/toes/head end) fit spheres (the joint balls)"
    );
    assert!(
        parsed
            .collision
            .iter()
            .all(|v| v.bone < parsed.globals.len()),
        "every volume indexes a real bone, so `globals[v.bone]` places it"
    );
}

/// The canonical bone count is a canon constant, so it is asserted against the reference rig
/// itself rather than trusted — a change to the reference fails HERE, not silently in a
/// requirement that always reads red.
#[test]
fn reference_rig_still_has_the_canonical_bone_count() {
    let path = default_reference();
    if !flicker_content::package::file_exists(&path) {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let raw = flicker_content::package::read_text(&path).expect("read the reference rig");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("parse the reference rig");
    let bones = json["skeleton"]["bones"]
        .as_array()
        .expect("skeleton.bones")
        .len();
    assert_eq!(
        bones, REFERENCE_BONES,
        "the reference rig moved to {bones} bones — update REFERENCE_BONES and sweep the canon"
    );
}

/// CONFORM against the real source: every bone lands in exactly one provenance bucket, the
/// buckets sum to the whole skeleton, and the inferred set is the one the reports name.
/// This is the stage's contract — the bone map's colours have no second source. The bone
/// rows the scene publishes are that map, one per bone, each tag a shipped `$token`.
#[test]
fn conform_of_the_real_source_classifies_every_bone() {
    let Some(doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    let src = doc.source.as_ref().unwrap();
    let rig = src.rig.as_ref().expect("conform produced a rig");
    let parsed = src.parsed.as_ref().unwrap();

    // 65, not 66: `root` is synthesized by the bake, not by conform.
    assert_eq!(
        parsed.bones(),
        CONFORMED_BONES,
        "conform reaches the canonical bone count"
    );
    assert_eq!(
        rig.map.len(),
        parsed.bones(),
        "one provenance per bone, no gaps"
    );
    let (ok, review, auto) = {
        let n = |s: MapState| rig.map.iter().filter(|m| **m == s).count();
        (n(MapState::Ok), n(MapState::Review), n(MapState::Auto))
    };
    assert_eq!(
        ok + review + auto,
        parsed.bones(),
        "the buckets partition the skeleton"
    );
    assert_eq!(
        auto,
        rig.out.infer.added.len(),
        "auto is exactly what infer added — not a recount"
    );
    assert!(
        review > 0,
        "the hip/shoulder/ankle derives flagged joints for review"
    );
    assert!(ok > 0, "source bones survived the rename");

    // The reports are the ONE source: a bone infer added must not also read as review.
    for (i, b) in parsed.model.bones.iter().enumerate() {
        if rig.out.infer.added.iter().any(|a| a == &b.name) {
            assert_eq!(rig.map[i], MapState::Auto, "{} is inferred", b.name);
        }
    }

    // The published shape: one row per bone in skeleton order, its tag a token the shipped
    // table resolves — a tag that fails to nothing would show as a raw `$ap_tag_*`.
    load_shipped_strings();
    let rows = doc.bone_rows();
    assert_eq!(rows.len(), parsed.bones(), "one row per bone");
    for ((name, state), b) in rows.iter().zip(&parsed.model.bones) {
        assert_eq!(name, &b.name, "rows ride skeleton order");
        let tag = strings::resolve(state.tag());
        assert!(!tag.starts_with('$'), "{} resolves, got {tag}", state.tag());
    }
    assert_eq!(doc.bone_count(), Some(parsed.bones()));
    assert_eq!(doc.tri_count(), Some(parsed.tris));
    assert_eq!(doc.vert_count(), Some(parsed.verts));
    assert_eq!(doc.bone_sel(), Some(0), "the map opens on its first row");
    assert!(
        doc.asset_name().is_some_and(|n| n == "PrismHumanBaseA"),
        "the asset bakes under its folder's name"
    );
    assert!(doc.file_name().is_some_and(|f| !f.is_empty()));
}

/// An authored offset moves the derived skeleton — and a zero offset reproduces the conform
/// exactly, which is what makes "Reset bone" a real undo rather than an approximation. Driven
/// through the accessors the scene's bone list + offset sliders use.
#[test]
fn authored_offsets_move_the_skeleton_and_reset_restores_it() {
    let Some(mut doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    // Pick a bone with children so the offset has to propagate down the chain.
    assert!(
        doc.select_bone_named("spine_01"),
        "the conformed rig has spine_01"
    );
    let sel = doc.bone_sel().expect("a bone is selected");
    assert_eq!(
        doc.selected_offset(),
        Some(BoneOffset::default()),
        "nothing authored yet"
    );
    let globals = |doc: &Document| {
        doc.source
            .as_ref()
            .unwrap()
            .parsed
            .as_ref()
            .unwrap()
            .globals
            .clone()
    };
    let before = globals(&doc);
    let pose_gen = doc.pose_gen;

    let offset = BoneOffset {
        t: [0.0, 0.0, 7.0],
        roll: 0.0,
        ..Default::default()
    };
    doc.set_selected_offset(offset);
    assert_eq!(doc.selected_offset(), Some(offset));
    assert_ne!(doc.pose_gen, pose_gen, "the live skin re-uploads");
    let after = globals(&doc);

    assert_ne!(
        before[sel].w_axis, after[sel].w_axis,
        "the edited bone moved"
    );
    let head = doc
        .source
        .as_ref()
        .unwrap()
        .parsed
        .as_ref()
        .unwrap()
        .bone_index("head")
        .unwrap();
    assert_ne!(
        before[head].w_axis, after[head].w_axis,
        "the offset propagated to children"
    );

    // Re-reporting the same value (controls report every frame) changes nothing.
    let pose_gen = doc.pose_gen;
    doc.set_selected_offset(offset);
    assert_eq!(doc.pose_gen, pose_gen, "a same-value report is not an edit");

    // Reset → identical frames, bit for bit.
    doc.set_selected_offset(BoneOffset::default());
    assert_eq!(
        globals(&doc),
        before,
        "zeroing the offset restores the conform result exactly"
    );
    assert!(
        !doc.select_bone_named("not_a_bone"),
        "an unknown name selects nothing"
    );
}

/// ATTACH: a point sits at its parent bone's conformed frame plus the authored offset, and
/// all six resolve once the rig carries canonical names. Driven through the accessors the
/// scene's attach list + offset sliders use.
#[test]
fn attach_points_track_their_parent_bone_and_offset() {
    let Some(mut doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    let rows = doc.attach_rows();
    assert_eq!(rows.len(), 6, "the design's six points");
    for ((id, label), (pid, plabel, _)) in rows.iter().zip(ATTACH_POINTS) {
        assert_eq!(id, pid, "rows ride rail order");
        assert_eq!(label, plabel, "each row carries its label token");
    }
    let n = rows.len();
    assert!(
        (0..n).all(|i| doc.attach_resolved(i)),
        "every parent bone exists in the conformed rig: {:?}",
        (0..n).map(|i| doc.attach_resolved(i)).collect::<Vec<_>>()
    );

    // Selecting a point then dragging its X slider moves exactly that point.
    assert_eq!(doc.attach_sel(), Some(0), "the first point opens selected");
    assert!(doc.select_attach("holster_r"));
    assert_eq!(doc.attach_sel(), Some(2));
    assert_eq!(doc.attach_offset(), Some([0.0; 3]));
    let before = doc.attach_world(2).expect("resolves");
    let other = doc.attach_world(3).expect("resolves");
    doc.set_attach_offset([5.0, 0.0, 0.0]);
    assert_eq!(doc.attach_offset(), Some([5.0, 0.0, 0.0]));
    let after = doc.attach_world(2).expect("still resolves");
    assert!(
        (after.x - before.x - 5.0).abs() < 1e-4,
        "{before} → {after}"
    );
    assert_eq!(doc.attach_world(3).unwrap(), other, "others unmoved");
    assert!(
        !doc.select_attach("nowhere"),
        "an unknown id selects nothing"
    );
}

/// REVIEW: every requirement is computed from real state. With the real asset conformed they
/// all pass; with nothing loaded there is nothing to claim.
#[test]
fn review_requirements_read_the_real_state() {
    let empty = Document::new();
    assert!(empty.requirements().is_empty(), "no asset → no claims");

    let Some(mut doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    let reqs = doc.requirements();
    assert_eq!(
        reqs.len(),
        4,
        "the character set: skeleton · mapping · attach parents · textures"
    );
    for (ok, text) in &reqs {
        assert!(ok, "requirement failed on the reference asset: {text}");
    }

    // A requirement is a real gate: break one and Commit must go dark.
    doc.source.as_mut().unwrap().textures = 0;
    assert!(
        !doc.requirements().iter().all(|(ok, _)| *ok),
        "a failed check blocks commit"
    );
}

/// A non-character is ROUTED, not force-conformed: declaring the asset a Prop dispatches the
/// prop workflow and makes Conform a no-op — no rig, and crucially no invented "no skeleton"
/// failure — while the class reads as the word the user chose. This is the fix for "the
/// import expects a specific thing": it now respects the class.
#[test]
fn a_prop_is_routed_not_conform_failed() {
    let Some(dir) = real_source() else {
        eprintln!("skipping: no content tree");
        return;
    };
    load_shipped_strings(); // the class label asserted below is token-resolved
    let mut doc = Document::new();
    // The Import Prop card's declaration — `open` dispatches the prop workflow with it.
    doc.pending_class = Some(AssetClass::Prop);
    doc.open(dir);
    assert_eq!(doc.workflow, WF_PROP, "a prop dispatches the prop workflow");
    assert_eq!(doc.class(), Some(AssetClass::Prop));
    assert_eq!(doc.workflow, WF_PROP, "a prop conforms by mounting");
    let src = doc.source.as_ref().unwrap();
    assert!(
        src.rig.is_none(),
        "the character conform path must not run on a prop"
    );
    assert!(
        doc.error().is_none(),
        "and it must NOT invent a skeleton failure"
    );
    assert!(doc.bone_rows().is_empty(), "no bone map without a conform");
    assert!(
        class_label(doc.class())
            .to_ascii_lowercase()
            .contains("prop"),
        "the class reads as the word the user chose: {}",
        class_label(doc.class())
    );
}

/// THE STAGED-RELOAD PATH (Aaron 2026-08-20): with a staged rig present under the asset's
/// name, `adopt_staged_from` replaces the parse+conform with the staged model loaded
/// ALREADY-CONFORMED — every bone-map row Ok, zero offsets, no rename — so the wizard
/// lands on the rig view holding exactly what was last committed, ready to adjust
/// further. Exercised against a scratch root, like the commit path.
#[test]
fn adopt_staged_reopens_the_committed_rig() {
    let Some(mut doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    // Stage a small baked rig under this asset's name in a scratch root.
    let name = doc.asset_name().expect("a folder is open").to_string();
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_adopt_staged");
    let _ = std::fs::remove_dir_all(&scratch);
    let staged = {
        let parsed = doc.source.as_ref().unwrap().parsed.as_ref().unwrap();
        flicker_content::bake_rig(&parsed.model, &name)
    };
    let dir = scratch.join(&name);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(
        dir.join(format!("{name}.json")),
        serde_json::to_string(&staged).expect("staged rig serializes"),
    )
    .expect("staged rig writes");
    let staged_bones = staged.skeleton.bones.len();

    // Wipe the conform result and adopt: the rig must come back pre-filled from the file.
    {
        let s = doc.source.as_mut().unwrap();
        s.parsed = None;
        s.rig = None;
    }
    assert!(
        doc.adopt_staged_from(&scratch, "staging"),
        "the staged rig adopts"
    );
    let src = doc.source.as_ref().unwrap();
    assert_eq!(
        src.reopened,
        Some("staging"),
        "provenance names where the rig came from"
    );
    let parsed = src.parsed.as_ref().expect("the staged model is parsed-in");
    assert_eq!(
        parsed.bones() + 1,
        staged_bones,
        "the synthesized root strips back off on the way in"
    );
    let rig = src
        .rig
        .as_ref()
        .expect("the staged model loads already-conformed");
    assert!(
        rig.map.iter().all(|s| *s == MapState::Ok),
        "every staged bone carries Ok provenance"
    );
    assert_eq!(rig.rename.renamed, 0, "a staged rig is already canonical");
    assert_eq!(
        rig.offsets.len(),
        parsed.bones(),
        "offset rows parallel the staged skeleton"
    );
    assert!(
        rig.out.reorient.limbs_aligned == 0,
        "no conform pass ran over the human's fitted joints"
    );

    // A MISSING staged rig falls through: state stays untouched, ready for the next root
    // in the staging→package search order (the promote-emptied-staging case Aaron hit —
    // the ONE copy of a promoted fit lives in package).
    {
        let s = doc.source.as_mut().unwrap();
        s.parsed = None;
        s.rig = None;
        s.reopened = None;
    }
    let empty = std::env::temp_dir().join("flicker_assetpipeline_adopt_staged_empty");
    let _ = std::fs::remove_dir_all(&empty);
    assert!(
        !doc.adopt_staged_from(&empty, "staging"),
        "an empty root adopts nothing"
    );
    {
        let src = doc.source.as_ref().unwrap();
        assert!(
            src.parsed.is_none() && src.rig.is_none() && src.reopened.is_none(),
            "no staged rig → the FBX path stays in charge"
        );
    }
    // …and the same scratch rig offered as the PACKAGE root adopts with its provenance.
    assert!(
        doc.adopt_staged_from(&scratch, "package"),
        "the promoted copy adopts when staging is empty"
    );
    assert_eq!(doc.source.as_ref().unwrap().reopened, Some("package"));
    let _ = std::fs::remove_dir_all(&scratch);
}

/// THE FIT-RETENTION PROOF (Aaron 2026-08-20: "It is unclear if the repositioned skeleton
/// layout is retained. Check that." — skips without the promoted golem): re-opening the
/// PROMOTED rig keeps every fitted joint's WORLD position exactly — through the load, the
/// chain splice, and a re-bake — and the fitted signature (the hand-tuned depths, NOT the
/// canonical reference positions) is what comes back.
#[test]
fn adopting_the_promoted_golem_retains_the_fitted_joints() {
    let path = flicker_content::roots()
        .package()
        .join("characters/GolemBase_Low/GolemBase_Low.json");
    if !flicker_content::package::file_exists(&path) {
        eprintln!("skipping: no promoted golem");
        return;
    }
    let mut m = flicker_content::load_rig_raw(&path).expect("promoted golem loads");
    let world = |m: &RawModel| -> std::collections::HashMap<String, Vec3> {
        let (globals, _) = rest_globals(m, &[]);
        m.bones
            .iter()
            .zip(&globals)
            .map(|(b, g)| (b.name.clone(), g.w_axis.truncate()))
            .collect()
    };
    let before = world(&m);
    // The AUTHORED signature, not canon: this body's hands sit far inboard of the
    // canonical 63.9 (the golem's own proportions) — if a conform pass had run over the
    // reload, they would snap back toward canon. The head is deliberately NOT pinned:
    // it is the joint the human keeps re-authoring, so freezing one fit's value here
    // made the guard fail on every legitimate re-promote.
    assert!(
        before["hand_l"].x < 50.0,
        "the promoted rig carries the AUTHORED hand, got {}",
        before["hand_l"]
    );
    // The chain heal moves NO joint: world frames are preserved by construction.
    let spliced =
        flicker_content::splice_canonical_chain(&mut m, &flicker_content::default_reference())
            .expect("splice runs");
    let after = world(&m);
    for (name, p) in &before {
        let q = after[name];
        assert!(
            (q - *p).length() < 1e-2,
            "splice moved {name}: {p} → {q} (spliced={spliced:?})"
        );
    }
    // …and a re-commit round trip returns the same skeleton, byte-shaped.
    let baked = flicker_content::bake_rig(&m, "GolemBase_Low");
    let m2 = {
        // rig_to_raw is crate-private to flicker-content; round-trip through serde instead.
        let text = serde_json::to_string(&baked).expect("bake serializes");
        let tmp = std::env::temp_dir().join("flicker_assetpipeline_fit_retention");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("X")).expect("tmp");
        std::fs::write(tmp.join("X/X.json"), text).expect("write");
        let m2 = flicker_content::load_rig_raw(&tmp.join("X/X.json")).expect("reload");
        let _ = std::fs::remove_dir_all(&tmp);
        m2
    };
    // A bake CARRIES the body so its pelvis stands over the root (bake_rig, 2026-09-07) — a
    // rig promoted before that rule may shift once, rigidly, in X and Y — so the round trip
    // is judged SHAPE-wise: every joint keeps its offset from the pelvis, and the carry is
    // horizontal only.
    let rebaked = world(&m2);
    let carry = rebaked["pelvis"] - after["pelvis"];
    assert!(
        carry.z.abs() < 1e-3,
        "a re-bake never lifts the body: {carry}"
    );
    for (name, p) in &after {
        let q = rebaked[name];
        assert!(
            (q - (*p + carry)).length() < 1e-2,
            "re-bake changed the shape at {name}: {p} → {q} (carry {carry})"
        );
    }
    let pelvis = rebaked["pelvis"];
    assert!(
        pelvis.x.abs() < 1e-3 && pelvis.y.abs() < 1e-3,
        "the re-baked pelvis stands over the root: {pelvis}"
    );
}

/// Commit ROUTES by class: a Prop writes a STATIC-prop rig (empty skeleton, retarget:false),
/// not a conformed character — the prop bake path exercised end to end against a scratch
/// dir (the character source stands in for a prop mesh; the class override is what selects
/// the bake, which is the routing under test).
#[test]
fn commit_routes_a_prop_to_the_static_bake() {
    let Some(mut doc) = parsed() else {
        eprintln!("skipping: no content tree");
        return;
    };
    {
        let s = doc.source.as_mut().unwrap();
        s.class = Some(AssetClass::Prop);
        s.prop = PropKind::Weapon;
    }
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_prop_commit");
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(!doc.has_committed());
    doc.commit_to(&scratch);

    let src = doc.source.as_ref().unwrap();
    assert!(
        src.error.is_none(),
        "the prop commit succeeds: {:?}",
        src.error
    );
    assert!(doc.has_committed(), "the commit is recorded");
    let out = src.committed.clone().expect("a committed path is recorded");
    let text = flicker_content::package::read_text(&out).expect("the prop rig was written");
    assert!(
        text.contains("\"bones\":[]"),
        "a prop bakes an EMPTY skeleton: {out:?}"
    );
    assert!(
        text.contains("\"retarget\":false"),
        "a prop is retarget:false"
    );

    // And it SHIPS ITS TEXTURES: the bake is handed the source mesh, so the vendor's maps
    // are copied beside the rig under the content standard's names and referenced by the
    // material — a prop that arrives as a lone `.json` renders untextured.
    let name = doc.asset_name().expect("a folder is open").to_string();
    let dir = out.parent().expect("the rig sits in the asset's folder");
    let rig: flicker_skeletal::format::RigFile =
        serde_json::from_str(&text).expect("the prop rig parses");
    let m = rig.mesh.materials.first().expect("the prop has a material");
    assert_eq!(
        m.base_color,
        format!("{name}_BaseColor.png"),
        "albedo wired into the material"
    );
    assert!(
        dir.join(&m.base_color).exists(),
        "and copied beside the rig"
    );
    for map in [&m.normal, &m.roughness, &m.metalness] {
        assert!(
            !map.is_empty(),
            "every source map the standard has a slot for is wired"
        );
        assert!(dir.join(map).exists(), "{map} copied beside the rig");
    }
    let _ = std::fs::remove_dir_all(&scratch);
}

/// MOST source folders hold SEVERAL riggable meshes — a weapon set is four or five pieces, an
/// outfit is tops/pants/gloves/shoes. Such a folder must offer a PICKER, not be refused: it opens
/// with the first pre-selected and parsed, and picking a different piece re-points the import
/// AND drops everything derived from the previous one.
#[test]
fn a_multi_mesh_folder_offers_a_picker() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content/source/PrismWeaps/MuseEpicSet");
    if !dir.exists() {
        eprintln!("skipping: no PrismWeaps source");
        return;
    }
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Prop); // the Import Accessory / Prop card's declaration
    doc.open(dir);
    let rows = doc.candidate_rows();
    assert!(rows.len() > 1, "a weapon set holds several meshes");
    {
        let src = doc.source.as_ref().expect("the folder opened");
        assert_eq!(rows.len(), src.candidates.len(), "one row per piece");
        assert!(
            src.error.is_none(),
            "several meshes is a CHOICE, not an error: {:?}",
            src.error
        );
        assert_eq!(
            src.fbx, src.candidates[0],
            "the first is pre-selected — never stuck"
        );
        assert!(src.parsed.is_some(), "the first pick is parsed on open");
    }
    assert_eq!(doc.workflow, WF_PROP);
    assert!(
        doc.file_name().is_some_and(|f| f == rows[0].1),
        "the display name is the picked file's name"
    );

    assert_eq!(
        doc.selected_candidate(),
        Some(rows[0].0.as_str()),
        "the picker's bound value is the first stem"
    );

    // Choose the second — the stale parse must be dropped so nothing derived carries forward.
    assert!(
        doc.select_candidate(&rows[1].0),
        "the stem selects the piece"
    );
    assert_eq!(doc.selected_candidate(), Some(rows[1].0.as_str()));
    let src = doc.source.as_ref().unwrap();
    assert_eq!(src.candidate_sel, 1);
    assert_eq!(
        src.fbx, src.candidates[1],
        "the import now points at the second mesh"
    );
    assert!(
        src.parsed.is_none(),
        "the previous mesh's parse was dropped"
    );
    assert!(
        src.report.is_none() && src.rig.is_none(),
        "and everything derived from it"
    );
    assert!(
        !doc.select_candidate("not-a-piece"),
        "an unknown stem selects nothing"
    );
}

/// THE multi-piece LOOP: once a piece is committed, "import next piece" keeps the folder + its
/// piece list intact and drops everything derived from the finished piece — so a weapon set or
/// an outfit is walked one piece at a time, formally, without leaving the scene. The picker is
/// right there to choose the next piece.
#[test]
fn committing_a_piece_offers_the_loop_back_to_the_picker() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content/source/PrismWeaps/MuseEpicSet");
    if !dir.exists() {
        eprintln!("skipping: no PrismWeaps source");
        return;
    }
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Prop);
    doc.open(dir);
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_next_piece");
    let _ = std::fs::remove_dir_all(&scratch);
    doc.commit_to(&scratch);
    assert!(doc.has_committed(), "the piece baked: {:?}", doc.error());
    assert!(
        doc.has_committed() && doc.candidate_rows().len() > 1,
        "the loop-back is offered"
    );

    let n = doc.candidate_rows().len();
    doc.start_next_piece();
    let src = doc.source.as_ref().unwrap();
    assert_eq!(
        src.candidates.len(),
        n,
        "the folder and its piece list are kept"
    );
    assert!(
        src.parsed.is_none() && src.rig.is_none() && src.committed.is_none(),
        "the finished piece's state is dropped so the next starts clean"
    );
    assert!(!doc.has_committed());
    assert_eq!(
        doc.candidate_rows().len(),
        n,
        "the picker still lists the whole set"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The FIT stage is the prop/garment's human-in-the-loop mount authoring: for a non-character
/// the Conform role is Mount, and picking a socket + writing the fit lands in `src.fit` — which
/// Commit then bakes. This is the whole point of the tool: the human places and verifies, the
/// bake honours it.
#[test]
fn fit_stage_authors_the_prop_mount() {
    let Some(dir) = real_source() else {
        eprintln!("skipping: no content tree");
        return;
    };
    let mut doc = Document::new();
    // The Import Prop card's declaration; `open` dispatches the prop workflow, whose
    // rig page IS Conform under the Mount role — not a separate later stage.
    doc.pending_class = Some(AssetClass::Prop);
    doc.open(dir);
    assert_eq!(doc.workflow, WF_PROP);
    assert_eq!(doc.workflow, WF_PROP, "a prop conforms by mounting");
    let sockets = doc.socket_rows();
    assert_eq!(
        sockets.len(),
        SOCKETS.len(),
        "the picker lists every socket"
    );
    assert!(
        sockets.iter().all(|(_, label)| label.starts_with('$')),
        "socket labels are tokens"
    );

    // Pick a socket and write the X-offset + rotation + scale, exactly as the sliders do.
    assert!(doc.select_socket("Weapon_R"), "a curated socket mounts");
    {
        let fit = doc.fit_mut().expect("a prop has a fit");
        fit.offset[0] = 3.5;
        fit.rot[2] = 45.0;
        fit.scale[1] = 2.0;
        fit.uniform = 1.5;
    }
    let fit = *doc.fit().expect("a prop has a fit");
    assert_eq!(
        SOCKETS[fit.socket].0, "Weapon_R",
        "the picked socket is now the mount"
    );
    assert!(
        (fit.offset[0] - 3.5).abs() < 1e-4,
        "the offset slider authored the fit"
    );
    assert!(
        (fit.rot[2] - 45.0).abs() < 1e-4,
        "the rotation slider authored the fit"
    );
    // Per-axis scale RESHAPES (only the dragged axis moves) and scale-all is a SEPARATE
    // multiplier — the paperdoll gadget's pair. Conflating them would silently rescale the
    // other two axes the moment the user touched one.
    assert!(
        (fit.scale[1] - 2.0).abs() < 1e-4,
        "the Y scale slider authored that axis"
    );
    assert!((fit.scale[0] - 1.0).abs() < 1e-4, "and left X alone");
    assert!((fit.scale[2] - 1.0).abs() < 1e-4, "and left Z alone");
    assert!(
        (fit.uniform - 1.5).abs() < 1e-4,
        "scale-all rides `fit_scale`"
    );

    // The whole point of widening: both reach the BAKED rig, because the format already
    // carried `scale` × `uniform` and `attach_world` already applied it.
    let baked = Fit {
        socket: fit.socket_name().to_string(),
        offset: fit.offset,
        rot_deg: fit.rot,
        scale: fit.scale,
        uniform: fit.uniform,
    }
    .to_attach();
    assert!(
        (baked.scale[1] - 2.0).abs() < 1e-4,
        "per-axis scale survives to the format"
    );
    assert!(
        (baked.uniform - 1.5).abs() < 1e-4,
        "scale-all survives to the format"
    );

    // And that authored socket is a REAL bone name the bake can resolve against the base.
    assert!(
        SOCKETS.get(fit.socket).is_some(),
        "the mount indexes the socket table"
    );
    assert!(
        !doc.select_socket("nowhere"),
        "an unknown socket mounts nothing"
    );
}

/// COMMIT writes a rig the engine's own loader accepts, carrying the authored offsets and
/// the bake's synthesized root. Written to a scratch dir — the live content tree is Aaron's,
/// and a test that rewrote a shipped character would be a destructive one.
#[test]
fn commit_writes_a_loadable_rig_carrying_the_authored_offsets() {
    let Some(mut doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    // Author a distinctive offset so the written file can be told from a plain bake.
    assert!(doc.select_bone_named("head"));
    let sel = doc.bone_sel().unwrap();
    doc.set_selected_offset(BoneOffset {
        t: [0.0, 0.0, 3.5],
        roll: 0.0,
        ..Default::default()
    });
    let baseline = doc
        .source
        .as_ref()
        .unwrap()
        .parsed
        .as_ref()
        .unwrap()
        .model
        .bones[sel]
        .translation[2];

    let out_root = std::env::temp_dir().join("flicker_assetpipeline_commit");
    let _ = std::fs::remove_dir_all(&out_root);
    doc.commit_to(&out_root);

    let src = doc.source.as_ref().unwrap();
    assert!(src.error.is_none(), "commit reported: {:?}", src.error);
    let written = src
        .committed
        .as_ref()
        .expect("commit recorded where it wrote");
    assert!(
        flicker_content::package::file_exists(written),
        "{} was written",
        written.display()
    );

    // Round-trip through the ENGINE's loader, not a bespoke parse — if the bake drifted from
    // what the runtime accepts, this is where it shows.
    let raw = flicker_content::package::read_text(written).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).expect("valid rig json");
    let bones = json["skeleton"]["bones"]
        .as_array()
        .expect("skeleton.bones");
    assert_eq!(
        bones.len(),
        REFERENCE_BONES,
        "the bake synthesized the root"
    );
    assert_eq!(bones[0]["name"], "root", "root is bone 0");

    // The Attach stage's six authored points SHIP (the audited third-step gap: they
    // used to be discarded at export). Each carries its id and canonical parent bone.
    let points = json["attach_points"].as_array().expect("attach_points");
    assert_eq!(
        points.len(),
        ATTACH_POINTS.len(),
        "all six authored points ship"
    );
    for ((id, _, parent), p) in ATTACH_POINTS.iter().zip(points) {
        assert_eq!(p["id"], *id, "point id ships");
        assert_eq!(p["bone"], *parent, "and rides its canonical bone");
    }

    // The authored offset is IN the file: the working model is untouched, the bake carries it.
    assert_eq!(
        doc.source
            .as_ref()
            .unwrap()
            .parsed
            .as_ref()
            .unwrap()
            .model
            .bones[sel]
            .translation[2],
        baseline,
        "the working model stays the conform baseline — offsets remain reversible"
    );
    let head = bones
        .iter()
        .find(|b| b["name"] == "head")
        .expect("head survived the bake");
    // `local` is a column-major 4x4; the translation is the last column's first three.
    let local = head["local"].as_array().expect("local matrix");
    let tz = local[14].as_f64().expect("t.z") as f32;
    assert!(
        (tz - (baseline + 3.5)).abs() < 1e-3,
        "the authored +3.5 is baked in: {tz} vs {}",
        baseline + 3.5
    );

    let _ = std::fs::remove_dir_all(&out_root);
}

/// THE COMMIT TRANSLATION GUARD (2026-08-20): a character committed from the AS-PROVIDED
/// editing view (vendor frames live in the bench) must land in staging with CANONICAL joint
/// orientations — the shared clips play absolute rotations in canonical frames, and commit
/// is that invariant's output gate. Positions ship as placed; the bench's working model
/// keeps its vendor frames after commit (the translation belongs to the output, not the view).
#[test]
fn committing_an_as_provided_rig_translates_frames_to_canon() {
    let Some(dir) = real_source() else {
        eprintln!("skipping: no content tree");
        return;
    };
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Skin);
    doc.as_provided = true;
    doc.open(dir);
    assert!(
        doc.source.as_ref().unwrap().rig.is_some(),
        "open conforms the vendor rig as provided"
    );

    // A bone's LOCAL frame rotation out of a rig json.
    let local_quat = |json: &serde_json::Value, name: &str| -> glam::Quat {
        let b = json["skeleton"]["bones"]
            .as_array()
            .expect("skeleton.bones")
            .iter()
            .find(|b| b["name"] == name)
            .unwrap_or_else(|| panic!("{name} present"));
        let local = b["local"].as_array().expect("local matrix");
        let mut m = [0.0f32; 16];
        for (i, f) in local.iter().enumerate().take(16) {
            m[i] = f.as_f64().unwrap_or(0.0) as f32;
        }
        let (_, q, _) = glam::Mat4::from_cols_array(&m).to_scale_rotation_translation();
        q
    };
    let model_pelvis = |doc: &Document| -> glam::Quat {
        let m = &doc.source.as_ref().unwrap().parsed.as_ref().unwrap().model;
        let i = m
            .bones
            .iter()
            .position(|b| b.name == "pelvis")
            .expect("pelvis present");
        glam::Quat::from_array(m.bones[i].rotation)
    };
    let ref_json: serde_json::Value =
        serde_json::from_str(&flicker_content::package::read_text(&default_reference()).unwrap())
            .unwrap();
    let canon_pelvis = local_quat(&ref_json, "pelvis");

    // The as-provided working model carries the vendor's pelvis frame, measurably off canon.
    // If this ever reads near-zero the vendor changed conventions and the as-provided view
    // stopped being a distinct state — worth failing loudly over.
    let vendor_pelvis = model_pelvis(&doc);
    assert!(
        vendor_pelvis.angle_between(canon_pelvis).to_degrees() > 10.0,
        "the vendor pelvis frame differs from canon (else as-provided is vacuous)"
    );

    let out_root = std::env::temp_dir().join("flicker_assetpipeline_as_provided_commit");
    let _ = std::fs::remove_dir_all(&out_root);
    doc.commit_to(&out_root);
    let src = doc.source.as_ref().unwrap();
    assert!(src.error.is_none(), "commit reported: {:?}", src.error);
    let written = src
        .committed
        .as_ref()
        .expect("commit recorded where it wrote");
    let json: serde_json::Value =
        serde_json::from_str(&flicker_content::package::read_text(written).unwrap()).unwrap();
    let committed_pelvis = local_quat(&json, "pelvis");
    assert!(
        committed_pelvis.angle_between(canon_pelvis).to_degrees() < 1.0,
        "the committed pelvis carries the canonical frame, got {:.2}° off",
        committed_pelvis.angle_between(canon_pelvis).to_degrees()
    );
    assert!(
        model_pelvis(&doc).angle_between(vendor_pelvis).to_degrees() < 0.01,
        "the working model keeps the vendor frames after commit"
    );

    let _ = std::fs::remove_dir_all(&out_root);
}

/// THE SMOKE-TEST BAKE (Aaron 2026-08-20): the Preview page's bake IS the commit bake (one
/// shared helper), so what the page plays can never drift from what Export writes. The
/// shared idle must resolve onto the baked bones and pose the body upright.
#[test]
fn the_preview_page_plays_the_commit_bake_under_the_shared_idle() {
    let Some(mut doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    // Author a distinctive joint move so the preview must carry it.
    assert!(doc.select_bone_named("head"));
    doc.set_selected_offset(BoneOffset {
        t: [0.0, 0.0, 3.5],
        roll: 0.0,
        ..Default::default()
    });

    let (_rig_file, bones, clip) = doc.bake_preview_parts().expect("the preview bakes");
    assert!(
        !clip.tracks.is_empty(),
        "the shared idle resolves onto the baked bones"
    );

    // The preview IS the commit: the written file carries the same skeleton bone-for-bone.
    let out_root = std::env::temp_dir().join("flicker_assetpipeline_bake_preview");
    let _ = std::fs::remove_dir_all(&out_root);
    doc.commit_to(&out_root);
    let src = doc.source.as_ref().unwrap();
    assert!(src.error.is_none(), "commit reported: {:?}", src.error);
    let written = src
        .committed
        .as_ref()
        .expect("commit recorded where it wrote");
    let json: serde_json::Value =
        serde_json::from_str(&flicker_content::package::read_text(written).unwrap()).unwrap();
    let wb = json["skeleton"]["bones"]
        .as_array()
        .expect("skeleton.bones");
    assert_eq!(wb.len(), bones.len(), "same skeleton size as the preview");
    for (i, b) in bones.iter().enumerate() {
        assert_eq!(wb[i]["name"], b.name, "bone {i} matches the preview");
        let l = wb[i]["local"].as_array().expect("local");
        let stored: Vec<f32> = l.iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let ours = b.local.to_cols_array();
        for k in 0..16 {
            assert!(
                (stored[k] - ours[k]).abs() < 1e-2,
                "bone {} local[{k}] drifted: {} vs {}",
                b.name,
                stored[k],
                ours[k]
            );
        }
    }

    // The smoke test's own smoke test: mid-idle the baked body poses UPRIGHT
    // (Z-tallest in source space) — a Katanami-class contortion would fail this.
    let locals = sample_local_poses(&bones, &clip, 100, true);
    let globals = global_transforms(&bones, &locals);
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for g in &globals {
        let p = g.w_axis.truncate();
        min = min.min(p);
        max = max.max(p);
    }
    let d = max - min;
    assert!(
        d.z > d.x && d.z > d.y,
        "the animated preview stands tall along Z, got extents {d:?}"
    );

    let _ = std::fs::remove_dir_all(&out_root);
}

/// Re-baking the skin WEIGHTS from the (repositioned) skeleton replaces the source's auto-skin
/// and re-skins the view — the rest mesh does not move, only its deformation changes. Real
/// content; skips without it.
#[test]
fn bake_skin_re_weights_without_moving_the_rest_mesh() {
    let Some(mut doc) = at_conform() else {
        eprintln!("skipping: no content tree");
        return;
    };
    let positions = |doc: &Document| -> Vec<[f32; 3]> {
        doc.source
            .as_ref()
            .unwrap()
            .parsed
            .as_ref()
            .unwrap()
            .model
            .vertices
            .iter()
            .map(|v| v.p)
            .collect()
    };
    let before = positions(&doc);
    let pose_gen = doc.pose_gen;
    doc.bake_skin_now();
    assert_eq!(
        positions(&doc),
        before,
        "the rest mesh does not move — only its weights change"
    );
    assert_ne!(doc.pose_gen, pose_gen, "the live skin re-uploads");
    let parsed = doc.source.as_ref().unwrap().parsed.as_ref().unwrap();
    assert!(
        parsed
            .model
            .vertices
            .iter()
            .all(|v| (v.weights.iter().sum::<f32>() - 1.0).abs() < 1e-3),
        "every vertex is weighted to the skeleton"
    );
}

/// The fitting body is the REFERENCE a piece is placed against, so its MESH must load, not
/// just its bones — judging whether hair sits on the skull needs a shape, not a stick figure.
/// Real content; skips without it. (Parked with the viewport tier: this moves with `RigView`.)
#[test]
fn the_fitting_body_loads_its_mesh_for_the_reference_view() {
    let Some(base) = BasePreview::load() else {
        eprintln!("skipping: no content tree");
        return;
    };
    assert!(!base.globals.is_empty(), "the fitting body has a skeleton");
    assert!(
        !base.verts.is_empty(),
        "the fitting body must carry a MESH — `fitting_base` prefers the ~3.3k-tri \
         GolemBase_Low, which is far under the budget"
    );
    assert!(
        base.verts.len() <= BASE_MESH_BUDGET,
        "and it fits the upload budget"
    );
    // A well-formed triangle list that indexes only real vertices — a bad one would fault the
    // draw rather than merely look wrong.
    assert!(
        !base.indices.is_empty() && base.indices.len() % 3 == 0,
        "a triangle list"
    );
    let n = base.verts.len() as u32;
    assert!(
        base.indices.iter().all(|i| *i < n),
        "every index is inside the vertex list"
    );

    // Framing now comes from the MESH when there is one, so the stage floor is the SOLE of the
    // foot rather than the lowest JOINT (the ankle sits well above the sole — `ANKLE_FRACTION`).
    // Getting this wrong floats the body above its own grid.
    assert!(base.floor < 0.0, "the recentred floor is below the origin");
    let lowest_vert = base
        .verts
        .iter()
        .map(|v| v.position[2])
        .fold(f32::MAX, f32::min);
    let lowest_joint = base
        .globals
        .iter()
        .map(|g| g.w_axis.z)
        .fold(f32::MAX, f32::min);
    assert!(
        lowest_vert <= lowest_joint + 1e-3,
        "the mesh must reach at or below the lowest joint (sole {lowest_vert}, joint {lowest_joint})"
    );
}

/// The Prep decimate field resolves against the SOURCE count: digits parse, empty or zero
/// means 100% (the source), and nothing above the source is asked for.
#[test]
fn the_decimate_target_resolves_against_the_source_count() {
    assert_eq!(Document::prep_target("8000", 123_623), 8000);
    assert_eq!(Document::prep_target("", 123_623), 123_623);
    assert_eq!(Document::prep_target("0", 123_623), 123_623);
    assert_eq!(Document::prep_target("999999", 123_623), 123_623);
    assert_eq!(Document::prep_target("12a", 123_623), 123_623);
}

/// A closed lat-long sphere in `parse_fbx`'s convention (per-corner vertices, sequential
/// indices) with SMOOTH normals and one uv, so the decimator's weld sees a closed interior
/// mesh it can legally collapse — the raw-mesh stand-in for a Meshy export.
/// A clean scratch SOURCE FOLDER — the stand-in for a vendor export directory, and the
/// thing the OS folder dialog returns. Shared by the rigged-document fixture and by the
/// pick-seam gate, so both speak about the same kind of place.
pub(crate) fn synth_source_dir(tag: &str) -> std::path::PathBuf {
    let scratch = std::env::temp_dir().join(format!("flicker_assetpipeline_{tag}"));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("the scratch source folder is created");
    scratch
}

/// A headless document with the canon rig installed on a synthetic sphere: opened in a
/// scratch folder, parsed from `sphere_mesh`, prepped, then conformed — the shape the
/// Rig step works on, with no vendor file in sight.
pub(crate) fn synthetic_rigged_doc(tag: &str) -> Document {
    let scratch = synth_source_dir(tag);
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Skin);
    doc.open(scratch);
    {
        let src = doc.source.as_mut().expect("the scratch folder opened");
        src.parsed = Some(Parsed::new(sphere_mesh(6, 8, 50.0)));
        src.error = None;
    }
    doc.ensure_prep_source();
    doc.conform();
    assert!(
        doc.bone_count().unwrap_or(0) > 0,
        "conform installs the canon on a raw mesh"
    );
    doc
}

/// Drop whatever the shape matcher found for the working body, leaving the document in the
/// NO-MATCH state — a vendor rig corrected onto the canon, a staged rig re-opened, or a mesh the
/// graph could not read. The rail then prompts the whole depth-first walk.
fn clear_shape_match(doc: &mut Document) {
    doc.source
        .as_mut()
        .and_then(|s| s.rig.as_mut())
        .expect("a rigged body")
        .shape = None;
}

/// Put `m` in as the working body's match — the bench's view of what the fit's `FitReport`
/// carried, which is the one thing the rail reads.
fn set_shape_match(doc: &mut Document, m: ShapeMatch) {
    doc.source
        .as_mut()
        .and_then(|s| s.rig.as_mut())
        .expect("a rigged body")
        .shape = Some(m);
}

/// THE RAIL PROMPTS WHAT THE MATCHER DID NOT MATCH (S2 431D08DF, on Aaron's fetlock amendment
/// 6234D0F6, under rule 513E5F78: *"what does not match is what the human is asked for"*).
///
/// The rail's list IS `ShapeMatch::marker_order` — the unmatched modules' prompts in the
/// depth-first walk order — whenever the fit read a shape, and the WHOLE walk when it did not. A
/// matched joint is not prompted, is still selectable and draggable, and is HELD through Infer
/// exactly as a joint the human placed is. The MATCH STATUS line reads the counts.
#[test]
fn the_rail_is_the_matchers_unmatched_walk() {
    load_shipped_strings();
    let mut doc = synthetic_rigged_doc("rail_unmatched");
    let recipe = doc.recipe();
    let id = flicker_content::baseline::module_id;
    let bone = |doc: &Document, name: &str| -> usize {
        doc.parsed()
            .expect("a rigged body")
            .model
            .bones
            .iter()
            .position(|b| b.name == name)
            .unwrap_or_else(|| panic!("the canon carries `{name}`"))
    };
    let world = |doc: &Document, name: &str| -> Vec3 {
        doc.parsed().expect("a rigged body").globals[bone(doc, name)]
            .w_axis
            .truncate()
    };
    // A prompt for a bone the working rig does not carry would be a rail the human cannot step
    // off (4BB12A75), so both sides of the comparison are read through the same filter.
    let carried = |doc: &Document, list: Vec<String>| -> Vec<String> {
        let bones: Vec<String> = doc
            .parsed()
            .expect("a rigged body")
            .model
            .bones
            .iter()
            .map(|b| b.name.clone())
            .collect();
        list.into_iter().filter(|n| bones.contains(n)).collect()
    };

    // THE FIT READ A SHAPE. The fixture is a ball: one core and no limbs, so some of the recipe's
    // modules find a partner in it and some cannot.
    let m = doc
        .shape_match()
        .expect("the raw-mesh fit read the ball's shape graph")
        .clone();
    assert!(
        !m.matched.is_empty() && !m.unmatched.is_empty(),
        "the ball answers for some modules and not others: {:?} / {:?}",
        m.matched,
        m.unmatched
    );

    // THE RAIL IS THE MATCHER'S OWN LIST — not a second filter that could drift out of step.
    let prompted = doc.markers();
    assert_eq!(
        prompted,
        carried(&doc, m.marker_order(&recipe)),
        "the rail walks exactly `ShapeMatch::marker_order`"
    );
    let full = carried(&doc, markers_for(&recipe));
    assert!(
        prompted.len() < full.len(),
        "and it is SHORTER than the whole walk ({} of {})",
        prompted.len(),
        full.len()
    );
    // ORDER IS THE WALK'S, still: every prompt keeps its place in the depth-first list.
    let mut at = 0usize;
    for name in &prompted {
        let k = full[at..]
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("`{name}` is a prompt of the full walk, in order"));
        at += k + 1;
    }

    // THE OTHER HALF of the same walk is the joints the MATCHER placed.
    let held: Vec<String> = full
        .iter()
        .filter(|n| !prompted.contains(n))
        .cloned()
        .collect();
    assert!(
        !held.is_empty(),
        "a matched module's joints are not prompted"
    );

    // A MATCHED joint is PLACED, not prompted — and it is still the human's to move.
    let first_held = held[0].clone();
    let i = bone(&doc, &first_held);
    let before = world(&doc, &first_held);
    let globals = doc.parsed().expect("a rigged body").globals.clone();
    doc.reposition_bone(i, &globals, Vec3::new(0.0, 0.0, 6.0));
    assert!(
        (world(&doc, &first_held) - before).length() > 1.0,
        "a matched joint is still draggable"
    );
    assert!(
        doc.placed()[i],
        "and the drag marks it placed like any other"
    );

    // AND INFER HOLDS THEM ALL. A matched module's chain was laid down the partner the match
    // chose — at the narrowings it found, with its ground joint on the floor — so deriving it
    // again from the flesh would throw that answer away. The same pin a placed joint gets, and
    // none of these carries the HUMAN's placed signal.
    let stood: Vec<Vec3> = held.iter().map(|n| world(&doc, n)).collect();
    for name in held.iter().skip(1) {
        assert!(
            !doc.placed()[bone(&doc, name)],
            "the matcher's joints are not the human's placed signal (`{name}`)"
        );
    }
    assert!(doc.infer_markers() > 0, "Infer still derives the unmatched");
    for (name, was) in held.iter().zip(&stood) {
        let now = world(&doc, name);
        assert!(
            (now - *was).length() < 1e-3,
            "the MATCHED `{name}` is held through Infer: {was:?} -> {now:?}"
        );
    }

    // THE MATCH STATUS reads the counts and names where the rail starts.
    let total = m.matched.len() + m.unmatched.len();
    let caption = doc.marker_match_caption();
    assert!(
        !caption.starts_with('$') && !caption.contains('{'),
        "the status resolves and takes its data: {caption}"
    );
    for want in [
        m.matched.len().to_string(),
        total.to_string(),
        prompted[0].clone(),
    ] {
        assert!(
            caption.contains(&want),
            "the status carries `{want}`: {caption}"
        );
    }

    // A DIFFERENT MATCH gives a different rail off the same walk: match the limbs instead, and
    // the human is asked for the trunk and the head.
    set_shape_match(
        &mut doc,
        ShapeMatch {
            matched: vec![
                (id("leg", ""), Matched::Pair(0)),
                (id("arm", ""), Matched::Pair(1)),
            ],
            unmatched: vec![id("trunk", ""), id("head", "")],
            ..ShapeMatch::default()
        },
    );
    assert_eq!(
        doc.markers().first().map(String::as_str),
        Some("pelvis"),
        "the unmatched trunk is walked first — the root before its own chain (D81498B7)"
    );
    assert!(
        doc.markers().iter().any(|n| n == "head"),
        "and the unmatched head with it"
    );
    assert!(
        !doc.markers().iter().any(|n| n == "thigh_l"),
        "while a MATCHED leg pair is placed, not prompted"
    );
    assert!(
        doc.marker_match_caption().contains('2'),
        "two of four: {}",
        doc.marker_match_caption()
    );

    // EVERYTHING MATCHED: nothing is left on the rail, and the status says so in its own sentence.
    set_shape_match(
        &mut doc,
        ShapeMatch {
            matched: vec![
                (id("trunk", ""), Matched::Core(0)),
                (id("head", ""), Matched::Limb(0)),
                (id("leg", ""), Matched::Pair(0)),
                (id("arm", ""), Matched::Pair(1)),
            ],
            ..ShapeMatch::default()
        },
    );
    assert!(doc.markers().is_empty(), "no module wants a human");
    assert!(
        doc.marker_caption().is_empty(),
        "so there is no joint to prompt for"
    );
    let all = doc.marker_match_caption();
    assert!(
        !all.starts_with('$') && !all.contains('{') && all.contains('4'),
        "and the status still reads the counts: {all}"
    );

    // NO MATCH AT ALL — no fit ran, or the mesh had no readable shape — is the WHOLE walk, which
    // is what the rail has always been.
    clear_shape_match(&mut doc);
    assert_eq!(doc.markers(), full, "no match prompts every joint");
    assert!(
        doc.marker_match_caption().is_empty(),
        "and there is no match to report"
    );
}

/// A RE-FIT RESETS THE RAIL: a Prep change rebuilds the prepped mesh, the rig re-installs on the
/// next Conform, and the rail opens on the NEW match's first prompt rather than wherever the
/// human had walked to on the old one.
#[test]
fn a_re_fit_resets_the_rail_to_the_new_match() {
    let mut doc = synthetic_rigged_doc("rail_refit");
    let opened = doc.markers();
    assert!(
        !opened.is_empty(),
        "the fresh fit leaves work for the human"
    );
    assert!(
        doc.step_marker(crate::services::MarkerStep::Skip),
        "the human walks off the first prompt"
    );
    assert_ne!(doc.marker(), 0, "and the rail really moved");

    // PREP CHANGED → the geometry is rebuilt and the rig goes with it.
    doc.rebuild_prepped_model();
    assert!(
        doc.shape_match().is_none(),
        "the old match went with the old rig"
    );
    doc.conform();

    assert_eq!(doc.marker(), 0, "the rail opens on the new list");
    assert_eq!(doc.markers(), opened, "which is the same fit again");
    assert!(
        doc.shape_match().is_some(),
        "and the re-fit read the shape again"
    );
    assert!(
        !doc.placed().iter().any(|p| *p),
        "a fresh rig has nothing placed"
    );
}

/// THE RIG STEP KEEPS THE FIT'S OWN MATCH (0F0208AC's seam, retired) AND ITS OWN BODY (DD7A59A9).
/// `rig_raw_mesh` hands back the fit's `FitReport`, and the rail's match IS that report's:
/// entering the Rig step thins the raw mesh ONCE — the fit's read, which the skin bind reuses —
/// and never a second `ShapeGraph::build` to re-derive the match the fit already acted on; the
/// read itself is kept beside the model, so the bakes after it read nothing either.
#[test]
fn the_rig_step_keeps_the_fits_own_match() {
    let scratch = synth_source_dir("fits_own_match");
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Skin);
    doc.open(scratch);
    {
        let src = doc.source.as_mut().expect("the scratch folder opened");
        src.parsed = Some(Parsed::new(sphere_mesh(6, 8, 50.0)));
        src.error = None;
    }
    doc.ensure_prep_source();
    // The same prepped mesh rigged on its own: what the fit itself reports.
    let mut probe = doc.parsed().expect("a prepped mesh").model.clone();
    let fit = flicker_content::rig_raw_mesh(&mut probe, doc.stature_cm, &doc.recipe())
        .expect("the ball rigs");
    let reads = flicker_content::ShapeGraph::reads();
    doc.conform();
    assert_eq!(
        flicker_content::ShapeGraph::reads() - reads,
        1,
        "the Rig step reads the body once — the fit's own read"
    );
    assert!(fit.shape.is_some(), "the fit read a shape off the ball");
    assert_eq!(
        doc.shape_match(),
        fit.shape.as_ref(),
        "the rail's match is the fit's own report"
    );
    // ...AND THE BAKE READS NOTHING MORE (DD7A59A9): Preview and Commit square the stance on the
    // body the fit read, kept beside the model; the Bake-skin button binds on it too.
    doc.character_bake_model().expect("the Preview bake runs");
    doc.character_bake_model().expect("the Commit bake runs");
    doc.bake_skin_now();
    assert_eq!(
        flicker_content::ShapeGraph::reads() - reads,
        1,
        "one body read per import, across conform and every bake"
    );
}

/// THE MARKERS RAIL's PLACED SIGNAL and its INFER verb (spec FF40E825; the signal the Fill verb
/// lacked, 20CA9653, and the first caller `fill_chain` has had since 3995EF9E).
///
/// A hand drag MARKS the joint placed, Infer NEVER does, the joints the human placed come through
/// Infer exactly where he left them, the unplaced interior of a chain lands BETWEEN them and on the
/// body's axis, and a second Infer after nudging one end re-derives the unplaced joints again.
#[test]
fn infer_derives_the_unplaced_joints_and_leaves_the_placed_ones_alone() {
    fn bone(d: &Document, name: &str) -> usize {
        d.parsed()
            .expect("a rigged body")
            .model
            .bones
            .iter()
            .position(|b| b.name == name)
            .unwrap_or_else(|| panic!("the canon carries `{name}`"))
    }
    fn world(d: &Document, name: &str) -> Vec3 {
        d.parsed().expect("a rigged body").globals[bone(d, name)]
            .w_axis
            .truncate()
    }
    fn drag(d: &mut Document, name: &str, delta: Vec3) {
        let i = bone(d, name);
        let globals = d.parsed().expect("a rigged body").globals.clone();
        d.reposition_bone(i, &globals, delta);
    }

    let mut doc = synthetic_rigged_doc("infer_markers");
    doc.mirror_joints = false; // one joint at a time, so the flags read plainly
                               // INFER'S OWN CONTRACT is the NO-MATCH case: the joints neither the human nor the shape
                               // matcher has spoken for. The ball fixture's own fit matches its trunk (the ball IS a core),
                               // and a matched module is held exactly like a placed joint — which is gated in
                               // `the_rail_is_the_matchers_unmatched_walk`, not here. Clearing the match leaves this gate
                               // measuring the spine fill it has always measured.
    clear_shape_match(&mut doc);
    assert!(
        !doc.placed().iter().any(|p| *p),
        "a freshly composed rig has nothing placed"
    );

    // THE HUMAN places the two ENDS of the spine chain, through the bench's own drag path.
    drag(&mut doc, "pelvis", Vec3::new(3.0, 0.0, 4.0));
    drag(&mut doc, "spine_03", Vec3::new(-2.0, 0.0, -5.0));
    let placed_names = ["pelvis", "spine_03"];
    assert_eq!(
        doc.placed().iter().filter(|p| **p).count(),
        2,
        "a drag marks exactly the joint it moved"
    );
    for name in placed_names {
        assert!(doc.placed()[bone(&doc, name)], "{name} reads as placed");
    }
    let held: Vec<Vec3> = placed_names.iter().map(|n| world(&doc, n)).collect();

    // INFER.
    let n = doc.infer_markers();
    assert!(n > 0, "the rest of the skeleton is derived");
    assert_eq!(
        doc.placed().iter().filter(|p| **p).count(),
        2,
        "Infer never marks anything placed"
    );
    for (name, was) in placed_names.iter().zip(&held) {
        assert!(
            (world(&doc, name) - *was).length() < 1e-3,
            "{name} is where the human put it: {:?} vs {was:?}",
            world(&doc, name)
        );
    }

    // THE INTERIOR of the run lands between the two ends, in order, and on the body's own axis
    // (the fixture is a ball planted on the plumb line, so its medial column is x = 0).
    let ends = (world(&doc, "pelvis"), world(&doc, "spine_03"));
    let (one, two) = (world(&doc, "spine_01"), world(&doc, "spine_02"));
    assert!(
        ends.0.z < one.z && one.z < two.z && two.z < ends.1.z,
        "the chain runs in order from {:?} through {one:?} and {two:?} to {:?}",
        ends.0,
        ends.1
    );
    let cell = 2.0 * doc.parsed().expect("a rigged body").radius / 128.0;
    for p in [one, two] {
        assert!(
            p.x.abs() < 3.0 * cell,
            "the inferred joint sits on the body's medial column: {p:?} (cell {cell:.2})"
        );
    }

    // A SECOND INFER after NUDGING one end re-derives ONLY the unplaced joints. The end is pulled
    // DOWN past where the interior was left, so a stale answer cannot pass: the interior has to
    // come down with it, while both placed joints stand exactly where the human's hand left them.
    let before = world(&doc, "spine_02");
    drag(&mut doc, "spine_03", Vec3::new(0.0, 0.0, -12.0));
    let moved_end = world(&doc, "spine_03");
    assert!(
        moved_end.z < before.z,
        "the nudge really does put the end below the stale interior"
    );
    assert_eq!(doc.infer_markers(), n, "the same joints are derived again");
    assert!(
        (world(&doc, "spine_03") - moved_end).length() < 1e-3,
        "the nudge stands: {:?} vs {moved_end:?}",
        world(&doc, "spine_03")
    );
    assert!(
        (world(&doc, "pelvis") - held[0]).length() < 1e-3,
        "the end that was not touched did not move either"
    );
    let (one, two) = (world(&doc, "spine_01"), world(&doc, "spine_02"));
    assert!(
        two.z < before.z - 1.0,
        "the unplaced interior was re-derived against the NEW end: {two:?} vs {before:?}"
    );
    assert!(
        held[0].z < one.z && one.z < two.z && two.z < moved_end.z,
        "and the chain still runs in order: {:?} {one:?} {two:?} {moved_end:?}",
        held[0]
    );
}

/// MIRROR FROM (697DEC55, the ratified order 42AB9BA8) is a SOURCE-SHAPE knob: the document value
/// is applied in Prep — after the facing turn, so world X = 0 really is the median plane, and
/// BEFORE the skeleton is fitted — so the mesh the Rig step works on comes out symmetric. Off puts
/// the lopsided original back rather than leaving a half-mirrored body behind. (`mirror_mesh`'s own
/// behaviour is gated in flicker-content; this is that the bench reaches it.)
#[test]
fn the_prep_mirror_makes_a_lopsided_source_symmetric() {
    let scratch = synth_source_dir("prep_mirror");
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Skin);
    doc.open(scratch);
    {
        let src = doc.source.as_mut().expect("the scratch folder opened");
        let mut m = sphere_mesh(6, 8, 50.0);
        for v in &mut m.vertices {
            if v.p[0] > 0.0 {
                v.p[0] *= 2.0; // the body's LEFT (+X) twice as wide as its right
            }
        }
        src.parsed = Some(Parsed::new(m));
        src.error = None;
    }
    doc.ensure_prep_source();
    // The stretched sphere is widest in X, so the facing default suggests a quarter turn; this
    // gate is about the mirror, and the mirror's plane is the UNTURNED body's.
    doc.facing_quarters = 0;
    doc.rebuild_prepped_model();
    // SYMMETRY is the question, and the BOUNDS cannot answer it: the prep re-centres the mesh on
    // X, so a lopsided body already spans evenly. Every vertex must have a partner at its own
    // reflection — that is what the skeleton's twin-joint assumptions need.
    let unpaired = |d: &Document| -> usize {
        let verts: Vec<Vec3> = d
            .parsed()
            .expect("a prepped mesh")
            .model
            .vertices
            .iter()
            .map(|v| Vec3::from_array(v.p))
            .collect();
        verts
            .iter()
            .filter(|p| {
                let want = Vec3::new(-p.x, p.y, p.z);
                !verts.iter().any(|q| (*q - want).length() < 1e-3)
            })
            .count()
    };
    assert!(unpaired(&doc) > 0, "the fixture really is lopsided");

    // KEEP THE NARROW half (the body's right, −X) and reflect it.
    assert!(doc.set_mirror_keep(Some(Side::Right)), "the knob changed");
    assert_eq!(
        unpaired(&doc),
        0,
        "every vertex of the mirrored body has a partner at its own reflection"
    );

    // OFF is a source-shape change too: the lopsided original comes back.
    assert!(doc.set_mirror_keep(None), "the knob changed back");
    assert!(unpaired(&doc) > 0, "Off restores the source shape");
}

/// FACE FORWARD (164AE2F3) runs inside the ONE bake path, right after the stance normaliser, and
/// BY DEFAULT (A79A6131): a head bound TURNED off the canon forward (−Y) is un-turned in
/// `character_bake_model` with the Prep box as it opens, so the Preview page plays and Commit
/// writes the same un-turned head. Unticked — the opt-out — the turn stands: the control is what
/// decides, not the mesh.
#[test]
fn the_bake_path_faces_a_yawed_head_forward() {
    let mut doc = synthetic_rigged_doc("face_forward");
    doc.mirror_joints = false; // yaw ONE face, not a symmetric pair
    let idx = |d: &Document, name: &str| {
        d.parsed()
            .expect("a rigged body")
            .model
            .bones
            .iter()
            .position(|b| b.name == name)
            .unwrap_or_else(|| panic!("the canon carries `{name}`"))
    };
    // TURN THE HEAD 45° to the body's left about the head joint — the birds' own pose, built out
    // of the bench's own reposition path so the bind is re-derived exactly as a hand drag leaves it.
    let head = idx(&doc, "head");
    let pivot = doc.parsed().expect("a rigged body").globals[head]
        .w_axis
        .truncate();
    let yaw = glam::Quat::from_rotation_z(45f32.to_radians());
    for name in ["jaw", "eye_l", "eye_r"] {
        let i = idx(&doc, name);
        let globals = doc.parsed().expect("a rigged body").globals.clone();
        let now = globals[i].w_axis.truncate();
        doc.reposition_bone(i, &globals, (pivot + yaw * (now - pivot)) - now);
    }
    // Where the face points, out of a baked model: the eyes' midpoint against the head joint.
    let facing = |m: &RawModel| -> (Vec3, Vec3) {
        let (g, _) = rest_globals(m, &[]);
        let at = |name: &str| {
            let i = m
                .bones
                .iter()
                .position(|b| b.name == name)
                .unwrap_or_else(|| panic!("the bake carries `{name}`"));
            g[i].w_axis.truncate()
        };
        ((at("eye_l") + at("eye_r")) * 0.5, at("head"))
    };
    let (eyes, head_at) = facing(&doc.parsed().expect("a rigged body").model);
    assert!(
        (eyes.x - head_at.x).abs() > 1.0,
        "the fixture's face really is turned off the midline: {eyes:?} vs {head_at:?}"
    );

    // AS IT OPENS: the bake un-turns it — the face lands on the midline, AHEAD of the head joint.
    assert!(doc.face_forward, "the Prep box opens ticked");
    let baked = doc.character_bake_model().expect("the bake path runs");
    let (eyes, head_at) = facing(&baked);
    assert!(
        (eyes.x - head_at.x).abs() < 0.1,
        "the un-turned face is on the midline: {eyes:?} vs {head_at:?}"
    );
    assert!(
        eyes.y < head_at.y,
        "and faces the canon forward (−Y): {eyes:?} vs {head_at:?}"
    );

    // OPTED OUT: the same path leaves the turn exactly as bound.
    doc.face_forward = false;
    let posed = doc.character_bake_model().expect("the bake path runs");
    let (eyes, head_at) = facing(&posed);
    assert!(
        (eyes.x - head_at.x).abs() > 1.0,
        "with the box unticked the head is left as posed: {eyes:?}"
    );
}

pub(crate) fn sphere_mesh(rings: usize, segments: usize, radius: f32) -> RawModel {
    let point = |i: usize, j: usize| -> [f32; 3] {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        let phi = std::f32::consts::TAU * j as f32 / segments as f32;
        [
            radius * theta.sin() * phi.cos(),
            radius * theta.sin() * phi.sin(),
            radius * theta.cos(),
        ]
    };
    let top = [0.0, 0.0, radius];
    let bottom = [0.0, 0.0, -radius];
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    for j in 0..segments {
        let j1 = (j + 1) % segments;
        tris.push([top, point(1, j), point(1, j1)]);
        tris.push([bottom, point(rings - 1, j1), point(rings - 1, j)]);
    }
    for i in 1..rings - 1 {
        for j in 0..segments {
            let j1 = (j + 1) % segments;
            let (a, b, c, d) = (point(i, j), point(i, j1), point(i + 1, j), point(i + 1, j1));
            tris.push([a, c, d]);
            tris.push([a, d, b]);
        }
    }
    let vertices: Vec<RawVertex> = tris
        .iter()
        .flatten()
        .map(|p| {
            let n = Vec3::from_array(*p).normalize_or_zero().to_array();
            RawVertex {
                p: *p,
                n,
                uv: [0.0, 0.0],
                joints: [0; 4],
                weights: [0.0; 4],
            }
        })
        .collect();
    let indices = (0..vertices.len() as u32).collect();
    RawModel {
        regions: Vec::new(),
        vertices,
        indices,
        bones: Vec::new(),
    }
}

/// PREP on a raw (boneless) mesh: the pristine source is cached once at 100%, APPLY collapses
/// it to the typed triangle count (clamped to the source) and RESET returns it, the working
/// mesh is always stature-scaled, and every geometry change drops the rig so it re-installs
/// on Conform. Headless: an empty scratch folder scans, and the raw mesh stands in for the
/// parse — the cache, the cut and the rescale are the services under test.
#[test]
fn prep_decimates_and_resets_against_the_cached_source() {
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_prep");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Skin);
    doc.open(scratch.clone());
    assert!(doc.prep_status().is_empty(), "no readout before a parse");

    let mesh = sphere_mesh(6, 8, 50.0);
    let source_tris = mesh.indices.len() / 3;
    {
        let src = doc.source.as_mut().unwrap();
        src.parsed = Some(Parsed::new(mesh));
        src.error = None;
    }
    let height = |doc: &Document| {
        let p = doc.parsed().expect("the raw mesh stands in for the parse");
        let (lo, hi) = p
            .model
            .vertices
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
                (lo.min(v.p[2]), hi.max(v.p[2]))
            });
        hi - lo
    };

    // Entering Prep caches the pristine source at 100% and conditions the working mesh.
    assert!(doc.prep.is_none());
    doc.ensure_prep_source();
    let cache = doc
        .prep
        .as_ref()
        .expect("a boneless mesh caches its source");
    assert_eq!(cache.source_tris, source_tris);
    assert_eq!(
        doc.decimate_target,
        source_tris.to_string(),
        "the field reads the source count (100%)"
    );
    assert_eq!(doc.tri_count(), Some(source_tris));
    assert!(
        doc.prep_status()
            .starts_with(&format!("{source_tris} / {source_tris}")),
        "got {}",
        doc.prep_status()
    );
    assert!(
        (height(&doc) - doc.stature_cm).abs() < 1e-2,
        "the working mesh is scaled to the target stature"
    );
    let mesh_gen = doc.mesh_gen;
    doc.ensure_prep_source();
    assert_eq!(doc.mesh_gen, mesh_gen, "re-entering Prep is a no-op");

    // APPLY a deeper target: the source collapses and the field re-reads the applied target.
    let target = source_tris / 2;
    doc.decimate_target = target.to_string();
    assert!(doc.apply_decimate_target(), "a new target applies");
    assert_ne!(doc.mesh_gen, mesh_gen, "the working geometry changed");
    let cut = doc.tri_count().unwrap();
    assert!(
        cut < source_tris,
        "the mesh lost triangles: {cut} of {source_tris}"
    );
    assert_eq!(doc.decimate_target, target.to_string());
    assert!(
        !doc.apply_decimate_target(),
        "re-applying the same target is a no-op"
    );
    assert!(
        doc.prep_status()
            .starts_with(&format!("{cut} / {source_tris}")),
        "got {}",
        doc.prep_status()
    );

    // A clamped entry: above the source reads back as the source, verbatim.
    doc.decimate_target = "999999".into();
    assert!(doc.apply_decimate_target());
    assert_eq!(doc.decimate_target, source_tris.to_string());
    assert_eq!(doc.tri_count(), Some(source_tris));

    // RESET: back to 100%, and a no-op once there.
    doc.decimate_target = target.to_string();
    assert!(doc.apply_decimate_target());
    assert!(doc.reset_decimate_target(), "reset restores the source");
    assert_eq!(doc.tri_count(), Some(source_tris));
    assert_eq!(doc.decimate_target, source_tris.to_string());
    assert!(!doc.reset_decimate_target(), "and is a no-op at 100%");

    // The height slider: a new stature rescales the prepped mesh without re-cutting it, and
    // any rig re-installs on Conform (the geometry it was bound to is gone).
    doc.stature_cm = 100.0;
    doc.rebuild_prepped_model();
    assert!((height(&doc) - 100.0).abs() < 1e-2, "got {}", height(&doc));
    assert_eq!(doc.tri_count(), Some(source_tris));
    assert!(doc.source.as_ref().unwrap().rig.is_none());

    // The Rig step installs the authored canon on the PREPPED mesh — and only now (`open`
    // deferred it): the canonical skeleton at the target stature, skinned, every row Ok.
    doc.conform();
    assert_eq!(
        doc.bone_count(),
        Some(CONFORMED_BONES),
        "the canon is installed, root excluded"
    );
    assert!(
        doc.bone_rows().iter().all(|(_, s)| *s == MapState::Ok),
        "an installed canon has nothing to review"
    );
    assert!(
        (height(&doc) - 100.0).abs() < 1e-2,
        "still at the target stature"
    );
    // Re-entering Prep after that reverts to the boneless prepped mesh so the controls act
    // again; the rig re-installs on the next Conform.
    doc.ensure_prep_source();
    assert_eq!(
        doc.bone_count(),
        Some(0),
        "Prep works the boneless mesh again"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The Prep height readout shows the stature in BOTH units, with the rounded inch carrying
/// into the foot (never "1′12″").
#[test]
fn height_readout_shows_metric_and_imperial() {
    assert_eq!(Document::height_readout(170.0), "170 cm · 5′7″");
    assert_eq!(Document::height_readout(182.88), "183 cm · 6′0″");
    assert_eq!(Document::height_readout(60.9), "61 cm · 2′0″");
}

/// The real Motifect BVH library — the animation workflow's genuine input. Skips
/// when the content tree (the clips or the reference rig) is absent, like every
/// other real-data test here.
fn real_bvh_source() -> Option<PathBuf> {
    let dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/source/characters/Motifect/Motifect_combat_complete_v1_0/BVH"
    ));
    let reference = default_reference();
    let have_ref = reference.exists() || reference.with_extension("json.gz").exists();
    (dir.exists() && have_ref).then_some(dir)
}

/// ANIMATION IMPORT, end to end on the real library: the Task card's class routes to
/// `import_animation`, the folder's BVH files fill the SAME picker meshes use, the
/// stage runner retargets the active clip IN MEMORY, the summary reports it, and
/// Commit honours the side-by-side pick — writing exactly the chosen variants.
#[test]
fn an_animation_walks_retarget_preview_and_commits_the_picked_variants() {
    let Some(dir) = real_bvh_source() else {
        eprintln!("skipping: no content tree");
        return;
    };
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Animation);
    doc.open(dir);
    assert_eq!(
        doc.workflow, WF_ANIMATION,
        "an animation dispatches the animation workflow"
    );
    assert_eq!(doc.workflow, WF_ANIMATION);
    {
        let src = doc.source.as_ref().unwrap();
        assert!(
            src.candidates.len() > 1,
            "the combat library offers a clip choice"
        );
        assert!(
            src.candidates
                .iter()
                .all(|p| p.extension().is_some_and(|e| e == "bvh")),
            "candidates are the folder's BVH clips"
        );
        assert!(
            src.error.is_none(),
            "no invented 'no riggable mesh': {:?}",
            src.error
        );
        assert!(src.clip.is_none(), "retarget waits for the stage runner");
    }
    assert!(
        doc.file_name().is_some_and(|f| f.ends_with(".bvh")),
        "the active pick is a clip"
    );
    assert!(
        doc.clip_summary().is_none(),
        "nothing to summarise before the retarget"
    );

    // The conform-step runner (the scene calls this beside analyze/conform).
    doc.prepare_clip();
    {
        let src = doc.source.as_ref().unwrap();
        let cp = src
            .clip
            .as_ref()
            .unwrap_or_else(|| panic!("retarget: {:?}", src.error));
        assert!(cp.duration > 0, "a real clip has length");
        assert!(
            !cp.ip.tracks.is_empty() && !cp.rm.tracks.is_empty(),
            "both variants resolve"
        );
        assert_eq!(cp.bones.len(), cp.parents.len());
        assert!(
            cp.rm_radius >= cp.radius,
            "the RootMotion frame is never tighter than rest"
        );
    }
    let summary = doc.clip_summary().expect("the retargeted clip summarises");
    assert!(
        summary.contains(doc.file_name().unwrap()),
        "the summary names the clip: {summary}"
    );
    assert!(
        summary.matches("[x]").count() == 2,
        "both variants are picked by default: {summary}"
    );

    // Nothing picked → an honest refusal, no files.
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_clip_commit");
    let _ = std::fs::remove_dir_all(&scratch);
    doc.variant_ip = false;
    doc.variant_rm = false;
    assert!(
        doc.clip_summary().unwrap().matches("[ ]").count() == 2,
        "the summary reflects the pick"
    );
    doc.commit_to(&scratch);
    {
        let src = doc.source.as_ref().unwrap();
        assert!(
            src.error
                .as_deref()
                .unwrap_or("")
                .contains("at least one variant"),
            "an empty pick refuses: {:?}",
            src.error
        );
        assert!(src.committed.is_none());
    }

    // Root Motion alone → exactly that variant lands, In-Place does not.
    doc.variant_rm = true;
    doc.commit_to(&scratch);
    let src = doc.source.as_ref().unwrap();
    assert!(
        src.error.is_none(),
        "the picked commit succeeds: {:?}",
        src.error
    );
    let set = scratch.join(src.asset_name());
    assert!(
        set.join("RootMotion").is_dir(),
        "the picked variant is written"
    );
    assert!(!set.join("In-Place").exists(), "the unpicked one is NOT");
    let out = src.committed.clone().expect("a committed path is recorded");
    let text = flicker_content::package::read_text(&out).expect("the emitted clip reads back");
    assert!(
        text.contains("\"retarget\":true"),
        "a clip ships retarget:true"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// THE SILENT-COMMIT REGRESSION (QA 2026-08-03: "doesn't always end up producing an
/// object in the staging folder"). A prop folder whose mesh never parsed must refuse
/// Export OUT LOUD if commit is ever reached, instead of returning silently with no file
/// and no message.
#[test]
fn an_unparsed_prop_refuses_export_loudly() {
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_unparsed_prop");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Prop);
    doc.pending_prop = Some(PropKind::Environment);
    doc.open(scratch.clone());
    assert_eq!(doc.workflow, WF_PROP, "the empty folder still dispatches");
    assert!(
        doc.source.as_ref().unwrap().parsed.is_none(),
        "no parse → no mount binding"
    );

    let out = std::env::temp_dir().join("flicker_assetpipeline_unparsed_prop_out");
    let _ = std::fs::remove_dir_all(&out);
    doc.commit_to(&out);
    assert!(
        doc.error().unwrap_or("").contains("nothing to commit"),
        "the refusal is SAID, not silent: {:?}",
        doc.error()
    );
    assert!(
        !doc.has_committed() && !out.exists(),
        "and nothing was written"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Every workflow's commit lands in a STAGING tier, routed by what the asset IS:
/// clips → the shared retarget library, environment props → props/, characters and
/// worn things → characters/. The Quartermaster's promote pass is the only door
/// into package/ — the one tree the engine loads content from.
#[test]
fn commit_roots_route_by_class_and_all_land_in_staging() {
    let case = |class: Option<AssetClass>, prop: Option<PropKind>, suffix: &str| {
        let scratch = std::env::temp_dir().join(format!(
            "flicker_assetpipeline_root_{}",
            suffix.replace('/', "_")
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let mut doc = Document::new();
        doc.pending_class = class;
        doc.pending_prop = prop;
        doc.open(scratch.clone());
        let root = doc.commit_root();
        assert!(
            root.ends_with(suffix),
            "{class:?}/{prop:?} routes to …/{suffix}, got {}",
            root.display()
        );
        assert!(
            root.strip_prefix(flicker_content::roots().staging())
                .is_ok(),
            "every commit root is inside staging/: {}",
            root.display()
        );
        let _ = std::fs::remove_dir_all(&scratch);
    };
    case(Some(AssetClass::Animation), None, "staging/retarget/clips");
    case(
        Some(AssetClass::Prop),
        Some(PropKind::Environment),
        "staging/props",
    );
    case(
        Some(AssetClass::Prop),
        Some(PropKind::Clothing),
        "staging/characters",
    );
    case(Some(AssetClass::Skin), None, "staging/characters");
    case(Some(AssetClass::Creature), None, "staging/creatures");
}

/// THE SKELETON PICK (the modular skeleton system, 2026-09-07): the Prep step steps through
/// the shipped presets (Humanoid first, clamped at the ends), and the pick is what Conform
/// composes onto a raw mesh — a digitigrade pick puts the ankle at its heel knob.
#[test]
fn the_prep_skeleton_pick_reaches_the_installed_rig() {
    let scratch = synth_source_dir("skeleton_pick");
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Skin);
    doc.open(scratch);
    {
        let src = doc.source.as_mut().expect("the scratch folder opened");
        src.parsed = Some(Parsed::new(sphere_mesh(6, 8, 50.0)));
        src.error = None;
    }
    doc.ensure_prep_source();
    assert_eq!(doc.skeleton_name(), "Humanoid");
    assert!(!doc.step_preset(-1), "the rail clamps at the first preset");
    assert!(
        doc.skeleton_summary().contains("67 bones"),
        "{}",
        doc.skeleton_summary()
    );
    let digitigrade = doc.presets.iter().position(|p| p.name == "Digitigrade");
    let Some(want) = digitigrade else {
        eprintln!("skipping the digitigrade pick: no shipped presets");
        return;
    };
    while doc.preset < want {
        assert!(doc.step_preset(1), "the rail steps forward");
    }
    assert_eq!(doc.skeleton_name(), "Digitigrade");
    assert!(
        doc.skeleton_summary().contains("digitigrade"),
        "{}",
        doc.skeleton_summary()
    );
    doc.conform();
    assert!(doc.error().is_none(), "{:?}", doc.error());
    let m = &doc.parsed().unwrap().model;
    assert_eq!(m.bones.len(), 66, "the same 66 bones as the humanoid");
    let (globals, _) = rest_globals(m, &[]);
    let foot = m.bones.iter().position(|b| b.name == "foot_l").unwrap();
    let pelvis = m.bones.iter().position(|b| b.name == "pelvis").unwrap();
    // THE PICK DECIDES THE KNOB, THE FIT DECIDES WHERE THE BODY STANDS. The digitigrade heel is
    // authored at 0.15 of the stature, which puts the ankle 0.560 − 0.15 of it below the pelvis.
    // Since the shape graph landed (spec 04803E0C) the fit MOVES the whole composed rest onto
    // whatever shape the mesh has — here a bare sphere, whose own bottom end is where the pelvis
    // is stood over — so the absolute height is the fit's business and the DROP is the pick's.
    let drop = globals[pelvis].w_axis.z - globals[foot].w_axis.z;
    assert!(
        (drop - (0.560 - 0.15) * doc.stature_cm).abs() < 0.5,
        "the digitigrade ankle rides at its heel knob, {} of the stature under the pelvis, got {}",
        0.560 - 0.15,
        drop / doc.stature_cm
    );
}

/// THE CREATURE WORKFLOW (Aaron 2026-09-07: "we do not have a quadruped rig" → 2026-09-08: the
/// modular skeleton has one): a raw body declared a Creature gets the Prep cache like a
/// character, opens on the shipped QUADRUPED pick, and Conform installs that recipe — hooves,
/// forehooves, the trunk along the back — for the Rig step to place by hand.
#[test]
fn a_creature_preps_and_rigs_on_the_quadruped_pick() {
    let scratch = synth_source_dir("creature_prep");
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Creature);
    doc.open(scratch);
    {
        let src = doc.source.as_mut().expect("the scratch folder opened");
        src.parsed = Some(Parsed::new(sphere_mesh(6, 8, 50.0)));
        src.error = None;
    }
    assert_eq!(doc.class(), Some(AssetClass::Creature));
    assert_eq!(doc.workflow, WF_CREATURE, "the creature rail is dispatched");
    assert_eq!(
        doc.skeleton_name(),
        "Quadruped",
        "a creature opens on the quadruped pick"
    );
    assert_eq!(doc.recipe().pattern(), Pattern::Quadruped);
    doc.ensure_prep_source();
    let cache = doc
        .prep
        .as_ref()
        .expect("a raw creature gets the Prep cache");
    assert!(cache.source_tris > 0, "the cache holds the pristine source");
    doc.conform();
    assert!(
        doc.error().is_none(),
        "the quadruped installs cleanly, got {:?}",
        doc.error()
    );
    assert_eq!(
        doc.bone_count().unwrap_or(0) + 1,
        doc.recipe_bones(),
        "the composed quadruped is installed"
    );
    let p = doc.parsed().unwrap();
    for name in ["hoof_l", "forehoof_r", "foredigit_l", "tail_06"] {
        assert!(p.bone_index(name).is_some(), "{name} is installed");
    }
    assert!(
        p.bone_index("middle_01_l").is_none(),
        "no fingers on a foreleg"
    );
}

/// A creature WITHOUT a rig (Conform never ran) commits as a static bake — empty skeleton,
/// `retarget:false`, no attach — into its own staging tier, so it can be looked at now
/// (real content; skips without it).
#[test]
fn commit_routes_an_unrigged_creature_to_the_static_bake() {
    let Some(mut doc) = parsed() else {
        eprintln!("skipping: no content tree");
        return;
    };
    doc.source.as_mut().unwrap().class = Some(AssetClass::Creature);
    assert!(
        doc.commit_root().ends_with("staging/creatures"),
        "a creature lands in its own tier, got {}",
        doc.commit_root().display()
    );
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_creature_commit");
    let _ = std::fs::remove_dir_all(&scratch);
    doc.commit_to(&scratch);
    let src = doc.source.as_ref().unwrap();
    assert!(
        src.error.is_none(),
        "the creature commit succeeds: {:?}",
        src.error
    );
    let out = src.committed.clone().expect("a committed path is recorded");
    let text = flicker_content::package::read_text(&out).expect("the creature rig was written");
    assert!(
        text.contains("\"bones\":[]"),
        "an unrigged creature ships no skeleton"
    );
    assert!(
        text.contains("\"retarget\":false"),
        "and plays no retargeted clips"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A RIGGED creature commits a rig — its quadruped recipe inside — into its tier, previews AT
/// REST (no quadruped gait exists to play), and re-opens from that tier with its recipe.
#[test]
fn a_rigged_creature_commits_previews_at_rest_and_re_opens() {
    let root = std::env::temp_dir().join("flicker_assetpipeline_creature_rigged");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let scratch = synth_source_dir("creature_rigged");
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Creature);
    doc.open(scratch.clone());
    {
        let src = doc.source.as_mut().expect("the scratch folder opened");
        src.parsed = Some(Parsed::new(sphere_mesh(6, 8, 50.0)));
        src.error = None;
    }
    doc.ensure_prep_source();
    doc.conform();
    assert!(doc.bone_count().unwrap_or(0) > 0, "rigged");
    let (_rig, bones, clip) = doc.bake_preview_parts().expect("the preview bakes");
    assert_eq!(clip.name, "rest", "a quadruped previews at rest");
    assert!(clip.tracks.is_empty() && clip.duration_ticks > 0);
    assert!(bones.iter().any(|b| b.name == "hoof_l"));

    doc.commit_to(&root);
    let src = doc.source.as_ref().unwrap();
    assert!(
        src.error.is_none(),
        "the rigged creature commits: {:?}",
        src.error
    );
    let name = src.asset_name().to_string();
    let path = root.join(&name).join(format!("{name}.json"));
    let staged = flicker_content::load_rig_raw(&path).expect("the commit reloads");
    assert!(
        staged.bones.iter().any(|b| b.name == "forehoof_l"),
        "the rig was baked"
    );
    let json: serde_json::Value =
        serde_json::from_str(&flicker_content::package::read_text(&path).unwrap()).unwrap();
    assert_eq!(
        json["skeleton_recipe"]["trunk"]["orientation"], "Quadruped",
        "the recipe rides in the rig"
    );

    let mut again = Document::new();
    again.pending_class = Some(AssetClass::Creature);
    again.prefer_staged = true;
    again.open(scratch);
    {
        let src = again.source.as_mut().expect("re-opened");
        src.parsed = Some(Parsed::new(sphere_mesh(6, 8, 50.0)));
        src.error = None;
    }
    assert!(
        again.adopt_staged_from(&root, "staging"),
        "a creature re-opens from its tier"
    );
    assert_eq!(again.recipe().pattern(), Pattern::Quadruped);
    assert_eq!(again.bone_count().unwrap_or(0), staged.bones.len());
    let _ = std::fs::remove_dir_all(&root);
}

/// An animation folder WITHOUT clips reports the real absence — never the mesh
/// path's "no riggable mesh", which would send the user hunting the wrong problem.
#[test]
fn an_animation_folder_without_bvh_reports_the_real_absence() {
    let scratch = std::env::temp_dir().join("flicker_assetpipeline_no_bvh");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let mut doc = Document::new();
    doc.pending_class = Some(AssetClass::Animation);
    doc.open(scratch.clone());
    assert!(
        doc.error().unwrap_or("").contains("No BVH"),
        "the error names the real absence: {:?}",
        doc.error()
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Rest frames compose parent→child, and a root bone's world frame IS its local one.
#[test]
fn rest_globals_compose_down_the_chain() {
    let bone = |name: &str, parent: i32, t: [f32; 3]| flicker_content::RawBone {
        name: name.into(),
        parent,
        translation: t,
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        inverse_bind: Mat4::IDENTITY.to_cols_array(),
    };
    let model = RawModel {
        regions: Vec::new(),
        vertices: Vec::new(),
        indices: Vec::new(),
        bones: vec![
            bone("root", -1, [0.0, 0.0, 10.0]),
            bone("child", 0, [0.0, 0.0, 5.0]),
            bone("grandchild", 1, [0.0, 0.0, 2.0]),
        ],
    };
    let (globals, parents) = rest_globals(&model, &[]);
    assert_eq!(parents, vec![-1, 0, 1]);
    assert_eq!(globals[0].w_axis.truncate(), Vec3::new(0.0, 0.0, 10.0));
    assert_eq!(globals[1].w_axis.truncate(), Vec3::new(0.0, 0.0, 15.0));
    assert_eq!(globals[2].w_axis.truncate(), Vec3::new(0.0, 0.0, 17.0));
    // The views frame about the asset's CENTRE, not the origin — in Z-up ground reckoning the
    // origin is its feet, so framing there put the body out of shot.
    let (centre, radius, floor, _) = model_bounds(&model, &globals);
    assert_eq!(
        centre,
        Vec3::new(0.0, 0.0, 13.5),
        "midway between the root and the tip"
    );
    assert_eq!(radius, 3.5, "half the 10 → 17 span");
    // The floor is the feet plane AFTER the same `-centre` shift the viewport draws through,
    // so it is negative and lands exactly on the lowest bone — draw the stage grid at the
    // asset's soles, not at the origin (which recentring puts at its waist).
    assert_eq!(floor, -3.5, "lowest extent (z=10) recentred about 13.5");
    assert!(floor < 0.0, "a recentred floor is always below the origin");
}

/// With nothing open there is nothing to bake and nothing to say: the live-tree commit
/// touches no disk and reports no error (contrast the unparsed-prop refusal above, which
/// has a source to report against).
#[test]
fn commit_with_nothing_open_touches_nothing() {
    let mut doc = Document::new();
    doc.commit();
    assert!(!doc.has_committed() && doc.error().is_none());
}

/// The Rig step's entry runs the conform on a parsed-but-unrigged model and is a no-op on a
/// rigged one — so re-entering the step, or reaching it after a piece pick dropped the rig,
/// always lands on a bone map and never re-runs the derive passes over an existing one.
#[test]
fn conform_runs_once_on_the_rig_step() {
    let Some(mut doc) = parsed() else {
        eprintln!("skipping: no content tree");
        return;
    };
    assert!(doc.bone_rows().is_empty(), "no map without a rig");
    doc.conform();
    assert_eq!(
        doc.bone_count(),
        Some(CONFORMED_BONES),
        "the Rig step's conform reaches the canonical count"
    );
    let rows = doc.bone_rows();
    assert_eq!(rows.len(), CONFORMED_BONES, "one row per bone");
    let sel = doc.bone_sel();
    doc.conform();
    assert_eq!(doc.bone_rows(), rows, "a second entry changes nothing");
    assert_eq!(doc.bone_sel(), sel);
}

/// THE ATTACH-TABLE GATE: the bench's rail (ids, labels, parents) and the library's
/// `DEFAULT_MOUNTS` (what a headless import ships) must name the same six points on the same
/// bones, in the same order — a character baked either way mounts the same weapons.
#[test]
fn the_attach_rail_matches_the_librarys_default_mounts() {
    assert_eq!(ATTACH_POINTS.len(), flicker_content::DEFAULT_MOUNTS.len());
    for ((id, _, parent), (lid, lbone)) in ATTACH_POINTS.iter().zip(flicker_content::DEFAULT_MOUNTS)
    {
        assert_eq!(*id, lid, "attach point id order");
        assert_eq!(*parent, lbone, "attach point {id} parent bone");
    }
}

/// THE REVIEW PAGE SAYS WHAT COMMIT DID (Aaron 2026-09-07: "it is unclear if anything
/// happens, some kind of message needs to be provided"): nothing before an attempt, the
/// written folder and its baked bone count after one, and the reason when it wrote nothing.
#[test]
fn the_commit_note_reports_where_the_rig_went_or_why_not() {
    let mut doc = synthetic_rigged_doc("commit_note");
    assert_eq!(doc.commit_note(), "", "nothing to say before a commit");
    let root = std::env::temp_dir().join("flicker_assetpipeline_commit_note");
    let _ = std::fs::remove_dir_all(&root);
    doc.commit_to(&root);
    let note = doc.commit_note();
    let bones = doc.parsed().unwrap().bones() + 1;
    assert!(
        note.contains(&format!("{bones} ")) && note.contains("→"),
        "the note names the destination and the baked bones: {note:?}"
    );
    assert!(doc.has_committed(), "and the commit happened");
    let _ = std::fs::remove_dir_all(&root);

    // A commit that cannot bake says so, in the same line.
    let mut doc = Document::new();
    doc.commit_to(&std::env::temp_dir());
    assert_eq!(doc.commit_note(), "", "no source, no attempt recorded");
}

/// RE-OPENING A STAGED RIG BRINGS ITS PREP BACK (Aaron 2026-09-07: "This should also load it at
/// the decimated quality and size I had previously set"): the staged body's height and
/// triangle count land in the Prep fields, so the bench shows what it was committed at.
#[test]
fn re_opening_a_staged_rig_restores_its_height_and_decimation() {
    let root = std::env::temp_dir().join("flicker_assetpipeline_staging_for_restore_prep");
    let _ = std::fs::remove_dir_all(&root);
    let mut first = synthetic_rigged_doc("restore_prep");
    let (lo, hi) = first
        .parsed()
        .unwrap()
        .model
        .vertices
        .iter()
        .map(|v| v.p[2])
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), z| {
            (lo.min(z), hi.max(z))
        });
    let tris = first.parsed().unwrap().tris;
    first.commit_to(&root);
    assert!(first.has_committed(), "the fixture commits");

    // A second document on the SAME source folder (its name is the asset name the commit
    // wrote under); the commit root is elsewhere, so re-creating the fixture cannot wipe it.
    let mut again = synthetic_rigged_doc("restore_prep");
    again.stature_cm = 1.0;
    again.decimate_target = "7".into();
    assert!(
        again.adopt_staged_from(&root, "staging"),
        "the staged rig re-opens"
    );
    assert_eq!(
        again.stature_cm,
        (hi - lo).round(),
        "the height is the staged body's"
    );
    assert_eq!(
        again.decimate_target,
        tris.to_string(),
        "the triangle target is the staged body's count"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// THE HEIGHT APPLIES TO A RE-OPENED RIG (Aaron 2026-09-07: "when you change it on the prep
/// screen it never picks it up"): the typed stature resizes mesh AND skeleton together, and
/// re-entering Prep never throws the adopted skeleton away — the cache from an earlier open of
/// the same folder is gone.
#[test]
fn the_height_resizes_a_re_opened_rig_and_prep_keeps_its_skeleton() {
    let root = std::env::temp_dir().join("flicker_assetpipeline_staging_for_height");
    let _ = std::fs::remove_dir_all(&root);
    let mut first = synthetic_rigged_doc("height_apply");
    first.commit_to(&root);
    assert!(first.has_committed());

    let mut again = synthetic_rigged_doc("height_apply");
    // A stale Prep cache from "an earlier open of the same folder": an open must clear it.
    again.ensure_prep_source();
    let dir = again.source.as_ref().unwrap().dir.clone();
    again.prefer_staged = true;
    again.open(dir);
    assert!(again.prep.is_none(), "an open starts Prep clean");
    assert!(
        again.adopt_staged_from(&root, "staging"),
        "the staged rig re-opens"
    );
    let bones = again.parsed().unwrap().bones();
    let before = again.stature_cm;

    // Entering Prep on the re-opened rig keeps its skeleton.
    again.ensure_prep_source();
    assert_eq!(
        again.parsed().unwrap().bones(),
        bones,
        "Prep kept the adopted skeleton"
    );
    assert!(again.source.as_ref().unwrap().rig.is_some(), "…and its rig");

    // APPLY a new height: the body and every joint scale together.
    let pelvis_before = again.parsed().unwrap().globals[0].w_axis.truncate();
    again.stature_cm = before * 2.0;
    assert!(again.apply_stature(), "a rigged body resizes");
    let p = again.parsed().unwrap();
    let (lo, hi) = p
        .model
        .vertices
        .iter()
        .map(|v| v.p[2])
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), z| {
            (lo.min(z), hi.max(z))
        });
    assert!(
        ((hi - lo) - before * 2.0).abs() < 1e-2,
        "the mesh is the new height"
    );
    let pelvis_after = p.globals[0].w_axis.truncate();
    assert!(
        (pelvis_after.z - pelvis_before.z * 2.0).abs() < 1e-2,
        "the skeleton scaled with it: {pelvis_before} → {pelvis_after}"
    );
    assert_eq!(p.bones(), bones, "no bone was lost");
    let _ = std::fs::remove_dir_all(&root);
}

/// A RE-OPENED BAKE KEEPS IDENTITY FRAMES (Aaron 2026-09-07): the frames a composed body's
/// pattern skeleton and clip libraries were baked with. Re-committing a re-opened rig zeroes
/// every bone's rest rotation and moves no joint.
#[test]
fn a_re_opened_rig_re_bakes_with_identity_frames_and_its_joints_in_place() {
    let root = std::env::temp_dir().join("flicker_assetpipeline_staging_for_frames");
    let _ = std::fs::remove_dir_all(&root);
    let mut first = synthetic_rigged_doc("frames");
    first.commit_to(&root);
    assert!(first.has_committed());
    let name = first.asset_name().unwrap().to_string();
    let path = root.join(&name).join(format!("{name}.json"));
    let before = flicker_content::load_rig_raw(&path).expect("the first bake reloads");
    let (world_before, _) = rest_globals(&before, &[]);

    let mut again = synthetic_rigged_doc("frames");
    again.prefer_staged = true;
    assert!(again.adopt_staged_from(&root, "staging"));
    again.commit_to(&root);
    assert!(again.has_committed(), "the re-commit wrote");
    let after = flicker_content::load_rig_raw(&path).expect("the re-bake reloads");
    let (world_after, _) = rest_globals(&after, &[]);
    assert_eq!(before.bones.len(), after.bones.len());
    for (i, b) in after.bones.iter().enumerate() {
        let q = glam::Quat::from_array(b.rotation);
        assert!(
            q.angle_between(glam::Quat::IDENTITY) < 1e-3,
            "{}: identity rest rotation, got {:?}",
            b.name,
            b.rotation
        );
        assert!(
            (world_after[i].w_axis - world_before[i].w_axis).length() < 1e-2,
            "{}: the joint stayed where it was",
            b.name
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The suffix table reads both cases and Blender's dotted form; a centre bone has no side.
#[test]
fn side_of_reads_the_suffix_table() {
    assert_eq!(side_of("thigh_l"), Some(Side::Left));
    assert_eq!(side_of("hand_r"), Some(Side::Right));
    assert_eq!(side_of("Weapon_L"), Some(Side::Left));
    assert_eq!(side_of("Weapon_R"), Some(Side::Right));
    assert_eq!(side_of("upperarm.R"), Some(Side::Right));
    assert_eq!(side_of("spine_01"), None);
    assert_eq!(side_of("root"), None);
    assert_eq!(crate::services::opposite(Side::Left), Side::Right);
}

/// A subtree lists its root first, then every descendant with each parent before its
/// children — the hand and its fingers; a root past the end lists nothing.
#[test]
fn a_subtree_lists_the_root_first_then_every_descendant() {
    let doc = synthetic_rigged_doc("subtree");
    let p = doc.parsed().unwrap();
    let hand = p.bone_index("hand_r").expect("the canon has a right hand");
    let sub = p.subtree(hand);
    assert_eq!(sub[0], hand);
    let index_01 = p.bone_index("index_01_r").unwrap();
    let index_02 = p.bone_index("index_02_r").unwrap();
    assert!(sub.contains(&index_01) && sub.contains(&index_02));
    let (a, b) = (
        sub.iter().position(|&i| i == index_01).unwrap(),
        sub.iter().position(|&i| i == index_02).unwrap(),
    );
    assert!(a < b, "a parent lists before its child");
    for &i in &sub[1..] {
        let parent = usize::try_from(p.parents[i]).unwrap();
        assert!(
            sub.contains(&parent),
            "{}: its parent is in the subtree",
            p.model.bones[i].name
        );
    }
    assert!(
        !sub.contains(&p.bone_index("hand_l").unwrap()),
        "the other hand is not"
    );
    assert!(
        !sub.contains(&p.bone_index("lowerarm_r").unwrap()),
        "nor the bone above"
    );
    assert!(p.subtree(p.parents.len() + 5).is_empty());
}

/// MIRROR → puts every twin of the selected subtree at the reflection of its partner's rest
/// WORLD position across the median plane X = 0 — the partner untouched, an asymmetry the two
/// sides had gone, the count reported — and never mirrors back through the drag's own mirror.
#[test]
fn mirror_to_twin_reflects_the_subtree_across_the_median_plane() {
    let mut doc = synthetic_rigged_doc("mirror_twin");
    doc.mirror_joints = true; // the DRAG's mirror must not leak into the reset
    let p = doc.parsed().unwrap();
    let hand = p.bone_index("hand_r").unwrap();
    let index_01 = p.bone_index("index_01_r").unwrap();
    let hand_l = p.bone_index("hand_l").unwrap();
    let sub = p.subtree(hand);
    let twins = sub.iter().filter(|&&i| doc.mirror_of(i).is_some()).count();
    assert!(twins >= 3, "the hand's subtree has twins to move");
    // Make the two sides asymmetric the way an un-mirrored drag would.
    doc.mirror_joints = false;
    let globals = doc.parsed().unwrap().globals.clone();
    doc.reposition_bone(hand, &globals, Vec3::new(4.0, -3.0, 6.0));
    let globals = doc.parsed().unwrap().globals.clone();
    doc.reposition_bone(index_01, &globals, Vec3::new(1.5, 2.0, -1.0));
    doc.mirror_joints = true;
    let before = doc.parsed().unwrap().globals.clone();
    let right = |g: &[Mat4], i: usize| g[i].w_axis.truncate();
    assert!(
        (right(&before, hand).x + right(&before, hand_l).x).abs() > 1.0,
        "the sides are asymmetric before the reset"
    );
    let gen_before = doc.pose_gen;

    let moved = doc.mirror_to_twin(hand);
    // Two twins were out of place (the hand and the first index link); the rest ride along
    // under them and are already at their reflections when their turn comes.
    assert_eq!(moved, 2, "only the twins that were out of place moved");
    assert!(moved <= twins);
    let after = doc.parsed().unwrap().globals.clone();
    for &i in &sub {
        let Some(t) = doc.mirror_of(i) else { continue };
        let src = right(&after, i);
        let twin = right(&after, t);
        let want = Vec3::new(-src.x, src.y, src.z);
        assert!(
            (twin - want).length() < 1e-2,
            "{}: twin at {twin}, the reflection is {want}",
            doc.parsed().unwrap().model.bones[i].name
        );
        assert!(
            (src - right(&before, i)).length() < 1e-4,
            "{}: the partner stayed put",
            doc.parsed().unwrap().model.bones[i].name
        );
    }
    assert_ne!(doc.pose_gen, gen_before, "a conform edit bumps the pose");
    assert_eq!(
        doc.mirror_to_twin(hand),
        0,
        "already symmetric: nothing moves again"
    );
    // A symmetric centre subtree (the neck and head) has nothing out of place to reflect.
    let neck = doc.parsed().unwrap().bone_index("neck_01").unwrap();
    assert_eq!(doc.mirror_to_twin(neck), 0);
}

/// The reset moves SKELETON only: every moved twin is re-bound to its new rest, so the palette is
/// the identity at rest and the mesh does not move.
#[test]
fn mirror_to_twin_leaves_the_mesh_at_rest() {
    let mut doc = synthetic_rigged_doc("mirror_rest");
    let hand = doc.parsed().unwrap().bone_index("hand_r").unwrap();
    doc.mirror_joints = false;
    let globals = doc.parsed().unwrap().globals.clone();
    doc.reposition_bone(hand, &globals, Vec3::new(5.0, 0.0, 5.0));
    assert!(doc.mirror_to_twin(hand) > 0);
    let p = doc.parsed().unwrap();
    let (rest, _) = rest_globals(&p.model, &[]);
    for (i, b) in p.model.bones.iter().enumerate() {
        let bind = Mat4::from_cols_array(&b.inverse_bind);
        let palette = rest[i] * bind;
        let drift = (palette - Mat4::IDENTITY)
            .to_cols_array()
            .iter()
            .map(|v| v.abs())
            .fold(0.0, f32::max);
        assert!(
            drift < 1e-3,
            "{}: palette drifts from identity by {drift}",
            b.name
        );
    }
}

/// MODULE EDITS COMPOSE THE SKELETON (P2c S4, ruling 0C796096): editing a module on top of the
/// pick changes what the next Conform installs — a heeled leg raises the ankle, a long tail adds
/// its bones, a head off drops the head — the readout says the pick is custom, the pattern
/// follows the legs, and a new pick clears the edits.
#[test]
fn module_edits_compose_the_skeleton_and_a_new_pick_clears_them() {
    let mut doc = synthetic_rigged_doc("modules");
    assert!(doc.recipe_edit.is_none());
    let base = doc.recipe_bones();
    assert_eq!(doc.skeleton_name(), "Humanoid");
    assert!(
        !doc.edit_recipe(|_| {}),
        "an edit that changes nothing reports nothing"
    );
    assert!(doc.edit_recipe(|t| t.legs = vec![LegKind::Digitigrade { heel: 0.15 }]));
    assert_eq!(doc.recipe().pattern(), Pattern::Digitigrade);
    assert_eq!(doc.skeleton_name(), "Humanoid (custom)");
    assert_eq!(doc.recipe_bones(), base, "a heeled leg is the same bones");
    assert!(doc.edit_recipe(|t| t.tails = vec![TailKind::Long { bones: 8 }]));
    assert_eq!(doc.recipe_bones(), base + 8);
    assert!(doc.edit_recipe(|t| t.head = false));
    assert_eq!(
        doc.recipe_bones(),
        base + 8 - 4,
        "head, jaw and two eyes go"
    );
    // The next Conform installs the composed skeleton.
    doc.conform();
    let p = doc.parsed().unwrap();
    assert!(p.bone_index("tail_08").is_some(), "the tail is installed");
    assert!(p.bone_index("head").is_none(), "the head is not");
    let ankle = p.bone_index("foot_l").unwrap();
    assert!(
        p.globals[ankle].w_axis.z > 0.10 * doc.stature_cm,
        "the heel is raised: {}",
        p.globals[ankle].w_axis.z
    );
    // Back to the pick's own trunk clears the edit.
    assert!(doc.edit_recipe(|t| {
        t.legs = vec![LegKind::Plantigrade];
        t.tails.clear();
        t.head = true;
    }));
    assert!(
        doc.recipe_edit.is_none(),
        "landing on the pick clears the edit"
    );
    assert_eq!(doc.skeleton_name(), "Humanoid");
    // A new pick is a new starting point.
    assert!(doc.edit_recipe(|t| t.head = false));
    assert!(doc.step_preset(1), "the shipped presets step");
    assert!(doc.recipe_edit.is_none());
}

/// A COMMITTED RIG CARRIES ITS MODULES AND RE-OPENS WITH THEM: the edited recipe is written into
/// the rig, and a re-open adopts the edit on top of its preset rather than the bare pick.
#[test]
fn a_re_opened_rig_adopts_its_own_modules() {
    let root = std::env::temp_dir().join("flicker_assetpipeline_modules_reopen");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut first = synthetic_rigged_doc("modules_first");
    assert!(first.edit_recipe(|t| {
        t.legs = vec![LegKind::Digitigrade { heel: 0.12 }];
        t.tails = vec![TailKind::ShortHair { bones: 5 }];
    }));
    first.conform();
    first.commit_to(&root);
    assert!(first.has_committed(), "the commit wrote");
    let name = first.asset_name().unwrap().to_string();
    let path = root.join(&name).join(format!("{name}.json"));
    let staged = flicker_content::load_rig_raw(&path).expect("the commit reloads");
    assert!(
        staged.bones.iter().any(|b| b.name == "tail_hair_05"),
        "the hair tail was baked"
    );

    let mut again = synthetic_rigged_doc("modules_first");
    again.prefer_staged = true;
    assert!(again.adopt_staged_from(&root, "staging"));
    let r = again.recipe();
    assert_eq!(r.trunk.legs, vec![LegKind::Digitigrade { heel: 0.12 }]);
    assert_eq!(r.trunk.tails, vec![TailKind::ShortHair { bones: 5 }]);
    assert_eq!(
        r.preset.as_deref(),
        Some("Humanoid"),
        "the starting point is kept"
    );
    assert_eq!(again.skeleton_name(), "Humanoid (custom)");
    assert_eq!(
        again.recipe_bones(),
        staged.bones.len() + 1,
        "the requirements count its bones"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// THE LIMB MODEL READS THE PACKAGED REFERENCES (G1 of the gait/IK design 37704D6B): the
/// quadruped's four limbs come out as two hoofed legs (stifles forward) and two forelegs
/// (elbows back, forehoof effectors), the bird's as two bird legs and two wings, the humanoid's
/// as two legs (ball) and two arms (hand); a two-bone solve plants the quadruped's hoof on a
/// target a hand's breadth forward within a millimetre without moving the hip. Real content;
/// skips without it.
#[test]
fn the_packaged_references_yield_limb_chains_the_solver_plants() {
    use flicker_mechanics::ik::{limbs_of, plant, LimbKind, LimbSide};
    use flicker_skeletal::format::{rig_bones, Pattern, RigFile};
    use flicker_skeletal::pose::global_transforms;
    let load = |pattern: Pattern| {
        let path = flicker_content::baseline::pattern_dir(pattern)
            .join(format!("{}.json", pattern.name()));
        let text = flicker_content::package::read_text(&path).ok()?;
        let file: RigFile = serde_json::from_str(&text).ok()?;
        let bones = rig_bones(&file);
        let locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        let rest = global_transforms(&bones, &locals);
        Some((bones, rest))
    };
    let Some((bones, rest)) = load(Pattern::Quadruped) else {
        eprintln!("skipping: no packaged references");
        return;
    };
    let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
    let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
    let limbs = limbs_of(&names, &rest);
    let legs: Vec<_> = limbs.iter().filter(|l| l.kind == LimbKind::Leg).collect();
    let arms: Vec<_> = limbs.iter().filter(|l| l.kind == LimbKind::Arm).collect();
    assert_eq!((legs.len(), arms.len()), (2, 2), "a quadruped: {limbs:?}");
    for l in &legs {
        assert!(names[l.effector].starts_with("hoof_"));
        assert!(l.pole.y < -0.5, "the stifle bends forward: {}", l.pole);
    }
    for a in &arms {
        assert!(names[a.effector].starts_with("forehoof_"));
        assert!(a.pole.y > 0.5, "the elbow bends back: {}", a.pole);
    }
    // Plant the left hoof a hand's breadth forward on the ground: the effector lands, the hip stays.
    let leg = *legs.iter().find(|l| l.side == LimbSide::Left).unwrap();
    let mut posed = rest.clone();
    let hip = posed[leg.root].w_axis.truncate();
    let target = posed[leg.effector].w_axis.truncate() + Vec3::new(0.0, -10.0, 0.0);
    let miss = plant(&mut posed, &parents, leg, target);
    assert!(
        miss < 0.1,
        "the hoof lands within a millimetre: miss {miss}"
    );
    assert_eq!(posed[leg.root].w_axis.truncate(), hip, "the hip stays");

    if let Some((bones, rest)) = load(Pattern::Bird) {
        let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
        let limbs = limbs_of(&names, &rest);
        assert_eq!(
            limbs.iter().filter(|l| l.kind == LimbKind::BirdLeg).count(),
            2
        );
        let wings: Vec<_> = limbs.iter().filter(|l| l.kind == LimbKind::Wing).collect();
        assert_eq!(wings.len(), 2);
        assert!(wings
            .iter()
            .all(|w| names[w.effector].starts_with("wing_tip_")));
    }
    if let Some((bones, rest)) = load(Pattern::Humanoid) {
        let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
        let limbs = limbs_of(&names, &rest);
        assert_eq!(limbs.len(), 4);
        assert!(limbs
            .iter()
            .filter(|l| l.kind == LimbKind::Leg)
            .all(|l| names[l.effector].starts_with("ball_")));
        assert!(limbs
            .iter()
            .filter(|l| l.kind == LimbKind::Arm)
            .all(|l| names[l.effector].starts_with("hand_")));
    }
}

/// THE GAIT GENERATOR WALKS THE PACKAGED QUADRUPED (G2 of the gait/IK design 37704D6B): two
/// seconds at a metre a second on a flat floor — the trunk advances with the controller, every
/// planted hoof holds its contact to within a millimetre a tick, every landing is on the floor,
/// every hoof reaches its target, and the walk cycles its four feet. Real content; skips
/// without it.
#[test]
fn the_quadruped_reference_walks_without_sliding() {
    use flicker_mechanics::gait::{FlatFloor, GaitKind, Locomotion, LocomotionFamily};
    use flicker_skeletal::format::{rig_bones, Pattern, RigFile};
    use flicker_skeletal::pose::global_transforms;
    let path = flicker_content::baseline::pattern_dir(Pattern::Quadruped).join("Quadruped.json");
    let Ok(text) = flicker_content::package::read_text(&path) else {
        eprintln!("skipping: no packaged quadruped");
        return;
    };
    let file: RigFile = serde_json::from_str(&text).expect("the packaged quadruped parses");
    let bones = rig_bones(&file);
    let locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
    let rest = global_transforms(&bones, &locals);
    let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
    let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
    let mut walker = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
    assert_eq!(walker.feet.len(), 4, "four hooves plant");
    let floor = FlatFloor { height: 0.0 };
    let mut globals = rest.clone();
    let dt = 1.0 / 60.0;
    let mut last: Vec<Option<(Vec3, Vec3)>> = vec![None; 4];
    let mut max_slide = 0.0f32;
    let mut landings = 0;
    for tick in 0..120 {
        let frame = walker.step(&mut globals, &parents, &rest, -Vec3::Y, 100.0, &floor, dt);
        assert_eq!(walker.gait, GaitKind::Walk);
        assert!(
            walker.residual < 0.5,
            "every hoof reaches its target: {} cm at tick {tick}",
            walker.residual
        );
        assert!(frame.origin.y < 0.0, "the trunk advances along −Y");
        for (k, foot) in walker.feet.iter().enumerate() {
            let effector = globals[foot.limb.effector].w_axis.truncate();
            match foot.contact() {
                Some(c) => {
                    assert!(c.point.z.abs() < 1e-3, "landed on the floor: {c:?}");
                    match last[k] {
                        Some((prev_c, prev_e)) if prev_c == c.point => {
                            max_slide = max_slide.max((effector - prev_e).length());
                        }
                        _ => landings += 1,
                    }
                    last[k] = Some((c.point, effector));
                }
                None => last[k] = None,
            }
        }
    }
    assert!(
        max_slide < 0.1,
        "a planted hoof never slides: {max_slide} cm"
    );
    assert!(
        landings >= 8,
        "the walk cycles its feet: {landings} landings"
    );
}

/// THE FLAP ON THE PACKAGED FLIERS (G4 of the gait/IK design 37704D6B): a crow's beat on the
/// Bird reference, two and a half cycles at 60 Hz — both wings are read off the rig with their
/// two feather groups each, the tips rise and fall together (mirrored across the midline to a
/// millimetre), every bone under a shoulder keeps its length, the tip never lies further from
/// the shoulder than the wing reaches, and nothing but the wings moves; the glide is the rest
/// raised by the dihedral. The Bat, when packaged, draws its leading digit's tip toward the
/// wrist at the lagged mid-upstroke and spreads it again through the downstroke. Real content;
/// skips without it.
#[test]
fn a_crows_flap_keeps_the_tip_within_the_wings_reach() {
    use flicker_mechanics::gait::{FlapCycle, Wings};
    use flicker_skeletal::format::{rig_bones, Pattern, RigFile};
    use flicker_skeletal::pose::global_transforms;
    use std::f32::consts::{PI, TAU};
    let load = |pattern: Pattern| {
        let path = flicker_content::baseline::pattern_dir(pattern)
            .join(format!("{}.json", pattern.name()));
        let text = flicker_content::package::read_text(&path).ok()?;
        let file: RigFile = serde_json::from_str(&text).ok()?;
        let bones = rig_bones(&file);
        let locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        let rest = global_transforms(&bones, &locals);
        Some((bones, rest))
    };
    let at = |g: &[Mat4], i: usize| g[i].w_axis.truncate();
    // Whether bone `i` hangs under `root`, walking its parents.
    let under = |i: usize, root: usize, parents: &[i32]| -> bool {
        let mut walk = i;
        loop {
            if walk == root {
                return true;
            }
            match usize::try_from(parents[walk]) {
                Ok(p) => walk = p,
                Err(_) => return false,
            }
        }
    };
    // Every bone under a shoulder keeps its length to its parent, the tip stays within the
    // wing's reach, the tips mirror across the midline, and the rest of the body holds still.
    let check = |g: &[Mat4], rest: &[Mat4], parents: &[i32], wings: &Wings, what: &str| {
        for w in &wings.wings {
            for (i, &p) in parents.iter().enumerate() {
                if !under(i, w.limb.root, parents) {
                    continue;
                }
                let Ok(p) = usize::try_from(p) else {
                    continue;
                };
                let now = (at(g, i) - at(g, p)).length();
                let was = (at(rest, i) - at(rest, p)).length();
                assert!(
                    (now - was).abs() < 1e-2,
                    "{what}: bone {i} keeps its length: {now} vs {was}"
                );
            }
            let span = (at(g, w.limb.effector) - at(g, w.limb.root)).length();
            assert!(
                span <= w.reach + 1e-2,
                "{what}: the tip within the wing's reach: {span} > {}",
                w.reach
            );
        }
        for (i, (now, was)) in g.iter().zip(rest).enumerate() {
            if !wings.wings.iter().any(|w| under(i, w.limb.root, parents)) {
                assert!(now.abs_diff_eq(*was, 1e-4), "{what}: bone {i} holds still");
            }
        }
        let (l, r) = (
            at(g, wings.wings[0].limb.effector),
            at(g, wings.wings[1].limb.effector),
        );
        assert!(
            (l.x + r.x).abs() < 0.1 && (l.y - r.y).abs() < 0.1 && (l.z - r.z).abs() < 0.1,
            "{what}: the tips mirror: {l} vs {r}"
        );
    };
    let Some((bones, rest)) = load(Pattern::Bird) else {
        eprintln!("skipping: no packaged bird");
        return;
    };
    let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
    let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
    let cycle = FlapCycle::crow();
    let mut wings = Wings::new(&names, &parents, &rest, cycle);
    assert_eq!(wings.wings.len(), 2, "both wings: {:?}", wings.wings);
    assert!(
        wings
            .wings
            .iter()
            .all(|w| w.feathers.len() == 2 && w.digits.is_empty()),
        "two feather groups a wing: {:?}",
        wings.wings
    );
    let left = wings.wings[0].clone();
    assert_eq!(names[left.limb.effector], "wing_tip_l");
    let tip_rest = at(&rest, left.limb.effector);
    let span = (tip_rest - at(&rest, left.limb.root)).length();
    let mut globals = rest.clone();
    let dt = 1.0 / 60.0;
    let (mut max, mut min) = (f32::MIN, f32::MAX);
    for tick in 0..40 {
        wings.step(&mut globals, &parents, &rest, dt);
        check(&globals, &rest, &parents, &wings, &format!("tick {tick}"));
        let z = at(&globals, left.limb.effector).z;
        max = max.max(z);
        min = min.min(z);
    }
    assert!(
        max - tip_rest.z > 0.25 * span && tip_rest.z - min > 0.1 * span,
        "the tip rises {} and falls {} on a span of {span}",
        max - tip_rest.z,
        tip_rest.z - min
    );
    wings.glide(&mut globals, &parents, &rest);
    check(&globals, &rest, &parents, &wings, "the glide");
    let elevation = |g: &[Mat4]| {
        let v = at(g, left.limb.effector) - at(g, left.limb.root);
        v.z.atan2(v.x.abs())
    };
    assert!(
        (elevation(&globals) - elevation(&rest) - cycle.dihedral_rad).abs() < 1e-3,
        "the glide is the rest raised by the dihedral: {} from {}",
        elevation(&globals),
        elevation(&rest)
    );

    let Some((bones, rest)) = load(Pattern::Bat) else {
        eprintln!("skipping the bat: no packaged bat");
        return;
    };
    let names: Vec<&str> = bones.iter().map(|b| b.name.as_str()).collect();
    let parents: Vec<i32> = bones.iter().map(|b| b.parent).collect();
    let cycle = FlapCycle::bat();
    let wings = Wings::new(&names, &parents, &rest, cycle);
    assert_eq!(wings.wings.len(), 2, "both wings: {:?}", wings.wings);
    assert!(
        wings
            .wings
            .iter()
            .all(|w| w.digits.len() == 4 && w.digits.iter().all(|d| d.len() == 3)),
        "four three-joint digits a wing: {:?}",
        wings.wings
    );
    let wing = wings.wings[0].clone();
    let (hand, tip) = (wing.limb.end, wing.limb.effector);
    assert_eq!(names[tip], "wing_digit_1_03_l");
    let reach_of = |g: &[Mat4]| (at(g, tip) - at(g, hand)).length();
    let lag = 2.0 * cycle.lag_rad + cycle.feather_lag;
    let mut g = rest.clone();
    wings.pose_phase(&mut g, &parents, &rest, lag / TAU);
    check(&g, &rest, &parents, &wings, "the bat's upstroke");
    assert!(
        reach_of(&g) < reach_of(&rest) - 1.0,
        "the leading digit's tip draws toward the wrist: {} from {}",
        reach_of(&g),
        reach_of(&rest)
    );
    wings.pose_phase(&mut g, &parents, &rest, (lag + PI) / TAU);
    check(&g, &rest, &parents, &wings, "the bat's downstroke");
    assert!(
        (reach_of(&g) - reach_of(&rest)).abs() < 1e-2,
        "spread through the downstroke: {} vs {}",
        reach_of(&g),
        reach_of(&rest)
    );
}

/// THE PREP FACING CONTROL faces a source onto the rig (2026-09-11, re-measured 2026-09-21): the
/// human's Turn 90° is EXACT — four turns return the geometry bit-for-bit — and it now rides on
/// top of the yaw `flicker_content::measure_facing` reads off the body itself, which is what the
/// bench seeds `Document::facing_yaw` with. The old bounding-box default ("a quarter-turn when
/// the longest dimension is X") is gone: swept over the hoofed family it read a bull's horns and
/// a ewe's wool as the body, and seven of seventeen sources are yawed 30–55° besides, which no
/// whole quarter-turn squares. The measurement itself is gated in `flicker-content`, where it
/// lives; what the bench owns is the turn and the document knobs.
#[test]
fn the_facing_control_turns_a_broadside_mesh_onto_the_rig() {
    use flicker_content::{face_to_rig, RawModel, RawVertex};

    let vert = |p: [f32; 3]| RawVertex {
        p,
        n: [0.0, 0.0, 1.0],
        uv: [0.0, 0.0],
        joints: [0; 4],
        weights: [0.0; 4],
    };
    // A "horse": long along X (±120), thin along Y (±25), standing to Z 170.
    let horse = RawModel {
        regions: Vec::new(),
        vertices: vec![
            vert([-120.0, 0.0, 0.0]),
            vert([120.0, 0.0, 85.0]),
            vert([0.0, -25.0, 170.0]),
            vert([0.0, 25.0, 40.0]),
        ],
        indices: vec![0, 1, 2],
        bones: Vec::new(),
    };
    let span = |m: &RawModel, axis: usize| {
        let (lo, hi) = m
            .vertices
            .iter()
            .map(|v| v.p[axis])
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), c| {
                (lo.min(c), hi.max(c))
            });
        hi - lo
    };

    // One turn takes the long axis from X onto Y (the rig faces −Y), length preserved.
    let mut turned = horse.clone();
    face_to_rig(&mut turned, 0.0, 1);
    assert!(
        span(&turned, 1) > span(&turned, 0),
        "the long axis is now Y, not X"
    );
    assert!(
        (span(&turned, 1) - span(&horse, 0)).abs() < 1e-4,
        "the length is preserved"
    );

    // Four turns are the identity, to the bit — the reason the turn is an integer matrix.
    let mut round = horse.clone();
    for _ in 0..4 {
        face_to_rig(&mut round, 0.0, 1);
    }
    for (a, b) in round.vertices.iter().zip(&horse.vertices) {
        assert_eq!(a.p, b.p, "four quarter-turns return the mesh exactly");
    }

    // No measured yaw and no turn of the human's own touches nothing at all.
    let mut still = horse.clone();
    face_to_rig(&mut still, 0.0, 0);
    for (a, b) in still.vertices.iter().zip(&horse.vertices) {
        assert_eq!(a.p, b.p, "a square, unturned body is left alone");
    }

    // A MEASURED yaw rides with the human's turn: 90° of yaw alone lays X onto Y too.
    let mut yawed = horse.clone();
    face_to_rig(&mut yawed, 90.0, 0);
    assert!(
        span(&yawed, 1) > span(&yawed, 0),
        "the measured yaw lays the long axis on Y as well"
    );

    // The document opens with neither: the measurement is taken when a source is cached.
    let doc = Document::new();
    assert_eq!(doc.facing_quarters, 0, "the human has turned nothing yet");
    assert_eq!(doc.facing_yaw, 0.0, "and nothing has been measured yet");
}

/// THE STEP RAIL MATCHES THE WORKFLOW (2026-09-11): every `ap_steps_<wf>` pill in the scene has
/// exactly one tab per [`Workflow::steps`] entry, values 0..n in order. A truncated rail (the
/// creature rail once stopped at three tabs, so the Rig step was the last reachable stop and a
/// creature could never be previewed, attached or committed with a skeleton) fails here.
#[test]
fn every_workflow_rail_matches_its_steps() {
    use crate::ui::Workflow;
    let scene: serde_json::Value =
        serde_json::from_str(crate::ui::SCENE).expect("the bench scene parses");
    // Walk the tree for a node by id.
    fn find<'a>(node: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
        match node {
            serde_json::Value::Object(m) => {
                if m.get("id").and_then(|v| v.as_str()) == Some(id) {
                    return Some(node);
                }
                m.values().find_map(|v| find(v, id))
            }
            serde_json::Value::Array(a) => a.iter().find_map(|v| find(v, id)),
            _ => None,
        }
    }
    for wf in Workflow::ALL {
        let rail_id = format!("ap_steps_{}", wf.name());
        let rail = find(&scene, &rail_id).unwrap_or_else(|| panic!("no rail `{rail_id}`"));
        let tabs = rail
            .get("children")
            .and_then(|c| c.as_array())
            .unwrap_or_else(|| panic!("`{rail_id}` has no tabs"));
        let steps = wf.steps();
        assert_eq!(
            tabs.len(),
            steps.len(),
            "`{rail_id}` has {} tabs but {:?} has {} steps",
            tabs.len(),
            wf,
            steps.len()
        );
        for (i, tab) in tabs.iter().enumerate() {
            assert_eq!(
                tab.get("value").and_then(|v| v.as_u64()),
                Some(i as u64),
                "`{rail_id}` tab {i} must bind to step index {i}"
            );
        }
    }
}

// ── THE REGION TAGGER (spec 0A81088E T2) ─────────────────────────────────────────────────

/// THE ONE SEAM: every region knob writes `model.regions[i]` through `edit_region`, and every
/// write bumps the generation the highlight and the bake read off. The four edits are the four
/// the panel offers — the tag stepper, the anchor stepper, the typed chain count and the
/// stiffness slider — and an edit that changes nothing is not a write.
#[test]
fn edit_region_moves_the_row_and_bumps_the_generation() {
    let mut doc = synthetic_rigged_doc("region_edit");
    // One region off a cut plane, so there is a row to edit (no disk, no fitting body).
    assert!(
        doc.select_culled(&[(Vec3::X, 0.0)]),
        "the cut plane selected a region"
    );
    assert_eq!(doc.regions().len(), 1);
    assert_eq!(doc.region_sel(), Some(0));
    let first = doc.region_gen;

    assert!(doc.edit_region(0, RegionEdit::Tag(1)), "the tag stepped");
    assert_eq!(doc.regions()[0].tag, RegionTag::Hair, "Cloth → Hair");
    assert!(doc.region_gen > first, "the generation moved");

    // The stepper is a RING: six tags, six steps, back where it started.
    for _ in 0..5 {
        doc.edit_region(0, RegionEdit::Tag(1));
    }
    assert_eq!(doc.regions()[0].tag, RegionTag::Cloth, "six steps wrap");
    doc.edit_region(0, RegionEdit::Tag(-1));
    assert_eq!(
        doc.regions()[0].tag,
        RegionTag::Appendage,
        "and back the other way"
    );

    let before = doc.regions()[0].anchor_bone.clone();
    assert!(
        doc.edit_region(0, RegionEdit::Anchor(1)),
        "the anchor stepped"
    );
    let after = doc.regions()[0].anchor_bone.clone();
    assert_ne!(after, before);
    let bones: Vec<String> = doc
        .parsed()
        .unwrap()
        .model
        .bones
        .iter()
        .map(|b| b.name.clone())
        .collect();
    assert!(
        bones.contains(&after),
        "the anchor is one of the model's bones"
    );

    assert!(doc.edit_region(0, RegionEdit::Chains("4".to_string())));
    assert_eq!(doc.regions()[0].chain_count, 4);
    // 0 is a real answer (rigid to the anchor); an unparsable field is no edit at all.
    assert!(doc.edit_region(0, RegionEdit::Chains("0".to_string())));
    assert_eq!(doc.regions()[0].chain_count, 0);
    let gen = doc.region_gen;
    assert!(!doc.edit_region(0, RegionEdit::Chains(String::new())));
    assert!(
        !doc.edit_region(0, RegionEdit::Chains("0".to_string())),
        "same value, no write"
    );
    assert_eq!(doc.region_gen, gen, "a no-op never bumps the generation");

    assert!(doc.edit_region(0, RegionEdit::Stiffness(0.02)));
    assert!((doc.regions()[0].params.stiffness - 0.02).abs() < 1e-6);
    assert!(doc.region_gen > gen);

    // The row reads back as the list shows it, with the tag resolved through the table.
    load_shipped_strings();
    let rows = doc.region_rows();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].1.contains(&after) && rows[0].1.contains("×0") && rows[0].1.contains("0.020"),
        "the row carries name · tag · anchor · chains · stiffness: {}",
        rows[0].1
    );
    assert!(!rows[0].1.contains('$'), "the tag resolves: {}", rows[0].1);

    // REMOVE drops it and the pick clears with it.
    assert!(doc.remove_region(0));
    assert!(doc.regions().is_empty() && doc.region_sel().is_none());
}

/// SELECT CULLED takes exactly what the panels' cut planes hide — `normal · x > d`, no more and
/// no less — and a selection that hides nothing is not a region.
#[test]
fn select_culled_takes_the_far_side_of_the_cut_plane() {
    let mut doc = synthetic_rigged_doc("region_culled");
    assert!(doc.select_culled(&[(Vec3::Z, 20.0)]));
    let picked: Vec<u32> = doc.regions()[0].verts.clone();
    let want: Vec<u32> = doc
        .parsed()
        .unwrap()
        .model
        .vertices
        .iter()
        .enumerate()
        .filter(|(_, v)| v.p[2] > 20.0)
        .map(|(i, _)| i as u32)
        .collect();
    assert!(!want.is_empty(), "the fixture has a far side");
    assert_eq!(picked, want, "exactly the far-side vertices");
    assert_eq!(
        doc.regions()[0].tag,
        RegionTag::Cloth,
        "a cull pick is cloth"
    );
    assert!(!doc.select_culled(&[]), "no cut selects nothing");
}

/// SPLIT GARMENT through the services seam: the real fitting body, the mount fit's placement and
/// the Prep page's hang — the same three `bake_garment` uses. Skips without the content tree, as
/// every real-data test here does.
#[test]
fn the_split_verb_finds_rows_against_the_fitting_body() {
    // Through the GZ SEAM: the fitting body is at rest as `.json.gz` and `fitting_base()` names
    // the logical `.json`, so a plain `Path::exists` would skip on every real tree there is.
    if !flicker_content::package::file_exists(&flicker_content::fitting_base()) {
        return;
    }
    let mut doc = synthetic_rigged_doc("region_split");
    let gen = doc.region_gen;
    let found = doc.split_regions().expect("the split ran");
    assert!(found > 0, "the piece stands clear of the body somewhere");
    assert_eq!(doc.regions().len(), found);
    assert_eq!(doc.region_rows().len(), found, "a row per region");
    assert_eq!(
        doc.region_sel(),
        Some(0),
        "the split opens on its first row"
    );
    assert!(doc.region_gen > gen);
    for r in doc.regions() {
        assert!(!r.verts.is_empty() && !r.anchor_bone.is_empty());
    }
    // THE HANG IS THE KNOB: opened wider, less of the piece reads as cloth. (It never reaches
    // nothing here — a vertex past the BODY's own grid hangs by definition, which is the answer
    // for a piece that stands outside the body's bounding box; `regions.rs` gates the threshold
    // itself on a fixture that lies inside the field.)
    let tight: usize = doc.regions().iter().map(|r| r.verts.len()).sum();
    doc.hang_cm = 30.0;
    doc.split_regions().expect("the split ran");
    let loose: usize = doc.regions().iter().map(|r| r.verts.len()).sum();
    assert!(
        loose < tight,
        "a wider hang takes less of the piece: {loose} < {tight}"
    );
}

/// DIAGNOSTIC (ignored): the bench's own Prep → Conform on a real source folder, printing what
/// the rig came to and the regions the hand-off proposed — `FLICKER_CREATURE_DIR=<folder>
/// [FLICKER_CLASS=character|creature] [FLICKER_TRIS=…] [FLICKER_STATURE=…] … --ignored
/// --nocapture`. The class defaults to creature; `character` opens the folder as the Character
/// workflow does (the Skin class, the Humanoid pick).
#[test]
#[ignore]
fn diagnose_the_hand_off_on_a_real_creature() {
    let Ok(dir) = std::env::var("FLICKER_CREATURE_DIR") else {
        eprintln!("skipping: FLICKER_CREATURE_DIR not set");
        return;
    };
    load_shipped_strings();
    let mut doc = Document::new();
    doc.pending_class = match std::env::var("FLICKER_CLASS").as_deref() {
        Ok("character") => Some(AssetClass::Skin),
        _ => Some(AssetClass::Creature),
    };
    // `FLICKER_PREFER_STAGED=1` ticks the Task page's "Import as rigged" box: the staged rig is
    // re-opened as committed, which is the one route to an older rig.
    doc.prefer_staged = std::env::var("FLICKER_PREFER_STAGED").is_ok();
    doc.open(PathBuf::from(dir));
    assert!(doc.parsed().is_some(), "the folder parsed");
    eprintln!(
        "OPENED class {:?} bones {:?} error {:?} rig {}",
        doc.class(),
        doc.bone_count(),
        doc.source.as_ref().and_then(|s| s.error.clone()),
        doc.source.as_ref().is_some_and(|s| s.rig.is_some())
    );
    doc.ensure_prep_source();
    eprintln!(
        "PREP stature {} target {} tris {:?} pick {}",
        doc.stature_cm,
        doc.decimate_target,
        doc.tri_count(),
        doc.skeleton_name()
    );
    if let Ok(t) = std::env::var("FLICKER_TRIS") {
        doc.decimate_target = t;
        doc.apply_decimate_target();
    }
    if let Ok(s) = std::env::var("FLICKER_STATURE") {
        doc.stature_cm = s.parse().unwrap();
    }
    doc.conform();
    eprintln!(
        "CONFORMED bones {:?} tris {:?} stature {} error {:?} rig {} status {:?} rail {:?}",
        doc.bone_count(),
        doc.tri_count(),
        doc.stature_cm,
        doc.source.as_ref().and_then(|s| s.error.clone()),
        doc.source.as_ref().is_some_and(|s| s.rig.is_some()),
        doc.rig_summary(),
        doc.marker_match_caption()
    );
    if let Some(m) = doc
        .source
        .as_ref()
        .and_then(|s| s.rig.as_ref())
        .and_then(|r| r.shape.as_ref())
    {
        eprintln!(
            "SHAPE matched {} unmatched {:?} warnings {:?}",
            m.matched.len(),
            m.unmatched,
            m.warnings
        );
    }
    let p = doc.parsed().expect("parsed");
    // JOINT BY JOINT (incident 2026-10-09, the dark elf's head, shoulder and hands): where every
    // joint landed against the body the fit read — outside the flesh by how much, a pair's two
    // sides against each other across the plane, the head and the face against the head core,
    // the finger fan against the hand's own tube.
    if std::env::var("FLICKER_JOINTS").is_ok() {
        let body = p.body();
        let flesh = &body.flesh;
        let world = &p.globals;
        let at = |name: &str| {
            p.model
                .bones
                .iter()
                .position(|b| b.name == name)
                .map(|i| world[i].w_axis.truncate())
        };
        let plane = body.graph.as_ref().map_or(0.0, |g| g.plane_x);
        for (i, b) in p.model.bones.iter().enumerate() {
            let q = world[i].w_axis.truncate();
            let out = flesh.distance_outside(q);
            let mirror = b
                .name
                .strip_suffix("_l")
                .and_then(|stem| at(&format!("{stem}_r")))
                .map(|r| {
                    let m = glam::Vec3::new(2.0 * plane - r.x, r.y, r.z);
                    (q - m).length()
                });
            eprintln!(
                "JOINT {:<18} ({:7.1},{:7.1},{:7.1}) outside {:5.1} cm{}",
                b.name,
                q.x,
                q.y,
                q.z,
                out,
                mirror.map_or(String::new(), |d| format!("  mirror error {d:5.1} cm"))
            );
        }
        if let Some(g) = body.graph.as_ref() {
            // The body's own pairs: how mirror-symmetric the two tubes are (attachment and tip).
            for (k, pr) in g.pairs.iter().enumerate() {
                let (l, r) = (&g.limbs[pr.l], &g.limbs[pr.r]);
                let m = |v: glam::Vec3| glam::Vec3::new(2.0 * plane - v.x, v.y, v.z);
                let tip = |x: &flicker_content::Limb| x.lead.last().copied().unwrap_or(x.at);
                eprintln!(
                    "PAIR {k} t {:.2}: attach l ({:.1},{:.1},{:.1}) r ({:.1},{:.1},{:.1}) mirror error {:.1} cm; tip l ({:.1},{:.1},{:.1}) r ({:.1},{:.1},{:.1}) mirror error {:.1} cm; arcs {:.1} / {:.1}",
                    pr.t, l.at.x, l.at.y, l.at.z, r.at.x, r.at.y, r.at.z, (l.at - m(r.at)).length(),
                    tip(l).x, tip(l).y, tip(l).z, tip(r).x, tip(r).y, tip(r).z, (tip(l) - m(tip(r))).length(),
                    l.arc, r.arc
                );
            }
            for (k, c) in g.cores.iter().enumerate() {
                let (r, f) = (c.rear(), c.front());
                eprintln!(
                    "CORE {k}: radius {:.1} arc {:.1} upright {} rear ({:.1},{:.1},{:.1}) front ({:.1},{:.1},{:.1})",
                    c.radius, c.arc, c.upright, r.x, r.y, r.z, f.x, f.y, f.z
                );
            }
        }
        // The hand: the fitted forearm and hand directions against the finger fan's.
        for side in ["l", "r"] {
            if let (Some(lower), Some(hand), Some(mid1), Some(mid3), Some(idx3), Some(th3)) = (
                at(&format!("lowerarm_{side}")),
                at(&format!("hand_{side}")),
                at(&format!("middle_01_{side}")),
                at(&format!("middle_03_{side}")),
                at(&format!("index_03_{side}")),
                at(&format!("thumb_03_{side}")),
            ) {
                let fore = (hand - lower).normalize_or_zero();
                let fan = (mid3 - hand).normalize_or_zero();
                eprintln!(
                    "HAND {side}: forearm→hand {:.1} cm, hand→middle_03 {:.1} cm, fan off the forearm {:.0}°, hand→middle_01 {:.1}; tips outside: middle {:.1} index {:.1} thumb {:.1}",
                    (hand - lower).length(),
                    (mid3 - hand).length(),
                    fore.dot(fan).clamp(-1.0, 1.0).acos().to_degrees(),
                    (mid1 - hand).length(),
                    flesh.distance_outside(mid3),
                    flesh.distance_outside(idx3),
                    flesh.distance_outside(th3)
                );
            }
        }
    }
    if let Some(s) = flicker_content::bake::Seating::read(&p.model, p.body()) {
        for a in s.appendages(&p.model) {
            eprintln!(
                "APPENDAGE on {}: {} verts, flat {:.2}, whole {:.2}",
                p.model.bones[a.bone].name,
                a.verts.len(),
                a.flat,
                a.whole
            );
        }
    }
    for r in doc.regions() {
        eprintln!(
            "REGION {} tag {:?} on {} — {} verts, {} chain(s), stiffness {}",
            r.name,
            r.tag,
            r.anchor_bone,
            r.verts.len(),
            r.chain_count,
            r.params.stiffness
        );
    }
}
