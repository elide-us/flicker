//! The emissive colour palette — a bounded, DATA-DRIVEN set of prefab glow colours
//! (`content/data/palettes.json`, `palettes.emissive`).
//!
//! The bench writes a chosen entry VERBATIM into `recipe.out.emissive` and snaps Roll's
//! random glow to the nearest entry, so emissive colours stay bounded (the material-set
//! budget invariant, MCP `B46FFC37`). Values are the recipe's native colour domain
//! (map byte / 255) with NO colour-space conversion — the same convention every shipped
//! ramp stop and the rolled `hsv()` glow already use.

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

/// Inject one style block per palette entry into the merged `ui_styles`, under
/// `sablework.glowsw.<id>` (resting) and `sablework.glowsw.<id>_sel` (selected) — LITERAL
/// rgba, the godmode `inject_legend_styles` pattern (this runs AFTER token resolution, so
/// no `$tokens`). The swatch is a `button`: `fill_top`/`fill_bot` carry the palette colour
/// through every button state, a border marks selection, and the glow halo is the colour
/// on focus. Call from `enter()` after `load_shared_styles`, and from the tests' `draw()`
/// helper, so the drawn-frame gates exercise the real look.
///
/// `style_bind` values escape the cross-scene style-path gate (they ride the Model
/// channel), so a bench test must fail-loud on any `glowsw` path that resolves to nothing
/// (drift-gate law 8634C200).
pub fn inject_glow_styles(ui_styles: &mut serde_json::Value, palette: &GlowPalette) {
    let Some(root) = ui_styles.as_object_mut() else {
        return;
    };
    let sw = root
        .entry("sablework")
        .or_insert_with(|| serde_json::json!({}));
    let Some(sw) = sw.as_object_mut() else { return };
    let glowsw = sw.entry("glowsw").or_insert_with(|| serde_json::json!({}));
    let Some(glowsw) = glowsw.as_object_mut() else {
        return;
    };
    for e in &palette.entries {
        let [r, g, b] = e.rgb;
        let fill = serde_json::json!([r, g, b, 1.0]);
        glowsw.insert(
            e.id.clone(),
            serde_json::json!({
                "fill_top": fill, "fill_bot": fill,
                "border": [0.12, 0.12, 0.14, 1.0], "border_w": 1, "radius": 3
            }),
        );
        glowsw.insert(
            format!("{}_sel", e.id),
            serde_json::json!({
                "fill_top": fill, "fill_bot": fill,
                "border": [1.0, 1.0, 1.0, 1.0], "border_w": 2, "radius": 3, "glow": fill
            }),
        );
    }
}
