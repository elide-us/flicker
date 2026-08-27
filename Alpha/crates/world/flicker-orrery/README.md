# flicker-orrery

The single, GPU-free model of the Prism solar system — the fixed **roster** of eight
worlds and the tilted-ellipse **orbits** they ride. An *orrery* is a model of the worlds
and their orbits. Both the solar-birth intro **cinematic** (the god's-eye view of the whole
system) and the from-Home **heliocentric sky** (the worlds crossing the sky above the home
world) read *this same* roster, so the two views can never disagree about which worlds
exist, where they sit, or how fast they move. It lives in the `world` cluster, depends only
on `glam` for vector math, draws nothing, and holds no state: a caller asks for the roster
and computes body positions from it.

> Design of record — why it is shaped this way, what is Prism-ruled vs. a rendering choice,
> the decisions and history — lives in the project's MCP memory, not here. This file
> documents how to use the crate.

**Vocabulary:** *Home* is the habitable home world; it alone carries the moon, and its
orbit is the unit of the calendar (one Home orbit = one canonical year). *Prism / BookV* is
the game's fixed cosmology (canon). *Layout units* are an AU-like distance unit chosen for
the viewer — not physical astronomical units.

## Where it sits
- **Builds on:** `glam` — `Vec3` and trig only. Nothing else.
- **Used by:**
  - [`flicker-solarbirth`](../../scenes/flicker-solarbirth) — the intro **cinematic**
    (god's-eye). Caches the roster, draws each body and its orbit ring, sizes bodies by
    composition class, and frames the camera on the system envelope.
  - [`flicker-pocclusters`](../../scenes/flicker-pocclusters) — the from-Home
    **heliocentric sky** (the "Cluster Editor" scene). Solves each world's apparent
    direction in the sky from the same roster.
  - `prism-alpha` pulls both scenes in but never names this crate directly.
- **Reads from the content tree:** nothing. There are no files, no scene data, no tokens —
  the roster is fixed Rust data.

## Public API

### The roster and a body
| Item | What it is for | The one thing to know |
|---|---|---|
| `roster() -> Vec<Planet>` | The whole system: the eight worlds, inner → outer | **Allocates** a fresh `Vec` of all 8 rows on every call — it is fixed data with no seed, so cache it; do not call it per frame. Always the same eight in the same order. |
| `Planet` | One world: its identity, orbit, and look | Plain data, every field `pub`. `planet_pos` / `orbit_ellipse` consume a `&Planet`. |
| `BodyKind` — `Rocky` · `GasGiant` · `IceGiant` · `Dwarf` | A world's composition class | The cinematic sizes bodies by this; the sky ignores it (all sky discs share one apparent size). |
| `BodyKind::label(self) -> &'static str` | Short HUD text — `"rocky"`, `"gas giant"`, `"ice giant"`, `"dwarf"` | A ready readout string; the cinematic uses it in its roster legend. |

`Planet` fields (all `pub`):

| Field | Meaning | Note |
|---|---|---|
| `name: &'static str` | World name — `"Chaos"`, `"Fire"`, `"Home"`, `"Earth"`, `"Light"`, `"Air"`, `"Water"`, `"Death"` | The ruled eight, inner → outer. |
| `kind: BodyKind` | Composition class | See `BodyKind`. |
| `color: [f32; 3]` | Linear-RGB school colour | Used **unlit** — the sun's point light shades it. |
| `a: f32` | Semi-major axis (layout units) | Orbit size; strictly increases inner → outer. |
| `e: f32` | Eccentricity (0 = circle) | Small for the inner rockies; largest for the outer dwarf (Death). |
| `incl: f32` | Inclination (**radians**) | Orbit-plane tilt out of the reference (XZ) plane. |
| `node: f32` | Longitude of ascending node Ω (**radians**) | Which compass direction the tilt leans. |
| `radius: f32` | Visual sphere radius (layout units) | An **independent** per-body constant — see Sharp edges; not derived from `kind`. |
| `phase0: f32` | Starting orbital angle (**radians**) | Spread so the worlds don't line up. |
| `rings: bool` | Air alone is `true` | |
| `occulted: bool` | Death alone is `true` | Rendered near-black; known by its shadow transit. |
| `moon: bool` | Home alone is `true` | Home carries the moon. This flag is how a consumer finds Home: `roster().iter().find(\|p\| p.moon)`. |

### Placing a body in space
| Item | What it is for | The one thing to know |
|---|---|---|
| `planet_pos(p: &Planet, t: f32) -> Vec3` | A body's world position at animation time `t` (seconds) | Sun at the origin. The angle advances **uniformly** — this is a cinematic, not a Kepler's-equation solve; the ellipse *shape* carries the realism, not physically exact speeds around it. |
| `orbit_ellipse(p: &Planet, segs: usize) -> Vec<(Vec3, Vec3)>` | The faint orbit ring as `segs` line segments | Samples the **same** tilted ellipse `planet_pos` rides, so the ring and the body can never drift apart. |

### The reckoning clock (one shared clock for both views)
| Item | What it is for | The one thing to know |
|---|---|---|
| `HOME_YEAR_SECONDS: f32` (`160.0`) | Cinematic seconds for Home to complete one orbit — one canonical **year** | Cosmetic pace only; retune freely — the *ratios* between bodies are fixed by Kepler's third law, not by this number. |
| `A_HOME: f32` (`2.8`) | Home's semi-major axis — the period anchor | Home's `a` in the roster equals this. |
| `orbital_period_years(a: f32) -> f32` | A body's orbital period in Home-years — Kepler's third law, `(a/A_HOME)^1.5` | Home is exactly `1.0`. Exposed for a calendar / celestial-year readout; today read only inside this crate (via `orbit_omega`) — **not yet consumed by a scene**. |
| `orbit_omega(a: f32) -> f32` | Mean angular speed (rad/s) so Home sweeps one year per `HOME_YEAR_SECONDS` | Used inside `planet_pos`; exposed for a caller that wants the raw angular rate directly. |

### The layout envelope
| Item | What it is for | The one thing to know |
|---|---|---|
| `SYSTEM_INNER: f32` (`0.4`) | Inner radius of the system in layout units | A **layout** scale, not a Prism-ruled distance. |
| `SYSTEM_OUTER: f32` (`15.5`) | Outer radius; clears Death's aphelion with margin so a dust cloud or rings can frame the whole system | The cinematic reads it live to frame its camera, and mirrors it into its dust recipe — see Sharp edges. |

## Interactions

**None in the flicker sense** — `flicker-orrery` is pure GPU-free data and math. It captures
no input signals, publishes and binds no Model keys, renders nothing, touches no files, and
runs no threads or workers. Its only coupling is that two scene crates read the roster and
place bodies with it:

- **flicker-solarbirth** (cinematic): caches `roster()`, draws each body with `planet_pos`
  + a `radius`-scaled sphere coloured by `color`, draws its `orbit_ellipse` ring, builds a
  legend line per world from `kind.label()` + `moon`/`rings`/`occulted`, filters the gas/ice
  giants by `kind`, spaces the dust lanes by each body's `a`, frames the camera on
  `SYSTEM_OUTER`, and a gate in that crate ties its dust-disk geometry back to `SYSTEM_INNER`
  / `SYSTEM_OUTER`.
- **flicker-pocclusters** (sky read): calls `roster()`, finds Home via `.moon`, and solves
  each world's apparent sky direction geocentrically — `planet_pos(p, t) − planet_pos(Home, t)`
  with `t = clock * HOME_YEAR_SECONDS` — then dims Death by `.occulted`, brightens Air by
  `.rings`, and tints by `.color`. It ignores `radius` and `kind` (every sky disc is one
  apparent size).

## Gates
`cargo test -p flicker-orrery` (2 tests):

- **`roster_is_the_ruled_eight`** — the eight names in inner → outer order; `a` strictly
  increasing; exactly one moon-bearer (Home), one ringed (Air), one occulted (Death); the
  class sequence `[Rocky×4, GasGiant×2, IceGiant, Dwarf]`; sizes rank by class (giants >
  ice giant > rockies > dwarf); inner rockies near-circular and Death the most eccentric;
  Death's aphelion inside `SYSTEM_OUTER`; Home's period exactly one year and periods growing
  with distance.
- **`planet_pos_respects_orbit_geometry`** — every sampled point stays on the body's ellipse
  (distance to the sun within `[a(1−e), a(1+e)]`), both perihelion and aphelion are reached,
  and an inclined orbit leaves the reference plane.

## Sharp edges
- **`roster()` allocates.** It rebuilds all eight rows and returns an owned `Vec` on every
  call. The data is fixed — cache the result and reuse it. (The cinematic caches it once; the
  sky currently rebuilds it each frame.)
- **Find Home by the flag, not by index:** `roster().iter().find(|p| p.moon)`. The order is
  gated, but the `moon`/`rings`/`occulted` flags are the contract for "which world is which".
- **`incl`, `node`, `phase0` are radians** on the public surface (they are authored in
  degrees *inside* `roster()` and converted there).
- **`radius` is not derived from `kind`.** It is an independent per-body constant; the gate
  only enforces that sizes *rank* by class. If you add a world or change a class, set the
  radius too — the gate catches a wrong *ordering*, not a wrong value.
- **What you may tune vs. what is canon-locked.** Tunable (rendering choices): every per-body
  `a` / `e` / `incl` / `node` / `radius` / `color`, plus `SYSTEM_INNER` / `SYSTEM_OUTER` /
  `HOME_YEAR_SECONDS`. Canon-locked (changing them contradicts Prism canon): the eight names,
  their inner → outer order, one-moon / one-ring / one-occulted, and the composition classes.
  (Why each is ruled is in MCP, not here.)
- **`SYSTEM_OUTER` has one live reader and one gated mirror.** The cinematic reads it live to
  place the camera and the orbits, but its dust-disk `inner`/`outer` live as literals in
  `solarbirth.scene.json` — kept equal to `SYSTEM_INNER` and `SYSTEM_OUTER × 1.4` only by a
  gate in `flicker-solarbirth`, not by a live bind. Retune the constant and that gate is what
  flags the now-stale scene file; nothing updates it automatically.
