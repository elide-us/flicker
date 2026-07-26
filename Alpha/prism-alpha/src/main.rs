//! prism-alpha — the unified Prism client shell (the single launcher).
//!
//! Every Prism test app / POC is just a `Scene`; this one binary hosts them all
//! behind the shared front-end (splash → menu → scene → pause). Its menu is the
//! **launcher**: the standard popup on the left (Settings / Quit) plus a SCENE-
//! SELECTION PANEL on the right (`scene_select`), one data-driven row per scene.
//! The per-app `flicker-*` crates stay as thin standalone entry points to the SAME
//! scenes — nothing about a scene is specific to which binary launches it.
//!
//! Roster today: the three scenes already re-homed into libraries. **Pack Editor is
//! still a raw `impl App` (not a `Scene`)**, so it joins the roster once it is
//! converted. `Scene`-conversion of a POC is the whole cost of adding it here.

use anyhow::Result;

use flicker_clicktrainer::ClickTrainer;
use flicker_controllertester::ControllerTester;
use flicker_shell::{SceneEntry, SceneInfo, ShellConfig};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "prism_alpha=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    // The launcher roster: one `SceneEntry` per scene, each carrying `SceneInfo` for
    // its panel row (name · mode · region · desc · meta). The row LOAD button fires the
    // entry `id`, which the shell menu dispatches to `factory()`.
    let scenes = vec![
        SceneEntry::new("solarbirth", "Solar Birth", "primary", flicker_solarbirth::scene).with_info(
            SceneInfo::new(
                "Solar Birth",
                "Cinematic",
                "Celestial",
                "Fly-in over the fixed Prism system as the dust cloud clears.",
                "Clay 0.1 · Cinematic · flight-path",
            ),
        ),
        // Click Trainer stays a plain launch button in the menu popup (above Settings),
        // not a scene-selection card — a quick minigame, not one of the showcase scenes.
        // No `SceneInfo` ⇒ the shell renders it as a popup button (see `SceneEntry::info`).
        SceneEntry::new("clicktrainer", "CLICK TRAINER", "primary", || {
            Box::new(ClickTrainer::new())
        }),
        SceneEntry::new("controllertester", "Controller Tester", "primary", || {
            Box::new(ControllerTester::new())
        })
        .with_info(SceneInfo::new(
            "Controller Tester",
            "Diagnostic",
            "Input",
            "Live gamepad / keyboard / mouse readout — buttons, sticks and triggers light up as you press them.",
            "Clay 0.1 · Tool · gilrs",
        )),
        SceneEntry::new("loomforge", "Loomforge Bench", "primary", flicker_loomforge::scene)
            .with_info(SceneInfo::new(
                "Loomforge Bench",
                "Rigging",
                "Animation",
                "Author a state machine, packs, creatures, and TAE windows — and save the pack back.",
                "Clay 0.1 · Editor · flicker.pack",
            )),
        SceneEntry::new("assetpipeline", "Kilnworks Bench", "primary", flicker_assetpipeline::scene)
            .with_info(SceneInfo::new(
                "Kilnworks Bench",
                "Rigging",
                "Content",
                "Open a source folder and drive it through classify, rig conform, attach and bake.",
                "Clay 0.1 · Editor · flicker.rig",
            )),
        SceneEntry::new("pocclusters", "Cluster Editor", "primary", flicker_pocclusters::scene)
            .with_info(SceneInfo::new(
                "Cluster Editor",
                "Tool",
                "CSG / Voxel",
                "3×3 voxel-cluster field — dual-contour + mesh, LOD, navmesh, and the virtual-voxel inspector.",
                "Clay 0.1 · Tool · dual-contour",
            )),
    ];

    // The shell owns the whole front-end + the winit run loop. `scene_select` renders
    // the scenes as the right-hand panel. Tier-3 player config (settings.json) lives in
    // this client's own root.
    flicker_shell::run(ShellConfig {
        scenes,
        settings_dir: Some(env!("CARGO_MANIFEST_DIR").into()),
        scene_select: true,
    })
}
