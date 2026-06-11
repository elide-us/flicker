# hex-map handoff (flat map + fence/dome view + seam topology)

State of `examples/hex-map` as of the 2026-06 working session. The client is a
**data-visualization tool**, not a gameplay system: the two maps are flat, and
all ring/dome tilting is purely to show celestial orientation.

## Where we are

**Slice 1 (done, committed):** the map is **flat**. All the dome/ring/tilt
*geometry* from the earlier sphere version was removed — `dome_position`,
`tilt_to_normal`, `ring_dome_radius/lift`, `dome_radius`, the guide circles and
meridian arcs, the `dome_center` field, the hover "stretch". Two flat hex maps
sit side by side (right = north, left = south, record-flipped), each rolled by
its **wheel**. `ring_dome_angle(k, rings)` is kept **only** as the celestial
latitude math (`k·90°/rings`); nothing draws a dome.

**Fence view (done, toggle `G`):** folds the flat map into a **faceted dome**
for inspection. Every ring tilts up by its even subdivision of the quarter-turn
— ring `k` of `rings` stands at `k·(90°/rings)`: centre flat, inner rings
steeper, equator vertical (six "fence" walls per map). Each ring side is relaid
collinear so every facet is coplanar; a hex point swings to vertical (±Y), face
out. North rises +Y, the record-flipped south hangs −Y. Slider capped at
**4 rings (UI only)** — topology/world-gen still handle more.

**Adjacency (DONE — deterministic, no proximity):** the across-equator stitch is
now the σ-zipper on **tile numbers only** (below). `neighbors_of` (main.rs) and
`equator_fill` (terrain.rs) no longer do the 3D-nearest search — both call
`topology::Topology::equator_partners`, so the highlight and the generated world
are roll-independent by construction. Same-map (inter-ring) adjacency stays on
`build_within_neighbors` (logical proximity) — that was always exact and
roll-independent; the fold only re-labels its *edges*, it doesn't move which
tiles touch.

## The edge model (the key the user handed over)

Each tile carries `a–f` clockwise from its **own NW edge**. The six edges have
fixed **fold roles**: `a`/`f` fold inward (toward the pole), `c`/`d` fold outward
(toward the equator), `b`/`e` are the same-ring tangential joins. North is drawn
in its own frame; the **south is flipped onto its bottom face**, so the same
local `a–f` *reads* `c b a f e d` top-down — i.e. the record flip is the
involution **σ = (a↔c)(d↔f)**, `b`/`e` fixed. So "mesh same edge in each tile's
own frame" (a-to-a) shows up in the *drawn* labels as σ. Constant across all six
sides — the labels rotate with the ring, so there is **no per-side letter
table**.

## The verified seam rule (now implemented)

The equator is a **σ-zipper** between the two outer rings (each `6·rings`
tiles, equal counts — no fan):
- north tile at ring index `i`: `c → south[i]` (primary), `f → south[i+1]`
  (bridge), modulo the ring; reciprocally south `a → north[j]`, `d → north[j-1]`.
- Distinct neighbour counts: equator **edge** tiles reach 6 (4 within + 2 cross),
  equator **corners** 5 (3 within + 2 cross). The old "always 7-tile highlight"
  invariant was flexed accordingly.
- Reproduces the hand-authored fence excerpts exactly (`60c-84a 84d-59f …` and
  the within-hemisphere `27c-45a 45f-26d …`), pinned by topology tests.

## topology.rs — the join rules (implemented)

`topology::Topology` knows the two maps' numbering for any ring count. Now
provides, beyond `ring_class` / `ring_side` / `equator_fence`:
- `fold(lo, hi, cross)` — the **reusable crease template**: primary `lo.c–hi.a`,
  bridge `hi.f–lo'.d` (within a hemisphere) or `hi.d–lo'.f` (`cross`, the σ swap
  on `{d,f}`). Acyclic; reproduces a fence excerpt verbatim from its two side
  sequences.
- `equator_cross()` — the full **cyclic** equator instance of `fold`.
- `equator_partners(n)` — the two cross twins of an equator tile (drives
  adjacency). Empty off the equator.
- `equator_edge_partner(n, edge)` — the **corner disambiguation rule**: which
  single twin is reached over a given drawn cross edge (`c`/`f` north, `a`/`d`
  south). This is the sub-tile wedge primitive — nearest-edge → one twin, so the
  shared corner never needs the whole fan.
- `JoinKind` enum — taxonomy; `EquatorCross` is the one implemented here.

## Next up

- **Inboard/Outboard edge-labeling via `fold`.** Inter-ring *tile* adjacency is
  already correct (logical proximity), but its **drawn edges** are still assigned
  geometrically in `edge_slots`/`build_rosette`. Feed those folds their side
  sequences through `fold(.., cross=false)` to label edges exactly (`c/d` out,
  `a/f` in). The fan is `k` inner ↔ `k+1` outer; the per-side offset (where the
  surplus tile rolls over = the corner) still needs deriving — only one
  within-hemisphere fence sample exists so far, so confirm with a second side.
- **Wedge UI hookup.** `equator_edge_partner` is the cross half; assemble a
  `dir → dominant drawn edge → neighbour` resolver (within half = logical
  adjacency) once there is a travel/render consumer. Deferred to avoid dead code
  overlapping `edge_slots`.

## Note: the sea-level test went green

`terrain::tests::six_epoch_layers_retained_per_tile` (the pre-existing
`sea_level != 0` red) now **passes** — the deterministic, symmetric adjacency
changed the hydrosphere's neighbour inputs. It was an adjacency-sensitive
world-gen content assertion, as suspected; no longer a known issue.

## Verify

`cargo build/clippy/test -p hex-map` (all green except the sea-level test above).
The user runs the app themselves; `G` toggles the fence/dome view, the slider
changes ring count, the wheels roll each map.
