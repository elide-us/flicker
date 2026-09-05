//! **Declarative intents** — S9 of the Prism UI convergence: "the UI is the
//! declaration". A screen's ROOT node declares, as DATA, which input signals it
//! binds and what each firing means, as `on_<signal>: "result_name"` props
//! (`on_menu = "pause_open"`, `on_cancel = "settings_close"`). [`UiIntents::of`]
//! collects that declaration once per tree build; the walker layer
//! ([`WalkerHandler::with_intents`](crate::WalkerHandler::with_intents)) then
//! consumes a declared signal's events on the bus and records the fired result
//! name, and the scene folds the fired names into its results exactly like a
//! click — plus the read-only mirror: each fired name is republished into the
//! next Model as `sig_<name> = true` ([`UiIntents::mirror_into`]) for scripts
//! to observe.
//!
//! The Router stays the ONE routing authority: this module never dispatches —
//! it only carries the declaration from the tree to the walker layer, which
//! sits in the ordinary chain (capture ▸ target ▸ bubble). A layer above the
//! walker (the exclusive `TextEntry` keyboard owner, a modal) that consumes a
//! signal therefore still starves the intent, structurally — e.g. pocclusters'
//! chat keeps swallowing `Menu` while it owns the keyboard. Text entry itself is
//! walker-owned (`EnterText` / `SubmitText` / `CancelText`, Aaron 2026-09-03), so
//! a screen names those only to override the walker's default meaning.
//!
//! # The prop convention
//!
//! The prop key suffix is the signal's stable NAME in the tree's own casing:
//! Lua/JSON props are snake_case, so `on_attack_light` names `AttackLight` —
//! the suffix is folded segment-by-segment onto the ONE frozen naming
//! ([`ActionSignal::name`], the serde variant names). There is no second name
//! table: an unknown suffix is warned and skipped (the vocabulary-gate
//! philosophy — a typo is a log line, never a silent dead binding), as is a
//! non-text value.
//!
//! # Cache discipline
//!
//! The declaration lives on the ROOT node only and is immutable data, so a
//! scene builds [`UiIntents`] exactly when it (re)builds its tree and retains
//! it beside `UiState` — never per frame.

use flicker_input_core::ActionSignal;
use flicker_script::{UiNode, Value, ValueMap};

/// A screen's collected signal→result-name bindings (the S9 declaration).
/// Build with [`of`](Self::of) when the tree is built; hand to the walker layer
/// with [`WalkerHandler::with_intents`](crate::WalkerHandler::with_intents).
#[derive(Clone, Debug, Default)]
pub struct UiIntents {
    /// `(signal, result name)` pairs, one per declared `on_<signal>` prop.
    map: Vec<(ActionSignal, String)>,
}

/// Prefix a fired intent name carries in the Model mirror (`sig_<name>`).
const SIG_PREFIX: &str = "sig_";

impl UiIntents {
    /// Collect the `on_<signal>` props of `tree`'s ROOT node — the screen is the
    /// declaring unit, so only the root is scanned. An unknown signal suffix or
    /// a non-text value is warned and skipped (vocabulary gate); the tree is
    /// otherwise untouched. Call once per tree build and retain the result.
    pub fn of(tree: &UiNode) -> Self {
        let mut map = Vec::new();
        for (key, value) in &tree.props {
            let Some(suffix) = key.strip_prefix("on_") else {
                continue;
            };
            let Some(signal) = ActionSignal::from_name(&snake_to_pascal(suffix)) else {
                tracing::warn!(
                    "ui intents: `{key}` names no ActionSignal (root `{}`) — skipped",
                    tree.id
                );
                continue;
            };
            match value {
                Value::Text(name) if !name.is_empty() => {
                    map.push((signal, name.clone()));
                }
                other => {
                    tracing::warn!(
                        "ui intents: `{key}` must be a non-empty result-name string, got {other:?} — skipped"
                    );
                }
            }
        }
        Self { map }
    }

    /// The declared result name for `signal`, if the screen bound it.
    pub fn result_for(&self, signal: ActionSignal) -> Option<&str> {
        self.map
            .iter()
            .find(|(s, _)| *s == signal)
            .map(|(_, name)| name.as_str())
    }

    /// `true` when the screen declared no bindings (the walker layer then
    /// consumes nothing on this path).
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The read-only mirror (S9 ruling): republish each fired intent name into
    /// `m` as `sig_<name> = true`, for scripts to observe. **Transient by
    /// contract**: a fired name rides exactly ONE Model publish (the first one
    /// after it fired — scenes build their Model before dispatch, so in
    /// practice the next frame's) and is then dropped; a script that needs the
    /// fact longer latches it itself. Absent keys simply read falsy in Lua, so
    /// no `false` is ever written.
    pub fn mirror_into(m: &mut ValueMap, fired: &[String]) {
        for name in fired {
            m.set(format!("{SIG_PREFIX}{name}"), true);
        }
    }
}

/// Fold a snake_case prop suffix onto the frozen PascalCase signal naming
/// (`attack_light` → `AttackLight`). Purely mechanical — the frozen table
/// stays [`ActionSignal::name`]'s serde variant names; this is a casing fold,
/// not a second vocabulary.
fn snake_to_pascal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for segment in s.split('_') {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_script::UiNode;

    fn screen_with(props: &[(&str, Value)]) -> UiNode {
        let mut n = UiNode {
            component: "surface".into(),
            id: "root".into(),
            ..Default::default()
        };
        for (k, v) in props {
            n.props.insert((*k).to_string(), v.clone());
        }
        n
    }

    #[test]
    fn collects_root_on_props_including_multi_segment_names() {
        let tree = screen_with(&[
            ("on_menu", Value::Text("pause_open".into())),
            ("on_cancel", Value::Text("settings_close".into())),
            ("on_attack_light", Value::Text("combo".into())),
            ("style", Value::Text("hud".into())), // not an intent prop
        ]);
        let intents = UiIntents::of(&tree);
        assert!(!intents.is_empty());
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));
        assert_eq!(
            intents.result_for(ActionSignal::Cancel),
            Some("settings_close")
        );
        assert_eq!(intents.result_for(ActionSignal::AttackLight), Some("combo"));
        assert_eq!(intents.result_for(ActionSignal::Confirm), None);
    }

    #[test]
    fn vocabulary_gate_skips_unknown_and_malformed_declarations() {
        let tree = screen_with(&[
            ("on_bogus_signal", Value::Text("x".into())), // no such signal → skip
            ("on_menu", Value::Number(3.0)),              // not a name string → skip
            ("on_cancel", Value::Text(String::new())),    // empty name → skip
        ]);
        let intents = UiIntents::of(&tree);
        assert!(
            intents.is_empty(),
            "every malformed declaration is skipped, never a panic"
        );
    }

    #[test]
    fn only_the_root_declares() {
        // A child's on_* props are NOT part of the screen declaration — the
        // screen is the declaring unit (child props stay component props).
        let mut child = UiNode {
            component: "cell".into(),
            ..Default::default()
        };
        child
            .props
            .insert("on_menu".into(), Value::Text("nope".into()));
        let mut tree = screen_with(&[("on_cancel", Value::Text("back".into()))]);
        tree.children.push(child);
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), None);
        assert_eq!(intents.result_for(ActionSignal::Cancel), Some("back"));
    }

    #[test]
    fn mirror_publishes_sig_keys_transiently() {
        let mut m = ValueMap::new();
        UiIntents::mirror_into(
            &mut m,
            &["pause_open".to_string(), "settings_close".to_string()],
        );
        assert!(m.is_on("sig_pause_open"));
        assert!(m.is_on("sig_settings_close"));
        // Nothing fired → nothing written (absent keys read falsy; no `false` spam).
        let mut quiet = ValueMap::new();
        UiIntents::mirror_into(&mut quiet, &[]);
        assert!(quiet.get("sig_pause_open").is_none());
    }
}
