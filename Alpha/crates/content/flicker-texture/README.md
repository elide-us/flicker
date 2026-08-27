# flicker-texture

The procedural texture **synthesizer**: it turns a small saved *recipe* into a full
set of PBR surface maps, entirely on the CPU. Stage ③ of the material pipeline
(Elements → Compounds → **Materials**) — elements and compounds answer *what a
place is made of*, this answers *what that looks like*. The shape is a synthesizer
all the way down: **six noise channels → one mix bus → an output stage → seven
maps**. Every map is a projection of the same mixed field, so turning one knob moves
the whole surface coherently, the way a real material behaves. It has no graphics
and no I/O — it hands back raw pixel buffers and lets the caller decide whether they
become a GPU texture, a PNG in `staging/`, or a terrain lookup.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## Vocabulary (flicker / synth terms used below)

- **Recipe** ([`TextureRecipe`]) — the whole instrument's state and *the only thing
  that is saved*: a seed, six channels, and an output stage, a few hundred bytes of
  JSON. The megabytes of image are a rebuildable consequence, so the artifact is the
  recipe and the maps are output.
- **Channel / voice** ([`Channel`]) — one noise source with its own scale, octaves,
  warp and shaping, folded into the mix bus by a blend mode at an amount.
- **Rack** — the fixed array of **six** channels (`[Channel; CHANNEL_COUNT]`), folded
  **ordinally** (channel 1 into 2 into 3 …). Not a node graph — a strip of sliders you
  can see all of at once.
- **Mix bus / field** — the single scalar `h ∈ [0,1]` per texel that the rack folds
  down to. Everything a renderer needs is a projection of this one field.
- **Output stage** ([`OutputStage`]) — how the field projects into each map: colour
  through a ramp, relief from the gradient, roughness/metalness by modulation,
  occlusion from the neighbourhood, emission from a banded glow.
- **Bake** — evaluate a recipe at a resolution into a [`MapSet`] of RGBA8 [`Map`]s.
- **Seamless / tileable** — a swatch abuts itself with no ridge: the noise lattice
  wraps on the tile *and* the baker's neighbourhood reads wrap with it.
- **Deterministic** — `(recipe, size)` is the entire input. No clock, no globals, no
  filesystem; identical input always produces identical bytes, at any resolution, on
  any machine.
- **Salt vs seed** — `seed` is the recipe's one re-roll knob; a channel's `salt`
  selects an *independent* field under that seed, so two voices never correlate. (The
  same salt/seed split as [`flicker-primitive`](../flicker-primitive/README.md).)
- **Patch** — a factory recipe (`presets::*`) bound to a real material id: the
  instrument's starting library.
- **Roll** — [`random`]: generate a whole recipe from a seed in one pass.

## Where it sits

Content cluster (`Alpha/crates/content/`), a peer of `flicker-materials` and
`flicker-primitive`.

- **Builds on:**
  - [`flicker-primitive`](../flicker-primitive/README.md) — the one seeded noise in
    the tree. This crate samples its **2D tileable** face (`value2_tiled`, `fbm2`,
    `worley2_tiled`, `ridged`, `billow`, `contrast`, `Fbm`); the tiling is what makes
    a swatch seamless.
  - [`flicker-materials`](../flicker-materials/README.md) — supplies `MaterialId`
    (`= u8`, the 256-row material index) that a recipe optionally binds to. This crate
    never *classifies*; it only names the binding, so a bound recipe can answer
    terrain's "what does material 47 look like?".
  - `serde` — a recipe round-trips to/from JSON.
- **Used by:**
  - `flicker-sablework` (scenes cluster) — the **Sablework bench**, the interactive
    face on this crate: the six-voice console, the live worker-baked preview, the
    Hue/Saturation/emit knobs, and the commit that writes a map set + recipe to
    staging. It has no README of its own yet; this crate is its whole engine.
- **Reads from the content tree:** **nothing.** Every output is a pure function of
  its arguments — no file, theme, stringtable, or package inputs. (Factory patches
  carry `MaterialId`s, but the crate never opens the material index; a caller/test
  does, and where that index lives is the app's business.)

## Public API

Everything below is re-exported from the crate root unless a `module::` prefix is
shown.

### The recipe — the saved artifact

| Item | What it is | The one thing to know |
|---|---|---|
| `struct TextureRecipe` | `{ id, name, material: Option<MaterialId>, seed: u64, channels: [Channel; 6], out: OutputStage }` | This — not the images — is what gets saved. `id` is the file stem and lookup key. `material` is `#[serde(default)]`, so an older unbound recipe still loads. |
| `TextureRecipe::default()` | One audible voice on an otherwise silent rack | So a fresh bench previews a surface, not a black square. |
| `TextureRecipe::active_channels()` | `-> usize` | How many voices are enabled — the strip's summary readout. |

### The rack — channels, sources, blends

| Item | What it is | The one thing to know |
|---|---|---|
| `const CHANNEL_COUNT` | `= 6` | Fixed, not a `Vec`: the count is part of the layout, the serialized form, and the controller walk. Growing it is a deliberate three-place change. |
| `struct Channel` | one voice: `enabled, source, scale, octaves, lacunarity, gain, warp, salt, contrast, invert, blend, amount` | Ranges are the *console's*, not the maths' — see the field notes below. `Channel::default()` is silent and neutral (the base every edit starts from). |
| `Channel::eval(u, v, seed)` | `-> f64` in `[0,1]` | Evaluate this voice at tile position `(u,v) ∈ [0,1)`. Tile-relative, not pixel indices, so the field is resolution-independent. |
| `Channel::period()` | `-> i64` | The lattice period this voice tiles on: its `scale`, floored at one cell. |
| `enum NoiseKind` | `Value · Fbm · Ridged · Billow · Worley · Stripe` | What a channel generates before shaping. Every kind is tileable. `NoiseKind::ALL` is console order; `.id()` is the stable serialized/label string. Default = `Value`. |
| `enum BlendMode` | `Base · Add · Mul · Screen · Overlay · Min · Max · Diff` | How a voice folds into the bus. `Base` **replaces** (the bed); the rest combine. `BlendMode::ALL`, `.id()`. Default = `Base`. |
| `BlendMode::apply(bus, top, amount)` | `-> f64` | Fold `top` into `bus`, then cross-fade by `amount ∈ [0,1]`. Endpoints are **exact**: `amount = 0` is the bus untouched, `1` is the blend itself (no float crumb across six folds). |
| `fn mix(rack, u, v, seed)` | `-> f64` | Fold every *enabled* channel of the rack into one field value. Bus starts at **0**, so an empty rack is an honest black field. |

**Channel field notes (the console's units):**

| Field | Meaning |
|---|---|
| `scale: u32` | **Cells across the tile**, and therefore the lattice period — integer, because a fractional cell count cannot close a seam. Clamped to ≥ 1. |
| `octaves: u32` | Octaves summed. **Ignored by `Value` and `Worley`** (single-lattice). Clamped to `1..=12`. |
| `lacunarity`, `gain` | Per-octave frequency step / amplitude step (fBm sources only). |
| `warp: f64` | Domain-warp strength, **in cells**; `0` is unwarped. The warp field shares this voice's period, so warping never opens a seam. This is the slider that turns ruled `Stripe` lines into rock. |
| `salt: u64` | Selects an independent field under the recipe seed — turning it re-rolls only this voice. |
| `contrast: f64` | Midpoint contrast: `1` neutral, above hardens, below flattens. |
| `invert: bool` | Flip about the midpoint after shaping. |
| `amount: f64` | How much of the fold to take, `0..=1`. |

### The output stage — field → maps

| Item | What it is | The one thing to know |
|---|---|---|
| `struct OutputStage` | `ramp, relief, roughness, roughness_mod, metalness, metalness_mod, ao, emissive, emissive_strength, emissive_band` | One field projects into every map; keeping one field is what keeps maps from drifting into a surface that cannot exist. |
| `OutputStage::roughness_at(h)` | `-> f32` | Roughness at field value `h`: `roughness ± roughness_mod` about the midpoint. Negative `roughness_mod` makes crests the *shiny* part. Safe to call standalone. |
| `OutputStage::metalness_at(h)` | `-> f32` | Metalness at `h`. Physically wants to be 0 or 1; the mod lets veins of one sit in the other. Safe to call standalone. |
| `OutputStage::emissive_at(h)` | `-> f32` | How strongly a texel at `h` glows. **Only correct on a bake-seated stage** — see Sharp edges; prefer letting `bake` call it. |
| `struct ColorRamp` | `{ stops: Vec<RampStop> }` | The field → base-colour mapping, and the **one** representation of a recipe's colour. |
| `ColorRamp::sample(t)` | `-> [f32; 3]` linear RGB | Sorted lazily on read and clamped at the ends, so a hand-edited out-of-order or empty ramp still gives the obvious intent (empty → mid-grey). |
| `ColorRamp::tint()` | `-> (hue, saturation)` | Reads the ramp's representative hue/sat from its most saturated stop — for the bench's colour knobs, not a second copy. `(0,0)` for a greyscale ramp. |
| `ColorRamp::recolor(hue, sat)` | sweeps every stop onto one hue, **keeping each stop's brightness** | The write side of the colour knobs: grey stone → gold without losing its dark→light shape. Collapses a multi-hue ramp to one hue by design. |
| `struct RampStop` | `{ at: f32, color: [f32; 3] }` | One stop: a position in `[0,1]` and linear RGB there. |

**Output-stage field notes:**

| Field | Meaning |
|---|---|
| `relief: f32` | Normal-map strength. `0` = flat (`(128,128,255)`), `1` = pronounced. |
| `roughness`, `roughness_mod` | Base roughness at the midpoint, and signed field modulation about it. |
| `metalness`, `metalness_mod` | Base metalness, and signed field modulation (ore veins in dielectric rock). |
| `ao: f32` | Ambient-occlusion strength; `0` writes a fully-open map. |
| `emissive: [f32; 3]` | The glow's **colour** (linear RGB), independent of the albedo under it — a blue rune in dark iron. |
| `emissive_strength: f32` | How brightly it glows. `0` (default) writes a black map and the shader adds nothing — emission is opt-in and costs an ordinary material nothing. |
| `emissive_band: f32` | **WHERE it glows, as a FRACTION of the field's own range** (not an absolute field value): `0.75` lights only the crests, `0` everything above the floor. `bake` seats this fraction into the field's real `[min,max]` — see Sharp edges. |

### The baker — recipe + size → maps

| Item | What it is | The one thing to know |
|---|---|---|
| `fn bake(recipe, size)` | `-> MapSet` | The main entry point. `size` clamped to ≥ 1. Cost `O(size² · enabled_channels)`. Internally seats the emissive band into the field's real range before reading maps. |
| `fn field(recipe, size)` | `-> Vec<f32>` | The raw `[0,1]` field, row-major. Exposed for the bench's live preview and as the natural unit to test tiling on. |
| `struct MapSet` | `{ size: u32, maps: Vec<Map> }` | Everything one recipe bakes to. |
| `MapSet::get(kind)` | `-> Option<&Map>` | Present for every `MapKind::ALL` after a `bake`, so a caller can index without a fallback. |
| `struct Map` | `{ kind: MapKind, size: u32, pixels: Vec<u8> }` | One baked map: **RGBA8**, `size × size`, row-major from top-left. Always RGBA even for scalar maps (that is the renderer's upload path); a scalar replicates its value across RGB with opaque alpha. |
| `enum MapKind` | `BaseColor · Normal · Roughness · Metallic · Ao · Height · Emit` | The seven maps, in `MapKind::ALL` (= preview-cycle) order. |
| `MapKind::role()` | `-> &'static str` | The `<Asset>_<Map>` filename suffix (`BaseColor`, `Normal`, … `Height`, `Emit`) — see the content-tree map vocabulary in [`Alpha/content/README.md`](../../../content/README.md). (Caveat: `Height` is not in that list — see Findings/Sharp edges.) |
| `MapKind::is_color()` | `-> bool` | `true` for `BaseColor` and `Emit` only. **Load-bearing:** it tells a caller which renderer upload path to use — `load_texture` (sRGB) for colour maps, `load_texture_linear` for data maps. Getting it backwards silently gamma-corrects data that was never a colour. |

**The seven maps:** `BaseColor` (sRGB albedo) · `Normal` (tangent-space, `(128,128,255)`
= flat) · `Roughness` · `Metallic` · `Ao` · `Height` (the raw field, for terrain
displacement) — all linear scalar in R — and `Emit` (sRGB colour; black ⇒ emits
nothing).

### The resolution ladder

| Item | What it is | The one thing to know |
|---|---|---|
| `const BAKE_DEFAULT` | `= 2048` | The size a commit bakes at, and the minimum system spec (2K). |
| `const PREVIEW_SIZE` | `= 256` | What the live preview regenerates at while dragging (~10 ms). The *same image* as the commit, sampled coarsely (resolution-independent). |
| `const BAKE_SIZES` | `[BakeSize; 4]` — 1K, 2K, 4K, 8K | The canon ladder (each rung doubles). A `const`, not a data file, so no second copy can disagree. |
| `struct BakeSize` | `{ px, label, enabled }` | `enabled = false` (4K, 8K) means **"not offered in the picker yet"**, gated pending an engine-scope texture-memory budget — **not broken**: pass its `px` to `bake` directly and it bakes correctly. |
| `BakeSize::peak_bake_bytes()` | `-> u64` | Peak RAM resident while baking (`28 × px²`: one f32 field + six RGBA8 maps), so a UI can show the cost rather than assert it. |
| `fn offered()` | `-> impl Iterator<&BakeSize>` | The rungs currently enabled (today: 1K, 2K). |
| `fn rung(px)` | `-> Option<&BakeSize>` | The rung matching `px`, enabled or not. |

### Factory patches and the randomizer

| Item | What it is | The one thing to know |
|---|---|---|
| `presets::all()` | `-> Vec<TextureRecipe>` | The four factory patches in library order. |
| `presets::{granite, sandstone, basalt, hematite}()` | one bound patch each | Bound to real material ids (Granite 10, Sandstone 12, Basalt 11, Hematite 40). **Recipes, not images** — settings a reader dials to. |
| `fn random(seed)` | `-> TextureRecipe` | Roll a whole instrument — rack, blends, ramp, output stage, glow — in one pass. Deterministic (same seed, same surface). Biased *on purpose* toward "plausibly a material": voice 1 is always the bed, detail voices sit back, one hue walked through value. `id`/`name` are the caller's to set. |

## Interactions

- **Signals / results / Model keys:** **none.** This is a pure compute crate with no
  UI surface — it captures no signals, fires no results, and reads or writes no Model
  keys. The Sablework bench that wraps it owns all of that (and follows the signal-level
  input contract there, not here).
- **What it hands other crates:** a [`TextureRecipe`] (the saveable artifact), a
  [`MapSet`] of seven RGBA8 CPU pixel buffers (the caller decides PNG vs GPU upload vs
  terrain lookup), the raw `f32` `field` (for live preview), and the resolution ladder.
  `MapKind::is_color()` is the seam that tells a GPU caller which upload path to use.
- **Threads / workers / async:** **none, deliberately.** A 2K bake is ~360 ms and even
  the preview is ~10 ms — both far past a 16 ms frame — so baking is meant to run on a
  worker (`flicker_worker::WorkerPool`) *by the caller*. Staying pure and
  single-threaded is exactly what makes that safe; threading it here would take the
  choice away from the caller.

## Gates

`cargo test -p flicker-texture` — **45 tests**, all green. The contracts they pin:

- **Blending & mixing** (`channel`) — `blend_amount_zero_is_the_bus_untouched`,
  `blend_modes_are_the_classic_identities`, `blends_stay_in_range`,
  `every_source_is_finite_and_in_range`, `a_disabled_channel_is_silent`,
  `ids_are_unique`.
- **Seamless & deterministic** (`bake`) — `the_mixed_field_is_periodic_on_the_tile`,
  `no_baked_map_has_a_seam` (the product guarantee: the step across the wrap is no worse
  than an ordinary interior step), `baking_is_deterministic`,
  `a_flat_field_bakes_a_flat_normal`, `a_zero_size_bakes_one_texel`.
- **Resolution-independence** (`bake`) — `relief_is_resolution_independent`,
  `occlusion_is_resolution_independent_and_actually_darkens` (the test whose absence once
  let AO ship blank). Every neighbourhood-derived map needs one of these.
- **The map set** (`bake`) — `the_set_is_complete_and_correctly_sized`,
  `only_the_colour_maps_are_srgb`.
- **Emission** (`bake` + `output`) — `no_glow_bakes_a_black_emit_map`,
  `an_authored_glow_bakes_coloured_light`, `a_hand_set_emit_band_bakes_a_visible_map`
  (a hand-set — not only a rolled — emit band must bake a visible map), `emission_is_off_by_default`,
  `the_band_ramps_from_its_edge_upward`, `a_degenerate_band_is_survivable`.
- **The ramp & colour knobs** (`output`) —
  `ramp_clamps_at_the_ends_and_interpolates_between`, `ramp_tolerates_unsorted_stops`,
  `degenerate_ramps_are_survivable`, `modulation_pivots_on_the_midpoint_and_clamps`,
  `recolor_sets_a_readable_tint_and_keeps_the_value_profile`, `a_grey_ramp_reports_no_tint`.
- **The recipe** (`recipe`) — `round_trips_through_json`,
  `an_unbound_recipe_loads_without_the_material_field`, `a_fresh_recipe_makes_a_sound`.
- **The ladder** (`size`) — `the_default_is_an_offered_rung`,
  `the_ladder_is_ordered_and_doubling`, `only_rungs_above_the_baseline_are_gated`,
  `peak_bake_cost_is_what_the_gate_is_about`.
- **Patches** (`presets`) — `patch_ids_and_material_bindings_are_unique`,
  `every_binding_resolves_in_the_material_index`, `every_patch_produces_a_varied_surface`.
- **The roll** (`random`) — `a_roll_is_reproducible_from_its_seed`,
  `every_roll_is_in_range_and_bakes`, `the_first_voice_always_establishes_the_field`,
  `rolls_actually_differ_from_each_other`, `a_roll_has_structure_rather_than_being_flat`,
  `adversarially_structured_seeds_still_vary`, `metalness_picks_a_side`,
  `glow_is_occasional_and_real`.

Run the instrument for real, headless:
`cargo run -p flicker-texture --example bake_patches -- <out_dir> [size]`
(bakes every factory patch to `<Name>_<Map>.png`, checks each swatch's seam; `size`
defaults to 2K). This is the offline face of the same `bake` the bench calls — if a
surface looks wrong in the window, it is wrong here too, without a GPU in the way.

## Sharp edges

- **`emissive_at` is only correct on a bake-seated stage.** Its siblings `roughness_at`
  and `metalness_at` are safe to call on any authored `OutputStage`. `emissive_at` is
  not: it reads `emissive_band` as an *absolute* field threshold, but a stored recipe
  holds a *fraction*, and the conversion (`seat_the_glow`) is crate-private and runs only
  inside `bake`. Call `bake` and read the `Emit` map; don't call `emissive_at` on an
  unbaked recipe expecting the documented fraction behaviour.
- **`emissive_band` means two things across a recipe's life.** In a stored/authored
  recipe it is a fraction of the field's range; `bake` clones the recipe and seats it into
  the field's real `[min,max]` before reading the emit map. So a hand-set band and a rolled
  band behave identically — but if you read `recipe.out.emissive_band` back you get the
  fraction you set, not the seated absolute value `bake` used internally.
- **`Height` is written under a role the content standard does not list.** `MapKind::role()`
  returns `Height`, and a commit writes `<Asset>_Height.png`, but the fixed map vocabulary
  in `Alpha/content/README.md` is `BaseColor · Normal · Roughness · Metallic · Emit · AO ·
  ORM` — no `Height`. Correct maps, but the naming claim overreaches (see Findings).
- **Tiling is bit-exact only on lattice lines.** Away from an integer multiple of a
  channel's period the tile repeat is mathematically periodic but an ULP apart — which
  nothing survives past 8-bit quantization, so the baked bytes still wrap. Tests assert
  bit-exactness *at* the seam and an epsilon in between; the epsilon one is not a bug to
  "fix".
- **An unwarped `Stripe` ignores salt and seed.** Its base pattern is a pure sine of
  position, so two unwarped `Stripe` voices differ only if their `scale`, `warp`, or
  shaping differ. Add `warp` (which *does* use salt) to make stripes into strata.
- **A rack with no `Base` first channel still works.** The bus starts at 0 and the first
  enabled fold operates on that — an honest black field, not an error.
- **Degenerate inputs are survivable, not errors.** An empty ramp samples mid-grey,
  coincident/out-of-order stops don't divide by zero, `size = 0` bakes one texel. These are
  reachable from a hand-edited recipe, so they resolve to the obvious intent rather than
  panicking. (A typo'd `NoiseKind`/`BlendMode` string in a recipe file *does* fail loud —
  serde rejects an unknown variant.)
- **4K and 8K are supported and tested, just gated.** `BakeSize::enabled == false` for them
  means "not in the picker yet" (pending an engine-scope memory budget — 8K costs ~1.8 GB
  resident and ~7 s), not "broken". `bake(&recipe, 8192)` works today.
