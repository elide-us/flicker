//! The mounted content package — `package.flk` serving reads in place of the
//! loose `content/package/` tree.
//!
//! An installed build ships the package tree as ONE store-only container (zip,
//! all entries Stored — see `flicker-content`'s `pack` module for the writer).
//! Mounting it here makes every loader package-capable at once, because all
//! package reads already funnel through the seam functions in
//! [`crate::compression`]: those functions consult this mount FIRST and fall
//! back to the filesystem, mirroring the gz-first/raw-fallback precedence the
//! seam already applies (shipped form wins; loose files are the dev
//! convenience).
//!
//! The mount is process-global and read-only, set once at startup by
//! `roots::init_from_app_dir` when a `package.flk` sits beside the content
//! root. A dev tree has no package file → nothing mounts → the seam behaves
//! byte-for-byte as before. Writers never touch the mount: staging and
//! promotion work on the filesystem, packing is an offline `content-tool` pass.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use zip::ZipArchive;

struct PackageMount {
    /// The directory this package REPLACES (`<content_root>/package`) —
    /// logical paths under it resolve into the archive.
    root: PathBuf,
    /// Entry names in sorted order — the existence + directory-listing index
    /// (reads go through the archive's own by-name lookup).
    names: Vec<String>,
    archive: ZipArchive<BufReader<File>>,
}

static MOUNT: Mutex<Option<PackageMount>> = Mutex::new(None);

/// Mount `flk` as the package tree rooted at `mount_root` (the path loaders
/// address, normally `roots().package()`). Returns the entry count. Replaces
/// any previous mount — there is ONE package per process.
pub fn mount_package(flk: &Path, mount_root: &Path) -> io::Result<usize> {
    let archive = ZipArchive::new(BufReader::new(File::open(flk)?))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    names.sort_unstable();
    let count = names.len();
    *MOUNT.lock().expect("package mount lock") = Some(PackageMount {
        root: mount_root.to_path_buf(),
        names,
        archive,
    });
    Ok(count)
}

/// Drop the mount (tests; the running game never unmounts).
pub fn unmount() {
    *MOUNT.lock().expect("package mount lock") = None;
}

/// `path` relative to `root` as a forward-slash entry name, when it is under it.
fn rel_name(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let s = rel.to_str()?;
    if s.is_empty() {
        return Some(String::new()); // the mount root itself (listing)
    }
    Some(if std::path::MAIN_SEPARATOR == '/' {
        s.to_owned()
    } else {
        s.replace('\\', "/")
    })
}

/// The at-rest entry name a LOGICAL name resolves to, mirroring the seam's
/// gz-first rule: a name already ending `.gz` resolves to itself; otherwise
/// `<name>.gz` wins over the raw name.
fn resolve(names: &[String], logical: &str) -> Option<String> {
    let contains = |name: &str| names.binary_search_by(|n| n.as_str().cmp(name)).is_ok();
    if logical.ends_with(".gz") {
        return contains(logical).then(|| logical.to_owned());
    }
    [format!("{logical}.gz"), logical.to_owned()]
        .into_iter()
        .find(|candidate| contains(candidate))
}

/// Read the RAW STORED BYTES for the logical `path` when the mount serves it:
/// `Ok(Some(bytes))` on a hit (bytes are the at-rest form — the caller applies
/// the same sniff/decode it applies to file bytes), `Ok(None)` when nothing is
/// mounted / the path is outside the mount / no entry matches (the caller
/// falls through to the filesystem), `Err` when the entry exists but cannot be
/// read — a corrupt shipped package fails loud, it does not fall back.
pub(crate) fn read_raw(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let mut guard = MOUNT.lock().expect("package mount lock");
    let Some(mount) = guard.as_mut() else {
        return Ok(None);
    };
    let Some(logical) = rel_name(&mount.root, path) else {
        return Ok(None);
    };
    let Some(name) = resolve(&mount.names, &logical) else {
        return Ok(None);
    };
    let mut entry = mount.archive.by_name(&name).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("package entry {name}: {e}"),
        )
    })?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("package entry {name}: {e}"),
        )
    })?;
    Ok(Some(bytes))
}

/// Gz-transparent existence in the mount — the mount half of
/// [`crate::compression::file_exists`].
pub(crate) fn exists(path: &Path) -> bool {
    let guard = MOUNT.lock().expect("package mount lock");
    let Some(mount) = guard.as_ref() else {
        return false;
    };
    let Some(logical) = rel_name(&mount.root, path) else {
        return false;
    };
    resolve(&mount.names, &logical).is_some()
}

/// The mounted children of `dir`, as `(name, is_dir)` pairs — `None` when the
/// mount cannot answer (not mounted / outside the root / no such directory),
/// so the caller lets the filesystem decide. File names are the AT-REST names
/// (`x.json.gz`), exactly what a filesystem listing of the package tree shows.
pub(crate) fn list(dir: &Path) -> Option<Vec<(String, bool)>> {
    let guard = MOUNT.lock().expect("package mount lock");
    let mount = guard.as_ref()?;
    let rel = rel_name(&mount.root, dir)?;
    let prefix = if rel.is_empty() {
        String::new()
    } else {
        format!("{rel}/")
    };

    let start = mount
        .names
        .partition_point(|n| n.as_str() < prefix.as_str());
    let mut children: Vec<(String, bool)> = Vec::new();
    for name in &mount.names[start..] {
        let Some(rest) = name.strip_prefix(&prefix) else {
            break;
        };
        let (child, is_dir) = match rest.split_once('/') {
            Some((first, _)) => (first, true),
            None => (rest, false),
        };
        // Sorted input ⇒ a directory's entries are adjacent; dedupe the run.
        if children.last().map(|(n, _)| n.as_str()) != Some(child) {
            children.push((child.to_owned(), is_dir));
        }
    }
    if children.is_empty() {
        return None; // the mount has no such directory — the filesystem decides
    }
    Some(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Name resolution is the seam's precedence in miniature: gz twin first,
    /// raw fallback, explicit .gz names verbatim.
    #[test]
    fn resolve_prefers_the_gz_twin() {
        let names = vec![
            "a/plain.png".to_owned(),
            "a/twin.json.gz".to_owned(),
            "b/both.json".to_owned(),
            "b/both.json.gz".to_owned(),
        ];
        assert_eq!(
            resolve(&names, "a/twin.json").as_deref(),
            Some("a/twin.json.gz")
        );
        assert_eq!(
            resolve(&names, "a/twin.json.gz").as_deref(),
            Some("a/twin.json.gz")
        );
        assert_eq!(
            resolve(&names, "a/plain.png").as_deref(),
            Some("a/plain.png")
        );
        assert_eq!(
            resolve(&names, "b/both.json").as_deref(),
            Some("b/both.json.gz")
        );
        assert_eq!(resolve(&names, "a/missing.json"), None);
    }
}
