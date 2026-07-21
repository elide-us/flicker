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
        SceneEntry::new("clicktrainer", "Click Trainer", "primary", || {
            Box::new(ClickTrainer::new())
        })
        .with_info(SceneInfo::new(
            "Click Trainer",
            "Mini-Game",
            "2D",
            "Aim / click-training drill — sprite targets under a vector HUD.",
            "Clay 0.1 · 2D · sprite+vector",
        )),
        SceneEntry::new("paperdoll", "Paper Doll", "primary", flicker_paperdoll::scene).with_info(
            SceneInfo::new(
                "Paper Doll",
                "Rigging",
                "Animation POC",
                "Skeletal rig + clip playback and the fit / dress gadget.",
                "Clay 0.1 · Tool · 66-bone",
            ),
        ),
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
