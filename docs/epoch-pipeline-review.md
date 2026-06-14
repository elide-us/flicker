# Epoch pipeline review — what each epoch does, and how its layers feed forward

> Current-state operational reference for `crates/flicker-worldgen` (the 6-epoch
> formation chain) as it stands after the cycles / biosphere / precipitation work.
> Companion to `docs/epoch-data-audit-handoff.md` (recorded-data vs spec) and
> `docs/biosphere-epoch-handoff.md` (the life thread). Re-verify names against code.

The chain is a sequence of **pure transforms** over `Vec<HexState>` (one snapshot
kept per epoch). Each reads the previous layer + the shared `EpochCtx { tables,
dirs, neighbors, seed }` and writes/refines fields. No epoch mutates a prior
snapshot; the only cross-hex side output is Epoch 3's `Plate` list (held in
`WorldData.plates`).

---

## 1. The chain at a glance

| # | Epoch | One-line job | Headline output |
|---|---|---|---|
| 1 | Composition | distribute element mass (heavy→equator, volatile→pole) + province noise | `composition` |
| 2 | Differentiation | density-sort: light → crust, heavy sinks; thin crust = volcanic | `crust`, `volcanic` |
| 3 | Tectonics | plates, boundaries, the proto-heightmap, hotspot chains, crust age | `elevation` |
| 4 | Hydrosphere | seas, temperature, atmosphere, precipitation, life precursors | `sea_level`, `temperature`, `precipitation` |
| 5 | Mineralization | hydrothermal veins + microbial life at the vents | `hydrothermal`, `vein_*`, `life_stage` |
| 6 | Erosion | weather the terrain, biomes, flora, time-gated coal/chalk | `biome`, `deposits`, refined `elevation` |

---

## 2. Per-epoch detail

### Epoch 1 — Composition (the seed)
- **Does:** per-hex element mix from abundances, biasing dense elements toward the
  equator (`equator_bias`) and gas-state ones toward the poles (`pole_bias`), with
  correlated fBm provinces (`contrast`, `frequency`). Normalized to a target mass.
- **Reads:** `dir.y` (latitude), `seed`, element density/state (tables).
- **Writes:** `composition`.
- **Feeds forward:** *everything.* E2 differentiates it; E4 outgasses its volatiles
  & reads its organics; E5 picks vein metals from it; the field sampler blends
  hardness from it.

### Epoch 2 — Differentiation
- **Does:** splits `composition` by element density — `≤ crust_density_max` rises to
  the `crust`, heavier sinks (still in the bulk). Polar cooling (`polar_thickening`)
  thickens crust; thin crust ⇒ high `volcanic`.
- **Reads:** `composition`, `dir.y`, element densities.
- **Writes:** `crust`, `crust_fraction`, `volcanic`.
- **Feeds forward:** `crust` is `surface()` — E5 deposits vein metal into it, E6
  derives erodibility from its hardness, the field sampler reads it. `volcanic`
  drives E3 (refinement), E4 (outgassing weight + prebiotic energy), E5
  (hydrothermal).

### Epoch 3 — Tectonics
- **Does:** grows a Voronoi plate partition over the neighbour graph; gives each
  plate a type (`continental`) + rigid drift; classifies each hex `boundary` from
  relative motion; writes `elevation` (continents high / oceans low + convergent
  mountains, divergent rifts), `orogeny`, hotspot island chains; ages crust
  (`plate_age`, BFS from spreading ridges). `cycles` scales accumulated drift
  (longer chains, taller belts).
- **Reads:** `neighbors`, `dirs`, `volcanic`, `seed`.
- **Writes:** `plate`, `continental`, `plate_age`, `boundary`, `elevation`,
  `orogeny`, refined `volcanic`; cross-hex `Plate` list.
- **Feeds forward:** `elevation` is the central field — E4 floods it, E6 erodes it.
  `boundary`/`orogeny`/`volcanic` drive E5 hydrothermal; `orogeny` drives sampler
  relief.

### Epoch 4 — Hydrosphere (+ life precursors)
- **Does:** bathtub-fills to a `sea_level` (the `ocean_fraction` percentile),
  setting `water_depth`; `temperature` from latitude (flattened by `axial_tilt`)
  minus an elevation lapse; outgasses a well-mixed `atmosphere` from the volcanic
  hexes' volatiles (H/C/N/S/Cl) + local water-vapor; `precipitation` = ocean
  proximity × warmth (floored), diffused inland; brews `prebiotic` precursors in
  warm shallow/wet organic cradles over `cycles`, tagging `life_stage = Prebiotic`.
- **Reads:** `elevation`, `volcanic`, `composition`, `dirs`, `neighbors`.
- **Writes:** `sea_level`, `water_depth`, `temperature`, `atmosphere`,
  `precipitation`, `prebiotic`, `life_stage`.
- **Feeds forward:** `temperature` + `precipitation` → E6 biomes & flora;
  `precipitation` → E4's own prebiotic land-wet; `prebiotic` → E5 microbial life;
  `water_depth` → E5 fluid drive & E6 chalk/ocean.

### Epoch 5 — Mineralization (+ microbial life)
- **Does:** computes a `hydrothermal` signature from boundary plumbing + volcanism +
  fluid proximity; traces ore **veins** greedily along the fault network, depositing
  metal up into the `crust` and tagging `vein_element`/`vein_strength`; crosses
  precursors into **microbial** life where `prebiotic × (1 + vent_boost × hydro) ≥
  microbial_threshold`, seeding `biomass`.
- **Reads:** `boundary`, `orogeny`, `volcanic`, `water_depth` (self + neighbours),
  `composition`, `prebiotic`, `neighbors`, `seed`.
- **Writes:** `hydrothermal`, `vein_element`, `vein_strength`, `crust` (+metal),
  `life_stage` (→ Microbial), `biomass`.
- **Feeds forward:** `vein_*` + the metal-enriched `crust` → E6 erodibility contrast
  & sampler filaments; `life_stage`/`biomass` → E6 advances them.

### Epoch 6 — Erosion (+ flora + preservation)
- **Does:** accumulates `flow` down the drainage graph; hydraulic erosion (mass-
  conserving, rate ∝ flow × slope × erodibility-from-hardness) + thermal creep
  refine `elevation`/`water_depth` and lay `sediment`; classifies `biome` from
  `temperature` + `precipitation` + elevation; advances the life thread on land
  (`growth = warmth × precipitation` → Fungal/Floral) and grows `biomass`;
  accumulates dead `organics`; banks time-gated **`deposits`** — coal/oil
  (`organics × decomposer_onset`, land=coal/sea=oil) and chalk (carbonate seas).
- **Reads:** `elevation`, `sea_level`, surface `composition`/`crust` (hardness),
  `temperature`, `precipitation`, `life_stage`/`biomass`, `water_depth`, `neighbors`.
- **Writes:** refined `elevation`/`water_depth`, `flow`, `sediment`, `watershed`,
  `biome`, `life_stage` (→ Fungal/Floral), `biomass`, `organics`, `deposits`; the
  cross-hex `Watershed` basin list.
- **Feeds forward:** terminal formation epoch — its layer is what the **field
  sampler** (sub-hex detail) and the future runtime/water-cycle read.

---

## 3. Layer dependency map — producer → consumers

The crux of "what effect each layer has on subsequent epochs." **Bold** = drives a
later *formation* epoch; *italic* = terminal (only the field sampler, the viewer,
or the not-yet-built runtime reads it).

| Layer | Set by | Read by later epochs |
|---|---|---|
| `composition` | E1 | **E2, E4, E5**; sampler |
| `crust` / `crust_fraction` | E2 | **E5** (metal in), **E6** (hardness); sampler |
| `volcanic` | E2 (→E3) | **E4, E5** |
| `plate` | E3 | *viewer* |
| `continental` | E3 | **E3** (plate_age); *viewer* |
| `plate_age` | E3 | *viewer only* — no downstream consumer yet |
| `boundary` | E3 | **E5**; (E3 plate_age) |
| `elevation` | E3 (→E6) | **E4, E6**; sampler — the spine |
| `orogeny` | E3 | **E5**; *sampler* |
| `sea_level` | E4 | **E6** |
| `water_depth` | E4 (→E6) | **E5, E6** |
| `temperature` | E4 | **E6** |
| `atmosphere` | E4 | *recorded only* — greenhouse/energy/clouds are future |
| `precipitation` | E4 | **E6** (biomes + flora) — now the single moisture truth |
| `prebiotic` | E4 | **E5** |
| `life_stage` | E4→E5→E6 | advanced each epoch (`max`); *viewer*, future gates |
| `biomass` | E5→E6 | **E6** (organics); *viewer* |
| `organics` | E6 | **E6** (deposits) |
| `deposits` | E6 | *Deposits view*; future underground layer |
| `hydrothermal` | E5 | **E5** (microbial life, veins) |
| `vein_element` / `vein_strength` | E5 | *E6 erodibility, sampler filaments* |
| `flow` | E6 | *Flow view*; future water cycle |
| `sediment` | E6 | *Sediment view*; future sampler / water cycle |
| `watershed` | E6 | *Watersheds view* (per-hex basin id) |
| `biome` | E6 | *viewer; runtime surface dressing* |
| `Plate` (cross-hex) | E3 | *recorded only* — future |
| `Watershed` (cross-hex) | E6 | *recorded* (basin id/outlet/members); future |

---

## 4. Observations worth noting

- **`elevation` is the keystone.** It is the one field re-read by *two* later epochs
  and the sampler; almost every macro feature traces back to it. Anything that
  perturbs E3 elevation ripples through seas, climate, erosion, and life.
- **`volcanic` is the quiet hub of the life/chemistry side** — it gates outgassing,
  prebiotic energy, and hydrothermal vents, so the hotspot island chains end up
  seeding atmosphere, ore, *and* the first life. One field, three subsystems.
- **The life thread is a clean monotonic chain** (`prebiotic → life_stage/biomass →
  organics → deposits`), each link consumed by the next — no orphan in the middle.
- **Terminal-but-recorded layers** (no subsequent-epoch consumer yet): `plate_age`,
  `atmosphere`, `deposits`, `flow`, `sediment`, `biome`, the `Plate` list. These are
  meaningful outputs whose consumers are the **viewer** and the **future runtime**
  (water cycle, underground layer, gameplay) — i.e. they point *out* of the
  formation chain, not forward within it. Candidates if we want richer cross-epoch
  coupling: feed `atmosphere` CO₂ back into E4 temperature (greenhouse), or
  `plate_age` into E6 erodibility (old crust weathers deeper).
- **Drainage is now surfaced.** `flow` (rivers, log-scaled), `sediment`, and the
  `watershed` basins each have a view; the cross-hex `Watershed` structure (the
  spec's last missing cross-hex output) is recorded in `WorldData.watersheds`.
  `flow`/`sediment` remain the hand-off points to the re-homed runtime water cycle
  (`epoch-data-audit-handoff` §5) — they now feed the *viewer*; the *sim* that
  consumes them as a conveyor is the next phase out of formation.

---

## 5. Verify

`cargo test -p flicker-worldgen` (47 unit + 1 integration), `-p flicker-world`
(15), clippy clean. The pentagon-patch integration test confirms every field stays
finite through the whole chain on the real icosahedral topology including the
5-neighbour defect.
