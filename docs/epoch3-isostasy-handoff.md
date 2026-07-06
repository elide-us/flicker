# Handoff — Epoch 3 isostasy & epoch-to-epoch visual continuity

> **Status: the isostasy fix landed.** Epoch 3's base elevation is now driven by
> Epoch 2 crust buoyancy, so the phase *visibly continues* the previous one — the
> user's core requirement: "visual continuity between these phases so I can guide
> the randomizer to things that end up being interesting." See **Landed** below for
> exactly what changed; the rest of this doc is the design record. Builds on
> `docs/epoch-pipeline-review.md` and `docs/flicker-world-handoff.md`.
>
> **Open:** visual confirmation is the user's (the headless guarantees are tested);
> a hypsometric (bimodal) elevation remap and the crust-**density** fold-in are
> documented-but-deferred refinements (see **Landed → deferred**).

## Landed (this session)

- **Isostasy base elevation** (`crates/flicker-worldgen/src/epoch3.rs`). The old
  random per-plate `continental ? continental_base : oceanic_base` is replaced by a
  **per-hex buoyancy lerp**: `base = lerp(oceanic_base, continental_base, t)` where
  `t` is the hex's `crust_fraction` **ranked across the planet** (`buoyancy_ranks`,
  a percentile rank — mirrors Epoch 4's bathtub trick, guarantees a full
  continent↔ocean spread from any spread of crust). Boundary deformation
  (mountains/rifts), hotspots, drift `cycles`, and `plate_age` add on top exactly as
  before. **Continents now land where Epoch 2's light crust floated up.**
- **`continental_fraction` repurposed** from "fraction of random plates that are
  continental" → a **land/ocean balance threshold**: the most-buoyant
  `continental_fraction` of hexes read continental (`t >= 1 - continental_fraction`).
  Same dial purpose (more → more land); shapes now follow the material. The PARAM_DEF
  band/label is unchanged, so the existing slider still drives it.
- **Per-hex vs per-plate continental.** The per-hex `HexState.continental` (used by
  `plate_age`) follows each hex's *own* buoyancy; the `Plate.continental` record
  (the Plates-view summary) follows the plate's *mean* buoyancy. `partition` now
  takes `prev` to compute it; the one external caller
  (`flicker-world/src/world.rs`) passes the Epoch-2 layer.
- **Reseed continuity falls out for free.** Base elevation depends only on the
  (upstream, unchanged) crust-fraction rank — *not* on any Epoch-3 knob — so
  reseeding Epoch 3 re-rolls the plate layout / mountain belts while the continents
  stay put. Verified by the extended reseed test.
- **Made the crust signal visually *dominant* (second pass — the actual "looks
  coherent" fix).** Tying base elevation to crust wasn't enough on screen: the
  tectonic deformation was 2.4× the continent height and fired on *any* boundary
  (incl. mid-ocean), so the Elevation view read as random mountains, not inherited
  continents. Two magnitude/structure changes:
  - **Widened the isostatic ramp** (`oceanic_base -0.45 → -0.6`,
    `continental_base 0.25 → 0.4`) so continents sit well above the Epoch-3 sea
    level (`0`) and basins well below — the gross land/ocean is the dominant relief.
    With the defaults the base crosses `0` at the continental threshold
    (`t = 0.6 = 1 - continental_fraction`), so the Epoch-3 coastline *is* the
    continental boundary, and the land share (~40%) matches Epoch 4's
    `ocean_fraction 0.6` → continuity carries into Epoch 4 too.
  - **Gated convergent orogeny by crust buoyancy** (`OCEANIC_OROGENY = 0.25`):
    continental collision throws up the full belt, ocean-ocean convergence only a
    modest island arc. Mountains now reinforce the inherited continents instead of
    spawning random mid-ocean land. (Hotspots stay ungated — real ocean island
    chains; they're localized specks that don't break the gross pattern.)
  - Verified headless: the extended reseed test now asserts the dense-crust tercile
    *averages ocean* (`elev < 0`) and the buoyant tercile *averages land*
    (`elev > 0`) in both the original and reseeded world — the gross land/ocean is
    crust-driven, not deformation-driven.
  - `continental_base` / `oceanic_base` are still plain `Default` fields (not HUD
    sliders). If "how high / how deep" wants to be a god-knob, promoting them into
    `PARAM_DEFS` + `ui_elements.json` is the follow-up — deferred, not done.
- **deferred (by data, not omission):** folding crust **mean density** into buoyancy
  (the handoff's "optional Pratt term") was dropped for v1 because elemental
  densities in `data/materials/periodic_table.json` are *gas-phase at STP* — oxygen,
  the dominant crust element by mass, is `0.00143 g/cm³`. A mass-weighted crust mean
  density is therefore a near-zero, noisy, unphysical signal. To revive it, weight by
  **rock/material** density (the `materials.json` rocks, ~2.7–3.0) or a felsic/mafic
  proxy, not raw elemental gas density. `crust_fraction` alone (the documented "main
  lever") carries the element-mix → continent coupling cleanly.
- **Tests added** (all headless, green): `epoch3::elevation_follows_crust_buoyancy`
  (the continuity guarantee — monotone elevation in crust buoyancy, full spread),
  `epoch3::continental_fraction_sets_the_land_share`, and the extended
  `world::reseeding_a_layer_preserves_the_upstream_layers` (continents stay /
  boundaries move).

---

## Original design notes (for the record)

## The north star (Elideus, this session)

A **planet-evolution simulation**: tweak input *variables* (the element mix, the
epoch knobs) and watch *derived effects* produce a meaningfully different, coherent,
realistic-looking planet — material distribution → hardness → erosion-shaped relief;
iron-density → gravity → growth; water weight → compressed seabeds; hard plates +
soft sediment + a million river-cycles → canyons. We don't have to *record* all the
physics, but the **variables must be defined so effects derive from them, and each
effect must produce a visible variation we can evolve further.** The end goal these
starter planets feed: the **heightmap-pixel → voxel-cluster** materialization bridge
(voxel-cluster is the finished renderer + celestial sim — don't rebuild it).

## The problem to fix

Stepping Epoch 2 → Epoch 3 looks like the map "jumps to something completely
different." Diagnosis:

- **The composition & crust DO carry through** — Epoch 3 never touches `composition`
  or `crust`. Viewing Epoch 3 in the Crust/Composition field is identical to Epoch
  2. The jump is: (1) the natural view auto-switches Crust → Elevation, and (2) the
  **elevation itself is built from a random plate partition with no reference to the
  materials** — each plate is randomly flagged continental/oceanic, so continents
  land wherever the random plates fell, not where the light/silica crust is.

## The agreed fix — isostasy

Drive Epoch 3's **base elevation from crust buoyancy** (real-world isostasy): light,
thick, silica-rich crust floats high (continents); dense, iron-rich, thin crust sits
low (ocean basins). Plate boundaries then carve mountains/rifts **on top**. Result:

- Continents appear **where Epoch 2's solid/light crust is** → Epoch 3 visibly
  continues Epoch 2.
- Tuning the **element mix** (Epoch 1) shapes where continents form → guides the
  whole chain from composition.
- Reseeding Epoch 3 varies the **tectonic detail** (boundaries, mountain belts)
  while the gross continents stay put (they follow composition) — the "guide the
  randomizer" behaviour.

### Implementation sketch (`crates/flicker-worldgen/src/epoch3.rs`)

Today: `let mut elev = if continental[p] { continental_base } else { oceanic_base };`
(random per-plate type). Change to a **per-hex** buoyancy base:

- **Buoyancy signal:** `crust_fraction` (Epoch 2 — how much light crust floated up =
  thickness proxy) is the main lever; optionally fold in the crust's mean density
  (lighter = more buoyant). Higher buoyancy → higher base elevation.
- `base_elev[i] = lerp(oceanic_base, continental_base, t)` where `t` is the hex's
  buoyancy **normalized across the planet** (use a percentile/min-max so a thin
  absolute spread still yields a full continent↔ocean range — mirror Epoch 4's
  bathtub-percentile trick).
- **`continental_fraction` knob** repurposes from "fraction of random plates that
  are continental" → a **land/ocean balance threshold** (the buoyancy percentile
  above which a hex reads continental). Same dial purpose, but shapes follow the
  material. Derive the per-hex `continental` flag (and per-plate, for `plate_age` /
  the Plates view, from the plate's mean) from this.
- **Keep** the plate partition, drift, boundary classification, mountain/rift
  deformation, hotspots, drift `cycles`, `plate_age` — only the *base* elevation
  changes from "random plate type" to "crust buoyancy." Boundary deformation adds on
  top exactly as now.

### Prep already done this session

- **Epoch 1–2 retune:** province `contrast` default 3.0 → 4.0 (`Epoch1Params` +
  `PARAM_DEFS`) so the crust-buoyancy field has a wider spatial spread for isostasy
  to carve. Test: `world::tests::higher_contrast_widens_the_crust_buoyancy_spread`.
  (Slider-reversible — re-validate the look.)

## Verify

- ✅ A test that `elevation` correlates with `crust_fraction` (continents where the
  light crust is) — the continuity guarantee, headless
  (`epoch3::elevation_follows_crust_buoyancy`).
- ✅ Reseeding Epoch 3 keeps the gross land/ocean pattern (composition-driven) while
  boundaries/mountains change (extended
  `world::reseeding_a_layer_preserves_the_upstream_layers`).
- ⬜ Visual confirmation is the user's (Mac Neo or the gaming PC — don't constrain on
  hardware; see `dev-box-profile`). Step Epoch 2 → Epoch 3 in the Elevation view:
  continents should sit over the high-`crust_fraction` (Crust view) regions, and a
  new Epoch-3 seed should move the mountain belts but not the continents.

## Relational alignment — the broader pass

The same principle applies down the chain (each epoch should read as a continuation):
- **E1 → E2:** ✅ crust is a direct density-split of composition.
- **E2 → E3:** the fix above.
- **E3 → E4:** water floods the *elevation* (already continuous); temperature is
  latitude+elevation. OK.
- **E4 → E5 → E6:** the life thread + drainage already build on prior fields.
A future relational pass could also feed `atmosphere` CO₂ back into temperature
(greenhouse) and `plate_age` into erodibility (old crust weathers deeper) — see
`epoch-pipeline-review.md` §4.

## Also landed this session (context for the viewer)

- **Per-layer reseed** (`world::seed_chain` / `mutate_epoch_params` /
  `generate_with_seeds`): seeds are a per-epoch chain; reseeding layer *e* advances
  `seeds[e]`, re-derives later ones, and re-rolls only that epoch's knobs — upstream
  layers stay byte-identical. This is what makes "build a nice world phase by phase"
  work, and what the isostasy fix completes (continuity *between* phases).
- **Terrain view** (`globe::build_terrain` + `FieldSampler::sample_blended_at`): the
  within-hex hardness relief, surfaced from the existing `FieldSampler`. Tuning
  constants (`SAMPLE_SCALE`, `MACRO_BUMP`, `MICRO_GAIN`, `TERRAIN_SUBDIV`) are blind
  first-guesses awaiting visual tuning — and it's still hardness-weighted *fractal*
  ridging, not yet true erosion-pattern shapes (the user's eventual want).
