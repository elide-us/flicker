//! A recipe — the whole instrument's state, and the only thing that gets saved.
//!
//! A recipe is small (a few hundred bytes of JSON) and produces megabytes of
//! image, which is the point: the *authored* artifact is the settings, and the
//! maps are a rebuildable consequence. Re-baking a recipe at a different
//! resolution is free; recovering the settings from baked PNGs is not.

use serde::{Deserialize, Serialize};

use flicker_materials::MaterialId;

use crate::channel::{Channel, CHANNEL_COUNT};
use crate::output::OutputStage;

/// The instrument's state: a seed, a rack, and an output stage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextureRecipe {
    /// Stable id — the file stem and the key a consumer looks it up by.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Which of the 256 materials this is the appearance of, if any.
    ///
    /// Optional because an unbound recipe is a legitimate scratch surface — you
    /// dial one in before deciding what it *is*. A bound recipe is what lets
    /// terrain ask "what does material 47 look like?".
    #[serde(default)]
    pub material: Option<MaterialId>,
    /// The one knob that re-rolls every field at once.
    pub seed: u64,
    /// The rack, in console order.
    pub channels: [Channel; CHANNEL_COUNT],
    /// Field → maps.
    pub out: OutputStage,
}

impl Default for TextureRecipe {
    /// An untitled recipe with one audible voice, so the preview shows a surface
    /// rather than a black square the moment the bench opens.
    fn default() -> Self {
        let mut channels = [Channel::default(); CHANNEL_COUNT];
        channels[0] = Channel {
            enabled: true,
            ..Channel::default()
        };
        Self {
            id: "untitled".into(),
            name: "Untitled".into(),
            material: None,
            seed: 1,
            channels,
            out: OutputStage::default(),
        }
    }
}

impl TextureRecipe {
    /// How many voices are actually sounding — the strip's summary readout.
    pub fn active_channels(&self) -> usize {
        self.channels.iter().filter(|c| c.enabled).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{BlendMode, NoiseKind};

    /// A recipe must survive a JSON round trip byte-identically, because that
    /// file IS the artifact — a lossy field would silently change a surface the
    /// next time it was opened.
    #[test]
    fn round_trips_through_json() {
        let mut r = TextureRecipe {
            id: "granite".into(),
            name: "Granite".into(),
            ..Default::default()
        };
        r.material = Some(12);
        r.seed = 0xDEAD_BEEF;
        r.channels[1] = Channel {
            enabled: true,
            source: NoiseKind::Worley,
            blend: BlendMode::Overlay,
            warp: 0.4,
            invert: true,
            ..Channel::default()
        };
        let text = serde_json::to_string_pretty(&r).expect("recipe serializes");
        let back: TextureRecipe = serde_json::from_str(&text).expect("recipe deserializes");
        assert_eq!(r, back);
    }

    /// `material` is `#[serde(default)]`, so an older unbound recipe still loads.
    #[test]
    fn an_unbound_recipe_loads_without_the_material_field() {
        let r = TextureRecipe {
            material: None,
            ..TextureRecipe::default()
        };
        let mut v: serde_json::Value = serde_json::to_value(&r).unwrap();
        v.as_object_mut().unwrap().remove("material");
        let back: TextureRecipe = serde_json::from_value(v).expect("loads without material");
        assert_eq!(back.material, None);
    }

    #[test]
    fn a_fresh_recipe_makes_a_sound() {
        assert_eq!(TextureRecipe::default().active_channels(), 1);
    }
}
