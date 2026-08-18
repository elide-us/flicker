//! **The life books** — what a biosphere does to the rock and the air.
//!
//! Per Ruling R8, life itself is **never simulated**. There is no organism here,
//! no population, no food web, and nothing that could be called a creature. What
//! is simulated is the *ledger* a biosphere keeps: carbon it pulls out of the
//! sky, oxygen it puts back, and how much of its own dead the ground keeps
//! instead of handing over. Life is an accounting entity that moves conserved
//! mass between reservoirs, exactly like every other stage.
//!
//! # Why this is where coal and oil come from
//!
//! Photosynthesis takes CO₂ out of the air and water out of the sea and makes
//! solid carbon out of them, releasing the leftover oxygen. Every kilogram of
//! that carbon that ends up **buried instead of eaten** is a kilogram the air
//! never gets back — which is simultaneously why a planet accumulates free
//! oxygen and why it has fossil fuel. They are the same ledger entry read from
//! two sides, and neither one is scheduled.
//!
//! What decides the split is **who is alive to eat it**, and that is chemistry
//! rather than a date:
//!
//! - Marine microbes make **cellulose**, which anything can digest.
//! - Megaflora ([`LifeStage::Floral`]) is the first tissue to contain
//!   **lignin** — the molecule that makes wood stiff, and that almost nothing
//!   could break down when it first appeared. So while there are forests and no
//!   decomposer able to touch lignin, dead wood does not rot. It piles up, gets
//!   buried, and becomes coal.
//! - The **decomposer guild** ([`LifeStage::Decomposers`] — the termites, in
//!   Prism's canon) is the stage at which lignin becomes food. From that moment
//!   the burial window is shut, and a world's entire hydrocarbon endowment is
//!   whatever it managed to bury first.
//!
//! The guild is not scheduled either: it arrives when there is **enough buried
//! lignin to be worth evolving to eat** ([`Levers::decomposer_niche_kg`]). A
//! world that buries carbon fast breeds its own decomposers sooner and ends up
//! with *less* coal — the window closes itself.
//!
//! # And why the sky changes colour
//!
//! Rot needs an oxidant. In an O₂-poor air the only rot available is anaerobic,
//! and anaerobic rot gives off **methane** — which the greenhouse read already
//! prices at twelve times CO₂. So a young living world runs hot under a methane
//! sky. As buried carbon lifts the oxygen, rot switches aerobic, the methane
//! stops, and the greenhouse it was holding up falls out from under the planet.
//! Nothing schedules that either; it is two rot pathways and a threshold.
//!
//! [`Levers::decomposer_niche_kg`]: crate::Levers::decomposer_niche_kg

use flicker_materials::{ElementId, Tables};
use flicker_worldstate::CompoundId;

use crate::atmosphere::{fly_as, CARBON_DIOXIDE, METHANE};
use crate::column::{elevation_m, FormationProcess};
use crate::planet::{sea_level_m, PlanetState, World};
use crate::stage::{Stage, StageRng};

/// Catalog ids of the organic compounds life builds and burial cooks.
/// Asserted against the loaded catalog so a re-numbered table fails loudly.
pub const CELLULOSE: CompoundId = 54;
pub const LIGNIN: CompoundId = 53;
pub const OILS: CompoundId = 61;
pub const COAL: CompoundId = 25;
/// Free molecular oxygen — a **biosignature**, not a primordial gas: nothing
/// else in this simulation makes it.
pub const OXYGEN: CompoundId = 96;

/// How much of the world must have frozen over before life is possible at all.
/// A DETECTION threshold, not a physical constant — the same kind of choice as
/// the habitability bands: "mostly solid ground", against a world that is still
/// substantially an ocean of lava.
/// The number itself now LIVES in processes.json (the gate authority); this
/// constant remains as the drift pin the coupling test compares against.
#[cfg(test)]
pub(crate) const LID_FOR_LIFE: f64 = 0.5;
/// And how much of that ground must stand under water. Small on purpose — a
/// tide pool is enough to start in; this exists to reject the single delivered
/// molecule, not to demand an Earth.
/// The number itself now LIVES in processes.json (the gate authority); this
/// constant remains as the drift pin the coupling test compares against.
#[cfg(test)]
pub(crate) const SEA_FOR_LIFE: f64 = 0.01;

/// Hydrogen and carbon, the two elements the whole organic ledger turns on.
const H: ElementId = 1;
const C: ElementId = 6;
const O: ElementId = 8;

/// How far life has got. **Advance-only** — a stage once reached is never given
/// back, so a biosphere that has learned to rot lignin cannot unlearn it and a
/// planet cannot re-open its coal window by getting cold for a while.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum LifeStage {
    /// Nothing alive. Every world starts here.
    #[default]
    Barren,
    /// Microbial mats in the sea: photosynthesis starts, and the first carbon
    /// leaves the air.
    Microbial,
    /// **Megaflora** on land — the first woody tissue, which is the first
    /// lignin, which is the beginning of the coal window.
    Floral,
    /// The **decomposer guild** (canon: termites). Lignin is food now, and the
    /// coal window is shut for good.
    Decomposers,
}

impl LifeStage {
    /// The next stage up, if there is one.
    fn next(self) -> Option<Self> {
        match self {
            Self::Barren => Some(Self::Microbial),
            Self::Microbial => Some(Self::Floral),
            Self::Floral => Some(Self::Decomposers),
            Self::Decomposers => None,
        }
    }

    /// A stringtable token naming this stage — the observer hands over token
    /// KEYS; resolving them is the scene's business (model-channel strings gate).
    pub fn token(self) -> &'static str {
        match self {
            Self::Barren => "$chem_life_barren",
            Self::Microbial => "$chem_life_microbial",
            Self::Floral => "$chem_life_floral",
            Self::Decomposers => "$chem_life_decomposers",
        }
    }
}

/// Coldest mean surface temperature life will start at, K — below this the
/// water it needs is ice.
const LIFE_COLD_K: f64 = 260.0;
/// Warmest, K — above this the sea is on its way to being sky.
const LIFE_HOT_K: f64 = 340.0;
/// Free-oxygen mass fraction of the air at which land life becomes possible —
/// the ozone-shield threshold, in the only currency this model has.
const LAND_LIFE_O2_FRAC: f64 = 0.02;
/// Fraction of the air's booked CO₂ that primary production fixes per Myr.
/// e-fold ≈ 2 BY — the pace at which Earth's own biosphere remade its sky (the
/// Great Oxidation arrived two billion years in, not at the 250 My the old
/// shortcut implied). Mechanism tests use [`Levers::brisk`](crate::Levers::brisk).
pub const DEFAULT_PRODUCTION_RATE: f64 = 0.0005;
/// Fraction of an exposed organic bed that rots per Myr, once something can eat
/// it. **Nearly all of it**: dead tissue left lying in the open does not last a
/// million years anywhere. Only the **top** bed is exposed, so burial under the
/// next thing to land is the one thing that ever saved a forest — which is the
/// entire reason coal exists, and why so little of what grows becomes any.
const DECAY_RATE: f64 = 0.9;
/// Free-oxygen mass fraction below which rot runs anaerobic and gives off
/// methane instead of carbon dioxide.
const ANAEROBIC_O2_FRAC: f64 = 0.01;
/// Buried lignin, kg, at which the decomposer guild finds it worth evolving to
/// eat wood. The default is Earth-ish in spirit: late enough for a Carboniferous
/// to happen, early enough that it does not last forever.
pub const DEFAULT_DECOMPOSER_NICHE_KG: f64 = 5.0e18;

/// **Biosphere** — the life books: what is alive, what it fixes, and what rots.
///
/// One stage doing three things in the order they actually happen: the world
/// decides what can live in it, what lives fixes carbon, and what is exposed
/// rots. See the module docs for why that is enough to produce coal, oil, an
/// oxygen atmosphere and a methane greenhouse without any of them being named
/// as a goal.
pub struct Biosphere {
    /// Fraction of the air's CO₂ fixed per Myr at full production.
    pub rate: f64,
    /// Buried lignin at which the decomposer guild arrives.
    pub decomposer_niche_kg: f64,
    /// Catalog stoichiometry, resolved once.
    cellulose: Vec<(ElementId, f64)>,
    lignin: Vec<(ElementId, f64)>,
    co2: Vec<(ElementId, f64)>,
    ch4: Vec<(ElementId, f64)>,
    water: Vec<(ElementId, f64)>,
}

impl Biosphere {
    /// Resolve the organic vocabulary from the catalog. Panics if a compound is
    /// missing or re-numbered — the vocabulary is `sim_required`.
    pub fn new(tables: &Tables, rate: f64, decomposer_niche_kg: f64) -> Self {
        let of = |name: &str, id: CompoundId| {
            let def = tables
                .compound(name)
                .unwrap_or_else(|| panic!("the biosphere needs '{name}' in compounds.json"));
            assert_eq!(
                def.id, id,
                "'{name}' moved in the catalog: {} != {id}",
                def.id
            );
            tables.compound_mass_fractions(def)
        };
        Self {
            rate,
            decomposer_niche_kg,
            cellulose: of("Cellulose", CELLULOSE),
            lignin: of("Lignin", LIGNIN),
            co2: of("Carbon Dioxide", CARBON_DIOXIDE),
            ch4: of("Methane", METHANE),
            water: of("Water", crate::atmosphere::WATER_VAPOUR),
        }
    }

    /// Mass fraction of `element` in a resolved compound.
    fn frac(fracs: &[(ElementId, f64)], element: ElementId) -> f64 {
        fracs
            .iter()
            .find(|&&(e, _)| e == element)
            .map_or(0.0, |&(_, f)| f)
    }

    /// **What the world will support right now**, given where life already got.
    /// Conditions only — never the clock, and never the habitability observer's
    /// verdict (that would make an observer causal). These read the same raw
    /// quantities the gauges classify, which is why the two agree without either
    /// one consulting the other.
    fn reachable(&self, world: &World, state: &PlanetState, surface_k: f64) -> LifeStage {
        let temperate = (LIFE_COLD_K..=LIFE_HOT_K).contains(&surface_k);
        if !temperate || state.ocean_mass_kg <= 0.0 {
            return world.life;
        }
        let air = world.reservoirs.atmosphere.mass_kg();
        let o2_frac = if air > 0.0 {
            world.reservoirs.atmosphere.species.amount(OXYGEN) / air
        } else {
            0.0
        };
        // Land to stand on, and enough oxygen overhead to stand there.
        let land = state.submerged_frac < 0.99;
        // The niche: buried lignin is what a wood-eater would evolve to eat.
        let lignin_buried: f64 = world
            .columns
            .iter()
            .flat_map(|c| c.layers.iter())
            .map(|l| l.minerals.amount(LIGNIN))
            .sum();

        let mut reach = LifeStage::Microbial;
        if land && o2_frac >= LAND_LIFE_O2_FRAC {
            reach = LifeStage::Floral;
            if lignin_buried >= self.decomposer_niche_kg {
                reach = LifeStage::Decomposers;
            }
        }
        reach
    }

    /// Fix `make` kg of an organic compound on one column: carbon out of the
    /// air, hydrogen out of the sea, and the oxygen neither of them needs
    /// released as O₂. Exactly conserved — every kilogram drawn is either in the
    /// bed or in the sky.
    fn fix(
        &self,
        world: &mut World,
        cell: usize,
        make: f64,
        tissue: CompoundId,
        organic: &[(ElementId, f64)],
    ) {
        let (f_c, f_h, f_o) = (
            Self::frac(organic, C),
            Self::frac(organic, H),
            Self::frac(organic, O),
        );
        if f_c <= 0.0 || make <= 0.0 {
            return;
        }
        let (co2_c, co2_o) = (Self::frac(&self.co2, C), Self::frac(&self.co2, O));
        let (w_h, w_o) = (Self::frac(&self.water, H), Self::frac(&self.water, O));
        if co2_c <= 0.0 || w_h <= 0.0 {
            return;
        }
        let air = &mut world.reservoirs.atmosphere;

        // Bounded by the carbon actually booked in the sky and the water
        // actually standing in the sea.
        let c_avail = air.species.amount(CARBON_DIOXIDE) * co2_c;
        let h_avail = world.reservoirs.ocean.contents.amount(H);
        let make = make
            .min(c_avail / f_c)
            .min(if f_h > 0.0 { h_avail / f_h } else { f64::MAX });
        if make <= 0.0 {
            return;
        }

        // Draw the carbon down as CO₂ — species first, then the elements it
        // implied, so the compound bound can only ever slacken.
        let co2_take = make * f_c / co2_c;
        let got = air.species.remove(CARBON_DIOXIDE, co2_take);
        let c_in = air.contents.remove(C, got * co2_c);
        let o_from_air = air.contents.remove(O, got * co2_o);

        // And the hydrogen out of the sea, with the oxygen that came with it.
        let h_in = world.reservoirs.ocean.contents.remove(H, make * f_h);
        let o_from_sea = world.reservoirs.ocean.contents.remove(O, h_in / w_h * w_o);

        // Build the tissue out of what arrived, and give back the rest as the
        // oxygen that makes this planet breathable.
        let o_in_tissue = (c_in / f_c * f_o).min(o_from_air + o_from_sea);
        let deposit = [(C, c_in), (H, h_in), (O, o_in_tissue)];
        let spare = o_from_air + o_from_sea - o_in_tissue;
        if spare > 0.0 {
            world.reservoirs.atmosphere.contents.add(O, spare);
            world.reservoirs.atmosphere.species.add(OXYGEN, spare);
        }
        let add: Vec<(ElementId, f64)> = deposit.into_iter().filter(|&(_, m)| m > 0.0).collect();
        if add.is_empty() {
            return;
        }
        let at = world.tick_myr;
        world.columns[cell].deposit(FormationProcess::Organic, at, &add);
        // Book the tissue on the bed it landed in, at the mass its own elements
        // can cover — the compound bound holds by construction.
        let booked = organic
            .iter()
            .filter(|&&(_, f)| f > 0.0)
            .map(|&(e, f)| add.iter().find(|&&(a, _)| a == e).map_or(0.0, |&(_, m)| m) / f)
            .fold(f64::MAX, f64::min);
        if booked.is_finite() && booked > 0.0 {
            if let Some(bed) = world.columns[cell].layers.last_mut() {
                bed.minerals.add(tissue, booked);
            }
        }
    }
}

impl Stage for Biosphere {
    fn name(&self) -> &'static str {
        "Biosphere"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let state = PlanetState::sample(world);
        let surface_k = crate::surface::mean_surface_temp_k(world, self.stellar_proxy());

        // ── 1. Who is alive. Advance-only: the world can offer less than it
        //       once did, but a biosphere never forgets what it learned. ──
        let reach = self.reachable(world, &state, surface_k);
        while world.life < reach {
            match world.life.next() {
                Some(up) => world.life = up,
                None => break,
            }
        }
        if world.life == LifeStage::Barren {
            return;
        }

        // ── 2. What it fixes. The sky's carbon is the budget; every column that
        //       can grow takes an equal share of it. ──
        let area = world.cell_area_m2();
        let sea = sea_level_m(world);
        let land: Vec<usize> = (0..world.columns.len())
            .filter(|&i| {
                !world.columns[i].layers.is_empty() && elevation_m(&world.columns[i], area) >= sea
            })
            .collect();
        let marine: Vec<usize> = (0..world.columns.len())
            .filter(|&i| {
                !world.columns[i].layers.is_empty() && elevation_m(&world.columns[i], area) < sea
            })
            .collect();

        let budget = world.reservoirs.atmosphere.species.amount(CARBON_DIOXIDE)
            * (self.rate * dt_myr).min(1.0);
        // Forests only where there are forests to have; the sea always works.
        let woody = world.life >= LifeStage::Floral && !land.is_empty();
        let (marine_share, land_share) = if woody { (0.5, 0.5) } else { (1.0, 0.0) };

        if !marine.is_empty() && marine_share > 0.0 {
            let each = budget * marine_share
                / marine.len() as f64
                / Self::frac(&self.cellulose, C).max(f64::MIN_POSITIVE);
            let cellulose = self.cellulose.clone();
            for &i in &marine {
                self.fix(world, i, each, CELLULOSE, &cellulose);
            }
        }
        if woody && land_share > 0.0 {
            // **Lignin** — the molecule the coal window is made of.
            let each = budget * land_share
                / land.len() as f64
                / Self::frac(&self.lignin, C).max(f64::MIN_POSITIVE);
            let lignin = self.lignin.clone();
            for &i in &land {
                self.fix(world, i, each, LIGNIN, &lignin);
            }
        }

        // ── 3. What rots. Only the EXPOSED bed, and only what something alive
        //       can actually digest — which is the whole coal mechanism. ──
        self.rot(world, dt_myr);
    }
}

impl Biosphere {
    /// The insolation the surface read is taken at. The biosphere does not own
    /// a stellar lever of its own — it reads the world at nominal brightness,
    /// and the star's real strength reaches it through the greenhouse the air
    /// is already carrying.
    fn stellar_proxy(&self) -> f64 {
        1.0
    }

    /// **Rot.** Take a bite out of every exposed organic bed and hand the
    /// elements back — as CO₂ if there is oxygen to respire with, as methane if
    /// there is not. Buried beds are untouched: burial is the only thing that
    /// ever saved a forest, and it is why there is coal at all.
    fn rot(&self, world: &mut World, dt_myr: f64) {
        let air_mass = world.reservoirs.atmosphere.mass_kg();
        let o2 = world.reservoirs.atmosphere.species.amount(OXYGEN);
        let aerobic = air_mass > 0.0 && o2 / air_mass >= ANAEROBIC_O2_FRAC;
        // Lignin is food only once the guild that eats wood has arrived.
        let eats_lignin = world.life >= LifeStage::Decomposers;
        let bite = (DECAY_RATE * dt_myr).clamp(0.0, 1.0);

        for cell in 0..world.columns.len() {
            let Some(bed) = world.columns[cell].layers.last_mut() else {
                continue;
            };
            if bed.formed_by != FormationProcess::Organic {
                continue;
            }
            // What of this bed is digestible right now.
            let mut edible = bed.minerals.amount(CELLULOSE) * bite;
            if eats_lignin {
                edible += bed.minerals.amount(LIGNIN) * bite;
            }
            if edible <= 0.0 {
                continue;
            }
            let cellulose = bed.minerals.amount(CELLULOSE) * bite;
            let lignin = if eats_lignin {
                bed.minerals.amount(LIGNIN) * bite
            } else {
                0.0
            };
            bed.minerals.remove(CELLULOSE, cellulose);
            if lignin > 0.0 {
                bed.minerals.remove(LIGNIN, lignin);
            }
            // Release the elements those compounds were holding.
            let mut freed: Vec<(ElementId, f64)> = Vec::new();
            for (fracs, mass) in [(&self.cellulose, cellulose), (&self.lignin, lignin)] {
                for &(e, f) in fracs {
                    let take = bed.elements.remove(e, mass * f);
                    if take > 0.0 {
                        match freed.iter_mut().find(|(fe, _)| *fe == e) {
                            Some(slot) => slot.1 += take,
                            None => freed.push((e, take)),
                        }
                    }
                }
            }
            if bed.elements.is_empty() {
                world.columns[cell].layers.pop();
            }
            if freed.is_empty() {
                continue;
            }

            let air = &mut world.reservoirs.atmosphere;
            if aerobic {
                // Respiration: the carbon burns back to CO₂ on the air's own
                // oxygen, which is what stops O₂ running away for ever.
                let need_o = Self::frac(&self.co2, O) / Self::frac(&self.co2, C).max(1e-12)
                    * freed
                        .iter()
                        .find(|&&(e, _)| e == C)
                        .map_or(0.0, |&(_, m)| m);
                let have_o = freed
                    .iter()
                    .find(|&&(e, _)| e == O)
                    .map_or(0.0, |&(_, m)| m);
                if need_o > have_o {
                    let short = (need_o - have_o).min(air.species.amount(OXYGEN));
                    let got = air.species.remove(OXYGEN, short);
                    let pulled = air.contents.remove(O, got);
                    match freed.iter_mut().find(|(e, _)| *e == O) {
                        Some(slot) => slot.1 += pulled,
                        None => freed.push((O, pulled)),
                    }
                }
                fly_as(air, &mut freed, CARBON_DIOXIDE, &self.co2, f64::INFINITY);
            } else {
                // No oxidant: anaerobic rot, and what comes off a swamp with no
                // oxygen in it is METHANE — twelve times the greenhouse of the
                // CO₂ it replaces.
                fly_as(air, &mut freed, METHANE, &self.ch4, f64::INFINITY);
                fly_as(air, &mut freed, CARBON_DIOXIDE, &self.co2, f64::INFINITY);
            }
            // Whatever would not fly dissolves back into the sea rather than
            // floating as loose atoms in the sky.
            for (e, m) in freed {
                if m > 0.0 {
                    world.reservoirs.ocean.contents.add(e, m);
                }
            }
        }
    }
}

/// Burial pressure at which organic matter has cooked into rock, Pa. Deeper than
/// [`LITHIFICATION_PA`](crate::column) by a good margin — the oil window is a
/// **depth** window, and depth is the honest driver here: a bed's recorded peak
/// pressure is real pascals of overburden, where its recorded peak temperature
/// is still the mantle's, not a geotherm's (the same gap that blocks
/// metamorphism).
const MATURATION_PA: f64 = 5.0e7;

/// **Maturation** — buried organic matter cooks into fuel.
///
/// A bed that has been carried deep enough stops being dead plants and becomes
/// rock: on land the lignin-rich piles turn to **coal**, under the sea the
/// algal muds turn to **oils**. Which one a bed becomes is decided by where it
/// was buried, not by a rule naming either.
///
/// Coalification is a **dehydration**: the tissue gives up its hydrogen and
/// oxygen and keeps its carbon. Those leave as water and — where the hydrogen
/// outlasts the oxygen — as **methane**, which is exactly why coal seams carry
/// firedamp. Nothing is created; the bed simply stops holding what it can no
/// longer hold.
pub struct Maturation {
    cellulose: Vec<(ElementId, f64)>,
    lignin: Vec<(ElementId, f64)>,
    coal: Vec<(ElementId, f64)>,
    oils: Vec<(ElementId, f64)>,
    co2: Vec<(ElementId, f64)>,
    ch4: Vec<(ElementId, f64)>,
    water: Vec<(ElementId, f64)>,
}

impl Maturation {
    /// Resolve the mature-product vocabulary from the catalog.
    pub fn new(tables: &Tables) -> Self {
        let of = |name: &str, id: CompoundId| {
            let def = tables
                .compound(name)
                .unwrap_or_else(|| panic!("maturation needs '{name}' in compounds.json"));
            assert_eq!(
                def.id, id,
                "'{name}' moved in the catalog: {} != {id}",
                def.id
            );
            tables.compound_mass_fractions(def)
        };
        Self {
            cellulose: of("Cellulose", CELLULOSE),
            lignin: of("Lignin", LIGNIN),
            coal: of("Coal", COAL),
            oils: of("Oils", OILS),
            co2: of("Carbon Dioxide", CARBON_DIOXIDE),
            ch4: of("Methane", METHANE),
            water: of("Water", crate::atmosphere::WATER_VAPOUR),
        }
    }
}

impl Stage for Maturation {
    fn name(&self) -> &'static str {
        "Maturation"
    }

    fn tick(&self, world: &mut World, _dt_myr: f64, _rng: &mut StageRng) {
        let area = world.cell_area_m2();
        let sea = sea_level_m(world);
        for cell in 0..world.columns.len() {
            let marine = elevation_m(&world.columns[cell], area) < sea;
            for bed in world.columns[cell].layers.iter_mut() {
                if bed.formed_by != FormationProcess::Organic || bed.peak_pt.0 < MATURATION_PA {
                    continue;
                }
                let cellulose = bed.minerals.amount(CELLULOSE);
                let lignin = bed.minerals.amount(LIGNIN);
                if cellulose + lignin <= 0.0 {
                    continue;
                }
                bed.minerals.remove(CELLULOSE, cellulose);
                bed.minerals.remove(LIGNIN, lignin);

                // Take back **only what the tissue itself was holding**. A bed
                // can carry other minerals too — calcite off the sea floor,
                // silicates the crystalliser grew — and draining the whole bed
                // would pull the elements out from under those bookings and
                // break the compound bound on rock that had nothing to do with
                // life.
                let mut freed: Vec<(ElementId, f64)> = Vec::new();
                for (fracs, mass) in [(&self.cellulose, cellulose), (&self.lignin, lignin)] {
                    for &(e, f) in fracs {
                        let take = bed.elements.remove(e, mass * f);
                        if take > 0.0 {
                            match freed.iter_mut().find(|(fe, _)| *fe == e) {
                                Some(slot) => slot.1 += take,
                                None => freed.push((e, take)),
                            }
                        }
                    }
                }
                if freed.is_empty() {
                    continue;
                }

                // The mature product takes what it can; the rest is expelled.
                let (id, fracs) = if marine {
                    (OILS, &self.oils)
                } else {
                    (COAL, &self.coal)
                };
                let made = fracs
                    .iter()
                    .filter(|&&(_, f)| f > 0.0)
                    .fold(f64::MAX, |g, &(e, f)| {
                        g.min(
                            freed
                                .iter()
                                .find(|&&(a, _)| a == e)
                                .map_or(0.0, |&(_, m)| m)
                                / f,
                        )
                    });
                if made.is_finite() && made > 0.0 {
                    for &(e, f) in fracs {
                        if let Some(slot) = freed.iter_mut().find(|(a, _)| *a == e) {
                            let took = (made * f).min(slot.1);
                            slot.1 -= took;
                            bed.elements.add(e, took);
                        }
                    }
                    bed.minerals.add(id, made);
                }

                // What coalification drives off: water first, then the methane
                // that outlasting hydrogen makes, then any carbon dioxide.
                let air = &mut world.reservoirs.atmosphere;
                fly_as(
                    air,
                    &mut freed,
                    crate::atmosphere::WATER_VAPOUR,
                    &self.water,
                    f64::INFINITY,
                );
                fly_as(air, &mut freed, METHANE, &self.ch4, f64::INFINITY);
                fly_as(air, &mut freed, CARBON_DIOXIDE, &self.co2, f64::INFINITY);
                // Anything still unaccounted for stays in the rock as free
                // elements — legal, and the audit checks it every tick.
                for (e, m) in freed {
                    if m > 0.0 {
                        bed.elements.add(e, m);
                    }
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
    use flicker_materials::JsonTableSource;
    use flicker_worldgrid::icosphere;

    fn tables() -> Tables {
        Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables")
    }

    /// A cool wet world with a CO₂ sky and a floor to grow on.
    fn living_world(seed: u64) -> (World, Tables) {
        let t = tables();
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, seed);
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 400.0;
        }
        // A crusted floor. Life accumulates ON something — a world that has not
        // frozen over yet has nowhere to put a peat bed.
        let mut rng = StageRng::new(seed);
        crate::crust::CrustGeneration { rate: 0.5 }.tick(&mut w, 20.0, &mut rng);
        // A sea.
        let sea = 1.4e21;
        w.reservoirs.ocean.contents.add(H, sea / 9.0);
        w.reservoirs.ocean.contents.add(O, sea * 8.0 / 9.0);
        w.reservoirs.delivered.add(H, sea / 9.0);
        w.reservoirs.delivered.add(O, sea * 8.0 / 9.0);
        // A CO₂ sky, booked at catalog stoichiometry.
        let t2 = tables();
        let co2 = t2.compound("Carbon Dioxide").expect("co2");
        let amount = 5.0e18;
        for (e, f) in t2.compound_mass_fractions(co2) {
            w.reservoirs.atmosphere.contents.add(e, amount * f);
            w.reservoirs.delivered.add(e, amount * f);
        }
        w.reservoirs.atmosphere.species.add(CARBON_DIOXIDE, amount);
        (w, t)
    }

    /// **The ledger entry that is coal and oxygen at once.** Photosynthesis
    /// takes carbon out of the sky, puts it in the ground, and releases the
    /// oxygen neither the tissue nor the water needed — conserved throughout.
    #[test]
    fn fixing_carbon_buries_it_and_releases_oxygen() {
        let (mut w, t) = living_world(71);
        let bio = Biosphere::new(&t, DEFAULT_PRODUCTION_RATE, DEFAULT_DECOMPOSER_NICHE_KG);
        let sky_before = w.reservoirs.atmosphere.species.amount(CARBON_DIOXIDE);
        let mut rng = StageRng::new(1);
        for _ in 0..5 {
            bio.tick(&mut w, 1.0, &mut rng);
            w.tick_myr += 1.0;
            w.audit("Biosphere");
            w.audit_compound_bound("Biosphere");
        }
        assert!(
            w.life >= LifeStage::Microbial,
            "a temperate wet world came alive"
        );
        assert!(
            w.reservoirs.atmosphere.species.amount(CARBON_DIOXIDE) < sky_before,
            "carbon left the sky"
        );
        assert!(
            w.reservoirs.atmosphere.species.amount(OXYGEN) > 0.0,
            "and the oxygen it displaced arrived — free O2 is a biosignature"
        );
        let organic: f64 = w
            .columns
            .iter()
            .flat_map(|c| c.layers.iter())
            .map(|l| l.minerals.amount(CELLULOSE))
            .sum();
        assert!(organic > 0.0, "the carbon is in the ground as tissue");
    }

    /// **The coal window, chemically.** Before the guild that eats wood exists,
    /// lignin laid down on land does not rot; once it exists, it does. The gate
    /// is not a date — it is which compound something can digest.
    #[test]
    fn lignin_survives_until_the_decomposers_arrive() {
        let (mut w, t) = living_world(72);
        let bio = Biosphere::new(&t, DEFAULT_PRODUCTION_RATE, DEFAULT_DECOMPOSER_NICHE_KG);
        // A lignin bed sitting exposed on the surface.
        let cell = 0usize;
        let lignin_mass = 1.0e17;
        let mut add: Vec<(ElementId, f64)> = Vec::new();
        for &(e, f) in &bio.lignin {
            add.push((e, lignin_mass * f));
            w.reservoirs.delivered.add(e, lignin_mass * f);
        }
        w.columns[cell].deposit(FormationProcess::Organic, 0.0, &add);
        w.columns[cell]
            .layers
            .last_mut()
            .expect("bed")
            .minerals
            .add(LIGNIN, lignin_mass);
        w.audit_compound_bound("seeded lignin");

        // Forests, but nothing that can eat wood.
        w.life = LifeStage::Floral;
        let mut rng = StageRng::new(2);
        bio.rot(&mut w, 1.0);
        w.audit("rot/floral");
        let survived: f64 = w.columns[cell]
            .layers
            .iter()
            .map(|l| l.minerals.amount(LIGNIN))
            .sum();
        assert!(
            (survived - lignin_mass).abs() < lignin_mass * 1e-9,
            "wood does not rot before the guild that eats wood: {survived:.3e}"
        );

        // The termites arrive, and the window shuts.
        w.life = LifeStage::Decomposers;
        bio.rot(&mut w, 1.0);
        w.audit("rot/decomposers");
        w.audit_compound_bound("rot/decomposers");
        let after: f64 = w.columns[cell]
            .layers
            .iter()
            .map(|l| l.minerals.amount(LIGNIN))
            .sum();
        assert!(
            after < survived,
            "and now it rots: {after:.3e} < {survived:.3e}"
        );
        let _ = &mut rng;
    }

    /// **Rot needs an oxidant, and what it gives off says which one it had.**
    /// An anoxic world's swamps breathe methane; an oxygenated world's breathe
    /// carbon dioxide. The greenhouse difference between those two is the whole
    /// early-Earth climate story, and nothing here schedules it.
    #[test]
    fn anoxic_rot_makes_methane_and_oxygenated_rot_makes_carbon_dioxide() {
        let seed_bed = |w: &mut World, bio: &Biosphere| {
            let mass = 1.0e17;
            let mut add: Vec<(ElementId, f64)> = Vec::new();
            for &(e, f) in &bio.cellulose {
                add.push((e, mass * f));
                w.reservoirs.delivered.add(e, mass * f);
            }
            w.columns[0].deposit(FormationProcess::Organic, 0.0, &add);
            w.columns[0]
                .layers
                .last_mut()
                .expect("bed")
                .minerals
                .add(CELLULOSE, mass);
        };

        // No oxygen in the sky: the swamp breathes methane.
        let (mut anoxic, t) = living_world(73);
        let bio = Biosphere::new(&t, DEFAULT_PRODUCTION_RATE, DEFAULT_DECOMPOSER_NICHE_KG);
        anoxic.life = LifeStage::Microbial;
        seed_bed(&mut anoxic, &bio);
        bio.rot(&mut anoxic, 1.0);
        anoxic.audit("anoxic rot");
        anoxic.audit_compound_bound("anoxic rot");
        assert!(
            anoxic.reservoirs.atmosphere.species.amount(METHANE) > 0.0,
            "anaerobic rot gives off methane"
        );

        // A breathable sky: the same bed rots to CO₂ instead.
        let (mut oxic, t2) = living_world(74);
        let bio2 = Biosphere::new(&t2, DEFAULT_PRODUCTION_RATE, DEFAULT_DECOMPOSER_NICHE_KG);
        oxic.life = LifeStage::Microbial;
        let o2_mass = oxic.reservoirs.atmosphere.mass_kg();
        oxic.reservoirs.atmosphere.contents.add(O, o2_mass);
        oxic.reservoirs.delivered.add(O, o2_mass);
        oxic.reservoirs.atmosphere.species.add(OXYGEN, o2_mass);
        seed_bed(&mut oxic, &bio2);
        let ch4_before = oxic.reservoirs.atmosphere.species.amount(METHANE);
        bio2.rot(&mut oxic, 1.0);
        oxic.audit("oxic rot");
        oxic.audit_compound_bound("oxic rot");
        assert_eq!(
            oxic.reservoirs.atmosphere.species.amount(METHANE),
            ch4_before,
            "with oxygen to respire with, no methane"
        );
    }

    /// Burial cooks tissue into fuel, and where it was buried decides which:
    /// land makes coal, the sea makes oils. Conserved — what the rock stops
    /// holding leaves as gas.
    #[test]
    fn burial_cooks_organics_into_coal_and_oil() {
        let (mut w, t) = living_world(75);
        let bio = Biosphere::new(&t, DEFAULT_PRODUCTION_RATE, DEFAULT_DECOMPOSER_NICHE_KG);
        let mat = Maturation::new(&t);
        let mass = 1.0e17;
        let cells = [0usize, 1usize];
        let mut add: Vec<(ElementId, f64)> = Vec::new();
        for &(e, f) in &bio.lignin {
            add.push((e, mass * f));
            // Credited once PER BED — the beds are seeded from outside the sim,
            // so every gram of them has to arrive on the ledger's right side.
            w.reservoirs.delivered.add(e, mass * f * cells.len() as f64);
        }
        // Two organic beds, both buried deep enough to have cooked.
        for cell in cells {
            w.columns[cell].deposit(FormationProcess::Organic, 0.0, &add);
            let bed = w.columns[cell].layers.last_mut().expect("bed");
            bed.minerals.add(LIGNIN, mass);
            bed.peak_pt = (MATURATION_PA * 2.0, 400.0);
        }
        w.audit_compound_bound("seeded");
        let mut rng = StageRng::new(3);
        mat.tick(&mut w, 1.0, &mut rng);
        w.audit("Maturation");
        w.audit_compound_bound("Maturation");

        let fuel: f64 = w
            .columns
            .iter()
            .flat_map(|c| c.layers.iter())
            .map(|l| l.minerals.amount(COAL) + l.minerals.amount(OILS))
            .sum();
        assert!(fuel > 0.0, "buried tissue became fuel");
        let raw: f64 = w
            .columns
            .iter()
            .flat_map(|c| c.layers.iter())
            .map(|l| l.minerals.amount(LIGNIN))
            .sum();
        assert_eq!(raw, 0.0, "and stopped being raw tissue");
    }

    /// Shallow tissue is not fuel. Maturation is a **burial** process, so a bed
    /// that has never been carried down stays exactly what it was.
    #[test]
    fn unburied_tissue_does_not_mature() {
        let (mut w, t) = living_world(76);
        let bio = Biosphere::new(&t, DEFAULT_PRODUCTION_RATE, DEFAULT_DECOMPOSER_NICHE_KG);
        let mat = Maturation::new(&t);
        let mass = 1.0e17;
        let mut add: Vec<(ElementId, f64)> = Vec::new();
        for &(e, f) in &bio.cellulose {
            add.push((e, mass * f));
            w.reservoirs.delivered.add(e, mass * f);
        }
        w.columns[0].deposit(FormationProcess::Organic, 0.0, &add);
        w.columns[0]
            .layers
            .last_mut()
            .expect("bed")
            .minerals
            .add(CELLULOSE, mass);
        // peak_pt left at zero: never buried.
        let mut rng = StageRng::new(4);
        mat.tick(&mut w, 1.0, &mut rng);
        w.audit("Maturation");
        let coal: f64 = w.columns[0]
            .layers
            .iter()
            .map(|l| l.minerals.amount(COAL))
            .sum();
        assert_eq!(coal, 0.0, "unburied tissue is not coal");
        let raw: f64 = w.columns[0]
            .layers
            .iter()
            .map(|l| l.minerals.amount(CELLULOSE))
            .sum();
        assert!(raw > 0.0, "it is still tissue");
    }

    /// The stages only ever go forward. A world that freezes over after
    /// learning to rot wood has not forgotten how.
    #[test]
    fn life_never_regresses() {
        let (mut w, t) = living_world(77);
        let bio = Biosphere::new(&t, DEFAULT_PRODUCTION_RATE, DEFAULT_DECOMPOSER_NICHE_KG);
        w.life = LifeStage::Decomposers;
        // Freeze it solid — nothing could live here now.
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 3.0;
        }
        let state = PlanetState::sample(&w);
        // `reachable` never reports below where life already got — the floor IS
        // the current stage, which is what advance-only means in one line.
        let reach = bio.reachable(&w, &state, 100.0);
        assert_eq!(
            reach,
            LifeStage::Decomposers,
            "a dead world offers no ADVANCE"
        );
        let mut rng = StageRng::new(5);
        bio.tick(&mut w, 1.0, &mut rng);
        assert_eq!(
            w.life,
            LifeStage::Decomposers,
            "and the books do not un-learn"
        );
    }
}

#[cfg(test)]
mod molten_world_is_not_alive {
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::planet::{PlanetState, World};
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    /// **A magma ocean is not a temperate world.** Aaron, one tick into a
    /// molten planet: *"the very first gate this thing hits after the very
    /// first tick of the molten world is LIFE GATE... I'm pretty sure 'world
    /// supports life' probably doesn't apply to a totally molten planet."*
    ///
    /// He was right, and it was two defects wearing one coat:
    ///
    /// 1. **Surface temperature ignored the planet's own heat.** It was purely
    ///    star + greenhouse, so a 3977 K magma ocean reported ~290 K and the
    ///    habitability gauge called it in band. Bare ground now reads the
    ///    mantle it *is*.
    /// 2. **The life gate asked for ingredients, not a place.** Water and air
    ///    are both nonzero on tick 1 (the infall delivers, the magma exhales),
    ///    and on a crustless ball every column has identical relief — so the
    ///    first drop of water "submerges" 100% of the planet and the sea looked
    ///    real. Life now needs a lid and a sea standing on it.
    #[test]
    fn a_magma_ocean_reads_molten_and_life_stays_shut() {
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("t"));
        let b = Budget::from_dir(&dir, &t).expect("b");
        let mut w = World::seed(icosphere(4), b, &t, 5);

        // Hand it the tick-1 conditions from the screenshot: water arrived, the
        // magma exhaled an atmosphere, and NOTHING has frozen.
        w.reservoirs.ocean.contents.add(1, 1.0e19);
        w.reservoirs.ocean.contents.add(8, 8.0e19);
        w.reservoirs.delivered.add(1, 1.0e19);
        w.reservoirs.delivered.add(8, 8.0e19);
        assert!(
            w.columns.iter().all(|c| c.layers.is_empty()),
            "no crust anywhere"
        );

        // 1. The surface reads MOLTEN, not temperate. Under the old law this
        //    came out near 290 K — a spring day on an ocean of lava.
        let surface = crate::surface::mean_surface_temp_k(&w, 1.0);
        let mantle = w.mantle.temp_k.iter().sum::<f64>() / w.mantle.n_cells() as f64;
        assert!(
            surface > 3000.0,
            "bare ground radiates the interior: surface {surface:.0} K vs mantle {mantle:.0} K"
        );
        assert!(
            (surface - mantle).abs() < 1.0,
            "with no lid at all, the surface IS the mantle"
        );

        // 2. And the life gate is shut, however much water and air there is.
        let state = PlanetState::sample(&w);
        assert_eq!(state.lid_frac, 0.0, "nothing has frozen");
        assert!(state.ocean_mass_kg > 0.0 && state.atmosphere_mass_kg >= 0.0);
        assert!(
            !crate::process_file::gate_of("Biosphere").holds(&state, &crate::Levers::default()),
            "a molten world is not a place to live, whatever it is made of"
        );

        // 3. Freeze a lid over it and the surface cools to the radiative
        //    balance — the gate is the WORLD's to open, and now it can.
        let at = w.tick_myr;
        for i in 0..w.columns.len() {
            w.columns[i].deposit(
                crate::column::FormationProcess::OceanicCrust,
                at,
                &[(14, 4.0e18), (8, 5.0e18)],
            );
        }
        let lidded = crate::surface::mean_surface_temp_k(&w, 1.0);
        assert!(
            lidded < 400.0,
            "once the lid is on, the star and the air decide: {lidded:.0} K"
        );
        assert_eq!(
            PlanetState::sample(&w).lid_frac,
            1.0,
            "the world is covered"
        );
    }
}
