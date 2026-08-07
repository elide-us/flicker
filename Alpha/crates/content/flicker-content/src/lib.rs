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
pub mod bvh;
pub mod conform;
pub mod manifest;
pub mod fbx;
pub mod ops;
pub mod package;
pub mod pipeline;
pub mod retarget;
pub mod rig;
pub mod roots;
pub mod scan;

pub use bake::{
    attach_world, bake_garment, bake_prop, bake_rig, bake_skin, fitting_base, garment_socket,
    write_garment, write_prop, write_rig, write_rig_file, Fit, MountPoint,
};
pub use pipeline::{import_folder, source_maps, ImportSummary, SourceMaps};
pub use conform::{
    conform_to_canonical, default_reference, derive_ankle_placement, derive_hip_placement,
    derive_shoulder_placement, infer_canonical_bones, reorient_to_canonical, AnkleReport,
    ConformOutput, ConformReport, HipReport, InferReport, ShoulderReport,
};
pub use fbx::{apply_orientation, parse_fbx, quarter_turn, RawBone, RawModel, RawVertex};
pub use ops::{
    keep_both_name, occupied, physical_path, probe_conflicts, BatchFileOp, Conflict, FileFacts,
    FileOp, Resolution, TRASH_DIR,
};
pub use rig::{rename_to_canonical, RenameReport};
pub use roots::{init_from_app_dir, roots, set_content_root, ContentConfig, ContentRoots};
pub use scan::{
    classify, classify_asset, classify_package, classify_package_head, scan_folder, AssetClass,
    AssetReport, Entry, Kind, PackageClass, PropKind, Scan,
};
