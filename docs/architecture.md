# flicker architecture

## Overview

flicker is a 2D game engine packaged as a Cargo workspace of single-responsibility
crates. Game content lives outside this workspace; the engine is generic and the
games that depend on it are opinionated. This document is a stub — fill in real
content as the architecture solidifies.

## Crate boundaries

Each crate owns one concern: math/time/input, rendering, 2D primitives, scripting,
networking, and the windowed app shell. The umbrella `flicker` crate re-exports all
of them so downstream games depend on a single name. Stub — expand once boundaries
are exercised by real implementations.

## The fixed-step loop

The engine drives simulation at a fixed timestep with interpolated rendering. The
loop lives in `flicker-core` and is plugged into the winit event loop by
`flicker-app`. Stub — flesh out once the loop is implemented.

## The sprite rendering pipeline

Sprites are batched into a single draw call per atlas using a wgpu render pipeline
defined in `flicker-render`. `flicker-2d` builds the higher-level Sprite, Tilemap,
and Camera2D abstractions on top of that pipeline. Stub — describe the actual
pipeline once it exists.

## Scripting integration

`flicker-script` embeds Luau via mlua. Games register host bindings through a
typed registration API; scripts can drive entity behavior and dialogue. Stub —
describe the binding lifecycle once it is implemented.

## Networking model

`flicker-net` is the client side of the live-session, sync, and auth protocols.
Servers live in separate repositories. Hot-path messages use bincode; control-path
calls use JSON over WebSocket or HTTPS. Stub — document message envelopes here.

## Client/server split

The engine ships only the client; live game state, chat, position telemetry, and
combat physics run in separate server projects. Persistent storage (loot, inventory,
character data) goes through an existing web backend. Stub — diagram the trust and
data-flow boundaries once they stabilize.
