# prism-alpha

The **single launcher binary**. One executable hosts *every* Prism scene — the Populous
bench, the world/epoch simulations, the authoring benches, the click trainer, the cinematic
fly-in — behind one shared front-end (intro splash → mode menu → scene → pause). A **scene**
here is one interactive screen (a bench, a game, a POC); the launcher's job is to register
all of them and let the shell pick which one to play. There is no per-scene executable: each
`flicker-*` scene crate is a library, and this binary is the only `main()` that ships.

Adding a scene to the app is one entry in this crate's `roster()` plus a scene file in the
content tree — nothing here draws or simulates anything itself.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:**
  - `flicker-shell` — the reusable front-end **shell**: it owns the whole flow (splash →
    menu → your scene → pause/settings) and the winit run loop, and it *is* the launcher
    when handed `scene_select: true`. It defines every type this crate names (`SceneEntry`,
    `SceneInfo`, `ShellConfig`, the `REALM_*` constants, `run`).
  - `flicker-content` — declares where this executable's content tree lives (`content.json`)
    before any scene loads, so scenes ask `flicker_content::roots()` instead of climbing out
    of their own crate directory.
  - **The twelve scene crates** it hosts — one `SceneEntry` each (see the roster table). Each
    is a thin library exposing a single `pub fn scene(&SceneDef) -> Box<dyn Scene>`.
- **Bypasses the `flicker` umbrella.** Unlike the shell, this crate does **not** depend on
  the `flicker` core umbrella crate at all — it reaches the engine only through
  `flicker-shell`, `flicker-content`, and the scene crates. Its own code (`main.rs`) is the
  roster plus a short `main()` and two gate tests; it holds no engine logic to pull the
  umbrella in for.
- **Used by:** nothing — this is the top of the tree, the shipped executable and the release
  artifact. Its version *is* the release tag (per-crate versioning, MCP rule `84EF7606`): the
  release gate reads `prism-alpha`'s `Cargo.toml` version, so bump it when this crate ships.
- **Reads from the content tree** (all under the root named by `content.json`, `../content`):
  - `content/sensorium/scenes/*.scene.json` — the **manifest**: the one folder the shell
    indexes at boot. Every launchable scene, and every splash/menu, is a file here. Read once
    on first use; a missing folder panics the boot.
  - `settings.json` (this crate's directory in a dev build; the platform per-user config dir
    in an installed build) — display, audio, input, language. Read and written by the shell.
  - Everything else the front-end needs — `ui_theme.json`, `ui_style.json`, the stringtable,
    the shell's own scripts and logos — is embedded in `flicker-shell` or lives elsewhere in
    the tree; this crate never names those paths. See
    [`../content/sensorium/README.md`](../content/sensorium/README.md).

## What a player / operator does

Run it from the workspace:

```
cargo run -p prism-alpha
```

That opens the window and drives the whole flow: **intro splash → main menu → a scene →
pause overlay**. The main menu is the **mode launcher** (because this crate passes
`scene_select: true`): the root lists the play modes, and choosing one opens that mode's
tier-2 page of scenes. Pick a scene's row to launch it; the pause overlay (opened from
in-scene) can return to the main menu, which unwinds back to the root.

The menu answers the standard menu **signals** — `Confirm` (launch / activate), `Cancel`
(back one tier / close), `NavUp` / `NavDown` / `NavLeft` / `NavRight` (move within a page),
`PanelNext` / `PanelPrev` (switch panel), `PageNext` / `PagePrev`, `TabNext` / `TabPrev`.
Signals are trigger-agnostic (rule `37722F91`): a mouse click is a `Confirm` aimed at what
the pointer hits, and which key/button/stick produces each signal is per-user **profile
data** (`settings.json` → `input_profile`), never wired here. The signal catalog and the
bindings belong to the input crates, not this launcher.

Two operator niceties, both driven by the crate version passed to the shell:

- The shipped version prints bottom-right on the menu (`env!("CARGO_PKG_VERSION")`).
- A one-shot GitHub Releases check lights an **UPDATE AVAILABLE** chip when a newer release
  exists. It needs a network and a non-empty version; it is silent on any failure and never
  runs for dev/POC clients (which pass an empty version). It only *notifies* — it does not
  patch. This behaviour lives in the shell.

The GPU window is verified by the maintainer, not from tests (rule `664B68A6`) — `cargo run`
is how you see it; `cargo test -p prism-alpha` is how you prove the roster.

## Adding a scene — the registration contract

This crate has no public API (it is a binary). Its real surface is **the roster**: the
`roster()` function in [`src/main.rs`](src/main.rs) returns a `Vec<SceneEntry>`, one per
launchable scene. A `SceneEntry` (defined in `flicker-shell`) is built fluently:

```rust
SceneEntry::new("clicktrainer", "Click Trainer", "primary", flicker_clicktrainer::scene)
    .with_realm(REALM_ADVENTURER)
    .with_info(SceneInfo::new(
        "Click Trainer",              // name   — the row title
        "Trainer",                    // mode   — a category tag (display only)
        "Aim / 2D",                   // region — short kind label
        "Click the shrinking targets…",   // desc — one-line description
        "Clay 0.1 · Bench · 2D + vector HUD", // meta — small build/type line
    ))
```

| Part of `SceneEntry` | What it is | The one thing to know |
|---|---|---|
| `id` (1st arg) | The scene's stable action name | Fired by the row's LOAD button as `Goto{id}`; **must equal** the `behaviour` field of the scene file `<id>.scene.json` (the gate enforces this). This is the join key across the whole system. |
| `label` (2nd arg) | Display text for a plain launch button | Used only when the entry is a root-level button, not a launcher row. Raw English today — see Sharp edges. |
| `variant` (3rd arg) | Button style: `primary` / `secondary` / `danger` | Styles the plain button. In this launcher every scene is a **panel row** (drawn from `SceneInfo`), so `variant` has no visible effect here — every entry passes `"primary"`. It is a real knob for single-button clients, not dead. |
| `factory` (4th arg) | `Fn(&SceneDef) -> Box<dyn Scene>` | The scene crate's `scene` fn, passed by name. The shell resolves `id` → the file's `SceneDef` and hands it to this factory (the entry is the *client half* of the behaviour registry). |
| `.with_realm(REALM_…)` | Tags the mode page it lists on | Repeatable — a shared tool can list under several realms. **No realm ⇒ the entry stays a root-level button**, not on any tier-2 page. |
| `.with_info(SceneInfo::new(…))` | The five-field panel row | Required for a launcher row; without it the entry is a plain button. Fields below. |

`SceneInfo` fields (all display strings): **name** (row title) · **mode** (category tag,
e.g. "Simulation", "Editor") · **region** (short kind, e.g. "World / Voxels") · **desc**
(one-line description) · **meta** (small build/type line).

**The realms** (constants in `flicker-shell`; the launcher's tier-1 → tier-2 map):

| Constant | Value | The mode page it fills | Holds |
|---|---|---|---|
| `REALM_ADVENTURER` | `"adventurer"` | Explore the World | player-facing scenes |
| `REALM_DM` | `"dm"` | Build the World | **empty** — no bench migrated yet (page is a note) |
| `REALM_GAMEMASTER` | `"gamemaster"` | Game Master | world-authoring: the sims + map benches |
| `REALM_DEVELOPER` | `"developer"` | Developer | engine tooling / benches / POCs |

The root-menu button *text* for each page (e.g. "Explore the World") is a stringtable token
owned by the shell's menu script, not by this crate.

### The current roster

Twelve scenes, grouped by realm as `roster()` orders them. Each links its crate README (API
+ signals + Model keys for that scene) and its scene file. How to author the file and its
pair script is the content tree's job — see
[`../content/sensorium/README.md`](../content/sensorium/README.md).

**Game Master** (`REALM_GAMEMASTER`)

| `id` | Row title | Crate | Scene file |
|---|---|---|---|
| `populous` | Populous Bench | [flicker-populous](../crates/scenes/flicker-populous/README.md) | [populous.scene.json](../content/sensorium/scenes/populous.scene.json) |
| `godmode` | God Mode | [flicker-godmode](../crates/scenes/flicker-godmode/README.md) | [godmode.scene.json](../content/sensorium/scenes/godmode.scene.json) |
| `pocepochs` | Epoch Simulation | [flicker-pocepochs](../crates/scenes/flicker-pocepochs/README.md) | [pocepochs.scene.json](../content/sensorium/scenes/pocepochs.scene.json) |

**Developer** (`REALM_DEVELOPER`)

| `id` | Row title | Crate | Scene file |
|---|---|---|---|
| `assetpipeline` | Clayworks Bench | [flicker-assetpipeline](../crates/scenes/flicker-assetpipeline/README.md) | [assetpipeline.scene.json](../content/sensorium/scenes/assetpipeline.scene.json) |
| `quartermaster` | Quartermaster Bench | [flicker-quartermaster](../crates/scenes/flicker-quartermaster/README.md) | [quartermaster.scene.json](../content/sensorium/scenes/quartermaster.scene.json) |
| `componentcatalog` | Component Catalog | [flicker-componentcatalog](../crates/scenes/flicker-componentcatalog/README.md) | [componentcatalog.scene.json](../content/sensorium/scenes/componentcatalog.scene.json) |
| `sablework` | Sablework Bench | [flicker-sablework](../crates/scenes/flicker-sablework/README.md) | [sablework.scene.json](../content/sensorium/scenes/sablework.scene.json) |
| `loomforge` | Loomforge Bench | [flicker-loomforge](../crates/scenes/flicker-loomforge/README.md) | [loomforge.scene.json](../content/sensorium/scenes/loomforge.scene.json) |

**Adventurer** (`REALM_ADVENTURER`)

| `id` | Row title | Crate | Scene file |
|---|---|---|---|
| `solarbirth` | Solar Birth | [flicker-solarbirth](../crates/scenes/flicker-solarbirth/README.md) | [solarbirth.scene.json](../content/sensorium/scenes/solarbirth.scene.json) |
| `clicktrainer` | Click Trainer | [flicker-clicktrainer](../crates/scenes/flicker-clicktrainer/README.md) | [clicktrainer.scene.json](../content/sensorium/scenes/clicktrainer.scene.json) |
| `pocclusters` | Prism Test Room | [flicker-pocclusters](../crates/scenes/flicker-pocclusters/README.md) | [pocclusters.scene.json](../content/sensorium/scenes/pocclusters.scene.json) |
| `controllertester` | Controller Tester | [flicker-controllertester](../crates/scenes/flicker-controllertester/README.md) | [controllertester.scene.json](../content/sensorium/scenes/controllertester.scene.json) |

`REALM_DM` is empty — the "Build the World" page renders its note until a DM bench migrates.

### End-to-end: register a new bench

1. **Ship the scene as a library crate** exposing `pub fn scene(def: &SceneDef) -> Box<dyn
   Scene>` (mirror any entry above — `flicker-clicktrainer` is the reference).
2. **Add it as a dependency** in [`Cargo.toml`](Cargo.toml) (path dependency, like its siblings).
3. **Add one `SceneEntry`** to `roster()` with its `id`, a `.with_realm(…)`, and a
   `.with_info(…)`.
4. **Author `content/sensorium/scenes/<id>.scene.json`** whose `behaviour` field equals the
   `id` (plus its pair `<id>.lua`). This is the content author's step — see the sensorium
   README.
5. `cargo test -p prism-alpha` — both gates below must stay green.

## The manifest ↔ roster gate — the one subtle concept

A launchable scene is **data on both sides**, bound by the shared `id`:

- The **scene file** carries a `behaviour` field naming the client code that plays it.
- The **roster entry** carries a `factory` that *is* that client code, keyed by `id`.

`every_authored_scene_resolves_and_every_bench_is_authored` closes the loop in three
directions, so no half of a scene can ship alone:

1. Every authored file's `behaviour` is either a shell **builtin** (splash/menu/pause/loading
   — see `flicker_shell::builtin_behaviours()`) or a roster `id`. *A file nothing plays is a
   black screen waiting to be clicked.*
2. Every roster entry has a scene file. *A launchable scene IS data — no file, no launch.*
3. Each launchable file's `behaviour` names its **own** roster `id`, so `Goto{id}` builds the
   bench the file says it is.

At runtime the shell boots from the manifest (whichever file claims `boot`), and every menu
launch and transition is `id`-addressed through this same roster — the engine never names a
scene type in Rust. A typo in either `id` fails **loud**: an unresolved boot scene panics,
and this gate fails the build for any file/roster mismatch (rule `4BB12A75`).

## Configuration files

| File | Committed? | What it is |
|---|---|---|
| [`content.json`](content.json) | yes | One knob, `content_root` (`../content`). The sub-roots — `package/ staging/ data/ sensorium/ source/` — are **derived** from it, so staging can never drift onto a different tree than package. Read by `flicker_content::init_from_app_dir` at startup. Describes the app's shape. |
| `settings.json` | no (per-user, gitignored) | Display mode/resolution/position, audio levels, `input` tuning, the `input_profile` (per-context signal→binding maps), and `language`. Read and written by the shell. |

**Dev vs installed resolution** (`main.rs` `main()`): an installed build is marked by a
`content.json` sitting beside the executable — then content resolves relative to the exe and
per-user `settings.json` goes to the platform config dir (the install tree is read-only). A
dev build (`cargo run`) has no exe-adjacent marker, so both content and settings stay in this
crate's directory. `packaging/` holds the per-platform install manifests (deb metadata is in
`Cargo.toml`, macOS `Info.plist`, Linux `.desktop`, Windows `.wxs`).

## Gates

Both live in [`src/main.rs`](src/main.rs) `#[cfg(test)]`; run with `cargo test -p prism-alpha`.

| Test | What breaks it |
|---|---|
| `roster_holds_the_migrated_benches` | The set of scenes per realm, in order, drifts from the ratified mode map — e.g. a stray re-add of an un-migrated bench, a realm re-tag, an entry missing its `SceneInfo`, or a scene landing in `REALM_DM` before its bench migrates. |
| `every_authored_scene_resolves_and_every_bench_is_authored` | The manifest↔roster binding above breaks in any of its three directions (orphan file, roster entry with no file, or a file whose `behaviour` names a different entry). |

## Sharp edges

- **The roster ships raw English.** Every `label` and all five `SceneInfo` fields are literal
  English in `main.rs`, not stringtable `$token`s — the launcher is the one screen that cannot
  be localised, and copying an entry passes every test while quietly bypassing the
  `raw_display_literals` gate (which only walks fixtures, never the real roster). This is a
  known, still-open gap (MCP incident `0DE5D5EE`); the localization rule it contradicts is
  `D5ED9ACF`. If you add a bench, you will hit this tension: the sensorium README tells
  authors "display copy is a `$token`," and this file does the opposite.
- **`variant` is a no-op for launcher rows.** It styles a plain button; a `scene_select`
  launcher draws rows from `SceneInfo`. Passing anything but `"primary"` changes nothing on
  screen here (but the knob is real for single-button clients — not dead).
- **`REALM_DM` is dark.** "Build the World" has no scene; its page is a placeholder note.
- **The update chip needs a network and a shipped version.** In a dev `cargo run` the version
  is real (from `Cargo.toml`), so the check *does* run; it is silent on any failure. It only
  notifies — patching is not implemented here.
- **This crate never draws.** If a scene is blank or misbehaves, the bug is in that scene's
  crate or its scene file, not in `prism-alpha` — the launcher only registers and routes.
