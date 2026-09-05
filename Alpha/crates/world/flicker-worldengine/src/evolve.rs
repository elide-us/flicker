//! **The evolution era — TWO LAYERS, TWO EDGE SYSTEMS (Aaron 2026-08-25):**
//! *"Molten seams drive insertion, crust seams drive compaction and uplift."*
//!
//! The molten/deep-crust seams are DISCONNECTED from the plates: they only
//! define where material is INSERTED (the volcanoes). The upper crust — the
//! separately-rolled plates, shelves and beds — has ITS OWN edges: inserted
//! material spreads WITHIN its plate and pushes toward that plate's rim, and
//! the rim is where the combination math happens — materials pushed
//! together, pressure, compaction, uplift, new layers pushed up and eroded
//! back down by the water cycle. The molten edge never acts on the plate
//! edge directly; only material does.
//!
//! The stage Aaron specified (2026-08-25): we skipped God Mode's plate
//! FORMATION phase — the shelves, beds and heat seams are already rolled — and
//! start the layer interactions directly. Each tick:
//!
//! 1. **Volcanism injects material.** The crust's lava vents and the seam heat
//!    below the lower crust push rock height into the plate shell — heat =
//!    pressure = volume, the same law that drives the plate relief.
//! 2. **Hot rock SPREADS.** A tile's fresh material spills to lower
//!    neighbours at a rate the heat below sets — the hotter the locality, the
//!    farther a rock field can grow.
//! 3. **Boundaries uplift and subduct.** Each plate turns about its own Euler
//!    pole (the God Mode conveyor's framing, repurposed); at a boundary the
//!    RELATIVE velocity's component across the edge classifies it — closing
//!    edges build mountains (a continent rides up, an ocean bed dives and
//!    loses material), opening edges leak a little ridge rock.
//! 4. **Plates CARRY their rock.** When a plate's accumulated turn reaches a
//!    tile step, its mutable material shifts one hex along its motion — so a
//!    field formed over a stationary plume is dragged away tick by tick and a
//!    NEW field grows at the plume: the Hawaiian chain, hundreds of miles of
//!    islands trailing a hot spot.
//! 5. **Rarely, a LAYER FORMS.** Where enough rock has piled up AND the place
//!    justifies it — compression at a closing boundary, or serious volcanic
//!    volume — the loose rock consolidates into a stratum: a new layer above
//!    the plate shell, which then rides the plate like everything else.
//!
//! Transformations, never outcomes (rule 935269B7): no mountain, chain or
//! stratum is placed. The poles are rolled once; everything after is the
//! per-tick process, two-pass (compute into buffers, then apply — the tick
//! engine's own law, F9C4514D).

use glam::Vec3;

use crate::crust::CrustField;
use crate::map::{HexMap, TileId};
use crate::plates::OCEAN_BED_H_FRAC;
use crate::seams::SeamField;

/// The derived plate rate's clamp band, radians of turn per tick —
/// GEOLOGICAL ceiling: the hottest ground crosses a tile every ~30 ticks;
/// cooler ground creeps slower, cold ground not at all (the local push field
/// carries no floor — stillness is a real state). The rate itself is DERIVED
/// (Aaron 2026-08-25: cells have no innate motion — the seams PUSH; a
/// plate's spin is the integrated shove of the welling under it), only its
/// magnitude is clamped into this band so no plate races or stalls the era.
const RATE_MAX: f32 = 0.0009;
// ── INJECTION CALIBRATION (Aaron 2026-08-25: "a single tick should be
// nearly imperceptible" — a dozen or so tiles of material per seam per tick,
// ~144 tiles on a 93K-tile planet at a dozen seams; volcanoes fire only
// OCCASIONALLY, and when one does it leaves a whole lava flow in one tick) ──
/// Upwelling pinches per SEAM per tick — Aaron's dozen: total per tick =
/// cells × this (12 seams × 12 ≈ 144 tiles), each a tiny discrete insertion
/// whose pressure then reverberates through the plate via the spread.
const UPWELL_PER_SEAM: u32 = 12;
/// Rock one upwelling pinch inserts (tile-width units). Raised twice on
/// Aaron's in-window reads (2026-08-25): +30% first, then ×2.5 — "islands
/// were being eroded away before they can exist"; the pinch COUNT stays his
/// dozen-per-seam, the AMOUNT carries the throughput.
const UPWELL_INJECT: f32 = 0.10;
/// **THE DECOMPRESSION FLOOR** — the share of every injected quantum that
/// is always FRESH mantle melt. The rest is drawn back out of the
/// subduction well, so once the collisions have sunk something the vents
/// run mostly on returned material instead of minting every gram. The
/// injector's own RATE is untouched — production-neutral, because the
/// spreading sets the rate and a return sized by the whole slab quadrupled
/// the crust (8D917A78) — and the mantle covers whatever the well cannot,
/// so a world with no subduction still spreads at full pour.
const MELT_FLOOR: f32 = 0.15;
// (CRUSTAL INSULATION IS GONE — 0b. From the uplift-mint fix (5097F306) to
// here, the injected quantum scaled by 1/(1 + (ground/6)²): the column over
// a vent choked it, so a maturing world's volcanism faded and "the planet
// finished growing". That was a RATE LIMIT standing in for an equilibrium
// the world did not have (D381F5FE lists it first among the result-shapers
// the reinstated well replaces). With the trench eating plate at every live
// collision edge, the planet finishes growing because the SINK matches the
// SOURCE. Measured over 12,000 bench-true ticks at SLAB_SHARE 0.025, the
// fade was carrying only ~7% of the deceleration: late-window mean-ground
// growth +0.0938/3000t with it, +0.1005/3000t without, and the whole gate
// suite — the anti-compounding canary included, its 1.3× line untouched —
// stays green either way. Removed, not weakened: the vents now pour at the
// rate the spreading sets, forever, and the well answers them.)
/// The chance per tick that ONE volcano erupts somewhere on the planet —
/// most ticks pass quiet; occasionally a vent floods its locality. (0.3 →
/// 0.45 in the same production crank.)
const PLANET_ERUPT_CHANCE: f32 = 0.45;
/// An eruption's lava flow, by ring from the vent — a LARGE one-tick deposit
/// across a somewhat large area, left to erode or compress like anything
/// else.
const ERUPT_FLOW: [f32; 3] = [0.7, 0.42, 0.21];
/// Only a real pile rides a step: loose material under this stands as part
/// of the ground (films and dust are the plate's skin, not cargo) — what
/// bounds a step's cost to the LIVING fields instead of every tile the era
/// ever touched.
const MOVE_MIN: f32 = 0.05;
// (COLD WELD needs no heat threshold under volcanoes-only production: every
// tile that is not itself a vent welds its resting pile into the plate — a
// lava flow becomes a LAYER of the crust instead of cargo, and the active
// area saturates at the vent fields.)
/// The activity floors. A write under ACT_EPS neither counts as a change nor
/// keeps its tile in the frontier; a change must clear RING_EPS to WAKE ITS
/// NEIGHBOURS — so a settling pile keeps trimming itself quietly while the
/// echo stops spreading, and the planet's active area SATURATES at the
/// disturbed localities instead of creeping outward ring by ring forever.
const ACT_EPS: f32 = 0.01;
const RING_EPS: f32 = 0.022;
// (Seam-heat injection is GONE — Aaron 2026-08-25: "there's no kind of
// material interchange here except as by the heat seams pushing materials;
// this is just generating materials out of nothing, not acceptable." The
// seams PUSH — motion and splitting only; the VENTS are the one mantle
// chimney. The derived motion had aligned divergent plate edges exactly onto
// the molten seams, which stacked every per-seam-tile source into visible
// dotted chains of invented land.)
/// Spill HYSTERESIS: a pile fills SILENTLY until it crosses the release
/// level, then dumps its excess down to the rest level in ONE batch — and
/// goes quiet again until refilled. A pinched tile therefore fires every
/// several pinches instead of trickling every tick: the discrete-event
/// economy the near-imperceptible tick demands, same shape as the pressure
/// uplift. The heat below scales how completely the batch drains — hot rock
/// flows, cold rock slumps reluctantly.
const SPILL_RELEASE: f32 = 0.16;
const SPILL_REST: f32 = 0.04;

/// PRESSURE (Aaron 2026-08-25: nothing is consumed — colliding plates raise
/// the PRESSURE in the hexes that merge, pressure uplifts, and extreme
/// pressure pushes a NEW layer up): what one collision event adds, how much
/// a resolve event uplifts, the decay that bleeds pressure off, and the
/// EXTREME threshold that forces a layer. (The per-tick closing-speed gain is
/// gone — rim pressure is sourced by MATERIAL arrivals now, the two-layer
/// law.)
// (MERGE_PRESSURE retired with the claims: collision pressure now arrives
// only through the opposing-flow jams.)
/// What a blocked rim arrival converts to, per unit of blocked material —
/// the coupling between the insertion economy and the crust's own edges.
const RIM_PRESS: f32 = 0.8;
/// How strongly a receiver's OWN push must oppose an incoming flow before
/// the meeting is a COLLISION (pressure, no transfer) — as a fraction of the
/// rate ceiling. Where two material streams are driven into each other,
/// mountains; everywhere else, flow.
const OPPOSE_FRAC: f32 = 0.25;
/// Uplift is a DISCRETE EVENT now (Aaron 2026-08-25: a tick moves hundreds
/// of tiles, not the planet): pressure accumulates silently, and only when a
/// tile's crosses the trigger does it fire one quantum of rock and pay the
/// trigger back — so a boundary tile changes occasionally, staggered by its
/// own accumulation, instead of creeping every tick.
const UPLIFT_TRIGGER: f32 = 0.35;
const UPLIFT_QUANTUM: f32 = 0.06;
const PRESSURE_DECAY: f32 = 0.995;
/// Pressure saturates — a boundary can only be so jammed.
const PRESSURE_MAX: f32 = 1.0;
const PRESSURE_FORM: f32 = 0.6;
// THE COLLISION EDGES (Aaron 2026-08-27: "not like plates as much as a more
// formal detection of edges where pressure from these motions encounter each
// other… as it starts to collide, as it continues to collide, as it stops
// colliding"): the crust-side corollary of the molten seams. Every jam
// deposits its flux into a per-tile ledger; the ledger is an EMA the cycle
// folds, an edge is LIVE while the EMA holds, and its AGE counts the ticks
// of persistence — birth, life and death tracked, plate-like behaviour left
// to EMERGE from where the flows actually keep meeting.
/// EMA fold per tick on the collision-flux ledger.
const EDGE_BLEND: f32 = 0.04;
/// The intensity at which an edge counts as LIVE (age runs while held).
const EDGE_LIVE: f32 = 0.004;
/// Ticks of persistence at which an edge's uplift leverage saturates.
const EDGE_AGE_REF: f32 = 400.0;
/// A mature edge's extra uplift leverage (quantum ×(1+gain) at saturation) —
/// a long-lived convergence line builds a RANGE where a young jam lifts a
/// hill; still gather-only, never a mint.
const EDGE_GAIN: f32 = 2.0;
/// OROGENIC SHORTENING: the share of the foreland's loose pile a stalled
/// mover SCRAPES onto itself each time its drift fires into the jam — two
/// columns become one taller one (conserved transfer), and the emptied
/// foreland is the foredeep in front of the range.
const STACK_SHARE: f32 = 0.4;
/// UPLIFT METAMORPHISM: converted rock hardens by the bed grade under the
/// jam (an indurated old marine floor collides into hard mountains), floored
/// at the plain uplift grade and clamped at the scale's ceiling.
const UPLIFT_HARD: f32 = 1.2;
pub const META_HARD_CAP: f32 = 2.4;
/// The grade sediment consolidates at — sedimentary beds are the soft end
/// of the spectrum until the marine press indurates them.
const SED_GRADE: f32 = 0.85;
// (The opening-boundary ridge-rock leak is gone for the same reason — an
// opening edge exposes newborn floor when a column actually vacates; it does
// not rain free rock along the line.)
/// MAX DENSITY (Aaron 2026-08-25, the quartz hypothetical): a column can only
/// hold so much LOOSE material — merging past the cap TRANSFORMS the capped
/// mass into a permanent stratum (compressed: less height, more density,
/// harder), and the overflow stays loose ABOVE it as the next young layer.
/// The single-material stand-in for the R7 derived-material-conditions
/// registry (composition + conditions → material; compounds.json carries the
/// densities and hardnesses the data-driven version will read).
const LOOSE_CAP: f32 = 1.1;
const DENSIFY: f32 = 1.0;
/// Rock height that can consolidate into a stratum, the compaction it keeps
/// (NEAR ALL of it — compaction reshapes, the ledger keeps its material:
/// Aaron's zero-loss law, 2026-08-25), and the heat that can justify
/// formation without compression — narrow conditions: forming a layer is rare.
const FORM_HEIGHT: f32 = 0.85;
const FORM_KEEP: f32 = 1.0;
const FORM_HEAT: f32 = 0.7;
/// A merged column's base is capped here; the overflow converts to loose
/// rock — visible uplift instead of an ever-thickening slab.
const BASE_CAP: f32 = 0.75;
/// **THE BASAL CEILING** — delamination in the era's OWN overburden units.
/// A column's load is its stack weighed by the grade each part consolidated
/// at (this model runs compaction, density and hardness on ONE axis: past
/// the cap loose mass becomes "less height, more density, harder"), and the
/// ceiling is the load a FULL slot ladder under the marine press's hardest
/// grade can carry. Past it the root FOUNDERS: the excess converts DOWN
/// into the well, never up as height. DERIVED from the era's own caps —
/// an imported absolute is a ceiling that never fires (E91E482D:
/// DELAMINATION_PA sat 4x out of reach and opened 0 times in 5762).
const ROOT_FULL: f32 = BASE_CAP + L3_CAP + L4_CAP;
const DELAM_LOAD: f32 = MARINE_HARD_CAP * ROOT_FULL;
/// **THE DISPOSITION** (the buoyant standoff, DCA4D316: land does not ride
/// over land) — which converging material SINKS. On the one-axis material
/// model, consolidated IS dense-and-hard: uplift metamorphism hardens, the
/// max-density law hardens, and sediment is "the soft end of the spectrum
/// until the marine press indurates them". So material at or above this
/// grade has been consolidated enough to stand — it arrests and
/// crumple-thickens through the existing scrape — while young mafic floor
/// below it subducts, wholly at the softest the vents pour.
///
/// **THE METAMORPHIC FLOOR** is the buoyancy line (0b, replacing the plain
/// erosion reference HARD_ROCK the first cut borrowed): `UPLIFT_HARD` is
/// exactly the grade an orogen converts material AT — the bottom of the
/// metamorphic band. What has been through the mill is continental and
/// stands; what has not is still ocean floor and may founder. Era-native
/// (the same constant the uplift's metamorphism clamps to) rather than a
/// number borrowed from the erosion table, and it widens the subductable
/// band from the softest 41% of the vent spectrum to the softest 59%.
const ARREST_GRADE: f32 = UPLIFT_HARD;
/// **THE SLAB SHARE** — the share of what its grade says should founder
/// that a LIVE collision edge's basement gives up per tick. 0a's debit hung
/// on the jam EVENT and reached only the loose rock lying on the foreland,
/// which is a dusting: the credit came to ~0.1% of production (S/P measured
/// 0.017 rising to 0.040 over 6000 bench-true ticks even after the
/// basement was added at the jam), the well sat empty, and mean ground kept
/// climbing. A trench is a PLACE: while the edge holds, its column
/// founders. A RATE, not a cliff — the plate bends down over many ticks,
/// and the sink self-limits as what is left at the edge indurates past the
/// metamorphic floor.
const SLAB_SHARE: f32 = 0.060;
/// FLOOD CONTROL (Aaron 2026-08-25: "a tile that wants to inject material
/// into a neighbor doesn't just get to do it — the pressure of all of the
/// cells around it matter"): each cell ACCEPTS at most this much moving
/// material per tick, and inflow is damped by the receiver's own pressure;
/// what a full cell refuses BACKS UP at its source. Liquid rules for fake
/// geology — the boiling-water reading.
const INTAKE_CAP: f32 = 0.15;
const FLOOD_RESIST: f32 = 1.5;
/// Torque → rate conversion for the derived motion, before the clamp.
const RATE_SCALE: f32 = 0.4;

// ── WEATHERING (Aaron 2026-08-25: trim the piles into layers — moisture from
// the uplift's own condensation zones, hardness mattering, sediment spreading
// downhill and consolidating into NEW cells; the planet taking shape). The
// distribution is AGGRESSIVE on purpose (second pass, same day: erosion that
// drains to one neighbour and never fails a consolidated face makes SPIKES,
// not mountains): spoil fans over every downhill neighbour, cliffs of strata
// calve, and sediment keeps flowing until the land lies near flat. ──
/// Moisture everywhere (the air is never bone dry) and the extra a
/// SUBMERGED tile soaks in. (The old conjured orographic term is the
/// WEATHER engine's job now — rain is earned, not assumed.)
const BASE_WET: f32 = 0.12;
const SUBMERGED_WET: f32 = 0.3;
// CARVING (Aaron 2026-08-25: "the erosion isn't carving hard enough --
// channels and valleys"): rainfall accumulates down the steepest-descent
// network into DISCHARGE, the erosion budget scales with its square root,
// and most spoil follows the channel instead of fanning -- streams cut.
const CARVE_GAIN: f32 = 0.30;
/// Discharge above this keeps a tile eroding even when nothing else touches
/// it -- rivers stay live and keep cutting their valleys.
pub const CHANNEL_LIVE: f32 = 8.0;
/// The share of spoil that follows the STEEPEST neighbour (the channel); the
/// rest fans drop-weighted as before.
const CHANNEL_SHARE: f32 = 0.65;
// MOBILE BASE LEVELS (Phase 2 of the erosion-equilibrium program): the
// forcing already oscillates -- the sea is a percentile of the height
// distribution, the ice age locks water into caps and lets it go again, the
// uplift raises columns -- but the erosion answered the new level in the
// same tick everywhere, so nothing was ever left stranded and the whole
// family of inherited disequilibria (terraces, knickzones, incised
// meanders) could not exist. The answer is made FINITE-RATE: a fall in a
// column's outlet is adopted at a CELERITY, and until it is adopted it is
// not erodible potential.
/// The share of the gap between a column's GRADED level and its outlet's
/// that closes in one tick, per unit of root discharge and per unit of the
/// grade the channel has to cut. A trunk river in soft country hears of a
/// fall in a handful of ticks; a rill in metamorphic rock takes hundreds.
const CELERITY_K: f32 = 0.06;
/// The ceiling on that share: no reach, however large its catchment, adopts
/// more than this much of a fresh fall in one tick — the knickzone is
/// always a FRONT, never a teleport to the divide.
const CELERITY_MAX: f32 = 0.5;
/// Below this the chase has ARRIVED and the graded level SNAPS to its
/// control — which is what lets a settled world read a lag of exactly 1.0
/// and erode bit-for-bit as it did before the front existed.
const GRADE_SETTLED: f32 = 1e-4;
// THE RIVERS CARRY (Aaron 2026-08-27, erosion pass 2 of the collision-edge
// plan): fluvial spoil no longer dumps on the ring — it RIDES the stream
// network with a finite capacity ∝ discharge × slope. What the water cannot
// carry deposits at the capacity break (fans at the range fronts, deltas
// and beds at the sea), and a live channel FLUSHES its own standing bed
// into any spare capacity — valleys stay open instead of refilling the
// same tick they are cut. Mass wasting (talus, rockfall) keeps the local
// fan: dry skirts do not ride rivers.
const CARRY_K: f32 = 0.5;
const FLUSH_FRAC: f32 = 0.5;
/// A base-level deposit SPREADS (the 21600-tick sediment towers: a trunk
/// river dumping its whole load on ONE cell out-ran the repose flow's
/// intake caps — and where ice damped the drain, the growing altitude froze
/// the summit harder and the ratchet climbed): the receiving cell keeps
/// this share, the rest fans drop-weighted to its lower ICE-FREE
/// neighbours — deltas at the sea, moraine fans at a glacier's gate,
/// braids at a capacity break.
///
/// RELAXED 0.35 → 0.60 in 0b: the fan is real geology, but the SIZE of it
/// was set to kill towers, and what actually stops a tower is the intake
/// cap plus the suspension ledger underneath this (a keeper) — every share
/// here, the kept one included, still settles at ≤ INTAKE_CAP and suspends
/// the refusal. So the river now leaves the majority of its load where it
/// drops it and still braids the rest. Verified over 36,000 bench-true
/// ticks: needles 0 at all 11 samples, max prominence 0.94.
const DELTA_KEEP: f32 = 0.60;
// HARD PROVINCES (Aaron: ranges, not speckle): each vent's characteristic
// grade draws from a SMOOTH seeded field over the sphere, so neighbouring
// vents pour kindred rock and hardness contrast arrives in belt-sized
// provinces; a small per-vent jitter keeps a province from being uniform.
const PROVINCE_FREQ: f32 = 4.0;
const PROVINCE_JITTER: f32 = 0.1;
/// The vent-output SPECTRUM (Aaron: "a spectrum of what materials are
/// emitted, like a stream -- not tick to tick"): each vent's characteristic
/// hardness is a seeded draw over this range, drifting slowly with its
/// cumulative output. 1.0 is reference; harder erodes slower.
const VENT_HARD_MIN: f32 = 0.55;
const VENT_HARD_SPAN: f32 = 1.1;
const VENT_HARD_DRIFT: f32 = 0.25;
/// How much material a fully-wet, fully-sloped, softest-material tile sheds
/// per tick, and the slope past which erosion stops caring (a cliff is a
/// cliff).
const ERODE_RATE: f32 = 0.11;
const SLOPE_CAP: f32 = 1.2;
/// The share of moving sediment lost in transit — ZERO (Aaron 2026-08-25:
/// planet scale, gigatons — the ledger loses nothing in motion); everything
/// spreads over EVERY downhill neighbour, weighted by drop (a
/// single-receiver drain carves channels and towers; water fans out).
const CARRY_LOSS: f32 = 0.0;
/// HARDNESS factors — how fast each material of a column sheds, top-down:
/// loose sediment washes at the drop of a rain, young volcanic rock erodes
/// freely, a consolidated stratum resists, the plate's base barely weathers.
const HARD_SEDIMENT: f32 = 1.3;
const HARD_ROCK: f32 = 1.0;
const HARD_STRATA: f32 = 0.3;
const HARD_BASE: f32 = 0.06;
/// DRY mass wasting — atmospheric erosion needs no water: a slope past the
/// talus angle sheds its excess regardless of moisture, which is what keeps
/// the heat-seam needles from growing into space.
const TALUS_SLOPE: f32 = 0.8;
const WASTE_RATE: f32 = 0.5;
/// ROCKFALL: past this local relief even CONSOLIDATED strata fail — cliffs
/// calve at this rate of the EXCESS, unbounded per tick: failure is
/// proportional to the relief, so intake can never outrun shedding (a flat
/// per-tick cap let a convergent boundary stack columns into a tower faster
/// than the cap could calve them). The rate < 1 keeps it staged decay toward
/// the threshold, never a teleport.
const STRATA_CLIFF: f32 = 1.4;
const CLIFF_RATE: f32 = 0.7;
// (An ABSOLUTE height ceiling was tried here and rejected before it shipped —
// Aaron: it would sand every range down to one worldwide plateau at the cap.
// The crumble limit is the DIFFERENCE FROM NEIGHBOURS, nothing else: a broad
// massif still falls, because its rim calves outward first, which hands its
// interior the very drops that fail it next — the collapse eats inward.)
/// SEDIMENT KEEPS MOVING: after deposition, sediment FLOWS downhill again —
/// this share per pass, this many passes per tick, stopping only where the
/// remaining drop is under the repose slope. Water-borne spoil travels far:
/// aprons become hills, basins fill into plains.
const SED_REPOSE: f32 = 0.12;
const SED_FLOW_FRAC: f32 = 0.6;
const SED_FLOW_PASSES: usize = 2;
/// Sediment that has piled deep enough CONSOLIDATES into a stratum — a NEW
/// cell in the stack — keeping part of its height; it settles where the land
/// lies flat or under water (marine beds), never on a steep face.
const SED_FORM: f32 = 0.5;
const SED_KEEP: f32 = 1.0;
const SED_FLAT: f32 = 0.15;
// ── MARINE COMPACTION (Aaron 2026-08-26: "as the erosion runoffs drop
// sediment into the ocean beds we can add hardness to those cells… the
// effect of the water is to provide the compaction… tuning up the hardness
// of ocean bed cells on an ongoing basis, which increases its weight" — the
// counterweight to the upwells: soft spoil filters off the land and
// indurates under the sea). The grade NEVER relaxes: land that later emerges
// carries its compacted bed into the collisions. ──
/// How much of the remaining headroom to the cap a fully-pressed column
/// gains per tick — asymptotic: an old bed approaches the cap, never jumps.
const MARINE_COMPACT_RATE: f32 = 0.004;
/// Standing-water depth (tile-widths) at which the compaction pressure
/// saturates — deeper water presses no harder than this.
const MARINE_DEPTH_CAP: f32 = 3.0;
/// The hardest water-compacted grade a bed can reach (1.0 = uncompacted).
/// (pub: the stack view normalizes its compaction shading against this cap.)
pub const MARINE_HARD_CAP: f32 = 2.2;
/// Fresh sediment LYING on a drowned column multiplies its compaction — the
/// spoil is the very material being pressed into the bed.
const MARINE_SED_BOOST: f32 = 3.0;
/// A stratum FORMING under the sea is a compaction event in itself: the
/// marine bed jumps this much of its remaining headroom at once.
const MARINE_FORM_BUMP: f32 = 0.08;

// ── THE ICE AGE RUNNER (Aaron 2026-08-26: finish the heat portion — the
// planet's overall temperature wanders over geological time, ice caps grow
// and shrink with it, and ice is "both more static and yet more aggressive
// about its erosive effects"). The runner is DETERMINISTIC: two slow seeded
// sines over the tick count around the dial's baseline. ──
/// The oscillation's two periods, in ticks — offset primes so the beat
/// pattern never repeats on a short cycle: real glacials and interglacials.
// ── THE WEATHER ENGINE (Aaron 2026-08-28: "we should be moving air through
// layers… there's no mountains blocking moisture to interiors, there's no
// motion consideration or concentration of moisture into pressure zones and
// weather systems") — three ATMOSPHERIC layers of in-flight moisture over
// the stack. The sea evaporates into the boundary layer; each layer ADVECTS
// along its own wind (banded circulation: trades, westerlies, polar
// easterlies, a seeded meander so no band is compass-drawn); rising ground
// LIFTS and RAINS the windward wall and SHADOWS the interior; convergence
// zones (ITCZ, the polar fronts) concentrate and rain, the subtropical
// divergences subside and dry — deserts and plains are where the rain never
// goes. RAIN, not conjured moisture, drives the weathering and seeds the
// streams. Moisture is a FIELD (it gates erosion budgets); it never adds
// material to a column. ──
pub const AIR_LAYERS: usize = 3;
/// Each deck's altitude above the sea (tile-widths): boundary, condensation,
/// high. Ground taller than a deck BLOCKS that layer's advection — the wall
/// rains, the interior dries.
pub const DECK_ALT: [f32; 3] = [1.2, 2.6, 4.2];
/// Evaporation into the boundary layer: open sea per tick (×local warmth),
/// and the land's recycle share of its standing rain.
const EVAP_SEA: f32 = 0.020;
const EVAP_LAND: f32 = 0.25;
/// The share of a layer's moisture that advects downwind per tick — upper
/// decks run faster.
const ADV_FRAC: [f32; 3] = [0.35, 0.50, 0.65];
/// Orographic machinery: the rise (tile-widths) that counts as a wall, the
/// share of arriving moisture that RAINS on the windward wall, and the
/// share that lifts to the next deck instead of crossing.
const OROG_RISE: f32 = 0.35;
const OROG_RAIN: f32 = 0.45;
const OROG_LIFT: f32 = 0.35;
/// Convergence: the share of standing moisture a convergence zone rains per
/// tick (×conv), the share divergence subsides a deck down, and the share
/// warm ground convects a deck up.
const CONV_RAIN: f32 = 0.10;
const SUBSIDE: f32 = 0.06;
const CONVECT_LIFT: f32 = 0.08;
/// Saturation: a deck holds only so much; the excess rains where it stands.
const AIR_CAP: [f32; 3] = [1.6, 1.2, 0.9];
const SAT_RAIN: f32 = 0.5;
/// THE DRIZZLE (Aaron 2026-08-28, the unreachable green target): every deck
/// precipitates this share of what it carries as it passes, so the upper
/// decks WATER THE INTERIORS. Without it, half the land measured rain
/// 0.000 forever — no thirst could green ground the sky never touched, and
/// the vegetation dial saturated near 20% whatever it asked.
const DECK_DRIZZLE: [f32; 3] = [0.004, 0.008, 0.010];
/// The rain ledger's EMA fold — what the erosion drinks and the map tints.
const RAIN_BLEND: f32 = 0.05;
/// How strongly the rain EMA feeds the erosion budget's wetness.
const RAIN_ERODE: f32 = 6.0;
/// How strongly the rain EMA seeds the stream network's rainfall — scaled
/// so a well-watered tile seeds like the old conjured moisture did, and
/// CHANNEL_LIVE keeps its calibration.
const RAIN_DISCH: f32 = 10.0;
/// The seeded meander rotating the banded winds (radians at full field).
const WIND_MEANDER: f32 = 0.25;
/// The boundary-layer moisture that reads as FULL on the display scale.
const AIR_VIS: f32 = 0.8;
// ── VEGETATION (Aaron 2026-08-28: "the greening and the assumed resistance
// to erosion that would come from it"): COVER grows where the rain sustains
// it (measured land-rain p90 ≈ 0.016, p99 ≈ 0.054 — full cover near the
// wet tail) and dies back to desert where the rain stops; standing cover
// BINDS the soil, damping the erosion budget — the Langbein-Schumm shape:
// deserts starve of rain, forests hold their ground, and the semi-arid
// middle erodes hardest. The sea drowns cover. ──
/// The sustained rain EMA that grows FULL cover.
const VEG_RAIN_FULL: f32 = 0.03;
/// Cover's approach rate toward its rain-set target, per tick — slow:
/// forests take geological beats to establish, and to die.
const VEG_GROW: f32 = 0.012;
/// How much of the erosion budget full cover holds back.
const VEG_SHIELD: f32 = 0.65;
/// THE GREEN TARGET (Aaron 2026-08-28: "a vegetation target… a slider for
/// tuning how much greening we get"): the dial's share of standing land the
/// flora should hold. The stock's THIRST adapts toward it — below target
/// the flora grows more drought-tolerant (full cover on less rain), above
/// it thirstier — so the green expands or contracts along the rain
/// gradient, still under the weather's bands, never painted on.
const DEFAULT_VEG_TARGET: f32 = 0.70;
/// Cover above this counts as GREENED land for the share the dial steers.
/// Public because the DISPLAY anchors to it too: a tile that COUNTS as green
/// must READ green (Aaron 2026-08-28 — 70% on the dial looked like bare tan),
/// so the paint ramp keys off the same threshold the metric does.
pub const GREEN_COVER: f32 = 0.35;
/// The thirst's proportional gain (per-tick step clamped ±2%) and the
/// working range — flora can be only so hardy, and only so thirsty.
const VEG_ADAPT: f32 = 0.5;
const VEG_THIRST_MIN: f32 = 0.15;
const VEG_THIRST_MAX: f32 = 4.0;

const ICE_AGE_PERIODS: [f32; 2] = [830.0, 2210.0];
/// Each sine's amplitude on the 0..1 climate scale — deep enough that
/// glacials drag the caps toward the mid-latitudes, shallow enough that the
/// default baseline never snowballs the equator.
const ICE_AGE_AMPS: [f32; 2] = [0.10, 0.07];
/// Where freezing begins on the LOCAL temperature scale. At the default
/// baseline (0.5) the equator (no latitude penalty) stays above this through
/// the deepest default glacial (0.33): permanent caps, wandering margins,
/// no accidental snowball.
const FREEZE_POINT: f32 = 0.30;
/// How hard latitude cools a tile: subtracted as `ICE_LAT · y⁴` — the quartic
/// keeps a broad temperate belt and tight polar caps that REACH toward the
/// equator as the planet cools. Anchored at both ends: at the DEFAULT
/// baseline (0.5) the poles stay frozen through the warmest interglacial
/// (0.67 − 0.50 < freeze), while the dial at FULL heat clears every cap even
/// in the deepest oscillation dip (0.83 − 0.50 > freeze) — a hothouse world
/// is capless by the dial, never by an edit.
const ICE_LAT: f32 = 0.50;
/// Altitude lapse: height above the sea line (tile-widths, scaled) cools the
/// tile — high ranges carry glaciers even off the poles.
const ICE_ALT: f32 = 0.10;
/// Full-coldness cap thickness (tile-width units) and the approach rates —
/// growth is slow (ice is patient), melt a little faster (a warm spell bites).
const ICE_THICK: f32 = 0.9;
const ICE_GROW: f32 = 0.05;
const ICE_MELT: f32 = 0.10;
/// The caps can lock at most this share of the conserved water — the sea
/// thins in an ice age but never vanishes: ice GROWS FROM the water, so a
/// dry world grows none.
const ICE_MAX_LOCK: f32 = 0.85;
/// Ice at least this thick erodes as a GLACIER and freezes its spoil in
/// place; thicker than ICE_SOLID reads as frozen-through in the views.
const ICE_ERODE_MIN: f32 = 0.05;
pub const ICE_SOLID: f32 = 0.35;
/// The glacial scour: how much harder an iced tile erodes than bare wet rock
/// — aggressive, needing no rain; its till is HELD under the ice (static)
/// and released downhill only on retreat: moraines and outwash for free.
const ICE_SCOUR: f32 = 2.5;
// ── THE CRUST SUB-GROUP (Aaron 2026-08-26): stack layers 2–5 — up to FOUR
// crust layers. L2 = the deep-rock base (BASE_CAP). L3 = the VEIN layer —
// pressure-formed and marine-formed strata, where the ore bodies live. L4 =
// the softer VOLCANIC layer — heat- and max-density-consolidated rock. L5 =
// the SURFACE layer, RESERVED for the future fine-erosion + biosphere pass:
// nothing writes it. Max compression is the ladder: a full slot forces the
// next cell above it (base→L3→L4→loose), the semi-permanent layers. ──
/// Slot capacities, tile-width units.
const L3_CAP: f32 = 1.4;
const L4_CAP: f32 = 1.8;
/// The vein layer resists erosion harder than the volcanic layer above it —
/// buried is the point: players dig through L4 to reach L3.
const HARD_L3_FACTOR: f32 = 0.45;

// ── THE STRATA'S FABRIC (Phase 1 of the erosion-equilibrium program): rock
// is not the same in every direction, and the direction is not authored —
// it is RECORDED from the deformation the era already performs. Deposition
// lays beds FLAT; the orogenic uplift at a live collision edge TILTS them
// and stamps their trend across the front. Erosion then reads the fabric:
// water running ALONG the strike of a dipping bed exploits the soft
// interbeds, water cutting ACROSS one has to saw through the resistant bed.
// This is a RATE MODIFIER on takes that already exist — no new mover, no new
// store, no cap touched (CA9045F5). ──
/// Radians of DIP a bed gains per height-unit of orogenic shortening the
/// uplift resolve actually performs. The quantum already carries the mature
/// edge's leverage, so a long-lived convergence steepens its fabric fast and
/// a passing jam barely tilts anything.
const FOLD_RATE: f32 = 1.0;
/// The fold's ceiling — beds fold, they never invert. Past vertical a bed is
/// overturned, which this era does not model; 60° is as steep as the
/// recorded fabric goes, and it is what a full directional factor means.
const DIP_CAP: f32 = std::f32::consts::FRAC_PI_3;
/// **THE ANISOTROPY BAND.** The directional factor on a consolidated take is
/// `1 ± ANISO_SPAN` — clamped to **[0.5, 1.5]**, a 3× spread between cutting
/// along the strike and cutting across it at full dip. Bounded on purpose: a
/// fabric may never zero a take (a bed that erodes at no rate is a permanent
/// landform, which is an OUTCOME) nor mint one.
const ANISO_SPAN: f32 = 0.5;

// ── DISSOLUTION (Phase 3 of the erosion-equilibrium program): the era's
// chemistry only ever BUILT — veins, marine induration, metamorphism, the
// max-density press. There was no channel that took rock away IN PLACE, and
// karst is the one landscape family mechanical erosion cannot make, because
// the rock leaves from inside and below rather than downslope.
//
// THE SOLUBLE CLASS IS DERIVED, NEVER AUTHORED. It is the VEIN slot (L3) —
// the era's sedimentary/marine bed, the slot the sediment path consolidates
// into — on a column the standing water has indurated past the era's OWN
// carbonate line (`MARINE_CALCITE_HARD`: already the grade at which the
// marine lottery precipitates calcite rather than burying carbon), whose bed
// has NOT been through the orogenic mill (`UPLIFT_HARD`, the metamorphic
// floor). Vent-poured igneous is the L4 slot and never qualifies.
// Metamorphics are excluded ON PURPOSE: marble dissolves in life, but the
// era's single grade axis cannot tell a marble from a hornfels — a smaller
// true claim beats a larger muddy one.
//
// The debit does not join the erosion budget: dissolution is chemistry, not
// stream power, so it runs where slope and the frontier cannot reach — under
// a cover, on flat ground, and on the floor of a closed pit. What it removes
// becomes WATER-BORNE VOLUME in a conserved store with no height. ──
/// Thickness of soluble bed a unit of wet throughput takes per tick. Sized
/// well UNDER the mechanical take on an active slope (a wet consolidated
/// reach sheds ~1e-3/tick): dissolution is a slow channel whose importance
/// is WHERE it acts, never how fast.
const DISSOLVE_RATE: f32 = 0.004;
/// How much of a stream's discharge counts as throughput through the bed —
/// the root, like the carving gain: a trunk river flushes more water past
/// the rock than a rill, but not in proportion to its catchment.
const DISSOLVE_FLOW: f32 = 0.02;
/// **THE WET FLOOR.** Below this throughput nothing dissolves: damp rock is
/// not karst, it takes MOVING water. Subtracted rather than gated, the way
/// the talus and cliff lines are, so the channel opens smoothly — and so
/// that R2's drain condition has a crisp OFF: cut the water and the
/// depression stops deepening.
const DISSOLVE_WET: f32 = 0.02;
/// **THE RATE-CONSTANT BOUND.** No column may lose more than this much bed
/// to solution in one tick, whatever the flood: chemistry is rate-limited by
/// the reaction, not by how much water is available to run the reaction.
const DISSOLVE_MAX: f32 = 0.01;

// ── MATERIAL VEINS (Aaron 2026-08-26; canon A4/68DB74FA: veins by
// tectonic-cycle distillation, hosted in the rock that made them — never
// placed). A vein NUCLEATES where the vein layer forms under real pressure
// (under the ridges), or where a rolled STATIC SITE's layer finally arrives
// (the worldwide distribution floor), or on the marine path (coal/calcite —
// the sedimentary sims, no biosphere modelled). It then SPREADS through the
// forming layer into a multi-cell body. ──
/// One mineable vein kind, built FROM THE REGISTRY (compounds.json) — every
/// natural, harvestable compound is a vein (Aaron 2026-08-26: ALL materials,
/// not a curated few). The label is the EXTRACTED element where the registry
/// names one (bauxite digs as Al); gemstones carry their own name. Pearl is
/// the ONE exclusion — biological, waiting on the biosphere pass.
pub struct VeinKind {
    /// The registry link (compounds.json id).
    pub compound: u8,
    /// The billboard's text: `extracted_element`, or the compound name.
    pub label: String,
    pub ink: [f32; 3],
    /// Sedimentary kinds form on the MARINE path (evaporites, calcite, coal,
    /// opal); everything else forms under pressure. The registry carries no
    /// genesis field yet — this id set is the stand-in until one is ruled.
    pub marine: bool,
    /// Draw weight — the accretion budget's element abundance where the
    /// budget names the element (abundance.json), a trace default where it
    /// does not (the rare metals), gemstones scaled far down again, and the
    /// registry's demand tier boosting the bulk-crafting class.
    pub weight: f32,
    /// The registry's `vein_demand` tier — `true` = the bulk-crafting class.
    pub demand_high: bool,
    pub gem: bool,
}

/// The registries the vein table is built from — shipped with the crate like
/// the orchestration script.
const COMPOUNDS_JSON: &str = include_str!("../../../../content/data/compounds.json");
const ABUNDANCE_JSON: &str = include_str!("../../../../content/data/abundance.json");
/// Marine-genesis compound ids (see `VeinKind::marine`).
const MARINE_COMPOUNDS: [u8; 6] = [10, 11, 12, 25, 29, 49];
/// Pearl — biological; excluded until the biosphere pass exists.
const PEARL: u8 = 51;
/// Trace weight for elements the accretion budget does not name, and the
/// further scale on gemstones.
const TRACE_WEIGHT: f32 = 0.01;
const GEM_SCALE: f32 = 0.05;

/// THE VEIN TABLE — parsed once from the registry.
pub fn vein_kinds() -> &'static [VeinKind] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<Vec<VeinKind>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let comps: serde_json::Value =
            serde_json::from_str(COMPOUNDS_JSON).expect("compounds.json parses");
        let abund: serde_json::Value =
            serde_json::from_str(ABUNDANCE_JSON).expect("abundance.json parses");
        let weight_of = |sym: &str| -> f32 {
            abund["elements"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|e| e["symbol"].as_str() == Some(sym))
                .and_then(|e| e["weight"].as_f64())
                .map(|w| w as f32)
                .unwrap_or(TRACE_WEIGHT)
        };
        // Signature inks by compound id; anything unlisted gets a stable
        // golden-ratio hue so every kind stays tellable on the x-ray.
        let ink_of = |id: u8| -> [f32; 3] {
            match id {
                22 => [0.95, 0.78, 0.22], // gold
                21 => [0.80, 0.84, 0.88], // platinum
                20 => [0.75, 0.75, 0.80], // silver (argentite)
                15 => [0.62, 0.28, 0.16], // hematite rust
                18 => [0.22, 0.62, 0.50], // copper verdigris
                24 => [0.82, 0.62, 0.50], // bauxite
                25 => [0.10, 0.09, 0.09], // coal
                12 => [0.88, 0.86, 0.78], // calcite chalk
                23 => [0.62, 0.78, 0.22], // uraninite
                31 => [0.92, 0.85, 0.25], // sulfur
                26 => [0.85, 0.85, 0.88], // quartz
                10 => [0.92, 0.84, 0.84], // halite
                43 => [0.92, 0.96, 1.00], // diamond
                44 => [0.20, 0.75, 0.45], // emerald
                45 => [0.85, 0.15, 0.25], // ruby
                46 => [0.20, 0.35, 0.85], // sapphire
                47 => [0.62, 0.40, 0.80], // amethyst
                48 => [0.90, 0.70, 0.30], // topaz
                49 => [0.86, 0.82, 0.90], // opal
                _ => {
                    let h = (f32::from(id) * 0.618_034).fract() * std::f32::consts::TAU;
                    [
                        0.5 + 0.35 * h.cos(),
                        0.5 + 0.35 * (h + 2.094).cos(),
                        0.5 + 0.35 * (h + 4.188).cos(),
                    ]
                }
            }
        };
        comps["compounds"]
            .as_array()
            .expect("compound rows")
            .iter()
            .filter(|x| {
                x["natural"].as_bool() == Some(true)
                    && x["harvestable"].as_bool() == Some(true)
                    && x["id"].as_u64() != Some(u64::from(PEARL))
            })
            .map(|x| {
                let id = x["id"].as_u64().expect("compound id") as u8;
                let gem = x["category"].as_str() == Some("gemstone");
                let label = x["extracted_element"]
                    .as_str()
                    .unwrap_or_else(|| x["name"].as_str().expect("compound name"))
                    .to_string();
                let demand_high = x["vein_demand"].as_str() == Some("high");
                let mut w = weight_of(&label) * if gem { GEM_SCALE } else { 1.0 };
                if demand_high {
                    w *= DEMAND_BOOST;
                }
                VeinKind {
                    compound: id,
                    ink: ink_of(id),
                    marine: MARINE_COMPOUNDS.contains(&id),
                    weight: w.max(TRACE_WEIGHT * GEM_SCALE),
                    demand_high,
                    gem,
                    label,
                }
            })
            .collect()
    })
}

/// The table index for a compound id — tests and tools ask by registry id.
pub fn vein_index_of(compound: u8) -> Option<u8> {
    vein_kinds()
        .iter()
        .position(|k| k.compound == compound)
        .map(|i| i as u8)
}

/// A weighted seeded draw over the table, restricted by a genesis predicate.
fn draw_where(h: u64, keep: impl Fn(&VeinKind) -> bool) -> u8 {
    let kinds = vein_kinds();
    let total: f32 = kinds.iter().filter(|k| keep(k)).map(|k| k.weight).sum();
    let mut pick = (h % 10_000) as f32 / 10_000.0 * total;
    for (i, k) in kinds.iter().enumerate() {
        if !keep(k) {
            continue;
        }
        pick -= k.weight;
        if pick <= 0.0 {
            return i as u8;
        }
    }
    0
}

/// The two genesis draws: pressure takes the non-marine side; a SOFT marine
/// bed draws the marine side MINUS calcite — the deep-compacted branch owns
/// the carbonate, so soft basins bury coal, evaporites and opal.
fn draw_kind(h: u64, marine: bool) -> u8 {
    if marine {
        draw_where(h, |k| k.marine && k.compound != 12)
    } else {
        draw_where(h, |k| !k.marine)
    }
}

/// The vein layer must be at least this tall to host (or keep) a vein.
const VEIN_L3_MIN: f32 = 0.3;
/// Pressure nucleation is a LOTTERY on the forming tile — sparse, seeded.
const VEIN_LOTTERY: u64 = 29;
/// A node's SIZE BUDGET (cells): most bodies are a handful, every seventh is
/// massive — and NOTHING exceeds the cap: a vein is a concentration under a
/// ridge, never a continent (Aaron 2026-08-26: "a seven-thousand-mile field
/// of gold is not a vein").
const NODE_BASE: u16 = 2;
const NODE_SPAN: u16 = 5;
const NODE_MASSIVE_EVERY: u64 = 7;
const NODE_MASSIVE_MUL: u16 = 3;
const NODE_MAX: u16 = 20;
/// New nodes keep this many tiles from every existing node's centre — the
/// discreteness that reads as DISTRIBUTION instead of carpet.
const VEIN_NODE_SEP_TILES: f32 = 6.0;
/// WATER IN-FALL (Aaron 2026-08-26: as material and landmass grow, water
/// falls in to hold the planet near the target coverage — by tick 1200 the
/// dry run had fallen under 30% water). Per tick, while coverage stands
/// BELOW the target, volume arrives proportional to the deficit; nothing is
/// ever taken back — land growth is the counterweight. Rising seas
/// reclassify land → shelf → bed and may drown vein fields: expected
/// results, never misfires.
const INFALL_GAIN: f32 = 0.006;
// ── THE OCEAN'S OWN HEAT (Aaron 2026-08-26, completing the banked
// three-layer water spec F4B8B7C2): the ocean tracks temperature by DEPTH.
// The SURFACE band is per-tile with THERMAL INERTIA (it lags the air — sea
// ice forms later and melts later than land ice). The DEEP ocean is the
// ratified OPTIMIZATION: one well-mixed global reservoir — a single scalar,
// O(1), never a per-tile ledger. The SHALLOW band is DERIVED (the mix of the
// two), stored nowhere. ──
/// How fast the surface band chases the air each tick (0..1) — the inertia.
const SST_CHASE: f32 = 0.15;
/// How fast the deep reservoir creeps toward the mean surface temp — the
/// centuries-slow overturn.
const DEEP_CHASE: f32 = 0.004;
/// The deep reservoir's clamp: never boiling, never frozen solid — the
/// abyssal 4°C analogue on the 0..1 scale.
const DEEP_MIN: f32 = 0.20;
const DEEP_MAX: f32 = 0.60;
/// SNOWFALL IS THE LIMIT (the 3600-tick ice ball, Aaron 2026-08-26): a cap
/// grows only as fast as weather DELIVERS water — growth scales with the
/// moisture field, floored so polar deserts still accumulate, slowly. A dry
/// interior range cannot teleport the ocean onto its peaks.
const ICE_SNOW_FLOOR: f32 = 0.06;
// ── THE SNOWBALL ESCAPE (the 3600/4800 ice balls, Aaron 2026-08-26): two
// missing pieces of real physics, so a frozen world is an EPOCH, never a
// terminal state. ──
/// VOLCANIC GREENHOUSE: a frozen surface stops weathering, volcanic gas
/// accumulates, the planet warms until the ice hands the ocean back — a slow
/// build proportional to the frozen share against a slow decay. At a full
/// ice ball the equilibrium lift is BUILD/DECAY ≈ +0.37: enough to break any
/// glacial, on a ~250-tick escape.
const GH_BUILD: f32 = 0.003;
const GH_DECAY: f32 = 0.004;
/// THE OCEAN'S CLAIM: while a coverage deficit stands, the ice ration
/// SHRINKS by deficit × this — the ocean outranks the freezer in the budget
/// war — and locked ice above the shrunken ration force-melts at the rate
/// below. This is what breaks the melt-refreeze deadlock (cap-melt drained
/// 1.6/tick while equilibrium regrowth reclaimed exactly 1.6/tick, forever).
const ICE_YIELD: f32 = 0.5;
const EXCESS_MELT: f32 = 0.05;
/// THE BOMBARDMENT ENDS (Aaron 2026-08-26: "the water infall needs to
/// eventually stop — continuing to roll water in is drowning the planet"):
/// the planet only ever receives this much water, as mean depth over the
/// whole surface. At the budget the sky closes for good; the caps and the
/// sea then share a BOUNDED total, and the deep-time ice-ball pump dies.
const WATER_BUDGET_DEPTH: f32 = 1.2;
/// MICROCLIMATE (the "very specific strict line, exactly a circle, hard
/// edged" cap boundary): a small seeded per-tile temperature offset breaks
/// the deterministic freeze threshold into a ragged, fjorded fringe several
/// tiles wide. Per-TILE, so the hemispheric-blindness law holds exactly.
const FREEZE_JITTER: f32 = 0.025;

/// THE CAPS ARE BUDGET (Aaron 2026-08-26): water locked in the ice is still
/// the planet's water. A coverage deficit draws on the CAPS FIRST — this
/// share of the locked reserve can melt into the ocean per tick — and only
/// the remaining shortfall imports new water from the sky. Past the point
/// the planet holds enough total water, captured water IS the in-fall.
const CAP_MELT_SHARE: f32 = 0.02;

/// THE BOOTSTRAP HORIZON (Aaron 2026-08-26: ~1200 ticks reach a decent
/// starting world; the run rolls here before the first display) — and the
/// tick at which the RESOURCE GUARANTEE settles anything the era has not
/// delivered on its own.
pub const BOOTSTRAP_TICKS: u64 = 1200;
/// Static-site QUOTAS (Aaron's ruled deviation from no-outcomes, 2026-08-26:
/// this bootstrap era SKIPS generative phases, so certain things are ENSURED
/// deterministically): quotas come from the registry's DEMAND TIER — the
/// bulk-crafting class (`vein_demand: "high"` in compounds.json; iron,
/// copper, tin are EXAMPLES of it, never a list) gets the core count and a
/// boosted draw; everything else the floor; gemstones one apiece.
const SITE_FLOOR: usize = 2;
const SITE_CORE: usize = 5;
const SITE_GEM: usize = 1;
/// How much a demand-high kind's DRAW weight is multiplied — the reason coal
/// actually forms on the marine path instead of losing every draw to salt.
const DEMAND_BOOST: f32 = 8.0;
const VEIN_SITE_SEP_TILES: f32 = 9.0;
/// A marine stratum's deposit draw: deep-compacted beds make CALCITE, softer
/// beds make COAL (the buried carbon — rocks.json's coal seams; no biosphere
/// modelled).
const MARINE_CALCITE_HARD: f32 = 1.5;
const MARINE_VEIN_LOTTERY: u64 = 61;
/// The vein stream's offset off the molten roll.
const VEIN_STREAM: u64 = 0x94D0_49BB_1331_11EB;

/// THE SUMMER MELT (Aaron 2026-08-26: near-permanent ice was banking
/// snow-mountains along the polar line — every cycle has its summers): an
/// iced tile still passes this share of its normal downhill sediment flow —
/// meltwater carrying till out from under the glacier — so a polar column
/// reaches equilibrium instead of trapping every arrival forever.
const SUMMER_MELT_FLOW: f32 = 0.35;
// (THE GLACIAL-FLOW RAMP IS GONE — 0b. From the 2400-tick towers to here,
// the summer damp scaled back UP with the loose pile's overburden, so "no
// column can outgrow the flow that thickening itself accelerates": an
// OUTCOME written into a rate, and the last of the four result-shapers
// D381F5FE names. The parts that were mechanism STAY — the summer melt
// above, and glacial ROCK entrainment below (a keeper: the ice mills its
// bed and deposits the till downhill). Verified over 36,000 bench-true
// ticks with the ramp removed: needles 0 at all 11 samples, max prominence
// 0.86, and the "no column outgrows its glacier" gate still green — the
// intake-capped drain already outruns any delivery without the ramp's
// help.)

/// **The pipeline's PROCEDURES, in cycle order.** One engine step runs ONE of
/// these ([`Evolution::tick_phase`]); a TICK is the completed cycle (Aaron
/// 2026-08-26: label the running procedure, cluster the tick around the
/// cycle of procedures).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// The ice-age runner: temperature, caps, locked water.
    Climate,
    /// The atmosphere: evaporation, winds, lift, convergence — the RAIN.
    Weather,
    /// Volcanism: upwell pinches and the occasional eruption.
    Upwell,
    /// Hot rock spills outward from its sources.
    Spread,
    /// Flows that meet JAM: collision pressure and its uplift.
    Collide,
    /// The molten push carries material and jams rims.
    Push,
    /// Rarely, a pile consolidates into a stratum.
    Form,
    /// Weathering: rain, streams, glaciers, talus, rockfall.
    Erode,
    /// The presses: marine compaction and the max-density law.
    Compact,
    /// Cold weld, then the frontier rebuild — the cycle closes.
    Weld,
}

/// The cycle, in order. `PHASES.len()` is what a display divides by.
pub const PHASES: [Phase; 10] = [
    Phase::Climate,
    Phase::Weather,
    Phase::Upwell,
    Phase::Spread,
    Phase::Collide,
    Phase::Push,
    Phase::Form,
    Phase::Erode,
    Phase::Compact,
    Phase::Weld,
];

/// One vein BODY: where it nucleated, what it is, how far it has grown and
/// how far its seeded budget lets it grow.
pub struct VeinNode {
    pub center: TileId,
    pub kind: u8,
    pub size: u16,
    budget: u16,
}

/// The state one cycle threads BETWEEN its procedures: the active set, the
/// opening snapshots the closing frontier rebuild diffs against, and the
/// cycle's one water level.
struct TickCarry {
    act: Vec<bool>,
    /// Which tiles stood at forming pressure when Collide snapshotted them —
    /// read by Form two procedures later, BEFORE the decay that follows.
    compressed: Vec<bool>,
    snap_base: Vec<f32>,
    snap_rock: Vec<f32>,
    snap_sed: Vec<f32>,
    snap_strata: Vec<f32>,
    sea: f32,
}

/// **The living state of the evolution era** — everything the ticks mutate,
/// over the (static, already-rolled) map, molten field, crust and plate
/// scheme.
pub struct Evolution {
    /// **The PUSH FIELD** — per TILE, the seams' shove away from the heat
    /// beneath (Aaron 2026-08-25: "the seams should be driving the motion of
    /// the crust, not the edges of the plates"): tangential, zero in the cold
    /// interiors, radiating outward from vents and seam lines. Never averaged
    /// into a rigid body — averaging is exactly what made arrows march over
    /// volcanoes as if the local sources had no say.
    push: Vec<Vec3>,
    /// Per-tile accumulated travel along the push — the LOCAL ratchet: a
    /// tile fires its own one-hex step when its drift crosses a tile of
    /// travel. Cold tiles never accumulate and never fire.
    drift: Vec<f32>,
    /// **The GROUND ledger** — the era's own base height per tile. Starts as
    /// bare thin sea floor EVERYWHERE (Aaron 2026-08-25: the plates phase is
    /// unneeded — the upwelling pushes materials and every piece of land is
    /// something volcanism built); grows only by cold weld and uplift.
    base: Vec<f32>,
    /// Loose volcanic rock per tile, in tile-width units of height.
    rock: Vec<f32>,
    /// The loose rock's HARDNESS per tile — a mass-weighted blend of what the
    /// vents that fed it emit (Aaron 2026-08-25: a SPECTRUM of materials,
    /// like a stream — not tick-to-tick noise). 1.0 is the reference grade;
    /// harder erodes slower.
    rock_hard: Vec<f32>,
    /// Cumulative emission per VENT (indexed as `crust.vents()`), the phase
    /// of each vent's slow output drift along its stream.
    emitted: Vec<f32>,
    /// Each vent's characteristic PROVINCE grade — drawn from a smooth
    /// seeded field over the sphere (rebuilt beside `emitted`), so kindred
    /// vents pour kindred rock: hardness in belts, not speckle.
    vent_grade: Vec<f32>,
    /// Last tick's DISCHARGE per tile — rainfall accumulated down the
    /// steepest-descent network: the streams. State for the carving and for
    /// a future river view.
    discharge: Vec<f32>,
    /// The CONSERVED WATER VOLUME (area-weighted height units). Set when the
    /// water dial pours; the sea LEVEL then solves from it every tick — as
    /// the upwelling builds land, the same water stands higher on what
    /// remains, and coverage is an OUTPUT, not the dial.
    water_volume: f32,
    /// Loose SEDIMENT per tile — eroded material in transit, the softest
    /// thing on any column.
    sediment: Vec<f32>,
    /// **THE ICE ledger** per tile (tile-width units): the cap standing on
    /// this column. Grows toward the local climate's equilibrium, melts past
    /// it; locks conserved water out of the sea while it stands.
    ice: Vec<f32>,
    /// The dial's climate BASELINE (0..1); the runner oscillates around it.
    climate_base: f32,
    /// The LIVE planet temperature this tick — baseline + the ice-age sines.
    temp: f32,
    /// Area-weighted water volume locked in the caps — subtracted from the
    /// conserved volume when the sea resolves: ice ages DROP the sea.
    ice_locked: f32,
    /// **The MARINE COMPACTION grade** per tile (1.0 = uncompacted, up to
    /// MARINE_HARD_CAP): the standing water's ongoing press on a drowned
    /// column's consolidated stack. Grows while submerged, faster under
    /// fresh sediment; NEVER relaxes — an emerged bed keeps its grade, and
    /// its disposition in the collisions reads from it (the weight).
    bed_hard: Vec<f32>,
    /// The crust sub-group's formed slots per tile: L3 (the vein layer —
    /// pressure- and marine-formed) and L4 (the volcanic layer). L2 is
    /// `base`; L5 is reserved and has NO ledger on purpose — nothing may
    /// write it until the fine-erosion/biosphere pass exists.
    l3_h: Vec<f32>,
    l4_h: Vec<f32>,
    /// The formed slots' GRADES — the hardness a stratum inherited from the
    /// material that consolidated into it, mass-blended like `rock_hard`.
    /// This is what makes the spectrum PERMANENT: hard-vent country forms
    /// hard strata and stands as massifs while soft country washes out.
    l3_hard: Vec<f32>,
    l4_hard: Vec<f32>,
    /// **THE FABRIC** — the ATTITUDE of the consolidated beds, recorded from
    /// the deformation the era performs (never authored). `strike` is the
    /// column's bedding TREND: an azimuth in the tile's own (east, north)
    /// tangent frame, wrapped to `[0, π)` because a strike is an undirected
    /// LINE — which is why the two sides of a front, whose convergences are
    /// anti-parallel, record the same trend. ONE trend per column: a single
    /// orogeny folds the whole stack about one axis.
    strike: Vec<f32>,
    /// Each formed slot's DIP (radians, 0..DIP_CAP) — how far that slot's
    /// beds have been rotated out of horizontal. Per slot, beside the grades,
    /// because the slots deform together but are BEDDED apart: mass settling
    /// flat into L4 dilutes L4's dip while the L3 beneath keeps the tilt it
    /// earned. Zero everywhere is the null fabric — a column that has never
    /// been shortened erodes exactly as it did before the fabric existed.
    l3_dip: Vec<f32>,
    l4_dip: Vec<f32>,
    /// **THE GRADED LEVEL** (Phase 2) — per tile, the outlet elevation this
    /// column has actually ADJUSTED to, which is not always the outlet
    /// elevation it HAS. A CONTROL variable, never a store: it holds no
    /// material, moves none, and the three-store ledger never sees it.
    /// Every tick it chases its own downstream control (`outlet_level`) —
    /// instantly upward, at `CELERITY_K` downward — and the gap between the
    /// two is exactly the share of a base-level fall the column has not yet
    /// answered. `f32::MIN` is "has never met its outlet": the first upward
    /// chase adopts it exactly, which is how a world reset, and a planet
    /// captured before this field existed, both stand up already at grade.
    graded: Vec<f32>,
    /// **SUSPENDED LOAD** per tile, in area-weighted VOLUME units — carried
    /// sediment the ground REFUSED (the 32400-tick relapse: spread fans and
    /// wake laws only slowed the towers, because the carry was the one
    /// transport exempt from Aaron's flood-control law — a trunk river
    /// could still land multi-unit loads around a constriction while every
    /// drain is rate-limited). A cell settles at most INTAKE_CAP of carried
    /// load per tick; the refusal stays here — water-borne, NO height, no
    /// act — and re-enters the flow at this tile next tick. Talus
    /// (0.5·over-slope) now always outruns delivery past slope ~1.1: no
    /// needle can be manufactured.
    suspend: Vec<f32>,
    /// **DISSOLVED LOAD** per tile, in area-weighted VOLUME units — WATER'S
    /// SECOND DENOMINATION (Phase 3). Sediment is the water cycle's currency
    /// (CA9045F5); this is the same currency in solution. Soluble bed the
    /// throughput has taken IN PLACE lives here: it carries NO height, joins
    /// no column, and is invisible to every slope, wake and relief law in the
    /// era — the only thing that can move it is the water, on the one stream
    /// tree the carry sweep already rides. It leaves solution ONLY where the
    /// condition reverses (a drying closed basin, or warm shallow water), and
    /// every one of those deliveries is capped by the same per-cell intake
    /// budget the settle uses. Being a holding state is the whole point: an
    /// over-saturated basin precipitates its cap and KEEPS the rest, so no
    /// delivery can ever out-run the drains (B390DA57).
    dissolved: Vec<f32>,
    /// **THE COLLISION-EDGE LEDGER** — the crust-side corollary of the
    /// molten seams. `edge_flux` collects this cycle's jam flux (Spread's
    /// refused arrivals, Push's stalled piles); the Weld arm folds it into
    /// `edge` (an EMA — the tracked intensity) and runs `edge_age`: ticks of
    /// consecutive persistence while the edge holds ≥ EDGE_LIVE, zero when
    /// it dies. SILENT state — tracking moves no material.
    edge_flux: Vec<f32>,
    edge: Vec<f32>,
    edge_age: Vec<u32>,
    /// **THE SUBDUCTION WELL** — ONE aggregate scalar in area-weighted
    /// VOLUME units: what the crust's convergences have sunk and the molten
    /// layer has not yet spent back. No provenance, no routing, no courier
    /// (882D1B83 was fundamental) — the planet is the well, one number in
    /// and one number out (064F3B58). This is the two-layer law's single
    /// coupling made real (C3B39430: only MATERIAL couples them): the crust
    /// seams debit DOWN at the jams and at the basal ceiling, the molten
    /// seams spend back UP at the vents. The CYCLE ORDER is the
    /// count-then-spend barrier — Upwell, the one spender, runs before
    /// every creditor, so a tick can never spend what it has not yet sunk.
    well: f32,
    /// Cumulative volume the well has ever taken, and how many times the
    /// basal ceiling has fired — the instruments 064F3B58's lesson demands:
    /// a bound that "works" must be caught actually firing.
    sunk: f32,
    delaminations: u64,
    /// Per-tile vein: 0 = none, else `1 + index` into [`vein_kinds`].
    vein: Vec<u8>,
    /// Per-tile node membership: 0 = none, else `1 + index` into
    /// `vein_nodes` — what bounds a body to its budget.
    vein_node_of: Vec<u16>,
    /// Every nucleated vein NODE — centre, kind, grown size, seeded budget.
    vein_nodes: Vec<VeinNode>,
    /// The rolled STATIC sites: (tile, kind) — dormant until the vein layer
    /// arrives, or until the bootstrap horizon ENSURES them.
    vein_sites: Vec<(TileId, u8)>,
    /// Whether the bootstrap-horizon resource guarantee has run.
    resources_ensured: bool,
    /// The WATER COVERAGE TARGET (0..1 share of surface under water) the
    /// in-fall pursues. The dial sets it; default 70% water.
    water_target: f32,
    /// The ocean SURFACE band's temperature per tile (0..1, the climate
    /// scale) — meaningful where water stands, tracking the air with lag;
    /// snapped to the air over dry ground so shorelines stay continuous.
    sst: Vec<f32>,
    /// THE DEEP OCEAN — one well-mixed global reservoir (the ratified
    /// optimization: a scalar, never a ledger).
    deep_temp: f32,
    /// The volcanic-greenhouse lift on the live temperature — builds while
    /// the surface is frozen, decays as it clears: the snowball escape.
    greenhouse: f32,
    /// The per-tile MICROCLIMATE offset (±FREEZE_JITTER, seeded once) — what
    /// makes the freeze line ragged instead of a compass-drawn circle.
    micro: Vec<f32>,
    /// Collision PRESSURE per tile — raised where plates push, bled off
    /// slowly, the era's main mountain builder.
    pressure: Vec<f32>,
    /// How many interior vacancies the safety-net heal has ever filled — the
    /// rotated-image step should leave it at ~ZERO; a growing count means the
    /// relocation is churning again (the icosa-line channel).
    heals: u64,
    /// How many eruptions the era has fired — the occasional big-flow ticks.
    eruptions: u64,
    /// How many plate STEPS have fired — the rare edge-advance events.
    steps: u64,
    /// THE ACTIVE FRONTIER: the tiles whose material changed last tick, plus
    /// their ring — the ONLY tiles the next tick's material phases process.
    /// Everything else stands perfectly still (Aaron 2026-08-25: a tick moves
    /// hundreds of tiles of a 93K world, near-imperceptible).
    frontier: Vec<bool>,
    /// How many tiles' material actually changed last tick — the honest
    /// imperceptibility readout.
    changed: usize,
    /// The near-ground MOISTURE display field — the boundary air layer on
    /// the visibility scale. What the haze shows, what the snowfall is
    /// limited by. (The weathering drinks RAIN now, not this.)
    moist: Vec<f32>,
    /// **THE AIR** — in-flight moisture per atmospheric layer per tile
    /// (boundary, condensation, high). Evaporation fills it, the winds move
    /// it, lift raises it, rain empties it. A field over the world, never
    /// column material.
    air: Vec<Vec<f32>>,
    /// **THE RAIN ledger** — precipitation per tile, EMA-folded: what the
    /// erosion budget drinks, what seeds the streams, what tints the land
    /// green or leaves it desert.
    rain: Vec<f32>,
    /// VEGETATION cover per tile (0..1) — grows toward the rain's target,
    /// dies back without it, drowns under the sea; binds the soil against
    /// the erosion budget and paints the continents green.
    veg: Vec<f32>,
    /// The GREEN TARGET the dial pursues (share of standing land greened),
    /// the flora's adapting THIRST (multiplier on the rain a full cover
    /// needs), and the last tick's measured green share — the gauge.
    veg_target: f32,
    veg_thirst: f32,
    green_share: f32,
    /// The WINDS, precomputed per layer at reset (banded circulation +
    /// seeded meander — static like the seams' push): each tile's downwind
    /// neighbour and its speed scale — plus the SECOND-best neighbour and
    /// the primary's share, so the flow splits instead of collapsing into
    /// hex-axis lanes (the cloud streaks).
    wind_to: Vec<Vec<u32>>,
    wind_w: Vec<Vec<f32>>,
    wind_to2: Vec<Vec<u32>>,
    wind_frac: Vec<Vec<f32>>,
    /// Surface CONVERGENCE per tile (+ concentrates and rains, − subsides
    /// and dries) — the analytic band profile with the meander folded in.
    conv: Vec<f32>,
    /// Each cell's area RELATIVE to the mean cell (pentagons ~0.85, ISEA
    /// crease cells a few percent under 1). The ledger's true quantity is
    /// VOLUME = height × area: every transfer between unequal cells converts
    /// by the area ratio, so a pentagon neither hoards nor leaks — the
    /// careful pentagon/crease accounting (Aaron 2026-08-25).
    area: Vec<f32>,
    /// The open cycle's threaded state (`None` between cycles) and which
    /// procedure runs next.
    carry: Option<TickCarry>,
    cursor: u8,
    ticks: u64,
}

impl Evolution {
    /// Start with a bare shell, the plates' motion derived from the seams.
    pub fn new(map: &HexMap, seams: &SeamField) -> Self {
        let mut e = Self {
            push: Vec::new(),
            drift: Vec::new(),
            base: Vec::new(),
            rock: Vec::new(),
            rock_hard: Vec::new(),
            emitted: Vec::new(),
            vent_grade: Vec::new(),
            discharge: Vec::new(),
            water_volume: 0.0,
            sediment: Vec::new(),
            bed_hard: Vec::new(),
            ice: Vec::new(),
            climate_base: 0.5,
            temp: 0.5,
            ice_locked: 0.0,
            carry: None,
            cursor: 0,
            l3_h: Vec::new(),
            l4_h: Vec::new(),
            l3_hard: Vec::new(),
            l4_hard: Vec::new(),
            strike: Vec::new(),
            l3_dip: Vec::new(),
            l4_dip: Vec::new(),
            graded: Vec::new(),
            suspend: Vec::new(),
            dissolved: Vec::new(),
            edge_flux: Vec::new(),
            edge: Vec::new(),
            edge_age: Vec::new(),
            well: 0.0,
            sunk: 0.0,
            delaminations: 0,
            vein: Vec::new(),
            vein_node_of: Vec::new(),
            vein_nodes: Vec::new(),
            vein_sites: Vec::new(),
            resources_ensured: false,
            water_target: 0.70,
            sst: Vec::new(),
            deep_temp: 0.35,
            greenhouse: 0.0,
            micro: Vec::new(),
            pressure: Vec::new(),
            moist: Vec::new(),
            air: Vec::new(),
            rain: Vec::new(),
            veg: Vec::new(),
            veg_target: DEFAULT_VEG_TARGET,
            veg_thirst: 1.0,
            green_share: 0.0,
            wind_to: Vec::new(),
            wind_w: Vec::new(),
            wind_to2: Vec::new(),
            wind_frac: Vec::new(),
            conv: Vec::new(),
            area: Vec::new(),
            heals: 0,
            eruptions: 0,
            steps: 0,
            frontier: Vec::new(),
            changed: 0,
            ticks: 0,
        };
        e.reset(map, seams);
        e
    }

    /// Back to the bare world: a uniform thin sea floor, nothing else (the
    /// plates phase is unneeded — every piece of land is something the
    /// upwelling builds), the clock cleared, the motion re-derived from the
    /// seams.
    pub fn reset(&mut self, map: &HexMap, seams: &SeamField) {
        self.push = vec![Vec3::ZERO; map.len()];
        self.drift = vec![0.0; map.len()];
        self.base = vec![OCEAN_BED_H_FRAC; map.len()];
        self.rock = vec![0.0; map.len()];
        self.rock_hard = vec![1.0; map.len()];
        self.emitted = Vec::new();
        self.vent_grade = Vec::new();
        self.discharge = vec![0.0; map.len()];
        let _ = self.rock.len();
        self.sediment = vec![0.0; map.len()];
        self.bed_hard = vec![1.0; map.len()];
        self.ice = vec![0.0; map.len()];
        self.temp = self.climate_base;
        self.ice_locked = 0.0;
        self.carry = None;
        self.cursor = 0;
        self.sst = vec![self.climate_base; map.len()];
        self.deep_temp = 0.35;
        self.greenhouse = 0.0;
        let mut mr = fastrand::Rng::with_seed(seams.seed().wrapping_add(0x51ED_2701_89AB_CDEF));
        self.micro = (0..map.len())
            .map(|_| (mr.f32() * 2.0 - 1.0) * FREEZE_JITTER)
            .collect();
        self.l3_h = vec![0.0; map.len()];
        self.l4_h = vec![0.0; map.len()];
        self.l3_hard = vec![1.0; map.len()];
        self.l4_hard = vec![1.0; map.len()];
        // A bare world lies FLAT: no shortening has happened, so there is no
        // fabric and every directional factor is exactly 1.0.
        self.strike = vec![0.0; map.len()];
        self.l3_dip = vec![0.0; map.len()];
        self.l4_dip = vec![0.0; map.len()];
        // No column has met its outlet yet: the first stream pass adopts
        // each one's control exactly, so a bare world starts AT grade and
        // the front only ever measures departures from it.
        self.graded = vec![f32::MIN; map.len()];
        self.suspend = vec![0.0; map.len()];
        self.dissolved = vec![0.0; map.len()];
        self.edge_flux = vec![0.0; map.len()];
        self.edge = vec![0.0; map.len()];
        self.edge_age = vec![0; map.len()];
        self.well = 0.0;
        self.sunk = 0.0;
        self.delaminations = 0;
        self.vein = vec![0; map.len()];
        self.vein_node_of = vec![0; map.len()];
        self.vein_nodes.clear();
        // THE STATIC SITES: a seeded, min-separated worldwide scatter of
        // dormant metal sites — the distribution floor Aaron asked for. A
        // site does NOTHING until the evolving vein layer reaches it; the
        // roll places the possibility, the era earns the vein.
        let mut vr = fastrand::Rng::with_seed(seams.seed().wrapping_add(VEIN_STREAM));
        let tile_r = 2.0 / (map.len() as f32).sqrt();
        let sep = (VEIN_SITE_SEP_TILES * 2.0 * tile_r).cos();
        self.vein_sites.clear();
        // QUOTAS: every kind is owed its floor of sites; the core
        // industrials their core count — placed min-separated, seeded.
        for (k, kind) in vein_kinds().iter().enumerate() {
            let want = if kind.demand_high {
                SITE_CORE
            } else if kind.gem {
                SITE_GEM
            } else {
                SITE_FLOOR
            };
            let mut placed = 0usize;
            let mut guard = 0usize;
            while placed < want && guard < want * 200 {
                guard += 1;
                let t = vr.usize(..map.len()) as TileId;
                let d = map.direction(t);
                if self
                    .vein_sites
                    .iter()
                    .all(|(s, _)| d.dot(map.direction(*s)) < sep)
                {
                    self.vein_sites.push((t, k as u8));
                    placed += 1;
                }
            }
        }
        self.resources_ensured = false;
        self.pressure = vec![0.0; map.len()];
        self.moist = vec![0.0; map.len()];
        self.air = vec![vec![0.0; map.len()]; AIR_LAYERS];
        self.rain = vec![0.0; map.len()];
        self.veg = vec![0.0; map.len()];
        self.veg_thirst = 1.0;
        self.green_share = 0.0;
        self.derive_winds(map, seams.seed());
        let mean_area = map.grid().area.iter().sum::<f32>() / map.len().max(1) as f32;
        self.area = map.grid().area.iter().map(|a| a / mean_area).collect();
        self.ticks = 0;
        self.eruptions = 0;
        self.steps = 0;
        // A fresh world stands STILL until something touches it.
        self.frontier = vec![false; map.len()];
        self.changed = 0;
        self.derive_motion(map, seams);
    }

    /// POUR the water: fix the conserved volume so that the CURRENT world is
    /// flooded to `pct` percent of its tiles — the dial's one meaning. From
    /// then on the LEVEL solves from the volume each tick: land the
    /// upwelling builds displaces the sea upward, and coverage becomes an
    /// output the bench can watch instead of a promise the world outgrows.
    pub fn set_water(&mut self, pct: f32) {
        let sea = self.sea_level(pct);
        self.water_volume = self.volume_below(sea);
    }

    /// The water standing below `sea` — area-weighted height units.
    fn volume_below(&self, sea: f32) -> f32 {
        (0..self.rock.len() as TileId)
            .map(|t| {
                let d = sea - self.ground(t);
                if d > 0.0 {
                    d * self.area[t as usize]
                } else {
                    0.0
                }
            })
            .sum()
    }

    /// The sea LEVEL the conserved volume stands at over the CURRENT ground:
    /// a bisection on the monotone fill curve. Zero volume is a dry world.
    pub fn resolve_sea(&self) -> f32 {
        // The caps LOCK water: an ice age stands the same conserved volume
        // lower — shorelines walk out; the melt hands it back.
        let volume = self.water_volume - self.ice_locked;
        if volume <= 0.0 {
            return f32::MIN;
        }
        let mut lo = 0.0f32;
        let mut hi = (0..self.rock.len() as TileId)
            .map(|t| self.ground(t))
            .fold(0.0f32, f32::max)
            + 1.0;
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if self.volume_below(mid) < volume {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    /// The share of tiles standing under the resolved sea — the coverage
    /// readout the bench tracks as the land grows.
    pub fn coverage(&self) -> f32 {
        let sea = self.resolve_sea();
        let n = self.rock.len().max(1);
        (0..n as TileId).filter(|t| self.ground(*t) < sea).count() as f32 / n as f32
    }

    /// **The motion IS the seams' push** (Aaron 2026-08-25): no cell has an
    /// innate velocity — every tile feels a shove AWAY from the heat under it
    /// (the negative gradient of the molten field, so seams, junctions,
    /// plumes and rifts all push), and a plate's spin is the TORQUE of those
    /// shoves integrated over its tiles: axis = Σ p × push(p), rate from the
    /// torque's magnitude, clamped into the slow band. The convection is the
    /// engine; the plates are the skin it drives. Call after `reset` and
    /// whenever the molten field re-rolls.
    pub fn derive_motion(&mut self, map: &HexMap, seams: &SeamField) {
        let n = map.len();
        self.push = (0..n as TileId)
            .map(|t| {
                let p = map.direction(t);
                // The push: downhill on the heat field, tangential at p.
                let mut g = Vec3::ZERO;
                let h0 = seams.heat(t);
                for nb in map.neighbours(t) {
                    let d = map.direction(*nb) - p;
                    // A true gradient estimate: Δh/|d| along d̂ — so the ISEA
                    // creases' uneven spacing does not bias the push.
                    let l2 = d.length_squared().max(1e-9);
                    g += d * ((seams.heat(*nb) - h0) / l2);
                }
                let shove = -(g - p * p.dot(g));
                let mag = shove.length();
                if mag < 1e-6 {
                    Vec3::ZERO
                } else {
                    // The DIRECTION is entirely the local field's; the SPEED
                    // saturates at the band's ceiling — hot ground creeps at
                    // the geological pace, cold ground does not creep at all.
                    shove / mag * (mag * RATE_SCALE).min(RATE_MAX)
                }
            })
            .collect();
        if self.drift.len() != n {
            self.drift = vec![0.0; n];
        }
    }

    /// The BANDED CIRCULATION, derived once per world (static, like the
    /// seams' push): per layer, each tile's wind — trades, westerlies and
    /// polar easterlies with the cells' meridional return, rotated by a
    /// seeded MEANDER so no band is compass-drawn — reduced to a downwind
    /// neighbour and a speed scale; plus the surface CONVERGENCE profile
    /// (the ITCZ, the subtropical divergence, the polar front).
    fn derive_winds(&mut self, map: &HexMap, seed: u64) {
        let n = map.len();
        let mut pr = fastrand::Rng::with_seed(seed ^ 0x5749_4E44_4D45_414E);
        let axes: Vec<(Vec3, f32)> = (0..3)
            .map(|_| {
                let v = Vec3::new(
                    pr.f32() * 2.0 - 1.0,
                    pr.f32() * 2.0 - 1.0,
                    pr.f32() * 2.0 - 1.0,
                );
                let v = if v.length_squared() < 1e-6 {
                    Vec3::X
                } else {
                    v
                };
                (v.normalize(), pr.f32() * std::f32::consts::TAU)
            })
            .collect();
        // Per-layer control points over |latitude|: (|lat|, zonal east+,
        // meridional toward-equator+), lerped between — the boundary layer
        // runs the classic bands, the middle deck transitions, the high
        // deck runs the westerly return flow.
        let bands: [&[(f32, f32, f32)]; AIR_LAYERS] = [
            &[
                (0.0, -0.9, 0.50),
                (0.30, -0.6, 0.45),
                (0.42, 1.0, -0.35),
                (0.68, 0.9, -0.30),
                (0.80, -0.7, 0.35),
                (1.0, -0.4, 0.20),
            ],
            &[
                (0.0, -0.5, 0.15),
                (0.35, 0.2, 0.0),
                (0.55, 0.9, -0.1),
                (1.0, 0.2, 0.1),
            ],
            &[
                (0.0, 0.6, -0.50),
                (0.35, 0.9, -0.30),
                (0.65, 1.0, 0.20),
                (1.0, 0.5, 0.30),
            ],
        ];
        const SPEEDS: [f32; AIR_LAYERS] = [1.0, 1.3, 1.7];
        let bump = |a: f32, c: f32, w: f32| (1.0 - ((a - c) / w).powi(2)).max(0.0);
        self.wind_to = vec![vec![u32::MAX; n]; AIR_LAYERS];
        self.wind_w = vec![vec![0.0; n]; AIR_LAYERS];
        self.wind_to2 = vec![vec![u32::MAX; n]; AIR_LAYERS];
        self.wind_frac = vec![vec![1.0; n]; AIR_LAYERS];
        self.conv = vec![0.0; n];
        for t in 0..n as TileId {
            let i = t as usize;
            let p = map.direction(t);
            let east = Vec3::Y.cross(p).normalize_or_zero();
            let north = p.cross(east).normalize_or_zero();
            let toward_eq = north * (-p.y.signum());
            let a = p.y.abs();
            let meander: f32 = axes
                .iter()
                .map(|(ax, ph)| (3.0 * p.dot(*ax) + ph).sin())
                .sum::<f32>()
                / 3.0;
            let ang = WIND_MEANDER * meander;
            for (l, pts) in bands.iter().enumerate() {
                let (mut z, mut m) = (pts[0].1, pts[0].2);
                for w in pts.windows(2) {
                    let (a0, z0, m0) = w[0];
                    let (a1, z1, m1) = w[1];
                    if a >= a0 && a <= a1 {
                        let f = (a - a0) / (a1 - a0).max(1e-6);
                        z = z0 + (z1 - z0) * f;
                        m = m0 + (m1 - m0) * f;
                    }
                }
                let dir = east * z + toward_eq * m;
                let dir = if dir.length_squared() < 1e-9 {
                    continue; // becalmed (the exact poles)
                } else {
                    let d = dir.normalize();
                    d * ang.cos() + p.cross(d) * ang.sin()
                };
                let mut best = (f32::MIN, i);
                let mut second = (f32::MIN, i);
                for nb in map.neighbours(t) {
                    let toward = map.direction(*nb) - p;
                    let along = (toward - p * p.dot(toward)).normalize_or_zero().dot(dir);
                    if along > best.0 {
                        second = best;
                        best = (along, *nb as usize);
                    } else if along > second.0 {
                        second = (along, *nb as usize);
                    }
                }
                if best.1 != i {
                    self.wind_to[l][i] = best.1 as u32;
                    self.wind_w[l][i] = SPEEDS[l];
                    // THE LANE-BREAKER: the flow splits over the two most-
                    // aligned neighbours, alignment-weighted — one-neighbour
                    // advection collapsed the wind into hex-axis lanes and
                    // the clouds streaked along them.
                    if second.1 != i && second.0 > 0.0 {
                        self.wind_to2[l][i] = second.1 as u32;
                        self.wind_frac[l][i] = (best.0 / (best.0 + second.0)).clamp(0.5, 1.0);
                    }
                }
            }
            let c = bump(a, 0.0, 0.14) - 0.9 * bump(a, 0.35, 0.12) + 0.55 * bump(a, 0.62, 0.11)
                - 0.6 * bump(a, 0.95, 0.14);
            self.conv[i] = c * (1.0 + 0.35 * meander);
        }
    }

    /// In-flight moisture in atmospheric layer `l` at `tile` — what the
    /// volumetric decks draw as concentrations.
    pub fn air_layer(&self, l: usize, tile: TileId) -> f32 {
        self.air
            .get(l)
            .and_then(|v| v.get(tile as usize))
            .copied()
            .unwrap_or(0.0)
    }

    /// The RAIN ledger at `tile` — precipitation, EMA-folded: what the
    /// erosion drinks and the land's green/desert tint reads.
    pub fn rainfall(&self, tile: TileId) -> f32 {
        self.rain.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// VEGETATION cover at `tile` (0..1) — what the sustained rain grew:
    /// the greening, the soil's shield, and the map's green ink.
    pub fn vegetation(&self, tile: TileId) -> f32 {
        self.veg.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The GREEN TARGET the flora pursues (share of standing land greened).
    pub fn veg_target(&self) -> f32 {
        self.veg_target
    }

    /// Set the green target — the dial's write. The thirst converges on it;
    /// nothing repaints instantly.
    pub fn set_veg_target(&mut self, share: f32) {
        self.veg_target = share.clamp(0.0, 1.0);
    }

    /// Last tick's measured GREEN SHARE of standing land — the live gauge
    /// the target steers.
    pub fn green_share(&self) -> f32 {
        self.green_share
    }

    /// How many interior vacancies the safety-net heal has ever filled —
    /// near zero under the rotated-image step.
    pub fn heals(&self) -> u64 {
        self.heals
    }

    /// How many ticks have run.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// How many eruptions have fired — the occasional ticks that left a whole
    /// lava flow.
    pub fn eruptions(&self) -> u64 {
        self.eruptions
    }

    /// How many plate steps have fired across the era.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// How many tiles' material changed last tick — the imperceptibility
    /// readout: typically HUNDREDS on a full-size world, spiking only when a
    /// plate steps or a volcano floods.
    pub fn changed_tiles(&self) -> usize {
        self.changed
    }

    /// Wake `tile` and its ring for the next tick — the hook a tool (or a
    /// test planting state by hand) uses to hand its edit to the frontier;
    /// nothing else re-examines a standing tile.
    pub fn disturb(&mut self, map: &HexMap, tile: TileId) {
        if let Some(f) = self.frontier.get_mut(tile as usize) {
            *f = true;
        }
        for nb in map.neighbours(tile) {
            if let Some(f) = self.frontier.get_mut(*nb as usize) {
                *f = true;
            }
        }
    }

    /// Plant a single-cell vein body by hand — the bench/test hook (the
    /// [`Self::disturb`] pattern): the kind lands at `tile` and a node
    /// registers, so the labels, lenses and census treat it as earned.
    pub fn plant_vein(&mut self, tile: TileId, kind: u8) {
        let i = tile as usize;
        self.vein[i] = 1 + kind;
        self.vein_nodes.push(VeinNode {
            center: tile,
            kind,
            size: 1,
            budget: 1,
        });
        self.vein_node_of[i] = self.vein_nodes.len() as u16;
    }

    /// Loose rock height at `tile`, tile-width units.
    pub fn rock(&self, tile: TileId) -> f32 {
        self.rock.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The loose rock's blended HARDNESS at `tile` — 1.0 is the reference
    /// grade; what the vents fed it, mass-weighted through every transfer.
    pub fn rock_hardness(&self, tile: TileId) -> f32 {
        self.rock_hard.get(tile as usize).copied().unwrap_or(1.0)
    }

    /// A vent's characteristic output grade: its PROVINCE draw (a smooth
    /// seeded field — neighbouring vents pour kindred rock), drifting slowly
    /// with its cumulative emission — the same vent pours a consistent
    /// stream that wanders over geological spans.
    fn vent_hardness(&self, vent_idx: usize) -> f32 {
        let base = self.vent_grade.get(vent_idx).copied().unwrap_or(1.0);
        let drift = (self.emitted.get(vent_idx).copied().unwrap_or(0.0) * 1.7).sin();
        (base + VENT_HARD_DRIFT * drift).clamp(0.4, 1.9)
    }

    /// Last tick's stream DISCHARGE at `tile` — rainfall accumulated down
    /// the steepest-descent network; what the carving cuts by.
    pub fn discharge(&self, tile: TileId) -> f32 {
        self.discharge.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The plate shell's base height at `tile` (tile-width units) — the part
    /// of the column that RIDES the conveyor.
    pub fn base(&self, tile: TileId) -> f32 {
        self.base.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Formed strata at `tile`: (engaged crust slots, total formed height) —
    /// the sub-group summary: L3 + L4 (L2 is `base`, L5 is reserved).
    pub fn strata(&self, tile: TileId) -> (u8, f32) {
        let l3 = self.layer3(tile);
        let l4 = self.layer4(tile);
        (u8::from(l3 > 0.0) + u8::from(l4 > 0.0), l3 + l4)
    }

    /// The VEIN layer's height at `tile` (stack layer 3).
    pub fn layer3(&self, tile: TileId) -> f32 {
        self.l3_h.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The VOLCANIC layer's height at `tile` (stack layer 4).
    pub fn layer4(&self, tile: TileId) -> f32 {
        self.l4_h.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Nucleate a vein NODE at `tile` — refused inside the separation ring
    /// of any existing node (discreteness IS the distribution), budgeted by
    /// the seeded draw: a handful of cells, every seventh massive, never
    /// past the cap.
    fn nucleate(&mut self, map: &HexMap, tile: TileId, kind: u8, h: u64, forced: bool) {
        let tile_r = 2.0 / (map.len() as f32).sqrt();
        let sep = (VEIN_NODE_SEP_TILES * 2.0 * tile_r).cos();
        let d = map.direction(tile);
        if !forced
            && self
                .vein_nodes
                .iter()
                .any(|nd| d.dot(map.direction(nd.center)) > sep)
        {
            return; // too close to a living body — the ore went there
        }
        let mut budget = NODE_BASE + (h % u64::from(NODE_SPAN)) as u16;
        if (h / 97).is_multiple_of(NODE_MASSIVE_EVERY) {
            budget *= NODE_MASSIVE_MUL;
        }
        let budget = budget.min(NODE_MAX);
        self.vein[tile as usize] = 1 + kind;
        self.vein_nodes.push(VeinNode {
            center: tile,
            kind,
            size: 1,
            budget,
        });
        self.vein_node_of[tile as usize] = self.vein_nodes.len() as u16;
    }

    /// The vein at `tile`: `None`, or an index into [`vein_kinds`].
    pub fn vein(&self, tile: TileId) -> Option<u8> {
        match self.vein.get(tile as usize).copied().unwrap_or(0) {
            0 => None,
            k => Some(k - 1),
        }
    }

    /// Every living vein NODE (centre tile, kind index) — nodes whose centre
    /// lost its vein are filtered out: what the element billboards label.
    pub fn vein_nodes(&self) -> impl Iterator<Item = (TileId, u8)> + '_ {
        self.vein_nodes
            .iter()
            .filter(|nd| self.vein.get(nd.center as usize).copied().unwrap_or(0) != 0)
            .map(|nd| (nd.center, nd.kind))
    }

    /// The living bodies themselves — sizes and budgets, for gates and tools.
    pub fn vein_bodies(&self) -> &[VeinNode] {
        &self.vein_nodes
    }

    /// **The material census**: how many hexes carry each vein kind, sorted
    /// most-common first — the right panel's live readout of the world's
    /// mineral wealth growing (Aaron 2026-08-26: watch it to tune spawn
    /// rates later).
    pub fn vein_census(&self) -> Vec<(u8, u32)> {
        let mut counts = vec![0u32; vein_kinds().len()];
        for v in &self.vein {
            if *v > 0 {
                counts[(*v - 1) as usize] += 1;
            }
        }
        let mut out: Vec<(u8, u32)> = counts
            .into_iter()
            .enumerate()
            .filter(|(_, c)| *c > 0)
            .map(|(k, c)| (k as u8, c))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    /// The whole grown height at `tile` — loose rock, loose sediment and the
    /// formed strata.
    pub fn grown(&self, tile: TileId) -> f32 {
        self.rock(tile) + self.sediment(tile) + self.strata(tile).1
    }

    /// Loose sediment at `tile`, tile-width units.
    pub fn sediment(&self, tile: TileId) -> f32 {
        self.sediment.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Standing ice at `tile`, tile-width units — the cap on this column.
    pub fn ice(&self, tile: TileId) -> f32 {
        self.ice.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The LIVE planet temperature (0..1) — the ice-age runner's current
    /// reading around the dial's baseline. What the climate gauge shows.
    pub fn climate(&self) -> f32 {
        self.temp
    }

    /// The water-coverage target the in-fall pursues (0..1).
    pub fn water_target(&self) -> f32 {
        self.water_target
    }

    /// Set the coverage target — the dial's write. The in-fall converges on
    /// it; nothing re-pours instantly.
    pub fn set_water_target(&mut self, share: f32) {
        self.water_target = share.clamp(0.0, 1.0);
    }

    /// The ocean's temperature by DEPTH at `tile`: (surface, shallow, deep)
    /// on the climate scale. Surface is the tracked band; DEEP is the one
    /// global reservoir (the ratified optimization); SHALLOW is their mix,
    /// derived and never stored.
    pub fn ocean_temps(&self, tile: TileId) -> (f32, f32, f32) {
        let sst = self
            .sst
            .get(tile as usize)
            .copied()
            .unwrap_or(self.climate_base);
        let deep = self.deep_temp;
        (sst, (sst + deep) * 0.5, deep)
    }

    /// The deep ocean's one global temperature.
    pub fn deep_ocean_temp(&self) -> f32 {
        self.deep_temp
    }

    /// Set the climate BASELINE the runner oscillates around (0..1).
    pub fn set_climate(&mut self, base: f32) {
        self.climate_base = base.clamp(0.0, 1.0);
    }

    /// A tile's LOCAL temperature this tick: the planet's live temperature,
    /// cooled by latitude (quartic — tight caps, broad tropics) and by
    /// altitude above the sea.
    pub fn local_temp(&self, tile: TileId, dir: Vec3, sea: f32) -> f32 {
        let lat = dir.y * dir.y;
        let alt = (self.ground(tile) - sea).max(0.0);
        let micro = self.micro.get(tile as usize).copied().unwrap_or(0.0);
        self.temp - ICE_LAT * lat * lat - ICE_ALT * alt + micro
    }

    /// The MARINE COMPACTION grade at `tile` — 1.0 uncompacted, rising while
    /// the column stands under water (the water is the press), kept for good
    /// once it emerges. What the erosion resists by and the collisions weigh.
    pub fn bed_hardness(&self, tile: TileId) -> f32 {
        self.bed_hard.get(tile as usize).copied().unwrap_or(1.0)
    }

    /// Collision pressure at `tile` — what the merge zones carry.
    pub fn pressure(&self, tile: TileId) -> f32 {
        self.pressure.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The tracked COLLISION-EDGE intensity at `tile` — the EMA of jam flux,
    /// the crust-side corollary of the molten seams. ≥ the live threshold
    /// means flows are meeting here NOW; what an edge lens draws.
    pub fn collision_edge(&self, tile: TileId) -> f32 {
        self.edge.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// How many consecutive ticks the edge at `tile` has stayed live — its
    /// PERSISTENCE, zero when the collision has stopped. Mature edges have
    /// uplift leverage: this is where the ranges grow.
    pub fn collision_age(&self, tile: TileId) -> u32 {
        self.edge_age.get(tile as usize).copied().unwrap_or(0)
    }

    /// **THE GRADED LEVEL** at `tile` — the outlet elevation this column has
    /// adjusted to. Equal to its live outlet wherever the ground is at
    /// grade; ABOVE it wherever a base-level fall is still climbing the
    /// drainage, which is what a knickzone IS.
    pub fn graded_level(&self, tile: TileId) -> f32 {
        self.graded.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The SUSPENDED load at `tile` — carried sediment the ground has not
    /// yet accepted (flood control), area-weighted volume units. Part of the
    /// conserved material ledger; no height until it settles.
    pub fn suspended(&self, tile: TileId) -> f32 {
        self.suspend.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The DISSOLVED load at `tile` — bed that has gone into solution and
    /// has not come back out, area-weighted volume units. The FOURTH store
    /// of the conserved material ledger (columns + suspension + well +
    /// this), and the second one that is water-borne: no height, no relief,
    /// moving only where the water moves.
    pub fn dissolved(&self, tile: TileId) -> f32 {
        self.dissolved.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// **THE WELL** — what the convergences have sunk and the fountains have
    /// not yet returned, in area-weighted VOLUME units; and the cumulative
    /// take behind it. The ledger's missing term: the world's material is
    /// the columns PLUS the suspension PLUS this.
    pub fn well(&self) -> f32 {
        self.well
    }

    /// Cumulative volume the collisions and the basal ceiling have sunk.
    pub fn sunk(&self) -> f32 {
        self.sunk
    }

    /// How many times the basal ceiling has fired. Zero over a real run
    /// means the derivation is wrong, not that the world is calm.
    pub fn delaminations(&self) -> u64 {
        self.delaminations
    }

    /// The era's OVERBURDEN at a column — its stack weighed by the grade
    /// each part consolidated at. This era's pressure unit, in the era's own
    /// numbers rather than an imported pascal.
    fn overburden(&self, i: usize) -> f32 {
        self.bed_hard[i]
            * (self.base[i] + self.l3_h[i] * self.l3_hard[i] + self.l4_h[i] * self.l4_hard[i])
            + self.rock[i] * self.rock_hard[i]
            + self.sediment[i] * SED_GRADE
    }

    /// **THE DISPOSITION, in one line** — the share of converging material
    /// at `grade` that leaves the surface for the well. Everything short of
    /// the metamorphic floor founders in proportion to how far short it
    /// stands; what has metamorphosed arrests and crumples (DCA4D316). One
    /// law for the cover and for every slot of the basement under it.
    fn sink_share(grade: f32) -> f32 {
        ((ARREST_GRADE - grade) / (ARREST_GRADE - VENT_HARD_MIN)).clamp(0.0, 1.0)
    }

    /// **THE FACE** — the grade of the material an oversteep column would
    /// shed FIRST: its loose pile while it has one (sediment always, then
    /// rock at whatever the vents or the orogen made it), otherwise the top
    /// of its consolidated ladder, and the bed itself when the ladder is
    /// bare. The discriminator the oversteep wake reads: read against
    /// `ARREST_GRADE` it is the same material line the disposition uses —
    /// what has consolidated past the metamorphic floor may STAND, what has
    /// not is a pending failure.
    fn face_grade(&self, i: usize) -> f32 {
        if self.sediment[i] > ACT_EPS {
            SED_GRADE
        } else if self.rock[i] > ACT_EPS {
            self.rock_hard[i]
        } else if self.l4_h[i] > ACT_EPS {
            self.bed_hard[i] * self.l4_hard[i]
        } else if self.l3_h[i] > ACT_EPS {
            self.bed_hard[i] * self.l3_hard[i]
        } else {
            self.bed_hard[i]
        }
    }

    /// **THE OUTLET LEVEL** the column at `i` grades to — the control its
    /// graded level chases. The SEA wherever its stream runs onto drowned
    /// ground, because a river grades to the water SURFACE and not to the
    /// floor beneath it (this is the whole coupling between the percentile
    /// sea, the ice age that moves it, and the land's answer); the receiving
    /// bed where the stream stays on land; and the column's own floor where
    /// nothing drains away at all — an internally-drained pit is its own
    /// base level, and holds while its drain condition holds (R2).
    fn outlet_level(&self, i: usize, flow_to: u32, sea: f32) -> f32 {
        let h = self.ground(i as TileId);
        match flow_to {
            u32::MAX => h,
            j => self.ground(j as TileId).max(sea).min(h),
        }
    }

    /// **THE CONDITION REVERSES** at `i` — is this a place the water can no
    /// longer hold what it carries? Two answers, both read from rates the
    /// era already keeps, neither of them a new line:
    ///
    /// 1. **A DRYING BASIN** (the EVAPORITE, 8CA52AC5's standing item —
    ///    *"salt is not a hydrothermal vein… it forms by a sea evaporating"*).
    ///    Three plain clauses, no new line among them: nothing drains away
    ///    (the column IS its own outlet), no river feeds it (the era's own
    ///    live-channel threshold), and the SUN TAKES MORE THAN THE SKY
    ///    BRINGS — the Weather phase's own open-water evaporation rate at
    ///    the local warmth, against this column's rain. A basin under a
    ///    trunk river or a wet sky never dries; a rain-starved sink does.
    /// 2. **WARM SHALLOW WATER** (the CARBONATE PLATFORM). A drowned bed
    ///    inside the marine press's own depth and over the freezing line
    ///    gives its load back. Cold deep water KEEPS it, which is why the
    ///    store has to be a holding state at all — and it is what closes the
    ///    cycle: what the rain takes off the land comes back as marine bed,
    ///    which the press then indurates into soluble rock again.
    fn returns(&self, map: &HexMap, i: usize, flow_to: u32, sea: f32) -> bool {
        let t = i as TileId;
        let depth = sea - self.ground(t);
        if depth > 0.0 {
            return depth < MARINE_DEPTH_CAP && self.sst[i] > FREEZE_POINT;
        }
        let warmth = (0.25 + self.local_temp(t, map.direction(t), sea)).clamp(0.2, 1.3);
        flow_to == u32::MAX && self.discharge[i] < CHANNEL_LIVE && self.rain[i] < EVAP_SEA * warmth
    }

    /// **THE RETURN** — bring `vol` of dissolved load out of solution at `i`,
    /// and hand back what stayed in. ONE delivery for both conditions,
    /// budgeted against the SAME per-cell `settled` intake the carry uses, so
    /// the two water-borne channels together can never land more than
    /// `INTAKE_CAP` on a column in a tick (CA9045F5, in letter).
    ///
    /// The new bed CONSOLIDATES rather than piling: it enters the grade
    /// system as vein-layer mass at the soft precipitate grade, mass-blended
    /// and laid FLAT exactly as the sediment path lays its beds — a
    /// chemically settled bed is the flattest thing the world makes. What the
    /// budget or the slot refuses STAYS IN SOLUTION at this tile; the store
    /// is its own holding state, so an over-saturated basin simply takes
    /// longer instead of manufacturing height (B390DA57's lesson, applied at
    /// design time rather than after five rounds).
    fn precipitate(&mut self, act: &mut [bool], settled: &mut [f32], i: usize, vol: f32) -> f32 {
        let room = (INTAKE_CAP - settled[i])
            .max(0.0)
            .min((L3_CAP - self.l3_h[i]).max(0.0));
        let put = (vol / self.area[i]).min(room);
        if put <= 0.0 {
            return vol;
        }
        self.l3_hard[i] = (self.l3_h[i] * self.l3_hard[i] + put * SED_GRADE) / (self.l3_h[i] + put);
        self.l3_dip[i] = Self::bedded_flat(self.l3_dip[i], self.l3_h[i], put);
        self.l3_h[i] += put;
        settled[i] += put;
        if put > ACT_EPS {
            act[i] = true;
        }
        vol - put * self.area[i]
    }

    /// **THE FOUNTAINS DRAW** — an injector's quantum is SOURCED, never
    /// resized: the pour at `i` is exactly what it always was, and this
    /// books where it came from. Everything above the decompression floor
    /// comes out of the well while the well can cover it; what the fountains
    /// cannot take stays where it sank, and the mantle makes up the rest.
    fn draw_melt(&mut self, i: usize, pour: f32) {
        let take = (pour * (1.0 - MELT_FLOOR)).min((self.well / self.area[i]).max(0.0));
        self.well -= take * self.area[i];
    }

    /// The formed slots' inherited GRADES at `tile`: (L3, L4) — what the
    /// strata consolidated FROM, and what their erosion divides by.
    pub fn strata_hardness(&self, tile: TileId) -> (f32, f32) {
        (
            self.l3_hard.get(tile as usize).copied().unwrap_or(1.0),
            self.l4_hard.get(tile as usize).copied().unwrap_or(1.0),
        )
    }

    /// **THE FABRIC** at `tile`: the column's recorded bedding TREND (strike
    /// azimuth, radians in the tile's east/north tangent frame, `[0, π)`)
    /// and each formed slot's DIP. All zero on ground that has never been
    /// shortened — the null fabric, which erodes isotropically.
    pub fn strata_fabric(&self, tile: TileId) -> (f32, f32, f32) {
        let i = tile as usize;
        (
            self.strike.get(i).copied().unwrap_or(0.0),
            self.l3_dip.get(i).copied().unwrap_or(0.0),
            self.l4_dip.get(i).copied().unwrap_or(0.0),
        )
    }

    /// The tile's canonical TANGENT FRAME — the same `(east, north)` basis
    /// the winds are derived in, so a stored azimuth means one thing
    /// everywhere on the sphere and survives capture/restore unchanged.
    fn frame(p: Vec3) -> (Vec3, Vec3) {
        let east = Vec3::Y.cross(p).normalize_or_zero();
        (east, p.cross(east).normalize_or_zero())
    }

    /// **DEPOSITION LAYS BEDS FLAT.** New mass consolidating into a slot
    /// settles horizontal, so the slot's recorded dip dilutes by exactly the
    /// mass blend its GRADE uses — `have` at the standing dip plus `take` at
    /// zero. An empty slot therefore opens at dip 0, and vent-poured rock,
    /// which carries no fabric of its own, can only ever flatten the bed it
    /// joins.
    fn bedded_flat(dip: f32, have: f32, take: f32) -> f32 {
        dip * have / (have + take)
    }

    /// **THE FABRIC'S ALIGNMENT** at `i`: `cos 2θ` between the local flow
    /// direction and the recorded strike LINE — `+1` where the stream runs
    /// ALONG the strike, `−1` where it cuts straight ACROSS it. Both
    /// directions come from geometry the tick already holds: the flow target
    /// is the stream tree's own receiver (`flow_to`), the strike is the
    /// azimuth the deformation recorded. A tile with nowhere to flow (a pit)
    /// or a degenerate tangent has no direction at all and reads 0 —
    /// isotropic, exactly as if it carried no fabric.
    fn strike_align(&self, map: &HexMap, i: usize, flow_to: u32, p: Vec3) -> f32 {
        if flow_to == u32::MAX {
            return 0.0;
        }
        let toward = map.direction(flow_to as TileId) - p;
        let flow = (toward - p * p.dot(toward)).normalize_or_zero();
        let (east, north) = Self::frame(p);
        if flow.length_squared() < 0.5 || east.length_squared() < 0.5 {
            return 0.0;
        }
        let (s, c) = self.strike[i].sin_cos();
        let along = flow.dot(east * c + north * s);
        2.0 * along * along - 1.0
    }

    /// Last tick's moisture at `tile`, `0..1` — what the atmosphere shows and
    /// the weathering drinks.
    pub fn moisture(&self, tile: TileId) -> f32 {
        self.moist.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The whole GROUND height at `tile` (tile-width units): the CRUST alone —
    /// the plate base riding the conveyor plus everything grown on top. The
    /// molten and bedrock layers beneath are the IMMOBILE frame (Aaron
    /// 2026-08-25): they drive injection but never join the moving surface's
    /// height. Ridges are BUILT from injected material, not painted.
    pub fn ground(&self, tile: TileId) -> f32 {
        self.base(tile) + self.grown(tile)
    }

    /// The sea level that floods `pct` percent of the CURRENT world. The dial
    /// means "this share of tiles stands UNDER water", so on a TIERED world
    /// (tick zero is three flat tiers) the line sits BETWEEN the percentile
    /// tier and the next distinct height above it — the whole tier at the
    /// percentile is submerged, in shallow water, rather than the flood
    /// evaporating because the strict count stopped at the tier's own height.
    pub fn sea_level(&self, pct: f32) -> f32 {
        let mut totals: Vec<f32> = (0..self.rock.len() as TileId)
            .map(|t| self.ground(t))
            .collect();
        if totals.is_empty() {
            return 0.0;
        }
        totals.sort_by(f32::total_cmp);
        let n = totals.len();
        let ask = (pct / 100.0).clamp(0.0, 1.0);
        // Water fills WHOLE tiers. Walk the distinct heights and place the
        // line in the gap whose cumulative coverage is CLOSEST to the ask —
        // never "everything" because a heavy top tier happened to straddle
        // the percentile. Coverage 0 (a line below the lowest ground) and
        // full flood are both legitimate answers of the walk.
        let mut best = (f32::MAX, totals[0] - 0.05); // |cover − ask|, line
        let mut cover_below = 0usize;
        let mut i = 0usize;
        while i < n {
            let v = totals[i];
            let mut j = i;
            while j < n && totals[j] <= v + 1e-5 {
                j += 1;
            }
            // The line ABOVE this tier: midway to the next distinct height.
            let line = if j < n {
                (v + totals[j]) * 0.5
            } else {
                v + 0.05
            };
            let cover = j as f32 / n as f32;
            let d = (cover - ask).abs();
            if d < best.0 {
                best = (d, line);
            }
            // …and the line BELOW the first tier was seeded above.
            let d0 = (cover_below as f32 / n as f32 - ask).abs();
            if d0 < best.0 {
                best = (d0, totals[0] - 0.05);
            }
            cover_below = j;
            i = j;
        }
        best.1
    }

    /// How many strata have formed across the world.
    pub fn strata_total(&self) -> u64 {
        (0..self.l3_h.len())
            .map(|i| u64::from(self.l3_h[i] > 0.0) + u64::from(self.l4_h[i] > 0.0))
            .sum()
    }

    /// The LOCAL push at `tile` — the seams' shove on this ground, radians of
    /// travel per tick, tangential. Zero where the mantle is quiet: cold
    /// crust does not creep, and every arrow radiates from a source.
    pub fn push_at(&self, tile: TileId) -> Vec3 {
        self.push.get(tile as usize).copied().unwrap_or(Vec3::ZERO)
    }

    /// How far along its NEXT one-hex step this TILE has come, `0..1` — the
    /// honest arrow length for the local ratchet.
    pub fn drift_progress(&self, tile: TileId) -> f32 {
        let n = self.rock.len();
        if n == 0 {
            return 0.0;
        }
        let tile_step = 4.0 / (n as f32).sqrt();
        self.drift
            .get(tile as usize)
            .map_or(0.0, |c| (c / tile_step).clamp(0.0, 1.0))
    }

    /// **One tick of the era.** Inject → spread → boundary uplift/subduction →
    /// advect on the ratchet → consolidate. Each phase computes into buffers
    /// before applying (two-pass, F9C4514D).
    /// Run ONE procedure of the cycle and advance the cursor. Returns `true`
    /// when this step COMPLETED the cycle (the tick). The cycle's water level
    /// is captured at its first phase — `sea` is read only there, so a cycle
    /// is internally consistent however often the caller re-resolves.
    pub fn tick_phase(
        &mut self,
        map: &HexMap,
        seams: &SeamField,
        crust: &CrustField,
        sea: f32,
    ) -> bool {
        let n = map.len();
        let tile_step = 4.0 / (n as f32).sqrt(); // one tile of angular travel
        if self.carry.is_none() {
            // The cycle OPENS: the frontier becomes this cycle's active set,
            // the ledgers are snapshotted for the closing rebuild, and the
            // water level is fixed for the cycle.
            if self.frontier.len() != n {
                self.frontier = vec![false; n];
            }
            self.carry = Some(TickCarry {
                act: std::mem::replace(&mut self.frontier, vec![false; n]),
                compressed: Vec::new(),
                snap_base: self.base.clone(),
                snap_rock: self.rock.clone(),
                snap_sed: self.sediment.clone(),
                snap_strata: self
                    .l3_h
                    .iter()
                    .zip(&self.l4_h)
                    .map(|(a, b)| a + b)
                    .collect(),
                sea,
            });
        }
        let phase = PHASES[self.cursor as usize];
        let TickCarry {
            mut act,
            mut compressed,
            snap_base,
            snap_rock,
            snap_sed,
            snap_strata,
            sea,
        } = self.carry.take().expect("the cycle is open");
        self.run_phase(
            phase,
            map,
            seams,
            crust,
            sea,
            &mut act,
            &mut compressed,
            (&snap_base, &snap_rock, &snap_sed, &snap_strata),
            n,
            tile_step,
        );
        self.cursor += 1;
        if self.cursor as usize == PHASES.len() {
            self.cursor = 0;
            // Weld's arm already rebuilt the frontier and counted the tick;
            // the carry is spent.
            true
        } else {
            self.carry = Some(TickCarry {
                act,
                compressed,
                snap_base,
                snap_rock,
                snap_sed,
                snap_strata,
                sea,
            });
            false
        }
    }

    /// The procedure the NEXT engine step will run — what the phase label
    /// shows as currently in progress.
    pub fn current_phase(&self) -> Phase {
        PHASES[self.cursor as usize]
    }

    /// One full CYCLE of the pipeline — every procedure once. The tests' and
    /// the step button's tick; the run loop steps [`tick_phase`] instead.
    pub fn tick(&mut self, map: &HexMap, seams: &SeamField, crust: &CrustField, sea: f32) {
        while !self.tick_phase(map, seams, crust, sea) {}
    }

    // ptr_arg: Collide REPLACES the compressed buffer wholesale, so the
    // parameter must be the Vec, not a slice.
    #[allow(clippy::too_many_arguments, clippy::ptr_arg)] // the cycle's shared bench, threaded once
    fn run_phase(
        &mut self,
        phase: Phase,
        map: &HexMap,
        seams: &SeamField,
        crust: &CrustField,
        sea: f32,
        act_ref: &mut Vec<bool>,
        compressed: &mut Vec<bool>,
        snaps: (&Vec<f32>, &Vec<f32>, &Vec<f32>, &Vec<f32>),
        n: usize,
        tile_step: f32,
    ) {
        // The section bodies below are the monolithic tick's, verbatim — the
        // names they used are rebound here.
        let act = act_ref;
        let (snap_base, snap_rock, snap_sed, snap_strata) = snaps;
        let _ = (&sea, &tile_step, &n);
        match phase {
            Phase::Climate => {
                // 0 — THE ICE AGE RUNNER (Aaron 2026-08-26): the planet's live
                // temperature = the dial's baseline + two slow seeded sines over the
                // tick count — glacials and interglacials that never repeat on a
                // short beat. Then every tile's cap moves toward its LOCAL
                // equilibrium: cold enough freezes (deeper cold, thicker target),
                // warm melts — growth patient, melt a little hungrier. The caps LOCK
                // area-weighted water volume out of the sea. SILENT state like the
                // marine press: ice changing thickness moves no material, so it
                // neither joins the frontier nor wakes a ring — only its erosion
                // does, through the normal channels below.
                {
                    let phase = |k: usize| {
                        fastrand::Rng::with_seed(seams.seed().wrapping_add(k as u64 + 11)).f32()
                            * std::f32::consts::TAU
                    };
                    let t = self.ticks as f32;
                    let mut osc = 0.0;
                    for (k, (period, amp)) in ICE_AGE_PERIODS.iter().zip(ICE_AGE_AMPS).enumerate() {
                        osc += amp * (t * std::f32::consts::TAU / period + phase(k)).sin();
                    }
                    // THE GREENHOUSE: the frozen share of the surface builds
                    // it, time decays it — the snowball escape hatch.
                    let frozen_share =
                        self.ice.iter().filter(|c| **c > ICE_ERODE_MIN).count() as f32 / n as f32;
                    self.greenhouse += GH_BUILD * frozen_share - GH_DECAY * self.greenhouse;
                    self.temp = (self.climate_base + osc + self.greenhouse).clamp(0.0, 1.05);
                    // Growth DRAWS FROM the sea: sum this tick's growth demand first,
                    // then scale it to what the conserved water can still give (the
                    // caps never lock more than ICE_MAX_LOCK of the volume — and a
                    // dry world grows no ice at all). Melt is never rationed.
                    let mut demand = 0.0f32;
                    let mut deltas = vec![0.0f32; n];
                    let mut wet_sum = 0.0f64;
                    let mut wet_n = 0usize;
                    #[allow(clippy::needless_range_loop)] // parallel stores, one index
                    for i in 0..n {
                        let tile = i as TileId;
                        let local = self.local_temp(tile, map.direction(tile), sea);
                        // THE OCEAN'S SURFACE BAND: over water it chases the
                        // air with thermal inertia; over dry ground it IS the
                        // air (continuous shorelines). Sea ice keys to the
                        // WATER's temperature, not the air's — the lag is the
                        // point: oceans freeze later and thaw later.
                        let wet = self.ground(tile) < sea;
                        if wet {
                            self.sst[i] += (local - self.sst[i]) * SST_CHASE;
                            wet_sum += f64::from(self.sst[i]);
                            wet_n += 1;
                        } else {
                            self.sst[i] = local;
                        }
                        let felt = if wet { self.sst[i] } else { local };
                        let target = if felt < FREEZE_POINT {
                            ICE_THICK * ((FREEZE_POINT - felt) / FREEZE_POINT).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let d = target - self.ice[i];
                        let mut step = d * if d > 0.0 { ICE_GROW } else { ICE_MELT };
                        if step > 0.0 {
                            // Growth is snowfall-limited: the moisture field
                            // is the delivery.
                            step *= self.moist[i].clamp(ICE_SNOW_FLOOR, 1.0);
                        }
                        deltas[i] = step;
                        if step > 0.0 {
                            demand += step * self.area[i];
                        }
                    }
                    // THE DEEP RESERVOIR: one well-mixed scalar creeping
                    // toward the mean surface temperature — the abyss never
                    // boils and never freezes solid.
                    if wet_n > 0 {
                        let mean = (wet_sum / wet_n as f64) as f32;
                        self.deep_temp += (mean - self.deep_temp) * DEEP_CHASE;
                        self.deep_temp = self.deep_temp.clamp(DEEP_MIN, DEEP_MAX);
                    }
                    // THE IN-FALL: coverage below target draws new water in,
                    // deficit-proportional — the slow delivery that keeps a growing
                    // world near its ocean share.
                    let below = (0..n).filter(|i| self.ground(*i as TileId) < sea).count() as f32
                        / n as f32;
                    let deficit = (self.water_target - below).max(0.0);
                    if deficit > 0.0 {
                        let want = deficit * INFALL_GAIN * n as f32;
                        // FIRST SOURCE: the caps. Melting locked ice raises the
                        // standing sea with NO new volume — the reserve was always
                        // part of the budget (resolve_sea solves on volume − locked).
                        let from_caps = want.min(self.ice_locked * CAP_MELT_SHARE);
                        if from_caps > 0.0 && self.ice_locked > 1e-6 {
                            let keep = 1.0 - from_caps / self.ice_locked;
                            for ice in &mut self.ice {
                                *ice *= keep;
                            }
                            self.ice_locked -= from_caps;
                        }
                        // Only the true shortfall falls in from the sky — and only
                        // while the BOMBARDMENT lasts: at the water budget the sky
                        // closes for good, and the caps and the sea share a bounded
                        // total from then on. (The freezer cannot eat imports either
                        // way: under a deficit the ocean's claim holds `givable` at
                        // zero.)
                        if self.water_volume < map.len() as f32 * WATER_BUDGET_DEPTH {
                            self.water_volume += want - from_caps;
                        }
                    }
                    // THE OCEAN'S CLAIM: the deficit shrinks the ice ration — and
                    // locked ice above the shrunken ration FORCE-MELTS, so a
                    // deadlocked glacier yields the sea its water back.
                    let ration_share = (ICE_MAX_LOCK - deficit * ICE_YIELD).max(0.2);
                    let ration = self.water_volume * ration_share;
                    if self.ice_locked > ration {
                        let melt = (self.ice_locked - ration) * EXCESS_MELT;
                        if self.ice_locked > 1e-6 {
                            let keep = 1.0 - melt / self.ice_locked;
                            for c in &mut self.ice {
                                *c *= keep;
                            }
                            self.ice_locked -= melt;
                        }
                    }
                    let givable = (ration - self.ice_locked).max(0.0);
                    let scale = if demand > 1e-9 {
                        (givable / demand).min(1.0)
                    } else {
                        0.0
                    };
                    let mut locked = 0.0;
                    #[allow(clippy::needless_range_loop)] // parallel stores, one index
                    for i in 0..n {
                        let step = if deltas[i] > 0.0 {
                            deltas[i] * scale
                        } else {
                            deltas[i]
                        };
                        self.ice[i] = (self.ice[i] + step).max(0.0);
                        locked += self.ice[i] * self.area[i];
                    }
                    self.ice_locked = locked;
                }
            }
            Phase::Weather => {
                // 0b — THE ATMOSPHERE (Aaron 2026-08-28): evaporation fills
                // the boundary layer, each deck advects along its banded
                // wind, walls rain their windward side and shadow the
                // interior, convergence concentrates and rains, divergence
                // subsides and dries, saturation spills — and the RAIN
                // ledger folds what fell. SILENT state: a field over the
                // world; no material moves, no frontier wakes.
                let mut rain_now = vec![0.0f32; n];
                // EVAPORATION: the open sea by its warmth; the land recycles
                // a share of its standing rain (evapotranspiration).
                for t in 0..n as TileId {
                    let i = t as usize;
                    if self.ground(t) < sea {
                        let warmth = (0.25 + self.sst[i]).clamp(0.2, 1.3);
                        self.air[0][i] += EVAP_SEA * warmth;
                    } else {
                        self.air[0][i] += EVAP_LAND * self.rain[i];
                    }
                }
                // LIFT + DRIZZLE + ADVECTION + THE WALLS, deck by deck,
                // bottom-up. One carry serves both downwind branches.
                let carry = |slf: &mut Self,
                             delta: &mut Vec<f32>,
                             rain_now: &mut Vec<f32>,
                             l: usize,
                             deck: f32,
                             i: usize,
                             j: usize,
                             mv: f32| {
                    if mv <= 1e-6 {
                        return;
                    }
                    if slf.ground(j as TileId) >= deck {
                        // THE WALL: ground past this deck blocks the
                        // crossing — the windward side rains a share, lifts
                        // a share toward the deck above, and the interior
                        // beyond stays DRY (the rain shadow).
                        let shed = mv * OROG_RAIN;
                        let up = mv * OROG_LIFT;
                        rain_now[i] += shed;
                        delta[i] -= shed + up;
                        if l + 1 < AIR_LAYERS {
                            slf.air[l + 1][i] += up;
                        } else {
                            rain_now[i] += up;
                        }
                        return;
                    }
                    let rise = slf.ground(j as TileId) - slf.ground(i as TileId);
                    if l == 0 && rise > OROG_RISE {
                        // Rising ground short of the wall SQUEEZES rain out
                        // on the climb — windward slopes drink.
                        let squeeze = mv * OROG_RAIN * (rise / (OROG_RISE * 3.0)).min(1.0);
                        rain_now[j] += squeeze;
                        delta[i] -= mv;
                        delta[j] += mv - squeeze;
                    } else {
                        delta[i] -= mv;
                        delta[j] += mv;
                    }
                };
                for l in 0..AIR_LAYERS {
                    let deck = sea + DECK_ALT[l];
                    let mut delta = vec![0.0f32; n];
                    for t in 0..n as TileId {
                        let i = t as usize;
                        let m = self.air[l][i];
                        if m <= 1e-5 {
                            continue;
                        }
                        // THE DRIZZLE: every deck precipitates a little of
                        // what it carries as it passes — the upper decks
                        // water the interiors no wall ever rains on.
                        let dz = m * DECK_DRIZZLE[l];
                        delta[i] -= dz;
                        rain_now[i] += dz;
                        if l == 0 {
                            // Convection off warm ground lifts the boundary
                            // layer into the condensation deck.
                            let warm = (self.local_temp(t, map.direction(t), sea) - 0.55).max(0.0);
                            let up = m * CONVECT_LIFT * warm.min(0.5);
                            if up > 0.0 {
                                delta[i] -= up;
                                self.air[1][i] += up;
                            }
                        }
                        let j = self.wind_to[l][i];
                        if j == u32::MAX {
                            continue;
                        }
                        let mv = m * ADV_FRAC[l] * self.wind_w[l][i] * 0.5;
                        // The flow SPLITS over the two most-aligned
                        // neighbours — the lane-breaker.
                        let frac = self.wind_frac[l][i];
                        carry(
                            self,
                            &mut delta,
                            &mut rain_now,
                            l,
                            deck,
                            i,
                            j as usize,
                            mv * frac,
                        );
                        let j2 = self.wind_to2[l][i];
                        if j2 != u32::MAX {
                            carry(
                                self,
                                &mut delta,
                                &mut rain_now,
                                l,
                                deck,
                                i,
                                j2 as usize,
                                mv * (1.0 - frac),
                            );
                        }
                    }
                    for (a, d) in self.air[l].iter_mut().zip(&delta) {
                        *a = (*a + d).max(0.0);
                    }
                }
                // CONVERGENCE RAIN · SUBSIDENCE DRYING · SATURATION SPILL ·
                // the fold. The display moisture is the boundary deck.
                #[allow(clippy::needless_range_loop)] // parallel stores, one index
                for i in 0..n {
                    let c = self.conv[i];
                    if c > 0.0 {
                        for l in 0..AIR_LAYERS {
                            let r = self.air[l][i] * CONV_RAIN * c;
                            self.air[l][i] -= r;
                            rain_now[i] += r;
                        }
                    } else if c < 0.0 {
                        for l in (1..AIR_LAYERS).rev() {
                            let down = self.air[l][i] * SUBSIDE * (-c);
                            self.air[l][i] -= down;
                            self.air[l - 1][i] += down;
                        }
                    }
                    for l in 0..AIR_LAYERS {
                        let over = self.air[l][i] - AIR_CAP[l];
                        if over > 0.0 {
                            let r = over * SAT_RAIN;
                            self.air[l][i] -= r;
                            rain_now[i] += r;
                        }
                    }
                }
                // THE STORM FOOTPRINT: rain falls in WEATHER, not on a
                // single hex — each tick's fall spreads half over its ring
                // before folding, so windward walls water their valleys,
                // catchments connect, and rivers grow long enough to live.
                {
                    let mut spread = vec![0.0f32; n];
                    for t in 0..n as TileId {
                        let i = t as usize;
                        let r = rain_now[i];
                        if r <= 1e-6 {
                            continue;
                        }
                        spread[i] += r * 0.5;
                        let nbs = map.neighbours(t);
                        let share = r * 0.5 / nbs.len() as f32;
                        for nb in nbs {
                            spread[*nb as usize] += share;
                        }
                    }
                    rain_now = spread;
                }
                // THE FOLD: the spread fall into the rain EMA, the display
                // moisture, and the vegetation's slow answer.
                let full = VEG_RAIN_FULL * self.veg_thirst;
                let (mut land_n, mut green_n) = (0usize, 0usize);
                #[allow(clippy::needless_range_loop)] // parallel stores, one index
                for i in 0..n {
                    self.rain[i] = self.rain[i] * (1.0 - RAIN_BLEND) + rain_now[i] * RAIN_BLEND;
                    self.moist[i] = (self.air[0][i] / AIR_VIS).clamp(0.0, 1.0);
                    // VEGETATION: cover creeps toward what the sustained
                    // rain can feed AT THE STOCK'S CURRENT THIRST — greening
                    // where the weather delivers, desertification where it
                    // stops, drowned under the sea.
                    let target = if self.ground(i as TileId) < sea {
                        0.0
                    } else {
                        land_n += 1;
                        if self.veg[i] >= GREEN_COVER {
                            green_n += 1;
                        }
                        (self.rain[i] / full).clamp(0.0, 1.0)
                    };
                    self.veg[i] += (target - self.veg[i]) * VEG_GROW;
                }
                // THE GREEN TARGET's controller: the flora's thirst walks
                // until the greened share of standing land meets the dial —
                // hardier stock reaches further down the rain gradient,
                // thirstier stock retreats toward the wet cores.
                if land_n > 0 {
                    self.green_share = green_n as f32 / land_n as f32;
                    let err = self.veg_target - self.green_share;
                    self.veg_thirst = (self.veg_thirst
                        * (1.0 - (VEG_ADAPT * err).clamp(-0.02, 0.02)))
                    .clamp(VEG_THIRST_MIN, VEG_THIRST_MAX);
                }
            }
            Phase::Upwell => {
                // 1 — UPWELLING + ERUPTIONS, in the MANTLE's frame — a moving plate
                // slides away from the sources, which is the whole chain story.
                // DISCRETE, near-imperceptible ticks (Aaron 2026-08-25): the upwell
                // zones leak a dozen pinches per seam per tick — sparse sampled
                // tiles, never the rejected every-hot-tile rain — and most ticks NO
                // volcano fires; occasionally one vent floods its locality with a
                // whole lava flow. Seeded by the tick + the roll: deterministic.
                let mut rng = fastrand::Rng::with_seed(
                    seams
                        .seed()
                        .wrapping_add(self.ticks.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                );
                // VOLCANOES ONLY (Aaron 2026-08-25): the deep crust is the BUFFER —
                // molten material reaches this layer nowhere but the vents that broke
                // through it. Underwater spreading, island chains, subduction ranges:
                // all of it is volcanoes, and the vent list IS the "dozen or so tiles
                // along the hot zones" — clustered chains on the seams, junction
                // fields, hot-spot fields. Production SAMPLES the vents: a dozen
                // pinches per seam per tick across the whole planet.
                let vents = crust.vents();
                if self.emitted.len() != vents.len() {
                    self.emitted = vec![0.0; vents.len()];
                    // THE PROVINCES: a smooth seeded field over the sphere
                    // hands each vent its characteristic grade — kindred
                    // vents pour kindred rock, so hardness contrast arrives
                    // in belts the size of ranges, not per-vent speckle.
                    let mut pr = fastrand::Rng::with_seed(seams.seed() ^ 0x5052_4F56_494E_4345);
                    let axes: Vec<(Vec3, f32)> = (0..3)
                        .map(|_| {
                            let v = Vec3::new(
                                pr.f32() * 2.0 - 1.0,
                                pr.f32() * 2.0 - 1.0,
                                pr.f32() * 2.0 - 1.0,
                            );
                            let v = if v.length_squared() < 1e-6 {
                                Vec3::X
                            } else {
                                v
                            };
                            (v.normalize(), pr.f32() * std::f32::consts::TAU)
                        })
                        .collect();
                    self.vent_grade = vents
                        .iter()
                        .enumerate()
                        .map(|(k, v)| {
                            let d = map.direction(*v);
                            let s = axes
                                .iter()
                                .map(|(a, ph)| (PROVINCE_FREQ * d.dot(*a) + ph).sin())
                                .sum::<f32>()
                                / 1.8;
                            let s = s.clamp(-1.0, 1.0) * 0.5 + 0.5;
                            let jit = (fastrand::Rng::with_seed(
                                seams.seed() ^ ((k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                            )
                            .f32()
                                * 2.0
                                - 1.0)
                                * PROVINCE_JITTER;
                            (VENT_HARD_MIN + VENT_HARD_SPAN * s + jit).clamp(0.4, 1.9)
                        })
                        .collect();
                }
                if !vents.is_empty() {
                    for _ in 0..(seams.cells() * UPWELL_PER_SEAM) {
                        let vi = rng.usize(..vents.len());
                        let t = vents[vi] as usize;
                        let hard = self.vent_hardness(vi);
                        let inject = UPWELL_INJECT;
                        // THE FOUNTAIN PAYS FROM THE WELL: the pinch's rate
                        // is untouched, but above the decompression floor
                        // this rock is what the collisions sank coming back
                        // up, not fresh mint.
                        self.draw_melt(t, inject);
                        let old = self.rock[t].max(0.0);
                        self.rock_hard[t] =
                            (old * self.rock_hard[t] + inject * hard) / (old + inject);
                        self.rock[t] += inject;
                        self.emitted[vi] += inject;
                        act[t] = true;
                    }
                }
                if !crust.vents().is_empty() && rng.f32() < PLANET_ERUPT_CHANCE {
                    let vi = rng.usize(..crust.vents().len());
                    let vent = crust.vents()[vi];
                    let erupt_hard = self.vent_hardness(vi);
                    self.eruptions += 1;
                    // The flow: ring by ring out from the vent, one tick's whole
                    // lava field.
                    let mut dist = vec![u8::MAX; n];
                    dist[vent as usize] = 0;
                    let mut ring = vec![vent];
                    let pour = |slf: &mut Self, i: usize, m: f32| {
                        slf.draw_melt(i, m); // the eruption draws too
                        let old = slf.rock[i].max(0.0);
                        slf.rock_hard[i] =
                            (old * slf.rock_hard[i] + m * erupt_hard) / (old + m).max(1e-6);
                        slf.rock[i] += m;
                    };
                    pour(self, vent as usize, ERUPT_FLOW[0]);
                    self.emitted[vi] += ERUPT_FLOW[0];
                    act[vent as usize] = true;
                    for d in 1..ERUPT_FLOW.len() as u8 {
                        let mut next = Vec::new();
                        for t in ring {
                            for nb in map.neighbours(t) {
                                let j = *nb as usize;
                                if dist[j] == u8::MAX {
                                    dist[j] = d;
                                    pour(self, j, ERUPT_FLOW[d as usize]);
                                    act[j] = true;
                                    next.push(*nb);
                                }
                            }
                        }
                        ring = next;
                    }
                }
            }
            Phase::Spread => {
                // 2 — SPREAD: hot rock flows outward; how far a field can grow is the
                // heat's call. Two-pass: spills computed against the current state.
                let mut delta = vec![0.0f32; n];
                // FLOOD CONTROL: each cell's per-tick intake budget; a pressurised
                // receiver resists inflow, and whatever is refused BACKS UP at the
                // source — liquid rules, the boiling-water reading.
                let mut intake = vec![0.0f32; n];
                for t in 0..n as TileId {
                    if !act[t as usize] {
                        continue; // a standing tile spills nothing this tick
                    }
                    if crust.is_vent(t) {
                        // A VOLCANO BUILDS ITS CONE: a vent's pile never batch-spills
                        // — consolidation stacks it into strata and the slope laws
                        // (talus, rockfall) shed its flanks as real flows.
                        continue;
                    }
                    let r = self.rock[t as usize];
                    if r < SPILL_RELEASE {
                        continue; // filling silently — no event yet
                    }
                    let heat = seams.heat(t);
                    if heat <= 0.0 {
                        continue;
                    }
                    // NO plates (Aaron 2026-08-25): the upwelling pushes material,
                    // and where flows MEET we calculate collision — a receiver whose
                    // own push drives back against the incoming flow takes PRESSURE
                    // instead of material; everywhere else the batch flows downhill.
                    let p_dir = map.direction(t);
                    let oppose = OPPOSE_FRAC * RATE_MAX;
                    let (mut lower, mut against): (Vec<TileId>, Vec<TileId>) = (vec![], vec![]);
                    for nb in map.neighbours(t) {
                        let j = *nb as usize;
                        let toward = map.direction(*nb) - p_dir;
                        let dir = (toward - p_dir * p_dir.dot(toward)).normalize_or_zero();
                        if self.push[j].dot(dir) < -oppose {
                            against.push(*nb);
                        } else if self.rock[j] < r {
                            lower.push(*nb);
                        }
                    }
                    if lower.is_empty() && against.is_empty() {
                        continue;
                    }
                    let ways = (lower.len() + against.len()) as f32;
                    // The BATCH: the whole excess above the rest level, hot rock
                    // draining more completely than cold.
                    let each = (r - SPILL_REST) * (0.4 + 0.6 * heat) / ways;
                    let src_hard = self.rock_hard[t as usize];
                    for nb in lower {
                        let j = nb as usize;
                        let want = each / (1.0 + FLOOD_RESIST * self.pressure[j]);
                        let grant = want.min((INTAKE_CAP - intake[j]).max(0.0));
                        if grant > 0.0 {
                            delta[t as usize] -= grant;
                            // Volume moves: the receiver's height converts by area —
                            // and the HARDNESS blends in, mass-weighted.
                            let m = grant * (self.area[t as usize] / self.area[j]);
                            let old = self.rock[j].max(0.0);
                            self.rock_hard[j] =
                                (old * self.rock_hard[j] + m * src_hard) / (old + m).max(1e-6);
                            delta[j] += m;
                            intake[j] += grant;
                        }
                    }
                    for nb in against {
                        let j = nb as usize;
                        // The collision: opposing streams jam — both sides gain the
                        // pressure the resolve events will spend, and the EDGE
                        // LEDGER records the meeting (both sides of the seam).
                        self.pressure[t as usize] =
                            (self.pressure[t as usize] + each * RIM_PRESS).min(PRESSURE_MAX);
                        self.pressure[j] =
                            (self.pressure[j] + each * RIM_PRESS * 0.5).min(PRESSURE_MAX);
                        self.edge_flux[t as usize] += each;
                        self.edge_flux[j] += each * 0.5;
                    }
                }
                for (r, d) in self.rock.iter_mut().zip(&delta) {
                    *r = (*r + d).max(0.0);
                }
            }
            Phase::Collide => {
                // 3 — THE CRUST'S OWN EDGES (Aaron's two-layer law: the molten seam
                // never acts on the plate edge directly — only MATERIAL does). Rim
                // pressure was sourced above, by arrivals the edge refused; here the
                // pressure RESOLVES, in discrete staggered events:
                //   · UPLIFT — a quantum of rock where the trigger is crossed:
                //     compaction pushing the pile up (mountains at the rims);
                //   · ADVANCE — past the claim trigger the pressurised side takes
                //     the foreign tile: the standing column subducts as a pile
                //     (zero-loss) and the boundary moves under the material budget.
                *compressed = self.pressure.iter().map(|p| *p >= PRESSURE_FORM).collect();
                #[allow(clippy::needless_range_loop)] // parallel stores, one index
                for i in 0..n {
                    self.pressure[i] = self.pressure[i].min(PRESSURE_MAX);
                    if self.pressure[i] >= UPLIFT_TRIGGER {
                        self.pressure[i] -= UPLIFT_TRIGGER;
                        // The resolve: compaction-uplift — the quantum is
                        // GATHERED from the jam itself (this tile's loose,
                        // then its ring, then the ring beyond — near supplies
                        // first, and the sustained draw is what founders the
                        // flanks into the foredeep). Uplift CONVERTS the pile
                        // that collided; it never mints. (The first cut wrote
                        // `rock += QUANTUM` with no debit — creation
                        // compounding with material flux was the 3600-tick
                        // accelerating-growth disease.)
                        // A MATURE EDGE has LEVERAGE: the tracked persistence
                        // scales the quantum — a long-lived convergence line
                        // builds a range where a young jam lifts a hill.
                        let lever =
                            1.0 + EDGE_GAIN * (self.edge_age[i] as f32 / EDGE_AGE_REF).min(1.0);
                        let quantum = UPLIFT_QUANTUM * lever;
                        let mut need = quantum;
                        let from_sed = need.min(self.sediment[i]);
                        self.sediment[i] -= from_sed;
                        need -= from_sed;
                        if need > 0.0 {
                            let ring: Vec<TileId> = map.neighbours(i as TileId).to_vec();
                            let gather =
                                |need: &mut f32, slf: &mut Self, act: &mut Vec<bool>, j: usize| {
                                    let take_s = need.min(slf.sediment[j]);
                                    slf.sediment[j] -= take_s;
                                    *need -= take_s;
                                    let take_r = need.min(slf.rock[j]);
                                    slf.rock[j] -= take_r;
                                    *need -= take_r;
                                    if take_s + take_r > 0.0 {
                                        act[j] = true;
                                    }
                                };
                            for nb in &ring {
                                if need <= 0.0 {
                                    break;
                                }
                                gather(&mut need, self, act, *nb as usize);
                            }
                            for nb in &ring {
                                if need <= 0.0 {
                                    break;
                                }
                                for nb2 in map.neighbours(*nb) {
                                    if need <= 0.0 {
                                        break;
                                    }
                                    let j = *nb2 as usize;
                                    if j == i || ring.contains(nb2) {
                                        continue;
                                    }
                                    gather(&mut need, self, act, j);
                                }
                            }
                        }
                        let got = quantum - need;
                        if got > 0.0 {
                            // METAMORPHISM: the converted rock hardens by the
                            // bed grade under the jam — an indurated old
                            // marine floor collides into HARD mountains.
                            let grade =
                                (UPLIFT_HARD * self.bed_hard[i]).clamp(UPLIFT_HARD, META_HARD_CAP);
                            let old = self.rock[i].max(0.0);
                            self.rock_hard[i] =
                                (old * self.rock_hard[i] + got * grade) / (old + got);
                            self.rock[i] += got;
                            act[i] = true;
                            // THE FABRIC RECORDS THE FOLD (Phase 1): an
                            // orogen folds what it hardens, so the SAME
                            // resolve that metamorphoses the wedge rotates
                            // the consolidated beds under it. The trend is
                            // the geometry the sim already computed — the
                            // local convergence is this tile's push, and the
                            // strike runs PERPENDICULAR to it (the fold
                            // axis, `p × convergence`). The two sides of a
                            // front push against each other, so their axes
                            // are anti-parallel and the undirected strike
                            // LINE they record is the same: one trend across
                            // the whole front, derived, never authored. The
                            // dip climbs with the shortening ACTUALLY
                            // performed — `got`, which already carries the
                            // mature edge's leverage — and stops at the cap:
                            // beds fold, they never invert.
                            let conv = self.push[i].normalize_or_zero();
                            if conv.length_squared() > 0.5 {
                                let p = map.direction(i as TileId);
                                let (east, north) = Self::frame(p);
                                let axis = p.cross(conv);
                                self.strike[i] = axis
                                    .dot(north)
                                    .atan2(axis.dot(east))
                                    .rem_euclid(std::f32::consts::PI);
                                let fold = FOLD_RATE * got;
                                self.l3_dip[i] = (self.l3_dip[i] + fold).min(DIP_CAP);
                                self.l4_dip[i] = (self.l4_dip[i] + fold).min(DIP_CAP);
                            }
                        }
                    }
                    self.pressure[i] *= PRESSURE_DECAY;
                }
            }
            Phase::Push => {
                // 4 — LOCAL DRIFT: the molten push moves MATERIAL, within its own
                // plate, away from the sources. A tile that accrues a full hex of
                // travel fires once: its loose pile shifts one hex along the push —
                // and a pile the plate's own rim blocks converts its shove into RIM
                // PRESSURE instead (the two-layer law: the molten field never
                // touches the plate edge; only material does). Cold tiles never
                // accrue and never fire.
                {
                    let mut fired: Vec<TileId> = Vec::new();
                    for t in 0..n as TileId {
                        let i = t as usize;
                        let rate = self.push[i].length();
                        if rate <= 0.0 {
                            continue;
                        }
                        self.drift[i] += rate;
                        if self.drift[i] >= tile_step {
                            self.drift[i] -= tile_step;
                            fired.push(t);
                        }
                    }
                    if !fired.is_empty() {
                        self.steps += fired.len() as u64;
                        for t in fired {
                            let i = t as usize;
                            if self.rock[i] + self.sediment[i] < MOVE_MIN || crust.is_vent(t) {
                                continue;
                            }
                            let p = map.direction(t);
                            let dir = self.push[i].normalize_or_zero();
                            let mut best = (f32::MIN, i);
                            for nb in map.neighbours(t) {
                                let j = *nb as usize;
                                let toward = map.direction(*nb) - p;
                                let along =
                                    (toward - p * p.dot(toward)).normalize_or_zero().dot(dir);
                                if along > best.0 {
                                    best = (along, j);
                                }
                            }
                            let j = best.1;
                            if j == i {
                                continue;
                            }
                            // COLLISION where flows meet: a receiver whose own push
                            // drives back against this flow jams it into pressure;
                            // otherwise the pile rides, its hardness blending in.
                            if self.push[j].dot(dir) < -(OPPOSE_FRAC * RATE_MAX) {
                                // THE WEIGHT: a water-compacted receiving column
                                // resists harder — the same flow jams into more
                                // pressure against an indurated bed (old marine
                                // floor meeting a flow builds the bigger range).
                                let shove = (self.rock[i] + self.sediment[i])
                                    * RIM_PRESS
                                    * self.bed_hard[j];
                                self.pressure[i] = (self.pressure[i] + shove).min(PRESSURE_MAX);
                                self.pressure[j] =
                                    (self.pressure[j] + shove * 0.5).min(PRESSURE_MAX);
                                self.edge_flux[i] += shove;
                                self.edge_flux[j] += shove * 0.5;
                                // THE SHORTENING (Aaron 2026-08-27): the jam
                                // finally CONSUMES ground — the stalled mover
                                // scrapes a share of the foreland's ROCK onto
                                // its own wedge. Two columns become one
                                // taller one (a conserved transfer, area-true,
                                // hardness blending in) and the emptied
                                // foreland founders into the foredeep.
                                // ROCK ONLY (the 32400-tick towers' last
                                // feeder): sediment is the water cycle's
                                // currency — a wedge beside a delta must not
                                // vacuum the delta into a spike; the loose
                                // sea-bed mud stays with the water system.
                                let scrape_r = self.rock[j] * STACK_SHARE;
                                if scrape_r > 0.0 {
                                    // THE DISPOSITION (Aaron 2026-08-29, the
                                    // reinstated ledger): what the foreland
                                    // gives up is split by its own GRADE.
                                    // Consolidated material — a
                                    // metamorphosed wedge, an indurated bed
                                    // — ARRESTS and crumples onto the wedge
                                    // exactly as before; young mafic floor
                                    // below the metamorphic floor SUBDUCTS,
                                    // leaving the surface for the well
                                    // instead of blowing off the top.
                                    let sank = scrape_r * Self::sink_share(self.rock_hard[j]);
                                    let vol = sank * self.area[j];
                                    self.well += vol;
                                    self.sunk += vol;
                                    let m = (scrape_r - sank) * (self.area[j] / self.area[i]);
                                    if m > 0.0 {
                                        let old = self.rock[i].max(0.0);
                                        self.rock_hard[i] = (old * self.rock_hard[i]
                                            + m * self.rock_hard[j])
                                            / (old + m).max(1e-6);
                                        self.rock[i] += m;
                                    }
                                    self.rock[j] -= scrape_r;
                                    act[j] = true;
                                }
                                act[i] = true;
                            } else {
                                let ar = self.area[i] / self.area[j];
                                let m = self.rock[i] * ar;
                                let old = self.rock[j].max(0.0);
                                self.rock_hard[j] = (old * self.rock_hard[j]
                                    + m * self.rock_hard[i])
                                    / (old + m).max(1e-6);
                                self.rock[j] += m;
                                self.rock[i] = 0.0;
                                // Sediment STAYS — the water cycle's
                                // currency, the one law everywhere (Weld,
                                // the scrape, and now the conveyor): the
                                // drift carries CRUST; only water moves
                                // mud. (The 32400-tick towers' last feeder:
                                // conveyor chains swept whole delta sheets
                                // into the jams, uncapped.)
                                act[i] = true;
                                act[j] = true;
                            }
                        }
                    }
                }
            }
            Phase::Form => {
                // 5 — CONSOLIDATION: rarely, the pile becomes a LAYER. Narrow
                // conditions: enough rock AND either compression at a closing edge or
                // serious volcanic heat. Compaction keeps part of the height.
                for (t, squeezed) in compressed.iter().enumerate() {
                    if !act[t] {
                        continue;
                    }
                    let pressured = *squeezed || self.pressure[t] >= PRESSURE_FORM;
                    if self.rock[t] >= FORM_HEIGHT
                        && (pressured || seams.heat(t as TileId) >= FORM_HEAT)
                    {
                        self.rock[t] -= FORM_HEIGHT;
                        // THE SLOT LADDER (Aaron 2026-08-26): the CAUSE picks
                        // the crust slot — pressure consolidates the deep
                        // VEIN layer (L3), heat consolidates the VOLCANIC
                        // layer (L4) — and a full slot forces the next cell
                        // up; past L4 the mass stays loose above (the
                        // semi-permanent layers, L5 reserved untouched).
                        let mut mass = FORM_HEIGHT * FORM_KEEP;
                        // THE GRADE RIDES IN: a stratum inherits the hardness
                        // of the rock that consolidated into it, mass-blended
                        // — this is what makes the spectrum PERMANENT.
                        let g = self.rock_hard[t];
                        if pressured {
                            let room = (L3_CAP - self.l3_h[t]).max(0.0);
                            let take = mass.min(room);
                            if take > 0.0 {
                                self.l3_hard[t] = (self.l3_h[t] * self.l3_hard[t] + take * g)
                                    / (self.l3_h[t] + take);
                                // The new bed lies FLAT — the fabric dilutes
                                // on the same blend the grade rides.
                                self.l3_dip[t] =
                                    Self::bedded_flat(self.l3_dip[t], self.l3_h[t], take);
                                self.l3_h[t] += take;
                            }
                            mass -= take;
                        }
                        let room = (L4_CAP - self.l4_h[t]).max(0.0);
                        let take = mass.min(room);
                        if take > 0.0 {
                            self.l4_hard[t] =
                                (self.l4_h[t] * self.l4_hard[t] + take * g) / (self.l4_h[t] + take);
                            self.l4_dip[t] = Self::bedded_flat(self.l4_dip[t], self.l4_h[t], take);
                            self.l4_h[t] += take;
                        }
                        self.rock[t] += mass - take;

                        // VEIN NUCLEATION under the ridges (canon A4:
                        // distillation concentrates at the pressure sites): a
                        // sparse seeded lottery, kind drawn by the accretion
                        // budget's weights, DISCRETE by node separation.
                        if pressured && self.vein[t] == 0 && self.l3_h[t] >= VEIN_L3_MIN {
                            let h = (seams.seed() ^ VEIN_STREAM)
                                .wrapping_add((t as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                            if h.is_multiple_of(VEIN_LOTTERY) {
                                let kind = draw_kind(h / VEIN_LOTTERY, false);
                                self.nucleate(map, t as TileId, kind, h, false);
                            }
                        }
                    }
                }
                // STATIC SITES ACTIVATE the moment the vein layer reaches
                // them — the rolled worldwide floor, earned by the era.
                for k in 0..self.vein_sites.len() {
                    let (site, kind) = self.vein_sites[k];
                    let i = site as usize;
                    if self.vein[i] == 0 && self.l3_h[i] >= VEIN_L3_MIN {
                        let h = (seams.seed() ^ VEIN_STREAM)
                            .wrapping_add((i as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
                        self.nucleate(map, site, kind, h, false);
                    }
                }
                // VEIN SPREAD: a body grows through the forming layer — a
                // neighbour whose vein layer is thick enough adopts its
                // node's kind (a seeded hash carves the organic edge), and
                // the node STOPS at its seeded budget: multiple cells,
                // never a continent.
                let mut adopt: Vec<(usize, u16)> = Vec::new();
                for t in 0..n as TileId {
                    let i = t as usize;
                    if self.vein_node_of[i] == 0 {
                        continue;
                    }
                    let node = self.vein_node_of[i];
                    for nb in map.neighbours(t) {
                        let j = *nb as usize;
                        if self.vein[j] == 0 && self.l3_h[j] >= VEIN_L3_MIN * 0.7 {
                            let h = (seams.seed() ^ VEIN_STREAM)
                                .wrapping_add((j as u64).wrapping_mul(0xA24B_AED4_963E_E407));
                            if !h.is_multiple_of(3) {
                                adopt.push((j, node));
                            }
                        }
                    }
                }
                for (j, node) in adopt {
                    let ni = node as usize - 1;
                    if self.vein[j] == 0 && self.vein_nodes[ni].size < self.vein_nodes[ni].budget {
                        self.vein[j] = 1 + self.vein_nodes[ni].kind;
                        self.vein_node_of[j] = node;
                        self.vein_nodes[ni].size += 1;
                    }
                }
            }
            Phase::Erode => {
                // 6 — WEATHERING: the moisture the uplift's own condensation zones
                // provide (plus the base air and the soak of standing water) wears
                // the piles down, SOFTEST material first — sediment, young rock, then
                // strata, the base barely at all — and the spoil moves to the LOWEST
                // neighbour as sediment. Where sediment lies deep on flat or drowned
                // ground it CONSOLIDATES into a NEW stratum: the planet taking shape.
                // MOISTURE is atmospheric — a cheap global state pass (no material
                // moves, no tile "changes"): the sky does not care which ground was
                // busy this tick.
                // (The moisture field is the WEATHER's now — evaporation,
                // winds, lift and convergence write it; the weathering
                // below drinks the RAIN ledger instead of conjuring wet.)
                // THE STREAMS (state, not material): rainfall accumulates down the
                // steepest-descent network — every tile hands its gathered water to
                // its steepest lower neighbour, highest ground first. The discharge
                // is what the carving cuts by, and what a river view will draw.
                // The ORDER, each tile's flow TARGET and its drop are KEPT: the
                // carry sweep below routes the spoil down this same tree.
                let mut order: Vec<TileId> = (0..n as TileId).collect();
                order.sort_unstable_by(|a, b| self.ground(*b).total_cmp(&self.ground(*a)));
                let mut flow_to: Vec<u32> = vec![u32::MAX; n];
                let mut flow_drop: Vec<f32> = vec![0.0; n];
                {
                    // The seed is the RAIN (a drowned tile's water routes
                    // nowhere useful — a token seed keeps the network sane).
                    let mut disch: Vec<f32> = (0..n)
                        .map(|i| {
                            if self.ground(i as TileId) < sea {
                                0.05
                            } else {
                                self.rain[i] * RAIN_DISCH + 0.02
                            }
                        })
                        .collect();
                    for t in &order {
                        let h = self.ground(*t);
                        let mut best: Option<(f32, usize)> = None;
                        for nb in map.neighbours(*t) {
                            let drop = h - self.ground(*nb);
                            if drop > 0.0 && best.is_none_or(|(d, _)| drop > d) {
                                best = Some((drop, *nb as usize));
                            }
                        }
                        if let Some((drop, j)) = best {
                            disch[j] += disch[*t as usize];
                            flow_to[*t as usize] = j as u32;
                            flow_drop[*t as usize] = drop;
                        }
                    }
                    self.discharge = disch;
                }
                // ── THE FRONT WALKS UP THE TREE (Phase 2) ──
                // A base level that moves does not become erodible potential
                // everywhere at once. Every column carries the outlet
                // elevation it has actually ADJUSTED to, and each tick that
                // level chases its own downstream control — the bed the
                // stream runs onto, the SEA where the stream reaches drowned
                // ground, its own floor where nothing drains away.
                //
                // The chase is ASYMMETRIC by construction. A control that
                // RISES is adopted AT ONCE: drowning is immediate (the sea
                // covers what it covers, and the reclassification is
                // unchanged), and a level that comes up drives no incision
                // to rate-limit in the first place. A control that FALLS is
                // adopted at CELERITY — a share of the gap per tick that
                // grows with the root of the discharge and divides by the
                // grade of the face the channel has to cut, so hard rock
                // slows the front and a trunk river outruns a rill.
                //
                // The sweep runs the stream order in REVERSE — lowest
                // ground first, and every receiver is strictly lower than
                // its giver, so this is exactly downstream-first. That is
                // what makes it a FRONT rather than a broadcast: a column's
                // control cannot fall until the column below it has actually
                // cut, so the knickzone climbs one reach at a time.
                //
                // NOT frontier-gated: a signal that stopped at the edge of
                // the active set would be a front with holes in it. Nothing
                // here moves material, wakes a tile, or writes a store — it
                // is one scalar per column, chasing one number.
                for t in order.iter().rev() {
                    let i = *t as usize;
                    let ctrl = self.outlet_level(i, flow_to[i], sea);
                    let g = self.graded[i];
                    self.graded[i] = if ctrl >= g {
                        ctrl
                    } else {
                        let c = (CELERITY_K * self.discharge[i].sqrt()
                            / self.face_grade(i).max(0.1))
                        .min(CELERITY_MAX);
                        let next = g + (ctrl - g) * c;
                        // Arrived: snap, so settled ground reads a lag of
                        // EXACTLY 1.0 and erodes bit-for-bit as it always did.
                        if next - ctrl <= GRADE_SETTLED {
                            ctrl
                        } else {
                            next
                        }
                    };
                }
                let mut sed_delta = vec![0.0f32; n];
                // Fluvial spoil per tile (height units) — collected by the take
                // chain, ROUTED by the carry sweep instead of dumped on the ring.
                let mut fluvial = vec![0.0f32; n];
                for t in 0..n as TileId {
                    let i = t as usize;
                    // Active ground weathers — CHANNELS weather regardless (a
                    // river keeps cutting its valley), and CLIFF-GRADE relief
                    // on a LOOSE face never sleeps: past the calving
                    // threshold an unconsolidated face is a pending event
                    // whoever built it. (The 25200-tick towers: a
                    // burst-built needle whose deliveries stopped fell out
                    // of the frontier, and talus — gated on activity —
                    // never tore it down. The steepest drop is on hand from
                    // the stream pass; the WORKING slopes below the cliff
                    // line stay frontier-gated, so the tick stays
                    // imperceptible.)
                    //
                    // THE WAKE IS MATERIAL (0b, ruling R1): the era used to
                    // wake EVERY oversteep column, which is a result-shaper
                    // — it stood in for an equilibrium the world did not
                    // have, and it sanded away exactly the standing risers
                    // (canyon walls, terrace fronts) the shape of the world
                    // is made of. With the well bounding rock height, the
                    // wake narrows to what actually fails on its own: a
                    // face under the metamorphic floor. A CONSOLIDATED face
                    // may stand oversteep once nothing is touching it —
                    // while anything IS touching it (the frontier, a live
                    // channel) it weathers and calves exactly as before, so
                    // no failure path is removed, only the standing
                    // reminder to fail.
                    if !act[i]
                        && self.discharge[i] < CHANNEL_LIVE
                        && (flow_drop[i] <= STRATA_CLIFF || self.face_grade(i) >= ARREST_GRADE)
                    {
                        continue;
                    }
                    let h = self.ground(t);
                    // Downhill: EVERY lower neighbour with its true SLOPE — the drop
                    // divided by the actual spacing, normalized to the mean tile, so
                    // the ISEA creases' uneven spacing neither steepens nor flattens
                    // a crossing. One receiver carves a channel; the fan is what
                    // builds aprons and cones.
                    let p_dir = map.direction(t);
                    let downs: Vec<(usize, f32)> = map
                        .neighbours(t)
                        .iter()
                        .filter_map(|nb| {
                            let nh = self.ground(*nb);
                            if nh >= h {
                                return None;
                            }
                            let dist = (map.direction(*nb) - p_dir).length() / tile_step;
                            Some((*nb as usize, (h - nh) / dist.max(0.25)))
                        })
                        .collect();
                    let Some(deepest) = downs.iter().map(|(_, d)| *d).max_by(f32::total_cmp) else {
                        continue; // a pit sheds nothing
                    };
                    let drop_sum: f32 = downs.iter().map(|(_, d)| d).sum();
                    let area = &self.area;
                    let i_area = area[i];
                    // The CHANNEL takes most of the spoil (the steepest neighbour —
                    // where the stream actually runs); the remainder fans
                    // drop-weighted. Concentration is what carves: an even fan
                    // builds aprons, a channel cuts a valley.
                    let channel = downs
                        .iter()
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(j, _)| *j);
                    let fan = |sed_delta: &mut Vec<f32>, amount: f32| {
                        let amount = amount * (1.0 - CARRY_LOSS);
                        let (main, rest) = match channel {
                            Some(j) => {
                                sed_delta[j] += amount * CHANNEL_SHARE * (i_area / area[j]);
                                (amount * CHANNEL_SHARE, amount * (1.0 - CHANNEL_SHARE))
                            }
                            None => (0.0, amount),
                        };
                        let _ = main;
                        let spread = rest / drop_sum.max(1e-6);
                        for (j, d) in &downs {
                            // Volume moves: heights convert by the area ratio.
                            sed_delta[*j] += spread * d * (i_area / area[*j]);
                        }
                    };
                    let slope = deepest.min(SLOPE_CAP);
                    // Shed from the top of the column, hardness gating each material.
                    // DISCHARGE CARVES: a stream's budget grows with the root of its
                    // catchment — channels cut valleys where a drizzle only weathers.
                    let carve = 1.0 + CARVE_GAIN * self.discharge[i].sqrt();
                    let glacial = self.ice[i] >= ICE_ERODE_MIN;
                    // A GLACIER needs no rain and out-cuts any stream (Aaron: more
                    // aggressive) — but its spoil is FROZEN IN PLACE (more static):
                    // the till stays under the ice and releases only on retreat.
                    // The wetness is EARNED now: standing water soaks as
                    // ever; dry land weathers only as hard as the rain the
                    // weather actually delivers — deserts and rain-shadow
                    // interiors keep their plains.
                    let wet = if h < sea {
                        BASE_WET + SUBMERGED_WET
                    } else {
                        // The COVER binds the soil: full vegetation holds
                        // back most of the budget — deserts starve of rain,
                        // forests hold their ground, and the semi-arid
                        // middle erodes hardest.
                        (BASE_WET * 0.3 + RAIN_ERODE * self.rain[i]).min(1.0)
                            * (1.0 - VEG_SHIELD * self.veg[i])
                    };
                    // THE FRONT IS A RATE LIMIT ON THE SIGNAL THAT ALREADY
                    // TRAVELS (Phase 2): a fall the column has not yet
                    // adjusted to is not erodible potential yet. The
                    // water-cut budget is scaled by the SHARE of its
                    // outlet's fall the column HAS adjusted to — ground at
                    // grade reads exactly 1.0 (and `× 1.0` is bit-identical,
                    // so a settled world erodes precisely as it did before
                    // the front existed), a reach just below a fresh drop
                    // reads near 0 and recovers at the celerity above. It is
                    // a RATE MODIFIER on the take that already exists, on
                    // the Phase 1 fabric's precedent: same stores, same
                    // budget arithmetic, same spoil route, no cap moved
                    // (CA9045F5).
                    //
                    // The GLACIER is exempt: ice grades to its own snout and
                    // is famously willing to cut below sea level, so a
                    // shoreline it never touches has no business throttling
                    // it. And the DRY drains below — talus, rockfall, and
                    // the repose flow after them — never read the lag
                    // either: a face fails on its own geometry whether or
                    // not its river has heard the news, and no needle-killer
                    // may be slowed.
                    let fall = h - self.outlet_level(i, flow_to[i], sea);
                    let lag = if fall > GRADE_SETTLED {
                        ((h - self.graded[i]) / fall).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let mut budget = if glacial {
                        ERODE_RATE * ICE_SCOUR * slope
                    } else {
                        ERODE_RATE * wet * slope * carve * lag
                    };
                    let mut spoil = 0.0f32;
                    let take =
                        |store: &mut f32, hardness: f32, budget: &mut f32, spoil: &mut f32| {
                            let want = *budget * hardness;
                            let got = want.min(*store);
                            *store -= got;
                            *spoil += got;
                            *budget -= got / hardness.max(1e-6);
                        };
                    take(
                        &mut self.sediment[i],
                        HARD_SEDIMENT,
                        &mut budget,
                        &mut spoil,
                    );
                    if budget > 0.0 {
                        // The SPECTRUM matters here: a tile fed by a hard-pouring
                        // vent sheds slower than one fed soft — ridges of hard rock
                        // survive as the soft country around them washes out.
                        take(
                            &mut self.rock[i],
                            HARD_ROCK / self.rock_hard[i].max(0.1),
                            &mut budget,
                            &mut spoil,
                        );
                    }
                    // THE FABRIC STEERS THE CUT (Phase 1): where the
                    // deformation recorded a dip, the consolidated takes are
                    // scaled by how the stream meets the bedding. Flow ALONG
                    // the strike of a dipping bed runs the soft interbeds and
                    // cuts FASTEST; flow ACROSS one has to saw through the
                    // resistant bed and cuts SLOWEST. It is a RATE MODIFIER
                    // on the takes that already exist — the same stores, the
                    // same budget arithmetic, the same spoil route, no cap
                    // moved (CA9045F5). Ground with NULL fabric — everything
                    // that has never been shortened — reads a factor of
                    // exactly 1.0, and `× 1.0` leaves the take bit-identical
                    // to the pre-fabric world.
                    let align = if self.l3_dip[i] > 0.0 || self.l4_dip[i] > 0.0 {
                        self.strike_align(map, i, flow_to[i], p_dir)
                    } else {
                        0.0
                    };
                    let fabric = |dip: f32| {
                        (1.0 + ANISO_SPAN * (dip / DIP_CAP) * align)
                            .clamp(1.0 - ANISO_SPAN, 1.0 + ANISO_SPAN)
                    };
                    // The formed slots shed TOP-DOWN: the softer volcanic
                    // layer (L4) first, then the vein layer (L3) — harder by
                    // its own factor AND the marine grade, which is what
                    // keeps the ore bodies buried under a lid.
                    if budget > 0.0 && self.l4_h[i] > 0.0 {
                        take(
                            &mut self.l4_h[i],
                            HARD_STRATA * fabric(self.l4_dip[i])
                                / (self.bed_hard[i] * self.l4_hard[i]).max(0.1),
                            &mut budget,
                            &mut spoil,
                        );
                        if self.l4_h[i] <= 1e-4 {
                            self.l4_h[i] = 0.0;
                        }
                    }
                    if budget > 0.0 && self.l3_h[i] > 0.0 {
                        take(
                            &mut self.l3_h[i],
                            HARD_STRATA * HARD_L3_FACTOR * fabric(self.l3_dip[i])
                                / (self.bed_hard[i] * self.l3_hard[i]).max(0.1),
                            &mut budget,
                            &mut spoil,
                        );
                        if self.l3_h[i] <= 1e-4 {
                            self.l3_h[i] = 0.0;
                        }
                        // A vein whose host layer wears away is EXPOSED and
                        // gone — worn out of the world, honestly.
                        if self.vein[i] != 0 && self.l3_h[i] < VEIN_L3_MIN * 0.5 {
                            self.vein[i] = 0;
                        }
                    }
                    if budget > 0.0 {
                        // Only the headroom above the floor is erodible — clamping
                        // AFTER a deeper take would refund material from nothing.
                        let floor = OCEAN_BED_H_FRAC * 0.5;
                        let mut headroom = (self.base[i] - floor).max(0.0);
                        take(
                            &mut headroom,
                            HARD_BASE / self.bed_hard[i].max(0.1),
                            &mut budget,
                            &mut spoil,
                        );
                        self.base[i] = floor + headroom;
                    }
                    if glacial {
                        self.sediment[i] += spoil; // the till, held under the ice
                    } else {
                        // Water-cut spoil is RIVER LOAD: it rides the stream
                        // network (the carry sweep below), not the local ring.
                        fluvial[i] += spoil;
                    }
                    // DRY mass wasting: past the talus angle a face sheds its excess
                    // no matter how dry the air is — loose needles collapse into
                    // skirts.
                    let over = deepest - TALUS_SLOPE;
                    if over > 0.0 {
                        let mut waste = over * WASTE_RATE;
                        let from_sed = waste.min(self.sediment[i]);
                        self.sediment[i] -= from_sed;
                        waste -= from_sed;
                        let from_rock = waste.min(self.rock[i]);
                        self.rock[i] -= from_rock;
                        fan(&mut sed_delta, from_sed + from_rock);
                    }
                    // ROCKFALL: past cliff relief even CONSOLIDATED strata calve —
                    // the needle-killer. A vent column that densified into a spike
                    // loses its face to its neighbours as loose spoil, tick by tick,
                    // until the relief is a mountain instead of a needle.
                    let cliff = deepest - STRATA_CLIFF;
                    if cliff > 0.0 && (self.l4_h[i] > 0.0 || self.l3_h[i] > 0.0) {
                        let mut fall = (cliff * CLIFF_RATE).min(self.l4_h[i] + self.l3_h[i]);
                        let from_l4 = fall.min(self.l4_h[i]);
                        self.l4_h[i] -= from_l4;
                        fall -= from_l4;
                        self.l3_h[i] -= fall.min(self.l3_h[i]);
                        let fall = from_l4 + fall;
                        if self.l4_h[i] <= 1e-4 {
                            self.l4_h[i] = 0.0;
                        }
                        if self.l3_h[i] <= 1e-4 {
                            self.l3_h[i] = 0.0;
                        }
                        if self.vein[i] != 0 && self.l3_h[i] < VEIN_L3_MIN * 0.5 {
                            self.vein[i] = 0;
                        }
                        fan(&mut sed_delta, fall);
                    }
                }
                for (i, (sd, d)) in self.sediment.iter_mut().zip(&sed_delta).enumerate() {
                    *sd += d;
                    if d.abs() > ACT_EPS {
                        act[i] = true; // spoil landed: the receiver is live too
                    }
                }
                // ── DISSOLUTION: THE REMOVAL CHANNEL (Phase 3) ──
                // Everything above moves rock DOWNSLOPE. This takes it away
                // where it stands — the era's missing removal-in-place, and
                // the only way it can make karst: the rock leaves from
                // inside and below, and what is over it comes down after it.
                //
                // NOT act-gated and NOT slope-gated, on purpose. The
                // frontier is an activity heuristic for material that MOVES
                // between columns; nothing moves between columns here, and
                // the places dissolution matters most — a buried bed under a
                // quiet cover, the flat floor of a closed pit — are exactly
                // the places the frontier and the stream power never reach.
                // What gates it instead is the PROCESS CONDITION itself
                // (R2): soluble bed present, and water actually moving
                // through it. Two loads and two compares reject every other
                // column, the same full-sweep economy the marine press pays.
                for t in 0..n as TileId {
                    let i = t as usize;
                    let bed = self.l3_h[i];
                    // The soluble class, in era terms: marine-cemented bed
                    // over the carbonate line, short of the metamorphic
                    // floor, standing in the air where meteoric water can
                    // reach it. (A drowned bed is not being flushed — it is
                    // sitting in the sea the load ends up in.)
                    if bed <= 0.0
                        || self.bed_hard[i] < MARINE_CALCITE_HARD
                        || self.l3_hard[i] >= UPLIFT_HARD
                        || self.ground(t) < sea
                    {
                        continue;
                    }
                    // THE THROUGHPUT: rain soaking in plus the stream running
                    // over, less the wet floor under which rock only gets
                    // damp. This is the drain condition, and cutting it is
                    // what stops a doline growing.
                    let flow =
                        self.rain[i] + DISSOLVE_FLOW * self.discharge[i].sqrt() - DISSOLVE_WET;
                    if flow <= 0.0 {
                        continue;
                    }
                    // THE FABRIC STEERS THE DISSOLUTION exactly as it steers
                    // the cut (Phase 1): water runs the bedding planes, so a
                    // dipping bed is opened ALONG its strike and resists
                    // ACROSS it. Same recorded fabric, same factor, same
                    // clamp band, same null — a bed with no tilt dissolves
                    // isotropically and lowers a plateau uniformly, which is
                    // precisely why this phase needed Phase 1 first.
                    let f = if self.l3_dip[i] > 0.0 {
                        (1.0 + ANISO_SPAN
                            * (self.l3_dip[i] / DIP_CAP)
                            * self.strike_align(map, i, flow_to[i], map.direction(t)))
                        .clamp(1.0 - ANISO_SPAN, 1.0 + ANISO_SPAN)
                    } else {
                        1.0
                    };
                    let mut got = (DISSOLVE_RATE * flow * f).min(DISSOLVE_MAX).min(bed);
                    if got <= 0.0 {
                        continue;
                    }
                    // The last crumb of a bed leaves WITH the take, never by
                    // being snapped away: a channel that rounds material off
                    // a column without crediting the store is a leak, and
                    // this one has to close to 1e-8 every tick.
                    if bed - got <= 1e-4 {
                        got = bed;
                    }
                    self.l3_h[i] = bed - got;
                    // A vein whose host layer goes into solution is gone the
                    // same way it goes when the layer is worn off.
                    if self.vein[i] != 0 && self.l3_h[i] < VEIN_L3_MIN * 0.5 {
                        self.vein[i] = 0;
                    }
                    // The debit becomes WATER-BORNE VOLUME — no height, no
                    // relief, and from here it moves only with the water.
                    self.dissolved[i] += got * self.area[i];
                    if got > ACT_EPS {
                        act[i] = true;
                    }
                }
                // THE CARRY (Aaron 2026-08-27): the river load rides DOWN the
                // stream tree, highest first. Each tile's water has a finite
                // capacity ∝ discharge × slope: what exceeds it deposits at
                // the capacity break (fans at the range fronts, fills in the
                // basins), a live channel FLUSHES its own standing bed into
                // spare capacity (valleys stay open), and the SEA is base
                // level — every load that reaches drowned ground lies down as
                // the delta and bed the marine press will indurate. Volume
                // rides area-true; the ledger only moves, never mints.
                {
                    let mut load = vec![0.0f32; n];
                    // THE SECOND DENOMINATION rides the SAME tree (Phase 3):
                    // no new advection path exists or may exist — the water
                    // moves both currencies or neither.
                    let mut dis_load = vec![0.0f32; n];
                    // FLOOD CONTROL on the settle (the 32400-tick relapse:
                    // spread fans and wake laws only SLOWED the towers — the
                    // carry was the one transport exempt from the intake
                    // law, still landing multi-unit loads around every
                    // constriction while all the drains are rate-limited).
                    // A cell settles at most INTAKE_CAP of carried load per
                    // tick; what the ground refuses SUSPENDS at the
                    // depositing tile — water-borne, no height — and
                    // re-enters the flow next tick.
                    let mut settled = vec![0.0f32; n];
                    let place = |slf: &mut Self,
                                 act: &mut Vec<bool>,
                                 settled: &mut Vec<f32>,
                                 j: usize,
                                 vol: f32|
                     -> f32 {
                        let room = (INTAKE_CAP - settled[j]).max(0.0);
                        let want = vol / slf.area[j];
                        let put = want.min(room);
                        if put > 0.0 {
                            slf.sediment[j] += put;
                            settled[j] += put;
                            if put > ACT_EPS {
                                act[j] = true;
                            }
                        }
                        (want - put) * slf.area[j]
                    };
                    // Every deposit SPREADS (the sediment-tower fix): the
                    // cell keeps DELTA_KEEP, the rest fans drop-weighted to
                    // its lower ICE-FREE neighbours; every share obeys the
                    // settle budget, and the closure returns what the ground
                    // REFUSED so the caller can suspend it.
                    let deposit = |slf: &mut Self,
                                   act: &mut Vec<bool>,
                                   settled: &mut Vec<f32>,
                                   i: usize,
                                   vol: f32|
                     -> f32 {
                        if vol <= 0.0 {
                            return 0.0;
                        }
                        let t = i as TileId;
                        let h = slf.ground(t);
                        let downs: Vec<(usize, f32)> = map
                            .neighbours(t)
                            .iter()
                            .filter_map(|nb| {
                                let j = *nb as usize;
                                let drop = h - slf.ground(*nb);
                                (drop > 0.0 && slf.ice[j] < ICE_ERODE_MIN).then_some((j, drop))
                            })
                            .collect();
                        let drop_sum: f32 = downs.iter().map(|(_, d)| d).sum();
                        let keep = if drop_sum > 1e-6 {
                            vol * DELTA_KEEP
                        } else {
                            vol
                        };
                        let mut refused = place(slf, act, settled, i, keep);
                        if drop_sum > 1e-6 {
                            let rest = vol - keep;
                            for (j, dr) in &downs {
                                refused += place(slf, act, settled, *j, rest * dr / drop_sum);
                            }
                        }
                        refused
                    };
                    for t in &order {
                        let i = *t as usize;
                        let target = flow_to[i];
                        let drowned = self.ground(*t) < sea;
                        // ICE IS A GATE like the sea (the tower autopsy: the
                        // carry delivered trunk loads ONTO glaciers, whose
                        // damped drain trapped them while the altitude froze
                        // the summit harder): a load never rides onto an
                        // iced cell — it lies down at the glacier's gate as
                        // the moraine fan.
                        let blocked =
                            target == u32::MAX || self.ice[target as usize] >= ICE_ERODE_MIN;
                        // ── WHAT IS IN SOLUTION (Phase 3) ──
                        // The dissolved load rides this tree and no other.
                        // No capacity break touches it: a river drops its
                        // sand where its capacity fails and carries its ions
                        // to the end of the line. It comes out ONLY where
                        // the condition reverses, capped against the same
                        // intake budget the settle spends; what it cannot
                        // land stays in solution, here, and tries again.
                        let dis = dis_load[i] + self.dissolved[i];
                        if dis > 0.0 {
                            self.dissolved[i] = 0.0;
                            let left = if self.returns(map, i, target, sea) {
                                self.precipitate(act, &mut settled, i, dis)
                            } else {
                                dis
                            };
                            if drowned || blocked {
                                self.dissolved[i] = left;
                            } else {
                                dis_load[target as usize] += left;
                            }
                        }
                        let mut vol = load[i] + fluvial[i] * self.area[i] + self.suspend[i];
                        self.suspend[i] = 0.0;
                        if vol <= 0.0 {
                            continue;
                        }
                        if drowned || blocked {
                            let refused = deposit(self, act, &mut settled, i, vol);
                            self.suspend[i] += refused;
                            continue;
                        }
                        let cap = CARRY_K * self.discharge[i] * flow_drop[i];
                        if vol < cap && self.discharge[i] >= CHANNEL_LIVE {
                            // The river scours its own bed into the spare
                            // capacity — the flush that keeps a valley open.
                            let take =
                                ((cap - vol) * FLUSH_FRAC).min(self.sediment[i] * self.area[i]);
                            if take > 0.0 {
                                self.sediment[i] -= take / self.area[i];
                                vol += take;
                                if take / self.area[i] > ACT_EPS {
                                    act[i] = true;
                                }
                            }
                        } else if vol > cap {
                            // The capacity break: the excess lies down here,
                            // spread like every deposit — the braid.
                            let refused = deposit(self, act, &mut settled, i, vol - cap);
                            self.suspend[i] += refused;
                            vol = cap;
                        }
                        load[target as usize] += vol;
                    }
                }
                // SEDIMENT KEEPS MOVING — the aggressive distribution: what landed
                // this tick flows on downhill, fanning drop-weighted, until the
                // remaining slope is under the repose angle. Aprons become hills;
                // closed basins and drowned ground fill toward FLAT — the plains.
                for _ in 0..SED_FLOW_PASSES {
                    let mut flow = vec![0.0f32; n];
                    let mut intake = vec![0.0f32; n];
                    for t in 0..n as TileId {
                        let i = t as usize;
                        // NOT act-gated: water-borne sediment is the long-haul
                        // transport of the cycle — it keeps creeping downhill toward
                        // the sea until the land lies at repose. Self-limiting: a
                        // settled slope moves nothing, so only live fronts (a few
                        // hundred coastline and apron tiles) ever write.
                        // Under ice the loose ROCK is entrained in the flow too —
                        // glacial transport — so the moving mass is the whole pile.
                        let glacial = self.ice[i] >= ICE_ERODE_MIN;
                        let s = self.sediment[i] + if glacial { self.rock[i] } else { 0.0 };
                        if s < 0.01 {
                            continue;
                        }
                        // SUMMER MELT: an iced bed passes a damped share —
                        // meltwater carrying till out from under the glacier, so
                        // a polar column reaches equilibrium instead of trapping
                        // every arrival forever. ONE law for both hemispheres.
                        let melt_damp = if glacial { SUMMER_MELT_FLOW } else { 1.0 };
                        let h = self.ground(t);
                        let p_dir = map.direction(t);
                        let downs: Vec<(usize, f32)> = map
                            .neighbours(t)
                            .iter()
                            .filter_map(|nb| {
                                let dist = (map.direction(*nb) - p_dir).length() / tile_step;
                                let slope = (h - self.ground(*nb)) / dist.max(0.25);
                                (slope > SED_REPOSE).then_some((*nb as usize, slope))
                            })
                            .collect();
                        if downs.is_empty() {
                            continue; // settled: the land lies flat enough here
                        }
                        let drop_sum: f32 = downs.iter().map(|(_, d)| d).sum();
                        let out = s * SED_FLOW_FRAC * melt_damp;
                        let mut moved = 0.0f32;
                        for (j, d) in &downs {
                            // Flood control: the receiver's budget and pressure gate
                            // the flow; what it refuses stays at the source.
                            let want =
                                (out * d / drop_sum) / (1.0 + FLOOD_RESIST * self.pressure[*j]);
                            let grant = want.min((INTAKE_CAP - intake[*j]).max(0.0));
                            if grant > 0.0 {
                                moved += grant;
                                // Volume moves: convert by the area ratio.
                                flow[*j] += grant * (self.area[i] / self.area[*j]);
                                intake[*j] += grant;
                            }
                        }
                        if moved > 0.0 {
                            // Debit sediment first; under ice the remainder comes
                            // off the entrained rock, deposited downhill as the
                            // glacier's milled sediment.
                            let from_sed = moved.min(self.sediment[i]);
                            flow[i] -= from_sed;
                            let from_rock = moved - from_sed;
                            if from_rock > 0.0 {
                                self.rock[i] = (self.rock[i] - from_rock).max(0.0);
                                act[i] = true;
                            }
                        }
                    }
                    for (i, (sd, d)) in self.sediment.iter_mut().zip(&flow).enumerate() {
                        *sd = (*sd + d).max(0.0);
                        if d.abs() > ACT_EPS {
                            act[i] = true;
                        }
                    }
                }
                // Sediment settles into a NEW layer where it lies deep on flat or
                // drowned ground — marine beds and floodplain strata.
                for t in 0..n as TileId {
                    let i = t as usize;
                    if !act[i] || self.sediment[i] < SED_FORM {
                        continue;
                    }
                    let h = self.ground(t);
                    let low_h = map
                        .neighbours(t)
                        .iter()
                        .map(|nb| self.ground(*nb))
                        .fold(f32::MAX, f32::min);
                    if h < sea || (h - low_h) < SED_FLAT {
                        self.sediment[i] -= SED_FORM;
                        // Sedimentary strata are VEIN-layer mass (L3): the
                        // buried beds players dig to; overflow climbs the
                        // ladder. They form at the SOFT end of the spectrum —
                        // the marine press is what indurates them later.
                        let mut mass = SED_FORM * SED_KEEP;
                        let room = (L3_CAP - self.l3_h[i]).max(0.0);
                        let take = mass.min(room);
                        if take > 0.0 {
                            self.l3_hard[i] = (self.l3_h[i] * self.l3_hard[i] + take * SED_GRADE)
                                / (self.l3_h[i] + take);
                            // Settled beds are the flattest thing the world
                            // makes: the marine floor and the floodplain lay
                            // down horizontal, and the fabric dilutes to say
                            // so.
                            self.l3_dip[i] = Self::bedded_flat(self.l3_dip[i], self.l3_h[i], take);
                            self.l3_h[i] += take;
                        }
                        mass -= take;
                        let room4 = (L4_CAP - self.l4_h[i]).max(0.0);
                        let take4 = mass.min(room4);
                        if take4 > 0.0 {
                            self.l4_hard[i] = (self.l4_h[i] * self.l4_hard[i] + take4 * SED_GRADE)
                                / (self.l4_h[i] + take4);
                            self.l4_dip[i] = Self::bedded_flat(self.l4_dip[i], self.l4_h[i], take4);
                            self.l4_h[i] += take4;
                        }
                        self.rock[i] += mass - take4;
                        // THE SEDIMENTARY DEPOSITS (coal + calcium, no
                        // biosphere modelled): a sparse lottery on marine
                        // beds — deep-compacted beds precipitate CALCITE,
                        // softer basins bury COAL.
                        if h < sea && self.vein[i] == 0 && self.l3_h[i] >= VEIN_L3_MIN {
                            let hh = (seams.seed() ^ VEIN_STREAM)
                                .wrapping_add((i as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
                            if hh.is_multiple_of(MARINE_VEIN_LOTTERY) {
                                // Deep-compacted beds concentrate the
                                // carbonate side of the marine table; softer
                                // basins the buried-carbon side — the draw
                                // still runs the budget's weights.
                                let kind = if self.bed_hard[i] >= MARINE_CALCITE_HARD {
                                    vein_index_of(12).unwrap_or(0) // Calcite → Ca
                                } else {
                                    draw_kind(hh / MARINE_VEIN_LOTTERY, true)
                                };
                                self.nucleate(map, i as TileId, kind, hh, false);
                            }
                        }
                        if h < sea {
                            // A stratum formed UNDER the sea is a compaction event:
                            // the marine bed indurates a step of its headroom.
                            self.bed_hard[i] +=
                                (MARINE_HARD_CAP - self.bed_hard[i]).max(0.0) * MARINE_FORM_BUMP;
                        }
                    }
                }
            }
            Phase::Compact => {
                // 6b — MARINE COMPACTION, the water's ongoing press (Aaron
                // 2026-08-26): every drowned column's consolidated stack indurates a
                // little each tick — the standing depth is the pressure (saturating),
                // fresh sediment lying on the bed multiplies it (that spoil is the
                // material being pressed in), and the grade approaches its cap
                // asymptotically. SILENT state: no material moves, so it neither
                // joins the frontier nor wakes a ring — the counterweight ledger the
                // collisions will weigh when these cells are shoved into others.
                for t in 0..n as TileId {
                    let i = t as usize;
                    let depth = sea - self.ground(t);
                    if depth <= 0.0 {
                        continue; // dry land keeps the grade it earned
                    }
                    let press = (depth / MARINE_DEPTH_CAP).min(1.0);
                    let feed = 1.0 + MARINE_SED_BOOST * self.sediment[i].min(1.0);
                    self.bed_hard[i] += (MARINE_HARD_CAP - self.bed_hard[i]).max(0.0)
                        * MARINE_COMPACT_RATE
                        * press
                        * feed;
                }

                // 7 — MAX DENSITY, the tick's CLOSING LAW: however material arrived
                // this tick — merges, pressure uplift, injection, a slide's deposit —
                // loose material past the cap TRANSFORMS. The capped mass becomes a
                // compressed permanent stratum; the overflow stays loose above, the
                // next young layer (the 500+100 → quartz + fresh 100 story).
                #[allow(clippy::needless_range_loop)] // parallel stores, one index
                for t in 0..n {
                    if !act[t] {
                        continue;
                    }
                    while self.rock[t] + self.sediment[t] > LOOSE_CAP + FORM_HEIGHT {
                        let mut take = LOOSE_CAP;
                        let from_rock = take.min(self.rock[t]);
                        self.rock[t] -= from_rock;
                        take -= from_rock;
                        self.sediment[t] -= take.min(self.sediment[t]);
                        // The pressed mass carries its blend: the rock part
                        // at the pile's grade, the sediment part soft.
                        let g = (from_rock * self.rock_hard[t]
                            + (LOOSE_CAP - from_rock) * SED_GRADE)
                            / LOOSE_CAP;
                        let mut mass = LOOSE_CAP * DENSIFY;
                        let room = (L4_CAP - self.l4_h[t]).max(0.0);
                        let take = mass.min(room);
                        if take > 0.0 {
                            self.l4_hard[t] =
                                (self.l4_h[t] * self.l4_hard[t] + take * g) / (self.l4_h[t] + take);
                            // Pressed loose material is MASSIVE — it carries
                            // no fabric of its own, so the bed it joins can
                            // only flatten.
                            self.l4_dip[t] = Self::bedded_flat(self.l4_dip[t], self.l4_h[t], take);
                            self.l4_h[t] += take;
                        }
                        mass -= take;
                        // Past a full volcanic slot the excess stays LOOSE —
                        // L5 is reserved; the ladder ends at the working
                        // surface, and a saturated column stops compressing.
                        self.rock[t] += mass;
                        if mass > 0.0 {
                            break;
                        }
                    }
                }

                // 7b — THE BASAL CEILING: a root can only carry so much.
                // Past DELAM_LOAD the base FOUNDERS — the excess load is
                // stripped off the BOTTOM of the ladder and goes DOWN the
                // well, never up as height. The debit's other half, and it
                // only makes sense landed WITH the circuit: close the
                // circuit alone and crust climbs without bound, fix the
                // ceiling alone and it barely matters — the two faults
                // conceal each other (064F3B58).
                let strip = |h: &mut f32, g: f32, over: &mut f32, sank: &mut f32| {
                    if *over > 0.0 && *h > 0.0 && g > 1e-6 {
                        let take = (*over / g).min(*h);
                        *h -= take;
                        *over -= take * g;
                        *sank += take;
                    }
                };
                #[allow(clippy::needless_range_loop)] // parallel stores, one index
                for i in 0..n {
                    if !act[i] {
                        continue;
                    }
                    let mut over = self.overburden(i) - DELAM_LOAD;
                    if over <= 0.0 {
                        continue;
                    }
                    self.delaminations += 1;
                    let (g2, g3, g4) = (
                        self.bed_hard[i],
                        self.bed_hard[i] * self.l3_hard[i],
                        self.bed_hard[i] * self.l4_hard[i],
                    );
                    let mut sank = 0.0;
                    strip(&mut self.base[i], g2, &mut over, &mut sank);
                    strip(&mut self.l3_h[i], g3, &mut over, &mut sank);
                    strip(&mut self.l4_h[i], g4, &mut over, &mut sank);
                    let vol = sank * self.area[i];
                    self.well += vol;
                    self.sunk += vol;
                }
            }
            Phase::Weld => {
                // 8 — COLD WELD, the loose-rock sink: an active tile away from the
                // heat whose pile has come to rest joins the plate — base thickens,
                // the loose stores empty, ONE last change and then silence. Sediment
                // welds with it on cold quiet ground (thin marine skins and aprons
                // become the bed's own thickness).
                #[allow(clippy::needless_range_loop)] // parallel stores, one index
                for t in 0..n {
                    if !act[t] {
                        continue;
                    }
                    if crust.is_vent(t as TileId) {
                        continue; // a volcano's own tile never welds shut here
                    }
                    // ROCK only: sediment is the water cycle's currency — it keeps
                    // flowing and consolidates through its own marine-bed path,
                    // never silently into the plate.
                    if self.rock[t] <= 0.0 || self.rock[t] >= SPILL_RELEASE {
                        continue; // empty, or still tall enough to spill again
                    }
                    let room = (BASE_CAP - self.base[t]).max(0.0);
                    let weld = self.rock[t].min(room);
                    if weld > 0.0 {
                        self.base[t] += weld;
                        self.rock[t] -= weld;
                    }
                }

                // THE EDGE LEDGER FOLDS (Aaron 2026-08-27): this cycle's
                // collision flux into the EMA; a live edge AGES, a quiet one
                // DIES — the crust-side seams, tracked from first contact to
                // last. The tracking itself stays SILENT — the EMA and the
                // age move nothing.
                //
                // …and THE TRENCH EATS (0b): where an edge is LIVE, the
                // disposition reaches the column's BASEMENT. 0a hung the
                // debit on the jam EVENT and reached only the dusting of
                // loose rock on the foreland — ~0.1% of production, so the
                // well sat empty and mean ground kept climbing. A
                // convergence is a PLACE, not an instant, and it consumes
                // PLATE: this is where nearly all of the era's material
                // settles (weld thickens the base, the max-density law
                // fills the slots). Each slot founders by the grade it
                // consolidated at — the SAME weights the overburden reads —
                // bottom-up, exactly as the basal ceiling strips: down,
                // never up. The grade does the discriminating on its own:
                // the metamorphic wedge on one side of the trench is
                // buoyant and stands, the young unpressed floor on the
                // other goes under. Attached to the collision-edge detector
                // the reinstatement ruling names (484129EB), inside the
                // sweep that ledger already runs — no second pass.
                //
                // TRANSPORT AUDIT (CA9045F5): the store moved is CRUST
                // (base and the consolidated slots) — never sediment, which
                // stays the water cycle's currency. Flood control does not
                // apply: nothing is delivered onto a column, the material
                // leaves for an aggregate ledger that carries no height.
                for i in 0..n {
                    self.edge[i] =
                        self.edge[i] * (1.0 - EDGE_BLEND) + self.edge_flux[i] * EDGE_BLEND;
                    self.edge_flux[i] = 0.0;
                    self.edge_age[i] = if self.edge[i] >= EDGE_LIVE {
                        self.edge_age[i].saturating_add(1)
                    } else {
                        0
                    };
                    if self.edge_age[i] == 0 {
                        continue;
                    }
                    let founder = |h: &mut f32, g: f32| -> f32 {
                        let down = *h * SLAB_SHARE * Self::sink_share(g);
                        *h -= down;
                        down
                    };
                    let bed = self.bed_hard[i];
                    let slab = founder(&mut self.base[i], bed)
                        + founder(&mut self.l3_h[i], bed * self.l3_hard[i])
                        + founder(&mut self.l4_h[i], bed * self.l4_hard[i]);
                    let vol = slab * self.area[i];
                    self.well += vol;
                    self.sunk += vol;
                }

                // THE FRONTIER REBUILDS from what actually changed: a tile whose
                // stores moved past ACT_EPS is live next tick, and so is its ring
                // (the neighbours now facing a new cliff or a new hollow). Writes
                // under the floor die here — the echo fades instead of ringing.
                let mut changed = 0usize;
                let mut next = vec![false; n];
                for i in 0..n {
                    let delta = (self.base[i] - snap_base[i])
                        .abs()
                        .max((self.rock[i] - snap_rock[i]).abs())
                        .max((self.sediment[i] - snap_sed[i]).abs())
                        .max((self.l3_h[i] + self.l4_h[i] - snap_strata[i]).abs());
                    if delta > ACT_EPS {
                        changed += 1;
                        next[i] = true;
                        // Only a MEANINGFUL change wakes the ring — a settling
                        // tile's last trims stay its own business.
                        if delta > RING_EPS {
                            for nb in map.neighbours(i as TileId) {
                                next[*nb as usize] = true;
                            }
                        }
                    }
                }
                self.changed = changed;
                self.frontier = next;
            }
        }
        if matches!(phase, Phase::Weld) {
            self.ticks += 1;
            // THE RESOURCE GUARANTEE (Aaron's ruled deviation, 2026-08-26):
            // at the bootstrap horizon, any quota site the era has not
            // reached on its own is ENSURED — the host layer is deposited
            // and the body is planted whole. Deterministic, once.
            if self.ticks >= BOOTSTRAP_TICKS && !self.resources_ensured {
                self.resources_ensured = true;
                self.ensure_resources(map, seams);
            }
        }
    }

    /// Deposit and plant every still-dormant quota site: the vein layer the
    /// site never earned is written in (buried under the column as ever), the
    /// node nucleates with its rolled kind and GROWS to its budget through
    /// the deposited host — a real multi-cell body, labelled like any other.
    fn ensure_resources(&mut self, map: &HexMap, seams: &SeamField) {
        for k in 0..self.vein_sites.len() {
            let (site, kind) = self.vein_sites[k];
            let i = site as usize;
            if self.vein[i] != 0 {
                continue; // the era got here first
            }
            let h = (seams.seed() ^ VEIN_STREAM)
                .wrapping_add((i as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
            self.l3_h[i] = self.l3_h[i].max(VEIN_L3_MIN + 0.15);
            // FORCED: the guarantee may not be refused — a quota body five
            // tiles from an organic one is fine under the ruled deviation.
            self.nucleate(map, site, kind, h, true);
            let Some(&node) = self.vein_node_of.get(i).filter(|n| **n != 0) else {
                continue;
            };
            let ni = node as usize - 1;
            // Grow the body ring by ring through deposited host rock.
            let mut ring: Vec<TileId> = vec![site];
            while self.vein_nodes[ni].size < self.vein_nodes[ni].budget && !ring.is_empty() {
                let mut next = Vec::new();
                'grow: for t in &ring {
                    for nb in map.neighbours(*t) {
                        let j = *nb as usize;
                        if self.vein[j] != 0 {
                            continue;
                        }
                        if self.vein_nodes[ni].size >= self.vein_nodes[ni].budget {
                            break 'grow;
                        }
                        self.l3_h[j] = self.l3_h[j].max(VEIN_L3_MIN + 0.15);
                        self.vein[j] = 1 + kind;
                        self.vein_node_of[j] = node;
                        self.vein_nodes[ni].size += 1;
                        next.push(*nb);
                    }
                }
                ring = next;
            }
        }
    }

    /// **CAPTURE the planet into a v2 `.epoch`** ([`crate::PlanetEpoch`]) — the
    /// bench's OUTPUT: the recipe that regenerates the static context
    /// (map, seams, and every seeded stream off `seams.seed()`) plus the
    /// era's path-dependent ledger. Derived fields (push, winds, areas,
    /// microclimate, vent grades) and transient cycle state (carry, cursor,
    /// edge flux, frontier) are deliberately NOT captured — [`Self::restore`]
    /// re-derives the former and a restored world stands still until
    /// disturbed, like a fresh one. Call this BETWEEN cycles (the commit path
    /// closes any open cycle first); mid-cycle carry state has no place in
    /// the file.
    pub fn capture(
        &self,
        map: &HexMap,
        seams: &SeamField,
        comment: impl Into<String>,
    ) -> crate::epochfile::PlanetEpoch {
        use crate::epochfile::{PlanetEpoch, PlanetEra, PlanetLedger, PlanetRecipe, VeinBody};
        let recipe = PlanetRecipe {
            freq: map.freq(),
            seed: seams.seed(),
            cells: seams.cells(),
            spots: seams.spots(),
        };
        let era = PlanetEra {
            ticks: self.ticks,
            eruptions: self.eruptions,
            steps: self.steps,
            heals: self.heals,
            water_volume: self.water_volume,
            ice_locked: self.ice_locked,
            climate_base: self.climate_base,
            temp: self.temp,
            deep_temp: self.deep_temp,
            greenhouse: self.greenhouse,
            water_target: self.water_target,
            veg_target: self.veg_target,
            veg_thirst: self.veg_thirst,
            green_share: self.green_share,
            resources_ensured: self.resources_ensured,
            well: self.well,
            sunk: self.sunk,
            delaminations: self.delaminations,
        };
        let ledger = PlanetLedger {
            base: self.base.clone(),
            l3_h: self.l3_h.clone(),
            l3_hard: self.l3_hard.clone(),
            l4_h: self.l4_h.clone(),
            l4_hard: self.l4_hard.clone(),
            strike: self.strike.clone(),
            l3_dip: self.l3_dip.clone(),
            l4_dip: self.l4_dip.clone(),
            graded: self.graded.clone(),
            dissolved: self.dissolved.clone(),
            rock: self.rock.clone(),
            rock_hard: self.rock_hard.clone(),
            sediment: self.sediment.clone(),
            bed_hard: self.bed_hard.clone(),
            pressure: self.pressure.clone(),
            edge: self.edge.clone(),
            edge_age: self.edge_age.clone(),
            drift: self.drift.clone(),
            suspend: self.suspend.clone(),
            ice: self.ice.clone(),
            sst: self.sst.clone(),
            discharge: self.discharge.clone(),
            moist: self.moist.clone(),
            rain: self.rain.clone(),
            veg: self.veg.clone(),
            vein: self.vein.clone(),
            vein_node_of: self.vein_node_of.clone(),
        };
        let veins = self
            .vein_nodes
            .iter()
            .map(|n| VeinBody {
                center: n.center,
                kind: n.kind,
                size: n.size,
                budget: n.budget,
            })
            .collect();
        PlanetEpoch::new(
            recipe,
            era,
            ledger,
            self.air.clone(),
            self.emitted.clone(),
            veins,
            comment,
        )
    }

    /// **RESTORE a captured planet** — the inverse of [`Self::capture`]. The
    /// caller rebuilds the static context from the file's recipe FIRST
    /// (`HexMap::new(recipe.freq)`, `SeamField::new(map, cells, spots, seed)`);
    /// this checks the handed context actually matches the recipe (loud,
    /// never a silent mis-restore), resets onto it — re-deriving every seeded
    /// field — then overwrites the durable ledger from the file. The restored
    /// world stands STILL (empty frontier) until something disturbs it, like
    /// a fresh world; `vein_sites` are reset's own seeded re-roll (identical
    /// by seed) and never stored.
    pub fn restore(
        &mut self,
        map: &HexMap,
        seams: &SeamField,
        crust: &CrustField,
        file: &crate::epochfile::PlanetEpoch,
    ) -> Result<(), String> {
        let r = &file.recipe;
        if map.freq() != r.freq
            || seams.seed() != r.seed
            || seams.cells() != r.cells
            || seams.spots() != r.spots
        {
            return Err(format!(
                "context is not the recipe's: map freq {} vs {}, seams (seed {:#x}, {} cells, \
                 {} spots) vs (seed {:#x}, {} cells, {} spots)",
                map.freq(),
                r.freq,
                seams.seed(),
                seams.cells(),
                seams.spots(),
                r.seed,
                r.cells,
                r.spots
            ));
        }
        if file.air.len() != AIR_LAYERS {
            return Err(format!(
                "file has {} air layers, the model runs {AIR_LAYERS}",
                file.air.len()
            ));
        }
        // The emission phases must span the derived crust's vents — a
        // mismatch would otherwise be silently ZEROED by the Upwell phase's
        // lazy sizing, exactly the quiet mis-restore this refuses.
        if file.emitted.len() != crust.vents().len() {
            return Err(format!(
                "file has {} vent emissions, the derived crust has {} vents",
                file.emitted.len(),
                crust.vents().len()
            ));
        }
        self.reset(map, seams);
        let l = &file.ledger;
        self.base = l.base.clone();
        self.l3_h = l.l3_h.clone();
        self.l3_hard = l.l3_hard.clone();
        self.l4_h = l.l4_h.clone();
        self.l4_hard = l.l4_hard.clone();
        // THE FABRIC, THE GRADED LEVEL AND THE DISSOLVED LOAD ARE ADDITIVE:
        // an epoch written before Phase 1 carries no fabric arrays at all,
        // one written before Phase 2 no graded levels, one written before
        // Phase 3 no dissolved store — and `reset()` has already laid this
        // world down flat, un-met and with nothing in solution. So a
        // pre-fabric planet restores as a NULL-FABRIC world, bit-for-bit the
        // world it was captured from; a pre-front planet restores UN-MET,
        // adopting each column's live outlet exactly on its first stream
        // pass — which is the same thing: a world at grade; and a
        // pre-dissolution planet restores with an EMPTY store, which is what
        // that planet had, because nothing had ever gone into solution.
        // Present arrays are validated to the recipe's tile count like every
        // other.
        for (dst, src) in [
            (&mut self.strike, &l.strike),
            (&mut self.l3_dip, &l.l3_dip),
            (&mut self.l4_dip, &l.l4_dip),
            (&mut self.graded, &l.graded),
            (&mut self.dissolved, &l.dissolved),
        ] {
            if !src.is_empty() {
                dst.clone_from(src);
            }
        }
        self.rock = l.rock.clone();
        self.rock_hard = l.rock_hard.clone();
        self.sediment = l.sediment.clone();
        self.bed_hard = l.bed_hard.clone();
        self.pressure = l.pressure.clone();
        self.edge = l.edge.clone();
        self.edge_age = l.edge_age.clone();
        self.drift = l.drift.clone();
        self.suspend = l.suspend.clone();
        self.ice = l.ice.clone();
        self.sst = l.sst.clone();
        self.discharge = l.discharge.clone();
        self.moist = l.moist.clone();
        self.rain = l.rain.clone();
        self.veg = l.veg.clone();
        self.vein = l.vein.clone();
        self.vein_node_of = l.vein_node_of.clone();
        self.air = file.air.clone();
        self.emitted = file.emitted.clone();
        self.vein_nodes = file
            .veins
            .iter()
            .map(|v| VeinNode {
                center: v.center,
                kind: v.kind,
                size: v.size,
                budget: v.budget,
            })
            .collect();
        let e = &file.era;
        self.ticks = e.ticks;
        self.eruptions = e.eruptions;
        self.steps = e.steps;
        self.heals = e.heals;
        self.water_volume = e.water_volume;
        self.ice_locked = e.ice_locked;
        self.climate_base = e.climate_base;
        self.temp = e.temp;
        self.deep_temp = e.deep_temp;
        self.greenhouse = e.greenhouse;
        self.water_target = e.water_target;
        self.veg_target = e.veg_target;
        self.veg_thirst = e.veg_thirst;
        self.green_share = e.green_share;
        self.resources_ensured = e.resources_ensured;
        self.well = e.well;
        self.sunk = e.sunk;
        self.delaminations = e.delaminations;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;
    use crate::plates::PlateField;
    use crate::seams::SeamField;

    fn world() -> (HexMap, SeamField, CrustField, PlateField) {
        let map = HexMap::new(MIN_FREQ);
        let seams = SeamField::new(&map, 6, 4, 42);
        let crust = CrustField::derive(&map, &seams);
        let plates = PlateField::new(&map, 12, 42);
        (map, seams, crust, plates)
    }

    /// **The planet epoch is a COMPLETE capture** — the format's one
    /// completeness gate: run a real era, capture, rebuild the static context
    /// from the file's own recipe, restore into a FRESH evolution, and the
    /// re-capture must equal the original byte-for-byte. Anything durable
    /// that capture misses, or restore fails to replay, breaks this equality
    /// — so a new ledger field that skips the format is a failing build, not
    /// a silent hole in every committed world.
    #[test]
    fn a_captured_planet_restores_to_an_identical_capture() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(70.0);
        e.set_water_target(0.70);
        e.set_veg_target(0.4);
        // A real stretch of era, then close the open cycle so the capture
        // stands between cycles (the commit path's own precondition).
        for t in 0..map.len() as TileId / 7 {
            e.disturb(&map, t * 7);
        }
        for _ in 0..40 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(e.ticks() > 0, "the era actually ran");

        let file = e.capture(&map, &seams, "round-trip gate");
        let json = file.to_json().expect("the planet serializes");
        let back = crate::epochfile::PlanetEpoch::from_json(&json).expect("the planet validates");

        // The reader's side of the contract: static context FROM THE RECIPE.
        let map2 = HexMap::new(back.recipe.freq);
        let seams2 = SeamField::new(
            &map2,
            back.recipe.cells,
            back.recipe.spots,
            back.recipe.seed,
        );
        let crust2 = CrustField::derive(&map2, &seams2);
        let mut e2 = Evolution::new(&map2, &seams2);
        e2.restore(&map2, &seams2, &crust2, &back)
            .expect("restore succeeds");
        assert_eq!(
            e2.capture(&map2, &seams2, "round-trip gate"),
            file,
            "restore replays every captured field"
        );

        // A mismatched context is refused LOUD, never silently mis-restored.
        let wrong = SeamField::new(&map2, back.recipe.cells, back.recipe.spots, 7);
        assert!(e2.restore(&map2, &wrong, &crust2, &back).is_err());
    }

    /// **The in-fall walks the world to its ocean share and holds** (Aaron
    /// 2026-08-26: land growth had drained the world under 30% water — new
    /// water arrives slowly while coverage stands below the target, and
    /// never drains). From a 30%-water start with a 70% target, coverage
    /// rises monotonically-ish toward the target and settles NEAR it without
    /// wild overshoot; with the target already met, the volume stands still.
    #[test]
    fn the_infall_holds_the_water_target() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_climate(1.0);
        // A planted TWO-TIER world with LOW relief (highs under the hot-world
        // glaciation altitude AND under the cliff line, so the never-sleeps
        // law leaves the staging alone): a bone-dry world at max heat pools
        // its first water as highland ice — honest physics, but this gate
        // isolates the in-fall itself, so it stages terrain no glacier can
        // claim and no talus will level.
        for i in (0..map.len()).step_by(2) {
            e.base[i] += 1.2;
        }
        e.set_water_target(0.70);
        let start = e.coverage();
        assert!(start < 0.5, "starts well under target: {start}");
        let mut last = start;
        for _ in 0..400 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
            let c = e.coverage();
            assert!(
                c >= last - 0.03,
                "coverage climbs, never drains: {last} -> {c}"
            );
            last = c;
        }
        // The stable claims: the in-fall RAISES coverage against the era's
        // own land growth (a fresh world's upwelling outruns any fixed pour,
        // so "reaches the target" is the real world's claim, not this lab's),
        // and while the deficit stands the ocean's claim keeps the freezer
        // from eating the imports.
        assert!(
            last > start + 0.15,
            "the in-fall lifts coverage against growing land: {start} -> {last}"
        );
        assert!(last < 0.82, "…without wild overshoot: {last}");
        let deficit = (e.water_target() - last).max(0.0);
        assert!(
            e.ice_locked <= e.water_volume * (ICE_MAX_LOCK - deficit * ICE_YIELD).max(0.2) + 1.0,
            "under a standing deficit the ice honours the shrunken ration"
        );

        // At (or above) target: no new water arrives.
        // The witness is the VOLUME ledger, not the level — the level still
        // rises when growing land displaces a met sea, and should.
        let mut m = Evolution::new(&map, &seams);
        m.set_climate(1.0);
        m.set_water(90.0);
        m.set_water_target(0.50);
        let v0 = m.water_volume;
        for _ in 0..30 {
            let sea = m.resolve_sea();
            m.tick(&map, &seams, &crust, sea);
        }
        assert!(
            m.water_volume <= v0 + 1e-3,
            "above target the sky is dry: {} -> {}",
            v0,
            m.water_volume
        );
    }

    /// **THE PLANET FINISHES GROWING — the maturation canary** (the
    /// 3600-tick "glacial spikes": sediment towers fed by an upwell that
    /// never faded; with the insulation fade retired in 0b, the WELL is the
    /// law that ends it). One long
    /// real-world run: land growth DECELERATES hard as columns thicken over
    /// their vents, no column outruns the crumble laws, and the ocean is
    /// never drowned out of existence by the land.
    #[test]
    fn the_planet_matures_and_no_sediment_tower_stands() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let mean_ground = |e: &Evolution| -> f32 {
            (0..map.len() as TileId).map(|t| e.ground(t)).sum::<f32>() / map.len() as f32
        };
        let run = |e: &mut Evolution, ticks: usize| {
            for _ in 0..ticks {
                let sea = e.resolve_sea();
                e.tick(&map, &seams, &crust, sea);
            }
        };
        run(&mut e, 300);
        let young = mean_ground(&e);
        run(&mut e, 300);
        let adolescent = mean_ground(&e);
        let early_rate = adolescent - young;
        run(&mut e, 900);
        let mature = mean_ground(&e);
        run(&mut e, 300);
        let late_rate = mean_ground(&e) - mature;
        // THE MINT DETECTOR: material creation must never COMPOUND. Before
        // the conserving-uplift fix this read 0.27 early vs 6.99 late (the
        // uplift minted rock from pressure, and pressure scaled with
        // material flux); honest injection is flat-or-fading. Full fade to
        // half needs ~10k ticks of insulation — out of test budget; the
        // regression this pins is acceleration.
        assert!(
            late_rate <= early_rate * 1.3 + 0.02,
            "growth never compounds: early {early_rate:.2}/300t, late {late_rate:.2}/300t"
        );
        // No sediment towers: the tallest column stays in mountain range,
        // not orbit (the 2400-tick autopsy found 114 units of sediment).
        let tallest = (0..map.len() as TileId)
            .map(|t| e.ground(t))
            .fold(0.0f32, f32::max);
        assert!(tallest < 20.0, "no tower: tallest column {tallest}");
        // …and the world keeps an OCEAN: land never drowns the sea out.
        assert!(
            e.coverage() > 0.25,
            "the ocean survives maturation: coverage {}",
            e.coverage()
        );
    }

    /// **The ocean carries its own heat by depth** (Aaron 2026-08-26,
    /// completing the banked three-layer water spec): the SURFACE band lags
    /// the air (thermal inertia — after a cold snap the sea is WARMER than
    /// the air above it, and sea ice therefore forms later than land ice);
    /// the DEEP ocean is ONE global reservoir — the ratified optimization: a
    /// scalar that creeps toward the surface mean inside its abyssal clamp,
    /// never a per-tile ledger; the SHALLOW band is their mix, derived and
    /// stored nowhere.
    #[test]
    fn the_ocean_lags_the_air_and_the_deep_is_one_reservoir() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.water_volume = map.len() as f32 * 1.5; // a real drowned world
        e.set_climate(0.9);
        for _ in 0..30 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let sea = e.resolve_sea();
        let wet = (0..map.len() as TileId)
            .find(|t| e.ground(*t) < sea && map.direction(*t).y.abs() < 0.3)
            .expect("an equatorial sea tile");

        // COLD SNAP: the air drops; the sea remembers the warmth.
        e.set_climate(0.1);
        let sea2 = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea2);
        let air = e.local_temp(wet, map.direction(wet), sea2);
        let (sst, mid, deep) = e.ocean_temps(wet);
        assert!(
            sst > air + 0.05,
            "the surface band lags the cold snap: sea {sst} vs air {air}"
        );
        assert!(
            (mid - (sst + deep) * 0.5).abs() < 1e-6,
            "shallow is the derived mix"
        );
        assert!(
            (DEEP_MIN..=DEEP_MAX).contains(&deep),
            "the abyss stays in its clamp: {deep}"
        );
        // The reservoir is ONE value — every tile reads the same deep.
        for t in [0u32, 7, 99, map.len() as TileId - 1] {
            assert_eq!(e.ocean_temps(t).2, deep, "one well-mixed reservoir");
        }
        // …and it BARELY moved through the snap: centuries, not seasons.
        let d0 = e.deep_ocean_temp();
        for _ in 0..10 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            (e.deep_ocean_temp() - d0).abs() < 0.05,
            "the deep creeps: {d0} -> {}",
            e.deep_ocean_temp()
        );

        // SEA ICE LAGS LAND ICE: right after the snap, an equally-cold dry
        // tile ices ahead of the wet one (the water's inertia holds the
        // freeze off), because ice over water keys to the SEA's temperature.
        let felt_wet = e.ocean_temps(wet).0;
        assert!(
            felt_wet > air,
            "what the ice feels over water is the water: {felt_wet} vs {air}"
        );
    }

    /// **The bombardment ends and the cap line is weather, not geometry**
    /// (Aaron 2026-08-26: deep time kept creeping to an ice ball — the
    /// in-fall must eventually STOP; and the polar cap edge was "exactly a
    /// circle, hard edged"). The water volume never exceeds the budget
    /// however long a deficit stands; and on a cold world the freeze line is
    /// RAGGED: iced and bare land tiles OVERLAP across a band of latitude
    /// instead of splitting at one circle.
    #[test]
    fn the_bombardment_ends_and_the_freeze_line_is_ragged() {
        let (map, seams, crust, _plates) = world();
        let cap = map.len() as f32 * WATER_BUDGET_DEPTH;
        let mut e = Evolution::new(&map, &seams);
        e.set_climate(0.9);
        e.set_water_target(0.999); // a bottomless ask: only the budget stops it
        for _ in 0..900 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.water_volume <= cap * 1.01,
            "the sky closes at the budget: {} of {cap}",
            e.water_volume
        );

        // The ragged line: freeze a world, then look at LAND tiles near the
        // cap fringe — the coldest bare tile sits POLEWARD of the warmest
        // iced tile: the two populations overlap across a real band.
        let mut c = Evolution::new(&map, &seams);
        c.water_volume = map.len() as f32 * 1.0;
        c.set_climate(0.45);
        for _ in 0..200 {
            let sea = c.resolve_sea();
            c.tick(&map, &seams, &crust, sea);
        }
        let sea = c.resolve_sea();
        let mut iced_min_lat = f32::MAX;
        let mut bare_max_lat = 0.0f32;
        for t in 0..map.len() as TileId {
            if c.ground(t) < sea {
                continue; // the sea edge has its own physics (SST lag)
            }
            let lat = map.direction(t).y.abs();
            // The young fringe counts: marginal tiles grow at the moisture
            // floor and take a while to thicken.
            if c.ice(t) > 0.01 {
                iced_min_lat = iced_min_lat.min(lat);
            } else {
                bare_max_lat = bare_max_lat.max(lat);
            }
        }
        assert!(iced_min_lat < f32::MAX, "a cap exists");
        assert!(
            bare_max_lat > iced_min_lat + 0.01,
            "the freeze line is a BAND, not a circle: iced from {iced_min_lat}, bare up to {bare_max_lat}"
        );
    }

    /// **The caps are the in-fall's first source** (Aaron 2026-08-26:
    /// "captured water becomes the source of infall rather than purely
    /// adding more"). A cold world with a big locked reserve and a coverage
    /// deficit draws its water DOWN FROM THE CAPS — locked falls, the
    /// standing sea rises — and imports from the sky only the shortfall:
    /// its volume grows measurably slower than a capless twin under the same
    /// deficit.
    #[test]
    fn the_caps_are_the_infalls_first_source() {
        let (map, seams, crust, _plates) = world();
        // The CAPPED world: deep cold grows a big locked reserve first.
        // A TWO-TIER world (a flat one drowns at any volume, so no deficit
        // can exist on it): half the tiles stand high, and a deep conserved
        // volume half-covers the rest — a real deficit AND a real reserve.
        let plant = |e: &mut Evolution| {
            for i in (0..map.len()).step_by(2) {
                e.base[i] += 3.0;
            }
            // Below the bombardment budget: the sky must still be open, or
            // neither world can import and the comparison reads 0 vs 0.
            e.water_volume = map.len() as f32 * 0.8;
        };
        let mut c = Evolution::new(&map, &seams);
        plant(&mut c);
        c.set_climate(0.10);
        c.set_water_target(0.0); // no deficit while the caps grow
        for _ in 0..120 {
            let sea = c.resolve_sea();
            c.tick(&map, &seams, &crust, sea);
        }
        assert!(c.ice_locked > 10.0, "a real reserve stands locked");

        // Now open a deficit and watch the sources: caps drain, sky trickles.
        let locked0 = c.ice_locked;
        let vol0 = c.water_volume;
        c.set_water_target(0.9);
        // The capless twin: same terrain and volume, same target, no reserve.
        let mut t = Evolution::new(&map, &seams);
        plant(&mut t);
        t.set_climate(1.0);
        t.set_water_target(0.9);
        let tvol0 = t.water_volume;
        for _ in 0..60 {
            let sea = c.resolve_sea();
            c.tick(&map, &seams, &crust, sea);
            let sea = t.resolve_sea();
            t.tick(&map, &seams, &crust, sea);
        }
        // Deep cold may REGROW what the deficit melts — the equilibrium law
        // is allowed to win that race; the promise is the SOURCE accounting
        // below, not net cap decline. The reserve must simply still be real.
        assert!(
            c.ice_locked > 1.0,
            "the caps persist: {locked0} -> {}",
            c.ice_locked
        );
        let import_capped = c.water_volume - vol0;
        let import_capless = t.water_volume - tvol0;
        assert!(
            import_capped < import_capless * 0.9,
            "captured water covers part of the ask: imported {import_capped} vs {import_capless}"
        );
    }

    /// **The bootstrap horizon ENSURES the resources** (Aaron's ruled
    /// deviation, 2026-08-26: this era skips generative phases, so abundance
    /// is guaranteed deterministically). Rolling a real world to the horizon:
    /// every vein kind has at least one living body, every demand-high kind
    /// (the bulk-crafting class) has several — broadly available — and no
    /// body anywhere outgrew the cap. Before the horizon nothing is forced;
    /// at it, the dormant quota sites are deposited and planted whole.
    #[test]
    fn the_bootstrap_horizon_ensures_the_resources() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(100.0);
        let sea = e.resolve_sea();
        while e.ticks() < BOOTSTRAP_TICKS {
            e.tick(&map, &seams, &crust, sea);
        }
        let mut per_kind = vec![0usize; vein_kinds().len()];
        for (_, k) in e.vein_nodes() {
            per_kind[k as usize] += 1;
        }
        for (i, kind) in vein_kinds().iter().enumerate() {
            assert!(
                per_kind[i] >= 1,
                "{} has at least one body in the world",
                kind.label
            );
            if kind.demand_high {
                assert!(
                    per_kind[i] >= 2,
                    "{} is bulk-crafting stock — broadly available, got {}",
                    kind.label,
                    per_kind[i]
                );
            }
        }
        assert!(
            e.vein_bodies()
                .iter()
                .all(|b| b.size <= NODE_MAX.max(b.budget)),
            "the guarantee plants bodies, never carpets"
        );
    }

    /// **The census counts the ledger** — one row per kind actually in the
    /// world, most-common first, totals exactly the veined hexes; empty on a
    /// fresh world; and the guaranteed world reports every quota kind.
    #[test]
    fn the_census_counts_the_ledger() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        assert!(e.vein_census().is_empty(), "a fresh world holds nothing");
        // The bootstrap guarantee plants the quotas; the census must agree
        // with the raw ledger exactly.
        e.ticks = BOOTSTRAP_TICKS;
        e.resources_ensured = false;
        e.ensure_resources(&map, &seams);
        let census = e.vein_census();
        assert!(!census.is_empty());
        let total: u32 = census.iter().map(|(_, c)| c).sum();
        let raw = e.vein.iter().filter(|v| **v > 0).count() as u32;
        assert_eq!(total, raw, "the census IS the ledger");
        for w in census.windows(2) {
            assert!(w[0].1 >= w[1].1, "most-common first");
        }
        let _ = crust;
    }

    /// **The vein table IS the registry** (Aaron 2026-08-26: veins for ALL
    /// materials). Every natural+harvestable compound is present — silver
    /// (argentite), uraninite, the salts, every gemstone — with exactly one
    /// exclusion: Pearl, biological, waiting on the biosphere pass. Labels
    /// are the extracted element where the registry names one, the compound
    /// name otherwise; weights follow the accretion budget (iron common,
    /// gold trace, gems rarer again); both genesis paths are non-empty.
    #[test]
    fn the_vein_table_is_the_whole_registry() {
        let kinds = vein_kinds();
        assert!(kinds.len() >= 29, "all materials: got {}", kinds.len());
        for id in [22, 21, 20, 23, 15, 18, 24, 25, 12, 43, 10] {
            assert!(
                vein_index_of(id).is_some(),
                "compound {id} is mineable (silver, uraninite, salts, gems…)"
            );
        }
        assert!(vein_index_of(51).is_none(), "pearl waits on the biosphere");
        for k in kinds {
            assert!(!k.label.is_empty(), "compound {} labels", k.compound);
            assert!(k.weight > 0.0);
        }
        let au = &kinds[vein_index_of(22).unwrap() as usize];
        let fe = &kinds[vein_index_of(15).unwrap() as usize];
        let dia = &kinds[vein_index_of(43).unwrap() as usize];
        assert_eq!(au.label, "Au");
        assert_eq!(fe.label, "Fe");
        assert_eq!(dia.label, "Diamond", "a gem carries its own name");
        assert!(fe.weight > au.weight, "iron is common, gold is trace");
        assert!(au.weight > dia.weight, "…and gems are rarer again");
        assert!(kinds.iter().any(|k| k.marine) && kinds.iter().any(|k| !k.marine));
    }

    /// **Veins form by the era's own distillation, and the cause picks the
    /// slot** (Aaron 2026-08-26; canon A4). Pressure consolidation fills the
    /// VEIN layer (L3) and — through a sparse seeded lottery — nucleates
    /// METAL veins that SPREAD into multi-cell bodies; heat consolidation
    /// fills the VOLCANIC layer (L4) and never nucleates; the rolled STATIC
    /// sites are min-separated across the world and activate the moment the
    /// vein layer reaches them; and the marine path buries COAL in soft beds
    /// and CALCITE in deep-compacted ones. Nodes report through
    /// `vein_nodes()` and die with their centre's vein.
    #[test]
    fn veins_form_under_pressure_and_the_cause_picks_the_slot() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let dry = -10.0; // a dry world: no marine path in this half

        // A PRESSED cluster: rock to consolidate + forming pressure. The
        // lottery is seeded per tile, so a 60-tile patch holds winners.
        let seed_tile: TileId = 500;
        let mut patch = vec![seed_tile];
        for r in 0..3 {
            let ring: Vec<TileId> = patch
                .iter()
                .flat_map(|t| map.neighbours(*t).to_vec())
                .collect();
            patch.extend(ring);
            patch.sort_unstable();
            patch.dedup();
            let _ = r;
        }
        for t in &patch {
            let i = *t as usize;
            e.rock[i] = FORM_HEIGHT + 0.2;
            e.pressure[i] = PRESSURE_FORM + 0.1;
            e.disturb(&map, *t);
        }
        for _ in 0..6 {
            e.tick(&map, &seams, &crust, dry);
            for t in &patch {
                let i = *t as usize;
                e.rock[i] = e.rock[i].max(FORM_HEIGHT + 0.2);
                e.pressure[i] = PRESSURE_FORM + 0.1;
                e.disturb(&map, *t);
            }
        }
        // The cause picked the slot: the pressed patch grew its VEIN layer.
        assert!(
            patch.iter().any(|t| e.layer3(*t) > 0.0),
            "pressure consolidates into L3"
        );
        // …and a vein nucleated and SPREAD into a body.
        let veined: Vec<TileId> = patch
            .iter()
            .copied()
            .filter(|t| e.vein(*t).is_some())
            .collect();
        assert!(!veined.is_empty(), "the pressed ridge nucleated a vein");
        assert!(
            veined.len() >= 2,
            "the node spans cells: {} veined tiles",
            veined.len()
        );
        // THE ANTI-CARPET (Aaron: a seven-thousand-mile field of gold is not
        // a vein): every body honours its seeded budget and the hard cap,
        // and the patch is nowhere near saturated.
        assert!(
            e.vein_bodies()
                .iter()
                .all(|b| b.size <= NODE_MAX.max(b.budget)),
            "no body outgrows its budget"
        );
        assert!(
            veined.len() < patch.len() / 2,
            "veins are concentrations, not a carpet: {} of {}",
            veined.len(),
            patch.len()
        );
        assert!(
            veined
                .iter()
                .all(|t| !vein_kinds()[e.vein(*t).unwrap() as usize].marine),
            "pressure veins come from the non-marine side of the table"
        );
        assert!(
            e.vein_nodes().count() >= 1,
            "the node reports for its billboard"
        );

        // HEAT consolidation fills L4 and nucleates nothing: a hot vent-free
        // tile with the same rock grows the volcanic layer only.
        let hot = (0..map.len() as TileId)
            .filter(|t| !crust.is_vent(*t) && !patch.contains(t))
            .max_by(|a, b| seams.heat(*a).total_cmp(&seams.heat(*b)))
            .expect("a hot tile");
        assert!(seams.heat(hot) >= FORM_HEAT, "the pick is genuinely hot");
        let mut h = Evolution::new(&map, &seams);
        // Level the neighbourhood too, so the Spread phase has no downhill
        // to drain the pile into before Form reads it.
        h.rock[hot as usize] = FORM_HEIGHT + 0.2;
        for nb in map.neighbours(hot) {
            h.rock[*nb as usize] = FORM_HEIGHT + 0.2;
        }
        h.disturb(&map, hot);
        h.tick(&map, &seams, &crust, dry);
        assert!(h.layer4(hot) > 0.0, "heat consolidates into L4");
        assert_eq!(h.layer3(hot), 0.0, "…never the vein layer");
        assert_eq!(h.vein(hot), None, "…and nucleates nothing");

        // THE STATIC SITES: rolled, min-separated, and earned on arrival.
        assert!(e.vein_sites.len() >= 8, "a worldwide floor of sites");
        let tile_r = 2.0 / (map.len() as f32).sqrt();
        let sep = VEIN_SITE_SEP_TILES * 2.0 * tile_r;
        for (i, (a, _)) in e.vein_sites.iter().enumerate() {
            for (b, _) in &e.vein_sites[i + 1..] {
                let d = map
                    .direction(*a)
                    .dot(map.direction(*b))
                    .clamp(-1.0, 1.0)
                    .acos();
                assert!(d >= sep * 0.999, "sites keep their separation");
            }
        }
        let (site, kind) = e.vein_sites[0];
        let mut st = Evolution::new(&map, &seams);
        assert_eq!(st.vein(site), None);
        st.l3_h[site as usize] = VEIN_L3_MIN + 0.1;
        st.rock[site as usize] = FORM_HEIGHT + 0.2;
        st.pressure[site as usize] = PRESSURE_FORM + 0.1;
        st.disturb(&map, site);
        st.tick(&map, &seams, &crust, dry);
        assert_eq!(
            st.vein(site),
            Some(kind),
            "the dormant site activates with ITS rolled kind"
        );

        // THE MARINE DEPOSITS: drowned flat beds bury coal; deep-compacted
        // beds precipitate calcite. A wide patch beats the sparse lottery.
        let mut m = Evolution::new(&map, &seams);
        let sea = 5.0;
        let mut coal = 0usize;
        let mut calc = 0usize;
        for t in 0..(map.len() as TileId) {
            let i = t as usize;
            if i.is_multiple_of(2) {
                m.sediment[i] = SED_FORM + 0.2;
                m.bed_hard[i] = if i.is_multiple_of(4) {
                    MARINE_CALCITE_HARD + 0.2
                } else {
                    1.0
                };
                m.disturb(&map, t);
            }
        }
        m.tick(&map, &seams, &crust, sea);
        let calcite = vein_index_of(12).expect("calcite is in the table");
        for t in 0..(map.len() as TileId) {
            if let Some(k) = m.vein(t) {
                assert!(
                    vein_kinds()[k as usize].marine,
                    "a marine bed only buries marine kinds"
                );
                if k == calcite {
                    calc += 1;
                } else {
                    coal += 1; // the soft-bed draw: coal, evaporites, opal
                }
            }
        }
        assert!(coal > 0, "soft marine beds bury sedimentary deposits");
        assert!(calc > 0, "deep-compacted beds precipitate calcite");
        // …and the buried-carbon path is IN the marine table.
        assert!(
            vein_kinds()[vein_index_of(25).expect("coal is in the table") as usize].marine,
            "coal is a marine kind"
        );
    }

    /// **The ice law is HEMISPHERICALLY BLIND — one function, no poles**
    /// (Aaron 2026-08-26, asked directly whether north and south had separate
    /// code: they never did; this pins it). For every tile, the local
    /// temperature at (x, y, z) equals the local temperature at (x, −y, z) on
    /// a mirrored column — the latitude term is y⁴ and nothing else in the
    /// law reads the sign.
    #[test]
    fn the_ice_law_is_hemispherically_blind() {
        let (map, seams, _crust, _plates) = world();
        let e = Evolution::new(&map, &seams);
        let sea = e.resolve_sea();
        for k in 0..64 {
            let a = k as f32 / 64.0 * std::f32::consts::TAU;
            for lat in [0.2f32, 0.5, 0.8, 0.95] {
                let r = (1.0 - lat * lat).sqrt();
                let north = Vec3::new(r * a.cos(), lat, r * a.sin());
                let south = Vec3::new(r * a.cos(), -lat, r * a.sin());
                // Same tile stores (tile 0), mirrored directions: the ONLY
                // input that differs is the sign of y.
                let tn = e.local_temp(0, north, sea);
                let ts = e.local_temp(0, south, sea);
                assert!(
                    (tn - ts).abs() < 1e-6,
                    "one law, both poles: {tn} vs {ts} at lat {lat}"
                );
            }
        }
    }

    /// **Ice FLOWS — no column outgrows its own glacier** (Aaron 2026-08-26:
    /// the 2400-tick south pole grew snow towers taller than the planet; the
    /// cap rose as one body). Heavy rock injection under deep ice for
    /// hundreds of ticks: the column's total ground stays BOUNDED and the
    /// mass demonstrably reaches the neighbourhood. (0b: the claim survived
    /// the removal of the overburden RAMP that used to be credited with it —
    /// what actually bounds the column is the summer melt draining into an
    /// intake-capped ring, which already outruns the delivery. The gate did
    /// not move; the ramp it named turned out not to be the mechanism.)
    #[test]
    fn thick_ice_flows_and_no_column_outgrows_its_glacier() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.water_volume = map.len() as f32 * 1.5; // a real ocean for the caps
        e.set_climate(0.05); // deep cold
        let pole = (0..map.len() as TileId)
            .filter(|t| !crust.is_vent(*t))
            .max_by(|a, b| {
                map.direction(*a)
                    .y
                    .abs()
                    .total_cmp(&map.direction(*b).y.abs())
            })
            .expect("tiles exist");
        for _ in 0..40 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(e.ice(pole) >= ICE_ERODE_MIN, "the pole is under ice");
        // The polar vent: inject a quantum of rock EVERY tick, far more than
        // the old sediment-only outlet could pass.
        let mut peak = 0.0f32;
        for _ in 0..300 {
            e.rock[pole as usize] += 0.35;
            e.disturb(&map, pole);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
            peak = peak.max(e.ground(pole));
        }
        let injected = 300.0 * 0.35;
        assert!(
            peak < 12.0,
            "the glacier moves the pile out — no tower: peak {peak} of {injected} injected"
        );
        // …and the mass went SOMEWHERE downhill, not into thin air: the
        // planet still holds it (ledger law) outside the source column.
        let column = e.ground(pole);
        assert!(
            column < injected * 0.2,
            "the column keeps only a fraction of what fell on it: {column}"
        );
    }

    /// **The summer melts the banks: frozen ground is an EQUILIBRIUM, not a
    /// trap** (Aaron 2026-08-26 — near-permanent ice was banking
    /// snow-mountains along the polar line). A drowned-in-ice column with a
    /// deep till pile still passes material downhill each cycle: its pile
    /// SHRINKS and its downhill neighbourhood GAINS — the melt outlet that
    /// keeps a polar column from trapping every arrival forever.
    #[test]
    fn the_summer_melt_drains_the_polar_banks() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // A REAL ocean (the young world is flat, so a percentile pour is a
        // film — and rationed cap growth correctly starves on a film): a
        // deep conserved volume for the cold to draw on.
        e.water_volume = map.len() as f32 * 1.5;
        e.set_climate(0.05); // deep cold: the pole is solidly iced
        let pole = (0..map.len() as TileId)
            .filter(|t| !crust.is_vent(*t))
            .max_by(|a, b| {
                map.direction(*a)
                    .y
                    .abs()
                    .total_cmp(&map.direction(*b).y.abs())
            })
            .expect("tiles exist");
        // Freeze the world in, then bank a pile on the polar tile.
        for _ in 0..40 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(e.ice(pole) >= ICE_ERODE_MIN, "the pole is under ice");
        e.sediment[pole as usize] = 2.0;
        e.disturb(&map, pole);
        let column_before = e.ground(pole);
        for _ in 0..25 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.sediment(pole) < 2.0 * 0.8,
            "the iced bank drains: {} of 2.0 left",
            e.sediment(pole)
        );
        // The column itself came DOWN — the material left the tile (the ring
        // is a waypoint in a flowing system, so the column total is the
        // honest witness that the outlet moved it out rather than converting
        // it in place).
        assert!(
            e.ground(pole) < column_before - 0.1,
            "the frozen column no longer traps its bank: {column_before} -> {}",
            e.ground(pole)
        );
    }

    /// **A tick is the completed CYCLE of procedures** (Aaron 2026-08-26).
    /// Stepping phase by phase: the label walks the roster in order, the tick
    /// counter stands still through the first eight procedures and increments
    /// exactly on the ninth — and a full `tick()` equals nine phase steps.
    #[test]
    fn the_tick_clusters_around_the_procedure_cycle() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let sea = e.resolve_sea();
        assert_eq!(e.ticks(), 0);
        for (k, want) in PHASES.iter().enumerate() {
            assert_eq!(
                e.current_phase(),
                *want,
                "the label walks the roster in order"
            );
            let done = e.tick_phase(&map, &seams, &crust, sea);
            assert_eq!(
                done,
                k == PHASES.len() - 1,
                "only the last procedure completes the cycle"
            );
            assert_eq!(
                e.ticks(),
                u64::from(k == PHASES.len() - 1),
                "the tick counts the CYCLE, not the step"
            );
        }
        assert_eq!(e.current_phase(), PHASES[0], "…and the cycle comes round");
    }

    /// **The ice age breathes, the caps follow the cold, and the frozen
    /// water is missing from the sea.** A cold baseline grows POLAR caps
    /// (high-latitude tiles ice, equatorial ones don't) and the resolved sea
    /// stands LOWER than the same volume stood unfrozen — with the FULL
    /// conservation held: standing water + locked ice = the poured volume.
    /// A hot baseline melts the caps back and the sea recovers. And the
    /// caps GROW FROM the water: they can never lock more than their share
    /// of what was actually poured.
    #[test]
    fn the_ice_age_breathes_and_locks_the_sea() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(60.0);
        e.set_climate(1.0); // hot: no caps
        for _ in 0..10 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let warm_sea = e.resolve_sea();
        assert!(
            (0..map.len() as TileId).all(|t| e.ice(t) < 1e-3),
            "a fully hot world grows no caps"
        );

        // COLD: the caps come, poleward first, and the sea drops.
        e.set_climate(0.15);
        for _ in 0..120 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let cold_sea = e.resolve_sea();
        let polar = (0..map.len() as TileId)
            .filter(|t| map.direction(*t).y.abs() > 0.9)
            .map(|t| e.ice(t))
            .fold(0.0f32, f32::max);
        let equatorial = (0..map.len() as TileId)
            .filter(|t| map.direction(*t).y.abs() < 0.15)
            .map(|t| e.ice(t))
            .fold(0.0f32, f32::max);
        assert!(polar > 0.05, "the poles carry ice: {polar}");
        assert!(
            polar > equatorial,
            "the caps are POLAR: pole {polar} vs equator {equatorial}"
        );
        assert!(
            cold_sea < warm_sea,
            "locked ice drops the sea: {warm_sea} -> {cold_sea}"
        );
        // FULL conservation: what stands plus what froze is what was poured.
        let standing = e.volume_below(cold_sea);
        assert!(
            (standing + e.ice_locked - e.water_volume).abs() < e.water_volume * 0.02,
            "standing {standing} + locked {} = poured {}",
            e.ice_locked,
            e.water_volume
        );
        assert!(
            e.ice_locked <= e.water_volume * ICE_MAX_LOCK * 1.001,
            "the caps never lock past their share"
        );

        // WARM again: the caps give the water back.
        e.set_climate(1.0);
        for _ in 0..200 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.resolve_sea() > cold_sea,
            "the melt hands the sea back: {cold_sea} -> {}",
            e.resolve_sea()
        );
    }

    /// **The water is the press: drowned columns indurate, dry land keeps
    /// its grade** (Aaron 2026-08-26). Under standing water the marine grade
    /// rises tick over tick — faster where fresh sediment lies on the bed
    /// (that spoil is the material being pressed in) — approaching its cap
    /// and never passing it; a dry column's grade never moves on its own;
    /// and the grade acts as WEIGHT: the same erosion budget takes LESS from
    /// a compacted stack than from a fresh one.
    #[test]
    fn water_compacts_the_beds_and_the_grade_is_weight() {
        let (map, seams, crust, _plates) = world();
        // A DEEP flood: the whole young world under two tiles of water.
        let mut e = Evolution::new(&map, &seams);
        let peak = (0..map.len() as TileId)
            .map(|t| e.ground(t))
            .fold(f32::MIN, f32::max);
        let sea = peak + 2.0;
        let wet: TileId = 0;
        let fed: TileId = 7; // any second tile — the flood covers them all
        assert_eq!(e.bed_hardness(wet), 1.0, "everything starts uncompacted");
        e.sediment[fed as usize] = 0.8;
        for _ in 0..40 {
            e.tick(&map, &seams, &crust, sea);
        }
        let (hw, hf) = (e.bed_hardness(wet), e.bed_hardness(fed));
        assert!(hw > 1.0, "a drowned bare bed indurates: {hw}");
        assert!(hf > hw, "a sediment-fed bed indurates faster: {hf} vs {hw}");
        assert!(
            hw <= MARINE_HARD_CAP && hf <= MARINE_HARD_CAP,
            "the cap holds"
        );

        // A DRY world: with no standing water anywhere, no grade moves —
        // dry land keeps whatever it earned (here: nothing).
        let mut d = Evolution::new(&map, &seams);
        let trough = (0..map.len() as TileId)
            .map(|t| d.ground(t))
            .fold(f32::MAX, f32::min);
        for _ in 0..10 {
            d.tick(&map, &seams, &crust, trough - 1.0);
        }
        assert!(
            (0..map.len() as TileId).all(|t| d.bed_hardness(t) == 1.0),
            "dry land's grade never moves on its own"
        );

        // THE WEIGHT: the same erosion takes less from a compacted stack.
        // Two synthetic twin columns above the sea, one indurated.
        let mut a = Evolution::new(&map, &seams);
        let mut b = Evolution::new(&map, &seams);
        let dry_t: TileId = 11;
        let t = dry_t as usize;
        for e2 in [&mut a, &mut b] {
            e2.l3_h[t] = 0.5;
            e2.l4_h[t] = 0.5;
            e2.disturb(&map, dry_t);
        }
        b.bed_hard[t] = MARINE_HARD_CAP; // the indurated twin
        for _ in 0..12 {
            a.tick(&map, &seams, &crust, trough - 1.0);
            b.tick(&map, &seams, &crust, trough - 1.0);
        }
        assert!(
            b.strata(dry_t).1 > a.strata(dry_t).1,
            "the compacted stack outlasts the fresh one: {} vs {}",
            b.strata(dry_t).1,
            a.strata(dry_t).1
        );
    }

    /// **The era is deterministic, sane and ALIVE.** Same roll, same state
    /// after the same ticks; no rock goes negative and nothing blows up; the
    /// world actually accumulates material; and the poles give every plate a
    /// TANGENTIAL velocity — the arrow the display draws.
    #[test]
    fn the_era_is_deterministic_alive_and_tangential() {
        let (map, seams, crust, _plates) = world();
        let mut a = Evolution::new(&map, &seams);
        let mut b = Evolution::new(&map, &seams);
        for _ in 0..30 {
            let e_sea = a.sea_level(71.0);
            a.tick(&map, &seams, &crust, e_sea);
            b.tick(&map, &seams, &crust, e_sea);
        }
        assert_eq!(a.ticks(), 30);
        assert_eq!(a.rock, b.rock, "same roll, same era");
        assert!(a.rock.iter().all(|r| r.is_finite() && *r >= 0.0));
        let total: f32 = a.rock.iter().sum();
        assert!(total > 1.0, "the world grew material: {total}");
        // The push field is LOCAL: tangential everywhere it exists, capped
        // at the band's ceiling, present around the heat and ABSENT in the
        // cold interiors — the seams drive the crust, nothing else moves.
        let (mut live, mut still) = (0usize, 0usize);
        for t in 0..map.len() as TileId {
            let v = a.push_at(t);
            let p = map.direction(t);
            assert!(v.dot(p).abs() < 1e-4, "push is tangential at {t}");
            assert!(v.length() <= RATE_MAX * 1.001, "push respects the band");
            if v.length() > 0.0 {
                live += 1;
            } else if seams.heat(t) < 0.05 {
                still += 1;
            }
        }
        assert!(live > 0, "the field pushes somewhere");
        assert!(still > 0, "…and truly cold ground does not creep");
        // Reset: back to the bare shell, the same derived field.
        let field: Vec<Vec3> = (0..map.len() as TileId).map(|t| a.push_at(t)).collect();
        a.reset(&map, &seams);
        assert_eq!(a.ticks(), 0);
        assert!(a.rock.iter().all(|r| *r == 0.0));
        for t in 0..map.len() as TileId {
            assert_eq!(a.push_at(t), field[t as usize], "reset keeps the field");
        }
    }

    /// **A TICK IS NEARLY IMPERCEPTIBLE — the law itself, measured.** Aaron
    /// (2026-08-25, after watching all 93K tiles churn every tick): the whole
    /// tick's footprint is HUNDREDS of tiles, not the planet. Over a real run
    /// the MEDIAN per-tick change count stays under 1/40th of the world
    /// (steps and eruptions spike singles), the QUIET ticks are genuinely
    /// small, and activity DIES OUT: a frontier with no new source shrinks.
    #[test]
    fn a_tick_is_nearly_imperceptible() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let mut counts: Vec<usize> = Vec::new();
        for _ in 0..60 {
            let sea = e.sea_level(71.0);
            e.tick(&map, &seams, &crust, sea);
            counts.push(e.changed_tiles());
        }
        let mut sorted = counts.clone();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        // ABSOLUTE hundreds — the count does not scale with the map: the
        // insertion budget (cells × a dozen) and its echo are the whole
        // typical tick, on a 23K test world exactly as on the 93K planet.
        assert!(
            median <= 900,
            "a typical tick moves hundreds, not the planet: median {median} (run: {:?})",
            &counts[..12]
        );
        assert!(median > 0, "…but the world is alive");
        // The quietest ticks are genuinely small — no ungated phase drags a
        // planet-wide pass along.
        assert!(
            sorted[5] < 400,
            "quiet ticks are genuinely quiet: {:?}",
            &sorted[..8]
        );
        // …and the activity SATURATES instead of ratcheting: the last third
        // of the run is not the runaway of the first third.
        let early: usize = counts[..20].iter().sum();
        let late: usize = counts[40..].iter().sum();
        assert!(
            late < early * 4,
            "activity saturates, never ratchets: early {early} late {late}"
        );
    }

    /// **Fields grow at the heat and STRETCH with the motion — the chain
    /// law.** Rock concentrates around the volcanism (vent tiles out-collect
    /// the world's median by far), and after enough ticks for several
    /// advection steps, the rock around a vent is displaced ALONG its plate's
    /// motion — the field trails away from the stationary plume the way an
    /// island chain trails a hot spot.
    #[test]
    fn fields_grow_at_the_heat_and_trail_along_the_motion() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // The derived motion is slow (a plate crosses a tile every ~10
        // ticks) — a chain needs a real span of steps to read.
        for _ in 0..200 {
            let e_sea = e.sea_level(71.0);
            e.tick(&map, &seams, &crust, e_sea);
        }
        // Concentration: vents out-collect the interior.
        let vent_mean = {
            let v = crust.vents();
            v.iter().map(|t| e.rock(*t)).sum::<f32>() / v.len() as f32
        };
        let mut all: Vec<f32> = (0..map.len() as TileId).map(|t| e.rock(t)).collect();
        all.sort_by(f32::total_cmp);
        let median = all[all.len() / 2];
        assert!(
            vent_mean > median * 4.0 + 0.05,
            "volcanic fields concentrate: vents {vent_mean} vs median {median}"
        );

        // THE SPREAD (Aaron 2026-08-25: material "spreading away from the
        // volcanos which should be the only sources"): around each vent the
        // durable record (rock + strata) sits measurably OFF the vent tile —
        // pushed out into a fan — and it lies on COOLER ground than the vent
        // itself: transport runs down the heat, away from the source, never
        // pooling only where it was born.
        let tile_w = 4.0 / (map.len() as f32).sqrt();
        let (mut w_dist, mut w_heat, mut w_sum, mut v_heat) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for &vent in crust.vents() {
            let vp = map.direction(vent);
            v_heat += seams.heat(vent);
            for t in 0..map.len() as TileId {
                let d = map.direction(t);
                let ang = vp.dot(d).clamp(-1.0, 1.0).acos();
                if ang < 6.0 * tile_w {
                    let w = e.rock(t) + e.strata(t).1;
                    w_dist += w * ang;
                    w_heat += w * seams.heat(t);
                    w_sum += w;
                }
            }
        }
        let mean_dist = w_dist / w_sum.max(1e-6);
        let mean_heat = w_heat / w_sum.max(1e-6);
        let vent_heat = v_heat / crust.vents().len().max(1) as f32;
        assert!(
            mean_dist > 0.8 * tile_w,
            "the record spreads off the vents: mean {mean_dist} vs tile {tile_w}"
        );
        // Direction is the law; the MARGIN rides the pile economics (a
        // richer upwelling consolidates more right at the vents), so it is
        // held just off zero.
        assert!(
            mean_heat < vent_heat - 0.005,
            "…and downhill on the heat: record at {mean_heat}, vents at {vent_heat}"
        );
    }

    /// **WEATHERING trims the piles and the spoil takes shape.** After a real
    /// run: sediment exists (the piles are shedding); the loftiest column's
    /// NEIGHBOURHOOD carries an apron of it (spoil lands downhill of spikes);
    /// moisture is OROGRAPHIC — the highest ground drinks more than lowland —
    /// and drowned tiles soak; no column runs away (the trim keeps even the
    /// hottest pile bounded); and somewhere sediment has CONSOLIDATED into a
    /// stratum on flat or drowned ground — the new cells the planet takes
    /// shape with.
    #[test]
    fn weathering_trims_piles_into_sediment_and_new_layers() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        for _ in 0..120 {
            let sea = e.sea_level(71.0);
            e.tick(&map, &seams, &crust, sea);
        }
        let sea = e.sea_level(71.0);
        let total_sed: f32 = (0..map.len() as TileId).map(|t| e.sediment(t)).sum();
        assert!(total_sed > 0.5, "the piles shed sediment: {total_sed}");

        // The loftiest column's ring carries spoil.
        let lofty = (0..map.len() as TileId)
            .max_by(|a, b| e.ground(*a).total_cmp(&e.ground(*b)))
            .unwrap();
        let apron: f32 = map.neighbours(lofty).iter().map(|n| e.sediment(*n)).sum();
        assert!(
            apron > 0.0 || e.sediment(lofty) > 0.0,
            "an apron forms at the tallest pile"
        );

        // The WEATHER delivers: rain has fallen somewhere on the world (the
        // engine earned it — evaporation, winds, lift), and the sea keeps
        // the boundary air breathing.
        let rained = (0..map.len() as TileId)
            .map(|t| e.rainfall(t))
            .fold(0.0f32, f32::max);
        assert!(rained > 1e-3, "the weather rains somewhere: {rained}");
        let drowned = (0..map.len() as TileId)
            .find(|t| e.ground(*t) < sea)
            .unwrap();
        assert!(
            e.moisture(drowned) > 0.05,
            "the sea breathes into the boundary layer: {}",
            e.moisture(drowned)
        );

        // No runaway spikes: the trim keeps the loftiest column bounded.
        assert!(
            e.ground(lofty) < 14.0,
            "the tallest pile stays trimmed: {}",
            e.ground(lofty)
        );

        // Somewhere, sediment became a LAYER on flat or drowned ground.
        assert!(
            e.strata_total() > 0,
            "sediment consolidates into new stack cells"
        );
    }

    /// **Erosion makes SLOPES, not needles** (Aaron 2026-08-25, second pass:
    /// "still just making spikes of materials"). After a long run every
    /// column's local relief is bounded — the fan, the talus, and the strata
    /// ROCKFALL together keep even a vent column a mountain with a skirt,
    /// never a spire; the spoil reaches far more tiles than the vents that
    /// shed it (the fan + the flow, not a one-neighbour drain); and where
    /// the water stands, sediment has settled toward FLAT — the plains.
    #[test]
    fn erosion_makes_slopes_not_needles() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // A REAL flood (76%): at 71% on the young tiered world the line sits
        // a film above the raw bed tier — any sediment lifts its tile "above
        // sea" and marine physics cannot exist by definition. 76% puts the
        // line into the shelf tier: the beds are genuinely deep. (260 ticks:
        // the weather engine's rain spins up over its EMA before the
        // erosion can shed marine sediment in quantity.)
        for _ in 0..260 {
            let sea = e.sea_level(76.0);
            e.tick(&map, &seams, &crust, sea);
        }
        let sea = e.sea_level(76.0);
        // No needles: the steepest local drop on the whole map stays inside
        // what one cliff face is allowed to hold.
        let mut steepest = 0.0f32;
        for t in 0..map.len() as TileId {
            let h = e.ground(t);
            let low = map
                .neighbours(t)
                .iter()
                .map(|nb| e.ground(*nb))
                .fold(f32::MAX, f32::min);
            steepest = steepest.max(h - low);
        }
        assert!(
            steepest < 3.0,
            "relief is mountains, not spikes: steepest drop {steepest}"
        );
        // The spoil SPREADS: far more tiles carry sediment than there are
        // vents shedding it.
        let touched = (0..map.len() as TileId)
            .filter(|t| e.sediment(*t) > 0.02)
            .count();
        assert!(
            touched > crust.vents().len() * 5,
            "the fan reaches the country: {touched} tiles vs {} vents",
            crust.vents().len()
        );
        // Plains: drowned ground settles near-flat where sediment reached it.
        let flat_floor = (0..map.len() as TileId).any(|t| {
            if e.ground(t) >= sea || e.sediment(t) <= 0.01 {
                return false;
            }
            let h = e.ground(t);
            map.neighbours(t)
                .iter()
                .all(|nb| (e.ground(*nb) - h).abs() < SED_REPOSE * 2.0)
        });
        assert!(flat_floor, "somewhere the sea floor has settled flat");
    }

    /// **Material arrives ONLY where the model says it can** (Aaron
    /// 2026-08-25: "the seams push materials — this is just generating
    /// materials out of nothing, not acceptable"). In the first ticks —
    /// before any plate has accumulated a step — every tile holding rock is
    /// a VENT, a vent's spill neighbour, or a pressurised plate boundary
    /// (collision uplift). A tile out in the open with rock on it means a
    /// per-seam-tile source crept back in, and this fails.
    #[test]
    fn material_arrives_only_at_vents_spills_and_boundaries() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        for _ in 0..3 {
            let sea = e.sea_level(71.0);
            e.tick(&map, &seams, &crust, sea);
        }
        // The sanctioned SOURCES (volcanoes only — the deep crust is the
        // buffer): the VENTS, by pinch or eruption — everything else may
        // only receive by SPILL from them (three ticks reach at most three
        // rings) or stand at a boundary/pressure site. Every other tile of
        // the planet must hold NOTHING.
        let mut dist = vec![u8::MAX; map.len()];
        let mut ring: Vec<TileId> = (0..map.len() as TileId)
            .filter(|t| crust.is_vent(*t))
            .collect();
        for t in &ring {
            dist[*t as usize] = 0;
        }
        let mut d = 0u8;
        while !ring.is_empty() && d < 3 {
            d += 1;
            let mut next = Vec::new();
            for t in ring {
                for nb in map.neighbours(t) {
                    if dist[*nb as usize] == u8::MAX {
                        dist[*nb as usize] = d;
                        next.push(*nb);
                    }
                }
            }
            ring = next;
        }
        let mut allowed: Vec<bool> = dist.iter().map(|d| *d != u8::MAX).collect();
        for t in 0..map.len() as TileId {
            // De-plated: the only extra source is a pressure RESOLVE where
            // opposing flows jammed.
            if e.pressure(t) > 0.0 {
                allowed[t as usize] = true;
            }
        }
        let stray: Vec<TileId> = (0..map.len() as TileId)
            .filter(|t| e.rock(*t) > 1e-6 && !allowed[*t as usize])
            .collect();
        assert!(
            stray.is_empty(),
            "rock out of nothing at {} open tiles (e.g. {:?})",
            stray.len(),
            &stray[..stray.len().min(4)]
        );
        // …and the tick is NEARLY IMPERCEPTIBLE (Aaron's law): three ticks'
        // upwelling touched on the order of a few hundred tiles of a 23k-tile
        // world, not a wash of it.
        let touched = (0..map.len() as TileId)
            .filter(|t| e.rock(*t) > 1e-6)
            .count();
        assert!(
            touched < map.len() / 10,
            "discrete pinches, not a rain: {touched} of {}",
            map.len()
        );
    }

    /// **A tall massif FALLS, even with no local cliffs** (Aaron 2026-08-25:
    /// "hard rock when it gets tall enough crumbles… a tall peak happens, but
    /// eventually it falls"). The scenario the slope tests cannot touch: a
    /// broad plateau of consolidated strata whose INTERNAL drops are zero —
    /// rockfall and talus never fire inside it. Summit crumble alone must
    /// bring it down toward the peak ceiling, edges first, the spoil landing
    /// on the surrounding country as the massif spreads into a RANGE.
    #[test]
    fn a_tall_massif_falls_even_without_local_cliffs() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let c: TileId = 100;
        let mut cluster: Vec<TileId> = vec![c];
        cluster.extend(map.neighbours(c).iter().copied());
        for t in &cluster {
            // The slot ladder caps formed height at L3+L4; the rest of the
            // massif stands as loose rock above — same 6.0 of relief.
            e.l3_h[*t as usize] = L3_CAP;
            e.l4_h[*t as usize] = L4_CAP;
            e.rock[*t as usize] = 6.0 - L3_CAP - L4_CAP;
        }
        // Planted by hand — hand it to the frontier the way any tool must.
        for t in cluster.clone() {
            e.disturb(&map, t);
        }
        // A fixed shallow sea: this gate is about the CRUMBLE, and the
        // tier-midpoint quirk of a percent ask on a near-uniform world would
        // drown the scenario before it starts.
        let sea = 0.2;
        let peak = |e: &Evolution| {
            cluster
                .iter()
                .map(|t| e.ground(*t) - sea)
                .fold(f32::MIN, f32::max)
        };
        let before = peak(&e);
        assert!(before > 5.0, "the scenario starts TALL: {before}");
        let centre_before = e.ground(c);
        // 80 ticks: long enough for calving to level the massif rim-first,
        // far too short for slow wet erosion alone to do it — so this gate
        // isolates the CRUMBLE, not the drizzle.
        for _ in 0..80 {
            e.tick(&map, &seams, &crust, sea);
        }
        let after = peak(&e);
        assert!(
            after < before * 0.55,
            "the massif came down: {before} → {after}"
        );
        assert!(
            e.ground(c) < centre_before * 0.7,
            "the INTERIOR fell too — the collapse ate inward, not just the rim"
        );
        // The fall is a SPREAD, not a deletion: the country around the
        // massif caught real material (sediment or new strata).
        let ring: Vec<TileId> = cluster
            .iter()
            .flat_map(|t| map.neighbours(*t).iter().copied())
            .filter(|t| !cluster.contains(t))
            .collect();
        let caught: f32 = ring.iter().map(|t| e.sediment(*t) + e.strata(*t).1).sum();
        assert!(caught > 0.5, "the range spread onto its skirts: {caught}");
    }

    /// **MAX DENSITY holds: no column hoards loose material past the cap.**
    /// The quartz law (Aaron 2026-08-25): however material arrives — merges,
    /// pressure uplift, vent injection — a column's LOOSE stack tops out at
    /// the cap; the excess has TRANSFORMED into permanent strata (compressed:
    /// DENSIFY of the height survives, as layers), and what remains loose is
    /// the young material above. After a long run, every column obeys.
    #[test]
    fn max_density_transforms_the_excess_into_strata() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        for _ in 0..150 {
            let sea = e.sea_level(71.0);
            e.tick(&map, &seams, &crust, sea);
        }
        let ceiling = LOOSE_CAP + FORM_HEIGHT + 1e-3; // the closing law is exact
        let mut transformed = 0usize;
        for t in 0..map.len() as TileId {
            let loose = e.rock(t) + e.sediment(t);
            assert!(
                loose <= ceiling,
                "tile {t} hoards {loose} loose — past the density cap"
            );
            let (n, h) = e.strata(t);
            if n > 0 && h > FORM_HEIGHT * FORM_KEEP + 1e-3 {
                transformed += 1; // more layer-height than one FORM event makes
            }
        }
        assert!(
            transformed > 0,
            "somewhere the cap transformed a hot column into stacked layers"
        );
    }

    /// **Layers form RARELY, where the era justifies them.** After a real run
    /// some strata exist — and they cover a small fraction of the world, at
    /// tiles that are (or were) hot or compressed, with compaction keeping
    /// only part of the consolidated height.
    #[test]
    fn strata_form_rarely_and_only_where_justified() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        for _ in 0..120 {
            let e_sea = e.sea_level(71.0);
            e.tick(&map, &seams, &crust, e_sea);
        }
        let formed = e.strata_total();
        assert!(formed > 0, "the era eventually forms layers");
        // Under the zero-loss ledger, thin MARINE beds are widespread — every
        // basin that catches sediment consolidates one. What stays rare is a
        // DEEP stack: many layers means a place the era kept justifying.
        let tiles_with: usize = (0..map.len() as TileId)
            .filter(|t| e.strata(*t).0 > 0)
            .count();
        assert!(
            tiles_with < map.len() / 3,
            "beds are common but not universal: {tiles_with} of {} tiles",
            map.len()
        );
        let deep: usize = (0..map.len() as TileId)
            .filter(|t| e.strata(*t).0 >= 2)
            .count();
        assert!(
            deep < map.len() / 20,
            "both-slot stacks stay rare: {deep} of {} tiles",
            map.len()
        );
        // THE SLOT LAW (Aaron 2026-08-26): the crust sub-group holds at most
        // its two formed slots (L3 the vein layer, L4 the volcanic layer),
        // each inside its own cap — max compression past a full ladder stays
        // loose above; L5 is reserved and never engaged.
        for t in 0..map.len() as TileId {
            let (n, h) = e.strata(t);
            assert!(n <= 2, "only L3 and L4 can engage");
            if n > 0 {
                assert!(h > 0.0 && h.is_finite());
            }
            assert!(
                e.layer3(t) <= L3_CAP + 1e-3 && e.layer4(t) <= L4_CAP + 1e-3,
                "each slot honours its cap"
            );
        }
    }
    /// **COLLISION where flows meet — mountains without plates** (Aaron
    /// 2026-08-25: "the idea of plates is unneeded here; the upwelling pushes
    /// materials, where they collide we calculate collision"). Over a real
    /// run, pressure exists and resolves as uplift SOMEWHERE the pushes
    /// oppose — and the world's total material NEVER shrinks. The ledger now
    /// has THREE stores, not two: the columns, the suspension, and the
    /// SUBDUCTION WELL. Counted in the true quantity (volume) and in f64, it
    /// closes to 1e-8 — material may only move between the three.
    #[test]
    fn flows_collide_into_pressure_and_the_ledger_only_gains() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(76.0);
        // FOUR STORES (Phase 3): the columns, the suspension, the well —
        // and now the DISSOLVED load, water's second denomination. A store
        // left out of this sum is a store the era can quietly leak into.
        let material = |e: &Evolution| -> f64 {
            (0..map.len() as TileId)
                .map(|t| {
                    let i = t as usize;
                    (e.base(t) + e.grown(t)) as f64 * e.area[i] as f64
                        + e.suspended(t) as f64
                        + e.dissolved(t) as f64
                })
                .sum::<f64>()
                + e.well() as f64
        };
        let mut prev = material(&e);
        let mut pressured = false;
        for _ in 0..200 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
            let now = material(&e);
            assert!(
                now >= prev * (1.0 - 1e-8) - 1e-8,
                "the ledger never drains, well and solution included: {prev} -> {now}"
            );
            assert!(
                (0..map.len() as TileId).all(|t| e.dissolved(t) >= 0.0),
                "nothing is ever owed to solution: the store is never negative"
            );
            prev = now;
            if !pressured {
                pressured = (0..map.len() as TileId).any(|t| e.pressure(t) > 0.05);
            }
        }
        assert!(pressured, "opposing flows jam into pressure somewhere");
        // The well is a store, never an overdraft: the fountains can only
        // ever hand back what the collisions actually put in it.
        assert!(e.well() >= 0.0, "the well is never overspent: {}", e.well());
        assert!(e.sunk() >= e.well(), "the take bounds the balance");
    }

    /// **The water is CONSERVED and the sea RISES as land grows** (Aaron
    /// 2026-08-25: "the upwelling will eventually fill the entire planet —
    /// keep track of the water level and area coverage"). Pour to a coverage,
    /// grow the world, and the volume stays put while the LEVEL climbs;
    /// coverage is an output the bench can watch.
    #[test]
    fn the_water_volume_is_conserved_and_the_sea_rises() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // CONTROL: hold the climate at full heat so no caps form — this gate
        // measures land displacing the sea; the ice↔sea coupling has its own
        // gate (`the_ice_age_breathes_and_locks_the_sea`).
        e.set_climate(1.0);
        e.set_water(76.0);
        let sea0 = e.resolve_sea();
        let cov0 = e.coverage();
        // On the BARE uniform floor any water covers everything — the ask
        // becomes meaningful only once the upwelling builds relief.
        assert!(cov0 > 0.99, "the bare world starts drowned: {cov0}");
        for _ in 0..250 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let sea1 = e.resolve_sea();
        assert!(
            sea1 > sea0,
            "the land the upwelling built displaced the sea upward: {sea0} -> {sea1}"
        );
        let cov1 = e.coverage();
        assert!(
            cov1 < cov0,
            "land emerged from the flood: coverage {cov0} -> {cov1}"
        );
        // The volume the level stands on is the SAME water.
        let vol = (0..map.len() as TileId)
            .map(|t| {
                let d = sea1 - e.ground(t);
                if d > 0.0 {
                    d
                } else {
                    0.0
                }
            })
            .sum::<f32>();
        assert!(vol > 0.0, "the sea still holds its water: {vol}");
    }

    /// **The vents pour a SPECTRUM** (Aaron 2026-08-25: "a spectrum of what
    /// materials are emitted, like a stream — not tick to tick"): after a
    /// run, the deposited rock's blended hardness VARIES across the world
    /// (different vents, different grades), stays inside the spectrum's
    /// clamp, and the harder fields measurably out-survive the softer ones
    /// under the same weathering.
    #[test]
    fn the_vents_pour_a_hardness_spectrum_and_hard_rock_survives() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(76.0);
        for _ in 0..150 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let mut grades: Vec<f32> = (0..map.len() as TileId)
            .filter(|t| e.rock(*t) > 0.05)
            .map(|t| e.rock_hardness(t))
            .collect();
        assert!(
            grades.len() > 20,
            "rock stands in quantity: {}",
            grades.len()
        );
        grades.sort_by(f32::total_cmp);
        let (lo, hi) = (grades[0], grades[grades.len() - 1]);
        // The scale's floor is the vents' clamp; its ceiling is the
        // METAMORPHIC cap — uplift through an indurated bed out-hardens
        // any vent pour.
        assert!((0.4..=META_HARD_CAP).contains(&lo) && (0.4..=META_HARD_CAP).contains(&hi));
        assert!(
            hi - lo > 0.25,
            "a real spectrum across the fields: {lo} .. {hi}"
        );
    }

    /// **The streams CARVE** (Aaron 2026-08-25: "channels and valleys"):
    /// after a run the discharge network exists and concentrates (a few
    /// tiles gather many tiles' rain), and along the high-discharge lines
    /// the ground runs LOWER than its banks — a channel, cut by the water.
    #[test]
    fn discharge_concentrates_and_channels_run_below_their_banks() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(76.0);
        for _ in 0..200 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let max_d = (0..map.len() as TileId)
            .map(|t| e.discharge(t))
            .fold(0.0f32, f32::max);
        // (Rain-localized weather waters less of the world than the old
        // conjured moisture did, and the drizzle smooths what falls — the
        // claim is concentration OVER THE LIVE LINE: channels exist.)
        assert!(
            max_d > CHANNEL_LIVE,
            "the network concentrates real catchments: max {max_d}"
        );
        // A channel runs below its banks: among high-discharge LAND tiles,
        // most sit lower than the mean of their non-channel neighbours.
        // (Stats at half the display live-line — the small test world's
        // catchments top out near CHANNEL_LIVE under the smoothed rain.)
        let live = CHANNEL_LIVE * 0.5;
        let sea = e.resolve_sea();
        let (mut below, mut total) = (0usize, 0usize);
        for t in 0..map.len() as TileId {
            if e.discharge(t) < live || e.ground(t) <= sea {
                continue;
            }
            let mut bank = 0.0f32;
            let mut nb_n = 0usize;
            for nb in map.neighbours(t) {
                if e.discharge(*nb) < live {
                    bank += e.ground(*nb);
                    nb_n += 1;
                }
            }
            if nb_n == 0 {
                continue;
            }
            total += 1;
            if e.ground(t) < bank / nb_n as f32 {
                below += 1;
            }
        }
        assert!(total > 5, "land channels exist: {total}");
        assert!(
            below * 2 > total,
            "channels run below their banks: {below} of {total}"
        );
    }

    /// **The vents pour in PROVINCES, not speckle**: the grade field is
    /// smooth over the sphere, so vents near each other pour kindred rock —
    /// hardness contrast arrives in belts the size of ranges. Near vent
    /// pairs must agree far better than distant ones.
    #[test]
    fn vent_grades_come_in_provinces_not_speckle() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea); // populates the vent grades
        let vents = crust.vents();
        assert!(vents.len() > 10, "vents exist: {}", vents.len());
        let (mut near, mut near_n) = (0.0f32, 0usize);
        let (mut far, mut far_n) = (0.0f32, 0usize);
        for a in 0..vents.len() {
            for b in a + 1..vents.len() {
                let dot = map.direction(vents[a]).dot(map.direction(vents[b]));
                let diff = (e.vent_grade[a] - e.vent_grade[b]).abs();
                if dot > 0.96 {
                    near += diff;
                    near_n += 1;
                } else if dot < 0.0 {
                    far += diff;
                    far_n += 1;
                }
            }
        }
        assert!(
            near_n >= 5 && far_n >= 5,
            "both pair pools exist: {near_n}/{far_n}"
        );
        let (near, far) = (near / near_n as f32, far / far_n as f32);
        assert!(
            near < far * 0.6,
            "provinces: near pairs agree, far pairs differ ({near} vs {far})"
        );
    }

    /// **The rivers CARRY to base level and the relief ORGANIZES** (Aaron
    /// 2026-08-27: mountains, valleys, drainage): after a long run the land
    /// is not a plain — the hypsometric spread is real; channels run
    /// INCISED below their banks; and the carry deposits on the way — land
    /// off the channels holds fans and floodplains, not just the sea beds.
    #[test]
    fn the_rivers_carry_and_the_relief_organizes() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // 70%: enough standing land for a channel population once the
        // weather, the vegetation and the streams have spun up (76% drowns
        // the small test world's land down to fragments by tick ~1000).
        e.set_water(70.0);
        for _ in 0..900 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let sea = e.resolve_sea();
        let mut land: Vec<f32> = (0..map.len() as TileId)
            .filter(|t| e.ground(*t) > sea)
            .map(|t| e.ground(t) - sea)
            .collect();
        assert!(land.len() > 100, "land stands: {}", land.len());
        land.sort_by(f32::total_cmp);
        let spread = land[land.len() * 9 / 10] - land[land.len() / 10];
        assert!(
            spread > 0.25,
            "the land has RELIEF, not a plain: p90-p10 {spread}"
        );
        // Channel incision: land channels sit below their banks by a real
        // margin on average — valleys, not paint.
        // Stats live-line at half the display threshold: the SMALL test
        // world's land catchments top out near CHANNEL_LIVE (the shipped
        // world's run 4x deeper) — concentration is what the stat needs.
        let live = CHANNEL_LIVE * 0.5;
        let (mut cut, mut cut_n) = (0.0f32, 0usize);
        for t in 0..map.len() as TileId {
            if e.discharge(t) < live || e.ground(t) <= sea {
                continue;
            }
            let mut bank = 0.0f32;
            let mut nb_n = 0usize;
            for nb in map.neighbours(t) {
                if e.discharge(*nb) < live {
                    bank += e.ground(*nb);
                    nb_n += 1;
                }
            }
            if nb_n > 0 {
                cut += bank / nb_n as f32 - e.ground(t);
                cut_n += 1;
            }
        }
        assert!(cut_n > 5, "land channels exist: {cut_n}");
        assert!(
            cut / cut_n as f32 > 0.0,
            "channels run INCISED on average: {}",
            cut / cut_n as f32
        );
        // The carry deposits along the way: sediment stands on quiet LAND
        // (fans, floodplains), not only on the drowned beds.
        let land_sed = (0..map.len() as TileId)
            .filter(|t| e.ground(*t) > sea && e.discharge(*t) < CHANNEL_LIVE)
            .map(|t| e.sediment(t))
            .fold(0.0f32, f32::max);
        assert!(
            land_sed > 0.05,
            "fans and floodplains hold sediment: max {land_sed}"
        );
    }

    #[test]
    fn the_carry_spreads_at_base_level_and_ice_is_a_gate() {
        // **The sediment-tower fix** (found by the 36000-tick probe: trunk
        // rivers dumping whole loads on single cells, worst where the
        // target had ICED and the damped drain trapped the pile): a load
        // whose flow target is a glacier lies down BEFORE it — spread as a
        // fan over the cell and its lower ice-free neighbours, never onto
        // the ice and never all in one cell.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let quiet = |t: TileId| {
            seams.heat(t) <= 0.0
                && !crust.is_vent(t)
                && map.neighbours(t).iter().all(|nb| !crust.is_vent(*nb))
        };
        let t = (0..map.len() as TileId)
            .find(|t| quiet(*t) && map.neighbours(*t).iter().all(|nb| quiet(*nb)))
            .expect("cold vent-free ground exists");
        let i = t as usize;
        let j = map.neighbours(t)[0] as usize;
        // A modest column (below the talus angle) whose steepest descent is
        // the ICED neighbour j — j sits lowest by a hair.
        e.rock[i] = 0.7;
        e.base[j] -= 0.05;
        e.ice[j] = 0.5;
        e.disturb(&map, t);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        // The carry never TARGETS the glacier — j sees only the repose
        // flow's intake-capped creep, a fraction of the gate's own take
        // (before the fix the whole trunk load landed on the iced cell).
        assert!(
            e.sediment[j] < 0.06,
            "only the capped creep reaches the ice: {}",
            e.sediment[j]
        );
        assert!(
            e.sediment[i] + e.sediment[j] > 0.02,
            "the fan landed at the gate: {} + {}",
            e.sediment[i],
            e.sediment[j]
        );
        let spread: f32 = map
            .neighbours(t)
            .iter()
            .filter(|nb| **nb as usize != j)
            .map(|nb| e.sediment[*nb as usize])
            .sum();
        assert!(
            spread > 0.005,
            "the rest fans to the ice-free ring: {spread}"
        );
    }

    #[test]
    fn an_oversteep_needle_never_sleeps() {
        // **Over-steep LOOSE ground never sleeps** (the 25200-tick towers):
        // a needle built by a burst and then abandoned — no activity, no
        // channel — must still shed, because the talus law runs on
        // steepness and MATERIAL, not on the frontier's memory. Sediment is
        // under the metamorphic floor by construction, so this tower wakes
        // itself; the mirror case (a consolidated face, which may stand) is
        // the inverse probe's.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let quiet = |t: TileId| {
            seams.heat(t) <= 0.0
                && !crust.is_vent(t)
                && map.neighbours(t).iter().all(|nb| !crust.is_vent(*nb))
        };
        let t = (0..map.len() as TileId)
            .find(|t| quiet(*t) && map.neighbours(*t).iter().all(|nb| quiet(*nb)))
            .expect("cold vent-free ground exists");
        let i = t as usize;
        // The abandoned tower: planted directly, NO disturb — the frontier
        // has never heard of it.
        e.sediment[i] = 6.0;
        let total = |e: &Evolution| {
            std::iter::once(t)
                .chain(map.neighbours(t).iter().copied())
                .map(|x| e.ground(x) * e.area[x as usize])
                .sum::<f32>()
        };
        let before_h = e.ground(t);
        let before_m = total(&e);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        assert!(
            e.ground(t) < before_h - 0.5,
            "the sleeping needle sheds anyway: {} -> {}",
            before_h,
            e.ground(t)
        );
        let _ = (before_m, &total);
        let ring_sed: f32 = map
            .neighbours(t)
            .iter()
            .map(|nb| e.sediment[*nb as usize])
            .sum();
        assert!(
            ring_sed > 0.3,
            "the shed lands on the ring (and rides on from there): {ring_sed}"
        );
    }

    #[test]
    fn mountains_cast_rain_shadows() {
        // **The wall rains its windward side and shadows the interior**
        // (Aaron 2026-08-28: "there's no mountains blocking moisture to
        // interiors"): a hand-built wall taller than the boundary deck, set
        // square across the precomputed wind, dries the tile beyond it.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(100.0); // a sea to evaporate from
                            // A windward chain in a real wind lane: u → wall → shadow.
        let u = (0..map.len() as TileId)
            .find(|t| {
                let i = *t as usize;
                let j = e.wind_to[0][i];
                if j == u32::MAX || crust.is_vent(*t) {
                    return false;
                }
                let k = e.wind_to[0][j as usize];
                k != u32::MAX && k != i as u32 && !crust.is_vent(j as TileId)
            })
            .expect("a wind lane exists");
        let wall = e.wind_to[0][u as usize] as usize;
        let shadow = e.wind_to[0][wall] as usize;
        e.base[wall] += 3.0; // past sea + DECK_ALT[0]: a true wall
        for _ in 0..120 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let (wet, dry) = (e.rain[u as usize], e.rain[shadow]);
        // The drizzle spills SOME lifted moisture over the wall (real
        // weather) — the shadow is a real deficit, not a void, and a
        // single-tile wall is the weakest staging of it.
        assert!(
            wet > dry * 1.25 + 1e-4,
            "the windward side drinks, the interior lies in the shadow: {wet} vs {dry}"
        );
    }

    #[test]
    fn the_rain_comes_in_belts() {
        // **Convergence concentrates, divergence dries** — on a pure water
        // world (no orography at all) the rain still BANDS: the ITCZ out-
        // rains the subtropical divergence — deserts are a latitude before
        // they are a landform.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(100.0);
        for _ in 0..150 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let band_mean = |lo: f32, hi: f32| {
            let (mut s, mut n) = (0.0f32, 0usize);
            for t in 0..map.len() as TileId {
                let a = map.direction(t).y.abs();
                if a >= lo && a < hi {
                    s += e.rain[t as usize];
                    n += 1;
                }
            }
            s / n.max(1) as f32
        };
        let itcz = band_mean(0.0, 0.12);
        let horse = band_mean(0.28, 0.42);
        assert!(
            itcz > horse * 1.5,
            "the ITCZ out-rains the subtropical divergence: {itcz} vs {horse}"
        );
    }

    #[test]
    fn deserts_erode_less_than_watered_country() {
        // **Plains survive where the rain never goes** (Aaron: "we aren't
        // getting plains because we're eroding everything in general"): two
        // identical columns, one under standing rain, one bone dry — the
        // watered one sheds measurably more.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let quiet = |t: TileId| {
            seams.heat(t) <= 0.0
                && !crust.is_vent(t)
                && map.neighbours(t).iter().all(|nb| !crust.is_vent(*nb))
        };
        // Both sites must be BECALMED: quiet heat AND no derived push —
        // a heat-gradient shove would ride a pile away mid-experiment.
        let still =
            |e: &Evolution, t: TileId| quiet(t) && e.push[t as usize].length_squared() < 1e-12;
        let a = (0..map.len() as TileId)
            .find(|t| still(&e, *t))
            .expect("a becalmed site");
        let b = (0..map.len() as TileId)
            .rev()
            .find(|t| still(&e, *t) && map.direction(*t).dot(map.direction(a)) < 0.5)
            .expect("a far becalmed site");
        let (ai, bi) = (a as usize, b as usize);
        e.rock[ai] = 0.6;
        e.rock[bi] = 0.6;
        for _ in 0..30 {
            // Hold the contrast against the weather's own writes: a is
            // WATERED, b is desert.
            e.rain[ai] = 0.12;
            e.rain[bi] = 0.0;
            e.disturb(&map, a);
            e.disturb(&map, b);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.rock[ai] < e.rock[bi] - 0.05,
            "rain erodes, drought preserves: wet {} vs dry {}",
            e.rock[ai],
            e.rock[bi]
        );
    }

    #[test]
    fn no_delivery_outruns_flood_control() {
        // **The carry obeys the intake law** (the 32400-tick relapse): a
        // huge suspended load over a blocked target settles at most
        // INTAKE_CAP per cell per tick — the rest stays water-borne with no
        // height — so talus can always outrun delivery and no needle can be
        // manufactured, while the material ledger stays whole.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let quiet = |t: TileId| {
            seams.heat(t) <= 0.0
                && !crust.is_vent(t)
                && map.neighbours(t).iter().all(|nb| !crust.is_vent(*nb))
        };
        let t = (0..map.len() as TileId)
            .find(|t| quiet(*t) && map.neighbours(*t).iter().all(|nb| quiet(*nb)))
            .expect("cold vent-free ground exists");
        let i = t as usize;
        let j = map.neighbours(t)[0] as usize;
        e.base[j] -= 0.05;
        e.ice[j] = 0.5; // the blocked target
        e.suspend[i] = 5.0; // a trunk delivery's worth, water-borne
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        assert!(
            e.sediment[i] <= INTAKE_CAP + 0.02,
            "one tick settles at most the intake cap: {}",
            e.sediment[i]
        );
        assert!(
            e.suspend[i] > 3.5,
            "the refusal stays in suspension: {}",
            e.suspend[i]
        );
        // The suspension DRAINS over the following ticks — capped settling,
        // never a tower: the site's prominence stays under the needle line.
        for _ in 0..40 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let nb_hi = map
            .neighbours(t)
            .iter()
            .map(|n| e.ground(*n))
            .fold(0.0f32, f32::max);
        assert!(
            e.ground(t) - nb_hi < 1.5,
            "no needle was manufactured: prominence {}",
            e.ground(t) - nb_hi
        );
        assert!(
            e.suspend[i] < 5.0,
            "the suspension is settling out: {}",
            e.suspend[i]
        );
    }

    #[test]
    fn the_drizzle_gives_the_dial_its_reach() {
        // **The upper decks water the interiors** (Aaron 2026-08-28: dialed
        // 70% and the big continent stayed bare): before the drizzle, half
        // the land measured rain 0.000 forever and the green share
        // saturated near 20% whatever the dial asked. With every deck
        // precipitating a little as it passes, a lush ask is reachable.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(70.0);
        e.set_veg_target(0.70);
        for _ in 0..420 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.green_share() > 0.35,
            "the dial reaches past the old support ceiling: {}",
            e.green_share()
        );
        // …and the rain SUPPORT itself is broad now: most land sees water.
        let sea = e.resolve_sea();
        let (mut dry, mut land) = (0usize, 0usize);
        for t in 0..map.len() as TileId {
            if e.ground(t) >= sea {
                land += 1;
                if e.rain[t as usize] < 1e-5 {
                    dry += 1;
                }
            }
        }
        assert!(
            (dry as f32) < land as f32 * 0.25,
            "the sky touches most of the land: {dry} bone-dry of {land}"
        );
    }

    #[test]
    fn the_green_target_walks_the_flora() {
        // **The GREEN TARGET is a dial, not paint** (Aaron 2026-08-28): the
        // flora's thirst adapts until the greened share of land meets the
        // ask — same world, same weather, a high target grows a wider
        // green than a starved one, and the thirst walks opposite ways.
        let run = |target: f32| {
            let (map, seams, crust, _plates) = world();
            let mut e = Evolution::new(&map, &seams);
            e.set_water(70.0);
            e.set_veg_target(target);
            for _ in 0..420 {
                let sea = e.resolve_sea();
                e.tick(&map, &seams, &crust, sea);
            }
            (e.green_share(), e.veg_thirst)
        };
        let (lush_share, lush_thirst) = run(0.60);
        let (starved_share, starved_thirst) = run(0.05);
        assert!(
            lush_share > starved_share + 0.05,
            "the dial moves the green: {lush_share} vs {starved_share}"
        );
        assert!(
            lush_thirst < starved_thirst,
            "hardier stock under a high ask, thirstier under a low one: {lush_thirst} vs {starved_thirst}"
        );
        assert!(
            (VEG_THIRST_MIN..=VEG_THIRST_MAX).contains(&lush_thirst),
            "the thirst stays in its working range"
        );
    }

    #[test]
    fn vegetation_greens_and_binds_the_watered_country() {
        // **The greening and its resistance** (Aaron 2026-08-28): cover
        // grows where rain is sustained, and an established forest SHIELDS
        // its column — same rain, same rock, the covered twin sheds less.
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let quiet = |t: TileId| {
            seams.heat(t) <= 0.0
                && !crust.is_vent(t)
                && map.neighbours(t).iter().all(|nb| !crust.is_vent(*nb))
        };
        let still =
            |e: &Evolution, t: TileId| quiet(t) && e.push[t as usize].length_squared() < 1e-12;
        let a = (0..map.len() as TileId)
            .find(|t| still(&e, *t))
            .expect("a becalmed site");
        let b = (0..map.len() as TileId)
            .rev()
            .find(|t| still(&e, *t) && map.direction(*t).dot(map.direction(a)) < 0.5)
            .expect("a far becalmed site");
        let (ai, bi) = (a as usize, b as usize);
        e.rock[ai] = 0.6;
        e.rock[bi] = 0.6;
        e.veg[ai] = 1.0; // an established forest
        for _ in 0..30 {
            e.rain[ai] = 0.05;
            e.rain[bi] = 0.05;
            e.veg[bi] = 0.0; // the bare twin: shield contrast, held
            e.disturb(&map, a);
            e.disturb(&map, b);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.veg[ai] > 0.9,
            "held rain sustains the forest: {}",
            e.veg[ai]
        );
        assert!(
            e.rock[ai] > e.rock[bi] + 0.05,
            "the cover binds the soil: forest {} vs bare {}",
            e.rock[ai],
            e.rock[bi]
        );
        // And growth is EARNED: a third site under the same held rain,
        // never planted, greens on its own clock.
        let c = (a + 1..map.len() as TileId)
            .find(|t| still(&e, *t) && *t != b)
            .expect("a third site");
        for _ in 0..30 {
            e.rain[c as usize] = 0.05;
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.veg[c as usize] > 0.2,
            "sustained rain grows cover from nothing: {}",
            e.veg[c as usize]
        );
    }

    #[test]
    #[ignore = "diagnostic probe — run with --ignored"]
    fn probe_spikes() {
        // BENCH-TRUE: the shipped world size, the water-world start, and the
        // geological drift cadence the scene runs (seams breathe, vents
        // re-derive, motion follows — every 12 ticks).
        let map = HexMap::new(96);
        let mut seams = SeamField::new(&map, 6, 4, 42);
        let mut crust = CrustField::derive(&map, &seams);
        let mut e = Evolution::new(&map, &seams);
        e.set_climate(0.63);
        e.set_water(66.0);
        for k in 0..=36000u32 {
            if k > 0 {
                let sea = e.resolve_sea();
                e.tick(&map, &seams, &crust, sea);
                if k % 12 == 0 {
                    seams.drift(&map, 0.06);
                    crust = CrustField::derive(&map, &seams);
                    e.derive_motion(&map, &seams);
                }
            }
            if k % 3600 == 0 {
                let sea = e.resolve_sea();
                let (mut hi, mut prom, mut t_prom) = (0.0f32, 0.0f32, 0 as TileId);
                let mut needles = 0usize;
                for t in 0..map.len() as TileId {
                    let g = e.ground(t);
                    hi = hi.max(g);
                    let nb_hi = map
                        .neighbours(t)
                        .iter()
                        .map(|n| e.ground(*n))
                        .fold(0.0f32, f32::max);
                    let p = g - nb_hi;
                    if p > prom {
                        prom = p;
                        t_prom = t;
                    }
                    if p > 1.5 {
                        needles += 1;
                    }
                }
                let i = t_prom as usize;
                eprintln!(
                    "PROBE k={k} tallest={hi:.2} maxprom={prom:.2} needles={needles} sea={sea:.2} @prom: ice={:.2} sed={:.2} rock={:.2} base={:.2} l3={:.2} l4={:.2} hard={:.2} disch={:.1} vent={} age={}",
                    e.ice[i], e.sediment[i], e.rock[i], e.base[i], e.l3_h[i], e.l4_h[i],
                    e.rock_hard[i], e.discharge[i], crust.is_vent(t_prom) as u8, e.edge_age[i]
                );
            }
        }
    }

    /// **The collision edges are TRACKED — born, persistent, dead** (Aaron
    /// 2026-08-27: "keeping track of where material is colliding as it
    /// starts to collide, as it continues to collide, as it stops
    /// colliding"): after a run the ledger holds live edges where the flows
    /// keep meeting, with real AGES (persistence, not one-tick flashes); and
    /// when the motion stops, the edges DIE — intensity decays below the
    /// live line and the ages reset. The crust-side corollary of the seams.
    #[test]
    fn collision_edges_are_born_persist_and_die() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(76.0);
        for _ in 0..250 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let live: Vec<TileId> = (0..map.len() as TileId)
            .filter(|t| e.collision_edge(*t) >= EDGE_LIVE)
            .collect();
        assert!(!live.is_empty(), "flows meet somewhere: live edges exist");
        let oldest = live.iter().map(|t| e.collision_age(*t)).max().unwrap();
        assert!(
            oldest > 30,
            "an edge PERSISTS across ticks: oldest {oldest}"
        );
        // Kill the motion: no flows, no meetings — every edge must DIE.
        e.push = vec![Vec3::ZERO; map.len()];
        for _ in 0..150 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        for t in &live {
            assert!(
                e.collision_edge(*t) < EDGE_LIVE && e.collision_age(*t) == 0,
                "a stopped collision's edge dies: tile {t}"
            );
        }
    }

    /// **The jam SCRAPES the foreland onto the wedge** (orogenic shortening,
    /// Aaron 2026-08-27: "we aren't ever really jamming any material cells
    /// together"): a stalled mover takes a share of the opposing column's
    /// loose pile — two columns become one taller one, a conserved transfer
    /// — the meeting lands in the edge ledger, and the uplift that follows
    /// is METAMORPHIC: pressed through an indurated bed, the converted rock
    /// hardens past its old grade.
    #[test]
    fn the_jam_scrapes_the_foreland_and_uplift_metamorphoses() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // A hand-built head-on meeting on cold, vent-free ground: the mover
        // t drives straight at its neighbour j, which drives straight back.
        let quiet = |e: &Evolution, t: TileId| {
            seams.heat(t) <= 0.0
                && !crust.is_vent(t)
                && map.neighbours(t).iter().all(|nb| !crust.is_vent(*nb))
                && e.rock(t) == 0.0
        };
        let t = (0..map.len() as TileId)
            .find(|t| quiet(&e, *t) && map.neighbours(*t).iter().all(|nb| quiet(&e, *nb)))
            .expect("cold vent-free ground exists");
        let j = map.neighbours(t)[0];
        let (ti, ji) = (t as usize, j as usize);
        let p = map.direction(t);
        let toward = map.direction(j) - p;
        let dir = (toward - p * p.dot(toward)).normalize();
        e.rock[ti] = 0.75;
        e.rock[ji] = 0.6;
        e.sediment[ji] = 0.3;
        e.bed_hard[ti] = 2.0;
        e.bed_hard[ji] = 2.0;
        e.push[ti] = dir * RATE_MAX;
        e.push[ji] = -dir * RATE_MAX;
        e.drift[ti] = 1.0; // fires this tick
        e.disturb(&map, t);
        let pair = |e: &Evolution| e.grown(t) * e.area[ti] + e.grown(j) * e.area[ji];
        let before = pair(&e);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        // The scrape: the foreland's loose pile moved onto the wedge — and
        // the jammed, pressured pile promptly CONSOLIDATED: the collision
        // built a stratum where the columns met.
        assert!(
            e.grown(t) > 0.7,
            "the wedge holds the shortened column: {}",
            e.grown(t)
        );
        assert!(
            e.l3_h[ti] > 0.5,
            "the jam formed a stratum on the wedge: {}",
            e.l3_h[ti]
        );
        assert!(e.rock[ji] < 0.45, "the foreland foundered: {}", e.rock[ji]);
        assert!(
            e.sediment[ti] < 0.1,
            "the scrape took ROCK only — sediment is the water cycle's currency: {}",
            e.sediment[ti]
        );
        assert!(
            e.grown(t) > e.grown(j) * 1.5,
            "two columns became one taller one: {} vs {}",
            e.grown(t),
            e.grown(j)
        );
        assert!(
            e.pressure(t) > 0.3 && e.pressure(j) > 0.15,
            "both sides jammed into pressure"
        );
        assert!(
            e.collision_edge(t) >= EDGE_LIVE && e.collision_age(t) >= 1,
            "the meeting is on the edge ledger"
        );
        // Conserved: the pair only LOST material (erosion fans a little to
        // the ring) — the jam never minted any.
        assert!(
            pair(&e) <= before + 1e-3,
            "shortening is a transfer: {before} -> {}",
            pair(&e)
        );
        // The follow-up resolve: uplift through the indurated bed HARDENS
        // the wedge past the plain grade it carried.
        let hard_before = e.rock_hardness(t);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        assert!(
            e.rock_hardness(t) > hard_before,
            "metamorphic uplift raises the grade: {hard_before} -> {}",
            e.rock_hardness(t)
        );
    }

    /// **The strata INHERIT the grade — the spectrum becomes PERMANENT**:
    /// two equal piles consolidate under equal pressure, one hard-fed, one
    /// soft-fed; the formed layers carry those grades, and under the same
    /// weathering the hard massif keeps more of its stratum than the soft
    /// one — ranges of hard country stand while soft country washes out.
    /// Two equatorial, cold, vent-free sites well apart — the staging both
    /// differential-survival gates share: one fixture, two sites, so the
    /// ONLY difference between them is the material planted on them.
    fn two_quiet_sites(map: &HexMap, seams: &SeamField, crust: &CrustField) -> (TileId, TileId) {
        let mut far = vec![u8::MAX; map.len()];
        let mut ring: Vec<TileId> = (0..map.len() as TileId)
            .filter(|t| crust.is_vent(*t))
            .collect();
        for t in &ring {
            far[*t as usize] = 0;
        }
        for d in 1..=4u8 {
            let mut next = Vec::new();
            for t in ring {
                for nb in map.neighbours(t) {
                    if far[*nb as usize] == u8::MAX {
                        far[*nb as usize] = d;
                        next.push(*nb);
                    }
                }
            }
            ring = next;
        }
        let ok = |t: TileId| {
            far[t as usize] == u8::MAX && seams.heat(t) <= 0.0 && map.direction(t).y.abs() < 0.3
        };
        let a = (0..map.len() as TileId).find(|t| ok(*t)).expect("a site");
        let b = (0..map.len() as TileId)
            .rev()
            .find(|t| ok(*t) && map.direction(*t).dot(map.direction(a)) < 0.5)
            .expect("a far site");
        (a, b)
    }

    #[test]
    fn strata_inherit_the_grade_and_the_hard_massif_outlives_the_soft() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // A thin sheet of water for a sane climate; the piles stand DRY.
        e.water_volume = map.len() as f32 * 0.02;
        let (a, b) = two_quiet_sites(&map, &seams, &crust);
        let (ai, bi) = (a as usize, b as usize);
        e.rock[ai] = 1.0;
        e.rock_hard[ai] = 1.8;
        e.pressure[ai] = 0.7;
        e.rock[bi] = 1.0;
        e.rock_hard[bi] = 0.6;
        e.pressure[bi] = 0.7;
        e.disturb(&map, a);
        e.disturb(&map, b);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        // Both consolidated under pressure into the vein layer — and the
        // GRADE rode in.
        assert!(
            e.l3_h[ai] > 0.5 && e.l3_h[bi] > 0.5,
            "both piles formed strata: {} / {}",
            e.l3_h[ai],
            e.l3_h[bi]
        );
        let (ga, gb) = (e.strata_hardness(a).0, e.strata_hardness(b).0);
        assert!(
            ga > 1.5 && gb < 0.75,
            "the strata inherited their grades: {ga} / {gb}"
        );
        // The same weathering, tick after tick: the soft stratum washes out
        // faster than the hard one.
        for _ in 0..80 {
            e.disturb(&map, a);
            e.disturb(&map, b);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.l3_h[bi] < e.l3_h[ai],
            "the hard massif outlives the soft: hard {} vs soft {}",
            e.l3_h[ai],
            e.l3_h[bi]
        );
        assert!(
            e.l3_h[bi] < 0.85,
            "the soft stratum is actually wearing: {}",
            e.l3_h[bi]
        );
    }

    /// **DEPOSITION LAYS FLAT, THE SHORTENING FOLDS** (Phase 1, the fabric's
    /// recording law). Two mechanisms in one staged column, in the order the
    /// world does them: a bed the era has just consolidated carries NO dip —
    /// sediment and pressed rock settle horizontal — and then a sustained
    /// convergence, resolving as the uplift that thickens the column, tilts
    /// those same beds and stamps their trend ACROSS the direction it is
    /// being shortened along.
    ///
    /// Nothing here asserts a landform (935269B7). The claims are the
    /// transformation and its geometry: flat on deposition, tilt on
    /// shortening, trend perpendicular to the staged convergence, and a fold
    /// that stops at the cap because beds fold and never invert.
    #[test]
    fn deposition_lies_flat_and_the_shortening_folds_the_beds() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // A thin sheet of water for a sane climate; the column stands DRY.
        e.water_volume = map.len() as f32 * 0.02;
        let (a, _b) = two_quiet_sites(&map, &seams, &crust);
        let ai = a as usize;
        // THE STAGED CONVERGENCE: the shortening direction the sim reads is
        // the tile's own push. Aim it at a chosen neighbour so the trend the
        // deformation records has something to be measured against.
        let p = map.direction(a);
        let nb = map.neighbours(a)[0];
        let toward = map.direction(nb) - p;
        let conv = (toward - p * p.dot(toward)).normalize();
        e.push[ai] = conv * RATE_MAX;
        // A pile at forming pressure: this tick consolidates it into a bed.
        e.rock[ai] = 1.0;
        e.pressure[ai] = 0.7;
        e.disturb(&map, a);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        assert!(
            e.l3_h[ai] > 0.5,
            "the pile consolidated into a bed: {}",
            e.l3_h[ai]
        );
        let (_, dip3, dip4) = e.strata_fabric(a);
        assert_eq!(
            (dip3, dip4),
            (0.0, 0.0),
            "a bed the world has just laid down is FLAT"
        );
        // Now SHORTEN it, tick after tick: the convergence holds, the ring
        // keeps supplying the quantum, and the uplift folds what it hardens.
        for _ in 0..30 {
            for t in map.neighbours(a) {
                e.rock[*t as usize] += 0.05;
            }
            e.pressure[ai] = 0.7;
            e.disturb(&map, a);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let (strike, dip3, dip4) = e.strata_fabric(a);
        assert!(
            dip3 > 0.0 && dip4 > 0.0,
            "the shortening tilted the beds: {dip3} / {dip4}"
        );
        assert!(
            dip3 <= DIP_CAP && dip4 <= DIP_CAP,
            "beds fold, they never invert: {dip3} / {dip4} vs cap {DIP_CAP}"
        );
        // THE TREND IS PERPENDICULAR TO THE CONVERGENCE — read back through
        // the same tangent frame the recording used.
        let (east, north) = Evolution::frame(p);
        let line = east * strike.cos() + north * strike.sin();
        assert!(
            line.dot(conv).abs() < 1e-3,
            "the strike runs ACROSS the convergence it was folded by: {}",
            line.dot(conv)
        );
    }

    /// **THE DIRECTIONAL DIFFERENTIAL** — the whole point of the fabric.
    /// ONE site, ONE staging, ONE tick, run twice: same grade, same dip, same
    /// erosion budget, and the ONLY difference between the runs is the TREND
    /// of the bedding relative to where the water goes. A stream running
    /// ALONG the strike of a dipping bed exploits the soft interbeds and cuts
    /// faster; a stream forced ACROSS it has to saw through the resistant bed
    /// and cuts slower — and both still cut, because the factor is bounded
    /// and can neither zero a take nor mint one.
    ///
    /// A RATE DIFFERENTIAL and nothing else — never that a strike ridge, a
    /// water gap or a trellis exists (935269B7).
    #[test]
    fn the_stream_along_the_strike_outcuts_the_stream_across_it() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let (map, seams, crust, _plates) = world();
        let (a, _b) = two_quiet_sites(&map, &seams, &crust);
        let ai = a as usize;
        // ONE unambiguous drain, dug just below the rest of the ring, so the
        // stream tree's receiver — and therefore the flow direction the
        // fabric is read against — is known in advance.
        let drain = map.neighbours(a)[0];
        let p = map.direction(a);
        let toward = map.direction(drain) - p;
        let flow = (toward - p * p.dot(toward)).normalize();
        let (east, north) = Evolution::frame(p);
        let flow_az = flow.dot(north).atan2(flow.dot(east)).rem_euclid(PI);
        /// Half a slot in each consolidated cell — enough bed that neither
        /// take runs out of material, little enough relief that nothing calves.
        const BED: f32 = 0.5;

        let run = |strike: f32| -> (f32, f32) {
            let mut e = Evolution::new(&map, &seams);
            e.water_volume = map.len() as f32 * 0.02;
            // Relief kept UNDER the calving line on purpose: rockfall is
            // geometric and the fabric never touches it (the needle-killers
            // stay exactly as they were), so a staging that calves would
            // measure the wrong law. Here the weathering take is the only
            // thing removing bed, which is what the fabric scales.
            e.l3_h[ai] = BED;
            e.l4_h[ai] = BED;
            e.l3_hard[ai] = UPLIFT_HARD;
            e.l4_hard[ai] = UPLIFT_HARD;
            e.l3_dip[ai] = DIP_CAP;
            e.l4_dip[ai] = DIP_CAP;
            e.strike[ai] = strike;
            e.base[drain as usize] -= 0.15;
            e.rain[ai] = 0.05;
            e.disturb(&map, a);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
            (BED - e.l3_h[ai], BED - e.l4_h[ai])
        };
        let along = run(flow_az);
        let across = run((flow_az + FRAC_PI_2).rem_euclid(PI));
        let (cut_along, cut_across) = (along.0 + along.1, across.0 + across.1);
        assert!(
            cut_across > 0.0,
            "the bed across the strike still erodes — a fabric may never zero \
             a take: {cut_across}"
        );
        assert!(
            cut_along > cut_across,
            "the stream ALONG the strike outcuts the one across it: \
             {cut_along} vs {cut_across}"
        );
        // THE CLAMP BAND: the factor is 1 ± ANISO_SPAN, so two takes of the
        // same bed at the same budget can never differ by more than
        // (1+SPAN)/(1−SPAN).
        let band = (1.0 + ANISO_SPAN) / (1.0 - ANISO_SPAN);
        let ratio = cut_along / cut_across;
        assert!(
            ratio <= band * 1.001,
            "the differential stays inside the band: {cut_along} vs \
             {cut_across} = {ratio}× (band {band}×)"
        );
        // …and a bed folded to the CAP spends the whole band: the factor is
        // the full 1 ± ANISO_SPAN, not a fraction of it.
        assert!(
            ratio >= band * 0.99,
            "a bed at the dip cap spends the whole band: {ratio}× of {band}×"
        );
    }

    /// **THE NULL FABRIC IS INERT** — the compatibility anchor for Phase 1.
    /// The fabric's whole claim to being additive is that at dip 0 the
    /// directional factor is exactly `1.0`, and multiplying an erodibility by
    /// exactly one changes nothing at all. So a world carrying a recorded
    /// TREND everywhere but no tilt anywhere must run BIT-FOR-BIT the same as
    /// a world carrying no fabric at all — every take, every deposit, every
    /// height, over a real stretch of era.
    ///
    /// This is what makes a pre-Phase-1 planet (and every gate staged before
    /// the fabric existed) behave exactly as it always did.
    #[test]
    fn a_null_fabric_world_erodes_exactly_as_it_always_did() {
        let (map, seams, crust, _plates) = world();
        let run = |stamp: f32| {
            let mut e = Evolution::new(&map, &seams);
            e.set_water(70.0);
            if stamp != 0.0 {
                for (i, s) in e.strike.iter_mut().enumerate() {
                    *s = stamp * (i % 7) as f32;
                }
            }
            for t in 0..map.len() as TileId / 7 {
                e.disturb(&map, t * 7);
            }
            for _ in 0..40 {
                let sea = e.resolve_sea();
                e.tick(&map, &seams, &crust, sea);
            }
            (
                e.base.clone(),
                e.l3_h.clone(),
                e.l4_h.clone(),
                e.rock.clone(),
                e.sediment.clone(),
            )
        };
        assert_eq!(
            run(0.0),
            run(0.31),
            "a recorded trend with no tilt moves not one bit of material"
        );
    }

    /// **THE FABRIC IS DURABLE — and its absence is legal.** A recorded
    /// bedding attitude survives capture → JSON → restore exactly (the
    /// round-trip family's contract: a durable field that skips the format is
    /// a broken planet). And an epoch written BEFORE the fabric existed —
    /// the arrays simply not in the file — still loads, standing its world up
    /// with no fabric at all, which by the gate above erodes as it always
    /// did.
    #[test]
    fn the_fabric_survives_the_epoch_and_a_pre_fabric_file_loads_flat() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(70.0);
        // A real stretch of era first, so the capture is a whole planet (the
        // vent emissions size themselves on the first cycle), then a recorded
        // fabric on a scatter of its columns.
        for t in 0..map.len() as TileId / 7 {
            e.disturb(&map, t * 7);
        }
        for _ in 0..8 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        for t in (0..map.len()).step_by(5) {
            e.strike[t] = 0.4 + (t % 3) as f32 * 0.5;
            e.l3_dip[t] = 0.2;
            e.l4_dip[t] = 0.35;
        }
        let file = e.capture(&map, &seams, "fabric round-trip");
        let json = file.to_json().expect("the planet serializes");
        let back = crate::epochfile::PlanetEpoch::from_json(&json).expect("the planet validates");
        let mut e2 = Evolution::new(&map, &seams);
        e2.restore(&map, &seams, &crust, &back)
            .expect("restore succeeds");
        assert_eq!(
            e2.capture(&map, &seams, "fabric round-trip"),
            file,
            "the fabric replays with every other durable field"
        );
        assert_eq!(
            e2.strata_fabric(5),
            e.strata_fabric(5),
            "the trend and both dips come back exactly"
        );

        // A PRE-FABRIC EPOCH: strip the arrays out of the file entirely, the
        // way a planet captured before Phase 1 has them.
        let mut v: serde_json::Value = serde_json::from_str(&json).expect("the file is json");
        let led = v["ledger"]
            .as_object_mut()
            .expect("the ledger is an object");
        for k in ["strike", "l3_dip", "l4_dip"] {
            assert!(led.remove(k).is_some(), "{k} was in the file to begin with");
        }
        let old = crate::epochfile::PlanetEpoch::from_json(&v.to_string())
            .expect("a pre-fabric epoch still validates");
        let mut e3 = Evolution::new(&map, &seams);
        e3.restore(&map, &seams, &crust, &old)
            .expect("a pre-fabric planet restores");
        assert!(
            (0..map.len() as TileId).all(|t| e3.strata_fabric(t) == (0.0, 0.0, 0.0)),
            "a pre-fabric planet stands up with no fabric at all"
        );
    }

    /// **THE INVERSE PROBE — DIFFERENTIAL SURVIVAL** (0b, the standing
    /// instrument the erosion-equilibrium program adds; ruling R1). The
    /// forward gates all ask "does relief get torn down?". This one asks the
    /// question the shape of the world actually depends on: **does WHAT IT
    /// IS MADE OF change how fast it goes?**
    ///
    /// One fixture, two sites, the same relief planted on each, both woken
    /// once and then left to the same process rules — the only difference is
    /// the material. A LOOSE pile (sediment, under the metamorphic floor)
    /// runs the loose drains: talus sheds its over-slope and the repose flow
    /// creeps it away every tick, wanted or not, and an oversteep loose face
    /// never sleeps. A CONSOLIDATED riser (an indurated bed under
    /// metamorphic strata, over the floor) runs the same rules and survives
    /// them: rockfall calves it toward the cliff line, erosion divides its
    /// takes by the grade it inherited, and once nothing is touching it any
    /// more it STANDS instead of being woken to fail.
    ///
    /// The assertion is the RATE DIFFERENTIAL and nothing else — never that
    /// a terrace, a riser or a canyon wall exists (935269B7). Landforms are
    /// for the world to produce; this gate only pins that the discriminator
    /// which would produce them is live.
    #[test]
    fn the_loose_pile_collapses_and_the_consolidated_riser_stands() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // A thin sheet of water for a sane climate; both risers stand DRY.
        e.water_volume = map.len() as f32 * 0.02;
        let (hard, loose) = two_quiet_sites(&map, &seams, &crust);
        let (hi, li) = (hard as usize, loose as usize);
        const RELIEF: f32 = 3.0;
        // EQUAL DIMENSION, opposite material. The riser is consolidated
        // ladder under an indurated bed — a face over ARREST_GRADE; the pile
        // is loose sediment, which is under it by construction (SED_GRADE).
        e.l3_h[hi] = L3_CAP;
        e.l4_h[hi] = RELIEF - L3_CAP;
        e.l3_hard[hi] = UPLIFT_HARD;
        e.l4_hard[hi] = UPLIFT_HARD;
        e.bed_hard[hi] = MARINE_HARD_CAP;
        e.sediment[li] = RELIEF;
        assert!(
            e.face_grade(hi) >= ARREST_GRADE && e.face_grade(li) < ARREST_GRADE,
            "the two faces sit on opposite sides of the line: {} / {}",
            e.face_grade(hi),
            e.face_grade(li)
        );
        let prom = |e: &Evolution, t: TileId| {
            e.ground(t)
                - map
                    .neighbours(t)
                    .iter()
                    .map(|n| e.ground(*n))
                    .fold(0.0f32, f32::max)
        };
        let (h0, l0) = (prom(&e, hard), prom(&e, loose));
        assert!(
            (h0 - l0).abs() < 1e-3 && h0 > RELIEF * 0.9,
            "both start as the same relief: {h0} / {l0}"
        );
        // BOTH ABANDONED — planted directly, neither disturbed, exactly as
        // the needle gate stages its tower: the frontier has never heard of
        // either of them, so the ONLY law that can reach them is the
        // oversteep wake, and the only thing it reads is the face.
        for _ in 0..60 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let (h1, l1) = (prom(&e, hard), prom(&e, loose));
        assert!(
            l1 < STRATA_CLIFF * 0.25,
            "the loose pile wakes itself and collapses toward repose: {l0} -> {l1}"
        );
        assert!(
            h1 > h0 * 0.9,
            "the consolidated riser is left standing: {h0} -> {h1}"
        );
        assert!(
            h1 - l1 > h0 * 0.5,
            "SURVIVAL IS A RATE and the material sets it: consolidated kept \
             {h1} of {h0}, loose kept {l1} of {l0}"
        );
    }

    /// **A STAGED DRAINAGE** — a plateau with one ramp cut down into it and a
    /// basin at its foot, so the stream tree is known BY CONSTRUCTION: each
    /// reach's only lower neighbour is the next reach down, the basin takes
    /// the water, and the sea over the basin is the whole drainage's base
    /// level. The fixture the mobile-base-level gates share; returns the
    /// ramp headwater-first, the basin last.
    fn staged_ramp(map: &HexMap, e: &mut Evolution, head: TileId, reaches: usize) -> Vec<TileId> {
        const PLATEAU: f32 = 4.0;
        const STEP: f32 = 0.3;
        const BASIN: f32 = 0.6;
        for b in e.base.iter_mut() {
            *b = PLATEAU;
        }
        let mut on = vec![false; map.len()];
        on[head as usize] = true;
        let mut ramp = vec![head];
        while ramp.len() < reaches {
            let last = *ramp.last().expect("the ramp has a head");
            // The next reach must touch the ramp ONCE — a reach folding back
            // beside an earlier one would hand it a short cut, and the tree
            // would stop being the ramp.
            let next = *map
                .neighbours(last)
                .iter()
                .find(|nb| {
                    !on[**nb as usize]
                        && map
                            .neighbours(**nb)
                            .iter()
                            .filter(|x| on[**x as usize])
                            .count()
                            == 1
                })
                .expect("the ramp has somewhere to go");
            on[next as usize] = true;
            ramp.push(next);
        }
        for (k, t) in ramp.iter().enumerate() {
            e.base[*t as usize] = PLATEAU - STEP * (k + 1) as f32;
        }
        e.base[*ramp.last().expect("a basin") as usize] = BASIN;
        ramp
    }

    /// **THE FALL CLIMBS THE DRAINAGE AT A FINITE CELERITY** (Phase 2, the
    /// mechanism gate). The forcing already moves the base level — the sea
    /// is a percentile of a changing height distribution and the ice age
    /// walks it up and down. What this pins is that the LAND'S ANSWER is
    /// finite-rate: a fall at the outlet is not erodible potential
    /// everywhere in the same tick.
    ///
    /// Three claims, all about RATES and none about landforms (935269B7):
    /// the reach at the outlet answers and the headwater does not (it is a
    /// FRONT, not a broadcast); the reach that answers does not answer in
    /// FULL (it is finite, not instantaneous); and the front does make
    /// ground over a run (it is bounded, not frozen).
    #[test]
    fn a_base_level_fall_climbs_the_drainage_at_a_finite_celerity() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.water_volume = map.len() as f32 * 0.02;
        let head = (0..map.len() as TileId)
            .find(|t| !crust.is_vent(*t))
            .expect("a quiet head");
        let ramp = staged_ramp(&map, &mut e, head, 8);
        let (near, far) = (ramp[ramp.len() - 2], ramp[0]);
        const SEA0: f32 = 1.80;
        const SEA1: f32 = 1.00;
        // Settle: the first stream pass adopts every column's outlet
        // exactly, so the world starts AT GRADE and every later departure is
        // the front's and nothing else.
        for t in &ramp {
            e.disturb(&map, *t);
        }
        for _ in 0..6 {
            for t in &ramp {
                e.rain[*t as usize] = 0.2;
            }
            e.tick(&map, &seams, &crust, SEA0);
        }
        assert!(
            (e.graded_level(near) - SEA0).abs() < 1e-5,
            "the reach at the mouth stands at grade before the fall: {}",
            e.graded_level(near)
        );
        let (near0, far0) = (e.graded_level(near), e.graded_level(far));

        // THE FALL. One tick at the new level.
        for t in &ramp {
            e.rain[*t as usize] = 0.2;
        }
        e.tick(&map, &seams, &crust, SEA1);
        let (near1, far1) = (e.graded_level(near), e.graded_level(far));
        assert!(
            near0 - near1 > 0.05,
            "the reach at the mouth ANSWERS the fall: {near0} -> {near1}"
        );
        assert!(
            near1 > SEA1 + 0.05,
            "and does NOT answer it in full — the answer is finite, not \
             instantaneous: {near1} vs the new level {SEA1}"
        );
        assert!(
            far0 - far1 < (near0 - near1) * 0.1,
            "the headwater has not heard of it yet — this is a FRONT, not a \
             broadcast: mouth moved {}, head moved {}",
            near0 - near1,
            far0 - far1
        );

        // …and over a run the front MAKES GROUND: the mouth cuts, its own
        // giver's control falls with it, and the knickzone climbs.
        for _ in 0..60 {
            for t in &ramp {
                e.rain[*t as usize] = 0.2;
            }
            e.tick(&map, &seams, &crust, SEA1);
        }
        let climbed = ramp[..ramp.len() - 2]
            .iter()
            .filter(|t| e.graded_level(**t) < e.ground(**t))
            .count();
        assert!(
            climbed > 0,
            "the front is not frozen: some reach above the mouth is now \
             carrying a fall it has not answered"
        );
        assert!(
            e.graded_level(far) > SEA1,
            "and it is bounded — the divide has NOT adopted the new base \
             level: {} vs {SEA1}",
            e.graded_level(far)
        );
    }

    /// **HARD ROCK SLOWS THE FRONT** (Phase 2, the celerity discriminator).
    /// One fixture, run twice, the ONLY difference the GRADE of the face the
    /// channel has to cut — same mass, same heights, same discharge, same
    /// fall — and the soft reach adopts more of that fall in the same tick.
    /// A rate differential, measured on the tick the fall lands, before
    /// erosion has moved anything worth measuring.
    #[test]
    fn the_harder_the_face_the_slower_the_front() {
        let (map, seams, crust, _plates) = world();
        let head = (0..map.len() as TileId)
            .find(|t| !crust.is_vent(*t))
            .expect("a quiet head");
        const SEA0: f32 = 1.80;
        const SEA1: f32 = 1.00;
        let answered = |grade: f32| -> f32 {
            let mut e = Evolution::new(&map, &seams);
            e.water_volume = map.len() as f32 * 0.02;
            let ramp = staged_ramp(&map, &mut e, head, 8);
            let near = ramp[ramp.len() - 2];
            for t in &ramp {
                e.disturb(&map, *t);
            }
            for _ in 0..3 {
                e.tick(&map, &seams, &crust, SEA0);
            }
            // The face, and nothing else: the SAME loose pile on the same
            // column, at two grades, planted on the tick the fall lands.
            // Height, mass, drop and discharge are identical between the
            // runs by construction — only what the channel must cut differs.
            e.sediment[near as usize] = 0.0;
            e.rock[near as usize] = 0.5;
            e.rock_hard[near as usize] = grade;
            assert!(
                (e.face_grade(near as usize) - grade).abs() < 1e-6,
                "the staged face is the one the celerity reads: {} vs {grade}",
                e.face_grade(near as usize)
            );
            let before = e.graded_level(near);
            e.tick(&map, &seams, &crust, SEA1);
            before - e.graded_level(near)
        };
        let (soft, hard) = (answered(0.6), answered(META_HARD_CAP));
        assert!(
            soft > 0.0 && hard > 0.0,
            "both faces answer the fall at all: soft {soft}, hard {hard}"
        );
        assert!(
            soft > hard * 2.0,
            "THE FRONT IS A RATE AND THE ROCK SETS IT: a soft face adopted \
             {soft} of the fall where a metamorphic one adopted {hard}"
        );
    }

    /// **DROWNING IS IMMEDIATE; INCISION IS NOT** (Phase 2, the asymmetry).
    /// The sea covers what it covers — a level that comes UP is adopted in
    /// the same tick, because there is no erosional response to rate-limit
    /// and the reclassification the era has always done still stands. A
    /// level that goes DOWN is adopted at the celerity. One reach, one
    /// fixture, the two directions measured against each other.
    #[test]
    fn a_rising_sea_is_answered_at_once_and_a_falling_one_is_not() {
        let (map, seams, crust, _plates) = world();
        let head = (0..map.len() as TileId)
            .find(|t| !crust.is_vent(*t))
            .expect("a quiet head");
        const SEA0: f32 = 1.20;
        const SWING: f32 = 0.4;
        let step = |to: f32| -> (f32, f32) {
            let mut e = Evolution::new(&map, &seams);
            e.water_volume = map.len() as f32 * 0.02;
            let ramp = staged_ramp(&map, &mut e, head, 8);
            let near = ramp[ramp.len() - 2];
            for t in &ramp {
                e.disturb(&map, *t);
            }
            for _ in 0..3 {
                e.tick(&map, &seams, &crust, SEA0);
            }
            let before = e.graded_level(near);
            e.tick(&map, &seams, &crust, to);
            (before, e.graded_level(near))
        };
        let (up0, up1) = step(SEA0 + SWING);
        let (down0, down1) = step(SEA0 - SWING);
        assert!(
            (up0 - SEA0).abs() < 1e-5 && (down0 - SEA0).abs() < 1e-5,
            "both arms start at the same grade: {up0} / {down0}"
        );
        assert!(
            (up1 - (SEA0 + SWING)).abs() < 1e-5,
            "a RISE is adopted whole, in the tick it happens — drowning is \
             immediate: {up1} vs {}",
            SEA0 + SWING
        );
        assert!(
            down1 > (SEA0 - SWING) + SWING * 0.2,
            "a FALL is not — the erosional answer is the only side that is \
             rate-limited: {down1} vs {}",
            SEA0 - SWING
        );
    }

    /// **A STEADY OUTLET COSTS THE FRONT NOTHING — and a planet from before
    /// the front stands up at grade** (Phase 2, the compatibility anchor;
    /// Phase 1's null-fabric gate in this phase's currency).
    ///
    /// Where a column's outlet is not falling, its graded level SNAPS onto
    /// that outlet and stays there however hard the column is cutting — so
    /// the lag it multiplies the water-cut budget by is exactly 1.0, and
    /// `× 1.0` leaves every take bit-for-bit what it was before the front
    /// existed. That is the compatibility property, and it is also how an
    /// older epoch behaves: a planet captured before the graded level
    /// existed restores UN-MET, and its first stream pass adopts every
    /// column's live outlet exactly — a world at grade, which is the world
    /// it was captured from.
    #[test]
    fn a_steady_outlet_costs_the_front_nothing_and_an_older_planet_stands_at_grade() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.water_volume = map.len() as f32 * 0.02;
        let head = (0..map.len() as TileId)
            .find(|t| !crust.is_vent(*t))
            .expect("a quiet head");
        let ramp = staged_ramp(&map, &mut e, head, 8);
        let mouth = ramp[ramp.len() - 2];
        // A line the mouth stays above and the basin's delta stays under for
        // the whole run — the outlet must not move for the claim to mean
        // anything, and the gate below refuses to pretend otherwise.
        const SEA: f32 = 1.50;
        let basin = *ramp.last().expect("a basin");
        let started = e.ground(mouth);
        for t in &ramp {
            e.disturb(&map, *t);
        }
        for _ in 0..30 {
            for t in &ramp {
                e.rain[*t as usize] = 0.2;
            }
            e.tick(&map, &seams, &crust, SEA);
            // Every tick, not just the last: while the mouth stands over the
            // sea and the sea stands over the basin, the reach's outlet is
            // the water line and nothing else — and it is AT GRADE, exactly,
            // however hard it is cutting.
            assert!(
                e.ground(mouth) > SEA && e.ground(basin) < SEA,
                "the mouth still drains to the standing sea: {} / {}",
                e.ground(mouth),
                e.ground(basin)
            );
            assert_eq!(
                e.graded_level(mouth),
                SEA,
                "a reach whose outlet does not move stands exactly at grade"
            );
        }
        assert!(
            e.ground(mouth) < started - 1e-4,
            "and it was really cutting while it did: {started} -> {}",
            e.ground(mouth)
        );

        // THE OLDER PLANET: strip the graded levels out of a capture the way
        // an epoch written before Phase 2 simply does not have them.
        let file = e.capture(&map, &seams, "pre-front");
        let json = file.to_json().expect("the planet serializes");
        let mut v: serde_json::Value = serde_json::from_str(&json).expect("the file is json");
        let led = v["ledger"]
            .as_object_mut()
            .expect("the ledger is an object");
        assert!(
            led.remove("graded").is_some(),
            "the graded level was in the file to begin with"
        );
        let old = crate::epochfile::PlanetEpoch::from_json(&v.to_string())
            .expect("a pre-front epoch still validates");
        let mut e2 = Evolution::new(&map, &seams);
        e2.restore(&map, &seams, &crust, &old)
            .expect("a pre-front planet restores");
        assert!(
            (0..map.len()).all(|i| e2.graded[i] == f32::MIN),
            "it stands up UN-MET — no column has seen its outlet yet"
        );
        e2.disturb(&map, mouth);
        e2.tick(&map, &seams, &crust, SEA);
        assert!(
            (0..map.len() as TileId)
                .all(|t| e2.graded_level(t).is_finite() && e2.graded_level(t) > f32::MIN),
            "and the first stream pass reaches EVERY column, frontier or not \
             — none is left carrying the sentinel"
        );
        assert_eq!(
            e2.graded_level(mouth),
            SEA,
            "the reach over the sea lands exactly at grade, lag 1.0 — the \
             era this planet was captured from"
        );
    }

    /// **THE CHANNEL LEAVES ITS SHOULDER BEHIND** (Phase 2, the abandonment
    /// differential — the mechanism terraces are made of, gated as a RATE
    /// and never as a landform, per ruling R2 and 935269B7).
    ///
    /// One channel, ONE reach, and two shoulders standing over it that are
    /// identical in every way a geometry can be — same mass, same height
    /// above the same water, same tick-by-tick history — differing only in
    /// the GRADE of what they are made of. The sea falls, the reach answers
    /// it and cuts down between them, and neither shoulder is touched by
    /// anything except its own laws: neither is ever disturbed, so the only
    /// thing that can reach them is the oversteep wake, and the only thing
    /// the wake reads is the face (0b, ruling R1).
    ///
    /// NOTHING PINS EITHER OF THEM. The consolidated shoulder keeps its
    /// height because its face is over the metamorphic floor and because its
    /// own summit lost the discharge when the channel left it behind; the
    /// loose one goes because the same wake finds it, and talus and repose
    /// take it down. Persistence is a PROCESS CONDITION — this asserts the
    /// two rates and nothing else.
    #[test]
    fn the_incising_channel_leaves_a_consolidated_shoulder_and_ramps_a_loose_one() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.water_volume = map.len() as f32 * 0.02;
        let head = (0..map.len() as TileId)
            .find(|t| !crust.is_vent(*t))
            .expect("a quiet head");
        let ramp = staged_ramp(&map, &mut e, head, 8);
        let on: Vec<bool> = (0..map.len())
            .map(|i| ramp.contains(&(i as TileId)))
            .collect();
        // The reaches wear a soft COVER — a channel cuts its own alluvium
        // long before it saws the basement, and this gate is about what the
        // shoulders do while it does.
        const COVER: f32 = 0.4;
        for t in &ramp[..ramp.len() - 1] {
            e.base[*t as usize] -= COVER;
            e.rock[*t as usize] = COVER;
            e.rock_hard[*t as usize] = 0.8;
        }
        // BOTH shoulders over the SAME reach, and not neighbours of each
        // other — one channel history, two materials, no cross-talk. The
        // HEADWATER reach, so what the shoulders watch is the channel's own
        // incision and not a trunk's load passing through.
        let reach = ramp[0];
        let free: Vec<TileId> = map
            .neighbours(reach)
            .iter()
            .copied()
            .filter(|nb| !on[*nb as usize])
            .collect();
        let sc = free[0];
        let sl = *free
            .iter()
            .find(|t| **t != sc && !map.neighbours(sc).contains(t))
            .expect("the reach has two shoulders that do not touch");
        const RELIEF: f32 = 0.5;
        const PILE: f32 = 0.5;
        /// A grade over the metamorphic floor — enough that the face
        /// ARRESTS, and short of loading the column into the basal ceiling.
        const META_SHOULDER: f32 = 1.6;
        // EQUAL DIMENSION, opposite material — the inverse probe's staging,
        // stood beside a channel instead of alone on a plain. The
        // consolidated shoulder is indurated strata (a face over the
        // metamorphic floor); the loose one is sediment, under it by
        // construction.
        for s in [sc, sl] {
            e.base[s as usize] = e.ground(reach) + RELIEF - PILE;
        }
        e.l3_h[sc as usize] = PILE * 0.6;
        e.l4_h[sc as usize] = PILE * 0.4;
        e.l3_hard[sc as usize] = META_SHOULDER;
        e.l4_hard[sc as usize] = META_SHOULDER;
        e.sediment[sl as usize] = PILE;
        assert!(
            e.face_grade(sc as usize) >= ARREST_GRADE && e.face_grade(sl as usize) < ARREST_GRADE,
            "the two shoulders sit on opposite sides of the material line: \
             {} / {}",
            e.face_grade(sc as usize),
            e.face_grade(sl as usize)
        );
        let stand = |e: &Evolution, s: TileId| e.ground(s) - e.ground(reach);
        let (c0, l0) = (stand(&e, sc), stand(&e, sl));
        let (sum_c0, sum_l0, reach0) = (e.ground(sc), e.ground(sl), e.ground(reach));
        assert!(
            (c0 - l0).abs() < 1e-4 && (c0 - RELIEF).abs() < 1e-4,
            "both shoulders start the same height over the same reach: \
             {c0} / {l0}"
        );
        // ONLY the ramp is ever disturbed: the shoulders are ABANDONED from
        // the first tick, exactly as the inverse probe stages its pair.
        for t in &ramp {
            e.disturb(&map, *t);
        }
        for _ in 0..6 {
            for t in &ramp {
                e.rain[*t as usize] = 0.25;
            }
            e.tick(&map, &seams, &crust, 1.80);
        }
        for _ in 0..200 {
            for t in &ramp {
                e.rain[*t as usize] = 0.25;
            }
            e.tick(&map, &seams, &crust, 0.90);
        }
        let (c1, l1) = (stand(&e, sc), stand(&e, sl));
        assert!(
            e.ground(reach) < reach0 - 0.1,
            "the channel really did cut down between them: {reach0} -> {}",
            e.ground(reach)
        );
        assert!(
            e.ground(sc) > sum_c0 - PILE * 0.2,
            "the consolidated shoulder keeps its own summit as the channel \
             leaves it behind: {sum_c0} -> {}",
            e.ground(sc)
        );
        assert!(
            e.ground(sl) < sum_l0 - PILE * 0.6,
            "the loose one ramps toward repose instead: {sum_l0} -> {}",
            e.ground(sl)
        );
        assert!(
            c1 > c0,
            "the consolidated shoulder stands HIGHER over its channel than \
             it did, because the channel went down and it did not: {c0} -> {c1}"
        );
        assert!(
            c1 - l1 > PILE * 0.6,
            "SURVIVAL IS A RATE AND THE MATERIAL SETS IT: over the same \
             reach, at the same starting height, consolidated stands {c1} \
             where loose stands {l1}"
        );
    }

    /// A SEALED SITE (Phase 3's shared staging): a consolidated marine bed on
    /// `t`, at marine grade `press`, with its ring raised so the column is
    /// its own outlet. That one geometric fact does three jobs at once — it
    /// makes the site an internally-drained pit (Phase 2's own-floor branch),
    /// it puts the site outside every DOWNSLOPE law (a pit sheds nothing, and
    /// an undisturbed column under the cliff line is never woken), and it
    /// stops the water carrying anything away, so what the bed loses and what
    /// the store gains can be compared exactly.
    fn sealed_bed(map: &HexMap, e: &mut Evolution, t: TileId, press: f32, bed: f32) {
        let i = t as usize;
        for nb in map.neighbours(t) {
            e.base[*nb as usize] += 1.0;
        }
        e.l3_h[i] = bed;
        e.l3_hard[i] = SED_GRADE;
        e.bed_hard[i] = press;
    }

    /// **THE SOLUBLE BED GOES INTO SOLUTION AND ITS INSOLUBLE TWIN DOES NOT**
    /// (Phase 3, the removal channel's discriminator). Two beds of identical
    /// thickness and identical grade, under identical wet throughput, on
    /// identical geometry — the ONLY difference is whether the standing water
    /// ever indurated the column past the era's own carbonate line, which is
    /// the whole of the soluble mapping. Both sites are sealed, so no
    /// downslope law can reach either of them and the only thing that can
    /// remove bed is the new channel.
    ///
    /// This asserts a MECHANISM and a rate differential (935269B7): that
    /// there is a removal-in-place at all, that it discriminates on material,
    /// and that it CONSERVES — the thickness the bed loses is exactly the
    /// volume the store gains, to the arithmetic.
    #[test]
    fn a_soluble_bed_dissolves_and_an_insoluble_twin_does_not() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // A thin sheet of water for a sane climate; the beds stand DRY.
        e.water_volume = map.len() as f32 * 0.02;
        let (a, b) = two_quiet_sites(&map, &seams, &crust);
        let (ai, bi) = (a as usize, b as usize);
        const BED: f32 = 0.5;
        // Over the carbonate line — the marine press made this column
        // carbonate country — versus uncompacted floor that never was.
        sealed_bed(&map, &mut e, a, MARINE_HARD_CAP, BED);
        sealed_bed(&map, &mut e, b, 1.0, BED);
        let wet = |e: &mut Evolution| {
            e.rain[ai] = 0.25;
            e.rain[bi] = 0.25;
        };
        let sea = e.resolve_sea();
        assert!(
            e.ground(a) > sea && e.ground(b) > sea,
            "both beds stand in the air where meteoric water reaches them"
        );

        // ONE tick first, so the conservation claim is the arithmetic itself
        // and not an accumulation over many.
        wet(&mut e);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        let step = BED - e.l3_h[ai];
        assert!(step > 0.0, "the soluble bed loses thickness at all: {step}");
        assert!(
            (e.dissolved(a) - step * e.area[ai]).abs() < 1e-7,
            "THE DEBIT IS THE CREDIT: the bed lost {step} where the store \
             took {}",
            e.dissolved(a) / e.area[ai]
        );

        for _ in 0..60 {
            wet(&mut e);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.l3_h[ai] < BED - step,
            "and it keeps going while the water runs: {}",
            e.l3_h[ai]
        );
        assert_eq!(
            e.l3_h[bi], BED,
            "THE INSOLUBLE TWIN LOSES NOTHING AT ALL — the same water over \
             the same bed on a column the sea never made carbonate"
        );
        assert_eq!(
            e.dissolved(b),
            0.0,
            "and puts nothing into solution: the store never opened for it"
        );
    }

    /// **DISSOLUTION RUNS THE BEDDING** (Phase 3, the fabric preference —
    /// Phase 1's differential, read a second time by a second channel). A
    /// dipping soluble bed is not equally attackable from every direction:
    /// water follows the bedding planes, so it opens the rock ALONG the
    /// strike and has to work ACROSS it. This is why the phase order matters
    /// — isotropic dissolution only lowers a plateau uniformly; it takes the
    /// fabric to turn removal-in-place into RELIEF.
    ///
    /// One fixture, run twice, the only difference the recorded TREND. Never
    /// disturbed and under the cliff line, so the mechanical takes never run
    /// and what is measured is the dissolution channel alone.
    #[test]
    fn dissolution_runs_along_the_bedding_faster_than_across_it() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let (map, seams, crust, _plates) = world();
        let (a, _b) = two_quiet_sites(&map, &seams, &crust);
        let ai = a as usize;
        // ONE unambiguous drain, so the flow direction the fabric is read
        // against is known in advance — the Phase 1 gate's own staging.
        let drain = map.neighbours(a)[0];
        let p = map.direction(a);
        let toward = map.direction(drain) - p;
        let flow = (toward - p * p.dot(toward)).normalize();
        let (east, north) = Evolution::frame(p);
        let flow_az = flow.dot(north).atan2(flow.dot(east)).rem_euclid(PI);
        const BED: f32 = 0.5;
        let run = |strike: f32| -> f32 {
            let mut e = Evolution::new(&map, &seams);
            e.water_volume = map.len() as f32 * 0.02;
            e.l3_h[ai] = BED;
            e.l3_hard[ai] = SED_GRADE;
            e.bed_hard[ai] = MARINE_HARD_CAP;
            e.l3_dip[ai] = DIP_CAP;
            e.strike[ai] = strike;
            e.base[drain as usize] -= 0.15;
            e.rain[ai] = 0.25;
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
            BED - e.l3_h[ai]
        };
        let along = run(flow_az);
        let across = run((flow_az + FRAC_PI_2).rem_euclid(PI));
        assert!(
            across > 0.0,
            "a bed across the strike still dissolves — a fabric may never \
             zero a channel: {across}"
        );
        assert!(
            along > across,
            "water opens the bed ALONG the strike faster than across it: \
             {along} vs {across}"
        );
        // The SAME clamp band Phase 1's cut spends: 1 ± ANISO_SPAN, and a
        // bed at the dip cap spends the whole of it and no more.
        let band = (1.0 + ANISO_SPAN) / (1.0 - ANISO_SPAN);
        let ratio = along / across;
        assert!(
            ratio <= band * 1.001 && ratio >= band * 0.99,
            "the differential is the band, whole and no wider: {ratio}× of \
             {band}×"
        );
    }

    /// **A DRYING BASIN PRECIPITATES ITS CAP AND KEEPS THE REST IN SOLUTION**
    /// (Phase 3, the return channel — and the close of 8CA52AC5's standing
    /// item: *"salt is not a hydrothermal vein… it forms by a sea
    /// evaporating"*). An enclosed sink on a hot, rain-starved world, handed
    /// an over-saturated brine in one go.
    ///
    /// Three claims, and the second is the one the five-round tower incident
    /// paid for (B390DA57): the load comes OUT of solution where the
    /// condition reverses; the delivery is capped at `INTAKE_CAP` like every
    /// other transport in the era; and the excess STAYS IN SOLUTION at the
    /// same tile rather than landing anyway. Plus: the new bed CONSOLIDATES
    /// — it joins the grade system as vein-layer mass, not a loose pile.
    #[test]
    fn a_drying_basin_precipitates_its_cap_and_keeps_the_rest_in_solution() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        // HOT and nearly waterless: the sky brings almost nothing, so the
        // sun outruns it — the condition an evaporite basin is made of.
        e.set_climate(1.0);
        e.water_volume = map.len() as f32 * 0.02;
        let (a, _b) = two_quiet_sites(&map, &seams, &crust);
        let ai = a as usize;
        for nb in map.neighbours(a) {
            e.base[*nb as usize] += 1.0; // enclosed: nothing drains away
        }
        /// Far more brine than one tick can lay down — the over-saturation
        /// the cap has to hold back.
        const BRINE: f32 = 5.0;
        e.dissolved[ai] = BRINE * e.area[ai];
        let dry = |e: &mut Evolution| {
            e.rain[ai] = 0.0;
            for nb in map.neighbours(a) {
                e.rain[*nb as usize] = 0.0;
            }
        };
        dry(&mut e);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        let laid = e.l3_h[ai];
        assert!(laid > 0.0, "the basin laid a bed down at all: {laid}");
        assert!(
            (laid - INTAKE_CAP).abs() < 1e-5,
            "AND NO MORE THAN ITS INTAKE: {laid} against the cap {INTAKE_CAP}"
        );
        assert!(
            (e.dissolved(a) / e.area[ai] - (BRINE - laid)).abs() < 1e-4,
            "THE REST STAYED IN SOLUTION — the store is its own holding \
             state: {} of {BRINE} still dissolved",
            e.dissolved(a) / e.area[ai]
        );
        assert_eq!(
            e.sediment[ai], 0.0,
            "and none of it piled as loose height: an evaporite is a BED"
        );
        assert!(
            (e.strata_hardness(a).0 - SED_GRADE).abs() < 1e-6,
            "the bed entered the GRADE system at the precipitate grade: {}",
            e.strata_hardness(a).0
        );
        assert_eq!(
            e.strata_fabric(a).1,
            0.0,
            "and it lies FLAT, like every other settled bed"
        );
        // …and it keeps going, a capped delivery at a time, while the brine
        // lasts — the channel is rate-limited, never refused outright.
        for _ in 0..3 {
            dry(&mut e);
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.l3_h[ai] > laid,
            "the basin keeps laying bed down: {laid} -> {}",
            e.l3_h[ai]
        );
        assert!(
            (e.l3_h[ai] * e.area[ai] + e.dissolved(a) - BRINE * e.area[ai]).abs() < 1e-3,
            "and every unit is accounted for: {} laid + {} in solution",
            e.l3_h[ai],
            e.dissolved(a) / e.area[ai]
        );
    }

    /// **THE INTERNAL DRAIN** (Phase 3 × Phase 2's own-floor branch; ruling
    /// R2). A closed depression whose floor is soluble and wet deepens with
    /// NO surface outlet at all — the rock leaves from inside, in solution,
    /// and the floor goes down with it. Phase 2 already made a pit its own
    /// base level; this is what gives that branch something to do.
    ///
    /// PERSISTENCE IS A PROCESS CONDITION, both ways (R2, 935269B7). Nothing
    /// pins the depression: it deepens exactly while the soluble-and-wet
    /// drain condition holds, and the moment the water stops the deepening
    /// stops with it — not one more unit of bed goes into solution. No
    /// assertion here says a doline exists.
    #[test]
    fn a_soluble_floored_wet_pit_deepens_while_the_water_runs_and_stops_when_it_does() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.water_volume = map.len() as f32 * 0.02;
        let (a, _b) = two_quiet_sites(&map, &seams, &crust);
        let ai = a as usize;
        const BED: f32 = 0.5;
        sealed_bed(&map, &mut e, a, MARINE_HARD_CAP, BED);
        let rim: Vec<TileId> = map.neighbours(a).to_vec();
        let rim0: Vec<f32> = rim.iter().map(|t| e.ground(*t)).collect();
        let floor0 = e.ground(a);
        assert!(
            rim.iter().all(|t| e.ground(*t) > floor0),
            "the depression starts closed: no rim tile is below the floor"
        );

        // THE DRAIN CONDITION HOLDS: water runs through the soluble floor.
        for _ in 0..80 {
            e.rain[ai] = 0.25;
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        let floor1 = e.ground(a);
        let sank = floor0 - floor1;
        assert!(
            sank > 0.0,
            "the floor goes DOWN with no outlet at all: {floor0} -> {floor1}"
        );
        assert!(
            e.dissolved(a) > 0.0,
            "and what left it is in solution, not downslope: {}",
            e.dissolved(a)
        );
        assert!(
            rim.iter()
                .zip(&rim0)
                .all(|(t, h0)| (e.ground(*t) - h0).abs() < sank * 0.1),
            "THE RIM DOES NOT BREACH — nothing over the edge moved; the \
             depression is the floor leaving, not the wall failing"
        );

        // CUT THE WATER. The floor is still soluble and the pit is still a
        // pit — only the throughput is gone, and that alone is the whole
        // condition.
        for _ in 0..40 {
            e.rain[ai] = 0.0;
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.ground(a) >= floor1 - sank * 0.02,
            "THE DRAIN STOPS WHEN THE WATER STOPS: the floor no longer goes \
             down ({floor1} -> {})",
            e.ground(a)
        );
        let bed = e.l3_h[ai];
        e.rain[ai] = 0.0;
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        assert!(
            e.l3_h[ai] >= bed,
            "…and not one more unit of bed goes into solution: {bed} -> {}",
            e.l3_h[ai]
        );
    }

    /// **A LOAD IN SOLUTION HAS NO HEIGHT, AND A WORLD WITHOUT THE CLASS
    /// NEVER OPENS THE STORE** (Phase 3's compatibility anchor — Phase 1's
    /// null-fabric precedent in the shape this channel admits).
    ///
    /// Two claims. First, the currency law's core: the dissolved store is
    /// water-borne VOLUME and nothing else, so a column carrying a large one
    /// must stand at exactly the same height as its twin carrying none and
    /// run bit-for-bit the same over a real stretch of era — no relief, no
    /// slope, no wake, no deposit sees it. (Both sites are sealed and WET,
    /// so the load can neither leave nor come back out: what is measured is
    /// the load simply sitting there.) Second, the additive claim: on ground
    /// the sea never indurated past the carbonate line there IS no soluble
    /// class, the channel never fires, and the store stays shut — which is
    /// what a pre-Phase-3 planet is.
    #[test]
    fn a_dissolved_load_has_no_height_and_a_world_without_the_class_never_opens_the_store() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.water_volume = map.len() as f32 * 0.02;
        let (a, b) = two_quiet_sites(&map, &seams, &crust);
        let (ai, bi) = (a as usize, b as usize);
        const BED: f32 = 0.5;
        // INSOLUBLE twins, so the only difference between them stays the
        // stamped load and nothing the channel itself does.
        sealed_bed(&map, &mut e, a, 1.0, BED);
        sealed_bed(&map, &mut e, b, 1.0, BED);
        e.dissolved[ai] = 4.0 * e.area[ai];
        for _ in 0..40 {
            e.rain[ai] = 0.25;
            e.rain[bi] = 0.25;
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        assert!(
            e.dissolved(a) > 0.0,
            "the load is still there — it had nowhere to go and no reason \
             to come out"
        );
        let stores = |e: &Evolution, i: usize| {
            (
                e.base[i],
                e.l3_h[i],
                e.l4_h[i],
                e.rock[i],
                e.sediment[i],
                e.ground(i as TileId),
            )
        };
        assert_eq!(
            stores(&e, ai),
            stores(&e, bi),
            "A LOAD IN SOLUTION IS INVISIBLE TO RELIEF: the column carrying \
             it stands exactly where its empty twin stands, store for store"
        );

        // THE NULL WORLD: nothing here ever became carbonate country.
        let mut e2 = Evolution::new(&map, &seams);
        e2.set_water(70.0);
        for t in 0..map.len() as TileId / 7 {
            e2.disturb(&map, t * 7);
        }
        for _ in 0..40 {
            let sea = e2.resolve_sea();
            e2.tick(&map, &seams, &crust, sea);
        }
        assert!(
            (0..map.len()).all(|i| e2.bed_hard[i] < MARINE_CALCITE_HARD),
            "this stretch of era never indurates a bed past the carbonate \
             line — there is no soluble class in this world"
        );
        assert!(
            (0..map.len() as TileId).all(|t| e2.dissolved(t) == 0.0),
            "and so the store never opens: not one column put anything into \
             solution"
        );
    }

    /// **THE DISSOLVED LOAD IS DURABLE — and its absence is legal.** What is
    /// in transit at capture must still be in transit at restore (the
    /// round-trip family's contract: a store that skips the format is a
    /// planet that quietly loses material). And an epoch written BEFORE the
    /// channel existed — the array simply not in the file — still loads,
    /// standing its world up with an empty store, which is exactly what that
    /// planet had.
    #[test]
    fn the_dissolved_load_survives_the_epoch_and_a_pre_dissolution_file_loads_empty() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(70.0);
        for t in 0..map.len() as TileId / 7 {
            e.disturb(&map, t * 7);
        }
        for _ in 0..8 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
        }
        for t in (0..map.len()).step_by(5) {
            e.dissolved[t] = 0.1 + (t % 3) as f32 * 0.25;
        }
        let file = e.capture(&map, &seams, "solution round-trip");
        let json = file.to_json().expect("the planet serializes");
        let back = crate::epochfile::PlanetEpoch::from_json(&json).expect("the planet validates");
        let mut e2 = Evolution::new(&map, &seams);
        e2.restore(&map, &seams, &crust, &back)
            .expect("restore succeeds");
        assert_eq!(
            e2.capture(&map, &seams, "solution round-trip"),
            file,
            "the store replays with every other durable field"
        );
        assert_eq!(
            e2.dissolved(5),
            e.dissolved(5),
            "and every unit comes back exactly"
        );

        // A PRE-DISSOLUTION EPOCH: strip the array out of the file entirely,
        // the way a planet captured before Phase 3 has it.
        let mut v: serde_json::Value = serde_json::from_str(&json).expect("the file is json");
        let led = v["ledger"]
            .as_object_mut()
            .expect("the ledger is an object");
        assert!(
            led.remove("dissolved").is_some(),
            "the store was in the file to begin with"
        );
        let old = crate::epochfile::PlanetEpoch::from_json(&v.to_string())
            .expect("a pre-dissolution epoch still validates");
        let mut e3 = Evolution::new(&map, &seams);
        e3.restore(&map, &seams, &crust, &old)
            .expect("a pre-dissolution planet restores");
        assert!(
            (0..map.len() as TileId).all(|t| e3.dissolved(t) == 0.0),
            "a pre-dissolution planet stands up with NOTHING in solution"
        );
    }

    /// A head-on jam on cold, vent-free ground with the foreland's material
    /// set to `grade` — one fixture, run twice, so the ONLY difference
    /// between the two worlds is what the converging material is made of.
    /// The grade rides the WHOLE column — the loose cover at the grade
    /// itself and every bed indurated to match (clamped into the marine
    /// press's own band) — because the disposition reads both.
    fn jam_with_grade(
        map: &HexMap,
        seams: &SeamField,
        crust: &CrustField,
        grade: f32,
    ) -> (f32, f32) {
        let mut e = Evolution::new(map, seams);
        let quiet = |e: &Evolution, t: TileId| {
            seams.heat(t) <= 0.0
                && !crust.is_vent(t)
                && map.neighbours(t).iter().all(|nb| !crust.is_vent(*nb))
                && e.rock(t) == 0.0
        };
        let t = (0..map.len() as TileId)
            .find(|t| quiet(&e, *t) && map.neighbours(*t).iter().all(|nb| quiet(&e, *nb)))
            .expect("cold vent-free ground exists");
        let j = map.neighbours(t)[0];
        let (ti, ji) = (t as usize, j as usize);
        let p = map.direction(t);
        let toward = map.direction(j) - p;
        let dir = (toward - p * p.dot(toward)).normalize();
        e.rock[ti] = 0.75;
        e.rock[ji] = 0.6;
        e.rock_hard[ji] = grade;
        // The BED carries the grade too, clamped into the marine press's own
        // band — the disposition reads the basement as well as the cover, so
        // a fixture that graded only the loose pile would leave half the
        // claim untested.
        e.bed_hard.fill(grade.clamp(1.0, MARINE_HARD_CAP));
        e.push[ti] = dir * RATE_MAX;
        e.push[ji] = -dir * RATE_MAX;
        e.drift[ti] = 1.0; // fires this tick
        e.disturb(map, t);
        let sea = e.resolve_sea();
        e.tick(map, seams, crust, sea);
        (e.well(), e.grown(t))
    }

    /// **THE DENSE FLOOR SUBDUCTS, THE CONSOLIDATED WEDGE STANDS** (Aaron
    /// 2026-08-29, reinstating the well; the buoyant standoff's disposition,
    /// DCA4D316 — land does not ride over land). One jam fixture, two runs,
    /// identical process rules; the only difference is the GRADE of the
    /// foreland's material. Young mafic floor at the softest the vents pour
    /// leaves the surface for the well; consolidated metamorphic material
    /// arrests and crumple-thickens through the same scrape as ever. A
    /// discriminator on the RULES, never an assertion about a landform.
    /// (0b: the disposition now reads the foreland's BASEMENT as well as its
    /// cover, so the fixture grades the whole column — the claim is
    /// unchanged, the surface it covers is wider.)
    #[test]
    fn the_dense_floor_subducts_and_the_consolidated_wedge_stands() {
        let (map, seams, crust, _plates) = world();
        let (soft_well, soft_wedge) = jam_with_grade(&map, &seams, &crust, VENT_HARD_MIN);
        let (hard_well, hard_wedge) = jam_with_grade(&map, &seams, &crust, META_HARD_CAP);
        assert!(
            soft_well > 0.0,
            "the dense floor goes DOWN: well {soft_well}"
        );
        assert!(
            hard_well == 0.0,
            "consolidated material never sinks: well {hard_well}"
        );
        assert!(
            hard_wedge > soft_wedge,
            "what arrests thickens the wedge, what subducts does not: \
             consolidated {hard_wedge} vs floor {soft_wedge}"
        );
    }

    /// **THE BASAL CEILING FIRES** (064F3B58's lesson: a bound that "works"
    /// must be caught actually firing — DELAMINATION_PA opened 0 times in
    /// 5762 columns because it was an imported absolute). A column loaded
    /// past what its root can carry, in the era's OWN overburden units:
    /// the base founders, the stripped material goes DOWN the well, and the
    /// counter moves.
    #[test]
    fn the_basal_ceiling_founders_the_overloaded_root() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        let t = (0..map.len() as TileId)
            .find(|t| seams.heat(*t) <= 0.0 && !crust.is_vent(*t))
            .expect("cold vent-free ground exists");
        let i = t as usize;
        // A full ladder under the marine press, on the site AND its ring, so
        // nothing here is a cliff and the slope laws have nothing to shave:
        // the ONLY rule that can act on this column is the ceiling.
        for x in std::iter::once(t).chain(map.neighbours(t).iter().copied()) {
            let k = x as usize;
            e.base[k] = BASE_CAP;
            e.l3_h[k] = L3_CAP;
            e.l4_h[k] = L4_CAP;
            e.bed_hard[k] = MARINE_HARD_CAP;
        }
        // The site alone carries METAMORPHIC grades, so its load — and only
        // its load — stands over what its root can carry.
        e.l3_hard[i] = UPLIFT_HARD;
        e.l4_hard[i] = UPLIFT_HARD;
        e.disturb(&map, t);
        assert!(
            e.overburden(i) > DELAM_LOAD,
            "the fixture stands over the ceiling: {} vs {DELAM_LOAD}",
            e.overburden(i)
        );
        let root = |e: &Evolution| e.base[i] + e.l3_h[i] + e.l4_h[i];
        let before = root(&e);
        let sea = e.resolve_sea();
        e.tick(&map, &seams, &crust, sea);
        assert!(e.delaminations() > 0, "the ceiling FIRES");
        assert!(
            root(&e) < before,
            "the root founders: {before} -> {}",
            root(&e)
        );
        assert!(
            e.base[i] < BASE_CAP,
            "the strip starts at the BASE: {}",
            e.base[i]
        );
        assert!(
            e.well() >= (before - root(&e)) * e.area[i] - 1e-6,
            "what foundered went DOWN the well: {}",
            e.well()
        );
    }

    /// **THE FOUNTAINS SPEND ONLY WHAT THE COLLISIONS SANK** — the
    /// count-then-spend circuit (064F3B58). The CYCLE ORDER is the barrier:
    /// Upwell, the one spender, runs before every creditor, so a tick can
    /// never hand back material it has not yet sunk. And the draw is real:
    /// under identical rules a primed well funds most of the pour while an
    /// empty one is covered by fresh melt, and neither ever overspends.
    #[test]
    fn the_fountains_spend_only_what_the_collisions_sank() {
        let at = |p: Phase| PHASES.iter().position(|q| *q == p).expect("in the cycle");
        assert!(
            at(Phase::Upwell) < at(Phase::Push)
                && at(Phase::Upwell) < at(Phase::Compact)
                && at(Phase::Upwell) < at(Phase::Weld),
            "count THEN spend: the spender runs before every creditor"
        );
        let (map, seams, crust, _plates) = world();
        let run = |primed: f32| -> (f32, f32) {
            let mut e = Evolution::new(&map, &seams);
            e.set_water(76.0);
            e.well = primed;
            for _ in 0..60 {
                let sea = e.resolve_sea();
                e.tick(&map, &seams, &crust, sea);
            }
            let grown: f32 = (0..map.len() as TileId)
                .map(|t| (e.base(t) + e.grown(t)) * e.area[t as usize])
                .sum();
            (grown, e.well())
        };
        let (dry_grown, dry_left) = run(0.0);
        let (fed_grown, fed_left) = run(1_000.0);
        assert!(
            dry_grown > 0.0 && dry_left >= 0.0,
            "an empty well still spreads — the mantle covers the shortfall"
        );
        assert!(
            fed_left < 1_000.0,
            "a primed well is actually drawn down: {fed_left}"
        );
        assert!(fed_left >= 0.0, "never overspent: {fed_left}");
        // Production is NEUTRAL: the well changes where the rock came from,
        // never how much the vents pour (8D917A78 — a return sized by the
        // slab instead of the injector quadrupled the crust).
        assert!(
            (fed_grown - dry_grown).abs() < dry_grown * 1e-3,
            "the pour is unchanged by the well: dry {dry_grown} vs fed {fed_grown}"
        );
    }

    #[test]
    #[ignore = "diagnostic probe — run with --ignored"]
    fn probe_well() {
        // BENCH-TRUE: the shipped world size, the water-world start and the
        // geological drift cadence the scene runs (every 12 ticks).
        let map = HexMap::new(96);
        let mut seams = SeamField::new(&map, 6, 4, 42);
        let mut crust = CrustField::derive(&map, &seams);
        let mut e = Evolution::new(&map, &seams);
        e.set_climate(0.63);
        e.set_water(66.0);
        let mean_ground = |e: &Evolution| -> f32 {
            (0..map.len() as TileId).map(|t| e.ground(t)).sum::<f32>() / map.len() as f32
        };
        // The COLUMN volume — what the ledger's surface store holds. Read
        // against the well's own take, it separates the two rates the
        // equilibrium is made of: production P = ΔV + Δsunk, sink S = Δsunk.
        // Ground stops climbing when S reaches P, and the well then banks
        // the decompression floor's mint share.
        let column_volume = |e: &Evolution| -> f64 {
            (0..map.len() as TileId)
                .map(|t| e.ground(t) as f64 * e.area[t as usize] as f64)
                .sum()
        };
        const WINDOW: u32 = 3000;
        let (mut prev_g, mut early, mut late) = (mean_ground(&e), 0.0f32, 0.0f32);
        let (mut prev_v, mut prev_s) = (column_volume(&e), 0.0f64);
        for k in 0..=12000u32 {
            if k > 0 {
                let sea = e.resolve_sea();
                e.tick(&map, &seams, &crust, sea);
                if k % 12 == 0 {
                    seams.drift(&map, 0.06);
                    crust = CrustField::derive(&map, &seams);
                    e.derive_motion(&map, &seams);
                }
            }
            if k % WINDOW == 0 {
                let mut load: Vec<f32> = (0..map.len()).map(|i| e.overburden(i)).collect();
                load.sort_by(f32::total_cmp);
                let (p50, p99, top) = (
                    load[load.len() / 2],
                    load[load.len() * 99 / 100],
                    load[load.len() - 1],
                );
                let g = mean_ground(&e);
                let grew = g - prev_g;
                if k == WINDOW {
                    early = grew;
                }
                if k == 12000 {
                    late = grew;
                }
                prev_g = g;
                let (sunk, well) = (e.sunk(), e.well());
                let welled = if sunk > 0.0 {
                    (sunk - well) / sunk
                } else {
                    0.0
                };
                let (v, s) = (column_volume(&e), sunk as f64);
                let (dv, ds) = (v - prev_v, s - prev_s);
                prev_v = v;
                prev_s = s;
                // THE DISSOLUTION READOUT (Phase 3, E91E482D's demand: a
                // channel that "works" must be caught actually firing on a
                // bench-true world, not only on its staged gate). `sol` is
                // how many columns the era has made soluble on its own —
                // marine-indurated past the carbonate line, short of the
                // metamorphic floor, standing in the air; `karst` is how
                // many of those have water actually moving through them;
                // `soln` is what is in solution right now.
                let sea_now = e.resolve_sea();
                let (mut sol, mut karst) = (0usize, 0usize);
                for i in 0..map.len() {
                    let t = i as TileId;
                    if e.l3_h[i] <= 0.0
                        || e.bed_hard[i] < MARINE_CALCITE_HARD
                        || e.l3_hard[i] >= UPLIFT_HARD
                        || e.ground(t) < sea_now
                    {
                        continue;
                    }
                    sol += 1;
                    if e.rain[i] + DISSOLVE_FLOW * e.discharge[i].sqrt() > DISSOLVE_WET {
                        karst += 1;
                    }
                }
                let soln: f32 = (0..map.len() as TileId).map(|t| e.dissolved(t)).sum();
                eprintln!(
                    "WELL k={k} sunk={sunk:.1} banked={well:.1} welled/sunk={welled:.3} \
                     delam={} load p50={p50:.2} p99={p99:.2} max={top:.2} \
                     ceiling={DELAM_LOAD:.2} mean_ground={g:.3} (+{grew:.3}/{WINDOW}t) \
                     cover={:.3} dV={dv:.1} dSink={ds:.1} S/P={:.3} \
                     sol={sol} karst={karst} soln={soln:.2}",
                    e.delaminations(),
                    e.coverage(),
                    if dv + ds > 0.0 { ds / (dv + ds) } else { 0.0 }
                );
            }
        }
        eprintln!("WELL early +{early:.4}/{WINDOW}t  late +{late:.4}/{WINDOW}t");
    }
}
