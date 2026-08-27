# Authoring a stage — recipes, rigs, passes

A companion to [`README.md`](README.md). The README's
[Stages](README.md#the-theme-the-style-file-the-stages) section tells you what a stage *is*
and where it lives; **this file is the full reference** for writing one — the lighting rig,
the recipe of passes, the attachments they draw into, and every problem the compiler will
report back at you. Read it when you are building a lit 3D surface: a turntable, a globe, a
world with a sky and fog, a cinematic with HDR and a colour grade, a room with a fire that
casts a shadow.

> **Scope.** This is the usage guide — how to author a stage. The design of record (why the
> system is shaped this way, the decisions, the migration history) lives in the project's MCP
> memory bank, never here. This file documents how to use it.

The one compiler is `flicker::ui::stage_def` (source: `flicker-widgets/src/stages.rs`); the
typed values it produces live in `flicker-render` (`stage.rs`, plus one `pipeline_*.rs` per
effect). You never touch either — you author JSON, and a build gate compiles every stage you
ship.

---

## Contents

1. [The 60-second model](#the-60-second-model)
2. [The smallest stage](#the-smallest-stage)
3. [The one subtle concept: order is derived, never authored](#the-one-subtle-concept)
4. [attachments — the images a surface owns](#attachments)
5. [passes — the roster](#passes--the-roster) (and **which passes render from data alone**)
6. [Per-pass keys](#per-pass-keys)
7. [rate — how often a surface re-renders](#rate)
8. [Lighting rigs — the trio, the list, the drivers](#lighting-rigs)
9. [Binds — replace, and the quiet typo](#binds)
10. [Worked example: Solar Birth (HDR volumetric)](#worked-example-solar-birth)
11. [Worked example: a room with a sun shadow](#worked-example-a-room-with-a-sun-shadow)
12. [Water — the animated water mesh](#water)
13. [Bloom — HDR glow](#bloom)
14. [How to extend](#how-to-extend)
15. [The gates your stage must pass](#the-gates)
16. [Sharp edges](#sharp-edges)

---

## The 60-second model

A **stage source** (or just *stage*) is a named offscreen sub-scene: a lighting rig, an
optional clear colour, an optional camera, a list of content **layers**, and a **recipe** of
engine **passes** that draw around that content into named **attachments** (the images the
surface owns). A `surface` node in your scene tree names a stage in its `source` prop and
composites the result into its rect. **The node says WHERE; the stage says WHAT.**

Two homes, one rule each:

- **A stage only one scene uses lives in that scene's file**, under the top-level `stages`
  section (a sibling of `tree`, `styles`, `exits`).
- **The shared library is `resources/ui_stages.json`.** Today it holds only the `lighting`
  presets every stage names (`studio`, `night`, `deep_space`, `hearth`). A scene *names* a
  preset; it never authors one, and it may not reuse a library source name.

Everything merges into one root at load, so a stage in your scene file and a preset in the
library resolve as if they were one file. The compiler reports every authoring problem by
name — at build time through [the gates](#the-gates), at runtime as a warning with the same
words. **A bad value still degrades to its default: a malformed stage costs the authored
look, never the picture.**

---

## The smallest stage

A stage that names nothing but a lighting rig gets the **default recipe** — one `scene` pass
into a colour + depth pair, re-rendered live:

```jsonc
"stages": {
  "my_turntable": { "lighting": "studio" }
}
```

That is a complete, valid stage. `attachments`, `passes`, `camera`, `clear`, `layers` and
`rate` are all optional; absence is the pre-recipe behaviour. Add a camera and a content
layer and you have a turntable:

```jsonc
"stages": {
  "my_turntable": {
    "lighting": "studio",
    "camera": { "kind": "orbit", "yaw": 0.55, "pitch": 0.18, "dist": 2.6, "target_y": 0.95 },
    "layers": [ { "draw": "material" }, { "draw": "grid", "y": -0.5 } ]
  }
}
```

`camera.kind` is `"orbit"` — the only framing kind; anything else is a problem. A stage with
**no** `camera` lets the scene's own camera own the view (that is how the globes work). The
`camera` defaults are the portrait framing above (`yaw 0.55, pitch 0.18, dist 2.6,
target_y 0.95`), and a partial camera fills the rest from those.

> `layers` draw kinds (`skinned` · `ring` · `grid` · `shells` · `shell` · `graticule` ·
> `material`) are **filler-specific**: a behaviour draws only the kinds it knows and *warns
> at load* naming the ones it was handed and cannot draw. The layer catalog is in the
> [README's stage table](README.md#the-theme-the-style-file-the-stages); their keys are
> listed under [How to extend](#how-to-extend).

---

## The one subtle concept

**You never write a draw order.** A recipe is a *set* of passes; the order they execute is
**derived** from what each pass `reads` and `writes`. A pass that reads an attachment runs
after every pass that writes it. Declaration order is only the tie-break between two passes
that don't depend on each other.

```jsonc
"passes": [
  { "pass": "sky" },
  { "pass": "scene" },
  { "pass": "ground_fog", "reads": ["depth"], "writes": ["color"] }
]
```

`ground_fog` reads `depth`; `scene` writes `depth`; therefore fog runs after the scene, no
matter what order you list them in. Every name in a `reads`/`writes` array **must be a key of
this stage's own `attachments`** — a name nothing declares is a problem and is dropped, so
the derived order can never depend on an image that does not exist. A cycle (two passes each
reading what the other writes) falls back to declaration order and warns.

This is why there are no pass numbers anywhere, and why "make this run last" is expressed as
"make it read what everything else writes" — which is exactly how the tonemap resolves last
(it reads `hdr`).

---

## attachments

`attachments` is a map of `name → { format, scale }` — the images this surface's passes read
and write. An authored block **replaces** the default `color` + `depth` pair rather than
adding to it, so a surface that wants an HDR working image *and* a depth declares all three:

```jsonc
"attachments": {
  "hdr":   { "format": "rgba16f" },
  "color": { "format": "surface" },
  "depth": { "format": "depth32" }
}
```

| Name | Convention | What it is |
|---|---|---|
| `color` | every surface has it | the swapchain-format colour the surface composites from |
| `depth` | added by default | the depth the depth-sampling passes (fog, disk, water) read |
| `hdr` | you add it for HDR | the **linear** rgba16f working colour the lit passes write and `tonemap_grade` resolves — see [HDR](#worked-example-solar-birth) |

| Format | wgpu | Use |
|---|---|---|
| `surface` | the swapchain's format | ordinary sRGB colour |
| `depth32` | the depth format | the one depth the sampling passes read |
| `rgba16f` | `Rgba16Float` | linear HDR working colour (the `hdr` attachment) |

**`scale`** (default `1.0`) makes a half-resolution image (`0.5`) — **but only on `color`.**
The surface is sized off `color`'s scale alone; a `scale` on any other attachment is not read.
The `hdr` attachment rejects a non-`1.0` scale as a loud problem (it is resolved 1:1 by the
tonemap). A `scale` on `depth` is **silently ignored** — see [Sharp edges](#sharp-edges).

---

## passes — the roster

A pass is a whole-surface step the engine draws around your content. Nine kinds
(`PassKind::KINDS`):

**The distinction the roster does not print, and you must know:** seven pass kinds render
entirely from what you author. Two do **not** — they are *ordering markers* that only do
something when the scene's Rust behaviour wires the per-frame runtime a recipe cannot hold.
Author one of the two into a new scene and it compiles clean and draws nothing, because a
content author has no way to supply the missing half.

| Pass | Renders from data alone? | What it does |
|---|---|---|
| `scene` | ✅ | draws the surface's own content (meshes, lines, the 2D layers). The default recipe is exactly this. |
| `sky` | ✅ | the procedural sky behind the scene; its palette is the rig's sky slots (0/1). No params. |
| `volumetric_disk` | ✅ | an accretion/dust disk sampled against `depth` (the Solar Birth cloud). |
| `ground_fog` | ✅ | a localized ground-fog slab sampled against `depth`. |
| `tonemap_grade` | ✅ | the HDR resolve: reads `hdr`, ACES-tonemaps + grades into `color`. |
| `bloom` | ✅ | HDR glow: reads `hdr`, extracts + blurs its bright parts, adds them back to `hdr`. **HDR-only.** See [Bloom](#bloom). |
| `water_surface` | ✅ | a real animated water **mesh** at `sea_level` — depth-writing geometry with a sun glint, writes `hdr`. See [Water](#water). |
| `composite` | ⚠️ **marker only** | names another surface in `from`. The **actual blit is recorded by the scene's Rust** (`FrameGraph::composite_panel`), so in a recipe this pass is *only* the ordering edge — it never blits by itself. No shipped scene authors one. |
| `shadow_map` | ⚠️ **needs the scene to wire it** | casts/samples a sun shadow. The light-view-projection matrix and the producer↔consumer handoff are per-frame runtime the **scene's behaviour wires** (`begin_shadow_view`, `set_shadow_source`); the pass stands in the recipe as the ordering marker and the anchor for its [loud structural gates](#the-gates). Only `pocclusters` wires it. See [the worked example](#worked-example-a-room-with-a-sun-shadow). |

Evidence for the ⚠️ rows: `flicker-render/src/frame_graph.rs` `apply_pass` — `PassKind::Composite(_) => {}`
and `PassKind::ShadowMap(_) => {}` are no-ops; the comments there name the Rust calls the
scene must record instead.

Two structural rules the compiler enforces on any recipe:

- **A `passes` list with no `scene` pass is a problem** — your content would be silently
  dropped, so the compiler refuses.
- **`hdr` and `tonemap_grade` are mutually required** — an `hdr` attachment with no
  `tonemap_grade` pass (nothing resolves it) is a problem, and a `tonemap_grade` with no
  `hdr` attachment (nothing to tonemap) is a problem.

---

## Per-pass keys

Every key below is optional; an unauthored one takes the default shown. A key the pass does
not have is a problem naming it. Colours are `$token` refs or four numbers (the alpha is
carried where a pass takes rgba; rgb-only passes read the first three). Every `*_bind` is
covered in [Binds](#binds).

**`volumetric_disk`** — reads `[depth]`, writes `[color]` by default.

| Key | Default | Meaning |
|---|---|---|
| `inner` / `outer` | `0.3` / `15.0` | inner / outer radius (world units) |
| `snow_line` | `2.7` | a visual density feature radius |
| `scale_height` | `0.06` | disk flare (`h(r) = scale_height·r`) |
| `density` | `1.0` | extinction strength |
| `formation` | `0.0` | formation progress `0..1` (inside-out dissipation) |
| `time` | `0.0` | swirl animation clock |
| `tint` / `glow` | `(.10,.09,.10)` / `(1,.55,.25)` | bulk dust tint / warm inner glow |
| binds | — | `formation_bind` · `time_bind` · `density_bind` |

*Annular gaps (the lanes forming planets carve) are simulation output on a typed channel — no
file can author them.*

**`ground_fog`** — reads `[depth]`, writes `[color]` by default.

| Key | Default | Meaning |
|---|---|---|
| `bottom` / `top` | `-1.0` / `0.0` | slab range (world Y); dense at bottom, clear at top |
| `floor` | `0.0` | world Y **added** to `bottom`/`top` at apply time (author a slab relative to a floor the sim finds) |
| `density` | `1.0` | extinction strength |
| `noise_scale` | `0.25` | spatial noise frequency |
| `coverage` | `0.55` | fraction of the plane fogged `0..1` |
| `height_power` | `1.5` | vertical falloff power |
| `wind` | `[0.4, 0.1]` | XZ drift |
| `bounds` | `[±1e6, ±1e6]` | XZ rectangle `[min_x, min_z, max_x, max_z]` the fog is localized to |
| `edge_fade` | `1.0` | feather distance to 0 at the rectangle edge |
| `fall_depth` / `flow` | `0.0` / `0.0` | the "curtain" that spills over the rim, and its flow speed |
| `color` | *live* | rgb; **absent = the renderer's live atmosphere `fog_color`**, so fog under a day/night cycle follows it |
| binds | — | `floor_bind` · `density_bind` · `time_bind` · `coverage_bind` |

**`tonemap_grade`** — reads `[hdr]`, writes `[color]`. Must read `hdr` (that read is what
makes it resolve last; dropping it from an authored `reads` is a problem).

| Key | Default | Meaning |
|---|---|---|
| `exposure` | `1.0` | linear exposure multiply before the ACES curve |
| `grade` | `(0,0,0)` | linear-RGB grade tint the resolve lerps toward — pure art, never bound |
| `grade_strength` | `0.0` | `0..1`; `0` = pure tonemap, no tint |
| binds | — | `grade_strength_bind` · `exposure_bind` |

The tint is authored; the **strength** is bindable, which is how a grade FOLLOWS a day/night
cycle: author the golden tint once and bind `grade_strength` to a warmth the scene publishes
per frame (the Prism Test Room's `grade_warmth` — 0 with the sun high, peaking as it sits on
the horizon, 0 once it has set). Authoring a number *and* binding the same slot is the usual
dead-data problem. **No binds = the authored numbers**, so a static grade is unchanged.

**`shadow_map`** — one kind, two roles split by `from`. See [the worked example](#worked-example-a-room-with-a-sun-shadow).

| Key | Default | Role | Meaning |
|---|---|---|---|
| `from` | *absent* | — | **absent = PRODUCER**; a string names the producer surface = **CONSUMER** |
| `light` | `0` | consumer | rig slot the shadow is cast for (a `dir` light) — the light darkened AND the direction rendered from |
| `bias` | `0.0015` | consumer | depth bias vs self-shadow acne (raise if it stipples, lower if shadows detach) |
| `extent` | `512.0` | producer | half-size (world units) of the orthographic caster box; inert on the consumer |

**`water_surface`** — reads `[depth]`, writes `[hdr]`. A real animated water **mesh**, not a
flat plane. See [Water](#water).

| Key | Default | Meaning |
|---|---|---|
| `sea_level` | `0.0` | world Y of the still plane the waves displace |
| `shallow` / `deep` | `(.10,.30,.34)` / `(.02,.06,.11)` | linear RGB **body** tint at a grazing view / looking straight down; the view angle lerps between them, and the result is lit by the rig (below) |
| `shore_fade` | `4.0` | sharpness of the shallow→deep view-angle transition |
| `spec_shininess` | `200.0` | glint exponent — larger = a tighter, brighter glint. One knob for **both** sky slots |
| `spec_strength` | `1.0` | glint strength multiplier — the same one knob over both sky slots |
| `normal_scale` | `1.0` | multiplies the wave slope used for **shading** (glint choppiness) without touching the geometry |
| `wave_falloff` | `0.0015` | far-field flattening rate `1/(1+dist·k)`; bigger = the ocean goes flat-mirror closer in, `0` = waves ripple to the horizon. **Directional** sources feel this at a fraction of the rate, so the open ocean keeps swelling |
| `env_strength` | `1.0` | `0..1`; how much of the live sky the surface mirrors (Fresnel-weighted). `1` = a real water reflection, `0` = the body colour alone |
| `wave_sources` | *none* | up to **6** wave generators, radial or directional (below); absent = still water |
| binds | — | `sea_level_bind` (floods it) · `time_bind` (scrolls the waves) |

The water is **environment-lit**: its body is multiplied by the rig's `ambient` plus the diffuse
of both **sky slots** (light `0` the sun, light `1` the moon), and the rig's
`sky_zenith`/`sky_horizon` palette is mirrored along the reflected view ray and Fresnel-blended
over it. Both come from the **live** rig, so the sea follows a day/night cycle with nothing
authored per time of day — aquamarine at noon, gold at sunset, faintly moonlit at night.
`shallow`/`deep` are the *tint*, not the final colour; `env_strength` is the one dial between
"coloured water" and "mirror". (There is no `bounds`: the ocean is a projected grid that always
reaches the horizon, so an authored field box is an unknown key.)

Each `wave_sources` entry is one of **two kinds**, and it names exactly one of `center` / `dir`:

- **Radial** — `{ center: [x, z], amplitude, wavelength, speed, phase }`. Rings spreading from a
  world point: `amplitude · sin(k·distance_from_center − ω·time + phase)`. Near-shore chop.
- **Directional** — `{ dir: [dx, dz], amplitude, wavelength, speed, phase }`. A plane wave
  marching along a world direction: `amplitude · sin(k·(p·dir) − ω·time + phase)`. It has no
  centre, so it is defined *everywhere* and is what keeps the open ocean at the horizon moving.
  `dir` is normalized for you.

| Wave key | Default | Meaning |
|---|---|---|
| `center` | — | world XZ `[x, z]` the ripples radiate from; makes the entry **radial** |
| `dir` | — | world XZ `[dx, dz]` the crests travel along (normalized at parse); makes the entry **directional** |
| `amplitude` | `0.0` | peak crest height (world units); **`0` = an inert wave**, so a wave you author must set it |
| `wavelength` | `40.0` | crest-to-crest distance (world units); stored as `k = 2π/wavelength` |
| `speed` | `0.0` | crest travel speed (units/sec); stored as `ω = speed·k`, so `0` = a standing wave |
| `phase` | `0.0` | radian offset so two sources are not in lockstep |

Naming **both** `center` and `dir`, naming **neither**, a zero-length `dir`, a 7th wave source,
an unknown wave key, or a non-object entry each fail loud.

Author a directional source **long** — `wavelength` ~120–200 with a moderate `speed` — because
it is the one thing still moving out at the horizon, where a short wavelength lands under a
pixel and shimmers instead of swelling. Two of them crossing ~30–60° apart read as open water;
one alone reads as corduroy.

**`bloom`** — reads `[hdr]`, writes `[hdr]`; **HDR stages only** (a `bloom` on a stage with no
`rgba16f` attachment is a problem). Pure art knobs — no binds, no colours. See [Bloom](#bloom).

| Key | Default | Meaning |
|---|---|---|
| `threshold` | `1.0` | linear-HDR brightness a pixel must exceed to bloom; ~`1.0` blooms only post-exposure highlights, not the lit scene below `1.0` |
| `knee` | `0.5` | soft-knee half-width below the threshold — the glow fades in instead of popping on; `0` = a hard cutoff |
| `intensity` | `0.6` | how strongly the blurred glow is added back (`hdr += bloom · intensity`) |
| `radius` | `1.0` | blur-spread multiplier (scales the Gaussian tap spacing; `1.0` = the base 9-tap radius) |

Every bloom key must be `>= 0`; a negative value is a named problem (a negative `threshold`
blooms the whole frame, a negative `intensity` subtracts light).

**`composite`** — `from` (required): the surface whose colour is drawn. (Marker only — see the
[roster note](#passes--the-roster).)

**`scene`** and **`sky`** take no params.

---

## rate

`stages.<source>.rate` (or a `surface` node's `rate` prop — same parser, same spellings) is
how often a surface re-renders. All four are authorable and live:

| `rate` | Behaviour |
|---|---|
| `"live"` (default) | re-render every frame |
| `"poster"` | never re-render; keep the last image and keep compositing it (a still turntable costs nothing) |
| `"dirty"` | re-render when the content says it changed |
| `{ "hz": N }` | re-render at most `N` times a second (`N` finite and `> 0`; a non-positive `hz` is a problem) |

`live` / `live_bind` on a `surface` node is boolean sugar for Live vs Poster when you author
no `rate`.

---

## Lighting rigs

`stages.<source>.lighting` names a preset in the library. A preset is `stages.lighting.<name>`
in `resources/ui_stages.json`, and it compiles two ways into the **same** light list.

### The legacy trio

Three named slots, always emitted in this order, black ones included — a preset written
before the list existed still reads identically:

```jsonc
"studio": {
  "sun_dir": [0.4, 0.8, 0.5], "sun": [0.85, 0.85, 0.85],
  "moon_dir": [-0.5, 0.3, -0.4], "moon": [0.2, 0.22, 0.3],
  "ambient": [0.35, 0.35, 0.35]
}
```

| Key | Slot | |
|---|---|---|
| `sun_dir` / `sun` | 0 | directional sun: direction (normalized) / colour |
| `moon_dir` / `moon` | 1 | directional moon |
| `point_pos` / `point` | 2 | a point light at a world position (a star at the origin) |
| `ambient` | — | flat fill |
| `sky_zenith` / `sky_horizon` | — | the sky pass's palette |

### The general list

`lights: [ … ]` — up to `8` (`MAX_LIGHTS`) lights of mixed kind, the modern form. It
**replaces** the roster (authoring the legacy keys *and* a `lights` array is a problem — the
array wins). `ambient` and the sky palette still apply.

```jsonc
"hearth": {
  "lights": [
    { "kind": "dir", "dir": [0,-1,0], "color": [0,0,0] },   // slot 0 — sun  (reserved)
    { "kind": "dir", "dir": [0,-1,0], "color": [0,0,0] },   // slot 1 — moon (reserved)
    { "kind": "point", "pos": [388,120,404], "color": [1,0.55,0.18],
      "intensity": 36.0, "radius": 16.0,
      "driver": { "kind": "flicker", "speed": 7.0, "depth": 0.35, "seed": 1 } }
  ]
}
```

Each light carries only the keys its `kind` reads; a key that is spelled right but
inapplicable to the kind (`radius`/`cone`/`pos` on a `dir`, `cone` on a `point`) is a problem,
because the shader would accept and then discard it. A kind missing a required field is
dropped with a problem.

| `kind` | Required | Also reads | |
|---|---|---|---|
| `dir` | `dir` | `color`, `intensity`, `driver` | parallel light (sun/moon); no falloff |
| `point` | `pos` | `color`, `intensity`, `radius`, `driver` | omni light at a position |
| `spot` | `pos`, `dir`, `cone` | `color`, `intensity`, `radius`, `driver` | cone from `pos` along `dir` |

- `color` is the **hue** (linear RGB); the magnitude rides `intensity` (default `1.0`).
- `radius` (default `0.0` = **no falloff at all**). A `radius > 0` enables real windowed
  inverse-square falloff — the one place the colour-carries-magnitude convention stops, so a
  light with a radius wants `intensity` in the tens.
- `cone` is `[inner, outer]` in **degrees** (spot only).

### The one thing to know about slots: 0 and 1 are the sky

`lights[0]` is the sun and `lights[1]` is the moon in **one** addressing scheme: the `sky`
pass reads exactly those two, and a scene running a day/night cycle overwrites them **by
index** every frame. So a **non-`dir`** light parked in slot 0 or 1 is a problem (the cycle
would eat it) — **put fixed lights at slot 2+.** When a room is driven by a cycle, slots 0/1
are reserved black `dir` placeholders (as `hearth` shows), and the sky/ambient/fog belong to
the cycle, not the preset — a number for them there would be dead data.

### Drivers

A light may carry a `driver` that modulates its intensity over the scene's clock, evaluated
once per stage per frame, deterministic from its seed:

| `kind` | Effect | |
|---|---|---|
| `flicker` | seeded value noise that only ever **dims** | a fire, a failing lamp |
| `pulse` | a sine that brightens and dims | a beacon, a heartbeat |

| Driver key | Default | Meaning |
|---|---|---|
| `speed` | `1.0` | noise samples (flicker) / cycles (pulse) per second |
| `depth` | `0.0` | how deep it cuts; **`0.0` = bit-exact no-op** (undriven) |
| `seed` | `0` | integer; seeds the noise/phase so two lamps differ — must be a whole `0..=4294967295` |

---

## Binds

Numbers a **simulation** owns are not authored — a `*_bind` names the key the scene publishes
each frame, and the bind **replaces** the field it is the twin of (no multiply, no offset):

```jsonc
{ "pass": "ground_fog", "floor_bind": "world_floor", "density_bind": "fog_density" }
```

- A bind key is a plain published name — never a `$token`.
- **Authoring a number AND binding the same slot is a problem** (`"density": 1.0` beside
  `"density_bind": …`) — the number would be dead data, so the compiler refuses it.
- A bind whose key **no scene publishes** leaves the pass's **default** standing — **and there
  is no general gate that catches a typo'd bind key.** `"density_bind": "fog_denstiy"` compiles
  clean and renders the default density forever, in silence. See [Sharp edges](#sharp-edges).

---

## Worked example: Solar Birth

`scenes/solarbirth.scene.json` — a full-window HDR cinematic: a deep-space sky, the
behaviour's orbiting bodies, a volumetric dust cloud over them, and an ACES tonemap with a
faint warm grade. **This is the pattern for any lit-3D stage that wants HDR:** declare an
`hdr` attachment, point the colour passes' `writes` at it, and end with a `tonemap_grade`.

```jsonc
"solarbirth_sky": {
  "lighting": "deep_space",
  "clear": [0.006, 0.008, 0.014, 1.0],
  "attachments": {
    "hdr":   { "format": "rgba16f" },
    "color": { "format": "surface" },
    "depth": { "format": "depth32" }
  },
  "passes": [
    { "pass": "sky",   "writes": ["hdr"] },
    { "pass": "scene", "writes": ["hdr", "depth"] },
    { "pass": "volumetric_disk", "reads": ["depth"], "writes": ["hdr"],
      "inner": 0.4, "outer": 21.7, "snow_line": 4.6, "scale_height": 0.1, "density": 3.5,
      "tint": [0.038, 0.033, 0.052, 1.0], "glow": [0.85, 0.44, 0.22, 1.0],
      "formation_bind": "dust_formation", "time_bind": "dust_time" },
    { "pass": "tonemap_grade", "grade": [1.06, 1.0, 0.92, 1.0], "grade_strength": 0.12 }
  ]
}
```

What to read from it:

- **The colour passes write `hdr`, not `color`.** The tonemap reads `hdr` and writes `color`,
  so its read of `hdr` is what orders it *last* — no pass numbers.
- **On an HDR stage, `clear` is a linear working-space value.** It is written into `hdr` and
  passes through the ACES curve like everything else, so the same numbers read *darker* on
  screen than on a pre-HDR path. Tune the backdrop here, not in the tonemap.
- **The two simulation-owned numbers are bound, never authored.** `dust_formation` and
  `dust_time` are published by the behaviour each frame; the disk's defaults show before the
  scene publishes anything.

---

## Worked example: a room with a sun shadow

`scenes/pocclusters.scene.json` — the Prism Test Room. Two stages cooperate: a **producer**
that renders the world's casters from the sun's view into a depth map, and the **consumer**
world stage that samples it. The rig is the `hearth` preset above (a real point-light fire
with a flicker driver at slot 2, sun/moon reserved at 0/1).

**Producer** (`pocclusters_sun_shadow`) — throttled, and it authors only the caster box:

```jsonc
"pocclusters_sun_shadow": {
  "lighting": "hearth",
  "rate": { "hz": 20 },
  "passes": [
    { "pass": "shadow_map", "extent": 640 },   // producer: no `from`; writes depth
    { "pass": "scene" }                          // draws the casters into that depth
  ]
}
```

**Consumer** (`pocclusters_world`, the root world stage) — names the producer and owns the
sampling knobs:

```jsonc
"passes": [
  { "pass": "sky",   "writes": ["hdr"] },
  { "pass": "shadow_map", "light": 0, "from": "pocclusters_sun_shadow", "bias": 0.0015 },
  { "pass": "scene", "writes": ["hdr", "depth"] },
  { "pass": "water_surface", "reads": ["depth"], "writes": ["hdr"],
    "sea_level": 120, "spec_shininess": 220, "spec_strength": 3.0, "env_strength": 1.0,
    "wave_sources": [ /* 3 radial around the island + 2 directional open-ocean swells */ ],
    "time_bind": "fog_time" },
  { "pass": "ground_fog", "reads": ["depth"], "writes": ["hdr"], /* … */ },
  { "pass": "bloom", "reads": ["hdr"], "writes": ["hdr"], "threshold": 1.0, "intensity": 0.6 },
  { "pass": "tonemap_grade", "grade": [1.18, 0.92, 0.68, 1.0],
    "grade_strength_bind": "grade_warmth" }
]
```

This is the full shipped HDR recipe. It **executes** `sky → shadow_map → scene →
water_surface → ground_fog → bloom → tonemap_grade`, but read where that order comes from,
because only some of it is derived:

- **Hard-derived (reads/writes edges):** `water_surface` and `ground_fog` read `depth` that
  `scene` writes, so both run after `scene`; `bloom` reads `hdr` that sky/scene/water/fog all
  write, so it runs after every one of them; `tonemap_grade` reads `hdr` that `bloom` writes,
  so it resolves dead last. None of that is authorable position — it falls out of the arrays.
- **Declaration order (the tie-break):** the consumer `shadow_map` reads and writes **nothing**
  (it only binds the producer's depth), so its slot is wherever you list it. And `water_surface`
  vs `ground_fog` is a tie — `ground_fog` reads `depth`, **not** the water's `hdr`, so nothing
  derives water-under-fog; it holds only because water is listed first. **Author the fog before
  the water and the water composites on top of it, with no error** (they draw right through
  each other at the horizon).

What else to read from it:

- **`light`/`bias` live on the consumer only; `extent` on the producer only.** The consumer's
  `shadow_map` line is the single authority for the two sampling knobs the runtime reads; the
  producer authors only the box it fits its depth render to. Do not duplicate either.
- **The producer runs first with no authored edge.** It is a separate offscreen surface the
  root consumes; the frame graph renders it before the root because the consumer depends on it.
- **What you cannot author, and no warning tells you:** the sun-view matrix and the
  producer↔consumer depth handoff are wired by the scene's *Rust behaviour*. A `shadow_map`
  pass you drop into a scene whose behaviour does not wire them compiles clean, passes the
  structural gates, and casts **no shadow**. Shadows are not yet a data-only feature; today
  `pocclusters` is the only scene wired for them.
- **Shadow-map resolution is not authorable.** `extent` sets the world area the box covers
  (data), but the depth map's pixel size is a Rust constant in the scene crate
  (`SHADOW_SIZE = 2048`, `flicker-pocclusters/src/lib.rs`). Sharpness is `SHADOW_SIZE /
  (2·extent)` texels per world unit — grow the world (or the extent) and the shadow softens
  with nothing in the JSON to compensate.

---

## Water

`water_surface` is a **real animated water mesh** — a wave-displaced triangle grid drawn as
depth-writing geometry, not a screen-space plane. It renders entirely from data (the engine
uploads the grid on the first watered frame; nothing in the scene's Rust needs wiring), and
`pocclusters` ships it — the sea flooding the island dome. Author a `water_surface` pass and
you get:

- **An environment-lit water body.** The grid is a *projected* grid — read as screen space and
  cast onto `y = sea_level`, so it always tiles the visible sea out to the horizon at a fixed
  vertex count, with no field box to author. The fragment shader lerps `shallow → deep` by view
  angle (grazing = `shallow`, straight-down = `deep`), sharpened by `shore_fade`, **multiplies
  that by the rig's ambient + the sun's diffuse term**, and **Fresnel-blends the live sky**
  (`sky_zenith`/`sky_horizon` mirrored along the reflected view ray, dialled by `env_strength`)
  over the top — so the reflection strengthens toward the horizon and the whole sea tracks the
  time of day for free.
- **Real geometry.** It writes and tests depth, so it **occludes and is occluded** by the
  terrain — terrain above `sea_level` stays dry, the rest floods — and composites
  translucently over the lit scene. It writes `hdr`, so its bright glint survives the tonemap
  (and blooms, if a `bloom` pass follows).
- **Waves, both kinds.** Up to six `wave_sources` sum across the surface in one loop — radial
  ripples around a `center`, and directional plane waves along a `dir` (the open-ocean swell).
  The vertex shader lifts every grid vertex and recomputes its normal analytically from the
  summed slopes; `normal_scale` exaggerates that shading slope (glint choppiness) without
  changing the geometry.

**Two things the shader depends on that no error will warn you about:**

- **The glint is the two sky slots, 0 and 1.** `spec_shininess`/`spec_strength` size one
  specular lobe run over `scene.lights[0]` (the sun) and `scene.lights[1]` (the moon) and
  summed — the moon's is naturally subtler because its radiance is. On a stage whose slots are
  **dark reserved `dir` placeholders** (a cycle-driven room like `pocclusters`, where the
  Celestial Cycle writes the real sun and moon into slots 0/1 every frame), the glint only
  appears once that cycle runs — a stage with black, undriven sky slots renders flat,
  glint-less water with no diagnostic.
- **The waves are frozen without a live `time_bind`.** `time` defaults to `0` and there is no
  automatic clock; the ripples only move while a `time_bind` names a key the scene publishes
  each frame (`pocclusters` reuses its one `fog_time`). A missing — or **typo'd** — `time_bind`
  compiles clean and renders motionless waves, the [silent-bind trap](#binds) in miniature.

The grid density (`WATER_GRID_N = 256`) is a Rust constant: because the grid is screen-space,
a `wavelength` very short relative to the *visible* sea aliases against it, with no JSON knob
to compensate — `wave_falloff` is the mitigation, flattening the far field before it shimmers.

---

## Bloom

`bloom` is an HDR post-effect: it makes the genuinely bright parts of the linear `hdr` image
**glow** — the sun glint on the water, the sun disc in the sky. It reads `hdr`, extracts
everything above `threshold` (with a soft `knee` shoulder) into a half-resolution buffer,
blurs it (a separable Gaussian whose spread scales with `radius`), and **adds** the result
back into `hdr` scaled by `intensity`. It renders entirely from data — no binds, no wiring.

Because it reads `hdr` it derives **after every pass that writes `hdr`** (sky, scene, water,
fog), and because it writes `hdr` the `tonemap_grade` that reads `hdr` derives **after it** —
so bloom always lands in the one correct slot, immediately before the resolve, with no
authored order. It is **HDR-only**: a `bloom` on a stage with no `rgba16f` attachment has no
HDR colour to glow and is a loud problem. Tune `threshold` around `1.0` so only post-exposure
highlights bloom and the lit scene (below `1.0`) does not wash out.

The half-res scratch and the 9-tap Gaussian are fixed in the renderer; `radius` widens the
halo but the tap count does not change.

---

## How to extend

- **Add a stage to a scene** — put a source under the scene file's top-level `stages`, name it
  from a `surface` node's `source`. If it is lit, name a `lighting` preset. Ship it and the
  `every_shipped_stage_compiles_clean` gate compiles it.
- **Add a lighting preset** — add it under `stages.lighting` in `resources/ui_stages.json`
  (presets are library-only). Use the [general list form](#the-general-list) for anything
  with falloff or a driver.
- **Make a stage HDR** — add an `hdr` (rgba16f) attachment, repoint your colour passes'
  `writes` to `hdr`, append a `tonemap_grade`. The `deep_space`/Solar Birth pattern.
- **Add a content layer** — a `layers[]` entry with a `draw` kind. The catalog and its keys:

  | `draw` | Keys (defaults) |
  |---|---|
  | `skinned` | — (the posed character) |
  | `ring` | `radius` (0.45) · `y` (0) · `segments` (24) · `color` · `color_active` |
  | `grid` | `spacing` (0.5) · `extent` (6.0) · `y` (0) · `color` |
  | `shells` | — (the scene's own shell stack) |
  | `shell` | `radius_scale` (1.0) · `inset` (0.0) · `color` |
  | `graticule` | `radius_scale` (1.0) |
  | `material` | — (the lit material sample) |

  Remember a layer only draws in a behaviour that supports its kind (it warns otherwise).
- **A new pass kind or draw kind is engine work**, not authoring — a new arm in the compiler
  and a typed value in `flicker-render`. You cannot add one from JSON.

---

## The gates

Every stage you ship is compiled by build gates in `flicker-widgets` (`stages.rs` tests).
Author against them:

| Gate | Holds |
|---|---|
| `every_shipped_stage_compiles_clean` | every library preset + every scene's stages compile with **zero** problems |
| `every_surface_source_in_a_shipped_scene_resolves` | every `surface` node's `source` names a real stage |
| `a_stage_in_the_shared_library_is_shared` | a library **source** is named by ≥2 scenes |
| `a_scenes_stages_merge_into_the_shared_block_and_never_shadow_the_library` | your stage never reuses a library name |
| `every_lit3d_shipped_stage_resolves_through_exactly_one_tonemap` | an HDR surface resolves once |
| `the_shadow_map_pass_compiles_its_roles_and_names_its_misuse` | producer/consumer roles + their misuse problems |
| `the_water_surface_pass_compiles_and_names_its_misuse` | water params, the wave roster + their misuse problems |
| `the_bloom_pass_compiles_and_names_its_misuse` | bloom knobs, HDR-only, and every out-of-range value named |
| `every_shipped_preset_compiles_to_a_rig_with_no_problems` | every `lighting` preset is clean |
| `the_general_lights_form_compiles_and_every_unknown_is_a_problem` | the `lights[]` vocabulary |
| `no_shipped_stage_pairs_a_skinned_layer_with_a_non_directional_light` | a posed character keeps a directional key light |

The compiler reports the same words at runtime as a `tracing::warn!`. A bad value still
degrades to its default.

---

## Sharp edges

- **Two of nine pass kinds are markers, not effects.** `composite` and `shadow_map` render
  nothing on their own — the scene's Rust behaviour wires their runtime. A content author
  cannot make either work from JSON; today only `pocclusters` wires a shadow, and no scene
  wires a recipe `composite`. See the [roster note](#passes--the-roster).
- **Water's sun glint needs a lit slot-0 sun.** The specular reads light slot 0's radiance
  (`spec_shininess`/`spec_strength`), so on a stage whose slot 0 is a dark reserved `dir`
  placeholder with no day/night cycle driving it, the water renders flat and glint-less —
  no warning, no gate. See [Water](#water).
- **Water waves are frozen without a live `time_bind`.** `time` defaults to `0`; a missing or
  typo'd `time_bind` compiles clean and renders motionless waves (the [silent-bind trap](#binds)).
- **Water-under-fog is declaration order, not derived.** `ground_fog` reads `depth`, not the
  water's `hdr`, so nothing orders the fog after the water — list the water first, or it
  composites on top of the fog at the horizon.
- **`bloom` is HDR-only.** A `bloom` on a stage with no `rgba16f` attachment has no HDR colour
  to extract and is a loud problem; its knobs must all be `>= 0`.
- **A typo'd bind key fails to silence.** `*_bind` names a published key; a name nothing
  publishes leaves the authored default with **no warning and no gate**. Copy a working bind
  key rather than typing one, and give a new bind a per-scene gate that asserts the inputs fn
  publishes it.
- **`scale` is live only on `color`.** The surface is sized off `color`'s scale alone. On
  `hdr` a non-`1.0` scale is a loud problem; on `depth` (or any other attachment) it is
  parsed, stored, and **silently ignored** — the per-attachment `scale` the schema advertises
  is really a per-surface scale only `color` expresses.
- **`clear` on an HDR stage is linear**, tonemapped like everything else — the same numbers
  read darker than on a non-HDR path.
- **`clear` on a root surface is the window clear**; absent, it leaves whatever the app set.
  On an offscreen surface, absent `clear` = transparent, so the node's own panel is the
  backdrop.
- **A bound field must not also be authored** — a number beside its `*_bind` is dead data and
  a problem.
- **Slots 0/1 are the sun and moon by index.** In a general `lights[]` rig, put fixed lights
  at slot 2+; a non-`dir` light in slot 0/1 is a problem, and under a day/night cycle those
  two slots are overwritten every frame.
- **A `radius` of 0 means no falloff**, not a zero-size light. A point light you expect to
  pool needs a `radius > 0` and an `intensity` in the tens.
- **Layers are filler-specific.** A `draw` kind a behaviour does not know is warned at load
  and drawn as nothing — the layer catalog is the engine's whole vocabulary, not any one
  scene's.
