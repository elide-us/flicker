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

/// The **Model-channel strings gate** — the drift gate for the publish seam the
/// tree-walking `raw_display_literals` is structurally blind to: display copy a
/// scene pushes from Rust into the Model (`m.set("k", "BONE MAP")`), which then
/// reaches the screen through `text_bind` and bypasses the stringtable (and
/// therefore every locale). A gate must cover the CHANNEL the drift travels, so
/// this one scans the SCENE'S OWN SOURCE — a crate self-gates with
/// `raw_model_publish_literals(include_str!("lib.rs"))` in its screen test.
///
/// Flagged: a string LITERAL (or the literal inside a `format!(…)`) passed as
/// the VALUE of a `.set(…)` / `.with(…)` publish, when it reads as display copy —
/// it contains a space next to letters, an ALL-CAPS word, or is a Titlecase
/// word. Exempt (the value shapes that are data, not copy): `$token`s (resolve
/// them instead), no-alphabetic strings, single glyphs, dotted style/section
/// paths, `lower_snake`/`kebab-case` identifiers (enum-ish data values), and
/// `%`-format strings. A deliberate exception carries
/// `// strings-gate-exempt: <reason>` on the SAME line, mirroring the
/// launcher's sanctioned composed-string precedent.
pub fn raw_model_publish_literals(rust_src: &str) -> Vec<String> {
    fn is_display_copy(v: &str) -> bool {
        if v.starts_with('$') || v.starts_with('%') || v.chars().count() <= 1 {
            return false;
        }
        if !v.chars().any(|c| c.is_alphabetic()) {
            return false;
        }
        // Dotted paths ("settings.nav.active") and identifier-shaped data values
        // ("walk", "en-us", "wf_next") are wiring, not copy.
        if v.contains('.') && !v.contains(' ') {
            return false;
        }
        if v.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return false;
        }
        // Strip format placeholders so "{}/{} pts" judges only its words.
        let mut plain = String::with_capacity(v.len());
        let mut depth = 0usize;
        for c in v.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ if depth == 0 => plain.push(c),
                _ => {}
            }
        }
        let spaced_words =
            plain.contains(' ') && plain.chars().filter(|c| c.is_alphabetic()).count() >= 3;
        let caps_word = plain
            .split(|c: char| !c.is_ascii_uppercase())
            .any(|w| w.len() >= 2);
        let titlecase = {
            let t = plain.trim();
            let mut ch = t.chars();
            matches!(ch.next(), Some(f) if f.is_ascii_uppercase())
                && t.len() >= 3
                && ch.all(|c| c.is_ascii_lowercase())
        };
        spaced_words || caps_word || titlecase
    }

    /// The first string literal in `s`, with its end offset (no escape handling
    /// beyond `\"` — publish copy in this codebase is plain).
    fn first_literal(s: &str) -> Option<(String, usize)> {
        let start = s.find('"')?;
        let mut out = String::new();
        let mut prev_backslash = false;
        for (i, c) in s[start + 1..].char_indices() {
            match c {
                '"' if !prev_backslash => return Some((out, start + 1 + i)),
                '\\' if !prev_backslash => prev_backslash = true,
                _ => {
                    prev_backslash = false;
                    out.push(c);
                }
            }
        }
        None
    }

    let mut flagged = Vec::new();
    for (n, line) in rust_src.lines().enumerate() {
        if line.contains("strings-gate-exempt") {
            continue;
        }
        for call in [".set(", ".with("] {
            let mut offset = 0usize;
            while let Some(at) = line[offset..].find(call) {
                offset += at + call.len();
                let after = &line[offset..];
                // Skip the KEY argument (a plain literal, else up to the first comma —
                // heuristic: exotic key expressions fall out as un-parsed, never as a
                // false flag).
                let value = match first_literal(after) {
                    Some((_, key_end)) if after[..key_end].find(',').is_none() => {
                        after[key_end + 1..].trim_start_matches([',', ' ']).trim_start()
                    }
                    _ => match after.find(',') {
                        Some(c) => after[c + 1..].trim_start(),
                        None => "",
                    },
                };
                let lit = if let Some(fmt) = value.strip_prefix("format!(") {
                    first_literal(fmt).map(|(v, _)| v)
                } else if value.starts_with('"') {
                    first_literal(value).map(|(v, _)| v)
                } else {
                    None
                };
                if let Some(v) = lit {
                    if is_display_copy(&v) {
                        flagged.push(format!("line {}: `{}`", n + 1, v));
                    }
                }
            }
        }
    }
    flagged
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

    /// The Model-channel gate flags publish-seam display copy and nothing else —
    /// it has to be able to FAIL (raw English) and to PASS every legitimate data
    /// value shape, or scenes cannot self-gate with it.
    #[test]
    fn model_publish_gate_flags_copy_and_passes_data() {
        let src = r#"
            m.set("hint", "BONE MAP");
            m.set("status", format!("Blocked: {}", err));
            m.set("caption", "Reference body");
            m.set("title", strings::resolve("$ap_bone_map").into_owned());
            m.set("nav_video_style", "settings.nav.active");
            m.set("mode", "walk");
            m.set("value", format!("{:.2}", v));
            m.set("count", 3.0);
            m.set("mark", "✕");
            m.set("pct", "%d");
            m.set("roster", format!("{count} scenes available")); // strings-gate-exempt: composed-dynamic, no format language
            let model = ValueMap::new().with("banner", "SETTINGS APPLIED");
        "#;
        let flags = raw_model_publish_literals(src);
        let all = flags.join("\n");
        assert!(all.contains("BONE MAP"), "all-caps copy is flagged: {all}");
        assert!(all.contains("Blocked: "), "English inside format! is flagged");
        assert!(all.contains("Reference body"), "spaced Titlecase copy is flagged");
        assert!(all.contains("SETTINGS APPLIED"), ".with copy is flagged");
        assert_eq!(flags.len(), 4, "…and nothing else: {all}");
    }

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
