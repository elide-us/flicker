# flicker

**flicker** is the client of **ClayEngine** — the game engine behind the **Prism** MMO —
written in Rust on top of `wgpu` and Luau. It ships as a single application, **Prism
Alpha**: one launcher binary that hosts a roster of playable and developer scenes across
several "realms." It targets Apple Silicon and Windows and Linux today, and is built
against the WebGPU spec to extend to iPadOS, iOS, and Android.

The engine pairs a **voxel client renderer** (sparse-octree clusters, dual-contour
meshing, render-time LOD, skeletal animation) with an **offline procedural planet
generator** (an ISEA icosahedral hex-sphere driven through a multi-epoch planet-evolution
simulation), a **declarative UI/component system** with a strict data-only Luau scripting
boundary, and a thin **networking crate** that connects out to the rest of the ecosystem.

This repository is the ClayEngine **client** (dev codename "flicker"). It is a work in
progress — an **alpha** — but a large, coherent one: the launcher, the scene/component
system, worldgen, the voxel pipeline, animation, the content-packaging pipeline, and the
release/installer pipeline are all real and working.

> Orientation, load-bearing design decisions, per-subsystem state, and history live in the
> project's MCP memory (see [CLAUDE.md](CLAUDE.md)), not in local docs.

## The ClayEngine ecosystem

flicker is one project in a family. The pieces are deliberately separate:

- **ClayEngine — the game engine.** Its client (this repo) runs on the player's machine.
  Its **servers live in a separate `clay-engine` repository**: a live game/**Physics
  Server** (IOCP networking, MMO scale), a **Simulation Server** (the slow procedural
  world-erosion batch), and the development **chat / world-state** servers the client
  connects to.
- **TheOracleRPC** (repo `elideus-group`) — the web backend and OAuth identity provider:
  authentication, entitlements, credits/subscriptions, and (in progress) unified messaging.
- **Unity** (repo `elideus-group-unity`) — the next-generation reimplementation of that
  backend.
- **Prism** — the MMO's setting and lore (the `Prism/` books). flicker's worldgen periodic
  table and hex-sphere pentagon "defects" are seeded from Prism canon.

`flicker-net` is the seam to all of it: it holds an anonymous release-update checker and a
`clay-chat` client today, and is where the future auth/entitlements (TheOracleRPC) and
live-play (Physics / world-state) connections land.

## Build & run

```
cargo build --workspace          # stable toolchain
cargo test  --workspace          # ~1,500 tests
cargo run   -p prism-alpha       # the Prism Alpha client (the launcher)
```

Use `--release` for any voxel or performance work; debug contour + mesh is slow. The
world-sim crates under `crates/` are standalone libraries exercised by their own tests
(e.g. `cargo test -p flicker-worldgen`).

## The Prism Alpha client

`prism-alpha` is the single launcher binary; it hosts every scene. Scenes are **data**:
each is a `*.scene.json` layout under `Alpha/content/sensorium/scenes/` (authoring guide:
[Alpha/content/sensorium/README.md](Alpha/content/sensorium/README.md)), driven by a Rust
behaviour crate under `Alpha/crates/scenes/`, with runtime behaviour scripted through a
strict data-only Luau boundary. The launcher presents them across realms — **Adventurer**,
**Developer**, and **Game Master** — with a manifest↔roster gate that keeps the shipped
menu and the packaged content in sync.

## Distribution

Releases are cut by a **tag-triggered GitHub Actions pipeline**
([.github/workflows/release.yml](.github/workflows/release.yml)) that publishes per-OS
installers to **GitHub Releases** — Windows **MSI** (WiX v5), macOS **`.pkg`** (Apple
Silicon `.app`), Linux **`.deb`** (cargo-deb) — plus portable archives. Game content ships
as a single deterministic, store-only **`package.flk`** packed once in CI and mounted by
the engine at runtime. Every release publishes a **`SHA256SUMS`**; alpha builds are
currently **unsigned** (verify against the checksums). The client shows an in-app
"update available" hint via an anonymous check against the public Releases API.

See [SECURITY.md](SECURITY.md) (reporting + supply-chain posture), [PRIVACY.md](PRIVACY.md),
and [WARRANTY.md](WARRANTY.md).

## Workspace layout

Crates are organized as `Alpha/crates/<cluster>/<crate>` (the application) plus a root
`crates/` group (standalone world-sim POCs):

- **core** — [`clayengine`](Alpha/crates/core/clayengine/README.md) (world-defining constants), [`flicker`](Alpha/crates/core/flicker/README.md) (umbrella),
  [`flicker-core`](Alpha/crates/core/flicker-core/README.md),
  [`flicker-worker`](Alpha/crates/core/flicker-worker/README.md).
- **platform / render / input** — [`flicker-app`](Alpha/crates/platform/flicker-app/README.md),
  [`flicker-render`](Alpha/crates/render/flicker-render/README.md), [`flicker-2d`](Alpha/crates/render/flicker-2d/README.md),
  [`flicker-input-core`](Alpha/crates/input/flicker-input-core/README.md) /
  [`-router`](Alpha/crates/input/flicker-input-router/README.md) /
  [`-device`](Alpha/crates/input/flicker-input-device/README.md).
- **scripting / frontend** — [`flicker-script`](Alpha/crates/scripting/flicker-script/README.md) (Luau host),
  [`flicker-scene`](Alpha/crates/frontend/flicker-scene/README.md), [`flicker-shell`](Alpha/crates/frontend/flicker-shell/README.md),
  [`flicker-widgets`](Alpha/crates/frontend/flicker-widgets/README.md),
  [`flicker-globe`](Alpha/crates/frontend/flicker-globe/README.md).
- **animation / content / mechanics** — [`flicker-flight`](Alpha/crates/animation/flicker-flight/README.md),
  [`flicker-skeletal`](Alpha/crates/animation/flicker-skeletal/README.md),
  [`flicker-content`](Alpha/crates/content/flicker-content/README.md),
  [`flicker-materials`](Alpha/crates/content/flicker-materials/README.md),
  [`flicker-primitive`](Alpha/crates/content/flicker-primitive/README.md), [`flicker-texture`](Alpha/crates/content/flicker-texture/README.md),
  [`flicker-mechanics`](Alpha/crates/mechanics/flicker-mechanics/README.md).
- **world** (`Alpha/crates/world/`) — [`flicker-voxel`](Alpha/crates/world/flicker-voxel/README.md),
  [`flicker-worldgen`](Alpha/crates/world/flicker-worldgen/README.md),
  [`flicker-worldengine`](Alpha/crates/world/flicker-worldengine/README.md),
  [`flicker-worldstate`](Alpha/crates/world/flicker-worldstate/README.md), [`flicker-worldgrid`](Alpha/crates/world/flicker-worldgrid/README.md), [`flicker-worldtile`](Alpha/crates/world/flicker-worldtile/README.md),
  [`flicker-poc-chemistry`](Alpha/crates/world/flicker-poc-chemistry/README.md),
  [`flicker-orrery`](Alpha/crates/world/flicker-orrery/README.md).
- **net** — [`flicker-net`](Alpha/crates/net/flicker-net/README.md) (the `clay-chat` client + the release-update checker).
- **scenes** — the scene behaviour crates: [`flicker-clicktrainer`](Alpha/crates/scenes/flicker-clicktrainer/README.md), [`flicker-componentcatalog`](Alpha/crates/scenes/flicker-componentcatalog/README.md), [`flicker-controllertester`](Alpha/crates/scenes/flicker-controllertester/README.md), [`flicker-solarbirth`](Alpha/crates/scenes/flicker-solarbirth/README.md), [`flicker-loomforge`](Alpha/crates/scenes/flicker-loomforge/README.md), [`flicker-assetpipeline`](Alpha/crates/scenes/flicker-assetpipeline/README.md), [`flicker-pocclusters`](Alpha/crates/scenes/flicker-pocclusters/README.md), [`flicker-populous`](Alpha/crates/scenes/flicker-populous/README.md), [`flicker-quartermaster`](Alpha/crates/scenes/flicker-quartermaster/README.md), [`flicker-sablework`](Alpha/crates/scenes/flicker-sablework/README.md).
- **prism-alpha** — the launcher application.
- **root `crates/`** — standalone POC libraries ([`flicker-celestial`](crates/flicker-celestial/README.md), [`flicker-system`](crates/flicker-system/README.md),
  [`flicker-greed`](crates/flicker-greed/README.md)).

## Status

**Alpha (the `0.2.x` line).** Not yet a feature-complete game, but a working client:
the launcher, the data-driven scene/component system, the voxel pipeline, worldgen,
skeletal animation, the content-packaging pipeline, and the cross-platform installer
pipeline all function. Online play (chat + world-state sync against the ClayEngine servers,
auth/entitlements via TheOracleRPC) is early — local/developer-facing today, with host
discovery and authentication arriving from the web backend.

## License

flicker's own code is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

**Bundled fonts are third-party** and are **not** under the above license. The Prism
typeface roles are built from four Google Fonts open-source families — **Cinzel**,
**Cormorant Garamond**, **EB Garamond**, and **Noto Sans Runic** — each under the **SIL
Open Font License 1.1**; the license texts are archived in
[Prism/Licenses/](Prism/Licenses/), and the shipped faces are renamed off their original
family names as the OFL's reserved-name rule requires. See [WARRANTY.md](WARRANTY.md) for
the full third-party notice.
