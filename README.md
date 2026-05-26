# flicker

A 2D game engine written in Rust on top of wgpu, targeting Apple Silicon first
and extending to Windows, iPadOS, iOS, Linux, and Android via the WebGPU spec.
Designed from the start for networked persistent-world games, with a Luau
scripting layer and a thin client-side networking crate that talks to separate
game-server and web-backend projects.

## Build

```
cargo build
cargo run -p hello-sprite
```

## Status

Pre-alpha. Scaffolding only. Not usable yet.

## Workspace layout

- `flicker-core` — math re-exports, time, input abstractions, and the fixed-step loop.
- `flicker-render` — wgpu device, surface, sprite batcher, and texture atlas.
- `flicker-2d` — Sprite, Tilemap, and Camera2D primitives.
- `flicker-script` — Luau runtime and binding registration API.
- `flicker-net` — client-side transport, state sync, and auth handshake.
- `flicker-app` — winit event loop, frame orchestration, and the public entry point.
- `flicker` — umbrella crate that re-exports each sub-crate; the one games depend on.
- `examples/hello-sprite` — minimal example; will eventually open a window and render a sprite.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
