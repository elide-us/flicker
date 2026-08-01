//! The UI **string table** — every UI display string is a token in
//! `Alpha/content/data/stringtable.json` (`{ "<token>": { "en-us": "…" } }`), and the
//! active language (a tier-3 player setting, default `en-us`) selects the text
//! (text ruling, 2026-07-31). Localization is expected to move into the database
//! layer later; the flat token→locale→text shape makes that a data-source swap.
//!
//! Resolution happens at the walker's DRAW boundary (`node_text`, `component_props`,
//! placeholders), so Lua components and Rust-built trees receive FINAL text through
//! the same helpers, and only display strings resolve — `bind` values and user text
//! (chat) never do. `$token` resolves; `$$` escapes a literal `$`; a missing token
//! passes through RAW — visibly wrong on screen and greppable, the strings analog of
//! the `unknown_kinds` vocabulary gate — and warns once per token. A (re)load bumps
//! [`generation`], which every node fingerprint folds in, so a language switch
//! invalidates exactly the cached text it changes.
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

static TABLE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static GENERATION: AtomicU32 = AtomicU32::new(0);
static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn table() -> &'static RwLock<HashMap<String, String>> {
    TABLE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Monotonic load counter. The draw cache folds this into every node fingerprint so a
/// stringtable (re)load — a language change — redraws exactly the nodes showing text.
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

/// Flatten `{ token: { locale: text } }` to `token → text` for `locale`, falling back
/// per token to `en-us` (the seed locale) when the requested locale has no entry.
pub fn flatten(json: &str, locale: &str) -> Result<HashMap<String, String>, serde_json::Error> {
    let root: HashMap<String, HashMap<String, String>> = serde_json::from_str(json)?;
    Ok(root
        .into_iter()
        .filter_map(|(token, locales)| {
            locales
                .get(locale)
                .or_else(|| locales.get("en-us"))
                .cloned()
                .map(|text| (token, text))
        })
        .collect())
}

/// (Re)load the active table from stringtable JSON for `locale`. Malformed JSON warns
/// and leaves the previous table in place (a bad edit never blanks the UI).
pub fn load_str(json: &str, locale: &str) {
    match flatten(json, locale) {
        Ok(map) => {
            if let Ok(mut t) = table().write() {
                *t = map;
            }
            GENERATION.fetch_add(1, Ordering::Relaxed);
        }
        Err(e) => tracing::warn!("stringtable did not parse ({e}); keeping the previous table"),
    }
}

/// Resolve one display string: `$token` → the active locale's text; `$$…` → a literal
/// `$…`; anything else passes through untouched. A missing token passes through RAW
/// (visible + greppable) and warns once.
pub fn resolve(s: &str) -> Cow<'_, str> {
    let Some(rest) = s.strip_prefix('$') else {
        return Cow::Borrowed(s);
    };
    if let Some(lit) = rest.strip_prefix('$') {
        return Cow::Owned(format!("${lit}"));
    }
    if let Some(hit) = table().read().ok().and_then(|t| t.get(rest).cloned()) {
        return Cow::Owned(hit);
    }
    let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut w) = warned.lock() {
        if w.insert(rest.to_string()) {
            tracing::warn!("string token '{s}' has no entry in the stringtable — rendering it raw");
        }
    }
    Cow::Borrowed(s)
}

/// Serializes tests that LOAD the process-wide table (`load_str` replaces it), so
/// parallel test threads never assert against each other's locale.
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One combined test: the global table is process-wide state, so the load and
    /// every assertion against it serialize inside a single test body.
    #[test]
    fn flatten_load_and_resolve() {
        let _g = test_guard();
        let json = r#"{
            "t_hello": { "en-us": "HELLO", "es-sp": "HOLA" },
            "t_only_en": { "en-us": "ONLY" }
        }"#;

        // flatten: exact locale, then per-token en-us fallback.
        let es = flatten(json, "es-sp").unwrap();
        assert_eq!(es.get("t_hello").map(String::as_str), Some("HOLA"));
        assert_eq!(es.get("t_only_en").map(String::as_str), Some("ONLY"), "falls back to en-us");

        // load + resolve through the global.
        let g0 = generation();
        load_str(json, "en-us");
        assert!(generation() > g0, "a load bumps the generation");
        assert_eq!(resolve("$t_hello"), "HELLO");
        assert_eq!(resolve("plain"), "plain", "non-sigil text passes through");
        assert_eq!(resolve("$$5.00"), "$5.00", "$$ escapes a literal $");
        assert_eq!(resolve("$t_missing"), "$t_missing", "a miss renders raw (the visible gate)");

        // Malformed reload keeps the previous table.
        load_str("{ not json", "en-us");
        assert_eq!(resolve("$t_hello"), "HELLO", "bad JSON never blanks the table");

        // The shipped seed parses and carries the shell slice.
        let shipped = include_str!("../../../../content/data/stringtable.json");
        let en = flatten(shipped, "en-us").expect("shipped stringtable parses");
        assert_eq!(en.get("menu_quit").map(String::as_str), Some("QUIT"));
    }
}
