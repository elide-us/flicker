//! The emissive colour palette — a bounded, DATA-DRIVEN set of prefab glow colours
//! (`content/data/palettes.json`, `palettes.emissive`).
//!
//! The MaterialBench writes a chosen entry VERBATIM into `recipe.out.emissive` and snaps
//! Roll's random glow to the nearest entry, so emissive colours stay bounded (the
//! material-set budget invariant, MCP `B46FFC37`). Values are the recipe's native colour
//! domain (map byte / 255) with NO colour-space conversion — the same convention every
//! shipped ramp stop and the rolled `hsv()` glow already use. Lives here, beside the sample
//! that shows a glow, so every consumer of the curated set reads the one loader.

use std::path::Path;

/// One prefab glow colour: a style-block id and its RGB in the recipe's domain.
pub struct GlowColor {
    pub id: String,
    pub rgb: [f32; 3],
}

/// The ordered emissive palette. The array INDEX is the identity — swatch order, nav
/// order, and the pick index are all a position in this list.
#[derive(Default)]
pub struct GlowPalette {
    entries: Vec<GlowColor>,
}

impl GlowPalette {
    /// Load `palettes.emissive` from the data dir. An unreadable or malformed file yields
    /// an EMPTY palette (a warn, never a panic): the swatch strip then shows nothing,
    /// which is visibly wrong rather than silently wrong (the materials-index precedent).
    /// There is deliberately NO Rust fallback table — a hardcoded fallback equal to the
    /// shipped data would be a second source of truth that drifts (AEEF2A68).
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("palettes.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("palettes.json unreadable at {}: {e}", path.display());
                return Self::default();
            }
        };
        let json: serde_json::Value = match serde_json::from_str(&text) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("palettes.json is malformed: {e}");
                return Self::default();
            }
        };
        let Some(arr) = json
            .get("palettes")
            .and_then(|p| p.get("emissive"))
            .and_then(|e| e.as_array())
        else {
            tracing::warn!("palettes.json has no palettes.emissive array");
            return Self::default();
        };
        let mut entries = Vec::new();
        for row in arr {
            match (
                row.get("id").and_then(|v| v.as_str()),
                row.get("rgb").and_then(|v| v.as_array()),
            ) {
                (Some(id), Some(rgb)) if rgb.len() == 3 => entries.push(GlowColor {
                    id: id.to_string(),
                    rgb: [
                        rgb[0].as_f64().unwrap_or(0.0) as f32,
                        rgb[1].as_f64().unwrap_or(0.0) as f32,
                        rgb[2].as_f64().unwrap_or(0.0) as f32,
                    ],
                }),
                _ => tracing::warn!("palettes.emissive: skipping a malformed entry"),
            }
        }
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn get(&self, i: usize) -> Option<&GlowColor> {
        self.entries.get(i)
    }
    pub fn iter(&self) -> impl Iterator<Item = &GlowColor> {
        self.entries.iter()
    }

    /// The index of the palette entry nearest `rgb` by squared Euclidean distance in the
    /// stored domain; ties resolve to the LOWER index (strictly-less compare). `None` for
    /// an empty palette.
    pub fn nearest(&self, rgb: [f32; 3]) -> Option<usize> {
        let d2 = |c: &[f32; 3]| {
            let (dr, dg, db) = (c[0] - rgb[0], c[1] - rgb[1], c[2] - rgb[2]);
            dr * dr + dg * dg + db * db
        };
        let mut best: Option<(usize, f32)> = None;
        for (i, e) in self.entries.iter().enumerate() {
            let dist = d2(&e.rgb);
            if best.is_none_or(|(_, b)| dist < b) {
                best = Some((i, dist));
            }
        }
        best.map(|(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repo's shipped palette — `Alpha/content/data/palettes.json`, the file every
    /// consumer loads.
    fn shipped() -> GlowPalette {
        GlowPalette::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data"
        )))
    }

    /// The shipped curated plastic set loads (ruled 2026-08-19): 8 entries, unique
    /// ids, every component in range — and BLACK IS NOT A MEMBER (absence of light,
    /// excluded by the same ruling). Pins the shipped file so the invariant can't
    /// erode to an empty strip — or regrow a black swatch — unnoticed.
    #[test]
    fn the_glow_palette_loads_the_shipped_set() {
        let pal = shipped();
        assert_eq!(pal.len(), 8, "the curated 8-colour emissive palette loads");
        let mut ids: Vec<&str> = pal.iter().map(|c| c.id.as_str()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "swatch ids are unique");
        for c in pal.iter() {
            assert!(
                c.rgb.iter().all(|&x| (0.0..=1.0).contains(&x)),
                "{}: rgb {:?} out of range",
                c.id,
                c.rgb
            );
            assert_ne!(
                c.rgb,
                [0.0, 0.0, 0.0],
                "{}: black is excluded from emissive",
                c.id
            );
        }
    }

    /// `nearest` is exact on a member and picks the closest off-palette colour.
    #[test]
    fn glow_nearest_picks_the_closest_entry() {
        let pal = shipped();
        let orange = pal.iter().position(|c| c.id == "orange").expect("orange");
        assert_eq!(
            pal.nearest(pal.get(orange).unwrap().rgb),
            Some(orange),
            "exact member"
        );
        assert_eq!(
            pal.nearest([0.95, 0.60, 0.10]),
            Some(orange),
            "near-orange snaps to orange"
        );
    }
}
