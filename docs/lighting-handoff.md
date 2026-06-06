# Handoff — Lighting, fog & color grading (day/night cycle)

> Standalone handoff for a fresh session. **Re-verify every anchor** (file
> paths, the exact shader line, uniform bindings, symbol names — they drift).
> Builds on: the **Lua UI framework** (`docs/ui.md` — the slider/value channel
> is how the controls plug in) and **`flicker-render`** (the 3D mesh pipeline +
> shader). Lighting is **gated behind the LOD / terrain rework**; the *UI* side
> is ready now. No new art assets are required for any of this.

## Status — Slice 1 landed (2026-06-06)

**Done: the `Scene` uniform foundation + sun/moon directional lighting +
time-of-day, the three cycle sliders, and the material recolor.** The lighting
is no longer hardcoded; scrubbing the sliders drives the scene live. Decisions
that were open are now locked (below). Next slices: **fog + colour grade**
(Part 2, fields already in the uniform, inert at 0), then **eclipse + "alien"
mood events** (Part 3).

**Locked decisions** (replacing the "Confirm first" list):
- **Forward shading, one `Scene` uniform** (lighting + reserved fog/grade) —
  no post-process pass. Lighting-only, **no sun/moon disc** (no art).
- **Three manual sliders, lower-right, two rows** (`UI.hud.lighting`): row 1 =
  **Sun** (1 unit, `0–24h`, 12 ruler marks @ 2h) + **Moon** (2 units, `0–4` wk,
  4 marks); row 2 = **Year** (full 3-unit span, `0–12` mo, 12 marks). Wide
  tracks + absolute-drag for touch. No auto-advance yet (manual scrub).
- **Cycle model** (`compute_scene`, example side): sun arc (sunrise E 06:00 →
  overhead 12:00 → sunset W 18:00, fades below horizon); moon = phase offset
  from the sun (new moon ≈ aligned/unlit at 0/4 wk, full moon opposite/brightest
  at 2 wk); year tilts the arc north/south by season. Eclipse will fall out of
  `dot(sun_dir, moon_dir)` gated by the year (Part 3).
- **Material:** added palette **index 10 = STONE** (matte neutral `~0.45`) in
  `mesh.wgsl`; the field now builds `Material::new(10, 10, 0)` (was `1,1,0` =
  navy). **The on-disk `bake/` was regenerated** (`--bake`) so the saved source
  carries index 10 — the old navy bake is backed up at
  `examples/voxel-cluster/bake.navy.bak`. **Gotcha:** the bake stores material
  and `ensure_source` loads it verbatim (bypassing `contour`), so any future
  material change needs a re-bake (or delete `bake/`) to take effect.

**New anchors (this session) — verify before building on them:**
- Engine: `Renderer::set_scene(SceneLighting)` (`renderer.rs`), the public
  `SceneLighting` struct (`mesh.rs`, re-exported from `lib.rs`), the GPU-side
  `SceneUniform` + `set_scene_uniform` + bind-group **binding 2** (`pipeline_mesh.rs`),
  the `Scene` struct + sun/moon/ambient fragment in `shaders/mesh.wgsl`, and the
  `mesh_pipeline_compiles_shader` test that validates the WGSL.
- Example: `compute_scene(time_of_day, moon_phase, year_month)` + the three
  `GameScene` fields, published via `hud_model` and read back in `update`
  (`examples/voxel-cluster/src/main.rs`).
- UI: `UI.hud.lighting` (`ui_elements.json`), the `lighting`/`lighting_rects`
  functions in `scripts/hud.lua`, and the optional `ticks` ruler arg on
  `Widgets.slider_draw` (`scripts/widgets.lua`).

### Follow-on landed same session — procedural sky + slider sizing

- **Procedural "scattering" sky** (the user's "option 3 faked by option 2"): a
  fullscreen pass (`shaders/sky.wgsl` + `pipeline_sky.rs`) drawn **first** in
  the main pass, before the mesh. It reconstructs the world view ray per pixel
  from the inverse view-projection and shades a horizon→zenith gradient + a
  faked sun glow (wide Rayleigh wash + tight Mie core, tinted by the cycle's
  `sun_color`, so it warms at dawn/dusk and dies at night) + a cooler moon glow.
  Driven by the **same `SceneLighting`** as the mesh — added `sky_zenith` /
  `sky_horizon` palette fields (set per time-of-day in `compute_scene`). New
  API: **`Renderer::draw_sky()`** (per-frame, like `draw_mesh`; no-op without a
  camera, so menus/loading keep their flat `clear_color`). `GameScene::render`
  calls it each active frame. Validated by `sky_pipeline_compiles_shader`.
- **Cycle sliders doubled** (`UI.hud.lighting`): `unit` 96→192, `track_h`
  16→32, `handle_w` 12→24, text bumped — ruler ticks/handle scale off the track.
- **Celestial-path overlay** ("The Advent" alignment viz): a `"celestial_paths"`
  HUD checkbox draws the sun's and moon's full orbital arcs as wireframe rings
  (`celestial_arc` + `draw_lines`, depth-tested) plus a `cross_marker` at each
  body's current position. The moon ring converges onto the sun's as the phase
  nears new (0/4 wk) = eclipse alignment. Subsumes the separate "light-source
  markers" idea below. The sun/moon geometry was refactored into shared
  `season_tilt` / `sun_direction` / `moon_direction` helpers so the drawn arc
  and the actual light are one source of truth.

- **Procedural sun + moon discs + eclipse** (all in `sky.wgsl`, no asset): a
  flat sun disc tinted by `sun_color` (gone at night) and a moon disc with an
  **analytic phase terminator** — the visible sphere point's outward normal lit
  by the sun, so the moon waxes/wanes as the phase slider moves. The moon
  (`moon_r = 0.047`) is a hair larger than the sun (`sun_r = 0.038`) — matched
  to the sun's bloomed *apparent* body (the bloom is kept tight via
  `pow(sd, 1600)` so the disc doesn't balloon) and still large enough to swallow
  the sun at totality. The shadow limb always washes into the local sky (day *or*
  night, opaque only where it covers the sun) so the unlit side never reads as a
  hard mismatched disc; and the **whole moon fades by daylight**
  (`day_hide = smoothstep(0,0.25,sun.y)·(1−eclipse)`) so it's a night body —
  faint by day, full at night, held solid only while eclipsing. The moon is
  composited last so it **eclipses the sun** when aligned. **Emergent "right
  time of year":** the season tilt offsets the moon's arc from the sun's except
  near the equinoxes, so a *total* eclipse needs `year ≈ 3 or 9` **and**
  `moon ≈ 0/4` (new) with the sun up — the Advent recipe, for free.

- **The Advent world-reaction** (the eclipse darkens the *ground*, not just the
  sky): both `compute_scene` (example) and `sky.wgsl` compute an `eclipse`
  factor from **disc overlap** — `coverage = 1 − smoothstep(moon_r−sun_r,
  moon_r+sun_r, separation)`, gated on the sun being up — using the **shared
  disc radii** (`SUN_DISC_R` / `MOON_DISC_R` in `main.rs`, mirrored in the
  shader). As coverage rises: the direct sun on the terrain is killed
  (`sun_color *= 1 − eclipse`), and ambient + sky sink into a **dim desaturated
  blood-shadow** (lerps toward `~(0.07, 0.02, 0.03)`) — cool/dark/mystical, not
  a bold red. The corona uses the same `coverage` with a fixed white base so it
  blazes against the darkened sky. So at a true Advent the whole world goes
  dark together. Full LUT/post grade is still the parked path; this is the
  forward, asset-free version.

- **Time simulation (auto-advance)**: a fourth cycle-panel row — a **Speed**
  slider in **sim-minutes per real-second**, `0 = paused` (`UI.hud.lighting.speed`,
  `GameScene.sim_speed`). When > 0, `update` advances `time_of_day` and drifts
  `moon_phase` (28-day month) + `year_month` (360-day year) at their natural
  cadence via `rem_euclid`, so the sky evolves and eclipses recur on their own —
  for running/recording the motion. Manual scrub still wins the frame it's
  dragged. Distinct widget id `cycle_speed` (vs the move-speed slider's `speed`).

- **Distance fog** (forward — the last Part 2 piece, **done**): in `mesh.wgsl`,
  `mix(lit, fog_color, 1 − exp(−density·dist))` with `dist = length(world_pos −
  camera_pos)`. `compute_scene` sets `fog_color = sky_horizon` (so the haze
  melts into the sky and reacts to time-of-day **and** the eclipse) and density
  from a **Fog** slider (fourth cycle-panel row, `UI.hud.lighting.fog`,
  `GameScene.fog`, `0..1`) × `0.0020` × a "thicker when the sun is low" curve.
  Fog lives only in the 3D mesh pass, so the HUD/2D stays crisp (invariant #2).

**With fog in, Parts 1 + 2 are complete.** Remaining is optional / separate:
- **Full LUT/post colour grade** — the parked richer path (offscreen target +
  fullscreen pass); the forward eclipse darkening already covers the Advent.
- **"Alien" mood events** — low-strength desaturated psychedelic grade presets
  the cycle blends toward (Part 3 flavour). *(User has accounting in place to
  spin these off separately — not in this track.)*
- **Scene selector** (orthogonal, its own track): `--bake <name>` →
  `bake/<name>/…`, a `scenes` dropdown, and a source-reload path
  (`source` swap + `generation` bump + `submit_field_jobs`).

The sections below are the **original** plan/contract; Parts 1 & 2 (sun/moon
lighting + time-of-day, the procedural sky, the Advent eclipse + world-reaction,
and distance fog) are **implemented**; the richer grade / alien events / scene
selector remain as separate tracks.

## Destination

A **day/night cycle** drives **sun + moon directional lights** over the matte
voxel terrain, with **eclipse** effects when the moon aligns with the sun; plus
**distance fog** (semi-predictable, e.g. morning fog) and **time-of-day color
grading**. Everything is live-controllable through UI **sliders** (time-of-day,
moon alignment, fog) on the widget framework we just built. The terrain has **no
UV map** — surfaces are **matte/diffuse only** (material colour × shading), so
"lighting" here means *light direction + colours + fog + grade*, nothing
texture-mapped.

Two parts (the user's split):

1. **Sun/moon directional lighting + time-of-day** (+ moon alignment / eclipse).
2. **Fog + color grading**, with the light source(s) **on cycles** (one
   `time_of_day` value animates the whole scene).

## Why this is a small lift — current state (verified this session)

- **The lighting already exists, hardcoded.** The mesh fragment shader does
  matte Lambertian directional shading with per-vertex **world-space normals**
  (`MeshVertex { position, normal, material }`, `crates/flicker-render/src/mesh.rs`).
  The "sun" is a constant in `crates/flicker-render/src/shaders/mesh.wgsl`:
  ```wgsl
  let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));
  let lambert   = max(dot(in.world_normal, light_dir), 0.0);
  let shaded    = base * (0.3 + 0.7 * lambert);   // 0.3 ambient + 0.7 diffuse
  return vec4<f32>(shaded, 1.0) * per_draw.tint;
  ```
  So "move the sun/moon" = **promote that constant to a uniform** and drive it.
  Not a new lighting system.
- **No UVs / textures** on terrain — `base` comes from the packed `material`
  index via a palette (`material_index_color` in the shader). Matte only.
- **Uniforms (mesh pipeline, `pipeline_mesh.rs`):** group 0 has
  **binding 0 = `Camera`** (`view_projection`; **shared with the lines pipeline**
  via `MeshPipeline::camera_buffer()`) and **binding 1 = `PerDraw`**
  (`model` / `tint` / `flags`, dynamic offset per draw). A lighting uniform
  should be a **new** binding (group 0 binding 2), *not* a reshape of `Camera`.
- **No post-process pass.** `Renderer::end_frame` encodes a single pass:
  mesh → lines → billboard → 2D, straight to the surface, clearing to
  `Renderer.clear_color`. So fog/grading as a **post** pass is *new machinery*;
  doing it **forward** (in the mesh fragment shader) needs none.
- **Billboard pipeline exists** (`Renderer::draw_billboard`) — an optional
  *visible* sun/moon disc can be a procedural tinted billboard (still no asset).
- **UI is ready.** The Lua widget framework (`Widgets.slider`, the
  Model-in/results-out value channel, `ui_elements.json`; see `docs/ui.md`) means
  a **time-of-day slider** is a few lines of JSON + one `Model` value in + one
  result out — identical to the existing `UI.hud.controls` move-speed slider in
  `scripts/hud.lua` (`GameScene` reads the result and applies it).

## Part 1 — Sun/moon directional lighting (time-of-day)

**Shader (`mesh.wgsl`).** Replace the constant `light_dir` with uniform values;
add a second (moon) term; add ambient + per-light colours:
```wgsl
let sun  = scene.sun_color  * max(dot(in.world_normal, scene.sun_dir),  0.0);
let moon = scene.moon_color * max(dot(in.world_normal, scene.moon_dir), 0.0);
let shaded = base * (scene.ambient + sun + moon);
```

**Uniform.** Add a frame-global **`Scene`** uniform (new bind-group entry, e.g.
group 0 binding 2 — keep `Camera` untouched since the lines pipeline shares it).
One uniform can carry Part 1 *and* Part 2:
```wgsl
struct Scene {
    sun_dir: vec3<f32>,   sun_color: vec3<f32>,
    moon_dir: vec3<f32>,  moon_color: vec3<f32>,
    ambient: vec3<f32>,
    camera_pos: vec3<f32>,                 // for fog distance (Part 2)
    fog_color: vec3<f32>,  fog_density: f32,   // Part 2
    grade: vec3<f32>,      grade_strength: f32, // Part 2
};
```
(Mind WGSL std140/std430 alignment — pad `vec3`s; mirror with a `#[repr(C)]`
CPU struct like `CameraUniform`/`PerDraw` in `pipeline_mesh.rs`.)

**Renderer API.** `Renderer::set_lighting(...)` (or `set_scene(...)`) writes the
uniform each frame, mirroring `set_camera` (cache it, upload in `end_frame`).

**Time-of-day → directions (example side, `voxel-cluster`).** From a
`time_of_day` slider (0..24h): sun elevation/azimuth around the day arc; below
the horizon → fade `sun_color` to ~0 (night). Moon direction = an offset from the
sun by the **alignment / phase** control. Sun colour warm at dawn/dusk, white at
noon; moon cool/blue and dim.

**Eclipse.** When `dot(sun_dir, moon_dir)` crosses a threshold (the two align),
blend toward an eclipse look (drop sun contribution + shift `grade`/`ambient`
cool-dark). Falls straight out of the two vectors — the "moon season aligns with
sun" the user described.

**UI.** A `time_of_day` **slider** + a `moon_alignment` **slider** (or a
**dropdown** for discrete phases) — a new `UI.<screen>.lighting` section (or fold
into the in-game controls). The scene reads the result values and computes the
`Scene` uniform. (See `scripts/widgets.lua` + the `UI.hud.controls` wiring for
the exact pattern.)

## Part 2 — Fog + color grading

**Fog — recommended forward (no new pass).** Distance fog in the mesh fragment
shader: `final = mix(shaded, scene.fog_color, fog_factor)`, where
`fog_factor = 1 - exp(-scene.fog_density * dist)` and
`dist = length(in.world_position - scene.camera_pos)`. fog colour + density from
the `Scene` uniform.

**"Semi-predictable" fog (morning fog).** Drive `fog_density` from `time_of_day`
example-side: a curve that peaks near dawn (~05:00–07:00), clears through the
morning, low at midday, optionally rising at dusk — *plus* a manual fog slider as
a multiplier/override. So it reads as weather without being scripted per-frame.

**Color grading — forward, asset-free (the cheap version).** A global tone at the
end of the mesh fragment: shift/multiply by `scene.grade` (lerp by
`grade_strength`), with `grade` set warm/cool by `time_of_day`. Good enough for
the time-of-day *feel* with zero new passes/targets.

**Post-process pass — only if you want LUT/filmic grading or screen-space
effects.** Render the 3D scene to an **offscreen HDR target**, run a **fullscreen
pass** that applies fog (depth-based) + grade (LUT), present, **then** draw the
2D UI on top (ungraded). This is the bigger, *optional* path — call it out, don't
default to it. The forward path above covers the stated goal.

**The cycle.** One `time_of_day` (+ `moon_phase`) feeds sun/moon dirs + colours,
fog density, and grade — a single slider animates the whole scene. A "day length"
control (or auto-advance with a pause toggle) makes it loop for ambience and for
the future "load the menu background while the cycle plays" idea.

## Invariants — do not break

1. **Matte only.** No UVs/textures on terrain — lighting is diffuse direction +
   colour, never texture-mapped. (If UV-mapped ground arrives later it's
   additive, not assumed here.)
2. **UI/2D is never fogged or graded.** The 2D overlay draws *after* the 3D (the
   painter's-order layering wing). Keep fog/grade in the **3D path** (mesh shader,
   or a 3D-only post pass *before* the 2D draws) so the HUD/menus stay crisp.
3. **Don't reshape the `Camera` uniform** — it's shared with the lines pipeline
   (`MeshPipeline::camera_buffer()`). Add lighting as a separate uniform/binding.
4. **Controls go through the existing Lua value channel** (Model in / results
   out) — no new engine↔script seam. (`flicker-script` boundary contract.)
5. **Billboards & lines are depth-tested against the 3D scene** — if you add a
   visible sun/moon disc as a billboard, it already composites correctly.

## Orthogonal — recolor the field mesh (unrelated, flagged by the user)

The voxel field mesh is built with `Material::new(1, 1, 0)`
(`examples/voxel-cluster/src/main.rs` ~`:628` and ~`:2456`), which the shader
palette (`material_index_color`) maps to **index 1 = `DEEP_WATER` navy**. To give
the "navier-stokes field" a distinct colour, either point it at a different
material index, add a palette entry in `mesh.wgsl`, or recolour index 1. Trivial;
independent of the lighting work.

## Confirm first / open decisions

- **Forward fog/grade** (small, no pass) vs a **post-process pass** (LUT/filmic,
  new offscreen target) — recommend forward first; add the pass only if grading
  ambition grows.
- **One `Scene` uniform** (lighting + fog + grade) vs separate uniforms.
- **Visible sun/moon disc** (procedural billboard) or **lighting-only**.
- **Where the lighting UI lives** — a dedicated panel/screen vs the in-game HUD
  controls. (A pause-menu "World" tab is a natural home once settings port to Lua.)
- **Auto day/night cycle** vs slider-only (manual scrub).

## Effort (grounded)

- **Part 1** (sun/moon + time-of-day + eclipse): **small** — a `Scene` uniform +
  `set_lighting` API + ~10 lines of shader + example math + 1–2 sliders. The
  shading already exists; this is plumbing. ~half a day.
- **Part 2 forward fog + cheap grade**: **small** — a few `Scene` fields + shader
  + a fog slider + a time→density/grade curve. The **post-process LUT** path is
  **moderate** (offscreen target + fullscreen pass + reorder 2D after it).
- **Field recolor**: **trivial** (one constant / one palette entry).

## Pinned — parked / later

- Shadows, atmospheric scattering, volumetric (god-ray) fog, a real LUT grading
  pipeline — each is its own track, **not** in "move the lights + grade colours."
- A visible, textured sun/moon (vs a flat tinted billboard) would want an asset.
- If lighting/fog state should persist (a "world settings" save), it joins the
  `settings.json` discussion (currently display-only) — see `docs/ui-lua-handoff.md`.
