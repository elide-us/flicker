//! prism-alpha — the unified Prism client shell (the single launcher).
//!
//! Every Prism test app / POC is just a `Scene`; this one binary hosts them all
//! behind the shared front-end (splash → mode menu → scene → pause). Its root
//! menu lists the three PLAY MODES — Explore the World (Adventurer), Build the
//! World (DM, under construction), Developer Mode — plus the Click Trainer
//! quick-launch button and the Settings/Quit chrome. Each mode opens its tier-2
//! page: Adventurer = the Solar Birth cinematic, DM = the coming-soon note,
//! Developer = the two-column SCENE-SELECTION launcher (one data-driven row per
//! tool/bench scene). Mode membership is the entry's `realms` tag list (a tool
//! can be shared across modes); `SceneInfo` stays pure display data for the row.
//!
//! The per-app `flicker-*` crates stay as thin standalone entry points to the SAME
//! scenes — nothing about a scene is specific to which binary launches it.
//! **Pack Editor is still a raw `impl App` (not a `Scene`)**, so it joins the
//! roster once it is converted. `Scene`-conversion of a POC is the whole cost of
//! adding it here.

use anyhow::Result;

use flicker_clicktrainer::ClickTrainer;
use flicker_controllertester::ControllerTester;
use flicker_shell::{SceneEntry, SceneInfo, ShellConfig, REALM_ADVENTURER, REALM_DEVELOPER};

/// The launcher roster: one `SceneEntry` per scene — its realm tags decide which
/// mode's tier-2 page lists it, and its `SceneInfo` fills that page's panel row
/// (name · mode · region · desc · meta). The row LOAD button fires the entry
/// `id`, which the shell menu dispatches to `factory()`.
fn roster() -> Vec<SceneEntry> {
    vec![
        // Adventurer mode ("Explore the World"): exactly the Solar Birth cinematic.
        SceneEntry::new("solarbirth", "Solar Birth", "primary", flicker_solarbirth::scene)
            .with_realm(REALM_ADVENTURER)
            .with_info(SceneInfo::new(
                "Solar Birth",
                "Cinematic",
                "Celestial",
                "Fly-in over the fixed Prism system as the dust cloud clears.",
                "Clay 0.1 · Cinematic · flight-path",
            )),
        // Click Trainer stays a plain launch button on the ROOT menu (above Settings),
        // not a mode-page card — a quick minigame, not one of the showcase scenes.
        // No `SceneInfo` and no realm ⇒ the shell renders it as a root popup button.
        SceneEntry::new("clicktrainer", "CLICK TRAINER", "primary", || {
            Box::new(ClickTrainer::new())
        }),
        // Developer mode: the tool/bench scenes of the two-column launcher.
        SceneEntry::new("controllertester", "Controller Tester", "primary", || {
            Box::new(ControllerTester::new())
        })
        .with_realm(REALM_DEVELOPER)
        .with_info(SceneInfo::new(
            "Controller Tester",
            "Diagnostic",
            "Input",
            "Live gamepad / keyboard / mouse readout — buttons, sticks and triggers light up as you press them.",
            "Clay 0.1 · Tool · gilrs",
        )),
        SceneEntry::new("loomforge", "Loomforge Bench", "primary", flicker_loomforge::scene)
            .with_realm(REALM_DEVELOPER)
            .with_info(SceneInfo::new(
                "Loomforge Bench",
                "Rigging",
                "Animation",
                "Author a state machine, packs, creatures, and TAE windows — and save the pack back.",
                "Clay 0.1 · Editor · flicker.pack",
            )),
        SceneEntry::new("assetpipeline", "Clayworks Bench", "primary", flicker_assetpipeline::scene)
            .with_realm(REALM_DEVELOPER)
            .with_info(SceneInfo::new(
                "Clayworks Bench",
                "Rigging",
                "Content",
                "Choose a workflow, then rig the asset, set attach points, and export to the pack editor.",
                "Clay 0.1 · Editor · flicker.rig",
            )),
        SceneEntry::new("pocclusters", "Cluster Editor", "primary", flicker_pocclusters::scene)
            .with_realm(REALM_DEVELOPER)
            .with_info(SceneInfo::new(
                "Cluster Editor",
                "Tool",
                "CSG / Voxel",
                "3×3 voxel-cluster field — dual-contour + mesh, LOD, navmesh, and the virtual-voxel inspector.",
                "Clay 0.1 · Tool · dual-contour",
            )),
        // MOVED from the standalone flicker-pocepochs app (now a library package).
        SceneEntry::new("pocepochs", "Planet Simulation", "primary", flicker_pocepochs::scene)
            .with_realm(REALM_DEVELOPER)
            .with_info(SceneInfo::new(
                "Planet Simulation",
                "Simulation",
                "World-Gen",
                "Seed a hex planet and run the epoch sim — reseed, resize, slice the layer stack, watch convection.",
                "Clay 0.1 · Sim · worldengine",
            )),
    ]
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "prism_alpha=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    // The shell owns the whole front-end + the winit run loop. `scene_select` makes
    // the menu the MODE LAUNCHER (root mode buttons → tier-2 scene pages). Tier-3
    // player config (settings.json) lives in this client's own root.
    flicker_shell::run(ShellConfig {
        scenes: roster(),
        settings_dir: Some(env!("CARGO_MANIFEST_DIR").into()),
        scene_select: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_shell::REALM_DM;

    /// The Aaron-ratified mode map, pinned: Adventurer = exactly Solar Birth; DM =
    /// nothing (its page is the coming-soon note); Developer = the dev tools
    /// INCLUDING the re-homed pocepochs and EXCLUDING solarbirth/clicktrainer;
    /// Click Trainer stays a realm-less root-level launch button.
    #[test]
    fn roster_realms_follow_the_ratified_mode_map() {
        let scenes = roster();
        let realm_ids = |realm: &str| -> Vec<&str> {
            scenes
                .iter()
                .filter(|e| e.realms.iter().any(|r| r == realm))
                .map(|e| e.id.as_str())
                .collect()
        };

        assert_eq!(realm_ids(REALM_ADVENTURER), ["solarbirth"], "Adventurer = exactly Solar Birth");
        assert!(realm_ids(REALM_DM).is_empty(), "DM mode is under construction");

        let dev = realm_ids(REALM_DEVELOPER);
        assert!(dev.contains(&"pocepochs"), "pocepochs moved into the Developer launcher");
        assert!(!dev.contains(&"solarbirth"), "Solar Birth moved OUT to Adventurer");
        assert!(!dev.contains(&"clicktrainer"), "Click Trainer stays on the root menu");

        // Click Trainer: realm-less AND info-less ⇒ a plain root popup button.
        let ct = scenes.iter().find(|e| e.id == "clicktrainer").expect("click trainer registered");
        assert!(ct.realms.is_empty() && ct.info.is_none());

        // Every other entry is realm-tagged and info-bearing (a mode-page panel row).
        for e in scenes.iter().filter(|e| e.id != "clicktrainer") {
            assert!(!e.realms.is_empty(), "'{}' belongs to a mode", e.id);
            assert!(e.info.is_some(), "'{}' carries panel metadata", e.id);
        }
    }
}
