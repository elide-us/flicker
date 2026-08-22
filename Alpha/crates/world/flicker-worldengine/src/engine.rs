//! [`WorldEngine`] — the forward-regenerative facade. It owns the topology, the
//! per-world [`WorldConfig`], and a lazy cache of one immutable [`EpochSnapshot`]
//! per epoch, and exposes the "play god" verbs behind the manifesto's locked
//! replay-forward mechanic:
//!
//! - [`snapshot`](WorldEngine::snapshot)`(e)` returns epoch `e`'s state, computing
//!   any missing epochs `..=e` forward from the seed layer on demand and caching
//!   them. Scrubbing the timeline is just repeated `snapshot` calls.
//! - editing a lever ([`set_lever`](WorldEngine::set_lever)) or reseeding an epoch
//!   ([`reseed`](WorldEngine::reseed)) **invalidates that epoch and everything
//!   after it** and leaves earlier epochs frozen — so tweaking Epoch 6 re-runs only
//!   Epoch 6, while tweaking Epoch 1 reseeds the whole chain. Changing the grid
//!   [`set_freq`](WorldEngine::set_freq) rebuilds the topology and invalidates all.
//!
//! Each epoch's output is a pure function of `(prev snapshot, config, per-epoch
//! seed)`, so the cache is sound: nothing is carried between runs but the snapshot.
//! The engine is GPU-free — a viewer drives it and renders the snapshots.

use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::{
    cooling, ChemistryParams, Epoch1, EpochCtx, EpochTransform, FormerPlan, HexState,
};
use flicker_worldgrid::{icosphere_with_outlines, Sphere};
use glam::Vec3;

use crate::config::{
    build_epoch1, build_transforms, mutate_epoch, next_seed, seed_chain, WorldConfig, MAX_FREQ,
    MIN_FREQ, WORLD_EPOCHS,
};
use crate::levers::{GeneratorError, GeneratorParams};
use crate::snapshot::{EpochSnapshot, Provenance};

/// Water-cycle sweeps run per unit of Epoch-4 `duration` — sets how far atmospheric
/// vapour travels (≈ one cell per sweep) before the batch pass ends.
const WATER_SWEEPS_PER_DURATION: u32 = 8;

/// Molten-convection passes per unit of Epoch-2 `duration` — how far the
/// differential-rotation shear carries material (the swirl strength).
const MOLTEN_STEPS_PER_DURATION: u32 = 4;

/// Tectonic steps per unit of Epoch-3 `duration` — how far a plate drifts over a
/// plume, i.e. how long the emergent hotspot chains grow.
const TECTONIC_STEPS_PER_DURATION: u32 = 4;

/// Cooling-clock advance per unit of `duration` for the **post-tectonic** epochs
/// (E4–E9). They used to *freeze* the thermal state after tectonics; now they keep
/// radiating, so `T` falls continuously across the whole history — one heat-loss
/// timeline, not a clock that stops at E3. Matches the molten/tectonic step rates so
/// the clock advances uniformly per duration unit. (These epochs' physics is unchanged
/// — only the recorded `T` advances; per-onset gating of each is a later slice.)
const COOL_STEPS_PER_DURATION: u32 = 4;

/// Blend weight of the **absolute** crust-thickness isostasy vs the percentile rank
/// in Epoch 3's base elevation (see `Epoch3::isostasy_abs`). High enough to break the
/// rank-stretched "thin ridge spine" relief into broad continental shelves, while a
/// little rank keeps a guaranteed continent↔ocean spread before the crust has piled.
const ISOSTASY_ABS_BLEND: f32 = 0.6;

/// Neighbour-average passes that spread the drifted crust's cell-scale thickness scatter
/// (the whole-cell conveyor's per-cell convergence/ridge resolution) into broad ranges
/// before isostasy — the fix for the single-cell "noise field" relief spikes that appear
/// the moment plate transport starts (`flicker_worldgen::smooth_crust_thickness`).
const CRUST_DESCATTER_PASSES: u32 = 2;

/// Talus slope (normalized elevation) above which the pre-hydraulic weathering pass
/// relaxes a neighbour pair (see `flicker_worldgen::run_protoatmospheric_erosion`) —
/// comparable to Epoch 6's, so it targets the sharp tectonic spikes/streaks and leaves
/// the broad continental slopes alone.
const WEATHER_TALUS: f32 = 0.08;
/// Fraction of the above-talus excess a single weathering pass relaxes — gentler than
/// Epoch 6's hydraulic carving: this is a background mitigation against runaway relief,
/// not the main erosion event.
const WEATHER_RATE: f32 = 0.25;
/// Weathering passes per unit of Epoch `duration` for the post-tectonic epochs (E4/E5):
/// elapsed deep time keeps grinding relief down between the mountain-building (E3) and
/// Epoch 6's full hydraulic erosion. Kept low so E6 still inherits essentially the same
/// terrain, only with the spikes already tamed. (E3 weathers at the tectonic step rate
/// instead, so it scrubs with the iteration slider.)
const WEATHER_STEPS_PER_DURATION: u32 = 2;

/// Hydrothermal percolation steps per unit of Epoch-5 `duration` — how far the
/// circulating fluid spreads along the fault network before the batch pass ends,
/// i.e. how far the emergent veins branch out.
const HYDRO_STEPS_PER_DURATION: u32 = 4;

/// The whole world generator behind one handle.
pub struct WorldEngine {
    /// Material vocabulary (feeds `EpochCtx.tables`).
    tables: Tables,
    /// Loaded lever defaults/ranges (defines defaults, clamps edits, drives reseed).
    params: GeneratorParams,
    /// The current input recipe.
    config: WorldConfig,
    /// Per-epoch seed chain (`seeds[e-1]` drives epoch `e`); a per-epoch reseed
    /// advances one entry and re-derives the later ones.
    seeds: Vec<u64>,
    /// Icosphere topology, rebuilt when `freq` changes.
    sphere: Sphere,
    /// Per-cell boundary polygons for the viewer (built with the sphere).
    outlines: Vec<Vec<Vec3>>,
    /// One slot per epoch (`cache[e-1]` = epoch `e`); `None` = invalid/uncomputed.
    /// `cache[0]` (the Epoch-1 seed layer) is always `Some`.
    cache: Vec<Option<EpochSnapshot>>,
    /// The compiled compound-former plan (material-pipeline stage 2), built once
    /// from the vocabulary; run after each epoch to form that epoch's compounds.
    former: FormerPlan,
    /// Optional override for Epoch 3's **tectonic iteration count** — lets the viewer
    /// scrub the plate simulation step by step (0 = just the partition + isostasy,
    /// before any drift; full = the complete deformation). `None` = run in full.
    epoch3_steps: Option<u32>,
    /// Optional override for Epoch 2's **molten-convection iteration count** — lets the
    /// viewer scrub the convection (0 = the raw buoyancy seed, before any convecting;
    /// full = the settled heat cells). `None` = run in full.
    epoch2_steps: Option<u32>,
}

impl WorldEngine {
    /// Build an engine from an explicit vocabulary + lever tables + recipe. The
    /// sphere and the Epoch-1 seed layer are built now (cheap); epochs 2-9 fill
    /// lazily on the first [`snapshot`](Self::snapshot).
    pub fn new(tables: Tables, params: GeneratorParams, mut config: WorldConfig) -> Self {
        config.freq = config.clamped_freq();
        let seeds = seed_chain(config.seed);
        let (sphere, outlines) = icosphere_with_outlines(config.freq);
        let former = FormerPlan::compile(&tables);
        let mut engine = Self {
            tables,
            params,
            config,
            seeds,
            sphere,
            outlines,
            cache: vec![None; WORLD_EPOCHS],
            former,
            epoch3_steps: None,
            epoch2_steps: None,
        };
        engine.cache[0] = Some(engine.build_seed_layer());
        engine
    }

    /// Build an engine from the repo's `Alpha/content/data` (material vocabulary +
    /// generator levers) at the Earth-like defaults — the convenience the tests and
    /// the headless bake tool use.
    pub fn from_repo() -> Result<Self, GeneratorError> {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        let tables = Tables::from_source(&JsonTableSource::new(dir))
            .expect("repo Alpha/content/data material tables load");
        let params = GeneratorParams::from_dir(dir)?;
        let config = WorldConfig::from_params(&params);
        Ok(Self::new(tables, params, config))
    }

    // ── read access (for the viewer / consumers) ──────────────────────────────

    pub fn config(&self) -> &WorldConfig {
        &self.config
    }
    pub fn params(&self) -> &GeneratorParams {
        &self.params
    }
    pub fn tables(&self) -> &Tables {
        &self.tables
    }
    pub fn sphere(&self) -> &Sphere {
        &self.sphere
    }
    pub fn outlines(&self) -> &[Vec<Vec3>] {
        &self.outlines
    }
    pub fn seeds(&self) -> &[u64] {
        &self.seeds
    }

    /// An already-computed epoch snapshot, or `None` if it hasn't been generated
    /// since the last invalidation (no work done). `epoch` is 1-indexed.
    pub fn peek(&self, epoch: usize) -> Option<&EpochSnapshot> {
        self.cache
            .get(epoch.wrapping_sub(1))
            .and_then(|c| c.as_ref())
    }

    // ── the forward-regenerative core ─────────────────────────────────────────

    /// The snapshot for `epoch` (1..=[`WORLD_EPOCHS`]), generating any missing
    /// epochs up to it forward from the last cached one. This is the single call the
    /// timeline scrubs through.
    pub fn snapshot(&mut self, epoch: usize) -> &EpochSnapshot {
        let epoch = epoch.clamp(1, WORLD_EPOCHS);
        self.ensure(epoch);
        self.cache[epoch - 1]
            .as_ref()
            .expect("ensure filled the slot")
    }

    /// Fill `cache[..=epoch-1]` forward from the lowest cached epoch. `cache[0]` is
    /// always present, so this always has a starting point.
    fn ensure(&mut self, epoch: usize) {
        for e in 2..=epoch {
            if self.cache[e - 1].is_some() {
                continue;
            }
            // Borrow the previous epoch out, run this one against it, put both back.
            // `run_epoch` reads only `self` (tables/sphere/seeds/config), never the
            // cache, so pulling `prev` out is safe and avoids a clone.
            let prev = self.cache[e - 2]
                .take()
                .expect("previous epoch present in order");
            let next = self.run_epoch(e, &prev);
            self.cache[e - 2] = Some(prev);
            self.cache[e - 1] = Some(next);
        }
    }

    /// Run one epoch as a pure function of `(prev, config, seeds[epoch-1])`. Epochs
    /// 2/3/5 are the accelerated analytic kernels (their `duration` knob already
    /// scales them); Epoch 4 is the hydrosphere (the water-cycle join lands here);
    /// Epoch 6 is the iterative erosion pass; Epochs 7-9 are pass-through stubs
    /// (the heightmap-strata layers of the next iteration) so the 9-slot timeline is
    /// exercised end to end.
    fn run_epoch(&self, epoch: usize, prev: &EpochSnapshot) -> EpochSnapshot {
        let seed = self.seeds[epoch - 1];
        let ctx = EpochCtx {
            tables: &self.tables,
            dirs: &self.sphere.dirs,
            neighbors: &self.sphere.neighbors,
            seed,
        };
        let (mut e2, e3, e4, e5, e6) = build_transforms(&self.config);
        // Cross-hex records persist forward once formed (plates from E3, basins from
        // E6); the accelerated epochs don't move them.
        let mut plates = prev.plates.clone();
        let mut watersheds = prev.watersheds.clone();
        let mut steps = 1u32;
        // Global thermal state carried on the snapshot — the continuous-sim master
        // clock. Unchanged by default (frozen forward from the prior epoch); the molten
        // (E2) and tectonic (E3) arms cool it and record where it landed.
        let mut temperature = prev.temperature;

        let mut cells: Vec<HexState> = match epoch {
            2 => {
                // Molten dynamics ON THE COOLING CLOCK: the planet cools from molten by
                // Newtonian decay, its pace set by its own radiogenic content (K/U slow
                // it). Mantle convection self-organizes a heat field from the material
                // buoyancy and convects the composition into its cells — the stir is
                // fierce while molten and gentle as it cools (vigor ∝ T, mean-preserving
                // so the default total stir is unchanged). The density differentiation
                // then firms a crust, completing only as far as the melt stayed hot
                // (`settle` from the clock, retiring the raw `duration` fraction). The
                // scrubber can hold the convection at an earlier step (0 = raw buoyancy).
                let mut molten = prev.cells.clone();
                let full = e2.duration.max(1) * MOLTEN_STEPS_PER_DURATION;
                steps = self.epoch2_steps.map(|s| s.min(full)).unwrap_or(full);
                let k = cooling::cooling_k(&prev.cells);
                let vigor = cooling::convection_vigor(k, full);
                flicker_worldgen::run_molten_convection_cooling(&mut molten, &ctx, steps, &vigor);
                e2.settle_override = Some(cooling::differentiation_settle(k, full));
                temperature = cooling::temperature_at(k, steps);
                e2.apply(&ctx, &molten)
            }
            3 => {
                // HEAT-DRIVEN plate tectonics. The plate partition is seeded from Epoch 2's
                // **convection cells** (upwelling maxima → emergent, seed-varied plate
                // count, not a hard-coded 8), and the plate **motion is the per-cell
                // convection flow field** (`convection_flow`, down the heat gradient) — a
                // curved, spatially-varying velocity, so the conveyor transports crust
                // along curved paths and the seams/belts come out **curved** instead of the
                // old straight spines. `drift_plates` moves the plates as coherent units
                // over that flow; isostasy/deformation then run on the tracked partition +
                // the same flow. One-shot boundary orogeny + analytic hotspots in `apply`
                // stay OFF; the iterative sims own the deformation.
                let (h_count, h_uplift, mtn, rift, thr, e3_dur) = (
                    e3.hotspots,
                    e3.hotspot_uplift,
                    e3.mountain_uplift,
                    e3.rift_drop,
                    e3.boundary_threshold,
                    e3.duration,
                );
                let full = e3_dur.max(1) * TECTONIC_STEPS_PER_DURATION;
                steps = self.epoch3_steps.map(|s| s.min(full)).unwrap_or(full);

                // TECTONICS ONSET on the cooling clock: the same convection that stirred
                // the melt only begins moving the crust as rigid plates once the planet
                // has cooled below the crust solidus AND firmed a coherent lid. The clock
                // continues cooling from the end of the molten era (`molten_steps`), so
                // `drift_steps` = the scrub position minus the onset delay — plates move
                // only for the post-onset steps. If the onset never arrives (still molten
                // through the window, or the crust never firmed a lid), `drift_steps` is 0
                // and the world keeps its undrifted lid + isostasy: a valid stagnant world.
                let k = cooling::cooling_k(&prev.cells);
                let molten_steps = self.epoch2_full_steps();
                let lid_firm = cooling::coherent_lid(&prev.cells);
                let drift_steps =
                    match cooling::tectonics_onset_delay(k, molten_steps, full, lid_firm) {
                        Some(delay) => steps.saturating_sub(delay),
                        None => 0,
                    };
                temperature = cooling::temperature_at(k, molten_steps + steps);

                // Convection flow (curved plate motion) + heat-seeded partition, both from
                // the frozen Epoch-2 heat field.
                let heat: Vec<f32> = prev.cells.iter().map(|c| c.heat).collect();
                let flow = flicker_worldgen::convection_flow(&ctx, &heat);
                let partition0 = e3.partition_heat(&ctx, &prev.cells, &heat, &flow);

                let (mut drifted, partition) =
                    e3.drift_plates(&ctx, &prev.cells, partition0, &flow, drift_steps);
                // De-scatter the drifted crust thickness before isostasy: the whole-cell
                // conveyor resolves each convergence/ridge per cell, so the first advance
                // otherwise imprints the flow's convergence lines as single-cell relief
                // spikes (the "noise field" that pops the instant transport starts). This
                // spreads them into broad ranges/basins; the sharp seams (run_orogeny,
                // below) are added from the flow afterward, so they stay crisp.
                if drift_steps > 0 {
                    flicker_worldgen::smooth_crust_thickness(
                        &mut drifted,
                        &ctx,
                        CRUST_DESCATTER_PASSES,
                    );
                }
                let buoyancy = flicker_worldgen::buoyancy_ranks(&drifted);
                let mut base3 = e3;
                base3.hotspots = 0;
                base3.mountain_uplift = 0.0; // one-shot boundary orogeny OFF — run_orogeny owns deformation
                base3.rift_drop = 0.0;
                // Softened isostasy + the Airy crust-pile: the piled (collided) crust is
                // the single convergent-mountain source, and blending the absolute
                // thickness into the base tames the rank-stretched thin-spine relief.
                base3.isostasy_abs = ISOSTASY_ABS_BLEND;
                base3.crust_pile = mtn;
                // Isostasy/deformation on the TRACKED partition + the convection flow (the
                // curved per-cell motion → curved seams).
                let mut cells = base3.apply_with(&ctx, &drifted, &partition, &flow);
                if drift_steps > 0 {
                    flicker_worldgen::run_orogeny(
                        &mut cells,
                        &ctx,
                        &partition.plate,
                        &flow,
                        &buoyancy,
                        mtn,
                        rift,
                        thr,
                        drift_steps,
                    );
                    flicker_worldgen::run_tectonic_hotspots(
                        &mut cells,
                        &ctx,
                        &partition.plate,
                        &flow,
                        h_count,
                        drift_steps,
                        h_uplift,
                    );
                    // Pre-hydraulic weathering: a thick proto-atmosphere + mass-wasting
                    // grinds the sharpest relief back down as fast as the tectonics build
                    // it, so hotspot streaks and swept boundary ridges don't run away over
                    // deep time. Weathers at the tectonic step rate → scrubs with the
                    // iteration slider (pristine at t=0, progressively softened forward).
                    flicker_worldgen::run_protoatmospheric_erosion(
                        &mut cells,
                        &ctx,
                        WEATHER_TALUS,
                        WEATHER_RATE,
                        drift_steps,
                    );
                }
                plates = partition.plates;
                cells
            }
            4 => {
                // Continued weathering carries into the early hydrosphere, THEN the
                // static endowment + the iterative water cycle. Weathering the inherited
                // relief first means sea level (binary-searched in `apply`) settles on
                // the softened terrain, not the raw tectonic spikes.
                let mut base = prev.cells.clone();
                let weather = e4.duration.max(1) * WEATHER_STEPS_PER_DURATION;
                flicker_worldgen::run_protoatmospheric_erosion(
                    &mut base,
                    &ctx,
                    WEATHER_TALUS,
                    WEATHER_RATE,
                    weather,
                );
                // Static hydrosphere endowment, then the iterative water cycle —
                // atmospheric vapour transport + orographic rain + runoff — which
                // rewrites `precipitation` as the emergent (rain-shadowed) field.
                let mut cells = e4.apply(&ctx, &base);
                let sweeps = e4.duration.max(1) * WATER_SWEEPS_PER_DURATION;
                flicker_worldgen::run_water_cycle(&mut cells, &ctx, sweeps);
                steps = sweeps;
                cells
            }
            5 => {
                // Mineralization: the analytic base sets the hydrothermal field +
                // microbial life (greedy path-trace OFF); then the ITERATIVE
                // percolation grows the ore-vein *concentration field* — fluid
                // leaches at hot sources, diffuses along the fault network, and
                // precipitates, so branching veins emerge from the transport rather
                // than a hand-walked line. The former (below) then turns that field
                // into conserved ore compounds; the harvestable-ore guarantee runs
                // once formation is complete (E6).
                let hp = flicker_worldgen::HydrothermalParams {
                    vein_threshold: e5.vein_threshold,
                    metal_density_min: e5.metal_density_min,
                    max_sources: e5.max_veins,
                };
                let e5_dur = e5.duration;
                let mut base5 = e5; // greedy path-trace off; veins grow iteratively
                base5.max_veins = 0;
                let mut cells = base5.apply(&ctx, &prev.cells);
                steps = e5_dur.max(1) * HYDRO_STEPS_PER_DURATION;
                flicker_worldgen::run_hydrothermal_veins(&mut cells, &ctx, &hp, steps);
                // Weathering keeps grinding through the mineralization era, right up to
                // Epoch 6's full hydraulic erosion taking over.
                let weather = e5_dur.max(1) * WEATHER_STEPS_PER_DURATION;
                flicker_worldgen::run_protoatmospheric_erosion(
                    &mut cells,
                    &ctx,
                    WEATHER_TALUS,
                    WEATHER_RATE,
                    weather,
                );
                cells
            }
            6 => {
                steps = e6.duration.max(1);
                let cells = e6.apply(&ctx, &prev.cells);
                watersheds = flicker_worldgen::watersheds(&cells);
                cells
            }
            _ => prev.cells.clone(), // 7,8,9 — heightmap-strata stubs
        };

        // The master clock keeps running past tectonics. The molten (E2) and tectonic
        // (E3) arms already cooled T and recorded where it landed; the later epochs used
        // to freeze it. Now they continue the same Newtonian decay along the shared
        // cooling axis, so T falls molten→cold across the *whole* history instead of
        // stopping at E3 — the timeline is one heat-loss clock. Their physics is
        // untouched (T is recorded, not yet gating them); each becomes a real T-gated
        // onset in later slices.
        if epoch >= 4 {
            let k = cooling::cooling_k(&prev.cells);
            temperature = cooling::temperature_at(k, self.cool_step_end(epoch));
        }

        // Material-pipeline stage 2: form this epoch's compounds from the elements
        // (and deliver water at the hydrosphere) — the second ledger.
        let chem = ChemistryParams {
            water_delivery: self.config.get("e4_water_delivery"),
            ..ChemistryParams::default()
        };
        self.former.form_for_epoch(&mut cells, epoch as u8, &chem);

        // Gameplay backstop: once all formation epochs are done, guarantee every
        // curated **harvestable ore** reached a mineable vein somewhere (force a
        // small conserved seam for any the physics missed). Replaces the old
        // every-element node guarantee — not every element needs a vein.
        if epoch == 6 {
            crate::nodes::ensure_ore_veins(&mut cells, &self.tables);
        }

        let conserved_mass = cells.iter().map(|c| c.composition.total()).sum();
        EpochSnapshot {
            epoch: epoch as u8,
            cells,
            plates,
            watersheds,
            temperature,
            provenance: Provenance {
                epoch: epoch as u8,
                seed,
                steps,
                conserved_mass,
            },
        }
    }

    /// The Epoch-1 seed layer (t=0 composition scatter) — cheap, closed-form, no
    /// neighbours. Rebuilt whenever the mix, base seed, or grid changes.
    fn build_seed_layer(&self) -> EpochSnapshot {
        let e1p = build_epoch1(&self.config, &self.params);
        let e1 = Epoch1::new(&self.tables, e1p, self.seeds[0]);
        let cells: Vec<HexState> = self
            .sphere
            .dirs
            .iter()
            .map(|&d| HexState::new(e1.seed_hex(d)))
            .collect();
        let conserved_mass = cells.iter().map(|c| c.composition.total()).sum();
        EpochSnapshot {
            epoch: 1,
            cells,
            plates: Vec::new(),
            watersheds: Vec::new(),
            // The planet is born molten — the master clock starts at its hot end.
            temperature: cooling::T_MOLTEN,
            provenance: Provenance {
                epoch: 1,
                seed: self.seeds[0],
                steps: 1,
                conserved_mass,
            },
        }
    }

    // ── edit verbs (each does the locked forward-only invalidation) ────────────

    /// Set a lever to a clamped value and invalidate its epoch forward. Editing an
    /// Epoch-1 lever (including the `ab_<symbol>` mix) reseeds the whole chain.
    pub fn set_lever(&mut self, id: &str, value: f64) {
        self.config.set_clamped(id, value, &self.params);
        let epoch = self.params.lever(id).map(|l| l.epoch as usize).unwrap_or(1);
        self.invalidate_from(epoch);
    }

    /// Reseed one epoch: advance its seed, re-derive the later seeds, jitter that
    /// epoch's knobs within their bands, and invalidate it forward — leaving earlier
    /// epochs byte-identical. `epoch` is 1-indexed.
    pub fn reseed(&mut self, epoch: usize) {
        let e = epoch.clamp(1, WORLD_EPOCHS);
        self.seeds[e - 1] = next_seed(self.seeds[e - 1]);
        for k in e..WORLD_EPOCHS {
            self.seeds[k] = next_seed(self.seeds[k - 1]);
        }
        mutate_epoch(&mut self.config, e as u8, self.seeds[e - 1], &self.params);
        self.invalidate_from(e);
    }

    /// Reset the base seed (a whole new world) and invalidate everything.
    pub fn set_seed(&mut self, base: u64) {
        self.config.seed = base;
        self.seeds = seed_chain(base);
        self.invalidate_all();
    }

    /// Change the grid frequency — rebuilds the topology and invalidates all epochs.
    pub fn set_freq(&mut self, freq: u32) {
        self.config.freq = freq.clamp(MIN_FREQ, MAX_FREQ);
        let (sphere, outlines) = icosphere_with_outlines(self.config.freq);
        self.sphere = sphere;
        self.outlines = outlines;
        self.invalidate_all();
    }

    /// The full Epoch-3 tectonic iteration count at the current `e3_duration` — the
    /// top of the viewer's iteration scrubber.
    pub fn epoch3_full_steps(&self) -> u32 {
        (self.config.get("e3_duration").round().max(1.0) as u32) * TECTONIC_STEPS_PER_DURATION
    }

    /// Real time (million years) one Epoch-3 tectonic iteration represents — plate
    /// speed × hex size. Lets the viewer label the scrubber in geological time.
    pub fn epoch3_my_per_step(&self) -> f32 {
        flicker_worldgen::MY_PER_TECTONIC_STEP
    }

    /// The **cooling-clock** coefficient `k` for the current world — the per-step
    /// Newtonian decay rate, set by the (conserved) composition's radiogenic content.
    /// Read off the always-present Epoch-1 seed layer (composition is invariant, so it
    /// is the same at every epoch). Lets the viewer plot the logarithmic heat-loss
    /// curve `T(step) = cooling::temperature_at(k, step)` across the molten→tectonic
    /// cooling span (`0..epoch2_full_steps()+epoch3_full_steps()`).
    pub fn cooling_k(&self) -> f32 {
        cooling::cooling_k(&self.cache[0].as_ref().expect("seed layer present").cells)
    }

    /// Cooling steps epoch `e` (1-indexed) advances the shared **heat-loss clock**.
    /// Epoch 1 is the molten seed (0 — `T` starts at its hot end); E2/E3 use their
    /// iteration counts; the post-tectonic epochs advance at [`COOL_STEPS_PER_DURATION`]
    /// per duration unit. This is the one continuous cooling axis the timeline scrubs —
    /// every epoch, not just the molten/tectonic pair.
    pub fn epoch_cool_steps(&self, e: usize) -> u32 {
        match e {
            0 | 1 => 0,
            2 => self.epoch2_full_steps(),
            3 => self.epoch3_full_steps(),
            _ => {
                (self.config.get(&format!("e{e}_duration")).round().max(1.0) as u32)
                    * COOL_STEPS_PER_DURATION
            }
        }
    }

    /// Cumulative cooling step at the **start** of epoch `e` (the clock position when it
    /// begins; `0` for E1/E2). The per-epoch boundaries the viewer lays the timeline
    /// segments and onset markers against.
    pub fn cool_step_before(&self, e: usize) -> u32 {
        (1..e).map(|j| self.epoch_cool_steps(j)).sum()
    }

    /// Cumulative cooling step at the **end** of epoch `e` — where its settled thermal
    /// state sits on the clock.
    pub fn cool_step_end(&self, e: usize) -> u32 {
        self.cool_step_before(e) + self.epoch_cool_steps(e)
    }

    /// Total cooling steps across the whole history — the span of the heat-loss timeline
    /// (`0..cooling_total_steps()`).
    pub fn cooling_total_steps(&self) -> u32 {
        (1..=WORLD_EPOCHS).map(|j| self.epoch_cool_steps(j)).sum()
    }

    /// Absolute cooling step at which **plate tectonics begin** — the molten-era length
    /// ([`epoch2_full_steps`](Self::epoch2_full_steps)) plus the onset delay (the crust
    /// has cooled below the solidus **and** firmed a coherent lid). `None` = the onset
    /// never arrives within the window (a stagnant, still-molten world). The viewer
    /// marks it on the heat-loss scrubber, so a delayed onset is visible as drift only
    /// beginning partway through the tectonic era.
    pub fn tectonics_onset_step(&mut self) -> Option<u32> {
        let molten = self.epoch2_full_steps();
        let tectonic = self.epoch3_full_steps();
        let k = self.cooling_k();
        let lid = cooling::coherent_lid(&self.snapshot(2).cells);
        cooling::tectonics_onset_delay(k, molten, tectonic, lid).map(|d| molten + d)
    }

    /// Scrub Epoch 3's tectonic simulation: run only `steps` iterations of drift +
    /// orogeny (0 = the undeformed partition + isostasy, `None` / full = the whole
    /// run). Invalidates Epoch 3 forward so the next `snapshot` shows that step.
    pub fn set_epoch3_steps(&mut self, steps: Option<u32>) {
        if self.epoch3_steps == steps {
            return;
        }
        self.epoch3_steps = steps;
        self.invalidate_from(3);
    }

    /// The full Epoch-2 molten-convection iteration count at the current `e2_duration`
    /// — the top of the viewer's convection scrubber.
    pub fn epoch2_full_steps(&self) -> u32 {
        (self.config.get("e2_duration").round().max(1.0) as u32) * MOLTEN_STEPS_PER_DURATION
    }

    /// Scrub Epoch 2's molten convection: run only `steps` iterations (0 = the raw
    /// buoyancy seed before any convecting, `None` / full = the settled heat cells).
    /// Invalidates Epoch 2 forward so the next `snapshot` shows that step.
    pub fn set_epoch2_steps(&mut self, steps: Option<u32>) {
        if self.epoch2_steps == steps {
            return;
        }
        self.epoch2_steps = steps;
        self.invalidate_from(2);
    }

    /// Invalidate `epoch..=9` (and rebuild the Epoch-1 seed layer if epoch 1 is hit).
    fn invalidate_from(&mut self, epoch: usize) {
        let e = epoch.clamp(1, WORLD_EPOCHS);
        for slot in self.cache.iter_mut().skip(e - 1) {
            *slot = None;
        }
        if e <= 1 {
            self.cache[0] = Some(self.build_seed_layer());
        }
    }

    /// Invalidate every epoch and rebuild the seed layer (topology/base-seed change).
    fn invalidate_all(&mut self) {
        for slot in self.cache.iter_mut() {
            *slot = None;
        }
        self.cache[0] = Some(self.build_seed_layer());
    }

    /// Force-compute every epoch (used before a full `.epoch` capture).
    pub fn realize_all(&mut self) {
        self.snapshot(WORLD_EPOCHS);
    }

    /// Capture the whole world — realise all nine epochs and bundle the recipe +
    /// snapshots into an [`EpochFile`] ready to `save` to a `.epoch`.
    pub fn capture(&mut self, comment: impl Into<String>) -> crate::epochfile::EpochFile {
        self.realize_all();
        let snapshots = self
            .cache
            .iter()
            .filter_map(|c| c.clone())
            .collect::<Vec<_>>();
        crate::epochfile::EpochFile::new(self.config.clone(), snapshots, comment)
    }

    /// Restore an engine from a captured recipe + snapshots: rebuild the topology
    /// from the recipe's freq and seed the cache with the stored snapshots. Any
    /// epoch missing from `snapshots` recomputes on demand.
    pub fn restore(
        tables: Tables,
        params: GeneratorParams,
        config: WorldConfig,
        snapshots: Vec<EpochSnapshot>,
    ) -> Self {
        let mut engine = Self::new(tables, params, config);
        for snap in snapshots {
            let idx = snap.epoch as usize;
            if (1..=WORLD_EPOCHS).contains(&idx) && snap.len() == engine.sphere.len() {
                engine.cache[idx - 1] = Some(snap);
            }
        }
        engine
    }
}
