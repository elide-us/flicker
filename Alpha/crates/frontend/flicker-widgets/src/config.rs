//! `config` — the ONE prop-reader surface a Component / template reads its own inputs
//! through. A node's inputs live in its `props` (a `HashMap<String, Value>`); these three
//! readers are the single source for pulling a scalar out of that map, so the walker
//! (which reads a `UiNode`) and the template builders (which read a raw props map) share
//! ONE implementation instead of two copies of the same match. Behaviour: an absent or
//! wrong-variant key yields `None` (or `false` for a flag) — never a panic.

use std::collections::HashMap;

use flicker_script::Value;

/// A text prop, else `None`.
pub(crate) fn text<'a>(props: &'a HashMap<String, Value>, key: &str) -> Option<&'a str> {
    match props.get(key) {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    }
}

/// A numeric prop, else `None`.
pub(crate) fn num(props: &HashMap<String, Value>, key: &str) -> Option<f64> {
    match props.get(key) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

/// A boolean flag — `true` ONLY when the prop is explicitly `Bool(true)` (absent,
/// non-bool, or a literal `false` → `false`).
pub(crate) fn flag(props: &HashMap<String, Value>, key: &str) -> bool {
    matches!(props.get(key), Some(Value::Bool(true)))
}
