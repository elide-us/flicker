# Prism Book Updates — World-Gen Unification Rulings

**Dates:** 2026-07-12 – 2026-07-13 · **Source:** the flicker world-gen unification review (engine side)
**Use:** carry this into the Prism recompile/adjudication thread. Each item names the
affected book/passage, the change, and why. Engine-side records are already aligned
(flicker memory `worldgen-unification-rulings`, CLAUDE.md banner); the books are the
remaining side of the sync.

---

## A. Ruled changes (Aaron, 2026-07-12–13 — bring the books into line)

### A1. Remove the solar-system-simulation dependency from the Nine-Epoch Bake (BookIII)

Current text (BookIII, Nine-Epoch Bake): *"Epochs 1–3 (molten) … are replaced by the
solar-system simulation, so the planet's starting composition is the exact accounting
of the matter the world accreted."*

Change: **delete the solar-system-simulation clause; keep the accounting principle.**
Proposed replacement language:

> The planet's starting composition is the exact accounting of the matter the world
> accreted: a single canonical accretion budget carrying every element of the
> simplified periodic table in fixed amounts. Epochs 1–3 (molten) are simulated
> directly by the world engine's chemistry — differentiation, convection, and the
> onset of plates are outputs of that budget, never seeds.

Why: ruled — there is no system-formation simulation in the world-gen pipeline. The
budget is authored and canonical; the chemistry simulation produces everything else.

### A2. World selection — the seven shards (wherever world-selection canon lives)

Capture as canon: **all playable worlds are generated from the same canonical
accretion budget; only the generation seed differs.** In lore they are seven shards
of reality. Each shipped shard is hand-selected by the designer from generated
candidates — the generator proposes, the designer chooses.

Why: settles "planets similar to Earth, never Earth," and guarantees completeness —
every world contains all 28 canon elements, so every crafting chain functions on
every shard.

### A3. Cosmology confirmations (BookV / BookVI — little or no text change)

The fixed 8-body roster, sun-anchored sky, real-ephemeris moon, and the deterministic
celestial event calendar are **confirmed settled canon**. Only action: remove any
residual cross-reference implying a formation simulation produces the live sky. The
mythic creation frame (BookVI) is untouched.

### A4. Ore genesis — veins form by tectonic-cycle distillation (BookIII, world system)

Proposed canon paragraph (draft voice — rework freely):

> Veins are made by the living planet, never placed. Subduction zones are sorting
> boundaries: when one plate runs beneath another, its cargo is carried down into the
> melt, and chemistry partitions what returns — each pass up through a seam filters
> and concentrates. Gold that rides a slab down comes back richer at another seam
> later. Over many convective cycles the seams of the world become its treasure
> maps: veins of varying density and grade, hosted in the rock that made them
> (aluminum in bauxite, iron in hematite), and refinement returns everything the
> container holds.

Engine-facing mechanical notes (book-optional): every simulation layer is a
three-dimensional **ledger volume**; large bodies and concentrations of elements,
compounds, and minerals are first-class parts of the hex's mass accounting; that
structure is exactly the hardness/softness metadata the erosion cycles iterate on.

Why: ruled — the *purpose* of subduction, convection, and maintaining separate layers
in the model is gameplay vein formation, and the mechanism is close enough to
reality to reproduce.

### A5. Epochs are condition-flagged phases, never scheduled stages (BookIII, Nine-Epoch Bake + anywhere epochs read as an ordered sequence) — ruled 2026-07-13

Risk in the current framing: the Nine-Epoch Bake reads as an ordered schedule of
stages. Change: **keep the epoch names; recast the definition.** Proposed language
(draft voice — rework freely):

> An epoch is a description of a phase of the world's formation, defined by the
> chemical state that opens it — crust formation begins the moment the world has
> cooled enough to form crust, not at an appointed hour. Epochs overlap, and some
> never end: the deep world convects and crust forms to this day. Transitions are
> never planned; they happen, and when they happen they mark the passage into or
> out of one or many epochs at once. And epochs can return — a world whose air once
> ran hot with carbon may run hot again; the epoch that shelters life endures only
> while its conditions hold.

Also carry: the bake's "epochs 1–6 are discardable scaffolding / 7–9 are the
retained world" distinction survives as **production-artifact language** (what is
kept), not as epoch semantics (what an epoch is); any numbering survives as a loose
customary order at most.

Why: ruled — the simulation programs **causes only and observes results**; reaching
a **life-supporting epoch is the objective, never a programmed path** (a world's
sim flags the transition when the chemistry produces it). Follows the earlier canon
demotion of all epoch/crust durations to non-canon working values (ledger 65AE9274).

### A6. Radiogenic heat — the world's inner fire is its own uranium and potassium (BookIII, world system) — ruled 2026-07-13

Approved as committed canon (an extension to the molten layer). Proposed canon
language (draft voice — rework freely):

> The deep heat of a world is the slow burning of its own substance: the uranium
> and potassium it accreted decay through the ages, and their warmth keeps the
> inner world molten and moving. Young worlds burn fiercer — the fuel was richer
> then — and every world cools as its fire fades, each at the pace its own
> chemistry set.

Engine notes (book-optional): per-cell heat keyed to the conserved U/K ledger
masses with real decay curves; heat-only today (decay products not mass-tracked —
the honest upgrade U→Pb+He, K→Ca stays a flagged option). Closes §C2.

### A7. Geology appendix — the sim-required minerals and rocks are adopted (BookIII) — ruled 2026-07-13

The Element → Mineral → Rock tier and the rocks.json content are committed canon:
**12 rock-forming minerals** (olivine, pyroxene, anorthite, albite, orthoclase —
the old Feldspar entry splits into its three feldspars — biotite, muscovite,
magnetite, pyrite, dolomite, spodumene, serpentine), **19 rocks as modal
mixtures** (peridotite the mantle, basalt/gabbro the ocean crust, granite the
continents, quartzite the ridge-maker, banded iron gated on oxygenation, coal
seams the buried carbon…), and the **erosional_resistance** trait (resistance to
weathering and incision, deliberately not Mohs — limestone is Mohs 3 but
dissolves; quartzite endures). A rock remains a **classifier** — a modal mixture
of conserved minerals, never a stoichiometric compound and never a third ledger.

Engine-side (queued mechanical task): one mineral registry — rocks.json's minerals
fold into compounds.json's vocabulary and id space; rocks.json keeps only the
modal recipes + resistance. Harvestable flags are curated gameplay canon and are
NOT extended in that pass (spodumene "the Li ore" / dolomite "second Mg ore" notes
flagged for a separate ruling). Design note: erosional_resistance (material
property) and quench-speed failure mode (§C3, formation history) are one erosion
design, done together.

**Ruled clarification (Aaron, 2026-07-13): minerals ARE compounds** — one list was
always the point; the consolidation is semantic identity, not housekeeping. Canon
note the books may carry: the original compound list was very limited simply from
lack of design need, not closure — **tables such as this remain open for
extension** as design need arises (each extension ratified as canon on adoption,
as this ruling does). The simplified periodic table is the contrast case: it stays
a hard constraint (28 elements, ceiling 30, Book rulings only).

**Follow-on ruling (Aaron, 2026-07-13): the combined Feldspar row is RETIRED.**
Book III's single Feldspar entry — two formulas in one row (KAlSi3O8, NaAlSi3O8) —
is superseded by the split feldspars (Albite/Orthoclase/Anorthite) and retired
from the compound catalog (engine id 42; retired ids are permanent, never reused).
Books: drop or annotate the combined Feldspar row in the recompile. The
formula-less rock-name rows (Slate/Limestone/Sandstone/Obsidian/Granite) remain
OPEN — same pattern, awaiting their own ruling.

### A8. Material determination — crafting is the final arbiter (BookIII, materials/crafting) — ruled 2026-07-13

BookIII leaves the compound→256-material mapping explicitly unassigned. Ruled
direction for when that passage is filled in: what a container's contents *express
as* is decided by a **registry of composition + condition rules** (diamond is
carbon under pressure and heat; loose quartz sediment is sand, the same sand
cemented is sandstone), the determination is **game-accurate rather than
geology-accurate**, and **the crafting system is the final arbitrator** — the
materials and elements that matter to crafting are the ones the rules must
distinguish; all else collapses to the nearest of the 256. Detailed rules live in
the engine spec of record, not the books; the books carry only this principle.

---

## B. Open forks proposed for closure (books ledger items)

- **B1 — 9C677816 (voxel-as-container vs material-ledger): CLOSED 2026-07-13, ledger 6C9C3A9C — ruled yes.** (The closure clears the recompile's fork flag only; voxel-as-container has been the design since BookIII's original text — the "1000 carbon = diamond" origin — never new canon.) Proposed canon =
  **voxel-as-container**. A voxel holds a portion of its cluster's element/compound/
  water distribution and *classifies to* a material; the hex is the container of the
  world's mass accounting one level up. This is the locked engine invariant and now
  also the vein-accounting model (A4).
- **B2 — 78524243 (water-as-compound vs water-as-effect): CLOSED 2026-07-13, ledger C72F8F49 — ruled yes** (BookIII §Volatiles and Static Formation Events). Proposed canon = **water
  is real H₂O in the compound accounting**, formed from H and O and delivered mass;
  "water/ice/lava as effects" survives only as render classification, never as the
  accounting.
- **B3 — atmospheric gas species (compound-table extension): applied 2026-07-14 (Aaron approved "expanding the Compounds table is exactly appropriate").** Under the R6b principle
  *the compound table is open for extension by design need* (element table stays ceilinged 28/30),
  the outgassing/atmosphere sim added five `sim_required` gas compounds to `compounds.json`:
  **N₂ (91), SO₂ (92), HCl (93), CH₄ (94), NH₃ (95)** — joining the existing H₂O (1) and CO₂ (2).
  These are the atmosphere's real named species (the "kinds": steam · carbon hotbox · N₂ temperate ·
  SO₂/HCl acidic · CH₄/NH₃ reducing), formed by the same conserved dual-ledger accounting as B2.
  Ids continue the space (90 → 91-95; retired 42 untouched). List is deliberately **complete
  enough to establish the pattern, known incomplete** (O₂, O₃, H₂S, CO, N₂O await design need).
  Physicals are proposed-starting-values pending ratification (gases: mohs/brittleness 0.0).

---

## C. Rulings requested from the books

- **C1 — valence strip: RULED 2026-07-13 (Aaron) — DROP.** Valence is not useful for
  anything built so far; remove it now, re-add later only if a future system needs
  it (books ledger 217A3A9B already leaned removal). Engine strip **LANDED
  2026-07-13** — reapplied: the first strip thread's landing was lost in the
  working tree when a parallel task touched the same data (regression noted by
  Aaron). `valence_electrons` is gone from `periodic_table.json` and the
  `Element` struct; worldgen epoch4's oxide model keeps the former values as
  LOCAL constants (`book_valence`), outside the vocabulary. Books drop the VE
  column in the recompile.
- **C2 — radiogenics: RULED 2026-07-13 (Aaron) — APPROVED as committed canon** (see
  §A6). U + K only is acceptable; Th stays out (ceiling-30 headroom remains if a
  future ruling wants it). Heat-only decay accepted for now; the mass-tracked
  upgrade (U→Pb+He, K→Ca — all expressible in the 28-table, ppb-scale) is a
  flagged future honesty option.
- **C3 — quench speed: RULED 2026-07-13 (Aaron) — PROMOTE to canon.** A
  long-standing conversation that never materialized and still needs to. Recorded at
  **layer-formation time** beginning with the crust epochs (provenance, like the
  layer's formation record). Quench speed + composition + stress history determine
  the rock's failure mode — shed grains, spall slabs, or fail catastrophically — a
  primary determinant of **what erodes** (differential erosion, coastline regimes,
  preserved verticality). Books promote the candidate field from canon-candidate to
  canon.
- **C4 — pentagons:** the 6-mountain / 6-ocean split remains idea-grade — confirm no
  action needed now on the split. **Annotated 2026-07-13 (Aaron):** the pentagon
  points stay important in both senses — lore (Advent gates, beyond-survivable) AND
  rendering: the runtime streams up to three hex maps resident at lowest LOD, a
  scheme pure hex adjacency supports and pentagon zones break, so the 12 zones are
  strategically managed exclusions the sim must render genuinely unreachable
  (impassable extremes — the one sanctioned carve-out from "no authored geography").

---

## D. Engine-side consequences of this ruling (cross-reference only; no book action)

Slated for **clean-sweep deletion** from the flicker repo (as thorough as possible):
`crates/flicker-system`, `examples/flicker-sol2`, `crates/flicker-celestial`, and
their docs (`flicker-sol2-*.md`, `flicker-celestial-*.md`,
`flicker-solarsystem-handoff.md`). The supernova/ejecta framing dies with them — the
books never contained it. Keepers: `Alpha/flicker-solarbirth` (intro cinematic over
the fixed roster), `flicker-flight` (camera cinematics), and voxel-cluster's
day/night sky (the best iteration toward the future calendar/skybox service).
`Epoch3Handoff` and all "system sim seeds the planet" language are dead everywhere.
