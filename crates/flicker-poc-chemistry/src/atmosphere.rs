//! **The sky tier** — how a planet gets an atmosphere.
//!
//! [`Outgassing`] is the tier's first stage: volatiles leave the hot mantle as
//! **real gas compounds**, element-conserved, so the air is made of the same
//! matter as everything else and the books can prove it. The chemistry is the
//! ported volatile vocabulary of the old epoch pipeline (H→H₂O, C→CO₂, N→N₂,
//! S→SO₂, Cl→HCl) and its tick-sim **distillation series**: each species has a
//! release floor, so *which* gas is leaving depends on how hot the planet still
//! is. A magma ocean exhales everything at once — a steam-and-sulfur burst; as
//! the surface cools past each floor, that species stops, and the residue is
//! what a still-molten world has left to give. The air *distills* as the planet
//! cools. Nothing names a target atmosphere — Venus, Mars and Earth are all
//! reachable exhalations of the same law.
//!
//! A planet breathes three ways, and they are one chemistry ([`GasVocabulary`])
//! driven by three different things: a **magma ocean** venting freely, a frozen
//! lid **seeping** what percolates through it ([`LID_TRICKLE`] — still heat-gated,
//! which is what gives the distillation series its range), and **eruptions**
//! ([`Volcanism`](crate::crust::Volcanism)), where decompression takes over and
//! the floors stop applying entirely. The last is why an old, cool, quiet world
//! can still be topping up its sky.
//!
//! [`WaterCycle`] is the second stage: **where water stands is an equilibrium,
//! not a decision**. A sea on molten ground boils into the air; a cooling air
//! rains its excess back into the sea; and how much stays aloft follows a
//! saturation law in the surface warmth — so the ocean *condenses out of the
//! steam* when the world is ready for it, and nobody schedules the moment.
//!
//! [`CarbonSink`] is the third: **a standing sea eats the carbon hotbox**. Air
//! CO₂ dissolves into the ocean, and where the sea floor has calcium to give,
//! the dissolved carbon precipitates as calcite — a sediment bed in the stratum
//! lifecycle, which is where a planet's limestone comes from and why an old,
//! wet, quiet world trends toward a nitrogen sky.
//!
//! The life books are the tier's next stage and land here as the module grows.
//! The greenhouse read that makes this air *matter* — potent gases holding the
//! star's warmth in — lives with the weather ([`crate::surface`]).

use flicker_materials::{ElementId, Tables};
use flicker_worldstate::CompoundId;

use crate::column::{elevation_m, FormationProcess};
use crate::mantle::MAGMA_OCEAN_K;
use crate::planet::{sea_level_m, World};
use crate::stage::{Stage, StageRng};

/// Catalog ids of the gas species (compounds.json). Asserted against the loaded
/// catalog at [`Outgassing::new`] so a re-numbered table fails loudly, never
/// silently misbooks a gas.
pub const WATER_VAPOUR: CompoundId = 1;
pub const CARBON_DIOXIDE: CompoundId = 2;
pub const NITROGEN: CompoundId = 91;
pub const SULFUR_DIOXIDE: CompoundId = 92;
pub const HYDROGEN_CHLORIDE: CompoundId = 93;
/// Methane — the reducing-branch carbon gas. Volcanism does not make it; the
/// **biosphere** does, when rot has no oxygen to work with
/// ([`crate::biosphere`]).
pub const METHANE: CompoundId = 94;
/// The mineral dissolved carbon precipitates as ([`CarbonSink`]).
pub const CALCITE: CompoundId = 12;

/// Fraction of a hot cell's volatile inventory that degasses per Myr at full
/// heat. Fierce over the few hundred Myr a magma ocean survives — which is when a
/// planet exhales most of the atmosphere it will ever have — and tapering with
/// the warmth after that. e-fold ≈ 500 My at full heat: even a magma era does
/// not empty a mantle in one era ("emission of gasses doesn't happen at once" —
/// Aaron, 2026-08-06; the old 100 My e-fold was a lump-sum shortcut).
pub const DEFAULT_OUTGAS_RATE: f64 = 0.002;

/// Once a cell has frozen over, the lid holds most of its gas in and the rest
/// **percolates** out through the solid rock — diffuse degassing, throttled to
/// this fraction of the open-magma rate.
///
/// This is not the same mechanism as an eruption, and the two do not
/// double-count: percolation is the whole cell seeping slowly and stays
/// floor-gated (which is what lets the distillation series keep working long
/// after the surface has frozen — every release floor sits *below* the solidus),
/// while an eruption is one vent moving melt bodily to the surface and
/// decompressing it. Earth does both. Each removes exactly what it moves, so the
/// ledger cannot notice the difference.
const LID_TRICKLE: f64 = 0.05;

/// The most willing gas's release floor, K — nitrogen's, the lowest in the
/// vocabulary. Below this the interior has nothing left it will give up, which
/// is [`Outgassing`]'s own gate.
/// The number itself now LIVES in processes.json (the gate authority); this
/// constant remains as the drift pin the coupling test compares against.
#[cfg(test)]
pub(crate) const LOWEST_RELEASE_FLOOR_K: f64 = 600.0;

/// How much of an erupted melt's mass can be dissolved volatiles — the magmatic
/// volatile budget, and therefore the hard ceiling on what one eruption can emit.
/// Basaltic magma runs about a percent (H₂O + CO₂ + S together); it is a *small*
/// number, and that it is small is exactly why a planet keeps its volatiles in
/// the rock instead of venting them all into the sky the first time it erupts.
const MAGMATIC_VOLATILE_FRAC: f64 = 0.01;

/// One gas the mantle can exhale: the compound it flies as, the volatile element
/// whose inventory drives it, and the temperature floor below which it stops.
struct GasSpecies {
    compound: CompoundId,
    /// The volatile that wants out. The other constituents (mostly oxygen) are
    /// drawn stoichiometrically from the same cell — the mantle has no shortage
    /// of oxygen, but every kilogram is still booked.
    driver: ElementId,
    /// Mass fraction of the driver within the compound (from the catalog).
    driver_frac: f64,
    /// Release floor, K — the species outgasses only while the cell is hotter.
    floor_k: f64,
    /// Full element mass-fractions of the compound (catalog stoichiometry).
    fracs: Vec<(ElementId, f64)>,
}

/// **What a planet can exhale, and as what** — the gas vocabulary, resolved once
/// from the compound catalog and shared by **both** ways gas leaves the rock:
///
/// - **Bulk degassing** ([`Outgassing`]) — driven by HEAT, so the release floors
///   apply: a species leaves the mantle only while the rock is hot enough to let
///   it go, which is what makes the air distill as the planet cools.
/// - **Eruption venting** ([`Volcanism`](crate::crust::Volcanism)) — driven by
///   DECOMPRESSION. The melt has already separated and risen; at the surface the
///   pressure that held its volatiles in solution is simply gone, so the floors
///   do **not** apply. That is why a cooled planet with volcanoes keeps topping
///   its air up long after bulk degassing has stopped for good.
///
/// One vocabulary, two mechanisms — never two copies of the chemistry.
pub struct GasVocabulary {
    species: Vec<GasSpecies>,
}

impl GasVocabulary {
    /// Resolve the vocabulary from the compound catalog. Panics if a species is
    /// missing or re-numbered — the vocabulary is `sim_required`.
    pub fn load(tables: &Tables) -> Self {
        // (name, catalog id, driver element, release floor K) — floor order is the
        // distillation series: sulfur burns off first as the world cools, nitrogen
        // trickles nearly forever, which is why an old quiet planet trends N₂.
        let vocabulary: [(&str, CompoundId, ElementId, f64); 5] = [
            ("Sulfur Dioxide", SULFUR_DIOXIDE, 16, 3400.0),
            ("Water", WATER_VAPOUR, 1, 2800.0),
            ("Carbon Dioxide", CARBON_DIOXIDE, 6, 2400.0),
            ("Hydrogen Chloride", HYDROGEN_CHLORIDE, 17, 2200.0),
            ("Nitrogen", NITROGEN, 7, 600.0),
        ];
        let species = vocabulary
            .into_iter()
            .map(|(name, id, driver, floor_k)| {
                let def = tables
                    .compound(name)
                    .unwrap_or_else(|| panic!("outgassing needs '{name}' in compounds.json"));
                assert_eq!(def.id, id, "'{name}' moved in the catalog: {} != {id}", def.id);
                let fracs = tables.compound_mass_fractions(def);
                let driver_frac = fracs
                    .iter()
                    .find(|(e, _)| *e == driver)
                    .map(|(_, f)| *f)
                    .unwrap_or_else(|| panic!("'{name}' does not contain element {driver}"));
                GasSpecies { compound: id, driver, driver_frac, floor_k, fracs }
            })
            .collect();
        Self { species }
    }

    /// **Decompression venting** — fly part of `melt` as gas into `air`, leaving
    /// the residue in `melt` for the caller to place as rock. No floors: the melt
    /// is already at the surface.
    ///
    /// Bounded by the **magmatic volatile budget** ([`MAGMATIC_VOLATILE_FRAC`]) —
    /// what a magma can carry dissolved in the first place, which is the physical
    /// limit on what an eruption can possibly emit. Without that bound this would
    /// fly every volatile atom the melt drew up, and a run strips the planet's
    /// entire sulfur inventory into the sky.
    ///
    /// Species are taken in vocabulary order, each bounded by every constituent
    /// the melt can supply, so the shared oxygen is never spent twice and the
    /// air's compound bound holds by construction.
    pub fn vent(&self, air: &mut crate::reservoir::Air, melt: &mut Vec<(ElementId, f64)>) {
        let mut budget: f64 =
            melt.iter().map(|&(_, m)| m).sum::<f64>() * MAGMATIC_VOLATILE_FRAC;
        // **Most volatile first**, which is the reverse of the bulk order: a high
        // release floor means the rock gives that species up only reluctantly —
        // it is the *least* volatile — so on decompression it is the last to come
        // out, not the first. Iterating the other way lets sulfur (the most
        // reluctant) eat the whole budget and starve the water and carbon dioxide
        // that actually dominate a real volcanic plume.
        //
        // Each species draws through the one booking primitive, capped by what is
        // left of the budget; what it takes is what the next species cannot have.
        for sp in self.species.iter().rev() {
            if budget <= 0.0 {
                break;
            }
            budget -= fly_as(air, melt, sp.compound, &sp.fracs, budget);
        }
        melt.retain(|&(_, m)| m > 0.0);
    }
}

/// **Outgassing** — the molten mantle exhales its volatiles as gas compounds.
///
/// Per cell, per species, hottest floor first (so the fiercest gases draw their
/// shared constituents before the residue does): while the cell's mantle stands
/// above the species' floor, a warmth-scaled fraction of the driver element's
/// local inventory leaves as that gas — constituent elements move mantle →
/// atmosphere (conserved), and the compound is recorded in the air's species
/// ledger (bounded by those elements, audited every tick).
///
/// The driver's "local inventory" is what the cell holds **less what its
/// not-yet-sunk metal has dissolved** ([`metal_bound_mass`]): sulfur is both a
/// driver and the core's light element, and while differentiation is under way
/// the metal phase owns its share of it. That subtraction is how core formation
/// and outgassing compete for the same sulfur atoms — the fix for the stripped-
/// mantle SO₂ sky of defect 7E01115B.
///
/// [`metal_bound_mass`]: crate::interior::metal_bound_mass
///
/// A frozen cell still degasses, but only by seeping: below the crust solidus
/// the release is throttled to [`LID_TRICKLE`]. That throttle is what gives the
/// distillation series room to work — every release floor lies below the solidus,
/// so it is *after* the lid closes that the world cools past each one in turn and
/// the air's composition actually changes. What a lid cannot hold back at all is
/// an eruption, which is a separate pathway with its own stage
/// ([`Volcanism`](crate::crust::Volcanism)).
pub struct Outgassing {
    /// Fraction of the driver inventory released per Myr at full heat.
    pub rate: f64,
    gases: GasVocabulary,
}

impl Outgassing {
    /// Resolve the gas vocabulary from the compound catalog.
    pub fn new(tables: &Tables, rate: f64) -> Self {
        Self { rate, gases: GasVocabulary::load(tables) }
    }
}

impl Stage for Outgassing {
    fn name(&self) -> &'static str {
        "Outgassing"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let solidus = crate::crust::SOLIDUS_K;
        for cell in 0..world.mantle.n_cells() {
            let t = world.mantle.temp_k[cell];
            // A solid lid throttles the escape to what percolates through it.
            let lid = if t < solidus { LID_TRICKLE } else { 1.0 };
            for sp in &self.gases.species {
                if t < sp.floor_k {
                    continue;
                }
                // Fierce at magma-ocean heat, tapering to nothing at the floor.
                let warmth = ((t - sp.floor_k) / (MAGMA_OCEAN_K - sp.floor_k).max(1.0))
                    .clamp(0.0, 1.0);
                let release = (self.rate * warmth * lid * dt_myr).min(1.0);
                if release <= 0.0 {
                    continue;
                }
                // How much of the driver is actually THERE for the taking: the
                // share dissolved in the cell's not-yet-sunk metal is spoken for
                // by the core and off the table. This subtraction is the
                // degassing half of the sulfur competition (defect 7E01115B —
                // see [`crate::interior::metal_bound_mass`]): S is the one
                // driver that is also the core's light element, so without it
                // the magma-ocean era strips the mantle's sulfur into a hundred-
                // bar SO₂ sky that nothing can ever rain out. For H/C/N/Cl the
                // term is zero. Then bound by what of every constituent this
                // cell can actually supply.
                let driver_avail = (world.mantle.mass(cell, sp.driver)
                    - crate::interior::metal_bound_mass(world, cell, sp.driver))
                .max(0.0);
                let want = driver_avail * release / sp.driver_frac;
                let gas = sp
                    .fracs
                    .iter()
                    .filter(|(_, f)| *f > 0.0)
                    .fold(want, |g, &(e, f)| g.min(world.mantle.mass(cell, e) / f));
                if gas <= 0.0 {
                    continue;
                }
                // Move the constituents mantle → air, and book the gas they fly as.
                let mut moved = 0.0;
                for &(e, f) in &sp.fracs {
                    let took = world.mantle.remove(cell, e, gas * f);
                    if took > 0.0 {
                        world.reservoirs.atmosphere.contents.add(e, took);
                        moved += took;
                    }
                }
                if moved > 0.0 {
                    world.reservoirs.atmosphere.species.add(sp.compound, moved);
                }
            }
        }
    }
}

/// **These elements fly as this gas.** Books as much of `compound` as `bundle`
/// can actually supply into the air's dual ledger — bounded by every
/// constituent, so a shared element is never spent twice — draining what it
/// takes out of the bundle. Returns the gas mass booked.
///
/// **The ONE place a gas is booked.** Every route into the sky that works from
/// an element bundle goes through here: each species of the volcanic series
/// ([`GasVocabulary::vent`]), the biosphere's respired CO₂, and the CH₄ that
/// anaerobic rot gives off instead ([`crate::biosphere`]). The bounding and
/// draining rule therefore exists once, and the air's compound bound holds by
/// construction for all of them.
///
/// `cap` is the most gas this call may book, which is how a caller with a
/// budget to share out — a magma can only carry so much dissolved volatile —
/// spends it species by species. Pass [`f64::INFINITY`] when the bundle itself
/// is the only limit.
///
/// (Bulk [`Outgassing`] does NOT come through here: it draws straight out of a
/// mantle cell rather than a bundle, and materialising one per cell per species
/// would allocate 92k times a tick to save a few lines.)
pub(crate) fn fly_as(
    air: &mut crate::reservoir::Air,
    bundle: &mut Vec<(ElementId, f64)>,
    compound: CompoundId,
    fracs: &[(ElementId, f64)],
    cap: f64,
) -> f64 {
    let avail = |b: &Vec<(ElementId, f64)>, e: ElementId| {
        b.iter().find(|&&(m, _)| m == e).map_or(0.0, |&(_, m)| m)
    };
    let gas = fracs
        .iter()
        .filter(|(_, f)| *f > 0.0)
        .fold(cap, |g, &(e, f)| g.min(avail(bundle, e) / f));
    if !gas.is_finite() || gas <= 0.0 {
        return 0.0;
    }
    let mut moved = 0.0;
    for &(e, f) in fracs {
        if let Some(slot) = bundle.iter_mut().find(|(m, _)| *m == e) {
            let took = (gas * f).min(slot.1);
            slot.1 -= took;
            if took > 0.0 {
                air.contents.add(e, took);
                moved += took;
            }
        }
    }
    if moved > 0.0 {
        air.species.add(compound, moved);
    }
    moved
}

/// One shell of the classified air — **a derived read of the species ledger,
/// never stored state**. The multiple-layers expression: each booked gas is a
/// shell, and where it sits in the stack follows from what it weighs.
pub struct AirShell {
    /// The gas this shell is (catalog id).
    pub compound: CompoundId,
    /// This gas's column over the surface, kg/m² — how much sky it is.
    pub column_kg_m2: f64,
    /// Formula-unit mass, u — heavier gas hugs the ground, lighter rides higher.
    pub molar_mass: f64,
}

/// Classify the air into stacked shells, heaviest lowest. A CO₂ hotbox is one
/// thick low shell; a temperate world is a broad N₂ band with a water-vapour
/// veil above it; nothing places either — the stack is read off the books.
pub fn air_shells(world: &World, tables: &Tables) -> Vec<AirShell> {
    let area = world.cell_area_m2() * world.columns.len().max(1) as f64;
    let mut shells: Vec<AirShell> = world
        .reservoirs
        .atmosphere
        .species
        .iter()
        .filter(|&(_, kg)| kg > 0.0)
        .filter_map(|(id, kg)| {
            let def = tables.compound_by_id(id)?;
            Some(AirShell {
                compound: id,
                column_kg_m2: kg / area,
                molar_mass: tables.compound_molar_mass(def),
            })
        })
        .collect();
    shells.sort_by(|a, b| b.molar_mass.total_cmp(&a.molar_mass));
    // **Six atmospheric layers is the ruled range** (Aaron, 2026-08-06). The
    // stack keeps the six most substantial skies and folds the trace gases away —
    // a truncation of the READ, never of the books: every species stays in the
    // conserved ledger whether or not it earns a shell.
    if shells.len() > MAX_AIR_SHELLS {
        shells.sort_by(|a, b| b.column_kg_m2.total_cmp(&a.column_kg_m2));
        shells.truncate(MAX_AIR_SHELLS);
        shells.sort_by(|a, b| b.molar_mass.total_cmp(&a.molar_mass));
    }
    shells
}

/// The most atmospheric layers the read ever reports — Aaron's ruled range.
pub const MAX_AIR_SHELLS: usize = 6;

/// Water-vapour column a temperate air holds, kg/m², at [`SATURATION_REF_K`] —
/// the scale of Earth's own (~25 kg/m² over a 288 K mean).
const SATURATION_REF_KG_M2: f64 = 25.0;
/// The reference warmth for [`SATURATION_REF_KG_M2`], K.
const SATURATION_REF_K: f64 = 288.0;
/// E-folding warmth of the saturation curve, K — the Clausius-Clapeyron slope
/// (~7% more vapour per kelvin). This one exponent is what makes a hot world a
/// steam bath and a cold one dry, with Earth's trace in between.
const SATURATION_EFOLD_K: f64 = 14.5;

/// How much water the air can hold aloft at this surface warmth, kg/m².
fn saturation_kg_m2(temp_k: f64) -> f64 {
    SATURATION_REF_KG_M2 * ((temp_k - SATURATION_REF_K) / SATURATION_EFOLD_K).exp()
}

/// Fraction of the sea standing on molten ground that boils off per Myr — a
/// delivered ocean cannot sit on a magma surface, so while the lid is open the
/// water lives in the sky.
const BOIL_RATE: f64 = 1.0;

/// **WaterCycle** — the conserved ocean ↔ atmosphere exchange.
///
/// Three motions, all booked mass, none scheduled:
///
/// - **The boil**: the molten fraction of the surface drives that share of the
///   sea into the air ([`BOIL_RATE`]).
/// - **The rain**: vapour above the saturation target rains into the ocean —
///   but only over the solid fraction of the ground, so a half-molten world
///   churns instead of settling.
/// - **The draw**: an undersaturated air pulls water off the sea up to the
///   target, which is how a warming star dries a world back out.
///
/// The target follows [`saturation_kg_m2`] at the mean radiative surface
/// temperature ([`crate::surface::mean_surface_temp_k`]) — which reads the
/// greenhouse, which reads this very vapour: the water-vapour feedback arrives
/// by construction, not by a rule. At Myr steps the exchange is an equilibrium
/// snap, not a rate race — vapour's real residence time is days.
pub struct WaterCycle {
    /// Multiplier on how hard the star shines — the boundary input; the
    /// celestial host supplies it later, the GM lever stands in until then.
    pub stellar: f64,
    /// The Water compound's element mass-fractions (catalog stoichiometry).
    fracs: Vec<(ElementId, f64)>,
}

impl WaterCycle {
    /// Resolve water's stoichiometry from the catalog. Panics if the catalog
    /// moved — same contract as [`Outgassing::new`].
    pub fn new(tables: &Tables, stellar: f64) -> Self {
        let def = tables.compound("Water").expect("the water cycle needs 'Water' in compounds.json");
        assert_eq!(def.id, WATER_VAPOUR, "'Water' moved in the catalog: {}", def.id);
        Self { stellar, fracs: tables.compound_mass_fractions(def) }
    }

    /// Move up to `kg` of water **sea → sky**, bounded by every constituent the
    /// ocean can actually supply; books the vapour it flies as.
    fn lift(&self, world: &mut World, kg: f64) {
        if kg <= 0.0 {
            return;
        }
        let take = self
            .fracs
            .iter()
            .filter(|(_, f)| *f > 0.0)
            .fold(kg, |k, &(e, f)| k.min(world.reservoirs.ocean.contents.amount(e) / f));
        if take <= 0.0 {
            return;
        }
        let mut moved = 0.0;
        for &(e, f) in &self.fracs {
            let got = world.reservoirs.ocean.contents.remove(e, take * f);
            if got > 0.0 {
                world.reservoirs.atmosphere.contents.add(e, got);
                moved += got;
            }
        }
        if moved > 0.0 {
            world.reservoirs.atmosphere.species.add(WATER_VAPOUR, moved);
        }
    }

    /// Rain up to `kg` of the air's booked steam **sky → sea**. The species
    /// ledger is debited first and the elements follow it, so the compound
    /// bound can only slacken, never break.
    fn rain(&self, world: &mut World, kg: f64) {
        if kg <= 0.0 {
            return;
        }
        let got = world.reservoirs.atmosphere.species.remove(WATER_VAPOUR, kg);
        if got <= 0.0 {
            return;
        }
        for &(e, f) in &self.fracs {
            let took = world.reservoirs.atmosphere.contents.remove(e, got * f);
            if took > 0.0 {
                world.reservoirs.ocean.contents.add(e, took);
            }
        }
    }
}

impl Stage for WaterCycle {
    fn name(&self) -> &'static str {
        "WaterCycle"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let n = world.mantle.n_cells();
        if n == 0 {
            return;
        }
        let solidus = crate::crust::SOLIDUS_K;
        let molten =
            world.mantle.temp_k.iter().filter(|&&t| t >= solidus).count() as f64 / n as f64;

        // The boil: a sea cannot stand on magma.
        let ocean_kg = world.reservoirs.ocean.mass_kg();
        if molten > 0.0 && ocean_kg > 0.0 {
            self.lift(world, ocean_kg * (BOIL_RATE * molten * dt_myr).min(1.0));
        }

        // The equilibrium: how much water this warmth keeps aloft.
        let area = world.cell_area_m2() * world.columns.len().max(1) as f64;
        let target = saturation_kg_m2(crate::surface::mean_surface_temp_k(world, self.stellar))
            * area;
        let vapour = world.reservoirs.atmosphere.species.amount(WATER_VAPOUR);
        if vapour > target {
            // The rain — over solid ground only.
            self.rain(world, (vapour - target) * (1.0 - molten));
        } else {
            // The draw.
            self.lift(world, target - vapour);
        }
    }
}

/// Dissolved-carbon fraction of the ocean per pascal of pCO₂ — Henry's law,
/// calibrated on Earth's own sea (~3.8e16 kg dissolved C in 1.4e21 kg of ocean
/// under a 40 Pa CO₂ sky). The sea holds ~38× the air's carbon at that trace,
/// and this is that ratio, per pascal.
const HENRY_FRAC_PER_PA: f64 = 6.8e-7;
/// Ceiling on the dissolved fraction — even under a hundred-bar sky the sea is
/// seltzer, not acid: solubility saturates.
const MAX_DISSOLVED_FRAC: f64 = 0.01;
/// The dissolved-carbon fraction the sea KEEPS — below this, nothing
/// precipitates. Together with Henry's law this floor pins the equilibrium the
/// whole cycle runs down to: floor/henry ≈ 40 Pa — Earth's trace-CO₂ sky is
/// the fixed point of a wet temperate world, emergent, never a target.
const DIC_FLOOR_FRAC: f64 = 2.7e-5;
/// Fraction of the air↔sea imbalance exchanged per Myr, either direction.
const EXCHANGE_RATE: f64 = 0.1;
/// Fraction of the dissolved excess (above the floor) precipitated per Myr
/// where the floor has calcium to give.
const PRECIPITATION_RATE: f64 = 0.01;
/// How much faster shell-building life lays carbonate down than chemistry
/// alone. Organisms do not wait for super-saturation; they pump against the
/// gradient, which is why an inhabited world's carbonate beds dwarf a sterile
/// one's.
const BIOGENIC_GAIN: f64 = 20.0;
/// How far life pulls the kept-dissolved floor down — the stock a sterile sea
/// would have held onto is available to a living one.
const BIOGENIC_FLOOR_RELIEF: f64 = 0.2;

/// **CarbonSink** — the ocean holds carbon in equilibrium with the sky, and the
/// floor turns the excess to stone.
///
/// Two motions, both gated on a standing sea:
///
/// - **The partition**: dissolved carbon tracks a Henry's-law target in the
///   air's pCO₂ (ceilinged — a hundred-bar sky makes seltzer, not acid).
///   Under-saturated, the sea drinks; over-saturated, it burps CO₂ back.
///   Species debited first, elements following, both directions.
/// - **The stone**: only the dissolved carbon above the floor the sea keeps
///   ([`DIC_FLOOR_FRAC`]) precipitates as **calcite**, onto submerged columns
///   whose top bed has *free* calcium (free = elements less what the bed's
///   minerals already lock — the per-layer compound bound is respected by
///   construction, never repaired after). The precipitate arrives as a
///   [`FormationProcess::Sediment`] bed with the mineral booked at catalog
///   stoichiometry: calcium from the floor, carbon and oxygen from the sea.
///
/// Floor over Henry pins where the pair settles: a wet temperate world runs its
/// sky down to ~40 Pa of CO₂ — Earth's trace — as the *fixed point* of two real
/// solubility constants, never as a target.
///
/// This is the abiotic half of why an old wet world trends toward a nitrogen
/// sky; the life books (organic burial, O₂) are a later stage. The calcite is
/// deliberately **not** left to [`Crystallization`](crate::crust::Crystallization)
/// — carbonate precipitates *as* the mineral, and teaching the global
/// crystalliser a carbonate recipe would let it compete for calcium in every
/// igneous bed on the planet.
pub struct CarbonSink {
    /// CO₂'s element mass-fractions (catalog stoichiometry).
    co2_fracs: Vec<(ElementId, f64)>,
    /// Calcite's element mass-fractions (catalog stoichiometry).
    calcite_fracs: Vec<(ElementId, f64)>,
    /// Calcium's element id, resolved from the catalog.
    ca: ElementId,
    /// Carbon's element id, resolved from the catalog.
    c: ElementId,
    /// Every compound's calcium fraction — what a bed's minerals already lock.
    ca_locked: std::collections::BTreeMap<CompoundId, f64>,
}

impl CarbonSink {
    /// Resolve the sink's chemistry from the catalog. Panics if the vocabulary
    /// moved — same contract as [`Outgassing::new`].
    pub fn new(tables: &Tables) -> Self {
        let co2 = tables.compound("Carbon Dioxide").expect("the sink needs 'Carbon Dioxide'");
        assert_eq!(co2.id, CARBON_DIOXIDE, "'Carbon Dioxide' moved: {}", co2.id);
        let calcite = tables.compound("Calcite").expect("the sink needs 'Calcite'");
        assert_eq!(calcite.id, CALCITE, "'Calcite' moved: {}", calcite.id);
        let ca = tables.element("Ca").expect("calcium in the periodic table").number;
        let c = tables.element("C").expect("carbon in the periodic table").number;
        let ca_locked = tables
            .compounds()
            .iter()
            .map(|c| {
                let f = tables
                    .compound_mass_fractions(c)
                    .into_iter()
                    .find(|&(e, _)| e == ca)
                    .map(|(_, f)| f)
                    .unwrap_or(0.0);
                (c.id, f)
            })
            .collect();
        Self {
            co2_fracs: tables.compound_mass_fractions(co2),
            calcite_fracs: tables.compound_mass_fractions(calcite),
            ca,
            c,
            ca_locked,
        }
    }
}

impl Stage for CarbonSink {
    fn name(&self) -> &'static str {
        "CarbonSink"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        if world.reservoirs.ocean.mass_kg() <= 0.0 {
            return;
        }

        // The drink — and the burp. The sea holds dissolved carbon in PARTITION
        // with the sky (Henry's law, ceilinged): above the target it exhales CO₂
        // back, below it it drinks. Two-way, like the water, so the pair can run
        // down to their joint equilibrium instead of past it.
        let ocean_kg = world.reservoirs.ocean.mass_kg();
        let f_c_in_co2 =
            self.co2_fracs.iter().find(|&&(e, _)| e == self.c).map_or(0.0, |&(_, f)| f);
        let area_total = world.cell_area_m2() * world.columns.len().max(1) as f64;
        let p_co2 = world.reservoirs.atmosphere.species.amount(CARBON_DIOXIDE)
            * world.gravity_m_s2()
            / area_total;
        let target_c = ocean_kg * (HENRY_FRAC_PER_PA * p_co2).min(MAX_DISSOLVED_FRAC);
        let dic = world.reservoirs.ocean.contents.amount(self.c);
        let step = (EXCHANGE_RATE * dt_myr).min(1.0);
        if dic < target_c && f_c_in_co2 > 0.0 {
            // Drink: move CO₂ air → sea toward the partition target.
            let want_gas = (target_c - dic) * step / f_c_in_co2;
            let got = world.reservoirs.atmosphere.species.remove(CARBON_DIOXIDE, want_gas);
            for &(e, f) in &self.co2_fracs {
                let took = world.reservoirs.atmosphere.contents.remove(e, got * f);
                if took > 0.0 {
                    world.reservoirs.ocean.contents.add(e, took);
                }
            }
        } else if dic > target_c && f_c_in_co2 > 0.0 {
            // Burp: the over-saturated sea exhales CO₂ back into the sky.
            let mut gas = (dic - target_c) * step / f_c_in_co2;
            for &(e, f) in &self.co2_fracs {
                if f > 0.0 {
                    gas = gas.min(world.reservoirs.ocean.contents.amount(e) / f);
                }
            }
            if gas > 0.0 {
                let mut moved = 0.0;
                for &(e, f) in &self.co2_fracs {
                    let took = world.reservoirs.ocean.contents.remove(e, gas * f);
                    if took > 0.0 {
                        world.reservoirs.atmosphere.contents.add(e, took);
                        moved += took;
                    }
                }
                if moved > 0.0 {
                    world.reservoirs.atmosphere.species.add(CARBON_DIOXIDE, moved);
                }
            }
        }

        // **The stone.** A sterile sea precipitates only what it is
        // super-saturated in — the dissolved carbon above the floor it keeps.
        //
        // A living one does not wait to be super-saturated. Shell-builders pump
        // carbonate against the gradient, and when they die their shells rain
        // down: precipitation runs far faster and reaches far further down the
        // dissolved stock. That is the difference between a thin chemical crust
        // and the **chalk beds** a world with plankton in it lays down — and it
        // is one term here, not a second carbonate pathway.
        let living = world.life >= crate::biosphere::LifeStage::Microbial;
        let (floor_frac, rate) = if living {
            (DIC_FLOOR_FRAC * BIOGENIC_FLOOR_RELIEF, PRECIPITATION_RATE * BIOGENIC_GAIN)
        } else {
            (DIC_FLOOR_FRAC, PRECIPITATION_RATE)
        };
        let carbon = self.c;
        let dissolved = world.reservoirs.ocean.contents.amount(carbon) - ocean_kg * floor_frac;
        if dissolved <= 0.0 {
            return;
        }
        let area = world.cell_area_m2();
        let sea = sea_level_m(world);
        let submerged: Vec<usize> = (0..world.columns.len())
            .filter(|&i| {
                !world.columns[i].layers.is_empty() && elevation_m(&world.columns[i], area) < sea
            })
            .collect();
        if submerged.is_empty() {
            return;
        }
        let f_ca = self.calcite_fracs.iter().find(|&&(e, _)| e == self.ca).map_or(0.0, |&(_, f)| f);
        let f_c = self.calcite_fracs.iter().find(|&&(e, _)| e == carbon).map_or(0.0, |&(_, f)| f);
        if f_ca <= 0.0 || f_c <= 0.0 {
            return;
        }
        let share = dissolved * (rate * dt_myr).min(1.0) / submerged.len() as f64;
        for i in submerged {
            // Free calcium: this bed's elements, less what its minerals lock.
            let top = world.columns[i].layers.last().expect("filtered non-empty");
            let locked: f64 = top
                .minerals
                .iter()
                .map(|(id, m)| m * self.ca_locked.get(&id).copied().unwrap_or(0.0))
                .sum();
            let free_ca = (top.elements.amount(self.ca) - locked).max(0.0);

            // The calcite this column can lay down: limited by its share of the
            // dissolved carbon, the floor's free calcium, and the sea's stock of
            // every other constituent.
            let mut make = (share / f_c).min(free_ca / f_ca);
            for &(e, f) in &self.calcite_fracs {
                if e != self.ca && f > 0.0 {
                    make = make.min(world.reservoirs.ocean.contents.amount(e) / f);
                }
            }
            if make <= 0.0 {
                continue;
            }

            // Move the matter: calcium off the floor, the rest out of the sea.
            let mut deposited: Vec<(ElementId, f64)> = Vec::new();
            let mut stoich_bound = f64::MAX;
            for &(e, f) in &self.calcite_fracs {
                if f <= 0.0 {
                    continue;
                }
                let got = if e == self.ca {
                    world.columns[i].layers.last_mut().expect("still there").elements.remove(e, make * f)
                } else {
                    world.reservoirs.ocean.contents.remove(e, make * f)
                };
                if got > 0.0 {
                    deposited.push((e, got));
                }
                stoich_bound = stoich_bound.min(got / f);
            }
            if deposited.is_empty() {
                continue;
            }
            let at = world.tick_myr;
            world.columns[i].deposit(FormationProcess::Sediment, at, &deposited);
            // Book the mineral on the bed the deposit landed in, at the exact
            // stoichiometric mass the moved elements can cover — the compound
            // bound holds by construction.
            let booked = stoich_bound.max(0.0).min(deposited.iter().map(|&(_, m)| m).sum::<f64>());
            if booked > 0.0 {
                if let Some(bed) = world.columns[i].layers.last_mut() {
                    bed.minerals.add(CALCITE, booked);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::stage::StageRng;
    use flicker_materials::JsonTableSource;
    use flicker_worldgrid::icosphere;

    fn tables() -> Tables {
        Tables::from_source(&JsonTableSource::new(&content_data_dir())).expect("tables")
    }

    fn world(freq: u32, seed: u64) -> (World, Tables) {
        let t = tables();
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        (World::seed(icosphere(freq), b, &t, seed), t)
    }

    fn run(stage: &Outgassing, w: &mut World, ticks: usize) {
        let mut rng = StageRng::new(7);
        for _ in 0..ticks {
            stage.tick(w, 1.0, &mut rng);
            w.tick_myr += 1.0;
            w.audit("Outgassing");
            w.audit_compound_bound("Outgassing");
        }
    }

    #[test]
    fn the_sea_boils_on_a_molten_world() {
        // Deliver an ocean onto a magma-hot seed and let the cycle run: the
        // water refuses to stand — it lives in the sky as booked steam.
        let (mut w, t) = world(4, 21);
        let mut delivery = crate::infall::WaterDelivery::new(&t);
        delivery.budget_kg = 1.0e21;
        delivery.rate = 0.05;
        let cycle = WaterCycle::new(&t, 1.0);
        let mut rng = StageRng::new(3);
        for _ in 0..20 {
            delivery.tick(&mut w, 1.0, &mut rng);
            cycle.tick(&mut w, 1.0, &mut rng);
            w.tick_myr += 1.0;
            w.audit("WaterCycle");
            w.audit_compound_bound("WaterCycle");
        }
        let vapour = w.reservoirs.atmosphere.species.amount(WATER_VAPOUR);
        let ocean = w.reservoirs.ocean.mass_kg();
        assert!(vapour > 0.0, "the delivered water went aloft");
        assert!(
            ocean < vapour * 0.05,
            "no sea stands on magma: ocean {ocean:.3e} vs vapour {vapour:.3e}"
        );
    }

    /// The gate reads work-that-is-possible, not water-that-exists: a steam sky
    /// over a fully molten world has nothing to exchange (no sea to boil or
    /// draw from, no ground for rain to land on), so the cycle must NOT open —
    /// the tick-1 "WaterCycle OPENED" card on a ball of lava was this gate
    /// reading mere presence. It opens when the first lid gives rain somewhere
    /// to land, which is the transition actually worth announcing.
    #[test]
    fn the_water_cycle_waits_for_ground_to_rain_on() {
        let (mut w, t) = world(4, 23);
        let cycle = WaterCycle::new(&t, 1.0);
        let steam = 1.0e20;
        for &(e, f) in &cycle.fracs {
            w.reservoirs.atmosphere.contents.add(e, steam * f);
            w.reservoirs.delivered.add(e, steam * f);
        }
        w.reservoirs.atmosphere.species.add(WATER_VAPOUR, steam);

        let state = crate::planet::PlanetState::sample(&w);
        assert!(state.water_vapour_kg > 0.0 && state.lid_frac == 0.0 && state.ocean_mass_kg == 0.0);
        assert!(!crate::process_file::gate_of("WaterCycle").holds(&state, &crate::Levers::default()), "steam over magma: nothing to exchange yet");

        crate::planet::freeze_lid(&mut w);
        let state = crate::planet::PlanetState::sample(&w);
        assert!(state.lid_frac > 0.0, "the lid closed");
        assert!(crate::process_file::gate_of("WaterCycle").holds(&state, &crate::Levers::default()), "ground to rain on: the cycle opens");
    }

    /// "No sea, no sink" — and with arrival routed by what the ground can hold,
    /// a magma world genuinely has no sea, so the sink cannot announce "a sea
    /// stands" on lava the way the tick-1 card did.
    #[test]
    fn the_sink_waits_for_a_standing_sea() {
        let (mut w, t) = world(4, 24);
        let delivery = crate::infall::WaterDelivery::new(&t);
        let mut rng = StageRng::new(5);
        delivery.tick(&mut w, 1.0, &mut rng);
        let state = crate::planet::PlanetState::sample(&w);
        assert_eq!(state.ocean_mass_kg, 0.0, "infall on magma leaves no standing sea");
        assert!(!crate::process_file::gate_of("CarbonSink").holds(&state, &crate::Levers::default()), "so the sink stays shut");
    }

    #[test]
    fn the_ocean_condenses_out_of_the_steam_as_the_world_cools() {
        // The same steam bath over a world whose lid has closed: the excess
        // rains out, the ocean appears, and a saturation trace stays aloft.
        let (mut w, t) = world(4, 22);
        let cycle = WaterCycle::new(&t, 1.0);
        // Seed the steam at CATALOG stoichiometry — the air's species ledger is
        // bounded against real H₂O fractions, not the 1:8 shorthand.
        let steam = 1.0e21;
        for &(e, f) in &cycle.fracs {
            w.reservoirs.atmosphere.contents.add(e, steam * f);
            w.reservoirs.delivered.add(e, steam * f);
        }
        w.reservoirs.atmosphere.species.add(WATER_VAPOUR, steam);
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 400.0;
        }
        // A cooled mantle is not a cooled SURFACE: bare ground radiates the
        // interior, so a world only has a temperate surface once it has a lid.
        crate::planet::freeze_lid(&mut w);
        let mut rng = StageRng::new(4);
        cycle.tick(&mut w, 1.0, &mut rng);
        w.audit("WaterCycle");
        w.audit_compound_bound("WaterCycle");

        let vapour = w.reservoirs.atmosphere.species.amount(WATER_VAPOUR);
        let ocean = w.reservoirs.ocean.mass_kg();
        assert!(ocean > steam * 0.9, "the steam rained out into a sea: {ocean:.3e}");
        assert!(vapour > 0.0, "a saturation trace stays aloft");
        assert!(vapour < steam * 0.01, "and it is a trace: {vapour:.3e}");
    }

    #[test]
    fn a_brighter_star_carries_more_water_aloft() {
        // Two identical cool worlds with the same sea, different stars: the one
        // under the brighter star holds more vapour — the star drives
        // evaporation, exactly as the boundary-input contract says.
        let vapour_under = |stellar: f64| {
            let (mut w, t) = world(4, 23);
            for c in 0..w.mantle.n_cells() {
                w.mantle.temp_k[c] = 400.0;
            }
            crate::planet::freeze_lid(&mut w);
            let sea = 1.4e21;
            let h = sea / 9.0;
            w.reservoirs.ocean.contents.add(1, h);
            w.reservoirs.ocean.contents.add(8, sea - h);
            w.reservoirs.delivered.add(1, h);
            w.reservoirs.delivered.add(8, sea - h);
            let cycle = WaterCycle::new(&t, stellar);
            let mut rng = StageRng::new(5);
            for _ in 0..3 {
                cycle.tick(&mut w, 1.0, &mut rng);
                w.audit("WaterCycle");
                w.audit_compound_bound("WaterCycle");
            }
            w.reservoirs.atmosphere.species.amount(WATER_VAPOUR)
        };
        let dim = vapour_under(1.0);
        let bright = vapour_under(1.4);
        assert!(dim > 0.0, "a temperate sea keeps a vapour trace aloft");
        assert!(bright > dim, "the brighter star lifts more: {bright:.3e} vs {dim:.3e}");
    }

    #[test]
    fn the_sea_drinks_the_carbon_and_lays_it_down_as_stone() {
        // A cool wet world under a CO₂ sky, with one calcium-bearing bed on the
        // sea floor: the air's carbon ends up as a calcite sediment bed.
        let (mut w, t) = world(4, 31);
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 400.0;
        }
        // A cooled mantle is not a cooled SURFACE: bare ground radiates the
        // interior, so a world only has a temperate surface once it has a lid.
        crate::planet::freeze_lid(&mut w);
        let sink = CarbonSink::new(&t);

        // A sea (ocean is element truth; 1:8 shorthand is fine on this side).
        let sea = 1.4e21;
        w.reservoirs.ocean.contents.add(1, sea / 9.0);
        w.reservoirs.ocean.contents.add(8, sea * 8.0 / 9.0);
        w.reservoirs.delivered.add(1, sea / 9.0);
        w.reservoirs.delivered.add(8, sea * 8.0 / 9.0);

        // A CO₂ sky, booked at catalog stoichiometry.
        let co2 = 1.0e19;
        for &(e, f) in &sink.co2_fracs {
            w.reservoirs.atmosphere.contents.add(e, co2 * f);
            w.reservoirs.delivered.add(e, co2 * f);
        }
        w.reservoirs.atmosphere.species.add(CARBON_DIOXIDE, co2);

        // One thin calcium-bearing bed on the floor (submerged: metres of rock
        // under kilometres of sea).
        let ca = sink.ca;
        let mut elements = flicker_worldstate::Composition::new();
        elements.add(ca, 2.0e18);
        elements.add(14, 3.0e18);
        elements.add(8, 4.0e18);
        w.reservoirs.delivered.add(ca, 2.0e18);
        w.reservoirs.delivered.add(14, 3.0e18);
        w.reservoirs.delivered.add(8, 4.0e18);
        w.columns[0].layers.push(crate::column::Layer {
            elements,
            minerals: flicker_worldstate::CompoundLedger::new(),
            formed_at_myr: 0.0,
            formed_by: FormationProcess::OceanicCrust,
            peak_pt: (0.0, 0.0),
            cooled: 0.0,
            eclogitised: 0.0,
        });

        let sky_before = w.reservoirs.atmosphere.species.amount(CARBON_DIOXIDE);
        let mut rng = StageRng::new(6);
        for _ in 0..10 {
            sink.tick(&mut w, 1.0, &mut rng);
            w.tick_myr += 1.0;
            w.audit("CarbonSink");
            w.audit_compound_bound("CarbonSink");
        }

        assert!(
            w.reservoirs.atmosphere.species.amount(CARBON_DIOXIDE) < sky_before,
            "the sea drank from the sky"
        );
        assert!(w.reservoirs.ocean.contents.amount(sink.c) > 0.0, "carbon is dissolved in it");
        let col = &w.columns[0];
        let calcite: f64 = col.layers.iter().map(|l| l.minerals.amount(CALCITE)).sum();
        assert!(calcite > 0.0, "and the floor turned some of it to stone");
        assert!(
            col.layers.iter().any(|l| l.formed_by == FormationProcess::Sediment),
            "as a sediment bed in the stratum lifecycle"
        );
    }

    #[test]
    fn a_dry_world_banks_no_carbon() {
        // The same CO₂ sky over a world with no sea: the sink has nothing to
        // drink with, and the air keeps its carbon.
        let (mut w, t) = world(4, 32);
        let sink = CarbonSink::new(&t);
        let co2 = 1.0e19;
        for &(e, f) in &sink.co2_fracs {
            w.reservoirs.atmosphere.contents.add(e, co2 * f);
            w.reservoirs.delivered.add(e, co2 * f);
        }
        w.reservoirs.atmosphere.species.add(CARBON_DIOXIDE, co2);
        assert!(!crate::process_file::gate_of("CarbonSink").holds(&crate::planet::PlanetState::sample(&w), &crate::Levers::default()), "no sea, no sink");
        let mut rng = StageRng::new(7);
        sink.tick(&mut w, 1.0, &mut rng);
        w.audit("CarbonSink");
        assert_eq!(
            w.reservoirs.atmosphere.species.amount(CARBON_DIOXIDE),
            co2,
            "the dry world's sky is untouched"
        );
    }

    #[test]
    fn the_air_reads_as_shells_heaviest_lowest() {
        // Book three gases and read the stack: CO₂ (44 u) below N₂ (28 u) below
        // water vapour (18 u) — order follows weight, nothing places it.
        let (mut w, t) = world(4, 41);
        let book = |w: &mut World, id: CompoundId, name: &str, kg: f64| {
            let def = t.compound(name).expect("in catalog");
            for (e, f) in t.compound_mass_fractions(def) {
                w.reservoirs.atmosphere.contents.add(e, kg * f);
                w.reservoirs.delivered.add(e, kg * f);
            }
            w.reservoirs.atmosphere.species.add(id, kg);
        };
        book(&mut w, CARBON_DIOXIDE, "Carbon Dioxide", 3.0e18);
        book(&mut w, NITROGEN, "Nitrogen", 2.0e18);
        book(&mut w, WATER_VAPOUR, "Water", 1.0e18);
        w.audit_compound_bound("shells");

        let shells = air_shells(&w, &t);
        let order: Vec<CompoundId> = shells.iter().map(|s| s.compound).collect();
        assert_eq!(order, vec![CARBON_DIOXIDE, NITROGEN, WATER_VAPOUR], "heaviest lowest");
        assert!(shells.iter().all(|s| s.column_kg_m2 > 0.0), "every shell has column mass");
        assert!(
            shells.windows(2).all(|p| p[0].molar_mass > p[1].molar_mass),
            "strictly ordered by weight"
        );
    }

    #[test]
    fn a_magma_ocean_outgasses_and_the_books_hold() {
        let (mut w, t) = world(4, 11);
        // Seeded at magma-ocean heat: everything above its floor exhales at once.
        let stage = Outgassing::new(&t, DEFAULT_OUTGAS_RATE);
        run(&stage, &mut w, 10);
        let air = &w.reservoirs.atmosphere;
        assert!(air.mass_kg() > 0.0, "a magma ocean exhales");
        for id in [SULFUR_DIOXIDE, WATER_VAPOUR, CARBON_DIOXIDE, NITROGEN] {
            assert!(air.species.amount(id) > 0.0, "species {id} in the burst");
        }
        // The audit ran every tick above — conservation and the compound bound held.
    }

    #[test]
    fn the_air_distills_as_the_planet_cools() {
        let (mut w, t) = world(4, 12);
        let stage = Outgassing::new(&t, DEFAULT_OUTGAS_RATE);

        // Cooled below the sulfur floor: SO₂ stops, CO₂ and N₂ keep coming.
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 2500.0;
        }
        let so2_before = w.reservoirs.atmosphere.species.amount(SULFUR_DIOXIDE);
        run(&stage, &mut w, 5);
        let air = &w.reservoirs.atmosphere;
        assert_eq!(air.species.amount(SULFUR_DIOXIDE), so2_before, "SO₂ floor passed");
        assert!(air.species.amount(CARBON_DIOXIDE) > 0.0, "CO₂ still exhaling at 2500 K");
        assert!(air.species.amount(NITROGEN) > 0.0, "N₂ still exhaling at 2500 K");

        // Cooled below everything but nitrogen: the residue is N₂ alone.
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 800.0;
        }
        let co2_mid = air.species.amount(CARBON_DIOXIDE);
        let n2_mid = air.species.amount(NITROGEN);
        run(&stage, &mut w, 5);
        let air = &w.reservoirs.atmosphere;
        assert_eq!(air.species.amount(CARBON_DIOXIDE), co2_mid, "CO₂ floor passed");
        assert!(air.species.amount(NITROGEN) > n2_mid, "N₂ trickles nearly forever");
    }

    #[test]
    fn a_cold_world_exhales_nothing() {
        let (mut w, t) = world(4, 13);
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 300.0;
        }
        let stage = Outgassing::new(&t, DEFAULT_OUTGAS_RATE);
        run(&stage, &mut w, 5);
        assert_eq!(w.reservoirs.atmosphere.mass_kg(), 0.0, "below every floor");
    }

    /// **The lid throttles, it does not stop.** Frozen ground still seeps — and
    /// it must, because every release floor lies below the solidus, so it is
    /// after the lid closes that the world cools past them in turn and the air
    /// actually distills. What the lid *does* stop is the flood.
    ///
    /// (The third pathway, focused eruption, is
    /// [`Volcanism`](crate::crust::Volcanism)'s — it needs no seepage and no
    /// floor at all.)
    #[test]
    fn the_solid_lid_throttles_release() {
        let solidus = crate::crust::SOLIDUS_K;
        let (mut open, t) = world(4, 14);
        for c in 0..open.mantle.n_cells() {
            open.mantle.temp_k[c] = solidus + 25.0;
        }
        let stage = Outgassing::new(&t, DEFAULT_OUTGAS_RATE);
        run(&stage, &mut open, 3);

        let (mut lidded, t2) = world(4, 14);
        for c in 0..lidded.mantle.n_cells() {
            lidded.mantle.temp_k[c] = solidus - 25.0;
        }
        let stage2 = Outgassing::new(&t2, DEFAULT_OUTGAS_RATE);
        run(&stage2, &mut lidded, 3);

        let open_kg = open.reservoirs.atmosphere.mass_kg();
        let lid_kg = lidded.reservoirs.atmosphere.mass_kg();
        assert!(open_kg > 0.0 && lid_kg > 0.0, "both exhale ({open_kg}, {lid_kg})");
        assert!(
            lid_kg < open_kg * 0.2,
            "the lid throttles: {lid_kg:.3e} vs open {open_kg:.3e}"
        );
    }

    #[test]
    fn warmth_follows_the_potent_gases_not_the_bulk() {
        // Two airs of identical mass: transparent N₂ holds nothing in, CO₂ does.
        let (mut n2_world, _t) = world(4, 15);
        n2_world.reservoirs.atmosphere.contents.add(7, 1.0e18);
        n2_world.reservoirs.atmosphere.species.add(NITROGEN, 1.0e18);

        let (mut co2_world, _t) = world(4, 15);
        co2_world.reservoirs.atmosphere.contents.add(6, 0.273e18);
        co2_world.reservoirs.atmosphere.contents.add(8, 0.727e18);
        co2_world.reservoirs.atmosphere.species.add(CARBON_DIOXIDE, 1.0e18);

        let cold = crate::surface::greenhouse_k(&n2_world);
        let warm = crate::surface::greenhouse_k(&co2_world);
        assert!(cold < 1e-9, "nitrogen is transparent: {cold}");
        assert!(warm > 1.0, "carbon dioxide holds warmth in: {warm}");
    }

    #[test]
    #[should_panic(expected = "compound bound broken")]
    fn a_gas_without_its_elements_fails_the_bound() {
        let (mut w, _t) = world(4, 16);
        // A booked gas whose constituent elements never entered the air.
        w.reservoirs.atmosphere.species.add(CARBON_DIOXIDE, 1.0e18);
        w.audit_compound_bound("corrupt-air");
    }
}
