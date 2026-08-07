//! The **rock tier**: Element → Compound (mineral) → *Rock*.
//!
//! A rock is a **modal mixture of minerals, not a stoichiometric compound** —
//! which is exactly why granite is defined as "coarse-grained igneous rock" with
//! no formula. It has none. It never did. So a rock row names the minerals it is
//! made of and the proportions they occur in, and the minerals themselves are
//! ordinary compounds in the one compound registry.
//!
//! # What the simulation actually wants from this
//!
//! **Erosional resistance** — how well a rock stands up to weather. On a scale of
//! its own (`0` = an evaporite that strips instantly, `1` = a quartzite that
//! stands as a ridge forever), deliberately **not** Mohs hardness, which measures
//! scratching and says almost nothing about whether a river can carry a rock away.
//!
//! This is the number the outcrop model queries, and everything about differential
//! erosion depends on it existing: a landscape of one uniform material erodes
//! evenly and then, once normalised, has not changed shape at all. The *contrast*
//! is the mechanism. A hard intrusion in soft sediment is what leaves a ridge
//! standing when the plain around it has worn down.

use std::collections::HashMap;

use serde::Deserialize;

/// One rock: a name, the minerals it is modally made of, and how well it lasts.
#[derive(Clone, Debug, Deserialize)]
pub struct RockDef {
    /// Stable slug (`"granite"`).
    pub id: String,
    /// Display name (`"Granite"`), and the key a modal map refers to a rock by.
    pub name: String,
    /// Coarse family (`"igneous_intrusive"`, `"sedimentary_clastic"`, …).
    #[serde(default)]
    pub class: String,
    /// Modal mineralogy: compound **name** → mass fraction. Keys are exact
    /// compound names from the compound registry, so a typo is findable.
    #[serde(default)]
    pub modal: HashMap<String, f32>,
    /// How well this rock resists being worn away and carried off — `0` strips
    /// instantly, `1` stands as a ridge forever. Its own scale, not Mohs.
    #[serde(default)]
    pub erosional_resistance: f32,
    /// Whether the simulation needs this rock to exist at all.
    #[serde(default)]
    pub sim_required: bool,
    /// Free-text provenance from the catalog. Optional, and explicitly `null` for
    /// some rows — so it is an `Option`, not a defaulted `String` (which rejects a
    /// literal null and fails the whole catalog load over a blank comment).
    #[serde(default)]
    pub note: Option<String>,
}

impl RockDef {
    /// The minerals this rock is made of, heaviest fraction first — the order a
    /// classifier wants to match in.
    pub fn modal_sorted(&self) -> Vec<(&str, f32)> {
        let mut v: Vec<(&str, f32)> = self.modal.iter().map(|(k, &f)| (k.as_str(), f)).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(b.0)));
        v
    }
}
