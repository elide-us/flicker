//! prism-alpha — the unified Prism client shell (the single launcher).
//!
//! Every Prism test app / POC is just a `Scene`; this one binary hosts them all
//! behind the shared front-end (splash → mode menu → scene → pause). The root menu
//! lists the PLAY MODES (Adventurer · Dungeon Maker · Game Master · Developer);
//! each mode's tier-2 page lists the scenes tagged to its realm, and a scene's
//! `SceneInfo` fills that page's panel row.
//!
//! Migration status (2026-08-12): the roster carries the benches MIGRATED to the new
//! system. Every launchable scene is a file in `content/sensorium/scenes/` — ONE folder,
//! the kernel manifest — and each roster entry below is the CLIENT BEHAVIOUR that plays
//! the file naming it (`resolve_shell_scene`: manifest → shell behaviours → these
//! entries, factory receiving the parsed `SceneDef`). The roster's other half is
//! launcher metadata (realm + `SceneInfo`). The dormant scene crates stay compiling +
//! UNCHANGED until they migrate.

use anyhow::Result;

use flicker_shell::{
    SceneEntry, SceneInfo, ShellConfig, REALM_ADVENTURER, REALM_DEVELOPER, REALM_GAMEMASTER,
};

/// The launcher roster — one `SceneEntry` per LAUNCHABLE scene. Its realm tag
/// decides which mode page lists it; the row LOAD button fires the entry `id`; the
/// shell resolves that id through the MANIFEST and hands the file's def to the
/// entry's factory (the entry is the client half of the behaviour registry).
///
/// Grows as benches migrate (see the module note). The gate test below holds the
/// roster and the scenes folder bound to each other.
fn roster() -> Vec<SceneEntry> {
    vec![
        SceneEntry::new("populous", "Populous Bench", "primary", flicker_populous::scene)
            .with_realm(REALM_GAMEMASTER)
            .with_info(SceneInfo::new(
                "Populous Bench",
                "Authoring",
                "World Map",
                "A hex map of a world — every tile its own index, and a dial from 23k to 144k tiles. Nothing else, yet.",
                "Clay 0.1 · Bench · hex map",
            )),
        SceneEntry::new("clicktrainer", "Click Trainer", "primary", flicker_clicktrainer::scene)
            .with_realm(REALM_DEVELOPER)
            .with_info(SceneInfo::new(
                "Click Trainer",
                "Trainer",
                "Aim / 2D",
                "Click the shrinking targets before they time out — live accuracy and reaction-time readouts over a 2D sprite game.",
                "Clay 0.1 · Bench · 2D + vector HUD",
            )),
        SceneEntry::new(
            "componentcatalog",
            "Component Catalog",
            "primary",
            flicker_componentcatalog::scene,
        )
        .with_realm(REALM_DEVELOPER)
        .with_info(SceneInfo::new(
            "Component Catalog",
            "Reference",
            "UI / Widgets",
            "One live copy of every Rust widget component with all features on — a nav rail of bookmarks over a card per control. The UI test scene.",
            "Clay 0.1 · Bench · UI catalog",
        )),
        SceneEntry::new("solarbirth", "Solar Birth", "primary", flicker_solarbirth::scene)
            .with_realm(REALM_ADVENTURER)
            .with_info(SceneInfo::new(
                "Solar Birth",
                "Cinematic",
                "Celestial",
                "A camera fly-in over the fixed Prism system as the dust clears — drag to orbit, wheel to zoom, Space to replay the flight.",
                "Clay 0.1 · Cinematic · fly-in",
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

    // Declare where THIS executable's content lives (committed `content.json`,
    // beside the per-user `settings.json` but versioned) BEFORE any scene loads:
    // benches ask `flicker_content::roots()` for the staging/package trees rather
    // than hardcoding a climb out of their own crate directory.
    flicker_content::init_from_app_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));

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
    use flicker_shell::{REALM_ADVENTURER, REALM_DEVELOPER, REALM_DM};

    /// The launchable roster is the set of benches MIGRATED to the new system, in
    /// realm order: Populous (Game Master), Click Trainer (Developer), Solar Birth
    /// (Adventurer). DM stays dark until its bench migrates (backlog in MCP). Pins the
    /// set so a stray re-add of an un-migrated bench is caught.
    #[test]
    fn roster_holds_the_migrated_benches() {
        let scenes = roster();
        let realm_ids = |realm: &str| -> Vec<&str> {
            scenes
                .iter()
                .filter(|e| e.realms.iter().any(|r| r == realm))
                .map(|e| e.id.as_str())
                .collect()
        };
        assert_eq!(realm_ids(REALM_GAMEMASTER), ["populous"], "Populous is the Game Master bench");
        assert_eq!(
            realm_ids(REALM_DEVELOPER),
            ["clicktrainer", "componentcatalog"],
            "Click Trainer + the Component Catalog are the Developer benches"
        );
        assert_eq!(realm_ids(REALM_ADVENTURER), ["solarbirth"], "Solar Birth is the Adventurer bench");
        assert!(
            realm_ids(REALM_DM).is_empty(),
            "realm '{REALM_DM}' has no migrated bench yet"
        );
        assert!(
            scenes.iter().all(|e| e.info.is_some()),
            "every launcher bench carries panel metadata for its row"
        );
    }

    /// THE MANIFEST↔ROSTER GATE — the client half of the behaviour registry, closed
    /// where both sides are visible (the shell's own gate can only vouch for its
    /// builtin behaviours). One folder, no strays:
    /// - every authored scene's `behaviour` is a SHELL builtin or a roster entry id
    ///   (a file nothing plays is a black screen waiting to be clicked);
    /// - every roster entry has an authored scene file (a launchable scene IS data —
    ///   no file, no launch);
    /// - each launchable file's `behaviour` names its OWN roster entry, so `Goto{id}`
    ///   builds the bench the file says it is.
    #[test]
    fn every_authored_scene_resolves_and_every_bench_is_authored() {
        use flicker_widgets::scene_def::SceneManifest;

        flicker_content::init_from_app_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
        let dir = flicker_content::roots().sensorium().join("scenes");
        let m = SceneManifest::load_dir(&dir).expect("the scenes folder indexes");
        let builtins = flicker_shell::builtin_behaviours();
        let scenes = roster();

        for def in m.scenes() {
            let builtin = builtins.contains(&def.behaviour.as_str());
            let registered = scenes.iter().any(|e| e.id == def.behaviour);
            assert!(
                builtin || registered,
                "scene file '{}' names behaviour '{}', which neither the shell builds \
                 nor any roster entry plays",
                def.id,
                def.behaviour
            );
        }
        for e in &scenes {
            let def = m
                .get(&e.id)
                .unwrap_or_else(|| panic!("roster entry '{}' has no scene file — a launchable scene is DATA", e.id));
            assert_eq!(
                def.behaviour, e.id,
                "scene file '{}' must name its own roster entry as its behaviour",
                e.id
            );
        }
    }
}
