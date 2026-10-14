//! flicker-content — the in-app content pipeline (golden-spec WS-F, node A3A3259C).
//!
//! The realized workflow (Aaron): point a folder at a location of RAW sources (Meshy FBX + PNG
//! textures, plus FBX/BVH animations) → the app detects what's in there → processes it into the ONE
//! self-describing `flicker.rig`, with **no external tools** (in-app Rust FBX, not Blender/Python).
//!
//! Pipeline stages (built incrementally):
//!   1. [`scan`] — INGEST: enumerate + classify a folder, pick riggable candidate(s), disambiguate. ← this slice
//!   2. FBX parse — a Rust FBX reader → mesh (verts/normals/uv/indices) + skeleton + skin.
//!   3. Canonical rig — bone rename + finger/twist/socket inference + limb-align (`Quat::from_rotation_arc`).
//!   4. Bake — emit `flicker.rig` JSON + role-named textures, ready to load + display.

pub mod bake;
pub mod baseline;
pub mod browse;
pub mod bundle;
pub mod bvh;
/// Per-bone body capsules read off the flesh — the rig's `collision` volumes, the body the
/// runtime cloth cannot pass through (spec 6C46CAB9).
pub mod capsules;
pub mod conform;
pub mod decimate;
pub mod fbx;
pub mod flesh;
pub mod manifest;
pub mod mirror;
pub mod ops;
pub mod pack;
pub mod package;
pub mod pipeline;
pub mod propset;
pub mod regions;
pub mod retarget;
pub mod rig;
pub mod scan;
/// THE SHAPE GRAPH (spec 04803E0C) — the flesh thinned to a curve skeleton and walked into cores,
/// limbs, sheets and symmetry pairs: a mesh's structure in a vocabulary with no anatomy in it.
pub mod shape;

// The content-roots service moved to flicker-core (beside the file seam it
// feeds) so UI-layer crates can resolve roots without dragging the content
// pipeline's closure (ufbx, skeletal) into theirs. Full re-export keeps every
// existing `flicker_content::roots()` / `::roots::…` call site compiling
// unchanged — one implementation, two doors.
pub use flicker_core::roots;

pub use bake::{
    attach_world, bake_garment, bake_prop, bake_rig, bake_skin, default_mounts, face_forward,
    fitting_base, garment_socket, load_rig_raw, square_stance, square_stance_on, write_garment,
    write_prop, write_rig, write_rig_file, FaceReport, Fit, MountPoint, StanceReport, StanceSource,
    DEFAULT_MOUNTS,
};
pub use browse::{
    breadcrumb, display_name, files_under, list_dir, logical, parent_within_roots, tree_rows, Row,
    SortKey, TreeRow,
};
// THE material BUNDLE (plan 30FE7F58 P3): a recipe + its baked maps under
// `materials/<id:03>/`, the promotable unit, and the ONE place a slot id
// becomes a path. `bundle_at(package_root, slot).is_some()` IS the definition
// of a DEFINED slot.
pub use bundle::{bundle_at, bundle_dir, MaterialBundle, MATERIALS_DIR};
pub use capsules::bone_capsules;
pub use conform::{
    align_trunk, conform_to_canonical, default_reference, derive_ankle_placement,
    derive_hip_placement, derive_shoulder_placement, face_to_rig, fill_chain, fit_baseline_to_mesh,
    fit_to_graph, infer_canonical_bones, install_baseline_skeleton, install_skeleton, match_recipe,
    measure_facing, reorient_to_canonical, rig_raw_mesh, scale_mesh_to_stature,
    splice_canonical_chain, straighten_frames, z_quarter_turns, AlignReport, AnkleReport,
    ConformMode, ConformOutput, ConformReport, FitReport, HipReport, InferReport, Matched,
    ScaleReport, ShapeMatch, ShoulderReport,
};
pub use decimate::decimate_to;
pub use fbx::{
    apply_orientation, first_material_color, parse_fbx, quarter_turn, RawBone, RawModel, RawVertex,
};
// THE half-body mirror verb (direction 697DEC55): a lopsided sculpt is cut on the median plane and
// one side reflected onto the other, the TAGGED regions carried through as authored.
pub use mirror::{mirror_mesh, MirrorReport, Side};
// THE guided-rig primitive: one voxel occupancy + inscribed-radius field per mesh, shared by the
// conform fits (joint landmarks at the flesh's NARROWINGS) and the bench's ortho depth read.
pub use flesh::{narrowings, Core, Flesh};
pub use flicker_core::roots::{
    init_from_app_dir, installed_app_dir, roots, set_content_root, ContentConfig, ContentRoots,
};
pub use ops::{
    keep_both_name, occupied, physical_path, probe_conflicts, BatchFileOp, Conflict, FileFacts,
    FileOp, Resolution, TRASH_DIR,
};
pub use pipeline::{import_folder, source_maps, ImportSummary, RawMeshPrep, SourceMaps};
pub use propset::{PropSet, PropVariant};
pub use shape::{Body, Limb, Link, Pair, ShapeGraph};
// The region tagger's headless half (spec 0A81088E): a garment finds its own hanging panels and
// each one gets the comb of chains that makes it drape.
pub use regions::{
    appendage_regions, build_cloth, region_from, split_garment, split_worn, verts_beyond,
    DEFAULT_HANG_CM,
};
pub use rig::{rename_to_canonical, RenameReport};
pub use scan::{
    classify, classify_asset, classify_package, classify_package_head, scan_folder, AssetClass,
    AssetReport, Entry, Kind, PackageClass, PropKind, Scan,
};
