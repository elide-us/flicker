# clayengine

The bottom crate of the voxel stack: the single, authoritative home for the
**world-defining constants** — the size of the world's atoms, the depth of its
level-of-detail ladder, and the physical scale everything is measured in. It
depends on nothing, so every layer above (storage, primitives, contouring,
meshing, navigation) can read these numbers without depending on one another
merely to learn the shape of the world. Four `const`s, one guard test, no
runtime.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

Three flicker words appear below: a **voxel** is the atom of the voxel world (a
small cube of space); a **cluster** is the fixed-size block of voxels that is the
unit of storage and streaming; **LOD** (level of detail) is the stride at which a
cluster is sampled — higher LOD, coarser mesh. The types that carry these
concepts live in [`flicker-voxel`](../../world/flicker-voxel/README.md); this
crate only fixes their governing numbers.

## Where it sits

- **Builds on:** nothing — this is the floor of the stack, zero dependencies.
- **Used by:** [`flicker-voxel`](../../world/flicker-voxel/README.md) (cluster
  storage, contouring, meshing, LOD, nav) and
  [`flicker-primitive`](../../content/flicker-primitive/README.md) (stampable
  shapes + heightmap). Those two are peers that both read this crate and never
  import each other; the whole world stack sits above them. `flicker-voxel`
  re-exports `CLUSTER_DIM` and `VOXEL_COUNT` for its own callers' convenience.
- **Reads from the content tree:** nothing.

## Public API

Four public constants. All are compile-time `const` — there is no other surface.

| Constant | Value | What it is | The one thing to know |
|---|---|---|---|
| `CLUSTER_DIM: u32` | `256` | Side length of a cluster, in voxels (a cluster is `CLUSTER_DIM³` voxels). | **The root canon.** `VOXEL_COUNT` and `MAX_LOD` are both computed from it. Must stay a power of two — the LOD ladder strides by `2^L`, so `MAX_LOD` is only meaningful if this is a power of two. |
| `VOXEL_COUNT: usize` | `16_777_216` | Voxels in one cluster (`CLUSTER_DIM³`, = 256³). | Derived from `CLUSTER_DIM` in-crate — never set it independently. |
| `MAX_LOD: u8` | `8` | Coarsest **usable** level of detail: the LOD at which a whole cluster reduces to one sample vector. | Derived as `log2(CLUSTER_DIM)` (`trailing_zeros`). At LOD `L` a cluster holds `CLUSTER_DIM >> L` samples per axis; `8` is where that reaches exactly 1. Not the same number as `flicker-voxel`'s `ClusterId::MAX_LOD` — see Sharp edges. |
| `FEET_PER_VOXEL: f32` | `0.5` | Physical edge length of one voxel, in feet — a voxel is a 6-inch cube. | The canonical foot↔voxel conversion for any world-space measurement (nav distances, physics, render scale). At `0.5`, a 256-voxel cluster is 128 ft on a side. Used directly by `flicker-voxel`'s nav today; available to every consumer. |

**These four are one source of truth** (canon-unanimity, MCP rule `13DDA9FD`).
Downstream crates **import the symbol** — a bare `256`, `16777216`, `8`, or `0.5`
standing in for one of these anywhere else is a second source of truth that will
drift silently the day the world is re-scaled. `VOXEL_COUNT` and `MAX_LOD` are
computed from `CLUSTER_DIM`, so changing `CLUSTER_DIM` alone re-derives both and
re-shapes the world; the guard test then forces the new values to be stated
deliberately.

## Interactions

None — pure compile-time constants. No signals, no Model keys, no results, no
threads, no I/O.

## Gates

- `fundamentals_match_spec` — pins `CLUSTER_DIM == 256`, `VOXEL_COUNT == 256³`,
  `MAX_LOD == 8`, the derivation `CLUSTER_DIM >> MAX_LOD == 1`, and
  `FEET_PER_VOXEL == 0.5`. Change any value (or break the `CLUSTER_DIM`→`MAX_LOD`
  relationship) without updating this test and `cargo test -p clayengine` fails.
  It pins *this crate's* values only — it does not catch a downstream hardcoder.

## Sharp edges

- **`MAX_LOD` here (8) is not `ClusterId::MAX_LOD` there (15).** This crate's
  `MAX_LOD` is the coarsest *usable* LOD. `flicker-voxel`'s `ClusterId::MAX_LOD`
  (= `0xF` = 15) is the width of a 4-bit LOD field, a different concept wearing
  the same name. For the real usable ceiling use `flicker_voxel::Lod::MAX`, which
  is defined as `Lod(clayengine::MAX_LOD)` and rejects `level > 8`. See
  [`flicker-voxel`'s README](../../world/flicker-voxel/README.md).
- **Import, never restate.** These are the canon (`13DDA9FD`); read the symbol,
  do not copy the literal into another crate.
- **`CLUSTER_DIM` must remain a power of two.** `MAX_LOD` is its base-2 log; a
  non-power-of-two would make `MAX_LOD` and the whole LOD ladder meaningless.
- **`FEET_PER_VOXEL` is `f32`** — the scale factor for world-space math, not a
  high-precision geodetic unit.
