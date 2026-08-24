//! Prop VARIATION SET — a small descriptor grouping interchangeable prop variants for randomized
//! placement (e.g. three grass tufts scattered as a field). This is the "does the system support
//! N variations of one thing" construct: the set names its member props (by their
//! `props/<name>/<name>.json` asset name) each with a spawn WEIGHT, and hands out a weighted-random
//! pick.
//!
//! Format: `flicker.propset` v1 — one gz-at-rest JSON per set (`<Name>.set.json.gz`), living beside
//! its member prop folders under `props/`. The members are ordinary `flicker.rig` props; the set
//! only REFERENCES them, so a variant can be shared across sets and promoted independently.
//!
//! Scope: this is the set + weighted pick ONLY. Where instances land — scatter over terrain, the
//! above-water mask, the near "LOD field" — is the placement stage that CONSUMES a `PropSet`, and
//! is deliberately not represented here.

use std::path::Path;

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

fn default_format() -> String {
    "flicker.propset".to_string()
}
fn default_version() -> u32 {
    1
}

/// One interchangeable member of a [`PropSet`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropVariant {
    /// The member prop's asset name — resolves to `props/<prop>/<prop>.json` in the content tree.
    pub prop: String,
    /// Relative spawn frequency. Only RATIOS matter (the weights need not sum to 1). Must be > 0.
    pub weight: f32,
}

/// A named set of interchangeable prop variants, chosen from at random by [`PropSet::pick`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropSet {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    pub variants: Vec<PropVariant>,
}

impl PropSet {
    /// A set with the canonical format/version header.
    pub fn new(name: impl Into<String>, variants: Vec<PropVariant>) -> Self {
        Self {
            format: default_format(),
            version: default_version(),
            name: name.into(),
            variants,
        }
    }

    /// Load a set from its LOGICAL path (`…/<Name>.set.json`; the `.gz` twin is read first, raw is
    /// the dev/test fallback — the shared package seam).
    pub fn load(path: &Path) -> Result<Self> {
        let text = crate::package::read_text(path)?;
        let set: PropSet = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parsing prop set {}: {e}", path.display()))?;
        set.validate()?;
        Ok(set)
    }

    /// Write the set gz-at-rest to its logical path (emits `<path>.gz`), creating parent dirs.
    pub fn write(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        crate::package::write_text(path, &text)?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.variants.is_empty(),
            "prop set '{}' has no variants",
            self.name
        );
        ensure!(
            self.variants.iter().all(|v| v.weight > 0.0),
            "prop set '{}' has a variant with a non-positive weight",
            self.name
        );
        Ok(())
    }

    /// The denominator for [`pick`](Self::pick): the sum of all variant weights.
    pub fn total_weight(&self) -> f32 {
        self.variants.iter().map(|v| v.weight).sum()
    }

    /// Pick a variant's prop name by weight. `r` is a uniform sample in `[0, 1)` — `fastrand::f32()`
    /// at runtime, or a deterministic per-cell hash (`flicker-render`'s `hash01`) for reproducible
    /// placement. Walks the weighted variants the way `flicker-jiggle`'s `rand_tier` does; never
    /// panics for a set that passed [`validate`](Self::validate) (non-empty, positive weights).
    pub fn pick(&self, r: f32) -> &str {
        let mut acc = self.total_weight() * r.clamp(0.0, 1.0);
        for v in &self.variants {
            acc -= v.weight;
            if acc < 0.0 {
                return &v.prop;
            }
        }
        // Float slop at r ≈ 1.0 lands here; the last variant is the correct bucket.
        &self
            .variants
            .last()
            .expect("validate() guarantees at least one variant")
            .prop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grass() -> PropSet {
        PropSet::new(
            "GrassField",
            vec![
                PropVariant {
                    prop: "Grass-Tall".into(),
                    weight: 1.0,
                },
                PropVariant {
                    prop: "Grass-Medium".into(),
                    weight: 1.0,
                },
                PropVariant {
                    prop: "Grass-Short".into(),
                    weight: 2.0,
                },
            ],
        )
    }

    #[test]
    fn pick_lands_in_the_weighted_buckets() {
        let s = grass(); // total weight 4: [0,0.25)->Tall, [0.25,0.5)->Medium, [0.5,1)->Short
        assert_eq!(s.pick(0.0), "Grass-Tall");
        assert_eq!(s.pick(0.10), "Grass-Tall");
        assert_eq!(s.pick(0.30), "Grass-Medium");
        assert_eq!(s.pick(0.60), "Grass-Short");
        assert_eq!(s.pick(0.999), "Grass-Short");
        // r is clamped, so out-of-range never panics and stays in-bounds.
        assert_eq!(s.pick(1.0), "Grass-Short");
        assert_eq!(s.pick(-1.0), "Grass-Tall");
    }

    #[test]
    fn pick_distribution_tracks_weights() {
        let s = grass();
        let (n, mut short) = (10_000u32, 0u32);
        for i in 0..n {
            if s.pick(i as f32 / n as f32) == "Grass-Short" {
                short += 1;
            }
        }
        // Short has weight 2 of 4 total → ~50%.
        let frac = short as f32 / n as f32;
        assert!((frac - 0.5).abs() < 0.02, "short fraction {frac} ~ 0.5");
    }

    #[test]
    fn round_trips_through_the_gz_seam() {
        let dir = std::env::temp_dir().join("flicker_content_propset_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("GrassField.set.json");
        let set = grass();
        set.write(&path).expect("write");
        assert!(
            dir.join("GrassField.set.json.gz").is_file(),
            "gz-at-rest twin written"
        );
        let back = PropSet::load(&path).expect("load");
        assert_eq!(back, set, "round-trips byte-for-byte through serde + gz");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_rejects_empty_and_nonpositive() {
        assert!(PropSet::new("Empty", vec![]).write(Path::new("/dev/null")).is_err());
        let bad = PropSet::new(
            "Bad",
            vec![PropVariant {
                prop: "X".into(),
                weight: 0.0,
            }],
        );
        assert!(bad.validate().is_err(), "zero weight is refused");
    }
}
