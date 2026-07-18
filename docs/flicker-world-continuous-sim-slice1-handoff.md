# flicker-world — Continuous Sim, Slice 1 (the cooling model + E2/E3 unify)

State-of-play for **Slice 1** of the continuous, cooling-driven planet sim
(`docs/flicker-world-continuous-sim.md` is the locked design of record; read it and
`docs/flicker-world-e3-tectonics-handoff.md` first). Slice 1 = the foundation:
introduce the master clock (**heat loss**) and dissolve the old E2/E3 boundary into
one cooling-driven convection with a **tectonics onset** condition. E4–E9 unchanged.

> **Landed 2026-07-09.** Tests: worldgen **87** (+6), worldengine **16** (+3),
> pocepochs 2 — all green; clippy clean on the three crates (the only warnings are
> the pre-existing `flicker-core/src/input/*` ones). Build clean across the
> workspace (incl. the old `flicker-world` app). Formatting left as-is — the
> repo isn't fmt-clean under the locally-installed rustfmt 1.9.0 (a version
> mismatch: untouched files like `epoch3.rs` also diff), so a `cargo fmt` would
> churn unrelated files; new code matches the on-disk ≤100-width style by hand.

---

## 1. What the slice delivers (the four confirmed asks)

1. **A global thermal state `T`.** One planetary heat budget, normalized `0..1`
   (`1` = molten magma ocean at birth). Recorded on each snapshot as
   `EpochSnapshot::temperature` (`#[serde(default)]`). **Distinct from the per-hex
   `HexState::temperature`** (a °C surface temperature written by Epoch 4) — this is
   one number for the whole planet.
2. **Newtonian cooling, composition-modulated.** `T = T_space + (T_molten −
   T_space)·(1 − k)^step` (closed form → the logarithmic feel). `k` is reduced by the
   planet's **radiogenic** content — of the Prism table the real heat producers
   present are **potassium (Z=19, the Earth-like default's 2.6 % — the dominant brake)
   and uranium (Z=92, trace unless enriched)**. More radiogenics → slower cooling →
   later onsets. This is the planet-to-planet variety dial.
3. **Convection vigor ∝ T and differentiation ∝ T.** The molten stir is fierce early
   and gentle late (`convection_vigor`, **mean-preserving** over the molten era so the
   *total* stir at the default matches the old uniform pass — the E6 guardrail); the
   density sort completes only as far as the melt stayed hot (`differentiation_settle`
   retires Epoch 2's old `duration`-fraction).
4. **The E2/E3 boundary is now one cooling-driven convection with a tectonics onset.**
   The same `convection_flow` that stirred the melt begins moving the crust as plates
   once `T` falls **below the crust solidus** AND the crust has firmed a **coherent
   lid**. A world that never cools/firms within the window stays **stagnant** (zero
   drift) — a valid outcome, no special-casing (it falls through the existing
   `drift_steps == 0` path: undrifted partition + isostasy, no orogeny).

## 2. Where it lives

- **new `crates/flicker-worldgen/src/cooling.rs`** — the whole model + tunables +
  6 unit tests. Public API:
  `radiogenic_index`, `cooling_k`, `temperature_at`, `convection_vigor`,
  `differentiation_settle`, `coherent_lid`, `tectonics_onset_delay`, plus the const
  guardrails (`T_MOLTEN`, `T_SPACE`, `BASE_COOLING_K`, `K_HEAT`/`U_HEAT`,
  `MAX_RADIOGENIC_SLOWDOWN`, `RADIOGENIC_HALF`, `T_SOLIDUS`, `T_DIFF_FREEZE`,
  `DIFF_RATE`, `CRUST_FIRM`, `LID_SHARE`). Exported from `lib.rs` as `pub mod cooling`.
- **`molten.rs`** — `run_molten_convection_cooling(cells, ctx, steps, vigor)` scales
  the per-step advection by `vigor[s]`. The old `run_molten_convection` delegates with
  an empty slice → uniform `1.0` → **byte-identical** to before (its 3 tests untouched).
- **`epoch2.rs`** — `Epoch2::settle_override: Option<f64>`. `Some` (engine, cooling-
  derived) retires the `duration` fraction; `None` (one-shot pipeline / tests) = the
  old `duration`-based settle. Its 3 tests untouched.
- **`engine.rs`** — the merge. E2 arm: compute `k` from the (conserved) composition,
  build the mean-preserving vigor schedule, run the cooling convection, set the cooling
  settle, record `T`. E3 arm: compute the **onset delay** (`molten_steps =
  epoch2_full_steps()`; lid from `coherent_lid`), derive `drift_steps =
  scrub_position − onset_delay` (0 if stagnant), and feed `drift_steps` (was `steps`)
  into `drift_plates` / `run_orogeny` / `run_tectonic_hotspots` /
  `run_protoatmospheric_erosion`. `T` recorded; carried forward unchanged by E4–E9.
- **`snapshot.rs`** — the `temperature` field. **`config.rs` / `flicker-world/world.rs`**
  — one-line `settle_override: None` in their `Epoch2` builders (kept compiling).

## 3. Calibration (the Earth-like guardrail) — verified by test

Constants tuned so **the default recipe reproduces today's plate tectonics**, so E6
is preserved:
- At the default, `T` crosses `T_SOLIDUS` **by the end of the molten era** →
  `tectonics_onset_delay == Some(0)` → Epoch 3 runs its **full** drift (asserted:
  `default_recipe_reaches_tectonics_onset_immediately_and_differentiates_fully`).
- The default molten era differentiates fully → `settle == 1.0` (same crust logic as
  today's `duration`-fraction default).
- Cooling is monotonic and starts molten (asserted:
  `cooling_clock_starts_molten_and_falls_monotonically`; `T(E1)=1`, falling through
  E2/E3, frozen E4–E6).
- Enriching potassium (`ab_K`) lowers `k` and pushes the onset **later** (asserted:
  `a_radiogenic_rich_recipe_delays_the_tectonics_onset`).

**The one at-default divergence to eyeball:** convection vigor front-loads the stir.
The *total* is preserved (mean-preserving), but the per-step trajectory differs and
convection is nonlinear, so the default E2 heat field — and thus the E3 plate
seeds/relief and E6 — will shift **slightly** from the previous build. Everything
else at the default is byte-identical logic. If E6 reads worse, the knob is the vigor
normalization in `cooling::convection_vigor` (or drop vigor-scaling toward uniform).

## 4. Verify (user runs the visuals)

`cargo run -p flicker-pocepochs` → scrub E2 (heat blooms) → E3 (plates/relief) → E6.
Confirm: **E6 still reads well** (the vigor front-load didn't regress it), and E2/E3
still look right. `T` is recorded on the snapshot but **not shown yet** (the
logarithmic heat-loss scrubber UI is a later slice). Keep
`cargo test -p flicker-worldgen -p flicker-worldengine -p flicker-pocepochs` green
(87 + 16 + 2) + clippy.

## 5. NOT in this slice (later slices — do not build without direction)

Per the design doc §4/§7, still to come as separate condition-gated layers on the
same clock: **water-delivery onset**, **outgassing** events, the
**moon→tilt→seasons** event, **chemistry onsets**, and the **logarithmic heat-loss
scrubber UI** (which would surface the recorded `temperature` + generated onset
markers, retiring the two per-epoch step scrubbers). The onset currently keys only on
the *upper* bound (T < solidus); a *lower* `T_dead` bound (convection too weak to move
a lid → Mars-style stagnation) is a possible refinement, not slice 1.

## 6. Notes / gotchas for the next session

- **Conservation holds** — convection only transfers mass, the crust veneer is a
  derived copy, drift moves only that copy; the E1–E3 exact-conservation test still
  passes. Don't move `composition` in any cooling path.
- **Onset uses the full molten era** (`molten_steps = epoch2_full_steps()`) even when
  the E2 convection scrubber is held at a partial step. `k` is unaffected (composition
  conserved), so this is correct; only the (already-partial) heat field feeds through.
- **`temperature` on old `.epoch` bakes** defaults to `0.0` (not `1.0`); it's a
  recorded scalar, not an input to regeneration, so this is harmless — any recomputed
  epoch stamps the real value.
