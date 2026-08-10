//! **Populous Bench** — the world as a MAP, built up from nothing.
//!
//! The bench's data core is [`HexMap`], a hex tiling of a sphere where every
//! tile is identified by its own index and carries **no other data at all**.
//! Not terrain, not material, not height, not owner. Each of those is a
//! decision, and a map that does not presume them is the one that can still
//! take any of them.
//!
//! It is a GAME MASTER scene: the realm where a world is authored, distinct
//! from the Developer benches that test the engine and from Dungeon Maker,
//! which builds an adventure inside a world that already exists.
//!
//! The surface is the DEFAULT PAGE ([`ui`]) grown stepwise: the `paged_menu`
//! page/tab control docked into the frame, the three-pane view (panel | RTT |
//! panel) inside its content panel, and the MAP now rendered in the centre
//! viewport — grey tiles, black outlines, the reference lines, a default
//! light, and an orbit camera the entered pane hands the sticks to. No
//! simulation and no data meaning yet: the sphere itself. Every further layer
//! docks into a pane rather than rebuilding the chrome.

mod map;
mod scene;
pub mod ui;

pub use map::{HexMap, TileId, DEFAULT_FREQ, MAX_FREQ, MIN_FREQ};
pub use scene::{scene, PopulousBench};
