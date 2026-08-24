//! God Mode's globe view — **the shared one**, plus the stage this bench is
//! authored by.
//!
//! The offscreen plumbing (target sizing, the FrameGraph pass, the composite)
//! moved to `flicker-globe` when the Populous bench needed exactly the same thing,
//! and reading `stages.<source>` moved to the one stage compiler in
//! `flicker-widgets`. What stays here is the only part that is actually God
//! Mode's: WHICH stage block it is drawn by.

pub use flicker_globe::view::Arrows;

/// The `stages.<source>` block this view is authored by, and the `source` the
/// bench's `surface` node names. One string, both sides.
pub const STAGE_SOURCE: &str = "godmode_globe";

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::render::StageLayer;
    use flicker_globe::GlobeWorld;

    fn styles() -> serde_json::Value {
        // The PRODUCTION loader over the SHIPPED scene file's style blocks (the
        // five-line split): stage sources live in the ui_stages.json satellite
        // and reach the root through load_styles_for's merge, exactly as the
        // runtime builds them from the manifest's def.
        let def = flicker::ui::SceneDef::parse(
            "godmode",
            include_str!("../../../../content/sensorium/scenes/godmode.scene.json"),
        )
        .expect("the shipped godmode.scene.json parses");
        flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        )
    }

    /// **The authored stage is actually READ.** The bench declares
    /// `stages.godmode_globe`, and a declaration nothing consumes is a name
    /// that resolves to nothing — which is how Sablework's lit view once
    /// shipped lit by a constant while its own `"lighting": "studio"` sat
    /// unused. So: the stage must compile (it exists), and it must emit light.
    #[test]
    fn the_authored_globe_stage_is_read() {
        let s = flicker::ui::stage_def(&styles(), STAGE_SOURCE)
            .unwrap_or_else(|| panic!("stages.{STAGE_SOURCE} is not authored"));
        assert!(
            s.lighting.sky_sun().color.length_squared() > 0.0
                || s.lighting.ambient.length_squared() > 0.0,
            "the globe would render black"
        );
        assert!(
            s.camera.is_none(),
            "a globe stage authors no camera — the maintainer flies the planet"
        );
    }

    /// The bench's own layer list is authored too: God Mode's shells come from
    /// a running simulation, so its stage says exactly that and nothing else —
    /// no authored shell, no authored frame (the grid is a key it toggles).
    #[test]
    fn the_stage_declares_the_simulated_shells() {
        let s = flicker::ui::stage_def(&styles(), STAGE_SOURCE).expect("authored");
        assert_eq!(
            s.layers,
            vec![StageLayer::Shells],
            "the sim publishes this world"
        );
    }

    /// An unknown source must fall back LIT, not black: a typo in a style file
    /// should cost the authored look, never the picture — the world shows the
    /// scene's own shells under the default light.
    #[test]
    fn an_unknown_source_still_lights_the_globe() {
        let world = GlobeWorld::new("no_such_stage", &styles(), None);
        let s = world.stage();
        assert!(
            s.lighting.sky_sun().color.length_squared() > 0.0
                || s.lighting.ambient.length_squared() > 0.0,
            "the fallback must still light the globe"
        );
        assert_eq!(s.layers, vec![StageLayer::Shells]);
    }
}
