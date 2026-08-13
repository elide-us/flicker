//! God Mode's globe view — **the shared one**, plus the stage this bench is
//! authored by.
//!
//! The offscreen plumbing (target sizing, the FrameGraph pass, the composite,
//! and reading `stages.<source>`) moved to `flicker-globe` when the Populous
//! bench needed exactly the same thing. What stays here is the only part that is
//! actually God Mode's: WHICH stage block it is drawn by.

pub use flicker_globe::view::Arrows;

/// The `stages.<source>` block this view is authored by, and the `source` the
/// bench's `rtt` node names. One string, both sides.
pub const STAGE_SOURCE: &str = "godmode_globe";

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_globe::{GlobeStage, StageLayer};

    fn styles() -> serde_json::Value {
        // The PRODUCTION loader, not a raw file read: stage sources live in the
        // ui_stages.json satellite and reach the root through load_styles's merge.
        flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            Some(&crate::scene_styles()),
        )
    }

    /// **The authored stage is actually READ.** The bench declares
    /// `stages.godmode_globe`, and a declaration nothing consumes is a name
    /// that resolves to nothing — which is how Sablework's lit view once
    /// shipped lit by a constant while its own `"lighting": "studio"` sat
    /// unused. So: the parsed stage must differ from the bare default, and it
    /// must emit light.
    #[test]
    fn the_authored_globe_stage_is_read() {
        let s = GlobeStage::from_styles(&styles(), STAGE_SOURCE);
        let bare = GlobeStage::default();
        assert!(
            s.lighting.sun_dir != bare.lighting.sun_dir
                || s.lighting.sun_color != bare.lighting.sun_color
                || s.lighting.ambient != bare.lighting.ambient,
            "stages.{STAGE_SOURCE} is authored but nothing in it reached the view"
        );
        assert!(
            s.lighting.sun_color.length_squared() > 0.0
                || s.lighting.ambient.length_squared() > 0.0,
            "the globe would render black"
        );
    }

    /// An unknown source must fall back LIT, not black: a typo in a style file
    /// should cost the authored look, never the picture.
    /// The bench's own layer list is authored too: God Mode's shells come from
    /// a running simulation, so its stage says exactly that and nothing else —
    /// no authored shell, no authored frame (the grid is a key it toggles).
    #[test]
    fn the_stage_declares_the_simulated_shells() {
        let s = GlobeStage::from_styles(&styles(), STAGE_SOURCE);
        assert_eq!(s.layers, vec![StageLayer::Shells], "the sim publishes this world");
    }

    #[test]
    fn an_unknown_source_still_lights_the_globe() {
        let s = GlobeStage::from_styles(&styles(), "no_such_stage");
        assert!(
            s.lighting.sun_color.length_squared() > 0.0
                || s.lighting.ambient.length_squared() > 0.0,
            "the fallback must still light the globe"
        );
    }
}
