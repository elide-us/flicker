//! # flicker-greed — "Glitter Greed"
//!
//! A small turn-based gem game, ported from a C++ weekend prototype. This crate
//! is *pure logic and data structures* — there is deliberately no console/stdin
//! loop (that is I/O, and belongs to whatever driver or Lua HUD sits on top).
//!
//! ## The game (derived from the prototype)
//!
//! - There are [`NUM_COLORS`] gem colours (`1..=7`). At the start of a game a
//!   random **target ring** ([`TargetRing`]) orders all colours into a cycle:
//!   an arrow of colour `X` strikes the colour that follows `X` in the ring.
//! - Each [`Player`] owns [`COLLECTIONS_PER_PLAYER`] collection slots. A
//!   [`GemCollection`] holds up to [`COLLECTION_CAPACITY`] gems, **all of the
//!   same colour**, plus a shield of level `0..=`[`MAX_SHIELD`].
//! - A turn is a roll of two d7 ([`Game::roll`]):
//!   - **doubles** → a *shield gem*: the player adds a shield to a slot.
//!   - **otherwise** → the player takes one of the two rolled colours and
//!     places it into a matching (or empty) slot.
//! - Filling a slot to [`COLLECTION_CAPACITY`] **fires an arrow**. The arrow's
//!   target colour is the ring successor of the placed colour, and every slot of
//!   that colour — on *every* player — is hit. Shields absorb first: a level-2
//!   shield blocks outright, a level-1 shield breaks, and an unshielded slot
//!   loses a gem (clearing its colour when it empties).
//!
//! ## Notes on the port
//!
//! The prototype was an unfinished weekend project; a few things were tidied
//! into their evident intent rather than transliterated:
//! - The original stored a redundant per-socket colour array; since every gem in
//!   a slot shares one colour, this is captured by `color` + `count`.
//! - `throw` on an illegal placement becomes a [`GreedError`] `Result`.
//! - Arrow resolution hits *all* matching slots (the C++ `break`-on-shield-2
//!   halted the whole sweep, which was a bug); a level-2 shield now protects only
//!   its own slot. The original also carried a `// TODO: Do not destroy self
//!   gems` — that intent is *not* applied here, so an arrow can still strike the
//!   firing player's own matching slots, matching the code as it actually ran.

#![forbid(unsafe_code)]

/// Number of distinct gem colours (`1..=NUM_COLORS`).
pub const NUM_COLORS: usize = 7;
/// Collection slots each player owns.
pub const COLLECTIONS_PER_PLAYER: usize = 5;
/// Gems that fill a collection and fire an arrow.
pub const COLLECTION_CAPACITY: u8 = 3;
/// Highest shield level a collection can reach.
pub const MAX_SHIELD: u8 = 2;
/// Default number of players (the prototype seated four).
pub const DEFAULT_PLAYERS: usize = 4;

/// Errors returned by illegal moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GreedError {
    /// Collection index is outside `0..COLLECTIONS_PER_PLAYER`.
    #[error("collection {0} is out of range")]
    BadCollection(usize),
    /// The collection already holds [`COLLECTION_CAPACITY`] gems.
    #[error("collection is already full")]
    CollectionFull,
    /// The gem's colour does not match the collection's existing colour.
    #[error("gem colour does not match the collection's colour")]
    ColorMismatch,
    /// A game was requested with zero players.
    #[error("a game needs at least one player")]
    NoPlayers,
}

/// A gem colour in `1..=NUM_COLORS`. Emptiness is modelled with `Option<Color>`,
/// so a `Color` is always a real colour.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Color(u8);

impl Color {
    /// Construct a colour, validating the `1..=NUM_COLORS` range.
    pub fn new(value: u8) -> Option<Self> {
        (1..=NUM_COLORS as u8).contains(&value).then_some(Self(value))
    }

    /// The underlying `1..=NUM_COLORS` value.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Every colour, in ascending order.
    pub fn all() -> [Self; NUM_COLORS] {
        std::array::from_fn(|i| Self(i as u8 + 1))
    }

    /// Roll one random colour (a single d`NUM_COLORS`).
    pub fn random(rng: &mut fastrand::Rng) -> Self {
        Self(rng.u8(1..=NUM_COLORS as u8))
    }
}

/// The outcome of a single hit against a collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitResult {
    /// A level-2 shield blocked the hit; nothing changed.
    Blocked,
    /// A level-1 shield absorbed the hit and dropped to 0.
    ShieldBroken,
    /// An unshielded collection lost a gem.
    GemDestroyed,
    /// The collection matched the colour but held no gem to lose.
    NoGem,
}

/// One player's collection slot: up to [`COLLECTION_CAPACITY`] same-colour gems
/// plus a shield.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GemCollection {
    color: Option<Color>,
    count: u8,
    shield: u8,
}

impl GemCollection {
    /// An empty, unshielded collection.
    pub const fn new() -> Self {
        Self { color: None, count: 0, shield: 0 }
    }

    /// The collection's colour, or `None` while empty.
    pub const fn color(&self) -> Option<Color> {
        self.color
    }

    /// How many gems the collection holds (`0..=COLLECTION_CAPACITY`).
    pub const fn count(&self) -> u8 {
        self.count
    }

    /// The current shield level (`0..=MAX_SHIELD`).
    pub const fn shield(&self) -> u8 {
        self.shield
    }

    /// Whether the collection holds no gems.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether the collection is full (and would fire on the next gem).
    pub const fn is_full(&self) -> bool {
        self.count >= COLLECTION_CAPACITY
    }

    /// Add a gem. Returns `Ok(true)` when this gem fills the collection and
    /// fires an arrow, `Ok(false)` otherwise. Errors if the collection is full
    /// or the colour does not match.
    pub fn add_gem(&mut self, color: Color) -> Result<bool, GreedError> {
        if self.is_full() {
            return Err(GreedError::CollectionFull);
        }
        match self.color {
            Some(existing) if existing != color => return Err(GreedError::ColorMismatch),
            None => self.color = Some(color),
            _ => {}
        }
        self.count += 1;
        Ok(self.count == COLLECTION_CAPACITY)
    }

    /// Raise the shield by one, saturating at [`MAX_SHIELD`].
    pub fn add_shield(&mut self) {
        self.shield = (self.shield + 1).min(MAX_SHIELD);
    }

    /// Resolve one hit against this collection, consuming a shield or a gem.
    pub fn take_hit(&mut self) -> HitResult {
        match self.shield {
            s if s >= MAX_SHIELD => HitResult::Blocked,
            1 => {
                self.shield = 0;
                HitResult::ShieldBroken
            }
            _ => {
                if self.count == 0 {
                    return HitResult::NoGem;
                }
                self.count -= 1;
                if self.count == 0 {
                    self.color = None;
                }
                HitResult::GemDestroyed
            }
        }
    }
}

/// A player and their [`COLLECTIONS_PER_PLAYER`] collection slots.
#[derive(Clone, Debug)]
pub struct Player {
    collections: [GemCollection; COLLECTIONS_PER_PLAYER],
}

impl Default for Player {
    fn default() -> Self {
        Self { collections: [GemCollection::new(); COLLECTIONS_PER_PLAYER] }
    }
}

impl Player {
    /// A player with all slots empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only view of every collection slot.
    pub fn collections(&self) -> &[GemCollection; COLLECTIONS_PER_PLAYER] {
        &self.collections
    }

    /// One collection slot, or `None` if the index is out of range.
    pub fn collection(&self, index: usize) -> Option<&GemCollection> {
        self.collections.get(index)
    }

    fn slot_mut(&mut self, index: usize) -> Result<&mut GemCollection, GreedError> {
        self.collections.get_mut(index).ok_or(GreedError::BadCollection(index))
    }

    /// Add a gem to the given slot; see [`GemCollection::add_gem`].
    pub fn add_gem(&mut self, index: usize, color: Color) -> Result<bool, GreedError> {
        self.slot_mut(index)?.add_gem(color)
    }

    /// Add a shield to the given slot.
    pub fn add_shield(&mut self, index: usize) -> Result<(), GreedError> {
        self.slot_mut(index)?.add_shield();
        Ok(())
    }

    /// The colours currently held across this player's non-empty slots.
    pub fn colors(&self) -> impl Iterator<Item = Color> + '_ {
        self.collections.iter().filter_map(|c| c.color())
    }

    /// Apply an arrow of `target` colour to this player: every slot of that
    /// colour takes a hit. Returns the `(slot index, result)` for each hit.
    pub fn resolve_arrow(&mut self, target: Color) -> Vec<(usize, HitResult)> {
        let mut hits = Vec::new();
        for (index, slot) in self.collections.iter_mut().enumerate() {
            if slot.color() == Some(target) {
                hits.push((index, slot.take_hit()));
            }
        }
        hits
    }
}

/// The random cycle of colours that decides which colour an arrow strikes.
#[derive(Clone, Debug)]
pub struct TargetRing {
    order: [Color; NUM_COLORS],
}

impl TargetRing {
    /// A fresh random ring (a shuffled permutation of every colour).
    pub fn random(rng: &mut fastrand::Rng) -> Self {
        let mut order = Color::all();
        rng.shuffle(&mut order);
        Self { order }
    }

    /// Build a ring from an explicit order, validating that it is a permutation
    /// of all [`NUM_COLORS`] colours.
    pub fn from_order(order: [Color; NUM_COLORS]) -> Option<Self> {
        let mut seen = [false; NUM_COLORS];
        for color in order {
            let slot = &mut seen[(color.get() - 1) as usize];
            if *slot {
                return None;
            }
            *slot = true;
        }
        Some(Self { order })
    }

    /// The colour order, in ring sequence.
    pub fn order(&self) -> &[Color; NUM_COLORS] {
        &self.order
    }

    /// The colour an arrow of `color` strikes — the ring's cyclic successor.
    pub fn target_of(&self, color: Color) -> Color {
        let index = self.order.iter().position(|&c| c == color).unwrap_or(0);
        self.order[(index + 1) % NUM_COLORS]
    }
}

/// The result of rolling the two dice for a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Roll {
    /// Doubles: a shield gem. The player adds a shield to a slot of their choice.
    Shield(Color),
    /// Two colours to choose between; the player takes one and places it.
    Gems { left: Color, right: Color },
}

/// A player's chosen action for their turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Add a shield to `collection` (valid after a [`Roll::Shield`]).
    Shield { collection: usize },
    /// Place a `color` gem into `collection` (one of the [`Roll::Gems`] colours).
    Gem { collection: usize, color: Color },
}

/// One collection struck by an arrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Index of the affected player.
    pub player: usize,
    /// Index of the affected collection slot.
    pub collection: usize,
    /// What the hit did.
    pub effect: HitResult,
}

/// An arrow fired by completing a collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arrow {
    /// The colour of the completed collection that fired.
    pub color: Color,
    /// The colour the arrow strikes (the ring successor of `color`).
    pub target: Color,
    /// Every collection the arrow hit, across all players.
    pub hits: Vec<Hit>,
}

/// What happened when a move was played.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnOutcome {
    /// `Some` when the move completed a collection and fired an arrow.
    pub arrow: Option<Arrow>,
}

/// A full game: the players, the target ring, whose turn it is, and the RNG.
#[derive(Debug)]
pub struct Game {
    players: Vec<Player>,
    ring: TargetRing,
    active: usize,
    rng: fastrand::Rng,
}

impl Game {
    /// Start a game with `num_players`, seeded from system entropy.
    pub fn new(num_players: usize) -> Result<Self, GreedError> {
        Self::from_rng(num_players, fastrand::Rng::new())
    }

    /// Start a game with a fixed seed — the ring layout and every roll are then
    /// reproducible, which is what the tests rely on.
    pub fn with_seed(num_players: usize, seed: u64) -> Result<Self, GreedError> {
        Self::from_rng(num_players, fastrand::Rng::with_seed(seed))
    }

    fn from_rng(num_players: usize, mut rng: fastrand::Rng) -> Result<Self, GreedError> {
        if num_players == 0 {
            return Err(GreedError::NoPlayers);
        }
        let ring = TargetRing::random(&mut rng);
        Ok(Self { players: vec![Player::new(); num_players], ring, active: 0, rng })
    }

    /// Read-only view of all players.
    pub fn players(&self) -> &[Player] {
        &self.players
    }

    /// The game's target ring.
    pub fn ring(&self) -> &TargetRing {
        &self.ring
    }

    /// The index of the player whose turn it is.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Roll the two dice for the active player's turn. Equal dice yield a
    /// [`Roll::Shield`]; otherwise a [`Roll::Gems`] pair to choose from.
    pub fn roll(&mut self) -> Roll {
        let left = Color::random(&mut self.rng);
        let right = Color::random(&mut self.rng);
        if left == right {
            Roll::Shield(left)
        } else {
            Roll::Gems { left, right }
        }
    }

    /// Apply the active player's chosen `mv`, resolve any arrow it fires, and
    /// advance to the next player.
    ///
    /// The caller is responsible for offering only colours the roll produced;
    /// this mirrors the prototype, which trusted its input. On error the active
    /// player is left unchanged so the move can be corrected and retried.
    pub fn play(&mut self, mv: Move) -> Result<TurnOutcome, GreedError> {
        let outcome = match mv {
            Move::Shield { collection } => {
                self.players[self.active].add_shield(collection)?;
                TurnOutcome::default()
            }
            Move::Gem { collection, color } => {
                let fired = self.players[self.active].add_gem(collection, color)?;
                if fired {
                    TurnOutcome { arrow: Some(self.fire_arrow(color)) }
                } else {
                    TurnOutcome::default()
                }
            }
        };
        self.active = (self.active + 1) % self.players.len();
        Ok(outcome)
    }

    /// Resolve an arrow of `color` against every player's matching slots.
    fn fire_arrow(&mut self, color: Color) -> Arrow {
        let target = self.ring.target_of(color);
        let mut hits = Vec::new();
        for (player, board) in self.players.iter_mut().enumerate() {
            for (collection, effect) in board.resolve_arrow(target) {
                hits.push(Hit { player, collection, effect });
            }
        }
        Arrow { color, target, hits }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: u8) -> Color {
        Color::new(v).unwrap()
    }

    #[test]
    fn color_bounds() {
        assert!(Color::new(0).is_none());
        assert!(Color::new(NUM_COLORS as u8 + 1).is_none());
        assert_eq!(Color::new(1).unwrap().get(), 1);
        assert_eq!(Color::all().len(), NUM_COLORS);
    }

    #[test]
    fn collection_fills_and_fires() {
        let mut col = GemCollection::new();
        assert_eq!(col.add_gem(c(3)), Ok(false));
        assert_eq!(col.color(), Some(c(3)));
        assert_eq!(col.add_gem(c(3)), Ok(false));
        assert_eq!(col.add_gem(c(3)), Ok(true)); // fires at capacity
        assert!(col.is_full());
        assert_eq!(col.add_gem(c(3)), Err(GreedError::CollectionFull));
    }

    #[test]
    fn collection_rejects_mismatched_color() {
        let mut col = GemCollection::new();
        col.add_gem(c(2)).unwrap();
        assert_eq!(col.add_gem(c(5)), Err(GreedError::ColorMismatch));
    }

    #[test]
    fn shield_caps_at_max() {
        let mut col = GemCollection::new();
        col.add_shield();
        assert_eq!(col.shield(), 1);
        col.add_shield();
        assert_eq!(col.shield(), MAX_SHIELD);
        col.add_shield();
        assert_eq!(col.shield(), MAX_SHIELD);
    }

    #[test]
    fn take_hit_peels_shield_then_gems() {
        // Level-2 shield blocks and stays.
        let mut col = GemCollection::new();
        col.add_gem(c(4)).unwrap();
        col.add_shield();
        col.add_shield();
        assert_eq!(col.take_hit(), HitResult::Blocked);
        assert_eq!(col.shield(), MAX_SHIELD);
        assert_eq!(col.count(), 1);

        // Level-1 shield breaks, gem preserved.
        let mut col = GemCollection::new();
        col.add_gem(c(4)).unwrap();
        col.add_shield();
        assert_eq!(col.take_hit(), HitResult::ShieldBroken);
        assert_eq!(col.shield(), 0);
        assert_eq!(col.count(), 1);

        // Unshielded: gem destroyed, colour cleared when empty.
        assert_eq!(col.take_hit(), HitResult::GemDestroyed);
        assert_eq!(col.count(), 0);
        assert_eq!(col.color(), None);
    }

    #[test]
    fn ring_is_cyclic_successor() {
        let ring = TargetRing::from_order(Color::all()).unwrap();
        assert_eq!(ring.target_of(c(1)), c(2));
        assert_eq!(ring.target_of(c(7)), c(1)); // wraps around
    }

    #[test]
    fn ring_rejects_non_permutation() {
        let bad = [c(1), c(1), c(3), c(4), c(5), c(6), c(7)];
        assert!(TargetRing::from_order(bad).is_none());
    }

    #[test]
    fn arrow_hits_every_matching_slot() {
        let mut p = Player::new();
        p.add_gem(0, c(3)).unwrap();
        p.add_gem(1, c(3)).unwrap();
        p.add_gem(2, c(5)).unwrap();
        let hits = p.resolve_arrow(c(3));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|(_, r)| *r == HitResult::GemDestroyed));
        assert_eq!(p.collection(2).unwrap().count(), 1); // colour 5 untouched
    }

    #[test]
    fn game_rolls_are_reproducible_and_turns_rotate() {
        let mut g1 = Game::with_seed(DEFAULT_PLAYERS, 1974).unwrap();
        let mut g2 = Game::with_seed(DEFAULT_PLAYERS, 1974).unwrap();
        assert_eq!(g1.ring().order(), g2.ring().order());
        for _ in 0..16 {
            assert_eq!(g1.roll(), g2.roll());
        }
        assert_eq!(g1.active(), 0);
        g1.play(Move::Shield { collection: 0 }).unwrap();
        assert_eq!(g1.active(), 1);
    }

    #[test]
    fn completing_a_collection_fires_an_arrow() {
        // One player keeps the turn (0 + 1) % 1 == 0, so three plays land in the
        // same slot.
        let mut g = Game::with_seed(1, 3).unwrap();
        let color = c(1);
        assert_eq!(g.play(Move::Gem { collection: 0, color }).unwrap().arrow, None);
        assert_eq!(g.play(Move::Gem { collection: 0, color }).unwrap().arrow, None);
        let arrow = g
            .play(Move::Gem { collection: 0, color })
            .unwrap()
            .arrow
            .expect("the third gem fires an arrow");
        assert_eq!(arrow.color, color);
        assert_eq!(arrow.target, g.ring().target_of(color));
    }

    #[test]
    fn zero_players_is_rejected() {
        assert_eq!(Game::with_seed(0, 1).unwrap_err(), GreedError::NoPlayers);
    }
}
