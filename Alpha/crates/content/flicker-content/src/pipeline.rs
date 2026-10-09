//! The full in-app import, end to end: a folder of raw Meshy sources → one baked `flicker.rig`.
//!
//! This is the orchestrator the import editor calls — it composes the stages that already exist
//! ([`scan`](crate::scan) → [`parse_fbx`](crate::parse_fbx) → rename → conform → [`bake_rig`](crate::bake_rig))
//! and adds texture wiring: the source's PNG maps are copied beside the rig under the content
//! standard's `<AssetName>_<Map>.png` names and referenced from the material. No external tools.
//!
//! [`wire_textures`] is that stage, and it is the crate's ONE texture-wiring path — the prop and
//! garment bakes ([`crate::bake::write_prop`] / [`crate::bake::write_garment`]) call it too, so
//! every class of asset carries its maps by the same rules and under the same names.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flicker_skeletal::format::SkeletonRecipe;

use crate::bake::{bake_rig, default_mounts};
use crate::conform::{
    conform_to_canonical, face_to_rig, measure_facing, rig_raw_mesh, scale_mesh_to_stature,
    straighten_frames, ConformMode,
};
use crate::decimate::decimate_to;
use crate::fbx::parse_fbx;
use crate::rig::rename_to_canonical;
use crate::scan::{scan_folder, Kind};

/// How to prepare a RAW (skeleton-less) mesh before rigging it — the Clayworks Prep step's two
/// numbers: the real-world stature to size the mesh to, and the triangle count to collapse it to
/// (`None` = keep the source count). A mesh that arrives rigged is game-ready and skips Prep, so
/// this is ignored for it.
#[derive(Debug, Clone)]
pub struct RawMeshPrep {
    pub stature_cm: f32,
    pub target_tris: Option<usize>,
    /// The skeleton to compose onto the mesh (the modular skeleton, 2026-09-07).
    pub recipe: SkeletonRecipe,
    /// Which side the bake-time stance normaliser squares a mid-stride body FROM (FEFDA2B2).
    pub stance_source: crate::bake::StanceSource,
    /// Which half of a LOPSIDED source the mesh mirror keeps — `None` is the bench's "Mirror
    /// from: Off", the default, because a mirror destroys everything one-sided that is not tagged
    /// (697DEC55). A SOURCE-SHAPE fix, so it runs before the skeleton is fitted.
    pub mirror_keep: Option<crate::mirror::Side>,
    /// Un-turn a source whose HEAD IS TURNED at bake ([`crate::bake::face_forward`]). ON BY
    /// DEFAULT (Aaron 2026-09-28, A79A6131: turned heads are common across the generated sources,
    /// not only the birds' 45°) — a normaliser like the stance's, a no-op on a head within 5° of
    /// forward; `false` is the opt-out.
    pub face_forward: bool,
    /// FACING: the human's quarter-turns about the vertical Z, ON TOP of the yaw
    /// [`crate::conform::measure_facing`] measures off the body itself — the bench's Turn 90°
    /// (69F4B20D), which is what flips a body that came out tail-first. `None` is no turn of his
    /// own. WITHOUT any facing at all the headless import fed a broadside body to the trunk
    /// alignment and measured its chest and rump ACROSS the animal instead of along it.
    pub facing_quarters: Option<u8>,
}

impl Default for RawMeshPrep {
    /// The bench's Prep as it opens: the canon's stature, the source's own triangle count, the
    /// humanoid pick, the stance read off the pose, no mirror, the head FACED FORWARD, no turn of
    /// the human's own.
    fn default() -> Self {
        Self {
            stature_cm: crate::baseline::STATURE,
            target_tris: None,
            recipe: SkeletonRecipe::humanoid(),
            stance_source: crate::bake::StanceSource::Auto,
            mirror_keep: None,
            face_forward: true,
            facing_quarters: None,
        }
    }
}

/// What an import produced.
#[derive(Debug, Clone)]
pub struct ImportSummary {
    pub source_fbx: PathBuf,
    pub rig_path: PathBuf,
    pub bones: usize,
    pub tris: usize,
    /// Role-named texture basenames written beside the rig.
    pub textures: Vec<String>,
}

/// Import one source folder into `out_dir/<asset_name>.json` (+ role-named textures), conforming to
/// `reference` (use [`crate::default_reference`]). Errors — rather than guessing — when the folder
/// has no riggable mesh or more than one (the editor disambiguates that case).
///
/// A mesh that arrives WITH a skeleton takes the vendor-rig path (rename → conform → bake). A RAW
/// mesh (no skeleton — the ultra Meshy generations) takes the boneless path exactly as the
/// Clayworks character rail runs it: Prep (collapse to `raw.target_tris`, size to
/// `raw.stature_cm`) → [`rig_raw_mesh`] → the canonical-frame gate Commit applies → bake; it
/// needs `raw`, and says so rather than guessing a stature. Either way the character ships the
/// six untuned [`default_mounts`], as the bench's Attach stage would.
pub fn import_folder(
    source_dir: &Path,
    out_dir: &Path,
    asset_name: &str,
    reference: &Path,
    raw: Option<RawMeshPrep>,
) -> Result<ImportSummary> {
    let scan =
        scan_folder(source_dir).with_context(|| format!("scanning {}", source_dir.display()))?;
    let rig_entry = match (scan.sole_riggable(), scan.riggable.len()) {
        (Some(e), _) => e.clone(),
        (None, 0) => bail!("no riggable mesh found in {}", source_dir.display()),
        (None, n) => {
            let names: Vec<_> = scan
                .candidates()
                .filter_map(|e| e.path.file_name().map(|s| s.to_string_lossy().into_owned()))
                .collect();
            bail!(
                "{n} riggable meshes in {} — the editor must pick one: {names:?}",
                source_dir.display()
            );
        }
    };

    let mut model = parse_fbx(&rig_entry.path)?;
    if model.bones.is_empty() {
        let Some(prep) = raw.as_ref() else {
            bail!(
                "{} carries no skeleton — a raw mesh needs a stature and a triangle target",
                rig_entry.path.display()
            );
        };
        let source_tris = model.indices.len() / 3;
        if let Some(target) = prep.target_tris.filter(|t| *t < source_tris) {
            model = decimate_to(&model, target);
        }
        // SIZE IT, then FACE IT ONTO THE RIG — Prep's own order, and the bench's
        // (`rebuild_prepped_model`): the scale grounds the mesh and plants it on the plumb line,
        // and the facing turn lays a side-profile body along the rig's −Y forward BEFORE anything
        // measures it (69F4B20D). `rig_raw_mesh`'s own scale is then the identity. The mirror
        // needs both to have happened — it cuts on world X = 0, which is the median plane only
        // for a centred, faced mesh.
        scale_mesh_to_stature(&mut model, prep.stature_cm);
        let yaw = measure_facing(&crate::flesh::Flesh::build_body(&model));
        let quarters = prep.facing_quarters.unwrap_or(0);
        tracing::info!(
            "import_folder: facing {yaw:.1}° measured + {} quarter-turn(s)",
            quarters % 4
        );
        face_to_rig(&mut model, yaw, quarters);
        // MIRROR THE MESH first of all, in PREP: it changes the SOURCE SHAPE, so a lopsided sculpt
        // is made symmetric BEFORE the skeleton is fitted to it (the ratified order 42AB9BA8) —
        // every twin-joint assumption the fit makes then holds.
        if let Some(keep) = prep.mirror_keep {
            let r = crate::mirror::mirror_mesh(&mut model, keep);
            tracing::info!(
                "import_folder: mirrored from the {keep:?} half — {} dropped, {} added, {} region(s) kept",
                r.dropped,
                r.added,
                r.kept_regions
            );
        }
        rig_prepped_mesh(&mut model, prep)?;
    } else {
        rename_to_canonical(&mut model);
        conform_to_canonical(&mut model, reference, ConformMode::Canonical).with_context(|| {
            format!(
                "conforming {} to {}",
                rig_entry.path.display(),
                reference.display()
            )
        })?;
    }
    let mut rig = bake_rig(&model, asset_name);
    // A raw mesh carries the recipe it was composed on; a vendor rig conformed onto the
    // canonical reference IS the humanoid recipe.
    rig.skeleton_recipe = Some(
        raw.as_ref()
            .map_or_else(SkeletonRecipe::humanoid, |p| p.recipe.clone()),
    );
    rig.attach_points = default_mounts()
        .into_iter()
        .map(|m| flicker_skeletal::format::AttachPoint {
            id: m.id,
            bone: m.bone,
            offset: m.offset,
        })
        .collect();

    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let textures = wire_textures(&scan, &rig_entry.path, out_dir, asset_name, &mut rig)?;

    let rig_path = out_dir.join(format!("{asset_name}.json"));
    let json = serde_json::to_string(&rig).context("serializing the rig")?;
    // Emits the gz-at-rest form (`<rig_path>.gz`) via the shared seam; readers
    // (and the summary) keep addressing the rig by its logical path.
    crate::package::write_text(&rig_path, &json)
        .with_context(|| format!("writing {}", rig_path.display()))?;

    Ok(ImportSummary {
        source_fbx: rig_entry.path,
        rig_path,
        bones: rig.skeleton.bones.len(),
        tris: rig.mesh.indices.len() / 3,
        textures,
    })
}

/// THE HEADLESS BAKE OF A PREPPED RAW MESH — the half of [`import_folder`] that runs once the mesh
/// is collapsed, sized, faced onto the rig and (if asked) mirrored: [`rig_raw_mesh`], then the two
/// un-poses on the bound skin, then the frames the composed skeleton ships with.
///
/// SQUARE THE STANCE before the rig is written — the headless half of the ONE bake path (ruling
/// 42AB9BA8): a mid-stride source is promoted standing on both feet. `prep.stance_source` names the
/// side to mirror from (FEFDA2B2); Auto reads the pose. Its feet are read off the body the fit
/// already read — the mesh is thinned once.
///
/// FACE FORWARD right after it and on the same bound skin, BY DEFAULT (A79A6131): the fit laid
/// the neck along the path the shape took and turned the face with it, and
/// [`crate::bake::face_forward`] un-turns a head more than 5° off forward — `prep.face_forward`
/// false is the opt-out.
pub(crate) fn rig_prepped_mesh(model: &mut crate::fbx::RawModel, prep: &RawMeshPrep) -> Result<()> {
    let fit = rig_raw_mesh(model, prep.stature_cm, &prep.recipe)?;
    // How the body sits in its limbs AS POSED, taken before an un-pose moves one — the skin it
    // moves in is put on last, on this seating.
    let seating = fit
        .body
        .as_ref()
        .and_then(|b| crate::bake::Seating::read(model, b));
    // THE HAND-OFF (ruling 7881216F): every boneless appendage the bind found — a horn, an
    // antler, an ear — ships as a tagged region: rigid with the bone it grows from, or (flat)
    // soft on its chains. Read off the fit's own body, before an un-pose moves it.
    if let Some(s) = &seating {
        let proposed = crate::regions::appendage_regions(model, &s.appendages(model));
        for r in &proposed {
            tracing::info!(
                "import_folder: {} on {} — {} vertices, {} chain(s)",
                r.name,
                r.anchor_bone,
                r.verts.len(),
                r.chain_count
            );
        }
        model.regions.extend(proposed);
    }
    let stance =
        crate::bake::square_stance_on(model, prep.stance_source, &prep.recipe, fit.body.as_ref());
    let mut moved = !stance.squared.is_empty();
    for (limb, lift) in stance.squared {
        tracing::info!("import_folder: squared {limb} ({lift:.1} cm of lift)");
    }
    for (limb, sunk) in stance.declined {
        tracing::warn!(
            "import_folder: {limb} left as posed — squaring it would sink the mesh {sunk:.1} cm \
             through its own floor"
        );
    }
    if prep.face_forward {
        let r = crate::bake::face_forward(model);
        tracing::info!(
            "import_folder: the head was {:.1}° off forward ({})",
            r.yaw_deg,
            if r.turned { "turned" } else { "left as posed" }
        );
        moved |= r.turned;
    }
    // THE SKIN IT MOVES IN, last: the un-poses ran on the tube skin, and the body that ships
    // carries its haunches and shoulders on the limbs under them
    // ([`crate::bake::bind_for_motion`]) — through the flesh as it now stands, the fit's own
    // read unless an un-pose moved it.
    let as_posed = fit.body.as_ref().map(|b| &b.flesh).filter(|_| !moved);
    crate::bake::bind_for_motion(model, seating.as_ref(), as_posed);
    // A COMPOSED rig keeps the IDENTITY frames its pattern skeleton and clip libraries were
    // baked with — the same bake the bench's Commit runs on a composed or re-opened body.
    straighten_frames(model);
    Ok(())
}

/// Copy the source's texture maps beside the rig under `<AssetName>_<Map>.png` and point the (single)
/// material at them. Meshy names the maps `…texture_0.png` (base), `…_metallic.png`, `…_roughness.png`.
///
/// The ONE texture-wiring path: the character import ([`import_folder`]) and the prop / garment bakes
/// ([`crate::bake::write_prop`] / [`crate::bake::write_garment`]) all come through here, so a prop
/// carries its maps by exactly the rules — and under exactly the names — a character does.
///
/// `mesh` is the source file the rig was baked FROM. A folder often holds a whole SET (a weapon set
/// is four or five pieces, an outfit is tops/pants/gloves/shoes) with one map set PER PIECE, all
/// named after their own mesh — so the maps sharing this mesh's stem are the ones that belong to it.
/// When none share it — the character layout, where Meshy writes `…biped_texture_0.png` beside a
/// `…biped_Character_output.fbx` — the folder's whole texture list is considered, which is what the
/// character import has always done.
pub(crate) fn wire_textures(
    scan: &crate::scan::Scan,
    mesh: &Path,
    out_dir: &Path,
    asset_name: &str,
    rig: &mut flicker_skeletal::format::RigFile,
) -> Result<Vec<String>> {
    let SourceMaps {
        base_color: base,
        metalness: metal,
        roughness: rough,
        normal,
    } = source_maps(scan, mesh);

    let mut copied = Vec::new();
    let bc = copy_role(
        base.as_deref(),
        "BaseColor",
        out_dir,
        asset_name,
        &mut copied,
    )?;
    let me = copy_role(
        metal.as_deref(),
        "Metallic",
        out_dir,
        asset_name,
        &mut copied,
    )?;
    let ro = copy_role(
        rough.as_deref(),
        "Roughness",
        out_dir,
        asset_name,
        &mut copied,
    )?;
    let no = copy_role(
        normal.as_deref(),
        "Normal",
        out_dir,
        asset_name,
        &mut copied,
    )?;

    if let Some(m) = rig.mesh.materials.get_mut(0) {
        m.name = asset_name.to_string();
        m.slot = asset_name.to_string();
        if let Some(x) = bc {
            m.base_color = x;
        }
        if let Some(x) = me {
            m.metalness = x;
        }
        if let Some(x) = ro {
            m.roughness = x;
        }
        if let Some(x) = no {
            m.normal = x;
        }
    }
    // Record the copied maps in the rig's PROVENANCE too, not only on the material. `source.textures`
    // is the manifest a validator (or a later session) reads to answer "does this asset carry its
    // maps?"; leaving it empty while the material referenced four PNGs said "untextured" about a
    // fully-textured asset. Written here because `wire_textures` is the ONE path every class comes
    // through, so one assignment fixes character, prop and garment at once.
    rig.source.textures = copied.clone();
    Ok(copied)
}

/// The source PNGs that belong to a mesh, classified by MAP ROLE from their filenames.
///
/// The ONE place that decides which file is the albedo and which is a PBR map. Both consumers read
/// it: [`wire_textures`] (which copies them beside the baked rig) and the asset-pipeline editor's
/// fit PREVIEW (which uploads them straight to the viewport) — so what the user sees textured in
/// the editor is exactly the map set the bake will ship.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMaps {
    /// Albedo — the un-suffixed `…texture_0.png`. sRGB colour data.
    pub base_color: Option<PathBuf>,
    /// LINEAR data from here down.
    pub metalness: Option<PathBuf>,
    pub roughness: Option<PathBuf>,
    pub normal: Option<PathBuf>,
}

/// Classify the folder's texture maps for `mesh` — see [`SourceMaps`].
///
/// `mesh` is the source file the rig was baked FROM. A folder often holds a whole SET (a weapon set
/// is four or five pieces, an outfit is tops/pants/gloves/shoes) with one map set PER PIECE, all
/// named after their own mesh — so the maps sharing this mesh's stem are the ones that belong to
/// it. When none share it — the character layout, where Meshy writes `…biped_texture_0.png` beside
/// a `…biped_Character_output.fbx` — the folder's whole texture list is considered.
pub fn source_maps(scan: &crate::scan::Scan, mesh: &Path) -> SourceMaps {
    // The maps named after THIS mesh, else (no piece-named map at all) the folder's whole list.
    let mine: Vec<&crate::scan::Entry> = scan
        .of_kind(Kind::Texture)
        .filter(|e| belongs_to(e, mesh))
        .collect();
    let textures: Vec<&crate::scan::Entry> = if mine.is_empty() {
        scan.of_kind(Kind::Texture).collect()
    } else {
        mine
    };

    let mut out = SourceMaps::default();
    for e in textures {
        let n = e
            .path
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        // Maps with no single-channel slot to sit in: Meshy's packed `…_metallic_roughness` (it
        // ships the dedicated `…_metallic` / `…_roughness` beside it, and the packed one would
        // otherwise land in `metalness`) and its `…_emit` / `…_emission`. Skipped explicitly so
        // neither is mistaken for a map role — or, worse, for the albedo. Matched with the role
        // separator, so only a `<stem>_<role>.png` suffix counts.
        if [
            "_metallic_roughness",
            "_metalness_roughness",
            "_emit",
            "_emission",
        ]
        .iter()
        .any(|s| n.contains(s))
        {
            continue;
        }
        if n.contains("metallic") || n.contains("metalness") {
            out.metalness = Some(e.path.clone());
        } else if n.contains("roughness") {
            out.roughness = Some(e.path.clone());
        } else if n.contains("normal") {
            out.normal = Some(e.path.clone());
        } else if out.base_color.is_none() {
            out.base_color = Some(e.path.clone()); // base color (the un-suffixed `…texture_0.png`)
        }
    }
    out
}

/// Does this texture belong to `mesh`? Meshy names a piece's maps after the piece
/// (`…_texture.fbx` → `…_texture.png`, `…_texture_metallic.png`, …), which is the only thing that
/// tells a katana's maps from its scabbard's in a shared set folder.
fn belongs_to(e: &crate::scan::Entry, mesh: &Path) -> bool {
    let Some(stem) = mesh.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else {
        return false;
    };
    let Some(name) = e
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
    else {
        return false;
    };
    !stem.is_empty() && name.starts_with(&stem)
}

/// The directory a file path sits in — `"."` for a bare name, so a relative `out` never resolves to
/// the empty path (which no filesystem call accepts).
pub(crate) fn dir_of(file: &Path) -> &Path {
    match file.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Copy one map file to `out_dir/<AssetName>_<Map>.png`; return the written basename.
fn copy_role(
    src: Option<&Path>,
    map: &str,
    out_dir: &Path,
    asset_name: &str,
    copied: &mut Vec<String>,
) -> Result<Option<String>> {
    let Some(src) = src else { return Ok(None) };
    let dst_name = format!("{asset_name}_{map}.png");
    std::fs::copy(src, out_dir.join(&dst_name))
        .with_context(|| format!("copying texture {} → {dst_name}", src.display()))?;
    copied.push(dst_name.clone());
    Ok(Some(dst_name))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Load the pipeline-produced `HumanBaseA` through the REAL engine loader together with the shared
    /// locomotion clips — the exact path the paperdoll takes. Confirms the automated bake yields an
    /// asset the engine accepts (66 bones, mesh + textures, clips resolve by name). `#[ignore]`d — it
    /// depends on `import_folder` having been run; do that then:
    ///   `cargo test -p flicker-content -- --ignored humanbasea_loads_in_engine --nocapture`
    #[test]
    #[ignore]
    fn humanbasea_loads_in_engine_with_clips() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content");
        let base = content.join("characters/HumanBaseA");
        // Prefer HumanBaseA's OWN re-baked (flat-foot) clips, matching the paperdoll's per-body path.
        let own = content.join("retarget/clips/HumanBaseA/locomotion");
        let clips = if own.is_dir() {
            own
        } else {
            content.join("retarget/clips/locomotion")
        };
        if !crate::package::file_exists(&base.join("HumanBaseA.json")) {
            eprintln!("skipping: run `--example import_folder` first to produce HumanBaseA");
            return;
        }
        let model = flicker_skeletal::format::load_dirs(&[&base, &clips])
            .expect("engine loads HumanBaseA + clips");
        let resolved: usize = model.clips.iter().map(|c| c.tracks.len()).sum();
        let unresolved: usize = model.clips.iter().map(|c| c.unresolved.len()).sum();
        eprintln!(
            "HumanBaseA: {} bones, {} verts, {} clips ({} tracks resolved, {} unresolved), material base_color '{}'",
            model.bones.len(),
            model.mesh.vertices.len(),
            model.clips.len(),
            resolved,
            unresolved,
            model
                .mesh
                .materials
                .first()
                .map(|m| m.base_color.as_str())
                .unwrap_or(""),
        );
        assert_eq!(model.bones.len(), 66, "engine sees the canonical 66 bones");
        assert!(model.mesh.vertices.len() > 10_000, "mesh carried through");
        assert!(!model.clips.is_empty(), "shared locomotion clips loaded");
        assert!(
            resolved > 0,
            "clip tracks resolve against HumanBaseA's bones by name"
        );
        assert_eq!(
            model.mesh.materials.first().map(|m| m.base_color.as_str()),
            Some("HumanBaseA_BaseColor.png")
        );
        assert!(
            base.join("HumanBaseA_BaseColor.png").exists(),
            "base-color texture written beside the rig"
        );

        // The copied locomotion pack drives the Animate view: its state machine builds against
        // HumanBaseA's clip list (same shared names) — so the walk plays, not just bind.
        use flicker_skeletal::state::{self, StateMachine};
        let pack =
            state::load_pack(&base.join("HumanBaseA.pack.json")).expect("HumanBaseA pack loads");
        let refs: Vec<state::ClipRef> = model
            .clips
            .iter()
            .map(|c| state::ClipRef {
                name: &c.name,
                duration_ticks: c.duration_ticks,
            })
            .collect();
        let sm = StateMachine::build(&pack, &refs)
            .expect("state machine builds against HumanBaseA clips");
        eprintln!(
            "pack state machine ready, initial state '{}'",
            sm.current_state_name()
        );
    }

    /// DECISIVE bind check: at the REST pose the skinning palette is `rest_world · inverse_bind`, which
    /// must be identity — so skinning the mesh at rest must reproduce the bind mesh exactly. If this
    /// holds, any in-window deformation is the ANIMATION/retarget, not my conform/bake's bind. If it
    /// fails, my inverse_bind is wrong. `#[ignore]`d; needs HumanBaseA baked. Run:
    ///   `cargo test -p flicker-content -- --ignored humanbasea_rest_skin --nocapture`
    #[test]
    #[ignore]
    fn humanbasea_rest_skin_matches_bind_mesh() {
        use flicker_skeletal::{pose, skin};
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/package/characters/HumanBaseA");
        if !crate::package::file_exists(&base.join("HumanBaseA.json")) {
            eprintln!("skipping: bake HumanBaseA first (--example import_folder)");
            return;
        }
        let model = flicker_skeletal::format::load_dir(&base).unwrap();
        let rest_locals: Vec<_> = model.bones.iter().map(|b| b.local).collect();
        let globals = pose::global_transforms(&model.bones, &rest_locals);
        let palette = skin::palette(&model.bones, &globals);
        let skinned = skin::skin(&model.mesh, &palette);
        let mut worst = 0.0f32;
        for (i, v) in model.mesh.vertices.iter().enumerate() {
            let p = skinned[i].position;
            let d = ((p[0] - v.p[0]).powi(2) + (p[1] - v.p[1]).powi(2) + (p[2] - v.p[2]).powi(2))
                .sqrt();
            worst = worst.max(d);
        }
        eprintln!(
            "REST-skin vs bind mesh: worst {worst:.4} cm across {} verts",
            model.mesh.vertices.len()
        );
        assert!(
            worst < 0.5,
            "rest pose must skin back to the bind mesh (worst {worst:.4} cm) — else inverse_bind is wrong"
        );
    }
}
