//! The **scene file** — one JSON document per scene — and the **manifest** that
//! indexes the folder they live in.
//!
//! ```json
//! {
//!   "boot":      true,
//!   "behaviour": "splash",
//!   "params":    { "image": "package/sensorium/assets/elideus_productions_yellow.png" },
//!   "tree":      { "component": "screen", "children": [ … ] },
//!   "exits":     { "done": { "to": "CeLogo", "mode": "replace" } }
//! }
//! ```
//!
//! # The engine knows where scenes LIVE, and nothing else about them
//!
//! Scene files sit in ONE folder (`content/sensorium/scenes/`), named
//! `<Name>.scene.json`. [`SceneManifest::load_dir`] enumerates that folder at
//! startup, so **the folder listing IS the roster**: adding a scene is dropping a
//! file in, and no Rust anywhere names a scene. A scene's [`id`](SceneDef::id) is
//! DERIVED from its file name (`TegLogo.scene.json` → `TegLogo`) rather than
//! restated inside the document — one source of truth, so a file and its id can
//! never disagree.
//!
//! # What an exit IS
//!
//! `exits` maps a **fired result name** to a target scene id. Not a timer, not a
//! condition — a RESULT. That is the whole design: a scene's *behaviour* stays
//! Rust (a splash's timer, a bench's simulation, a menu button's click), and when
//! that behaviour fires a result the FILE decides where it goes. A splash keeps
//! its timer and fires `done`; its file routes `done -> CeLogo`. A menu button
//! already fires a result named after its target. One mechanism serves both, so
//! "what follows what" stops being a constructor call in Rust
//! (`Transition::Replace(Box::new(MenuScene::new()))`) and becomes a line a human
//! can read and reorder.
//!
//! `mode` is optional and defaults to `"replace"`; the three legal spellings
//! (`replace` / `replace_root` / `push`) are exactly [`flicker_scene::GotoMode`] —
//! the file names the same three stack moves the manager already performs, with no
//! second vocabulary to keep in sync.
//!
//! # What a BEHAVIOUR is, and why the rule survives it
//!
//! Some scenes DO something no data can express: a splash's fade timer, a planet
//! sim, a bench's tool state. `behaviour` names one — `"splash"`, `"menu"`,
//! `"godmode"` — and the client's registry maps that NAME to a Rust impl. The
//! engine therefore knows BEHAVIOURS, never scenes: adding a scene is always
//! data, and only a genuinely new *kind* of scene costs Rust. `params` carries
//! whatever that behaviour needs to be pointed at (which image a splash plays),
//! so two scenes can share one behaviour without either being named in code.
//!
//! # Fail LOUD
//!
//! Every authored NAME in the file is resolved AT LOAD and every failure is an
//! `Err` (rule: a name that fails to NOTHING is the difference between authorable
//! and not). An unreadable file, an unparseable tree, a missing `behaviour`, an
//! unknown component kind, an unknown template name, an unspellable `mode`, a
//! mistyped top-level key, zero or two scenes claiming `boot` — all refuse to load
//! rather than yielding a scene that renders a blank screen. The TWO checks this
//! module cannot make are whether an exit's `to` names a scene the client can
//! actually resolve (its roster may extend past the manifest — see
//! [`SceneDef::targets`]) and whether `behaviour` names something registered
//! (the registry belongs to the client).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use flicker_scene::{GotoMode, Transition};
use flicker_script::UiNode;

use crate::template::{expand, TemplateDef, TemplateRegistry};

/// The file-name suffix that marks a scene file — `TegLogo.scene.json`. The part
/// before it is the scene's id, so the naming convention IS the roster key.
pub const SCENE_FILE_SUFFIX: &str = ".scene.json";

/// The scene id a file name declares, or `None` if the name is not a scene file.
///
/// `TegLogo.scene.json` → `Some("TegLogo")`; `notes.json` / `.DS_Store` → `None`.
#[must_use]
pub fn scene_id_from_file_name(name: &str) -> Option<&str> {
    let id = name.strip_suffix(SCENE_FILE_SUFFIX)?;
    (!id.is_empty()).then_some(id)
}

/// Where one FIRED RESULT sends the scene manager: the target scene `to`, and
/// which stack reshape gets it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneExit {
    /// The target scene id, resolved through the client's roster.
    pub to: String,
    /// The stack move — [`GotoMode::Replace`] unless the file says otherwise.
    pub mode: GotoMode,
}

/// One loaded scene file: its `id` (from the FILE NAME), the `behaviour` that
/// plays it, that behaviour's `params`, the authored UI `tree`, and its `exits`.
/// Built by [`SceneDef::parse`]; consumed by the Rust behaviour bound to it.
#[derive(Debug, Clone)]
pub struct SceneDef {
    /// The scene's id — derived from the file name, the name the roster registers
    /// it under and every [`Transition::Goto`] addresses it by. Never spelled
    /// inside the document.
    pub id: String,
    /// The registered behaviour that plays this scene (`"splash"`, `"menu"`, …).
    /// Resolved by the CLIENT's behaviour registry; a scene file without one could
    /// only ever be a blank screen, so it is required.
    pub behaviour: String,
    /// That behaviour's own configuration — whatever it needs to be pointed at
    /// (a splash's image path). Opaque here: only the behaviour knows its keys,
    /// and only the behaviour can report a missing one.
    pub params: serde_json::Map<String, serde_json::Value>,
    /// `true` on the ONE scene the application boots into. Exactly one scene in a
    /// [`SceneManifest`] may claim it — so even "what starts first" is authored.
    pub boot: bool,
    /// The authored UI tree, already template-expanded and vocabulary-checked.
    ///
    /// `None` when the file authors no `tree` — a scene whose visuals are still
    /// drawn by Rust or by an immediate-mode script (the intro splash plays its
    /// fade/hold timeline in `logo.lua` and blits one image; there is no component
    /// tree to author yet). Such a scene still owns a scene FILE, because its
    /// ROUTING is authorable today even though its composition is not.
    pub tree: Option<UiNode>,
    /// Fired result name → where it goes.
    pub exits: HashMap<String, SceneExit>,
}

/// The top-level keys a scene file may carry. Anything else is a typo — and a
/// typo'd `exit` (for `exits`) would otherwise load a scene with no way out, so
/// this is checked rather than ignored.
///
/// `id` is deliberately NOT here: the id comes from the file name, and a document
/// that restates it would be a second source of truth free to disagree with the
/// first.
const SCENE_KEYS: &[&str] = &["_comment", "boot", "behaviour", "params", "tree", "exits"];

impl SceneDef {
    /// Read one scene file. `id` is the file's derived id (see
    /// [`scene_id_from_file_name`]); `reg` is the template registry its `tree` is
    /// expanded against — normally [`builtin_templates`](crate::builtin_templates),
    /// plus any bespoke builder the client registered.
    pub fn parse(id: &str, text: &str, reg: &TemplateRegistry) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| format!("scene '{id}' is not JSON: {e}"))?;
        Self::from_json(id, &value, reg)
    }

    /// [`parse`](Self::parse) from an already-decoded document.
    pub fn from_json(
        id: &str,
        value: &serde_json::Value,
        reg: &TemplateRegistry,
    ) -> Result<Self, String> {
        let obj = value
            .as_object()
            .ok_or_else(|| format!("scene '{id}': file must be a JSON object"))?;
        let unknown: Vec<&str> = obj
            .keys()
            .map(String::as_str)
            .filter(|k| !SCENE_KEYS.contains(k))
            .collect();
        if !unknown.is_empty() {
            let hint = if unknown.contains(&"id") {
                // The single most likely stale key: files used to carry their own id.
                " — a scene's id comes from its FILE NAME, so drop the `id` key"
            } else {
                ""
            };
            return Err(format!(
                "scene '{id}' names unknown top-level key(s) {unknown:?}; \
                 legal keys are {SCENE_KEYS:?}{hint}"
            ));
        }

        let behaviour = obj
            .get("behaviour")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if behaviour.is_empty() {
            return Err(format!(
                "scene '{id}' names no `behaviour` — every scene needs the registered \
                 Rust behaviour that plays it, e.g. \"behaviour\": \"splash\""
            ));
        }

        let params = match obj.get("params") {
            None | Some(serde_json::Value::Null) => serde_json::Map::new(),
            Some(serde_json::Value::Object(map)) => map.clone(),
            Some(_) => return Err(format!("scene '{id}': `params` must be an object")),
        };

        let boot = match obj.get("boot") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err(format!("scene '{id}': `boot` must be true or false")),
        };

        let tree = match obj.get("tree") {
            None | Some(serde_json::Value::Null) => None,
            Some(json) => Some(build_tree(json, reg).map_err(|e| format!("scene '{id}': {e}"))?),
        };

        let mut exits = HashMap::new();
        match obj.get("exits") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Object(map)) => {
                for (result, spec) in map {
                    let exit = parse_exit(spec)
                        .map_err(|e| format!("scene '{id}' exit `{result}`: {e}"))?;
                    exits.insert(result.clone(), exit);
                }
            }
            Some(_) => return Err(format!("scene '{id}': `exits` must be an object")),
        }

        Ok(Self { id: id.to_string(), behaviour, params, boot, tree, exits })
    }

    /// One string entry from this scene's [`params`](Self::params) — the shape a
    /// behaviour reads its configuration through (a splash's `image`).
    #[must_use]
    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(serde_json::Value::as_str)
    }

    /// The transition `result` names, or `None` when this scene declares no exit
    /// for it.
    ///
    /// `None` is ORDINARY: most results are handled by the scene itself (a menu's
    /// `settings` opens an overlay, a bench's `save` writes a file) and only the
    /// ones that leave the scene are authored here. A result that MUST route —
    /// a splash's `done` — is gated by the client's scene-file test, not by
    /// guessing a destination at runtime.
    #[must_use]
    pub fn exit(&self, result: &str) -> Option<Transition> {
        self.exits.get(result).map(|e| Transition::Goto {
            id: e.to.clone(),
            mode: e.mode,
        })
    }

    /// Every `(fired result, target scene id)` this file declares — the client's
    /// hook for gating that each target actually resolves in its roster. The one
    /// authored name this module cannot check itself, because a client's roster
    /// legitimately extends past the manifest (its registered benches).
    pub fn targets(&self) -> impl Iterator<Item = (&str, &str)> {
        self.exits.iter().map(|(r, e)| (r.as_str(), e.to.as_str()))
    }
}

/// The indexed scenes folder: every `<Name>.scene.json` it holds, keyed by the id
/// its file name declares, plus the ONE scene that claimed `boot`.
///
/// This is the whole of what the engine knows about scenes — a directory path in,
/// a roster out. Nothing in Rust names a member.
#[derive(Debug, Clone)]
pub struct SceneManifest {
    /// id → its loaded file. `BTreeMap` so iteration order is the folder listing
    /// sorted, not hash order: a manifest reads the same on every machine.
    scenes: BTreeMap<String, SceneDef>,
    /// The id of the scene flagged `"boot": true` — validated to be exactly one.
    boot: String,
}

impl SceneManifest {
    /// Index `dir` — every `<Name>.scene.json` in it, in one pass, at startup.
    ///
    /// Anything that is not a scene file is skipped with a WARN rather than a
    /// failure (a `.DS_Store`, an editor backup, a stray note must not stop the
    /// app from booting) — but every file that IS one must load, and the folder
    /// must yield exactly one `boot` scene.
    pub fn load_dir(dir: &Path, reg: &TemplateRegistry) -> Result<Self, String> {
        let listing = std::fs::read_dir(dir)
            .map_err(|e| format!("scenes folder {} could not be read: {e}", dir.display()))?;
        // Sort by file name so a load ERROR names files in a stable order too.
        let mut names: Vec<String> = Vec::new();
        for entry in listing {
            let entry = entry
                .map_err(|e| format!("scenes folder {} could not be walked: {e}", dir.display()))?;
            if entry.path().is_dir() {
                continue;
            }
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();

        let mut files: Vec<(String, String)> = Vec::new();
        for name in names {
            if name.starts_with('.') {
                continue; // dotfiles are never content
            }
            let Some(id) = scene_id_from_file_name(&name) else {
                tracing::warn!(
                    "{} is not a scene file — a scene must be named <Name>{SCENE_FILE_SUFFIX}, \
                     so this file is NOT in the roster",
                    dir.join(&name).display()
                );
                continue;
            };
            let path = dir.join(&name);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("scene file {} could not be read: {e}", path.display()))?;
            files.push((id.to_string(), text));
        }
        Self::from_files(files, reg)
            .map_err(|e| format!("{e} (scenes folder: {})", dir.display()))
    }

    /// Build a manifest from already-read `(id, text)` pairs — what
    /// [`load_dir`](Self::load_dir) does once the bytes are in hand, and the seam
    /// a test drives without touching a disk.
    pub fn from_files(
        files: impl IntoIterator<Item = (String, String)>,
        reg: &TemplateRegistry,
    ) -> Result<Self, String> {
        let mut scenes = BTreeMap::new();
        for (id, text) in files {
            let def = SceneDef::parse(&id, &text, reg)?;
            scenes.insert(id, def);
        }
        if scenes.is_empty() {
            return Err("no scene files found — the roster is empty, so there is \
                        nothing to boot into"
                .to_string());
        }

        // Exactly one scene claims the boot. Zero strands the app with no entry
        // point; two make the entry point a coin flip — both are loud.
        let claimed: Vec<&str> = scenes.values().filter(|d| d.boot).map(|d| d.id.as_str()).collect();
        let boot = match claimed.as_slice() {
            [one] => (*one).to_string(),
            [] => {
                return Err(format!(
                    "no scene claims `\"boot\": true` — exactly one of {:?} must, \
                     or the application has no entry point",
                    scenes.keys().collect::<Vec<_>>()
                ))
            }
            many => {
                return Err(format!(
                    "{many:?} all claim `\"boot\": true` — exactly one scene may, \
                     or which one starts is undefined"
                ))
            }
        };
        Ok(Self { scenes, boot })
    }

    /// The scene registered under `id`, or `None` when the folder holds no such
    /// file.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&SceneDef> {
        self.scenes.get(id)
    }

    /// The id of the scene the application boots into — the one that flagged
    /// `"boot": true`.
    #[must_use]
    pub fn boot(&self) -> &str {
        &self.boot
    }

    /// Every registered scene id, in sorted (stable) order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.scenes.keys().map(String::as_str)
    }

    /// Every loaded scene file, in sorted (stable) order.
    pub fn scenes(&self) -> impl Iterator<Item = &SceneDef> {
        self.scenes.values()
    }

    /// How many scenes the folder yielded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.scenes.len()
    }

    /// Always `false` — a manifest that loaded is never empty (see
    /// [`from_files`](Self::from_files)). Present because `len` without `is_empty`
    /// is a clippy lint, and because it documents that invariant.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scenes.is_empty()
    }
}

/// Parse + expand + vocabulary-check one authored `tree`.
fn build_tree(json: &serde_json::Value, reg: &TemplateRegistry) -> Result<UiNode, String> {
    let node = flicker_script::parse_ui_json(json).map_err(|e| format!("`tree` did not parse: {e}"))?;
    // Template names are resolved BEFORE expansion, because `expand` stands an
    // unknown template in as an empty screen (a warn, and a page with a hole in
    // it). At load a typo is simply an error.
    let missing = unknown_templates(&node, reg);
    if !missing.is_empty() {
        return Err(format!("`tree` names unknown template(s) {missing:?}"));
    }
    let node = expand(node, reg);
    let unknown = crate::unknown_kinds(&node);
    if !unknown.is_empty() {
        return Err(format!("`tree` names unknown component kind(s) {unknown:?}"));
    }
    Ok(node)
}

/// Every template name in `node` (including its slots) that `reg` does not know,
/// deduped in walk order.
fn unknown_templates(node: &UiNode, reg: &TemplateRegistry) -> Vec<String> {
    fn walk(n: &UiNode, reg: &TemplateRegistry, out: &mut Vec<String>) {
        if let Some(name) = &n.template {
            if !matches!(reg.get(name.as_str()), Some(TemplateDef::Builder(_) | TemplateDef::Data(_)))
                && !out.contains(name)
            {
                out.push(name.clone());
            }
        }
        for c in &n.children {
            walk(c, reg, out);
        }
        for group in n.slots.values() {
            for c in group {
                walk(c, reg, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(node, reg, &mut out);
    out
}

/// One `exits` entry: `{ "to": "<scene id>", "mode": "replace" }`.
fn parse_exit(spec: &serde_json::Value) -> Result<SceneExit, String> {
    let obj = spec
        .as_object()
        .ok_or_else(|| "must be an object, e.g. { \"to\": \"Main\" }".to_string())?;
    let to = obj
        .get("to")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if to.is_empty() {
        return Err("has no `to` scene id".to_string());
    }
    let mode = match obj.get("mode").and_then(serde_json::Value::as_str) {
        None => GotoMode::Replace,
        Some("replace") => GotoMode::Replace,
        Some("replace_root") => GotoMode::ReplaceRoot,
        Some("push") => GotoMode::Push,
        Some(other) => {
            return Err(format!(
                "names unknown mode `{other}`; legal modes are replace / replace_root / push"
            ))
        }
    };
    Ok(SceneExit { to, mode })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_templates;

    fn goto(t: Option<Transition>) -> Option<(String, GotoMode)> {
        match t {
            Some(Transition::Goto { id, mode }) => Some((id, mode)),
            _ => None,
        }
    }

    /// The splash shape: a behaviour + its params, no tree yet, one exit, the
    /// default mode. The id comes from the FILE NAME, never from the document.
    #[test]
    fn an_exit_routes_a_fired_result() {
        let def = SceneDef::parse(
            "TegLogo",
            r#"{ "behaviour": "splash", "params": { "image": "logo.png" },
                 "exits": { "done": { "to": "CeLogo" } } }"#,
            &builtin_templates(),
        )
        .expect("scene file loads");
        assert_eq!(def.id, "TegLogo", "the id is the file's, not the document's");
        assert_eq!(def.behaviour, "splash");
        assert_eq!(def.param_str("image"), Some("logo.png"));
        assert!(def.param_str("nope").is_none());
        assert!(!def.boot, "`boot` is opt-in");
        assert!(def.tree.is_none(), "no `tree` authored → None, not an empty screen");
        assert_eq!(
            goto(def.exit("done")),
            Some(("CeLogo".to_string(), GotoMode::Replace)),
            "`mode` defaults to replace"
        );
        assert!(def.exit("nope").is_none(), "an undeclared result is not an exit");
        assert_eq!(def.targets().collect::<Vec<_>>(), vec![("done", "CeLogo")]);
    }

    /// A file name is the id; anything not named `<Name>.scene.json` is not a scene.
    #[test]
    fn the_file_name_is_the_scene_id() {
        assert_eq!(scene_id_from_file_name("TegLogo.scene.json"), Some("TegLogo"));
        assert_eq!(scene_id_from_file_name("Main.scene.json"), Some("Main"));
        for not_a_scene in [".scene.json", "Main.json", "Main.scene.JSON", ".DS_Store", "README"] {
            assert!(
                scene_id_from_file_name(not_a_scene).is_none(),
                "'{not_a_scene}' is not a scene file"
            );
        }
    }

    /// All three stack moves are spellable, and only those three.
    #[test]
    fn every_goto_mode_is_spelled_in_data() {
        let def = SceneDef::parse(
            "S",
            r#"{ "behaviour": "menu", "exits": {
                   "a": { "to": "x", "mode": "replace" },
                   "b": { "to": "y", "mode": "replace_root" },
                   "c": { "to": "z", "mode": "push" } } }"#,
            &builtin_templates(),
        )
        .expect("scene file loads");
        assert_eq!(goto(def.exit("a")), Some(("x".into(), GotoMode::Replace)));
        assert_eq!(goto(def.exit("b")), Some(("y".into(), GotoMode::ReplaceRoot)));
        assert_eq!(goto(def.exit("c")), Some(("z".into(), GotoMode::Push)));

        let bad = SceneDef::parse(
            "S",
            r#"{ "behaviour": "menu", "exits": { "a": { "to": "x", "mode": "pop" } } }"#,
            &builtin_templates(),
        );
        assert!(bad.is_err_and(|e| e.contains("pop")), "an unspellable mode fails loud");
    }

    /// An authored tree goes through the SAME reader + template tier a Lua-declared
    /// screen does: `stat_row` is a data proto, and it must be gone (expanded into
    /// real components) by the time the def is built.
    #[test]
    fn an_authored_tree_parses_and_expands() {
        let def = SceneDef::parse(
            "S",
            r#"{ "behaviour": "menu", "tree": { "component": "screen", "children": [
                   { "template": "stat_row", "caption": "$menu_settings", "value_bind": "v" } ] } }"#,
            &builtin_templates(),
        )
        .expect("scene file loads");
        let tree = def.tree.expect("tree authored");
        assert_eq!(tree.component, "screen");
        fn any_template(n: &UiNode) -> bool {
            n.template.is_some() || n.children.iter().any(any_template)
        }
        assert!(!any_template(&tree), "no unexpanded template survives the load");
        assert!(crate::unknown_kinds(&tree).is_empty(), "the built tree names known kinds only");
    }

    /// Every authored NAME fails LOUD rather than to nothing.
    #[test]
    fn authored_names_that_do_not_resolve_are_load_errors() {
        let reg = builtin_templates();
        let cases = [
            (r#"{ "exits": {} }"#, "no `behaviour`"),
            (r#"{ "behaviour": "" }"#, "an empty `behaviour`"),
            (r#"{ "behaviour": "menu", "exit": { "done": { "to": "x" } } }"#, "unknown top-level key"),
            (r#"{ "behaviour": "menu", "params": [] }"#, "params that are not a map"),
            (r#"{ "behaviour": "menu", "boot": "yes" }"#, "a non-boolean boot"),
            (r#"{ "behaviour": "menu", "tree": { "component": "buttonn" } }"#, "unknown component kind"),
            (r#"{ "behaviour": "menu", "tree": { "template": "no_such_template" } }"#, "unknown template"),
            (r#"{ "behaviour": "menu", "tree": { "id": "x" } }"#, "a node with neither component nor template"),
            (r#"{ "behaviour": "menu", "exits": { "done": {} } }"#, "an exit with no target"),
            (r#"{ "behaviour": "menu", "exits": [] }"#, "exits that are not a map"),
            ("not json at all", "unparseable"),
            ("[]", "a document that is not an object"),
        ];
        for (json, why) in cases {
            assert!(SceneDef::parse("S", json, &reg).is_err(), "{why} must be a load error: {json}");
        }
    }

    /// A file that still restates its own `id` names the fix, because that is the
    /// one stale key every pre-manifest scene file carries.
    #[test]
    fn a_restated_id_says_so() {
        let err = SceneDef::parse(
            "TegLogo",
            r#"{ "id": "teg_logo", "behaviour": "splash" }"#,
            &builtin_templates(),
        )
        .expect_err("a document may not restate its id");
        assert!(err.contains("FILE NAME"), "the error points at the fix: {err}");
    }

    /// The manifest is a ROSTER: ids in, one boot out, nothing named in Rust.
    #[test]
    fn a_manifest_indexes_the_files_and_finds_the_one_boot() {
        let files = vec![
            ("Main".to_string(), r#"{ "behaviour": "menu" }"#.to_string()),
            (
                "TegLogo".to_string(),
                r#"{ "boot": true, "behaviour": "splash",
                     "exits": { "done": { "to": "Main" } } }"#
                    .to_string(),
            ),
        ];
        let m = SceneManifest::from_files(files, &builtin_templates()).expect("manifest loads");
        assert_eq!(m.boot(), "TegLogo", "the FILE says what starts first");
        assert_eq!(m.ids().collect::<Vec<_>>(), ["Main", "TegLogo"], "sorted, stable");
        assert_eq!(m.len(), 2);
        assert!(!m.is_empty());
        assert_eq!(m.get("Main").map(|d| d.behaviour.as_str()), Some("menu"));
        assert!(m.get("nope").is_none(), "an unregistered id resolves to nothing");
    }

    /// Zero boots, two boots, an empty folder and a bad member are all loud.
    #[test]
    fn a_manifest_refuses_an_undefined_entry_point() {
        let reg = builtin_templates();
        let none = SceneManifest::from_files(
            vec![("Main".to_string(), r#"{ "behaviour": "menu" }"#.to_string())],
            &reg,
        );
        assert!(none.is_err_and(|e| e.contains("boot")), "zero boot scenes fails loud");

        let boot = r#"{ "boot": true, "behaviour": "menu" }"#.to_string();
        let two = SceneManifest::from_files(
            vec![("A".to_string(), boot.clone()), ("B".to_string(), boot)],
            &reg,
        );
        assert!(
            two.is_err_and(|e| e.contains("\"A\"") && e.contains("\"B\"")),
            "two boot scenes fails loud, naming both"
        );

        assert!(SceneManifest::from_files(vec![], &reg).is_err(), "an empty roster fails loud");

        let bad = SceneManifest::from_files(
            vec![("Broken".to_string(), "{ nope".to_string())],
            &reg,
        );
        assert!(bad.is_err_and(|e| e.contains("Broken")), "a bad member names itself: ");
    }

    /// `load_dir` reads a real folder: scene files become the roster, everything
    /// else is skipped rather than fatal (a `.DS_Store` must not stop a boot).
    #[test]
    fn load_dir_indexes_a_folder_and_skips_non_scenes() {
        let dir = std::env::temp_dir().join("flicker_scene_manifest_load_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp scenes folder");
        std::fs::write(
            dir.join("Main.scene.json"),
            br#"{ "boot": true, "behaviour": "menu" }"#,
        )
        .unwrap();
        std::fs::write(dir.join(".DS_Store"), b"\0junk").unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a scene").unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();

        let m = SceneManifest::load_dir(&dir, &builtin_templates()).expect("folder indexes");
        assert_eq!(m.ids().collect::<Vec<_>>(), ["Main"], "only scene files join the roster");
        assert_eq!(m.boot(), "Main");

        // A file that IS a scene file but does not load is fatal, and says which.
        std::fs::write(dir.join("Broken.scene.json"), b"{ nope").unwrap();
        let err = SceneManifest::load_dir(&dir, &builtin_templates())
            .expect_err("a broken scene file fails the whole load");
        assert!(err.contains("Broken"), "the error names the file: {err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A missing folder is a loud error, not an empty roster.
    #[test]
    fn a_missing_scenes_folder_is_an_error() {
        let dir = std::env::temp_dir().join("flicker_scene_manifest_absent");
        let _ = std::fs::remove_dir_all(&dir);
        let err = SceneManifest::load_dir(&dir, &builtin_templates())
            .expect_err("no folder = no roster = loud");
        assert!(err.contains("could not be read"), "{err}");
    }
}
