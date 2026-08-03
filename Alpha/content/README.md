# Alpha/content — the standardized content tree

The single, standardized home for flicker's **content assets**. Clients reference
content here instead of each carrying private copies (which is why the publisher
logos and shell Lua scripts used to be duplicated into every client).

## Layout — by type, then by subject

The top level splits by **how the data ships**: `package/` is everything the game
reads from disk at runtime on a player's machine (the future package-file
payload, each file individually gz-compressed at rest); `staging/` is processed
content awaiting promotion into it; `data/` is the RPC-tier tables;
`sensorium/resources` + `sensorium/scripts` are compile-/host-loaded UI
content; `source/` is authoring input that never ships. Within a
character-scoped kind, a **subject subfolder** (e.g. `katanami/`) keeps one
subject's assets together and avoids collisions.

```
Alpha/content/
├─ data/       periodic_table.json, materials.json, …        # element / material / string tables (RPC-tier, stays loose)
├─ package/                                                   # ═ THE RUNTIME PACKAGE ═ (text content gz at rest: <name>.<ext>.gz)
│  ├─ characters/ <char>/…                                    # rig/pack/clip/garment bundles + PNG textures (see below)
│  ├─ retarget/   clips/<lib>/{In-Place,RootMotion}/*.json.gz # retargeted clip libraries
│  ├─ flights/    <name>.flight.gz                            # camera cinematics (flicker-flight)
│  ├─ epochs/     <name>.epoch.gz                             # captured worlds (flicker-worldengine)
│  ├─ bakes/      cluster_X_Y_Z.json.gz                       # baked voxel clusters (see below)
│  └─ sensorium/  fonts/ assets/                              # Prism faces + logos/cursor (raw binaries, compile-embedded)
├─ staging/    <mirrors package/>                             # processed output awaiting PROMOTION (see below); never shipped
├─ sensorium/  scripts/ resources/                            # UI + input content (shell + HUD Lua, ui_elements.json)
└─ source/     <Set>/…                                        # RAW vendor exports + manifests (authoring input, never shipped)
```

(Runtime user-state like a client's `settings.json` is **not** content and stays
out of this tree.)

### The root is declared per executable, not hardcoded

An app names its content tree in a committed `content.json` beside its crate
manifest (`{ "content_root": "../content" }`); every sub-root above is
**derived** from that one knob, so `staging/` and `package/` can never point at
different trees. `flicker_content::roots()` is the accessor — a module asks it
where content is instead of spelling out a `CARGO_MANIFEST_DIR` climb. The app
installs the root once at startup (`init_from_app_dir`), and the library falls
back to this repo's own tree when no executable has declared one, so the
offline `content-tool` and the library tests still work.

## `staging/` — the promotion gate

The ingest benches (Clayworks, Loomforge, the retargeter) **write into
`staging/`, never into `package/`.** Content reaches the runtime package only
by an explicit **promotion** performed in the Content Manager bench, which also
records it in the package manifest. This splits two events that used to be one:
"I imported an asset" and "the asset ships."

Consequence worth knowing: **a freshly imported asset is not visible to the
running game until it is promoted.** Pressing *Commit* at the end of a Clayworks
import writes the baked rig into `staging/`, and the game only ever loads from
`package/` — so the asset appears in-game at PROMOTION time, not at import time.
That is the point of the tier, not a bug. (The bench's "Commit" step is its own
final wizard action — it has nothing to do with git.)

Staging is gz-at-rest exactly like `package/`, so a promotion is a plain byte
move through the shared seam — no transcoding, and never a second gz path.
See `Alpha/content/staging/README.md`.

### `ui_elements.json` is a shared library (one file)

`ui_elements.json` is a **flat map of named UI-element definitions**, not a per-client
file. There is **one** shared copy — `sensorium/resources/ui_elements.json` — loaded into a
ScriptHost and exposed to Lua as `UI`; each client's scripts read the elements they
need by name (unused keys are ignored). Clients **add their elements to the shared
file** rather than shipping a private copy: the shell contributes
`modal`/`screens`/`settings`/`logo`/`loading`; flicker-csg contributes `hud`. If two
clients would want the same top-level key, **namespace the key** (e.g. `csg_hud` vs
`world_hud`) — not the file. Migrate the remaining clients (world, sol2, voxel-cluster)
the same way: merge their elements in, repoint them at `sensorium/resources/ui_elements.json`,
delete the local copy.

(Genuinely distinct per-client *data* files — e.g. a client's `.flight` — stay
separate files, namespaced by a `<client>/` subfolder only if their filenames would
collide.)

## Asset naming — the internal standard (2026-07-16)

**Vendor names never enter the content tree.** Generators emit unstable, non-unique,
unreadable stems — `Meshy_AI_Lonely_Muse_Top_Duste_0716234729_texture` is vendor-prefixed,
truncated mid-word, and timestamped. Nothing downstream should ever see one. The asset
**processing pipeline is where names, tags and labels are unified** — do it there, once.

**Asset name.** `PascalCase-Hyphenated`, named for **what the object is**, unique across the
tree, no vendor prefix / timestamp / `_texture` / `_Game_Mesh` noise:
`Corset-Duster`, `Hem-Pants`, `Foot-Boots`, `Hand-Gloves`, `Neck-Pendant`, `Katana`,
`Katana-Sheath`, `Dagger`, `Dagger-Sheath`. The file is `<AssetName>.json`.

**Textures.** `<AssetName>_<Map>.png`, where `<Map>` is from the fixed internal vocabulary:
`BaseColor` · `Normal` · `Roughness` · `Metallic` · `Emit` · `AO` · `ORM` (packed
occlusion/roughness/metal).

> **Named by ROLE, not by filename** (2026-07-17). `<Map>` comes from which **material
> input** the texture feeds (Base Color → `BaseColor`, Normal → `Normal`, …) — see
> `io_scene_flicker_rig.save_material_textures`, THE texture path for every FBX converter.
> This is robust where a filename heuristic isn't: an FBX with **embedded** textures exposes
> them as `Image_0`, `Image_3`, … (no vendor name at all), and role-driven naming still lands
> them correctly, uniquely, and namespaced. (Caught 2026-07-17 on the hair, whose Meshy export
> embedded its maps.) An image feeding several inputs (a packed map) is written once; a
> multi-material mesh namespaces by slot (`<AssetName>_m<i>_<Map>`).

> **The `<AssetName>_` prefix is load-bearing, not cosmetic.** Vendors name every item's
> maps IDENTICALLY, so unprefixed they collide in one output dir and the first item
> silently wins — every other piece renders with its texture, with no error anywhere.
> (Caught 2026-07-16: all five Muse002 pieces resolved to the older Muse001 albedo.)

**The mapping is RECORDED, not typed.** Each source bundle carries a
`manifest.json` beside it (`Alpha/content/source/<Set>/manifest.json`) giving, per item, a
`match` (a stable SUBSTRING of the vendor stem), the internal `name`, and its `slot`. It is
versioned with the repo and is the only source→internal record; ad-hoc CLI renames are not
acceptable because nothing remembers them. **An FBX with no manifest entry is SKIPPED, never
auto-named** — an unnamed asset in the tree is worse than an absent one. Match order matters
(`Katana_Saya` before `Katana`).

Converter: `tools/blender/convert_meshy_prop.py --manifest <set>/manifest.json`.

> **Vendor exports carry NO usable scale.** Meshy normalises every asset's longest axis to
> ~1.899 units about the origin — a katana and a pendant come out the same size — and its
> "resize to height" only re-defaults. Geometry is therefore stored RAW and a piece's real
> scale/orientation/position is **authored fit data** recorded separately. Never bake a
> guessed scale into an asset.

## Two asset classes, two destinations

- **Structured definitions** — `data/`, `rigs/`, `packs/`, `clips/`, `flights/`,
  `meshes/` (JSON today). These are **database-migration candidates**: the material
  tables already load through a swappable `flicker_materials::TableSource`
  (JSON → DB); rig / pack / clip / flight definitions follow the same pattern when
  TheOracle backend is available.
- **Physical blobs** — `textures/` (and future audio / fonts / compiled meshes).
  These go to **object / blob storage**, not relational rows.

## Architectural objective (not built yet)

A future **content-processing pipeline** will bake these authored JSON/PNG sources
into **compact, compressed binary** runtime formats (smaller, faster to load), with
the authored files staying the source of truth. This is the content cluster's
planned "resource manager + loaders" (see `docs/crate-clusters.md`). Recorded so it
isn't lost — do not build it yet.

## Raw export bundles (`package/characters/`)

Some content arrives as an **entangled raw export** that isn't cleanly typed yet.
The Katanami animation set is the first example: each of its `fbximport` JSON files
redundantly embeds the full skeleton + mesh, so there is no standalone rig or
clip-only file, and the loader (`flicker_skeletal::format::load_dir`) picks the rig
heuristically by mesh-vertex count.

These live **intact** under `package/characters/<name>/` — the **authored source of truth**.
Splitting one into clean `rigs/` + `clips/` + `textures/` + `meshes/` by-type assets
is a job for the content-processing pipeline (above), not a plain file move. Until
then a client loads the raw bundle directly (e.g. `flicker-paperdoll` and
`flicker-packeditor` both load `characters/katanami/`). Think of `characters/` as
*source*, the by-type folders as *processed / canonical* assets.

## Baked voxel clusters (`package/bakes/`)

The nine `cluster_X_0_Z.json.gz` files (a 3×3 cluster field) are **baked voxel
clusters** — the LOD-0 compressed cluster data (gzipped JSON), which *is* the durable
voxel data (layer 2 of the three-layer voxel model). They are produced by
`flicker_voxel::bake::BakedCluster` (contoured `Cluster` → gzipped JSON) — written by
the `examples/voxel-cluster` app (`src/{main,display}.rs::save_to_disk`) and by
`flicker-csg`'s `--bake` mode. Already a compressed format, so the closest thing to the
future binary-bake objective.

**`flicker-pocclusters` loads them from here** (`package/bakes/`, via its `bake_dir_path()` →
`../content/package/bakes`). The `examples/voxel-cluster` app still keeps its **own copy** in
`examples/voxel-cluster/bake/` — point it here too when voxel-cluster is next touched
(dedup).

## Migration is incremental

Content moves into this tree crate-by-crate as each client is hardened — not in one
big-bang sweep. Expect the tree to fill in over several passes.
