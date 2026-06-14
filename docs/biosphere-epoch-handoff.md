# Handoff — the Biosphere arc: life, time-gated agents, and preservation geology

> The design for the **life epoch** (precursors → prokaryotes → fungus → flora)
> and the **time-gated preservation** mechanic that motivates it — late
> decomposers leaving Carboniferous coal/oil, carbonate plankton building the
> chalk Cliffs of Dover. Plus the **per-epoch "cycles" (within-epoch time)**
> primitive that the whole arc hangs on. Companion to
> `docs/epoch-data-audit-handoff.md` (what each epoch records) and
> `docs/clayengine_world_generation_spec_v2.md` (which leaves biology explicitly
> open: "Biology beyond biome assignment — part of epoch 6 sophistication or a
> separate epoch 6.5, to be decided").
>
> **Status:** cycles primitive (tectonics + hydrosphere) built; **prebiotic
> precursor chemistry built into Epoch 4** (the first life-thread). **Resolved
> (Elideus):** life/chemistry is a **cross-cutting thread woven through the
> existing epochs, NOT a standalone epoch** — see §3/§5.1. The fuzzy gap between
> the six formation epochs and the layer-9 runtime sim is where the biosphere
> lives; the old "GM/underground layers" were loose conceptual placeholders, and
> layer 9 is itself a 9-band sim (so "9" is already overloaded). Later life stages
> still need steering.

---

## 1. The vision (Elideus)

A planet should read its biological history in its rocks. The load-bearing idea:
**which agents exist at which time changes what gets preserved.**

- **Carboniferous coal/oil window.** Trees (lignin) evolve *before* the organisms
  that can rot lignin ("termites" / wood-decomposers / white-rot fungi). For that
  finite interval, dead forest isn't decomposed — it buries and compresses into
  **coal and oil**. Once decomposers arrive, burial no longer outruns decay and
  the coal window closes. So a planet's oil endowment is a function of *how late
  the decomposers showed up*.
- **Cliffs of Dover.** Once carbonate-shelled marine plankton
  (coccolithophores/foraminifera) evolve, shallow warm seas rain their CaCO₃
  shells onto the floor for the rest of time → thick **chalk/limestone**. Tectonic
  uplift later exposes it as white cliffs. Thickness ∝ how long carbonate life has
  existed × shallow-warm-sea area.

These are not cosmetic: they are **harvestable resource layers** whose existence
and abundance fall out of the *timing* of life — exactly the kind of emergent,
tunable history the epoch sim is for.

## 2. The keystone primitive — within-epoch cycles (time)

Time-gating ("agent X appears after cycle N") is impossible without a notion of
elapsed cycles *inside* an epoch. So the first concrete step is a per-epoch
**length / cycle** control. Each epoch interprets "cycles" as its own within-epoch
elapsed time / pass count:

| Epoch | What a cycle means | Status |
|---|---|---|
| 3 Tectonics | **drift time** — displacement = motion-rate × cycles → taller belts, deeper rifts, **longer island chains** | ✅ built (`Epoch3.cycles`, HUD "Drift time"; `cycles=1` = unchanged) |
| 6 Erosion | erosion–deposition passes | ✅ already exists (`Epoch6.iterations`, HUD "Iterations") |
| 4 Hydrosphere | chemistry time — gates prebiotic precursor accumulation | ✅ built (`Epoch4.cycles`, HUD "Chem cycles") |
| 1–2 Composition/Differentiation | weakly time-like (differentiation could deepen) | low priority |
| later life stages | generations / geologic stages — where the time-gates live | ⏳ threaded into 5/6 + the runtime sim |

**Recommendation (decided):** keep `cycles` **per-epoch** (each epoch's time is
independently tunable), not a single global "planet age." A global multiplier can
layer on top later if wanted.

## 3. Life as a cross-cutting thread (not an epoch) — decided

Life and early chemistry are **threaded through the existing epochs**, building on
the fields each already produces, rather than bolted on as one new epoch. The
thread, earliest-reasonable first:

1. **Prebiotic precursors — Epoch 4 (built).** The first water is the first place
   chemistry can happen, so precursors start here. `HexState.prebiotic` (0..1)
   accumulates where **liquid water × organic elements (C/N/P/S) × energy (warmth ×
   volcanism)** coincide, over `Epoch4.cycles` — warm shallow seas and **wet
   volcanic shores (the hotspot island chains carry their `volcanic` forward from
   Epoch 3)** are the cradles. Knobs `e4_cycles` / `e4_prebiotic_rate`; `Prebiotic`
   view (barren → algal green → amber soup). Tests:
   `prebiotic_chemistry_favors_warm_shallow_organic_water`,
   `more_cycles_brew_more_precursors`.
2. **Microbial life — Epoch 5 (built).** `LifeStage` enum (`Barren → Prebiotic →
   Microbial → Fungal → Floral`, ordered, never regressed) + `HexState.biomass`.
   Epoch 5 crosses precursors into prokaryotic life where energy pushes them over
   the edge: `potential = prebiotic × (1 + vent_life_boost × hydrothermal)` — the
   vents are the cradle — and `potential ≥ microbial_threshold` ⇒ `Microbial` with
   `biomass = potential`. (Epoch 4 also tags the `Prebiotic` stage where precursors
   form.) Knobs `e5_microbial` ("Life threshold") / `e5_vent_life` ("Vent boost"); a
   **`Life` view** (per-stage tint brightened by biomass) is Epoch 5's natural view.
   Test: `microbial_life_emerges_from_precursors`.
3. **Fungus / flora & dead-matter — Epoch 6 (built).** On land, `growth =
   warmth × moisture` advances `life_stage` (advance-only `max`): `≥ floral_threshold`
   ⇒ Floral (forest), `≥ fungal_threshold` ⇒ Fungal; `biomass` grows with `growth`,
   and dead **`organics`** = `biomass × organics_rate × iterations` accumulate (the
   coal/oil precursor). Ocean keeps its Epoch-5 microbial life; alpine stays bare.
   Knobs `e6_flora` ("Flora threshold") / `e6_organics` ("Peat rate"); the `Life`
   view now shows olive fungus / green forest on land. Test:
   `flora_and_organics_establish_on_temperate_wet_land`.

4. **Time-gated preservation — Epoch 6 (built).** The headline. Into a dedicated
   `deposits: Composition` ledger: **coal/oil** = `organics × decomposer_onset` as
   carbon (`decomposer_onset` = how late wood-decomposers came; `1` = never → the
   coal window wide open, `0` = always present → none); land carbon reads as coal,
   submerged as oil. **Chalk** = `(1 − carbonate_onset) × shallow × warmth ×
   carbonate_rate` as calcium+carbon carbonate on warm shallow sea shelves (uplift
   later exposes the cliffs). Knobs `e6_decomposer` / `e6_carbonate`; a **`Deposits`
   view** (coal near-black ↔ chalk white, by calcium share, brightness by mass).
   Test: `time_gates_preserve_coal_and_chalk`.

Per-hex fields grow as the thread advances: `prebiotic`, `life_stage`, `biomass`,
`organics`, `deposits` — **all built**. The thread spans Epochs 4→5→6, ending in
the preserved geology.

### The time-gated agents (the payoff)

- **Decomposers gate** (`decomposer_gate`, in cycles). Before it: `organics` buries
  → converts to a **coal/oil** deposit (add C to the deposits layer), thickness ∝
  organics produced while ungated. After it: decomposers recycle organics → no net
  coal. ⇒ the Carboniferous window is literally `[flora_onset, decomposer_gate)`.
- **Carbonate-life gate** (`carbonate_gate`, in cycles). After it: shallow warm
  submerged hexes accumulate **chalk/limestone** (Ca + C carbonate deposit) each
  remaining cycle, thickness ∝ `(cycles − carbonate_gate)` × shallow-warm factor.
  Uplift (high-but-submerged shelves, or Epoch 3 convergence) exposes cliffs.

## 4. Data model additions (proposed)

Per-hex on `HexState`:
- `life_stage: LifeStage` (enum above).
- `biomass: f32` (0..1) — standing living matter.
- `organics: f32` — accumulated dead matter (the coal/oil precursor).

Preservation deposits stay **in the element ledger** (consistent with "veins are
composition thresholds, not objects" — `water-cycle-handoff` §0, the
`voxel-data-layering` memory): coal/oil = **C**, chalk = **Ca + C** (carbonate).
Open question §5.2: fold into `crust`/`composition`, or a dedicated
`deposits: Composition` so harvestable resources are separable from bulk rock
(recommended).

## 5. Open decisions — need steering before building

1. **Epoch placement — RESOLVED (Elideus).** Life is a **cross-cutting thread
   through the existing epochs**, not a new epoch (§3). The old "GM/underground"
   layers were loose conceptual placeholders with no strict limits; layer 9 is
   itself a 9-band sim. **Parked (there may be value, not yet framed):** promoting
   real formal sub-surface layers — water tables, ice layers, underground strata —
   later. Don't build those unprompted.
2. **Deposit representation — RESOLVED (Elideus): a dedicated `deposits:
   Composition` layer.** Keeps harvestable resources distinct from bulk rock — the
   foundation for the larger underground/deposits concept ("more data is better to
   express the simulation"). Coal/oil = carbon (land vs submerged = the contextual
   read), chalk = calcium+carbon carbonate. Built (§3.4).
3. **Life-sim richness (Phase 1 vs 2).** Discrete `life_stage` + gates now
   (recommended); a real population/NPP model later.
4. **Which gates/knobs to expose.** `life_onset`, `decomposer_gate`,
   `carbonate_gate`, plus the Biosphere `cycles` — the dials that author a planet's
   oil/chalk endowment.
5. **Views.** A "Life" field (stage/biomass) and a "Deposits" field (coal/oil/chalk)
   in `color.rs`.

## 6. Slice ladder

- **Cycles primitive.** Tectonics ✅, Hydrosphere ✅. Optional: unify naming
  (`iterations` ↔ `cycles`) across epochs. Small.
- **Prebiotic precursors (Epoch 4). ✅** `prebiotic` field, knobs, view, tests (§3.1).
- **Microbial life (Epoch 5). ✅** `LifeStage` + `biomass`; prokaryotic life at the
  vents; `Life` view; `e5_microbial` / `e5_vent_life` knobs (§3.2).
- **Fungus / flora + dead matter (Epoch 6). ✅** `life_stage` → Fungal/Floral,
  `biomass` grows, `organics` accumulate on temperate-wet land (§3.3).
- **Time-gated preservation (the headline). ✅** Dedicated `deposits` ledger;
  `decomposer_onset` → coal/oil, `carbonate_onset` → chalk; `Deposits` view (§3.4).
- **Precipitation alignment. ✅** Epoch 6 reads Epoch 4 `precipitation` (single
  moisture truth); E4 split `humidity` (atmosphere vapor) vs `precipitation`
  (proximity-driven, coastline-inclusive).
- **Then (the underground concept):** grow `deposits` into the harvestable
  underground layer — depth/strata, harvest hooks, the formal sub-surface layers
  (water tables / ice / strata) the deposits ledger was kept distinct for; resource
  harvest hooks; tie into the re-homed water cycle (`epoch-data-audit-handoff` §5).

## 7. Verify (what's built so far)

`cargo test -p flicker-worldgen` (46 unit + 1 integration: cycles; prebiotic;
life `microbial_life_emerges_from_precursors`,
`flora_and_organics_establish_on_temperate_wet_land`; preservation
`time_gates_preserve_coal_and_chalk`), `-p flicker-world` (15), clippy clean.
Visual: the **`Life`** field shows the thread evolve epoch to epoch — amber
precursors (E4) → teal microbial mats at the vents (E5) → olive fungus / green
forest (E6); the **`Deposits`** field shows the geology it leaves — near-black
coal/oil from late-decomposer organics, white chalk from carbonate seas. Author it
all with the epoch knobs ("Precursor rate", "Life/Vent", "Flora/Peat",
"Decomposer onset", "Carbonate onset"). The formation life arc is complete.
