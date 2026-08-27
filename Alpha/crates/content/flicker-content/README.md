# flicker-content

The **in-app content pipeline**: point it at a folder of raw vendor sources (Meshy FBX +
PNG textures, FBX/BVH animations, static prop meshes) and it scans, classifies, parses,
conforms, and **bakes** them into the engine's one self-describing `flicker.rig` format —
with **no external tools** (in-app Rust FBX via `ufbx`, never Blender/Python). It also owns
the two content **tiers** every bench writes into and the game reads from — `staging/` and
`package/` — the **gz-at-rest** seam between them, the **promotion** ledger, and the
store-only `package.flk` shipping container. It is a library the content benches (Clayworks,
Sablework, Loomforge, the Quartermaster) call; it also ships a handful of headless CLIs for
running the same steps without a window.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Vocabulary (flicker words used below)

- **content tree** — the folder (`Alpha/content/`) holding all game content, in tiers.
- **staging tier** (`staging/`) — where the benches WRITE processed output. Not shipped, not
  read by the running game.
- **package tier** (`package/`) — the runtime read set. Content arrives here only by
  **promotion** from staging, never by a bench writing straight in.
- **promote / promotion** — move an asset's folder `staging/<rel>` → the mirrored
  `package/<rel>` and append one row to the package **manifest** (the promotion ledger).
- **gz-at-rest** — every text content file lives on disk individually gzip-compressed
  (`<name>.json.gz`). Readers address the **logical** path (`<name>.json`); the seam finds
  the `.gz` twin. A **physical** path is what is actually on disk.
- **the seam** — the one read/write routine (`flicker-core::compression`, re-exported as
  [`package`](#reading--writing-content-at-rest)) that makes gz-at-rest transparent.
- **`flicker.rig`** — the engine's single self-describing asset format (rig / skin / anim /
  prop / collision are all one schema). Owned by `flicker-skeletal`; this crate WRITES it.
  For the format itself see the content-tree guide, linked below, and MCP.
- **conform / canon / canonical** — remap a vendor rig onto the one canonical skeleton (67
  bone names + parent topology + Z-up/cm conventions) so it drives the shared animation clips.
- **bake** — emit the final `flicker.rig` (+ role-named textures) ready to load and display.
- **prop** — a boneless static mesh (weapon, accessory, set dressing, grass). **propset** — a
  named set of interchangeable prop **variants** with spawn weights.
- **garment** — a mesh skinned onto the canonical body; **socket** — the bone a prop/garment
  mounts to; **fit / attach** — the authored offset/rotation/scale of that mount.
- **retarget** — re-express a BVH/FBX motion **clip** onto the canonical skeleton; each clip
  yields two **variants** — *In-Place* (pelvis pinned, a treadmill) and *RootMotion* (travel kept).
- **mount / `package.flk`** — the shipped package is a store-only zip mounted read-only over
  `package/`; a dev tree has no `.flk` and reads loose files.

Authors who write into these trees use the content-tree guides, not this crate:
[`Alpha/content/README.md`](../../../content/README.md) (the tree's layout + the
`flicker.rig` format) and [`Alpha/content/staging/README.md`](../../../content/staging/README.md)
(the staging tier). This README documents the **Rust API** those tools are built on.

## Where it sits

- **Builds on:** `flicker-core` (the content-roots service, the gz `compression` seam, and the
  `package.flk` mount all live there; this crate re-exports the first two under its own name —
  one implementation, two doors), `flicker-skeletal` (the `flicker.rig` format types
  `RigFile`/`Skeleton`/`Mesh`/`Material`/`Attach` — the bake writes them via their `Serialize`
  derive, so there is one schema), `ufbx` (the in-app FBX reader), `glam`, `zip` (store-only).
- **Used by:** the content benches — Clayworks/`flicker-assetpipeline` (character import,
  conform, bake, retarget), Sablework/`flicker-texture` (material recipes), the
  Quartermaster (`flicker-quartermaster`, staging→package promotion + the content browser),
  `prism-alpha` (calls `init_from_app_dir` at startup), and `flicker-pocclusters` (loads the
  promoted grass propset). The offline `content-tool` bin and the crate's `examples/` are
  headless entry points into the same functions.
- **Reads from / writes to the content tree** (all paths resolved through
  [`roots()`](#content-roots--the-packagestaging-seam), never hardcoded):
  - `<root>/content.json` — the per-executable root declaration. Read once at startup by
    `init_from_app_dir`; absent or invalid → the default `../content` layout (warns, never blocks).
  - `staging/` — bench output is WRITTEN here (import, bake, retarget, propset).
  - `package/` — READ here (fitting body, reference skeleton, real-data test guards); WRITTEN
    only by promotion.
  - `package/manifest.json` (gz-at-rest) — the promotion ledger, appended on each promote.
  - `package.flk` beside the root — the shipped container; mounted over `package/` if present.
  - `source/` — raw vendor exports; authoring INPUT, never written, never shipped.

## Public API

Everything below is reachable from `lib.rs`. Grouped by concern, not by Rust item kind.

### Content roots & the package/staging seam

The data-interface rule: a module ASKS where content is, it never spells a path. `roots()`,
`ContentRoots`, `ContentConfig`, and the init/probe functions are **re-exported from
`flicker-core::roots`** (they moved there so UI crates can resolve roots without pulling in
the FBX/skeletal pipeline). `flicker_content::roots` names both the module and the function.

| Item | What it is for | The one thing to know |
|---|---|---|
| `roots() -> ContentRoots` | The process's resolved content roots. | Call it wherever you need a path. Before any exe has declared a root (library tests, `content-tool`, `examples/`) it falls back to this crate's own repo position — the ONE remaining hardcoded climb, and it lives only here. |
| `ContentRoots` | One root + its DERIVED sub-roots. | `.root()` `.package()` `.staging()` `.data()` `.sensorium()` `.source()`. Sub-roots are derived from the one root, so `staging/` and `package/` can never name different trees. `new()`/`resolve()` build one. |
| `ContentConfig { content_root: String }` | The committed `content.json` (serde, `#[serde(default)]`). | `load_from(app_dir)` is best-effort: missing = the normal default (`../content`), invalid = a `warn` + default. A malformed config never stops startup. Relative `content_root` hangs off the app dir; absolute wins. |
| `init_from_app_dir(app_dir) -> ContentRoots` | Startup: read `content.json`, install the process root, mount `package.flk` if present. | Call ONCE before anything touches content. A present-but-**unreadable** `package.flk` **panics** (same fatality class as a broken scene manifest); no `.flk` → loose-tree behaviour. |
| `set_content_root(Option<PathBuf>)` | Point the process at a tree directly. | The escape hatch for tests and tools handed a tree on the CLI. Never mounts a `.flk`. Prefer `init_from_app_dir`. |
| `installed_app_dir() -> Option<PathBuf>` | The exe's dir IF this is an installed layout. | `Some` only when a `content.json` sits beside the exe; dev `cargo run` gets `None`. The ONE place an exe path is consulted. |
| `CONTENT_CONFIG_FILE: &str` | `"content.json"`. | The declaration filename. |

### Reading & writing content at rest (`package`)

The gz-at-rest seam, re-exported from `flicker-core::compression` plus this crate's offline
converter. Processed-content writers EMIT gz; readers address the logical path.

| Item | What it is for | The one thing to know |
|---|---|---|
| `package::read_text` / `read_bytes` | Read a logical content path. | Tries `<path>.gz` first, then the raw file (dev-loose / test fixture), then the mounted `.flk`. Shipped form wins. |
| `package::write_text` / `write_bytes` | Write processed content. | ALWAYS emits the gz-at-rest form (`<path>.gz`) — one-way discipline. Creates parent dirs. |
| `package::file_exists` / `gz_sibling` / `names_gz` | Existence + at-rest name helpers. | `gz_sibling` appends `.gz`; `names_gz` tests for it. |
| `gzify_dir(dir) -> GzifyStats` | Recursively convert loose text content to gz-at-rest. | Verifies each round-trip BEFORE deleting the raw file; idempotent (a second pass converts nothing). Only `GZIFY_EXTENSIONS` (`json`, `flight`, `epoch`); binaries and docs skipped. |
| `GZIFY_EXTENSIONS: &[&str]` | The text formats `gzify_dir` converts. | `["json","flight","epoch"]`. |
| `GzifyStats` | What one gzify pass did. | `converted` / `skipped_gz` / `skipped_other` / `bytes_before` / `bytes_after`. |

### The store-only package container (`pack`)

`package.flk` is a zip whose entries are all **Stored** — the tree is already gz-at-rest, so
the container adds no compression. The `zip` dep compiles with no compression backends, so it
*cannot* deflate.

| Item | What it is for | The one thing to know |
|---|---|---|
| `pack_dir(dir, out) -> PackStats` | Pack a tree into the store-only `.flk`. | **Deterministic**: sorted forward-slash entry paths, fixed zip-epoch timestamps, 0644 perms → same tree = byte-identical output. Verifies every entry against the tree before renaming into place. Refuses a symlink, non-UTF-8 name, empty tree, dot-files, or an out-file inside the tree. |
| `verify_pack(flk, tree: Option) -> PackStats` | CRC-read every entry; with a tree, byte-compare + require exact name-set equality both ways. | The `manifest.json` inside is packed as ordinary content — provenance, never the index. |
| `PackStats { files, bytes }` | What one pack/verify covered. | `bytes` is uncompressed == stored content size. |

### The promotion ledger (`manifest`)

`package/manifest.json` is **not an index** (the packer walks the tree for that). It records
promotion INTENT — one row appended per promote, removed on undo.

| Item | What it is for | The one thing to know |
|---|---|---|
| `manifest::append(path, ManifestEntry)` | Add one promotion row. | Creates the manifest on first promote; normalizes logical paths to forward slashes at this seam (so a `PathBuf`-built row can't land backslashed). |
| `manifest::read(path) -> Vec<ManifestEntry>` | Read the rows. | Absent → empty (v1). Gz-transparent. |
| `manifest::remove(path, &ManifestEntry)` | The undo of `append` — removes the LAST equal row. | Removing a row that is not there is a LOUD error (a ledger that lost what it recorded is corrupt), never a silent no-op. |
| `ManifestEntry { name, class, path, promoted_from }` | One promotion. | `path`/`promoted_from` are logical, forward-slash. `class` is a **free-form provenance string** — nothing filters behaviour on it, and it is NOT validated against any enum (see Sharp edges). |

### File operations (`ops`) — the Content Manager's reversible moves

`package` owns how content is ENCODED; `ops` owns how it is REARRANGED. Every op records
enough to undo itself (the bench's *one mutation, one Ctrl+Z* promise).

| Item | What it is for | The one thing to know |
|---|---|---|
| `FileOp` | One reversible change: `Move { src, dst, .. }` (covers rename) or `Mkdir`. | Build with `FileOp::mv` / `rename` / `mkdir`. Meaningful only inside a batch. |
| `BatchFileOp` | A group that applies/unwinds as ONE history entry. | `new(ops, trash_root, batch_id)`, then `apply()` / `revert()`; `len`/`is_empty`/`is_applied`. A partial-failure `apply` unwinds what already landed. Reverts newest-first. |
| `probe_conflicts(&[(src,dst)]) -> Vec<Conflict>` | Find collisions before running, so the UI prompts once each. | Free destinations produce no entry. |
| `Conflict` / `FileFacts` / `Resolution` | The conflict prompt's data + the user's choice. | `Resolution::{Replace, KeepBoth (default), Skip}`. Replace PARKS the displaced file under `.trash/<batch_id>/` so the batch stays revertible — never unlinks. |
| `keep_both_name(dst) -> PathBuf` | The first free `<stem>_NN.<ext>` beside `dst`. | Splits on the FIRST dot so `Foo.pack.json` → `Foo_01.pack.json`. Gives up after 99. |
| `physical_path(logical) -> Option` / `occupied(logical)` | Resolve a logical path to the real file / test if anything is there. | A move relocates the physical bytes as-is — never re-encodes (both tiers are gz-at-rest). |
| `TRASH_DIR: &str` | `".trash"`. | Where Replace parks displaced files, under the staging root. Not a user recycle bin. |

### Ingest: scan & classify (`scan`)

Two classifications: **source** files (what to rig) and **package** files (the browser's Type
column). Source-side is coarse (extension + name heuristic — the FBX parser is the real
determinant); package-side must sniff, because ~600 files are generically `<name>.json.gz`.

| Item | What it is for | The one thing to know |
|---|---|---|
| `scan_folder(root) -> Scan` | Recursively classify every file; find the riggable candidates. | Riggable = `Kind::MeshFbx`. |
| `Scan` | The scan result. | `needs_selection()` (>1 riggable → the editor MUST ask which), `sole_riggable()`, `candidates()`, `of_kind(k)`. |
| `Entry { path, kind }` / `Kind` | One classified source file. | `Kind::{MeshFbx, AnimFbx, Bvh, Texture, Rig, Manifest, Other}`. |
| `classify(path) -> Kind` | Classify one source path by extension + name. | An FBX whose name reads like a motion clip (`walk`, `idle`, `Animation_…`) is `AnimFbx`. |
| `classify_asset(scan, bones: Option<usize>) -> AssetReport` | Classify a whole source FOLDER. | Bone count is the deciding signal (≥20 = character); sharpens once the FBX is parsed. `confidence` is DERIVED from agreeing evidence, never decorative. |
| `AssetReport { class, prop, confidence, evidence }` / `AssetClass` / `PropKind` | The verdict + why. | `AssetClass::{Skin, Prop, Animation}` (`.id()` → `"skin"`/`"prop"`/`"animation"`). `PropKind::{Weapon, Clothing, Environment, Accessory}`. |
| `classify_package(path) -> PackageClass` | Classify a PROCESSED file (the content browser's Type). | Cheap extensions first (`.png`, `.ttf`, `.flight`, `.epoch`, `.pack.json`, `.rbp.json`, `.texture.json`, `manifest.json`); a bare `.json` is sniffed by its head (see `classify_package_head`). **Does not yet recognize `flicker.propset` — see Finding #1.** |
| `classify_package_head(&[u8]) -> PackageClass` | The layout-agnostic sniff, over an already-read head. | Keys off a populated `clips` array (→ Clip) or a populated `mesh` (→ Rig), because `format` can sit megabytes deep in an alphabetically-ordered file. |
| `PackageClass` | The processed-file kinds. | `Rig, Clip, CombatPack, RetargetBasePose, Bake, Flight, Epoch, Texture, TextureRecipe, Font, Doc, Manifest, Folder, Unknown` (`.id()` for style/log keys). **No `PropSet` variant — Finding #1.** |

### FBX parse (`fbx`)

Reads a raw FBX into a convention-neutral `RawModel`, normalised to the engine's space:
**Z-up, centimetres, right-handed** (Meshy authors Y-up metres — ufbx does the conversion; do
not pre-scale).

| Item | What it is for | The one thing to know |
|---|---|---|
| `parse_fbx(path) -> RawModel` | Parse one FBX to the intermediate model. | Bone-less is NOT an error — a static prop IS a mesh with no skeleton (`bones: []`). Only NO geometry fails. Mesh is non-deduped (one vertex per triangle-corner), V-flipped to top-origin UVs. |
| `RawModel { vertices, indices, bones }` | The parsed model, FBX-native then engine space. | The intermediate every conform/bake/decimate stage consumes and produces. |
| `RawVertex` / `RawBone` | Per-corner vertex (pos/normal/uv + 4 joints/weights); one bone (name, parent `-1`=root, local TRS, `inverse_bind`). | `inverse_bind` = inverse rest WORLD frame, so rest-pose skinning is the identity. |
| `first_material_color(path) -> Option<[f32;3]>` | **POC** — read the first material's flat base colour (linear RGB). | For untextured flat-shaded props (Synty foliage) whose colour lives in the material, not a map. **TEMPORARY**, conflicts with the Materials-Unification plan — see Finding #2 and Sharp edges. |
| `apply_orientation(&mut model, r: Mat4)` | Rotate a whole asset into a different ground reckoning (stand up a source authored on its side). | Only root locals move; children follow through the hierarchy. |
| `quarter_turn(axis) -> Mat4` | An EXACT integer 90° turn about a world axis. | Built by axis permutation, not `sin`/`cos`, so four turns return the asset bit-for-bit — a repeatedly-nudged orientation control must not drift into the bake. |

### Canonical conform (`conform`, `rig`)

Remaps a vendor Meshy rig onto the canonical skeleton so it drives the shared clips. The full
port of `tools/blender/rename_meshy_to_canonical.py`.

| Item | What it is for | The one thing to know |
|---|---|---|
| `rename_to_canonical(&mut model) -> RenameReport` | Rename Meshy bones to canonical names; drop head-tip markers. | `RenameReport { renamed, dropped, unmapped }` — a large `unmapped` means the source isn't a standard Meshy biped. |
| `conform_to_canonical(&mut model, reference, ConformMode) -> ConformOutput` | The full conform: derive joint widths → reorient → infer missing bones. | `ConformMode::Canonical` (standard) reorients onto the reference; `ConformMode::AsProvided` keeps the vendor frames and only completes the bone set (the diagnostic path). |
| `ConformMode` / `ConformOutput` | The mode + a report bundle (`hip`/`shoulder`/`ankle`/`reorient`/`infer`). | See §Sharp edges for the two modes. |
| `derive_hip_placement` / `derive_shoulder_placement` / `derive_ankle_placement` | Mesh-derived WIDTH/height corrections for joints Meshy plants weakly. | Each returns its own report; **WIDTH/height only** — Meshy's bone LENGTHS are trusted. Run BEFORE reorient (they consume rest positions). |
| `reorient_to_canonical(&mut model, reference) -> ConformReport` | Turn every bone onto the reference's world orientation, limb frames down this body's own limbs. | `ConformReport { limbs_aligned }`. |
| `infer_canonical_bones(&mut model, reference, mode) -> InferReport` | Add the bones Meshy never makes (30 fingers, 8 twists, 2 weapon sockets, jaw/eyes). | Inferred bones carry NO weights (appended after joint indices are baked); they resolve + rotate but deform nothing until a hand mesh is weighted to them. Ends by splicing the chain. |
| `splice_canonical_chain(&mut model, reference) -> Vec<String>` | Reparent canonical bones onto their canonical parent (world frame preserved), re-sort parents-before-children. | Also runs STANDALONE over a reloaded staged rig whose baked chain predates the splice fix; the human's fitted joints stay put, only the chain composition is repaired. |
| `scale_mesh_to_stature(&mut model, cm) -> ScaleReport` | Uniformly resize a mesh to a target height, ground it, plant it plumb. | The frame the authored canon lives in; pair with `install_baseline_skeleton` at the same stature. |
| `install_baseline_skeleton(&mut model, cm)` / `fit_baseline_to_mesh(&mut model, cm)` | Install (or rough-fit) the authored canon onto a rig-LESS mesh. | The raw-mesh rig path: bind == authored canon by construction. `fit_` pulls the arm joints onto the mesh so the human's follow-up is a nudge; never bends the torso. |
| `default_reference() -> PathBuf` | The conform reference: `GolemBaseSkeleton` (authored, skeleton-only A-pose at 170 cm). | The canon target — nobody's body. DISTINCT from `fitting_base()` (below). |

### The authored baseline skeleton (`baseline`)

The reference skeleton is generated from anthropometric fractions as DATA — never from a
vendor mesh.

| Item | What it is for | The one thing to know |
|---|---|---|
| `TOPOLOGY: [(&str,&str); 67]` | THE canon statement — 67 bone names + parents, hierarchy order. | The one place the canon topology is spelled; every conformed character carries exactly this. |
| `CANON_BONES: usize` | `TOPOLOGY.len()` — the canonical bone count. | The single number every consumer reads (loader tests, Clayworks requirements). |
| `STATURE: f32` | `170.0` cm — the ruled base height everything scales off. | |
| `world_positions() -> HashMap<String,Vec3>` | Every bone's authored world rest position (left authored, right mirrored). | |
| `golem_base_skeleton() -> RigFile` | Assemble the baseline as a skeleton-only `flicker.rig`. | Translation-only locals (identity rest rotations); clips supply rotations wholesale. |
| `emit(characters_root) -> PathBuf` | Write it as `GolemBaseSkeleton/GolemBaseSkeleton.json` (gz). | Regenerate with `cargo run -p flicker-content --example bake_baseline`. |

### Mesh decimation (`decimate`)

A standalone QEM edge-collapse for a `RawModel` — one pass, snapshotting at each retention
bucket so the Clayworks Prep slider scrubs instantly. Never touches the voxel LOD path.

| Item | What it is for | The one thing to know |
|---|---|---|
| `decimate_levels(model) -> DecimateLevels` | Precompute 100%→50% in 5% buckets (11 levels). | Reduction is a percent OF THE SOURCE tri count. Preserves UV seams + silhouette borders (only interior edges collapse). |
| `DecimateLevels` | The precomputed levels. | `levels` / `keep_fracs` / `source_tris`; `level_for_keep_pct` / `model_for_keep_pct`. |
| `decimate(model, keep: f32) -> RawModel` | One-shot decimate to a keep-fraction (0.5..=1.0). | For headless use. |
| `BUCKET_STEP` (`0.05`) / `MIN_KEEP` (`0.50`) | The slider step + floor. | Keep at most half removed. |

### Bake (`bake`) — emit `flicker.rig`

The last stage: assemble and write the canonical file. Editor-facing types (`Fit`,
`MountPoint`) so callers never touch the `flicker-skeletal` types.

| Item | What it is for | The one thing to know |
|---|---|---|
| `bake_rig(model, source_name) -> RigFile` | Assemble a CHARACTER rig. | Synthesizes an identity `root` at bone 0 (Meshy has none) — every parent + vertex joint index shifts +1. `retarget: true`. |
| `bake_prop(model, source_name, flat_color: Option) -> RigFile` | Assemble a boneless PROP rig. | No skeleton, no root synthesis, no +1 shift; `retarget: false`. `flat_color` (POC) fills the material `color` — see Finding #2. |
| `bake_garment(garment, source_name, body, socket, fit) -> RigFile` | Assemble a garment SKINNED onto a body. | Bakes the fit into vertex positions, transfers skin from the nearest body vertex; emits the body's full skeleton. |
| `bake_skin(&mut model)` | Re-derive skin weights from the CURRENT skeleton, discarding the vendor auto-skin. | The system's whole point (a vendor weights against its own mis-placed skeleton). Nearest bone SEGMENTS by inverse-square distance, top-4, pruned; `root`/weapon sockets never own flesh. |
| `write_rig(model, source_fbx, name, out, &[MountPoint])` | Bake a character AND write it (+ textures + attach points). | Funnels through `write_rig_file`; wires source textures like the import. |
| `write_prop(model, source_fbx, name, out, &Fit, flat_color)` | Bake a prop AND write it (fit folded into `attach`, textures wired). | The bench Commit and the `import_prop` CLI both call this — with DIFFERENT `flat_color` (Finding #2). |
| `write_garment(model, source_fbx, name, out, &Fit)` | Bake a garment onto `fitting_base()` AND write it. | Takes the body's weights, keeps its own material. |
| `write_rig_file(rig, out)` | The shared JSON writer every bake funnels through. | Emits gz-at-rest via the seam; readers address the logical `out`. |
| `load_rig_raw(path) -> RawModel` | The INVERSE of `bake_rig` — reload a staged/promoted rig to edit further. | Strips the synthesized root, undoes the +1 shift; gz-transparent. |
| `attach_world(socket_inverse_bind, &Attach) -> Mat4` | The world placement a fit resolves to on a socket. | IS the engine's `PieceFit::matrix` — the garment bake AND the editor preview both call it, so what the user approves is what bakes. |
| `fitting_base() -> PathBuf` | The BODY garments skin onto / the editor previews against. | Prefers `GolemBase_Low` (game-ready cut). Searches **package THEN staging** (promoted content wins). DISTINCT from `default_reference()` (the conform canon). |
| `garment_socket(name) -> &'static str` | A starting socket bone inferred from a garment's name. | Only seeds the editor's picker; the user confirms. |
| `Fit` / `MountPoint` | Editor-authored placement (socket + offset/rot-deg/per-axis scale + uniform) / a character attach point. | `Fit::to_attach()` floors every factor at 0.001. `Fit::default()` = mount to no bone (environment prop). |

### Props & variation sets (`propset`)

| Item | What it is for | The one thing to know |
|---|---|---|
| `PropSet { format, version, name, variants }` | A named set of interchangeable prop variants for randomized placement (e.g. a grass field). | `new()` / `load(path)` / `write(path)` (gz-at-rest, logical `<Name>.set.json`); `total_weight()`; `validate()` requires non-empty + all weights > 0. |
| `PropSet::pick(r: f32) -> &str` | Weighted-random pick of a member's name. | `r` in `[0,1)` — `fastrand::f32()` at runtime or a per-cell hash for reproducible placement. Clamps, never panics for a validated set. Returns a BARE name; the CALLER resolves it to a rig (Finding #3). |
| `PropVariant { prop, weight }` | One member: its prop asset name + relative spawn weight. | Only weight RATIOS matter. See Finding #3 on how `prop` resolves to a path. |

### Clip retarget (`bvh`, `retarget`)

| Item | What it is for | The one thing to know |
|---|---|---|
| `parse_bvh(path) -> Bvh` | Parse a Motifect `.bvh` (Y-up) to a hierarchy + per-frame motion. | The Y-up→Z-up convert + name-map + rebase are the retarget stage's job. |
| `Bvh` / `BvhJoint` | The parsed motion. | `Bvh::fps()`, `frame_locals(frame)`, `global_rotations(local)`. |
| `build_variants(bvh_path, skeleton) -> ClipVariants` | Retarget one BVH onto a skeleton, BOTH variants in memory (the preview seam). | Resamples to the 60 Hz canon (source rate must divide 60 evenly). |
| `ClipVariants { stem, in_place, root_motion }` | Both variants as complete `flicker.rig` clip JSON. | *In-Place* pins pelvis planar travel to rest (keeps vertical bob); *RootMotion* keeps travel. |
| `write_variants(&v, out_dir, in_place: bool, root_motion: bool) -> Vec<PathBuf>` | Write the PICKED variants under `{In-Place,RootMotion}/<stem>.json`. | The bench Commit passes the user's side-by-side choice. |
| `emit_variants(bvh_path, skeleton, out_dir) -> (PathBuf, PathBuf)` | Write BOTH — the CLI/library form. | |

### Full import orchestration (`pipeline`)

| Item | What it is for | The one thing to know |
|---|---|---|
| `import_folder(source_dir, out_dir, asset_name, reference) -> ImportSummary` | The whole character import: scan → parse → rename → conform → bake → wire textures → write. | Errors (never guesses) on zero or >1 riggable mesh. |
| `ImportSummary { source_fbx, rig_path, bones, tris, textures }` | What the import produced. | |
| `source_maps(scan, mesh) -> SourceMaps` | Classify a folder's texture maps by role for one mesh. | The ONE place that decides which PNG is albedo vs a PBR map; the editor's fit preview reads it too, so preview == bake. |
| `SourceMaps { base_color, metalness, roughness, normal }` | The role-classified maps. | Meshy's packed `_metallic_roughness` + `_emit`/`_emission` are skipped (no single-channel slot). |

## The headless tools

The crate is a library, but these run its steps without a window (verify with them; the GPU
app is the user's to eyeball). All resolve `roots()` to `Alpha/content` outside an app.

**`content-tool` bin** — offline maintenance of the package tree:

```text
content-tool gzify <dir>                    convert eligible text content to gz-at-rest
content-tool pack  <package-dir> <out-file> pack the tree into the store-only package.flk
content-tool verify <flk> [<package-dir>]   CRC-read every entry; with a tree, byte-compare
```

**`examples/`** — one function each, the same code the benches call:

```text
cargo run -p flicker-content --example import_folder  -- <source_dir> <out_dir> <AssetName> [reference.json]
cargo run -p flicker-content --example import_prop    -- <source.fbx> <out_dir> <AssetName>
cargo run -p flicker-content --example build_propset   -- <out_dir> <SetName> <prop:weight>...
cargo run -p flicker-content --example promote_asset   -- <rel> <class> <name>...
cargo run -p flicker-content --example retarget_clips  -- <bvh_dir> <skeleton.json> <out_dir>
cargo run -p flicker-content --example bake_baseline
cargo run -p flicker-content --example inspect_fbx     -- <file.fbx | folder>
```

Worked example — the grass propset in the tree, the first props promoted (bake three tufts,
group them, promote):

```text
# 1. bake each tuft into staging (lands as staging/props/environment/<Name>/<Name>.json.gz)
cargo run -p flicker-content --example import_prop -- \
  Alpha/content/source/Environment/Grass.fbx Alpha/content/staging/props/environment/Grass-Medium Grass-Medium
# 2. group them into a weighted set (shorter grass more common)
cargo run -p flicker-content --example build_propset -- \
  Alpha/content/staging/props/environment/GrassField GrassField Grass-Tall:1 Grass-Medium:1.5 Grass-Short:2
# 3. promote staging → package + append the manifest ledger
cargo run -p flicker-content --example promote_asset -- \
  props/environment prop Grass-Tall Grass-Medium Grass-Short
cargo run -p flicker-content --example promote_asset -- props/environment propset GrassField
```

## Interactions

- **Input signals:** None. This is a headless data pipeline — it captures no `ActionSignal`s
  and wires to no keys. The benches that host it own their own input.
- **Results / intents fired:** None. Functions return values (`Result<…>`, reports); the
  benches route them.
- **What it hands other crates:** `RigFile`/`Skeleton`/`Mesh` values (via `flicker-skeletal`),
  `RawModel` intermediates, `PackStats`/`GzifyStats`/`ImportSummary` reports, and — for the
  running game — content ON DISK: gz-at-rest `flicker.rig` files under `package/`, the
  `package.flk` mount, and the `manifest.json` ledger.
- **Model keys / Lua:** None — this crate never touches the per-frame Model or the Lua layer.
- **Threads / workers / async:** None here. The benches run bake/preview jobs on their own
  workers; every function in this crate is synchronous and blocking.

## Gates

The tests that enforce the contracts — run `cargo test -p flicker-content`. By name:

- **Root resolution + the `content.json` contract** — `sub_roots_derive_from_the_one_knob` / `a_relative_root_hangs_off_the_app_dir_and_an_absolute_one_wins` / `a_missing_or_invalid_config_falls_back_to_the_default` / `installed_layout_is_declared_by_an_exe_adjacent_content_json` / `the_undeclared_fallback_finds_the_repo_tree`. These live in `flicker-core` (where the `roots` module is defined and re-exported from) — run `cargo test -p flicker-core`, not `-p flicker-content`.
- `package::…::gzify_converts_once_and_is_idempotent` / `gzify_rejects_a_non_directory` — the gz-at-rest converter + verify-before-delete.
- `pack::…::repack_is_byte_identical` / `verify_round_trips_and_catches_corruption` / `refusals_fail_loud` / `mounted_package_serves_the_seam` / `symlinks_are_refused` — the store-only container, determinism, and the mount→seam path (ghost-root tested).
- `manifest::…::the_manifest_appends_reads_and_removes_rows` / `rows_normalize_to_forward_slash_logical_paths` — the promotion ledger + path normalization + loud-undo.
- `ops::…` (physical-path resolution, gz-preserving move, rename, whole-directory move, Replace-parks-and-restores, keep-both, conflict probe, 40-item atomic batch, partial-failure unwind, mkdir revert) — the reversible file ops.
- `scan::…::classify_by_extension_and_name` / `scan_a_meshy_folder_finds_one_riggable` / `multiple_meshes_trip_the_selection_guard_across_subdirs` / `bone_count_decides_skin_versus_prop` / `the_sniff_is_layout_agnostic` / `a_nested_manifest_classifies_on_either_separator` / `every_real_package_file_classifies` — ingest + package classification. **`every_real_package_file_classifies` is RED on any tree with a promoted propset — see Finding #1.**
- `bake::…::bake_skin_keeps_flesh_on_the_chain_and_off_the_root` / `staged_rig_round_trips_through_load` / `write_prop_copies_the_source_maps_beside_the_rig` / `write_garment_copies_the_source_maps_beside_the_rig` — skin defaults, the bake∘load∘bake identity, and texture-carrying.
- `propset::…::pick_lands_in_the_weighted_buckets` / `pick_distribution_tracks_weights` / `round_trips_through_the_gz_seam` / `validate_rejects_empty_and_nonpositive` — the variation set.
- `decimate::…` (level-0-verbatim, monotonic non-deduped buckets, closed-mesh-to-floor, keep-pct mapping), `fbx::…::quarter_turns_are_exact_and_stand_an_asset_up`, `baseline::…::the_baseline_is_symmetric_level_flat_and_at_stature`.

Real-data guards (`parses_the_real_female_base_character`, `golem_rebake_keeps_the_torso_core_on_the_chain`, `every_real_package_file_classifies`, …) SKIP when the content tree is absent and read `roots()` when present. The `humanbasea_*` tests are `#[ignore]`d (they need an import run first).

## Sharp edges

- **A freshly imported asset is invisible to the game until it is PROMOTED.** Benches write to
  `staging/`; the game reads only `package/`. "I imported it" and "it ships" are two events.
- **"Commit" is overloaded.** The Clayworks bench's final wizard step (writes to staging) is
  NOT a git commit. Say "the bench's Commit step" when you mean the button.
- **`default_reference()` vs `fitting_base()` are different bodies.** The first is the CONFORM
  canon (skeleton-only `GolemBaseSkeleton`, the reorient target); the second is the FITTING
  body (`GolemBase_Low`, a real mesh, what garments/props skin onto). Same golem lineage,
  opposite jobs.
- **`flat_color` prop colour is a POC.** It bakes the FBX base colour into the rig's per-material
  `color`, which deliberately conflicts with the Materials-Unification plan (the durable home
  for prop colour). The headless `import_prop` passes it; the bench Commit passes
  `None`. See Finding #2 — remove once prop colour comes through the materials system.
- **Inferred bones carry no weights.** After conform, fingers/twists/sockets resolve and rotate
  but deform nothing until each body's hand mesh is weighted to them.
- **The gz seam is one-way.** Writers always emit `.gz`; `read_*` accept either form. Move ops
  relocate the physical bytes as-is (never re-encode); normalizing a loose file to gz is
  `content-tool gzify`'s job, and it is idempotent.
- **The undeclared `roots()` fallback assumes the repo layout.** Outside an app that called
  `init_from_app_dir`/`set_content_root`, `roots()` climbs to this crate's own repo position.
  Tools and tests rely on that; a relocated checkout would need `set_content_root`.
- **A propset variant names a bare string with no in-crate resolver.** `PropSet` never checks a
  named prop exists (only weights > 0), and `pick()` hands back a bare name — a typo surfaces
  only where the consumer loads the mesh, in another crate. See Finding #3.
