# flicker-greed

"Glitter Greed" — the pure turn logic for a small gem-and-dice game. The crate is
rules and data only: it deals colours, resolves turns, and fires arrows, but it has
**no I/O** — no console loop, no rendering, no input handling. A *driver* (game code,
or a Lua HUD) sits on top, shows the board, turns player input into a `Move`, and reads
back the outcome. Everything here is deterministic and testable: seed a `Game` and every
ring layout and dice roll repeats exactly.

> Design of record — why it is shaped this way, the C++-port history, decisions — lives in
> the project's MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:** `fastrand` (the RNG; its `u8`/`shuffle` take `&mut Rng`), `thiserror`
  (the error enum). Nothing else — no flicker engine crates.
- **Used by:** nothing in the shipping build yet. This is a standalone POC library living
  in the root `crates/` tree (reference-only, not compiled into `prism-alpha`); it is
  slated for the `mechanics` cluster once hardened. A driver/HUD is what will consume it.
- **Reads from the content tree:** nothing. No scene files, no themes, no assets.

## The game in 60 seconds

Seven gem colours, numbered `1..=7`. Each player owns five **collection slots**; a slot
holds up to three gems, all the same colour, plus a shield (level 0–2).

A turn is a roll of two d7:

- **Doubles** → a *shield gem*: the player raises the shield on a slot of their choice.
- **Otherwise** → two colours; the player takes **one** and drops it into a slot that is
  empty or already that colour.

Filling a slot to three gems **fires an arrow**. Every game has one random **target ring**
— a fixed cyclic order of all seven colours. An arrow fired by a colour strikes that
colour's *successor in the ring*, and hits every slot of the struck colour **on every
player at once**. At each hit, a shield absorbs first: a level-2 shield blocks and stays,
a level-1 shield breaks to 0, and an unshielded slot loses one gem (its colour clears when
the last gem goes).

That is the whole game. There is no scoring and no win condition — the driver decides when
a match ends.

## Minimal turn

```rust
use flicker_greed::{Game, Move, Roll, DEFAULT_PLAYERS, GreedError};

fn one_turn() -> Result<(), GreedError> {
    let mut game = Game::new(DEFAULT_PLAYERS)?;      // 4 players, random ring + rolls

    // Turning a Roll into a Move is the DRIVER's job — see Sharp edges.
    let mv = match game.roll() {
        Roll::Shield(_rolled)   => Move::Shield { collection: 0 },
        Roll::Gems { left, .. } => Move::Gem { collection: 0, color: left },
    };

    let outcome = game.play(mv)?;                     // apply the move, advance the turn
    if let Some(arrow) = outcome.arrow {
        // A slot filled: arrow.target was struck on every player; arrow.hits lists each.
        let _ = arrow;
    }
    Ok(())
}
```

For reproducible runs (what the tests use) swap `Game::new(n)` for `Game::with_seed(n, seed)`.

## Public API

### Constants

| Const | Value | Meaning |
|---|---|---|
| `NUM_COLORS` | 7 | distinct gem colours, `1..=7` |
| `COLLECTIONS_PER_PLAYER` | 5 | slots each player owns |
| `COLLECTION_CAPACITY` | 3 | gems that fill a slot and fire an arrow |
| `MAX_SHIELD` | 2 | highest shield level a slot can reach |
| `DEFAULT_PLAYERS` | 4 | a convenience default; `Game::new`/`with_seed` still take an explicit count |

### `Color` — a gem colour

A colour is always a real `1..=7` value; **emptiness is modelled as `Option<Color>`**, never
a zero colour.

| Item | Signature | The one thing to know |
|---|---|---|
| `Color::new` | `(u8) -> Option<Color>` | the only constructor; `None` outside `1..=7`, so an invalid colour cannot exist |
| `Color::get` | `(self) -> u8` | the raw `1..=7` value |
| `Color::all` | `() -> [Color; 7]` | every colour, ascending |
| `Color::random` | `(&mut fastrand::Rng) -> Color` | one uniform colour; borrows the RNG mutably |

### `GemCollection` — one slot

Fields are private; use the accessors. A slot's colour is fixed by its first gem and clears
only when the slot empties.

| Item | Signature | The one thing to know |
|---|---|---|
| `new` | `() -> GemCollection` | empty, unshielded (also `Default`) |
| `color` | `(&self) -> Option<Color>` | `None` while empty |
| `count` | `(&self) -> u8` | gems held, `0..=3` |
| `shield` | `(&self) -> u8` | shield level, `0..=2` |
| `is_empty` / `is_full` | `(&self) -> bool` | full = `count >= COLLECTION_CAPACITY` |
| `add_gem` | `(&mut self, Color) -> Result<bool, GreedError>` | `Ok(true)` = this gem filled the slot (an arrow should fire); `Err` = `CollectionFull` or `ColorMismatch` |
| `add_shield` | `(&mut self)` | +1, saturates at `MAX_SHIELD`; never fails |
| `take_hit` | `(&mut self) -> HitResult` | resolves one incoming hit — peels a shield before a gem |

### `Player` — a player and their five slots

| Item | Signature | The one thing to know |
|---|---|---|
| `new` | `() -> Player` | all slots empty (also `Default`) |
| `collections` | `(&self) -> &[GemCollection; 5]` | read-only view of all slots |
| `collection` | `(&self, usize) -> Option<&GemCollection>` | one slot; `None` if the index is out of range |
| `add_gem` | `(&mut self, usize, Color) -> Result<bool, GreedError>` | slot-indexed `GemCollection::add_gem`; `Err(BadCollection)` on a bad index |
| `add_shield` | `(&mut self, usize) -> Result<(), GreedError>` | `Err(BadCollection)` on a bad index |
| `colors` | `(&self) -> impl Iterator<Item = Color>` | colours of the non-empty slots; **may repeat** (two slots can share a colour) |
| `resolve_arrow` | `(&mut self, Color) -> Vec<(usize, HitResult)>` | hits every slot of that colour; returns `(slot index, result)` per hit |

### `TargetRing` — the colour cycle that aims arrows

Always a full permutation of all seven colours, so `target_of` is always well-defined.

| Item | Signature | The one thing to know |
|---|---|---|
| `random` | `(&mut fastrand::Rng) -> TargetRing` | a shuffled permutation of every colour |
| `from_order` | `([Color; 7]) -> Option<TargetRing>` | `None` unless the array is a full permutation of all 7 |
| `order` | `(&self) -> &[Color; 7]` | the ring sequence |
| `target_of` | `(&self, Color) -> Color` | the ring's cyclic successor — the colour an arrow of the given colour strikes |

### `Game` — a whole match

| Item | Signature | The one thing to know |
|---|---|---|
| `new` | `(usize) -> Result<Game, GreedError>` | seeds from system entropy; `Err(NoPlayers)` if count is 0 |
| `with_seed` | `(usize, u64) -> Result<Game, GreedError>` | fixed seed → ring layout and every roll are reproducible |
| `players` | `(&self) -> &[Player]` | read-only view of all boards |
| `ring` | `(&self) -> &TargetRing` | this match's target ring |
| `active` | `(&self) -> usize` | index of the player whose turn it is |
| `roll` | `(&mut self) -> Roll` | two d7 for the active player — doubles → `Shield`, else `Gems` |
| `play` | `(&mut self, Move) -> Result<TurnOutcome, GreedError>` | apply the move, resolve any arrow, advance to the next player; on `Err` the active player is left unchanged so the move can be corrected and retried |

### Turn types

- **`Roll`** — the roll result. `Shield(Color)` on doubles; `Gems { left, right }` otherwise.
- **`Move`** — the player's chosen action. `Shield { collection }` or
  `Gem { collection, color }`. The driver builds this from the `Roll` (see Sharp edges).

### Result types (all fields `pub`)

- **`TurnOutcome`** — `{ arrow: Option<Arrow> }`; `Some` only when the move fired an arrow.
- **`Arrow`** — `{ color, target, hits }`: the completed slot's `color`, the `target` colour
  it struck (ring successor), and `hits` — every collection struck across all players.
- **`Hit`** — `{ player, collection, effect }`: which player, which slot, and the `HitResult`.
- **`HitResult`** — `Blocked` (level-2 shield, nothing changed) · `ShieldBroken` (level-1 → 0)
  · `GemDestroyed` (lost a gem) · `NoGem` (matched the colour but held no gem — see Sharp edges).

### `GreedError`

`BadCollection(usize)` (slot index out of range) · `CollectionFull` · `ColorMismatch`
(gem colour ≠ the slot's colour) · `NoPlayers` (a game needs ≥ 1 player).

## Interactions

**None — pure logic library.** No input signals, no Model keys, no content-tree reads, no
threads or async. A driver/HUD on top translates player input (signals) into `Move` values
and renders `TurnOutcome`; that translation layer is not part of this crate.

## Gates

The 11 unit tests in `src/lib.rs` are the contract:

| Test | Guards |
|---|---|
| `color_bounds` | `Color::new` rejects 0 and > 7; `all()` yields 7 |
| `collection_fills_and_fires` | `add_gem` reports `true` at capacity; a further gem → `CollectionFull` |
| `collection_rejects_mismatched_color` | a second colour into a slot → `ColorMismatch` |
| `shield_caps_at_max` | `add_shield` saturates at `MAX_SHIELD` |
| `take_hit_peels_shield_then_gems` | level-2 blocks, level-1 breaks, then a gem is lost and colour clears when empty |
| `ring_is_cyclic_successor` | `target_of` wraps (7 → 1) |
| `ring_rejects_non_permutation` | `from_order` rejects a non-permutation |
| `arrow_hits_every_matching_slot` | `resolve_arrow` hits all slots of the colour, leaves the rest |
| `game_rolls_are_reproducible_and_turns_rotate` | same seed → same ring + rolls; `play` advances `active` |
| `completing_a_collection_fires_an_arrow` | the filling gem yields an `Arrow` with the right `color`/`target` |
| `zero_players_is_rejected` | `Game::with_seed(0, …)` → `NoPlayers` |

Run: `cargo test -p flicker-greed`.

## Sharp edges

- **The `Roll` → `Move` contract is the driver's, not the engine's.** `Game::play` does not
  check that a `Move` matches the last `Roll` — it does not even store the roll. A
  `Gem` of a colour the roll never offered, or a `Shield` played after a non-doubles roll,
  **succeeds silently** as long as the slot index and colour are structurally valid. The
  rule "take one of the two rolled colours; shield only on doubles" must be enforced in the
  driver; nothing here will catch a mistake.
- **`Roll::Shield(Color)` carries the doubled colour for display only.** The shield lands on
  whichever slot the `Move::Shield { collection }` names, regardless of colour — you do not
  pass the colour into the move, and the shielded slot need not match it.
- **`HitResult::NoGem` never comes from an arrow.** It arises only if you call
  `GemCollection::take_hit` directly on an empty, unshielded slot. The arrow path
  (`resolve_arrow`) selects slots by colour, and an empty slot has no colour, so it is never
  selected.
- **Placement can fail.** `add_gem` returns `ColorMismatch` if the slot already holds another
  colour, or `CollectionFull` at three gems. A slot's colour is set by its first gem and
  cleared only when it empties.
- **A player can hold one colour in more than one slot.** `colors()` may then repeat that
  colour, and a single arrow hits *every* matching slot.
- **`Player::add_gem` returns the "fired" flag but does not resolve the arrow.** Arrows cross
  all players, so resolving them is `Game::play`'s job. Drive turns through `Game` unless you
  are deliberately composing `Player`/`GemCollection` yourself.
- **An arrow can strike the firing player.** The sweep does not exclude the active player; if
  they hold a slot of the struck colour, it takes a hit like anyone else's.
- **No win or end condition.** The driver owns "the match is over".
