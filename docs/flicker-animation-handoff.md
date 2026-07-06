# flicker-animation — skeletal animation viewer POC (handoff)

Status as of 2026-07-05. Covers `examples/flicker-animation` (the Rust viewer,
"Part 2") and the shared **Format Contract** it agrees on with the C++ `fbximport`
converter ("Part 1", which lives in the ClayEngine solution, not this repo).

This is a **POC / pre-alpha** sandbox to play with the internal model/animation
format and prove the animation runtime before promoting anything to an Alpha crate.
Consistent with the named engine need for a "granny-tier" animation system (MCP
memory `flicker` 386AC689) and the Katanami→FBX→internal-format direction recorded
this session (MCP memory `flicker` 50EA9C0F).

---

## What exists (built this session)

**Part 1 — `fbximport` (C++17, FBX SDK 2018.1)** — lives at
`C:\Users\aaron\Repos\VoxelmancyProject\ClayEngine\FbxImport\` (standalone vcxproj,
NOT registered in `ClayEngine.sln`; mirrors `TestingConsole`). A disposable console
tool that reads the extracted Katanami FBX and emits the Format Contract JSON. It is
a bootstrap crutch, not a general pipeline. Validated against the real assets:
94-bone rig + 11 925-vertex skinned mesh from `Katana_Morph_Color1.FBX`, and 13
baked-at-60 Hz clips from the `Animation\In-Place\*` folders. Output lands in
`FbxImport\out\*.json`.

**Part 2 — `examples/flicker-animation` (Rust)** — the viewer. **Slices 1–3 are
done.**
- **Slice 1** — skeleton-wireframe viewer: loads the rig JSON, samples the selected
  clip at the current tick on the CPU, accumulates global bone transforms (forward
  kinematics), draws parent→child bone segments through the **existing lines
  pipeline**. Proves the CPU-authoritative pose layer in isolation.
- **Slice 2** — CPU-skinned mesh: each frame builds the skinning palette
  (`global × inverse_bind` per bone), skins the 11 925 vertices on the CPU (4-influence
  LBS), and re-uploads the deformed mesh. Per-frame skin + re-upload (free previous,
  upload new) is fine for one POC character.
- **Slice 3** — textured, per-submesh. The mesh is segmented into material submeshes
  (see "Material segmentation" below); each submesh is drawn with its albedo texture
  via the **new reusable `flicker-render` textured mesh pipeline**, or flat gray where
  no texture maps. `M` toggles the mesh, `T` toggles textures (→ flat gray), `B`
  toggles the skeleton overlay.

**Reusable textured mesh pipeline (`crates/flicker-render/src/pipeline_mesh_textured.rs`
+ `shaders/mesh_textured.wgsl`).** Built to be general (character skins now,
voxel-cluster surfaces later), fully **additive** — the existing `MeshVertex` /
`draw_mesh` path is untouched. `TexturedVertex { position, normal, uv }`;
`Renderer::upload_textured_mesh` / `free_textured_mesh` /
`draw_textured_mesh(mesh, texture, model, opts)`. Bind group 0 = camera+per-draw+scene
(same uniforms/lighting as the flat mesh shader); bind group 1 = albedo `texture_2d` +
**linear** sampler (a per-texture `mesh_bind_group` built in `Renderer::load_texture`,
mirroring the billboard bind group). Alpha-tests (`texel.a < 0.5` → discard) to cut
hair-card edges. Shares the 3D depth pass; renders right after the flat mesh pass.

Modules in the example (all inside it — no library crate yet, by design):

| file | role |
|---|---|
| `src/format.rs` | serde types for the `flicker.rig` contract (incl. `submeshes` + `materials`) + `load_dir()`; picks the rig, resolves clip track bone **names** → skeleton indices, computes the source→engine `world` matrix + orbit radius. |
| `src/pose.rs` | the CPU-authoritative pose layer: `sample_local_poses(clip, tick)` → per-bone local TRS; `global_transforms()` accumulates parent→child. |
| `src/skin.rs` | `palette()` (`global × inverse_bind`) + `skin()` (4-influence CPU LBS → `SkinnedVertex` position+normal; the viewer slices this per submesh into textured/flat GPU vertices). |
| `src/main.rs` | the `App` (viewer): orbit camera, playback controls, per-submesh textured/flat draw, texture loading (`image` crate), HUD. Plus three skip-safe headless tests (load+resolve, finite pose, rest-skin-matches-bind). |

Run: `cargo run -p flicker-animation` (the **user** runs the window and verifies;
this crate never launches it). Controls: drag = rotate · wheel = zoom · Space =
play/pause · ←/→ = step tick · ↑/↓ = cycle clip · PgUp/PgDn = raise/lower the model
(vertical reframing — the root sits at the feet, so this lifts the camera's view up
the body) · M = toggle skinned mesh · T = toggle textures · B = toggle skeleton
overlay · R = reset · Esc =
quit.

`cargo build -p flicker-animation`, `cargo clippy -p flicker-animation --all-targets`,
and `cargo test -p flicker-animation` (3 tests) are all clean.

---

## The Format Contract (the shared seam)

Canonical/debug form is JSON (human-readable, diff-able). One mesh FBX → one rig
JSON (`skeleton` + `mesh`, empty `clips`); one clip FBX → one clip JSON (`clips`,
empty `mesh`). The Rust loader stitches them by bone name.

```jsonc
{
  "format": "flicker.rig", "version": 1,
  "source": {
    "file": "Katana_Morph_Color1.FBX", "fbx_version": "7500",
    "source_axis": "Z_up", "source_unit": "cm", "applied_transform": "none",
    "textures": ["Katanami_Body_BaseColor.TGA", "..."]
  },
  "skeleton": { "bones": [
    { "name": "root", "parent": -1,
      "local": [/*4x4 row-major rest pose, 16 floats*/],
      "inverse_bind": [/*4x4 row-major, 16 floats*/] }
  ] },
  "mesh": {
    "vertices": [ { "p":[x,y,z], "n":[nx,ny,nz], "uv":[u,v],
                    "joints":[b0,b1,b2,b3], "weights":[w0,w1,w2,w3] } ],
    "indices": [0,1,2],
    "submeshes": [ { "material":0, "start":0, "count":984 } ],  // index ranges by material
    "materials": [ { "name":"Mat_Body_Color_1", "slot":"Cloth",
                     "base_color":"Katanami_Body_BaseColor.png" } ]  // "" = flat/untextured
  },
  "morphs": [],
  "clips": [ { "name":"Attack_1", "tick_rate_hz":60, "duration_ticks":61,
    "tracks": [ { "bone":"spine_01",
      "keys": [ { "t":0, "T":[x,y,z], "R":[x,y,z,w], "S":[x,y,z] } ] } ] } ]
}
```

Rules: **matrices are 16 floats, row-major storage, FBX-native ROW-VECTOR
convention** — a point transforms as `p * M`, so translation sits in the **last row**
(`m[12..15]`), NOT the last column. A column-vector engine (glam: `M * p`) must decode
these as `Mat4::from_cols_array(&m)` (i.e. treat the row-major floats as columns —
that transpose *is* the row→column-vector conversion). **Do not additionally
transpose.** Rotations are quaternions `[x,y,z,w]`; `joints`/`weights` always length 4,
zero-padded, weights normalized to 1.0; `tracks[].bone` is a **name** resolved to a
skeleton index by the loader (never assume clip bone order == skeleton order); dense
keys (one per tick), no reduction. The clip `T/R/S` keys are decomposed values (not
matrices) and are convention-independent.

> **Convention bug fixed (2026-07-05).** The contract originally said only "row-major"
> without pinning the vector convention. The converter emits row-vector (FBX) matrices;
> the viewer first decoded them as column-vector (an extra `.transpose()`), which put
> translation in the wrong place. Skeleton playback still looked right (it uses the
> `T/R/S` keys), but skinning exploded (it uses the `inverse_bind` matrix) and the
> orbit framing collapsed to the origin (it uses the rest-pose matrices). Fix:
> `format.rs::mat4_from_contract` decodes without the transpose. Regression guard: the
> `skinning_rest_matches_bind` test asserts rest-pose skinning reproduces the bind mesh.

---

## Decisions that landed (and why)

- **CPU-authoritative pose.** The pose (local + global bone transforms) is computed
  on the CPU and is the single source of truth. This is the layer Slice 1 validates.
  It matches existing project canon (memory 386AC689): TAE-style event timeline as
  authority, a logical pose on the CPU for hitboxes, palette skinning on the GPU
  later. The eventual GPU split is: CPU owns the pose/palette, a GPU vertex shader
  reads a bone-matrix palette storage buffer to skin — **deferred to alpha**.

- **The converter emits SOURCE space; the viewer normalizes.** UE source is Z-up,
  centimetres. The converter records `source_axis="Z_up"`, `source_unit="cm"`,
  `applied_transform="none"` and emits everything unmodified (it does NOT call the
  FBX SDK's axis/unit `ConvertScene` — a known cause of subtly-wrong rigs because it
  only partially rewrites the scene). The viewer applies **one** world matrix
  `translate(-center) · R(Z-up→Y-up) · scale(0.01)` (`format.rs`), keyed off the
  `source_*` fields. Rationale: minimal converter surgery → the rig, skin, and
  animation stay internally consistent (that consistency matters more than matching a
  target axis), and the contract's `source_*` fields carry exactly the info the
  consumer needs. Geometric transforms ARE baked into vertices at import; pre/post
  rotation + pivots are folded in by baking animation via `EvaluateLocalTransform`.

- **Clip tracks keyed by bone name, resolved by the loader.** The clip FBX carries
  its own skeleton copy; the rig's authority is the mesh FBX. The Katanami clips
  carry **101** tracks = the 94 deforming bones **+ 7 UE IK helper bones**
  (`ik_foot_*`, `ik_hand_*`, `ik_*_root`) the deforming rig lacks. Name-based
  resolution drops those 7 automatically (they surface as `ResolvedClip::unresolved`,
  logged once per clip at startup — expected, harmless).

- **Clip name = input file stem** (`Attack_1`, `Idle_nonWeapon`, …). UE exports every
  take under the shared AnimStack name "Unreal Take", which is useless for the clip
  picker, so the converter falls back to the file stem.

- **Viewer implements `App` directly** (mirroring `examples/mesh-smoke`), not the
  `Scene`/`SceneManager` stack — a single-purpose viewer doesn't need the scene/HUD
  machinery.

- **Orbit camera is mirrored, not depended-on.** The umbrella `flicker` crate does
  not re-export `flicker-world`, so the small `OrbitCam` logic (drag→yaw/pitch,
  wheel→distance, `Camera::orbit`) is copied into `main.rs`.

- **Toolset v143** for the C++ project (v142 is not installed on this box; the CRT is
  binary-compatible so the vs2015-built `libfbxsdk-md.lib` links fine under /MD).

---

## Material segmentation & the texture mapping (Slice 3)

**UV origin convention (load-bearing).** UVs are emitted **top-origin** (V increases
downward, wgpu-native). FBX authors UVs **bottom-origin** (V up, Maya/OpenGL), so the
converter flips `v → 1 - v` at export (in `extractMesh`) — consumers sample directly,
no re-flip. **Symptom if this is wrong: everything vertically mirrored** — the face
maps onto the skirt, the back panel onto the chest, etc. (This is what the first
textured run showed before the flip was added.)

**The converter now segments the mesh by material.** `extractMesh` reads the
per-polygon `FbxLayerElementMaterial` (eByPolygon/eIndexToDirect), buckets triangles
by material, and emits them **grouped** so each material occupies a contiguous
`submesh` range in `indices` (= a range in `vertices`, since the list is
non-deduplicated with sequential indices). The Katanami mesh has **6 material slots**;
triangle counts 328 / 2136 / 104 / 370 / 873 / 164.

**The FBX material→texture wiring is UNRELIABLE for this asset** — two model versions
are blended into the source content folder, `Mat_Hair_Color_2` points at the *Body*
texture, and the 3 biggest slots are unnamed "Fbx Default Material" with no texture ref.
So the mapping is driven by the **Unreal material-slot names** the user read off
(slot order 0–5 = **Cloth, Body, Wine, Hair, Head, Eyes**), via a name-keyed table
`mapMaterial()` in `main.cpp` (**PROVISIONAL** — edit + rebuild + re-run to correct):

| slot | FBX name | tris | → base_color (PNG) | note |
|---|---|---|---|---|
| 0 Cloth | Mat_Body_Color_1 | 328 | Katanami_Body_BaseColor | slot uses Body maps (screenshot) |
| 1 Body | Fbx Default Material 1 | 2136 | Katanami_Body_BaseColor | |
| 2 Wine | Fbx Default Material 2 | 104 | *flat wine-red via `color`* | user: wine-red flat-shaded |
| 3 Hair | Mat_Hair_Color_2 | 370 | Katanami_Hair_BaseColor | |
| 4 Head | Fbx Default Material 4 | 873 | Katanami_Body_BaseColor | best-guess |
| 5 Eyes | Mat_Eyes_Inst | 164 | Eyes_Albedo | FBX-confirmed |

Materials also carry an optional flat **`color:[r,g,b]`** (0..1), used when `base_color`
is empty — for untextured props (Wine → wine-red). The viewer packs it into the flat
mesh pipeline's direct-RGB material escape (`material & 0xFFF == 0xFFF`), so an exact
solid colour renders with no new pipeline.

> **OPEN with the user:** (a) the textures on disk may be the *wrong model version*
> (user's caution) — the albedo may not match this mesh's UVs; (b) the Wine and Head
> mappings are guesses. Correct in `mapMaterial()` (or supply the right TGAs), re-run
> `FbxImport.exe <mesh> out\...json`, and re-copy into `assets/`.

**TGA→PNG converter.** `fbximport --textures <out_dir>` converts the referenced albedo
TGAs to PNG via vendored stb (`stb_image.h` / `stb_image_write.h` / `stb_impl.cpp`;
`STB_*_IMPLEMENTATION` in the one TU, SDLCheck off for it in the vcxproj). Produced:
`Katanami_Body_BaseColor.png` (4096²), `Katanami_Hair_BaseColor.png` (4096²),
`Eyes_Albedo.png` (1024²). The viewer loads these via the `image` crate in `App::init`.

**Asset refresh loop:** `FbxImport.exe <mesh.fbx> out\Katana_Morph_Color1.json` +
`FbxImport.exe --textures out` → copy `out\Katana_Morph_Color1.json` + `out\*.png` into
`examples/flicker-animation/assets/`.

## PBR maps (Slice 3b, 2026-07-05)

Normal / roughness / metalness / AO are now wired end-to-end on top of the albedo path,
so the character reads as proper PBR (surface relief from the normal maps, AO on ambient)
and the katana blade gets a reflective-steel metal/rough response. **Fully additive** —
the flat `MeshVertex`/`draw_mesh` path and the albedo-only `draw_textured_mesh` are
untouched; all other flicker-render consumers build unchanged.

**Which maps, and the atlases.** Each `_BaseColor.png` albedo derives its map set by
suffix substitution (`BaseColor → Normal/Roughness/Metalness/AO`). Body atlas →
cloth/body/head; Hair atlas → hair **and the katana prop** (its FBX ref was wrong, so
`main()` overrides albedo *and* all four maps to Hair). `Eyes_Albedo.png` doesn't follow
the convention → no PBR maps (albedo only). Wine is flat-coloured → no maps. The 8 new
PBR TGAs (`Katanami_{Body,Hair}_{Normal,Roughness,Metalness,AO}.TGA`) are converted to
PNG by `fbximport --textures` alongside the albedos.

**Contract.** Each `mesh.materials[]` entry now carries `"normal"`, `"roughness"`,
`"metalness"`, `"ao"` (PNG basenames, or `""`) in addition to `base_color`. Derived in
the converter (`derivePbrMaps` in `main.cpp`) for BOTH the character `mapMaterial` path
and the static-prop diffuse-ref path, and parsed by `format.rs::Material`.

**sRGB vs LINEAR (load-bearing).** Albedo is sRGB **colour** data → `load_texture`
(`Rgba8UnormSrgb`). The four maps are **LINEAR** data (tangent-space normals + scalar
roughness/metalness/AO) → `load_texture_linear` (`Rgba8Unorm`, added this slice). Loading
a normal map as sRGB would gamma-shift it and bend the lighting. *Symptom if wrong:*
washed-out / oddly-tinted relief.

**Tangents.** `TexturedVertex` gains `tangent: [f32;4]` (`xyz` + handedness `w`). The
mesh is **non-deduplicated** (each triangle = 3 sequential unique vertices), so the
example computes ONE tangent per triangle from the 3 positions + UVs (`dP/dUV` solve,
`build_textured_verts` in `main.rs`) and assigns it to all 3 corners — no cross-vertex
averaging. Done for both the CPU-skinned character submeshes and the katana upload. The
shader re-orthonormalizes (Gram-Schmidt) against the interpolated normal and builds the
bitangent as `cross(N,T) * w`.

**The pragmatic BRDF** (`shaders/mesh_textured.wgsl`). One pipeline, **default 1×1
textures** for any omitted map (flat normal `(128,128,255)` / white roughness+AO / black
metalness), so an albedo-only draw (Eyes, or a caller using `draw_textured_mesh`) still
works as a matte dielectric. Per fragment: sample albedo (sRGB) + the 4 maps (linear);
perturb the world normal through the TBN; then keep the existing **sun/moon/point
Lambertian diffuse**, multiplied by `(1 - metalness)` and by AO on ambient; and add a
**pragmatic specular** — a normalized-ish Blinn-Phong lobe whose exponent rides
smoothness (`1 - roughness`, ~2 rough → ~2048 mirror) and whose colour is
`F0 = mix(0.04, albedo, metalness)` (white dielectric / albedo-tinted metal), plus a
small ambient-specular so metal reads reflective away from a direct highlight. Not full
Cook-Torrance — "good-enough reflective steel," kept stable and not blown-out. The old
`gloss` sheen term (`flags.y`) is preserved on top.

**Where each piece lives.**
- **Converter:** `Material` struct + `derivePbrMaps` + `writeJson` + `runTextures`
  (`FbxImport/main.cpp`).
- **flicker-render:** linear upload (`texture.rs::from_rgba8_linear`,
  `renderer.rs::load_texture_linear`); the tangent attribute, combined material bind
  group (albedo + 4 maps + 1 sampler in ONE group — packed to stay within the default
  `max_bind_groups=4`), default 1×1 map textures, and `PbrMaps` +
  `draw_textured_mesh_pbr` (`pipeline_mesh_textured.rs`, `renderer.rs`); the PBR fragment
  shader (`shaders/mesh_textured.wgsl`). New export: `PbrMaps`.
- **Example:** `format.rs::Material` fields; `build_textured_verts` (tangents) +
  `resolve_maps` + the sRGB/linear texture loading in `init` + the `draw_textured_mesh_pbr`
  calls (character + katana) in `main.rs`. `T` still toggles textures (suppresses the
  whole map set → matte).

**Bind-group note.** To stay within wgpu's default `max_bind_groups` limit of 4, the 5
material textures + shared sampler live in a **single** bind group (group 1), built
**per-draw** in the pipeline's `prepare` from each texture's view (defaulting omitted
maps). The legacy single-texture `mesh_bind_group` on `LoadedTexture` is still built by
`load_texture[_linear]` (kept for API/back-compat) but is no longer bound by this
pipeline.

**Visual-correctness caveats (user verifies the window; the agent can't).** Confirm the
blade reads reflective/metallic and the kimono/cloth shows surface relief. If the relief
looks **inverted** (bumps read as dents), the normal-map **green channel** likely needs
flipping (DirectX vs OpenGL convention) — flip `tn.y` in the shader. If lighting looks
subtly wrong along UV seams, the **tangent handedness** (`w`) may need inverting. These
are one-line changes and can't be validated headless.

## Props & weapon attach (2026-07-05)

**Converter now handles STATIC meshes.** `convertFile` emits geometry for any
mesh-bearing FBX (skinned OR static), and only bakes clips when there's a skeleton
(a static prop can carry an empty anim stack — skipped). The character material
mapping (`mapMaterial`) is applied **only to skinned meshes**; static props keep their
raw material name and stay untextured (FBX default names like `Mat_Body_Color_1`
collide across assets — the wine bottle's material shares the character's Cloth name),
so the consumer assigns a flat colour/texture per prop.

**Assets imported** (`KatanamiExtraction\Meshes\`, converted to `FbxImport\out\`):
`Mesh_Katana.FBX` (176 tris, 1 material, no texture) and `Mesh_Wine_Bottle.FBX`
(104 tris). Katana JSON copied into the example `assets/`; the bottle is imported but
not yet rendered (reserved for the pickup demo).

**Prop texturing.** Static props derive their `base_color` from the **FBX diffuse
texture reference** (converter reads `sDiffuse` → `FbxFileTexture`, as `.png`). The
katana's material `Mat_Hair_Color_1` references `Katanami_Body_BaseColor` — so the blade
/guard/handle/twine are packed into the **body atlas** (already loaded), selected by UV
channel 0 (`UVmap_0`; the katana has 4 UV channels — 0/1/2 + LightMapUV). No katana
texture exists on disk. **OPEN:** the FBX diffuse refs were unreliable for the character
(all "Hair" mats pointed at Body too), so if the katana texture looks wrong, it likely
wants `Katanami_Hair_BaseColor` instead — a one-line mapping swap. Confirm the atlas from
Unreal's katana material.

**Weapon attach (rigid).** The rig has `Weapon_R` / `Weapon_L` socket bones. The viewer
loads the katana via `format::load_mesh` (geometry only), uploads it once (per submesh:
textured via the albedo, else flat steel), and each frame draws it at
`world × globals[Weapon_R]` — the socket's animated global transform. Grip offset is
identity for now (tune if the blade sits wrong). `K` toggles equipped. This is the
substrate for the eventual **equip / pickup state transition** (start unequipped →
weapon on ground → collision → to hand) — deferred pending the animation state-machine
design (below).

## Scope notes / things surfaced (NOT built — the user drives next)

- **PBR maps are now wired** (2026-07-05). Albedo + normal/roughness/metalness/AO are
  all sampled by the textured pipeline; the character reads as proper PBR and the katana
  blade gets a reflective-steel response. See "PBR maps (Slice 3b)" below for the full
  contract + pipeline detail. (Was: "only base color is wired.")
- **No mipmaps** on the albedo (linear mag/min, `mipmap_filter: Nearest`) → some
  shimmer at distance on the 4K maps. A mip chain is a follow-up.
- **Per-frame re-upload per submesh.** Fine for one POC character; the GPU-skinning /
  static-vertex-buffer optimization is an alpha concern.
- **Renderer constraint recorded (MCP `flicker` invariant F1F14C20):** contoured voxel
  meshes have inverted UV/normal orientation on the 3 negative-axis (`---`) face
  directions (QEF side effect); must be handled by design when this textured pipeline
  is wired to voxel geometry — NOT via VoxelFarm's duplicate-UV-buffer hack. Plus a
  related back-side-vector quad-gap issue. Record-only for now.
- **MCP memory synced:** Slice-3 decisions are in the `flicker` store — spec 1E75AEBA
  (contract + submeshes/materials), decision 04BE5862 (viewer POC + textured pipeline),
  invariant F1F14C20 (the UV/normal constraint).

## Deferred to alpha (do NOT build now)

- GPU skinning pipeline in `flicker-render` (vertex shader reads a bone-matrix palette
  storage buffer — the CPU-authoritative-pose / GPU-compute split).
- Keyframe compression (dense one-key-per-tick today).
- **Crate extraction**: promote `format`/`pose`/`skin` into a real crate — likely
  `Alpha/flicker-skeletal` (an `Alpha/` crates dir was established this session,
  memory 50EA9C0F). **Naming is the user's call.**
- **Mipmaps** on the textured pipeline (the 4K maps still use `mipmap_filter: Nearest`
  → distance shimmer). PBR (normal/roughness/metalness/AO) is now **built** (see "PBR
  maps (Slice 3b)"); a proper mip chain is the remaining follow-up. The textured
  pipeline is the intended reuse point for voxel-cluster surface texturing.
- Hitbox/hurtbox capsule binding + the TAE-style event-timeline layer (the eventual
  combat spine — out of scope for the viewer).
