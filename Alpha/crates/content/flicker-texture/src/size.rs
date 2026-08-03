//! The bake-resolution ladder — one definition of what sizes exist and which are
//! currently offered.
//!
//! **2K is the baseline** (Aaron, 2026-08-02): the default a commit bakes at, and
//! the minimum system spec the game targets. 4K and 8K are real, tested rungs —
//! the synthesizer is resolution-independent, so they cost nothing to *support* —
//! but they are **off by default** until the engine-scope memory budget is
//! settled. That decision is above this crate's pay grade, which is exactly why
//! the gate is a single named flag here rather than a condition duplicated at
//! every call site.
//!
//! # What each rung actually costs
//!
//! A bake holds the f32 field plus six RGBA8 maps at once, so peak resident cost
//! is `px² × (4 + 6×4)` = `28 × px²` bytes. Times are one three-channel patch
//! (Granite), release build, M5 MacBook Pro, single-threaded:
//!
//! | rung | maps    | peak RAM | bake time |
//! |------|---------|----------|-----------|
//! | 1K   | 24 MB   | 28 MB    | 98 ms     |
//! | 2K   | 96 MB   | 112 MB   | 364 ms    |
//! | 4K   | 384 MB  | 448 MB   | 1.7 s     |
//! | 8K   | 1536 MB | 1792 MB  | 6.9 s     |
//!
//! These are the numbers the engine-scope decision needs: 8K is not gated out of
//! caution, it is gated because one material costs 1.8 GB resident and seven
//! seconds to produce. [`BakeSize::peak_bake_bytes`] computes the memory figure,
//! so a UI can show the cost rather than assert it.
//!
//! # Nothing here runs on the frame thread
//!
//! Even the baseline is ~360 ms and the preview is ~10 ms — both far past a
//! 16 ms frame. Baking is therefore a **worker job**: the caller submits it to
//! `flicker_worker::WorkerPool`, tags the result with a generation, and draws the
//! newest one that has landed. This crate stays pure and single-threaded so that
//! stays true — a deterministic function is the thing that is safe to run on
//! another thread, and threading it here would only take that choice away from
//! the caller.

/// One rung of the ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BakeSize {
    /// Edge length in pixels. Square — every map is `px × px`.
    pub px: u32,
    /// Short label for the console (`"2K"`).
    pub label: &'static str,
    /// Whether this rung is currently offered.
    ///
    /// `false` does not mean broken: a disabled rung bakes correctly if a caller
    /// passes its `px` directly. It means "not offered in the picker yet", and it
    /// flips when the engine-level texture-memory budget lands.
    pub enabled: bool,
}

impl BakeSize {
    /// Peak bytes resident while baking at this size: the `f32` field plus six
    /// RGBA8 maps, all live at once because the derived maps read the field's
    /// neighbourhood.
    pub const fn peak_bake_bytes(&self) -> u64 {
        let px = self.px as u64;
        px * px * (4 + 6 * 4)
    }
}

/// Every rung, smallest first.
///
/// A `const` array rather than a data file: this is a canon constant of the
/// engine tier, like the element ceiling. A second copy that could disagree —
/// in a settings JSON, in the bench, in a shader — is the drift this exists to
/// prevent.
pub const BAKE_SIZES: [BakeSize; 4] = [
    // Below baseline. Kept because it is the fast rung for iterating on a recipe,
    // not because anything ships at it.
    BakeSize { px: 1024, label: "1K", enabled: true },
    // THE BASELINE — the default commit size and the minimum system spec.
    BakeSize { px: 2048, label: "2K", enabled: true },
    // Supported and correct; off until the memory budget is settled.
    BakeSize { px: 4096, label: "4K", enabled: false },
    BakeSize { px: 8192, label: "8K", enabled: false },
];

/// The size a commit bakes at unless told otherwise, and the minimum spec the
/// game targets.
pub const BAKE_DEFAULT: u32 = 2048;

/// The size the bench's live preview regenerates at while a control is moving.
///
/// Far below the bake ladder on purpose: at ~10 ms it turns around in about a
/// frame's time on a worker, so a drag stays smooth and the image trails it by a
/// frame or two rather than stuttering with it. Because the synthesizer is
/// resolution-independent, the preview is the *same image* as the commit — just
/// sampled more coarsely — so what you dial in is what you get.
pub const PREVIEW_SIZE: u32 = 256;

/// The preview must stay far cheaper than a baseline bake, or it stops being
/// live. A compile-time assertion rather than a test: both sides are constants,
/// so raising `PREVIEW_SIZE` past the line should fail the BUILD, not wait for
/// someone to run the suite.
const _: () = assert!(PREVIEW_SIZE < BAKE_DEFAULT / 4);

/// The rungs currently offered in a picker.
pub fn offered() -> impl Iterator<Item = &'static BakeSize> {
    BAKE_SIZES.iter().filter(|s| s.enabled)
}

/// The rung matching `px`, enabled or not.
pub fn rung(px: u32) -> Option<&'static BakeSize> {
    BAKE_SIZES.iter().find(|s| s.px == px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The baseline must BE a rung and must BE offered — the one fact the rest of
    /// the pipeline assumes when it bakes without being told a size.
    #[test]
    fn the_default_is_an_offered_rung() {
        let d = rung(BAKE_DEFAULT).expect("the default is on the ladder");
        assert!(d.enabled, "the default rung must be offered");
        assert_eq!(d.label, "2K", "2K is the ratified baseline");
    }

    /// The ladder is ordered and each rung doubles — a picker steps it, and a
    /// half-step would break the "same image, sampled finer" property authors
    /// rely on.
    #[test]
    fn the_ladder_is_ordered_and_doubling() {
        for pair in BAKE_SIZES.windows(2) {
            assert_eq!(pair[1].px, pair[0].px * 2, "{:?} → {:?}", pair[0], pair[1]);
        }
    }

    /// Everything at or below the baseline is offered; the gated rungs are the
    /// ones ABOVE it. Pins the ruling so a casual edit that enables 8K has to
    /// change a test that says why it was off.
    #[test]
    fn only_rungs_above_the_baseline_are_gated() {
        for s in BAKE_SIZES {
            assert_eq!(
                s.enabled,
                s.px <= BAKE_DEFAULT,
                "{} is {} — the gate is 'above the 2K baseline, pending the engine memory budget'",
                s.label,
                if s.enabled { "offered" } else { "gated" }
            );
        }
        assert_eq!(offered().count(), 2);
    }

    /// The cost figures the gating decision rests on.
    #[test]
    fn peak_bake_cost_is_what_the_gate_is_about() {
        let mb = |px: u32| rung(px).unwrap().peak_bake_bytes() / (1024 * 1024);
        assert_eq!(mb(1024), 28);
        assert_eq!(mb(2048), 112);
        assert_eq!(mb(4096), 448);
        assert_eq!(mb(8192), 1792);
    }
}
