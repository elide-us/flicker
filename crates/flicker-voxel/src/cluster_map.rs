//! Multi-cluster storage keyed by [`ClusterId`].
//!
//! Holds the in-memory residency for a set of clusters. For this step
//! the map is small (a fixed 3×3 field, 9 entries), all at LOD 0, and
//! is built once at startup with no eviction. Later steps will
//! reintroduce dirty tracking, JIT mesh management, and worker-pool
//! population — this module is deliberately minimal so those
//! evolutions can land without touching the addressing or storage
//! shape.
//!
//! # Storage shape
//!
//! `HashMap<ClusterId, Cluster>`. `Cluster` is large and intentionally
//! non-`Clone` (see [`crate::cluster`]), so entries are moved in via
//! [`insert`] and read by reference via [`get`]. There is no remove —
//! Step 3 builds the map once and never mutates it after generation.
//! Removal can be added when a use case appears.
//!
//! # Why the address is a `ClusterId` and not a tuple
//!
//! The bit-pack encodes LOD as a first-class field. Two clusters that
//! differ only in LOD are distinct entries; the map distinguishes
//! them without the caller needing to derive a composite key. When
//! Step 4 adds LOD residency, the same `HashMap<ClusterId, …>` shape
//! just gains entries at higher LODs.
//!
//! [`insert`]: ClusterMap::insert
//! [`get`]: ClusterMap::get

use std::collections::HashMap;

use crate::cluster::Cluster;
use crate::cluster_id::ClusterId;

/// In-memory residency for a set of clusters.
#[derive(Debug, Default)]
pub struct ClusterMap {
    clusters: HashMap<ClusterId, Cluster>,
}

impl ClusterMap {
    /// An empty map. Equivalent to `ClusterMap::default()`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of resident clusters.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.clusters.len()
    }

    /// `true` iff no clusters are resident.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// Insert a cluster at the given id. Returns the previous cluster
    /// at that id, if any. (Step 3 never overwrites — the map is
    /// populated once at startup — but the signature mirrors
    /// `HashMap::insert` so callers do not need to special-case the
    /// "first-time insert" path.)
    pub fn insert(&mut self, id: ClusterId, cluster: Cluster) -> Option<Cluster> {
        self.clusters.insert(id, cluster)
    }

    /// Borrow the cluster at the given id, if present.
    #[inline]
    #[must_use]
    pub fn get(&self, id: ClusterId) -> Option<&Cluster> {
        self.clusters.get(&id)
    }

    /// Iterate every `(id, cluster)` pair in storage order
    /// (unspecified, since the underlying `HashMap` does not preserve
    /// any). Useful for the example's "draw every cluster" loop.
    pub fn iter(&self) -> impl Iterator<Item = (ClusterId, &Cluster)> {
        self.clusters.iter().map(|(id, c)| (*id, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(x: u16, z: u16) -> ClusterId {
        ClusterId::new(0, x, 0, z)
    }

    #[test]
    fn new_is_empty() {
        let m = ClusterMap::new();
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
        assert!(m.get(id(0, 0)).is_none());
    }

    #[test]
    fn insert_and_get_round_trip() {
        let mut m = ClusterMap::new();
        let cid = id(2, 3);
        assert!(m.insert(cid, Cluster::empty()).is_none());
        assert_eq!(m.len(), 1);
        assert!(!m.is_empty());
        assert!(m.get(cid).is_some());
        assert!(m.get(id(0, 0)).is_none());
    }

    #[test]
    fn second_insert_returns_previous() {
        let mut m = ClusterMap::new();
        let cid = id(1, 1);
        assert!(m.insert(cid, Cluster::empty()).is_none());
        // A second insert at the same id returns the previous value.
        let prev = m.insert(cid, Cluster::empty());
        assert!(prev.is_some());
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn iter_visits_every_inserted_cluster() {
        let mut m = ClusterMap::new();
        let ids = [id(0, 0), id(1, 0), id(2, 0)];
        for &cid in &ids {
            m.insert(cid, Cluster::empty());
        }
        let mut seen: Vec<ClusterId> = m.iter().map(|(i, _)| i).collect();
        seen.sort_by_key(|i| i.bits());
        let mut want: Vec<ClusterId> = ids.to_vec();
        want.sort_by_key(|i| i.bits());
        assert_eq!(seen, want);
    }
}
