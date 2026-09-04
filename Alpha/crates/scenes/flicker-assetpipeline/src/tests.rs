//! The services' tests — the pipeline exercised against the real content tree (skipping
//! when it is absent, like `flicker-content`'s own real-data tests) and headlessly against
//! scratch roots. No UI: the scene's own tests live with the scene.

use std::path::{Path, PathBuf};

use flicker::render::{Mat4, Vec3};
use flicker::ui::strings;
use flicker_content::{default_reference, AssetClass, Fit, PropKind, RawModel, RawVertex};
use flicker_skeletal::pose::{global_transforms, sample_local_poses};

use crate::services::{
    BoneOffset, Document, MapState, Parsed, class_label, model_bounds, rest_globals, ATTACH_POINTS, CONFORMED_BONES, REFERENCE_BONES, SOCKETS, WF_ANIMATION, WF_CHARACTER, WF_PROP,
};
use crate::meshes::{BasePreview, BASE_MESH_BUDGET};

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
fn load_shipped_strings() {
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
    assert_eq!(
        doc.workflow,
        WF_PROP,
        "a prop conforms by mounting"
    );
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
    let rebaked = world(&m2);
    for (name, p) in &after {
        let q = rebaked[name];
        assert!((q - *p).length() < 1e-2, "re-bake moved {name}: {p} → {q}");
    }
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
    assert_eq!(
        doc.workflow,
        WF_PROP,
        "a prop conforms by mounting"
    );
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
/// A headless document with the canon rig installed on a synthetic sphere: opened in a
/// scratch folder, parsed from `sphere_mesh`, prepped, then conformed — the shape the
/// Rig step works on, with no vendor file in sight.
pub(crate) fn synthetic_rigged_doc(tag: &str) -> Document {
    let scratch = std::env::temp_dir().join(format!("flicker_assetpipeline_{tag}"));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
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

fn sphere_mesh(rings: usize, segments: usize, radius: f32) -> RawModel {
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
    let (centre, radius, floor) = model_bounds(&model, &globals);
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
