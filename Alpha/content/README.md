# Alpha/content — the standardized content tree

The single, standardized home for flicker's **content assets**. Clients reference
content here instead of each carrying private copies (which is why the publisher
logos and shell Lua scripts used to be duplicated into every client).

## Layout — by type, then by subject

Top-level folders are the **data kinds** (mirroring the engine's internal data
structures *and* the eventual database tables / blob buckets). Within a
character-scoped kind, a **subject subfolder** (e.g. `katanami/`) keeps one
subject's assets together and avoids collisions.

```
Alpha/content/
├─ data/       periodic_table.json, materials.json          # element / material tables (was repo-root data/materials)
├─ rigs/       <char>/…                                      # skeleton + bind (flicker.rig)
├─ packs/      <char>/<name>.pack.json                       # animation / combat state graphs
├─ clips/      <char>/{In-Place,RootMotion,…}/*.json         # animation clip library
├─ flights/    <name>.flight                                 # camera cinematics (flicker-flight)
├─ meshes/     <char>/*.json                                 # mesh / morph definitions
├─ textures/   <char>/*.png                                  # texture maps
├─ scripts/    *.lua                                         # shared front-end Lua (splash/menu/settings/pause)
├─ resources/  ui_elements.json                              # shared UI layouts (json)
├─ assets/     *.png                                         # shared graphics (publisher / engine logos)
├─ bakes/      cluster_X_Y_Z.json.gz                         # baked voxel clusters (see below)
└─ characters/ <char>/…                                      # RAW export bundles (see below)
```

(Runtime user-state like a client's `settings.json` is **not** content and stays
out of this tree.)

### `ui_elements.json` is a shared library (one file)

`ui_elements.json` is a **flat map of named UI-element definitions**, not a per-client
file. There is **one** shared copy — `resources/ui_elements.json` — loaded into a
ScriptHost and exposed to Lua as `UI`; each client's scripts read the elements they
need by name (unused keys are ignored). Clients **add their elements to the shared
file** rather than shipping a private copy: the shell contributes
`modal`/`screens`/`settings`/`logo`/`loading`; flicker-csg contributes `hud`. If two
clients would want the same top-level key, **namespace the key** (e.g. `csg_hud` vs
`world_hud`) — not the file. Migrate the remaining clients (world, sol2, voxel-cluster)
the same way: merge their elements in, repoint them at `resources/ui_elements.json`,
delete the local copy.

(Genuinely distinct per-client *data* files — e.g. a client's `.flight` — stay
separate files, namespaced by a `<client>/` subfolder only if their filenames would
collide.)

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

## Raw export bundles (`characters/`)

Some content arrives as an **entangled raw export** that isn't cleanly typed yet.
The Katanami animation set is the first example: each of its `fbximport` JSON files
redundantly embeds the full skeleton + mesh, so there is no standalone rig or
clip-only file, and the loader (`flicker_skeletal::format::load_dir`) picks the rig
heuristically by mesh-vertex count.

These live **intact** under `characters/<name>/` — the **authored source of truth**.
Splitting one into clean `rigs/` + `clips/` + `textures/` + `meshes/` by-type assets
is a job for the content-processing pipeline (above), not a plain file move. Until
then a client loads the raw bundle directly (e.g. `flicker-paperdoll` and
`flicker-packeditor` both load `characters/katanami/`). Think of `characters/` as
*source*, the by-type folders as *processed / canonical* assets.

## Baked voxel clusters (`bakes/`)

The nine `cluster_X_0_Z.json.gz` files (a 3×3 cluster field) are **baked voxel
clusters** — the LOD-0 compressed cluster data (gzipped JSON), which *is* the durable
voxel data (layer 2 of the three-layer voxel model). They are produced by
`flicker_voxel::bake::BakedCluster` (contoured `Cluster` → gzipped JSON) — written by
the `examples/voxel-cluster` app (`src/{main,display}.rs::save_to_disk`) and by
`flicker-csg`'s `--bake` mode. Already a compressed format, so the closest thing to the
future binary-bake objective.

**`flicker-csg` now loads them from here** (`bakes/`, via its `bake_dir_path()` →
`../content/bakes`). The `examples/voxel-cluster` app still keeps its **own copy** in
`examples/voxel-cluster/bake/` — point it here too when voxel-cluster is next touched
(dedup).

## Migration is incremental

Content moves into this tree crate-by-crate as each client is hardened — not in one
big-bang sweep. Expect the tree to fill in over several passes.
