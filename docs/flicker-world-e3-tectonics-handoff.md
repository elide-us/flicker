# flicker-world — Epoch 3 Plate Tectonics Handoff (the "ridge-spine" issue)

State-of-play for the E1–3 iterative rebuild, focused on the current Epoch-3
tectonics and the artefact to fix next. Read alongside
`docs/flicker-world-epoch-redesign.md` (manifesto) and
`docs/flicker-world-epoch-redesign-slice1-handoff.md` (the engine core).

> **Update (2026-07-08) — ridge-spine fixes 1–3 landed + a pre-hydraulic weathering
> layer.** Fixes 1–3 addressed the structural causes: the **double-count is reconciled**
> (isostatic crust-pile is now the *single* convergent-mountain source; `run_orogeny` cut
> back to the subduction asymmetry — trenches + volcanic arcs — plus rifts), the **rank
> isostasy is softened** (base elevation blends the percentile rank with an *absolute*
> Airy thickness term), and the **arc magnitude lowered** (`OROGENY_RATE` 0.5→0.3). The
> user's visual pass then confirmed the plate/material structure is exactly right and E6
> is fantastic, and identified the *remaining* thin streaks as unchecked deep-time relief
> growth (hotspot chains + swept ridges) with nothing eroding them until E6. So a **new
> gentle, hardness-weighted, water-independent weathering pass** (`run_protoatmospheric_erosion`
> — Epoch 6's thermal-creep primitive dialled down) now runs in **E3 (at the tectonic step
> rate, so it scrubs and keeps pace with the growth) and gently onward in E4/E5**, wearing
> the sharp features down before E6's full hydraulic erosion. This is the real fix for
> what §4 item 4 called "overlapping ridges" — erode them, don't just smooth the input.
> **Fix 5 (re-check E4 sea level) remains** a visual-verify item. Tests: worldgen 76 (+4),
> worldengine 13, pocepochs 2; clippy clean on the three crates. See §4/§5 for knobs.

> **Update (2026-07-08b) — plate motion rebuilt as a rigid conveyor (slice 1).** Watching
> the E3 scrubber, the user diagnosed that the drift wasn't *moving plates as units* — the
> old `drift_material`/`advect_crust_step` **diffused** 20% of every cell's crust each step
> AND **re-derived the partition from the smeared field every step**, so seams marched and
> blended ("rippled") and nothing was truly transported. Replaced with **`Epoch3::drift_plates`**
> (`crates/flicker-worldgen/src/epoch3.rs`): the partition is derived **once**, then plates
> move as **coherent units** — `advance_plates` transports each plate's crust by **whole
> cells** along its motion (a phase accumulator advances one full cell every `1/ADVECT_FRAC`=5
> steps), the **plate label rides with the crust** so seams stay crisp/stable, convergent
> streamlines **accrete** (thicker-crust plate overrides → its label claims the cell), and
> no-inflow cells are spreading ridges that **produce** thin fresh crust (`produce_fresh_crust`
> from the local mantle). The conserved element ledger (`composition`) is **never moved**
> (the crust veneer is what travels — the E2 crust-vs-composition split), so conservation is
> by construction. `Epoch3::apply` was split into `apply_with(ctx, prev, &Partition)` so the
> engine feeds isostasy/deformation the **tracked** partition instead of re-deriving it. This
> is **slice 1 (motion + seams)**; **slice 2 = realistic subduction** (crust returning to the
> mantle at trenches, heavy-under-light asymmetry) + differentiated ridge melt — NOT yet
> built. Tests: worldgen 77, worldengine 13, pocepochs 2; clippy clean. Motion is now
> coherent full-cell hops (~4 advances at the default `e3_duration`); if plates should
> traverse further, raise `e3_duration` or the advance rate.
> **Open next:** slice 2 (subduction), then one Epoch-2 nit (user, deferred).

> **Update (2026-07-08c) — Epoch 2 rebuilt as a convection engine (heat-field slice).**
> The "E2 nit" grew into a redesign: E2 was a **zonal shear** (`run_molten_convection`
> pushed composition eastward, faster at the equator) that banded material and drove
> nothing — and E3 seeded plates from `crust_fraction` peaks, so there was **no heat→seam
> link**. Reworked `run_molten_convection` (`crates/flicker-worldgen/src/molten.rs`) into a
> **material-driven mantle convection**: buoyancy from composition density (light rises /
> dense sinks) self-organizes into coherent **convection cells** via diffusion + a buoyancy
> feedback, and the surface flow advects the melt hot→cold (conserved). It writes a new
> `HexState::heat` field (`0..1`, cold downwelling ↔ hot upwelling). Differentiation
> (`apply`) is unchanged. Added a **Heat view** to the pocepochs V-rotation
> (Plates→Heat→Relief). Chosen by the user (AskUserQuestion): **emergent-from-material**
> convection, and **heat-field + view FIRST**, then a follow-up rewires E3's plate
> seeds/motions onto the heat (downwelling → convergent/subduction, upwelling → divergent
> ridges). **Not yet wired: E3 still seeds from `crust_fraction`** — that's the next slice.
> Note: replacing the zonal shear changes E2's material transport, so **E3–E6 shift** (the
> convection sorts material into cells instead of equatorial bands) — re-verify E6. Tuning
> consts in molten.rs (`HEAT_DIFFUSE`/`HEAT_FEEDBACK`/`CONVECT_ADVECT`) set cell count/
> sharpness. Tests: worldgen 79, worldengine 13, pocepochs 2; clippy clean.

> **Update (2026-07-08d) — heat verified good + exploration tooling.** User confirmed the
> heat map reads well (seed-varying "blooms", some swirl/streaking). Three follow-ups:
> **(1) Full-Earth size unlocked** — `MAX_FREQ` 48→**96** (config.rs); the pocepochs
> planet-size slider now caps at `EARTH_FREQ` (was ½-Earth), warns above ½-Earth (still
> defaults to ½-Earth for snappiness). **(2) E2 convection scrubber** — mirrors the E3
> iteration scrubber: `WorldEngine::set_epoch2_steps`/`epoch2_full_steps` +
> `run_molten_convection` honoring `steps==0` (the raw buoyancy seed) + a slider on Epoch 2
> in `world_hud.lua`/`scene.rs` to watch the heat blooms form/move/blend. **(3) "Always 8
> plates"** — because `e3_plates`=8 hard-seeds the count; **deferred into the E3-from-heat
> rewire** (plates will emerge from the convection cells → the variable count the user's
> "accumulate plates from the molten crust over time" describes), NOT throwaway-patched.
> Tests: worldgen 79, worldengine 13, pocepochs 2; clippy clean.
> **Open next (unchanged):** wire E3 seeds/**motion** to the heat field — motion must be a
> per-cell **flow field** (curved), which is what turns the recurring straight mountain
> spines into curved ranges (user's diagnosis) AND makes plate count emergent; then E3
> slice 2 (subduction).

> **Update (2026-07-08e) — E3-from-heat rewire LANDED.** The big one. E3 is now driven by
> the E2 convection heat instead of `crust_fraction` peaks + a hard-coded plate count:
> **(a)** [`convection_flow`](crates/flicker-worldgen/src/epoch3.rs) — per-cell tangent flow
> down the heat gradient (hot→cold) = the plate **motion**, a curved spatially-varying
> field; **(b)** [`Epoch3::partition_heat`] seeds plates at heat **maxima** (upwelling) with
> **emergent count** (one per bloom, no truncation to 8) and watershed-grows over the heat;
> **(c)** the whole E3 pipeline now takes the per-cell flow: `drift_plates`/`advance_plates`
> advect crust along it (curved paths), `apply_with(…, flow)` classifies seams from the
> *local* flow (`(flow[i]−flow[nb])·across`, across every edge → curved convergent/divergent
> lines), and `run_orogeny`/`run_tectonic_hotspots` take the per-cell flow. The one-shot
> `EpochTransform::apply` synthesizes a **uniform per-plate flow** so the legacy pipeline is
> byte-identical (uniform flow → intra-plate closing 0 → old straight boundaries). Result:
> **emergent, seed-varied plate count** (the "always 8" is gone) + **curved seams/ranges**
> (motion follows the convection cells, not a straight vector). Convergence sits at cold
> downwellings (→ mountains via pile + trench/arc via run_orogeny), divergence at hot
> upwellings (→ ridges producing fresh crust). Tests: worldgen 80 (+conveyor/partition
> rewired, +emergent-count test), worldengine 13, pocepochs 2; clippy clean.
> **Open next:** E3 **slice 2** — realistic subduction (crust→mantle at trenches,
> heavy-under-light) + differentiated ridge melt; then the deferred Epoch-2 differentiation
> polish if wanted. (Province boundaries vs flow-seam alignment is a possible refinement.)

> **Update (2026-07-08f) — E3 slice 2 (realistic subduction) LANDED + viewer legend/view
> filter.** `advance_plates` (`epoch3.rs`) now resolves each convergence by buoyancy: the
> thickest-crust arrival is the **overrider** (keeps its crust, claims the cell); a clearly
> thinner slab (`crust_fraction` gap > `SUBDUCT_CONTRAST` 0.2) **subducts** — only
> `SUBDUCT_ACCRETE` (0.15) of it scrapes onto the overrider, the rest **recycles to the
> mantle** (dropped from the crust copy-layer; `composition` already holds it); similar
> buoyancy → **collision**, both pile (continent-continent → curved belts). `produce_fresh_crust`
> now differentiates ridge melt (light formers, density ≤ `FRESH_CRUST_DENSITY_MAX` 3.5).
> Net: **trench recycling balances ridge production** → crust cycles instead of growing
> unbounded (fixes the slice-1 note). Conserved `composition` ledger untouched. **Viewer:**
> a per-view **legend** (swatches+labels, top-right) and a **per-epoch V-view filter**
> (`globe::views_for_epoch`: E1 Material · E2 +Heat · E3 +Relief/Plates · E4 +Hydro · E5
> +Veins · E6+ all; snaps to a valid view on scrub). Tests: worldgen 81, worldengine 13,
> pocepochs 2; clippy clean.
> **Direction locked (user):** after slice 2, **unify the epochs into a continuous layered
> simulation** — each epoch becomes the *onset* of a process that keeps ticking into later
> epochs (convection keeps running under tectonics, chemistry keeps layering), not a
> one-shot hand-off. Recommended path: incremental — make each epoch's process persist into
> later epochs (prototype: convection continuing through E3 so seams shift over time) →
> later a unified-tick model (epochs = onset markers on one evolving state). This feeds the
> future heightmap detail tier + atmospheric cycles. **This is the next major thread.**

---

## 1. Where we are (E1–3)

The forward-regenerative engine (`crates/flicker-worldengine`) drives 9 immutable
epoch snapshots; the viewer is the `flicker-pocepochs` shell client (timeline
scrubber, planet-size slider on E1, **tectonic-iteration scrubber on E3**, view
modes incl. `Plates`). E1 (composition scatter), E2 (molten differentiation), and
E3 have been iterated into real, material-driven, deep-time sims.

**Epoch 3 today = real plate tectonics with actual crust motion:**
- Partition is **material-derived + reseed-stable**: `craton_field` (E2
  `crust_fraction` smoothed, `CRATON_SMOOTH_PASSES` 3) → `craton_seeds` (local
  maxima) → **watershed grow** (`grow_plates`; boundaries settle in thin-crust
  valleys → they *track the material*, not straight Voronoi) → `plate_drift`
  (toward the plate's thinnest-crust edge). Pure function of the frozen E2 crust +
  the fixed iteration count, so R only jitters deformation magnitudes
  (`e3_plates` and `e3_duration` are excluded from reseed jitter).
- **The crust physically drifts**: `Epoch3::drift_material` advects each plate's
  crust (composition + `crust_fraction`) along its motion for `steps` iterations,
  **conserved**, re-deriving the partition each step so **seams migrate** and
  provinces collide + merge.
- **Calibrated timeline**: `MY_PER_TECTONIC_STEP` ≈ **0.32 My/iteration**
  (`ADVECT_FRAC` 0.2 × a ~50-mi hex at ~5 cm/yr ≈ 1.6 My/hex). Default full =
  20 iters ≈ 6.4 My (~4 hexes of drift).
- **Deformation** (`tectonics::run_orogeny`, run on the *drifted* material at the
  final partition): subduction (heavy plate → trench, light overrider → mountain
  arc), collision (both pile the full belt), rift at divergence; uplift
  accumulates + diffuses inland over `steps`, trenches/rifts accumulate sharp.
- **Isostatic base** (`Epoch3::apply`): `buoyancy_ranks` turns `crust_fraction`
  into a **percentile rank**, then a linear ramp `oceanic_base(-0.6) →
  continental_base(0.4)` sets base elevation.

Engine E3 arm order (`engine.rs`): `drift_material` → `partition(drifted)` →
`apply(drifted)` (isostasy) → `run_orogeny` → `run_tectonic_hotspots`.

---

## 2. The symptom (see the two screenshots)

By the water stage, most generations look the same: a **mostly-submerged planet**
with a crisscrossing network of **long, thin, high ridge-spines** that spike well
above everything else, over broad low watery basins. It's rough in roughly the
right *spirit*, but the relief is **bimodal and linear** — sharp spine-or-ocean,
little in between — rather than believable continents + mountain belts.

---

## 3. Assessment — why (four compounding causes)

1. **Double-counted mountains.** The drift now **piles crust at convergence** →
   high `crust_fraction` → high isostatic base *there*. THEN `run_orogeny` adds
   *more* uplift at those same convergent boundaries. Convergence contributes
   twice → over-tall spikes. (The two mountain sources were never reconciled after
   material advection landed.)

2. **Rank-based isostasy amplifies everything to the extremes.** `buoyancy_ranks`
   maps `crust_fraction` to *percentiles*, then a linear ramp to elevation. So a
   thin ridge of slightly-piled crust ranks near the **top → near-max height**,
   and everything else ranks low → **near-min → underwater** — regardless of the
   actual magnitude of the piling. This is the biggest driver of "either a spike
   or an ocean." (The rank trick was right for the *static* thin-spread E2 crust;
   it's wrong once the drift creates real spread.)

3. **Migrating boundaries lay overlapping ridges.** As the seams migrate over the
   drift iterations, crust piles along the swept boundary **paths**, not just their
   final positions — so different iterations' convergence lines leave crust ridges
   at different places → an **overlapping crisscross network**. `run_orogeny` then
   stacks its own ridges at the final boundaries on top.

4. **Advection is zero-sum, so it self-sharpens.** Crust conserved: what piles into
   spines is drained from everywhere else (→ thinner crust → below sea level →
   water). More iterations ⇒ sharper spines AND more ocean — the bimodal look
   intensifies with time rather than settling into continents.

**Net:** the user's instinct (lower subduction/impaction magnitude) will reduce
spike *height*, but the **spine network structure** comes from causes 1–3, so
magnitude alone won't land it.

---

## 4. Proposed fixes (priority order)

1. **[DONE] Kill the double-count.** Took the recommended path: relief comes **from
   the piled crust via isostasy** (new `Epoch3::crust_pile`, a saturating Airy term on
   the drifted `crust_fraction` — the *single* convergent-mountain source), and
   `run_orogeny` was cut back to the **subduction asymmetry only** — a trench on the
   down-going plate + a volcanic **arc** on the overrider + rifts. Its symmetric
   continental-collision belt was **removed** (that relief is now the isostatic pile), so
   the two never double-count. (`run_orogeny`'s `else` collision branch → an explicit
   `else if theirs + SUBDUCT_MARGIN < mine` override-arc branch; symmetric collision hits
   neither.)

2. **[DONE] Soften the rank isostasy.** `Epoch3::apply`'s base elevation now blends the
   percentile rank with an **absolute** Airy term — `crust_fraction / planetary_mean`
   passed through a `tanh` — weighted by the new `Epoch3::isostasy_abs` (engine sets it to
   `ISOSTASY_ABS_BLEND` = 0.6). Near-mean crust reads as broad shelves instead of a
   rank-stretched full gradient whose only emergent land is spines; the per-hex
   `continental` flag stays a **pure percentile** so `continental_fraction` is still an
   exact land-share knob. Both new fields **default to 0** → the old one-shot
   `six_epoch_stack` pipeline is byte-identical (it never drifts, so nothing piles).

3. **[DONE, partial] Lower the deformation magnitudes.** `OROGENY_RATE` 0.5→0.3 (it is
   the subduction *arc* now, not a belt). `TRENCH_RATE` / `RIFT_RATE` / `DEPRESS_MAX` /
   the ramp `continental_base`/`oceanic_base` were left as-is — trim these next only if
   the visual pass shows trenches/rifts too deep (the pile saturation + absolute-isostasy
   compression already cap the highs).

4. **[DONE, via erosion] Tame the overlapping-ridge / streak artefact.** Solved by
   *eroding* rather than smoothing the input: `run_protoatmospheric_erosion`
   (`crates/flicker-worldgen/src/erosion.rs`) — a gentle, **hardness-weighted**,
   talus-limited **thermal creep** on the derived elevation (soft rock sheds, hard rock
   stands as ridges), symmetric so the elevation field is conserved. The engine runs it in
   **E3** (steps = the tectonic iteration count, so it scrubs with the slider and keeps
   pace with relief growth) and **gently in E4/E5** (`WEATHER_STEPS_PER_DURATION`), before
   E6's full hydraulic erosion. Knobs: `WEATHER_TALUS` 0.08 / `WEATHER_RATE` 0.25 /
   `WEATHER_STEPS_PER_DURATION` 2 (engine consts, not levers yet). E6's own
   material-conserving erosion is untouched. Interleaving weathering *between* tectonic
   steps (uplift-a-bit, erode-a-bit) is a possible later refinement over the current
   post-pass.

5. **[TODO] Re-check E4 sea level** — the "mostly water" is E4's binary-search sea level
   reacting to the elevation distribution; now that the base relief is saner, verify the
   land/ocean balance before tuning `e4_water_delivery` or the ramp.

---

## 5. Where things live / knobs

- `crates/flicker-worldgen/src/epoch3.rs` — `buoyancy_ranks` (percentile rank, still
  drives the `continental` flag + the run_orogeny buoyancy contrast); `apply` (the
  **softened isostasy**: `isostasy_abs` blend of rank + absolute Airy `ISOSTASY_ABS_GAIN`
  tanh, then the `crust_pile` saturating pile term with `PILE_HALF`; ramp
  `continental_base`/`oceanic_base`); `advect_crust_step` + `drift_material`
  (`ADVECT_FRAC`); calibration consts (`MY_PER_TECTONIC_STEP`, `PLATE_SPEED_CM_YR`). New
  `Epoch3` fields `isostasy_abs` / `crust_pile` **default 0** (old pipeline unchanged).
- `crates/flicker-worldgen/src/tectonics.rs` — `run_orogeny` = **asymmetry only** now
  (`OROGENY_RATE` **0.3** arc, `OROGENY_DIFFUSE` 0.12, `OROGENY_MAX` 1.0, `SUBDUCT_MARGIN`
  0.18, `TRENCH_RATE` 0.35, `RIFT_RATE` 0.25, `DEPRESS_MAX` 2.0). Symmetric collision adds
  no belt; the override plate lifts an arc, the heavy plate digs a trench.
- `crates/flicker-worldengine/src/engine.rs` — E3 arm. Double-count **reconciled here**:
  `base3.crust_pile = mtn` (the mountain source) + `base3.isostasy_abs = ISOSTASY_ABS_BLEND`
  (0.6); `run_orogeny` gets `mtn` for the *arc* only. `set_epoch3_steps` /
  `epoch3_full_steps` / `epoch3_my_per_step` (scrubber).
- Levers (`Alpha/content/data/epoch_defaults.json`): `e3_mountain_uplift` (now scales the
  isostatic pile **and** the arc), `e3_rift_drop`, `e3_duration` (drift amount / My),
  `e3_plates` (count). Tuning consts (`ISOSTASY_ABS_BLEND`, `ISOSTASY_ABS_GAIN`,
  `PILE_HALF`, `OROGENY_RATE`) are code-side, not levers — surface them if the visual pass
  wants them on the HUD.

## 6. Verify (the user runs visuals)
`cargo run -p flicker-pocepochs` → scrub to E3, `V` cycles Relief/Material/Hydro/
Veins/**Plates**, the E3 slider scrubs the tectonic iterations (shows My). Fixes
are verified by the user against these views; keep `cargo test -p flicker-worldgen
-p flicker-worldengine -p flicker-pocepochs` green (72 + 13 + 2 currently) + clippy.
