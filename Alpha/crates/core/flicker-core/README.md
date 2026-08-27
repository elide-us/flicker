# flicker-core

The foundation crate of the `core` cluster — everything else in the workspace sits on top of
it. It answers three questions that all content handling depends on:

1. **Where does content live?** The **content-roots data interface** (`roots`): a module asks
   where a tree is, it never hardcodes a path. This is the single home of that answer.
2. **How is content read and written on disk?** The **gz-at-rest + package seam**
   (`compression` + `mount`): every text content file lives gzip-compressed at rest, and a
   shipped build serves it all from one mounted `package.flk` container — both made transparent
   behind one set of read/write functions, so a loader never knows or cares which form it got.
3. **How do I gzip an in-memory buffer?** The **generic gzip helpers** (`compress_gzip` /
   `decompress_gzip` / `is_gzipped`) — the workspace-wide compressor for bake files, wire
   packets, and any byte stream where size matters.

This crate is *below* the game's UI world: it knows nothing of the Model, of signals, of scenes
or Lua. Those layers are built above it and reach content through the functions documented here.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Vocabulary (flicker words used below)

- **content tree** — the folder (`Alpha/content/`) holding all game content, in tiers.
- **content root** — that folder's path. Declared ONCE per executable (see `content.json`);
  everything else is derived from it.
- **staging tier** (`staging/`) — where the content benches WRITE processed output. Not shipped.
- **package tier** (`package/`) — the runtime read set: what a shipped game actually reads.
- **gz-at-rest** — every text content file is stored individually gzip-compressed as
  `<name>.<ext>.gz`. Readers name the **logical** path (`intro.flight`); the seam finds the
  at-rest twin (`intro.flight.gz`). A **physical** path is what is really on disk.
- **the seam** — the one set of read/write functions (`compression`) that makes gz-at-rest and
  the package mount transparent, so a caller only ever names logical paths.
- **`package.flk`** — the shipped package tier as ONE store-only zip container, **mounted**
  read-only over `package/` at startup. A dev tree has no `.flk` and reads loose files instead.
- **data-interface rule** — code ASKS `roots()` where content is; it never spells out a path.

## Where it sits

- **Builds on:** `flate2` (the gzip backend; its `Compression` level type is re-exported so a
  caller needn't add `flate2` just to pick a level), `zip` (store-only, no compression backends
  compiled in — it *cannot* deflate), `serde` + `serde_json` (`content.json`), `tracing`,
  `thiserror`. (`glam` and `bytemuck` are declared but unreferenced — see Finding 3.)
- **Used by:** roughly seventeen workspace crates and the `flicker` umbrella (which re-exports
  this crate as `flicker::core`). The data interface alone — `roots()` — has 17 call sites
  spanning the content pipeline (`flicker-content`), the frontend (`flicker-shell`,
  `flicker-widgets`), most scene crates (`assetpipeline`, `controllertester`, `loomforge`,
  `pocclusters`, `quartermaster`, `sablework`, `solarbirth`, `jiggle`), the world baker
  (`flicker-voxel`), and `prism-alpha` at startup.
- **One implementation, two doors.** `flicker-content` **re-exports** this crate's `roots` and
  `compression` under its own name — `flicker_content::roots` and `flicker_content::package`.
  Both doors reach the exact same code. Crates already pulling in the content pipeline use that
  door; crates that only need paths and bytes (UI, scenes) depend on `flicker-core` directly and
  skip the FBX/skeletal weight. See [`flicker-content`'s README](../../content/flicker-content/README.md).
- **Reads from the content tree:**
  - `<app_dir>/content.json` — the per-executable root declaration. Read once by
    `init_from_app_dir`. Absent → the default `../content` layout (the normal dev case);
    malformed → a `warn` + default (a bad config never blocks startup).
  - `<content_root>/package.flk` — the shipped container. Mounted over `package/` if present;
    a present-but-**unreadable** one is fatal (panics). Absent → loose-tree behaviour, unchanged.

## Public API

Grouped by concern. Import paths are exact: the `roots` and `compression` modules are reached
path-qualified (`flicker_core::roots::…`, `flicker_core::compression::…`); only the buffer gzip
helpers are re-exported at the crate root.

### Where content lives — the content-roots data interface (`flicker_core::roots`)

| Item | What it is for | The one thing to know |
|---|---|---|
| `roots() -> ContentRoots` | The process's resolved content roots — call it wherever you need a path. | Reads process-global state set by `init_from_app_dir`/`set_content_root`. If neither ran (library tests, the offline `content-tool`, `examples/`), it falls back to a compile-time climb to this crate's own repo position — the ONE remaining hardcoded content climb, consolidated here instead of in ~50 callers. See Sharp edges. |
| `ContentRoots` | One root plus the sub-roots DERIVED from it. | Accessors: `.root()` `.package()` `.staging()` `.data()` `.sensorium()` `.source()`. Sub-roots are derived from the one root, so `staging/` and `package/` can never name different trees. `.sensorium()` is the top-level authoring/host-loaded UI tree (`scenes/ scripts/ resources/`), NOT the shipped `package/sensorium/` (assets/fonts) — see Sharp edges. Cheap to clone; `new(root)` / `resolve(app_dir, cfg)` build one. |
| `ContentConfig { content_root: String }` | The committed `content.json`, deserialized. | `load_from(app_dir)` is best-effort: a missing file is the normal default case (`../content`), a malformed file warns and defaults. Relative `content_root` hangs off the app dir; absolute is taken as-is. Every field is `#[serde(default)]` so an older file still loads — which also means a mistyped key is silently ignored (Finding 4). |
| `CONTENT_CONFIG_FILE: &str` | `"content.json"`. | The declaration filename, beside the app's manifest / the installed exe. |
| `init_from_app_dir(app_dir) -> ContentRoots` | Startup entry point: read `content.json`, install the resolved root for the process, and mount `package.flk` if one sits beside the root. | **Call once, before anything touches content.** A present-but-unreadable `package.flk` **panics** (naming the path) — the scene-manifest fatality class; nothing meaningful runs on a broken package. |
| `set_content_root(Option<PathBuf>)` | Point the process at a tree directly (or clear it with `None`). | The escape hatch for tests and tools handed a tree on the command line. **Never mounts** a `.flk`. Prefer `init_from_app_dir` for a real app. |
| `installed_app_dir() -> Option<PathBuf>` | The running exe's directory IF this is an installed layout. | `Some` only when a `content.json` sits beside the exe; a dev `cargo run` (exe in `target/…`) gets `None`, so the caller falls back to its compile-time root. The ONE place an exe path is consulted. |
| `dir_declares_content(dir) -> bool` | The classifier `installed_app_dir` applies. | Split out so tests exercise it without faking `current_exe()`. True iff `dir/content.json` is a file. |

### Reading & writing content at rest — the gz-at-rest + package seam (`flicker_core::compression`)

The read/write functions every content loader funnels through. Callers name **logical** paths;
the seam resolves gz-at-rest and the package mount underneath. **Not** re-exported at the crate
root (the names are too generic there); reach them as `flicker_core::compression::…` — or via
`flicker_content::package`, the content crate's named door.

| Item | What it is for | The one thing to know |
|---|---|---|
| `read_bytes(path)` / `read_text(path)` | Read a content file by its logical path, transparently decompressing. | Resolution order, shipped-form-wins: **mounted `.flk` entry → filesystem `.gz` twin → raw file**. A mount hit that can't be read errors LOUD (never falls back); a missing file surfaces as `NotFound` for the logical name. `read_text` adds UTF-8 decode (`InvalidData` on non-UTF-8). |
| `read_bytes_prefix(path, max_bytes)` | Read at most the first `max_bytes` of the DECOMPRESSED form. | Streamed — stops at the cap, so an inspector learns what a 500 KB clip IS without inflating it. The result may end mid-token; scan it, do not parse it as a complete document. |
| `write_bytes(path, bytes)` / `write_text(path, text)` | Write processed content, EMITTING the gz-at-rest form. Returns the path actually written. | One-way discipline: a non-`.gz` logical path is written as `<path>.gz` and any pre-existing raw twin is removed, so a save can't leave a stale raw file shadowing the fresh gz. Creates parent dirs. Writers never touch the mount. |
| `file_exists(path)` | Gz-transparent existence for a logical path. | True when the mount serves it, or the `.gz` twin, or the raw file is present. Use this wherever a loader used to call `path.exists()`. |
| `list_dir(path) -> Vec<DirEntry>` | List a content directory's children — mount ∪ filesystem, mount first, each name once, sorted. | `NotFound` only when NEITHER side knows the directory. File names are the AT-REST names (`x.json.gz`) — exactly what a raw listing shows, so existing name filters keep working. |
| `DirEntry { path, is_dir }` | One child from `list_dir`. | `path` is the parent joined with the at-rest child name. |
| `gz_sibling(path)` / `names_gz(path)` | The at-rest name helpers. | `gz_sibling` appends `.gz` keeping the inner extension (`intro.flight` → `intro.flight.gz`); `names_gz` tests whether a path already names the gz form. |

### The mounted package container (`flicker_core::mount`)

`package.flk` is a store-only zip (all entries Stored — the tree is already gz-at-rest). Mounting
it makes every seam reader package-capable at once, because reads already funnel through
`compression`. The mount is process-global and read-only; its read side is internal — the seam
consults it for you. The two functions a caller touches:

| Item | What it is for | The one thing to know |
|---|---|---|
| `mount_package(flk, mount_root) -> io::Result<usize>` | Mount `flk` as the tree rooted at `mount_root` (normally `roots().package()`); returns the entry count. | There is ONE package per process — a second call replaces the first. Normally you don't call this directly: `init_from_app_dir` does, when a `package.flk` is present. |
| `unmount()` | Drop the mount. | For tests; the running game never unmounts. |

### Generic gzip — buffer in, buffer out (re-exported at the crate root)

For payloads already fully in memory (a finished bake, a wire packet, an asset `Vec<u8>`). Reach
these as `flicker_core::…` directly.

| Item | What it is for | The one thing to know |
|---|---|---|
| `compress_gzip(input) -> Vec<u8>` | Gzip-compress at the default level (≈ 6). | Balanced speed/ratio for the typical payload (repetitive JSON, dense bit fields). |
| `compress_gzip_with_level(input, level) -> Vec<u8>` | Gzip-compress at a caller-chosen `Compression` level. | `0` = store (frames only), `1` = fastest/weakest, `9` = strongest/slowest. Bump it for shipping content where size dominates. |
| `decompress_gzip(input) -> Result<Vec<u8>, CompressionError>` | Inverse of `compress_gzip`. | Errors (`CompressionError::Io`) on a corrupt header, truncated body, or bad CRC. |
| `is_gzipped(bytes) -> bool` | 2-byte magic sniff (`1F 8B`) for autodetection. | Only decides "try the decoder" vs "trust as uncompressed"; does not validate the rest of the header. |
| `CompressionError` | The module's error type. | Currently wraps `io::Error` (every failure is an IO-level decoder rejection); a dedicated type leaves room to grow without breaking callers. |
| `Compression` | The re-exported `flate2` level type for `compress_gzip_with_level`. | `Compression::none()` / `fast()` / `best()` / `new(0..=9)` / `default()`. |

## Interactions

- **Input signals:** None. This is a headless foundation library — it captures no `ActionSignal`s
  and wires to no keys. The layers above own input.
- **Results / intents fired:** None. Every function returns a value (`io::Result`, a report, a
  path); nothing is dispatched.
- **Model keys / Lua:** None. This crate sits below the per-frame Model and the Lua layer entirely.
- **What it hands other crates:** resolved content paths (`ContentRoots` and its sub-roots),
  decoded content bytes/text through the seam, and the process-global read-only package mount.
- **Process-global state / threads:** two `Mutex`-guarded process globals — the **content root**
  (set once at startup by `init_from_app_dir`/`set_content_root`, read by `roots()`) and the
  **package mount** (set once, read-only, replaced only by a re-mount). No worker threads, no
  async; every function is synchronous.

## Gates

The tests that enforce the contracts — `cargo test -p flicker-core` (16 tests, all green). By name:

**Root resolution + the `content.json` contract** (`roots.rs`)
- `sub_roots_derive_from_the_one_knob` — `package`/`staging`/`data`/`source` all hang off the one
  root; `staging` and `package` share a parent (can't name different trees).
- `a_relative_root_hangs_off_the_app_dir_and_an_absolute_one_wins` — relative vs absolute resolution.
- `a_missing_or_invalid_config_falls_back_to_the_default` — missing OR malformed `content.json`
  (and an empty `{}`) all yield the `../content` default; a valid override is honoured.
- `installed_layout_is_declared_by_an_exe_adjacent_content_json` — the single installed-layout marker.
- `the_undeclared_fallback_finds_the_repo_tree` — the `roots()` repo-climb fallback lands on a real
  tree (`data/periodic_table.json` exists), so library tests and the offline tool keep working.

**The gz-at-rest + mount seam** (`compression.rs`, `mount.rs`)
- `write_then_read_is_gz_first_at_the_logical_path` — THE seam contract: a write emits `<path>.gz`
  (raw twin removed), reads back by the logical path, the gz twin wins over a planted raw twin,
  raw is the fallback when no twin exists, and an explicit `.gz` path reads directly.
- `read_bytes_missing_file_is_not_found` — a wholly absent logical path is `NotFound`, not empty.
- `resolve_prefers_the_gz_twin` — mount name resolution: gz twin first, raw fallback, explicit
  `.gz` verbatim, miss → `None` (falls through to the filesystem).

**Generic gzip** (`compression.rs`)
- `round_trip_empty_input`, `round_trip_small_payload_is_byte_equal`,
  `round_trip_repetitive_payload_compresses_well` (crushes ≥ 50×),
  `round_trip_random_payload_does_not_blow_up` (incompressible input doesn't balloon),
  `is_gzipped_detects_magic_bytes`, `decompress_rejects_invalid_input` (loud on corrupt/non-gzip),
  `compression_level_0_still_round_trips`, `compression_level_best_round_trips`.

## Sharp edges

- **`roots()` reads process-global state — call `init_from_app_dir` (or `set_content_root`) first.**
  In a real app, `prism-alpha` does this at startup. Before it runs, `roots()` returns the
  compile-time repo fallback — correct for library tests and the offline `content-tool`, wrong for
  a relocated checkout (use `set_content_root` there). The fallback is a single deliberate climb,
  not a per-caller one, and it is tested; it is also the one place a wrong content root can resolve
  *silently* to the repo tree (see Finding 5).
- **A present-but-unreadable `package.flk` is FATAL.** `init_from_app_dir` panics naming the path —
  a corrupt shipped package is the same class of failure as a broken scene manifest. No `.flk` at
  all is fine (loose-tree dev behaviour); only a broken one aborts.
- **Two `sensorium` homes, one accessor.** `ContentRoots::sensorium()` returns the top-level
  authoring / host-loaded UI tree (`scenes/`, `scripts/`, `resources/`). The shipped UI *assets*
  (fonts, atlases) live at `package/sensorium/{assets,fonts}` and are reached via `.package()`.
  Same word, different jobs.
- **The seam is one-way at the write side.** `write_*` always emit `.gz`; `read_*` accept either
  form. Normalizing a pre-existing loose tree to gz-at-rest is the offline `content-tool gzify`
  pass's job (in `flicker-content`), and it is idempotent. `write_*` never touch the mount —
  packing is an offline pass.
- **`read_bytes_prefix` returns a byte prefix, not a document.** It may end mid-token; scan it,
  never parse it as complete.
- **Shipped form wins, always.** Both in the mount and on the filesystem, the gz/mounted form
  takes precedence over a raw twin, so a stale hand-edited raw file can never shadow shipped
  content. If you edit a loose file for a dev run, make sure no `.gz` twin sits beside it.
- **`content.json` describes the app's SHAPE, not a player's preferences** — it is committed and
  versioned, distinct from the gitignored per-user `settings.json`. A relative `content_root` is
  resolved against the app directory; an absolute one is taken as-is.
