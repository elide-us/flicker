# Flicker Engine — Initial Project Setup Spec

This document is a prescriptive setup spec for Claude Code to execute in an empty git repository. Follow it literally. Do not improvise architecture, do not add crates beyond what is specified, do not add dependencies beyond what is specified. Where there is ambiguity, stop and ask.

## Project identity

- **Name:** `flicker` (working title, do not rename without instruction)
- **Type:** 2D game engine with networked persistent-world support
- **Language:** Rust (stable channel, edition 2021)
- **Graphics:** wgpu (WebGPU-spec, generates native Metal/D3D12/Vulkan)
- **Scripting:** Luau via mlua
- **Windowing:** winit
- **Math:** glam
- **UI:** egui (added in a later phase, not v0.1)
- **License:** MIT OR Apache-2.0 (dual, Rust ecosystem standard)

## Target platforms

Primary: macOS (Apple Silicon), Windows 11 (x86_64), iPadOS, iOS.
Secondary (gets it for free via wgpu): Linux x86_64, Linux ARM, Android.
Build host assumed for this setup: macOS Apple Silicon.

## Architectural posture

The engine is a Cargo workspace of single-responsibility crates. Game content (scripts, sprites, data) is **not** part of the engine — it lives in a separate game project that depends on flicker crates. The engine is generic; the game is opinionated.

Server-side game state (live sessions, chat, position telemetry, sync, combat physics, world hosting) lives in **separate server projects not in this workspace**. Auth and persistent storage operations (loot, inventory, character data) go through an existing web service backend, also not in this workspace. The engine includes a `flicker-net` crate that defines the client side of those protocols, but the servers themselves are out of scope for this repo.

## Workspace layout

Create the following Cargo workspace structure:

```
flicker/
├── Cargo.toml                 # workspace root
├── Cargo.lock
├── README.md
├── LICENSE-MIT
├── LICENSE-APACHE
├── .gitignore
├── .gitattributes
├── rust-toolchain.toml
├── rustfmt.toml
├── clippy.toml
├── .github/
│   └── workflows/
│       └── ci.yml             # build + test on macOS, Windows, Linux
├── crates/
│   ├── flicker-core/          # math re-exports, time, input abstractions, fixed-step loop
│   ├── flicker-render/        # wgpu device, surface, sprite batcher, atlas
│   ├── flicker-2d/            # Sprite, Tilemap, Camera2D
│   ├── flicker-script/        # Luau runtime, binding registration API
│   ├── flicker-net/           # client-side transport, state sync, auth handshake
│   ├── flicker-app/           # winit event loop, frame orchestration, public entry point
│   └── flicker/               # umbrella re-export crate (the one games depend on)
├── examples/
│   └── hello-sprite/          # minimal example: window opens, sprite renders
└── docs/
    └── architecture.md        # one-page architectural overview, write as a stub
```

The following crates are **planned but not created in this initial setup**: `flicker-ui`, `flicker-minigame`, `flicker-dialogue`, `flicker-station`, `flicker-build`. They will be added in later phases. Do not scaffold them now.

## Workspace `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
    "crates/flicker-core",
    "crates/flicker-render",
    "crates/flicker-2d",
    "crates/flicker-script",
    "crates/flicker-net",
    "crates/flicker-app",
    "crates/flicker",
    "examples/hello-sprite",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.80"
license = "MIT OR Apache-2.0"
repository = "https://github.com/<TODO>/flicker"
authors = ["<TODO author line>"]

[workspace.dependencies]
# Graphics & windowing
wgpu = "22"
winit = "0.30"
pollster = "0.4"          # for blocking on async device init in examples

# Math
glam = { version = "0.29", features = ["bytemuck"] }
bytemuck = { version = "1", features = ["derive"] }

# Scripting
mlua = { version = "0.10", features = ["luau", "vendored", "async", "serialize"] }

# Async runtime (for net + async script bridge)
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "sync", "time"] }

# Networking
tokio-tungstenite = "0.24"  # WebSocket client
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
bincode = "1"             # fast binary format for hot-path messages

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Error handling
thiserror = "2"
anyhow = "1"

# Image loading (for textures)
image = { version = "0.25", default-features = false, features = ["png", "jpeg"] }

# Internal cross-crate deps
flicker-core = { version = "0.1.0", path = "crates/flicker-core" }
flicker-render = { version = "0.1.0", path = "crates/flicker-render" }
flicker-2d = { version = "0.1.0", path = "crates/flicker-2d" }
flicker-script = { version = "0.1.0", path = "crates/flicker-script" }
flicker-net = { version = "0.1.0", path = "crates/flicker-net" }
flicker-app = { version = "0.1.0", path = "crates/flicker-app" }

[profile.dev]
opt-level = 1             # base dev profile gets some optimization; debugging stays usable

[profile.dev.package."*"]
opt-level = 3             # dependencies always built optimized — huge win for wgpu/winit

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

Use **workspace-inherited versions** for every shared dependency in the individual crate manifests (`wgpu.workspace = true` style). This is the only way the version pinning stays manageable.

## Per-crate manifests

### `crates/flicker-core/Cargo.toml`
```toml
[package]
name = "flicker-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
glam.workspace = true
bytemuck.workspace = true
tracing.workspace = true
thiserror.workspace = true
```

### `crates/flicker-render/Cargo.toml`
```toml
[package]
name = "flicker-render"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
flicker-core.workspace = true
wgpu.workspace = true
glam.workspace = true
bytemuck.workspace = true
image.workspace = true
tracing.workspace = true
thiserror.workspace = true
pollster.workspace = true
```

### `crates/flicker-2d/Cargo.toml`
```toml
[package]
name = "flicker-2d"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
flicker-core.workspace = true
flicker-render.workspace = true
glam.workspace = true
tracing.workspace = true
```

### `crates/flicker-script/Cargo.toml`
```toml
[package]
name = "flicker-script"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
flicker-core.workspace = true
mlua.workspace = true
serde.workspace = true
tracing.workspace = true
thiserror.workspace = true
```

### `crates/flicker-net/Cargo.toml`
```toml
[package]
name = "flicker-net"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
flicker-core.workspace = true
tokio.workspace = true
tokio-tungstenite.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
bincode.workspace = true
tracing.workspace = true
thiserror.workspace = true
```

### `crates/flicker-app/Cargo.toml`
```toml
[package]
name = "flicker-app"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
flicker-core.workspace = true
flicker-render.workspace = true
flicker-2d.workspace = true
winit.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
pollster.workspace = true
```

### `crates/flicker/Cargo.toml` (umbrella)
```toml
[package]
name = "flicker"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "2D game engine with networked persistent-world support"

[dependencies]
flicker-core.workspace = true
flicker-render.workspace = true
flicker-2d.workspace = true
flicker-script.workspace = true
flicker-net.workspace = true
flicker-app.workspace = true
```

The umbrella crate's `lib.rs` re-exports each sub-crate as a module (`pub use flicker_core as core;` etc.) so game projects depend on `flicker` and reach everything via `flicker::core`, `flicker::render`, etc.

### `examples/hello-sprite/Cargo.toml`
```toml
[package]
name = "hello-sprite"
version = "0.1.0"
edition.workspace = true
publish = false

[dependencies]
flicker = { path = "../../crates/flicker" }
anyhow.workspace = true
tracing-subscriber.workspace = true
```

## Per-crate `lib.rs` stubs

Each crate gets a `src/lib.rs` with a one-line module doc comment and no actual implementation. The point of this initial setup is to confirm the workspace compiles end-to-end with all dependencies resolved, not to implement the engine. Each `lib.rs` should look like:

```rust
//! flicker-core: math, time, input abstractions, and the fixed-step game loop.
```

The `examples/hello-sprite/src/main.rs` should be a `fn main()` that prints "flicker hello-sprite stub" and exits. Do not attempt to open a window in the initial scaffold — that's the first real implementation task and gets its own pass.

## Toolchain & tooling files

### `rust-toolchain.toml`
```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

### `rustfmt.toml`
```toml
edition = "2021"
max_width = 100
use_field_init_shorthand = true
```

### `clippy.toml`
```toml
# Project-wide clippy config; keep minimal until lints start firing in real code.
```

### `.gitignore`
Standard Rust + macOS + IDE entries:
```
/target
**/*.rs.bk
Cargo.lock.bak
.DS_Store
.idea/
.vscode/
*.swp
```

Note: **do** commit `Cargo.lock` for the workspace root since this produces binary artifacts. (Standard Rust convention: lock files are committed for binaries, not for libraries; a workspace with binaries commits it.)

### `.gitattributes`
```
* text=auto eol=lf
*.rs text eol=lf
*.toml text eol=lf
*.md text eol=lf
```

### `README.md`

Write a brief README with:
- One-paragraph description of the project (a 2D game engine in Rust on wgpu, targeting Apple Silicon and beyond, designed for networked persistent-world games)
- Build instructions: `cargo build`, `cargo run -p hello-sprite`
- A "Status" section that says "Pre-alpha. Scaffolding only. Not usable yet."
- A "Workspace layout" section that lists each crate with its one-line purpose.
- License section (MIT OR Apache-2.0).

### `LICENSE-MIT` and `LICENSE-APACHE`

Use the standard Rust ecosystem versions of these files. Copyright line should be `Copyright (c) 2026 <TODO author>` — leave the TODO so the user can fill it in.

### `.github/workflows/ci.yml`

A minimal CI workflow that runs on push and pull_request:

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:

jobs:
  build:
    strategy:
      matrix:
        os: [macos-latest, windows-latest, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: fmt
        run: cargo fmt --all -- --check
      - name: clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: build
        run: cargo build --workspace --all-targets
      - name: test
        run: cargo test --workspace
```

### `docs/architecture.md`

Write a stub document with these headings and one paragraph of placeholder text each, so the structure exists and content can be filled in later:

- Overview
- Crate boundaries
- The fixed-step loop
- The sprite rendering pipeline
- Scripting integration
- Networking model
- Client/server split

## Verification steps

After scaffolding, run these commands and confirm they all succeed:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo run -p hello-sprite
```

The last command should print "flicker hello-sprite stub" and exit cleanly.

If any of those fail, **do not paper over the failure** — surface it and ask. The point of this initial setup is to establish a clean baseline. A workspace that builds with zero warnings on day one is much easier to keep clean than one that starts with debt.

## What is explicitly out of scope for this pass

Do not implement any of the following in this scaffolding pass; they each get their own dedicated implementation pass:

- Opening an actual window
- Initializing a wgpu device or surface
- Loading a texture
- Drawing a sprite
- Setting up the fixed-step loop
- Loading or running a Lua script
- Any network code beyond the empty crate
- Any game logic of any kind

This setup pass is purely "the workspace exists, the dependencies resolve, the empty crates compile, CI is wired up." That's the entire deliverable.

## Things to confirm with the user before proceeding

Before doing anything destructive or non-reversible:

1. Confirm the author name and email to use in `Cargo.toml` and license files.
2. Confirm the GitHub repo URL (or that it doesn't exist yet and the field should stay as `<TODO>`).
3. Confirm Rust 1.80+ is acceptable as the MSRV (this is needed for some of the wgpu 22 and mlua 0.10 features).

If anything in this spec seems ambiguous or in conflict with what you find on the user's machine (e.g., a different Rust version, an existing project layout), stop and ask rather than guessing.