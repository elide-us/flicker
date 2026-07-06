# hex-map handoff — modular client + the toroidal navigator

State of `examples/hex-map` after the 2026-06 working sessions. The client is a
**data-visualization / troubleshooting tool**, not gameplay: two flat hex maps
(north = right, south = left, record-flipped) stitched into one continuous
"planet" by the σ-zipper, plus a **navigator** for walking that planet and
checking the stitch holds.

## 1. Module map (the refactor)

`main.rs` went from one ~1928-line god-file to a ~900-line scene orchestrator
plus cohesive modules. `pub use geom::*` at the crate root re-exports the
geometry so `crate::…` paths (incl. `terrain.rs`) keep resolving.

| Module | Owns |
|---|---|
| `geom.rs` | hex math + spacing/size consts, `HexInst`, the two-map layout builders (`build_hex_instances`, `build_within_neighbors`, `first_ring`/`left_ring`/`ring_offsets`, `left_center`, `flip_ns`, `hex_corners`, `edge_normal`, `hex_center`, `ring_dome_angle`, `hex_world_corners`, `ray_triangle`, `build_hex_fill_mesh`), and the **placement formulas `sep(rings)` / `wheel_z(rings)` / `map_radius`**. No Renderer. |
| `text.rs` | glyph atlas + disc texture, `glyph_*`, `smoothstep`, `draw_text_billboard` |
| `gadget.rs` | `WheelAxisGadget` — one map's roll wheel + XYZ compass as a unit; `north()`/`south()`/`set_placement(cx, wheel_z)`/`transform()`/`center()`/`wheel_hit`/`apply_drag`/`paint_compass`/`paint_wheel`. Plus `roll_transform`/`draw_wheel`/`draw_compass`/`arrowhead`/`project_to_screen`. |
| `map_structure.rs` | `MapStructure` — one hemisphere: `is_south` + its gadget + the **fence/dome fold** (`fence_frame`/`tile_fence_center`/`tile_tilt`). Tile *data* stays in the scene's shared `Vec<HexInst>`; this is a placement+behaviour view (`owns`). Also the **free tile-draw primitive** `draw_tile`/`draw_hex` + `TileAssets` (reused by the navigator) + EDGE/tint consts. |
| `snap_map.rs` | `SnapMap` (the click-to-select fold-in) and **`horizon(hexes, within, rings, center)`** — the navigator's flat patch: centre + its neighbours, within on logical edges, **cross-twins on the outward edge** (toward the equator). |
| `snap_segment.rs` | `SnapMapSegment` — the navigator panel (turtle + horizon, drawn above and centred between the maps). |
| `topology.rs` | the σ-zipper (unchanged by the refactor) — `equator_partners`, `equator_cross`, `equator_edge_partner`, `fold`, `ring_class`/`ring_side`/`equator_fence`. |
| `terrain.rs` | world-gen six-epoch stack + meshes; `celestial_dirs` (a **stub** sphere mapping — see §5). |

`HexScene` (main.rs) owns `hexes`/`within`/`terrain`/`world`/`[north, south]:
MapStructure`/the navigator state/slider/HUD/fence flag and the `Scene` impl.

## 2. Placement: ring-scaled separation (slice 6 + 6b)

- `sep(rings) = 2·(rings·HEX_SPACING + HEX_SIZE) + CLEAR_GAP` — the south map's
  **centre column** `cx`. Grows with the ring count so the maps never overlap.
- **Unified anchor**: the south map's mirror axis = roll column = compass = `cx`.
  `build_hex_instances` reflects with `cx + left_center().x − p.x` (one reflect,
  was two). `MapStructure::set_placement(cx, wheel_z)` slides the whole unit
  (tiles' roll axis, wheel, compass) keeping its roll.
- `wheel_z(rings) = −(map_radius(rings) + WHEEL_MARGIN)` — wheels scale **south**
  with the ring count so a bigger map can't draw over them.
- `MAX_RINGS = 5`. `set_rings` recomputes `sep`/`wheel_z` for both maps.

## 3. The navigator (slice 7 — `SnapMapSegment`)

A small flat hex-flower above/between the maps with a Logo-style **turtle**;
the world scrolls under the fixed turtle as you travel. Purpose: watch which
tiles the engine snaps as you walk, and verify toroidal continuity.

**State (HexScene):** `player_tile: u32`, `player_offset: Vec3` (sub-tile, hex
plane), `heading: f32` (yaw; 0 = north), `navigate: bool`.

**Controls:** `N` toggles navigate mode (RMB-look suspended). **A/D turn** the
heading, **W/S move** along it. Map stays north-up; the turtle rotates. Turtle
also drawn **on the hemisphere map** (`draw_map_turtle`, world-+Y lift so it's
visible on the flipped/rolled south map) while navigating. HUD shows
`Navigate [N]` + `turtle on <tile>`.

**Movement / crossing (`resolve_crossings`):** the movement cell tiles
**gap-free** (apothem `HEX_SPACING/2`, NOT the visual apothem — this was the
ping-pong fix). When the offset leaves the cell it steps onto the neighbour on
the **geometric edge you walked into** (`neighbor_in_dir`, via `horizon`) and
**resets the offset into the new tile** (translation `off − n_off`). **Crossing
onto the other map (the equator seam) carries the record flip**: x-mirror the
offset, negate the heading — without this the turtle ping-pongs across the seam
(the bug that bit 33↔52). No walls on outward edges; the only walls are the
**12 pentagon-defect corners** (clamp; step around them).

**Rendering:** `horizon` patch, scrolled by `−offset`, culled to
`HORIZON_RADIUS = HEX_SPACING` (1 tile underfoot, 2 at an edge, up to 3–4 at a
join — never the whole rosette). Panel at `(sep/2, PANEL_Y=5000, 0)`, scale 0.55.

## 4. The stitch (topology.rs)

In drawn labels every fold pairs edges by **σ = (a↔c)(d↔f)** (b/e fixed); σ is the
record-flip x-mirror. Equator cross: north tile `i`'s `c` ↔ south `i` `a`
(primary), `f` ↔ `i+1` `d` (bridge), mod `6·rings`. `equator_partners` gives the
two twins; `equator_edge_partner` the per-edge twin. Equator **edge** tiles have
6 neighbours, **corner** tiles 5 (the defects).

## 5. The irrationality — and the deliberate fakes (read this)

A sphere of only hexagons is impossible (Euler): you need **12 pentagons**. This
model has exactly **12 five-neighbour equator-corner tiles** = those pentagons.
See memory `world-scale-fake-geodesics`:

- A constant-heading walk is a **straight line in tile-space but a curve on the
  sphere**. At **~50-mile tiles** and slow play (sun crossing the sky = hours),
  the curvature is **invisible** — so we **fake it**. Do NOT build true geodesic
  curving.
- The **celestial sim** must NOT fake it: drive the sun off the player's
  **virtual sphere position** (`celestial_dirs` + sub-tile interp). But
  `celestial_dirs` is a **stub** (longitudes not σ-consistent across the seam);
  making it σ-faithful is the prerequisite when that's wanted (same Euler tax).

## 6. Still faked / open (where a new context picks up)

1. **Seam-frame coherence.** Crossing now carries the flip (offset x-mirror +
   heading negate) and uses the **geometric outward edge** (no walls). It does
   NOT use `equator_edge_partner`'s σ correspondence for movement — an earlier
   attempt to (fold-reflect across the σ edges `c`/`f`) put walls on every
   non-`c`/`f` outward edge and was reverted. Reconciling the geometric-outward
   crossing with the σ labels is the **"rotated local frames"** problem: the
   topology was authored on per-tile rotated fence labels, but the flat render
   gives every hex the same orientation (`a` = NW). Until those agree, the seam
   crossing is continuous + flip-carried but only approximately frame-faithful.
   Candidate direction: per-tile rotated local frame, or drive the turtle in the
   celestial/sphere frame (needs §5's σ-consistent `celestial_dirs`).
2. **Pentagon-defect corners clamp** (walls). Arguably should be passable (5
   neighbours cover 360° with compression); currently you step around them.
3. **Horizon-radius slider** (the last original spec item) — NOT built;
   `HORIZON_RADIUS` is fixed. Zooming to ≥2 rings needs an **N-ring horizon
   walk** through the seam (depends on item 1).

## 7. Verify

`cargo build/test/clippy -p hex-map` — 23 tests green; the only clippy noise is
two pre-existing geom-test nits (a constant-value assert + a loop-index). The
user runs the app: `N` enters navigate mode, A/D turn, W/S drive; tiles scroll
under the turtle; walk across the equator and back; the map-turtle marks the
real position. Slider 1–5 rings; `G` fences; wheels roll each map; the two maps
separate as rings grow.
