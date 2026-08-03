//! Where THIS executable's content lives — the per-app content roots.
//!
//! Every crate used to hardcode its own climb to the content tree
//! (`concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/package/characters")`),
//! which bakes the repo layout into ~50 call sites and makes the tree
//! un-relocatable. The data-interface rule is the opposite: a module asks the
//! interface where content is, it never spells out a path.
//!
//! An executable declares its root ONCE, in a committed `content.json` beside
//! its crate manifest:
//!
//! ```json
//! { "content_root": "../content" }
//! ```
//!
//! The path is relative to the app's own directory (so `Alpha/prism-alpha` +
//! `../content` = `Alpha/content`); an absolute path is taken as-is. The
//! sub-roots are **DERIVED**, never separately configured — there is one knob,
//! so `staging/` and `package/` can never drift onto different trees.
//!
//! # The two roots that matter for the content pipeline
//!
//! - [`ContentRoots::staging`] — where the ingest benches (Clayworks,
//!   Loomforge, the retargeter) WRITE their processed output. Not shipped, not
//!   read by the running game.
//! - [`ContentRoots::package`] — the runtime read set. Content only arrives
//!   here by PROMOTION from staging (the Content Manager's job), never by a
//!   bench writing straight into it.
//!
//! Both trees are gz-at-rest, so a promotion is a plain byte move through the
//! [`crate::package`] seam — no transcoding step.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The per-executable content declaration, read from the app's own directory.
pub const CONTENT_CONFIG_FILE: &str = "content.json";

/// Where an app's content sits when it ships no `content.json` — the repo
/// layout every client happens to use (`<app>/../content`).
const DEFAULT_CONTENT_ROOT: &str = "../content";

fn default_content_root() -> String {
    DEFAULT_CONTENT_ROOT.to_string()
}

/// The committed, per-executable content declaration (`content.json`).
///
/// Distinct from the per-user `settings.json`, which is runtime state and
/// gitignored: this file is versioned with the app because it describes the
/// app's SHAPE, not a player's preferences.
///
/// Every field carries `#[serde(default)]` so an older file still loads once a
/// field is added — same discipline as the shell's `GameSettings`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentConfig {
    /// The content tree's location, relative to the app directory (or
    /// absolute). Sub-roots are derived from it.
    #[serde(default = "default_content_root")]
    pub content_root: String,
}

impl Default for ContentConfig {
    fn default() -> Self {
        Self { content_root: default_content_root() }
    }
}

impl ContentConfig {
    /// Read `<app_dir>/content.json`. **Best-effort**: a missing file is the
    /// normal case for an app that wants the default layout, and an invalid
    /// one is a warning, never a hard failure — a malformed config must not
    /// stop the app from starting.
    pub fn load_from(app_dir: &Path) -> Self {
        let path = app_dir.join(CONTENT_CONFIG_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(cfg) => {
                tracing::info!("content config: {}", path.display());
                cfg
            }
            Err(e) => {
                tracing::warn!("ignoring invalid {}: {e}", path.display());
                Self::default()
            }
        }
    }
}

/// The resolved content tree: one root, and the sub-roots derived from it.
///
/// Cheap to clone and to build — call [`roots`] wherever you need a path
/// rather than threading one through a call chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentRoots {
    root: PathBuf,
}

impl ContentRoots {
    /// Wrap an already-resolved content root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve a config against the app's directory: a relative `content_root`
    /// hangs off `app_dir`, an absolute one is taken as-is.
    pub fn resolve(app_dir: &Path, cfg: &ContentConfig) -> Self {
        let declared = Path::new(&cfg.content_root);
        let root =
            if declared.is_absolute() { declared.to_path_buf() } else { app_dir.join(declared) };
        Self::new(root)
    }

    /// The content tree itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The runtime read set — reached by PROMOTION only.
    pub fn package(&self) -> PathBuf {
        self.root.join("package")
    }

    /// Where the ingest benches write their processed output.
    pub fn staging(&self) -> PathBuf {
        self.root.join("staging")
    }

    /// RPC-tier tables (`periodic_table.json`, `stringtable.json`, …).
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// Host-loaded UI content (`scripts/`, `resources/`).
    pub fn sensorium(&self) -> PathBuf {
        self.root.join("sensorium")
    }

    /// Raw vendor exports — authoring input that never ships.
    pub fn source(&self) -> PathBuf {
        self.root.join("source")
    }
}

/// Set once at startup by the running executable; `None` until then.
static CONTENT_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Point the process at a content tree directly. Prefer [`init_from_app_dir`],
/// which reads the app's `content.json`; this is the escape hatch for tests and
/// for tools that are handed a tree on the command line.
pub fn set_content_root(root: Option<PathBuf>) {
    *CONTENT_ROOT.lock().expect("content root lock") = root;
}

/// Read `<app_dir>/content.json`, install the resolved root for the process,
/// and hand it back. Call once at startup, before anything touches content.
pub fn init_from_app_dir(app_dir: &Path) -> ContentRoots {
    let roots = ContentRoots::resolve(app_dir, &ContentConfig::load_from(app_dir));
    set_content_root(Some(roots.root().to_path_buf()));
    tracing::info!("content root: {}", roots.root().display());
    roots
}

/// The process's content roots.
///
/// Falls back to this crate's own position in the repo when no executable has
/// declared a root — which is what keeps the library's tests and the offline
/// `content-tool` working outside an app. That fallback is the ONE remaining
/// hardcoded climb in the content path, and it lives here rather than in every
/// caller.
pub fn roots() -> ContentRoots {
    let declared = CONTENT_ROOT.lock().expect("content root lock").clone();
    ContentRoots::new(declared.unwrap_or_else(|| {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_roots_derive_from_the_one_knob() {
        let r = ContentRoots::new("/tmp/tree");
        assert_eq!(r.package(), Path::new("/tmp/tree/package"));
        assert_eq!(r.staging(), Path::new("/tmp/tree/staging"));
        assert_eq!(r.data(), Path::new("/tmp/tree/data"));
        assert_eq!(r.source(), Path::new("/tmp/tree/source"));
        // The point of deriving: staging and package can never name different trees.
        assert_eq!(r.staging().parent(), r.package().parent());
    }

    #[test]
    fn a_relative_root_hangs_off_the_app_dir_and_an_absolute_one_wins() {
        let cfg = ContentConfig { content_root: "../content".into() };
        let r = ContentRoots::resolve(Path::new("/repo/Alpha/prism-alpha"), &cfg);
        assert_eq!(r.package(), Path::new("/repo/Alpha/prism-alpha/../content/package"));

        let abs = ContentConfig { content_root: "/srv/content".into() };
        let r = ContentRoots::resolve(Path::new("/repo/Alpha/prism-alpha"), &abs);
        assert_eq!(r.root(), Path::new("/srv/content"));
    }

    /// A missing file is the normal "wants the default layout" case, and an
    /// invalid one must not stop an app from starting.
    #[test]
    fn a_missing_or_invalid_config_falls_back_to_the_default() {
        let dir = std::env::temp_dir().join("flicker_roots_cfg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(ContentConfig::load_from(&dir).content_root, DEFAULT_CONTENT_ROOT);

        std::fs::write(dir.join(CONTENT_CONFIG_FILE), b"{ not json").unwrap();
        assert_eq!(ContentConfig::load_from(&dir).content_root, DEFAULT_CONTENT_ROOT);

        std::fs::write(dir.join(CONTENT_CONFIG_FILE), br#"{"content_root":"../elsewhere"}"#)
            .unwrap();
        assert_eq!(ContentConfig::load_from(&dir).content_root, "../elsewhere");

        // An unknown/absent field must still load — the forward-compat discipline.
        std::fs::write(dir.join(CONTENT_CONFIG_FILE), br#"{}"#).unwrap();
        assert_eq!(ContentConfig::load_from(&dir).content_root, DEFAULT_CONTENT_ROOT);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The un-declared fallback must still land on a real content tree, or every
    /// library test that reads content breaks.
    #[test]
    fn the_undeclared_fallback_finds_the_repo_tree() {
        set_content_root(None);
        let r = roots();
        assert!(r.data().join("periodic_table.json").exists(), "fallback root: {:?}", r.root());
    }
}
