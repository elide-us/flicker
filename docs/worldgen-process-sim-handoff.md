# flicker world-gen — process-sim handoff

**What it is.** An offline planet-generation sim. A planet is a grid of hexes; each hex is a
vertical **column of material layers** holding a conserved element ledger. A per-hex **process
pipeline** ticks the planet forward over deep time and the geology emerges — cooling,
convection, core/crust formation, outgassing, oceans. Viewed live in the `flicker-pocepochs` app.

## Run it
```
cargo run -p flicker-pocepochs   # viewer: Space plays · V cycles material/heat/layer-stack · Up slices the stack
cargo test --workspace           # invariants: mass conservation, re-sim determinism
cargo clippy --workspace
```

## The tick engine — `crates/flicker-worldgen/src/process.rs`
- Each tick iterates every hex and runs the ordered `processes()` collection.
- Every `Process` is the same shape: `applies(&Ctx) -> bool` (gate on the hex's material state)
  then `compute(&Ctx, &mut Vec<Effect>)` — **pure**: reads the frozen state, emits effects,
  mutates nothing.
- `run_tick` is two-pass: **compute** all effects over the frozen world, then **apply** them.
  A process can push mass to a *neighbour* hex; that lands in the apply pass, so mass → pressure
  → temperature cascade on the next tick.
- `Effect`: `Transfer` (conserved element move, same hex or cross-hex), `Deliver` (external
  water), `Compound` (named-species accounting), `SetTemperature`.
- Temperature is **per-hex, in Kelvin** (`HexState.temperature`), starting ~1900 K and cooling
  over billions of years. `cooling::normalized()` gives the 0..1 read for rendering/rates.

## Key files
| File | What |
|---|---|
| `crates/flicker-worldgen/src/process.rs` | the pipeline + all processes |
| `crates/flicker-worldgen/src/layer.rs` | `LayerLedger` / `Layer` — a hex's column |
| `crates/flicker-worldgen/src/classify.rs` | `(composition, temp, pressure) → {phase, material, colour, density}` |
| `crates/flicker-worldgen/src/cooling.rs` | Kelvin thermal constants + `normalized()` |
| `crates/flicker-worldengine/src/sim.rs` | `Simulation` / `World`; `tick_world` rations water + calls `run_tick` |
| `crates/flicker-worldengine/src/habitability.rs` | the life-supporting observer (5 condition gauges) |
| `Alpha/flicker-pocepochs/src/{scene,globe}.rs` | viewer: per-hex physical-thickness layer stack |
| `Alpha/content/data/{periodic_table,compounds,abundance}.json` | element / compound vocabulary |

## Built so far
Per-hex temperature; convection (cross-hex mantle movement); core differentiation; crust
freezing; outgassing (real gas compounds, distilled by temperature); hydrosphere (water delivery
+ condensation); layer classification + physical-thickness rendering; the 5-axis habitability
observer. Tests are invariants only (conservation, determinism) — no fixed-outcome assertions.

## Candidate next objectives
Each is a new `Process` in `process.rs`, or a term added to the `Temperature` process:
- More heat-balance terms on `Temperature`: compression heating from overburden (mass →
  pressure → temperature), latent heat of phase change, conduction between layers.
- A surface-temperature signal (insolation + atmospheric greenhouse) — lights the habitability
  "surface temp" gauge.
- Ocean pH (dissolve SO₂/HCl into the ocean) — lights the "ocean pH" gauge.
- Reducing-atmosphere branch (CH₄/NH₃ when free oxygen is scarce).
- Body/cluster IDs as an accounting overlay: convection nodes, dynamic plates, sky cells.
