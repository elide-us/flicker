//! The pack **library** — what the Pack Browser browses.
//!
//! A "pack" on disk is a `*.pack.json` ([`PackFile`]): a state machine plus its authored
//! provenance. This module scans a content root for them and derives a one-line summary of
//! each **from the file itself** — nothing here is invented. The browser shows real packs.
//!
//! **What is deliberately NOT modelled.** The design's pack *type* (Locomotion / Weapon /
//! Ability / Creature) and *movesets* are combat taxonomy with no runtime behind them yet
//! (`flicker-mechanics` is still the geometry layer; hitbox↔hurtbox resolution is a later
//! slice). Rather than invent a schema the engine cannot honour, `kind` is **derived from
//! what the pack actually contains** and carries an `Inferred` fallback, so the browser can
//! group packs without any file claiming a gameplay category it cannot back up.

use std::path::{Path, PathBuf};

use flicker_skeletal::state::{read_pack, PackFile};

/// How a pack is grouped in the filter rail. Derived, never authored — see the module note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackKind {
    /// Movement/idle-dominated: the locomotion core of a character.
    Locomotion,
    /// Carries attack states with authored TAE combat windows.
    Weapon,
    /// Has TAE events but no combat metadata — utility/ability behaviour.
    Ability,
    /// Anything else (small or structural packs).
    Creature,
}

impl PackKind {
    pub const ALL: [PackKind; 4] =
        [PackKind::Locomotion, PackKind::Weapon, PackKind::Ability, PackKind::Creature];

    pub fn label(self) -> &'static str {
        match self {
            PackKind::Locomotion => "Locomotion",
            PackKind::Weapon => "Weapon",
            PackKind::Ability => "Ability",
            PackKind::Creature => "Creature",
        }
    }

    /// The design's per-type accent, as a `loomforge.pack_kind.*` style path.
    pub fn color_path(self) -> &'static str {
        match self {
            PackKind::Locomotion => "loomforge.pack_kind.locomotion.color",
            PackKind::Weapon => "loomforge.pack_kind.weapon.color",
            PackKind::Ability => "loomforge.pack_kind.ability.color",
            PackKind::Creature => "loomforge.pack_kind.creature.color",
        }
    }
}

/// One pack in the library — every field read or derived from the file on disk.
#[derive(Clone, Debug)]
pub struct PackEntry {
    pub path: PathBuf,
    /// Display name: the file stem with `.pack` dropped (`Katanami.pack.json` → `Katanami`).
    pub name: String,
    /// The character directory the pack sits in — the rig it is authored against.
    pub skeleton: String,
    pub format: String,
    pub version: u32,
    pub note: String,
    pub states: usize,
    pub transitions: usize,
    /// States carrying at least one TAE event.
    pub event_states: usize,
    pub events: usize,
    /// Events carrying authored combat metadata (the Weapon signal).
    pub combat_events: usize,
    pub kind: PackKind,
}

/// Strip the `.pack.json` double extension down to a display name.
pub fn pack_display_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("pack");
    stem.strip_suffix(".pack").unwrap_or(stem).to_string()
}

/// The rig directory a pack is authored against (its parent folder name).
pub fn pack_skeleton(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("—")
        .to_string()
}

/// Classify a pack from what it actually contains. Pure — the browser's grouping is a
/// *reading* of the file, never a claim the file makes about gameplay.
pub fn classify(pack: &PackFile) -> PackKind {
    let sm = &pack.state_machine;
    let combat = sm
        .states
        .iter()
        .flat_map(|s| s.events.iter())
        .filter(|e| e.combat.is_some())
        .count();
    let events: usize = sm.states.iter().map(|s| s.events.len()).sum();
    if combat > 0 {
        PackKind::Weapon
    } else if events > 0 {
        PackKind::Ability
    } else if sm.states.len() >= LOCOMOTION_MIN_STATES {
        PackKind::Locomotion
    } else {
        PackKind::Creature
    }
}

/// A pack needs at least this many states to read as a locomotion core rather than a stub.
const LOCOMOTION_MIN_STATES: usize = 4;

/// Summarize one already-loaded pack.
pub fn summarize(path: &Path, pack: &PackFile) -> PackEntry {
    let sm = &pack.state_machine;
    let transitions: usize =
        sm.states.iter().map(|s| s.transitions.len()).sum::<usize>() + sm.any.len();
    let events: usize = sm.states.iter().map(|s| s.events.len()).sum();
    let event_states = sm.states.iter().filter(|s| !s.events.is_empty()).count();
    let combat_events = sm
        .states
        .iter()
        .flat_map(|s| s.events.iter())
        .filter(|e| e.combat.is_some())
        .count();
    PackEntry {
        path: path.to_path_buf(),
        name: pack_display_name(path),
        skeleton: pack_skeleton(path),
        format: pack.format.clone(),
        version: pack.version,
        note: pack.note.clone(),
        states: sm.states.len(),
        transitions,
        event_states,
        events,
        combat_events,
        kind: classify(pack),
    }
}

/// Find every `*.pack.json` under `root` (one level of character directories, which is how
/// `Alpha/content/characters` is laid out), newest-name-sorted for a stable grid order.
/// Unreadable packs are skipped rather than failing the whole library.
pub fn scan_packs(root: &Path) -> Vec<PackEntry> {
    let mut found: Vec<PathBuf> = Vec::new();
    collect_pack_paths(root, 0, &mut found);
    found.sort();
    let mut out: Vec<PackEntry> = found
        .iter()
        .filter_map(|p| read_pack(p).ok().map(|pack| summarize(p, &pack)))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Recurse at most `MAX_SCAN_DEPTH` directories deep — the content tree is
/// `characters/<name>/<name>.pack.json`, so this never walks the whole repo.
fn collect_pack_paths(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_pack_paths(&p, depth + 1, out);
        } else if is_pack_file(&p) {
            out.push(p);
        }
    }
}

const MAX_SCAN_DEPTH: usize = 2;

fn is_pack_file(p: &Path) -> bool {
    p.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with(".pack.json"))
}

/// Count packs of each kind, for the filter rail's per-type counts.
pub fn kind_counts(entries: &[PackEntry]) -> [usize; 4] {
    let mut counts = [0usize; 4];
    for e in entries {
        let i = PackKind::ALL.iter().position(|k| *k == e.kind).unwrap_or(3);
        counts[i] += 1;
    }
    counts
}

/// The distinct skeletons present, for the rail's skeleton filter rows.
pub fn skeletons(entries: &[PackEntry]) -> Vec<String> {
    let mut s: Vec<String> = entries.iter().map(|e| e.skeleton.clone()).collect();
    s.sort();
    s.dedup();
    s
}

/// Apply the rail's filters. An empty `kinds`/`skels` selection means "no filter", so a
/// freshly-opened browser shows everything rather than nothing.
pub fn filter<'a>(
    entries: &'a [PackEntry],
    kinds: &[PackKind],
    skels: &[String],
    search: &str,
) -> Vec<&'a PackEntry> {
    let q = search.trim().to_ascii_lowercase();
    entries
        .iter()
        .filter(|e| kinds.is_empty() || kinds.contains(&e.kind))
        .filter(|e| skels.is_empty() || skels.contains(&e.skeleton))
        .filter(|e| q.is_empty() || e.name.to_ascii_lowercase().contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_root() -> PathBuf {
        // crates/flicker-loomforge/src → repo root → Alpha/content/characters
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../Alpha/content/characters")
    }

    #[test]
    fn display_name_drops_the_double_extension() {
        assert_eq!(pack_display_name(Path::new("a/b/Katanami.pack.json")), "Katanami");
        assert_eq!(pack_skeleton(Path::new("a/katanami/Katanami.pack.json")), "katanami");
    }

    #[test]
    fn scans_the_real_content_tree() {
        let root = content_root();
        if !root.exists() {
            return; // content tree not present in this checkout
        }
        let packs = scan_packs(&root);
        assert!(!packs.is_empty(), "the content tree ships at least one .pack.json");
        // Every entry is real: a name, a skeleton, and at least one state.
        for p in &packs {
            assert!(!p.name.is_empty());
            assert!(!p.skeleton.is_empty());
            assert!(p.states > 0, "{} parsed to zero states", p.name);
            assert!(p.path.exists());
        }
        // Katanami is the rich pack the bench opens on.
        assert!(packs.iter().any(|p| p.name == "Katanami"));
    }

    #[test]
    fn counts_and_skeletons_agree_with_the_entries() {
        let root = content_root();
        if !root.exists() {
            return;
        }
        let packs = scan_packs(&root);
        assert_eq!(kind_counts(&packs).iter().sum::<usize>(), packs.len());
        assert!(skeletons(&packs).len() <= packs.len());
    }

    #[test]
    fn empty_filters_mean_show_everything() {
        let root = content_root();
        if !root.exists() {
            return;
        }
        let packs = scan_packs(&root);
        assert_eq!(filter(&packs, &[], &[], "").len(), packs.len());
        // A search that matches nothing yields nothing (and does not panic).
        assert!(filter(&packs, &[], &[], "zzz-no-such-pack").is_empty());
        // Filtering by a kind that is present keeps only that kind.
        if let Some(first) = packs.first().map(|p| p.kind) {
            let got = filter(&packs, &[first], &[], "");
            assert!(got.iter().all(|e| e.kind == first));
            assert!(!got.is_empty());
        }
    }

    #[test]
    fn classification_reads_the_contents_not_a_label() {
        let root = content_root();
        if !root.exists() {
            return;
        }
        for p in scan_packs(&root) {
            match p.kind {
                // A Weapon pack must actually carry authored combat metadata.
                PackKind::Weapon => assert!(p.combat_events > 0),
                // An Ability pack has events but no combat metadata.
                PackKind::Ability => assert!(p.events > 0 && p.combat_events == 0),
                // Locomotion is the eventless multi-state core.
                PackKind::Locomotion => {
                    assert_eq!(p.events, 0);
                    assert!(p.states >= LOCOMOTION_MIN_STATES);
                }
                PackKind::Creature => assert_eq!(p.events, 0),
            }
        }
    }
}
