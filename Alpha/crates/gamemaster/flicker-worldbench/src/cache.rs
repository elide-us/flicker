//! **THE TILE CACHE** — what the world render leaves behind (direction
//! E8CB0C61, slice B): every hex the tile thread finishes is written to disk
//! as its picture, keyed by the WORLD it was made of, so RENDER WORLD's output
//! survives the session, the MAP page lays cached hexes on the sheet as the
//! view reaches them, and the job resumes where it stopped.
//!
//! The key is the recipe (its JSON, every dial) and the stage the shown world
//! stands at (epochs, era cycles, loop steps, the cut), hashed FNV-1a — a
//! different world is a different directory, never a stale picture on the
//! wrong hex. The cache lives under the user's application-support directory
//! (`flicker/worldtiles/<key>/<cell>.png`), or wherever `FLICKER_WORLDTILES`
//! points. Pictures are the 512² previews the sheet draws (a quarter of the 2K
//! map); the full 2K is the pixel tier's to keep when the level of detail
//! wants it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The application-support name the cache lives under.
pub const APP: &str = "flicker";
/// The environment variable that moves the cache root (tests, external drives).
pub const ROOT_ENV: &str = "FLICKER_WORLDTILES";

/// One world's cache: its directory and the cells already in it.
pub struct TileCache {
    key: String,
    dir: PathBuf,
    cells: HashSet<u32>,
}

/// FNV-1a over bytes — a stable hash across runs and builds, which a cache
/// key must be (the standard library's hasher promises no such thing).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl TileCache {
    /// The cache root: `$FLICKER_WORLDTILES`, else the user's application
    /// support directory for [`APP`], then `worldtiles`.
    pub fn root() -> PathBuf {
        match std::env::var_os(ROOT_ENV) {
            Some(p) => PathBuf::from(p),
            None => flicker_shell::user_settings_dir(APP).join("worldtiles"),
        }
    }

    /// A world's key: the recipe's JSON and its stage, hashed.
    pub fn key(recipe_json: &str, stage: &str) -> String {
        let mut bytes = recipe_json.as_bytes().to_vec();
        bytes.push(0);
        bytes.extend_from_slice(stage.as_bytes());
        format!("{:016x}", fnv1a64(&bytes))
    }

    /// Open (create) the cache of the world `key` names under [`Self::root`],
    /// reading which cells it already holds, and write its manifest (what it
    /// is the cache of).
    pub fn open(key: &str, recipe_json: &str, stage: &str) -> Self {
        Self::open_at(&Self::root(), key, recipe_json, stage)
    }

    /// [`Self::open`] under `root`.
    pub fn open_at(root: &Path, key: &str, recipe_json: &str, stage: &str) -> Self {
        let dir = root.join(key);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(
                "worldbench: tile cache {} cannot be made: {e}",
                dir.display()
            );
        }
        let manifest = dir.join("manifest.json");
        if !manifest.exists() {
            let body = serde_json::json!({ "key": key, "stage": stage, "recipe": recipe_json });
            if let Err(e) = std::fs::write(&manifest, body.to_string()) {
                tracing::warn!("worldbench: tile cache manifest not written: {e}");
            }
        }
        let cells = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name();
                        let name = name.to_str()?;
                        name.strip_suffix(".png")?.parse::<u32>().ok()
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            key: key.to_string(),
            dir,
            cells,
        }
    }

    pub fn key_str(&self) -> &str {
        &self.key
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The cells the cache holds.
    pub fn cells(&self) -> &HashSet<u32> {
        &self.cells
    }

    pub fn has(&self, cell: u32) -> bool {
        self.cells.contains(&cell)
    }

    /// Where `cell`'s picture lives (whether or not it is there yet).
    pub fn path(&self, cell: u32) -> PathBuf {
        self.dir.join(format!("{cell}.png"))
    }

    /// Note a cell written by the tile thread.
    pub fn landed(&mut self, cell: u32) {
        self.cells.insert(cell);
    }

    /// Write a picture (RGBA, `dim`²) as `cell`'s PNG — the tile thread's call.
    pub fn write_picture(path: &Path, rgba: &[u8], dim: u32) -> Result<(), String> {
        let img: image::RgbaImage = image::ImageBuffer::from_raw(dim, dim, rgba.to_vec())
            .ok_or_else(|| "picture buffer".to_string())?;
        img.save(path).map_err(|e| e.to_string())
    }

    /// Read `cell`'s picture back: RGBA and its dimension.
    pub fn read_picture(path: &Path) -> Option<(Vec<u8>, u32)> {
        let img = image::open(path).ok()?.into_rgba8();
        let (w, h) = img.dimensions();
        if w != h {
            return None;
        }
        Some((img.into_raw(), w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A world's cache is its own directory**: the key is stable, two
    /// worlds never share one, a written picture reads back and is listed on
    /// the next open.
    #[test]
    fn a_world_has_its_own_cache_and_pictures_come_back() {
        let tmp = std::env::temp_dir().join(format!("wb-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let key = TileCache::key("{\"seed\":1}", "epochs0");
        assert_eq!(key, TileCache::key("{\"seed\":1}", "epochs0"), "stable");
        assert_ne!(key, TileCache::key("{\"seed\":2}", "epochs0"));
        assert_ne!(key, TileCache::key("{\"seed\":1}", "epochs1"));
        let mut cache = TileCache::open_at(&tmp, &key, "{\"seed\":1}", "epochs0");
        assert!(cache.cells().is_empty());
        assert!(cache.dir().starts_with(&tmp));
        let rgba: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 7 % 256) as u8).collect();
        TileCache::write_picture(&cache.path(42), &rgba, 4).expect("written");
        cache.landed(42);
        assert!(cache.has(42));
        let (back, dim) = TileCache::read_picture(&cache.path(42)).expect("read back");
        assert_eq!((dim, back), (4, rgba));
        let again = TileCache::open_at(&tmp, &key, "{\"seed\":1}", "epochs0");
        assert!(again.has(42), "listed on the next open");
        assert!(again.dir().join("manifest.json").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
