# Handoff — hex-world local-render model + the two pole quirks

> **⚠ SUPERSEDED** by the *HexWorld — Flat Neighbor Graph & Celestial Orientation*
> spec. The sphere-as-substrate model in this doc is **retired**: the world data
> is now a flat hex graph with baked edge-neighbour refs, the render is a flat
> local bubble, and the sphere survives only as a read-only per-hex celestial
> orientation. Kept for history; do not implement from it.

> Captures the design that the `examples/hex-world` prototype proved out, and
> **flags two intentional pole-region cheats** so a future session doesn't
> "fix" them or re-litigate the topology. Re-verify code anchors (line numbers
> drift); the decisions below are the load-bearing part.

## Governing principle — we never assemble the sphere

Rendering is **local-only**: at most ~7 hexes (the focus hex + its ring of
neighbours) are ever built, and they're **gnomonic-flattened onto the focus
hex's tangent plane** (the "planar snap"). As the camera crosses a hex
boundary the focus changes and the patch re-snaps onto the new plane. The
sphere's curvature is paid out **across the sequence of snaps**, never inside a
single patch.

Consequence: **global tiling perfection is a non-requirement.** A patch only
has to mesh *locally*. This is the whole reason the hex model is sufficient.

## Why the cubed-sphere was explored and then shelved

A cubed sphere (square tiles) was seriously considered: squares fill a 2048²
texture with no wasted corners, the sphere closes with 8 mild 3-valent corners
instead of 2 convergent poles, and adjacency is a uniform 4-grid. It was
**shelved** because it solves *global* clean tiling — which local-only rendering
makes unnecessary — and the hex model already meshes locally everywhere except
the poles (which we fake regardless). Recorded so it isn't re-explored without a
new reason (e.g. if a future requirement actually needs a globally-assembled
mesh, or whole-texture efficiency dominates).

## Hex topology, as built (`examples/hex-world/src/topology.rs`)

- Polar lat/lon rings: ring `k` holds `6k` hexes; **total = `2 + 6R(R+1)`**
  (R=1 → 14, R=3 → 74). Two hemispheres, mirror-joined at the equator with **no
  shared band** (that's why the minimum is 14, not 8).
- Latitude spacing **`90°/(R+0.5)`** so the two equator rings *straddle* the
  line instead of coinciding (regression-tested).
- Fold adjacency is identity in longitude: `N(R,p) ↔ S(R,p)`. The equator is
  the *clean* seam — same count both sides; the southern hemisphere carries a
  **+half-equator-cell longitude offset** so the teeth interlock.
- Adjacency is longitude-sector overlap → **variable neighbour count (4–6)**,
  validated by an adjacency-symmetry test (the same test that guards the fold).

## Hex shape & meshing (`planet_mesh.rs`)

- **Pointy-top** hexes: flat E/W edges (shared cleanly with in-ring neighbours),
  pointed N/S (interlocking teeth across rings). Built in the hex's own tangent
  frame, then projected onto the focus plane.
- `EDGE_RATIO` = flat-band fraction; `BAND_HALF` = how far points overhang so
  teeth *overlap* rather than merely touch.
- **Variable neighbour count is not a hole.** A hex with 5 neighbours just means
  a *larger* (sparser-ring) neighbour seats across two of its edges; with the
  ring column offset right it closes with no gap. The "missing tile" wedges seen
  during bring-up were offset bugs, not topology.

## The pole — a join of folded surfaces

At each pole the surrounding hexes must **fold downward** so their 30° edges
meet — the curvature concentration the cube would have put at corners, here put
at the 2 poles. It cannot be flattened away inside one patch. Handling:

- **Resolve at locality**, not globally. The bent join only has to look right in
  the ≤7-hex patch you're actually standing in.
- **Keep the real data, fake only the render.** The pole region stays real
  ring-0/ring-1 hex textures so the simulation (ice/water/erosion) reads it
  normally; the renderer draws a **faked cap** over it, marked untravellable.
  (Current code still renders the crown + 6 pentagons — a partial fake cap; the
  intended end state is a simpler cone-style cap over the same real data.)

## ⚑ FLAGGED QUIRK 1 — the ring ±1 pole neighbour (5, not 6)

The hex chain **ends** at the pole: there is no "next hex" across the pole to
continue a flat patch into. So a near-pole hex's poleward direction comes up
empty — a polar hex effectively has **5 real neighbours, not 6**.

- Do **not** assume 6 neighbours at ring 0/1 in nav / sim / render code; the
  poleward slot may be empty by design.
- We deliberately **do not fully resolve** the across-the-pole continuation. At
  LOD8 the horizon is ~1–3 tiles, so polar terrain at range is ≈ dots and is
  shadow-faked with other techniques rather than truly meshed across the pole.

## ⚑ FLAGGED QUIRK 2 — horizon-extent "read a tile twice" magic

To fill the visible horizon where there is no real neighbour (the pole, and
possibly other ring-boundary extents), we may **deliberately re-read/duplicate a
tile** to fake the extent. This is an **intentional cheat**, not a bug —
justified because terrain at that range is low-LOD/dots. Flag it wherever it's
implemented so a later pass doesn't "correct" it into real work.

## Horizon / LOD context

`MAX_LOD = log2(CLUSTER_DIM=256) = 8`. The original hex rationale assumed a much
larger horizon (LOD15 spanning multiple hexes); at LOD8 the horizon is small,
which is *why* local-only rendering is enough and why the pole fakes above are
invisible in practice.

## Remaining work on the prototype

1. Per-ring **column offsets** so non-pole patches close without the wedge
   (extends the equator's half-cell idea to every ring boundary).
2. The pole **cap over real data** (replace crown+pentagons with the simpler
   faked cap; mark untravellable).
3. (Later) wire tile heightmaps to real world-gen/ledger data instead of the
   procedural `world_height` — the bridge to `docs/flicker-world-system-spec.md`.
