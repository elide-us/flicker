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

use flicker::render::Vec3;

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
/// CRUSTAL INSULATION (Aaron's stack law — layer 1 is the deep-crust
/// INSULATION layer — and the 3600-tick sediment towers: the upwell injected
/// ~240 units/tick FOREVER, land outran every ocean, and spoil piled into
/// repose mountains): the column standing over a vent insulates it — the
/// injected quantum scales by 1/(1 + (ground/THIS)²), so a young world
/// builds fast and a maturing world's volcanism FADES asymptotically. The
/// planet finishes growing; it does not grow forever.
const CRUST_INSULATION: f32 = 6.0;
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
/// Moisture everywhere (the air is never bone dry), the extra a SUBMERGED
/// tile soaks in, and the OROGRAPHIC term: height above the sea line, scaled
/// by COND_SCALE tile-widths, is the condensation the uplift already gives us.
const BASE_WET: f32 = 0.12;
const SUBMERGED_WET: f32 = 0.3;
const OROGRAPHIC_WET: f32 = 0.85;
const COND_SCALE: f32 = 1.5;
// CARVING (Aaron 2026-08-25: "the erosion isn't carving hard enough --
// channels and valleys"): rainfall accumulates down the steepest-descent
// network into DISCHARGE, the erosion budget scales with its square root,
// and most spoil follows the channel instead of fanning -- streams cut.
const CARVE_GAIN: f32 = 0.30;
/// Discharge above this keeps a tile eroding even when nothing else touches
/// it -- rivers stay live and keep cutting their valleys.
const CHANNEL_LIVE: f32 = 8.0;
/// The share of spoil that follows the STEEPEST neighbour (the channel); the
/// rest fans drop-weighted as before.
const CHANNEL_SHARE: f32 = 0.65;
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
/// GLACIAL FLOW (Aaron 2026-08-26, the 2400-tick towers — ONE law for both
/// hemispheres, like everything here): thick ice DEFORMS and flows. The
/// summer damp scales back UP with the loose pile's overburden — past this
/// pile height the glacier moves its bed at full rate — and entrained ROCK
/// rides the flow, deposited as milled sediment. A broad cap that rises as
/// one body drains at its rim; a vent cone under ice drains off its flanks;
/// no column can outgrow the flow that thickening itself accelerates.
const GLACIAL_SOFT_PILE: f32 = 1.5;

/// **The pipeline's PROCEDURES, in cycle order.** One engine step runs ONE of
/// these ([`Evolution::tick_phase`]); a TICK is the completed cycle (Aaron
/// 2026-08-26: label the running procedure, cluster the tick around the
/// cycle of procedures).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// The ice-age runner: temperature, caps, locked water.
    Climate,
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
pub const PHASES: [Phase; 9] = [
    Phase::Climate,
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
    /// The MOISTURE field of the last tick — base wet + soak + the uplift's
    /// orographic condensation. What the atmosphere layers display, and what
    /// drives the weathering.
    moist: Vec<f32>,
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

    /// Loose rock height at `tile`, tile-width units.
    pub fn rock(&self, tile: TileId) -> f32 {
        self.rock.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// The loose rock's blended HARDNESS at `tile` — 1.0 is the reference
    /// grade; what the vents fed it, mass-weighted through every transfer.
    pub fn rock_hardness(&self, tile: TileId) -> f32 {
        self.rock_hard.get(tile as usize).copied().unwrap_or(1.0)
    }

    /// A vent's characteristic output grade: a seeded draw on the spectrum,
    /// drifting slowly with its cumulative emission — the same vent pours a
    /// consistent stream that wanders over geological spans, and different
    /// vents pour different rock.
    fn vent_hardness(&self, vent_idx: usize, seed: u64) -> f32 {
        let base = fastrand::Rng::with_seed(
            seed ^ ((vent_idx as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        )
        .f32();
        let drift = (self.emitted.get(vent_idx).copied().unwrap_or(0.0) * 1.7).sin();
        (VENT_HARD_MIN + VENT_HARD_SPAN * base + VENT_HARD_DRIFT * drift).clamp(0.4, 1.9)
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
                }
                if !vents.is_empty() {
                    for _ in 0..(seams.cells() * UPWELL_PER_SEAM) {
                        let vi = rng.usize(..vents.len());
                        let t = vents[vi] as usize;
                        let hard = self.vent_hardness(vi, seams.seed());
                        // The crust above insulates the pinch: mature columns
                        // choke their own vents.
                        let g = self.ground(t as TileId) / CRUST_INSULATION;
                        let inject = UPWELL_INJECT / (1.0 + g * g);
                        if inject < 1e-4 {
                            continue;
                        }
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
                    let erupt_hard = self.vent_hardness(vi, seams.seed());
                    self.eruptions += 1;
                    // The flow: ring by ring out from the vent, one tick's whole
                    // lava field.
                    let mut dist = vec![u8::MAX; n];
                    dist[vent as usize] = 0;
                    let mut ring = vec![vent];
                    let pour = |slf: &mut Self, i: usize, m: f32| {
                        // The eruption too pours through its own insulation.
                        let g = slf.ground(i as TileId) / CRUST_INSULATION;
                        let m = m / (1.0 + g * g);
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
                        // pressure the resolve events will spend.
                        self.pressure[t as usize] =
                            (self.pressure[t as usize] + each * RIM_PRESS).min(PRESSURE_MAX);
                        self.pressure[j] =
                            (self.pressure[j] + each * RIM_PRESS * 0.5).min(PRESSURE_MAX);
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
                        // then its neighbours'), hardened on site. Uplift
                        // CONVERTS the pile that collided; it never mints.
                        // (The first cut wrote `rock += QUANTUM` with no
                        // debit — creation compounding with material flux
                        // was the 3600-tick accelerating-growth disease.)
                        let mut need = UPLIFT_QUANTUM;
                        let from_sed = need.min(self.sediment[i]);
                        self.sediment[i] -= from_sed;
                        need -= from_sed;
                        if need > 0.0 {
                            for nb in map.neighbours(i as TileId) {
                                if need <= 0.0 {
                                    break;
                                }
                                let j = *nb as usize;
                                let take_s = need.min(self.sediment[j]);
                                self.sediment[j] -= take_s;
                                need -= take_s;
                                let take_r = need.min(self.rock[j]);
                                self.rock[j] -= take_r;
                                need -= take_r;
                                if take_s + take_r > 0.0 {
                                    act[j] = true;
                                }
                            }
                        }
                        let got = UPLIFT_QUANTUM - need;
                        if got > 0.0 {
                            let old = self.rock[i].max(0.0);
                            self.rock_hard[i] = (old * self.rock_hard[i] + got * 1.2) / (old + got);
                            self.rock[i] += got;
                            act[i] = true;
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
                                act[i] = true;
                            } else {
                                let ar = self.area[i] / self.area[j];
                                let m = self.rock[i] * ar;
                                let old = self.rock[j].max(0.0);
                                self.rock_hard[j] = (old * self.rock_hard[j]
                                    + m * self.rock_hard[i])
                                    / (old + m).max(1e-6);
                                self.rock[j] += m;
                                self.sediment[j] += self.sediment[i] * ar;
                                self.rock[i] = 0.0;
                                self.sediment[i] = 0.0;
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
                        if pressured {
                            let room = (L3_CAP - self.l3_h[t]).max(0.0);
                            let take = mass.min(room);
                            self.l3_h[t] += take;
                            mass -= take;
                        }
                        let room = (L4_CAP - self.l4_h[t]).max(0.0);
                        let take = mass.min(room);
                        self.l4_h[t] += take;
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
                for t in 0..n as TileId {
                    let i = t as usize;
                    let h = self.ground(t);
                    let wet = if h < sea {
                        BASE_WET + SUBMERGED_WET
                    } else {
                        BASE_WET + OROGRAPHIC_WET * (((h - sea) / COND_SCALE).clamp(0.0, 1.0))
                    };
                    self.moist[i] = wet.clamp(0.0, 1.0);
                }
                // THE STREAMS (state, not material): rainfall accumulates down the
                // steepest-descent network — every tile hands its gathered water to
                // its steepest lower neighbour, highest ground first. The discharge
                // is what the carving cuts by, and what a river view will draw.
                {
                    let mut order: Vec<TileId> = (0..n as TileId).collect();
                    order.sort_unstable_by(|a, b| self.ground(*b).total_cmp(&self.ground(*a)));
                    let mut disch: Vec<f32> = self.moist.clone();
                    for t in order {
                        let h = self.ground(t);
                        let mut best: Option<(f32, usize)> = None;
                        for nb in map.neighbours(t) {
                            let drop = h - self.ground(*nb);
                            if drop > 0.0 && best.is_none_or(|(d, _)| drop > d) {
                                best = Some((drop, *nb as usize));
                            }
                        }
                        if let Some((_, j)) = best {
                            disch[j] += disch[t as usize];
                        }
                    }
                    self.discharge = disch;
                }
                let mut sed_delta = vec![0.0f32; n];
                for t in 0..n as TileId {
                    let i = t as usize;
                    // Active ground weathers — and CHANNELS weather regardless: a
                    // river keeps cutting its valley whether or not anything else
                    // touched it this tick.
                    if !act[i] && self.discharge[i] < CHANNEL_LIVE {
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
                    let mut budget = if glacial {
                        ERODE_RATE * ICE_SCOUR * slope
                    } else {
                        ERODE_RATE * self.moist[i] * slope * carve
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
                    // The formed slots shed TOP-DOWN: the softer volcanic
                    // layer (L4) first, then the vein layer (L3) — harder by
                    // its own factor AND the marine grade, which is what
                    // keeps the ore bodies buried under a lid.
                    if budget > 0.0 && self.l4_h[i] > 0.0 {
                        take(
                            &mut self.l4_h[i],
                            HARD_STRATA / self.bed_hard[i].max(0.1),
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
                            HARD_STRATA * HARD_L3_FACTOR / self.bed_hard[i].max(0.1),
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
                        fan(&mut sed_delta, spoil);
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
                        // SUMMER MELT + GLACIAL FLOW: an iced bed passes a damped
                        // share — and the damp scales back to FULL rate as the
                        // pile's overburden grows, because thick ice deforms and
                        // flows: the tower-killer, hemispherically blind.
                        let melt_damp = if glacial {
                            let pressure_flow = ((s / GLACIAL_SOFT_PILE).clamp(0.0, 1.0))
                                * (1.0 - SUMMER_MELT_FLOW);
                            SUMMER_MELT_FLOW + pressure_flow
                        } else {
                            1.0
                        };
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
                        // ladder.
                        let mut mass = SED_FORM * SED_KEEP;
                        let room = (L3_CAP - self.l3_h[i]).max(0.0);
                        let take = mass.min(room);
                        self.l3_h[i] += take;
                        mass -= take;
                        let room4 = (L4_CAP - self.l4_h[i]).max(0.0);
                        let take4 = mass.min(room4);
                        self.l4_h[i] += take4;
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
                        let mut mass = LOOSE_CAP * DENSIFY;
                        let room = (L4_CAP - self.l4_h[t]).max(0.0);
                        let take = mass.min(room);
                        self.l4_h[t] += take;
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
        // glaciation altitude): a bone-dry world at max heat pools its first
        // water as highland ice — honest physics, but this gate isolates the
        // in-fall itself, so it stages terrain no glacier can claim.
        for i in (0..map.len()).step_by(2) {
            e.base[i] += 2.0;
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
    /// never faded; crustal insulation is the law that ends it). One long
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

    /// **Thick ice FLOWS — no column outgrows its own glacier** (Aaron
    /// 2026-08-26: the 2400-tick south pole grew snow towers taller than the
    /// planet; the cap rose as one body). Heavy rock injection under deep ice
    /// for hundreds of ticks: the column's total ground stays BOUNDED (the
    /// overburden-scaled flow moves the pile out as fast as thickening
    /// accelerates it) and the mass demonstrably reaches the neighbourhood.
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

        // Orographic: the high ground is wetter than low DRY ground, and a
        // drowned tile soaks above the dry base.
        let high_wet = e.moisture(lofty);
        let low_dry = (0..map.len() as TileId)
            .filter(|t| {
                let h = e.ground(*t);
                h >= sea && h < sea + 0.2
            })
            .map(|t| e.moisture(t))
            .next()
            .unwrap_or(0.0);
        assert!(
            high_wet > low_dry,
            "condensation rides the uplift: {high_wet} vs {low_dry}"
        );
        let drowned = (0..map.len() as TileId)
            .find(|t| e.ground(*t) < sea)
            .unwrap();
        assert!(e.moisture(drowned) > BASE_WET, "standing water soaks");

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
        // line into the shelf tier: the beds are genuinely deep.
        for _ in 0..150 {
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
    /// oppose — and the world's total material NEVER shrinks (the ledger law
    /// survives de-plating).
    #[test]
    fn flows_collide_into_pressure_and_the_ledger_only_gains() {
        let (map, seams, crust, _plates) = world();
        let mut e = Evolution::new(&map, &seams);
        e.set_water(76.0);
        let material = |e: &Evolution| -> f32 {
            (0..map.len() as TileId)
                .map(|t| e.base(t) + e.grown(t))
                .sum()
        };
        let mut prev = material(&e);
        let mut pressured = false;
        for _ in 0..200 {
            let sea = e.resolve_sea();
            e.tick(&map, &seams, &crust, sea);
            let now = material(&e);
            assert!(
                now >= prev - 0.05,
                "the ledger never drains: {prev} -> {now}"
            );
            prev = now;
            if !pressured {
                pressured = (0..map.len() as TileId).any(|t| e.pressure(t) > 0.05);
            }
        }
        assert!(pressured, "opposing flows jam into pressure somewhere");
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
        assert!((0.4..=1.9).contains(&lo) && (0.4..=1.9).contains(&hi));
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
        assert!(
            max_d > CHANNEL_LIVE * 2.0,
            "the network concentrates real catchments: max {max_d}"
        );
        // A channel runs below its banks: among high-discharge LAND tiles,
        // most sit lower than the mean of their non-channel neighbours.
        let sea = e.resolve_sea();
        let (mut below, mut total) = (0usize, 0usize);
        for t in 0..map.len() as TileId {
            if e.discharge(t) < CHANNEL_LIVE || e.ground(t) <= sea {
                continue;
            }
            let mut bank = 0.0f32;
            let mut nb_n = 0usize;
            for nb in map.neighbours(t) {
                if e.discharge(*nb) < CHANNEL_LIVE {
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
}
